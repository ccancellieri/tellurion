//! The `#20` acceptance proof: the real `tellurion` binary, built with the
//! bundled database driver compiled out (`--no-default-features --features
//! flatgeobuf`), serves `/collections` and paginated GeoJSON items through
//! the abstract driver contract backed by nothing but a local `.fgb` file.
//! Mirrors `tellurion-pmtiles`' own `pmtiles_binary.rs` proof, adapted to
//! the features lane.
//!
//! Run exactly the invocation this proves:
//!
//! ```sh
//! cargo test -p tellurion --no-default-features --features flatgeobuf
//! cargo tree -p tellurion --no-default-features --features flatgeobuf -e normal
//! ```
//!
//! The second command is the other half of the proof (checked separately,
//! not by this file): it must list `tellurion-flatgeobuf`/`flatgeobuf` and
//! no `postgres`/`postgis`/`deadpool` crate anywhere in the graph.
//!
//! Gated two ways: `required-features = ["flatgeobuf"]` in `Cargo.toml`
//! skips building this file entirely under the default feature set, and the
//! inner `#![cfg]` below additionally requires `postgis` to be *off* — see
//! `pmtiles_binary.rs`'s own doc comment for why `required-features` alone
//! can't express that.

#![cfg(all(feature = "flatgeobuf", not(feature = "postgis")))]

mod common;

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;

use common::{http_get, ServerProcess};

/// The committed FlatGeobuf fixture lives in `tellurion-flatgeobuf`'s own
/// test tree (`crates/tellurion-flatgeobuf/tests/fixtures/tiny.fgb`) — one
/// file, reused by that crate's own tests and this real-binary proof rather
/// than duplicated. Resolved relative to the workspace root regardless of
/// this test binary's own working directory.
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tellurion-flatgeobuf/tests/fixtures/tiny.fgb")
}

fn write_temp_config(env_var: &str) -> PathBuf {
    let mut path = common::unique_temp_path("tellurion-server-flatgeobuf-binary-test");
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
    driver: flatgeobuf
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

/// The proof, end to end: `/collections` lists the FlatGeobuf-backed
/// collection with its real, header-derived extent and an `items` link;
/// `/collections/demo/items` pages across three pages (limit=2 over 5
/// fixture features) with a stable, non-repeating `id` per feature and an
/// exact `numberMatched` — all with zero database involvement (the binary
/// this test spawns was built with `postgis` compiled out).
#[test]
fn real_flatgeobuf_binary_serves_collections_and_paginated_items_with_no_database_driver() {
    let env_var = "TELLURION_FLATGEOBUF_BINARY_TEST_FILE";
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
        "exactly the one flatgeobuf-backed collection"
    );
    assert_eq!(list[0]["id"], "demo");
    assert_eq!(
        list[0]["extent"]["spatial"]["bbox"][0],
        serde_json::json!([-4.0, 46.0, 4.0, 54.0]),
        "extent must come straight from the fgb header envelope, no database involved"
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
        "numberMatched is exact (the fgb header's own features_count), not an estimate"
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

    drop(process);
    let _ = std::fs::remove_file(config_path);
}
