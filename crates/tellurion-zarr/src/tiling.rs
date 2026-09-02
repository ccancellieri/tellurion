//! Web Mercator tile addressing -> Zarr array pixel-window math (`#37`).
//! Pure: no file I/O, no decoding, so the window-clamping and resampling
//! logic here is unit-testable on its own. Tiles are addressed the same WMTS
//! z/x/y, top-left-origin way `tellurion-cog::tiling` (and, through it,
//! `tellurion-tiles`' own MVT lane) already does — this driver only ever
//! serves onto that grid, the same "PNG lane is MVT-first / one tile grid"
//! convention.
//!
//! [`select_overview`]/[`plan_window`] mirror `tellurion-cog::tiling`'s own
//! two functions of the same name exactly — same policy (the coarsest level
//! whose resolution is still at least as fine as the destination tile
//! needs), same reasoning, duplicated rather than shared for the same
//! "driver crates in this workspace never depend on one another" choice
//! `driver.rs`'s own module doc explains. A store with only one level (a
//! plain, non-pyramid array — [`crate::reader::ZarrMeta::levels`] has a
//! single entry) degrades to exactly this crate's original (`#37` first
//! slice) behavior: every tile reads that one level at native resolution,
//! world-bounds-clamped; a request whose window is too large still refuses
//! on the per-request pixel budget (`driver::check_window_budget`) rather
//! than downsampling.

use tellurion_core::TileCoord;

use crate::reader::ZarrLevel;

/// The pixel -> geographic-coordinate affine transform an array's `.zattrs`-
/// declared `tellurion:extent_crs84` implies — the same axis-aligned-only
/// shape `tellurion-cog::geokeys::GeoTransform` carries for a GeoTIFF's
/// `ModelPixelScaleTag`/`ModelTiepointTag`, duplicated here rather than
/// shared (see `driver.rs`'s own doc for why raster driver crates in this
/// workspace never depend on one another).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// Geographic X (longitude, degrees) at pixel column 0.
    pub origin_x: f64,
    /// Geographic Y (latitude, degrees) at pixel row 0.
    pub origin_y: f64,
    /// Degrees per pixel, X axis (always positive).
    pub pixel_scale_x: f64,
    /// Degrees per pixel, Y axis (always positive; row increases southward,
    /// so geographic Y at pixel row `py` is `origin_y - py * pixel_scale_y`).
    pub pixel_scale_y: f64,
}

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
/// fractional WMTS row position `row` on a `matrix_side`-wide grid at some
/// zoom — see `tellurion-cog::tiling::lat_of_row`'s own doc; same formula.
fn lat_of_row(row: f64, matrix_side: f64) -> f64 {
    let angle = std::f64::consts::PI * (1.0 - 2.0 * row / matrix_side);
    angle.sinh().atan().to_degrees()
}

/// `coord`'s geographic bounding box on the WebMercatorQuad grid.
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

/// A tile's requested source-pixel window at one resolution level.
/// `full_x0`/`full_x1` are the exact (unclamped) real-valued X extent the
/// tile's geographic bbox maps to — needed to place the read window
/// correctly inside the destination tile even when part of the tile falls
/// outside the array, X only, for the same reason
/// `tellurion-cog::tiling::WindowPlan`'s own doc gives (Web Mercator's X axis
/// is linear in longitude; its Y axis is not, so [`resample_to_tile`] inverts
/// the real projection per destination row instead). `clamped_*` is the
/// integer, world-bounds-clamped pixel rectangle actually read, IN
/// `level_index`'s OWN pixel coordinates.
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

/// Picks the coarsest level whose per-pixel resolution is still at least as
/// fine as `desired_deg_per_px` — minimizing the number of source pixels a
/// caller has to read to fill the destination tile. Exactly
/// `tellurion-cog::tiling::select_overview`'s own policy (see that
/// function's doc for the full reasoning); `levels` MUST be sorted
/// finest-first (`reader::open` guarantees this for a `multiscales`
/// pyramid, and a single-level `Vec` trivially satisfies it too), so this
/// degrades to "always level 0" for a plain, non-pyramid store with exactly
/// one level.
pub fn select_overview(
    levels: &[ZarrLevel],
    total_geo_width_deg: f64,
    desired_deg_per_px: f64,
) -> usize {
    let mut chosen = 0;
    for (index, level) in levels.iter().enumerate() {
        let deg_per_px = total_geo_width_deg / f64::from(level.width());
        if deg_per_px <= desired_deg_per_px {
            chosen = index;
        } else {
            break;
        }
    }
    chosen
}

/// Plans the read for `bbox` against `levels` (finest-first; see
/// [`select_overview`]'s own doc) — picks which level to read
/// ([`select_overview`], from `dest_size`'s own implied resolution) and the
/// world-bounds-clamped pixel window on that level. `None` means `bbox` does
/// not intersect the array's own extent at all (a legitimately empty tile);
/// world-bounds clamping otherwise always succeeds. `transform` is always
/// the FINEST level's own pixel -> CRS84 transform (`reader::ZarrMeta::
/// transform`); every level shares one geographic extent
/// (`total_geo_width_deg`/`total_geo_height_deg`, both `transform.origin_*`-
/// relative), only its own pixel count differs — the same "one shared
/// extent, many pixel counts" shape `tellurion-cog::tiling::plan_window`
/// already uses for a COG's own overview pyramid.
pub fn plan_window(
    levels: &[ZarrLevel],
    transform: &Transform,
    total_geo_width_deg: f64,
    total_geo_height_deg: f64,
    bbox: LonLatBbox,
    dest_size: u32,
) -> Option<WindowPlan> {
    let desired_deg_per_px = (bbox.max_lon - bbox.min_lon) / f64::from(dest_size);
    let level_index = select_overview(levels, total_geo_width_deg, desired_deg_per_px);
    let level = &levels[level_index];

    let scale_x = total_geo_width_deg / f64::from(level.width());
    let scale_y = total_geo_height_deg / f64::from(level.height());

    let full_x0 = (bbox.min_lon - transform.origin_x) / scale_x;
    let full_x1 = (bbox.max_lon - transform.origin_x) / scale_x;
    // Geographic Y decreases as pixel row increases (raster convention),
    // hence `origin_y - lat` rather than `lat - origin_y`.
    let full_y0 = (transform.origin_y - bbox.max_lat) / scale_y;
    let full_y1 = (transform.origin_y - bbox.min_lat) / scale_y;

    let clamp_x = |v: f64| v.clamp(0.0, f64::from(level.width()));
    let clamp_y = |v: f64| v.clamp(0.0, f64::from(level.height()));

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

/// Nearest-neighbor resamples `samples` (a `win_w x win_h` buffer covering
/// `plan`'s clamped read window) onto a `dest_size x dest_size`
/// WebMercatorQuad tile canvas — warping `coord`'s own tile grid onto
/// `plan`'s CRS84 source pixels. `None` at a destination pixel means it falls
/// outside `plan`'s clamped window entirely (a tile only partially covered by
/// the array); the caller renders that as transparent, never a guessed
/// value. See `tellurion-cog::tiling::resample_to_tile`'s own doc for why
/// columns interpolate linearly while rows invert the true per-row Mercator
/// projection instead of a straight corner-to-corner chord.
pub fn resample_to_tile(
    samples: &[f64],
    win_w: u32,
    win_h: u32,
    plan: &WindowPlan,
    coord: TileCoord,
    dest_size: u32,
    origin_y: f64,
) -> Vec<Option<f64>> {
    let mut dest = vec![None; dest_size as usize * dest_size as usize];
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

            let src_off = sy as usize * win_w as usize + sx as usize;
            let dst_off = dy as usize * dest_size as usize + dx as usize;
            dest[dst_off] = Some(samples[src_off]);
        }
    }
    dest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{Compressor, DType};

    #[test]
    fn tile_zero_zero_zero_covers_the_whole_world() {
        let bbox = tile_lonlat_bbox(TileCoord { z: 0, x: 0, y: 0 });
        assert!((bbox.min_lon - -180.0).abs() < 1e-9);
        assert!((bbox.max_lon - 180.0).abs() < 1e-9);
        assert!((bbox.max_lat - 85.051_128_78).abs() < 1e-6);
        assert!((bbox.min_lat - -85.051_128_78).abs() < 1e-6);
    }

    fn transform() -> Transform {
        Transform {
            origin_x: -10.0,
            origin_y: 10.0,
            pixel_scale_x: 0.01,
            pixel_scale_y: 0.01,
        }
    }

    fn level(width: u64, height: u64) -> ZarrLevel {
        ZarrLevel {
            shape: vec![height, width],
            chunks: vec![height, width],
            dtype: DType::U8,
            compressor: Compressor::Raw,
            fill_value: 0.0,
            dimension_separator: ".".to_string(),
            path: String::new(),
        }
    }

    #[test]
    fn select_overview_picks_the_coarsest_level_that_is_still_fine_enough() {
        // Level 0: 0.01 deg/px; level 1: 0.02 deg/px; level 2: 0.04 deg/px.
        let levels = vec![level(1000, 1000), level(500, 500), level(250, 250)];
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
            select_overview(&levels, total_deg, 0.02),
            1,
            "exact level 1 match"
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

    #[test]
    fn select_overview_always_returns_zero_for_a_single_level_non_pyramid_store() {
        let levels = vec![level(2000, 2000)];
        assert_eq!(select_overview(&levels, 20.0, 0.00001), 0);
        assert_eq!(select_overview(&levels, 20.0, 1000.0), 0);
    }

    #[test]
    fn plan_window_is_none_when_the_bbox_never_touches_the_array() {
        let levels = vec![level(2000, 2000)];
        let bbox = LonLatBbox {
            min_lon: 50.0,
            max_lon: 51.0,
            min_lat: 50.0,
            max_lat: 51.0,
        };
        assert!(plan_window(&levels, &transform(), 20.0, 20.0, bbox, 256).is_none());
    }

    #[test]
    fn plan_window_clamps_a_tile_straddling_the_array_edge() {
        // Array covers lon [-10,10], lat [-10,10]; bbox straddles the
        // eastern edge, half inside, half outside.
        let levels = vec![level(2000, 2000)];
        let bbox = LonLatBbox {
            min_lon: 5.0,
            max_lon: 15.0,
            min_lat: -1.0,
            max_lat: 1.0,
        };
        let plan = plan_window(&levels, &transform(), 20.0, 20.0, bbox, 256).unwrap();
        assert_eq!(plan.level_index, 0);
        assert!(
            plan.full_x1 > 2000.0,
            "unclamped extent reaches past the array"
        );
        assert_eq!(plan.clamped_x1, 2000);
        assert!(plan.clamped_x0 < plan.clamped_x1);
    }

    #[test]
    fn plan_window_covers_the_full_array_for_a_tile_that_fully_contains_it() {
        let levels = vec![level(2000, 2000)];
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
    fn plan_window_picks_a_coarser_level_for_a_low_zoom_request_over_a_pyramid() {
        // Level 0 (finest): 2000px over a 20-degree-wide array => 0.01
        // deg/px. Level 1 (coarsest): 200px => 0.1 deg/px. A world-covering
        // destination tile (360 degrees over 256px, ~1.4 deg/px) is coarser
        // than either level's own resolution, so the coarsest one that still
        // qualifies (level 1) must be chosen -- not level 0.
        let levels = vec![level(2000, 2000), level(200, 200)];
        let bbox = LonLatBbox {
            min_lon: -180.0,
            max_lon: 180.0,
            min_lat: -85.0,
            max_lat: 85.0,
        };
        let plan = plan_window(&levels, &transform(), 20.0, 20.0, bbox, 256).unwrap();
        assert_eq!(
            plan.level_index, 1,
            "the coarse level should have been selected"
        );
        assert_eq!((plan.clamped_x0, plan.clamped_y0), (0, 0));
        assert_eq!((plan.clamped_x1, plan.clamped_y1), (200, 200));
    }

    #[test]
    fn resample_to_tile_places_samples_at_the_right_destination_offset_when_clamped() {
        // A 2x2 window representing the LEFT HALF of a destination tile
        // (full X extent is twice as wide) -- the right half must stay
        // `None` (transparent).
        let samples = vec![1.0, 1.0, 1.0, 1.0];
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
        let dest = resample_to_tile(&samples, 2, 2, &plan, coord, 4, origin_y);
        let px = |x: usize, y: usize| dest[y * 4 + x];
        assert_eq!(px(0, 0), Some(1.0), "left half is real data");
        assert_eq!(px(1, 0), Some(1.0));
        assert_eq!(px(2, 0), None, "right half stays transparent");
        assert_eq!(px(3, 0), None);
    }

    #[test]
    fn resample_to_tile_follows_the_true_curve_not_a_linear_corner_interpolation() {
        // One source row per degree of latitude, north pole at row 0.
        let win_h = 180u32;
        let win_w = 1u32;
        let mut samples = vec![0.0; win_h as usize];
        for (row, sample) in samples.iter_mut().enumerate() {
            *sample = row as f64;
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
            scale_y: 1.0,
        };
        let origin_y = 90.0;
        let dest_size = 256;

        let dest = resample_to_tile(&samples, win_w, win_h, &plan, coord, dest_size, origin_y);
        let row_at = |dy: u32| dest[dy as usize * dest_size as usize].unwrap();

        let top = row_at(0);
        let center = row_at(dest_size / 2);
        let bottom = row_at(dest_size - 1);

        assert!(top <= center && center <= bottom);

        let expected_top = 90.0 - bbox.max_lat;
        let expected_bottom = 90.0 - bbox.min_lat;
        assert!((top - expected_top).abs() < 1.5);
        assert!((bottom - expected_bottom).abs() < 1.5);

        let linear_center_lat = bbox.max_lat - 0.5 * (bbox.max_lat - bbox.min_lat);
        let linear_center_row = 90.0 - linear_center_lat;
        assert!(
            center < linear_center_row - 2.0,
            "center row {center} should sit well north of the naive linear estimate {linear_center_row}"
        );
    }
}
