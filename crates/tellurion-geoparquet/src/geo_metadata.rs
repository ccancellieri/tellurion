//! Parses the GeoParquet "geo" file-level key-value metadata: a JSON
//! document, stored under the literal key `"geo"` in the Parquet
//! `FileMetaData`'s key-value pairs, that names the primary geometry column
//! and its encoding, geometry type(s), bbox, CRS, and (GeoParquet 1.1+)
//! "covering" bbox-column reference used for row-group pruning. See the
//! spec: <https://geoparquet.org/releases/v1.1.0/>.
//!
//! This module only decodes the JSON shape; resolving a covering column's
//! dotted path (e.g. `["bbox", "xmin"]`) to a physical Parquet leaf-column
//! index requires the file's own schema and lives in `driver.rs` instead,
//! next to the row-group statistics code that consumes it.

use serde::Deserialize;

use crate::error::{GeoparquetDriverError, Result};

/// The fixed key this driver looks for in `FileMetaData::key_value_metadata`.
/// A file with no such key is not a valid GeoParquet file — see
/// `GeoparquetDriverError::MissingGeoMetadata`.
pub(crate) const GEO_METADATA_KEY: &str = "geo";

#[derive(Debug, Deserialize)]
struct RawGeoMetadata {
    primary_column: String,
    columns: std::collections::HashMap<String, RawColumnMetadata>,
}

#[derive(Debug, Deserialize)]
struct RawColumnMetadata {
    #[serde(default)]
    geometry_types: Vec<String>,
    #[serde(default)]
    bbox: Option<Vec<f64>>,
    #[serde(default)]
    crs: Option<serde_json::Value>,
    #[serde(default)]
    covering: Option<RawCovering>,
}

#[derive(Debug, Deserialize)]
struct RawCovering {
    bbox: RawCoveringBbox,
}

#[derive(Debug, Deserialize)]
struct RawCoveringBbox {
    xmin: Vec<String>,
    ymin: Vec<String>,
    xmax: Vec<String>,
    ymax: Vec<String>,
}

/// The primary geometry column's metadata, decoded and reduced to what this
/// driver actually consumes — never the raw JSON shape past this module.
#[derive(Debug, Clone)]
pub(crate) struct GeoMetadata {
    pub primary_column: String,
    /// GeoParquet's `geometry_types`, e.g. `["Point"]`, `["Polygon",
    /// "MultiPolygon"]`, or `[]` (any type, mixed dataset). Verbatim from the
    /// file — `driver.rs` derives a single reported `geometry_type` from
    /// this the same "exactly one candidate, else unknown" way
    /// `CatalogSource::temporal_column` treats ambiguity elsewhere in this
    /// contract.
    pub geometry_types: Vec<String>,
    /// `[minx, miny, maxx, maxy]`, CRS84. `None` when the column has no
    /// `bbox` entry (legal per spec) or the entry isn't a well-formed 2D/3D
    /// bbox array.
    pub bbox: Option<[f64; 4]>,
    /// `None` here means CRS84 (the spec's default when `crs` is absent or
    /// JSON `null`) — see `driver.rs::srid_from_crs` for the
    /// absent-vs-unrecognized distinction that later collapses onto
    /// `PhysicalCollection::srid`.
    pub crs: Option<serde_json::Value>,
    pub covering: Option<CoveringPaths>,
}

/// Dotted-path column references from a GeoParquet 1.1 `covering.bbox`
/// block, e.g. `xmin: ["bbox", "xmin"]` for a `bbox` struct column with a
/// `xmin` float child. Kept as raw path segments (not yet resolved to a
/// Parquet leaf-column index) because that resolution needs the file's own
/// `SchemaDescriptor`, unavailable to this metadata-only module.
#[derive(Debug, Clone)]
pub(crate) struct CoveringPaths {
    pub xmin: Vec<String>,
    pub ymin: Vec<String>,
    pub xmax: Vec<String>,
    pub ymax: Vec<String>,
}

/// Reduces a spec `bbox` array (4 entries for 2D, 6 for 3D per
/// <https://geoparquet.org/releases/v1.1.0/#bbox>) to CRS84 `[minx, miny,
/// maxx, maxy]`, dropping the z pair from a 3D bbox. Any other length is a
/// malformed entry — `None`, not an error, matching `CatalogSource::extent`'s
/// own "cannot answer" convention rather than failing the whole file over
/// one cosmetic field.
fn reduce_bbox(raw: &[f64]) -> Option<[f64; 4]> {
    match raw.len() {
        4 => Some([raw[0], raw[1], raw[2], raw[3]]),
        6 => Some([raw[0], raw[1], raw[3], raw[4]]),
        _ => None,
    }
}

/// Parses the `"geo"` key's JSON value and reduces it to this driver's
/// primary-column view. Fails only when the JSON itself is malformed or the
/// document is missing the fields the spec makes mandatory
/// (`primary_column`, and that column's own entry in `columns`) — a column
/// entry missing merely *optional* fields (`bbox`, `crs`, `covering`) is
/// still a valid, if less capable, GeoParquet file.
pub(crate) fn parse_geo_metadata(raw_json: &str) -> Result<GeoMetadata> {
    let raw: RawGeoMetadata = serde_json::from_str(raw_json)?;
    let column = raw.columns.get(&raw.primary_column).ok_or_else(|| {
        GeoparquetDriverError::InvalidGeoMetadata(format!(
            "primary_column '{}' has no entry in columns",
            raw.primary_column
        ))
    })?;

    let bbox = column.bbox.as_deref().and_then(reduce_bbox);
    let covering = column.covering.as_ref().map(|covering| CoveringPaths {
        xmin: covering.bbox.xmin.clone(),
        ymin: covering.bbox.ymin.clone(),
        xmax: covering.bbox.xmax.clone(),
        ymax: covering.bbox.ymax.clone(),
    });

    Ok(GeoMetadata {
        primary_column: raw.primary_column,
        geometry_types: column.geometry_types.clone(),
        bbox,
        crs: column.crs.clone(),
        covering,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_minimal_document_with_no_optional_fields() {
        let raw = r#"{"version":"1.1.0","primary_column":"geometry","columns":{"geometry":{"encoding":"WKB"}}}"#;
        let meta = parse_geo_metadata(raw).unwrap();
        assert_eq!(meta.primary_column, "geometry");
        assert!(meta.geometry_types.is_empty());
        assert_eq!(meta.bbox, None);
        assert_eq!(meta.crs, None);
        assert!(meta.covering.is_none());
    }

    #[test]
    fn parses_bbox_geometry_types_and_covering() {
        let raw = r#"{
            "version": "1.1.0",
            "primary_column": "geometry",
            "columns": {
                "geometry": {
                    "encoding": "WKB",
                    "geometry_types": ["Point"],
                    "bbox": [-4.0, 46.0, 4.0, 54.0],
                    "covering": {
                        "bbox": {
                            "xmin": ["bbox", "xmin"],
                            "ymin": ["bbox", "ymin"],
                            "xmax": ["bbox", "xmax"],
                            "ymax": ["bbox", "ymax"]
                        }
                    }
                }
            }
        }"#;
        let meta = parse_geo_metadata(raw).unwrap();
        assert_eq!(meta.geometry_types, vec!["Point".to_string()]);
        assert_eq!(meta.bbox, Some([-4.0, 46.0, 4.0, 54.0]));
        let covering = meta.covering.unwrap();
        assert_eq!(covering.xmin, vec!["bbox".to_string(), "xmin".to_string()]);
        assert_eq!(covering.ymax, vec!["bbox".to_string(), "ymax".to_string()]);
    }

    #[test]
    fn reduces_a_3d_bbox_by_dropping_the_z_pair() {
        let raw = r#"{"version":"1.1.0","primary_column":"g","columns":{"g":{"encoding":"WKB","bbox":[-1.0,-2.0,-3.0,1.0,2.0,3.0]}}}"#;
        let meta = parse_geo_metadata(raw).unwrap();
        assert_eq!(meta.bbox, Some([-1.0, -2.0, 1.0, 2.0]));
    }

    #[test]
    fn rejects_a_primary_column_with_no_matching_entry() {
        let raw = r#"{"version":"1.1.0","primary_column":"missing","columns":{"geometry":{"encoding":"WKB"}}}"#;
        assert!(matches!(
            parse_geo_metadata(raw),
            Err(GeoparquetDriverError::InvalidGeoMetadata(_))
        ));
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(matches!(
            parse_geo_metadata("not json"),
            Err(GeoparquetDriverError::Json(_))
        ));
    }
}
