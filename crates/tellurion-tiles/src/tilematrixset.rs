//! The TileMatrixSet documents this server serves (OGC 17-083r4):
//! `WebMercatorQuad` and, since `#190`, `WorldCRS84Quad`. Matrices are
//! computed from each grid's standard formula, not copy-pasted from a table:
//! cell size halves and both matrix dimensions double each zoom level,
//! starting from each grid's well-known zoom-0 case (one 256x256 mercator
//! tile covering the whole projected world; TWO side-by-side 256x256 CRS84
//! tiles covering `[-180, -90, 180, 90]`). Grid *identity* (ids, index
//! bounds, CRS84 envelope math) lives in `tellurion_core::tms` — shared
//! with drivers and the cache key — while this module owns only what the
//! `/tileMatrixSets` endpoints actually serve: the full documents.

use serde::Serialize;
use tellurion_core::TileMatrixSet;

pub const WEB_MERCATOR_QUAD_ID: &str = "WebMercatorQuad";
pub const WEB_MERCATOR_QUAD_URI: &str =
    "http://www.opengis.net/def/tilematrixset/OGC/1.0/WebMercatorQuad";
pub const WEB_MERCATOR_QUAD_CRS: &str = "http://www.opengis.net/def/crs/EPSG/0/3857";
pub const WEB_MERCATOR_QUAD_TITLE: &str = "Google Maps Compatible for the World";

/// `WorldCRS84Quad` (`#190`, OGC 17-083r4 Annex E): same registry family as
/// the WebMercatorQuad constants above, same "id/URI/CRS/title verified
/// against the OGC definition server" bar. The CRS is CRS84 itself
/// (longitude-first WGS84), the one URI `tellurion_core::crs::CRS84_URI`
/// already pins for the features lane.
pub const WORLD_CRS84_QUAD_ID: &str = "WorldCRS84Quad";
pub const WORLD_CRS84_QUAD_URI: &str =
    "http://www.opengis.net/def/tilematrixset/OGC/1.0/WorldCRS84Quad";
pub const WORLD_CRS84_QUAD_CRS: &str = "http://www.opengis.net/def/crs/OGC/1.3/CRS84";
pub const WORLD_CRS84_QUAD_TITLE: &str = "CRS84 for the World";

/// Half the Web Mercator world extent in meters (EPSG:3857). `pub(crate)`
/// (not `MAX_ZOOM`-style `pub`) so `crate::mercator`'s own tile-bounds math
/// (`#86`, the OGC API Maps window compositor) can derive a tile's mercator
/// extent from the same constant this module's own matrix table uses,
/// instead of a second, independently-verified copy of the same number.
pub(crate) const WEB_MERCATOR_ORIGIN: f64 = 20_037_508.342_789_244;
pub(crate) const TILE_SIZE_PX: u32 = 256;
/// Standardized rendering pixel size (0.28mm) used to derive `scaleDenominator`
/// from `cellSize` (OGC 17-083r4 SS5.2.1).
const STANDARDIZED_PIXEL_SIZE_M: f64 = 0.00028;
pub const MAX_ZOOM: u8 = 24;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TileMatrix {
    pub id: String,
    #[serde(rename = "scaleDenominator")]
    pub scale_denominator: f64,
    #[serde(rename = "cellSize")]
    pub cell_size: f64,
    #[serde(rename = "pointOfOrigin")]
    pub point_of_origin: [f64; 2],
    #[serde(rename = "tileWidth")]
    pub tile_width: u32,
    #[serde(rename = "tileHeight")]
    pub tile_height: u32,
    #[serde(rename = "matrixWidth")]
    pub matrix_width: u64,
    #[serde(rename = "matrixHeight")]
    pub matrix_height: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TileMatrixSetDoc {
    pub id: String,
    pub title: String,
    pub uri: String,
    pub crs: String,
    /// OGC 17-083r4's `tileMatrixSet.json` schema requires this property
    /// under the plural name `tileMatrices` (`required: ["crs", "tileMatrices"]`).
    #[serde(rename = "tileMatrices")]
    pub tile_matrices: Vec<TileMatrix>,
}

/// Computes matrices for zoom `0..=max_zoom` from the standard formula.
pub fn web_mercator_quad_matrices(max_zoom: u8) -> Vec<TileMatrix> {
    let cell_size_zoom_0 = (2.0 * WEB_MERCATOR_ORIGIN) / TILE_SIZE_PX as f64;
    (0..=max_zoom)
        .map(|zoom| {
            let matrix_side = 1u64 << zoom;
            let cell_size = cell_size_zoom_0 / matrix_side as f64;
            TileMatrix {
                id: zoom.to_string(),
                scale_denominator: cell_size / STANDARDIZED_PIXEL_SIZE_M,
                cell_size,
                point_of_origin: [-WEB_MERCATOR_ORIGIN, WEB_MERCATOR_ORIGIN],
                tile_width: TILE_SIZE_PX,
                tile_height: TILE_SIZE_PX,
                matrix_width: matrix_side,
                matrix_height: matrix_side,
            }
        })
        .collect()
}

/// The full `WebMercatorQuad` TileMatrixSet document (zoom 0..=24).
pub fn web_mercator_quad_document() -> TileMatrixSetDoc {
    TileMatrixSetDoc {
        id: WEB_MERCATOR_QUAD_ID.to_string(),
        title: WEB_MERCATOR_QUAD_TITLE.to_string(),
        uri: WEB_MERCATOR_QUAD_URI.to_string(),
        crs: WEB_MERCATOR_QUAD_CRS.to_string(),
        tile_matrices: web_mercator_quad_matrices(MAX_ZOOM),
    }
}

/// Computes `WorldCRS84Quad` matrices for level `0..=max_zoom` (`#190`) from
/// the grid's own formula, mirroring [`web_mercator_quad_matrices`]: level 0
/// is TWO 256x256 tiles side by side over `[-180, -90, 180, 90]` (matrix
/// 2x1, cell size `180 / 256` degrees), both dimensions doubling and the
/// cell size halving per level. `scaleDenominator` follows OGC 17-083r4
/// SS5.2.1's rule for a geographic CRS — the degree cell size converted to
/// meters at the equator (`WEB_MERCATOR_ORIGIN / 180` meters per degree,
/// derived from the same constant the mercator table uses so the two ladders
/// can never disagree on the earth's size), over the standardized 0.28mm
/// pixel — which reproduces the published Annex E table (level 0:
/// `279541132.0143589...`).
pub fn world_crs84_quad_matrices(max_zoom: u8) -> Vec<TileMatrix> {
    let meters_per_degree = WEB_MERCATOR_ORIGIN / 180.0;
    let cell_size_zoom_0 = 180.0 / TILE_SIZE_PX as f64;
    (0..=max_zoom)
        .map(|zoom| {
            let cell_size = cell_size_zoom_0 / (1u64 << zoom) as f64;
            TileMatrix {
                id: zoom.to_string(),
                scale_denominator: cell_size * meters_per_degree / STANDARDIZED_PIXEL_SIZE_M,
                cell_size,
                point_of_origin: [-180.0, 90.0],
                tile_width: TILE_SIZE_PX,
                tile_height: TILE_SIZE_PX,
                matrix_width: TileMatrixSet::WorldCrs84Quad.matrix_width(zoom),
                matrix_height: TileMatrixSet::WorldCrs84Quad.matrix_height(zoom),
            }
        })
        .collect()
}

/// The full `WorldCRS84Quad` TileMatrixSet document (level 0..=24, `#190`).
pub fn world_crs84_quad_document() -> TileMatrixSetDoc {
    TileMatrixSetDoc {
        id: WORLD_CRS84_QUAD_ID.to_string(),
        title: WORLD_CRS84_QUAD_TITLE.to_string(),
        uri: WORLD_CRS84_QUAD_URI.to_string(),
        crs: WORLD_CRS84_QUAD_CRS.to_string(),
        tile_matrices: world_crs84_quad_matrices(MAX_ZOOM),
    }
}

/// The served document for one registry entry (`#190`) — the seam that ties
/// `tellurion_core::tms::TileMatrixSet` (identity, shared with drivers and
/// the cache) to this module's own full documents, so `/tileMatrixSets/{id}`
/// can never serve a definition the rest of the system doesn't recognize,
/// nor vice versa.
pub fn document_for(tms: TileMatrixSet) -> TileMatrixSetDoc {
    match tms {
        TileMatrixSet::WebMercatorQuad => web_mercator_quad_document(),
        TileMatrixSet::WorldCrs84Quad => world_crs84_quad_document(),
    }
}

/// The registered `tileMatrixSetURI` for one registry entry — used by the
/// tileset bodies, which need the URI without paying for the full document.
pub fn uri_of(tms: TileMatrixSet) -> &'static str {
    match tms {
        TileMatrixSet::WebMercatorQuad => WEB_MERCATOR_QUAD_URI,
        TileMatrixSet::WorldCrs84Quad => WORLD_CRS84_QUAD_URI,
    }
}

/// The grid's own CRS URI — same accessor shape as [`uri_of`].
pub fn crs_of(tms: TileMatrixSet) -> &'static str {
    match tms {
        TileMatrixSet::WebMercatorQuad => WEB_MERCATOR_QUAD_CRS,
        TileMatrixSet::WorldCrs84Quad => WORLD_CRS84_QUAD_CRS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_zero_matches_the_published_reference_values() {
        let matrices = web_mercator_quad_matrices(0);
        let z0 = &matrices[0];
        assert!((z0.cell_size - 156_543.033_928_041).abs() < 1e-3);
        assert!((z0.scale_denominator - 559_082_264.028_717).abs() < 1.0);
        assert_eq!(z0.matrix_width, 1);
        assert_eq!(z0.matrix_height, 1);
        assert_eq!(z0.tile_width, 256);
        assert_eq!(
            z0.point_of_origin,
            [-WEB_MERCATOR_ORIGIN, WEB_MERCATOR_ORIGIN]
        );
    }

    #[test]
    fn zoom_one_matches_the_published_reference_values() {
        let matrices = web_mercator_quad_matrices(1);
        let z1 = &matrices[1];
        assert!((z1.cell_size - 78_271.516_964_020_4).abs() < 1e-3);
        assert_eq!(z1.matrix_width, 2);
        assert_eq!(z1.matrix_height, 2);
    }

    #[test]
    fn matrix_side_doubles_every_zoom() {
        let matrices = web_mercator_quad_matrices(4);
        for (zoom, matrix) in matrices.iter().enumerate() {
            let expected = 1u64 << zoom;
            assert_eq!(matrix.matrix_width, expected);
            assert_eq!(matrix.matrix_height, expected);
            assert_eq!(matrix.id, zoom.to_string());
        }
    }

    #[test]
    fn cell_size_halves_every_zoom() {
        let matrices = web_mercator_quad_matrices(3);
        for pair in matrices.windows(2) {
            assert!((pair[0].cell_size / 2.0 - pair[1].cell_size).abs() < 1e-6);
        }
    }

    #[test]
    fn full_document_covers_zoom_0_through_24() {
        let doc = web_mercator_quad_document();
        assert_eq!(doc.id, WEB_MERCATOR_QUAD_ID);
        assert_eq!(doc.tile_matrices.len(), 25);
        assert_eq!(doc.tile_matrices.last().unwrap().id, "24");
    }

    #[test]
    fn serializes_the_tile_matrix_array_under_the_schema_required_plural_key() {
        let json = serde_json::to_value(web_mercator_quad_document()).unwrap();
        assert!(json.get("tileMatrices").is_some());
        assert!(json.get("tileMatrix").is_none());
    }

    /// `#190`: level 0 of the published OGC 17-083r4 WorldCRS84Quad table —
    /// two root tiles side by side, 0.703125-degree cells, and the table's
    /// own scale denominator.
    #[test]
    fn world_crs84_level_zero_matches_the_published_reference_values() {
        let matrices = world_crs84_quad_matrices(0);
        let z0 = &matrices[0];
        assert_eq!(z0.matrix_width, 2);
        assert_eq!(z0.matrix_height, 1);
        assert_eq!(z0.tile_width, 256);
        assert_eq!(z0.point_of_origin, [-180.0, 90.0]);
        assert!((z0.cell_size - 0.703_125).abs() < 1e-12);
        assert!((z0.scale_denominator - 279_541_132.014_358_9).abs() < 1.0);
    }

    /// `#190`: the CRS84 ladder halves its cell size and doubles BOTH matrix
    /// dimensions per level, staying twice as wide as tall throughout.
    #[test]
    fn world_crs84_ladder_halves_cell_size_and_doubles_both_dimensions() {
        let matrices = world_crs84_quad_matrices(4);
        for pair in matrices.windows(2) {
            assert!((pair[0].cell_size / 2.0 - pair[1].cell_size).abs() < 1e-12);
            assert_eq!(pair[1].matrix_width, 2 * pair[0].matrix_width);
            assert_eq!(pair[1].matrix_height, 2 * pair[0].matrix_height);
        }
        for (zoom, matrix) in matrices.iter().enumerate() {
            assert_eq!(
                matrix.matrix_width,
                2 * matrix.matrix_height,
                "level {zoom}"
            );
        }
    }

    /// `#190`: `document_for` serves each registry entry's own document —
    /// ids, URIs, and CRSs never crossed.
    #[test]
    fn document_for_ties_each_registry_entry_to_its_own_document() {
        for tms in TileMatrixSet::ALL {
            let doc = document_for(tms);
            assert_eq!(doc.id, tms.id());
            assert_eq!(doc.uri, uri_of(tms));
            assert_eq!(doc.crs, crs_of(tms));
            assert_eq!(doc.tile_matrices.len(), 25);
        }
        assert_eq!(
            document_for(TileMatrixSet::WorldCrs84Quad).crs,
            WORLD_CRS84_QUAD_CRS
        );
    }
}
