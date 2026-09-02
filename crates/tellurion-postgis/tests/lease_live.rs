//! Live tests for the advisory-lock applier lease (`#193`, closing the
//! transactional-outbox design doc's deferred "clustered applier lease"):
//! two independently-built drivers stand in for two replicas and contend
//! for the same `LeaseKey` against a real PostgreSQL. Skipped gracefully
//! unless `TELLURION_TEST_DATABASE_URL` is set, matching every other live
//! test in this workspace.
//!
//! These are deliberately live-only, not fake-backed: mutual exclusion
//! between two sessions, and release-on-disconnect, are properties of the
//! *database*, and a fake that implements them proves only that the fake
//! was written to agree with the implementation. The unit tests in
//! `lease_sql.rs` cover the pure key derivation; `tellurion_core::applier`
//! covers the loop's leader/follower behavior against a scripted
//! coordinator. What is left — and what is here — is whether Postgres
//! actually behaves the way both of those assume.
//!
//! Note what this file does NOT do: create a table, run any DDL, or seed
//! anything for the lease itself. An advisory lock needs no storage, which
//! is the whole reason this slice adds no schema and no `tellurion-ingest`
//! provisioning step.

use std::env;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tellurion_core::{
    CollectionDecl, DriverFactory, Lease, LeaseBinding, LeaseGuard, LeaseKey, Mutation,
    MutationKind, Sequence, StorageDecl, StorageDriver, INDEX_APPLIER_CONSUMER,
};
use tellurion_postgis::test_harness;
use tellurion_postgis::PostgisDriverFactory;

const URL_ENV_VAR: &str = "TELLURION_POSTGIS_LEASE_LIVE_TEST_URL";

/// Builds one driver — one "replica". Each call produces an independent
/// driver, and every `try_acquire` opens its own session, so two of these
/// contend exactly the way two processes do.
fn build_driver(database_url: &str) -> Arc<dyn StorageDriver> {
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

fn replica(database_url: &str) -> Arc<dyn Lease> {
    build_driver(database_url)
        .lease()
        .expect("the postgis driver advertises the lease capability")
}

/// A namespace of this test *process*'s own, so a live run never contends
/// with a real deployment against the same database — nor with another
/// checkout of this repository running this same file against it (`#138`).
///
/// Per-process rather than a fixed constant because an advisory lock is
/// database-global and this file's tests assert *exclusivity*: with a
/// shared namespace, two concurrent runs of
/// `only_one_replica_leads_and_the_loser_is_told_so_without_an_error` would
/// each expect to lead the same key, and one would fail its `expect("the
/// first replica leads")` — an ordinary-looking assertion failure that says
/// nothing about the real cause. This is the one live test file whose
/// shared resource is not a table, so uniquifying a table name would not
/// have reached it.
///
/// The process id is exactly the right discriminator: two simultaneously
/// live runs on one host always have different pids, and a pid reused after
/// a run exits cannot overlap it, because Postgres drops a session advisory
/// lock when the session ends.
///
/// Base 36, no prefix, because the budget is tight and deliberate: the
/// tests below assert on the leader's `application_name` verbatim, Postgres
/// caps that at 63 bytes, and the longest label here is `tellurion lease
/// {ns}/index-applier/public/default/applier-pair` — 58 bytes plus `ns`. A
/// Linux pid is at most 5 base-36 digits (`pid_max` ≤ 2^22), so this fits
/// with nothing to spare, which is why `leader_sessions` keeps asserting
/// the length by name: a longer namespace must fail loudly there rather
/// than silently turn these into tests of `lease_sql::session_label`'s
/// truncation (which has its own unit tests) instead of leadership.
fn test_namespace() -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut pid = u64::from(std::process::id());
    if pid == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while pid > 0 {
        out.push(DIGITS[(pid % 36) as usize]);
        pid /= 36;
    }
    out.reverse();
    String::from_utf8(out).expect("base-36 digits are ASCII")
}

fn key(collection: &str) -> LeaseKey {
    LeaseKey::for_collection(
        Some(&test_namespace()),
        INDEX_APPLIER_CONSUMER,
        "public",
        "default",
        collection,
    )
}

async fn connect(database_url: &str) -> tokio_postgres::Client {
    test_harness::connect(database_url).await
}

/// How many sessions currently announce themselves as this key's leader.
/// `pg_stat_activity` is exactly where an operator looks to answer "which
/// replica leads?", so asserting on it also pins the operator-facing half
/// of the feature.
async fn leader_sessions(database_url: &str, key: &LeaseKey) -> i64 {
    let client = connect(database_url).await;
    let label = format!("tellurion lease {key}");
    assert!(
        label.len() <= 63,
        "this test asserts on the label verbatim, so it must fit application_name: {label}"
    );
    client
        .query_one(
            "SELECT count(*) FROM pg_stat_activity WHERE application_name = $1",
            &[&label],
        )
        .await
        .expect("pg_stat_activity is readable")
        .get::<_, i64>(0)
}

/// Postgres releases an advisory lock when the holder's *session* ends,
/// which it learns about asynchronously — so "the lock is free again" is
/// eventually, not instantly, true after a guard drops. Retries rather than
/// sleeping a fixed amount so the test neither flakes on a slow backend nor
/// wastes time on a fast one.
async fn acquire_within(lease: &dyn Lease, key: &LeaseKey, timeout: Duration) -> LeaseGuard {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match lease.try_acquire(key).await.expect("coordinator reachable") {
            Some(guard) => return guard,
            None if std::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            None => panic!("lease was never released within {timeout:?}"),
        }
    }
}

/// The core mutual-exclusion property, and the shape of the answer that
/// makes it usable: the loser gets `Ok(None)` — an ordinary "somebody else
/// leads" — never an error. A follower replica that logged an error every
/// poll tick would make a healthy two-replica deployment look broken.
#[tokio::test]
async fn only_one_replica_leads_and_the_loser_is_told_so_without_an_error() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping only_one_replica_leads...: TELLURION_TEST_DATABASE_URL not set");
        return;
    };
    let a = replica(&database_url);
    let b = replica(&database_url);
    let key = key("exclusion");

    let leader = a
        .try_acquire(&key)
        .await
        .expect("coordinator reachable")
        .expect("the first replica leads");
    assert!(leader.is_live());
    assert_eq!(leader.key(), &key);

    for _ in 0..3 {
        let follower = b.try_acquire(&key).await.expect("coordinator reachable");
        assert!(
            follower.is_none(),
            "a second replica must not lead while the first holds the lease"
        );
    }

    // Exactly one session announces itself as the leader: the follower's
    // losing attempts close their sessions without ever claiming the label.
    assert_eq!(leader_sessions(&database_url, &key).await, 1);
}

/// Failover, which is the whole point: dropping the guard — what happens
/// when a leader's task returns on shutdown — releases the lease, and the
/// standby takes over. No expiry to wait out, because there is no expiry:
/// the session ending IS the release.
#[tokio::test]
async fn dropping_the_guard_hands_leadership_to_the_standby() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping dropping_the_guard_hands_leadership...: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let a = replica(&database_url);
    let b = replica(&database_url);
    let key = key("failover");

    let leader = a
        .try_acquire(&key)
        .await
        .expect("coordinator reachable")
        .expect("the first replica leads");
    assert!(b
        .try_acquire(&key)
        .await
        .expect("coordinator reachable")
        .is_none());

    drop(leader);

    let promoted = acquire_within(b.as_ref(), &key, Duration::from_secs(10)).await;
    assert!(promoted.is_live());
    assert_eq!(leader_sessions(&database_url, &key).await, 1);
}

/// Leadership is per collection, not per process: two collections must be
/// leadable at once, or one collection's wedged applier would stall every
/// other collection's index behind it.
#[tokio::test]
async fn distinct_collections_do_not_contend_for_one_leadership() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping distinct_collections_do_not_contend...: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let a = replica(&database_url);
    let b = replica(&database_url);

    let first = a
        .try_acquire(&key("alpha"))
        .await
        .expect("coordinator reachable")
        .expect("leads alpha");
    let second = b
        .try_acquire(&key("beta"))
        .await
        .expect("coordinator reachable")
        .expect("a different replica leads beta at the same time");
    assert!(first.is_live() && second.is_live());
}

/// The namespace's reason to exist, proven against a real database: two
/// deployments sharing one PostgreSQL must not fight over each other's
/// leadership of identically-named collections.
#[tokio::test]
async fn a_namespace_keeps_two_deployments_on_one_database_from_contending() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping a_namespace_keeps_two_deployments...: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let staging = replica(&database_url);
    let preview = replica(&database_url);
    let collection = "shared-database";

    // Both namespaces carry this process's own discriminator for the same
    // reason `test_namespace` exists (`#138`): a fixed `lease-live-staging`
    // would be led by whichever concurrent run got there first, and the
    // other run's `expect("staging leads its own namespace")` would fail for
    // a reason that has nothing to do with namespacing. These two keys are
    // never asserted on through `application_name`, so they are free to be
    // long.
    let process = test_namespace();
    let staging_key = LeaseKey::for_collection(
        Some(&format!("lease-live-staging-{process}")),
        INDEX_APPLIER_CONSUMER,
        "public",
        "default",
        collection,
    );
    let preview_key = LeaseKey::for_collection(
        Some(&format!("lease-live-preview-{process}")),
        INDEX_APPLIER_CONSUMER,
        "public",
        "default",
        collection,
    );

    let _staging_leader = staging
        .try_acquire(&staging_key)
        .await
        .expect("coordinator reachable")
        .expect("staging leads its own namespace");
    let preview_leader = preview
        .try_acquire(&preview_key)
        .await
        .expect("coordinator reachable");
    assert!(
        preview_leader.is_some(),
        "a second deployment must lead its own namespace regardless of the first"
    );
}

// ---- end to end: two leased appliers over one real collection ----

async fn seed_tables(database_url: &str, table: &str) {
    let client = connect(database_url).await;
    // Hand-kept in sync with `tellurion-ingest::outbox`/`::index`'s own
    // `create_*_table_sql`, the same convention `index_live.rs` follows and
    // for the same reason (the two crates never depend on each other).
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
             );
             CREATE TABLE {table}_index (
                 feature_id text PRIMARY KEY,
                 version bigint NOT NULL,
                 kind text NOT NULL CHECK (kind IN ('upsert', 'delete')),
                 doc jsonb,
                 updated_at timestamptz NOT NULL DEFAULT now()
             );"
        ),
    )
    .await
    .expect("seeds the data, outbox, and index tables");
}

fn collection(table: &str) -> CollectionDecl {
    serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\n"
    ))
    .expect("valid CollectionDecl yaml")
}

/// The acceptance criterion for `#193`: two replicas both running the
/// applier for the same collection, both leased, converge the index — with
/// exactly one of them ever draining, and the other still stopping promptly
/// on shutdown rather than being a parked task.
#[tokio::test]
async fn two_leased_appliers_converge_the_index_with_exactly_one_drainer() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping two_leased_appliers_converge...: TELLURION_TEST_DATABASE_URL not set");
        return;
    };
    let table = "tellurion_postgis_lease_live_test_pair";
    seed_tables(&database_url, table).await;

    let collection = collection(table);
    let key = key("applier-pair");
    let (tx, rx) = tokio::sync::watch::channel(false);

    let writer = build_driver(&database_url);
    let write = writer.write_sink().expect("driver exposes WriteSink");
    for (id, name) in [("1", "acme"), ("2", "beta"), ("3", "gamma")] {
        write
            .apply(
                &collection,
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

    let mut handles = Vec::new();
    for _ in 0..2 {
        let driver = build_driver(&database_url);
        handles.push(tokio::spawn(tellurion_core::run_applier(
            driver.outbox_source().expect("driver exposes OutboxSource"),
            driver.index_sink().expect("driver exposes IndexSink"),
            collection.clone(),
            10,
            Duration::from_millis(20),
            Some(LeaseBinding::new(
                driver.lease().expect("driver exposes the lease capability"),
                key.clone(),
            )),
            rx.clone(),
        )));
    }

    // Converge, then assert on the leadership the convergence ran under.
    let index = build_driver(&database_url)
        .index_sink()
        .expect("driver exposes IndexSink");
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let high_water = index
            .applied_high_water(&collection)
            .await
            .expect("index high-water readable");
        if high_water == Sequence(3) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "leased appliers did not converge the index (high-water {high_water:?})"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Exactly one of the two tasks is the leader — the other has been
    // polling as a follower the whole time and claimed no session label.
    assert_eq!(leader_sessions(&database_url, &key).await, 1);

    tx.send(true).expect("shutdown signal delivered");
    for handle in handles {
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("both a leader and a follower stop promptly on shutdown")
            .expect("applier task did not panic");
    }
}
