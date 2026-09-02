//! End-to-end test against the real `tellurion` binary: spawns the compiled
//! process on an OS-assigned ephemeral port (`PORT=0`), parses the actual
//! bound address out of its startup log line, then drives it over plain
//! HTTP exactly the way an external client would — landing page,
//! `/conformance`, a collection, its items, and one tile. Skipped cleanly
//! (not failed) unless `DATABASE_URL` is set, matching every other
//! database-backed test in this workspace.
//!
//! Every config this file writes declares `driver: postgis`, so the whole
//! file is gated on that feature — same idiom `pmtiles_binary.rs` and
//! `flatgeobuf_binary.rs` use for their own driver. Without this, a build
//! with `--no-default-features --features pmtiles` (or `flatgeobuf`) still
//! compiles these tests, and with `DATABASE_URL` set they'd spawn the real
//! binary against a config naming a driver that isn't registered — a boot
//! failure, not a skip.

#![cfg(feature = "postgis")]

mod common;

use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio_postgres::NoTls;

use common::{
    http_get, http_request_with_headers, http_write_request, parse_listening_addr, spawn_server,
};

/// How long [`real_binary_exits_cleanly_and_promptly_on_sigterm`] waits for
/// the real binary to actually exit once SIGTERM has been delivered. Same
/// load sensitivity as `common::startup_timeout`, on the way out instead of
/// the way in — the wait below is already a poll loop, not a sleep, but its
/// ceiling still needs to stay well clear of ordinary host contention.
/// Override with `TELLURION_TEST_SHUTDOWN_TIMEOUT_SECS`.
fn shutdown_timeout() -> Duration {
    common::env_duration_secs("TELLURION_TEST_SHUTDOWN_TIMEOUT_SECS")
        .unwrap_or(Duration::from_secs(30))
}

/// Serializes the server-spawning tests in this file. Each one boots the real
/// binary, opens a connection pool, and drives it over HTTP with a 5s request
/// timeout; running four of those on parallel test threads is what made
/// `real_binary_rejects_a_transposed_row_col_request` flake under the full
/// workspace suite. The processes never share tables or ports, so one at a
/// time loses nothing but the contention.
static SERVER_TEST_LOCK: Mutex<()> = Mutex::new(());

/// A panicking test poisons the lock; later tests should still run rather
/// than cascade the failure.
fn serialize_server_test() -> std::sync::MutexGuard<'static, ()> {
    SERVER_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

async fn seed_table(database_url: &str, table: &str) {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .expect("connects to the test database");
    tokio::spawn(async move {
        let _ = connection.await;
    });

    client
        .batch_execute(&format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326) NOT NULL,
                 observed_at timestamptz,
                 name text
             );
             INSERT INTO {table} (geom, observed_at, name) VALUES
                 (ST_SetSRID(ST_MakePoint(10, 45), 4326), '2020-01-01T00:00:00Z', 'a'),
                 (ST_SetSRID(ST_MakePoint(11, 46), 4326), '2020-06-01T00:00:00Z', 'b'),
                 (ST_SetSRID(ST_MakePoint(12, 47), 4326), '2021-01-01T00:00:00Z', 'c');
             ANALYZE {table};"
        ))
        .await
        .expect("seeds the test table");
}

/// Seeds a single point at an off-diagonal WebMercatorQuad tile coordinate
/// (z=4, tileRow=7, tileCol=0 — verified via `ST_TileEnvelope`/`ST_Contains`
/// against a live PostGIS instance) for
/// [`real_binary_rejects_a_transposed_row_col_request`], which asserts the
/// route only serves this point at the correct `{tileMatrix}/{tileRow}/{tileCol}`
/// coordinates, not the transposed ones.
async fn seed_off_diagonal_point(database_url: &str, table: &str) {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .expect("connects to the test database");
    tokio::spawn(async move {
        let _ = connection.await;
    });

    client
        .batch_execute(&format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326) NOT NULL
             );
             INSERT INTO {table} (geom) VALUES
                 (ST_SetSRID(ST_MakePoint(-170, 10), 4326));
             ANALYZE {table};"
        ))
        .await
        .expect("seeds the off-diagonal test table");
}

/// Seeds an empty, writable table for
/// [`real_binary_writes_and_reads_back_an_item_over_http`]: the data table
/// plus its `<table>_outbox` obligation log, matching
/// `tellurion-postgis`'s own `write_sql::outbox_table_name` convention (the
/// two crates hand-keep this in sync rather than sharing a constant — see
/// that module's doc). No rows: the test itself is what puts one there,
/// through the real write endpoint.
async fn seed_write_table(database_url: &str, table: &str) {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .expect("connects to the test database");
    tokio::spawn(async move {
        let _ = connection.await;
    });

    client
        .batch_execute(&format!(
            "DROP TABLE IF EXISTS {table};
             DROP TABLE IF EXISTS {table}_outbox;
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326),
                 name text
             );
             CREATE TABLE {table}_outbox (
                 sequence bigserial PRIMARY KEY,
                 feature_id text NOT NULL,
                 kind text NOT NULL CHECK (kind IN ('upsert', 'delete')),
                 payload jsonb,
                 committed_at timestamptz NOT NULL DEFAULT now(),
                 extent_crs84 jsonb
             );"
        ))
        .await
        .expect("seeds the empty writable table and its outbox");
}

async fn seed_patch_table_3857(database_url: &str, table: &str) {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .expect("connects to the test database");
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
        .batch_execute(&format!(
            "DROP TABLE IF EXISTS {table};
             DROP TABLE IF EXISTS {table}_outbox;
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 3857),
                 name text,
                 count integer
             );
             CREATE TABLE {table}_outbox (
                 sequence bigserial PRIMARY KEY,
                 feature_id text NOT NULL,
                 kind text NOT NULL CHECK (kind IN ('upsert', 'delete')),
                 payload jsonb,
                 committed_at timestamptz NOT NULL DEFAULT now(),
                 extent_crs84 jsonb
             );
             INSERT INTO {table} (geom, name, count)
             VALUES (ST_SetSRID(ST_MakePoint(500000, 6000000), 3857), 'alpha', 7);"
        ))
        .await
        .expect("seeds the 3857 PATCH table");
}

/// Same fixture as [`seed_write_table`], but the pk is a real `uuid` column
/// with a server-side default (`gen_random_uuid()`, built into PostgreSQL
/// core since v13) — the `#87` counterpart of the `bigserial`-pk fixture,
/// for [`real_binary_round_trips_over_a_server_assigned_uuid_id_over_http`].
async fn seed_write_table_uuid(database_url: &str, table: &str) {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .expect("connects to the test database");
    tokio::spawn(async move {
        let _ = connection.await;
    });

    client
        .batch_execute(&format!(
            "DROP TABLE IF EXISTS {table};
             DROP TABLE IF EXISTS {table}_outbox;
             CREATE TABLE {table} (
                 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
                 geom geometry(Point, 4326),
                 name text
             );
             CREATE TABLE {table}_outbox (
                 sequence bigserial PRIMARY KEY,
                 feature_id text NOT NULL,
                 kind text NOT NULL CHECK (kind IN ('upsert', 'delete')),
                 payload jsonb,
                 committed_at timestamptz NOT NULL DEFAULT now(),
                 extent_crs84 jsonb
             );"
        ))
        .await
        .expect("seeds the empty uuid-pk writable table and its outbox");
}

/// Same fixture as [`seed_write_table`], but the pk is a `text` column with
/// deliberately NO server-side default (`#94`) — the pk is caller-supplied,
/// so there is nothing for the database to mint. The `#87` counterpart of
/// [`seed_write_table_uuid`], for
/// [`real_binary_round_trips_over_a_caller_supplied_text_id_over_http`].
async fn seed_write_table_text(database_url: &str, table: &str) {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .expect("connects to the test database");
    tokio::spawn(async move {
        let _ = connection.await;
    });

    client
        .batch_execute(&format!(
            "DROP TABLE IF EXISTS {table};
             DROP TABLE IF EXISTS {table}_outbox;
             CREATE TABLE {table} (
                 id text PRIMARY KEY,
                 geom geometry(Point, 4326),
                 name text
             );
             CREATE TABLE {table}_outbox (
                 sequence bigserial PRIMARY KEY,
                 feature_id text NOT NULL,
                 kind text NOT NULL CHECK (kind IN ('upsert', 'delete')),
                 payload jsonb,
                 committed_at timestamptz NOT NULL DEFAULT now(),
                 extent_crs84 jsonb
             );"
        ))
        .await
        .expect("seeds the empty text-pk writable table and its outbox");
}

fn write_temp_config(table: &str) -> PathBuf {
    let mut path = common::unique_temp_path("tellurion-server-binary-test");
    path.set_extension("yaml");
    // `log_json: true` gives the startup log a machine-parseable line —
    // the plain-text formatter interleaves ANSI color codes between a
    // field's name and value, which would make "addr=" unreliable to
    // find as a contiguous substring.
    let yaml = format!(
        r#"
server:
  port: 8080
  request_timeout_s: 30
  log_json: true
cache:
  memory_percent: 10.0
storages:
  - id: main
    driver: postgis
    url_env: DATABASE_URL
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: demo
    catalog: default
    storage: main
    table: {table}
    geometry: geom
    pk: id
    datetime: observed_at
    tiles: {{ minzoom: 0, maxzoom: 5, caps: {{}} }}
"#
    );
    std::fs::write(&path, common::legacy_config(&yaml)).expect("writes the throwaway config");
    path
}

/// Like [`write_temp_config`], but the collection declares only routing
/// (`id`/`tenant`/`storage`) — no `table`, `geometry`, or `pk` — exercising
/// `#19`'s derived-descriptor path end to end through the real binary.
/// `collection_id` doubles as the physical table name, since an omitted
/// `table` derives to the collection's `id` by convention.
fn write_temp_config_with_omitted_physical_fields(collection_id: &str) -> PathBuf {
    let mut path = common::unique_temp_path("tellurion-server-binary-test-derived");
    path.set_extension("yaml");
    let yaml = format!(
        r#"
server:
  port: 8080
  request_timeout_s: 30
  log_json: true
cache:
  memory_percent: 10.0
storages:
  - id: main
    driver: postgis
    url_env: DATABASE_URL
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: {collection_id}
    catalog: default
    storage: main
    tiles: {{ minzoom: 0, maxzoom: 5, caps: {{}} }}
"#
    );
    std::fs::write(&path, common::legacy_config(&yaml)).expect("writes the throwaway config");
    path
}

/// Like [`write_temp_config`], with `routing: { write: main }` added so the
/// collection's write lane actually resolves — `Router::resolve_write` has
/// no "defaults to the single storage" fallback the read lanes get (see its
/// own doc), so a config with no explicit write routing at all refuses
/// every `PUT`/`DELETE` with a `CapabilityUnsupported` 404. No `auth:`
/// block: `state.authorizer` stays `None`, so the write lane's own
/// policy checkpoint (`write_handlers::authorize_write_lane`) allows the
/// request through unconditionally, the same open-by-default behavior
/// every other lane in this file's configs already gets. Geometry, primary
/// key and SRID are deliberately left to catalog derivation: `srid` is not
/// an operator-configurable field, so fully pinning the other physical fields
/// would take the no-introspection fast path and lose a non-4326 table's CRS.
fn write_temp_config_with_write_routing(table: &str) -> PathBuf {
    let mut path = common::unique_temp_path("tellurion-server-binary-test-write");
    path.set_extension("yaml");
    let yaml = format!(
        r#"
server:
  port: 8080
  request_timeout_s: 30
  log_json: true
cache:
  memory_percent: 10.0
storages:
  - id: main
    driver: postgis
    url_env: DATABASE_URL
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: demo
    catalog: default
    storage: main
    table: {table}
    routing: {{ write: main }}
    tiles: {{ minzoom: 0, maxzoom: 5, caps: {{}} }}
"#
    );
    std::fs::write(&path, common::legacy_config(&yaml)).expect("writes the throwaway config");
    path
}

/// Like [`write_temp_config_with_write_routing`], with `id_type: uuid`
/// declared on the collection (`#87`).
fn write_temp_config_with_write_routing_uuid(table: &str) -> PathBuf {
    let mut path = common::unique_temp_path("tellurion-server-binary-test-write-uuid");
    path.set_extension("yaml");
    let yaml = format!(
        r#"
server:
  port: 8080
  request_timeout_s: 30
  log_json: true
cache:
  memory_percent: 10.0
storages:
  - id: main
    driver: postgis
    url_env: DATABASE_URL
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: demo
    catalog: default
    storage: main
    table: {table}
    geometry: geom
    pk: id
    id_type: uuid
    routing: {{ write: main }}
    tiles: {{ minzoom: 0, maxzoom: 5, caps: {{}} }}
"#
    );
    std::fs::write(&path, common::legacy_config(&yaml)).expect("writes the throwaway config");
    path
}

/// Like [`write_temp_config_with_write_routing`], with `id_type: text`
/// declared on the collection (`#94`).
fn write_temp_config_with_write_routing_text(table: &str) -> PathBuf {
    let mut path = common::unique_temp_path("tellurion-server-binary-test-write-text");
    path.set_extension("yaml");
    let yaml = format!(
        r#"
server:
  port: 8080
  request_timeout_s: 30
  log_json: true
cache:
  memory_percent: 10.0
storages:
  - id: main
    driver: postgis
    url_env: DATABASE_URL
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: demo
    catalog: default
    storage: main
    table: {table}
    geometry: geom
    pk: id
    id_type: text
    routing: {{ write: main }}
    tiles: {{ minzoom: 0, maxzoom: 5, caps: {{}} }}
"#
    );
    std::fs::write(&path, common::legacy_config(&yaml)).expect("writes the throwaway config");
    path
}

#[cfg(unix)]
fn write_shutdown_config() -> PathBuf {
    let mut path = common::unique_temp_path("tellurion-server-binary-test-shutdown");
    path.set_extension("yaml");
    std::fs::write(
        &path,
        common::legacy_config(
            r#"
server:
  port: 0
  log_json: true
  drain_timeout_s: 1
  readiness_probe_interval_s: 1
  readiness_probe_timeout_s: 1
cache:
  memory_percent: 10.0
"#,
        ),
    )
    .expect("writes the shutdown config");
    path
}

#[cfg(unix)]
#[test]
fn real_binary_exits_cleanly_and_promptly_on_sigterm() {
    let _serial = serialize_server_test();
    let config_path = write_shutdown_config();
    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env("TELLURION_CONFIG", &config_path)
        .env("RUST_LOG", "info");

    let (mut process, _addr, _stderr_log) = spawn_server(command);

    let signal_result = unsafe { libc::kill(process.child.id() as libc::pid_t, libc::SIGTERM) };
    assert_eq!(signal_result, 0, "SIGTERM reaches the real server process");

    let started = Instant::now();
    let ceiling = shutdown_timeout();
    let status = loop {
        if let Some(status) = process.child.try_wait().expect("polls server exit status") {
            break status;
        }
        assert!(
            started.elapsed() < ceiling,
            "the server should exit within its bounded drain window (configured \
             drain_timeout_s=1) — waited {:?} against a {:?} ceiling",
            started.elapsed(),
            ceiling,
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        status.success(),
        "SIGTERM should produce a clean exit: {status}"
    );

    drop(process);
    let _ = std::fs::remove_file(config_path);
}

#[test]
fn real_binary_serves_the_full_request_lifecycle_over_http() {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        eprintln!(
            "skipping real_binary_serves_the_full_request_lifecycle_over_http: DATABASE_URL not set"
        );
        return;
    };

    let _serial = serialize_server_test();
    let table = "tellurion_server_binary_test_items";
    let runtime = tokio::runtime::Runtime::new().expect("builds a runtime for seeding");
    runtime.block_on(seed_table(&database_url, table));

    let config_path = write_temp_config(table);

    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env("DATABASE_URL", &database_url)
        .env("TELLURION_CONFIG", &config_path)
        .env("PORT", "0");

    let (process, addr, _stderr_log) = spawn_server(command);

    let landing = http_get(&addr, "/");
    assert_eq!(
        landing.status, 200,
        "top-level landing page should return 200"
    );
    assert_eq!(landing.content_type.as_deref(), Some("application/json"));

    let features_landing = http_get(&addr, "/public/features/catalogs/default");
    assert_eq!(
        features_landing.status, 200,
        "features root landing page should return 200"
    );
    assert_eq!(
        features_landing.content_type.as_deref(),
        Some("application/json")
    );

    let conformance = http_get(&addr, "/public/features/catalogs/default/conformance");
    assert_eq!(conformance.status, 200, "/conformance should return 200");
    assert_eq!(
        conformance.content_type.as_deref(),
        Some("application/json")
    );

    let collection = http_get(&addr, "/public/features/catalogs/default/collections/demo");
    assert_eq!(collection.status, 200, "collection should return 200");
    assert_eq!(collection.content_type.as_deref(), Some("application/json"));

    let items = http_get(
        &addr,
        "/public/features/catalogs/default/collections/demo/items",
    );
    assert_eq!(items.status, 200, "items should return 200");
    assert_eq!(items.content_type.as_deref(), Some("application/geo+json"));

    let tile = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/0/0/0.mvt",
    );
    assert_eq!(
        tile.status, 200,
        "z0 tile covering all seeded points should return 200"
    );
    assert_eq!(
        tile.content_type.as_deref(),
        Some("application/vnd.mapbox-vector-tile")
    );
    assert!(!tile.body.is_empty(), "the tile body should not be empty");

    drop(process);
    let _ = std::fs::remove_file(config_path);
}

/// Regression test for the `{tileMatrix}/{tileRow}/{tileCol}` path order
/// (OGC API Tiles — row before column): drives the real binary over HTTP
/// with a seeded point at an off-diagonal tile coordinate (tileRow != tileCol)
/// and asserts the correct-order request serves it while the transposed
/// request — tileRow and tileCol swapped — does not. A route that silently
/// reverted to `{tileMatrix}/{tileCol}/{tileRow}` (slippy/XYZ order) would
/// fail this test: both requests would resolve to the *other* tile's
/// content, so "correct order returns the point" and "transposed order
/// returns empty" would flip together.
///
/// Coordinates: seed point (lon -170, lat 10) falls in WebMercatorQuad tile
/// z=4, tileRow=7, tileCol=0 — verified independently against a live
/// PostGIS instance with `ST_Contains(ST_Transform(ST_TileEnvelope(4, 0, 7),
/// 4326), the point)`. The transposed coordinate (tileRow=0, tileCol=7)
/// covers a tile near the north pole, nowhere near the seeded point, so it
/// is genuinely empty rather than accidentally also containing data.
#[test]
fn real_binary_rejects_a_transposed_row_col_request() {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        eprintln!(
            "skipping real_binary_rejects_a_transposed_row_col_request: DATABASE_URL not set"
        );
        return;
    };

    let _serial = serialize_server_test();
    let table = "tellurion_server_binary_test_off_diagonal";
    let runtime = tokio::runtime::Runtime::new().expect("builds a runtime for seeding");
    runtime.block_on(seed_off_diagonal_point(&database_url, table));

    let config_path = write_temp_config(table);

    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env("DATABASE_URL", &database_url)
        .env("TELLURION_CONFIG", &config_path)
        .env("PORT", "0");

    let (process, addr, _stderr_log) = spawn_server(command);

    let correct = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/4/7/0.mvt",
    );
    assert_eq!(
        correct.status, 200,
        "z=4, tileRow=7, tileCol=0 covers the seeded point and should return 200"
    );
    assert!(
        !correct.body.is_empty(),
        "the correctly-ordered tile should carry the seeded point's geometry"
    );

    let transposed = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/4/0/7.mvt",
    );
    assert_eq!(
        transposed.status, 204,
        "the transposed coordinate (tileRow=0, tileCol=7) is nowhere near the \
         seeded point and must come back empty, not the same content"
    );

    drop(process);
    let _ = std::fs::remove_file(config_path);
}

/// `#19` end to end through the real binary: a config declaring nothing but
/// routing for its one collection (no `table`/`geometry`/`pk`) still boots
/// and serves real items — table derives from the collection id, geometry
/// and pk derive from the storage's `CatalogSource`.
#[test]
fn real_binary_boots_and_serves_a_collection_with_omitted_physical_fields() {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        eprintln!(
            "skipping real_binary_boots_and_serves_a_collection_with_omitted_physical_fields: DATABASE_URL not set"
        );
        return;
    };

    let _serial = serialize_server_test();
    let collection_id = "tellurion_server_binary_test_omitted_fields";
    let runtime = tokio::runtime::Runtime::new().expect("builds a runtime for seeding");
    runtime.block_on(seed_table(&database_url, collection_id));

    let config_path = write_temp_config_with_omitted_physical_fields(collection_id);

    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env("DATABASE_URL", &database_url)
        .env("TELLURION_CONFIG", &config_path)
        .env("PORT", "0");

    let (process, addr, _stderr_log) = spawn_server(command);

    let items = http_get(
        &addr,
        &format!("/public/features/catalogs/default/collections/{collection_id}/items"),
    );
    assert_eq!(
        items.status, 200,
        "a collection configured with only routing fields must still serve items"
    );
    let body: serde_json::Value = serde_json::from_slice(&items.body).expect("valid JSON body");
    assert_eq!(
        body["numberReturned"], 3,
        "all three seeded rows come back through the derived physical shape"
    );

    drop(process);
    let _ = std::fs::remove_file(config_path);
}

/// `#27` end to end: `/collections/{cid}` for a non-empty PostGIS-backed
/// collection serves a real `extent.spatial.bbox`, not `null`.
#[test]
fn real_binary_collection_metadata_reports_a_derived_spatial_extent() {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        eprintln!(
            "skipping real_binary_collection_metadata_reports_a_derived_spatial_extent: DATABASE_URL not set"
        );
        return;
    };

    let _serial = serialize_server_test();
    let table = "tellurion_server_binary_test_extent";
    let runtime = tokio::runtime::Runtime::new().expect("builds a runtime for seeding");
    runtime.block_on(seed_table(&database_url, table));

    let config_path = write_temp_config(table);

    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env("DATABASE_URL", &database_url)
        .env("TELLURION_CONFIG", &config_path)
        .env("PORT", "0");

    let (process, addr, _stderr_log) = spawn_server(command);

    let collection = http_get(&addr, "/public/features/catalogs/default/collections/demo");
    assert_eq!(collection.status, 200);
    let body: serde_json::Value =
        serde_json::from_slice(&collection.body).expect("valid JSON body");

    let bbox = body["extent"]["spatial"]["bbox"][0]
        .as_array()
        .expect("extent.spatial.bbox[0] is present and is an array")
        .iter()
        .map(|v| v.as_f64().expect("bbox entries are numbers"))
        .collect::<Vec<_>>();
    assert_eq!(bbox.len(), 4);
    // Wide enough to absorb `ST_EstimatedExtent`'s `float4`-precision
    // statistics (the seeded table is `ANALYZE`d, so the estimated path is
    // the one exercised here) — a real, documented approximation, not a bug.
    const TOLERANCE_DEG: f64 = 0.1;
    assert!(
        (bbox[0] - 10.0).abs() < TOLERANCE_DEG,
        "minx was {}",
        bbox[0]
    );
    assert!(
        (bbox[1] - 45.0).abs() < TOLERANCE_DEG,
        "miny was {}",
        bbox[1]
    );
    assert!(
        (bbox[2] - 12.0).abs() < TOLERANCE_DEG,
        "maxx was {}",
        bbox[2]
    );
    assert!(
        (bbox[3] - 47.0).abs() < TOLERANCE_DEG,
        "maxy was {}",
        bbox[3]
    );
    assert_eq!(
        body["extent"]["spatial"]["crs"],
        "http://www.opengis.net/def/crs/OGC/1.3/CRS84"
    );

    drop(process);
    let _ = std::fs::remove_file(config_path);
}

/// `#25` end to end through the real binary and a real HTTP connection: a
/// `PUT` on a write-routed, auth-disabled collection creates an item, a
/// `GET` on the same URL reads the same geometry and properties back, and a
/// `DELETE` removes it again — nothing from this test survives it. Every
/// other test in this file only ever reads a seeded table; this is the
/// workspace's one live proof that a write actually lands and round-trips
/// through the API rather than only through `tellurion-postgis`'s own
/// driver-level `WriteSink` tests.
#[test]
fn real_binary_writes_and_reads_back_an_item_over_http() {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        eprintln!(
            "skipping real_binary_writes_and_reads_back_an_item_over_http: DATABASE_URL not set"
        );
        return;
    };

    let _serial = serialize_server_test();
    let table = "tellurion_server_binary_test_write_e2e";
    let runtime = tokio::runtime::Runtime::new().expect("builds a runtime for seeding");
    runtime.block_on(seed_write_table(&database_url, table));

    let config_path = write_temp_config_with_write_routing(table);

    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env("DATABASE_URL", &database_url)
        .env("TELLURION_CONFIG", &config_path)
        .env("PORT", "0");

    let (process, addr, _stderr_log) = spawn_server(command);
    let item_path = "/public/features/catalogs/default/collections/demo/items/1";

    // `Content-Crs` naming CRS84 explicitly, rather than leaning on it being
    // the default for an absent header (OGC API Features Part 4 Requirement
    // 41, `/req/features/default-crs`). `seed_write_table` makes this table
    // 4326, so the round trip asserted below would hold either way -- but it
    // would hold by accident of the fixture's SRID, with nothing here saying
    // which CRS the body is in. `geopackage_binary.rs` copied this proof onto
    // a 3857 table, inherited that silence, and shipped an assertion that
    // could never pass; naming the CRS here is what stops the next copy
    // inheriting the same trap.
    //
    // CRS84 and deliberately not `.../EPSG/0/4326`: those differ in axis
    // order (CRS84 is longitude-first, EPSG:4326 latitude-first -- see
    // `crs::is_lat_lon_order`), so declaring 4326 here would transpose the
    // point and change what this test means rather than document it.
    const CRS84_CONTENT_CRS: &str = "<http://www.opengis.net/def/crs/OGC/1.3/CRS84>";
    let feature = br#"{"type":"Feature","geometry":{"type":"Point","coordinates":[7.5,45.5]},"properties":{"name":"e2e-write-test"}}"#;
    let put = http_request_with_headers(
        &addr,
        "PUT",
        item_path,
        feature,
        &[("Content-Crs", CRS84_CONTENT_CRS)],
    );
    assert_eq!(
        put.status, 204,
        "a PUT on a write-routed collection should create the item and return 204"
    );

    let got = http_write_request(&addr, "GET", item_path, &[]);
    assert_eq!(got.status, 200, "the written item should read back as 200");
    assert_eq!(got.content_type.as_deref(), Some("application/geo+json"));
    let body: serde_json::Value = serde_json::from_slice(&got.body).expect("valid JSON body");
    assert_eq!(body["properties"]["name"], "e2e-write-test");
    let coordinates = body["geometry"]["coordinates"]
        .as_array()
        .expect("geometry.coordinates is present and is an array")
        .iter()
        .map(|v| v.as_f64().expect("coordinate entries are numbers"))
        .collect::<Vec<_>>();
    assert_eq!(coordinates.len(), 2);
    const TOLERANCE_DEG: f64 = 1e-9;
    assert!(
        (coordinates[0] - 7.5).abs() < TOLERANCE_DEG,
        "x was {}",
        coordinates[0]
    );
    assert!(
        (coordinates[1] - 45.5).abs() < TOLERANCE_DEG,
        "y was {}",
        coordinates[1]
    );

    let delete = http_write_request(&addr, "DELETE", item_path, &[]);
    assert_eq!(
        delete.status, 204,
        "cleanup: DELETE should remove the item this test created"
    );
    let after_delete = http_write_request(&addr, "GET", item_path, &[]);
    assert_eq!(
        after_delete.status, 404,
        "the deleted item should no longer be readable"
    );

    drop(process);
    let _ = std::fs::remove_file(config_path);
}

#[test]
fn real_binary_patch_unsets_a_property_without_moving_3857_geometry() {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        eprintln!("skipping PostGIS PATCH test: DATABASE_URL not set");
        return;
    };
    let _serial = serialize_server_test();
    let table = "tellurion_server_binary_test_patch_3857";
    let runtime = tokio::runtime::Runtime::new().expect("builds a runtime for seeding");
    runtime.block_on(seed_patch_table_3857(&database_url, table));
    let config_path = write_temp_config_with_write_routing(table);
    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env("DATABASE_URL", &database_url)
        .env("TELLURION_CONFIG", &config_path)
        .env("PORT", "0");
    let (process, addr, stderr_log) = spawn_server(command);
    let response = http_request_with_headers(
        &addr,
        "PATCH",
        "/public/features/catalogs/default/collections/demo/items/1",
        br#"{"properties":{"count":null}}"#,
        &[("Content-Type", "application/merge-patch+json")],
    );
    let stderr = stderr_log
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("\n");
    assert_eq!(
        response.status,
        200,
        "PATCH response body: {}\nserver stderr:\n{stderr}",
        String::from_utf8_lossy(&response.body)
    );
    let (client, connection) = runtime
        .block_on(tokio_postgres::connect(&database_url, NoTls))
        .expect("connects for verification");
    runtime.spawn(async move {
        let _ = connection.await;
    });
    let row = runtime
        .block_on(client.query_one(
            &format!(
                "SELECT ST_X(geom), ST_Y(geom), count, \
                 (SELECT payload FROM {table}_outbox ORDER BY sequence DESC LIMIT 1) \
                 FROM {table} WHERE id = 1"
            ),
            &[],
        ))
        .expect("reads committed PATCH state");
    const TOLERANCE_METRES: f64 = 0.001;
    let x = row.get::<_, f64>(0);
    let y = row.get::<_, f64>(1);
    assert!((x - 500000.0).abs() < TOLERANCE_METRES, "x was {x}");
    assert!((y - 6000000.0).abs() < TOLERANCE_METRES, "y was {y}");
    assert_eq!(row.get::<_, Option<i32>>(2), None);
    let payload: serde_json::Value = row.get(3);
    assert_eq!(payload["properties"]["count"], serde_json::Value::Null);
    drop(process);
    let _ = std::fs::remove_file(config_path);
}

/// `#88` end to end through the real binary: a `POST` on a write-routed
/// collection creates an item with a server-assigned id, returns `201` with
/// a `Location` header pointing at it, and a `GET` on that exact URL reads
/// the same geometry and properties back — the create-lane counterpart of
/// [`real_binary_writes_and_reads_back_an_item_over_http`]'s own `PUT`
/// proof. A second `POST` mints a distinct, higher id than the first,
/// proving the minted sequence is real (a live PostGIS `bigserial`), not a
/// fixed or repeating stand-in.
#[test]
fn real_binary_creates_an_item_with_a_server_assigned_id_over_http() {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        eprintln!(
            "skipping real_binary_creates_an_item_with_a_server_assigned_id_over_http: DATABASE_URL not set"
        );
        return;
    };

    let _serial = serialize_server_test();
    let table = "tellurion_server_binary_test_create_e2e";
    let runtime = tokio::runtime::Runtime::new().expect("builds a runtime for seeding");
    runtime.block_on(seed_write_table(&database_url, table));

    let config_path = write_temp_config_with_write_routing(table);

    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env("DATABASE_URL", &database_url)
        .env("TELLURION_CONFIG", &config_path)
        .env("PORT", "0");

    let (process, addr, _stderr_log) = spawn_server(command);
    let items_path = "/public/features/catalogs/default/collections/demo/items";

    let first_feature = br#"{"type":"Feature","geometry":{"type":"Point","coordinates":[7.5,45.5]},"properties":{"name":"e2e-create-first"}}"#;
    let first_create = http_write_request(&addr, "POST", items_path, first_feature);
    assert_eq!(
        first_create.status, 201,
        "a POST on a write-routed collection should create the item and return 201"
    );
    let first_location = first_create
        .location
        .clone()
        .expect("a create response carries a Location header");
    assert!(
        first_location.starts_with(&format!("{items_path}/")),
        "Location was: {first_location}"
    );

    let second_feature = br#"{"type":"Feature","geometry":{"type":"Point","coordinates":[8.5,46.5]},"properties":{"name":"e2e-create-second"}}"#;
    let second_create = http_write_request(&addr, "POST", items_path, second_feature);
    assert_eq!(second_create.status, 201);
    let second_location = second_create
        .location
        .expect("a create response carries a Location header");
    assert_ne!(
        first_location, second_location,
        "two creates must mint distinct ids"
    );

    let got = http_write_request(&addr, "GET", &first_location, &[]);
    assert_eq!(
        got.status, 200,
        "the created item should read back as 200 at its Location"
    );
    assert_eq!(got.content_type.as_deref(), Some("application/geo+json"));
    let body: serde_json::Value = serde_json::from_slice(&got.body).expect("valid JSON body");
    assert_eq!(body["properties"]["name"], "e2e-create-first");

    let delete_first = http_write_request(&addr, "DELETE", &first_location, &[]);
    assert_eq!(delete_first.status, 204, "cleanup: DELETE the first item");
    let delete_second = http_write_request(&addr, "DELETE", &second_location, &[]);
    assert_eq!(delete_second.status, 204, "cleanup: DELETE the second item");

    drop(process);
    let _ = std::fs::remove_file(config_path);
}

/// `#87` end to end through the real binary: a collection declaring
/// `id_type: uuid` mints a real server-side uuid on `POST` (`201`,
/// `Location` pointing at it, no config-load or id-type refusal reaches the
/// client), and `GET`/`PUT`/`DELETE` all round-trip against that exact
/// minted id through the ordinary path-param route — the same routes
/// [`real_binary_writes_and_reads_back_an_item_over_http`] and
/// [`real_binary_creates_an_item_with_a_server_assigned_id_over_http`] prove
/// for the default `Integer` id-type, proving every id-bearing boundary
/// (create, path-param parse on read/update/delete) honors the declared
/// `id_type` all the way from config to the real HTTP surface.
#[test]
fn real_binary_round_trips_over_a_server_assigned_uuid_id_over_http() {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        eprintln!(
            "skipping real_binary_round_trips_over_a_server_assigned_uuid_id_over_http: DATABASE_URL not set"
        );
        return;
    };

    let _serial = serialize_server_test();
    let table = "tellurion_server_binary_test_uuid_e2e";
    let runtime = tokio::runtime::Runtime::new().expect("builds a runtime for seeding");
    runtime.block_on(seed_write_table_uuid(&database_url, table));

    let config_path = write_temp_config_with_write_routing_uuid(table);

    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env("DATABASE_URL", &database_url)
        .env("TELLURION_CONFIG", &config_path)
        .env("PORT", "0");

    let (process, addr, _stderr_log) = spawn_server(command);
    let items_path = "/public/features/catalogs/default/collections/demo/items";

    let feature = br#"{"type":"Feature","geometry":{"type":"Point","coordinates":[7.5,45.5]},"properties":{"name":"uuid-e2e-created"}}"#;
    let create = http_write_request(&addr, "POST", items_path, feature);
    assert_eq!(
        create.status, 201,
        "a POST on a uuid id_type collection should create the item and return 201"
    );
    let location = create
        .location
        .expect("a create response carries a Location header");
    let minted_id = location
        .strip_prefix(&format!("{items_path}/"))
        .expect("Location points under the items path");
    uuid::Uuid::parse_str(minted_id).expect("the minted id is a real uuid, not an integer");

    let got = http_write_request(&addr, "GET", &location, &[]);
    assert_eq!(
        got.status, 200,
        "the created item should read back as 200 at its Location"
    );
    let body: serde_json::Value = serde_json::from_slice(&got.body).expect("valid JSON body");
    assert_eq!(body["properties"]["name"], "uuid-e2e-created");
    assert_eq!(body["id"], minted_id);

    let updated = br#"{"type":"Feature","geometry":{"type":"Point","coordinates":[8.5,46.5]},"properties":{"name":"uuid-e2e-updated"}}"#;
    let put = http_write_request(&addr, "PUT", &location, updated);
    assert_eq!(
        put.status, 204,
        "a PUT at the minted uuid's own Location should update it and return 204"
    );

    let got_after_put = http_write_request(&addr, "GET", &location, &[]);
    assert_eq!(got_after_put.status, 200);
    let body_after_put: serde_json::Value =
        serde_json::from_slice(&got_after_put.body).expect("valid JSON body");
    assert_eq!(body_after_put["properties"]["name"], "uuid-e2e-updated");

    let delete = http_write_request(&addr, "DELETE", &location, &[]);
    assert_eq!(delete.status, 204, "DELETE should remove the minted item");
    let after_delete = http_write_request(&addr, "GET", &location, &[]);
    assert_eq!(
        after_delete.status, 404,
        "the deleted uuid-pk item should no longer be readable"
    );

    drop(process);
    let _ = std::fs::remove_file(config_path);
}

/// `#94` end to end through the real binary: a collection declaring
/// `id_type: text` requires the caller to supply the id in the `POST` body
/// (`201`, `Location` pointing at exactly that id — never a server-minted
/// one), a `POST` re-using the same id is a `409`, and `GET`/`PUT`/`DELETE`
/// all round-trip against that exact caller-supplied id through the
/// ordinary path-param route — the `text` counterpart of
/// [`real_binary_round_trips_over_a_server_assigned_uuid_id_over_http`].
#[test]
fn real_binary_round_trips_over_a_caller_supplied_text_id_over_http() {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        eprintln!(
            "skipping real_binary_round_trips_over_a_caller_supplied_text_id_over_http: DATABASE_URL not set"
        );
        return;
    };

    let _serial = serialize_server_test();
    let table = "tellurion_server_binary_test_text_e2e";
    let runtime = tokio::runtime::Runtime::new().expect("builds a runtime for seeding");
    runtime.block_on(seed_write_table_text(&database_url, table));

    let config_path = write_temp_config_with_write_routing_text(table);

    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env("DATABASE_URL", &database_url)
        .env("TELLURION_CONFIG", &config_path)
        .env("PORT", "0");

    let (process, addr, _stderr_log) = spawn_server(command);
    let items_path = "/public/features/catalogs/default/collections/demo/items";

    let missing_id = br#"{"type":"Feature","geometry":{"type":"Point","coordinates":[7.5,45.5]},"properties":{"name":"text-e2e-no-id"}}"#;
    let refused = http_write_request(&addr, "POST", items_path, missing_id);
    assert_eq!(
        refused.status, 400,
        "a POST on a text id_type collection with no body id should be refused"
    );

    let feature = br#"{"type":"Feature","id":"text-e2e-1","geometry":{"type":"Point","coordinates":[7.5,45.5]},"properties":{"name":"text-e2e-created"}}"#;
    let create = http_write_request(&addr, "POST", items_path, feature);
    assert_eq!(
        create.status, 201,
        "a POST on a text id_type collection with a body id should create the item and return 201"
    );
    let location = create
        .location
        .expect("a create response carries a Location header");
    assert_eq!(
        location,
        format!("{items_path}/text-e2e-1"),
        "Location points at exactly the caller-supplied id, never a minted one"
    );

    let conflict = http_write_request(&addr, "POST", items_path, feature);
    assert_eq!(
        conflict.status, 409,
        "re-using the same caller-supplied id should be refused as a conflict"
    );

    let got = http_write_request(&addr, "GET", &location, &[]);
    assert_eq!(
        got.status, 200,
        "the created item should read back as 200 at its Location"
    );
    let body: serde_json::Value = serde_json::from_slice(&got.body).expect("valid JSON body");
    assert_eq!(body["properties"]["name"], "text-e2e-created");
    assert_eq!(body["id"], "text-e2e-1");

    let updated = br#"{"type":"Feature","geometry":{"type":"Point","coordinates":[8.5,46.5]},"properties":{"name":"text-e2e-updated"}}"#;
    let put = http_write_request(&addr, "PUT", &location, updated);
    assert_eq!(
        put.status, 204,
        "a PUT at the caller-supplied id's own Location should update it and return 204"
    );

    let got_after_put = http_write_request(&addr, "GET", &location, &[]);
    assert_eq!(got_after_put.status, 200);
    let body_after_put: serde_json::Value =
        serde_json::from_slice(&got_after_put.body).expect("valid JSON body");
    assert_eq!(body_after_put["properties"]["name"], "text-e2e-updated");

    let delete = http_write_request(&addr, "DELETE", &location, &[]);
    assert_eq!(
        delete.status, 204,
        "DELETE should remove the caller-supplied-id item"
    );
    let after_delete = http_write_request(&addr, "GET", &location, &[]);
    assert_eq!(
        after_delete.status, 404,
        "the deleted text-pk item should no longer be readable"
    );

    drop(process);
    let _ = std::fs::remove_file(config_path);
}

#[cfg(test)]
mod parsing_tests {
    use super::parse_listening_addr;

    #[test]
    fn extracts_the_address_from_the_listening_line() {
        let line = r#"{"timestamp":"2026-07-18T00:00:00Z","level":"INFO","fields":{"message":"tellurion listening","addr":"127.0.0.1:54321","config_path":"./config.yaml"},"target":"tellurion"}"#;
        assert_eq!(
            parse_listening_addr(line),
            Some("127.0.0.1:54321".to_string())
        );
    }

    #[test]
    fn ignores_unrelated_lines() {
        let line = r#"{"timestamp":"2026-07-18T00:00:00Z","level":"INFO","fields":{"message":"some other event","addr":"127.0.0.1:1"},"target":"tellurion"}"#;
        assert_eq!(parse_listening_addr(line), None);

        assert_eq!(parse_listening_addr("not json at all"), None);
    }
}
