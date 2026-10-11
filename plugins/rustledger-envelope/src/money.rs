// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A single amount: decimal value plus commodity.
//!
//! Amounts cross the plugin boundary as strings (see
//! [`rustledger_plugin_types::AmountData`]), and every envelope operation keeps
//! the commodity it came with — no conversion, ever.

use std::str::FromStr;

use rust_decimal::Decimal;
use rustledger_plugin_types::AmountData;

/// A decimal value in one commodity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Money {
    /// The value.
    pub value: Decimal,
    /// The commodity (e.g. `TRY`, `USD`).
    pub currency: String,
}

impl Money {
    /// Build an amount.
    #[must_use]
    pub fn new(value: Decimal, currency: impl Into<String>) -> Self {
        Self {
            value,
            currency: currency.into(),
        }
    }

    /// Parse a wire amount (`AmountData`), as the host sends it.
    #[must_use]
    pub fn from_amount_data(data: &AmountData) -> Option<Self> {
        let value = Decimal::from_str(data.number.trim()).ok()?;
        Some(Self::new(value, data.currency.clone()))
    }

    /// Whether the value is zero.
    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.value.is_zero()
    }

    /// The wire shape of this amount.
    ///
    /// The number is written without trailing zeros beyond the second decimal,
    /// so a cost-derived value like `1500.0000` reads as `1500.00` and integer
    /// arithmetic does not leak its scale into the generated ledger text.
    #[must_use]
    pub fn to_amount_data(&self) -> AmountData {
        let text = self.value.normalize().to_string();
        let number = match text.split_once('.') {
            Some((whole, fraction)) if fraction.len() < 2 => format!("{whole}.{fraction:0<2}"),
            Some(_) => text,
            None => format!("{text}.00"),
        };
        AmountData {
            number,
            currency: self.currency.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn wire_amounts_round_trip_and_reject_nonsense() {
        let parsed = Money::from_amount_data(&AmountData {
            number: " 1500.0000 ".to_string(),
            currency: "USD".to_string(),
        })
        .expect("a well-formed wire amount parses");
        assert_eq!(parsed.value, Decimal::from_str("1500.00").expect("parses"));
        assert_eq!(parsed.currency, "USD");

        // Integer arithmetic must not leak its scale into the ledger text.
        assert_eq!(parsed.to_amount_data().number, "1500.00");
        assert_eq!(
            Money::new(Decimal::from_str("4000.00").expect("parses"), "TRY")
                .to_amount_data()
                .number,
            "4000.00"
        );
        assert_eq!(
            Money::new(Decimal::ZERO, "TRY").to_amount_data().number,
            "0.00"
        );
        assert_eq!(
            Money::new(Decimal::from_str("0.5").expect("parses"), "TRY")
                .to_amount_data()
                .number,
            "0.50"
        );
        assert_eq!(
            Money::new(Decimal::from_str("-0.125").expect("parses"), "TRY")
                .to_amount_data()
                .number,
            "-0.125"
        );

        assert!(
            Money::from_amount_data(&AmountData {
                number: "not a number".to_string(),
                currency: "USD".to_string(),
            })
            .is_none()
        );
    }
}
