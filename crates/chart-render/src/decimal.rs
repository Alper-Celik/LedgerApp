// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Exact decimal values for chart points.
//!
//! The app hands chart values over as decimal *strings* (money is never a
//! float), so this module parses those strings into a fixed-point
//! [`Decimal`] — an `i128` count of millionths — and only converts to `f64`
//! when a coordinate has to be computed. Values are exact up to six fractional
//! digits (enough for every Beancount commodity in practice); a seventh digit
//! rounds the sixth.
//!
//! Parsing is deliberately forgiving on presentation: leading/trailing
//! whitespace, digit group separators (`,`, `_`, `'`, space), a leading `+`,
//! and an optional `1e6`-style exponent are accepted. Absurdly large magnitudes
//! saturate instead of panicking, so a rogue value can never take a chart down.

use core::fmt;

/// Fractional digits kept by [`Decimal`] (six millionths).
pub const FRACTION_DIGITS: usize = 6;

/// Number of [`Decimal`] micro-units in one whole unit (`10^6`).
pub const SCALE: i128 = 1_000_000;

const SCALE_F64: f64 = 1_000_000.0;

/// A decimal money value stored as an `i128` count of millionths.
///
/// Ordering and equality work on the exact value, which makes it safe to use
/// for sorting a table of amounts before rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Decimal {
    micro: i128,
}

/// Why a decimal string could not be parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecimalError {
    /// The string held no digits at all.
    Empty,
    /// A character that cannot appear in a decimal number.
    NonNumeric(char),
}

impl fmt::Display for DecimalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("decimal value is empty"),
            Self::NonNumeric(c) => write!(f, "unexpected character {c:?} in decimal value"),
        }
    }
}

impl core::error::Error for DecimalError {}

impl Decimal {
    /// Zero.
    pub const ZERO: Self = Self { micro: 0 };

    /// Wraps a raw micro-unit count (`1` == one millionth).
    pub const fn from_micros(micro: i128) -> Self {
        Self { micro }
    }

    /// The raw micro-unit count.
    pub const fn micros(self) -> i128 {
        self.micro
    }

    /// Builds a value from whole units.
    pub const fn from_whole(whole: i128) -> Self {
        Self {
            micro: whole.saturating_mul(SCALE),
        }
    }

    /// `true` when the value is exactly zero.
    pub const fn is_zero(self) -> bool {
        self.micro == 0
    }

    /// `true` when the value is below zero.
    pub const fn is_negative(self) -> bool {
        self.micro < 0
    }

    /// The absolute value; `i128::MIN` saturates.
    pub const fn abs(self) -> Self {
        Self {
            micro: self.micro.saturating_abs(),
        }
    }

    /// The negated value; `i128::MIN` saturates.
    pub const fn neg(self) -> Self {
        Self {
            micro: self.micro.saturating_neg(),
        }
    }

    /// Saturating addition, used to total a series.
    pub const fn saturating_add(self, other: Self) -> Self {
        Self {
            micro: self.micro.saturating_add(other.micro),
        }
    }

    /// The value as `f64`, for geometry only. Saturating values stay finite.
    #[allow(clippy::cast_precision_loss)]
    pub fn to_f64(self) -> f64 {
        self.micro as f64 / SCALE_F64
    }

    /// Plain text with a `-` sign and no group separators (`-1234.5`).
    pub fn to_plain_string(self) -> String {
        self.render(false)
    }

    /// Grouped text with `,` every three integer digits and up to six trimmed
    /// fraction digits (`-1,234,567.5`).
    pub fn to_grouped_string(self) -> String {
        self.render(true)
    }

    fn render(self, grouped: bool) -> String {
        let negative = self.micro < 0;
        let mut micros = self.micro.unsigned_abs();
        let whole = micros / SCALE as u128;
        micros %= SCALE as u128;

        let mut out = String::with_capacity(24);
        if negative {
            out.push('-');
        }

        let digits = whole.to_string();
        if grouped {
            push_grouped(&mut out, &digits);
        } else {
            out.push_str(&digits);
        }

        if micros != 0 {
            let mut frac = format!("{micros:06}");
            while frac.ends_with('0') {
                frac.pop();
            }
            out.push('.');
            out.push_str(&frac);
        }
        out
    }
}

fn push_grouped(out: &mut String, digits: &str) {
    let len = digits.len();
    for (idx, ch) in digits.char_indices() {
        if idx > 0 && (len - idx) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
}

impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_plain_string())
    }
}

/// Parses a decimal string such as `"1,234.56"`, `"-0.005"` or `"1.2e3"`.
///
/// Returns [`DecimalError::Empty`] for a blank string and
/// [`DecimalError::NonNumeric`] for garbage. Values that do not fit in an
/// `i128` of micro-units saturate at the extreme instead of failing.
pub fn parse(input: &str) -> Result<Decimal, DecimalError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(DecimalError::Empty);
    }

    let (negative, body) = match trimmed.as_bytes()[0] {
        b'-' => (true, &trimmed[1..]),
        b'+' => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };

    // Split off an optional exponent.
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(at) => {
            let (head, tail) = body.split_at(at);
            (head, Some(&tail[1..]))
        }
        None => (body, None),
    };

    let mut whole: i128 = 0;
    let mut fraction: i128 = 0;
    let mut fraction_digits = 0usize;
    let mut round_up = false;
    let mut seen_dot = false;
    let mut seen_digit = false;

    for ch in mantissa.chars() {
        match ch {
            '0'..='9' => {
                seen_digit = true;
                let digit = ch as u8 - b'0';
                if seen_dot {
                    if fraction_digits < FRACTION_DIGITS {
                        fraction = fraction * 10 + i128::from(digit);
                        fraction_digits += 1;
                    } else if fraction_digits == FRACTION_DIGITS {
                        round_up = digit >= 5;
                        fraction_digits += 1;
                    }
                } else {
                    whole = whole.saturating_mul(10).saturating_add(i128::from(digit));
                }
            }
            ',' | '_' | '\'' | ' ' => {}
            '.' if !seen_dot => seen_dot = true,
            other => return Err(DecimalError::NonNumeric(other)),
        }
    }

    if !seen_digit {
        return Err(DecimalError::Empty);
    }

    // Pad the fraction to full precision.
    while fraction_digits < FRACTION_DIGITS {
        fraction *= 10;
        fraction_digits += 1;
    }
    if round_up {
        fraction = fraction.saturating_add(1);
    }

    let mut micros = whole.saturating_mul(SCALE).saturating_add(fraction);

    // Apply the exponent, in powers of ten, saturating well before `i128`
    // would overflow so the result stays finite.
    if let Some(exponent) = exponent {
        let exponent = parse_exponent(exponent)?;
        micros = apply_exponent(micros, exponent);
    }

    if negative {
        micros = micros.saturating_neg();
    }

    Ok(Decimal { micro: micros })
}

fn parse_exponent(input: &str) -> Result<i32, DecimalError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(DecimalError::Empty);
    }
    let (negative, digits) = match trimmed.as_bytes()[0] {
        b'-' => (true, &trimmed[1..]),
        b'+' => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };
    let mut value: i32 = 0;
    let mut seen = false;
    for ch in digits.chars() {
        match ch {
            '0'..='9' => {
                seen = true;
                // Cap the exponent; anything past 32 saturates anyway.
                value = (value * 10 + i32::from(ch as u8 - b'0')).min(32);
            }
            ',' | '_' | '\'' | ' ' => {}
            other => return Err(DecimalError::NonNumeric(other)),
        }
    }
    if !seen {
        return Err(DecimalError::Empty);
    }
    Ok(if negative { -value } else { value })
}

fn apply_exponent(micros: i128, exponent: i32) -> i128 {
    if exponent == 0 {
        return micros;
    }
    let magnitude = exponent.unsigned_abs();
    let factor = 10i128.checked_pow(magnitude).unwrap_or(i128::MAX);
    if exponent > 0 {
        micros.saturating_mul(factor)
    } else {
        // Round half away from zero, the rule the mantissa path documents: a
        // bare division would make `5e-7` parse as 0 while `0.0000005` parses as
        // one micro-unit, so the same value would depend on its spelling.
        let divisor = factor.max(1);
        let quotient = micros / divisor;
        let remainder = (micros % divisor).abs();
        if remainder.saturating_mul(2) >= divisor {
            quotient + micros.signum()
        } else {
            quotient
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_integers() {
        assert_eq!(parse("0").unwrap().micros(), 0);
        assert_eq!(parse("42").unwrap().micros(), 42 * SCALE);
        assert_eq!(parse("+7").unwrap(), parse("7").unwrap());
        assert_eq!(parse("-7").unwrap().micros(), -7 * SCALE);
    }

    #[test]
    fn parses_separators_and_fractions() {
        assert_eq!(parse("1,234.5").unwrap(), parse("1234.50").unwrap());
        assert_eq!(parse("1_234.50").unwrap().micros(), 1_234_500_000);
        assert_eq!(parse(" 1 234.5 ").unwrap().micros(), 1_234_500_000);
        assert_eq!(parse("'1234.5").unwrap().micros(), 1_234_500_000);
        assert_eq!(parse(".5").unwrap().micros(), 500_000);
        assert_eq!(parse("-0.000001").unwrap().micros(), -1);
    }

    #[test]
    fn rounds_the_seventh_fraction_digit() {
        assert_eq!(parse("0.0000004").unwrap().micros(), 0);
        assert_eq!(parse("0.0000005").unwrap().micros(), 1);
        assert_eq!(parse("0.1234565").unwrap().micros(), 123_457);
    }

    #[test]
    fn parses_exponents() {
        assert_eq!(parse("1.5e3").unwrap().micros(), 1_500 * SCALE);
        assert_eq!(parse("1.5E-1").unwrap().micros(), 150_000);
        assert_eq!(parse("2e+2").unwrap().micros(), 200 * SCALE);
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse(""), Err(DecimalError::Empty));
        assert_eq!(parse("   "), Err(DecimalError::Empty));
        assert_eq!(parse("abc"), Err(DecimalError::NonNumeric('a')));
        assert_eq!(parse("1.2.3"), Err(DecimalError::NonNumeric('.')));
        assert_eq!(parse("12x"), Err(DecimalError::NonNumeric('x')));
    }

    #[test]
    fn saturates_on_huge_magnitudes() {
        let huge = "99999999999999999999999999999999999999999999";
        let parsed = parse(huge).unwrap();
        assert_eq!(parsed.micros(), i128::MAX);
        assert!(parsed.to_f64().is_finite());
        let negative_huge = parse(&format!("-{huge}")).unwrap();
        assert!(negative_huge.micros() <= i128::MIN + 1);
        assert!(negative_huge.to_f64().is_finite());
    }

    #[test]
    fn formats_with_grouping_and_trimmed_fraction() {
        assert_eq!(
            parse("1234567.89").unwrap().to_grouped_string(),
            "1,234,567.89"
        );
        assert_eq!(parse("-1234.500").unwrap().to_grouped_string(), "-1,234.5");
        assert_eq!(parse("12").unwrap().to_grouped_string(), "12");
        assert_eq!(parse("0.000000").unwrap().to_grouped_string(), "0");
        assert_eq!(parse("123").unwrap().to_plain_string(), "123");
        assert_eq!(
            parse("999999999999999999999999")
                .unwrap()
                .to_grouped_string(),
            "999,999,999,999,999,999,999,999"
        );
        // Beyond the i128 micro-unit range the value saturates, but still
        // formats as a finite, grouped number.
        let saturated = parse("999999999999999999999999999999999")
            .unwrap()
            .to_grouped_string();
        assert!(saturated.starts_with("170,141,183,460,469"), "{saturated}");
    }

    #[test]
    fn orders_by_value() {
        assert!(parse("1.5").unwrap() > parse("1.49").unwrap());
        assert!(parse("-2").unwrap() < parse("-1.99").unwrap());
    }
}
