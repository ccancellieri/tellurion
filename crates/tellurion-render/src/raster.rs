//! Rasterizes a decoded MVT tile into a PNG using a [`RenderStyle`]. Pure:
//! bytes in, bytes out. Handles Point/MultiPoint (filled circles),
//! LineString/MultiLineString (stroked), and Polygon/MultiPolygon (even-odd
//! fill so holes render correctly regardless of MVT ring winding, plus a
//! stroked outline).

use geozero::error::Result as GzResult;
use geozero::mvt::{Message, Tile};
use geozero::{FeatureProcessor, GeomProcessor, GeozeroDatasource, PropertyProcessor};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

use crate::error::{RenderError, Result};
use crate::style::RenderStyle;

/// MVT layer extent per the Mapbox Vector Tile spec when a layer omits one.
const DEFAULT_EXTENT: u32 = 4096;

/// MVT geometries legitimately carry a small buffer past their layer's
/// nominal extent so adjoining tiles share edge pixels on simplification.
/// Anything further out than one full extent-width beyond the tile in any
/// direction contributes nothing visible; it is clamped (if part of a shape
/// that also has an in-view vertex) or the whole shape is culled (if none of
/// its vertices are within this margin), rather than handed to the
/// rasterizer as unbounded coordinates.
const CULL_MARGIN_EXTENT_MULTIPLE: f64 = 1.0;

/// Decodes `mvt` and rasterizes every layer's features, in order, onto a
/// square `tile_size`-pixel canvas using `style`, returning encoded PNG
/// bytes. A tile with no layers (or only culled geometry) yields a fully
/// transparent PNG of the requested size.
pub fn render_mvt_to_png(mvt: &[u8], style: &RenderStyle, tile_size: u32) -> Result<Vec<u8>> {
    render_layers(mvt, tile_size, |_name| Some(*style))
}

/// Shared core behind [`render_mvt_to_png`] and the styled renderer: decodes
/// `mvt` and rasterizes every layer whose name `style_for_layer` maps to
/// `Some`, in tile-layer order, skipping layers it maps to `None`, onto a
/// fresh square `tile_size`-pixel canvas. Kept `pub(crate)` so the styled
/// path (per-layer paint lookup) reuses the same decode/rasterize logic
/// instead of duplicating it.
pub(crate) fn render_layers(
    mvt: &[u8],
    tile_size: u32,
    style_for_layer: impl FnMut(&str) -> Option<RenderStyle>,
) -> Result<Vec<u8>> {
    let mut pixmap =
        Pixmap::new(tile_size, tile_size).ok_or(RenderError::InvalidTileSize(tile_size))?;
    let scale = tile_size as f64;
    paint_mvt_onto(
        &mut pixmap,
        mvt,
        |nx, ny| ((nx * scale) as f32, (ny * scale) as f32),
        style_for_layer,
    )?;
    pixmap
        .encode_png()
        .map_err(|source| RenderError::Encode(source.to_string()))
}

/// Decodes `mvt` and paints every layer whose name `style_for_layer` maps to
/// `Some` directly onto the caller-owned `pixmap`, in tile-layer order —
/// no allocation, no PNG encode: the geometry core shared by [`render_layers`]
/// (a fresh, `tile_size`-square canvas, one tile per call) and the OGC API
/// Maps window compositor (`tellurion-tiles::maps`, `#86`), which paints
/// several covering tiles' worth of geometry onto ONE shared output canvas
/// instead of a separate canvas per tile.
///
/// `project` receives each vertex already normalized to its own layer's
/// `[0, 1]` tile-local space (raw MVT coordinates divided by that layer's own
/// `extent`, so every caller works in the same unit regardless of a layer's
/// declared extent) and returns the destination pixel coordinates to paint it
/// at. [`render_layers`] passes a plain `(x, y) -> (x * tile_size, y *
/// tile_size)` scale; the map compositor passes a per-tile affine (or
/// reprojected) transform into a shared window instead. Culling/clamping
/// still happens in the layer's own raw, un-normalized `[0, extent]` space
/// (the [`CULL_MARGIN_EXTENT_MULTIPLE`] margin is extent-relative either
/// way), so behavior is identical to before this was pulled out of
/// `render_layers` for every existing caller.
pub(crate) fn paint_mvt_onto(
    pixmap: &mut Pixmap,
    mvt: &[u8],
    project: impl Fn(f64, f64) -> (f32, f32),
    mut style_for_layer: impl FnMut(&str) -> Option<RenderStyle>,
) -> Result<()> {
    let mut decoded =
        Tile::decode(mvt).map_err(|source| RenderError::Decode(source.to_string()))?;
    for layer in &mut decoded.layers {
        let Some(style) = style_for_layer(&layer.name) else {
            continue;
        };
        let extent = f64::from(layer.extent.unwrap_or(DEFAULT_EXTENT).max(1));
        let margin = extent * CULL_MARGIN_EXTENT_MULTIPLE;
        let mut painter =
            LayerPainter::new(pixmap, extent, -margin, extent + margin, &project, &style);
        layer
            .process(&mut painter)
            .map_err(|source| RenderError::Geometry(source.to_string()))?;
    }
    Ok(())
}

/// Encodes a raw straight-alpha RGBA8 pixel buffer (row-major,
/// `width * height * 4` bytes) into a PNG — the raster-tile counterpart of
/// [`render_mvt_to_png`] (`#37`): a `RasterSource` driver hands back
/// already-decoded, already-resampled pixels, so there is no geometry to
/// rasterize here, only a straight -> premultiplied conversion
/// (`tiny_skia::Pixmap` stores premultiplied alpha internally) and the same
/// PNG encode every other tile in this workspace goes through.
pub fn encode_rgba_to_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let expected_len = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4);
    if width == 0 || height == 0 || rgba.len() != expected_len {
        return Err(RenderError::InvalidDimensions { width, height });
    }
    let mut pixmap =
        Pixmap::new(width, height).ok_or(RenderError::InvalidDimensions { width, height })?;
    for (pixel, sample) in pixmap.pixels_mut().iter_mut().zip(rgba.chunks_exact(4)) {
        *pixel =
            tiny_skia::ColorU8::from_rgba(sample[0], sample[1], sample[2], sample[3]).premultiply();
    }
    pixmap
        .encode_png()
        .map_err(|source| RenderError::Encode(source.to_string()))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GeomMode {
    Idle,
    Point,
    Line,
    PolygonRing,
}

struct LayerPainter<'a, 'b, P: Fn(f64, f64) -> (f32, f32)> {
    pixmap: &'a mut Pixmap,
    /// This layer's own declared MVT extent (raw units) — divided out in
    /// [`project`](Self::project) so `project_fn` always receives `[0, 1]`
    /// normalized tile-local coordinates, regardless of the layer's own
    /// extent.
    extent: f64,
    bounds_lo: f64,
    bounds_hi: f64,
    project_fn: &'b P,
    style: &'b RenderStyle,
    mode: GeomMode,
    in_polygon: bool,
    line_first: bool,
    path: PathBuilder,
    polygon_path: PathBuilder,
    line_in_view: bool,
    polygon_in_view: bool,
}

impl<'a, 'b, P: Fn(f64, f64) -> (f32, f32)> LayerPainter<'a, 'b, P> {
    fn new(
        pixmap: &'a mut Pixmap,
        extent: f64,
        bounds_lo: f64,
        bounds_hi: f64,
        project_fn: &'b P,
        style: &'b RenderStyle,
    ) -> Self {
        Self {
            pixmap,
            extent,
            bounds_lo,
            bounds_hi,
            project_fn,
            style,
            mode: GeomMode::Idle,
            in_polygon: false,
            line_first: true,
            path: PathBuilder::new(),
            polygon_path: PathBuilder::new(),
            line_in_view: false,
            polygon_in_view: false,
        }
    }

    fn in_view(&self, x: f64, y: f64) -> bool {
        (self.bounds_lo..=self.bounds_hi).contains(&x)
            && (self.bounds_lo..=self.bounds_hi).contains(&y)
    }

    fn clamp(&self, v: f64) -> f64 {
        v.clamp(self.bounds_lo, self.bounds_hi)
    }

    fn project(&self, x: f64, y: f64) -> (f32, f32) {
        (self.project_fn)(x / self.extent, y / self.extent)
    }

    fn fill_paint(&self) -> Paint<'static> {
        let mut paint = Paint::default();
        let [r, g, b, a] = self.style.fill_rgba;
        paint.set_color_rgba8(r, g, b, a);
        paint.anti_alias = true;
        paint
    }

    fn stroke_paint(&self) -> Paint<'static> {
        let mut paint = Paint::default();
        let [r, g, b, a] = self.style.stroke_rgba;
        paint.set_color_rgba8(r, g, b, a);
        paint.anti_alias = true;
        paint
    }

    fn stroke(&self) -> Stroke {
        Stroke {
            width: self.style.stroke_width,
            ..Stroke::default()
        }
    }
}

impl<P: Fn(f64, f64) -> (f32, f32)> GeomProcessor for LayerPainter<'_, '_, P> {
    fn xy(&mut self, x: f64, y: f64, _idx: usize) -> GzResult<()> {
        let in_view = self.in_view(x, y);
        let (cx, cy) = (self.clamp(x), self.clamp(y));
        let (px, py) = self.project(cx, cy);
        match self.mode {
            GeomMode::Point => {
                if in_view {
                    let mut circle = PathBuilder::new();
                    circle.push_circle(px, py, self.style.point_radius);
                    if let Some(path) = circle.finish() {
                        let paint = self.fill_paint();
                        self.pixmap.fill_path(
                            &path,
                            &paint,
                            FillRule::Winding,
                            Transform::identity(),
                            None,
                        );
                    }
                }
            }
            GeomMode::Line => {
                self.line_in_view |= in_view;
                if self.line_first {
                    self.path.move_to(px, py);
                    self.line_first = false;
                } else {
                    self.path.line_to(px, py);
                }
            }
            GeomMode::PolygonRing => {
                self.polygon_in_view |= in_view;
                if self.line_first {
                    self.polygon_path.move_to(px, py);
                    self.line_first = false;
                } else {
                    self.polygon_path.line_to(px, py);
                }
            }
            GeomMode::Idle => {}
        }
        Ok(())
    }

    fn point_begin(&mut self, _idx: usize) -> GzResult<()> {
        self.mode = GeomMode::Point;
        Ok(())
    }

    fn point_end(&mut self, _idx: usize) -> GzResult<()> {
        self.mode = GeomMode::Idle;
        Ok(())
    }

    fn multipoint_begin(&mut self, _size: usize, _idx: usize) -> GzResult<()> {
        self.mode = GeomMode::Point;
        Ok(())
    }

    fn multipoint_end(&mut self, _idx: usize) -> GzResult<()> {
        self.mode = GeomMode::Idle;
        Ok(())
    }

    fn linestring_begin(&mut self, _tagged: bool, _size: usize, _idx: usize) -> GzResult<()> {
        self.line_first = true;
        if self.in_polygon {
            self.mode = GeomMode::PolygonRing;
        } else {
            self.mode = GeomMode::Line;
            self.path = PathBuilder::new();
            self.line_in_view = false;
        }
        Ok(())
    }

    fn linestring_end(&mut self, _tagged: bool, _idx: usize) -> GzResult<()> {
        match self.mode {
            GeomMode::Line => {
                let finished = std::mem::replace(&mut self.path, PathBuilder::new()).finish();
                if let (true, Some(path)) = (self.line_in_view, finished) {
                    let paint = self.stroke_paint();
                    let stroke = self.stroke();
                    self.pixmap
                        .stroke_path(&path, &paint, &stroke, Transform::identity(), None);
                }
                self.mode = GeomMode::Idle;
            }
            GeomMode::PolygonRing => {
                self.polygon_path.close();
            }
            _ => {}
        }
        Ok(())
    }

    fn polygon_begin(&mut self, _tagged: bool, _size: usize, _idx: usize) -> GzResult<()> {
        self.in_polygon = true;
        self.polygon_path = PathBuilder::new();
        self.polygon_in_view = false;
        Ok(())
    }

    fn polygon_end(&mut self, _tagged: bool, _idx: usize) -> GzResult<()> {
        self.in_polygon = false;
        let finished = std::mem::replace(&mut self.polygon_path, PathBuilder::new()).finish();
        if let (true, Some(path)) = (self.polygon_in_view, finished) {
            let fill = self.fill_paint();
            self.pixmap
                .fill_path(&path, &fill, FillRule::EvenOdd, Transform::identity(), None);
            let stroke_paint = self.stroke_paint();
            let stroke = self.stroke();
            self.pixmap
                .stroke_path(&path, &stroke_paint, &stroke, Transform::identity(), None);
        }
        self.mode = GeomMode::Idle;
        Ok(())
    }
}

impl<P: Fn(f64, f64) -> (f32, f32)> PropertyProcessor for LayerPainter<'_, '_, P> {}
impl<P: Fn(f64, f64) -> (f32, f32)> FeatureProcessor for LayerPainter<'_, '_, P> {}

#[cfg(test)]
mod tests {
    use super::*;
    use geozero::mvt::tile;

    const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    /// Opaque colors so pixel assertions don't have to reason about
    /// premultiplied-alpha blending or anti-aliased edges.
    fn style() -> RenderStyle {
        RenderStyle::new("#00ff00", "#ff0000", 3.0, 4.0).unwrap()
    }

    /// Encodes an MVT geometry command header: 3 low bits are the command id
    /// (1 = MoveTo, 2 = LineTo, 7 = ClosePath), the rest is the repeat count.
    fn cmd(id: u32, count: u32) -> u32 {
        id | (count << 3)
    }

    /// Zigzag-encodes a signed delta the way MVT geometry parameters are
    /// packed (`vector_tile.proto`'s `sint32` convention).
    fn zz(n: i32) -> u32 {
        ((n << 1) ^ (n >> 31)) as u32
    }

    fn move_to(dx: i32, dy: i32) -> Vec<u32> {
        vec![cmd(1, 1), zz(dx), zz(dy)]
    }

    fn line_to(deltas: &[(i32, i32)]) -> Vec<u32> {
        let mut v = vec![cmd(2, deltas.len() as u32)];
        for (dx, dy) in deltas {
            v.push(zz(*dx));
            v.push(zz(*dy));
        }
        v
    }

    fn close_path() -> Vec<u32> {
        vec![cmd(7, 1)]
    }

    fn layer(name: &str, extent: u32, features: Vec<tile::Feature>) -> tile::Layer {
        tile::Layer {
            version: 2,
            name: name.to_string(),
            extent: Some(extent),
            features,
            ..Default::default()
        }
    }

    fn tile_bytes(layers: Vec<tile::Layer>) -> Vec<u8> {
        Tile { layers }.encode_to_vec()
    }

    fn feature(geom_type: tile::GeomType, geometry: Vec<u32>) -> tile::Feature {
        let mut feature = tile::Feature {
            geometry,
            ..Default::default()
        };
        feature.set_type(geom_type);
        feature
    }

    fn pixel_rgba(pixmap: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let p = pixmap.pixel(x, y).unwrap().demultiply();
        [p.red(), p.green(), p.blue(), p.alpha()]
    }

    #[test]
    fn renders_a_point_tile_with_the_correct_pixel_lit() {
        // extent == tile_size == 100 makes raw MVT units 1:1 with pixels.
        let geometry = move_to(50, 50);
        let mvt = tile_bytes(vec![layer(
            "points",
            100,
            vec![feature(tile::GeomType::Point, geometry)],
        )]);

        let png = render_mvt_to_png(&mvt, &style(), 100).unwrap();
        assert_eq!(&png[0..8], &PNG_MAGIC);

        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert_eq!(pixmap.width(), 100);
        assert_eq!(pixmap.height(), 100);
        assert_eq!(pixel_rgba(&pixmap, 50, 50), [0, 255, 0, 255]);
        assert_eq!(
            pixel_rgba(&pixmap, 5, 5)[3],
            0,
            "corner must stay transparent"
        );
    }

    #[test]
    fn renders_a_line_tile_with_the_stroke_visible() {
        let geometry = [move_to(10, 50), line_to(&[(80, 0)])].concat();
        let mvt = tile_bytes(vec![layer(
            "lines",
            100,
            vec![feature(tile::GeomType::Linestring, geometry)],
        )]);

        let png = render_mvt_to_png(&mvt, &style(), 100).unwrap();
        assert_eq!(&png[0..8], &PNG_MAGIC);

        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (100, 100));
        assert_eq!(pixel_rgba(&pixmap, 50, 50), [255, 0, 0, 255]);
        assert_eq!(
            pixel_rgba(&pixmap, 5, 5)[3],
            0,
            "off-line pixel must stay transparent"
        );
    }

    /// Exterior ring 10..90, interior ring (hole) 30..70: an even-odd fill
    /// must light the band between the rings and leave the hole transparent.
    /// geozero's MVT reader classifies a ring as interior vs. a new exterior
    /// by the sign of its signed area, so the hole ring is wound opposite to
    /// the exterior ring (clockwise vs. counter-clockwise), not merely nested
    /// inside it.
    fn donut_geometry() -> Vec<u32> {
        [
            move_to(10, 10),
            line_to(&[(80, 0), (0, 80), (-80, 0)]),
            close_path(),
            move_to(20, -60), // cursor sits at (10, 90) after ring 1
            line_to(&[(0, 40), (40, 0), (0, -40)]),
            close_path(),
        ]
        .concat()
    }

    #[test]
    fn renders_a_polygon_with_a_hole_using_even_odd_fill() {
        let mvt = tile_bytes(vec![layer(
            "polys",
            100,
            vec![feature(tile::GeomType::Polygon, donut_geometry())],
        )]);

        let png = render_mvt_to_png(&mvt, &style(), 100).unwrap();
        assert_eq!(&png[0..8], &PNG_MAGIC);

        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (100, 100));
        assert_eq!(
            pixel_rgba(&pixmap, 20, 20),
            [0, 255, 0, 255],
            "band between the rings must be filled"
        );
        assert_eq!(
            pixel_rgba(&pixmap, 50, 50)[3],
            0,
            "hole must stay transparent"
        );
        assert_eq!(
            pixel_rgba(&pixmap, 95, 95)[3],
            0,
            "outside the polygon must stay transparent"
        );
    }

    #[test]
    fn renders_multiple_layers_in_order() {
        let poly_layer = layer(
            "polys",
            100,
            vec![feature(tile::GeomType::Polygon, donut_geometry())],
        );
        let line_geometry = [move_to(10, 50), line_to(&[(80, 0)])].concat();
        let line_layer = layer(
            "lines",
            100,
            vec![feature(tile::GeomType::Linestring, line_geometry)],
        );
        let mvt = tile_bytes(vec![poly_layer, line_layer]);

        let png = render_mvt_to_png(&mvt, &style(), 100).unwrap();
        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (100, 100));
        assert_eq!(
            pixel_rgba(&pixmap, 20, 20),
            [0, 255, 0, 255],
            "polygon layer must be drawn"
        );
        assert_eq!(
            pixel_rgba(&pixmap, 50, 50),
            [255, 0, 0, 255],
            "line layer drawn after the polygon must win at the overlap"
        );
        assert_eq!(
            pixel_rgba(&pixmap, 95, 5)[3],
            0,
            "untouched area must stay transparent"
        );
    }

    #[test]
    fn zero_tile_size_is_rejected() {
        let mvt = tile_bytes(vec![]);
        assert!(matches!(
            render_mvt_to_png(&mvt, &style(), 0),
            Err(RenderError::InvalidTileSize(0))
        ));
    }

    #[test]
    fn malformed_mvt_is_a_decode_error() {
        assert!(matches!(
            render_mvt_to_png(b"not a tile", &style(), 256),
            Err(RenderError::Decode(_))
        ));
    }

    #[test]
    fn empty_tile_still_encodes_a_blank_png() {
        let mvt = tile_bytes(vec![]);
        let png = render_mvt_to_png(&mvt, &style(), 64).unwrap();
        assert_eq!(&png[0..8], &PNG_MAGIC);
        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (64, 64));
        assert_eq!(pixel_rgba(&pixmap, 32, 32)[3], 0);
    }

    // -- `encode_rgba_to_png` (`#37`) ----------------------------------------

    #[test]
    fn encode_rgba_to_png_round_trips_opaque_pixels() {
        let rgba = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
        ];
        let png = encode_rgba_to_png(&rgba, 2, 2).unwrap();
        assert_eq!(&png[0..8], &PNG_MAGIC);
        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (2, 2));
        assert_eq!(pixel_rgba(&pixmap, 0, 0), [255, 0, 0, 255]);
        assert_eq!(pixel_rgba(&pixmap, 1, 0), [0, 255, 0, 255]);
    }

    #[test]
    fn encode_rgba_to_png_round_trips_transparent_pixels() {
        let rgba = vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let png = encode_rgba_to_png(&rgba, 2, 2).unwrap();
        let pixmap = Pixmap::decode_png(&png).unwrap();
        assert_eq!(pixel_rgba(&pixmap, 0, 0)[3], 0);
    }

    #[test]
    fn encode_rgba_to_png_rejects_a_mismatched_buffer_length() {
        assert!(matches!(
            encode_rgba_to_png(&[0, 0, 0, 0], 2, 2),
            Err(RenderError::InvalidDimensions {
                width: 2,
                height: 2
            })
        ));
    }

    #[test]
    fn encode_rgba_to_png_rejects_zero_dimensions() {
        assert!(matches!(
            encode_rgba_to_png(&[], 0, 0),
            Err(RenderError::InvalidDimensions {
                width: 0,
                height: 0
            })
        ));
    }

    #[test]
    fn geometry_entirely_beyond_the_buffer_margin_is_culled_not_panicked() {
        // A line whose every vertex sits ~50 tile-widths away from an
        // extent-100 tile: must be culled silently, not drawn or panicked on.
        let geometry = [move_to(5_000, 5_000), line_to(&[(200, 200)])].concat();
        let mvt = tile_bytes(vec![layer(
            "far",
            100,
            vec![feature(tile::GeomType::Linestring, geometry)],
        )]);

        let png = render_mvt_to_png(&mvt, &style(), 100).unwrap();
        let pixmap = Pixmap::decode_png(&png).unwrap();
        for y in 0..100 {
            for x in 0..100 {
                assert_eq!(pixel_rgba(&pixmap, x, y)[3], 0);
            }
        }
    }
}
