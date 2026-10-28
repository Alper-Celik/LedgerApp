// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The monthly model: what the ledger implies for each budget month.
//!
//! # What a month contains
//!
//! For every month in the budget horizon the model produces a set of balanced
//! entries, all of whose postings live inside `Equity:Envelope:`. The
//! invariants that hold at every month boundary are:
//!
//! * every entry's postings sum to zero;
//! * `To-Be-Budgeted = on-budget money − Σ category balances`, per commodity;
//! * `Σ Equity:Envelope:* = 0`.
//!
//! # Category balances
//!
//! A posting on an expense account becomes a posting on its mirrored category
//! account ([`crate::accounts::category_account`]), and a category's *balance*
//! is the sum over its subtree — so a category funded at `Expenses:Groceries`
//! and spent at `Expenses:Groceries:Bread` reports one number.
//!
//! # Order of operations inside a month
//!
//! 1. [`EntryKind::Money`] — the contra of the month's change in on-budget
//!    money, which is what gives `To-Be-Budgeted` its real figure. The first
//!    month also carries the money held before the horizon, but only when
//!    `include starting balance` is set;
//! 2. [`EntryKind::Funding`] — inherited assignments (`fund`);
//! 3. [`EntryKind::Activity`] — the month's spending, mirrored;
//! 4. [`EntryKind::Adjustment`] — the user's one-off `adjust` deltas, then the
//!    `zero`/`pad` reconciliation, which therefore reconciles the balance *after*
//!    the month's spending;
//! 5. [`EntryKind::Sweep`] — the rollover policy applied at the month boundary.
//!
//! All of a month's entries are dated its last day and appear in that order, so
//! the month's state is exactly right at every boundary; within a month the
//! model is deliberately month-granular.
//!
//! Assertions ([`Assertion`]) are dated the **first day of the following
//! month**: beancount applies a `balance` at the start of its date, so an
//! assertion dated on the month's own last day would be evaluated before that
//! day's transactions. Dated a day later it sees the whole month, including the
//! sweep, which makes rustledger's own Late validation a check on the plugin's
//! arithmetic.
//!
//! # Currency
//!
//! Nothing is ever converted, and every figure is tracked per commodity, so a
//! category funded in TRY and spent in USD reports two balances and
//! `To-Be-Budgeted` can be short in one commodity while long in another.
//! Spending consumes a posting's weight (`units × cost`, else `units × price`,
//! else `units`) — what the expense cost; the money side measures what an
//! account *holds*, its units, so buying USD with TRY puts USD in the pool
//! rather than the TRY the USD cost.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use regex_lite::Regex;
use rust_decimal::Decimal;
use rustledger_plugin_types::{
    AmountData, CostNumberData, DirectiveData, DirectiveWrapper, PluginError, PostingData,
    PriceAnnotationView,
};

use crate::accounts::{self, MONEY, TO_BE_BUDGETED};
use crate::config::{Config, Rollover, Target};
use crate::inventory::Inventory;
use crate::money::Money;
use crate::month::Month;

/// Longest horizon the plugin will plan, so a typo'd `end date` cannot turn into
/// tens of thousands of generated directives.
const MAX_MONTHS: usize = 600;

/// The complete plan for the budget horizon.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    /// Accounts to open (every generated account, once).
    pub opens: Vec<Open>,
    /// One plan per budget month, in order.
    pub months: Vec<MonthPlan>,
    /// Warnings and errors for the host to report.
    pub diagnostics: Vec<PluginError>,
}

/// An account the plugin opens.
#[derive(Clone, Debug)]
pub struct Open {
    /// Date of the `open` directive.
    pub date: String,
    /// Account name.
    pub account: String,
}

/// One budget month.
#[derive(Clone, Debug)]
pub struct MonthPlan {
    /// The month.
    pub month: Month,
    /// Generated entries, in application order.
    pub entries: Vec<Entry>,
    /// Balance assertions for the end of the month.
    pub assertions: Vec<Assertion>,
}

/// Which step of a month an entry belongs to, in the order the steps apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// Contra of the change in on-budget money.
    Money,
    /// Inherited category assignments.
    Funding,
    /// The month's mirrored spending.
    Activity,
    /// One-off `adjust` / `zero` / `pad` operations.
    Adjustment,
    /// The rollover policy applied at the month boundary.
    Sweep,
}

impl EntryKind {
    /// The narration prefix used for this kind of entry.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Money => "Envelope backing",
            Self::Funding => "Envelope funding",
            Self::Adjustment => "Envelope adjustment",
            Self::Activity => "Envelope activity",
            Self::Sweep => "Envelope rollover",
        }
    }
}

/// One generated transaction.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Which step of the month this entry belongs to.
    pub kind: EntryKind,
    /// The transaction's date.
    pub date: String,
    /// The transaction's narration.
    pub narration: String,
    /// Postings, summing to zero.
    pub legs: Vec<Leg>,
}

/// One posting of a generated entry.
#[derive(Clone, Debug)]
pub struct Leg {
    /// Account.
    pub account: String,
    /// Signed amount in one commodity.
    pub amount: Money,
}

/// A balance assertion for a month's closing balances, dated the first day of
/// the following month so it sees the whole month, sweep included.
#[derive(Clone, Debug)]
pub struct Assertion {
    /// Date of the assertion.
    pub date: String,
    /// Account asserted.
    pub account: String,
    /// Expected balance.
    pub amount: Money,
}

/// One posting of the ledger, reduced to what budgeting needs: when, where, and
/// what it was worth in the commodity it was actually worth in.
#[derive(Clone, Debug)]
struct Movement {
    month: Month,
    account: String,
    /// What the account holds, in the commodity it holds it in: an FX purchase
    /// must show the currency it bought, not that currency's cost.
    units: Money,
    /// What the posting was worth, in the commodity it was worth in — its
    /// weight, which is what spending consumes. `None` when a cost or price
    /// makes that unreadable, in which case the posting is not an expense the
    /// budget can value.
    weight: Option<Money>,
    filename: Option<String>,
    lineno: Option<u32>,
}

/// A ledger location for a diagnostic: the transaction that caused it.
type SourceLocation = (Option<String>, Option<u32>);

/// Build the plan for `config` from the booked directive stream.
///
/// `directives` is the host's post-booking stream: transaction postings have
/// their amounts, costs and prices filled in, which is what the activity step
/// reads.
#[must_use]
pub fn plan(config: &Config, directives: &[DirectiveWrapper]) -> Plan {
    let mut diagnostics = Vec::new();

    let horizon = match horizon(config, directives) {
        Ok(horizon) => horizon,
        Err(problem) => {
            diagnostics.push(problem);
            return Plan {
                diagnostics,
                ..Plan::default()
            };
        }
    };

    let budget_accounts = compile(&config.budget_accounts);
    let mappings: Vec<(Regex, String)> = config
        .mappings
        .iter()
        .filter_map(|mapping| {
            Regex::new(&mapping.pattern)
                .ok()
                .map(|pattern| (pattern, mapping.node.clone()))
        })
        .collect();
    let movements = movements(directives);

    // Assignments grouped by category and commodity, so each month can pick the
    // newest declaration at or before it.
    let mut funds: BTreeMap<String, BTreeMap<String, Vec<(Month, Decimal)>>> = BTreeMap::new();
    for fund in &config.funds {
        funds
            .entry(fund.node.clone())
            .or_default()
            .entry(fund.amount.currency.clone())
            .or_default()
            .push((fund.month, fund.amount.value));
    }

    // Money on hand, per commodity: what arrived before the horizon (the
    // baseline), and what each month adds to it.
    let mut pre_horizon = Inventory::new();
    let mut per_month_money: BTreeMap<i32, Inventory> = BTreeMap::new();
    // Spending per month, per category account, and — only when `strict` needs
    // it — the location of the first posting that landed in each category, so a
    // warning can point back at the ledger without re-scanning (or re-matching)
    // every posting per category.
    let mut per_month_activity: BTreeMap<i32, BTreeMap<String, Inventory>> = BTreeMap::new();
    let mut per_month_locations: BTreeMap<i32, BTreeMap<String, SourceLocation>> = BTreeMap::new();

    for movement in &movements {
        if accounts::is_generated(&movement.account) {
            continue;
        }
        if let Some(category) = category_for(config, &mappings, &movement.account) {
            if config.strict {
                per_month_locations
                    .entry(movement.month.index())
                    .or_default()
                    .entry(category.clone())
                    .or_insert_with(|| (movement.filename.clone(), movement.lineno));
            }
            let Some(weight) = &movement.weight else {
                continue;
            };
            let month = per_month_activity
                .entry(movement.month.index())
                .or_default();
            month
                .entry(category)
                .or_default()
                .add(&weight.currency, weight.value);
            continue;
        }
        if matches_any(budget_accounts.as_slice(), &movement.account) {
            let units = &movement.units;
            if movement.month < config.start {
                pre_horizon.add(&units.currency, units.value);
            } else if movement.month <= *horizon.last().expect("horizon is never empty") {
                per_month_money
                    .entry(movement.month.index())
                    .or_default()
                    .add(&units.currency, units.value);
            }
        }
    }

    let mut tbb = Inventory::new();
    let mut categories: BTreeMap<String, Inventory> = BTreeMap::new();
    let mut generated: BTreeSet<String> = BTreeSet::new();
    generated.insert(MONEY.to_string());
    generated.insert(TO_BE_BUDGETED.to_string());

    // Rollover policies are declared per category: iterate the declared nodes in
    // name order and resolve each month's policy from them.
    let rollover_nodes: BTreeSet<&str> = config
        .rollovers
        .iter()
        .map(|declaration| declaration.node.as_str())
        .collect();

    let mut months = Vec::with_capacity(horizon.len());
    for (position, &month) in horizon.iter().enumerate() {
        let date = month.last_day();
        let mut plan = MonthPlan {
            month,
            entries: Vec::new(),
            assertions: Vec::new(),
        };

        // 1. The month's change in on-budget money, mirrored as the contra the
        //    pool is backed by. The first month also carries what was held
        //    before the horizon, when the configuration counts it.
        let mut delta = per_month_money
            .get(&month.index())
            .cloned()
            .unwrap_or_default();
        if position == 0 && config.include_starting_balance {
            delta.add_inventory(&pre_horizon);
        }
        if !delta.is_empty() {
            let mut legs = Vec::new();
            push_against_pool(&mut legs, MONEY, &delta.negated());
            tbb.add_inventory(&delta);
            plan.entries.push(Entry {
                kind: EntryKind::Money,
                date: date.clone(),
                narration: format!("{} {}", EntryKind::Money.label(), month.label()),
                legs,
            });
        }

        // 2. Assignments, inherited from the newest `fund` at or before this
        //    month. A category with no assignment in force gets nothing.
        let mut funding = Vec::new();
        let mut assigned: BTreeSet<String> = BTreeSet::new();
        for (node, commodity_funds) in &funds {
            let mut amounts = Inventory::new();
            for (commodity, declarations) in commodity_funds {
                if let Some((_, amount)) = declarations
                    .iter()
                    .filter(|(from, _)| *from <= month)
                    .max_by_key(|(from, _)| from.index())
                {
                    amounts.add(commodity, *amount);
                }
            }
            if amounts.is_empty() {
                continue;
            }
            let account = match accounts::category_account(node, &config.expense_root) {
                Some(account) => account,
                None => continue,
            };
            push_against_pool(&mut funding, &account, &amounts);
            categories
                .entry(account.clone())
                .or_default()
                .add_inventory(&amounts);
            tbb.add_inventory(&amounts.negated());
            assigned.insert(account);
        }
        if !funding.is_empty() {
            generated.extend(funding.iter().map(|leg| leg.account.clone()));
            plan.entries.push(Entry {
                kind: EntryKind::Funding,
                date: date.clone(),
                narration: format!("{} {}", EntryKind::Funding.label(), month.label()),
                legs: funding,
            });
        }

        // 3. The month's spending, mirrored: the category gives the money back
        //    to the pool, which is what `balance = budgeted − spent` means once
        //    the money side has already moved the pool.
        let mut activity = Vec::new();
        if let Some(spending) = per_month_activity.remove(&month.index()) {
            for (account, spent) in spending {
                if config.strict && !assigned.contains(&account) {
                    // The first posting that landed in the category this month,
                    // recorded during the scan; a transaction with no source
                    // location reports the warning without one.
                    let location = per_month_locations
                        .get(&month.index())
                        .and_then(|locations| locations.get(&account))
                        .and_then(|location| match location {
                            (Some(file), Some(line)) => Some((file.clone(), *line)),
                            _ => None,
                        });
                    let mut problem = PluginError::warning(format!(
                        "{}: spending in {account} with no assignment",
                        month.label()
                    ));
                    if let Some((file, line)) = location {
                        problem = problem.at(file, line);
                    }
                    diagnostics.push(problem);
                }
                push_against_pool(&mut activity, &account, &spent.negated());
                categories
                    .entry(account)
                    .or_default()
                    .add_inventory(&spent.negated());
                tbb.add_inventory(&spent);
            }
        }
        if !activity.is_empty() {
            generated.extend(activity.iter().map(|leg| leg.account.clone()));
            plan.entries.push(Entry {
                kind: EntryKind::Activity,
                date: date.clone(),
                narration: format!("{} {}", EntryKind::Activity.label(), month.label()),
                legs: activity,
            });
        }

        // 4. One-off deltas, then reconciliation, both against the balance as it
        //    stands after the month's spending.
        let mut adjustment = Vec::new();
        for declaration in config.adjustments.iter().filter(|it| it.month == month) {
            let account = match accounts::category_account(&declaration.node, &config.expense_root)
            {
                Some(account) => account,
                None => continue,
            };
            let mut amounts = Inventory::new();
            amounts.add(&declaration.amount.currency, declaration.amount.value);
            push_against_pool(&mut adjustment, &account, &amounts);
            categories
                .entry(account)
                .or_default()
                .add_inventory(&amounts);
            tbb.add_inventory(&amounts.negated());
        }
        for target in config.targets.iter().filter(|it| target_month(it) == month) {
            match target {
                Target::Zero { node, .. } => {
                    let account = match accounts::category_account(node, &config.expense_root) {
                        Some(account) => account,
                        None => continue,
                    };
                    let current = subtree_balance(&categories, node, &config.expense_root);
                    if current.is_empty() {
                        continue;
                    }
                    push_against_pool(&mut adjustment, &account, &current.negated());
                    categories
                        .entry(account)
                        .or_default()
                        .add_inventory(&current.negated());
                    tbb.add_inventory(&current);
                }
                Target::Pad { node, amount, .. } => {
                    let account = match accounts::category_account(node, &config.expense_root) {
                        Some(account) => account,
                        None => continue,
                    };
                    let targets: Vec<Money> = match amount {
                        Some(amount) => vec![amount.clone()],
                        None => config
                            .targets
                            .iter()
                            .filter_map(|other| match other {
                                Target::Balance {
                                    month: other_month,
                                    node: other_node,
                                    amount,
                                } if *other_month == month && other_node == node => {
                                    Some(amount.clone())
                                }
                                _ => None,
                            })
                            .collect(),
                    };
                    for target in targets {
                        let current = subtree_balance(&categories, node, &config.expense_root)
                            .get(&target.currency);
                        let delta = target.value - current;
                        if delta.is_zero() {
                            continue;
                        }
                        let mut amounts = Inventory::new();
                        amounts.add(&target.currency, delta);
                        push_against_pool(&mut adjustment, &account, &amounts);
                        categories
                            .entry(account.clone())
                            .or_default()
                            .add_inventory(&amounts);
                        tbb.add_inventory(&amounts.negated());
                        if matches!(rollover_for(config, node, month), Rollover::Reset) {
                            diagnostics.push(PluginError::warning(format!(
                                "{}: {node} is padded to {} {} but its rollover resets it at the month boundary",
                                month.label(),
                                target.value,
                                target.currency
                            )));
                        }
                    }
                }
                Target::Balance { .. } => {}
            }
        }
        if !adjustment.is_empty() {
            generated.extend(adjustment.iter().map(|leg| leg.account.clone()));
            plan.entries.push(Entry {
                kind: EntryKind::Adjustment,
                date: date.clone(),
                narration: format!("{} {}", EntryKind::Adjustment.label(), month.label()),
                legs: adjustment,
            });
        }

        // 5. The rollover policy, applied at the boundary this month ends.
        let mut sweep = Vec::new();
        for &node in &rollover_nodes {
            let policy = rollover_for(config, node, month);
            let account = match accounts::category_account(node, &config.expense_root) {
                Some(account) => account,
                None => continue,
            };
            let balance = subtree_balance(&categories, &node, &config.expense_root);
            let change = match policy {
                Rollover::Carry => Inventory::new(),
                Rollover::Reset => balance.negated(),
                Rollover::Cap(cap) => {
                    let held = balance.get(&cap.currency);
                    if held > cap.value {
                        let mut excess = Inventory::new();
                        excess.add(&cap.currency, -(held - cap.value));
                        excess
                    } else {
                        Inventory::new()
                    }
                }
            };
            if change.is_empty() {
                continue;
            }
            push_against_pool(&mut sweep, &account, &change);
            categories
                .entry(account)
                .or_default()
                .add_inventory(&change);
            tbb.add_inventory(&change.negated());
        }
        if !sweep.is_empty() {
            generated.extend(sweep.iter().map(|leg| leg.account.clone()));
            plan.entries.push(Entry {
                kind: EntryKind::Sweep,
                date: date.clone(),
                narration: format!("{} {}", EntryKind::Sweep.label(), month.label()),
                legs: sweep,
            });
        }

        // Assertions see the whole month, sweep included.
        let assertion_date = month.next().first_day();
        let mut asserted: BTreeSet<(String, String)> = BTreeSet::new();
        for target in config.targets.iter().filter(|it| target_month(it) == month) {
            if let Target::Balance { node, amount, .. } = target {
                if let Some(account) = accounts::category_account(node, &config.expense_root) {
                    asserted.insert((account.clone(), amount.currency.clone()));
                    plan.assertions.push(Assertion {
                        date: assertion_date.clone(),
                        account,
                        amount: amount.clone(),
                    });
                }
            }
        }
        if config.assert_balances {
            // rustledger checks a `balance` against the account *and its
            // subaccounts*, and a category's balance is its subtree sum, so the
            // asserted figure is the subtree — the same number the budget calls
            // available.
            for account in categories.keys() {
                let subtree = subtree_of(&categories, account);
                for (commodity, amount) in subtree.iter() {
                    if asserted.contains(&(account.clone(), commodity.to_string())) {
                        continue;
                    }
                    plan.assertions.push(Assertion {
                        date: assertion_date.clone(),
                        account: account.clone(),
                        amount: Money::new(amount, commodity),
                    });
                }
            }
            for (commodity, amount) in tbb.iter() {
                if asserted.contains(&(TO_BE_BUDGETED.to_string(), commodity.to_string())) {
                    continue;
                }
                plan.assertions.push(Assertion {
                    date: assertion_date.clone(),
                    account: TO_BE_BUDGETED.to_string(),
                    amount: Money::new(amount, commodity),
                });
            }
        }

        for (commodity, amount) in tbb.iter() {
            if amount.is_sign_negative() {
                diagnostics.push(PluginError::warning(format!(
                    "{}: budgeted more than available: {TO_BE_BUDGETED} is {amount} {commodity}",
                    month.label()
                )));
            }
        }

        months.push(plan);
    }

    let open_date = config.start.first_day();
    let already_open: BTreeSet<&str> = directives
        .iter()
        .filter_map(|directive| match &directive.data {
            DirectiveData::Open(open) => Some(open.account.as_str()),
            _ => None,
        })
        .collect();
    let opens = generated
        .into_iter()
        .filter(|account| !already_open.contains(account.as_str()))
        .map(|account| Open {
            date: open_date.clone(),
            account,
        })
        .collect();

    Plan {
        opens,
        months,
        diagnostics,
    }
}

/// The budget horizon: `start date` through `end date`, else `months ahead` past
/// the newest directive in the ledger.
fn horizon(config: &Config, directives: &[DirectiveWrapper]) -> Result<Vec<Month>, PluginError> {
    let start = i64::from(config.start.index());
    // A derived end is clamped to the start (the ledger may be older than the
    // budget); an explicit one before the start is a configuration error below.
    let end = match config.end {
        Some(end) => i64::from(end.index()),
        None => {
            let newest = directives
                .iter()
                .filter_map(|directive| Month::from_date(&directive.date))
                .max()
                .unwrap_or(config.start);
            (i64::from(newest.index()) + i64::from(config.months_ahead)).max(start)
        }
    };
    if end < start {
        let end = config.end.expect("a derived end is clamped to the start");
        return Err(PluginError::error(format!(
            "end date {} is before start date {}",
            end.label(),
            config.start.label()
        )));
    }
    // The span is counted in i64 so a `months ahead` past i32 cannot wrap into a
    // shorter horizon: it reaches the limit check and is refused.
    let count = end - start + 1;
    if count > MAX_MONTHS as i64 {
        // An explicit or moderate derived end is labelled exactly as before; a
        // `months ahead` beyond the i32 index space has no month to name.
        let horizon = i32::try_from(end).map_or_else(
            |_| format!("from {}", config.start.label()),
            |index| {
                format!(
                    "{}..{}",
                    config.start.label(),
                    Month::from_index(index).label()
                )
            },
        );
        return Err(PluginError::error(format!(
            "budget horizon {horizon} spans {count} months; refusing to generate more than {MAX_MONTHS}"
        )));
    }
    Ok((0..count as i32)
        .map(|offset| Month::from_index(config.start.index() + offset))
        .collect())
}

/// Every posting of every transaction, reduced to what budgeting reads from it.
fn movements(directives: &[DirectiveWrapper]) -> Vec<Movement> {
    let mut movements = Vec::new();
    for directive in directives {
        let DirectiveData::Transaction(transaction) = &directive.data else {
            continue;
        };
        let Some(month) = Month::from_date(&directive.date) else {
            continue;
        };
        for posting in &transaction.postings {
            let Some(units) = posting.units.as_ref() else {
                continue;
            };
            let Ok(quantity) = Decimal::from_str(units.number.trim()) else {
                continue;
            };
            if quantity.is_zero() {
                continue;
            }
            movements.push(Movement {
                month,
                account: posting.account.clone(),
                units: Money::new(quantity, units.currency.clone()),
                weight: posting_value(posting, units, quantity),
                filename: directive.filename.clone(),
                lineno: directive.lineno,
            });
        }
    }
    movements
}

/// What a posting was worth, in the commodity it was worth in: its weight.
///
/// `units × cost` when a cost spec carries a value, else `units × price` when a
/// price annotation does, else the units themselves. Nothing is converted: this
/// is the same figure beancount uses to balance the transaction, so an expense
/// always costs the envelope exactly what the ledger says it cost.
///
/// `quantity` is `units.number` already parsed by [`movements`].
fn posting_value(posting: &PostingData, units: &AmountData, quantity: Decimal) -> Option<Money> {
    if let Some(cost) = &posting.cost
        && let Some(currency) = &cost.currency
    {
        let value = match cost.number.as_ref()? {
            CostNumberData::PerUnit { value } => quantity * Decimal::from_str(value.trim()).ok()?,
            CostNumberData::Total { value } => Decimal::from_str(value.trim()).ok()?,
            CostNumberData::PerUnitFromTotal { total, .. } => {
                Decimal::from_str(total.trim()).ok()?
            }
            CostNumberData::Compound { per_unit, total } => {
                quantity * Decimal::from_str(per_unit.trim()).ok()?
                    + Decimal::from_str(total.trim()).ok()?
            }
        };
        return Some(Money::new(value, currency.clone()));
    }

    if let Some(price) = &posting.price {
        match price.view() {
            PriceAnnotationView::Unit(amount) => {
                let per_unit = Decimal::from_str(amount.number.trim()).ok()?;
                return Some(Money::new(quantity * per_unit, amount.currency.clone()));
            }
            PriceAnnotationView::Total(amount) => {
                let total = Decimal::from_str(amount.number.trim()).ok()?;
                return Some(Money::new(total, amount.currency.clone()));
            }
            PriceAnnotationView::UnitIncomplete { .. }
            | PriceAnnotationView::TotalIncomplete { .. } => {}
        }
    }

    Some(Money::new(quantity, units.currency.clone()))
}

/// The category a posting's account belongs to, honouring `map` overrides.
fn category_for(config: &Config, mappings: &[(Regex, String)], account: &str) -> Option<String> {
    if accounts::is_generated(account) {
        return None;
    }
    if let Some((_, node)) = mappings
        .iter()
        .find(|(pattern, _)| pattern.is_match(account))
    {
        return accounts::category_account(node, &config.expense_root);
    }
    accounts::category_account(account, &config.expense_root)
}

/// The category balances of `node`'s subtree, as one inventory.
fn subtree_balance(
    categories: &BTreeMap<String, Inventory>,
    node: &str,
    expense_root: &str,
) -> Inventory {
    match accounts::category_account(node, expense_root) {
        Some(account) => subtree_of(categories, &account),
        None => Inventory::new(),
    }
}

/// The balances of `account` and everything under it, as one inventory.
fn subtree_of(categories: &BTreeMap<String, Inventory>, account: &str) -> Inventory {
    let mut total = Inventory::new();
    for (candidate, balance) in categories {
        if accounts::is_under(candidate, account) {
            total.add_inventory(balance);
        }
    }
    total
}

/// The rollover policy in force for `node` in `month`: the newest declaration at
/// or before it, else carrying everything.
fn rollover_for(config: &Config, node: &str, month: Month) -> Rollover {
    config
        .rollovers
        .iter()
        .filter(|declaration| declaration.node == node)
        .filter(|declaration| declaration.month <= month)
        .max_by_key(|declaration| declaration.month.index())
        .map_or(Rollover::Carry, |declaration| declaration.policy.clone())
}

/// Append `change` (a signed, per-commodity change to `account`) and the
/// opposite legs on `To-Be-Budgeted`, so the entry balances inside the namespace.
fn push_against_pool(legs: &mut Vec<Leg>, account: &str, change: &Inventory) {
    // `Inventory` never holds a zero position (see `Inventory::add`), so every
    // position here deserves a posting.
    for (commodity, amount) in change.iter() {
        legs.push(Leg {
            account: account.to_string(),
            amount: Money::new(amount, commodity),
        });
        legs.push(Leg {
            account: TO_BE_BUDGETED.to_string(),
            amount: Money::new(-amount, commodity),
        });
    }
}

/// The month a target applies to.
fn target_month(target: &Target) -> Month {
    match target {
        Target::Zero { month, .. } | Target::Pad { month, .. } | Target::Balance { month, .. } => {
            *month
        }
    }
}

/// Compile the configured patterns; the parser already rejected broken ones.
fn compile(patterns: &[String]) -> Vec<Regex> {
    patterns
        .iter()
        .filter_map(|pattern| Regex::new(pattern).ok())
        .collect()
}

/// Whether `account` matches any on-budget pattern.
fn matches_any(patterns: &[Regex], account: &str) -> bool {
    patterns.iter().any(|pattern| pattern.is_match(account))
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rust_decimal::Decimal;
    use rustledger_plugin_types::{
        AmountData, DirectiveData, MetaValueData, PostingData, TransactionData,
    };

    use super::*;
    use crate::config::{Adjustment, Fund, Mapping, RolloverDecl};

    fn money(number: &str, currency: &str) -> Money {
        Money::new(
            Decimal::from_str(number).expect("test numbers parse"),
            currency,
        )
    }

    fn config(start: &str, months: usize) -> Config {
        let start = Month::parse(start).expect("test month parses");
        Config {
            start,
            end: Some(Month::from_index(start.index() + months as i32 - 1)),
            months_ahead: 0,
            budget_accounts: vec![r"^Assets:Bank".to_string()],
            income_accounts: vec![r"^Income:".to_string()],
            funds: Vec::new(),
            adjustments: Vec::new(),
            targets: Vec::new(),
            rollovers: Vec::new(),
            mappings: Vec::new(),
            include_starting_balance: false,
            assert_balances: true,
            strict: false,
            expense_root: "Expenses".to_string(),
            income_root: "Income".to_string(),
            warnings: Vec::new(),
        }
    }

    fn txn(date: &str, postings: &[(&str, &str, &str)]) -> DirectiveWrapper {
        DirectiveWrapper {
            directive_type: "transaction".to_string(),
            date: date.to_string(),
            filename: Some("ledger.bean".to_string()),
            lineno: Some(7),
            data: DirectiveData::Transaction(TransactionData {
                flag: "*".to_string(),
                payee: None,
                narration: "test".to_string(),
                tags: Vec::new(),
                links: Vec::new(),
                metadata: Vec::new(),
                postings: postings
                    .iter()
                    .map(|(account, number, currency)| PostingData {
                        account: (*account).to_string(),
                        units: Some(AmountData {
                            number: (*number).to_string(),
                            currency: (*currency).to_string(),
                        }),
                        cost: None,
                        price: None,
                        flag: None,
                        metadata: Vec::new(),
                        span: None,
                    })
                    .collect(),
            }),
        }
    }

    /// Every leg of one entry, as `(account, number, commodity)`.
    fn legs(plan: &Plan, kind: EntryKind) -> Vec<(String, String, String)> {
        plan.months
            .iter()
            .flat_map(|month| month.entries.iter())
            .filter(|entry| entry.kind == kind)
            .flat_map(|entry| entry.legs.iter())
            .map(|leg| {
                (
                    leg.account.clone(),
                    leg.amount.to_amount_data().number,
                    leg.amount.currency.clone(),
                )
            })
            .collect()
    }

    /// The legs of one entry as `account number commodity` lines, in order.
    fn rendered(plan: &Plan, kind: EntryKind) -> Vec<String> {
        legs(plan, kind)
            .into_iter()
            .map(|(account, number, commodity)| format!("{account} {number} {commodity}"))
            .collect()
    }

    fn assertions(plan: &Plan) -> Vec<String> {
        plan.months
            .iter()
            .flat_map(|month| month.assertions.iter())
            .map(|assertion| {
                format!(
                    "{} {} {} {}",
                    assertion.date,
                    assertion.account,
                    assertion.amount.to_amount_data().number,
                    assertion.amount.currency
                )
            })
            .collect()
    }

    /// The whole budget must balance inside its own namespace.
    fn assert_entries_balance(plan: &Plan) {
        let mut total = Inventory::new();
        for month in &plan.months {
            for entry in &month.entries {
                let mut entry_total = Inventory::new();
                for leg in &entry.legs {
                    entry_total.add(&leg.amount.currency, leg.amount.value);
                    total.add(&leg.amount.currency, leg.amount.value);
                }
                assert!(
                    entry_total.is_empty(),
                    "{} does not balance: {entry_total:?}",
                    entry.narration
                );
            }
        }
        assert!(
            total.is_empty(),
            "the namespace does not sum to zero: {total:?}"
        );
    }

    #[test]
    fn spending_is_mirrored_and_the_pool_keeps_the_real_figure() {
        let mut config = config("2026-01", 1);
        config.funds.push(Fund {
            month: Month::new(2026, 1),
            node: "Expenses:Food".to_string(),
            amount: money("100.00", "TRY"),
        });
        let directives = vec![txn(
            "2026-01-05",
            &[
                ("Expenses:Food", "30.00", "TRY"),
                ("Assets:Bank", "-30.00", "TRY"),
            ],
        )];

        let plan = plan(&config, &directives);
        assert_entries_balance(&plan);
        assert_eq!(
            rendered(&plan, EntryKind::Money),
            vec![
                "Equity:Envelope:Money 30.00 TRY",
                "Equity:Envelope:To-Be-Budgeted -30.00 TRY",
            ]
        );
        assert_eq!(
            rendered(&plan, EntryKind::Funding),
            vec![
                "Equity:Envelope:Food 100.00 TRY",
                "Equity:Envelope:To-Be-Budgeted -100.00 TRY",
            ]
        );
        assert_eq!(
            rendered(&plan, EntryKind::Activity),
            vec![
                "Equity:Envelope:Food -30.00 TRY",
                "Equity:Envelope:To-Be-Budgeted 30.00 TRY",
            ]
        );
        // To be budgeted = on-budget money (−30) − categories (70) = −100, and
        // the month is flagged because that is money the ledger does not have.
        assert_eq!(
            assertions(&plan),
            vec![
                "2026-02-01 Equity:Envelope:Food 70.00 TRY",
                "2026-02-01 Equity:Envelope:To-Be-Budgeted -100.00 TRY",
            ]
        );
        assert_eq!(plan.diagnostics.len(), 1);
        assert!(
            plan.diagnostics[0]
                .message
                .contains("budgeted more than available")
        );
    }

    #[test]
    fn funding_is_inherited_until_it_is_replaced() {
        let config = {
            let mut config = config("2026-01", 4);
            config.funds.push(Fund {
                month: Month::new(2026, 1),
                node: "Expenses:Food".to_string(),
                amount: money("100.00", "TRY"),
            });
            config.funds.push(Fund {
                month: Month::new(2026, 3),
                node: "Expenses:Food".to_string(),
                amount: money("40.00", "TRY"),
            });
            config
        };

        let plan = plan(&config, &[]);
        let funded: Vec<String> = legs(&plan, EntryKind::Funding)
            .into_iter()
            .filter(|(account, _, _)| account == "Equity:Envelope:Food")
            .map(|(_, number, _)| number)
            .collect();
        assert_eq!(funded, vec!["100.00", "100.00", "40.00", "40.00"]);
    }

    #[test]
    fn funding_is_inherited_per_commodity() {
        // `0.00` stops the assignment for *its* commodity, not the category:
        // the USD assignment declared later keeps running, and a commodity that
        // was never assigned never appears.
        let mut config = config("2026-01", 3);
        config.funds.push(Fund {
            month: Month::new(2026, 1),
            node: "Expenses:Vps".to_string(),
            amount: money("100.00", "TRY"),
        });
        config.funds.push(Fund {
            month: Month::new(2026, 2),
            node: "Expenses:Vps".to_string(),
            amount: money("20.00", "USD"),
        });
        config.funds.push(Fund {
            month: Month::new(2026, 3),
            node: "Expenses:Vps".to_string(),
            amount: money("0.00", "TRY"),
        });

        let plan = plan(&config, &[]);
        assert_entries_balance(&plan);
        assert_eq!(
            rendered(&plan, EntryKind::Funding),
            vec![
                "Equity:Envelope:Vps 100.00 TRY",
                "Equity:Envelope:To-Be-Budgeted -100.00 TRY",
                "Equity:Envelope:Vps 100.00 TRY",
                "Equity:Envelope:To-Be-Budgeted -100.00 TRY",
                "Equity:Envelope:Vps 20.00 USD",
                "Equity:Envelope:To-Be-Budgeted -20.00 USD",
                // March: the TRY assignment was stopped, the USD one was not.
                "Equity:Envelope:Vps 20.00 USD",
                "Equity:Envelope:To-Be-Budgeted -20.00 USD",
            ]
        );
    }

    #[test]
    fn reset_rollover_sweeps_the_surplus_and_the_deficit_back_to_the_pool() {
        let config = {
            let mut config = config("2026-01", 2);
            config.funds.push(Fund {
                month: Month::new(2026, 1),
                node: "Expenses:Food".to_string(),
                amount: money("100.00", "TRY"),
            });
            config.rollovers.push(RolloverDecl {
                month: Month::new(2026, 1),
                node: "Expenses:Food".to_string(),
                policy: Rollover::Reset,
            });
            config
        };
        let directives = vec![
            txn(
                "2026-01-05",
                &[
                    ("Expenses:Food", "30.00", "TRY"),
                    ("Assets:Bank", "-30.00", "TRY"),
                ],
            ),
            // February overspends the (freshly assigned) envelope.
            txn(
                "2026-02-05",
                &[
                    ("Expenses:Food", "140.00", "TRY"),
                    ("Assets:Bank", "-140.00", "TRY"),
                ],
            ),
        ];

        let plan = plan(&config, &directives);
        assert_entries_balance(&plan);
        assert_eq!(
            rendered(&plan, EntryKind::Sweep),
            vec![
                // January's 70.00 of unspent money goes back to the pool…
                "Equity:Envelope:Food -70.00 TRY",
                "Equity:Envelope:To-Be-Budgeted 70.00 TRY",
                // …and February's 40.00 overspend is covered from it.
                "Equity:Envelope:Food 40.00 TRY",
                "Equity:Envelope:To-Be-Budgeted -40.00 TRY",
            ]
        );
    }

    #[test]
    fn cap_rollover_sweeps_only_the_excess() {
        let config = {
            let mut config = config("2026-01", 1);
            config.funds.push(Fund {
                month: Month::new(2026, 1),
                node: "Expenses:Fun".to_string(),
                amount: money("100.00", "TRY"),
            });
            config.rollovers.push(RolloverDecl {
                month: Month::new(2026, 1),
                node: "Expenses:Fun".to_string(),
                policy: Rollover::Cap(money("50.00", "TRY")),
            });
            config
        };

        let plan = plan(&config, &[]);
        assert_eq!(
            rendered(&plan, EntryKind::Sweep),
            vec![
                "Equity:Envelope:Fun -50.00 TRY",
                "Equity:Envelope:To-Be-Budgeted 50.00 TRY",
            ]
        );
    }

    #[test]
    fn cap_rollover_only_sweeps_a_surplus_in_its_own_commodity() {
        // The cap is a ceiling on one commodity: a deficit is carried (there is
        // nothing to sweep back), and a position in another commodity is not
        // measured against it.
        let mut config = config("2026-01", 2);
        for (number, currency) in [("100.00", "TRY"), ("5.00", "USD")] {
            config.funds.push(Fund {
                month: Month::new(2026, 1),
                node: "Expenses:Fun".to_string(),
                amount: money(number, currency),
            });
        }
        config.rollovers.push(RolloverDecl {
            month: Month::new(2026, 1),
            node: "Expenses:Fun".to_string(),
            policy: Rollover::Cap(money("50.00", "TRY")),
        });
        let directives = vec![txn(
            "2026-02-05",
            &[
                ("Expenses:Fun", "200.00", "TRY"),
                ("Assets:Bank", "-200.00", "TRY"),
            ],
        )];

        let plan = plan(&config, &directives);
        assert_entries_balance(&plan);
        assert_eq!(
            rendered(&plan, EntryKind::Sweep),
            vec![
                // January held 100.00 TRY against a 50.00 TRY cap…
                "Equity:Envelope:Fun -50.00 TRY",
                "Equity:Envelope:To-Be-Budgeted 50.00 TRY",
                // …February ends at -50.00 TRY, below the cap, so nothing is
                // swept; the USD position was never subject to the TRY cap.
            ]
        );
        assert!(
            assertions(&plan).contains(&"2026-03-01 Equity:Envelope:Fun -50.00 TRY".to_string())
        );
        assert!(
            assertions(&plan).contains(&"2026-03-01 Equity:Envelope:Fun 10.00 USD".to_string())
        );
    }

    #[test]
    fn carry_rollover_emits_no_sweep() {
        let config = {
            let mut config = config("2026-01", 1);
            config.funds.push(Fund {
                month: Month::new(2026, 1),
                node: "Expenses:Fun".to_string(),
                amount: money("100.00", "TRY"),
            });
            config.rollovers.push(RolloverDecl {
                month: Month::new(2026, 1),
                node: "Expenses:Fun".to_string(),
                policy: Rollover::Carry,
            });
            config
        };

        let plan = plan(&config, &[]);
        assert!(legs(&plan, EntryKind::Sweep).is_empty());
    }

    #[test]
    fn a_rollover_crosses_the_year_boundary() {
        // December's sweep lands on 2026-12-31 and January's assertion on
        // 2027-02-01: the month arithmetic, not just `Month`, has to step into
        // the next year.
        let mut config = config("2026-11", 3);
        config.funds.push(Fund {
            month: Month::new(2026, 11),
            node: "Expenses:Fun".to_string(),
            amount: money("100.00", "TRY"),
        });
        config.rollovers.push(RolloverDecl {
            month: Month::new(2026, 12),
            node: "Expenses:Fun".to_string(),
            policy: Rollover::Reset,
        });
        // January switches back to carrying, so its assertion is the one dated
        // in the new year.
        config.rollovers.push(RolloverDecl {
            month: Month::new(2027, 1),
            node: "Expenses:Fun".to_string(),
            policy: Rollover::Carry,
        });

        let plan = plan(&config, &[]);
        assert_entries_balance(&plan);
        assert_eq!(
            rendered(&plan, EntryKind::Sweep),
            vec![
                // December resets the two months of assignments…
                "Equity:Envelope:Fun -200.00 TRY",
                "Equity:Envelope:To-Be-Budgeted 200.00 TRY",
                // …and January carries.
            ]
        );
        assert_eq!(
            assertions(&plan),
            vec![
                "2026-12-01 Equity:Envelope:Fun 100.00 TRY",
                "2026-12-01 Equity:Envelope:To-Be-Budgeted -100.00 TRY",
                // January's reset leaves nothing to assert at 2027-01-01, so the
                // next assertions are February's, dated in the new year.
                "2027-02-01 Equity:Envelope:Fun 100.00 TRY",
                "2027-02-01 Equity:Envelope:To-Be-Budgeted -100.00 TRY",
            ]
        );
    }

    #[test]
    fn spending_on_a_leaf_lands_in_its_parents_subtree() {
        let config = {
            let mut config = config("2026-01", 1);
            config.funds.push(Fund {
                month: Month::new(2026, 1),
                node: "Expenses:Groceries".to_string(),
                amount: money("100.00", "TRY"),
            });
            config
        };
        let directives = vec![txn(
            "2026-01-05",
            &[
                ("Expenses:Groceries:Bread", "30.00", "TRY"),
                ("Assets:Bank", "-30.00", "TRY"),
            ],
        )];

        let plan = plan(&config, &directives);
        assert_eq!(
            rendered(&plan, EntryKind::Activity),
            vec![
                "Equity:Envelope:Groceries:Bread -30.00 TRY",
                "Equity:Envelope:To-Be-Budgeted 30.00 TRY",
            ]
        );
        assert_eq!(
            assertions(&plan),
            vec![
                // rustledger checks a `balance` against the account and its
                // subaccounts, so the parent asserts the subtree (100 − 30) —
                // the same figure the budget calls available in Groceries.
                "2026-02-01 Equity:Envelope:Groceries 70.00 TRY",
                "2026-02-01 Equity:Envelope:Groceries:Bread -30.00 TRY",
                // To be budgeted = on-budget money (−30) − categories (70).
                "2026-02-01 Equity:Envelope:To-Be-Budgeted -100.00 TRY",
            ]
        );
    }

    #[test]
    fn zero_releases_a_whole_subtree_and_pad_sets_a_balance() {
        let config = {
            let mut config = config("2026-01", 2);
            config.funds.push(Fund {
                month: Month::new(2026, 1),
                node: "Expenses:Groceries".to_string(),
                amount: money("100.00", "TRY"),
            });
            config.targets.push(Target::Zero {
                month: Month::new(2026, 1),
                node: "Expenses:Groceries".to_string(),
            });
            config.targets.push(Target::Pad {
                month: Month::new(2026, 2),
                node: "Expenses:Groceries".to_string(),
                amount: Some(money("25.00", "TRY")),
            });
            config
        };

        let plan = plan(&config, &[]);
        assert_entries_balance(&plan);
        assert_eq!(
            rendered(&plan, EntryKind::Adjustment),
            vec![
                // January releases the assigned 100.00…
                "Equity:Envelope:Groceries -100.00 TRY",
                "Equity:Envelope:To-Be-Budgeted 100.00 TRY",
                // …and February is reconciled to exactly 25.00 on top of the
                // assignment that arrived with it (100.00 − 25.00 = 75.00 out).
                "Equity:Envelope:Groceries -75.00 TRY",
                "Equity:Envelope:To-Be-Budgeted 75.00 TRY",
            ]
        );
    }

    #[test]
    fn bare_pad_takes_its_target_from_the_declared_balance() {
        let config = {
            let mut config = config("2026-01", 1);
            config.targets.push(Target::Pad {
                month: Month::new(2026, 1),
                node: "Expenses:Travel".to_string(),
                amount: None,
            });
            config.targets.push(Target::Balance {
                month: Month::new(2026, 1),
                node: "Expenses:Travel".to_string(),
                amount: money("250.00", "EUR"),
            });
            config
        };

        let plan = plan(&config, &[]);
        assert_eq!(
            rendered(&plan, EntryKind::Adjustment),
            vec![
                "Equity:Envelope:Travel 250.00 EUR",
                "Equity:Envelope:To-Be-Budgeted -250.00 EUR",
            ]
        );
        // The declared assertion is emitted as written, so rustledger's own
        // validation checks it.
        assert!(
            assertions(&plan).contains(&"2026-02-01 Equity:Envelope:Travel 250.00 EUR".to_string())
        );
    }

    #[test]
    fn nothing_is_ever_converted() {
        let mut config = config("2026-01", 1);
        config.funds.push(Fund {
            month: Month::new(2026, 1),
            node: "Expenses:Vps".to_string(),
            amount: money("30.00", "USD"),
        });
        let directives = vec![
            txn(
                "2026-01-05",
                &[
                    ("Expenses:Vps", "12.00", "USD"),
                    ("Assets:Bank", "-12.00", "USD"),
                ],
            ),
            txn(
                "2026-01-06",
                &[
                    ("Expenses:Food", "500.00", "TRY"),
                    ("Assets:Bank", "-500.00", "TRY"),
                ],
            ),
        ];

        let plan = plan(&config, &directives);
        assert_entries_balance(&plan);
        assert_eq!(
            rendered(&plan, EntryKind::Activity),
            vec![
                "Equity:Envelope:Food -500.00 TRY",
                "Equity:Envelope:To-Be-Budgeted 500.00 TRY",
                "Equity:Envelope:Vps -12.00 USD",
                "Equity:Envelope:To-Be-Budgeted 12.00 USD",
            ]
        );
        // Each commodity stands on its own: USD was funded more than it
        // received, while TRY only passed through.
        let shortfalls: Vec<&String> = plan
            .diagnostics
            .iter()
            .map(|diagnostic| &diagnostic.message)
            .filter(|message| message.contains("budgeted more than available"))
            .collect();
        assert_eq!(shortfalls.len(), 1);
        assert!(shortfalls[0].contains("USD"));
    }

    #[test]
    fn activity_uses_the_postings_cost_as_written() {
        let config = config("2026-01", 1);
        let mut shares = txn("2026-01-05", &[("Expenses:Investing", "10.00", "AAPL")]);
        if let DirectiveData::Transaction(transaction) = &mut shares.data {
            transaction.postings[0].cost = Some(rustledger_plugin_types::CostData {
                number: Some(CostNumberData::PerUnit {
                    value: "150.00".to_string(),
                }),
                currency: Some("USD".to_string()),
                date: None,
                label: None,
                merge: false,
            });
            transaction.postings.push(PostingData {
                account: "Assets:Bank".to_string(),
                units: Some(AmountData {
                    number: "-1500.00".to_string(),
                    currency: "USD".to_string(),
                }),
                cost: None,
                price: None,
                flag: None,
                metadata: Vec::new(),
                span: None,
            });
        }

        let plan = plan(&config, &[shares]);
        assert_eq!(
            rendered(&plan, EntryKind::Activity),
            vec![
                "Equity:Envelope:Investing -1500.00 USD",
                "Equity:Envelope:To-Be-Budgeted 1500.00 USD",
            ]
        );
    }

    #[test]
    fn map_redirections_and_strict_warnings() {
        let config = {
            let mut config = config("2026-01", 1);
            config.strict = true;
            config.mappings.push(Mapping {
                pattern: r"^Expenses:Cafe$".to_string(),
                node: "Expenses:Dining-Out".to_string(),
            });
            config.funds.push(Fund {
                month: Month::new(2026, 1),
                node: "Expenses:Dining-Out".to_string(),
                amount: money("200.00", "TRY"),
            });
            config
        };
        let directives = vec![
            txn(
                "2026-01-05",
                &[
                    ("Expenses:Cafe", "50.00", "TRY"),
                    ("Assets:Bank", "-50.00", "TRY"),
                ],
            ),
            txn(
                "2026-01-06",
                &[
                    ("Expenses:Coffee", "20.00", "TRY"),
                    ("Assets:Bank", "-20.00", "TRY"),
                ],
            ),
        ];

        let plan = plan(&config, &directives);
        let activity = rendered(&plan, EntryKind::Activity);
        assert!(activity.contains(&"Equity:Envelope:Dining-Out -50.00 TRY".to_string()));
        assert!(activity.contains(&"Equity:Envelope:Coffee -20.00 TRY".to_string()));
        // Only the unfunded category is reported, and with its source location.
        let strict: Vec<&PluginError> = plan
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("with no assignment"))
            .collect();
        assert_eq!(strict.len(), 1);
        assert!(strict[0].message.contains("Equity:Envelope:Coffee"));
        assert_eq!(strict[0].line_number, Some(7));
        assert_eq!(strict[0].source_file.as_deref(), Some("ledger.bean"));
    }

    #[test]
    fn starting_balance_is_only_counted_when_configured() {
        let opening = txn(
            "2025-12-31",
            &[
                ("Assets:Bank", "500.00", "TRY"),
                ("Equity:Opening-Balances", "-500.00", "TRY"),
            ],
        );

        let excluded = plan(&config("2026-01", 1), std::slice::from_ref(&opening));
        // Money held before the horizon is not part of the budget at all, so the
        // month has no backing entry to make.
        assert!(legs(&excluded, EntryKind::Money).is_empty());

        let mut included_config = config("2026-01", 1);
        included_config.include_starting_balance = true;
        let included = plan(&included_config, &[opening]);
        assert_eq!(
            rendered(&included, EntryKind::Money),
            vec![
                "Equity:Envelope:Money -500.00 TRY",
                "Equity:Envelope:To-Be-Budgeted 500.00 TRY",
            ]
        );
    }

    #[test]
    fn generated_accounts_are_opened_once_and_never_reopened() {
        let mut config = config("2026-01", 1);
        config.funds.push(Fund {
            month: Month::new(2026, 1),
            node: "Expenses:Food".to_string(),
            amount: money("100.00", "TRY"),
        });
        let already_open = DirectiveWrapper {
            directive_type: "open".to_string(),
            date: "2026-01-01".to_string(),
            filename: None,
            lineno: None,
            data: DirectiveData::Open(rustledger_plugin_types::OpenData {
                account: "Equity:Envelope:Food".to_string(),
                currencies: Vec::new(),
                booking: None,
                metadata: Vec::new(),
            }),
        };

        let plan = plan(&config, &[already_open]);
        let accounts: Vec<&str> = plan
            .opens
            .iter()
            .map(|open| open.account.as_str())
            .collect();
        assert!(!accounts.contains(&"Equity:Envelope:Food"));
        assert!(accounts.contains(&"Equity:Envelope:To-Be-Budgeted"));
        assert!(accounts.contains(&"Equity:Envelope:Money"));
        assert_eq!(plan.opens[0].date, "2026-01-01");
    }

    #[test]
    fn the_horizon_comes_from_the_ledger_when_no_end_date_is_given() {
        let mut config = config("2026-01", 1);
        config.end = None;
        config.months_ahead = 2;
        let directives = vec![txn(
            "2026-03-15",
            &[
                ("Expenses:Food", "1.00", "TRY"),
                ("Assets:Bank", "-1.00", "TRY"),
            ],
        )];

        let plan = plan(&config, &directives);
        let months: Vec<String> = plan
            .months
            .iter()
            .map(|month| month.month.label())
            .collect();
        assert_eq!(
            months,
            vec!["2026-01", "2026-02", "2026-03", "2026-04", "2026-05"]
        );
    }

    #[test]
    fn an_absurd_horizon_is_refused_instead_of_generated() {
        let mut config = config("2026-01", 1);
        config.end = Some(Month::new(3026, 1));

        let plan = plan(&config, &[]);
        assert!(plan.months.is_empty());
        assert_eq!(plan.diagnostics.len(), 1);
        assert!(plan.diagnostics[0].message.contains("refusing to generate"));
    }

    #[test]
    fn a_months_ahead_past_i32_is_refused_not_wrapped() {
        // `months ahead` is parsed as a u32; casting it to the i32 index space
        // would wrap a large value into the past and quietly produce a one-month
        // horizon instead of refusing.
        let mut config = config("2026-01", 1);
        config.end = None;
        config.months_ahead = u32::MAX;

        let plan = plan(&config, &[]);
        assert!(plan.months.is_empty());
        assert_eq!(plan.diagnostics.len(), 1);
        assert!(plan.diagnostics[0].message.contains("refusing to generate"));
    }

    #[test]
    fn adjustments_move_money_between_a_category_and_the_pool() {
        let config = {
            let mut config = config("2026-01", 1);
            config.funds.push(Fund {
                month: Month::new(2026, 1),
                node: "Expenses:Food".to_string(),
                amount: money("100.00", "TRY"),
            });
            config.adjustments.push(Adjustment {
                month: Month::new(2026, 1),
                node: "Expenses:Food".to_string(),
                amount: money("-40.00", "TRY"),
            });
            config
        };

        let plan = plan(&config, &[]);
        assert_eq!(
            rendered(&plan, EntryKind::Adjustment),
            vec![
                "Equity:Envelope:Food -40.00 TRY",
                "Equity:Envelope:To-Be-Budgeted 40.00 TRY",
            ]
        );
        assert!(
            assertions(&plan).contains(&"2026-02-01 Equity:Envelope:Food 60.00 TRY".to_string())
        );
    }

    #[test]
    fn a_padded_category_whose_rollover_resets_is_flagged() {
        let config = {
            let mut config = config("2026-01", 1);
            config.targets.push(Target::Pad {
                month: Month::new(2026, 1),
                node: "Expenses:Travel".to_string(),
                amount: Some(money("250.00", "TRY")),
            });
            config.rollovers.push(RolloverDecl {
                month: Month::new(2026, 1),
                node: "Expenses:Travel".to_string(),
                policy: Rollover::Reset,
            });
            config
        };

        let plan = plan(&config, &[]);
        assert!(
            plan.diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("rollover resets it"))
        );
    }

    #[test]
    fn unused_income_configuration_is_accepted() {
        // The ledger's income patterns are read for fava-envelope compatibility;
        // availability comes from on-budget balances, so they change nothing.
        let config = config("2026-01", 1);
        let directives = vec![txn(
            "2026-01-05",
            &[
                ("Income:Salary", "-100.00", "TRY"),
                ("Assets:Bank", "100.00", "TRY"),
            ],
        )];

        let plan = plan(&config, &directives);
        assert_entries_balance(&plan);
        assert!(legs(&plan, EntryKind::Activity).is_empty());
        assert_eq!(
            rendered(&plan, EntryKind::Money),
            vec![
                "Equity:Envelope:Money -100.00 TRY",
                "Equity:Envelope:To-Be-Budgeted 100.00 TRY",
            ]
        );
    }

    #[test]
    fn unsupported_custom_directives_are_ignored() {
        let custom = DirectiveWrapper {
            directive_type: "custom".to_string(),
            date: "2026-01-01".to_string(),
            filename: None,
            lineno: None,
            data: DirectiveData::Custom(rustledger_plugin_types::CustomData {
                custom_type: "fava-extension".to_string(),
                values: vec![MetaValueData::String("fava_envelope".to_string())],
                metadata: Vec::new(),
            }),
        };

        let plan = plan(&config("2026-01", 1), &[custom]);
        assert_eq!(plan.months.len(), 1);
        assert!(plan.diagnostics.is_empty());
    }
    fn posting(account: &str, number: &str, currency: &str) -> PostingData {
        PostingData {
            account: account.to_string(),
            units: Some(AmountData {
                number: number.to_string(),
                currency: currency.to_string(),
            }),
            cost: None,
            price: None,
            flag: None,
            metadata: Vec::new(),
            span: None,
        }
    }

    fn cost(number: CostNumberData) -> rustledger_plugin_types::CostData {
        rustledger_plugin_types::CostData {
            number: Some(number),
            currency: Some("USD".to_string()),
            date: None,
            label: None,
            merge: false,
        }
    }

    fn price(
        amount: &str,
        currency: &str,
        is_total: bool,
    ) -> rustledger_plugin_types::PriceAnnotationData {
        rustledger_plugin_types::PriceAnnotationData {
            is_total,
            amount: Some(AmountData {
                number: amount.to_string(),
                currency: currency.to_string(),
            }),
            number: None,
            currency: None,
        }
    }

    fn txn_of(date: &str, postings: Vec<PostingData>) -> DirectiveWrapper {
        DirectiveWrapper {
            directive_type: "transaction".to_string(),
            date: date.to_string(),
            filename: None,
            lineno: None,
            data: DirectiveData::Transaction(TransactionData {
                flag: "*".to_string(),
                payee: None,
                narration: "priced".to_string(),
                tags: Vec::new(),
                links: Vec::new(),
                metadata: Vec::new(),
                postings,
            }),
        }
    }

    #[test]
    fn a_postings_weight_is_read_as_written() {
        let mut per_unit = posting("Expenses:PerUnit", "10.00", "AAPL");
        per_unit.cost = Some(cost(CostNumberData::PerUnit {
            value: "150.00".to_string(),
        }));
        let mut total = posting("Expenses:Total", "10.00", "AAPL");
        total.cost = Some(cost(CostNumberData::Total {
            value: "1500.00".to_string(),
        }));
        let mut booked = posting("Expenses:Booked", "10.00", "AAPL");
        booked.cost = Some(cost(CostNumberData::PerUnitFromTotal {
            per_unit: "150.00".to_string(),
            total: "1500.00".to_string(),
        }));
        let mut compound = posting("Expenses:Compound", "2.00", "SHARE");
        compound.cost = Some(cost(CostNumberData::Compound {
            per_unit: "5.00".to_string(),
            total: "10.00".to_string(),
        }));
        let mut priced = posting("Expenses:Priced", "10.00", "EUR");
        priced.price = Some(price("1.10", "USD", false));
        let mut priced_total = posting("Expenses:PricedTotal", "10.00", "EUR");
        priced_total.price = Some(price("11.00", "USD", true));
        // An incomplete annotation says nothing about what the posting was
        // worth, so the units stand.
        let mut incomplete = posting("Expenses:IncompletePrice", "10.00", "EUR");
        incomplete.price = Some(rustledger_plugin_types::PriceAnnotationData {
            is_total: false,
            amount: None,
            number: None,
            currency: Some("USD".to_string()),
        });

        let plan = plan(
            &config("2026-01", 1),
            &[txn_of(
                "2026-01-05",
                vec![
                    per_unit,
                    total,
                    booked,
                    compound,
                    priced,
                    priced_total,
                    incomplete,
                ],
            )],
        );

        assert_eq!(
            rendered(&plan, EntryKind::Activity),
            vec![
                "Equity:Envelope:Booked -1500.00 USD",
                "Equity:Envelope:To-Be-Budgeted 1500.00 USD",
                "Equity:Envelope:Compound -20.00 USD",
                "Equity:Envelope:To-Be-Budgeted 20.00 USD",
                "Equity:Envelope:IncompletePrice -10.00 EUR",
                "Equity:Envelope:To-Be-Budgeted 10.00 EUR",
                "Equity:Envelope:PerUnit -1500.00 USD",
                "Equity:Envelope:To-Be-Budgeted 1500.00 USD",
                "Equity:Envelope:Priced -11.00 USD",
                "Equity:Envelope:To-Be-Budgeted 11.00 USD",
                "Equity:Envelope:PricedTotal -11.00 USD",
                "Equity:Envelope:To-Be-Budgeted 11.00 USD",
                "Equity:Envelope:Total -1500.00 USD",
                "Equity:Envelope:To-Be-Budgeted 1500.00 USD",
            ]
        );
    }

    #[test]
    fn postings_the_budget_cannot_value_are_skipped() {
        let mut no_units = posting("Expenses:NoUnits", "0", "TRY");
        no_units.units = None;
        let mut unparseable = posting("Expenses:Unparseable", "0", "TRY");
        unparseable.units = Some(AmountData {
            number: "not a number".to_string(),
            currency: "TRY".to_string(),
        });
        let zero = posting("Expenses:Zero", "0.00", "TRY");
        // A hand-written posting inside the namespace is the budget's own money,
        // never a category of its own.
        let handwritten = posting("Equity:Envelope:Handwritten", "5.00", "TRY");
        let real = posting("Expenses:Real", "12.00", "TRY");
        let bank = posting("Assets:Bank", "-12.00", "TRY");

        // A directive that is not a transaction, and one whose date does not
        // parse, are ignored rather than guessed at.
        let not_a_transaction = DirectiveWrapper {
            directive_type: "open".to_string(),
            date: "2026-01-01".to_string(),
            filename: None,
            lineno: None,
            data: DirectiveData::Open(rustledger_plugin_types::OpenData {
                account: "Expenses:Real".to_string(),
                currencies: Vec::new(),
                booking: None,
                metadata: Vec::new(),
            }),
        };
        let undated = txn_of("whenever", vec![posting("Expenses:Undated", "3.00", "TRY")]);

        let plan = plan(
            &config("2026-01", 1),
            &[
                not_a_transaction,
                undated,
                txn_of(
                    "2026-01-05",
                    vec![no_units, unparseable, zero, handwritten, real, bank],
                ),
            ],
        );

        assert_eq!(
            rendered(&plan, EntryKind::Activity),
            vec![
                "Equity:Envelope:Real -12.00 TRY",
                "Equity:Envelope:To-Be-Budgeted 12.00 TRY",
            ]
        );
        assert_eq!(
            rendered(&plan, EntryKind::Money),
            vec![
                "Equity:Envelope:Money 12.00 TRY",
                "Equity:Envelope:To-Be-Budgeted -12.00 TRY",
            ]
        );
    }

    #[test]
    fn transactions_after_the_horizon_are_not_budgeted() {
        let directives = vec![
            txn(
                "2026-01-05",
                &[
                    ("Expenses:Food", "10.00", "TRY"),
                    ("Assets:Bank", "-10.00", "TRY"),
                ],
            ),
            txn(
                "2026-03-05",
                &[
                    ("Expenses:Food", "99.00", "TRY"),
                    ("Assets:Bank", "-99.00", "TRY"),
                ],
            ),
        ];

        let plan = plan(&config("2026-01", 1), &directives);
        assert_eq!(plan.months.len(), 1);
        assert_eq!(
            rendered(&plan, EntryKind::Activity),
            vec![
                "Equity:Envelope:Food -10.00 TRY",
                "Equity:Envelope:To-Be-Budgeted 10.00 TRY",
            ]
        );
        assert_eq!(
            rendered(&plan, EntryKind::Money),
            vec![
                "Equity:Envelope:Money 10.00 TRY",
                "Equity:Envelope:To-Be-Budgeted -10.00 TRY",
            ]
        );
    }

    #[test]
    fn off_budget_accounts_do_not_fund_the_pool() {
        // Paying from money the budget does not track still consumes the
        // category, but adds nothing to the pool.
        let directives = vec![txn(
            "2026-01-05",
            &[
                ("Expenses:Food", "80.00", "TRY"),
                ("Liabilities:Loan:Mom", "-80.00", "TRY"),
            ],
        )];

        let plan = plan(&config("2026-01", 1), &directives);
        assert!(legs(&plan, EntryKind::Money).is_empty());
        assert_eq!(
            rendered(&plan, EntryKind::Activity),
            vec![
                "Equity:Envelope:Food -80.00 TRY",
                "Equity:Envelope:To-Be-Budgeted 80.00 TRY",
            ]
        );
    }

    #[test]
    fn an_end_date_before_the_start_is_refused() {
        let mut config = config("2026-05", 1);
        config.end = Some(Month::new(2026, 1));

        let plan = plan(&config, &[]);
        assert!(plan.months.is_empty());
        assert_eq!(plan.diagnostics.len(), 1);
        assert!(plan.diagnostics[0].message.contains("before start date"));
    }

    #[test]
    fn a_strict_warning_without_a_source_location_still_reports() {
        let mut config = config("2026-01", 1);
        config.strict = true;
        let mut directive = txn(
            "2026-01-05",
            &[
                ("Expenses:Coffee", "20.00", "TRY"),
                ("Assets:Bank", "-20.00", "TRY"),
            ],
        );
        directive.filename = None;
        directive.lineno = None;

        let plan = plan(&config, &[directive]);
        let strict: Vec<&PluginError> = plan
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("with no assignment"))
            .collect();
        assert_eq!(strict.len(), 1);
        assert!(strict[0].source_file.is_none());
        assert!(strict[0].line_number.is_none());
    }

    #[test]
    fn malformed_cost_or_price_values_are_ignored() {
        // A posting the budget cannot value must be skipped, never counted as
        // zero: silently turning a broken cost into "free" would spend an
        // envelope by the wrong amount.
        let mut bad_per_unit = posting("Expenses:BadPerUnit", "10.00", "AAPL");
        bad_per_unit.cost = Some(cost(CostNumberData::PerUnit {
            value: "not a number".to_string(),
        }));
        let mut bad_total = posting("Expenses:BadTotal", "10.00", "AAPL");
        bad_total.cost = Some(cost(CostNumberData::Total {
            value: "not a number".to_string(),
        }));
        let mut bad_booked = posting("Expenses:BadBooked", "10.00", "AAPL");
        bad_booked.cost = Some(cost(CostNumberData::PerUnitFromTotal {
            per_unit: "not a number".to_string(),
            total: "not a number".to_string(),
        }));
        let mut bad_compound = posting("Expenses:BadCompound", "2.00", "SHARE");
        bad_compound.cost = Some(cost(CostNumberData::Compound {
            per_unit: "not a number".to_string(),
            total: "not a number".to_string(),
        }));
        // `{}`: an at-cost posting whose cost was never resolved. Guessing the
        // units as its value would understate the spend, so it is skipped.
        let mut unresolved = posting("Expenses:UnresolvedCost", "10.00", "AAPL");
        unresolved.cost = Some(rustledger_plugin_types::CostData {
            number: None,
            currency: Some("USD".to_string()),
            date: None,
            label: None,
            merge: false,
        });
        let mut bad_priced = posting("Expenses:BadPriced", "10.00", "EUR");
        bad_priced.price = Some(price("not a number", "USD", false));
        let mut bad_priced_total = posting("Expenses:BadPricedTotal", "10.00", "EUR");
        bad_priced_total.price = Some(price("not a number", "USD", true));

        let plan = plan(
            &config("2026-01", 1),
            &[txn_of(
                "2026-01-05",
                vec![
                    bad_per_unit,
                    bad_total,
                    bad_booked,
                    bad_compound,
                    unresolved,
                    bad_priced,
                    bad_priced_total,
                ],
            )],
        );

        assert!(legs(&plan, EntryKind::Activity).is_empty());
        assert!(legs(&plan, EntryKind::Money).is_empty());
    }

    #[test]
    fn the_horizon_clamps_to_the_start_when_the_ledger_is_older() {
        let mut config = config("2026-05", 1);
        config.end = None;
        let directives = vec![txn(
            "2026-01-05",
            &[
                ("Expenses:Food", "10.00", "TRY"),
                ("Assets:Bank", "-10.00", "TRY"),
            ],
        )];

        let plan = plan(&config, &directives);
        assert_eq!(plan.months.len(), 1);
        assert_eq!(plan.months[0].month, Month::new(2026, 5));
        // January is before the budget starts, so it is not a budget month and
        // (without `include starting balance`) contributes no money.
        assert!(legs(&plan, EntryKind::Activity).is_empty());
        assert!(legs(&plan, EntryKind::Money).is_empty());
    }

    #[test]
    fn the_plugins_own_accounts_are_never_categories() {
        // Defence in depth: the movement loops skip generated accounts before
        // asking, and `category_for` refuses them too, so a hand-written posting
        // inside the namespace can never mirror into itself.
        assert_eq!(
            category_for(&config("2026-01", 1), &[], "Equity:Envelope:Handwritten"),
            None
        );
        assert_eq!(
            category_for(&config("2026-01", 1), &[], "Expenses:Food:Groceries").as_deref(),
            Some("Equity:Envelope:Food:Groceries")
        );
    }
    #[test]
    fn buying_currency_shows_the_currency_it_bought() {
        // The acquired leg of an FX purchase carries a price, so its *weight* is
        // TRY. What the budget has to see is the commodity the account now
        // holds — otherwise a USD envelope could never be funded from a USD
        // purchase, and every USD category would look permanently short.
        let mut config = config("2026-01", 1);
        config.funds.push(Fund {
            month: Month::new(2026, 1),
            node: "Expenses:Vps".to_string(),
            amount: money("20.00", "USD"),
        });
        let mut bought = txn(
            "2026-01-03",
            &[
                ("Assets:Bank:US", "100.00", "USD"),
                ("Assets:Bank", "-3500.00", "TRY"),
            ],
        );
        if let DirectiveData::Transaction(transaction) = &mut bought.data {
            transaction.postings[0].price = Some(price("35.00", "TRY", false));
        }
        let spent = txn(
            "2026-01-25",
            &[
                ("Expenses:Vps", "20.00", "USD"),
                ("Assets:Bank:US", "-20.00", "USD"),
            ],
        );

        let plan = plan(&config, &[bought, spent]);
        assert_entries_balance(&plan);
        assert_eq!(
            rendered(&plan, EntryKind::Money),
            vec![
                "Equity:Envelope:Money 3500.00 TRY",
                "Equity:Envelope:To-Be-Budgeted -3500.00 TRY",
                "Equity:Envelope:Money -80.00 USD",
                "Equity:Envelope:To-Be-Budgeted 80.00 USD",
            ]
        );
        // The USD assignment is covered by the USD that was bought; the TRY
        // shortfall is real, because that TRY was spent without being assigned.
        let messages: Vec<&str> = plan
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect();
        assert!(
            messages.iter().any(|message| message.contains("TRY")),
            "{messages:?}"
        );
        assert!(
            !messages.iter().any(|message| message.contains("USD")),
            "{messages:?}"
        );
    }
}
