//! Boot validation for GeoParquet feature and tile lanes. The committed
//! fixture resolves to CRS84/EPSG:4326, so its Tiles lane is honest.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use arrow_array::{ArrayRef, BinaryArray, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use geozero::GeozeroGeometry;
use parquet::arrow::ArrowWriter;
use parquet::file::metadata::KeyValue;
use parquet::file::properties::WriterProperties;
use tellurion_core::{AppConfig, Error, Registry, Router, TileMatrixSet};
use tellurion_geoparquet::GeoparquetDriverFactory;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny.parquet")
}

fn crs_fixture(crs: serde_json::Value) -> (PathBuf, String) {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let name = format!("tellurion-geoparquet-tile-crs-{stamp}");
    let path = std::env::temp_dir().join(format!("{name}.parquet"));
    let mut wkb = Vec::new();
    let mut wkb_writer = geozero::wkb::WkbWriter::new(&mut wkb, geozero::wkb::WkbDialect::Wkb);
    geozero::geojson::GeoJson(r#"{"type":"Point","coordinates":[0.0,0.0]}"#)
        .process_geom(&mut wkb_writer)
        .unwrap();
    let schema = Arc::new(Schema::new(vec![Field::new(
        "geometry",
        DataType::Binary,
        false,
    )]));
    let batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![Arc::new(BinaryArray::from(vec![wkb.as_slice()])) as ArrayRef],
    )
    .unwrap();
    let geo = serde_json::json!({
        "version": "1.1.0",
        "primary_column": "geometry",
        "columns": { "geometry": { "encoding": "WKB", "crs": crs } }
    })
    .to_string();
    let properties = WriterProperties::builder()
        .set_key_value_metadata(Some(vec![KeyValue::new("geo".to_string(), geo)]))
        .build();
    let file = std::fs::File::create(&path).unwrap();
    let mut writer = ArrowWriter::try_new(file, schema, Some(properties)).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
    (path, name)
}

#[tokio::test]
async fn a_crs84_tiles_lane_explicitly_routed_to_geoparquet_boots_cleanly() {
    let env_var = "TELLURION_GEOPARQUET_BOOT_VALIDATION_TEST_FILE";
    std::env::set_var(env_var, fixture_path());

    let config: AppConfig = serde_yaml::from_str(&format!(
        r#"
storages: [ {{ id: main, driver: geoparquet, url_env: {env_var} }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: tiny
    catalog: default
    storage: main
    routing: {{ tiles: main }}
"#
    ))
    .unwrap();
    config.validate().unwrap();

    let mut registry = Registry::new();
    registry.register(Arc::new(GeoparquetDriverFactory::new()));
    let router = Router::build(&config, &registry).unwrap();

    router.validate_catalog().await.unwrap();
    let (decl, source) = router
        .resolve_tiles("public", "default", "tiny")
        .await
        .unwrap();
    assert_eq!(decl.srid, Some(4326));
    assert!(source.supports_tile_matrix_set(TileMatrixSet::WebMercatorQuad));
    assert!(!source.supports_tile_matrix_set(TileMatrixSet::WorldCrs84Quad));

    std::env::remove_var(env_var);
}

/// A features-only collection over the same storage remains unchanged.
#[tokio::test]
async fn a_features_only_collection_over_the_same_storage_boots_clean() {
    let env_var = "TELLURION_GEOPARQUET_BOOT_VALIDATION_TEST_FILE_FEATURES_OK";
    std::env::set_var(env_var, fixture_path());

    let config: AppConfig = serde_yaml::from_str(&format!(
        r#"
storages: [ {{ id: main, driver: geoparquet, url_env: {env_var} }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: tiny
    catalog: default
    storage: main
"#
    ))
    .unwrap();
    config.validate().unwrap();

    let mut registry = Registry::new();
    registry.register(Arc::new(GeoparquetDriverFactory::new()));
    let router = Router::build(&config, &registry).unwrap();
    router.validate_catalog().await.unwrap();

    let (decl, source) = router
        .resolve_features("public", "default", "tiny")
        .await
        .unwrap();
    let page = source
        .items(&decl, &tellurion_core::ItemsQuery::default())
        .await
        .unwrap();
    assert_eq!(page.features_geojson.len(), 5);

    std::env::remove_var(env_var);
}

#[tokio::test]
async fn metadata_resolved_projected_and_unknown_crs_cannot_boot_or_resolve_a_tiles_lane() {
    for crs in [
        serde_json::json!({ "id": { "authority": "EPSG", "code": 3857 } }),
        serde_json::json!({ "id": { "authority": "OGC", "code": "CRS84" } }),
    ] {
        let (path, table) = crs_fixture(crs);
        let env_var = "TELLURION_GEOPARQUET_BOOT_VALIDATION_UNSUPPORTED_CRS";
        std::env::set_var(env_var, &path);
        let config: AppConfig = serde_yaml::from_str(&format!(
            r#"
storages: [ {{ id: main, driver: geoparquet, url_env: {env_var} }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: {table}
    routing: {{ tiles: main }}
"#
        ))
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(GeoparquetDriverFactory::new()));
        let router = Router::build(&config, &registry).unwrap();

        assert!(matches!(
            router.validate_catalog().await,
            Err(Error::Config(_))
        ));
        assert!(matches!(
            router.resolve_tiles("public", "default", "demo").await,
            Err(Error::CapabilityUnsupported { ref capability, .. }) if capability == "tiles"
        ));

        std::env::remove_var(env_var);
        std::fs::remove_file(path).unwrap();
    }
}
