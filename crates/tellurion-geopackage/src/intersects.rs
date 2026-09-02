//! Exact `S_INTERSECTS` geometry evaluation — the in-process half of the
//! bbox-then-exact strategy `sql.rs`'s own "bbox pushdown" doc describes.
//! `geo_types::Geometry<f64>` (already a transitive dependency of `geozero`)
//! is the shared coordinate representation; `geo::Intersects` is the actual
//! DE-9IM-based predicate, built directly on `geo_types` with no new
//! geometry engine or C binding.
//!
//! Both sides of a test go through the same 2D-only path: [`literal_to_check`]
//! converts a CQL2 [`GeometryLiteral`] (the query's own "needle"), and
//! [`row_intersects`] decodes one candidate row's GeoPackage WKB body (the
//! same body [`crate::gpb::decode`] already strips the GPB header from
//! everywhere else this driver reads a geometry column). Both refuse by name
//! — [`crate::error::GeopackageError::IntersectsUnsupported`] — rather than
//! silently dropping a third (Z) coordinate: `geo_types::Geometry` has no Z
//! field at all, so a naive conversion would silently answer against a 2D
//! projection instead of the actual 3D shape. This driver's own write path
//! only ever stores 2D geometry (see `gpb.rs`'s own doc), so the row-side
//! refusal only fires against a foreign, externally-authored GeoPackage.

use geo::{BoundingRect, Intersects};
use geo_types::{
    Coord, Geometry, GeometryCollection, LineString, MultiLineString, MultiPoint, MultiPolygon,
    Point, Polygon, Rect,
};

use tellurion_core::{GeometryLiteral, WktGeometry};

use crate::error::{GeopackageError, Result};

/// One `S_INTERSECTS` predicate's query geometry, already converted to
/// `geo_types` for [`row_intersects`], plus its own 2D bounding box for the
/// R*Tree candidate pre-filter (`sql::bbox_clause`).
pub(crate) struct IntersectsCheck {
    needle: Geometry<f64>,
    pub(crate) needle_bbox: [f64; 4],
}

fn coord([x, y]: [f64; 2]) -> Coord<f64> {
    Coord { x, y }
}

fn line_string(points: &[[f64; 2]]) -> LineString<f64> {
    LineString::new(points.iter().copied().map(coord).collect())
}

/// `rings[0]` is the exterior ring, every ring after it a hole — the same
/// convention `WktGeometry::Polygon`'s own doc states (WKT itself doesn't
/// distinguish them structurally).
fn polygon(rings: &[Vec<[f64; 2]>]) -> Polygon<f64> {
    let mut rings = rings.iter();
    let exterior = rings
        .next()
        .map(|r| line_string(r))
        .unwrap_or_else(|| LineString::new(Vec::new()));
    Polygon::new(exterior, rings.map(|r| line_string(r)).collect())
}

/// `WktGeometry` is always exactly 2D and non-empty by construction — its
/// own parser rejects `Z`/`M`/`ZM` dimensionality and `EMPTY` before this
/// type is ever built (see `WktGeometry`'s own doc) — so every one of its
/// seven variants converts straight across to the matching `geo_types`
/// variant with no fallibility of its own.
fn wkt_to_geo(g: &WktGeometry) -> Geometry<f64> {
    match g {
        WktGeometry::Point(c) => Geometry::Point(Point::new(c[0], c[1])),
        WktGeometry::LineString(pts) => Geometry::LineString(line_string(pts)),
        WktGeometry::Polygon(rings) => Geometry::Polygon(polygon(rings)),
        WktGeometry::MultiPoint(pts) => Geometry::MultiPoint(MultiPoint::new(
            pts.iter()
                .copied()
                .map(|c| Point::new(c[0], c[1]))
                .collect(),
        )),
        WktGeometry::MultiLineString(lines) => Geometry::MultiLineString(MultiLineString::new(
            lines.iter().map(|l| line_string(l)).collect(),
        )),
        WktGeometry::MultiPolygon(polys) => Geometry::MultiPolygon(MultiPolygon::new(
            polys.iter().map(|p| polygon(p)).collect(),
        )),
        WktGeometry::GeometryCollection(items) => Geometry::GeometryCollection(
            GeometryCollection::new_from(items.iter().map(wkt_to_geo).collect()),
        ),
    }
}

/// `true` when `value` (a GeoJSON `Geometry` object, or any JSON value
/// nested inside one) contains a coordinate tuple longer than 2 — RFC 7946's
/// optional third ("elevation") member. Walked generically rather than by
/// GeoJSON key name (`"coordinates"`/`"geometries"`): any all-number JSON
/// array is a coordinate tuple in every one of GeoJSON's geometry shapes,
/// `GeometryCollection` included, so this needs no per-type-name branch to
/// stay correct across all seven.
fn geojson_has_extra_dimension(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(items) => {
            if !items.is_empty() && items.iter().all(serde_json::Value::is_number) {
                items.len() > 2
            } else {
                items.iter().any(geojson_has_extra_dimension)
            }
        }
        serde_json::Value::Object(map) => map.values().any(geojson_has_extra_dimension),
        _ => false,
    }
}

/// Converts a CQL2 spatial literal to the [`IntersectsCheck`] this driver's
/// exact evaluator tests candidate rows against, refusing by name any shape
/// outside the 2D classes `geo_types`/`geo::Intersects` cover — a `GeoJson`
/// payload carrying a Z coordinate, or one that isn't a bare geometry object
/// at all (a `Feature`, say). `Bbox`/`Wkt` are always exactly 2D by
/// construction, so only the `GeoJson` arm can fail.
pub(crate) fn literal_to_check(literal: &GeometryLiteral) -> Result<IntersectsCheck> {
    let needle = match literal {
        GeometryLiteral::Bbox([minx, miny, maxx, maxy]) => {
            Geometry::Rect(Rect::new(coord([*minx, *miny]), coord([*maxx, *maxy])))
        }
        GeometryLiteral::Wkt(wkt) => wkt_to_geo(wkt),
        GeometryLiteral::GeoJson(value) => {
            if geojson_has_extra_dimension(value) {
                return Err(GeopackageError::IntersectsUnsupported(
                    "the geometry literal carries a Z/M coordinate, which this driver's 2D exact evaluator cannot represent".to_string(),
                ));
            }
            use geozero::ToGeo;
            let text = value.to_string();
            geozero::geojson::GeoJson(&text).to_geo().map_err(|e| {
                GeopackageError::IntersectsUnsupported(format!(
                    "the geometry literal is not a supported 2D geometry: {e}"
                ))
            })?
        }
    };
    let needle_bbox = needle.bounding_rect().ok_or_else(|| {
        GeopackageError::IntersectsUnsupported("the geometry literal is empty".to_string())
    })?;
    Ok(IntersectsCheck {
        needle,
        needle_bbox: [
            needle_bbox.min().x,
            needle_bbox.min().y,
            needle_bbox.max().x,
            needle_bbox.max().y,
        ],
    })
}

/// `true` when ISO WKB `wkb`'s own geometry-type code declares a Z and/or M
/// coordinate: `type_code / 1000` is 1 (Z), 2 (M), or 3 (ZM) in the ISO SQL
/// 13249-3 convention the GeoPackage spec mandates for a
/// `StandardGeoPackageBinary` WKB body (`gpb.rs`'s own header doc covers the
/// outer GPB wrapper this body sits inside; this reads the WKB body's own,
/// separate type header). A hand-rolled peek at the same bytes every WKB
/// reader in this workspace already trusts, not a second geometry parse.
fn wkb_has_extra_dimension(wkb: &[u8]) -> Result<bool> {
    if wkb.len() < 5 {
        return Err(GeopackageError::MalformedGeometry(
            "WKB body shorter than its own type header".to_string(),
        ));
    }
    let little_endian = wkb[0] != 0;
    let type_bytes: [u8; 4] = wkb[1..5].try_into().expect("sliced exactly 4 bytes");
    let type_code = if little_endian {
        u32::from_le_bytes(type_bytes)
    } else {
        u32::from_be_bytes(type_bytes)
    };
    Ok(type_code / 1000 != 0)
}

/// Tests one candidate row's decoded geometry (`wkb`: the WKB body a
/// [`crate::gpb::decode`] call already stripped the GPB header from) against
/// `check`'s own needle geometry, refusing by name a Z/M-tagged row this
/// driver's 2D evaluator can't honestly represent rather than silently
/// testing a lossy 2D projection of it.
pub(crate) fn row_intersects(wkb: &[u8], check: &IntersectsCheck) -> Result<bool> {
    if wkb_has_extra_dimension(wkb)? {
        return Err(GeopackageError::IntersectsUnsupported(
            "a stored row geometry carries a Z/M coordinate, which this driver's 2D exact evaluator cannot represent"
                .to_string(),
        ));
    }
    use geozero::ToGeo;
    let row_geom: Geometry<f64> = geozero::wkb::Wkb(wkb)
        .to_geo()
        .map_err(GeopackageError::Geozero)?;
    Ok(row_geom.intersects(&check.needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bbox_literal_converts_to_a_rect() {
        let check = literal_to_check(&GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0])).unwrap();
        assert_eq!(check.needle_bbox, [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn wkt_polygon_literal_converts_and_reports_its_own_bbox() {
        let wkt = WktGeometry::Polygon(vec![vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 0.0]]]);
        let check = literal_to_check(&GeometryLiteral::Wkt(wkt)).unwrap();
        assert_eq!(check.needle_bbox, [0.0, 0.0, 4.0, 4.0]);
    }

    #[test]
    fn geojson_literal_with_a_z_coordinate_is_refused() {
        let value = serde_json::json!({"type": "Point", "coordinates": [1.0, 2.0, 3.0]});
        assert!(matches!(
            literal_to_check(&GeometryLiteral::GeoJson(value)),
            Err(GeopackageError::IntersectsUnsupported(_))
        ));
    }

    #[test]
    fn geojson_literal_with_no_recognizable_geometry_is_refused() {
        // A `FeatureCollection` with no features carries no geometry at all
        // for geozero's reader to emit — the same "not a supported 2D
        // geometry" refusal a malformed/unrecognized shape gets.
        let value = serde_json::json!({"type": "FeatureCollection", "features": []});
        assert!(matches!(
            literal_to_check(&GeometryLiteral::GeoJson(value)),
            Err(GeopackageError::IntersectsUnsupported(_))
        ));
    }

    fn point_wkb(x: f64, y: f64) -> Vec<u8> {
        let text = format!(r#"{{"type":"Point","coordinates":[{x},{y}]}}"#);
        let mut buf = Vec::new();
        let mut writer = geozero::wkb::WkbWriter::new(&mut buf, geozero::wkb::WkbDialect::Wkb);
        geozero::GeozeroGeometry::process_geom(&geozero::geojson::GeoJson(&text), &mut writer)
            .unwrap();
        buf
    }

    #[test]
    fn row_intersects_matches_a_point_inside_the_needle_polygon() {
        let wkt = WktGeometry::Polygon(vec![vec![
            [0.0, 0.0],
            [4.0, 0.0],
            [4.0, 4.0],
            [0.0, 4.0],
            [0.0, 0.0],
        ]]);
        let check = literal_to_check(&GeometryLiteral::Wkt(wkt)).unwrap();
        assert!(row_intersects(&point_wkb(2.0, 2.0), &check).unwrap());
        assert!(!row_intersects(&point_wkb(10.0, 10.0), &check).unwrap());
    }
}
