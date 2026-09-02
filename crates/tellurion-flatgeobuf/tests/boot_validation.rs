//! `#20`/`#21` acceptance: this driver never implements `TileSource`, so a
//! collection whose `tiles` lane is explicitly routed to a `flatgeobuf`
//! storage must fail `Router::validate_catalog` at boot with the ordinary
//! missing-capability error — not silently degrade, and not panic later the
//! first time a query-building `TileSource` consumer would have reached it.
//! Exercises the real `FlatgeobufDriverFactory` against the committed test
//! fixture, not a fake driver — `tellurion-core`'s own test suite already
//! covers the capability-gate mechanism generically. Mirrors
//! `tellurion-pmtiles`' own `tests/boot_validation.rs`, with the lane
//! failure/success cases swapped (that driver is tiles-only; this one is
//! features-only).

use std::path::PathBuf;
use std::sync::Arc;

use tellurion_core::{AppConfig, Error, Registry, Router};
use tellurion_flatgeobuf::FlatgeobufDriverFactory;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny.fgb")
}

#[tokio::test]
async fn a_tiles_lane_explicitly_routed_to_flatgeobuf_fails_boot_with_the_missing_capability_error()
{
    let env_var = "TELLURION_FLATGEOBUF_BOOT_VALIDATION_TEST_FILE";
    std::env::set_var(env_var, fixture_path());

    let config: AppConfig = serde_yaml::from_str(&format!(
        r#"
storages: [ {{ id: main, driver: flatgeobuf, url_env: {env_var} }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: demo
    catalog: default
    storage: main
    routing: {{ tiles: main }}
"#
    ))
    .unwrap();
    config.validate().unwrap();

    let mut registry = Registry::new();
    registry.register(Arc::new(FlatgeobufDriverFactory::new()));
    let router = Router::build(&config, &registry).unwrap();

    match router.validate_catalog().await {
        Err(Error::Config(message)) => {
            assert!(message.contains("demo"), "message was: {message}");
            assert!(message.contains("tiles"), "message was: {message}");
            assert!(message.contains("main"), "message was: {message}");
        }
        other => panic!("expected Err(Config(_)), got {other:?}"),
    }

    std::env::remove_var(env_var);
}

/// The mirror-image happy path: a `features`-only collection over the same
/// storage boots clean and serves real GeoJSON — proving the failure above
/// is specifically about the `tiles` lane, not about this driver/file being
/// broken in some other way.
#[tokio::test]
async fn a_features_only_collection_over_the_same_storage_boots_clean() {
    let env_var = "TELLURION_FLATGEOBUF_BOOT_VALIDATION_TEST_FILE_FEATURES_OK";
    std::env::set_var(env_var, fixture_path());

    let config: AppConfig = serde_yaml::from_str(&format!(
        r#"
storages: [ {{ id: main, driver: flatgeobuf, url_env: {env_var} }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: demo
    catalog: default
    storage: main
"#
    ))
    .unwrap();
    config.validate().unwrap();

    let mut registry = Registry::new();
    registry.register(Arc::new(FlatgeobufDriverFactory::new()));
    let router = Router::build(&config, &registry).unwrap();
    router.validate_catalog().await.unwrap();

    let (decl, source) = router
        .resolve_features("public", "default", "demo")
        .await
        .unwrap();
    let page = source
        .items(&decl, &tellurion_core::ItemsQuery::default())
        .await
        .unwrap();
    assert_eq!(page.features_geojson.len(), 5);

    std::env::remove_var(env_var);
}
