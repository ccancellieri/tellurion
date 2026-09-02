//! GeoPackage (SQLite) provisioning DDL for the `geopackage` driver
//! (issue `#73`). The server never creates or alters a `.gpkg` file's
//! schema — this module is the only place a feature table, its GeoPackage
//! spec metadata rows (`gpkg_spatial_ref_sys`/`gpkg_contents`/
//! `gpkg_geometry_columns`), its R*Tree spatial index (Annex L) and
//! maintenance triggers, and its outbox table come from. This DDL-only
//! module opens its own direct `rusqlite` connection here, the same "talk to
//! the backend directly" arrangement `db.rs` already documents for
//! `tokio-postgres` — unlike this crate's `geopackage seed` counterpart
//! (`geopackage_seed.rs`), which does depend on `tellurion-geopackage`,
//! because a *write* has to route through that crate's own transactional
//! outbox+R*Tree machinery to stay consistent; provisioning has no such
//! atomicity obligation, so it stays on the same direct-SQL footing as every
//! other DDL module here. The physical shape below must stay in sync with
//! `tellurion-geopackage`'s own reader/writer SQL by hand, the same
//! arrangement `outbox.rs`/`index.rs` already document for the
//! PostGIS-backed tables.
//!
//! ## The R*Tree maintenance triggers
//!
//! The six triggers below are the GeoPackage spec's own published template
//! (Annex L.1) for keeping an `rtree_<table>_<column>` spatial index table
//! in sync with the feature table it indexes — every conformant GeoPackage
//! writer reproduces this exact trigger shape verbatim; nothing here is
//! specific to this driver. They call `ST_MinX`/`ST_MaxX`/`ST_MinY`/
//! `ST_MaxY`/`ST_IsEmpty`, five small SQL functions this crate never
//! registers itself (registration only matters at write time, and this
//! crate performs no INSERT/UPDATE/DELETE against the feature table it
//! provisions — an empty table never fires a trigger); `tellurion-
//! geopackage::functions` is what registers them on every connection it
//! opens, before its own write path ever runs one of these triggers for
//! real.

use std::path::PathBuf;

use anyhow::Context;

/// Whitelist-validates and double-quotes `name` for use as a SQL
/// identifier — this crate's own small counterpart to
/// `tellurion-geopackage::ident::quote_ident` (a driver crate this one
/// never depends on, see this module's own doc).
///
/// The 63-byte cap is not SQLite's — SQLite has no identifier length limit
/// at all — it is this workspace's, so a GeoPackage table and the PostGIS
/// table of the same collection can carry the same name. The message says
/// so: `#272` found this function enforcing the limit while describing only
/// the character rules, which makes a refused 64-byte name look like a
/// character-set problem the operator cannot find.
fn quote_ident(name: &str) -> anyhow::Result<String> {
    let mut chars = name.chars();
    let first = chars
        .next()
        .filter(|c| c.is_ascii_alphabetic() || *c == '_');
    if first.is_none() || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        anyhow::bail!(
            "'{name}' is not a valid SQLite identifier: only ASCII letters, digits, and '_' are allowed, and it may not start with a digit"
        );
    }
    if name.len() > 63 {
        anyhow::bail!(
            "'{name}' is {} bytes; this crate caps identifiers at 63 so a GeoPackage table can \
             carry the same name as the PostGIS table of the same collection, where 63 is \
             PostgreSQL's own hard limit. Note that the name may be one this command DERIVED from \
             the one you passed (the '_outbox' companion, or an R*Tree index's own name), so the \
             fix is a shorter --table (#272).",
            name.len()
        );
    }
    Ok(format!("\"{name}\""))
}

const ALLOWED_COLUMN_TYPES: [&str; 6] = ["TEXT", "INTEGER", "REAL", "BOOLEAN", "DATE", "DATETIME"];

/// Parses `--columns name:TYPE,name2:TYPE2` CLI input into `(name, TYPE)`
/// pairs, validating each `TYPE` against the small set this driver's write
/// path can bind a scalar JSON value into (`tellurion-geopackage::
/// write_sql`'s own doc: SQLite is dynamically typed, so this is a
/// documentation/affinity hint, not a hard constraint — the whitelist here
/// exists to catch a typo, not to enforce SQLite's own type system, which
/// has none in the way this whitelist implies).
pub fn parse_columns(raw: &[String]) -> anyhow::Result<Vec<(String, String)>> {
    raw.iter()
        .map(|entry| {
            let (name, sql_type) = entry.split_once(':').ok_or_else(|| {
                anyhow::anyhow!("column '{entry}' must be in 'name:TYPE' form")
            })?;
            let sql_type = sql_type.to_ascii_uppercase();
            if !ALLOWED_COLUMN_TYPES.contains(&sql_type.as_str()) {
                anyhow::bail!(
                    "column '{name}': unsupported type '{sql_type}' (expected one of {ALLOWED_COLUMN_TYPES:?})"
                );
            }
            Ok((name.to_string(), sql_type))
        })
        .collect()
}

pub struct CreateTablesArgs {
    pub path: PathBuf,
    pub table: String,
    pub geometry: String,
    pub srid: i32,
    pub geometry_type: String,
    pub columns: Vec<(String, String)>,
    /// Print the DDL without touching the file at all — same escape hatch
    /// `outbox::create_tables`/`index::create_tables` offer.
    pub dry_run: bool,
}

/// The three GeoPackage spec core metadata tables (Requirements 10-13) plus
/// the well-known SRS rows every conformant reader expects to find
/// pre-registered: `-1` (undefined Cartesian), `0` (undefined geographic),
/// `4326` (WGS 84 — served on the tiles lane reprojected to Web Mercator at
/// tile-encode time, `#89`), and `3857` (Web Mercator — this driver's own
/// tiles lane's native CRS, `tellurion-geopackage::driver`'s own
/// `TileSource` doc). All
/// idempotent (`IF NOT EXISTS`/`INSERT OR IGNORE`), safe to run before every
/// table's own provisioning below.
fn core_tables_and_srs_sql() -> String {
    "CREATE TABLE IF NOT EXISTS gpkg_spatial_ref_sys (
    srs_name TEXT NOT NULL,
    srs_id INTEGER NOT NULL PRIMARY KEY,
    organization TEXT NOT NULL,
    organization_coordsys_id INTEGER NOT NULL,
    definition TEXT NOT NULL,
    description TEXT
);
CREATE TABLE IF NOT EXISTS gpkg_contents (
    table_name TEXT NOT NULL PRIMARY KEY,
    data_type TEXT NOT NULL,
    identifier TEXT UNIQUE,
    description TEXT DEFAULT '',
    last_change DATETIME NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    min_x DOUBLE, min_y DOUBLE, max_x DOUBLE, max_y DOUBLE,
    srs_id INTEGER,
    CONSTRAINT fk_gc_r_srs_id FOREIGN KEY (srs_id) REFERENCES gpkg_spatial_ref_sys(srs_id)
);
CREATE TABLE IF NOT EXISTS gpkg_geometry_columns (
    table_name TEXT NOT NULL,
    column_name TEXT NOT NULL,
    geometry_type_name TEXT NOT NULL,
    srs_id INTEGER NOT NULL,
    z TINYINT NOT NULL,
    m TINYINT NOT NULL,
    CONSTRAINT pk_geom_cols PRIMARY KEY (table_name, column_name),
    CONSTRAINT uk_gc_table_name UNIQUE (table_name),
    CONSTRAINT fk_gc_tn FOREIGN KEY (table_name) REFERENCES gpkg_contents(table_name),
    CONSTRAINT fk_gc_srs FOREIGN KEY (srs_id) REFERENCES gpkg_spatial_ref_sys(srs_id)
);
INSERT OR IGNORE INTO gpkg_spatial_ref_sys
    (srs_name, srs_id, organization, organization_coordsys_id, definition, description)
VALUES
    ('Undefined cartesian SRS', -1, 'NONE', -1, 'undefined', 'undefined cartesian coordinate reference system'),
    ('Undefined geographic SRS', 0, 'NONE', 0, 'undefined', 'undefined geographic coordinate reference system'),
    ('WGS 84 geodetic', 4326, 'EPSG', 4326,
     'GEOGCS[\"WGS 84\",DATUM[\"WGS_1984\",SPHEROID[\"WGS 84\",6378137,298.257223563]],PRIMEM[\"Greenwich\",0],UNIT[\"degree\",0.0174532925199433],AUTHORITY[\"EPSG\",\"4326\"]]',
     'longitude/latitude coordinates in decimal degrees on the WGS 84 spheroid'),
    ('WGS 84 / Pseudo-Mercator', 3857, 'EPSG', 3857,
     'PROJCS[\"WGS 84 / Pseudo-Mercator\",GEOGCS[\"WGS 84\",DATUM[\"WGS_1984\",SPHEROID[\"WGS 84\",6378137,298.257223563]],PRIMEM[\"Greenwich\",0],UNIT[\"degree\",0.0174532925199433]],PROJECTION[\"Mercator_1SP\"],PARAMETER[\"central_meridian\",0],PARAMETER[\"scale_factor\",1],PARAMETER[\"false_easting\",0],PARAMETER[\"false_northing\",0],UNIT[\"metre\",1],AXIS[\"X\",EAST],AXIS[\"Y\",NORTH],AUTHORITY[\"EPSG\",\"3857\"]]',
     'Spherical Web Mercator projected CRS used by this workspace''s WebMercatorQuad tile grid');
"
    .to_string()
}

/// The full per-table DDL: the feature table itself, its `gpkg_contents`/
/// `gpkg_geometry_columns` registration rows, the R*Tree spatial index
/// virtual table, its six maintenance triggers (the spec's own published
/// template — see this module's own top-level doc), and the outbox table
/// (`"<table>_outbox"`, the same shape `tellurion-postgis`'s own outbox
/// table uses, adapted to SQLite: `sequence` is `INTEGER PRIMARY KEY
/// AUTOINCREMENT` rather than `bigserial` — `AUTOINCREMENT`, not a plain
/// rowid alias, so a deleted row's sequence number is never reused, the
/// same monotonic-forever guarantee `bigserial` gives PostGIS).
fn create_tables_sql(args: &CreateTablesArgs) -> anyhow::Result<String> {
    let table = quote_ident(&args.table)?;
    let geom = quote_ident(&args.geometry)?;
    let outbox_table = quote_ident(&format!("{}_outbox", args.table))?;
    let rtree_table = quote_ident(&format!("rtree_{}_{}", args.table, args.geometry))?;
    let rtree_bare = format!("rtree_{}_{}", args.table, args.geometry);

    // GeoPackage requirement 27: a feature table's geometry column's own SQL
    // declared type SHOULD match its `gpkg_geometry_columns.geometry_type_name`
    // registration — SQLite's manifest typing never enforces this (it stores
    // a BLOB regardless of the declared type text), but this keeps the file
    // itself conformant for any other GeoPackage reader that inspects it.
    let mut column_defs = vec![
        "\"id\" INTEGER PRIMARY KEY".to_string(),
        format!("{geom} {}", args.geometry_type),
    ];
    for (name, sql_type) in &args.columns {
        column_defs.push(format!("{} {sql_type}", quote_ident(name)?));
    }

    let mut sql = core_tables_and_srs_sql();
    sql.push_str(&format!(
        "CREATE TABLE IF NOT EXISTS {table} (\n    {}\n);\n",
        column_defs.join(",\n    ")
    ));
    sql.push_str(&format!(
        "INSERT OR IGNORE INTO gpkg_contents (table_name, data_type, identifier, srs_id) VALUES ({table_lit}, 'features', {table_lit}, {srid});\n",
        table_lit = quote_sql_string(&args.table),
        srid = args.srid,
    ));
    sql.push_str(&format!(
        "INSERT OR IGNORE INTO gpkg_geometry_columns (table_name, column_name, geometry_type_name, srs_id, z, m) VALUES ({table_lit}, {geom_lit}, {geom_type_lit}, {srid}, 0, 0);\n",
        table_lit = quote_sql_string(&args.table),
        geom_lit = quote_sql_string(&args.geometry),
        geom_type_lit = quote_sql_string(&args.geometry_type),
        srid = args.srid,
    ));
    sql.push_str(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS {rtree_table} USING rtree(id, minx, maxx, miny, maxy);\n"
    ));

    let i = "\"id\"";
    sql.push_str(&format!(
        "CREATE TRIGGER IF NOT EXISTS \"{rtree_bare}_insert\" AFTER INSERT ON {table}
  WHEN (NEW.{geom} NOT NULL AND NOT ST_IsEmpty(NEW.{geom}))
BEGIN
  INSERT OR REPLACE INTO {rtree_table} VALUES (
    NEW.{i}, ST_MinX(NEW.{geom}), ST_MaxX(NEW.{geom}), ST_MinY(NEW.{geom}), ST_MaxY(NEW.{geom})
  );
END;
-- R*Tree virtual tables reject `INSERT OR REPLACE` on an existing id. Drop
-- and recreate this trigger so re-provisioning also repairs files made by
-- earlier versions that used that invalid conflict strategy.
DROP TRIGGER IF EXISTS \"{rtree_bare}_update1\";
CREATE TRIGGER \"{rtree_bare}_update1\" AFTER UPDATE OF {geom} ON {table}
  WHEN (OLD.{i} = NEW.{i} AND (NEW.{geom} NOTNULL AND NOT ST_IsEmpty(NEW.{geom})))
BEGIN
  DELETE FROM {rtree_table} WHERE id = OLD.{i};
  INSERT INTO {rtree_table} VALUES (
    NEW.{i}, ST_MinX(NEW.{geom}), ST_MaxX(NEW.{geom}), ST_MinY(NEW.{geom}), ST_MaxY(NEW.{geom})
  );
END;
CREATE TRIGGER IF NOT EXISTS \"{rtree_bare}_update2\" AFTER UPDATE OF {geom} ON {table}
  WHEN (OLD.{i} = NEW.{i} AND (NEW.{geom} ISNULL OR ST_IsEmpty(NEW.{geom})))
BEGIN
  DELETE FROM {rtree_table} WHERE id = OLD.{i};
END;
CREATE TRIGGER IF NOT EXISTS \"{rtree_bare}_update3\" AFTER UPDATE OF {i} ON {table}
  WHEN OLD.{i} != NEW.{i} AND (NEW.{geom} NOTNULL AND NOT ST_IsEmpty(NEW.{geom}))
BEGIN
  DELETE FROM {rtree_table} WHERE id = OLD.{i};
  INSERT OR REPLACE INTO {rtree_table} VALUES (
    NEW.{i}, ST_MinX(NEW.{geom}), ST_MaxX(NEW.{geom}), ST_MinY(NEW.{geom}), ST_MaxY(NEW.{geom})
  );
END;
CREATE TRIGGER IF NOT EXISTS \"{rtree_bare}_update4\" AFTER UPDATE ON {table}
  WHEN OLD.{i} != NEW.{i} AND (NEW.{geom} ISNULL OR ST_IsEmpty(NEW.{geom}))
BEGIN
  DELETE FROM {rtree_table} WHERE id IN (OLD.{i}, NEW.{i});
END;
CREATE TRIGGER IF NOT EXISTS \"{rtree_bare}_delete\" AFTER DELETE ON {table}
  WHEN old.{geom} NOT NULL
BEGIN
  DELETE FROM {rtree_table} WHERE id = OLD.{i};
END;
"
    ));

    sql.push_str(&format!(
        "CREATE TABLE IF NOT EXISTS {outbox_table} (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    feature_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('upsert', 'delete')),
    payload TEXT,
    committed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    extent_crs84 TEXT
);\n"
    ));

    Ok(sql)
}

/// The one statement that upgrades an outbox table provisioned before
/// `#141`/`#142` existed. `CREATE TABLE IF NOT EXISTS` above does nothing at
/// all to a table that is already there, so a pre-existing file would never
/// gain `extent_crs84` from it — and the server does no DDL of its own, it
/// refuses by name (`tellurion-geopackage`'s `OutboxExtentColumnMissing`) and
/// points back here.
///
/// SQLite has no `ADD COLUMN IF NOT EXISTS` (PostgreSQL does, which is why
/// `outbox.rs`'s equivalent is a single idempotent statement), so this is
/// issued on its own and its "duplicate column name" failure is the expected,
/// ignored outcome on an already-upgraded file — see
/// [`is_duplicate_column_error`]. Printed alongside the rest of the DDL so an
/// operator applying it by hand sees exactly the same statements.
fn outbox_extent_migration_sql(table: &str) -> anyhow::Result<String> {
    let outbox_table = quote_ident(&format!("{table}_outbox"))?;
    Ok(format!(
        "-- Idempotent by way of the error it raises on an already-upgraded file:\n\
         ALTER TABLE {outbox_table} ADD COLUMN extent_crs84 TEXT;\n"
    ))
}

/// Whether `error` is SQLite's own "this column is already here" refusal —
/// the expected outcome of rerunning [`outbox_extent_migration_sql`], and
/// the ONLY error it is ever right to swallow there. Matched on the message
/// text because SQLite reports it as a generic `SQLITE_ERROR`, the same way
/// `tellurion-geopackage::driver::map_outbox_missing` matches "no such
/// table".
fn is_duplicate_column_error(error: &rusqlite::Error) -> bool {
    error.to_string().contains("duplicate column name")
}

/// Single-quotes and escapes `value` for a SQL string literal — free-form
/// text (an identifier or description), never an identifier itself, the
/// same distinction `tellurion-postgis::ident::quote_sql_string` documents.
fn quote_sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Names a `SQLITE_BUSY`/`SQLITE_LOCKED` as the concurrent writer it is, and
/// passes every other failure through untouched (`#272`). Keeping the
/// original error as the source means a genuine schema mistake still reads
/// as one, with SQLite's own message intact.
fn busy_or_raw(error: rusqlite::Error, path: &std::path::Path) -> anyhow::Error {
    match crate::provision::sqlite_contention(&error, path) {
        Some(named) => anyhow::Error::new(error).context(named),
        None => anyhow::Error::new(error),
    }
}

pub async fn create_tables(args: CreateTablesArgs) -> anyhow::Result<()> {
    let sql = create_tables_sql(&args)?;
    let migration = outbox_extent_migration_sql(&args.table)?;
    // Always printed, dry run or not — same requirement `outbox::
    // create_tables`/`index::create_tables` already follow.
    println!("{sql}");
    println!("{migration}");
    if args.dry_run {
        return Ok(());
    }

    // `#272`: no advisory lock, and none needed — SQLite's single-writer
    // transaction already spans the `IF NOT EXISTS` check and the
    // `sqlite_master` insert, which is exactly the atomicity PostgreSQL's
    // own `IF NOT EXISTS` lacks. Measured with this DDL, six concurrent
    // writers over fifteen rounds: **zero rounds ended with a wrong or
    // duplicated catalog**, in both configurations below — the race the
    // PostgreSQL side has does not exist here. What SQLite does not do by
    // default is *wait*: with no busy timeout, 15 of 15 rounds and 73 of 90
    // writers failed outright with "database is locked"; with the five-
    // second timeout `provision::open_geopackage` now sets, 0 of 15 and 0
    // of 90. A wait that still expires is reported as the contention it is
    // rather than as SQLite's own message. See that function's own doc.
    let mut conn = crate::provision::open_geopackage(&args.path)?;
    let transaction = conn
        .transaction()
        .map_err(|error| busy_or_raw(error, &args.path))
        .with_context(|| {
            format!(
                "starting schema transaction for feature table '{}' in '{}'",
                args.table,
                args.path.display()
            )
        })?;
    transaction
        .execute_batch(&sql)
        .map_err(|error| busy_or_raw(error, &args.path))
        .with_context(|| {
            format!(
                "provisioning feature table '{}' in '{}'",
                args.table,
                args.path.display()
            )
        })?;
    // `#141`/`#142`: separate from the batch above precisely because its
    // "already there" outcome is an error rather than a no-op, and swallowing
    // that one error inside `execute_batch` would mean swallowing every other
    // statement's too.
    if let Err(error) = transaction.execute_batch(&migration) {
        if !is_duplicate_column_error(&error) {
            return Err(busy_or_raw(error, &args.path)).with_context(|| {
                format!(
                    "adding the extent_crs84 column to the outbox for '{}' in '{}'",
                    args.table,
                    args.path.display()
                )
            });
        }
    }
    transaction
        .commit()
        .map_err(|error| busy_or_raw(error, &args.path))
        .with_context(|| {
            format!(
                "committing feature table '{}' in '{}'",
                args.table,
                args.path.display()
            )
        })?;
    tracing::info!(
        table = %args.table,
        path = %args.path.display(),
        "created (or confirmed existing) the GeoPackage feature table, its spec metadata, R*Tree index, and outbox table"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;

    fn args() -> CreateTablesArgs {
        CreateTablesArgs {
            path: PathBuf::from("unused-in-these-tests.gpkg"),
            table: "demo".to_string(),
            geometry: "geom".to_string(),
            srid: 4326,
            geometry_type: "POINT".to_string(),
            columns: vec![("name".to_string(), "TEXT".to_string())],
            dry_run: true,
        }
    }

    #[test]
    fn parse_columns_accepts_a_valid_pair() {
        let parsed =
            parse_columns(&["name:text".to_string(), "population:INTEGER".to_string()]).unwrap();
        assert_eq!(
            parsed,
            vec![
                ("name".to_string(), "TEXT".to_string()),
                ("population".to_string(), "INTEGER".to_string()),
            ]
        );
    }

    #[test]
    fn parse_columns_rejects_a_missing_colon() {
        assert!(parse_columns(&["name-text".to_string()]).is_err());
    }

    #[test]
    fn parse_columns_rejects_an_unsupported_type() {
        assert!(parse_columns(&["name:JSON".to_string()]).is_err());
    }

    #[test]
    fn ddl_is_idempotent_and_provisions_every_required_piece() {
        let sql = create_tables_sql(&args()).unwrap();
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS gpkg_spatial_ref_sys"));
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS gpkg_contents"));
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS gpkg_geometry_columns"));
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"demo\""));
        assert!(sql.contains("\"id\" INTEGER PRIMARY KEY"));
        assert!(sql.contains("\"geom\" POINT"));
        assert!(sql.contains("\"name\" TEXT"));
        assert!(sql.contains("CREATE VIRTUAL TABLE IF NOT EXISTS \"rtree_demo_geom\" USING rtree(id, minx, maxx, miny, maxy)"));
        assert!(sql.contains("CREATE TRIGGER IF NOT EXISTS \"rtree_demo_geom_insert\""));
        assert!(sql.contains("DROP TRIGGER IF EXISTS \"rtree_demo_geom_update1\""));
        assert!(sql.contains("CREATE TRIGGER \"rtree_demo_geom_update1\""));
        assert!(sql.contains("DELETE FROM \"rtree_demo_geom\" WHERE id = OLD.\"id\";"));
        assert!(sql.contains("CREATE TRIGGER IF NOT EXISTS \"rtree_demo_geom_update2\""));
        assert!(sql.contains("CREATE TRIGGER IF NOT EXISTS \"rtree_demo_geom_update3\""));
        assert!(sql.contains("CREATE TRIGGER IF NOT EXISTS \"rtree_demo_geom_update4\""));
        assert!(sql.contains("CREATE TRIGGER IF NOT EXISTS \"rtree_demo_geom_delete\""));
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"demo_outbox\""));
        assert!(sql.contains("sequence INTEGER PRIMARY KEY AUTOINCREMENT"));
        assert!(sql.contains("kind TEXT NOT NULL CHECK (kind IN ('upsert', 'delete'))"));
    }

    #[test]
    fn rejects_a_table_name_that_fails_identifier_whitelisting() {
        let mut args = args();
        args.table = "demo; DROP TABLE x; --".to_string();
        assert!(create_tables_sql(&args).is_err());
    }

    #[test]
    fn create_tables_against_a_real_temp_file_is_idempotent_and_matches_the_driver_side_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.gpkg");
        let sql = create_tables_sql(&CreateTablesArgs {
            path: path.clone(),
            dry_run: false,
            ..args()
        })
        .unwrap();

        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&sql)
            .expect("first provisioning run succeeds");
        // Rerun: every statement is `IF NOT EXISTS`/`INSERT OR IGNORE`, so a
        // second pass is a no-op, not an error.
        conn.execute_batch(&sql)
            .expect("rerunning provisioning is idempotent");

        let table_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'demo'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 1);

        let outbox_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'demo_outbox'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(outbox_count, 1);

        let contents_row: (String, i32) = conn
            .query_row(
                "SELECT data_type, srs_id FROM gpkg_contents WHERE table_name = 'demo'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(contents_row, ("features".to_string(), 4326));
    }

    #[tokio::test]
    async fn failed_reprovisioning_rolls_back_the_trigger_migration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollback.gpkg");
        let create_args = CreateTablesArgs {
            path: path.clone(),
            dry_run: false,
            ..args()
        };
        create_tables(create_args)
            .await
            .expect("initial provisioning succeeds");

        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            r#"
DROP TRIGGER rtree_demo_geom_update1;
CREATE TRIGGER rtree_demo_geom_update1 AFTER UPDATE OF "geom" ON "demo"
BEGIN
  INSERT OR REPLACE INTO rtree_demo_geom VALUES (NEW."id", 0, 0, 0, 0);
END;
DROP TABLE demo_outbox;
CREATE INDEX demo_outbox ON demo(id);
"#,
        )
        .expect("installs the legacy trigger and a later DDL conflict");
        let legacy_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = 'rtree_demo_geom_update1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        drop(conn);

        let err = create_tables(CreateTablesArgs {
            path: path.clone(),
            dry_run: false,
            ..args()
        })
        .await
        .expect_err("the conflicting index makes the later outbox DDL fail");
        assert!(
            format!("{err:#}").contains("demo_outbox"),
            "error was: {err:#}"
        );

        let conn = Connection::open(&path).unwrap();
        let preserved_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = 'rtree_demo_geom_update1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preserved_sql, legacy_sql);
    }
}
