//! MVT -> PNG rasterization with a per-layer paint instead of one style for
//! the whole tile. Reuses [`crate::raster::render_layers`] (the same
//! decode/rasterize core [`crate::render_mvt_to_png`] uses) so this module
//! contains no drawing code of its own.

use std::collections::BTreeMap;

use crate::error::Result;
use crate::raster::render_layers;
use crate::style::RenderStyle;

/// Paint applied to one MVT layer when rasterizing a styled tile. Same shape
/// as [`RenderStyle`] (a resolved style document's per-layer paint), kept as
/// its own type so this module doesn't force callers to depend on the
/// collection-wide style type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerPaint {
    pub fill_rgba: [u8; 4],
    pub stroke_rgba: [u8; 4],
    pub stroke_width: f32,
    pub point_radius: f32,
}

impl From<LayerPaint> for RenderStyle {
    fn from(paint: LayerPaint) -> Self {
        RenderStyle {
            fill_rgba: paint.fill_rgba,
            stroke_rgba: paint.stroke_rgba,
            stroke_width: paint.stroke_width,
            point_radius: paint.point_radius,
        }
    }
}

/// Decodes `mvt` and rasterizes it using a paint resolved per MVT layer
/// name: `paints` is checked first, then `default_paint`; a layer matching
/// neither is skipped (drawn as nothing) rather than guessing a color.
pub fn render_mvt_to_png_styled(
    mvt: &[u8],
    paints: &BTreeMap<String, LayerPaint>,
    default_paint: Option<&LayerPaint>,
    tile_size: u32,
) -> Result<Vec<u8>> {
    render_layers(mvt, tile_size, |layer_name| {
        paints
            .get(layer_name)
            .or(default_paint)
            .copied()
            .map(RenderStyle::from)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use geozero::mvt::{tile, Message, Tile};
    use tiny_skia::Pixmap;

    const GREEN: LayerPaint = LayerPaint {
        fill_rgba: [0, 255, 0, 255],
        stroke_rgba: [0, 255, 0, 255],
        stroke_width: 1.0,
        point_radius: 4.0,
    };
    const RED: LayerPaint = LayerPaint {
        fill_rgba: [255, 0, 0, 255],
        stroke_rgba: [255, 0, 0, 255],
        stroke_width: 1.0,
        point_radius: 4.0,
    };

    fn cmd(id: u32, count: u32) -> u32 {
        id | (count << 3)
    }
    fn zz(n: i32) -> u32 {
        ((n << 1) ^ (n >> 31)) as u32
    }
    fn move_to(dx: i32, dy: i32) -> Vec<u32> {
        vec![cmd(1, 1), zz(dx), zz(dy)]
    }

    fn point_layer(name: &str, dx: i32, dy: i32) -> tile::Layer {
        let mut feature = tile::Feature {
            geometry: move_to(dx, dy),
            ..Default::default()
        };
        feature.set_type(tile::GeomType::Point);
        tile::Layer {
            version: 2,
            name: name.to_string(),
            extent: Some(100),
            features: vec![feature],
            ..Default::default()
        }
    }

    fn pixel_rgba(pixmap: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let p = pixmap.pixel(x, y).unwrap().demultiply();
        [p.red(), p.green(), p.blue(), p.alpha()]
    }

    #[test]
    fn distinguishes_two_layers_by_paint() {
        let mvt = Tile {
            layers: vec![
                point_layer("buildings", 20, 20),
                point_layer("roads", 80, 80),
            ],
        }
        .encode_to_vec();

        let mut paints = BTreeMap::new();
        paints.insert("buildings".to_string(), GREEN);
        paints.insert("roads".to_string(), RED);

        let png = render_mvt_to_png_styled(&mvt, &paints, None, 100).unwrap();
        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert_eq!(pixel_rgba(&pixmap, 20, 20), [0, 255, 0, 255]);
        assert_eq!(pixel_rgba(&pixmap, 80, 80), [255, 0, 0, 255]);
    }

    #[test]
    fn unlisted_layer_without_default_is_skipped() {
        let mvt = Tile {
            layers: vec![
                point_layer("buildings", 20, 20),
                point_layer("water", 80, 80),
            ],
        }
        .encode_to_vec();

        let mut paints = BTreeMap::new();
        paints.insert("buildings".to_string(), GREEN);

        let png = render_mvt_to_png_styled(&mvt, &paints, None, 100).unwrap();
        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert_eq!(pixel_rgba(&pixmap, 20, 20), [0, 255, 0, 255]);
        assert_eq!(
            pixel_rgba(&pixmap, 80, 80)[3],
            0,
            "layer absent from paints and no default must not be drawn"
        );
    }

    #[test]
    fn unlisted_layer_falls_back_to_default_paint() {
        let mvt = Tile {
            layers: vec![point_layer("water", 80, 80)],
        }
        .encode_to_vec();

        let png = render_mvt_to_png_styled(&mvt, &BTreeMap::new(), Some(&RED), 100).unwrap();
        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert_eq!(pixel_rgba(&pixmap, 80, 80), [255, 0, 0, 255]);
    }

    #[test]
    fn malformed_mvt_is_a_decode_error() {
        use crate::error::RenderError;
        assert!(matches!(
            render_mvt_to_png_styled(b"not a tile", &BTreeMap::new(), None, 256),
            Err(RenderError::Decode(_))
        ));
    }
}
