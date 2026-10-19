// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Account naming: the `Equity:Envelope:` namespace and the expense mirror.
//!
//! Every account the plugin generates lives under one prefix, so the whole
//! budget is one subtree sum away, no other account in the ledger is touched,
//! and consumers that do not care about envelopes can ignore one string.

/// Root of every generated account.
pub const NAMESPACE: &str = "Equity:Envelope";

/// The pool of money not yet assigned to a category, per commodity.
pub const TO_BE_BUDGETED: &str = "Equity:Envelope:To-Be-Budgeted";

/// The contra account of the ledger's on-budget money.
///
/// Its per-commodity balance is the negative of the sum of all on-budget
/// account balances, which is what lets [`TO_BE_BUDGETED`] hold the budget's
/// real `to be budgeted` figure while every generated transaction balances
/// inside the namespace:
///
/// `Σ Equity:Envelope:* = 0` and `To-Be-Budgeted = on-budget money − Σ categories`.
pub const MONEY: &str = "Equity:Envelope:Money";

/// Category a posting on the bare expense root lands in.
pub const UNCATEGORIZED: &str = "Equity:Envelope:Uncategorized";

/// The bridge between an account's actual holdings and the currency its
/// envelope is valued in. It holds the marked-to-market holdings on one side
/// and the target-currency value on the other, so the namespace still sums to
/// zero per commodity.
pub const VALUATION: &str = "Equity:Envelope:Valuation";

/// Whether `account` is one of the plugin's generated accounts.
#[must_use]
pub fn is_generated(account: &str) -> bool {
    is_under(account, NAMESPACE)
}

/// Whether `account` is `node` itself or a descendant of it.
#[must_use]
pub fn is_under(account: &str, node: &str) -> bool {
    account == node
        || (account.len() > node.len()
            && account.starts_with(node)
            && account.as_bytes()[node.len()] == b':')
}

/// Whether `account` is a category: the expense root, or under it.
#[must_use]
pub fn is_category(account: &str, expense_root: &str) -> bool {
    is_under(account, expense_root)
}

/// The category account for an expense posting account.
///
/// `Expenses:Groceries:Bread` with root `Expenses` becomes
/// `Equity:Envelope:Groceries:Bread`; a posting on the bare root becomes
/// [`UNCATEGORIZED`]. Returns `None` for accounts outside the expense tree.
#[must_use]
pub fn category_account(expense_account: &str, expense_root: &str) -> Option<String> {
    if !is_category(expense_account, expense_root) {
        return None;
    }
    let tail = expense_account
        .strip_prefix(expense_root)?
        .trim_start_matches(':');
    if tail.is_empty() {
        Some(UNCATEGORIZED.to_string())
    } else {
        Some(format!("{NAMESPACE}:{tail}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expense_accounts_mirror_into_the_namespace() {
        assert_eq!(
            category_account("Expenses:Groceries:Bread", "Expenses").as_deref(),
            Some("Equity:Envelope:Groceries:Bread")
        );
        // The bare root has no component to mirror.
        assert_eq!(
            category_account("Expenses", "Expenses").as_deref(),
            Some(UNCATEGORIZED)
        );
        // Only the expense tree is a category.
        assert_eq!(category_account("Assets:Bank", "Expenses"), None);
        assert_eq!(category_account("ExpensesOther:Thing", "Expenses"), None);
    }

    #[test]
    fn namespace_membership_respects_component_boundaries() {
        assert!(is_under("Equity:Envelope:Food", "Equity:Envelope:Food"));
        assert!(is_under(
            "Equity:Envelope:Food:Bread",
            "Equity:Envelope:Food"
        ));
        // A longer account that merely starts with the same characters is not a
        // descendant — this is the whole reason the separator is checked.
        assert!(!is_under("Equity:Envelope:Foodie", "Equity:Envelope:Food"));
        assert!(!is_under(
            "Equity:Envelope",
            "Equity:Envelope:To-Be-Budgeted"
        ));

        assert!(is_generated("Equity:Envelope:Uncategorized"));
        assert!(!is_generated("Equity:Opening-Balances"));
        assert!(is_category("Expenses", "Expenses"));
    }
}
