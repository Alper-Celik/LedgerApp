// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! BQL: one statement over the loaded ledger, answered as a table of text
//! cells.
//!
//! `rustledger_query` owns the language and the arithmetic; this module owns
//! the three things a UI and a model-facing tool need on top of it:
//!
//! - **One statement.** The tool is one call, so a second `;`-separated
//!   statement is refused here with a clear message rather than surfacing as a
//!   parser complaint about an unexpected token.
//! - **Text cells.** Every cell is a string already formatted the way the
//!   table wants it. Amounts and numbers come from the ledger's own
//!   [`Decimal`](rust_decimal::Decimal), so the text is the exact decimal the
//!   book holds — no float ever touches it, and the frontend never re-parses
//!   to display.
//! - **A cap.** The assistant asks for a bounded table; `truncated` says the
//!   cut happened, so it never mistakes a slice for the whole answer.
//!
//! The query runs over [`Ledger::balance_view`] — the booked, pad-merged
//! stream the register and the account tree read — so a query about balances
//! sees the same numbers the rest of the app shows. The loader's own docs name
//! this as the stream BQL hosts should query.

use rustledger_loader::Ledger;
use rustledger_query::{Executor, QueryResult, Value};

/// The rows a `query` tool call hands the model, when the command layer has no
/// reason to pick another number. Small enough that one answer stays readable
/// in a conversation, large enough for a year of monthly totals.
pub const TOOL_MAX_ROWS: u32 = 200;

/// One query answer: the header, the rows, and whether the cap cut them off.
///
/// `rows` are rectangular with `columns`: every row has exactly one cell per
/// column. `truncated` is true only when the executor produced more rows than
/// the caller's cap allowed, so an empty table and a cut one never look alike.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct QueryTable {
    /// The header, in the statement's target order.
    pub columns: Vec<String>,
    /// One string per cell, row-major.
    pub rows: Vec<Vec<String>>,
    /// Whether more rows existed than [`run`] returned.
    pub truncated: bool,
}

/// Run one BQL statement over `ledger` and answer its rows as text.
///
/// `max_rows` caps the row count; `0` means no cap. A capped answer keeps the
/// statement's own order and reports the cut in
/// [`QueryTable::truncated`] — it never re-runs the statement with a different
/// `LIMIT`, so what comes back is exactly the statement's first rows.
///
/// # Errors
///
/// A blank statement, a second `;`-separated statement, a parse failure, or an
/// execution failure (unknown column, type mismatch, …), each as a message
/// with the offending part named.
pub fn run(ledger: &Ledger, bql: &str, max_rows: u32) -> Result<QueryTable, String> {
    if bql.trim().is_empty() {
        return Err("a BQL statement is required".to_string());
    }
    if let Some(at) = second_statement_at(bql) {
        return Err(format!(
            "one statement at a time: a second one starts at byte {at}"
        ));
    }
    let statement = rustledger_query::parse(bql).map_err(|error| format!("BQL: {error}"))?;

    // `set_account_types` is what makes `POSSIGN` / `ACCOUNT_SORTKEY` classify
    // a ledger that renamed its roots (`option "name_expenses"`); the
    // discrepancy map is the balance checker's own answer for
    // `#balances.discrepancy`, which cannot be re-derived here without a
    // second implementation of the tolerance rule.
    let directives = ledger.balance_view();
    let mut executor = Executor::new(&directives);
    executor.set_account_types(ledger.options.to_account_types());
    executor.set_balance_discrepancies(ledger.balance_discrepancies.iter().map(|failure| {
        (
            failure.date,
            failure.account.as_str().to_owned(),
            failure.amount.currency.as_str().to_owned(),
            failure.asserted,
            failure.amount.clone(),
        )
    }));

    let result = executor
        .execute(&statement)
        .map_err(|error| format!("BQL: {error}"))?;
    Ok(table(result, max_rows))
}

/// The tool descriptor for `run`, as the chat request's `tools` list carries
/// it: name, description and a JSON Schema for `{ "query": string }`.
///
/// One source of truth: the assistant advertises exactly what [`run`] accepts
/// and the command layer dispatches by the same name, so the three cannot drift
/// The executor's answer as the app's table, capped at `max_rows`.
fn table(mut result: QueryResult, max_rows: u32) -> QueryTable {
    let cap = usize::try_from(max_rows).unwrap_or(usize::MAX);
    let truncated = max_rows != 0 && result.rows.len() > cap;
    if truncated {
        result.rows.truncate(cap);
    }
    let rows = result
        .rows
        .iter()
        .map(|row| row.iter().map(cell).collect())
        .collect();
    QueryTable {
        columns: result.columns,
        rows,
        truncated,
    }
}

/// One cell as text.
///
/// `Null` is the empty cell rather than the word `NULL`: an absent payee or an
/// unset metadata key should read as blank in a table, and the header already
/// says what the column is.
fn cell(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Integer(number) => number.to_string(),
        Value::Date(date) => date.to_string(),
        Value::Boolean(flag) => flag.to_string(),
        Value::Amount(amount) => amount.to_string(),
        Value::Position(position) => position.to_string(),
        Value::Inventory(inventory) => inventory.to_string(),
        Value::StringSet(values) => values.join(", "),
        Value::Set(values) => values.iter().map(cell).collect::<Vec<_>>().join(", "),
        Value::Metadata(metadata) => {
            // A hash map with no order of its own: sorted so the same entry
            // always renders the same text.
            let mut pairs: Vec<(&String, &rustledger_core::MetaValue)> = metadata.iter().collect();
            pairs.sort_by(|(left, _), (right, _)| left.cmp(right));
            pairs
                .into_iter()
                .map(|(key, value)| format!("{key}: {value}"))
                .collect::<Vec<_>>()
                .join(", ")
        }
        Value::Interval(interval) => interval_text(interval),
        Value::Object(fields) => fields
            .iter()
            .map(|(key, value)| format!("{key}: {}", cell(value)))
            .collect::<Vec<_>>()
            .join(", "),
        Value::Null => String::new(),
    }
}

/// An interval as `3 months` — the unit named the way the statement's own
/// keyword reads, pluralized.
fn interval_text(interval: &rustledger_query::Interval) -> String {
    let unit = match interval.unit {
        rustledger_query::IntervalUnit::Day => "day",
        rustledger_query::IntervalUnit::Week => "week",
        rustledger_query::IntervalUnit::Month => "month",
        rustledger_query::IntervalUnit::Quarter => "quarter",
        rustledger_query::IntervalUnit::Year => "year",
    };
    if interval.count == 1 {
        format!("{} {unit}", interval.count)
    } else {
        format!("{} {unit}s", interval.count)
    }
}

/// The byte offset of a `;` that ends the statement while more non-blank text
/// follows — the start of a second statement.
///
/// The parser already refuses leftover tokens, but its message names the token
/// ("unexpected token 'SELECT'"), which reads as a grammar mistake rather than
/// the rule the tool lives by. The scan here is lexical on purpose: the BQL
/// string grammar has no backslash escapes, a double-quoted string ends at the
/// first `"`, and a single-quoted one ends at the first `'` (a doubled `''`
/// continues it), so tracking the one open quote is enough to skip a `;`
/// inside a literal.
fn second_statement_at(source: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    // Non-blank text outside a literal: the statement has begun.
    let mut started = false;
    // A `;` has closed that statement.
    let mut ended = false;
    for (at, character) in source.char_indices() {
        if let Some(open) = quote {
            if character == open {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' if !ended => {
                quote = Some(character);
                started = true;
            }
            ';' if started => ended = true,
            ';' => {}
            _ if ended && !character.is_whitespace() => return Some(at),
            _ if !character.is_whitespace() => started = true,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::load_journal;
    use crate::testing::TempDir;
    use rustledger_core::Decimal;
    use std::fs;
    use std::str::FromStr as _;

    /// A small committed-shaped book: three entries in two expense categories
    /// plus a fourth outside the filter, so a `WHERE` and a `GROUP BY` both
    /// have something to do.
    const JOURNAL: &str = "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food
2026-01-01 open Expenses:Coffee

2026-01-02 * \"Coffee Shop\" \"Espresso\"
  Expenses:Coffee       4.50 USD
  Assets:Bank:Checking

2026-01-03 * \"Supermarket\" \"Bread\"
  Expenses:Food        12.25 USD
  Assets:Bank:Checking

2026-01-04 * \"Coffee Shop\" \"Second cup\"
  Expenses:Coffee       3.25 USD
  Assets:Bank:Checking
";

    fn ledger() -> Ledger {
        ledger_of(JOURNAL)
    }

    /// A ledger loaded from `text`, with the temporary directory dropped
    /// afterwards: the loader keeps the file's text in its source map, so
    /// nothing here needs the directory to outlive the load.
    fn ledger_of(text: &str) -> Ledger {
        let dir = TempDir::new("query");
        let path = dir.file("main.bean");
        fs::write(&path, text).unwrap();
        load_journal(&path).expect("the fixture loads")
    }

    #[test]
    fn group_by_over_the_booked_stream_sums_exactly() {
        let table = run(
            &ledger(),
            "SELECT account, SUM(number) WHERE account ~ \"Expenses:\" GROUP BY account ORDER BY account",
            0,
        )
        .expect("the statement runs");

        assert_eq!(table.columns, vec!["account", "SUM(number)"]);
        assert_eq!(
            table.rows,
            vec![
                vec!["Expenses:Coffee".to_string(), "7.75".to_string()],
                vec!["Expenses:Food".to_string(), "12.25".to_string()],
            ],
            "grouped sums stay exact decimal text"
        );
        assert!(!table.truncated);
    }

    #[test]
    fn amounts_render_from_the_postings_own_decimals() {
        let table = run(
            &ledger(),
            "SELECT number WHERE account = \"Expenses:Coffee\" ORDER BY date",
            0,
        )
        .expect("the statement runs");

        // Decimal text, not a float's rounding: 4.50 keeps its scale, which
        // `Decimal::to_string` preserves and an `f64` would not.
        assert_eq!(table.rows, vec![vec!["4.50"], vec!["3.25"]]);
        for row in &table.rows {
            let exact = Decimal::from_str(&row[0]).expect("a decimal literal");
            assert_eq!(exact.to_string(), row[0]);
        }
    }

    #[test]
    fn a_date_column_renders_iso() {
        let table =
            run(&ledger(), "SELECT date ORDER BY date LIMIT 1", 0).expect("the statement runs");
        assert_eq!(table.columns, vec!["date"]);
        assert_eq!(table.rows, vec![vec!["2026-01-02"]]);
    }

    #[test]
    fn an_absent_value_is_an_empty_cell() {
        let ledger = ledger_of(
            "\
2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food

2026-01-02 * \"Bread without a payee\"
  Expenses:Food        12.25 USD
  Assets:Bank:Checking
",
        );
        let table = run(&ledger, "SELECT payee", 0).expect("the statement runs");
        assert_eq!(table.columns, vec!["payee"]);
        assert_eq!(table.rows, vec![vec![String::new()], vec![String::new()]]);
    }

    #[test]
    fn garbage_is_refused_by_the_parser() {
        let error = run(&ledger(), "SELECT FROM WHERE", 0).expect_err("garbage is refused");
        assert!(
            error.starts_with("BQL: "),
            "the parser names the failure: {error}"
        );
    }

    #[test]
    fn a_blank_statement_is_refused() {
        assert!(run(&ledger(), "  \n\t", 0).is_err());
    }

    #[test]
    fn a_second_statement_is_refused() {
        let error = run(&ledger(), "SELECT date; SELECT date", 0)
            .expect_err("a second statement is refused");
        assert!(
            error.contains("one statement at a time"),
            "the refusal names the rule: {error}"
        );
    }

    #[test]
    fn a_trailing_semicolon_is_fine() {
        let table =
            run(&ledger(), "SELECT date LIMIT 1;", 0).expect("a terminator is not a statement");
        assert_eq!(table.rows, vec![vec!["2026-01-02"]]);
    }

    #[test]
    fn a_semicolon_inside_a_literal_is_not_a_statement_boundary() {
        let table = run(&ledger(), "SELECT \"a;b\" AS text LIMIT 1", 0)
            .expect("a `;` inside a literal is just text");
        assert_eq!(table.rows, vec![vec!["a;b"]]);
    }

    #[test]
    fn rows_are_capped_and_the_cut_is_reported() {
        let ledger = ledger();
        let whole = run(&ledger, "SELECT date, account", 0).expect("uncapped");
        assert_eq!(whole.rows.len(), 6, "three entries, two postings each");
        assert!(!whole.truncated);

        let capped = run(&ledger, "SELECT date, account", 2).expect("capped");
        assert_eq!(capped.rows, whole.rows[..2], "the first rows, in order");
        assert!(capped.truncated);

        let generous = run(&ledger, "SELECT date, account", 6).expect("exactly enough");
        assert_eq!(generous.rows, whole.rows);
        assert!(!generous.truncated, "a table that fits is not truncated");
    }

    #[test]
    fn the_scan_ignores_quoted_semicolons() {
        assert_eq!(second_statement_at("SELECT date; SELECT date"), Some(13));
        assert_eq!(second_statement_at("SELECT date;"), None);
        assert_eq!(second_statement_at("SELECT date;  \n"), None);
        assert_eq!(second_statement_at("SELECT date;;"), None);
        assert_eq!(
            second_statement_at("; SELECT date"),
            None,
            "a leading `;` is the parser's problem"
        );
        assert_eq!(second_statement_at("SELECT \"a;b\""), None);
        assert_eq!(second_statement_at("SELECT 'a;b'"), None);
        assert_eq!(second_statement_at("SELECT 'it''s; fine'"), None);
        assert_eq!(second_statement_at("SELECT date; \"second\""), Some(13));
    }
}
