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
            custom("2026-01-01", vec![text("budget account"), text("^Assets:Bank")]),
            custom(
                "2026-01-01",
                vec![text("fund"), account("Expenses:Food"), amount("100.00", "TRY")],
            ),
            txn(
                "2026-01-02",
                &[("Income:Salary", "-5000.00", "TRY"), ("Assets:Bank", "5000.00", "TRY")],
            ),
            txn(
                "2026-01-05",
                &[("Expenses:Food", "30.00", "TRY"), ("Assets:Bank", "-30.00", "TRY")],
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
            custom("2026-01-01", vec![text("budget account"), text("^Assets:Bank")]),
            txn(
                "2026-01-05",
                &[("Expenses:Food", "1.00", "TRY"), ("Assets:Bank", "-1.00", "TRY")],
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
}
