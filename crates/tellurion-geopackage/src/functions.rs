//! Registers the small set of SQL scalar functions the GeoPackage spec's
//! R*Tree Spatial Index Extension trigger templates call (`ST_MinX`,
//! `ST_MaxX`, `ST_MinY`, `ST_MaxY`, `ST_IsEmpty`) — see
//! `tellurion-ingest`'s own geopackage provisioning module for the exact
//! trigger SQL that invokes them.
//!
//! These five names are ordinarily supplied by loading SpatiaLite as a
//! runtime extension; this driver never does that (no system dependency,
//! per the crate's own top-level "self-contained" doc), so it implements
//! just the five the triggers actually reference, directly against the GPB
//! header this crate already parses for every other purpose (`gpb.rs`) —
//! not a general-purpose spatial SQL function library.
//!
//! Must be called once per opened [`rusqlite::Connection`] before any INSERT/
//! UPDATE/DELETE against a provisioned feature table runs — only a write
//! connection's own trigger firings ever call these; a read-only connection
//! never executes an INSERT/UPDATE/DELETE, so it never needs them, but this
//! driver registers them uniformly on every connection it opens anyway (see
//! `pool.rs`) rather than special-casing the one connection that does.

use rusqlite::functions::FunctionFlags;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, Error as SqliteError, Result as SqliteResult};

use crate::gpb;

const FLAGS: FunctionFlags = FunctionFlags::SQLITE_UTF8.union(FunctionFlags::SQLITE_DETERMINISTIC);

fn user_error(error: crate::error::GeopackageError) -> SqliteError {
    SqliteError::UserFunctionError(Box::new(error))
}

/// The requested geometry BLOB argument's 2D envelope: `Ok(None)` for a SQL
/// `NULL` argument or a geometry that decodes empty (both make every
/// `ST_MinX`/`ST_MaxX`/`ST_MinY`/`ST_MaxY` call answer SQL `NULL`, which is
/// exactly what leaves the RTree-trigger `WHEN` clauses' three-valued AND/OR
/// logic evaluating correctly without any short-circuit assumption — see
/// `tellurion-ingest`'s trigger SQL doc comment). Prefers the envelope this
/// driver's own [`gpb::encode_from_geojson_geometry`] always stores in the
/// header; falls back to decoding the WKB body only for a blob that (unlike
/// anything this driver itself writes) carries none.
fn envelope_of(ctx: &rusqlite::functions::Context<'_>) -> SqliteResult<Option<[f64; 4]>> {
    let raw = ctx.get_raw(0);
    let blob = match raw {
        ValueRef::Null => return Ok(None),
        other => other.as_blob().map_err(SqliteError::from)?,
    };
    gpb::envelope_of_blob(blob).map_err(user_error)
}

/// Registers `ST_MinX`/`ST_MaxX`/`ST_MinY`/`ST_MaxY`/`ST_IsEmpty` on `conn` —
/// idempotent (`CREATE`-style registration overwrites any prior definition
/// of the same name/arity on this connection, matching SQLite's own
/// `sqlite3_create_function` semantics), so safe to call once per freshly
/// opened connection with no "already registered" bookkeeping needed.
pub(crate) fn register(conn: &Connection) -> SqliteResult<()> {
    conn.create_scalar_function("ST_MinX", 1, FLAGS, move |ctx| {
        Ok(envelope_of(ctx)?.map(|[minx, _, _, _]| minx))
    })?;
    conn.create_scalar_function("ST_MaxX", 1, FLAGS, move |ctx| {
        Ok(envelope_of(ctx)?.map(|[_, _, maxx, _]| maxx))
    })?;
    conn.create_scalar_function("ST_MinY", 1, FLAGS, move |ctx| {
        Ok(envelope_of(ctx)?.map(|[_, miny, _, _]| miny))
    })?;
    conn.create_scalar_function("ST_MaxY", 1, FLAGS, move |ctx| {
        Ok(envelope_of(ctx)?.map(|[_, _, _, maxy]| maxy))
    })?;
    conn.create_scalar_function("ST_IsEmpty", 1, FLAGS, move |ctx| {
        let raw = ctx.get_raw(0);
        let blob = match raw {
            ValueRef::Null => return Ok(None::<i64>),
            other => other.as_blob().map_err(SqliteError::from)?,
        };
        let decoded = gpb::decode(blob).map_err(user_error)?;
        Ok(Some(if decoded.is_empty { 1i64 } else { 0i64 }))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point_blob(x: f64, y: f64) -> Vec<u8> {
        gpb::encode_from_geojson_geometry(
            4326,
            &serde_json::json!({"type": "Point", "coordinates": [x, y]}),
            tellurion_core::RequestedCrs::Omitted,
        )
        .unwrap()
    }

    #[test]
    fn registered_functions_answer_the_envelope_of_a_point() {
        let conn = Connection::open_in_memory().unwrap();
        register(&conn).unwrap();
        conn.execute("CREATE TABLE t (geom BLOB)", []).unwrap();
        conn.execute("INSERT INTO t (geom) VALUES (?1)", [point_blob(10.0, 20.0)])
            .unwrap();

        let (minx, maxx, miny, maxy, is_empty): (f64, f64, f64, f64, i64) = conn
            .query_row(
                "SELECT ST_MinX(geom), ST_MaxX(geom), ST_MinY(geom), ST_MaxY(geom), ST_IsEmpty(geom) FROM t",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        assert_eq!(
            (minx, maxx, miny, maxy, is_empty),
            (10.0, 10.0, 20.0, 20.0, 0)
        );
    }

    #[test]
    fn a_null_geometry_answers_null_everywhere_rather_than_erroring() {
        let conn = Connection::open_in_memory().unwrap();
        register(&conn).unwrap();
        conn.execute("CREATE TABLE t (geom BLOB)", []).unwrap();
        conn.execute("INSERT INTO t (geom) VALUES (NULL)", [])
            .unwrap();

        let minx: Option<f64> = conn
            .query_row("SELECT ST_MinX(geom) FROM t", [], |row| row.get(0))
            .unwrap();
        assert_eq!(minx, None);
        let is_empty: Option<i64> = conn
            .query_row("SELECT ST_IsEmpty(geom) FROM t", [], |row| row.get(0))
            .unwrap();
        assert_eq!(is_empty, None);
    }
}
