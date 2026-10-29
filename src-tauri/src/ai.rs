// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The assistant's transport: one OpenAI-compatible chat endpoint, the
//! configuration that points at it, and the two calls the chat screen makes.
//!
//! # Why this speaks the wire itself
//!
//! `omp`'s published crates (`omp-llm`, `omp-llm-types`, `omp-llm-transport`,
//! `omp-llm-openai`, `omp-llm-error`, `omp-llm-catalog` — all MIT) were probed
//! first, because a maintained OpenAI codec would be worth a dependency. They
//! were not taken, for three reasons that a probe measured rather than guessed:
//!
//! * The family does not compile on the stable toolchain this app builds with.
//!   The least optional crate of the set, `omp-llm-types`, depends on
//!   `omp-core`, which opens with `#![feature(min_specialization)]`: a probe
//!   that only added the crates as dependencies failed to build after nine
//!   minutes with `error[E0554]: #![feature] may not be used on the stable
//!   release channel`. `omp-llm-egress` (the HTTP policy, limits and retry this
//!   module would actually want) repeats it with
//!   `#![feature(impl_trait_in_assoc_type)]`, and the `omp-llm` facade depends
//!   on egress unconditionally, so there is no smaller subset to take either.
//! * The types are a *gateway* protocol, not a client: `ChatRequest` carries a
//!   `ResolvedModelPolicy` the server is supposed to have resolved, `Thread`
//!   items carry commit sequence numbers, images arrive as BLAKE3-addressed
//!   `BlobPart`s, tool results carry ULID call ids and attribution, and the
//!   event stream is framed (Accepted/Attempt/PartStart/PartDelta/PartEnd/
//!   Invoke/Outcome) for an in-process daemon that also owns retries and
//!   credential rotation.
//! * The catalog answers for *its own* providers (`providers.toml` plus a
//!   bundled zstd model list); an arbitrary OpenAI-compatible base URL — Ollama,
//!   llama.cpp, vLLM, OpenRouter, a company gateway — is discovered from that
//!   endpoint's `/models`, not from a bundled catalog.
//!
//! Against that, the wire this app needs is four request fields, two response
//! fields, and an SSE reader; `ureq` is already here for the price providers,
//! and `serde_json` for everything else. So the transport below is our own thin
//! one, and the seam in the middle ([`HttpTransport`]) is what the tests drive.
//!
//! # What a caller gets
//!
//! [`models`] lists the endpoint's models; [`complete`] answers one turn in
//! full; [`stream`] answers the same turn as deltas through a callback and
//! returns the assembled response at the end. Nothing in this module knows
//! about Tauri: the command layer forwards [`StreamEvent`]s as events and
//! returns [`ChatResponse`]s.
//!
//! # What crosses IPC
//!
//! The shapes the assistant panel sends and receives — [`ChatRequest`],
//! [`ChatResponse`], [`ModelInfo`] and the parts they are built from — derive
//! `specta::Type`, so the panel's bindings are generated from this one
//! definition rather than a second copy of the wire. The derives are additive:
//! nothing here reads Tauri, and the JSON that goes to the endpoint is
//! unchanged.
//!
//! # The API key
//!
//! The key is the one secret this app holds, so the type carries it privately
//! and every public way out redacts it: [`AiConfig`]'s `Serialize` emits
//! `has_key` and never the key (the on-disk shape is a separate private
//! struct), `Debug` prints `<redacted>`, and no error message includes it. The
//! file is written atomically with mode 0600 on Unix.
//!
//! # The wire, exactly
//!
//! Requests are `POST {base}/chat/completions` with `model`, `messages`,
//! `tools`, `temperature`, `max_tokens`, `stream` and — when streaming —
//! `stream_options.include_usage`, so the last chunk reports tokens. `max_tokens`
//! is the field name deliberately: the endpoints this app targets (Ollama,
//! llama.cpp, vLLM, OpenRouter, DeepSeek, most gateways) accept it, while
//! `max_completion_tokens` is OpenAI's newer spelling that most compatible
//! servers ignore.
//!
//! Responses are read tolerantly where servers differ in ways a chat can
//! survive: `arguments` may be a JSON string or a JSON object, `content` may be
//! null, a model list may be a bare array, a model's context window may be
//! spelled `context_window`, `context_length`, `max_model_len` or
//! `max_context_length`, and an error may arrive as a non-2xx status *or* as a
//! 200 carrying an `error` object. Tolerating those costs a few `Option`s;
//! failing on them would strand the user on a working endpoint.
//!
//! # Images
//!
//! [`ChatRequest::media`] attaches inline images to the newest user message as
//! `image_url` parts with a `data:` URL, which is what a desktop app can build
//! from a local file without a hosting step. A data URL may be passed through
//! whole instead of base64 + mime. Media on a request whose last message is not
//! the user's is refused: an image belongs to the turn being asked, and
//! silently attaching it to an older message would misrepresent the
//! conversation to the model.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize, Serializer, ser::SerializeStruct as _};
use serde_json::Value;

/// Where the configuration lives inside the config directory the caller names
/// (the app's own, not a ledger's).
const CONFIG_FILE: &str = "ai.json";

/// The shape this module writes. A file with another version is refused by
/// name rather than read as if it were this one.
const CONFIG_VERSION: u32 = 1;

/// The base URL a fresh configuration points at. Any OpenAI-compatible service
/// can replace it; the field is the API root, so it ends in `/v1` for most.
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// The most one non-streaming response body may be: a generous answer plus
/// usage fits in a fraction of this, and a runaway endpoint cannot exhaust the
/// process.
const MAX_RESPONSE: u64 = 8 * 1024 * 1024;

/// The most an error body may be read for: enough for a provider's JSON
/// explanation, not enough for a page of HTML.
const MAX_ERROR_BODY: u64 = 64 * 1024;

/// The most one line of an SSE stream may be. A single `data:` line can carry a
/// whole tool-call argument object; anything past this is a broken or hostile
/// endpoint, not a chat.
const MAX_STREAM_LINE: usize = 8 * 1024 * 1024;

/// How much of an error body's text ends up in a message.
const DETAIL_CHARS: usize = 300;

/// How long a connect may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// How long a single read may stall — for a stream this is the longest pause
/// between chunks, and a reasoning model that thinks for minutes is still
/// inside it.
const READ_TIMEOUT: Duration = Duration::from_secs(300);

/// How long the whole non-streaming request may take.
const TOTAL_TIMEOUT: Duration = Duration::from_secs(300);

// -- configuration ------------------------------------------------------------

/// How to reach the assistant's endpoint and which model to ask.
///
/// The key is deliberately not a public field: `has_key()` answers whether one
/// is set, and [`crate::ai::AiConfig::with_api_key`] is the only way in. See
/// the module docs for the rest of the secret handling.
#[derive(Clone, PartialEq)]
pub struct AiConfig {
    base_url: String,
    api_key: String,
    model: String,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_string(),
            api_key: String::new(),
            model: String::new(),
            max_tokens: None,
            temperature: None,
        }
    }
}

impl AiConfig {
    /// The endpoint's API root, e.g. `https://api.openai.com/v1`.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The model id sent to the endpoint.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// The per-answer token ceiling, when the user set one.
    pub fn max_tokens(&self) -> Option<u32> {
        self.max_tokens
    }

    /// The sampling temperature, when the user set one.
    pub fn temperature(&self) -> Option<f32> {
        self.temperature
    }

    /// Whether a key is configured. The key itself never leaves this module as
    /// a return value or a serialized field.
    pub fn has_key(&self) -> bool {
        !self.api_key.trim().is_empty()
    }

    /// The same configuration pointing at another API root.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// The same configuration asking for another model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// The same configuration with a token ceiling, or without one for `None`.
    pub fn with_max_tokens(mut self, max_tokens: Option<u32>) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// The same configuration with a temperature, or the endpoint's default
    /// for `None`.
    pub fn with_temperature(mut self, temperature: Option<f32>) -> Self {
        self.temperature = temperature;
        self
    }

    /// The same configuration with a new key. Call this only when the user
    /// typed one: an empty string clears the stored key.
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = api_key.into();
        self
    }

    /// The key, for the request that needs it. Private on purpose: within this
    /// module the transport reads it, and no caller can hand it back out.
    fn key(&self) -> &str {
        self.api_key.trim()
    }

    /// The configuration in `dir`; see [`load`].
    pub fn load(dir: &Path) -> Result<Self, String> {
        load(dir)
    }

    /// Write this configuration into `dir`; see [`save`].
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        save(dir, self)
    }

    /// The API root as a URL, rejecting what a request cannot use.
    ///
    /// A trailing slash is dropped, and so is a pasted full endpoint
    /// (`…/chat/completions`), because that is what copying the URL out of a
    /// provider's documentation gives.
    fn base(&self) -> Result<String, Failure> {
        let base = self.base_url.trim().trim_end_matches('/');
        if base.is_empty() {
            return Err(Failure::Config(
                "no base URL is set — the OpenAI-compatible API root, e.g. https://api.openai.com/v1"
                    .to_string(),
            ));
        }
        let lower = base.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return Err(Failure::Config(format!(
                "the base URL `{base}` is not an http(s) URL"
            )));
        }
        match base.strip_suffix("/chat/completions") {
            Some(root) => Ok(root.to_string()),
            None => Ok(base.to_string()),
        }
    }

    /// Where one chat completion is posted.
    fn chat_url(&self) -> Result<String, Failure> {
        Ok(format!("{}/chat/completions", self.base()?))
    }

    /// Where the endpoint's model list lives.
    fn models_url(&self) -> Result<String, Failure> {
        Ok(format!("{}/models", self.base()?))
    }
}

/// The key is a secret: `Debug` must never put it in a log line.
impl fmt::Debug for AiConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AiConfig")
            .field("base_url", &self.base_url)
            .field("api_key", &if self.has_key() { "<set>" } else { "<unset>" })
            .field("model", &self.model)
            .field("max_tokens", &self.max_tokens)
            .field("temperature", &self.temperature)
            .finish()
    }
}

/// The frontend's view: everything but the key, plus whether one is set.
///
/// This is the only serialized shape of a configuration, so no command can
/// leak the key by returning the struct it loaded.
impl Serialize for AiConfig {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("AiConfig", 5)?;
        state.serialize_field("base_url", &self.base_url)?;
        state.serialize_field("model", &self.model)?;
        state.serialize_field("max_tokens", &self.max_tokens)?;
        state.serialize_field("temperature", &self.temperature)?;
        state.serialize_field("has_key", &self.has_key())?;
        state.end()
    }
}

/// What `ai.json` holds. Separate from [`AiConfig`] because this is the one
/// place the key is written, and it is never serialized for the frontend.
#[derive(Serialize, Deserialize)]
struct StoredConfig {
    version: u32,
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    max_tokens: Option<u32>,
    #[serde(default)]
    temperature: Option<f32>,
}

/// Where the configuration lives under `dir`.
fn config_path(dir: &Path) -> PathBuf {
    dir.join(CONFIG_FILE)
}

/// The configuration in `dir`, or the defaults when there is none yet.
///
/// A present file that cannot be read or parsed is an error, never a silent
/// reset: the file holds the user's endpoint and key, and replacing it with
/// defaults would look like the app forgot them.
pub fn load(dir: &Path) -> Result<AiConfig, String> {
    let path = config_path(dir);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(AiConfig::default()),
        Err(error) => return Err(format!("cannot read `{}`: {error}", path.display())),
    };
    let stored: StoredConfig = serde_json::from_str(&text)
        .map_err(|error| format!("`{}` is not an assistant config: {error}", path.display()))?;
    if stored.version != CONFIG_VERSION {
        return Err(format!(
            "`{}` is version {}, but this app reads version {CONFIG_VERSION}",
            path.display(),
            stored.version
        ));
    }
    Ok(AiConfig {
        base_url: stored.base_url,
        api_key: stored.api_key,
        model: stored.model,
        max_tokens: stored.max_tokens,
        temperature: stored.temperature,
    })
}

/// Write the configuration into `dir`, replacing any previous one in one step.
///
/// The file holds a secret, so it is written through a temporary file created
/// with mode 0600 (Unix) and renamed over its destination; the rename is atomic
/// and the mode is applied to the destination too, so an earlier file written
/// with different permissions ends up private as well.
pub fn save(dir: &Path, config: &AiConfig) -> Result<(), String> {
    fs::create_dir_all(dir)
        .map_err(|error| format!("cannot create `{}`: {error}", dir.display()))?;
    let stored = StoredConfig {
        version: CONFIG_VERSION,
        base_url: config.base_url.clone(),
        api_key: config.api_key.clone(),
        model: config.model.clone(),
        max_tokens: config.max_tokens,
        temperature: config.temperature,
    };
    let mut text = serde_json::to_string_pretty(&stored)
        .map_err(|error| format!("cannot render the assistant config: {error}"))?;
    text.push('\n');
    let path = config_path(dir);
    let temp = dir.join(format!(
        "{CONFIG_FILE}.tmp-{}-{}",
        std::process::id(),
        temp_counter()
    ));
    let written = create_private(&temp).and_then(|mut file| {
        file.write_all(text.as_bytes())?;
        // On the device before the rename, so the rename never points at a
        // file whose bytes are still in a cache.
        file.sync_all()
    });
    if let Err(error) = written {
        let _ = fs::remove_file(&temp);
        return Err(format!("cannot write `{}`: {error}", temp.display()));
    }
    if let Err(error) = fs::rename(&temp, &path) {
        let _ = fs::remove_file(&temp);
        return Err(format!("cannot replace `{}`: {error}", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if let Err(error) = fs::set_permissions(&path, fs::Permissions::from_mode(0o600)) {
            return Err(format!(
                "cannot make `{}` private: {error}",
                path.display()
            ));
        }
    }
    Ok(())
}

/// Create the temporary file, private from its first byte.
///
/// `create_new` refuses an existing path, so a symlink planted at the temporary
/// name is never followed. A leftover file from a process that had this pid
/// before is the one thing that refuses a legitimate write, so it is cleared
/// once and the create retried.
fn create_private(path: &Path) -> io::Result<File> {
    fn options() -> OpenOptions {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            // The mode only applies when the file is created, which is exactly
            // the window the key sits in before the rename.
            options.mode(0o600);
        }
        options
    }
    match options().open(path) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            fs::remove_file(path)?;
            options().open(path)
        }
        other => other,
    }
}

/// A number no other write in this process is using.
fn temp_counter() -> u32 {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Whether a file is private, for the test that pins the mode down.
#[cfg(all(unix, test))]
fn file_mode_is_private(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    fs::metadata(path)
        .map(|meta| meta.permissions().mode() & 0o077 == 0)
        .unwrap_or(false)
}

// -- public request and response shapes ---------------------------------------

/// Who authored a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    /// The application's instructions.
    System,
    /// The person using the app.
    User,
    /// The model.
    Assistant,
    /// A tool's answer to an earlier call.
    Tool,
}

/// One turn of the conversation, in the shape the endpoint accepts.
///
/// An assistant message that asked for tools keeps them in `tool_calls`; each
/// answer is a [`ChatRole::Tool`] message naming the call it answers in
/// `tool_call_id`. Sending them back verbatim is what makes a tool loop a
/// conversation rather than a re-prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct ChatMessage {
    pub role: ChatRole,
    /// The message's text. Empty is valid — an assistant turn that only calls
    /// tools has no prose.
    #[serde(default)]
    pub text: String,
    /// The calls this message (an assistant turn) made.
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    /// The call this message answers, for [`ChatRole::Tool`] messages.
    #[serde(default)]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    /// A message with text and nothing else.
    pub fn text(role: ChatRole, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// The answer a tool gives to the call `id`.
    pub fn tool_result(call_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            role: ChatRole::Tool,
            text: text.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(call_id.into()),
        }
    }
}

/// One tool the model may call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct ToolSchema {
    /// The dispatch name the model emits.
    pub name: String,
    /// What the tool does, for the model to read.
    pub description: String,
    /// The tool's arguments as a JSON Schema object.
    ///
    /// `specta` has no JSON-value type of its own — its `serde_json` feature
    /// would describe `Value` as serde's *internally tagged* enum, a shape this
    /// field never puts on the wire — so the generated TypeScript says
    /// `unknown` and the panel hands the object through as it received it.
    #[specta(type = specta_typescript::Unknown)]
    pub parameters: Value,
}

/// One call the model asked for, or one the caller replays back.
///
/// `arguments` stays the JSON *text* the wire carries, not a parsed value: a
/// replayed call must be byte-identical to what the model produced, and a
/// model's arguments can be malformed JSON that still has to round-trip so the
/// caller can show the failure.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ToolCall {
    /// The provider's id for this call, echoed back with the result.
    pub id: String,
    /// The tool's name.
    pub name: String,
    /// The arguments as JSON text.
    pub arguments: String,
}

/// How much detail the endpoint should spend on an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum ImageDetail {
    /// Let the endpoint choose.
    Auto,
    /// A cheaper, coarser read.
    Low,
    /// The full image.
    High,
}

impl ImageDetail {
    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Low => "low",
            Self::High => "high",
        }
    }
}

/// One image attached to the turn being asked.
///
/// The payload is base64 — the Tauri layer reads the file and encodes it — or a
/// complete `data:` URL in `data_base64`, which is passed through when the
/// caller already has one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct MediaPart {
    /// The image's media type, e.g. `image/png`.
    pub mime: String,
    /// The image as base64, or a full `data:` URL.
    pub data_base64: String,
    /// How much detail to spend on it.
    #[serde(default)]
    pub detail: Option<ImageDetail>,
}

impl MediaPart {
    /// An image from its bytes.
    pub fn inline(mime: impl Into<String>, bytes: &[u8]) -> Self {
        Self {
            mime: mime.into(),
            data_base64: BASE64.encode(bytes),
            detail: None,
        }
    }

    /// The same image, with a detail hint.
    pub fn with_detail(mut self, detail: ImageDetail) -> Self {
        self.detail = Some(detail);
        self
    }

    /// The `data:` URL the wire wants.
    fn data_url(&self) -> String {
        let payload = self.data_base64.trim();
        if payload.starts_with("data:") {
            return payload.to_string();
        }
        format!("data:{};base64,{}", self.mime.trim(), payload)
    }
}

/// One turn's worth of input.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct ChatRequest {
    /// The conversation, oldest first.
    pub messages: Vec<ChatMessage>,
    /// The tools the model may call.
    #[serde(default)]
    pub tools: Vec<ToolSchema>,
    /// Images for the newest user message.
    #[serde(default)]
    pub media: Vec<MediaPart>,
}

/// Token accounting, when the endpoint reports it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct Usage {
    /// Prompt-side tokens.
    pub prompt_tokens: u32,
    /// Generated tokens.
    pub completion_tokens: u32,
    /// Both, as the endpoint counted them.
    pub total_tokens: u32,
}

/// One finished turn.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct ChatResponse {
    /// The prose the model produced.
    pub text: String,
    /// The calls the model asked for.
    pub tool_calls: Vec<ToolCall>,
    /// Tokens used, when the endpoint said.
    pub usage: Option<Usage>,
    /// Why generation stopped, in the endpoint's words (`stop`, `tool_calls`,
    /// `length`, …).
    pub finish_reason: Option<String>,
    /// Whether the caller stopped the stream early. A cancelled response holds
    /// the deltas produced so far.
    pub cancelled: bool,
}

/// One delta of a streamed turn, as the command layer forwards it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamEvent {
    /// More prose.
    TextDelta {
        /// The fragment, ready to append.
        text: String,
    },
    /// More of a tool call: its id and name arrive once, its arguments in
    /// fragments, all keyed by the call's position.
    ToolCallDelta {
        /// The call's position in the answer.
        index: u32,
        /// The provider's id, on the fragment that first announces it.
        id: Option<String>,
        /// The tool's name, on the fragment that first announces it.
        name: Option<String>,
        /// The fragment of the arguments' JSON text.
        arguments: String,
    },
    /// The endpoint's token accounting for the finished turn.
    Usage {
        /// Prompt-side tokens.
        prompt_tokens: u32,
        /// Generated tokens.
        completion_tokens: u32,
        /// Both.
        total_tokens: u32,
    },
    /// The stream is over.
    Done {
        /// Why generation stopped, when the endpoint said.
        finish_reason: Option<String>,
    },
}

/// One model the endpoint offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ModelInfo {
    /// The id to put in [`AiConfig::with_model`].
    pub id: String,
    /// The context window, when the endpoint publishes one.
    pub context_window: Option<u32>,
    /// Whether the model takes images, when the endpoint says.
    pub vision: Option<bool>,
}

// -- failures -----------------------------------------------------------------

/// What went wrong, classified the way a chat screen reacts to it.
///
/// The public functions answer with strings (the command layer's error type);
/// this is the classification behind those strings, and the thing the tests
/// pin down.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Failure {
    /// The configuration cannot make a request at all.
    Config(String),
    /// The request is not a valid conversation.
    Request(String),
    /// 401 or 403: the key is missing, wrong, or not allowed.
    Unauthorized { status: u16, detail: String },
    /// 429: slow down; `retry_after` is the endpoint's own advice, in seconds.
    RateLimited {
        retry_after: Option<u64>,
        detail: String,
    },
    /// 5xx: the endpoint's own fault.
    Server { status: u16, detail: String },
    /// Another non-2xx status, with the endpoint's explanation when it sent one.
    Rejected { status: u16, detail: String },
    /// A 2xx that still carries an error (or an error frame mid-stream).
    Provider(String),
    /// A 2xx whose body is not the JSON this module reads.
    Malformed(String),
    /// The request never reached a 2xx: DNS, TLS, connection, timeouts.
    Transport(String),
}

impl Failure {
    /// The message the command layer shows.
    fn message(&self) -> String {
        match self {
            Self::Config(detail) => format!("the assistant is not configured: {detail}"),
            Self::Request(detail) => format!("the request cannot be sent: {detail}"),
            Self::Unauthorized { status, detail } => {
                format!("the endpoint rejected the API key ({status}){}", suffix(detail))
            }
            Self::RateLimited {
                retry_after,
                detail,
            } => match retry_after {
                Some(seconds) => format!(
                    "the endpoint is rate limiting requests; try again in {seconds}s{}",
                    suffix(detail)
                ),
                None => format!("the endpoint is rate limiting requests{}", suffix(detail)),
            },
            Self::Server { status, detail } => format!(
                "the endpoint returned a server error ({status}){}",
                suffix(detail)
            ),
            Self::Rejected { status, detail } => {
                format!("the endpoint rejected the request ({status}){}", suffix(detail))
            }
            Self::Provider(detail) => format!("the endpoint reported an error: {detail}"),
            Self::Malformed(detail) => {
                format!("the endpoint's answer could not be read: {detail}")
            }
            Self::Transport(detail) => format!("could not reach the endpoint: {detail}"),
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message())
    }
}

/// `: <detail>` for a non-empty detail, nothing otherwise.
fn suffix(detail: &str) -> String {
    if detail.is_empty() {
        String::new()
    } else {
        format!(": {detail}")
    }
}

/// The text an error body carries, cut to something a notification can hold.
fn error_detail(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // A provider's JSON explanation beats the raw body it arrived in.
    if let Ok(envelope) = serde_json::from_str::<WireResponse>(trimmed)
        && let Some(error) = envelope.error
    {
        let text = error.text();
        if !text.is_empty() {
            return truncate(&text);
        }
    }
    truncate(trimmed)
}

/// `text` with at most [`DETAIL_CHARS`] characters, on a character boundary.
fn truncate(text: &str) -> String {
    let mut cut: String = text.chars().take(DETAIL_CHARS).collect();
    if text.chars().count() > DETAIL_CHARS {
        cut.push('…');
    }
    cut
}

/// Classify a non-2xx status, keeping the endpoint's own explanation.
fn classify(status: u16, retry_after: Option<u64>, body: &str) -> Failure {
    let detail = error_detail(body);
    match status {
        401 | 403 => Failure::Unauthorized { status, detail },
        429 => Failure::RateLimited {
            retry_after,
            detail,
        },
        500..=599 => Failure::Server { status, detail },
        _ => Failure::Rejected { status, detail },
    }
}

/// A 2xx body that is not JSON, with the first of it for the message.
fn malformed(body: &str, error: &serde_json::Error) -> Failure {
    let snippet = truncate(body.trim());
    if snippet.is_empty() {
        Failure::Malformed(format!("the body is empty ({error})"))
    } else {
        Failure::Malformed(format!("{error}; the body begins `{snippet}`"))
    }
}

// -- the HTTP seam ------------------------------------------------------------

/// Which verb an exchange uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    Get,
    Post,
}

/// One HTTP exchange, as the client needs it.
///
/// This, and [`HttpReply`], are the seam the tests replace: everything above
/// them is mapping and everything below them is one library call.
#[derive(Debug)]
struct HttpCall<'a> {
    method: Method,
    url: &'a str,
    /// The bearer token, empty when the endpoint needs none (a local server).
    api_key: &'a str,
    body: Option<&'a [u8]>,
    /// Whether the answer is an event stream — which sets `Accept` and lifts
    /// the whole-request deadline.
    stream: bool,
}

/// A response's status and body; the body is read lazily so a stream is not
/// buffered whole.
struct HttpReply {
    status: u16,
    /// The `Retry-After` header, in seconds, when the endpoint sent one.
    retry_after: Option<u64>,
    body: Box<dyn Read + Send>,
}

/// The one thing a test has to fake.
trait HttpTransport {
    fn send(&self, call: HttpCall<'_>) -> Result<HttpReply, String>;
}

/// `ureq`, blocking, with one shared agent so connections and TLS sessions are
/// reused across calls.
struct UreqTransport;

impl HttpTransport for UreqTransport {
    fn send(&self, call: HttpCall<'_>) -> Result<HttpReply, String> {
        let mut request = match call.method {
            Method::Get => AGENT.get(call.url),
            Method::Post => AGENT.post(call.url),
        };
        request = request.set("Content-Type", "application/json");
        if !call.api_key.is_empty() {
            request = request.set("Authorization", &format!("Bearer {}", call.api_key));
        }
        if call.stream {
            request = request.set("Accept", "text/event-stream");
        } else {
            // A whole answer arrives in one body, so the request as a whole has
            // a deadline; a stream's deadline is per read instead, on the agent.
            request = request
                .set("Accept", "application/json")
                .timeout(TOTAL_TIMEOUT);
        }
        let result = match call.body {
            Some(body) => request.send_bytes(body),
            None => request.call(),
        };
        match result {
            Ok(response) => reply(response),
            // A non-2xx is an answer, not a transport failure: its body is what
            // classification reads.
            Err(ureq::Error::Status(_, response)) => reply(response),
            Err(error) => Err(error.to_string()),
        }
    }
}

/// The shared agent: timeouts that hold for every call, and connection reuse.
static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .timeout_write(READ_TIMEOUT)
        .build()
});

/// A `ureq` response as this module's reply.
fn reply(response: ureq::Response) -> Result<HttpReply, String> {
    let status = response.status();
    let retry_after = response
        .header("retry-after")
        .and_then(|value| value.trim().parse::<u64>().ok());
    let body: Box<dyn Read + Send> = Box::new(response.into_reader());
    Ok(HttpReply {
        status,
        retry_after,
        body,
    })
}

// -- the public calls ---------------------------------------------------------

/// The endpoint's models, by id.
///
/// Most OpenAI-compatible servers answer `/models` with ids alone; the ones
/// that also publish a context window or an image capability are read too, and
/// anything unknown stays `None` rather than being guessed from the id's shape.
pub fn models(config: &AiConfig) -> Result<Vec<ModelInfo>, String> {
    models_with(&UreqTransport, config).map_err(|failure| failure.message())
}

/// One turn, answered in full.
pub fn complete(config: &AiConfig, request: ChatRequest) -> Result<ChatResponse, String> {
    complete_with(&UreqTransport, config, request).map_err(|failure| failure.message())
}

/// One turn, answered as it is written.
///
/// Each delta goes to `on_event`; returning [`ControlFlow::Break`] stops the
/// read at once and answers a [`ChatResponse`] with `cancelled` set and the
/// text produced so far. The assembled response is the same shape [`complete`]
/// answers, so a caller can treat both paths alike.
pub fn stream(
    config: &AiConfig,
    request: ChatRequest,
    on_event: impl FnMut(StreamEvent) -> ControlFlow<()>,
) -> Result<ChatResponse, String> {
    stream_with(&UreqTransport, config, request, on_event).map_err(|failure| failure.message())
}

fn models_with(
    transport: &impl HttpTransport,
    config: &AiConfig,
) -> Result<Vec<ModelInfo>, Failure> {
    let url = config.models_url()?;
    let reply = transport
        .send(HttpCall {
            method: Method::Get,
            url: &url,
            api_key: config.key(),
            body: None,
            stream: false,
        })
        .map_err(Failure::Transport)?;
    let status = reply.status;
    let retry_after = reply.retry_after;
    let body = read_text(reply.body, MAX_RESPONSE)?;
    if !(200..300).contains(&status) {
        return Err(classify(status, retry_after, &body));
    }
    let list: WireModelList =
        serde_json::from_str(&body).map_err(|error| malformed(&body, &error))?;
    Ok(list.into_models())
}

fn complete_with(
    transport: &impl HttpTransport,
    config: &AiConfig,
    request: ChatRequest,
) -> Result<ChatResponse, Failure> {
    run(transport, config, request, false, &mut |_| ControlFlow::Continue(()))
}

fn stream_with(
    transport: &impl HttpTransport,
    config: &AiConfig,
    request: ChatRequest,
    mut on_event: impl FnMut(StreamEvent) -> ControlFlow<()>,
) -> Result<ChatResponse, Failure> {
    run(transport, config, request, true, &mut on_event)
}

/// The one request path: build, send, classify, read — the only difference
/// between a whole answer and a stream being the reader.
fn run(
    transport: &impl HttpTransport,
    config: &AiConfig,
    request: ChatRequest,
    stream: bool,
    on_event: &mut dyn FnMut(StreamEvent) -> ControlFlow<()>,
) -> Result<ChatResponse, Failure> {
    let url = config.chat_url()?;
    let body = request_body(config, &request, stream)?;
    let reply = transport
        .send(HttpCall {
            method: Method::Post,
            url: &url,
            api_key: config.key(),
            body: Some(&body),
            stream,
        })
        .map_err(Failure::Transport)?;
    let status = reply.status;
    let retry_after = reply.retry_after;
    if !(200..300).contains(&status) {
        let body = read_text(reply.body, MAX_ERROR_BODY)?;
        return Err(classify(status, retry_after, &body));
    }
    if stream {
        consume_stream(reply.body, on_event)
    } else {
        let body = read_text(reply.body, MAX_RESPONSE)?;
        parse_response(&body)
    }
}

/// A reader's whole contents as text, bounded by `limit`.
///
/// Lossy rather than strict: an endpoint's error page in some legacy encoding
/// is still worth showing, and a chat's own body is UTF-8 by contract.
fn read_text(reader: Box<dyn Read + Send>, limit: u64) -> Result<String, Failure> {
    let mut bytes = Vec::new();
    reader
        .take(limit)
        .read_to_end(&mut bytes)
        .map_err(|error| Failure::Transport(format!("could not read the answer: {error}")))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

// -- requests -----------------------------------------------------------------

/// The JSON body of one request.
fn request_body(
    config: &AiConfig,
    request: &ChatRequest,
    stream: bool,
) -> Result<Vec<u8>, Failure> {
    if config.model.trim().is_empty() {
        return Err(Failure::Config(
            "no model is set — choose one for the assistant".to_string(),
        ));
    }
    if request.messages.is_empty() {
        return Err(Failure::Request("the conversation has no messages".to_string()));
    }
    let messages = wire_messages(request)?;
    let tools: Vec<WireTool> = request
        .tools
        .iter()
        .map(|tool| WireTool {
            kind: "function",
            function: WireToolFunction {
                name: &tool.name,
                description: &tool.description,
                parameters: &tool.parameters,
            },
        })
        .collect();
    let body = WireRequest {
        model: config.model.trim(),
        messages: &messages,
        tools: &tools,
        temperature: config.temperature,
        max_tokens: config.max_tokens,
        stream,
        stream_options: stream.then_some(WireStreamOptions { include_usage: true }),
    };
    serde_json::to_vec(&body)
        .map_err(|error| Failure::Request(format!("cannot render the request: {error}")))
}

/// The conversation as the wire takes it, with the media attached to the turn
/// being asked.
fn wire_messages(request: &ChatRequest) -> Result<Vec<WireMessage<'_>>, Failure> {
    let media_at = if request.media.is_empty() {
        None
    } else {
        let last = request
            .messages
            .last()
            .filter(|message| message.role == ChatRole::User);
        Some(match last {
            Some(_) => request.messages.len() - 1,
            None => {
                return Err(Failure::Request(
                    "images can only be attached to a request whose last message is the user's"
                        .to_string(),
                ));
            }
        })
    };
    let mut messages = Vec::with_capacity(request.messages.len());
    for (index, message) in request.messages.iter().enumerate() {
        if message.role == ChatRole::Tool && message.tool_call_id.as_deref().unwrap_or("").is_empty()
        {
            return Err(Failure::Request(
                "a tool result needs the id of the call it answers".to_string(),
            ));
        }
        let content = match media_at {
            Some(at) if at == index => {
                let mut parts = Vec::with_capacity(request.media.len() + 1);
                if !message.text.is_empty() {
                    parts.push(WirePart::Text { text: &message.text });
                }
                for media in &request.media {
                    parts.push(WirePart::ImageUrl {
                        image_url: WireImageUrl {
                            url: media.data_url(),
                            detail: media.detail.map(ImageDetail::as_str),
                        },
                    });
                }
                WireContent::Parts(parts)
            }
            _ => WireContent::Text(&message.text),
        };
        let tool_calls: Vec<WireCall> = message
            .tool_calls
            .iter()
            .map(|call| WireCall {
                id: &call.id,
                kind: "function",
                function: WireCallFunction {
                    name: &call.name,
                    arguments: &call.arguments,
                },
            })
            .collect();
        messages.push(WireMessage {
            role: role_name(message.role),
            content,
            tool_calls,
            tool_call_id: message.tool_call_id.as_deref(),
        });
    }
    Ok(messages)
}

/// The wire's name for a role.
fn role_name(role: ChatRole) -> &'static str {
    match role {
        ChatRole::System => "system",
        ChatRole::User => "user",
        ChatRole::Assistant => "assistant",
        ChatRole::Tool => "tool",
    }
}

#[derive(Serialize)]
struct WireRequest<'a> {
    model: &'a str,
    messages: &'a [WireMessage<'a>],
    #[serde(skip_serializing_if = "slice_is_empty")]
    tools: &'a [WireTool<'a>],
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<WireStreamOptions>,
}

#[derive(Serialize)]
struct WireStreamOptions {
    include_usage: bool,
}

#[derive(Serialize)]
struct WireMessage<'a> {
    role: &'a str,
    content: WireContent<'a>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<WireCall<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
}

/// A message's content: plain text, or text and images.
#[derive(Serialize)]
#[serde(untagged)]
enum WireContent<'a> {
    Text(&'a str),
    Parts(Vec<WirePart<'a>>),
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WirePart<'a> {
    Text {
        text: &'a str,
    },
    ImageUrl {
        image_url: WireImageUrl<'a>,
    },
}

#[derive(Serialize)]
struct WireImageUrl<'a> {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a str>,
}

#[derive(Serialize)]
struct WireTool<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    function: WireToolFunction<'a>,
}

#[derive(Serialize)]
struct WireToolFunction<'a> {
    name: &'a str,
    description: &'a str,
    parameters: &'a Value,
}

/// A call as the wire carries it, in either direction.
#[derive(Serialize)]
struct WireCall<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: &'a str,
    function: WireCallFunction<'a>,
}

#[derive(Serialize)]
struct WireCallFunction<'a> {
    name: &'a str,
    arguments: &'a str,
}

/// `skip_serializing_if` for borrowed slices.
fn slice_is_empty<T>(slice: &&[T]) -> bool {
    slice.is_empty()
}

// -- responses ----------------------------------------------------------------

/// A whole answer.
#[derive(Deserialize)]
struct WireResponse {
    #[serde(default)]
    choices: Vec<WireChoice>,
    #[serde(default)]
    usage: Option<WireUsage>,
    #[serde(default)]
    error: Option<WireError>,
}

#[derive(Deserialize, Default)]
struct WireChoice {
    #[serde(default)]
    message: WireAssistant,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct WireAssistant {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<WireIncomingCall>,
}

#[derive(Deserialize, Default)]
struct WireIncomingCall {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<WireIncomingFunction>,
}

#[derive(Deserialize, Default)]
struct WireIncomingFunction {
    #[serde(default)]
    name: Option<String>,
    /// A JSON string on OpenAI and most compatible servers, but a JSON object
    /// on some (Ollama and a few proxies), so both are accepted.
    #[serde(default)]
    arguments: Option<WireArguments>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WireArguments {
    Text(String),
    Value(Value),
}

impl WireArguments {
    fn text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Value(value) => value.to_string(),
        }
    }
}

#[derive(Deserialize, Default)]
struct WireUsage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
    #[serde(default)]
    total_tokens: Option<u64>,
}

impl WireUsage {
    /// The same counts narrowed to what a chat turn can hold.
    fn usage(&self) -> Usage {
        let prompt = narrow(self.prompt_tokens);
        let completion = narrow(self.completion_tokens);
        Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: match self.total_tokens {
                Some(total) => narrow(Some(total)),
                None => prompt.saturating_add(completion),
            },
        }
    }
}

/// A count as a `u32`; an implausible one saturates rather than failing the
/// whole answer.
fn narrow(value: Option<u64>) -> u32 {
    value.map_or(0, |value| u32::try_from(value).unwrap_or(u32::MAX))
}

/// An error object, as OpenAI and most compatible servers send one.
#[derive(Deserialize)]
#[serde(untagged)]
enum WireError {
    Object {
        #[serde(default)]
        message: Option<String>,
        #[serde(default)]
        code: Option<Value>,
        #[serde(default)]
        r#type: Option<String>,
    },
    Text(String),
}

impl WireError {
    /// The most useful sentence the object holds.
    fn text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Object {
                message,
                code,
                r#type,
            } => {
                let mut text = message.clone().unwrap_or_default();
                let mut note = |label: &str, value: &str| {
                    if value.is_empty() {
                        return;
                    }
                    if !text.is_empty() {
                        text.push_str(" — ");
                    }
                    text.push_str(label);
                    text.push_str(": ");
                    text.push_str(value);
                };
                if let Some(kind) = r#type.as_deref() {
                    note("type", kind);
                }
                if let Some(code) = code.as_ref().and_then(scalar_text) {
                    note("code", &code);
                }
                text
            }
        }
    }
}

/// A JSON scalar as text, for codes that are numbers as often as strings.
fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// Read one whole answer.
fn parse_response(body: &str) -> Result<ChatResponse, Failure> {
    let response: WireResponse =
        serde_json::from_str(body).map_err(|error| malformed(body, &error))?;
    if let Some(error) = response.error {
        return Err(Failure::Provider(error.text()));
    }
    let choice = response
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| Failure::Malformed("the answer has no choices".to_string()))?;
    let tool_calls = choice
        .message
        .tool_calls
        .into_iter()
        .map(|call| {
            let function = call.function.unwrap_or_default();
            ToolCall {
                id: call.id.unwrap_or_default(),
                name: function.name.unwrap_or_default(),
                arguments: function.arguments.map(|arguments| arguments.text()).unwrap_or_default(),
            }
        })
        .collect();
    Ok(ChatResponse {
        text: choice.message.content.unwrap_or_default(),
        tool_calls,
        usage: response.usage.as_ref().map(WireUsage::usage),
        finish_reason: choice.finish_reason,
        cancelled: false,
    })
}

// -- streaming ----------------------------------------------------------------

/// One chunk of a stream.
#[derive(Deserialize)]
struct WireChunk {
    #[serde(default)]
    choices: Vec<WireChunkChoice>,
    #[serde(default)]
    usage: Option<WireUsage>,
    #[serde(default)]
    error: Option<WireError>,
}

#[derive(Deserialize, Default)]
struct WireChunkChoice {
    #[serde(default)]
    delta: WireDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct WireDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<WireDeltaCall>,
}

#[derive(Deserialize, Default)]
struct WireDeltaCall {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<WireIncomingFunction>,
}

/// A tool call as it is being streamed.
#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
}

/// The assembled answer while a stream is read.
#[derive(Default)]
struct Assembled {
    text: String,
    calls: BTreeMap<u32, PartialCall>,
    usage: Option<Usage>,
    finish_reason: Option<String>,
    cancelled: bool,
}

impl Assembled {
    /// The answer as a caller of [`complete`] would see it.
    fn into_response(self) -> ChatResponse {
        ChatResponse {
            text: self.text,
            tool_calls: self
                .calls
                .into_values()
                .map(|call| ToolCall {
                    id: call.id,
                    name: call.name,
                    arguments: call.arguments,
                })
                .collect(),
            usage: self.usage,
            finish_reason: self.finish_reason,
            cancelled: self.cancelled,
        }
    }
}

/// How one event left the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    /// More events follow.
    Continue,
    /// The endpoint said `[DONE]`: stop reading, even if the socket stays open.
    End,
    /// The caller stopped watching: stop reading and keep what arrived.
    Cancel,
}

/// Read one event stream to its end (or to the caller's break).
fn consume_stream(
    reader: Box<dyn Read + Send>,
    on_event: &mut dyn FnMut(StreamEvent) -> ControlFlow<()>,
) -> Result<ChatResponse, Failure> {
    let mut assembled = Assembled::default();
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    // An SSE event's `data:` lines are joined with newlines until a blank line
    // dispatches it. OpenAI puts each JSON on one line; assembling anyway keeps
    // a server that wraps one across lines working.
    let mut data = String::new();
    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| Failure::Transport(format!("the stream broke: {error}")))?;
        if read == 0 {
            // The connection ended without `[DONE]`: take whatever the last
            // event said.
            if !data.trim().is_empty() {
                apply(&data, &mut assembled, on_event)?;
            }
            break;
        }
        if line.len() > MAX_STREAM_LINE {
            return Err(Failure::Malformed(format!(
                "the stream sent a line longer than {MAX_STREAM_LINE} bytes"
            )));
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches(['\r', '\n']);
        if text.is_empty() {
            if data.trim().is_empty() {
                continue;
            }
            let flow = apply(&data, &mut assembled, on_event)?;
            data.clear();
            if flow != Flow::Continue {
                break;
            }
            continue;
        }
        if let Some(rest) = text.strip_prefix("data:") {
            // One optional space after the colon is the SSE framing, not data.
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest);
        }
        // `event:`, `id:`, `retry:` and comment lines carry nothing this client
        // reads; the JSON in `data:` is the whole protocol here.
    }
    // A cancelled stream is already over as far as the caller is concerned;
    // there is no `Done` to announce.
    if !assembled.cancelled {
        // Nothing is left to read, so the caller's answer to `Done` cannot
        // stop anything.
        let _ = on_event(StreamEvent::Done {
            finish_reason: assembled.finish_reason.clone(),
        });
    }
    Ok(assembled.into_response())
}

/// Handle one event's data: a chunk of answer, the end of the stream, or a
/// provider error.
fn apply(
    data: &str,
    assembled: &mut Assembled,
    on_event: &mut dyn FnMut(StreamEvent) -> ControlFlow<()>,
) -> Result<Flow, Failure> {
    let data = data.trim();
    if data == "[DONE]" {
        return Ok(Flow::End);
    }
    let chunk: WireChunk = serde_json::from_str(data).map_err(|error| malformed(data, &error))?;
    if let Some(error) = chunk.error {
        return Err(Failure::Provider(error.text()));
    }
    if let Some(usage) = chunk.usage {
        let usage = usage.usage();
        assembled.usage = Some(usage);
        if on_event(StreamEvent::Usage {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
        }) == ControlFlow::Break(())
        {
            assembled.cancelled = true;
            return Ok(Flow::Cancel);
        }
    }
    for choice in chunk.choices {
        if let Some(reason) = choice.finish_reason {
            assembled.finish_reason = Some(reason);
        }
        if let Some(text) = choice.delta.content
            && !text.is_empty()
        {
            assembled.text.push_str(&text);
            if on_event(StreamEvent::TextDelta { text: text.clone() }) == ControlFlow::Break(())
            {
                assembled.cancelled = true;
                return Ok(Flow::Cancel);
            }
        }
        for call in choice.delta.tool_calls {
            let entry = assembled.calls.entry(call.index).or_default();
            let function = call.function.unwrap_or_default();
            let id = call.id.filter(|id| !id.is_empty());
            let name = function.name.filter(|name| !name.is_empty());
            let arguments = function
                .arguments
                .map(|arguments| arguments.text())
                .unwrap_or_default();
            if let Some(id) = id.clone()
                && entry.id.is_empty()
            {
                entry.id = id;
            }
            if let Some(name) = name.clone()
                && entry.name.is_empty()
            {
                entry.name = name;
            }
            entry.arguments.push_str(&arguments);
            if on_event(StreamEvent::ToolCallDelta {
                index: call.index,
                id,
                name,
                arguments,
            }) == ControlFlow::Break(())
            {
                assembled.cancelled = true;
                return Ok(Flow::Cancel);
            }
        }
    }
    Ok(Flow::Continue)
}

// -- models -------------------------------------------------------------------

/// The endpoint's model list: an envelope with `data`, or a bare array.
#[derive(Deserialize)]
#[serde(untagged)]
enum WireModelList {
    Envelope { data: Vec<WireModel> },
    Bare(Vec<WireModel>),
}

impl WireModelList {
    fn into_models(self) -> Vec<ModelInfo> {
        let models = match self {
            Self::Envelope { data } => data,
            Self::Bare(models) => models,
        };
        // Keyed by id, so a server that lists one twice answers once (the first
        // entry is the one the endpoint advertises prominently); ordered by id,
        // so the picker does not shuffle between calls.
        let mut by_id: BTreeMap<String, ModelInfo> = BTreeMap::new();
        for model in models {
            if let Some(info) = model.info() {
                by_id.entry(info.id.clone()).or_insert(info);
            }
        }
        by_id.into_values().collect()
    }
}

#[derive(Deserialize)]
struct WireModel {
    #[serde(default)]
    id: Option<String>,
    /// Ollama's older spelling in `/api/tags`; harmless here.
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    context_window: Option<u64>,
    #[serde(default)]
    context_length: Option<u64>,
    #[serde(default)]
    max_model_len: Option<u64>,
    #[serde(default)]
    max_context_length: Option<u64>,
    #[serde(default)]
    supports_vision: Option<bool>,
    #[serde(default)]
    vision: Option<bool>,
    #[serde(default)]
    architecture: Option<WireArchitecture>,
}

#[derive(Deserialize)]
struct WireArchitecture {
    #[serde(default)]
    input_modalities: Option<Vec<String>>,
}

impl WireModel {
    fn info(&self) -> Option<ModelInfo> {
        let id = self
            .id
            .as_deref()
            .or(self.name.as_deref())
            .map(str::trim)
            .filter(|id| !id.is_empty())?;
        Some(ModelInfo {
            id: id.to_string(),
            context_window: self
                .context_window
                .or(self.context_length)
                .or(self.max_model_len)
                .or(self.max_context_length)
                .and_then(|window| u32::try_from(window).ok()),
            vision: self.vision(),
        })
    }

    /// The image capability, when the endpoint states one outright; otherwise
    /// from OpenRouter's `architecture.input_modalities`, where "image" means
    /// the model takes pictures.
    fn vision(&self) -> Option<bool> {
        if let Some(vision) = self.supports_vision {
            return Some(vision);
        }
        if let Some(vision) = self.vision {
            return Some(vision);
        }
        let modalities = self.architecture.as_ref()?.input_modalities.as_ref()?;
        Some(
            modalities
                .iter()
                .any(|modality| modality.eq_ignore_ascii_case("image")),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io::Cursor;

    use super::*;

    /// One exchange a fake transport saw.
    #[derive(Debug, Clone, PartialEq)]
    struct Recorded {
        method: Method,
        url: String,
        api_key: String,
        body: Vec<u8>,
        stream: bool,
    }

    impl Recorded {
        /// The request body as JSON.
        fn json(&self) -> Value {
            serde_json::from_slice(&self.body).expect("the request body is JSON")
        }
    }

    /// A transport that answers from a script and keeps what it was asked.
    struct FakeTransport {
        replies: RefCell<VecDeque<HttpReply>>,
        calls: RefCell<Vec<Recorded>>,
    }

    impl FakeTransport {
        fn new(replies: impl IntoIterator<Item = HttpReply>) -> Self {
            Self {
                replies: RefCell::new(replies.into_iter().collect()),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<Recorded> {
            self.calls.borrow().clone()
        }
    }

    impl HttpTransport for FakeTransport {
        fn send(&self, call: HttpCall<'_>) -> Result<HttpReply, String> {
            self.calls.borrow_mut().push(Recorded {
                method: call.method,
                url: call.url.to_string(),
                api_key: call.api_key.to_string(),
                body: call.body.unwrap_or_default().to_vec(),
                stream: call.stream,
            });
            self.replies
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| "the script has no reply left".to_string())
        }
    }

    fn reply(status: u16, body: &str) -> HttpReply {
        HttpReply {
            status,
            retry_after: None,
            body: Box::new(Cursor::new(body.as_bytes().to_vec())),
        }
    }

    fn reply_after(status: u16, body: &str, retry_after: u64) -> HttpReply {
        HttpReply {
            status,
            retry_after: Some(retry_after),
            body: Box::new(Cursor::new(body.as_bytes().to_vec())),
        }
    }

    /// A configured endpoint, with a key, for the tests that only care about
    /// the wire.
    fn config() -> AiConfig {
        AiConfig::default()
            .with_base_url("https://example.test/v1/")
            .with_model("test-model")
            .with_api_key("sk-secret")
    }

    fn user(text: &str) -> ChatMessage {
        ChatMessage::text(ChatRole::User, text)
    }

    /// One canned answer with prose, a tool call and usage.
    const ANSWER: &str = r#"{
        "id": "chatcmpl-1",
        "model": "test-model",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Bonjour",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "post", "arguments": "{\"amount\": \"1.00\"}"}
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {"prompt_tokens": 12, "completion_tokens": 7, "total_tokens": 19}
    }"#;

    // -- the request the wire sees --------------------------------------------

    #[test]
    fn request_carries_model_messages_and_sampling() {
        let transport = FakeTransport::new([reply(200, ANSWER)]);
        let config = config().with_max_tokens(Some(256)).with_temperature(Some(0.25));
        let request = ChatRequest {
            messages: vec![
                ChatMessage::text(ChatRole::System, "You keep books."),
                user("What did I spend?"),
            ],
            tools: vec![ToolSchema {
                name: "post".to_string(),
                description: "Post a transaction".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"amount": {"type": "string"}},
                }),
            }],
            media: Vec::new(),
        };
        let answer = complete_with(&transport, &config, request).expect("the turn completes");
        assert_eq!(answer.text, "Bonjour");
        assert_eq!(
            answer.tool_calls,
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "post".to_string(),
                arguments: "{\"amount\": \"1.00\"}".to_string(),
            }]
        );
        assert_eq!(
            answer.usage,
            Some(Usage {
                prompt_tokens: 12,
                completion_tokens: 7,
                total_tokens: 19,
            })
        );
        assert_eq!(answer.finish_reason.as_deref(), Some("tool_calls"));
        assert!(!answer.cancelled);

        let calls = transport.calls();
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        // The trailing slash was dropped, so the path is the endpoint's.
        assert_eq!(call.url, "https://example.test/v1/chat/completions");
        assert_eq!(call.method, Method::Post);
        assert_eq!(call.api_key, "sk-secret");
        assert!(!call.stream);
        let body = call.json();
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["stream"], false);
        assert_eq!(body["max_tokens"], 256);
        assert_eq!(body["temperature"], 0.25);
        assert!(body.get("stream_options").is_none());
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "You keep books.");
        assert_eq!(body["messages"][1]["content"], "What did I spend?");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "post");
        assert_eq!(body["tools"][0]["function"]["parameters"]["type"], "object");
        // The key travels in the header, never in the body.
        assert!(!String::from_utf8_lossy(&call.body).contains("sk-secret"));
    }

    #[test]
    fn request_without_optional_settings_omits_them() {
        let transport = FakeTransport::new([reply(200, ANSWER)]);
        let request = ChatRequest {
            messages: vec![user("hello")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        complete_with(&transport, &config(), request).expect("the turn completes");
        let body = transport.calls()[0].json();
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("temperature").is_none());
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn images_attach_to_the_newest_user_message() {
        let transport = FakeTransport::new([reply(200, ANSWER)]);
        let request = ChatRequest {
            messages: vec![
                ChatMessage::text(ChatRole::System, "You keep books."),
                user("What is this receipt?"),
            ],
            tools: Vec::new(),
            media: vec![
                MediaPart::inline("image/png", b"\x89PNG").with_detail(ImageDetail::High),
            ],
        };
        complete_with(&transport, &config(), request).expect("the turn completes");
        let body = transport.calls()[0].json();
        let content = &body["messages"][1]["content"];
        assert!(content.is_array(), "the user turn became parts: {content}");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "What is this receipt?");
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(content[1]["image_url"]["detail"], "high");
        assert_eq!(
            content[1]["image_url"]["url"],
            format!("data:image/png;base64,{}", BASE64.encode(b"\x89PNG"))
        );
        // The system turn keeps its plain string content.
        assert_eq!(body["messages"][0]["content"], "You keep books.");
    }

    #[test]
    fn image_may_arrive_as_a_data_url_and_alone() {
        let transport = FakeTransport::new([reply(200, ANSWER)]);
        let request = ChatRequest {
            messages: vec![ChatMessage::text(ChatRole::User, "")],
            tools: Vec::new(),
            media: vec![MediaPart {
                mime: "image/jpeg".to_string(),
                data_base64: "data:image/jpeg;base64,AAAA".to_string(),
                detail: None,
            }],
        };
        complete_with(&transport, &config(), request).expect("the turn completes");
        let body = transport.calls()[0].json();
        let content = &body["messages"][0]["content"];
        // An image-only turn is just the image: no empty text part is invented.
        assert_eq!(content.as_array().map(Vec::len), Some(1), "{content}");
        assert_eq!(content[0]["type"], "image_url");
        assert_eq!(content[0]["image_url"]["url"], "data:image/jpeg;base64,AAAA");
        assert!(content[0]["image_url"].get("detail").is_none());
    }

    #[test]
    fn images_on_a_turn_that_is_not_the_users_are_refused() {
        let transport = FakeTransport::new([]);
        let request = ChatRequest {
            messages: vec![
                user("and now?"),
                ChatMessage::text(ChatRole::Assistant, "Sure."),
            ],
            tools: Vec::new(),
            media: vec![MediaPart::inline("image/png", b"x")],
        };
        let error = complete_with(&transport, &config(), request).unwrap_err();
        assert!(matches!(error, Failure::Request(_)), "{error:?}");
        assert!(transport.calls().is_empty(), "nothing was sent");
    }

    #[test]
    fn a_tool_result_without_a_call_id_is_refused() {
        let request = ChatRequest {
            messages: vec![ChatMessage {
                role: ChatRole::Tool,
                text: "done".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
            }],
            tools: Vec::new(),
            media: Vec::new(),
        };
        let error = complete_with(&FakeTransport::new([]), &config(), request).unwrap_err();
        assert!(matches!(error, Failure::Request(_)), "{error:?}");
    }

    #[test]
    fn a_missing_model_is_a_configuration_error() {
        let config = config().with_model("  ");
        let request = ChatRequest {
            messages: vec![user("hi")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        let error = complete_with(&FakeTransport::new([]), &config, request).unwrap_err();
        assert!(matches!(error, Failure::Config(_)), "{error:?}");
    }

    #[test]
    fn an_empty_conversation_is_a_request_error() {
        let request = ChatRequest::default();
        let error = complete_with(&FakeTransport::new([]), &config(), request).unwrap_err();
        assert!(matches!(error, Failure::Request(_)), "{error:?}");
    }

    #[test]
    fn a_non_http_base_url_is_refused() {
        let config = config().with_base_url("ftp://example.test/v1");
        let request = ChatRequest {
            messages: vec![user("hi")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        let error = complete_with(&FakeTransport::new([]), &config, request).unwrap_err();
        assert!(matches!(error, Failure::Config(_)), "{error:?}");
    }

    #[test]
    fn a_pasted_endpoint_url_is_reduced_to_its_root() {
        let config = config().with_base_url("https://example.test/v1/chat/completions");
        assert_eq!(
            config.chat_url().expect("a url"),
            "https://example.test/v1/chat/completions"
        );
        assert_eq!(config.models_url().expect("a url"), "https://example.test/v1/models");
    }

    #[test]
    fn a_local_endpoint_without_a_key_sends_no_authorization() {
        let transport = FakeTransport::new([reply(200, ANSWER)]);
        let config = config().with_api_key("");
        let request = ChatRequest {
            messages: vec![user("hi")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        complete_with(&transport, &config, request).expect("the turn completes");
        assert_eq!(transport.calls()[0].api_key, "");
        assert!(!config.has_key());
    }

    // -- tool calls round-trip ------------------------------------------------

    #[test]
    fn a_tool_call_round_trips_verbatim() {
        // The model asked for a tool; the caller answers and asks again.
        let transport = FakeTransport::new([reply(200, ANSWER), reply(200, ANSWER)]);
        let config = config();
        let first = ChatRequest {
            messages: vec![user("Post a coffee.")],
            tools: vec![ToolSchema {
                name: "post".to_string(),
                description: "Post a transaction".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            }],
            media: Vec::new(),
        };
        let answer = complete_with(&transport, &config, first).expect("the turn completes");
        let call = answer.tool_calls[0].clone();

        let second = ChatRequest {
            messages: vec![
                user("Post a coffee."),
                ChatMessage {
                    role: ChatRole::Assistant,
                    text: answer.text.clone(),
                    tool_calls: answer.tool_calls.clone(),
                    tool_call_id: None,
                },
                ChatMessage::tool_result(call.id.clone(), "posted"),
            ],
            tools: Vec::new(),
            media: Vec::new(),
        };
        complete_with(&transport, &config, second).expect("the turn completes");

        let body = transport.calls()[1].json();
        let assistant = &body["messages"][1];
        assert_eq!(assistant["role"], "assistant");
        assert_eq!(assistant["tool_calls"][0]["id"], "call_1");
        assert_eq!(assistant["tool_calls"][0]["type"], "function");
        assert_eq!(assistant["tool_calls"][0]["function"]["name"], "post");
        // The arguments are the same JSON text the model produced, not a
        // re-rendering of a parsed value.
        assert_eq!(
            assistant["tool_calls"][0]["function"]["arguments"],
            "{\"amount\": \"1.00\"}"
        );
        let result = &body["messages"][2];
        assert_eq!(result["role"], "tool");
        assert_eq!(result["tool_call_id"], "call_1");
        assert_eq!(result["content"], "posted");
        assert!(result.get("tool_calls").is_none());
    }

    #[test]
    fn tool_arguments_sent_as_an_object_are_reduced_to_text() {
        let answer = r#"{
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_9",
                        "type": "function",
                        "function": {"name": "post", "arguments": {"amount": "2.00"}}
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }"#;
        let transport = FakeTransport::new([reply(200, answer)]);
        let request = ChatRequest {
            messages: vec![user("hi")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        let response = complete_with(&transport, &config(), request).expect("the turn completes");
        let parsed: Value = serde_json::from_str(&response.tool_calls[0].arguments)
            .expect("the arguments are JSON text");
        assert_eq!(parsed["amount"], "2.00");
        assert_eq!(response.text, "");
    }

    #[test]
    fn a_tool_call_without_an_id_still_answers() {
        let answer = r#"{
            "choices": [{
                "message": {"tool_calls": [{"function": {"name": "post"}}]},
                "finish_reason": "tool_calls"
            }]
        }"#;
        let transport = FakeTransport::new([reply(200, answer)]);
        let request = ChatRequest {
            messages: vec![user("hi")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        let response = complete_with(&transport, &config(), request).expect("the turn completes");
        assert_eq!(
            response.tool_calls,
            vec![ToolCall {
                id: String::new(),
                name: "post".to_string(),
                arguments: String::new(),
            }]
        );
    }

    // -- error classification -------------------------------------------------

    #[test]
    fn a_401_is_an_authentication_failure() {
        let body = r#"{"error":{"message":"Incorrect API key provided","code":"invalid_api_key","type":"invalid_request_error"}}"#;
        let transport = FakeTransport::new([reply(401, body)]);
        let request = ChatRequest {
            messages: vec![user("hi")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        let error = complete_with(&transport, &config(), request).unwrap_err();
        match &error {
            Failure::Unauthorized { status, detail } => {
                assert_eq!(*status, 401);
                assert!(detail.contains("Incorrect API key provided"), "{detail}");
            }
            other => panic!("expected Unauthorized, got {other:?}"),
        }
        let message = error.message();
        assert!(message.contains("API key"), "{message}");
        // The key itself never appears in a message.
        assert!(!message.contains("sk-secret"), "{message}");
    }

    #[test]
    fn a_403_is_also_an_authentication_failure() {
        let error = classify(403, None, "");
        assert!(matches!(error, Failure::Unauthorized { status: 403, .. }));
    }

    #[test]
    fn a_429_keeps_the_endpoints_retry_advice() {
        let transport = FakeTransport::new([reply_after(
            429,
            r#"{"error":{"message":"Rate limit reached"}}"#,
            30,
        )]);
        let request = ChatRequest {
            messages: vec![user("hi")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        let error = complete_with(&transport, &config(), request).unwrap_err();
        match &error {
            Failure::RateLimited {
                retry_after,
                detail,
            } => {
                assert_eq!(*retry_after, Some(30));
                assert!(detail.contains("Rate limit reached"), "{detail}");
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
        let message = error.message();
        assert!(message.contains("30s"), "{message}");
    }

    #[test]
    fn a_5xx_is_a_server_failure() {
        let error = classify(503, None, "<html>Service Unavailable</html>");
        match &error {
            Failure::Server { status, detail } => {
                assert_eq!(*status, 503);
                assert!(detail.contains("Service Unavailable"), "{detail}");
            }
            other => panic!("expected Server, got {other:?}"),
        }
        assert!(error.message().contains("server error (503)"));
    }

    #[test]
    fn a_400_keeps_the_providers_explanation() {
        let error = classify(
            400,
            None,
            r#"{"error":{"message":"This model's maximum context length is 8192 tokens","type":"invalid_request_error"}}"#,
        );
        match &error {
            Failure::Rejected { status, detail } => {
                assert_eq!(*status, 400);
                assert!(detail.contains("maximum context length"), "{detail}");
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn a_long_error_detail_is_cut_on_a_character_boundary() {
        let body = format!(r#"{{"error":"{}"}}"#, "é".repeat(400));
        let detail = error_detail(&body);
        assert!(detail.chars().count() <= DETAIL_CHARS + 1, "{detail}");
        assert!(detail.ends_with('…'));
    }

    #[test]
    fn malformed_json_in_a_success_is_its_own_failure() {
        let transport = FakeTransport::new([reply(200, "<html>proxy says hi</html>")]);
        let request = ChatRequest {
            messages: vec![user("hi")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        let error = complete_with(&transport, &config(), request).unwrap_err();
        assert!(matches!(error, Failure::Malformed(_)), "{error:?}");
        assert!(error.message().contains("could not be read"), "{error}");
        assert!(error.message().contains("proxy says hi"), "{error}");
    }

    #[test]
    fn an_empty_success_body_is_malformed() {
        let transport = FakeTransport::new([reply(200, "")]);
        let request = ChatRequest {
            messages: vec![user("hi")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        let error = complete_with(&transport, &config(), request).unwrap_err();
        assert!(matches!(error, Failure::Malformed(_)), "{error:?}");
        assert!(error.message().contains("empty"), "{error}");
    }

    #[test]
    fn an_error_object_on_a_200_is_a_provider_failure() {
        let body = r#"{"error":{"message":"The model has been decommissioned","code":404}}"#;
        let transport = FakeTransport::new([reply(200, body)]);
        let request = ChatRequest {
            messages: vec![user("hi")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        let error = complete_with(&transport, &config(), request).unwrap_err();
        match &error {
            Failure::Provider(detail) => {
                assert!(detail.contains("decommissioned"), "{detail}");
                assert!(detail.contains("code: 404"), "{detail}");
            }
            other => panic!("expected Provider, got {other:?}"),
        }
    }

    #[test]
    fn a_success_without_choices_is_malformed() {
        let transport = FakeTransport::new([reply(200, r#"{"choices":[]}"#)]);
        let request = ChatRequest {
            messages: vec![user("hi")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        let error = complete_with(&transport, &config(), request).unwrap_err();
        assert!(matches!(error, Failure::Malformed(_)), "{error:?}");
        assert!(error.message().contains("no choices"), "{error}");
    }

    #[test]
    fn a_transport_error_is_reported_as_unreachable() {
        struct Unreachable;
        impl HttpTransport for Unreachable {
            fn send(&self, _: HttpCall<'_>) -> Result<HttpReply, String> {
                Err("connection refused".to_string())
            }
        }
        let request = ChatRequest {
            messages: vec![user("hi")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        let error = complete_with(&Unreachable, &config(), request).unwrap_err();
        assert!(matches!(error, Failure::Transport(_)), "{error:?}");
        assert!(
            error.message().starts_with("could not reach the endpoint"),
            "{error}"
        );
    }

    // -- streaming ------------------------------------------------------------

    /// An SSE script with prose, a streamed tool call and usage.
    fn sse() -> String {
        [
            r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":""}}]}"#,
            "",
            r#"data: {"choices":[{"index":0,"delta":{"content":"Bon"}}]}"#,
            "",
            r#"data: {"choices":[{"index":0,"delta":{"content":"jour"}}]}"#,
            "",
            r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"post","arguments":""}}]}}]}"#,
            "",
            r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"amount\""}}]}}]}"#,
            "",
            r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":": \"1.00\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            "",
            r#"data: {"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":7,"total_tokens":19}}"#,
            "",
            "data: [DONE]",
            "",
        ]
        .join("\n")
    }

    #[test]
    fn a_stream_becomes_deltas_and_one_assembled_answer() {
        let transport = FakeTransport::new([reply(200, &sse())]);
        let mut events = Vec::new();
        let request = ChatRequest {
            messages: vec![user("Post a coffee.")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        let response = stream_with(&transport, &config(), request, |event| {
            events.push(event);
            ControlFlow::Continue(())
        })
        .expect("the stream completes");

        assert_eq!(
            events,
            vec![
                StreamEvent::TextDelta {
                    text: "Bon".to_string()
                },
                StreamEvent::TextDelta {
                    text: "jour".to_string()
                },
                StreamEvent::ToolCallDelta {
                    index: 0,
                    id: Some("call_1".to_string()),
                    name: Some("post".to_string()),
                    arguments: String::new(),
                },
                StreamEvent::ToolCallDelta {
                    index: 0,
                    id: None,
                    name: None,
                    arguments: "{\"amount\"".to_string(),
                },
                StreamEvent::ToolCallDelta {
                    index: 0,
                    id: None,
                    name: None,
                    arguments: ": \"1.00\"}".to_string(),
                },
                StreamEvent::Usage {
                    prompt_tokens: 12,
                    completion_tokens: 7,
                    total_tokens: 19,
                },
                StreamEvent::Done {
                    finish_reason: Some("tool_calls".to_string()),
                },
            ]
        );
        assert_eq!(response.text, "Bonjour");
        assert_eq!(
            response.tool_calls,
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "post".to_string(),
                arguments: "{\"amount\": \"1.00\"}".to_string(),
            }]
        );
        assert_eq!(
            response.usage,
            Some(Usage {
                prompt_tokens: 12,
                completion_tokens: 7,
                total_tokens: 19,
            })
        );
        assert!(!response.cancelled);

        // The stream request asked for usage in its chunks.
        let call = &transport.calls()[0];
        assert!(call.stream);
        let body = call.json();
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn a_stream_split_mid_line_still_assembles() {
        // A byte-at-a-time reader: line boundaries land inside chunks, and a
        // multibyte character is split across reads.
        struct Trickle(Vec<u8>, usize);
        impl Read for Trickle {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                let end = (self.1 + 1).min(self.0.len());
                let take = end - self.1;
                buffer[..take].copy_from_slice(&self.0[self.1..end]);
                self.1 = end;
                Ok(take)
            }
        }
        let script = sse().replace("jour", "joür");
        let transport = FakeTransport::new([HttpReply {
            status: 200,
            retry_after: None,
            body: Box::new(Trickle(script.into_bytes(), 0)),
        }]);
        let mut text = String::new();
        let request = ChatRequest {
            messages: vec![user("hi")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        let response = stream_with(&transport, &config(), request, |event| {
            if let StreamEvent::TextDelta { text: delta } = event {
                text.push_str(&delta);
            }
            ControlFlow::Continue(())
        })
        .expect("the stream completes");
        assert_eq!(text, "Bonjoür");
        assert_eq!(response.text, "Bonjoür");
    }

    #[test]
    fn carriage_returns_and_comments_do_not_break_the_stream() {
        let script = ": keep-alive\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\r\n\r\ndata: [DONE]\r\n";
        let transport = FakeTransport::new([reply(200, script)]);
        let mut events = Vec::new();
        let request = ChatRequest {
            messages: vec![user("hi")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        stream_with(&transport, &config(), request, |event| {
            events.push(event);
            ControlFlow::Continue(())
        })
        .expect("the stream completes");
        assert_eq!(
            events,
            vec![
                StreamEvent::TextDelta {
                    text: "hi".to_string()
                },
                StreamEvent::Done {
                    finish_reason: None
                },
            ]
        );
    }

    #[test]
    fn breaking_the_callback_cancels_the_stream() {
        let transport = FakeTransport::new([reply(200, &sse())]);
        let mut events = Vec::new();
        let request = ChatRequest {
            messages: vec![user("hi")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        let response = stream_with(&transport, &config(), request, |event| {
            events.push(event);
            ControlFlow::Break(())
        })
        .expect("the stream answers what it read");
        assert_eq!(
            events,
            vec![StreamEvent::TextDelta {
                text: "Bon".to_string()
            }]
        );
        assert!(response.cancelled);
        assert_eq!(response.text, "Bon");
        // The cancel stopped the read: no tool calls followed.
        assert!(response.tool_calls.is_empty());
    }

    #[test]
    fn an_error_frame_mid_stream_is_a_provider_failure() {
        let script = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: {\"error\":{\"message\":\"overloaded\"}}\n\n";
        let transport = FakeTransport::new([reply(200, script)]);
        let request = ChatRequest {
            messages: vec![user("hi")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        let error = stream_with(&transport, &config(), request, |_| ControlFlow::Continue(()))
            .unwrap_err();
        assert!(matches!(error, Failure::Provider(_)), "{error:?}");
        assert!(error.message().contains("overloaded"), "{error}");
    }

    #[test]
    fn done_stops_reading_even_if_the_connection_stays_open() {
        // A reader that hands over the whole script once and then fails every
        // later read, the way a keep-alive socket blocks after `[DONE]`.
        struct HangUp {
            payload: Option<Vec<u8>>,
        }
        impl Read for HangUp {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                match self.payload.take() {
                    Some(bytes) => {
                        let take = bytes.len().min(buffer.len());
                        buffer[..take].copy_from_slice(&bytes[..take]);
                        if take < bytes.len() {
                            self.payload = Some(bytes[take..].to_vec());
                        }
                        Ok(take)
                    }
                    None => Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "the socket is still open",
                    )),
                }
            }
        }
        let script = b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n".to_vec();
        let transport = FakeTransport::new([HttpReply {
            status: 200,
            retry_after: None,
            body: Box::new(HangUp {
                payload: Some(script),
            }),
        }]);
        let request = ChatRequest {
            messages: vec![user("hi")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        let response =
            stream_with(&transport, &config(), request, |_| ControlFlow::Continue(()))
                .expect("the stream ends at [DONE]");
        assert_eq!(response.text, "hi");
        assert!(!response.cancelled);
    }

    #[test]
    fn a_stream_that_ends_without_done_still_answers() {
        let script = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n";
        let transport = FakeTransport::new([reply(200, script)]);
        let request = ChatRequest {
            messages: vec![user("hi")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        let response =
            stream_with(&transport, &config(), request, |_| ControlFlow::Continue(()))
                .expect("the stream completes");
        assert_eq!(response.text, "hi");
    }

    #[test]
    fn malformed_stream_json_is_malformed() {
        let script = "data: {not json}\n\n";
        let transport = FakeTransport::new([reply(200, script)]);
        let request = ChatRequest {
            messages: vec![user("hi")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        let error = stream_with(&transport, &config(), request, |_| ControlFlow::Continue(()))
            .unwrap_err();
        assert!(matches!(error, Failure::Malformed(_)), "{error:?}");
    }

    #[test]
    fn a_stream_error_status_is_classified_before_reading() {
        let transport = FakeTransport::new([reply_after(
            429,
            r#"{"error":{"message":"slow down"}}"#,
            5,
        )]);
        let request = ChatRequest {
            messages: vec![user("hi")],
            tools: Vec::new(),
            media: Vec::new(),
        };
        let error = stream_with(&transport, &config(), request, |_| ControlFlow::Continue(()))
            .unwrap_err();
        match error {
            Failure::RateLimited { retry_after, .. } => assert_eq!(retry_after, Some(5)),
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    // -- models ---------------------------------------------------------------

    #[test]
    fn models_are_read_with_what_the_endpoint_publishes() {
        let body = r#"{
            "object": "list",
            "data": [
                {"id": "gpt-4o", "architecture": {"input_modalities": ["text", "image"]}},
                {"id": "gpt-3.5", "context_length": 16385},
                {"id": "local", "max_model_len": 32768, "supports_vision": false},
                {"id": "gpt-4o"},
                {"id": "  "}
            ]
        }"#;
        let transport = FakeTransport::new([reply(200, body)]);
        let models = models_with(&transport, &config()).expect("the list is read");
        assert_eq!(
            models,
            vec![
                ModelInfo {
                    id: "gpt-3.5".to_string(),
                    context_window: Some(16385),
                    vision: None,
                },
                ModelInfo {
                    id: "gpt-4o".to_string(),
                    context_window: None,
                    vision: Some(true),
                },
                ModelInfo {
                    id: "local".to_string(),
                    context_window: Some(32768),
                    vision: Some(false),
                },
            ]
        );
        let call = &transport.calls()[0];
        assert_eq!(call.method, Method::Get);
        assert_eq!(call.url, "https://example.test/v1/models");
        assert_eq!(call.api_key, "sk-secret");
    }

    #[test]
    fn a_bare_model_array_is_read_too() {
        let transport = FakeTransport::new([reply(
            200,
            r#"[{"id":"a","context_window":8192},{"name":"b"}]"#,
        )]);
        let models = models_with(&transport, &config()).expect("the list is read");
        assert_eq!(
            models,
            vec![
                ModelInfo {
                    id: "a".to_string(),
                    context_window: Some(8192),
                    vision: None,
                },
                ModelInfo {
                    id: "b".to_string(),
                    context_window: None,
                    vision: None,
                },
            ]
        );
    }

    #[test]
    fn a_model_list_that_is_not_a_list_is_malformed() {
        let transport = FakeTransport::new([reply(200, r#"{"oops":true}"#)]);
        let error = models_with(&transport, &config()).unwrap_err();
        assert!(matches!(error, Failure::Malformed(_)), "{error:?}");
    }

    // -- configuration --------------------------------------------------------

    #[test]
    fn the_configuration_round_trips_through_its_directory() {
        let dir = crate::testing::TempDir::new("ai-config");
        let saved = AiConfig::default()
            .with_base_url("https://example.test/v1")
            .with_model("test-model")
            .with_api_key("sk-secret")
            .with_max_tokens(Some(512))
            .with_temperature(Some(0.4));
        save(dir.dir(), &saved).expect("the config is saved");
        let loaded = load(dir.dir()).expect("the config is loaded");
        assert_eq!(loaded, saved);
        assert!(loaded.has_key());
    }

    #[test]
    fn a_missing_configuration_is_the_default() {
        let dir = crate::testing::TempDir::new("ai-config-missing");
        let loaded = load(dir.dir()).expect("a default is answered");
        assert_eq!(loaded, AiConfig::default());
        assert!(!loaded.has_key());
        // Loading never creates the file.
        assert!(!config_path(dir.dir()).exists());
    }

    #[test]
    fn another_version_is_refused_by_name() {
        let dir = crate::testing::TempDir::new("ai-config-version");
        fs::write(
            config_path(dir.dir()),
            r#"{"version": 99, "base_url": "https://example.test/v1", "model": "m"}"#,
        )
        .expect("the file is written");
        let error = load(dir.dir()).unwrap_err();
        assert!(error.contains("version 99"), "{error}");
    }

    #[test]
    fn a_broken_file_is_not_silently_replaced() {
        let dir = crate::testing::TempDir::new("ai-config-broken");
        fs::write(config_path(dir.dir()), "{ not json").expect("the file is written");
        assert!(load(dir.dir()).is_err());
    }

    #[test]
    fn the_serialized_configuration_never_holds_the_key() {
        let config = config();
        let json = serde_json::to_value(&config).expect("the config serializes");
        assert_eq!(json["base_url"], "https://example.test/v1/");
        assert_eq!(json["model"], "test-model");
        assert_eq!(json["has_key"], true);
        assert!(json.get("api_key").is_none(), "{json}");
        assert!(!json.to_string().contains("sk-secret"), "{json}");

        let without = serde_json::to_value(AiConfig::default()).expect("it serializes");
        assert_eq!(without["has_key"], false);
    }

    #[test]
    fn debug_redacts_the_key() {
        let printed = format!("{:?}", config());
        assert!(!printed.contains("sk-secret"), "{printed}");
        assert!(printed.contains("<set>"), "{printed}");
        let unset = format!("{:?}", AiConfig::default());
        assert!(unset.contains("<unset>"), "{unset}");
    }

    #[test]
    fn the_configuration_file_is_private_on_unix() {
        let dir = crate::testing::TempDir::new("ai-config-mode");
        save(dir.dir(), &config()).expect("the config is saved");
        #[cfg(unix)]
        {
            assert!(
                file_mode_is_private(&config_path(dir.dir())),
                "the config file is not 0600"
            );
        }
        // The key is on disk — that is the point of the file — but only there.
        let text = fs::read_to_string(config_path(dir.dir())).expect("the file is read");
        assert!(text.contains("sk-secret"), "the key is stored for the request");
    }

    #[test]
    fn a_rewrite_replaces_the_previous_configuration() {
        let dir = crate::testing::TempDir::new("ai-config-rewrite");
        save(dir.dir(), &config()).expect("the first save");
        let replacement = AiConfig::default()
            .with_base_url("http://localhost:11434/v1")
            .with_model("llama3");
        save(dir.dir(), &replacement).expect("the second save");
        let loaded = load(dir.dir()).expect("the config is loaded");
        assert_eq!(loaded, replacement);
        assert!(!loaded.has_key());
        // No temporary file is left behind.
        let leftovers: Vec<_> = fs::read_dir(dir.dir())
            .expect("the directory is read")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    // -- helpers --------------------------------------------------------------

    #[test]
    fn a_message_constructor_sets_only_what_it_names() {
        let message = ChatMessage::text(ChatRole::System, "be brief");
        assert_eq!(message.role, ChatRole::System);
        assert_eq!(message.text, "be brief");
        assert!(message.tool_calls.is_empty());
        assert!(message.tool_call_id.is_none());

        let result = ChatMessage::tool_result("call_1", "ok");
        assert_eq!(result.role, ChatRole::Tool);
        assert_eq!(result.tool_call_id.as_deref(), Some("call_1"));
    }

    #[test]
    fn usage_without_a_total_is_summed() {
        let usage: WireUsage =
            serde_json::from_str(r#"{"prompt_tokens":3,"completion_tokens":4}"#).unwrap();
        assert_eq!(
            usage.usage(),
            Usage {
                prompt_tokens: 3,
                completion_tokens: 4,
                total_tokens: 7,
            }
        );
    }

    #[test]
    fn stream_events_serialize_with_a_kind() {
        let text = serde_json::to_value(StreamEvent::TextDelta {
            text: "hi".to_string(),
        })
        .expect("the event serializes");
        assert_eq!(text, serde_json::json!({"kind": "text_delta", "text": "hi"}));
        let done = serde_json::to_value(StreamEvent::Done {
            finish_reason: Some("stop".to_string()),
        })
        .expect("the event serializes");
        assert_eq!(done["kind"], "done");
        assert_eq!(done["finish_reason"], "stop");
    }
}
