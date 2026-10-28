// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Tauri command surface: thin wrappers that hand the frontend's payloads
//! to the domain modules. These names and their signatures are what specta
//! exports to `src/bindings.ts`; the logic lives next door so it can be driven
//! without a running app.

use std::path::{Path, PathBuf};

use tauri::State;

use crate::accounts::{account_balances, account_transaction_views, modified_accounts};
use crate::editing::{
    edit_file_source, edit_transaction, edit_transaction_source, transaction_file_view,
    transaction_source_text,
};
use crate::folders;
use crate::git::{commit_staged, ledger_diff_text};
use crate::model::{
    AccountBalance, FolderListing, LoadSummary, TransactionEdit, TransactionFile, TransactionView,
};
use crate::session::{AppState, load_baselined, summary_of};

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
#[specta::specta]
pub fn greet(name: String) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// One entry per account that appears in the tree — accounts with postings
/// and their ancestors — each carrying its full subtree balance.
#[tauri::command]
#[specta::specta]
pub fn accounts(state: State<'_, AppState>) -> Vec<AccountBalance> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    guard
        .as_ref()
        .map(|loaded| {
            let modified = modified_accounts(&loaded.ledger, &loaded.baseline);
            account_balances(&loaded.ledger, &modified)
        })
        .unwrap_or_default()
}

/// Transactions touching `account` — the account itself or any sub-account —
/// newest first, each carrying all of its postings so the UI can show the whole
/// entry even when only one leg hits this account. An empty `account` is the
/// virtual root: the whole ledger.
#[tauri::command]
#[specta::specta]
pub fn account_transactions(account: String, state: State<'_, AppState>) -> Vec<TransactionView> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    guard
        .as_ref()
        .map(|loaded| account_transaction_views(&loaded.ledger, &loaded.baseline, &account))
        .unwrap_or_default()
}

/// Parse, book, and validate a beancount journal, then keep it as app state.
/// `path` may be the journal itself or a directory holding one (`main.bean` and
/// friends, found by the loader). A failed load leaves the previously loaded
/// ledger untouched.
#[tauri::command]
#[specta::specta]
pub fn load_ledger(path: String, state: State<'_, AppState>) -> Result<LoadSummary, String> {
    let loaded = load_baselined(Path::new(&path))?;
    let summary = summary_of(&loaded);
    // A poisoned lock means an earlier command panicked while holding the ledger;
    // its contents are still consistent, so recover rather than propagating it as
    // a permanent failure to every later command.
    *state.loaded.write().unwrap_or_else(|e| e.into_inner()) = Some(loaded);
    Ok(summary)
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

/// The folders inside `path` — or inside the user's home when `None` — for the
/// in-app folder picker: each one's absolute path, and whether a root journal
/// sits in it.
///
/// There is deliberately no native dialog behind this: the Qt runtime drops
/// `run_on_main_thread` closures (so tauri-plugin-dialog never opens), and a
/// GTK dialog in this process segfaults because rfd initialises GTK on one
/// thread while the idle that builds the widget is dispatched on Qt's. Listing
/// directories needs neither a toolkit nor a portal.
#[tauri::command]
#[specta::specta]
pub fn list_folders(path: Option<String>) -> Result<FolderListing, String> {
    let path = path.map(PathBuf::from).unwrap_or_else(folders::home);
    folders::list_folders(&path)
}

/// Rewrite one transaction's own lines in the `.bean` file it came from, reload
/// the ledger from disk, and stage the file.
///
/// Only the lines that changed are rewritten — the rest of the file stays
/// byte-identical. The reload is the authority on whether the edit was legal (a
/// balanced transaction, a parseable header): when it raises an error the file
/// is put back exactly as it was and the loader's complaint is returned, so an
/// edit can never leave a broken ledger on disk. The file is staged for a
/// later commit; a ledger outside a git repository is still written, there is
/// simply nothing to stage.
#[tauri::command]
#[specta::specta]
pub fn update_transaction(edit: TransactionEdit, state: State<'_, AppState>) -> Result<(), String> {
    edit_transaction(&state, edit)
}

/// The transaction's exact source text (its own lines, as they appear in the
/// file): the header from its date token on, every posting line, and the
/// newline that ends the last of them, with their original indentation. This
/// is the byte range [`update_transaction_source`] rewrites.
#[tauri::command]
#[specta::specta]
pub fn transaction_source(id: u32, state: State<'_, AppState>) -> Result<String, String> {
    transaction_source_text(&state, id)
}

/// Replace a transaction's source text with `text` — the source editor's way in
/// — through the same pipeline as [`update_transaction`]: reload the ledger and
/// refuse the edit (putting the file back byte-for-byte) if the reload fails or
/// raises an error the ledger did not already have, and stage the file on
/// success.
#[tauri::command]
#[specta::specta]
pub fn update_transaction_source(
    id: u32,
    text: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    edit_transaction_source(&state, id, &text)
}

/// The whole ledger file a transaction lives in, together with the entry's own
/// position inside it — the file editor's way in. The entire `.bean` is handed
/// over (a transaction may share its file with everything else), and the entry
/// is where the editor opens.
#[tauri::command]
#[specta::specta]
pub fn transaction_file(id: u32, state: State<'_, AppState>) -> Result<TransactionFile, String> {
    transaction_file_view(&state, id)
}

/// Replace a ledger file's entire source text with `text` — the file editor's
/// way out — through the same pipeline as [`update_transaction_source`]: the
/// loader, not this function, is the authority on whether arbitrary text is a
/// legal ledger, a refusal puts the file back byte-for-byte, and a success
/// stages it for a later commit.
#[tauri::command]
#[specta::specta]
pub fn update_file_source(id: u32, text: String, state: State<'_, AppState>) -> Result<(), String> {
    edit_file_source(&state, id, &text)
}

/// Unified diff of the ledger's files against HEAD, index and working tree
/// together, for the diff view: git's own patch text under a `=== path ===`
/// header per changed file.
#[tauri::command]
#[specta::specta]
pub fn ledger_diff(state: State<'_, AppState>) -> Result<String, String> {
    ledger_diff_text(&state)
}

/// Commit the ledger's staged changes; returns the new commit's short id.
#[tauri::command]
#[specta::specta]
pub fn commit_ledger(message: String, state: State<'_, AppState>) -> Result<String, String> {
    commit_staged(&state, &message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppRuntime;
    use specta_typescript::Typescript;
    use tauri_specta::{Builder, collect_commands};

    #[test]
    fn export_bindings() {
        Builder::<AppRuntime>::new()
            .commands(collect_commands![
                greet,
                accounts,
                account_transactions,
                load_ledger,
                ledger_summary,
                list_folders,
                update_transaction,
                transaction_source,
                update_transaction_source,
                transaction_file,
                update_file_source,
                ledger_diff,
                commit_ledger,
            ])
            .export(Typescript::default(), "../src/bindings.ts")
            .unwrap();
    }
}
