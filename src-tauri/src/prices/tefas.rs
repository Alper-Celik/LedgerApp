// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! TEFAS: the Turkish fund platform's daily unit prices (NAV), as a
//! [`PriceSource`] of this app's own — upstream `rustledger` has no TEFAS
//! provider, but its trait is public and object-safe, so nothing is forked.
//!
//! # The endpoint
//!
//! TEFAS rewrote its site in April 2026: the old `/api/DB/BindHistoryInfo`
//! endpoint (which `tefas-crawler` and friends used) answers
//! `404 Method not found or disabled!`, and prices now come from
//! `POST https://www.tefas.gov.tr/api/funds/fonFiyatBilgiGetir` with a JSON
//! body `{"fonKodu": "AFA", "dil": "TR", "periyod": 13}` and a JSON envelope
//! `{"errorCode": …, "errorMessage": …, "resultList": [{"tarih": "YYYY-MM-DD",
//! "fiyat": 1.322508, …}]}`. `periyod` is a fixed period code, not a day count:
//! `13` is the shortest bucket (about a week), so it always carries the newest
//! published NAV; the API's other codes (`1`, `3`, `6`, `12`, `36`, `60`) are
//! coarser buckets capped at five years of history.
//!
//! Because `periyod` buckets are not date ranges, this source is **latest
//! only**: `historical_coverage` keeps the trait's default, so the dispatch
//! refuses a dated request before any I/O instead of mislabelling a bucket's
//! newest row with a requested day.
//!
//! # Shape changes
//!
//! An API read off a site's own frontend is not a contract, which is why the
//! parse is a pure function with its own error text: an empty body (the site
//! answers non-browser clients that way when its bot protection is on), a
//! non-JSON body, an envelope without `resultList`, or rows without `tarih`
//! and `fiyat` all come back as "TEFAS did not answer … the site may have
//! changed shape", never as a silent empty price.
//!
//! # Assumption: fund codes are the commodities
//!
//! A TEFAS fund code (`AFA`, `TTE`, `TI2`, …) is a short uppercase
//! alphanumeric — the ledger is expected to hold the fund under exactly that
//! code, quoted in `TRY`:

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use rust_decimal::Decimal;
use rustledger::cmd::price::PriceResponse;
use rustledger::cmd::price::sources::{PricePair, PriceSource};
use rustledger_core::NaiveDate;

/// The name a request asks this source by, and the name the source list shows.
pub(crate) const NAME: &str = "tefas";

/// What the source list says it covers.
pub(crate) const DESCRIPTION: &str = "TEFAS — Turkish fund unit prices, in TRY (tefas.gov.tr)";

/// Every TEFAS price is in lira; the platform publishes nothing else.
pub(crate) const QUOTE_CURRENCY: &str = "TRY";

/// The v2 daily-price endpoint (see the module docs).
const ENDPOINT: &str = "https://www.tefas.gov.tr/api/funds/fonFiyatBilgiGetir";

/// The API's shortest period bucket — about a week of business days, which
/// always contains the newest NAV.
const WEEKLY_PERIOD: u32 = 13;

/// Named honestly rather than imitating a browser: this is the app asking.
const USER_AGENT: &str = "Mozilla/5.0 (compatible; ledger-app/0.1)";

/// How long a connection may take to come up, before the request's own timeout
/// even starts counting.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The source, ready to be boxed into the fetch map.
pub(crate) fn source(timeout: Duration) -> Arc<dyn PriceSource> {
    Arc::new(TefasSource {
        timeout,
        agent: ureq::AgentBuilder::new()
            .timeout_connect(CONNECT_TIMEOUT)
            .build(),
    })
}

/// A TEFAS source over one connection-pooling agent.
struct TefasSource {
    /// The whole-request deadline. Upstream's sources carry a timeout they do
    /// not apply (ureq 3 cannot time a single request); this one is set on the
    /// request, so a hung endpoint cannot hold the fetch thread.
    timeout: Duration,
    /// The agent owns the connection pool; building one per source keeps
    /// repeated fetches to the same host on one connection.
    agent: ureq::Agent,
}

impl PriceSource for TefasSource {
    fn name(&self) -> &'static str {
        NAME
    }

    fn description(&self) -> &'static str {
        DESCRIPTION
    }

    fn fetch_latest(&self, pair: &PricePair) -> anyhow::Result<PriceResponse> {
        let fund = checked_fund_code(&pair.ticker).map_err(anyhow::Error::msg)?;
        let body = self.post(&fund).map_err(anyhow::Error::msg)?;
        let (date, price) = parse_latest(&fund, &body).map_err(anyhow::Error::msg)?;
        Ok(PriceResponse {
            price,
            currency: QUOTE_CURRENCY.to_string(),
            date,
            source: NAME.to_string(),
        })
    }
}

impl TefasSource {
    /// The endpoint's answer body, or why there is none.
    fn post(&self, fund: &str) -> Result<String, String> {
        let payload = payload_of(fund);
        let response = self
            .agent
            .post(ENDPOINT)
            .timeout(self.timeout)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json")
            .set("User-Agent", USER_AGENT)
            .send_string(&payload);
        match response {
            Ok(response) => response
                .into_string()
                .map_err(|error| format!("could not read the TEFAS answer for `{fund}`: {error}")),
            Err(ureq::Error::Status(code, response)) => {
                let body = response.into_string().unwrap_or_default();
                Err(format!(
                    "TEFAS answered HTTP {code} for `{fund}`: {}",
                    preview(&body)
                ))
            }
            Err(error) => Err(format!("could not reach TEFAS for `{fund}`: {error}")),
        }
    }
}

/// The JSON body the v2 endpoint takes for one fund's newest prices.
fn payload_of(fund: &str) -> String {
    serde_json::json!({
        "fonKodu": fund,
        "dil": "TR",
        "periyod": WEEKLY_PERIOD,
    })
    .to_string()
}

/// The newest `(date, price)` in a `fonFiyatBilgiGetir` answer — pure, so the
/// canned payloads below stand in for the site.
///
/// The row's own `tarih` labels the price; nothing here reads the clock.
/// Rows are taken in any order and the greatest date wins, because the API's
/// ordering is not part of the envelope's contract.
fn parse_latest(fund: &str, body: &str) -> Result<(NaiveDate, Decimal), String> {
    let body = body.trim();
    if body.is_empty() {
        return Err(format!(
            "TEFAS answered with an empty body for `{fund}`; the site refuses plain HTTP \
             clients that way behind its bot protection"
        ));
    }
    let json: serde_json::Value =
        serde_json::from_str(body).map_err(|_| shape_error(fund, body))?;

    if let Some(message) = json
        .get("errorMessage")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
    {
        return Err(format!("TEFAS reported `{message}` for `{fund}`"));
    }

    let rows = json
        .get("resultList")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| shape_error(fund, body))?;

    rows.iter()
        .filter_map(|row| {
            let date = row
                .get("tarih")
                .and_then(serde_json::Value::as_str)
                .and_then(feed_date)?;
            let price = row.get("fiyat").and_then(price_of)?;
            Some((date, price))
        })
        .max_by_key(|(date, _)| *date)
        .ok_or_else(|| {
            format!(
                "TEFAS has no unit price for `{fund}`; the API answers an empty list for a \
                 fund code it does not know"
            )
        })
}

/// A feed date: a bare `YYYY-MM-DD`, or the date prefix of an ISO timestamp.
fn feed_date(raw: &str) -> Option<NaiveDate> {
    raw.get(..10)?.parse::<NaiveDate>().ok()
}

/// A `fiyat` value as an exact decimal. The field arrives as a JSON number;
/// a string is accepted too (with a comma read as the Turkish decimal mark),
/// so a serialization change cannot turn every price into "no price".
fn price_of(value: &serde_json::Value) -> Option<Decimal> {
    let raw = match value {
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => text.replace(',', "."),
        _ => return None,
    };
    Decimal::from_str(raw.trim())
        .ok()
        .filter(|price| *price > Decimal::ZERO)
}

/// The error for an answer that is not the envelope this source knows.
fn shape_error(fund: &str, body: &str) -> String {
    format!(
        "TEFAS did not answer the `{ENDPOINT}` envelope for `{fund}`; the site may have \
         changed shape. Expected {{\"errorCode\", \"errorMessage\", \"resultList\": \
         [{{\"tarih\", \"fiyat\"}}]}}, got: {}",
        preview(body)
    )
}

/// A body cut down to something an error message can carry.
fn preview(body: &str) -> String {
    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = flat.chars().take(200).collect();
    if cut.len() < flat.len() {
        format!("{cut}…")
    } else {
        cut
    }
}

/// A TEFAS fund code: a short uppercase alphanumeric, which is also the
/// commodity the ledger holds it under.
fn checked_fund_code(ticker: &str) -> Result<String, String> {
    // A fund code becomes a ledger commodity, so it is held to the commodity rule
    // first and only then to the platform's own length bound — one place decides
    // what a commodity may look like, and this adds the narrower constraint.
    let code = crate::prices::checked_commodity("fund code", ticker)?;
    if !(2..=10).contains(&code.len()) {
        return Err(format!(
            "`{ticker}` is not a TEFAS fund code (the short alphanumeric code the platform \
             lists a fund under, e.g. `AFA`)"
        ));
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A canned answer in the shape the v2 endpoint returns, deliberately not
    /// in date order: the newest NAV must win regardless.
    const ANSWER: &str = r#"{"errorCode":null,"errorMessage":null,"resultList":[
        {"fonKodu":"AFA","fonUnvan":"AK PORTFÖY AMERİKA YABANCI HİSSE SENEDİ FONU","tarih":"2026-10-27","fiyat":1.322382},
        {"fonKodu":"AFA","fonUnvan":"AK PORTFÖY AMERİKA YABANCI HİSSE SENEDİ FONU","tarih":"2026-10-28","fiyat":1.322508},
        {"fonKodu":"AFA","fonUnvan":"AK PORTFÖY AMERİKA YABANCI HİSSE SENEDİ FONU","tarih":"2026-10-21","fiyat":1.322749}
    ]}"#;

    #[test]
    fn takes_the_newest_nav_out_of_the_v2_envelope() {
        let (date, price) = parse_latest("AFA", ANSWER).expect("the canned answer parses");
        assert_eq!(date, rustledger_core::naive_date(2026, 10, 28).unwrap());
        assert_eq!(price.to_string(), "1.322508");
    }

    #[test]
    fn the_rows_own_date_labels_the_price() {
        // A timestamp prefix is a date too, and either way the label is the
        // feed's, never the fetch day.
        let body = r#"{"resultList":[{"tarih":"2026-10-23T00:00:00","fiyat":1.25}]}"#;
        let (date, price) = parse_latest("AFA", body).expect("a timestamp prefix parses");
        assert_eq!(date.to_string(), "2026-10-23");
        assert_eq!(price.to_string(), "1.25");
    }

    #[test]
    fn a_comma_decimal_string_is_read_as_a_turkish_price() {
        let body = r#"{"resultList":[{"tarih":"2026-10-23","fiyat":"1,2500"}]}"#;
        let (_, price) = parse_latest("AFA", body).expect("a string price parses");
        assert_eq!(price.to_string(), "1.2500");
    }

    #[test]
    fn an_error_envelope_is_reported_with_its_own_message() {
        let body = r#"{"errorCode":500,"errorMessage":"Sistem Hatası!!","resultList":null}"#;
        let error = parse_latest("AFA", body).expect_err("an error envelope is an error");
        assert!(error.contains("Sistem Hatası!!"), "{error}");
    }

    #[test]
    fn an_unknown_fund_is_an_empty_list_and_says_so() {
        // What the endpoint answers for a code it does not know (verified
        // against the live API: HTTP 200, empty `resultList`).
        let body = r#"{"errorCode":null,"errorMessage":null,"resultList":[]}"#;
        let error = parse_latest("ZZZ", body).expect_err("no rows, no price");
        assert!(error.contains("`ZZZ`"), "{error}");
        assert!(error.contains("no unit price"), "{error}");
    }

    #[test]
    fn an_empty_body_names_the_bot_protection() {
        let error = parse_latest("AFA", "  ").expect_err("an empty body is not a price");
        assert!(error.contains("empty body"), "{error}");
        assert!(error.contains("bot protection"), "{error}");
    }

    #[test]
    fn the_legacy_shape_is_reported_as_a_shape_change() {
        // The pre-April-2026 endpoint's `{"data": [...]}` shape must not be
        // mistaken for a quote.
        let error = parse_latest("AFA", r#"{"data":[{"FIYAT":1.32}]}"#)
            .expect_err("the old shape is not the envelope");
        assert!(error.contains("changed shape"), "{error}");
        assert!(error.contains("resultList"), "{error}");
        let error = parse_latest("AFA", "<html>maintenance</html>").expect_err("html is not JSON");
        assert!(error.contains("changed shape"), "{error}");
    }

    #[test]
    fn rows_without_a_usable_price_are_skipped_not_guessed() {
        let body = r#"{"resultList":[
            {"tarih":"2026-10-28","fiyat":null},
            {"tarih":"2026-10-27","fiyat":0},
            {"tarih":"garbage","fiyat":1.5},
            {"tarih":"2026-10-26","fiyat":1.4}
        ]}"#;
        let (date, price) = parse_latest("AFA", body).expect("the one usable row is taken");
        assert_eq!(date.to_string(), "2026-10-26");
        assert_eq!(price.to_string(), "1.4");
    }

    #[test]
    fn the_payload_carries_the_documented_fields() {
        let payload: serde_json::Value =
            serde_json::from_str(&payload_of("afa")).expect("the payload is JSON");
        assert_eq!(payload["fonKodu"], "afa");
        assert_eq!(payload["dil"], "TR");
        assert_eq!(payload["periyod"], WEEKLY_PERIOD);
    }

    #[test]
    fn fund_codes_are_checked_and_uppercased() {
        assert_eq!(checked_fund_code(" afa ").unwrap(), "AFA");
        assert_eq!(checked_fund_code("TI2").unwrap(), "TI2");
        for bad in ["", "A", "A F A", "AFA/TRY", "abcdefghijk"] {
            assert!(
                checked_fund_code(bad).is_err(),
                "`{bad}` is not a fund code"
            );
        }
    }

    #[test]
    fn the_source_is_latest_only_and_needs_no_key() {
        let source = source(Duration::from_secs(5));
        assert_eq!(source.name(), "tefas");
        assert!(!source.requires_api_key());
        // The trait's default is "latest only", which is the point: the API
        // serves period buckets, not date ranges.
        let past = rustledger_core::naive_date(2026, 10, 1).unwrap();
        let error = source
            .fetch_price(&rustledger::cmd::price::PriceRequest {
                ticker: "AFA".to_string(),
                currency: QUOTE_CURRENCY.to_string(),
                date: Some(past),
            })
            .expect_err("a dated request is refused before any I/O");
        assert!(error.to_string().contains("latest quote"), "{error}");
    }
}
