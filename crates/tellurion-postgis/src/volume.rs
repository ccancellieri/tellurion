//! `VolumeSource` driver logic (`#41`): the geometry-type contract, the
//! world-to-tile-local affine transform, and mesh assembly (triangulation +
//! transform + complexity caps) that sit between `driver.rs`'s SQL/decode
//! plumbing and `tellurion_core::VolumeMesh`. Pure aside from
//! [`VolumeGeometryKind::from_catalog`] reading catalog-shaped strings — no
//! I/O of its own.

use tellurion_core::{CollectionDecl, TileCoord, VolumeMesh};

use crate::ewkb::SolidZ;

/// The 3D solid geometry type contract (`#41` part 1): a `VolumeSource`
/// column must be one of these, detected the same way the 2D path derives
/// its metadata — via `geometry_columns`' `type`/`coord_dimension`, not
/// assumed from config. `MultiPolygon` is the degenerate "roof print" case:
/// a flat set of polygon faces rather than a closed solid, accepted because
/// the decode-and-triangulate pipeline treats every face identically
/// regardless of which of the three wrapper types it came from — see
/// `ewkb`'s own module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VolumeGeometryKind {
    PolyhedralSurface,
    Tin,
    MultiPolygon,
}

impl VolumeGeometryKind {
    /// Classifies a `geometry_columns` row. `coord_dimension` must be
    /// exactly 3 (XYZ) — 2 (no Z, nothing to extrude) and 4 (XYZM, this
    /// reader has no M support to fall back on) are both refused, matching
    /// `ewkb::decode_solid`'s own Z-required, M-ignored contract.
    pub(crate) fn from_catalog(type_name: &str, coord_dimension: i32) -> Option<Self> {
        if coord_dimension != 3 {
            return None;
        }
        match type_name {
            "POLYHEDRALSURFACE" => Some(Self::PolyhedralSurface),
            "TIN" => Some(Self::Tin),
            "MULTIPOLYGON" => Some(Self::MultiPolygon),
            _ => None,
        }
    }
}

/// Web Mercator whole-world half-extent in meters (EPSG:3857) — the same
/// constant `ST_TileEnvelope` is built from. Duplicated here rather than
/// imported from `tellurion-tiles`/`tellurion-places`' own copies, matching
/// this workspace's existing choice to keep each crate independent of the
/// others for small, stable geodesy constants like this one (see
/// `tellurion-places::handlers`' own doc comment for the same rationale).
const WEB_MERCATOR_ORIGIN_M: f64 = 20_037_508.342_789_244;

/// The affine transform from EPSG:3857 world meters to one tile's local
/// `[0, 1] x [0, 1]` XY square, Y increasing downward — the exact
/// tile-local convention `tellurion_render::extrude_mvt_to_glb` documents
/// and `VolumeMesh`'s own doc comment requires (`#41` part 4). The MVT lane
/// gets this for free from `ST_AsMVTGeom`, which derives its tile-local
/// integers from precisely this same envelope, server-side; this driver has
/// to reproduce the affine half of that math in Rust because its SQL only
/// reprojects (`ST_Transform(geom, 3857)`) before handing raw EWKB to
/// `ewkb::decode_solid` — see this struct's own construction site in
/// `driver.rs` for the SQL text.
///
/// Z is untouched: `ST_Transform` never reprojects the Z ordinate, so a
/// solid's real-world height arrives already in the units `VolumeMesh`
/// expects, with nothing further to compute.
///
/// Bounds verified against a live `ST_TileEnvelope(z, x, y)` call rather
/// than assumed — see this module's own tests.
pub(crate) struct TileTransform {
    min_x: f64,
    max_y: f64,
    tile_size: f64,
}

impl TileTransform {
    pub(crate) fn for_coord(coord: TileCoord) -> Self {
        let matrix_side = (1u64 << coord.z) as f64;
        let tile_size = (2.0 * WEB_MERCATOR_ORIGIN_M) / matrix_side;
        let min_x = -WEB_MERCATOR_ORIGIN_M + f64::from(coord.x) * tile_size;
        let max_y = WEB_MERCATOR_ORIGIN_M - f64::from(coord.y) * tile_size;
        Self {
            min_x,
            max_y,
            tile_size,
        }
    }

    /// Maps one EPSG:3857 world-meters point to this tile's local space.
    pub(crate) fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        [
            (p[0] - self.min_x) / self.tile_size,
            (self.max_y - p[1]) / self.tile_size,
            p[2],
        ]
    }
}

/// Counted-and-logged reasons a solid or face was dropped rather than
/// rendered (`#41` part 5 / part 3's degenerate-face guard). `driver.rs`
/// logs these once per tile with their totals rather than once per
/// occurrence, so a genuinely ill-fitting table doesn't spam the log per
/// row.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VolumeTileStats {
    /// A solid whose total ring-point count (summed across every face,
    /// before any triangulation work) exceeded the effective per-zoom
    /// vertex budget — dropped whole, never partially triangulated.
    pub(crate) solids_over_budget: u64,
    /// A face `tellurion_render::triangulate_face` refused: too few
    /// points, collinear, or not planar within tolerance.
    pub(crate) faces_skipped_degenerate: u64,
}

impl VolumeTileStats {
    pub(crate) fn any_dropped(&self) -> bool {
        self.solids_over_budget > 0 || self.faces_skipped_degenerate > 0
    }
}

/// Assembles a tile's `VolumeMesh` from its decoded solids (`#41` parts 3-5):
/// each solid's total vertex count is checked against `vertex_cap` *before*
/// any triangulation runs (cheap, from the parsed EWKB alone); a solid over
/// budget is dropped whole and counted. Every surviving solid's faces are
/// triangulated (`tellurion_render::triangulate_face`) and each resulting
/// triangle's vertices are mapped through `transform` into tile-local space.
/// Never panics; a degenerate face is skipped and counted rather than
/// aborting the tile.
pub(crate) fn build_volume_mesh(
    solids: Vec<SolidZ>,
    transform: &TileTransform,
    vertex_cap: u64,
) -> (VolumeMesh, VolumeTileStats) {
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    let mut stats = VolumeTileStats::default();

    for solid in solids {
        if solid.total_points() > vertex_cap {
            stats.solids_over_budget += 1;
            continue;
        }
        for face in &solid.faces {
            let Some(triangles) = tellurion_render::triangulate_face(&face.rings) else {
                stats.faces_skipped_degenerate += 1;
                continue;
            };
            for triangle in triangles {
                for vertex in triangle {
                    let local = transform.apply(vertex);
                    let index = positions.len() as u32;
                    positions.push(local);
                    indices.push(index);
                }
            }
        }
    }

    (VolumeMesh { positions, indices }, stats)
}

/// Per-zoom vertex budget for one solid (`#41` part 5), resolved with the
/// same override-wins-else-heuristic precedence
/// `descriptor::heuristics::effective_feature_cap` applies for MVT feature
/// counts — an operator-configured `places3d.vertex_caps` entry always
/// wins; the zoom-driven heuristic (`descriptor::heuristics::
/// volume_vertex_cap`) fills the gap otherwise. A collection with no
/// `places3d` at all never reaches this (there is no volume lane to cap
/// without it — see `resolve_places3d`), but this stays total (falls back
/// to the plain heuristic) rather than panicking, since nothing about the
/// vertex-cap check itself depends on `places3d` being present.
pub(crate) fn effective_volume_vertex_cap(collection: &CollectionDecl, zoom: u8) -> u64 {
    let caps = collection
        .places3d
        .as_ref()
        .map(|places3d| &places3d.vertex_caps);
    match caps {
        Some(caps) => tellurion_core::heuristics::effective_volume_vertex_cap(caps, zoom),
        None => tellurion_core::heuristics::volume_vertex_cap(zoom),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_geometry_kind_accepts_the_three_supported_types() {
        assert_eq!(
            VolumeGeometryKind::from_catalog("POLYHEDRALSURFACE", 3),
            Some(VolumeGeometryKind::PolyhedralSurface)
        );
        assert_eq!(
            VolumeGeometryKind::from_catalog("TIN", 3),
            Some(VolumeGeometryKind::Tin)
        );
        assert_eq!(
            VolumeGeometryKind::from_catalog("MULTIPOLYGON", 3),
            Some(VolumeGeometryKind::MultiPolygon)
        );
    }

    #[test]
    fn volume_geometry_kind_rejects_a_2d_column() {
        assert_eq!(VolumeGeometryKind::from_catalog("POLYGON", 2), None);
        assert_eq!(VolumeGeometryKind::from_catalog("POINT", 2), None);
    }

    #[test]
    fn volume_geometry_kind_rejects_an_unsupported_3d_type() {
        // A plain 3D Polygon (not wrapped in PolyhedralSurface/TIN/
        // MultiPolygon) is a real, valid PostGIS type but not one this
        // driver's EWKB reader (or the design's face model) handles.
        assert_eq!(VolumeGeometryKind::from_catalog("POLYGON", 3), None);
    }

    #[test]
    fn volume_geometry_kind_rejects_an_m_carrying_column() {
        assert_eq!(
            VolumeGeometryKind::from_catalog("POLYHEDRALSURFACE", 4),
            None
        );
    }

    /// Golden bounds for zoom 2, tile (1, 1) — cross-checked against a live
    /// `ST_TileEnvelope(2, 1, 1)` call: `xmin -10018754.171394622, ymin 0,
    /// xmax 0, ymax 10018754.171394622`.
    #[test]
    fn tile_transform_matches_a_live_st_tile_envelope_call() {
        let transform = TileTransform::for_coord(TileCoord { z: 2, x: 1, y: 1 });
        assert!((transform.min_x - (-10_018_754.171_394_622)).abs() < 1e-6);
        assert!((transform.max_y - 10_018_754.171_394_622).abs() < 1e-6);
        assert!((transform.tile_size - 10_018_754.171_394_622).abs() < 1e-6);
    }

    #[test]
    fn tile_transform_maps_the_whole_world_tile_corners_to_the_unit_square() {
        let transform = TileTransform::for_coord(TileCoord { z: 0, x: 0, y: 0 });
        let nw = transform.apply([-WEB_MERCATOR_ORIGIN_M, WEB_MERCATOR_ORIGIN_M, 7.0]);
        assert!((nw[0] - 0.0).abs() < 1e-9, "west edge -> local x=0");
        assert!(
            (nw[1] - 0.0).abs() < 1e-9,
            "north edge -> local y=0 (Y-down)"
        );
        assert_eq!(nw[2], 7.0, "Z passes through untouched");

        let se = transform.apply([WEB_MERCATOR_ORIGIN_M, -WEB_MERCATOR_ORIGIN_M, -3.0]);
        assert!((se[0] - 1.0).abs() < 1e-9, "east edge -> local x=1");
        assert!(
            (se[1] - 1.0).abs() < 1e-9,
            "south edge -> local y=1 (Y-down)"
        );
        assert_eq!(se[2], -3.0);
    }

    #[test]
    fn tile_transform_maps_the_tile_center_to_local_half_half() {
        let transform = TileTransform::for_coord(TileCoord { z: 3, x: 5, y: 2 });
        let matrix_side = 8.0;
        let tile_size = (2.0 * WEB_MERCATOR_ORIGIN_M) / matrix_side;
        let min_x = -WEB_MERCATOR_ORIGIN_M + 5.0 * tile_size;
        let max_y = WEB_MERCATOR_ORIGIN_M - 2.0 * tile_size;
        let center = [min_x + tile_size / 2.0, max_y - tile_size / 2.0, 0.0];
        let local = transform.apply(center);
        assert!((local[0] - 0.5).abs() < 1e-9);
        assert!((local[1] - 0.5).abs() < 1e-9);
    }

    fn face(rings: Vec<Vec<[f64; 3]>>) -> crate::ewkb::FaceZ {
        crate::ewkb::FaceZ { rings }
    }

    fn flat_square_face(z: f64) -> crate::ewkb::FaceZ {
        face(vec![vec![
            [0.0, 0.0, z],
            [1.0, 0.0, z],
            [1.0, 1.0, z],
            [0.0, 1.0, z],
        ]])
    }

    #[test]
    fn build_volume_mesh_triangulates_and_transforms_a_simple_solid() {
        let identity = TileTransform {
            min_x: 0.0,
            max_y: 0.0,
            tile_size: 1.0,
        };
        let solid = SolidZ {
            faces: vec![flat_square_face(3.0)],
        };
        let (mesh, stats) = build_volume_mesh(vec![solid], &identity, 1_000);
        assert_eq!(mesh.indices.len(), 6, "one quad face -> two triangles");
        assert_eq!(mesh.positions.len(), 6);
        assert!(!stats.any_dropped());
        for p in &mesh.positions {
            assert_eq!(p[2], 3.0);
        }
    }

    #[test]
    fn build_volume_mesh_drops_a_solid_over_the_vertex_budget() {
        let identity = TileTransform {
            min_x: 0.0,
            max_y: 0.0,
            tile_size: 1.0,
        };
        let big_solid = SolidZ {
            faces: vec![flat_square_face(0.0), flat_square_face(1.0)],
        };
        // 2 faces * 4 points = 8 total points; a budget of 4 must reject it.
        let (mesh, stats) = build_volume_mesh(vec![big_solid], &identity, 4);
        assert!(mesh.positions.is_empty());
        assert!(mesh.indices.is_empty());
        assert_eq!(stats.solids_over_budget, 1);
        assert_eq!(stats.faces_skipped_degenerate, 0);
    }

    #[test]
    fn build_volume_mesh_keeps_other_solids_when_one_is_over_budget() {
        let identity = TileTransform {
            min_x: 0.0,
            max_y: 0.0,
            tile_size: 1.0,
        };
        let small = SolidZ {
            faces: vec![flat_square_face(0.0)],
        };
        let big = SolidZ {
            faces: vec![flat_square_face(0.0), flat_square_face(1.0)],
        };
        let (mesh, stats) = build_volume_mesh(vec![big, small], &identity, 4);
        assert_eq!(mesh.indices.len(), 6, "the small solid's one face survives");
        assert_eq!(stats.solids_over_budget, 1);
    }

    #[test]
    fn build_volume_mesh_skips_a_degenerate_face_but_counts_it() {
        let identity = TileTransform {
            min_x: 0.0,
            max_y: 0.0,
            tile_size: 1.0,
        };
        let collinear_face = face(vec![vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
        ]]);
        let solid = SolidZ {
            faces: vec![collinear_face, flat_square_face(0.0)],
        };
        let (mesh, stats) = build_volume_mesh(vec![solid], &identity, 1_000);
        assert_eq!(mesh.indices.len(), 6, "the good face still renders");
        assert_eq!(stats.faces_skipped_degenerate, 1);
    }

    #[test]
    fn build_volume_mesh_on_no_solids_yields_an_empty_mesh() {
        let identity = TileTransform {
            min_x: 0.0,
            max_y: 0.0,
            tile_size: 1.0,
        };
        let (mesh, stats) = build_volume_mesh(vec![], &identity, 1_000);
        assert!(mesh.positions.is_empty());
        assert!(mesh.indices.is_empty());
        assert!(!stats.any_dropped());
    }
}
