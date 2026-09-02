//! Composites several MVT tiles onto ONE output canvas of an arbitrary
//! pixel size (OGC API — Maps Part 1, `#86`) — the map-window counterpart of
//! [`crate::render_mvt_to_png`]/[`crate::render_mvt_to_png_styled`], which
//! each rasterize exactly one MVT tile onto its own `tile_size`-square
//! canvas. A map request names a bbox/width/height window rather than a
//! single tile-pyramid coordinate, so it may need zero, one, or several
//! covering tiles' worth of geometry painted into the same image — this
//! module owns only the "paint N tiles onto one canvas, then encode" step;
//! choosing which tiles cover a request and how each tile's own local
//! coordinates map into the output window is the caller's job (see
//! `tellurion-tiles::maps`), since that depends on tile-matrix math and CRS
//! handling this crate has no notion of at all — this crate stays pure
//! rasterization, "bytes (plus a projection) in, bytes out".

use std::collections::BTreeMap;

use tiny_skia::Pixmap;

use crate::error::{RenderError, Result};
use crate::raster::paint_mvt_onto;
use crate::style::RenderStyle;
use crate::styled::LayerPaint;

/// One covering tile's contribution to a composited map render: its raw MVT
/// bytes plus the per-vertex projection from that tile's own `[0, 1]`
/// normalized tile-local space (see [`crate::raster::paint_mvt_onto`]'s own
/// doc for what "normalized" means) into the shared output canvas's pixel
/// space. Every tile covering one map request shares the same style
/// resolution — passed once to [`render_map_window`]/
/// [`render_map_window_styled`], not per tile — only the source bytes and
/// the destination projection vary tile to tile.
pub struct MapTile<'a> {
    pub mvt: &'a [u8],
    pub project: Box<dyn Fn(f64, f64) -> (f32, f32) + 'a>,
}

/// Builds the shared `width`x`height` output canvas every covering tile in
/// `tiles` paints onto, in order — refused up front (before any tile is
/// decoded) when the requested output dimensions can't back a real pixmap,
/// the same [`RenderError::InvalidDimensions`] shape
/// [`crate::encode_rgba_to_png`] already uses for a degenerate raster
/// window.
fn new_canvas(width: u32, height: u32) -> Result<Pixmap> {
    Pixmap::new(width, height).ok_or(RenderError::InvalidDimensions { width, height })
}

/// Composites `tiles` onto one `width`x`height` canvas using a single
/// collection-wide `style` (the unstyled map lane's own paint — mirrors
/// [`crate::render_mvt_to_png`]'s "one style for every layer" rule), then
/// encodes the result as PNG. An empty `tiles` slice (every covering tile
/// was empty, or none intersected the request) yields a fully transparent
/// PNG of the requested size, the same "nothing to draw is still a valid
/// image" convention [`crate::render_mvt_to_png`] already follows for a
/// layerless tile.
pub fn render_map_window(
    width: u32,
    height: u32,
    style: &RenderStyle,
    tiles: &[MapTile<'_>],
) -> Result<Vec<u8>> {
    let mut pixmap = new_canvas(width, height)?;
    for tile in tiles {
        paint_mvt_onto(&mut pixmap, tile.mvt, &tile.project, |_name| Some(*style))?;
    }
    pixmap
        .encode_png()
        .map_err(|source| RenderError::Encode(source.to_string()))
}

/// Styled counterpart of [`render_map_window`] — same multi-tile
/// compositing, but each MVT layer's paint comes from `paints` (falling back
/// to `default_paint`, then to drawing nothing), exactly the per-layer
/// lookup [`crate::render_mvt_to_png_styled`] already does for one tile.
pub fn render_map_window_styled(
    width: u32,
    height: u32,
    paints: &BTreeMap<String, LayerPaint>,
    default_paint: Option<&LayerPaint>,
    tiles: &[MapTile<'_>],
) -> Result<Vec<u8>> {
    let mut pixmap = new_canvas(width, height)?;
    for tile in tiles {
        paint_mvt_onto(&mut pixmap, tile.mvt, &tile.project, |layer_name| {
            paints
                .get(layer_name)
                .or(default_paint)
                .copied()
                .map(RenderStyle::from)
        })?;
    }
    pixmap
        .encode_png()
        .map_err(|source| RenderError::Encode(source.to_string()))
}

/// One covering RASTER tile's contribution to a composited map render
/// (`#37`) — the raster counterpart of [`MapTile`], carrying decoded pixels
/// instead of MVT bytes.
///
/// A `RasterSource` driver hands back an already-decoded, already-resampled
/// straight-RGBA8 window (`tellurion_core::RasterWindow`) rather than
/// geometry, so there is nothing here to rasterize: the whole job is
/// deciding, for each destination pixel, which source sample it shows.
/// `dest` bounds that work to the rectangle this tile can possibly reach
/// (so N disjoint covering tiles cost one pass over the canvas between
/// them, not N), and `sample` is the inverse projection the caller owns —
/// tile-matrix and CRS math this crate deliberately has no notion of, the
/// same split [`MapTile::project`] already makes in the other direction.
pub struct RasterMapTile<'a> {
    /// Row-major straight (non-premultiplied) RGBA8, `width * height * 4`
    /// bytes — exactly `tellurion_core::RasterWindow`'s own layout.
    pub rgba: &'a [u8],
    pub width: u32,
    pub height: u32,
    /// Half-open destination pixel rectangle `[x0, y0, x1, y1)` this tile
    /// may write into, already clamped to the output canvas by the caller.
    /// An empty rectangle (`x0 >= x1` or `y0 >= y1`) contributes nothing.
    pub dest: [u32; 4],
    /// Destination pixel CENTRE (in output pixel coordinates) to this tile's
    /// own normalized `[0, 1]` tile-local space, x rightwards and y
    /// downwards — the same normalized convention
    /// [`crate::raster::paint_mvt_onto`] uses for MVT geometry. A result
    /// outside `[0, 1)` on either axis means this destination pixel is not
    /// covered by this tile and is left untouched.
    pub sample: Box<dyn Fn(f64, f64) -> (f64, f64) + 'a>,
}

/// Composites `tiles` onto one `width`x`height` straight-RGBA8 canvas by
/// nearest-neighbour sampling, then encodes the result through
/// [`crate::encode_rgba_to_png`] — the SAME encoder the raster tile lane
/// already ends in, so a map window and a map tile of the same pixels
/// produce bytes the same way.
///
/// Nearest neighbour, not interpolation, deliberately: every raster driver
/// in this workspace has already resampled its own decode onto the
/// destination tile grid (`RasterWindow`'s own doc), and a colormap's
/// classified colors are categorical — blending two of them invents a third
/// the classification never assigns.
///
/// A fully transparent source sample never overwrites what is already on
/// the canvas: a driver marks edge-of-coverage with alpha `0`
/// (`RasterWindow`'s own contract), and two adjacent covering tiles can
/// round to the same destination pixel, so blitting the transparent one
/// second would punch a hole through its neighbour's real data. An empty
/// `tiles` slice yields a fully transparent PNG of the requested size — the
/// same "nothing to draw is still a valid image" convention
/// [`render_map_window`] follows.
pub fn render_raster_map_window(
    width: u32,
    height: u32,
    tiles: &[RasterMapTile<'_>],
) -> Result<Vec<u8>> {
    let len = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4);
    if width == 0 || height == 0 || len == 0 {
        return Err(RenderError::InvalidDimensions { width, height });
    }
    let mut canvas = vec![0u8; len];
    for tile in tiles {
        let [x0, y0, x1, y1] = tile.dest;
        let expected = (tile.width as usize)
            .saturating_mul(tile.height as usize)
            .saturating_mul(4);
        if tile.width == 0 || tile.height == 0 || tile.rgba.len() != expected {
            return Err(RenderError::InvalidDimensions {
                width: tile.width,
                height: tile.height,
            });
        }
        let x1 = x1.min(width);
        let y1 = y1.min(height);
        for py in y0..y1 {
            for px in x0..x1 {
                let (nx, ny) = (tile.sample)(f64::from(px) + 0.5, f64::from(py) + 0.5);
                if !(0.0..1.0).contains(&nx) || !(0.0..1.0).contains(&ny) {
                    continue;
                }
                let sx = ((nx * f64::from(tile.width)) as u32).min(tile.width - 1);
                let sy = ((ny * f64::from(tile.height)) as u32).min(tile.height - 1);
                let src = ((sy as usize) * (tile.width as usize) + sx as usize) * 4;
                if tile.rgba[src + 3] == 0 {
                    continue;
                }
                let dst = ((py as usize) * (width as usize) + px as usize) * 4;
                canvas[dst..dst + 4].copy_from_slice(&tile.rgba[src..src + 4]);
            }
        }
    }
    crate::raster::encode_rgba_to_png(&canvas, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geozero::mvt::{tile, Message, Tile};

    const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    fn style() -> RenderStyle {
        RenderStyle::new("#00ff00", "#ff0000", 3.0, 4.0).unwrap()
    }

    fn cmd(id: u32, count: u32) -> u32 {
        id | (count << 3)
    }
    fn zz(n: i32) -> u32 {
        ((n << 1) ^ (n >> 31)) as u32
    }
    fn move_to(dx: i32, dy: i32) -> Vec<u32> {
        vec![cmd(1, 1), zz(dx), zz(dy)]
    }

    fn point_layer(name: &str, extent: u32, dx: i32, dy: i32) -> tile::Layer {
        let mut feature = tile::Feature {
            geometry: move_to(dx, dy),
            ..Default::default()
        };
        feature.set_type(tile::GeomType::Point);
        tile::Layer {
            version: 2,
            name: name.to_string(),
            extent: Some(extent),
            features: vec![feature],
            ..Default::default()
        }
    }

    fn mvt_with_point(extent: u32, dx: i32, dy: i32) -> Vec<u8> {
        Tile {
            layers: vec![point_layer("pts", extent, dx, dy)],
        }
        .encode_to_vec()
    }

    fn pixel_rgba(pixmap: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let p = pixmap.pixel(x, y).unwrap().demultiply();
        [p.red(), p.green(), p.blue(), p.alpha()]
    }

    /// A tile placed at the LEFT half and one at the RIGHT half of a
    /// double-wide canvas, each identity-projected into its own half — proves
    /// two tiles' geometry lands in genuinely different regions of the same
    /// shared canvas, not just that two independent renders each look right
    /// on their own.
    #[test]
    fn composites_two_tiles_side_by_side_onto_one_canvas() {
        // Point at the tile-local center (extent/2, extent/2) in each source
        // tile, projected into its own 50x100 half of a 100x100 canvas.
        let left_mvt = mvt_with_point(100, 50, 50);
        let right_mvt = mvt_with_point(100, 50, 50);

        let left = MapTile {
            mvt: &left_mvt,
            project: Box::new(|nx, ny| (nx as f32 * 50.0, ny as f32 * 100.0)),
        };
        let right = MapTile {
            mvt: &right_mvt,
            project: Box::new(|nx, ny| (50.0 + nx as f32 * 50.0, ny as f32 * 100.0)),
        };

        let png = render_map_window(100, 100, &style(), &[left, right]).unwrap();
        assert_eq!(&png[0..8], &PNG_MAGIC);
        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (100, 100));
        assert_eq!(
            pixel_rgba(&pixmap, 25, 50),
            [0, 255, 0, 255],
            "left tile's point must land in the left half"
        );
        assert_eq!(
            pixel_rgba(&pixmap, 75, 50),
            [0, 255, 0, 255],
            "right tile's point must land in the right half"
        );
        assert_eq!(
            pixel_rgba(&pixmap, 75, 5)[3],
            0,
            "untouched corner must stay transparent"
        );
    }

    #[test]
    fn empty_tile_list_still_encodes_a_blank_png_at_the_requested_size() {
        let png = render_map_window(40, 30, &style(), &[]).unwrap();
        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (40, 30));
        assert_eq!(pixel_rgba(&pixmap, 20, 15)[3], 0);
    }

    #[test]
    fn zero_dimensions_are_rejected_before_any_tile_is_painted() {
        let mvt = mvt_with_point(100, 50, 50);
        let tile = MapTile {
            mvt: &mvt,
            project: Box::new(|nx, ny| (nx as f32, ny as f32)),
        };
        assert!(matches!(
            render_map_window(0, 10, &style(), &[tile]),
            Err(RenderError::InvalidDimensions {
                width: 0,
                height: 10
            })
        ));
    }

    #[test]
    fn styled_window_paints_layers_by_their_own_resolved_paint() {
        let mvt = mvt_with_point(100, 50, 50);
        let tile = MapTile {
            mvt: &mvt,
            project: Box::new(|nx, ny| (nx as f32 * 20.0, ny as f32 * 20.0)),
        };
        let mut paints = BTreeMap::new();
        paints.insert(
            "pts".to_string(),
            LayerPaint {
                fill_rgba: [10, 20, 30, 255],
                stroke_rgba: [10, 20, 30, 255],
                stroke_width: 1.0,
                point_radius: 5.0,
            },
        );
        let png = render_map_window_styled(20, 20, &paints, None, &[tile]).unwrap();
        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert_eq!(pixel_rgba(&pixmap, 10, 10), [10, 20, 30, 255]);
    }

    // -- `#37`: the raster compositor ----------------------------------

    /// A `w`x`h` straight-RGBA source window filled with one colour.
    fn flat_window(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        rgba.iter()
            .copied()
            .cycle()
            .take((w * h * 4) as usize)
            .collect()
    }

    /// Identity: one 2x2 source window over the whole 2x2 canvas.
    fn identity_tile(rgba: &[u8], size: u32) -> RasterMapTile<'_> {
        RasterMapTile {
            rgba,
            width: size,
            height: size,
            dest: [0, 0, size, size],
            sample: Box::new(move |px, py| (px / f64::from(size), py / f64::from(size))),
        }
    }

    #[test]
    fn a_raster_window_lands_on_the_canvas_pixel_for_pixel() {
        let mut src = flat_window(2, 2, [0, 0, 0, 255]);
        // Top-left red, the rest black — enough to pin orientation as well
        // as presence.
        src[0..4].copy_from_slice(&[255, 0, 0, 255]);
        let png = render_raster_map_window(2, 2, &[identity_tile(&src, 2)]).unwrap();
        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert_eq!(pixel_rgba(&pixmap, 0, 0), [255, 0, 0, 255]);
        assert_eq!(pixel_rgba(&pixmap, 1, 1), [0, 0, 0, 255]);
    }

    /// `dest` is what bounds the work: a tile whose rectangle covers only
    /// part of the canvas leaves the rest alone. Without this the compositor
    /// would be O(canvas) per tile rather than O(canvas) in total.
    #[test]
    fn a_raster_tile_writes_only_inside_its_own_destination_rectangle() {
        let src = flat_window(1, 1, [0, 0, 255, 255]);
        let tile = RasterMapTile {
            rgba: &src,
            width: 1,
            height: 1,
            dest: [0, 0, 1, 1],
            sample: Box::new(|_, _| (0.5, 0.5)),
        };
        let png = render_raster_map_window(2, 2, &[tile]).unwrap();
        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert_eq!(pixel_rgba(&pixmap, 0, 0), [0, 0, 255, 255]);
        assert_eq!(
            pixmap.pixel(1, 1).unwrap().alpha(),
            0,
            "a pixel outside the tile's destination rectangle must stay untouched"
        );
    }

    /// A sample outside the tile's own `[0, 1)` normalized space is not this
    /// tile's pixel — the seam rule that lets `dest` round outwards without
    /// smearing a tile past its own edge.
    #[test]
    fn a_sample_outside_the_tiles_own_space_writes_nothing() {
        let src = flat_window(1, 1, [0, 255, 0, 255]);
        let tile = RasterMapTile {
            rgba: &src,
            width: 1,
            height: 1,
            dest: [0, 0, 1, 1],
            sample: Box::new(|_, _| (1.5, 0.5)),
        };
        let png = render_raster_map_window(1, 1, &[tile]).unwrap();
        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert_eq!(pixmap.pixel(0, 0).unwrap().alpha(), 0);
    }

    /// Edge-of-coverage transparency must never punch a hole through a
    /// neighbouring tile's real data: two adjacent covering tiles can round
    /// to the same destination pixel, and the transparent one may be second.
    #[test]
    fn a_transparent_source_sample_never_erases_a_neighbours_pixel() {
        let opaque = flat_window(1, 1, [255, 255, 0, 255]);
        let transparent = flat_window(1, 1, [0, 0, 0, 0]);
        let png = render_raster_map_window(
            1,
            1,
            &[identity_tile(&opaque, 1), identity_tile(&transparent, 1)],
        )
        .unwrap();
        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert_eq!(pixel_rgba(&pixmap, 0, 0), [255, 255, 0, 255]);
    }

    /// No covering tile intersected anything — still a valid image of the
    /// requested size, the same convention `render_map_window` follows.
    #[test]
    fn no_raster_tiles_at_all_is_a_transparent_image_of_the_requested_size() {
        let png = render_raster_map_window(3, 4, &[]).unwrap();
        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (3, 4));
        assert!(pixmap.pixels().iter().all(|p| p.alpha() == 0));
    }

    #[test]
    fn a_degenerate_canvas_or_source_window_is_refused() {
        assert!(matches!(
            render_raster_map_window(0, 4, &[]),
            Err(RenderError::InvalidDimensions { .. })
        ));
        let short = vec![0u8; 3];
        let tile = RasterMapTile {
            rgba: &short,
            width: 2,
            height: 2,
            dest: [0, 0, 2, 2],
            sample: Box::new(|_, _| (0.0, 0.0)),
        };
        assert!(matches!(
            render_raster_map_window(2, 2, &[tile]),
            Err(RenderError::InvalidDimensions { .. })
        ));
    }
}
