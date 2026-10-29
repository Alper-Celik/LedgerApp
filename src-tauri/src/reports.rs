// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The money-across-months reports: net worth, cash flow, spending by category,
//! and the same spending restated in an earlier month's money.
//!
//! Every figure is an exact decimal string in the report's one currency, taken
//! from the ledger's own [`Decimal`] amounts — nothing here goes through a
//! float. The reports read [`Ledger::balance_view`], the booked stream with pad
//! effects applied, so a padded asset values the same here as in the account
//! tree.
//!
//! The month range ends at the ledger's newest transaction and runs back
//! `months` calendar months. A ledger that has not been written to this month
//! therefore still reports its last month, which is what a test fixture and a
//! freshly opened journal both need. Months before the ledger's first entry are
//! zero figures: the range is about time, not about the ledger's age.
//!
//! A report uses one commodity throughout. The ledger's first operating
//! currency that appears in scope wins; otherwise the commodity carrying the
//! largest absolute figure (ties by name). Amounts in other commodities are
//! left out rather than converted — the index providers' FX series is the place
//! for conversion, and it is deliberately not mixed into these statements.

use std::collections::BTreeMap;

use rust_decimal::Decimal;
use rustledger_core::{Directive, IncompleteAmount, Posting};
use rustledger_loader::Ledger;

use crate::index::{
    IndexSeries, index_at, is_year_month, missing_point, percentage_change, real_amount,
};

/// One month of the ledger's net worth: assets plus liabilities, the liabilities
/// being the negative balances Beancount books them as.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct NetWorthMonth {
    /// `YYYY-MM`.
    pub month: String,
    /// Net worth at the month's end, an exact decimal string.
    pub value: String,
    /// The commodity every figure is in.
    pub currency: String,
}

/// One month of income and expenses.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct CashflowMonth {
    /// `YYYY-MM`.
    pub month: String,
    /// Money that came in: income postings, signed so they read positive.
    pub inflow: String,
    /// Money that went out: expense postings, positive.
    pub outflow: String,
    /// The commodity every figure is in.
    pub currency: String,
}

/// One month of one expense category.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct CategoryMonth {
    /// `YYYY-MM`.
    pub month: String,
    /// That month's spending in the category, a positive decimal string.
    pub spent: String,
}

/// Spending over time for one top-level expense branch.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct CategoryMonths {
    /// The grouped account, e.g. `Expenses:Food:Groceries`: the top two levels
    /// under `Expenses`. Deeper accounts roll up into it.
    pub account: String,
    /// The branch's own path below `Expenses`, e.g. `Food:Groceries`.
    pub label: String,
    /// The commodity every figure is in.
    pub currency: String,
    /// One row per month of the report range, oldest first; a month with no
    /// spending reports `"0"`.
    pub months: Vec<CategoryMonth>,
}

/// The index a lifestyle report restated its spending with.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct LifestyleIndex {
    /// The series id, e.g. `tr-cpi`.
    pub id: String,
    /// The series label.
    pub label: String,
    /// The index's own change from the first month of the range to the last,
    /// in percent with two decimals; `None` when the range holds one month.
    pub change: Option<String>,
}

/// The index value in force in one month.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct LifestyleIndexMonth {
    /// `YYYY-MM`.
    pub month: String,
    /// The series value at that month, an exact decimal string.
    pub index_value: String,
}

/// One month's spending, nominal and restated in the earliest month's money.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct LifestyleTotal {
    /// `YYYY-MM`.
    pub month: String,
    /// What was spent that month, nominal.
    pub nominal: String,
    /// The same spending converted with [`real_amount`] into the earliest
    /// month's money.
    pub real: String,
    /// The commodity every figure is in.
    pub currency: String,
}

/// One month of one category's lifestyle figures.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct LifestyleCategoryMonth {
    /// `YYYY-MM`.
    pub month: String,
    /// Nominal spending in the category.
    pub nominal: String,
    /// The same spending in the earliest month's money.
    pub real: String,
}

/// One expense category in a lifestyle report.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct LifestyleCategory {
    /// The grouped account, e.g. `Expenses:Food:Groceries`.
    pub account: String,
    /// The branch's own path below `Expenses`, e.g. `Food:Groceries`.
    pub label: String,
    /// The commodity every figure is in.
    pub currency: String,
    /// One row per month of the range.
    pub months: Vec<LifestyleCategoryMonth>,
    /// The latest month's nominal spending against the month before it, in
    /// percent; `None` when there is no previous month or it was zero.
    pub nominal_change: Option<String>,
    /// The same change measured in real terms.
    pub real_change: Option<String>,
}

/// Spending across a range of months, nominal and inflation-adjusted.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct LifestyleReport {
    /// The index used and its own movement over the range.
    pub index: LifestyleIndex,
    /// The index value in force in each month.
    pub months: Vec<LifestyleIndexMonth>,
    /// Total spending per month, nominal and real.
    pub totals: Vec<LifestyleTotal>,
    /// Spending per expense category, nominal and real.
    pub categories: Vec<LifestyleCategory>,
}

/// The month key of a `YYYY-MM` label: months since year 0, so arithmetic on a
/// range is subtraction rather than a calendar.
fn month_key(label: &str) -> Option<i64> {
    if !is_year_month(label) {
        return None;
    }
    let year: i64 = label[..4].parse().ok()?;
    let month: i64 = label[5..7].parse().ok()?;
    Some(year * 12 + month - 1)
}

fn month_label(key: i64) -> String {
    let year = key.div_euclid(12);
    let month = key.rem_euclid(12) + 1;
    format!("{year:04}-{month:02}")
}

/// The first day of a month label, the date the index lookup asks with.
fn month_date(month: &str) -> String {
    format!("{month}-01")
}

/// The report's months, oldest first, ending at the newest transaction's month.
fn report_months(view: &[Directive], months: u32) -> Vec<String> {
    if months == 0 {
        return Vec::new();
    }
    let latest = view
        .iter()
        .filter_map(|directive| match directive {
            Directive::Transaction(transaction) => transaction
                .date
                .to_string()
                .get(0..7)
                .filter(|label| is_year_month(label))
                .map(str::to_owned),
            _ => None,
        })
        .max();
    let Some(latest) = latest.and_then(|label| month_key(&label)) else {
        return Vec::new();
    };
    (0..i64::from(months))
        .rev()
        .map(|offset| month_label(latest - offset))
        .collect()
}

/// Whether `account` is `prefix` or one of its sub-accounts.
fn is_under(account: &str, prefix: &str) -> bool {
    account == prefix
        || (account.len() > prefix.len()
            && account.starts_with(prefix)
            && account.as_bytes()[prefix.len()] == b':')
}

fn account_root(account: &str) -> &str {
    account.split(':').next().unwrap_or(account)
}

/// The two-level expense branch an account rolls up into, with its label:
/// `Expenses:Food:Groceries:Organic` is
/// `("Expenses:Food:Groceries", "Food:Groceries")`, and an account with only
/// one level below `Expenses` keeps that level. `None` for an account that is
/// not under `Expenses`.
fn expense_branch(account: &str) -> Option<(String, String)> {
    let mut parts = account.split(':');
    if parts.next()? != "Expenses" {
        return None;
    }
    let top = parts.next()?;
    let mut branch = top.to_owned();
    if let Some(second) = parts.next() {
        branch.push(':');
        branch.push_str(second);
    }
    Some((format!("Expenses:{branch}"), branch))
}

/// A posting's complete units, or `None` for a leg the booking pass left
/// incomplete (which does not happen post-booking).
fn posting_units(posting: &Posting) -> Option<(Decimal, &str)> {
    match &posting.units {
        Some(IncompleteAmount::Complete(amount)) => Some((amount.number, amount.currency.as_str())),
        _ => None,
    }
}

/// The one commodity a report uses: the ledger's first operating currency the
/// figure map holds, else the commodity carrying the largest absolute figure,
/// ties broken by name so two loads agree.
fn report_currency(ledger: &Ledger, held: &BTreeMap<String, Decimal>) -> Option<String> {
    for currency in &ledger.options.operating_currency {
        if held.contains_key(currency) {
            return Some(currency.clone());
        }
    }
    held.iter()
        .max_by(|a, b| a.1.abs().cmp(&b.1.abs()).then_with(|| a.0.cmp(b.0)))
        .map(|(currency, _)| currency.clone())
}

/// Net worth at the end of each of the last `months` months.
///
/// Net worth is the booked balance of every `Assets:` and `Liabilities:`
/// account combined. Beancount keeps liabilities negative, so the sum is
/// already assets minus debts; no sign is flipped here.
pub fn net_worth_months(ledger: &Ledger, months: u32) -> Vec<NetWorthMonth> {
    let view = ledger.balance_view();
    let range = report_months(&view, months);
    if range.is_empty() {
        return Vec::new();
    }
    // One row per account-month posting would be too many; the sums collapse
    // repeats, and the running comparison below then asks each month which of
    // them had happened by its end.
    let mut totals: BTreeMap<(String, String), Decimal> = BTreeMap::new();
    let mut held: BTreeMap<String, Decimal> = BTreeMap::new();
    for directive in &view {
        let Directive::Transaction(transaction) = directive else {
            continue;
        };
        let date = transaction.date.to_string();
        let Some(month) = date.get(0..7).filter(|label| is_year_month(label)) else {
            continue;
        };
        for posting in &transaction.postings {
            let account = posting.value.account.as_str();
            if !is_under(account, "Assets") && !is_under(account, "Liabilities") {
                continue;
            }
            let Some((amount, currency)) = posting_units(&posting.value) else {
                continue;
            };
            *held.entry(currency.to_owned()).or_default() += amount.abs();
            *totals
                .entry((month.to_owned(), currency.to_owned()))
                .or_default() += amount;
        }
    }
    let Some(currency) = report_currency(ledger, &held) else {
        return Vec::new();
    };
    range
        .into_iter()
        .map(|month| {
            let mut value = Decimal::ZERO;
            for ((entry_month, entry_currency), amount) in &totals {
                if entry_currency == &currency && entry_month.as_str() <= month.as_str() {
                    value += amount;
                }
            }
            NetWorthMonth {
                month,
                value: value.to_string(),
                currency: currency.clone(),
            }
        })
        .collect()
}

/// Income and expenses in each of the last `months` months.
///
/// Inflow is the month's `Income:` postings with their sign flipped (income is
/// negative in Beancount), outflow the month's `Expenses:` postings as booked.
/// Both are reported as positive figures when the ledger was written the usual
/// way; a reversal posting can make either negative, which is left alone rather
/// than hidden.
pub fn cashflow_months(ledger: &Ledger, months: u32) -> Vec<CashflowMonth> {
    let view = ledger.balance_view();
    let range = report_months(&view, months);
    if range.is_empty() {
        return Vec::new();
    }
    #[derive(Default)]
    struct Flow {
        inflow: Decimal,
        outflow: Decimal,
    }
    let mut flows: BTreeMap<(String, String), Flow> = BTreeMap::new();
    let mut held: BTreeMap<String, Decimal> = BTreeMap::new();
    for directive in &view {
        let Directive::Transaction(transaction) = directive else {
            continue;
        };
        let date = transaction.date.to_string();
        let Some(month) = date.get(0..7).filter(|label| is_year_month(label)) else {
            continue;
        };
        for posting in &transaction.postings {
            let account = posting.value.account.as_str();
            let Some((amount, currency)) = posting_units(&posting.value) else {
                continue;
            };
            let flow = flows
                .entry((month.to_owned(), currency.to_owned()))
                .or_default();
            match account_root(account) {
                "Income" => {
                    *held.entry(currency.to_owned()).or_default() += amount.abs();
                    flow.inflow -= amount;
                }
                "Expenses" => {
                    *held.entry(currency.to_owned()).or_default() += amount.abs();
                    flow.outflow += amount;
                }
                _ => {}
            }
        }
    }
    let Some(currency) = report_currency(ledger, &held) else {
        return Vec::new();
    };
    range
        .into_iter()
        .map(|month| {
            let flow = flows.get(&(month.clone(), currency.clone()));
            CashflowMonth {
                month,
                inflow: flow.map_or_else(|| "0".to_owned(), |flow| flow.inflow.to_string()),
                outflow: flow.map_or_else(|| "0".to_owned(), |flow| flow.outflow.to_string()),
                currency: currency.clone(),
            }
        })
        .collect()
}

/// Spending per expense branch in each of the last `months` months.
///
/// Accounts are grouped at the top two levels under `Expenses`, so
/// `Expenses:Food:Groceries:Organic` is reported under
/// `Expenses:Food:Groceries`, with the label `Food:Groceries`. A branch with no
/// spending in the range is left out; a month with none inside a listed branch
/// reports `"0"`.
pub fn category_months(ledger: &Ledger, months: u32) -> Vec<CategoryMonths> {
    let view = ledger.balance_view();
    let range = report_months(&view, months);
    if range.is_empty() {
        return Vec::new();
    }
    let mut spent: BTreeMap<(String, String, String), Decimal> = BTreeMap::new();
    let mut labels: BTreeMap<String, String> = BTreeMap::new();
    let mut held: BTreeMap<String, Decimal> = BTreeMap::new();
    for directive in &view {
        let Directive::Transaction(transaction) = directive else {
            continue;
        };
        let date = transaction.date.to_string();
        let Some(month) = date.get(0..7).filter(|label| is_year_month(label)) else {
            continue;
        };
        for posting in &transaction.postings {
            let account = posting.value.account.as_str();
            let Some((branch, label)) = expense_branch(account) else {
                continue;
            };
            let Some((amount, currency)) = posting_units(&posting.value) else {
                continue;
            };
            *held.entry(currency.to_owned()).or_default() += amount.abs();
            labels.entry(branch.clone()).or_insert(label);
            *spent
                .entry((branch, month.to_owned(), currency.to_owned()))
                .or_default() += amount;
        }
    }
    let Some(currency) = report_currency(ledger, &held) else {
        return Vec::new();
    };
    let first = range.first().map(String::as_str).unwrap_or_default();
    let last = range.last().map(String::as_str).unwrap_or_default();
    let mut by_branch: BTreeMap<String, BTreeMap<String, Decimal>> = BTreeMap::new();
    for ((branch, month, entry_currency), amount) in spent {
        if entry_currency != currency || month.as_str() < first || month.as_str() > last {
            continue;
        }
        by_branch.entry(branch).or_default().insert(month, amount);
    }
    by_branch
        .into_iter()
        .filter_map(|(account, months_spent)| {
            let label = labels.remove(&account)?;
            let months = range
                .iter()
                .map(|month| CategoryMonth {
                    month: month.clone(),
                    spent: months_spent
                        .get(month)
                        .copied()
                        .unwrap_or_default()
                        .to_string(),
                })
                .collect();
            Some(CategoryMonths {
                account,
                label,
                currency: currency.clone(),
                months,
            })
        })
        .collect()
}

/// Spending over the last `months` months, restated in the earliest month's
/// money with `series`.
///
/// The nominal figures are the same expense walk as [`category_months`]; the
/// real figures pass each month's nominal total through
/// [`real_amount`] with `base` set to the range's first month, so the first
/// month's real figure equals its nominal one. The report refuses a series in a
/// different currency than the ledger's spending, and a series that does not
/// reach back to the range's first month — in both cases there is no honest
/// number to show.
pub fn lifestyle_report(
    ledger: &Ledger,
    months: u32,
    series: &IndexSeries,
) -> Result<LifestyleReport, String> {
    let view = ledger.balance_view();
    let range = report_months(&view, months);
    let index = LifestyleIndex {
        id: series.id.clone(),
        label: series.label.clone(),
        change: None,
    };
    if range.is_empty() {
        return Ok(LifestyleReport {
            index,
            months: Vec::new(),
            totals: Vec::new(),
            categories: Vec::new(),
        });
    }

    // The expense walk, kept per posting until the report's commodity is known.
    struct ExpenseRow {
        month: String,
        group: Option<String>,
        currency: String,
        amount: Decimal,
    }
    let mut rows: Vec<ExpenseRow> = Vec::new();
    let mut held: BTreeMap<String, Decimal> = BTreeMap::new();
    let mut labels: BTreeMap<String, String> = BTreeMap::new();
    for directive in &view {
        let Directive::Transaction(transaction) = directive else {
            continue;
        };
        let date = transaction.date.to_string();
        let Some(month) = date.get(0..7).filter(|label| is_year_month(label)) else {
            continue;
        };
        for posting in &transaction.postings {
            let account = posting.value.account.as_str();
            if account_root(account) != "Expenses" {
                continue;
            }
            let Some((amount, currency)) = posting_units(&posting.value) else {
                continue;
            };
            *held.entry(currency.to_owned()).or_default() += amount.abs();
            let branch = expense_branch(account);
            if let Some((branch, label)) = &branch {
                labels
                    .entry(branch.clone())
                    .or_insert_with(|| label.clone());
            }
            rows.push(ExpenseRow {
                month: month.to_owned(),
                group: branch.map(|(branch, _)| branch),
                currency: currency.to_owned(),
                amount,
            });
        }
    }
    let Some(currency) = report_currency(ledger, &held) else {
        return Ok(LifestyleReport {
            index,
            months: Vec::new(),
            totals: Vec::new(),
            categories: Vec::new(),
        });
    };
    // An exchange rate is not a price level: restating spending with it divides
    // one month's rate by another's and calls the result inflation.
    if matches!(series.id.as_str(), "ecb-usd" | "ecb-try") {
        return Err(format!(
            "`{}` is a reference exchange rate, not a price level; choose a consumer or producer price index",
            series.id
        ));
    }
    if series.currency != currency {
        return Err(format!(
            "price index `{}` measures {}, but the report is in {currency}",
            series.id, series.currency
        ));
    }

    let first = range.first().cloned().unwrap_or_default();
    let last = range.last().cloned().unwrap_or_default();
    let mut monthly: BTreeMap<String, Decimal> = BTreeMap::new();
    let mut categories: BTreeMap<String, BTreeMap<String, Decimal>> = BTreeMap::new();
    for row in rows {
        if row.currency != currency {
            continue;
        }
        *monthly.entry(row.month.clone()).or_default() += row.amount;
        if row.month.as_str() < first.as_str() || row.month.as_str() > last.as_str() {
            continue;
        }
        if let Some(group) = row.group {
            *categories
                .entry(group)
                .or_default()
                .entry(row.month)
                .or_default() += row.amount;
        }
    }

    let mut index_months = Vec::with_capacity(range.len());
    let mut index_values: Vec<Decimal> = Vec::with_capacity(range.len());
    for month in &range {
        let date = month_date(month);
        let value = index_at(series, &date).ok_or_else(|| missing_point(series, &date))?;
        index_values.push(value);
        index_months.push(LifestyleIndexMonth {
            month: month.clone(),
            index_value: value.to_string(),
        });
    }
    let change = match index_values.as_slice() {
        [first, .., last] => percentage_change(*first, *last),
        _ => None,
    };

    let mut totals = Vec::with_capacity(range.len());
    for month in &range {
        let nominal = monthly.get(month).copied().unwrap_or_default();
        totals.push(LifestyleTotal {
            month: month.clone(),
            nominal: nominal.to_string(),
            real: real_string(nominal, month, &first, series)?,
            currency: currency.clone(),
        });
    }

    let mut category_reports = Vec::with_capacity(categories.len());
    for (account, months_spent) in categories {
        let Some(label) = labels.get(&account) else {
            continue;
        };
        let mut month_rows = Vec::with_capacity(range.len());
        for month in &range {
            let nominal = months_spent.get(month).copied().unwrap_or_default();
            month_rows.push(LifestyleCategoryMonth {
                month: month.clone(),
                nominal: nominal.to_string(),
                real: real_string(nominal, month, &first, series)?,
            });
        }
        let (nominal_change, real_change) = if month_rows.len() >= 2 {
            let previous = &month_rows[month_rows.len() - 2];
            let latest = &month_rows[month_rows.len() - 1];
            (
                percentage_change(decimal(&previous.nominal), decimal(&latest.nominal)),
                percentage_change(decimal(&previous.real), decimal(&latest.real)),
            )
        } else {
            (None, None)
        };
        category_reports.push(LifestyleCategory {
            account,
            label: label.clone(),
            currency: currency.clone(),
            months: month_rows,
            nominal_change,
            real_change,
        });
    }

    Ok(LifestyleReport {
        index: LifestyleIndex { change, ..index },
        months: index_months,
        totals,
        categories: category_reports,
    })
}

/// A report string back to [`Decimal`] — only for values this module produced,
/// so the zero fallback is unreachable but keeps the change arithmetic total.
fn decimal(text: &str) -> Decimal {
    text.parse().unwrap_or(Decimal::ZERO)
}

fn real_string(
    amount: Decimal,
    month: &str,
    base: &str,
    series: &IndexSeries,
) -> Result<String, String> {
    real_amount(
        &amount.to_string(),
        &month_date(month),
        &month_date(base),
        series,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::IndexPoint;
    use crate::session::load_journal;
    use crate::testing::TempDir;
    use std::fs;

    /// Two months, two expense branches, one asset and one liability: every
    /// report below reads the same figures.
    const FIXTURE: &str = "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank
2026-01-01 open Liabilities:Card
2026-01-01 open Income:Salary
2026-01-01 open Expenses:Food:Groceries:Organic
2026-01-01 open Expenses:Fun

2026-01-15 * \"Employer\"
  Assets:Bank               1000.00 USD
  Income:Salary            -1000.00 USD

2026-01-20 * \"Market\"
  Expenses:Food:Groceries:Organic    120.00 USD
  Assets:Bank               -120.00 USD

2026-02-10 * \"Restaurant\"
  Expenses:Fun                80.00 USD
  Liabilities:Card           -80.00 USD

2026-02-25 * \"Employer\"
  Assets:Bank                500.00 USD
  Income:Salary             -500.00 USD

2026-02-26 * \"Market\"
  Expenses:Food:Groceries:Organic     60.00 USD
  Assets:Bank                -60.00 USD
";

    fn ledger() -> Ledger {
        let dir = TempDir::new("reports");
        let path = dir.file("main.bean");
        fs::write(&path, FIXTURE).unwrap();
        load_journal(&path).unwrap()
    }

    fn series() -> IndexSeries {
        IndexSeries::new(
            "us-cpi",
            "US CPI",
            "FRED",
            false,
            "USD",
            vec![
                IndexPoint::new("2026-01", "100").unwrap(),
                IndexPoint::new("2026-02", "125").unwrap(),
            ],
        )
        .unwrap()
    }

    /// Net worth is assets plus liabilities, carried to each month's end:
    /// January closes at 880.00, February at 1240.00.
    #[test]
    fn net_worth_carries_balances_to_each_month_end() {
        let report = net_worth_months(&ledger(), 2);
        assert_eq!(
            report
                .iter()
                .map(|row| (
                    row.month.as_str(),
                    row.value.as_str(),
                    row.currency.as_str()
                ))
                .collect::<Vec<_>>(),
            [("2026-01", "880.00", "USD"), ("2026-02", "1240.00", "USD")]
        );
    }

    /// Cash flow reads the month's income and expense postings, both positive.
    #[test]
    fn cashflow_sums_income_and_expenses_per_month() {
        let report = cashflow_months(&ledger(), 2);
        assert_eq!(
            report
                .iter()
                .map(|row| {
                    (
                        row.month.as_str(),
                        row.inflow.as_str(),
                        row.outflow.as_str(),
                        row.currency.as_str(),
                    )
                })
                .collect::<Vec<_>>(),
            [
                ("2026-01", "1000.00", "120.00", "USD"),
                ("2026-02", "500.00", "140.00", "USD")
            ]
        );
    }

    /// Categories are the two levels under `Expenses`, with zero months filled
    /// in so a chart's x-axis is the range rather than the months that happened
    /// to spend.
    #[test]
    fn categories_roll_up_to_two_levels_and_fill_empty_months() {
        let report = category_months(&ledger(), 2);
        assert_eq!(report.len(), 2);
        assert_eq!(
            report
                .iter()
                .map(|category| {
                    (
                        category.account.as_str(),
                        category.label.as_str(),
                        category.currency.as_str(),
                        category
                            .months
                            .iter()
                            .map(|month| (month.month.as_str(), month.spent.as_str()))
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>(),
            [
                (
                    "Expenses:Food:Groceries",
                    "Food:Groceries",
                    "USD",
                    vec![("2026-01", "120.00"), ("2026-02", "60.00")]
                ),
                (
                    "Expenses:Fun",
                    "Fun",
                    "USD",
                    vec![("2026-01", "0"), ("2026-02", "80.00")]
                ),
            ]
        );
    }

    /// The lifestyle report restates each month in January's money: February's
    /// 140.00 nominal is 112.00 real, and Food's fall is -50% nominal but -60%
    /// real.
    #[test]
    fn lifestyle_restates_spending_in_the_first_months_money() {
        let report = lifestyle_report(&ledger(), 2, &series()).unwrap();
        assert_eq!(report.index.id, "us-cpi");
        assert_eq!(report.index.change.as_deref(), Some("25.00"));
        assert_eq!(
            report
                .months
                .iter()
                .map(|month| (month.month.as_str(), month.index_value.as_str()))
                .collect::<Vec<_>>(),
            [("2026-01", "100"), ("2026-02", "125")]
        );
        assert_eq!(
            report
                .totals
                .iter()
                .map(|total| {
                    (
                        total.month.as_str(),
                        total.nominal.as_str(),
                        total.real.as_str(),
                        total.currency.as_str(),
                    )
                })
                .collect::<Vec<_>>(),
            [
                ("2026-01", "120.00", "120.00", "USD"),
                ("2026-02", "140.00", "112.00", "USD")
            ]
        );
        let food = &report.categories[0];
        assert_eq!(food.account, "Expenses:Food:Groceries");
        assert_eq!(
            food.months
                .iter()
                .map(|month| {
                    (
                        month.month.as_str(),
                        month.nominal.as_str(),
                        month.real.as_str(),
                    )
                })
                .collect::<Vec<_>>(),
            [
                ("2026-01", "120.00", "120.00"),
                ("2026-02", "60.00", "48.00")
            ]
        );
        assert_eq!(food.nominal_change.as_deref(), Some("-50.00"));
        assert_eq!(food.real_change.as_deref(), Some("-60.00"));

        let fun = &report.categories[1];
        assert_eq!(fun.account, "Expenses:Fun");
        assert_eq!(fun.nominal_change, None);
        assert_eq!(fun.real_change, None);
    }

    /// A one-month range has no previous month, so it reports no percentage
    /// change rather than a fabricated one.
    #[test]
    fn lifestyle_with_one_month_has_no_change() {
        let report = lifestyle_report(&ledger(), 1, &series()).unwrap();
        assert_eq!(report.months.len(), 1);
        assert_eq!(report.index.change, None);
        assert!(report.categories.iter().all(|category| {
            category.nominal_change.is_none() && category.real_change.is_none()
        }));
    }

    /// A series in another currency, or one that does not reach the first month
    /// of the range, is refused with the series named.
    #[test]
    fn lifestyle_refuses_a_series_it_cannot_use() {
        let wrong_currency = IndexSeries::new(
            "tr-cpi",
            "Türkiye CPI",
            "TÜİK",
            false,
            "TRY",
            vec![
                IndexPoint::new("2026-01", "100").unwrap(),
                IndexPoint::new("2026-02", "125").unwrap(),
            ],
        )
        .unwrap();
        let error = lifestyle_report(&ledger(), 2, &wrong_currency).unwrap_err();
        assert!(error.contains("tr-cpi"), "{error}");
        assert!(error.contains("USD"), "{error}");

        let late = IndexSeries::new(
            "us-cpi",
            "US CPI",
            "FRED",
            false,
            "USD",
            vec![IndexPoint::new("2026-02", "125").unwrap()],
        )
        .unwrap();
        let error = lifestyle_report(&ledger(), 2, &late).unwrap_err();
        assert!(error.contains("us-cpi"), "{error}");
        assert!(error.contains("2026-01-01"), "{error}");
    }

    /// No ledger data is an empty report rather than an error.
    #[test]
    fn an_empty_ledger_reports_nothing() {
        let dir = TempDir::new("reports-empty");
        let path = dir.file("main.bean");
        fs::write(&path, "option \"operating_currency\" \"USD\"\n").unwrap();
        let ledger = load_journal(&path).unwrap();
        assert!(net_worth_months(&ledger, 3).is_empty());
        assert!(cashflow_months(&ledger, 3).is_empty());
        assert!(category_months(&ledger, 3).is_empty());
        let report = lifestyle_report(&ledger, 3, &series()).unwrap();
        assert!(report.months.is_empty());
        assert!(report.totals.is_empty());
    }
}
