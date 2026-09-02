//! Collection render style, parsed from CSS-hex color strings.

use crate::error::{RenderError, Result};

/// Visual style applied when rasterizing a collection's features to PNG.
///
/// Colors are straight (non-premultiplied) RGBA bytes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderStyle {
    pub fill_rgba: [u8; 4],
    pub stroke_rgba: [u8; 4],
    pub stroke_width: f32,
    pub point_radius: f32,
}

impl RenderStyle {
    /// Builds a style from CSS-hex color strings (`"#rrggbb"` or `"#rrggbbaa"`).
    pub fn new(
        fill_hex: &str,
        stroke_hex: &str,
        stroke_width: f32,
        point_radius: f32,
    ) -> Result<Self> {
        Ok(RenderStyle {
            fill_rgba: parse_css_hex_color(fill_hex)?,
            stroke_rgba: parse_css_hex_color(stroke_hex)?,
            stroke_width,
            point_radius,
        })
    }
}

/// Parses a CSS-hex color (`"#rrggbb"` or `"#rrggbbaa"`) into RGBA bytes.
///
/// A 6-digit value gets a fully opaque alpha (`0xff`).
pub fn parse_css_hex_color(s: &str) -> Result<[u8; 4]> {
    let invalid = |reason: &'static str| RenderError::InvalidColor {
        value: s.to_string(),
        reason,
    };
    let hex = s
        .strip_prefix('#')
        .ok_or_else(|| invalid("missing leading '#'"))?;
    if !hex.is_ascii() {
        return Err(invalid("non-hex digit"));
    }
    let byte = |slice: &str| -> Result<u8> {
        u8::from_str_radix(slice, 16).map_err(|_| invalid("non-hex digit"))
    };
    match hex.len() {
        6 => Ok([
            byte(&hex[0..2])?,
            byte(&hex[2..4])?,
            byte(&hex[4..6])?,
            0xff,
        ]),
        8 => Ok([
            byte(&hex[0..2])?,
            byte(&hex[2..4])?,
            byte(&hex[4..6])?,
            byte(&hex[6..8])?,
        ]),
        _ => Err(invalid("expected 6 or 8 hex digits after '#'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_six_digit_hex_as_opaque() {
        let rgba = parse_css_hex_color("#3388ff").unwrap();
        assert_eq!(rgba, [0x33, 0x88, 0xff, 0xff]);
    }

    #[test]
    fn parses_eight_digit_hex_with_alpha() {
        let rgba = parse_css_hex_color("#3388ff66").unwrap();
        assert_eq!(rgba, [0x33, 0x88, 0xff, 0x66]);
    }

    #[test]
    fn rejects_missing_hash() {
        assert!(parse_css_hex_color("3388ff").is_err());
    }

    #[test]
    fn rejects_wrong_length() {
        assert!(parse_css_hex_color("#38f").is_err());
        assert!(parse_css_hex_color("#3388ff6").is_err());
    }

    #[test]
    fn rejects_non_hex_digits() {
        assert!(parse_css_hex_color("#zz88ff").is_err());
    }

    #[test]
    fn rejects_multi_byte_utf8_instead_of_panicking() {
        // A byte length of 6 (matching the 6-digit branch) with a non-ASCII
        // character positioned so a fixed byte offset would land inside its
        // multi-byte encoding, if the length check alone gated the slice.
        assert!(parse_css_hex_color("#1\u{e0}234").is_err());
    }

    #[test]
    fn render_style_new_builds_from_both_colors() {
        let style = RenderStyle::new("#3388ff66", "#3366cc", 1.5, 3.0).unwrap();
        assert_eq!(style.fill_rgba, [0x33, 0x88, 0xff, 0x66]);
        assert_eq!(style.stroke_rgba, [0x33, 0x66, 0xcc, 0xff]);
        assert_eq!(style.stroke_width, 1.5);
        assert_eq!(style.point_radius, 3.0);
    }
}
