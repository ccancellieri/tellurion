//! Live tests for the write lane's first slice (`#25`, the transactional-
//! outbox design): `WriteSink`/`OutboxSource` through the actual
//! `PostgisDriverFactory` entry point, against a real PostGIS instance.
//! Skipped gracefully unless `TELLURION_TEST_DATABASE_URL` is set, matching
//! every other live test in this workspace.
//!
//! The outbox table DDL below is hand-kept in sync with
//! `tellurion-ingest::outbox`'s own `create_outbox_table_sql` — the two
//! crates never depend on each other (`tellurion-postgis::write_sql`'s own
//! module doc explains why), so this is deliberately NOT imported from that
//! crate.

use std::env;

use serde_json::json;
use tellurion_core::{
    CollectionDecl, DriverFactory, Error as CoreError, IdType, Mutation, MutationKind,
    PropertyDecl, PropertyType, RequestedCrs, SchemaDecl, Sequence, StorageDecl, StorageDriver,
};
use tellurion_postgis::test_harness;
use tellurion_postgis::PostgisDriverFactory;

const URL_ENV_VAR: &str = "TELLURION_POSTGIS_WRITE_LIVE_TEST_URL";

async fn connect(database_url: &str) -> tokio_postgres::Client {
    test_harness::connect(database_url).await
}

/// A writable data table: an integer pk, a point geometry, and a mix of
/// typed attribute columns wide enough to exercise every `PropertyType`'s
/// `$N::text::<cast>` bind. No outbox table — callers that need one call
/// [`seed_outbox_table`] separately, so a test can exercise "the outbox
/// table is absent" without a second fixture.
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
                 name text,
                 population integer,
                 active boolean
             );"
        ),
    )
    .await
    .expect("seeds the data table");
}

/// Same shape as [`seed_data_table`], but the geometry column is typed
/// `geometry(Point, {srid})` for a caller-chosen `srid` rather than the
/// fixed 4326 every other fixture in this file uses — the fixture the
/// `Content-Crs`-on-write live tests below seed against, so a real
/// non-4326-typed column proves the actual bug this lane closes: before
/// `write_sql::input_geom_expr` existed, every insert this driver ever ran
/// tagged its geometry SRID 4326 regardless of the column's own typmod,
/// which Postgres/PostGIS would reject outright against a column
/// constrained to a different SRID (a `SRID mismatch` error), not merely
/// store silently-wrong coordinates.
async fn seed_srid_data_table(database_url: &str, table: &str, srid: i32) {
    let client = connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table} CASCADE;
             DROP TABLE IF EXISTS {table}_outbox;
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, {srid}),
                 name text,
                 population integer,
                 active boolean
             );"
        ),
    )
    .await
    .expect("seeds the srid-typed data table");
}

/// Same shape as [`seed_data_table`], but the pk is a real `uuid` column
/// with a server-side default (`gen_random_uuid()`, built into PostgreSQL
/// core since v13 — no extension needed) — the fixture every `#87` `Uuid`
/// id-type live test seeds against.
async fn seed_uuid_data_table(database_url: &str, table: &str) {
    let client = connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table} CASCADE;
             DROP TABLE IF EXISTS {table}_outbox;
             CREATE TABLE {table} (
                 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
                 geom geometry(Point, 4326),
                 name text,
                 population integer,
                 active boolean
             );"
        ),
    )
    .await
    .expect("seeds the uuid-pk data table");
}

/// Same shape as [`seed_data_table`], but the pk is a real `text` column
/// with deliberately NO server-side default — the fixture every `#94` `Text`
/// id-type create live test seeds against. Unlike [`seed_uuid_data_table`],
/// the absence of a default is the expected shape for `Text`: the pk is
/// always caller-supplied, so there is nothing here for the database to
/// mint.
async fn seed_text_data_table(database_url: &str, table: &str) {
    let client = connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table} CASCADE;
             DROP TABLE IF EXISTS {table}_outbox;
             CREATE TABLE {table} (
                 id text PRIMARY KEY,
                 geom geometry(Point, 4326),
                 name text,
                 population integer,
                 active boolean
             );"
        ),
    )
    .await
    .expect("seeds the text-pk data table");
}

/// Matches `tellurion-ingest::outbox::create_outbox_table_sql` exactly — see
/// this file's own module doc.
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

fn collection(table: &str) -> CollectionDecl {
    serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\n"
    ))
    .expect("valid CollectionDecl yaml")
}

/// Same shape as [`collection`], with `srid` set to the collection's own
/// storage SRID (OGC API Features Part 4, `/req/features/crs-other-crs`
/// live coverage below) — `srid` is `#[serde(skip)]` on `CollectionDecl`
/// (only ever derived at request time by `Router::effective_decl`, never
/// operator-configured), so a direct-driver-call test like this one has to
/// set it by hand the same way `Router` would have.
fn collection_with_srid(table: &str, srid: i32) -> CollectionDecl {
    let mut decl = collection(table);
    decl.srid = Some(srid);
    decl
}

/// Same shape as [`collection`], with `id_type: uuid` declared (`#87`).
fn collection_uuid(table: &str) -> CollectionDecl {
    let mut decl = collection(table);
    decl.id_type = IdType::Uuid;
    decl
}

/// Same shape as [`collection`], with `id_type: text` declared (`#94`).
fn collection_text(table: &str) -> CollectionDecl {
    let mut decl = collection(table);
    decl.id_type = IdType::Text;
    decl
}

/// Same shape as [`collection`], with a declared schema narrow enough to
/// exercise every `PropertyType`'s cast and the required/type-mismatch
/// rejection paths.
fn collection_with_schema(table: &str) -> CollectionDecl {
    let mut decl = collection(table);
    decl.schema = Some(SchemaDecl {
        properties: vec![
            PropertyDecl {
                name: "name".to_string(),
                type_: PropertyType::String,
                required: true,
            },
            PropertyDecl {
                name: "population".to_string(),
                type_: PropertyType::Integer,
                required: false,
            },
            PropertyDecl {
                name: "active".to_string(),
                type_: PropertyType::Boolean,
                required: false,
            },
        ],
        additional_properties: true,
    });
    decl
}

async fn build_driver(database_url: &str) -> std::sync::Arc<dyn StorageDriver> {
    // Safety: this test binary sets this one env var exactly once per test
    // process before any connection pool spawns worker tasks, matching
    // `tests/live.rs`'s own documented safety argument for the same pattern.
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

/// A write goes through `WriteSink` and durably lands both the data row and
/// the matching outbox obligation — the design doc's section 8 acceptance
/// criterion's first half. Also proves an `Upsert` against an existing id
/// replaces the row in place (the "PUT replaces" semantics the write
/// endpoint's own doc describes) rather than erroring or duplicating.
#[tokio::test]
async fn upsert_and_replace_land_the_row_and_an_outbox_obligation() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping upsert_and_replace_land_the_row_and_an_outbox_obligation: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_upsert";
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let outbox = driver.outbox_source().expect("driver exposes OutboxSource");
    let collection = collection(table);

    let feature = json!({
        "type": "Feature",
        "geometry": {"type": "Point", "coordinates": [10.0, 45.0]},
        "properties": {"name": "acme", "population": 42, "active": true}
    });
    let sequence = sink
        .apply(
            &collection,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(feature.clone()),
            },
        )
        .await
        .expect("upsert succeeds");
    assert_eq!(sequence, Sequence(1));

    let raw = connect(&database_url).await;
    let row = raw
        .query_one(
            &format!("SELECT name, population, active FROM {table} WHERE id = 1"),
            &[],
        )
        .await
        .expect("the row exists");
    let name: String = row.get(0);
    let population: i32 = row.get(1);
    let active: bool = row.get(2);
    assert_eq!(name, "acme");
    assert_eq!(population, 42);
    assert!(active);

    // Replace the same id with different values — an upsert, not a second row.
    let replacement = json!({
        "type": "Feature",
        "geometry": {"type": "Point", "coordinates": [11.0, 46.0]},
        "properties": {"name": "acme-renamed", "population": 43, "active": false}
    });
    let second_sequence = sink
        .apply(
            &collection,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(replacement),
            },
        )
        .await
        .expect("replace succeeds");
    assert_eq!(
        second_sequence,
        Sequence(2),
        "each apply advances the sequence"
    );

    let count: i64 = raw
        .query_one(&format!("SELECT count(*) FROM {table}"), &[])
        .await
        .expect("count succeeds")
        .get(0);
    assert_eq!(count, 1, "a replace must update in place, never duplicate");

    let row = raw
        .query_one(&format!("SELECT name FROM {table} WHERE id = 1"), &[])
        .await
        .expect("the row still exists");
    let name: String = row.get(0);
    assert_eq!(name, "acme-renamed");

    // The outbox carries both obligations, in order, and idempotent version
    // stamps (`#25` design doc section 4: version IS the committing
    // sequence in this first slice).
    let obligations = outbox
        .read_after(&collection, Sequence(0), 10)
        .await
        .expect("read_after succeeds");
    assert_eq!(obligations.len(), 2);
    assert_eq!(obligations[0].sequence, Sequence(1));
    assert_eq!(obligations[0].feature_id, "1");
    assert_eq!(obligations[0].version, Sequence(1));
    assert_eq!(obligations[1].sequence, Sequence(2));
    assert_eq!(obligations[1].version, Sequence(2));
    match &obligations[1].kind {
        MutationKind::Upsert(value) => {
            assert_eq!(value["properties"]["name"], "acme-renamed");
        }
        other => panic!("expected Upsert, got {other:?}"),
    }

    let high_water = outbox
        .primary_high_water(&collection)
        .await
        .expect("primary_high_water succeeds");
    assert_eq!(high_water, Sequence(2));
}

/// A `Delete` mutation removes the row and appends a tombstone obligation
/// with no payload.
#[tokio::test]
async fn delete_removes_the_row_and_appends_a_tombstone_obligation() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping delete_removes_the_row_and_appends_a_tombstone_obligation: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_delete";
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let outbox = driver.outbox_source().expect("driver exposes OutboxSource");
    let collection = collection(table);

    let feature = json!({"type": "Feature", "geometry": null, "properties": {}});
    sink.apply(
        &collection,
        Mutation {
            feature_id: "1".to_string(),
            kind: MutationKind::Upsert(feature),
        },
    )
    .await
    .expect("upsert succeeds");

    sink.apply(
        &collection,
        Mutation {
            feature_id: "1".to_string(),
            kind: MutationKind::Delete,
        },
    )
    .await
    .expect("delete succeeds");

    let raw = connect(&database_url).await;
    let count: i64 = raw
        .query_one(&format!("SELECT count(*) FROM {table} WHERE id = 1"), &[])
        .await
        .expect("count succeeds")
        .get(0);
    assert_eq!(count, 0, "the row must be gone after delete");

    let obligations = outbox
        .read_after(&collection, Sequence(1), 10)
        .await
        .expect("read_after succeeds");
    assert_eq!(obligations.len(), 1);
    assert_eq!(obligations[0].kind, MutationKind::Delete);
}

/// `read_after` never re-serves an already-seen obligation, and reports
/// "caught up" as an empty page — the ordering/dedup basics this first
/// slice defines (design doc section 4).
#[tokio::test]
async fn read_after_ordering_and_catch_up() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping read_after_ordering_and_catch_up: TELLURION_TEST_DATABASE_URL not set");
        return;
    };
    let table = "tellurion_postgis_write_live_test_ordering";
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let outbox = driver.outbox_source().expect("driver exposes OutboxSource");
    let collection = collection(table);

    for id in 1..=3 {
        let feature = json!({
            "type": "Feature",
            "geometry": {"type": "Point", "coordinates": [1.0, 1.0]},
            "properties": {}
        });
        sink.apply(
            &collection,
            Mutation {
                feature_id: id.to_string(),
                kind: MutationKind::Upsert(feature),
            },
        )
        .await
        .expect("upsert succeeds");
    }

    let all = outbox
        .read_after(&collection, Sequence(0), 10)
        .await
        .expect("read_after succeeds");
    assert_eq!(all.len(), 3);
    assert!(
        all.windows(2).all(|w| w[0].sequence < w[1].sequence),
        "obligations must come back strictly ascending by sequence"
    );

    let limited = outbox
        .read_after(&collection, Sequence(0), 2)
        .await
        .expect("read_after with a limit succeeds");
    assert_eq!(limited.len(), 2, "limit is honored");

    let remainder = outbox
        .read_after(&collection, limited[1].sequence, 10)
        .await
        .expect("read_after resuming from the last-seen sequence succeeds");
    assert_eq!(remainder.len(), 1, "no obligation is skipped or re-served");
    assert_eq!(remainder[0].feature_id, "3");

    let caught_up = outbox
        .read_after(&collection, all.last().unwrap().sequence, 10)
        .await
        .expect("read_after at the high-water mark succeeds");
    assert!(caught_up.is_empty(), "Ok(vec![]) means caught up");
}

/// The core atomicity invariant (design doc section 2, invariant 2): the
/// data mutation and the outbox obligation commit in ONE transaction. Here
/// the data table exists but the outbox table does not — the data
/// statement itself would succeed in isolation, but `apply` must fail with
/// the named `OutboxTableMissing` error AND leave no trace of the row it
/// almost wrote, proving the rollback actually happened rather than the
/// data half silently landing anyway.
#[tokio::test]
async fn apply_rolls_back_the_data_mutation_when_the_outbox_table_is_absent() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping apply_rolls_back_the_data_mutation_when_the_outbox_table_is_absent: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_atomicity";
    // Deliberately no `seed_outbox_table` call.
    seed_data_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection(table);

    let feature = json!({
        "type": "Feature",
        "geometry": {"type": "Point", "coordinates": [10.0, 45.0]},
        "properties": {"name": "acme"}
    });
    let result = sink
        .apply(
            &collection,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(feature),
            },
        )
        .await;

    match result {
        Err(tellurion_core::Error::Config(message)) => {
            assert!(message.contains("outbox"), "message was: {message}");
            assert!(
                message.contains(&format!("{table}_outbox")),
                "message was: {message}"
            );
        }
        other => panic!("expected a named Config error, got {}", other.is_ok()),
    }

    let raw = connect(&database_url).await;
    let count: i64 = raw
        .query_one(&format!("SELECT count(*) FROM {table}"), &[])
        .await
        .expect("count succeeds")
        .get(0);
    assert_eq!(
        count, 0,
        "the data mutation must have rolled back along with the failed outbox insert"
    );
}

/// `OutboxSource::read_after`/`primary_high_water` against a collection
/// whose outbox table was never provisioned fail with the same named error
/// `WriteSink::apply` does — never a raw, unnamed SQL error.
#[tokio::test]
async fn outbox_reads_fail_with_a_named_error_when_the_table_is_absent() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping outbox_reads_fail_with_a_named_error_when_the_table_is_absent: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_absent_reads";
    seed_data_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let outbox = driver.outbox_source().expect("driver exposes OutboxSource");
    let collection = collection(table);

    match outbox.read_after(&collection, Sequence(0), 10).await {
        Err(tellurion_core::Error::Config(message)) => {
            assert!(message.contains("outbox"), "message was: {message}");
        }
        other => panic!("expected a named Config error, got {}", other.is_ok()),
    }

    match outbox.primary_high_water(&collection).await {
        Err(tellurion_core::Error::Config(message)) => {
            assert!(message.contains("outbox"), "message was: {message}");
        }
        other => panic!("expected a named Config error, got {}", other.is_ok()),
    }
}

/// A collection's declared schema (`#44`) is enforced by the caller before
/// `apply` — this test proves the OTHER half: once past that gate, a
/// declared property casts through to its real column type correctly (this
/// is where `PropertyType::Integer`/`Boolean` actually round-trip against
/// real `integer`/`boolean` columns, not just build the right SQL text —
/// `write_sql`'s own unit tests already cover the text; this proves it
/// executes).
#[tokio::test]
async fn a_declared_schemas_typed_properties_round_trip_through_real_columns() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping a_declared_schemas_typed_properties_round_trip_through_real_columns: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_typed";
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection_with_schema(table);

    let feature = json!({
        "type": "Feature",
        "geometry": null,
        "properties": {"name": "typed", "population": 7, "active": false}
    });
    sink.apply(
        &collection,
        Mutation {
            feature_id: "1".to_string(),
            kind: MutationKind::Upsert(feature),
        },
    )
    .await
    .expect("typed upsert succeeds");

    let raw = connect(&database_url).await;
    let row = raw
        .query_one(
            &format!("SELECT population, active FROM {table} WHERE id = 1"),
            &[],
        )
        .await
        .expect("the row exists");
    let population: i32 = row.get(0);
    let active: bool = row.get(1);
    assert_eq!(population, 7);
    assert!(!active);
}

/// A free-form collection (no declared schema) writes a property that names
/// a real column it never declared — the live-catalog-lookup fallback in
/// `resolve_property_types`, proving the free-form "accept as-is" path
/// actually persists data, not just skips validation.
#[tokio::test]
async fn a_free_form_collection_writes_an_undeclared_property_via_the_live_catalog_lookup() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping a_free_form_collection_writes_an_undeclared_property_via_the_live_catalog_lookup: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_freeform";
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection(table);
    assert!(collection.schema.is_none(), "this collection is free-form");

    let feature = json!({
        "type": "Feature",
        "geometry": null,
        "properties": {"population": 99}
    });
    sink.apply(
        &collection,
        Mutation {
            feature_id: "1".to_string(),
            kind: MutationKind::Upsert(feature),
        },
    )
    .await
    .expect("free-form upsert against a real column succeeds");

    let raw = connect(&database_url).await;
    let population: i32 = raw
        .query_one(&format!("SELECT population FROM {table} WHERE id = 1"), &[])
        .await
        .expect("the row exists")
        .get(0);
    assert_eq!(population, 99);
}

// -- WriteSink::create (`#88`) -----------------------------------------------

/// A server-assigned create mints an id from the pk column's own
/// `bigserial` default, commits the row and a matching outbox obligation in
/// the same transaction, and hands the minted id back — the create-lane
/// counterpart of `upsert_and_replace_land_the_row_and_an_outbox_
/// obligation`.
#[tokio::test]
async fn create_mints_a_server_assigned_id_and_lands_the_row_and_an_outbox_obligation() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping create_mints_a_server_assigned_id_and_lands_the_row_and_an_outbox_obligation: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_create";
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let outbox = driver.outbox_source().expect("driver exposes OutboxSource");
    let collection = collection(table);

    let feature = json!({
        "type": "Feature",
        "geometry": {"type": "Point", "coordinates": [10.0, 45.0]},
        "properties": {"name": "acme", "population": 42, "active": true}
    });
    let (new_id, sequence) = sink
        .create(&collection, feature)
        .await
        .expect("create succeeds");
    assert_eq!(new_id, "1", "the first create on an empty table mints id 1");
    assert_eq!(sequence, Sequence(1));

    let raw = connect(&database_url).await;
    let row = raw
        .query_one(
            &format!("SELECT name, population, active FROM {table} WHERE id = 1"),
            &[],
        )
        .await
        .expect("the row exists at the minted id");
    let name: String = row.get(0);
    let population: i32 = row.get(1);
    let active: bool = row.get(2);
    assert_eq!(name, "acme");
    assert_eq!(population, 42);
    assert!(active);

    let obligations = outbox
        .read_after(&collection, Sequence(0), 10)
        .await
        .expect("read_after succeeds");
    assert_eq!(obligations.len(), 1);
    assert_eq!(obligations[0].feature_id, "1");
    match &obligations[0].kind {
        MutationKind::Upsert(value) => {
            assert_eq!(value["properties"]["name"], "acme");
        }
        other => panic!("expected Upsert, got {other:?}"),
    }
}

/// Two creates against the same collection mint distinct, monotonically
/// increasing ids and outbox sequences — the pk's own `bigserial` sequence
/// never repeats a value, and `create`'s own sequence tracks the outbox
/// insert order.
#[tokio::test]
async fn two_creates_mint_distinct_monotonic_ids() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping two_creates_mint_distinct_monotonic_ids: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_create_monotonic";
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection(table);

    let feature = |name: &str| {
        json!({
            "type": "Feature",
            "geometry": null,
            "properties": {"name": name}
        })
    };

    let (first_id, first_sequence) = sink
        .create(&collection, feature("first"))
        .await
        .expect("first create succeeds");
    let (second_id, second_sequence) = sink
        .create(&collection, feature("second"))
        .await
        .expect("second create succeeds");

    assert_ne!(first_id, second_id, "two creates must mint distinct ids");
    let first_id: i64 = first_id.parse().expect("id is numeric");
    let second_id: i64 = second_id.parse().expect("id is numeric");
    assert!(second_id > first_id, "ids must increase monotonically");
    assert!(
        second_sequence > first_sequence,
        "the outbox sequence must also increase monotonically"
    );

    let raw = connect(&database_url).await;
    let count: i64 = raw
        .query_one(&format!("SELECT count(*) FROM {table}"), &[])
        .await
        .expect("count succeeds")
        .get(0);
    assert_eq!(count, 2, "both creates must have landed distinct rows");
}

/// The atomicity invariant, for create: the outbox table is absent, so the
/// server-assigned `INSERT` succeeds in isolation but the transaction as a
/// whole must roll back — no row survives, matching `apply_rolls_back_the_
/// data_mutation_when_the_outbox_table_is_absent`'s own proof for
/// upsert/delete.
#[tokio::test]
async fn create_rolls_back_the_data_mutation_when_the_outbox_table_is_absent() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping create_rolls_back_the_data_mutation_when_the_outbox_table_is_absent: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_create_atomicity";
    // Deliberately no `seed_outbox_table` call.
    seed_data_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection(table);

    let feature = json!({
        "type": "Feature",
        "geometry": {"type": "Point", "coordinates": [10.0, 45.0]},
        "properties": {"name": "acme"}
    });
    match sink.create(&collection, feature).await {
        Err(tellurion_core::Error::Config(message)) => {
            assert!(message.contains("outbox"), "message was: {message}");
        }
        other => panic!("expected a named Config error, got {}", other.is_ok()),
    }

    let raw = connect(&database_url).await;
    let count: i64 = raw
        .query_one(&format!("SELECT count(*) FROM {table}"), &[])
        .await
        .expect("count succeeds")
        .get(0);
    assert_eq!(
        count, 0,
        "the server-assigned insert must have rolled back along with the failed outbox insert"
    );
}

/// A pk column with no server-side default (not a `bigserial`/identity
/// column) is an unprovisioned create target — `create` refuses cleanly, by
/// name, rather than the caller seeing a raw `NOT NULL` violation.
#[tokio::test]
async fn create_fails_named_when_the_pk_column_has_no_server_default() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping create_fails_named_when_the_pk_column_has_no_server_default: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_create_no_default";
    let client = connect(&database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table} CASCADE;
             DROP TABLE IF EXISTS {table}_outbox;
             CREATE TABLE {table} (
                 id bigint PRIMARY KEY,
                 geom geometry(Point, 4326),
                 name text
             );"
        ),
    )
    .await
    .expect("seeds a data table whose pk has no server default");
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection(table);

    let feature = json!({"type": "Feature", "geometry": null, "properties": {}});
    match sink.create(&collection, feature).await {
        Err(tellurion_core::Error::Config(message)) => {
            assert!(
                message.contains("server-assigned"),
                "message was: {message}"
            );
        }
        other => panic!("expected a named Config error, got {}", other.is_ok()),
    }

    let raw = connect(&database_url).await;
    let count: i64 = raw
        .query_one(&format!("SELECT count(*) FROM {table}"), &[])
        .await
        .expect("count succeeds")
        .get(0);
    assert_eq!(count, 0, "a refused create must never land a partial row");
}

/// `#87`: a `Uuid` id-type collection's `create` mints a real server-side
/// `uuid` (the table's own `DEFAULT gen_random_uuid()`) rather than an
/// integer, and the outbox obligation it appends in the same transaction
/// carries that exact minted value as `feature_id` — the `Uuid` counterpart
/// of `create_mints_a_server_assigned_id_and_lands_the_row_and_an_outbox_
/// obligation`. Two creates minting distinct ids also proves the mint is
/// real (a fresh `gen_random_uuid()` call each time), not a fixed or
/// repeating stand-in — `Uuid` values have no inherent ordering the way a
/// `bigserial` counter does, so this checks distinctness, not monotonicity.
#[tokio::test]
async fn create_mints_a_uuid_and_the_outbox_row_carries_it() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping create_mints_a_uuid_and_the_outbox_row_carries_it: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_create_uuid";
    seed_uuid_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let outbox = driver.outbox_source().expect("driver exposes OutboxSource");
    let collection = collection_uuid(table);

    let feature = json!({
        "type": "Feature",
        "geometry": {"type": "Point", "coordinates": [10.0, 45.0]},
        "properties": {"name": "acme", "population": 42, "active": true}
    });
    let (first_id, first_sequence) = sink
        .create(&collection, feature.clone())
        .await
        .expect("create succeeds");
    let parsed_first = uuid::Uuid::parse_str(&first_id).expect("minted id is a real uuid");
    assert_eq!(first_sequence, Sequence(1));

    let raw = connect(&database_url).await;
    let row = raw
        .query_one(
            &format!("SELECT name, population, active FROM {table} WHERE id = $1"),
            &[&parsed_first],
        )
        .await
        .expect("the row exists at the minted id");
    let name: String = row.get(0);
    let population: i32 = row.get(1);
    let active: bool = row.get(2);
    assert_eq!(name, "acme");
    assert_eq!(population, 42);
    assert!(active);

    let (second_id, second_sequence) = sink
        .create(&collection, feature)
        .await
        .expect("second create succeeds");
    assert_ne!(first_id, second_id, "two creates must mint distinct uuids");
    assert_eq!(second_sequence, Sequence(2));

    let obligations = outbox
        .read_after(&collection, Sequence(0), 10)
        .await
        .expect("read_after succeeds");
    assert_eq!(obligations.len(), 2);
    assert_eq!(
        obligations[0].feature_id, first_id,
        "the outbox row carries the exact minted uuid"
    );
    assert_eq!(obligations[1].feature_id, second_id);
}

/// `#87`: a collection declares `id_type: uuid` over a table whose physical
/// pk column is `bigint`, not `uuid` — `validate_id_type_for_create` refuses
/// this by name, before the `INSERT` is ever built, the declaration-
/// validation counterpart of `create_fails_named_when_the_pk_column_has_no_
/// server_default` above (a pk of the RIGHT type but no default) for a pk of
/// the WRONG type entirely.
#[tokio::test]
async fn create_fails_named_when_the_declared_id_type_does_not_match_the_physical_pk_column() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping create_fails_named_when_the_declared_id_type_does_not_match_the_physical_pk_column: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    // `#272`: shortened from `..._create_id_type_mismatch` (57 bytes), whose
    // derived `{table}_outbox` was 64 — one byte past the limit, and so
    // silently truncated. See the text-pk sibling below.
    let table = "tellurion_postgis_write_live_test_create_int_mismatch";
    // A perfectly ordinary integer-pk table — `collection_uuid` below is
    // what makes this a mismatch, not the table itself.
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection_uuid(table);

    let feature = json!({"type": "Feature", "geometry": null, "properties": {}});
    match sink.create(&collection, feature).await {
        Err(CoreError::Config(message)) => {
            assert!(message.contains("id_type"), "message was: {message}");
        }
        other => panic!("expected a named Config error, got {}", other.is_ok()),
    }

    let raw = connect(&database_url).await;
    let count: i64 = raw
        .query_one(&format!("SELECT count(*) FROM {table}"), &[])
        .await
        .expect("count succeeds")
        .get(0);
    assert_eq!(count, 0, "a refused create must never land a partial row");
}

/// `#87`: a `uuid`-typed pk column with no server-side default (not backed
/// by `DEFAULT gen_random_uuid()` or similar) is an unprovisioned create
/// target for exactly the same reason a `bigint` pk with no `bigserial`
/// default is — `validate_id_type_for_create` confirms the column really is
/// `uuid` (it is) and lets `create` proceed to the `INSERT`, which then
/// fails its own `NOT NULL` violation on the omitted pk column, translated
/// to the same named `PkNotServerAssignable` refusal
/// `create_fails_named_when_the_pk_column_has_no_server_default` proves for
/// `Integer` — extending that idiom to `Uuid`, per the pk column's own real
/// type rather than a special case.
#[tokio::test]
async fn create_fails_named_when_a_uuid_pk_column_has_no_server_default() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping create_fails_named_when_a_uuid_pk_column_has_no_server_default: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_create_uuid_no_default";
    let client = connect(&database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table} CASCADE;
             DROP TABLE IF EXISTS {table}_outbox;
             CREATE TABLE {table} (
                 id uuid PRIMARY KEY,
                 geom geometry(Point, 4326),
                 name text
             );"
        ),
    )
    .await
    .expect("seeds a uuid-pk data table whose pk has no server default");
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection_uuid(table);

    let feature = json!({"type": "Feature", "geometry": null, "properties": {}});
    match sink.create(&collection, feature).await {
        Err(CoreError::Config(message)) => {
            assert!(
                message.contains("server-assigned"),
                "message was: {message}"
            );
        }
        other => panic!("expected a named Config error, got {}", other.is_ok()),
    }

    let raw = connect(&database_url).await;
    let count: i64 = raw
        .query_one(&format!("SELECT count(*) FROM {table}"), &[])
        .await
        .expect("count succeeds")
        .get(0);
    assert_eq!(count, 0, "a refused create must never land a partial row");
}

/// `#94`: the heart of the text-pk create-mode inversion — a `Text`
/// id-type collection's `create` binds the caller-supplied `id` from the
/// feature body directly (not server-minted), lands the row at exactly that
/// id, and the outbox obligation it appends in the same transaction carries
/// that exact id as `feature_id` — the `Text` counterpart of
/// `create_mints_a_server_assigned_id_and_lands_the_row_and_an_outbox_
/// obligation`/`create_mints_a_uuid_and_the_outbox_row_carries_it`.
#[tokio::test]
async fn create_lands_a_caller_supplied_text_id_and_the_outbox_row_carries_it() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping create_lands_a_caller_supplied_text_id_and_the_outbox_row_carries_it: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_create_text";
    seed_text_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let outbox = driver.outbox_source().expect("driver exposes OutboxSource");
    let collection = collection_text(table);

    let feature = json!({
        "type": "Feature",
        "id": "acme-1",
        "geometry": {"type": "Point", "coordinates": [10.0, 45.0]},
        "properties": {"name": "acme", "population": 42, "active": true}
    });
    let (new_id, sequence) = sink
        .create(&collection, feature)
        .await
        .expect("create succeeds");
    assert_eq!(
        new_id, "acme-1",
        "the returned id is exactly what the database stored, read back via RETURNING"
    );
    assert_eq!(sequence, Sequence(1));

    let raw = connect(&database_url).await;
    let row = raw
        .query_one(
            &format!("SELECT name, population, active FROM {table} WHERE id = $1"),
            &[&"acme-1"],
        )
        .await
        .expect("the row exists at the caller-supplied id");
    let name: String = row.get(0);
    let population: i32 = row.get(1);
    let active: bool = row.get(2);
    assert_eq!(name, "acme");
    assert_eq!(population, 42);
    assert!(active);

    let obligations = outbox
        .read_after(&collection, Sequence(0), 10)
        .await
        .expect("read_after succeeds");
    assert_eq!(obligations.len(), 1);
    assert_eq!(obligations[0].feature_id, "acme-1");
    match &obligations[0].kind {
        MutationKind::Upsert(value) => {
            assert_eq!(value["properties"]["name"], "acme");
        }
        other => panic!("expected Upsert, got {other:?}"),
    }
}

/// `#94`: the create-mode inversion's refusal half — a `Text` id-type
/// collection's feature body with no top-level `id` refuses by name before
/// any SQL runs, rather than falling through to a server-minted id the way
/// `Integer`/`Uuid` would.
#[tokio::test]
async fn create_fails_named_when_a_text_collections_feature_body_has_no_id() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping create_fails_named_when_a_text_collections_feature_body_has_no_id: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_create_text_missing_id";
    seed_text_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection_text(table);

    let feature = json!({
        "type": "Feature",
        "geometry": {"type": "Point", "coordinates": [10.0, 45.0]},
        "properties": {"name": "acme"}
    });
    match sink.create(&collection, feature).await {
        Err(tellurion_core::Error::Invalid(message)) => {
            assert!(message.contains("id"), "message was: {message}");
        }
        other => panic!("expected a named Invalid error, got {}", other.is_ok()),
    }

    let raw = connect(&database_url).await;
    let count: i64 = raw
        .query_one(&format!("SELECT count(*) FROM {table}"), &[])
        .await
        .expect("count succeeds")
        .get(0);
    assert_eq!(count, 0, "a refused create must never land a partial row");
}

/// `#94`: creating with an id that already exists in the table is a named
/// `409`, never a raw constraint-violation error — the create-lane
/// counterpart of the assets lane's own `AssetKeyConflict`.
#[tokio::test]
async fn create_fails_with_a_named_409_when_a_text_id_already_exists() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping create_fails_with_a_named_409_when_a_text_id_already_exists: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_create_text_conflict";
    seed_text_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection_text(table);

    let first = json!({
        "type": "Feature",
        "id": "acme-1",
        "geometry": null,
        "properties": {"name": "first"}
    });
    sink.create(&collection, first)
        .await
        .expect("the first create with this id succeeds");

    let conflicting = json!({
        "type": "Feature",
        "id": "acme-1",
        "geometry": null,
        "properties": {"name": "second"}
    });
    match sink.create(&collection, conflicting).await {
        Err(tellurion_core::Error::Conflict(message)) => {
            assert!(message.contains("acme-1"), "message was: {message}");
        }
        other => panic!("expected a named Conflict error, got {}", other.is_ok()),
    }

    let raw = connect(&database_url).await;
    let count: i64 = raw
        .query_one(&format!("SELECT count(*) FROM {table}"), &[])
        .await
        .expect("count succeeds")
        .get(0);
    assert_eq!(
        count, 1,
        "the conflicting create must never land a second row"
    );
}

/// `#94`: a collection declares `id_type: text` over a table whose physical
/// pk column is `bigint`, not `text`/`character varying` —
/// `validate_id_type_for_create` refuses this by name, before the `INSERT`
/// is ever built, the `Text` counterpart of `create_fails_named_when_the_
/// declared_id_type_does_not_match_the_physical_pk_column`.
#[tokio::test]
async fn create_fails_named_when_the_declared_text_id_type_does_not_match_the_physical_pk_column() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping create_fails_named_when_the_declared_text_id_type_does_not_match_the_physical_pk_column: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    // `#272`: shortened from `..._create_text_id_type_mismatch` (62 bytes).
    // `seed_outbox_table` below derives `{table}_outbox` from this, which was
    // 69 bytes — past PostgreSQL's 63-byte limit, so it was being silently
    // TRUNCATED rather than rejected, and this fixture's outbox shared a
    // physical name with anything else that agreed for 63 bytes. Fixture
    // names must leave room for the companions the tests derive from them;
    // `test_harness::apply_fixture_ddl` now refuses by name if one does not.
    let table = "tellurion_postgis_write_live_test_create_text_mismatch";
    // A perfectly ordinary integer-pk table — `collection_text` below is
    // what makes this a mismatch, not the table itself.
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection_text(table);

    let feature = json!({"type": "Feature", "id": "acme-1", "geometry": null, "properties": {}});
    match sink.create(&collection, feature).await {
        Err(CoreError::Config(message)) => {
            assert!(message.contains("id_type"), "message was: {message}");
        }
        other => panic!("expected a named Config error, got {}", other.is_ok()),
    }

    let raw = connect(&database_url).await;
    let count: i64 = raw
        .query_one(&format!("SELECT count(*) FROM {table}"), &[])
        .await
        .expect("count succeeds")
        .get(0);
    assert_eq!(count, 0, "a refused create must never land a partial row");
}

/// A property naming no real column — free-form or declared-open — fails
/// with the named `UnwritableProperty` error rather than a raw, unnamed SQL
/// error or a silently dropped value.
#[tokio::test]
async fn writing_a_property_with_no_matching_column_fails_named() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping writing_a_property_with_no_matching_column_fails_named: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_unwritable";
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection(table);

    let feature = json!({
        "type": "Feature",
        "geometry": null,
        "properties": {"no_such_column": "x"}
    });
    match sink
        .apply(
            &collection,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(feature),
            },
        )
        .await
    {
        Err(tellurion_core::Error::Invalid(message)) => {
            assert!(message.contains("no_such_column"), "message was: {message}");
        }
        other => panic!("expected Err(Invalid(_)), got {}", other.is_ok()),
    }

    let raw = connect(&database_url).await;
    let count: i64 = raw
        .query_one(&format!("SELECT count(*) FROM {table}"), &[])
        .await
        .expect("count succeeds")
        .get(0);
    assert_eq!(count, 0, "a rejected write must never land a partial row");
}

// -- `Content-Crs` on write (OGC API Features Part 4, `/req/features/
// content-crs-header`, `/req/features/crs-other-crs`) -----------------------

/// `apply_with_crs(..., RequestedCrs::Storage)` against a collection whose
/// storage SRID is NOT 4326 tags the inserted geometry with that collection's
/// own SRID and stores the coordinates untransformed — the concrete, live-DB
/// proof of the fix: before `write_sql::input_geom_expr` existed, `apply`
/// always tagged SRID 4326 (`upsert_and_replace_land_the_row_and_an_outbox_
/// obligation` above proves that path is untouched), which a column typed
/// `geometry(Point, 3857)` would reject outright rather than silently accept
/// under the wrong CRS.
#[tokio::test]
async fn apply_with_crs_storage_tags_the_non_4326_storage_srid_and_preserves_coordinates() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping apply_with_crs_storage_tags_the_non_4326_storage_srid_and_preserves_coordinates: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_crs_apply";
    seed_srid_data_table(&database_url, table, 3857).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    assert!(
        sink.crs_capable(),
        "PostGIS must answer WriteSink::crs_capable() true"
    );
    let collection = collection_with_srid(table, 3857);

    // Arbitrary Web Mercator meters — not a real place, just a value with no
    // resemblance to a valid CRS84 lon/lat pair, so a test that silently
    // fell back to interpreting these as degrees would fail loudly (either
    // a PostGIS "SRID mismatch" error inserting into a 3857-typed column
    // under a wrongly-tagged SRID 4326, or wildly wrong coordinates read
    // back) rather than passing by coincidence.
    let feature = json!({
        "type": "Feature",
        "geometry": {"type": "Point", "coordinates": [1_113_194.9, 6_800_125.4]},
        "properties": {"name": "acme", "population": 1, "active": true}
    });
    sink.apply_with_crs(
        &collection,
        Mutation {
            feature_id: "1".to_string(),
            kind: MutationKind::Upsert(feature),
        },
        RequestedCrs::Storage,
    )
    .await
    .expect("apply_with_crs against a crs_capable sink succeeds");

    let raw = connect(&database_url).await;
    let row = raw
        .query_one(
            &format!("SELECT ST_SRID(geom), ST_X(geom), ST_Y(geom) FROM {table} WHERE id = 1"),
            &[],
        )
        .await
        .expect("the row exists");
    let srid: i32 = row.get(0);
    let x: f64 = row.get(1);
    let y: f64 = row.get(2);
    assert_eq!(
        srid, 3857,
        "the stored geometry must carry the declared storage SRID, not 4326"
    );
    assert!((x - 1_113_194.9).abs() < 0.01, "x was: {x}");
    assert!((y - 6_800_125.4).abs() < 0.01, "y was: {y}");
}

/// `create_with_crs(..., RequestedCrs::Storage)`'s counterpart of the
/// `apply_with_crs` proof above, for the server-assigned-id create path
/// (`#88`).
#[tokio::test]
async fn create_with_crs_storage_tags_the_non_4326_storage_srid_and_preserves_coordinates() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping create_with_crs_storage_tags_the_non_4326_storage_srid_and_preserves_coordinates: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_crs_create";
    seed_srid_data_table(&database_url, table, 3857).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection_with_srid(table, 3857);

    let feature = json!({
        "type": "Feature",
        "geometry": {"type": "Point", "coordinates": [1_113_194.9, 6_800_125.4]},
        "properties": {"name": "acme", "population": 1, "active": true}
    });
    let (new_id, _sequence) = sink
        .create_with_crs(&collection, feature, RequestedCrs::Storage)
        .await
        .expect("create_with_crs against a crs_capable sink succeeds");

    let raw = connect(&database_url).await;
    let row = raw
        .query_one(
            &format!(
                "SELECT ST_SRID(geom), ST_X(geom), ST_Y(geom) FROM {table} WHERE id = {new_id}"
            ),
            &[],
        )
        .await
        .expect("the row exists at the minted id");
    let srid: i32 = row.get(0);
    let x: f64 = row.get(1);
    let y: f64 = row.get(2);
    assert_eq!(
        srid, 3857,
        "the stored geometry must carry the declared storage SRID, not 4326"
    );
    assert!((x - 1_113_194.9).abs() < 0.01, "x was: {x}");
    assert!((y - 6_800_125.4).abs() < 0.01, "y was: {y}");
}

// -- `#116`: the default write path (`Content-Crs` absent) must transform,
// not tag ----------------------------------------------------------------
//
// `apply`/`create` (never `apply_with_crs`/`create_with_crs`) resolve to
// `RequestedCrs::Omitted` — the path a client hits by simply never sending
// a `Content-Crs` header. Before this fix, `write_sql::input_geom_expr`
// tagged every such write's geometry SRID 4326 unconditionally: a table
// typed `geometry(Point, 3857)` (this file's own `seed_srid_data_table`)
// would have rejected that outright with a PostGIS "SRID mismatch" error,
// which is exactly why the pre-fix test suite never caught this — every
// live write test before this section ran against a 4326-typed table.
//
// A live latitude-first-authority case (e.g. a second recognized
// lat/lon-ordered EPSG code) to exercise the `ST_FlipCoordinates` branch
// `write_sql::input_geom_expr`'s default-path arm carries is deliberately
// NOT covered here: `tellurion_core::crs::is_lat_lon_order` only ever
// recognizes SRID 4326 as latitude-before-longitude, and SRID 4326 is
// exactly the storage SRID the arm short-circuits away from a transform
// (that byte-for-byte-unchanged case is pinned at the unit-test level in
// `write_sql`'s own tests, not here). No live SRID this workspace can
// reach today ever takes that branch, so a live test claiming to exercise
// it would be theater, not proof.

/// `apply(..., RequestedCrs::Omitted)` against a collection whose storage
/// SRID is NOT 4326 reprojects the CRS84 request body into that storage
/// SRID rather than tagging it 4326 — the concrete, live-DB proof of the
/// `#116` fix for the replace/`PUT` path. Proven two ways: the stored row's
/// own SRID is the collection's storage SRID, and transforming it back to
/// CRS84 (via PostGIS's own `ST_Transform`, exactly the primitive the write
/// path itself uses) round-trips to the original CRS84 input — not merely
/// "some SRID was recorded," but "the coordinates really did move."
#[tokio::test]
async fn apply_default_path_transforms_crs84_input_into_a_non_4326_storage_srid() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping apply_default_path_transforms_crs84_input_into_a_non_4326_storage_srid: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_default_crs_apply";
    seed_srid_data_table(&database_url, table, 3857).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection_with_srid(table, 3857);

    // Rome, in CRS84 (longitude, latitude) — nowhere near a valid Web
    // Mercator meters pair, so a test that silently fell back to tagging
    // rather than transforming would fail loudly (a stored SRID of 3857
    // with these raw degree values as if they were meters) rather than
    // passing by coincidence.
    let (lon, lat) = (12.4964, 41.9028);
    let feature = json!({
        "type": "Feature",
        "geometry": {"type": "Point", "coordinates": [lon, lat]},
        "properties": {"name": "acme", "population": 1, "active": true}
    });
    sink.apply(
        &collection,
        Mutation {
            feature_id: "1".to_string(),
            kind: MutationKind::Upsert(feature),
        },
    )
    .await
    .expect("apply against the default (Content-Crs absent) path succeeds");

    let raw = connect(&database_url).await;
    let row = raw
        .query_one(
            &format!("SELECT ST_SRID(geom), ST_X(geom), ST_Y(geom) FROM {table} WHERE id = 1"),
            &[],
        )
        .await
        .expect("the row exists");
    let srid: i32 = row.get(0);
    let x: f64 = row.get(1);
    let y: f64 = row.get(2);
    assert_eq!(
        srid, 3857,
        "the stored geometry must carry the collection's own storage SRID, not 4326"
    );
    assert!(
        (x - lon).abs() > 1.0 && (y - lat).abs() > 1.0,
        "x={x}, y={y} look untransformed (still close to the raw CRS84 degrees)"
    );

    let round_trip = raw
        .query_one(
            &format!(
                "SELECT ST_X(ST_Transform(geom, 4326)), ST_Y(ST_Transform(geom, 4326)) FROM {table} WHERE id = 1"
            ),
            &[],
        )
        .await
        .expect("round-trip transform succeeds");
    let round_trip_lon: f64 = round_trip.get(0);
    let round_trip_lat: f64 = round_trip.get(1);
    assert!(
        (round_trip_lon - lon).abs() < 1e-6,
        "round-tripped lon was: {round_trip_lon}"
    );
    assert!(
        (round_trip_lat - lat).abs() < 1e-6,
        "round-tripped lat was: {round_trip_lat}"
    );
}

/// `create(..., RequestedCrs::Omitted)`'s counterpart of the `apply` proof
/// above, for the server-assigned-id create/`POST` path (`#88`).
#[tokio::test]
async fn create_default_path_transforms_crs84_input_into_a_non_4326_storage_srid() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping create_default_path_transforms_crs84_input_into_a_non_4326_storage_srid: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_default_crs_create";
    seed_srid_data_table(&database_url, table, 3857).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection_with_srid(table, 3857);

    let (lon, lat) = (12.4964, 41.9028);
    let feature = json!({
        "type": "Feature",
        "geometry": {"type": "Point", "coordinates": [lon, lat]},
        "properties": {"name": "acme", "population": 1, "active": true}
    });
    let (new_id, _sequence) = sink
        .create(&collection, feature)
        .await
        .expect("create against the default (Content-Crs absent) path succeeds");

    let raw = connect(&database_url).await;
    let row = raw
        .query_one(
            &format!(
                "SELECT ST_SRID(geom), ST_X(geom), ST_Y(geom) FROM {table} WHERE id = {new_id}"
            ),
            &[],
        )
        .await
        .expect("the row exists at the minted id");
    let srid: i32 = row.get(0);
    let x: f64 = row.get(1);
    let y: f64 = row.get(2);
    assert_eq!(
        srid, 3857,
        "the stored geometry must carry the collection's own storage SRID, not 4326"
    );
    assert!(
        (x - lon).abs() > 1.0 && (y - lat).abs() > 1.0,
        "x={x}, y={y} look untransformed (still close to the raw CRS84 degrees)"
    );

    let round_trip = raw
        .query_one(
            &format!(
                "SELECT ST_X(ST_Transform(geom, 4326)), ST_Y(ST_Transform(geom, 4326)) FROM {table} WHERE id = {new_id}"
            ),
            &[],
        )
        .await
        .expect("round-trip transform succeeds");
    let round_trip_lon: f64 = round_trip.get(0);
    let round_trip_lat: f64 = round_trip.get(1);
    assert!(
        (round_trip_lon - lon).abs() < 1e-6,
        "round-tripped lon was: {round_trip_lon}"
    );
    assert!(
        (round_trip_lat - lat).abs() < 1e-6,
        "round-tripped lat was: {round_trip_lat}"
    );
}

/// The `#116` byte-for-byte guarantee, proven live: a collection genuinely
/// stored in 4326 keeps the pre-fix tag-only behavior on the default path —
/// no `ST_Transform` ever runs against it, so this is really the same query
/// `upsert_and_replace_land_the_row_and_an_outbox_obligation` already
/// proves, restated here to sit next to its non-4326 counterparts above and
/// make the contrast explicit.
#[tokio::test]
async fn apply_default_path_still_tags_when_storage_srid_is_already_4326() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping apply_default_path_still_tags_when_storage_srid_is_already_4326: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_write_live_test_default_crs_4326";
    seed_srid_data_table(&database_url, table, 4326).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let sink = driver.write_sink().expect("driver exposes WriteSink");
    let collection = collection_with_srid(table, 4326);

    let (lon, lat) = (12.4964, 41.9028);
    let feature = json!({
        "type": "Feature",
        "geometry": {"type": "Point", "coordinates": [lon, lat]},
        "properties": {"name": "acme", "population": 1, "active": true}
    });
    sink.apply(
        &collection,
        Mutation {
            feature_id: "1".to_string(),
            kind: MutationKind::Upsert(feature),
        },
    )
    .await
    .expect("apply against a 4326-stored collection succeeds");

    let raw = connect(&database_url).await;
    let row = raw
        .query_one(
            &format!("SELECT ST_SRID(geom), ST_X(geom), ST_Y(geom) FROM {table} WHERE id = 1"),
            &[],
        )
        .await
        .expect("the row exists");
    let srid: i32 = row.get(0);
    let x: f64 = row.get(1);
    let y: f64 = row.get(2);
    assert_eq!(srid, 4326);
    assert!((x - lon).abs() < 1e-9, "x was: {x}");
    assert!((y - lat).abs() < 1e-9, "y was: {y}");
}
