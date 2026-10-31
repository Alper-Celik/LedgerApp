// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! One commodity's price history: the days the journal says what it was worth,
//! and where each statement came from.
//!
//! Two sources are read, and the reader chooses between them:
//!
//! - **`explicit`** — `price` directives for the commodity. A day stated more
//!   than once keeps the last directive in journal order, which is what
//!   beancount's own price map holds (and what a reader of the file would take).
//! - **`transactions`** — the rate implied by postings that carry a cost
//!   (`10 AAPL {150 USD}` implies 150 USD per AAPL that day) or a price
//!   annotation (`10 AAPL @ 150 USD`, `10 AAPL @@ 1500 USD`). Only complete
//!   amounts can imply a rate: a posting with an incomplete amount, or a
//!   cost/price in the units' own commodity, states no exchange rate and is
//!   ignored. The envelope plugin's ledger entries and every
//!   synthesised posting are ignored too — they are bookkeeping, not trades.
//!
//! `both` merges the two. There is one point per date and currency; when a day
//! has both an explicit price and a transaction-implied one, the explicit price
//! wins and the point says `explicit`, because it is the reader's own
//! statement. Two trades the same day at different costs collapse the same way
//! the two price directives do: the last statement in journal order wins.
//!
//! The series is daily, not monthly: a price is a fact about a day. Dates are
//! `YYYY-MM-DD` and amounts are exact decimal strings throughout — the
//! frontend never parses a price into a float, and neither does this module.
//!
//! A monthly index-derived price (the index-as-price path writes ordinary
//! `YYYY-MM-01 price COMMODITY VALUE CURRENCY` lines) is therefore an ordinary
//! explicit point, with nothing special about it here.

use std::collections::{BTreeMap, BTreeSet};

use rust_decimal::Decimal;
use rustledger_core::{
    Amount, CostNumber, Directive, IncompleteAmount, Posting, PriceAnnotation, PriceKind,
    SYNTHESIZED_FILE_ID,
};
use rustledger_loader::Ledger;

use crate::session::AppState;

/// The account namespace the envelope plugin writes its bookkeeping into. Every
/// posting there is the plugin's own, so no posting there states a trade.
const ENVELOPE_NAMESPACE: &str = "Equity:Envelope";

/// The point came from a `price` directive.
const EXPLICIT: &str = "explicit";
/// The point was implied by a posting's cost or price annotation.
const TRANSACTIONS: &str = "transactions";

/// One day's price for one commodity, in one currency, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct PriceHistoryPoint {
    /// The day, `YYYY-MM-DD`.
    pub date: String,
    /// The exact price as a decimal string.
    pub value: String,
    /// The currency the price is in.
    pub currency: String,
    /// `"explicit"` for a `price` directive, `"transactions"` for a rate
    /// implied by a posting.
    pub source: String,
}

/// The figures a reader wants before scrolling: how many days, when they run,
/// and the range of prices.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct PriceHistorySummary {
    /// How many points the answer holds.
    pub points: u32,
    /// The oldest point's day, `None` for an empty history.
    pub first: Option<String>,
    /// The newest point's day, `None` for an empty history.
    pub last: Option<String>,
    /// The smallest price across the returned points, an exact decimal string.
    /// `None` for an empty history. When the answer holds several currencies
    /// the figure is the smallest decimal among them, which is a range of
    /// numbers rather than of money; ask for one currency to compare values.
    pub min: Option<String>,
    /// The largest price across the returned points; see [`Self::min`].
    pub max: Option<String>,
}

/// How many points each source holds for a commodity before the requested
/// range and point limit narrow them. The screen uses this to say *why* an
/// answer is empty: nothing in the ledger, nothing in the chosen source, or
/// nothing in the chosen months.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct PriceHistoryCounts {
    /// Points stated by `price` directives.
    pub explicit: u32,
    /// Points implied by trades.
    pub transactions: u32,
}

/// One commodity's price history as the screen reads it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct PriceHistory {
    /// The commodity the history is for.
    pub commodity: String,
    /// The source selector the answer was built for: `explicit`,
    /// `transactions` or `both`.
    pub source: String,
    /// Every currency the commodity has ever been priced in, across both
    /// sources, sorted. This is the currency picker's list, and it does not
    /// move when the source changes — an option that disappears under another
    /// source would be a control that changes under the hand.
    pub currencies: Vec<String>,
    /// What each source holds for the commodity and the requested currency,
    /// before `from`/`to` and `limit` apply — the empty state's evidence.
    pub counts: PriceHistoryCounts,
    /// The points, oldest first; a day with several currencies has one point
    /// per currency, ordered by currency.
    pub points: Vec<PriceHistoryPoint>,
    /// The small summary above the series.
    pub summary: PriceHistorySummary,
}

/// A commodity's history, from the currently loaded ledger.
///
/// See the module docs for the sources, the merge rule and the format. `from`
/// and `to` are inclusive `YYYY-MM` month labels; `limit`, when given, keeps
/// only the newest that many points after the month and currency filters.
/// Every figure is an exact decimal string.
pub fn history(
    state: &AppState,
    commodity: &str,
    currency: Option<&str>,
    source: &str,
    from: Option<&str>,
    to: Option<&str>,
    limit: Option<u32>,
) -> Result<PriceHistory, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger loaded")?;
    history_of(&loaded.ledger, commodity, currency, source, from, to, limit)
}

/// [`history`] over a ledger already in hand — the seam the tests drive.
pub(crate) fn history_of(
    ledger: &Ledger,
    commodity: &str,
    currency: Option<&str>,
    source: &str,
    from: Option<&str>,
    to: Option<&str>,
    limit: Option<u32>,
) -> Result<PriceHistory, String> {
    let commodity = commodity.trim();
    if commodity.is_empty() {
        return Err("price history needs a commodity".to_string());
    }
    if !matches!(source, EXPLICIT | TRANSACTIONS | "both") {
        return Err(format!(
            "unknown price history source `{source}`; use `explicit`, `transactions` or `both`"
        ));
    }
    let from = from.map(|month| checked_month("from", month)).transpose()?;
    let to = to.map(|month| checked_month("to", month)).transpose()?;
    if let (Some(from), Some(to)) = (&from, &to)
        && from > to
    {
        return Err(format!("from `{from}` is after to `{to}`"));
    }
    let currency = currency.filter(|code| !code.is_empty());

    let collected = collect(ledger, commodity);
    let counts = PriceHistoryCounts {
        explicit: count_of(&collected.explicit, currency),
        transactions: count_of(&collected.transactions, currency),
    };

    // One row per date and currency. `BTreeMap` keeps the answer ordered by
    // (date, currency), and the later insertion wins — which is exactly the
    // tie-break both sources are documented to use.
    let mut merged: BTreeMap<(String, String), (Decimal, &'static str)> = BTreeMap::new();
    if matches!(source, TRANSACTIONS | "both") {
        for (key, value) in &collected.transactions {
            merged.insert(key.clone(), (*value, TRANSACTIONS));
        }
    }
    if matches!(source, EXPLICIT | "both") {
        // Inserted second: the reader's explicit statement wins over a rate
        // inferred from a trade on the same day.
        for (key, value) in &collected.explicit {
            merged.insert(key.clone(), (*value, EXPLICIT));
        }
    }

    let mut rows: Vec<((String, String), Decimal, &'static str)> = merged
        .into_iter()
        .filter(|((date, code), _)| {
            if let Some(currency) = currency
                && code != currency
            {
                return false;
            }
            let month = date.get(..7).unwrap_or(date);
            if let Some(from) = from.as_deref()
                && month < from
            {
                return false;
            }
            if let Some(to) = to.as_deref()
                && month > to
            {
                return false;
            }
            true
        })
        .map(|(key, (value, source))| (key, value, source))
        .collect();

    if let Some(limit) = limit {
        let keep = usize::try_from(limit).unwrap_or(usize::MAX);
        if rows.len() > keep {
            rows.drain(..rows.len() - keep);
        }
    }

    let summary = PriceHistorySummary {
        points: u32::try_from(rows.len()).unwrap_or(u32::MAX),
        first: rows.first().map(|((date, _), _, _)| date.clone()),
        last: rows.last().map(|((date, _), _, _)| date.clone()),
        min: rows.iter().map(|(_, value, _)| *value).min().map(|value| value.to_string()),
        max: rows.iter().map(|(_, value, _)| *value).max().map(|value| value.to_string()),
    };

    let points = rows
        .into_iter()
        .map(|((date, code), value, source)| PriceHistoryPoint {
            date,
            value: value.to_string(),
            currency: code,
            source: source.to_string(),
        })
        .collect();

    Ok(PriceHistory {
        commodity: commodity.to_string(),
        source: source.to_string(),
        currencies: collected.currencies.into_iter().collect(),
        counts,
        points,
        summary,
    })
}

/// How many points a source holds for the requested currency, before the month
/// range and point limit narrow the answer.
fn count_of(points: &BTreeMap<(String, String), Decimal>, currency: Option<&str>) -> u32 {
    let count = points
        .keys()
        .filter(|(_, code)| currency.is_none_or(|wanted| code == wanted))
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// What the ledger says about one commodity, split by source.
#[derive(Default)]
struct Collected {
    /// `(date, currency) → price`, the last statement in journal order winning.
    explicit: BTreeMap<(String, String), Decimal>,
    /// The same, for rates implied by postings.
    transactions: BTreeMap<(String, String), Decimal>,
    /// Every currency either source prices the commodity in.
    currencies: BTreeSet<String>,
}

/// One pass over the ledger, filling both sources.
fn collect(ledger: &Ledger, commodity: &str) -> Collected {
    let mut collected = Collected::default();

    for spanned in &ledger.directives {
        match &spanned.value {
            Directive::Price(price) => {
                if price.currency.as_str() != commodity {
                    continue;
                }
                let currency = price.amount.currency.to_string();
                collected
                    .currencies
                    .insert(currency.clone());
                collected.explicit.insert(
                    (price.date.to_string(), currency),
                    price.amount.number,
                );
            }
            Directive::Transaction(transaction) => {
                for posting in &transaction.postings {
                    // A synthesised posting has no source text: a plugin built
                    // it, so it is not a trade the reader wrote.
                    if posting.file_id == SYNTHESIZED_FILE_ID {
                        continue;
                    }
                    let account = posting.value.account.as_str();
                    if is_envelope_account(account) {
                        continue;
                    }
                    let Some(IncompleteAmount::Complete(units)) = &posting.value.units else {
                        // Both sides of the rate are needed; an interpolated
                        // amount was never stated, so it cannot imply a price.
                        continue;
                    };
                    // Only a posting *of this commodity* states its rate. A
                    // trade of something else the same day says nothing about
                    // this one, and letting it through would overwrite the day's
                    // answer with another commodity's price.
                    if units.currency.as_str() != commodity {
                        continue;
                    }
                    let Some((value, currency)) = implied_price(&posting.value, units) else {
                        continue;
                    };
                    collected
                        .currencies
                        .insert(currency.clone());
                    collected.transactions.insert(
                        (transaction.date.to_string(), currency),
                        value,
                    );
                }
            }
            _ => {}
        }
    }

    collected
}

/// The rate one posting implies, as `(price, currency)`.
///
/// A price annotation (`@`/`@@`) is the posting's own exchange rate, so it is
/// read first; a `{cost}` states what the position was acquired for and is the
/// fallback. Either side must be complete, positive, and in a currency other
/// than the units' own — a rate of `AAPL` in `AAPL` says nothing about what an
/// `AAPL` is worth.
fn implied_price(posting: &Posting, units: &Amount) -> Option<(Decimal, String)> {
    if let Some(price) = &posting.price
        && let Some((value, currency)) = price_value(price, units.number)
        && currency != units.currency.as_str()
    {
        return Some((value, currency));
    }
    if let Some(cost) = posting.cost.as_deref() {
        let currency = cost.currency.as_ref()?.to_string();
        if currency == units.currency.as_str() {
            return None;
        }
        let number = cost_number(cost.number.as_ref()?, units.number.abs())?;
        if number > Decimal::ZERO {
            return Some((number, currency));
        }
    }
    None
}

/// A posting's `@`/`@@` annotation as a per-unit price, when it is complete and
/// positive. `@@` states the total for the posting, so it is divided by the
/// units' magnitude; a zero posting has no per-unit rate.
fn price_value(price: &PriceAnnotation, units: Decimal) -> Option<(Decimal, String)> {
    let Some(IncompleteAmount::Complete(amount)) = &price.amount else {
        return None;
    };
    let value = match price.kind {
        PriceKind::Unit => amount.number,
        PriceKind::Total => {
            if units.is_zero() {
                return None;
            }
            amount.number / units.abs()
        }
    };
    (value > Decimal::ZERO).then(|| (value, amount.currency.to_string()))
}

/// A cost spec's number as a per-unit figure. `{{ total }}` and the compound
/// form are stated as totals, so they are divided by the units' magnitude —
/// which is what booking has already done in [`CostNumber::PerUnitFromTotal`].
fn cost_number(number: &CostNumber, units: Decimal) -> Option<Decimal> {
    match number {
        CostNumber::PerUnit { value } => Some(*value),
        CostNumber::Total { value } => {
            if units.is_zero() {
                return None;
            }
            Some(*value / units)
        }
        CostNumber::PerUnitFromTotal(booked) => Some(booked.per_unit),
        CostNumber::Compound { per_unit, total } => {
            if units.is_zero() {
                return None;
            }
            Some(*per_unit + *total / units)
        }
    }
}

/// Whether `account` is inside the envelope plugin's namespace — the plugin's
/// own bookkeeping, never a trade.
fn is_envelope_account(account: &str) -> bool {
    account == ENVELOPE_NAMESPACE
        || (account.len() > ENVELOPE_NAMESPACE.len()
            && account.starts_with(ENVELOPE_NAMESPACE)
            && account.as_bytes()[ENVELOPE_NAMESPACE.len()] == b':')
}

/// A `YYYY-MM` month label. The month's existence is checked here because the
/// range is a string comparison over dates; the loader has no say in what the
/// screen asks for.
fn checked_month(label: &str, value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let valid = bytes.len() == 7
        && bytes[4] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..].iter().all(u8::is_ascii_digit)
        && matches!((bytes[5] - b'0') * 10 + (bytes[6] - b'0'), 1..=12);
    if valid {
        Ok(value.to_string())
    } else {
        Err(format!("{label} must be a `YYYY-MM` month, got `{value}`"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::load_journal;
    use crate::testing::TempDir;

    /// The history of `AAPL` from a journal fixture, with no filters unless the
    /// test names them.
    fn history_of_text(text: &str, source: &str) -> PriceHistory {
        let dir = TempDir::new("price-history");
        let path = dir.seed("main.bean", text);
        let ledger = load_journal(&path).unwrap();
        history_of(&ledger, "AAPL", None, source, None, None, None).unwrap()
    }

    fn fixture(body: &str) -> String {
        format!(
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Broker
2026-01-01 open Equity:Opening-Balances
2026-01-01 open Equity:Envelope:Food

{body}
"
        )
    }

    fn values(history: &PriceHistory) -> Vec<(&str, &str, &str)> {
        history
            .points
            .iter()
            .map(|point| (point.date.as_str(), point.value.as_str(), point.source.as_str()))
            .collect()
    }

    #[test]
    fn explicit_prices_are_daily_and_the_last_statement_of_a_day_wins() {
        let history = history_of_text(
            &fixture(
                "\
2026-01-05 price AAPL 150.00 USD
2026-01-05 price AAPL 151.00 USD
2026-01-06 price AAPL 152.50 USD
",
            ),
            "explicit",
        );

        assert_eq!(
            values(&history),
            vec![
                ("2026-01-05", "151.00", "explicit"),
                ("2026-01-06", "152.50", "explicit"),
            ]
        );
        assert_eq!(history.currencies, vec!["USD"]);
        assert_eq!(history.summary.points, 2);
        assert_eq!(history.summary.first.as_deref(), Some("2026-01-05"));
        assert_eq!(history.summary.last.as_deref(), Some("2026-01-06"));
        assert_eq!(history.summary.min.as_deref(), Some("151.00"));
        assert_eq!(history.summary.max.as_deref(), Some("152.50"));
    }

    #[test]
    fn trades_imply_the_cost_and_the_price_annotation_at_their_rates() {
        let history = history_of_text(
            &fixture(
                "\
2026-01-07 * \"buy at cost\"
  Assets:Broker              10 AAPL {150.00 USD}
  Equity:Opening-Balances

2026-01-08 * \"buy with a unit price\"
  Assets:Broker              10 AAPL @ 175.00 USD
  Equity:Opening-Balances

2026-01-09 * \"buy with a total price\"
  Assets:Broker              10 AAPL @@ 1800.00 USD
  Equity:Opening-Balances
",
            ),
            "transactions",
        );

        assert_eq!(
            values(&history),
            vec![
                ("2026-01-07", "150.00", "transactions"),
                ("2026-01-08", "175.00", "transactions"),
                ("2026-01-09", "180.00", "transactions"),
            ]
        );
    }

    #[test]
    fn two_trades_on_one_day_keep_the_last_statement_in_journal_order() {
        // The decision the ticket leaves open: a day has one price per
        // currency, and a later trade supersedes an earlier one, exactly as a
        // later `price` directive does. A chart should not draw two values for
        // the same day.
        let history = history_of_text(
            &fixture(
                "\
2026-01-07 * \"morning\"
  Assets:Broker              10 AAPL {150.00 USD}
  Equity:Opening-Balances

2026-01-07 * \"afternoon\"
  Assets:Broker              10 AAPL {160.00 USD}
  Equity:Opening-Balances
",
            ),
            "transactions",
        );

        assert_eq!(values(&history), vec![("2026-01-07", "160.00", "transactions")]);
    }

    #[test]
    fn both_prefers_the_explicit_price_on_a_shared_day() {
        let history = history_of_text(
            &fixture(
                "\
2026-01-07 price AAPL 152.00 USD

2026-01-07 * \"buy\"
  Assets:Broker              10 AAPL {150.00 USD}
  Equity:Opening-Balances

2026-01-08 * \"buy\"
  Assets:Broker              10 AAPL {155.00 USD}
  Equity:Opening-Balances
",
            ),
            "both",
        );

        assert_eq!(
            values(&history),
            vec![
                ("2026-01-07", "152.00", "explicit"),
                ("2026-01-08", "155.00", "transactions"),
            ]
        );
        assert_eq!(history.counts.explicit, 1);
        assert_eq!(history.counts.transactions, 2);
    }

    #[test]
    fn a_trade_in_another_commodity_is_not_this_commoditys_price() {
        // Both trades are in dollars on the same day. Only the AAPL one is a
        // statement about AAPL; a shared day must not let the MSFT rate
        // overwrite it.
        let history = history_of_text(
            &fixture(
                "\
2026-01-07 * \"buy AAPL\"
  Assets:Broker              10 AAPL {150.00 USD}
  Equity:Opening-Balances

2026-01-07 * \"buy MSFT\"
  Assets:Broker              10 MSFT {300.00 USD}
  Equity:Opening-Balances
",
            ),
            "transactions",
        );

        assert_eq!(values(&history), vec![("2026-01-07", "150.00", "transactions")]);
    }

    #[test]
    fn a_posting_with_an_incomplete_amount_implies_nothing() {
        let history = history_of_text(
            &fixture(
                "\
2026-01-07 * \"currency only\"
  Assets:Broker              AAPL
  Equity:Opening-Balances

2026-01-08 * \"number only\"
  Assets:Broker              10
  Equity:Opening-Balances
",
            ),
            "transactions",
        );

        assert!(history.points.is_empty(), "{:?}", history.points);
    }

    #[test]
    fn a_cost_in_the_units_own_commodity_is_not_a_price() {
        let history = history_of_text(
            &fixture(
                "\
2026-01-07 * \"no exchange rate\"
  Assets:Broker              10 AAPL {150.00 AAPL}
  Equity:Opening-Balances
",
            ),
            "transactions",
        );

        assert!(history.points.is_empty(), "{:?}", history.points);
    }

    #[test]
    fn a_price_directive_without_a_currency_is_a_loader_error_not_a_point() {
        let dir = TempDir::new("price-history-bad-price");
        let path = dir.seed(
            "main.bean",
            "2026-01-07 price AAPL 150.00\n",
        );
        let ledger = load_journal(&path).unwrap();
        assert!(
            !ledger.errors.is_empty(),
            "the malformed directive should be reported"
        );

        let history =
            history_of(&ledger, "AAPL", None, "explicit", None, None, None).unwrap();
        assert!(history.points.is_empty(), "{:?}", history.points);
    }

    #[test]
    fn a_commodity_the_ledger_never_prices_has_an_empty_history() {
        let history = history_of_text(&fixture(""), "both");

        assert!(history.points.is_empty());
        assert!(history.currencies.is_empty());
        assert_eq!(
            history.summary,
            PriceHistorySummary {
                points: 0,
                first: None,
                last: None,
                min: None,
                max: None,
            }
        );
    }

    #[test]
    fn from_and_to_are_inclusive_months() {
        let text = fixture(
            "\
2026-01-15 price AAPL 150.00 USD
2026-02-15 price AAPL 155.00 USD
2026-03-15 price AAPL 160.00 USD
",
        );
        let dir = TempDir::new("price-history-range");
        let path = dir.seed("main.bean", &text);
        let ledger = load_journal(&path).unwrap();

        let history = history_of(
            &ledger,
            "AAPL",
            None,
            "explicit",
            Some("2026-02"),
            Some("2026-02"),
            None,
        )
        .unwrap();
        assert_eq!(values(&history), vec![("2026-02-15", "155.00", "explicit")]);

        // The month labels bound the whole month, and a bound outside the
        // ledger's own span is not an error.
        let history = history_of(
            &ledger,
            "AAPL",
            None,
            "explicit",
            Some("2025-12"),
            Some("2026-12"),
            None,
        )
        .unwrap();
        assert_eq!(history.summary.points, 3);
    }

    #[test]
    fn limit_keeps_the_newest_points_and_the_currency_filter_narrows_both_sources() {
        let text = fixture(
            "\
2026-01-15 price AAPL 150.00 USD
2026-02-15 price AAPL 155.00 USD
2026-03-15 price AAPL 160.00 USD
2026-03-15 price AAPL 5000.00 TRY
",
        );
        let dir = TempDir::new("price-history-limit");
        let path = dir.seed("main.bean", &text);
        let ledger = load_journal(&path).unwrap();

        let history = history_of(
            &ledger,
            "AAPL",
            None,
            "explicit",
            None,
            None,
            Some(2),
        )
        .unwrap();
        assert_eq!(
            history.summary.first.as_deref(),
            Some("2026-03-15"),
            "two points are kept, oldest first"
        );
        assert_eq!(history.points.len(), 2);

        // The currency picker keeps every currency across both sources even
        // when the selected source has none of them.
        assert_eq!(history.currencies, vec!["TRY", "USD"]);
        let history = history_of(
            &ledger,
            "AAPL",
            Some("TRY"),
            "explicit",
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(values(&history), vec![("2026-03-15", "5000.00", "explicit")]);
    }

    #[test]
    fn an_index_derived_monthly_price_is_an_ordinary_explicit_point() {
        // The index-as-price path writes the first day of a month like any
        // other `price` directive; no special case exists for it here.
        let history = history_of_text(
            &fixture("2026-02-01 price AAPL 1234.5678 TRY\n"),
            "explicit",
        );

        assert_eq!(
            values(&history),
            vec![("2026-02-01", "1234.5678", "explicit")]
        );
    }

    #[test]
    fn envelope_and_synthesised_postings_are_not_trades() {
        let history = history_of_text(
            &fixture(
                "\
2026-01-07 * \"envelope bookkeeping\"
  Equity:Envelope:Food       10 AAPL {150.00 USD}
  Equity:Opening-Balances

2026-01-08 * \"a real trade\"
  Assets:Broker              10 AAPL {155.00 USD}
  Equity:Opening-Balances
",
            ),
            "transactions",
        );

        assert_eq!(values(&history), vec![("2026-01-08", "155.00", "transactions")]);
    }

    #[test]
    fn a_bad_source_or_month_is_refused_with_a_sentence() {
        let text = fixture("");
        let dir = TempDir::new("price-history-refusal");
        let path = dir.seed("main.bean", &text);
        let ledger = load_journal(&path).unwrap();

        let refused = history_of(&ledger, "AAPL", None, "guess", None, None, None)
            .expect_err("an unknown source was accepted");
        assert!(refused.contains("explicit"), "{refused}");

        let refused =
            history_of(&ledger, "AAPL", None, "both", Some("2026-13"), None, None)
                .expect_err("an impossible month was accepted");
        assert!(refused.contains("YYYY-MM"), "{refused}");
    }
}
