// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use specta_typescript::Typescript;
use tauri_specta::{Builder, collect_commands};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, RwLock};

use rustledger_core::{Decimal, Directive, IncompleteAmount, Span, Spanned, Transaction};
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
    /// Whether the transaction's own lines differ from the ledger's last commit,
    /// staged or not. False everywhere when the ledger is not in a git
    /// repository or nothing changed — the register never fails over git.
    pub modified: bool,
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

/// A loaded ledger together with the baseline the register's `modified` flags
/// are measured against.
struct Loaded {
    ledger: Ledger,
    /// Every file's text as this session first saw it, keyed by path. It stands
    /// in wherever git has no HEAD blob for the file: a ledger that is not
    /// committed yet — a fresh journal, a directory that was never added — is
    /// not a change in itself, only the edits made to it are.
    baseline: HashMap<PathBuf, Arc<str>>,
}

impl Loaded {
    /// Wrap a freshly loaded ledger, recording what its files look like now.
    fn fresh(ledger: Ledger) -> Self {
        let mut loaded = Self {
            ledger,
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
        .map(Loaded::fresh)
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
        .map(|loaded| account_balances(&loaded.ledger))
        .unwrap_or_default()
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
    let prefix = format!("{account}:");
    let root = account.is_empty();
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
            let touches = root
                || tx.postings.iter().any(|posting| {
                    let name = posting.value.account.as_str();
                    name == account || name.starts_with(&prefix)
                });
            if !touches {
                return None;
            }
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
            let modified = sources
                .as_ref()
                .and_then(|sources| sources.get(id).copied().flatten())
                .is_some_and(|index| {
                    touches_changes(ledger, &changed, &mut line_starts, &ledger.directives[index])
                });
            Some(TransactionView {
                id: u32::try_from(id).unwrap_or(u32::MAX),
                date: tx.date.to_string(),
                flag: tx.flag.to_string(),
                payee: tx.payee.as_ref().map(|p| p.as_str().to_owned()),
                narration: tx.narration.as_str().to_owned(),
                postings,
                modified,
            })
        })
        .collect();
    // Stable sort: descending date, ties keep their ledger order.
    views.sort_by(|a, b| b.date.cmp(&a.date));
    views
}

/// Accumulate per-(account, currency) units over the pad-merged transaction
/// stream, roll each account's totals up into its ancestors, then report one
/// entry per tree node.
fn account_balances(ledger: &Ledger) -> Vec<AccountBalance> {
    let mut totals: HashMap<String, HashMap<String, Decimal>> = HashMap::new();
    for directive in ledger.balance_view() {
        let Directive::Transaction(tx) = directive else {
            continue;
        };
        for posting in &tx.postings {
            let Some(IncompleteAmount::Complete(amount)) = &posting.value.units else {
                // Residual/interpolated postings never occur post-booking.
                continue;
            };
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

    fs::write(path, rewritten.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
    let reloaded = match rustledger_loader::load(path, &LoadOptions::default()) {
        Ok(ledger) => ledger,
        Err(e) => {
            let _ = fs::write(path, &previous_bytes);
            return Err(format!("{}: {e}", path.display()));
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
            loaded.remember_files();
        }
        None => *guard = Some(Loaded::fresh(reloaded)),
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

/// Which lines of the ledger's files differ from what they are measured
/// against, keyed by source-map file id — the measure behind
/// [`TransactionView::modified`].
///
/// A file's reference is its HEAD blob when git has one, and otherwise the text
/// the session started with (see [`Loaded::baseline`]): a ledger that is not
/// committed yet is not a change in itself. Empty when the ledger is not in a
/// repository or nothing differs.
fn working_tree_changes(
    ledger: &Ledger,
    baseline: &HashMap<PathBuf, Arc<str>>,
) -> HashMap<u16, Vec<(u32, u32)>> {
    let mut changed: HashMap<u16, Vec<(u32, u32)>> = HashMap::new();
    for file in ledger.source_map.files() {
        let Some(id) = u16::try_from(file.id).ok() else {
            continue;
        };
        let Some((repo, relative)) = repo_and_relative(&file.path) else {
            continue;
        };
        let lines = match head_blob(&repo, &relative) {
            // Committed: the working tree is compared against HEAD.
            Some(head) => {
                if head == *file.source {
                    continue;
                }
                changed_lines(&head, &file.source)
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
                changed_lines(start, &file.source)
            }
        };
        if !lines.is_empty() {
            changed.insert(id, lines);
        }
    }
    changed
}

/// The 1-based lines of `new` that `old` does not have, merged into inclusive
/// ranges. libgit2's own line diff drives this, so rewrites, insertions and
/// deletions are all covered; a deleted line has no number of its own in the new
/// file, so the line that takes its place is the one reported.
fn changed_lines(old: &str, new: &str) -> Vec<(u32, u32)> {
    let Ok(patch) = git2::Patch::from_buffers(
        old.as_bytes(),
        Some(Path::new("head")),
        new.as_bytes(),
        Some(Path::new("worktree")),
        None,
    ) else {
        return Vec::new();
    };
    let mut lines: BTreeSet<u32> = BTreeSet::new();
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
                    lines.insert(line.min(last_line));
                    line += 1;
                }
                '-' => {
                    lines.insert(line.min(last_line));
                }
                ' ' => line += 1,
                // `\ No newline at end of file` and friends occupy no line.
                _ => {}
            }
        }
    }
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for line in lines {
        match ranges.last_mut() {
            Some(last) if line <= last.1.saturating_add(1) => last.1 = line,
            _ => ranges.push((line, line)),
        }
    }
    ranges
}

/// Whether a directive's own source lines are among the working tree's changes
/// for its file. A span's trailing trivia — the blank line or comment after the
/// last posting — belongs to no transaction and is ignored.
fn touches_changes(
    ledger: &Ledger,
    changed: &HashMap<u16, Vec<(u32, u32)>>,
    line_starts: &mut HashMap<u16, Vec<usize>>,
    source: &Spanned<Directive>,
) -> bool {
    let Some(ranges) = changed.get(&source.file_id) else {
        return false;
    };
    let Some(file) = ledger.source_map.get(source.file_id as usize) else {
        return false;
    };
    let text: &str = &file.source;
    let starts = line_starts
        .entry(source.file_id)
        .or_insert_with(|| line_starts_of(text));
    let body = text.get(source.span.start..source.span.end).unwrap_or("");
    let last_byte = source.span.start + body.trim_end().len();
    let first = line_of(starts, source.span.start);
    let last = line_of(starts, last_byte.saturating_sub(1).max(source.span.start));
    ranges.iter().any(|&(from, to)| from <= last && first <= to)
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
            ledger_diff,
            commit_ledger,
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
                ledger_diff,
                commit_ledger,
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

        let balances = account_balances(&ledger);
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
        assert!(root.iter().all(|view| !view.modified));
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

    /// The narrations of the transactions the register would mark as modified.
    fn modified_narrations(state: &AppState) -> Vec<String> {
        views(state)
            .into_iter()
            .filter(|view| view.modified)
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
        // The edited transaction is the only row that reports itself modified.
        assert_eq!(modified_narrations(&state), ["Groceries and coffee"]);

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
        assert_eq!(modified_narrations(&state), Vec::<String>::new());
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
        assert_eq!(modified_narrations(&state), ["Groceries and coffee"]);

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
        assert_eq!(modified_narrations(&state), Vec::<String>::new());
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
        assert_eq!(modified_narrations(&state), Vec::<String>::new());

        let id = view_id(&state, "Groceries");
        edit_transaction(
            &state,
            edit_of(id, "2026-01-10", "Groceries", vec![posting(0, "-25.00")]),
        )
        .unwrap();

        // The edit — not the file's untracked state — is what shows up.
        assert_eq!(modified_narrations(&state), ["Groceries"]);
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
        assert!(modified_narrations(&state).is_empty());
        assert!(ledger_diff_text(&state).is_err());
        assert!(commit_staged(&state, "no repository").is_err());
    }
}
