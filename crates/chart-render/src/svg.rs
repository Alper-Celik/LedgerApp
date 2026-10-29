// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Builds the SVG document for a chart.
//!
//! Everything is expressed as plain shapes (`rect`, `line`, `path`, `circle`),
//! including the text: see [`crate::font`]. That keeps the rasteriser
//! font-free and the output deterministic.
//!
//! Because text is drawn as outlines, the document would otherwise be opaque to
//! a screen reader (or a test): every drawn string is therefore also recorded
//! in the document's `<desc>` element.

use std::fmt::Write as _;

use crate::font;
use crate::spec::{ChartKind, ChartSpec, NO_DATA_LABEL, ParsedSeries, parsed_series};
use crate::style::{Color, RenderOptions, Theme};

/// Series beyond this count are folded into one "other" slice on a donut.
const MAX_SLICES: usize = 12;

const TITLE_SCALE: f64 = 3.0;
const LABEL_SCALE: f64 = 2.0;

/// A hard ceiling on drawn ticks, whatever the caller asks for.
const MAX_TICKS: usize = 24;

/// How many characters of drawn text are recorded in `<desc>`.
const MAX_DESCRIBED_TEXT: usize = 2000;

/// One donut slice.
#[derive(Debug, Clone)]
pub(crate) struct Slice {
    pub(crate) label: String,
    pub(crate) value: f64,
    pub(crate) share: f64,
    pub(crate) color: Color,
}

/// The value axis: a padded domain plus evenly spaced tick values.
#[derive(Debug, Clone)]
pub(crate) struct Axis {
    pub(crate) min: f64,
    pub(crate) max: f64,
    pub(crate) ticks: Vec<f64>,
}

/// Renders `spec` into an SVG document of exactly `options` canvas size.
pub(crate) fn build_svg(spec: &ChartSpec, options: &RenderOptions) -> String {
    let options = options.clone().normalized();
    let theme = &options.theme;
    let canvas_w = f64::from(options.width);
    let canvas_h = f64::from(options.height);
    let pad = (canvas_w.min(canvas_h) * 0.03).clamp(6.0, 24.0);
    let narrow = canvas_w < 320.0;

    let series = parsed_series(spec);
    let drawable: Vec<&ParsedSeries> = series.iter().filter(|s| !s.is_empty()).collect();
    let has_points = !drawable.is_empty();

    let slices = if matches!(spec.kind, ChartKind::Donut) && has_points {
        donut_slices(&series, theme)
    } else {
        Vec::new()
    };
    let donut_total: f64 = slices.iter().map(|slice| slice.value).sum();
    let empty = !has_points || (matches!(spec.kind, ChartKind::Donut) && slices.is_empty());

    let mut painter = Painter::new();
    let _ = write!(
        painter.out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{0}\" height=\"{1}\" viewBox=\"0 0 {0} {1}\">",
        num(canvas_w),
        num(canvas_h)
    );
    painter.rect(0.0, 0.0, canvas_w, canvas_h, theme.background);
    // Reserved for the accessible description, filled in once the text is known.
    painter.out.push_str("<!--DESC-->");

    // ---- header: title + legend -------------------------------------------
    let title_scale = if narrow { 2.0 } else { TITLE_SCALE };
    let title = spec
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty());
    let mut cursor = pad;
    let title_y = cursor;
    if title.is_some() {
        cursor += font::line_height(title_scale) + 16.0;
    }

    let legend_entries = if options.legend {
        legend_items(
            &series,
            &slices,
            theme,
            matches!(spec.kind, ChartKind::Donut),
        )
    } else {
        Vec::new()
    };
    let legend_scale = if narrow { 1.0 } else { LABEL_SCALE };
    let legend_rows = layout_legend(&legend_entries, canvas_w, pad, legend_scale);
    let legend_y = cursor;
    let legend_row_height = font::line_height(legend_scale) + 10.0;
    if !legend_rows.is_empty() {
        cursor += legend_rows.len() as f64 * legend_row_height + 14.0;
    }

    if let Some(title) = title {
        painter.text(pad, title_y, title, title_scale, theme.foreground);
    }
    for (row, items) in legend_rows.iter().enumerate() {
        let y = legend_y + row as f64 * legend_row_height;
        let swatch_y = y + (font::line_height(legend_scale) - 10.0) / 2.0;
        for (x, text, color) in items {
            painter.rect(*x, swatch_y, 10.0, 10.0, *color);
            painter.text(x + 16.0, y, text, legend_scale, theme.muted);
        }
    }

    // ---- value axis --------------------------------------------------------
    let axis = if empty {
        Axis {
            min: 0.0,
            max: 1.0,
            ticks: vec![0.0, 0.5, 1.0],
        }
    } else {
        value_axis(&drawable, spec.kind, options.ticks)
    };
    let tick_labels: Vec<String> = axis.ticks.iter().map(|t| format_axis_value(*t)).collect();
    let y_label_width = if matches!(spec.kind, ChartKind::Donut) {
        0.0
    } else {
        tick_labels
            .iter()
            .map(|label| measure(label, LABEL_SCALE))
            .fold(0.0_f64, f64::max)
            .min((canvas_w - 2.0 * pad) * 0.35)
    };

    let categories: Vec<String> = drawable
        .iter()
        .max_by_key(|s| s.labels.len())
        .map(|s| s.labels.clone())
        .unwrap_or_default();
    let has_categories = !categories.is_empty();
    let show_x_labels = has_categories && !matches!(spec.kind, ChartKind::Donut);

    let x_label_height = if show_x_labels {
        font::line_height(LABEL_SCALE) + 12.0
    } else {
        0.0
    };
    let caption = spec
        .x_label
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty());
    let caption_height = if caption.is_some() {
        font::line_height(LABEL_SCALE) + 8.0
    } else {
        0.0
    };

    let mut plot_left = pad + y_label_width + if y_label_width > 0.0 { 16.0 } else { 0.0 };
    let plot_right = canvas_w - pad;
    if plot_right - plot_left < 24.0 {
        plot_left = pad;
    }
    let mut plot_top = cursor.max(pad + 4.0);
    let mut plot_bottom = canvas_h - pad - x_label_height - caption_height;
    if plot_bottom - plot_top < 24.0 {
        plot_top = (pad + 4.0).min(canvas_h * 0.4);
        plot_bottom = (canvas_h - pad - x_label_height - caption_height).max(plot_top + 8.0);
    }
    let plot_width = (plot_right - plot_left).max(1.0);
    let plot_height = (plot_bottom - plot_top).max(1.0);
    let show_y_labels = y_label_width > 0.0 && plot_left > pad + y_label_width;

    let map_y = |value: f64| -> f64 { map_y(value, &axis, plot_top, plot_height) };

    // ---- grid + axes ------------------------------------------------------
    if !matches!(spec.kind, ChartKind::Donut) {
        for (tick, label) in axis.ticks.iter().zip(tick_labels.iter()) {
            let y = map_y(*tick);
            painter.line(plot_left, y, plot_right, y, theme.grid, 1.0);
            if show_y_labels {
                let width = measure(label, LABEL_SCALE);
                painter.text(
                    plot_left - 12.0 - width,
                    y - font::line_height(LABEL_SCALE) / 2.0,
                    label,
                    LABEL_SCALE,
                    theme.muted,
                );
                painter.line(plot_left - 5.0, y, plot_left, y, theme.axis, 1.0);
            }
        }
        painter.line(plot_left, plot_top, plot_left, plot_bottom, theme.axis, 1.0);
        painter.line(
            plot_left,
            plot_bottom,
            plot_right,
            plot_bottom,
            theme.axis,
            1.0,
        );
        // Emphasise the zero baseline when the domain straddles it.
        if axis.min < 0.0 && axis.max > 0.0 {
            let zero = map_y(0.0);
            painter.line(plot_left, zero, plot_right, zero, theme.axis, 2.0);
        }
    }

    // ---- data -------------------------------------------------------------
    let _ = write!(
        painter.out,
        "<defs><clipPath id=\"plot-clip\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/></clipPath></defs>",
        num(plot_left),
        num(plot_top),
        num(plot_width),
        num(plot_height)
    );
    painter.out.push_str("<g clip-path=\"url(#plot-clip)\">");

    if empty {
        let scale = if plot_width > 220.0 { 3.0 } else { 2.0 };
        let text = fit_text(NO_DATA_LABEL, plot_width - 8.0, scale);
        let width = measure(&text, scale);
        painter.text(
            plot_left + (plot_width - width) / 2.0,
            plot_top + (plot_height - font::line_height(scale)) / 2.0,
            &text,
            scale,
            theme.muted,
        );
    } else {
        match spec.kind {
            ChartKind::Line | ChartKind::Area => {
                let max_points = drawable
                    .iter()
                    .map(|s| s.values.len())
                    .max()
                    .unwrap_or(1)
                    .max(1);
                for (index, series) in drawable.iter().enumerate() {
                    draw_line_series(
                        &mut painter,
                        series,
                        theme.series_color(index),
                        spec.kind,
                        max_points,
                        plot_left,
                        plot_width,
                        &map_y,
                    );
                }
            }
            ChartKind::Bar => {
                let max_points = drawable
                    .iter()
                    .map(|s| s.values.len())
                    .max()
                    .unwrap_or(1)
                    .max(1);
                let series_count = drawable.len().max(1);
                for (index, series) in drawable.iter().enumerate() {
                    draw_bars(
                        &mut painter,
                        series,
                        theme.series_color(index),
                        max_points,
                        series_count,
                        index,
                        plot_left,
                        plot_width,
                        &map_y,
                    );
                }
            }
            ChartKind::Donut => draw_donut(
                &mut painter,
                &slices,
                donut_total,
                theme,
                spec,
                plot_left,
                plot_top,
                plot_width,
                plot_height,
            ),
        }
    }
    painter.out.push_str("</g>");

    // ---- captions ---------------------------------------------------------
    if show_x_labels {
        let slot = plot_width / categories.len().max(1) as f64;
        let label_y = plot_bottom + 10.0;
        let sample = categories.first().map(String::as_str).unwrap_or("");
        let needed = measure(&fit_text(sample, slot - 8.0, LABEL_SCALE), LABEL_SCALE) + 14.0;
        let stride = ((needed / slot.max(1.0)).ceil() as usize).max(1);
        for (index, label) in categories.iter().enumerate().step_by(stride) {
            let center = plot_left + (index as f64 + 0.5) * slot;
            let text = fit_text(label, (slot * stride as f64 - 10.0).max(8.0), LABEL_SCALE);
            let width = measure(&text, LABEL_SCALE);
            let mut x = center - width / 2.0;
            let limit = (canvas_w - pad - width).max(pad);
            if !x.is_finite() {
                x = pad;
            }
            x = x.clamp(pad, limit);
            painter.text(x, label_y, &text, LABEL_SCALE, theme.muted);
        }
    }
    if let Some(caption) = caption {
        let text = fit_text(caption, canvas_w - 2.0 * pad, LABEL_SCALE);
        let width = measure(&text, LABEL_SCALE);
        let y = canvas_h - pad - font::line_height(LABEL_SCALE);
        painter.text(
            ((canvas_w - width) / 2.0).max(pad),
            y,
            &text,
            LABEL_SCALE,
            theme.muted,
        );
    }

    painter.out.push_str("</svg>");

    let described = describe(&spec.heading_with_currency(), &painter.texts);
    painter.out.replace("<!--DESC-->", &described)
}

/// Builds the `<title>`/`<desc>` metadata for the document.
fn describe(heading: &str, texts: &[String]) -> String {
    let mut joined = String::new();
    for text in texts {
        if joined.len() >= MAX_DESCRIBED_TEXT {
            break;
        }
        if !joined.is_empty() {
            joined.push_str(" | ");
        }
        joined.push_str(text);
    }
    if joined.len() > MAX_DESCRIBED_TEXT {
        let mut cut = MAX_DESCRIBED_TEXT;
        while cut > 0 && !joined.is_char_boundary(cut) {
            cut -= 1;
        }
        joined.truncate(cut);
    }
    format!(
        "<title>{}</title><desc>{}</desc>",
        escape_xml(heading),
        escape_xml(&joined)
    )
}

fn legend_items(
    series: &[ParsedSeries],
    slices: &[Slice],
    theme: &Theme,
    donut: bool,
) -> Vec<(String, Color)> {
    if donut {
        return slices
            .iter()
            .map(|slice| {
                (
                    format!("{} {}%", slice.label, trim_number(slice.share * 100.0, 0)),
                    slice.color,
                )
            })
            .collect();
    }
    series
        .iter()
        .enumerate()
        .filter(|(_, series)| !series.is_empty())
        .map(|(index, series)| (series.name.clone(), theme.series_color(index)))
        .collect()
}

/// Places legend entries left to right, wrapping at the canvas edge.
fn layout_legend(
    items: &[(String, Color)],
    canvas_w: f64,
    pad: f64,
    scale: f64,
) -> Vec<Vec<(f64, String, Color)>> {
    let mut rows: Vec<Vec<(f64, String, Color)>> = Vec::new();
    let avail = (canvas_w - 2.0 * pad).max(10.0);
    let mut row: Vec<(f64, String, Color)> = Vec::new();
    let mut x = pad;
    for (label, color) in items {
        if x <= pad {
            x = pad;
        }
        let text = fit_text(label, (avail - 26.0).max(16.0), scale);
        let width = 16.0 + measure(&text, scale);
        if x + width > pad + avail && !row.is_empty() {
            rows.push(std::mem::take(&mut row));
            x = pad;
        }
        row.push((x, text, *color));
        x += width + 20.0;
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

/// Computes the value axis for the given series.
fn value_axis(series: &[&ParsedSeries], kind: ChartKind, ticks: usize) -> Axis {
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
    // Bars and areas are read against the zero baseline.
    if matches!(kind, ChartKind::Bar | ChartKind::Area) || (min < 0.0 && max > 0.0) {
        min = min.min(0.0);
        max = max.max(0.0);
    }
    nice_axis(min, max, ticks)
}

/// Rounds a domain outwards to a "nice" step and returns the tick values.
pub(crate) fn nice_axis(min: f64, mut max: f64, target: usize) -> Axis {
    let target = target.clamp(2, 10);
    let mut min = min;
    if !min.is_finite() || !max.is_finite() {
        min = 0.0;
        max = 1.0;
    }
    if max < min {
        std::mem::swap(&mut min, &mut max);
    }
    // A flat series still needs a window to be drawn in.
    if max - min <= f64::EPSILON * min.abs().max(1.0) {
        let magnitude = min.abs().max(1.0);
        min -= magnitude;
        max += magnitude;
    }

    let mut step = nice_step((max - min) / target as f64);
    if !step.is_finite() || step <= 0.0 {
        step = (max - min) / target as f64;
    }
    if !step.is_finite() || step <= 0.0 {
        return Axis {
            min: 0.0,
            max: 1.0,
            ticks: vec![0.0, 0.5, 1.0],
        };
    }

    let lo = (min / step).floor() * step;
    let hi = (max / step).ceil() * step;
    if !lo.is_finite() || !hi.is_finite() || hi <= lo {
        return Axis {
            min: 0.0,
            max: 1.0,
            ticks: vec![0.0, 0.5, 1.0],
        };
    }

    let count = (((hi - lo) / step).round() as i64).clamp(1, MAX_TICKS as i64);
    let ticks = (0..=count).map(|i| lo + i as f64 * step).collect();
    Axis {
        min: lo,
        max: hi,
        ticks,
    }
}

fn nice_step(raw: f64) -> f64 {
    if !raw.is_finite() || raw <= 0.0 {
        return 1.0;
    }
    let exponent = raw.log10().floor();
    let base = 10f64.powf(exponent);
    if !base.is_finite() || base <= 0.0 {
        return 1.0;
    }
    let normalized = raw / base;
    let multiple = if normalized <= 1.0 {
        1.0
    } else if normalized <= 2.0 {
        2.0
    } else if normalized <= 2.5 {
        2.5
    } else if normalized <= 5.0 {
        5.0
    } else {
        10.0
    };
    let step = multiple * base;
    if step.is_finite() && step > 0.0 {
        step
    } else {
        1.0
    }
}

fn map_y(value: f64, axis: &Axis, top: f64, height: f64) -> f64 {
    if !value.is_finite() {
        return top + height / 2.0;
    }
    let span = axis.max - axis.min;
    if !span.is_finite() || span <= 0.0 {
        return top + height / 2.0;
    }
    let value = value.clamp(axis.min, axis.max);
    let ratio = (value - axis.min) / span;
    if !ratio.is_finite() {
        return top + height / 2.0;
    }
    top + height * (1.0 - ratio)
}

pub(crate) fn donut_slices(series: &[ParsedSeries], theme: &Theme) -> Vec<Slice> {
    let multiple_series = series.iter().filter(|s| !s.is_empty()).count() > 1;
    let mut entries: Vec<(String, f64)> = Vec::new();
    for series in series {
        for (index, value) in series.values.iter().enumerate() {
            let magnitude = value.abs().to_f64();
            if !(magnitude > 0.0) || !magnitude.is_finite() {
                continue;
            }
            let label = match series.labels.get(index) {
                Some(label) if !label.trim().is_empty() => {
                    if multiple_series {
                        format!("{}: {}", series.name, label)
                    } else {
                        label.clone()
                    }
                }
                _ => series.name.clone(),
            };
            entries.push((label, magnitude));
        }
    }
    if entries.is_empty() {
        return Vec::new();
    }
    // Biggest slices first, everything past MAX_SLICES folded into one.
    entries.sort_by(|a, b| b.1.total_cmp(&a.1));
    if entries.len() > MAX_SLICES {
        let tail = entries.split_off(MAX_SLICES - 1);
        let rest: f64 = tail.iter().map(|(_, value)| *value).sum();
        entries.push(("Other".to_string(), rest));
    }
    let total: f64 = entries.iter().map(|(_, value)| *value).sum();
    if !(total > 0.0) || !total.is_finite() {
        return Vec::new();
    }
    entries
        .into_iter()
        .enumerate()
        .map(|(index, (label, value))| Slice {
            label,
            value,
            share: value / total,
            color: theme.series_color(index),
        })
        .collect()
}

fn draw_line_series(
    painter: &mut Painter,
    series: &ParsedSeries,
    color: Color,
    kind: ChartKind,
    max_points: usize,
    left: f64,
    width: f64,
    map_y: &impl Fn(f64) -> f64,
) {
    let points: Vec<(f64, f64)> = series
        .values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let ratio = if max_points <= 1 {
                0.5
            } else {
                index as f64 / (max_points - 1) as f64
            };
            (left + width * ratio, map_y(value.to_f64()))
        })
        .collect();
    let Some(&(first_x, first_y)) = points.first() else {
        return;
    };
    let baseline = map_y(0.0);

    if matches!(kind, ChartKind::Area) {
        let mut d = String::new();
        let _ = write!(
            d,
            "M{} {}L{} {}",
            num(first_x),
            num(baseline),
            num(first_x),
            num(first_y)
        );
        for (x, y) in points.iter().skip(1) {
            let _ = write!(d, "L{} {}", num(*x), num(*y));
        }
        let last_x = points.last().map_or(first_x, |point| point.0);
        let _ = write!(d, "L{} {}Z", num(last_x), num(baseline));
        let _ = write!(
            painter.out,
            "<path d=\"{d}\" fill=\"{}\" stroke=\"none\"/>",
            color.with_alpha(56).to_hex()
        );
    }

    let mut d = String::new();
    let _ = write!(d, "M{} {}", num(first_x), num(first_y));
    for (x, y) in points.iter().skip(1) {
        let _ = write!(d, "L{} {}", num(*x), num(*y));
    }
    let _ = write!(
        painter.out,
        "<path d=\"{d}\" fill=\"none\" stroke=\"{}\" stroke-width=\"2.5\" stroke-linejoin=\"round\" stroke-linecap=\"round\"/>",
        color.to_hex()
    );

    if points.len() <= 40 {
        for (x, y) in &points {
            let _ = write!(
                painter.out,
                "<circle cx=\"{}\" cy=\"{}\" r=\"3\" fill=\"{}\"/>",
                num(*x),
                num(*y),
                color.to_hex()
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_bars(
    painter: &mut Painter,
    series: &ParsedSeries,
    color: Color,
    max_points: usize,
    series_count: usize,
    series_index: usize,
    left: f64,
    width: f64,
    map_y: &impl Fn(f64) -> f64,
) {
    let slot = width / max_points.max(1) as f64;
    let group = slot * 0.72;
    let bar_width = (group / series_count.max(1) as f64).max(1.0);
    let baseline = map_y(0.0);
    for (index, value) in series.values.iter().enumerate() {
        let x = left + index as f64 * slot + (slot - group) / 2.0 + series_index as f64 * bar_width;
        let y = map_y(value.to_f64());
        let top = y.min(baseline);
        let mut height = (baseline - y).abs();
        if height < 1.0 && !value.is_zero() {
            height = 1.0;
        }
        if height <= 0.0 {
            continue;
        }
        painter.rect(
            x,
            top,
            (bar_width - 1.5).max(1.0),
            height,
            color.with_alpha(230),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_donut(
    painter: &mut Painter,
    slices: &[Slice],
    total: f64,
    theme: &Theme,
    spec: &ChartSpec,
    left: f64,
    top: f64,
    width: f64,
    height: f64,
) {
    let cx = left + width / 2.0;
    let cy = top + height / 2.0;
    let radius = (width.min(height) * 0.45).max(6.0);
    let inner = (radius * 0.58).max(2.0);

    if slices.len() == 1 {
        let slice = &slices[0];
        let _ = write!(
            painter.out,
            "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\"/>",
            num(cx),
            num(cy),
            num(radius),
            slice.color.to_hex()
        );
        let _ = write!(
            painter.out,
            "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\"/>",
            num(cx),
            num(cy),
            num(inner),
            theme.background.to_hex()
        );
    } else {
        let mut start = -90.0_f64;
        for slice in slices {
            let sweep = (slice.share * 360.0).clamp(0.0, 360.0);
            if sweep <= 0.0 {
                continue;
            }
            let end = start + sweep;
            let (ox1, oy1) = polar(cx, cy, radius, start);
            let (ox2, oy2) = polar(cx, cy, radius, end);
            let (ix2, iy2) = polar(cx, cy, inner, end);
            let (ix1, iy1) = polar(cx, cy, inner, start);
            let large = if sweep > 180.0 { 1 } else { 0 };
            let _ = write!(
                painter.out,
                "<path d=\"M{} {}A{} {} 0 {} 1 {} {}L{} {}A{} {} 0 {} 0 {} {}Z\" fill=\"{}\" stroke=\"{}\" stroke-width=\"1.5\"/>",
                num(ox1),
                num(oy1),
                num(radius),
                num(radius),
                large,
                num(ox2),
                num(oy2),
                num(ix2),
                num(iy2),
                num(inner),
                num(inner),
                large,
                num(ix1),
                num(iy1),
                slice.color.to_hex(),
                theme.background.to_hex()
            );

            if radius >= 48.0 && sweep >= 24.0 {
                let label = format!("{}%", trim_number(slice.share * 100.0, 0));
                let (lx, ly) = polar(cx, cy, (radius + inner) / 2.0, start + sweep / 2.0);
                let label_width = measure(&label, 1.5);
                painter.text(
                    lx - label_width / 2.0,
                    ly - font::line_height(1.5) / 2.0,
                    &label,
                    1.5,
                    theme.background,
                );
            }
            start = end;
        }
    }

    // Total in the hole.
    if inner >= 26.0 {
        let value = fit_text(&format_axis_value(total), inner * 1.6, 2.0);
        let currency = font::sanitize(spec.currency.trim());
        let value_width = measure(&value, 2.0);
        painter.text(
            cx - value_width / 2.0,
            cy - font::line_height(2.0) / 2.0 - if currency.is_empty() { 0.0 } else { 8.0 },
            &value,
            2.0,
            theme.foreground,
        );
        if !currency.is_empty() {
            let width = measure(&currency, 1.5);
            painter.text(
                cx - width / 2.0,
                cy + font::line_height(2.0) / 2.0 + 4.0,
                &currency,
                1.5,
                theme.muted,
            );
        }
    }
}

fn polar(cx: f64, cy: f64, radius: f64, degrees: f64) -> (f64, f64) {
    let radians = degrees.to_radians();
    (cx + radius * radians.cos(), cy + radius * radians.sin())
}

// ---- primitives -----------------------------------------------------------

/// Accumulates the document and remembers every string it drew.
struct Painter {
    out: String,
    texts: Vec<String>,
}

impl Painter {
    fn new() -> Self {
        Self {
            out: String::with_capacity(16 * 1024),
            texts: Vec::new(),
        }
    }

    fn rect(&mut self, x: f64, y: f64, width: f64, height: f64, color: Color) {
        if !(width > 0.0) || !(height > 0.0) {
            return;
        }
        let _ = write!(
            self.out,
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>",
            num(x),
            num(y),
            num(width),
            num(height),
            color.to_hex()
        );
    }

    fn line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, color: Color, width: f64) {
        let _ = write!(
            self.out,
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"{}\"/>",
            num(x1),
            num(y1),
            num(x2),
            num(y2),
            color.to_hex(),
            trim_number(width, 2)
        );
    }

    /// Draws `text` with the stencil font: one path of merged pixel rectangles.
    fn text(&mut self, x: f64, y: f64, text: &str, scale: f64, color: Color) {
        let text = font::sanitize(text);
        if text.trim().is_empty() || !x.is_finite() || !y.is_finite() || !(scale > 0.0) {
            return;
        }
        self.texts.push(text.clone());

        let mut path = String::new();
        let mut cursor = x;
        for ch in text.chars() {
            for (row, bits) in font::glyph(ch).iter().enumerate() {
                let bits = u32::from(*bits);
                let mut column = 0;
                while column < font::GLYPH_W {
                    if bits & (1 << (4 - column)) != 0 {
                        let start = column;
                        while column < font::GLYPH_W && bits & (1 << (4 - column)) != 0 {
                            column += 1;
                        }
                        let run = f64::from(column - start) * scale;
                        let _ = write!(
                            path,
                            "M{} {}h{}v{}h-{}z",
                            num(cursor + f64::from(start) * scale),
                            num(y + row as f64 * scale),
                            num(run),
                            num(scale),
                            num(run)
                        );
                    } else {
                        column += 1;
                    }
                }
            }
            cursor += f64::from(font::ADVANCE) * scale;
        }
        if path.is_empty() {
            return;
        }
        let _ = write!(self.out, "<path d=\"{path}\" fill=\"{}\"/>", color.to_hex());
    }
}

fn escape_xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            ' '..='~' => out.push(ch),
            _ => out.push('?'),
        }
    }
    out
}

/// Formats a coordinate compactly (`0`, `12.5`, `-3.25`), always finite.
fn num(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    trim_number(value.clamp(-1.0e6, 1.0e6), 2)
}

/// Trims a fixed-point number of trailing zeros and a bare trailing dot.
fn trim_number(value: f64, decimals: usize) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    let mut text = format!("{value:.decimals$}");
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    if text == "-0" || text.is_empty() {
        text = "0".to_string();
    }
    text
}

/// Axis tick text: grouped below ten thousand, then `K`/`M`/`B`/`T`.
pub(crate) fn format_axis_value(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    let magnitude = value.abs();
    if magnitude >= 1.0e15 {
        // Past "T" the suffix scale runs out; an exponent stays short.
        let exponent = magnitude.log10().floor();
        let mantissa = value / 10f64.powf(exponent);
        if exponent.is_finite() && mantissa.is_finite() && exponent.abs() < 400.0 {
            return format!("{}E{}", trim_number(mantissa, 1), exponent as i64);
        }
        return "0".to_string();
    }
    // (threshold to switch suffix, divisor, suffix)
    let scales: [(f64, f64, &str); 4] = [
        (1.0e12, 1.0e12, "T"),
        (1.0e9, 1.0e9, "B"),
        (1.0e6, 1.0e6, "M"),
        (1.0e4, 1.0e3, "K"),
    ];
    for (threshold, divisor, suffix) in scales {
        if magnitude >= threshold {
            let scaled = value / divisor;
            let decimals = if scaled.abs() < 10.0 {
                2
            } else if scaled.abs() < 100.0 {
                1
            } else {
                0
            };
            return format!("{}{}", trim_number(scaled, decimals), suffix);
        }
    }
    let text = trim_number(value, 2);
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", text.as_str()),
    };
    let (whole, fraction) = match digits.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (digits, None),
    };
    let mut grouped = String::with_capacity(whole.len() + whole.len() / 3 + 4);
    let len = whole.len();
    for (index, ch) in whole.char_indices() {
        if index > 0 && (len - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    match fraction {
        Some(fraction) => format!("{sign}{grouped}.{fraction}"),
        None => format!("{sign}{grouped}"),
    }
}

fn measure(text: &str, scale: f64) -> f64 {
    font::text_width(&font::sanitize(text), scale)
}

/// Truncates `text` with a trailing `...` so it fits in `max_width`.
fn fit_text(text: &str, max_width: f64, scale: f64) -> String {
    let text = font::sanitize(text);
    if max_width <= 0.0 {
        return String::new();
    }
    if font::text_width(&text, scale) <= max_width {
        return text;
    }
    let per_char = f64::from(font::ADVANCE) * scale;
    if per_char <= 0.0 {
        return String::new();
    }
    let allowed = (max_width / per_char).floor() as usize;
    if allowed <= 3 {
        return String::new();
    }
    let mut out: String = text.chars().take(allowed - 3).collect();
    while font::text_width(&out, scale) > max_width && !out.is_empty() {
        out.pop();
    }
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::ChartSpec;

    fn spec(kind: &str, points: &[(&str, &str)]) -> ChartSpec {
        let points: Vec<String> = points
            .iter()
            .map(|(label, value)| format!(r#"{{"label": "{label}", "value": "{value}"}}"#))
            .collect();
        ChartSpec::from_json(&format!(
            r#"{{"title": "T", "kind": "{kind}", "currency": "EUR",
                "series": [{{"name": "S", "points": [{}]}}]}}"#,
            points.join(",")
        ))
        .unwrap()
    }

    /// The accessible description, which lists every drawn string.
    fn desc(svg: &str) -> &str {
        let start = svg.find("<desc>").map_or(0, |at| at + "<desc>".len());
        let end = svg.find("</desc>").unwrap_or(svg.len());
        &svg[start..end]
    }

    #[test]
    fn nice_steps_are_round_numbers() {
        assert_eq!(nice_step(0.9), 1.0);
        assert_eq!(nice_step(1.7), 2.0);
        assert_eq!(nice_step(3.0), 5.0);
        assert_eq!(nice_step(7.0), 10.0);
        assert_eq!(nice_step(1200.0), 2000.0);
        assert_eq!(nice_step(-1.0), 1.0);
        assert_eq!(nice_step(f64::NAN), 1.0);
    }

    #[test]
    fn nice_axis_covers_the_domain() {
        let axis = nice_axis(-3.0, 17.0, 5);
        assert!(axis.min <= -3.0);
        assert!(axis.max >= 17.0);
        assert!(axis.ticks.len() >= 2);
        assert!(axis.ticks.iter().all(|t| t.is_finite()));
        for pair in axis.ticks.windows(2) {
            assert!(pair[0] < pair[1]);
        }
    }

    #[test]
    fn flat_and_infinite_domains_stay_finite() {
        for (min, max) in [
            (0.0, 0.0),
            (5.0, 5.0),
            (f64::NAN, 1.0),
            (1.0, f64::INFINITY),
        ] {
            let axis = nice_axis(min, max, 5);
            assert!(axis.min.is_finite() && axis.max.is_finite());
            assert!(axis.max > axis.min);
            assert!(axis.ticks.iter().all(|t| t.is_finite()));
        }
    }

    #[test]
    fn huge_domain_keeps_ticks_finite() {
        let axis = nice_axis(-1.7e32, 1.7e32, 5);
        assert!(axis.ticks.iter().all(|t| t.is_finite()));
        assert!(axis.ticks.len() <= MAX_TICKS + 1);
        for tick in &axis.ticks {
            let y = map_y(*tick, &axis, 10.0, 100.0);
            assert!(y.is_finite());
            assert!((10.0..=110.0).contains(&y));
        }
    }

    #[test]
    fn axis_values_are_human_readable() {
        assert_eq!(format_axis_value(0.0), "0");
        assert_eq!(format_axis_value(1234.0), "1,234");
        assert_eq!(format_axis_value(-1234.5), "-1,234.5");
        assert_eq!(format_axis_value(12_000.0), "12K");
        assert_eq!(format_axis_value(1_500_000.0), "1.5M");
        assert_eq!(format_axis_value(-2.5e9), "-2.5B");
        assert_eq!(format_axis_value(3.0e12), "3T");
        assert_eq!(format_axis_value(1.7e32), "1.7E32");
        assert_eq!(format_axis_value(-5.0e31), "-5E31");
        assert_eq!(format_axis_value(f64::NAN), "0");
    }

    #[test]
    fn every_kind_produces_a_document_of_the_requested_size() {
        let points = [("a", "1"), ("b", "2.5"), ("c", "-1")];
        for kind in ["line", "bar", "area", "donut"] {
            let svg = build_svg(&spec(kind, &points), &RenderOptions::sized(400, 240));
            assert!(svg.starts_with("<svg "), "{kind} is missing its root");
            assert!(svg.ends_with("</svg>"));
            assert!(svg.contains("width=\"400\" height=\"240\""), "{kind}");
            assert!(!svg.contains("NaN") && !svg.contains("inf"), "{kind}");
        }
    }

    #[test]
    fn empty_spec_renders_the_placeholder() {
        let spec = ChartSpec::from_json(r#"{"kind": "bar", "series": []}"#).unwrap();
        let svg = build_svg(&spec, &RenderOptions::default());
        assert!(desc(&svg).contains(NO_DATA_LABEL), "{svg}");
    }

    #[test]
    fn text_is_drawn_as_paths_never_as_text_elements() {
        let svg = build_svg(
            &spec("line", &[("Jan", "1"), ("Feb", "2")]),
            &RenderOptions::default(),
        );
        assert!(!svg.contains("<text"), "stencil text must not need a font");
        assert!(svg.contains("<path"));
        // ... but the drawn strings stay machine-readable.
        assert!(svg.contains("<desc>"));
        assert!(svg.contains(">Jan<") || svg.contains("Jan |") || svg.contains("Jan"));
    }

    #[test]
    fn legend_wraps_and_truncates_long_names() {
        let long = "x".repeat(300);
        let spec = ChartSpec::from_json(&format!(
            r#"{{"kind": "line", "series": [
                {{"name": "{long}", "points": [{{"label": "a", "value": "1"}}]}},
                {{"name": "second", "points": [{{"label": "a", "value": "2"}}]}}
            ]}}"#
        ))
        .unwrap();
        let svg = build_svg(&spec, &RenderOptions::sized(320, 240));
        assert!(desc(&svg).contains("..."), "{svg}");
        assert!(!svg.contains("NaN"));
    }

    #[test]
    fn tiny_canvas_still_renders() {
        for (width, height) in [(64, 64), (64, 400), (400, 64), (100, 100)] {
            let svg = build_svg(
                &spec("bar", &[("a", "1")]),
                &RenderOptions::sized(width, height),
            );
            assert!(svg.starts_with("<svg "));
        }
    }

    #[test]
    fn donut_folds_extra_slices_into_other() {
        let points: Vec<(String, String)> = (0..20)
            .map(|index| (format!("l{index}"), format!("{}", index + 1)))
            .collect();
        let refs: Vec<(&str, &str)> = points
            .iter()
            .map(|(label, value)| (label.as_str(), value.as_str()))
            .collect();
        let svg = build_svg(&spec("donut", &refs), &RenderOptions::default());
        assert!(desc(&svg).contains("Other"), "{svg}");
        assert!(desc(&svg).contains('%'));
    }

    #[test]
    fn every_drawn_string_lands_in_the_description() {
        let svg = build_svg(&spec("line", &[("Jan", "1000")]), &RenderOptions::default());
        assert!(desc(&svg).contains("Jan"), "{svg}");
        assert!(desc(&svg).contains('S')); // the legend label
        assert!(svg.len() < 200_000, "document grew unexpectedly");
    }
}
