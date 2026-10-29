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
//! No streaming ([`crate::ai_stream`]) and no entry writing ([`crate::editing`]'s
//! transaction edits). [`ask`] runs the model's *read* tools itself — up to
//! [`ASK_ROUNDS`] model calls — because a terminal cannot dispatch them, but a
//! call to a write tool comes back in [`Answer::tool_calls`] as a proposal and
//! is never executed. The only write is [`write_prices`], because appending
//! quotes is the one change a price fetch has always been allowed to make.

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

    /// The loaded session — ledger and the baseline edits are measured
    /// against — for a read that needs both (a register row's `modified` flag).
    fn with_loaded<T>(&self, read: impl FnOnce(&session::Loaded) -> T) -> T {
        let guard = self.state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().expect("an opened ledger keeps its ledger");
        read(loaded)
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
pub fn write_prices(opened: &Opened, file: &str, quotes: &[PriceQuote]) -> Result<usize, String> {
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

/// One write call the model asked for, unexecuted.
///
/// [`ask`] runs reads and only prints these: a proposal is the user's to
/// approve, and the CLI has no place to ask.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AnswerCall {
    /// The tool's dispatch name.
    pub name: String,
    /// The arguments as the JSON text the model produced.
    pub arguments: String,
}

/// One assistant answer, after the reads it asked for have been run.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Answer {
    /// The last prose the model produced.
    pub text: String,
    /// Write calls the model asked for, kept as proposals and never executed.
    pub tool_calls: Vec<AnswerCall>,
    /// Why generation stopped, in the endpoint's words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    /// How many model calls the answer took, tool rounds included.
    pub rounds: u32,
    /// Whether the loop stopped at [`ASK_ROUNDS`] with the model still asking
    /// for tools, so the answer may be incomplete.
    pub truncated: bool,
}

/// How many model calls one [`ask`] may make: the question, then the follow-ups
/// its tool answers need. Four is enough for "read, read once more, answer"
/// without letting a confused model loop, and each round is a paid request.
pub const ASK_ROUNDS: u32 = 4;

/// How many rows a `query` tool answer may carry. The same cap the panel uses,
/// so a CLI answer and a window answer read the same table.
const ASK_QUERY_ROWS: u32 = 200;

/// What the model is told when it asks for a write: the call was not run.
const ASK_PROPOSAL_PENDING: &str = "This change is a proposal for the user to approve. Nothing has been written and it is not applied.";

/// One question, answered by the endpoint in `dir`: the app's system prompt and
/// the app's tools, then as many read-tool rounds as the model asks for, up to
/// [`ASK_ROUNDS`].
///
/// The prompt and the tools come from [`crate::ai_cmd`], the module the panel
/// reads, so the CLI's assistant is the same assistant; only the composition
/// and the read dispatch are here, because the panel composes them in the
/// frontend. `ledger` is what the read tools answer from: `None` is a question
/// asked without a journal, and each read then answers the model with "no
/// ledger is loaded" instead of failing the whole ask.
///
/// A write tool is never run: it comes back in [`Answer::tool_calls`] so the
/// caller can print it as a proposal. Nothing this function does writes a file.
pub fn ask(config_dir: &Path, ledger: Option<&Opened>, prompt: &str) -> Result<Answer, String> {
    ask_with(&crate::ai_cmd::Endpoint, config_dir, ledger, prompt)
}

/// [`ask`] over a scripted backend — the seam its tests drive.
fn ask_with(
    backend: &impl crate::ai_cmd::Backend,
    config_dir: &Path,
    ledger: Option<&Opened>,
    prompt: &str,
) -> Result<Answer, String> {
    use crate::ai::{ChatMessage, ChatRequest, ChatRole};

    let dir = ai_dir(config_dir);
    // One schema build for the whole ask: the same table every round, so a
    // round cannot be offered a different tool set than the one before it.
    let tools = crate::ai_cmd::tools();
    let mut messages = vec![
        ChatMessage::text(ChatRole::System, crate::ai_cmd::SYSTEM_PROMPT),
        ChatMessage::text(ChatRole::User, prompt),
    ];
    let mut proposals: Vec<AnswerCall> = Vec::new();
    let mut text = String::new();
    let mut finish_reason = None;
    let mut rounds = 0;
    let mut truncated = false;

    while rounds < ASK_ROUNDS {
        let request = ChatRequest {
            messages: messages.clone(),
            tools: tools.clone(),
            media: Vec::new(),
        };
        let response = crate::ai_cmd::chat_with(backend, &dir, request)?;
        rounds += 1;
        finish_reason.clone_from(&response.finish_reason);
        // A round that only calls tools has no prose; the answer's text is the
        // last prose the model did say.
        if !response.text.trim().is_empty() {
            text.clone_from(&response.text);
        }
        if response.tool_calls.is_empty() {
            break;
        }

        // The calls are announced before their answers, the order the wire
        // requires; a tool answer without its assistant turn is a broken
        // conversation the endpoint would refuse.
        messages.push(ChatMessage {
            role: ChatRole::Assistant,
            text: response.text.clone(),
            tool_calls: response.tool_calls.clone(),
            tool_call_id: None,
        });

        let mut ran = 0;
        for call in response.tool_calls {
            if crate::ai_cmd::is_read_only(&call.name) {
                let answer = run_read_tool(ledger, config_dir, &call.name, &call.arguments);
                messages.push(ChatMessage::tool_result(&call.id, answer));
                ran += 1;
            } else if matches!(call.name.as_str(), "add_transaction" | "write_file") {
                // A write is a proposal, never an answer to the model: running
                // it would be the app applying an edit the user never saw. The
                // call still gets a tool message — an unanswered call would make
                // the next round an invalid conversation — saying it is pending.
                // A write is a proposal, never an answer to the model: running
                // it would be the app applying an edit the user never saw. The
                // call still gets a tool message — an unanswered call would make
                // the next round an invalid conversation — saying it is pending.
                messages.push(ChatMessage::tool_result(
                    &call.id,
                    serde_json::json!({
                        "status": "proposed",
                        "tool": call.name,
                        "note": ASK_PROPOSAL_PENDING,
                    })
                    .to_string(),
                ));
                // The same proposal asked for twice is printed once.
                if !proposals.iter().any(|existing| {
                    existing.name == call.name && existing.arguments == call.arguments
                }) {
                    proposals.push(AnswerCall {
                        name: call.name,
                        arguments: call.arguments,
                    });
                }
            } else {
                // Not a name this build answers: telling the model is what lets
                // it correct itself, while proposing an unknown call to the
                // reader sent them to a button that cannot run it.
                messages.push(ChatMessage::tool_result(
                    &call.id,
                    serde_json::json!({
                        "error": format!(
                            "`{}` is not a tool this build can run; the tools are the ones in the request",
                            call.name
                        ),
                    })
                    .to_string(),
                ));
                ran += 1;
            }
        }
        if ran == 0 {
            break;
        }
        truncated = rounds == ASK_ROUNDS;
    }

    Ok(Answer {
        text,
        tool_calls: proposals,
        finish_reason,
        rounds,
        truncated,
    })
}

/// Run one read tool by name and answer the model with JSON text.
///
/// This is the CLI's half of the panel's dispatcher: the same tools, the same
/// façade calls the screens use, so a total the model quotes in the terminal is
/// the total the window shows. A refusal — a bad argument, a missing journal, a
/// tool this build does not have — goes back as `{"error": …}` for the model to
/// read and correct, never as a failed `ask`.
fn run_read_tool(
    ledger: Option<&Opened>,
    config_dir: &Path,
    name: &str,
    arguments: &str,
) -> String {
    match read_tool_answer(ledger, config_dir, name, arguments) {
        Ok(value) => value.to_string(),
        Err(message) => serde_json::json!({ "error": message }).to_string(),
    }
}

/// [`run_read_tool`] with the answer as a value, for the exhaustive match.
fn read_tool_answer(
    ledger: Option<&Opened>,
    config_dir: &Path,
    name: &str,
    arguments: &str,
) -> Result<serde_json::Value, String> {
    use serde_json::Value;

    let args: Value = if arguments.trim().is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(arguments)
            .map_err(|error| format!("the arguments are not JSON: {error}"))?
    };
    if !args.is_object() {
        return Err("the arguments must be a JSON object".to_string());
    }

    match name {
        "query" => {
            let opened = opened_or(ledger)?;
            let bql = text_arg(&args, &["query", "bql"])?;
            to_value(query(opened, &bql, ASK_QUERY_ROWS)?)
        }
        "accounts" => {
            let opened = opened_or(ledger)?;
            opened.with_loaded(|loaded| {
                let modified = crate::accounts::modified_accounts(&loaded.ledger, &loaded.baseline);
                to_value(crate::accounts::account_balances(&loaded.ledger, &modified))
            })
        }
        "transactions" => {
            let opened = opened_or(ledger)?;
            let account = text_arg(&args, &["account"])?;
            opened.with_loaded(|loaded| {
                to_value(crate::accounts::account_transaction_views(
                    &loaded.ledger,
                    &loaded.baseline,
                    &account,
                ))
            })
        }
        "files" => {
            let opened = opened_or(ledger)?;
            opened.with_loaded(|loaded| to_value(crate::files::journal_file_views(loaded)))
        }
        "grep" | "read_lines" => {
            let opened = opened_or(ledger)?;
            let dir = opened
                .root
                .parent()
                .ok_or_else(|| format!("{} has no directory to search", opened.root.display()))?;
            // The file tools already own path confinement, .gitignore handling
            // and the hashes a follow-up edit needs; re-implementing them here
            // is how a terminal would search something the panel cannot.
            let tool = if name == "grep" { "grep" } else { "read" };
            let answer =
                crate::tools::invoke(dir, tool, &args).map_err(|error| error.to_string())?;
            Ok(answer)
        }
        "budget_range" => {
            let opened = opened_or(ledger)?;
            let from = optional_text(&args, "from");
            let to = optional_text(&args, "to");
            to_value(budget_range(opened, from, to)?)
        }
        "index_series" => to_value(index_series(config_dir)?),
        "lifestyle_report" => {
            let opened = opened_or(ledger)?;
            let months = integer_arg(&args, &["months"])?;
            let index = text_arg(&args, &["index"])?;
            to_value(lifestyle(opened, config_dir, months, Some(&index))?)
        }
        "price_targets" => {
            let opened = opened_or(ledger)?;
            to_value(price_targets(opened))
        }
        other => Err(format!(
            "`{other}` is not a tool this build can run; the tools are the ones in the request"
        )),
    }
}

/// The opened ledger a read needs, or the sentence the model is told.
fn opened_or(ledger: Option<&Opened>) -> Result<&Opened, String> {
    ledger.ok_or_else(|| {
        "no ledger is loaded, so this tool has nothing to read; ask the user to open a journal"
            .to_string()
    })
}

/// One required string argument, under any of the names a schema may use.
fn text_arg(args: &serde_json::Value, names: &[&str]) -> Result<String, String> {
    for name in names {
        if let Some(value) = args.get(*name) {
            let text = value
                .as_str()
                .ok_or_else(|| format!("`{name}` must be a string"))?
                .trim();
            if !text.is_empty() {
                return Ok(text.to_string());
            }
        }
    }
    Err(format!("`{}` is required", names[0]))
}

/// One optional string argument; blank is absent, not an empty argument.
fn optional_text(args: &serde_json::Value, name: &str) -> Option<String> {
    args.get(name)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// One required whole-number argument.
fn integer_arg(args: &serde_json::Value, names: &[&str]) -> Result<u32, String> {
    let name = names[0];
    let value = args
        .get(name)
        .ok_or_else(|| format!("`{name}` is required"))?
        .as_u64()
        .ok_or_else(|| format!("`{name}` must be a whole number"))?;
    u32::try_from(value).map_err(|_| format!("`{name}` is too large"))
}

/// A serialisable answer as the JSON value the model reads.
fn to_value<T: serde::Serialize>(value: T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|error| format!("the answer could not be encoded: {error}"))
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
        let error =
            ask(&config, None, "how much did I spend?").expect_err("no model is configured");
        assert!(error.contains("model"), "{error}");
    }

    /// A model that answers from a fixed script, one response per call, and
    /// keeps every request it was sent.
    struct Scripted {
        answers: std::cell::RefCell<std::collections::VecDeque<crate::ai::ChatResponse>>,
        requests: std::cell::RefCell<Vec<crate::ai::ChatRequest>>,
    }

    impl Scripted {
        fn new(answers: Vec<crate::ai::ChatResponse>) -> Self {
            Self {
                answers: std::cell::RefCell::new(answers.into()),
                requests: std::cell::RefCell::new(Vec::new()),
            }
        }

        fn requests(&self) -> Vec<crate::ai::ChatRequest> {
            self.requests.borrow().clone()
        }
    }

    impl crate::ai_cmd::Backend for Scripted {
        fn models(
            &self,
            _config: &crate::ai::AiConfig,
        ) -> Result<Vec<crate::ai::ModelInfo>, String> {
            Ok(Vec::new())
        }

        fn chat(
            &self,
            _config: &crate::ai::AiConfig,
            request: crate::ai::ChatRequest,
        ) -> Result<crate::ai::ChatResponse, String> {
            self.requests.borrow_mut().push(request);
            self.answers
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| "the script ran out of answers".to_string())
        }
    }

    /// A turn that only calls tools: the call announced, no prose.
    fn calls(calls: Vec<crate::ai::ToolCall>) -> crate::ai::ChatResponse {
        crate::ai::ChatResponse {
            text: String::new(),
            tool_calls: calls,
            finish_reason: Some("tool_calls".to_string()),
            ..crate::ai::ChatResponse::default()
        }
    }

    /// A turn that answers in prose.
    fn says(text: &str) -> crate::ai::ChatResponse {
        crate::ai::ChatResponse {
            text: text.to_string(),
            finish_reason: Some("stop".to_string()),
            ..crate::ai::ChatResponse::default()
        }
    }

    #[test]
    fn ask_runs_a_read_tool_and_answers_from_its_result() {
        let (dir, opened) = opened();
        let config = dir.file("config");
        fs::create_dir_all(&config).unwrap();
        let query = "SELECT account, SUM(number) WHERE account ~ \"Expenses:\" GROUP BY account ORDER BY account";
        let scripted = Scripted::new(vec![
            calls(vec![crate::ai::ToolCall {
                id: "call-1".to_string(),
                name: "query".to_string(),
                arguments: serde_json::json!({ "query": query }).to_string(),
            }]),
            says("You spent 250.50 USD on groceries."),
        ]);

        let answer = ask_with(&scripted, &config, Some(&opened), "What did I spend?")
            .expect("the turn completes");

        // The prose is the model's, the read ran, and the loop took two calls.
        assert_eq!(answer.text, "You spent 250.50 USD on groceries.");
        assert!(answer.tool_calls.is_empty(), "a read is not a proposal");
        assert_eq!(answer.rounds, 2);
        assert!(!answer.truncated);
        assert_eq!(answer.finish_reason.as_deref(), Some("stop"));

        // The second request carries the call and its answer, in the wire's
        // order, and the answer is the BQL executor's own table.
        let requests = scripted.requests();
        assert_eq!(requests.len(), 2);
        let messages = &requests[1].messages;
        assert_eq!(messages.len(), 4, "system, user, call, answer");
        assert_eq!(messages[2].role, crate::ai::ChatRole::Assistant);
        assert_eq!(messages[2].tool_calls[0].id, "call-1");
        assert_eq!(messages[3].role, crate::ai::ChatRole::Tool);
        assert_eq!(messages[3].tool_call_id.as_deref(), Some("call-1"));
        let table: serde_json::Value =
            serde_json::from_str(&messages[3].text).expect("the tool answer is JSON");
        assert_eq!(
            table["columns"],
            serde_json::json!(["account", "SUM(number)"])
        );
        assert_eq!(
            table["rows"],
            serde_json::json!([["Expenses:Food:Groceries", "250.50"]])
        );

        // The write tools are offered too — the model may propose one — and the
        // read tool's schema came from the same table the panel sends.
        let names: Vec<&str> = requests[1]
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert!(names.contains(&"add_transaction"));
        assert!(names.contains(&"write_file"));
    }

    #[test]
    fn ask_proposes_a_write_and_never_applies_it() {
        let (dir, opened) = opened();
        let config = dir.file("config");
        fs::create_dir_all(&config).unwrap();
        let journal = dir.file("main.bean");
        let before = fs::read_to_string(&journal).expect("the journal is readable");

        // The model proposes an entry. Nothing runs it — not even the journal
        // file it names may be opened for writing.
        let arguments = serde_json::json!({
            "file": journal.display().to_string(),
            "date": "2026-02-01",
            "flag": "*",
            "payee": "Cafe",
            "narration": "Coffee",
            "postings": [
                { "account": "Expenses:Food:Groceries", "units": "4.50", "currency": "USD" },
                { "account": "Assets:Bank" }
            ]
        })
        .to_string();
        let scripted = Scripted::new(vec![calls(vec![crate::ai::ToolCall {
            id: "call-w".to_string(),
            name: "add_transaction".to_string(),
            arguments: arguments.clone(),
        }])]);

        let answer = ask_with(&scripted, &config, Some(&opened), "Add a coffee")
            .expect("the turn completes");

        assert!(answer.text.is_empty(), "the model said no prose");
        assert_eq!(
            answer.tool_calls,
            vec![AnswerCall {
                name: "add_transaction".to_string(),
                arguments,
            }],
            "the write call comes back as a proposal, arguments verbatim"
        );
        assert_eq!(
            answer.rounds, 1,
            "a write is not a read: no follow-up round"
        );

        // The proof: the journal is byte-for-byte what it was, and the write
        // tool was never answered as a tool message (the model is not told it
        // happened).
        assert_eq!(
            fs::read_to_string(&journal).expect("the journal is readable"),
            before,
            "the proposal must not have written anything"
        );
        assert_eq!(scripted.requests().len(), 1);
    }

    #[test]
    fn ask_answers_a_write_call_with_a_pending_note_in_a_mixed_round() {
        let (dir, opened) = opened();
        let config = dir.file("config");
        fs::create_dir_all(&config).unwrap();
        let arguments = serde_json::json!({
            "file": dir.file("main.bean").display().to_string(),
            "date": "2026-02-01",
            "flag": "*",
            "narration": "Coffee",
            "postings": [
                { "account": "Expenses:Food:Groceries", "units": "4.50", "currency": "USD" },
                { "account": "Assets:Bank" }
            ]
        })
        .to_string();
        let scripted = Scripted::new(vec![
            calls(vec![
                crate::ai::ToolCall {
                    id: "call-read".to_string(),
                    name: "files".to_string(),
                    arguments: "{}".to_string(),
                },
                crate::ai::ToolCall {
                    id: "call-write".to_string(),
                    name: "add_transaction".to_string(),
                    arguments: arguments.clone(),
                },
            ]),
            says("I proposed the entry; apply it in the panel."),
        ]);

        let answer = ask_with(&scripted, &config, Some(&opened), "Add a coffee")
            .expect("the turn completes");
        assert_eq!(answer.rounds, 2);
        assert_eq!(answer.tool_calls.len(), 1, "the write is reported once");

        // Every call is answered — an assistant turn whose call has no tool
        // answer would be a conversation the endpoint refuses — and the write's
        // answer says it is pending, not done.
        let requests = scripted.requests();
        let messages = &requests[1].messages;
        assert_eq!(messages.len(), 5, "system, user, call, read, write");
        let read_answer: serde_json::Value =
            serde_json::from_str(&messages[3].text).expect("the read answer is JSON");
        assert_eq!(
            read_answer[0]["path"],
            serde_json::json!(dir.file("main.bean").display().to_string())
        );
        let write_answer: serde_json::Value =
            serde_json::from_str(&messages[4].text).expect("the write answer is JSON");
        assert_eq!(write_answer["status"], "proposed");
        assert!(
            write_answer["note"]
                .as_str()
                .is_some_and(|note| note.contains("Nothing has been written"))
        );

        // Still nothing on disk: the mixed round did not turn the proposal into
        // a write.
        let journal = fs::read_to_string(dir.file("main.bean")).expect("the journal is readable");
        assert!(
            !journal.contains("Coffee"),
            "the proposal was not applied:\n{journal}"
        );
    }

    #[test]
    fn ask_without_a_ledger_tells_the_model_the_tools_have_nothing_to_read() {
        let dir = TempDir::new("api-ask-no-ledger");
        let config = dir.file("config");
        fs::create_dir_all(&config).unwrap();
        let scripted = Scripted::new(vec![
            calls(vec![crate::ai::ToolCall {
                id: "call-f".to_string(),
                name: "files".to_string(),
                arguments: "{}".to_string(),
            }]),
            says("No journal is open."),
        ]);

        let answer =
            ask_with(&scripted, &config, None, "Which files?").expect("the turn completes");
        assert_eq!(answer.text, "No journal is open.");
        let requests = scripted.requests();
        let tool_answer: serde_json::Value =
            serde_json::from_str(&requests[1].messages[3].text).expect("the refusal is JSON");
        assert!(
            tool_answer["error"]
                .as_str()
                .is_some_and(|error| error.contains("no ledger is loaded")),
            "the model is told why: {tool_answer}"
        );
    }
}
