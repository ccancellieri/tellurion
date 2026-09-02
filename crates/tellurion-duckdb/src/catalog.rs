//! Introspection against DuckDB's own standard catalog views —
//! `information_schema.tables`/`.columns` for table/column existence and
//! declared types, `duckdb_constraints()` (DuckDB's native system table
//! function, not a SQLite-style `PRAGMA`) for primary-key detection — plus a
//! bounded, sampled extent computation over the geometry column. Pure
//! `&duckdb::Connection -> Result<...>` functions — no async, no pooling
//! concern — called from `driver.rs` inside a `pool::with_reader` closure (or,
//! for `validate_collection`'s cheap boot-time check, directly against a
//! locked pool connection — see that function's own doc for why it alone
//! runs synchronously).
//!
//! ## Geometry-column auto-detection
//!
//! Nothing in a plain DuckDB catalog marks "the" geometry column the way
//! GeoPackage's `gpkg_geometry_columns` or a GeoParquet file's own `"geo"`
//! metadata do (see the crate's own top-level "EXTENSION note" for why this
//! driver never loads the `spatial` extension's registry either). This
//! module's own convention, applied consistently by every function below
//! that needs a geometry column and wasn't handed an explicit
//! `CollectionDecl.geometry` override: a table with **exactly one** `BLOB`
//! column reports that column as its geometry column; zero or more than one
//! candidate is ambiguous and reports `None` (ordinary collection catalog
//! reporting, [`table_shape`]) or fails with a named error requiring an
//! explicit `geometry:` override (boot validation, [`resolve_table_shape`]) —
//! the same "exactly one candidate, else unknown" idiom
//! `tellurion_core::catalog::CatalogSource::temporal_column`'s own doc
//! describes, applied here to a column's storage type instead of its name.

use duckdb::types::Value;
use duckdb::Connection;

use crate::error::{DuckdbDriverError, Result};
use crate::ident::{quote_ident, quote_literal};
use crate::sql::geometry_bbox_from_wkb;

/// One declared column: its name and DuckDB's own broad type name
/// (`information_schema.columns.data_type`, e.g. `"BIGINT"`, `"VARCHAR"`,
/// `"BLOB"`) — mirrors `tellurion-postgis`'s identical
/// `information_schema.columns.data_type` convention for the same
/// "backend's own broad type name, never a guess" contract.
#[derive(Debug, Clone)]
pub(crate) struct ColumnInfo {
    pub(crate) name: String,
    pub(crate) sql_type: String,
}

/// Whether `table` exists as an ordinary base table in this database — a
/// view is deliberately excluded: this driver's declared collection must be
/// a real table, since a view has no stable row identity a keyset `pk`
/// could order over any more safely than a query the operator could just as
/// well point at the base table directly.
pub(crate) fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM information_schema.tables \
         WHERE table_name = ? AND table_type = 'BASE TABLE'",
        [table],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Every column `table` declares, in ordinal order. Empty (never an error)
/// when `table` doesn't exist — callers that need existence checked call
/// [`table_exists`] first.
pub(crate) fn list_columns(conn: &Connection, table: &str) -> Result<Vec<ColumnInfo>> {
    let mut stmt = conn.prepare(
        "SELECT column_name, data_type FROM information_schema.columns \
         WHERE table_name = ? ORDER BY ordinal_position",
    )?;
    let rows = stmt.query_map([table], |row| {
        Ok(ColumnInfo {
            name: row.get(0)?,
            sql_type: row.get(1)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// The single `BLOB`-typed column among `columns`, when there is exactly
/// one — this module's own "geometry-column auto-detection" convention
/// (see the module doc).
fn single_blob_column(columns: &[ColumnInfo]) -> Option<&str> {
    let mut candidates = columns
        .iter()
        .filter(|c| c.sql_type.eq_ignore_ascii_case("BLOB"));
    match (candidates.next(), candidates.next()) {
        (Some(only), None) => Some(only.name.as_str()),
        _ => None,
    }
}

/// DuckDB's integer family — a keyset `pk` must be one of these (the same
/// "v0.1 assumes a single-column integer primary key" limitation every other
/// relational driver in this workspace documents), since paging compares it
/// with `>`/`ORDER BY` and encodes it as plain decimal text in a
/// [`crate::sql`] token.
fn is_integer_type(sql_type: &str) -> bool {
    matches!(
        sql_type.to_ascii_uppercase().as_str(),
        "TINYINT"
            | "SMALLINT"
            | "INTEGER"
            | "BIGINT"
            | "HUGEINT"
            | "UTINYINT"
            | "USMALLINT"
            | "UINTEGER"
            | "UBIGINT"
    )
}

/// One of DuckDB's timestamp/date family — the single-candidate heuristic
/// [`table_shape`]'s `temporal_column` result applies, mirroring
/// `tellurion-geopackage::catalog::temporal_column`'s identical "exactly one
/// DATE/DATETIME-typed column, else `None`" contract, widened to DuckDB's
/// own richer temporal type names.
fn is_temporal_type(sql_type: &str) -> bool {
    let upper = sql_type.to_ascii_uppercase();
    upper.starts_with("TIMESTAMP") || upper == "DATE"
}

/// The one column `duckdb_constraints()` reports as this table's
/// `PRIMARY KEY`, when it is a single column — `None` for a composite key
/// (more than one column) or a table declared with no `PRIMARY KEY`
/// constraint at all, mirroring `tellurion-geopackage::catalog::
/// integer_primary_key`'s identical "exactly one candidate, else `None`"
/// shape (that driver reads `PRAGMA table_info`'s `pk` rank instead, DuckDB's
/// own SQLite-pragma compatibility layer; this driver uses DuckDB's native
/// system table instead, since it is the documented, non-compatibility-shim
/// surface for this information).
pub(crate) fn primary_key_column(conn: &Connection, table: &str) -> Result<Option<String>> {
    let literal = quote_literal(table)?;
    let mut stmt = conn.prepare(&format!(
        "SELECT constraint_column_names FROM duckdb_constraints() \
         WHERE table_name = {literal} AND constraint_type = 'PRIMARY KEY'"
    ))?;
    let rows = stmt.query_map([], |row| row.get::<_, Value>(0))?;

    let mut candidates: Vec<String> = Vec::new();
    for row in rows {
        if let Value::List(items) = row? {
            for item in items {
                if let Value::Text(name) = item {
                    candidates.push(name);
                }
            }
        }
    }
    Ok(match candidates.len() {
        1 => candidates.into_iter().next(),
        _ => None,
    })
}

/// Best-effort physical shape for one table, as reported by `CatalogSource::
/// collections` — never fails, and never consults a `CollectionDecl`
/// override: an ambiguous or absent geometry column reports `None`, the same
/// "backend cannot answer" shape `extent`/`row_estimate` already use,
/// requiring the operator to pin `CollectionDecl.geometry` explicitly (see
/// the crate's own driver-authoring-aligned `require_feature_capable` boot
/// check in `tellurion_core::descriptor`).
pub(crate) struct PhysicalShape {
    pub(crate) geometry_column: Option<String>,
    pub(crate) primary_key: Option<String>,
    pub(crate) temporal_column: Option<String>,
}

pub(crate) fn physical_shape(conn: &Connection, table: &str) -> Result<PhysicalShape> {
    let columns = list_columns(conn, table)?;
    let geometry_column = single_blob_column(&columns).map(str::to_string);
    let primary_key = primary_key_column(conn, table)?;
    let mut temporal_candidates = columns
        .iter()
        .filter(|c| is_temporal_type(&c.sql_type))
        .map(|c| c.name.as_str());
    let temporal_column = match (temporal_candidates.next(), temporal_candidates.next()) {
        (Some(only), None) => Some(only.to_string()),
        _ => None,
    };
    Ok(PhysicalShape {
        geometry_column,
        primary_key,
        temporal_column,
    })
}

/// Resolved, validated physical shape for one *declared* collection —
/// override-aware (unlike [`physical_shape`] above) and fails, by name,
/// instead of silently reporting `None`. Used both by boot-time validation
/// and, cached, by every query this driver actually runs (see `driver.rs`'s
/// `DuckdbBackend::resolved_shape`).
pub(crate) struct TableShape {
    pub(crate) table: String,
    pub(crate) geometry_column: String,
    pub(crate) primary_key: String,
    /// Every declared column on this table, geometry included — callers
    /// filter out `geometry_column` themselves where that matters (attribute
    /// listing, property projection), the same "exclude only what's
    /// structural, right at the point of use" shape
    /// `tellurion-geoparquet::driver`'s own `attribute_schema_inner` takes.
    pub(crate) columns: Vec<ColumnInfo>,
}

/// Resolves and validates one declared collection's physical shape against
/// `table`'s real schema: the table must exist; the geometry column —
/// `geometry_override` when the operator declared one, else this module's
/// single-`BLOB`-column auto-detection — must exist and be a `BLOB` (see the
/// crate's own top-level "EXTENSION note" for why this driver never expects
/// DuckDB's native `spatial`-extension `GEOMETRY` type); the primary key —
/// `pk_override` when declared, else [`primary_key_column`]'s own detection —
/// must exist and be an integer type. Every failure names the collection,
/// table, and specific column so a config typo or an unprovisioned table is
/// diagnosable from the boot error alone.
pub(crate) fn resolve_table_shape(
    conn: &Connection,
    collection: &str,
    table: &str,
    geometry_override: Option<&str>,
    pk_override: Option<&str>,
) -> Result<TableShape> {
    if !table_exists(conn, table)? {
        return Err(DuckdbDriverError::MissingTable {
            collection: collection.to_string(),
            table: table.to_string(),
        });
    }
    let columns = list_columns(conn, table)?;
    let column_type = |name: &str| {
        columns
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.sql_type.as_str())
    };

    let geometry_column = match geometry_override {
        Some(name) => name.to_string(),
        None => single_blob_column(&columns)
            .ok_or_else(|| DuckdbDriverError::AmbiguousGeometryColumn {
                collection: collection.to_string(),
                table: table.to_string(),
            })?
            .to_string(),
    };
    let geometry_type =
        column_type(&geometry_column).ok_or_else(|| DuckdbDriverError::MissingGeometryColumn {
            collection: collection.to_string(),
            table: table.to_string(),
            column: geometry_column.clone(),
        })?;
    if !geometry_type.eq_ignore_ascii_case("BLOB") {
        return Err(DuckdbDriverError::GeometryColumnNotBlob {
            collection: collection.to_string(),
            table: table.to_string(),
            column: geometry_column.clone(),
            sql_type: geometry_type.to_string(),
        });
    }

    let primary_key = match pk_override {
        Some(pk) => pk.to_string(),
        None => {
            primary_key_column(conn, table)?.ok_or_else(|| DuckdbDriverError::NoPrimaryKey {
                collection: collection.to_string(),
                table: table.to_string(),
            })?
        }
    };
    let pk_type =
        column_type(&primary_key).ok_or_else(|| DuckdbDriverError::MissingPrimaryKeyColumn {
            collection: collection.to_string(),
            table: table.to_string(),
            pk: primary_key.clone(),
        })?;
    if !is_integer_type(pk_type) {
        return Err(DuckdbDriverError::PrimaryKeyNotInteger {
            collection: collection.to_string(),
            table: table.to_string(),
            pk: primary_key.clone(),
            sql_type: pk_type.to_string(),
        });
    }

    Ok(TableShape {
        table: table.to_string(),
        geometry_column,
        primary_key,
        columns,
    })
}

/// An exact `COUNT(*)` — cheap in a columnar engine (DuckDB answers a bare,
/// unfiltered `COUNT(*)` from each column's own zonemap/row-group metadata,
/// never a full per-row scan), so unlike `tellurion-geoparquet`'s identically
/// exact-and-free file-metadata row count, this one is a real query — just a
/// metadata-only one — rather than a value already sitting in a cached
/// header.
pub(crate) fn row_estimate(conn: &Connection, table: &str) -> Result<u64> {
    let ident = quote_ident(table)?;
    let count: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {ident}"), [], |row| {
        row.get(0)
    })?;
    Ok(u64::try_from(count).unwrap_or(0))
}

/// Bounded, approximate spatial extent: folds the bbox of the first
/// `sample_limit` rows' geometries, in the table's own physical scan order —
/// an approximation in the same spirit as `tellurion_core::catalog::
/// GeometryProfile`'s own bounded sample (never a full-table scan just to
/// answer one extent), but deliberately simpler than a uniform random
/// sample: the first N rows of physical order, not `USING SAMPLE`, because
/// DuckDB's own `USING SAMPLE` reservoir algorithm still has to visit the
/// whole table to guarantee uniformity, which defeats the point for a table
/// this driver has no size ceiling on. `Ok(None)` for an empty table or one
/// whose sampled rows are all `NULL` geometries.
pub(crate) fn extent(
    conn: &Connection,
    table: &str,
    geometry_column: &str,
    sample_limit: u32,
) -> Result<Option<[f64; 4]>> {
    let table_ident = quote_ident(table)?;
    let geom_ident = quote_ident(geometry_column)?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {geom_ident} FROM {table_ident} LIMIT {sample_limit}"
    ))?;
    let rows = stmt.query_map([], |row| row.get::<_, Option<Vec<u8>>>(0))?;

    let mut bbox: Option<[f64; 4]> = None;
    for row in rows {
        let Some(wkb) = row? else { continue };
        let Some(feature_bbox) = geometry_bbox_from_wkb(&wkb)? else {
            continue;
        };
        bbox = Some(match bbox {
            None => feature_bbox,
            Some([minx, miny, maxx, maxy]) => [
                minx.min(feature_bbox[0]),
                miny.min(feature_bbox[1]),
                maxx.max(feature_bbox[2]),
                maxy.max(feature_bbox[3]),
            ],
        });
    }
    Ok(bbox)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE demo (id BIGINT PRIMARY KEY, geom BLOB, name VARCHAR, observed_at TIMESTAMP);
             CREATE TABLE no_pk (id BIGINT, geom BLOB);
             CREATE TABLE text_pk (id VARCHAR PRIMARY KEY, geom BLOB);
             CREATE TABLE two_blobs (id BIGINT PRIMARY KEY, a BLOB, b BLOB);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn table_exists_is_true_for_a_real_table_and_false_otherwise() {
        let conn = fixture();
        assert!(table_exists(&conn, "demo").unwrap());
        assert!(!table_exists(&conn, "nope").unwrap());
    }

    #[test]
    fn list_columns_reports_duckdb_type_names_in_ordinal_order() {
        let conn = fixture();
        let columns = list_columns(&conn, "demo").unwrap();
        let names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["id", "geom", "name", "observed_at"]);
        assert_eq!(columns[1].sql_type, "BLOB");
    }

    #[test]
    fn primary_key_column_finds_the_single_column_pk() {
        let conn = fixture();
        assert_eq!(
            primary_key_column(&conn, "demo").unwrap().as_deref(),
            Some("id")
        );
    }

    #[test]
    fn primary_key_column_is_none_when_the_table_declares_no_pk() {
        let conn = fixture();
        assert_eq!(primary_key_column(&conn, "no_pk").unwrap(), None);
    }

    #[test]
    fn physical_shape_auto_detects_the_single_blob_column_pk_and_temporal_column() {
        let conn = fixture();
        let shape = physical_shape(&conn, "demo").unwrap();
        assert_eq!(shape.geometry_column.as_deref(), Some("geom"));
        assert_eq!(shape.primary_key.as_deref(), Some("id"));
        assert_eq!(shape.temporal_column.as_deref(), Some("observed_at"));
    }

    #[test]
    fn physical_shape_reports_no_geometry_column_when_two_blob_columns_are_ambiguous() {
        let conn = fixture();
        let shape = physical_shape(&conn, "two_blobs").unwrap();
        assert_eq!(shape.geometry_column, None);
    }

    #[test]
    fn resolve_table_shape_accepts_a_well_formed_table() {
        let conn = fixture();
        let shape = resolve_table_shape(&conn, "demo_collection", "demo", None, None).unwrap();
        assert_eq!(shape.primary_key, "id");
        assert_eq!(shape.geometry_column, "geom");
    }

    #[test]
    fn resolve_table_shape_rejects_a_missing_table() {
        let conn = fixture();
        assert!(matches!(
            resolve_table_shape(&conn, "demo_collection", "nope", None, None),
            Err(DuckdbDriverError::MissingTable { .. })
        ));
    }

    #[test]
    fn resolve_table_shape_rejects_an_ambiguous_geometry_column_with_no_override() {
        let conn = fixture();
        assert!(matches!(
            resolve_table_shape(&conn, "demo_collection", "two_blobs", None, None),
            Err(DuckdbDriverError::AmbiguousGeometryColumn { .. })
        ));
    }

    #[test]
    fn resolve_table_shape_accepts_an_explicit_geometry_override_that_disambiguates() {
        let conn = fixture();
        let shape =
            resolve_table_shape(&conn, "demo_collection", "two_blobs", Some("b"), None).unwrap();
        assert_eq!(shape.geometry_column, "b");
    }

    #[test]
    fn resolve_table_shape_rejects_a_geometry_override_naming_a_non_blob_column() {
        let conn = fixture();
        assert!(matches!(
            resolve_table_shape(&conn, "demo_collection", "demo", Some("name"), None),
            Err(DuckdbDriverError::GeometryColumnNotBlob { .. })
        ));
    }

    #[test]
    fn resolve_table_shape_rejects_a_table_with_no_primary_key_and_no_override() {
        let conn = fixture();
        assert!(matches!(
            resolve_table_shape(&conn, "demo_collection", "no_pk", None, None),
            Err(DuckdbDriverError::NoPrimaryKey { .. })
        ));
    }

    #[test]
    fn resolve_table_shape_rejects_a_non_integer_primary_key() {
        let conn = fixture();
        assert!(matches!(
            resolve_table_shape(&conn, "demo_collection", "text_pk", None, None),
            Err(DuckdbDriverError::PrimaryKeyNotInteger { .. })
        ));
    }

    #[test]
    fn resolve_table_shape_accepts_an_explicit_pk_override_on_a_pk_less_table() {
        let conn = fixture();
        let shape =
            resolve_table_shape(&conn, "demo_collection", "no_pk", None, Some("id")).unwrap();
        assert_eq!(shape.primary_key, "id");
    }

    #[test]
    fn row_estimate_counts_rows_exactly() {
        let conn = fixture();
        conn.execute_batch(
            "INSERT INTO demo (id, geom, name) VALUES (1, NULL, 'a'), (2, NULL, 'b')",
        )
        .unwrap();
        assert_eq!(row_estimate(&conn, "demo").unwrap(), 2);
    }

    #[test]
    fn extent_is_none_for_an_empty_table() {
        let conn = fixture();
        assert_eq!(extent(&conn, "demo", "geom", 100).unwrap(), None);
    }
}
