// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The assistant panel's surface: the settings the user edits, the endpoint's
//! model list, one turn, the instructions the model is given, and the read
//! tools it may ask for.
//!
//! # Why this is its own module
//!
//! [`crate::ai`] owns the wire: it turns a configuration and a conversation
//! into one HTTP exchange and classifies what comes back. This module owns what
//! sits above that and is specific to *this* app — which directory the
//! configuration is read from, the settings view that never carries the key,
//! the prompt that tells the model what kind of books it is looking at, and the
//! table of tools the panel offers. Keeping the two apart leaves `ai.rs`
//! testable without a configuration directory and leaves this file's tests
//! free of the network.
//!
//! # The key never comes back
//!
//! [`AiSettingsView`] is what the panel receives, and it has no field for the
//! key — only `has_key`. [`configure`] takes the key as an argument, the one
//! direction it travels from the panel, and hands it to `ai.rs`, which stores
//! it privately. Nothing here can read a stored key back out; the only way to
//! learn whether one is set is `has_key`.
//!
//! # Blocking
//!
//! [`models`] and [`chat`] each make one HTTP call and wait for it, because
//! `ai.rs` is blocking (`ureq`) — a model answer can take minutes. A Tauri
//! command that ran them on the event loop would freeze the whole app, so the
//! command layer spawns each one on a worker thread; the functions here stay
//! plain so their tests need no runtime.
//!
//! # The tools, defined once
//!
//! [`tools`] is the projection of one `const` table ([`TOOLS`]). The panel
//! sends those schemas to the model and executes the calls the model makes, so
//! a second hand-written copy of a tool's name or arguments — a Python stub, a
//! paragraph in a prompt — is exactly the drift the table exists to prevent;
//! anything else that needs the tools should walk `tools()` too.
//!
//! Most tools are reads ([`is_read_only`]); the two writes, `add_transaction`
//! and `write_file`, only *stage a proposal*: the panel renders the exact entry
//! or text and lets the user approve or reject it, and the CLI prints it. A
//! call to one never touches a file, which is why the model is told the change
//! is pending rather than applied.

use std::path::Path;

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::ai::{self, AiConfig};

/// The wire types this surface speaks are `ai`'s own; re-exported so the command
/// layer can name them without reaching past this module's contract.
pub use crate::ai::{ChatRequest, ChatResponse, ModelInfo, ToolSchema};

/// The assistant's settings as the panel shows and edits them: everything but
/// the key, plus whether one is stored.
///
/// The key has no field here on purpose, so no command can return it by
/// accident; `ai.rs` serializes its own configuration into these same fields,
/// which is why the panel's view and the stored shape cannot disagree.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct AiSettingsView {
    /// The endpoint's API root, e.g. `https://api.openai.com/v1`.
    pub base_url: String,
    /// The model id sent to the endpoint; empty until the user picks one.
    pub model: String,
    /// The per-answer token ceiling, when the user set one.
    pub max_tokens: Option<u32>,
    /// The sampling temperature, when the user set one.
    pub temperature: Option<f32>,
    /// Whether a key is stored. The key itself never crosses IPC.
    pub has_key: bool,
    /// The instructions the panel sends with every turn: defined in this module,
    /// so the two halves of the app cannot disagree about them.
    pub system_prompt: String,
}

impl AiSettingsView {
    /// The view of a stored configuration.
    fn of(config: &AiConfig) -> Self {
        Self {
            base_url: config.base_url().to_string(),
            model: config.model().to_string(),
            max_tokens: config.max_tokens(),
            temperature: config.temperature(),
            has_key: config.has_key(),
            system_prompt: SYSTEM_PROMPT.to_string(),
        }
    }
}

/// The configuration in `dir`, as the settings screen shows it.
///
/// A directory with no configuration file answers the defaults, not an error:
/// the panel opens on a fresh configuration the user then fills in.
pub fn settings(dir: &Path) -> Result<AiSettingsView, String> {
    Ok(AiSettingsView::of(&AiConfig::load(dir)?))
}

/// Replace the configuration in `dir` with the one the user saved.
///
/// `api_key` is [`Option`] because the panel cannot show the stored key back,
/// so it cannot send it either: `None` means "unchanged" and keeps whatever is
/// stored, while `Some` — including an empty string, which clears the key —
/// replaces it. Everything else is replaced as given; a base URL or model this
/// function cannot use is reported by the first request, by `ai.rs`, rather
/// than refused here, because a half-typed settings form must still be
/// savable.
pub fn configure(
    dir: &Path,
    base_url: String,
    api_key: Option<String>,
    model: String,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
) -> Result<AiSettingsView, String> {
    let config = AiConfig::load(dir)?
        .with_base_url(base_url)
        .with_model(model)
        .with_max_tokens(max_tokens)
        .with_temperature(temperature);
    let config = match api_key {
        Some(api_key) => config.with_api_key(api_key),
        None => config,
    };
    config.save(dir)?;
    Ok(AiSettingsView::of(&config))
}

/// The models the endpoint in `dir` offers. Blocking; see the module docs.
pub fn models(dir: &Path) -> Result<Vec<ModelInfo>, String> {
    models_with(&Endpoint, dir)
}

/// One turn answered by the endpoint in `dir`. Blocking; the caller spawns a
/// thread and forwards the answer to the panel.
///
/// The request is forwarded exactly as the panel built it — messages, tools and
/// media included — because composing it is the panel's job: it holds the
/// conversation, [`SYSTEM_PROMPT`] and [`tools`].
pub fn chat(dir: &Path, request: ChatRequest) -> Result<ChatResponse, String> {
    chat_with(&Endpoint, dir, request)
}

// -- the seam the tests drive ------------------------------------------------

/// The two `ai.rs` calls behind one name.
///
/// `ai.rs`'s own tests script its HTTP transport; everything this module does
/// is above that wire — load the directory's configuration, hand it to one of
/// two calls, return what came back — so the seam a test scripts here is the
/// calls themselves. [`Endpoint`] is the real one; `api.rs`'s assistant loop
/// takes any [`Backend`], which is how a scripted model drives it.
pub(crate) trait Backend {
    fn models(&self, config: &AiConfig) -> Result<Vec<ModelInfo>, String>;
    fn chat(&self, config: &AiConfig, request: ChatRequest) -> Result<ChatResponse, String>;
}

/// The real backend: `ai.rs`, over the network.
pub(crate) struct Endpoint;

impl Backend for Endpoint {
    fn models(&self, config: &AiConfig) -> Result<Vec<ModelInfo>, String> {
        ai::models(config)
    }

    fn chat(&self, config: &AiConfig, request: ChatRequest) -> Result<ChatResponse, String> {
        ai::complete(config, request)
    }
}

fn models_with(backend: &impl Backend, dir: &Path) -> Result<Vec<ModelInfo>, String> {
    let config = AiConfig::load(dir)?;
    backend.models(&config)
}

pub(crate) fn chat_with(
    backend: &impl Backend,
    dir: &Path,
    request: ChatRequest,
) -> Result<ChatResponse, String> {
    let config = AiConfig::load(dir)?;
    backend.chat(&config, request)
}

// -- the instructions --------------------------------------------------------

/// What the model is told before the conversation starts.
///
/// It names the bookkeeping rules the panel cannot enforce from its side: the
/// figures are exact decimal strings (summing them as floats is the one mistake
/// that silently corrupts an answer), the conversation carries summaries rather
/// than the journal, an edit is ever only proposed, and facts come from the
/// tools.
pub const SYSTEM_PROMPT: &str = "\
You are the assistant built into a Beancount ledger app. The user keeps their
books as Beancount text files and the app has loaded them.

- Facts come from the tools. Ask for balances, transactions, budgets, prices and
  index series instead of recalling anything, and never invent an amount, an
  account, a date or a total. If the tools do not answer a question, say what
  you could not find.
- You see summaries and tool results, not the ledger. There is no dump of the
  journal in this conversation; do not claim to have read one.
- Money is an exact decimal string, e.g. \"1234.56\", together with its
  commodity. Never sum, average or otherwise compute with those values as
  floating point numbers, and never restate an amount from memory: ask a tool
  for the total and copy the string it returns, sign and commodity included.
- You may propose a change to the books with `add_transaction` (one new entry)
  or `write_file` (a journal file's whole new text). Those tools only stage a
  proposal: the user reads it and approves or rejects it, and nothing is
  written until they do. Never say a change has been applied, and never repeat
  a proposal unless the user asks for it again.
- Answer in the language the user writes in.
";

// -- the tools ---------------------------------------------------------------

/// One tool argument, as both the schema and a dispatcher need it.
#[derive(Debug, Clone, Copy)]
struct ToolArg {
    /// The name the model passes it under.
    name: &'static str,
    /// The JSON Schema type; see [`ArgKind`].
    kind: ArgKind,
    /// What the model is told the argument means.
    description: &'static str,
    /// Whether a call without it is refused by the panel.
    required: bool,
}

/// The argument types the tools take. An enum rather than a string, so a
/// misspelled type cannot reach a model.
#[derive(Debug, Clone, Copy)]
enum ArgKind {
    /// `"string"`: an account name, a `YYYY-MM` month, a BQL statement.
    Text,
    /// `"integer"`: a count of months.
    Whole,
    /// `"array"` of posting objects, each `{account, units, currency}`.
    Postings,
}

impl ArgKind {
    /// The JSON Schema `type` this argument carries.
    fn json_type(self) -> &'static str {
        match self {
            Self::Text => "string",
            Self::Whole => "integer",
            Self::Postings => "array",
        }
    }
}

/// One tool, before it becomes a [`ToolSchema`].
#[derive(Debug, Clone, Copy)]
struct ToolSpec {
    /// The dispatch name: what the model calls and the panel matches on.
    name: &'static str,
    /// What the tool answers, for the model to read.
    description: &'static str,
    /// The arguments, in the order the panel offers them.
    args: &'static [ToolArg],
    /// Whether the tool only reads. A write tool stages a proposal instead of
    /// touching a file, and the panel and CLI both ask this before running one.
    read_only: bool,
}

/// The tools the panel offers, in the order they are offered: the reads first,
/// then the two writes.
///
/// Each read is a whole call the panel already makes over IPC, so the model's
/// request is served by the same code path as the screens': the figures it
/// reads are the figures the user sees. The writes are deliberately last and
/// marked [`ToolSpec::read_only`] false: naming one stages a proposal, never a
/// write.
const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "query",
        description: "Run one BQL (Beancount Query Language) statement against the loaded ledger and return its columns and rows, e.g. `SELECT account, sum(position) GROUP BY account`. Amounts arrive as exact decimal strings; use this for any total instead of adding figures yourself. At most 200 rows come back.",
        args: &[ToolArg {
            name: "query",
            kind: ArgKind::Text,
            description: "One BQL statement, e.g. `SELECT payee, sum(position) WHERE account ~ \"^Expenses:Food\" GROUP BY payee`.",
            required: true,
        }],
        read_only: true,
    },
    ToolSpec {
        name: "accounts",
        description: "Every account with its exact balance as a decimal string, its commodity, and whether its subtree has uncommitted changes. Balances follow beancount `bal` semantics: a parent is already the sum of its whole subtree, so never add a parent to its children.",
        args: &[],
        read_only: true,
    },
    ToolSpec {
        name: "transactions",
        description: "The transactions of one account, newest first, with date, payee, narration, postings and amounts as decimal strings. An account with a long history answers with a lot of rows; read a narrower account (a leaf) rather than a root.",
        args: &[ToolArg {
            name: "account",
            kind: ArgKind::Text,
            description: "The account to read, e.g. `Expenses:Food`.",
            required: true,
        }],
        read_only: true,
    },
    ToolSpec {
        name: "grep",
        description: "Search the ledger's own directory for a regular expression, honouring .gitignore. Returns matching paths with line numbers and the line text. Use it to find a payee, an account spelling or a directive; it never edits.",
        args: &[
            ToolArg {
                name: "pattern",
                kind: ArgKind::Text,
                description: "A regular expression, e.g. `^2026-.*Migros`.",
                required: true,
            },
            ToolArg {
                name: "glob",
                kind: ArgKind::Text,
                description: "Restrict to paths matching this glob, e.g. `**/*.bean`.",
                required: false,
            },
        ],
        read_only: true,
    },
    ToolSpec {
        name: "read_lines",
        description: "Read a journal file as numbered lines, each with an eight-hex-character content hash. A file longer than one answer allows is read a page at a time with `offset`/`limit`: read the next page until `truncated` is false. To change the file, propose the new text with `write_file`; reading it again gives fresh hashes.",
        args: &[
            ToolArg {
                name: "path",
                kind: ArgKind::Text,
                description: "A path from the `files` tool, relative to the ledger's directory.",
                required: true,
            },
            ToolArg {
                name: "offset",
                kind: ArgKind::Whole,
                description: "First line to return, 1-based. Omit to start at the top.",
                required: false,
            },
            ToolArg {
                name: "limit",
                kind: ArgKind::Whole,
                description: "How many lines to return. Omit for the reader's default page.",
                required: false,
            },
        ],
        read_only: true,
    },
    ToolSpec {
        name: "files",
        description: "The journal files the loaded ledger includes, each with the path as loaded, the label to show, how many transactions it writes and how it stands in git. Use a path from here when pointing at where an entry lives.",
        args: &[],
        read_only: true,
    },
    ToolSpec {
        name: "budget_range",
        description: "The envelope budget over a window of consecutive months, with each month's budgeted, spent and remaining amounts as decimal strings. Both ends are optional and default to the last six months the ledger budgets.",
        args: &[
            ToolArg {
                name: "from",
                kind: ArgKind::Text,
                description: "The window's first month as `YYYY-MM`; defaults against the ledger's newest budget month.",
                required: false,
            },
            ToolArg {
                name: "to",
                kind: ArgKind::Text,
                description: "The window's last month as `YYYY-MM`; defaults to the ledger's newest budget month.",
                required: false,
            },
        ],
        read_only: true,
    },
    ToolSpec {
        name: "index_series",
        description: "The price index series imported into the app, each with its id, label, publisher, currency and monthly points as decimal strings. `lifestyle_report` needs one of these ids.",
        args: &[],
        read_only: true,
    },
    ToolSpec {
        name: "lifestyle_report",
        description: "Spending over the newest `months` months, restated in the first month's money with the index series `index`: nominal and real totals per month and per category, as decimal strings.",
        args: &[
            ToolArg {
                name: "months",
                kind: ArgKind::Whole,
                description: "How many of the newest months to cover, e.g. `12`.",
                required: true,
            },
            ToolArg {
                name: "index",
                kind: ArgKind::Text,
                description: "The id of an `index_series` entry, e.g. `us-cpi`.",
                required: true,
            },
        ],
        read_only: true,
    },
    ToolSpec {
        name: "price_targets",
        description: "Every commodity the loaded ledger holds or quotes, with the newest `price` directive for it when the ledger has one. These are the ledger's own prices, never a fetched quote; amounts are decimal strings.",
        args: &[],
        read_only: true,
    },
    ToolSpec {
        name: "add_transaction",
        description: "Propose one new transaction in a journal file. The app stages this as a proposal: the user sees the exact entry and approves or rejects it, so nothing is written by this call. The answer says the proposal is pending — never report the entry as added until the user approves it.",
        args: &[
            ToolArg {
                name: "file",
                kind: ArgKind::Text,
                description: "The journal file to append to: a `path` from the `files` tool, e.g. `main.bean`.",
                required: true,
            },
            ToolArg {
                name: "date",
                kind: ArgKind::Text,
                description: "The entry's date as `YYYY-MM-DD`.",
                required: true,
            },
            ToolArg {
                name: "flag",
                kind: ArgKind::Text,
                description: "`*` for a cleared entry or `!` for one to review.",
                required: true,
            },
            ToolArg {
                name: "payee",
                kind: ArgKind::Text,
                description: "Who was paid, e.g. `Migros`; omit it when the narration says enough.",
                required: false,
            },
            ToolArg {
                name: "narration",
                kind: ArgKind::Text,
                description: "What the entry is for, e.g. `Groceries`.",
                required: true,
            },
            ToolArg {
                name: "postings",
                kind: ArgKind::Postings,
                description: "At least two postings. Leave `units` and `currency` out of the leg beancount should interpolate.",
                required: true,
            },
        ],
        read_only: false,
    },
    ToolSpec {
        name: "write_file",
        description: "Propose a journal file's whole new text, for a change `add_transaction` cannot express. The app stages this as a proposal: the user sees a diff and approves or rejects it, so nothing is written by this call. The answer says the proposal is pending — never report the file as changed until the user approves it.",
        args: &[
            ToolArg {
                name: "path",
                kind: ArgKind::Text,
                description: "The journal file to replace: a `path` from the `files` tool.",
                required: true,
            },
            ToolArg {
                name: "text",
                kind: ArgKind::Text,
                description: "The file's complete new text, unchanged lines included.",
                required: true,
            },
        ],
        read_only: false,
    },
];

/// The tools the panel offers the model, as OpenAI-shaped schemas.
pub fn tools() -> Vec<ToolSchema> {
    TOOLS.iter().map(schema_of).collect()
}

/// Whether `name` is a tool that only reads.
///
/// An unknown name is not read-only: a dispatcher that has not learned the tool
/// must not run it, and this answer makes "unknown" fail closed.
pub fn is_read_only(name: &str) -> bool {
    TOOLS.iter().any(|spec| spec.name == name && spec.read_only)
}

/// One table row as the schema the wire takes.
fn schema_of(spec: &ToolSpec) -> ToolSchema {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for arg in spec.args {
        properties.insert(arg.name.to_string(), schema_arg(arg));
        if arg.required {
            required.push(Value::String(arg.name.to_string()));
        }
    }
    let mut parameters = Map::new();
    parameters.insert("type".to_string(), json!("object"));
    parameters.insert("properties".to_string(), Value::Object(properties));
    if !required.is_empty() {
        parameters.insert("required".to_string(), Value::Array(required));
    }
    // The panel dispatches strictly: an argument a tool does not take is a
    // mistake the model should learn about from the schema, not a silently
    // ignored key.
    parameters.insert("additionalProperties".to_string(), json!(false));
    ToolSchema {
        name: spec.name.to_string(),
        description: spec.description.to_string(),
        parameters: Value::Object(parameters),
    }
}

/// One argument's JSON Schema, shaped by [`ArgKind`].
fn schema_arg(arg: &ToolArg) -> Value {
    match arg.kind {
        ArgKind::Text | ArgKind::Whole => json!({
            "type": arg.kind.json_type(),
            "description": arg.description,
        }),
        // One posting per object, the same three fields `NewPosting` takes: a
        // nested schema so a model cannot send a bare account string or an
        // amount without its commodity and hope the panel guesses.
        ArgKind::Postings => json!({
            "type": "array",
            "description": arg.description,
            "minItems": 2,
            "items": {
                "type": "object",
                "properties": {
                    "account": {
                        "type": "string",
                        "description": "The account to post to, e.g. `Expenses:Food:Groceries`.",
                    },
                    "units": {
                        "type": "string",
                        "description": "The amount as an exact decimal string, e.g. `-12.34`; omit it on the leg beancount interpolates.",
                    },
                    "currency": {
                        "type": "string",
                        "description": "The commodity, e.g. `USD`; omit it to use the ledger's operating currency.",
                    },
                },
                "required": ["account"],
                "additionalProperties": false,
            },
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeSet;

    use super::*;
    use crate::ai::{ChatMessage, ChatRole, ToolCall, Usage};
    use crate::testing::TempDir;

    /// The configuration a fake backend was handed, as the endpoint sees it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Seen {
        base_url: String,
        model: String,
        has_key: bool,
    }

    /// A backend that answers from a script and keeps what it was asked —
    /// `ai.rs`'s transport fake, one layer up.
    struct FakeBackend {
        answer: Result<ChatResponse, String>,
        model_list: Result<Vec<ModelInfo>, String>,
        seen: RefCell<Vec<Seen>>,
        requests: RefCell<Vec<ChatRequest>>,
    }

    impl FakeBackend {
        fn answering(answer: ChatResponse) -> Self {
            Self {
                answer: Ok(answer),
                model_list: Ok(Vec::new()),
                seen: RefCell::new(Vec::new()),
                requests: RefCell::new(Vec::new()),
            }
        }

        fn failing(message: &str) -> Self {
            Self {
                answer: Err(message.to_string()),
                model_list: Err(message.to_string()),
                seen: RefCell::new(Vec::new()),
                requests: RefCell::new(Vec::new()),
            }
        }

        fn listing(models: Vec<ModelInfo>) -> Self {
            Self {
                answer: Ok(ChatResponse::default()),
                model_list: Ok(models),
                seen: RefCell::new(Vec::new()),
                requests: RefCell::new(Vec::new()),
            }
        }

        /// What the backend was told, without what it was asked.
        fn record(&self, config: &AiConfig) {
            self.seen.borrow_mut().push(Seen {
                base_url: config.base_url().to_string(),
                model: config.model().to_string(),
                has_key: config.has_key(),
            });
        }

        fn seen(&self) -> Vec<Seen> {
            self.seen.borrow().clone()
        }

        fn requests(&self) -> Vec<ChatRequest> {
            self.requests.borrow().clone()
        }
    }

    impl Backend for FakeBackend {
        fn models(&self, config: &AiConfig) -> Result<Vec<ModelInfo>, String> {
            self.record(config);
            self.model_list.clone()
        }

        fn chat(&self, config: &AiConfig, request: ChatRequest) -> Result<ChatResponse, String> {
            self.record(config);
            self.requests.borrow_mut().push(request);
            self.answer.clone()
        }
    }

    /// The turn a recorded endpoint answered: one line of prose, one tool call
    /// whose arguments are the JSON text the wire carries, and its usage.
    fn recorded_answer() -> ChatResponse {
        ChatResponse {
            text: "Let me look that up.".to_string(),
            tool_calls: vec![ToolCall {
                id: "call_1".to_string(),
                name: "transactions".to_string(),
                arguments: "{\"account\":\"Expenses:Food\",\"month\":\"2026-03\"}".to_string(),
            }],
            usage: Some(Usage {
                prompt_tokens: 12,
                completion_tokens: 7,
                total_tokens: 19,
            }),
            finish_reason: Some("tool_calls".to_string()),
            cancelled: false,
        }
    }

    fn configured(dir: &TempDir) -> AiSettingsView {
        configure(
            dir.dir(),
            "https://example.test/v1".to_string(),
            Some("sk-secret".to_string()),
            "test-model".to_string(),
            Some(2048),
            Some(0.1),
        )
        .expect("the settings save")
    }

    #[test]
    fn settings_round_trip_without_the_key() {
        let dir = TempDir::new("ai-settings");
        let view = configured(&dir);
        assert_eq!(view.base_url, "https://example.test/v1");
        assert_eq!(view.model, "test-model");
        assert_eq!(view.max_tokens, Some(2048));
        assert_eq!(view.temperature, Some(0.1));
        assert!(view.has_key);

        // What was saved is what comes back.
        assert_eq!(settings(dir.dir()).expect("the settings load"), view);

        // Nothing the panel can see carries the key, while the file holds it:
        // the key is stored, not echoed.
        let json = serde_json::to_string(&view).expect("the view serializes");
        assert!(
            !json.contains("sk-secret"),
            "the view leaks the key: {json}"
        );
        assert!(
            json.contains("\"has_key\":true"),
            "the view says a key is set"
        );
        let stored = std::fs::read_to_string(dir.file("ai.json")).expect("the config was written");
        assert!(
            stored.contains("sk-secret"),
            "the key is what the file is for"
        );
    }

    #[test]
    fn settings_without_a_file_are_the_defaults() {
        let dir = TempDir::new("ai-defaults");
        let view = settings(dir.dir()).expect("an empty directory answers the defaults");
        assert_eq!(view.base_url, "https://api.openai.com/v1");
        assert_eq!(view.model, "");
        assert_eq!(view.max_tokens, None);
        assert_eq!(view.temperature, None);
        assert!(!view.has_key);
    }

    #[test]
    fn configure_without_a_key_keeps_the_stored_one() {
        let dir = TempDir::new("ai-keep-key");
        configured(&dir);
        let view = configure(
            dir.dir(),
            "https://other.test/v1".to_string(),
            None,
            "other-model".to_string(),
            Some(512),
            Some(0.5),
        )
        .expect("the settings save without a key");
        assert!(view.has_key, "a save without a key must not forget one");
        assert_eq!(view.base_url, "https://other.test/v1");
        assert_eq!(view.model, "other-model");
        assert_eq!(view.max_tokens, Some(512));
        assert_eq!(view.temperature, Some(0.5));
        let stored = std::fs::read_to_string(dir.file("ai.json")).expect("the config was written");
        assert!(stored.contains("sk-secret"), "the stored key survived");
        assert!(stored.contains("other-model"), "the rest was replaced");
    }

    #[test]
    fn configure_with_an_empty_key_clears_it() {
        let dir = TempDir::new("ai-clear-key");
        configured(&dir);
        let view = configure(
            dir.dir(),
            "https://example.test/v1".to_string(),
            Some(String::new()),
            "test-model".to_string(),
            None,
            None,
        )
        .expect("the settings save");
        assert!(!view.has_key, "an empty key is a deliberate clear");
        let stored = std::fs::read_to_string(dir.file("ai.json")).expect("the config was written");
        assert!(
            !stored.contains("sk-secret"),
            "the key was replaced, not kept"
        );
    }

    #[test]
    fn the_tool_table_is_well_formed_and_unique() {
        let schemas = tools();
        assert_eq!(
            schemas
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            [
                "query",
                "accounts",
                "transactions",
                "grep",
                "read_lines",
                "files",
                "budget_range",
                "index_series",
                "lifestyle_report",
                "price_targets",
                "add_transaction",
                "write_file",
            ],
            "the panel offers the reads it can execute, then the two writes it stages"
        );
        let names: BTreeSet<&str> = schemas.iter().map(|tool| tool.name.as_str()).collect();
        assert_eq!(names.len(), schemas.len(), "the model dispatches by name");

        // The writes are the last two, and only those two; an unknown name is
        // not read-only either, so a dispatcher fails closed.
        let writes: Vec<&str> = TOOLS
            .iter()
            .filter(|spec| !spec.read_only)
            .map(|spec| spec.name)
            .collect();
        assert_eq!(writes, ["add_transaction", "write_file"]);
        for spec in TOOLS {
            assert_eq!(
                is_read_only(spec.name),
                spec.read_only,
                "{}: the read-only flag is what dispatchers ask for",
                spec.name
            );
        }
        assert!(!is_read_only("delete_everything"));

        for spec in TOOLS {
            let description = spec.description.to_lowercase();
            if spec.read_only {
                assert!(
                    !description.contains("proposal"),
                    "{}: a read never stages a proposal",
                    spec.name
                );
            } else {
                assert!(
                    description.contains("proposal"),
                    "{}: a write must say it only stages a proposal",
                    spec.name
                );
                assert!(
                    description.contains("approve"),
                    "{}: a write must say the user approves it",
                    spec.name
                );
            }
        }

        for tool in &schemas {
            assert!(!tool.name.trim().is_empty());
            assert!(
                !tool.description.trim().is_empty(),
                "{}: a tool without a description cannot be chosen",
                tool.name
            );
            let parameters = tool
                .parameters
                .as_object()
                .unwrap_or_else(|| panic!("{}: the parameters are not a JSON object", tool.name));
            assert_eq!(
                parameters.get("type").and_then(Value::as_str),
                Some("object"),
                "{}: arguments are an object",
                tool.name
            );
            assert_eq!(
                parameters
                    .get("additionalProperties")
                    .and_then(Value::as_bool),
                Some(false),
                "{}: unknown arguments are refused",
                tool.name
            );
            let properties = parameters
                .get("properties")
                .and_then(Value::as_object)
                .unwrap_or_else(|| panic!("{}: no properties", tool.name));
            let required: Vec<&str> = parameters
                .get("required")
                .map(|required| {
                    required
                        .as_array()
                        .unwrap_or_else(|| panic!("{}: required is not a list", tool.name))
                        .iter()
                        .filter_map(Value::as_str)
                        .collect()
                })
                .unwrap_or_default();
            for name in &required {
                assert!(
                    properties.contains_key(*name),
                    "{}: required `{name}` is not a property",
                    tool.name
                );
            }
            for (name, property) in properties {
                let kind = property.get("type").and_then(Value::as_str);
                assert!(
                    matches!(kind, Some("string" | "integer" | "array")),
                    "{}: `{name}` has type {kind:?}",
                    tool.name
                );
                assert!(
                    property
                        .get("description")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.trim().is_empty()),
                    "{}: `{name}` has no description",
                    tool.name
                );
                // A posting array is the one nested shape; its items are the
                // `NewPosting` fields, and each is documented like a top-level
                // argument.
                if kind == Some("array") {
                    let items = property
                        .get("items")
                        .and_then(Value::as_object)
                        .unwrap_or_else(|| panic!("{}: `{name}` has no item schema", tool.name));
                    assert_eq!(
                        items.get("type").and_then(Value::as_str),
                        Some("object"),
                        "{}: `{name}` items are objects",
                        tool.name
                    );
                    assert_eq!(
                        items.get("additionalProperties").and_then(Value::as_bool),
                        Some(false),
                        "{}: `{name}` items refuse unknown fields",
                        tool.name
                    );
                    let fields = items
                        .get("properties")
                        .and_then(Value::as_object)
                        .unwrap_or_else(|| panic!("{}: `{name}` items have no fields", tool.name));
                    for field in ["account", "units", "currency"] {
                        let shape = fields
                            .get(field)
                            .unwrap_or_else(|| panic!("{}: `{name}` lacks `{field}`", tool.name));
                        assert_eq!(
                            shape.get("type").and_then(Value::as_str),
                            Some("string"),
                            "{}: `{name}.{field}` is a string",
                            tool.name
                        );
                        assert!(
                            shape
                                .get("description")
                                .and_then(Value::as_str)
                                .is_some_and(|text| !text.trim().is_empty()),
                            "{}: `{name}.{field}` has no description",
                            tool.name
                        );
                    }
                    let item_required: Vec<&str> = items
                        .get("required")
                        .and_then(Value::as_array)
                        .map(|list| list.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    assert_eq!(
                        item_required,
                        ["account"],
                        "{}: `{name}` needs only the account",
                        tool.name
                    );
                }
            }
        }
    }

    #[test]
    fn the_system_prompt_states_the_rules() {
        let prompt = SYSTEM_PROMPT.to_lowercase();
        assert!(!prompt.trim().is_empty());
        for needle in [
            "beancount",
            "decimal string",
            "floating point",
            "never",
            "tool",
            "language",
            "applied",
            "proposal",
            "approve",
        ] {
            assert!(
                prompt.contains(needle),
                "the prompt does not mention `{needle}`"
            );
        }
    }

    #[test]
    fn chat_answers_a_tool_call_from_the_configured_endpoint() {
        let dir = TempDir::new("ai-chat");
        configured(&dir);
        let backend = FakeBackend::answering(recorded_answer());
        let request = ChatRequest {
            messages: vec![
                ChatMessage::text(ChatRole::System, SYSTEM_PROMPT),
                ChatMessage::text(ChatRole::User, "What did I spend on food in March?"),
            ],
            tools: tools(),
            media: Vec::new(),
        };
        let answer = chat_with(&backend, dir.dir(), request.clone()).expect("the turn completes");

        // The recorded turn arrives unchanged; the panel dispatches the call by
        // name and passes its arguments through as the text they are.
        assert_eq!(answer, recorded_answer());
        assert_eq!(answer.tool_calls[0].name, "transactions");
        assert_eq!(
            answer.tool_calls[0].arguments,
            "{\"account\":\"Expenses:Food\",\"month\":\"2026-03\"}"
        );
        assert_eq!(answer.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(
            answer.usage.map(|usage| usage.total_tokens),
            Some(19),
            "the usage the endpoint reported is kept for the panel"
        );

        // And it went to the endpoint the directory points at, with the
        // conversation exactly as the panel built it.
        assert_eq!(
            backend.seen(),
            vec![Seen {
                base_url: "https://example.test/v1".to_string(),
                model: "test-model".to_string(),
                has_key: true,
            }]
        );
        assert_eq!(backend.requests(), vec![request]);
    }

    #[test]
    fn chat_passes_the_endpoint_error_through() {
        let dir = TempDir::new("ai-chat-error");
        let backend = FakeBackend::failing("the endpoint rejected the API key (401)");
        let error =
            chat_with(&backend, dir.dir(), ChatRequest::default()).expect_err("the turn fails");
        assert_eq!(error, "the endpoint rejected the API key (401)");
    }

    #[test]
    fn models_lists_what_the_endpoint_publishes() {
        let dir = TempDir::new("ai-models");
        let model = ModelInfo {
            id: "test-model".to_string(),
            context_window: Some(32_768),
            vision: Some(true),
        };
        let backend = FakeBackend::listing(vec![model.clone()]);
        assert_eq!(
            models_with(&backend, dir.dir()).expect("the list loads"),
            vec![model]
        );
        // An unconfigured directory asks the default endpoint, without a key.
        assert_eq!(
            backend.seen(),
            vec![Seen {
                base_url: "https://api.openai.com/v1".to_string(),
                model: String::new(),
                has_key: false,
            }]
        );
    }
}
