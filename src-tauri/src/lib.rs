// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use specta_typescript::Typescript;
use tauri_specta::{Builder, collect_commands};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, RwLock};

use rustledger_core::{Decimal, Directive, IncompleteAmount, MetaValue, Span, Spanned, Transaction};
use rustledger_loader::{Ledger, LedgerError, LoadOptions, SourceFile};
use tauri::State;

/// The runtime this app runs on: the Qt runtime (Qt windowing + QtWebEngine)
/// on Linux, wry everywhere else — or on Linux too when built with
/// `--features wry`.
#[cfg(all(target_os = "linux", not(feature = "wry")))]
pub type AppRuntime = tauri_runtime_qt::QtRuntime;
#[cfg(any(not(target_os = "linux"), feature = "wry"))]
pub type AppRuntime = tauri::Wry;

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
#[specta::specta]
fn greet(name: String) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// Accounts the ledger's envelope-budgeting plugin generates
/// (`plugins/rustledger-envelope`). The whole budget lives under this prefix, so
/// one string is enough to keep it out of the app — tree, balances and register
/// — while the plugin keeps doing the arithmetic.
const ENVELOPE_NAMESPACE: &str = "Equity:Envelope";

/// Whether `account` is one the envelope plugin generated.
fn is_envelope_account(account: &str) -> bool {
    account == ENVELOPE_NAMESPACE
        || (account.len() > ENVELOPE_NAMESPACE.len()
            && account.starts_with(ENVELOPE_NAMESPACE)
            && account.as_bytes()[ENVELOPE_NAMESPACE.len()] == b':')
}

/// How a file, an entry or an account stands against the ledger's last commit —
/// the one letter a git client would put beside it.
///
/// The reference is HEAD's blob where git has one, and the text the session
/// started with otherwise (see [`Loaded::baseline`]): a ledger that is not
/// committed yet is not a change in itself, only what was written to it is.
/// Whether that change is *staged* is not distinguished — every edit this app
/// makes stages its file on the way out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    /// As committed, or a ledger with no commit to be measured against.
    Unchanged,
    /// New to git: the file is not in the last commit, or the entry's lines were
    /// not there before.
    Added,
    /// There and rewritten: the file is committed and differs from it, or the
    /// entry's lines stand where committed lines were.
    Modified,
}

/// Fully-qualified Beancount account name, e.g. "Expenses:Food:Groceries".
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct AccountBalance {
    pub name: String,
    /// Exact subtree balance in the account's primary commodity (the first
    /// `option "operating_currency"` it holds, else the commodity with the
    /// largest absolute position), as a decimal string — beancount `bal`
    /// semantics: leaves report their own postings, parents the sum of their
    /// whole subtree. The frontend never sums.
    pub balance: String,
    /// Beancount commodity of `balance`, e.g. `USD` or `TRY` — the code the
    /// frontend needs to place a symbol or code next to the amount.
    pub currency: String,
    /// How this account stands against the last commit: the strongest change in
    /// its own subtree, [`Change::Modified`] over [`Change::Added`], so a branch
    /// reads as changed when anything under it is. [`Change::Unchanged`] when
    /// the ledger is not in a git repository or nothing changed — the sidebar
    /// never fails over git.
    pub change: Change,
}

/// One posting as shown on an account page.
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct PostingView {
    pub account: String,
    /// Exact units as a decimal string.
    pub units: String,
    pub currency: String,
}

/// A transaction that touches an account, with every posting it contains.
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct TransactionView {
    /// Position of the transaction in the ledger's transaction stream; stable id for the UI.
    pub id: u32,
    /// `YYYY-MM-DD`.
    pub date: String,
    /// Transaction flag, `*` or `!`.
    pub flag: String,
    pub payee: Option<String>,
    pub narration: String,
    pub postings: Vec<PostingView>,
    /// How the entry's own lines stand against the last commit: [`Change::Added`]
    /// when its lines are all new, [`Change::Modified`] when they stand where
    /// committed lines were. [`Change::Unchanged`] everywhere when the ledger is
    /// not in a git repository or nothing changed — the register never fails
    /// over git.
    pub change: Change,
}

/// Fields the register may change about one transaction. `id` is the
/// `TransactionView.id` the frontend already has: the transaction's index in the
/// ledger's transaction stream, which this command re-derives the same way.
#[derive(Debug, serde::Deserialize, specta::Type)]
pub struct TransactionEdit {
    pub id: u32,
    pub date: String,          // YYYY-MM-DD
    pub flag: String,          // "*" or "!"
    pub payee: Option<String>,
    pub narration: String,
    /// Posting amount edits, by the posting's index inside the transaction.
    pub postings: Vec<PostingEdit>,
}

/// One posting's new units figure. Everything else on the posting's line — its
/// currency, `@`/`@@` price, `{cost}` spec and comment — stays as written.
#[derive(Debug, serde::Deserialize, specta::Type)]
pub struct PostingEdit {
    pub index: u32,
    pub units: String,
}

/// A whole ledger file as the source editor sees it: the text the editor is
/// handed, the path it came from, and the transaction's position in it. Line and
/// column are 1-based and in Monaco's coordinates (`column` counts UTF-16 code
/// units, which is how a Monaco model measures a line), so the editor can reveal
/// the entry without re-deriving anything from byte offsets.
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct TransactionFile {
    pub path: String,
    pub text: String,
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

/// One file of the loaded journal — the journal the session was opened with and
/// every file it pulls in with `include`. These are the files a new transaction
/// may be written into, and what the file explorer lists.
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct JournalFile {
    /// The path as loaded, which is what a new entry sends back to say where to
    /// write. The frontend only ever echoes a path it was handed; it never
    /// builds one.
    pub path: String,
    /// Name to show: the path relative to the journal's own directory when the
    /// file lives under it, else the path as it stands.
    pub label: String,
    /// How many transactions the file itself writes — every entry in it, the
    /// ones the register hides included, since this is a fact about the file
    /// rather than about a view of the ledger.
    pub transactions: u32,
    /// How the file stands in git: [`Change::Modified`] when the file is in the
    /// last commit and differs from it, [`Change::Added`] when git does not have
    /// it and it has been written to. The same measure the entries inside it
    /// carry, read at file level.
    pub change: Change,
}

/// A transaction that is not in the ledger yet, as the new-entry form fills it
/// out: the shape of a [`TransactionEdit`] plus the file to write it into, and
/// with the amounts optional — a posting left without a figure is the leg
/// beancount interpolates.
#[derive(Debug, serde::Deserialize, specta::Type)]
pub struct NewTransaction {
    /// Path of the journal file to append to, as [`journal_files`] reported it.
    pub file: String,
    pub date: String, // YYYY-MM-DD
    pub flag: String, // "*" or "!"
    pub payee: Option<String>,
    pub narration: String,
    pub postings: Vec<NewPosting>,
}

/// One posting of a new transaction: the account, and what it is worth.
#[derive(Debug, serde::Deserialize, specta::Type)]
pub struct NewPosting {
    pub account: String,
    /// The units figure as typed. `None` (or empty) writes the account alone and
    /// leaves the amount to be interpolated at load time.
    pub units: Option<String>,
    /// The commodity. `None` (or empty) writes the ledger's first
    /// `option "operating_currency"`.
    pub currency: Option<String>,
}

/// An account the ledger does not have yet, as the new-account form fills it
/// out. What it becomes is one `open` directive.
#[derive(Debug, serde::Deserialize, specta::Type)]
pub struct NewAccount {
    /// Path of the journal file to append to, as [`journal_files`] reported it.
    pub file: String,
    /// The account's full name, e.g. `Expenses:Coffee`.
    pub name: String,
    /// The day it is opened, `YYYY-MM-DD`.
    pub date: String,
    /// The commodities it may hold. Empty is beancount's "any of them".
    pub currencies: Vec<String>,
}

/// A loaded ledger together with the baseline the register's `modified` flags
/// are measured against.
struct Loaded {
    ledger: Ledger,
    /// The journal this session was opened with — the file the user pointed at,
    /// with its `include`s followed. Every edit reloads *this*, never the file
    /// it just wrote: an included file on its own carries none of the `open`
    /// directives that live in its parent, so reloading it alone reports every
    /// account it touches as never opened.
    root: PathBuf,
    /// Every file's text as this session first saw it, keyed by path. It stands
    /// in wherever git has no HEAD blob for the file: a ledger that is not
    /// committed yet — a fresh journal, a directory that was never added — is
    /// not a change in itself, only the edits made to it are.
    baseline: HashMap<PathBuf, Arc<str>>,
}

impl Loaded {
    /// Wrap a freshly loaded ledger taken from `root`, recording what its files
    /// look like now.
    fn fresh(ledger: Ledger, root: PathBuf) -> Self {
        let mut loaded = Self {
            ledger,
            root,
            baseline: HashMap::new(),
        };
        loaded.remember_files();
        loaded
    }

    /// Take in files this session has not seen before, keeping what earlier
    /// loads recorded so that an edit still reads as a change against its
    /// baseline.
    fn remember_files(&mut self) {
        for file in self.ledger.source_map.files() {
            self.baseline
                .entry(file.path.clone())
                .or_insert_with(|| Arc::clone(&file.source));
        }
    }
}

/// Load a ledger from disk and take the session baseline for its files, so the
/// app and a reload after an edit agree on what "changed" means.
fn load_baselined(path: &Path) -> Result<Loaded, String> {
    rustledger_loader::load(path, &LoadOptions::default())
        .map(|ledger| Loaded::fresh(ledger, path.to_path_buf()))
        .map_err(|e| e.to_string())
}

/// Shared app state: the currently loaded ledger, if any.
#[derive(Default)]
struct AppState {
    loaded: RwLock<Option<Loaded>>,
}

impl AppState {
    /// Open the ledger at startup: the last CLI argument if one was given,
    /// else `LEDGER_PATH` from the environment (set per-machine via a
    /// git-ignored `.envrc.local`), else `main.bean` in the working
    /// directory. A missing or broken file just leaves the app running with
    /// no ledger loaded.
    fn new() -> Self {
        let path = resolve_ledger_path();
        let loaded = match load_baselined(Path::new(&path)) {
            Ok(loaded) => Some(loaded),
            Err(e) => {
                eprintln!("not loading {}: {e}", path.display());
                None
            }
        };
        Self {
            loaded: RwLock::new(loaded),
        }
    }
}

/// Resolve the ledger to open: last CLI argument, else `LEDGER_PATH`, else
/// `main.bean`. A directory is resolved to the root journal inside it.
fn resolve_ledger_path() -> PathBuf {
    let raw = std::env::args()
        .skip(1) // drop the program name
        .last()
        .or_else(|| std::env::var("LEDGER_PATH").ok())
        .unwrap_or_else(|| "main.bean".to_string());
    let path = PathBuf::from(raw);
    if path.is_dir() {
        rustledger_loader::discover_journal_file(&path).unwrap_or(path)
    } else {
        path
    }
}

/// Summary of a successful ledger load.
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct LoadSummary {
    /// Path of the loaded root journal.
    pub path: String,
    /// Number of loaded directives (after booking and plugins).
    pub directive_count: u32,
    /// Number of validation/processing errors collected; the ledger is still
    /// loaded and usable.
    pub error_count: u32,
}

/// Parse, book, and validate a beancount file, then keep it as app state.
/// A failed load leaves the previously loaded ledger untouched.
#[tauri::command]
#[specta::specta]
fn load_ledger(path: String, state: State<'_, AppState>) -> Result<LoadSummary, String> {
    let loaded = load_baselined(Path::new(&path))?;
    let summary = LoadSummary {
        path,
        directive_count: u32::try_from(loaded.ledger.directives.len()).unwrap_or(u32::MAX),
        error_count: u32::try_from(loaded.ledger.errors.len()).unwrap_or(u32::MAX),
    };
    // A poisoned lock means an earlier command panicked while holding the ledger;
    // its contents are still consistent, so recover rather than propagating it as
    // a permanent failure to every later command.
    *state.loaded.write().unwrap_or_else(|e| e.into_inner()) = Some(loaded);
    Ok(summary)
}

/// One entry per account that appears in the tree — accounts with postings
/// and their ancestors — each carrying its full subtree balance.
#[tauri::command]
#[specta::specta]
fn accounts(state: State<'_, AppState>) -> Vec<AccountBalance> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    guard
        .as_ref()
        .map(|loaded| {
            let changes = account_changes(&loaded.ledger, &loaded.baseline);
            account_balances(&loaded.ledger, &changes)
        })
        .unwrap_or_default()
}

/// How each account in the ledger's tree stands against the last commit, by
/// full account name: the change of the entries that touch it, rolled up every
/// colon-prefix ancestor so a parent branch reads as changed when anything under
/// it is. [`Change::Modified`] wins over [`Change::Added`] where both meet.
///
/// The whole walk is skipped when the working tree has nothing to report, so a
/// clean ledger costs one diff.
fn account_changes(ledger: &Ledger, baseline: &HashMap<PathBuf, Arc<str>>) -> BTreeMap<String, Change> {
    let changed = working_tree_changes(ledger, baseline);
    let mut marked: BTreeMap<String, Change> = BTreeMap::new();
    if changed.is_empty() {
        return marked;
    }
    let view = ledger.balance_view();
    let sources = source_directive_index(ledger, &view);
    let mut line_starts: HashMap<u16, Vec<usize>> = HashMap::new();
    // An account's own `open` line counts before anything is posted to it:
    // opening an account is a change to that account, not only to its file.
    for source in &ledger.directives {
        let Directive::Open(open) = &source.value else {
            continue;
        };
        let account = open.account.as_str();
        if is_envelope_account(account) {
            continue;
        }
        let change = directive_change(ledger, &changed, &mut line_starts, source);
        if change != Change::Unchanged {
            mark_account(&mut marked, account, change);
        }
    }
    for (id, directive) in view.iter().enumerate() {
        let Directive::Transaction(tx) = directive else {
            continue;
        };
        let change = sources
            .get(id)
            .copied()
            .flatten()
            .map_or(Change::Unchanged, |index| {
                directive_change(ledger, &changed, &mut line_starts, &ledger.directives[index])
            });
        if change == Change::Unchanged {
            continue;
        }
        for posting in &tx.postings {
            mark_account(&mut marked, posting.value.account.as_str(), change);
        }
    }
    marked
}

/// Marks every colon-prefix of `account` — the account itself and each ancestor
/// — with `change`, keeping the stronger of the two where they meet:
/// [`Change::Modified`] over [`Change::Added`], so a branch that holds an edited
/// entry reads as modified rather than merely added to.
fn mark_account(marked: &mut BTreeMap<String, Change>, account: &str, change: Change) {
    let mut prefix = String::new();
    for part in account.split(':') {
        prefix = if prefix.is_empty() {
            part.to_owned()
        } else {
            format!("{prefix}:{part}")
        };
        let entry = marked.entry(prefix.clone()).or_insert(Change::Unchanged);
        if change == Change::Modified || *entry == Change::Unchanged {
            *entry = change;
        }
    }
}

/// Transactions touching `account` — the account itself or any sub-account —
/// newest first, each carrying all of its postings so the UI can show the whole
/// entry even when only one leg hits this account. An empty `account` is the
/// virtual root: the whole ledger.
#[tauri::command]
#[specta::specta]
fn account_transactions(account: String, state: State<'_, AppState>) -> Vec<TransactionView> {
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
fn update_transaction(edit: TransactionEdit, state: State<'_, AppState>) -> Result<(), String> {
    edit_transaction(&state, edit)
}

/// The transaction's exact source text (its own lines, as they appear in the
/// file): the header from its date token on, every posting line, and the
/// newline that ends the last of them, with their original indentation. This
/// is the byte range [`update_transaction_source`] rewrites.
#[tauri::command]
#[specta::specta]
fn transaction_source(id: u32, state: State<'_, AppState>) -> Result<String, String> {
    transaction_source_text(&state, id)
}

/// Replace a transaction's source text with `text` — the source editor's way in
/// — through the same pipeline as [`update_transaction`]: reload the ledger and
/// refuse the edit (putting the file back byte-for-byte) if the reload fails or
/// raises an error the ledger did not already have, and stage the file on
/// success.
#[tauri::command]
#[specta::specta]
fn update_transaction_source(
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
fn transaction_file(id: u32, state: State<'_, AppState>) -> Result<TransactionFile, String> {
    transaction_file_view(&state, id)
}

/// Replace a ledger file's entire source text with `text` — the file editor's
/// way out — through the same pipeline as [`update_transaction_source`]: the
/// loader, not this function, is the authority on whether arbitrary text is a
/// legal ledger, a refusal puts the file back byte-for-byte, and a success
/// stages it for a later commit.
#[tauri::command]
#[specta::specta]
fn update_file_source(id: u32, text: String, state: State<'_, AppState>) -> Result<(), String> {
    edit_file_source(&state, id, &text)
}

/// The journal's files — the root journal first, then everything it includes —
/// for the new-entry form's file picker.
#[tauri::command]
#[specta::specta]
fn journal_files(state: State<'_, AppState>) -> Vec<JournalFile> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    guard.as_ref().map(journal_file_views).unwrap_or_default()
}

/// The journal file a new transaction should go into, given the accounts it
/// mentions — the new-entry form's default, so the form can preselect a file
/// before the user has to think about one. `None` only when no ledger is loaded.
#[tauri::command]
#[specta::specta]
fn suggest_transaction_file(accounts: Vec<String>, state: State<'_, AppState>) -> Option<String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref()?;
    let index = default_file(&FILE_SUGGESTION, &loaded.ledger, &accounts)?;
    let file = loaded.ledger.source_map.files().get(index)?;
    Some(file.path.display().to_string())
}

/// Append a new transaction to one of the journal's files, through the same
/// pipeline as every other edit: reload the ledger and refuse the write (putting
/// the file back byte-for-byte) if the reload fails or raises an error the
/// ledger did not already have, and stage the file on success.
#[tauri::command]
#[specta::specta]
fn add_transaction(entry: NewTransaction, state: State<'_, AppState>) -> Result<(), String> {
    create_transaction(&state, entry)
}

/// Every transaction one journal file writes, newest first, each carrying all of
/// its postings — the file view's transaction mode. The rows are the register's
/// own, ids included, so an entry opened from here edits and stages exactly as
/// it does from an account page.
#[tauri::command]
#[specta::specta]
fn file_transactions(file: String, state: State<'_, AppState>) -> Result<Vec<TransactionView>, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger loaded")?;
    file_transaction_views(&loaded.ledger, &loaded.baseline, &file)
}

/// A journal file's whole source text — the file view's text mode reads the file
/// as it is on disk, from the copy this session holds.
#[tauri::command]
#[specta::specta]
fn file_text(file: String, state: State<'_, AppState>) -> Result<String, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger loaded")?;
    Ok(source_of_file(&loaded.ledger, &file)?.source.to_string())
}

/// Replace a journal file's whole source text — the text mode's way out —
/// through the same pipeline as every other edit: the loader is the authority on
/// whether the text is a legal ledger, a refusal puts the file back
/// byte-for-byte, and a success stages it for a later commit.
#[tauri::command]
#[specta::specta]
fn update_file_text(file: String, text: String, state: State<'_, AppState>) -> Result<(), String> {
    edit_named_file_source(&state, &file, &text)
}

/// Append an `open` directive to one of the journal's files — the new-account
/// form's way in — through the same reload, refusal and staging pipeline as
/// every other edit.
#[tauri::command]
#[specta::specta]
fn add_account(account: NewAccount, state: State<'_, AppState>) -> Result<(), String> {
    create_account(&state, account)
}

/// The journal file a new account should be opened in, given its name — the
/// new-account form's default. `None` only when no ledger is loaded.
#[tauri::command]
#[specta::specta]
fn suggest_account_file(name: String, state: State<'_, AppState>) -> Option<String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref()?;
    let index = default_file(&ACCOUNT_SUGGESTION, &loaded.ledger, &[name])?;
    let file = loaded.ledger.source_map.files().get(index)?;
    Some(file.path.display().to_string())
}

/// Unified diff of the ledger's files against HEAD, index and working tree
/// together, for the diff view: git's own patch text under a `=== path ===`
/// header per changed file.
#[tauri::command]
#[specta::specta]
fn ledger_diff(state: State<'_, AppState>) -> Result<String, String> {
    ledger_diff_text(&state)
}

/// Commit the ledger's staged changes; returns the new commit's short id.
#[tauri::command]
#[specta::specta]
fn commit_ledger(message: String, state: State<'_, AppState>) -> Result<String, String> {
    commit_staged(&state, &message)
}

/// Collect the transactions whose postings touch `account` or one of its
/// sub-accounts, newest first (stable, so equal dates keep ledger order), each
/// with every posting it contains. The `enumerate` index over `balance_view`
/// is the transaction's position in the ledger stream, a stable id for a file.
/// An empty `account` matches every transaction — the virtual root.
fn account_transaction_views(
    ledger: &Ledger,
    baseline: &HashMap<PathBuf, Arc<str>>,
    account: &str,
) -> Vec<TransactionView> {
    let view = ledger.balance_view();
    // Flagging rows costs a git diff plus a walk of the directive stream; both
    // are skipped when the working tree has nothing to report — no repository,
    // no change against HEAD or the session baseline — and every row then reads
    // as unmodified.
    let changed = working_tree_changes(ledger, baseline);
    let sources = (!changed.is_empty()).then(|| source_directive_index(ledger, &view));
    let mut line_starts: HashMap<u16, Vec<usize>> = HashMap::new();
    let mut views: Vec<TransactionView> = view
        .iter()
        .enumerate()
        .filter_map(|(id, directive)| {
            let Directive::Transaction(tx) = directive else {
                return None;
            };
            if !registerable(tx) {
                return None;
            }
            let touches = tx
                .postings
                .iter()
                .any(|posting| mentions_account(posting.value.account.as_str(), account));
            if !touches {
                return None;
            }
            Some(transaction_view(
                ledger,
                &changed,
                sources.as_deref(),
                &mut line_starts,
                id,
                tx,
            ))
        })
        .collect();
    // Stable sort: descending date, ties keep their ledger order.
    views.sort_by(|a, b| b.date.cmp(&a.date));
    views
}

/// Every transaction the file `file` itself writes, newest first, each carrying
/// all of its postings — the file view's transaction mode.
///
/// An entry belongs to a file only through the directive it came from, so the
/// walk that pairs the ledger's two streams is needed here however little has
/// changed. Synthesized entries — a `pad`'s transaction, a plugin's — belong to
/// no file and are never among them.
fn file_transaction_views(
    ledger: &Ledger,
    baseline: &HashMap<PathBuf, Arc<str>>,
    file: &str,
) -> Result<Vec<TransactionView>, String> {
    let found = source_of_file(ledger, file)?;
    let wanted = u16::try_from(found.id)
        .map_err(|_| "the journal has more files than an entry can name".to_string())?;
    let view = ledger.balance_view();
    let changed = working_tree_changes(ledger, baseline);
    let sources = source_directive_index(ledger, &view);
    let mut line_starts: HashMap<u16, Vec<usize>> = HashMap::new();
    let mut views: Vec<TransactionView> = view
        .iter()
        .enumerate()
        .filter_map(|(id, directive)| {
            let Directive::Transaction(tx) = directive else {
                return None;
            };
            if !registerable(tx) {
                return None;
            }
            let index = sources.get(id).copied().flatten()?;
            if ledger.directives[index].file_id != wanted {
                return None;
            }
            Some(transaction_view(
                ledger,
                &changed,
                Some(&sources),
                &mut line_starts,
                id,
                tx,
            ))
        })
        .collect();
    // Stable sort: descending date, ties keep their ledger order.
    views.sort_by(|a, b| b.date.cmp(&a.date));
    Ok(views)
}

/// Whether a transaction is one a register shows. The plugin's generated entries
/// are bookkeeping, not entries the user wrote, so they never reach it — the
/// check both register scopes share.
fn registerable(tx: &Transaction) -> bool {
    !tx.postings
        .iter()
        .all(|posting| is_envelope_account(posting.value.account.as_str()))
}

/// One transaction of the ledger stream as a register row: every posting it
/// contains, and how its own source lines stand against the last commit.
fn transaction_view(
    ledger: &Ledger,
    changed: &HashMap<u16, FileChange>,
    sources: Option<&[Option<usize>]>,
    line_starts: &mut HashMap<u16, Vec<usize>>,
    id: usize,
    tx: &Transaction,
) -> TransactionView {
    let postings: Vec<PostingView> = tx
        .postings
        .iter()
        .filter_map(|posting| {
            let Some(IncompleteAmount::Complete(amount)) = &posting.value.units else {
                // Residual/interpolated postings never occur post-booking.
                return None;
            };
            Some(PostingView {
                account: posting.value.account.as_str().to_owned(),
                units: amount.number.to_string(),
                currency: amount.currency.as_str().to_owned(),
            })
        })
        .collect();
    let change = sources
        .and_then(|sources| sources.get(id).copied().flatten())
        .map_or(Change::Unchanged, |index| {
            directive_change(ledger, changed, line_starts, &ledger.directives[index])
        });
    TransactionView {
        id: u32::try_from(id).unwrap_or(u32::MAX),
        date: tx.date.to_string(),
        flag: tx.flag.to_string(),
        payee: tx.payee.as_ref().map(|p| p.as_str().to_owned()),
        narration: tx.narration.as_str().to_owned(),
        postings,
        change,
    }
}

/// The journal file the loader holds under the path `file` — the one lookup
/// every command that names a file starts with, so an unknown path fails the
/// same way whichever one asked.
fn source_of_file<'a>(ledger: &'a Ledger, file: &str) -> Result<&'a SourceFile, String> {
    ledger
        .source_map
        .get_by_path(Path::new(file))
        .ok_or_else(|| format!("`{file}` is not a file of this journal"))
}

/// Accumulate per-(account, currency) units over the pad-merged transaction
/// stream, roll each account's totals up into its ancestors, then report one
/// entry per tree node.
fn account_balances(ledger: &Ledger, changes: &BTreeMap<String, Change>) -> Vec<AccountBalance> {
    let mut totals: HashMap<String, HashMap<String, Decimal>> = HashMap::new();
    // Every account the ledger opens belongs in the tree, balance or not: an
    // account opened and not yet posted to is exactly the one a user is about to
    // use, and leaving it out made "the account is open" invisible. An account
    // that does have postings ignores this — the seed is only for the ones the
    // postings never mention — so the commodity an account holds is still the one
    // its postings put there.
    for directive in &ledger.directives {
        let Directive::Open(open) = &directive.value else {
            continue;
        };
        let account = open.account.as_str();
        if is_envelope_account(account) {
            continue;
        }
        totals.entry(account.to_owned()).or_insert_with(|| {
            open.currencies
                .iter()
                .map(|currency| (currency.as_str().to_owned(), Decimal::ZERO))
                .collect()
        });
    }
    for directive in ledger.balance_view() {
        let Directive::Transaction(tx) = directive else {
            continue;
        };
        for posting in &tx.postings {
            let Some(IncompleteAmount::Complete(amount)) = &posting.value.units else {
                // Residual/interpolated postings never occur post-booking.
                continue;
            };
            // The envelope plugin's postings are bookkeeping against accounts it
            // owns; letting them through would also inflate their ancestors, so
            // `Equity` would carry money the user does not have.
            if is_envelope_account(posting.value.account.as_str()) {
                continue;
            }
            *totals
                .entry(posting.value.account.as_str().to_owned())
                .or_default()
                .entry(amount.currency.as_str().to_owned())
                .or_default() += amount.number;
        }
    }
    // Roll every account's own totals into each colon-prefix ancestor (the
    // final prefix re-adds the account's own totals, which `drain` removed).
    // Adding to a parent that also has postings is correct: its entry then
    // holds own + descendants, exactly the subtree total.
    for (name, currencies) in totals.drain().collect::<Vec<_>>() {
        let mut prefix = String::new();
        for part in name.split(':') {
            prefix = if prefix.is_empty() {
                part.to_owned()
            } else {
                format!("{prefix}:{part}")
            };
            let node = totals.entry(prefix.clone()).or_default();
            for (currency, units) in &currencies {
                *node.entry(currency.clone()).or_default() += units;
            }
        }
    }
    let mut balances: Vec<AccountBalance> = totals
        .iter()
        .map(|(name, held)| {
            let (currency, units) = primary_holding(held, &ledger.options.operating_currency);
            AccountBalance {
                name: name.clone(),
                balance: units.to_string(),
                currency,
                change: changes.get(name).copied().unwrap_or(Change::Unchanged),
            }
        })
        .collect();
    balances.sort_by(|a, b| a.name.cmp(&b.name));
    balances
}

/// An account's primary commodity and its units in it: the first operating
/// currency the account holds, else the commodity with the largest absolute
/// position.
fn primary_holding(held: &HashMap<String, Decimal>, operating: &[String]) -> (String, Decimal) {
    for currency in operating {
        if let Some(units) = held.get(currency) {
            return (currency.clone(), *units);
        }
    }
    held.iter()
        .max_by_key(|(_, units)| units.abs())
        .map(|(currency, units)| (currency.clone(), *units))
        .unwrap_or_default()
}

/// The index into `ledger.directives` of the source directive behind each entry
/// of `ledger.balance_view()`.
///
/// `balance_view` hands out bare `Directive`s, so a transaction's source
/// location has to be recovered from the directive stream. The two are the same
/// stream — both sorted by date — with synthesized pad transactions inserted, so
/// walking them in lockstep and advancing the source stream only on a match
/// pairs every entry of the view with the directive it came from.
fn source_directive_index(ledger: &Ledger, view: &[Directive]) -> Vec<Option<usize>> {
    let mut sources = Vec::with_capacity(view.len());
    let mut next = 0;
    for directive in view {
        let matched = ledger
            .directives
            .get(next)
            .filter(|source| source.value == *directive)
            .map(|_| next);
        if matched.is_some() {
            next += 1;
        }
        sources.push(matched);
    }
    sources
}

/// The transaction behind a `TransactionView.id` — the entry at `id` in the
/// ledger's transaction stream — together with its source span and the file it
/// was loaded from. Every edit path starts by locating its transaction this
/// way, so an unknown id fails the same way whichever editor asked.
fn source_of_transaction<'a>(
    ledger: &'a Ledger,
    id: u32,
) -> Result<(&'a Spanned<Directive>, &'a Transaction, &'a SourceFile), String> {
    let view = ledger.balance_view();
    let sources = source_directive_index(ledger, &view);
    let source = sources
        .get(id as usize)
        .copied()
        .flatten()
        .map(|index| &ledger.directives[index])
        .ok_or_else(|| format!("no transaction with id {id}"))?;
    let Directive::Transaction(tx) = &source.value else {
        return Err(format!("directive {id} is not a transaction"));
    };
    let file = ledger
        .source_map
        .get(source.file_id as usize)
        .ok_or_else(|| format!("no source file for transaction {id}"))?;
    Ok((source, tx, file))
}

/// The body of [`update_transaction`], split out so it can be driven without a
/// running Tauri app.
fn edit_transaction(state: &AppState, edit: TransactionEdit) -> Result<(), String> {
    // Read-only pass: work out the new file text from the loaded copy.
    let (path, previous, rewritten, reported) = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().ok_or("no ledger loaded")?;
        let ledger = &loaded.ledger;
        let (source, tx, file) = source_of_transaction(ledger, edit.id)?;
        let previous = file.source.to_string();
        let rewritten = rewrite_transaction(&previous, source.span, tx, &edit)?;
        (
            file.path.clone(),
            previous,
            rewritten,
            error_signatures(&ledger.errors),
        )
    };

    apply_file_edit(state, &path, &previous, rewritten, reported)
}

/// The body of [`transaction_source`], split out like [`edit_transaction`]: the
/// transaction's own lines exactly as they stand in the loaded copy of its file.
fn transaction_source_text(state: &AppState, id: u32) -> Result<String, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger loaded")?;
    let (source, _, file) = source_of_transaction(&loaded.ledger, id)?;
    file.source
        .get(source.span.start..source.span.end)
        .map(str::to_string)
        .ok_or_else(|| format!("transaction {id} is outside its file"))
}

/// The body of [`update_transaction_source`]: the transaction's span has
/// `text` spliced over it, and the result goes through the same pipeline as
/// [`edit_transaction`] — the loader, not this function, is the authority on
/// whether arbitrary text is a legal transaction.
fn edit_transaction_source(state: &AppState, id: u32, text: &str) -> Result<(), String> {
    let (path, previous, rewritten, reported) = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().ok_or("no ledger loaded")?;
        let ledger = &loaded.ledger;
        let (source, _, file) = source_of_transaction(ledger, id)?;
        let previous = file.source.to_string();
        let (Some(head), Some(tail)) =
            (previous.get(..source.span.start), previous.get(source.span.end..))
        else {
            return Err(format!("transaction {id} is outside its file"));
        };
        let mut rewritten = String::with_capacity(previous.len() + text.len());
        rewritten.push_str(head);
        rewritten.push_str(text);
        rewritten.push_str(tail);
        (
            file.path.clone(),
            previous,
            rewritten,
            error_signatures(&ledger.errors),
        )
    };

    apply_file_edit(state, &path, &previous, rewritten, reported)
}

/// The body of [`transaction_file`]: the file the entry came from, its whole
/// text, and the entry's own position in it (1-based line, UTF-16 column).
fn transaction_file_view(state: &AppState, id: u32) -> Result<TransactionFile, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger loaded")?;
    let (source, _, file) = source_of_transaction(&loaded.ledger, id)?;
    let (start_line, start_column) = line_column(&file.source, source.span.start);
    let (end_line, end_column) = line_column(&file.source, source.span.end);
    Ok(TransactionFile {
        path: file.path.display().to_string(),
        text: file.source.to_string(),
        start_line,
        start_column,
        end_line,
        end_column,
    })
}

/// The body of [`update_file_source`]: the whole file text replaces the loaded
/// copy, and the result goes through the same pipeline as
/// [`edit_transaction_source`] — [`apply_file_edit`] reloads, reverts on a new
/// loader error, and stages what survives.
fn edit_file_source(state: &AppState, id: u32, text: &str) -> Result<(), String> {
    let (path, previous, reported) = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().ok_or("no ledger loaded")?;
        let ledger = &loaded.ledger;
        // The entry is only how the editor knows which file it is editing; it
        // need not still exist in `text`, the file is the unit being written.
        let (_, _, file) = source_of_transaction(ledger, id)?;
        (
            file.path.clone(),
            file.source.to_string(),
            error_signatures(&ledger.errors),
        )
    };

    apply_file_edit(state, &path, &previous, text.to_string(), reported)
}

/// The body of [`add_transaction`], split out like [`edit_file_source`] so it
/// can be driven without a running Tauri app: work out the file's new text from
/// the loaded copy, then take the same reload-and-stage path as every edit.
fn create_transaction(state: &AppState, entry: NewTransaction) -> Result<(), String> {
    let (path, previous, rewritten, reported) = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().ok_or("no ledger loaded")?;
        let ledger = &loaded.ledger;
        // The entry names the file it goes into; a path the journal does not
        // hold — a stale picker, a hand-written payload — is refused rather than
        // written to, so this command can only ever touch the ledger's own
        // files.
        let file = ledger
            .source_map
            .get_by_path(Path::new(&entry.file))
            .ok_or_else(|| format!("`{}` is not a file of this journal", entry.file))?;
        let previous = file.source.to_string();
        let rewritten = append_transaction(&previous, ledger, &entry)?;
        (
            file.path.clone(),
            previous,
            rewritten,
            error_signatures(&ledger.errors),
        )
    };

    apply_file_edit(state, &path, &previous, rewritten, reported)
}

/// The journal's files, as the explorer and the new-entry picker show them: in
/// the loader's own order, which is the root journal first, then what it
/// includes.
fn journal_file_views(loaded: &Loaded) -> Vec<JournalFile> {
    let ledger = &loaded.ledger;
    let files = ledger.source_map.files();
    // The loader resolves everything a journal mentions relative to the root
    // journal's directory (its own `base_dir` rule, shared by `include` and
    // `option "documents"`), so that is the directory a label is relative to.
    let base = files.first().and_then(|file| file.path.parent());
    // One walk of the transaction stream, counted per file — the same pairing the
    // register uses to know which file an entry came from. An entry the loader
    // synthesized has no file, so a `pad`'s transaction or a plugin's entries are
    // no file's.
    let view = ledger.balance_view();
    let sources = source_directive_index(ledger, &view);
    let mut counts = vec![0u32; files.len()];
    for (id, directive) in view.iter().enumerate() {
        if !matches!(directive, Directive::Transaction(_)) {
            continue;
        }
        let Some(index) = sources.get(id).copied().flatten() else {
            continue;
        };
        let file = ledger.directives[index].file_id as usize;
        if let Some(count) = counts.get_mut(file) {
            *count = count.saturating_add(1);
        }
    }
    let changed = working_tree_changes(ledger, &loaded.baseline);
    files
        .iter()
        .enumerate()
        .map(|(id, file)| JournalFile {
            path: file.path.display().to_string(),
            label: base
                .and_then(|base| file.path.strip_prefix(base).ok())
                .filter(|rest| !rest.as_os_str().is_empty())
                .unwrap_or(file.path.as_path())
                .display()
                .to_string(),
            transactions: counts[id],
            change: u16::try_from(id)
                .ok()
                .and_then(|id| changed.get(&id))
                .map_or(Change::Unchanged, FileChange::file),
        })
        .collect()
}

/// Picks the journal file a new transaction defaults to.
///
/// A strategy sees the loaded ledger and the accounts already typed, and answers
/// with a file index; the form only ever asks [`suggest_transaction_file`], so
/// which rule answers it — this one, a smarter one, or one backed by a model —
/// is invisible to the form and to the command that writes the entry.
trait FileSuggestion {
    /// The index into `ledger.source_map.files()` a new entry belongs in, or
    /// `None` when this strategy has nothing to say about these accounts.
    fn suggest(&self, ledger: &Ledger, accounts: &[String]) -> Option<usize>;
}

/// The rule the app ships with: entries live next to their neighbours, so the
/// file of the newest entry that already touches one of the accounts — on the
/// account itself, or anywhere on its branch of the tree — is where a new one
/// goes.
struct RecentAccountActivity;

impl FileSuggestion for RecentAccountActivity {
    fn suggest(&self, ledger: &Ledger, accounts: &[String]) -> Option<usize> {
        if accounts.is_empty() {
            return None;
        }
        let view = ledger.balance_view();
        let sources = source_directive_index(ledger, &view);
        let mut suggested = None;
        for (id, directive) in view.iter().enumerate() {
            let Directive::Transaction(tx) = directive else {
                continue;
            };
            let touches = tx.postings.iter().any(|posting| {
                accounts
                    .iter()
                    .any(|account| same_branch(posting.value.account.as_str(), account))
            });
            if !touches {
                continue;
            }
            // `balance_view` is in date order, so the last match on the walk is
            // the newest entry. One the loader synthesized — a `pad`'s
            // transaction, a plugin's generated entries — has no file of its own
            // and cannot answer.
            let Some(index) = sources.get(id).copied().flatten() else {
                continue;
            };
            suggested = Some(ledger.directives[index].file_id as usize);
        }
        suggested
    }
}

/// The rule for a new account: it is opened where its family already lives.
///
/// A new account has no entries of its own to follow, so the answer comes from
/// the name's ancestors, nearest first — the file that opens one of them, and
/// failing that the file of the newest entry on an ancestor's branch. A ledger
/// that opens all its accounts in one file answers with that file for everything
/// under it, which is exactly right; a ledger opens `Expenses:*` together, and
/// follows.
struct AccountFamily;

impl FileSuggestion for AccountFamily {
    fn suggest(&self, ledger: &Ledger, accounts: &[String]) -> Option<usize> {
        let name = accounts.first()?.trim();
        if name.is_empty() {
            return None;
        }
        // `Expenses:Food` before `Expenses` for `Expenses:Food:Groceries`.
        let mut ancestors: Vec<&str> = Vec::new();
        let mut end = name.len();
        while let Some(at) = name[..end].rfind(':') {
            ancestors.push(&name[..at]);
            end = at;
        }
        for ancestor in &ancestors {
            let opened = ledger.directives.iter().position(|source| {
                matches!(&source.value, Directive::Open(open) if open.account.as_str() == *ancestor)
            });
            if let Some(index) = opened {
                return Some(ledger.directives[index].file_id as usize);
            }
        }
        for ancestor in &ancestors {
            if let Some(index) = RecentAccountActivity.suggest(ledger, &[(*ancestor).to_string()]) {
                return Some(index);
            }
        }
        None
    }
}

/// The strategy the app asks today for a new transaction. Swapping the default
/// rule — for a smarter one, or for an AI-backed one — is a change to this line
/// and nothing else.
const FILE_SUGGESTION: RecentAccountActivity = RecentAccountActivity;

/// The strategy the app asks today for a new account.
const ACCOUNT_SUGGESTION: AccountFamily = AccountFamily;

/// The file index a new entry or account defaults to: what the strategy answers,
/// else the root journal. A new entry always has somewhere to go, and the file
/// every other one hangs off is the journal it belongs to.
fn default_file(
    strategy: &dyn FileSuggestion,
    ledger: &Ledger,
    accounts: &[String],
) -> Option<usize> {
    let count = ledger.source_map.files().len();
    match strategy.suggest(ledger, accounts) {
        Some(index) if index < count => Some(index),
        _ if count > 0 => Some(0),
        _ => None,
    }
}

/// Whether `name` is `account` itself or an account below it.
fn is_under(name: &str, account: &str) -> bool {
    name == account
        || (name.len() > account.len()
            && name.starts_with(account)
            && name.as_bytes()[account.len()] == b':')
}

/// Whether a posting's `name` is one the register for `account` lists: the
/// account itself or anything under it. An empty `account` is the virtual root,
/// which lists everything.
fn mentions_account(name: &str, account: &str) -> bool {
    account.is_empty() || is_under(name, account)
}

/// Whether `name` and `account` lie on the same branch of the account tree:
/// either is the other, or one sits under the other. Which file an entry belongs
/// in is read that way round on both sides — an entry on `Expenses:Food` says
/// where a new entry on `Expenses:Food:Groceries` goes, even though the
/// register's own one-way filter would not have listed it there.
fn same_branch(name: &str, account: &str) -> bool {
    is_under(name, account) || is_under(account, name)
}

/// `text` with `entry` appended to the end of the file as a new transaction.
fn append_transaction(
    text: &str,
    ledger: &Ledger,
    entry: &NewTransaction,
) -> Result<String, String> {
    let rendered = render_transaction(text, ledger, entry)?;
    Ok(append_directive(text, &rendered))
}

/// `text` with `rendered` appended to the end of the file.
///
/// Only bytes are added: one blank line separates the directive from whatever
/// the file already ends with, and not a byte of what was there is rewritten, so
/// the change the diff view shows is the directive itself.
fn append_directive(text: &str, rendered: &str) -> String {
    let mut rewritten = String::with_capacity(text.len() + rendered.len() + 2);
    rewritten.push_str(text);
    if !text.is_empty() {
        if !text.ends_with('\n') {
            rewritten.push_str("\n\n");
        } else if !text.ends_with("\n\n") {
            rewritten.push('\n');
        }
    }
    rewritten.push_str(rendered);
    rewritten
}

/// The body of [`add_account`], split out like [`create_transaction`] so it can
/// be driven without a running Tauri app.
fn create_account(state: &AppState, account: NewAccount) -> Result<(), String> {
    let (path, previous, rewritten, reported) = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().ok_or("no ledger loaded")?;
        let file = source_of_file(&loaded.ledger, &account.file)?;
        let previous = file.source.to_string();
        let rewritten = append_directive(&previous, &render_open(&account)?);
        (
            file.path.clone(),
            previous,
            rewritten,
            error_signatures(&loaded.ledger.errors),
        )
    };

    apply_file_edit(state, &path, &previous, rewritten, reported)
}

/// The text of `account` as the `open` directive it becomes: the day, the
/// account, and the commodities it may hold — none written when none were given,
/// which is beancount's "any commodity".
///
/// Only the tokens an `open` directive is made of are written, from a name and
/// commodities that cannot split the line or open a comment, so nothing typed
/// into the form can land in the file as syntax of its own. Whether the name is
/// an account, and whether it is already open, is the loader's to say — and it
/// says it before anything is staged.
fn render_open(account: &NewAccount) -> Result<String, String> {
    let date = checked_date(&account.date)?;
    let name = account.name.trim();
    if name.is_empty() || name.chars().any(|c| c.is_whitespace() || c == '"' || c == ';') {
        return Err(format!("`{name}` is not an account name"));
    }
    let mut rendered = format!("{date} open {name}");
    let mut commodities = Vec::new();
    for commodity in &account.currencies {
        let commodity = commodity.trim();
        if commodity.is_empty() {
            continue;
        }
        if commodity
            .chars()
            .any(|c| c.is_whitespace() || c == ',' || c == '"' || c == ';')
        {
            return Err(format!("`{commodity}` is not a commodity"));
        }
        commodities.push(commodity);
    }
    if !commodities.is_empty() {
        rendered.push(' ');
        rendered.push_str(&commodities.join(", "));
    }
    rendered.push('\n');
    Ok(rendered)
}

/// The body of [`update_file_text`]: a journal file's whole text replaces the
/// loaded copy, and the result goes through the same pipeline as
/// [`edit_file_source`] — [`apply_file_edit`] reloads, reverts on a new loader
/// error, and stages what survives.
fn edit_named_file_source(state: &AppState, file: &str, text: &str) -> Result<(), String> {
    let (path, previous, reported) = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().ok_or("no ledger loaded")?;
        let found = source_of_file(&loaded.ledger, file)?;
        (
            found.path.clone(),
            found.source.to_string(),
            error_signatures(&loaded.ledger.errors),
        )
    };

    apply_file_edit(state, &path, &previous, text.to_string(), reported)
}

/// The text of `entry`, laid out the way the file it is going into lays its own
/// postings out: the header, then one line per posting, each account padded to
/// the column the file's postings put their amounts at.
///
/// Only the tokens beancount reads are written, from values that have been
/// validated or quoted on the way in, so nothing typed into the form can land in
/// the file as syntax of its own.
fn render_transaction(
    text: &str,
    ledger: &Ledger,
    entry: &NewTransaction,
) -> Result<String, String> {
    let date = checked_date(&entry.date)?;
    let flag = checked_flag(&entry.flag)?;
    let payee = entry
        .payee
        .as_deref()
        .map(str::trim)
        .filter(|payee| !payee.is_empty());
    let narration = entry.narration.trim();
    let mut rendered = String::new();
    rendered.push_str(&date);
    rendered.push(' ');
    rendered.push_str(flag);
    // A payee that only repeats the narration is not written down twice — the
    // same rule the register's header rewrite follows.
    if let Some(payee) = payee.filter(|payee| *payee != narration) {
        rendered.push(' ');
        rendered.push_str(&quote_beancount(payee));
    }
    rendered.push(' ');
    rendered.push_str(&quote_beancount(narration));

    let column = amount_column(text)
        // A file with no posting to take a column from — a root journal of
        // `open`s and `include`s, say — still gets postings that line up with
        // each other: the entry is its own convention then.
        .unwrap_or_else(|| 2 + longest_account(entry) + 1);
    for (position, posting) in entry.postings.iter().enumerate() {
        let account = posting.account.trim();
        if account.is_empty() {
            return Err(format!("posting {} has no account", position + 1));
        }
        rendered.push('\n');
        rendered.push_str("  ");
        rendered.push_str(account);
        let figure = match posting
            .units
            .as_deref()
            .map(str::trim)
            .filter(|units| !units.is_empty())
        {
            // An amount left blank is the leg beancount interpolates: the
            // account stands alone, the way a hand-written ledger leaves it.
            None => continue,
            Some(units) => Decimal::from_str(units)
                .map_err(|e| format!("`{units}` is not an amount: {e}"))?
                .to_string(),
        };
        let currency = posting
            .currency
            .as_deref()
            .map(str::trim)
            .filter(|currency| !currency.is_empty())
            .or_else(|| ledger.options.operating_currency.first().map(String::as_str))
            .ok_or("the ledger declares no `operating_currency` to write amounts in")?;
        // Pad out to the file's own amount column, never below one space, so an
        // account longer than the file's widest cannot touch its figure.
        rendered.extend(std::iter::repeat_n(' ', column.saturating_sub(2 + account.len()).max(1)));
        rendered.push_str(&figure);
        rendered.push(' ');
        rendered.push_str(currency);
    }
    rendered.push('\n');
    Ok(rendered)
}

/// The longest account `entry` posts to, in bytes — the width a file with no
/// postings of its own gets its new entry aligned to.
fn longest_account(entry: &NewTransaction) -> usize {
    entry
        .postings
        .iter()
        .map(|posting| posting.account.trim().len())
        .max()
        .unwrap_or(0)
}

/// The column the file's own postings put their amounts at: the one most of them
/// agree on, so a new entry lines up with the entries around it. `None` when the
/// file has no posting with a figure yet.
fn amount_column(text: &str) -> Option<usize> {
    let mut columns: BTreeMap<usize, usize> = BTreeMap::new();
    for line in text.lines() {
        if let Some(column) = posting_amount_column(line) {
            *columns.entry(column).or_default() += 1;
        }
    }
    // The most common column wins; a tie goes to the leftmost, so an entry never
    // drifts further right than the file already puts its own amounts.
    columns
        .into_iter()
        .max_by_key(|&(column, count)| (count, std::cmp::Reverse(column)))
        .map(|(column, _)| column)
}

/// The byte column a posting line's amount figure starts at, when the line is
/// one of the file's own postings: indented, an account name, then a figure.
///
/// A posting the file leaves without an amount has no figure to align to, and
/// neither has anything else on the line: an account name starts with a capital
/// and never ends with a colon, which is what keeps a metadata line's
/// `key: value` from reading as one.
fn posting_amount_column(line: &str) -> Option<usize> {
    let indent = line.len() - line.trim_start().len();
    if indent == 0 {
        return None;
    }
    let rest = &line[indent..];
    let account_end = rest.find(char::is_whitespace)?;
    let account = &rest[..account_end];
    let named = account.chars().next()?.is_ascii_uppercase()
        && account.contains(':')
        && !account.ends_with(':')
        && account
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, ':' | '-' | '_'));
    if !named {
        return None;
    }
    let value = &rest[account_end..];
    let gap = value.len() - value.trim_start().len();
    value
        .trim_start()
        .starts_with(|character: char| character.is_ascii_digit() || matches!(character, '-' | '+'))
        .then_some(indent + account_end + gap)
}

/// The 1-based line and UTF-16 column of `byte` in `text` — Monaco's own
/// coordinates. A byte offset that is not a character boundary (or past the
/// end) answers for the largest boundary before it, so a stale offset can never
/// panic.
fn line_column(text: &str, byte: usize) -> (u32, u32) {
    let head = text.get(..byte.min(text.len())).unwrap_or(text);
    let line = head.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
    let line_start = head.rfind('\n').map(|index| index + 1).unwrap_or(0);
    let column = text[line_start..head.len()].chars().map(char::len_utf16).sum::<usize>() as u32 + 1;
    (line, column)
}

/// Write a transaction edit's `rewritten` file text over the loaded copy at
/// `path`, reload the ledger from disk, and stage the file — the shared tail of
/// both edit paths (field-by-field and source text).
///
/// The reload is the authority on whether the edit was legal (a balanced
/// transaction, a parseable header): when it fails or raises an error the
/// ledger did not already have, the file is put back exactly as it was and the
/// loader's complaint is returned, so an edit can never leave a broken ledger
/// on disk. A ledger outside a git repository is still written, there is simply
/// nothing to stage.
fn apply_file_edit(
    state: &AppState,
    path: &Path,
    previous: &str,
    rewritten: String,
    reported: Vec<String>,
) -> Result<(), String> {
    if rewritten == previous {
        // The edit describes the file exactly as it already stands.
        return Ok(());
    }

    // The offsets came from the loaded copy; a file rewritten under us (an
    // editor write, a checkout) would be clobbered by spans that no longer line
    // up with it.
    let previous_bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if std::str::from_utf8(&previous_bytes).ok() != Some(previous) {
        return Err(format!(
            "{} changed on disk since it was loaded; reload the ledger",
            path.display()
        ));
    }

    // Reload the journal this session was opened with, not the file just
    // written: an included file on its own has none of the `open` directives
    // its parent carries, so reloading it alone would report every account it
    // touches as never opened and refuse a perfectly good edit.
    let root = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        guard
            .as_ref()
            .map(|loaded| loaded.root.clone())
            .unwrap_or_else(|| path.to_path_buf())
    };

    fs::write(path, rewritten.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
    let reloaded = match rustledger_loader::load(&root, &LoadOptions::default()) {
        Ok(ledger) => ledger,
        Err(e) => {
            let _ = fs::write(path, &previous_bytes);
            return Err(format!("{}: {e}", root.display()));
        }
    };
    let raised = new_errors(&reported, error_signatures(&reloaded.errors));
    if !raised.is_empty() {
        let _ = fs::write(path, &previous_bytes);
        return Err(raised.join("\n"));
    }

    // The reload is the ledger the app now holds; the baseline keeps what the
    // session started from, so this edit reads as a change.
    let mut guard = state.loaded.write().unwrap_or_else(|e| e.into_inner());
    match guard.as_mut() {
        Some(loaded) => {
            loaded.ledger = reloaded;
            loaded.root = root;
            loaded.remember_files();
        }
        None => *guard = Some(Loaded::fresh(reloaded, root)),
    }
    stage_file(path)
}

/// The body of [`ledger_diff`]: the ledger's files against HEAD.
fn ledger_diff_text(state: &AppState) -> Result<String, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger loaded")?;
    let ledger = &loaded.ledger;
    let (repo, _) = ledger_repo(ledger).ok_or("the ledger is not inside a git repository")?;
    let mut options = git2::DiffOptions::new();
    // Only the ledger's own files: the repository around it may hold anything.
    for file in ledger.source_map.files() {
        if let Some((owner, relative)) = repo_and_relative(&file.path) {
            if owner.path() == repo.path() {
                if let Some(spec) = relative.to_str() {
                    options.pathspec(spec);
                }
            }
        }
    }
    options.include_untracked(true);
    let head = head_tree(&repo);
    let diff = repo
        .diff_tree_to_workdir_with_index(head.as_ref(), Some(&mut options))
        .map_err(|e| format!("git diff: {e}"))?;
    render_diff(&diff)
}

/// The body of [`commit_ledger`]: commit what the frontend staged of the
/// ledger's files, on top of HEAD (an empty repository has no parent).
///
/// Only the ledger's own paths go into the commit. A repository can hold
/// unrelated work — staged or not — and the frontend asked for the ledger's
/// changes, so the commit is built from a private index seeded with HEAD's tree
/// and overlaid with the ledger's staged entries. Everything else stays staged,
/// exactly as `git commit <paths>` leaves it.
fn commit_staged(state: &AppState, message: &str) -> Result<String, String> {
    if message.trim().is_empty() {
        return Err("commit message must not be empty".to_string());
    }
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger loaded")?;
    let ledger = &loaded.ledger;
    let (repo, _) = ledger_repo(ledger).ok_or("the ledger is not inside a git repository")?;
    let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
    let head = parent.as_ref().and_then(|commit| commit.tree().ok());

    let index = repo.index().map_err(|e| format!("git index: {e}"))?;
    let mut staged = git2::Index::new().map_err(|e| format!("git index: {e}"))?;
    if let Some(head) = &head {
        staged.read_tree(head).map_err(|e| format!("git index: {e}"))?;
    }
    let mut tracked = 0;
    for file in ledger.source_map.files() {
        let Some((owner, relative)) = repo_and_relative(&file.path) else {
            continue;
        };
        if owner.path() != repo.path() {
            continue;
        }
        if let Some(entry) = index.get_path(&relative, 0) {
            staged
                .add(&entry)
                .map_err(|e| format!("git index: {e}"))?;
            tracked += 1;
        }
    }
    // With no ledger file in it the tree is HEAD's own, message and all.
    if tracked == 0 {
        return Err("nothing staged to commit".to_string());
    }
    let tree_id = staged
        .write_tree_to(&repo)
        .map_err(|e| format!("git index: {e}"))?;
    if head.as_ref().is_some_and(|head| head.id() == tree_id) {
        return Err("nothing staged to commit".to_string());
    }
    let tree = repo.find_tree(tree_id).map_err(|e| format!("git tree: {e}"))?;
    // The user's own identity, exactly as git would use it.
    let signature = repo
        .signature()
        .map_err(|e| format!("git identity: {e} (set user.name and user.email)"))?;
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let commit = repo
        .commit(Some("HEAD"), &signature, &signature, message, &tree, &parents)
        .map_err(|e| format!("git commit: {e}"))?;
    Ok(commit.to_string().chars().take(7).collect())
}

/// The repository the ledger lives in, found by walking up from its files, so
/// the ledger directory itself does not have to sit at the repository's root.
fn ledger_repo(ledger: &Ledger) -> Option<(git2::Repository, PathBuf)> {
    ledger
        .source_map
        .files()
        .iter()
        .find_map(|file| repo_and_relative(&file.path))
}

/// A file's repository, found by walking up from the file, together with the
/// file's path relative to that repository's working directory (git2 only takes
/// repository-relative paths). `None` for a file outside any repository.
fn repo_and_relative(path: &Path) -> Option<(git2::Repository, PathBuf)> {
    let absolute = fs::canonicalize(path).ok()?;
    let repo = git2::Repository::discover(&absolute).ok()?;
    let workdir = fs::canonicalize(repo.workdir()?).ok()?;
    let relative = absolute.strip_prefix(&workdir).ok()?.to_path_buf();
    Some((repo, relative))
}

/// The repository's HEAD tree, or `None` while HEAD has no commit yet.
fn head_tree(repo: &git2::Repository) -> Option<git2::Tree<'_>> {
    repo.head().ok()?.peel_to_tree().ok()
}

/// The file's content at HEAD; `None` when HEAD does not have it — an untracked
/// file, or a repository whose first commit is still to come.
fn head_blob(repo: &git2::Repository, relative: &Path) -> Option<String> {
    let tree = head_tree(repo)?;
    let entry = tree.get_path(relative).ok()?;
    let blob = repo.find_blob(entry.id()).ok()?;
    String::from_utf8(blob.content().to_vec()).ok()
}

/// Stage a freshly written ledger file so a later commit picks it up. A ledger
/// outside version control is not a failure: the write already happened and
/// there is simply nothing to stage.
fn stage_file(path: &Path) -> Result<(), String> {
    let Some((repo, relative)) = repo_and_relative(path) else {
        return Ok(());
    };
    let mut index = repo.index().map_err(|e| format!("git index: {e}"))?;
    index
        .add_path(&relative)
        .map_err(|e| format!("git add {}: {e}", relative.display()))?;
    index.write().map_err(|e| format!("git index: {e}"))
}

/// Render a diff as one `=== path ===` block per changed file: git's own unified
/// patch text, which already carries the hunk headers, under a header the
/// frontend can split on.
fn render_diff(diff: &git2::Diff<'_>) -> Result<String, String> {
    let mut text = String::new();
    let mut current: Option<String> = None;
    diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
        let name = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        if current.as_deref() != Some(name.as_str()) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&format!("=== {name} ===\n"));
            current = Some(name);
        }
        // The callback is handed each line's text without its diff marker, so
        // put the marker back and leave the block reading as a patch the frontend
        // can colour line by line.
        match line.origin() {
            '+' | '-' | ' ' => text.push(line.origin()),
            _ => {}
        }
        text.push_str(&String::from_utf8_lossy(line.content()));
        true
    })
    .map_err(|e| format!("git diff: {e}"))?;
    Ok(text)
}

/// What one file's working copy holds against what it is measured against, by
/// kind of change: the lines it added, the lines that stand where a line the
/// reference had is gone, and whether the reference was the last commit at all.
struct FileChange {
    /// Lines that are new in the working copy, merged into inclusive ranges.
    added: Vec<(u32, u32)>,
    /// Lines that took the place of something the reference had.
    rewritten: Vec<(u32, u32)>,
    /// Whether the reference was HEAD's blob rather than the session's baseline:
    /// a file git does not have is new to git however it was edited.
    committed: bool,
}

impl FileChange {
    /// How the file itself stands: nothing, new to git, or committed and
    /// rewritten.
    fn file(&self) -> Change {
        if self.committed {
            Change::Modified
        } else {
            Change::Added
        }
    }
}

/// Which lines of the ledger's files differ from what they are measured
/// against, keyed by source-map file id — the measure behind
/// [`TransactionView::change`] and [`JournalFile::change`].
///
/// A file's reference is its HEAD blob when git has one, and otherwise the text
/// the session started with (see [`Loaded::baseline`]): a ledger that is not
/// committed yet is not a change in itself. Empty when the ledger is not in a
/// repository or nothing differs.
fn working_tree_changes(
    ledger: &Ledger,
    baseline: &HashMap<PathBuf, Arc<str>>,
) -> HashMap<u16, FileChange> {
    let mut changed: HashMap<u16, FileChange> = HashMap::new();
    for file in ledger.source_map.files() {
        let Some(id) = u16::try_from(file.id).ok() else {
            continue;
        };
        let Some((repo, relative)) = repo_and_relative(&file.path) else {
            continue;
        };
        let (lines, committed) = match head_blob(&repo, &relative) {
            // Committed: the working tree is compared against HEAD.
            Some(head) => {
                if head == *file.source {
                    continue;
                }
                (changed_lines(&head, &file.source), true)
            }
            // Not committed: compared against the session's starting point, so
            // an untracked ledger lights up only where it was edited.
            None => {
                let Some(start) = baseline.get(&file.path) else {
                    continue;
                };
                if start.as_ref() == file.source.as_ref() {
                    continue;
                }
                (changed_lines(start, &file.source), false)
            }
        };
        let (added, rewritten) = lines;
        if !added.is_empty() || !rewritten.is_empty() {
            changed.insert(
                id,
                FileChange {
                    added,
                    rewritten,
                    committed,
                },
            );
        }
    }
    changed
}

/// Which lines of `new` `old` does not have, split by kind and merged into
/// inclusive ranges: the lines that were added, and the lines that stand where a
/// line `old` had is gone. libgit2's own line diff drives this, so rewrites,
/// insertions and deletions are all covered; a deleted line has no number of its
/// own in the new file, so the line that takes its place is the one reported.
fn changed_lines(old: &str, new: &str) -> (Vec<(u32, u32)>, Vec<(u32, u32)>) {
    let Ok(patch) = git2::Patch::from_buffers(
        old.as_bytes(),
        Some(Path::new("head")),
        new.as_bytes(),
        Some(Path::new("worktree")),
        None,
    ) else {
        return (Vec::new(), Vec::new());
    };
    let mut added: BTreeSet<u32> = BTreeSet::new();
    let mut rewritten: BTreeSet<u32> = BTreeSet::new();
    // A deletion at the end of the file points past its last line; clamping
    // keeps the reported change on a line the file actually has.
    let last_line = u32::try_from(new.lines().count().max(1)).unwrap_or(u32::MAX);
    for hunk_index in 0..patch.num_hunks() {
        let Ok((hunk, count)) = patch.hunk(hunk_index) else {
            continue;
        };
        let mut line = hunk.new_start();
        for line_index in 0..count {
            let Ok(diff_line) = patch.line_in_hunk(hunk_index, line_index) else {
                continue;
            };
            match diff_line.origin() {
                '+' => {
                    added.insert(line.min(last_line));
                    line += 1;
                }
                '-' => {
                    rewritten.insert(line.min(last_line));
                }
                ' ' => line += 1,
                // `\ No newline at end of file` and friends occupy no line.
                _ => {}
            }
        }
    }
    (merge_lines(added), merge_lines(rewritten))
}

/// A set of 1-based line numbers as inclusive 1-based ranges.
fn merge_lines(lines: BTreeSet<u32>) -> Vec<(u32, u32)> {
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for line in lines {
        match ranges.last_mut() {
            Some(last) if line <= last.1.saturating_add(1) => last.1 = line,
            _ => ranges.push((line, line)),
        }
    }
    ranges
}

/// How a directive's own source lines have changed, if at all: [`Change::Modified`]
/// when any of them stands where a removed line was, else [`Change::Added`] when
/// any of them is new, else [`Change::Unchanged`].
///
/// A span's trailing trivia — the blank line or comment after the last posting —
/// belongs to no transaction and is ignored.
fn directive_change(
    ledger: &Ledger,
    changed: &HashMap<u16, FileChange>,
    line_starts: &mut HashMap<u16, Vec<usize>>,
    source: &Spanned<Directive>,
) -> Change {
    let Some(file_change) = changed.get(&source.file_id) else {
        return Change::Unchanged;
    };
    let Some(file) = ledger.source_map.get(source.file_id as usize) else {
        return Change::Unchanged;
    };
    let text: &str = &file.source;
    let starts = line_starts
        .entry(source.file_id)
        .or_insert_with(|| line_starts_of(text));
    let body = text.get(source.span.start..source.span.end).unwrap_or("");
    let last_byte = source.span.start + body.trim_end().len();
    let first = line_of(starts, source.span.start);
    let last = line_of(starts, last_byte.saturating_sub(1).max(source.span.start));
    let over = |ranges: &Vec<(u32, u32)>| ranges.iter().any(|&(from, to)| from <= last && first <= to);

    if over(&file_change.rewritten) {
        Change::Modified
    } else if over(&file_change.added) {
        Change::Added
    } else {
        Change::Unchanged
    }
}

/// The byte offset at which each line of `text` starts, for offset → line lookup.
fn line_starts_of(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
    starts
}

/// The 1-based line `offset` falls on, given the file's line starts.
fn line_of(starts: &[usize], offset: usize) -> u32 {
    u32::try_from(starts.partition_point(|&start| start <= offset)).unwrap_or(u32::MAX)
}

/// One replacement of a byte range inside a file: `text` goes where `start..end`
/// was.
struct Replacement {
    start: usize,
    end: usize,
    text: String,
}

/// The file text with `edit` applied to the transaction at `span`: its header
/// line rewritten from the date through the narration, and the units figure on
/// each edited posting's line. Every other byte of the file is copied straight
/// through — including the tags, links, comments, costs and prices on the very
/// lines being rewritten.
fn rewrite_transaction(
    text: &str,
    span: Span,
    tx: &Transaction,
    edit: &TransactionEdit,
) -> Result<String, String> {
    let mut replacements = header_replacement(text, span, edit)?;
    for posting in &edit.postings {
        replacements.push(posting_replacement(text, tx, posting)?);
    }
    replacements.sort_by_key(|replacement| replacement.start);
    // Two edits answering to the same bytes would each be written against
    // offsets the other has already moved.
    for pair in replacements.windows(2) {
        if pair[1].start < pair[0].end {
            return Err("edit rewrites the same text twice".to_string());
        }
    }
    let mut rewritten = String::with_capacity(text.len());
    let mut cursor = 0;
    for replacement in &replacements {
        rewritten.push_str(&text[cursor..replacement.start]);
        rewritten.push_str(&replacement.text);
        cursor = replacement.end;
    }
    rewritten.push_str(&text[cursor..]);
    Ok(rewritten)
}

/// The header rewrite: `DATE FLAG "payee" "narration"`, with whatever follows on
/// the line — tags, links, a comment — left exactly as it was.
///
/// The edit is surgical: the date, the flag and the description's literals are
/// located as tokens in the line, and only the ones whose value the edit
/// changes have their bytes replaced. Every other byte keeps the file's own
/// spelling — the line's indent, the widths of the separators, a literal's
/// quoting and escapes. The single-literal `"payee|narration"` and two-literal
/// `"payee" "narration"` forms are converted into one another only when the
/// edit forces it (a distinct payee where the source spelled neither a payee
/// nor a `|`), never because a value was merely re-typed. A payee the narration
/// already covers, or none at all, collapses to the narration alone.
fn header_replacement(
    text: &str,
    span: Span,
    edit: &TransactionEdit,
) -> Result<Vec<Replacement>, String> {
    let start = text[..span.start.min(text.len())]
        .rfind('\n')
        .map_or(0, |at| at + 1);
    let end = text[start..]
        .find('\n')
        .map_or(text.len(), |at| start + at);
    let line = &text[start..end];
    let header = header_tokens(line)
        .ok_or_else(|| "the transaction's header line has no payee or narration".to_string())?;
    let date = checked_date(&edit.date)?;
    let flag = checked_flag(&edit.flag)?;
    let payee = edit.payee.as_deref().filter(|payee| !payee.is_empty());
    let distinct_payee = payee.filter(|payee| *payee != edit.narration);
    let narration = edit.narration.as_str();
    let replace = |range: (usize, usize), text: String| Replacement {
        start: start + range.0,
        end: start + range.1,
        text,
    };

    let mut replacements = Vec::new();

    // The date and the flag are single tokens: replace each only when the bytes
    // it spells are no longer the bytes the edit asks for.
    if &line[header.date.0..header.date.1] != date.as_str() {
        replacements.push(replace(header.date, date));
    }
    match header.flag {
        Some(flag_range) if &line[flag_range.0..flag_range.1] != flag => {
            replacements.push(replace(flag_range, flag.to_string()));
        }
        Some(_) => {}
        // No flag token to replace: put one after the date, leaving the
        // whitespace that already separated the date from the description.
        None => replacements.push(replace((header.date.1, header.date.1), format!(" {flag}"))),
    }

    // The description is either two literals (`"payee" "narration"`) or one
    // (`"payee|narration"`, or just the narration). Touch each literal whose
    // value changed; convert between the forms only when the edit's payee
    // forces a form the source did not spell.
    match header.strings.len() {
        2 => {
            let payee_range = header.strings[0];
            let narration_range = header.strings[1];
            let payee_literal = &line[payee_range.0..payee_range.1];
            let narration_literal = &line[narration_range.0..narration_range.1];
            match distinct_payee {
                Some(payee) => {
                    if unquote_beancount(payee_literal) != payee {
                        replacements.push(replace(payee_range, quote_beancount(payee)));
                    }
                    if unquote_beancount(narration_literal) != narration {
                        replacements.push(replace(narration_range, quote_beancount(narration)));
                    }
                }
                // The edit drops the payee, so the two-literal form has to give
                // way to one: withdraw the payee literal and the whitespace that
                // joined it to the narration, leaving an untouched narration its
                // own bytes.
                None => {
                    replacements.push(replace((payee_range.0, narration_range.0), String::new()));
                    if unquote_beancount(narration_literal) != narration {
                        replacements.push(replace(narration_range, quote_beancount(narration)));
                    }
                }
            }
        }
        1 => {
            let range = header.strings[0];
            let value = unquote_beancount(&line[range.0..range.1]);
            match distinct_payee {
                // A single literal that already spelled `payee|narration` keeps
                // that form; one spelled without a `|` has to grow into two.
                Some(payee) if value.contains('|') => {
                    let rewritten = format!("{payee}|{narration}");
                    if rewritten != value {
                        replacements.push(replace(range, quote_beancount(&rewritten)));
                    }
                }
                Some(payee) => replacements.push(replace(
                    range,
                    format!("{} {}", quote_beancount(payee), quote_beancount(narration)),
                )),
                None => {
                    if value != narration {
                        replacements.push(replace(range, quote_beancount(narration)));
                    }
                }
            }
        }
        // A header cannot describe three literals; whatever was loaded, rewrite
        // the whole description as the tokens the edit asks for.
        _ => {
            let last = *header.strings.last().expect("header_tokens keeps at least one");
            let description = match distinct_payee {
                Some(payee) => {
                    format!("{} {}", quote_beancount(payee), quote_beancount(narration))
                }
                None => quote_beancount(narration),
            };
            replacements.push(replace((header.strings[0].0, last.1), description));
        }
    }

    Ok(replacements)
}

/// The units rewrite for one posting: the new figure spliced in where the amount
/// already is, or inserted after the account when the line never spelled the
/// amount out (the interpolated leg of a transaction).
fn posting_replacement(
    text: &str,
    tx: &Transaction,
    edit: &PostingEdit,
) -> Result<Replacement, String> {
    let posting = tx
        .postings
        .get(edit.index as usize)
        .ok_or_else(|| format!("transaction has no posting {}", edit.index))?;
    let Some(IncompleteAmount::Complete(amount)) = &posting.value.units else {
        return Err(format!("posting {} has no amount to edit", edit.index));
    };
    let number = Decimal::from_str(edit.units.trim())
        .map_err(|e| format!("`{}` is not an amount: {e}", edit.units))?;
    let line = text
        .get(posting.span.start..posting.span.end)
        .ok_or_else(|| format!("posting {} is outside its file", edit.index))?;
    let (start, end, replacement) = units_replacement(
        line,
        posting.value.account.as_str(),
        amount.currency.as_str(),
        &number.to_string(),
    )?;
    Ok(Replacement {
        start: posting.span.start + start,
        end: posting.span.start + end,
        text: replacement,
    })
}

/// Where a posting line's units figure sits, and what to put in its place,
/// leaving the account, the currency, the `@`/`@@` price, the `{cost}` spec and
/// any trailing comment exactly as written.
///
/// The figure is everything between the account and the currency; beancount
/// allows an arithmetic expression there, so the whole run is replaced rather
/// than a single token. A posting whose line carries no currency had its amount
/// interpolated at load time and has to have it spelled out.
fn units_replacement(
    line: &str,
    account: &str,
    currency: &str,
    number: &str,
) -> Result<(usize, usize, String), String> {
    let account_at = line
        .find(account)
        .filter(|at| {
            line[..*at]
                .chars()
                .next_back()
                .is_none_or(|before| !before.is_alphanumeric() && before != ':' && before != '_')
        })
        .ok_or_else(|| format!("`{account}` is not on its posting line"))?;
    let after_account = account_at + account.len();
    // The units stop where the rest of the posting's annotation starts: a
    // comment, a price, or a cost spec.
    let units_end = [';', '@', '{']
        .into_iter()
        .filter_map(|marker| line[after_account..].find(marker))
        .min()
        .unwrap_or(line.len() - after_account);
    let units = &line[after_account..after_account + units_end];
    match units.find(currency) {
        Some(at) => {
            let figure = units[..at].trim_end();
            if figure.is_empty() {
                // The line names the currency but no figure: put the number in
                // front of the currency.
                let insert = after_account + at;
                Ok((insert, insert, format!("{number} ")))
            } else {
                let start = after_account + (units[..at].len() - units[..at].trim_start().len());
                Ok((start, after_account + figure.len(), number.to_string()))
            }
        }
        None => Ok((after_account, after_account, format!(" {number} {currency}"))),
    }
}

/// Quote a string for a transaction header, escaping exactly what the lexer
/// reads as an escape so the value round-trips.
fn quote_beancount(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            _ => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

/// A transaction header's editable tokens, as byte ranges within its line: the
/// date, the flag (when the line spells one) and the description literals.
struct HeaderTokens {
    /// The date token, past the line's indent.
    date: (usize, usize),
    /// The flag token, if the line has one.
    flag: Option<(usize, usize)>,
    /// The description: a single `"payee|narration"` (or narration) literal, or
    /// a payee literal and a narration literal.
    strings: Vec<(usize, usize)>,
}

/// Locate a header line's tokens so an edit can replace one and leave the rest
/// of the line's bytes alone.
///
/// `None` when the line carries no description literal — a header an edit
/// cannot address. The line's indent, the whitespace between tokens and
/// anything after the description (tags, links, a comment) are deliberately
/// absent from every range.
fn header_tokens(line: &str) -> Option<HeaderTokens> {
    let date_start = line.len() - line.trim_start().len();
    let date_end = date_start
        + line[date_start..]
            .find(char::is_whitespace)
            .unwrap_or(line.len() - date_start);
    let first_quote = date_end + line[date_end..].find('"')?;
    let between = &line[date_end..first_quote];
    let flag = between.find(|character: char| !character.is_whitespace()).map(|offset| {
        let from = date_end + offset;
        (from, from + between[offset..].trim_end().len())
    });
    // The header's quoted strings are its first tokens, and a header has at
    // most two; a quote further along the line — inside a comment, say — is not
    // part of the run, because something other than whitespace separates it.
    let mut strings = Vec::new();
    let mut at = first_quote;
    loop {
        let string_end = end_of_string(line, at)?;
        strings.push((at, string_end));
        let rest = &line[string_end..];
        match rest.find('"') {
            Some(next) if rest[..next].trim().is_empty() => at = string_end + next,
            _ => break,
        }
    }
    Some(HeaderTokens {
        date: (date_start, date_end),
        flag,
        strings,
    })
}

/// The value a header string literal spells, with beancount's escapes undone —
/// the inverse of `quote_beancount`, so an edit's value can be compared with a
/// literal without rewriting the literal when the two agree.
fn unquote_beancount(literal: &str) -> String {
    let inner = literal
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(literal);
    let mut value = String::with_capacity(inner.len());
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            value.push(character);
            continue;
        }
        match characters.next() {
            Some('n') => value.push('\n'),
            Some('r') => value.push('\r'),
            Some('t') => value.push('\t'),
            Some(escaped) => value.push(escaped),
            None => value.push('\\'),
        }
    }
    value
}

/// The byte offset just past the quoted string that starts at `start`.
fn end_of_string(line: &str, start: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut at = start + 1;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'"' => return Some(at + 1),
            _ => at += 1,
        }
    }
    None
}

/// Validate a `YYYY-MM-DD` date and hand back its canonical spelling.
fn checked_date(date: &str) -> Result<String, String> {
    let invalid = || format!("`{date}` is not a YYYY-MM-DD date");
    let mut parts = date.split('-');
    let year: i32 = parts.next().ok_or_else(invalid)?.parse().map_err(|_| invalid())?;
    let month: u32 = parts.next().ok_or_else(invalid)?.parse().map_err(|_| invalid())?;
    let day: u32 = parts.next().ok_or_else(invalid)?.parse().map_err(|_| invalid())?;
    if parts.next().is_some() {
        return Err(invalid());
    }
    rustledger_core::naive_date(year, month, day)
        .map(|date| date.to_string())
        .ok_or_else(invalid)
}

/// Validate a transaction flag: a single character, and not one that would end
/// the header token it is written in.
fn checked_flag(flag: &str) -> Result<&str, String> {
    let mut characters = flag.chars();
    match (characters.next(), characters.next()) {
        (Some(character), None)
            if !character.is_whitespace() && character != '"' && character != ';' =>
        {
            Ok(flag)
        }
        _ => Err(format!("`{flag}` is not a transaction flag")),
    }
}

/// The ledger's errors in comparable form: code and message are what separate an
/// edit's mistakes from the errors the file already had.
fn error_signatures(errors: &[LedgerError]) -> Vec<String> {
    errors
        .iter()
        .map(|error| format!("{}: {}", error.code, error.message))
        .collect()
}

/// The errors in `after` that `before` did not already report, counting repeats
/// so that fixing one error while raising another still reads as new.
fn new_errors(before: &[String], after: Vec<String>) -> Vec<String> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for error in before {
        *seen.entry(error.as_str()).or_default() += 1;
    }
    after
        .into_iter()
        .filter(|error| match seen.get_mut(error.as_str()) {
            Some(count) if *count > 0 => {
                *count -= 1;
                false
            }
            _ => true,
        })
        .collect()
}

/// The envelope plugin's own plumbing accounts, mirroring its
/// `accounts.rs` (`TO_BE_BUDGETED` and `MONEY`).
///
/// `To-Be-Budgeted` is the pool of money the configuration has not assigned to
/// an envelope — the budget screen's own headline — and `Money` is the contra of
/// the ledger's on-budget money, which the plugin reads to keep the pool's
/// figure real. Neither is an envelope a user funded, so neither becomes a
/// group: the pool is reported as [`BudgetMonth::to_budget`] and the contra is
/// left out of the month's lines.
const ENVELOPE_POOL: &str = "Equity:Envelope:To-Be-Budgeted";
const ENVELOPE_MONEY: &str = "Equity:Envelope:Money";

/// What the budget says about a ledger the plugin has not budgeted: an empty
/// screen would read as "this month is worth nothing", which is a different
/// statement from "nothing was written".
const NO_ENVELOPE_BUDGET: &str = "this ledger has no envelope budget data — the envelope plugin has written nothing under `Equity:Envelope:`, so there is no month to show";

/// One envelope of a budget month, as the plugin's own bookkeeping reports it.
///
/// Every figure is an exact decimal string in [`BudgetMonth::currency`], the
/// app's convention: nothing is a float and nothing is rounded here.
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct BudgetEnvelope {
    /// The envelope's label inside its group — the account's path below the
    /// group, e.g. `Bread` for `Equity:Envelope:Food:Bread` in group `Food`.
    pub name: String,
    /// The account the plugin writes this envelope's postings to, which is what
    /// names it in the ledger.
    pub full_name: String,
    /// What the month put into the envelope, net: inherited funding, one-off
    /// `adjust`/`zero`/`pad` deltas, and the rollover sweep's own correction,
    /// which takes money back out and so reads negative.
    pub budgeted: String,
    /// What the month spent out of it, the plugin's mirrored activity, reported
    /// as a positive figure.
    pub spent: String,
    /// What the envelope held at the month's end, every earlier month carried
    /// into it — the subtree total the plugin's own `balance` assertions check.
    pub balance: String,
}

/// One top-level branch of the plugin's envelope tree — a group in the budget
/// screen's sense — and its totals over its envelopes.
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct BudgetGroup {
    /// The branch's own name, the first account component below the namespace.
    pub name: String,
    /// Its envelopes' figures summed, exact decimal strings like theirs.
    pub budgeted: String,
    pub spent: String,
    pub balance: String,
    /// The envelopes below it, in the ledger's tree order.
    pub envelopes: Vec<BudgetEnvelope>,
}

/// One month of the ledger's envelope budget, as the budget screen reads it.
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct BudgetMonth {
    /// The month the figures belong to, `YYYY-MM`.
    pub month: String,
    /// Every month the plugin budgeted, oldest first — the screen's navigation.
    pub months: Vec<String>,
    /// The month's groups, alphabetically.
    pub groups: Vec<BudgetGroup>,
    /// `Equity:Envelope:To-Be-Budgeted` at the month's end: what the
    /// configuration has not assigned to an envelope, per the plugin's rule
    /// `To-Be-Budgeted = on-budget money − Σ categories`.
    pub to_budget: String,
    /// The commodity every figure is in: the ledger's operating currency.
    pub currency: String,
}

/// Whether `text` is a `YYYY-MM` month label — the budget's unit of time, the
/// same one the plugin's `month.rs` writes.
fn is_month_label(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 7
        && bytes[4] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && matches!((bytes[5], bytes[6]), (b'0', b'1'..=b'9') | (b'1', b'0'..=b'2'))
}

/// What one envelope did in one month.
#[derive(Clone, Copy, Default)]
struct EnvelopeFlow {
    /// Postings the month moved into the envelope, net of the sweep's
    /// corrections.
    budgeted: Decimal,
    /// Postings the month spent out of it, as a positive figure.
    spent: Decimal,
}

/// The commodity the budget reports in: the ledger's operating currency when the
/// plugin budgeted in it, else the commodity the most money moved in (ties go to
/// the later name, so the payload does not wobble between loads of one ledger).
fn budget_currency(held: &BTreeMap<String, Decimal>, operating: &[String]) -> Option<String> {
    if let Some(currency) = operating.iter().find(|currency| held.contains_key(*currency)) {
        return Some(currency.clone());
    }
    let mut best: Option<(&String, Decimal)> = None;
    for (currency, total) in held {
        if best.is_none_or(|(_, highest)| total.abs() > highest) {
            best = Some((currency, total.abs()));
        }
    }
    best.map(|(currency, _)| currency.clone())
}

/// A group while it is being collected: its envelopes and the totals they add up
/// to, so the totals never have to be read back out of the rendered strings.
#[derive(Default)]
struct BudgetGroupTotals {
    envelopes: Vec<BudgetEnvelope>,
    budgeted: Decimal,
    spent: Decimal,
    balance: Decimal,
}

/// One month of the ledger's envelope budget.
///
/// The plugin (`plugins/rustledger-envelope`) is the only writer of the
/// `Equity:Envelope:` namespace and the owner of its arithmetic. For every
/// budget month it emits one entry per step — the backing contra
/// (`Equity:Envelope:Money` against `To-Be-Budgeted`), the inherited funding,
/// the month's mirrored activity, the `adjust`/`zero`/`pad` reconciliation and
/// the rollover sweep — each dated the month's last day, tagged `#envelope` and
/// carrying `envelope-kind`/`envelope-month` metadata (see its `emit.rs`). Every
/// posting stays inside the namespace, so the budget is one prefix away from the
/// rest of the app.
///
/// This is the one surface that *reads* those accounts instead of hiding them
/// (see [`is_envelope_account`]): a month's flows are the postings dated inside
/// it, an envelope's balance is every posting up to and including the requested
/// month — the same subtree total the plugin's month-end assertions measure —
/// and the pool's balance is the screen's `to_budget`.
///
/// `envelope-kind` decides what a posting is: activity is spending, funding,
/// adjustments and the sweep are everything else. Without the metadata — a
/// hand-written entry in the namespace — the posting's sign decides, which is
/// what the plugin's own funding/activity postings imply (money leaving an
/// envelope is spending).
///
/// `month` is a `YYYY-MM` label; `None` is the newest month with data, and a
/// month outside the data (a gap, or before the first month) reports what stood
/// at its end: the carries, with nothing moved in it.
fn budget_month(ledger: &Ledger, month: Option<&str>) -> Result<BudgetMonth, String> {
    if let Some(text) = month
        && !is_month_label(text)
    {
        return Err(format!("`{text}` is not a budget month in YYYY-MM form"));
    }
    // Every posting inside the namespace, summed per account, month and
    // commodity. The per-commodity totals pick the one to report in: the plugin
    // tracks each commodity separately, and the budget screen shows one.
    let mut flows: BTreeMap<(String, String, String), EnvelopeFlow> = BTreeMap::new();
    let mut held: BTreeMap<String, Decimal> = BTreeMap::new();
    for directive in ledger.balance_view() {
        let Directive::Transaction(tx) = directive else {
            continue;
        };
        let date = tx.date.to_string();
        // The plugin dates every entry the month's last day, so the date says
        // which month an entry belongs to (`model.rs`: `let date =
        // month.last_day()`).
        let Some(entry_month) = date.get(0..7).filter(|text| is_month_label(text)) else {
            continue;
        };
        let kind = match tx.meta.get("envelope-kind") {
            Some(MetaValue::String(kind)) => Some(kind.as_str()),
            _ => None,
        };
        for posting in &tx.postings {
            let account = posting.value.account.as_str();
            if !is_envelope_account(account) {
                continue;
            }
            let Some(IncompleteAmount::Complete(amount)) = &posting.value.units else {
                // Residual/interpolated postings never occur post-booked.
                continue;
            };
            let commodity = amount.currency.as_str().to_owned();
            *held.entry(commodity.clone()).or_default() += amount.number.abs();
            let flow = flows
                .entry((account.to_owned(), entry_month.to_owned(), commodity))
                .or_default();
            let spending = match kind {
                Some("Envelope activity") => true,
                Some("Envelope funding" | "Envelope adjustment" | "Envelope rollover") => false,
                // No kind at all, or the backing entry, which only ever touches
                // the pool and the contra.
                _ => amount.number.is_sign_negative(),
            };
            if spending {
                flow.spent -= amount.number;
            } else {
                flow.budgeted += amount.number;
            }
        }
    }
    let Some(currency) = budget_currency(&held, &ledger.options.operating_currency) else {
        return Err(NO_ENVELOPE_BUDGET.to_string());
    };

    // The reported month's figures. Each account's months come in order, so a
    // month past the requested one ends its history.
    let mut months: BTreeSet<String> = BTreeSet::new();
    let mut histories: BTreeMap<String, Vec<(String, EnvelopeFlow)>> = BTreeMap::new();
    for ((account, entry_month, commodity), flow) in flows {
        if commodity != currency {
            continue;
        }
        months.insert(entry_month.clone());
        histories.entry(account).or_default().push((entry_month, flow));
    }
    let Some(newest) = months.iter().next_back() else {
        return Err(NO_ENVELOPE_BUDGET.to_string());
    };
    let requested = month.unwrap_or(newest).to_owned();

    let mut groups: BTreeMap<String, BudgetGroupTotals> = BTreeMap::new();
    let mut to_budget = Decimal::ZERO;
    for (account, history) in histories {
        // `Equity:Envelope:<path>`; the namespace itself holds nothing.
        let Some(path) = account
            .strip_prefix(ENVELOPE_NAMESPACE)
            .and_then(|tail| tail.strip_prefix(':'))
        else {
            continue;
        };
        let mut budgeted = Decimal::ZERO;
        let mut spent = Decimal::ZERO;
        let mut balance = Decimal::ZERO;
        let mut exists = false;
        for (entry_month, flow) in history {
            if entry_month.as_str() > requested.as_str() {
                break;
            }
            exists = true;
            if entry_month == requested {
                budgeted = flow.budgeted;
                spent = flow.spent;
            }
            balance += flow.budgeted - flow.spent;
        }
        if account == ENVELOPE_POOL {
            to_budget = balance;
            continue;
        }
        // The contra of the on-budget money is not an envelope; a group for it
        // would read as a category holding the user's whole bank balance.
        if !exists || account == ENVELOPE_MONEY {
            continue;
        }
        let (group_name, name) = match path.split_once(':') {
            Some((group_name, name)) => (group_name.to_owned(), name.to_owned()),
            None => (path.to_owned(), path.to_owned()),
        };
        let envelope = BudgetEnvelope {
            name,
            full_name: account,
            budgeted: budgeted.to_string(),
            spent: spent.to_string(),
            balance: balance.to_string(),
        };
        let group = groups.entry(group_name).or_default();
        group.budgeted += budgeted;
        group.spent += spent;
        group.balance += balance;
        group.envelopes.push(envelope);
    }
    let groups: Vec<BudgetGroup> = groups
        .into_iter()
        .map(|(name, totals)| BudgetGroup {
            name,
            budgeted: totals.budgeted.to_string(),
            spent: totals.spent.to_string(),
            balance: totals.balance.to_string(),
            envelopes: totals.envelopes,
        })
        .collect();
    Ok(BudgetMonth {
        month: requested,
        months: months.into_iter().collect(),
        groups,
        to_budget: to_budget.to_string(),
        currency,
    })
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
fn budget(month: Option<String>, state: State<'_, AppState>) -> Result<BudgetMonth, String> {
    budget_view(&state, month.as_deref())
}

/// [`budget`] against the held ledger, so the tests can ask the same question
/// the screen does.
fn budget_view(state: &AppState, month: Option<&str>) -> Result<BudgetMonth, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger loaded")?;
    budget_month(&loaded.ledger, month)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = Builder::<AppRuntime>::new()
        // Then register them (separated by a comma)
        .commands(collect_commands![
            greet,
            accounts,
            account_transactions,
            load_ledger,
            update_transaction,
            transaction_source,
            update_transaction_source,
            transaction_file,
            update_file_source,
            journal_files,
            suggest_transaction_file,
            add_transaction,
            file_transactions,
            file_text,
            update_file_text,
            add_account,
            suggest_account_file,
            ledger_diff,
            commit_ledger,
            budget,
        ]);

    // Only export bindings on non-release builds
    #[cfg(debug_assertions)]
    builder
        .export(Typescript::default(), "../src/bindings.ts")
        .expect("Failed to export typescript bindings");

    let builder = tauri::Builder::<AppRuntime>::default()
        .manage(AppState::new())
        // and finally tell Tauri how to invoke them
        .invoke_handler(builder.invoke_handler());

    // QtWebEngine never hands over custom-protocol POST bodies, so the Qt
    // runtime carries the invoke payload in a header instead. Without this,
    // every `invoke()` from the frontend silently fails to reach Rust.
    #[cfg(all(target_os = "linux", not(feature = "wry")))]
    let builder = builder.invoke_system(tauri_runtime_qt::INVOKE_SYSTEM_SCRIPT);

    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn export_bindings() {
        Builder::<AppRuntime>::new()
            .commands(collect_commands![
                greet,
                accounts,
                account_transactions,
                load_ledger,
                update_transaction,
                transaction_source,
                update_transaction_source,
                transaction_file,
                update_file_source,
                journal_files,
                suggest_transaction_file,
                add_transaction,
                file_transactions,
                file_text,
                update_file_text,
                add_account,
                suggest_account_file,
                ledger_diff,
                commit_ledger,
                budget,
            ])
            .export(Typescript::default(), "../src/bindings.ts")
            .unwrap();
    }

    #[test]
    fn computes_account_balances_with_pads_and_multi_currency() {
        let mut fs = rustledger_loader::VirtualFileSystem::new();
        fs.add_file(
            "main.beancount",
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank
2026-01-01 open Assets:Bank:Checking
2026-01-01 open Assets:Bank:Emergency
2026-01-01 open Assets:Cash
2026-01-01 open Expenses:Food
2026-01-01 open Income:Salary
2026-01-01 open Equity:Opening-Balances

2026-01-01 pad Assets:Bank:Checking Equity:Opening-Balances
2026-01-02 balance Assets:Bank:Checking 100.00 USD

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -50.00 USD
  Expenses:Food                    50.00 USD

2026-01-11 * \"Salary\"
  Assets:Bank:Checking            200.00 USD
  Income:Salary                  -200.00 USD

2026-01-12 * \"Cash\"
  Assets:Cash                      10.00 EUR @ 1.10 USD
  Assets:Bank:Checking            -11.00 USD

2026-01-13 * \"Transfer\"
  Assets:Bank                     -30.00 USD
  Assets:Bank:Emergency            30.00 USD
",
        );
        let ledger = rustledger_loader::load_with_fs(
            Path::new("main.beancount"),
            &LoadOptions::default(),
            Box::new(fs),
        )
        .unwrap();
        assert!(ledger.errors.is_empty(), "errors: {:?}", ledger.errors);

        let balances = account_balances(&ledger, &BTreeMap::new());
        let get = |name: &str| {
            balances
                .iter()
                .find(|b| b.name == name)
                .map(|b| b.balance.as_str())
                .unwrap_or_else(|| panic!("missing {name} in {balances:?}"))
        };
        let currency = |name: &str| {
            balances
                .iter()
                .find(|b| b.name == name)
                .map(|b| b.currency.as_str())
                .unwrap_or_else(|| panic!("missing {name} in {balances:?}"))
        };

        // Checking: 100 (pad) - 50 + 200 - 11 (bought EUR cash).
        assert_eq!(get("Assets:Bank:Checking"), "239.00");
        assert_eq!(get("Assets:Bank:Emergency"), "30.00");
        // Assets:Bank has postings AND sub-accounts: its entry is the subtree
        // total (own -30 + Checking 239 + Emergency 30).
        assert_eq!(get("Assets:Bank"), "239.00");
        // Ancestors without own postings get synthesized: USD 239 + EUR 10,
        // USD is the operating currency → direct match.
        assert_eq!(get("Assets"), "239.00");
        // USD held + USD is an operating currency → direct match.
        assert_eq!(get("Expenses:Food"), "50.00");
        assert_eq!(get("Income:Salary"), "-200.00");
        // Fallback: EUR only, not an operating currency → largest |units|.
        assert_eq!(get("Assets:Cash"), "10.00");
        assert_eq!(get("Equity:Opening-Balances"), "-100.00");
        // The reported commodity is the one the units are in.
        assert_eq!(currency("Assets:Bank:Checking"), "USD");
        assert_eq!(currency("Assets"), "USD");
        // EUR is not an operating currency, so Cash falls back to it.
        assert_eq!(currency("Assets:Cash"), "EUR");
        // Exact scale is preserved end-to-end.
        assert!(balances.iter().all(|b| b.balance.contains('.')));
    }

    #[test]
    fn lists_account_transactions() {
        let mut fs = rustledger_loader::VirtualFileSystem::new();
        fs.add_file(
            "main.beancount",
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank
2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food
2026-01-01 open Income:Salary

2026-01-05 * \"Employer\" \"Salary\"
  Assets:Bank:Checking            100.00 USD
  Income:Salary                  -100.00 USD

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD

2026-01-12 ! \"Transfer\"
  Assets:Bank                     -30.00 USD
  Assets:Bank:Checking             30.00 USD
",
        );
        let ledger = rustledger_loader::load_with_fs(
            Path::new("main.beancount"),
            &LoadOptions::default(),
            Box::new(fs),
        )
        .unwrap();
        assert!(ledger.errors.is_empty(), "errors: {:?}", ledger.errors);

        // Ledger-stream ids of the transactions touching an account, recomputed
        // here so the assertion does not depend on how many non-transaction
        // directives the fixture happens to contain.
        let stream_ids = |account: &str| -> Vec<u32> {
            let prefix = format!("{account}:");
            ledger
                .balance_view()
                .into_iter()
                .enumerate()
                .filter_map(|(id, directive)| {
                    let Directive::Transaction(tx) = directive else {
                        return None;
                    };
                    let touches = tx.postings.iter().any(|posting| {
                        let name = posting.value.account.as_str();
                        name == account || name.starts_with(&prefix)
                    });
                    touches.then(|| u32::try_from(id).unwrap())
                })
                .collect()
        };

        // A leaf account returns exactly the transactions touching it, newest
        // first (Salary 01-05, Groceries 01-10, Transfer 01-12 → reversed).
        let checking = account_transaction_views(&ledger, &HashMap::new(), "Assets:Bank:Checking");
        assert_eq!(checking.len(), 3);
        assert_eq!(
            checking
                .iter()
                .map(|t| t.date.as_str())
                .collect::<Vec<_>>(),
            ["2026-01-12", "2026-01-10", "2026-01-05"]
        );
        assert_eq!(
            checking
                .iter()
                .map(|t| t.narration.as_str())
                .collect::<Vec<_>>(),
            ["Transfer", "Groceries", "Salary"]
        );

        // Every returned transaction carries all of its postings, including the
        // counter-leg that does not touch the account. Newest entry first.
        assert_eq!(
            checking[0]
                .postings
                .iter()
                .map(|p| (p.account.as_str(), p.units.as_str()))
                .collect::<Vec<_>>(),
            [
                ("Assets:Bank", "-30.00"),
                ("Assets:Bank:Checking", "30.00"),
            ]
        );
        assert_eq!(checking[0].flag, "!");
        assert_eq!(checking[0].postings[0].currency, "USD");
        assert_eq!(
            checking[1]
                .postings
                .iter()
                .map(|p| p.account.as_str())
                .collect::<Vec<_>>(),
            ["Assets:Bank:Checking", "Expenses:Food"]
        );
        assert_eq!(checking[1].flag, "*");
        assert_eq!(checking[1].postings.len(), 2);
        assert!(checking.iter().all(|t| t.postings.len() == 2));
        assert!(checking[1].payee.is_none());
        // Explicit payee survives; narration is the description.
        assert_eq!(checking[2].payee.as_deref(), Some("Employer"));
        assert_eq!(checking[2].narration, "Salary");

        // A parent account also returns transactions that only touch its
        // sub-account: the leaf's two entries have no posting on Assets:Bank.
        let bank = account_transaction_views(&ledger, &HashMap::new(), "Assets:Bank");
        assert_eq!(
            bank.iter().map(|t| t.narration.as_str()).collect::<Vec<_>>(),
            ["Transfer", "Groceries", "Salary"]
        );
        assert!(bank[1].postings.iter().all(|p| p.account != "Assets:Bank"));

        // Ids are unique, and ascending they are the ledger stream order.
        let mut ids: Vec<u32> = checking.iter().map(|t| t.id).collect();
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "duplicate ids: {ids:?}");
        ids.sort_unstable();
        assert_eq!(ids, stream_ids("Assets:Bank:Checking"));

        // The virtual root — an empty account — is the whole ledger.
        let root = account_transaction_views(&ledger, &HashMap::new(), "");
        assert_eq!(root.len(), 3);
        assert_eq!(
            root.iter().map(|t| t.narration.as_str()).collect::<Vec<_>>(),
            ["Transfer", "Groceries", "Salary"]
        );
        let mut root_ids: Vec<u32> = root.iter().map(|t| t.id).collect();
        root_ids.sort_unstable();
        assert_eq!(root_ids, ids);

        // An account with no postings in a transaction stream yields an empty
        // view rather than an error.
        assert!(account_transaction_views(&ledger, &HashMap::new(), "Assets:Cash").is_empty());

        // A ledger whose files are not in a git repository reports no row as
        // modified — the flag never turns git's absence into an error.
        assert!(root.iter().all(|view| view.change == Change::Unchanged));
    }

    /// A throwaway directory holding a real ledger file and a real repository,
    /// removed when the fixture drops.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        /// A directory of its own: the process id plus a counter keeps tests
        /// running at the same time out of each other's way.
        fn new(name: &str) -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "ledger-app-{name}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn file(&self, name: &str) -> PathBuf {
            self.path.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// A repository in `dir` whose first commit holds `tracked` — the files the
    /// caller has already written. Identity is set, as `commit_ledger` needs it.
    fn repo_with_commit(dir: &TempDir, tracked: &[&str]) -> git2::Repository {
        let repo = git2::Repository::init(&dir.path).unwrap();
        {
            let mut config = repo.config().unwrap();
            config.set_str("user.name", "Test Ledger").unwrap();
            config.set_str("user.email", "ledger@example.com").unwrap();
        }
        {
            let mut index = repo.index().unwrap();
            for name in tracked {
                index.add_path(Path::new(name)).unwrap();
            }
            index.write().unwrap();
            let tree_id = index.write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            let signature = repo.signature().unwrap();
            repo.commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])
                .unwrap();
        }
        repo
    }

    /// Write the ledger fixture, put it under version control with one commit,
    /// and hand back the repository and the ledger file's path.
    fn committed_fixture(dir: &TempDir, ledger: &str) -> (git2::Repository, PathBuf) {
        let path = dir.file("main.bean");
        fs::write(&path, ledger).unwrap();
        let repo = repo_with_commit(dir, &["main.bean"]);
        (repo, path)
    }

    /// The app state around a freshly loaded copy of `path`, baseline included —
    /// the same `load_baselined` the app itself uses.
    fn loaded_state(path: &Path) -> AppState {
        AppState {
            loaded: RwLock::new(Some(load_baselined(path).unwrap())),
        }
    }

    /// The `TransactionView.id` the frontend would be holding for `narration`,
    /// taken from the same views it reads.
    fn view_id(state: &AppState, narration: &str) -> u32 {
        views(state)
            .into_iter()
            .find(|view| view.narration == narration)
            .unwrap_or_else(|| panic!("no transaction narrated `{narration}`"))
            .id
    }

    /// The narrations of the transactions the register would mark as changed,
    /// whatever kind of change it is.
    fn changed_narrations(state: &AppState) -> Vec<String> {
        views(state)
            .into_iter()
            .filter(|view| view.change != Change::Unchanged)
            .map(|view| view.narration)
            .collect()
    }

    /// Every transaction of the held ledger, as the register sees them.
    fn views(state: &AppState) -> Vec<TransactionView> {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().unwrap();
        account_transaction_views(&loaded.ledger, &loaded.baseline, "")
    }

    /// A units edit for the posting at `index`.
    fn posting(index: u32, units: &str) -> PostingEdit {
        PostingEdit {
            index,
            units: units.to_string(),
        }
    }

    /// A transaction edit of the shape the register sends, for the tests that
    /// rewrite the header as little as possible.
    fn edit_of(
        id: u32,
        date: &str,
        narration: &str,
        postings: Vec<PostingEdit>,
    ) -> TransactionEdit {
        TransactionEdit {
            id,
            date: date.to_string(),
            flag: "*".to_string(),
            payee: None,
            narration: narration.to_string(),
            postings,
        }
    }

    /// A journal split across files — the root holds the `open` directives, an
    /// included file the entries — is editable like any other: the reload after
    /// a write has to follow the `include`, or the included file on its own
    /// reports every account it touches as never opened and the edit is refused.
    #[test]
    fn an_edit_to_an_included_file_reloads_the_whole_journal() {
        let dir = TempDir::new("included");
        let root = dir.file("main.bean");
        let entries = dir.file("2026.bean");
        fs::write(
            &root,
            "\
option \"operating_currency\" \"USD\"

include \"2026.bean\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food
",
        )
        .unwrap();
        fs::write(
            &entries,
            "\
2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food
",
        )
        .unwrap();
        repo_with_commit(&dir, &["main.bean", "2026.bean"]);

        let state = loaded_state(&root);
        let id = view_id(&state, "Groceries");
        edit_transaction(
            &state,
            edit_of(
                id,
                "2026-01-10",
                "Groceries and coffee",
                vec![posting(0, "-25.00")],
            ),
        )
        .unwrap();

        // Only the included file changed, and the root is untouched.
        let written = fs::read_to_string(&entries).unwrap();
        assert!(written.contains("Groceries and coffee"), "entries: {written}");
        assert!(written.contains("-25.00"), "entries: {written}");
        assert!(fs::read_to_string(&root).unwrap().contains("include \"2026.bean\""));

        // The ledger the app now holds is the whole journal, and it is clean.
        {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            let loaded = guard.as_ref().unwrap();
            assert_eq!(loaded.root, root);
            assert!(loaded.ledger.errors.is_empty(), "errors: {:?}", loaded.ledger.errors);
        }
    }

    #[test]
    fn edits_a_transaction_in_place_and_stages_it() {
        let dir = TempDir::new("edit");
        let (repo, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food
2026-01-01 open Income:Salary

2026-01-05 * \"Employer\" \"Salary\"
  Assets:Bank:Checking            100.00 USD
  Income:Salary                  -100.00 USD

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food

2026-01-12 * \"Rent\"
  Assets:Bank:Checking            -50.00 USD
  Expenses:Food                    50.00 USD
",
        );
        let state = loaded_state(&path);
        let id = view_id(&state, "Groceries");

        edit_transaction(
            &state,
            edit_of(
                id,
                "2026-01-10",
                "Groceries and coffee",
                vec![posting(0, "-25.00")],
            ),
        )
        .unwrap();

        // Only the two lines the edit addressed changed: the header lost its
        // none-payee, the units figure was replaced in place, the interpolated
        // leg keeps its amount implied and every other byte is untouched.
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food
2026-01-01 open Income:Salary

2026-01-05 * \"Employer\" \"Salary\"
  Assets:Bank:Checking            100.00 USD
  Income:Salary                  -100.00 USD

2026-01-10 * \"Groceries and coffee\"
  Assets:Bank:Checking            -25.00 USD
  Expenses:Food

2026-01-12 * \"Rent\"
  Assets:Bank:Checking            -50.00 USD
  Expenses:Food                    50.00 USD
"
        );

        // The reloaded ledger — the one the app now holds — is clean.
        {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            let errors = &guard.as_ref().unwrap().ledger.errors;
            assert!(errors.is_empty(), "errors: {errors:?}");
        }
        // The edited transaction is the only row that reports itself changed, and
        // it reads as modified — its lines stand where committed lines were.
        assert_eq!(changed_narrations(&state), ["Groceries and coffee"]);
        assert_eq!(
            views(&state)
                .into_iter()
                .find(|view| view.narration == "Groceries and coffee")
                .expect("the edited entry is in the register")
                .change,
            Change::Modified
        );

        // The file is staged: the index differs from HEAD, and only by it.
        let head = repo.head().unwrap().peel_to_tree().unwrap();
        let staged = repo.diff_tree_to_index(Some(&head), None, None).unwrap();
        assert_eq!(
            staged
                .deltas()
                .map(|delta| delta.new_file().path().unwrap().display().to_string())
                .collect::<Vec<_>>(),
            ["main.bean"]
        );
    }

    #[test]
    fn an_unbalancing_edit_is_refused_and_rolled_back() {
        let dir = TempDir::new("unbalanced");
        let (_, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD
",
        );
        let state = loaded_state(&path);
        let id = view_id(&state, "Groceries");
        let before = fs::read(&path).unwrap();

        // Raising one leg without the other leaves 5.00 USD unaccounted for.
        let refused = edit_transaction(
            &state,
            edit_of(id, "2026-01-10", "Groceries", vec![posting(0, "-25.00")]),
        );
        assert!(refused.is_err(), "an unbalanced edit went through");

        // The file is byte-for-byte what it was, and the app still holds the
        // ledger it had before the attempt.
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(changed_narrations(&state), Vec::<String>::new());
        assert_eq!(view_id(&state, "Groceries"), id);
    }

    #[test]
    fn the_source_path_round_trips_and_stages_an_edit() {
        let dir = TempDir::new("source");
        let (repo, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD

2026-01-12 * \"Rent\"
  Assets:Bank:Checking            -50.00 USD
  Expenses:Food                    50.00 USD
",
        );
        let state = loaded_state(&path);
        let id = view_id(&state, "Groceries");
        let before = fs::read(&path).unwrap();
        let head = repo.head().unwrap().peel_to_tree().unwrap();

        // The text is the transaction's own lines, from its date token through
        // the blank line that separates it from the next directive — the span
        // the loader computed, indentation and all.
        let source = transaction_source_text(&state, id).unwrap();
        assert_eq!(
            source,
            "\
2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD

"
        );

        // Feeding the text straight back is a no-op: the same bytes, and
        // nothing new staged.
        edit_transaction_source(&state, id, &source).unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(
            repo.index().unwrap().write_tree().unwrap(),
            head.id(),
            "the round trip staged something"
        );
        assert_eq!(transaction_source_text(&state, id).unwrap(), source);

        // An edit through the text path rewrites the same span in place; the
        // rest of the file stays byte-identical.
        edit_transaction_source(
            &state,
            id,
            "\
2026-01-10 * \"Groceries and coffee\"
  Assets:Bank:Checking            -25.00 USD
  Expenses:Food                    25.00 USD

",
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries and coffee\"
  Assets:Bank:Checking            -25.00 USD
  Expenses:Food                    25.00 USD

2026-01-12 * \"Rent\"
  Assets:Bank:Checking            -50.00 USD
  Expenses:Food                    50.00 USD
"
        );

        // The reloaded ledger is clean, the edited transaction kept its id, and
        // it is the only row reporting itself modified.
        {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            let errors = &guard.as_ref().unwrap().ledger.errors;
            assert!(errors.is_empty(), "errors: {errors:?}");
        }
        assert_eq!(view_id(&state, "Groceries and coffee"), id);
        assert_eq!(changed_narrations(&state), ["Groceries and coffee"]);

        // The file is staged.
        let staged = repo.diff_tree_to_index(Some(&head), None, None).unwrap();
        assert_eq!(
            staged
                .deltas()
                .map(|delta| delta.new_file().path().unwrap().display().to_string())
                .collect::<Vec<_>>(),
            ["main.bean"]
        );
    }

    #[test]
    fn an_unbalancing_source_edit_is_refused_and_rolled_back() {
        let dir = TempDir::new("unbalanced-source");
        let (_, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD
",
        );
        let state = loaded_state(&path);
        let id = view_id(&state, "Groceries");
        let before = fs::read(&path).unwrap();

        // Raising one leg without the other leaves 5.00 USD unaccounted for.
        let refused = edit_transaction_source(
            &state,
            id,
            "\
2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -25.00 USD
  Expenses:Food                    20.00 USD
",
        );
        assert!(refused.is_err(), "an unbalanced source edit went through");

        // The file is byte-for-byte what it was, and the app still holds the
        // ledger it had before the attempt.
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(changed_narrations(&state), Vec::<String>::new());
        assert_eq!(view_id(&state, "Groceries"), id);
    }

    #[test]
    fn an_unknown_id_is_refused_by_both_source_commands() {
        let dir = TempDir::new("unknown-id");
        let (_, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD
",
        );
        let state = loaded_state(&path);
        let before = fs::read(&path).unwrap();

        // One past the last entry of the directive stream the ids index.
        let missing = {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            u32::try_from(guard.as_ref().unwrap().ledger.balance_view().len()).unwrap()
        };
        assert!(transaction_source_text(&state, missing)
            .unwrap_err()
            .contains(&format!("no transaction with id {missing}")));
        assert!(edit_transaction_source(&state, missing, "whatever\n").is_err());
        // Neither attempt touched the file.
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn a_posting_keeps_its_price_and_cost_when_its_number_changes() {
        let dir = TempDir::new("annotations");
        let (_, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Cash
2026-01-01 open Assets:Stock
2026-01-01 open Assets:Bank:Checking

2026-01-13 * \"Cash\"
  Assets:Cash                      10.00 EUR @ 1.10 USD
  Assets:Bank:Checking            -11.00 USD

2026-01-14 * \"Stock\"
  Assets:Stock                     10.00 FOO {2.50 USD}
  Assets:Bank:Checking            -25.00 USD
",
        );
        let state = loaded_state(&path);

        // Both legs move together so the transaction stays balanced: the price
        // and the cost spec are what decide the counter-leg.
        edit_transaction(
            &state,
            edit_of(
                view_id(&state, "Cash"),
                "2026-01-13",
                "Cash",
                vec![posting(0, "12.00"), posting(1, "-13.20")],
            ),
        )
        .unwrap();
        edit_transaction(
            &state,
            edit_of(
                view_id(&state, "Stock"),
                "2026-01-14",
                "Stock",
                vec![posting(0, "12.00"), posting(1, "-30.00")],
            ),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Cash
2026-01-01 open Assets:Stock
2026-01-01 open Assets:Bank:Checking

2026-01-13 * \"Cash\"
  Assets:Cash                      12.00 EUR @ 1.10 USD
  Assets:Bank:Checking            -13.20 USD

2026-01-14 * \"Stock\"
  Assets:Stock                     12.00 FOO {2.50 USD}
  Assets:Bank:Checking            -30.00 USD
"
        );
    }

    /// A header edit touches only the token the edit changed. The line keeps its
    /// indent, its separator widths, its single-literal `payee|narration` form
    /// and its trailing comment; only the narration literal's content moves.
    #[test]
    fn a_header_edit_keeps_the_files_own_formatting() {
        let dir = TempDir::new("header-format");
        let (_, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

  2026-01-10  *  \"Corner Shop|Groceries\"  ; weekly
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD
",
        );
        let state = loaded_state(&path);
        let id = views(&state)[0].id;

        // Only the narration changes; the edit goes through the same
        // `edit_transaction` the `update_transaction` command calls.
        edit_transaction(
            &state,
            TransactionEdit {
                payee: Some("Corner Shop".to_string()),
                ..edit_of(id, "2026-01-10", "Groceries and coffee", Vec::new())
            },
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

  2026-01-10  *  \"Corner Shop|Groceries and coffee\"  ; weekly
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD
"
        );
    }

    /// A posting-only edit changes nothing about the header, so the header line
    /// comes back exactly as the file spelled it — separators, form and comment.
    #[test]
    fn a_posting_edit_leaves_the_header_byte_identical() {
        let dir = TempDir::new("header-untouched");
        let (_, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

  2026-01-10  *  \"Corner Shop|Groceries\"  ; weekly
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD
",
        );
        let state = loaded_state(&path);
        let id = views(&state)[0].id;

        edit_transaction(
            &state,
            TransactionEdit {
                payee: Some("Corner Shop".to_string()),
                ..edit_of(
                    id,
                    "2026-01-10",
                    "Groceries",
                    vec![posting(0, "-25.00"), posting(1, "25.00")],
                )
            },
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

  2026-01-10  *  \"Corner Shop|Groceries\"  ; weekly
  Assets:Bank:Checking            -25.00 USD
  Expenses:Food                    25.00 USD
"
        );
    }

    /// When the edit genuinely forces a form change — a distinct payee where the
    /// source spelled a single literal with no `|` — only the description's
    /// tokens are rewritten: the indent and the trailing comment stay put.
    #[test]
    fn a_header_form_change_keeps_the_indent_and_comment() {
        let dir = TempDir::new("header-form");
        let (_, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

  2026-01-10  *  \"Groceries\"  ; weekly
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD
",
        );
        let state = loaded_state(&path);
        let id = views(&state)[0].id;

        edit_transaction(
            &state,
            TransactionEdit {
                payee: Some("Corner Shop".to_string()),
                ..edit_of(id, "2026-01-10", "Groceries", Vec::new())
            },
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

  2026-01-10  *  \"Corner Shop\" \"Groceries\"  ; weekly
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD
"
        );

        // The rewritten header still describes what the edit asked for: the
        // payee is now explicit and the narration is unchanged.
        let view = views(&state).into_iter().next().unwrap();
        assert_eq!(view.payee.as_deref(), Some("Corner Shop"));
        assert_eq!(view.narration, "Groceries");
    }

    /// A units edit replaces only the figure's bytes: the run of spaces that
    /// aligns the amount under the account survives, as does everything else.
    #[test]
    fn a_units_edit_keeps_column_alignment() {
        let dir = TempDir::new("column-aligned");
        let (_, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking                    -20.00 USD
  Expenses:Food                             20.00 USD
",
        );
        let state = loaded_state(&path);
        let id = view_id(&state, "Groceries");

        edit_transaction(
            &state,
            edit_of(
                id,
                "2026-01-10",
                "Groceries",
                vec![posting(0, "-21.50"), posting(1, "21.50")],
            ),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking                    -21.50 USD
  Expenses:Food                             21.50 USD
"
        );
    }

    #[test]
    fn reports_the_diff_and_commits_what_was_staged() {
        let dir = TempDir::new("commit");
        let (repo, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD
",
        );
        let state = loaded_state(&path);
        // Unrelated work in the same repository must stay out of a ledger commit.
        fs::write(dir.file("notes.txt"), "shopping list\n").unwrap();
        {
            let mut index = repo.index().unwrap();
            index.add_path(Path::new("notes.txt")).unwrap();
            index.write().unwrap();
        }
        edit_transaction(
            &state,
            TransactionEdit {
                flag: "!".to_string(),
                payee: Some("Corner Shop".to_string()),
                ..edit_of(
                    view_id(&state, "Groceries"),
                    "2026-01-10",
                    "Groceries",
                    vec![posting(0, "-25.00"), posting(1, "25.00")],
                )
            },
        )
        .unwrap();

        // The diff view sees the staged change: one block per file, with the
        // old and the new header and units lines in it.
        let diff = ledger_diff_text(&state).unwrap();
        assert!(diff.contains("=== main.bean ===\n"), "diff: {diff}");
        assert!(
            diff.contains("-2026-01-10 * \"Groceries\"\n"),
            "diff: {diff}"
        );
        assert!(
            diff.contains("+2026-01-10 ! \"Corner Shop\" \"Groceries\"\n"),
            "diff: {diff}"
        );
        assert!(
            diff.contains("+  Assets:Bank:Checking            -25.00 USD\n"),
            "diff: {diff}"
        );
        assert!(
            diff.contains("-  Expenses:Food                    20.00 USD\n"),
            "diff: {diff}"
        );

        // An empty message is not a commit.
        assert!(commit_staged(&state, "").is_err());
        assert!(commit_staged(&state, "   ").is_err());

        let short = commit_staged(&state, "groceries went up").unwrap();
        assert_eq!(short.len(), 7, "short id: {short}");
        assert!(
            short.chars().all(|character| character.is_ascii_hexdigit()),
            "short id: {short}"
        );

        // HEAD is that commit, and its tree carries what the file says.
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        assert!(head.id().to_string().starts_with(&short));
        assert_eq!(head.message().unwrap(), "groceries went up");
        assert_eq!(head.parent_count(), 1);
        let entry = head
            .tree()
            .unwrap()
            .get_path(Path::new("main.bean"))
            .unwrap();
        let blob = repo.find_blob(entry.id()).unwrap();
        assert_eq!(blob.content(), fs::read(&path).unwrap().as_slice());
        // The unrelated staged file stayed out of the commit, and out of HEAD —
        // it is still staged, waiting for a commit of its own.
        assert!(head.tree().unwrap().get_path(Path::new("notes.txt")).is_err());
        assert!(
            repo.index()
                .unwrap()
                .get_path(Path::new("notes.txt"), 0)
                .is_some()
        );

        // The ledger has nothing left to commit.
        assert!(commit_staged(&state, "again").is_err());
    }

    /// The ledger the untracked-file tests work on: two transactions, one of
    /// them with a leg whose amount is interpolated at load time.
    const UNTRACKED_LEDGER: &str = "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food

2026-01-12 * \"Rent\"
  Assets:Bank:Checking            -50.00 USD
  Expenses:Food                    50.00 USD
";

    /// A repository that has commits, but not of the ledger — the workspace's
    /// shape while the journal is still untracked.
    fn untracked_fixture(dir: &TempDir) -> (git2::Repository, PathBuf) {
        let path = dir.file("main.bean");
        fs::write(&path, UNTRACKED_LEDGER).unwrap();
        fs::write(dir.file("README.md"), "notes\n").unwrap();
        let repo = repo_with_commit(dir, &["README.md"]);
        (repo, path)
    }

    #[test]
    fn an_untracked_ledger_reports_only_its_own_edits() {
        let dir = TempDir::new("untracked");
        let (_, path) = untracked_fixture(&dir);
        let state = loaded_state(&path);

        // Loading a ledger git has never seen is not a change in itself: the
        // register starts quiet instead of lighting up whole file.
        assert_eq!(changed_narrations(&state), Vec::<String>::new());

        let id = view_id(&state, "Groceries");
        edit_transaction(
            &state,
            edit_of(id, "2026-01-10", "Groceries", vec![posting(0, "-25.00")]),
        )
        .unwrap();

        // The edit — not the file's untracked state — is what shows up, and it is
        // a modification of an entry in a file git has never seen: the file
        // itself is new to git, which is what its diff says too.
        assert_eq!(changed_narrations(&state), ["Groceries"]);
        assert_eq!(
            views(&state)
                .into_iter()
                .find(|view| view.narration == "Groceries")
                .expect("the edited entry is in the register")
                .change,
            Change::Modified
        );
        {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            let listed = journal_file_views(guard.as_ref().unwrap());
            assert_eq!(listed[0].change, Change::Added, "{listed:?}");
        }
        assert_eq!(view_id(&state, "Groceries"), id);
    }

    #[test]
    fn an_untracked_ledger_commits_as_a_new_file() {
        let dir = TempDir::new("untracked-commit");
        let (repo, path) = untracked_fixture(&dir);
        let state = loaded_state(&path);
        edit_transaction(
            &state,
            edit_of(
                view_id(&state, "Groceries"),
                "2026-01-10",
                "Groceries",
                vec![posting(0, "-25.00")],
            ),
        )
        .unwrap();

        // The staged ledger — untracked, so new to HEAD — is in the diff, and
        // its lines read as additions.
        let diff = ledger_diff_text(&state).unwrap();
        assert!(diff.contains("=== main.bean ===\n"), "diff: {diff}");
        assert!(diff.contains("new file mode"), "diff: {diff}");
        assert!(
            diff.contains("+2026-01-01 open Assets:Bank:Checking\n"),
            "diff: {diff}"
        );
        assert!(
            diff.contains("+  Assets:Bank:Checking            -25.00 USD\n"),
            "diff: {diff}"
        );

        let short = commit_staged(&state, "add the journal").unwrap();
        assert_eq!(short.len(), 7, "short id: {short}");
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        assert!(head.id().to_string().starts_with(&short));
        let tree = head.tree().unwrap();
        // The ledger landed in the commit, and the commit did not drop what was
        // already there.
        let entry = tree.get_path(Path::new("main.bean")).unwrap();
        let blob = repo.find_blob(entry.id()).unwrap();
        assert_eq!(blob.content(), fs::read(&path).unwrap().as_slice());
        assert!(tree.get_path(Path::new("README.md")).is_ok());
    }

    #[test]
    fn a_ledger_outside_git_is_still_editable() {
        let dir = TempDir::new("nogit");
        let path = dir.file("main.bean");
        fs::write(
            &path,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food
",
        )
        .unwrap();
        let state = loaded_state(&path);
        let id = view_id(&state, "Groceries");

        // A payee that only repeats the narration is not written down twice.
        edit_transaction(
            &state,
            TransactionEdit {
                payee: Some("Groceries".to_string()),
                ..edit_of(id, "2026-01-10", "Groceries", vec![posting(0, "-25.00")])
            },
        )
        .unwrap();
        // A quote in the narration is escaped, not left to break the header.
        edit_transaction(
            &state,
            edit_of(id, "2026-01-10", "Groceries \"to go\"", Vec::new()),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-10 * \"Groceries \\\"to go\\\"\"
  Assets:Bank:Checking            -25.00 USD
  Expenses:Food
"
        );
        // The escaped narration reads back as the value that was written, and
        // the transaction kept its id.
        assert_eq!(view_id(&state, "Groceries \"to go\""), id);

        // With no repository there is nothing to compare against and nothing to
        // stage — and nothing fails on that account either.
        assert!(changed_narrations(&state).is_empty());
        assert!(ledger_diff_text(&state).is_err());
        assert!(commit_staged(&state, "no repository").is_err());
    }
    #[test]
    fn envelope_budget_accounts_stay_out_of_the_app() {
        let mut fs = rustledger_loader::VirtualFileSystem::new();
        fs.add_file(
            "main.beancount",
            "\
option \"operating_currency\" \"TRY\"

2026-01-01 open Assets:Bank
2026-01-01 open Expenses:Food
2026-01-01 open Equity:Envelope:Groceries
2026-01-01 open Equity:Envelope:To-Be-Budgeted

2026-01-10 * \"Groceries\"
  Assets:Bank                     -50.00 TRY
  Expenses:Food                    50.00 TRY

2026-01-31 * \"Envelope funding 2026-01\" #envelope
  Equity:Envelope:Groceries       100.00 TRY
  Equity:Envelope:To-Be-Budgeted -100.00 TRY

2026-01-31 * \"Envelope activity 2026-01\" #envelope
  Equity:Envelope:Groceries       -50.00 TRY
  Equity:Envelope:To-Be-Budgeted   50.00 TRY
",
        );
        let ledger = rustledger_loader::load_with_fs(
            Path::new("main.beancount"),
            &LoadOptions::default(),
            Box::new(fs),
        )
        .unwrap();
        assert!(ledger.errors.is_empty(), "errors: {:?}", ledger.errors);

        // The generated accounts are the plugin's own bookkeeping: the tree shows
        // neither them nor their money.
        let balances = account_balances(&ledger, &BTreeMap::new());
        let names: Vec<&str> = balances.iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"Assets:Bank"), "{names:?}");
        assert!(!names.iter().any(|name| name.contains("Envelope")), "{names:?}");
        let get = |name: &str| {
            balances
                .iter()
                .find(|b| b.name == name)
                .map(|b| (b.balance.as_str(), b.currency.as_str()))
        };
        assert_eq!(get("Assets:Bank"), Some(("-50.00", "TRY")));
        // Equity only ever held envelope money here, and it is filtered, so no
        // Equity node is left to report.
        assert_eq!(get("Equity"), None);

        // The generated entries never reach the register; the user's own entry
        // does, and keeps its ledger-stream id (the id the editor works from).
        let root = account_transaction_views(&ledger, &HashMap::new(), "");
        let narrations: Vec<&str> = root.iter().map(|t| t.narration.as_str()).collect();
        assert_eq!(narrations, vec!["Groceries"], "{narrations:?}");
        let index = ledger
            .balance_view()
            .iter()
            .position(|directive| {
                matches!(directive, Directive::Transaction(tx) if tx.narration.as_str() == "Groceries")
            })
            .expect("the user's transaction is in the stream");
        assert_eq!(root[0].id, u32::try_from(index).unwrap());
    }

    /// The column the fixtures' posting lines align their amounts at.
    const FIXTURE_COLUMN: usize = 34;

    /// A posting line of the shape the fixtures write: two-space indent, the
    /// account, then spaces out to [`FIXTURE_COLUMN`], then the figure and its
    /// commodity.
    fn aligned(account: &str, units: &str, currency: &str) -> String {
        format!(
            "  {account}{}{units} {currency}",
            " ".repeat(FIXTURE_COLUMN.saturating_sub(2 + account.len()))
        )
    }

    /// A posting of a new entry, as the form sends it.
    fn new_posting(account: &str, units: Option<&str>) -> NewPosting {
        NewPosting {
            account: account.to_string(),
            units: units.map(str::to_string),
            currency: Some("USD".to_string()),
        }
    }

    /// A new entry for `file`, with the fixtures' own shape.
    fn new_entry(file: &Path, date: &str, narration: &str, postings: Vec<NewPosting>) -> NewTransaction {
        NewTransaction {
            file: file.display().to_string(),
            date: date.to_string(),
            flag: "*".to_string(),
            payee: None,
            narration: narration.to_string(),
            postings,
        }
    }

    /// A journal split the way a real one is: a root of `open` directives and
    /// `include`s, and a file of entries it pulls in — the shape the file picker
    /// exists for. The entries file aligns its amounts at [`FIXTURE_COLUMN`].
    fn split_fixture(dir: &TempDir) -> (git2::Repository, PathBuf, PathBuf) {
        let root = dir.file("main.bean");
        let entries = dir.file("2026.bean");
        fs::write(
            &root,
            "\
option \"operating_currency\" \"USD\"

include \"2026.bean\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food
2026-01-01 open Expenses:Coffee
",
        )
        .unwrap();
        fs::write(
            &entries,
            format!(
                "2026-01-10 * \"Groceries\"\n{}\n{}\n",
                aligned("Assets:Bank:Checking", "-20.00", "USD"),
                aligned("Expenses:Food", "20.00", "USD"),
            ),
        )
        .unwrap();
        let repo = repo_with_commit(dir, &["main.bean", "2026.bean"]);
        (repo, root, entries)
    }

    /// A new entry lands in the file it names, at the end, laid out like that
    /// file's own postings — through the same reload, refusal and staging path as
    /// every other edit.
    #[test]
    fn adds_a_transaction_to_the_chosen_file() {
        let dir = TempDir::new("add");
        let (repo, root, entries) = split_fixture(&dir);
        let state = loaded_state(&root);

        // The picker's view of the journal: the root journal first, both files
        // named relative to the root's directory.
        {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            let files = journal_file_views(guard.as_ref().unwrap());
            assert_eq!(
                files
                    .iter()
                    .map(|file| (file.label.as_str(), file.path.as_str()))
                    .collect::<Vec<_>>(),
                [
                    ("main.bean", root.display().to_string().as_str()),
                    ("2026.bean", entries.display().to_string().as_str()),
                ]
            );
        }

        create_transaction(
            &state,
            NewTransaction {
                payee: Some("Corner Shop".to_string()),
                ..new_entry(
                    &entries,
                    "2026-01-14",
                    "Coffee",
                    vec![
                        new_posting("Assets:Bank:Checking", Some("-16.20")),
                        new_posting("Expenses:Coffee", Some("16.20")),
                    ],
                )
            },
        )
        .unwrap();

        // The file the entry named gained the entry and nothing else: one blank
        // line separates it from what was there, and its postings line up with
        // the file's own amounts.
        let mut expected = String::from("2026-01-10 * \"Groceries\"\n");
        expected.push_str(&aligned("Assets:Bank:Checking", "-20.00", "USD"));
        expected.push('\n');
        expected.push_str(&aligned("Expenses:Food", "20.00", "USD"));
        expected.push_str("\n\n2026-01-14 * \"Corner Shop\" \"Coffee\"\n");
        expected.push_str(&aligned("Assets:Bank:Checking", "-16.20", "USD"));
        expected.push('\n');
        expected.push_str(&aligned("Expenses:Coffee", "16.20", "USD"));
        expected.push('\n');
        assert_eq!(fs::read_to_string(&entries).unwrap(), expected);
        // The root journal is untouched: the entry named its own file.
        assert!(fs::read_to_string(&root).unwrap().ends_with("open Expenses:Coffee\n"));

        // The ledger the app now holds is clean, and the new entry is in the
        // register with both legs, the only row reporting itself modified.
        {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            let errors = &guard.as_ref().unwrap().ledger.errors;
            assert!(errors.is_empty(), "errors: {errors:?}");
        }
        let coffee = views(&state)
            .into_iter()
            .find(|view| view.narration == "Coffee")
            .expect("the new entry is in the register");
        // Appended, so its lines are all new: the entry reads as added, where the
        // file it went into — a file git has — reads as modified.
        assert_eq!(coffee.change, Change::Added);
        assert_eq!(coffee.date, "2026-01-14");
        assert_eq!(coffee.payee.as_deref(), Some("Corner Shop"));
        assert_eq!(
            coffee
                .postings
                .iter()
                .map(|posting| (posting.account.as_str(), posting.units.as_str()))
                .collect::<Vec<_>>(),
            [
                ("Assets:Bank:Checking", "-16.20"),
                ("Expenses:Coffee", "16.20"),
            ]
        );
        assert_eq!(changed_narrations(&state), ["Coffee"]);

        // The file is staged, and only it.
        let head = repo.head().unwrap().peel_to_tree().unwrap();
        let staged = repo.diff_tree_to_index(Some(&head), None, None).unwrap();
        assert_eq!(
            staged
                .deltas()
                .map(|delta| delta.new_file().path().unwrap().display().to_string())
                .collect::<Vec<_>>(),
            ["2026.bean"]
        );
    }

    /// A posting left without an amount is the leg beancount interpolates: the
    /// account is written alone, and the reload books it.
    #[test]
    fn a_posting_without_an_amount_is_left_to_be_interpolated() {
        let dir = TempDir::new("add-implicit");
        let (_, root, entries) = split_fixture(&dir);
        let state = loaded_state(&root);

        create_transaction(
            &state,
            new_entry(
                &entries,
                "2026-01-15",
                "Coffee",
                vec![
                    new_posting("Assets:Bank:Checking", Some("-4.00")),
                    new_posting("Expenses:Coffee", None),
                ],
            ),
        )
        .unwrap();

        // The implied leg is just the account: no figure, no commodity.
        let written = fs::read_to_string(&entries).unwrap();
        assert!(
            written.ends_with("2026-01-15 * \"Coffee\"\n  Assets:Bank:Checking            -4.00 USD\n  Expenses:Coffee\n"),
            "entries: {written}"
        );
        let coffee = views(&state)
            .into_iter()
            .find(|view| view.narration == "Coffee")
            .expect("the new entry is in the register");
        assert_eq!(
            coffee
                .postings
                .iter()
                .map(|posting| (posting.account.as_str(), posting.units.as_str()))
                .collect::<Vec<_>>(),
            [
                ("Assets:Bank:Checking", "-4.00"),
                ("Expenses:Coffee", "4.00"),
            ]
        );
    }

    /// An entry the ledger refuses — legs that do not balance, or a file the
    /// journal does not hold — is not written: the file keeps every byte it had,
    /// and the app keeps the ledger it had.
    #[test]
    fn an_entry_the_loader_refuses_is_not_written() {
        let dir = TempDir::new("add-refused");
        let (_, root, entries) = split_fixture(&dir);
        let state = loaded_state(&root);
        let before = fs::read(&entries).unwrap();

        let unbalanced = create_transaction(
            &state,
            new_entry(
                &entries,
                "2026-01-14",
                "Coffee",
                vec![
                    new_posting("Assets:Bank:Checking", Some("-3.50")),
                    new_posting("Expenses:Coffee", Some("3.00")),
                ],
            ),
        );
        assert!(unbalanced.is_err(), "an unbalanced entry went through");

        let elsewhere = create_transaction(
            &state,
            new_entry(
                Path::new("/tmp/elsewhere.bean"),
                "2026-01-14",
                "Coffee",
                vec![
                    new_posting("Assets:Bank:Checking", Some("-3.50")),
                    new_posting("Expenses:Coffee", Some("3.50")),
                ],
            ),
        );
        assert!(
            elsewhere.unwrap_err().contains("not a file of this journal"),
            "a path outside the journal was written to"
        );

        assert_eq!(fs::read(&entries).unwrap(), before);
        assert!(views(&state).iter().all(|view| view.narration != "Coffee"));
        assert_eq!(changed_narrations(&state), Vec::<String>::new());
    }

    /// The default file follows the accounts: the file of the newest entry that
    /// already touches one of them, whichever side of the tree it is on — and the
    /// root journal when nothing does.
    #[test]
    fn the_default_file_follows_the_accounts_newest_entry() {
        let dir = TempDir::new("suggest");
        let root = dir.file("main.bean");
        let older = dir.file("2025.bean");
        let newer = dir.file("2026.bean");
        fs::write(
            &root,
            "\
option \"operating_currency\" \"USD\"

include \"2025.bean\"
include \"2026.bean\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food
2026-01-01 open Income:Salary
",
        )
        .unwrap();
        fs::write(
            &older,
            format!(
                "2025-12-30 * \"Groceries\"\n{}\n{}\n",
                aligned("Assets:Bank:Checking", "-20.00", "USD"),
                aligned("Expenses:Food", "20.00", "USD"),
            ),
        )
        .unwrap();
        fs::write(
            &newer,
            format!(
                "2026-01-10 * \"Groceries\"\n{}\n{}\n\n2026-01-12 * \"Salary\"\n{}\n{}\n",
                aligned("Assets:Bank:Checking", "-20.00", "USD"),
                aligned("Expenses:Food", "20.00", "USD"),
                aligned("Assets:Bank:Checking", "200.00", "USD"),
                aligned("Income:Salary", "-200.00", "USD"),
            ),
        )
        .unwrap();
        let state = loaded_state(&root);

        // What the app's own strategy answers, as `suggest_transaction_file`
        // hands it on — the root fallback is the command's, not the rule's.
        let suggested = |accounts: &[&str]| -> Option<String> {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            let loaded = guard.as_ref().unwrap();
            let names: Vec<String> = accounts.iter().map(|name| name.to_string()).collect();
            let index = default_file(&FILE_SUGGESTION, &loaded.ledger, &names)?;
            Some(loaded.ledger.source_map.files()[index].path.display().to_string())
        };

        // Both files hold an entry on Expenses:Food; the newer one wins.
        assert_eq!(suggested(&["Expenses:Food"]), Some(newer.display().to_string()));
        // And it is the accounts that decide, not the journal's newest entry:
        // the salary of 01-12 does not touch Expenses:Food.
        assert_eq!(suggested(&["Income:Salary"]), Some(newer.display().to_string()));
        // An entry on the account's parent, or its child, is on the same branch.
        assert_eq!(
            suggested(&["Expenses:Food:Groceries"]),
            Some(newer.display().to_string())
        );
        assert_eq!(
            suggested(&["Assets:Bank"]),
            Some(newer.display().to_string())
        );
        // Nothing touches an account the ledger has never seen, let alone the
        // ones no entry mentions: the rule answers nothing and the app falls back
        // to the journal it was opened with.
        assert_eq!(suggested(&["Expenses:Coffee"]), Some(root.display().to_string()));
        assert_eq!(suggested(&[]), Some(root.display().to_string()));
        {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            let loaded = guard.as_ref().unwrap();
            assert_eq!(
                RecentAccountActivity.suggest(&loaded.ledger, &["Expenses:Coffee".to_string()]),
                None
            );
        }
    }

    /// An account is appended as an `open` directive, at the end of the file it
    /// names, and lands in the tree — an account opened and not yet posted to is
    /// the one a user is about to use.
    #[test]
    fn adds_an_account_to_the_chosen_file() {
        let dir = TempDir::new("add-account");
        let (repo, root, entries) = split_fixture(&dir);
        let state = loaded_state(&root);
        let before = fs::read(&entries).unwrap();

        create_account(
            &state,
            NewAccount {
                file: entries.display().to_string(),
                name: "Expenses:Tea".to_string(),
                date: "2026-01-11".to_string(),
                currencies: vec!["USD".to_string()],
            },
        )
        .unwrap();

        // The directive is appended, separated by one blank line, and nothing
        // else in the file moved.
        let written = fs::read_to_string(&entries).unwrap();
        assert!(
            written.ends_with("\n\n2026-01-11 open Expenses:Tea USD\n"),
            "entries: {written}"
        );
        assert!(written.starts_with(&String::from_utf8(before).unwrap()));

        // The account is in the tree with the commodity it was opened for and
        // nothing in it; one opened without commodities reports an empty one.
        let balances = {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            let loaded = guard.as_ref().unwrap();
            assert!(loaded.ledger.errors.is_empty(), "errors: {:?}", loaded.ledger.errors);
            let changes = account_changes(&loaded.ledger, &loaded.baseline);
            account_balances(&loaded.ledger, &changes)
        };
        let held = |name: &str| {
            balances
                .iter()
                .find(|balance| balance.name == name)
                .map(|balance| (balance.balance.as_str(), balance.currency.as_str()))
                .unwrap_or_else(|| panic!("missing {name} in {balances:?}"))
        };
        assert_eq!(held("Expenses:Tea"), ("0", "USD"));
        assert_eq!(held("Expenses:Coffee"), ("0", ""));
        // An account that has postings keeps the commodity its postings put
        // there, whatever it was opened for.
        assert_eq!(held("Expenses:Food"), ("20.00", "USD"));
        // Opening an account is a change to that account and to its ancestors,
        // on top of the change to the file it was written into.
        let change = |name: &str| {
            balances
                .iter()
                .find(|balance| balance.name == name)
                .map(|balance| balance.change)
                .unwrap_or_else(|| panic!("missing {name} in {balances:?}"))
        };
        assert_eq!(change("Expenses:Tea"), Change::Added);
        assert_eq!(change("Expenses"), Change::Added);
        assert_eq!(change("Expenses:Food"), Change::Unchanged);
        assert_eq!(change("Assets:Bank:Checking"), Change::Unchanged);

        // Only the file it went into is staged.
        let head = repo.head().unwrap().peel_to_tree().unwrap();
        let staged = repo.diff_tree_to_index(Some(&head), None, None).unwrap();
        assert_eq!(
            staged
                .deltas()
                .map(|delta| delta.new_file().path().unwrap().display().to_string())
                .collect::<Vec<_>>(),
            ["2026.bean"]
        );

        // Opening an account the ledger already has is the loader's to refuse,
        // and a refusal leaves the file byte-for-byte as it was.
        let before = fs::read(&entries).unwrap();
        let duplicate = create_account(
            &state,
            NewAccount {
                file: entries.display().to_string(),
                name: "Expenses:Coffee".to_string(),
                date: "2026-01-11".to_string(),
                currencies: Vec::new(),
            },
        );
        assert!(duplicate.is_err(), "an account was opened twice");
        assert_eq!(fs::read(&entries).unwrap(), before);

        // A name that is not one, and a file the journal does not hold, are
        // refused before anything is written.
        assert!(create_account(
            &state,
            NewAccount {
                file: entries.display().to_string(),
                name: "Expenses:Two Words".to_string(),
                date: "2026-01-11".to_string(),
                currencies: Vec::new(),
            },
        )
        .is_err());
        assert!(create_account(
            &state,
            NewAccount {
                file: "/tmp/elsewhere.bean".to_string(),
                name: "Expenses:Tea".to_string(),
                date: "2026-01-11".to_string(),
                currencies: Vec::new(),
            },
        )
        .unwrap_err()
        .contains("not a file of this journal"));
        assert_eq!(fs::read(&entries).unwrap(), before);
    }

    /// The default file for a new account follows its family: the file that opens
    /// the nearest existing ancestor first, then the file of the newest entry on
    /// an ancestor's branch, then the journal itself.
    #[test]
    fn the_account_default_follows_the_family() {
        let dir = TempDir::new("suggest-account");
        let root = dir.file("main.bean");
        let older = dir.file("2025.bean");
        let newer = dir.file("2026.bean");
        fs::write(
            &root,
            "\
option \"operating_currency\" \"USD\"

include \"2025.bean\"
include \"2026.bean\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food
",
        )
        .unwrap();
        fs::write(&older, "2025-01-01 open Expenses:Food:Groceries\n").unwrap();
        fs::write(
            &newer,
            format!(
                "2026-01-10 * \"Groceries\"\n{}\n{}\n",
                aligned("Assets:Bank:Checking", "-20.00", "USD"),
                aligned("Expenses:Food", "20.00", "USD"),
            ),
        )
        .unwrap();
        let state = loaded_state(&root);

        let suggested = |name: &str| -> Option<String> {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            let loaded = guard.as_ref().unwrap();
            let index = default_file(&ACCOUNT_SUGGESTION, &loaded.ledger, &[name.to_string()])?;
            Some(loaded.ledger.source_map.files()[index].path.display().to_string())
        };
        let rule = |name: &str| -> Option<usize> {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            AccountFamily.suggest(&guard.as_ref().unwrap().ledger, &[name.to_string()])
        };

        // The nearest ancestor's own `open` wins, even though the family's
        // newest entry lives in another file.
        assert_eq!(
            suggested("Expenses:Food:Groceries:Organic"),
            Some(older.display().to_string())
        );
        // With no `open` to go by, the family's entries decide.
        assert_eq!(suggested("Expenses:Coffee"), Some(newer.display().to_string()));
        // Nothing opens or posts to the branch: the rule answers nothing and the
        // app falls back to the journal it was opened with.
        assert_eq!(rule("Liabilities:Card"), None);
        assert_eq!(suggested("Liabilities:Card"), Some(root.display().to_string()));
        assert_eq!(suggested(""), Some(root.display().to_string()));
    }

    /// The file explorer's own view of the journal: what each file holds, and
    /// which of them are uncommitted.
    #[test]
    fn the_file_views_report_what_each_file_holds() {
        let dir = TempDir::new("file-views");
        let (_, root, entries) = split_fixture(&dir);
        let state = loaded_state(&root);

        let files = |state: &AppState| {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            journal_file_views(guard.as_ref().unwrap())
        };
        let listed = files(&state);
        assert_eq!(listed.len(), 2);
        assert_eq!(
            listed
                .iter()
                .map(|file| (file.label.as_str(), file.transactions, file.change))
                .collect::<Vec<_>>(),
            [
                ("main.bean", 0, Change::Unchanged),
                ("2026.bean", 1, Change::Unchanged)
            ]
        );

        // A file's own transactions, in the register's own row shape — the ids
        // are the register's, so an entry opened from here edits and stages like
        // any other.
        let rows = {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            let loaded = guard.as_ref().unwrap();
            file_transaction_views(&loaded.ledger, &loaded.baseline, &entries.display().to_string())
                .unwrap()
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].narration, "Groceries");
        assert_eq!(rows[0].postings.len(), 2);
        assert_eq!(
            rows[0].id,
            views(&state)
                .into_iter()
                .find(|view| view.narration == "Groceries")
                .expect("the entry is in the register")
                .id
        );

        // The root journal writes no entries; an unknown path is refused rather
        // than answered with an empty list.
        {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            let loaded = guard.as_ref().unwrap();
            assert!(file_transaction_views(
                &loaded.ledger,
                &loaded.baseline,
                &root.display().to_string()
            )
            .unwrap()
            .is_empty());
            assert!(file_transaction_views(&loaded.ledger, &loaded.baseline, "/tmp/elsewhere.bean")
                .is_err());
        }

        // An edit marks the file it went into, and only it.
        edit_transaction(
            &state,
            edit_of(
                view_id(&state, "Groceries"),
                "2026-01-10",
                "Groceries",
                vec![posting(0, "-25.00"), posting(1, "25.00")],
            ),
        )
        .unwrap();
        let listed = files(&state);
        assert_eq!(
            listed
                .iter()
                .map(|file| (file.label.as_str(), file.transactions, file.change))
                .collect::<Vec<_>>(),
            [
                // The file is in the last commit, so an edit to it is a
                // modification of it — where the entry inside carries its own
                // kind of change.
                ("main.bean", 0, Change::Unchanged),
                ("2026.bean", 1, Change::Modified)
            ]
        );
    }

    /// The whole-file text path — reading a file, feeding it back unchanged, and
    /// writing a changed one — goes through the same loader and staging pipeline
    /// as every other edit.
    #[test]
    fn a_file_is_edited_as_whole_text() {
        let dir = TempDir::new("file-text");
        let (repo, root, entries) = split_fixture(&dir);
        let state = loaded_state(&root);
        let path = entries.display().to_string();
        let before = fs::read(&entries).unwrap();
        let head = repo.head().unwrap().peel_to_tree().unwrap();

        let text = {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            let loaded = guard.as_ref().unwrap();
            source_of_file(&loaded.ledger, &path).unwrap().source.to_string()
        };
        assert!(text.contains("2026-01-10 * \"Groceries\""));

        // The same text back is no edit at all: no bytes written, nothing staged.
        edit_named_file_source(&state, &path, &text).unwrap();
        assert_eq!(fs::read(&entries).unwrap(), before);
        assert_eq!(repo.index().unwrap().write_tree().unwrap(), head.id());

        // A file written with one more entry is staged, and the app holds the
        // ledger it now describes.
        let rewritten = format!(
            "{text}\n2026-01-20 * \"Coffee\"\n{}\n{}\n",
            aligned("Assets:Bank:Checking", "-3.00", "USD"),
            aligned("Expenses:Coffee", "3.00", "USD"),
        );
        edit_named_file_source(&state, &path, &rewritten).unwrap();
        assert_eq!(fs::read_to_string(&entries).unwrap(), rewritten);
        assert_eq!(
            views(&state)
                .into_iter()
                .filter(|view| view.narration == "Coffee")
                .count(),
            1
        );
        let staged = repo.diff_tree_to_index(Some(&head), None, None).unwrap();
        assert_eq!(
            staged
                .deltas()
                .map(|delta| delta.new_file().path().unwrap().display().to_string())
                .collect::<Vec<_>>(),
            ["2026.bean"]
        );

        // Text the ledger refuses leaves the file byte-for-byte as it was, and a
        // path the journal does not hold is refused without reading anything.
        let before = fs::read(&entries).unwrap();
        assert!(edit_named_file_source(
            &state,
            &path,
            "2026-01-21 * \"Coffee\"\n  Assets:Bank:Checking            -3.00 USD\n"
        )
        .is_err());
        assert_eq!(fs::read(&entries).unwrap(), before);
        assert!(edit_named_file_source(&state, "/tmp/elsewhere.bean", "whatever\n")
            .unwrap_err()
            .contains("not a file of this journal"));
    }

    /// A group's own label with its totals, then one row per envelope — the
    /// shape the budget screen renders, as comparable strings.
    fn group_rows(group: &BudgetGroup) -> Vec<(&str, &str, &str, &str)> {
        let mut rows = vec![(
            group.name.as_str(),
            group.budgeted.as_str(),
            group.spent.as_str(),
            group.balance.as_str(),
        )];
        rows.extend(group.envelopes.iter().map(|envelope| {
            (
                envelope.name.as_str(),
                envelope.budgeted.as_str(),
                envelope.spent.as_str(),
                envelope.balance.as_str(),
            )
        }));
        rows
    }

    /// Every group's rows, as the screen reads them.
    fn budget_rows(month: &BudgetMonth) -> Vec<Vec<(&str, &str, &str, &str)>> {
        month.groups.iter().map(group_rows).collect()
    }

    /// A ledger holding what the envelope plugin writes for two months, in the
    /// shape `emit.rs` gives it: one entry per step of the month, dated the
    /// month's last day, tagged `#envelope`, with `envelope-kind` and
    /// `envelope-month` metadata, every posting inside `Equity:Envelope:`.
    ///
    /// Food is funded 300 a month (plus a 20 adjustment in January), and its
    /// spending is recorded on `Expenses:Food:Bread`, so the funding lands on
    /// the parent envelope while the activity lands on the child — the case a
    /// group total exists for. Fun is reset by its rollover at the end of
    /// February, which sweeps its 80 balance back to the pool.
    fn envelope_fixture() -> &'static str {
        "\
option \"operating_currency\" \"TRY\"

2026-01-01 open Assets:Bank
2026-01-01 open Expenses:Food
2026-01-01 open Expenses:Food:Bread
2026-01-01 open Expenses:Fun
2026-01-01 open Equity:Envelope:Food
2026-01-01 open Equity:Envelope:Food:Bread
2026-01-01 open Equity:Envelope:Fun
2026-01-01 open Equity:Envelope:Money
2026-01-01 open Equity:Envelope:To-Be-Budgeted

2026-01-15 * \"Bakery\"
  Assets:Bank                     -50.00 TRY
  Expenses:Food:Bread              50.00 TRY

2026-01-31 * \"Envelope backing 2026-01\" #envelope
  envelope-kind: \"Envelope backing\"
  envelope-month: \"2026-01\"
  Equity:Envelope:Money         -1000.00 TRY
  Equity:Envelope:To-Be-Budgeted 1000.00 TRY

2026-01-31 * \"Envelope funding 2026-01\" #envelope
  envelope-kind: \"Envelope funding\"
  envelope-month: \"2026-01\"
  Equity:Envelope:Food             300.00 TRY
  Equity:Envelope:Fun              200.00 TRY
  Equity:Envelope:To-Be-Budgeted  -500.00 TRY

2026-01-31 * \"Envelope activity 2026-01\" #envelope
  envelope-kind: \"Envelope activity\"
  envelope-month: \"2026-01\"
  Equity:Envelope:Food:Bread       -50.00 TRY
  Equity:Envelope:Fun              -30.00 TRY
  Equity:Envelope:To-Be-Budgeted    80.00 TRY

2026-01-31 * \"Envelope adjustment 2026-01\" #envelope
  envelope-kind: \"Envelope adjustment\"
  envelope-month: \"2026-01\"
  Equity:Envelope:Food              20.00 TRY
  Equity:Envelope:To-Be-Budgeted   -20.00 TRY

2026-02-28 * \"Envelope funding 2026-02\" #envelope
  envelope-kind: \"Envelope funding\"
  envelope-month: \"2026-02\"
  Equity:Envelope:Food             300.00 TRY
  Equity:Envelope:Fun              200.00 TRY
  Equity:Envelope:To-Be-Budgeted  -500.00 TRY

2026-02-28 * \"Envelope activity 2026-02\" #envelope
  envelope-kind: \"Envelope activity\"
  envelope-month: \"2026-02\"
  Equity:Envelope:Food:Bread       -80.00 TRY
  Equity:Envelope:Fun             -120.00 TRY
  Equity:Envelope:To-Be-Budgeted   200.00 TRY

2026-02-28 * \"Envelope rollover 2026-02\" #envelope
  envelope-kind: \"Envelope rollover\"
  envelope-month: \"2026-02\"
  Equity:Envelope:Fun              -80.00 TRY
  Equity:Envelope:To-Be-Budgeted    80.00 TRY
"
    }

    /// The budget opens on the newest month the plugin budgeted, and reads the
    /// month's figures per envelope and per group, with the pool of unassigned
    /// money underneath.
    #[test]
    fn budget_reads_the_newest_envelope_month() {
        let dir = TempDir::new("budget-month");
        let (_, path) = committed_fixture(&dir, envelope_fixture());
        let state = loaded_state(&path);
        let month = budget_view(&state, None).unwrap();

        assert_eq!(month.month, "2026-02");
        assert_eq!(month.months, ["2026-01", "2026-02"]);
        assert_eq!(month.currency, "TRY");
        // The pool: 1000 backed in, 500 + 500 funded, 80 + 120 + 200 spent
        // activity back, the adjustment's 20 and the sweep's 80 out.
        assert_eq!(month.to_budget, "340.00");
        assert_eq!(
            budget_rows(&month),
            vec![
                vec![
                    // The group's own envelope holds the funding, its child the
                    // spending, so only the group row is the whole story.
                    ("Food", "300.00", "80.00", "490.00"),
                    ("Food", "300.00", "0", "620.00"),
                    ("Bread", "0", "80.00", "-130.00"),
                ],
                vec![
                    // The rollover's 80 goes back to the pool, so the month
                    // budgeted Fun 120 of its 200, not all of it.
                    ("Fun", "120.00", "120.00", "170.00"),
                    ("Fun", "120.00", "120.00", "170.00"),
                ],
            ]
        );
    }

    /// A requested month is read on its own terms: its own flows, and the
    /// balance every earlier month carries into it.
    #[test]
    fn budget_reads_any_budget_month() {
        let dir = TempDir::new("budget-any-month");
        let (_, path) = committed_fixture(&dir, envelope_fixture());
        let state = loaded_state(&path);

        let january = budget_view(&state, Some("2026-01")).unwrap();
        assert_eq!(january.month, "2026-01");
        assert_eq!(january.months, ["2026-01", "2026-02"]);
        assert_eq!(january.to_budget, "560.00");
        assert_eq!(
            budget_rows(&january),
            vec![
                vec![
                    ("Food", "320.00", "50.00", "270.00"),
                    ("Food", "320.00", "0", "320.00"),
                    ("Bread", "0", "50.00", "-50.00"),
                ],
                vec![
                    ("Fun", "200.00", "30.00", "170.00"),
                    ("Fun", "200.00", "30.00", "170.00"),
                ],
            ]
        );

        // A month the plugin never budgeted carries the balances it ended with
        // and moves nothing of its own.
        let march = budget_view(&state, Some("2026-03")).unwrap();
        assert_eq!(march.month, "2026-03");
        assert_eq!(march.to_budget, "340.00");
        assert_eq!(
            budget_rows(&march),
            vec![
                vec![
                    ("Food", "0", "0", "490.00"),
                    ("Food", "0", "0", "620.00"),
                    ("Bread", "0", "0", "-130.00"),
                ],
                vec![("Fun", "0", "0", "170.00"), ("Fun", "0", "0", "170.00")],
            ]
        );

        // Before the first budget month there is nothing to carry: an empty
        // month rather than a made-up one.
        let before = budget_view(&state, Some("2025-12")).unwrap();
        assert!(before.groups.is_empty());
        assert_eq!(before.to_budget, "0");

        // A label that is not a month is refused rather than guessed at.
        for bad in ["2026-1", "2026-13", "junk", ""] {
            assert!(budget_view(&state, Some(bad)).is_err(), "`{bad}` must not read");
        }
    }

    /// The budget's two refusals: nothing loaded, and a ledger the plugin has
    /// not budgeted — an empty month would claim something the ledger does not.
    #[test]
    fn budget_refuses_a_ledger_it_cannot_read() {
        assert_eq!(
            budget_view(&AppState::default(), None).unwrap_err(),
            "no ledger loaded"
        );

        let dir = TempDir::new("budget-none");
        let (_, path) = committed_fixture(
            &dir,
            "option \"operating_currency\" \"USD\"\n\n2026-01-01 open Assets:Bank\n",
        );
        let state = loaded_state(&path);
        let error = budget_view(&state, None).unwrap_err();
        assert!(error.contains("Equity:Envelope:"), "{error}");
    }

    /// A hand-written entry in the namespace carries no metadata, so the sign of
    /// its postings decides what it is: money into an envelope is funding, money
    /// out of it is spending.
    #[test]
    fn budget_reads_envelope_entries_without_metadata() {
        let dir = TempDir::new("budget-plain");
        let (_, path) = committed_fixture(
            &dir,
            "\
option \"operating_currency\" \"TRY\"

2026-01-01 open Assets:Bank
2026-01-01 open Expenses:Food
2026-01-01 open Equity:Envelope:Groceries
2026-01-01 open Equity:Envelope:To-Be-Budgeted

2026-01-10 * \"Groceries\"
  Assets:Bank                     -50.00 TRY
  Expenses:Food                    50.00 TRY

2026-01-31 * \"Envelope funding 2026-01\" #envelope
  Equity:Envelope:Groceries       100.00 TRY
  Equity:Envelope:To-Be-Budgeted -100.00 TRY

2026-01-31 * \"Envelope activity 2026-01\" #envelope
  Equity:Envelope:Groceries       -50.00 TRY
  Equity:Envelope:To-Be-Budgeted   50.00 TRY
",
        );
        let state = loaded_state(&path);
        let month = budget_view(&state, None).unwrap();
        assert_eq!(month.month, "2026-01");
        assert_eq!(month.months, ["2026-01"]);
        // No backing entry in this ledger, so the pool is short by the 50 that
        // was already spent when it was funded.
        assert_eq!(month.to_budget, "-50.00");
        assert_eq!(
            budget_rows(&month),
            vec![vec![
                ("Groceries", "100.00", "50.00", "50.00"),
                ("Groceries", "100.00", "50.00", "50.00"),
            ]]
        );
    }

    /// The budget is the only surface that reads the envelope accounts: the
    /// tree, the balances and the register still hide them, metadata or not.
    #[test]
    fn envelope_accounts_stay_out_of_the_other_surfaces() {
        let dir = TempDir::new("budget-hidden");
        let (_, path) = committed_fixture(&dir, envelope_fixture());
        let state = loaded_state(&path);

        let balances = {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            account_balances(&guard.as_ref().unwrap().ledger, &BTreeMap::new())
        };
        let names: Vec<&str> = balances.iter().map(|balance| balance.name.as_str()).collect();
        // Equity holds nothing but envelope money here, so the branch that would
        // report it has nothing left to report.
        assert_eq!(
            names,
            [
                "Assets",
                "Assets:Bank",
                "Expenses",
                "Expenses:Food",
                "Expenses:Food:Bread",
                "Expenses:Fun",
            ]
        );
        let bank = balances
            .iter()
            .find(|balance| balance.name == "Assets:Bank")
            .unwrap();
        assert_eq!((bank.balance.as_str(), bank.currency.as_str()), ("-50.00", "TRY"));

        // The plugin's entries never reach the register; the user's own does.
        let shown: Vec<String> = views(&state).into_iter().map(|view| view.narration).collect();
        assert_eq!(shown, ["Bakery"]);
    }
}
