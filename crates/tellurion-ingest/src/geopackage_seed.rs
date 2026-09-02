//! `geopackage seed`: populates a `.gpkg` feature table already provisioned
//! by `geopackage create-tables` (`geopackage.rs`) with the same
//! deterministic synthetic grid the top-level `seed` subcommand writes into
//! PostGIS (`seed.rs`) — same row count, same names, same timestamps, same
//! point/polygon alternation (`synthetic::grid`), adapted only where the
//! backend honestly differs:
//!
//! - **No DDL.** `seed` owns its own table (`DROP`/`CREATE` on every run);
//!   this command never runs DDL at all — the server-wide rule this
//!   workspace holds everywhere else. The target table, its geometry
//!   column, and its SRID all come from the GeoPackage metadata
//!   `create-tables` already wrote, discovered here through
//!   `tellurion_core::CatalogSource` rather than re-asked for on the command
//!   line.
//! - **Schema-adaptive properties, not a fixed shape.** `seed`'s Postgres
//!   table always has `name`/`observed_at` because `seed` created it.
//!   `create-tables` lets an operator provision *any* column set, so this
//!   command writes `name` (required — refused by name if the table has
//!   none) and `observed_at` (written only if the table declares one; a
//!   demo table that skipped it, like this crate's own README quickstart,
//!   just gets rows with no `observed_at` property).
//! - **Points only when the table says points only.** A table provisioned
//!   with `--geometry-type POINT` gets an all-point grid; anything else
//!   (`GEOMETRY`, `POLYGON`, ...) gets the same point/polygon alternation
//!   `seed` writes, since this driver never enforces the declared geometry
//!   type against what a row actually stores (`tellurion-ingest::
//!   geopackage`'s own doc).
//! - **Coordinates scaled to the table's own SRID**, not always geographic
//!   degrees: `seed`'s Postgres demo is always SRID 4326, but a `.gpkg`
//!   table's own tiles-lane-native SRID is 3857 — see `coordinate_extent`
//!   (the tiles lane also serves a 4326-declared table, reprojected).
//!
//! ## Writing through the driver crate, not around it
//!
//! Every row goes through `tellurion_core::WriteSink::apply` on a real
//! `geopackage` driver instance (built via `DriverFactory::build`, exactly
//! as the server itself builds one at boot) — the data mutation and its
//! outbox obligation commit in the one SQLite transaction that machinery
//! guarantees, keeping the R*Tree index consistent with the data the same
//! way a real PUT request would. There is no raw-SQL shortcut here: this
//! module holds no `rusqlite` dependency of its own.
//!
//! `DriverFactory::build` also gives this command its unprovisioned-file
//! refusal for free: `StorageDecl.url_env`-driven construction runs the same
//! `ConnectionPool::open` check every driver build does, so an unprovisioned
//! or non-GeoPackage path fails with that check's own named error before
//! this module writes anything.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Context;
use tellurion_core::{
    CollectionDecl, DriverFactory, Mutation, MutationKind, PhysicalCollection, RoutingDecl,
    SearchConf, SettingsDecl, StorageDecl, StorageDriver, StyleConf, TilesConf, VisibilityDecl,
    WriteSink, ZoomCaps,
};
use tellurion_geopackage::GeopackageDriverFactory;

use crate::synthetic;

/// Half the Web Mercator world extent in meters (EPSG:3857) — duplicated
/// from `tellurion-geopackage::driver`'s own private constant of the same
/// value rather than imported, the same "one well-known figure, driver
/// crates don't share it across the boundary" call that crate's own doc
/// makes for its copy of the identical constant.
const WEB_MERCATOR_ORIGIN: f64 = 20_037_508.342_789_244;

pub struct SeedArgs {
    pub path: PathBuf,
    pub table: String,
    pub catalog: String,
    pub storage: String,
}

pub async fn run(args: SeedArgs) -> anyhow::Result<()> {
    let driver = open_driver(&args.path)?;
    let physical = find_feature_table(&driver, &args.table, &args.path).await?;

    let geometry_column = physical.geometry_column.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "table '{}' in '{}' has no registered geometry column",
            args.table,
            args.path.display()
        )
    })?;
    let pk = physical.primary_key.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "table '{}' in '{}' has no single-column INTEGER PRIMARY KEY this driver's v0.1 write path can address",
            args.table,
            args.path.display()
        )
    })?;
    let srid = physical.srid.unwrap_or(4326);

    let attribute_columns = driver
        .catalog_source()
        .attribute_schema(&physical)
        .await?
        .unwrap_or_default();
    let has_column = |name: &str| attribute_columns.iter().any(|c| c.name == name);
    if !has_column("name") {
        anyhow::bail!(
            "table '{}' in '{}' has no 'name' TEXT column; provision one with \
             `tellurion-ingest geopackage create-tables --table {} --columns name:TEXT[,observed_at:DATETIME]` \
             before seeding",
            args.table,
            args.path.display(),
            args.table,
        );
    }
    let include_observed_at = has_column("observed_at");
    let include_polygons = !physical
        .geometry_type
        .as_deref()
        .unwrap_or("")
        .eq_ignore_ascii_case("POINT");

    let write_sink = driver.write_sink().ok_or_else(|| {
        anyhow::anyhow!(
            "geopackage storage at '{}' does not advertise a write sink",
            args.path.display()
        )
    })?;

    let collection = seeded_collection_decl(
        &args.table,
        &args.catalog,
        &args.storage,
        &args.table,
        &geometry_column,
        &pk,
        srid,
        include_observed_at,
    );

    let inserted = seed_features(
        write_sink.as_ref(),
        &collection,
        include_polygons,
        include_observed_at,
        srid,
    )
    .await?;
    tracing::info!(
        count = inserted,
        table = %args.table,
        path = %args.path.display(),
        "seeded demo features into geopackage"
    );

    println!(
        "{}",
        crate::yaml_snippet::render_collection_snippet(collection)?
    );
    Ok(())
}

/// Bridges this command's own `--path` flag into `DriverFactory::build`'s
/// env-var-indirection contract (`StorageDecl.url_env`, the same one the
/// server reads at boot): sets a process-local environment variable to
/// `path` and builds the `geopackage` driver against it, which also runs
/// that driver's own "is this a provisioned GeoPackage" check
/// (`ConnectionPool::open`) — reused here rather than reimplemented, per
/// this module's own top-level doc.
///
/// The variable name is unique to this call, not a fixed literal:
/// `set_var` and `build`'s own `env::var` read are two separate statements
/// operating on whole-process state, and this crate's own tests call `run`
/// (and so this function) from several `#[tokio::test]`s that the harness
/// runs concurrently on different threads. A fixed name let one call's
/// `set_var` land, on an unlucky thread interleaving, between another
/// concurrent call's `set_var` and its `build`-internal read — handing that
/// call a different `.gpkg` path than the one it just set. A per-call
/// counter makes that interleaving impossible rather than merely rare,
/// which matters just as much for two callers in the same real process as
/// it does for two concurrent tests.
fn open_driver(path: &Path) -> anyhow::Result<Arc<dyn StorageDriver>> {
    static NEXT_CALL_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let call_id = NEXT_CALL_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path_env = format!("TELLURION_INGEST_GEOPACKAGE_SEED_PATH_{call_id}");
    std::env::set_var(&path_env, path);
    let decl = StorageDecl {
        id: "geopackage-seed".to_string(),
        driver: "geopackage".to_string(),
        url_env: path_env,
        pool_size: None,
    };
    GeopackageDriverFactory::new()
        .build(&decl)
        .with_context(|| format!("opening geopackage storage at '{}'", path.display()))
}

async fn find_feature_table(
    driver: &Arc<dyn StorageDriver>,
    table: &str,
    path: &Path,
) -> anyhow::Result<PhysicalCollection> {
    let collections: Vec<PhysicalCollection> = driver.catalog_source().collections().await?;
    collections
        .into_iter()
        .find(|c| c.name == table)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "table '{table}' is not a provisioned feature table in '{}'; run \
                 `tellurion-ingest geopackage create-tables --path {} --table {table} ...` first",
                path.display(),
                path.display(),
            )
        })
}

/// `(minx, miny, maxx, maxy)` a synthetic grid should span for `srid` — full
/// geographic extent for 4326 (matching `seed.rs`'s own Postgres demo
/// exactly), the full Web Mercator world square for 3857 (the SRID this
/// crate's own tiles-serving quickstart provisions), and the same
/// geographic default for anything else — an honest simplification, not a
/// claim of correctness for an arbitrary CRS.
fn coordinate_extent(srid: i32) -> (f64, f64, f64, f64) {
    if srid == 3857 {
        (
            -WEB_MERCATOR_ORIGIN,
            -WEB_MERCATOR_ORIGIN,
            WEB_MERCATOR_ORIGIN,
            WEB_MERCATOR_ORIGIN,
        )
    } else {
        (-180.0, -80.0, 180.0, 80.0)
    }
}

fn square_geojson(center_x: f64, center_y: f64, half_x: f64, half_y: f64) -> serde_json::Value {
    let (w, e) = (center_x - half_x, center_x + half_x);
    let (s, n) = (center_y - half_y, center_y + half_y);
    serde_json::json!({
        "type": "Polygon",
        "coordinates": [[[w, s], [e, s], [e, n], [w, n], [w, s]]]
    })
}

/// Writes `synthetic::grid()` through `write_sink`, one `WriteSink::apply`
/// call per row (pk `1..=500`, matching the grid's own row count) — see this
/// module's own top-level doc for why every row commits its data and its
/// outbox obligation atomically rather than in a separate step.
async fn seed_features(
    write_sink: &dyn WriteSink,
    collection: &CollectionDecl,
    include_polygons: bool,
    include_observed_at: bool,
    srid: i32,
) -> anyhow::Result<usize> {
    let (minx, miny, maxx, maxy) = coordinate_extent(srid);
    // A fraction of one grid cell, so a polygon cell's square never
    // overlaps its neighbors regardless of which coordinate space it's
    // sized in.
    let half_x = (maxx - minx) / synthetic::LON_STEPS as f64 * 0.3;
    let half_y = (maxy - miny) / synthetic::LAT_STEPS as f64 * 0.3;

    let mut total = 0usize;
    for (idx, feature) in synthetic::grid().into_iter().enumerate() {
        let x = minx + feature.u * (maxx - minx);
        let y = miny + feature.v * (maxy - miny);

        let geometry = if include_polygons && feature.is_polygon {
            square_geojson(x, y, half_x, half_y)
        } else {
            serde_json::json!({"type": "Point", "coordinates": [x, y]})
        };

        let mut properties = serde_json::Map::new();
        properties.insert("name".to_string(), serde_json::Value::String(feature.name));
        if include_observed_at {
            properties.insert(
                "observed_at".to_string(),
                serde_json::Value::String(format_utc(feature.observed_at)),
            );
        }

        let feature_id = (idx + 1).to_string();
        let mutation = Mutation {
            feature_id: feature_id.clone(),
            kind: MutationKind::Upsert(serde_json::json!({
                "type": "Feature",
                "geometry": geometry,
                "properties": properties,
            })),
        };
        write_sink
            .apply(collection, mutation)
            .await
            .with_context(|| format!("seeding row {feature_id}"))?;
        total += 1;
    }
    Ok(total)
}

#[allow(clippy::too_many_arguments)]
fn seeded_collection_decl(
    id: &str,
    catalog: &str,
    storage: &str,
    table: &str,
    geometry: &str,
    pk: &str,
    srid: i32,
    has_observed_at: bool,
) -> CollectionDecl {
    let mut caps = std::collections::BTreeMap::new();
    caps.insert(0u8, 2000u64);
    caps.insert(10u8, 20000u64);

    CollectionDecl {
        id: id.to_string(),
        kind: tellurion_core::CollectionKind::Vector,
        external_id: None,
        catalog: catalog.to_string(),
        storage: storage.to_string(),
        routing: RoutingDecl::default(),
        table: Some(table.to_string()),
        geometry: Some(geometry.to_string()),
        pk: Some(pk.to_string()),
        id_type: tellurion_core::IdType::default(),
        datetime: has_observed_at.then(|| "observed_at".to_string()),
        modified_column: None,
        row_estimate: None,
        srid: Some(srid),
        projection: None,
        geometry_profile: None,
        tiles: TilesConf {
            minzoom: 0,
            maxzoom: 14,
            caps: ZoomCaps(caps),
        },
        geometry_variants: Vec::new(),
        style: StyleConf::default(),
        places3d: None,
        schema: None,
        search: SearchConf::default(),
        tile_invalidation: false,
        settings: SettingsDecl::default(),
        attribute_columns: None,
        tile_properties: Vec::new(),
        visibility: VisibilityDecl::default(),
        object_store: None,
        stac_metadata: false,
        stac_item_assets: false,
    }
}

/// Formats a `SystemTime` at or after the Unix epoch as `YYYY-MM-DDTHH:MM:SSZ`
/// (UTC, whole seconds) — a tiny, dependency-free ISO 8601 formatter (no
/// date/time crate lives anywhere else in this workspace either), good
/// enough for a synthetic demo timestamp with no sub-second component to
/// represent. Civil-date math per Howard Hinnant's well-known
/// `civil_from_days` algorithm (a standard, public-domain integer
/// day-count-to-calendar-date conversion).
fn format_utc(time: SystemTime) -> String {
    let secs = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let time_of_day = secs.rem_euclid(86_400);
    let (hour, minute, second) = (
        time_of_day / 3600,
        (time_of_day / 60) % 60,
        time_of_day % 60,
    );
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// `z` = days since `1970-01-01` -> `(year, month, day)`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tellurion_core::{ItemsQuery, MutationKind as CoreMutationKind, Sequence};

    fn temp_gpkg_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "tellurion-ingest-geopackage-seed-test-{}-{name}.gpkg",
            std::process::id(),
        ));
        path
    }

    fn cleanup(path: &Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    /// Opens a fresh driver instance for reading back what a test just
    /// wrote. Delegates to the real `open_driver` rather than repeating its
    /// env-var-indirection dance with a second, test-local literal name:
    /// two call sites picking their own distinct literal names happened to
    /// be safe, but only by convention — a future test copying the pattern
    /// with a name already in use elsewhere would silently reintroduce the
    /// exact race `open_driver`'s own doc describes. `open_driver`'s
    /// per-call counter removes that whole class of mistake.
    async fn open_reader(path: &Path) -> Arc<dyn StorageDriver> {
        open_driver(path).expect("driver builds against a provisioned file")
    }

    fn provision_args(
        path: PathBuf,
        columns: Vec<(String, String)>,
    ) -> crate::geopackage::CreateTablesArgs {
        crate::geopackage::CreateTablesArgs {
            path,
            table: "demo".to_string(),
            geometry: "geom".to_string(),
            srid: 4326,
            geometry_type: "GEOMETRY".to_string(),
            columns,
            dry_run: false,
        }
    }

    #[test]
    fn format_utc_renders_the_epoch() {
        assert_eq!(format_utc(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn format_utc_renders_a_later_instant() {
        // 1970-01-21T19:00:00Z, the last row's timestamp in the 500-row grid
        // (499 * 3600 seconds after the epoch).
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(499 * 3600);
        assert_eq!(format_utc(time), "1970-01-21T19:00:00Z");
    }

    #[test]
    fn coordinate_extent_uses_the_web_mercator_world_square_for_3857() {
        let (minx, miny, maxx, maxy) = coordinate_extent(3857);
        assert_eq!(minx, -WEB_MERCATOR_ORIGIN);
        assert_eq!(miny, -WEB_MERCATOR_ORIGIN);
        assert_eq!(maxx, WEB_MERCATOR_ORIGIN);
        assert_eq!(maxy, WEB_MERCATOR_ORIGIN);
    }

    #[test]
    fn coordinate_extent_defaults_to_geographic_degrees() {
        assert_eq!(coordinate_extent(4326), (-180.0, -80.0, 180.0, 80.0));
        assert_eq!(coordinate_extent(2154), (-180.0, -80.0, 180.0, 80.0));
    }

    #[test]
    fn square_geojson_is_a_closed_ring_around_its_center() {
        let polygon = square_geojson(10.0, 20.0, 1.0, 2.0);
        assert_eq!(polygon["type"], "Polygon");
        let ring = polygon["coordinates"][0].as_array().unwrap();
        assert_eq!(ring.len(), 5);
        assert_eq!(ring.first(), ring.last());
    }

    /// End-to-end: provisions a temp `.gpkg` with a `name`+`observed_at`
    /// schema, seeds it via the actual `run` code path, then reads the
    /// result back through a fresh driver instance — proving the row count,
    /// the R*Tree spatial index (a bbox-filtered read returns a strict,
    /// non-empty subset rather than everything or nothing), and outbox
    /// consistency (one obligation per written row, high-water mark matches).
    #[tokio::test]
    async fn seeds_the_grid_and_maintains_the_rtree_and_outbox() {
        let path = temp_gpkg_path("ok");
        cleanup(&path);

        crate::geopackage::create_tables(provision_args(
            path.clone(),
            vec![
                ("name".to_string(), "TEXT".to_string()),
                ("observed_at".to_string(), "DATETIME".to_string()),
            ],
        ))
        .await
        .expect("provisioning succeeds");

        run(SeedArgs {
            path: path.clone(),
            table: "demo".to_string(),
            catalog: "default".to_string(),
            storage: "main".to_string(),
        })
        .await
        .expect("seeding succeeds");

        let driver = open_reader(&path).await;
        let features = driver.feature_source().expect("advertises reads");
        let collection =
            seeded_collection_decl("demo", "default", "main", "demo", "geom", "id", 4326, true);

        let all = features
            .items(
                &collection,
                &ItemsQuery {
                    limit: 1000,
                    ..Default::default()
                },
            )
            .await
            .expect("reads every row back");
        assert_eq!(all.features_geojson.len(), synthetic::ROW_COUNT);

        // One quadrant of the globe: a strict, non-empty subset proves the
        // R*Tree pushdown actually narrowed the read rather than either
        // scanning everything or finding nothing.
        let quadrant = features
            .items(
                &collection,
                &ItemsQuery {
                    limit: 1000,
                    bbox: Some([-180.0, -80.0, 0.0, 0.0]),
                    ..Default::default()
                },
            )
            .await
            .expect("bbox-filtered read succeeds");
        assert!(!quadrant.features_geojson.is_empty());
        assert!(quadrant.features_geojson.len() < all.features_geojson.len());

        let outbox = driver.outbox_source().expect("advertises the outbox");
        let high_water = outbox
            .primary_high_water(&collection)
            .await
            .expect("reads the outbox high-water mark");
        assert_eq!(high_water, Sequence(synthetic::ROW_COUNT as u64));

        let obligations = outbox
            .read_after(&collection, Sequence(0), 1000)
            .await
            .expect("drains the outbox");
        assert_eq!(obligations.len(), synthetic::ROW_COUNT);
        assert!(obligations
            .iter()
            .all(|o| matches!(o.kind, CoreMutationKind::Upsert(_))));

        cleanup(&path);
    }

    #[tokio::test]
    async fn reseeding_upserts_the_grid_without_duplicate_features_or_rtree_rows() {
        let path = temp_gpkg_path("reseed");
        cleanup(&path);

        crate::geopackage::create_tables(provision_args(
            path.clone(),
            vec![("name".to_string(), "TEXT".to_string())],
        ))
        .await
        .expect("provisioning succeeds");

        let args = SeedArgs {
            path: path.clone(),
            table: "demo".to_string(),
            catalog: "default".to_string(),
            storage: "main".to_string(),
        };
        run(args).await.expect("initial seed succeeds");
        run(SeedArgs {
            path: path.clone(),
            table: "demo".to_string(),
            catalog: "default".to_string(),
            storage: "main".to_string(),
        })
        .await
        .expect("repeat seed succeeds");

        let conn = Connection::open(&path).expect("opens the seeded GeoPackage");
        let feature_count: i64 = conn
            .query_row("SELECT count(*) FROM demo", [], |row| row.get(0))
            .expect("counts seeded features");
        let rtree_count: i64 = conn
            .query_row("SELECT count(*) FROM rtree_demo_geom", [], |row| row.get(0))
            .expect("counts R*Tree entries");
        assert_eq!(feature_count, synthetic::ROW_COUNT as i64);
        assert_eq!(rtree_count, feature_count);

        cleanup(&path);
    }

    #[tokio::test]
    async fn reprovisioning_replaces_the_legacy_update_trigger_before_reseeding() {
        let path = temp_gpkg_path("legacy-trigger-migration");
        cleanup(&path);
        let create_args =
            || provision_args(path.clone(), vec![("name".to_string(), "TEXT".to_string())]);

        crate::geopackage::create_tables(create_args())
            .await
            .expect("initial provisioning succeeds");
        run(SeedArgs {
            path: path.clone(),
            table: "demo".to_string(),
            catalog: "default".to_string(),
            storage: "main".to_string(),
        })
        .await
        .expect("initial seed succeeds");

        let conn = Connection::open(&path).expect("opens the populated GeoPackage");
        conn.execute_batch(
            r#"
DROP TRIGGER rtree_demo_geom_update1;
CREATE TRIGGER rtree_demo_geom_update1 AFTER UPDATE OF "geom" ON "demo"
  WHEN (OLD."id" = NEW."id" AND (NEW."geom" NOTNULL AND NOT ST_IsEmpty(NEW."geom")))
BEGIN
  INSERT OR REPLACE INTO rtree_demo_geom VALUES (
    NEW."id", ST_MinX(NEW."geom"), ST_MaxX(NEW."geom"), ST_MinY(NEW."geom"), ST_MaxY(NEW."geom")
  );
END;
"#,
        )
        .expect("installs the legacy update trigger");
        drop(conn);

        crate::geopackage::create_tables(create_args())
            .await
            .expect("reprovisioning migrates the trigger");
        let conn = Connection::open(&path).expect("opens the migrated GeoPackage");
        let migrated_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = 'rtree_demo_geom_update1'",
                [],
                |row| row.get(0),
            )
            .expect("reads the migrated trigger");
        assert!(migrated_sql.contains("DELETE FROM \"rtree_demo_geom\""));
        assert!(!migrated_sql.contains("INSERT OR REPLACE"));
        drop(conn);

        run(SeedArgs {
            path: path.clone(),
            table: "demo".to_string(),
            catalog: "default".to_string(),
            storage: "main".to_string(),
        })
        .await
        .expect("reseed succeeds after trigger migration");

        let conn = Connection::open(&path).expect("opens the reseeded GeoPackage");
        let feature_count: i64 = conn
            .query_row("SELECT count(*) FROM demo", [], |row| row.get(0))
            .expect("counts seeded features");
        let rtree_count: i64 = conn
            .query_row("SELECT count(*) FROM rtree_demo_geom", [], |row| row.get(0))
            .expect("counts R*Tree entries");
        assert_eq!(feature_count, synthetic::ROW_COUNT as i64);
        assert_eq!(rtree_count, feature_count);
        drop(conn);
        cleanup(&path);
    }

    #[tokio::test]
    async fn seeds_points_only_when_the_table_is_provisioned_point_only() {
        let path = temp_gpkg_path("points-only");
        cleanup(&path);

        crate::geopackage::create_tables(crate::geopackage::CreateTablesArgs {
            path: path.clone(),
            table: "demo".to_string(),
            geometry: "geom".to_string(),
            srid: 4326,
            geometry_type: "POINT".to_string(),
            columns: vec![("name".to_string(), "TEXT".to_string())],
            dry_run: false,
        })
        .await
        .expect("provisioning succeeds");

        run(SeedArgs {
            path: path.clone(),
            table: "demo".to_string(),
            catalog: "default".to_string(),
            storage: "main".to_string(),
        })
        .await
        .expect("seeding succeeds");

        let driver = open_reader(&path).await;
        let features = driver.feature_source().expect("advertises reads");
        let collection =
            seeded_collection_decl("demo", "default", "main", "demo", "geom", "id", 4326, false);
        let all = features
            .items(
                &collection,
                &ItemsQuery {
                    limit: 1000,
                    ..Default::default()
                },
            )
            .await
            .expect("reads every row back");
        assert_eq!(all.features_geojson.len(), synthetic::ROW_COUNT);
        assert!(all
            .features_geojson
            .iter()
            .all(|f| f["geometry"]["type"] == "Point"));

        cleanup(&path);
    }

    #[tokio::test]
    async fn refuses_an_unprovisioned_file_by_name() {
        // A path nobody ran `create-tables` against — no file at all, which
        // `ConnectionPool::open` refuses by name before this command reads
        // or writes anything.
        let path = temp_gpkg_path("unprovisioned");
        cleanup(&path);

        let err = run(SeedArgs {
            path: path.clone(),
            table: "demo".to_string(),
            catalog: "default".to_string(),
            storage: "main".to_string(),
        })
        .await
        .unwrap_err();
        // The top-level `anyhow::Context` message wraps the driver crate's
        // own named refusal (`GeopackageError::NotAGeoPackage`, reused via
        // `ConnectionPool::open` — see `open_driver`'s own doc); the full
        // chain, not just the top frame, is where that reused message lives.
        assert!(
            format!("{err:#}").contains("not a provisioned GeoPackage"),
            "error was: {err:#}"
        );

        cleanup(&path);
    }

    #[tokio::test]
    async fn refuses_a_missing_table_by_name() {
        let path = temp_gpkg_path("missing-table");
        cleanup(&path);
        crate::geopackage::create_tables(provision_args(
            path.clone(),
            vec![("name".to_string(), "TEXT".to_string())],
        ))
        .await
        .expect("provisioning succeeds");

        let err = run(SeedArgs {
            path: path.clone(),
            table: "does-not-exist".to_string(),
            catalog: "default".to_string(),
            storage: "main".to_string(),
        })
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("is not a provisioned feature table"),
            "error was: {err}"
        );

        cleanup(&path);
    }

    #[tokio::test]
    async fn refuses_a_table_missing_the_name_column() {
        let path = temp_gpkg_path("no-name-column");
        cleanup(&path);
        crate::geopackage::create_tables(provision_args(path.clone(), vec![]))
            .await
            .expect("provisioning succeeds");

        let err = run(SeedArgs {
            path: path.clone(),
            table: "demo".to_string(),
            catalog: "default".to_string(),
            storage: "main".to_string(),
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("'name'"), "error was: {err}");

        cleanup(&path);
    }

    /// Regression guard for the env-var race `open_driver`'s own doc
    /// describes: eight real OS threads each build a driver against their
    /// own uniquely-tabled `.gpkg` file concurrently and check that the
    /// table they read back is their own, not another worker's. Before the
    /// per-call counter fix (a fixed literal env-var name, `set_var`
    /// immediately followed by `build`), an equivalent concurrent stress —
    /// with the race window artificially widened to make it deterministic
    /// rather than dependent on host contention — corrupted 7 of 8 opens,
    /// every one of them observing the last writer's table instead of its
    /// own. This exercises the real, fixed code path with no artificial
    /// widening: it passes because the counter makes the interleaving
    /// impossible, not just unlikely.
    #[test]
    fn open_driver_survives_concurrent_calls_with_no_cross_contamination() {
        const WORKERS: usize = 8;
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut paths = Vec::new();
        for i in 0..WORKERS {
            let path = temp_gpkg_path(&format!("concurrent-open-{i}"));
            cleanup(&path);
            rt.block_on(crate::geopackage::create_tables(
                crate::geopackage::CreateTablesArgs {
                    path: path.clone(),
                    table: format!("t{i}"),
                    geometry: "geom".to_string(),
                    srid: 4326,
                    geometry_type: "GEOMETRY".to_string(),
                    columns: vec![],
                    dry_run: false,
                },
            ))
            .expect("provisioning succeeds");
            paths.push(path);
        }

        let handles: Vec<_> = paths
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, path)| {
                std::thread::spawn(move || {
                    let driver =
                        open_driver(&path).expect("driver builds against a provisioned file");
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    let collections: Vec<PhysicalCollection> = rt
                        .block_on(driver.catalog_source().collections())
                        .expect("lists collections");
                    let name = collections
                        .into_iter()
                        .next()
                        .expect("exactly one table")
                        .name;
                    (i, name)
                })
            })
            .collect();

        let mut mismatches = Vec::new();
        for handle in handles {
            let (i, observed) = handle.join().unwrap();
            let expected = format!("t{i}");
            if observed != expected {
                mismatches.push(format!(
                    "worker {i} expected {expected}, observed {observed}"
                ));
            }
        }

        for path in &paths {
            cleanup(path);
        }

        assert!(
            mismatches.is_empty(),
            "open_driver should never hand a caller a different path than the one \
             it opened: {mismatches:?}"
        );
    }
}
