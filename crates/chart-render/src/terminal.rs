// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Terminal output: the kitty graphics protocol, with a Unicode fallback.
//!
//! The caller gets raw bytes because only the caller knows where they go
//! (stdout, a pager, a PTY). Nothing here writes to a stream itself.

use std::fmt::Write as _;

use crate::base64;
use crate::png_dimensions;
use crate::render_png;
use crate::spec::{ChartKind, ChartSpec, NO_DATA_LABEL, ParsedSeries};
use crate::style::{MAX_DIMENSION, MIN_DIMENSION, RenderOptions, Theme};
use crate::svg::{Axis, Slice, format_axis_value, nice_axis};

/// Largest base64 payload the kitty protocol accepts per escape sequence.
pub const KITTY_CHUNK_SIZE: usize = 4096;

/// Default terminal width in cells.
pub const DEFAULT_COLUMNS: usize = 72;

/// Default terminal height in cells.
pub const DEFAULT_ROWS: usize = 18;

/// Default assumed cell width in pixels.
pub const DEFAULT_CELL_WIDTH: u32 = 8;

/// Default assumed cell height in pixels.
pub const DEFAULT_CELL_HEIGHT: u32 = 16;

/// Markers used to tell series apart in the Unicode fallback.
const MARKERS: [char; 8] = ['●', '○', '◆', '◇', '▲', '△', '■', '□'];

/// Fill characters for bar and area bodies, one per series.
const FILLS: [char; 4] = ['█', '▓', '▒', '░'];

/// Which encoder to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    /// Kitty when the environment looks like a capable terminal.
    Auto,
    /// Always emit a kitty graphics escape sequence.
    Kitty,
    /// Always emit a Unicode block chart.
    Unicode,
}

/// How to draw into a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalOptions {
    /// Total width in character cells (20..=400).
    pub columns: usize,
    /// Total height in character cells (8..=120).
    pub rows: usize,
    /// Which encoder to use.
    pub protocol: Protocol,
    /// Colours for the raster image; unused by the Unicode fallback.
    pub theme: Theme,
    /// Assumed cell width in pixels, for sizing the kitty image.
    pub cell_width: u32,
    /// Assumed cell height in pixels, for sizing the kitty image.
    pub cell_height: u32,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            columns: DEFAULT_COLUMNS,
            rows: DEFAULT_ROWS,
            protocol: Protocol::Auto,
            theme: Theme::dark(),
            cell_width: DEFAULT_CELL_WIDTH,
            cell_height: DEFAULT_CELL_HEIGHT,
        }
    }
}

impl TerminalOptions {
    /// Defaults with an explicit cell size.
    pub fn sized(columns: usize, rows: usize) -> Self {
        Self {
            columns,
            rows,
            ..Self::default()
        }
    }

    /// The cell size after clamping.
    pub fn cells(&self) -> (usize, usize) {
        (self.columns.clamp(20, 400), self.rows.clamp(8, 120))
    }

    fn normalized(mut self) -> Self {
        let (columns, rows) = self.cells();
        self.columns = columns;
        self.rows = rows;
        self.cell_width = self.cell_width.clamp(4, 40);
        self.cell_height = self.cell_height.clamp(8, 80);
        self
    }
}

/// Renders a chart for the terminal, picking the encoder from the environment.
pub fn render_terminal(spec: &ChartSpec, options: TerminalOptions) -> Vec<u8> {
    render_terminal_for(spec, options, kitty_available())
}

/// Renders a chart for the terminal with the capability answer supplied by the
/// caller, so the choice is explicit and testable.
pub fn render_terminal_for(spec: &ChartSpec, options: TerminalOptions, kitty: bool) -> Vec<u8> {
    let options = options.normalized();
    match options.protocol {
        Protocol::Kitty => render_kitty(spec, &options),
        Protocol::Unicode => render_unicode(spec, &options).into_bytes(),
        Protocol::Auto if kitty => render_kitty(spec, &options),
        Protocol::Auto => render_unicode(spec, &options).into_bytes(),
    }
}

/// `true` when the current process looks like it runs inside a terminal that
/// speaks the kitty graphics protocol.
pub fn kitty_available() -> bool {
    kitty_supported(|key| std::env::var(key).ok())
}

/// Capability probe over an arbitrary environment lookup.
///
/// The kitty protocol is also implemented by Ghostty and WezTerm, which set
/// their own `TERM` values, so those are accepted too.
pub fn kitty_supported(lookup: impl Fn(&str) -> Option<String>) -> bool {
    let set = |key: &str| {
        lookup(key)
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
    };
    if set("KITTY_WINDOW_ID") || set("KITTY_PID") || set("KITTY_LISTEN_ON") {
        return true;
    }
    lookup("TERM")
        .map(|term| {
            let term = term.to_ascii_lowercase();
            term.contains("kitty") || term.contains("ghostty") || term.contains("wezterm")
        })
        .unwrap_or(false)
}

/// Renders the chart to a PNG sized for the terminal and wraps it in a kitty
/// graphics escape sequence.
pub fn render_kitty(spec: &ChartSpec, options: &TerminalOptions) -> Vec<u8> {
    let options = options.clone().normalized();
    let (columns, rows) = (options.columns, options.rows);
    let width = (columns as u32 * options.cell_width).clamp(MIN_DIMENSION, MAX_DIMENSION);
    let height = (rows as u32 * options.cell_height).clamp(MIN_DIMENSION, MAX_DIMENSION);
    let png = render_png(
        spec,
        RenderOptions {
            width,
            height,
            theme: options.theme.clone(),
            legend: true,
            ticks: 4,
        },
    );
    kitty_sequence(&png, Some((columns as u32, rows as u32)))
}

/// Encodes a PNG into kitty graphics protocol escape sequences.
///
/// The payload is split into [`KITTY_CHUNK_SIZE`] base64 characters per
/// sequence; the first sequence carries the control keys (`a=T`, `f=100`, the
/// pixel size, and the display cell size), the rest only the `m` continuation
/// flag. No trailing newline is added.
pub fn kitty_sequence(png: &[u8], cells: Option<(u32, u32)>) -> Vec<u8> {
    let mut control = String::from("a=T,f=100,q=2");
    if let Some((width, height)) = png_dimensions(png) {
        let _ = write!(control, ",s={width},v={height}");
    }
    if let Some((columns, rows)) = cells {
        if columns > 0 && rows > 0 {
            let _ = write!(control, ",c={columns},r={rows}");
        }
    }
    let payload = base64::encode(png);
    let mut out = Vec::with_capacity(payload.len() + 64);
    write_chunks(&mut out, &control, &payload);
    out
}

fn write_chunks(out: &mut Vec<u8>, control: &str, payload: &str) {
    if payload.is_empty() {
        out.extend_from_slice(format!("\x1b_G{control},m=0;\x1b\\").as_bytes());
        return;
    }
    let mut first = true;
    let mut offset = 0;
    while offset < payload.len() {
        let end = (offset + KITTY_CHUNK_SIZE).min(payload.len());
        let chunk = &payload[offset..end];
        let more = u8::from(end < payload.len());
        if first {
            out.extend_from_slice(format!("\x1b_G{control},m={more};{chunk}\x1b\\").as_bytes());
            first = false;
        } else {
            out.extend_from_slice(format!("\x1b_Gm={more};{chunk}\x1b\\").as_bytes());
        }
        offset = end;
    }
}

/// Renders a Unicode block chart, readable in any UTF-8 terminal.
pub fn render_unicode(spec: &ChartSpec, options: &TerminalOptions) -> String {
    let options = options.clone().normalized();
    let columns = options.columns;
    let parsed = spec.parsed_series();
    let drawable: Vec<&ParsedSeries> = parsed.iter().filter(|series| !series.is_empty()).collect();

    let mut out = String::new();
    out.push_str(&truncate(&spec.heading_with_currency(), columns));
    out.push('\n');

    if drawable.is_empty() {
        out.push('\n');
        let label = truncate(NO_DATA_LABEL, columns);
        let pad = columns.saturating_sub(label.chars().count()) / 2;
        out.push_str(&" ".repeat(pad));
        out.push_str(&label);
        out.push('\n');
        return out;
    }

    if matches!(spec.kind, ChartKind::Donut) {
        let slices = crate::svg::donut_slices(&parsed, &options.theme);
        out.push('\n');
        out.push_str(&render_donut_lines(&slices, columns, options.rows));
        return out;
    }

    let legend: Vec<(char, String)> = drawable
        .iter()
        .enumerate()
        .map(|(index, series)| (MARKERS[index % MARKERS.len()], series.name.clone()))
        .collect();
    let caption = spec
        .x_label
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty());

    // Budget the cell grid: heading + legend + blank + plot + axis + labels
    // (+ caption) must fit in `rows`. When it cannot, the legend goes first.
    let footer = 2 + usize::from(caption.is_some());
    let mut legend_rows = legend_lines(&legend, columns);
    let mut plot_rows = options
        .rows
        .saturating_sub(1 + legend_rows.len() + 1 + footer);
    if plot_rows < 3 {
        legend_rows.clear();
        plot_rows = options.rows.saturating_sub(1 + 1 + footer);
    }
    let plot_rows = plot_rows.clamp(3, 40);

    for line in &legend_rows {
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');

    let axis = terminal_axis(&drawable, spec.kind);
    let labels = axis_label_rows(&axis, plot_rows);
    let label_width = labels
        .iter()
        .map(|(_, label)| label.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(3, 9);
    let mut grid = vec![vec![' '; columns.saturating_sub(label_width + 2).max(8)]; plot_rows];
    let plot_cols = grid[0].len();

    for (row, _) in &labels {
        if let Some(line) = grid.get_mut(*row) {
            for cell in line.iter_mut() {
                *cell = '·';
            }
        }
    }

    let max_points = drawable
        .iter()
        .map(|series| series.values.len())
        .max()
        .unwrap_or(1)
        .max(1);
    let kind_fill = matches!(spec.kind, ChartKind::Bar | ChartKind::Area);
    let baseline_row = row_of(0.0, &axis, plot_rows);
    // Later series are drawn first so the first series' bars stay visible.
    for (index, series) in drawable.iter().enumerate().rev() {
        let marker = MARKERS[index % MARKERS.len()];
        let fill = FILLS[index % FILLS.len()];
        for (point, value) in series.values.iter().enumerate() {
            let ratio = if max_points <= 1 {
                0.5
            } else {
                point as f64 / (max_points - 1) as f64
            };
            let column = ((plot_cols.saturating_sub(1)) as f64 * ratio).round() as usize;
            let column = column.min(plot_cols - 1);
            let row = row_of(value.to_f64(), &axis, plot_rows);
            if kind_fill {
                let (from, to) = (row.min(baseline_row), row.max(baseline_row));
                for body_row in from..=to {
                    if let Some(line) = grid.get_mut(body_row) {
                        line[column] = fill;
                    }
                }
            }
            if let Some(line) = grid.get_mut(row) {
                line[column] = marker;
            }
        }
    }

    for row in 0..plot_rows {
        let label = labels
            .iter()
            .find(|(at, _)| *at == row)
            .map(|(_, label)| label.as_str())
            .unwrap_or("");
        let _ = write!(out, "{} ┤", pad_left(label, label_width));
        let line: String = grid[row].iter().collect();
        out.push_str(line.trim_end());
        out.push('\n');
    }

    let _ = write!(
        out,
        "{} └{}",
        " ".repeat(label_width),
        "─".repeat(plot_cols)
    );
    out.push('\n');

    let categories: &[String] = drawable
        .iter()
        .max_by_key(|series| series.labels.len())
        .map(|series| series.labels.as_slice())
        .unwrap_or(&[]);
    let mut axis_labels = vec![' '; plot_cols];
    if let Some(first) = categories.first() {
        for (index, ch) in truncate(first, plot_cols).chars().enumerate() {
            if index < plot_cols {
                axis_labels[index] = ch;
            }
        }
    }
    if let Some(last) = categories.last() {
        let text = truncate(last, plot_cols.saturating_sub(1));
        let width = text.chars().count();
        if width * 2 + 2 <= plot_cols {
            let start = plot_cols - width;
            for (index, ch) in text.chars().enumerate() {
                if start + index < plot_cols {
                    axis_labels[start + index] = ch;
                }
            }
        }
    }
    let _ = write!(
        out,
        "{}  {}",
        " ".repeat(label_width),
        axis_labels.iter().collect::<String>().trim_end()
    );
    out.push('\n');

    if let Some(caption) = caption {
        out.push_str(&truncate(caption, columns));
        out.push('\n');
    }
    out
}

fn render_donut_lines(slices: &[Slice], columns: usize, rows: usize) -> String {
    let mut out = String::new();
    if slices.is_empty() {
        let pad = columns.saturating_sub(NO_DATA_LABEL.len()) / 2;
        out.push_str(&" ".repeat(pad));
        out.push_str(NO_DATA_LABEL);
        out.push('\n');
        return out;
    }
    // Heading and the blank line under it are already spent.
    let budget = rows.saturating_sub(2).max(1);
    let overflow = slices.len() > budget;
    let shown = if overflow {
        budget.saturating_sub(1).max(1)
    } else {
        slices.len()
    };
    let name_width = slices
        .iter()
        .take(shown)
        .map(|slice| truncate(&slice.label, 28).chars().count())
        .max()
        .unwrap_or(0)
        .min(columns / 3);
    let percents: Vec<String> = slices
        .iter()
        .take(shown)
        .map(|slice| format_percent(slice.share))
        .collect();
    let percent_width = percents
        .iter()
        .map(|percent| percent.chars().count())
        .max()
        .unwrap_or(3);
    let bar_width = columns
        .saturating_sub(name_width + percent_width + 3)
        .max(4);
    for (slice, percent) in slices.iter().zip(percents.iter()).take(shown) {
        let name = pad_right(&truncate(&slice.label, name_width), name_width);
        let filled = ((slice.share * bar_width as f64).round() as usize).min(bar_width);
        let _ = writeln!(
            out,
            "{name} {}{} {}",
            "█".repeat(filled),
            "░".repeat(bar_width - filled),
            pad_left(percent, percent_width)
        );
    }
    if slices.len() > shown {
        let _ = writeln!(out, "… {} more", slices.len() - shown);
    }
    debug_assert!(out.lines().count() + 2 <= rows.max(3));
    out
}

fn terminal_axis(series: &[&ParsedSeries], kind: ChartKind) -> Axis {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for series in series {
        for value in &series.values {
            let value = value.to_f64();
            if value.is_finite() {
                min = min.min(value);
                max = max.max(value);
            }
        }
    }
    if !min.is_finite() || !max.is_finite() {
        min = 0.0;
        max = 1.0;
    }
    if matches!(kind, ChartKind::Bar | ChartKind::Area) || (min < 0.0 && max > 0.0) {
        min = min.min(0.0);
        max = max.max(0.0);
    }
    nice_axis(min, max, 4)
}

/// Picks the rows that carry a value label: one per "nice" tick, as long as
/// neighbouring labels keep at least one blank row apart.
fn axis_label_rows(axis: &Axis, plot_rows: usize) -> Vec<(usize, String)> {
    let mut rows: Vec<(usize, String)> = Vec::new();
    for tick in &axis.ticks {
        let row = row_of(*tick, axis, plot_rows);
        if rows
            .last()
            .is_some_and(|(previous, _)| row.abs_diff(*previous) < 2)
        {
            continue;
        }
        rows.push((row, truncate(&format_axis_value(*tick), 9)));
    }
    rows
}

fn row_of(value: f64, axis: &Axis, plot_rows: usize) -> usize {
    let span = axis.max - axis.min;
    if !value.is_finite() || !span.is_finite() || span <= 0.0 {
        return plot_rows / 2;
    }
    let ratio = ((axis.max - value.clamp(axis.min, axis.max)) / span).clamp(0.0, 1.0);
    let row = (ratio * (plot_rows.saturating_sub(1)) as f64).round();
    if row.is_finite() {
        (row as usize).min(plot_rows.saturating_sub(1))
    } else {
        plot_rows / 2
    }
}

fn legend_lines(items: &[(char, String)], columns: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut width = 0usize;
    for (marker, name) in items {
        let piece = format!("{marker} {name}");
        let piece_width = piece.chars().count();
        if width == 0 {
            current = piece;
            width = piece_width;
        } else if width + 2 + piece_width <= columns {
            current.push_str("  ");
            current.push_str(&piece);
            width += 2 + piece_width;
        } else {
            lines.push(truncate(&current, columns));
            current = piece;
            width = piece_width;
        }
    }
    if width > 0 {
        lines.push(truncate(&current, columns));
    }
    lines
}

/// One decimal place, but never a trailing `.0`.
fn format_percent(share: f64) -> String {
    if !share.is_finite() {
        return "0%".to_string();
    }
    let value = (share * 1000.0).round() / 10.0;
    if (value - value.round()).abs() < f64::EPSILON {
        format!("{}%", value.round() as i64)
    } else {
        format!("{value:.1}%")
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    if max <= 1 {
        return "…".chars().take(max).collect();
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push('…');
    out
}

fn pad_left(text: &str, width: usize) -> String {
    let len = text.chars().count();
    format!("{}{text}", " ".repeat(width.saturating_sub(len)))
}

fn pad_right(text: &str, width: usize) -> String {
    let len = text.chars().count();
    format!("{text}{}", " ".repeat(width.saturating_sub(len)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::MIN_DIMENSION;

    fn spec(kind: &str) -> ChartSpec {
        ChartSpec::from_json(&format!(
            r#"{{"title": "Net worth", "kind": "{kind}", "currency": "EUR",
                "x_label": "Month",
                "series": [
                    {{"name": "Assets", "points": [
                        {{"label": "Jan", "value": "1,000.00"}},
                        {{"label": "Feb", "value": "1,500.50"}},
                        {{"label": "Mar", "value": "1,250"}}
                    ]}},
                    {{"name": "Debts", "points": [
                        {{"label": "Jan", "value": "-200"}},
                        {{"label": "Feb", "value": "-150"}},
                        {{"label": "Mar", "value": "-320"}}
                    ]}}
                ]}}"#
        ))
        .unwrap()
    }

    #[test]
    fn kitty_detection_uses_the_environment() {
        assert!(kitty_supported(|key| match key {
            "KITTY_WINDOW_ID" => Some("1".to_string()),
            _ => None,
        }));
        assert!(kitty_supported(|key| match key {
            "TERM" => Some("xterm-kitty".to_string()),
            _ => None,
        }));
        assert!(kitty_supported(|key| match key {
            "TERM" => Some("xterm-ghostty".to_string()),
            _ => None,
        }));
        assert!(!kitty_supported(|key| match key {
            "TERM" => Some("xterm-256color".to_string()),
            _ => None,
        }));
        assert!(!kitty_supported(|_| None));
        assert!(!kitty_supported(|key| match key {
            "KITTY_WINDOW_ID" => Some("   ".to_string()),
            _ => None,
        }));
    }

    #[test]
    fn chunker_splits_at_the_protocol_limit() {
        let payload = "A".repeat(KITTY_CHUNK_SIZE * 2 + 7);
        let mut out = Vec::new();
        write_chunks(&mut out, "a=T,f=100", &payload);
        let text = String::from_utf8(out).unwrap();
        let frames: Vec<&str> = text.split("\x1b\\").filter(|f| !f.is_empty()).collect();
        assert_eq!(frames.len(), 3);
        for frame in &frames[..2] {
            assert!(frame.contains("m=1;"));
            let body = frame.rsplit_once(';').unwrap().1;
            assert_eq!(body.len(), KITTY_CHUNK_SIZE);
            assert!(frame.starts_with("\x1b_G"));
        }
        let last = frames[2];
        assert!(last.contains("m=0;"));
        assert_eq!(last.rsplit_once(';').unwrap().1.len(), 7);
        // Control keys only appear on the first chunk.
        assert!(frames[0].contains("a=T,f=100"));
        assert!(!frames[1].contains("a=T"));
        let joined: String = frames
            .iter()
            .map(|frame| frame.rsplit_once(';').unwrap().1)
            .collect();
        assert_eq!(joined, payload);
    }

    #[test]
    fn kitty_sequence_encodes_a_png() {
        let png = crate::render_png(&spec("line"), RenderOptions::sized(640, 360));
        let sequence = kitty_sequence(&png, Some((80, 24)));
        let text = String::from_utf8(sequence).unwrap();
        assert!(text.starts_with("\x1b_Ga=T,f=100,q=2,s=640,v=360,c=80,r=24,"));
        assert!(text.ends_with("\x1b\\"));
        let (_, payload) = text.split_once(';').unwrap();
        let payload = payload.trim_end_matches("\x1b\\");
        assert!(
            payload.len() > KITTY_CHUNK_SIZE,
            "test needs multiple chunks"
        );
        assert!(
            payload.starts_with("iVBORw0KGgo"),
            "PNG payload must be base64"
        );
    }

    #[test]
    fn unicode_chart_respects_the_requested_width() {
        for kind in ["line", "bar", "area"] {
            let options = TerminalOptions::sized(40, 12);
            let text = render_unicode(&spec(kind), &options);
            for line in text.lines() {
                assert!(
                    line.chars().count() <= 40,
                    "{kind}: line wider than 40 cells: {line:?}"
                );
            }
            assert!(text.contains("Assets"));
            assert!(text.contains('┤'));
            assert!(text.contains('└'));
        }
    }

    #[test]
    fn unicode_charts_are_deterministic_and_never_panic_on_tiny_sizes() {
        for (columns, rows) in [(20, 6), (22, 7), (200, 120), (400, 120)] {
            let options = TerminalOptions::sized(columns, rows);
            let text = render_unicode(&spec("bar"), &options);
            assert!(!text.is_empty());
            assert_eq!(text, render_unicode(&spec("bar"), &options));
            for line in text.lines() {
                assert!(line.chars().count() <= columns.max(20));
            }
        }
    }

    #[test]
    fn donut_renders_a_proportion_bar() {
        let text = render_unicode(&spec("donut"), &TerminalOptions::sized(60, 14));
        assert!(text.contains('█'));
        assert!(text.contains('%'));
    }

    #[test]
    fn empty_spec_says_no_data() {
        let empty = ChartSpec::from_json(r#"{"kind": "line", "series": []}"#).unwrap();
        let text = render_unicode(&empty, &TerminalOptions::default());
        assert!(text.contains(NO_DATA_LABEL));
        let donut =
            ChartSpec::from_json(r#"{"kind": "donut", "series": [{"name": "a", "points": []}]}"#)
                .unwrap();
        assert!(render_unicode(&donut, &TerminalOptions::default()).contains(NO_DATA_LABEL));
    }

    #[test]
    fn auto_falls_back_without_kitty_support() {
        let text = render_terminal_for(&spec("line"), TerminalOptions::default(), false);
        let text = String::from_utf8(text).unwrap();
        assert!(!text.contains('\x1b'));
        assert!(text.contains("NET WORTH") || text.contains("Net worth"));
    }

    #[test]
    fn kitty_output_is_a_sequence_not_text() {
        let bytes = render_terminal_for(&spec("line"), TerminalOptions::default(), true);
        assert!(bytes.starts_with(b"\x1b_G"));
        assert!(bytes.ends_with(b"\x1b\\"));
        let png_start = bytes
            .windows(8)
            .position(|window| window == [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        assert!(png_start.is_none(), "PNG must be base64 encoded, not raw");
    }

    #[test]
    fn kitty_image_is_sized_to_the_cell_grid() {
        let options = TerminalOptions::sized(50, 20);
        let bytes = render_kitty(&spec("bar"), &options);
        let text = String::from_utf8(bytes).unwrap();
        let control = text.trim_end_matches("\x1b\\").rsplit_once(';').unwrap().0;
        assert!(control.contains("c=50,r=20"), "{control}");
        let expected_w = 50 * DEFAULT_CELL_WIDTH;
        let expected_h = 20 * DEFAULT_CELL_HEIGHT;
        assert!(
            control.contains(&format!("s={expected_w},v={expected_h}")),
            "{control}"
        );
        assert!((expected_w as u32) >= MIN_DIMENSION);
    }
}
