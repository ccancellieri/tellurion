//! Introspection against `gpkg_contents`/`gpkg_geometry_columns`/
//! `gpkg_spatial_ref_sys` (the GeoPackage spec's own metadata tables) plus
//! `PRAGMA table_info` for a feature table's own SQLite-declared column
//! shape. Pure `&rusqlite::Connection -> Result<...>` functions — no async,
//! no pooling concern — called from `driver.rs` inside a `pool::with_reader`
//! closure the same way `sql.rs`'s query builders are.

use rusqlite::{Connection, OptionalExtension};

use crate::error::Result;
use crate::ident::quote_ident;

/// One `gpkg_contents`/`gpkg_geometry_columns` entry whose `data_type` is
/// `'features'` — this driver's only collection kind (tiles-only or
/// attributes-only GeoPackage content types are out of this slice's scope).
pub(crate) struct FeatureTableInfo {
    pub(crate) table_name: String,
    pub(crate) geometry_column: String,
    pub(crate) geometry_type: Option<String>,
    pub(crate) srid: Option<i32>,
    /// `None` when the table has no single-column `INTEGER PRIMARY KEY` this
    /// driver's v0.1 keyset paging can use (composite or absent) — mirrors
    /// `tellurion-postgis`'s own "v0.1 assumes an integer primary key"
    /// documented limitation.
    pub(crate) primary_key: Option<String>,
}

/// Every provisioned feature table in this file, joined against its
/// registered geometry column and SRID.
pub(crate) fn list_feature_tables(conn: &Connection) -> Result<Vec<FeatureTableInfo>> {
    let mut stmt = conn.prepare(
        "SELECT c.table_name, g.column_name, g.geometry_type_name, c.srs_id
         FROM gpkg_contents c
         JOIN gpkg_geometry_columns g ON g.table_name = c.table_name
         WHERE c.data_type = 'features'
         ORDER BY c.table_name",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<i32>>(3)?,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (table_name, geometry_column, geometry_type, srid) = row?;
        let primary_key = integer_primary_key(conn, &table_name)?;
        out.push(FeatureTableInfo {
            table_name,
            geometry_column,
            geometry_type,
            srid,
            primary_key,
        });
    }
    Ok(out)
}

/// The sole column `PRAGMA table_info` marks with a non-zero `pk` rank, when
/// there is exactly one — `None` for a composite key (more than one such
/// column) or a table declared with no explicit `INTEGER PRIMARY KEY` at
/// all (every column reports `pk = 0`).
pub(crate) fn integer_primary_key(conn: &Connection, table: &str) -> Result<Option<String>> {
    let ident = quote_ident(table)?;
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({ident})"))?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
    })?;

    let mut candidates = Vec::new();
    for row in rows {
        let (name, pk_rank) = row?;
        if pk_rank != 0 {
            candidates.push(name);
        }
    }
    Ok(match candidates.len() {
        1 => candidates.into_iter().next(),
        _ => None,
    })
}

pub(crate) struct AttributeColumnInfo {
    pub(crate) name: String,
    /// SQLite's own declared column type text (`PRAGMA table_info`'s `type`
    /// column) — a type *affinity* hint, not a hard constraint the way a
    /// PostgreSQL column type is; reported exactly as declared, including an
    /// empty string for an untyped column, rather than guessing one.
    pub(crate) sql_type: String,
}

/// `table`'s non-geometry columns: name plus SQLite's own declared type
/// text. Always answers (`Ok`, possibly empty) — `PRAGMA table_info` has no
/// "no statistics yet" failure mode to decline from, unlike `extent`/
/// `row_estimate` below.
pub(crate) fn attribute_columns(
    conn: &Connection,
    table: &str,
    geometry_column: &str,
) -> Result<Vec<AttributeColumnInfo>> {
    let ident = quote_ident(table)?;
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({ident})"))?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (name, sql_type) = row?;
        if name != geometry_column {
            out.push(AttributeColumnInfo { name, sql_type });
        }
    }
    Ok(out)
}

/// The single column whose declared type (case-insensitively) is `DATE` or
/// `DATETIME` — the GeoPackage spec's own recommended data types for a
/// temporal column (Annex on GeoPackage Data Types) — when there is exactly
/// one; deliberately dumb about anything else, mirroring
/// `tellurion-postgis`'s own "two or more candidates, or zero, both resolve
/// to `None`" temporal-column contract.
pub(crate) fn temporal_column(
    conn: &Connection,
    table: &str,
    geometry_column: &str,
) -> Result<Option<String>> {
    let columns = attribute_columns(conn, table, geometry_column)?;
    let mut candidates = columns.into_iter().filter(|c| {
        let declared = c.sql_type.to_ascii_uppercase();
        declared == "DATE" || declared == "DATETIME"
    });
    let first = candidates.next();
    Ok(match (first, candidates.next()) {
        (Some(only), None) => Some(only.name),
        _ => None,
    })
}

fn table_or_view_exists(conn: &Connection, name: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type IN ('table', 'view') AND name = ?1",
        [name],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// `table`'s spatial extent: `gpkg_contents.min_x/min_y/max_x/max_y` (the
/// spec's own recommended storage location, §1.1.2.1) when present, else a
/// real scan of the R*Tree spatial-index table's own stored bounds — the
/// GeoPackage counterpart of `tellurion-postgis`'s `ST_EstimatedExtent`-then-
/// `ST_Extent` two-tier fallback, except both tiers here are exact (SQLite
/// carries no `pg_statistic`-style estimate to prefer). `Ok(None)` when
/// neither answers: an empty table, or one with no R*Tree index provisioned
/// for it.
/// Four optional bounds, in `(minx, miny, maxx, maxy)` order — the shape
/// both `gpkg_contents`' own stored bounds and a freshly computed R*Tree
/// scan share below.
type OptionalBounds = (Option<f64>, Option<f64>, Option<f64>, Option<f64>);

pub(crate) fn extent(
    conn: &Connection,
    table: &str,
    geometry_column: &str,
) -> Result<Option<[f64; 4]>> {
    let stored: Option<OptionalBounds> = conn
        .query_row(
            "SELECT min_x, min_y, max_x, max_y FROM gpkg_contents WHERE table_name = ?1",
            [table],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    if let Some((Some(minx), Some(miny), Some(maxx), Some(maxy))) = stored {
        return Ok(Some([minx, miny, maxx, maxy]));
    }

    let rtree_table = format!("rtree_{table}_{geometry_column}");
    if !table_or_view_exists(conn, &rtree_table)? {
        return Ok(None);
    }
    let rtree_ident = quote_ident(&rtree_table)?;
    let computed: OptionalBounds = conn.query_row(
        &format!("SELECT MIN(minx), MIN(miny), MAX(maxx), MAX(maxy) FROM {rtree_ident}"),
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    Ok(match computed {
        (Some(minx), Some(miny), Some(maxx), Some(maxy)) => Some([minx, miny, maxx, maxy]),
        _ => None,
    })
}

/// An exact `COUNT(*)` — unlike `tellurion-postgis::catalog::
/// ROW_ESTIMATE_SQL`'s `pg_class.reltuples`, plain SQLite carries no
/// pre-computed table-cardinality statistic to read instead, so this is a
/// real (if typically cheap, for the local, moderate-sized files this driver
/// targets) count rather than an estimate — a deliberate, documented
/// difference from the PostGIS driver's own contract, not an oversight.
pub(crate) fn row_estimate(conn: &Connection, table: &str) -> Result<Option<u64>> {
    let ident = quote_ident(table)?;
    let count: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {ident}"), [], |row| {
        row.get(0)
    })?;
    Ok(u64::try_from(count).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE gpkg_contents (
                table_name TEXT PRIMARY KEY, data_type TEXT, identifier TEXT,
                min_x REAL, min_y REAL, max_x REAL, max_y REAL, srs_id INTEGER
             );
             CREATE TABLE gpkg_geometry_columns (
                table_name TEXT, column_name TEXT, geometry_type_name TEXT,
                srs_id INTEGER, z TINYINT, m TINYINT
             );
             CREATE TABLE demo (id INTEGER PRIMARY KEY, geom BLOB, name TEXT, observed_at DATETIME);
             INSERT INTO gpkg_contents (table_name, data_type, min_x, min_y, max_x, max_y, srs_id)
                VALUES ('demo', 'features', -4.0, 46.0, 4.0, 54.0, 4326);
             INSERT INTO gpkg_geometry_columns (table_name, column_name, geometry_type_name, srs_id, z, m)
                VALUES ('demo', 'geom', 'POINT', 4326, 0, 0);
             INSERT INTO demo (id, geom, name, observed_at) VALUES (1, X'00', 'alpha', '2024-01-01T00:00:00Z');
             INSERT INTO demo (id, geom, name, observed_at) VALUES (2, X'00', 'bravo', '2024-02-01T00:00:00Z');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn list_feature_tables_reports_the_joined_metadata() {
        let conn = fixture();
        let tables = list_feature_tables(&conn).unwrap();
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].table_name, "demo");
        assert_eq!(tables[0].geometry_column, "geom");
        assert_eq!(tables[0].geometry_type.as_deref(), Some("POINT"));
        assert_eq!(tables[0].srid, Some(4326));
        assert_eq!(tables[0].primary_key.as_deref(), Some("id"));
    }

    #[test]
    fn attribute_columns_excludes_only_the_geometry_column() {
        let conn = fixture();
        let columns = attribute_columns(&conn, "demo", "geom").unwrap();
        let names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"id"));
        assert!(names.contains(&"name"));
        assert!(names.contains(&"observed_at"));
        assert!(!names.contains(&"geom"));
    }

    #[test]
    fn temporal_column_finds_the_sole_datetime_column() {
        let conn = fixture();
        assert_eq!(
            temporal_column(&conn, "demo", "geom").unwrap().as_deref(),
            Some("observed_at")
        );
    }

    #[test]
    fn temporal_column_is_none_with_zero_candidates() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, geom BLOB, name TEXT)")
            .unwrap();
        assert_eq!(temporal_column(&conn, "t", "geom").unwrap(), None);
    }

    #[test]
    fn extent_reads_the_stored_gpkg_contents_bounds() {
        let conn = fixture();
        assert_eq!(
            extent(&conn, "demo", "geom").unwrap(),
            Some([-4.0, 46.0, 4.0, 54.0])
        );
    }

    #[test]
    fn extent_is_none_with_no_stored_bounds_and_no_rtree_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE gpkg_contents (table_name TEXT PRIMARY KEY, min_x REAL, min_y REAL, max_x REAL, max_y REAL);
             INSERT INTO gpkg_contents (table_name) VALUES ('t');",
        )
        .unwrap();
        assert_eq!(extent(&conn, "t", "geom").unwrap(), None);
    }

    #[test]
    fn row_estimate_is_an_exact_count() {
        let conn = fixture();
        assert_eq!(row_estimate(&conn, "demo").unwrap(), Some(2));
    }

    #[test]
    fn integer_primary_key_is_none_for_a_table_with_no_declared_pk() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (a TEXT, b TEXT)")
            .unwrap();
        assert_eq!(integer_primary_key(&conn, "t").unwrap(), None);
    }
}
