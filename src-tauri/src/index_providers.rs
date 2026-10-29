// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The publishers the app can fetch a monthly index from.
//!
//! Every source is split in two: a pure `parse` that turns the publisher's
//! payload text into points, and a thin `fetch` that downloads the endpoint and
//! hands the text to `parse`. Tests run entirely on canned payloads, so no test
//! touches the network.
//!
//! `rustledger` 0.24 already ships a blocking-`ureq` market-quote fetcher
//! (`rustledger::cmd::price`: `PriceSource`, `PriceSourceRegistry`,
//! `fetch_price`, an on-disk cache, eleven sources including `ecb`, `ratesapi`
//! and `yahoo`). That layer fetches security prices, not inflation indexes, so
//! it is deliberately not reused here and this module is deliberately not
//! folded into it. A future reader looking for quote fetching should start
//! there; a future reader looking for CPI/HICP series should stay here.
//!
//! Sources whose publisher offers no stable, documented download endpoint still
//! implement the same trait: their `parse` reads the `date,value` CSV the
//! importer accepts (the portal exports are downloadable by hand), while
//! `fetch` returns an explanation instead of a URL guessed to 404. TÜİK's data
//! portal hands out short-lived signed download links and ENAG publishes a
//! monthly table rather than an API, so both are in that group.
//!
//! The ECB reference rates are here because this ledger is multi-currency, but
//! they are a *rate* series, not a price level: a point is the quoted currency
//! per euro on the month's last trading day, `currency` is the base (`EUR`), and
//! [`real_amount`](crate::index::real_amount) must not be applied to them.

use std::collections::BTreeMap;
use std::io::Read;
use std::time::Duration;

use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::index::{IndexPoint, IndexSeries, is_iso_date};
use crate::index_store::points_from_csv;

/// The largest payload a provider will read. The ECB's full daily history is
/// the biggest documented payload used here (about 8 MB), so this is generous
/// while still keeping a broken endpoint from filling memory.
const MAX_PAYLOAD: u64 = 64 * 1024 * 1024;

/// What a provider publishes, without its points.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProviderSeries {
    /// The id the installed series takes, e.g. `us-cpi`.
    pub id: &'static str,
    /// The label the reports show.
    pub label: &'static str,
    /// The publisher's own name.
    pub source: &'static str,
    /// Whether the publisher is not a statistical agency.
    pub unofficial: bool,
    /// The base money the index is expressed in.
    pub currency: &'static str,
}

/// A source of one monthly index series.
pub(crate) trait IndexProvider: Send + Sync {
    /// What this provider publishes.
    fn series(&self) -> ProviderSeries;

    /// The documented endpoint to download, or an explanation of why there is
    /// none to download from automatically.
    fn endpoint(&self) -> Result<String, String>;

    /// Turn the publisher's raw payload into points. Pure: no clock, no
    /// network, no filesystem.
    fn parse(&self, payload: &str) -> Result<Vec<IndexPoint>, String>;

    /// Download and parse the series.
    fn fetch(&self) -> Result<IndexSeries, String> {
        let endpoint = self.endpoint()?;
        let payload = fetch_text(&endpoint)?;
        let points = self.parse(&payload)?;
        let meta = self.series();
        IndexSeries::new(
            meta.id,
            meta.label,
            meta.source,
            meta.unofficial,
            meta.currency,
            points,
        )
    }
}

/// The providers the app offers, in the order a picker may show them.
pub(crate) const PROVIDERS: &[&dyn IndexProvider] = &[
    &FRED_CPI,
    &FRED_PPI,
    &EUROSTAT_HICP,
    &TUIK_CPI,
    &TUIK_PPI,
    &ENAG,
    &ECB_USD,
    &ECB_TRY,
];

/// The provider with this series id, if the app knows it.
pub(crate) fn provider(id: &str) -> Option<&'static dyn IndexProvider> {
    PROVIDERS
        .iter()
        .copied()
        .find(|provider| provider.series().id == id)
}

/// Blocking download, the same choice `rustledger::cmd::price` makes: a Tauri
/// command already runs off the UI thread, and a plain blocking GET needs no
/// runtime in a backend that otherwise has none.
fn fetch_text(url: &str) -> Result<String, String> {
    let response = ureq::get(url)
        .timeout(Duration::from_secs(60))
        .call()
        .map_err(|error| format!("could not fetch {url}: {error}"))?;
    let mut text = String::new();
    response
        .into_reader()
        .take(MAX_PAYLOAD)
        .read_to_string(&mut text)
        .map_err(|error| format!("could not read {url}: {error}"))?;
    Ok(text)
}

/// Whether a CSV/TSV cell opens with a date, the test that separates data rows
/// from the header row a publisher's export starts with.
fn date_like(text: &str) -> bool {
    text.len() >= 7
        && text.as_bytes()[..4].iter().all(u8::is_ascii_digit)
        && text.as_bytes()[4] == b'-'
}

// -- FRED ---------------------------------------------------------------------

/// A series from FRED, the St. Louis Fed's public database, whose
/// `fredgraph.csv` endpoint is documented and needs no key.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FredProvider {
    meta: ProviderSeries,
    /// FRED's own series id, e.g. `CPIAUCSL`.
    fred_series: &'static str,
}

/// US consumer price index, all urban consumers, seasonally adjusted.
pub(crate) const FRED_CPI: FredProvider = FredProvider {
    meta: ProviderSeries {
        id: "us-cpi",
        label: "US CPI (CPIAUCSL)",
        source: "FRED",
        unofficial: false,
        currency: "USD",
    },
    fred_series: "CPIAUCSL",
};

/// US producer price index, all commodities.
pub(crate) const FRED_PPI: FredProvider = FredProvider {
    meta: ProviderSeries {
        id: "us-ppi",
        label: "US PPI (PPIACO)",
        source: "FRED",
        unofficial: false,
        currency: "USD",
    },
    fred_series: "PPIACO",
};

impl IndexProvider for FredProvider {
    fn series(&self) -> ProviderSeries {
        self.meta
    }

    fn endpoint(&self) -> Result<String, String> {
        Ok(format!(
            "https://fred.stlouisfed.org/graph/fredgraph.csv?id={}",
            self.fred_series
        ))
    }

    /// `observation_date,<series>` rows, one per month. FRED marks a month with
    /// no observation as `.`, which is skipped rather than mapped to zero.
    fn parse(&self, payload: &str) -> Result<Vec<IndexPoint>, String> {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .trim(csv::Trim::All)
            .from_reader(payload.as_bytes());
        let mut points = Vec::new();
        for (index, record) in reader.records().enumerate() {
            let row = index + 1;
            let record = record.map_err(|error| format!("FRED row {row}: {error}"))?;
            let date = record.get(0).unwrap_or("").trim();
            let value = record.get(1).unwrap_or("").trim();
            if !date_like(date) {
                continue;
            }
            if value.is_empty() || value == "." {
                continue;
            }
            let point =
                IndexPoint::new(date, value).map_err(|error| format!("FRED row {row}: {error}"))?;
            points.push(point);
        }
        if points.is_empty() {
            return Err(format!(
                "FRED returned no observations for {}",
                self.fred_series
            ));
        }
        Ok(points)
    }
}

// -- Eurostat -----------------------------------------------------------------

/// The euro area's harmonised index of consumer prices, from Eurostat's
/// documented dissemination API (JSON-stat 2.0).
#[derive(Debug, Clone, Copy)]
pub(crate) struct EurostatHicpProvider {
    meta: ProviderSeries,
}

pub(crate) const EUROSTAT_HICP: EurostatHicpProvider = EurostatHicpProvider {
    meta: ProviderSeries {
        id: "eu-hicp",
        label: "Euro area HICP (Eurostat)",
        source: "Eurostat",
        unofficial: false,
        currency: "EUR",
    },
};

impl IndexProvider for EurostatHicpProvider {
    fn series(&self) -> ProviderSeries {
        self.meta
    }

    fn endpoint(&self) -> Result<String, String> {
        Ok("https://ec.europa.eu/eurostat/api/dissemination/statistics/1.0/data/prc_hicp_midx?format=JSON&lang=EN&freq=M&unit=I15&coicop=CP00&geo=EA20".to_owned())
    }

    /// Parse JSON-stat 2.0. The URL above selects one category in every
    /// dimension but `time`, so a response with more than one category there
    /// would make "the" series ambiguous and is refused. Values stay raw JSON
    /// numbers (`RawValue`) until parsed as [`Decimal`], so no float touches an
    /// index point.
    fn parse(&self, payload: &str) -> Result<Vec<IndexPoint>, String> {
        let dataset: EurostatDataset = serde_json::from_str(payload)
            .map_err(|error| format!("Eurostat returned invalid JSON: {error}"))?;
        let time = dataset
            .id
            .iter()
            .position(|dimension| dimension == "time")
            .ok_or_else(|| "Eurostat response has no `time` dimension".to_owned())?;
        if dataset.id.len() != dataset.size.len() {
            return Err("Eurostat response has inconsistent dimension sizes".to_owned());
        }
        // JSON-stat flattens dimensions row-major: the last dimension varies
        // fastest. Strides let this read the time positions even if a future
        // response orders the dimensions differently.
        let mut stride = vec![1usize; dataset.size.len()];
        for position in (0..dataset.size.len().saturating_sub(1)).rev() {
            stride[position] = stride[position + 1]
                .checked_mul(dataset.size[position + 1])
                .ok_or_else(|| "Eurostat response size overflows".to_owned())?;
        }
        let mut base = 0usize;
        for (position, name) in dataset.id.iter().enumerate() {
            if position == time {
                continue;
            }
            let dimension = dataset
                .dimension
                .get(name)
                .ok_or_else(|| format!("Eurostat response has no `{name}` dimension"))?;
            if dataset.size[position] != 1 || dimension.category.index.len() != 1 {
                return Err(format!(
                    "Eurostat response holds more than one `{name}` category; the endpoint must select a single series"
                ));
            }
            let Some((_, &category_position)) = dimension.category.index.iter().next() else {
                return Err(format!("Eurostat `{name}` dimension is empty"));
            };
            base += category_position * stride[position];
        }
        let time_dimension = dataset
            .dimension
            .get(&dataset.id[time])
            .ok_or_else(|| "Eurostat response has no time categories".to_owned())?;
        let mut points = Vec::with_capacity(time_dimension.category.index.len());
        for (label, position) in &time_dimension.category.index {
            let linear = base + position * stride[time];
            let Some(raw) = dataset.value.get(&linear.to_string()) else {
                continue;
            };
            let text = raw.get().trim();
            if text == "null" {
                continue;
            }
            let point = IndexPoint::new(label, text)
                .map_err(|error| format!("Eurostat {label}: {error}"))?;
            points.push(point);
        }
        if points.is_empty() {
            return Err("Eurostat returned no values for the series".to_owned());
        }
        Ok(points)
    }
}

#[derive(Debug, Deserialize)]
struct EurostatDataset {
    id: Vec<String>,
    size: Vec<usize>,
    dimension: BTreeMap<String, EurostatDimension>,
    #[serde(default)]
    value: BTreeMap<String, Box<RawValue>>,
}

#[derive(Debug, Deserialize)]
struct EurostatDimension {
    category: EurostatCategory,
}

#[derive(Debug, Deserialize)]
struct EurostatCategory {
    index: BTreeMap<String, usize>,
}

// -- Publishers with no stable download endpoint -----------------------------

/// A publisher whose portal has no stable, documented download URL. `parse`
/// reads the `date,value` CSV the importer accepts; `fetch` explains where a
/// human has to download it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CsvProvider {
    meta: ProviderSeries,
    explanation: &'static str,
}

/// Türkiye's consumer price index, from TÜİK.
pub(crate) const TUIK_CPI: CsvProvider = CsvProvider {
    meta: ProviderSeries {
        id: "tr-cpi",
        label: "Türkiye CPI (TÜİK)",
        source: "TÜİK",
        unofficial: false,
        currency: "TRY",
    },
    explanation: "TÜİK publishes through its data portal, whose download links are short-lived signed URLs rather than a stable endpoint; export the series as a `date,value` CSV and import it",
};

/// Türkiye's producer price index, from TÜİK.
pub(crate) const TUIK_PPI: CsvProvider = CsvProvider {
    meta: ProviderSeries {
        id: "tr-ppi",
        label: "Türkiye PPI (TÜİK)",
        source: "TÜİK",
        unofficial: false,
        currency: "TRY",
    },
    explanation: "TÜİK publishes through its data portal, whose download links are short-lived signed URLs rather than a stable endpoint; export the series as a `date,value` CSV and import it",
};

/// ENAG's independent inflation measure for Türkiye.
pub(crate) const ENAG: CsvProvider = CsvProvider {
    meta: ProviderSeries {
        id: "tr-enag",
        label: "Türkiye inflation (ENAG)",
        source: "ENAG",
        unofficial: true,
        currency: "TRY",
    },
    explanation: "ENAG publishes a monthly table rather than a documented download API; save it as a `date,value` CSV and import it",
};

impl IndexProvider for CsvProvider {
    fn series(&self) -> ProviderSeries {
        self.meta
    }

    fn endpoint(&self) -> Result<String, String> {
        Err(self.explanation.to_owned())
    }

    fn parse(&self, payload: &str) -> Result<Vec<IndexPoint>, String> {
        points_from_csv(payload)
    }
}

// -- ECB reference rates ------------------------------------------------------

/// The ECB's daily euro reference rates, collapsed to one point per month: the
/// rate on the month's last trading day.
///
/// This is the one series here that is not a price level — it is a currency
/// rate, so its points are only for converting between currencies, never for
/// [`real_amount`](crate::index::real_amount). The `currency` field is the base
/// (`EUR`) and a point's value is the quote currency per euro.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EcbProvider {
    meta: ProviderSeries,
    /// The quote currency, e.g. `USD`.
    quote: &'static str,
}

/// US dollars per euro, month-end.
pub(crate) const ECB_USD: EcbProvider = EcbProvider {
    meta: ProviderSeries {
        id: "ecb-usd",
        label: "ECB reference rate (USD per EUR)",
        source: "ECB",
        unofficial: false,
        currency: "EUR",
    },
    quote: "USD",
};

/// Turkish lira per euro, month-end.
pub(crate) const ECB_TRY: EcbProvider = EcbProvider {
    meta: ProviderSeries {
        id: "ecb-try",
        label: "ECB reference rate (TRY per EUR)",
        source: "ECB",
        unofficial: false,
        currency: "EUR",
    },
    quote: "TRY",
};

impl IndexProvider for EcbProvider {
    fn series(&self) -> ProviderSeries {
        self.meta
    }

    fn endpoint(&self) -> Result<String, String> {
        Ok("https://www.ecb.europa.eu/stats/eurofxref/eurofxref-hist.xml".to_owned())
    }

    /// The ECB's `eurofxref-hist.xml` is a flat list of
    /// `<Cube time="YYYY-MM-DD">` days, each holding one
    /// `<Cube currency="USD" rate="…"/>` per currency. The format is fixed and
    /// tiny, so a scanner over those two attributes avoids an XML dependency.
    /// Rows are kept by the month's latest trading day; weekends and holidays
    /// are simply absent from the payload.
    fn parse(&self, payload: &str) -> Result<Vec<IndexPoint>, String> {
        let needle = format!("currency=\"{}\"", self.quote);
        let mut latest: BTreeMap<String, (String, Decimal)> = BTreeMap::new();
        let mut seen_days = 0usize;
        let mut rest = payload;
        while let Some(start) = rest.find("<Cube time=\"") {
            rest = &rest[start + "<Cube time=\"".len()..];
            let Some(end) = rest.find('"') else {
                break;
            };
            let date = &rest[..end];
            if !is_iso_date(date) {
                return Err(format!(
                    "ECB reference rates hold the invalid date `{date}`"
                ));
            }
            rest = &rest[end..];
            let block_end = rest.find("<Cube time=\"").unwrap_or(rest.len());
            let block = &rest[..block_end];
            seen_days += 1;
            if let Some(currency_at) = block.find(&needle) {
                let after_currency = &block[currency_at + needle.len()..];
                if let Some(rate_at) = after_currency.find("rate=\"") {
                    let after_rate = &after_currency[rate_at + "rate=\"".len()..];
                    if let Some(rate_end) = after_rate.find('"') {
                        let rate_text = after_rate[..rate_end].trim();
                        let rate: Decimal = rate_text.parse().map_err(|_| {
                            format!("ECB {date}: `{rate_text}` is not a decimal rate")
                        })?;
                        let month = date[..7].to_owned();
                        let newer = latest
                            .get(&month)
                            .is_none_or(|known| known.0.as_str() < date);
                        if newer {
                            latest.insert(month, (date.to_owned(), rate));
                        }
                    }
                }
            }
            rest = &rest[block_end..];
        }
        if seen_days == 0 {
            return Err("ECB reference rates hold no dated cubes".to_owned());
        }
        if latest.is_empty() {
            return Err(format!("ECB reference rates hold no `{}` rate", self.quote));
        }
        let mut points = Vec::with_capacity(latest.len());
        for (month, (_, rate)) in latest {
            points.push(
                IndexPoint::new(&format!("{month}-01"), &rate.to_string())
                    .map_err(|error| format!("ECB {month}: {error}"))?,
            );
        }
        Ok(points)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(points: &[IndexPoint]) -> Vec<(&str, &str)> {
        points
            .iter()
            .map(|point| (point.date.as_str(), point.value.as_str()))
            .collect()
    }

    /// FRED's CSV: the header is skipped, a `.` observation is skipped, and the
    /// remaining monthly rows keep their published scale.
    #[test]
    fn fred_parse_reads_monthly_observations() {
        let payload = "\
observation_date,CPIAUCSL
2026-01-01,300.100
2026-02-01,.
2026-03-01,305.250
";
        assert_eq!(
            values(&FRED_CPI.parse(payload).unwrap()),
            [("2026-01-01", "300.100"), ("2026-03-01", "305.250")]
        );
    }

    /// Eurostat's JSON-stat payload: time is the last dimension, missing values
    /// are skipped, and the number's spelling reaches the point unchanged.
    #[test]
    fn eurostat_parse_reads_json_stat() {
        let payload = r#"{
            "id": ["freq", "unit", "coicop", "geo", "time"],
            "size": [1, 1, 1, 1, 3],
            "dimension": {
                "freq": {"category": {"index": {"M": 0}}},
                "unit": {"category": {"index": {"I15": 0}}},
                "coicop": {"category": {"index": {"CP00": 0}}},
                "geo": {"category": {"index": {"EA20": 0}}},
                "time": {"category": {"index": {"2026-01": 0, "2026-02": 1, "2026-03": 2}}}
            },
            "value": {"0": 123.4, "1": 124.567, "2": null}
        }"#;
        assert_eq!(
            values(&EUROSTAT_HICP.parse(payload).unwrap()),
            [("2026-01-01", "123.4"), ("2026-02-01", "124.567")]
        );
    }

    /// JSON-stat's flattening arithmetic must follow the declared dimension
    /// order, not assume time is last.
    #[test]
    fn eurostat_parse_follows_dimension_order() {
        let payload = r#"{
            "id": ["time", "geo"],
            "size": [2, 1],
            "dimension": {
                "time": {"category": {"index": {"2026-01": 0, "2026-02": 1}}},
                "geo": {"category": {"index": {"EA20": 0}}}
            },
            "value": {"0": 100, "1": 101.5}
        }"#;
        assert_eq!(
            values(&EUROSTAT_HICP.parse(payload).unwrap()),
            [("2026-01-01", "100"), ("2026-02-01", "101.5")]
        );
    }

    /// A response holding more than one series is ambiguous and refused rather
    /// than resolved by guessing a position.
    #[test]
    fn eurostat_parse_refuses_multiple_categories() {
        let payload = r#"{
            "id": ["geo", "time"],
            "size": [2, 1],
            "dimension": {
                "geo": {"category": {"index": {"EA20": 0, "EU27_2020": 1}}},
                "time": {"category": {"index": {"2026-01": 0}}}
            },
            "value": {"0": 100, "1": 101}
        }"#;
        let error = EUROSTAT_HICP.parse(payload).unwrap_err();
        assert!(error.contains("single series"), "{error}");
    }

    /// TÜİK and ENAG parse the same `date,value` export the importer accepts;
    /// their fetch says why there is no URL to call.
    #[test]
    fn csv_providers_parse_the_import_dialect() {
        let payload = "date,value\n2026-01,100\n2026-02,125.5\n";
        assert_eq!(
            values(&TUIK_CPI.parse(payload).unwrap()),
            [("2026-01-01", "100"), ("2026-02-01", "125.5")]
        );
        assert!(TUIK_PPI.parse(payload).is_ok());
        assert!(ENAG.parse(payload).is_ok());
        assert!(TUIK_CPI.fetch().unwrap_err().contains("data portal"));
        assert!(ENAG.fetch().unwrap_err().contains("monthly table"));
    }

    /// The ECB XML scanner keeps the last trading day of each month for the
    /// requested quote currency and ignores the other currencies.
    #[test]
    fn ecb_parse_keeps_the_months_last_trading_day() {
        let payload = r#"<?xml version="1.0" encoding="UTF-8"?>
<gesmes:Envelope xmlns:gesmes="http://www.gesmes.org/xml/2002-08-01">
  <Cube>
    <Cube time="2026-02-27"><Cube currency="USD" rate="1.1000"/><Cube currency="TRY" rate="40.00"/></Cube>
    <Cube time="2026-02-28"><Cube currency="USD" rate="1.1200"/><Cube currency="TRY" rate="40.50"/></Cube>
    <Cube time="2026-03-02"><Cube currency="USD" rate="1.1300"/></Cube>
  </Cube>
</gesmes:Envelope>"#;
        assert_eq!(
            values(&ECB_USD.parse(payload).unwrap()),
            [("2026-02-01", "1.1200"), ("2026-03-01", "1.1300")]
        );
        assert_eq!(
            values(&ECB_TRY.parse(payload).unwrap()),
            [("2026-02-01", "40.50")]
        );
    }

    /// A currency the payload does not quote is an error that names it, not an
    /// empty series.
    #[test]
    fn ecb_parse_names_a_missing_currency() {
        let payload =
            r#"<Cube><Cube time="2026-02-28"><Cube currency="USD" rate="1.12"/></Cube></Cube>"#;
        let error = ECB_TRY.parse(payload).unwrap_err();
        assert!(error.contains("TRY"), "{error}");
    }

    /// The registry offers every provider by its series id.
    #[test]
    fn registry_finds_providers_by_id() {
        for provider in PROVIDERS {
            let id = provider.series().id;
            assert!(self::provider(id).is_some(), "missing provider {id}");
        }
        assert!(provider("nope").is_none());
    }
}
