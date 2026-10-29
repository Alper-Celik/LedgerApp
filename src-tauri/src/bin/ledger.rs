// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `ledger` — the app's backend on the command line.
//!
//! Every subcommand is a call into `ledger_app_lib::api`, the same façade the
//! Tauri commands wrap. That is the whole point: one loader, one BQL executor,
//! one budget reader, one report writer, so the terminal and the window cannot
//! disagree about a total. What this file owns is what a binary must own —
//! argument parsing, the table layout, and the exit code.
//!
//! # Exit codes
//!
//! `0` the command answered (or the journal passed `check`); `1` the command
//! failed, with the reason on stderr; `2` the command was misused — clap's own
//! usage errors exit 2 as well, so a script can treat every 2 alike.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chart_render::TerminalOptions;
use clap::{Parser, Subcommand, ValueEnum};
use ledger_app_lib::api;

#[derive(Parser)]
#[command(
    name = "ledger",
    version,
    about = "Inspect a Beancount journal with the ledger-app backend",
    long_about = "Inspect a Beancount journal with the ledger-app backend.\n\n\
        Every subcommand runs the app's own loader, BQL executor, budget reader \
        and report writer; nothing is re-implemented here. The journal is the \
        `--ledger` path, else LEDGER_PATH, else ./main.bean. Read commands take \
        --json for scripting, and the answers' money figures are always the \
        backend's exact decimal strings."
)]
struct Cli {
    /// The journal — or a folder holding one — to read.
    #[arg(long, global = true, value_name = "PATH")]
    ledger: Option<PathBuf>,

    /// Print the command's answer as JSON instead of a table.
    #[arg(long, global = true)]
    json: bool,

    /// The app's config directory (indexes, assistant settings). Defaults to
    /// LEDGER_CONFIG_DIR, else the platform's app-config path for ledger-app.
    #[arg(long, global = true, value_name = "DIR")]
    config_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Load the journal and report its directives and every problem found.
    Check {
        /// The journal or folder; the last word beats --ledger.
        path: Option<PathBuf>,
    },
    /// Run one BQL statement and print its rows.
    Query {
        /// The statement, e.g. "SELECT account, sum(number) GROUP BY account".
        bql: String,
        /// Rows to return; 0 means every row.
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Show the envelope budget for one month or a window of months.
    Budget {
        /// Exactly this many months, ending at the newest budgeted month.
        #[arg(long)]
        months: Option<u32>,
        /// The window's first month, YYYY-MM (default: six months back).
        #[arg(long, value_name = "YYYY-MM")]
        from: Option<String>,
        /// The window's last month, YYYY-MM (default: the newest budgeted one).
        #[arg(long, value_name = "YYYY-MM")]
        to: Option<String>,
    },
    /// Run one report over the last N months.
    Report {
        /// Which report to run.
        #[arg(value_enum)]
        report: ReportKind,
        /// Months in the window, ending at the newest entry.
        #[arg(long, default_value_t = 12)]
        months: u32,
        /// The index series for the lifestyle report (default: the only one).
        #[arg(long, value_name = "ID")]
        index: Option<String>,
    },
    /// List the ledger's commodities, and optionally fetch or write quotes.
    Prices {
        /// Fetch a quote for every commodity that names its quote currency.
        #[arg(long)]
        fetch: bool,
        /// Append the fetched quotes to this journal file as price directives.
        #[arg(long, value_name = "FILE", requires = "fetch")]
        write: Option<PathBuf>,
    },
    /// Inspect, download or import index series.
    Index {
        #[command(subcommand)]
        command: IndexCommand,
    },
    /// Ask the assistant: it may run the app's read tools itself, and a write it
    /// wants is printed as a proposal, never applied.
    Ask {
        /// The question, as one argument.
        prompt: String,
    },
    /// Render a chart spec for the terminal (kitty graphics or Unicode).
    Chart {
        /// The chart spec JSON file.
        spec: PathBuf,
        /// Chars of width to draw in; the renderer's default when omitted.
        #[arg(long, value_name = "N")]
        width: Option<usize>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum ReportKind {
    /// Assets minus liabilities at each month's end.
    NetWorth,
    /// Money in and money out per month.
    Cashflow,
    /// Spending per two-level expense branch, one column per month.
    Categories,
    /// Spending restated in the first month's money with an index series.
    Lifestyle,
}

#[derive(Subcommand)]
enum IndexCommand {
    /// List the stored index series.
    List,
    /// Download one provider's series and store it.
    Refresh {
        /// The series id, e.g. us-cpi.
        id: String,
    },
    /// Import a publisher's date,value CSV as a series.
    Import {
        /// The series id to store it under, e.g. tr-cpi.
        id: String,
        /// The label the reports show.
        #[arg(long)]
        label: String,
        /// The publisher's name.
        #[arg(long)]
        source: String,
        /// The currency the index is expressed in.
        #[arg(long)]
        currency: String,
        /// The CSV file, two columns: date,value.
        #[arg(long, value_name = "PATH")]
        csv: PathBuf,
    },
}

/// Why a command did not answer, and the exit code that says which kind.
enum CliError {
    /// The arguments cannot be carried out together; exit 2.
    Usage(String),
    /// The command ran and failed; exit 1.
    Failed(String),
}

impl From<String> for CliError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            let (message, code) = match error {
                CliError::Usage(message) => (message, 2),
                CliError::Failed(message) => (message, 1),
            };
            eprintln!("ledger: {message}");
            ExitCode::from(code)
        }
    }
}

fn run(cli: &Cli) -> Result<u8, CliError> {
    match &cli.command {
        Command::Check { path } => check(cli, path.as_deref()),
        Command::Query { bql, limit } => query(cli, bql, *limit),
        Command::Budget { months, from, to } => budget(cli, *months, from.clone(), to.clone()),
        Command::Report {
            report: kind,
            months,
            index,
        } => report(cli, *kind, *months, index.as_deref()),
        Command::Prices { fetch, write } => prices(cli, *fetch, write.as_deref()),
        Command::Index { command } => index(cli, command),
        Command::Ask { prompt } => ask(cli, prompt),
        Command::Chart { spec, width } => chart(spec, *width),
    }
}

// ------------------------------------------------------------------ setup ---

/// The journal every command but `chart` reads: the flag, else the environment
/// variable the app's own launcher honours, else `main.bean` beside the cwd.
fn ledger_path(cli: &Cli) -> PathBuf {
    cli.ledger
        .clone()
        .or_else(|| std::env::var_os("LEDGER_PATH").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("main.bean"))
}

fn open_ledger(cli: &Cli) -> Result<api::Opened, CliError> {
    Ok(api::open(&ledger_path(cli))?)
}

/// The app's config directory: the flag, else the environment variable, else
/// the path the Tauri app resolves for the same identifier (`tauri.conf.json`),
/// so `index list` sees what the window installed and `ask` reads its settings.
fn config_dir(cli: &Cli) -> PathBuf {
    if let Some(dir) = &cli.config_dir {
        return dir.clone();
    }
    if let Some(dir) = std::env::var_os("LEDGER_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    const IDENTIFIER: &str = "dev.alper-celik.ledger-app";
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
        })
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    };
    base.unwrap_or_else(|| PathBuf::from(".")).join(IDENTIFIER)
}

// ------------------------------------------------------------------ output ---

/// Write the answer, treating a closed pipe (`… | head`) as success: the reader
/// asked for less, which is not this command's failure.
fn emit(write: impl FnOnce(&mut dyn Write) -> io::Result<()>) -> Result<u8, CliError> {
    let mut out = io::stdout().lock();
    match write(&mut out).and_then(|()| out.flush()) {
        Ok(()) => Ok(0),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(0),
        Err(error) => Err(CliError::Failed(format!(
            "could not write to stdout: {error}"
        ))),
    }
}

fn emit_json<T: serde::Serialize>(value: &T) -> Result<u8, CliError> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| CliError::Failed(format!("could not encode the answer: {error}")))?;
    emit(|out| writeln!(out, "{text}"))
}

/// The cells as an aligned table.
///
/// Widths are character counts — ledger text is not wide — and a cell that
/// reads as a decimal figure is right-aligned so decimal points stack. No cell
/// is reformatted: the backend's exact money strings go out as they came,
/// which is why nothing here parses one into a number.
fn render_table(columns: &[String], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = columns.iter().map(|column| text_width(column)).collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(index) {
                *width = (*width).max(text_width(cell));
            }
        }
    }

    let mut text = String::new();
    push_row(
        &mut text,
        &columns.iter().map(String::as_str).collect::<Vec<_>>(),
        &widths,
    );
    let rule = widths
        .iter()
        .map(|width| "-".repeat(*width))
        .collect::<Vec<_>>()
        .join("  ");
    text.push_str(&rule);
    text.push('\n');
    for row in rows {
        let cells = row.iter().map(String::as_str).collect::<Vec<_>>();
        push_row(&mut text, &cells, &widths);
    }
    text
}

fn push_row(text: &mut String, cells: &[&str], widths: &[usize]) {
    for (index, width) in widths.iter().enumerate() {
        if index > 0 {
            text.push_str("  ");
        }
        let cell = cells.get(index).copied().unwrap_or("");
        let pad = width.saturating_sub(text_width(cell));
        if is_amount(cell) {
            text.push_str(&" ".repeat(pad));
            text.push_str(cell);
        } else if index + 1 == widths.len() {
            text.push_str(cell);
        } else {
            text.push_str(cell);
            text.push_str(&" ".repeat(pad));
        }
    }
    text.push('\n');
}

fn text_width(cell: &str) -> usize {
    cell.chars().count()
}

/// Whether a cell is a decimal figure the table should right-align. A look at
/// the characters only: no cell is parsed, and a month label's inner `-` keeps
/// dates left-aligned.
fn is_amount(cell: &str) -> bool {
    let body = cell.strip_prefix(['-', '+']).unwrap_or(cell);
    !body.is_empty()
        && body.chars().any(|c| c.is_ascii_digit())
        && body
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == ',')
}

fn issue_severity(issue: &api::Issue) -> &'static str {
    match issue.severity {
        api::Severity::Error => "error",
        api::Severity::Warning => "warning",
    }
}

fn issue_counts(issues: &[api::Issue]) -> (usize, usize) {
    let errors = issues
        .iter()
        .filter(|issue| issue.severity == api::Severity::Error)
        .count();
    (errors, issues.len() - errors)
}

// --------------------------------------------------------------- commands ---

fn check(cli: &Cli, path: Option<&Path>) -> Result<u8, CliError> {
    let path = path.map_or_else(|| ledger_path(cli), Path::to_path_buf);
    let check = api::check(&path)?;
    let (errors, warnings) = issue_counts(&check.errors);

    if cli.json {
        emit_json(&check)?;
        if check.ok {
            return Ok(0);
        }
        eprintln!("ledger: {errors} error(s) in the journal");
        return Ok(1);
    }

    let summary = format!(
        "{}: {} directives, {errors} errors, {warnings} warnings",
        check.root.display(),
        check.directives
    );
    let code = emit(|out| {
        writeln!(out, "{summary}")?;
        if check.errors.is_empty() {
            return Ok(());
        }
        writeln!(out)?;
        let columns: Vec<String> = ["SEVERITY", "CODE", "LOCATION", "MESSAGE"]
            .map(String::from)
            .to_vec();
        let rows: Vec<Vec<String>> = check
            .errors
            .iter()
            .map(|issue| {
                vec![
                    issue_severity(issue).to_string(),
                    issue.code.clone(),
                    issue.location().unwrap_or_else(|| "-".to_string()),
                    issue.message.clone(),
                ]
            })
            .collect();
        out.write_all(render_table(&columns, &rows).as_bytes())
    })?;
    if check.ok {
        Ok(code)
    } else {
        eprintln!("ledger: {errors} error(s) in the journal");
        Ok(1)
    }
}

fn query(cli: &Cli, bql: &str, limit: u32) -> Result<u8, CliError> {
    let opened = open_ledger(cli)?;
    let table = api::query(&opened, bql, limit)?;
    if cli.json {
        return emit_json(&table);
    }
    let text = render_table(&table.columns, &table.rows);
    let code = emit(|out| out.write_all(text.as_bytes()))?;
    if table.truncated {
        eprintln!(
            "ledger: truncated at {} rows; raise --limit (0 means no cap) or add LIMIT",
            table.rows.len()
        );
    }
    Ok(code)
}

fn budget(
    cli: &Cli,
    months: Option<u32>,
    from: Option<String>,
    to: Option<String>,
) -> Result<u8, CliError> {
    if months.is_some() && (from.is_some() || to.is_some()) {
        return Err(CliError::Usage(
            "--months cannot be combined with --from/--to".to_string(),
        ));
    }
    let opened = open_ledger(cli)?;
    if let Some(months) = months {
        return budget_range_answer(cli, &api::budget_months(&opened, months)?);
    }
    if from.is_some() || to.is_some() {
        return budget_range_answer(cli, &api::budget_range(&opened, from, to)?);
    }

    let month = api::budget_month(&opened, None)?;
    if cli.json {
        return emit_json(&month);
    }
    emit(|out| {
        writeln!(
            out,
            "{} · {} · to be budgeted {}",
            month.month, month.currency, month.to_budget
        )?;
        if month.groups.is_empty() {
            return Ok(());
        }
        writeln!(out)?;
        let columns: Vec<String> = ["GROUP", "ENVELOPE", "BUDGETED", "SPENT", "BALANCE"]
            .map(String::from)
            .to_vec();
        let mut rows: Vec<Vec<String>> = Vec::new();
        for group in &month.groups {
            rows.push(vec![
                group.name.clone(),
                String::new(),
                group.budgeted.clone(),
                group.spent.clone(),
                group.balance.clone(),
            ]);
            for envelope in &group.envelopes {
                rows.push(vec![
                    group.name.clone(),
                    envelope.name.clone(),
                    envelope.budgeted.clone(),
                    envelope.spent.clone(),
                    envelope.balance.clone(),
                ]);
            }
        }
        out.write_all(render_table(&columns, &rows).as_bytes())
    })
}

fn budget_range_answer(cli: &Cli, range: &api::BudgetRange) -> Result<u8, CliError> {
    if cli.json {
        return emit_json(range);
    }
    emit(|out| {
        writeln!(out, "{}..{} · {}", range.from, range.to, range.currency)?;
        if range.months.is_empty() {
            return writeln!(out, "no budget data in this window");
        }
        writeln!(out)?;
        let columns: Vec<String> = ["MONTH", "TO BE BUDGETED", "BUDGETED", "SPENT", "BALANCE"]
            .map(String::from)
            .to_vec();
        let rows: Vec<Vec<String>> = range
            .months
            .iter()
            .map(|month| {
                vec![
                    month.month.clone(),
                    month.to_budget.clone(),
                    month.budgeted.clone(),
                    month.spent.clone(),
                    month.balance.clone(),
                ]
            })
            .collect();
        out.write_all(render_table(&columns, &rows).as_bytes())
    })
}

fn report(cli: &Cli, report: ReportKind, months: u32, index: Option<&str>) -> Result<u8, CliError> {
    if months == 0 {
        return Err(CliError::Usage("--months needs at least 1".to_string()));
    }
    if index.is_some() && !matches!(report, ReportKind::Lifestyle) {
        return Err(CliError::Usage(
            "--index only applies to the lifestyle report".to_string(),
        ));
    }
    let opened = open_ledger(cli)?;
    match report {
        ReportKind::NetWorth => {
            let rows = api::net_worth(&opened, months);
            if cli.json {
                return emit_json(&rows);
            }
            emit(|out| {
                let columns: Vec<String> = ["MONTH", "NET WORTH", "CURRENCY"]
                    .map(String::from)
                    .to_vec();
                let cells: Vec<Vec<String>> = rows
                    .iter()
                    .map(|row| vec![row.month.clone(), row.value.clone(), row.currency.clone()])
                    .collect();
                out.write_all(render_table(&columns, &cells).as_bytes())
            })
        }
        ReportKind::Cashflow => {
            let rows = api::cashflow(&opened, months);
            if cli.json {
                return emit_json(&rows);
            }
            emit(|out| {
                let columns: Vec<String> = ["MONTH", "INFLOW", "OUTFLOW", "CURRENCY"]
                    .map(String::from)
                    .to_vec();
                let cells: Vec<Vec<String>> = rows
                    .iter()
                    .map(|row| {
                        vec![
                            row.month.clone(),
                            row.inflow.clone(),
                            row.outflow.clone(),
                            row.currency.clone(),
                        ]
                    })
                    .collect();
                out.write_all(render_table(&columns, &cells).as_bytes())
            })
        }
        ReportKind::Categories => {
            let rows = api::categories(&opened, months);
            if cli.json {
                return emit_json(&rows);
            }
            emit(|out| {
                let Some(first) = rows.first() else {
                    return writeln!(out, "no expenses in this window");
                };
                let mut columns = vec!["ACCOUNT".to_string()];
                columns.extend(first.months.iter().map(|month| month.month.clone()));
                let cells: Vec<Vec<String>> = rows
                    .iter()
                    .map(|row| {
                        let mut cells = vec![row.account.clone()];
                        cells.extend(row.months.iter().map(|month| month.spent.clone()));
                        cells
                    })
                    .collect();
                writeln!(out, "spending in {}", first.currency)?;
                out.write_all(render_table(&columns, &cells).as_bytes())
            })
        }
        ReportKind::Lifestyle => {
            let config = config_dir(cli);
            let report = api::lifestyle(&opened, &config, months, index)?;
            if cli.json {
                return emit_json(&report);
            }
            emit(|out| {
                writeln!(out, "index {} ({})", report.index.label, report.index.id)?;
                if let Some(change) = &report.index.change {
                    writeln!(out, "index change over the window: {change}%")?;
                }
                let Some(first) = report.totals.first() else {
                    return writeln!(out, "no expenses in this window");
                };
                writeln!(out)?;
                writeln!(out, "total spending in {}", first.currency)?;
                let columns: Vec<String> = ["MONTH", "NOMINAL", "REAL"].map(String::from).to_vec();
                let cells: Vec<Vec<String>> = report
                    .totals
                    .iter()
                    .map(|total| {
                        vec![
                            total.month.clone(),
                            total.nominal.clone(),
                            total.real.clone(),
                        ]
                    })
                    .collect();
                out.write_all(render_table(&columns, &cells).as_bytes())?;

                if report.categories.is_empty() {
                    return Ok(());
                }
                writeln!(out)?;
                writeln!(out, "by category")?;
                let columns: Vec<String> = ["ACCOUNT", "MONTH", "NOMINAL", "REAL"]
                    .map(String::from)
                    .to_vec();
                let cells: Vec<Vec<String>> = report
                    .categories
                    .iter()
                    .flat_map(|category| {
                        category.months.iter().map(move |month| {
                            vec![
                                category.account.clone(),
                                month.month.clone(),
                                month.nominal.clone(),
                                month.real.clone(),
                            ]
                        })
                    })
                    .collect();
                out.write_all(render_table(&columns, &cells).as_bytes())
            })
        }
    }
}

fn prices(cli: &Cli, fetch: bool, write: Option<&Path>) -> Result<u8, CliError> {
    let opened = open_ledger(cli)?;
    let targets = api::price_targets(&opened);

    let mut fetched: Option<Vec<api::PriceFetch>> = None;
    if fetch {
        let mut requests = Vec::new();
        for target in &targets {
            let currency = target.currency.as_deref().unwrap_or("");
            if currency.is_empty() {
                eprintln!(
                    "ledger: skipping {}: the ledger names no quote currency for it",
                    target.commodity
                );
                continue;
            }
            // `USD in USD` is not a quote; a provider can only answer it with an
            // error, so it never becomes a request.
            if currency == target.commodity {
                continue;
            }
            requests.push(api::QuoteRequest {
                commodity: target.commodity.clone(),
                currency: currency.to_string(),
                source: None,
                date: None,
            });
        }
        if requests.is_empty() {
            return Err(CliError::Failed(
                "nothing to fetch: every commodity is quoted in itself or names no currency"
                    .to_string(),
            ));
        }
        let outcomes = api::fetch_prices(&opened, requests)?;
        if let Some(file) = write {
            let quotes: Vec<api::PriceQuote> = outcomes
                .iter()
                .filter_map(|outcome| match outcome {
                    api::PriceFetch::Quote(quote) => Some(quote.clone()),
                    api::PriceFetch::Failed(_) => None,
                })
                .collect();
            api::write_prices(&opened, &file.display().to_string(), &quotes)?;
            eprintln!(
                "ledger: appended {} quote(s) to {}",
                quotes.len(),
                file.display()
            );
        }
        fetched = Some(outcomes);
    }

    if cli.json {
        return match &fetched {
            Some(fetched) => emit_json(&serde_json::json!({
                "targets": targets,
                "fetched": fetched,
            })),
            None => emit_json(&targets),
        };
    }

    emit(|out| {
        let columns: Vec<String> = ["COMMODITY", "CURRENCY", "KIND", "LATEST", "AS OF", "SOURCE"]
            .map(String::from)
            .to_vec();
        let cells: Vec<Vec<String>> = targets
            .iter()
            .map(|target| {
                let latest = target.latest.as_ref();
                vec![
                    target.commodity.clone(),
                    target.currency.clone().unwrap_or_else(|| "-".to_string()),
                    target.kind.clone(),
                    latest.map_or_else(|| "-".to_string(), |quote| quote.price.clone()),
                    latest.map_or_else(|| "-".to_string(), |quote| quote.date.clone()),
                    latest.map_or_else(|| "-".to_string(), |quote| quote.source.clone()),
                ]
            })
            .collect();
        out.write_all(render_table(&columns, &cells).as_bytes())?;

        let Some(fetched) = &fetched else {
            return Ok(());
        };
        writeln!(out)?;
        let columns: Vec<String> = ["COMMODITY", "CURRENCY", "DATE", "PRICE", "DETAIL"]
            .map(String::from)
            .to_vec();
        let cells: Vec<Vec<String>> = fetched
            .iter()
            .map(|outcome| match outcome {
                api::PriceFetch::Quote(quote) => vec![
                    quote.commodity.clone(),
                    quote.currency.clone(),
                    quote.date.clone(),
                    quote.price.clone(),
                    quote.source.clone(),
                ],
                api::PriceFetch::Failed(failure) => vec![
                    failure.commodity.clone(),
                    failure.currency.clone(),
                    "-".to_string(),
                    "-".to_string(),
                    format!("failed via {}: {}", failure.source, failure.message),
                ],
            })
            .collect();
        out.write_all(render_table(&columns, &cells).as_bytes())
    })
}

fn index(cli: &Cli, command: &IndexCommand) -> Result<u8, CliError> {
    let config = config_dir(cli);
    match command {
        IndexCommand::List => {
            let series = api::index_series(&config)?;
            if cli.json {
                return emit_json(&series);
            }
            emit(|out| {
                if series.is_empty() {
                    return writeln!(
                        out,
                        "no index series stored in {}",
                        api::indexes_dir(&config).display()
                    );
                }
                let columns: Vec<String> = ["ID", "LABEL", "SOURCE", "CURRENCY", "POINTS", "RANGE"]
                    .map(String::from)
                    .to_vec();
                let cells: Vec<Vec<String>> = series
                    .iter()
                    .map(|series| {
                        let first = series.points.first().map(|point| point.date.clone());
                        let last = series.points.last().map(|point| point.date.clone());
                        vec![
                            series.id.clone(),
                            series.label.clone(),
                            series.source.clone(),
                            series.currency.clone(),
                            series.points.len().to_string(),
                            format!(
                                "{}..{}",
                                first.unwrap_or_else(|| "-".to_string()),
                                last.unwrap_or_else(|| "-".to_string())
                            ),
                        ]
                    })
                    .collect();
                out.write_all(render_table(&columns, &cells).as_bytes())
            })
        }
        IndexCommand::Refresh { id } => {
            let series = api::index_refresh(&config, id)?;
            series_answer(cli, &series)
        }
        IndexCommand::Import {
            id,
            label,
            source,
            currency,
            csv,
        } => {
            let text = std::fs::read_to_string(csv)
                .map_err(|error| CliError::Failed(format!("{}: {error}", csv.display())))?;
            let series = api::index_import(&config, id, label, source, currency, &text)?;
            series_answer(cli, &series)
        }
    }
}

fn series_answer(cli: &Cli, series: &api::IndexSeries) -> Result<u8, CliError> {
    if cli.json {
        return emit_json(series);
    }
    let first = series.points.first().map(|point| point.date.as_str());
    let last = series.points.last().map(|point| point.date.as_str());
    let range = match (first, last) {
        (Some(first), Some(last)) => format!("{first}..{last}"),
        _ => "-".to_string(),
    };
    emit(|out| {
        writeln!(
            out,
            "{}: {} ({}, {}), {} points {range}",
            series.id,
            series.label,
            series.source,
            series.currency,
            series.points.len()
        )
    })
}

fn ask(cli: &Cli, prompt: &str) -> Result<u8, CliError> {
    let config = config_dir(cli);
    // The assistant's read tools answer from the same opened ledger every other
    // command uses. A journal that cannot be loaded is not fatal to the
    // question: the model is told the tools have nothing to read, and the
    // reason is on stderr so the asker knows why a figure is missing.
    let opened = match api::open(&ledger_path(cli)) {
        Ok(opened) => Some(opened),
        Err(error) => {
            eprintln!("ledger: the journal could not be opened for the assistant's tools: {error}");
            None
        }
    };
    let answer = api::ask(&config, opened.as_ref(), prompt)?;
    if cli.json {
        return emit_json(&answer);
    }
    if !answer.text.is_empty() {
        let text = &answer.text;
        emit(|out| {
            out.write_all(text.as_bytes())?;
            if text.ends_with('\n') {
                Ok(())
            } else {
                out.write_all(b"\n")
            }
        })?;
    }
    // Reads ran; a write did not. It is a proposal for the user, printed with
    // the exact arguments the model produced, and nothing on disk has changed.
    for call in &answer.tool_calls {
        let arguments = match serde_json::from_str::<serde_json::Value>(&call.arguments) {
            Ok(value) => {
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| call.arguments.clone())
            }
            Err(_) => call.arguments.clone(),
        };
        eprintln!("ledger: proposed `{}` (not applied):", call.name);
        for line in arguments.lines() {
            eprintln!("  {line}");
        }
    }
    if !answer.tool_calls.is_empty() {
        eprintln!("ledger: nothing was written; review and apply the proposal in the app.");
    }
    if answer.truncated {
        eprintln!(
            "ledger: stopped after {} tool rounds; ask again to continue.",
            api::ASK_ROUNDS
        );
    }
    if let Some(reason) = &answer.finish_reason
        && reason != "stop"
        && reason != "tool_calls"
    {
        eprintln!("ledger: the model stopped because of `{reason}`");
    }
    Ok(0)
}

fn chart(spec: &Path, width: Option<usize>) -> Result<u8, CliError> {
    let text = std::fs::read_to_string(spec)
        .map_err(|error| CliError::Failed(format!("{}: {error}", spec.display())))?;
    let options = match width {
        Some(columns) => TerminalOptions {
            columns,
            ..TerminalOptions::default()
        },
        None => TerminalOptions::default(),
    };
    // The renderer chooses between a kitty escape and a Unicode chart; the bytes
    // go out untouched, because re-encoding an escape sequence corrupts it.
    let bytes = api::chart_with(&text, options)?;
    emit(|out| {
        out.write_all(&bytes)?;
        if bytes.last() == Some(&b'\n') {
            Ok(())
        } else {
            out.write_all(b"\n")
        }
    })
}
