//! End-to-end, live-PostGIS proof of OGC API Features — Part 4 (20-002r1
//! draft) Optimistic Locking (`#107`): both requirement classes,
//! `req/optimistic-locking-etags` and `req/optimistic-locking-timestamps`,
//! driven against the real compiled `tellurion` binary exactly the way
//! `binary.rs` proves every other end-to-end feature — spawned on an
//! OS-assigned port, driven over plain HTTP, no test-only shortcuts.
//!
//! Skipped cleanly (not failed) unless `TELLURION_TEST_DATABASE_URL` is
//! set — every other live-database test in this workspace outside this
//! crate's own `*_binary.rs` family gates on that same variable; this file
//! reads it and forwards it to the spawned binary's own `DATABASE_URL` (its
//! config's `url_env`), rather than requiring the caller to export both.
//!
//! The centerpiece is [`real_binary_interleaved_writers_the_loser_gets_412`]:
//! two writers both read a feature's current state, one of them commits a
//! change, and the other — now holding a stale `If-Match` — is refused with
//! `412`. That is the entire point of these two classes: without them, two
//! clients can silently overwrite each other with no way to detect it.
//!
//! [`real_binary_a_writer_blocked_between_check_and_apply_is_refused`] is
//! the `#150` counterpart, and it is a strictly harder ordering: there, the
//! losing writer's precondition is evaluated BEFORE the winner commits, so
//! the guard has already said yes by the time the conflict exists. See that
//! test's own doc for how the window is held open deliberately rather than
//! raced for.

#![cfg(feature = "postgis")]

mod common;

use std::process::Command;

use tokio_postgres::NoTls;

use common::{http_get, http_request_with_headers, http_write_request, spawn_server};

async fn connect(database_url: &str) -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .expect("connects to the test database");
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

/// A writable data table with a real `updated_at timestamptz` column (the
/// Timestamps class's own declared source) plus its outbox — the fixture
/// every test in this file seeds against. No rows: each test writes its own
/// through the real endpoints, the same convention `binary.rs`'s own
/// `seed_write_table` follows.
async fn seed_locking_table(database_url: &str, table: &str) {
    let client = connect(database_url).await;
    client
        .batch_execute(&format!(
            "DROP TABLE IF EXISTS {table};
             DROP TABLE IF EXISTS {table}_outbox;
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326),
                 name text,
                 updated_at timestamptz
             );
             CREATE TABLE {table}_outbox (
                 sequence bigserial PRIMARY KEY,
                 feature_id text NOT NULL,
                 kind text NOT NULL CHECK (kind IN ('upsert', 'delete')),
                 payload jsonb,
                 committed_at timestamptz NOT NULL DEFAULT now(),
                 extent_crs84 jsonb
             );"
        ))
        .await
        .expect("seeds the locking-test data table and its outbox");
}

/// Declares `modified_column: updated_at` (Optimistic Locking, Timestamps)
/// alongside `routing: { write: main }` (so both `If-Match`/`If-Unmodified-
/// Since` guards have a real write lane to protect) — the one config every
/// test in this file shares.
fn write_locking_config(table: &str) -> std::path::PathBuf {
    let mut path = common::unique_temp_path("tellurion-server-locking-binary-test");
    path.set_extension("yaml");
    let yaml = format!(
        r#"
server:
  port: 8080
  request_timeout_s: 30
  log_json: true
cache:
  memory_percent: 10.0
storages:
  - id: main
    driver: postgis
    url_env: DATABASE_URL
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: demo
    catalog: default
    storage: main
    table: {table}
    geometry: geom
    pk: id
    modified_column: updated_at
    routing: {{ write: main }}
"#
    );
    std::fs::write(&path, common::legacy_config(&yaml)).expect("writes the throwaway config");
    path
}

fn feature_body(name: &str, updated_at: &str) -> String {
    format!(
        r#"{{"type":"Feature","geometry":{{"type":"Point","coordinates":[10.0,45.0]}},"properties":{{"name":"{name}","updated_at":"{updated_at}"}}}}"#
    )
}

/// Blocks until the server's own backend is genuinely waiting on a row lock
/// held by this test — the seam that makes
/// [`real_binary_a_writer_blocked_between_check_and_apply_is_refused`]
/// deterministic instead of timing-dependent.
///
/// `pg_stat_activity` is asked about the exact table under test (every test
/// in this file owns a uniquely named one), so this can never be satisfied
/// by an unrelated backend blocked somewhere else in the same database. A
/// sleep here would prove nothing: too short and the window is not open yet
/// (the assertion would pass for the wrong reason), too long and the test is
/// slow AND still not guaranteed.
async fn wait_until_the_servers_write_is_blocked(client: &tokio_postgres::Client, table: &str) {
    const CEILING: std::time::Duration = std::time::Duration::from_secs(10);
    const POLL: std::time::Duration = std::time::Duration::from_millis(20);
    let deadline = std::time::Instant::now() + CEILING;
    let pattern = format!("%{table}%");
    loop {
        let row = client
            .query_one(
                "SELECT count(*) FROM pg_stat_activity \
                 WHERE wait_event_type = 'Lock' AND state = 'active' AND query LIKE $1",
                &[&pattern],
            )
            .await
            .expect("reads pg_stat_activity");
        let blocked: i64 = row.get(0);
        if blocked > 0 {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the server's write never blocked on the held row lock for '{table}'; \
             the check-to-apply window this test opens on purpose did not open"
        );
        tokio::time::sleep(POLL).await;
    }
}

/// Skips (not fails) when `TELLURION_TEST_DATABASE_URL` isn't set, matching
/// every other live-database test in this workspace. Returns the value so
/// callers can forward it to the spawned binary's own `DATABASE_URL`.
fn require_test_database_url(test_name: &str) -> Option<String> {
    match std::env::var("TELLURION_TEST_DATABASE_URL") {
        Ok(value) => Some(value),
        Err(_) => {
            eprintln!("skipping {test_name}: TELLURION_TEST_DATABASE_URL not set");
            None
        }
    }
}

#[test]
fn real_binary_etag_is_stable_then_changes_after_a_real_write() {
    let Some(database_url) =
        require_test_database_url("real_binary_etag_is_stable_then_changes_after_a_real_write")
    else {
        return;
    };
    let table = "tellurion_locking_binary_test_etag";
    let runtime = tokio::runtime::Runtime::new().expect("builds a runtime for seeding");
    runtime.block_on(seed_locking_table(&database_url, table));

    let config_path = write_locking_config(table);
    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env("DATABASE_URL", &database_url)
        .env("TELLURION_CONFIG", &config_path)
        .env("PORT", "0");
    let (process, addr, _stderr_log) = spawn_server(command);

    let path = "/public/features/catalogs/default/collections/demo/items/1";

    let create = http_write_request(
        &addr,
        "PUT",
        path,
        feature_body("alpha", "2024-06-01T00:00:00Z").as_bytes(),
    );
    assert_eq!(create.status, 204, "the initial PUT should create the item");

    let first = http_get(&addr, path);
    assert_eq!(first.status, 200);
    let first_etag = first.etag.clone().expect("first GET carries an ETag");

    let second = http_get(&addr, path);
    assert_eq!(second.status, 200);
    let second_etag = second.etag.clone().expect("second GET carries an ETag");
    assert_eq!(
        first_etag, second_etag,
        "no write landed between the two reads; the ETag must be stable"
    );

    let update = http_write_request(
        &addr,
        "PUT",
        path,
        feature_body("beta", "2024-06-02T00:00:00Z").as_bytes(),
    );
    assert_eq!(update.status, 204, "the replacing PUT should succeed");

    let third = http_get(&addr, path);
    assert_eq!(third.status, 200);
    let third_etag = third.etag.expect("third GET carries an ETag");
    assert_ne!(
        third_etag, second_etag,
        "a real write that changed the stored geometry/properties must change the ETag"
    );

    drop(process);
    let _ = std::fs::remove_file(config_path);
}

/// The concurrency proof this whole slice exists for: two writers observe
/// the SAME initial state (both `GET` before either writes — the
/// "interleaving"), one commits, and the other's now-stale `If-Match` is
/// refused with `412`. Proves the guard actually protects against a real,
/// live lost-update race, not merely that the header comparison function is
/// correct in isolation (`tellurion_core::locking`'s own unit tests already
/// cover that).
#[test]
fn real_binary_interleaved_writers_the_loser_gets_412() {
    let Some(database_url) =
        require_test_database_url("real_binary_interleaved_writers_the_loser_gets_412")
    else {
        return;
    };
    let table = "tellurion_locking_binary_test_interleaved";
    let runtime = tokio::runtime::Runtime::new().expect("builds a runtime for seeding");
    runtime.block_on(seed_locking_table(&database_url, table));

    let config_path = write_locking_config(table);
    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env("DATABASE_URL", &database_url)
        .env("TELLURION_CONFIG", &config_path)
        .env("PORT", "0");
    let (process, addr, _stderr_log) = spawn_server(command);

    let path = "/public/features/catalogs/default/collections/demo/items/1";

    // Seed the row both writers will race over.
    let create = http_write_request(
        &addr,
        "PUT",
        path,
        feature_body("initial", "2024-06-01T00:00:00Z").as_bytes(),
    );
    assert_eq!(create.status, 204);

    // Writer A and writer B both read the SAME current state — neither has
    // written anything yet, so both legitimately hold the same ETag.
    let read_by_a = http_get(&addr, path);
    let etag_a = read_by_a
        .etag
        .clone()
        .expect("writer A's GET carries an ETag");
    let read_by_b = http_get(&addr, path);
    let etag_b = read_by_b
        .etag
        .clone()
        .expect("writer B's GET carries an ETag");
    assert_eq!(
        etag_a, etag_b,
        "both writers must observe the identical pre-write state"
    );

    // Writer A commits first — this is the write that makes B's copy stale.
    let write_a = http_request_with_headers(
        &addr,
        "PUT",
        path,
        feature_body("from-writer-a", "2024-06-02T00:00:00Z").as_bytes(),
        &[("If-Match", etag_a.as_str())],
    );
    assert_eq!(
        write_a.status, 204,
        "writer A's write, against the current ETag, must succeed"
    );

    // Writer B, still holding the ETag from before A's write, now loses.
    let write_b = http_request_with_headers(
        &addr,
        "PUT",
        path,
        feature_body("from-writer-b", "2024-06-03T00:00:00Z").as_bytes(),
        &[("If-Match", etag_b.as_str())],
    );
    assert_eq!(
        write_b.status, 412,
        "writer B's write, against a now-stale ETag, must be refused"
    );

    // The stored state reflects writer A's change only — B's write never
    // landed, silently or otherwise.
    let final_state = http_get(&addr, path);
    assert_eq!(final_state.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&final_state.body).unwrap();
    assert_eq!(
        body["properties"]["name"], "from-writer-a",
        "the loser's write must never have landed: {body}"
    );

    drop(process);
    let _ = std::fs::remove_file(config_path);
}

/// `#150` — the tight interleaving the test above CANNOT reach.
///
/// `real_binary_interleaved_writers_the_loser_gets_412` proves
/// GET / GET / write-A / write-B: by the time B's precondition is evaluated,
/// A has already committed, so a guard evaluated anywhere at all catches it.
/// This test proves check-A / check-B / apply-A / apply-B — BOTH
/// preconditions evaluated before EITHER write lands — which a guard
/// evaluated in Rust before the write transaction opens cannot catch, because
/// by then it has already said yes.
///
/// The window is opened deliberately rather than raced for. Writer A is this
/// test itself, holding an open transaction that has updated the row but not
/// committed. Writer B is a real `PUT` through the real binary: its
/// precondition read sees A's uncommitted change as absent (MVCC), passes,
/// and its write then blocks on A's row lock — the check-to-apply window,
/// held open and observable. Only once the server is provably blocked
/// (`wait_until_the_servers_write_is_blocked`) does A commit, and B's write
/// resumes against a row it never checked.
///
/// With the precondition evaluated inside the write statement, B's `UPDATE`
/// re-evaluates its `xmin` predicate against the row A just committed,
/// matches nothing, and the request is refused. With the precondition
/// evaluated only in Rust beforehand, B's upsert simply applies and A's
/// change is gone — a lost update that no client could detect.
#[test]
fn real_binary_a_writer_blocked_between_check_and_apply_is_refused() {
    let Some(database_url) = require_test_database_url(
        "real_binary_a_writer_blocked_between_check_and_apply_is_refused",
    ) else {
        return;
    };
    let table = "tellurion_locking_binary_test_check_to_apply";
    let runtime = tokio::runtime::Runtime::new().expect("builds a runtime for seeding");
    runtime.block_on(seed_locking_table(&database_url, table));

    let config_path = write_locking_config(table);
    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env("DATABASE_URL", &database_url)
        .env("TELLURION_CONFIG", &config_path)
        .env("PORT", "0");
    let (process, addr, _stderr_log) = spawn_server(command);

    let path = "/public/features/catalogs/default/collections/demo/items/1";
    let create = http_write_request(
        &addr,
        "PUT",
        path,
        feature_body("initial", "2024-06-01T00:00:00Z").as_bytes(),
    );
    assert_eq!(create.status, 204);

    // Both writers observe the same state. B holds this ETag; A is about to
    // invalidate it.
    let etag = http_get(&addr, path).etag.expect("GET carries an ETag");

    // Writer A: a real, uncommitted change to the same row. Nothing else in
    // this test may commit until we say so.
    let holder = runtime.block_on(connect(&database_url));
    runtime.block_on(async {
        holder
            .batch_execute("BEGIN")
            .await
            .expect("writer A opens its transaction");
        holder
            .execute(
                &format!(
                    "UPDATE {table} SET name = 'from-writer-a', \
                     updated_at = '2024-06-02T00:00:00Z' WHERE id = 1"
                ),
                &[],
            )
            .await
            .expect("writer A updates the contested row");
    });

    // Writer B: a real request through the real binary. Its precondition
    // passes (A has not committed), then its write blocks.
    let addr_b = addr.clone();
    let etag_b = etag.clone();
    let writer_b = std::thread::spawn(move || {
        http_request_with_headers(
            &addr_b,
            "PUT",
            "/public/features/catalogs/default/collections/demo/items/1",
            feature_body("from-writer-b", "2024-06-03T00:00:00Z").as_bytes(),
            &[("If-Match", etag_b.as_str())],
        )
    });

    let observer = runtime.block_on(connect(&database_url));
    runtime.block_on(wait_until_the_servers_write_is_blocked(&observer, table));

    // The window has been open across B's whole check. Now A commits, and
    // B's already-approved write resumes against a row that has changed
    // underneath it.
    runtime.block_on(async {
        holder
            .batch_execute("COMMIT")
            .await
            .expect("writer A commits");
    });

    let response = writer_b.join().expect("writer B's request thread");
    assert_eq!(
        response.status, 412,
        "writer B's precondition was invalidated between its check and its \
         apply; the write must be refused, not applied"
    );

    let final_state = http_get(&addr, path);
    assert_eq!(final_state.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&final_state.body).unwrap();
    assert_eq!(
        body["properties"]["name"], "from-writer-a",
        "writer A's committed change must survive: {body}"
    );

    // The refusal rolled the whole transaction back, so B never committed an
    // outbox obligation either — a change feed must not report a write that
    // did not happen.
    let obligations: i64 = runtime.block_on(async {
        observer
            .query_one(
                &format!(
                    "SELECT count(*) FROM {table}_outbox \
                     WHERE payload->'properties'->>'name' = 'from-writer-b'"
                ),
                &[],
            )
            .await
            .expect("counts writer B's outbox obligations")
            .get(0)
    });
    assert_eq!(
        obligations, 0,
        "a refused conditional write must leave no outbox obligation behind"
    );

    drop(process);
    let _ = std::fs::remove_file(config_path);
}

#[test]
fn real_binary_if_unmodified_since_guards_a_stale_write() {
    let Some(database_url) =
        require_test_database_url("real_binary_if_unmodified_since_guards_a_stale_write")
    else {
        return;
    };
    let table = "tellurion_locking_binary_test_timestamps";
    let runtime = tokio::runtime::Runtime::new().expect("builds a runtime for seeding");
    runtime.block_on(seed_locking_table(&database_url, table));

    let config_path = write_locking_config(table);
    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env("DATABASE_URL", &database_url)
        .env("TELLURION_CONFIG", &config_path)
        .env("PORT", "0");
    let (process, addr, _stderr_log) = spawn_server(command);

    let path = "/public/features/catalogs/default/collections/demo/items/1";

    let create = http_write_request(
        &addr,
        "PUT",
        path,
        feature_body("initial", "2024-06-01T00:00:00Z").as_bytes(),
    );
    assert_eq!(create.status, 204);

    let read = http_get(&addr, path);
    let last_modified = read
        .last_modified
        .expect("a collection with a declared modified_column must carry Last-Modified");
    assert_eq!(last_modified, "Sat, 01 Jun 2024 00:00:00 GMT");

    // Stale: the resource was modified (at 2024-06-01) after this
    // caller's own snapshot time (2024-03-01) — refused.
    let stale_write = http_request_with_headers(
        &addr,
        "PUT",
        path,
        feature_body("attempted-stale-update", "2024-06-04T00:00:00Z").as_bytes(),
        &[("If-Unmodified-Since", "Fri, 01 Mar 2024 00:00:00 GMT")],
    );
    assert_eq!(stale_write.status, 412);

    // Current: `If-Unmodified-Since` exactly at the resource's own
    // `Last-Modified` — the resource has not changed AFTER that instant, so
    // the write proceeds.
    let current_write = http_request_with_headers(
        &addr,
        "PUT",
        path,
        feature_body("accepted-current-update", "2024-06-05T00:00:00Z").as_bytes(),
        &[("If-Unmodified-Since", last_modified.as_str())],
    );
    assert_eq!(current_write.status, 204);

    drop(process);
    let _ = std::fs::remove_file(config_path);
}

#[test]
fn real_binary_delete_with_a_stale_if_match_is_412() {
    let Some(database_url) =
        require_test_database_url("real_binary_delete_with_a_stale_if_match_is_412")
    else {
        return;
    };
    let table = "tellurion_locking_binary_test_delete";
    let runtime = tokio::runtime::Runtime::new().expect("builds a runtime for seeding");
    runtime.block_on(seed_locking_table(&database_url, table));

    let config_path = write_locking_config(table);
    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env("DATABASE_URL", &database_url)
        .env("TELLURION_CONFIG", &config_path)
        .env("PORT", "0");
    let (process, addr, _stderr_log) = spawn_server(command);

    let path = "/public/features/catalogs/default/collections/demo/items/1";
    let create = http_write_request(
        &addr,
        "PUT",
        path,
        feature_body("initial", "2024-06-01T00:00:00Z").as_bytes(),
    );
    assert_eq!(create.status, 204);

    let stale_delete =
        http_request_with_headers(&addr, "DELETE", path, &[], &[("If-Match", "\"stale\"")]);
    assert_eq!(
        stale_delete.status, 412,
        "a stale If-Match must refuse the DELETE"
    );

    let still_present = http_get(&addr, path);
    assert_eq!(
        still_present.status, 200,
        "the item must still exist after the refused DELETE"
    );

    let etag = still_present.etag.expect("GET carries an ETag");
    let real_delete =
        http_request_with_headers(&addr, "DELETE", path, &[], &[("If-Match", etag.as_str())]);
    assert_eq!(
        real_delete.status, 204,
        "the current ETag as If-Match must let the DELETE proceed"
    );

    drop(process);
    let _ = std::fs::remove_file(config_path);
}

/// Class declaration honesty (`#107`, item 3): the real, running server's
/// `/conformance` response and this collection's own `/collections/demo`
/// representation both name the two Optimistic Locking classes — proving
/// the whole registry wiring (`WriteSink::locking_conformance_classes` on
/// the real PostGIS driver, `Router::locking_conformance_classes`'s fold,
/// `CanonicalCapabilities`/`CollectionSummary`) end to end, not just at the
/// unit-test level.
#[test]
fn real_binary_declares_both_locking_classes() {
    let Some(database_url) = require_test_database_url("real_binary_declares_both_locking_classes")
    else {
        return;
    };
    let table = "tellurion_locking_binary_test_conformance";
    let runtime = tokio::runtime::Runtime::new().expect("builds a runtime for seeding");
    runtime.block_on(seed_locking_table(&database_url, table));

    let config_path = write_locking_config(table);
    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env("DATABASE_URL", &database_url)
        .env("TELLURION_CONFIG", &config_path)
        .env("PORT", "0");
    let (process, addr, _stderr_log) = spawn_server(command);

    let conformance = http_get(&addr, "/public/features/catalogs/default/conformance");
    assert_eq!(conformance.status, 200);
    let conformance_body: serde_json::Value = serde_json::from_slice(&conformance.body).unwrap();
    let classes: Vec<&str> = conformance_body["conformsTo"]
        .as_array()
        .expect("conformsTo present")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(classes.contains(
        &"http://www.opengis.net/spec/ogcapi-features-4/1.0/req/optimistic-locking-etags"
    ));

    let collection = http_get(&addr, "/public/features/catalogs/default/collections/demo");
    assert_eq!(collection.status, 200);
    let collection_body: serde_json::Value = serde_json::from_slice(&collection.body).unwrap();
    let collection_classes: Vec<&str> = collection_body["lockingConformanceClasses"]
        .as_array()
        .expect("lockingConformanceClasses present")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(collection_classes.contains(
        &"http://www.opengis.net/spec/ogcapi-features-4/1.0/req/optimistic-locking-etags"
    ));
    assert!(collection_classes.contains(
        &"http://www.opengis.net/spec/ogcapi-features-4/1.0/req/optimistic-locking-timestamps"
    ));

    drop(process);
    let _ = std::fs::remove_file(config_path);
}
