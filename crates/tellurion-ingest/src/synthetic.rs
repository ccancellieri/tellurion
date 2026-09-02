//! Shared synthetic demo-data generator for `seed` (PostGIS) and
//! `geopackage seed`: the deterministic global grid of point/polygon
//! features both subcommands populate their table with. Only the row
//! *shape* lives here — name, timestamp, grid position, and point-vs-polygon
//! parity — because that part is backend-agnostic; each caller still owns
//! its own geometry encoding (`seed.rs`'s WKT text vs `geopackage_seed.rs`'s
//! GeoJSON) and coordinate-space mapping, since those genuinely differ per
//! backend (a bound SQL parameter vs a GeoJSON value keyed to whatever SRID
//! the target table was actually provisioned under).

use std::time::{Duration, SystemTime};

pub const LON_STEPS: i32 = 25;
pub const LAT_STEPS: i32 = 20;
pub const ROW_COUNT: usize = (LON_STEPS * LAT_STEPS) as usize;

/// One synthetic feature: a name, a deterministic timestamp, a fractional
/// grid position in `(0, 1)` on each axis (the caller maps this into its own
/// coordinate space), and whether this cell should render as a small
/// polygon rather than a bare point.
pub struct SyntheticFeature {
    pub name: String,
    pub observed_at: SystemTime,
    pub u: f64,
    pub v: f64,
    pub is_polygon: bool,
}

/// Builds the deterministic 25x20 grid (500 cells), alternating point/
/// polygon cells — identical names, timestamps, and positions every run, so
/// seeding is reproducible for benchmarking.
pub fn grid() -> Vec<SyntheticFeature> {
    let mut out = Vec::with_capacity(ROW_COUNT);
    let mut total = 0usize;

    for i in 0..LON_STEPS {
        for j in 0..LAT_STEPS {
            let u = (i as f64 + 0.5) / LON_STEPS as f64;
            let v = (j as f64 + 0.5) / LAT_STEPS as f64;
            out.push(SyntheticFeature {
                name: format!("feature-{i}-{j}"),
                observed_at: SystemTime::UNIX_EPOCH + Duration::from_secs(total as u64 * 3600),
                u,
                v,
                is_polygon: (i + j) % 2 != 0,
            });
            total += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_has_the_documented_row_count() {
        assert_eq!(grid().len(), ROW_COUNT);
        assert_eq!(ROW_COUNT, 500);
    }

    #[test]
    fn grid_alternates_point_and_polygon_cells() {
        let rows = grid();
        assert!(rows.iter().any(|f| f.is_polygon));
        assert!(rows.iter().any(|f| !f.is_polygon));
    }

    #[test]
    fn grid_positions_stay_within_the_unit_square() {
        for feature in grid() {
            assert!(feature.u > 0.0 && feature.u < 1.0);
            assert!(feature.v > 0.0 && feature.v < 1.0);
        }
    }

    #[test]
    fn grid_names_are_unique() {
        let rows = grid();
        let mut names: Vec<&str> = rows.iter().map(|f| f.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), rows.len());
    }
}
