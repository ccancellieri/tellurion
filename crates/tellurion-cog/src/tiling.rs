//! Web Mercator tile addressing -> GeoTIFF pixel-window math (`#37`). Pure:
//! no file I/O, no decoding, so the overview-selection and window-clamping
//! logic here is unit-testable without a real GeoTIFF fixture. Tiles are
//! addressed the same WMTS z/x/y, top-left-origin way `tellurion-tiles`'
//! MVT lane already does (`TileCoord`); this driver only ever serves onto
//! that grid, per this workspace's "PNG lane is MVT-first / one tile grid"
//! convention (see the project's own design doc) — there is no
//! `tileMatrixSet` choice to make here.

use tellurion_core::TileCoord;

use crate::geokeys::GeoTransform;
use crate::reader::Level;

/// A tile's geographic (lon/lat, degrees) bounding box, from the standard
/// spherical Web Mercator inverse projection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LonLatBbox {
    pub min_lon: f64,
    pub min_lat: f64,
    pub max_lon: f64,
    pub max_lat: f64,
}

/// The (non-linear) spherical Web Mercator inverse projection's latitude at
/// fractional WMTS row position `row` (whole-number `coord.y` plus, for a
/// specific destination pixel, however far into that row it sits) on a
/// `matrix_side`-wide grid at some zoom. `tile_lonlat_bbox` below calls this
/// at `row`/`row + 1` for a tile's own top/bottom edge; [`resample_to_tile`]
/// calls it at every fractional row a destination pixel's own `dy` maps to
/// — the same formula either way, since a tile's corner IS just the row
/// position at its own edge.
fn lat_of_row(row: f64, matrix_side: f64) -> f64 {
    let angle = std::f64::consts::PI * (1.0 - 2.0 * row / matrix_side);
    angle.sinh().atan().to_degrees()
}

/// `coord`'s geographic bounding box on the WebMercatorQuad grid (WMTS
/// z/x/y, top-left origin — row `y` increases southward, matching
/// `TileCoord`'s own convention throughout this workspace).
pub fn tile_lonlat_bbox(coord: TileCoord) -> LonLatBbox {
    let matrix_side = 2f64.powi(i32::from(coord.z));
    let min_lon = f64::from(coord.x) / matrix_side * 360.0 - 180.0;
    let max_lon = (f64::from(coord.x) + 1.0) / matrix_side * 360.0 - 180.0;
    LonLatBbox {
        min_lon,
        max_lon,
        max_lat: lat_of_row(f64::from(coord.y), matrix_side),
        min_lat: lat_of_row(f64::from(coord.y) + 1.0, matrix_side),
    }
}

/// Picks the coarsest overview level whose per-pixel resolution is still at
/// least as fine as `desired_deg_per_px` — minimizing the number of source
/// pixels a caller has to read to fill the destination tile. `levels` MUST
/// be sorted finest-first (widest width first; `reader::CogMeta::open`
/// guarantees this), so resolution is monotonically coarsening through the
/// slice and the first level that fails the test means every later one does
/// too. Falls back to the finest level (index `0`) when even that is
/// coarser than requested — the caller upsamples via nearest-neighbor
/// rather than this function pretending a finer level exists.
pub fn select_overview(
    levels: &[Level],
    total_geo_width_deg: f64,
    desired_deg_per_px: f64,
) -> usize {
    let mut chosen = 0;
    for (index, level) in levels.iter().enumerate() {
        let deg_per_px = total_geo_width_deg / f64::from(level.width);
        if deg_per_px <= desired_deg_per_px {
            chosen = index;
        } else {
            break;
        }
    }
    chosen
}

/// A tile's requested source-pixel window at one overview level. `full_x0`/
/// `full_x1` are the exact (unclamped) real-valued X extent the tile's
/// geographic bbox maps to — needed to place the read window correctly
/// inside the destination tile even when part of the tile falls outside the
/// raster — X only: Web Mercator's X axis is a direct linear rescale of
/// longitude, the same as an EPSG:4326 raster's own pixel columns, so a
/// destination column's source column is always a linear interpolation
/// between these two (see [`resample_to_tile`]). There is no `full_y0`/
/// `full_y1` counterpart: Web Mercator's Y axis is non-linear in latitude,
/// so [`resample_to_tile`] finds each destination ROW's true source row by
/// inverting the real projection for that row (`scale_y`, this raster
/// level's own degrees-per-pixel), not by interpolating two corner values.
/// `clamped_x0`/`clamped_y0`/`clamped_x1`/`clamped_y1` are the integer,
/// world-bounds-clamped pixel rectangle actually read (`[x0, x1)` x
/// `[y0, y1)`, half-open).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowPlan {
    pub level_index: usize,
    pub full_x0: f64,
    pub full_x1: f64,
    pub clamped_x0: u32,
    pub clamped_y0: u32,
    pub clamped_x1: u32,
    pub clamped_y1: u32,
    /// This level's degrees-of-latitude-per-source-pixel-row — the divisor
    /// [`resample_to_tile`] uses to turn an inverted-Mercator latitude back
    /// into a source pixel row.
    pub scale_y: f64,
}

/// Plans the read for `bbox` against `levels`. `Ok(None)` — no, this returns
/// `Option`, not `Result`: `None` means `bbox` does not intersect the
/// raster's own extent at all (a legitimately empty tile); everything else
/// (overview choice, world-bounds clamping) always succeeds, since `bbox`
/// itself is never invalid input here.
pub fn plan_window(
    levels: &[Level],
    transform: &GeoTransform,
    total_geo_width_deg: f64,
    total_geo_height_deg: f64,
    bbox: LonLatBbox,
    dest_size: u32,
) -> Option<WindowPlan> {
    let desired_deg_per_px = (bbox.max_lon - bbox.min_lon) / f64::from(dest_size);
    let level_index = select_overview(levels, total_geo_width_deg, desired_deg_per_px);
    let level = &levels[level_index];

    let scale_x = total_geo_width_deg / f64::from(level.width);
    let scale_y = total_geo_height_deg / f64::from(level.height);

    let full_x0 = (bbox.min_lon - transform.origin_x) / scale_x;
    let full_x1 = (bbox.max_lon - transform.origin_x) / scale_x;
    // Geographic Y decreases as pixel row increases (raster convention),
    // hence `origin_y - lat` rather than `lat - origin_y`; `max_lat` maps to
    // the smaller (northern, top) pixel row.
    let full_y0 = (transform.origin_y - bbox.max_lat) / scale_y;
    let full_y1 = (transform.origin_y - bbox.min_lat) / scale_y;

    let clamp_x = |v: f64| v.clamp(0.0, f64::from(level.width));
    let clamp_y = |v: f64| v.clamp(0.0, f64::from(level.height));

    let clamped_x0 = clamp_x(full_x0.min(full_x1)).floor() as u32;
    let clamped_x1 = clamp_x(full_x0.max(full_x1)).ceil() as u32;
    let clamped_y0 = clamp_y(full_y0.min(full_y1)).floor() as u32;
    let clamped_y1 = clamp_y(full_y0.max(full_y1)).ceil() as u32;

    if clamped_x1 <= clamped_x0 || clamped_y1 <= clamped_y0 {
        return None;
    }

    Some(WindowPlan {
        level_index,
        full_x0,
        full_x1,
        clamped_x0,
        clamped_y0,
        clamped_x1,
        clamped_y1,
        scale_y,
    })
}

/// Nearest-neighbor resamples `window_rgba` (a `win_w x win_h` RGBA8 buffer
/// covering `plan`'s clamped read window) onto a `dest_size x dest_size`
/// WebMercatorQuad tile canvas (`#92`) — warping `coord`'s own tile grid
/// onto `plan`'s EPSG:4326 source pixels, so a tile only partially covered
/// by the raster places real pixels at the right spot and leaves the rest
/// transparent (`[0, 0, 0, 0]`), rather than stretching the clamped data
/// across the whole tile.
///
/// Columns are a straight linear interpolation across `plan`'s unclamped X
/// extent (`full_x0`/`full_x1`): Web Mercator's X axis is a direct linear
/// rescale of longitude, exactly like an EPSG:4326 raster's own pixel
/// columns, so composing the two stays linear. Rows are NOT: Web Mercator's
/// Y axis is linear in *projected* space but non-linear in latitude, while
/// an EPSG:4326 raster's rows are linear in latitude — so each destination
/// row's true source row comes from inverting the real Mercator projection
/// for that row's own fractional position on `coord`'s tile grid
/// (`origin_y`/`plan.scale_y` turn the resulting latitude into a source
/// pixel row), not from interpolating the tile's two corner rows in a
/// straight line. That flattened-corner shortcut is exact only exactly at
/// the equator; it gets worse approaching the poles and at low zoom levels,
/// where a single tile spans many degrees of latitude non-uniformly.
pub fn resample_to_tile(
    window_rgba: &[u8],
    win_w: u32,
    win_h: u32,
    plan: &WindowPlan,
    coord: TileCoord,
    dest_size: u32,
    origin_y: f64,
) -> Vec<u8> {
    let mut dest = vec![0u8; dest_size as usize * dest_size as usize * 4];
    let full_w = plan.full_x1 - plan.full_x0;
    let matrix_side = 2f64.powi(i32::from(coord.z));

    for dy in 0..dest_size {
        let row = f64::from(coord.y) + (f64::from(dy) + 0.5) / f64::from(dest_size);
        let lat = lat_of_row(row, matrix_side);
        let src_y = (origin_y - lat) / plan.scale_y;
        if src_y < f64::from(plan.clamped_y0) || src_y >= f64::from(plan.clamped_y1) {
            continue;
        }
        let sy = ((src_y.floor() as u32).saturating_sub(plan.clamped_y0)).min(win_h - 1);

        for dx in 0..dest_size {
            let src_x = plan.full_x0 + (f64::from(dx) + 0.5) / f64::from(dest_size) * full_w;
            if src_x < f64::from(plan.clamped_x0) || src_x >= f64::from(plan.clamped_x1) {
                continue;
            }
            let sx = ((src_x.floor() as u32).saturating_sub(plan.clamped_x0)).min(win_w - 1);

            let src_off = (sy as usize * win_w as usize + sx as usize) * 4;
            let dst_off = (dy as usize * dest_size as usize + dx as usize) * 4;
            dest[dst_off..dst_off + 4].copy_from_slice(&window_rgba[src_off..src_off + 4]);
        }
    }
    dest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(width: u32, height: u32, tile: u32) -> Level {
        Level {
            ifd_index: 0,
            width,
            height,
            tile_width: tile,
            tile_height: tile,
        }
    }

    #[test]
    fn tile_zero_zero_zero_covers_the_whole_world() {
        let bbox = tile_lonlat_bbox(TileCoord { z: 0, x: 0, y: 0 });
        assert!((bbox.min_lon - -180.0).abs() < 1e-9);
        assert!((bbox.max_lon - 180.0).abs() < 1e-9);
        // Web Mercator's max latitude (~85.0511 degrees).
        assert!((bbox.max_lat - 85.051_128_78).abs() < 1e-6);
        assert!((bbox.min_lat - -85.051_128_78).abs() < 1e-6);
    }

    #[test]
    fn zoom_one_quadrants_split_the_world_into_four() {
        let nw = tile_lonlat_bbox(TileCoord { z: 1, x: 0, y: 0 });
        let ne = tile_lonlat_bbox(TileCoord { z: 1, x: 1, y: 0 });
        assert!((nw.max_lon - 0.0).abs() < 1e-9);
        assert!((ne.min_lon - 0.0).abs() < 1e-9);
        assert!((nw.max_lat - ne.max_lat).abs() < 1e-12);
    }

    #[test]
    fn select_overview_picks_the_coarsest_level_that_is_still_fine_enough() {
        // Level 0: 0.01 deg/px; level 1: 0.02 deg/px; level 2: 0.04 deg/px.
        let levels = vec![
            level(1000, 1000, 128),
            level(500, 500, 128),
            level(250, 250, 128),
        ];
        let total_deg = 10.0; // 1000 * 0.01
        assert_eq!(
            select_overview(&levels, total_deg, 0.005),
            0,
            "finer than level 0 => use level 0"
        );
        assert_eq!(
            select_overview(&levels, total_deg, 0.01),
            0,
            "exact level 0 match"
        );
        assert_eq!(
            select_overview(&levels, total_deg, 0.015),
            0,
            "between 0 and 1, level 0 is finer, still <= desired only at boundary"
        );
        assert_eq!(
            select_overview(&levels, total_deg, 0.02),
            1,
            "exact level 1 match"
        );
        assert_eq!(
            select_overview(&levels, total_deg, 0.03),
            1,
            "between 1 and 2, level 1 still qualifies, level 2 doesn't yet"
        );
        assert_eq!(
            select_overview(&levels, total_deg, 0.04),
            2,
            "exact level 2 match"
        );
        assert_eq!(
            select_overview(&levels, total_deg, 100.0),
            2,
            "much coarser than needed => coarsest level"
        );
    }

    fn transform() -> GeoTransform {
        GeoTransform {
            origin_x: -10.0,
            origin_y: 10.0,
            pixel_scale_x: 0.01,
            pixel_scale_y: 0.01,
        }
    }

    #[test]
    fn plan_window_is_none_when_the_bbox_never_touches_the_raster() {
        let levels = vec![level(2000, 2000, 128)]; // covers [-10,-10]..[10,10]
        let bbox = LonLatBbox {
            min_lon: 50.0,
            max_lon: 51.0,
            min_lat: 50.0,
            max_lat: 51.0,
        };
        assert!(plan_window(&levels, &transform(), 20.0, 20.0, bbox, 256).is_none());
    }

    #[test]
    fn plan_window_clamps_a_tile_straddling_the_raster_edge() {
        let levels = vec![level(2000, 2000, 128)]; // covers lon [-10,10], lat [-10,10]
                                                   // Straddles the eastern edge: half inside, half outside.
        let bbox = LonLatBbox {
            min_lon: 5.0,
            max_lon: 15.0,
            min_lat: -1.0,
            max_lat: 1.0,
        };
        let plan = plan_window(&levels, &transform(), 20.0, 20.0, bbox, 256).unwrap();
        assert_eq!(plan.level_index, 0);
        // Full (unclamped) x1 would be at pixel 2500 (15 degrees past -10 at
        // 0.01 deg/px); clamped to the raster's own width (2000).
        assert!(
            plan.full_x1 > 2000.0,
            "unclamped extent reaches past the raster"
        );
        assert_eq!(plan.clamped_x1, 2000);
        assert!(plan.clamped_x0 < plan.clamped_x1);
    }

    #[test]
    fn plan_window_covers_the_full_raster_for_a_tile_that_fully_contains_it() {
        let levels = vec![level(2000, 2000, 128)];
        let bbox = LonLatBbox {
            min_lon: -20.0,
            max_lon: 20.0,
            min_lat: -20.0,
            max_lat: 20.0,
        };
        let plan = plan_window(&levels, &transform(), 20.0, 20.0, bbox, 256).unwrap();
        assert_eq!((plan.clamped_x0, plan.clamped_y0), (0, 0));
        assert_eq!((plan.clamped_x1, plan.clamped_y1), (2000, 2000));
    }

    #[test]
    fn resample_to_tile_places_pixels_at_the_right_destination_offset_when_clamped() {
        // A 2x2 source window, entirely red, representing the LEFT HALF of a
        // destination tile (full X extent is twice as wide as the clamped
        // window) -- the right half must stay transparent. Only X-axis
        // clamping is under test here: `origin_y`/`scale_y` are derived from
        // the tile's own true latitude bbox so its entire angular height
        // maps to exactly the window's 2 source rows, keeping every
        // destination row inside the window regardless of the true per-row
        // curve (that curve is what the two tests below exercise).
        let window_rgba = vec![
            255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
        ];
        let coord = TileCoord { z: 4, x: 3, y: 5 };
        let bbox = tile_lonlat_bbox(coord);
        let origin_y = bbox.max_lat;
        let scale_y = (bbox.max_lat - bbox.min_lat) / 2.0;
        let plan = WindowPlan {
            level_index: 0,
            full_x0: 0.0,
            full_x1: 4.0,
            clamped_x0: 0,
            clamped_y0: 0,
            clamped_x1: 2,
            clamped_y1: 2,
            scale_y,
        };
        let dest = resample_to_tile(&window_rgba, 2, 2, &plan, coord, 4, origin_y);
        let px = |x: usize, y: usize| -> [u8; 4] {
            let off = (y * 4 + x) * 4;
            [dest[off], dest[off + 1], dest[off + 2], dest[off + 3]]
        };
        assert_eq!(px(0, 0), [255, 0, 0, 255], "left half is real data");
        assert_eq!(px(1, 0), [255, 0, 0, 255]);
        assert_eq!(px(2, 0), [0, 0, 0, 0], "right half stays transparent");
        assert_eq!(px(3, 0), [0, 0, 0, 0]);
    }

    /// `#92`: the pre-fix behavior (flatten a tile's two corner latitudes
    /// into a straight line, then interpolate) diverges sharply from the
    /// true per-row Mercator inverse near the pole, at a low zoom where a
    /// single tile spans many degrees of latitude non-uniformly.
    #[test]
    fn lat_of_row_diverges_from_a_linear_corner_interpolation_near_the_pole() {
        // z=2, y=0: this tile spans [66.51, 85.05] degrees latitude -- right
        // at the world's own top edge, the most non-linear band there is.
        let coord = TileCoord { z: 2, x: 0, y: 0 };
        let bbox = tile_lonlat_bbox(coord);
        let matrix_side = 4.0;
        let dest_size = 256;
        let dy = dest_size / 2;

        let t = (f64::from(dy) + 0.5) / f64::from(dest_size);
        let row = f64::from(coord.y) + t;
        let true_lat = lat_of_row(row, matrix_side);

        // The pre-`#92` behavior: a straight chord between the tile's own
        // two corner latitudes.
        let linear_lat = bbox.max_lat - t * (bbox.max_lat - bbox.min_lat);

        assert!(
            (true_lat - linear_lat).abs() > 1.0,
            "true_lat={true_lat}, linear_lat={linear_lat}: expected a large divergence near the pole"
        );
        assert!(
            true_lat > linear_lat,
            "the true curve bulges toward the pole relative to the straight chord"
        );
    }

    /// The same divergence, proven through the actual public resampling
    /// entry point on a synthetic "known raster" — corner and center pixel
    /// assertions (`#92`'s own test requirement) rather than the bare
    /// latitude math above.
    #[test]
    fn resample_to_tile_follows_the_true_curve_not_a_linear_corner_interpolation() {
        // One source row per degree of latitude, north pole at row 0; each
        // row's own gray value equals its row index, so the resolved pixel
        // color reveals exactly which source row got sampled.
        let win_h = 180u32;
        let win_w = 1u32;
        let mut window_rgba = vec![0u8; win_h as usize * 4];
        for row in 0..win_h {
            let off = row as usize * 4;
            window_rgba[off..off + 4].copy_from_slice(&[row as u8, row as u8, row as u8, 255]);
        }

        let coord = TileCoord { z: 2, x: 0, y: 0 }; // near-pole: lat [66.51, 85.05]
        let bbox = tile_lonlat_bbox(coord);
        let plan = WindowPlan {
            level_index: 0,
            full_x0: 0.0,
            full_x1: 1.0,
            clamped_x0: 0,
            clamped_y0: 0,
            clamped_x1: win_w,
            clamped_y1: win_h,
            scale_y: 1.0, // one source row per degree of latitude
        };
        let origin_y = 90.0; // pixel row 0 == latitude 90
        let dest_size = 256;

        let dest = resample_to_tile(
            &window_rgba,
            win_w,
            win_h,
            &plan,
            coord,
            dest_size,
            origin_y,
        );
        let row_at = |dy: u32| -> u8 { dest[(dy as usize * dest_size as usize) * 4] };

        let top = row_at(0);
        let center = row_at(dest_size / 2);
        let bottom = row_at(dest_size - 1);

        // North to south stays monotonic even though rows are no longer
        // linearly interpolated.
        assert!(top <= center && center <= bottom);

        // The two corners (pixel CENTERS of the first/last destination row,
        // not the tile's exact boundary) land close to the tile's own true
        // edge latitudes.
        let expected_top = 90.0 - bbox.max_lat;
        let expected_bottom = 90.0 - bbox.min_lat;
        assert!((f64::from(top) - expected_top).abs() < 1.5);
        assert!((f64::from(bottom) - expected_bottom).abs() < 1.5);

        // The center: the naive pre-`#92` linear-corner estimate lands
        // several whole source rows south of where the true curve does —
        // not a rounding-level difference.
        let linear_center_lat = bbox.max_lat - 0.5 * (bbox.max_lat - bbox.min_lat);
        let linear_center_row = 90.0 - linear_center_lat;
        assert!(
            f64::from(center) < linear_center_row - 2.0,
            "center row {center} should sit well north of the naive linear estimate {linear_center_row}"
        );
    }
}
