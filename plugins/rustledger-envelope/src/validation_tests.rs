// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The emitted directives, judged by rustledger's own validator.
//!
//! [`crate::model`]'s tests prove the plan's arithmetic is internally
//! consistent. They cannot prove that the *directives the plugin emits* are
//! directives rustledger accepts: the host's rules are its own crate's, and
//! the plugin's idea of a category's balance, of an account's lifecycle and of
//! what balances is only correct if rustledger agrees. This test closes that
//! gap by driving the real entry point ([`crate::process`]) over a three-month,
//! two-commodity fixture and then materialising the op list exactly the way the
//! host does — every `Keep(i)` back to fixture directive `i`, every `Insert`
//! converted to a [`rustledger_core::Directive`] — and running the two-phase
//! [`rustledger_validate::ValidationSession`] the loader runs (early before
//! booking, late after the plugin pass, then `finalize`).
//!
//! What that buys, in error codes:
//!
//! * **E1001** — every account a generated posting lands on must be opened, so
//!   the plugin's `open`s have to cover `Equity:Envelope:Money`,
//!   `Equity:Envelope:To-Be-Budgeted` and every category it ever posts to;
//! * **E3001** — every generated transaction must balance by rustledger's
//!   weight rules, not merely by the plan's own inventory;
//! * **E2001** — every `balance` the plugin asserts must match the inventory
//!   rustledger computes from the generated transactions. rustledger's check
//!   covers the account *and its subaccounts*, so the fixture funds
//!   `Expenses:Groceries` while every purchase lands on
//!   `Expenses:Groceries:Bread`: the parent has to be asserted as the subtree
//!   sum (12000.00 TRY assigned, 1800.00 TRY spent below it) and the child as
//!   its own balance, and both have to match what the validator computes. An
//!   assertion built from the parent account's own inventory fails here.
//!
//! Finally the budget invariant the README states — `Σ Equity:Envelope:* = 0`
//! per commodity and `To-Be-Budgeted = on-budget money − Σ categories` — is
//! asserted independently, from the materialised directives alone, so a plugin
//! and a validator that were wrong in the same way would still be caught.

use std::collections::BTreeMap;
use std::str::FromStr;

use rust_decimal::Decimal;
use rustledger_core::{
    Account, Amount, Balance, Currency, Custom, Directive, Link, MetaValue, Metadata, NaiveDate,
    Open, Posting, Spanned, Tag, Transaction, sort_directives,
};
use rustledger_plugin_types::{
    AmountData, CustomData, DirectiveData, DirectiveWrapper, MetaValueData, OpenData,
    PluginAccountTypes, PluginInput, PluginOp, PluginOptions, PostingData, TransactionData,
    validate_op_coverage,
};
use rustledger_validate::{Severity, ValidationOptions, ValidationSession};

use crate::accounts::{MONEY, TO_BE_BUDGETED, category_account, is_generated};
use crate::process;

// ── The fixture ─────────────────────────────────────────────────────────────

/// The ledger options the host passes alongside the directives.
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

fn account_value(name: &str) -> MetaValueData {
    MetaValueData::Account(name.to_string())
}

fn amount_value(number: &str, currency: &str) -> MetaValueData {
    MetaValueData::Amount(AmountData {
        number: number.to_string(),
        currency: currency.to_string(),
    })
}

fn flag(value: bool) -> MetaValueData {
    MetaValueData::Bool(value)
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

/// An account the fixture's own postings use.
///
/// The plugin opens only its `Equity:Envelope:` accounts, so the ledgers the
/// tests feed it must open everything they post to themselves; leaving one out
/// is exactly the E1001 this file is built to catch.
fn open(date: &str, account: &str) -> DirectiveWrapper {
    DirectiveWrapper {
        directive_type: "open".to_string(),
        date: date.to_string(),
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

fn txn(date: &str, narration: &str, postings: &[(&str, &str, &str)]) -> DirectiveWrapper {
    DirectiveWrapper {
        directive_type: "transaction".to_string(),
        date: date.to_string(),
        filename: None,
        lineno: None,
        data: DirectiveData::Transaction(TransactionData {
            flag: "*".to_string(),
            payee: None,
            narration: narration.to_string(),
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

/// Three budget months over TRY and USD, in the shape the README describes.
///
/// * income into an on-budget account: `^Assets:Bank` covers `Assets:Bank`
///   (TRY) and `Assets:Bank:US` (USD), funded by `Income:Salary`;
/// * a funded category: `Expenses:Food`, assigned 4000.00 TRY every month and
///   capped to 3000.00 TRY at each month boundary, so the excess is swept back
///   to the pool;
/// * a funded category whose spending lands on a *child*: `Expenses:Groceries`
///   assigned 4000.00 TRY every month, with every purchase on
///   `Expenses:Groceries:Bread` — so the parent's assertion can only be right if
///   it is the subtree sum rustledger computes, not the parent account's own
///   balance;
/// * an unfunded one: `Expenses:Fees`, never assigned and reset at each month
///   boundary, so its deficit is charged to the pool;
/// * a second commodity: USD spending in a TRY ledger, funded by USD income
///   rather than by converting TRY — nothing here is ever converted.
fn fixture() -> Vec<DirectiveWrapper> {
    vec![
        open("2026-01-01", "Assets:Bank"),
        open("2026-01-01", "Assets:Bank:US"),
        open("2026-01-01", "Expenses:Food"),
        open("2026-01-01", "Expenses:Fees"),
        open("2026-01-01", "Expenses:Groceries"),
        open("2026-01-01", "Expenses:Groceries:Bread"),
        open("2026-01-01", "Income:Salary"),
        custom("2026-01-01", vec![text("start date"), text("2026-01")]),
        custom("2026-01-01", vec![text("end date"), text("2026-03")]),
        custom(
            "2026-01-01",
            vec![text("budget account"), text("^Assets:Bank")],
        ),
        custom(
            "2026-01-01",
            vec![
                text("fund"),
                account_value("Expenses:Food"),
                amount_value("4000.00", "TRY"),
            ],
        ),
        custom(
            "2026-01-01",
            vec![
                text("fund"),
                account_value("Expenses:Groceries"),
                amount_value("4000.00", "TRY"),
            ],
        ),
        custom(
            "2026-01-01",
            vec![
                text("rollover"),
                account_value("Expenses:Food"),
                amount_value("3000.00", "TRY"),
            ],
        ),
        custom(
            "2026-01-01",
            vec![
                text("rollover"),
                account_value("Expenses:Fees"),
                flag(false),
            ],
        ),
        // January: income, funded spending, unfunded spending, and the USD pair.
        txn(
            "2026-01-05",
            "salary",
            &[
                ("Assets:Bank", "9000.00", "TRY"),
                ("Income:Salary", "-9000.00", "TRY"),
            ],
        ),
        txn(
            "2026-01-09",
            "groceries",
            &[
                ("Expenses:Food", "1200.00", "TRY"),
                ("Assets:Bank", "-1200.00", "TRY"),
            ],
        ),
        txn(
            "2026-01-25",
            "bread",
            &[
                ("Expenses:Groceries:Bread", "700.00", "TRY"),
                ("Assets:Bank", "-700.00", "TRY"),
            ],
        ),
        txn(
            "2026-01-12",
            "contract",
            &[
                ("Assets:Bank:US", "300.00", "USD"),
                ("Income:Salary", "-300.00", "USD"),
            ],
        ),
        txn(
            "2026-01-17",
            "bank fee",
            &[
                ("Expenses:Fees", "80.00", "TRY"),
                ("Assets:Bank", "-80.00", "TRY"),
            ],
        ),
        txn(
            "2026-01-21",
            "cloud bill",
            &[
                ("Expenses:Fees", "20.00", "USD"),
                ("Assets:Bank:US", "-20.00", "USD"),
            ],
        ),
        // February.
        txn(
            "2026-02-04",
            "salary",
            &[
                ("Assets:Bank", "9000.00", "TRY"),
                ("Income:Salary", "-9000.00", "TRY"),
            ],
        ),
        txn(
            "2026-02-11",
            "groceries",
            &[
                ("Expenses:Food", "1500.00", "TRY"),
                ("Assets:Bank", "-1500.00", "TRY"),
            ],
        ),
        txn(
            "2026-02-15",
            "bread",
            &[
                ("Expenses:Groceries:Bread", "500.00", "TRY"),
                ("Assets:Bank", "-500.00", "TRY"),
            ],
        ),
        txn(
            "2026-02-06",
            "contract",
            &[
                ("Assets:Bank:US", "200.00", "USD"),
                ("Income:Salary", "-200.00", "USD"),
            ],
        ),
        txn(
            "2026-02-19",
            "bank fee",
            &[
                ("Expenses:Fees", "60.00", "TRY"),
                ("Assets:Bank", "-60.00", "TRY"),
            ],
        ),
        txn(
            "2026-02-24",
            "cloud bill",
            &[
                ("Expenses:Fees", "30.00", "USD"),
                ("Assets:Bank:US", "-30.00", "USD"),
            ],
        ),
        // March: the USD stops, the TRY continues.
        txn(
            "2026-03-03",
            "salary",
            &[
                ("Assets:Bank", "9000.00", "TRY"),
                ("Income:Salary", "-9000.00", "TRY"),
            ],
        ),
        txn(
            "2026-03-08",
            "groceries",
            &[
                ("Expenses:Food", "900.00", "TRY"),
                ("Assets:Bank", "-900.00", "TRY"),
            ],
        ),
        txn(
            "2026-03-12",
            "bread",
            &[
                ("Expenses:Groceries:Bread", "600.00", "TRY"),
                ("Assets:Bank", "-600.00", "TRY"),
            ],
        ),
    ]
}

// ── Host-side materialisation ───────────────────────────────────────────────

fn decimal(number: &str, label: &str) -> Decimal {
    Decimal::from_str(number.trim())
        .unwrap_or_else(|problem| panic!("{label}: {number:?} is not a decimal: {problem}"))
}

fn parse_date(value: &str, label: &str) -> NaiveDate {
    NaiveDate::from_str(value)
        .unwrap_or_else(|problem| panic!("{label}: date {value:?} does not parse: {problem}"))
}

fn amount_of(data: &AmountData, label: &str) -> Amount {
    Amount::new(decimal(&data.number, label), data.currency.as_str())
}

fn meta_value(value: &MetaValueData, label: &str) -> MetaValue {
    match value {
        MetaValueData::String(text) => MetaValue::String(text.clone()),
        MetaValueData::Number(number) => MetaValue::Number(decimal(number, label)),
        MetaValueData::Date(date) => MetaValue::Date(parse_date(date, label)),
        MetaValueData::Account(name) => MetaValue::Account(Account::new(name.as_str())),
        MetaValueData::Currency(currency) => MetaValue::Currency(Currency::new(currency.as_str())),
        MetaValueData::Tag(tag) => MetaValue::Tag(Tag::new(tag.as_str())),
        MetaValueData::Link(link) => MetaValue::Link(Link::new(link.as_str())),
        MetaValueData::Amount(amount) => MetaValue::Amount(amount_of(amount, label)),
        MetaValueData::Bool(flag) => MetaValue::Bool(*flag),
    }
}

fn metadata(entries: &[(String, MetaValueData)], label: &str) -> Metadata {
    entries
        .iter()
        .map(|(key, value)| (key.clone(), meta_value(value, label)))
        .collect()
}

/// One posting, as the host would build it from the wire format.
fn posting_of(posting: &PostingData, label: &str) -> Posting {
    assert!(
        posting.cost.is_none() && posting.price.is_none(),
        "{label}: this fixture carries no cost or price annotations, so their \
         conversion is not implemented here"
    );
    let mut out = match &posting.units {
        Some(units) => Posting::new(posting.account.as_str(), amount_of(units, label)),
        None => Posting::auto(posting.account.as_str()),
    };
    if let Some(flag) = &posting.flag {
        let mut characters = flag.chars();
        out.flag = match (characters.next(), characters.next()) {
            (Some(character), None) => Some(character),
            _ => panic!("{label}: posting flag {flag:?} is not one character"),
        };
    }
    out.meta = metadata(&posting.metadata, label);
    out
}

fn transaction_of(date: NaiveDate, data: &TransactionData, label: &str) -> Transaction {
    let mut characters = data.flag.chars();
    let flag = match (characters.next(), characters.next()) {
        (Some(flag), None) => flag,
        _ => panic!(
            "{label}: transaction flag {:?} is not one character",
            data.flag
        ),
    };
    let mut transaction = Transaction::new(date, data.narration.clone()).with_flag(flag);
    transaction.payee = data.payee.clone().map(Into::into);
    transaction.tags = data.tags.iter().map(|tag| Tag::new(tag.as_str())).collect();
    transaction.links = data
        .links
        .iter()
        .map(|link| Link::new(link.as_str()))
        .collect();
    transaction.meta = metadata(&data.metadata, label);
    for posting in &data.postings {
        transaction
            .postings
            .push(Spanned::synthesized(posting_of(posting, label)));
    }
    transaction
}

/// The host's conversion of one wrapper: the directive it names.
///
/// Only the directive kinds a directive plugin may see are handled. A kind this
/// fixture cannot produce is a loud failure naming the directive, not a
/// silently different ledger.
fn convert(wrapper: &DirectiveWrapper, origin: &str) -> Directive {
    let label = format!(
        "{origin} ({} dated {})",
        wrapper.directive_type, wrapper.date
    );
    let date = parse_date(&wrapper.date, &label);
    match &wrapper.data {
        DirectiveData::Custom(data) => {
            let mut directive = Custom::new(date, data.custom_type.clone());
            for value in &data.values {
                directive = directive.with_value(meta_value(value, &label));
            }
            directive.meta = metadata(&data.metadata, &label);
            Directive::Custom(directive)
        }
        DirectiveData::Transaction(data) => {
            Directive::Transaction(transaction_of(date, data, &label))
        }
        DirectiveData::Open(data) => {
            let mut open = Open::new(date, data.account.as_str());
            open.currencies = data.currencies.iter().map(Currency::new).collect();
            open.booking = data.booking.clone();
            open.meta = metadata(&data.metadata, &label);
            Directive::Open(open)
        }
        DirectiveData::Balance(data) => {
            let mut balance =
                Balance::new(date, data.account.as_str(), amount_of(&data.amount, &label));
            balance.tolerance = data
                .tolerance
                .as_ref()
                .map(|tolerance| decimal(tolerance, &label));
            balance.meta = metadata(&data.metadata, &label);
            Directive::Balance(balance)
        }
        other => panic!("{label}: conversion does not handle {other:?}"),
    }
}

#[test]
fn the_emitted_budget_passes_rustledgers_own_validation() {
    let directives = fixture();
    let output = process(PluginInput {
        directives: directives.clone(),
        options: options(),
        config: None,
    });

    // The plugin's own verdict on the fixture: no diagnostics at all (a
    // warning here would mean the fixture leans on a behaviour the plugin
    // reports rather than generates).
    assert!(
        output.errors.is_empty(),
        "the plugin reported diagnostics: {:?}",
        output.errors
    );
    validate_op_coverage(directives.len(), &output.ops)
        .expect("the op list is a cover of the fixture");

    // Materialise the op list the way the host does.
    let mut materialised: Vec<Directive> = Vec::with_capacity(output.ops.len());
    for (position, op) in output.ops.iter().enumerate() {
        match op {
            PluginOp::Keep(index) => {
                materialised.push(convert(
                    &directives[*index],
                    &format!("fixture directive {index}"),
                ));
            }
            PluginOp::Insert(wrapper) => {
                materialised.push(convert(wrapper, &format!("plugin insert {position}")));
            }
            other => panic!("a directive plugin may only keep and insert, got {other:?}"),
        }
    }
    sort_directives(&mut materialised);

    // The plugin opens its own namespace, once per account it posts to.
    let food = category_account("Expenses:Food", "Expenses").expect("Food is a category");
    let fees = category_account("Expenses:Fees", "Expenses").expect("Fees is a category");
    let groceries =
        category_account("Expenses:Groceries", "Expenses").expect("Groceries is a category");
    let bread =
        category_account("Expenses:Groceries:Bread", "Expenses").expect("Bread is a category");
    for account in [
        MONEY,
        TO_BE_BUDGETED,
        food.as_str(),
        fees.as_str(),
        groceries.as_str(),
        bread.as_str(),
    ] {
        assert!(
            materialised.iter().any(
                |directive| matches!(directive, Directive::Open(open) if open.account == account)
            ),
            "the plugin never opened {account}"
        );
    }

    // The loader's order: sort, run early, book/run plugins, run late, finalize.
    let today = parse_date("2026-12-31", "the test's today");
    let (session, early) =
        ValidationSession::new(ValidationOptions::default()).run_early(&materialised, today);
    let (session, late) = session.run_late(&materialised, today);
    let actuals = session.balance_actuals().to_vec();
    let deferred = session.finalize();

    let failures: Vec<String> = early
        .iter()
        .chain(&late)
        .chain(&deferred)
        .filter(|error| error.code.severity() == Severity::Error)
        .map(|error| format!("[{}] {} {}", error.code.code(), error.date, error.message))
        .collect();
    assert!(
        failures.is_empty(),
        "rustledger rejects the plugin's ledger:\n{}",
        failures.join("\n")
    );

    // Every `balance` the plugin emitted reached the validator's Late check and
    // matched the inventory it computed there.
    let asserted = materialised
        .iter()
        .filter(|directive| matches!(directive, Directive::Balance(_)))
        .count();
    assert!(
        asserted > 0,
        "the plugin must assert its arithmetic for the validator to check it"
    );
    assert_eq!(
        actuals.len(),
        asserted,
        "every emitted balance assertion must reach the validator's check"
    );
    let wrong: Vec<String> = actuals
        .iter()
        .filter(|actual| actual.exceeds_tolerance)
        .map(|actual| {
            format!(
                "{} {} {} asserted, off by {}",
                actual.date, actual.account, actual.currency, actual.diff
            )
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "the plugin's assertions do not match the validator's inventories:\n{}",
        wrong.join("\n")
    );

    // ── The budget invariant, from the materialised directives alone ──
    //
    // Weights are units here: the fixture carries no costs or prices, so what a
    // posting is worth is exactly what `crate::model` mirroring it would read.
    let end_of_the_horizon = parse_date("2026-03-31", "the last day of the horizon");
    let mut per_account: BTreeMap<(String, String), Decimal> = BTreeMap::new();
    for directive in &materialised {
        let Directive::Transaction(transaction) = directive else {
            continue;
        };
        if transaction.date > end_of_the_horizon {
            continue;
        }
        for posting in &transaction.postings {
            let Some(units) = posting.amount() else {
                continue;
            };
            *per_account
                .entry((posting.account.to_string(), units.currency.to_string()))
                .or_default() += units.number;
        }
    }
    let balance = |account: &str, commodity: &str| -> Decimal {
        per_account
            .get(&(account.to_string(), commodity.to_string()))
            .copied()
            .unwrap_or_default()
    };
    let commodities = ["TRY", "USD"];

    // `Σ Equity:Envelope:* = 0`: the budget is net-worth neutral.
    for commodity in commodities {
        let total: Decimal = per_account
            .iter()
            .filter(|((account, _), _)| is_generated(account))
            .filter(|((_, owned), _)| owned == commodity)
            .map(|(_, number)| *number)
            .sum();
        assert!(
            total.is_zero(),
            "the envelope namespace holds {total} {commodity}, not zero"
        );
    }

    // `To-Be-Budgeted = on-budget money − Σ categories`, and the contra is the
    // negated on-budget money.
    let on_budget = ["Assets:Bank", "Assets:Bank:US"];
    // The fixture never posts to the bare expense root, so
    // `Equity:Envelope:Uncategorized` does not exist: every other generated
    // account is a category.
    let is_category =
        |account: &str| is_generated(account) && account != MONEY && account != TO_BE_BUDGETED;
    for commodity in commodities {
        let money: Decimal = on_budget
            .iter()
            .map(|account| balance(account, commodity))
            .sum();
        let categories: Decimal = per_account
            .iter()
            .filter(|((account, _), _)| is_category(account))
            .filter(|((_, owned), _)| owned == commodity)
            .map(|(_, number)| *number)
            .sum();
        assert_eq!(
            balance(TO_BE_BUDGETED, commodity),
            money - categories,
            "To-Be-Budgeted must be on-budget money minus the categories in {commodity}"
        );
        assert_eq!(
            balance(MONEY, commodity),
            -money,
            "the money contra must be the negated on-budget money in {commodity}"
        );
    }

    // The fixture's arithmetic, by hand: Food was assigned 4000.00 TRY a month
    // and the cap swept everything above 3000.00 TRY back each boundary, so
    // exactly 3000.00 TRY is left; Groceries was assigned 4000.00 TRY a month
    // with all 1800.00 TRY of its spending below it, so its own account holds
    // 12000.00 TRY and Bread -1800.00 TRY; the unfunded spending was reset to
    // the pool, so To-Be-Budgeted is exactly the on-budget money of each
    // commodity.
    assert_eq!(
        balance(&food, "TRY"),
        decimal("3000.00", "the Food balance")
    );
    assert_eq!(
        balance(&groceries, "TRY"),
        decimal("12000.00", "the Groceries balance")
    );
    assert_eq!(
        balance(&bread, "TRY"),
        decimal("-1800.00", "the Bread balance")
    );
    assert_eq!(
        balance(TO_BE_BUDGETED, "TRY"),
        decimal("8260.00", "To-Be-Budgeted in TRY")
    );
    assert_eq!(
        balance(TO_BE_BUDGETED, "USD"),
        decimal("450.00", "To-Be-Budgeted in USD")
    );
    assert_eq!(
        balance(MONEY, "TRY"),
        decimal("-21460.00", "the money contra in TRY")
    );
    assert_eq!(
        balance(MONEY, "USD"),
        decimal("-450.00", "the money contra in USD")
    );

    // And the assertions themselves say these numbers — one per month, dated
    // the first of the next, so each sees its whole month including the sweep.
    // These are the figures rustledger checked (and `actuals` above proves it
    // checked them), pinned against the fixture's arithmetic by hand.
    let assertion = |account: &str, commodity: &str, date: NaiveDate| -> Option<Decimal> {
        actuals
            .iter()
            .find(|actual| {
                actual.account == account && actual.currency == commodity && actual.date == date
            })
            .map(|actual| actual.asserted)
    };
    let february = parse_date("2026-02-01", "January's assertion date");
    let march = parse_date("2026-03-01", "February's assertion date");
    let april = parse_date("2026-04-01", "March's assertion date");
    // Food: assigned 4000.00 TRY a month, spent 1200.00 then 1500.00 then
    // 900.00, and capped at 3000.00 TRY — so 2800.00 after January, then the
    // cap from February on.
    for (date, month, expected) in [
        (february, "January's", "2800.00"),
        (march, "February's", "3000.00"),
        (april, "March's", "3000.00"),
    ] {
        assert_eq!(
            assertion(&food, "TRY", date),
            Some(decimal(expected, "a pinned figure")),
            "{month} Food assertion"
        );
    }
    // Groceries is the parent whose spending happens below it, so its assertion
    // is the subtree sum — the assignments of 4000.00 TRY a month plus what
    // Bread spent — and it differs from the parent account's own 12000.00 TRY.
    for (date, month, expected) in [
        (february, "January's", "3300.00"),
        (march, "February's", "6800.00"),
        (april, "March's", "10200.00"),
    ] {
        assert_eq!(
            assertion(&groceries, "TRY", date),
            Some(decimal(expected, "a pinned figure")),
            "{month} Groceries assertion"
        );
    }
    // …while the child is asserted as its own balance, exactly the spending.
    for (date, month, expected) in [
        (february, "January's", "-700.00"),
        (march, "February's", "-1200.00"),
        (april, "March's", "-1800.00"),
    ] {
        assert_eq!(
            assertion(&bread, "TRY", date),
            Some(decimal(expected, "a pinned figure")),
            "{month} Bread assertion"
        );
    }
    // The parent's asserted figure must not be its own account balance: that
    // difference is the whole point of the pair (rustledger checks the subtree),
    // and it is what the fixture would silently lose if Bread's spending moved
    // back onto its parent.
    assert_ne!(
        assertion(&groceries, "TRY", april),
        Some(balance(&groceries, "TRY")),
        "the fixture must keep a child whose balance differs from its parent's"
    );
    // The pool keeps the real figure each month, both commodities: the unfunded
    // spending was reset into it and the swept excess too.
    for (date, month, try_expected, usd_expected) in [
        (february, "January's", "920.00", "280.00"),
        (march, "February's", "4160.00", "450.00"),
        (april, "March's", "8260.00", "450.00"),
    ] {
        assert_eq!(
            assertion(TO_BE_BUDGETED, "TRY", date),
            Some(decimal(try_expected, "a pinned figure")),
            "{month} pool in TRY"
        );
        assert_eq!(
            assertion(TO_BE_BUDGETED, "USD", date),
            Some(decimal(usd_expected, "a pinned figure")),
            "{month} pool in USD"
        );
    }
}

/// A hand-written entry inside `Equity:Envelope:` is kept by the plugin, and
/// rustledger checks every emitted assertion against the ledger as a whole. An
/// assertion built from generated postings alone would disagree with this
/// transfer and fail `E2001` — and the app, which rolls back an edit that
/// introduces a new error, would refuse to save the entry. It must reach
/// `finalize` clean, with the assertion figures including the transfer.
#[test]
fn a_hand_written_envelope_transfer_keeps_the_assertions_true() {
    let food = category_account("Expenses:Food", "Expenses").expect("Food is a category");
    let directives = vec![
        open("2026-01-01", "Assets:Bank"),
        open("2026-01-01", "Income:Salary"),
        custom("2026-01-01", vec![text("start date"), text("2026-01")]),
        custom("2026-01-01", vec![text("end date"), text("2026-01")]),
        custom(
            "2026-01-01",
            vec![text("budget account"), text("^Assets:Bank")],
        ),
        custom(
            "2026-01-01",
            vec![
                text("fund"),
                account_value("Expenses:Food"),
                amount_value("100.00", "TRY"),
            ],
        ),
        txn(
            "2026-01-05",
            "salary",
            &[
                ("Assets:Bank", "1000.00", "TRY"),
                ("Income:Salary", "-1000.00", "TRY"),
            ],
        ),
        // The transfer the user wrote by hand: money from the pool into an
        // envelope, with none of the plugin's tags or metadata.
        txn(
            "2026-01-20",
            "move budget",
            &[
                (TO_BE_BUDGETED, "-50.00", "TRY"),
                (food.as_str(), "50.00", "TRY"),
            ],
        ),
    ];

    let output = process(PluginInput {
        directives: directives.clone(),
        options: options(),
        config: None,
    });
    assert!(
        output.errors.is_empty(),
        "the plugin reported diagnostics: {:?}",
        output.errors
    );
    validate_op_coverage(directives.len(), &output.ops)
        .expect("the op list is a cover of the fixture");

    // Materialise the op list the way the host does.
    let mut materialised: Vec<Directive> = Vec::with_capacity(output.ops.len());
    for (position, op) in output.ops.iter().enumerate() {
        match op {
            PluginOp::Keep(index) => materialised.push(convert(
                &directives[*index],
                &format!("fixture directive {index}"),
            )),
            PluginOp::Insert(wrapper) => {
                materialised.push(convert(wrapper, &format!("plugin insert {position}")));
            }
            other => panic!("a directive plugin may only keep and insert, got {other:?}"),
        }
    }
    sort_directives(&mut materialised);

    // The loader's order: sort, run early, book/run plugins, run late, finalize.
    let today = parse_date("2026-12-31", "the test's today");
    let (session, early) =
        ValidationSession::new(ValidationOptions::default()).run_early(&materialised, today);
    let (session, late) = session.run_late(&materialised, today);
    let actuals = session.balance_actuals().to_vec();
    let deferred = session.finalize();

    let failures: Vec<String> = early
        .iter()
        .chain(&late)
        .chain(&deferred)
        .filter(|error| error.code.severity() == Severity::Error)
        .map(|error| format!("[{}] {} {}", error.code.code(), error.date, error.message))
        .collect();
    assert!(
        failures.is_empty(),
        "rustledger rejects the ledger around a hand-written envelope transfer (E2001 \
         would roll the edit back):\n{}",
        failures.join("\n")
    );

    let balances = materialised
        .iter()
        .filter(|directive| matches!(directive, Directive::Balance(_)))
        .count();
    assert_eq!(
        actuals.len(),
        balances,
        "every emitted balance assertion must reach the validator's check"
    );
    let checked: Vec<String> = actuals
        .iter()
        .map(|actual| {
            format!(
                "{} {} {} asserted, off by {}",
                actual.date, actual.account, actual.currency, actual.diff
            )
        })
        .collect();
    assert!(
        actuals.iter().all(|actual| !actual.exceeds_tolerance),
        "the plugin's assertions do not match the validator's inventories:\n{}",
        checked.join("\n")
    );

    // 100.00 assigned + 50.00 moved in by hand…
    assert!(
        actuals.iter().any(|actual| {
            actual.account == food
                && actual.currency == "TRY"
                && actual.asserted == decimal("150.00", "the Food assertion")
        }),
        "Food must be asserted as 150.00 TRY, transfer included:\n{}",
        checked.join("\n")
    );
    // …and the pool: 1000.00 earned − 100.00 assigned − 50.00 moved.
    assert!(
        actuals.iter().any(|actual| {
            actual.account == TO_BE_BUDGETED
                && actual.currency == "TRY"
                && actual.asserted == decimal("850.00", "the pool assertion")
        }),
        "the pool must be asserted as 850.00 TRY, transfer included:\n{}",
        checked.join("\n")
    );
}
