//! Derived tiles-serving-parameter heuristics (`#19`): formulas that turn a
//! collection's descriptor — specifically its `row_estimate` — into per-zoom
//! feature caps, a per-zoom simplification tolerance, and an MVT tile
//! buffer. `tellurion-postgis`'s tile query used to either take these from
//! explicit config or apply a single flat constant; this module centralizes
//! the "otherwise" case so it exists exactly once, documented, instead of
//! scattered across driver crates.
//!
//! Precedence is unchanged from before this module existed: an explicit
//! `TilesConf.caps` entry for a zoom (or one inherited from the nearest
//! lower configured zoom — see [`ZoomCaps::explicit`]) always wins; a
//! heuristic only fills the gap when a collection's config says nothing at
//! all about a zoom. There is no config override for tolerance or buffer
//! (there never was one) — the heuristic simply replaces what used to be a
//! bare per-zoom formula (tolerance) or a hardcoded literal (buffer) with a
//! documented one; behavior for existing collections is unchanged either way
//! since neither ever had a config-tunable value to diverge from.
//!
//! ## Feature cap ([`feature_cap`], [`effective_feature_cap`])
//!
//! Grows geometrically with zoom — a low-zoom tile spans a huge area and
//! should stay compact and legible; a high-zoom tile spans a small area and
//! can afford to show more — starting from [`MIN_FEATURE_CAP`], doubling
//! every [`CAP_ZOOM_DOUBLING_STEP`] zoom levels, and clamped so it never
//! exceeds the collection's own row estimate (capping above what actually
//! exists changes nothing) nor [`MAX_FEATURE_CAP`] (an upper bound on how
//! large a single tile payload is allowed to grow regardless of density).
//!
//! ## Simplification tolerance ([`simplify_tolerance_meters`])
//!
//! The Web Mercator ground distance one 256px tile pixel covers at a given
//! zoom — geometry detail finer than one pixel is invisible at that zoom, so
//! `ST_SimplifyPreserveTopology` can drop it for free. Zoom-only; no row
//! estimate involved. (Unchanged from the formula this module now
//! centralizes — see the struct-free history in `tellurion-postgis`.)
//!
//! ## Tile buffer ([`tile_buffer_px`])
//!
//! A fixed fraction of the MVT extent: geometry outside a tile's core square
//! but within this margin is still included in the query, so a stroke or
//! label crossing a tile boundary doesn't visibly clip at the seam.
//! Extent-only; no zoom or density involved.

use crate::config::{ZoomCaps, DEFAULT_TILE_CAP};

/// Feature cap floor: even an all-but-empty collection gets at least this
/// many features per tile before the cap can start binding — a cap this low
/// would truncate normal-density data for no benefit.
pub const MIN_FEATURE_CAP: u64 = 500;

/// Feature cap ceiling: independent of row estimate, no single tile is
/// allowed to grow past this many features regardless of zoom or density.
pub const MAX_FEATURE_CAP: u64 = 50_000;

/// The cap doubles every this many zoom levels between [`MIN_FEATURE_CAP`]
/// (at zoom 0) and [`MAX_FEATURE_CAP`].
const CAP_ZOOM_DOUBLING_STEP: u32 = 2;

/// Derived per-zoom feature cap (see the module docs' "Feature cap" section)
/// for a collection with `row_estimate` rows total. Pure formula — has no
/// notion of "explicit config"; [`effective_feature_cap`] decides whether to
/// call this at all.
pub fn feature_cap(zoom: u8, row_estimate: u64) -> u64 {
    let doublings = u32::from(zoom) / CAP_ZOOM_DOUBLING_STEP;
    // `.min(63)` is a defensive shift-overflow guard, not a realistic case:
    // config validation caps `zoom` at 24, so `doublings` never exceeds 12.
    let grown = MIN_FEATURE_CAP.saturating_mul(1u64 << doublings.min(63));
    let ceiling = row_estimate.clamp(MIN_FEATURE_CAP, MAX_FEATURE_CAP);
    grown.clamp(MIN_FEATURE_CAP, MAX_FEATURE_CAP).min(ceiling)
}

/// Precedence-resolved per-zoom feature cap (`#19`): [`ZoomCaps::explicit`]
/// (an operator-configured cap for this zoom, or one inherited from the
/// nearest lower configured zoom) always wins; only when the collection's
/// `caps` has nothing configured at or below `zoom` does `row_estimate` —
/// when known — feed [`feature_cap`]'s heuristic; [`DEFAULT_TILE_CAP`]
/// applies only when neither is available. This is the single place
/// precedence is decided; `feature_cap` itself has no notion of "explicit".
pub fn effective_feature_cap(caps: &ZoomCaps, zoom: u8, row_estimate: Option<u64>) -> u64 {
    caps.explicit(zoom)
        .unwrap_or_else(|| row_estimate.map_or(DEFAULT_TILE_CAP, |rows| feature_cap(zoom, rows)))
}

/// Ground distance (meters, Web Mercator / EPSG:3857) one 256px tile pixel
/// covers at `zoom` — the simplification tolerance below which geometry
/// detail is invisible and safe for `ST_SimplifyPreserveTopology` to drop.
pub fn simplify_tolerance_meters(zoom: u8) -> f64 {
    const WEB_MERCATOR_CIRCUMFERENCE_M: f64 = 40_075_016.685_578_49;
    const TILE_SIZE_PX: f64 = 256.0;
    WEB_MERCATOR_CIRCUMFERENCE_M / (TILE_SIZE_PX * 2f64.powi(i32::from(zoom)))
}

/// Density-reference mean vertex count per feature (`#102`): the
/// [`GeometryProfile::vertices`] mean at which
/// [`simplify_tolerance_meters_for_profile`]'s density scale starts raising
/// tolerance above the zoom-only baseline — roughly the vertex count of a
/// modestly detailed polygon (a few dozen boundary points). A collection
/// whose sampled features carry no more detail than this keeps today's
/// zoom-only tolerance exactly; only a collection detailed enough to have
/// real simplification headroom gets a larger one.
const DENSITY_REFERENCE_MEAN_VERTICES: f64 = 32.0;

/// Ceiling on how much larger than the zoom-only baseline
/// [`simplify_tolerance_meters_for_profile`]'s density scale is allowed to
/// grow, regardless of how dense a collection's profile reports it to be.
/// This is a general per-collection adjustment, not the vertex-budget
/// retry's own bounded raise (`tellurion-postgis`'s tile lane, `#102`) —
/// that raise stacks on top of whatever this function returns, so this
/// ceiling is kept modest on its own rather than trying to absorb both
/// concerns in one number.
const MAX_DENSITY_TOLERANCE_SCALE: f64 = 4.0;

/// [`simplify_tolerance_meters`] scaled by `profile`'s observed geometry
/// density (`#101`/`#102`): a collection whose sampled features carry
/// meaningfully more vertices than [`DENSITY_REFERENCE_MEAN_VERTICES`] gets
/// a larger tolerance at the same zoom than a sparse collection would —
/// most of that extra detail is sub-pixel wiggle
/// `ST_SimplifyPreserveTopology` can safely drop, and doing so pre-emptively
/// (rather than only reacting to a specific tile blowing its vertex budget)
/// is what closes the gap the zoom-only formula always had: two collections
/// with wildly different geometry no longer get the same tolerance at the
/// same zoom.
///
/// `profile: None` returns exactly [`simplify_tolerance_meters`]'s own
/// value, unchanged — the fallback this module's doc promises for a
/// collection with no profile. A profile whose mean vertex count is at or
/// below the reference also returns exactly the baseline: the scale never
/// drops below `1.0`, so a sparse or default-density collection's tolerance
/// is never *reduced* by having a profile at all — only a dense one's is
/// ever raised, and never by more than [`MAX_DENSITY_TOLERANCE_SCALE`].
pub fn simplify_tolerance_meters_for_profile(
    zoom: u8,
    profile: Option<crate::catalog::GeometryProfile>,
) -> f64 {
    let baseline = simplify_tolerance_meters(zoom);
    let Some(profile) = profile else {
        return baseline;
    };
    let scale = (profile.vertices.mean / DENSITY_REFERENCE_MEAN_VERTICES)
        .clamp(1.0, MAX_DENSITY_TOLERANCE_SCALE);
    baseline * scale
}

/// Volume vertex-count floor (`#41`): even a single small face gets at
/// least this many vertices of budget before the cap can start binding —
/// low enough that an ordinary building-scale solid (a few dozen faces)
/// never trips it, high enough that a truly pathological solid still gets
/// bounded.
pub const MIN_VOLUME_VERTEX_CAP: u64 = 3_000;

/// Volume vertex-count ceiling (`#41`): independent of zoom, no single
/// solid is allowed to contribute more vertices than this to one tile's
/// mesh, regardless of how the heuristic below would otherwise grow.
pub const MAX_VOLUME_VERTEX_CAP: u64 = 300_000;

/// The volume vertex cap doubles every this many zoom levels between
/// [`MIN_VOLUME_VERTEX_CAP`] (at zoom 0) and [`MAX_VOLUME_VERTEX_CAP`] — the
/// same doubling cadence [`feature_cap`] uses for MVT row counts.
const VOLUME_CAP_ZOOM_DOUBLING_STEP: u32 = 2;

/// Derived per-zoom vertex budget for one solid (`#41`'s volume complexity
/// cap — the `VolumeSource` mesh-path counterpart of [`feature_cap`]'s MVT
/// row-count cap): the same geometric growth [`feature_cap`] applies, but
/// with no row-estimate ceiling to clamp against — unlike a feature table's
/// row count, there is no "how much geometry actually exists" density
/// signal available here, so growth is purely zoom-driven between
/// [`MIN_VOLUME_VERTEX_CAP`] and [`MAX_VOLUME_VERTEX_CAP`].
pub fn volume_vertex_cap(zoom: u8) -> u64 {
    let doublings = u32::from(zoom) / VOLUME_CAP_ZOOM_DOUBLING_STEP;
    MIN_VOLUME_VERTEX_CAP
        .saturating_mul(1u64 << doublings.min(63))
        .clamp(MIN_VOLUME_VERTEX_CAP, MAX_VOLUME_VERTEX_CAP)
}

/// Precedence-resolved per-zoom vertex budget, mirroring
/// [`effective_feature_cap`]'s precedence rule: an operator-configured
/// `caps` entry for this zoom (or one inherited from the nearest lower
/// configured zoom — [`ZoomCaps::explicit`]) always wins; [`volume_vertex_cap`]'s
/// heuristic fills the gap otherwise. Unlike `effective_feature_cap`, there
/// is no flat-default third tier: a zoom-driven heuristic always has an
/// answer, so nothing here ever needs [`DEFAULT_TILE_CAP`].
pub fn effective_volume_vertex_cap(caps: &ZoomCaps, zoom: u8) -> u64 {
    caps.explicit(zoom)
        .unwrap_or_else(|| volume_vertex_cap(zoom))
}

/// MVT tile buffer, in the same units as `extent` (the MVT encoding grid,
/// not screen pixels): geometry outside a tile's core `[0, extent)` square
/// but within this margin is still included, so a stroke or label crossing a
/// tile boundary doesn't visibly clip at the seam. A fixed 1/16th of the
/// extent — the ratio `tellurion-postgis` applied as a bare literal (`256`
/// against a `4096` extent) before this module existed; expressed as a
/// formula so it scales if the extent ever changes, not because 1/16th is
/// derived from anything.
pub fn tile_buffer_px(extent: u32) -> u32 {
    extent / 16
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::time::SystemTime;

    use crate::catalog::{FeatureSizeStats, GeometryProfile, VertexStats};

    #[test]
    fn feature_cap_grows_with_zoom() {
        let low = feature_cap(0, 10_000_000);
        let high = feature_cap(10, 10_000_000);
        assert!(
            high > low,
            "a higher zoom should get a larger cap for the same dense collection"
        );
    }

    #[test]
    fn feature_cap_never_drops_below_the_floor() {
        assert_eq!(feature_cap(0, 0), MIN_FEATURE_CAP);
        assert_eq!(feature_cap(24, 0), MIN_FEATURE_CAP);
    }

    #[test]
    fn feature_cap_never_exceeds_the_ceiling() {
        assert_eq!(feature_cap(24, u64::MAX), MAX_FEATURE_CAP);
    }

    #[test]
    fn feature_cap_never_exceeds_the_row_estimate() {
        assert_eq!(
            feature_cap(20, 1_500),
            1_500,
            "capping above what the collection actually has changes nothing"
        );
    }

    #[test]
    fn effective_feature_cap_prefers_an_explicit_zoom_cap_over_the_heuristic() {
        let caps = ZoomCaps(BTreeMap::from([(0, 2_000)]));
        assert_eq!(
            effective_feature_cap(&caps, 0, Some(10_000_000)),
            2_000,
            "an explicit cap must win even though the heuristic would compute something else"
        );
    }

    #[test]
    fn effective_feature_cap_inherits_an_explicit_cap_from_a_lower_configured_zoom() {
        let caps = ZoomCaps(BTreeMap::from([(0, 2_000)]));
        assert_eq!(
            effective_feature_cap(&caps, 5, Some(10_000_000)),
            2_000,
            "zoom 5 has no cap of its own but inherits zoom 0's, exactly like ZoomCaps::effective"
        );
    }

    #[test]
    fn effective_feature_cap_falls_back_to_the_heuristic_when_nothing_is_configured() {
        let caps = ZoomCaps::default();
        let expected = feature_cap(10, 10_000_000);
        assert_eq!(effective_feature_cap(&caps, 10, Some(10_000_000)), expected);
    }

    #[test]
    fn effective_feature_cap_falls_back_to_the_flat_default_without_a_row_estimate() {
        let caps = ZoomCaps::default();
        assert_eq!(
            effective_feature_cap(&caps, 10, None),
            DEFAULT_TILE_CAP,
            "no explicit cap and no row estimate: same flat default as before this module existed"
        );
    }

    // -- volume vertex cap (`#41`) ---------------------------------------

    #[test]
    fn volume_vertex_cap_grows_with_zoom() {
        assert!(volume_vertex_cap(10) > volume_vertex_cap(0));
    }

    #[test]
    fn volume_vertex_cap_never_drops_below_the_floor() {
        assert_eq!(volume_vertex_cap(0), MIN_VOLUME_VERTEX_CAP);
    }

    #[test]
    fn volume_vertex_cap_never_exceeds_the_ceiling() {
        assert_eq!(volume_vertex_cap(24), MAX_VOLUME_VERTEX_CAP);
    }

    #[test]
    fn effective_volume_vertex_cap_prefers_an_explicit_zoom_cap_over_the_heuristic() {
        let caps = ZoomCaps(BTreeMap::from([(0, 42)]));
        assert_eq!(effective_volume_vertex_cap(&caps, 0), 42);
    }

    #[test]
    fn effective_volume_vertex_cap_inherits_from_a_lower_configured_zoom() {
        let caps = ZoomCaps(BTreeMap::from([(0, 42)]));
        assert_eq!(effective_volume_vertex_cap(&caps, 5), 42);
    }

    #[test]
    fn effective_volume_vertex_cap_falls_back_to_the_heuristic_when_nothing_is_configured() {
        let caps = ZoomCaps::default();
        assert_eq!(
            effective_volume_vertex_cap(&caps, 10),
            volume_vertex_cap(10)
        );
    }

    #[test]
    fn tolerance_shrinks_as_zoom_increases() {
        let low = simplify_tolerance_meters(0);
        let high = simplify_tolerance_meters(20);
        assert!(high < low);
        assert!(high > 0.0);
    }

    // -- profile-adaptive tolerance (`#102`) -----------------------------

    /// A `GeometryProfile` fixture with the given mean vertex count per
    /// feature — the only field `simplify_tolerance_meters_for_profile`
    /// reads; every other field is a placeholder value irrelevant to this
    /// module's own formula.
    fn profile_with_mean_vertices(mean: f64) -> GeometryProfile {
        GeometryProfile {
            sample_size: 100,
            computed_at: SystemTime::now(),
            vertices: VertexStats {
                mean,
                median: mean,
                p95: mean,
                max: mean as u64,
                total_estimated: None,
            },
            vertex_density_per_area: None,
            multi_part_fraction: 0.0,
            mean_ring_count: None,
            feature_size: FeatureSizeStats::default(),
        }
    }

    #[test]
    fn tolerance_for_profile_falls_back_to_the_zoom_only_value_without_a_profile() {
        for zoom in [0, 5, 12, 20] {
            assert_eq!(
                simplify_tolerance_meters_for_profile(zoom, None),
                simplify_tolerance_meters(zoom),
                "no profile must be byte-identical to the pre-#102 zoom-only formula"
            );
        }
    }

    #[test]
    fn tolerance_for_profile_matches_the_baseline_for_a_sparse_profile() {
        let sparse = profile_with_mean_vertices(4.0);
        assert_eq!(
            simplify_tolerance_meters_for_profile(10, Some(sparse)),
            simplify_tolerance_meters(10),
            "a profile at or below the density reference must not change the baseline"
        );
    }

    #[test]
    fn tolerance_for_profile_matches_the_baseline_exactly_at_the_reference() {
        let at_reference = profile_with_mean_vertices(DENSITY_REFERENCE_MEAN_VERTICES);
        assert_eq!(
            simplify_tolerance_meters_for_profile(10, Some(at_reference)),
            simplify_tolerance_meters(10)
        );
    }

    #[test]
    fn tolerance_for_profile_grows_for_a_dense_profile() {
        let dense = profile_with_mean_vertices(DENSITY_REFERENCE_MEAN_VERTICES * 2.0);
        let baseline = simplify_tolerance_meters(10);
        let scaled = simplify_tolerance_meters_for_profile(10, Some(dense));
        assert_eq!(scaled, baseline * 2.0);
    }

    #[test]
    fn tolerance_for_profile_is_bounded_by_the_max_density_scale() {
        let extremely_dense = profile_with_mean_vertices(DENSITY_REFERENCE_MEAN_VERTICES * 1_000.0);
        let baseline = simplify_tolerance_meters(10);
        let scaled = simplify_tolerance_meters_for_profile(10, Some(extremely_dense));
        assert_eq!(
            scaled,
            baseline * MAX_DENSITY_TOLERANCE_SCALE,
            "an extreme density must clamp at the documented ceiling, never escalate unbounded"
        );
    }

    #[test]
    fn tile_buffer_is_one_sixteenth_of_the_extent() {
        assert_eq!(tile_buffer_px(4096), 256);
    }
}
