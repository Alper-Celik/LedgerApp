// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Price quotes: what in the ledger can be priced, fetching quotes for it, and
//! writing the fetched quotes into a journal file as `price` directives.
//!
//! # Where the fetching comes from
//!
//! `rustledger`'s own `cmd::price` layer — the same code `rledger price` runs —
//! is the fetcher: `PriceSourceRegistry` over eleven blocking sources, the
//! `discovery` pass that reads a ledger's `price:`/`quote_currency:` metadata
//! into per-commodity `(source, ticker, quote currency)` mappings, and
//! `cache::PriceCache`. Nothing about those sources is forked. Upstream has no
//! TEFAS provider (Turkish fund NAVs), so [`tefas`] implements the upstream
//! `PriceSource` trait and is kept in the name→source map this module builds
//! around the registry — the registry exposes no way to register a source of
//! one's own, and its lookups (`get`, `list_sources`) are public, so a thin map
//! over it is all the wrapping needed.
//!
//! # Blocking
//!
//! Every fetch here blocks on network I/O. [`fetch`] must therefore run off the
//! async runtime's thread (`tauri::async_runtime::spawn_blocking` from an
//! `async fn` command), and it holds no lock while it does: the ledger-derived
//! mapping is snapshotted under the read lock first.
//!
//! # The cache
//!
//! Quotes are cached in upstream's `cmd::price::cache` — the same
//! `~/.cache/rledger/prices.json` `rledger price` reads, keyed by
//! `(source, ticker, currency, date)` with a 30-minute TTL and past-day
//! entries that never expire. The app's own index store is not reused: it holds
//! analysis of the ledger's files, keyed by path and mtime, whereas a quote
//! cache is a TTL table of third-party values that are also useful to the CLI.
//!
//! # Failure shape of [`fetch`]
//!
//! One request in, one outcome out, in request order: [`PriceFetch::Quote`] or
//! [`PriceFetch::Failed`], so a row can show its own error while its neighbours
//! show their prices. `Err` is returned only when *no* request produced a quote
//! (the error text is the per-request failures joined); an empty request list
//! is an empty `Ok`. The frontend's register sends one request per row, so a
//! single-row fetch shows its failure through the `Err` arm either way.
//!
//! A request may leave its quote currency empty — the target has none to offer
//! — and that means "the source decides". Only a source that publishes exactly
//! one currency can answer that (TEFAS and its lira); every other source is
//! refused with a message naming the missing currency, because inventing one
//! would price the commodity in something the user never asked for.
//!
//! # What counts as what
//!
//! `PriceTarget::kind` is `"currency"` for an ISO-4217 fiat code (the precious
//! metals aside — a ledger prices those, it does not spend them), `"fund"` for
//! a commodity the ledger marks as one — `price: "TRY:tefas/AFA"`, or a
//! `commodity` directive whose `asset-class`/`type`/`category`/`instrument`
//! metadata says `FUND` — and `"commodity"` otherwise. Nothing is inferred from
//! the shape of the code alone: a three-letter ticker is as likely to be crypto
//! as a Turkish fund.
//!
//! # Writing
//!
//! [`write_to_file`] renders `YYYY-MM-DD price COMMODITY PRICE CURRENCY` lines
//! appended to the named journal file, then hands the whole file text to
//! [`edit_named_file_source`] — the app's one write path, whose
//! `editing::apply_file_edit` reloads the journal and puts the bytes back if the
//! loader complains about something the ledger did not already say. A second
//! call with the same quotes writes nothing: the ledger already carries that
//! exact `date price COMMODITY PRICE CURRENCY` line. A *different* price for a
//! day the ledger already prices is written — the quote moved, and dropping it
//! silently would leave the ledger saying something nobody chose.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use rust_decimal::Decimal;
use rustledger::cmd::price::cache::{PriceCache, cache_key};
use rustledger::cmd::price::discovery::{DiscoveredCommodity, discover_symbols};
use rustledger::cmd::price::{PriceRequest, PriceResponse, PriceSource, PriceSourceRegistry};
use rustledger::config::{CommodityMapping, PriceConfig, SourceRef};
use rustledger_core::{Directive, MetaValue, NaiveDate};
use rustledger_loader::Ledger;
use serde::Serialize;
use specta::Type;

use crate::editing::edit_named_file_source;
use crate::files::source_of_file;
use crate::session::AppState;

mod tefas;

/// How long a fetched quote stays fresh in the shared quote cache — upstream's
/// own default, so the app and `rledger price` agree on what "cached" means.
const CACHE_TTL_SECS: u64 = 1800;

/// The timeout the TEFAS source applies to its request. Upstream's sources
/// carry a timeout they do not enforce (ureq 3 cannot time a single request);
/// ours is set on the request itself, so a hung TEFAS cannot hold the fetch
/// thread.
const TEFAS_TIMEOUT: Duration = Duration::from_secs(30);

/// The fixed quote currency of a source that publishes exactly one — the only
/// way an empty requested currency can mean "the source decides". TEFAS prices
/// funds in lira and nothing else.
fn fixed_quote_currency(source: &str) -> Option<&'static str> {
    match source {
        tefas::NAME => Some(tefas::QUOTE_CURRENCY),
        _ => None,
    }
}

/// Whether a source serves a documented API of its own rather than an endpoint
/// read off the provider's web frontend. The distinction is what the UI shows
/// beside a source name: a documented API is one the provider means to keep
/// stable, an undocumented endpoint is one that can change shape under us —
/// which is exactly what happened to TEFAS in April 2026.
fn is_official(source: &str) -> bool {
    !matches!(source, "yahoo" | "eastmoneyfund" | tefas::NAME)
}

/// One source of quotes, as the picker shows it.
#[derive(Debug, Clone, Serialize, Type)]
pub struct PriceSourceInfo {
    /// The registry name, lowercase — what a [`QuoteRequest::source`] names.
    pub name: String,
    /// What the source covers, in the provider's own words.
    pub description: String,
    /// See [`is_official`].
    pub official: bool,
}

/// One quote: one commodity, one currency, one day, one exact decimal price.
///
/// Amounts are decimal strings throughout — the frontend never parses a price
/// into a float, and neither does the ledger (see [`write_to_file`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize, Type)]
pub struct PriceQuote {
    /// The commodity being priced, e.g. `AFA` or `AAPL`.
    pub commodity: String,
    /// The currency the price is in, e.g. `TRY`.
    pub currency: String,
    /// The quote's own day, `YYYY-MM-DD` — never the day it was fetched.
    pub date: String,
    /// The exact price as a decimal string.
    pub price: String,
    /// The source that supplied it, or `"ledger"` for a quote read back from
    /// the ledger itself (see [`PriceTarget::latest`]).
    pub source: String,
}

/// Something the ledger holds or quotes, and what is already known about its
/// price.
#[derive(Debug, Clone, Serialize, Type)]
pub struct PriceTarget {
    /// The commodity code as the ledger writes it.
    pub commodity: String,
    /// The currency to price it in: the quote currency of its newest `price`
    /// directive, else the `quote_currency:`/`price:` metadata, else the
    /// ledger's first `operating_currency`. `None` when the ledger says none —
    /// a fetch then has to carry a currency, or the source has to publish one.
    pub currency: Option<String>,
    /// `"currency"`, `"commodity"` or `"fund"`; see the module docs.
    pub kind: String,
    /// The newest `price` directive for this commodity already in the ledger,
    /// read from the loaded ledger and not fetched. `None` when the ledger has
    /// priced it never.
    pub latest: Option<PriceQuote>,
}

/// One fetch: a commodity, the currency to quote it in, and the source to ask,
/// with an optional date for a source that serves history.
///
/// An empty `currency` means "the source decides": [`fixed_quote_currency`]
/// answers for a source that publishes exactly one, and every other source is
/// refused with a message naming the missing currency rather than guessing
/// one.
#[derive(Debug, Clone, serde::Deserialize, Type)]
pub struct QuoteRequest {
    /// The commodity to price.
    pub commodity: String,
    /// The currency to quote it in; empty asks the source to decide.
    pub currency: String,
    /// The source name; empty means the registry's default (`yahoo`).
    pub source: Option<String>,
    /// `YYYY-MM-DD` for a dated quote; `None` for the newest.
    pub date: Option<String>,
}

/// What became of one request — see the module docs on [`fetch`]'s shape.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PriceFetch {
    /// The quote that request produced.
    Quote(PriceQuote),
    /// Why that request produced none.
    Failed(PriceFailure),
}

/// A request that produced no quote, kept per-request so a row can show its own
/// error while the rows around it show prices.
#[derive(Debug, Clone, Serialize, Type)]
pub struct PriceFailure {
    /// The requested commodity, as asked for.
    pub commodity: String,
    /// The requested currency, as asked for — possibly empty.
    pub currency: String,
    /// The source that was asked, as resolved (the registry default when the
    /// request named none).
    pub source: String,
    /// What went wrong, ready to show.
    pub message: String,
}

/// The sources this build can fetch from: upstream's registry plus TEFAS.
pub fn sources() -> Vec<PriceSourceInfo> {
    let registry = PriceSourceRegistry::new(&PriceConfig::default());
    let mut list: Vec<PriceSourceInfo> = registry
        .list_sources()
        .into_iter()
        .filter_map(|name| {
            let source = registry.get(name)?;
            Some(PriceSourceInfo {
                name: name.to_string(),
                description: describe(&source),
                official: is_official(name),
            })
        })
        .collect();
    list.push(PriceSourceInfo {
        name: tefas::NAME.to_string(),
        description: tefas::DESCRIPTION.to_string(),
        official: false,
    });
    list.sort_by(|left, right| left.name.cmp(&right.name));
    list
}

/// A source's description, with the API key it needs named when the key is not
/// in the environment — otherwise picking it just fails at fetch time with no
/// hint in the picker.
fn describe(source: &Arc<dyn PriceSource>) -> String {
    let mut description = source.description().to_string();
    if source.requires_api_key()
        && let Some(variable) = source.api_key_env_var()
        && std::env::var_os(variable).is_none()
    {
        description.push_str(&format!(" — needs an API key in `{variable}`"));
    }
    description
}

/// Every commodity the loaded ledger holds or quotes, one row each, with the
/// newest `price` directive for it if the ledger has one. Nothing is fetched.
pub fn targets(state: &AppState) -> Vec<PriceTarget> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let Some(loaded) = guard.as_ref() else {
        return Vec::new();
    };
    targets_of(&loaded.ledger)
}

/// [`targets`] over a ledger already in hand.
fn targets_of(ledger: &Ledger) -> Vec<PriceTarget> {
    let discovered = discover(&ledger);
    let mut commodities: BTreeSet<String> = BTreeSet::new();
    let mut marked: HashSet<String> = HashSet::new();
    let mut newest: BTreeMap<String, (NaiveDate, String, String)> = BTreeMap::new();

    commodities.extend(ledger.options.operating_currency.iter().cloned());
    // Every directive that names a commodity, so the row exists whether the
    // ledger *holds* the thing, *quotes* it, or merely declares it.
    for spanned in &ledger.directives {
        match &spanned.value {
            Directive::Price(price) => {
                let commodity = price.currency.to_string();
                commodities.insert(commodity.clone());
                commodities.insert(price.amount.currency.to_string());
                // Ties go to the later directive in the file, the same way a
                // reader of the journal would take it.
                let newer = newest
                    .get(&commodity)
                    .is_none_or(|(date, _, _)| *date <= price.date);
                if newer {
                    newest.insert(
                        commodity,
                        (
                            price.date,
                            price.amount.currency.to_string(),
                            price.amount.number.to_string(),
                        ),
                    );
                }
            }
            Directive::Commodity(commodity) => {
                commodities.insert(commodity.currency.to_string());
                if meta_marks_fund(&commodity.meta) {
                    marked.insert(commodity.currency.to_string());
                }
            }
            Directive::Open(open) => {
                commodities.extend(open.currencies.iter().map(ToString::to_string));
            }
            Directive::Transaction(transaction) => {
                for posting in &transaction.postings {
                    match &posting.value.units {
                        Some(rustledger_core::IncompleteAmount::Complete(amount)) => {
                            commodities.insert(amount.currency.to_string());
                        }
                        Some(rustledger_core::IncompleteAmount::CurrencyOnly(currency)) => {
                            commodities.insert(currency.to_string());
                        }
                        Some(rustledger_core::IncompleteAmount::NumberOnly(_)) | None => {}
                    }
                }
            }
            _ => {}
        }
    }

    commodities
        .into_iter()
        .map(|commodity| {
            let latest = newest
                .get(&commodity)
                .map(|(date, currency, price)| PriceQuote {
                    commodity: commodity.clone(),
                    currency: currency.clone(),
                    date: date.to_string(),
                    price: price.clone(),
                    // A quote read back from the ledger was written by someone or
                    // something else; naming a source here would be a guess.
                    source: "ledger".to_string(),
                });
            let info = discovered.get(&commodity);
            let currency = latest
                .as_ref()
                .map(|quote| quote.currency.clone())
                .or_else(|| info.and_then(Discovered::quote_currency))
                .or_else(|| ledger.options.operating_currency.first().cloned());
            let kind = kind_of(&commodity, info, marked.contains(&commodity)).to_string();
            PriceTarget {
                commodity,
                currency,
                kind,
                latest,
            }
        })
        .collect()
}

/// The discovery pass over a ledger, with the activity filter off so a
/// commodity the register does not currently hold is still discoverable, and
/// the ticker-shape heuristic off so nothing is invented.
fn discover(ledger: &Ledger) -> HashMap<String, Discovered> {
    discover_symbols(
        &ledger.directives,
        &ledger.options,
        true,
        false,
        None,
        &HashMap::new(),
    )
    .into_iter()
    .map(|(commodity, info)| (commodity, Discovered { info }))
    .collect()
}

/// A discovered commodity. A newtype around upstream's shape so the ledger-side
/// reading (what the metadata says) stays separable from what is fetched.
struct Discovered {
    info: DiscoveredCommodity,
}

impl Discovered {
    /// The quote currency the ledger declares for it, if it declares one.
    fn quote_currency(&self) -> Option<String> {
        self.info.quote_currency.clone().or_else(|| {
            self.info
                .quote_specs
                .first()
                .map(|spec| spec.quote_currency.clone())
        })
    }
}

/// `"currency"`, `"fund"` or `"commodity"` — see the module docs.
fn kind_of(commodity: &str, info: Option<&Discovered>, marked_fund: bool) -> &'static str {
    if marked_fund || info.is_some_and(Discovered::priced_by_tefas) {
        return "fund";
    }
    if is_fiat(commodity) {
        return "currency";
    }
    "commodity"
}

impl Discovered {
    /// Whether the ledger routes this commodity's prices through TEFAS —
    /// `price: "TRY:tefas/AFA"` is the ledger saying "this is a fund".
    fn priced_by_tefas(&self) -> bool {
        fn uses(mapping: &CommodityMapping, source: &str) -> bool {
            match mapping {
                CommodityMapping::Simple(_) => false,
                CommodityMapping::Detailed(detailed) => match &detailed.source {
                    SourceRef::Single(name) => name == source,
                    SourceRef::Fallback(entries) => {
                        entries.iter().any(|entry| entry.source_name() == source)
                    }
                },
            }
        }
        self.info
            .mapping
            .as_ref()
            .is_some_and(|mapping| uses(mapping, tefas::NAME))
            || self.info.quote_specs.iter().any(|spec| {
                spec.mapping
                    .as_ref()
                    .is_some_and(|mapping| uses(mapping, tefas::NAME))
            })
    }
}

/// Whether a commodity directive's metadata says "this is a fund". The keys are
/// the ones ledgers in the wild use; `FUND` may arrive as a bare word (which the
/// parser classifies as a currency, not a string) or as a quoted string.
fn meta_marks_fund(meta: &rustledger_core::Metadata) -> bool {
    const KEYS: [&str; 5] = [
        "asset-class",
        "asset_class",
        "type",
        "category",
        "instrument",
    ];
    meta.iter().any(|(key, value)| {
        let key = key.to_ascii_lowercase();
        if !KEYS.contains(&key.as_str()) {
            return false;
        }
        let word = match value {
            MetaValue::String(text) => text.clone(),
            MetaValue::Currency(code) => code.to_string(),
            _ => return false,
        };
        word.to_ascii_uppercase().contains("FUND")
    })
}

/// ISO-4217 fiat codes — the currencies a ledger spends and converts between.
/// The precious-metal codes (`XAU`, `XAG`, `XPT`, `XPD`) are deliberately
/// absent: they are in the standard, but in a ledger they are held and priced
/// like a commodity, so classifying them as currencies would put them at the
/// wrong end of a conversion.
fn is_fiat(code: &str) -> bool {
    const FIAT: &str = "\
AED AFN ALL AMD ANG AOA ARS AUD AWG AZN BAM BBD BDT BGN BHD BIF BMD BND BOB \
BOV BRL BSD BTN BWP BYN BZD CAD CDF CHE CHF CHW CLF CLP CNY COP COU CRC CUC \
CUP CVE CZK DJF DKK DOP DZD EGP ERN ETB EUR FJD FKP GBP GEL GHS GIP GMD GNF \
GTQ GYD HKD HNL HRK HTG HUF IDR ILS INR IQD IRR ISK JMD JOD JPY KES KGS KHR \
KMF KPW KRW KWD KYD KZT LAK LBP LKR LRD LSL LYD MAD MDL MGA MKD MMK MNT MOP \
MRU MUR MVR MWK MXN MXV MYR MZN NAD NGN NIO NOK NPR NZD OMR PAB PEN PGK PHP \
PKR PLN PYG QAR RON RSD RUB RWF SAR SBD SCR SDG SEK SGD SHP SLE SLL SOS SRD \
SSP STN SVC SYP SZL THB TJS TMT TND TOP TRY TTD TWD TZS UAH UGX USD USN UYI \
UYU UYW UZS VED VES VND VUV WST XAF XCD XDR XOF XPF XSU XTS XUA XXX YER ZAR \
ZMW ZWL";
    FIAT.split(' ').any(|fiat| fiat == code)
}

/// Fetch the requested quotes.
///
/// Blocking: call it from a worker thread, never the async runtime's. One
/// outcome per request, in request order; `Err` only when every request failed
/// (see the module docs). The ledger is read once, under the read lock, for the
/// `price:`/`quote_currency:` metadata that names a commodity's ticker — the
/// lock is released before the first request goes out.
pub fn fetch(state: &AppState, requests: Vec<QuoteRequest>) -> Result<Vec<PriceFetch>, String> {
    let mapping = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        guard
            .as_ref()
            .map(|loaded| ledger_mapping(&loaded.ledger))
            .unwrap_or_default()
    };
    fetch_with_mapping(&mapping, requests)
}

/// The `commodity → (source, ticker, quote currency)` table a ledger's metadata
/// declares, in the shape upstream's own resolution takes. A commodity with no
/// metadata simply has no entry, and its ticker is its own code.
fn ledger_mapping(ledger: &Ledger) -> HashMap<String, CommodityMapping> {
    discover(ledger)
        .into_iter()
        .filter_map(|(commodity, found)| found.info.mapping.map(|mapping| (commodity, mapping)))
        .collect()
}

/// [`fetch`] with the mapping already in hand — the seam the tests drive, since
/// no test may touch the network.
fn fetch_with_mapping(
    mapping: &HashMap<String, CommodityMapping>,
    requests: Vec<QuoteRequest>,
) -> Result<Vec<PriceFetch>, String> {
    let registry = PriceSourceRegistry::new(&PriceConfig::default());
    let tefas = tefas::source(TEFAS_TIMEOUT);
    let mut cache = PriceCache::load(CACHE_TTL_SECS);
    let mut outcomes = Vec::with_capacity(requests.len());
    let mut failures = Vec::new();

    for request in requests {
        match quote_for(&registry, &tefas, &mut cache, mapping, &request) {
            Ok(quote) => outcomes.push(PriceFetch::Quote(quote)),
            Err(failure) => {
                failures.push(failure.render());
                outcomes.push(PriceFetch::Failed(failure));
            }
        }
    }
    cache.save();

    if outcomes
        .iter()
        .all(|outcome| matches!(outcome, PriceFetch::Failed(_)))
        && !outcomes.is_empty()
    {
        return Err(failures.join("\n"));
    }
    Ok(outcomes)
}

impl PriceFailure {
    /// The failure as one line, for the `Err` arm of [`fetch`].
    fn render(&self) -> String {
        let currency = if self.currency.is_empty() {
            "the source's currency"
        } else {
            self.currency.as_str()
        };
        format!(
            "{} in {currency} from {}: {}",
            self.commodity, self.source, self.message
        )
    }
}

/// One request, one quote — or the failure that stopped it.
///
/// The order is deliberate: validate the request, resolve the source and the
/// ticker, consult the cache, and only then go to the network, so a request
/// that cannot succeed never reaches a provider.
fn quote_for(
    registry: &PriceSourceRegistry,
    tefas: &Arc<dyn PriceSource>,
    cache: &mut PriceCache,
    mapping: &HashMap<String, CommodityMapping>,
    request: &QuoteRequest,
) -> Result<PriceQuote, PriceFailure> {
    let commodity = request.commodity.trim().to_string();
    let requested_currency = request.currency.trim().to_string();
    let source_name = request
        .source
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| registry.default_source_name())
        .to_string();

    let failure = |message: String| PriceFailure {
        commodity: commodity.clone(),
        currency: requested_currency.clone(),
        source: source_name.clone(),
        message,
    };

    if commodity.is_empty() {
        return Err(failure("no commodity to price".to_string()));
    }
    let source = if source_name == tefas::NAME {
        Arc::clone(tefas)
    } else {
        registry
            .get(&source_name)
            .ok_or_else(|| failure(format!("no price source named `{source_name}`")))?
    };

    // An empty currency asks the source to decide. Only a source with one
    // published quote currency can answer that; the rest are told so instead of
    // being handed a currency nobody asked for.
    let currency = if requested_currency.is_empty() {
        fixed_quote_currency(&source_name)
            .ok_or_else(|| {
                failure(format!(
                    "`{source_name}` cannot decide the quote currency; name the currency for `{commodity}`"
                ))
            })?
            .to_string()
    } else {
        requested_currency.clone()
    };

    let date = match request
        .date
        .as_deref()
        .map(str::trim)
        .filter(|date| !date.is_empty())
    {
        Some(date) => Some(
            date.parse::<NaiveDate>()
                .map_err(|error| failure(format!("`{date}` is not a YYYY-MM-DD date: {error}")))?,
        ),
        None => None,
    };

    let ticker = ticker_for(
        mapping.get(&commodity),
        &source_name,
        registry.default_source_name(),
        &commodity,
    );
    let key = cache_key(&source_name, &ticker, &currency, date);
    if let Some(cached) = cache.get(&key) {
        return Ok(quote_of(&commodity, &cached));
    }

    let response = source
        .fetch_price(&PriceRequest {
            ticker: ticker.clone(),
            currency: currency.clone(),
            date,
        })
        .map_err(|error| failure(format!("{error:#}")))?;
    cache.insert(&key, &response);
    Ok(quote_of(&commodity, &response))
}

/// The fetched response as one of our quotes.
fn quote_of(commodity: &str, response: &PriceResponse) -> PriceQuote {
    PriceQuote {
        commodity: commodity.to_string(),
        currency: response.currency.clone(),
        date: response.date.to_string(),
        price: response.price.to_string(),
        source: response.source.clone(),
    }
}

/// The ticker a source is asked for: the ledger's own ticker for that source
/// when its metadata names one (`price: "USD:yahoo/BRK-B"` prices `BRK_B` as
/// `BRK-B`), and the commodity code itself otherwise.
///
/// Mirrors upstream's resolution rather than calling it, because the request
/// names one source and upstream's resolution answers with a whole attempt
/// chain. A `Simple` mapping is a ticker for the *default* source only, which is
/// what upstream reads it as too.
fn ticker_for(
    mapping: Option<&CommodityMapping>,
    source: &str,
    default_source: &str,
    commodity: &str,
) -> String {
    let own_code = || commodity.to_string();
    let Some(mapping) = mapping else {
        return own_code();
    };
    match mapping {
        CommodityMapping::Simple(ticker) => {
            if source == default_source {
                ticker.clone()
            } else {
                own_code()
            }
        }
        CommodityMapping::Detailed(detailed) => match &detailed.source {
            SourceRef::Single(name) => {
                if name == source {
                    detailed.ticker.clone().unwrap_or_else(own_code)
                } else {
                    own_code()
                }
            }
            SourceRef::Fallback(entries) => entries
                .iter()
                .find(|entry| entry.source_name() == source)
                .and_then(|entry| {
                    entry
                        .ticker()
                        .map(str::to_string)
                        .or_else(|| detailed.ticker.clone())
                })
                .unwrap_or_else(own_code),
        },
    }
}

/// Write `quotes` into the journal file `file` as `price` directives, through
/// the app's write pipeline (see the module docs).
///
/// `file` is the path [`journal_files`](crate::commands::journal_files) hands
/// out — the loaded ledger's own spelling of the file — and a path the journal
/// does not hold is refused rather than written to. Quotes the file already
/// states verbatim, and repeats inside `quotes` itself, are skipped, so writing
/// the same fetch twice is a no-op.
pub fn write_to_file(state: &AppState, file: &str, quotes: &[PriceQuote]) -> Result<usize, String> {
    let (previous, priced) = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().ok_or("no ledger loaded")?;
        let source = source_of_file(&loaded.ledger, file)?;
        (source.source.to_string(), priced_directives(&loaded.ledger))
    };
    let lines = price_lines(quotes, &priced)?;
    if lines.is_empty() {
        // Every quote is already a directive in the ledger; a second fetch of
        // the same day changes nothing — and says so, rather than reporting a
        // count nothing verified.
        return Ok(0);
    }
    edit_named_file_source(state, file, &with_appended(&previous, &lines), None)?;
    Ok(lines.lines().count())
}

/// The `price` directives the ledger already carries, as
/// `(commodity, date, currency, amount)` — the whole line, so a quote is skipped
/// only when the file already says exactly what it would add.
fn priced_directives(ledger: &Ledger) -> HashSet<(String, String, String, Decimal)> {
    ledger
        .directives
        .iter()
        .filter_map(|spanned| match &spanned.value {
            Directive::Price(price) => Some((
                price.currency.to_string(),
                price.date.to_string(),
                price.amount.currency.to_string(),
                price.amount.number,
            )),
            _ => None,
        })
        .collect()
}

/// The `price` lines for the quotes the ledger does not already carry, in the
/// order they were handed in. Everything that ends up in the ledger as syntax is
/// checked here first: a quote comes off the network, so none of it is trusted
/// as ledger text.
fn price_lines(
    quotes: &[PriceQuote],
    priced: &HashSet<(String, String, String, Decimal)>,
) -> Result<String, String> {
    let mut lines = String::new();
    let mut seen: HashSet<(String, String, String, Decimal)> = HashSet::new();
    for quote in quotes {
        let commodity = checked_commodity("commodity", &quote.commodity)?;
        let currency = checked_commodity("currency", &quote.currency)?;
        let date = checked_price_date(&quote.date)?;
        let price = checked_price(&quote.price)?;
        // A line that is already in the ledger, or already in this batch, is
        // not added twice. A *different* price for the same day is: that is a
        // quote that moved, and dropping it silently would leave the ledger
        // saying something nobody chose.
        let line = (commodity.clone(), date.clone(), currency.clone(), price);
        if priced.contains(&line) || !seen.insert(line) {
            continue;
        }
        lines.push_str(&date);
        lines.push_str(" price ");
        lines.push_str(&commodity);
        lines.push(' ');
        lines.push_str(&price.to_string());
        lines.push(' ');
        lines.push_str(&currency);
        lines.push('\n');
    }
    Ok(lines)
}

/// A beancount commodity or currency code: uppercase letters, digits and
/// `' . _ -`, at most 24 bytes, opening and closing on an alphanumeric. Codes
/// arrive from providers in mixed case, so the checked form is uppercased —
/// which is the only spelling the ledger's lexer accepts.
fn checked_commodity(kind: &str, value: &str) -> Result<String, String> {
    let code = value.trim().to_uppercase();
    let shaped = !code.is_empty()
        && code.len() <= 24
        && code.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '\'' | '.' | '_' | '-'))
        && code
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_alphanumeric());
    if !shaped {
        return Err(format!("`{value}` is not a beancount {kind} code"));
    }
    Ok(code)
}

/// A `YYYY-MM-DD` date. The day's existence is deliberately left to the
/// loader: it is the authority on what a legal ledger holds, and a refusal
/// there puts the file back byte for byte.
fn checked_price_date(value: &str) -> Result<String, String> {
    let date = value.trim();
    let shaped = date.len() == 10
        && date.bytes().enumerate().all(|(index, byte)| match index {
            4 | 7 => byte == b'-',
            _ => byte.is_ascii_digit(),
        });
    if !shaped {
        return Err(format!("`{value}` is not a YYYY-MM-DD date"));
    }
    Ok(date.to_string())
}

/// A price as an exact decimal — parsed, never floated, and refused unless
/// positive: a non-positive price is a broken feed, not a bargain.
fn checked_price(value: &str) -> Result<Decimal, String> {
    let price = Decimal::from_str(value.trim())
        .map_err(|error| format!("`{value}` is not a decimal price: {error}"))?;
    if price <= Decimal::ZERO {
        return Err(format!("`{value}` is not a positive price"));
    }
    Ok(price)
}

/// `text` with `lines` appended, one blank line apart — the same rule
/// `editing`'s own appends follow, so a quote lands in the file the way a
/// hand-written entry does.
fn with_appended(text: &str, lines: &str) -> String {
    let mut rewritten = String::with_capacity(text.len() + lines.len() + 2);
    rewritten.push_str(text);
    if !text.is_empty() {
        if !text.ends_with('\n') {
            rewritten.push_str("\n\n");
        } else if !text.ends_with("\n\n") {
            rewritten.push('\n');
        }
    }
    rewritten.push_str(lines);
    rewritten
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TempDir, loaded_state};
    use std::fs;

    /// A ledger holding a currency (TRY, the operating one), another currency a
    /// commodity is priced in (USD), a commodity both priced and held (GOOG), a
    /// commodity priced twice (AAPL — the newest must win), and a fund the
    /// ledger marks through TEFAS metadata (AFA).
    const FIXTURE: &str = "\
option \"operating_currency\" \"TRY\"

2026-01-01 open Assets:Bank:TRY TRY
2026-01-01 open Assets:Broker:USD USD
2026-01-01 open Assets:Broker:Equity
2026-01-01 open Assets:Fund:AFA AFA

2026-01-01 commodity AFA
  price: \"TRY:tefas/AFA\"

2026-01-02 * \"Move cash\"
  Assets:Bank:TRY      -100.00 TRY
  Assets:Broker:USD     100.00 TRY

2026-01-03 price AAPL 180.00 USD
2026-01-05 price AAPL 190.00 USD
2026-01-06 price EUR 35.00 TRY

2026-01-07 * \"Buy shares\"
  Assets:Broker:Equity      2 GOOG @ 140.00 USD
  Assets:Bank:TRY      -280.00 USD

2026-01-09 price GOOG 145.00 USD
";

    fn fixture() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new("prices");
        let path = dir.file("main.bean");
        fs::write(&path, FIXTURE).unwrap();
        (dir, path)
    }

    fn target<'a>(targets: &'a [PriceTarget], commodity: &str) -> &'a PriceTarget {
        targets
            .iter()
            .find(|target| target.commodity == commodity)
            .unwrap_or_else(|| panic!("no target for {commodity}"))
    }

    #[test]
    fn targets_cover_every_commodity_the_ledger_holds_or_quotes() {
        let (_dir, path) = fixture();
        let state = loaded_state(&path);
        let targets = targets(&state);

        let try_row = target(&targets, "TRY");
        assert_eq!(try_row.kind, "currency");
        assert_eq!(try_row.currency.as_deref(), Some("TRY"));
        assert!(try_row.latest.is_none(), "TRY is quoted, never priced");

        let usd_row = target(&targets, "USD");
        assert_eq!(usd_row.kind, "currency");
        assert!(usd_row.latest.is_none());

        // A currency with a price directive carries that directive's currency.
        let eur_row = target(&targets, "EUR");
        assert_eq!(eur_row.kind, "currency");
        assert_eq!(eur_row.currency.as_deref(), Some("TRY"));
        let latest = eur_row.latest.as_ref().expect("EUR is priced");
        assert_eq!(latest.date, "2026-01-06");
        assert_eq!(latest.price, "35.00");
        assert_eq!(latest.currency, "TRY");
        assert_eq!(latest.source, "ledger");
    }

    #[test]
    fn targets_take_the_newest_price_directive_and_fall_back_to_the_operating_currency() {
        let (_dir, path) = fixture();
        let state = loaded_state(&path);
        let targets = targets(&state);

        let gold = target(&targets, "GOOG");
        assert_eq!(gold.kind, "commodity");
        assert_eq!(gold.currency.as_deref(), Some("USD"));
        let latest = gold.latest.as_ref().expect("GOOG is priced");
        assert_eq!(latest.date, "2026-01-09");
        assert_eq!(latest.price, "145.00");

        let apple = target(&targets, "AAPL");
        assert_eq!(apple.kind, "commodity");
        let latest = apple.latest.as_ref().expect("AAPL is priced twice");
        assert_eq!(latest.date, "2026-01-05", "the newest directive wins");
        assert_eq!(latest.price, "190.00");
        assert_eq!(latest.currency, "USD");

        // The fund: marked by `price: "TRY:tefas/AFA"`, never priced yet, so
        // the currency comes from the metadata and the quote falls back to the
        // ledger's operating currency.
        let fund = target(&targets, "AFA");
        assert_eq!(fund.kind, "fund");
        assert_eq!(fund.currency.as_deref(), Some("TRY"));
        assert!(fund.latest.is_none());
    }

    #[test]
    fn a_ledger_with_no_operating_currency_leaves_the_currency_unset() {
        let dir = TempDir::new("prices-bare");
        let path = dir.file("main.bean");
        fs::write(&path, "2026-01-01 commodity VTI\n").unwrap();
        let state = loaded_state(&path);
        let targets = targets(&state);
        let vti = target(&targets, "VTI");
        assert_eq!(vti.kind, "commodity");
        assert_eq!(vti.currency, None, "nothing in the ledger names a currency");
        assert!(vti.latest.is_none());
    }

    fn quote(commodity: &str, currency: &str, date: &str, price: &str) -> PriceQuote {
        PriceQuote {
            commodity: commodity.to_string(),
            currency: currency.to_string(),
            date: date.to_string(),
            price: price.to_string(),
            source: "tefas".to_string(),
        }
    }

    #[test]
    fn write_to_file_appends_price_lines_once() {
        let dir = TempDir::new("prices-write");
        let path = dir.file("main.bean");
        fs::write(
            &path,
            "option \"operating_currency\" \"TRY\"\n\n2026-01-01 open Assets:Bank:TRY TRY\n\n\
             2026-01-05 price AAPL 190.00 USD\n",
        )
        .unwrap();
        let state = loaded_state(&path);
        let file = path.display().to_string();

        let quotes = [
            quote("AFA", "TRY", "2026-10-28", "1.322508"),
            // Already priced that day: probing again must not duplicate it.
            quote("AAPL", "USD", "2026-01-05", "190.00"),
            quote("AAPL", "USD", "2026-10-28", "191.10"),
        ];
        let appended = write_to_file(&state, &file, &quotes).expect("the quotes are writable");
        // Two of the three are new: the one the ledger already prices is skipped.
        assert_eq!(appended, 2);

        let text = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            &lines[lines.len() - 2..],
            &[
                "2026-10-28 price AFA 1.322508 TRY",
                "2026-10-28 price AAPL 191.10 USD"
            ]
        );
        assert_eq!(
            text.matches("2026-01-05 price AAPL").count(),
            1,
            "the ledger's own AAPL price is still there exactly once"
        );

        // A second run with the same quotes writes nothing at all.
        write_to_file(&state, &file, &quotes).expect("a second write is a no-op");
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn a_changed_price_for_a_day_the_ledger_already_prices_is_written() {
        let dir = TempDir::new("prices-update");
        let path = dir.file("main.bean");
        fs::write(&path, "2026-10-28 price AAPL 190.00 USD\n").unwrap();
        let state = loaded_state(&path);
        let file = path.display().to_string();

        let quotes = [quote("AAPL", "USD", "2026-10-28", "191.10")];
        write_to_file(&state, &file, &quotes).expect("a moved quote is writable");

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("2026-10-28 price AAPL 190.00 USD"), "{text}");
        assert!(text.contains("2026-10-28 price AAPL 191.10 USD"), "{text}");

        // The appended line makes the next identical write a no-op.
        write_to_file(&state, &file, &quotes).expect("the second write is a no-op");
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn a_quote_the_loader_refuses_leaves_the_file_untouched() {
        let dir = TempDir::new("prices-refused");
        let path = dir.file("main.bean");
        let before = "option \"operating_currency\" \"TRY\"\n\n\
                      2026-01-01 open Assets:Bank:TRY TRY\n";
        fs::write(&path, before).unwrap();
        let state = loaded_state(&path);

        // The shape is a date; the day is not one. Nothing here checks the
        // calendar — the reload is the authority, and it must put the bytes back.
        let quotes = [quote("AFA", "TRY", "2026-02-30", "1.322508")];
        let refused = write_to_file(&state, &path.display().to_string(), &quotes);
        assert!(refused.is_err(), "the loader must refuse an impossible day");
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn a_quote_whose_price_is_not_a_positive_decimal_never_reaches_the_file() {
        let dir = TempDir::new("prices-garbage");
        let path = dir.file("main.bean");
        let before = "2026-01-01 open Assets:Bank:TRY TRY\n";
        fs::write(&path, before).unwrap();
        let state = loaded_state(&path);

        for broken in ["", "1,322508", "-1.32", "0"] {
            let quotes = [quote("AFA", "TRY", "2026-10-28", broken)];
            let result = write_to_file(&state, &path.display().to_string(), &quotes);
            assert!(result.is_err(), "`{broken}` is not a price");
            assert_eq!(fs::read_to_string(&path).unwrap(), before);
        }
    }

    #[test]
    fn a_file_the_journal_does_not_hold_is_refused() {
        let (_dir, path) = fixture();
        let state = loaded_state(&path);
        let quotes = [quote("AFA", "TRY", "2026-10-28", "1.322508")];
        let result = write_to_file(&state, "/nowhere/else.bean", &quotes);
        assert!(result.is_err());
    }

    #[test]
    fn commodity_codes_are_checked_and_uppercased() {
        assert_eq!(checked_commodity("currency", "try").unwrap(), "TRY");
        assert_eq!(checked_commodity("commodity", " BRK-B ").unwrap(), "BRK-B");
        for bad in ["", "1USD", "US D", "USD.", "BTC/ETH"] {
            assert!(
                checked_commodity("commodity", bad).is_err(),
                "`{bad}` is not a beancount code"
            );
        }
    }

    #[test]
    fn the_ledger_ticker_wins_over_the_commodity_code() {
        let detailed = CommodityMapping::Detailed(rustledger::config::DetailedMapping {
            source: SourceRef::Single("yahoo".to_string()),
            ticker: Some("BRK-B".to_string()),
            quote_currency: None,
        });
        assert_eq!(
            ticker_for(Some(&detailed), "yahoo", "yahoo", "BRK_B"),
            "BRK-B"
        );
        assert_eq!(
            ticker_for(Some(&detailed), "ecb", "yahoo", "BRK_B"),
            "BRK_B",
            "a mapping for another source leaves the code alone"
        );

        // A fallback chain carries a ticker per source.
        let chained = CommodityMapping::Detailed(rustledger::config::DetailedMapping {
            source: SourceRef::Fallback(vec![
                rustledger::config::FallbackEntry::Name("ecb".to_string()),
                rustledger::config::FallbackEntry::Detailed(rustledger::config::FallbackDetail {
                    source: "yahoo".to_string(),
                    ticker: Some("EURUSD=X".to_string()),
                }),
            ]),
            ticker: Some("EUR".to_string()),
            quote_currency: Some("TRY".to_string()),
        });
        assert_eq!(
            ticker_for(Some(&chained), "yahoo", "yahoo", "EUR"),
            "EURUSD=X"
        );
        assert_eq!(
            ticker_for(Some(&chained), "ecb", "yahoo", "EUR"),
            "EUR",
            "a bare chain entry inherits the parent ticker"
        );

        // `Simple` is a ticker for the default source only.
        let simple = CommodityMapping::Simple("BTC-USD".to_string());
        assert_eq!(
            ticker_for(Some(&simple), "yahoo", "yahoo", "BTC"),
            "BTC-USD"
        );
        assert_eq!(ticker_for(Some(&simple), "coinbase", "yahoo", "BTC"), "BTC");
        assert_eq!(ticker_for(None, "yahoo", "yahoo", "BTC"), "BTC");
    }

    #[test]
    fn kinds_follow_the_ledger_and_the_currency_table() {
        assert!(is_fiat("TRY") && is_fiat("USD") && is_fiat("EUR"));
        assert!(!is_fiat("BTC") && !is_fiat("AFA") && !is_fiat("XAU"));
    }

    #[test]
    fn a_source_with_one_currency_answers_for_an_empty_one_and_the_rest_refuse() {
        assert_eq!(fixed_quote_currency("tefas"), Some("TRY"));
        assert_eq!(fixed_quote_currency("yahoo"), None);
        assert!(!is_official("tefas") && !is_official("yahoo") && is_official("ecb"));
    }

    #[test]
    fn sources_are_lowercase_named_and_list_tefas_as_unofficial() {
        let list = sources();
        let names: Vec<&str> = list.iter().map(|source| source.name.as_str()).collect();
        assert!(
            names.windows(2).all(|pair| pair[0] < pair[1]),
            "the list is sorted and free of duplicates: {names:?}"
        );
        assert!(
            names
                .iter()
                .all(|name| name.chars().all(|c| c.is_ascii_lowercase() || c == '_')),
            "names stay stable, lowercase identifiers: {names:?}"
        );
        let tefas = list
            .iter()
            .find(|source| source.name == "tefas")
            .expect("the TEFAS source is listed");
        assert!(!tefas.official);
        assert!(tefas.description.contains("TRY"), "{}", tefas.description);
        for official in ["ecb", "ratesapi", "coinbase"] {
            let source = list
                .iter()
                .find(|source| source.name == official)
                .unwrap_or_else(|| panic!("{official} is listed"));
            assert!(source.official, "{official} serves a documented API");
        }
        let yahoo = list
            .iter()
            .find(|source| source.name == "yahoo")
            .expect("yahoo");
        assert!(!yahoo.official, "yahoo is an endpoint read off the site");
    }

    #[test]
    fn fetch_with_no_ledger_and_no_requests_is_an_empty_answer() {
        // No ledger loaded: the mapping is empty, and an empty batch asks
        // nothing of any provider.
        let state = AppState {
            loaded: std::sync::RwLock::new(None),
        };
        let outcomes = fetch(&state, Vec::new()).expect("an empty batch is not a failure");
        assert!(outcomes.is_empty());
    }

    #[test]
    fn an_empty_request_list_is_an_empty_answer() {
        let result = fetch_with_mapping(&HashMap::new(), Vec::new());
        assert!(matches!(result, Ok(outcomes) if outcomes.is_empty()));
    }

    #[test]
    fn a_request_that_cannot_succeed_fails_before_any_network() {
        // No source by that name, an empty currency with no source of one, and
        // a date that is not one: all three are answered from the request
        // alone, so this runs offline.
        let error = fetch_with_mapping(
            &HashMap::new(),
            vec![
                QuoteRequest {
                    commodity: "AAPL".to_string(),
                    currency: "USD".to_string(),
                    source: Some("nowhere".to_string()),
                    date: None,
                },
                QuoteRequest {
                    commodity: "AAPL".to_string(),
                    currency: String::new(),
                    source: Some("yahoo".to_string()),
                    date: None,
                },
                QuoteRequest {
                    commodity: "AAPL".to_string(),
                    currency: "USD".to_string(),
                    source: Some("yahoo".to_string()),
                    date: Some("last tuesday".to_string()),
                },
            ],
        )
        .expect_err("every request failed, so the batch is an error");
        assert!(error.contains("no price source named `nowhere`"), "{error}");
        assert!(
            error.contains("cannot decide the quote currency"),
            "{error}"
        );
        assert!(error.contains("is not a YYYY-MM-DD date"), "{error}");
    }
}
