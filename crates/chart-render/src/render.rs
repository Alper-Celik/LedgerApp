// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! SVG → PNG in memory, for both the app and the terminal encoder.

use crate::spec::ChartSpec;
use crate::style::{Color, RenderOptions};
use crate::svg;

/// The 8-byte PNG signature.
pub const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// Renders `spec` to an SVG document (exposed for debugging and tests).
pub fn svg_source(spec: &ChartSpec, options: RenderOptions) -> String {
    svg::build_svg(spec, &options)
}

/// Renders `spec` to PNG bytes.
///
/// The canvas is exactly the requested size (clamped to 64..=4096 per edge).
/// The function never panics and never returns an empty `Vec`: if
/// rasterisation somehow fails, a solid background-coloured image of the right
/// size is returned so callers can always write bytes out.
pub fn render_png(spec: &ChartSpec, options: RenderOptions) -> Vec<u8> {
    let options = options.normalized();
    let document = svg::build_svg(spec, &options);
    if let Some(png) = rasterize(&document, &options) {
        return png;
    }
    solid_png(options.width, options.height, options.theme.background)
        .unwrap_or_else(|| PNG_SIGNATURE.to_vec())
}

/// Strict variant of [`render_png`] that reports rasterisation failure.
pub fn try_render_png(spec: &ChartSpec, options: &RenderOptions) -> Option<Vec<u8>> {
    let options = options.clone().normalized();
    let document = svg::build_svg(spec, &options);
    rasterize(&document, &options)
}

fn rasterize(document: &str, options: &RenderOptions) -> Option<Vec<u8>> {
    let tree = resvg::usvg::Tree::from_str(document, &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(options.width, options.height)?;
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
    pixmap.encode_png().ok()
}

fn solid_png(width: u32, height: u32, color: Color) -> Option<Vec<u8>> {
    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    pixmap.fill(tiny_skia::Color::from_rgba8(
        color.r, color.g, color.b, color.a,
    ));
    pixmap.encode_png().ok()
}

/// Reads the pixel size out of a PNG's `IHDR` chunk.
///
/// Returns `None` when the bytes are not a PNG or the header is truncated.
pub fn png_dimensions(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 || png[..8] != PNG_SIGNATURE {
        return None;
    }
    if &png[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(png[20..24].try_into().ok()?);
    Some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Theme;

    fn spec() -> ChartSpec {
        ChartSpec::from_json(
            r#"{"title": "T", "kind": "line", "currency": "EUR",
                "series": [{"name": "A", "points": [{"label": "x", "value": "1"}]}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn png_has_signature_and_requested_size() {
        let png = render_png(&spec(), RenderOptions::sized(320, 200));
        assert_eq!(&png[..8], &PNG_SIGNATURE);
        assert_eq!(png_dimensions(&png), Some((320, 200)));
    }

    #[test]
    fn dimensions_clamp_extremes() {
        let png = render_png(&spec(), RenderOptions::sized(1, 100_000));
        assert_eq!(png_dimensions(&png), Some((64, 4096)));
    }

    #[test]
    fn rejects_non_png_bytes() {
        assert_eq!(png_dimensions(b"not a png at all"), None);
        assert_eq!(png_dimensions(&[]), None);
        let mut truncated = PNG_SIGNATURE.to_vec();
        truncated.extend_from_slice(&[0, 0, 0, 13]);
        assert_eq!(png_dimensions(&truncated), None);
    }

    #[test]
    fn solid_fallback_is_a_valid_image() {
        let png = solid_png(40, 30, Theme::light().background).unwrap();
        assert_eq!(png_dimensions(&png), Some((40, 30)));
    }

    #[test]
    fn output_is_deterministic() {
        let first = render_png(&spec(), RenderOptions::default());
        let second = render_png(&spec(), RenderOptions::default());
        assert_eq!(first, second);
    }
}
