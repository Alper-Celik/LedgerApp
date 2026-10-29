// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! End-to-end PNG tests over the public API.

use chart_render::{
    ChartKind, ChartSpec, MAX_DIMENSION, MIN_DIMENSION, NO_DATA_LABEL, PNG_SIGNATURE,
    RenderOptions, png_dimensions, render_png, svg_source,
};

fn spec(kind: &str, series: &[(&str, &[(&str, &str)])]) -> ChartSpec {
    let mut json = format!(
        r#"{{"title": "Net worth", "kind": "{kind}", "currency": "EUR", "x_label": "Month", "series": ["#
    );
    for (index, (name, points)) in series.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str(&format!(r#"{{"name": "{name}", "points": ["#));
        for (point_index, (label, value)) in points.iter().enumerate() {
            if point_index > 0 {
                json.push(',');
            }
            json.push_str(&format!(r#"{{"label": "{label}", "value": "{value}"}}"#));
        }
        json.push_str("]}");
    }
    json.push_str("]}");
    ChartSpec::from_json(&json).expect("generated spec parses")
}

fn sample(kind: &str) -> ChartSpec {
    spec(
        kind,
        &[
            (
                "Assets",
                &[("Jan", "1,000.00"), ("Feb", "1,500.50"), ("Mar", "1,250")],
            ),
            (
                "Debts",
                &[("Jan", "-200"), ("Feb", "-150"), ("Mar", "-320.25")],
            ),
        ],
    )
}

#[test]
fn png_signature_and_ihdr_match_the_requested_size_for_every_kind() {
    for kind in ["line", "bar", "area", "donut"] {
        for (width, height) in [(1200, 720), (640, 400), (64, 64), (4096, 64)] {
            let png = render_png(&sample(kind), RenderOptions::sized(width, height));
            assert_eq!(
                &png[..8],
                &PNG_SIGNATURE,
                "{kind} at {width}x{height} lacks the PNG signature"
            );
            assert_eq!(
                png_dimensions(&png),
                Some((width, height)),
                "{kind} at {width}x{height} has the wrong IHDR size"
            );
            assert!(png.len() > 100, "{kind} produced a suspiciously small file");
        }
    }
}

#[test]
fn kinds_survive_a_json_round_trip() {
    for (word, kind) in [
        ("line", ChartKind::Line),
        ("bar", ChartKind::Bar),
        ("area", ChartKind::Area),
        ("donut", ChartKind::Donut),
    ] {
        let spec = sample(word);
        assert_eq!(spec.kind, kind);
        let json = spec.to_json().unwrap();
        let reparsed = ChartSpec::from_json(&json).unwrap();
        assert_eq!(reparsed, spec);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["kind"], word);
        assert_eq!(value["currency"], "EUR");
        assert_eq!(value["series"][0]["points"][0]["value"], "1,000.00");
    }
}

#[test]
fn rendering_is_deterministic() {
    for kind in ["line", "bar", "area", "donut"] {
        let options = RenderOptions::sized(800, 480);
        let first = render_png(&sample(kind), options.clone());
        let second = render_png(&sample(kind), options.clone());
        assert_eq!(first, second, "{kind} PNG is not byte-identical");
        assert_eq!(
            svg_source(&sample(kind), options.clone()),
            svg_source(&sample(kind), options),
            "{kind} SVG is not identical"
        );
    }
}

#[test]
fn canvas_edges_are_clamped_to_the_supported_range() {
    let png = render_png(&sample("line"), RenderOptions::sized(0, 0));
    assert_eq!(png_dimensions(&png), Some((MIN_DIMENSION, MIN_DIMENSION)));
    let png = render_png(&sample("line"), RenderOptions::sized(u32::MAX, u32::MAX));
    assert_eq!(png_dimensions(&png), Some((MAX_DIMENSION, MAX_DIMENSION)));
}

#[test]
fn empty_and_all_malformed_specs_render_a_placeholder() {
    let no_series = ChartSpec::from_json(r#"{"kind": "line", "series": []}"#).unwrap();
    for kind in ["line", "bar", "area", "donut"] {
        let empty =
            ChartSpec::from_json(&format!(r#"{{"kind": "{kind}", "series": []}}"#)).unwrap();
        let svg = svg_source(&empty, RenderOptions::sized(600, 300));
        assert!(svg.contains(NO_DATA_LABEL), "{kind} lacks the placeholder");
        let png = render_png(&empty, RenderOptions::sized(600, 300));
        assert_eq!(png_dimensions(&png), Some((600, 300)));
    }

    let svg = svg_source(&no_series, RenderOptions::default());
    assert!(svg.contains(NO_DATA_LABEL));

    let malformed = ChartSpec::from_json(
        r#"{"kind": "bar", "series": [{"name": "A", "points": [
            {"label": "x", "value": "not-a-number"},
            {"label": "y", "value": ""}
        ]}]}"#,
    )
    .unwrap();
    assert!(malformed.is_empty());
    assert!(svg_source(&malformed, RenderOptions::default()).contains(NO_DATA_LABEL));
}

#[test]
fn a_zero_total_donut_has_no_slices() {
    let spec = spec("donut", &[("Only", &[("a", "0.00"), ("b", "0")])]);
    let svg = svg_source(&spec, RenderOptions::sized(600, 300));
    assert!(svg.contains(NO_DATA_LABEL));
}

#[test]
fn huge_values_with_separators_do_not_break_the_layout() {
    let spec = spec(
        "line",
        &[
            (
                "Large",
                &[
                    ("Jan", "+1,234,567,890,123.45"),
                    ("Feb", "-9,876,543,210.99"),
                    ("Mar", "999999999999999999999999999999999999999999"),
                    ("Apr", "0.000001"),
                ],
            ),
            (
                "Tiny",
                &[("Jan", "-0.0000005"), ("Feb", "0.0000004"), ("Mar", "1e30")],
            ),
        ],
    );

    for kind in ["line", "bar", "area"] {
        let svg = svg_source(&spec, RenderOptions::sized(900, 500));
        assert!(!svg.contains("NaN"), "{kind} produced NaN coordinates");
        assert!(!svg.contains("inf"), "{kind} produced infinite coordinates");
        assert!(!svg.contains("e+"), "{kind} produced exponent notation");

        // Every coordinate we emit must land inside the canvas (plus a small
        // margin for stroke widths and text overhang at the edges).
        for number in svg_numbers(&svg) {
            assert!(
                (-50.0..=1000.0).contains(&number),
                "{kind}: coordinate {number} escapes the 900x500 canvas"
            );
        }

        let png = render_png(&spec, RenderOptions::sized(900, 500));
        assert_eq!(png_dimensions(&png), Some((900, 500)));
    }

    let donut = render_png(&spec, RenderOptions::sized(700, 420));
    assert_eq!(png_dimensions(&donut), Some((700, 420)));
}

#[test]
fn separators_are_just_presentation() {
    let plain = spec("bar", &[("A", &[("x", "1234.5")])]);
    let grouped = spec("bar", &[("A", &[("x", "1,234.50")])]);
    assert_eq!(
        render_png(&plain, RenderOptions::sized(500, 320)),
        render_png(&grouped, RenderOptions::sized(500, 320))
    );
}

#[test]
fn light_theme_changes_pixels_but_not_geometry() {
    let dark = render_png(&sample("line"), RenderOptions::sized(600, 360));
    let light = render_png(
        &sample("line"),
        RenderOptions {
            width: 600,
            height: 360,
            ..RenderOptions::light()
        },
    );
    assert_ne!(dark, light);
    assert_eq!(png_dimensions(&dark), png_dimensions(&light));
}

#[test]
fn more_series_than_palette_colours_is_fine() {
    let series: Vec<(String, Vec<(String, String)>)> = (0..15)
        .map(|index| {
            (
                format!("Series {index}"),
                vec![
                    ("a".to_string(), index.to_string()),
                    ("b".to_string(), "1".to_string()),
                ],
            )
        })
        .collect();
    let mut json = String::from(r#"{"kind": "line", "series": ["#);
    for (index, (name, points)) in series.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        let points = points
            .iter()
            .map(|(label, value)| format!(r#"{{"label": "{label}", "value": "{value}"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        json.push_str(&format!(r#"{{"name": "{name}", "points": [{points}]}}"#));
    }
    json.push_str("]}");
    let spec = ChartSpec::from_json(&json).unwrap();
    let png = render_png(&spec, RenderOptions::sized(1000, 600));
    assert_eq!(png_dimensions(&png), Some((1000, 600)));
}

/// Extracts every number that appears inside a numeric attribute value in
/// `svg`.
///
/// Element text (the accessible `<desc>`, where labels live) and colour values
/// (`#0f1216`) are skipped: only geometry is interesting here.
fn svg_numbers(svg: &str) -> Vec<f64> {
    // Skip the root element: its `xmlns` URL contains "2000".
    let body = match svg.find('>') {
        Some(at) => &svg[at + 1..],
        None => svg,
    };
    let mut numbers = Vec::new();
    let mut token = String::new();
    let mut in_value = false;
    let mut value_start = false;
    let mut skip_value = false;
    for ch in body.chars() {
        if ch == '"' {
            flush_token(&mut token, &mut numbers);
            in_value = !in_value;
            value_start = in_value;
            skip_value = false;
            continue;
        }
        if !in_value {
            continue;
        }
        if value_start {
            value_start = false;
            skip_value = ch == '#';
            if skip_value {
                continue;
            }
        }
        if skip_value {
            continue;
        }
        if ch.is_ascii_digit() || matches!(ch, '-' | '.' | ' ' | ',') {
            token.push(ch);
        } else {
            flush_token(&mut token, &mut numbers);
        }
    }
    numbers
}

fn flush_token(token: &mut String, numbers: &mut Vec<f64>) {
    for part in token.split([' ', ',']) {
        if let Ok(value) = part.parse::<f64>() {
            if value.is_finite() {
                numbers.push(value);
            }
        }
    }
    token.clear();
}
