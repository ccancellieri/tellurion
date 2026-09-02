//! The closed tile-matrix-set registry (`#190`): exactly the OGC-registered
//! grids this workspace can serve, as a plain `Copy` enum — the same
//! boot-time, named-registry philosophy `crate::extension::NamedRegistry`
//! applies to drivers, but even narrower: a tile grid changes what every
//! driver's envelope math and every cache key MEANS, so the set is closed at
//! compile time, never extended by configuration or dynamic loading.
//!
//! Lives in `tellurion-core` rather than `tellurion-tiles` because three
//! layers below the protocol crate need the identity: `TileSource` (a driver
//! declares which grids its envelope math can honor and receives the grid
//! alongside every `TileCoord`, whose `z`/`x`/`y` are meaningless without
//! it), `cache::TileKey` (two grids' tiles at the same `z`/`x`/`y` must
//! never collide), and `tellurion-postgis` (which builds the CRS84 tile
//! envelope in SQL from [`world_crs84_tile_bounds_deg`] below). The full
//! OGC 17-083r4 *documents* (scale ladders, `tileMatrices` JSON) stay in
//! `tellurion-tiles::tilematrixset`, the one crate that serves them — this
//! module carries only identity plus the pure grid math shared across
//! crates, mirroring how `crate::crs` keeps CRS identity here while
//! projection-heavy rendering math lives with its protocol crate.

use crate::storage::TileCoord;

/// Every tile matrix set this server can ever serve (`#190`). Ordering of
/// [`TileMatrixSet::ALL`] is the advertisement order (`/tileMatrixSets`,
/// per-collection tileset listings): `WebMercatorQuad` first, since it is
/// every driver's native grid and the only one that existed before `#190`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TileMatrixSet {
    /// EPSG:3857 spherical Web Mercator, one square root tile — the grid
    /// every driver in this workspace natively serves.
    WebMercatorQuad,
    /// OGC 17-083r4 `WorldCRS84Quad`: plain CRS84 geographic degrees, TWO
    /// side-by-side root tiles at level 0 (matrix 2x1) covering
    /// `[-180, -90, 180, 90]`, halving per level — served only by drivers
    /// that compute their tile envelope per request (PostGIS); archive-native
    /// drivers refuse it by name at resolve time.
    WorldCrs84Quad,
}

impl TileMatrixSet {
    /// Advertisement order — see the enum's own doc.
    pub const ALL: [TileMatrixSet; 2] = [
        TileMatrixSet::WebMercatorQuad,
        TileMatrixSet::WorldCrs84Quad,
    ];

    /// The OGC-registered identifier, exactly as it appears in request paths
    /// (`.../tiles/{tileMatrixSetId}/...`) and TileSet metadata.
    pub fn id(self) -> &'static str {
        match self {
            TileMatrixSet::WebMercatorQuad => "WebMercatorQuad",
            TileMatrixSet::WorldCrs84Quad => "WorldCRS84Quad",
        }
    }

    /// Case-sensitive, exact-id lookup — the OGC registry ids above are the
    /// only spellings a client can have learned from anything this server
    /// advertises, so anything else is an unknown resource (404), never a
    /// fuzzy match.
    pub fn from_id(id: &str) -> Option<TileMatrixSet> {
        TileMatrixSet::ALL.into_iter().find(|tms| tms.id() == id)
    }

    /// The EPSG code of the CRS this grid's tile coordinates — and therefore
    /// every tile's own content — are expressed in (`#262`).
    ///
    /// This is not a new fact about the grid, it is the fact the grid is
    /// *made of*: OGC 17-083r4 defines a tile matrix set (SS4.14) as a
    /// "tiling scheme consisting of a set of tile matrices defined at
    /// different scales covering approximately the same area and having a
    /// common coordinate reference system", and its Requirement 1
    /// (`/req/tilematrixset/model`, Table 6) makes `crs` a One (mandatory)
    /// part of the data structure. Until `#262` the two values below were
    /// spelled inline at each of the places that needed them —
    /// `tellurion-postgis`' tile-envelope CTEs and candidate predicate, the
    /// `tellurion-tiles` TileMatrixSet documents — where nothing kept them
    /// in step with the grid they described.
    ///
    /// Why an EPSG *code* rather than a CRS URI: every consumer of this
    /// answer compares it against a collection's storage SRID, which is an
    /// integer everywhere in this workspace (`CollectionDecl::srid`,
    /// `PhysicalCollection::srid`), and both grids' CRSs have EPSG codes.
    /// `crate::crs::epsg_uri` turns it into the URI when a *protocol*
    /// surface needs one.
    ///
    /// Note this is the CRS a tile's coordinates are *in*, not something a
    /// tile declares: a tile carries no CRS of its own on the wire, which
    /// is why a server that cannot express a collection's geometry in this
    /// CRS has no honest way to annotate the tile and must either transform
    /// or refuse (`#262`).
    pub fn crs_srid(self) -> i32 {
        match self {
            // EPSG:3857 WGS 84 / Pseudo-Mercator.
            TileMatrixSet::WebMercatorQuad => 3857,
            // CRS84 is EPSG:4326's axes in longitude/latitude order; the
            // SRID PostGIS stores those degrees under is 4326 either way
            // (`crate::crs`'s own module doc on the axis-order trap).
            TileMatrixSet::WorldCrs84Quad => 4326,
        }
    }

    /// Tile columns at level `z`: `2^z` for the square WebMercatorQuad
    /// matrix, `2^(z+1)` for WorldCRS84Quad's two-root-tile matrix (OGC
    /// 17-083r4: level 0 is 2x1, both dimensions doubling per level).
    pub fn matrix_width(self, z: u8) -> u64 {
        match self {
            TileMatrixSet::WebMercatorQuad => 1u64 << z,
            TileMatrixSet::WorldCrs84Quad => 2u64 << z,
        }
    }

    /// Tile rows at level `z` — `2^z` for both grids: WorldCRS84Quad's 180
    /// degrees of latitude split into `2^z` rows of the same angular size as
    /// its `2^(z+1)` columns over 360 degrees of longitude (square tiles in
    /// degrees, twice as many across as down).
    pub fn matrix_height(self, z: u8) -> u64 {
        1u64 << z
    }
}

impl std::fmt::Display for TileMatrixSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

/// The full CRS84-degrees extent of one `WorldCRS84Quad` tile `(z, x, y)`,
/// as `[minlon, minlat, maxlon, maxlat]` (`#190`) — derived from the grid
/// definition (row 0 at the north edge, column 0 at the antimeridian, tile
/// side `180 / 2^z` degrees in BOTH axes), never hardcoded per level. The
/// one envelope function every WorldCRS84Quad consumer shares:
/// `tellurion-postgis` binds these four numbers into its tile-envelope CTE
/// (`sql::build_mvt_candidate_fragment`), so the SQL lane's envelope can
/// never drift from the grid the handlers validated `x`/`y` against.
///
/// Callers are expected to have already bounds-checked `x < 2^(z+1)` and
/// `y < 2^z` (the tiles handler's `parse_tile_coord` does); out-of-range
/// indices produce an out-of-world box, not a panic.
pub fn world_crs84_tile_bounds_deg(coord: TileCoord) -> [f64; 4] {
    // 180 / 2^z — exact in binary floating point for every z this server
    // allows (the zoom ceiling is 24), so tile edges at every level align
    // bit-for-bit with their parents' edges.
    let tile_size_deg = 180.0 / (1u64 << coord.z) as f64;
    let minlon = -180.0 + f64::from(coord.x) * tile_size_deg;
    let maxlat = 90.0 - f64::from(coord.y) * tile_size_deg;
    [
        minlon,
        maxlat - tile_size_deg,
        minlon + tile_size_deg,
        maxlat,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coord(z: u8, x: u32, y: u32) -> TileCoord {
        TileCoord { z, x, y }
    }

    #[test]
    fn ids_round_trip_through_the_registry() {
        for tms in TileMatrixSet::ALL {
            assert_eq!(TileMatrixSet::from_id(tms.id()), Some(tms));
        }
        assert_eq!(TileMatrixSet::from_id("webmercatorquad"), None);
        assert_eq!(TileMatrixSet::from_id("WorldMercatorWGS84Quad"), None);
    }

    /// `#262`: every grid names the CRS its tiles are expressed in, and the
    /// two are genuinely different — which is the whole reason the SQL lane
    /// has to know which one it is building an envelope in before it can
    /// compare that envelope against a stored geometry.
    #[test]
    fn every_grid_names_the_crs_its_tiles_are_expressed_in() {
        assert_eq!(TileMatrixSet::WebMercatorQuad.crs_srid(), 3857);
        assert_eq!(TileMatrixSet::WorldCrs84Quad.crs_srid(), 4326);
        let distinct: std::collections::HashSet<i32> = TileMatrixSet::ALL
            .into_iter()
            .map(|t| t.crs_srid())
            .collect();
        assert_eq!(
            distinct.len(),
            TileMatrixSet::ALL.len(),
            "two grids sharing a CRS would make the SQL lane's grid-vs-storage comparison \
             ambiguous about which envelope it is transforming"
        );
    }

    #[test]
    fn web_mercator_matrix_stays_square() {
        for z in [0u8, 1, 4, 24] {
            assert_eq!(
                TileMatrixSet::WebMercatorQuad.matrix_width(z),
                TileMatrixSet::WebMercatorQuad.matrix_height(z)
            );
        }
        assert_eq!(TileMatrixSet::WebMercatorQuad.matrix_width(0), 1);
    }

    #[test]
    fn world_crs84_matrix_is_twice_as_wide_as_tall_at_every_level() {
        for z in [0u8, 1, 4, 24] {
            let tms = TileMatrixSet::WorldCrs84Quad;
            assert_eq!(tms.matrix_width(z), 2 * tms.matrix_height(z), "level {z}");
        }
        assert_eq!(TileMatrixSet::WorldCrs84Quad.matrix_width(0), 2);
        assert_eq!(TileMatrixSet::WorldCrs84Quad.matrix_height(0), 1);
    }

    /// `#190`'s level-0 anchor case: exactly two root tiles, side by side,
    /// splitting the full `[-180, -90, 180, 90]` CRS84 world at the prime
    /// meridian.
    #[test]
    fn level_zero_is_two_root_tiles_splitting_the_world_at_the_prime_meridian() {
        assert_eq!(
            world_crs84_tile_bounds_deg(coord(0, 0, 0)),
            [-180.0, -90.0, 0.0, 90.0]
        );
        assert_eq!(
            world_crs84_tile_bounds_deg(coord(0, 1, 0)),
            [0.0, -90.0, 180.0, 90.0]
        );
    }

    /// A pinned z2 case, verified by hand against the grid definition: at
    /// level 2 the tile side is 45 degrees, columns run 0..=7 west-to-east
    /// from -180, rows run 0..=3 north-to-south from +90 — so (x=3, y=1) is
    /// the 45-degree square just northwest of (0, 0).
    #[test]
    fn a_known_z2_tile_maps_to_its_published_bbox() {
        assert_eq!(
            world_crs84_tile_bounds_deg(coord(2, 3, 1)),
            [-45.0, 0.0, 0.0, 45.0]
        );
        // And the grid's own corners at the same level.
        assert_eq!(
            world_crs84_tile_bounds_deg(coord(2, 0, 0)),
            [-180.0, 45.0, -135.0, 90.0]
        );
        assert_eq!(
            world_crs84_tile_bounds_deg(coord(2, 7, 3)),
            [135.0, -90.0, 180.0, -45.0]
        );
    }

    /// Every level's tiles jointly cover the world exactly: the last column
    /// ends at +180, the last row at -90, and adjacent tiles share edges.
    #[test]
    fn tiles_tile_the_world_without_gaps_or_overlap() {
        let z = 3u8;
        let width = TileMatrixSet::WorldCrs84Quad.matrix_width(z) as u32;
        let height = TileMatrixSet::WorldCrs84Quad.matrix_height(z) as u32;
        assert_eq!(
            world_crs84_tile_bounds_deg(coord(z, width - 1, height - 1))[2],
            180.0
        );
        assert_eq!(
            world_crs84_tile_bounds_deg(coord(z, width - 1, height - 1))[1],
            -90.0
        );
        let left = world_crs84_tile_bounds_deg(coord(z, 2, 2));
        let right = world_crs84_tile_bounds_deg(coord(z, 3, 2));
        assert_eq!(left[2], right[0], "adjacent columns share an edge");
        let below = world_crs84_tile_bounds_deg(coord(z, 2, 3));
        assert_eq!(left[1], below[3], "adjacent rows share an edge");
    }
}
