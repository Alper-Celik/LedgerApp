// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! History commands: the operation log, its undo, and the resolution of a sync
//! client's conflicted copies.

use std::path::PathBuf;

use tauri::State;

use crate::editing::edit_named_file_source;
use crate::session::AppState;

use super::{ledger_dir, ledger_path};

/// Where the operation log lives: the ledger's own hidden directory, so the
/// lines and images travel with the ledger when it syncs through a file sync.
fn history_dir(state: &AppState) -> Result<PathBuf, String> {
    Ok(ledger_dir(state)?
        .join(crate::conversations::DIRECTORY)
        .join("log"))
}

/// One page of the operation log, newest first.
#[tauri::command]
#[specta::specta]
pub fn history(
    origin: Option<String>,
    path: Option<String>,
    // `u32`, not `usize`: specta refuses to export BigInt-style types (a JS
    // number cannot carry them), and the log's page size is nowhere near 4G.
    limit: u32,
    state: State<'_, AppState>,
) -> Result<Vec<crate::history::Operation>, String> {
    let origin = origin.as_deref().map(|name| match name {
        "ai" => crate::history::Origin::Ai,
        "importer" => crate::history::Origin::Importer,
        _ => crate::history::Origin::User,
    });
    crate::history::list(
        &history_dir(&state)?,
        crate::history::Filter {
            origin,
            path: path.map(PathBuf::from),
            limit: limit as usize,
        },
    )
}

/// Undo one operation by writing its pre-images back through the loader-verified
/// pipeline. Refuses when a file changed since, and refuses to undo an operation
/// that created files (nothing to restore — deleting them is the caller's call).
#[tauri::command]
#[specta::specta]
pub fn history_undo(id: String, state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let root = history_dir(&state)?;
    let plan = crate::history::revert_plan(&root, &id)?;
    // Validate the whole plan first: undoing half an operation and then refusing
    // the rest would leave the ledger in a state the log cannot describe.
    let mut writes = Vec::with_capacity(plan.len());
    for entry in plan {
        let Some(bytes) = entry.before else {
            return Err(format!(
                "{} did not exist before operation {id}; undo would have to delete it",
                entry.path.display()
            ));
        };
        let text = String::from_utf8(bytes)
            .map_err(|_| format!("{} is not UTF-8 text", entry.path.display()))?;
        writes.push((entry.path.display().to_string(), text));
    }
    let mut restored = Vec::with_capacity(writes.len());
    for (path, text) in writes {
        edit_named_file_source(&state, &path, &text, None)?;
        restored.push(path);
    }
    Ok(restored)
}

/// Compare a sync client's conflicted copy with its original: a directive-level
/// merge plan whose base comes from the operation log. Must be called before
/// [`conflict_resolve`] — the copy's bytes are what the resolve proves against.
#[tauri::command]
#[specta::specta]
pub fn conflict_compare(
    original: String,
    copy: String,
    state: State<'_, AppState>,
) -> Result<crate::conflict_cmd::ConflictView, String> {
    // Both paths come from the webview, and `resolve` can delete the copy: they
    // are confined to the ledger's own directory like every other file argument.
    let original = ledger_path(&state, &original)?;
    let copy = ledger_path(&state, &copy)?;
    crate::conflict_cmd::compare(&ledger_dir(&state)?, &original, &copy)
}

/// Apply the user's picks to the compared original, through the loader-verified
/// write path, and retire the conflict copy when it is unchanged.
#[tauri::command]
#[specta::specta]
pub fn conflict_resolve(
    original: String,
    picks: Vec<(String, String)>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let original = ledger_path(&state, &original)?;
    crate::conflict_cmd::resolve(&state, &original, &picks)
}
