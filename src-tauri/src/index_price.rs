// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Index series as ledger prices: the bridge between the index store (a
//! monthly measurement kept in the app's config directory) and the journal
//! (where a price is something the rest of the app can chart and convert with).
//!
//! An index point *is* a price: the price of one unit of the series' currency
//! in that month. Writing the points as `price` directives is therefore not a
//! conversion — it is the same figure, moved to where the ledger keeps prices.
//! The series' own arithmetic (`index::value_at`) and the ledger's price lookup
//! then agree, because both are step functions over the same monthly points.
//!
//! # The commodity name
//!
//! A series becomes the commodity `IDX.<UPPER-CASED ID>`, e.g. `IDX.TR-CPI` for
//! the series `tr-cpi`. The rule is stable — the id is the identity the store
//! already keys on, so a refresh replaces the same commodity's months rather
//! than inventing a second one — and the `IDX.` prefix is a reserved namespace
//! that keeps an index out of the way of a real commodity: a series imported
//! under the id `usd` becomes `IDX.USD`, never the currency `USD`. Characters a
//! beancount code cannot hold (a colon, say) become `-`; the result is then
//! checked by `prices::checked_commodity`, the same gate every fetched quote
//! passes, so a name the ledger's lexer would split is refused here rather than
//! written.
//!
//! The ticket's other candidate, `IDX:TR-CPI`, is not expressible: `:` is not
//! part of a beancount currency token (it separates metadata and tags), so the
//! colon form could only exist in the UI, not in the journal. The dot is legal
//! and reads the same way.
//!
//! # The date of a price
//!
//! A point is written on **its own date, the first of its month**. The series is
//! a step function whose value for a month covers the whole month
//! ([`IndexSeries::value_at`]), and a price directive dated the first has
//! exactly the same step function: a lookup on any day of the month answers the
//! month's value. Dating it the last day would make the ledger's price lag the
//! index by up to a month, and the chart and the real-terms report would
//! disagree about which month a value belongs to.
//!
//! # Refusals
//!
//! Writing is refused, with nothing written, when the series has no points or no
//! usable currency, or when its commodity would collide with something the
//! ledger already holds or quotes. The currency is the series' own, not the
//! ledger's: a `price` directive may state any currency, and a dollar index
//! written into a lira ledger is a true fact about the index. What *uses* the
//! series, the real-terms report, is where a currency that is not the ledger's
//! money is refused — it needs a rate the ledger does not have.
//!
//! The command takes the series *id* and reads the store, so a series fetched a
//! moment ago — stored but not in any frontend copy — is written from the
//! store's own bytes.
//!
//! A second run writes nothing: `prices::write_to_file` skips a quote the ledger
//! already states verbatim, and the result says how many were already there.

use std::collections::HashSet;
use std::path::Path;

use rustledger_core::{Directive, IncompleteAmount};
use rustledger_loader::Ledger;
use serde::Serialize;
use specta::Type;

use crate::index::IndexSeries;
use crate::index_store::load_indexes;
use crate::prices::{checked_commodity, write_to_file, PriceQuote};
use crate::session::AppState;

/// The reserved prefix every index commodity carries. See the module docs.
const PREFIX: &str = "IDX.";

/// What one write did: the commodity it named, the file it landed in, and how
/// many of the series' points were new.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct IndexPriceWrite {
    /// The series id the points came from.
    pub series: String,
    /// The ledger commodity the points were written as, e.g. `IDX.TR-CPI`.
    pub commodity: String,
    /// The currency the prices are in.
    pub currency: String,
    /// The journal file the directives went into.
    pub file: String,
    /// How many `price` directives this run appended.
    pub written: u32,
    /// How many points the ledger already carried verbatim.
    pub skipped: u32,
}

/// The ledger commodity a series id becomes: `IDX.` plus the id, upper-cased,
/// with every character a beancount code cannot hold replaced by `-`.
///
/// The result is checked by [`checked_commodity`], so it is exactly as legal as
/// a fetched ticker: at most 24 bytes, opening and closing alphanumeric.
pub fn commodity_for(series_id: &str) -> Result<String, String> {
    let id = series_id.trim();
    if id.is_empty() {
        return Err("an index series needs a non-empty id".to_owned());
    }
    let mut code = String::with_capacity(PREFIX.len() + id.len());
    code.push_str(PREFIX);
    for character in id.chars() {
        let character = character.to_ascii_uppercase();
        if character.is_ascii_alphanumeric() || matches!(character, '\'' | '.' | '_' | '-') {
            code.push(character);
        } else {
            code.push('-');
        }
    }
    checked_commodity("commodity", &code)
        .map_err(|error| format!("index `{id}` cannot be written as a commodity: {error}"))
}

/// The quotes one series contributes: one per point, in the point's own order,
/// on the point's own date (see the module docs).
///
/// Refused when the series has no points or no usable currency — the two states
/// a hand-edited `indexes.json` can hold that the writer must not paper over.
pub fn quotes_for(series: &IndexSeries) -> Result<Vec<PriceQuote>, String> {
    if series.points.is_empty() {
        return Err(format!(
            "index `{}` has no points, so there is no price to write",
            series.id
        ));
    }
    let commodity = commodity_for(&series.id)?;
    let currency = checked_commodity("currency", &series.currency).map_err(|error| {
        format!(
            "index `{}` has no usable currency (`{}`): {error}",
            series.id, series.currency
        )
    })?;
    Ok(series
        .points
        .iter()
        .map(|point| PriceQuote {
            commodity: commodity.clone(),
            currency: currency.clone(),
            date: point.date.clone(),
            price: point.value.clone(),
            source: format!("index:{}", series.id),
        })
        .collect())
}

/// Every commodity the ledger uses as money or holds as a thing: postings,
/// account declarations, commodity declarations, balance assertions, and the
/// quote side of a price directive.
///
/// The *subject* of a price directive is deliberately excluded: after the first
/// write the index commodity appears there and nowhere else, which is what makes
/// a second run idempotent instead of a collision.
fn used_commodities(ledger: &Ledger) -> HashSet<String> {
    let mut used = HashSet::new();
    for spanned in &ledger.directives {
        match &spanned.value {
            Directive::Transaction(transaction) => {
                for posting in &transaction.postings {
                    match &posting.value.units {
                        Some(IncompleteAmount::Complete(amount)) => {
                            used.insert(amount.currency.to_string());
                        }
                        Some(IncompleteAmount::CurrencyOnly(currency)) => {
                            used.insert(currency.to_string());
                        }
                        Some(IncompleteAmount::NumberOnly(_)) | None => {}
                    }
                }
            }
            Directive::Open(open) => {
                used.extend(open.currencies.iter().map(ToString::to_string));
            }
            Directive::Commodity(commodity) => {
                used.insert(commodity.currency.to_string());
            }
            Directive::Balance(balance) => {
                used.insert(balance.amount.currency.to_string());
            }
            Directive::Price(price) => {
                used.insert(price.amount.currency.to_string());
            }
            _ => {}
        }
    }
    used
}

/// Write `series` into `file` (the loaded ledger's root journal when `None` or
/// empty) as one `price` directive per point.
///
/// The whole refusal and write path is here so it can be driven without a
/// running app; [`write_stored`] is the store-reading wrapper the command and
/// the CLI use.
pub fn write_series(
    state: &AppState,
    series: &IndexSeries,
    file: Option<&str>,
) -> Result<IndexPriceWrite, String> {
    let quotes = quotes_for(series)?;
    let commodity = quotes[0].commodity.clone();
    let currency = quotes[0].currency.clone();
    let (root, used) = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().ok_or("no ledger is open")?;
        (
            loaded.root.display().to_string(),
            used_commodities(&loaded.ledger),
        )
    };
    if used.contains(&commodity) {
        return Err(format!(
            "`{commodity}` is already a commodity this ledger holds or quotes, so the prices \
             of index `{}` would be mixed with it; nothing was written",
            series.id
        ));
    }
    let file = file
        .map(str::trim)
        .filter(|file| !file.is_empty())
        .unwrap_or(root.as_str())
        .to_owned();
    let written = write_to_file(state, &file, &quotes)?;
    let written = u32::try_from(written).unwrap_or(u32::MAX);
    let total = u32::try_from(quotes.len()).unwrap_or(u32::MAX);
    Ok(IndexPriceWrite {
        series: series.id.clone(),
        commodity,
        currency,
        file,
        written,
        skipped: total.saturating_sub(written),
    })
}

/// Write the series stored under `id` in `dir` — the app's index directory — as
/// prices in the loaded ledger's root journal (or `file`).
///
/// The series is read from the store, not from a caller's copy: a series fetched
/// by a provider a moment ago is on disk, and a hand-edited `indexes.json` is
/// the authority on what its points are. A missing id is refused with the ids
/// that are stored.
pub fn write_stored(
    state: &AppState,
    dir: &Path,
    id: &str,
    file: Option<&str>,
) -> Result<IndexPriceWrite, String> {
    let wanted = id.trim();
    let stored = load_indexes(dir)?;
    let Some(series) = stored.iter().find(|series| series.id == wanted) else {
        let names: Vec<&str> = stored.iter().map(|series| series.id.as_str()).collect();
        return Err(format!(
            "no index series is stored as `{wanted}`; stored: {}",
            if names.is_empty() {
                "none".to_owned()
            } else {
                names.join(", ")
            }
        ));
    };
    write_series(state, series, file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::IndexPoint;
    use crate::testing::{TempDir, loaded_state};
    use std::fs;

    fn series(id: &str, currency: &str, points: &[(&str, &str)]) -> IndexSeries {
        IndexSeries::new(
            id,
            "TÜİK CPI",
            "TÜİK",
            false,
            currency,
            points
                .iter()
                .map(|(date, value)| IndexPoint::new(date, value).unwrap())
                .collect(),
        )
        .unwrap()
    }

    fn ledger_with(text: &str) -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new("index-price");
        let path = dir.file("main.bean");
        fs::write(&path, text).unwrap();
        (dir, path)
    }

    const TRY_LEDGER: &str = "\
option \"operating_currency\" \"TRY\"

2026-01-01 open Assets:Bank:TRY TRY
";

    #[test]
    fn the_commodity_is_the_reserved_prefix_plus_the_upper_cased_id() {
        assert_eq!(commodity_for("tr-cpi").unwrap(), "IDX.TR-CPI");
        // A series id a user typed can hold characters a ledger cannot: the
        // colon of a label-shaped id becomes the legal `-`.
        assert_eq!(commodity_for("cpi:try").unwrap(), "IDX.CPI-TRY");
        // The prefix is what keeps an id that spells a currency from becoming
        // that currency.
        assert_eq!(commodity_for("usd").unwrap(), "IDX.USD");
        assert!(commodity_for("").is_err());
        // A code that would end on a separator is refused, not silently
        // rewritten into a different id's commodity.
        assert!(commodity_for("tr-").is_err());
        assert!(commodity_for(&"x".repeat(30)).is_err());
    }

    #[test]
    fn a_single_point_is_one_price() {
        let (_dir, path) = ledger_with(TRY_LEDGER);
        let state = loaded_state(&path);
        let one = series("tr-cpi", "TRY", &[("2025-12-01", "1032.5")]);
        let answer = write_series(&state, &one, None).unwrap();
        assert_eq!(answer.written, 1);
        assert_eq!(answer.skipped, 0);
        assert_eq!(answer.commodity, "IDX.TR-CPI");
        assert_eq!(answer.file, path.display().to_string());
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\n2025-12-01 price IDX.TR-CPI 1032.5 TRY\n"),
            "{text}"
        );
    }

    #[test]
    fn points_are_priced_on_the_first_of_their_month() {
        let (_dir, path) = ledger_with(TRY_LEDGER);
        let state = loaded_state(&path);
        // A publisher's month-end date normalises to the month's first day;
        // the price must carry the same date the index's own lookup uses.
        let one = series(
            "tr-cpi",
            "TRY",
            &[("2025-11-30", "1000.0"), ("2025-12-31", "1032.5")],
        );
        write_series(&state, &one, None).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("2025-11-01 price IDX.TR-CPI 1000.0 TRY"), "{text}");
        assert!(text.contains("2025-12-01 price IDX.TR-CPI 1032.5 TRY"), "{text}");
    }

    #[test]
    fn a_second_run_writes_nothing_and_says_what_was_already_there() {
        let (_dir, path) = ledger_with(TRY_LEDGER);
        let state = loaded_state(&path);
        let one = series(
            "tr-cpi",
            "TRY",
            &[("2025-11-01", "1000.0"), ("2025-12-01", "1032.5")],
        );
        let first = write_series(&state, &one, None).unwrap();
        assert_eq!(first.written, 2);
        let after_first = fs::read_to_string(&path).unwrap();

        let second = write_series(&state, &one, None).unwrap();
        assert_eq!(second.written, 0);
        assert_eq!(second.skipped, 2);
        assert_eq!(fs::read_to_string(&path).unwrap(), after_first);
    }

    #[test]
    fn the_written_commodity_is_visible_to_the_price_targets() {
        let (_dir, path) = ledger_with(TRY_LEDGER);
        let state = loaded_state(&path);
        let one = series("tr-cpi", "TRY", &[("2025-12-01", "1032.5")]);
        write_series(&state, &one, None).unwrap();
        // The write reloads the ledger, so the new commodity is a target the
        // prices screen can chart — the reader's model of an index as a price.
        let targets = crate::prices::targets(&state);
        let row = targets
            .iter()
            .find(|target| target.commodity == "IDX.TR-CPI")
            .expect("the index commodity is a price target");
        let latest = row.latest.as_ref().expect("the point was written");
        assert_eq!(latest.date, "2025-12-01");
        assert_eq!(latest.price, "1032.5");
        assert_eq!(latest.currency, "TRY");
    }

    #[test]
    fn a_series_in_another_currency_is_written_in_its_own() {
        // A dollar index in a lira ledger is a true fact; only the report that
        // restates lira with it needs a rate, and that refusal is the report's.
        let (_dir, path) = ledger_with(TRY_LEDGER);
        let state = loaded_state(&path);
        let us = series("us-cpi", "USD", &[("2025-12-01", "320.5")]);
        let answer = write_series(&state, &us, None).unwrap();
        assert_eq!(answer.written, 1);
        assert_eq!(answer.currency, "USD");
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("2025-12-01 price IDX.US-CPI 320.5 USD"), "{text}");
    }

    #[test]
    fn a_ledger_without_an_operating_currency_still_takes_prices() {
        // The prices state their own currency; no `option` is involved, and a
        // ledger that declares none is no reason to refuse a true directive.
        let (_dir, path) = ledger_with("2026-01-01 open Assets:Bank\n");
        let state = loaded_state(&path);
        let one = series("tr-cpi", "TRY", &[("2025-12-01", "1032.5")]);
        let answer = write_series(&state, &one, None).unwrap();
        assert_eq!(answer.written, 1);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("2025-12-01 price IDX.TR-CPI 1032.5 TRY"), "{text}");
    }

    #[test]
    fn a_code_the_ledger_holds_as_a_commodity_is_refused() {
        let (_dir, path) = ledger_with(
            "option \"operating_currency\" \"TRY\"\n\n\
             2026-01-01 open Assets:Fund:CPI IDX.US-CPI\n",
        );
        let state = loaded_state(&path);
        let before = fs::read_to_string(&path).unwrap();
        let one = series("us-cpi", "TRY", &[("2025-12-01", "320.5")]);
        let error = write_series(&state, &one, None).unwrap_err();
        assert!(error.contains("IDX.US-CPI"), "{error}");
        assert!(error.contains("already a commodity"), "{error}");
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn a_real_commodity_named_like_the_bare_id_does_not_collide() {
        // The prefix's point: a ledger holding the ticker `US-CPI` is not the
        // index, and the index is still writable as `IDX.US-CPI`.
        let (_dir, path) = ledger_with(
            "option \"operating_currency\" \"TRY\"\n\n\
             2026-01-01 open Assets:Broker:US-CPI US-CPI\n\
             2026-01-05 price US-CPI 12.00 TRY\n",
        );
        let state = loaded_state(&path);
        let one = series("us-cpi", "TRY", &[("2025-12-01", "320.5")]);
        let answer = write_series(&state, &one, None).unwrap();
        assert_eq!(answer.written, 1);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("2025-12-01 price IDX.US-CPI 320.5 TRY"), "{text}");
        assert!(text.contains("2026-01-05 price US-CPI 12.00 TRY"), "{text}");
    }

    #[test]
    fn a_series_with_no_points_is_refused() {
        let (_dir, path) = ledger_with(TRY_LEDGER);
        let state = loaded_state(&path);
        let mut empty = series("tr-cpi", "TRY", &[("2025-12-01", "1032.5")]);
        empty.points.clear();
        let error = write_series(&state, &empty, None).unwrap_err();
        assert!(error.contains("no points"), "{error}");
    }

    #[test]
    fn a_series_with_an_empty_currency_is_refused() {
        let (_dir, path) = ledger_with(TRY_LEDGER);
        let state = loaded_state(&path);
        let mut no_currency = series("tr-cpi", "TRY", &[("2025-12-01", "1032.5")]);
        no_currency.currency = "  ".to_owned();
        let error = write_series(&state, &no_currency, None).unwrap_err();
        assert!(error.contains("currency"), "{error}");
    }

    #[test]
    fn a_crlf_ledger_keeps_its_line_endings() {
        let (_dir, path) = ledger_with(
            "option \"operating_currency\" \"TRY\"\r\n\r\n\
             2026-01-01 open Assets:Bank:TRY TRY\r\n",
        );
        let state = loaded_state(&path);
        let one = series("tr-cpi", "TRY", &[("2025-12-01", "1032.5")]);
        write_series(&state, &one, None).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\r\n2025-12-01 price IDX.TR-CPI 1032.5 TRY\r\n"), "{text:?}");
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                assert!(
                    index > 0 && text.as_bytes()[index - 1] == b'\r',
                    "a bare LF at byte {index} in {text:?}"
                );
            }
        }
    }

    #[test]
    fn the_command_path_reads_the_series_from_the_store_by_id() {
        let (dir, path) = ledger_with(TRY_LEDGER);
        let state = loaded_state(&path);
        let indexes = dir.dir().join("indexes");
        let stored = series("tr-cpi", "TRY", &[("2025-12-01", "1032.5")]);
        crate::index_store::save_series(&indexes, &stored).unwrap();

        let answer = write_stored(&state, &indexes, "tr-cpi", None).unwrap();
        assert_eq!(answer.written, 1);

        // A newer revision stored under the same id is what the next run
        // writes: nothing here caches the series the first call read.
        let revised = series(
            "tr-cpi",
            "TRY",
            &[("2025-12-01", "1032.5"), ("2026-01-01", "1071.9")],
        );
        crate::index_store::save_series(&indexes, &revised).unwrap();
        let again = write_stored(&state, &indexes, "tr-cpi", None).unwrap();
        assert_eq!(again.written, 1);
        assert_eq!(again.skipped, 1);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("2026-01-01 price IDX.TR-CPI 1071.9 TRY"), "{text}");
    }

    #[test]
    fn an_unknown_series_id_names_what_is_stored() {
        let (dir, path) = ledger_with(TRY_LEDGER);
        let state = loaded_state(&path);
        let indexes = dir.dir().join("indexes");
        crate::index_store::save_series(
            &indexes,
            &series("tr-cpi", "TRY", &[("2025-12-01", "1032.5")]),
        )
        .unwrap();
        let error = write_stored(&state, &indexes, "us-cpi", None).unwrap_err();
        assert!(error.contains("us-cpi"), "{error}");
        assert!(error.contains("tr-cpi"), "{error}");
    }
}
