//! The `#18` acceptance proof: the real `tellurion` binary, built with the
//! bundled database driver compiled out (`--no-default-features --features
//! pmtiles`), serves `/collections` and a tile through the abstract driver
//! contract backed by nothing but a local PMTiles archive.
//!
//! Run exactly the invocation this proves:
//!
//! ```sh
//! cargo test -p tellurion --no-default-features --features pmtiles
//! cargo tree -p tellurion --no-default-features --features pmtiles -e normal
//! ```
//!
//! The second command is the other half of the proof (checked separately,
//! not by this file): it must list `tellurion-pmtiles`/`pmtiles` and no
//! `postgres`/`postgis`/`deadpool` crate anywhere in the graph.
//!
//! Gated two ways: `required-features = ["pmtiles"]` in `Cargo.toml` skips
//! building this file entirely under the default feature set (nothing to do
//! without a `pmtiles` driver registered), and the inner `#![cfg]` below
//! additionally requires `postgis` to be *off* — `required-features` alone
//! can only require a feature, never its absence, so plain `cargo test
//! --features pmtiles` (postgis still default-on) would otherwise silently
//! run this against a binary that still carries the database driver,
//! defeating the point of the proof.

#![cfg(all(feature = "pmtiles", not(feature = "postgis")))]

mod common;

use std::path::PathBuf;
use std::process::Command;

use geozero::mvt::{tile, Message, Tile};

use common::{http_get, ServerProcess};

/// The committed PMTiles fixture lives in `tellurion-pmtiles`'s own test
/// tree (`crates/tellurion-pmtiles/tests/fixtures/tiny.pmtiles`) — one
/// archive, reused by that crate's own tests and this real-binary proof
/// rather than duplicated. Resolved relative to the workspace root
/// regardless of this test binary's own working directory.
fn fixture_archive_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tellurion-pmtiles/tests/fixtures/tiny.pmtiles")
}

/// `tests/fixtures/tiny.pmtiles` was written by
/// `tellurion-pmtiles/examples/gen_fixture.rs` with three tiles, each a
/// single-point MVT layer named after the tile it's in — reproduced here
/// (same construction as that generator and as tellurion-tiles' own test
/// suite) so this test can assert the served bytes are the exact real tile,
/// not just "some non-empty response".
fn expected_mvt_tile(layer_name: &str) -> Vec<u8> {
    let mut layer = tile::Layer {
        version: 2,
        name: layer_name.to_string(),
        extent: Some(4096),
        ..Default::default()
    };
    let mut feature = tile::Feature {
        geometry: vec![9, 50, 34],
        ..Default::default()
    };
    feature.set_type(tile::GeomType::Point);
    layer.features.push(feature);
    Tile {
        layers: vec![layer],
    }
    .encode_to_vec()
}

fn write_temp_config(env_var: &str) -> PathBuf {
    let mut path = common::unique_temp_path("tellurion-server-pmtiles-binary-test");
    path.set_extension("yaml");
    // `log_json: true` gives the startup log a machine-parseable line, same
    // reason `tests/binary.rs` sets it.
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
    driver: pmtiles
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
    tiles: {{ minzoom: 0, maxzoom: 2, caps: {{}} }}
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
        .env(env_var, fixture_archive_path())
        .env("TELLURION_CONFIG", config_path)
        .env("PORT", "0");
    let (process, addr, _stderr_log) = common::spawn_server(command);
    (process, addr)
}

/// The proof, end to end: `/collections` lists the PMTiles-backed collection
/// with its real, header-derived extent, and its tiles lane serves the real
/// MVT bytes for an addressed coordinate — all with zero database
/// involvement (the binary this test spawns was built with `postgis`
/// compiled out).
#[test]
fn real_pmtiles_binary_serves_collections_and_a_real_tile_with_no_database_driver() {
    let env_var = "TELLURION_PMTILES_BINARY_TEST_ARCHIVE";
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
    let body: serde_json::Value =
        serde_json::from_slice(&collections.body).expect("valid JSON body");
    let list = body["collections"].as_array().expect("collections array");
    assert_eq!(list.len(), 1, "exactly the one pmtiles-backed collection");
    assert_eq!(list[0]["id"], "demo");
    assert_eq!(
        list[0]["extent"]["spatial"]["bbox"][0],
        serde_json::json!([-5.0, 45.0, 5.0, 55.0]),
        "extent must come straight from the archive header, no database involved"
    );
    assert!(
        list[0]["links"]
            .as_array()
            .unwrap()
            .iter()
            .all(|link| link["rel"] != "items"),
        "a tiles-only collection must not advertise an items link"
    );

    let tile = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/0/0/0.mvt",
    );
    assert_eq!(tile.status, 200, "the addressed z0 tile should return 200");
    assert_eq!(
        tile.content_type.as_deref(),
        Some("application/vnd.mapbox-vector-tile")
    );
    assert_eq!(
        tile.body,
        expected_mvt_tile("world"),
        "served bytes must be the exact, decompressed MVT tile the fixture archive carries"
    );

    let leaf_tile = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/2/1/2.mvt",
    );
    assert_eq!(leaf_tile.status, 200);
    assert_eq!(leaf_tile.body, expected_mvt_tile("leaf"));

    // A valid coordinate this archive never addressed comes back empty
    // (204), the same empty-tile semantics every other driver's TileSource
    // uses — not a 500 or a 404.
    let missing_tile = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/2/0/0.mvt",
    );
    assert_eq!(
        missing_tile.status, 204,
        "an in-range but never-addressed coordinate must come back empty"
    );

    drop(process);
    let _ = std::fs::remove_file(config_path);
}
