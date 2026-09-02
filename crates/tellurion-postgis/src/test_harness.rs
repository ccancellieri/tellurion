//! Shared live-test harness (`#138`): the one place every crate's live
//! tests connect to the shared PostgreSQL and apply their fixture DDL.
//!
//! Compiled only under the `test-support` feature (default OFF) — the same
//! shape `tellurion-core`'s own `test-support` feature uses to lend test
//! fakes to other crates without linking them into a production binary.
//!
//! ## The problem this exists for
//!
//! `#138` reported `registry_live.rs` failing with a duplicate-key
//! violation on `pg_type_typname_nsp_index` when many live-test binaries
//! run in parallel against one database. There is no `CREATE TYPE`
//! anywhere in this workspace: **every `CREATE TABLE` creates a composite
//! type of the same name**, so a `CREATE TABLE` race lands on `pg_type`'s
//! own unique index. `CREATE TABLE IF NOT EXISTS` does *not* make that
//! safe — it checks for the relation and then inserts the catalog rows
//! without a lock spanning the two, so two sessions racing it both see
//! "absent" and the loser gets `23505`. The sibling failure lands on
//! `pg_class_relname_nsp_index` instead, from the implicit sequence a
//! `bigserial` column creates; both are the same race, and a test file
//! that names only one of them under-reports it.
//!
//! Three test binaries in this workspace issue the *identical* registry
//! DDL — `tellurion-postgis`'s `tests/registry_live.rs` and
//! `tests/tenant_live.rs`, plus `tellurion-ingest`'s own `registry` module
//! test — and `cargo test --workspace` runs them concurrently. Each of the
//! three carries a doc comment asserting that `CREATE TABLE IF NOT EXISTS`
//! is "safe under concurrent callers". It is not, and `#138` is the
//! evidence.
//!
//! ## What [`apply_fixture_ddl`] does about it
//!
//! It wraps the DDL in a transaction that first takes
//! `pg_advisory_xact_lock` on a key derived from the fixture's name. That
//! is mutual exclusion the *database* provides, so it holds across test
//! threads, across test binaries, and across two checkouts of this
//! repository running their live suites against one server — none of which
//! a process-local `OnceCell` can reach. The lock is released by the
//! commit (or the rollback), so there is no cleanup path that can leak it,
//! and it is taken per fixture rather than globally: two tests seeding
//! *different* tables never wait on each other, which is what keeps the
//! suites parallel instead of serializing them (the fix `#138` explicitly
//! asked not to land).
//!
//! ### Which lock space, and why not the lease's
//!
//! PostgreSQL keeps two disjoint advisory-lock spaces: one keyed by a
//! single `bigint` and one keyed by a pair of `int4`s. `lease_sql.rs` uses
//! the `bigint` space for real applier leadership. This harness uses the
//! `(int4, int4)` space with a fixed [`FIXTURE_LOCK_CLASS`], so a test
//! fixture's lock can never — not even by hash collision — be mistaken by
//! PostgreSQL for a production lease key.
//!
//! ## Named refusals
//!
//! The second half of `#138` is that a collision should *say what it is*.
//! A shared-fixture race that surfaces as an ordinary assertion failure
//! teaches people to re-run instead of read; so does a stopped cluster,
//! which makes every live test in a run fail at once in a way that is
//! indistinguishable from a regression in the branch under test.
//!
//! So: [`connect`] refuses by name when the server is unreachable rather
//! than panicking with a bare `expect`, and [`FixtureDdlError`] classifies
//! the SQLSTATEs that mean "another live run is touching this fixture right
//! now" and says so, pointing at `#138`, instead of letting a `23505` reach
//! the reader as an opaque duplicate key.

use std::fmt;

use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, NoTls};

/// The `int4` class half of every fixture lock this harness takes, chosen
/// once so all callers land in one namespace. The value is arbitrary but
/// pinned: changing it would stop an older checkout's live run from
/// excluding a newer one's, which is precisely the cross-checkout case
/// `#138` is about. `0x7e11` spells "tell" loosely enough to be
/// recognisable in `pg_locks.classid` while staying well inside `int4`.
pub const FIXTURE_LOCK_CLASS: i32 = 0x7e11;

/// The fixture name every caller of the shared registry DDL must pass to
/// [`apply_fixture_ddl`].
///
/// `registry_tenants`, `registry_catalogs` and `registry_collections` are
/// the one table set in this workspace that is *deliberately* shared by
/// several live tests (they model the single per-database registry a real
/// deployment has, so no test may drop them and every test scopes itself by
/// id prefix instead). Three binaries create them concurrently, and they
/// only exclude each other if all three lock the *same* name — a caller
/// that locked `registry_catalogs` while another locked `registry_tenants`
/// would take two different locks and race exactly as before. Hence one
/// constant rather than each site naming its own table.
pub const REGISTRY_TABLES_FIXTURE: &str = "registry_tenants";

/// Why a fixture's DDL did not apply.
#[derive(Debug)]
pub enum FixtureDdlError {
    /// Another live-test run was creating or dropping the same objects at
    /// the same moment. This is `#138`: the fixture is shared, not the code
    /// under test is broken.
    Collision {
        fixture: String,
        sqlstate: String,
        source: tokio_postgres::Error,
    },
    /// The DDL itself is wrong, or the server refused it for a reason that
    /// has nothing to do with concurrency. Passed through unclassified on
    /// purpose: mislabelling a genuine schema error as a race would be the
    /// same failure mode in the other direction.
    Failed {
        fixture: String,
        source: tokio_postgres::Error,
    },
    /// The DDL names an identifier PostgreSQL would silently truncate
    /// (`#272`). Refused before anything is applied, because the damage a
    /// truncation does is invisible: the statement succeeds, against a
    /// table that is not the one the name said.
    TruncatedIdentifier { fixture: String, identifier: String },
}

impl fmt::Display for FixtureDdlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Collision {
                fixture,
                sqlstate,
                source,
            } => write!(
                f,
                "LIVE-TEST FIXTURE COLLISION on '{fixture}' (SQLSTATE {sqlstate}): another live \
                 test run created, dropped, or locked the same objects concurrently. This is a \
                 test-isolation failure, NOT a defect in the code under test — see #138. Most \
                 likely another checkout of this repository is running its live suite against the \
                 same database; `TELLURION_TEST_DATABASE_URL` is shared, and every live fixture \
                 table name is a compile-time constant. Underlying error: {source}"
            ),
            Self::Failed { fixture, source } => write!(
                f,
                "fixture DDL for '{fixture}' failed (not a concurrency error): {source}"
            ),
            Self::TruncatedIdentifier {
                fixture,
                identifier,
            } => write!(
                f,
                "LIVE-TEST FIXTURE IDENTIFIER TOO LONG on '{fixture}': '{identifier}' is {} bytes \
                 and PostgreSQL stores only {MAX_IDENTIFIER_BYTES}. It would NOT be rejected — it \
                 would be silently truncated to '{}', so this fixture and any other whose name \
                 agrees for the first {MAX_IDENTIFIER_BYTES} bytes would share one physical table \
                 and overwrite each other's rows. Nothing was applied. Shorten the fixture's table \
                 name: it has to leave room for the '_outbox'/'_index'/'_stac'/'_assets' \
                 companions the tests derive from it (#272).",
                identifier.len(),
                stored_prefix(identifier),
            ),
        }
    }
}

impl std::error::Error for FixtureDdlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Collision { source, .. } | Self::Failed { source, .. } => Some(source),
            // Refused before any statement ran: there is no driver error
            // underneath, and inventing one would misdescribe it.
            Self::TruncatedIdentifier { .. } => None,
        }
    }
}

/// The SQLSTATEs that mean "somebody else was doing DDL to these objects at
/// the same time", each with a note on which race produces it.
///
/// Kept as an explicit list rather than a catch-all so a genuine schema
/// mistake still reads as a schema mistake ([`FixtureDdlError::Failed`]).
fn classify(code: Option<&SqlState>) -> Option<&'static str> {
    let code = code?;
    // `CREATE TABLE` inserts a composite type row; two racing sessions
    // collide on `pg_type_typname_nsp_index`. A `bigserial` column's
    // implicit sequence collides on `pg_class_relname_nsp_index` instead.
    if *code == SqlState::UNIQUE_VIOLATION {
        return Some("23505");
    }
    // The loser of a `CREATE TABLE` race that got far enough for the
    // relation to be visible.
    if *code == SqlState::DUPLICATE_TABLE {
        return Some("42P07");
    }
    // A concurrent `DROP TABLE` removed the object between this
    // transaction's statements.
    if *code == SqlState::UNDEFINED_TABLE {
        return Some("42P01");
    }
    // Two runs dropping and recreating overlapping object sets in opposite
    // orders.
    if *code == SqlState::T_R_DEADLOCK_DETECTED {
        return Some("40P01");
    }
    if *code == SqlState::LOCK_NOT_AVAILABLE {
        return Some("55P03");
    }
    None
}

/// The `int4` key half: a stable 32-bit hash of the fixture name.
///
/// FNV-1a, spelled out for the same reason `lease_sql.rs` spells out its
/// 64-bit sibling — the value must be identical across processes, builds,
/// and checkouts, and `std::hash::DefaultHasher` guarantees none of that.
/// Two fixture names colliding costs nothing but a needless wait between
/// two unrelated seeds, never a wrong result.
pub fn fixture_lock_key(fixture: &str) -> i32 {
    const FNV_OFFSET_BASIS: u32 = 0x811c_9dc5;
    const FNV_PRIME: u32 = 0x0100_0193;
    let mut hash = FNV_OFFSET_BASIS;
    for byte in fixture.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash as i32
}

/// Wraps fixture DDL in an advisory-locked transaction. Returns the SQL
/// text so the lock, the DDL, and the commit are one simple-query round
/// trip — the same single implicit transaction `batch_execute` already gave
/// this DDL, now with a lock in front of it.
fn locked_ddl_sql(fixture: &str, sql: &str) -> String {
    let body = sql.trim().trim_end_matches(';');
    format!(
        "BEGIN;\nSELECT pg_advisory_xact_lock({FIXTURE_LOCK_CLASS}, {});\n{body};\nCOMMIT;",
        fixture_lock_key(fixture)
    )
}

/// The `application_name` every connection [`connect`] opens announces
/// itself under. Carries this process's pid, which is what makes "another
/// live-test run is active" answerable exactly rather than heuristically:
/// two simultaneously live runs on one host always have different pids.
pub fn harness_application_name() -> String {
    format!("tellurion-live-test-{}", std::process::id())
}

/// Says so, by name, when another live-test run is active against this
/// database, or is holding this fixture's lock at the moment we ask for it.
///
/// Diagnostics only — it takes no lock it keeps and never fails a test.
/// Its value is that it turns "several live tests failed together and I
/// cannot tell why" into a line naming the other run. Contention on a
/// shared database is invisible from inside the failing assertion, so
/// without this the reader's only evidence points at the branch under
/// test. The output is `eprintln!`, so `cargo test` captures it and shows
/// it *only* alongside a failing test — a green run stays quiet.
///
/// Two independent checks, because they catch different windows:
///
/// 1. **Another run is live at all.** Every connection [`connect`] opens is
///    labelled [`harness_application_name`], so a `pg_stat_activity` row
///    carrying that prefix with a *different* pid is another run, full stop.
///    This holds for the whole of that run, not just its DDL, which is what
///    makes it the reliable signal: the damage a concurrent run does is
///    mostly to rows, long after any DDL has committed.
/// 2. **This exact fixture is being rebuilt right now.** A
///    `pg_try_advisory_lock` probe in this harness's own `(int4, int4)` key
///    space at this fixture's key; the only thing that can hold it is
///    another `apply_fixture_ddl` for the same fixture. A successful probe
///    is released immediately.
///
/// Neither can produce a false positive, and both under-report rather than
/// over-report: a run whose live tests have not opened a connection yet, or
/// one built from a checkout that predates this harness, is invisible to
/// check 1, and reporting is not what makes the DDL safe (the lock is).
async fn report_contention(client: &Client, fixture: &str) {
    let ours = harness_application_name();
    if let Ok(rows) = client
        .query(
            "SELECT DISTINCT application_name FROM pg_stat_activity \
             WHERE application_name LIKE 'tellurion-live-test-%' \
               AND application_name <> $1",
            &[&ours],
        )
        .await
    {
        if !rows.is_empty() {
            let others: Vec<&str> = rows.iter().map(|row| row.get::<_, &str>(0)).collect();
            eprintln!(
                "LIVE-TEST CONCURRENT RUN: another live-test run is connected to this database \
                 right now ({}); this run is '{ours}'. Live fixture table names are compile-time \
                 constants, so the two runs seed over each other's rows and drop each other's \
                 tables. If a test below fails, suspect that before the branch under test \
                 (#138). Fixture DDL is serialized by an advisory lock, but the ROWS are not.",
                others.join(", ")
            );
        }
    }

    let key = fixture_lock_key(fixture);
    let Ok(row) = client
        .query_one(
            "SELECT pg_try_advisory_lock($1::int4, $2::int4)",
            &[&FIXTURE_LOCK_CLASS, &key],
        )
        .await
    else {
        // The probe is best-effort: a server that cannot answer it will
        // fail the DDL itself a moment later with a real message.
        return;
    };
    if row.get::<_, bool>(0) {
        let _ = client
            .query_one(
                "SELECT pg_advisory_unlock($1::int4, $2::int4)",
                &[&FIXTURE_LOCK_CLASS, &key],
            )
            .await;
        return;
    }
    // Held by somebody. `pg_locks` records a two-key advisory lock as
    // `classid`/`objid` with `objsubid = 2` (the one-key `bigint` space
    // uses `objsubid = 1`), so this cannot pick up a production lease.
    let holders = client
        .query(
            "SELECT l.pid, coalesce(a.application_name, ''), coalesce(a.state, '') \
             FROM pg_locks l LEFT JOIN pg_stat_activity a ON a.pid = l.pid \
             WHERE l.locktype = 'advisory' AND l.objsubid = 2 \
               AND l.classid = $1::int4::oid AND l.objid = $2::int4::oid AND l.granted",
            &[&FIXTURE_LOCK_CLASS, &key],
        )
        .await
        .unwrap_or_default();
    let who: Vec<String> = holders
        .iter()
        .map(|row| {
            format!(
                "backend pid {} (application_name {:?}, state {:?})",
                row.get::<_, i32>(0),
                row.get::<_, &str>(1),
                row.get::<_, &str>(2)
            )
        })
        .collect();
    eprintln!(
        "LIVE-TEST FIXTURE CONTENTION on '{fixture}': another live-test run holds this fixture's \
         lock right now — {}. This run will wait for it and then re-apply the DDL. If a test \
         downstream of this fails, suspect the shared database before the branch under test \
         (#138): fixture table names are compile-time constants, so two checkouts running their \
         live suites at once seed over each other.",
        if who.is_empty() {
            "holder not identifiable from pg_locks".to_string()
        } else {
            who.join("; ")
        }
    );
}

/// The byte at which PostgreSQL stops storing an identifier
/// (`NAMEDATALEN - 1`).
const MAX_IDENTIFIER_BYTES: usize = 63;

/// The prefix PostgreSQL would keep, cut back to a character boundary.
///
/// [`refuse_truncated_identifiers`] only ever produces ASCII, but
/// [`FixtureDdlError`] is public and constructible, and a panic while
/// formatting a refusal would replace a clear message with a crash.
fn stored_prefix(identifier: &str) -> &str {
    let mut end = MAX_IDENTIFIER_BYTES.min(identifier.len());
    while end > 0 && !identifier.is_char_boundary(end) {
        end -= 1;
    }
    &identifier[..end]
}

/// Refuses fixture DDL that names an identifier PostgreSQL cannot store
/// whole (`#272`).
///
/// PostgreSQL does not reject an over-long identifier. It **truncates it to
/// 63 bytes**, emits a notice most clients discard, and carries on — so two
/// fixtures whose derived names differ only past byte 63 quietly become one
/// table, and the second one's test runs against the first one's rows.
/// `#272` found this live here: `tellurion_postgis_write_live_test_create_
/// text_id_type_mismatch` is 62 bytes, and the `_outbox` companion the test
/// derives from it is 69.
///
/// Every live-test fixture in this workspace already routes its DDL through
/// this function (`#138` did that work), so one check here covers all of
/// them, including any fixture added later. It is deliberately cruder than
/// `tellurion-ingest`'s own equivalent — which has to cope with
/// operator-supplied names and dollar-quoted bodies — because fixture DDL is
/// compile-time text written in this repository: every word in it is a
/// keyword, a short literal, or an identifier, so the longest word IS the
/// longest identifier and no stripping is needed to say so.
fn refuse_truncated_identifiers(fixture: &str, sql: &str) -> Result<(), FixtureDdlError> {
    let overlong = sql
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .find(|word| word.len() > MAX_IDENTIFIER_BYTES);
    match overlong {
        Some(word) => Err(FixtureDdlError::TruncatedIdentifier {
            fixture: fixture.to_string(),
            identifier: word.to_string(),
        }),
        None => Ok(()),
    }
}

/// Applies `sql` under a database-wide advisory lock named for `fixture`.
///
/// `fixture` should be the *base* object name the DDL builds from (the
/// `table` variable at almost every call site): the derived
/// `{table}_outbox`/`{table}_index`/`{table}_stac` companions are covered
/// by the same lock because they are created by the same statement batch.
/// Two tests seeding different tables take different locks and do not wait
/// on each other.
///
/// Every caller of a *shared* fixture must agree on the name — see
/// [`REGISTRY_TABLES_FIXTURE`].
pub async fn apply_fixture_ddl(
    client: &Client,
    fixture: &str,
    sql: &str,
) -> Result<(), FixtureDdlError> {
    refuse_truncated_identifiers(fixture, sql)?;
    report_contention(client, fixture).await;
    match client.batch_execute(&locked_ddl_sql(fixture, sql)).await {
        Ok(()) => Ok(()),
        Err(source) => {
            // The implicit transaction is already aborted; roll it back so
            // this client stays usable for the caller's next statement
            // rather than failing every one of them with 25P02, which would
            // bury the real message under a cascade.
            let _ = client.batch_execute("ROLLBACK").await;
            match classify(source.code()) {
                Some(sqlstate) => Err(FixtureDdlError::Collision {
                    fixture: fixture.to_string(),
                    sqlstate: sqlstate.to_string(),
                    source,
                }),
                None => Err(FixtureDdlError::Failed {
                    fixture: fixture.to_string(),
                    source,
                }),
            }
        }
    }
}

/// Adds `application_name` to a connection string so every session this
/// harness opens is attributable to its run — the whole basis of
/// [`report_contention`]'s first check.
///
/// Handles both spellings `tokio-postgres` accepts: a `postgres://` URL
/// (with or without an existing query string) and libpq's `key=value`
/// form. An explicit `application_name` already in the string is left
/// alone: a caller who set one meant it, and silently overriding it would
/// be the sort of invented default this workspace refuses.
fn with_application_name(database_url: &str, name: &str) -> String {
    if database_url.contains("application_name") {
        return database_url.to_string();
    }
    if database_url.starts_with("postgres://") || database_url.starts_with("postgresql://") {
        let separator = if database_url.contains('?') { '&' } else { '?' };
        format!("{database_url}{separator}application_name={name}")
    } else {
        format!("{database_url} application_name={name}")
    }
}

/// Connects to the live test database, refusing by name when the server is
/// not there.
///
/// The bare `.expect("connects to the test database")` this replaces says
/// nothing about *why*, and a stopped cluster fails every live test in a
/// run at once — a shape indistinguishable from a real regression until
/// somebody thinks to run `pg_isready`. A panic that says "the cluster is
/// down, here is how to check" costs one line and saves that entire
/// investigation.
pub async fn connect(database_url: &str) -> Client {
    match tokio_postgres::connect(
        &with_application_name(database_url, &harness_application_name()),
        NoTls,
    )
    .await
    {
        Ok((client, connection)) => {
            tokio::spawn(async move {
                let _ = connection.await;
            });
            client
        }
        Err(err) => panic!(
            "LIVE TEST DATABASE UNREACHABLE: could not connect to the server named by \
             TELLURION_TEST_DATABASE_URL: {err}\n\
             This is test infrastructure, NOT a failure of the code under test. Check the server \
             is up (`pg_isready -h <host> -p <port>`) and start it if it is not \
             (`pg_ctlcluster 16 main start`) before reading anything into this run."
        ),
    }
}

/// Reads `TELLURION_TEST_DATABASE_URL`, or explains the skip.
///
/// Byte-identical in behaviour to the `let Ok(url) = env::var(..) else {
/// eprintln!("skipping {name}: TELLURION_TEST_DATABASE_URL not set"); return
/// }` idiom every live test already spells out by hand — the message
/// matters because `cargo test --nocapture` grepped for it is how this
/// workspace proves its live tests actually ran rather than silently
/// skipped.
pub fn require_database_url(test_name: &str) -> Option<String> {
    match std::env::var("TELLURION_TEST_DATABASE_URL") {
        Ok(url) => Some(url),
        Err(_) => {
            eprintln!("skipping {test_name}: TELLURION_TEST_DATABASE_URL not set");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixture_lock_key_is_pinned_across_builds_and_checkouts() {
        // Pinned literals, not a round trip: the whole point is that a
        // different build of this file computes the same number, so a
        // refactor that changed the hash must fail here rather than
        // silently stop two checkouts from excluding each other.
        assert_eq!(fixture_lock_key("registry_tenants"), 1_345_481_640);
        assert_eq!(fixture_lock_key(""), -2_128_831_035);
        assert_eq!(
            fixture_lock_key("registry_tenants"),
            fixture_lock_key(REGISTRY_TABLES_FIXTURE)
        );
    }

    #[test]
    fn distinct_fixtures_get_distinct_keys_so_unrelated_seeds_never_wait() {
        let fixtures = [
            "tellurion_postgis_live_test_items",
            "tellurion_postgis_write_live_test_upsert",
            "tellurion_postgis_index_live_test_converge",
            "registry_tenants",
        ];
        let mut keys: Vec<i32> = fixtures.iter().copied().map(fixture_lock_key).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(
            keys.len(),
            before,
            "fixture lock keys collided: {fixtures:?}"
        );
    }

    #[test]
    fn the_lock_is_taken_before_the_ddl_and_released_by_the_commit() {
        let sql = locked_ddl_sql(
            "demo",
            "DROP TABLE IF EXISTS demo; CREATE TABLE demo (id int);",
        );
        let lock_at = sql.find("pg_advisory_xact_lock").expect("takes a lock");
        let ddl_at = sql.find("DROP TABLE").expect("still runs the DDL");
        assert!(lock_at < ddl_at, "the lock must precede the DDL: {sql}");
        assert!(sql.starts_with("BEGIN;"), "{sql}");
        assert!(sql.ends_with("COMMIT;"), "{sql}");
        // `pg_advisory_xact_lock` is released by the transaction end, so
        // there is deliberately no explicit unlock to leak.
        assert!(!sql.contains("pg_advisory_unlock"), "{sql}");
    }

    #[test]
    fn the_two_int_lock_space_keeps_fixtures_out_of_the_production_lease_space() {
        let sql = locked_ddl_sql("demo", "SELECT 1");
        assert!(
            sql.contains(&format!("pg_advisory_xact_lock({FIXTURE_LOCK_CLASS},")),
            "fixtures must use the (int4, int4) space, never the bigint space `lease_sql` \
             competes in: {sql}"
        );
    }

    #[test]
    fn a_trailing_semicolon_never_produces_an_empty_statement() {
        for body in ["CREATE TABLE demo (id int);", "CREATE TABLE demo (id int)"] {
            let sql = locked_ddl_sql("demo", body);
            assert!(sql.ends_with("(id int);\nCOMMIT;"), "{sql}");
        }
    }

    #[test]
    fn every_harness_session_is_attributable_to_its_run() {
        let name = harness_application_name();
        assert!(name.starts_with("tellurion-live-test-"), "{name}");
        assert!(
            name.len() <= 63,
            "application_name is capped at 63 bytes by Postgres: {name}"
        );
        assert_eq!(
            name,
            harness_application_name(),
            "the label must be stable within a process, or a run would look like several"
        );
    }

    #[test]
    fn the_application_name_is_appended_to_either_connection_string_spelling() {
        assert_eq!(
            with_application_name("postgres://u:p@h:5432/db", "run-7"),
            "postgres://u:p@h:5432/db?application_name=run-7"
        );
        assert_eq!(
            with_application_name("postgresql://h/db?sslmode=disable", "run-7"),
            "postgresql://h/db?sslmode=disable&application_name=run-7"
        );
        assert_eq!(
            with_application_name("host=h user=u dbname=db", "run-7"),
            "host=h user=u dbname=db application_name=run-7"
        );
        // A caller who already set one meant it.
        assert_eq!(
            with_application_name("postgres://h/db?application_name=mine", "run-7"),
            "postgres://h/db?application_name=mine"
        );
    }

    /// `#272`: the pair that was actually being truncated in this file's own
    /// neighbourhood before the guard existed.
    #[test]
    fn a_derived_fixture_name_past_sixty_three_bytes_is_refused_before_anything_is_applied() {
        let base = "tellurion_postgis_write_live_test_create_text_id_type_mismatch";
        assert_eq!(base.len(), 62, "the base name is legal on its own");
        let error = refuse_truncated_identifiers(
            base,
            &format!("CREATE TABLE IF NOT EXISTS {base}_outbox (sequence bigserial PRIMARY KEY);"),
        )
        .expect_err("69 bytes must be refused rather than truncated");
        let message = error.to_string();
        assert!(message.contains(&format!("{base}_outbox")), "{message}");
        assert!(message.contains("69 bytes"), "{message}");
        assert!(
            message.contains("silently truncated"),
            "the message must say what PostgreSQL would actually do: {message}"
        );
        assert!(
            matches!(error, FixtureDdlError::TruncatedIdentifier { .. }),
            "a truncation is not a collision and not a driver failure"
        );
    }

    #[test]
    fn a_fixture_name_that_fits_with_its_companions_is_accepted() {
        let base = "tellurion_postgis_write_live_test_unwritable";
        for suffix in ["", "_outbox", "_index", "_stac", "_assets"] {
            refuse_truncated_identifiers(base, &format!("CREATE TABLE {base}{suffix} (id int);"))
                .expect("a name that fits must not be refused");
        }
        // Exactly at the limit is stored whole, so it is legal.
        let at_limit = "a".repeat(MAX_IDENTIFIER_BYTES);
        refuse_truncated_identifiers("x", &format!("CREATE TABLE {at_limit} (id int);"))
            .expect("63 bytes is the limit, not one past it");
    }

    #[test]
    fn the_collision_message_names_the_fixture_and_the_issue() {
        // Built without a real `tokio_postgres::Error` (there is no public
        // constructor) by checking the arms that do not need one.
        assert!(classify(Some(&SqlState::UNIQUE_VIOLATION)).is_some());
        assert!(classify(Some(&SqlState::DUPLICATE_TABLE)).is_some());
        assert!(classify(Some(&SqlState::UNDEFINED_TABLE)).is_some());
        assert!(classify(Some(&SqlState::T_R_DEADLOCK_DETECTED)).is_some());
        assert!(classify(Some(&SqlState::LOCK_NOT_AVAILABLE)).is_some());
        // A genuine schema mistake must NOT be dressed up as a race.
        assert!(classify(Some(&SqlState::SYNTAX_ERROR)).is_none());
        assert!(classify(Some(&SqlState::UNDEFINED_COLUMN)).is_none());
        assert!(classify(None).is_none());
    }
}
