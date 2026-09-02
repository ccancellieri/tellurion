//! The `#73` acceptance proof: the real `tellurion` binary, built with the
//! bundled database driver compiled out (`--no-default-features --features
//! geopackage`), serves the full read/write/tiles request lifecycle through
//! the abstract driver contract backed by nothing but a local `.gpkg` file —
//! no database service, no container runtime. Mirrors `tellurion-postgis`'s
//! own `binary.rs` write-lane proof (`real_binary_writes_and_reads_back_an_
//! item_over_http`), adapted to the embedded driver.
//!
//! Run exactly the invocation this proves:
//!
//! ```sh
//! cargo test -p tellurion --no-default-features --features geopackage
//! cargo tree -p tellurion --no-default-features --features geopackage -e normal
//! ```
//!
//! The second command is the other half of the proof (checked separately,
//! not by this file): it must list `tellurion-geopackage`/`rusqlite` and no
//! `postgres`/`postgis`/`deadpool` crate anywhere in the graph.
//!
//! Gated two ways: `required-features = ["geopackage"]` in `Cargo.toml`
//! skips building this file entirely under a feature set with `geopackage`
//! off, and the inner `#![cfg]` below additionally requires `postgis` to be
//! *off* — `geopackage` is this server's own *default-on* feature (unlike
//! every other file-backed driver's own binary test in this directory), so
//! without this second gate the file would also compile — and both drivers
//! would register — under the plain default feature set, where this test's
//! single-collection, single-driver config assumptions don't hold. See
//! `pmtiles_binary.rs`'s own doc comment for why `required-features` alone
//! can't express "on, and this other feature off".
//!
//! ## Provisioning choice
//!
//! This driver's provisioning subcommand lives in `tellurion-ingest`
//! (`geopackage create-tables`), a *separate* binary crate this one has no
//! Cargo dependency edge to (by design — `tellurion-ingest` never depends on
//! a driver crate, and nothing here would make `CARGO_BIN_EXE_tellurion-
//! ingest` a reliable env var without adding one purely to resolve a path).
//! Rather than add that edge just to shell out to a sibling binary, this
//! file provisions its own temp `.gpkg` fixture directly via `rusqlite` —
//! the driver contract's own explicitly sanctioned fallback ("the crate's
//! own provisioning code invoked from the test if the CLI binary is awkward
//! to drive"). The DDL below is the same shape `tellurion-ingest::
//! geopackage`'s own module produces, kept in sync by hand — the same
//! three-way hand-kept-convention arrangement that module's own doc already
//! describes between the ingest crate and the driver crate.

#![cfg(all(feature = "geopackage", not(feature = "postgis")))]

mod common;

use std::path::PathBuf;
use std::process::Command;

use rusqlite::Connection;

use common::{http_get, http_request_with_headers, http_write_request, spawn_server};

fn temp_path(suffix: &str) -> PathBuf {
    let mut path = common::unique_temp_path("tellurion-server-geopackage-binary-test");
    path.set_extension("-{suffix}");
    path
}

/// Provisions a fresh `.gpkg` file with one feature table (`demo`, SRID
/// `3857` so the tiles-lane assertion below also has something to prove —
/// see `tellurion-geopackage::driver`'s own `TileSource` doc for why that
/// SRID is required), its GeoPackage spec metadata rows, its R*Tree spatial
/// index and maintenance triggers, and its outbox table — no data rows; the
/// test itself writes those over real HTTP. See this file's own top-level
/// doc for why this duplicates `tellurion-ingest::geopackage`'s DDL by hand
/// rather than shelling out to that crate's binary.
fn provision_gpkg(path: &PathBuf) {
    let conn = Connection::open(path).expect("creates the .gpkg file");
    conn.execute_batch(
        r#"
CREATE TABLE gpkg_spatial_ref_sys (
    srs_name TEXT NOT NULL,
    srs_id INTEGER NOT NULL PRIMARY KEY,
    organization TEXT NOT NULL,
    organization_coordsys_id INTEGER NOT NULL,
    definition TEXT NOT NULL,
    description TEXT
);
CREATE TABLE gpkg_contents (
    table_name TEXT NOT NULL PRIMARY KEY,
    data_type TEXT NOT NULL,
    identifier TEXT UNIQUE,
    description TEXT DEFAULT '',
    last_change DATETIME NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    min_x DOUBLE, min_y DOUBLE, max_x DOUBLE, max_y DOUBLE,
    srs_id INTEGER
);
CREATE TABLE gpkg_geometry_columns (
    table_name TEXT NOT NULL,
    column_name TEXT NOT NULL,
    geometry_type_name TEXT NOT NULL,
    srs_id INTEGER NOT NULL,
    z TINYINT NOT NULL,
    m TINYINT NOT NULL,
    CONSTRAINT pk_geom_cols PRIMARY KEY (table_name, column_name)
);
INSERT INTO gpkg_spatial_ref_sys (srs_name, srs_id, organization, organization_coordsys_id, definition, description)
VALUES ('WGS 84 / Pseudo-Mercator', 3857, 'EPSG', 3857, 'PROJCS["WGS 84 / Pseudo-Mercator"]', 'Web Mercator');

CREATE TABLE "demo" ("id" INTEGER PRIMARY KEY, "geom" POINT, "name" TEXT);
INSERT INTO gpkg_contents (table_name, data_type, identifier, srs_id) VALUES ('demo', 'features', 'demo', 3857);
INSERT INTO gpkg_geometry_columns (table_name, column_name, geometry_type_name, srs_id, z, m) VALUES ('demo', 'geom', 'POINT', 3857, 0, 0);

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
  INSERT OR REPLACE INTO "rtree_demo_geom" VALUES (
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
    .expect("provisions the fixture .gpkg schema");
}

fn write_temp_config(env_var: &str) -> PathBuf {
    let path = temp_path("config.yaml");
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
    driver: geopackage
    url_env: {env_var}
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: demo
    catalog: default
    storage: main
    routing: {{ write: main }}
    tiles: {{ minzoom: 0, maxzoom: 5, caps: {{}} }}
"#
    );
    std::fs::write(&path, common::legacy_config(&yaml)).expect("writes the throwaway config");
    path
}

/// The proof, end to end, with zero database service anywhere: collections
/// list, a `PUT`-created item reads back exactly, a `filter=` query narrows
/// to the right feature, `DELETE` removes an item, and the tiles lane
/// serves a real MVT tile carrying what's left — all through the real
/// binary built with the bundled database driver compiled out.
#[test]
fn real_geopackage_binary_serves_the_full_read_write_tiles_lifecycle_over_http() {
    let env_var = "TELLURION_GEOPACKAGE_BINARY_TEST_PATH";
    let gpkg_path = temp_path("fixture.gpkg");
    provision_gpkg(&gpkg_path);
    let config_path = write_temp_config(env_var);

    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env(env_var, &gpkg_path)
        .env("TELLURION_CONFIG", &config_path)
        .env("PORT", "0");

    let (process, addr, _stderr_log) = spawn_server(command);

    let landing = http_get(&addr, "/");
    assert_eq!(landing.status, 200, "landing page should return 200");

    let collections = http_get(&addr, "/public/features/catalogs/default/collections");
    assert_eq!(collections.status, 200, "/collections should return 200");
    let body: serde_json::Value =
        serde_json::from_slice(&collections.body).expect("valid JSON body");
    let list = body["collections"].as_array().expect("collections array");
    assert_eq!(
        list.len(),
        1,
        "exactly the one geopackage-backed collection"
    );
    assert_eq!(list[0]["id"], "demo");

    // Write two features over the real write endpoint.
    //
    // Both `PUT`s declare `Content-Crs` naming this collection's own storage
    // CRS, and that header is load-bearing rather than decorative. This
    // fixture's table is SRID 3857 (`provision_gpkg` picks it so the
    // tiles-lane assertion below has something to prove), while a GeoJSON
    // write body carrying no `Content-Crs` is CRS84 -- OGC API Features
    // Part 4 Requirement 41, `/req/features/default-crs`, which is exactly
    // what `write_handlers::resolve_content_crs` returns for an absent
    // header. Without this header the numbers below are degrees, the server
    // correctly reprojects them into metres, and the read-back assertion
    // compares 500000.0 against 55659745396.63678.
    //
    // Declaring the CRS is the fix rather than restating the expectation in
    // Web Mercator metres. Rewriting the assertion would make the test pass
    // by asserting a coordinate nobody chose, and would delete the property
    // this test exists to prove -- that a write round-trips through the
    // driver contract unchanged. Declaring it keeps that identity AND buys a
    // second proof: Requirement 40 (`/req/features/content-crs-header`) and
    // Requirement 42 clause B (`/req/features/crs-other-crs`) on the
    // GeoPackage write lane, whose sink answers `crs_capable` true for
    // exactly the 4326/3857 pair (see
    // `write_handlers::refuse_unreprojectable_content_crs`). The 204s below
    // are therefore also the assertion that this driver ACCEPTED a declared
    // storage-CRS write instead of refusing it by name.
    //
    // This file compiles only under `--no-default-features --features
    // geopackage`, a combination no CI leg built until the `geopackage` leg
    // was added, which is how the original assertion shipped never having
    // run. See `binary.rs`'s own write proof, which this file mirrors, for
    // the CRS84 counterpart of this header.
    const STORAGE_CRS_URI: &str = "http://www.opengis.net/def/crs/EPSG/0/3857";
    let storage_content_crs = format!("<{STORAGE_CRS_URI}>");
    let write_headers = [("Content-Crs", storage_content_crs.as_str())];

    let alpha = br#"{"type":"Feature","geometry":{"type":"Point","coordinates":[500000.0,6000000.0]},"properties":{"name":"alpha"}}"#;
    let put_alpha = http_request_with_headers(
        &addr,
        "PUT",
        "/public/features/catalogs/default/collections/demo/items/1",
        alpha,
        &write_headers,
    );
    assert_eq!(put_alpha.status, 204, "PUT should create item 1");

    let bravo = br#"{"type":"Feature","geometry":{"type":"Point","coordinates":[-500000.0,6000000.0]},"properties":{"name":"bravo"}}"#;
    let put_bravo = http_request_with_headers(
        &addr,
        "PUT",
        "/public/features/catalogs/default/collections/demo/items/2",
        bravo,
        &write_headers,
    );
    assert_eq!(put_bravo.status, 204, "PUT should create item 2");

    // Read item 1 back exactly.
    let got = http_write_request(
        &addr,
        "GET",
        "/public/features/catalogs/default/collections/demo/items/1",
        &[],
    );
    assert_eq!(got.status, 200, "the written item should read back as 200");
    assert_eq!(got.content_type.as_deref(), Some("application/geo+json"));
    let item: serde_json::Value = serde_json::from_slice(&got.body).expect("valid JSON body");
    assert_eq!(item["properties"]["name"], "alpha");
    let coordinates = item["geometry"]["coordinates"]
        .as_array()
        .expect("geometry.coordinates is present and is an array")
        .iter()
        .map(|v| v.as_f64().expect("coordinate entries are numbers"))
        .collect::<Vec<_>>();
    assert_eq!(coordinates, vec![500000.0, 6000000.0]);

    // A `filter=` query narrows to exactly the matching feature.
    let filtered = http_get(
        &addr,
        "/public/features/catalogs/default/collections/demo/items?filter=name%20%3D%20'alpha'",
    );
    assert_eq!(
        filtered.status, 200,
        "a filtered items query should return 200"
    );
    let filtered_body: serde_json::Value =
        serde_json::from_slice(&filtered.body).expect("valid JSON body");
    assert_eq!(filtered_body["numberReturned"], 1);
    assert_eq!(filtered_body["features"][0]["properties"]["name"], "alpha");

    // DELETE item 2; it must no longer be readable.
    let delete = http_write_request(
        &addr,
        "DELETE",
        "/public/features/catalogs/default/collections/demo/items/2",
        &[],
    );
    assert_eq!(delete.status, 204, "DELETE should remove item 2");
    let after_delete = http_write_request(
        &addr,
        "GET",
        "/public/features/catalogs/default/collections/demo/items/2",
        &[],
    );
    assert_eq!(
        after_delete.status, 404,
        "the deleted item should no longer be readable"
    );

    // z0/x0/y0 covers the whole EPSG:3857 world, so it must carry item 1's
    // geometry — a real MVT tile encoded in Rust via geozero, never a stub.
    let tile = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/0/0/0.mvt",
    );
    assert_eq!(
        tile.status, 200,
        "z0 tile covering the world should return 200"
    );
    assert_eq!(
        tile.content_type.as_deref(),
        Some("application/vnd.mapbox-vector-tile")
    );
    assert!(!tile.body.is_empty(), "the tile body should not be empty");

    // Decodes the real MVT bytes (never trust "non-empty" alone): exactly
    // one layer named after the collection, exactly one feature (item 2 was
    // deleted above), tagged with the id this driver's own MVT encoding
    // embeds (`tellurion-geopackage::driver`'s own doc: id-only tags,
    // mirroring PostGIS's `ST_AsMVT` shape).
    {
        use geozero::mvt::{tile as mvt_tile, Message, Tile};
        let decoded = Tile::decode(tile.body.as_slice()).expect("valid MVT protobuf bytes");
        assert_eq!(decoded.layers.len(), 1, "exactly one MVT layer");
        assert_eq!(decoded.layers[0].name, "demo");
        assert_eq!(
            decoded.layers[0].features.len(),
            1,
            "exactly item 1's feature (item 2 was deleted)"
        );
        let feature = &decoded.layers[0].features[0];
        assert_eq!(feature.r#type(), mvt_tile::GeomType::Point);
    }

    drop(process);
    let _ = std::fs::remove_file(config_path);
    let _ = std::fs::remove_file(&gpkg_path);
    let _ = std::fs::remove_file(format!("{}-wal", gpkg_path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", gpkg_path.display()));
}
