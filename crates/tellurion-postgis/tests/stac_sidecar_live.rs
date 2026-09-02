//! Live tests for the per-item STAC metadata sidecar (`#202`):
//! `StacMetadataSource::stac_metadata` through the actual
//! `PostgisDriverFactory` entry point, against a real PostGIS instance.
//! Skipped gracefully unless `TELLURION_TEST_DATABASE_URL` is set, matching
//! every other live test in this workspace.
//!
//! The sidecar table DDL below is hand-kept in sync with
//! `tellurion-ingest::stac`'s own `create_stac_table_sql` — the two crates
//! never depend on each other (`tellurion-postgis::stac_sql`'s own module
//! doc explains why), so this is deliberately NOT imported from either
//! crate, the same convention `index_live.rs` already follows.

use std::env;

use serde_json::json;
use tellurion_core::{CollectionDecl, DriverFactory, StorageDecl, StorageDriver};
use tellurion_postgis::test_harness;
use tellurion_postgis::PostgisDriverFactory;

const URL_ENV_VAR: &str = "TELLURION_POSTGIS_STAC_SIDECAR_LIVE_TEST_URL";

async fn connect(database_url: &str) -> tokio_postgres::Client {
    test_harness::connect(database_url).await
}

async fn seed_data_table(database_url: &str, table: &str) {
    let client = connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table} CASCADE;
             DROP TABLE IF EXISTS {table}_stac;
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326),
                 name text
             );"
        ),
    )
    .await
    .expect("seeds the data table");
}

/// Matches `tellurion-ingest::stac::create_stac_table_sql` exactly.
async fn seed_stac_table(database_url: &str, table: &str) {
    let client = connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "CREATE TABLE IF NOT EXISTS {table}_stac (
                 feature_id text PRIMARY KEY,
                 version bigint NOT NULL DEFAULT 0,
                 doc jsonb NOT NULL,
                 updated_at timestamptz NOT NULL DEFAULT now()
             );
             CREATE INDEX IF NOT EXISTS {table}_stac_version_idx ON {table}_stac (version);"
        ),
    )
    .await
    .expect("seeds the sidecar table");
}

fn collection(table: &str) -> CollectionDecl {
    serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\nstac_metadata: true\n"
    ))
    .expect("valid CollectionDecl yaml")
}

async fn build_driver(database_url: &str) -> std::sync::Arc<dyn StorageDriver> {
    // Safety: this test binary sets this one env var exactly once per test
    // process before any connection pool spawns worker tasks, matching
    // `tests/index_live.rs`'s own documented safety argument for the same
    // pattern.
    unsafe {
        env::set_var(URL_ENV_VAR, database_url);
    }
    let factory = PostgisDriverFactory::new(60);
    let decl = StorageDecl {
        id: "main".to_string(),
        driver: "postgis".to_string(),
        url_env: URL_ENV_VAR.to_string(),
        pool_size: None,
    };
    factory.build(&decl).expect("driver builds")
}

/// The whole read contract in one pass: a batched `feature_id = ANY(...)`
/// lookup returns exactly the rows that exist, keyed by feature id, and is
/// silently sparse for the ids that have none.
#[tokio::test]
async fn a_batched_lookup_returns_only_the_ids_that_have_a_sidecar_row() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping a_batched_lookup_returns_only_the_ids_that_have_a_sidecar_row: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_stac_live_test_lookup";
    seed_data_table(&database_url, table).await;
    seed_stac_table(&database_url, table).await;

    let client = connect(&database_url).await;
    client
        .execute(
            &format!("INSERT INTO {table}_stac (feature_id, version, doc) VALUES ($1, $2, $3)"),
            &[
                &"1",
                &1i64,
                &json!({"properties": {"eo:cloud_cover": 12}, "stac_extensions": ["x"]}),
            ],
        )
        .await
        .expect("inserts a sidecar row");
    client
        .execute(
            &format!("INSERT INTO {table}_stac (feature_id, version, doc) VALUES ($1, $2, $3)"),
            &[&"3", &1i64, &json!({"properties": {"eo:cloud_cover": 90}})],
        )
        .await
        .expect("inserts a second sidecar row");

    let driver = build_driver(&database_url).await;
    let sidecar = driver
        .stac_metadata_source()
        .expect("PostGIS advertises the STAC metadata capability");
    let collection = collection(table);

    let docs = sidecar
        .stac_metadata(
            &collection,
            &["1".to_string(), "2".to_string(), "3".to_string()],
        )
        .await
        .expect("the batched lookup succeeds");

    assert_eq!(docs.len(), 2, "only ids with a row come back: {docs:?}");
    assert_eq!(docs["1"]["properties"]["eo:cloud_cover"], 12);
    assert_eq!(docs["1"]["stac_extensions"], json!(["x"]));
    assert_eq!(docs["3"]["properties"]["eo:cloud_cover"], 90);
    assert!(
        !docs.contains_key("2"),
        "an id with no sidecar row must be absent, not present-and-empty"
    );
}

/// An empty page never reaches the backend at all — the trait's own "no
/// round trip for nothing" half of the one-per-page cost model. Proven by
/// asking a collection whose sidecar table does NOT exist: a query would
/// have refused with `StacTableMissing`, so `Ok(<empty>)` is only possible
/// if no query ran.
#[tokio::test]
async fn an_empty_page_costs_no_round_trip_at_all() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping an_empty_page_costs_no_round_trip_at_all: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_stac_live_test_empty";
    seed_data_table(&database_url, table).await;
    // Deliberately no `seed_stac_table` call.

    let driver = build_driver(&database_url).await;
    let sidecar = driver
        .stac_metadata_source()
        .expect("PostGIS advertises the STAC metadata capability");

    let docs = sidecar
        .stac_metadata(&collection(table), &[])
        .await
        .expect("an empty page short-circuits before any query");
    assert!(docs.is_empty());
}

/// A collection that declares `stac_metadata: true` but whose sidecar table
/// was never provisioned refuses cleanly and by name (`StacTableMissing`,
/// surfaced as a named `Error::Config`) rather than answering an empty
/// sidecar — which would be indistinguishable from a provisioned sidecar
/// holding no rows for this page. The server never creates this table;
/// `tellurion-ingest stac create-tables` does.
#[tokio::test]
async fn a_missing_sidecar_table_is_a_named_refusal_not_an_empty_answer() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping a_missing_sidecar_table_is_a_named_refusal_not_an_empty_answer: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_stac_live_test_absent";
    seed_data_table(&database_url, table).await;
    // Deliberately no `seed_stac_table` call.

    let driver = build_driver(&database_url).await;
    let sidecar = driver
        .stac_metadata_source()
        .expect("PostGIS advertises the STAC metadata capability");

    match sidecar
        .stac_metadata(&collection(table), &["1".to_string()])
        .await
    {
        Err(tellurion_core::Error::Config(message)) => {
            assert!(
                message.contains(&format!("{table}_stac")),
                "message was: {message}"
            );
            assert!(
                message.contains("tellurion-ingest stac create-tables"),
                "the refusal must point at the command that provisions it: {message}"
            );
        }
        other => panic!("expected a named Config error, got ok={}", other.is_ok()),
    }
}

/// A `doc` that is not a JSON object is a storage anomaly, named rather
/// than merged or silently dropped.
#[tokio::test]
async fn a_non_object_doc_is_a_named_storage_error() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping a_non_object_doc_is_a_named_storage_error: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_stac_live_test_malformed";
    seed_data_table(&database_url, table).await;
    seed_stac_table(&database_url, table).await;

    let client = connect(&database_url).await;
    client
        .execute(
            &format!("INSERT INTO {table}_stac (feature_id, version, doc) VALUES ($1, $2, $3)"),
            &[&"1", &1i64, &json!("not-an-object")],
        )
        .await
        .expect("inserts a malformed sidecar row");

    let driver = build_driver(&database_url).await;
    let sidecar = driver
        .stac_metadata_source()
        .expect("PostGIS advertises the STAC metadata capability");

    let result = sidecar
        .stac_metadata(&collection(table), &["1".to_string()])
        .await;
    assert!(
        result.is_err(),
        "a scalar doc has no member set to merge and must not pass silently"
    );
}
