// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Renders a ledger chart to a PNG and to a terminal.
//!
//! The crate is deliberately CLI-free and I/O-free: it takes a [`ChartSpec`]
//! (the JSON shape the app will emit), returns bytes, and lets the caller
//! decide where they go.
//!
//! ```
//! use chart_render::{render_png, ChartSpec, RenderOptions, TerminalOptions, render_terminal_for};
//!
//! let spec = ChartSpec::from_json(
//!     r#"{"title": "Net worth", "kind": "line", "currency": "EUR",
//!         "series": [{"name": "Assets", "points": [
//!             {"label": "Jan", "value": "1,000.00"},
//!             {"label": "Feb", "value": "1,250.50"}]}]}"#,
//! )
//! .unwrap();
//!
//! let png = render_png(&spec, RenderOptions::default());
//! assert_eq!(&png[..8], &chart_render::PNG_SIGNATURE);
//!
//! let cells = render_terminal_for(&spec, TerminalOptions::default(), false);
//! assert!(String::from_utf8(cells).unwrap().contains("Assets"));
//! ```
//!
//! # Pipeline
//!
//! 1. [`ChartSpec`] holds decimal *strings*; [`Decimal`] keeps them exact until
//!    a coordinate has to be computed.
//! 2. The chart is composed as an SVG document (axes, ticks, grid, legend) by
//!    hand — no chart library.
//! 3. `resvg` rasterises it to a [`tiny_skia`] pixmap which is encoded as PNG.
//! 4. For a terminal, [`render_terminal`] either wraps that PNG in kitty
//!    graphics protocol escape sequences or draws a Unicode block chart.
//!
//! Output is deterministic: no timestamps, no host data, no system fonts (text
//! is drawn with a built-in 5×7 stencil font as vector paths), so the same spec
//! always produces the same bytes.
//!
//! # Supported spec subset
//!
//! * `kind`: `line`, `bar`, `area` and `donut`. Anything else is a
//!   deserialisation error rather than a silently empty chart.
//! * `title`, `x_label`, `currency`: optional; the currency is shown in the
//!   donut's centre, not in the axis labels.
//! * Series are plotted **by point index**: series of different lengths are
//!   aligned at their first point, and the horizontal category labels come from
//!   the longest series. Values are decimal *strings* — group separators
//!   (`,` `_` `'` space), a leading `+` and `1e6`-style exponents are accepted,
//!   six fraction digits are kept, and magnitudes too large for `i128` saturate
//!   instead of failing.
//! * Points whose value does not parse are dropped (and counted); a chart with
//!   no parsable point draws a `NO DATA` placeholder.
//! * `bar`/`area` always include the zero baseline; `donut` uses absolute
//!   values, keeps the twelve largest slices and folds the rest into `Other`,
//!   and shows `NO DATA` when the total is zero.
//! * Text is uppercase-only (lowercase is drawn as uppercase), `€ £ ¥ ₺` become
//!   `EUR GBP JPY TRY`, and characters outside printable ASCII become `?`.
//! * `kitty` output uses `a=T,f=100,q=2` (no terminal reply), transmits the PNG
//!   in 4096-character base64 chunks, and asks the terminal to display it in the
//!   requested cell grid (`c`/`r`).

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod base64;
mod decimal;
mod font;
mod render;
mod spec;
mod style;
mod svg;
mod terminal;

pub use decimal::{Decimal, DecimalError, FRACTION_DIGITS, SCALE, parse as parse_decimal};
pub use render::{PNG_SIGNATURE, png_dimensions, render_png, svg_source, try_render_png};
pub use spec::{ChartKind, ChartSpec, NO_DATA_LABEL, ParsedSeries, Point, Series, parsed_series};
pub use style::{
    Color, DEFAULT_HEIGHT, DEFAULT_TICKS, DEFAULT_WIDTH, MAX_DIMENSION, MIN_DIMENSION,
    RenderOptions, Theme,
};
pub use terminal::{
    DEFAULT_CELL_HEIGHT, DEFAULT_CELL_WIDTH, DEFAULT_COLUMNS, DEFAULT_ROWS, KITTY_CHUNK_SIZE,
    Protocol, TerminalOptions, kitty_available, kitty_sequence, kitty_supported, render_kitty,
    render_terminal, render_terminal_for, render_unicode,
};

/// The version of this crate, for diagnostics.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
