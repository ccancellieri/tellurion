//! Hermetic, driver-level contract tests: a real temp-file `.gpkg` fixture,
//! provisioned by this file's own DDL (the crate's public surface is the
//! `DriverFactory`/`StorageDriver` traits, not a provisioning API — see
//! `tellurion-ingest::geopackage`'s own doc for why provisioning lives
//! there, not here), driven entirely through
//! `tellurion_core::{DriverFactory, StorageDriver}` — no `Router`, no HTTP,
//! no server process. Covers catalog introspection, bbox and CQL2 filtered
//! reads, refusal cases, write+outbox atomicity, paging stability, MVT
//! output sanity, and unprovisioned-file refusal, per the crate's own
//! testing obligations.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use rusqlite::Connection;
use tellurion_core::{
    AppConfig, AppContext, BatchItemOutcome, CollectionDecl, DriverFactory, Error as CoreError,
    FileStyleStore, ItemsQuery, MokaTileCache, Mutation, MutationKind, ObligationExtent, Registry,
    RequestedCrs, Resolver, Router as CoreRouter, Sequence, StaticResolver, StorageDecl,
    StyleStore, TileCache,
};
use tellurion_geopackage::GeopackageDriverFactory;
use tower::ServiceExt;

fn temp_gpkg_path(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "tellurion-geopackage-contract-test-{}-{}-{name}.gpkg",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    path
}

/// Removes the fixture file and its WAL-mode `-wal`/`-shm` sidecars (SQLite
/// only cleans those up on a graceful `close()`, which a test that just
/// drops its `Arc<dyn StorageDriver>` never triggers) — keeps the OS temp
/// directory from accumulating leftovers across repeated test runs.
fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{}-wal", path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", path.display()));
}

/// Provisions `path` with one feature table (`demo`: `id`/`geom`/`name`/
/// `population`), its GeoPackage metadata rows, its R*Tree spatial index and
/// maintenance triggers, and (unless `with_outbox` is false) the outbox
/// table — the same DDL shape `tellurion-ingest::geopackage` produces,
/// duplicated here for the same reason `tellurion-server`'s own
/// `geopackage_binary.rs` test duplicates it (see that file's own doc): no
/// dependency edge exists (or should exist) from a test to the ingest
/// binary crate.
fn provision(path: &Path, srid: i32, with_outbox: bool) {
    let conn = Connection::open(path).expect("creates the .gpkg file");
    conn.execute_batch(&format!(
        r#"
CREATE TABLE gpkg_spatial_ref_sys (
    srs_name TEXT NOT NULL, srs_id INTEGER NOT NULL PRIMARY KEY,
    organization TEXT NOT NULL, organization_coordsys_id INTEGER NOT NULL,
    definition TEXT NOT NULL, description TEXT
);
CREATE TABLE gpkg_contents (
    table_name TEXT NOT NULL PRIMARY KEY, data_type TEXT NOT NULL, identifier TEXT UNIQUE,
    description TEXT DEFAULT '', last_change DATETIME NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    min_x DOUBLE, min_y DOUBLE, max_x DOUBLE, max_y DOUBLE, srs_id INTEGER
);
CREATE TABLE gpkg_geometry_columns (
    table_name TEXT NOT NULL, column_name TEXT NOT NULL, geometry_type_name TEXT NOT NULL,
    srs_id INTEGER NOT NULL, z TINYINT NOT NULL, m TINYINT NOT NULL,
    CONSTRAINT pk_geom_cols PRIMARY KEY (table_name, column_name)
);
INSERT INTO gpkg_spatial_ref_sys VALUES ('test srs', {srid}, 'EPSG', {srid}, 'n/a', NULL);

CREATE TABLE "demo" ("id" INTEGER PRIMARY KEY, "geom" POINT, "name" TEXT, "population" INTEGER, "observed_at" DATETIME);
INSERT INTO gpkg_contents (table_name, data_type, identifier, srs_id) VALUES ('demo', 'features', 'demo', {srid});
INSERT INTO gpkg_geometry_columns (table_name, column_name, geometry_type_name, srs_id, z, m) VALUES ('demo', 'geom', 'POINT', {srid}, 0, 0);

CREATE VIRTUAL TABLE "rtree_demo_geom" USING rtree(id, minx, maxx, miny, maxy);

CREATE TRIGGER "rtree_demo_geom_insert" AFTER INSERT ON "demo"
  WHEN (NEW."geom" NOT NULL AND NOT ST_IsEmpty(NEW."geom"))
BEGIN
  INSERT OR REPLACE INTO "rtree_demo_geom" VALUES (
    NEW."id", ST_MinX(NEW."geom"), ST_MaxX(NEW."geom"), ST_MinY(NEW."geom"), ST_MaxY(NEW."geom")
  );
END;
CREATE TRIGGER "rtree_demo_geom_update1" AFTER UPDATE OF "geom" ON "demo"
  WHEN (OLD."id" = NEW."id" AND (NEW."geom" NOTNULL AND NOT ST_IsEmpty(NEW."geom")))
BEGIN
  DELETE FROM "rtree_demo_geom" WHERE id = OLD."id";
  INSERT INTO "rtree_demo_geom" VALUES (
    NEW."id", ST_MinX(NEW."geom"), ST_MaxX(NEW."geom"), ST_MinY(NEW."geom"), ST_MaxY(NEW."geom")
  );
END;
CREATE TRIGGER "rtree_demo_geom_update2" AFTER UPDATE OF "geom" ON "demo"
  WHEN (OLD."id" = NEW."id" AND (NEW."geom" ISNULL OR ST_IsEmpty(NEW."geom")))
BEGIN
  DELETE FROM "rtree_demo_geom" WHERE id = OLD."id";
END;
CREATE TRIGGER "rtree_demo_geom_update3" AFTER UPDATE OF "id" ON "demo"
  WHEN OLD."id" != NEW."id" AND (NEW."geom" NOTNULL AND NOT ST_IsEmpty(NEW."geom"))
BEGIN
  DELETE FROM "rtree_demo_geom" WHERE id = OLD."id";
  INSERT OR REPLACE INTO "rtree_demo_geom" VALUES (
    NEW."id", ST_MinX(NEW."geom"), ST_MaxX(NEW."geom"), ST_MinY(NEW."geom"), ST_MaxY(NEW."geom")
  );
END;
CREATE TRIGGER "rtree_demo_geom_update4" AFTER UPDATE ON "demo"
  WHEN OLD."id" != NEW."id" AND (NEW."geom" ISNULL OR ST_IsEmpty(NEW."geom"))
BEGIN
  DELETE FROM "rtree_demo_geom" WHERE id IN (OLD."id", NEW."id");
END;
CREATE TRIGGER "rtree_demo_geom_delete" AFTER DELETE ON "demo"
  WHEN old."geom" NOT NULL
BEGIN
  DELETE FROM "rtree_demo_geom" WHERE id = OLD."id";
END;
"#
    ))
    .expect("provisions the fixture .gpkg schema");

    if with_outbox {
        conn.execute_batch(
            r#"
CREATE TABLE "demo_outbox" (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    feature_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('upsert', 'delete')),
    payload TEXT,
    committed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    extent_crs84 TEXT
);
"#,
        )
        .expect("provisions the outbox table");
    }
}

fn build_driver(path: &Path) -> Arc<dyn tellurion_core::StorageDriver> {
    let env_var = format!(
        "TELLURION_GEOPACKAGE_CONTRACT_TEST_{}",
        path.file_name()
            .unwrap()
            .to_string_lossy()
            .replace(['-', '.'], "_")
    );
    std::env::set_var(&env_var, path);
    let decl = StorageDecl {
        id: "main".to_string(),
        driver: "geopackage".to_string(),
        url_env: env_var,
        pool_size: None,
    };
    GeopackageDriverFactory::new()
        .build(&decl)
        .expect("builds the driver against a provisioned fixture")
}

fn collection(table: &str, srid: Option<i32>) -> CollectionDecl {
    let mut decl: CollectionDecl = serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\ndatetime: observed_at\n"
    ))
    .unwrap();
    decl.srid = srid;
    decl
}

/// Same shape as [`collection`], with `id_type: uuid` declared — the
/// unconditional-refusal fixture (`#87`): the GeoPackage format mandates an
/// `INTEGER PRIMARY KEY` feature id column, so this driver refuses any
/// other declared `id_type` by name regardless of what the physical table
/// actually looks like.
fn collection_uuid(table: &str, srid: Option<i32>) -> CollectionDecl {
    let mut decl = collection(table, srid);
    decl.id_type = tellurion_core::IdType::Uuid;
    decl
}

/// Same shape as [`collection`], with `id_type: text` declared (`#94`) — the
/// same unconditional-refusal fixture as [`collection_uuid`], for the other
/// non-`Integer` `id_type` this driver refuses unconditionally.
fn collection_text(table: &str, srid: Option<i32>) -> CollectionDecl {
    let mut decl = collection(table, srid);
    decl.id_type = tellurion_core::IdType::Text;
    decl
}

fn point_feature(x: f64, y: f64, name: &str, population: i64) -> serde_json::Value {
    serde_json::json!({
        "type": "Feature",
        "geometry": {"type": "Point", "coordinates": [x, y]},
        "properties": {"name": name, "population": population}
    })
}

fn stored_geometry_blob(path: &Path, id: i64) -> Vec<u8> {
    Connection::open(path)
        .unwrap()
        .query_row("SELECT geom FROM demo WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .unwrap()
}

fn gpkg_xy_envelope(blob: &[u8]) -> [f64; 4] {
    assert_eq!(&blob[0..2], b"GP");
    assert_eq!((blob[3] >> 1) & 0x7, 1, "fixture writes a 2D envelope");
    let f64_at = |offset: usize| f64::from_le_bytes(blob[offset..offset + 8].try_into().unwrap());
    [f64_at(8), f64_at(24), f64_at(16), f64_at(32)]
}

fn assert_near(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1.0e-6,
        "expected {expected}, got {actual}"
    );
}

/// `#90`: a deliberately dense `LineString` of `vertex_count` points, tiny
/// steps apart around `(x0, y0)` — unlike `tellurion-postgis`, this driver
/// applies no simplification at all (`#90`'s own non-goal for this slice),
/// so there is no tolerance to out-engineer here the way the PostGIS live
/// tests need: every one of `vertex_count` coordinates is guaranteed to
/// reach the shared encoder's vertex accounting untouched.
fn dense_linestring_feature(x0: f64, y0: f64, vertex_count: usize) -> serde_json::Value {
    let coords: Vec<[f64; 2]> = (0..vertex_count)
        .map(|i| [x0 + (i as f64) * 0.0000001, y0])
        .collect();
    serde_json::json!({
        "type": "Feature",
        "geometry": {"type": "LineString", "coordinates": coords},
        "properties": {"name": "dense", "population": 0}
    })
}

fn crossing_line_feature() -> serde_json::Value {
    serde_json::json!({
        "type": "Feature",
        "geometry": {
            "type": "LineString",
            "coordinates": [[-25_000_000.0, 0.0], [25_000_000.0, 0.0]]
        },
        "properties": {"name": "crossing", "population": 1}
    })
}

fn crossing_polygon_with_hole_feature() -> serde_json::Value {
    serde_json::json!({
        "type": "Feature",
        "geometry": {
            "type": "Polygon",
            "coordinates": [
                [[15_000_000.0, -5_000_000.0], [25_000_000.0, -5_000_000.0], [25_000_000.0, 5_000_000.0], [15_000_000.0, 5_000_000.0], [15_000_000.0, -5_000_000.0]],
                [[17_000_000.0, -1_000_000.0], [17_000_000.0, 1_000_000.0], [19_000_000.0, 1_000_000.0], [19_000_000.0, -1_000_000.0], [17_000_000.0, -1_000_000.0]]
            ]
        },
        "properties": {"name": "crossing", "population": 1}
    })
}

/// A closed square ring of side `2 * half`, centered at `(cx, cy)`, wound
/// counter-clockwise if `ccw` else clockwise.
fn square_ring(cx: f64, cy: f64, half: f64, ccw: bool) -> Vec<[f64; 2]> {
    let mut ring = vec![
        [cx - half, cy - half],
        [cx + half, cy - half],
        [cx + half, cy + half],
        [cx - half, cy + half],
    ];
    if !ccw {
        ring.reverse();
    }
    ring.push(ring[0]);
    ring
}

/// A `MultiPolygon` wound the ordinary (OGC Simple Features / GeoJSON)
/// way real-world tools like ogr2ogr actually produce it: every exterior
/// ring counter-clockwise, every hole clockwise (`#100`) — one plain
/// polygon, plus one polygon with a hole, well clear of each other so
/// there is no question of which ring belongs to which polygon. Sized in
/// hundreds of kilometers so a world-covering z0 tile's coarse
/// (extent-4096-over-the-whole-world) quantization still leaves every
/// ring comfortably non-degenerate.
fn conventionally_wound_multipolygon_feature() -> serde_json::Value {
    let plain = vec![square_ring(-1_500_000.0, 0.0, 500_000.0, true)];
    let with_hole = vec![
        square_ring(1_500_000.0, 0.0, 500_000.0, true),
        square_ring(1_500_000.0, 0.0, 150_000.0, false),
    ];
    serde_json::json!({
        "type": "Feature",
        "geometry": {
            "type": "MultiPolygon",
            "coordinates": [plain, with_hole]
        },
        "properties": {"name": "osm-like", "population": 0}
    })
}

/// The `id` property string of every feature in a decoded one-layer MVT
/// tile — this driver always writes `id` as tag 0 regardless of storage SRID
/// (the shared encoder's fixed first tag), so this is a cheap, geometry-blind
/// way for a test to ask "which rows landed in this tile" without hand-
/// decoding the geometry's own command/parameter integers.
fn mvt_feature_ids(bytes: &[u8]) -> std::collections::HashSet<String> {
    use geozero::mvt::{Message, Tile};
    let decoded = Tile::decode(bytes).expect("valid MVT protobuf bytes");
    let mut ids = std::collections::HashSet::new();
    for layer in &decoded.layers {
        for feature in &layer.features {
            for pair in feature.tags.chunks(2) {
                let key = &layer.keys[pair[0] as usize];
                if key == "id" {
                    if let Some(id) = &layer.values[pair[1] as usize].string_value {
                        ids.insert(id.clone());
                    }
                }
            }
        }
    }
    ids
}

#[tokio::test]
async fn catalog_introspection_reports_the_provisioned_shape() {
    let path = temp_gpkg_path("catalog");
    provision(&path, 3857, true);
    let driver = build_driver(&path);

    let collections = driver.catalog_source().collections().await.unwrap();
    assert_eq!(collections.len(), 1);
    let physical = &collections[0];
    assert_eq!(physical.name, "demo");
    assert_eq!(physical.geometry_column.as_deref(), Some("geom"));
    assert_eq!(physical.primary_key.as_deref(), Some("id"));
    assert_eq!(physical.srid, Some(3857));
    assert_eq!(physical.geometry_type.as_deref(), Some("POINT"));

    let schema = driver
        .catalog_source()
        .attribute_schema(physical)
        .await
        .unwrap()
        .unwrap();
    let names: Vec<&str> = schema.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"name"));
    assert!(names.contains(&"population"));
    assert!(names.contains(&"observed_at"));
    assert!(!names.contains(&"geom"));

    assert_eq!(
        driver
            .catalog_source()
            .temporal_column(physical)
            .await
            .unwrap()
            .as_deref(),
        Some("observed_at")
    );

    cleanup(&path);
}

#[tokio::test]
async fn write_then_read_back_then_delete_round_trips_through_the_public_contract() {
    let path = temp_gpkg_path("write_roundtrip");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let write_sink = driver.write_sink().expect("advertises WriteSink");
    let features = driver.feature_source().expect("advertises FeatureSource");

    let sequence = write_sink
        .apply_with_crs(
            &collection,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(point_feature(10.0, 20.0, "alpha", 100)),
            },
            RequestedCrs::Storage,
        )
        .await
        .unwrap();
    assert_eq!(sequence, Sequence(1));

    let item = features
        .item(&collection, "1", None)
        .await
        .unwrap()
        .expect("the written item reads back");
    assert_eq!(item["properties"]["name"], "alpha");
    assert_eq!(item["geometry"]["coordinates"][0], 10.0);
    assert_eq!(item["geometry"]["coordinates"][1], 20.0);

    write_sink
        .apply_with_crs(
            &collection,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Delete,
            },
            RequestedCrs::Storage,
        )
        .await
        .unwrap();
    assert_eq!(features.item(&collection, "1", None).await.unwrap(), None);

    // The outbox saw both the upsert and the delete, in order.
    let outbox = driver.outbox_source().expect("advertises OutboxSource");
    let obligations = outbox
        .read_after(&collection, Sequence(0), 10)
        .await
        .unwrap();
    assert_eq!(obligations.len(), 2);
    assert!(matches!(obligations[0].kind, MutationKind::Upsert(_)));
    assert!(matches!(obligations[1].kind, MutationKind::Delete));
    assert_eq!(
        outbox.primary_high_water(&collection).await.unwrap(),
        Sequence(2)
    );

    cleanup(&path);
}

#[tokio::test]
async fn omitted_crs_write_reprojects_crs84_coordinates_and_envelope_to_3857() {
    let path = temp_gpkg_path("write_crs84_to_3857");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let write_sink = driver.write_sink().expect("advertises WriteSink");

    assert!(
        write_sink.crs_capable(),
        "the sink must advertise the Content-Crs contract it implements"
    );
    write_sink
        .apply_with_crs(
            &collection,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(point_feature(12.0, 41.0, "rome", 1)),
            },
            RequestedCrs::Omitted,
        )
        .await
        .unwrap();

    let item = driver
        .feature_source()
        .unwrap()
        .item(&collection, "1", None)
        .await
        .unwrap()
        .unwrap();
    assert_near(
        item["geometry"]["coordinates"][0].as_f64().unwrap(),
        1_335_833.889_519_282_8,
    );
    assert_near(
        item["geometry"]["coordinates"][1].as_f64().unwrap(),
        5_012_341.663_847_514,
    );

    let envelope = gpkg_xy_envelope(&stored_geometry_blob(&path, 1));
    assert_near(envelope[0], 1_335_833.889_519_282_8);
    assert_near(envelope[1], 5_012_341.663_847_514);
    assert_near(envelope[2], 1_335_833.889_519_282_8);
    assert_near(envelope[3], 5_012_341.663_847_514);
    cleanup(&path);
}

#[tokio::test]
async fn write_crs_identity_and_declared_storage_paths_do_not_reproject() {
    let path_4326 = temp_gpkg_path("write_crs84_identity");
    provision(&path_4326, 4326, true);
    let driver_4326 = build_driver(&path_4326);
    let collection_4326 = collection("demo", Some(4326));
    driver_4326
        .write_sink()
        .unwrap()
        .apply_with_crs(
            &collection_4326,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(point_feature(12.0, 41.0, "degrees", 1)),
            },
            RequestedCrs::Crs84,
        )
        .await
        .unwrap();
    let identity = driver_4326
        .feature_source()
        .unwrap()
        .item(&collection_4326, "1", None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        identity["geometry"]["coordinates"],
        serde_json::json!([12, 41])
    );
    cleanup(&path_4326);

    let path_3857 = temp_gpkg_path("write_storage_identity");
    provision(&path_3857, 3857, true);
    let driver_3857 = build_driver(&path_3857);
    let collection_3857 = collection("demo", Some(3857));
    driver_3857
        .write_sink()
        .unwrap()
        .apply_with_crs(
            &collection_3857,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(point_feature(
                    1_335_833.889_519_282_8,
                    5_012_341.663_847_514,
                    "meters",
                    1,
                )),
            },
            RequestedCrs::Storage,
        )
        .await
        .unwrap();
    let stored = driver_3857
        .feature_source()
        .unwrap()
        .item(&collection_3857, "1", None)
        .await
        .unwrap()
        .unwrap();
    assert_near(
        stored["geometry"]["coordinates"][0].as_f64().unwrap(),
        1_335_833.889_519_282_8,
    );
    assert_near(
        stored["geometry"]["coordinates"][1].as_f64().unwrap(),
        5_012_341.663_847_514,
    );
    cleanup(&path_3857);
}

#[tokio::test]
async fn batch_reprojects_every_crs84_geometry_to_3857() {
    let path = temp_gpkg_path("batch_crs84_to_3857");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let mutations = vec![
        Mutation {
            feature_id: "1".to_string(),
            kind: MutationKind::Upsert(point_feature(12.0, 41.0, "rome", 1)),
        },
        Mutation {
            feature_id: "2".to_string(),
            kind: MutationKind::Upsert(point_feature(2.0, 48.0, "paris", 2)),
        },
    ];
    driver
        .write_sink()
        .unwrap()
        .apply_batch(&collection, mutations, RequestedCrs::Crs84, false)
        .await
        .unwrap();

    let features = driver.feature_source().unwrap();
    let first = features
        .item(&collection, "1", None)
        .await
        .unwrap()
        .unwrap();
    let second = features
        .item(&collection, "2", None)
        .await
        .unwrap()
        .unwrap();
    assert_near(
        first["geometry"]["coordinates"][0].as_f64().unwrap(),
        1_335_833.889_519_282_8,
    );
    assert_near(
        first["geometry"]["coordinates"][1].as_f64().unwrap(),
        5_012_341.663_847_514,
    );
    assert_near(
        second["geometry"]["coordinates"][0].as_f64().unwrap(),
        222_638.981_586_547_13,
    );
    assert_near(
        second["geometry"]["coordinates"][1].as_f64().unwrap(),
        6_106_854.834_885_075,
    );
    cleanup(&path);
}

#[tokio::test]
async fn write_refuses_a_storage_srid_outside_the_embedded_transform_contract() {
    let path = temp_gpkg_path("write_unsupported_crs");
    provision(&path, 2154, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(2154));
    let error = driver
        .write_sink()
        .unwrap()
        .apply_with_crs(
            &collection,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(point_feature(2.0, 48.0, "unsupported", 1)),
            },
            RequestedCrs::Crs84,
        )
        .await
        .expect_err("unsupported storage CRS must be refused rather than relabelled");
    assert!(
        matches!(&error, CoreError::Invalid(message) if message.contains("2154")),
        "unsupported input transformation must be a named client refusal, got {error:?}"
    );
    assert!(driver
        .feature_source()
        .unwrap()
        .item(&collection, "1", None)
        .await
        .unwrap()
        .is_none());
    cleanup(&path);
}

#[tokio::test]
async fn declared_storage_crs_is_an_identity_write_for_any_storage_srid() {
    let path = temp_gpkg_path("write_storage_identity_2154");
    provision(&path, 2154, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(2154));

    driver
        .write_sink()
        .unwrap()
        .apply_with_crs(
            &collection,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(point_feature(700_000.0, 6_600_000.0, "lambert-93", 1)),
            },
            RequestedCrs::Storage,
        )
        .await
        .unwrap();

    let item = driver
        .feature_source()
        .unwrap()
        .item(&collection, "1", None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        item["geometry"]["coordinates"],
        serde_json::json!([700000, 6600000])
    );
    assert_eq!(
        gpkg_xy_envelope(&stored_geometry_blob(&path, 1)),
        [700_000.0, 6_600_000.0, 700_000.0, 6_600_000.0]
    );
    cleanup(&path);
}

#[tokio::test]
async fn delete_does_not_require_a_geometry_transform() {
    let path = temp_gpkg_path("delete_without_transform");
    provision(&path, 2154, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(2154));

    driver
        .write_sink()
        .unwrap()
        .apply_with_crs(
            &collection,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Delete,
            },
            RequestedCrs::Crs84,
        )
        .await
        .expect("a delete carries no coordinates to transform");
    cleanup(&path);
}

#[tokio::test]
async fn null_geometry_does_not_require_a_geometry_transform() {
    let path = temp_gpkg_path("null_geometry_without_transform");
    provision(&path, 2154, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(2154));
    let feature = serde_json::json!({
        "type": "Feature",
        "geometry": null,
        "properties": {"name": "non-spatial", "population": 1}
    });

    driver
        .write_sink()
        .unwrap()
        .apply_with_crs(
            &collection,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(feature),
            },
            RequestedCrs::Crs84,
        )
        .await
        .expect("a null geometry carries no coordinates to transform");
    let item = driver
        .feature_source()
        .unwrap()
        .item(&collection, "1", None)
        .await
        .unwrap()
        .unwrap();
    assert!(item["geometry"].is_null());
    cleanup(&path);
}

#[tokio::test]
async fn create_stays_unsupported_when_the_sink_is_crs_capable() {
    let path = temp_gpkg_path("create_stays_unsupported");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let error = driver
        .write_sink()
        .unwrap()
        .create_with_crs(
            &collection,
            point_feature(12.0, 41.0, "rome", 1),
            RequestedCrs::Omitted,
        )
        .await
        .expect_err("CRS support must not invent server-assigned create support");
    assert!(matches!(
        error,
        CoreError::CapabilityUnsupported { capability, .. } if capability == "create"
    ));
    cleanup(&path);
}

#[test]
fn features_conformance_is_declared_only_for_explicitly_supported_storage_crs() {
    let path = temp_gpkg_path("features_conformance_crs_gate");
    provision(&path, 4326, true);
    let driver = build_driver(&path);
    let sink = driver.write_sink().unwrap();

    for srid in [4326, 3857] {
        assert_eq!(
            sink.features_conformance_classes(&collection("demo", Some(srid))),
            vec![tellurion_core::FEATURES_PART4_FEATURES_CLASS]
        );
    }
    for srid in [None, Some(2154)] {
        assert!(
            sink.features_conformance_classes(&collection("demo", srid))
                .is_empty(),
            "an unknown or unsupported storage CRS must withhold the class"
        );
    }
    cleanup(&path);
}

/// `OutboxSource::prune_before` (`#160`): the retention worker supplies a
/// consumer-aware floor, while the driver removes no more than one bounded
/// batch from the eligible prefix. A later obligation must never disappear
/// merely because an earlier consumer has caught up.
#[tokio::test]
async fn outbox_pruning_honors_the_floor_and_batch_cap() {
    let path = temp_gpkg_path("outbox_prune_floor_cap");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let write_sink = driver.write_sink().expect("advertises WriteSink");
    let outbox = driver.outbox_source().expect("advertises OutboxSource");

    for id in 1..=5 {
        write_sink
            .apply(
                &collection,
                Mutation {
                    feature_id: id.to_string(),
                    kind: MutationKind::Upsert(point_feature(id as f64, 0.0, "row", id)),
                },
            )
            .await
            .unwrap();
    }

    assert_eq!(
        outbox
            .prune_before(&collection, Sequence(3), 2)
            .await
            .unwrap(),
        2,
        "one pass must not delete more than its configured batch"
    );
    let remaining = outbox
        .read_after(&collection, Sequence(0), 10)
        .await
        .unwrap();
    assert_eq!(
        remaining
            .iter()
            .map(|obligation| obligation.sequence)
            .collect::<Vec<_>>(),
        vec![Sequence(3), Sequence(4), Sequence(5)],
        "the computed floor is inclusive and must not cross into later obligations"
    );

    assert_eq!(
        outbox
            .prune_before(&collection, Sequence(3), 10)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        outbox
            .read_after(&collection, Sequence(0), 10)
            .await
            .unwrap()
            .iter()
            .map(|obligation| obligation.sequence)
            .collect::<Vec<_>>(),
        vec![Sequence(4), Sequence(5)]
    );

    cleanup(&path);
}

/// Repeated producer/pruner cycles keep the embedded outbox bounded without
/// reclaiming the SQLite file itself: each pass leaves the newest obligation
/// behind for the lagging consumer, and no stale prefix accumulates.
#[tokio::test]
async fn repeated_outbox_write_and_prune_cycles_keep_row_growth_bounded() {
    let path = temp_gpkg_path("outbox_prune_bounded_growth");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let write_sink = driver.write_sink().expect("advertises WriteSink");
    let outbox = driver.outbox_source().expect("advertises OutboxSource");

    for id in 1..=12 {
        let sequence = write_sink
            .apply(
                &collection,
                Mutation {
                    feature_id: id.to_string(),
                    kind: MutationKind::Upsert(point_feature(id as f64, 0.0, "row", id)),
                },
            )
            .await
            .unwrap();

        if sequence.0 > 1 {
            assert_eq!(
                outbox
                    .prune_before(&collection, Sequence(sequence.0 - 1), 2)
                    .await
                    .unwrap(),
                1
            );
        }

        let remaining = outbox
            .read_after(&collection, Sequence(0), 20)
            .await
            .unwrap();
        assert_eq!(
            remaining.len(),
            1,
            "cycle {id} must retain only the lagging tail"
        );
        assert_eq!(remaining[0].sequence, sequence);
    }

    cleanup(&path);
}

/// `#87`, extended to `Text` by `#94`: a collection declaring a non-
/// `Integer` `id_type` against this driver refuses by name — both on the
/// read path (`item`) and the write path (`apply`) — rather than silently
/// treating a non-numeric id as a failed integer parse (a plain "not
/// found"/no-op). The physical table here is a completely ordinary,
/// correctly provisioned `INTEGER PRIMARY KEY` fixture: the refusal is
/// unconditional on the declared `id_type` alone, never a live check against
/// the table's real shape (contrast `tellurion-postgis`'s `IdTypeMismatch`,
/// which the format can actually support and so must check live). Proven
/// for both non-`Integer` `id_type` values this driver can be asked to
/// declare — `Uuid` and `Text` — since the refusal is a single `!=
/// IdType::Integer` check with nothing type-specific about it.
#[tokio::test]
async fn item_and_apply_refuse_named_when_the_collection_declares_a_non_integer_id_type() {
    for (fixture_name, collection_fixture) in [
        (
            "id_type_refusal_uuid",
            collection_uuid as fn(&str, Option<i32>) -> CollectionDecl,
        ),
        (
            "id_type_refusal_text",
            collection_text as fn(&str, Option<i32>) -> CollectionDecl,
        ),
    ] {
        let path = temp_gpkg_path(fixture_name);
        provision(&path, 3857, true);
        let driver = build_driver(&path);
        let collection = collection_fixture("demo", Some(3857));
        let features = driver.feature_source().expect("advertises FeatureSource");
        let write_sink = driver.write_sink().expect("advertises WriteSink");

        match features.item(&collection, "1", None).await {
            Err(CoreError::Config(message)) => {
                assert!(message.contains("id_type"), "message was: {message}");
            }
            other => panic!(
                "expected a named Config error from item() for {fixture_name}, got {other:?}"
            ),
        }

        match write_sink
            .apply(
                &collection,
                Mutation {
                    feature_id: "1".to_string(),
                    kind: MutationKind::Upsert(point_feature(10.0, 20.0, "alpha", 100)),
                },
            )
            .await
        {
            Err(CoreError::Config(message)) => {
                assert!(message.contains("id_type"), "message was: {message}");
            }
            other => panic!(
                "expected a named Config error from apply() for {fixture_name}, got {other:?}"
            ),
        }

        cleanup(&path);
    }
}

/// Atomicity: a write against a collection whose outbox table was never
/// provisioned must fail *and* leave the data table unchanged — proving the
/// data mutation and the outbox insert commit (or roll back) together,
/// never one without the other.
#[tokio::test]
async fn a_write_with_no_outbox_table_rolls_back_the_data_mutation_too() {
    let path = temp_gpkg_path("no_outbox");
    provision(&path, 3857, false);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let write_sink = driver.write_sink().unwrap();
    let features = driver.feature_source().unwrap();

    let result = write_sink
        .apply(
            &collection,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(point_feature(1.0, 2.0, "orphan", 1)),
            },
        )
        .await;
    assert!(result.is_err(), "a write with no outbox table must fail");
    assert_eq!(
        features.item(&collection, "1", None).await.unwrap(),
        None,
        "the data row must not survive a failed outbox insert"
    );

    cleanup(&path);
}

#[tokio::test]
async fn bbox_filtered_items_returns_only_features_inside_the_requested_box() {
    let path = temp_gpkg_path("bbox");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let write_sink = driver.write_sink().unwrap();
    let features = driver.feature_source().unwrap();

    for (id, x, y, name) in [
        (1, 100.0, 100.0, "inside-a"),
        (2, 150.0, 150.0, "inside-b"),
        (3, 10_000_000.0, 10_000_000.0, "outside"),
    ] {
        write_sink
            .apply_with_crs(
                &collection,
                Mutation {
                    feature_id: id.to_string(),
                    kind: MutationKind::Upsert(point_feature(x, y, name, 0)),
                },
                RequestedCrs::Storage,
            )
            .await
            .unwrap();
    }

    let page = features
        .items(
            &collection,
            &ItemsQuery {
                bbox: Some([0.0, 0.0, 1000.0, 1000.0]),
                limit: 10,
                ..ItemsQuery::default()
            },
        )
        .await
        .unwrap();
    let names: Vec<&str> = page
        .features_geojson
        .iter()
        .map(|f| f["properties"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names.len(),
        2,
        "only the two in-box points match: {names:?}"
    );
    assert!(names.contains(&"inside-a"));
    assert!(names.contains(&"inside-b"));
    assert!(!names.contains(&"outside"));

    cleanup(&path);
}

/// `#150`: this driver WITHHOLDS the OGC API Features — Part 4 Optimistic
/// Locking, ETags class. `write_apply_inner`'s single-SQLite-transaction
/// commit (`#107`) does not earn it: the class exists to stop a lost update,
/// and stopping one needs the precondition re-verified inside the write
/// statement, which needs a per-row version SQLite does not have. See
/// `WriteSink::locking_conformance_classes`'s doc on `GeopackageBackend` for
/// the full reasoning.
#[tokio::test]
async fn locking_conformance_classes_withholds_etags_it_cannot_honour_atomically() {
    let path = temp_gpkg_path("locking_classes");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let write_sink = driver.write_sink().expect("advertises WriteSink");
    assert!(
        write_sink.locking_conformance_classes().is_empty(),
        "declaring a class this driver cannot honour atomically would be an \
         overclaim: {:?}",
        write_sink.locking_conformance_classes()
    );
    cleanup(&path);
}

/// `#150`: and the withholding is matched by a NAMED refusal on the write
/// path, not by silently ignoring a caller's precondition. The two must
/// agree — a client told the guard is unavailable while the server quietly
/// writes anyway is worse off than one told nothing at all.
#[tokio::test]
async fn a_conditional_write_is_refused_by_name_rather_than_silently_downgraded() {
    let path = temp_gpkg_path("locking_refusal");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let write_sink = driver.write_sink().expect("advertises WriteSink");
    let decl = collection("demo", Some(3857));

    match write_sink.row_version(&decl, "1").await {
        Err(CoreError::CapabilityUnsupported { capability, .. }) => {
            assert_eq!(capability, "optimistic-locking");
        }
        other => panic!("expected a named CapabilityUnsupported refusal, got {other:?}"),
    }

    let refusal = write_sink
        .apply_conditional(
            &decl,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Delete,
            },
            RequestedCrs::Omitted,
            &tellurion_core::locking::RowVersion::new("whatever"),
        )
        .await;
    match refusal {
        Err(CoreError::CapabilityUnsupported { capability, .. }) => {
            assert_eq!(capability, "optimistic-locking");
        }
        other => panic!("expected a named CapabilityUnsupported refusal, got {other:?}"),
    }

    cleanup(&path);
}

#[tokio::test]
async fn update_conformance_classes_declares_json_merge_patch() {
    let path = temp_gpkg_path("update_classes");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let write_sink = driver.write_sink().expect("advertises WriteSink");
    assert_eq!(
        write_sink.update_conformance_classes(),
        vec![tellurion_core::outbox::UPDATE_CONFORMANCE_CLASS]
    );
    cleanup(&path);
}

/// `#107`: exercise the HTTP merge-patch handler and the real GeoPackage
/// driver in one process. Keeping this test socket-free makes the committed
/// row and outbox observable without racing a child server's startup.
#[tokio::test]
async fn merge_patch_unsets_a_property_in_the_row_and_outbox() {
    let path = temp_gpkg_path("merge_patch");
    provision(&path, 4326, true);

    let env_var = format!(
        "TELLURION_GEOPACKAGE_PATCH_TEST_{}",
        path.file_name()
            .unwrap()
            .to_string_lossy()
            .replace(['-', '.'], "_")
    );
    std::env::set_var(&env_var, &path);
    let config: AppConfig = serde_yaml::from_str(&format!(
        r#"
storages: [ {{ id: main, driver: geopackage, url_env: {env_var} }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    datetime: observed_at
    srid: 4326
    routing: {{ write: main }}
"#
    ))
    .unwrap();
    config.validate().unwrap();

    let mut registry = Registry::new();
    registry.register(Arc::new(GeopackageDriverFactory::new()));
    let core_router = CoreRouter::build(&config, &registry).unwrap();
    let (collection, sink) = core_router
        .resolve_write("public", "default", "demo")
        .await
        .unwrap();
    sink.apply(
        &collection,
        Mutation {
            feature_id: "1".to_string(),
            kind: MutationKind::Upsert(point_feature(12.5, 41.9, "alpha", 7)),
        },
    )
    .await
    .unwrap();

    let geometry_before: Vec<u8> = Connection::open(&path)
        .unwrap()
        .query_row("SELECT geom FROM demo WHERE id = 1", [], |row| row.get(0))
        .unwrap();

    let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
    let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1024));
    let styles: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
    let app = tellurion_features::router().with_state(Arc::new(AppContext::new(
        config,
        core_router,
        resolver,
        None,
        cache,
        styles,
    )));

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/collections/demo/items/1")
                .header(header::CONTENT_TYPE, "application/merge-patch+json")
                .body(Body::from(r#"{"properties":{"population":null}}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let response_status = response.status();
    let response_bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        response_status,
        StatusCode::OK,
        "unexpected PATCH response: {}",
        String::from_utf8_lossy(&response_bytes)
    );
    let response_body: serde_json::Value = serde_json::from_slice(&response_bytes).unwrap();
    assert_eq!(
        response_body["geometry"]["coordinates"],
        serde_json::json!([12.5, 41.9])
    );
    assert!(response_body["properties"]
        .as_object()
        .unwrap()
        .contains_key("population"));
    assert!(response_body["properties"]["population"].is_null());

    let conn = Connection::open(&path).unwrap();
    let (population, geometry_after): (Option<i64>, Vec<u8>) = conn
        .query_row(
            "SELECT population, geom FROM demo WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(population, None);
    assert_eq!(geometry_after, geometry_before);
    let outbox_payload: String = conn
        .query_row(
            "SELECT payload FROM demo_outbox ORDER BY sequence DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let outbox_payload: serde_json::Value = serde_json::from_str(&outbox_payload).unwrap();
    assert!(outbox_payload["properties"]
        .as_object()
        .unwrap()
        .contains_key("population"));
    assert!(outbox_payload["properties"]["population"].is_null());

    drop(conn);
    cleanup(&path);
}

#[tokio::test]
async fn cql2_filter_narrows_items_and_refuses_a_binary_spatial_predicate() {
    let path = temp_gpkg_path("cql2");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let write_sink = driver.write_sink().unwrap();
    let features = driver.feature_source().unwrap();
    assert!(features.filter_capable());

    // `#105`: GeoPackage compiles comparison/`IS NULL`, `LIKE`/`BETWEEN`/
    // `IN`, `S_INTERSECTS`, and every temporal predicate — but refuses the
    // six wider spatial predicates by name (proven below), so
    // `spatial-functions` is withheld. `#134` withholds
    // `basic-spatial-functions` on top of that, because this driver places
    // `S_INTERSECTS` only in restricted positions while the class is defined
    // in terms of the general form — the behavioural half of that decision is
    // `intersects_general_form_and_declared_class_agree` below, which is what
    // makes this list falsifiable rather than merely restated.
    // `case-insensitive-comparison` is withheld by every driver.
    let declared = features.cql2_conformance_classes();
    let withheld = [
        tellurion_core::filter::CQL2_CLASS_SPATIAL_FUNCTIONS,
        tellurion_core::filter::CQL2_CLASS_BASIC_SPATIAL_FUNCTIONS,
        tellurion_core::filter::CQL2_CLASS_CASE_INSENSITIVE_COMPARISON,
    ];
    for class in tellurion_core::filter::CQL2_CONFORMANCE_CLASSES {
        if withheld.contains(class) {
            assert!(
                !declared.contains(class),
                "GeoPackage must not declare {class}"
            );
        } else {
            assert!(
                declared.contains(class),
                "GeoPackage should declare: {class}"
            );
        }
    }
    assert!(!declared.contains(&tellurion_core::filter::CQL2_CLASS_CASE_INSENSITIVE_COMPARISON));
    assert_eq!(features.filter_capable(), !declared.is_empty());

    // `#217`: GeoPackage neither reprojects a response (`crs_capable`) nor
    // transforms a filter's spatial literals (`filter_crs_capable`). The
    // first is what keeps OGC API — Features Part 3 Requirement 8
    // (`/req/filter/filter-crs-param`) from ever becoming binding on this
    // driver — its condition is "Server supports additional coordinate
    // reference systems" — so declaring the Part 3 classes stays honest here
    // while `filter-crs` itself is refused by name at the protocol layer.
    // Pinned together: if this driver ever learned to reproject without
    // learning to transform a filter literal, it would become the overclaim
    // `#217` was opened for.
    assert!(!features.crs_capable());
    assert!(
        !features.filter_crs_capable(),
        "GeoPackage evaluates a filter's spatial literals in the storage CRS only; declaring \
         filter_crs_capable here would advertise a transform it never performs"
    );

    for (id, name, population) in [(1, "alpha", 100), (2, "bravo", 5)] {
        write_sink
            .apply(
                &collection,
                Mutation {
                    feature_id: id.to_string(),
                    kind: MutationKind::Upsert(point_feature(0.0, 0.0, name, population)),
                },
            )
            .await
            .unwrap();
    }

    let filter = tellurion_core::filter::parse_text("population > 10").unwrap();
    let page = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(filter),
                limit: 10,
                ..ItemsQuery::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page.features_geojson.len(), 1);
    assert_eq!(page.features_geojson[0]["properties"]["name"], "alpha");

    // The refusal rule: a binary spatial predicate beyond `S_INTERSECTS` —
    // this driver's exact evaluator only covers `S_INTERSECTS` — is a named
    // error, never a silently coarse (or dropped) answer.
    let spatial = tellurion_core::filter::parse_text("S_WITHIN(geom, POINT(0 0))");
    if let Ok(spatial) = spatial {
        let refused = features
            .items(
                &collection,
                &ItemsQuery {
                    filter: Some(spatial),
                    limit: 10,
                    ..ItemsQuery::default()
                },
            )
            .await;
        assert!(
            matches!(refused, Err(CoreError::Invalid(_))),
            "a spatial predicate beyond S_INTERSECTS must be refused, not silently answered"
        );
    }

    cleanup(&path);
}

/// Exact `S_INTERSECTS` diverges from the R*Tree's own coarse bbox test: an
/// L-shaped polygon's bounding box covers its missing quadrant too, so a
/// point sitting in that quadrant must bbox-overlap the polygon (an old,
/// bbox-only answer would have wrongly matched it) yet not actually
/// intersect it, while a point inside the L's real footprint must match.
#[tokio::test]
async fn intersects_predicate_answers_exactly_not_by_bbox_alone() {
    let path = temp_gpkg_path("intersects_exact");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let write_sink = driver.write_sink().unwrap();
    let features = driver.feature_source().unwrap();

    // An L-shape: the unit square [0,4]x[0,4] minus its top-right [2,4]x[2,4]
    // quadrant. Its bounding box is still the full [0,4]x[0,4] square.
    let l_shape = serde_json::json!({
        "type": "Feature",
        "geometry": {
            "type": "Polygon",
            "coordinates": [[
                [0.0, 0.0], [4.0, 0.0], [4.0, 2.0], [2.0, 2.0],
                [2.0, 4.0], [0.0, 4.0], [0.0, 0.0]
            ]]
        },
        "properties": {"name": "l-shape", "population": 1}
    });
    write_sink
        .apply_with_crs(
            &collection,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(l_shape),
            },
            RequestedCrs::Storage,
        )
        .await
        .unwrap();

    // (3, 3) sits inside the L's own bbox but inside the missing quadrant —
    // a bbox-only answer would wrongly match; the exact one must not.
    let bbox_only_would_match =
        tellurion_core::filter::parse_text("S_INTERSECTS(geom, POINT(3 3))").unwrap();
    let page = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(bbox_only_would_match),
                limit: 10,
                ..ItemsQuery::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        page.features_geojson.len(),
        0,
        "a point in the L's missing quadrant must not exactly intersect it, even though its bbox does"
    );

    // (1, 1) sits inside the L's real footprint — the exact test must match.
    let inside_the_l =
        tellurion_core::filter::parse_text("S_INTERSECTS(geom, POINT(1 1))").unwrap();
    let page = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(inside_the_l),
                limit: 10,
                ..ItemsQuery::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page.features_geojson.len(), 1);
    assert_eq!(page.features_geojson[0]["properties"]["name"], "l-shape");

    cleanup(&path);
}

/// `#134`, the whole slice in one test: what this driver *does* with
/// `S_INTERSECTS` and what it *declares* about `S_INTERSECTS` must be the
/// same claim.
///
/// CQL2 (OGC 21-065r2) defines `basic-spatial-functions` in terms of the
/// general form — a `spatialPredicate` is a `predicate`, and Basic CQL2's
/// Requirement 1 (which the class's own Dependency pulls in, minus the
/// `spatialPredicate` exception the class lifts) promises the whole
/// `booleanExpression` grammar over it. The class's normative Abstract Test
/// Suite spells that out: Conformance Test 26
/// (`/conf/basic-spatial-functions/test-data`) asserts exact item counts for
/// `S_INTERSECTS(...) and S_INTERSECTS(...)`, `S_INTERSECTS(...) and not
/// S_INTERSECTS(...)` and `S_INTERSECTS(...) or S_INTERSECTS(...)`, and
/// Conformance Test 27 (`/conf/basic-spatial-functions/logical`) composes the
/// stored spatial predicates under `NOT`/`AND`/`OR` together. The three
/// shapes below are those three, transplanted onto this fixture.
///
/// So the assertion is not "the class is absent" — a list can be confidently
/// wrong and still pass that. It is that the declaration is a *function of*
/// the behaviour: run the general form, and require the class to be declared
/// exactly when the general form is honoured. A slice that lifts the
/// restriction (`sql::collect_intersects_check`) without re-declaring fails
/// here; a slice that re-declares without lifting fails here; and a
/// regression that answers a general-form query with the coarse bbox set
/// instead of refusing fails the middle assertion, which insists each shape
/// either returns the exactly-right rows or is refused by name.
#[tokio::test]
async fn intersects_general_form_and_declared_class_agree() {
    let path = temp_gpkg_path("intersects_general_form");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let write_sink = driver.write_sink().unwrap();
    let features = driver.feature_source().unwrap();

    // Three points and two overlapping boxes, so each composition below has a
    // different right answer and no two can be confused:
    //   A = BBOX(0,0,6,6)   covers near (1,1) and mid (5,5)
    //   B = BBOX(4,4,10,10) covers mid (5,5) and far (9,9)
    for (id, x, y, name) in [
        (1, 1.0, 1.0, "near"),
        (2, 5.0, 5.0, "mid"),
        (3, 9.0, 9.0, "far"),
    ] {
        write_sink
            .apply_with_crs(
                &collection,
                Mutation {
                    feature_id: id.to_string(),
                    kind: MutationKind::Upsert(point_feature(x, y, name, 1)),
                },
                RequestedCrs::Storage,
            )
            .await
            .unwrap();
    }

    let names = |page: &tellurion_core::FeaturePage| -> Vec<String> {
        let mut out: Vec<String> = page
            .features_geojson
            .iter()
            .map(|f| f["properties"]["name"].as_str().unwrap().to_string())
            .collect();
        out.sort();
        out
    };
    let run = |text: &'static str| {
        let features = features.clone();
        let collection = collection.clone();
        async move {
            let filter = tellurion_core::filter::parse_text(text)
                .unwrap_or_else(|e| panic!("`{text}` must parse as CQL2-text: {e}"));
            features
                .items(
                    &collection,
                    &ItemsQuery {
                        filter: Some(filter),
                        limit: 10,
                        ..ItemsQuery::default()
                    },
                )
                .await
        }
    };

    // The restricted form — one `S_INTERSECTS`, in AND-position — is what this
    // driver genuinely honours, and withholding the class must not cost it.
    // Asserted first, so a regression that refused every spatial filter
    // outright cannot pass this test by making the general form "consistently
    // unsupported".
    let page = run("S_INTERSECTS(geom, BBOX(0,0,6,6))")
        .await
        .expect("one S_INTERSECTS in AND-position is this driver's supported form");
    assert_eq!(names(&page), ["mid", "near"]);
    let page = run("population > 0 AND S_INTERSECTS(geom, BBOX(4,4,10,10))")
        .await
        .expect("S_INTERSECTS AND-ed with a scalar predicate is still AND-position");
    assert_eq!(names(&page), ["far", "mid"]);

    // The general form: Conformance Test 26's own three compositions, each
    // with the rows the standard's semantics require.
    let general_form: [(&'static str, &[&str]); 3] = [
        (
            "S_INTERSECTS(geom, BBOX(0,0,6,6)) AND S_INTERSECTS(geom, BBOX(4,4,10,10))",
            &["mid"],
        ),
        (
            "S_INTERSECTS(geom, BBOX(0,0,6,6)) AND NOT S_INTERSECTS(geom, BBOX(4,4,10,10))",
            &["near"],
        ),
        (
            "S_INTERSECTS(geom, BBOX(0,0,6,6)) OR S_INTERSECTS(geom, BBOX(4,4,10,10))",
            &["far", "mid", "near"],
        ),
    ];
    let mut general_form_honoured = true;
    for (text, expected) in general_form {
        match run(text).await {
            Ok(page) => {
                // Named refusal or the right answer — never a third option.
                // Dropping the refusal without redesigning the evaluator
                // would AND one disjunct's R*Tree bbox clause into the SQL
                // and exact-test only the rows that survived it, silently
                // losing the rows the other disjunct should have contributed.
                // That is the degradation this arm exists to catch.
                assert_eq!(
                    names(&page),
                    expected,
                    "`{text}` was answered, so it must be answered correctly"
                );
            }
            Err(err) => {
                general_form_honoured = false;
                let CoreError::Invalid(message) = &err else {
                    panic!("`{text}` must be refused as a 400-mapping Invalid, got: {err}");
                };
                assert!(
                    message.contains("S_INTERSECTS"),
                    "`{text}` must be refused BY NAME, naming the construct: {message}"
                );
            }
        }
    }

    // The agreement, in one line. `basic-spatial-functions` is declared iff
    // the general form the class is defined in terms of actually works.
    let declared = features.cql2_conformance_classes();
    assert_eq!(
        declared.contains(&tellurion_core::filter::CQL2_CLASS_BASIC_SPATIAL_FUNCTIONS),
        general_form_honoured,
        "declaring basic-spatial-functions promises S_INTERSECTS anywhere the booleanExpression \
         BNF admits a predicate; declared={declared:?}, general form honoured={general_form_honoured}"
    );
    // ...and the classes that never depended on the spatial predicate's
    // position are untouched by this narrowing — withholding one class must
    // not quietly cost a deployment the others.
    for kept in [
        tellurion_core::filter::CQL2_CLASS_BASIC,
        tellurion_core::filter::CQL2_CLASS_CQL2_TEXT,
        tellurion_core::filter::CQL2_CLASS_CQL2_JSON,
        tellurion_core::filter::CQL2_CLASS_ADVANCED_COMPARISON_OPERATORS,
        tellurion_core::filter::CQL2_CLASS_TEMPORAL_FUNCTIONS,
    ] {
        assert!(
            declared.contains(&kept),
            "still honoured, still declared: {kept}"
        );
    }

    cleanup(&path);
}

/// Pagination correctness under an exact `S_INTERSECTS` post-filter: rows
/// that bbox-overlap the query needle but don't actually intersect it are
/// interleaved, by pk, with rows that do — including straddling a page
/// boundary (`limit: 2` against four true matches). Every matching id must
/// come back exactly once, in order, across the full walk; no false-positive
/// id may ever appear.
#[tokio::test]
async fn paging_under_an_intersects_filter_has_no_dup_or_skip_across_page_boundaries() {
    let path = temp_gpkg_path("intersects_paging");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let write_sink = driver.write_sink().unwrap();
    let features = driver.feature_source().unwrap();

    // Needle: the diagonal from (0,0) to (10,10) — bbox [0,0]x[10,10].
    // Points exactly on the diagonal (even pk) intersect it exactly; points
    // off the diagonal but still inside its bbox (odd pk) bbox-overlap it
    // without actually intersecting — a false-positive candidate the R*Tree
    // pushdown alone can't rule out.
    let rows: [(i64, f64, f64); 8] = [
        (1, 1.0, 9.0), // off-diagonal, in bbox: false positive
        (2, 2.0, 2.0), // on diagonal: true match
        (3, 9.0, 1.0), // off-diagonal, in bbox: false positive
        (4, 4.0, 4.0), // on diagonal: true match
        (5, 3.0, 7.0), // off-diagonal, in bbox: false positive
        (6, 6.0, 6.0), // on diagonal: true match
        (7, 7.0, 3.0), // off-diagonal, in bbox: false positive
        (8, 8.0, 8.0), // on diagonal: true match
    ];
    for (id, x, y) in rows {
        write_sink
            .apply_with_crs(
                &collection,
                Mutation {
                    feature_id: id.to_string(),
                    kind: MutationKind::Upsert(point_feature(x, y, &format!("f{id}"), id)),
                },
                RequestedCrs::Storage,
            )
            .await
            .unwrap();
    }

    let filter =
        tellurion_core::filter::parse_text("S_INTERSECTS(geom, LINESTRING(0 0, 10 10))").unwrap();

    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page = features
            .items(
                &collection,
                &ItemsQuery {
                    filter: Some(filter.clone()),
                    limit: 2,
                    token: token.clone(),
                    ..ItemsQuery::default()
                },
            )
            .await
            .unwrap();
        for f in &page.features_geojson {
            seen.push(f["id"].as_str().unwrap().to_string());
        }
        match page.next_token {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    assert_eq!(
        seen,
        vec!["2", "4", "6", "8"],
        "every on-diagonal id exactly once, in order, no off-diagonal false positive"
    );

    cleanup(&path);
}

#[tokio::test]
async fn paging_walks_every_feature_exactly_once_in_ascending_pk_order() {
    let path = temp_gpkg_path("paging");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let write_sink = driver.write_sink().unwrap();
    let features = driver.feature_source().unwrap();

    for id in 1..=5i64 {
        write_sink
            .apply(
                &collection,
                Mutation {
                    feature_id: id.to_string(),
                    kind: MutationKind::Upsert(point_feature(
                        id as f64,
                        id as f64,
                        &format!("f{id}"),
                        id,
                    )),
                },
            )
            .await
            .unwrap();
    }

    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page = features
            .items(
                &collection,
                &ItemsQuery {
                    limit: 2,
                    token: token.clone(),
                    ..ItemsQuery::default()
                },
            )
            .await
            .unwrap();
        for f in &page.features_geojson {
            seen.push(f["id"].as_str().unwrap().to_string());
        }
        match page.next_token {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    assert_eq!(seen, vec!["1", "2", "3", "4", "5"]);

    cleanup(&path);
}

#[tokio::test]
async fn mvt_tile_carries_a_real_feature_and_refuses_an_unsupported_srid() {
    let path = temp_gpkg_path("tiles");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection_3857 = collection("demo", Some(3857));
    let write_sink = driver.write_sink().unwrap();
    let tiles = driver.tile_source().expect("advertises TileSource");

    write_sink
        .apply_with_crs(
            &collection_3857,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(point_feature(1000.0, 2000.0, "tile-point", 1)),
            },
            RequestedCrs::Storage,
        )
        .await
        .unwrap();

    let coord = tellurion_core::TileCoord { z: 0, x: 0, y: 0 };
    let bytes = tiles
        .mvt_tile(&collection_3857, coord, None)
        .await
        .unwrap()
        .expect("a world-covering z0 tile carries the written point");
    assert_eq!(
        bytes.as_ref(),
        &[
            26, 35, 10, 4, 100, 101, 109, 111, 18, 13, 18, 2, 0, 0, 24, 1, 34, 5, 9, 128, 32, 128,
            32, 26, 2, 105, 100, 34, 3, 10, 1, 49, 40, 128, 32, 120, 2,
        ],
        "the shared encoder must preserve the representative GeoPackage wire bytes"
    );
    assert!(!bytes.is_empty());

    use geozero::mvt::{Message, Tile};
    let decoded = Tile::decode(bytes.as_ref()).expect("valid MVT protobuf bytes");
    assert_eq!(decoded.layers.len(), 1);
    assert_eq!(decoded.layers[0].features.len(), 1);

    // Refusal: `#89` widens the tiles lane to 3857 and 4326, but an SRID
    // outside both of those (no general source-CRS matrix) must still refuse
    // by name rather than serve a distorted tile.
    let collection_2154 = collection("demo", Some(2154));
    let refused = tiles.mvt_tile(&collection_2154, coord, None).await;
    assert!(matches!(refused, Err(CoreError::Invalid(_))));

    cleanup(&path);
}

#[tokio::test]
async fn mvt_tile_preserves_legacy_crossing_line_and_polygon_bytes() {
    let cases: [(&str, serde_json::Value, &[u8]); 2] = [
        (
            "line",
            crossing_line_feature(),
            &[
                26, 39, 10, 4, 100, 101, 109, 111, 18, 17, 18, 2, 0, 0, 24, 2, 34, 9, 9, 247, 7,
                130, 32, 10, 238, 79, 0, 26, 2, 105, 100, 34, 3, 10, 1, 49, 40, 128, 32, 120, 2,
            ],
        ),
        (
            "polygon",
            crossing_polygon_with_hole_feature(),
            &[
                26, 62, 10, 4, 100, 101, 109, 111, 18, 40, 18, 2, 0, 0, 24, 3, 34, 32, 9, 250, 55,
                128, 40, 26, 0, 253, 15, 252, 15, 0, 0, 254, 15, 15, 9, 227, 12, 177, 6, 26, 152,
                3, 0, 0, 153, 3, 151, 3, 0, 15, 26, 2, 105, 100, 34, 3, 10, 1, 49, 40, 128, 32,
                120, 2,
            ],
        ),
    ];

    for (label, value, expected) in cases {
        let path = temp_gpkg_path(&format!("legacy_crossing_{label}"));
        provision(&path, 3857, true);
        let driver = build_driver(&path);
        let collection = collection("demo", Some(3857));
        driver
            .write_sink()
            .unwrap()
            .apply_with_crs(
                &collection,
                Mutation {
                    feature_id: "1".to_string(),
                    kind: MutationKind::Upsert(value),
                },
                RequestedCrs::Storage,
            )
            .await
            .unwrap();

        let bytes = driver
            .tile_source()
            .unwrap()
            .mvt_tile(
                &collection,
                tellurion_core::TileCoord { z: 0, x: 0, y: 0 },
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(bytes.as_ref(), expected, "legacy {label} bytes changed");
        cleanup(&path);
    }
}

#[tokio::test]
async fn mvt_tile_rejects_zoom_25_before_querying() {
    let path = temp_gpkg_path("tiles_zoom_25");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let tiles = driver.tile_source().unwrap();
    // A missing table makes any attempted query fail differently. The zoom
    // error must win before SQL preparation reaches this deliberately absent
    // relation.
    let collection = collection("missing_table", Some(3857));

    let result = tiles.mvt_tile(
        &collection,
        tellurion_core::TileCoord { z: 25, x: 0, y: 0 },
        None,
    );
    let error = result
        .await
        .expect_err("unsupported zoom 25 must fail before issuing a tile query");
    assert!(error.to_string().contains("zoom 25"));

    cleanup(&path);
}

#[tokio::test]
async fn mvt_tile_rejects_zoom_64_without_panicking() {
    let path = temp_gpkg_path("tiles_zoom_64");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let tiles = driver.tile_source().unwrap();
    let collection = collection("missing_table", Some(3857));

    let result = tiles.mvt_tile(
        &collection,
        tellurion_core::TileCoord { z: 64, x: 0, y: 0 },
        None,
    );
    let error = result
        .await
        .expect_err("unsupported zoom 64 must return an error rather than panic");
    assert!(error.to_string().contains("zoom 64"));

    cleanup(&path);
}

/// `#100`: real-world OSM/ogr2ogr GeoPackages wind a polygon's exterior
/// ring counter-clockwise and its holes clockwise — the OGC Simple
/// Features / GeoJSON convention. `MvtWriter`'s tile-space y-flip needs
/// the opposite, so a `MultiPolygon` wound this ordinary way must still
/// round-trip through this driver's own MVT encode path and back through
/// geozero's own reader without either polygon's rings ever landing on
/// the wrong side of the exterior/hole classification. Before `driver.rs`
/// routed geometry through winding normalization, the plain
/// polygon's exterior ring read back as an orphan hole and the tile
/// failed to decode (`MvtError::GeometryFormat`).
#[tokio::test]
async fn mvt_tile_encodes_a_conventionally_wound_multipolygon_without_misclassifying_its_rings() {
    let path = temp_gpkg_path("tiles_multipolygon_winding");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let write_sink = driver.write_sink().unwrap();
    let tiles = driver.tile_source().expect("advertises TileSource");

    write_sink
        .apply_with_crs(
            &collection,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(conventionally_wound_multipolygon_feature()),
            },
            RequestedCrs::Storage,
        )
        .await
        .unwrap();

    let coord = tellurion_core::TileCoord { z: 0, x: 0, y: 0 };
    let bytes = tiles
        .mvt_tile(&collection, coord, None)
        .await
        .unwrap()
        .expect("a world-covering z0 tile carries the written multipolygon");

    use geozero::mvt::{Message, Tile};
    let mut decoded = Tile::decode(bytes.as_ref()).expect("valid MVT protobuf bytes");
    assert_eq!(decoded.layers.len(), 1);
    let mut layer = decoded.layers.remove(0);

    use geozero::ProcessToJson;
    let geojson_text = layer
        .to_json()
        .expect("a conventionally-wound exterior must not read back as an orphan hole");
    let geojson: serde_json::Value = serde_json::from_str(&geojson_text).unwrap();
    let features = geojson["features"].as_array().unwrap();
    assert_eq!(features.len(), 1);
    let geometry = &features[0]["geometry"];
    assert_eq!(geometry["type"], "MultiPolygon");
    let polygons = geometry["coordinates"].as_array().unwrap();
    assert_eq!(polygons.len(), 2, "both polygons must round-trip");
    assert_eq!(
        polygons[0].as_array().unwrap().len(),
        1,
        "the plain polygon has no hole"
    );
    assert_eq!(
        polygons[1].as_array().unwrap().len(),
        2,
        "the second polygon's hole must stay attached to it"
    );

    cleanup(&path);
}

/// `#89`: a 4326-stored table's tile lane reprojects vertices to Web
/// Mercator at encode time instead of refusing. One point per z1 quadrant,
/// each comfortably clear of the lon=0/lat=0 boundaries so a bbox-inclusive
/// edge can never split one across two tiles, proves the transform's sign
/// and axis handling all at once: an x/y swap or a hemisphere flip would
/// land at least one point in the wrong tile, not just distort its position
/// inside the right one. The same fixture also proves the R*Tree bbox
/// pushdown still prunes correctly against 4326-stored (degrees)
/// coordinates: a tile whose reprojected query window covers none of the
/// four points comes back `Ok(None)` — a cheap, sound answer, not a decode
/// of every stored row to find nothing.
#[tokio::test]
async fn mvt_tile_reprojects_a_4326_stored_table_onto_the_matching_3857_quadrant() {
    let path = temp_gpkg_path("tiles_4326_reproject");
    provision(&path, 4326, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(4326));
    let write_sink = driver.write_sink().unwrap();
    let tiles = driver.tile_source().expect("advertises TileSource");

    for (id, lon, lat, name) in [
        (1, 45.0, 30.0, "ne"),
        (2, -45.0, 30.0, "nw"),
        (3, 45.0, -30.0, "se"),
        (4, -45.0, -30.0, "sw"),
    ] {
        write_sink
            .apply(
                &collection,
                Mutation {
                    feature_id: id.to_string(),
                    kind: MutationKind::Upsert(point_feature(lon, lat, name, id)),
                },
            )
            .await
            .unwrap();
    }

    for (label, coord, expected_id) in [
        ("ne", tellurion_core::TileCoord { z: 1, x: 1, y: 0 }, "1"),
        ("nw", tellurion_core::TileCoord { z: 1, x: 0, y: 0 }, "2"),
        ("se", tellurion_core::TileCoord { z: 1, x: 1, y: 1 }, "3"),
        ("sw", tellurion_core::TileCoord { z: 1, x: 0, y: 1 }, "4"),
    ] {
        let bytes = tiles
            .mvt_tile(&collection, coord, None)
            .await
            .unwrap()
            .unwrap_or_else(|| {
                panic!("the {label} point must reproject into its own quadrant tile")
            });
        assert_eq!(
            mvt_feature_ids(&bytes),
            [expected_id.to_string()].into_iter().collect(),
            "tile {label} ({coord:?}) carried an unexpected feature set"
        );
    }

    // Bbox pruning stays effective against 4326-stored coordinates: this
    // tile's reprojected query window sits entirely north of every stored
    // point (lat above ~67°), so it must come back empty, not an error.
    let coord_empty = tellurion_core::TileCoord { z: 2, x: 0, y: 0 };
    let empty = tiles
        .mvt_tile(&collection, coord_empty, None)
        .await
        .unwrap();
    assert_eq!(
        empty, None,
        "a tile outside all stored 4326 data must come back empty"
    );

    cleanup(&path);
}

/// `#90`: a tile whose candidate rows sum well under the effective vertex
/// budget must serve every feature, unaffected — proven with an exact
/// decoded feature count and byte-for-byte determinism across two fetches.
#[tokio::test]
async fn mvt_tile_is_unaffected_when_under_the_vertex_budget() {
    let path = temp_gpkg_path("tiles_vertex_budget_unaffected");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let write_sink = driver.write_sink().unwrap();
    let tiles = driver.tile_source().expect("advertises TileSource");

    for (id, x, y, name) in [
        (1, 1000.0, 2000.0, "a"),
        (2, 1100.0, 2000.0, "b"),
        (3, 1000.0, 2100.0, "c"),
    ] {
        write_sink
            .apply_with_crs(
                &collection,
                Mutation {
                    feature_id: id.to_string(),
                    kind: MutationKind::Upsert(point_feature(x, y, name, id)),
                },
                RequestedCrs::Storage,
            )
            .await
            .unwrap();
    }

    let coord = tellurion_core::TileCoord { z: 0, x: 0, y: 0 };
    let first = tiles
        .mvt_tile(&collection, coord, None)
        .await
        .unwrap()
        .expect("a world-covering z0 tile carries all three seeded points");
    let second = tiles
        .mvt_tile(&collection, coord, None)
        .await
        .unwrap()
        .expect("a world-covering z0 tile carries all three seeded points");

    assert_eq!(
        first.as_ref(),
        second.as_ref(),
        "an under-budget tile's wire bytes must be stable/deterministic"
    );
    assert_eq!(
        mvt_feature_ids(&first),
        ["1".to_string(), "2".to_string(), "3".to_string()]
            .into_iter()
            .collect(),
        "no seeded row is anywhere near the default vertex budget; all three must survive"
    );

    cleanup(&path);
}

/// `#90`: on the 3857 native path (`mvt_tile_inner`'s own doc — no
/// reprojection), a
/// tight `settings.tile_vertex_budget` drops the dense linestring (inserted
/// last, highest pk) while the three simple points ahead of it still fit.
#[tokio::test]
async fn mvt_tile_drops_the_marginal_geometry_on_the_3857_native_path_when_it_exceeds_the_vertex_budget(
) {
    let path = temp_gpkg_path("tiles_vertex_budget_exceeded_3857");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let write_sink = driver.write_sink().unwrap();
    let tiles = driver.tile_source().expect("advertises TileSource");

    for (id, x, y, name) in [
        (1, 1000.0, 2000.0, "a"),
        (2, 1100.0, 2000.0, "b"),
        (3, 1000.0, 2100.0, "c"),
    ] {
        write_sink
            .apply_with_crs(
                &collection("demo", Some(3857)),
                Mutation {
                    feature_id: id.to_string(),
                    kind: MutationKind::Upsert(point_feature(x, y, name, id)),
                },
                RequestedCrs::Storage,
            )
            .await
            .unwrap();
    }
    write_sink
        .apply_with_crs(
            &collection("demo", Some(3857)),
            Mutation {
                feature_id: "4".to_string(),
                kind: MutationKind::Upsert(dense_linestring_feature(1050.0, 2050.0, 200)),
            },
            RequestedCrs::Storage,
        )
        .await
        .unwrap();

    let coord = tellurion_core::TileCoord { z: 0, x: 0, y: 0 };

    let mut tight = collection("demo", Some(3857));
    tight.settings.tile_vertex_budget = Some(10);
    let truncated = tiles
        .mvt_tile(&tight, coord, None)
        .await
        .unwrap()
        .expect("the three simple points still fit under the budget");
    assert_eq!(
        mvt_feature_ids(&truncated),
        ["1".to_string(), "2".to_string(), "3".to_string()]
            .into_iter()
            .collect(),
        "the 200-vertex dense linestring must be dropped under a budget of 10, leaving only \
         the three simple points whose combined total (3) still fits"
    );

    let mut generous = collection("demo", Some(3857));
    generous.settings.tile_vertex_budget = Some(1_000_000);
    let full = tiles
        .mvt_tile(&generous, coord, None)
        .await
        .unwrap()
        .expect("all four rows fit comfortably under a generous budget");
    assert_eq!(
        mvt_feature_ids(&full),
        [
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "4".to_string()
        ]
        .into_iter()
        .collect(),
        "a generous budget must serve every seeded row, proving the truncation above is \
         genuinely budget-driven and not some other fault dropping the dense row"
    );

    cleanup(&path);
}

/// `#90`: the same drop-the-marginal-geometry behavior as the 3857 test
/// above, but on the 4326 reprojected path — proves the vertex count is
/// taken the same way, and the budget enforced the same way, on both source
/// CRS paths through the shared encoder.
#[tokio::test]
async fn mvt_tile_drops_the_marginal_geometry_on_the_4326_reprojected_path_when_it_exceeds_the_vertex_budget(
) {
    let path = temp_gpkg_path("tiles_vertex_budget_exceeded_4326");
    provision(&path, 4326, true);
    let driver = build_driver(&path);
    let write_sink = driver.write_sink().unwrap();
    let tiles = driver.tile_source().expect("advertises TileSource");

    for (id, lon, lat, name) in [
        (1, 45.0, 30.0, "a"),
        (2, 45.001, 30.0, "b"),
        (3, 45.0, 30.001, "c"),
    ] {
        write_sink
            .apply(
                &collection("demo", Some(4326)),
                Mutation {
                    feature_id: id.to_string(),
                    kind: MutationKind::Upsert(point_feature(lon, lat, name, id)),
                },
            )
            .await
            .unwrap();
    }
    write_sink
        .apply(
            &collection("demo", Some(4326)),
            Mutation {
                feature_id: "4".to_string(),
                kind: MutationKind::Upsert(dense_linestring_feature(45.0005, 30.0005, 200)),
            },
        )
        .await
        .unwrap();

    // The "ne" quadrant tile from `mvt_tile_reprojects_a_4326_stored_
    // table_onto_the_matching_3857_quadrant` above — every seeded coordinate
    // here sits well inside it.
    let coord = tellurion_core::TileCoord { z: 1, x: 1, y: 0 };

    let mut tight = collection("demo", Some(4326));
    tight.settings.tile_vertex_budget = Some(10);
    let truncated = tiles
        .mvt_tile(&tight, coord, None)
        .await
        .unwrap()
        .expect("the three simple points still fit under the budget");
    assert_eq!(
        mvt_feature_ids(&truncated),
        ["1".to_string(), "2".to_string(), "3".to_string()]
            .into_iter()
            .collect(),
        "the 200-vertex dense linestring must be dropped under a budget of 10, leaving only \
         the three simple points whose combined total (3) still fits"
    );

    let mut generous = collection("demo", Some(4326));
    generous.settings.tile_vertex_budget = Some(1_000_000);
    let full = tiles
        .mvt_tile(&generous, coord, None)
        .await
        .unwrap()
        .expect("all four rows fit comfortably under a generous budget");
    assert_eq!(
        mvt_feature_ids(&full),
        [
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "4".to_string()
        ]
        .into_iter()
        .collect(),
        "a generous budget must serve every seeded row on the reprojected path too"
    );

    cleanup(&path);
}

/// `#85`: an allowlisted `tile_properties` set projects each column's real
/// value into the tile's attribute table, verbatim — a `TEXT` column
/// round-trips as an MVT string, an `INTEGER` column as an MVT integer
/// (`sint_value`, the shared encoder's JSON integer mapping). No
/// `tile_properties` at all
/// (the pre-`#85` default) still carries only `id`.
#[tokio::test]
async fn mvt_tile_projects_the_allowlisted_properties_verbatim() {
    let path = temp_gpkg_path("tile_properties");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let mut projected = collection("demo", Some(3857));
    projected.tile_properties = vec!["name".to_string(), "population".to_string()];
    let write_sink = driver.write_sink().unwrap();
    let tiles = driver.tile_source().expect("advertises TileSource");

    write_sink
        .apply_with_crs(
            &projected,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(point_feature(1000.0, 2000.0, "acme", 42)),
            },
            RequestedCrs::Storage,
        )
        .await
        .unwrap();

    let coord = tellurion_core::TileCoord { z: 0, x: 0, y: 0 };
    let bytes = tiles
        .mvt_tile(&projected, coord, None)
        .await
        .unwrap()
        .expect("a world-covering z0 tile carries the written point");

    use geozero::mvt::{Message, Tile};
    let decoded = Tile::decode(bytes.as_ref()).expect("valid MVT protobuf bytes");
    let layer = &decoded.layers[0];
    assert_eq!(layer.features.len(), 1);
    let tags = &layer.features[0].tags;
    let mut attrs = std::collections::HashMap::new();
    for pair in tags.chunks(2) {
        let key = &layer.keys[pair[0] as usize];
        let value = &layer.values[pair[1] as usize];
        attrs.insert(key.as_str(), value.clone());
    }

    assert_eq!(attrs["id"].string_value.as_deref(), Some("1"));
    assert_eq!(attrs["name"].string_value.as_deref(), Some("acme"));
    assert_eq!(attrs["population"].sint_value, Some(42));

    // Pk-only default: the same collection with no `tile_properties` never
    // carries `name`/`population`, only `id` — unchanged from before `#85`.
    let pk_only = collection("demo", Some(3857));
    assert!(pk_only.tile_properties.is_empty());
    let bytes = tiles
        .mvt_tile(&pk_only, coord, None)
        .await
        .unwrap()
        .expect("still carries the written point");
    let decoded = Tile::decode(bytes.as_ref()).expect("valid MVT protobuf bytes");
    assert_eq!(decoded.layers[0].keys, vec!["id".to_string()]);

    cleanup(&path);
}

#[tokio::test]
async fn mvt_tile_rejects_a_non_finite_real_property_by_column_name() {
    let path = temp_gpkg_path("tile_non_finite_property");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let mut collection = collection("demo", Some(3857));
    collection.tile_properties = vec!["population".to_string()];
    driver
        .write_sink()
        .unwrap()
        .apply_with_crs(
            &collection,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(point_feature(0.0, 0.0, "invalid", 1)),
            },
            RequestedCrs::Storage,
        )
        .await
        .unwrap();
    let connection = Connection::open(&path).unwrap();
    // This raw connection does not register the driver's spatial SQL
    // functions. The catch-all id-change R*Tree trigger resolves
    // `ST_IsEmpty` while preparing any UPDATE, even though this statement
    // only changes a scalar column and cannot affect the existing index row.
    connection
        .execute_batch("DROP TRIGGER rtree_demo_geom_update4")
        .unwrap();
    connection
        .execute(
            "UPDATE demo SET population = ?1 WHERE id = 1",
            [f64::INFINITY],
        )
        .unwrap();

    let error = driver
        .tile_source()
        .unwrap()
        .mvt_tile(
            &collection,
            tellurion_core::TileCoord { z: 0, x: 0, y: 0 },
            None,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("population"));
    assert!(error.to_string().contains("not finite"));
    cleanup(&path);
}

#[tokio::test]
async fn mvt_tile_preserves_the_invalid_category_for_a_dynamic_blob_property() {
    let path = temp_gpkg_path("tile_blob_property");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let mut collection = collection("demo", Some(3857));
    collection.tile_properties = vec!["population".to_string()];
    driver
        .write_sink()
        .unwrap()
        .apply_with_crs(
            &collection,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(point_feature(0.0, 0.0, "blob", 1)),
            },
            RequestedCrs::Storage,
        )
        .await
        .unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("DROP TRIGGER rtree_demo_geom_update4")
        .unwrap();
    connection
        .execute(
            "UPDATE demo SET population = ?1 WHERE id = 1",
            [rusqlite::types::Value::Blob(vec![1, 2, 3])],
        )
        .unwrap();

    let error = driver
        .tile_source()
        .unwrap()
        .mvt_tile(
            &collection,
            tellurion_core::TileCoord { z: 0, x: 0, y: 0 },
            None,
        )
        .await
        .unwrap_err();
    let CoreError::Invalid(message) = error else {
        panic!("a dynamic BLOB scalar must remain an Invalid error")
    };
    assert!(message.contains("population"));
    assert!(message.contains("BLOB"));

    cleanup(&path);
}

#[tokio::test]
async fn mvt_tile_preserves_the_invalid_category_for_an_unsupported_exact_filter_row() {
    let path = temp_gpkg_path("tile_exact_filter_z_row");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    driver
        .write_sink()
        .unwrap()
        .apply_with_crs(
            &collection,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(point_feature(0.0, 0.0, "z", 1)),
            },
            RequestedCrs::Storage,
        )
        .await
        .unwrap();

    let mut blob = stored_geometry_blob(&path, 1);
    assert_eq!(blob[40], 1, "fixture WKB must be little-endian");
    assert_eq!(&blob[41..45], &1_u32.to_le_bytes());
    blob[41..45].copy_from_slice(&1001_u32.to_le_bytes());
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "DROP TRIGGER rtree_demo_geom_update1;
             DROP TRIGGER rtree_demo_geom_update2;
             DROP TRIGGER rtree_demo_geom_update4;",
        )
        .unwrap();
    connection
        .execute("UPDATE demo SET geom = ?1 WHERE id = 1", [blob])
        .unwrap();

    let filter = tellurion_core::filter::parse_text("S_INTERSECTS(geom, POINT(0 0))").unwrap();
    let error = driver
        .tile_source()
        .unwrap()
        .mvt_tile(
            &collection,
            tellurion_core::TileCoord { z: 0, x: 0, y: 0 },
            Some(&filter),
        )
        .await
        .unwrap_err();
    let CoreError::Invalid(message) = error else {
        panic!("an unsupported exact-filter row must remain an Invalid error")
    };
    assert!(message.contains("Z/M"));

    cleanup(&path);
}

/// Registers `geom_z6` on the `demo` table as a second, operator-produced
/// geometry column (`#104`): a real column plus its `gpkg_geometry_columns`
/// row, and deliberately **no** `rtree_demo_geom_z6` index — a GeoPackage
/// R*Tree is an optional per-column extension, and the tiles lane must keep
/// pruning against the base column's index rather than assume the operator
/// built one for every variant (`sql::build_tile_plan`'s own doc).
fn provision_variant_column(path: &Path, srid: i32) {
    let conn = Connection::open(path).expect("opens the provisioned fixture");
    conn.execute_batch(&format!(
        r#"
ALTER TABLE "demo" ADD COLUMN "geom_z6" POINT;
INSERT INTO gpkg_geometry_columns (table_name, column_name, geometry_type_name, srs_id, z, m)
    VALUES ('demo', 'geom_z6', 'POINT', {srid}, 0, 0);
"#
    ))
    .expect("adds the pre-generalized variant column");
}

/// A fixture connection that can actually run DML against `demo`. The
/// R*Tree maintenance triggers `provision` installs call the five scalar
/// functions the driver registers on every connection it opens
/// (`ST_MinX`/`ST_MaxX`/`ST_MinY`/`ST_MaxY`/`ST_IsEmpty`, see
/// `src/functions.rs`), and SQLite resolves a trigger body's function names
/// when the triggering statement is *prepared* — not lazily, when a `WHEN`
/// clause turns out false — so a plain `Connection::open` cannot even
/// compile an `UPDATE demo ...`. These are the same five, read off the same
/// GPB header this file's own [`gpkg_xy_envelope`] already parses.
fn open_writable(path: &Path) -> Connection {
    use rusqlite::functions::FunctionFlags;
    let conn = Connection::open(path).expect("opens the provisioned fixture");
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    for (name, index) in [
        ("ST_MinX", 0),
        ("ST_MinY", 1),
        ("ST_MaxX", 2),
        ("ST_MaxY", 3),
    ] {
        conn.create_scalar_function(name, 1, flags, move |ctx| {
            let blob = ctx.get_raw(0);
            Ok(match blob {
                rusqlite::types::ValueRef::Null => None,
                other => Some(gpkg_xy_envelope(other.as_blob()?)[index]),
            })
        })
        .expect("registers the fixture's own envelope function");
    }
    conn.create_scalar_function("ST_IsEmpty", 1, flags, move |ctx| {
        let blob = ctx.get_raw(0);
        Ok(match blob {
            rusqlite::types::ValueRef::Null => None,
            // GPB header flags, bit 4: the spec's own "empty geometry" flag.
            other => Some(i64::from((other.as_blob()?[3] >> 4) & 1)),
        })
    })
    .expect("registers the fixture's own emptiness predicate");
    conn
}

fn linestring_feature(coords: &[[f64; 2]]) -> serde_json::Value {
    serde_json::json!({
        "type": "Feature",
        "geometry": {"type": "LineString", "coordinates": coords},
        "properties": {"name": "variant-fixture", "population": 0}
    })
}

/// The single decoded feature's GeoJSON geometry type in a one-layer tile —
/// the cheapest honest way for a test to ask "which column did this tile
/// read", since a variant column holds a genuinely different geometry from
/// the base one.
fn sole_feature_geometry_type(bytes: &[u8]) -> String {
    use geozero::mvt::{Message, Tile};
    use geozero::ProcessToJson;
    let mut decoded = Tile::decode(bytes).expect("valid MVT protobuf bytes");
    assert_eq!(decoded.layers.len(), 1);
    let mut layer = decoded.layers.remove(0);
    let geojson: serde_json::Value = serde_json::from_str(&layer.to_json().unwrap()).unwrap();
    let features = geojson["features"].as_array().unwrap();
    assert_eq!(features.len(), 1, "fixture writes exactly one feature");
    features[0]["geometry"]["type"]
        .as_str()
        .unwrap()
        .to_string()
}

/// `#104`/`#200`: the tiles lane reads the declared `geometry_variants`
/// column for a zoom the variant covers, and the base geometry column for
/// every zoom outside it. The fixture makes the two columns tell themselves
/// apart by shape rather than by position: the base column holds a
/// `LineString`, the variant column a `Point` sitting inside that
/// LineString's own envelope (so the base column's R*Tree — the only one
/// this fixture provisions — keeps the row as a candidate for both tiles).
///
/// The same test pins the "no variants declared changes nothing" half of the
/// acceptance: the zoom-0 tile a variant-declaring collection serves is
/// byte-for-byte the tile the identical collection with no variants serves.
#[tokio::test]
async fn mvt_tile_reads_the_declared_geometry_variant_only_inside_its_zoom_range() {
    let path = temp_gpkg_path("tiles_geometry_variants");
    provision(&path, 3857, true);
    provision_variant_column(&path, 3857);
    let driver = build_driver(&path);
    let base_only = collection("demo", Some(3857));
    let write_sink = driver.write_sink().unwrap();
    let tiles = driver.tile_source().expect("advertises TileSource");

    // A point first, purely to capture a well-formed GPB blob this driver
    // wrote itself — hand-assembling one here would test the fixture's own
    // encoder, not the driver's.
    write_sink
        .apply_with_crs(
            &base_only,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(point_feature(1_000_000.0, 1_000_000.0, "pre", 0)),
            },
            RequestedCrs::Storage,
        )
        .await
        .unwrap();
    let point_blob = stored_geometry_blob(&path, 1);

    // The same row's base geometry becomes a LineString whose envelope
    // contains that point; the captured point blob becomes the variant.
    write_sink
        .apply_with_crs(
            &base_only,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(linestring_feature(&[
                    [500_000.0, 500_000.0],
                    [2_000_000.0, 2_000_000.0],
                    [3_000_000.0, 1_500_000.0],
                ])),
            },
            RequestedCrs::Storage,
        )
        .await
        .unwrap();
    open_writable(&path)
        .execute(
            "UPDATE demo SET geom_z6 = ?1 WHERE id = 1",
            rusqlite::params![point_blob],
        )
        .expect("loads the operator-produced variant column");

    let mut with_variant = collection("demo", Some(3857));
    with_variant.geometry_variants = vec![tellurion_core::GeometryVariantDecl {
        column: "geom_z6".to_string(),
        minzoom: 1,
        maxzoom: 1,
    }];

    // z1, inside the declared range: the variant column's Point.
    let inside = tellurion_core::TileCoord { z: 1, x: 1, y: 0 };
    let bytes = tiles
        .mvt_tile(&with_variant, inside, None)
        .await
        .unwrap()
        .expect("the variant column's geometry is inside this tile");
    assert_eq!(
        sole_feature_geometry_type(&bytes),
        "Point",
        "a zoom the variant covers must read the variant column"
    );

    // z0, outside it: the base column's LineString.
    let outside = tellurion_core::TileCoord { z: 0, x: 0, y: 0 };
    let bytes_outside = tiles
        .mvt_tile(&with_variant, outside, None)
        .await
        .unwrap()
        .expect("a world-covering z0 tile carries the base geometry");
    assert_eq!(
        sole_feature_geometry_type(&bytes_outside),
        "LineString",
        "a zoom no variant covers must fall back to the base column"
    );

    // And the same tile from a collection that declares no variants at all
    // is byte-for-byte identical — the fallback path is the pre-`#200` path,
    // not a re-derivation of it.
    let bytes_base_only = tiles
        .mvt_tile(&base_only, outside, None)
        .await
        .unwrap()
        .expect("a world-covering z0 tile carries the base geometry");
    assert_eq!(
        bytes_outside, bytes_base_only,
        "declaring a variant must not change a tile outside its zoom range"
    );
    let bytes_base_only_z1 = tiles
        .mvt_tile(&base_only, inside, None)
        .await
        .unwrap()
        .expect("the base LineString also falls inside the z1 quadrant tile");
    assert_eq!(
        sole_feature_geometry_type(&bytes_base_only_z1),
        "LineString",
        "with no variants declared, every zoom reads the base column"
    );

    cleanup(&path);
}

/// The tile lane exact-tests too: a candidate that bbox-overlaps the needle
/// but doesn't actually intersect it must be omitted from the tile, not
/// rendered as a false positive.
#[tokio::test]
async fn mvt_tile_omits_a_bbox_candidate_that_fails_the_exact_intersects_test() {
    let path = temp_gpkg_path("tiles_intersects");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let write_sink = driver.write_sink().unwrap();
    let tiles = driver.tile_source().unwrap();

    for (id, x, y) in [(1, 1.0, 9.0), (2, 2.0, 2.0)] {
        write_sink
            .apply_with_crs(
                &collection,
                Mutation {
                    feature_id: id.to_string(),
                    kind: MutationKind::Upsert(point_feature(x, y, &format!("f{id}"), id)),
                },
                RequestedCrs::Storage,
            )
            .await
            .unwrap();
    }

    let filter =
        tellurion_core::filter::parse_text("S_INTERSECTS(geom, LINESTRING(0 0, 10 10))").unwrap();
    let coord = tellurion_core::TileCoord { z: 0, x: 0, y: 0 };
    let bytes = tiles
        .mvt_tile(&collection, coord, Some(&filter))
        .await
        .unwrap()
        .expect("the on-diagonal point still renders");

    use geozero::mvt::{Message, Tile};
    let decoded = Tile::decode(bytes.as_ref()).unwrap();
    assert_eq!(decoded.layers[0].features.len(), 1);

    cleanup(&path);
}

/// `item()` under an active `S_INTERSECTS` filter: a row that exists but
/// fails the exact test comes back `Ok(None)`, indistinguishable from a
/// missing id — the same contract this driver's plain attribute filters
/// already honor for `item()`.
#[tokio::test]
async fn item_lookup_under_an_intersects_filter_excludes_like_a_missing_id() {
    let path = temp_gpkg_path("item_intersects");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let write_sink = driver.write_sink().unwrap();
    let features = driver.feature_source().unwrap();

    write_sink
        .apply_with_crs(
            &collection,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(point_feature(1.0, 9.0, "off-diagonal", 1)),
            },
            RequestedCrs::Storage,
        )
        .await
        .unwrap();
    write_sink
        .apply_with_crs(
            &collection,
            Mutation {
                feature_id: "2".to_string(),
                kind: MutationKind::Upsert(point_feature(2.0, 2.0, "on-diagonal", 1)),
            },
            RequestedCrs::Storage,
        )
        .await
        .unwrap();

    let filter =
        tellurion_core::filter::parse_text("S_INTERSECTS(geom, LINESTRING(0 0, 10 10))").unwrap();

    let excluded = features
        .item(&collection, "1", Some(&filter))
        .await
        .unwrap();
    let missing = features.item(&collection, "999", None).await.unwrap();
    assert_eq!(
        excluded, missing,
        "an excluded row must look exactly like a missing one"
    );

    let matched = features
        .item(&collection, "2", Some(&filter))
        .await
        .unwrap()
        .expect("the on-diagonal row passes the exact test");
    assert_eq!(matched["properties"]["name"], "on-diagonal");

    cleanup(&path);
}

#[test]
fn build_refuses_a_missing_file() {
    let path = temp_gpkg_path("missing");
    let env_var = "TELLURION_GEOPACKAGE_CONTRACT_TEST_MISSING_FILE";
    std::env::set_var(env_var, &path);
    let decl = StorageDecl {
        id: "main".to_string(),
        driver: "geopackage".to_string(),
        url_env: env_var.to_string(),
        pool_size: None,
    };
    let result = GeopackageDriverFactory::new().build(&decl);
    assert!(matches!(result, Err(CoreError::Config(_))));
}

#[test]
fn build_refuses_a_plain_sqlite_file_with_no_gpkg_contents_table() {
    let path = temp_gpkg_path("not_a_gpkg");
    Connection::open(&path)
        .unwrap()
        .execute("CREATE TABLE not_a_gpkg (id INTEGER)", [])
        .unwrap();
    let env_var = "TELLURION_GEOPACKAGE_CONTRACT_TEST_NOT_A_GPKG";
    std::env::set_var(env_var, &path);
    let decl = StorageDecl {
        id: "main".to_string(),
        driver: "geopackage".to_string(),
        url_env: env_var.to_string(),
        pool_size: None,
    };
    let result = GeopackageDriverFactory::new().build(&decl);
    assert!(matches!(result, Err(CoreError::Config(_))));

    cleanup(&path);
}

// -- `WriteSink::apply_batch` (`#114`) ---------------------------------------

/// A chunk of entirely clean upserts commits every row and every outbox
/// obligation, each reporting `Applied` with a distinct sequence, in one
/// SQLite transaction on the single writer connection.
#[tokio::test]
async fn apply_batch_applies_every_clean_item_and_reports_its_sequence() {
    let path = temp_gpkg_path("batch_clean");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let write_sink = driver.write_sink().expect("advertises WriteSink");

    let mutations = vec![
        Mutation {
            feature_id: "1".to_string(),
            kind: MutationKind::Upsert(point_feature(10.0, 20.0, "alpha", 100)),
        },
        Mutation {
            feature_id: "2".to_string(),
            kind: MutationKind::Upsert(point_feature(11.0, 21.0, "bravo", 200)),
        },
        Mutation {
            feature_id: "3".to_string(),
            kind: MutationKind::Upsert(point_feature(12.0, 22.0, "charlie", 300)),
        },
    ];
    let results = write_sink
        .apply_batch(&collection, mutations, RequestedCrs::Omitted, false)
        .await
        .expect("batch apply succeeds");

    assert_eq!(results.len(), 3);
    for (index, result) in results.iter().enumerate() {
        assert_eq!(result.feature_id, (index + 1).to_string());
        assert!(matches!(result.outcome, BatchItemOutcome::Applied(_)));
    }

    let features = driver.feature_source().expect("advertises FeatureSource");
    for id in ["1", "2", "3"] {
        assert!(features
            .item(&collection, id, None)
            .await
            .unwrap()
            .is_some());
    }

    cleanup(&path);
}

/// A chunk mixing clean rows with a deliberately dirty one (a feature id
/// that doesn't parse as this collection's mandatory `INTEGER` primary key)
/// commits every clean row despite the dirty row's own savepoint rolling
/// back — proving the chunk's atomicity is per-item, not all-or-nothing —
/// and reports the dirty row's refusal as the identical
/// `tellurion_core::Error` variant `WriteSink::apply` gives for the same bad
/// input outside a batch at all.
#[tokio::test]
async fn apply_batch_refuses_a_dirty_row_while_its_clean_siblings_still_commit() {
    let path = temp_gpkg_path("batch_dirty");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let write_sink = driver.write_sink().expect("advertises WriteSink");

    let dirty = Mutation {
        feature_id: "not-an-integer".to_string(),
        kind: MutationKind::Upsert(point_feature(0.0, 0.0, "dirty", 0)),
    };
    let single_item_error = write_sink
        .apply(&collection, dirty.clone())
        .await
        .expect_err("a non-integer id must be refused by the single-item lane too");

    let mutations = vec![
        Mutation {
            feature_id: "1".to_string(),
            kind: MutationKind::Upsert(point_feature(10.0, 20.0, "alpha", 100)),
        },
        dirty,
        Mutation {
            feature_id: "2".to_string(),
            kind: MutationKind::Upsert(point_feature(11.0, 21.0, "bravo", 200)),
        },
    ];
    let results = write_sink
        .apply_batch(&collection, mutations, RequestedCrs::Omitted, false)
        .await
        .expect("batch apply itself succeeds even though one item is refused");

    assert_eq!(results.len(), 3);
    assert!(matches!(results[0].outcome, BatchItemOutcome::Applied(_)));
    assert!(matches!(results[2].outcome, BatchItemOutcome::Applied(_)));
    match &results[1].outcome {
        BatchItemOutcome::Refused(err) => {
            assert_eq!(
                std::mem::discriminant(err),
                std::mem::discriminant(&single_item_error),
                "the batch lane's refusal must name the same problem the \
                 single-item lane gives for identical bad input"
            );
        }
        other => panic!("expected the dirty row to be refused, got {other:?}"),
    }

    let features = driver.feature_source().expect("advertises FeatureSource");
    assert!(features
        .item(&collection, "1", None)
        .await
        .unwrap()
        .is_some());
    assert!(features
        .item(&collection, "2", None)
        .await
        .unwrap()
        .is_some());

    cleanup(&path);
}

/// `strict: true` stops attempting further mutations the instant one is
/// refused — the result `Vec` is shorter than the input, and nothing after
/// the refusal was ever attempted.
#[tokio::test]
async fn apply_batch_strict_mode_stops_at_the_first_refusal() {
    let path = temp_gpkg_path("batch_strict");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let write_sink = driver.write_sink().expect("advertises WriteSink");

    let mutations = vec![
        Mutation {
            feature_id: "1".to_string(),
            kind: MutationKind::Upsert(point_feature(10.0, 20.0, "alpha", 100)),
        },
        Mutation {
            feature_id: "not-an-integer".to_string(),
            kind: MutationKind::Upsert(point_feature(0.0, 0.0, "dirty", 0)),
        },
        Mutation {
            feature_id: "2".to_string(),
            kind: MutationKind::Upsert(point_feature(11.0, 21.0, "bravo", 200)),
        },
    ];
    let results = write_sink
        .apply_batch(&collection, mutations, RequestedCrs::Omitted, true)
        .await
        .expect("batch apply itself succeeds");

    assert_eq!(
        results.len(),
        2,
        "strict mode must stop after the refused item, never attempting the remainder"
    );
    assert!(matches!(results[0].outcome, BatchItemOutcome::Applied(_)));
    assert!(matches!(results[1].outcome, BatchItemOutcome::Refused(_)));

    let features = driver.feature_source().expect("advertises FeatureSource");
    assert!(features
        .item(&collection, "1", None)
        .await
        .unwrap()
        .is_some());
    assert_eq!(features.item(&collection, "2", None).await.unwrap(), None);

    cleanup(&path);
}

/// `#87`: a collection declaring a non-`Integer` `id_type` refuses the WHOLE
/// batch up front, the same unconditional guard `apply` applies per single
/// item — never a per-item refusal for something the format can't support
/// for ANY item.
#[tokio::test]
async fn apply_batch_refuses_the_whole_call_for_a_non_integer_id_type_collection() {
    let path = temp_gpkg_path("batch_non_integer_id");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let collection = collection_uuid("demo", Some(3857));
    let write_sink = driver.write_sink().expect("advertises WriteSink");

    let mutations = vec![Mutation {
        feature_id: "1".to_string(),
        kind: MutationKind::Upsert(point_feature(10.0, 20.0, "alpha", 100)),
    }];
    let result = write_sink
        .apply_batch(&collection, mutations, RequestedCrs::Omitted, false)
        .await;
    assert!(matches!(result, Err(CoreError::Config(_))));

    cleanup(&path);
}

#[tokio::test]
async fn apply_batch_rolls_back_the_chunk_when_outbox_infrastructure_is_missing() {
    let path = temp_gpkg_path("batch_missing_outbox");
    provision(&path, 3857, false);
    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let write_sink = driver.write_sink().expect("advertises WriteSink");

    let result = write_sink
        .apply_batch(
            &collection,
            vec![Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(point_feature(10.0, 20.0, "alpha", 100)),
            }],
            RequestedCrs::Omitted,
            false,
        )
        .await;
    assert!(matches!(result, Err(CoreError::Config(_))));

    let features = driver.feature_source().expect("advertises FeatureSource");
    assert_eq!(features.item(&collection, "1", None).await.unwrap(), None);
    cleanup(&path);
}

#[tokio::test]
async fn apply_batch_rolls_back_when_the_outbox_schema_rejects_an_insert() {
    let path = temp_gpkg_path("batch_outbox_constraint");
    provision(&path, 3857, true);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        r#"
DROP TABLE "demo_outbox";
CREATE TABLE "demo_outbox" (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    feature_id TEXT NOT NULL CHECK (feature_id <> '2'),
    kind TEXT NOT NULL CHECK (kind IN ('upsert', 'delete')),
    payload TEXT,
    committed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    extent_crs84 TEXT
);
"#,
    )
    .unwrap();
    drop(conn);

    let driver = build_driver(&path);
    let collection = collection("demo", Some(3857));
    let write_sink = driver.write_sink().expect("advertises WriteSink");
    let result = write_sink
        .apply_batch(
            &collection,
            vec![
                Mutation {
                    feature_id: "1".to_string(),
                    kind: MutationKind::Upsert(point_feature(10.0, 20.0, "alpha", 100)),
                },
                Mutation {
                    feature_id: "2".to_string(),
                    kind: MutationKind::Upsert(point_feature(11.0, 21.0, "beta", 200)),
                },
            ],
            RequestedCrs::Omitted,
            false,
        )
        .await;
    assert!(
        result.is_err(),
        "outbox constraints are chunk infrastructure"
    );

    let features = driver.feature_source().expect("advertises FeatureSource");
    assert_eq!(features.item(&collection, "1", None).await.unwrap(), None);
    assert_eq!(features.item(&collection, "2", None).await.unwrap(), None);
    let conn = Connection::open(&path).unwrap();
    let outbox_count: i64 = conn
        .query_row("SELECT count(*) FROM demo_outbox", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        outbox_count, 0,
        "the failed chunk must leave no outbox rows"
    );
    cleanup(&path);
}

// ---------------------------------------------------------------------------
// `#142` / `#141`: the CRS84 extent this driver records on every obligation.
// ---------------------------------------------------------------------------

/// `#142`: a write into a PROJECTED collection records its extent in CRS84 —
/// degrees — not in the metres it stored and not in the metres the request
/// body carried.
///
/// The request declares `Content-Crs: EPSG:3857`, so the outbox payload holds
/// metres verbatim; a consumer reading THOSE as CRS84 is exactly the defect.
/// What travels on the obligation instead is the extent this driver's own
/// inverse Web Mercator produced from what it actually stored.
#[tokio::test]
async fn a_projected_write_records_its_extent_in_crs84_degrees() {
    let path = temp_gpkg_path("invalidation_extent_projected");
    provision(&path, 3857, true);
    let driver = build_driver(&path);
    let write = driver.write_sink().expect("write sink");
    let outbox = driver.outbox_source().expect("outbox source");
    let decl = collection("demo", Some(3857));

    // 12.49E / 41.90N, in EPSG:3857 metres.
    write
        .apply_with_crs(
            &decl,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(point_feature(1_390_330.0, 5_146_501.0, "rome", 1)),
            },
            RequestedCrs::Storage,
        )
        .await
        .expect("a storage-CRS upsert succeeds");

    let obligations = outbox
        .read_after(&decl, Sequence(0), 10)
        .await
        .expect("reads the obligation back");
    assert_eq!(obligations.len(), 1);
    match obligations[0].extent {
        ObligationExtent::Crs84 { prior, current } => {
            assert_eq!(prior, None, "there was no prior row");
            let current = current.expect("a point upsert has a current extent");
            // The exact inverse of the metres written above, computed by
            // hand from the spherical Web Mercator formulas rather than by
            // calling the code under test.
            assert!(
                (current[0] - 12.489_546_9).abs() < 1.0e-6
                    && (current[1] - 41.903_271_6).abs() < 1.0e-6,
                "the recorded extent must be CRS84 degrees, got {current:?}"
            );
        }
        other => panic!("expected a recorded CRS84 extent, got {other:?}"),
    }

    // And the delete records where it WAS (`#141`) — from the row it removed,
    // not from a payload it does not have.
    write
        .apply(
            &decl,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Delete,
            },
        )
        .await
        .expect("delete succeeds");
    let obligations = outbox
        .read_after(&decl, Sequence(1), 10)
        .await
        .expect("reads the delete obligation back");
    assert_eq!(obligations.len(), 1);
    match obligations[0].extent {
        ObligationExtent::Crs84 { prior, current } => {
            assert_eq!(current, None, "a delete leaves nothing behind");
            let prior = prior.expect("a delete must record where the feature was");
            assert!(
                (prior[0] - 12.489_546_9).abs() < 1.0e-6
                    && (prior[1] - 41.903_271_6).abs() < 1.0e-6,
                "the recorded prior extent must be CRS84 degrees, got {prior:?}"
            );
        }
        other => panic!("expected a recorded CRS84 extent, got {other:?}"),
    }

    cleanup(&path);
}

/// A storage CRS this driver cannot express in CRS84 records NOTHING rather
/// than something wrong: `ObligationExtent::Unrecorded`, which the
/// invalidation consumer reads as UNKNOWN and degrades conservatively on.
///
/// EPSG:2154 is the fixture the existing `write_refuses_a_storage_srid_outside
/// _the_embedded_transform_contract` test already uses — a write into it is
/// only reachable at all by declaring the storage CRS, which is precisely the
/// case where nothing can be inferred about degrees.
#[tokio::test]
async fn a_storage_crs_this_driver_cannot_express_records_no_extent_at_all() {
    let path = temp_gpkg_path("invalidation_extent_unmappable");
    provision(&path, 2154, true);
    let driver = build_driver(&path);
    let write = driver.write_sink().expect("write sink");
    let outbox = driver.outbox_source().expect("outbox source");
    let decl = collection("demo", Some(2154));

    write
        .apply_with_crs(
            &decl,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(point_feature(652_000.0, 6_862_000.0, "paris", 1)),
            },
            RequestedCrs::Storage,
        )
        .await
        .expect("a declared-storage-CRS write is an identity write for any SRID");

    let obligations = outbox
        .read_after(&decl, Sequence(0), 10)
        .await
        .expect("reads the obligation back");
    assert_eq!(obligations.len(), 1);
    assert_eq!(
        obligations[0].extent,
        ObligationExtent::Unrecorded,
        "a CRS this driver cannot express in CRS84 must record nothing, not a guess"
    );

    cleanup(&path);
}

/// Campaign rule 4, executed on this driver: ingest owns all DDL, and a write
/// against an outbox table provisioned before `#141`/`#142` is refused BY
/// NAME — never quietly written without the extent the invalidation lane
/// depends on.
#[tokio::test]
async fn a_write_against_a_pre_extent_outbox_is_refused_by_name() {
    let path = temp_gpkg_path("invalidation_legacy_outbox");
    provision(&path, 4326, false);
    // The outbox exactly as `tellurion-ingest geopackage create-tables` used
    // to emit it, before the extent column existed.
    Connection::open(&path)
        .unwrap()
        .execute_batch(
            r#"
CREATE TABLE "demo_outbox" (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    feature_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('upsert', 'delete')),
    payload TEXT,
    committed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
"#,
        )
        .expect("provisions a pre-#141 outbox table");

    let driver = build_driver(&path);
    let write = driver.write_sink().expect("write sink");
    let decl = collection("demo", Some(4326));

    let error = write
        .apply(
            &decl,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(point_feature(12.49, 41.90, "rome", 1)),
            },
        )
        .await
        .expect_err("a write against a pre-#141 outbox must be refused");
    let message = error.to_string();
    assert!(
        message.contains("extent_crs84")
            && message.contains("tellurion-ingest geopackage create-tables"),
        "the refusal must name the column and the command that supplies it; got: {message}"
    );

    cleanup(&path);
}
