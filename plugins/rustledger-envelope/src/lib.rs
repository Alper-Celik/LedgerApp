// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Envelope budgeting for rustledger, as a WASM directive plugin.
//!
//! The plugin mirrors expense accounts 1:1 into an `Equity:Envelope:` namespace
//! and generates the monthly funding, activity, reconciliation and rollover
//! entries that make a ledger behave like an envelope budget. It is declared in
//! the journal with the `plugin` directive:
//!
//! ```beancount
//! plugin "rustledger_envelope.wasm"
//!
//! 2026-10-01 custom "envelope" "start date" "2026-10"
//! 2026-10-01 custom "envelope" "budget account" "Assets:Cash"
//! 2026-10-01 custom "envelope" "fund" Expenses:Groceries 4000.00 TRY
//! ```
//!
//! # Where it runs
//!
//! rustledger invokes WASM plugins in the post-booking pass, so the plugin sees
//! booked, cost-filled postings: an expense posting's value is its weight as
//! written (`units × cost`, else `units × price`, else `units`), which means
//! envelopes need no price map and no currency conversion. Late validation runs
//! after this plugin, so every generated entry is checked for balance and
//! currency constraints like any other transaction.
//!
//! # Accounting shape
//!
//! Every generated posting stays inside `Equity:Envelope:`, so the namespace is
//! self-contained (`Σ Equity:Envelope:* = 0`), net-worth neutral, and inert for
//! every other consumer of the ledger: one prefix is enough to ignore the whole
//! budget.
//!
//! The generated month is described in [`model`]; the configuration language in
//! [`config`]; the entry shapes in [`emit`].

mod accounts;
mod config;
mod emit;
mod inventory;
mod model;
mod money;
mod month;

use rustledger_plugin_types::{PluginInput, PluginOp, PluginOutput, wasm_plugin_main};

use crate::config::Config;

/// Entry point: parse the configuration, plan the budget months, emit both.
fn process(input: PluginInput) -> PluginOutput {
    let keep_all = || (0..input.directives.len()).map(PluginOp::Keep).collect();

    match Config::parse(&input.directives, &input.options) {
        Ok(config) => {
            let mut plan = model::plan(&config, &input.directives);
            // Configuration diagnostics come first: they describe the intent the
            // month-by-month ones were derived from.
            plan.diagnostics.splice(0..0, config.warnings);
            emit::build(&plan, input.directives.len())
        }
        Err(errors) => PluginOutput {
            ops: keep_all(),
            errors,
        },
    }
}

wasm_plugin_main! {
    process: process,
}

#[cfg(test)]
mod tests {
    use rustledger_plugin_types::{
        AmountData, CustomData, DirectiveData, DirectiveWrapper, MetaValueData, OpenData,
        PluginAccountTypes, PluginInput, PluginOp, PluginOptions, PostingData, TransactionData,
        validate_op_coverage,
    };

    use super::process;

    fn options() -> PluginOptions {
        PluginOptions {
            operating_currencies: vec!["TRY".to_string()],
            title: None,
            account_types: PluginAccountTypes {
                assets: "Assets".to_string(),
                liabilities: "Liabilities".to_string(),
                equity: "Equity".to_string(),
                income: "Income".to_string(),
                expenses: "Expenses".to_string(),
            },
        }
    }

    fn text(value: &str) -> MetaValueData {
        MetaValueData::String(value.to_string())
    }

    fn account(account: &str) -> MetaValueData {
        MetaValueData::Account(account.to_string())
    }

    fn amount(number: &str, currency: &str) -> MetaValueData {
        MetaValueData::Amount(AmountData {
            number: number.to_string(),
            currency: currency.to_string(),
        })
    }

    fn custom(date: &str, values: Vec<MetaValueData>) -> DirectiveWrapper {
        DirectiveWrapper {
            directive_type: "custom".to_string(),
            date: date.to_string(),
            filename: None,
            lineno: None,
            data: DirectiveData::Custom(CustomData {
                custom_type: "envelope".to_string(),
                values,
                metadata: Vec::new(),
            }),
        }
    }

    fn txn(date: &str, postings: &[(&str, &str, &str)]) -> DirectiveWrapper {
        DirectiveWrapper {
            directive_type: "transaction".to_string(),
            date: date.to_string(),
            filename: None,
            lineno: None,
            data: DirectiveData::Transaction(TransactionData {
                flag: "*".to_string(),
                payee: None,
                narration: "coffee".to_string(),
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

    fn insertions(output: &rustledger_plugin_types::PluginOutput) -> Vec<&DirectiveWrapper> {
        output
            .ops
            .iter()
            .filter_map(|op| match op {
                PluginOp::Insert(wrapper) => Some(wrapper),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_configured_budget_generates_envelope_entries_and_keeps_every_input() {
        let directives = vec![
            custom("2026-01-01", vec![text("start date"), text("2026-01")]),
            custom(
                "2026-01-01",
                vec![text("budget account"), text("^Assets:Bank")],
            ),
            custom(
                "2026-01-01",
                vec![
                    text("fund"),
                    account("Expenses:Food"),
                    amount("100.00", "TRY"),
                ],
            ),
            txn(
                "2026-01-02",
                &[
                    ("Income:Salary", "-5000.00", "TRY"),
                    ("Assets:Bank", "5000.00", "TRY"),
                ],
            ),
            txn(
                "2026-01-05",
                &[
                    ("Expenses:Food", "30.00", "TRY"),
                    ("Assets:Bank", "-30.00", "TRY"),
                ],
            ),
        ];
        let input = PluginInput {
            directives,
            options: options(),
            config: None,
        };

        let output = process(input);
        assert!(output.errors.is_empty(), "{:?}", output.errors);
        assert_eq!(
            output
                .ops
                .iter()
                .filter(|op| matches!(op, PluginOp::Keep(_)))
                .count(),
            5,
            "every directive the user wrote is kept"
        );
        validate_op_coverage(5, &output.ops).expect("the op list covers the input exactly");

        let inserted = insertions(&output);
        let narrations: Vec<&str> = inserted
            .iter()
            .filter_map(|wrapper| match &wrapper.data {
                DirectiveData::Transaction(transaction) => Some(transaction.narration.as_str()),
                _ => None,
            })
            .collect();
        assert!(narrations.contains(&"Envelope funding 2026-01"));
        assert!(narrations.contains(&"Envelope activity 2026-01"));
        assert!(inserted.iter().any(|wrapper| matches!(
            &wrapper.data,
            DirectiveData::Balance(balance)
                if balance.account == "Equity:Envelope:Food" && balance.amount.number == "70.00"
        )));
        assert!(inserted.iter().any(|wrapper| matches!(
            &wrapper.data,
            DirectiveData::Open(OpenData { account, .. }) if account == "Equity:Envelope:Money"
        )));
        // The user's own transaction is never rewritten.
        assert!(
            output
                .ops
                .iter()
                .all(|op| !matches!(op, PluginOp::Modify(..) | PluginOp::Delete(..)))
        );
    }

    #[test]
    fn a_configuration_without_a_start_date_reports_and_changes_nothing() {
        let directives = vec![
            custom(
                "2026-01-01",
                vec![text("budget account"), text("^Assets:Bank")],
            ),
            txn(
                "2026-01-05",
                &[
                    ("Expenses:Food", "1.00", "TRY"),
                    ("Assets:Bank", "-1.00", "TRY"),
                ],
            ),
        ];
        let input = PluginInput {
            directives,
            options: options(),
            config: None,
        };

        let output = process(input);
        assert_eq!(output.ops.len(), 2);
        assert!(matches!(output.ops[0], PluginOp::Keep(0)));
        assert!(matches!(output.ops[1], PluginOp::Keep(1)));
        assert!(
            output
                .errors
                .iter()
                .any(|error| error.message.contains("start date")),
            "{:?}",
            output.errors
        );
    }
    /// The worked example in `README.md`, pinned: if the docs and the plugin
    /// ever disagree, this test fails.
    #[test]
    fn the_readme_worked_example_holds() {
        use rustledger_plugin_types::PluginOutput;

        fn open(account: &str) -> DirectiveWrapper {
            DirectiveWrapper {
                directive_type: "open".to_string(),
                date: "2026-01-01".to_string(),
                filename: None,
                lineno: None,
                data: DirectiveData::Open(OpenData {
                    account: account.to_string(),
                    currencies: Vec::new(),
                    booking: None,
                    metadata: Vec::new(),
                }),
            }
        }

        let directives = vec![
            open("Assets:Bank"),
            open("Expenses:Groceries"),
            open("Expenses:Fees"),
            open("Income:Salary"),
            custom("2026-01-01", vec![text("start date"), text("2026-01")]),
            custom("2026-01-01", vec![text("end date"), text("2026-02")]),
            custom(
                "2026-01-01",
                vec![text("budget account"), text("^Assets:Bank")],
            ),
            custom(
                "2026-01-01",
                vec![
                    text("fund"),
                    account("Expenses:Groceries"),
                    amount("4000.00", "TRY"),
                ],
            ),
            custom(
                "2026-01-01",
                vec![
                    text("fund"),
                    account("Expenses:Fees"),
                    amount("200.00", "TRY"),
                ],
            ),
            custom(
                "2026-01-01",
                vec![
                    text("rollover"),
                    account("Expenses:Fees"),
                    MetaValueData::Bool(false),
                ],
            ),
            txn(
                "2026-01-02",
                &[
                    ("Assets:Bank", "30000.00", "TRY"),
                    ("Income:Salary", "-30000.00", "TRY"),
                ],
            ),
            txn(
                "2026-01-10",
                &[
                    ("Expenses:Groceries", "2500.00", "TRY"),
                    ("Assets:Bank", "-2500.00", "TRY"),
                ],
            ),
            txn(
                "2026-01-20",
                &[
                    ("Expenses:Fees", "40.00", "TRY"),
                    ("Assets:Bank", "-40.00", "TRY"),
                ],
            ),
            txn(
                "2026-02-06",
                &[
                    ("Expenses:Groceries", "4600.00", "TRY"),
                    ("Assets:Bank", "-4600.00", "TRY"),
                ],
            ),
            txn(
                "2026-02-20",
                &[
                    ("Expenses:Fees", "40.00", "TRY"),
                    ("Assets:Bank", "-40.00", "TRY"),
                ],
            ),
        ];

        let output: PluginOutput = process(PluginInput {
            directives,
            options: options(),
            config: None,
        });
        assert!(output.errors.is_empty(), "{:?}", output.errors);

        let mut lines: Vec<String> = Vec::new();
        for op in &output.ops {
            let PluginOp::Insert(wrapper) = op else {
                continue;
            };
            match &wrapper.data {
                DirectiveData::Open(open) => lines.push(format!("open {}", open.account)),
                DirectiveData::Balance(balance) => lines.push(format!(
                    "{} balance {} {} {}",
                    wrapper.date, balance.account, balance.amount.number, balance.amount.currency
                )),
                DirectiveData::Transaction(transaction) => {
                    lines.push(format!("{} {}", wrapper.date, transaction.narration));
                }
                _ => {}
            }
        }
        assert_eq!(
            lines,
            vec![
                "open Equity:Envelope:Fees",
                "open Equity:Envelope:Groceries",
                "open Equity:Envelope:Money",
                "open Equity:Envelope:To-Be-Budgeted",
                "2026-01-31 Envelope backing 2026-01",
                "2026-01-31 Envelope funding 2026-01",
                "2026-01-31 Envelope activity 2026-01",
                "2026-01-31 Envelope rollover 2026-01",
                "2026-02-28 Envelope backing 2026-02",
                "2026-02-28 Envelope funding 2026-02",
                "2026-02-28 Envelope activity 2026-02",
                "2026-02-28 Envelope rollover 2026-02",
                // January's fees sweep back to the pool, its groceries carry.
                "2026-02-01 balance Equity:Envelope:Groceries 1500.00 TRY",
                "2026-02-01 balance Equity:Envelope:To-Be-Budgeted 25960.00 TRY",
                "2026-03-01 balance Equity:Envelope:Groceries 900.00 TRY",
                "2026-03-01 balance Equity:Envelope:To-Be-Budgeted 21920.00 TRY",
            ]
        );
    }
}

#[cfg(test)]
mod validation_tests;
