// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The ledger's envelope budget: the one surface that *reads* the accounts the
//! envelope plugin writes (`plugins/rustledger-envelope`) instead of hiding
//! them. The plugin owns the arithmetic; this module only reports it per month.

use std::collections::{BTreeMap, BTreeSet};

use rustledger_core::{Decimal, Directive, IncompleteAmount, MetaValue};
use rustledger_loader::Ledger;

use crate::model::{BudgetEnvelope, BudgetGroup, BudgetMonth};
use crate::session::AppState;

/// The prefix every account the envelope plugin generates lives under.
const ENVELOPE_NAMESPACE: &str = "Equity:Envelope";

/// The envelope plugin's own plumbing accounts, mirroring its
/// `accounts.rs` (`TO_BE_BUDGETED` and `MONEY`).
///
/// `To-Be-Budgeted` is the pool of money the configuration has not assigned to
/// an envelope — the budget screen's own headline — and `Money` is the contra of
/// the ledger's on-budget money, which the plugin reads to keep the pool's
/// figure real. Neither is an envelope a user funded, so neither becomes a
/// group: the pool is reported as [`BudgetMonth::to_budget`] and the contra is
/// left out of the month's lines.
const ENVELOPE_POOL: &str = "Equity:Envelope:To-Be-Budgeted";
const ENVELOPE_MONEY: &str = "Equity:Envelope:Money";

/// What the budget says about a ledger the plugin has not budgeted: an empty
/// screen would read as "this month is worth nothing", which is a different
/// statement from "nothing was written".
const NO_ENVELOPE_BUDGET: &str = "this ledger has no envelope budget data — the envelope plugin has written nothing under `Equity:Envelope:`, so there is no month to show";

/// Whether `account` is one the envelope plugin generated.
fn is_envelope_account(account: &str) -> bool {
    account == ENVELOPE_NAMESPACE
        || (account.len() > ENVELOPE_NAMESPACE.len()
            && account.starts_with(ENVELOPE_NAMESPACE)
            && account.as_bytes()[ENVELOPE_NAMESPACE.len()] == b':')
}

/// Whether `text` is a `YYYY-MM` month label — the budget's unit of time, the
/// same one the plugin's `month.rs` writes.
fn is_month_label(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 7
        && bytes[4] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && matches!(
            (bytes[5], bytes[6]),
            (b'0', b'1'..=b'9') | (b'1', b'0'..=b'2')
        )
}

/// What one envelope did in one month.
#[derive(Clone, Copy, Default)]
struct EnvelopeFlow {
    /// Postings the month moved into the envelope, net of the sweep's
    /// corrections.
    budgeted: Decimal,
    /// Postings the month spent out of it, as a positive figure.
    spent: Decimal,
}

/// The commodity the budget reports in: the ledger's operating currency when the
/// plugin budgeted in it, else the commodity the most money moved in (ties go to
/// the later name, so the payload does not wobble between loads of one ledger).
fn budget_currency(held: &BTreeMap<String, Decimal>, operating: &[String]) -> Option<String> {
    if let Some(currency) = operating
        .iter()
        .find(|currency| held.contains_key(*currency))
    {
        return Some(currency.clone());
    }
    let mut best: Option<(&String, Decimal)> = None;
    for (currency, total) in held {
        if best.is_none_or(|(_, highest)| total.abs() > highest) {
            best = Some((currency, total.abs()));
        }
    }
    best.map(|(currency, _)| currency.clone())
}

/// A group while it is being collected: its envelopes and the totals they add
/// up to, so the totals never have to be read back out of the rendered strings.
#[derive(Default)]
struct BudgetGroupTotals {
    envelopes: Vec<BudgetEnvelope>,
    budgeted: Decimal,
    spent: Decimal,
    balance: Decimal,
}

/// The ledger's envelope postings, summed in one walk and narrowed to the
/// commodity the payload reports in.
///
/// A month used to be read straight off the ledger, which made the range screen
/// — six months plus its probe — clone the ledger and re-add every posting once
/// per month. Collecting first makes a window cost the same one walk a single
/// month costs; [`EnvelopeFlows::render`] then draws any number of months from
/// these totals.
struct EnvelopeFlows {
    /// The commodity every figure is in: [`budget_currency`]'s pick.
    currency: String,
    /// Every month the plugin wrote in that commodity, oldest first.
    months: BTreeSet<String>,
    /// An account's months' flows, in month order: its balance at a month is
    /// the running sum of them.
    accounts: BTreeMap<String, BTreeMap<String, EnvelopeFlow>>,
}

/// Every posting inside the envelope namespace, summed per account, month and
/// commodity, then narrowed to the commodity the payload reports in.
///
/// The per-commodity sums survive the narrowing because the reported commodity
/// — the ledger's operating currency when the plugin budgeted in it, else the
/// one the most money moved in — is only knowable once the walk is done.
///
/// The only failure is [`NO_ENVELOPE_BUDGET`]. A currency is picked only from
/// the per-commodity totals, and each of those comes from a posting that also
/// wrote a flow, so a collected set always has at least one month.
fn collect_envelope_flows(ledger: &Ledger) -> Result<EnvelopeFlows, String> {
    // Every posting inside the namespace, summed per account, month and
    // commodity. The per-commodity totals pick the one to report in: the plugin
    // tracks each commodity separately, and the budget screen shows one.
    let mut flows: BTreeMap<(String, String, String), EnvelopeFlow> = BTreeMap::new();
    let mut held: BTreeMap<String, Decimal> = BTreeMap::new();
    for directive in ledger.balance_view() {
        let Directive::Transaction(tx) = directive else {
            continue;
        };
        let date = tx.date.to_string();
        // The plugin dates every entry the month's last day, so the date says
        // which month an entry belongs to (`model.rs`: `let date =
        // month.last_day()`).
        let Some(entry_month) = date.get(0..7).filter(|text| is_month_label(text)) else {
            continue;
        };
        let kind = match tx.meta.get("envelope-kind") {
            Some(MetaValue::String(kind)) => Some(kind.as_str()),
            _ => None,
        };
        for posting in &tx.postings {
            let account = posting.value.account.as_str();
            if !is_envelope_account(account) {
                continue;
            }
            let Some(IncompleteAmount::Complete(amount)) = &posting.value.units else {
                // Residual/interpolated postings never occur post-booked.
                continue;
            };
            let commodity = amount.currency.as_str().to_owned();
            *held.entry(commodity.clone()).or_default() += amount.number.abs();
            let flow = flows
                .entry((account.to_owned(), entry_month.to_owned(), commodity))
                .or_default();
            let spending = match kind {
                Some("Envelope activity") => true,
                Some("Envelope funding" | "Envelope adjustment" | "Envelope rollover") => false,
                // No kind at all, or the backing entry, which only ever touches
                // the pool and the contra.
                _ => amount.number.is_sign_negative(),
            };
            if spending {
                flow.spent -= amount.number;
            } else {
                flow.budgeted += amount.number;
            }
        }
    }
    let Some(currency) = budget_currency(&held, &ledger.options.operating_currency) else {
        return Err(NO_ENVELOPE_BUDGET.to_string());
    };
    // Narrow to the reported commodity once, so a render walks one account's
    // months without stepping over the commodities the screen never shows. Each
    // account's months stay in order, so a month past the requested one ends
    // its history.
    let mut months = BTreeSet::new();
    let mut accounts: BTreeMap<String, BTreeMap<String, EnvelopeFlow>> = BTreeMap::new();
    for ((account, entry_month, commodity), flow) in flows {
        if commodity != currency {
            continue;
        }
        months.insert(entry_month.clone());
        accounts
            .entry(account)
            .or_default()
            .insert(entry_month, flow);
    }
    // The currency came from a commodity total, and a commodity total only
    // exists because a posting wrote a flow: the narrowed set has a month.
    debug_assert!(!months.is_empty());
    Ok(EnvelopeFlows {
        currency,
        months,
        accounts,
    })
}

impl EnvelopeFlows {
    /// One month of the ledger's envelope budget, drawn from the collected
    /// flows.
    ///
    /// The plugin (`plugins/rustledger-envelope`) is the only writer of the
    /// `Equity:Envelope:` namespace and the owner of its arithmetic. For every
    /// budget month it emits one entry per step — the backing contra
    /// (`Equity:Envelope:Money` against `To-Be-Budgeted`), the inherited
    /// funding, the month's mirrored activity, the
    /// `adjust`/`zero`/`pad` reconciliation and the rollover sweep — each dated
    /// the month's last day, tagged `#envelope` and carrying
    /// `envelope-kind`/`envelope-month` metadata (see its `emit.rs`). Every
    /// posting stays inside the namespace, so the budget is one prefix away
    /// from the rest of the app.
    ///
    /// This is the one surface that *reads* those accounts instead of hiding
    /// them (see [`is_envelope_account`]): a month's flows are the postings
    /// dated inside it, an envelope's balance is every posting up to and
    /// including the requested month — the same subtree total the plugin's
    /// month-end assertions measure — and the pool's balance is the screen's
    /// `to_budget`.
    ///
    /// `envelope-kind` decides what a posting is: activity is spending, funding,
    /// adjustments and the sweep are everything else. Without the metadata — a
    /// hand-written entry in the namespace — the posting's sign decides, which is
    /// what the plugin's own funding/activity postings imply (money leaving an
    /// envelope is spending).
    ///
    /// `month` is a `YYYY-MM` label the caller has validated, or `None` for the
    /// newest month with data; a month outside the data (a gap, or before the
    /// first month) reports what stood at its end: the carries, with nothing
    /// moved in it.
    fn render(&self, month: Option<&str>) -> BudgetMonth {
        // `collect_envelope_flows` answers nothing when the namespace is empty,
        // so a collected set always names its newest month; the fallback keeps
        // the renderer total instead of panicking on a broken invariant.
        let requested = month
            .map(str::to_owned)
            .unwrap_or_else(|| self.months.iter().next_back().cloned().unwrap_or_default());

        let mut groups: BTreeMap<String, BudgetGroupTotals> = BTreeMap::new();
        let mut to_budget = Decimal::ZERO;
        for (account, history) in &self.accounts {
            // `Equity:Envelope:<path>`; the namespace itself holds nothing.
            let Some(path) = account
                .strip_prefix(ENVELOPE_NAMESPACE)
                .and_then(|tail| tail.strip_prefix(':'))
            else {
                continue;
            };
            let mut budgeted = Decimal::ZERO;
            let mut spent = Decimal::ZERO;
            let mut balance = Decimal::ZERO;
            let mut exists = false;
            for (entry_month, flow) in history {
                if entry_month.as_str() > requested.as_str() {
                    break;
                }
                exists = true;
                if *entry_month == requested {
                    budgeted = flow.budgeted;
                    spent = flow.spent;
                }
                balance += flow.budgeted - flow.spent;
            }
            if account.as_str() == ENVELOPE_POOL {
                to_budget = balance;
                continue;
            }
            // The contra of the on-budget money is not an envelope; a group for
            // it would read as a category holding the user's whole bank
            // balance.
            if !exists || account.as_str() == ENVELOPE_MONEY {
                continue;
            }
            let (group_name, name) = match path.split_once(':') {
                Some((group_name, name)) => (group_name.to_owned(), name.to_owned()),
                None => (path.to_owned(), path.to_owned()),
            };
            let envelope = BudgetEnvelope {
                name,
                full_name: account.clone(),
                budgeted: budgeted.to_string(),
                spent: spent.to_string(),
                balance: balance.to_string(),
            };
            let group = groups.entry(group_name).or_default();
            group.budgeted += budgeted;
            group.spent += spent;
            group.balance += balance;
            group.envelopes.push(envelope);
        }
        let groups: Vec<BudgetGroup> = groups
            .into_iter()
            .map(|(name, totals)| BudgetGroup {
                name,
                budgeted: totals.budgeted.to_string(),
                spent: totals.spent.to_string(),
                balance: totals.balance.to_string(),
                envelopes: totals.envelopes,
            })
            .collect();
        BudgetMonth {
            month: requested,
            months: self.months.iter().cloned().collect(),
            groups,
            to_budget: to_budget.to_string(),
            currency: self.currency.clone(),
        }
    }
}

/// One month of the ledger's envelope budget.
///
/// [`EnvelopeFlows::render`] carries the payload's rules; this is the reading
/// side's entry point: it validates the label and gives the ledger to
/// [`collect_envelope_flows`] for the one walk a month — or a range of them —
/// costs.
pub(crate) fn budget_month(ledger: &Ledger, month: Option<&str>) -> Result<BudgetMonth, String> {
    if let Some(text) = month
        && !is_month_label(text)
    {
        return Err(format!("`{text}` is not a budget month in YYYY-MM form"));
    }
    Ok(collect_envelope_flows(ledger)?.render(month))
}

/// [`budget_month`] against the held ledger, so the command and the tests ask
/// the same question the screen does.
pub(crate) fn budget_view(state: &AppState, month: Option<&str>) -> Result<BudgetMonth, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger loaded")?;
    budget_month(&loaded.ledger, month)
}

/// How many months a range covers when the caller names neither end: the six
/// months ending at the ledger's newest budget month — half a year, the window
/// the multi-month screen opens on.
const DEFAULT_RANGE_MONTHS: i32 = 6;

/// The widest window a caller may ask for. Ten years is longer than any budget
/// screen and short enough that one request cannot become ten thousand.
const MAX_RANGE_MONTHS: usize = 120;

/// Several consecutive months of the ledger's envelope budget.
///
/// The single-month [`BudgetMonth`] is the screen's *one* page; this is the
/// scroll: the same figures, month by month, so a chart can draw them and the
/// table can page without asking the backend per month. It is deliberately not
/// a second reader of the ledger: [`budget_range_of`] renders every month of
/// the window from the same one-walk flow set [`budget_month`] renders its
/// month from, and only sums what came back, so the two surfaces cannot
/// disagree about a month they both show.
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct BudgetRange {
    /// The first month in the window, `YYYY-MM`.
    pub from: String,
    /// The last month in the window, `YYYY-MM`.
    pub to: String,
    /// The commodity every figure is in, as [`BudgetMonth::currency`].
    pub currency: String,
    /// The window's months, oldest first, with no gaps: a month the plugin did
    /// not budget reports what stood at its end, exactly as
    /// [`budget_month`]'s own month-outside-the-data rule does.
    pub months: Vec<BudgetRangeMonth>,
}

/// One month of a [`BudgetRange`]: the single-month payload's groups, plus the
/// month's own totals so a chart can draw a series without re-adding groups in
/// the frontend.
///
/// The three totals are the group totals summed with the same exact decimal
/// arithmetic the single-month payload uses (never floats, never re-parsed from
/// a rounded figure); `to_budget` is passed through from
/// [`BudgetMonth::to_budget`].
#[derive(Debug, serde::Serialize, specta::Type)]
pub struct BudgetRangeMonth {
    /// `YYYY-MM`.
    pub month: String,
    /// `Equity:Envelope:To-Be-Budgeted` at the month's end.
    pub to_budget: String,
    /// Every group's `budgeted` added up — the month's funding over all
    /// envelopes, net of the rollover's corrections.
    pub budgeted: String,
    /// Every group's `spent` added up, as a positive figure.
    pub spent: String,
    /// Every group's `balance` added up — what the envelopes held at the
    /// month's end.
    pub balance: String,
    /// The month's groups, alphabetically, shaped exactly as
    /// [`BudgetMonth::groups`] is.
    pub groups: Vec<BudgetGroup>,
}

/// [`budget_month`] for a window of months against the held ledger.
///
/// Both ends are optional. With neither named the window is the last
/// [`DEFAULT_RANGE_MONTHS`] months of the ledger's own data (see
/// [`budget_range_of`]); either end named, the other still defaults against the
/// ledger's newest budget month. A ledger the plugin has not budgeted at all is
/// an empty window rather than an error — "nothing is budgeted" is a state the
/// screen shows, not a failure it reports.
pub fn budget_range(
    state: &AppState,
    from: Option<String>,
    to: Option<String>,
) -> Result<BudgetRange, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger loaded")?;
    budget_range_of(&loaded.ledger, from, to)
}

/// The window itself, once the ledger is in hand.
///
/// The default window ends at the newest month the plugin wrote and reaches
/// back at most [`DEFAULT_RANGE_MONTHS`] months, never before the first month
/// it wrote: a ledger two months old shows those two months, not four empty
/// ones. An explicit `to` before the ledger's first month therefore clips
/// `from` up to it — the window still ends where the caller asked.
fn budget_range_of(
    ledger: &Ledger,
    from: Option<String>,
    to: Option<String>,
) -> Result<BudgetRange, String> {
    // Labels are checked before anything else so a typo is refused even when
    // the ledger has no budget to show.
    for label in [&from, &to].into_iter().flatten() {
        if !is_month_label(label) {
            return Err(format!("`{label}` is not a budget month in YYYY-MM form"));
        }
    }
    // One walk of the ledger for the whole window: the probe, the window's
    // ends and every month below all draw on these flows.
    let flows = match collect_envelope_flows(ledger) {
        Ok(flows) => flows,
        Err(error) if error == NO_ENVELOPE_BUDGET => {
            // Nothing to show, but the window still names what was asked for:
            // the caller renders its empty state over that.
            return Ok(BudgetRange {
                from: from.unwrap_or_default(),
                to: to.unwrap_or_default(),
                currency: ledger
                    .options
                    .operating_currency
                    .first()
                    .cloned()
                    .unwrap_or_default(),
                months: Vec::new(),
            });
        }
        Err(error) => return Err(error),
    };
    let probe = flows.render(None);
    let to = to.unwrap_or_else(|| probe.month.clone());
    let to_index =
        month_index(&to).ok_or_else(|| format!("`{to}` is not a budget month in YYYY-MM form"))?;
    let from = match from {
        Some(from) => from,
        None => {
            let earliest = probe
                .months
                .first()
                .and_then(|label| month_index(label))
                .unwrap_or(to_index);
            month_label(
                (to_index - DEFAULT_RANGE_MONTHS + 1)
                    .max(earliest)
                    .min(to_index),
            )
        }
    };
    let from_index = month_index(&from)
        .ok_or_else(|| format!("`{from}` is not a budget month in YYYY-MM form"))?;
    if from_index > to_index {
        return Err(format!("the budget range `{from}`–`{to}` runs backwards"));
    }
    // A window is a screen. An unbounded range — a tool call, a hand-written URL —
    // would render one month payload per month of recorded history, so it is
    // refused by name rather than clamped: a silent short answer would look
    // like data.
    let span = (to_index - from_index + 1) as usize;
    if span > MAX_RANGE_MONTHS {
        return Err(format!(
            "the budget range `{from}`–`{to}` spans {span} months; ask for at most {MAX_RANGE_MONTHS}"
        ));
    }
    let mut months = Vec::with_capacity(span);
    for index in from_index..=to_index {
        let label = month_label(index);
        // The same render the single-month payload answers with, so a month
        // cannot differ between the two surfaces.
        let month = flows.render(Some(&label));
        let mut budgeted = Decimal::ZERO;
        let mut spent = Decimal::ZERO;
        let mut balance = Decimal::ZERO;
        for group in &month.groups {
            budgeted += decimal(&group.budgeted)?;
            spent += decimal(&group.spent)?;
            balance += decimal(&group.balance)?;
        }
        months.push(BudgetRangeMonth {
            month: label,
            to_budget: month.to_budget,
            budgeted: budgeted.to_string(),
            spent: spent.to_string(),
            balance: balance.to_string(),
            groups: month.groups,
        });
    }
    Ok(BudgetRange {
        from,
        to,
        currency: probe.currency,
        months,
    })
}

/// A payload figure as the exact decimal it was rendered from. The payload's
/// strings come from `Decimal::to_string`, so this only fails on a bug — never
/// on ledger data.
fn decimal(text: &str) -> Result<Decimal, String> {
    text.parse::<Decimal>()
        .map_err(|_| format!("`{text}` is not a decimal figure"))
}

/// A `YYYY-MM` label as months since year zero, so months can be counted and
/// stepped without a date library.
fn month_index(label: &str) -> Option<i32> {
    if !is_month_label(label) {
        return None;
    }
    let year: i32 = label.get(0..4)?.parse().ok()?;
    let month: i32 = label.get(5..7)?.parse().ok()?;
    Some(year * 12 + month - 1)
}

/// [`month_index`] back to the label the plugin writes.
fn month_label(index: i32) -> String {
    format!(
        "{:04}-{:02}",
        index.div_euclid(12),
        index.rem_euclid(12) + 1
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TempDir, loaded_state};
    use std::fs;

    /// A group's own label with its totals, then one row per envelope — the
    /// shape the budget screen renders, as comparable strings.
    fn group_rows(group: &BudgetGroup) -> Vec<(&str, &str, &str, &str)> {
        let mut rows = vec![(
            group.name.as_str(),
            group.budgeted.as_str(),
            group.spent.as_str(),
            group.balance.as_str(),
        )];
        rows.extend(group.envelopes.iter().map(|envelope| {
            (
                envelope.name.as_str(),
                envelope.budgeted.as_str(),
                envelope.spent.as_str(),
                envelope.balance.as_str(),
            )
        }));
        rows
    }

    /// Every group's rows, as the screen reads them.
    fn budget_rows(month: &BudgetMonth) -> Vec<Vec<(&str, &str, &str, &str)>> {
        month.groups.iter().map(group_rows).collect()
    }

    /// A ledger holding what the envelope plugin writes for two months, in the
    /// shape `emit.rs` gives it: one entry per step of the month, dated the
    /// month's last day, tagged `#envelope`, with `envelope-kind` and
    /// `envelope-month` metadata, every posting inside `Equity:Envelope:`.
    ///
    /// Food is funded 300 a month (plus a 20 adjustment in January), and its
    /// spending is recorded on `Expenses:Food:Bread`, so the funding lands on
    /// the parent envelope while the activity lands on the child — the case a
    /// group total exists for. Fun is reset by its rollover at the end of
    /// February, which sweeps its 80 balance back to the pool.
    fn envelope_fixture() -> &'static str {
        "\
option \"operating_currency\" \"TRY\"

2026-01-01 open Assets:Bank
2026-01-01 open Expenses:Food
2026-01-01 open Expenses:Food:Bread
2026-01-01 open Expenses:Fun
2026-01-01 open Equity:Envelope:Food
2026-01-01 open Equity:Envelope:Food:Bread
2026-01-01 open Equity:Envelope:Fun
2026-01-01 open Equity:Envelope:Money
2026-01-01 open Equity:Envelope:To-Be-Budgeted

2026-01-15 * \"Bakery\"
  Assets:Bank                     -50.00 TRY
  Expenses:Food:Bread              50.00 TRY

2026-01-31 * \"Envelope backing 2026-01\" #envelope
  envelope-kind: \"Envelope backing\"
  envelope-month: \"2026-01\"
  Equity:Envelope:Money         -1000.00 TRY
  Equity:Envelope:To-Be-Budgeted 1000.00 TRY

2026-01-31 * \"Envelope funding 2026-01\" #envelope
  envelope-kind: \"Envelope funding\"
  envelope-month: \"2026-01\"
  Equity:Envelope:Food             300.00 TRY
  Equity:Envelope:Fun              200.00 TRY
  Equity:Envelope:To-Be-Budgeted  -500.00 TRY

2026-01-31 * \"Envelope activity 2026-01\" #envelope
  envelope-kind: \"Envelope activity\"
  envelope-month: \"2026-01\"
  Equity:Envelope:Food:Bread       -50.00 TRY
  Equity:Envelope:Fun              -30.00 TRY
  Equity:Envelope:To-Be-Budgeted    80.00 TRY

2026-01-31 * \"Envelope adjustment 2026-01\" #envelope
  envelope-kind: \"Envelope adjustment\"
  envelope-month: \"2026-01\"
  Equity:Envelope:Food              20.00 TRY
  Equity:Envelope:To-Be-Budgeted   -20.00 TRY

2026-02-28 * \"Envelope funding 2026-02\" #envelope
  envelope-kind: \"Envelope funding\"
  envelope-month: \"2026-02\"
  Equity:Envelope:Food             300.00 TRY
  Equity:Envelope:Fun              200.00 TRY
  Equity:Envelope:To-Be-Budgeted  -500.00 TRY

2026-02-28 * \"Envelope activity 2026-02\" #envelope
  envelope-kind: \"Envelope activity\"
  envelope-month: \"2026-02\"
  Equity:Envelope:Food:Bread       -80.00 TRY
  Equity:Envelope:Fun             -120.00 TRY
  Equity:Envelope:To-Be-Budgeted   200.00 TRY

2026-02-28 * \"Envelope rollover 2026-02\" #envelope
  envelope-kind: \"Envelope rollover\"
  envelope-month: \"2026-02\"
  Equity:Envelope:Fun              -80.00 TRY
  Equity:Envelope:To-Be-Budgeted    80.00 TRY
"
    }

    /// A ledger of `ledger` opened as the app's state.
    fn state_of(dir: &TempDir, name: &str, ledger: &str) -> AppState {
        let path = dir.file(name);
        fs::write(&path, ledger).unwrap();
        loaded_state(&path)
    }

    /// The budget opens on the newest month the plugin budgeted, and reads the
    /// month's figures per envelope and per group, with the pool of unassigned
    /// money underneath.
    #[test]
    fn budget_reads_the_newest_envelope_month() {
        let dir = TempDir::new("budget-month");
        let state = state_of(&dir, "main.bean", envelope_fixture());
        let month = budget_view(&state, None).unwrap();

        assert_eq!(month.month, "2026-02");
        assert_eq!(month.months, ["2026-01", "2026-02"]);
        assert_eq!(month.currency, "TRY");
        // The pool: 1000 backed in, 500 + 500 funded, 80 + 120 + 200 spent
        // activity back, the adjustment's 20 and the sweep's 80 out.
        assert_eq!(month.to_budget, "340.00");
        assert_eq!(
            budget_rows(&month),
            vec![
                vec![
                    // The group's own envelope holds the funding, its child the
                    // spending, so only the group row is the whole story.
                    ("Food", "300.00", "80.00", "490.00"),
                    ("Food", "300.00", "0", "620.00"),
                    ("Bread", "0", "80.00", "-130.00"),
                ],
                vec![
                    // The rollover's 80 goes back to the pool, so the month
                    // budgeted Fun 120 of its 200, not all of it.
                    ("Fun", "120.00", "120.00", "170.00"),
                    ("Fun", "120.00", "120.00", "170.00"),
                ],
            ]
        );
    }

    /// A requested month is read on its own terms: its own flows, and the
    /// balance every earlier month carries into it.
    #[test]
    fn budget_reads_any_budget_month() {
        let dir = TempDir::new("budget-any-month");
        let state = state_of(&dir, "main.bean", envelope_fixture());

        let january = budget_view(&state, Some("2026-01")).unwrap();
        assert_eq!(january.month, "2026-01");
        assert_eq!(january.months, ["2026-01", "2026-02"]);
        assert_eq!(january.to_budget, "560.00");
        assert_eq!(
            budget_rows(&january),
            vec![
                vec![
                    ("Food", "320.00", "50.00", "270.00"),
                    ("Food", "320.00", "0", "320.00"),
                    ("Bread", "0", "50.00", "-50.00"),
                ],
                vec![
                    ("Fun", "200.00", "30.00", "170.00"),
                    ("Fun", "200.00", "30.00", "170.00"),
                ],
            ]
        );

        // A month the plugin never budgeted carries the balances it ended with
        // and moves nothing of its own.
        let march = budget_view(&state, Some("2026-03")).unwrap();
        assert_eq!(march.month, "2026-03");
        assert_eq!(march.to_budget, "340.00");
        assert_eq!(
            budget_rows(&march),
            vec![
                vec![
                    ("Food", "0", "0", "490.00"),
                    ("Food", "0", "0", "620.00"),
                    ("Bread", "0", "0", "-130.00"),
                ],
                vec![("Fun", "0", "0", "170.00"), ("Fun", "0", "0", "170.00")],
            ]
        );

        // Before the first budget month there is nothing to carry: an empty
        // month rather than a made-up one.
        let before = budget_view(&state, Some("2025-12")).unwrap();
        assert!(before.groups.is_empty());
        assert_eq!(before.to_budget, "0");

        // A label that is not a month is refused rather than guessed at.
        for bad in ["2026-1", "2026-13", "junk", ""] {
            assert!(
                budget_view(&state, Some(bad)).is_err(),
                "`{bad}` must not read"
            );
        }
    }

    /// The budget's two refusals: nothing loaded, and a ledger the plugin has
    /// not budgeted — an empty month would claim something the ledger does not.
    #[test]
    fn budget_refuses_a_ledger_it_cannot_read() {
        assert_eq!(
            budget_view(&AppState::default(), None).unwrap_err(),
            "no ledger loaded"
        );

        let dir = TempDir::new("budget-none");
        let state = state_of(
            &dir,
            "main.bean",
            "option \"operating_currency\" \"USD\"\n\n2026-01-01 open Assets:Bank\n",
        );
        let error = budget_view(&state, None).unwrap_err();
        assert!(error.contains("Equity:Envelope:"), "{error}");
    }

    /// A hand-written entry in the namespace carries no metadata, so the sign of
    /// its postings decides what it is: money into an envelope is funding, money
    /// out of it is spending.
    #[test]
    fn budget_reads_envelope_entries_without_metadata() {
        let dir = TempDir::new("budget-plain");
        let state = state_of(
            &dir,
            "main.bean",
            "\
option \"operating_currency\" \"TRY\"

2026-01-01 open Assets:Bank
2026-01-01 open Expenses:Food
2026-01-01 open Equity:Envelope:Groceries
2026-01-01 open Equity:Envelope:To-Be-Budgeted

2026-01-10 * \"Groceries\"
  Assets:Bank                     -50.00 TRY
  Expenses:Food                    50.00 TRY

2026-01-31 * \"Envelope funding 2026-01\" #envelope
  Equity:Envelope:Groceries       100.00 TRY
  Equity:Envelope:To-Be-Budgeted -100.00 TRY

2026-01-31 * \"Envelope activity 2026-01\" #envelope
  Equity:Envelope:Groceries       -50.00 TRY
  Equity:Envelope:To-Be-Budgeted   50.00 TRY
",
        );
        let month = budget_view(&state, None).unwrap();
        assert_eq!(month.month, "2026-01");
        assert_eq!(month.months, ["2026-01"]);
        // No backing entry in this ledger, so the pool is short by the 50 that
        // was already spent when it was funded.
        assert_eq!(month.to_budget, "-50.00");
        assert_eq!(
            budget_rows(&month),
            vec![vec![
                ("Groceries", "100.00", "50.00", "50.00"),
                ("Groceries", "100.00", "50.00", "50.00"),
            ]]
        );
    }

    /// One month of a range, as the screen reads it: its label, its totals, and
    /// its groups in the single-month payload's own rows.
    fn range_row(month: &BudgetRangeMonth) -> (String, &str, &str, &str, &str) {
        (
            month.month.clone(),
            month.budgeted.as_str(),
            month.spent.as_str(),
            month.balance.as_str(),
            month.to_budget.as_str(),
        )
    }

    /// A range month's groups, as the single-month tests read theirs.
    fn range_groups(month: &BudgetRangeMonth) -> Vec<Vec<(&str, &str, &str, &str)>> {
        month.groups.iter().map(group_rows).collect()
    }

    /// A ledger the plugin budgeted for every month of `months`, funding one
    /// envelope `100 TRY` a month and spending nothing — enough to check which
    /// months a default window covers without repeating the two-month
    /// fixture's arithmetic.
    fn monthly_fixture(months: &[&str]) -> String {
        let mut ledger = String::from(
            "\
option \"operating_currency\" \"TRY\"

2025-01-01 open Assets:Bank
2025-01-01 open Equity:Envelope:Food
2025-01-01 open Equity:Envelope:To-Be-Budgeted
",
        );
        for month in months {
            ledger.push_str(&format!(
                "\n{month}-01 * \"Envelope funding {month}\" #envelope\n  envelope-kind: \"Envelope funding\"\n  envelope-month: \"{month}\"\n  Equity:Envelope:Food            100.00 TRY\n  Equity:Envelope:To-Be-Budgeted -100.00 TRY\n"
            ));
        }
        ledger
    }

    /// With no ends named, the window is the ledger's own last six months —
    /// oldest first, clipped to the data — and each month carries the same
    /// figures the single-month payload reports for it.
    #[test]
    fn budget_range_defaults_to_the_last_six_months_of_data() {
        let dir = TempDir::new("budget-range-default");
        let state = state_of(&dir, "main.bean", envelope_fixture());
        let range = budget_range(&state, None, None).unwrap();

        // The fixture spans two months; a shorter ledger must not be padded
        // back six.
        assert_eq!(range.from, "2026-01");
        assert_eq!(range.to, "2026-02");
        assert_eq!(range.currency, "TRY");
        assert_eq!(
            range.months.iter().map(range_row).collect::<Vec<_>>(),
            vec![
                ("2026-01".to_string(), "520.00", "80.00", "440.00", "560.00"),
                (
                    "2026-02".to_string(),
                    "420.00",
                    "200.00",
                    "660.00",
                    "340.00"
                ),
            ]
        );
        // The months are the single-month payload's, group for group.
        let february = budget_view(&state, Some("2026-02")).unwrap();
        assert_eq!(range_groups(&range.months[1]), budget_rows(&february));
    }

    /// Eight months of data: the default window keeps the newest six, and a
    /// window named by hand covers what was asked for, gaps included.
    #[test]
    fn budget_range_window_is_six_months_and_steppable() {
        let dir = TempDir::new("budget-range-six");
        let state = state_of(
            &dir,
            "main.bean",
            &monthly_fixture(&[
                "2025-08", "2025-09", "2025-10", "2025-11", "2025-12", "2026-01", "2026-02",
                "2026-03",
            ]),
        );

        let range = budget_range(&state, None, None).unwrap();
        let labels: Vec<&str> = range.months.iter().map(|m| m.month.as_str()).collect();
        assert_eq!(
            labels,
            [
                "2025-10", "2025-11", "2025-12", "2026-01", "2026-02", "2026-03"
            ]
        );
        assert_eq!(range.from, "2025-10");
        assert_eq!(range.to, "2026-03");
        // Eight fundings of 100, one a month: the window's last month stands on
        // all eight, the first on the three before it and its own.
        assert_eq!(range.months[5].balance, "800.00");
        assert_eq!(range.months[0].balance, "300.00");
        assert_eq!(range.months[0].budgeted, "100.00");

        // A hand-named window is honoured month by month, the oldest first.
        let january = budget_range(
            &state,
            Some("2025-12".to_string()),
            Some("2026-02".to_string()),
        )
        .unwrap();
        assert_eq!(january.from, "2025-12");
        assert_eq!(january.to, "2026-02");
        assert_eq!(
            january
                .months
                .iter()
                .map(|m| m.month.as_str())
                .collect::<Vec<_>>(),
            ["2025-12", "2026-01", "2026-02"]
        );

        // Only one end named: the other defaults to the ledger's newest month,
        // and a `to` before the data still yields the one month asked for.
        let from_only = budget_range(&state, Some("2026-02".to_string()), None).unwrap();
        assert_eq!(from_only.to, "2026-03");
        assert_eq!(from_only.months.len(), 2);
        let to_only = budget_range(&state, None, Some("2025-06".to_string())).unwrap();
        assert_eq!(to_only.from, "2025-06");
        assert_eq!(to_only.months.len(), 1);
        assert!(to_only.months[0].groups.is_empty());
        assert_eq!(to_only.months[0].budgeted, "0");
    }

    /// The range's two refusals: a backwards window and a label that is not a
    /// month, both answered before the ledger is read.
    #[test]
    fn budget_range_refuses_a_backwards_or_bad_window() {
        let dir = TempDir::new("budget-range-refusals");
        let state = state_of(&dir, "main.bean", envelope_fixture());

        let error = budget_range(
            &state,
            Some("2026-03".to_string()),
            Some("2026-01".to_string()),
        )
        .unwrap_err();
        assert!(error.contains("backwards"), "{error}");

        for (from, to) in [
            (Some("2026-1".to_string()), None),
            (None, Some("2026-13".to_string())),
            (Some("junk".to_string()), None),
        ] {
            let error = budget_range(&state, from, to).unwrap_err();
            assert!(error.contains("not a budget month"), "{error}");
        }

        assert_eq!(
            budget_range(&AppState::default(), None, None).unwrap_err(),
            "no ledger loaded"
        );
    }

    /// A ledger with no envelope budget answers an empty window, not an error:
    /// the screen shows "nothing budgeted" over the range it asked for.
    #[test]
    fn budget_range_is_empty_without_an_envelope_budget() {
        let dir = TempDir::new("budget-range-none");
        let state = state_of(
            &dir,
            "main.bean",
            "option \"operating_currency\" \"USD\"\n\n2026-01-01 open Assets:Bank\n",
        );
        let range = budget_range(
            &state,
            Some("2026-01".to_string()),
            Some("2026-03".to_string()),
        )
        .unwrap();
        assert!(range.months.is_empty());
        assert_eq!(range.from, "2026-01");
        assert_eq!(range.to, "2026-03");
        assert_eq!(range.currency, "USD");

        // Nothing named: the window has no end to default to, so it names none.
        let empty = budget_range(&state, None, None).unwrap();
        assert!(empty.months.is_empty());
        assert_eq!(empty.from, "");
        assert_eq!(empty.to, "");
    }

    /// A range is the single-month payload tiled, not a second arithmetic:
    /// every month it renders must be byte-identical to what [`budget_month`]
    /// answers for that label, totals included. The fixture skips February, so
    /// the month with nothing moved in it — the carries alone — is checked too.
    #[test]
    fn range_months_are_byte_identical_to_the_one_month_payload() {
        let dir = TempDir::new("budget-range-parity");
        let state = state_of(&dir, "main.bean", &monthly_fixture(&["2026-01", "2026-03"]));
        let range = budget_range(
            &state,
            Some("2026-01".to_string()),
            Some("2026-03".to_string()),
        )
        .unwrap();
        assert_eq!(
            range
                .months
                .iter()
                .map(|month| month.month.as_str())
                .collect::<Vec<_>>(),
            ["2026-01", "2026-02", "2026-03"]
        );

        for month in &range.months {
            let single = budget_view(&state, Some(&month.month)).unwrap();
            // The month's own payload: the groups serialize to the same bytes,
            // and the pool is the single month's own figure.
            assert_eq!(
                serde_json::to_string(&month.groups).unwrap(),
                serde_json::to_string(&single.groups).unwrap(),
                "{}",
                month.month
            );
            assert_eq!(month.to_budget, single.to_budget, "{}", month.month);
            // The range's totals are the single month's groups re-added with
            // the same exact decimal arithmetic — never a rerounded figure.
            let total = |field: fn(&BudgetGroup) -> &str| {
                single
                    .groups
                    .iter()
                    .map(|group| decimal(field(group)).unwrap())
                    .sum::<Decimal>()
                    .to_string()
            };
            assert_eq!(
                month.budgeted,
                total(|group| &group.budgeted),
                "{}",
                month.month
            );
            assert_eq!(month.spent, total(|group| &group.spent), "{}", month.month);
            assert_eq!(
                month.balance,
                total(|group| &group.balance),
                "{}",
                month.month
            );
        }

        // February had no entry: nothing budgeted or spent, January's 100
        // carried.
        assert_eq!(range.months[1].budgeted, "0");
        assert_eq!(range.months[1].spent, "0");
        assert_eq!(range.months[1].balance, "100.00");
    }
}
