//! Live tests for `WriteSink::apply_batch` (`#114`) against a real PostGIS
//! instance. Skipped gracefully unless `TELLURION_TEST_DATABASE_URL` is set,
//! matching every other live test in this workspace — see
//! `write_live.rs`'s own module doc, whose fixtures this file reuses the
//! same shape of.

use std::env;

use serde_json::json;
use tellurion_core::{
    BatchItemOutcome, CollectionDecl, DriverFactory, Error as CoreError, Mutation, MutationKind,
    RequestedCrs, StorageDecl, StorageDriver,
};
use tellurion_postgis::test_harness;
use tellurion_postgis::PostgisDriverFactory;

const URL_ENV_VAR: &str = "TELLURION_POSTGIS_BATCH_LIVE_TEST_URL";

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
    .expect("seeds the data and outbox tables");
}

fn collection(table: &str) -> CollectionDecl {
    serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\n"
    ))
    .expect("valid CollectionDecl yaml")
}

async fn build_driver(database_url: &str) -> std::sync::Arc<dyn StorageDriver> {
    // Safety: this test binary sets this one env var exactly once per test
    // process before any connection pool spawns worker tasks — same
    // argument `write_live.rs`'s own `build_driver` documents.
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

fn upsert(id: &str, name: &str) -> Mutation {
    Mutation {
        feature_id: id.to_string(),
        kind: MutationKind::Upsert(json!({
            "type": "Feature",
            "geometry": {"type": "Point", "coordinates": [10.0, 45.0]},
            "properties": {"name": name}
        })),
    }
}

/// A chunk of entirely clean upserts commits every row and every outbox
/// obligation, each reporting `Applied` with a distinct sequence.
#[tokio::test]
async fn a_clean_chunk_applies_every_item_and_reports_its_sequence() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping a_clean_chunk_applies_every_item_and_reports_its_sequence: TELLURION_TEST_DATABASE_URL not set");
        return;
    };
    let table = "tellurion_postgis_batch_live_test_clean";
    seed_data_table(&database_url, table).await;
    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection(table);

    let mutations = vec![upsert("1", "a"), upsert("2", "b"), upsert("3", "c")];
    let results = sink
        .apply_batch(&collection, mutations, RequestedCrs::Omitted, false)
        .await
        .expect("batch apply succeeds");

    assert_eq!(results.len(), 3);
    for (index, result) in results.iter().enumerate() {
        assert_eq!(result.feature_id, (index + 1).to_string());
        assert!(
            matches!(result.outcome, BatchItemOutcome::Applied(_)),
            "item {} should have applied, got {:?}",
            result.feature_id,
            result.outcome
        );
    }

    let raw = connect(&database_url).await;
    let count: i64 = raw
        .query_one(&format!("SELECT count(*) FROM {table}"), &[])
        .await
        .expect("count succeeds")
        .get(0);
    assert_eq!(count, 3);
}

/// A chunk mixing clean rows with a deliberately dirty one (a feature id
/// that doesn't parse as this collection's `Integer` id type) commits every
/// clean row in the SAME transaction the dirty row's own savepoint rolled
/// back from — proving the chunk's atomicity is per-item, not
/// all-or-nothing — and reports the dirty row's refusal with the identical
/// `tellurion_core::Error` variant `WriteSink::apply` gives for the same
/// bad input outside a batch at all.
#[tokio::test]
async fn a_dirty_row_is_refused_by_name_while_its_clean_siblings_still_commit() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping a_dirty_row_is_refused_by_name_while_its_clean_siblings_still_commit: TELLURION_TEST_DATABASE_URL not set");
        return;
    };
    let table = "tellurion_postgis_batch_live_test_dirty";
    seed_data_table(&database_url, table).await;
    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection(table);

    // The single-item lane's own refusal for the exact same bad id, as a
    // reference to compare the batch outcome's error against.
    let single_item_error = sink
        .apply(&collection, upsert("not-an-integer", "dirty"))
        .await
        .expect_err("a non-integer id must be refused by the single-item lane too");

    let mutations = vec![
        upsert("1", "a"),
        upsert("not-an-integer", "dirty"),
        upsert("2", "b"),
    ];
    let results = sink
        .apply_batch(&collection, mutations, RequestedCrs::Omitted, false)
        .await
        .expect("batch apply itself succeeds even though one item is refused");

    assert_eq!(results.len(), 3);
    assert!(matches!(results[0].outcome, BatchItemOutcome::Applied(_)));
    assert!(matches!(results[2].outcome, BatchItemOutcome::Applied(_)));
    match &results[1].outcome {
        BatchItemOutcome::Refused(err) => {
            assert_eq!(
                std::mem::discriminant(err),
                std::mem::discriminant(&single_item_error),
                "the batch lane's refusal must name the same problem the \
                 single-item lane gives for identical bad input"
            );
            assert!(matches!(err, CoreError::Invalid(_)));
        }
        other => panic!("expected the dirty row to be refused, got {other:?}"),
    }

    let raw = connect(&database_url).await;
    let count: i64 = raw
        .query_one(&format!("SELECT count(*) FROM {table}"), &[])
        .await
        .expect("count succeeds")
        .get(0);
    assert_eq!(
        count, 2,
        "both clean rows must have committed despite the dirty row's own savepoint rolling back"
    );
}

/// `strict: true` stops attempting further mutations the instant one is
/// refused — the caller sees a shorter result `Vec` than the input, and
/// nothing after the refusal was ever attempted (so it never touched the
/// database at all).
#[tokio::test]
async fn strict_mode_stops_at_the_first_refusal_and_still_commits_the_earlier_rows() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping strict_mode_stops_at_the_first_refusal_and_still_commits_the_earlier_rows: TELLURION_TEST_DATABASE_URL not set");
        return;
    };
    let table = "tellurion_postgis_batch_live_test_strict";
    seed_data_table(&database_url, table).await;
    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection(table);

    let mutations = vec![
        upsert("1", "a"),
        upsert("not-an-integer", "dirty"),
        upsert("2", "b"),
        upsert("3", "c"),
    ];
    let results = sink
        .apply_batch(&collection, mutations, RequestedCrs::Omitted, true)
        .await
        .expect("batch apply itself succeeds");

    assert_eq!(
        results.len(),
        2,
        "strict mode must stop after the refused item, never attempting the remainder"
    );
    assert!(matches!(results[0].outcome, BatchItemOutcome::Applied(_)));
    assert!(matches!(results[1].outcome, BatchItemOutcome::Refused(_)));

    let raw = connect(&database_url).await;
    let count: i64 = raw
        .query_one(&format!("SELECT count(*) FROM {table}"), &[])
        .await
        .expect("count succeeds")
        .get(0);
    assert_eq!(count, 1, "only the row before the refusal was ever applied");
}

/// A driver-level refusal — every property key across the WHOLE chunk gets
/// resolved once (`write_apply_batch_inner`'s own doc) — still refuses an
/// unwritable property by the same name the single-item lane uses.
#[tokio::test]
async fn an_unwritable_property_is_refused_by_the_same_name_as_the_single_item_lane() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping an_unwritable_property_is_refused_by_the_same_name_as_the_single_item_lane: TELLURION_TEST_DATABASE_URL not set");
        return;
    };
    let table = "tellurion_postgis_batch_live_test_unwritable";
    seed_data_table(&database_url, table).await;
    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection(table);

    let bad_feature = Mutation {
        feature_id: "1".to_string(),
        kind: MutationKind::Upsert(json!({
            "type": "Feature",
            "geometry": {"type": "Point", "coordinates": [10.0, 45.0]},
            "properties": {"does_not_exist": "x"}
        })),
    };

    let single_item_error = sink
        .apply(&collection, bad_feature.clone())
        .await
        .expect_err("an unwritable property must be refused by the single-item lane too");

    let results = sink
        .apply_batch(&collection, vec![bad_feature], RequestedCrs::Omitted, false)
        .await
        .expect("batch apply itself succeeds");

    match &results[0].outcome {
        BatchItemOutcome::Refused(err) => {
            assert_eq!(
                std::mem::discriminant(err),
                std::mem::discriminant(&single_item_error)
            );
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// A missing outbox is infrastructure, not a deterministic item refusal:
/// the whole chunk errors and the outer transaction rolls every data row
/// back.
#[tokio::test]
async fn missing_outbox_aborts_and_rolls_back_the_whole_chunk() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping missing_outbox_aborts_and_rolls_back_the_whole_chunk: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_batch_live_test_missing_outbox";
    seed_data_table(&database_url, table).await;
    let raw = connect(&database_url).await;
    raw.batch_execute(&format!("DROP TABLE {table}_outbox"))
        .await
        .expect("drops the outbox table");

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection(table);
    let result = sink
        .apply_batch(
            &collection,
            vec![upsert("1", "a"), upsert("2", "b")],
            RequestedCrs::Omitted,
            false,
        )
        .await;
    assert!(matches!(result, Err(CoreError::Config(_))));

    let count: i64 = raw
        .query_one(&format!("SELECT count(*) FROM {table}"), &[])
        .await
        .expect("count succeeds")
        .get(0);
    assert_eq!(count, 0, "the failed chunk must leave no data rows");
}

#[tokio::test]
async fn outbox_constraint_failure_aborts_and_rolls_back_the_whole_chunk() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping outbox_constraint_failure_aborts_and_rolls_back_the_whole_chunk: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_batch_live_test_outbox_constraint";
    seed_data_table(&database_url, table).await;
    let raw = connect(&database_url).await;
    raw.batch_execute(&format!(
        "ALTER TABLE {table}_outbox ADD CONSTRAINT reject_feature CHECK (feature_id <> '2')"
    ))
    .await
    .expect("adds the outbox-only constraint");

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection(table);
    let result = sink
        .apply_batch(
            &collection,
            vec![upsert("1", "a"), upsert("2", "b")],
            RequestedCrs::Omitted,
            false,
        )
        .await;
    assert!(
        result.is_err(),
        "outbox constraints are chunk infrastructure"
    );

    let count: i64 = raw
        .query_one(&format!("SELECT count(*) FROM {table}"), &[])
        .await
        .expect("count succeeds")
        .get(0);
    assert_eq!(count, 0, "the failed chunk must leave no data rows");
    let outbox_count: i64 = raw
        .query_one(&format!("SELECT count(*) FROM {table}_outbox"), &[])
        .await
        .expect("outbox count succeeds")
        .get(0);
    assert_eq!(
        outbox_count, 0,
        "the failed chunk must leave no outbox rows"
    );
}
