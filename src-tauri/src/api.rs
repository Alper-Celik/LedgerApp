// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The app's backend without Tauri: the façade the `ledger` CLI drives.
//!
//! # Why this module exists
//!
//! Every capability below is already a plain function over `&AppState` or
//! `&Ledger`; the commands in `commands.rs` are thin wrappers that unwrap a
//! `tauri::State` and hand the payload to the frontend. A binary cannot use
//! those wrappers — they need a running Tauri app — and re-implementing them
//! would give the terminal a second loader, a second budget reader and a
//! second report writer, which is exactly how a CLI starts disagreeing with
//! the window about a total.
//!
//! So this is the second, Tauri-free wrapper: [`open`] loads a ledger into the
//! same [`AppState`] the app uses, and every function here takes that handle
//! and returns plain data ([`serde`] structs and exact decimal *strings*). No
//! `tauri::State`, no `AppHandle`, no events.
//!
//! # What it deliberately owns
//!
//! Two things the app gets from `AppHandle` and a CLI has to be told:
//!
//! - the app's config directory, where indexes and the assistant's settings
//!   live. [`indexes_dir`] and [`ai_dir`] name the same subdirectories
//!   `commands.rs` does, so the CLI and the app read one set of files; the
//!   caller passes the base directory in, because nothing here guesses a
//!   platform path.
//! - the composition of the assistant's first turn: [`SYSTEM_PROMPT`] plus the
//!   read tools plus the user's prompt. The app's panel composes it in the
//!   frontend; the CLI cannot, so the composition is repeated here from the
//!   same three `ai_cmd` exports — not from a second prompt.
//!
//! # What it deliberately does not do
//!
//! No streaming ([`crate::ai_stream`]), no entry writing ([`crate::editing`]'s
//! transaction edits), and no tool loop: [`ask`] is one model call, and a call
//! the model asks for comes back in [`Answer`] for the caller to print. The
//! only write is [`write_prices`], because appending quotes is the one change
//! a price fetch has always been allowed to make.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use chart_render::{ChartSpec, TerminalOptions, render_terminal};
use rustledger_loader::{ErrorSeverity, Ledger, LedgerError};

use crate::session::{self, AppState};

pub use crate::budget::{BudgetRange, BudgetRangeMonth};
pub use crate::index::{IndexPoint, IndexSeries};
pub use crate::model::{BudgetEnvelope, BudgetGroup, BudgetMonth};
pub use crate::prices::{
    PriceFailure, PriceFetch, PriceQuote, PriceSourceInfo, PriceTarget, QuoteRequest,
};
pub use crate::query::QueryTable;
pub use crate::reports::{
    CashflowMonth, CategoryMonth, CategoryMonths, LifestyleCategory, LifestyleCategoryMonth,
    LifestyleIndex, LifestyleIndexMonth, LifestyleReport, LifestyleTotal, NetWorthMonth,
};

// ------------------------------------------------------------------ loading ---

/// One loaded ledger: the handle every other function in this module takes.
///
/// The three public fields are the load's own report — where the journal is,
/// how much of it the loader produced, and every problem it raised — because a
/// CLI's first question is whether the file it was pointed at is even a ledger.
/// The session behind them is private: it must be the same one the app's
/// commands would hold, so an edit made through [`write_prices`] reloads and
/// re-baselines exactly as the window does.
///
/// Serialising one skips the session, so a `--json` answer shows the report
/// and nothing else.
#[derive(serde::Serialize)]
pub struct Opened {
    /// The root journal the load used: the file the session reloads after an
    /// edit, with its `include`s followed.
    pub root: PathBuf,
    /// How many directives the load produced, after booking and plugins.
    pub directives: u32,
    /// Every problem the load reported, errors and warnings alike.
    pub errors: Vec<Issue>,
    #[serde(skip)]
    state: AppState,
}

impl Opened {
    /// Whether the load reported no error-severity problem. Warnings do not
    /// make a journal unsound — they are the same messages the app shows in
    /// its problems list.
    pub fn is_sound(&self) -> bool {
        !self
            .errors
            .iter()
            .any(|issue| issue.severity == Severity::Error)
    }

    /// The ledger itself, for a call that only needs to read it.
    ///
    /// `open` is the only constructor and it always stores the ledger, so the
    /// lock cannot hold `None` here.
    fn with_ledger<T>(&self, read: impl FnOnce(&Ledger) -> T) -> T {
        let guard = self.state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().expect("an opened ledger keeps its ledger");
        read(&loaded.ledger)
    }
}

/// The report fields only: the session is a lock, and debugging a lock is not
/// what a caller wants to see.
impl std::fmt::Debug for Opened {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Opened")
            .field("root", &self.root)
            .field("directives", &self.directives)
            .field("errors", &self.errors.len())
            .finish_non_exhaustive()
    }
}

/// Load the ledger at `path` — the journal itself, or a directory holding one
/// (the loader's `main.bean` discovery) — and keep it for the calls that
/// follow.
///
/// The load is the app's own ([`session::load_baselined`]), baseline included,
/// so `modified` flags and edits made later behave as they do in the window. A
/// journal that parses but fails validation is still `Ok`: `errors` carries the
/// problems, exactly as the app keeps a ledger that reports them. `Err` means
/// the file could not be loaded at all.
pub fn open(path: &Path) -> Result<Opened, String> {
    let loaded = session::load_baselined(path)?;
    let directives = directive_count(&loaded.ledger);
    let errors = issues_of(&loaded.ledger);
    Ok(Opened {
        root: loaded.root.clone(),
        directives,
        errors,
        state: AppState {
            loaded: RwLock::new(Some(loaded)),
        },
    })
}

/// The load's verdict as data, for a caller that only wants to know whether the
/// journal is sound.
///
/// The same load [`open`] makes, minus the session: `ledger check` has nothing
/// to do afterwards, so nothing here keeps the ledger alive.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Check {
    /// The root journal the load used.
    pub root: PathBuf,
    /// How many directives the load produced.
    pub directives: u32,
    /// Every problem the load reported.
    pub errors: Vec<Issue>,
    /// True when no error-severity problem was reported.
    pub ok: bool,
}

/// Load `path` and answer its report without keeping it.
pub fn check(path: &Path) -> Result<Check, String> {
    let opened = open(path)?;
    let ok = opened.is_sound();
    Ok(Check {
        root: opened.root,
        directives: opened.directives,
        errors: opened.errors,
        ok,
    })
}

/// How bad one problem is, in the loader's own terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// A problem that must be fixed; the ledger is still usable.
    Error,
    /// A problem worth showing, not worth failing a check over.
    Warning,
}

/// One problem the load reported, as plain data.
///
/// The loader's [`LedgerError`] carries the span and the source map needed for
/// a rich snippet; this carries the line and column a terminal can print.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Issue {
    /// Whether the load treats it as an error or a warning.
    pub severity: Severity,
    /// The loader's code, e.g. `E0001`.
    pub code: String,
    /// The message, ready to print.
    pub message: String,
    /// The file the problem points into, when the loader names one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// 1-based line, when the loader knows it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// 1-based column, when the loader knows it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
}

impl Issue {
    /// `file:line:column`, when the loader named a place — the prefix a
    /// compiler-style line takes. `None` for a problem about the ledger as a
    /// whole.
    pub fn location(&self) -> Option<String> {
        let file = self.file.as_ref()?;
        Some(match (self.line, self.column) {
            (Some(line), Some(column)) => format!("{}:{line}:{column}", file.display()),
            (Some(line), None) => format!("{}:{line}", file.display()),
            _ => file.display().to_string(),
        })
    }
}

impl std::fmt::Display for Issue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let severity = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        match self.location() {
            Some(location) => write!(
                formatter,
                "{location}: {severity}[{}]: {}",
                self.code, self.message
            ),
            None => write!(formatter, "{severity}[{}]: {}", self.code, self.message),
        }
    }
}

fn issues_of(ledger: &Ledger) -> Vec<Issue> {
    ledger.errors.iter().map(issue_of).collect()
}

fn issue_of(error: &LedgerError) -> Issue {
    Issue {
        severity: match error.severity {
            ErrorSeverity::Error => Severity::Error,
            ErrorSeverity::Warning => Severity::Warning,
        },
        code: error.code.clone(),
        message: error.message.clone(),
        file: error
            .location
            .as_ref()
            .map(|location| location.file.clone()),
        line: error.location.as_ref().map(|location| location.line),
        column: error.location.as_ref().map(|location| location.column),
    }
}

fn directive_count(ledger: &Ledger) -> u32 {
    u32::try_from(ledger.directives.len()).unwrap_or(u32::MAX)
}

// -------------------------------------------------------------------- query ---

/// Run one BQL statement over the opened ledger.
///
/// `limit` caps the rows; `0` means no cap. The table's cells are the exact
/// decimal strings the executor produced — nothing here formats money.
pub fn query(opened: &Opened, bql: &str, limit: u32) -> Result<QueryTable, String> {
    opened.with_ledger(|ledger| crate::query::run(ledger, bql, limit))
}

// ------------------------------------------------------------------- budget ---

/// One month of the envelope budget — the newest month when `month` is `None`.
pub fn budget_month(opened: &Opened, month: Option<&str>) -> Result<BudgetMonth, String> {
    crate::budget::budget_view(&opened.state, month)
}

/// A window of consecutive budget months. Either end may be `None`; the
/// backend's own default window applies (six months ending at the newest
/// budgeted month).
pub fn budget_range(
    opened: &Opened,
    from: Option<String>,
    to: Option<String>,
) -> Result<BudgetRange, String> {
    crate::budget::budget_range(&opened.state, from, to)
}

/// Exactly `months` consecutive budget months, ending at the newest budgeted
/// month — the "last N months" window a person asks for, which the range
/// endpoints cannot express without knowing where the ledger's data ends.
///
/// The window's end is read from the backend's own default range, and the start
/// is that month shifted back. That probe is a second walk of the ledger, taken
/// on purpose: the alternative is re-deriving "the newest budgeted month" here,
/// which is the one rule the probe already owns.
pub fn budget_months(opened: &Opened, months: u32) -> Result<BudgetRange, String> {
    if months == 0 {
        return Err("a budget window needs at least one month".to_string());
    }
    let probe = budget_range(opened, None, None)?;
    if probe.to.is_empty() {
        // Nothing budgeted: the backend's empty window already says so.
        return Ok(probe);
    }
    let from = shift_month(&probe.to, -i64::from(months) + 1)
        .ok_or_else(|| format!("`{}` is not a YYYY-MM month", probe.to))?;
    budget_range(opened, Some(from), Some(probe.to))
}

/// `label` moved by `delta` months, e.g. `("2026-01", -2)` → `"2025-11"`.
fn shift_month(label: &str, delta: i64) -> Option<String> {
    if label.len() != 7 || label.as_bytes()[4] != b'-' {
        return None;
    }
    let year: i64 = label.get(0..4)?.parse().ok()?;
    let month: i64 = label.get(5..7)?.parse().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    let key = year * 12 + month - 1 + delta;
    Some(format!(
        "{:04}-{:02}",
        key.div_euclid(12),
        key.rem_euclid(12) + 1
    ))
}

// ------------------------------------------------------------------ reports ---

/// Net worth at the end of each of the last `months` months.
pub fn net_worth(opened: &Opened, months: u32) -> Vec<NetWorthMonth> {
    opened.with_ledger(|ledger| crate::reports::net_worth_months(ledger, months))
}

/// Inflows and outflows per month.
pub fn cashflow(opened: &Opened, months: u32) -> Vec<CashflowMonth> {
    opened.with_ledger(|ledger| crate::reports::cashflow_months(ledger, months))
}

/// Spending per two-level expense branch, one row per branch.
pub fn categories(opened: &Opened, months: u32) -> Vec<CategoryMonths> {
    opened.with_ledger(|ledger| crate::reports::category_months(ledger, months))
}

/// Spending restated with an installed index series.
///
/// `index` names a stored series; `None` accepts a store holding exactly one,
/// so a caller that imported a single series need not repeat its id, and
/// refuses with the ids to choose from when there are several.
pub fn lifestyle(
    opened: &Opened,
    config_dir: &Path,
    months: u32,
    index: Option<&str>,
) -> Result<LifestyleReport, String> {
    let stored = index_series(config_dir)?;
    let series = match index {
        Some(id) => stored
            .into_iter()
            .find(|series| series.id == id)
            .ok_or_else(|| format!("no stored index is called `{id}`"))?,
        None => match stored.len() {
            1 => stored.into_iter().next().expect("one series"),
            0 => {
                return Err(
                    "a lifestyle report needs an index series; import one with `ledger index import`"
                        .to_string(),
                );
            }
            _ => {
                let ids: Vec<&str> = stored.iter().map(|series| series.id.as_str()).collect();
                return Err(format!("choose an index with --index: {}", ids.join(", ")));
            }
        },
    };
    opened.with_ledger(|ledger| crate::reports::lifestyle_report(ledger, months, &series))
}

// ------------------------------------------------------------------- prices ---

/// The quote sources this build can ask (`ecb`, `yahoo`, `tefas`, …).
pub fn price_sources() -> Vec<PriceSourceInfo> {
    crate::prices::sources()
}

/// Every commodity the ledger holds or quotes, with the newest `price`
/// directive already in it. Nothing is fetched.
pub fn price_targets(opened: &Opened) -> Vec<PriceTarget> {
    crate::prices::targets(&opened.state)
}

/// Fetch quotes for `requests`, blocking on the network.
///
/// Per-request failures come back as [`PriceFetch::Failed`]; the call only
/// errors when every request failed, the command layer's own rule.
pub fn fetch_prices(
    opened: &Opened,
    requests: Vec<QuoteRequest>,
) -> Result<Vec<PriceFetch>, String> {
    crate::prices::fetch(&opened.state, requests)
}

/// Append `quotes` to one journal file as `price` directives, through the
/// loader-verified write pipeline every app edit takes: the journal is
/// reloaded, a quote that raises a new validation error is rolled back, and the
/// file is staged in git when the ledger lives in a repository.
///
/// `file` may name a journal file by the path the loader holds or by a path the
/// shell resolved; the two are matched here so a caller need not know which
/// form the loader recorded.
pub fn write_prices(opened: &Opened, file: &str, quotes: &[PriceQuote]) -> Result<(), String> {
    let resolved = opened.with_ledger(|ledger| journal_file(ledger, file));
    let file = resolved.as_deref().unwrap_or(file);
    crate::prices::write_to_file(&opened.state, file, quotes)
}

/// The journal's own path for `file`, when `file` names one of its files under
/// another spelling (relative vs absolute, a symlinked parent).
fn journal_file(ledger: &Ledger, file: &str) -> Option<String> {
    let wanted = Path::new(file);
    let canonical = wanted.canonicalize().ok();
    ledger
        .source_map
        .files()
        .iter()
        .find(|known| {
            known.path == wanted
                || known.path.display().to_string() == file
                || canonical.as_deref().is_some_and(|canonical| {
                    known.path.canonicalize().ok().as_deref() == Some(canonical)
                })
        })
        .map(|known| known.path.display().to_string())
}

// ------------------------------------------------------------------ indexes ---

/// Where index series live under the app's config directory: the same
/// `indexes` subdirectory `commands.rs` reads, so a series fetched in the
/// window is visible here and vice versa.
pub fn indexes_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("indexes")
}

/// Every stored series, id-sorted. No file is no series, not an error.
pub fn index_series(config_dir: &Path) -> Result<Vec<IndexSeries>, String> {
    crate::index_store::load_indexes(&indexes_dir(config_dir))
}

/// Store a publisher's `date,value` CSV as the series `id`, replacing any
/// series already stored under it.
pub fn index_import(
    config_dir: &Path,
    id: &str,
    label: &str,
    source: &str,
    currency: &str,
    csv_text: &str,
) -> Result<IndexSeries, String> {
    crate::index_store::import_csv(
        &indexes_dir(config_dir),
        id,
        label,
        source,
        currency,
        csv_text,
    )
}

/// Download one provider's series and store it, blocking on the network. The
/// id is the series id (`us-cpi`, `tr-cpi`, …), the same one `commands.rs`'s
/// `index_refresh` command takes.
pub fn index_refresh(config_dir: &Path, id: &str) -> Result<IndexSeries, String> {
    let provider = crate::index_providers::provider(id)
        .ok_or_else(|| format!("no provider is called `{id}`"))?;
    let series = provider.fetch()?;
    crate::index_store::save_series(&indexes_dir(config_dir), &series)?;
    Ok(series)
}

// ---------------------------------------------------------------- assistant ---

/// Where the assistant's settings live under the app's config directory: the
/// same `ai` subdirectory `commands.rs` reads, so the key the window stored is
/// the key the CLI uses.
pub fn ai_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("ai")
}

/// One call the model asked for. The CLI does not run it (one call is one
/// call); it prints it so the asker knows what the model wanted.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AnswerCall {
    /// The tool's dispatch name.
    pub name: String,
    /// The arguments as the JSON text the model produced.
    pub arguments: String,
}

/// One assistant turn's answer.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Answer {
    /// The prose the model produced.
    pub text: String,
    /// The calls the model asked for, unexecuted.
    pub tool_calls: Vec<AnswerCall>,
    /// Why generation stopped, in the endpoint's words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

/// One assistant turn: the app's system prompt, the app's read tools, and one
/// model call — no streaming, no tool loop.
///
/// The prompt and the tools come from [`crate::ai_cmd`], the module the panel
/// reads, so the CLI's assistant is the same assistant; only the composition is
/// here, because the panel composes it in the frontend. A turn the model answers
/// with tool calls comes back as [`Answer::tool_calls`] — running them and
/// asking again is the panel's job, and deliberately not this one's.
pub fn ask(config_dir: &Path, prompt: &str) -> Result<Answer, String> {
    use crate::ai::{ChatMessage, ChatRequest, ChatRole};

    let request = ChatRequest {
        messages: vec![
            ChatMessage::text(ChatRole::System, crate::ai_cmd::SYSTEM_PROMPT),
            ChatMessage::text(ChatRole::User, prompt),
        ],
        tools: crate::ai_cmd::tools(),
        media: Vec::new(),
    };
    let response = crate::ai_cmd::chat(&ai_dir(config_dir), request)?;
    Ok(Answer {
        text: response.text,
        tool_calls: response
            .tool_calls
            .into_iter()
            .map(|call| AnswerCall {
                name: call.name,
                arguments: call.arguments,
            })
            .collect(),
        finish_reason: response.finish_reason,
    })
}

// -------------------------------------------------------------------- chart ---

/// Render a chart spec for a terminal, with the renderer's default options.
///
/// The bytes are either a kitty graphics escape sequence (when the terminal
/// speaks it) or a Unicode block chart; the renderer chooses, so a caller never
/// has to detect a terminal. Write them to stdout as they come — re-encoding
/// them as text would corrupt the escape.
pub fn chart(spec_json: &str) -> Result<Vec<u8>, String> {
    chart_with(spec_json, TerminalOptions::default())
}

/// [`chart`] with explicit options — the CLI's `--width` sets the cell count.
pub fn chart_with(spec_json: &str, options: TerminalOptions) -> Result<Vec<u8>, String> {
    let spec = ChartSpec::from_json(spec_json)
        .map_err(|error| format!("this is not a chart spec: {error}"))?;
    Ok(render_terminal(&spec, options))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use chart_render::Protocol;

    use crate::testing::TempDir;

    /// A small book with real spending, a quote, and the envelope plugin's own
    /// flows — one ledger enough for every read the façade offers: the BQL
    /// executor, the monthly reports, the budget reader, the price targets and
    /// the terminal chart all have something to answer with.
    ///
    /// The envelope entries mirror what `plugins/rustledger-envelope` writes:
    /// dated the month's last day, tagged with `envelope-kind`, every posting
    /// inside `Equity:Envelope:`.
    const JOURNAL: &str = "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank USD
2026-01-01 open Income:Salary USD
2026-01-01 open Expenses:Food:Groceries USD
2026-01-01 open Equity:Envelope:To-Be-Budgeted USD
2026-01-01 open Equity:Envelope:Money USD
2026-01-01 open Equity:Envelope:Food:Groceries USD

2026-01-05 * \"Employer\" \"Salary\"
  Assets:Bank      3000.00 USD
  Income:Salary   -3000.00 USD

2026-01-10 * \"Market\" \"Groceries\"
  Expenses:Food:Groceries   250.50 USD
  Assets:Bank              -250.50 USD

2026-01-15 * \"Envelope\" \"January funding\"
  envelope-kind: \"Envelope funding\"
  Equity:Envelope:To-Be-Budgeted   -400.00 USD
  Equity:Envelope:Food:Groceries    400.00 USD

2026-01-31 * \"Envelope\" \"January activity\"
  envelope-kind: \"Envelope activity\"
  Equity:Envelope:Food:Groceries   -250.50 USD
  Equity:Envelope:Money             250.50 USD

2026-01-31 price AAPL 190.00 USD
";

    fn opened() -> (TempDir, Opened) {
        let dir = TempDir::new("api");
        let path = dir.file("main.bean");
        fs::write(&path, JOURNAL).expect("the fixture is written");
        let opened = open(&path).expect("the fixture loads");
        (dir, opened)
    }

    #[test]
    fn opens_the_journal_and_reports_what_the_load_found() {
        let (dir, opened) = opened();
        assert_eq!(opened.root, dir.file("main.bean"));
        assert!(opened.directives > 0);
        assert!(opened.errors.is_empty(), "errors: {:?}", opened.errors);
        assert!(opened.is_sound());

        // The handle's JSON is the load's report only: the session is skipped,
        // so no lock or ledger text can leak into a `--json` answer.
        let json = serde_json::to_value(&opened).expect("the report serialises");
        assert_eq!(
            json["directives"].as_u64(),
            Some(u64::from(opened.directives))
        );
        assert!(json["root"].is_string());
        assert!(json["errors"].is_array());
        assert!(json.get("state").is_none(), "the session is not serialised");
    }

    #[test]
    fn check_names_the_problem_and_fails_an_unsound_journal() {
        let dir = TempDir::new("api-broken");
        let path = dir.file("main.bean");
        fs::write(
            &path,
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank USD
2026-01-01 open Expenses:Food USD

2026-01-02 * \"Shop\" \"Lunch\"
  Expenses:Food   10.00 USD
  Assets:Bank     -5.00 USD
",
        )
        .unwrap();

        let check = check(&path).expect("the journal loads with problems");
        assert!(!check.ok, "an unbalanced entry is an error");
        let issue = check
            .errors
            .iter()
            .find(|issue| issue.severity == Severity::Error)
            .expect("the load reports the unbalanced entry");
        assert!(!issue.message.is_empty(), "the message is printable");
        assert!(!issue.code.is_empty(), "the code names the rule");
        // The line is what a terminal user acts on; the loader points at the
        // entry that raised it.
        assert!(
            issue.location().is_some(),
            "a refused entry has a place: {issue}"
        );
    }

    #[test]
    fn a_missing_journal_is_an_error_not_an_empty_ledger() {
        let dir = TempDir::new("api-missing");
        let error = open(&dir.file("nowhere.bean")).expect_err("a missing file is refused");
        assert!(!error.is_empty(), "the reason is carried: {error}");
    }

    #[test]
    fn query_keeps_exact_decimals_and_caps_the_rows() {
        let (_dir, opened) = opened();
        let table = query(
            &opened,
            "SELECT account, SUM(number) WHERE account ~ \"Expenses:\" GROUP BY account ORDER BY account",
            0,
        )
        .expect("the statement runs");
        assert_eq!(table.columns, vec!["account", "SUM(number)"]);
        assert_eq!(
            table.rows,
            vec![vec![
                "Expenses:Food:Groceries".to_string(),
                "250.50".to_string()
            ]]
        );
        assert!(!table.truncated);

        // One row asked for, two entries matching: the table says it was cut.
        let capped = query(&opened, "SELECT date ORDER BY date", 1).expect("the statement runs");
        assert_eq!(capped.rows.len(), 1);
        assert!(capped.truncated, "the cap is reported, never silent");
    }

    #[test]
    fn budget_reads_the_envelope_flows_the_latest_month() {
        let (_dir, opened) = opened();
        let month = budget_month(&opened, None).expect("the fixture is budgeted");
        assert_eq!(month.month, "2026-01");
        assert_eq!(month.currency, "USD");
        let food = month
            .groups
            .iter()
            .find(|group| group.name == "Food")
            .expect("the Food group");
        assert_eq!(food.budgeted, "400.00");
        assert_eq!(food.spent, "250.50");
        assert_eq!(food.balance, "149.50");
        let envelope = food
            .envelopes
            .iter()
            .find(|envelope| envelope.name == "Groceries")
            .expect("the Groceries envelope");
        assert_eq!(envelope.full_name, "Equity:Envelope:Food:Groceries");
    }

    #[test]
    fn budget_months_spans_exactly_the_window_asked_for() {
        let (_dir, opened) = opened();
        let range = budget_months(&opened, 3).expect("three months");
        assert_eq!(range.from, "2025-11");
        assert_eq!(range.to, "2026-01");
        let months: Vec<&str> = range.months.iter().map(|m| m.month.as_str()).collect();
        assert_eq!(months, vec!["2025-11", "2025-12", "2026-01"]);
        let last = &range.months[2];
        assert_eq!(last.spent, "250.50");
        assert_eq!(last.budgeted, "400.00");

        let error = budget_months(&opened, 0).expect_err("zero months is refused");
        assert!(error.contains("at least one month"), "{error}");
    }

    #[test]
    fn reports_answer_the_month_window_ending_at_the_newest_entry() {
        let (_dir, opened) = opened();
        let net_worth = net_worth(&opened, 2);
        let months: Vec<&str> = net_worth.iter().map(|row| row.month.as_str()).collect();
        assert_eq!(months, vec!["2025-12", "2026-01"]);
        assert_eq!(net_worth[1].value, "2749.50");
        assert_eq!(net_worth[1].currency, "USD");

        let cashflow = cashflow(&opened, 1);
        assert_eq!(cashflow.len(), 1);
        assert_eq!(cashflow[0].inflow, "3000.00");
        assert_eq!(cashflow[0].outflow, "250.50");

        let categories = categories(&opened, 1);
        let food = categories
            .iter()
            .find(|row| row.account == "Expenses:Food:Groceries")
            .expect("the grocery branch");
        assert_eq!(food.months[0].month, "2026-01");
        assert_eq!(food.months[0].spent, "250.50");
    }

    #[test]
    fn an_imported_index_feeds_the_lifestyle_report() {
        let dir = TempDir::new("api-index");
        let config = dir.file("config");
        fs::create_dir_all(&config).unwrap();
        let series = index_import(
            &config,
            "test-cpi",
            "Test CPI",
            "Test Statistics",
            "USD",
            "2025-11,100.00\n2025-12,102.00\n2026-01,105.00\n",
        )
        .expect("the CSV imports");
        assert_eq!(series.points.len(), 3);

        let listed = index_series(&config).expect("the store reads");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "test-cpi");

        let (_books, opened) = opened();
        // No `--index`: one stored series is the one meant.
        let report = lifestyle(&opened, &config, 1, None).expect("the report runs");
        assert_eq!(report.index.id, "test-cpi");
        assert_eq!(report.totals[0].month, "2026-01");
        assert_eq!(report.totals[0].nominal, "250.50");
        // One month of data: the first month's real figure is its nominal one.
        assert_eq!(report.totals[0].real, "250.50");

        let unknown = lifestyle(&opened, &config, 1, Some("nope")).expect_err("unknown id");
        assert!(
            unknown.contains("no stored index is called `nope`"),
            "{unknown}"
        );
    }

    #[test]
    fn a_lifestyle_report_without_an_index_says_to_import_one() {
        let dir = TempDir::new("api-no-index");
        let config = dir.file("config");
        fs::create_dir_all(&config).unwrap();
        let (_books, opened) = opened();
        let error = lifestyle(&opened, &config, 1, None).expect_err("no index is installed");
        assert!(error.contains("import one"), "{error}");
    }

    #[test]
    fn prices_read_the_ledgers_newest_quote_without_fetching() {
        let (_dir, opened) = opened();
        let targets = price_targets(&opened);
        let apple = targets
            .iter()
            .find(|target| target.commodity == "AAPL")
            .expect("the price directive names AAPL");
        let latest = apple.latest.as_ref().expect("the ledger quotes it");
        assert_eq!(latest.price, "190.00");
        assert_eq!(latest.currency, "USD");
        assert_eq!(latest.date, "2026-01-31");
        assert_eq!(latest.source, "ledger");
        // Every commodity the ledger holds is a target too, even unquoted.
        assert!(targets.iter().any(|target| target.commodity == "USD"));
    }

    #[test]
    fn writing_a_quote_needs_a_file_of_the_journal() {
        let (dir, opened) = opened();
        let quote = PriceQuote {
            commodity: "AAPL".to_string(),
            currency: "USD".to_string(),
            date: "2026-02-02".to_string(),
            price: "191.25".to_string(),
            source: "test".to_string(),
        };

        let outside = dir.file("other.bean");
        let error = write_prices(&opened, &outside.display().to_string(), &[quote.clone()])
            .expect_err("a file outside the journal is refused");
        assert!(error.contains("is not a file of this journal"), "{error}");

        // The journal itself is accepted by its absolute path, the spelling a
        // shell hands the CLI.
        write_prices(&opened, &opened.root.display().to_string(), &[quote])
            .expect("the quote is appended");
        let text = fs::read_to_string(dir.file("main.bean")).expect("the journal is readable");
        assert!(
            text.contains("2026-02-02 price AAPL 191.25 USD"),
            "the appended directive is in the file:\n{text}"
        );
    }

    #[test]
    fn chart_renders_a_spec_to_terminal_bytes() {
        let spec = r#"{"title": "Net worth", "kind": "line", "currency": "USD",
            "series": [{"name": "Assets", "points": [
                {"label": "Jan", "value": "1,000.00"},
                {"label": "Feb", "value": "1,250.50"}]}]}"#;

        let unicode = chart_with(
            spec,
            TerminalOptions {
                columns: 60,
                protocol: Protocol::Unicode,
                ..TerminalOptions::default()
            },
        )
        .expect("the spec renders");
        let text = String::from_utf8_lossy(&unicode);
        assert!(!unicode.is_empty());
        assert!(
            text.to_uppercase().contains("ASSETS"),
            "the legend is drawn:\n{text}"
        );

        // The default path picks a protocol for the environment; either way the
        // answer is bytes, never a re-encoded string.
        assert!(!chart(spec).expect("the default renders").is_empty());

        let error = chart("{ not json }").expect_err("garbage is refused");
        assert!(error.contains("not a chart spec"), "{error}");
    }

    #[test]
    fn ask_without_a_model_reports_a_configuration_problem() {
        let dir = TempDir::new("api-ask");
        let config = dir.file("config");
        fs::create_dir_all(&config).unwrap();
        // An empty configuration has no model, and `ai.rs` refuses before any
        // request goes out — so this asserts the composition reached the
        // backend without a network.
        let error = ask(&config, "how much did I spend?").expect_err("no model is configured");
        assert!(error.contains("model"), "{error}");
    }
}
