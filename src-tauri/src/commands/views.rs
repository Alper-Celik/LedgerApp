// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Read-side commands over the loaded ledger — accounts, files, budgets, prices,
//! indexes, queries and reports — and the dashboards that arrange them.

use std::path::PathBuf;

use tauri::{Manager, State};

use crate::AppRuntime;
use crate::accounts::{account_balances, account_transaction_views, modified_accounts};
use crate::budget::{BudgetRange, budget_view};
use crate::budget_write::{self, BudgetSettings};
use crate::dashboards::{self, Dashboard};
use crate::editing::{
    create_transaction, edit_file_source, edit_named_file_source, edit_transaction,
    edit_transaction_source, transaction_file_view, transaction_source_text,
};
use crate::files::{
    file_transaction_views, journal_file_views, source_of_file, suggested_transaction_file,
};
use crate::model::{
    AccountBalance, BudgetMonth, JournalFile, NewTransaction, TransactionEdit, TransactionFile,
    TransactionView,
};
use crate::session::AppState;

use super::app_config_dir;

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
/// means. The figures stay read-only because the plugin computes them on every
/// load; what the screen writes instead is the configuration behind them —
/// [`budget_settings`], [`budget_init`], [`budget_fund_set`] and
/// [`budget_rollover_set`].
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

/// The envelope budget's configuration, as the editor reads it: whether the
/// journal declares an envelope plugin, the parsed config block (`None` until a
/// `start date` exists) and every account a `fund`/`rollover` names, with the
/// newest value and the month it started (`since`).
///
/// Read from the ledger's `custom "envelope"` directives, never from the
/// `Equity:Envelope:` entries the plugin writes: those are the plugin's output
/// and are recomputed on every load.
#[tauri::command]
#[specta::specta]
pub fn budget_settings(state: State<'_, AppState>) -> Result<BudgetSettings, String> {
    budget_write::settings(&state)
}

/// Append the budget's configuration block to a journal file: `start date`, an
/// optional `end date` and `months ahead`, one `budget account` pattern per
/// entry, and `assert`/`strict` only when they differ from the plugin's
/// defaults. Refuses when the journal already has a `start date` (the error says
/// where) or when `budget_accounts` is empty.
///
/// The write goes through the loader-verified edit pipeline, so an invalid
/// pattern is refused by the plugin and the file is put back byte-for-byte.
/// `plugin_declared` on [`budget_settings`] is the form's warning that the
/// journal does not load an envelope plugin yet; it is not a precondition.
#[tauri::command]
#[specta::specta]
pub fn budget_init(
    file: String,
    start: String,
    months_ahead: Option<u32>,
    end: Option<String>,
    budget_accounts: Vec<String>,
    r#assert: bool,
    strict: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    budget_write::init(
        &state,
        &file,
        &start,
        months_ahead,
        end.as_deref(),
        &budget_accounts,
        r#assert,
        strict,
    )
}

/// Make `units currency` the amount assigned to `account` from `month`
/// (`YYYY-MM`) on.
///
/// The plugin inherits a `fund` until a later one replaces it, so this is an
/// upsert: the value already taking effect is left alone when unchanged, an
/// existing directive dated exactly the month's first day is rewritten rather
/// than duplicated, and otherwise one line is added. `0.00` is legal and stops
/// the funding; the plugin's own `To-Be-Budgeted`/`Money` accounts are refused.
#[tauri::command]
#[specta::specta]
pub fn budget_fund_set(
    file: String,
    account: String,
    units: String,
    currency: String,
    month: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    budget_write::fund_set(&state, &file, &account, &units, &currency, &month)
}

/// Set `account`'s rollover policy from `month` on: `carry`, `reset`, or `cap`
/// with `units`/`currency`.
///
/// Same dating and upsert rules as [`budget_fund_set`]: an unchanged policy
/// writes nothing, a directive at exactly the month's first day is rewritten,
/// and a cap without both `units` and `currency` is refused.
#[tauri::command]
#[specta::specta]
pub fn budget_rollover_set(
    file: String,
    account: String,
    kind: String,
    units: Option<String>,
    currency: Option<String>,
    month: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    budget_write::rollover_set(
        &state,
        &file,
        &account,
        &kind,
        units.as_deref(),
        currency.as_deref(),
        &month,
    )
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

/// One commodity's price history, from the two sources the journal states it
/// in: explicit `price` directives, the rates implied by trades, or both.
///
/// `source` is `"explicit"`, `"transactions"` or `"both"`; `currency` narrows
/// the answer to one quote currency; `from`/`to` are inclusive `YYYY-MM` month
/// labels and `limit` keeps only the newest that many points. A point says
/// which source it came from, and `both` prefers the explicit price when a day
/// has one. Every amount is an exact decimal string; see
/// [`crate::price_history`] for the full rule.
#[tauri::command]
#[specta::specta]
pub fn price_history(
    commodity: String,
    currency: Option<String>,
    source: String,
    from: Option<String>,
    to: Option<String>,
    limit: Option<u32>,
    state: State<'_, AppState>,
) -> Result<crate::price_history::PriceHistory, String> {
    crate::price_history::history(
        &state,
        &commodity,
        currency.as_deref(),
        &source,
        from.as_deref(),
        to.as_deref(),
        limit,
    )
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
