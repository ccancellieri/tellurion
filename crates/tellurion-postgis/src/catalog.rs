//! The `CatalogSource` query: `information_schema` + PostGIS's
//! `geometry_columns` view, giving real physical metadata (geometry column,
//! primary key, srid, geometry type) instead of assuming config is correct.
//!
//! Scoped to the `public` schema, matching every other query this driver
//! issues: `sql.rs` never schema-qualifies a table/column identifier, so an
//! unqualified `CollectionDecl.table` only ever resolves against whatever
//! `search_path` puts first — `public` on a stock PostgreSQL install. A
//! table in another schema is invisible to this query the same way it would
//! be to `sql.rs`'s own queries.
//!
//! `key_column_usage` is joined with `ordinal_position = 1` so a composite
//! primary key reports only its first column — consistent with this
//! driver's existing "integer primary keys only" assumption (see the
//! crate-level docs).

pub(crate) const CATALOG_QUERY: &str = "\
SELECT gc.f_table_name AS table_name, \
       gc.f_geometry_column AS geometry_column, \
       gc.srid AS srid, \
       gc.type AS geometry_type, \
       kcu.column_name AS primary_key \
FROM geometry_columns gc \
LEFT JOIN information_schema.table_constraints tc \
       ON tc.table_schema = gc.f_table_schema \
      AND tc.table_name = gc.f_table_name \
      AND tc.constraint_type = 'PRIMARY KEY' \
LEFT JOIN information_schema.key_column_usage kcu \
       ON kcu.constraint_name = tc.constraint_name \
      AND kcu.table_schema = tc.table_schema \
      AND kcu.ordinal_position = 1 \
WHERE gc.f_table_schema = 'public' \
ORDER BY gc.f_table_name, gc.f_geometry_column";

/// `#27`: a cheap, statistics-based extent (`pg_statistic`, populated by
/// `ANALYZE`), transformed to CRS84. Table/geometry-column names are bound as
/// plain text parameters — `ST_EstimatedExtent(text, text)` reads catalog
/// statistics by name, it never touches the table as a SQL identifier, so no
/// `quote_ident` is needed here (unlike `sql.rs`'s query builders).
///
/// The inner `SELECT` always returns exactly one row (it has no `FROM`), so
/// a table with no statistics yet (e.g. never `ANALYZE`d) comes back as one
/// row of four `NULL` columns rather than zero rows — `driver.rs` checks for
/// that and falls back to `sql::build_real_extent_plan`. Some PostGIS
/// versions raise a hard error instead of returning `NULL` in that case;
/// `driver.rs` falls back on either outcome.
pub(crate) const ESTIMATED_EXTENT_SQL: &str = "\
SELECT ST_XMin(t) AS minx, ST_YMin(t) AS miny, ST_XMax(t) AS maxx, ST_YMax(t) AS maxy \
FROM (SELECT ST_Transform(ST_SetSRID(ST_EstimatedExtent($1, $2)::geometry, $3), 4326) AS t) sub";

/// `#19`: a cheap, statistics-based row-count estimate — the same
/// `pg_class.reltuples` approach `tellurion-postgis::sql`'s unfiltered
/// items-count query uses (see its own doc comment for the `GREATEST`
/// rationale), duplicated here rather than shared because that one lives in
/// a dynamic, `SqlParam`-based query builder and this one is a fixed,
/// single-parameter statement `driver.rs` executes directly. `to_regclass`
/// resolves `$1` the same way an unqualified `sql.rs` table reference does
/// (via `search_path`), so this reads the same table `sql.rs`'s own queries
/// would.
pub(crate) const ROW_ESTIMATE_SQL: &str =
    "SELECT GREATEST(reltuples, 0)::bigint AS estimate FROM pg_class WHERE oid = to_regclass($1)";

/// `#19`: every non-geometry column's name and broad type
/// (`information_schema.columns.data_type`, e.g. `"text"`, `"integer"`) for
/// a table that has a geometry column to exclude — the common case, since
/// this driver's `geometry_columns`-sourced `PhysicalCollection` always
/// reports one. See [`ATTRIBUTE_SCHEMA_SQL_NO_GEOMETRY`] for the (defensive,
/// not currently reachable through this driver) geometry-less fallback.
pub(crate) const ATTRIBUTE_SCHEMA_SQL: &str = "\
SELECT column_name, data_type FROM information_schema.columns \
WHERE table_schema = 'public' AND table_name = $1 AND column_name <> $2 \
ORDER BY ordinal_position";

/// [`ATTRIBUTE_SCHEMA_SQL`] without the geometry-column exclusion, for a
/// `PhysicalCollection` that reports none.
pub(crate) const ATTRIBUTE_SCHEMA_SQL_NO_GEOMETRY: &str = "\
SELECT column_name, data_type FROM information_schema.columns \
WHERE table_schema = 'public' AND table_name = $1 \
ORDER BY ordinal_position";

/// `#19`: temporal column detection — every column whose type is a
/// timestamp/timestamptz/date. `driver.rs` treats anything but exactly one
/// returned row as "no derivable datetime column" — deliberately dumb, see
/// `CatalogSource::temporal_column`'s doc comment.
pub(crate) const TEMPORAL_COLUMN_SQL: &str = "\
SELECT column_name FROM information_schema.columns \
WHERE table_schema = 'public' AND table_name = $1 \
  AND data_type IN ('timestamp without time zone', 'timestamp with time zone', 'date')";

/// `#41`: the geometry-type contract check `PostgisBackend::
/// volume_geometry_kind` runs before ever building a volume-tile query — the
/// same `geometry_columns` view [`CATALOG_QUERY`] reads from, scoped to one
/// collection's own table+column so this is a cheap, indexed lookup rather
/// than a second full catalog scan. `coord_dimension` distinguishes an XYZ
/// column (3, the only decodable case here) from XY (2, no Z to extract at
/// all) or XYZM (4, an M ordinate this driver's EWKB reader has no use for).
pub(crate) const VOLUME_GEOMETRY_KIND_SQL: &str = "\
SELECT type, coord_dimension FROM geometry_columns \
WHERE f_table_schema = 'public' AND f_table_name = $1 AND f_geometry_column = $2";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_query_scopes_to_the_public_schema() {
        assert!(CATALOG_QUERY.contains("gc.f_table_schema = 'public'"));
        assert!(CATALOG_QUERY.contains("geometry_columns"));
    }

    #[test]
    fn estimated_extent_sql_uses_estimated_extent_and_transforms_to_crs84() {
        assert!(ESTIMATED_EXTENT_SQL.contains("ST_EstimatedExtent($1, $2)"));
        assert!(ESTIMATED_EXTENT_SQL.contains("ST_Transform"));
        assert!(ESTIMATED_EXTENT_SQL.contains(", 4326)"));
    }

    #[test]
    fn row_estimate_sql_clamps_the_never_analyzed_sentinel() {
        assert!(ROW_ESTIMATE_SQL.contains("GREATEST(reltuples, 0)"));
        assert!(ROW_ESTIMATE_SQL.contains("to_regclass($1)"));
    }

    #[test]
    fn attribute_schema_sql_excludes_the_named_geometry_column() {
        assert!(ATTRIBUTE_SCHEMA_SQL.contains("information_schema.columns"));
        assert!(ATTRIBUTE_SCHEMA_SQL.contains("column_name <> $2"));
        assert!(!ATTRIBUTE_SCHEMA_SQL_NO_GEOMETRY.contains("column_name <>"));
    }

    #[test]
    fn temporal_column_sql_matches_timestamp_and_date_types_only() {
        assert!(TEMPORAL_COLUMN_SQL.contains("timestamp without time zone"));
        assert!(TEMPORAL_COLUMN_SQL.contains("timestamp with time zone"));
        assert!(TEMPORAL_COLUMN_SQL.contains("'date'"));
    }

    #[test]
    fn volume_geometry_kind_sql_scopes_to_the_named_table_and_column() {
        assert!(VOLUME_GEOMETRY_KIND_SQL.contains("geometry_columns"));
        assert!(VOLUME_GEOMETRY_KIND_SQL.contains("f_table_name = $1"));
        assert!(VOLUME_GEOMETRY_KIND_SQL.contains("f_geometry_column = $2"));
        assert!(VOLUME_GEOMETRY_KIND_SQL.contains("coord_dimension"));
    }
}
