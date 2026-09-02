//! The DuckDB features-lane acceptance proof (issue #145): the real
//! `tellurion` binary, built with the bundled database driver compiled out
//! (`--no-default-features --features duckdb`), serves `/collections` and
//! paginated, bbox-filtered, CQL2-filtered GeoJSON items through the
//! abstract driver contract backed by nothing but a local `.duckdb` file —
//! no database service, no container runtime, no network access anywhere in
//! this test (see `tellurion-duckdb::driver`'s own "EXTENSION note" for why
//! that holds unconditionally for this driver, not just this test). Mirrors
//! `tellurion-geoparquet`'s own `geoparquet_binary.rs` proof, swapping the
//! file format and adding the CQL2-filter assertion this driver, unlike
//! GeoParquet, actually supports.
//!
//! Run exactly the invocation this proves:
//!
//! ```sh
//! cargo test -p tellurion --no-default-features --features duckdb
//! cargo tree -p tellurion --no-default-features --features duckdb -e normal
//! ```
//!
//! The second command is the other half of the proof (checked separately,
//! not by this file): it must list `tellurion-duckdb`/`duckdb` and no
//! `postgres`/`postgis`/`deadpool` crate anywhere in the graph.
//!
//! Gated two ways: `required-features = ["duckdb"]` in `Cargo.toml` skips
//! building this file entirely under the default feature set, and the inner
//! `#![cfg]` below additionally requires `postgis` to be *off* — see
//! `pmtiles_binary.rs`'s own doc comment for why `required-features` alone
//! can't express that.
//!
//! ## Provisioning choice
//!
//! This driver has no separate provisioning subcommand (unlike GeoPackage's
//! `tellurion-ingest geopackage create-tables`) — a `.duckdb` file is just a
//! database anyone can provision with a plain `CREATE TABLE`, so this test
//! provisions its own temp fixture directly via the `duckdb` crate, the same
//! "own the fixture, no cross-binary dependency" choice
//! `geopackage_binary.rs`'s own doc explains for its comparable case.

#![cfg(all(feature = "duckdb", not(feature = "postgis")))]

mod common;

use std::path::PathBuf;
use std::process::Command;

use duckdb::Connection;

use common::{http_get, ServerProcess};

fn temp_path(suffix: &str) -> PathBuf {
    let mut path = common::unique_temp_path("tellurion-server-duckdb-binary-test");
    path.set_extension(suffix);
    path
}

fn point_wkb(lon: f64, lat: f64) -> Vec<u8> {
    use geozero::GeozeroGeometry;
    let geojson = format!(r#"{{"type":"Point","coordinates":[{lon},{lat}]}}"#);
    let mut buf = Vec::new();
    let mut writer = geozero::wkb::WkbWriter::new(&mut buf, geozero::wkb::WkbDialect::Wkb);
    geozero::geojson::GeoJson(&geojson)
        .process_geom(&mut writer)
        .unwrap();
    buf
}

/// Same five points every other file-driver's own binary/fixture test uses,
/// for family resemblance.
const FEATURES: [(&str, i64, f64, f64); 5] = [
    ("alpha", 1, -4.0, 46.0),
    ("bravo", 2, -2.0, 48.0),
    ("charlie", 3, 0.0, 50.0),
    ("delta", 4, 2.0, 52.0),
    ("echo", 5, 4.0, 54.0),
];

fn provision_duckdb(path: &PathBuf) {
    let conn = Connection::open(path).expect("creates the .duckdb file");
    conn.execute_batch(
        "CREATE TABLE demo (id BIGINT PRIMARY KEY, geom BLOB, name VARCHAR, population BIGINT)",
    )
    .expect("provisions the fixture table");
    for (name, population, lon, lat) in FEATURES {
        conn.execute(
            "INSERT INTO demo (id, geom, name, population) VALUES (?, ?, ?, ?)",
            duckdb::params![population, point_wkb(lon, lat), name, population],
        )
        .expect("seeds one fixture feature");
    }
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
    driver: duckdb
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
"#
    );
    std::fs::write(&path, common::legacy_config(&yaml)).expect("writes the throwaway config");
    path
}

fn json_body(response: &common::HttpResponse) -> serde_json::Value {
    serde_json::from_slice(&response.body).expect("valid JSON body")
}

fn extract_token(href: &str) -> String {
    href.split('?')
        .nth(1)
        .expect("next link has a query string")
        .split('&')
        .find_map(|pair| pair.strip_prefix("token="))
        .expect("next link carries a token parameter")
        .to_string()
}

fn next_link(body: &serde_json::Value) -> Option<String> {
    body["links"].as_array().unwrap().iter().find_map(|link| {
        (link["rel"] == "next").then(|| link["href"].as_str().unwrap().to_string())
    })
}

/// The proof, end to end: `/collections` lists the DuckDB-backed collection
/// with its real, auto-detected geometry column and extent; `items` pages
/// across three pages (limit=2 over 5 features) with a stable, non-repeating
/// `id` and an exact `numberMatched`; a `bbox` query narrows correctly; a
/// CQL2 `filter` query narrows correctly through this driver's own basic-
/// comparison SQL pushdown; a single item round-trips by its real integer
/// primary key — all with zero database involvement (the binary this test
/// spawns was built with `postgis` compiled out) and zero network access.
#[test]
fn real_duckdb_binary_serves_collections_paginated_bbox_and_filtered_items_with_no_database_driver()
{
    let env_var = "TELLURION_DUCKDB_BINARY_TEST_FILE";
    let duckdb_path = temp_path("fixture.duckdb");
    provision_duckdb(&duckdb_path);
    let config_path = write_temp_config(env_var);

    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env(env_var, &duckdb_path)
        .env("TELLURION_CONFIG", &config_path)
        .env("PORT", "0");
    let (process, addr, _stderr_log) = common::spawn_server(command);

    let landing = http_get(&addr, "/");
    assert_eq!(landing.status, 200, "landing page should return 200");

    let collections = http_get(&addr, "/public/features/catalogs/default/collections");
    assert_eq!(collections.status, 200, "/collections should return 200");
    let body = json_body(&collections);
    let list = body["collections"].as_array().expect("collections array");
    assert_eq!(list.len(), 1, "exactly the one duckdb-backed collection");
    assert_eq!(list[0]["id"], "demo");
    assert_eq!(
        list[0]["extent"]["spatial"]["bbox"][0],
        serde_json::json!([-4.0, 46.0, 4.0, 54.0]),
        "extent must come from the real table's own geometry, no database involved"
    );
    assert!(
        list[0]["links"]
            .as_array()
            .unwrap()
            .iter()
            .any(|link| link["rel"] == "items"),
        "a features-capable collection must advertise an items link"
    );

    // Page 1: limit=2 over 5 fixture features.
    let page1 = http_get(
        &addr,
        "/public/features/catalogs/default/collections/demo/items?limit=2",
    );
    assert_eq!(page1.status, 200, "items page 1 should return 200");
    assert_eq!(page1.content_type.as_deref(), Some("application/geo+json"));
    let body1 = json_body(&page1);
    assert_eq!(body1["type"], "FeatureCollection");
    assert_eq!(body1["numberReturned"], 2);
    assert_eq!(body1["numberMatched"], 5, "numberMatched is exact");
    let next1 = next_link(&body1).expect("page 1 has a next link");

    let token1 = extract_token(&next1);
    let page2 = http_get(
        &addr,
        &format!("/public/features/catalogs/default/collections/demo/items?limit=2&token={token1}"),
    );
    let body2 = json_body(&page2);
    assert_eq!(body2["numberReturned"], 2);
    let next2 = next_link(&body2).expect("page 2 has a next link");

    let token2 = extract_token(&next2);
    let page3 = http_get(
        &addr,
        &format!("/public/features/catalogs/default/collections/demo/items?limit=2&token={token2}"),
    );
    let body3 = json_body(&page3);
    assert_eq!(body3["numberReturned"], 1);
    assert!(
        next_link(&body3).is_none(),
        "the last page must not advertise a next link"
    );

    let mut ids = std::collections::HashSet::new();
    for body in [&body1, &body2, &body3] {
        for feature in body["features"].as_array().unwrap() {
            assert!(ids.insert(feature["id"].as_str().unwrap().to_string()));
        }
    }
    assert_eq!(ids.len(), 5);

    // A single feature fetched by id round-trips through the collection item
    // route with the same geometry.
    let some_id = body1["features"][0]["id"].as_str().unwrap();
    let item = http_get(
        &addr,
        &format!("/public/features/catalogs/default/collections/demo/items/{some_id}"),
    );
    assert_eq!(item.status, 200);
    let item_body = json_body(&item);
    assert_eq!(item_body["id"], serde_json::json!(some_id));
    assert_eq!(item_body["geometry"], body1["features"][0]["geometry"]);

    // A bbox filter exercises the in-process WKB bbox post-filter end to end.
    let bbox_page = http_get(
        &addr,
        "/public/features/catalogs/default/collections/demo/items?bbox=-5,45,-1,55",
    );
    assert_eq!(bbox_page.status, 200);
    let bbox_body = json_body(&bbox_page);
    let bbox_features = bbox_body["features"].as_array().unwrap();
    assert!(!bbox_features.is_empty());
    assert!(bbox_features.len() < 5);
    for feature in bbox_features {
        let x = feature["geometry"]["coordinates"][0].as_f64().unwrap();
        assert!(x <= -1.0, "feature outside the requested bbox: {feature}");
    }

    // A CQL2 `filter` query narrows to exactly the matching feature, proving
    // this driver's own basic-comparison SQL pushdown end to end.
    let filtered = http_get(
        &addr,
        "/public/features/catalogs/default/collections/demo/items?filter=name%20%3D%20'charlie'",
    );
    assert_eq!(
        filtered.status, 200,
        "a filtered items query should return 200"
    );
    let filtered_body = json_body(&filtered);
    assert_eq!(filtered_body["numberReturned"], 1);
    assert_eq!(
        filtered_body["features"][0]["properties"]["name"],
        "charlie"
    );

    drop(process);
    let _ = std::fs::remove_file(config_path);
    let _ = std::fs::remove_file(&duckdb_path);
}
