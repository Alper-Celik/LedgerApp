// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `custom "envelope" …` configuration language.
//!
//! # Keys
//!
//! Every key is a `custom "envelope"` directive; the first value is the key, the
//! rest are its arguments. Directives are dated, and for every key the newest
//! directive at or before a month is the one in force for that month, which is
//! how a category's behaviour can change month by month.
//!
//! | Key | Arguments | Meaning |
//! |-----|-----------|---------|
//! | `start date` | STRING `YYYY-MM` | First budget month. **Required.** |
//! | `end date` | STRING `YYYY-MM` | Last budget month (inclusive). Default: `months ahead` past the newest directive date in the ledger. |
//! | `months ahead` | NUMBER | How many months past the newest ledger directive to budget. Default 0. |
//! | `budget account` | STRING regex | An on-budget account: assets and liabilities holding spendable money. Repeatable; at least one is required. |
//! | `income account` | STRING regex | Accepted and validated (its regex must compile) so a file shared with fava-envelope keeps working. The budget's availability comes from the on-budget accounts, per commodity, so this changes no number. Repeatable; default `^Income:`. |
//! | `fund` | ACCOUNT, AMOUNT | Assign `AMOUNT` to the category at ACCOUNT in the directive's month. **Inherited**: every later month keeps assigning it until a `fund` for the same category replaces it (`0.00` stops it). |
//! | `adjust` | ACCOUNT, AMOUNT | One-off, month-scoped signed change to the category's balance (moving money to/from `To-Be-Budgeted`). Never inherited. |
//! | `zero` | ACCOUNT | One-off: release the category subtree's whole balance (every commodity) to `To-Be-Budgeted`. |
//! | `pad` | ACCOUNT, AMOUNT | One-off: set the category subtree's balance in that commodity to `AMOUNT`, taking from or giving back to `To-Be-Budgeted`. |
//! | `pad` | ACCOUNT | Beancount's pad: resolved by the next `balance` for the same category and commodity; without one it is an error (E2003's envelope twin). |
//! | `balance` | ACCOUNT, AMOUNT | Assert the category subtree's balance in that commodity at the end of the directive's month. Emitted as a `balance` directive, so rustledger's own Late validation checks it. |
//! | `rollover` | ACCOUNT, BOOL | `TRUE` carries the category's balance into the next month (default), `FALSE` resets it to zero at the month boundary. |
//! | `rollover` | ACCOUNT, AMOUNT | Carries the balance up to `AMOUNT`, sweeping the excess to `To-Be-Budgeted`. |
//! | `map` | STRING regex, ACCOUNT | Optional: send postings matching the account pattern to ACCOUNT's category instead of their own. First match wins. |
//! | `include starting balance` | BOOL | Whether the on-budget money held before the first budget month counts as available. Default `FALSE`. |
//! | `assert` | BOOL | Emit a `balance` assertion per category and commodity at each month end. Default `TRUE`. |
//! | `strict` | BOOL | Report spending in a category that was never funded. Default `FALSE`. |
//!
//! # Rules the parser enforces
//!
//! * A key this plugin does not implement is a *warning*, not an error: the same
//!   directives are read by fava-envelope (`allocate`, `mapping`, …), so a file
//!   sharing both tools may hold keys only one of them knows. It is collected in
//!   [`Config::warnings`] and the rest of the configuration stays in force.
//! * For a key the plugin does implement, a wrong value kind, a malformed month
//!   or a missing argument is an *error*: a budget whose configuration was
//!   silently ignored is worse than one that refuses to load.
//! * ACCOUNT arguments must name a real account path string (`Expenses:Groceries`);
//!   the plugin resolves it to its envelope account, which is why typos in the
//!   expense tree are visible in a `grep`. For the category keys (`fund`,
//!   `adjust`, `zero`, `pad`, `balance`) the account must be under
//!   [`Config::expense_root`]; an account elsewhere is an error naming the key
//!   and the account.
//! * A `rollover` may not be declared on a category that has an ancestor or a
//!   descendant with one — overlapping policies would sweep the same money
//!   twice.
//! * `pad` without a following `balance` and two `pad`s before one `balance`
//!   are errors, mirroring beancount's `E2003` / `E2004`.

use std::collections::BTreeMap;

use regex_lite::Regex;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rustledger_plugin_types::{
    DirectiveData, DirectiveWrapper, MetaValueData, PluginError, PluginOptions,
};

use crate::accounts::is_under;
use crate::money::Money;
use crate::month::Month;

/// Everything the monthly model needs, parsed from the ledger.
#[derive(Clone, Debug)]
pub struct Config {
    /// First budget month.
    pub start: Month,
    /// Explicit last budget month (inclusive). `None`: derive it from the
    /// ledger's newest directive plus [`Config::months_ahead`].
    pub end: Option<Month>,
    /// Months to budget past the newest ledger directive.
    pub months_ahead: u32,
    /// On-budget account patterns.
    pub budget_accounts: Vec<String>,
    /// Income account patterns: validated for fava-envelope compatibility, and
    /// deliberately not read by the budget, whose availability is the on-budget
    /// accounts' balances per commodity.
    pub income_accounts: Vec<String>,
    /// Assignments, in directive order, each effective from its month.
    pub funds: Vec<Fund>,
    /// One-off month-scoped balance changes.
    pub adjustments: Vec<Adjustment>,
    /// One-off `zero` / `pad` / `balance` operations.
    pub targets: Vec<Target>,
    /// Rollover policies, each effective from its month.
    pub rollovers: Vec<RolloverDecl>,
    /// Category overrides for matching postings.
    pub mappings: Vec<Mapping>,
    /// Whether money held before [`Config::start`] counts as available.
    pub include_starting_balance: bool,
    /// Whether to emit month-end assertions.
    pub assert_balances: bool,
    /// Whether to report spending in unfunded categories.
    pub strict: bool,
    /// The ledger's expense root (`option "name_expenses"`, default `Expenses`).
    pub expense_root: String,
    /// The ledger's income root (`option "name_income"`, default `Income`).
    pub income_root: String,
    /// Non-fatal diagnostics: keys this plugin does not implement (the same
    /// directives are read by fava-envelope, which has its own keys) and other
    /// warnings. They do not stop the budget; the rest of the configuration
    /// stays in force and the host reports these.
    pub warnings: Vec<PluginError>,
}

/// An assignment to a category, effective from its month onward.
#[derive(Clone, Debug)]
pub struct Fund {
    /// Month the assignment starts being effective.
    pub month: Month,
    /// The category, as an expense account path.
    pub node: String,
    /// The amount assigned each month.
    pub amount: Money,
}

/// A one-off, month-scoped signed change to a category's balance.
#[derive(Clone, Debug)]
pub struct Adjustment {
    /// Month the change applies to.
    pub month: Month,
    /// The category, as an expense account path.
    pub node: String,
    /// Signed amount: positive funds the category, negative releases it.
    pub amount: Money,
}

/// A one-off reconciliation operation.
#[derive(Clone, Debug)]
pub enum Target {
    /// Release the whole subtree balance (all commodities) to `To-Be-Budgeted`.
    Zero {
        /// Month the operation applies to.
        month: Month,
        /// The category, as an expense account path.
        node: String,
    },
    /// Set the subtree balance in one commodity to `amount`.
    ///
    /// `amount` is `None` for beancount's bare `pad ACCOUNT`, which is resolved
    /// from the `balance` declared for the same category in the same month.
    Pad {
        /// Month the operation applies to.
        month: Month,
        /// The category, as an expense account path.
        node: String,
        /// The target balance, or `None` when resolved from a `balance`.
        amount: Option<Money>,
    },
    /// Assert the subtree balance in one commodity at the month's end.
    Balance {
        /// Month the assertion applies to.
        month: Month,
        /// The category, as an expense account path.
        node: String,
        /// The asserted balance.
        amount: Money,
    },
}

/// What happens to a category's balance at a month boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rollover {
    /// Carry the whole balance, positive or negative.
    Carry,
    /// Reset to zero: surplus back to `To-Be-Budgeted`, deficit taken from it.
    Reset,
    /// Carry up to the cap, sweeping the excess.
    Cap(Money),
}

/// A rollover policy, effective from its month.
#[derive(Clone, Debug)]
pub struct RolloverDecl {
    /// Month the policy starts being effective.
    pub month: Month,
    /// The category, as an expense account path.
    pub node: String,
    /// The policy.
    pub policy: Rollover,
}

/// A posting pattern redirected to a category.
#[derive(Clone, Debug)]
pub struct Mapping {
    /// Regex matched against the posting's account.
    pub pattern: String,
    /// The category, as an expense account path.
    pub node: String,
}

impl Config {
    /// Parse the ledger's `custom "envelope"` directives.
    ///
    /// `options` supplies the ledger's configured roots
    /// ([`PluginOptions::account_types`]) and is the only place the expense and
    /// income roots come from.
    ///
    /// # Errors
    ///
    /// Returns the hard errors — diagnostics that make the configuration
    /// unusable: no `start date`, no `budget account`, a malformed month, a
    /// wrong value kind or missing argument for a key the plugin implements, a
    /// category account that is not under the expense root, a pattern that does
    /// not compile, a bare `pad` with no `balance` (or two before one), or
    /// overlapping `rollover` declarations. The caller then passes the
    /// directive stream through untouched and reports these.
    ///
    /// Non-fatal diagnostics — an unknown key, i.e. a fava-envelope-only key —
    /// are returned in [`Config::warnings`] and do not fail the parse; the rest
    /// of the configuration stays in force.
    pub fn parse(
        directives: &[DirectiveWrapper],
        options: &PluginOptions,
    ) -> Result<Self, Vec<PluginError>> {
        let mut parser = Parser::new(options);
        for directive in directives {
            parser.handle(directive);
        }
        parser.finish()
    }
}

/// State accumulated while walking the `custom "envelope"` directives.
///
/// The parser never resolves a month: it records every declaration with the
/// month of its directive and leaves "newest wins over time" to the model.
struct Parser<'a> {
    /// The ledger's options, for the expense and income roots.
    options: &'a PluginOptions,
    /// Location of the directive being handled, attached to diagnostics.
    location: Option<(String, u32)>,
    start: Option<Month>,
    end: Option<Month>,
    months_ahead: u32,
    budget_accounts: Vec<String>,
    income_accounts: Vec<String>,
    funds: Vec<Fund>,
    adjustments: Vec<Adjustment>,
    targets: Vec<Target>,
    rollovers: Vec<RolloverDecl>,
    mappings: Vec<Mapping>,
    include_starting_balance: bool,
    assert_balances: bool,
    strict: bool,
    /// Hard errors: make the configuration unusable, returned from `parse`.
    errors: Vec<PluginError>,
    /// Non-fatal diagnostics: reported in [`Config::warnings`].
    warnings: Vec<PluginError>,
}

impl<'a> Parser<'a> {
    fn new(options: &'a PluginOptions) -> Self {
        Self {
            options,
            location: None,
            start: None,
            end: None,
            months_ahead: 0,
            budget_accounts: Vec::new(),
            income_accounts: Vec::new(),
            funds: Vec::new(),
            adjustments: Vec::new(),
            targets: Vec::new(),
            rollovers: Vec::new(),
            mappings: Vec::new(),
            include_starting_balance: false,
            assert_balances: true,
            strict: false,
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Handle one directive, ignoring anything that is not `custom "envelope"`.
    fn handle(&mut self, directive: &DirectiveWrapper) {
        let DirectiveData::Custom(custom) = &directive.data else {
            return;
        };
        if custom.custom_type != "envelope" {
            return;
        }
        self.location = directive.filename.clone().zip(directive.lineno);
        let Some(key) = custom.values.first().and_then(value::string) else {
            self.push_error(PluginError::error("envelope: directive is missing its key"));
            return;
        };
        let args = &custom.values[1..];
        match key {
            "start date" => {
                if let Some(month) = self.month_arg(key, args, 0) {
                    self.start = Some(month);
                }
            }
            "end date" => {
                if let Some(month) = self.month_arg(key, args, 0) {
                    self.end = Some(month);
                }
            }
            "months ahead" => {
                if let Some(number) = self.number_arg(key, args, 0) {
                    if number.is_sign_negative() || !number.fract().is_zero() {
                        self.error(
                            key,
                            &format!("expected a non-negative whole number, got {number}"),
                        );
                    } else if let Some(value) =
                        number.to_u64().and_then(|raw| u32::try_from(raw).ok())
                    {
                        self.months_ahead = value;
                    } else {
                        self.error(key, &format!("{number} months is out of range"));
                    }
                }
            }
            "budget account" => {
                if let Some(pattern) = self.pattern_arg(key, args, 0) {
                    self.budget_accounts.push(pattern);
                }
            }
            "income account" => {
                if let Some(pattern) = self.pattern_arg(key, args, 0) {
                    self.income_accounts.push(pattern);
                }
            }
            "fund" => {
                if let (Some(node), Some(amount), Some(month)) = (
                    self.expense_account_arg(key, args, 0),
                    self.amount_arg(key, args, 1),
                    self.directive_month(key, directive),
                ) {
                    self.funds.push(Fund {
                        month,
                        node,
                        amount,
                    });
                }
            }
            "adjust" => {
                if let (Some(node), Some(amount), Some(month)) = (
                    self.expense_account_arg(key, args, 0),
                    self.amount_arg(key, args, 1),
                    self.directive_month(key, directive),
                ) {
                    self.adjustments.push(Adjustment {
                        month,
                        node,
                        amount,
                    });
                }
            }
            "zero" => {
                if let (Some(node), Some(month)) = (
                    self.expense_account_arg(key, args, 0),
                    self.directive_month(key, directive),
                ) {
                    self.targets.push(Target::Zero { month, node });
                }
            }
            "pad" => {
                let Some(node) = self.expense_account_arg(key, args, 0) else {
                    return;
                };
                let amount = if args.len() >= 2 {
                    match self.amount_arg(key, args, 1) {
                        Some(amount) => Some(amount),
                        None => return,
                    }
                } else {
                    None
                };
                if let Some(month) = self.directive_month(key, directive) {
                    self.targets.push(Target::Pad {
                        month,
                        node,
                        amount,
                    });
                }
            }
            "balance" => {
                if let (Some(node), Some(amount), Some(month)) = (
                    self.expense_account_arg(key, args, 0),
                    self.amount_arg(key, args, 1),
                    self.directive_month(key, directive),
                ) {
                    self.targets.push(Target::Balance {
                        month,
                        node,
                        amount,
                    });
                }
            }
            "rollover" => {
                let Some(node) = self.account_arg(key, args, 0) else {
                    return;
                };
                let Some(second) = args.get(1) else {
                    self.error(key, "expected a BOOL or an AMOUNT as argument 2");
                    return;
                };
                let policy = if let Some(flag) = value::boolean(second) {
                    if flag {
                        Rollover::Carry
                    } else {
                        Rollover::Reset
                    }
                } else if let Some(amount) = value::amount(second) {
                    Rollover::Cap(amount)
                } else {
                    self.error(key, "expected a BOOL or an AMOUNT as argument 2");
                    return;
                };
                if let Some(month) = self.directive_month(key, directive) {
                    self.rollovers.push(RolloverDecl {
                        month,
                        node,
                        policy,
                    });
                }
            }
            "map" => {
                if let (Some(pattern), Some(node)) = (
                    self.pattern_arg(key, args, 0),
                    self.account_arg(key, args, 1),
                ) {
                    self.mappings.push(Mapping { pattern, node });
                }
            }
            "include starting balance" => {
                if let Some(flag) = self.bool_arg(key, args, 0) {
                    self.include_starting_balance = flag;
                }
            }
            "assert" => {
                if let Some(flag) = self.bool_arg(key, args, 0) {
                    self.assert_balances = flag;
                }
            }
            "strict" => {
                if let Some(flag) = self.bool_arg(key, args, 0) {
                    self.strict = flag;
                }
            }
            other => self.warning(format!(
                "envelope: unknown key {other:?}; this plugin does not implement it"
            )),
        }
    }

    /// Check the required keys and the cross-declaration rules, then build.
    fn finish(mut self) -> Result<Config, Vec<PluginError>> {
        self.location = None;
        if self.start.is_none() {
            self.error("start date", "missing; a budget needs a first month");
        }
        if self.budget_accounts.is_empty() {
            self.error(
                "budget account",
                "at least one on-budget account is required",
            );
        }
        self.check_pads();
        self.check_rollovers();
        if !self.errors.is_empty() {
            return Err(self.errors);
        }
        let mut config = Config {
            start: self.start.expect("a start date was checked above"),
            end: self.end,
            months_ahead: self.months_ahead,
            budget_accounts: self.budget_accounts,
            income_accounts: self.income_accounts,
            funds: self.funds,
            adjustments: self.adjustments,
            targets: self.targets,
            rollovers: self.rollovers,
            mappings: self.mappings,
            include_starting_balance: self.include_starting_balance,
            assert_balances: self.assert_balances,
            strict: self.strict,
            expense_root: self.options.account_types.expenses.clone(),
            income_root: self.options.account_types.income.clone(),
            warnings: self.warnings,
        };
        if config.income_accounts.is_empty() {
            // No `income account` declared: default to the ledger's configured
            // income root, which is `Income` unless the ledger renames it, so a
            // renamed income tree still classifies its income.
            let default_income = format!("^{}:", config.income_root);
            config.income_accounts.push(default_income);
        }
        Ok(config)
    }

    /// A bare `pad` is resolved by a following `balance` for the same month and
    /// category: without one it is an error, and two before one `balance` are
    /// an error too (beancount's `E2003` / `E2004`).
    fn check_pads(&mut self) {
        let mut pending: BTreeMap<(Month, String), u32> = BTreeMap::new();
        let mut problems: Vec<String> = Vec::new();
        for target in &self.targets {
            match target {
                Target::Pad {
                    month,
                    node,
                    amount: None,
                } => {
                    *pending.entry((*month, node.clone())).or_insert(0) += 1;
                }
                Target::Balance { month, node, .. } => {
                    if let Some(count) = pending.remove(&(*month, node.clone())) {
                        if count >= 2 {
                            problems.push(format!(
                                "two pad declarations for {node} in {} before one balance",
                                month.label()
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
        for ((month, node), _) in &pending {
            problems.push(format!("no balance for {node} in {}", month.label()));
        }
        for problem in problems {
            self.error("pad", &problem);
        }
    }

    /// Rollover policies may not overlap: an account and any of its ancestors or
    /// descendants sweep the same money twice. Two declarations on the *same*
    /// category are a month-by-month policy change, not an overlap.
    fn check_rollovers(&mut self) {
        let mut problems: Vec<String> = Vec::new();
        for i in 0..self.rollovers.len() {
            for j in (i + 1)..self.rollovers.len() {
                let left = &self.rollovers[i];
                let right = &self.rollovers[j];
                if left.node == right.node {
                    continue;
                }
                if is_under(&left.node, &right.node) || is_under(&right.node, &left.node) {
                    problems.push(format!(
                        "{} and {} overlap; only one of an account and its descendants may roll over",
                        left.node, right.node
                    ));
                }
            }
        }
        for problem in problems {
            self.error("rollover", &problem);
        }
    }

    /// Record a hard error, attaching the current directive's location.
    fn push_error(&mut self, diagnostic: PluginError) {
        let diagnostic = match &self.location {
            Some((file, line)) => diagnostic.at(file.clone(), *line),
            None => diagnostic,
        };
        self.errors.push(diagnostic);
    }

    /// Record a non-fatal warning, attaching the current directive's location.
    fn push_warning(&mut self, diagnostic: PluginError) {
        let diagnostic = match &self.location {
            Some((file, line)) => diagnostic.at(file.clone(), *line),
            None => diagnostic,
        };
        self.warnings.push(diagnostic);
    }

    fn error(&mut self, key: &str, problem: &str) {
        self.push_error(PluginError::error(format!("envelope: {key}: {problem}")));
    }

    fn warning(&mut self, message: String) {
        self.push_warning(PluginError::warning(message));
    }

    fn string_arg(&mut self, key: &str, args: &[MetaValueData], index: usize) -> Option<String> {
        match args.get(index).and_then(value::string) {
            Some(text) => Some(text.to_string()),
            None => {
                self.error(key, &format!("expected a STRING as argument {}", index + 1));
                None
            }
        }
    }

    fn month_arg(&mut self, key: &str, args: &[MetaValueData], index: usize) -> Option<Month> {
        let text = self.string_arg(key, args, index)?;
        match Month::parse(&text) {
            Some(month) => Some(month),
            None => {
                self.error(key, &format!("invalid month {text:?}, expected YYYY-MM"));
                None
            }
        }
    }

    fn pattern_arg(&mut self, key: &str, args: &[MetaValueData], index: usize) -> Option<String> {
        let pattern = self.string_arg(key, args, index)?;
        if let Err(error) = Regex::new(&pattern) {
            self.error(key, &format!("invalid pattern {pattern:?}: {error}"));
            return None;
        }
        Some(pattern)
    }

    fn account_arg(&mut self, key: &str, args: &[MetaValueData], index: usize) -> Option<String> {
        match args.get(index).and_then(value::account) {
            Some(account) => Some(account.to_string()),
            None => {
                self.error(
                    key,
                    &format!("expected an ACCOUNT as argument {}", index + 1),
                );
                None
            }
        }
    }

    fn amount_arg(&mut self, key: &str, args: &[MetaValueData], index: usize) -> Option<Money> {
        match args.get(index).and_then(value::amount) {
            Some(amount) => Some(amount),
            None => {
                self.error(
                    key,
                    &format!("expected an AMOUNT as argument {}", index + 1),
                );
                None
            }
        }
    }

    /// An ACCOUNT argument that must belong to the expense tree.
    ///
    /// The plugin mirrors expense accounts into its envelope, so a category
    /// argument outside the ledger's expense root can never be resolved; catching
    /// it here makes the typo visible instead of silently budgeting nothing.
    fn expense_account_arg(
        &mut self,
        key: &str,
        args: &[MetaValueData],
        index: usize,
    ) -> Option<String> {
        let root = self.options.account_types.expenses.clone();
        let account = self.account_arg(key, args, index)?;
        if !is_under(&account, &root) {
            self.error(
                key,
                &format!("{account:?} is not under the expense root {root:?}"),
            );
            return None;
        }
        Some(account)
    }

    fn number_arg(&mut self, key: &str, args: &[MetaValueData], index: usize) -> Option<Decimal> {
        match args.get(index).and_then(value::number) {
            Some(number) => Some(number),
            None => {
                self.error(key, &format!("expected a NUMBER as argument {}", index + 1));
                None
            }
        }
    }

    fn bool_arg(&mut self, key: &str, args: &[MetaValueData], index: usize) -> Option<bool> {
        match args.get(index).and_then(value::boolean) {
            Some(flag) => Some(flag),
            None => {
                self.error(key, &format!("expected a BOOL as argument {}", index + 1));
                None
            }
        }
    }

    fn directive_month(&mut self, key: &str, directive: &DirectiveWrapper) -> Option<Month> {
        match directive.date.get(..7).and_then(Month::parse) {
            Some(month) => Some(month),
            None => {
                self.error(
                    key,
                    &format!(
                        "invalid directive date {:?}, expected YYYY-MM-DD",
                        directive.date
                    ),
                );
                None
            }
        }
    }
}

/// Value-kind accessors, so argument checking reads the same everywhere.
///
/// `None` means the directive wrote the wrong kind of value, which the parser
/// reports as an error naming the key and the expected kind.
pub mod value {
    use rust_decimal::Decimal;
    use rustledger_plugin_types::MetaValueData;

    use crate::money::Money;

    /// The AMOUNT carried by `value`.
    #[must_use]
    pub fn amount(value: &MetaValueData) -> Option<Money> {
        match value {
            MetaValueData::Amount(amount) => Money::from_amount_data(amount),
            _ => None,
        }
    }

    /// The NUMBER carried by `value`.
    #[must_use]
    pub fn number(value: &MetaValueData) -> Option<Decimal> {
        match value {
            MetaValueData::Number(number) => number.trim().parse().ok(),
            _ => None,
        }
    }

    /// The STRING carried by `value`.
    #[must_use]
    pub fn string(value: &MetaValueData) -> Option<&str> {
        match value {
            MetaValueData::String(text) => Some(text),
            _ => None,
        }
    }

    /// The ACCOUNT carried by `value`.
    #[must_use]
    pub fn account(value: &MetaValueData) -> Option<&str> {
        match value {
            MetaValueData::Account(account) => Some(account),
            _ => None,
        }
    }

    /// The BOOL carried by `value`.
    #[must_use]
    pub fn boolean(value: &MetaValueData) -> Option<bool> {
        match value {
            MetaValueData::Bool(flag) => Some(*flag),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;
    use rustledger_plugin_types::{
        AmountData, CustomData, DirectiveData, DirectiveWrapper, MetaValueData,
        PluginErrorSeverity, PluginOptions,
    };

    use super::{Config, Money, Month, Rollover, Target};

    fn directive(date: &str, custom_type: &str, values: Vec<MetaValueData>) -> DirectiveWrapper {
        DirectiveWrapper {
            directive_type: "custom".to_string(),
            date: date.to_string(),
            filename: None,
            lineno: None,
            data: DirectiveData::Custom(CustomData {
                custom_type: custom_type.to_string(),
                values,
                metadata: Vec::new(),
            }),
        }
    }

    fn envelope(date: &str, values: Vec<MetaValueData>) -> DirectiveWrapper {
        directive(date, "envelope", values)
    }

    fn text(value: &str) -> MetaValueData {
        MetaValueData::String(value.to_string())
    }

    fn account(value: &str) -> MetaValueData {
        MetaValueData::Account(value.to_string())
    }

    fn amount(number: &str, currency: &str) -> MetaValueData {
        MetaValueData::Amount(AmountData {
            number: number.to_string(),
            currency: currency.to_string(),
        })
    }

    fn number(value: &str) -> MetaValueData {
        MetaValueData::Number(value.to_string())
    }

    fn flag(value: bool) -> MetaValueData {
        MetaValueData::Bool(value)
    }

    /// The minimum a usable config needs: a start month and an on-budget account.
    fn required() -> Vec<DirectiveWrapper> {
        vec![
            envelope("2026-10-01", vec![text("start date"), text("2026-10")]),
            envelope(
                "2026-10-01",
                vec![text("budget account"), text("^Assets:Cash$")],
            ),
        ]
    }

    fn parse(
        directives: &[DirectiveWrapper],
    ) -> Result<Config, Vec<rustledger_plugin_types::PluginError>> {
        Config::parse(directives, &PluginOptions::default())
    }

    #[test]
    fn parses_a_full_config() {
        let directives = vec![
            envelope("2026-10-01", vec![text("start date"), text("2026-10")]),
            envelope("2026-10-01", vec![text("end date"), text("2027-03")]),
            envelope("2026-10-01", vec![text("months ahead"), number("2")]),
            envelope(
                "2026-10-01",
                vec![text("budget account"), text("^Assets:Cash$")],
            ),
            envelope("2026-10-01", vec![text("income account"), text("^Income:")]),
            envelope(
                "2026-10-01",
                vec![text("include starting balance"), flag(true)],
            ),
            envelope("2026-10-01", vec![text("assert"), flag(false)]),
            envelope("2026-10-01", vec![text("strict"), flag(true)]),
            envelope(
                "2026-10-01",
                vec![
                    text("fund"),
                    account("Expenses:Groceries"),
                    amount("4000.00", "TRY"),
                ],
            ),
            envelope(
                "2026-10-05",
                vec![
                    text("adjust"),
                    account("Expenses:Groceries"),
                    amount("-100.00", "TRY"),
                ],
            ),
            envelope("2026-10-06", vec![text("zero"), account("Expenses:Fun")]),
            envelope("2026-10-07", vec![text("pad"), account("Expenses:Fun")]),
            envelope(
                "2026-10-08",
                vec![
                    text("balance"),
                    account("Expenses:Fun"),
                    amount("500.00", "TRY"),
                ],
            ),
            envelope(
                "2026-10-09",
                vec![text("rollover"), account("Expenses:Groceries"), flag(true)],
            ),
            envelope(
                "2026-10-09",
                vec![
                    text("map"),
                    text("^Expenses:Dining"),
                    account("Expenses:Food"),
                ],
            ),
        ];
        let config = parse(&directives).expect("valid config");
        assert_eq!(config.start, Month::new(2026, 10));
        assert_eq!(config.end, Some(Month::new(2027, 3)));
        assert_eq!(config.months_ahead, 2);
        assert_eq!(config.budget_accounts, vec!["^Assets:Cash$".to_string()]);
        assert_eq!(config.income_accounts, vec!["^Income:".to_string()]);
        assert!(config.include_starting_balance);
        assert!(!config.assert_balances);
        assert!(config.strict);
        assert_eq!(config.expense_root, "Expenses");
        assert_eq!(config.income_root, "Income");
        assert_eq!(config.funds.len(), 1);
        assert_eq!(config.funds[0].month, Month::new(2026, 10));
        assert_eq!(config.funds[0].node, "Expenses:Groceries");
        assert_eq!(
            config.funds[0].amount,
            Money::new(Decimal::new(400000, 2), "TRY")
        );
        assert_eq!(config.adjustments.len(), 1);
        assert_eq!(
            config.adjustments[0].amount,
            Money::new(Decimal::new(-10000, 2), "TRY")
        );
        assert_eq!(config.targets.len(), 3);
        assert!(matches!(
            config.targets[0],
            Target::Zero { ref node, .. } if node == "Expenses:Fun"
        ));
        assert!(matches!(
            config.targets[1],
            Target::Pad {
                amount: None,
                ref node,
                ..
            } if node == "Expenses:Fun"
        ));
        assert!(matches!(
            config.targets[2],
            Target::Balance { ref node, .. } if node == "Expenses:Fun"
        ));
        assert_eq!(config.rollovers.len(), 1);
        assert_eq!(config.rollovers[0].policy, Rollover::Carry);
        assert_eq!(config.mappings.len(), 1);
        assert_eq!(config.mappings[0].pattern, "^Expenses:Dining");
        assert_eq!(config.mappings[0].node, "Expenses:Food");
        assert!(config.warnings.is_empty());
    }

    #[test]
    fn fava_only_key_is_a_warning_not_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![
                text("fund"),
                account("Expenses:Groceries"),
                amount("300.00", "TRY"),
            ],
        ));
        directives.push(envelope(
            "2026-10-02",
            vec![text("allocate"), text("Expenses:Food")],
        ));
        directives.push(envelope(
            "2026-10-03",
            vec![text("rollover"), account("Expenses:Fun"), flag(true)],
        ));
        let config = parse(&directives).expect("an unknown key does not fail the parse");
        assert_eq!(config.warnings.len(), 1);
        assert_eq!(config.warnings[0].severity, PluginErrorSeverity::Warning);
        assert!(config.warnings[0].message.contains("allocate"));
        // The rest of the configuration is intact, before and after the warning.
        assert_eq!(config.start, Month::new(2026, 10));
        assert_eq!(config.budget_accounts, vec!["^Assets:Cash$".to_string()]);
        assert_eq!(config.funds.len(), 1);
        assert_eq!(config.funds[0].node, "Expenses:Groceries");
        assert_eq!(config.rollovers.len(), 1);
        assert_eq!(config.rollovers[0].policy, Rollover::Carry);
    }

    #[test]
    fn non_envelope_custom_directives_are_ignored() {
        let mut directives = required();
        directives.push(directive(
            "2026-10-01",
            "fava",
            vec![text("allocate"), text("Expenses:Food")],
        ));
        directives.push(directive("2026-10-01", "budget", vec![text("whatever")]));
        let config = parse(&directives).expect("valid config");
        assert!(config.funds.is_empty());
        assert!(config.rollovers.is_empty());
        assert!(config.warnings.is_empty());
    }

    #[test]
    fn category_account_outside_the_expense_root_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![
                text("fund"),
                account("Liabilities:CreditCard"),
                amount("100.00", "TRY"),
            ],
        ));
        let diagnostics = parse(&directives).expect_err("only expense accounts are categories");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("fund")
                && diagnostic.message.contains("Liabilities:CreditCard")
                && diagnostic.message.contains("Expenses")
        }));
    }

    #[test]
    fn wrong_value_kind_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("fund"), account("Expenses:Food"), text("4000")],
        ));
        let diagnostics = parse(&directives).expect_err("a wrong argument kind errors");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("fund")
                && diagnostic.message.contains("AMOUNT")
        }));
    }

    #[test]
    fn missing_start_date_is_an_error() {
        let directives = vec![envelope(
            "2026-10-01",
            vec![text("budget account"), text("^Assets:Cash$")],
        )];
        let diagnostics = parse(&directives).expect_err("start date is required");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("start date")
        }));
    }

    #[test]
    fn missing_budget_account_is_an_error() {
        let directives = vec![envelope(
            "2026-10-01",
            vec![text("start date"), text("2026-10")],
        )];
        let diagnostics = parse(&directives).expect_err("a budget account is required");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("budget account")
        }));
    }

    #[test]
    fn malformed_month_is_an_error() {
        let mut directives = required();
        directives[0] = envelope("2026-10-01", vec![text("start date"), text("2026-13")]);
        let diagnostics = parse(&directives).expect_err("2026-13 is not a month");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("start date")
                && diagnostic.message.contains("2026-13")
        }));
    }

    #[test]
    fn invalid_pattern_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("budget account"), text("^Assets(:[")],
        ));
        let diagnostics = parse(&directives).expect_err("the pattern does not compile");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("budget account")
                && diagnostic.message.contains("pattern")
        }));
    }

    #[test]
    fn bare_pad_without_a_balance_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("pad"), account("Expenses:Fun")],
        ));
        let diagnostics = parse(&directives).expect_err("a bare pad needs a balance");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("pad")
                && diagnostic.message.contains("Expenses:Fun")
        }));
    }

    #[test]
    fn two_bare_pads_before_one_balance_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("pad"), account("Expenses:Fun")],
        ));
        directives.push(envelope(
            "2026-10-02",
            vec![text("pad"), account("Expenses:Fun")],
        ));
        directives.push(envelope(
            "2026-10-03",
            vec![
                text("balance"),
                account("Expenses:Fun"),
                amount("500.00", "TRY"),
            ],
        ));
        let diagnostics = parse(&directives).expect_err("two pads before one balance");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("pad")
                && diagnostic.message.contains("before one balance")
        }));
    }

    #[test]
    fn literal_pad_is_a_target_with_its_amount() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![
                text("pad"),
                account("Expenses:Fun"),
                amount("250.00", "TRY"),
            ],
        ));
        let config = parse(&directives).expect("valid config");
        assert_eq!(config.targets.len(), 1);
        let Target::Pad { amount, .. } = &config.targets[0] else {
            panic!("expected a pad target, got {:?}", config.targets[0]);
        };
        assert_eq!(
            amount.as_ref(),
            Some(&Money::new(Decimal::new(25000, 2), "TRY"))
        );
    }

    #[test]
    fn overlapping_rollovers_are_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("rollover"), account("Expenses:Food"), flag(true)],
        ));
        directives.push(envelope(
            "2026-11-01",
            vec![
                text("rollover"),
                account("Expenses:Food:Groceries"),
                flag(true),
            ],
        ));
        let diagnostics = parse(&directives).expect_err("nested rollovers overlap");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("rollover")
                && diagnostic.message.contains("Expenses:Food:Groceries")
        }));
    }

    #[test]
    fn rollover_redeclared_on_the_same_category_is_allowed() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("rollover"), account("Expenses:Food"), flag(true)],
        ));
        directives.push(envelope(
            "2026-11-01",
            vec![text("rollover"), account("Expenses:Food"), flag(false)],
        ));
        let config = parse(&directives).expect("a policy change is not an overlap");
        assert_eq!(config.rollovers.len(), 2);
        assert_eq!(config.rollovers[0].policy, Rollover::Carry);
        assert_eq!(config.rollovers[1].policy, Rollover::Reset);
    }

    #[test]
    fn rollover_with_an_amount_caps() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![
                text("rollover"),
                account("Expenses:Food"),
                amount("500.00", "TRY"),
            ],
        ));
        let config = parse(&directives).expect("valid config");
        assert_eq!(config.rollovers.len(), 1);
        assert_eq!(
            config.rollovers[0].policy,
            Rollover::Cap(Money::new(Decimal::new(50000, 2), "TRY"))
        );
        assert!(config.assert_balances, "assert defaults to TRUE");
        assert_eq!(
            config.income_accounts,
            vec!["^Income:".to_string()],
            "income account defaults to ^Income:"
        );
    }

    #[test]
    fn directive_without_a_key_is_an_error() {
        let mut directives = required();
        directives.push(envelope("2026-10-01", Vec::new()));
        let diagnostics = parse(&directives).expect_err("a keyless directive cannot be classified");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("missing its key")
        }));
    }

    #[test]
    fn a_non_string_key_is_treated_as_missing() {
        // The key itself must come from a STRING value: `envelope 2026` has none.
        let mut directives = required();
        directives.push(envelope("2026-10-01", vec![number("2026")]));
        let diagnostics = parse(&directives).expect_err("a non-string key is not a key");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("missing its key")
        }));
    }

    #[test]
    fn months_ahead_negative_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("months ahead"), number("-1")],
        ));
        let diagnostics = parse(&directives).expect_err("months ahead cannot be negative");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("months ahead")
                && diagnostic.message.contains("non-negative whole number")
        }));
    }

    #[test]
    fn months_ahead_fractional_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("months ahead"), number("1.5")],
        ));
        let diagnostics = parse(&directives).expect_err("months ahead cannot be fractional");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("months ahead")
                && diagnostic.message.contains("non-negative whole number")
        }));
    }

    #[test]
    fn months_ahead_out_of_range_is_an_error() {
        let mut directives = required();
        // Non-negative and whole, so it passes the sign/fraction check; it fits
        // in u64 but not in u32, so only the range check rejects it.
        directives.push(envelope(
            "2026-10-01",
            vec![text("months ahead"), number("4294967296")],
        ));
        let diagnostics = parse(&directives).expect_err("a month count past u32 cannot be stored");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("months ahead")
                && diagnostic.message.contains("out of range")
        }));
    }

    #[test]
    fn months_ahead_wrong_kind_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("months ahead"), text("2")],
        ));
        let diagnostics = parse(&directives).expect_err("months ahead takes a NUMBER");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("months ahead")
                && diagnostic
                    .message
                    .contains("expected a NUMBER as argument 1")
        }));
    }

    #[test]
    fn end_date_wrong_kind_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("end date"), number("2027")],
        ));
        let diagnostics = parse(&directives).expect_err("end date takes a STRING month");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("end date")
                && diagnostic
                    .message
                    .contains("expected a STRING as argument 1")
        }));
    }

    #[test]
    fn start_date_wrong_kind_is_an_error() {
        let mut directives = required();
        directives[0] = envelope("2026-10-01", vec![text("start date"), number("2026")]);
        let diagnostics = parse(&directives).expect_err("start date takes a STRING month");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("start date")
                && diagnostic
                    .message
                    .contains("expected a STRING as argument 1")
        }));
    }

    #[test]
    fn budget_account_wrong_kind_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("budget account"), flag(true)],
        ));
        let diagnostics = parse(&directives).expect_err("a pattern must be a STRING");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("budget account")
                && diagnostic
                    .message
                    .contains("expected a STRING as argument 1")
        }));
    }

    #[test]
    fn income_account_wrong_kind_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("income account"), number("1")],
        ));
        let diagnostics = parse(&directives).expect_err("a pattern must be a STRING");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("income account")
                && diagnostic
                    .message
                    .contains("expected a STRING as argument 1")
        }));
    }

    #[test]
    fn adjust_wrong_amount_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("adjust"), account("Expenses:Food"), text("100")],
        ));
        let diagnostics = parse(&directives).expect_err("adjust takes an AMOUNT");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("adjust")
                && diagnostic
                    .message
                    .contains("expected an AMOUNT as argument 2")
        }));
    }

    #[test]
    fn zero_wrong_account_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("zero"), text("Expenses:Fun")],
        ));
        let diagnostics = parse(&directives).expect_err("zero takes an ACCOUNT");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("zero")
                && diagnostic
                    .message
                    .contains("expected an ACCOUNT as argument 1")
        }));
    }

    #[test]
    fn balance_wrong_amount_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("balance"), account("Expenses:Fun"), text("500")],
        ));
        let diagnostics = parse(&directives).expect_err("balance takes an AMOUNT");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("balance")
                && diagnostic
                    .message
                    .contains("expected an AMOUNT as argument 2")
        }));
    }

    #[test]
    fn balance_without_a_pad_is_allowed() {
        // `check_pads` only tracks categories with a pending bare pad; a balance
        // for a category that never declared one is not an error.
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![
                text("balance"),
                account("Expenses:Fun"),
                amount("500.00", "TRY"),
            ],
        ));
        let config = parse(&directives).expect("a balance without a pad is allowed");
        assert_eq!(config.targets.len(), 1);
        assert!(matches!(
            config.targets[0],
            Target::Balance { ref node, .. } if node == "Expenses:Fun"
        ));
    }

    #[test]
    fn pad_wrong_account_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("pad"), text("Expenses:Fun")],
        ));
        let diagnostics = parse(&directives).expect_err("pad takes an ACCOUNT");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("pad")
                && diagnostic
                    .message
                    .contains("expected an ACCOUNT as argument 1")
        }));
    }

    #[test]
    fn pad_wrong_amount_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("pad"), account("Expenses:Fun"), text("250")],
        ));
        let diagnostics = parse(&directives).expect_err("a second pad argument is an AMOUNT");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("pad")
                && diagnostic
                    .message
                    .contains("expected an AMOUNT as argument 2")
        }));
    }

    #[test]
    fn rollover_wrong_account_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("rollover"), text("Expenses:Food"), flag(true)],
        ));
        let diagnostics = parse(&directives).expect_err("rollover takes an ACCOUNT");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("rollover")
                && diagnostic
                    .message
                    .contains("expected an ACCOUNT as argument 1")
        }));
    }

    #[test]
    fn rollover_missing_second_argument_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("rollover"), account("Expenses:Food")],
        ));
        let diagnostics = parse(&directives).expect_err("rollover takes a policy");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("rollover")
                && diagnostic
                    .message
                    .contains("expected a BOOL or an AMOUNT as argument 2")
        }));
    }

    #[test]
    fn rollover_wrong_second_argument_kind_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("rollover"), account("Expenses:Food"), text("carry")],
        ));
        let diagnostics = parse(&directives).expect_err("rollover's policy is a BOOL or an AMOUNT");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("rollover")
                && diagnostic
                    .message
                    .contains("expected a BOOL or an AMOUNT as argument 2")
        }));
    }

    #[test]
    fn map_wrong_account_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("map"), text("^Expenses:Dining"), text("Expenses:Food")],
        ));
        let diagnostics = parse(&directives).expect_err("map's target is an ACCOUNT");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("map")
                && diagnostic
                    .message
                    .contains("expected an ACCOUNT as argument 2")
        }));
    }

    #[test]
    fn map_wrong_pattern_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("map"), number("1"), account("Expenses:Food")],
        ));
        let diagnostics = parse(&directives).expect_err("map's pattern is a STRING");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("map")
                && diagnostic
                    .message
                    .contains("expected a STRING as argument 1")
        }));
    }

    #[test]
    fn include_starting_balance_wrong_kind_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("include starting balance"), text("yes")],
        ));
        let diagnostics = parse(&directives).expect_err("the flag must be a BOOL");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("include starting balance")
                && diagnostic.message.contains("expected a BOOL as argument 1")
        }));
    }

    #[test]
    fn assert_wrong_kind_is_an_error() {
        let mut directives = required();
        directives.push(envelope("2026-10-01", vec![text("assert"), number("1")]));
        let diagnostics = parse(&directives).expect_err("assert takes a BOOL");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("assert")
                && diagnostic.message.contains("expected a BOOL as argument 1")
        }));
    }

    #[test]
    fn strict_wrong_kind_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-10-01",
            vec![text("strict"), amount("1.00", "TRY")],
        ));
        let diagnostics = parse(&directives).expect_err("strict takes a BOOL");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("strict")
                && diagnostic.message.contains("expected a BOOL as argument 1")
        }));
    }

    #[test]
    fn invalid_directive_month_on_a_pad_is_an_error() {
        let mut directives = required();
        directives.push(envelope(
            "2026-13-01",
            vec![
                text("pad"),
                account("Expenses:Fun"),
                amount("250.00", "TRY"),
            ],
        ));
        let diagnostics = parse(&directives).expect_err("2026-13 is not a month");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("pad")
                && diagnostic.message.contains("invalid directive date")
                && diagnostic.message.contains("2026-13-01")
        }));
    }

    #[test]
    fn malformed_directive_date_on_a_rollover_is_an_error() {
        let mut directives = required();
        // Too short for `get(..7)`, so the date never reaches `Month::parse`.
        directives.push(envelope(
            "2026",
            vec![text("rollover"), account("Expenses:Food"), flag(true)],
        ));
        let diagnostics = parse(&directives).expect_err("2026 is not a YYYY-MM-DD date");
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == PluginErrorSeverity::Error
                && diagnostic.message.contains("rollover")
                && diagnostic.message.contains("invalid directive date")
        }));
    }

    #[test]
    fn an_error_carries_the_directive_location() {
        let mut directives = required();
        let mut located = envelope("2026-10-01", vec![text("strict"), text("not a bool")]);
        located.filename = Some("budget.bean".to_string());
        located.lineno = Some(42);
        directives.push(located);
        let diagnostics = parse(&directives).expect_err("strict must be a BOOL");
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.message.contains("strict"))
            .expect("the strict diagnostic is reported");
        assert_eq!(diagnostic.source_file.as_deref(), Some("budget.bean"));
        assert_eq!(diagnostic.line_number, Some(42));
    }

    #[test]
    fn a_warning_carries_the_directive_location() {
        let mut directives = required();
        let mut located = envelope("2026-10-01", vec![text("allocate"), text("Expenses:Food")]);
        located.filename = Some("budget.bean".to_string());
        located.lineno = Some(7);
        directives.push(located);
        let config = parse(&directives).expect("an unknown key is only a warning");
        assert_eq!(config.warnings.len(), 1);
        assert_eq!(config.warnings[0].severity, PluginErrorSeverity::Warning);
        assert_eq!(
            config.warnings[0].source_file.as_deref(),
            Some("budget.bean")
        );
        assert_eq!(config.warnings[0].line_number, Some(7));
    }
}
