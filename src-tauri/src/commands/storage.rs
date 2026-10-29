// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Storage commands: the Android SAF tree behind its private mirror, the folder
//! picker and its one-pick-at-a-time guard.

use std::path::{Path, PathBuf};

use tauri::State;

#[cfg(target_os = "android")]
use tauri::Manager;

use crate::AppRuntime;
use crate::session::AppState;

#[cfg(target_os = "android")]
use super::app_config_dir;

/// One folder is picked at a time.
///
/// Copying a tree takes seconds, and the picker closes when the URI is returned —
/// before the copy is done. A second tap would then mirror a second folder into
/// the same directory: each call's file creation races the other's, and the
/// survivor is a mixture whose write-back copies one ledger's journal into the
/// other folder.
static PICKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Held for the duration of a pick; dropped even on the error path.
struct PickGuard;

impl Drop for PickGuard {
    fn drop(&mut self) {
        PICKING.store(false, std::sync::atomic::Ordering::Release);
    }
}

fn begin_pick() -> Result<PickGuard, String> {
    if PICKING.swap(true, std::sync::atomic::Ordering::Acquire) {
        return Err("a folder is already being opened".to_string());
    }
    Ok(PickGuard)
}

/// Where the chooser opens: the open ledger's own directory, or nothing (the
/// chooser's default) when no ledger is loaded yet.
fn chooser_start(state: &AppState) -> Option<PathBuf> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    guard
        .as_ref()
        .and_then(|loaded| loaded.root.parent().map(Path::to_path_buf))
}

/// The folder the desktop's own chooser returned, or `None` when the user
/// cancelled.
///
/// One command, one Tauri-level chooser behind it — Tauri's dialog plugin, on
/// every desktop target, so the frontend and this signature never learn which
/// platform answered:
///
/// * Linux: the plugin's xdg-desktop-portal backend. Its default GTK backend is
///   the one that segfaults in this process (rfd builds the dialog on Qt's GLib
///   main context after initialising GTK on its own thread — see `folders.rs`),
///   and the portal is a D-Bus service, so nothing toolkit-shaped is loaded.
/// * Windows / macOS: the plugin's native backend.
/// * Phones: the plugin has no folder picker there, so this answers with an
///   error and the frontend falls back to the in-app picker. Android's storage
///   framework is the eventual answer — a tree URI read through a
///   `ContentResolver`, not a filesystem path.
#[tauri::command]
#[specta::specta]
pub async fn pick_ledger_folder(
    app: tauri::AppHandle<AppRuntime>,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    // Read before any `.await`: the lock must not be held across one.
    let start = chooser_start(&state);
    let _picking = begin_pick()?;

    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    {
        use tauri_plugin_dialog::DialogExt;

        // The chooser is modal and blocks its thread — and `blocking_pick_folder`
        // must not run on the main thread — so it gets a worker of its own.
        let chosen = tauri::async_runtime::spawn_blocking(move || {
            let mut dialog = app.dialog().file().set_title("Open ledger folder");
            if let Some(dir) = start {
                dialog = dialog.set_directory(dir);
            }
            dialog.blocking_pick_folder()
        })
        .await
        .map_err(|error| error.to_string())?;

        let picked = chosen
            .and_then(|folder| folder.into_path().ok())
            .map(|folder| folder.to_string_lossy().into_owned());
        if picked.is_some() {
            // Picking a folder is the way out of the last folder failure: what
            // the app was pointed at is no longer that folder.
            state.clear_storage_failure();
        }
        Ok(picked)
    }

    #[cfg(target_os = "android")]
    {
        // A phone has no folder dialog: a directory is a SAF tree behind a
        // content URI. Pick it, mirror it into app-private files (what the
        // loader, git and the editor can open), persist the grant, and hand the
        // mirror back as if it were a folder.
        let _ = start;
        let handle = app.clone();
        let picked = tauri::async_runtime::spawn_blocking(move || {
            crate::storage::AndroidTree::pick(&handle)
        })
        .await
        .map_err(|error| error.to_string())??;
        let Some(tree) = picked else {
            return Ok(None);
        };
        let config = app_config_dir(&app)?;
        let mirror = config.join("tree");
        // A tree that was open still owes the folder whatever the app wrote into
        // its mirror; pushing is best effort (the reader is leaving it) but it
        // must not be skipped silently, since the mirror is about to be dropped.
        if let Ok(previous) = std::fs::read_to_string(config.join("storage.json"))
            .map_err(|error| error.to_string())
            .and_then(|json| crate::storage::AndroidTree::from_json(&app, json.trim()))
        {
            // The mirror is about to be replaced, so anything the folder does not
            // have yet would be lost with it: a file both sides changed exists
            // only here, and opening another folder is not consent to drop it.
            let report = match crate::storage::sync_back(&previous, &mirror) {
                Ok(report) => report,
                Err(error) => {
                    // This one does stop the pick — the mirror must not be
                    // dropped with bytes only it holds — but the sentence is
                    // kept as well, so it outlives the transient error the
                    // picker will be showing.
                    let message = format!(
                        "The folder that was open did not take the app's last writes: {error}. Pick it again with “Open folder…” to retry, before opening another one."
                    );
                    state.note_storage_failure(message.as_str());
                    return Err(message);
                }
            };
            if !report.conflicts.is_empty() {
                return Err(format!(
                    "{} file(s) in the previous folder changed on both sides; settle them before opening another folder",
                    report.conflicts.len()
                ));
            }
        }
        crate::storage::adopt_tree(&tree, &mirror)?;
        let settings = config.join("storage.json");
        // The URI lands before the mode flips: a crash between the two leaves a
        // tree that the next launch can restore, not a mirror nothing owns.
        std::fs::write(&settings, tree.to_json()?)
            .map_err(|error| format!("{}: {error}", settings.display()))?;
        crate::storage::set_tree_active(true);
        // The folder the app is pointed at is one the reader just picked, so a
        // restore failure from an earlier launch no longer applies.
        state.clear_storage_failure();
        Ok(Some(mirror.display().to_string()))
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android"
    )))]
    {
        let _ = (&app, start);
        Err("this platform has no folder chooser yet".to_owned())
    }
}

/// Which storage the ledger lives in: a directory this process reads directly,
/// or an Android SAF tree behind a private mirror. The sidebar reads this to
/// decide whether the git views can exist at all.
#[tauri::command]
#[specta::specta]
pub fn storage_kind() -> crate::storage::StorageKind {
    crate::storage::kind()
}

/// What last went wrong with the ledger's folder rather than with the ledger, or
/// `None` when the last write to it worked.
///
/// The folder has four silent failure paths — the phone's restore, before any
/// window exists; the assistant's flush of a conversation; the history's note of
/// files a repository operation rewrote; and a folder being left for another one
/// — and none of them has a command to fail, because the operation they follow
/// already happened. They all record their sentence here, so the UI can show one
/// alert with the picker beside it. A restore that fails is the worst case: the
/// app opens with no ledger and this is its only account of why.
#[tauri::command]
#[specta::specta]
pub fn storage_error(state: State<'_, AppState>) -> Option<String> {
    state.storage_failure()
}

/// Where a SAF tree is mirrored: app-private data, per install. The mirror is
/// what the loader, the editor and git see; the tree is only the source and the
/// sink.
#[cfg(target_os = "android")]
fn tree_mirror(app: &tauri::AppHandle<AppRuntime>) -> Result<PathBuf, String> {
    Ok(app_config_dir(app)?.join("tree"))
}

/// The open SAF tree and its private mirror, for a closure.
///
/// Every tree command needs the same three things — the persisted grant, the tree
/// it names, and the mirror beside it — and each of them can fail. Resolving them
/// once here keeps the three commands (and the conversation flush) from drifting
/// apart on what "tree mode" and "which mirror" mean.
#[cfg(target_os = "android")]
fn with_tree<T>(
    app: &tauri::AppHandle<AppRuntime>,
    work: impl FnOnce(&crate::storage::AndroidTree, &Path) -> Result<T, String>,
) -> Result<T, String> {
    let config = app_config_dir(app)?;
    let settings = config.join("storage.json");
    let json = std::fs::read_to_string(&settings)
        .map_err(|error| format!("{}: {error}", settings.display()))?;
    let tree = crate::storage::AndroidTree::from_json(app, json.trim())?;
    work(&tree, &config.join("tree"))
}

/// Android: write the mirror's changed files back into the SAF tree. Called after
/// a successful edit (and by the UI's explicit sync action); reports what moved
/// and what could not.
#[tauri::command]
#[specta::specta]
pub async fn sync_ledger_tree(
    app: tauri::AppHandle<AppRuntime>,
) -> Result<crate::storage::SyncReport, String> {
    #[cfg(target_os = "android")]
    {
        // Only a ledger that came from a tree has anything to write back; the
        // check is here as well as in the UI, so no caller can push the previous
        // tree's mirror behind a local ledger.
        if crate::storage::kind() != crate::storage::StorageKind::Tree {
            return Ok(crate::storage::SyncReport::default());
        }
        let handle = app.clone();
        let report = tauri::async_runtime::spawn_blocking(move || {
            with_tree(&handle, |tree, mirror| {
                crate::storage::sync_back(tree, mirror)
            })
        })
        .await
        .map_err(|error| error.to_string())??;
        // The folder took the write-back, which is the evidence every earlier
        // folder failure was missing: stop reporting it.
        app.state::<AppState>().clear_storage_failure();
        Ok(report)
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("a SAF tree exists only on Android".to_owned())
    }
}

/// Android: pull the folder's versions into the mirror before the ledger is read
/// again — what "Reload ledger" has to mean when the ledger is behind a tree.
#[tauri::command]
#[specta::specta]
pub async fn reload_ledger_tree(
    app: tauri::AppHandle<AppRuntime>,
) -> Result<crate::storage::SyncReport, String> {
    #[cfg(target_os = "android")]
    {
        if crate::storage::kind() != crate::storage::StorageKind::Tree {
            return Ok(crate::storage::SyncReport::default());
        }
        let handle = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            with_tree(&handle, |tree, mirror| {
                crate::storage::mirror_into(tree, mirror)
            })
        })
        .await
        .map_err(|error| error.to_string())?
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(crate::storage::SyncReport::default())
    }
}

/// Android: settle every file the folder and the app both changed on one side.
#[tauri::command]
#[specta::specta]
pub async fn resolve_tree_conflicts(
    take: crate::storage::Resolve,
    app: tauri::AppHandle<AppRuntime>,
) -> Result<crate::storage::SyncReport, String> {
    #[cfg(target_os = "android")]
    {
        if crate::storage::kind() != crate::storage::StorageKind::Tree {
            return Ok(crate::storage::SyncReport::default());
        }
        let handle = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            with_tree(&handle, |tree, mirror| {
                crate::storage::resolve_conflicts(tree, mirror, take)
            })
        })
        .await
        .map_err(|error| error.to_string())?
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = (take, app);
        Ok(crate::storage::SyncReport::default())
    }
}

/// Android: reopen the tree the app was last pointed at, before the window
/// shows. A phone cannot resolve a folder by path, so the URI the picker stored
/// is the only way back to the ledger; the mirror is rebuilt from it and the
/// ledger loaded from the mirror.
///
/// Runs from `setup` and never fails the launch: a revoked grant, a deleted
/// folder or a tree without a journal leaves the app open with whatever the
/// local fallback found, which is what `pick_ledger_folder` can then fix. What
/// went wrong is recorded on [`AppState`] as well, because a phone can end up
/// with no ledger at all and a console line nobody sees: [`storage_error`] hands
/// the sentence to the UI, which offers the picker beside it.
#[cfg(target_os = "android")]
pub fn restore_tree(app: &tauri::AppHandle<AppRuntime>) {
    let Ok(config) = app_config_dir(app) else {
        return;
    };
    let Ok(json) = std::fs::read_to_string(config.join("storage.json")) else {
        // No tree was ever picked: nothing failed, so there is nothing to report.
        return;
    };
    let tree = match crate::storage::AndroidTree::from_json(app, json.trim()) {
        Ok(tree) => tree,
        // A grant this build cannot read is a folder this launch cannot reopen.
        Err(error) => {
            note_restore_failure(app, &error);
            return;
        }
    };
    let mirror = config.join("tree");
    // `mirror_into` reconciles: files this device wrote and did not push are
    // pushed first, files the folder gained or changed are pulled, and a file
    // both sides changed is kept in the mirror and left alone. Nothing is
    // deleted, so a revoked grant or a transient provider error costs the new
    // bytes, not the ones already here.
    match crate::storage::mirror_into(&tree, &mirror) {
        Ok(report) if !report.conflicts.is_empty() => eprintln!(
            "tree restore: {} file(s) changed on both sides and were not written",
            report.conflicts.len()
        ),
        Ok(_) => {}
        Err(error) => {
            note_restore_failure(app, &error);
            return;
        }
    }
    crate::storage::set_tree_active(true);
    // The journal inside the mirror, resolved the same way a picked folder is.
    let state = app.state::<AppState>();
    match crate::session::load_baselined(&mirror) {
        Ok(loaded) => {
            *state.loaded.write().unwrap_or_else(|e| e.into_inner()) = Some(loaded);
            // The folder is open again: whatever an earlier launch could not
            // reopen is no longer true, and the UI must stop saying it.
            state.clear_storage_failure();
        }
        Err(error) => note_restore_failure(app, &format!("{}: {error}", mirror.display())),
    }
}

/// Record that this launch could not reopen the folder, both for the console —
/// the only trace before a window exists — and for the UI, which reads it over
/// IPC (`storage_error`) and offers the picker as the way out.
#[cfg(target_os = "android")]
fn note_restore_failure(app: &tauri::AppHandle<AppRuntime>, error: &str) {
    eprintln!("tree restore: {error}");
    app.state::<AppState>()
        .note_storage_failure(crate::session::reopen_failure(error));
}

/// Android: push what the mirror holds into the SAF tree, without a report.
///
/// Conversations, chat transcripts and the log live inside the ledger folder, so
/// they reach the folder through the same mirror as an edit. Nothing on those
/// paths bumps the ledger revision, which is why the write-back cannot be left
/// to the UI's effect: a conversation the app never flushed is deleted by the
/// next restore's reconciliation as if the folder had deleted it.
pub(super) fn sync_tree_now(app: &tauri::AppHandle<AppRuntime>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        if crate::storage::kind() != crate::storage::StorageKind::Tree {
            return Ok(());
        }
        with_tree(app, |tree, mirror| {
            crate::storage::sync_back(tree, mirror).map(|_| ())
        })?;
        // The folder just took what the app had for it, which is the evidence
        // every earlier folder failure was missing: stop reporting it.
        app.state::<AppState>().clear_storage_failure();
        Ok(())
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(())
    }
}
