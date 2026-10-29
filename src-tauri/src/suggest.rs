// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Quick add: what the phone's sheet opens with — the accounts the last entry
//! moved money between, the categories used lately, and the payees worth
//! offering again.
//!
//! The sheet used to derive this in the browser from the whole register it had
//! downloaded. Now the book answers it directly: one cheap scan for the newest
//! date that anchors the window, then one pass over the window's transactions
//! (the booked stream the register shows), so the sheet opens the same way with
//! no register in memory and the counting happens where the ledger is.
//!
//! # The window
//!
//! "Recent" counts back `days` days from the ledger's **newest transaction**,
//! not from the wall clock. A fixture, a freshly opened journal and a book the
//! user is writing all then mean the same thing, and the answer never depends
//! on the machine's clock or time zone — the convention the reports already
//! follow for their month range. A book nobody has written to for a year still
//! suggests what its own last entries did, which is what a quick add on an old
//! book should do. `days == 0` is no window at all: every entry counts.
//!
//! # What it answers
//!
//! - **The pair**: the newest transaction in the window (greatest date, ties
//!   broken by its position in the ledger stream) that moves money between a
//!   funding account (`Assets`/`Liabilities`) and a category account
//!   (`Expenses`/`Income`). Its
//!   two account names are how "last used" is read back. The legs are matched
//!   against the ledger's own `option "name_*"` roots, so a renamed chart still
//!   classifies correctly. A pair naming an account that is no longer in the
//!   account tree is dropped rather than offered: the sheet can only prefill an
//!   account the book still has.
//! - **Categories**: how often each category account appears inside the window,
//!   counted once per transaction (a split names several; each counts once).
//!   Ordered by count, highest first, ties by plain code-unit name order — no
//!   locale can change the answer.
//! - **Payees**: the payee, or the narration when there is none, counted once
//!   per transaction inside the same window and ordered the same way.
//!
//! Both lists are capped at [`MAX_SUGGESTIONS`]. The lists are what the keypad
//! offers, not a report: a caller wanting the whole book should ask the ledger
//! its own question.

use std::collections::{BTreeMap, BTreeSet};

use rustledger_core::{Directive, NaiveDate};
use rustledger_loader::Ledger;
use rustledger_query::{Interval, IntervalUnit};

use crate::model::AccountBalance;

/// How many entries each suggestion list carries — the sheet offers eight
/// rows, the same number it always has.
const MAX_SUGGESTIONS: usize = 8;

/// The sheet's opening guesses: the remembered pair, the categories, the
/// payees.
///
/// `Default` is the empty book's answer — no pair, no lists — so a ledger with
/// no entries needs no special case at the caller.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct QuickAdd {
    /// The most recent funding/category pair, as `(from, to)`: the account the
    /// money came from and the category it went to.
    pub last_pair: Option<(String, String)>,
    /// Category accounts by recent use, most used first; at most
    /// [`MAX_SUGGESTIONS`].
    pub categories: Vec<String>,
    /// Payees (or narrations) by recent use, most used first; at most
    /// [`MAX_SUGGESTIONS`].
    pub payees: Vec<String>,
}

/// The sheet's guesses from the ledger's last `days` days.
///
/// `accounts` is the account tree the sidebar shows ([`crate::accounts`'s
/// view](crate::model::AccountBalance)); it vets [`QuickAdd::last_pair`], since
/// a remembered account can disappear from the book between two opens. An empty
/// slice is "no tree to vet against" and keeps the pair rather than blanking it.
pub fn quick_add(ledger: &Ledger, accounts: &[AccountBalance], days: u32) -> QuickAdd {
    let view = ledger.balance_view();
    let Some(newest) = view
        .iter()
        .filter_map(|directive| match directive {
            Directive::Transaction(transaction) => Some(transaction.date),
            _ => None,
        })
        .max()
    else {
        return QuickAdd::default();
    };
    // Day arithmetic through the query crate's public `Interval`: it is the
    // tree's one date-shifting helper that does not need a new dependency.
    let cutoff = (days != 0)
        .then(|| Interval::new(-i64::from(days), IntervalUnit::Day).add_to_date(newest))
        .flatten();

    let funding_roots = [
        &ledger.options.name_assets,
        &ledger.options.name_liabilities,
    ];
    let category_roots = [&ledger.options.name_expenses, &ledger.options.name_income];

    let mut category_counts: BTreeMap<&str, u32> = BTreeMap::new();
    let mut payee_counts: BTreeMap<&str, u32> = BTreeMap::new();
    let mut pair: Option<(NaiveDate, usize, &str, &str)> = None;

    for (index, directive) in view.iter().enumerate() {
        let Directive::Transaction(transaction) = directive else {
            continue;
        };
        if cutoff.is_some_and(|cutoff| transaction.date < cutoff) {
            continue;
        }
        let mut funding = None;
        let mut category = None;
        let mut categories: BTreeSet<&str> = BTreeSet::new();
        for posting in &transaction.postings {
            let account = posting.value.account.as_str();
            let root = account.split(':').next().unwrap_or(account);
            if funding.is_none() && funding_roots.iter().any(|name| name.as_str() == root) {
                funding = Some(account);
            }
            if category_roots.iter().any(|name| name.as_str() == root) {
                // The pair reads as the entry's first category leg; the count
                // is once per entry no matter how many it names.
                if category.is_none() {
                    category = Some(account);
                }
                categories.insert(account);
            }
        }
        for account in &categories {
            *category_counts.entry(account).or_default() += 1;
        }
        if let (Some(funding), Some(category)) = (funding, category) {
            let newer = pair.is_none_or(|(date, seen, _, _)| {
                transaction.date > date || (transaction.date == date && index > seen)
            });
            if newer {
                pair = Some((transaction.date, index, funding, category));
            }
        }

        let text = transaction
            .payee
            .as_ref()
            .map_or_else(|| transaction.narration.as_str(), |payee| payee.as_str())
            .trim();
        if !text.is_empty() {
            *payee_counts.entry(text).or_default() += 1;
        }
    }

    let known: BTreeSet<&str> = accounts
        .iter()
        .map(|account| account.name.as_str())
        .collect();
    let last_pair = pair
        .map(|(_, _, from, to)| (from, to))
        .filter(|(from, to)| known.is_empty() || (known.contains(from) && known.contains(to)))
        .map(|(from, to)| (from.to_string(), to.to_string()));

    QuickAdd {
        last_pair,
        categories: ranked(&category_counts),
        payees: ranked(&payee_counts),
    }
}

/// The most used names first, ties in plain code-unit order, capped.
fn ranked<'a>(counts: &BTreeMap<&'a str, u32>) -> Vec<String> {
    let mut entries: Vec<(&str, u32)> =
        counts.iter().map(|(name, count)| (*name, *count)).collect();
    entries.sort_by(|(left_name, left_count), (right_name, right_count)| {
        right_count
            .cmp(left_count)
            .then_with(|| left_name.cmp(right_name))
    });
    entries
        .into_iter()
        .take(MAX_SUGGESTIONS)
        .map(|(name, _)| name.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::load_journal;
    use crate::testing::TempDir;
    use std::fs;

    /// Four entries with a clear "last used" answer: the salary pair is the
    /// oldest, groceries the newest, coffee the most frequent category.
    const JOURNAL: &str = "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank:Checking
2026-01-01 open Assets:Cash
2026-01-01 open Expenses:Coffee
2026-01-01 open Expenses:Food
2026-01-01 open Income:Salary

2026-03-01 * \"Employer\" \"Salary\"
  Assets:Bank:Checking        1000.00 USD
  Income:Salary              -1000.00 USD

2026-03-02 * \"Coffee Shop\" \"Espresso\"
  Expenses:Coffee               4.50 USD
  Assets:Bank:Checking

2026-03-03 * \"Cafe\" \"Flat white\"
  Expenses:Coffee               3.25 USD
  Assets:Cash

2026-03-04 * \"Supermarket\" \"Bread\"
  Expenses:Food                12.25 USD
  Assets:Bank:Checking
";

    fn ledger_of(text: &str) -> Ledger {
        let dir = TempDir::new("suggest");
        let path = dir.file("main.bean");
        fs::write(&path, text).unwrap();
        load_journal(&path).expect("the fixture loads")
    }

    /// The account tree a caller would pass: every account the book names.
    fn tree(ledger: &Ledger) -> Vec<AccountBalance> {
        crate::accounts::account_balances(ledger, &BTreeSet::new())
    }

    #[test]
    fn categories_rank_by_count_then_name() {
        let ledger = ledger_of(JOURNAL);
        let quick = quick_add(&ledger, &tree(&ledger), 0);
        assert_eq!(
            quick.categories,
            vec![
                "Expenses:Coffee".to_string(),
                "Expenses:Food".to_string(),
                "Income:Salary".to_string(),
            ],
            "two coffees beat the single grocery and salary, which tie and sort by name"
        );
    }

    #[test]
    fn the_pair_is_the_newest_entry_with_both_legs() {
        let ledger = ledger_of(JOURNAL);
        let quick = quick_add(&ledger, &tree(&ledger), 0);
        assert_eq!(
            quick.last_pair,
            Some((
                "Assets:Bank:Checking".to_string(),
                "Expenses:Food".to_string()
            )),
            "2026-03-04 is the newest entry with a funding and a category leg"
        );
    }

    #[test]
    fn the_window_filters_categories_payees_and_the_pair() {
        let ledger = ledger_of(JOURNAL);
        // Two days back from the newest entry (2026-03-04) reaches 03-02: the
        // salary's pair (03-01) is out, and so is its payee.
        let quick = quick_add(&ledger, &tree(&ledger), 2);
        assert_eq!(
            quick.categories,
            vec!["Expenses:Coffee".to_string(), "Expenses:Food".to_string()],
            "two coffees, one grocery — the salary is outside the window"
        );
        assert_eq!(
            quick.payees,
            vec![
                "Cafe".to_string(),
                "Coffee Shop".to_string(),
                "Supermarket".to_string(),
            ],
            "one each, ties by name; \"Employer\" is outside the window"
        );
        assert_eq!(
            quick.last_pair,
            Some((
                "Assets:Bank:Checking".to_string(),
                "Expenses:Food".to_string()
            ))
        );

        // One day back from the newest entry reaches 03-03, so only the last
        // two entries are in: both categories tie at one and the name decides.
        let narrow = quick_add(&ledger, &tree(&ledger), 1);
        assert_eq!(
            narrow.categories,
            vec!["Expenses:Coffee".to_string(), "Expenses:Food".to_string()]
        );
        assert_eq!(
            narrow.payees,
            vec!["Cafe".to_string(), "Supermarket".to_string()],
            "\"Coffee Shop\" is a day too old"
        );
        assert_eq!(
            narrow.last_pair,
            Some((
                "Assets:Bank:Checking".to_string(),
                "Expenses:Food".to_string()
            ))
        );
    }

    #[test]
    fn an_empty_ledger_answers_the_default() {
        let ledger = ledger_of(
            "\
2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food
",
        );
        assert_eq!(quick_add(&ledger, &tree(&ledger), 90), QuickAdd::default());
        assert_eq!(quick_add(&ledger, &[], 90), QuickAdd::default());
    }

    #[test]
    fn income_is_a_category_too_when_it_is_the_newest() {
        let ledger = ledger_of(JOURNAL);
        // Back from the salary (2026-03-01) far enough to include only it:
        // a window measured from the newest transaction still starts at the
        // newest, so instead ask a book whose newest entry is the salary.
        let salary_book = ledger_of(
            "\
2026-01-01 open Assets:Bank:Checking
2026-01-01 open Income:Salary

2026-03-01 * \"Employer\" \"Salary\"
  Assets:Bank:Checking        1000.00 USD
  Income:Salary              -1000.00 USD
",
        );
        let quick = quick_add(&salary_book, &tree(&salary_book), 30);
        assert_eq!(
            quick.last_pair,
            Some((
                "Assets:Bank:Checking".to_string(),
                "Income:Salary".to_string()
            ))
        );
        assert_eq!(quick.categories, vec!["Income:Salary".to_string()]);
        // The four-entry book keeps its own answer: the income is out of its
        // window and its newest pair is groceries.
        assert_eq!(
            quick_add(&ledger, &tree(&ledger), 1).last_pair,
            Some((
                "Assets:Bank:Checking".to_string(),
                "Expenses:Food".to_string()
            ))
        );
    }

    #[test]
    fn a_pair_whose_account_left_the_tree_is_dropped() {
        let ledger = ledger_of(JOURNAL);
        let known: Vec<AccountBalance> = tree(&ledger)
            .into_iter()
            .filter(|account| account.name != "Expenses:Food")
            .collect();
        let quick = quick_add(&ledger, &known, 0);
        assert_eq!(
            quick.last_pair, None,
            "the sheet cannot prefill a gone account"
        );
        assert!(
            quick.categories.contains(&"Expenses:Food".to_string()),
            "the lists still reflect the book"
        );
    }

    #[test]
    fn the_payee_falls_back_to_the_narration() {
        let ledger = ledger_of(
            "\
2026-01-01 open Assets:Cash
2026-01-01 open Expenses:Food

2026-02-01 * \"Bakery run\"
  Expenses:Food                 2.00 USD
  Assets:Cash
",
        );
        let quick = quick_add(&ledger, &tree(&ledger), 30);
        assert_eq!(quick.payees, vec!["Bakery run".to_string()]);
    }

    #[test]
    fn a_split_counts_each_category_once_per_entry() {
        let ledger = ledger_of(
            "\
2026-01-01 open Assets:Cash
2026-01-01 open Expenses:Food
2026-01-01 open Expenses:Coffee

2026-02-01 * \"Corner shop\" \"Bread and a coffee\"
  Expenses:Food                 2.00 USD
  Expenses:Coffee               3.00 USD
  Assets:Cash                  -5.00 USD
",
        );
        let quick = quick_add(&ledger, &tree(&ledger), 30);
        assert_eq!(
            quick.categories,
            vec!["Expenses:Coffee".to_string(), "Expenses:Food".to_string()],
            "one entry, one count each — not one per posting"
        );
        assert_eq!(quick.payees, vec!["Corner shop".to_string()]);
        assert_eq!(
            quick.last_pair,
            Some(("Assets:Cash".to_string(), "Expenses:Food".to_string())),
            "the pair reads the entry's first category leg, in posting order"
        );
    }

    #[test]
    fn both_lists_stop_at_the_cap() {
        let mut text = String::from("2026-01-01 open Assets:Cash\n");
        for index in 0..(MAX_SUGGESTIONS + 4) {
            text.push_str(&format!("2026-01-01 open Expenses:Category{index:02}\n"));
        }
        for index in 0..(MAX_SUGGESTIONS + 4) {
            text.push_str(&format!(
                "2026-02-01 * \"Payee{index:02}\"\n  Expenses:Category{index:02}  1.00 USD\n  Assets:Cash\n\n"
            ));
        }
        let ledger = ledger_of(&text);
        let quick = quick_add(&ledger, &tree(&ledger), 0);
        assert_eq!(quick.categories.len(), MAX_SUGGESTIONS);
        assert_eq!(quick.payees.len(), MAX_SUGGESTIONS);
        assert_eq!(
            quick.categories.first().map(String::as_str),
            Some("Expenses:Category00"),
            "all counts tie, so the name order decides"
        );
    }
}
