// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The writable half of the envelope budget: the plugin's `custom "envelope"`
//! configuration.
//!
//! The budget screen's *figures* are the plugin's output — every load recomputes
//! them and appends the result to the ledger — so writing a figure back would
//! fight the next run of the plugin. What is genuinely the reader's is the
//! configuration that drives it: the `start date`/`budget account` block, each
//! category's `fund` and its `rollover` policy. This module reads those
//! directives for the screen ([`BudgetSettings`]) and writes them by appending
//! or updating one line, through the same loader-verified edit funnel as every
//! other write ([`crate::editing::edit_named_file_source`]).
//!
//! # The rule every date follows
//!
//! The plugin resolves a dated key by taking the newest directive **at or before
//! the month in force** (`plugins/rustledger-envelope/src/model.rs` builds each
//! month's funding with `max_by_key(|(from, _)| from.index())`, and a tie keeps
//! the last directive in the stream). A `fund` therefore applies from its month
//! until a later `fund` for the same account replaces it, and a `rollover` does
//! the same. [`BudgetSettings`] reports the newest directive for an account
//! *overall* — not the one in force at `start` — because the editor's job is to
//! show the latest value it might change; the [`EnvelopeSetting::since`] field
//! says when that value started.
//!
//! # What the editor deliberately does not expose
//!
//! The one-off keys — `adjust`, `zero`, `pad`, `balance`, `map`, `income
//! account`, `include starting balance` — are not writable from the budget
//! screen. They are month-scoped instructions rather than the budget's steady
//! state, and a UI that silently rewrote them would be editing the reader's
//! journal in ways the screen cannot show. They stay hand-written in the
//! ledger.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rustledger_core::{Directive, MetaValue, NaiveDate, SYNTHESIZED_FILE_ID};
use rustledger_loader::Ledger;

use crate::budget::{ENVELOPE_MONEY, ENVELOPE_POOL, is_month_label};
use crate::files::source_of_file;
use crate::prices::checked_commodity;
use crate::rewrite::quote_beancount;
use crate::session::AppState;

/// The prefix every `custom "envelope"` line carries after its date.
const ENVELOPE_PREFIX: &str = "custom \"envelope\"";

/// Where an amount starts, as both sides of the payload need it.
///
/// `units` is an exact decimal string with the scale it was written with
/// (`4000.00` stays `4000.00`), never a float. `currency` is the beancount
/// commodity code.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct Amount {
    /// The exact decimal quantity.
    pub units: String,
    /// The commodity code, e.g. `TRY`.
    pub currency: String,
}

/// What happens to a category's balance at a month boundary — the three shapes
/// the plugin's `rollover` key understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum RolloverKind {
    /// Carry the whole balance into the next month (the plugin's default).
    Carry,
    /// Reset the balance to zero, sweeping it back to `To-Be-Budgeted`.
    Reset,
    /// Carry up to [`RolloverSetting::amount`], sweeping the excess.
    Cap,
}

/// One account's rollover policy, as the screen edits it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct RolloverSetting {
    /// Which boundary policy is in force.
    pub kind: RolloverKind,
    /// The cap, only for [`RolloverKind::Cap`].
    pub amount: Option<Amount>,
}

impl Default for RolloverSetting {
    fn default() -> Self {
        Self {
            kind: RolloverKind::Carry,
            amount: None,
        }
    }
}

/// One category the configuration mentions, and the values the editor shows.
///
/// The account is the *expense* account (`Expenses:Groceries`), not the plugin's
/// mirrored envelope account: that is what the `fund`/`rollover` directives
/// name.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct EnvelopeSetting {
    /// The category, as the directive names it.
    pub account: String,
    /// The newest `fund` amount for the account, or `None` when it was never
    /// funded.
    pub fund: Option<Amount>,
    /// `YYYY-MM` of that newest `fund`, or `None` when the account has none.
    /// This is the month the displayed amount starts applying.
    pub since: Option<String>,
    /// The newest `rollover` policy, defaulting to the plugin's carry.
    pub rollover: RolloverSetting,
    /// The journal file the account's newest `fund`/`rollover` directive lives
    /// in, as [`JournalFile::path`](crate::model::JournalFile::path) spells it.
    pub file: String,
}

/// The budget's configuration as a whole.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct BudgetConfig {
    /// The file the `start date` directive lives in.
    pub file: String,
    /// First budget month, `YYYY-MM`.
    pub start: String,
    /// Last budget month, when one is declared.
    pub end: Option<String>,
    /// Months to budget past the newest ledger directive, when one is declared.
    pub months_ahead: Option<u32>,
    /// The on-budget account patterns, in directive order.
    pub budget_accounts: Vec<String>,
    /// Whether the plugin emits its month-end assertions (default `TRUE`).
    pub assert: bool,
    /// Whether the plugin warns about spending in never-funded categories
    /// (default `FALSE`).
    pub strict: bool,
}

/// Everything the budget screen's editor reads: whether the plugin is declared,
/// the configuration block, and every account it configures.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct BudgetSettings {
    /// The journal declares an envelope plugin (`plugin "…envelope…wasm"`).
    /// The init form warns when this is false; the commands still write.
    pub plugin_declared: bool,
    /// The parsed configuration, or `None` when no `start date` exists.
    pub config: Option<BudgetConfig>,
    /// Every account any `fund`/`rollover` directive names, alphabetically.
    pub envelopes: Vec<EnvelopeSetting>,
}

/// [`BudgetSettings`] for the loaded journal.
pub(crate) fn settings(state: &AppState) -> Result<BudgetSettings, String> {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let loaded = guard.as_ref().ok_or("no ledger loaded")?;
    Ok(settings_of(&loaded.ledger))
}

/// [`settings`] over a ledger already in hand.
pub(crate) fn settings_of(ledger: &Ledger) -> BudgetSettings {
    let mut start: Option<Newest<String>> = None;
    let mut end: Option<Newest<String>> = None;
    let mut months_ahead: Option<Newest<u32>> = None;
    let mut budget_accounts = Vec::new();
    let mut asserted: Option<Newest<bool>> = None;
    let mut strict: Option<Newest<bool>> = None;
    let mut funds: BTreeMap<String, Newest<Assignment>> = BTreeMap::new();
    let mut rollovers: BTreeMap<String, Newest<Policy>> = BTreeMap::new();
    // Where each account's newest directive of any kind lives, so a screen that
    // edits both keys can name one file.
    let mut latest_any: BTreeMap<String, (NaiveDate, usize, String)> = BTreeMap::new();

    for (seq, spanned) in ledger.directives.iter().enumerate() {
        let Directive::Custom(custom) = &spanned.value else {
            continue;
        };
        if custom.custom_type != "envelope" {
            continue;
        }
        let Some(file) = file_of(ledger, spanned.file_id) else {
            continue;
        };
        let Some(key) = custom.values.first().and_then(meta_string) else {
            continue;
        };
        let date = custom.date;
        match key {
            "start date" => {
                if let Some(month) = custom
                    .values
                    .get(1)
                    .and_then(meta_string)
                    .filter(|month| is_month_label(month))
                {
                    remember(&mut start, date, seq, file, month.to_string());
                }
            }
            "end date" => {
                if let Some(month) = custom
                    .values
                    .get(1)
                    .and_then(meta_string)
                    .filter(|month| is_month_label(month))
                {
                    remember(&mut end, date, seq, file, month.to_string());
                }
            }
            "months ahead" => {
                if let Some(count) = custom.values.get(1).and_then(meta_count) {
                    remember(&mut months_ahead, date, seq, file, count);
                }
            }
            "budget account" => {
                if let Some(pattern) = custom.values.get(1).and_then(meta_string) {
                    budget_accounts.push(pattern.to_string());
                }
            }
            "assert" => {
                if let Some(flag) = custom.values.get(1).and_then(meta_bool) {
                    remember(&mut asserted, date, seq, file, flag);
                }
            }
            "strict" => {
                if let Some(flag) = custom.values.get(1).and_then(meta_bool) {
                    remember(&mut strict, date, seq, file, flag);
                }
            }
            "fund" => {
                let Some(account) = custom.values.get(1).and_then(meta_account) else {
                    continue;
                };
                let Some(amount) = custom.values.get(2).and_then(meta_amount) else {
                    continue;
                };
                let account = account.to_string();
                update_latest(&mut latest_any, &account, date, seq, &file);
                let entry = Newest {
                    date,
                    seq,
                    file,
                    value: amount,
                };
                match funds.get_mut(&account) {
                    Some(current) if entry.beats(current) => *current = entry,
                    Some(_) => {}
                    None => {
                        funds.insert(account, entry);
                    }
                }
            }
            "rollover" => {
                let Some(account) = custom.values.get(1).and_then(meta_account) else {
                    continue;
                };
                let Some(policy) = custom.values.get(2).and_then(meta_policy) else {
                    continue;
                };
                let account = account.to_string();
                update_latest(&mut latest_any, &account, date, seq, &file);
                let entry = Newest {
                    date,
                    seq,
                    file,
                    value: policy,
                };
                match rollovers.get_mut(&account) {
                    Some(current) if entry.beats(current) => *current = entry,
                    Some(_) => {}
                    None => {
                        rollovers.insert(account, entry);
                    }
                }
            }
            _ => {}
        }
    }

    let accounts: BTreeSet<&String> = funds.keys().chain(rollovers.keys()).collect();
    let envelopes = accounts
        .into_iter()
        .map(|account| {
            let fund = funds.get(account);
            EnvelopeSetting {
                account: account.clone(),
                fund: fund.map(|entry| entry.value.to_amount()),
                since: fund.map(|entry| month_label(entry.date)),
                rollover: rollovers
                    .get(account)
                    .map(|entry| entry.value.to_setting())
                    .unwrap_or_default(),
                file: latest_any
                    .get(account)
                    .map(|(_, _, file)| file.clone())
                    .unwrap_or_default(),
            }
        })
        .collect();

    let config = start.map(|start| BudgetConfig {
        file: start.file,
        start: start.value,
        end: end.map(|entry| entry.value),
        months_ahead: months_ahead.map(|entry| entry.value),
        budget_accounts,
        assert: asserted.is_none_or(|entry| entry.value),
        strict: strict.is_some_and(|entry| entry.value),
    });

    BudgetSettings {
        plugin_declared: ledger.plugins.iter().any(|plugin| {
            let file = plugin
                .name
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&plugin.name);
            file.to_ascii_lowercase().contains("envelope")
        }),
        config,
        envelopes,
    }
}

/// The loaded path of the file a directive came from, in the spelling
/// [`JournalFile::path`](crate::model::JournalFile::path) uses.
fn file_of(ledger: &Ledger, file_id: u16) -> Option<String> {
    if file_id == SYNTHESIZED_FILE_ID {
        return None;
    }
    ledger
        .source_map
        .get(usize::from(file_id))
        .map(|file| file.path.display().to_string())
}

/// The newest directive for one config key.
#[derive(Clone)]
struct Newest<T> {
    date: NaiveDate,
    seq: usize,
    file: String,
    value: T,
}

impl<T> Newest<T> {
    /// The plugin's tie rule: a later date wins, and on one date the later
    /// directive in the stream does (`max_by_key` keeps the last maximum).
    fn beats(&self, current: &Self) -> bool {
        (self.date, self.seq) > (current.date, current.seq)
    }
}

fn remember<T>(slot: &mut Option<Newest<T>>, date: NaiveDate, seq: usize, file: String, value: T) {
    let candidate = Newest {
        date,
        seq,
        file,
        value,
    };
    if slot.as_ref().is_none_or(|current| candidate.beats(current)) {
        *slot = Some(candidate);
    }
}

fn update_latest(
    latest: &mut BTreeMap<String, (NaiveDate, usize, String)>,
    account: &str,
    date: NaiveDate,
    seq: usize,
    file: &str,
) {
    let candidate = (date, seq, file.to_string());
    latest
        .entry(account.to_string())
        .and_modify(|current| {
            if (candidate.0, candidate.1) > (current.0, current.1) {
                *current = candidate.clone();
            }
        })
        .or_insert(candidate);
}

/// An amount as the plugin's directive carries it: a decimal and a commodity
/// code, never a rendered string.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Assignment {
    number: Decimal,
    currency: String,
}

impl Assignment {
    fn to_amount(&self) -> Amount {
        Amount {
            units: self.number.to_string(),
            currency: self.currency.clone(),
        }
    }
}

/// A rollover as the plugin's directive carries it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Policy {
    Carry,
    Reset,
    Cap(Assignment),
}

impl Policy {
    fn to_setting(&self) -> RolloverSetting {
        match self {
            Self::Carry => RolloverSetting::default(),
            Self::Reset => RolloverSetting {
                kind: RolloverKind::Reset,
                amount: None,
            },
            Self::Cap(amount) => RolloverSetting {
                kind: RolloverKind::Cap,
                amount: Some(amount.to_amount()),
            },
        }
    }
}

fn meta_string(value: &MetaValue) -> Option<&str> {
    match value {
        MetaValue::String(text) => Some(text),
        _ => None,
    }
}

fn meta_account(value: &MetaValue) -> Option<&str> {
    match value {
        MetaValue::Account(account) => Some(account.as_str()),
        _ => None,
    }
}

fn meta_bool(value: &MetaValue) -> Option<bool> {
    match value {
        MetaValue::Bool(flag) => Some(*flag),
        _ => None,
    }
}

/// A whole non-negative number, in the two shapes the lexer may give it
/// (`2` arrives as an `Int` in this rustledger, `2.0` as a `Number`).
fn meta_count(value: &MetaValue) -> Option<u32> {
    let number = match value {
        MetaValue::Number(number) => *number,
        MetaValue::Int(number) => Decimal::from(*number),
        _ => return None,
    };
    if number.is_sign_negative() || !number.fract().is_zero() {
        return None;
    }
    number.to_u64().and_then(|count| u32::try_from(count).ok())
}

fn meta_amount(value: &MetaValue) -> Option<Assignment> {
    match value {
        MetaValue::Amount(amount) => Some(Assignment {
            number: amount.number,
            currency: amount.currency.to_string(),
        }),
        _ => None,
    }
}

fn meta_policy(value: &MetaValue) -> Option<Policy> {
    match value {
        MetaValue::Bool(true) => Some(Policy::Carry),
        MetaValue::Bool(false) => Some(Policy::Reset),
        _ => meta_amount(value).map(Policy::Cap),
    }
}

/// A directive's date as `YYYY-MM`.
fn month_label(date: NaiveDate) -> String {
    let text = date.to_string();
    text.get(..7).map(str::to_string).unwrap_or(text)
}

/// The first day of a validated `YYYY-MM` label.
fn month_start(month: &str) -> Result<NaiveDate, String> {
    format!("{month}-01")
        .parse()
        .map_err(|_| format!("month `{month}` is not a calendar month"))
}

/// Append the budget's configuration block to `file`.
///
/// A second `start date` is refused rather than added: the plugin reads the
/// newest one, so two would make the older directive silently dead. The error
/// names the file and line that already holds it.
///
/// The command does not require the envelope plugin to be declared — a journal
/// that has the directives but not the `plugin` line is a configuration the
/// reader may be about to complete, and [`BudgetSettings::plugin_declared`] is
/// the form's warning.
pub(crate) fn init(
    state: &AppState,
    file: &str,
    start: &str,
    months_ahead: Option<u32>,
    end: Option<&str>,
    budget_accounts: &[String],
    assert: bool,
    strict: bool,
) -> Result<(), String> {
    let start = checked_month("start", start)?;
    let end = end.map(|end| checked_month("end", end)).transpose()?;
    if budget_accounts.is_empty() {
        return Err("budget_accounts is empty; an envelope budget needs at least one on-budget account pattern".to_string());
    }

    let (previous, rewritten) = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().ok_or("no ledger loaded")?;
        if let Some((found, line)) = existing_start_date(&loaded.ledger) {
            return Err(format!(
                "{found} already declares a budget `start date` at line {line}; edit that directive instead of adding a second"
            ));
        }
        let source = source_of_file(&loaded.ledger, file)?;
        let previous = source.source.to_string();

        let date = month_start(&start)?;
        let date = date.to_string();
        let mut lines = vec![directive_line(
            &date,
            "start date",
            &[quote_beancount(&start)],
        )];
        if let Some(end) = &end {
            lines.push(directive_line(&date, "end date", &[quote_beancount(end)]));
        }
        if let Some(count) = months_ahead {
            lines.push(directive_line(&date, "months ahead", &[count.to_string()]));
        }
        for pattern in budget_accounts {
            lines.push(directive_line(
                &date,
                "budget account",
                &[quote_beancount(pattern)],
            ));
        }
        // Both flags are only written when they differ from the plugin's own
        // defaults, so the block says what changes and nothing else.
        if !assert {
            lines.push(directive_line(&date, "assert", &["FALSE".to_string()]));
        }
        if strict {
            lines.push(directive_line(&date, "strict", &["TRUE".to_string()]));
        }

        let rewritten = insert_block(&previous, &lines);
        (previous, rewritten)
    };

    if rewritten == previous {
        return Ok(());
    }
    crate::editing::edit_named_file_source(state, file, &rewritten, None)
}

/// The file and 1-based line of the journal's first `start date`, if any.
fn existing_start_date(ledger: &Ledger) -> Option<(String, usize)> {
    for spanned in &ledger.directives {
        let Directive::Custom(custom) = &spanned.value else {
            continue;
        };
        if custom.custom_type != "envelope"
            || custom.values.first().and_then(meta_string) != Some("start date")
        {
            continue;
        }
        let Some(file) = ledger.source_map.get(usize::from(spanned.file_id)) else {
            continue;
        };
        let line = file.line_col(spanned.span.start).0;
        return Some((file.path.display().to_string(), line));
    }
    None
}

/// Make `units currency` the amount assigned to `account` from `month` on.
///
/// The plugin inherits a `fund` until a later one replaces it, so the command
/// is an upsert: a directive already dated `month-01` for this account is
/// rewritten in place, anything older is left alone, and an unchanged value
/// writes nothing at all.
pub(crate) fn fund_set(
    state: &AppState,
    file: &str,
    account: &str,
    units: &str,
    currency: &str,
    month: &str,
) -> Result<(), String> {
    let month = checked_month("month", month)?;
    let account = checked_account(account)?.to_string();
    let number = checked_units(units)?;
    let currency = checked_currency(currency)?;
    let requested = Assignment {
        number,
        currency: currency.clone(),
    };

    let rewritten = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().ok_or("no ledger loaded")?;
        refuse_plumbing(&loaded.ledger, &account)?;
        let date = month_start(&month)?;
        if newest_fund(&loaded.ledger, &account)
            .is_some_and(|entry| entry.0 <= date && entry.1 == requested)
        {
            return Ok(());
        }
        let source = source_of_file(&loaded.ledger, file)?;
        let previous = source.source.to_string();
        let line = directive_line(
            &date.to_string(),
            "fund",
            &[
                account.clone(),
                requested.number.to_string(),
                requested.currency.clone(),
            ],
        );
        match same_date_directive(&loaded.ledger, source.id, "fund", &account, date) {
            Some(start) => replace_line(&previous, start, &line),
            None => insert_block(&previous, &[line]),
        }
    };

    crate::editing::edit_named_file_source(state, file, &rewritten, None)
}

/// Set `account`'s rollover policy from `month` on: `carry`, `reset`, or `cap`
/// (which requires `units` and `currency`).
///
/// The dating rule and the upsert are [`fund_set`]'s: an existing directive at
/// exactly `month-01` is rewritten, an unchanged policy writes nothing.
pub(crate) fn rollover_set(
    state: &AppState,
    file: &str,
    account: &str,
    kind: &str,
    units: Option<&str>,
    currency: Option<&str>,
    month: &str,
) -> Result<(), String> {
    let month = checked_month("month", month)?;
    let account = checked_account(account)?.to_string();
    let requested = checked_policy(kind, units, currency)?;

    let rewritten = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().ok_or("no ledger loaded")?;
        refuse_plumbing(&loaded.ledger, &account)?;
        let date = month_start(&month)?;
        if newest_rollover(&loaded.ledger, &account)
            .is_some_and(|entry| entry.0 <= date && entry.1 == requested)
        {
            return Ok(());
        }
        let source = source_of_file(&loaded.ledger, file)?;
        let previous = source.source.to_string();
        let line = directive_line(
            &date.to_string(),
            "rollover",
            &[account.clone(), policy_token(&requested)],
        );
        match same_date_directive(&loaded.ledger, source.id, "rollover", &account, date) {
            Some(start) => replace_line(&previous, start, &line),
            None => insert_block(&previous, &[line]),
        }
    };

    crate::editing::edit_named_file_source(state, file, &rewritten, None)
}

/// The `rollover` policy the caller asked for, with the argument rules the
/// plugin applies: an amount needs both `units` and `currency`, a bare policy
/// takes neither.
fn checked_policy(
    kind: &str,
    units: Option<&str>,
    currency: Option<&str>,
) -> Result<Policy, String> {
    let normalized = kind.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "carry" | "reset" => {
            if units.is_some() || currency.is_some() {
                return Err(format!(
                    "kind `{normalized}` takes no amount; omit units and currency"
                ));
            }
            Ok(if normalized == "carry" {
                Policy::Carry
            } else {
                Policy::Reset
            })
        }
        "cap" => {
            let units = units.ok_or("kind `cap` needs units and currency")?;
            let currency = currency.ok_or("kind `cap` needs units and currency")?;
            Ok(Policy::Cap(Assignment {
                number: checked_units(units)?,
                currency: checked_currency(currency)?,
            }))
        }
        _ => Err(format!(
            "kind `{kind}` is not one of `carry`, `reset` or `cap`"
        )),
    }
}

/// The rendered second argument of a `rollover` directive.
fn policy_token(policy: &Policy) -> String {
    match policy {
        Policy::Carry => "TRUE".to_string(),
        Policy::Reset => "FALSE".to_string(),
        Policy::Cap(amount) => format!("{} {}", amount.number, amount.currency),
    }
}

/// A validated `YYYY-MM` month label, trimmed.
fn checked_month(argument: &str, value: &str) -> Result<String, String> {
    let month = value.trim();
    if !is_month_label(month) {
        return Err(format!("{argument} `{value}` is not a YYYY-MM month"));
    }
    Ok(month.to_string())
}

/// An account path that cannot inject syntax of its own. Whether it is a real
/// expense account is the plugin's decision; this only keeps the line a line.
fn checked_account(account: &str) -> Result<&str, String> {
    let account = account.trim();
    if account.is_empty() {
        return Err("account is empty".to_string());
    }
    if account
        .chars()
        .any(|character| character.is_whitespace() || matches!(character, '"' | ';'))
    {
        return Err(format!("account `{account}` is not an account name"));
    }
    Ok(account)
}

/// The caller's decimal, kept at the scale it was written with.
fn checked_units(units: &str) -> Result<Decimal, String> {
    Decimal::from_str(units.trim())
        .map_err(|error| format!("units `{units}` is not a decimal amount: {error}"))
}

/// A commodity through the same rule prices are written with.
fn checked_currency(currency: &str) -> Result<String, String> {
    checked_commodity("currency", currency)
}

/// Refuse the plugin's own bookkeeping accounts, using the plugin's reasoning:
/// those subtrees are asserted as a whole, so a category inside them could
/// never keep the assertion true.
fn refuse_plumbing(ledger: &Ledger, account: &str) -> Result<(), String> {
    if is_plumbing(account) {
        return Err(format!(
            "{account:?} is the plugin's own bookkeeping, not a category; no category may live under {ENVELOPE_POOL} or {ENVELOPE_MONEY}"
        ));
    }
    let root = ledger.options.to_account_types().expenses;
    if let Some(category) = category_of(account, &root)
        && is_plumbing(&category)
    {
        return Err(format!(
            "{account:?} resolves to {category:?}, which is the plugin's own bookkeeping, not a category; no category may live under {ENVELOPE_POOL} or {ENVELOPE_MONEY}"
        ));
    }
    Ok(())
}

/// `account` itself or a descendant of `node` (component boundaries matter).
fn is_under(account: &str, node: &str) -> bool {
    account == node
        || (account.len() > node.len()
            && account.starts_with(node)
            && account.as_bytes()[node.len()] == b':')
}

/// The plugin's mirroring rule (`plugins/rustledger-envelope/src/accounts.rs`):
/// an expense posting account becomes `Equity:Envelope:<tail>`.
fn category_of(account: &str, expense_root: &str) -> Option<String> {
    if !is_under(account, expense_root) {
        return None;
    }
    let tail = account.strip_prefix(expense_root)?.trim_start_matches(':');
    if tail.is_empty() {
        Some("Equity:Envelope:Uncategorized".to_string())
    } else {
        Some(format!("Equity:Envelope:{tail}"))
    }
}

fn is_plumbing(category: &str) -> bool {
    is_under(category, ENVELOPE_POOL)
        || is_under(category, ENVELOPE_MONEY)
        || is_under(ENVELOPE_POOL, category)
        || is_under(ENVELOPE_MONEY, category)
}

/// The newest `fund` for `account` overall: its date and amount.
fn newest_fund(ledger: &Ledger, account: &str) -> Option<(NaiveDate, Assignment)> {
    let mut newest: Option<(NaiveDate, usize, Assignment)> = None;
    for (seq, spanned) in ledger.directives.iter().enumerate() {
        let Directive::Custom(custom) = &spanned.value else {
            continue;
        };
        if custom.custom_type != "envelope"
            || custom.values.first().and_then(meta_string) != Some("fund")
            || custom.values.get(1).and_then(meta_account) != Some(account)
        {
            continue;
        }
        let Some(amount) = custom.values.get(2).and_then(meta_amount) else {
            continue;
        };
        let candidate = (custom.date, seq, amount);
        if newest
            .as_ref()
            .is_none_or(|current| (candidate.0, candidate.1) > (current.0, current.1))
        {
            newest = Some(candidate);
        }
    }
    newest.map(|(date, _, amount)| (date, amount))
}

/// The newest `rollover` for `account` overall: its date and policy.
fn newest_rollover(ledger: &Ledger, account: &str) -> Option<(NaiveDate, Policy)> {
    let mut newest: Option<(NaiveDate, usize, Policy)> = None;
    for (seq, spanned) in ledger.directives.iter().enumerate() {
        let Directive::Custom(custom) = &spanned.value else {
            continue;
        };
        if custom.custom_type != "envelope"
            || custom.values.first().and_then(meta_string) != Some("rollover")
            || custom.values.get(1).and_then(meta_account) != Some(account)
        {
            continue;
        }
        let Some(policy) = custom.values.get(2).and_then(meta_policy) else {
            continue;
        };
        let candidate = (custom.date, seq, policy);
        if newest
            .as_ref()
            .is_none_or(|current| (candidate.0, candidate.1) > (current.0, current.1))
        {
            newest = Some(candidate);
        }
    }
    newest.map(|(date, _, policy)| (date, policy))
}

/// The byte offset of the last `key` directive for `account` dated exactly
/// `date` in file `file_id`, so it can be rewritten instead of duplicated.
fn same_date_directive(
    ledger: &Ledger,
    file_id: usize,
    key: &str,
    account: &str,
    date: NaiveDate,
) -> Option<usize> {
    let mut found = None;
    for spanned in &ledger.directives {
        if usize::from(spanned.file_id) != file_id {
            continue;
        }
        let Directive::Custom(custom) = &spanned.value else {
            continue;
        };
        if custom.custom_type != "envelope"
            || custom.date != date
            || custom.values.first().and_then(meta_string) != Some(key)
            || custom.values.get(1).and_then(meta_account) != Some(account)
        {
            continue;
        }
        found = Some(spanned.span.start);
    }
    found
}

/// One `custom "envelope"` line.
fn directive_line(date: &str, key: &str, arguments: &[String]) -> String {
    let mut line = format!("{date} {ENVELOPE_PREFIX} \"{key}\"");
    for argument in arguments {
        line.push(' ');
        line.push_str(argument);
    }
    line
}

/// Replace the whole line that starts at `offset`, keeping its line ending.
fn replace_line(text: &str, offset: usize, line: &str) -> String {
    let start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    let rest = &text[start..];
    let content_end = match rest.find('\n') {
        Some(index) if index > 0 && rest.as_bytes()[index - 1] == b'\r' => start + index - 1,
        Some(index) => start + index,
        None => text.len(),
    };
    let mut rewritten = String::with_capacity(text.len() + line.len());
    rewritten.push_str(&text[..start]);
    rewritten.push_str(line);
    rewritten.push_str(&text[content_end..]);
    rewritten
}

/// Insert `lines` inside the file's envelope group, else after its last
/// `plugin`/`option` line, else at the top of the file.
///
/// The block is separated from what follows by one blank line; the file's line
/// endings and its trailing-newline presence are preserved, and every other
/// byte is left alone.
fn insert_block(text: &str, lines: &[String]) -> String {
    let eol = line_ending(text);
    let offset = insertion_offset(text);
    let prefix = &text[..offset];
    let suffix = &text[offset..];
    let mut block = lines.join(eol);

    let mut rewritten = String::with_capacity(text.len() + block.len() + 2 * eol.len() + 2);
    rewritten.push_str(prefix);
    if !prefix.is_empty() && !prefix.ends_with('\n') {
        rewritten.push_str(eol);
    }
    if suffix.is_empty() {
        // Nothing follows: keep the file's own trailing-newline state rather
        // than inventing a blank line at the end of it.
        if text.is_empty() || text.ends_with('\n') {
            block.push_str(eol);
        }
        rewritten.push_str(&block);
    } else {
        block.push_str(eol);
        rewritten.push_str(&block);
        if !suffix.starts_with('\n') && !suffix.starts_with("\r\n") {
            rewritten.push_str(eol);
        }
        rewritten.push_str(suffix);
    }
    rewritten
}

/// The line ending the file already uses (`\r\n` when its first newline is a
/// CRLF, else `\n`).
fn line_ending(text: &str) -> &'static str {
    match text.find('\n') {
        Some(index) if index > 0 && text.as_bytes()[index - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

/// Where a new block goes, as a byte offset at a line boundary.
fn insertion_offset(text: &str) -> usize {
    let lines = line_spans(text);

    // The file's envelope group: after the last directive and any metadata
    // lines that belong to it, so a new directive never splits one.
    if let Some(index) = lines
        .iter()
        .rposition(|(start, end)| is_envelope_line(line_text(text, *start, *end)))
    {
        let mut offset = lines[index].1;
        for &(start, end) in &lines[index + 1..] {
            let line = line_text(text, start, end);
            if line.starts_with([' ', '\t']) && !line.trim().is_empty() {
                offset = end;
            } else {
                break;
            }
        }
        return offset;
    }

    // Otherwise beneath the journal's preamble, which is where a `plugin` or
    // `option` line is expected to be.
    lines
        .iter()
        .rposition(|(start, end)| {
            let line = line_text(text, *start, *end).trim_start();
            starts_keyword(line, "plugin") || starts_keyword(line, "option")
        })
        .map_or(0, |index| lines[index].1)
}

/// The file's lines, each as `(start, end)` with the newline included.
fn line_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            spans.push((start, index + 1));
            start = index + 1;
        }
    }
    if start < text.len() {
        spans.push((start, text.len()));
    }
    spans
}

fn line_text(text: &str, start: usize, end: usize) -> &str {
    &text[start..end]
}

/// Whether a line is a `custom "envelope"` directive: a date token, then the
/// key. The date is what keeps a comment or a body line from reading as one.
fn is_envelope_line(line: &str) -> bool {
    let Some((date, rest)) = line.trim_start().split_once(char::is_whitespace) else {
        return false;
    };
    let dated = date.len() == 10
        && date.bytes().enumerate().all(|(index, byte)| match index {
            4 | 7 => byte == b'-',
            _ => byte.is_ascii_digit(),
        });
    if !dated {
        return false;
    }
    let Some(after) = rest.trim_start().strip_prefix(ENVELOPE_PREFIX) else {
        return false;
    };
    after.is_empty() || after.starts_with(char::is_whitespace)
}

/// Whether a line starts with `keyword` as a directive, not as a longer word.
fn starts_keyword(line: &str, keyword: &str) -> bool {
    line.strip_prefix(keyword)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget_write;
    use crate::testing::{TempDir, loaded_state};
    use std::fs;

    const LEDGER: &str = "\
option \"operating_currency\" \"TRY\"

2026-01-01 open Assets:Bank
2026-01-01 open Expenses:Food
";

    const CONFIGURED: &str = "\
option \"operating_currency\" \"TRY\"

2026-01-01 custom \"envelope\" \"start date\" \"2026-01\"
2026-01-01 custom \"envelope\" \"budget account\" \"Assets:Bank\"

2026-01-01 open Assets:Bank
2026-01-01 open Expenses:Food
";

    fn fixture(name: &str, ledger: &str) -> (TempDir, std::path::PathBuf, AppState) {
        let dir = TempDir::new(name);
        let path = dir.seed("main.bean", ledger);
        let state = loaded_state(&path);
        (dir, path, state)
    }

    fn journal(path: &std::path::Path) -> String {
        path.display().to_string()
    }

    fn read(path: &std::path::Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    fn occurrences(text: &str, needle: &str) -> usize {
        text.matches(needle).count()
    }

    /// The one envelope the tests care about, by account.
    fn envelope<'a>(settings: &'a BudgetSettings, account: &str) -> &'a EnvelopeSetting {
        settings
            .envelopes
            .iter()
            .find(|envelope| envelope.account == account)
            .unwrap_or_else(|| panic!("no envelope for {account}: {:?}", settings.envelopes))
    }

    #[test]
    fn init_writes_a_parseable_block_and_refuses_a_second_start_date() {
        let (_dir, path, state) = fixture("budget-init", LEDGER);
        let file = journal(&path);
        let before = read(&path);

        budget_write::init(
            &state,
            &file,
            "2026-02",
            Some(2),
            Some("2026-12"),
            &["Assets:Bank".to_string()],
            false,
            true,
        )
        .unwrap();

        let text = read(&path);
        // The block follows the option line, and one blank line separates it
        // from the open directives that follow.
        let block = "\
2026-02-01 custom \"envelope\" \"start date\" \"2026-02\"
2026-02-01 custom \"envelope\" \"end date\" \"2026-12\"
2026-02-01 custom \"envelope\" \"months ahead\" 2
2026-02-01 custom \"envelope\" \"budget account\" \"Assets:Bank\"
2026-02-01 custom \"envelope\" \"assert\" FALSE
2026-02-01 custom \"envelope\" \"strict\" TRUE
";
        // The block goes straight after the option line ("immediately after"
        // the last preamble line), and the blank line it replaced now follows
        // the block, so what follows is still one blank line away.
        let expected = format!(
            "option \"operating_currency\" \"TRY\"\n{block}\n2026-01-01 open Assets:Bank\n2026-01-01 open Expenses:Food\n"
        );
        assert_eq!(text, expected);
        // Every byte before and after the insertion survives.
        let prefix = "option \"operating_currency\" \"TRY\"\n";
        let suffix = "\n2026-01-01 open Assets:Bank\n2026-01-01 open Expenses:Food\n";
        assert!(text.starts_with(prefix));
        assert!(text.ends_with(suffix));
        assert!(text.len() > before.len());

        let settings = settings_of(&state.loaded.read().unwrap().as_ref().unwrap().ledger);
        let config = settings.config.expect("the written config parses");
        assert_eq!(config.start, "2026-02");
        assert_eq!(config.end.as_deref(), Some("2026-12"));
        assert_eq!(config.months_ahead, Some(2));
        assert_eq!(config.budget_accounts, ["Assets:Bank"]);
        assert!(!config.assert);
        assert!(config.strict);
        assert_eq!(config.file, file);
        assert!(!settings.plugin_declared);

        let second = budget_write::init(
            &state,
            &file,
            "2026-03",
            None,
            None,
            &["Assets:Bank".to_string()],
            true,
            false,
        )
        .unwrap_err();
        assert!(second.contains("start date"), "{second}");
        assert!(second.contains(&file), "{second}");
        assert_eq!(read(&path), text, "a refused init writes nothing");
    }

    #[test]
    fn fund_inserts_replaces_at_one_date_and_is_a_no_op_when_unchanged() {
        let (_dir, path, state) = fixture("budget-fund", CONFIGURED);
        let file = journal(&path);

        budget_write::fund_set(&state, &file, "Expenses:Food", "400.00", "TRY", "2026-02").unwrap();
        let after_insert = read(&path);
        assert!(
            after_insert
                .contains("2026-02-01 custom \"envelope\" \"fund\" Expenses:Food 400.00 TRY")
        );
        assert_eq!(
            occurrences(&after_insert, "custom \"envelope\" \"fund\""),
            1
        );

        let settings = settings_of(&state.loaded.read().unwrap().as_ref().unwrap().ledger);
        let food = envelope(&settings, "Expenses:Food");
        assert_eq!(
            food.fund,
            Some(Amount {
                units: "400.00".to_string(),
                currency: "TRY".to_string()
            })
        );
        assert_eq!(food.since.as_deref(), Some("2026-02"));
        assert_eq!(food.rollover, RolloverSetting::default());
        assert_eq!(food.file, file);

        // An unchanged amount (even spelled at another scale) writes nothing.
        budget_write::fund_set(&state, &file, "Expenses:Food", "400", "TRY", "2026-02").unwrap();
        assert_eq!(read(&path), after_insert);

        // A different amount at the same month rewrites that one line.
        budget_write::fund_set(&state, &file, "Expenses:Food", "500.00", "TRY", "2026-02").unwrap();
        let after_replace = read(&path);
        assert_eq!(
            occurrences(&after_replace, "custom \"envelope\" \"fund\""),
            1
        );
        assert!(
            after_replace
                .contains("2026-02-01 custom \"envelope\" \"fund\" Expenses:Food 500.00 TRY")
        );
        assert!(!after_replace.contains("400.00 TRY"));

        // A later month adds a line and becomes the newest value shown.
        budget_write::fund_set(&state, &file, "Expenses:Food", "600.00", "TRY", "2026-03").unwrap();
        let after_later = read(&path);
        assert_eq!(occurrences(&after_later, "custom \"envelope\" \"fund\""), 2);
        let settings = settings_of(&state.loaded.read().unwrap().as_ref().unwrap().ledger);
        let food = envelope(&settings, "Expenses:Food");
        assert_eq!(food.fund.as_ref().unwrap().units, "600.00");
        assert_eq!(food.since.as_deref(), Some("2026-03"));
    }

    #[test]
    fn rollover_carry_reset_and_cap_round_trip() {
        let (_dir, path, state) = fixture("budget-rollover", CONFIGURED);
        let file = journal(&path);

        budget_write::rollover_set(
            &state,
            &file,
            "Expenses:Food",
            "reset",
            None,
            None,
            "2026-02",
        )
        .unwrap();
        let settings = settings_of(&state.loaded.read().unwrap().as_ref().unwrap().ledger);
        assert_eq!(
            envelope(&settings, "Expenses:Food").rollover,
            RolloverSetting {
                kind: RolloverKind::Reset,
                amount: None
            }
        );

        budget_write::rollover_set(
            &state,
            &file,
            "Expenses:Food",
            "cap",
            Some("500.00"),
            Some("TRY"),
            "2026-02",
        )
        .unwrap();
        let after_cap = read(&path);
        assert!(
            after_cap
                .contains("2026-02-01 custom \"envelope\" \"rollover\" Expenses:Food 500.00 TRY")
        );
        let settings = settings_of(&state.loaded.read().unwrap().as_ref().unwrap().ledger);
        assert_eq!(
            envelope(&settings, "Expenses:Food").rollover,
            RolloverSetting {
                kind: RolloverKind::Cap,
                amount: Some(Amount {
                    units: "500.00".to_string(),
                    currency: "TRY".to_string()
                })
            }
        );

        // Same cap: no write.
        budget_write::rollover_set(
            &state,
            &file,
            "Expenses:Food",
            "cap",
            Some("500.00"),
            Some("TRY"),
            "2026-02",
        )
        .unwrap();
        assert_eq!(read(&path), after_cap);

        budget_write::rollover_set(
            &state,
            &file,
            "Expenses:Food",
            "carry",
            None,
            None,
            "2026-02",
        )
        .unwrap();
        let settings = settings_of(&state.loaded.read().unwrap().as_ref().unwrap().ledger);
        assert_eq!(
            envelope(&settings, "Expenses:Food").rollover,
            RolloverSetting::default()
        );
        assert_eq!(
            occurrences(&read(&path), "custom \"envelope\" \"rollover\""),
            1
        );
    }

    #[test]
    fn a_plumbing_account_is_refused_with_the_plugins_reasoning() {
        let (_dir, path, state) = fixture("budget-plumbing", CONFIGURED);
        let file = journal(&path);
        let before = read(&path);

        let error = budget_write::fund_set(
            &state,
            &file,
            "Equity:Envelope:To-Be-Budgeted",
            "100.00",
            "TRY",
            "2026-02",
        )
        .unwrap_err();
        assert!(error.contains("bookkeeping"), "{error}");
        assert!(error.contains("To-Be-Budgeted"), "{error}");

        // The plugin's own mirroring rule catches the same account written as
        // an expense category.
        let error =
            budget_write::fund_set(&state, &file, "Expenses:Money", "100.00", "TRY", "2026-02")
                .unwrap_err();
        assert!(error.contains("Equity:Envelope:Money"), "{error}");

        let error = budget_write::rollover_set(
            &state,
            &file,
            "Equity:Envelope:Money",
            "carry",
            None,
            None,
            "2026-02",
        )
        .unwrap_err();
        assert!(error.contains("bookkeeping"), "{error}");
        assert_eq!(read(&path), before);
    }

    #[test]
    fn malformed_month_amount_and_currency_name_their_argument() {
        let (_dir, path, state) = fixture("budget-bad-args", CONFIGURED);
        let file = journal(&path);
        let before = read(&path);

        let error =
            budget_write::fund_set(&state, &file, "Expenses:Food", "1.00", "TRY", "2026-13")
                .unwrap_err();
        assert!(
            error.contains("month") && error.contains("2026-13"),
            "{error}"
        );

        let error =
            budget_write::fund_set(&state, &file, "Expenses:Food", "four", "TRY", "2026-02")
                .unwrap_err();
        assert!(error.contains("units") && error.contains("four"), "{error}");

        let error =
            budget_write::fund_set(&state, &file, "Expenses:Food", "1.00", "1TRY", "2026-02")
                .unwrap_err();
        assert!(
            error.contains("currency") && error.contains("1TRY"),
            "{error}"
        );

        let error = budget_write::rollover_set(
            &state,
            &file,
            "Expenses:Food",
            "cap",
            None,
            None,
            "2026-02",
        )
        .unwrap_err();
        assert!(error.contains("cap") && error.contains("units"), "{error}");

        let error = budget_write::rollover_set(
            &state,
            &file,
            "Expenses:Food",
            "carry",
            Some("1.00"),
            Some("TRY"),
            "2026-02",
        )
        .unwrap_err();
        assert!(error.contains("carry"), "{error}");

        assert_eq!(read(&path), before);
    }

    #[test]
    fn crlf_files_keep_crlf_and_their_trailing_newline() {
        let ledger = "option \"operating_currency\" \"TRY\"\r\n\r\n2026-01-01 open Assets:Bank\r\n\
                      2026-01-01 open Expenses:Food\r\n";
        let (_dir, path, state) = fixture("budget-crlf", ledger);
        let file = journal(&path);

        budget_write::init(
            &state,
            &file,
            "2026-02",
            None,
            None,
            &["Assets:Bank".to_string()],
            true,
            false,
        )
        .unwrap();
        budget_write::fund_set(&state, &file, "Expenses:Food", "100.00", "TRY", "2026-02").unwrap();

        let text = read(&path);
        let bytes = text.as_bytes();
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' {
                assert!(
                    index > 0 && bytes[index - 1] == b'\r',
                    "lone LF at byte {index} in {text:?}"
                );
            }
        }
        assert!(text.ends_with("\r\n"));
        assert!(text.contains("2026-02-01 custom \"envelope\" \"start date\" \"2026-02\"\r\n"));
        assert!(
            text.contains("2026-02-01 custom \"envelope\" \"fund\" Expenses:Food 100.00 TRY\r\n")
        );
    }

    #[test]
    fn plugin_declared_follows_the_plugin_directive_name() {
        let ledger = "\
plugin \"rustledger_envelope.wasm\"

option \"operating_currency\" \"TRY\"

2026-01-01 open Assets:Bank
";
        let (_dir, _path, state) = fixture("budget-plugin", ledger);
        let settings = settings_of(&state.loaded.read().unwrap().as_ref().unwrap().ledger);
        assert!(settings.plugin_declared);
        assert!(settings.config.is_none());

        let (_dir, _path, state) = fixture("budget-no-plugin", LEDGER);
        let settings = settings_of(&state.loaded.read().unwrap().as_ref().unwrap().ledger);
        assert!(!settings.plugin_declared);
    }

    #[test]
    fn the_newest_directive_overall_is_reported_with_its_since_month() {
        let ledger = "\
option \"operating_currency\" \"TRY\"

2026-01-01 custom \"envelope\" \"start date\" \"2026-01\"
2026-01-01 custom \"envelope\" \"budget account\" \"Assets:Bank\"
2026-01-01 custom \"envelope\" \"fund\" Expenses:Food 100.00 TRY
2026-01-01 custom \"envelope\" \"rollover\" Expenses:Food FALSE
2026-03-01 custom \"envelope\" \"fund\" Expenses:Food 300.00 TRY
2026-04-01 custom \"envelope\" \"rollover\" Expenses:Food 50.00 TRY

2026-01-01 open Assets:Bank
2026-01-01 open Expenses:Food
";
        let (_dir, _path, state) = fixture("budget-newest", ledger);
        let settings = settings_of(&state.loaded.read().unwrap().as_ref().unwrap().ledger);
        let food = envelope(&settings, "Expenses:Food");
        assert_eq!(food.fund.as_ref().unwrap().units, "300.00");
        assert_eq!(food.since.as_deref(), Some("2026-03"));
        assert_eq!(
            food.rollover,
            RolloverSetting {
                kind: RolloverKind::Cap,
                amount: Some(Amount {
                    units: "50.00".to_string(),
                    currency: "TRY".to_string()
                })
            }
        );
    }

    #[test]
    fn end_to_end_init_then_fund_reloads_into_the_same_settings() {
        let (_dir, path, state) = fixture("budget-end-to-end", LEDGER);
        let file = journal(&path);

        budget_write::init(
            &state,
            &file,
            "2026-01",
            None,
            None,
            &["Assets:Bank".to_string()],
            true,
            false,
        )
        .unwrap();
        budget_write::fund_set(&state, &file, "Expenses:Food", "400.00", "TRY", "2026-01").unwrap();

        // A fresh load from disk, not the session's own copy.
        let reloaded = loaded_state(&path);
        let settings = budget_write::settings(&reloaded).unwrap();
        let config = settings
            .config
            .as_ref()
            .expect("the config survives a reload");
        assert_eq!(config.start, "2026-01");
        assert_eq!(config.budget_accounts, ["Assets:Bank"]);
        assert!(config.assert);
        assert!(!config.strict);
        assert_eq!(config.file, file);

        let food = envelope(&settings, "Expenses:Food");
        assert_eq!(
            food.fund,
            Some(Amount {
                units: "400.00".to_string(),
                currency: "TRY".to_string()
            })
        );
        assert_eq!(food.since.as_deref(), Some("2026-01"));
        assert_eq!(food.rollover, RolloverSetting::default());
    }

    #[test]
    fn a_second_init_is_refused_even_when_the_start_date_is_in_another_file() {
        let dir = TempDir::new("budget-include");
        dir.seed(
            "main.bean",
            "option \"operating_currency\" \"TRY\"\ninclude \"budget.bean\"\n\n2026-01-01 open Assets:Bank\n",
        );
        let included = dir.seed(
            "budget.bean",
            "2026-01-01 custom \"envelope\" \"start date\" \"2026-01\"\n",
        );
        let main = dir.file("main.bean");
        let state = loaded_state(&main);

        let error = budget_write::init(
            &state,
            &journal(&main),
            "2026-02",
            None,
            None,
            &["Assets:Bank".to_string()],
            true,
            false,
        )
        .unwrap_err();
        assert!(error.contains(&journal(&included)), "{error}");
        assert!(error.contains("line 1"), "{error}");
    }
}
