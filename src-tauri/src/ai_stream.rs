// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Streaming chat turns, wired the way a command needs them: load the
//! assistant's configuration from the app's directory, stream the turn, and
//! hand every delta to the caller as it arrives.
//!
//! [`crate::ai`] owns the wire and is deliberately transport-agnostic; this is
//! the one place that pair — *which directory holds the configuration* and
//! *where the deltas go* — is fixed. It knows nothing about Tauri: the command
//! layer passes a closure that emits each event as `ai://delta` and returns
//! [`ControlFlow::Break`] when the frontend cancelled, which stops the read at
//! once.
//!
//! # The event shape
//!
//! [`StreamEvent`] crosses the IPC boundary as an internally tagged object —
//! `{"kind": …}` with `snake_case` tags — so a listener can switch on one
//! field instead of probing for whichever variant is present:
//!
//! ```json
//! {"kind": "text_delta", "text": "Here is "}
//! {"kind": "tool_call_delta", "index": 0, "id": "call_1", "name": "query", "arguments": "{\"query\":"}
//! {"kind": "tool_call_delta", "index": 0, "id": null, "name": null, "arguments": "\"SELECT date\"}"}
//! {"kind": "usage", "prompt_tokens": 120, "completion_tokens": 8, "total_tokens": 128}
//! {"kind": "done", "finish_reason": "tool_calls"}
//! ```
//!
//! A tool call arrives in fragments keyed by `index`; its `id` and `name` are
//! on the fragment that first announces it and `null` afterwards, so the
//! frontend appends `arguments` and never has to guess which call a fragment
//! belongs to. [`crate::ai::ChatResponse`] is still the assembled answer: the
//! command returns it when the stream ends, so the caller has one place to read
//! the final text, calls and token usage even if an event was dropped.
//!
//! # Why the seam
//!
//! [`crate::ai::stream`] goes to the network; a test cannot. `ai`'s own tests
//! pin the SSE reading with scripted bodies behind its private transport. This
//! module's contract is the part above that wire — the configuration really
//! comes from `dir`, every delta is forwarded in order, and `Break` stops it —
//! so the tests script the deltas and assert exactly that.

use std::ops::ControlFlow;
use std::path::Path;

use crate::ai::{self, AiConfig, ChatRequest, ChatResponse, StreamEvent};

/// One streaming turn from the configuration in `dir`.
///
/// `emit` sees each delta in the order the endpoint produced it; returning
/// [`ControlFlow::Break`] cancels the read, and the answered
/// [`ChatResponse`] then carries `cancelled` with the text produced so far.
///
/// # Errors
///
/// A configuration file that cannot be read or parsed, or the endpoint failing
/// (unreachable, a non-2xx answer, a malformed stream) — the messages
/// [`crate::ai`] already classifies.
pub fn chat_stream(
    dir: &Path,
    request: ChatRequest,
    emit: impl FnMut(StreamEvent) -> ControlFlow<()>,
) -> Result<ChatResponse, String> {
    chat_stream_with(&Endpoint, dir, request, emit)
}

/// The production streamer: the real endpoint through [`crate::ai`].
struct Endpoint;

impl Streamer for Endpoint {
    fn stream(
        &self,
        config: &AiConfig,
        request: ChatRequest,
        emit: &mut dyn FnMut(StreamEvent) -> ControlFlow<()>,
    ) -> Result<ChatResponse, String> {
        ai::stream(config, request, |event| emit(event))
    }
}

/// What [`chat_stream`] needs from the wire, behind a seam the tests replace.
trait Streamer {
    fn stream(
        &self,
        config: &AiConfig,
        request: ChatRequest,
        emit: &mut dyn FnMut(StreamEvent) -> ControlFlow<()>,
    ) -> Result<ChatResponse, String>;
}

/// [`chat_stream`] with the endpoint as a parameter — the same two steps, with
/// the second one swappable.
fn chat_stream_with(
    streamer: &impl Streamer,
    dir: &Path,
    request: ChatRequest,
    mut emit: impl FnMut(StreamEvent) -> ControlFlow<()>,
) -> Result<ChatResponse, String> {
    let config = AiConfig::load(dir)?;
    streamer.stream(&config, request, &mut emit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{ChatMessage, ChatRole, ToolCall, ToolSchema, Usage};
    use crate::testing::TempDir;
    use serde_json::json;
    use std::cell::{Cell, RefCell};
    use std::fs;

    /// What the endpoint was handed, for the assertions that the
    /// configuration came from the directory.
    #[derive(Debug, Default, PartialEq, Eq)]
    struct Seen {
        base_url: String,
        model: String,
        tools: Vec<String>,
    }

    /// A scripted streamer: it plays `events` through `emit` and answers the
    /// response the real one would have assembled, with `cancelled` set when
    /// the caller broke out early.
    struct Scripted {
        events: Vec<StreamEvent>,
        answer: ChatResponse,
        failure: Option<String>,
        seen: RefCell<Seen>,
        cancelled: Cell<bool>,
    }

    impl Scripted {
        fn new(events: Vec<StreamEvent>, answer: ChatResponse) -> Self {
            Self {
                events,
                answer,
                failure: None,
                seen: RefCell::default(),
                cancelled: Cell::new(false),
            }
        }

        fn failing(message: &str) -> Self {
            Self {
                failure: Some(message.to_string()),
                ..Self::new(Vec::new(), ChatResponse::default())
            }
        }
    }

    impl Streamer for Scripted {
        fn stream(
            &self,
            config: &AiConfig,
            request: ChatRequest,
            emit: &mut dyn FnMut(StreamEvent) -> ControlFlow<()>,
        ) -> Result<ChatResponse, String> {
            *self.seen.borrow_mut() = Seen {
                base_url: config.base_url().to_string(),
                model: config.model().to_string(),
                tools: request.tools.iter().map(|tool| tool.name.clone()).collect(),
            };
            if let Some(message) = &self.failure {
                return Err(message.clone());
            }
            for event in self.events.clone() {
                if emit(event) == ControlFlow::Break(()) {
                    self.cancelled.set(true);
                    return Ok(ChatResponse {
                        cancelled: true,
                        ..self.answer.clone()
                    });
                }
            }
            Ok(self.answer.clone())
        }
    }

    /// The delta script of a turn that answers prose and then asks for a tool.
    fn tool_call_script() -> Vec<StreamEvent> {
        vec![
            StreamEvent::TextDelta {
                text: "Looking ".into(),
            },
            StreamEvent::TextDelta {
                text: "that up.".into(),
            },
            StreamEvent::ToolCallDelta {
                index: 0,
                id: Some("call_1".into()),
                name: Some("query".into()),
                arguments: "{\"query\":".into(),
            },
            StreamEvent::ToolCallDelta {
                index: 0,
                id: None,
                name: None,
                arguments: "\"SELECT date\"}".into(),
            },
            StreamEvent::Usage {
                prompt_tokens: 120,
                completion_tokens: 8,
                total_tokens: 128,
            },
            StreamEvent::Done {
                finish_reason: Some("tool_calls".into()),
            },
        ]
    }

    /// A directory holding a saved configuration and nothing else.
    fn configured_dir() -> TempDir {
        let dir = TempDir::new("ai-stream");
        AiConfig::default()
            .with_base_url("http://ledger.test/v1")
            .with_model("ledger-model")
            .save(dir.dir())
            .expect("the config saves");
        dir
    }

    #[test]
    fn a_scripted_run_forwards_deltas_then_the_tool_call() {
        let dir = configured_dir();
        let scripted = Scripted::new(
            tool_call_script(),
            ChatResponse {
                text: "Looking that up.".into(),
                tool_calls: vec![ToolCall {
                    id: "call_1".into(),
                    name: "query".into(),
                    arguments: "{\"query\":\"SELECT date\"}".into(),
                }],
                usage: Some(Usage {
                    prompt_tokens: 120,
                    completion_tokens: 8,
                    total_tokens: 128,
                }),
                finish_reason: Some("tool_calls".into()),
                cancelled: false,
            },
        );

        let mut forwarded = Vec::new();
        let answer = chat_stream_with(
            &scripted,
            dir.dir(),
            ChatRequest {
                messages: vec![ChatMessage::text(ChatRole::User, "What did I spend?")],
                tools: vec![ToolSchema {
                    name: "query".into(),
                    description: "Run BQL".into(),
                    parameters: json!({"type": "object"}),
                }],
                media: Vec::new(),
            },
            |event| {
                forwarded.push(event);
                ControlFlow::Continue(())
            },
        )
        .expect("the run finishes");

        assert_eq!(
            forwarded,
            tool_call_script(),
            "every delta reaches the caller in order"
        );
        // The tool call arrives after the prose, in fragments keyed by index.
        assert!(matches!(
            (&forwarded[2], &forwarded[3]),
            (
                StreamEvent::ToolCallDelta {
                    index: 0,
                    name: Some(_),
                    ..
                },
                StreamEvent::ToolCallDelta {
                    index: 0,
                    name: None,
                    ..
                }
            )
        ));
        assert_eq!(answer.text, "Looking that up.");
        assert_eq!(answer.tool_calls[0].name, "query");
        assert!(!answer.cancelled);
        assert_eq!(
            *scripted.seen.borrow(),
            Seen {
                base_url: "http://ledger.test/v1".into(),
                model: "ledger-model".into(),
                tools: vec!["query".into()],
            },
            "the configuration and tools came from the directory and the request"
        );
    }

    #[test]
    fn a_break_stops_the_read() {
        let dir = configured_dir();
        let scripted = Scripted::new(
            tool_call_script(),
            ChatResponse {
                text: "Looking that up.".into(),
                ..ChatResponse::default()
            },
        );

        let mut forwarded = 0;
        let answer = chat_stream_with(&scripted, dir.dir(), ChatRequest::default(), |_| {
            forwarded += 1;
            ControlFlow::Break(())
        })
        .expect("a cancelled run is not an error");

        assert_eq!(forwarded, 1, "the reader stops at the first Break");
        assert!(scripted.cancelled.get());
        assert!(
            answer.cancelled,
            "the answer says the caller stopped it, and keeps the text so far"
        );
        assert_eq!(answer.text, "Looking that up.");
    }

    #[test]
    fn no_configuration_file_reads_the_defaults() {
        let dir = TempDir::new("ai-stream-empty");
        let scripted = Scripted::new(Vec::new(), ChatResponse::default());
        let mut forwarded = 0;
        chat_stream_with(&scripted, dir.dir(), ChatRequest::default(), |_| {
            forwarded += 1;
            ControlFlow::Continue(())
        })
        .expect("a directory with no config answers the defaults");
        assert_eq!(forwarded, 0);
        assert_eq!(
            scripted.seen.borrow().base_url,
            AiConfig::default().base_url(),
            "the fresh configuration is the one the settings panel opens on"
        );
    }

    #[test]
    fn a_broken_configuration_fails_before_any_event() {
        let dir = TempDir::new("ai-stream-broken");
        fs::write(dir.file("ai.json"), "{").unwrap();
        let scripted = Scripted::new(tool_call_script(), ChatResponse::default());
        let mut forwarded = 0;
        let error = chat_stream_with(&scripted, dir.dir(), ChatRequest::default(), |_| {
            forwarded += 1;
            ControlFlow::Continue(())
        })
        .expect_err("a config file that does not parse is an error");
        assert!(
            error.contains("ai.json"),
            "the error names the file: {error}"
        );
        assert_eq!(forwarded, 0, "nothing was streamed");
    }

    #[test]
    fn an_endpoint_failure_is_returned() {
        let dir = configured_dir();
        let error = chat_stream_with(
            &Scripted::failing("the endpoint answered 500"),
            dir.dir(),
            ChatRequest::default(),
            |_| ControlFlow::Continue(()),
        )
        .expect_err("the stream failed");
        assert_eq!(error, "the endpoint answered 500");
    }

    #[test]
    fn the_events_serialize_as_the_frontend_reads_them() {
        // The wire shape a listener switches on: one `kind`, snake_case tags.
        assert_eq!(
            serde_json::to_value(StreamEvent::TextDelta { text: "hi".into() }).unwrap(),
            json!({"kind": "text_delta", "text": "hi"})
        );
        assert_eq!(
            serde_json::to_value(StreamEvent::ToolCallDelta {
                index: 2,
                id: Some("call_1".into()),
                name: Some("query".into()),
                arguments: "{\"query\":".into(),
            })
            .unwrap(),
            json!({
                "kind": "tool_call_delta",
                "index": 2,
                "id": "call_1",
                "name": "query",
                "arguments": "{\"query\":"
            })
        );
        assert_eq!(
            serde_json::to_value(StreamEvent::Usage {
                prompt_tokens: 1,
                completion_tokens: 2,
                total_tokens: 3,
            })
            .unwrap(),
            json!({"kind": "usage", "prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3})
        );
        assert_eq!(
            serde_json::to_value(StreamEvent::Done {
                finish_reason: Some("stop".into()),
            })
            .unwrap(),
            json!({"kind": "done", "finish_reason": "stop"})
        );
    }
}
