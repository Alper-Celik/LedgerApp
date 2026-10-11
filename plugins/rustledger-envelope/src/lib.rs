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
//! envelopes need no price map and no currency conversion. The Late validation
//! pass still runs after this plugin, so the generated entries are checked for
//! balance and currency constraints like any other transaction.
//!
//! # Accounting shape
//!
//! Every generated posting stays inside `Equity:Envelope:`, so the namespace is
//! self-contained (`Σ Equity:Envelope:* = 0`), net-worth neutral, and inert for
//! every other consumer of the ledger.
//!
//! # Status
//!
//! ABI wiring only: [`process`] currently passes the directive stream through
//! unchanged. The configuration parser, the monthly model and the generated
//! entries land next.

use rustledger_plugin_types::{PluginInput, PluginOutput, wasm_plugin_main};

/// Entry point: turns the host's directive stream into an op list.
fn process(input: PluginInput) -> PluginOutput {
    PluginOutput::passthrough(input.directives.len())
}

wasm_plugin_main! {
    process: process,
}
