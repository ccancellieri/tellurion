//! Spherical Web Mercator projection math and `WebMercatorQuad` tile-matrix
//! geometry for the OGC API — Maps window compositor (`#86`,
//! `tellurion-tiles::maps`): converting a request's bbox into the tile(s)
//! that cover it, and projecting a covering tile's own normalized `[0, 1]`
//! local coordinates into either Web Mercator meters or CRS84 (WGS84
//! longitude/latitude) degrees for the final output canvas. Pure math, no
//! I/O — the same "framework-free" discipline `tellurion_core::crs` follows
//! for its own CRS handling.
//!
//! `forward`/`inverse` use the standard spherical Web Mercator formula (the
//! same pseudo-projection EPSG:3857 and every XYZ/WebMercatorQuad tile
//! scheme use — a sphere of [`EARTH_RADIUS_M`], not the WGS84 ellipsoid),
//! derived from [`tilematrixset::WEB_MERCATOR_ORIGIN`] so this module's earth
//! radius can never silently drift from the tile matrix table's own zoom-0
//! cell size.

use crate::tilematrixset::{TILE_SIZE_PX, WEB_MERCATOR_ORIGIN};

/// Sphere radius the spherical Web Mercator formula uses (meters) — the same
/// constant EPSG:3857 and every WebMercatorQuad-compatible tile scheme are
/// built on. Derived from [`WEB_MERCATOR_ORIGIN`] (`= EARTH_RADIUS_M * PI`,
/// the half-world-extent every zoom level's cell size is computed from)
/// rather than hardcoded a second time, so the two can never disagree.
pub(crate) fn earth_radius_m() -> f64 {
    WEB_MERCATOR_ORIGIN / std::f64::consts::PI
}

/// The northernmost/southernmost latitude the spherical Web Mercator
/// projection can express — the latitude whose projected `y` is exactly
/// [`WEB_MERCATOR_ORIGIN`] (see
/// `forward_of_the_max_web_mercator_latitude_reaches_the_projection_origin_edge`,
/// which pins this constant against [`forward`] itself). Callers projecting
/// a CRS84 bbox that was NOT authored against this projection — a
/// collection's own derived spatial extent, which is expressed in true
/// WGS84 and may legitimately reach the poles (`maps::collection_window`) —
/// clamp to it first, since [`forward`] is deliberately unclamped and
/// answers `+-inf` at a pole.
pub(crate) const MAX_LATITUDE_DEG: f64 = 85.051_128_779_806_59;

/// Forward spherical Web Mercator: WGS84 longitude/latitude (degrees) to
/// Web Mercator meters (EPSG:3857). Not clamped to the projection's own
/// valid latitude range (`+-85.0511...`) — a caller with a latitude short of
/// the true `+-90` pole still gets the formula's own (finite, if extreme)
/// answer; one AT or past a pole produces a non-finite result instead
/// (`tan`'s own asymptote), which `covering_tiles`' `clamp_index` already
/// treats as "clamp to tile 0" rather than propagating a `NaN`/`Infinity`
/// into a tile coordinate. `bbox`/`bbox-crs` validation happens once, at the
/// request boundary (`maps::parse_request`), not inside this pure
/// conversion — this module never rejects a latitude on its own.
pub(crate) fn forward(lon_deg: f64, lat_deg: f64) -> (f64, f64) {
    let r = earth_radius_m();
    let lon_rad = lon_deg.to_radians();
    let lat_rad = lat_deg.to_radians();
    let x = r * lon_rad;
    let y = r * (std::f64::consts::FRAC_PI_4 + lat_rad / 2.0).tan().ln();
    (x, y)
}

/// Inverse spherical Web Mercator: Web Mercator meters (EPSG:3857) back to
/// WGS84 longitude/latitude (degrees) — the exact inverse of [`forward`].
pub(crate) fn inverse(x_m: f64, y_m: f64) -> (f64, f64) {
    let r = earth_radius_m();
    let lon_deg = (x_m / r).to_degrees();
    let lat_deg = (2.0 * (y_m / r).exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees();
    (lon_deg, lat_deg)
}

/// The full mercator-meters extent of one `WebMercatorQuad` tile
/// `(z, x, y)`, as `[minx, miny, maxx, maxy]` — derived the same way
/// [`tilematrixset::web_mercator_quad_matrices`] computes each zoom level's
/// `cellSize` (half the world at zoom 0, halving every zoom), never
/// hardcoded per zoom.
pub(crate) fn tile_bounds_m(z: u8, x: u32, y: u32) -> [f64; 4] {
    let tile_size_m = tile_size_m(z);
    let minx = -WEB_MERCATOR_ORIGIN + f64::from(x) * tile_size_m;
    let maxy = WEB_MERCATOR_ORIGIN - f64::from(y) * tile_size_m;
    [minx, maxy - tile_size_m, minx + tile_size_m, maxy]
}

/// One `WebMercatorQuad` tile's full mercator-meters width/height at zoom
/// `z` — `2 * WEB_MERCATOR_ORIGIN / matrixWidth(z)`, the same halving
/// progression [`tilematrixset::web_mercator_quad_matrices`]'s `cellSize`
/// follows, just expressed per-tile (`cellSize * TILE_SIZE_PX`) rather than
/// per-pixel.
fn tile_size_m(z: u8) -> f64 {
    (2.0 * WEB_MERCATOR_ORIGIN) / matrix_side(z) as f64
}

fn matrix_side(z: u8) -> u64 {
    1u64 << z
}

/// One meters-per-pixel resolution table entry: `cellSize` at zoom 0, halved
/// every zoom — [`pick_zoom`]'s own building block, kept separate so a test
/// can pin its value against [`tilematrixset::web_mercator_quad_matrices`]'s
/// own `cellSize` without going through zoom selection.
fn cell_size_m(z: u8) -> f64 {
    tile_size_m(z) / f64::from(TILE_SIZE_PX)
}

/// The finest zoom [`native_resolution_m_per_px`] will ever answer for.
/// `cellSize` at zoom 30 is ~3.7 cm/px; a window smaller than one tile at
/// that level is far below any tiled vector source's own resolution, and
/// the cap keeps the `1u64 << z` shift in [`matrix_side`] well inside its
/// own range for a degenerate (near-zero-span) window.
const MAX_NATIVE_ZOOM: u8 = 30;

/// The `WebMercatorQuad` resolution (meters per output pixel) at which a
/// window whose longest side spans `span_m` meters is rendered at the tile
/// grid's OWN native scale: the `cellSize` of the coarsest zoom level whose
/// single tile is no larger than that span. Equivalently, the level at
/// which the window is exactly one tile wide, give or take the halving step
/// — so a window rendered at this resolution is always between
/// [`TILE_SIZE_PX`] and `2 * TILE_SIZE_PX` pixels on its longest side, with
/// no pixel count invented anywhere: the number falls out of the tile
/// matrix set's own published `cellSize` progression and the window itself.
///
/// `None` for a non-finite or non-positive span (a degenerate window
/// `maps::parse_request` refuses before this is ever reached) rather than a
/// substituted value.
pub(crate) fn native_resolution_m_per_px(span_m: f64) -> Option<f64> {
    if !span_m.is_finite() || span_m <= 0.0 {
        return None;
    }
    // Coarsest z with `tile_size_m(z) <= span_m`, i.e. `2^z >= 2 * ORIGIN /
    // span_m` — the same halving progression `tile_size_m` itself walks.
    let exact = ((2.0 * WEB_MERCATOR_ORIGIN) / span_m).log2().ceil();
    if !exact.is_finite() {
        return None;
    }
    let zoom = exact.clamp(0.0, f64::from(MAX_NATIVE_ZOOM)) as u8;
    Some(cell_size_m(zoom))
}

/// Picks the coarsest `WebMercatorQuad` zoom level, within `[minzoom,
/// maxzoom]`, fine enough to meet or exceed `target_resolution_m_per_px`
/// (meters one output pixel represents) — the same "smallest zoom whose
/// resolution already satisfies what was asked for" rule a static-map
/// renderer picks a source zoom by. A non-finite or non-positive target
/// (a degenerate request `maps::parse_request` should already have refused
/// before this is ever called) falls back to `maxzoom`, the finest allowed
/// resolution, rather than an unbounded or undefined zoom.
pub(crate) fn pick_zoom(target_resolution_m_per_px: f64, minzoom: u8, maxzoom: u8) -> u8 {
    if !target_resolution_m_per_px.is_finite() || target_resolution_m_per_px <= 0.0 {
        return maxzoom;
    }
    let zoom0_resolution = cell_size_m(0);
    let ideal = (zoom0_resolution / target_resolution_m_per_px)
        .log2()
        .ceil();
    let ideal_zoom = if ideal.is_finite() {
        ideal.clamp(0.0, u32::from(u8::MAX) as f64) as u32
    } else {
        u32::from(maxzoom)
    };
    ideal_zoom.clamp(u32::from(minzoom), u32::from(maxzoom)) as u8
}

/// Inclusive tile column/row range, at zoom `z`, whose `WebMercatorQuad`
/// tiles intersect `bbox_m` (`[minx, miny, maxx, maxy]`, mercator meters) —
/// `(min_col, max_col, min_row, max_row)`, every index already clamped to
/// this zoom's own `[0, matrixSide - 1]` bounds (`row`/`col` increase
/// south/east, row 0 at the north edge, the same XYZ convention
/// `tellurion-tiles::handlers::parse_tile_coord` already assumes).
pub(crate) fn covering_tiles(bbox_m: [f64; 4], z: u8) -> (u32, u32, u32, u32) {
    let [minx, miny, maxx, maxy] = bbox_m;
    let side = matrix_side(z);
    let max_index = (side - 1) as u32;
    let tile_size = tile_size_m(z);

    let col_for = |x: f64| -> u32 {
        let raw = ((x + WEB_MERCATOR_ORIGIN) / tile_size).floor();
        clamp_index(raw, max_index)
    };
    let row_for = |y: f64| -> u32 {
        let raw = ((WEB_MERCATOR_ORIGIN - y) / tile_size).floor();
        clamp_index(raw, max_index)
    };

    let min_col = col_for(minx);
    let max_col = col_for(maxx);
    // North (top) edge is the SMALLER row index — `row_for` already follows
    // the south/east-increasing convention, so the min/max ROW swap
    // relative to the min/max Y (mercator meters increase northward, rows
    // increase southward).
    let min_row = row_for(maxy);
    let max_row = row_for(miny);

    (min_col, max_col, min_row, max_row)
}

fn clamp_index(raw: f64, max_index: u32) -> u32 {
    if !raw.is_finite() || raw <= 0.0 {
        0
    } else if raw >= f64::from(max_index) {
        max_index
    } else {
        raw as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tilematrixset::web_mercator_quad_matrices;

    #[test]
    fn earth_radius_matches_the_standard_epsg_3857_sphere() {
        // Verified 2026-07 against the published WGS84 semi-major axis used
        // as the spherical Web Mercator radius (6,378,137.0 m).
        assert!((earth_radius_m() - 6_378_137.0).abs() < 1e-6);
    }

    #[test]
    fn forward_of_the_origin_is_the_mercator_origin() {
        let (x, y) = forward(0.0, 0.0);
        assert!(x.abs() < 1e-6);
        assert!(y.abs() < 1e-6);
    }

    #[test]
    fn forward_then_inverse_round_trips() {
        for (lon, lat) in [
            (0.0, 0.0),
            (-122.4194, 37.7749), // San Francisco
            (12.4964, 41.9028),   // Rome
            (179.9, -70.0),
        ] {
            let (x, y) = forward(lon, lat);
            let (lon2, lat2) = inverse(x, y);
            assert!((lon - lon2).abs() < 1e-6, "lon {lon} -> {lon2}");
            assert!((lat - lat2).abs() < 1e-6, "lat {lat} -> {lat2}");
        }
    }

    #[test]
    fn forward_of_the_max_web_mercator_latitude_reaches_the_projection_origin_edge() {
        // The well-known Web Mercator latitude bound, where y equals half
        // the projected world extent — pinned through `MAX_LATITUDE_DEG`
        // itself, the constant `maps::collection_window` clamps a
        // collection's own CRS84 extent to before projecting it.
        assert!((85.051_128_779_806_59 - MAX_LATITUDE_DEG).abs() < f64::EPSILON);
        let (_, y) = forward(0.0, MAX_LATITUDE_DEG);
        assert!((y - WEB_MERCATOR_ORIGIN).abs() < 1e-3);
    }

    #[test]
    fn tile_bounds_at_zoom_zero_is_the_whole_world() {
        let bounds = tile_bounds_m(0, 0, 0);
        assert!((bounds[0] - -WEB_MERCATOR_ORIGIN).abs() < 1e-6);
        assert!((bounds[1] - -WEB_MERCATOR_ORIGIN).abs() < 1e-6);
        assert!((bounds[2] - WEB_MERCATOR_ORIGIN).abs() < 1e-6);
        assert!((bounds[3] - WEB_MERCATOR_ORIGIN).abs() < 1e-6);
    }

    #[test]
    fn tile_bounds_top_left_at_zoom_one_is_the_northwest_quadrant() {
        // z=1, x=0, y=0: the northwest quarter of the projected world.
        let bounds = tile_bounds_m(1, 0, 0);
        assert!((bounds[0] - -WEB_MERCATOR_ORIGIN).abs() < 1e-6);
        assert!((bounds[3] - WEB_MERCATOR_ORIGIN).abs() < 1e-6);
        assert!((bounds[2] - 0.0).abs() < 1e-6);
        assert!((bounds[1] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn cell_size_matches_the_published_tile_matrix_set_table() {
        let matrices = web_mercator_quad_matrices(3);
        for matrix in &matrices {
            let z: u8 = matrix.id.parse().unwrap();
            assert!(
                (cell_size_m(z) - matrix.cell_size).abs() < 1e-6,
                "zoom {z}: {} vs {}",
                cell_size_m(z),
                matrix.cell_size
            );
        }
    }

    #[test]
    fn native_resolution_renders_the_whole_world_at_exactly_one_tile() {
        let span = 2.0 * WEB_MERCATOR_ORIGIN;
        let resolution = native_resolution_m_per_px(span).unwrap();
        assert!((span / resolution - f64::from(TILE_SIZE_PX)).abs() < 1e-6);
    }

    #[test]
    fn native_resolution_keeps_every_window_between_one_and_two_tiles_wide() {
        // Every span from the whole world down to a meter-scale window
        // renders between TILE_SIZE_PX and 2 * TILE_SIZE_PX pixels wide —
        // the property the derived default output size relies on.
        let mut span = 2.0 * WEB_MERCATOR_ORIGIN;
        while span > 1.0 {
            let pixels = span / native_resolution_m_per_px(span).unwrap();
            assert!(
                (f64::from(TILE_SIZE_PX)..=2.0 * f64::from(TILE_SIZE_PX)).contains(&pixels),
                "span {span} rendered {pixels} pixels wide"
            );
            span /= 3.0;
        }
    }

    #[test]
    fn native_resolution_has_no_answer_for_a_degenerate_span() {
        assert!(native_resolution_m_per_px(0.0).is_none());
        assert!(native_resolution_m_per_px(-1.0).is_none());
        assert!(native_resolution_m_per_px(f64::NAN).is_none());
    }

    #[test]
    fn native_resolution_is_capped_at_its_finest_zoom() {
        // A sub-millimeter window cannot ask for a finer level than the cap.
        assert_eq!(
            native_resolution_m_per_px(1e-9).unwrap(),
            cell_size_m(MAX_NATIVE_ZOOM)
        );
    }

    #[test]
    fn pick_zoom_chooses_the_coarsest_zoom_meeting_the_requested_resolution() {
        // At zoom 10 the WebMercatorQuad cell size is ~152.87 m/px; asking
        // for exactly that resolution must land on zoom 10, not finer.
        let z10_resolution = cell_size_m(10);
        assert_eq!(pick_zoom(z10_resolution, 0, 14), 10);
    }

    #[test]
    fn pick_zoom_clamps_to_the_collections_configured_range() {
        assert_eq!(
            pick_zoom(0.01, 0, 5),
            5,
            "far finer than z5 clamps to maxzoom"
        );
        assert_eq!(
            pick_zoom(1_000_000_000.0, 3, 14),
            3,
            "far coarser than z3 clamps to minzoom"
        );
    }

    #[test]
    fn pick_zoom_falls_back_to_maxzoom_for_a_degenerate_target() {
        assert_eq!(pick_zoom(0.0, 0, 14), 14);
        assert_eq!(pick_zoom(f64::NAN, 0, 14), 14);
        assert_eq!(pick_zoom(-1.0, 0, 14), 14);
    }

    #[test]
    fn covering_tiles_at_zoom_zero_is_the_single_root_tile() {
        assert_eq!(
            covering_tiles(
                [
                    -WEB_MERCATOR_ORIGIN,
                    -WEB_MERCATOR_ORIGIN,
                    WEB_MERCATOR_ORIGIN,
                    WEB_MERCATOR_ORIGIN
                ],
                0
            ),
            (0, 0, 0, 0)
        );
    }

    #[test]
    fn covering_tiles_finds_the_correct_single_tile_at_zoom_one() {
        // A point in the northwest quadrant of the world at zoom 1 must
        // resolve to tile (0, 0), not any of the other three.
        let bbox = [
            -WEB_MERCATOR_ORIGIN / 2.0,
            WEB_MERCATOR_ORIGIN / 2.0,
            -WEB_MERCATOR_ORIGIN / 2.0,
            WEB_MERCATOR_ORIGIN / 2.0,
        ];
        assert_eq!(covering_tiles(bbox, 1), (0, 0, 0, 0));
    }

    #[test]
    fn covering_tiles_spans_a_bbox_crossing_a_tile_boundary() {
        // A bbox straddling the vertical center line at zoom 1 must cover
        // both column 0 and column 1.
        let bbox = [
            -WEB_MERCATOR_ORIGIN / 4.0,
            -WEB_MERCATOR_ORIGIN / 4.0,
            WEB_MERCATOR_ORIGIN / 4.0,
            WEB_MERCATOR_ORIGIN / 4.0,
        ];
        let (min_col, max_col, min_row, max_row) = covering_tiles(bbox, 1);
        assert_eq!((min_col, max_col), (0, 1));
        assert_eq!((min_row, max_row), (0, 1));
    }

    #[test]
    fn covering_tiles_clamps_an_out_of_world_bbox_to_the_matrix_bounds() {
        let bbox = [
            -WEB_MERCATOR_ORIGIN * 4.0,
            -WEB_MERCATOR_ORIGIN * 4.0,
            WEB_MERCATOR_ORIGIN * 4.0,
            WEB_MERCATOR_ORIGIN * 4.0,
        ];
        assert_eq!(covering_tiles(bbox, 2), (0, 3, 0, 3));
    }
}
