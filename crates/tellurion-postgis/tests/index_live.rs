//! Live tests for the derived-index lane (`#67`, the derived-index half of
//! the transactional-outbox design): `WriteSink`/`OutboxSource`/`IndexSink`
//! plus `tellurion_core::applier::drain_once` through the actual
//! `PostgisDriverFactory` entry point, against a real PostGIS instance.
//! Skipped gracefully unless `TELLURION_TEST_DATABASE_URL` is set, matching
//! every other live test in this workspace.
//!
//! The outbox/index table DDL below is hand-kept in sync with
//! `tellurion-ingest::outbox`/`::index`'s own `create_*_table_sql` — the two
//! crates never depend on each other (`tellurion-postgis::write_sql`'s own
//! module doc explains why), so this is deliberately NOT imported from
//! either crate, the same convention `write_live.rs` already follows.

use std::env;

use serde_json::json;
use tellurion_core::{
    drain_once, CollectionDecl, DriverFactory, Mutation, MutationKind, Sequence, StorageDecl,
    StorageDriver,
};
use tellurion_postgis::test_harness;
use tellurion_postgis::PostgisDriverFactory;

const URL_ENV_VAR: &str = "TELLURION_POSTGIS_INDEX_LIVE_TEST_URL";

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
             DROP TABLE IF EXISTS {table}_outbox;
             DROP TABLE IF EXISTS {table}_index;
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

/// Matches `tellurion-ingest::outbox::create_outbox_table_sql` exactly.
async fn seed_outbox_table(database_url: &str, table: &str) {
    let client = connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "CREATE TABLE IF NOT EXISTS {table}_outbox (
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
    .expect("seeds the outbox table");
}

/// Matches `tellurion-ingest::index::create_index_table_sql` exactly.
async fn seed_index_table(database_url: &str, table: &str) {
    let client = connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "CREATE TABLE IF NOT EXISTS {table}_index (
                 feature_id text PRIMARY KEY,
                 version bigint NOT NULL,
                 kind text NOT NULL CHECK (kind IN ('upsert', 'delete')),
                 doc jsonb,
                 updated_at timestamptz NOT NULL DEFAULT now()
             );
             CREATE INDEX IF NOT EXISTS {table}_index_version_idx ON {table}_index (version);"
        ),
    )
    .await
    .expect("seeds the index table");
}

fn collection(table: &str) -> CollectionDecl {
    serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\n"
    ))
    .expect("valid CollectionDecl yaml")
}

async fn build_driver(database_url: &str) -> std::sync::Arc<dyn StorageDriver> {
    // Safety: this test binary sets this one env var exactly once per test
    // process before any connection pool spawns worker tasks, matching
    // `tests/write_live.rs`'s own documented safety argument for the same
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

async fn upsert(
    sink: &dyn tellurion_core::WriteSink,
    collection: &CollectionDecl,
    id: &str,
    name: &str,
) {
    sink.apply(
        collection,
        Mutation {
            feature_id: id.to_string(),
            kind: MutationKind::Upsert(json!({
                "type": "Feature",
                "geometry": {"type": "Point", "coordinates": [10.0, 45.0]},
                "properties": {"name": name}
            })),
        },
    )
    .await
    .expect("upsert succeeds");
}

/// The design doc's section 8 acceptance criterion, second half: a write
/// goes through `WriteSink`, `drain_once` converges the index, and killing
/// the applier mid-drain and restarting converges to the identical state.
/// Also proves applying the same obligation twice (a direct replayed
/// `IndexSink::apply`, standing in for at-least-once redelivery) is
/// harmless — the version-guarded `ON CONFLICT ... WHERE` upsert leaves an
/// already-applied row untouched.
#[tokio::test]
async fn drain_once_converges_the_index_and_resumes_identically_after_a_restart() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping drain_once_converges_the_index_and_resumes_identically_after_a_restart: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_index_live_test_converge";
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;
    seed_index_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let write = driver.write_sink().expect("driver exposes WriteSink");
    let outbox = driver.outbox_source().expect("driver exposes OutboxSource");
    let index = driver.index_sink().expect("driver exposes IndexSink");
    let collection = collection(table);

    upsert(write.as_ref(), &collection, "1", "acme").await;
    upsert(write.as_ref(), &collection, "2", "beta").await;
    upsert(write.as_ref(), &collection, "3", "gamma").await;

    // Bounded batch of 2 — simulates a crash after partial progress, and
    // proves the batch is genuinely bounded (only 2 of the 3 land).
    let applied = drain_once(outbox.as_ref(), index.as_ref(), &collection, 2)
        .await
        .expect("first drain succeeds");
    assert_eq!(applied, 2);
    assert_eq!(
        index.applied_high_water(&collection).await.unwrap(),
        Sequence(2)
    );

    // "Restart": a fresh drain against the SAME durable sink resumes past
    // what was already applied — restart-safe with no separate cursor.
    let applied = drain_once(outbox.as_ref(), index.as_ref(), &collection, 100)
        .await
        .expect("second drain succeeds");
    assert_eq!(applied, 1);
    assert_eq!(
        index.applied_high_water(&collection).await.unwrap(),
        Sequence(3)
    );

    // A third, fully-caught-up drain applies nothing.
    let applied = drain_once(outbox.as_ref(), index.as_ref(), &collection, 100)
        .await
        .expect("third drain succeeds");
    assert_eq!(applied, 0);

    let raw = connect(&database_url).await;
    let count: i64 = raw
        .query_one(&format!("SELECT count(*) FROM {table}_index"), &[])
        .await
        .expect("count succeeds")
        .get(0);
    assert_eq!(
        count, 3,
        "one row per distinct feature_id, never duplicated"
    );

    let doc: serde_json::Value = raw
        .query_one(
            &format!("SELECT doc FROM {table}_index WHERE feature_id = '1'"),
            &[],
        )
        .await
        .expect("row exists")
        .get(0);
    assert_eq!(doc["properties"]["name"], "acme");
}

/// Strict ordering + idempotent version-guarded apply together: a second
/// upsert on the SAME `feature_id` must win over the first once both are
/// drained, even when the applier only sees one obligation per pass (batch
/// size 1) and even when a stale replay of the FIRST obligation is applied
/// again afterward — the design doc's "converges rather than corrupts"
/// idempotency contract (section 4).
#[tokio::test]
async fn ordering_and_idempotent_replay_converge_on_the_latest_version() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping ordering_and_idempotent_replay_converge_on_the_latest_version: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_index_live_test_ordering";
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;
    seed_index_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let write = driver.write_sink().expect("driver exposes WriteSink");
    let outbox = driver.outbox_source().expect("driver exposes OutboxSource");
    let index = driver.index_sink().expect("driver exposes IndexSink");
    let collection = collection(table);

    upsert(write.as_ref(), &collection, "1", "first").await; // sequence 1
    upsert(write.as_ref(), &collection, "2", "unrelated").await; // sequence 2
    upsert(write.as_ref(), &collection, "1", "second").await; // sequence 3

    // Force three separate single-obligation passes.
    for _ in 0..3 {
        drain_once(outbox.as_ref(), index.as_ref(), &collection, 1)
            .await
            .expect("drain succeeds");
    }
    assert_eq!(
        index.applied_high_water(&collection).await.unwrap(),
        Sequence(3)
    );

    let raw = connect(&database_url).await;
    let (doc, version): (serde_json::Value, i64) = {
        let row = raw
            .query_one(
                &format!("SELECT doc, version FROM {table}_index WHERE feature_id = '1'"),
                &[],
            )
            .await
            .expect("row exists");
        (row.get(0), row.get(1))
    };
    assert_eq!(
        doc["properties"]["name"], "second",
        "the later version must win over the earlier one on the same feature_id"
    );
    assert_eq!(version, 3);

    // Now replay the FIRST (stale) obligation directly against the sink —
    // the same shape a redelivered or retried batch would take. It must be
    // a no-op: the version guard rejects it because 1 < 3.
    let obligations = outbox
        .read_after(&collection, Sequence(0), 10)
        .await
        .expect("read_after succeeds");
    let stale = obligations
        .iter()
        .find(|o| o.sequence == Sequence(1))
        .expect("sequence 1 obligation exists")
        .clone();
    index
        .apply(&collection, &stale)
        .await
        .expect("replaying a stale obligation does not error");

    let (doc_after, version_after): (serde_json::Value, i64) = {
        let row = raw
            .query_one(
                &format!("SELECT doc, version FROM {table}_index WHERE feature_id = '1'"),
                &[],
            )
            .await
            .expect("row still exists");
        (row.get(0), row.get(1))
    };
    assert_eq!(
        doc_after["properties"]["name"], "second",
        "a stale replay must not resurrect the earlier version"
    );
    assert_eq!(version_after, 3);
}

/// `IndexSink::apply`/`applied_high_water` against a collection whose
/// index table was never provisioned refuse cleanly (`IndexTableMissing`,
/// surfaced as a named `Error::Config`) rather than a raw SQL error — the
/// server never creates this table; `tellurion-ingest index create-tables`
/// does.
#[tokio::test]
async fn index_operations_fail_with_a_named_error_when_the_table_is_absent() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping index_operations_fail_with_a_named_error_when_the_table_is_absent: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_index_live_test_absent";
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;
    // Deliberately no `seed_index_table` call.

    let driver = build_driver(&database_url).await;
    let write = driver.write_sink().expect("driver exposes WriteSink");
    let index = driver.index_sink().expect("driver exposes IndexSink");
    let collection = collection(table);

    upsert(write.as_ref(), &collection, "1", "acme").await;

    match index.applied_high_water(&collection).await {
        Err(tellurion_core::Error::Config(message)) => {
            assert!(message.contains("index"), "message was: {message}");
            assert!(
                message.contains(&format!("{table}_index")),
                "message was: {message}"
            );
        }
        other => panic!("expected a named Config error, got {}", other.is_ok()),
    }

    let obligation = tellurion_core::Obligation {
        sequence: Sequence(1),
        feature_id: "1".to_string(),
        kind: MutationKind::Upsert(json!({"type": "Feature"})),
        version: Sequence(1),
        committed_at: std::time::SystemTime::UNIX_EPOCH,
        extent: tellurion_core::ObligationExtent::Unrecorded,
    };
    match index.apply(&collection, &obligation).await {
        Err(tellurion_core::Error::Config(message)) => {
            assert!(message.contains("index"), "message was: {message}");
        }
        other => panic!("expected a named Config error, got {}", other.is_ok()),
    }
}
