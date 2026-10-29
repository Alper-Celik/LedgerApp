// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! End-to-end terminal tests: kitty framing and the Unicode fallback.

use chart_render::{
    ChartSpec, KITTY_CHUNK_SIZE, PNG_SIGNATURE, Protocol, RenderOptions, TerminalOptions,
    kitty_sequence, kitty_supported, png_dimensions, render_png, render_terminal_for,
    render_unicode,
};

fn spec(kind: &str) -> ChartSpec {
    ChartSpec::from_json(&format!(
        r#"{{"title": "Monthly cash flow", "kind": "{kind}", "currency": "EUR",
            "x_label": "Month",
            "series": [
                {{"name": "Income", "points": [
                    {{"label": "Jan", "value": "1,000.00"}},
                    {{"label": "Feb", "value": "1,800.50"}},
                    {{"label": "Mar", "value": "1,250"}},
                    {{"label": "Apr", "value": "2,010.75"}}
                ]}},
                {{"name": "Expenses", "points": [
                    {{"label": "Jan", "value": "-900"}},
                    {{"label": "Feb", "value": "-1,150"}},
                    {{"label": "Mar", "value": "-1,320.40"}},
                    {{"label": "Apr", "value": "-870"}}
                ]}}
            ]}}"#
    ))
    .unwrap()
}

#[test]
fn unicode_fallback_fits_the_requested_width() {
    for kind in ["line", "bar", "area", "donut"] {
        for columns in [40, 72, 120] {
            let options = TerminalOptions::sized(columns, 20);
            let text = render_unicode(&spec(kind), &options);
            assert!(!text.is_empty(), "{kind} rendered nothing");
            for line in text.lines() {
                assert!(
                    line.chars().count() <= columns,
                    "{kind} at {columns} cols overflowed: {line:?} ({} cells)",
                    line.chars().count()
                );
            }
        }
    }
}

#[test]
fn unicode_fallback_fits_the_requested_rows() {
    for kind in ["line", "bar", "area", "donut"] {
        for (columns, rows) in [(20, 8), (40, 10), (72, 18), (120, 40)] {
            let text = render_unicode(&spec(kind), &TerminalOptions::sized(columns, rows));
            let lines = text.lines().count();
            assert!(
                lines <= rows.max(8),
                "{kind} at {columns}x{rows} produced {lines} lines:\n{text}"
            );
            for line in text.lines() {
                assert!(
                    line.chars().count() <= columns.max(20),
                    "{kind} at {columns}x{rows}: {line:?}"
                );
            }
        }
    }
}

#[test]
fn unicode_fallback_is_plain_text() {
    let bytes = render_terminal_for(&spec("line"), TerminalOptions::default(), false);
    let text = String::from_utf8(bytes).unwrap();
    assert!(!text.contains('\x1b'), "the fallback must not emit escapes");
    assert!(text.contains("Income"));
    assert!(text.contains("Expenses"));
    assert!(text.contains('●') || text.contains('○'));
}

#[test]
fn auto_selects_the_encoder_from_the_capability_flag() {
    let options = TerminalOptions::default();
    let plain = render_terminal_for(&spec("line"), options.clone(), false);
    let graphics = render_terminal_for(&spec("line"), options.clone(), true);
    assert_ne!(plain, graphics);
    assert!(!plain.starts_with(b"\x1b"));

    let forced_text = TerminalOptions {
        protocol: Protocol::Unicode,
        ..TerminalOptions::default()
    };
    assert!(!render_terminal_for(&spec("line"), forced_text, true).starts_with(b"\x1b"));

    let forced_kitty = TerminalOptions {
        protocol: Protocol::Kitty,
        ..TerminalOptions::default()
    };
    assert!(render_terminal_for(&spec("line"), forced_kitty, false).starts_with(b"\x1b_G"));
}

#[test]
fn kitty_protocol_frames_a_large_png_in_chunks() {
    // A 1600x900 PNG is far beyond one protocol chunk.
    let png = render_png(&spec("line"), RenderOptions::sized(1600, 900));
    assert_eq!(png_dimensions(&png), Some((1600, 900)));
    let sequence = kitty_sequence(&png, Some((100, 30)));
    let text = std::str::from_utf8(&sequence).expect("escape sequence is ASCII");

    let frames = parse_frames(text);
    assert!(
        frames.len() >= 3,
        "expected several chunks, got {}",
        frames.len()
    );

    let mut payload = String::new();
    for (index, frame) in frames.iter().enumerate() {
        assert!(
            frame.payload.len() <= KITTY_CHUNK_SIZE,
            "chunk {index} has {} base64 chars",
            frame.payload.len()
        );
        if index + 1 < frames.len() {
            assert_eq!(frame.more, 1, "chunk {index} should be marked as continued");
        } else {
            assert_eq!(frame.more, 0, "the last chunk must terminate the transfer");
        }
        assert!(frame.starts_with_escape);
        payload.push_str(&frame.payload);
    }

    // Control keys belong to the first chunk only, and must describe the image.
    assert!(frames[0].control.contains("a=T"));
    assert!(frames[0].control.contains("f=100"));
    assert!(frames[0].control.contains("s=1600"));
    assert!(frames[0].control.contains("v=900"));
    assert!(frames[0].control.contains("c=100"));
    assert!(frames[0].control.contains("r=30"));
    for frame in frames.iter().skip(1) {
        assert!(
            !frame.control.contains("a=T"),
            "control keys repeated: {:?}",
            frame.control
        );
        assert!(!frame.control.contains("s="));
    }

    let decoded = base64_decode(&payload);
    assert_eq!(&decoded[..8], &PNG_SIGNATURE);
    assert_eq!(decoded, png, "decoded payload differs from the source PNG");
}

#[test]
fn kitty_sequence_without_cells_omits_them() {
    let png = render_png(&spec("bar"), RenderOptions::sized(400, 240));
    let sequence = kitty_sequence(&png, None);
    let text = std::str::from_utf8(&sequence).unwrap();
    assert!(
        text.starts_with("\x1b_Ga=T,f=100,q=2,s=400,v=240,"),
        "{text:.60}"
    );
    assert!(!text.contains(",c="));
    assert!(text.ends_with("\x1b\\"));
}

#[test]
fn kitty_detection_matches_known_terminals() {
    assert!(kitty_supported(|key| match key {
        "KITTY_WINDOW_ID" => Some("1".into()),
        _ => None,
    }));
    assert!(kitty_supported(|key| match key {
        "KITTY_LISTEN_ON" => Some("unix:/tmp/kitty".into()),
        _ => None,
    }));
    assert!(kitty_supported(|key| match key {
        "TERM" => Some("xterm-kitty".into()),
        _ => None,
    }));
    assert!(kitty_supported(|key| match key {
        "TERM" => Some("xterm-wezterm".into()),
        _ => None,
    }));
    for term in ["xterm-256color", "screen-256color", "linux", "dumb"] {
        assert!(!kitty_supported(|key| match key {
            "TERM" => Some(term.into()),
            _ => None,
        }));
    }
    assert!(!kitty_supported(|_| None));
}

#[test]
fn empty_charts_say_no_data_in_the_terminal_too() {
    let empty = ChartSpec::from_json(r#"{"kind": "bar", "series": []}"#).unwrap();
    let text = render_unicode(&empty, &TerminalOptions::default());
    assert!(text.contains("NO DATA"));
}

#[test]
fn terminal_render_is_deterministic() {
    for kind in ["line", "bar", "area", "donut"] {
        let options = TerminalOptions::sized(60, 16);
        assert_eq!(
            render_unicode(&spec(kind), &options),
            render_unicode(&spec(kind), &options)
        );
    }
}

#[test]
fn huge_values_do_not_wreck_the_terminal_chart() {
    let spec = ChartSpec::from_json(
        r#"{"kind": "line", "series": [{"name": "Big", "points": [
            {"label": "a", "value": "1,234,567,890,123.45"},
            {"label": "b", "value": "-9,876,543,210.99"},
            {"label": "c", "value": "999999999999999999999999999999999999999"}
        ]}]}"#,
    )
    .unwrap();
    let options = TerminalOptions::sized(36, 12);
    let text = render_unicode(&spec, &options);
    for line in text.lines() {
        assert!(line.chars().count() <= 36, "overflow: {line:?}");
    }
    assert!(text.contains('●') || text.contains('▲'));
}

// ---- helpers --------------------------------------------------------------

struct Frame {
    control: String,
    payload: String,
    more: u32,
    starts_with_escape: bool,
}

/// Splits a kitty graphics stream into `ESC _ G <control> ; <payload> ESC \\`
/// frames.
fn parse_frames(text: &str) -> Vec<Frame> {
    let mut frames = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("\x1b_G") {
        let after_escape = &rest[start + 3..];
        let Some(end) = after_escape.find("\x1b\\") else {
            break;
        };
        let body = &after_escape[..end];
        let (control, payload) = body.split_once(';').expect("frame has a payload separator");
        let more = control
            .split(',')
            .find_map(|key| key.strip_prefix("m="))
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        frames.push(Frame {
            control: control.to_string(),
            payload: payload.to_string(),
            more,
            starts_with_escape: start == 0 || rest[..start].ends_with('\n'),
        });
        rest = &after_escape[end + 2..];
    }
    frames
}

fn base64_decode(input: &str) -> Vec<u8> {
    fn value(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = input
        .bytes()
        .filter(|byte| *byte != b'=')
        .filter_map(value)
        .collect();
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let mut buffer = 0u32;
        for (index, byte) in chunk.iter().enumerate() {
            buffer |= u32::from(*byte) << (18 - 6 * index);
        }
        out.push((buffer >> 16) as u8);
        if chunk.len() > 2 {
            out.push((buffer >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(buffer as u8);
        }
    }
    out
}
