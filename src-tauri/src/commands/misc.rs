// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The remaining commands: folder listing for the in-app picker, the embedded
//! language server, and attachments.

use std::path::PathBuf;

use tauri::{Emitter, State};

use crate::AppRuntime;
use crate::folders;
use crate::lsp;
use crate::model::FolderListing;
use crate::session::AppState;

use super::ledger_path;

/// The folders inside `path` — or inside the user's home when `None` — for the
/// in-app folder picker: each one's absolute path, and whether a root journal
/// sits in it.
///
/// This backs the in-app picker, the fallback for platforms where the desktop's
/// own chooser ([`pick_ledger_folder`]) has no implementation — phones today.
#[tauri::command]
#[specta::specta]
pub fn list_folders(path: Option<String>) -> Result<FolderListing, String> {
    let path = path.map(PathBuf::from).unwrap_or_else(folders::home);
    folders::list_folders(&path)
}

/// Start the embedded language server for the loaded journal, if it is not
/// already running for that journal.
///
/// The result says whether the caller still has to send `initialize`: a new
/// server is waiting for it, while one an earlier editor already initialized
/// refuses a second one. Messages come back as `lsp-message` events, one
/// JSON-RPC object each; the editor talks to the server from then on through
/// [`lsp_send`]. The server loads the journal itself, from disk, so the account
/// and currency completions cover every included file and not only the open
/// buffer.
#[tauri::command]
#[specta::specta]
pub fn lsp_start(
    app: tauri::AppHandle<AppRuntime>,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let journal = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().ok_or("no ledger loaded")?;
        std::path::absolute(&loaded.root)
            .map_err(|e| format!("cannot resolve {}: {e}", loaded.root.display()))?
    };
    lsp::start(journal, move |message| {
        let _ = app.emit("lsp-message", message);
    })
}

/// One JSON-RPC message to the language server, as [`lsp_start`] left it.
#[tauri::command]
#[specta::specta]
pub fn lsp_send(message: String) -> Result<(), String> {
    lsp::send(&message)
}

/// The absolute path of a journal file, the form a `file:` URI needs.
///
/// The loader keeps the paths it read — the root journal's spelling and its
/// `include`s — which may be relative to the process, while the language
/// server's URIs are absolute. This is the one conversion those two need.
#[tauri::command]
#[specta::specta]
pub fn lsp_file_path(file: String) -> Result<String, String> {
    std::path::absolute(&file)
        .map(|path| path.display().to_string())
        .map_err(|e| format!("cannot resolve {file}: {e}"))
}

/// What kind of file an attachment is, how big, and (for PDFs/spreadsheets) how
/// many pages or sheets it has.
#[tauri::command]
#[specta::specta]
pub async fn attachment_describe(
    path: String,
    state: State<'_, AppState>,
) -> Result<crate::attachments::AttachmentInfo, String> {
    // `describe` opens the PDF or workbook to count pages and sheets — the same
    // blocking parse the page and table readers run off the GUI thread.
    let file = ledger_path(&state, &path)?;
    tauri::async_runtime::spawn_blocking(move || crate::attachments::describe(&file))
        .await
        .map_err(|error| format!("describing the attachment: {error}"))?
}

/// Rasterise a PDF's pages — what a vision model is shown, instead of extracted
/// text. Capped by page count, dpi and a total pixel budget.
#[tauri::command]
#[specta::specta]
pub async fn attachment_pdf_pages(
    path: String,
    dpi: u32,
    max_pages: u32,
    state: State<'_, AppState>,
) -> Result<Vec<crate::attachments::PdfPage>, String> {
    // Rasterising twenty pages takes seconds, and a command with no `async` runs
    // inline on the GUI thread — the window would stop repainting while the
    // pages render. Read the path first: the lock must not be held across the
    // hop to the blocking pool.
    let file = ledger_path(&state, &path)?;
    let mut options = crate::attachments::RenderOptions::default();
    options.dpi = dpi;
    options.max_pages = max_pages;
    tauri::async_runtime::spawn_blocking(move || crate::attachments::render_pdf(&file, options))
        .await
        .map_err(|error| format!("rendering the pages: {error}"))?
}

/// A spreadsheets's (or CSV's) rows as strings, headers first.
#[tauri::command]
#[specta::specta]
pub async fn attachment_table(
    path: String,
    sheet: Option<String>,
    state: State<'_, AppState>,
) -> Result<crate::attachments::Table, String> {
    // Same rule as the pages: a workbook is parsed off the GUI thread.
    let file = ledger_path(&state, &path)?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::attachments::read_table(
            &file,
            sheet.as_deref(),
            crate::attachments::TableOptions::default(),
        )
    })
    .await
    .map_err(|error| format!("reading the sheet: {error}"))?
}

/// Text files an attachment may be (markdown, JSON, CSV), size-capped.
#[tauri::command]
#[specta::specta]
pub async fn attachment_text(
    path: String,
    max_bytes: u32,
    state: State<'_, AppState>,
) -> Result<String, String> {
    // Up to `max_bytes` of text read and decoded: a file, not a field.
    let file = ledger_path(&state, &path)?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::attachments::read_text(&file, u64::from(max_bytes))
    })
    .await
    .map_err(|error| format!("reading the attachment: {error}"))?
}
