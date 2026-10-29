// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The account tree and the register: subtree balances and the transactions
//! touching an account, as the frontend lists them.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use rustledger_core::{Decimal, Directive, IncompleteAmount};
use rustledger_loader::Ledger;

use crate::git::working_tree_changes;
use crate::model::{AccountBalance, PostingView, TransactionView};
use crate::source::{source_directive_index, touches_changes};

/// Accounts the ledger's envelope-budgeting plugin generates
/// (`plugins/rustledger-envelope`). The whole budget lives under this prefix, so
/// one string is enough to keep it out of the app — tree, balances and register
/// — while the plugin keeps doing the arithmetic.
const ENVELOPE_NAMESPACE: &str = "Equity:Envelope";

/// Whether `account` is one the envelope plugin generated.
fn is_envelope_account(account: &str) -> bool {
    account == ENVELOPE_NAMESPACE
        || (account.len() > ENVELOPE_NAMESPACE.len()
            && account.starts_with(ENVELOPE_NAMESPACE)
            && account.as_bytes()[ENVELOPE_NAMESPACE.len()] == b':')
}

/// Every colon-prefix of `account`, the account itself included, in prefix
/// order: `A:B:C` visits `A`, then `A:B`, then `A:B:C`.
fn for_each_account_prefix(account: &str, mut visit: impl FnMut(&str)) {
    let mut prefix = String::new();
    for part in account.split(':') {
        prefix = if prefix.is_empty() {
            part.to_owned()
        } else {
            format!("{prefix}:{part}")
        };
        visit(&prefix);
    }
}

/// The accounts touched by at least one uncommitted transaction, with every
/// colon-prefix ancestor of each — the same "changed since HEAD or since this
/// session's baseline" test [`account_transaction_views`] runs per row, rolled
/// up so a parent branch reads as changed when anything under it is.
///
/// Like the per-row flag, the whole walk is skipped when the working tree has
/// nothing to report, so a clean ledger costs one diff.
pub(crate) fn modified_accounts(
    ledger: &Ledger,
    baseline: &HashMap<PathBuf, Arc<str>>,
) -> BTreeSet<String> {
    let changed = working_tree_changes(ledger, baseline);
    let mut marked = BTreeSet::new();
    if changed.is_empty() {
        return marked;
    }
    let view = ledger.balance_view();
    let sources = source_directive_index(ledger, &view);
    let mut line_starts: HashMap<u16, Vec<usize>> = HashMap::new();
    for (id, directive) in view.iter().enumerate() {
        let Directive::Transaction(tx) = directive else {
            continue;
        };
        let modified = sources.get(id).copied().flatten().is_some_and(|index| {
            touches_changes(
                ledger,
                &changed,
                &mut line_starts,
                &ledger.directives[index],
            )
        });
        if !modified {
            continue;
        }
        for posting in &tx.postings {
            for_each_account_prefix(posting.value.account.as_str(), |prefix| {
                marked.insert(prefix.to_owned());
            });
        }
    }
    marked
}

/// Collect the transactions whose postings touch `account` or one of its
/// sub-accounts, newest first (stable, so equal dates keep ledger order), each
/// with every posting it contains. The `enumerate` index over `balance_view`
/// is the transaction's position in the ledger stream, a stable id for a file.
/// An empty `account` matches every transaction — the virtual root.
pub(crate) fn account_transaction_views(
    ledger: &Ledger,
    baseline: &HashMap<PathBuf, Arc<str>>,
    account: &str,
) -> Vec<TransactionView> {
    let view = ledger.balance_view();
    // Flagging rows costs a git diff plus a walk of the directive stream; both
    // are skipped when the working tree has nothing to report — no repository,
    // no change against HEAD or the session baseline — and every row then reads
    // as unmodified.
    let changed = working_tree_changes(ledger, baseline);
    let sources = (!changed.is_empty()).then(|| source_directive_index(ledger, &view));
    collect_transaction_views(ledger, &view, sources.as_deref(), &changed, account, None)
}

/// The register rows one journal file wrote: the same rows and ids
/// [`account_transaction_views`] gives the virtual root, narrowed to the
/// transactions whose source directive belongs to `file`.
///
/// The caller brings its own `view` and `sources` — the pair
/// [`source_directive_index`] builds for `ledger.balance_view()` — so the file
/// view, which needs both to answer which entries the file holds, pays for
/// neither a second `balance_view` clone nor a second source index. Only the
/// matching transactions are materialised: the rest are rejected by their file
/// id before a row is built.
pub(crate) fn account_transaction_views_in_file(
    ledger: &Ledger,
    baseline: &HashMap<PathBuf, Arc<str>>,
    view: &[Directive],
    sources: &[Option<usize>],
    file: usize,
) -> Vec<TransactionView> {
    let changed = working_tree_changes(ledger, baseline);
    collect_transaction_views(ledger, view, Some(sources), &changed, "", Some(file))
}

/// The register's rows for one account and, when `file` is named, for the
/// transactions that file holds — the one collector behind
/// [`account_transaction_views`] and [`account_transaction_views_in_file`].
///
/// `sources` may be absent when the caller flags nothing (a clean working
/// tree); a file filter always needs it, or no row could be placed in a file.
fn collect_transaction_views(
    ledger: &Ledger,
    view: &[Directive],
    sources: Option<&[Option<usize>]>,
    changed: &HashMap<u16, Vec<(u32, u32)>>,
    account: &str,
    file: Option<usize>,
) -> Vec<TransactionView> {
    let prefix = format!("{account}:");
    let root = account.is_empty();
    let mut line_starts: HashMap<u16, Vec<usize>> = HashMap::new();
    let mut views: Vec<TransactionView> = view
        .iter()
        .enumerate()
        .filter_map(|(id, directive)| {
            let Directive::Transaction(tx) = directive else {
                return None;
            };
            // The plugin's generated entries are bookkeeping, not entries the
            // user wrote: they never reach the register. (The id stays the
            // ledger-stream position, so the user's own rows keep theirs.)
            if tx
                .postings
                .iter()
                .all(|posting| is_envelope_account(posting.value.account.as_str()))
            {
                return None;
            }
            let touches = root
                || tx.postings.iter().any(|posting| {
                    let name = posting.value.account.as_str();
                    name == account || name.starts_with(&prefix)
                });
            if !touches {
                return None;
            }
            // A file filter reads the same source index the `modified` flag
            // does: the directive's own file id, not the file's path, decides
            // the row — a synthesized entry belongs to no file.
            if let Some(file) = file {
                let source = sources?.get(id).copied().flatten()?;
                if usize::from(ledger.directives[source].file_id) != file {
                    return None;
                }
            }
            let postings: Vec<PostingView> = tx
                .postings
                .iter()
                .filter_map(|posting| {
                    let Some(IncompleteAmount::Complete(amount)) = &posting.value.units else {
                        // Residual/interpolated postings never occur post-booking.
                        return None;
                    };
                    Some(PostingView {
                        account: posting.value.account.as_str().to_owned(),
                        units: amount.number.to_string(),
                        currency: amount.currency.as_str().to_owned(),
                    })
                })
                .collect();
            let modified = sources
                .and_then(|sources| sources.get(id).copied().flatten())
                .is_some_and(|index| {
                    touches_changes(
                        ledger,
                        changed,
                        &mut line_starts,
                        &ledger.directives[index],
                    )
                });
            Some(TransactionView {
                id: u32::try_from(id).unwrap_or(u32::MAX),
                date: tx.date.to_string(),
                flag: tx.flag.to_string(),
                payee: tx.payee.as_ref().map(|p| p.as_str().to_owned()),
                narration: tx.narration.as_str().to_owned(),
                postings,
                modified,
            })
        })
        .collect();
    // Stable sort: descending date, ties keep their ledger order.
    views.sort_by(|a, b| b.date.cmp(&a.date));
    views
}

/// Accumulate per-(account, currency) units over the pad-merged transaction
/// stream, roll each account's totals up into its ancestors, then report one
/// entry per tree node.
pub(crate) fn account_balances(
    ledger: &Ledger,
    modified: &BTreeSet<String>,
) -> Vec<AccountBalance> {
    let mut totals: HashMap<String, HashMap<String, Decimal>> = HashMap::new();
    for directive in ledger.balance_view() {
        let Directive::Transaction(tx) = directive else {
            continue;
        };
        for posting in &tx.postings {
            let Some(IncompleteAmount::Complete(amount)) = &posting.value.units else {
                // Residual/interpolated postings never occur post-booking.
                continue;
            };
            // The envelope plugin's postings are bookkeeping against accounts it
            // owns; letting them through would also inflate their ancestors, so
            // `Equity` would carry money the user does not have.
            if is_envelope_account(posting.value.account.as_str()) {
                continue;
            }
            *totals
                .entry(posting.value.account.as_str().to_owned())
                .or_default()
                .entry(amount.currency.as_str().to_owned())
                .or_default() += amount.number;
        }
    }
    // Roll every account's own totals into each colon-prefix ancestor (the
    // final prefix re-adds the account's own totals, which `drain` removed).
    // Adding to a parent that also has postings is correct: its entry then
    // holds own + descendants, exactly the subtree total.
    for (name, currencies) in totals.drain().collect::<Vec<_>>() {
        for_each_account_prefix(&name, |prefix| {
            let node = totals.entry(prefix.to_owned()).or_default();
            for (currency, units) in &currencies {
                *node.entry(currency.clone()).or_default() += units;
            }
        });
    }
    let mut balances: Vec<AccountBalance> = totals
        .iter()
        .map(|(name, held)| {
            let (currency, units) = primary_holding(held, &ledger.options.operating_currency);
            AccountBalance {
                name: name.clone(),
                balance: units.to_string(),
                currency,
                modified: modified.contains(name),
            }
        })
        .collect();
    balances.sort_by(|a, b| a.name.cmp(&b.name));
    balances
}

/// An account's primary commodity and its units in it: the first operating
/// currency the account holds, else the commodity with the largest absolute
/// position.
fn primary_holding(held: &HashMap<String, Decimal>, operating: &[String]) -> (String, Decimal) {
    for currency in operating {
        if let Some(units) = held.get(currency) {
            return (currency.clone(), *units);
        }
    }
    held.iter()
        .max_by_key(|(_, units)| units.abs())
        .map(|(currency, units)| (currency.clone(), *units))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use rustledger_loader::LoadOptions;

    #[test]
    fn computes_account_balances_with_pads_and_multi_currency() {
        let mut fs = rustledger_loader::VirtualFileSystem::new();
        fs.add_file(
            "main.beancount",
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank
2026-01-01 open Assets:Bank:Checking
2026-01-01 open Assets:Bank:Emergency
2026-01-01 open Assets:Cash
2026-01-01 open Expenses:Food
2026-01-01 open Income:Salary
2026-01-01 open Equity:Opening-Balances

2026-01-01 pad Assets:Bank:Checking Equity:Opening-Balances
2026-01-02 balance Assets:Bank:Checking 100.00 USD

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -50.00 USD
  Expenses:Food                    50.00 USD

2026-01-11 * \"Salary\"
  Assets:Bank:Checking            200.00 USD
  Income:Salary                  -200.00 USD

2026-01-12 * \"Cash\"
  Assets:Cash                      10.00 EUR @ 1.10 USD
  Assets:Bank:Checking            -11.00 USD

2026-01-13 * \"Transfer\"
  Assets:Bank                     -30.00 USD
  Assets:Bank:Emergency            30.00 USD
",
        );
        let ledger = rustledger_loader::load_with_fs(
            Path::new("main.beancount"),
            &LoadOptions::default(),
            Box::new(fs),
        )
        .unwrap();
        assert!(ledger.errors.is_empty(), "errors: {:?}", ledger.errors);

        let balances = account_balances(&ledger, &BTreeSet::new());
        let get = |name: &str| {
            balances
                .iter()
                .find(|b| b.name == name)
                .map(|b| b.balance.as_str())
                .unwrap_or_else(|| panic!("missing {name} in {balances:?}"))
        };
        let currency = |name: &str| {
            balances
                .iter()
                .find(|b| b.name == name)
                .map(|b| b.currency.as_str())
                .unwrap_or_else(|| panic!("missing {name} in {balances:?}"))
        };

        // Checking: 100 (pad) - 50 + 200 - 11 (bought EUR cash).
        assert_eq!(get("Assets:Bank:Checking"), "239.00");
        assert_eq!(get("Assets:Bank:Emergency"), "30.00");
        // Assets:Bank has postings AND sub-accounts: its entry is the subtree
        // total (own -30 + Checking 239 + Emergency 30).
        assert_eq!(get("Assets:Bank"), "239.00");
        // Ancestors without own postings get synthesized: USD 239 + EUR 10,
        // USD is the operating currency → direct match.
        assert_eq!(get("Assets"), "239.00");
        // USD held + USD is an operating currency → direct match.
        assert_eq!(get("Expenses:Food"), "50.00");
        assert_eq!(get("Income:Salary"), "-200.00");
        // Fallback: EUR only, not an operating currency → largest |units|.
        assert_eq!(get("Assets:Cash"), "10.00");
        assert_eq!(get("Equity:Opening-Balances"), "-100.00");
        // The reported commodity is the one the units are in.
        assert_eq!(currency("Assets:Bank:Checking"), "USD");
        assert_eq!(currency("Assets"), "USD");
        // EUR is not an operating currency, so Cash falls back to it.
        assert_eq!(currency("Assets:Cash"), "EUR");
        // Exact scale is preserved end-to-end.
        assert!(balances.iter().all(|b| b.balance.contains('.')));
    }

    #[test]
    fn lists_account_transactions() {
        let mut fs = rustledger_loader::VirtualFileSystem::new();
        fs.add_file(
            "main.beancount",
            "\
option \"operating_currency\" \"USD\"

2026-01-01 open Assets:Bank
2026-01-01 open Assets:Bank:Checking
2026-01-01 open Expenses:Food
2026-01-01 open Income:Salary

2026-01-05 * \"Employer\" \"Salary\"
  Assets:Bank:Checking            100.00 USD
  Income:Salary                  -100.00 USD

2026-01-10 * \"Groceries\"
  Assets:Bank:Checking            -20.00 USD
  Expenses:Food                    20.00 USD

2026-01-12 ! \"Transfer\"
  Assets:Bank                     -30.00 USD
  Assets:Bank:Checking             30.00 USD
",
        );
        let ledger = rustledger_loader::load_with_fs(
            Path::new("main.beancount"),
            &LoadOptions::default(),
            Box::new(fs),
        )
        .unwrap();
        assert!(ledger.errors.is_empty(), "errors: {:?}", ledger.errors);

        // Ledger-stream ids of the transactions touching an account, recomputed
        // here so the assertion does not depend on how many non-transaction
        // directives the fixture happens to contain.
        let stream_ids = |account: &str| -> Vec<u32> {
            let prefix = format!("{account}:");
            ledger
                .balance_view()
                .into_iter()
                .enumerate()
                .filter_map(|(id, directive)| {
                    let Directive::Transaction(tx) = directive else {
                        return None;
                    };
                    let touches = tx.postings.iter().any(|posting| {
                        let name = posting.value.account.as_str();
                        name == account || name.starts_with(&prefix)
                    });
                    touches.then(|| u32::try_from(id).unwrap())
                })
                .collect()
        };

        // A leaf account returns exactly the transactions touching it, newest
        // first (Salary 01-05, Groceries 01-10, Transfer 01-12 → reversed).
        let checking = account_transaction_views(&ledger, &HashMap::new(), "Assets:Bank:Checking");
        assert_eq!(checking.len(), 3);
        assert_eq!(
            checking.iter().map(|t| t.date.as_str()).collect::<Vec<_>>(),
            ["2026-01-12", "2026-01-10", "2026-01-05"]
        );
        assert_eq!(
            checking
                .iter()
                .map(|t| t.narration.as_str())
                .collect::<Vec<_>>(),
            ["Transfer", "Groceries", "Salary"]
        );

        // Every returned transaction carries all of its postings, including the
        // counter-leg that does not touch the account. Newest entry first.
        assert_eq!(
            checking[0]
                .postings
                .iter()
                .map(|p| (p.account.as_str(), p.units.as_str()))
                .collect::<Vec<_>>(),
            [("Assets:Bank", "-30.00"), ("Assets:Bank:Checking", "30.00"),]
        );
        assert_eq!(checking[0].flag, "!");
        assert_eq!(checking[0].postings[0].currency, "USD");
        assert_eq!(
            checking[1]
                .postings
                .iter()
                .map(|p| p.account.as_str())
                .collect::<Vec<_>>(),
            ["Assets:Bank:Checking", "Expenses:Food"]
        );
        assert_eq!(checking[1].flag, "*");
        assert_eq!(checking[1].postings.len(), 2);
        assert!(checking.iter().all(|t| t.postings.len() == 2));
        assert!(checking[1].payee.is_none());
        // Explicit payee survives; narration is the description.
        assert_eq!(checking[2].payee.as_deref(), Some("Employer"));
        assert_eq!(checking[2].narration, "Salary");

        // A parent account also returns transactions that only touch its
        // sub-account: the leaf's two entries have no posting on Assets:Bank.
        let bank = account_transaction_views(&ledger, &HashMap::new(), "Assets:Bank");
        assert_eq!(
            bank.iter()
                .map(|t| t.narration.as_str())
                .collect::<Vec<_>>(),
            ["Transfer", "Groceries", "Salary"]
        );
        assert!(bank[1].postings.iter().all(|p| p.account != "Assets:Bank"));

        // Ids are unique, and ascending they are the ledger stream order.
        let mut ids: Vec<u32> = checking.iter().map(|t| t.id).collect();
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "duplicate ids: {ids:?}");
        ids.sort_unstable();
        assert_eq!(ids, stream_ids("Assets:Bank:Checking"));

        // The virtual root — an empty account — is the whole ledger.
        let root = account_transaction_views(&ledger, &HashMap::new(), "");
        assert_eq!(root.len(), 3);
        assert_eq!(
            root.iter()
                .map(|t| t.narration.as_str())
                .collect::<Vec<_>>(),
            ["Transfer", "Groceries", "Salary"]
        );
        let mut root_ids: Vec<u32> = root.iter().map(|t| t.id).collect();
        root_ids.sort_unstable();
        assert_eq!(root_ids, ids);

        // An account with no postings in a transaction stream yields an empty
        // view rather than an error.
        assert!(account_transaction_views(&ledger, &HashMap::new(), "Assets:Cash").is_empty());

        // A ledger whose files are not in a git repository reports no row as
        // modified — the flag never turns git's absence into an error.
        assert!(root.iter().all(|view| !view.modified));
    }

    #[test]
    fn envelope_budget_accounts_stay_out_of_the_app() {
        let mut fs = rustledger_loader::VirtualFileSystem::new();
        fs.add_file(
            "main.beancount",
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
        let ledger = rustledger_loader::load_with_fs(
            Path::new("main.beancount"),
            &LoadOptions::default(),
            Box::new(fs),
        )
        .unwrap();
        assert!(ledger.errors.is_empty(), "errors: {:?}", ledger.errors);

        // The generated accounts are the plugin's own bookkeeping: the tree shows
        // neither them nor their money.
        let balances = account_balances(&ledger, &BTreeSet::new());
        let names: Vec<&str> = balances.iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"Assets:Bank"), "{names:?}");
        assert!(
            !names.iter().any(|name| name.contains("Envelope")),
            "{names:?}"
        );
        let get = |name: &str| {
            balances
                .iter()
                .find(|b| b.name == name)
                .map(|b| (b.balance.as_str(), b.currency.as_str()))
        };
        assert_eq!(get("Assets:Bank"), Some(("-50.00", "TRY")));
        // Equity only ever held envelope money here, and it is filtered, so no
        // Equity node is left to report.
        assert_eq!(get("Equity"), None);

        // The generated entries never reach the register; the user's own entry
        // does, and keeps its ledger-stream id (the id the editor works from).
        let root = account_transaction_views(&ledger, &HashMap::new(), "");
        let narrations: Vec<&str> = root.iter().map(|t| t.narration.as_str()).collect();
        assert_eq!(narrations, vec!["Groceries"], "{narrations:?}");
        let index = ledger
            .balance_view()
            .iter()
            .position(|directive| {
                matches!(directive, Directive::Transaction(tx) if tx.narration.as_str() == "Groceries")
            })
            .expect("the user's transaction is in the stream");
        assert_eq!(root[0].id, u32::try_from(index).unwrap());
    }
}
