//! The GeoParquet features-lane acceptance proof: the real `tellurion`
//! binary, built with the bundled database driver compiled out
//! (`--no-default-features --features geoparquet`), serves `/collections`
//! and paginated GeoJSON items through the abstract driver contract backed
//! by nothing but a local `.parquet` file. Mirrors
//! `tellurion-flatgeobuf`'s own `flatgeobuf_binary.rs` proof, swapping the
//! file format.
//!
//! Run exactly the invocation this proves:
//!
//! ```sh
//! cargo test -p tellurion --no-default-features --features geoparquet
//! cargo tree -p tellurion --no-default-features --features geoparquet -e normal
//! ```
//!
//! The second command is the other half of the proof (checked separately,
//! not by this file): it must list `tellurion-geoparquet`/`parquet` and no
//! `postgres`/`postgis`/`deadpool` crate anywhere in the graph.
//!
//! Gated two ways: `required-features = ["geoparquet"]` in `Cargo.toml`
//! skips building this file entirely under the default feature set, and the
//! inner `#![cfg]` below additionally requires `postgis` to be *off* — see
//! `pmtiles_binary.rs`'s own doc comment for why `required-features` alone
//! can't express that.

#![cfg(all(feature = "geoparquet", not(feature = "postgis")))]

mod common;

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;

use common::{http_get, ServerProcess};
use geozero::mvt::{Message, Tile};

/// The committed GeoParquet fixture lives in `tellurion-geoparquet`'s own
/// test tree (`crates/tellurion-geoparquet/tests/fixtures/tiny.parquet`) —
/// one file, reused by that crate's own tests and this real-binary proof
/// rather than duplicated. Resolved relative to the workspace root
/// regardless of this test binary's own working directory.
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tellurion-geoparquet/tests/fixtures/tiny.parquet")
}

fn write_temp_config(env_var: &str) -> PathBuf {
    let mut path = common::unique_temp_path("tellurion-server-geoparquet-binary-test");
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
    driver: geoparquet
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
    # The fixture file's own physical identity is its file stem ("tiny") —
    # GeoParquet, unlike FlatGeobuf, has no embedded dataset-name field a
    # driver could echo back as "demo" instead (see
    # tellurion-geoparquet's `header_name` doc comment). This override is
    # the documented `table` knob (`CollectionDecl::table`) for exactly this
    # case: physical name decoupled from the operator-facing collection id.
    table: tiny
"#
    );
    std::fs::write(&path, common::legacy_config(&yaml)).expect("writes the throwaway config");
    path
}

/// Builds the command and delegates to [`common::spawn_server`] for the
/// listen-and-wait plumbing.
fn spawn_server(config_path: &PathBuf, env_var: &str) -> (ServerProcess, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env(env_var, fixture_path())
        .env("TELLURION_CONFIG", config_path)
        .env("PORT", "0");
    let (process, addr, _stderr_log) = common::spawn_server(command);
    (process, addr)
}

fn json_body(response: &common::HttpResponse) -> serde_json::Value {
    serde_json::from_slice(&response.body).expect("valid JSON body")
}

/// Pulls the `token` query parameter off a `next` link href, exactly as a
/// real OGC API Features client would follow pagination.
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

/// The proof, end to end: `/collections` lists the GeoParquet-backed
/// collection with its real, "geo"-metadata-derived extent and an `items`
/// link; `/collections/demo/items` pages across three pages (limit=2 over 5
/// fixture features) with a stable, non-repeating `id` per feature and an
/// exact `numberMatched` — all with zero database involvement (the binary
/// this test spawns was built with `postgis` compiled out).
#[test]
fn real_geoparquet_binary_serves_collections_and_paginated_items_with_no_database_driver() {
    let env_var = "TELLURION_GEOPARQUET_BINARY_TEST_FILE";
    let config_path = write_temp_config(env_var);
    let (process, addr) = spawn_server(&config_path, env_var);

    let landing = http_get(&addr, "/");
    assert_eq!(landing.status, 200, "landing page should return 200");

    let collections = http_get(&addr, "/public/features/catalogs/default/collections");
    assert_eq!(collections.status, 200, "/collections should return 200");
    assert_eq!(
        collections.content_type.as_deref(),
        Some("application/json")
    );
    let body = json_body(&collections);
    let list = body["collections"].as_array().expect("collections array");
    assert_eq!(
        list.len(),
        1,
        "exactly the one geoparquet-backed collection"
    );
    assert_eq!(list[0]["id"], "demo");
    assert_eq!(
        list[0]["extent"]["spatial"]["bbox"][0],
        serde_json::json!([-4.0, 46.0, 4.0, 54.0]),
        "extent must come straight from the file's 'geo' metadata bbox, no database involved"
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
    assert_eq!(
        body1["numberMatched"], 5,
        "numberMatched is exact (the file's own row count), not an estimate"
    );
    let features1 = body1["features"].as_array().unwrap();
    assert_eq!(features1.len(), 2);
    for feature in features1 {
        assert_eq!(feature["type"], "Feature");
        assert!(feature["properties"]["name"].is_string());
        assert!(feature["properties"]["value"].is_number());
        assert_eq!(feature["geometry"]["type"], "Point");
    }
    let next1 = next_link(&body1).expect("page 1 has a next link");

    // Page 2, following the real next link.
    let token1 = extract_token(&next1);
    let page2 = http_get(
        &addr,
        &format!("/public/features/catalogs/default/collections/demo/items?limit=2&token={token1}"),
    );
    assert_eq!(page2.status, 200);
    let body2 = json_body(&page2);
    assert_eq!(body2["numberReturned"], 2);
    let next2 = next_link(&body2).expect("page 2 has a next link");

    // Page 3: the final, partial page.
    let token2 = extract_token(&next2);
    let page3 = http_get(
        &addr,
        &format!("/public/features/catalogs/default/collections/demo/items?limit=2&token={token2}"),
    );
    assert_eq!(page3.status, 200);
    let body3 = json_body(&page3);
    assert_eq!(body3["numberReturned"], 1);
    assert!(
        next_link(&body3).is_none(),
        "the last page must not advertise a next link"
    );

    // Every id across the three pages is present exactly once.
    let mut ids = HashSet::new();
    for body in [&body1, &body2, &body3] {
        for feature in body["features"].as_array().unwrap() {
            let id = feature["id"].as_str().unwrap().to_string();
            assert!(ids.insert(id), "an id repeated across pages");
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

    // A bbox filter exercises the row-group-pruning + row-level covering
    // read path end to end through the real HTTP surface.
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

    let tiles = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/demo/tiles",
    );
    assert_eq!(tiles.status, 200, "TileSet list should return 200");
    let tileset = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad",
    );
    assert_eq!(
        tileset.status, 200,
        "WebMercatorQuad TileSet should return 200"
    );

    let tile = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/0/0/0.mvt",
    );
    assert_eq!(tile.status, 200, "covering MVT tile should return 200");
    assert_eq!(
        tile.content_type.as_deref(),
        Some("application/vnd.mapbox-vector-tile")
    );
    let decoded = Tile::decode(tile.body.as_slice())
        .expect("every 200 MVT response is one valid tile document");
    assert_eq!(decoded.layers.len(), 1);
    assert_eq!(decoded.layers[0].name, "demo");

    let empty = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/1/1/0.mvt",
    );
    assert_eq!(empty.status, 204, "valid uncovered tile should be empty");
    assert!(empty.body.is_empty());

    let invalid = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/1/0/2.mvt",
    );
    assert_eq!(invalid.status, 400, "out-of-range tile column is invalid");

    drop(process);
    let _ = std::fs::remove_file(config_path);
}
