//! End-to-end proof that the optional Shapefile driver serves Features and
//! WebMercatorQuad MVT without the database driver.

#![cfg(all(feature = "shapefile", not(feature = "postgis")))]

mod common;

use std::{io::Write, path::PathBuf, process::Command};

use common::{http_get, ServerProcess};
use geozero::mvt::{Message, Tile};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

struct Fixture {
    _directory: tempfile::TempDir,
    path: PathBuf,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("fixture directory");
    let path = directory.path().join("dataset.zip");
    std::fs::write(&path, fixture_archive()).expect("fixture archive");
    Fixture {
        _directory: directory,
        path,
    }
}

fn write_temp_config(env_var: &str) -> PathBuf {
    let mut path = common::unique_temp_path("tellurion-server-shapefile-binary-test");
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
    driver: shapefile
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
    table: dataset
"#
    );
    std::fs::write(&path, common::legacy_config(&yaml)).expect("throwaway config");
    path
}

fn spawn_server(config_path: &PathBuf, env_var: &str) -> (ServerProcess, String, Fixture) {
    let fixture = fixture();
    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env(env_var, &fixture.path)
        .env("TELLURION_CONFIG", config_path)
        .env("PORT", "0");
    let (process, address, _stderr_log) = common::spawn_server(command);
    (process, address, fixture)
}

#[test]
fn real_shapefile_binary_serves_features_and_mvt_with_no_database_driver() {
    let env_var = "TELLURION_SHAPEFILE_BINARY_TEST_FILE";
    let config_path = write_temp_config(env_var);
    let (process, address, _fixture) = spawn_server(&config_path, env_var);

    let collections = http_get(&address, "/public/features/catalogs/default/collections");
    assert_eq!(collections.status, 200);
    let collections_body: serde_json::Value =
        serde_json::from_slice(&collections.body).expect("collections JSON");
    assert_eq!(collections_body["collections"][0]["id"], "demo");

    let items = http_get(
        &address,
        "/public/features/catalogs/default/collections/demo/items?limit=1",
    );
    assert_eq!(items.status, 200);
    assert_eq!(items.content_type.as_deref(), Some("application/geo+json"));
    let items_body: serde_json::Value = serde_json::from_slice(&items.body).expect("items JSON");
    assert_eq!(items_body["numberReturned"], 1);
    assert_eq!(items_body["numberMatched"], 2);
    assert_eq!(items_body["features"][0]["id"], "0");
    assert_eq!(items_body["features"][0]["geometry"]["type"], "Point");
    assert_eq!(
        items_body["features"][0]["properties"]["name"],
        "fixture-one"
    );
    assert!(items_body["links"]
        .as_array()
        .expect("items links")
        .iter()
        .any(|link| link["rel"] == "next" && link["href"].is_string()));

    let tile = http_get(
        &address,
        "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/0/0/0.mvt",
    );
    assert_eq!(tile.status, 200);
    assert_eq!(
        tile.content_type.as_deref(),
        Some("application/vnd.mapbox-vector-tile")
    );
    let decoded = Tile::decode(tile.body.as_slice()).expect("valid MVT");
    assert_eq!(decoded.layers.len(), 1);
    assert_eq!(decoded.layers[0].name, "demo");

    let empty = http_get(
        &address,
        "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/1/1/0.mvt",
    );
    assert_eq!(empty.status, 204, "valid uncovered coordinate is empty");

    let invalid = http_get(
        &address,
        "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/1/0/2.mvt",
    );
    assert_eq!(invalid.status, 400, "out-of-range coordinate is invalid");

    drop(process);
    let _ = std::fs::remove_file(config_path);
}

fn fixture_archive() -> Vec<u8> {
    let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, bytes) in [
        ("dataset.shp", point_shp()),
        ("dataset.shx", point_shx()),
        ("dataset.dbf", point_dbf()),
        (
            "dataset.prj",
            b"GEOGCS[\"WGS 84\",AUTHORITY[\"EPSG\",\"4326\"]]".to_vec(),
        ),
    ] {
        writer.start_file(name, options).expect("ZIP member");
        writer.write_all(&bytes).expect("ZIP member bytes");
    }
    writer.finish().expect("finished ZIP").into_inner()
}

fn point_shp() -> Vec<u8> {
    let mut shp = header(156, 1, [10.0, 10.0, 11.0, 11.0]);
    for (record, x, y) in [(1_u32, 10.0_f64, 10.0_f64), (2, 11.0, 11.0)] {
        shp.extend_from_slice(&record.to_be_bytes());
        shp.extend_from_slice(&10_u32.to_be_bytes());
        shp.extend_from_slice(&1_i32.to_le_bytes());
        shp.extend_from_slice(&x.to_le_bytes());
        shp.extend_from_slice(&y.to_le_bytes());
    }
    shp
}

fn point_shx() -> Vec<u8> {
    let mut shx = header(116, 1, [10.0, 10.0, 11.0, 11.0]);
    for offset in [50_u32, 64] {
        shx.extend_from_slice(&offset.to_be_bytes());
        shx.extend_from_slice(&10_u32.to_be_bytes());
    }
    shx
}

fn header(byte_length: usize, type_code: i32, bbox: [f64; 4]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(100);
    bytes.extend_from_slice(&9994_i32.to_be_bytes());
    bytes.extend_from_slice(&[0; 20]);
    bytes.extend_from_slice(
        &u32::try_from(byte_length / 2)
            .expect("file length")
            .to_be_bytes(),
    );
    bytes.extend_from_slice(&1000_i32.to_le_bytes());
    bytes.extend_from_slice(&type_code.to_le_bytes());
    for value in bbox {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&[0; 32]);
    bytes
}

fn point_dbf() -> Vec<u8> {
    let mut dbf = vec![0x03, 126, 1, 1];
    dbf.extend_from_slice(&2_u32.to_le_bytes());
    dbf.extend_from_slice(&65_u16.to_le_bytes());
    dbf.extend_from_slice(&21_u16.to_le_bytes());
    dbf.extend_from_slice(&[0; 20]);
    let mut field = [0_u8; 32];
    field[..4].copy_from_slice(b"name");
    field[11] = b'C';
    field[16] = 20;
    dbf.extend_from_slice(&field);
    dbf.push(0x0d);
    for name in [b"fixture-one".as_slice(), b"fixture-two".as_slice()] {
        dbf.push(b' ');
        dbf.extend_from_slice(name);
        dbf.extend(std::iter::repeat_n(b' ', 20 - name.len()));
    }
    dbf.push(0x1a);
    dbf
}
