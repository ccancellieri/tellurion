//! Live tests for freshness-gated search routing (`#67`, the deferred half
//! of the transactional-outbox design's search lane, design doc section 4):
//! `Router::resolve_search` against a real PostGIS instance, through the
//! actual `PostgisDriverFactory` entry point — the same pattern
//! `tests/index_live.rs` uses for the derived-index lane it builds on.
//! Skipped gracefully unless `TELLURION_TEST_DATABASE_URL` is set.
//!
//! Every test routes `search: [index, main]` across two distinct storage
//! ids ("index", "main") that both happen to point at the same live
//! database — a genuinely separate index technology is out of scope for
//! this slice (the design doc's own section 8 sanctions "a second PostGIS
//! storage" as the simplest honest choice). Using two storage ids rather
//! than one matters here: it is what proves `resolve_search` only ever
//! freshness-gates the search lane's primary entry ("index") and never
//! mistakes the fallback tail entry ("main") for a second index attempt,
//! even though both are PostGIS storages that always advertise
//! `SearchSource` (`Router::resolve_search`'s own doc explains why that
//! asymmetry is deliberate).

use std::env;

use serde_json::json;
use tellurion_core::{
    drain_once, AppConfig, ItemsQuery, Registry, Router, SearchQuery, SearchResolution,
};
use tellurion_postgis::test_harness;
use tellurion_postgis::PostgisDriverFactory;

const MAIN_URL_ENV_VAR: &str = "TELLURION_POSTGIS_SEARCH_LIVE_TEST_MAIN_URL";
const INDEX_URL_ENV_VAR: &str = "TELLURION_POSTGIS_SEARCH_LIVE_TEST_INDEX_URL";

async fn connect(database_url: &str) -> tokio_postgres::Client {
    test_harness::connect(database_url).await
}

/// The data/outbox/index table trio a write+index+search-routed collection
/// needs — same DDL `tests/index_live.rs` seeds, hand-kept in sync with
/// `tellurion-ingest`'s own table-creating commands (that module's own doc).
///
/// `#272`: this file derives `{table}_index_version_idx` and
/// `{table}_index_search_idx` — 18 and 17 bytes — so a `table` here has 45
/// bytes to spend, not 63. Every name below dropped its `_test` segment for
/// that reason: three were over, and PostgreSQL was silently TRUNCATING the
/// derived index names rather than refusing them, which is how two of these
/// fixtures could have come to share one index without anything failing.
/// `test_harness::apply_fixture_ddl` refuses that by name now.
async fn seed_tables(database_url: &str, table: &str) {
    let client = connect(database_url).await;
    test_harness::apply_fixture_ddl(&client, table, &format!(
            "DROP TABLE IF EXISTS {table} CASCADE;
             DROP TABLE IF EXISTS {table}_outbox;
             DROP TABLE IF EXISTS {table}_index;
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326),
                 name text
             );
             CREATE TABLE {table}_outbox (
                 sequence bigserial PRIMARY KEY,
                 feature_id text NOT NULL,
                 kind text NOT NULL CHECK (kind IN ('upsert', 'delete')),
                 payload jsonb,
                 committed_at timestamptz NOT NULL DEFAULT now(),
                 extent_crs84 jsonb
             );
             CREATE TABLE {table}_index (
                 feature_id text PRIMARY KEY,
                 version bigint NOT NULL,
                 kind text NOT NULL CHECK (kind IN ('upsert', 'delete')),
                 doc jsonb,
                 updated_at timestamptz NOT NULL DEFAULT now()
             );
             CREATE INDEX IF NOT EXISTS {table}_index_version_idx ON {table}_index (version);
             ALTER TABLE {table}_index ADD COLUMN IF NOT EXISTS search_text tsvector GENERATED ALWAYS AS (jsonb_to_tsvector('simple', coalesce(doc -> 'properties', '{{}}'::jsonb), '[\"string\"]')) STORED;
             CREATE INDEX IF NOT EXISTS {table}_index_search_idx ON {table}_index USING GIN (search_text);"
        ))
        .await
        .expect("seeds the data/outbox/index table trio");
}

/// Same trio, minus the index table — for the "index never provisioned"
/// scenario (`tests/index_live.rs`'s own `IndexTableMissing` test uses the
/// identical omission).
async fn seed_tables_without_index(database_url: &str, table: &str) {
    let client = connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table} CASCADE;
             DROP TABLE IF EXISTS {table}_outbox;
             DROP TABLE IF EXISTS {table}_index;
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326),
                 name text
             );
             CREATE TABLE {table}_outbox (
                 sequence bigserial PRIMARY KEY,
                 feature_id text NOT NULL,
                 kind text NOT NULL CHECK (kind IN ('upsert', 'delete')),
                 payload jsonb,
                 committed_at timestamptz NOT NULL DEFAULT now(),
                 extent_crs84 jsonb
             );"
        ),
    )
    .await
    .expect("seeds the data/outbox tables, deliberately without the index table");
}

/// Two storage ids ("main", "index"), both the `postgis` driver, both
/// pointing at `database_url` — see this file's own module doc for why two
/// ids matter even though they share one physical database. `with_index`
/// controls whether `routing.index: index` is declared at all (omitted for
/// the validation-refusal test, which routes `search` at "index" without
/// ever provisioning it). `search_routing` is the YAML flow-list literal
/// for `routing.search` (e.g. `"[index, main]"`).
fn config_yaml(table: &str, with_index: bool, search_routing: &str, search_bound: u64) -> String {
    let mut lines = vec![
        "storages:".to_string(),
        format!("  - {{ id: main, driver: postgis, url_env: {MAIN_URL_ENV_VAR} }}"),
        format!("  - {{ id: index, driver: postgis, url_env: {INDEX_URL_ENV_VAR} }}"),
        "tenants: [ { id: public } ]".to_string(),
        "catalogs: [ { id: default, tenant: public } ]".to_string(),
        "collections:".to_string(),
        format!("  - id: {table}"),
        "    catalog: default".to_string(),
        "    storage: main".to_string(),
        format!("    table: {table}"),
        "    geometry: geom".to_string(),
        "    pk: id".to_string(),
        "    routing:".to_string(),
        "      write: main".to_string(),
    ];
    if with_index {
        lines.push("      index: index".to_string());
    }
    lines.push(format!("      search: {search_routing}"));
    lines.push("    search:".to_string());
    lines.push(format!("      freshness_bound: {search_bound}"));
    lines.join("\n") + "\n"
}

async fn build_router(database_url: &str, config_yaml: &str) -> Router {
    // Safety: this test binary sets these two env vars exactly once per test
    // process before any connection pool spawns worker tasks, always to the
    // same value across every test in this file — the identical safety
    // argument `tests/index_live.rs::build_driver` documents for its own
    // single env var.
    unsafe {
        env::set_var(MAIN_URL_ENV_VAR, database_url);
        env::set_var(INDEX_URL_ENV_VAR, database_url);
    }
    let config: AppConfig = serde_yaml::from_str(config_yaml).expect("valid AppConfig yaml");
    config.validate().expect("referential integrity holds");

    let mut registry = Registry::new();
    registry.register(std::sync::Arc::new(PostgisDriverFactory::new(60)));
    Router::build(&config, &registry).expect("router builds")
}

async fn upsert(router: &Router, table: &str, id: &str, name: &str) {
    let (decl, write) = router
        .resolve_write("public", "default", table)
        .await
        .expect("resolves the write lane");
    write
        .apply(
            &decl,
            tellurion_core::Mutation {
                feature_id: id.to_string(),
                kind: tellurion_core::MutationKind::Upsert(json!({
                    "type": "Feature",
                    "geometry": {"type": "Point", "coordinates": [10.0, 45.0]},
                    "properties": {"name": name}
                })),
            },
        )
        .await
        .expect("upsert succeeds");
}

async fn drain(router: &Router, table: &str, batch_size: u32) -> usize {
    let (decl, outbox) = router
        .resolve_outbox("public", "default", table)
        .await
        .expect("resolves the outbox source");
    let (_, index) = router
        .resolve_index("public", "default", table)
        .await
        .expect("resolves the index sink");
    drain_once(outbox.as_ref(), index.as_ref(), &decl, batch_size)
        .await
        .expect("drain succeeds")
}

/// Deliverable 2's "serve-within-bound" scenario: a fully-drained index
/// (lag `0`) with `freshness_bound: 0` is served directly from the derived
/// index's own `SearchSource`, not the primary.
#[tokio::test]
async fn resolve_search_serves_from_the_index_when_the_lag_is_within_bound() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping resolve_search_serves_from_the_index_when_the_lag_is_within_bound: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_search_live_within_bound";
    seed_tables(&database_url, table).await;

    let router = build_router(&database_url, &config_yaml(table, true, "[index, main]", 0)).await;
    router
        .validate_catalog()
        .await
        .expect("boot validation accepts a provisioned, fully-routed collection");

    upsert(&router, table, "1", "acme").await;
    upsert(&router, table, "2", "beta").await;
    let applied = drain(&router, table, 100).await;
    assert_eq!(applied, 2, "both obligations land in one bounded pass");

    let (decl, resolution) = router
        .resolve_search("public", "default", table)
        .await
        .expect("resolves the search lane");
    let SearchResolution::Index(search) = resolution else {
        panic!("expected the fully-caught-up index to serve, got the fallback tail instead");
    };
    let page = search
        .search(&decl, &SearchQuery { limit: 10, q: None })
        .await
        .expect("search succeeds");
    assert_eq!(page.features_geojson.len(), 2);
    let names: std::collections::HashSet<_> = page
        .features_geojson
        .iter()
        .map(|doc| doc["properties"]["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        names,
        std::collections::HashSet::from(["acme".to_string(), "beta".to_string()])
    );

    // `#181`: the same fresh index narrows by free text through the
    // GIN-backed `search_text` column — `q` selects exactly the document
    // whose text-typed properties match, and the driver advertises the
    // capability the dispatch path checks before ever sending a `q`.
    assert!(search.text_search_capable());
    let page = search
        .search(
            &decl,
            &SearchQuery {
                limit: 10,
                q: Some("acme".to_string()),
            },
        )
        .await
        .expect("free-text search succeeds");
    assert_eq!(page.features_geojson.len(), 1);
    assert_eq!(page.features_geojson[0]["properties"]["name"], "acme");
}

/// Deliverable 2's "fallback-on-lag-exceeded" scenario: the primary has
/// three committed writes but only one has been drained into the index, so
/// `lag = 2 > freshness_bound (0)` — the search lane falls back to the
/// tail's degraded `FeatureSource`, which serves the primary's real,
/// up-to-date row count (proving this is a genuine fallback, not a stale
/// index answer wearing a different hat).
#[tokio::test]
async fn resolve_search_falls_back_to_the_primary_when_the_lag_exceeds_the_bound() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping resolve_search_falls_back_to_the_primary_when_the_lag_exceeds_the_bound: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_search_live_lag_exceeded";
    seed_tables(&database_url, table).await;

    let router = build_router(&database_url, &config_yaml(table, true, "[index, main]", 0)).await;
    router
        .validate_catalog()
        .await
        .expect("boot validation passes");

    upsert(&router, table, "1", "acme").await;
    upsert(&router, table, "2", "beta").await;
    upsert(&router, table, "3", "gamma").await;
    // Only the first obligation is drained — the index is now two sequences
    // behind the primary's outbox high-water.
    let applied = drain(&router, table, 1).await;
    assert_eq!(applied, 1);

    let (decl, resolution) = router
        .resolve_search("public", "default", table)
        .await
        .expect("resolves the search lane");
    let SearchResolution::Fallback(features) = resolution else {
        panic!("expected the stale index to be refused in favor of the fallback tail");
    };
    let page = features
        .items(&decl, &ItemsQuery::default())
        .await
        .expect("items query against the primary succeeds");
    assert_eq!(
        page.features_geojson.len(),
        3,
        "the primary serves every committed write, unlike the lagging index"
    );
}

/// Deliverable 2's "fallback-or-refusal-when-lag-unknown" scenario: the
/// index table for this collection was never provisioned at all (no
/// `tellurion-ingest index create-tables` run), so `SearchSource::
/// applied_high_water` errors and the lag cannot be measured — treated the
/// same as "exceeds the bound," so the search lane falls back to the tail.
#[tokio::test]
async fn resolve_search_falls_back_to_the_primary_when_the_lag_cannot_be_determined() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping resolve_search_falls_back_to_the_primary_when_the_lag_cannot_be_determined: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_search_live_lag_unknown";
    seed_tables_without_index(&database_url, table).await;

    // `routing.index: index` is declared (satisfying the static provisioning
    // check — the collection DID declare it derives an index at "index"),
    // but the physical `<table>_index` table underneath it was never
    // created, so this is a request-time, not a config-load, refusal.
    let router = build_router(&database_url, &config_yaml(table, true, "[index, main]", 0)).await;

    upsert(&router, table, "1", "acme").await;

    let (decl, resolution) = router
        .resolve_search("public", "default", table)
        .await
        .expect("resolves the search lane despite the unprovisioned index table");
    let SearchResolution::Fallback(features) = resolution else {
        panic!("expected an unmeasurable lag to fall back to the tail, not serve the index");
    };
    let page = features
        .items(&decl, &ItemsQuery::default())
        .await
        .expect("items query against the primary succeeds");
    assert_eq!(page.features_geojson.len(), 1);
}

/// Deliverable 3's validation refusal: `routing.search` names storage
/// "index" as its primary entry, but this collection never provisions it
/// via `routing.index` — a clean, named `Error::Config` refusal at
/// `validate_catalog`'s config-load sweep, never a silent no-op or a raw
/// request-time surprise.
#[tokio::test]
async fn router_refuses_a_search_lane_routed_to_an_index_the_collection_never_provisions() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping router_refuses_a_search_lane_routed_to_an_index_the_collection_never_provisions: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_search_live_unprovisioned";
    seed_tables(&database_url, table).await;

    // No `routing.index` at all this time — `search: [index, main]` still
    // names the "index" storage as its primary entry.
    let router = build_router(
        &database_url,
        &config_yaml(table, false, "[index, main]", 0),
    )
    .await;

    match router.validate_catalog().await {
        Err(tellurion_core::Error::Config(message)) => {
            assert!(message.contains("routing.index"), "message was: {message}");
            assert!(message.contains("'search'"), "message was: {message}");
            assert!(message.contains("'index'"), "message was: {message}");
        }
        other => panic!("expected a named Config refusal, got {}", other.is_ok()),
    }

    // The same refusal surfaces at first touch too (`#59`'s lazy-mode
    // symmetry), not only from the eager boot sweep above.
    match router.resolve_search("public", "default", table).await {
        Err(tellurion_core::Error::Config(_)) => {}
        other => panic!(
            "expected the same named refusal at first touch, got {}",
            other.is_ok()
        ),
    }
}
