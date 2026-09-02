//! A tile's per-request vertex-complexity accounting (`#90`): a small, pure
//! running accumulator with no I/O and no dependency on any particular
//! geometry decoder. `tellurion-postgis` gets the equivalent behavior
//! declaratively, in SQL (`sql::build_mvt_budgeted_plan`'s cumulative
//! `SUM(ST_NPoints(geom)) OVER (ORDER BY id)`); this type exists for
//! backends that build a tile by streaming individual features through Rust
//! instead — `tellurion-geopackage`'s embedded MVT encoder is the first (and
//! so far only) caller, counting each feature's vertices as it decides
//! whether to encode it, never re-decoding a geometry to count it a second
//! time (see that crate's own `count_vertices` doc for how the count itself
//! is taken from the same streaming pass the encoder already runs).
//!
//! Both enforcement sites converge on the same rule: a per-tile budget,
//! composing with (never replacing) the existing per-zoom feature cap, that
//! keeps the monotonic *prefix* of candidate geometries whose cumulative
//! vertex count still fits — the geometry that would tip the running total
//! over budget, and everything considered after it, is dropped rather than
//! ever producing an unbounded tile. This mirrors the SQL side's own
//! `WHERE running_vertices <= budget` exactly: once a `SUM(...) OVER (ORDER
//! BY ...)` running total exceeds the budget, every later row (the sum only
//! grows) fails that same test too — there is no "skip the big one, keep
//! trying smaller ones after" behavior on either backend.

/// A running per-tile vertex-count budget. `try_include` is the only way to
/// spend it; once it refuses once, it keeps refusing — see the module doc
/// for why that's the deliberate, SQL-matching behavior rather than a
/// best-effort bin-pack.
#[derive(Debug, Clone, Copy)]
pub struct VertexBudget {
    budget: u64,
    spent: u64,
    exhausted: bool,
}

impl VertexBudget {
    pub fn new(budget: u64) -> Self {
        Self {
            budget,
            spent: 0,
            exhausted: false,
        }
    }

    /// Attempts to include a geometry contributing `vertices` vertices.
    /// `true` means it fits (the running total is updated to include it);
    /// `false` means including it would push the running total past the
    /// budget — the geometry must be dropped, and every later call also
    /// returns `false` regardless of its own `vertices`, even a small one
    /// that would technically still fit under the original budget alone.
    /// This keeps the accumulator's behavior a strict, order-dependent
    /// prefix truncation, matching the SQL-side cumulative-sum plan (see
    /// the module doc) rather than a denser but harder-to-reason-about
    /// best-effort pack.
    pub fn try_include(&mut self, vertices: u64) -> bool {
        if self.exhausted {
            return false;
        }
        match self.spent.checked_add(vertices) {
            Some(total) if total <= self.budget => {
                self.spent = total;
                true
            }
            _ => {
                self.exhausted = true;
                false
            }
        }
    }

    /// The running total spent so far — every vertex from every geometry
    /// `try_include` accepted, in encounter order.
    pub fn spent(&self) -> u64 {
        self.spent
    }

    /// Whether any `try_include` call has ever refused — the tile-level
    /// "some geometry was dropped for budget" signal a caller logs/counts
    /// once per tile rather than per dropped feature.
    pub fn exceeded(&self) -> bool {
        self.exhausted
    }
}

/// Bounded escalation factor (`#102`) the PostGIS tile lane
/// (`tellurion-postgis`'s `mvt_tile_inner`) applies to a profile-adjusted
/// tolerance when its pre-flight vertex probe (`build_mvt_vertex_total_
/// plan`) reports a tile would exceed its budget: a single retry at
/// `tolerance * VERTEX_BUDGET_RETRY_TOLERANCE_FACTOR`, never a repeated or
/// unbounded search — see [`decide_tile_path`]'s own doc for why the retry
/// itself is only ever attempted once, and only for a collection whose
/// tolerance a geometry profile actually informed in the first place
/// (`descriptor::heuristics::simplify_tolerance_meters_for_profile`'s own
/// "profile: None returns exactly the baseline" contract governs whether
/// that precondition holds).
pub const VERTEX_BUDGET_RETRY_TOLERANCE_FACTOR: f64 = 2.0;

/// Which of the three vertex-budget paths one tile took (`#102`) — the
/// per-tile outcome `mvt_tile_inner` records into its `tile_vertex_budget_*`
/// metrics counters (see that function's own doc for the exact counter
/// names) and uses to decide which query plan actually serves the tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileSimplificationPath {
    /// The pre-flight probe's vertex total fit the budget at the normal
    /// (zoom/profile-derived) tolerance — no retry, no truncation. Every
    /// tile took this path before `#102`, and every tile whose profile-
    /// adjusted tolerance already keeps it under budget still does.
    Normal,
    /// The probe reported the tile over budget at the normal tolerance, but
    /// a geometry profile was available to retry with; the retry's own
    /// probe (at the raised tolerance) came back under budget, so this tile
    /// serves the raised-tolerance geometry rather than a truncated one.
    Adapted,
    /// The probe reported the tile over budget at the normal tolerance, and
    /// either no profile was available to retry with (today's behavior,
    /// preserved byte-for-byte for a collection without one) or the retry
    /// itself still came back over budget — truncation applies, at the
    /// raised tolerance if a retry was attempted, at the normal one
    /// otherwise.
    TruncatedAfterAdapt,
}

/// Decides which [`TileSimplificationPath`] a tile takes, from the
/// pre-flight probe's vertex total(s) alone — no I/O, so the retry policy
/// is unit-testable independent of a live database. `retry_total_vertices`
/// is `None` when the caller never attempted a retry probe at all (no
/// geometry profile to raise the tolerance with — see
/// `VERTEX_BUDGET_RETRY_TOLERANCE_FACTOR`'s own doc), and `Some(n)` when it
/// did, reporting the raised-tolerance probe's own vertex total. Whether to
/// attempt a retry probe at all is the caller's decision (it has to run a
/// second SQL query to get a number for this function to look at); this
/// function only ever looks at numbers it's handed, never triggers I/O
/// itself.
pub fn decide_tile_path(
    normal_total_vertices: u64,
    vertex_budget: u64,
    retry_total_vertices: Option<u64>,
) -> TileSimplificationPath {
    if normal_total_vertices <= vertex_budget {
        return TileSimplificationPath::Normal;
    }
    match retry_total_vertices {
        Some(retry_total) if retry_total <= vertex_budget => TileSimplificationPath::Adapted,
        _ => TileSimplificationPath::TruncatedAfterAdapt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn includes_geometries_while_the_running_total_fits() {
        let mut budget = VertexBudget::new(100);
        assert!(budget.try_include(40));
        assert!(budget.try_include(40));
        assert_eq!(budget.spent(), 80);
        assert!(!budget.exceeded());
    }

    #[test]
    fn refuses_a_geometry_that_would_push_the_running_total_over_budget() {
        let mut budget = VertexBudget::new(100);
        assert!(budget.try_include(60));
        assert!(!budget.try_include(50), "60 + 50 = 110 > 100 must refuse");
        assert_eq!(
            budget.spent(),
            60,
            "a refused geometry must not be added to the running total"
        );
        assert!(budget.exceeded());
    }

    #[test]
    fn a_geometry_landing_exactly_on_the_budget_is_included() {
        let mut budget = VertexBudget::new(100);
        assert!(budget.try_include(100));
        assert_eq!(budget.spent(), 100);
        assert!(!budget.exceeded());
    }

    #[test]
    fn once_exhausted_a_later_smaller_geometry_is_still_refused() {
        let mut budget = VertexBudget::new(100);
        assert!(budget.try_include(90));
        assert!(!budget.try_include(20), "90 + 20 = 110 > 100 must refuse");
        assert!(
            !budget.try_include(1),
            "a tiny geometry after the budget broke must still be refused, matching the \
             SQL side's monotonic cumulative sum: once one row's inclusion pushes the \
             running total over budget, every later row's running total is also over"
        );
        assert_eq!(
            budget.spent(),
            90,
            "spent stays at the last accepted total, never creeps up from a refused geometry"
        );
    }

    #[test]
    fn a_single_geometry_over_the_whole_budget_is_refused_even_from_empty() {
        let mut budget = VertexBudget::new(10);
        assert!(!budget.try_include(11));
        assert_eq!(budget.spent(), 0);
        assert!(budget.exceeded());
    }

    #[test]
    fn a_zero_vertex_geometry_never_refuses_and_never_spends() {
        let mut budget = VertexBudget::new(0);
        assert!(budget.try_include(0));
        assert_eq!(budget.spent(), 0);
        assert!(!budget.exceeded());
    }

    // -- tile-path decision (`#102`) --------------------------------------

    #[test]
    fn decide_tile_path_is_normal_when_the_probe_fits_the_budget() {
        assert_eq!(
            decide_tile_path(500, 1_000, None),
            TileSimplificationPath::Normal
        );
    }

    #[test]
    fn decide_tile_path_is_normal_at_exactly_the_budget() {
        assert_eq!(
            decide_tile_path(1_000, 1_000, None),
            TileSimplificationPath::Normal
        );
    }

    #[test]
    fn decide_tile_path_truncates_when_over_budget_with_no_retry_attempted() {
        assert_eq!(
            decide_tile_path(1_500, 1_000, None),
            TileSimplificationPath::TruncatedAfterAdapt,
            "no profile means no retry probe was ever run, so this must match today's              immediate-truncation behavior"
        );
    }

    #[test]
    fn decide_tile_path_adapts_when_the_retry_probe_fits() {
        assert_eq!(
            decide_tile_path(1_500, 1_000, Some(900)),
            TileSimplificationPath::Adapted
        );
    }

    #[test]
    fn decide_tile_path_adapts_at_exactly_the_budget_on_retry() {
        assert_eq!(
            decide_tile_path(1_500, 1_000, Some(1_000)),
            TileSimplificationPath::Adapted
        );
    }

    #[test]
    fn decide_tile_path_truncates_when_the_retry_probe_still_overflows() {
        assert_eq!(
            decide_tile_path(1_500, 1_000, Some(1_200)),
            TileSimplificationPath::TruncatedAfterAdapt,
            "a retry that still overflows must fall back to truncation, not loop again"
        );
    }
}
