//! Live tests for the database-backed `AssetRecordStore` capability
//! (assets-and-object-storage proposal, first slice) against a real
//! PostGIS instance. Skipped gracefully unless `TELLURION_TEST_DATABASE_URL`
//! is set, matching every other live test in this workspace.
//!
//! The DDL below is hand-kept in sync with
//! `tellurion-ingest::assets`'s own `create_assets_table_sql` — the two
//! crates never depend on each other (`tellurion-postgis::asset_sql`'s own
//! module doc explains why), so this is deliberately NOT imported from that
//! crate — the same arrangement `write_live.rs` already documents for the
//! outbox table.

use std::env;

use tellurion_core::{
    AssetKind, AssetState, CollectionDecl, Digest, DriverFactory, FinalizeOutcome, NewAssetKind,
    NewAssetRecord, StorageDecl, StorageDriver,
};
use tellurion_postgis::test_harness;
use tellurion_postgis::PostgisDriverFactory;
use uuid::Uuid;

const URL_ENV_VAR: &str = "TELLURION_POSTGIS_ASSETS_LIVE_TEST_URL";

async fn connect(database_url: &str) -> tokio_postgres::Client {
    test_harness::connect(database_url).await
}

/// A writable data table (the asset store needs a real physical table to
/// derive `"<table>_assets"` from — see `CollectionDecl::resolved_table`),
/// no assets table. Callers that need one call [`seed_assets_table`]
/// separately, so a test can exercise "the assets table is absent" without
/// a second fixture.
async fn seed_data_table(database_url: &str, table: &str) {
    let client = connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table} CASCADE;
             DROP TABLE IF EXISTS {table}_assets;
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326)
             );"
        ),
    )
    .await
    .expect("seeds the data table");
}

/// Matches `tellurion-ingest::assets::create_assets_table_sql` exactly —
/// see this file's own module doc.
async fn seed_assets_table(database_url: &str, table: &str) {
    let client = connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "CREATE TABLE IF NOT EXISTS {table}_assets (
                 id uuid PRIMARY KEY,
                 item_id text NOT NULL DEFAULT '',
                 asset_key text NOT NULL,
                 kind text NOT NULL CHECK (kind IN ('managed', 'remote')),
                 state text NOT NULL CHECK (state IN ('pending', 'available', 'failed')),
                 href text,
                 media_type text,
                 title text,
                 description text,
                 roles jsonb NOT NULL DEFAULT '[]',
                 declared_size bigint,
                 digest_algorithm text,
                 digest_value text,
                 failure_reason text,
                 created_at timestamptz NOT NULL DEFAULT now(),
                 updated_at timestamptz NOT NULL DEFAULT now(),
                 UNIQUE (item_id, asset_key)
             );"
        ),
    )
    .await
    .expect("seeds the assets table");
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
    // `write_live.rs`'s own documented safety argument for the same
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

fn managed_new_record(id: Uuid, digest: Digest) -> NewAssetRecord {
    NewAssetRecord {
        id,
        kind: NewAssetKind::Managed {
            media_type: Some("image/png".to_string()),
            title: Some("thumbnail".to_string()),
            description: None,
            roles: vec!["thumbnail".to_string()],
            declared_size: 5,
            digest,
        },
    }
}

/// Register -> finalize(available) round trip, at both collection level
/// (`item_id: None`) and item level (`item_id: Some(..)`), durably landing
/// in the real table with the digest readable back byte-for-byte.
#[tokio::test]
async fn register_and_finalize_round_trip_at_both_collection_and_item_level() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping register_and_finalize_round_trip_at_both_collection_and_item_level: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_assets_live_test_roundtrip";
    seed_data_table(&database_url, table).await;
    seed_assets_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let store = driver
        .asset_record_store()
        .expect("driver exposes AssetRecordStore");
    let collection = collection(table);
    let digest = tellurion_core::compute_sha256(b"hello");

    let collection_level = store
        .register(
            &collection,
            None,
            "thumb",
            managed_new_record(Uuid::new_v4(), digest.clone()),
        )
        .await
        .expect("collection-level register succeeds");
    assert_eq!(collection_level.state, AssetState::Pending);
    assert_eq!(collection_level.kind, AssetKind::Managed);
    assert_eq!(collection_level.digest.as_ref(), Some(&digest));

    let item_level = store
        .register(
            &collection,
            Some("feature-1"),
            "thumb",
            managed_new_record(Uuid::new_v4(), digest.clone()),
        )
        .await
        .expect("item-level register with the identical key succeeds (different scope)");
    assert_ne!(collection_level.id, item_level.id);

    let finalized = store
        .finalize(&collection, None, "thumb", FinalizeOutcome::Available)
        .await
        .expect("finalize succeeds");
    assert_eq!(finalized.state, AssetState::Available);
    assert_eq!(finalized.id, collection_level.id);

    // The item-level record is untouched by the collection-level finalize.
    let item_record = store
        .get(&collection, Some("feature-1"), "thumb")
        .await
        .expect("get succeeds")
        .expect("item-level record exists");
    assert_eq!(item_record.state, AssetState::Pending);
}

/// A second registration at an already-claimed `(item_id, key)` refuses
/// with the named `Conflict` — the real `UNIQUE (item_id, asset_key)`
/// constraint, rewritten from a raw Postgres error.
#[tokio::test]
async fn a_conflicting_key_refuses_with_a_named_conflict() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping a_conflicting_key_refuses_with_a_named_conflict: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_assets_live_test_conflict";
    seed_data_table(&database_url, table).await;
    seed_assets_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let store = driver
        .asset_record_store()
        .expect("driver exposes AssetRecordStore");
    let collection = collection(table);
    let digest = tellurion_core::compute_sha256(b"a");

    store
        .register(
            &collection,
            None,
            "thumb",
            managed_new_record(Uuid::new_v4(), digest.clone()),
        )
        .await
        .expect("first register succeeds");

    let err = store
        .register(
            &collection,
            None,
            "thumb",
            managed_new_record(Uuid::new_v4(), digest),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, tellurion_core::Error::Conflict(_)),
        "expected a named Conflict, got {err:?}"
    );
}

/// Every asset operation against a collection whose `"<table>_assets"`
/// table was never provisioned refuses with the same named `Config` error
/// naming the table — never a raw, unnamed SQL error.
#[tokio::test]
async fn asset_operations_fail_with_a_named_error_when_the_table_is_absent() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping asset_operations_fail_with_a_named_error_when_the_table_is_absent: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_assets_live_test_missing_table";
    // Deliberately no `seed_assets_table` call.
    seed_data_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let store = driver
        .asset_record_store()
        .expect("driver exposes AssetRecordStore");
    let collection = collection(table);
    let digest = tellurion_core::compute_sha256(b"a");

    let err = store
        .register(
            &collection,
            None,
            "thumb",
            managed_new_record(Uuid::new_v4(), digest),
        )
        .await
        .unwrap_err();
    match err {
        tellurion_core::Error::Config(message) => {
            assert!(message.contains("asset"), "message was: {message}");
            assert!(
                message.contains(&format!("{table}_assets")),
                "message was: {message}"
            );
        }
        other => panic!("expected a named Config error, got {other:?}"),
    }
}

/// `delete` removes the row durably; a subsequent `get` sees nothing.
#[tokio::test]
async fn delete_removes_the_row_durably() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping delete_removes_the_row_durably: TELLURION_TEST_DATABASE_URL not set");
        return;
    };
    let table = "tellurion_postgis_assets_live_test_delete";
    seed_data_table(&database_url, table).await;
    seed_assets_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let store = driver
        .asset_record_store()
        .expect("driver exposes AssetRecordStore");
    let collection = collection(table);
    let digest = tellurion_core::compute_sha256(b"a");

    store
        .register(
            &collection,
            None,
            "thumb",
            managed_new_record(Uuid::new_v4(), digest),
        )
        .await
        .expect("register succeeds");

    let deleted = store
        .delete(&collection, None, "thumb")
        .await
        .expect("delete succeeds");
    assert!(deleted.is_some());

    let after = store
        .get(&collection, None, "thumb")
        .await
        .expect("get succeeds");
    assert!(after.is_none());
}

/// `list` (reconcile surface, `#93`): every row this collection's table
/// holds, both collection- and item-level, `item_id`/`key` correctly
/// carried back — the real SQL path `asset_sql::build_list_plan` builds.
#[tokio::test]
async fn list_returns_every_row_scoped_by_item_and_key() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping list_returns_every_row_scoped_by_item_and_key: TELLURION_TEST_DATABASE_URL not set");
        return;
    };
    let table = "tellurion_postgis_assets_live_test_list";
    seed_data_table(&database_url, table).await;
    seed_assets_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let store = driver
        .asset_record_store()
        .expect("driver exposes AssetRecordStore");
    let collection = collection(table);
    let digest = tellurion_core::compute_sha256(b"a");

    store
        .register(
            &collection,
            None,
            "thumb",
            managed_new_record(Uuid::new_v4(), digest.clone()),
        )
        .await
        .expect("collection-level register succeeds");
    store
        .register(
            &collection,
            Some("feature-1"),
            "photo",
            managed_new_record(Uuid::new_v4(), digest),
        )
        .await
        .expect("item-level register succeeds");

    let entries = store.list(&collection).await.expect("list succeeds");
    assert_eq!(entries.len(), 2);
    assert!(entries
        .iter()
        .any(|entry| entry.item_id.is_none() && entry.key == "thumb"));
    assert!(entries
        .iter()
        .any(|entry| entry.item_id.as_deref() == Some("feature-1") && entry.key == "photo"));
}

/// `item_assets` (`#221`) against the real table: one batched
/// `item_id = ANY($1)` read returns every named item's records and NOTHING
/// else — not another item's, and above all not the collection-level row,
/// whose `''` sentinel the statement excludes outright. This is the half a
/// hand-built fixture cannot prove: that the actual Postgres predicate, run
/// against the actual `UNIQUE (item_id, asset_key)`-shaped table `ingest`
/// provisions, draws the scope boundary `#221` depends on.
#[tokio::test]
async fn item_assets_batches_every_named_item_and_excludes_collection_scope() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping item_assets_batches_every_named_item_and_excludes_collection_scope: TELLURION_TEST_DATABASE_URL not set");
        return;
    };
    let table = "tellurion_postgis_assets_live_test_item_lookup";
    seed_data_table(&database_url, table).await;
    seed_assets_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let store = driver
        .asset_record_store()
        .expect("driver exposes AssetRecordStore");
    let collection = collection(table);
    let digest = tellurion_core::compute_sha256(b"a");

    // One collection-level record, and one record on each of three items.
    store
        .register(
            &collection,
            None,
            "license",
            managed_new_record(Uuid::new_v4(), digest.clone()),
        )
        .await
        .expect("collection-level register succeeds");
    for item in ["feature-1", "feature-2", "feature-3"] {
        store
            .register(
                &collection,
                Some(item),
                "cog",
                managed_new_record(Uuid::new_v4(), digest.clone()),
            )
            .await
            .expect("item-level register succeeds");
    }

    // A page naming two of the three items, plus the `""` a feature with no
    // `id` member degrades to and an id that has no records at all.
    let page = vec![
        "feature-1".to_string(),
        String::new(),
        "feature-3".to_string(),
        "feature-absent".to_string(),
    ];
    let entries = store
        .item_assets(&collection, &page)
        .await
        .expect("the batched item lookup succeeds");

    assert_eq!(
        entries.len(),
        2,
        "expected exactly the two named items' records, got: {entries:?}"
    );
    assert!(entries
        .iter()
        .all(|entry| entry.item_id.is_some() && entry.key == "cog"));
    let mut ids: Vec<&str> = entries
        .iter()
        .filter_map(|entry| entry.item_id.as_deref())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["feature-1", "feature-3"]);
    assert!(
        !entries.iter().any(|entry| entry.key == "license"),
        "the collection-level record must never come back from an item lookup, even when the \
         page carried the empty-string scope"
    );

    // Every state is reported — the advertisability rule belongs to the
    // STAC lane, not to this capability. Both registrations above are
    // managed, so both are still `pending`.
    assert!(entries
        .iter()
        .all(|entry| entry.record.state == AssetState::Pending));

    // The empty page never touches the pool at all.
    assert!(store
        .item_assets(&collection, &[])
        .await
        .expect("an empty page succeeds")
        .is_empty());
}

/// The same named `AssetsTableMissing` refusal every other assets query
/// gets — never an empty result, which an operator could not tell apart
/// from a provisioned table whose items simply have no records.
#[tokio::test]
async fn item_assets_refuses_by_name_when_the_assets_table_is_absent() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping item_assets_refuses_by_name_when_the_assets_table_is_absent: TELLURION_TEST_DATABASE_URL not set");
        return;
    };
    let table = "tellurion_postgis_assets_live_test_item_lookup_missing";
    // Deliberately no `seed_assets_table` call.
    seed_data_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let store = driver
        .asset_record_store()
        .expect("driver exposes AssetRecordStore");

    let err = store
        .item_assets(&collection(table), &["feature-1".to_string()])
        .await
        .unwrap_err();
    match err {
        tellurion_core::Error::Config(message) => {
            assert!(message.contains("asset"), "message was: {message}");
            assert!(
                message.contains(&format!("{table}_assets")),
                "message was: {message}"
            );
        }
        other => panic!("expected a named Config error, got {other:?}"),
    }
}
