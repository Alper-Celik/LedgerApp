// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The chart description shared with the app and the CLI.

use serde::{Deserialize, Serialize};

use crate::decimal::{Decimal, parse as parse_decimal};

/// Text drawn in the middle of a chart that has nothing to plot.
pub const NO_DATA_LABEL: &str = "NO DATA";

/// Which chart to draw.
///
/// Deserialised from the lowercase words in the wire format. Unknown kinds are
/// rejected rather than silently defaulted, so a typo in the app's JSON shows
/// up as an error instead of an empty chart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChartKind {
    /// Points connected by a stroked polyline.
    Line,
    /// One bar per point per series, grouped by category.
    Bar,
    /// A filled band under the line, down to the zero baseline.
    Area,
    /// A ring of slices, one per point.
    Donut,
}

/// One labelled point of a series.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    /// Category label, e.g. `"2026-03"` or `"Assets:Bank"`.
    pub label: String,
    /// Amount as a decimal string, e.g. `"1,234.56"` or `"-12.00"`.
    pub value: String,
}

impl Point {
    /// Parses the value, keeping it exact.
    pub fn decimal(&self) -> Result<Decimal, crate::decimal::DecimalError> {
        parse_decimal(&self.value)
    }
}

/// A named set of points; every series of a chart shares one scale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Series {
    /// Legend label.
    pub name: String,
    /// Points in category order.
    #[serde(default)]
    pub points: Vec<Point>,
}

/// A chart request: the same shape the app emits and the CLI consumes.
///
/// `serde` ignores unknown fields (forward compatible) and defaults every
/// optional part, so `{"kind": "line", "series": []}` is a valid, if empty,
/// chart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChartSpec {
    /// Optional heading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Chart geometry.
    pub kind: ChartKind,
    /// ISO currency code or symbol shown next to totals.
    #[serde(default)]
    pub currency: String,
    /// Optional caption under the horizontal axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_label: Option<String>,
    /// Series in legend order; the drawing palette cycles over them.
    #[serde(default)]
    pub series: Vec<Series>,
}

/// A series whose points are all numeric.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSeries {
    /// Legend label, verbatim.
    pub name: String,
    /// Labels in point order (unparsable points keep their neighbours' order).
    pub labels: Vec<String>,
    /// Parsed values, parallel to `labels`.
    pub values: Vec<Decimal>,
    /// How many points were dropped because their value did not parse.
    pub skipped: usize,
}

impl ParsedSeries {
    /// Number of drawable points.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// `true` when there is nothing to draw for this series.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl ChartSpec {
    /// Parses a chart spec from JSON.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Serialises the spec to compact JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Every point of every series, parsed.
    pub fn parsed_series(&self) -> Vec<ParsedSeries> {
        parsed_series(self)
    }

    /// `true` when nothing can be plotted (no series, or no parsable point).
    pub fn is_empty(&self) -> bool {
        self.series
            .iter()
            .all(|series| series.points.iter().all(|p| p.decimal().is_err()))
    }

    /// Heading, falling back to the axis label and then to the kind name.
    pub fn heading(&self) -> String {
        if let Some(title) = self
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            return title.to_string();
        }
        if let Some(label) = self
            .x_label
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            return label.to_string();
        }
        match self.kind {
            ChartKind::Line => "LINE CHART",
            ChartKind::Bar => "BAR CHART",
            ChartKind::Area => "AREA CHART",
            ChartKind::Donut => "BREAKDOWN",
        }
        .to_string()
    }

    /// The heading plus the currency, when one is set.
    pub fn heading_with_currency(&self) -> String {
        let heading = self.heading();
        let currency = self.currency.trim();
        if currency.is_empty() {
            heading
        } else {
            format!("{heading} ({currency})")
        }
    }
}

/// Parses every point of every series; malformed values are counted and
/// dropped so one bad row cannot blank a whole chart.
pub fn parsed_series(spec: &ChartSpec) -> Vec<ParsedSeries> {
    spec.series
        .iter()
        .map(|series| {
            let mut labels = Vec::with_capacity(series.points.len());
            let mut values = Vec::with_capacity(series.points.len());
            let mut skipped = 0usize;
            for point in &series.points {
                match point.decimal() {
                    Ok(value) => {
                        labels.push(point.label.clone());
                        values.push(value);
                    }
                    Err(_) => skipped += 1,
                }
            }
            ParsedSeries {
                name: series.name.clone(),
                labels,
                values,
                skipped,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ChartSpec {
        ChartSpec::from_json(
            r#"{
                "title": "Net worth",
                "kind": "line",
                "currency": "EUR",
                "x_label": "Month",
                "series": [
                    {"name": "Assets", "points": [
                        {"label": "Jan", "value": "1,000.00"},
                        {"label": "Feb", "value": "1,250.50"}
                    ]},
                    {"name": "Debts", "points": [
                        {"label": "Jan", "value": "-200"},
                        {"label": "Feb", "value": "-150"}
                    ]}
                ]
            }"#,
        )
        .expect("sample spec parses")
    }

    #[test]
    fn deserialises_the_wire_shape() {
        let spec = sample();
        assert_eq!(spec.kind, ChartKind::Line);
        assert_eq!(spec.currency, "EUR");
        assert_eq!(spec.series.len(), 2);
        assert_eq!(spec.series[0].points[1].value, "1,250.50");
        assert_eq!(spec.heading_with_currency(), "Net worth (EUR)");
    }

    #[test]
    fn every_kind_round_trips() {
        for (word, kind) in [
            ("line", ChartKind::Line),
            ("bar", ChartKind::Bar),
            ("area", ChartKind::Area),
            ("donut", ChartKind::Donut),
        ] {
            let json = format!(r#"{{"kind": "{word}", "series": []}}"#);
            let spec = ChartSpec::from_json(&json).unwrap();
            assert_eq!(spec.kind, kind);
            let round_tripped = ChartSpec::from_json(&spec.to_json().unwrap()).unwrap();
            assert_eq!(round_tripped.kind, kind);
            assert_eq!(round_tripped, spec);
        }
    }

    #[test]
    fn unknown_kind_is_an_error() {
        assert!(ChartSpec::from_json(r#"{"kind": "pie", "series": []}"#).is_err());
    }

    #[test]
    fn optional_fields_default() {
        let spec = ChartSpec::from_json(r#"{"kind": "bar"}"#).unwrap();
        assert!(spec.title.is_none());
        assert!(spec.x_label.is_none());
        assert!(spec.currency.is_empty());
        assert!(spec.series.is_empty());
        assert!(spec.is_empty());
        assert_eq!(spec.heading(), "BAR CHART");
        assert_eq!(spec.heading_with_currency(), "BAR CHART");
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let spec = ChartSpec::from_json(r#"{"kind": "line", "future": 1, "series": []}"#).unwrap();
        assert_eq!(spec.kind, ChartKind::Line);
    }

    #[test]
    fn parsed_series_drops_bad_points_only() {
        let spec = ChartSpec::from_json(
            r#"{"kind": "line", "series": [
                {"name": "A", "points": [
                    {"label": "x", "value": "1"},
                    {"label": "y", "value": "nope"},
                    {"label": "z", "value": "3"}
                ]}
            ]}"#,
        )
        .unwrap();
        let parsed = spec.parsed_series();
        assert_eq!(parsed[0].labels, vec!["x", "z"]);
        assert_eq!(
            parsed[0].values,
            vec![Decimal::from_whole(1), Decimal::from_whole(3)]
        );
        assert_eq!(parsed[0].skipped, 1);
        assert_eq!(parsed[0].len(), 2);
        assert!(!spec.is_empty());
    }

    #[test]
    fn empty_points_mean_empty_spec() {
        let spec =
            ChartSpec::from_json(r#"{"kind": "donut", "series": [{"name": "A", "points": []}]}"#)
                .unwrap();
        assert!(spec.is_empty());
    }
}
