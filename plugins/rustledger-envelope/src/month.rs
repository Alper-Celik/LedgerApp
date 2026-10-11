// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Calendar months, as the budget's unit of time.
//!
//! The WASM sandbox has no clock, so nothing here ever consults "today": the
//! budget horizon comes from the ledger (the config's `start date`, an optional
//! `end date`, and the newest directive date), which also keeps the plugin's
//! output stable across runs.

/// A calendar month (`2026-10`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Month {
    /// Gregorian year.
    pub year: i32,
    /// Month of the year, 1-12.
    pub month: u32,
}

impl Month {
    /// Build a month, clamping `month` into 1-12.
    #[must_use]
    pub fn new(year: i32, month: u32) -> Self {
        Self {
            year,
            month: month.clamp(1, 12),
        }
    }

    /// Parse `YYYY-MM`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let (year, month) = text.trim().split_once('-')?;
        if year.len() != 4 || month.len() != 2 {
            return None;
        }
        let year = year.parse::<i32>().ok()?;
        let month = month.parse::<u32>().ok()?;
        if !(1..=12).contains(&month) {
            return None;
        }
        Some(Self { year, month })
    }

    /// Parse the month of a `YYYY-MM-DD` date.
    #[must_use]
    pub fn from_date(text: &str) -> Option<Self> {
        Self::parse(text.get(0..7)?)
    }

    /// Months since year 0, for arithmetic and ordering.
    #[must_use]
    pub fn index(self) -> i32 {
        self.year * 12 + self.month as i32 - 1
    }

    /// The month `index` months since year 0.
    #[must_use]
    pub fn from_index(index: i32) -> Self {
        Self::new(index.div_euclid(12), index.rem_euclid(12) as u32 + 1)
    }

    /// The following month.
    #[must_use]
    pub fn next(self) -> Self {
        Self::from_index(self.index() + 1)
    }

    /// `YYYY-MM`.
    #[must_use]
    pub fn label(self) -> String {
        format!("{:04}-{:02}", self.year, self.month)
    }

    /// `YYYY-MM-01`.
    #[must_use]
    pub fn first_day(self) -> String {
        format!("{}", self.label()) + "-01"
    }

    /// `YYYY-MM-<last day>`.
    #[must_use]
    pub fn last_day(self) -> String {
        format!("{}-{:02}", self.label(), days_in_month(self.year, self.month))
    }
}

/// Days in `month` of `year`, Gregorian.
#[must_use]
pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => unreachable!("month is clamped into 1-12"),
    }
}

/// Whether `year` is a Gregorian leap year.
#[must_use]
pub fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}
