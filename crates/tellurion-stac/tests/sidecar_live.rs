//! End-to-end live proof of the per-item STAC metadata sidecar (`#202`):
//! a real PostGIS table plus a real `"<table>_stac"` sidecar, served through
//! the real `PostgisDriverFactory` and the real axum router this crate
//! exports. Skipped gracefully unless `TELLURION_TEST_DATABASE_URL` is set,
//! same convention `tests/live.rs` follows.
//!
//! What this proves that `tests/sidecar.rs`'s in-memory fake can't: the
//! whole chain — `CollectionDecl::stac_metadata` -> `Router::
//! resolve_stac_metadata` -> the driver's batched `feature_id = ANY($1)`
//! query against a genuinely provisioned table -> `to_stac_item`'s merge —
//! lines up on real ids and real `jsonb`, including the id type the
//! features lane actually emits (a bigserial pk arrives as the string
//! `"1"`, which is exactly what must key the sidecar table).
//!
//! The sidecar DDL below is hand-kept in sync with
//! `tellurion-ingest::stac`'s own `create_stac_table_sql`, the same
//! arrangement `tellurion-postgis`'s live tests document.

use std::env;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use serde_json::{json, Value};
use tower::ServiceExt;

use tellurion_core::{
    AppConfig, AppContext, FileStyleStore, MokaTileCache, Registry, Resolver, Router as CoreRouter,
    StaticResolver, StyleStore, TileCache,
};
use tellurion_postgis::test_harness;
use tellurion_postgis::PostgisDriverFactory;

const URL_ENV_VAR: &str = "TELLURION_STAC_SIDECAR_LIVE_TEST_URL";

async fn seed(database_url: &str, table: &str) {
    let client = test_harness::connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table};
             DROP TABLE IF EXISTS {table}_stac;
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326) NOT NULL,
                 name text
             );
             INSERT INTO {table} (geom, name) VALUES
                 (ST_SetSRID(ST_MakePoint(1, 1), 4326), 'from-feature'),
                 (ST_SetSRID(ST_MakePoint(2, 2), 4326), 'untouched');
             CREATE TABLE {table}_stac (
                 feature_id text PRIMARY KEY,
                 version bigint NOT NULL DEFAULT 0,
                 doc jsonb NOT NULL,
                 updated_at timestamptz NOT NULL DEFAULT now()
             );
             CREATE INDEX IF NOT EXISTS {table}_stac_version_idx ON {table}_stac (version);
             INSERT INTO {table}_stac (feature_id, version, doc) VALUES
                 ('1', 1, '{{\"stac_extensions\": [\"https://example.test/eo.json\"], \
                            \"properties\": {{\"name\": \"from-sidecar\", \
                                              \"datetime\": \"2021-05-05T00:00:00Z\"}}}}'::jsonb);
             ANALYZE {table};"
        ),
    )
    .await
    .expect("seeds the test table and its sidecar");
}

fn build_config_yaml(table: &str, stac_metadata: bool) -> String {
    format!(
        r#"
storages: [ {{ id: main, driver: postgis, url_env: {URL_ENV_VAR} }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: {table}
    geometry: geom
    pk: id
    stac_metadata: {stac_metadata}
"#
    )
}

fn build_app(config: &AppConfig, registry: &Registry) -> axum::Router {
    let core_router = CoreRouter::build(config, registry).unwrap();
    let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
    let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
    let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(config));
    let ctx = Arc::new(AppContext::new(
        config.clone(),
        core_router,
        resolver,
        None,
        cache,
        style_store,
    ));
    tellurion_stac::router().with_state(ctx)
}

async fn get(app: &axum::Router, uri: &str) -> Response {
    app.clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// One test in this file, deliberately: it owns the single `URL_ENV_VAR`
/// writer, the same safety argument `tests/live.rs` documents for its own.
/// It covers both halves of the acceptance criteria by serving the SAME
/// seeded table twice — once with the opt-in, once without.
#[tokio::test]
async fn a_provisioned_sidecar_merges_into_live_items_and_is_inert_without_the_opt_in() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping a_provisioned_sidecar_merges_into_live_items_and_is_inert_without_the_opt_in: \
             TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_stac_sidecar_live_test";
    seed(&database_url, table).await;

    // Safety: this file has exactly one `#[tokio::test]`, so there is only
    // ever one writer of this env var — same argument `tests/live.rs` makes.
    unsafe {
        env::set_var(URL_ENV_VAR, &database_url);
    }

    let mut registry = Registry::new();
    registry.register(Arc::new(PostgisDriverFactory::new(60)));

    // -- opted in: the sidecar merges, with sidecar precedence -------------
    let config: AppConfig = serde_yaml::from_str(&build_config_yaml(table, true)).unwrap();
    config.validate().unwrap();
    let app = build_app(&config, &registry);

    let response = get(&app, "/collections/demo/items").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let merged = &body["features"][0];
    assert_eq!(merged["id"], "1");
    assert_eq!(
        merged["properties"]["name"], "from-sidecar",
        "the sidecar wins over the feature's own colliding property: {merged}"
    );
    assert_eq!(merged["properties"]["datetime"], "2021-05-05T00:00:00Z");
    assert_eq!(
        merged["stac_extensions"],
        json!(["https://example.test/eo.json"])
    );
    // The row with no sidecar entry is untouched, in the same page.
    let untouched = &body["features"][1];
    assert_eq!(untouched["id"], "2");
    assert_eq!(untouched["properties"]["name"], "untouched");
    assert!(untouched["properties"]["datetime"].is_null());
    assert!(untouched.get("stac_extensions").is_none());

    // The single-item lane merges identically.
    let single = body_json(get(&app, "/collections/demo/items/1").await).await;
    assert_eq!(single["properties"]["name"], "from-sidecar");

    // -- not opted in: byte-identical to a deployment with no sidecar ------
    let plain_config: AppConfig = serde_yaml::from_str(&build_config_yaml(table, false)).unwrap();
    plain_config.validate().unwrap();
    let plain_app = build_app(&plain_config, &registry);

    let plain = body_json(get(&plain_app, "/collections/demo/items").await).await;
    assert_eq!(
        plain["features"][0]["properties"]["name"], "from-feature",
        "a collection with no stac_metadata opt-in must ignore a sidecar table that exists"
    );
    assert!(plain["features"][0]["properties"]["datetime"].is_null());
    assert!(plain["features"][0].get("stac_extensions").is_none());
}
