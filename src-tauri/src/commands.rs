// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Tauri command surface: thin wrappers that hand the frontend's payloads
//! to the domain modules. These names and their signatures are what specta
//! exports to `src/bindings.ts`; the logic lives next door so it can be driven
//! without a running app.

use std::path::{Path, PathBuf};

use tauri::{Emitter, Manager, State};

use crate::AppRuntime;
use crate::accounts::{account_balances, account_transaction_views, modified_accounts};
use crate::budget::{BudgetRange, budget_view};
use crate::dashboards::{self, Dashboard};
use crate::editing::{
    create_transaction, edit_file_source, edit_named_file_source, edit_transaction,
    edit_transaction_source, transaction_file_view, transaction_source_text,
};
use crate::files::{
    file_transaction_views, journal_file_views, source_of_file, suggested_transaction_file,
};
use crate::folders;
use crate::git::{commit_staged, ledger_diff_text};
use crate::lsp;
use crate::model::{
    AccountBalance, BudgetMonth, FolderListing, JournalFile, LoadSummary, NewTransaction,
    TransactionEdit, TransactionFile, TransactionView,
};
use crate::repo;
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
    app_config_dir(app)
        .map(|config| config.join("tree"))
        .is_ok_and(|mirror| root.starts_with(mirror))
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

        Ok(chosen
            .and_then(|folder| folder.into_path().ok())
            .map(|folder| folder.to_string_lossy().into_owned()))
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
            let report = crate::storage::sync_back(&previous, &mirror)
                .map_err(|error| format!("the previous folder was not written: {error}"))?;
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

/// The journal's files — the root journal first, then everything it includes —
/// for the file explorer and the new-entry form's file picker.
#[tauri::command]
#[specta::specta]
pub fn journal_files(state: State<'_, AppState>) -> Vec<JournalFile> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    guard.as_ref().map(journal_file_views).unwrap_or_default()
}

/// Every transaction one journal file writes, newest first, each carrying all of
/// its postings — the file view's transaction mode. The rows are the register's
/// own, ids included, so an entry opened from here edits and stages exactly as
/// it does from an account page.
#[tauri::command]
#[specta::specta]
pub fn file_transactions(
    file: String,
    state: State<'_, AppState>,
) -> Result<Vec<TransactionView>, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger loaded")?;
    file_transaction_views(&loaded.ledger, &loaded.baseline, &file)
}

/// A journal file's whole text, as this session loaded it — the file view's
/// text mode reads the file the ledger was opened from, not a fresh read of
/// disk, so what the editor shows and what an edit rewrites are the same copy.
#[tauri::command]
#[specta::specta]
pub fn file_text(file: String, state: State<'_, AppState>) -> Result<String, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger loaded")?;
    Ok(source_of_file(&loaded.ledger, &file)?.source.to_string())
}

/// Replace a journal file's whole text — the text mode's way out — through the
/// same pipeline as [`update_file_source`]: the loader is the authority on
/// whether the text is a legal ledger, a refusal puts the file back
/// byte-for-byte, and a success stages it for a later commit.
#[tauri::command]
#[specta::specta]
pub fn update_file_text(
    file: String,
    text: String,
    base: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    edit_named_file_source(&state, &file, &text, base.as_deref())
}

/// The journal file a new transaction should go into, given the accounts it
/// mentions — the new-entry form's default, so the form can preselect a file
/// before the user has to think about one. `None` only when no ledger is loaded.
#[tauri::command]
#[specta::specta]
pub fn suggest_transaction_file(
    accounts: Vec<String>,
    state: State<'_, AppState>,
) -> Option<String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    suggested_transaction_file(&guard.as_ref()?.ledger, &accounts)
}

/// Append a new transaction to one of the journal's files, through the same
/// pipeline as every other edit: reload the ledger and refuse the write (putting
/// the file back byte-for-byte) if the reload fails or raises an error the
/// ledger did not already have, and stage the file on success.
#[tauri::command]
#[specta::specta]
pub fn add_transaction(entry: NewTransaction, state: State<'_, AppState>) -> Result<(), String> {
    create_transaction(&state, entry)
}

/// One month of the ledger's envelope budget — the budget screen's data.
///
/// `month` is a `YYYY-MM` label, or `None` for the newest month the plugin has
/// budgeted, which is what the screen opens on. The figures are for that month,
/// in the ledger's operating currency; see [`BudgetMonth`] for what each one
/// means. Read-only on purpose: the plugin owns the allocations (its `fund`,
/// `adjust`, `zero`, `pad` and `rollover` configuration) and rewrites them on
/// every load, so an edit from here would fight the next run of it.
#[tauri::command]
#[specta::specta]
pub fn budget(month: Option<String>, state: State<'_, AppState>) -> Result<BudgetMonth, String> {
    budget_view(&state, month.as_deref())
}

/// Several months of the same envelope budget, oldest first — the multi-month
/// grid's data.
///
/// `from`/`to` are `YYYY-MM` labels; with neither, the newest budgeted month
/// back at most six months. An empty `months` list is not an error: it is what a
/// ledger without an envelope budget looks like, and the screen says so.
#[tauri::command]
#[specta::specta]
pub fn budget_range(
    from: Option<String>,
    to: Option<String>,
    state: State<'_, AppState>,
) -> Result<BudgetRange, String> {
    crate::budget::budget_range(&state, from, to)
}

/// Where dashboards live: the app's own config directory, never the ledger.
///
/// A dashboard is a view over the ledger, not ledger data: keeping it out of the
/// journal means it cannot turn a read-only ledger into a dirty one, and one set
/// of dashboards serves every ledger the app opens.
pub(crate) fn dashboards_dir(app: &tauri::AppHandle<AppRuntime>) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map_err(|error| format!("no config directory: {error}"))
}

/// Every saved dashboard, oldest id order as stored; a first run answers with
/// the six seeded defaults (and writes nothing until one is saved).
#[tauri::command]
#[specta::specta]
pub fn dashboards(app: tauri::AppHandle<AppRuntime>) -> Result<Vec<Dashboard>, String> {
    dashboards::load(&dashboards_dir(&app)?)
}

/// Create or replace one dashboard by id.
#[tauri::command]
#[specta::specta]
pub fn save_dashboard(
    dashboard: Dashboard,
    app: tauri::AppHandle<AppRuntime>,
) -> Result<(), String> {
    dashboards::save(&dashboards_dir(&app)?, dashboard)
}

/// Remove one dashboard; `false` when no dashboard had that id.
#[tauri::command]
#[specta::specta]
pub fn delete_dashboard(id: String, app: tauri::AppHandle<AppRuntime>) -> Result<bool, String> {
    dashboards::delete(&dashboards_dir(&app)?, &id)
}

/// The folder the chooser and the repository work from: the open ledger's own
/// directory, which is what `repo`/`storage` need (never the journal file).
fn ledger_dir(state: &AppState) -> Result<PathBuf, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger is open")?;
    loaded
        .root
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{} has no directory", loaded.root.display()))
}

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

/// Where the app's own data lives (indexes, dashboards, the assistant's
/// settings): the app's config directory — per-app data, never part of a
/// ledger, so none of it can dirty a read-only journal.
pub(crate) fn app_config_dir(app: &tauri::AppHandle<AppRuntime>) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map_err(|error| format!("no config directory: {error}"))
}

/// The quote sources this build can ask (`ecb`, `yahoo`, `tefas`, …).
#[tauri::command]
#[specta::specta]
pub fn price_sources() -> Vec<crate::prices::PriceSourceInfo> {
    crate::prices::sources()
}

/// Every commodity the ledger could price, with `latest` being the newest
/// `price` directive already in it — the prices view never fetches on load.
#[tauri::command]
#[specta::specta]
pub fn price_targets(state: State<'_, AppState>) -> Vec<crate::prices::PriceTarget> {
    crate::prices::targets(&state)
}

/// Fetch quotes for the given requests. Blocking network work, so it runs on a
/// worker thread; per-request failures come back as `PriceFetch::Failed` and the
/// command only errors when every request failed.
#[tauri::command]
#[specta::specta]
pub async fn fetch_prices(
    requests: Vec<crate::prices::QuoteRequest>,
    app: tauri::AppHandle<AppRuntime>,
) -> Result<Vec<crate::prices::PriceFetch>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        crate::prices::fetch(&state, requests)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Append fetched quotes to one journal file as `price` directives, through the
/// same loader-verified write pipeline as every other edit.
#[tauri::command]
#[specta::specta]
pub fn write_prices(
    file: String,
    quotes: Vec<crate::prices::PriceQuote>,
    state: State<'_, AppState>,
) -> Result<u32, String> {
    crate::prices::write_to_file(&state, &file, &quotes)
        .map(|appended| u32::try_from(appended).unwrap_or(u32::MAX))
}

// --------------------------------------------------------------- assistant ---

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

// ----------------------------------------------------------------- storage ---

/// Which storage the ledger lives in: a directory this process reads directly,
/// or an Android SAF tree behind a private mirror. The sidebar reads this to
/// decide whether the git views can exist at all.
#[tauri::command]
#[specta::specta]
pub fn storage_kind() -> crate::storage::StorageKind {
    crate::storage::kind()
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
        tauri::async_runtime::spawn_blocking(move || {
            with_tree(&handle, |tree, mirror| {
                crate::storage::sync_back(tree, mirror)
            })
        })
        .await
        .map_err(|error| error.to_string())?
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

// ------------------------------------------------------------------ query ---

/// Run one BQL statement against the loaded ledger. Every cell is text, so
/// amounts reach the caller exactly as the ledger holds them.
#[tauri::command]
#[specta::specta]
pub fn query(
    bql: String,
    max_rows: u32,
    state: State<'_, AppState>,
) -> Result<crate::query::QueryTable, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger is open")?;
    let cap = if max_rows == 0 {
        crate::query::TOOL_MAX_ROWS
    } else {
        max_rows
    };
    crate::query::run(&loaded.ledger, &bql, cap)
}

// -------------------------------------------------------------- assistant ---

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

/// What the phone's quick-add sheet opens with: the last pair used and the most
/// frequent categories/payees over a window of days. One pass over the ledger,
/// instead of shipping the whole register to the browser.
#[tauri::command]
#[specta::specta]
pub fn quick_add_suggestions(
    days: u32,
    state: State<'_, AppState>,
) -> Result<crate::suggest::QuickAdd, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger is open")?;
    let accounts = account_balances(
        &loaded.ledger,
        &modified_accounts(&loaded.ledger, &loaded.baseline),
    );
    Ok(crate::suggest::quick_add(&loaded.ledger, &accounts, days))
}

/// Android: reopen the tree the app was last pointed at, before the window
/// shows. A phone cannot resolve a folder by path, so the URI the picker stored
/// is the only way back to the ledger; the mirror is rebuilt from it and the
/// ledger loaded from the mirror.
///
/// Runs from `setup` and never fails the launch: a revoked grant, a deleted
/// folder or a tree without a journal leaves the app open with whatever the
/// local fallback found, which is what `pick_ledger_folder` can then fix.
#[cfg(target_os = "android")]
pub fn restore_tree(app: &tauri::AppHandle<AppRuntime>) {
    let Ok(config) = app_config_dir(app) else {
        return;
    };
    let Ok(json) = std::fs::read_to_string(config.join("storage.json")) else {
        return;
    };
    let Ok(tree) = crate::storage::AndroidTree::from_json(app, json.trim()) else {
        return;
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
            eprintln!("tree restore: {error}");
            return;
        }
    }
    crate::storage::set_tree_active(true);
    // The journal inside the mirror, resolved the same way a picked folder is.
    match crate::session::load_baselined(&mirror) {
        Ok(loaded) => {
            let state = app.state::<AppState>();
            *state.loaded.write().unwrap_or_else(|e| e.into_inner()) = Some(loaded);
        }
        Err(error) => eprintln!("tree restore: {}: {error}", mirror.display()),
    }
}

// ---------------------------------------------------------------- indexes ---

/// Where price indexes live: beside the dashboards in the app's config dir, so
/// a fetched series is shared by every ledger the app opens.
fn indexes_dir(app: &tauri::AppHandle<AppRuntime>) -> Result<PathBuf, String> {
    Ok(app_config_dir(app)?.join("indexes"))
}

/// What a provider publishes, without its points — the picker's fix, without
/// points: a stored series can be refreshed, but with nothing stored there is
/// still a list of series worth adding.
///
/// `endpoint` is `Err` for the publishers that expose no stable download (TÜİK's
/// portal hands out short-lived signed links; ENAG prints a table), and the
/// reason is shown rather than a broken "refresh".
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct IndexProviderInfo {
    pub id: String,
    pub label: String,
    pub source: String,
    pub unofficial: bool,
    pub currency: String,
    pub downloadable: bool,
    pub note: String,
}

/// Every index series this build knows how to build.
#[tauri::command]
#[specta::specta]
pub fn index_provider_list() -> Vec<IndexProviderInfo> {
    crate::index_providers::PROVIDERS
        .iter()
        .map(|provider| {
            let meta = provider.series();
            let (downloadable, note) = match provider.endpoint() {
                Ok(endpoint) => (true, endpoint),
                Err(reason) => (false, reason),
            };
            IndexProviderInfo {
                id: meta.id.to_owned(),
                label: meta.label.to_owned(),
                source: meta.source.to_owned(),
                unofficial: meta.unofficial,
                currency: meta.currency.to_owned(),
                downloadable,
                note,
            }
        })
        .collect()
}

/// The stored index series — what the inflation screen lists and the lifestyle
/// report can be run against.
#[tauri::command]
#[specta::specta]
pub fn index_series(
    app: tauri::AppHandle<AppRuntime>,
) -> Result<Vec<crate::index::IndexSeries>, String> {
    crate::index_store::load_indexes(&indexes_dir(&app)?)
}

/// Store a series the user pasted or imported as CSV (`date,value` rows).
#[tauri::command]
#[specta::specta]
pub fn index_import(
    id: String,
    label: String,
    source: String,
    currency: String,
    csv_text: String,
    app: tauri::AppHandle<AppRuntime>,
) -> Result<crate::index::IndexSeries, String> {
    crate::index_store::import_csv(
        &indexes_dir(&app)?,
        &id,
        &label,
        &source,
        &currency,
        &csv_text,
    )
}

/// Download one provider's series and store it. Blocking network work on a
/// worker thread; the provider id is the series id (`us-cpi`, `tr-enag`, …).
#[tauri::command]
#[specta::specta]
pub async fn index_refresh(
    id: String,
    app: tauri::AppHandle<AppRuntime>,
) -> Result<crate::index::IndexSeries, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let provider = crate::index_providers::provider(&id)
            .ok_or_else(|| format!("no provider is called `{id}`"))?;
        let series = provider.fetch()?;
        crate::index_store::save_series(&indexes_dir(&app)?, &series)?;
        Ok(series)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Spending per month in nominal and index-adjusted terms, per category — the
/// lifestyle-inflation screen's data. `index` names a stored series.
#[tauri::command]
#[specta::specta]
pub fn lifestyle_report(
    months: u32,
    index: String,
    app: tauri::AppHandle<AppRuntime>,
    state: State<'_, AppState>,
) -> Result<crate::reports::LifestyleReport, String> {
    let series = crate::index_store::load_indexes(&indexes_dir(&app)?)?
        .into_iter()
        .find(|series| series.id == index)
        .ok_or_else(|| format!("no stored index is called `{index}`"))?;
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger is open")?;
    crate::reports::lifestyle_report(&loaded.ledger, months, &series)
}

// ---------------------------------------------------------- conversations ---

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
    // a conversation it thinks does not exist yet. The flush walks the tree
    // through the SAF provider's blocking calls, so it runs off this thread.
    let handle = app.clone();
    if let Err(error) = tauri::async_runtime::spawn_blocking(move || sync_tree_now(&handle))
        .await
        .map_err(|error| error.to_string())
        .and_then(|flushed| flushed)
    {
        eprintln!("conversation not copied to the folder: {error}");
    }
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
    let handle = app.clone();
    if let Err(error) = tauri::async_runtime::spawn_blocking(move || sync_tree_now(&handle))
        .await
        .map_err(|error| error.to_string())
        .and_then(|flushed| flushed)
    {
        eprintln!("conversation rename not copied to the folder: {error}");
    }
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
    let handle = app.clone();
    if let Err(error) = tauri::async_runtime::spawn_blocking(move || sync_tree_now(&handle))
        .await
        .map_err(|error| error.to_string())
        .and_then(|flushed| flushed)
    {
        eprintln!("conversation deletion not copied to the folder: {error}");
    }
    Ok(())
}

// ---------------------------------------------------------------- history ---

/// Android: push what the mirror holds into the SAF tree, without a report.
///
/// Conversations, chat transcripts and the log live inside the ledger folder, so
/// they reach the folder through the same mirror as an edit. Nothing on those
/// paths bumps the ledger revision, which is why the write-back cannot be left
/// to the UI's effect: a conversation the app never flushed is deleted by the
/// next restore's reconciliation as if the folder had deleted it.
fn sync_tree_now(app: &tauri::AppHandle<AppRuntime>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        if crate::storage::kind() != crate::storage::StorageKind::Tree {
            return Ok(());
        }
        with_tree(app, |tree, mirror| {
            crate::storage::sync_back(tree, mirror).map(|_| ())
        })
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(())
    }
}

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

// -------------------------------------------------------------- conflicts ---

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

// ----------------------------------------------------------------- reports ---

/// Month-end net worth over the last `months` months, oldest first.
#[tauri::command]
#[specta::specta]
pub fn report_net_worth(
    months: u32,
    state: State<'_, AppState>,
) -> Result<Vec<crate::reports::NetWorthMonth>, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger is open")?;
    Ok(crate::reports::net_worth_months(&loaded.ledger, months))
}

/// Money in and out per month over the last `months` months.
#[tauri::command]
#[specta::specta]
pub fn report_cashflow(
    months: u32,
    state: State<'_, AppState>,
) -> Result<Vec<crate::reports::CashflowMonth>, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger is open")?;
    Ok(crate::reports::cashflow_months(&loaded.ledger, months))
}

/// Spending per expense category per month over the last `months` months.
#[tauri::command]
#[specta::specta]
pub fn report_categories(
    months: u32,
    state: State<'_, AppState>,
) -> Result<Vec<crate::reports::CategoryMonths>, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger is open")?;
    Ok(crate::reports::category_months(&loaded.ledger, months))
}

// ------------------------------------------------------------- attachments ---

/// Resolve a path the frontend named, refusing anything outside the ledger's own
/// directory: attachments are user files beside the journal, never `/etc`.
fn ledger_path(state: &AppState, path: &str) -> Result<PathBuf, String> {
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

// ------------------------------------------------------------- file tools ---

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

// -------------------------------------------------------------------- git ---

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
fn note_best_effort(state: &State<'_, AppState>) {
    if let Err(error) = note_rewritten_files(state) {
        eprintln!("history: rewritten files not noted: {error}");
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
