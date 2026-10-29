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
use crate::repo;
use crate::lsp;
use crate::model::{
    AccountBalance, BudgetMonth, FolderListing, JournalFile, LoadSummary, NewTransaction,
    TransactionEdit, TransactionFile, TransactionView,
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
    // A server started for the old journal has the old ledger in memory; the
    // next editor starts one for this one.
    lsp::stop();
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

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        let _ = (&app, start);
        Err("this platform has no folder chooser yet".to_owned())
    }
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
    state: State<'_, AppState>,
) -> Result<(), String> {
    edit_named_file_source(&state, &file, &text)
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

/// Where price indexes and dashboards live: the app's config directory, keyed
/// by nothing yet — both are per-app data, not ledger data.
pub(crate) fn app_data_dir(app: &tauri::AppHandle<AppRuntime>) -> Result<PathBuf, String> {
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
) -> Result<(), String> {
    crate::prices::write_to_file(&state, &file, &quotes)
}

// --------------------------------------------------------------- assistant ---

/// Where the assistant's own settings live: the app's config dir, never the
/// ledger — the key must not be committed.
fn ai_dir(app: &tauri::AppHandle<AppRuntime>) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join("ai"))
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

// ---------------------------------------------------------------- indexes ---

/// Where price indexes live: beside the dashboards in the app's config dir, so
/// a fetched series is shared by every ledger the app opens.
fn indexes_dir(app: &tauri::AppHandle<AppRuntime>) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join("indexes"))
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
pub fn conversations(
    state: State<'_, AppState>,
) -> Result<crate::conversations::Listing, String> {
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
pub fn conversation_append(
    id: String,
    title: String,
    model: String,
    message: crate::conversations::Message,
    state: State<'_, AppState>,
) -> Result<crate::conversations::Summary, String> {
    let dir = ledger_dir(&state)?;
    if id.is_empty() {
        crate::conversations::create(&dir, &crate::conversations::new_id(), &title, &model, &[])?;
    }
    let id = if id.is_empty() {
        crate::conversations::list(&dir)?
            .sessions
            .first()
            .map(|session| session.id.clone())
            .ok_or("the conversation could not be created")?
    } else {
        id
    };
    crate::conversations::append(&dir, &id, &message)
}

/// Rename a conversation (the file name stays: it is a creation record).
#[tauri::command]
#[specta::specta]
pub fn conversation_rename(
    id: String,
    title: String,
    state: State<'_, AppState>,
) -> Result<crate::conversations::Summary, String> {
    crate::conversations::rename(&ledger_dir(&state)?, &id, &title)
}

/// Delete a conversation's file; its history is the repository's.
#[tauri::command]
#[specta::specta]
pub fn conversation_delete(id: String, state: State<'_, AppState>) -> Result<(), String> {
    crate::conversations::delete(&ledger_dir(&state)?, &id)
}

// ---------------------------------------------------------------- history ---

/// Where the operation log lives: the ledger's own hidden directory, so the
/// lines and images travel with the ledger when it syncs through a file sync.
fn history_dir(state: &AppState) -> Result<PathBuf, String> {
    Ok(ledger_dir(state)?.join(crate::conversations::DIRECTORY).join("log"))
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
    let origin = origin
        .as_deref()
        .map(|name| match name {
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
    let mut restored = Vec::new();
    for entry in plan {
        let Some(bytes) = entry.before else {
            return Err(format!(
                "{} did not exist before operation {id}; undo would have to delete it",
                entry.path.display()
            ));
        };
        let text = String::from_utf8(bytes)
            .map_err(|_| format!("{} is not UTF-8 text", entry.path.display()))?;
        edit_named_file_source(&state, &entry.path.display().to_string(), &text)?;
        restored.push(entry.path.display().to_string());
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
    crate::conflict_cmd::compare(&ledger_dir(&state)?, Path::new(&original), Path::new(&copy))
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
    crate::conflict_cmd::resolve(&state, Path::new(&original), &picks)
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
                pick_ledger_folder,
                lsp_start,
                lsp_send,
                lsp_file_path,
                update_transaction,
                transaction_source,
                update_transaction_source,
                transaction_file,
                update_file_source,
                journal_files,
                file_transactions,
                file_text,
                update_file_text,
                suggest_transaction_file,
                add_transaction,
                budget,
                budget_range,
                dashboards,
                save_dashboard,
                delete_dashboard,
                git_state,
                git_init,
                git_set_identity,
                price_sources,
                price_targets,
                fetch_prices,
                write_prices,
                ai_settings,
                ai_configure,
                ai_models,
                ai_tools,
                ai_chat,
                index_provider_list,
                index_series,
                index_import,
                index_refresh,
                lifestyle_report,
                conversations,
                conversation,
                conversation_append,
                conversation_rename,
                conversation_delete,
                history,
                history_undo,
                sync_conflicts,
                conflict_compare,
                conflict_resolve,
                ledger_diff,
                commit_ledger,
            ])
            .export(Typescript::default(), "../src/bindings.ts")
            .unwrap();
    }
}
