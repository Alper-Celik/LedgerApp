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
//!
//! # The configuration block and the plugin declaration
//!
//! [`config_set`] rewrites the six configuration keys ([`CONFIG_KEYS`]) of the
//! one block that holds `start date`, wherever that file is, and leaves every
//! other envelope directive exactly where it stood. [`init`] and
//! [`config_set`] also declare the plugin through the same funnel when the
//! journal has no `plugin` line yet: the `.wasm` must exist beside the journal
//! first, because a declaration for a missing file makes the loader fail — so
//! a missing module is an explicit refusal and nothing is written.
//! [`plugin_status`] says whether the journal declares the plugin and where its
//! `.wasm` could live; [`plugin_enable`] and [`plugin_install`] are the repair
//! path for a journal that was configured before this rule existed, or whose
//! module the reader has just downloaded.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
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

/// The plugin's file name, and the line a declaration writes.
const ENVELOPE_PLUGIN: &str = "rustledger_envelope.wasm";

/// The directives [`config_set`] owns. Every other envelope key is a
/// month-scoped instruction and is left byte-for-byte alone.
const CONFIG_KEYS: [&str; 6] = [
    "start date",
    "end date",
    "months ahead",
    "budget account",
    "assert",
    "strict",
];

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

/// One place the envelope plugin's `.wasm` could sit.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct PluginWasm {
    /// Absolute path of the candidate plugin file.
    pub path: String,
    /// Whether a file exists at [`PluginWasm::path`].
    pub present: bool,
}

/// Whether the journal declares the envelope plugin, and where its `.wasm`
/// could live.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct BudgetPluginStatus {
    /// A `plugin` directive names the envelope module.
    pub declared: bool,
    /// The loaded path of the file that declares it, when one does.
    pub file: Option<String>,
    /// The candidate locations for `rustledger_envelope.wasm`: the declaring
    /// file's directory first, then the root journal's. Empty when no ledger
    /// is loaded.
    pub wasm: Vec<PluginWasm>,
}

/// Everything the budget screen's editor reads: whether the plugin is declared,
/// the configuration block, and every account it configures.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct BudgetSettings {
    /// The journal declares an envelope plugin (`plugin "…envelope…wasm"`).
    /// A configuration write adds the line itself when its module is present;
    /// this field is what the screen's notice and repair controls read.
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
        plugin_declared: ledger
            .plugins
            .iter()
            .any(|plugin| names_envelope_plugin(&plugin.name)),
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

/// The file part of a `plugin` directive's path, in either slash convention.
fn plugin_file_name(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

/// Whether a `plugin` directive names the envelope module.
///
/// The module ships as `rustledger_envelope.wasm`; the comparison is on the
/// file name and ignores case, so a declaration through a path
/// (`plugins/rustledger_envelope.wasm`) reads the same as a bare one.
fn names_envelope_plugin(name: &str) -> bool {
    plugin_file_name(name)
        .to_ascii_lowercase()
        .contains("envelope")
}

/// The root journal of the loaded ledger: the first file the loader read, which
/// is the one the session was opened with.
fn root_journal(ledger: &Ledger) -> Result<&rustledger_loader::SourceFile, String> {
    ledger
        .source_map
        .files()
        .first()
        .ok_or_else(|| "the journal holds no files".to_string())
}

/// Whether the journal declares the envelope plugin, where, and where its
/// `.wasm` can live.
///
/// The first candidate is the declaring file's own directory, because a
/// `plugin` path is resolved relative to the file that declares it; before a
/// declaration exists that file is the root journal, which is where
/// [`declaration_for`] puts the line. The root stays a second candidate for a
/// declaration that lives in an included file: the loader resolves every
/// plugin against the root journal, so a module beside the root is still one
/// the loader can find. No ledger loaded answers the empty status rather than
/// an error: the screen reads it before one is open.
pub(crate) fn plugin_status(state: &AppState) -> BudgetPluginStatus {
    let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
    let Some(loaded) = guard.as_ref() else {
        return BudgetPluginStatus::default();
    };
    let root = root_journal(&loaded.ledger).map_or(&loaded.root, |file| &file.path);
    plugin_status_of(&loaded.ledger, root)
}

fn plugin_status_of(ledger: &Ledger, root: &Path) -> BudgetPluginStatus {
    let declared = ledger
        .plugins
        .iter()
        .find(|plugin| names_envelope_plugin(&plugin.name));
    let file = declared.and_then(|plugin| {
        ledger
            .source_map
            .get(plugin.file_id)
            .map(|source| source.path.display().to_string())
    });

    let mut directories: Vec<PathBuf> = Vec::new();
    // The loader resolves every `plugin` path against one base directory: the
    // root journal's, not the declaring include's (`rustledger-loader` passes
    // that `base_dir` straight to `resolve_path`). The root's directory is
    // therefore the candidate an install must write to, and it comes first; the
    // declaring file's directory stays as a second place to look, for a module
    // put beside an include by hand.
    for directory in [
        root.parent(),
        file.as_deref().and_then(|file| Path::new(file).parent()),
    ]
    .into_iter()
    .flatten()
    {
        let directory = directory.to_path_buf();
        if !directories.contains(&directory) {
            directories.push(directory);
        }
    }

    let wasm = directories
        .into_iter()
        .map(|directory| {
            let path = directory.join(ENVELOPE_PLUGIN);
            PluginWasm {
                present: path.is_file(),
                path: path.display().to_string(),
            }
        })
        .collect();

    BudgetPluginStatus {
        declared: declared.is_some(),
        file,
        wasm,
    }
}

/// The refusal when no candidate holds the plugin.
///
/// The screen shows this sentence under the form with the draft still in it,
/// so it names the exact path to put the file at and the command that can
/// copy it there.
fn missing_wasm(status: &BudgetPluginStatus) -> String {
    let path = status
        .wasm
        .first()
        .map(|wasm| wasm.path.as_str())
        .unwrap_or(ENVELOPE_PLUGIN);
    format!(
        "the envelope plugin is not declared and its file is missing: put rustledger_envelope.wasm at {path} first, or install a downloaded copy from the budget screen. A plugin path that does not point at a file makes the loader fail, so nothing was written"
    )
}

/// The `plugin` directive that loads the envelope module.
fn plugin_line() -> String {
    format!("plugin {}", quote_beancount(ENVELOPE_PLUGIN))
}

/// The declaration a configuration write has to add: the root journal and the
/// text that now carries the `plugin` line, or `None` when one is declared.
///
/// The line goes into the root journal rather than the file being configured,
/// so the file that declares the plugin is also the file the `.wasm` sits
/// beside — a `plugin` path is resolved relative to the declaring file.
/// [`plugin_enable`] and [`plugin_install`] write the same line into the same
/// file, so a journal configured here and one repaired there read alike.
///
/// A missing module is a refusal from here, before the caller writes anything:
/// declaring a file that is not there makes the next load fail, and the edit
/// funnel would put the configuration back with it.
fn declaration_for(ledger: &Ledger, root: &Path) -> Result<Option<(PathBuf, String)>, String> {
    let status = plugin_status_of(ledger, root);
    if status.declared {
        return Ok(None);
    }
    if !status.wasm.iter().any(|wasm| wasm.present) {
        return Err(missing_wasm(&status));
    }
    let root_file = root_journal(ledger)?;
    Ok(Some((
        root_file.path.clone(),
        insert_preamble(&root_file.source, &[plugin_line()]),
    )))
}

/// Write a configuration block, adding the plugin declaration when the journal
/// does not have one yet.
///
/// The two go into one funnel call when they share a file — the usual case,
/// with the block in the root journal — so the configuration and the line that
/// reads it land together or not at all. A block that lives in an included
/// file is written first and the declaration second: a `plugin` line written
/// before any `start date` runs the plugin against an empty configuration, and
/// its own diagnostic would roll the declaration back.
///
/// The include's first edit reloads without the plugin running, so it cannot
/// see the block's plugin-level complaints. When the declaration that follows
/// then fails, only the root journal is rolled back by the funnel — the include
/// would keep a block the plugin rejects, and a retry would be refused for a
/// `start date` that never worked. Capturing the include before the first edit
/// and putting it back is what makes the pair land together or not at all.
fn commit_config(
    state: &AppState,
    target: &Path,
    rewritten: &str,
    declaration: Option<(PathBuf, String)>,
) -> Result<(), String> {
    let target_name = target.display().to_string();
    match declaration {
        Some((root, root_text)) => {
            if root.as_path() == target {
                let rewritten = insert_preamble(rewritten, &[plugin_line()]);
                return crate::editing::edit_named_file_source(
                    state,
                    &target_name,
                    &rewritten,
                    None,
                );
            }
            let before = std::fs::read_to_string(target)
                .map_err(|error| format!("{target_name}: {error}"))?;
            crate::editing::edit_named_file_source(state, &target_name, rewritten, None)?;
            let declared = crate::editing::edit_named_file_source(
                state,
                &root.display().to_string(),
                &root_text,
                None,
            );
            match declared {
                Ok(()) => Ok(()),
                Err(error) => {
                    // Best effort: the sentence that matters is the plugin's, so a
                    // restore that also fails is reported beside it, not instead.
                    if let Err(restore) =
                        crate::editing::edit_named_file_source(state, &target_name, &before, None)
                    {
                        return Err(format!(
                            "{error}; and {target_name} could not be put back: {restore}"
                        ));
                    }
                    Err(error)
                }
            }
        }
        None => crate::editing::edit_named_file_source(state, &target_name, rewritten, None),
    }
}

/// Declare the envelope plugin in the root journal.
///
/// Already declared: the status comes back unchanged and nothing is written.
/// Otherwise the plugin file has to exist first, in one of the candidate
/// locations [`plugin_status`] reports. A declaration for a missing file makes
/// the loader fail, and the edit funnel would put the journal back, so the
/// check is what turns that rollback into a sentence naming the path to put
/// the file at.
pub(crate) fn plugin_enable(state: &AppState) -> Result<BudgetPluginStatus, String> {
    let declaration = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().ok_or("no ledger loaded")?;
        let root = root_journal(&loaded.ledger)?.path.clone();
        // A declaration with no configuration runs the plugin against nothing, and
        // its own "missing start date" diagnostic makes the funnel roll the line
        // back — an action that can only fail. The line is written with the first
        // block instead.
        if configuration_block(&loaded.ledger).is_none() {
            return Err(
                "the journal has no envelope configuration yet; the `plugin` line is written with the first budget setup, because the plugin refuses to load without a `start date`"
                    .to_string(),
            );
        }
        declaration_for(&loaded.ledger, &root)?
    };

    if let Some((root, text)) = declaration {
        crate::editing::edit_named_file_source(state, &root.display().to_string(), &text, None)?;
    }
    Ok(plugin_status(state))
}

/// Install the reader's own copy of the plugin beside the file that declares
/// it and, when the journal already has a configuration, declare it.
///
/// The source is whatever the file chooser handed back — normally the release
/// asset the reader downloaded. It is read and checked for the WebAssembly
/// magic before anything is written; the target is the first candidate
/// [`plugin_status`] names, which is the declaring file's directory and, with
/// no declaration yet, the root journal's. A file already there is kept when
/// it is byte-identical and never overwritten when it is a different plugin: a
/// ledger already using an envelope module must not see it change under a
/// stray download. Anything that cannot succeed — an unreadable source, the
/// wrong bytes, a missing target directory — is refused before the first
/// write, so a failed install leaves the ledger untouched.
///
/// With no configuration in the journal the file is copied but the `plugin`
/// line is not written: the plugin reports a missing `start date` on the next
/// load and the edit funnel would roll the declaration back. The next
/// configuration write adds the line with the block, in one edit.
pub(crate) fn plugin_install(state: &AppState, source: &str) -> Result<BudgetPluginStatus, String> {
    #[cfg(target_os = "android")]
    {
        let _ = (state, source);
        return Err(
            "installing the envelope plugin is not available on Android: the ledger is a mirror of a storage tree, so copy rustledger_envelope.wasm into the ledger folder itself with the system file manager and sync it; a file the app chose from its own storage would not reach the ledger"
                .to_string(),
        );
    }

    #[cfg(not(target_os = "android"))]
    {
        let (target, configured) = {
            let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
            let loaded = guard.as_ref().ok_or("no ledger loaded")?;
            let root = root_journal(&loaded.ledger)?;
            let status = plugin_status_of(&loaded.ledger, &root.path);
            let target = status
                .wasm
                .first()
                .map(|wasm| PathBuf::from(&wasm.path))
                .ok_or_else(|| {
                    format!(
                        "{} has no directory; the plugin cannot be installed beside it",
                        root.path.display()
                    )
                })?;
            let directory = target.parent().unwrap_or_else(|| Path::new("."));
            if !directory.is_dir() {
                return Err(format!(
                    "{} is not a directory; the plugin cannot be installed beside the journal",
                    directory.display()
                ));
            }
            (target, configuration_block(&loaded.ledger).is_some())
        };

        let bytes = std::fs::read(source).map_err(|error| format!("{source}: {error}"))?;
        if !bytes.starts_with(b"\0asm") {
            return Err(not_a_wasm_module(source, &bytes));
        }
        if target.exists() {
            let existing =
                std::fs::read(&target).map_err(|error| format!("{}: {error}", target.display()))?;
            if existing != bytes {
                return Err(format!(
                    "refusing to overwrite {}: it already holds a different plugin; move that file away first",
                    target.display()
                ));
            }
        } else {
            crate::fsutil::write_atomic(&target, &bytes, Some(0o644))?;
        }
        if configured {
            plugin_enable(state)
        } else {
            Ok(plugin_status(state))
        }
    }
}

/// What a refused install says about the source: enough of its head to tell a
/// script, an archive or a truncated download from a module.
#[cfg(not(target_os = "android"))]
fn not_a_wasm_module(source: &str, bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return format!(
            "{source} is empty; the plugin is a WebAssembly module that starts with the bytes `\\0asm`"
        );
    }
    let head: String = bytes
        .iter()
        .take(8)
        .map(|byte| {
            if byte.is_ascii_graphic() || *byte == b' ' {
                char::from(*byte).to_string()
            } else {
                format!("\\x{byte:02x}")
            }
        })
        .collect();
    format!(
        "{source} is not a WebAssembly module: it starts with `{head}` ({} bytes), not `\\0asm`",
        bytes.len()
    )
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

/// Append the budget's configuration block to `file`, and the plugin
/// declaration when the journal does not have one.
///
/// A second `start date` is refused rather than added: the plugin reads the
/// newest one, so two would make the older directive silently dead. The error
/// names the file and line that already holds it.
///
/// The plugin declaration is part of the same write when the block and the
/// root journal are the same file, so the screen's setup is complete in one
/// edit; a block written into an included file gets the declaration in a
/// second funnel call against the root journal. A journal whose
/// `rustledger_envelope.wasm` is missing is refused before anything is
/// written — the sentence names the path, and so the setup is never a
/// half-configured journal the next load would reject.
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
    let budget_accounts = checked_patterns(budget_accounts)?;

    let (target, rewritten, declaration) = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().ok_or("no ledger loaded")?;
        if let Some((found, line)) = existing_start_date(&loaded.ledger) {
            return Err(format!(
                "{found} already declares a budget `start date` at line {line}; edit that directive instead of adding a second"
            ));
        }
        let source = source_of_file(&loaded.ledger, file)?;
        let root = root_journal(&loaded.ledger)?.path.clone();
        let declaration = declaration_for(&loaded.ledger, &root)?;
        let previous = source.source.to_string();
        let lines = config_block_lines(
            &month_start(&start)?.to_string(),
            &start,
            months_ahead,
            end.as_deref(),
            &budget_accounts,
            assert,
            strict,
        );
        let rewritten = insert_block(&previous, &lines);
        (source.path.clone(), rewritten, declaration)
    };

    commit_config(state, &target, &rewritten, declaration)
}

/// Replace the journal's existing envelope configuration with the one given.
///
/// The block rewritten is the one that holds `start date`, in whichever journal
/// file that is: [`BudgetConfig::file`] comes back as that file, which need not
/// be `file`. Every directive with a key in [`CONFIG_KEYS`] in that file is
/// removed and the requested set is written once, in its place; a key the new
/// configuration does not use (an `end date` when `months ahead` replaces it,
/// or an `assert`/`strict` that is back at the plugin's default) therefore has
/// no line left to override it. Every other envelope directive — `fund`,
/// `rollover`, `adjust`, `zero`, `pad`, `balance`, `map`, … — keeps its exact
/// bytes and position.
///
/// `file` is the screen's context, not the target: a configuration that lives
/// in an included file is edited where it lives, because a second `start date`
/// in the named file would lose to the newer one.
///
/// The two ways of bounding the budget are mutually exclusive in the plugin
/// (an `end date` or a number of `months ahead`), so asking for both is refused
/// here rather than written as a block the next load would reject.
///
/// A journal that has the block but no `plugin` line gets one in the same
/// write (or, when the block lives in an included file, in a second call to
/// the root journal), so editing a legacy configuration repairs it. A missing
/// `.wasm` is refused before anything is written.
pub(crate) fn config_set(
    state: &AppState,
    _file: &str,
    start: &str,
    months_ahead: Option<u32>,
    end: Option<&str>,
    budget_accounts: &[String],
    assert: bool,
    strict: bool,
) -> Result<BudgetConfig, String> {
    let start = checked_month("start", start)?;
    let end = end.map(|end| checked_month("end", end)).transpose()?;
    if months_ahead.is_some() && end.is_some() {
        return Err(
            "`months ahead` and `end date` cannot both be set; choose a last month (`end`) or a number of months ahead (`months_ahead`), not both"
                .to_string(),
        );
    }
    let budget_accounts = checked_patterns(budget_accounts)?;
    let lines = config_block_lines(
        &month_start(&start)?.to_string(),
        &start,
        months_ahead,
        end.as_deref(),
        &budget_accounts,
        assert,
        strict,
    );

    let (target, rewritten, declaration) = {
        let guard = state.loaded.read().unwrap_or_else(|e| e.into_inner());
        let loaded = guard.as_ref().ok_or("no ledger loaded")?;
        let (file_id, starts) = configuration_block(&loaded.ledger).ok_or_else(|| {
            "this journal has no envelope budget `start date` yet; set one up with budget_init before editing it"
                .to_string()
        })?;
        let source = source_of_id(&loaded.ledger, file_id)?;
        let root = root_journal(&loaded.ledger)?.path.clone();
        let declaration = declaration_for(&loaded.ledger, &root)?;
        let previous = source.source.to_string();
        let rewritten = rewrite_config_lines(&previous, &starts, &lines);
        (source.path.clone(), rewritten, declaration)
    };

    commit_config(state, &target, &rewritten, declaration)?;

    Ok(BudgetConfig {
        file: target.display().to_string(),
        start,
        end,
        months_ahead,
        budget_accounts,
        assert,
        strict,
    })
}

/// One configuration block's lines, as the plugin writes them.
fn config_block_lines(
    date: &str,
    start: &str,
    months_ahead: Option<u32>,
    end: Option<&str>,
    budget_accounts: &[String],
    assert: bool,
    strict: bool,
) -> Vec<String> {
    let mut lines = vec![directive_line(
        date,
        "start date",
        &[quote_beancount(start)],
    )];
    if let Some(end) = end {
        lines.push(directive_line(date, "end date", &[quote_beancount(end)]));
    }
    if let Some(count) = months_ahead {
        lines.push(directive_line(date, "months ahead", &[count.to_string()]));
    }
    for pattern in budget_accounts {
        lines.push(directive_line(
            date,
            "budget account",
            &[quote_beancount(pattern)],
        ));
    }
    // Both flags are only written when they differ from the plugin's own
    // defaults, so the block says what changes and nothing else.
    if !assert {
        lines.push(directive_line(date, "assert", &["FALSE".to_string()]));
    }
    if strict {
        lines.push(directive_line(date, "strict", &["TRUE".to_string()]));
    }
    lines
}

/// The on-budget patterns, with the one rule the block syntax needs: at least
/// one, none of them empty.
fn checked_patterns(patterns: &[String]) -> Result<Vec<String>, String> {
    if patterns.is_empty() {
        return Err("budget_accounts is empty; an envelope budget needs at least one on-budget account pattern".to_string());
    }
    if patterns.iter().any(|pattern| pattern.trim().is_empty()) {
        return Err(
            "budget_accounts contains an empty pattern; every on-budget pattern must match at least one character"
                .to_string(),
        );
    }
    Ok(patterns.to_vec())
}

/// The file and the line starts of the configuration block that holds the
/// newest `start date`.
fn configuration_block(ledger: &Ledger) -> Option<(usize, Vec<usize>)> {
    let (file_id, _) = newest_start(ledger)?;
    let starts = ledger
        .directives
        .iter()
        .filter(|spanned| usize::from(spanned.file_id) == file_id)
        .filter_map(|spanned| {
            let Directive::Custom(custom) = &spanned.value else {
                return None;
            };
            if custom.custom_type != "envelope" {
                return None;
            }
            let key = custom.values.first().and_then(meta_string)?;
            CONFIG_KEYS.contains(&key).then_some(spanned.span.start)
        })
        .collect();
    Some((file_id, starts))
}

/// The newest valid `start date` directive, and the file it lives in.
fn newest_start(ledger: &Ledger) -> Option<(usize, NaiveDate)> {
    let mut newest: Option<(NaiveDate, usize, usize)> = None;
    for (seq, spanned) in ledger.directives.iter().enumerate() {
        let Directive::Custom(custom) = &spanned.value else {
            continue;
        };
        if custom.custom_type != "envelope"
            || custom.values.first().and_then(meta_string) != Some("start date")
        {
            continue;
        }
        let Some(month) = custom.values.get(1).and_then(meta_string) else {
            continue;
        };
        if !is_month_label(month) {
            continue;
        }
        let candidate = (custom.date, seq, usize::from(spanned.file_id));
        if newest
            .as_ref()
            .is_none_or(|current| (candidate.0, candidate.1) > (current.0, current.1))
        {
            newest = Some(candidate);
        }
    }
    newest.map(|(date, _, file_id)| (file_id, date))
}

/// The journal file the loader holds under `file_id`.
///
/// A directive found by month may live in any of the journal's files; the write
/// has to land in that one, or the second directive would lose to the first.
fn source_of_id(ledger: &Ledger, file_id: usize) -> Result<&rustledger_loader::SourceFile, String> {
    ledger
        .source_map
        .get(file_id)
        .ok_or_else(|| format!("the journal holds no file with id {file_id}"))
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
            .is_some_and(|entry| month_label(entry.0) <= month && entry.1 == requested)
        {
            return Ok(());
        }
        let arguments = [
            account.clone(),
            requested.number.to_string(),
            requested.currency.clone(),
        ];
        match same_month_directive(&loaded.ledger, "fund", &account, &month) {
            // Rewritten where it stands, on the date it already carries: a
            // second directive in the same month would lose to the older one, and
            // changing the amount is not a reason to move the reader's line.
            Some((file_id, start, on)) => {
                let target = source_of_id(&loaded.ledger, file_id)?;
                let line = directive_line(&on.to_string(), "fund", &arguments);
                (
                    target.path.clone(),
                    replace_line(&target.source, start, &line),
                )
            }
            None => {
                let source = source_of_file(&loaded.ledger, file)?;
                let line = directive_line(&date.to_string(), "fund", &arguments);
                (source.path.clone(), insert_block(&source.source, &[line]))
            }
        }
    };

    let (target, rewritten) = rewritten;
    crate::editing::edit_named_file_source(state, &target.display().to_string(), &rewritten, None)
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
            .is_some_and(|entry| month_label(entry.0) <= month && entry.1 == requested)
        {
            return Ok(());
        }
        let arguments = [account.clone(), policy_token(&requested)];
        match same_month_directive(&loaded.ledger, "rollover", &account, &month) {
            Some((file_id, start, on)) => {
                let target = source_of_id(&loaded.ledger, file_id)?;
                let line = directive_line(&on.to_string(), "rollover", &arguments);
                (
                    target.path.clone(),
                    replace_line(&target.source, start, &line),
                )
            }
            None => {
                let source = source_of_file(&loaded.ledger, file)?;
                let line = directive_line(&date.to_string(), "rollover", &arguments);
                (source.path.clone(), insert_block(&source.source, &[line]))
            }
        }
    };

    let (target, rewritten) = rewritten;
    crate::editing::edit_named_file_source(state, &target.display().to_string(), &rewritten, None)
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
fn same_month_directive(
    ledger: &Ledger,
    key: &str,
    account: &str,
    month: &str,
) -> Option<(usize, usize, NaiveDate)> {
    // The plugin resolves by month, and a tie within a month keeps the *last*
    // directive in the canonical stream: a `fund` dated the 15th beats one dated
    // the 1st. So the grep is by month, and the last one found is the one that
    // governs — replacing it is the only way an edit takes effect.
    let mut found: Option<(usize, usize, NaiveDate)> = None;
    for spanned in &ledger.directives {
        let Directive::Custom(custom) = &spanned.value else {
            continue;
        };
        if custom.custom_type != "envelope"
            || month_label(custom.date) != month
            || custom.values.first().and_then(meta_string) != Some(key)
            || custom.values.get(1).and_then(meta_account) != Some(account)
        {
            continue;
        }
        found = Some((
            usize::from(spanned.file_id),
            spanned.span.start,
            custom.date,
        ));
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

/// The whole line that starts at `offset`, newline included.
fn line_range(text: &str, offset: usize) -> (usize, usize) {
    let start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    let end = text[start..]
        .find('\n')
        .map_or(text.len(), |index| start + index + 1);
    (start, end)
}

/// Replace the lines at `starts` with `block`, all of it in the place the first
/// of them occupied.
///
/// The starts come from directive spans, which the loader sorts by date rather
/// than by position, so they are sorted here. A range that overlaps an earlier
/// one is skipped (two directives cannot share a line, but the guard keeps a
/// malformed span from duplicating bytes). The block inherits the removed
/// line's newline, so a file whose last line had none keeps it that way.
fn rewrite_config_lines(text: &str, starts: &[usize], block: &[String]) -> String {
    let mut ranges: Vec<(usize, usize)> = starts
        .iter()
        .map(|start| line_range(text, *start))
        .collect();
    ranges.sort_unstable();

    let eol = line_ending(text);
    let joined = block.join(eol);
    let mut rewritten = String::with_capacity(text.len() + joined.len() + eol.len());
    let mut cursor = 0;
    let mut inserted = false;
    for (start, end) in ranges {
        if start < cursor {
            continue;
        }
        rewritten.push_str(&text[cursor..start]);
        if !inserted {
            rewritten.push_str(&joined);
            if text[start..end].ends_with('\n') || end < text.len() {
                rewritten.push_str(eol);
            }
            inserted = true;
        }
        cursor = end;
    }
    rewritten.push_str(&text[cursor..]);
    rewritten
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
    insert_at(text, lines, insertion_offset(text))
}

/// Insert `lines` after the file's last `plugin`/`option` line, else at the top
/// — where a `plugin` declaration belongs, even when the file already has an
/// envelope group further down.
fn insert_preamble(text: &str, lines: &[String]) -> String {
    insert_at(text, lines, preamble_offset(text))
}

fn insert_at(text: &str, lines: &[String], offset: usize) -> String {
    let eol = line_ending(text);
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

    preamble_offset(text)
}

/// The offset just past the file's last `plugin`/`option` line, else zero.
fn preamble_offset(text: &str) -> usize {
    let lines = line_spans(text);
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

    /// A configured budget with a last month, an explicit non-default flag and
    /// the plugin's other directives around the configuration: the block the
    /// editor rewrites, a `fund` and a `rollover` it must leave alone, and
    /// ordinary directives after.
    const CONFIGURED_WITH_END: &str = "\
option \"operating_currency\" \"TRY\"

2026-01-01 custom \"envelope\" \"start date\" \"2026-01\"
2026-01-01 custom \"envelope\" \"end date\" \"2026-06\"
2026-01-01 custom \"envelope\" \"budget account\" \"Assets:Bank\"
2026-01-01 custom \"envelope\" \"assert\" FALSE
2026-01-01 custom \"envelope\" \"fund\" Expenses:Food 400.00 TRY
2026-01-01 custom \"envelope\" \"rollover\" Expenses:Food FALSE

2026-01-01 open Assets:Bank
2026-01-01 open Expenses:Food
";

    /// A fixture journal, with the envelope plugin's module beside it.
    ///
    /// Every configuration write now declares the plugin, so a fixture that
    /// expects to configure a journal needs a module the loader can run; the
    /// pass-through module below covers any directive count.
    fn fixture(name: &str, ledger: &str) -> (TempDir, std::path::PathBuf, AppState) {
        let dir = TempDir::new(name);
        let path = dir.seed("main.bean", ledger);
        fs::write(dir.file(ENVELOPE_PLUGIN), passthrough_wasm()).unwrap();
        let state = loaded_state(&path);
        (dir, path, state)
    }

    /// A fixture journal with no `.wasm` beside it: the state a reader starts
    /// in when they have not downloaded the plugin yet.
    fn fixture_without_wasm(name: &str, ledger: &str) -> (TempDir, std::path::PathBuf, AppState) {
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

    /// A minimal envelope-plugin module the loader can run: every input
    /// directive comes back as `Keep`, with no diagnostics, however many there
    /// are.
    ///
    /// The AppState fixtures are bare journals, but the loader executes every
    /// `plugin` it finds — a missing or malformed module raises a `PLUGIN`
    /// error, and the edit funnel would refuse (and roll back) the declaration
    /// that introduced it. A configuration write changes the directive count,
    /// so a module with a baked `Keep(0) … Keep(n)` list would have to be
    /// regenerated per write; this one reads the count back out of the
    /// MessagePack input instead.
    ///
    /// The ABI: `PluginInput` is a three-field struct serialized as an array,
    /// so the directive vector's header sits at `ptr + 1`. The fixture
    /// journals stay under sixteen directives, where rmp writes the length as
    /// the low nibble of a fixarray (`0x90 | n`). The output is
    /// `[ops, errors]` at address 0 — each op `{"Keep": i}` as a one-pair map,
    /// followed by the empty error array — and `alloc` puts the host's input
    /// far enough away not to overwrite it. `process` returns the address and
    /// length packed the way the runtime expects (`ptr << 32 | len`).
    fn passthrough_wasm() -> Vec<u8> {
        let text = r#"(module
  (memory (export "memory") 1)
  (func (export "__rustledger_abi_version") (result i32) i32.const 1)
  (func (export "alloc") (param i32) (result i32) i32.const 8192)
  (func (export "process") (param $ptr i32) (param $len i32) (result i64)
    (local $count i32)
    (local $out i32)
    (local $next i32)
    local.get $ptr
    i32.load8_u offset=1
    i32.const 15
    i32.and
    local.set $count
    i32.const 0
    i32.const 0x92
    i32.store8
    i32.const 1
    local.get $count
    i32.const 0x90
    i32.or
    i32.store8
    i32.const 2
    local.set $out
    i32.const 0
    local.set $next
    block $done
      loop $ops
        local.get $next
        local.get $count
        i32.ge_u
        br_if $done
        local.get $out
        i32.const 0x81
        i32.store8
        local.get $out
        i32.const 0xa4
        i32.store8 offset=1
        local.get $out
        i32.const 0x7065654b
        i32.store offset=2
        local.get $out
        local.get $next
        i32.store8 offset=6
        local.get $out
        i32.const 7
        i32.add
        local.set $out
        local.get $next
        i32.const 1
        i32.add
        local.set $next
        br $ops
      end
    end
    local.get $out
    i32.const 0x90
    i32.store8
    local.get $out
    i32.const 1
    i32.add
    i64.extend_i32_u
  )
)"#;
        wat::parse_str(text).expect("the fixture module parses")
    }

    /// The file with every configuration directive line and the plugin
    /// declaration removed — what [`budget_write::config_set`] promises to
    /// leave alone.
    fn without_config_lines(text: &str) -> String {
        let keys = [
            "\"start date\"",
            "\"end date\"",
            "\"months ahead\"",
            "\"budget account\"",
            "\"assert\"",
            "\"strict\"",
        ];
        text.split_inclusive('\n')
            .filter(|line| {
                if line.trim_start().starts_with("plugin ") {
                    return false;
                }
                !line.contains("custom \"envelope\"") || !keys.iter().any(|key| line.contains(key))
            })
            .collect()
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
        // The declaration and the block follow the option line; one blank line
        // separates each of the two groups, and the open directives that used
        // to sit under the option line stay one blank line below.
        let block = "\
2026-02-01 custom \"envelope\" \"start date\" \"2026-02\"
2026-02-01 custom \"envelope\" \"end date\" \"2026-12\"
2026-02-01 custom \"envelope\" \"months ahead\" 2
2026-02-01 custom \"envelope\" \"budget account\" \"Assets:Bank\"
2026-02-01 custom \"envelope\" \"assert\" FALSE
2026-02-01 custom \"envelope\" \"strict\" TRUE
";
        let expected = format!(
            "option \"operating_currency\" \"TRY\"\nplugin \"rustledger_envelope.wasm\"\n\n{block}\n2026-01-01 open Assets:Bank\n2026-01-01 open Expenses:Food\n"
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
        assert!(
            settings.plugin_declared,
            "the setup declares the plugin when its module is there"
        );

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

    /// The plugin resolves a dated key by *month*, and a tie keeps the last
    /// directive in the stream: a `fund` dated the 15th beats one dated the 1st.
    /// Matching by day would insert a second directive that the older one beats,
    /// so the edit would look applied and change nothing.
    #[test]
    fn a_mid_month_directive_is_replaced_not_duplicated() {
        let (_dir, path, state) = fixture(
            "budget-mid-month",
            "\
option \"operating_currency\" \"TRY\"

2026-01-01 custom \"envelope\" \"start date\" \"2026-01\"
2026-01-01 custom \"envelope\" \"budget account\" \"^Assets:Bank\"
2026-01-15 custom \"envelope\" \"fund\" Expenses:Food 100.00 TRY
",
        );
        let file = journal(&path);

        budget_write::fund_set(&state, &file, "Expenses:Food", "150.00", "TRY", "2026-01").unwrap();

        let after = read(&path);
        assert_eq!(occurrences(&after, "custom \"envelope\" \"fund\""), 1);
        assert!(
            after.contains("2026-01-15 custom \"envelope\" \"fund\" Expenses:Food 150.00 TRY"),
            "the governing directive was not the one rewritten:\n{after}"
        );
        let settings = settings_of(&state.loaded.read().unwrap().as_ref().unwrap().ledger);
        assert_eq!(
            envelope(&settings, "Expenses:Food").fund,
            Some(Amount {
                units: "150.00".to_string(),
                currency: "TRY".to_string()
            })
        );
        // The same amount at that month is still a no-op.
        budget_write::fund_set(&state, &file, "Expenses:Food", "150.00", "TRY", "2026-01").unwrap();
        assert_eq!(read(&path), after);
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

    #[test]
    fn config_set_rewrites_the_block_in_place_and_switches_the_limit() {
        let (_dir, path, state) = fixture("budget-config-set", CONFIGURED_WITH_END);
        let file = journal(&path);
        let before = read(&path);

        let accounts = vec!["Assets:Bank".to_string(), "Liabilities:TR:.*".to_string()];
        let config = budget_write::config_set(
            &state,
            &file,
            "2026-02",
            Some(3),
            None,
            &accounts,
            true,
            true,
        )
        .unwrap();

        assert_eq!(
            config,
            BudgetConfig {
                file: file.clone(),
                start: "2026-02".to_string(),
                end: None,
                months_ahead: Some(3),
                budget_accounts: accounts.clone(),
                assert: true,
                strict: true,
            }
        );

        let text = read(&path);
        assert!(!text.contains("\"end date\""), "{text}");
        assert_eq!(occurrences(&text, "plugin \"rustledger_envelope.wasm\""), 1);
        assert!(
            text.starts_with(
                "option \"operating_currency\" \"TRY\"\nplugin \"rustledger_envelope.wasm\"\n\n"
            ),
            "the declaration joins the preamble:\n{text}"
        );
        assert!(
            budget_write::settings(&state).unwrap().plugin_declared,
            "editing a legacy configuration adds the plugin line"
        );
        assert!(text.contains("2026-02-01 custom \"envelope\" \"start date\" \"2026-02\""));
        assert!(text.contains("2026-02-01 custom \"envelope\" \"months ahead\" 3"));
        assert!(text.contains("2026-02-01 custom \"envelope\" \"budget account\" \"Assets:Bank\""));
        assert!(
            text.contains(
                "2026-02-01 custom \"envelope\" \"budget account\" \"Liabilities:TR:.*\""
            )
        );
        // Assert is back at the plugin's default, so it has no line; strict is
        // not, so it has one.
        assert!(!text.contains("\"assert\""), "{text}");
        assert!(text.contains("2026-02-01 custom \"envelope\" \"strict\" TRUE"));
        // Fund and rollover survive byte for byte, in their own place.
        assert!(text.contains("2026-01-01 custom \"envelope\" \"fund\" Expenses:Food 400.00 TRY"));
        assert!(text.contains("2026-01-01 custom \"envelope\" \"rollover\" Expenses:Food FALSE"));
        assert_eq!(without_config_lines(&text), without_config_lines(&before));

        // The reverse: a last month replaces the count, and the count's line is
        // gone — both present would be rejected by the plugin.
        let config = budget_write::config_set(
            &state,
            &file,
            "2026-02",
            None,
            Some("2026-12"),
            &accounts,
            false,
            false,
        )
        .unwrap();
        assert_eq!(config.end.as_deref(), Some("2026-12"));
        assert_eq!(config.months_ahead, None);

        let text = read(&path);
        assert!(!text.contains("\"months ahead\""), "{text}");
        assert!(text.contains("2026-02-01 custom \"envelope\" \"end date\" \"2026-12\""));
        assert!(text.contains("2026-02-01 custom \"envelope\" \"assert\" FALSE"));
        assert!(!text.contains("\"strict\""), "{text}");
        assert_eq!(without_config_lines(&text), without_config_lines(&before));

        // The same request again writes nothing.
        let again = budget_write::config_set(
            &state,
            &file,
            "2026-02",
            None,
            Some("2026-12"),
            &accounts,
            false,
            false,
        )
        .unwrap();
        assert_eq!(again.end.as_deref(), Some("2026-12"));
        assert_eq!(read(&path), text);
    }

    #[test]
    fn config_set_edits_the_file_that_holds_the_start_date() {
        let dir = TempDir::new("budget-config-include");
        dir.seed(
            "main.bean",
            "option \"operating_currency\" \"TRY\"\ninclude \"budget.bean\"\n\n2026-01-01 open Assets:Bank\n2026-01-01 open Expenses:Food\n",
        );
        let included = dir.seed(
            "budget.bean",
            "2026-01-01 custom \"envelope\" \"start date\" \"2026-01\"\n\
             2026-01-01 custom \"envelope\" \"budget account\" \"Assets:Bank\"\n\
             2026-01-01 custom \"envelope\" \"fund\" Expenses:Food 100.00 TRY\n",
        );
        fs::write(dir.file(ENVELOPE_PLUGIN), passthrough_wasm()).unwrap();
        let main = dir.file("main.bean");
        let state = loaded_state(&main);

        let config = budget_write::config_set(
            &state,
            &journal(&main),
            "2026-03",
            Some(1),
            None,
            &["Assets:Bank".to_string()],
            true,
            false,
        )
        .unwrap();

        assert_eq!(config.file, journal(&included));
        let main_text = read(&main);
        // The declaration goes into the root journal even when the block lives
        // in an include: that is the file a `plugin` path resolves from and
        // the file the module sits beside.
        assert_eq!(
            occurrences(&main_text, "plugin \"rustledger_envelope.wasm\""),
            1
        );
        assert!(
            main_text.starts_with(
                "option \"operating_currency\" \"TRY\"\nplugin \"rustledger_envelope.wasm\"\n"
            ),
            "{main_text}"
        );
        let included_text = read(&included);
        assert!(
            included_text.contains("2026-03-01 custom \"envelope\" \"start date\" \"2026-03\""),
            "{included_text}"
        );
        assert!(included_text.contains("2026-03-01 custom \"envelope\" \"months ahead\" 1"));
        assert!(
            included_text
                .contains("2026-01-01 custom \"envelope\" \"fund\" Expenses:Food 100.00 TRY")
        );
        assert!(
            !included_text.contains("plugin "),
            "the block's file is not where the declaration goes:\n{included_text}"
        );
        let settings = budget_write::settings(&state).unwrap();
        assert!(settings.plugin_declared);
        assert_eq!(
            settings.config.map(|config| config.start),
            Some("2026-03".to_string())
        );
    }

    #[test]
    fn config_set_refuses_missing_configuration_or_both_limits() {
        let (_dir, path, state) = fixture("budget-config-missing", LEDGER);
        let file = journal(&path);
        let before = read(&path);

        let error = budget_write::config_set(
            &state,
            &file,
            "2026-02",
            Some(1),
            None,
            &["Assets:Bank".to_string()],
            true,
            false,
        )
        .unwrap_err();
        assert!(error.contains("budget_init"), "{error}");
        assert_eq!(read(&path), before);

        let (_dir, path, state) = fixture("budget-config-both-limits", CONFIGURED_WITH_END);
        let file = journal(&path);
        let before = read(&path);
        let error = budget_write::config_set(
            &state,
            &file,
            "2026-02",
            Some(1),
            Some("2026-12"),
            &["Assets:Bank".to_string()],
            true,
            false,
        )
        .unwrap_err();
        assert!(error.contains("months_ahead"), "{error}");
        assert!(error.contains("end"), "{error}");
        assert_eq!(read(&path), before);
    }

    #[test]
    fn config_set_keeps_crlf_and_its_trailing_newline() {
        let ledger = "option \"operating_currency\" \"TRY\"\r\n\r\n\
                      2026-01-01 custom \"envelope\" \"start date\" \"2026-01\"\r\n\
                      2026-01-01 custom \"envelope\" \"end date\" \"2026-06\"\r\n\
                      2026-01-01 custom \"envelope\" \"budget account\" \"Assets:Bank\"\r\n\r\n\
                      2026-01-01 open Assets:Bank\r\n";
        let (_dir, path, state) = fixture("budget-config-crlf", ledger);

        budget_write::config_set(
            &state,
            &journal(&path),
            "2026-02",
            Some(2),
            None,
            &["Assets:Bank".to_string()],
            true,
            false,
        )
        .unwrap();

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
        assert!(!text.contains("\"end date\""), "{text}");
        assert!(text.contains("2026-02-01 custom \"envelope\" \"months ahead\" 2\r\n"));
    }

    #[test]
    fn plugin_status_names_the_wasm_and_enable_refuses_without_one() {
        let (_dir, path, state) = fixture_without_wasm("budget-plugin-missing", CONFIGURED);
        let status = budget_write::plugin_status(&state);
        assert!(!status.declared);
        assert!(status.file.is_none());
        assert_eq!(status.wasm.len(), 1);
        let candidate = &status.wasm[0];
        assert_eq!(
            candidate.path,
            path.with_file_name("rustledger_envelope.wasm")
                .display()
                .to_string()
        );
        assert!(!candidate.present);

        let before = read(&path);
        let error = budget_write::plugin_enable(&state).unwrap_err();
        assert!(error.contains(&candidate.path), "{error}");
        assert!(error.contains(ENVELOPE_PLUGIN), "{error}");
        assert_eq!(read(&path), before, "a refused enable writes nothing");
    }

    #[test]
    fn plugin_enable_writes_the_declaration_once() {
        let (_dir, path, state) = fixture("budget-plugin-enable", CONFIGURED);

        let status = budget_write::plugin_status(&state);
        assert!(!status.declared);
        assert!(status.wasm[0].present);

        let status = budget_write::plugin_enable(&state).unwrap();
        assert!(status.declared);
        assert_eq!(status.file.as_deref(), Some(journal(&path).as_str()));

        let text = read(&path);
        assert_eq!(occurrences(&text, "plugin \"rustledger_envelope.wasm\""), 1);
        assert!(
            text.starts_with(
                "option \"operating_currency\" \"TRY\"\nplugin \"rustledger_envelope.wasm\"\n"
            ),
            "the declaration joins the journal's preamble:\n{text}"
        );

        let again = budget_write::plugin_enable(&state).unwrap();
        assert!(again.declared);
        assert_eq!(read(&path), text, "a second enable writes nothing");

        let reloaded = loaded_state(&path);
        assert!(budget_write::settings(&reloaded).unwrap().plugin_declared);
    }

    #[test]
    fn install_refuses_a_non_wasm_and_a_different_existing_plugin() {
        let (dir, path, state) = fixture_without_wasm("budget-plugin-install-refused", CONFIGURED);
        let before = read(&path);
        let source = dir.seed("downloaded.txt", "not a plugin\n");

        let error = budget_write::plugin_install(&state, &journal(&source)).unwrap_err();
        assert!(error.contains("not a WebAssembly module"), "{error}");
        assert!(!dir.file("rustledger_envelope.wasm").exists());
        assert_eq!(read(&path), before);

        // A different module already at the target is never clobbered.
        let target = dir.file("rustledger_envelope.wasm");
        let existing: &[u8] = b"\0asm\x01\x00\x00\x00different";
        fs::write(&target, existing).unwrap();
        let source = dir.file("downloaded.wasm");
        fs::write(&source, passthrough_wasm()).unwrap();

        let error = budget_write::plugin_install(&state, &journal(&source)).unwrap_err();
        assert!(error.contains("refusing to overwrite"), "{error}");
        assert_eq!(fs::read(&target).unwrap(), existing);
        assert_eq!(read(&path), before);
    }

    #[test]
    fn install_puts_the_plugin_beside_the_journal_and_declares_it() {
        let (dir, path, state) = fixture_without_wasm("budget-plugin-install", CONFIGURED);
        let source = dir.file("downloads/rustledger_envelope.wasm");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, passthrough_wasm()).unwrap();

        let status = budget_write::plugin_install(&state, &journal(&source)).unwrap();
        assert!(status.declared);
        let target = dir.file("rustledger_envelope.wasm");
        assert!(target.is_file());
        assert_eq!(fs::read(&target).unwrap(), passthrough_wasm());

        let text = read(&path);
        assert_eq!(occurrences(&text, "plugin \"rustledger_envelope.wasm\""), 1);

        // The same bytes again write neither the plugin nor the journal.
        let again = budget_write::plugin_install(&state, &journal(&source)).unwrap();
        assert!(again.declared);
        assert_eq!(read(&path), text);

        let status = budget_write::plugin_status(&state);
        assert!(status.declared);
        assert!(status.wasm[0].present);
    }

    #[test]
    fn install_without_a_configuration_copies_the_module_but_declares_nothing() {
        let (dir, path, state) = fixture_without_wasm("budget-plugin-install-bare", LEDGER);
        let before = read(&path);
        let source = dir.seed("downloads/rustledger_envelope.wasm", "");
        fs::write(&source, passthrough_wasm()).unwrap();

        let status = budget_write::plugin_install(&state, &journal(&source)).unwrap();
        // Copying the file is safe; the `plugin` line is not written until
        // there is a `start date` for the plugin to read.
        assert!(!status.declared);
        assert!(status.wasm[0].present);
        assert_eq!(read(&path), before);

        // Setup then lands the block and the declaration together.
        budget_write::init(
            &state,
            &journal(&path),
            "2026-02",
            None,
            None,
            &["Assets:Bank".to_string()],
            true,
            false,
        )
        .unwrap();
        let text = read(&path);
        assert_eq!(occurrences(&text, "plugin \"rustledger_envelope.wasm\""), 1);
        assert!(budget_write::settings(&state).unwrap().plugin_declared);
    }

    #[test]
    fn configuration_writes_refuse_a_missing_wasm_and_write_nothing() {
        let (_dir, path, state) = fixture_without_wasm("budget-config-no-wasm", LEDGER);
        let before = read(&path);
        let error = budget_write::init(
            &state,
            &journal(&path),
            "2026-02",
            None,
            None,
            &["Assets:Bank".to_string()],
            true,
            false,
        )
        .unwrap_err();
        assert!(error.contains("rustledger_envelope.wasm"), "{error}");
        assert!(error.contains("nothing was written"), "{error}");
        assert_eq!(read(&path), before);

        let (_dir, path, state) =
            fixture_without_wasm("budget-config-set-no-wasm", CONFIGURED_WITH_END);
        let before = read(&path);
        let error = budget_write::config_set(
            &state,
            &journal(&path),
            "2026-02",
            None,
            None,
            &["Assets:Bank".to_string()],
            true,
            false,
        )
        .unwrap_err();
        assert!(error.contains("rustledger_envelope.wasm"), "{error}");
        assert!(error.contains("nothing was written"), "{error}");
        assert_eq!(read(&path), before);
    }
}
