//! `#20`/`#21` acceptance: this driver never implements `FeatureSource`, so a
//! collection whose `features` lane is explicitly routed to a `pmtiles`
//! storage must fail `Router::validate_catalog` at boot with the ordinary
//! missing-capability error — not silently degrade, and not panic later the
//! first time a query-building `FeatureSource` consumer would have reached
//! it. Exercises the real `PmtilesDriverFactory` against the committed test
//! archive, not a fake driver — `tellurion-core`'s own test suite already
//! covers the capability-gate mechanism generically.

use std::path::PathBuf;
use std::sync::Arc;

use tellurion_core::{AppConfig, Error, Registry, Router};
use tellurion_pmtiles::PmtilesDriverFactory;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny.pmtiles")
}

#[tokio::test]
async fn a_features_lane_explicitly_routed_to_pmtiles_fails_boot_with_the_missing_capability_error()
{
    let env_var = "TELLURION_PMTILES_BOOT_VALIDATION_TEST_ARCHIVE";
    std::env::set_var(env_var, fixture_path());

    let config: AppConfig = serde_yaml::from_str(&format!(
        r#"
storages: [ {{ id: main, driver: pmtiles, url_env: {env_var} }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: demo
    catalog: default
    storage: main
    routing: {{ features: main }}
"#
    ))
    .unwrap();
    config.validate().unwrap();

    let mut registry = Registry::new();
    registry.register(Arc::new(PmtilesDriverFactory::new()));
    let router = Router::build(&config, &registry).unwrap();

    match router.validate_catalog().await {
        Err(Error::Config(message)) => {
            assert!(message.contains("demo"), "message was: {message}");
            assert!(message.contains("features"), "message was: {message}");
            assert!(message.contains("main"), "message was: {message}");
        }
        other => panic!("expected Err(Config(_)), got {other:?}"),
    }

    std::env::remove_var(env_var);
}

/// The mirror-image happy path: a `tiles`-only collection over the same
/// storage boots clean and serves a real tile — proving the failure above is
/// specifically about the `features` lane, not about this driver/archive
/// being broken in some other way.
#[tokio::test]
async fn a_tiles_only_collection_over_the_same_storage_boots_clean() {
    let env_var = "TELLURION_PMTILES_BOOT_VALIDATION_TEST_ARCHIVE_TILES_OK";
    std::env::set_var(env_var, fixture_path());

    let config: AppConfig = serde_yaml::from_str(&format!(
        r#"
storages: [ {{ id: main, driver: pmtiles, url_env: {env_var} }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: demo
    catalog: default
    storage: main
    tiles: {{ minzoom: 0, maxzoom: 2, caps: {{}} }}
"#
    ))
    .unwrap();
    config.validate().unwrap();

    let mut registry = Registry::new();
    registry.register(Arc::new(PmtilesDriverFactory::new()));
    let router = Router::build(&config, &registry).unwrap();
    router.validate_catalog().await.unwrap();

    let (decl, source) = router
        .resolve_tiles("public", "default", "demo")
        .await
        .unwrap();
    let tile = source
        .mvt_tile(&decl, tellurion_core::TileCoord { z: 0, x: 0, y: 0 }, None)
        .await
        .unwrap();
    assert!(tile.is_some_and(|bytes| !bytes.is_empty()));

    std::env::remove_var(env_var);
}
