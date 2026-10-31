// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Tauri command surface: thin wrappers that hand the frontend's payloads
//! to the domain modules. These names and their signatures are what specta
//! exports to `src/bindings.ts`; the logic lives next door so it can be driven
//! without a running app.
//!
//! The groups are re-exported here, so every command keeps the path it had: `git`
//! for the repository, `storage` for the SAF tree and the folder picker,
//! `assistant` for the model and its conversations, `history` for the operation
//! log and sync conflicts, `views` for the ledger's own data, and `misc` for
//! folders, the language server and attachments.

mod assistant;
mod git;
mod history;
mod misc;
mod storage;
mod views;

pub use assistant::*;
pub use git::*;
pub use history::*;
pub use misc::*;
pub use storage::*;
pub use views::*;

use std::path::{Path, PathBuf};

use tauri::{Manager, State};

use crate::AppRuntime;
use crate::lsp;
use crate::model::LoadSummary;
use crate::session::{AppState, load_baselined, summary_of};

/// Parse, book, and validate a beancount journal, then keep it as app state.
/// `path` may be the journal itself or a directory holding one (`main.bean` and
/// friends, found by the loader). A failed load leaves the previously loaded
/// ledger untouched.
#[tauri::command]
#[specta::specta]
pub fn load_ledger(
    path: String,
    app: tauri::AppHandle<AppRuntime>,
    state: State<'_, AppState>,
) -> Result<LoadSummary, String> {
    let loaded = load_baselined(Path::new(&path))?;
    let summary = summary_of(&loaded);
    // Whether this ledger is the SAF mirror is a property of the path, not a flag
    // that stays set for the process: opening anything else afterwards must end
    // tree mode, or git would stay off and every revision would push the old
    // tree's mirror behind an unrelated ledger.
    crate::storage::set_tree_active(is_mirror_ledger(&app, &loaded.root));
    // The reader has now read what the folder holds: adopting those bytes as
    // this device's last-seen baseline is what keeps the sync bar from
    // re-reporting the very reload that showed it (and leaves a git-owned
    // ledger's log alone — see `sync::watches`).
    crate::sync::adopt_loaded(&loaded);
    // A poisoned lock means an earlier command panicked while holding the ledger;
    // its contents are still consistent, so recover rather than propagating it as
    // a permanent failure to every later command.
    *state.loaded.write().unwrap_or_else(|e| e.into_inner()) = Some(loaded);
    // A server started for the old journal has the old ledger in memory; the
    // next editor starts one for this one.
    lsp::stop();
    Ok(summary)
}

/// Whether `root` is inside the app's private SAF mirror.
fn is_mirror_ledger(app: &tauri::AppHandle<AppRuntime>, root: &Path) -> bool {
    app_config_dir(app).is_ok_and(|config| root.starts_with(tree_mirror(&config)))
}

/// The ledger currently open — its root journal and what the load found — or
/// `None` when nothing is loaded. The UI reads this to show which journal it is
/// looking at, including the one opened at startup.
#[tauri::command]
#[specta::specta]
pub fn ledger_summary(state: State<'_, AppState>) -> Option<LoadSummary> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    guard.as_ref().map(summary_of)
}

/// Where the app's own data lives (indexes, dashboards, the assistant's
/// settings): the app's config directory — per-app data, never part of a
/// ledger, so none of it can dirty a read-only journal.
pub(crate) fn app_config_dir(app: &tauri::AppHandle<AppRuntime>) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map_err(|error| format!("no config directory: {error}"))
}

/// Where an Android SAF tree is mirrored: app-private data, per install. The
/// mirror is what the loader, the editor and git see; the tree is only the
/// source and the sink. One definition, because every caller that names the
/// mirror by hand (`is_mirror_ledger`, the picker, the restore) has to agree
/// with the others on where it is or tree mode is decided by a path nobody
/// wrote to.
pub(crate) fn tree_mirror(config: &Path) -> PathBuf {
    config.join("tree")
}

/// The folder the chooser and the repository work from: the open ledger's own
/// directory, which is what `repo`/`storage` need (never the journal file).
pub(crate) fn ledger_dir(state: &AppState) -> Result<PathBuf, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger is open")?;
    loaded
        .root
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{} has no directory", loaded.root.display()))
}

/// Resolve a path the frontend named, refusing anything outside the ledger's own
/// directory: attachments are user files beside the journal, never `/etc`.
pub(crate) fn ledger_path(state: &AppState, path: &str) -> Result<PathBuf, String> {
    let root = ledger_dir(state)?;
    let candidate = Path::new(path);
    let resolved = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let canonical_root = std::fs::canonicalize(&root).map_err(|e| e.to_string())?;
    let canonical = std::fs::canonicalize(&resolved)
        .map_err(|error| format!("{}: {error}", resolved.display()))?;
    if !canonical.starts_with(&canonical_root) {
        return Err(format!(
            "{} is outside the ledger's directory; only files there can be attached",
            canonical.display()
        ));
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    #[test]
    fn export_bindings() {
        // The same registry `run()` installs: a command that reaches one and not
        // the other is a method `bindings.ts` promises and the backend refuses.
        crate::command_builder()
            .export(
                specta_typescript::Typescript::default(),
                "../src/bindings.ts",
            )
            .expect("Failed to export typescript bindings");
    }
}
