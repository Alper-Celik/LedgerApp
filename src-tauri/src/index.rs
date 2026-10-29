// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Price indexes: a monthly series of index points, and the one definition of
//! how a nominal amount is restated in another month's money.
//!
//! A point's `value` is the price level the publisher reports (a CPI or HICP
//! index, e.g. `1982-84 = 100` or `2015 = 100`), as an exact decimal string.
//! Values are deliberately *not* parsed to `f64`: every conversion here is
//! [`rust_decimal`] arithmetic, so the payload the frontend renders is the
//! payload the ledger's own amounts are made of.
//!
//! The series is a step function: the value in force on a date is the last
//! point at or before it, and a month's point therefore covers the whole month.
//! Before the first point there is no value to convert with.

use rust_decimal::{Decimal, RoundingStrategy};

/// The only frequency a series carries. Monthly points are what every provider
/// publishes and what every report reads; the string is kept in the payload so
/// the frontend sees the unit it is charting.
pub const MONTHLY: &str = "monthly";

/// How many fractional places a real amount keeps when the division does not
/// terminate. The ledger's own amounts carry at most a handful of places, and
/// eight is far beyond any currency's minor unit, so this only trims the tail
/// of a repeating quotient.
const REAL_SCALE: u32 = 8;

/// One published index point, dated the first of its month.
///
/// `date` is `YYYY-MM-DD` and `value` a positive decimal string. The pair is
/// stored as the publisher gave it (after normalising the date and the decimal
/// spelling), so two loads produce the same JSON.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct IndexPoint {
    /// The first day of the month the point belongs to, `YYYY-MM-DD`.
    pub date: String,
    /// The price level at that month, as an exact decimal string.
    pub value: String,
}

impl IndexPoint {
    /// A point from a publisher's `date,value` pair. `date` may be `YYYY-MM` or
    /// `YYYY-MM-DD`; both are normalised to the first of the month, because the
    /// series is monthly. The value must parse as a positive decimal.
    pub fn new(date: &str, value: &str) -> Result<Self, String> {
        let date = first_of_month(date)?;
        let value: Decimal = value
            .trim()
            .parse()
            .map_err(|_| format!("`{value}` is not a decimal index value"))?;
        if value <= Decimal::ZERO {
            return Err(format!(
                "the index value at {date} must be positive, got {value}"
            ));
        }
        Ok(Self {
            date,
            value: value.to_string(),
        })
    }
}

/// A complete monthly index series, as the store keeps it and the frontend
/// receives it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct IndexSeries {
    /// Stable identity, e.g. `us-cpi`; a ledger holds one series per id.
    pub id: String,
    /// Human label, e.g. `US CPI (FRED, CPIAUCSL)`.
    pub label: String,
    /// The publisher, e.g. `FRED` or `TÜİK`.
    pub source: String,
    /// True for a series a non-official body publishes (ENAG in Türkiye): the
    /// report says so rather than presenting it as an official statistic.
    pub unofficial: bool,
    /// [`MONTHLY`]; kept in the payload for the frontend's axis label.
    #[serde(default = "monthly")]
    pub frequency: String,
    /// ISO code of the money the index measures, e.g. `TRY`.
    pub currency: String,
    /// The points, oldest first and one per month after [`Self::normalised`].
    pub points: Vec<IndexPoint>,
}

fn monthly() -> String {
    MONTHLY.to_owned()
}

impl IndexSeries {
    /// A series from freshly parsed points. The result is normalised: dates are
    /// the first of their month, points are date-sorted with a repeated month's
    /// last point winning, and every value is a positive decimal.
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        source: impl Into<String>,
        unofficial: bool,
        currency: impl Into<String>,
        points: Vec<IndexPoint>,
    ) -> Result<Self, String> {
        Self {
            id: id.into(),
            label: label.into(),
            source: source.into(),
            unofficial,
            frequency: MONTHLY.to_owned(),
            currency: currency.into(),
            points,
        }
        .normalised()
    }

    /// The canonical form of a series that came from JSON or a provider: trim
    /// the labels, require the monthly frequency, validate every point and put
    /// the points in the one order the lookup relies on.
    ///
    /// Deserialisation cannot enforce any of that — an installed `indexes.json`
    /// is user-editable — so this is the single gate every loaded series passes
    /// through.
    pub fn normalised(mut self) -> Result<Self, String> {
        self.id = self.id.trim().to_owned();
        self.label = self.label.trim().to_owned();
        self.source = self.source.trim().to_owned();
        self.currency = self.currency.trim().to_owned();
        if self.id.is_empty() {
            return Err("an index series needs a non-empty id".to_owned());
        }
        if self.label.is_empty() {
            return Err(format!("index `{}` has no label", self.id));
        }
        if self.source.is_empty() {
            return Err(format!("index `{}` has no source", self.id));
        }
        if self.currency.is_empty() {
            return Err(format!("index `{}` has no currency", self.id));
        }
        if self.frequency.is_empty() {
            self.frequency = MONTHLY.to_owned();
        }
        if self.frequency != MONTHLY {
            return Err(format!(
                "index `{}` is `{}`; only monthly series are supported",
                self.id, self.frequency
            ));
        }
        if self.points.is_empty() {
            return Err(format!("index `{}` has no points", self.id));
        }
        let mut points = Vec::with_capacity(self.points.len());
        for point in std::mem::take(&mut self.points) {
            let value: Decimal = point.value.trim().parse().map_err(|_| {
                format!(
                    "index `{}`: `{}` at {} is not a decimal value",
                    self.id, point.value, point.date
                )
            })?;
            if value <= Decimal::ZERO {
                return Err(format!(
                    "index `{}`: the value at {} must be positive, got {value}",
                    self.id, point.date
                ));
            }
            let date =
                first_of_month(&point.date).map_err(|e| format!("index `{}`: {e}", self.id))?;
            points.push(IndexPoint {
                date,
                value: value.to_string(),
            });
        }
        points.sort_by(|a, b| a.date.cmp(&b.date));
        let mut ordered: Vec<IndexPoint> = Vec::with_capacity(points.len());
        for point in points {
            match ordered.last_mut() {
                // A second point in one month replaces the first: the later
                // row of a date-ordered export is the revision.
                Some(last) if last.date == point.date => *last = point,
                _ => ordered.push(point),
            }
        }
        self.points = ordered;
        Ok(self)
    }

    /// The value in force at `date`: the last point dated at or before it.
    /// `None` before the series starts — there is no money to restate into
    /// before the first month — and after the last point the latest value
    /// stands, which is what a published index's most recent month buys.
    pub fn value_at(&self, date: &str) -> Option<Decimal> {
        let date = first_of_month(date).ok()?;
        self.points
            .iter()
            .filter(|point| point.date.as_str() <= date.as_str())
            .max_by(|a, b| a.date.cmp(&b.date))
            .and_then(|point| point.value.parse().ok())
    }
}

/// [`IndexSeries::value_at`] as the module's one lookup, per the index contract.
pub fn index_at(series: &IndexSeries, date: &str) -> Option<Decimal> {
    series.value_at(date)
}

/// Restate `units` of money, spent at `at`, in the money of `base`:
/// `units * index(base) / index(at)`.
///
/// This is the one definition every real-terms report uses. The multiplication
/// is exact; the quotient is rounded once to at most [`REAL_SCALE`] places,
/// midpoint away from zero, and then padded to at least the amount's own scale
/// — so `"100.00"` keeps two decimals even when the division is exact. The
/// result is a plain decimal string, never a float.
///
/// Both dates must be inside the series: before its first point there is no
/// index to convert with, and the error names the series and the date.
pub fn real_amount(
    units: &str,
    at: &str,
    base: &str,
    series: &IndexSeries,
) -> Result<String, String> {
    let amount: Decimal = units
        .trim()
        .parse()
        .map_err(|_| format!("`{units}` is not a decimal amount"))?;
    let at_date = first_of_month(at).map_err(|e| format!("price index `{}`: {e}", series.id))?;
    let base_date =
        first_of_month(base).map_err(|e| format!("price index `{}`: {e}", series.id))?;
    let at_value = series
        .value_at(&at_date)
        .ok_or_else(|| missing_point(series, &at_date))?;
    let base_value = series
        .value_at(&base_date)
        .ok_or_else(|| missing_point(series, &base_date))?;
    let converted = amount
        .checked_mul(base_value)
        .and_then(|scaled| scaled.checked_div(at_value))
        .ok_or_else(|| {
            format!(
                "could not convert {amount} with price index `{}`",
                series.id
            )
        })?;
    let rounded =
        converted.round_dp_with_strategy(REAL_SCALE, RoundingStrategy::MidpointAwayFromZero);
    let scale = amount.scale().max(rounded.scale());
    let mut result = rounded;
    result.rescale(scale);
    Ok(result.to_string())
}

/// The percentage change from `previous` to `latest`, rounded to two decimal
/// places and returned without a `%` sign, e.g. `"25.00"`. `None` when there is
/// no previous figure to compare against: a zero base has no percentage change.
pub(crate) fn percentage_change(previous: Decimal, latest: Decimal) -> Option<String> {
    if previous.is_zero() {
        return None;
    }
    let change = (latest - previous) * Decimal::from(100u32) / previous;
    let mut change = change.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
    change.rescale(2);
    Some(change.to_string())
}

/// The error for a date the series cannot answer for.
pub(crate) fn missing_point(series: &IndexSeries, date: &str) -> String {
    format!(
        "price index `{}` has no point at or before {date}",
        series.id
    )
}

/// Whether `text` is a `YYYY-MM` month label.
pub(crate) fn is_year_month(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 7
        && bytes[4] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && matches!(
            (bytes[5], bytes[6]),
            (b'0', b'1'..=b'9') | (b'1', b'0'..=b'2')
        )
}

/// Whether `text` is a strictly valid `YYYY-MM-DD` date.
pub(crate) fn is_iso_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes[..4].iter().all(u8::is_ascii_digit)
        || !bytes[5..7].iter().all(u8::is_ascii_digit)
        || !bytes[8..].iter().all(u8::is_ascii_digit)
    {
        return false;
    }
    let year: u32 = text[..4].parse().unwrap_or(0);
    let month: u32 = text[5..7].parse().unwrap_or(0);
    let day: u32 = text[8..10].parse().unwrap_or(0);
    (1..=12).contains(&month) && (1..=days_in_month(year, month)).contains(&day)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Normalise a point's date to `YYYY-MM-01`: the series is monthly, so a
/// publisher's full date and a `YYYY-MM` column land on the same point.
pub(crate) fn first_of_month(text: &str) -> Result<String, String> {
    let text = text.trim();
    if is_year_month(text) || is_iso_date(text) {
        return Ok(format!("{}-01", &text[..7]));
    }
    Err(format!("`{text}` is not a YYYY-MM or YYYY-MM-DD date"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(points: &[(&str, &str)]) -> IndexSeries {
        let points: Vec<IndexPoint> = points
            .iter()
            .map(|(date, value)| IndexPoint::new(date, value).unwrap())
            .collect();
        IndexSeries::new("tr-cpi", "Türkiye CPI", "TÜİK", false, "TRY", points).unwrap()
    }

    /// The lookup is the step function a monthly index is: a month's point is
    /// in force until the next point, and nothing is known before the first.
    #[test]
    fn lookup_is_step_wise_and_none_before_the_first_point() {
        let series = series(&[("2026-01", "100"), ("2026-03", "110")]);
        assert_eq!(index_at(&series, "2026-01-01"), Some(Decimal::from(100)));
        assert_eq!(index_at(&series, "2026-02-14"), Some(Decimal::from(100)));
        assert_eq!(index_at(&series, "2026-03-31"), Some(Decimal::from(110)));
        assert_eq!(index_at(&series, "2026-04-30"), Some(Decimal::from(110)));
        assert_eq!(index_at(&series, "2025-12-31"), None);
    }

    /// A nominal amount is restated into the base month's money with the
    /// published ratio, exactly: 123.45 at 125 into a 100 base is 98.76.
    #[test]
    fn real_amount_uses_the_published_ratio() {
        let series = series(&[("2026-01", "100"), ("2026-02", "125")]);
        assert_eq!(
            real_amount("123.45", "2026-02-01", "2026-01-01", &series).unwrap(),
            "98.76"
        );
        // Converting a month into itself is the identity.
        assert_eq!(
            real_amount("123.45", "2026-01-01", "2026-01-01", &series).unwrap(),
            "123.45"
        );
    }

    /// The quotient is padded back to the amount's own scale, so a 2-decimal
    /// ledger amount never comes back with a different number of decimals.
    #[test]
    fn real_amount_keeps_the_amounts_scale() {
        let series = series(&[("2026-01", "100"), ("2026-02", "125")]);
        assert_eq!(
            real_amount("100.00", "2026-02-01", "2026-01-01", &series).unwrap(),
            "80.00"
        );
    }

    /// A repeating quotient is rounded once, to eight places, midpoint away
    /// from zero — the documented scale of a real amount.
    #[test]
    fn real_amount_rounds_a_repeating_quotient() {
        let series = series(&[("2026-01", "3"), ("2026-02", "100")]);
        assert_eq!(
            real_amount("10.00", "2026-01-01", "2026-02-01", &series).unwrap(),
            "333.33333333"
        );
        assert_eq!(
            real_amount("10.00", "2026-02-01", "2026-01-01", &series).unwrap(),
            "0.30"
        );
    }

    /// A date before the series is rejected with the series named, as the
    /// contract requires; a malformed date names it too.
    #[test]
    fn real_amount_rejects_dates_before_the_series() {
        let series = series(&[("2026-01", "100"), ("2026-02", "125")]);
        let error = real_amount("10.00", "2025-12-01", "2026-01-01", &series).unwrap_err();
        assert!(error.contains("tr-cpi"), "{error}");
        assert!(error.contains("2025-12-01"), "{error}");
        let error = real_amount("10.00", "not-a-date", "2026-01-01", &series).unwrap_err();
        assert!(error.contains("tr-cpi"), "{error}");
    }

    /// The constructor canonicalises the order and the date spelling the lookup
    /// assumes, and a repeated month keeps its last row (the revision).
    #[test]
    fn series_normalises_order_and_dates() {
        let series = IndexSeries::new(
            " tr-cpi ",
            "Türkiye CPI",
            "TÜİK",
            false,
            "TRY",
            vec![
                IndexPoint::new("2026-02-15", "120").unwrap(),
                IndexPoint::new("2026-01-31", "100").unwrap(),
                IndexPoint::new("2026-02-01", "125").unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(series.id, "tr-cpi");
        assert_eq!(series.frequency, MONTHLY);
        assert_eq!(
            series
                .points
                .iter()
                .map(|point| (point.date.as_str(), point.value.as_str()))
                .collect::<Vec<_>>(),
            [("2026-01-01", "100"), ("2026-02-01", "125")]
        );
    }

    /// A series the contract cannot chart is refused: no points, a non-monthly
    /// frequency, a zero value, or a date that is not a date.
    #[test]
    fn series_validation_rejects_bad_shape() {
        let empty = IndexSeries::new("x", "X", "S", false, "TRY", Vec::new()).unwrap_err();
        assert!(empty.contains("no points"), "{empty}");

        let mut monthly = series(&[("2026-01", "100")]);
        monthly.frequency = "daily".to_owned();
        assert!(monthly.normalised().unwrap_err().contains("monthly"));

        let zero = IndexPoint::new("2026-01", "0").unwrap_err();
        assert!(zero.contains("positive"), "{zero}");

        let bad_date = IndexPoint::new("2026-02-30", "100").unwrap_err();
        assert!(bad_date.contains("2026-02-30"), "{bad_date}");
    }

    /// A percentage is exact and two-decimal, with no previous figure yielding
    /// no percentage rather than an invented 100%.
    #[test]
    fn percentage_change_is_two_decimal() {
        assert_eq!(
            percentage_change(Decimal::from(120), Decimal::from(60)),
            Some("-50.00".to_owned())
        );
        assert_eq!(
            percentage_change(Decimal::from(100), Decimal::from(125)),
            Some("25.00".to_owned())
        );
        assert_eq!(percentage_change(Decimal::ZERO, Decimal::from(10)), None);
    }
}
