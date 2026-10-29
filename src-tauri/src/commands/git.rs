// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Git commands: the repository's state, branches, staging, merges and the ledger
//! diff, plus the sync-client conflict scan.

use std::path::{Path, PathBuf};

use tauri::State;

use crate::editing::edit_named_file_source;
use crate::git::{commit_staged, ledger_diff_text};
use crate::repo;
use crate::session::AppState;

use super::{ledger_dir, ledger_path};

/// Is the ledger under git, and with which identity? Also `repo: false` when the
/// ledger lives in a SAF tree, because git cannot run there — the settings view
/// then offers only the history journal.
#[tauri::command]
#[specta::specta]
pub fn git_state(state: State<'_, AppState>) -> repo::RepoState {
    let dir = ledger_dir(&state);
    match dir {
        Ok(dir) => repo::state(&dir),
        // No ledger open: nothing to report, and no error worth a dialog.
        Err(_) => repo::RepoState {
            repo: false,
            root: None,
            user_name: None,
            user_email: None,
        },
    }
}

/// Initialise a repository in the ledger's directory. Idempotent, and it makes
/// no commit: what to commit is the diff view's business.
#[tauri::command]
#[specta::specta]
pub fn git_init(state: State<'_, AppState>) -> Result<(), String> {
    repo::init(&ledger_dir(&state)?)
}

/// Write `user.name`/`user.email` into the ledger repository's own config.
#[tauri::command]
#[specta::specta]
pub fn git_set_identity(
    name: String,
    email: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    repo::set_identity(&ledger_dir(&state)?, &name, &email)
}

/// The repository's branches, local and remote.
#[tauri::command]
#[specta::specta]
pub fn git_branches(state: State<'_, AppState>) -> Result<Vec<crate::git_ops::BranchInfo>, String> {
    crate::git_ops::branches(&ledger_dir(&state)?)
}

/// Create a branch, optionally switching to it.
#[tauri::command]
#[specta::specta]
pub fn git_create_branch(
    name: String,
    checkout: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    crate::git_ops::create_branch(&ledger_dir(&state)?, &name, checkout)
}

/// Switch branches; refuses while the ledger has uncommitted changes, naming them.
#[tauri::command]
#[specta::specta]
pub fn git_checkout(name: String, state: State<'_, AppState>) -> Result<(), String> {
    crate::git_ops::checkout(&ledger_dir(&state)?, &name)?;
    note_best_effort(&state);
    Ok(())
}

/// The commit log, newest first.
#[tauri::command]
#[specta::specta]
pub fn git_log(
    limit: u32,
    path: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<crate::git_ops::CommitInfo>, String> {
    crate::git_ops::log(
        &ledger_dir(&state)?,
        limit as usize,
        path.as_deref().map(Path::new),
    )
}

/// One commit's patch, in the diff view's own rendering.
#[tauri::command]
#[specta::specta]
pub fn git_commit_diff(id: String, state: State<'_, AppState>) -> Result<String, String> {
    crate::git_ops::commit_diff(&ledger_dir(&state)?, &id)
}

/// Stage paths (relative to the ledger directory or absolute).
#[tauri::command]
#[specta::specta]
pub fn git_stage(paths: Vec<String>, state: State<'_, AppState>) -> Result<(), String> {
    let root = ledger_dir(&state)?;
    let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    crate::git_ops::stage(&root, &paths)
}

/// Unstage paths, leaving the working tree alone.
#[tauri::command]
#[specta::specta]
pub fn git_unstage(paths: Vec<String>, state: State<'_, AppState>) -> Result<(), String> {
    let root = ledger_dir(&state)?;
    let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    crate::git_ops::unstage(&root, &paths)
}

/// Note what a repository operation rewrote, and never fail it over the note.
///
/// The repository has already moved when this runs: a log lock held elsewhere or
/// an unwritable log directory must not report a completed checkout, merge or
/// revert as failed — the caller would retry something that already happened.
/// What it does instead is record the miss on the state, so Settings can say the
/// log is behind rather than letting a rewrite the app made itself surface later
/// as an outside change.
fn note_best_effort(state: &State<'_, AppState>) {
    if let Err(error) = note_rewritten_files(state) {
        eprintln!("history: rewritten files not noted: {error}");
        state.note_storage_failure(format!(
            "The files a repository operation rewrote could not be noted: {error}. An edit the app made itself may show as an outside change."
        ));
    }
}

/// Tell the operation log what the working tree holds after a repository
/// operation, so its own rewrites are not reported as changes made outside the
/// app. The journal files are hashed again; git and the log stay separate
/// records, this only corrects the baseline `sync::changed` compares against.
fn note_rewritten_files(state: &State<'_, AppState>) -> Result<(), String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger is open")?;
    let root = loaded
        .root
        .parent()
        .ok_or_else(|| format!("{} has no directory", loaded.root.display()))?;
    // Every file the loader read, not just the root: a branch switch or a merge
    // rewrites whichever ones the two branches disagree on.
    // Every file the loader read, including ones the operation just deleted: a
    // branch switch can remove an include, and a path left out here keeps its old
    // hash in the state file, so `sync::changed` would report the app's own
    // deletion as one made outside it, for ever.
    let paths: Vec<PathBuf> = loaded
        .ledger
        .source_map
        .files()
        .iter()
        .map(|file| file.path.clone())
        .collect();
    crate::history::note_seen(&crate::sync::log_dir(root), &paths)
}

/// Restore paths from the index (never deletes uncommitted bytes).
#[tauri::command]
#[specta::specta]
pub fn git_discard(paths: Vec<String>, state: State<'_, AppState>) -> Result<(), String> {
    let root = ledger_dir(&state)?;
    let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    crate::git_ops::discard(&root, &paths)?;
    note_best_effort(&state);
    Ok(())
}

/// Merge a branch into the current one: fast-forward, up to date, or a list of
/// conflicted files with their index stages.
#[tauri::command]
#[specta::specta]
pub fn git_merge(
    theirs: String,
    state: State<'_, AppState>,
) -> Result<crate::git_ops::MergeOutcome, String> {
    let outcome = crate::git_ops::merge(&ledger_dir(&state)?, &theirs)?;
    note_best_effort(&state);
    Ok(outcome)
}

/// A conflicted file's three versions, from the index stages.
#[tauri::command]
#[specta::specta]
pub fn git_conflict_texts(
    path: String,
    state: State<'_, AppState>,
) -> Result<crate::git_ops::ConflictTexts, String> {
    crate::git_ops::conflict_texts(&ledger_dir(&state)?, Path::new(&path))
}

/// Write a resolved text for a conflicted file and stage it.
#[tauri::command]
#[specta::specta]
pub fn git_resolve(path: String, text: String, state: State<'_, AppState>) -> Result<(), String> {
    // The merged text is a ledger edit like any other: the loader must accept it
    // (and a refusal must put the bytes back), and only then does git stage it.
    let file = ledger_path(&state, &path)?;
    let file = file.display().to_string();
    edit_named_file_source(&state, &file, &text, None)?;
    let root = ledger_dir(&state)?;
    crate::git_ops::stage(&root, &[PathBuf::from(&file)])
}

/// Finish a merge whose conflicts are all resolved.
#[tauri::command]
#[specta::specta]
pub fn git_merge_continue(state: State<'_, AppState>) -> Result<(), String> {
    crate::git_ops::merge_continue(&ledger_dir(&state)?)?;
    note_best_effort(&state);
    Ok(())
}

/// Abandon a merge, restoring the branch and working tree.
#[tauri::command]
#[specta::specta]
pub fn git_merge_abort(state: State<'_, AppState>) -> Result<(), String> {
    crate::git_ops::merge_abort(&ledger_dir(&state)?)?;
    note_best_effort(&state);
    Ok(())
}

/// Undo one commit as a new commit (history stays append-only).
#[tauri::command]
#[specta::specta]
pub fn git_revert_commit(id: String, state: State<'_, AppState>) -> Result<String, String> {
    let commit = crate::git_ops::revert_commit(&ledger_dir(&state)?, &id)?;
    note_best_effort(&state);
    Ok(commit)
}

/// Files the ledger's folder changed behind the app (another device, a cloud
/// client): the app should offer a reload rather than keep serving what it
/// loaded. Empty when nothing moved or when the ledger is not in a synced folder.
#[tauri::command]
#[specta::specta]
pub fn sync_changes(
    state: State<'_, AppState>,
) -> Result<Vec<crate::sync::ExternalChange>, String> {
    // In a SAF tree the ledger's own paths are the private mirror, so comparing
    // it against its log can only ever say "unchanged": the folder is the other
    // side, and the reconciliation (`sync_ledger_tree`) reports what it found
    // there. Running this anyway would report the app's own writes as external
    // after a restore, on every revision bump.
    if crate::storage::kind() == crate::storage::StorageKind::Tree {
        return Ok(Vec::new());
    }
    crate::sync::changed(&crate::sync::log_dir(&ledger_dir(&state)?))
}

/// Sync-client conflict copies found in the ledger (Drive's `name (1).bean`,
/// Syncthing's `.sync-conflict-…`, and the rest) — the resolver's input.
#[tauri::command]
#[specta::specta]
pub fn sync_conflicts(state: State<'_, AppState>) -> Result<crate::conflict_names::Report, String> {
    crate::conflict_names::scan_report(&ledger_dir(&state)?)
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
