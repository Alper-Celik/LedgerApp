// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Assistant commands: the model endpoint and its settings, streamed turns, the
//! saved conversations, and the base tool table the panel offers the model.

use std::path::PathBuf;

use tauri::{Emitter, Manager, State};

use crate::AppRuntime;
use crate::session::AppState;

use super::storage::sync_tree_now;
use super::{app_config_dir, ledger_dir};

/// Where the assistant's own settings live: the app's config dir, never the
/// ledger — the key must not be committed.
fn ai_dir(app: &tauri::AppHandle<AppRuntime>) -> Result<PathBuf, String> {
    Ok(app_config_dir(app)?.join("ai"))
}

/// The assistant's endpoint/model, and whether a key is stored (never the key).
#[tauri::command]
#[specta::specta]
pub fn ai_settings(
    app: tauri::AppHandle<AppRuntime>,
) -> Result<crate::ai_cmd::AiSettingsView, String> {
    crate::ai_cmd::settings(&ai_dir(&app)?)
}

/// Store the endpoint, model and (optionally) a new key. `None` keeps the
/// stored key, `Some("")` clears it.
#[tauri::command]
#[specta::specta]
pub fn ai_configure(
    base_url: String,
    api_key: Option<String>,
    model: String,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    app: tauri::AppHandle<AppRuntime>,
) -> Result<crate::ai_cmd::AiSettingsView, String> {
    crate::ai_cmd::configure(
        &ai_dir(&app)?,
        base_url,
        api_key,
        model,
        max_tokens,
        temperature,
    )
}

/// What the endpoint says it can run. Blocking HTTP, so a worker thread.
#[tauri::command]
#[specta::specta]
pub async fn ai_models(
    app: tauri::AppHandle<AppRuntime>,
) -> Result<Vec<crate::ai_cmd::ModelInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || crate::ai_cmd::models(&ai_dir(&app)?))
        .await
        .map_err(|error| error.to_string())?
}

/// The tool schemas the panel offers the model — the same table the backend
/// keeps, so the two cannot drift.
#[tauri::command]
#[specta::specta]
pub fn ai_tools() -> Vec<crate::ai_cmd::ToolSchema> {
    crate::ai_cmd::tools()
}

/// One completion turn. Blocking (a provider may take a while), so a worker
/// thread; tool calls come back for the panel to execute and answer.
#[tauri::command]
#[specta::specta]
pub async fn ai_chat(
    request: crate::ai_cmd::ChatRequest,
    app: tauri::AppHandle<AppRuntime>,
) -> Result<crate::ai_cmd::ChatResponse, String> {
    tauri::async_runtime::spawn_blocking(move || crate::ai_cmd::chat(&ai_dir(&app)?, request))
        .await
        .map_err(|error| error.to_string())?
}

/// One streamed completion turn. Deltas go out as `ai://delta` events while the
/// request is in flight; the resolved value is the assembled answer, so a caller
/// that missed events still gets the text and the tool calls.
#[tauri::command]
#[specta::specta]
pub async fn ai_stream(
    request: crate::ai_cmd::ChatRequest,
    app: tauri::AppHandle<AppRuntime>,
) -> Result<crate::ai_cmd::ChatResponse, String> {
    let dir = ai_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let emitter = app.clone();
        crate::ai_stream::chat_stream(&dir, request, move |event| {
            // A listener that cannot be reached is the panel's problem, not the
            // request's: keep reading the stream and let the return value carry
            // the answer.
            let _ = emitter.emit("ai://delta", event);
            std::ops::ControlFlow::Continue(())
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

/// The ledger's saved conversations, newest first — they live inside the ledger
/// so they sync with it.
#[tauri::command]
#[specta::specta]
pub fn conversations(state: State<'_, AppState>) -> Result<crate::conversations::Listing, String> {
    crate::conversations::list(&ledger_dir(&state)?)
}

/// One conversation with its messages.
#[tauri::command]
#[specta::specta]
pub fn conversation(
    id: String,
    state: State<'_, AppState>,
) -> Result<crate::conversations::Conversation, String> {
    crate::conversations::load(&ledger_dir(&state)?, &id)
}

/// Copy what the assistant just wrote into the ledger folder, and report — never
/// fail the command — when it did not get there: the conversation is already
/// saved, so the reader needs to know the folder is behind, not to see the
/// append answered as failed. The failure goes on `AppState` so Settings can
/// show it after the fact, since nothing here has a return value to carry it.
async fn flush_folder(app: tauri::AppHandle<AppRuntime>, what: &str) {
    let handle = app.clone();
    // The flush walks the tree through the SAF provider's blocking calls, so it
    // runs off this thread.
    let outcome = tauri::async_runtime::spawn_blocking(move || sync_tree_now(&handle))
        .await
        .map_err(|error| error.to_string())
        .and_then(|flushed| flushed);
    if let Err(error) = outcome {
        eprintln!("{what} not copied to the folder: {error}");
        app.state::<AppState>().note_storage_failure(format!(
            "{what} did not reach the ledger folder: {error}. Pick the folder again with “Open folder…” if this keeps happening."
        ));
    }
}

/// Append one message to a conversation, creating it when `id` is empty.
#[tauri::command]
#[specta::specta]
pub async fn conversation_append(
    id: String,
    title: String,
    model: String,
    message: crate::conversations::Message,
    app: tauri::AppHandle<AppRuntime>,
    state: State<'_, AppState>,
) -> Result<crate::conversations::Summary, String> {
    let dir = ledger_dir(&state)?;
    // `create` answers with the summary it wrote; deriving the id from a listing
    // would pick the wrong conversation when two share a second.
    let id = if id.is_empty() {
        crate::conversations::create(&dir, &crate::conversations::new_id(), &title, &model, &[])?.id
    } else {
        id
    };
    let summary = crate::conversations::append(&dir, &id, &message)?;
    // The message is written. A folder this device cannot reach is reported, not
    // turned into a failed append — the panel would answer that by appending to
    // a conversation it thinks does not exist yet.
    flush_folder(app, "The conversation's new message").await;
    Ok(summary)
}

/// Rename a conversation (the file name stays: it is a creation record).
#[tauri::command]
#[specta::specta]
pub async fn conversation_rename(
    id: String,
    title: String,
    app: tauri::AppHandle<AppRuntime>,
    state: State<'_, AppState>,
) -> Result<crate::conversations::Summary, String> {
    let summary = crate::conversations::rename(&ledger_dir(&state)?, &id, &title)?;
    flush_folder(app, "The conversation's new name").await;
    Ok(summary)
}

/// Delete a conversation's file; its history is the repository's.
#[tauri::command]
#[specta::specta]
pub async fn conversation_delete(
    id: String,
    app: tauri::AppHandle<AppRuntime>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    crate::conversations::delete(&ledger_dir(&state)?, &id)?;
    flush_folder(app, "The conversation's deletion").await;
    Ok(())
}

/// The assistant's base tools (read/edit/grep/glob) as OpenAI-style schemas,
/// serialised: a `serde_json::Value` cannot cross specta (it has no static type
/// on the TypeScript side), and the panel hands each schema straight back to the
/// model anyway.
#[tauri::command]
#[specta::specta]
pub fn tool_schemas() -> Result<String, String> {
    let tools: Vec<serde_json::Value> = crate::tools::registry()
        .iter()
        .map(|definition| {
            serde_json::json!({
                "name": definition.name,
                "description": definition.description,
                "parameters": definition.schema,
                "read_only": definition.read_only,
            })
        })
        .collect();
    serde_json::to_string(&tools).map_err(|error| error.to_string())
}

/// Run one base tool against the open ledger's directory. `edit` is hash-anchored
/// and atomic: one stale anchor refuses the whole batch.
#[tauri::command]
#[specta::specta]
pub fn tool_invoke(
    name: String,
    args: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let root = ledger_dir(&state)?;
    // `args` (a JSON object as text): `arguments` is a reserved word in the
    // strict-mode module specta emits, so it cannot be a parameter name there.
    let arguments: serde_json::Value =
        serde_json::from_str(&args).map_err(|error| format!("arguments: {error}"))?;
    let result =
        crate::tools::invoke(&root, &name, &arguments).map_err(|error| error.to_string())?;
    serde_json::to_string(&result).map_err(|error| error.to_string())
}
