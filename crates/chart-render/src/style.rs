// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Colours and rendering options.

/// Smallest canvas edge a chart is rendered at.
pub const MIN_DIMENSION: u32 = 64;

/// Largest canvas edge a chart is rendered at (keeps a hostile spec from
/// asking for a gigapixel allocation).
pub const MAX_DIMENSION: u32 = 4096;

/// Default canvas width in pixels.
pub const DEFAULT_WIDTH: u32 = 1200;

/// Default canvas height in pixels.
pub const DEFAULT_HEIGHT: u32 = 720;

/// Default number of value-axis ticks.
pub const DEFAULT_TICKS: usize = 5;

/// An 8-bit sRGB colour with straight alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Alpha channel; `0` is transparent, `255` opaque.
    pub a: u8,
}

impl Color {
    /// An opaque colour.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// A colour with an explicit alpha.
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// The same colour at a different opacity.
    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    /// Parses `#rrggbb` / `#rrggbbaa` (the leading `#` is optional).
    ///
    /// Returns `None` for anything else.
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.strip_prefix('#').unwrap_or(hex);
        let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
        match hex.len() {
            6 => Some(Self::rgb(channel(0)?, channel(2)?, channel(4)?)),
            8 => Some(Self::rgba(
                channel(0)?,
                channel(2)?,
                channel(4)?,
                channel(6)?,
            )),
            _ => None,
        }
    }

    /// `#rrggbb`, or `#rrggbbaa` when not fully opaque.
    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }
}

/// The colour set used for a chart.
///
/// `series` is cycled, so a chart with more series than colours repeats them
/// (series count and colour count need not match).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// Canvas fill.
    pub background: Color,
    /// Titles and axis values.
    pub foreground: Color,
    /// Secondary labels: legend text, axis captions.
    pub muted: Color,
    /// Horizontal grid lines.
    pub grid: Color,
    /// Axis lines and the zero baseline.
    pub axis: Color,
    /// Series colours, cycled in order.
    pub series: Vec<Color>,
}

impl Theme {
    /// A dark canvas for terminals and dark UIs.
    pub fn dark() -> Self {
        Self {
            background: Color::rgb(0x0f, 0x12, 0x16),
            foreground: Color::rgb(0xe6, 0xe9, 0xef),
            muted: Color::rgb(0x9a, 0xa4, 0xb2),
            grid: Color::rgb(0x23, 0x2a, 0x33),
            axis: Color::rgb(0x47, 0x51, 0x5f),
            series: vec![
                Color::rgb(0x4c, 0x9a, 0xff),
                Color::rgb(0xf5, 0xa6, 0x23),
                Color::rgb(0x50, 0xc8, 0x78),
                Color::rgb(0xe0, 0x5d, 0x6f),
                Color::rgb(0xa7, 0x8b, 0xfa),
                Color::rgb(0x2d, 0xd4, 0xbf),
                Color::rgb(0xf4, 0x72, 0xb6),
                Color::rgb(0xa3, 0xe6, 0x35),
            ],
        }
    }

    /// A light canvas for printing and light UIs.
    pub fn light() -> Self {
        Self {
            background: Color::rgb(0xff, 0xff, 0xff),
            foreground: Color::rgb(0x1b, 0x24, 0x30),
            muted: Color::rgb(0x5b, 0x66, 0x75),
            grid: Color::rgb(0xe3, 0xe7, 0xed),
            axis: Color::rgb(0x98, 0xa2, 0xb3),
            series: vec![
                Color::rgb(0x25, 0x63, 0xeb),
                Color::rgb(0xd9, 0x77, 0x06),
                Color::rgb(0x05, 0x96, 0x69),
                Color::rgb(0xdc, 0x26, 0x26),
                Color::rgb(0x7c, 0x3a, 0xed),
                Color::rgb(0x0d, 0x94, 0x88),
                Color::rgb(0xdb, 0x27, 0x77),
                Color::rgb(0x65, 0xa3, 0x0d),
            ],
        }
    }

    /// Colour for series `index`, cycling through the palette.
    pub fn series_color(&self, index: usize) -> Color {
        if self.series.is_empty() {
            return self.foreground;
        }
        self.series[index % self.series.len()]
    }

    fn is_usable(&self) -> bool {
        !self.series.is_empty()
    }

    fn normalized(&self) -> Self {
        if self.is_usable() {
            self.clone()
        } else {
            Self {
                series: Theme::dark().series,
                ..self.clone()
            }
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

/// How to draw a PNG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderOptions {
    /// Canvas width in pixels, clamped to [`MIN_DIMENSION`]..=[`MAX_DIMENSION`].
    pub width: u32,
    /// Canvas height in pixels, clamped to [`MIN_DIMENSION`]..=[`MAX_DIMENSION`].
    pub height: u32,
    /// Colours.
    pub theme: Theme,
    /// Draw the legend under the title.
    pub legend: bool,
    /// Approximate number of value-axis ticks (2..=10 are honoured).
    pub ticks: usize,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            theme: Theme::dark(),
            legend: true,
            ticks: DEFAULT_TICKS,
        }
    }
}

impl RenderOptions {
    /// Default options with an explicit canvas size.
    pub fn sized(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            ..Self::default()
        }
    }

    /// Default options on a light canvas.
    pub fn light() -> Self {
        Self {
            theme: Theme::light(),
            ..Self::default()
        }
    }

    /// The canvas size after clamping, never zero.
    pub fn canvas(&self) -> (u32, u32) {
        (
            self.width.clamp(MIN_DIMENSION, MAX_DIMENSION),
            self.height.clamp(MIN_DIMENSION, MAX_DIMENSION),
        )
    }

    pub(crate) fn normalized(mut self) -> Self {
        let (width, height) = self.canvas();
        self.width = width;
        self.height = height;
        self.ticks = self.ticks.clamp(2, 10);
        self.theme = self.theme.normalized();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        assert_eq!(Color::rgb(0x12, 0x34, 0x56).to_hex(), "#123456");
        assert_eq!(Color::rgba(0x12, 0x34, 0x56, 0x78).to_hex(), "#12345678");
        let parsed = Color::from_hex("#4c9aff").unwrap();
        assert_eq!(parsed, Color::rgb(0x4c, 0x9a, 0xff));
        assert_eq!(Color::from_hex("4c9aff80").unwrap().a, 0x80);
        assert!(Color::from_hex("#abc").is_none());
        assert!(Color::from_hex("nonsense!").is_none());
    }

    #[test]
    fn palette_cycles_and_never_panics_on_empty() {
        let theme = Theme::dark();
        let first = theme.series_color(0);
        assert_eq!(first, theme.series_color(theme.series.len()));
        let empty = Theme {
            series: Vec::new(),
            ..Theme::dark()
        };
        assert_eq!(empty.series_color(0), empty.foreground);
        assert!(!empty.normalized().series.is_empty());
    }

    #[test]
    fn canvas_is_clamped() {
        let options = RenderOptions::sized(0, 100_000);
        assert_eq!(options.canvas(), (MIN_DIMENSION, MAX_DIMENSION));
        let options = RenderOptions {
            ticks: 99,
            ..RenderOptions::default()
        };
        assert_eq!(options.normalized().ticks, 10);
    }
}
