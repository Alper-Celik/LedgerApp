// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Turning a [`Plan`](crate::model::Plan) into the host's op list.
//!
//! The plugin never changes what the user wrote: every input directive is
//! `Keep`n, and the plan's opens, entries and assertions are `Insert`ed. That
//! keeps the user's transactions untouched (their source spans, their order,
//! their formatting) and makes every generated number visible as its own
//! transaction.
//!
//! Shapes emitted, all with the narration from
//! [`EntryKind::label`](crate::model::EntryKind::label) plus the month, the tag
//! `envelope`, and the account's own commodity:
//!
//! * one `transaction` per [`Entry`](crate::model::Entry), postings in the
//!   order the model produced them;
//! * one `open` per planned account, dated at the first budget month;
//! * one `balance` per [`Assertion`](crate::model::Assertion), dated at the
//!   month's end — rustledger's Late validation then verifies the plugin's own
//!   arithmetic;
//! * every diagnostic from the plan (the host validates the op list's coverage
//!   of the input, so this module does not re-check it in production code).

use rustledger_plugin_types::{
    BalanceData, DirectiveData, DirectiveWrapper, MetaValueData, OpenData, PluginOp, PluginOutput,
    PostingData, TransactionData,
};

use crate::model::Plan;

/// Emit the op list: keep the input, insert the plan.
#[must_use]
pub fn build(plan: &Plan, input_len: usize) -> PluginOutput {
    let mut ops: Vec<PluginOp> = (0..input_len).map(PluginOp::Keep).collect();

    ops.extend(plan.opens.iter().map(|open| {
        PluginOp::Insert(wrapper(
            open.date.clone(),
            "open",
            DirectiveData::Open(OpenData {
                account: open.account.clone(),
                currencies: Vec::new(),
                booking: None,
                metadata: Vec::new(),
            }),
        ))
    }));

    for month in &plan.months {
        for entry in &month.entries {
            let postings: Vec<PostingData> = entry
                .legs
                .iter()
                .filter(|leg| !leg.amount.is_zero())
                .map(|leg| PostingData {
                    account: leg.account.clone(),
                    units: Some(leg.amount.to_amount_data()),
                    cost: None,
                    price: None,
                    flag: None,
                    metadata: Vec::new(),
                    span: None,
                })
                .collect();
            // An entry whose every leg is zero is noise: emitting a
            // transaction with no postings would fail validation.
            if postings.is_empty() {
                continue;
            }
            ops.push(PluginOp::Insert(wrapper(
                entry.date.clone(),
                "transaction",
                DirectiveData::Transaction(TransactionData {
                    flag: "*".to_string(),
                    payee: None,
                    narration: entry.narration.clone(),
                    tags: vec!["envelope".to_string()],
                    links: Vec::new(),
                    metadata: vec![
                        (
                            "envelope-kind".to_string(),
                            MetaValueData::String(entry.kind.label().to_string()),
                        ),
                        (
                            "envelope-month".to_string(),
                            MetaValueData::String(month.month.label()),
                        ),
                    ],
                    postings,
                }),
            )));
        }
    }

    for month in &plan.months {
        for assertion in &month.assertions {
            ops.push(PluginOp::Insert(wrapper(
                assertion.date.clone(),
                "balance",
                DirectiveData::Balance(BalanceData {
                    account: assertion.account.clone(),
                    amount: assertion.amount.to_amount_data(),
                    tolerance: None,
                    metadata: Vec::new(),
                }),
            )));
        }
    }

    // The host checks op coverage as well (`validate_op_coverage` is shared by
    // the loader's plugin pass and the FFI's), so a malformed list is reported
    // there rather than silently dropped; the test module asserts coverage here.
    PluginOutput {
        ops,
        errors: plan.diagnostics.clone(),
    }
}

/// A synthesized directive wrapper: no source identity, and the directive type
/// named explicitly the way `rustledger_plugin_types` derives it from the data.
fn wrapper(date: String, kind: &str, data: DirectiveData) -> DirectiveWrapper {
    DirectiveWrapper {
        directive_type: kind.to_string(),
        date,
        filename: None,
        lineno: None,
        data,
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;
    use rustledger_plugin_types::{
        DirectiveData, MetaValueData, PluginError, PluginOp, validate_op_coverage,
    };

    use super::build;
    use crate::model::{Assertion, Entry, EntryKind, Leg, MonthPlan, Open, Plan};
    use crate::money::Money;
    use crate::month::Month;

    fn amount(minor: i64, scale: u32, currency: &str) -> Money {
        Money::new(Decimal::new(minor, scale), currency)
    }

    fn leg(account: &str, minor: i64, scale: u32, currency: &str) -> Leg {
        Leg {
            account: account.to_string(),
            amount: amount(minor, scale, currency),
        }
    }

    /// A plan exercising every emitted shape, in the order they appear.
    fn sample_plan() -> Plan {
        Plan {
            opens: vec![
                Open {
                    date: "2026-10-01".to_string(),
                    account: "Equity:Envelope:Groceries".to_string(),
                },
                Open {
                    date: "2026-10-01".to_string(),
                    account: "Equity:Envelope:To-Be-Budgeted".to_string(),
                },
            ],
            months: vec![
                MonthPlan {
                    month: Month::new(2026, 10),
                    entries: vec![
                        Entry {
                            kind: EntryKind::Funding,
                            date: "2026-10-01".to_string(),
                            narration: "Envelope funding 2026-10".to_string(),
                            legs: vec![
                                leg("Equity:Envelope:Groceries", 400000, 2, "TRY"),
                                leg("Equity:Envelope:To-Be-Budgeted", -400000, 2, "TRY"),
                            ],
                        },
                        Entry {
                            kind: EntryKind::Activity,
                            date: "2026-10-31".to_string(),
                            narration: "Envelope activity 2026-10".to_string(),
                            legs: vec![
                                // A zero leg is dropped.
                                leg("Equity:Envelope:Groceries", 0, 2, "TRY"),
                                leg("Equity:Envelope:Uncategorized", 1250, 2, "TRY"),
                            ],
                        },
                        Entry {
                            kind: EntryKind::Sweep,
                            date: "2026-10-31".to_string(),
                            narration: "Envelope rollover 2026-10".to_string(),
                            // Every leg zero: no transaction is emitted.
                            legs: vec![leg("Equity:Envelope:Groceries", 0, 2, "TRY")],
                        },
                    ],
                    assertions: vec![Assertion {
                        date: "2026-10-31".to_string(),
                        account: "Equity:Envelope:Groceries".to_string(),
                        amount: amount(400000, 2, "TRY"),
                    }],
                },
                MonthPlan {
                    month: Month::new(2026, 11),
                    entries: vec![Entry {
                        kind: EntryKind::Money,
                        date: "2026-11-01".to_string(),
                        narration: "Envelope backing 2026-11".to_string(),
                        legs: vec![
                            leg("Equity:Envelope:Money", 9999, 3, "USD"),
                            leg("Equity:Envelope:To-Be-Budgeted", -9999, 3, "USD"),
                        ],
                    }],
                    assertions: vec![Assertion {
                        date: "2026-11-30".to_string(),
                        account: "Equity:Envelope:Money".to_string(),
                        amount: amount(-9999, 3, "USD"),
                    }],
                },
            ],
            diagnostics: Vec::new(),
        }
    }

    fn inserts(output: &rustledger_plugin_types::PluginOutput) -> Vec<DirectiveData> {
        output
            .ops
            .iter()
            .filter_map(|op| match op {
                PluginOp::Insert(wrapper) => Some(wrapper.data.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn keeps_all_input_first_and_in_order() {
        let plan = sample_plan();
        let output = build(&plan, 3);

        assert_eq!(output.ops.len(), 3 + 2 + 3 + 2);
        let keeps: Vec<usize> = output.ops[..3]
            .iter()
            .map(|op| match op {
                PluginOp::Keep(i) => *i,
                other => panic!("expected Keep at the front, got {other:?}"),
            })
            .collect();
        assert_eq!(keeps, vec![0, 1, 2]);
        assert!(
            output.ops[3..].iter().all(|op| matches!(op, PluginOp::Insert(_))),
            "only Inserts follow the Keeps"
        );
    }

    #[test]
    fn zero_input_has_no_ops() {
        let output = build(&Plan::default(), 0);
        assert!(output.ops.is_empty());
        assert!(output.errors.is_empty());
    }

    #[test]
    fn opens_carry_account_and_empty_defaults() {
        let output = build(&sample_plan(), 0);
        let inserted = inserts(&output);
        match &inserted[0] {
            DirectiveData::Open(open) => {
                assert_eq!(open.account, "Equity:Envelope:Groceries");
                assert!(open.currencies.is_empty());
                assert!(open.booking.is_none());
                assert!(open.metadata.is_empty());
            }
            other => panic!("expected Open, got {other:?}"),
        }
        match &output.ops[0] {
            PluginOp::Insert(wrapper) => {
                assert_eq!(wrapper.date, "2026-10-01");
                assert!(wrapper.filename.is_none());
                assert!(wrapper.lineno.is_none());
            }
            other => panic!("expected Insert, got {other:?}"),
        }
    }

    #[test]
    fn transaction_shape_and_amount_strings() {
        let output = build(&sample_plan(), 0);
        // opens (2), then the first transaction.
        match &output.ops[2] {
            PluginOp::Insert(wrapper) => {
                assert_eq!(wrapper.date, "2026-10-01");
                match &wrapper.data {
                    DirectiveData::Transaction(txn) => {
                        assert_eq!(txn.flag, "*");
                        assert!(txn.payee.is_none());
                        assert_eq!(txn.narration, "Envelope funding 2026-10");
                        assert_eq!(txn.tags, vec!["envelope".to_string()]);
                        assert!(txn.links.is_empty());
                        assert_eq!(
                            txn.metadata,
                            vec![
                                (
                                    "envelope-kind".to_string(),
                                    MetaValueData::String("Envelope funding".to_string())
                                ),
                                (
                                    "envelope-month".to_string(),
                                    MetaValueData::String("2026-10".to_string())
                                )
                            ]
                        );
                        assert_eq!(txn.postings.len(), 2);
                        assert_eq!(txn.postings[0].account, "Equity:Envelope:Groceries");
                        let units = txn.postings[0].units.as_ref().expect("units");
                        assert_eq!(units.number, "4000.00");
                        assert_eq!(units.currency, "TRY");
                        assert!(txn.postings[0].cost.is_none());
                        assert!(txn.postings[0].price.is_none());
                        assert!(txn.postings[0].flag.is_none());
                        assert!(txn.postings[0].metadata.is_empty());
                        assert!(txn.postings[0].span.is_none());

                        let contra = txn.postings[1].units.as_ref().expect("units");
                        assert_eq!(contra.number, "-4000.00");
                        assert_eq!(contra.currency, "TRY");
                    }
                    other => panic!("expected Transaction, got {other:?}"),
                }
            }
            other => panic!("expected Insert, got {other:?}"),
        }
    }

    #[test]
    fn zero_legs_and_all_zero_entries_are_skipped() {
        let output = build(&sample_plan(), 0);
        let inserted = inserts(&output);
        let transactions: Vec<&DirectiveData> = inserted
            .iter()
            .filter(|data| matches!(data, DirectiveData::Transaction(_)))
            .map(|data| data)
            .collect();
        // Rollover entry (all zero legs) dropped; activity (one zero leg) kept
        // with a single posting.
        assert_eq!(transactions.len(), 3);
        match transactions[1] {
            DirectiveData::Transaction(txn) => {
                assert_eq!(txn.narration, "Envelope activity 2026-10");
                assert_eq!(txn.postings.len(), 1);
                assert_eq!(txn.postings[0].account, "Equity:Envelope:Uncategorized");
                assert_eq!(txn.postings[0].units.as_ref().unwrap().number, "12.50");
            }
            other => panic!("expected Transaction, got {other:?}"),
        }
    }

    #[test]
    fn balance_assertions_carry_amount_strings() {
        let output = build(&sample_plan(), 0);
        let inserted = inserts(&output);
        let balances: Vec<&DirectiveData> = inserted
            .iter()
            .filter(|data| matches!(data, DirectiveData::Balance(_)))
            .map(|data| data)
            .collect();
        assert_eq!(balances.len(), 2);
        match balances[0] {
            DirectiveData::Balance(balance) => {
                assert_eq!(balance.account, "Equity:Envelope:Groceries");
                assert_eq!(balance.amount.number, "4000.00");
                assert_eq!(balance.amount.currency, "TRY");
                assert!(balance.tolerance.is_none());
                assert!(balance.metadata.is_empty());
            }
            other => panic!("expected Balance, got {other:?}"),
        }
        match balances[1] {
            DirectiveData::Balance(balance) => {
                assert_eq!(balance.account, "Equity:Envelope:Money");
                assert_eq!(balance.amount.number, "-9.999");
                assert_eq!(balance.amount.currency, "USD");
            }
            other => panic!("expected Balance, got {other:?}"),
        }
    }

    #[test]
    fn diagnostics_land_in_errors_in_order() {
        let mut plan = sample_plan();
        plan.diagnostics = vec![
            PluginError::warning("first warning"),
            PluginError::error("second error").at("ledger.beancount", 12),
        ];
        let output = build(&plan, 1);
        assert_eq!(output.errors.len(), 2);
        assert_eq!(output.errors[0].message, "first warning");
        assert_eq!(output.errors[1].message, "second error");
    }

    #[test]
    fn built_op_list_covers_the_input() {
        let plan = sample_plan();
        for input_len in 0..5 {
            let output = build(&plan, input_len);
            assert_eq!(
                validate_op_coverage(input_len, &output.ops),
                Ok(())
            );
        }
    }
}
