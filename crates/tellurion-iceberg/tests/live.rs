//! Live validation against a REAL, external Iceberg REST catalog (`#123`).
//!
//! Every other test in this crate builds its own catalog and its own table,
//! hermetically, in-process. That is the right default and stays the
//! default — but it means every claim this driver makes is proven against
//! fixtures this repository also wrote. This file is the one place the
//! driver meets a REST catalog implementation nobody here controls
//! (Polaris, Nessie, Lakekeeper, Tabular, a Spark REST catalog, …), so that
//! divergences between a real server's behaviour and our fixtures surface
//! as a test failure rather than as a production incident.
//!
//! **Skipped unless [`LOCATOR_ENV_VAR`] is set.** Skipped means PASSED, not
//! failed: `cargo test` must never need a network service, and CI has none
//! — exactly the shape `tellurion-postgis`'s own `tests/live.rs` uses for a
//! real PostGIS instance.
//!
//! To run it, point that variable at a complete iceberg locator for a table
//! the catalog already serves — the same string production would hold in a
//! storage's `url_env` (see `location.rs` for the grammar):
//!
//! ```text
//! export TELLURION_ICEBERG_LIVE_TEST_LOCATOR='https://catalog.example.com?namespace=geo&table=points&geometry=geom&bbox=xmin,ymin,xmax,ymax'
//! # …plus, for a table whose files live on S3:
//! #   &s3_endpoint=https://s3.eu-west-1.amazonaws.com&s3_region=eu-west-1
//! #   &s3_access_key_env=AWS_ACCESS_KEY_ID&s3_secret_key_env=AWS_SECRET_ACCESS_KEY
//! cargo test -p tellurion-iceberg --test live -- --nocapture
//! ```
//!
//! The table is READ, never written and never created: this driver has no
//! write path and no DDL, and pointing it at a catalog does not give it
//! one. Bring your own table.
//!
//! What is asserted is deliberately narrow — everything a driver can know
//! about an arbitrary table it has never seen:
//!
//! - the catalog resolves the table and this driver loads it (which is
//!   already most of the REST protocol: `GET /v1/config`, then
//!   `GET /v1/namespaces/{ns}/tables/{table}`, then the whole `FileIO`
//!   chain — manifest list, manifests, data files);
//! - the declared geometry/bbox columns validate against the REAL schema;
//! - `items` returns features whose shape matches the contract every other
//!   driver in this workspace upholds;
//! - a served id round-trips through `item` to the same feature.
//!
//! Row counts, extents and attribute names are NOT asserted: they are
//! properties of whatever table you pointed this at, not of this driver.

use std::env;

use tellurion_core::{CollectionDecl, DriverFactory, ItemsQuery, StorageDecl};
use tellurion_iceberg::IcebergDriverFactory;

/// Holds the complete locator, exactly as `StorageDecl.url_env` would in
/// production — so this test exercises the real
/// `DriverFactory::build` env-var path rather than constructing a location
/// some other way.
const LOCATOR_ENV_VAR: &str = "TELLURION_ICEBERG_LIVE_TEST_LOCATOR";

/// `Some(decl)` when a live catalog is configured, `None` (with a printed
/// reason) otherwise. Returning `None` is what makes an unconfigured run a
/// PASS.
fn live_storage(test_name: &str) -> Option<StorageDecl> {
    if env::var(LOCATOR_ENV_VAR).is_err() {
        eprintln!("skipping {test_name}: {LOCATOR_ENV_VAR} not set");
        return None;
    }
    Some(StorageDecl {
        id: "iceberg-live".to_string(),
        driver: "iceberg".to_string(),
        url_env: LOCATOR_ENV_VAR.to_string(),
        pool_size: None,
    })
}

fn collection_decl() -> CollectionDecl {
    // `id` is the only field this driver reads: table identity comes from
    // the locator, not from here (`driver.rs`'s own note on the same
    // parameter).
    serde_yaml::from_str("id: live\ncatalog: default\nstorage: iceberg-live\n")
        .expect("a minimal collection declaration")
}

#[tokio::test]
async fn a_real_rest_catalog_resolves_a_table_this_driver_can_describe_and_serve() {
    let Some(decl) =
        live_storage("a_real_rest_catalog_resolves_a_table_this_driver_can_describe_and_serve")
    else {
        return;
    };

    let driver = IcebergDriverFactory::new()
        .build(&decl)
        .expect("the locator parses and the driver builds");
    let catalog = driver.catalog_source();

    // Load: the REST protocol, the whole `FileIO` chain, and the
    // declared-column validation against the catalog's own schema.
    let collections = catalog
        .collections()
        .await
        .expect("the live catalog resolves the declared table");
    assert_eq!(
        collections.len(),
        1,
        "this driver always reports exactly the one declared table"
    );
    let physical = &collections[0];
    assert!(
        physical.geometry_column.is_some(),
        "the declared geometry column validated against the live schema"
    );

    // Optional facts stay optional: a real table may or may not carry a
    // `total-records` summary or bbox statistics, and this driver reports
    // `None` rather than inventing either. Asserting only that asking is
    // safe is the honest assertion here.
    let _row_estimate = catalog
        .row_estimate(physical)
        .await
        .expect("row_estimate answers, even if with None");
    let _extent = catalog
        .extent(physical)
        .await
        .expect("extent answers, even if with None");

    let features = driver
        .feature_source()
        .expect("the iceberg driver always offers the feature capability");
    let collection = collection_decl();
    let page = features
        .items(
            &collection,
            &ItemsQuery {
                limit: 5,
                ..Default::default()
            },
        )
        .await
        .expect("the live table serves an items page");

    if page.features_geojson.is_empty() {
        eprintln!(
            "note: the live table served zero features; the round-trip assertion below needs at \
             least one row and is skipped"
        );
        return;
    }

    for feature in &page.features_geojson {
        assert_eq!(feature["type"], serde_json::json!("Feature"));
        assert!(feature["id"].is_string(), "got: {feature}");
        assert!(feature["geometry"]["type"].is_string(), "got: {feature}");
    }

    // The id/cursor contract: an id harvested from a listing resolves
    // through `item` to the same feature. This is the claim most likely to
    // break against a real writer's file layout, which is exactly why it is
    // the one asserted here.
    let id = page.features_geojson[0]["id"]
        .as_str()
        .expect("a string feature id")
        .to_string();
    let looked_up = features
        .item(&collection, &id, None)
        .await
        .expect("item resolves an id this driver itself just minted")
        .expect("the id round-trips to a feature");
    assert_eq!(looked_up["id"], page.features_geojson[0]["id"]);
    assert_eq!(looked_up["geometry"], page.features_geojson[0]["geometry"]);
}
