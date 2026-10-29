// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The journal's language server, embedded in the Tauri process.
//!
//! `rustledger-lsp` has two front doors: `start_stdio`, which owns a process's
//! stdin/stdout, and `run_main_loop`, which takes the two channels a transport
//! would otherwise feed. The editor lives inside the app, so this module is the
//! transport: the webview's messages arrive through a Tauri command and are
//! pushed onto the loop's channel, and the loop's replies are emitted back as
//! events.
//!
//! `run_main_loop` expects the `initialize` exchange to be over — `start_stdio`
//! consumes it before entering the loop — so `Session::start` answers that one
//! request itself and then hands the same receiver to the loop. Everything
//! after it (`initialized`, `didOpen`, `didChange`, `didClose`, completion,
//! hover, `shutdown`, `exit`) is the server's own dispatch.
//!
//! Positions are negotiated as UTF-8: the handlers emit byte offsets, and the
//! editor converts its UTF-16 columns at this boundary.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender};
use lsp_server::{ErrorCode, Message, Response};
use lsp_types::{
    CompletionOptions, HoverProviderCapability, InitializeParams, InitializeResult,
    PositionEncodingKind, ServerCapabilities, ServerInfo, TextDocumentSyncCapability,
    TextDocumentSyncKind,
};
use rustledger_lsp::handlers::utils::PositionEncoding;
use rustledger_lsp::run_main_loop;

/// The server the app holds, if any. The state lives here rather than in
/// [`AppState`](crate::session::AppState) because it is the language server's
/// own lifetime: a started server captures the journal path it loaded and is
/// replaced — never mutated — when the ledger is reopened.
static SESSION: Mutex<Option<Session>> = Mutex::new(None);

/// One running language server and the channel its client speaks on.
///
/// Dropping it closes the client side of the loop, which is how the server
/// thread is asked to stop: the main loop breaks on a closed channel, and the
/// event thread ends once the loop drops its sender.
pub struct Session {
    sender: Sender<Message>,
    journal: PathBuf,
    /// Set once the `initialize` exchange has been answered. A client that
    /// reattaches after a failed start must not send `initialize` a second
    /// time: the loop treats it as a protocol violation.
    initialized: Arc<AtomicBool>,
    /// Kept only so the thread is not detached silently; it ends with the
    /// channel, so shutdown never joins it.
    _server: JoinHandle<()>,
    /// Feeds the caller's emitter with every message the server emits.
    _events: JoinHandle<()>,
}

impl Session {
    /// Start a server for `journal` and forward every message it emits to
    /// `emit` as JSON.
    ///
    /// The journal is loaded by the server itself, from the path handed over —
    /// that loaded ledger is what supplies accounts, currencies, payees and
    /// cross-file diagnostics, so it must be the root journal rather than the
    /// file that happens to be open.
    pub fn start(journal: PathBuf, emit: impl Fn(String) + Send + 'static) -> Result<Self, String> {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let (server_sender, server_receiver) = crossbeam_channel::unbounded();

        let server_journal = journal.clone();
        let initialized = Arc::new(AtomicBool::new(false));
        let server_initialized = Arc::clone(&initialized);
        let server = std::thread::Builder::new()
            .name("rustledger-lsp".into())
            .spawn(move || {
                let receiver = match initialize(receiver, &server_sender) {
                    Some(receiver) => receiver,
                    None => return,
                };
                server_initialized.store(true, Ordering::SeqCst);
                let _ = run_main_loop(
                    receiver,
                    server_sender,
                    Some(server_journal),
                    PositionEncoding::Utf8,
                );
            })
            .map_err(|e| format!("cannot start the language server: {e}"))?;

        let events = std::thread::Builder::new()
            .name("rustledger-lsp-events".into())
            .spawn(move || {
                for message in server_receiver {
                    if let Ok(json) = serde_json::to_string(&message) {
                        emit(json);
                    }
                }
            })
            .map_err(|e| format!("cannot start the language server bridge: {e}"))?;

        Ok(Self {
            sender,
            journal,
            initialized,
            _server: server,
            _events: events,
        })
    }

    /// The root journal this server loaded; the same path means the same
    /// server, and a different one means the ledger was reopened.
    pub fn journal(&self) -> &Path {
        &self.journal
    }

    /// Whether the `initialize` exchange has already happened.
    pub fn initialized(&self) -> bool {
        self.initialized.load(Ordering::SeqCst)
    }

    /// Hand one message to the server. Ordering is the channel's: a
    /// notification sent before a request is handled before it.
    pub fn send(&self, message: Message) -> Result<(), String> {
        self.sender
            .send(message)
            .map_err(|_| "the language server is not running".to_string())
    }
}

/// Start the server for `journal`, or keep the one already running for it.
///
/// The result says whether the caller still has to send `initialize`: a new
/// server is waiting for it, while one an earlier editor already initialized
/// refuses a second one. Dropping the previous session closes its channel,
/// which is the server's stop signal, so a journal change needs no explicit
/// shutdown message here.
pub(crate) fn start(
    journal: PathBuf,
    emit: impl Fn(String) + Send + 'static,
) -> Result<bool, String> {
    let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(running) = session.as_ref()
        && running.journal() == journal.as_path()
    {
        return Ok(!running.initialized());
    }
    *session = None;
    *session = Some(Session::start(journal, emit)?);
    Ok(true)
}

/// Hand one JSON-RPC message to the running server.
pub(crate) fn send(message: &str) -> Result<(), String> {
    let message: Message = serde_json::from_str(message)
        .map_err(|e| format!("invalid language server message: {e}"))?;
    let guard = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    match guard.as_ref() {
        Some(session) => session.send(message),
        None => Err("the language server is not running".to_string()),
    }
}

/// Stop the running server, if any. The next editor to ask starts one for the
/// journal as it stands then.
pub(crate) fn stop() {
    *SESSION.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Consume the single `initialize` request and advertise the capabilities the
/// editor actually uses, leaving the receiver for [`run_main_loop`].
///
/// Returns `None` — after answering with an error, when it can — if the first
/// message is not a well-formed `initialize`; the loop cannot be started
/// halfway through its lifecycle.
fn initialize(receiver: Receiver<Message>, sender: &Sender<Message>) -> Option<Receiver<Message>> {
    let request = match receiver.recv() {
        Ok(Message::Request(request)) if request.method == "initialize" => request,
        _ => return None,
    };
    let id = request.id.clone();
    match serde_json::from_value::<InitializeParams>(request.params) {
        Ok(_) => {
            let result = InitializeResult {
                capabilities: capabilities(),
                server_info: Some(ServerInfo {
                    name: "rustledger-lsp".to_string(),
                    version: Some(rustledger_lsp::VERSION.to_string()),
                }),
            };
            let _ = sender.send(Message::Response(Response::new_ok(id, result)));
            Some(receiver)
        }
        Err(error) => {
            let _ = sender.send(Message::Response(Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                error.to_string(),
            )));
            None
        }
    }
}

/// What the editor consumes — not everything the server can answer. The
/// trigger characters are the server's own list, so completion opens on the
/// same positions its handlers understand.
fn capabilities() -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(PositionEncodingKind::UTF8),
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![
                ":".to_string(),
                " ".to_string(),
                "\"".to_string(),
                "#".to_string(),
                "^".to_string(),
            ]),
            ..Default::default()
        }),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_server::Request;
    use std::time::Duration;

    /// One message from the bridge, parsed back from the event payload.
    fn receive(events: &Receiver<String>) -> serde_json::Value {
        let json = events
            .recv_timeout(Duration::from_secs(15))
            .expect("the language server sent no message");
        serde_json::from_str(&json).expect("the bridge emitted invalid JSON")
    }

    /// Wait for a notification with `method`, skipping anything ahead of it.
    fn receive_notification(events: &Receiver<String>, method: &str) -> serde_json::Value {
        loop {
            let message = receive(events);
            if message["method"] == method {
                return message;
            }
        }
    }

    /// Wait for the response to request `id`, skipping notifications and
    /// server-initiated requests (the server registers file watchers).
    fn receive_response(events: &Receiver<String>, id: i32) -> serde_json::Value {
        loop {
            let message = receive(events);
            if message["id"].as_i64() == Some(i64::from(id)) {
                return message;
            }
        }
    }

    /// The client sends JSON-RPC objects without the `jsonrpc` member (the
    /// stdio transport adds it), and Rust parses them through the untagged
    /// `lsp_server::Message`. Pin the three shapes the bridge relies on.
    #[test]
    fn the_webview_wire_shape_parses() {
        let request: lsp_server::Message =
            serde_json::from_str(r#"{"id":1,"method":"textDocument/completion","params":{}}"#)
                .expect("a request parses");
        assert!(
            matches!(request, Message::Request(request) if request.method == "textDocument/completion")
        );

        let notification: lsp_server::Message =
            serde_json::from_str(r#"{"method":"initialized","params":{}}"#)
                .expect("a notification parses");
        assert!(
            matches!(notification, Message::Notification(notification) if notification.method == "initialized")
        );

        let response: lsp_server::Message =
            serde_json::from_str(r#"{"id":"register-file-watchers","result":null}"#)
                .expect("a response parses");
        assert!(
            matches!(response, Message::Response(response) if response.id.to_string().contains("register-file-watchers"))
        );
    }

    /// The whole pipeline the editor uses: initialize, open the journal, then
    /// ask for completions at the head of a posting. The ledger comes from the
    /// repository's own `main.bean`, so the accounts in the reply are the ones
    /// opened there (and only reachable through the loaded ledger, since the
    /// buffer is parsed on its own).
    #[test]
    fn serves_completions_from_the_loaded_journal() {
        let journal =
            std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../main.bean"))
                .expect("the repository's main.bean exists");
        let text = std::fs::read_to_string(&journal).expect("main.bean is readable");

        let (emitted, events) = crossbeam_channel::unbounded();
        let session = Session::start(journal.clone(), move |message| {
            let _ = emitted.send(message);
        })
        .expect("the language server starts");

        session
            .send(Message::Request(Request::new(
                1.into(),
                "initialize".to_string(),
                serde_json::json!({
                    "capabilities": {
                        "general": { "positionEncodings": ["utf-8"] }
                    }
                }),
            )))
            .expect("initialize is queued");

        let started = receive_response(&events, 1);
        assert_eq!(
            started["result"]["capabilities"]["completionProvider"]["triggerCharacters"][0],
            ":"
        );
        assert_eq!(
            started["result"]["capabilities"]["positionEncoding"],
            "utf-8"
        );

        session
            .send(Message::Notification(lsp_server::Notification::new(
                "initialized".to_string(),
                serde_json::json!({}),
            )))
            .expect("initialized is queued");

        let uri = rustledger_lsp::path_to_uri(&journal)
            .expect("the journal path is a file URI")
            .as_str()
            .to_string();
        session
            .send(Message::Notification(lsp_server::Notification::new(
                "textDocument/didOpen".to_string(),
                serde_json::json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": "beancount",
                        "version": 1,
                        "text": text,
                    }
                }),
            )))
            .expect("didOpen is queued");

        let diagnostics = receive_notification(&events, "textDocument/publishDiagnostics");
        assert_eq!(diagnostics["params"]["uri"], uri);
        assert_eq!(diagnostics["params"]["diagnostics"], serde_json::json!([]));

        // The posting line in the repository's journal, at its indentation:
        // the completion context is an account name.
        let posting = text
            .lines()
            .position(|line| line.trim_start().starts_with("Expenses:Food"))
            .expect("main.bean posts to Expenses:Food");
        session
            .send(Message::Request(Request::new(
                2.into(),
                "textDocument/completion".to_string(),
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": posting, "character": 2 },
                }),
            )))
            .expect("completion is queued");

        let completions = receive_response(&events, 2);
        let labels: Vec<&str> = completions["result"]
            .as_array()
            .expect("completion returns an array")
            .iter()
            .filter_map(|item| item["label"].as_str())
            .collect();
        assert!(
            labels.contains(&"Expenses:Food"),
            "expected the journal's accounts, got {labels:?}"
        );

        // Hover reads the same account out of the ledger's `open` directive.
        session
            .send(Message::Request(Request::new(
                3.into(),
                "textDocument/hover".to_string(),
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": posting, "character": 4 },
                }),
            )))
            .expect("hover is queued");
        let hover = receive_response(&events, 3);
        assert!(
            hover["result"].to_string().contains("Expenses:Food"),
            "expected the account's `open` info, got {}",
            hover["result"]
        );

        // The single-transaction editor opens the entry's own lines under a
        // document key of their own — never the file's URI, which would make
        // the server treat these lines as the whole journal. The loaded ledger
        // is still what supplies the accounts, including ones the fragment
        // never mentions.
        let header = text
            .lines()
            .position(|line| line.starts_with("2026-01-10 * \"Groceries\""))
            .expect("main.bean has the groceries entry");
        let fragment = text
            .lines()
            .skip(header)
            .take_while(|line| !line.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let entry_path =
            std::path::absolute(".rustledger-entry-1.bean").expect("a document key is absolute");
        let entry_uri = rustledger_lsp::path_to_uri(&entry_path)
            .expect("the document key is a file URI")
            .as_str()
            .to_string();
        session
            .send(Message::Notification(lsp_server::Notification::new(
                "textDocument/didOpen".to_string(),
                serde_json::json!({
                    "textDocument": {
                        "uri": entry_uri,
                        "languageId": "beancount",
                        "version": 1,
                        "text": fragment,
                    }
                }),
            )))
            .expect("the entry fragment opens");

        session
            .send(Message::Request(Request::new(
                4.into(),
                "textDocument/completion".to_string(),
                serde_json::json!({
                    "textDocument": { "uri": entry_uri },
                    "position": { "line": 1, "character": 2 },
                }),
            )))
            .expect("fragment completion is queued");
        let fragment_completions = receive_response(&events, 4);
        let fragment_labels: Vec<&str> = fragment_completions["result"]
            .as_array()
            .expect("completion returns an array")
            .iter()
            .filter_map(|item| item["label"].as_str())
            .collect();
        assert!(
            fragment_labels.contains(&"Expenses:Food"),
            "expected the fragment's own account, got {fragment_labels:?}"
        );
        assert!(
            fragment_labels.contains(&"Income:Salary"),
            "expected the ledger's other accounts, got {fragment_labels:?}"
        );

        session
            .send(Message::Request(Request::new(
                5.into(),
                "textDocument/hover".to_string(),
                serde_json::json!({
                    "textDocument": { "uri": entry_uri },
                    "position": { "line": 2, "character": 4 },
                }),
            )))
            .expect("fragment hover is queued");
        let fragment_hover = receive_response(&events, 5);
        assert!(
            fragment_hover["result"]
                .to_string()
                .contains("Expenses:Food"),
            "expected the fragment's account hover, got {}",
            fragment_hover["result"]
        );

        session
            .send(Message::Notification(lsp_server::Notification::new(
                "textDocument/didClose".to_string(),
                serde_json::json!({ "textDocument": { "uri": entry_uri } }),
            )))
            .expect("the fragment closes");

        session
            .send(Message::Request(Request::new(
                6.into(),
                "shutdown".to_string(),
                serde_json::Value::Null,
            )))
            .expect("shutdown is queued");
        assert_eq!(
            receive_response(&events, 6)["result"],
            serde_json::Value::Null
        );

        session
            .send(Message::Notification(lsp_server::Notification::new(
                "exit".to_string(),
                serde_json::Value::Null,
            )))
            .expect("exit is queued");
    }
}
