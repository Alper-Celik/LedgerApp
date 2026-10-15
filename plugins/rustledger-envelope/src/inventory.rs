// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Per-commodity money.
//!
//! Envelope budgeting here never converts currencies: an envelope holds an
//! inventory of positions, exactly like a beancount account does, and every
//! operation is applied commodity by commodity. Summing across commodities is
//! never meaningful and therefore never offered.

use std::collections::BTreeMap;

use rust_decimal::Decimal;

/// A set of positions keyed by commodity.
///
/// Ordered by commodity so every iteration (and therefore every generated
/// posting and assertion) is deterministic.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inventory {
    positions: BTreeMap<String, Decimal>,
}

impl Inventory {
    /// An empty inventory.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The position in `commodity` (zero when absent).
    #[must_use]
    pub fn get(&self, commodity: &str) -> Decimal {
        self.positions.get(commodity).copied().unwrap_or_default()
    }

    /// Add `amount` to `commodity`, dropping the position when it reaches zero.
    pub fn add(&mut self, commodity: &str, amount: Decimal) {
        let next = self.get(commodity) + amount;
        if next.is_zero() {
            self.positions.remove(commodity);
        } else {
            self.positions.insert(commodity.to_string(), next);
        }
    }

    /// Add every position of `other`.
    pub fn add_inventory(&mut self, other: &Self) {
        for (commodity, amount) in other.iter() {
            self.add(commodity, amount);
        }
    }

    /// This inventory, negated per commodity.
    #[must_use]
    pub fn negated(&self) -> Self {
        let mut out = Self::new();
        for (commodity, amount) in self.iter() {
            out.add(commodity, -amount);
        }
        out
    }

    /// Whether every position is zero (i.e. nothing is held).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// Positions in commodity order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, Decimal)> {
        self.positions
            .iter()
            .map(|(commodity, amount)| (commodity.as_str(), *amount))
    }
}
