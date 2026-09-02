//! Live round trip against a real PostGIS instance: seeds a temp table, then
//! exercises `GET`/`POST /search` through the real `PostgisDriverFactory`
//! and the real axum router this crate exports (`#36` slice C). Skipped
//! gracefully unless `TELLURION_TEST_DATABASE_URL` is set — `cargo test`
//! never needs a database by default — same convention
//! `tellurion-postgis`'s own `tests/live.rs` follows, which this file
//! mirrors closely (seed helper, env-var indirection to the driver's own
//! `url_env`).
//!
//! What this proves that `tests/handlers.rs`'s in-memory fake driver can't:
//! `intersects` (composed into a `Filter::Intersects` node, see
//! `handlers::compose_filter`) actually narrows through PostGIS's own real
//! `ST_Intersects`/`ST_GeomFromGeoJSON` SQL (`tellurion-postgis::sql::
//! compile_filter`), not just that the request parses and the AST composes
//! correctly — the fake driver in `tests/handlers.rs` evaluates filters with
//! its own small, deliberately-simplified Rust evaluator, not real SQL.

use std::env;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::Response;
use serde_json::{json, Value};
use tower::ServiceExt;

use tellurion_core::{
    AppConfig, AppContext, FileStyleStore, MokaTileCache, Registry, Resolver, Router as CoreRouter,
    StaticResolver, StyleStore, TileCache,
};
use tellurion_postgis::test_harness;
use tellurion_postgis::PostgisDriverFactory;

const URL_ENV_VAR: &str = "TELLURION_STAC_SEARCH_LIVE_TEST_URL";

async fn seed(database_url: &str, table: &str) {
    let client = test_harness::connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326) NOT NULL,
                 observed_at timestamptz,
                 name text
             );
             INSERT INTO {table} (geom, observed_at, name) VALUES
                 (ST_SetSRID(ST_MakePoint(1, 1), 4326), '2020-01-01T00:00:00Z', 'inside'),
                 (ST_SetSRID(ST_MakePoint(9, 9), 4326), '2020-01-01T00:00:00Z', 'outside');
             ANALYZE {table};"
        ),
    )
    .await
    .expect("seeds the test table");
}

fn build_config_yaml(table: &str) -> String {
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
    datetime: observed_at
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

async fn post(app: &axum::Router, uri: &str, body: Value) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn body_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn item_ids(body: &Value) -> Vec<String> {
    let mut ids: Vec<String> = body["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap().to_string())
        .collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn intersects_narrows_through_real_postgis_over_get_and_post() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping intersects_narrows_through_real_postgis_over_get_and_post: \
             TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_stac_search_live_test_intersects";
    seed(&database_url, table).await;

    // Safety: this test binary's tests each seed a differently-named table
    // and set this same env var; `cargo test` within one binary runs tests
    // concurrently by default, so a genuinely shared env var across tests
    // would race. This file has exactly one `#[tokio::test]`, so there is
    // only ever one writer.
    unsafe {
        env::set_var(URL_ENV_VAR, &database_url);
    }

    let config: AppConfig = serde_yaml::from_str(&build_config_yaml(table)).unwrap();
    config.validate().unwrap();
    let mut registry = Registry::new();
    registry.register(Arc::new(PostgisDriverFactory::new(60)));
    let app = build_app(&config, &registry);

    // A polygon covering (1,1) ("inside") but not (9,9) ("outside").
    let geometry = json!({
        "type": "Polygon",
        "coordinates": [[[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0], [0.0, 0.0]]],
    });

    let get_href = format!(
        "/search?collections=demo&intersects={}",
        urlencoding_minimal(&geometry.to_string())
    );
    let get_response = get(&app, &get_href).await;
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_body = body_json(get_response).await;
    assert_eq!(item_ids(&get_body), vec!["1".to_string()]);

    let post_response = post(
        &app,
        "/search",
        json!({ "collections": ["demo"], "intersects": geometry }),
    )
    .await;
    assert_eq!(post_response.status(), StatusCode::OK);
    let post_body = body_json(post_response).await;
    assert_eq!(item_ids(&post_body), item_ids(&get_body));

    // `intersects` composed (AND) with a real CQL2 `filter` — proves the
    // composition, not just `intersects` alone, reaches PostGIS's own
    // `compile_filter` correctly: narrowing to the empty set when the
    // filter alone can't match anything inside the polygon.
    let combined_href = format!(
        "{get_href}&filter={}",
        urlencoding_minimal("name='outside'")
    );
    let combined = body_json(get(&app, &combined_href).await).await;
    assert!(
        combined["features"].as_array().unwrap().is_empty(),
        "expected no rows: 'outside' is outside the intersects polygon: {combined}"
    );

    let matching_href = format!("{get_href}&filter={}", urlencoding_minimal("name='inside'"));
    let matching = body_json(get(&app, &matching_href).await).await;
    assert_eq!(item_ids(&matching), vec!["1".to_string()]);
}

/// Minimal query-value percent-encoding for this test file's own requests —
/// mirrors `tests/handlers.rs`'s identical helper (this crate's real
/// `params::percent_encode` is private).
fn urlencoding_minimal(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
