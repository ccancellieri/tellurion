use std::sync::Arc;

use serde_json::json;
use tellurion_core::{AppConfig, Error, ItemsQuery, Registry, Router};
use tellurion_memory::{MemoryDataset, MemoryDriver, MemoryDriverFactory};

fn roads() -> MemoryDataset {
    MemoryDataset::from_feature_collection(
        "roads",
        json!({"type": "FeatureCollection", "features": [
            {"type": "Feature", "id": "r1",
             "geometry": {"type": "LineString", "coordinates": [[1, 2], [3, 4]]},
             "properties": {"name": "Main", "lanes": 2}}
        ]}),
    )
    .unwrap()
}

fn config(collection: &str) -> AppConfig {
    let config: AppConfig = serde_yaml::from_str(&format!(
        r#"
storages:
  - id: memory-main
    driver: memory
    url_env: UNUSED
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: roads
    catalog: default
    storage: memory-main
    {collection}
"#
    ))
    .unwrap();
    config.validate().unwrap();
    config
}

fn registry_with_roads() -> Registry {
    let driver = MemoryDriver::new([roads()]).unwrap();
    let mut factory = MemoryDriverFactory::new();
    factory.insert("memory-main", driver).unwrap();
    let mut registry = Registry::new();
    registry.register(Arc::new(factory));
    registry
}

#[tokio::test]
async fn reference_driver_passes_boot_and_exposes_derived_metadata() {
    let config = config("routing: { features: memory-main }");
    let router = Router::build(&config, &registry_with_roads()).unwrap();

    router.validate_catalog().await.unwrap();
    let descriptor = router
        .collection_descriptor("public", "default", "roads")
        .await
        .unwrap();
    assert_eq!(descriptor.table, "roads");
    assert_eq!(descriptor.geometry.as_deref(), Some("geometry"));
    assert_eq!(descriptor.pk.as_deref(), Some("id"));
    assert_eq!(descriptor.extent.unwrap().bbox, [1.0, 2.0, 3.0, 4.0]);
    assert_eq!(descriptor.row_estimate, Some(1));
    let attributes = descriptor.attributes.unwrap();
    assert!(attributes
        .iter()
        .any(|column| column.name == "name" && column.sql_type == "text"));
    assert!(attributes
        .iter()
        .any(|column| column.name == "lanes" && column.sql_type == "bigint"));

    let (decl, features) = router
        .resolve_features("public", "default", "roads")
        .await
        .unwrap();
    assert!(!features.filter_capable());
    // `#105`: this driver rejects CQL2 outright — never overrides
    // `cql2_conformance_classes` either, so its declared set is empty
    // through the full `Router::resolve_features` resolution chain
    // (including the observability wrapper every resolved source passes
    // through).
    assert!(features.cql2_conformance_classes().is_empty());
    assert_eq!(decl.resolved_table(), "roads");
    let page = features.items(&decl, &ItemsQuery::default()).await.unwrap();
    assert_eq!(page.number_matched, Some(1));
}

#[tokio::test]
async fn explicit_tiles_lane_is_rejected_at_boot() {
    let config = config("routing: { features: memory-main, tiles: memory-main }");
    let router = Router::build(&config, &registry_with_roads()).unwrap();

    let error = router.validate_catalog().await.unwrap_err();
    assert!(
        matches!(error, Error::Config(message) if message.contains("routing lane 'tiles'") && message.contains("does not implement the 'tiles' capability"))
    );
}

#[tokio::test]
async fn explicit_write_lane_is_rejected_at_boot() {
    let config = config("routing: { features: memory-main, write: memory-main }");
    let router = Router::build(&config, &registry_with_roads()).unwrap();

    let error = router.validate_catalog().await.unwrap_err();
    assert!(
        matches!(error, Error::Config(message) if message.contains("routing lane 'write'") && message.contains("does not implement the 'write' capability"))
    );
}

#[tokio::test]
async fn absent_physical_collection_is_rejected_at_boot() {
    let config = config("table: missing");
    let router = Router::build(&config, &registry_with_roads()).unwrap();

    let error = router.validate_catalog().await.unwrap_err();
    assert!(
        matches!(error, Error::Config(message) if message.contains("does not report a table named 'missing'"))
    );
}

#[test]
fn factory_build_is_keyed_by_storage_id() {
    let config = config("");
    let registry = Registry::new();
    assert!(matches!(
        Router::build(&config, &registry),
        Err(Error::Config(message)) if message.contains("unknown driver 'memory'")
    ));

    let mut registry = Registry::new();
    registry.register(Arc::new(MemoryDriverFactory::new()));
    assert!(matches!(
        Router::build(&config, &registry),
        Err(Error::Config(message)) if message.contains("memory-main")
    ));
}

#[test]
fn duplicate_preloads_are_configuration_errors() {
    assert!(MemoryDriver::new([roads(), roads()]).is_err());

    let mut factory = MemoryDriverFactory::new();
    factory
        .insert("memory-main", MemoryDriver::new([roads()]).unwrap())
        .unwrap();
    assert!(factory
        .insert("memory-main", MemoryDriver::new([roads()]).unwrap())
        .is_err());
}
