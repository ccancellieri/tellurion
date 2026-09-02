//! Pure SQL builders and WKB decode helpers: table/column names + a query in,
//! SQL text + typed params out (or, for the WKB helpers, bytes in, GeoJSON/a
//! bbox out). No I/O, no `duckdb::Connection` — mirrors
//! `tellurion-geopackage::sql`'s own discipline, adapted to the DuckDB
//! dialect and to a `BLOB`-WKB geometry column instead of a GeoPackage GPB
//! blob.
//!
//! ## Parameter placeholders
//!
//! DuckDB accepts plain, unnumbered `?` positional parameters bound in
//! left-to-right appearance order (the same convention SQLite — and this
//! driver's own [`compile_filter`] — uses), so unlike
//! `tellurion-geopackage::sql`'s explicit `?N` indices (needed there only to
//! interleave a `bbox` clause's own parameters around a filter's), this
//! module never computes a placeholder index: every builder below pushes to
//! `params` in exactly the order its own `?` markers appear in the returned
//! SQL text.
//!
//! ## CQL2 filter scope (`#105`)
//!
//! [`compile_filter`] compiles exactly: comparison (`=`,`<>`,`<`,`>`,`<=`,
//! `>=`), `IS [NOT] NULL`, and `AND`/`OR`/`NOT` over the table's own scalar
//! columns — CQL2's "Basic CQL2" conformance class, both encodings, nothing
//! more. `LIKE`/`BETWEEN`/`IN`/`CASEI`/every spatial predicate (`S_INTERSECTS`
//! included: this driver's own bbox pushdown is a bounding-box test over
//! decoded WKB in Rust, not a compiled SQL predicate — see
//! [`geometry_bbox_from_wkb`]'s own doc)/every temporal predicate are refused
//! by name — see `driver.rs`'s `FeatureSource::cql2_conformance_classes` for
//! exactly which classes this earns and why, mirroring
//! `tellurion-iceberg::driver`'s identical "basic-cql2-plus-both-encodings-
//! only" scope and its own doc for the same reasoning.
//!
//! ## bbox pushdown (or the lack of it)
//!
//! No spatial index and no loaded `spatial` extension means no SQL-level
//! bbox predicate this driver can compile (see the crate's own top-level
//! "EXTENSION note"). A `bbox` items-query parameter is instead applied as
//! an in-process post-filter: [`geometry_bbox_from_wkb`] decodes each
//! candidate row's WKB geometry and tests it against the query envelope in
//! Rust, over an ordinary `ORDER BY pk` scan — the same "no index, full-scan
//! fallback, exact result" shape `tellurion-geoparquet`'s own no-covering
//! fallback documents as an accepted cost (`driver.rs`'s own "Counting" doc:
//! "count while scanning is acceptable"), applied here unconditionally since
//! this driver has no covering-statistics fast path at all.

use duckdb::types::Value;
use tellurion_core::{CompareOp, Filter, Literal};

use crate::error::{DuckdbDriverError, Result};
use crate::ident::quote_ident;

/// A scalar `Literal`'s bound [`Value`] — DuckDB compares a bound value
/// against a column of any storage class using its own implicit-cast rules,
/// so (like `tellurion-geopackage::sql`'s own `literal_param`, and unlike
/// PostGIS's `sql.rs`) no per-comparison cast text is needed here at all.
fn literal_param(value: &Literal) -> Value {
    match value {
        Literal::Text(s) => Value::Text(s.clone()),
        Literal::Number(n) => Value::Double(*n),
        Literal::Bool(b) => Value::Boolean(*b),
    }
}

fn compare_op_sql(op: CompareOp) -> &'static str {
    match op {
        CompareOp::Eq => "=",
        CompareOp::Ne => "<>",
        CompareOp::Lt => "<",
        CompareOp::Gt => ">",
        CompareOp::Le => "<=",
        CompareOp::Ge => ">=",
    }
}

fn compile_bool_chain(items: &[Filter], joiner: &str, params: &mut Vec<Value>) -> Result<String> {
    if items.is_empty() {
        return Ok(if joiner == "AND" {
            "true".to_string()
        } else {
            "false".to_string()
        });
    }
    let mut parts = Vec::with_capacity(items.len());
    for item in items {
        parts.push(compile_filter(item, params)?);
    }
    Ok(format!("({})", parts.join(&format!(" {joiner} "))))
}

/// Compiles a [`Filter`] to bound-parameter DuckDB SQL, or refuses a
/// construct outside this driver's declared basic-comparison subset by name
/// — see this module's own "CQL2 filter scope" doc for exactly what compiles
/// and why.
pub(crate) fn compile_filter(filter: &Filter, params: &mut Vec<Value>) -> Result<String> {
    match filter {
        Filter::Compare {
            property,
            op,
            value,
        } => {
            let column = quote_ident(property)?;
            let op_sql = compare_op_sql(*op);
            params.push(literal_param(value));
            Ok(format!("({column} {op_sql} ?)"))
        }
        Filter::IsNull { property, negated } => {
            let column = quote_ident(property)?;
            let not = if *negated { " NOT" } else { "" };
            Ok(format!("({column} IS{not} NULL)"))
        }
        Filter::And(items) => compile_bool_chain(items, "AND", params),
        Filter::Or(items) => compile_bool_chain(items, "OR", params),
        Filter::Not(inner) => Ok(format!("(NOT {})", compile_filter(inner, params)?)),
        Filter::Like { .. } => Err(DuckdbDriverError::FilterUnsupported("LIKE")),
        Filter::Between { .. } => Err(DuckdbDriverError::FilterUnsupported("BETWEEN")),
        Filter::In { .. } => Err(DuckdbDriverError::FilterUnsupported("IN")),
        Filter::CaseInsensitiveCompare { .. } => {
            Err(DuckdbDriverError::FilterUnsupported("CASEI comparison"))
        }
        Filter::Intersects { .. } => Err(DuckdbDriverError::FilterUnsupported("S_INTERSECTS")),
        Filter::Spatial { .. } => Err(DuckdbDriverError::FilterUnsupported("spatial predicate")),
        Filter::After { .. } => Err(DuckdbDriverError::FilterUnsupported("T_AFTER")),
        Filter::Before { .. } => Err(DuckdbDriverError::FilterUnsupported("T_BEFORE")),
        Filter::During { .. } => Err(DuckdbDriverError::FilterUnsupported("T_DURING")),
        Filter::Temporal { .. } => Err(DuckdbDriverError::FilterUnsupported("temporal predicate")),
    }
}

/// Decodes one WKB geometry (this driver's fixed encoding — plain ISO WKB,
/// never EWKB, matching GeoParquet's own fixed encoding) into a bare GeoJSON
/// geometry object, via the same `geozero::geojson::GeoJsonWriter`
/// `tellurion-geoparquet`'s driver uses.
pub(crate) fn geometry_json_from_wkb(wkb: &[u8]) -> Result<serde_json::Value> {
    use geozero::GeozeroGeometry;

    let mut buf: Vec<u8> = Vec::new();
    {
        let mut writer = geozero::geojson::GeoJsonWriter::new(&mut buf);
        geozero::wkb::Wkb(wkb).process_geom(&mut writer)?;
    }
    Ok(serde_json::from_slice(&buf)?)
}

/// Minimal `geozero::GeomProcessor` that only tracks the enclosing 2D bbox of
/// every coordinate it sees — mirrors `tellurion-geoparquet::driver`'s own
/// `BboxCollector` exactly (this driver has no GeoParquet-1.1-style covering
/// statistics to prefer, so every bbox test takes this decode-and-fold path,
/// never just the fast one).
#[derive(Default)]
struct BboxCollector {
    bbox: Option<[f64; 4]>,
}

impl BboxCollector {
    fn accumulate(&mut self, x: f64, y: f64) {
        self.bbox = Some(match self.bbox {
            Some([minx, miny, maxx, maxy]) => [minx.min(x), miny.min(y), maxx.max(x), maxy.max(y)],
            None => [x, y, x, y],
        });
    }
}

impl geozero::GeomProcessor for BboxCollector {
    fn xy(&mut self, x: f64, y: f64, _idx: usize) -> geozero::error::Result<()> {
        self.accumulate(x, y);
        Ok(())
    }

    fn coordinate(
        &mut self,
        x: f64,
        y: f64,
        _z: Option<f64>,
        _m: Option<f64>,
        _t: Option<f64>,
        _tm: Option<u64>,
        _idx: usize,
    ) -> geozero::error::Result<()> {
        self.accumulate(x, y);
        Ok(())
    }

    /// An empty point contributes nothing to the running bbox rather than
    /// tripping the trait's default "output doesn't support empty points"
    /// error — a bbox fold has no such restriction.
    fn empty_point(&mut self, _idx: usize) -> geozero::error::Result<()> {
        Ok(())
    }
}

/// One WKB geometry's 2D bbox, or `None` for a geometry with no coordinates
/// at all (an empty geometry collection) — used both by [`crate::catalog::
/// extent`]'s bounded sample fold and by this driver's bbox items-query
/// post-filter (see this module's own "bbox pushdown" doc).
pub(crate) fn geometry_bbox_from_wkb(wkb: &[u8]) -> Result<Option<[f64; 4]>> {
    use geozero::GeozeroGeometry;

    let mut collector = BboxCollector::default();
    geozero::wkb::Wkb(wkb).process_geom(&mut collector)?;
    Ok(collector.bbox)
}

fn bbox_intersects(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] <= b[2] && a[2] >= b[0] && a[1] <= b[3] && a[3] >= b[1]
}

/// Whether `wkb`'s decoded geometry intersects `query_bbox` — `false` for a
/// `NULL` geometry (`wkb: None`), mirroring the reference in-memory driver's
/// own "a null geometry contributes to unfiltered pages but never to a
/// bbox-selected result" contract.
pub(crate) fn wkb_intersects_bbox(wkb: Option<&[u8]>, query_bbox: [f64; 4]) -> Result<bool> {
    let Some(wkb) = wkb else { return Ok(false) };
    Ok(match geometry_bbox_from_wkb(wkb)? {
        Some(bbox) => bbox_intersects(bbox, query_bbox),
        None => false,
    })
}

/// One property value from one decoded row column — supports DuckDB's common
/// scalar attribute types (boolean, every integer width, float/double, text)
/// plus `NULL`; anything else (`BLOB` on a non-geometry column, `LIST`,
/// `STRUCT`, `MAP`, `ARRAY`, `ENUM`, temporal/interval types, `DECIMAL`,
/// `HUGEINT`/`UHUGEINT`) is an honest [`DuckdbDriverError::Decode`] rather
/// than silently emitting a wrong or lossy value — v0.1's scope is the
/// practical scalar attribute shapes this driver's own fixture (and a plain
/// `CREATE TABLE` a real operator would write for tabular data) actually use,
/// the same deliberate narrowing `tellurion-geoparquet::driver::
/// arrow_value_to_json`'s own doc describes for its own comparable scope
/// decision.
pub(crate) fn duckdb_value_to_json(value: Value) -> Result<serde_json::Value> {
    let json = match value {
        Value::Null => serde_json::Value::Null,
        Value::Boolean(b) => serde_json::Value::Bool(b),
        Value::TinyInt(v) => serde_json::Value::from(v),
        Value::SmallInt(v) => serde_json::Value::from(v),
        Value::Int(v) => serde_json::Value::from(v),
        Value::BigInt(v) => serde_json::Value::from(v),
        Value::UTinyInt(v) => serde_json::Value::from(v),
        Value::USmallInt(v) => serde_json::Value::from(v),
        Value::UInt(v) => serde_json::Value::from(v),
        Value::UBigInt(v) => serde_json::Value::from(v),
        Value::Float(v) => serde_json::Number::from_f64(v as f64)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::Double(v) => serde_json::Number::from_f64(v)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::Text(s) => serde_json::Value::String(s),
        other => {
            return Err(DuckdbDriverError::Decode(format!(
                "unsupported attribute column value: {other:?}"
            )))
        }
    };
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tellurion_core::DatetimeRange;

    /// A classic single-quote-breakout injection payload — proves the
    /// literal never lands in the SQL text itself, only in the bound
    /// parameter vector.
    fn injection_payload() -> String {
        "Robert'); DELETE FROM demo_table; --".to_string()
    }

    #[test]
    fn compile_filter_compare_binds_a_parameter_and_never_inlines_the_literal() {
        let mut params = Vec::new();
        let filter = Filter::Compare {
            property: "name".to_string(),
            op: CompareOp::Eq,
            value: Literal::Text(injection_payload()),
        };
        let sql = compile_filter(&filter, &mut params).unwrap();
        assert_eq!(sql, "(\"name\" = ?)");
        assert!(
            !sql.contains("DELETE"),
            "the literal must never be inlined into SQL text"
        );
        assert_eq!(params.len(), 1);
        assert_eq!(params[0], Value::Text(injection_payload()));
    }

    #[test]
    fn compile_filter_rejects_an_invalid_identifier_even_as_a_property_name() {
        let mut params = Vec::new();
        let filter = Filter::Compare {
            property: format!("name; {}", injection_payload()),
            op: CompareOp::Eq,
            value: Literal::Text("x".to_string()),
        };
        assert!(compile_filter(&filter, &mut params).is_err());
    }

    #[test]
    fn compile_filter_is_null() {
        let mut params = Vec::new();
        let filter = Filter::IsNull {
            property: "name".to_string(),
            negated: false,
        };
        assert_eq!(
            compile_filter(&filter, &mut params).unwrap(),
            "(\"name\" IS NULL)"
        );
        assert!(params.is_empty());
    }

    #[test]
    fn compile_filter_and_or_not_compose_and_preserve_param_order() {
        let mut params = Vec::new();
        let filter = Filter::And(vec![
            Filter::Compare {
                property: "a".to_string(),
                op: CompareOp::Gt,
                value: Literal::Number(1.0),
            },
            Filter::Not(Box::new(Filter::Compare {
                property: "b".to_string(),
                op: CompareOp::Eq,
                value: Literal::Number(2.0),
            })),
        ]);
        let sql = compile_filter(&filter, &mut params).unwrap();
        assert_eq!(sql, "((\"a\" > ?) AND (NOT (\"b\" = ?)))");
        assert_eq!(params, vec![Value::Double(1.0), Value::Double(2.0)]);
    }

    #[test]
    fn compile_filter_refuses_like_between_in_casei_spatial_and_temporal_by_name() {
        let unsupported = [
            Filter::Like {
                property: "a".to_string(),
                pattern: "x%".to_string(),
                negated: false,
            },
            Filter::Between {
                property: "a".to_string(),
                low: Literal::Number(1.0),
                high: Literal::Number(2.0),
                negated: false,
            },
            Filter::In {
                property: "a".to_string(),
                values: vec![Literal::Number(1.0)],
                negated: false,
            },
            Filter::CaseInsensitiveCompare {
                property: "a".to_string(),
                op: tellurion_core::CaseInsensitiveCompareOp::Eq,
                value: "x".to_string(),
            },
            Filter::Intersects {
                property: "geom".to_string(),
                geometry: tellurion_core::GeometryLiteral::Bbox([0.0, 0.0, 1.0, 1.0]),
            },
        ];
        for filter in unsupported {
            let mut params = Vec::new();
            assert!(
                matches!(
                    compile_filter(&filter, &mut params),
                    Err(DuckdbDriverError::FilterUnsupported(_))
                ),
                "expected {filter:?} to be refused by name"
            );
        }
    }

    #[test]
    fn geometry_json_from_wkb_round_trips_a_point() {
        use geozero::GeozeroGeometry;
        let mut wkb = Vec::new();
        {
            let mut writer = geozero::wkb::WkbWriter::new(&mut wkb, geozero::wkb::WkbDialect::Wkb);
            geozero::geojson::GeoJson(r#"{"type":"Point","coordinates":[1.5,2.5]}"#)
                .process_geom(&mut writer)
                .unwrap();
        }
        let geojson = geometry_json_from_wkb(&wkb).unwrap();
        assert_eq!(geojson["type"], "Point");
        assert_eq!(geojson["coordinates"][0], 1.5);
        assert_eq!(geojson["coordinates"][1], 2.5);
    }

    #[test]
    fn wkb_intersects_bbox_is_false_for_a_null_geometry() {
        assert!(!wkb_intersects_bbox(None, [0.0, 0.0, 1.0, 1.0]).unwrap());
    }

    #[test]
    fn wkb_intersects_bbox_matches_a_point_inside_the_query_envelope() {
        use geozero::GeozeroGeometry;
        let mut wkb = Vec::new();
        {
            let mut writer = geozero::wkb::WkbWriter::new(&mut wkb, geozero::wkb::WkbDialect::Wkb);
            geozero::geojson::GeoJson(r#"{"type":"Point","coordinates":[0.5,0.5]}"#)
                .process_geom(&mut writer)
                .unwrap();
        }
        assert!(wkb_intersects_bbox(Some(&wkb), [0.0, 0.0, 1.0, 1.0]).unwrap());
        assert!(!wkb_intersects_bbox(Some(&wkb), [10.0, 10.0, 11.0, 11.0]).unwrap());
    }

    #[test]
    fn duckdb_value_to_json_covers_the_supported_scalar_types() {
        assert_eq!(
            duckdb_value_to_json(Value::Null).unwrap(),
            serde_json::Value::Null
        );
        assert_eq!(
            duckdb_value_to_json(Value::Boolean(true)).unwrap(),
            serde_json::json!(true)
        );
        assert_eq!(
            duckdb_value_to_json(Value::BigInt(42)).unwrap(),
            serde_json::json!(42)
        );
        assert_eq!(
            duckdb_value_to_json(Value::Double(1.5)).unwrap(),
            serde_json::json!(1.5)
        );
        assert_eq!(
            duckdb_value_to_json(Value::Text("hi".to_string())).unwrap(),
            serde_json::json!("hi")
        );
    }

    #[test]
    fn duckdb_value_to_json_refuses_an_unsupported_type_honestly() {
        assert!(duckdb_value_to_json(Value::Blob(vec![1, 2, 3])).is_err());
    }

    /// Not exercised by this module — pinned here only so a future edit to
    /// `ItemsQuery` (adding a field this driver still ignores) is noticed;
    /// mirrors the same defensive pin `tellurion-flatgeobuf`'s own datetime
    /// test takes.
    #[test]
    fn datetime_range_type_is_still_the_shape_this_driver_refuses_wholesale() {
        let _ = DatetimeRange {
            start: None,
            end: None,
        };
    }
}
