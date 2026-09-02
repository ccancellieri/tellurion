//! Serialised DDL for every PostgreSQL object this CLI provisions (`#272`).
//!
//! ## The race, which is not what the error message suggests
//!
//! `CREATE TYPE` appears nowhere in this workspace, yet two operators
//! running `tellurion-ingest … create-tables` at the same time get a `23505`
//! unique violation on `pg_type_typname_nsp_index`. Every `CREATE TABLE`
//! implicitly creates a composite type of the same name, so a `CREATE TABLE`
//! race lands on `pg_type`'s own unique index; a `bigserial` column's
//! implicit sequence lands on `pg_class_relname_nsp_index` beside it, and a
//! `CREATE INDEX` lands there too.
//!
//! **`CREATE TABLE IF NOT EXISTS` does not make this safe.** It checks for
//! the relation and then inserts the catalog rows, with no lock spanning the
//! two: two sessions racing it both see "absent" and the loser fails. The
//! `IF NOT EXISTS` on every statement in this crate's DDL makes a *rerun*
//! idempotent, which is a different property from making a *concurrent* run
//! safe, and the doc comments that conflated the two are why `#272` exists.
//!
//! Measured against this workspace's cluster with the outbox DDL — six
//! concurrent sessions, twenty rounds, each round starting from a dropped
//! table: **17 of 20 rounds failed, 85 of 120 sessions**, split between
//! `23505` on `pg_type_typname_nsp_index` (the composite type), `23505` on
//! `pg_class_relname_nsp_index` (the `bigserial` column's implicit
//! sequence) and `42P07` (the relation, for the loser that got far enough
//! to see it). Behind `pg_advisory_xact_lock`: **0 of 20 rounds, 0 of 120
//! sessions**. `#138` measured 5 of 20 for the registry DDL; the rate is
//! higher here because `bigserial` makes each `CREATE TABLE` three catalog
//! insertions rather than one, which is three chances to collide.
//!
//! This is not hypothetical for a control plane several people administer:
//! one operator and a CI job, or a retried invocation overlapping its
//! predecessor, is enough. And the failure names a PostgreSQL catalog index
//! rather than anything the operator typed, so it reads as a database
//! defect.
//!
//! ## What [`apply_ddl`] does about it
//!
//! It takes `pg_advisory_xact_lock` on a key derived from the name of the
//! object being provisioned, in the same transaction as the DDL. That is
//! mutual exclusion the *database* provides, so it reaches across
//! processes, across hosts, and across an operator and a CI job — none of
//! which a process-local lock can see. The lock is released by the commit
//! (or the rollback), so no failure path can leak it, and it is taken per
//! object rather than globally: provisioning two different collections'
//! tables never waits.
//!
//! ### Which lock space, and why not the lease's
//!
//! PostgreSQL keeps two disjoint advisory-lock spaces, one keyed by a single
//! `bigint` and one by a pair of `int4`s. `tellurion-postgis`'s `lease_sql`
//! uses the `bigint` space for real applier leadership. Provisioning uses
//! the `(int4, int4)` space under a fixed [`PROVISION_LOCK_CLASS`], so a
//! provisioning lock can never — not even by hash collision — be mistaken by
//! PostgreSQL for a lease key.
//!
//! ### Why the same class and key function as the test harness
//!
//! `#138` gave `tellurion-postgis`'s live-test harness the same idiom for
//! the same DDL. That harness and this module protect *the same tables*: a
//! live test seeding `demo_outbox` and an operator running `outbox
//! create-tables --table demo` race each other exactly as two operators do.
//! They only exclude each other if they land on the same key, so
//! [`PROVISION_LOCK_CLASS`] and [`lock_key`] are deliberately identical to
//! `tellurion_postgis::test_harness`'s [`FIXTURE_LOCK_CLASS`] and
//! `fixture_lock_key`, and [`REGISTRY_TABLES_OBJECT`] to its
//! `REGISTRY_TABLES_FIXTURE`.
//!
//! They are a hand-kept copy rather than a shared import because this crate
//! never depends on a driver crate in production (see this crate's own
//! top-level doc — the same reason five modules here each carry their own
//! `quote_table_ident`), and the harness is behind a `test-support` feature
//! that must never link into a shipped binary. The copy is not kept honest
//! by a comment: the tests at the bottom of this file assert equality
//! against the harness itself, through the dev-dependency `#138` already
//! added.
//!
//! [`FIXTURE_LOCK_CLASS`]: https://docs.rs/tellurion-postgis
//!
//! ## What it deliberately does not change
//!
//! The DDL text is handed to `batch_execute` byte for byte, and the SQL each
//! command prints for an operator to apply by hand is the same text it
//! always was — an operator pasting it into `psql` gets exactly the
//! statements they got before this module existed. `batch_execute` of a
//! multi-statement string already ran as one implicit transaction, so the
//! explicit `BEGIN`/`COMMIT` around it changes nothing an uncontended caller
//! can observe. A single operator provisioning a database sees no
//! difference at all — `provisioning_under_the_lock_builds_the_same_table_
//! as_an_unlocked_batch_execute` in `outbox.rs` compares the two against the
//! column, index and sequence catalogs rather than asserting it.
//!
//! ## Named refusals
//!
//! An uncontended acquisition is immediate. A contended one says, by name,
//! which backend holds the lock and that this command is waiting for it —
//! because "the CLI hung" is the report we would otherwise get. The wait is
//! bounded ([`LOCK_WAIT`]) and its expiry is a refusal that names the other
//! operator, not a silent abandonment of the DDL: an unbounded wait in a CI
//! job is its own outage.

use std::path::Path;

use anyhow::Context;
use rusqlite::Connection;
use tokio_postgres::error::SqlState;
use tokio_postgres::Client;

/// The `int4` class half of every provisioning lock, chosen once so every
/// caller lands in one namespace. Identical to
/// `tellurion_postgis::test_harness::FIXTURE_LOCK_CLASS` on purpose — see
/// this module's doc — and pinned by a test against it.
pub const PROVISION_LOCK_CLASS: i32 = 0x7e11;

/// The object name `registry::create_tables` locks under.
///
/// `registry_tenants`, `registry_catalogs` and `registry_collections` are
/// created by one statement batch and shared by the whole deployment, so
/// they are one lockable unit under one name. A caller that locked
/// `registry_catalogs` while another locked `registry_tenants` would take
/// two different locks and race exactly as before, which is why this is a
/// constant rather than each site naming a table of its own — and why it is
/// spelled the same as the test harness's `REGISTRY_TABLES_FIXTURE`.
pub const REGISTRY_TABLES_OBJECT: &str = "registry_tenants";

/// How long a contended provisioning waits before refusing by name.
///
/// Not a tuning knob and not a timeout on the DDL itself (see
/// [`acquire`], which restores the server's own `lock_timeout` before the
/// DDL runs): it bounds only the wait for *another operator's* provisioning
/// transaction. Every `create-tables` in this crate is a handful of
/// catalog writes, so a wait this long means somebody else's provisioning
/// is in flight — or stuck — rather than that ours is slow, and saying so
/// is more useful than blocking forever in a CI job. `variants materialize`
/// is the one command that can legitimately hold it longer (it backfills a
/// column), and a second operator being told "that is running, wait for it"
/// is the right answer there too.
const LOCK_WAIT: &str = "30s";

/// PostgreSQL truncates identifiers at `NAMEDATALEN - 1` bytes. The same
/// number `sanitize::sanitize_identifier` and every `quote_table_ident` in
/// this crate already enforce, named once here for the refusal below.
const MAX_IDENTIFIER_BYTES: usize = 63;

/// A stable 32-bit key for `object`, as the `int4` key half of the advisory
/// lock.
///
/// FNV-1a, spelled out rather than `std::hash::DefaultHasher`, because the
/// value must be identical across processes, builds and versions of this
/// binary: two operators only exclude each other if their two builds compute
/// the same number. Byte-identical to
/// `tellurion_postgis::test_harness::fixture_lock_key`, asserted by a test
/// below. Two object names colliding costs a needless wait between two
/// unrelated provisionings, never a wrong result.
pub fn lock_key(object: &str) -> i32 {
    const FNV_OFFSET_BASIS: u32 = 0x811c_9dc5;
    const FNV_PRIME: u32 = 0x0100_0193;
    let mut hash = FNV_OFFSET_BASIS;
    for byte in object.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash as i32
}

/// Refuses, by name, DDL that would hand PostgreSQL an identifier longer
/// than it can store (`#272`).
///
/// PostgreSQL does not reject an over-long identifier — it **truncates it to
/// 63 bytes and carries on**, with a notice most clients discard. Two
/// logically distinct tables whose names differ only past byte 63 therefore
/// become one table, silently, and the second provisioning "succeeds"
/// against the first one's rows. `tellurion_postgis_write_live_test_create_
/// text_id_type_mismatch` is 62 bytes and its `_outbox` companion is 69:
/// this is a live property of this workspace, not a hypothetical.
///
/// Every module in this crate already validates the identifiers it derives
/// (`quote_table_ident`/`quote_ident` reject at 63 bytes), so this is the
/// backstop rather than the primary check: it sits at the one place all
/// PostgreSQL DDL passes through, so a derivation added later that forgets
/// to validate still refuses instead of truncating.
///
/// String literals, dollar-quoted bodies and comments are stripped before
/// scanning — a long `description` value is not an identifier, and
/// mislabelling one would be the same failure in the other direction.
/// Identifiers *inside* a dollar-quoted body (a `DO` block's own constraint
/// names) are consequently not covered; they are compile-time constants in
/// this crate, not derived from operator input.
pub fn refuse_overlong_identifiers(sql: &str) -> anyhow::Result<()> {
    for token in identifier_tokens(sql) {
        if token.len() > MAX_IDENTIFIER_BYTES {
            anyhow::bail!(
                "'{token}' is {} bytes; PostgreSQL identifiers are limited to \
                 {MAX_IDENTIFIER_BYTES} and it would be SILENTLY TRUNCATED to '{}' rather than \
                 rejected — two objects whose names differ only past byte {MAX_IDENTIFIER_BYTES} \
                 would become one. Refusing to provision. This name is derived from the one you \
                 passed, so shorten that: a table name must leave room for the '_outbox'/\
                 '_index'/'_assets'/'_stac' companions and their index suffixes this crate \
                 derives from it (#272).",
                token.len(),
                truncated(&token),
            );
        }
    }
    Ok(())
}

/// What PostgreSQL would actually store: the first `MAX_IDENTIFIER_BYTES`
/// bytes, cut back to a character boundary.
///
/// The boundary walk is not decoration. A double-quoted identifier may hold
/// any UTF-8 at all, so byte 63 can land mid-character — and a panic while
/// building a refusal message would replace a clear refusal with a crash,
/// which is the one thing worse than the truncation being reported. (This is
/// approximate for a multi-byte name: PostgreSQL truncates on *character*
/// boundaries by its own rules. The message is a warning about a name being
/// too long, not a promise about the exact bytes that would survive.)
fn truncated(identifier: &str) -> &str {
    let mut end = MAX_IDENTIFIER_BYTES;
    while end > 0 && !identifier.is_char_boundary(end) {
        end -= 1;
    }
    &identifier[..end]
}

/// The identifier-shaped tokens of `sql`, with comments, string literals and
/// dollar-quoted bodies removed. Deliberately crude — it exists to catch a
/// name that is too long, not to parse SQL.
fn identifier_tokens(sql: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let bytes = sql.as_bytes();
    let mut i = 0;
    let mut current = String::new();
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                flush(&mut current, &mut tokens);
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                flush(&mut current, &mut tokens);
                i += 2;
                while i < bytes.len() && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
            }
            b'\'' => {
                flush(&mut current, &mut tokens);
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\'' {
                        // `''` is an escaped quote, not the end.
                        if bytes.get(i + 1) == Some(&b'\'') {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
            }
            b'$' => {
                flush(&mut current, &mut tokens);
                match dollar_tag(bytes, i) {
                    Some(tag) => {
                        let after = i + tag.len();
                        match sql[after..].find(&tag) {
                            Some(end) => i = after + end + tag.len(),
                            None => i = bytes.len(),
                        }
                    }
                    // `$1` and friends: a bind placeholder, not a quote.
                    None => i += 1,
                }
            }
            b'"' => {
                flush(&mut current, &mut tokens);
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += 1;
                }
                tokens.push(sql[start..i].to_string());
                i += 1;
            }
            _ if c.is_ascii_alphanumeric() || c == b'_' => {
                current.push(c as char);
                i += 1;
            }
            _ => {
                flush(&mut current, &mut tokens);
                i += 1;
            }
        }
    }
    flush(&mut current, &mut tokens);
    tokens
}

fn flush(current: &mut String, tokens: &mut Vec<String>) {
    if !current.is_empty() {
        tokens.push(std::mem::take(current));
    }
}

/// The `$tag$` opening a dollar-quoted block at `at`, tag delimiters
/// included, or `None` when this `$` opens no block (`$1`, or a bare `$`).
fn dollar_tag(bytes: &[u8], at: usize) -> Option<String> {
    let mut end = at + 1;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        // A tag may not start with a digit; `$1` is a placeholder.
        if bytes[end].is_ascii_digit() && end == at + 1 {
            return None;
        }
        end += 1;
    }
    if bytes.get(end) == Some(&b'$') {
        Some(String::from_utf8_lossy(&bytes[at..=end]).into_owned())
    } else {
        None
    }
}

/// Applies `sql` in one transaction that first takes the provisioning lock
/// for `object`.
///
/// `object` is the *base* name the DDL builds from — the `--table` argument
/// at almost every call site. The derived `{table}_outbox`/`{table}_index`/
/// `{table}_assets`/`{table}_stac` companions are covered by the same lock
/// because the same statement batch creates them, and because a live test
/// seeding the same collection locks that same base name.
///
/// The error is the caller's to describe: this returns the driver's own
/// failure unchanged (wrapped only where the lock itself is what went
/// wrong), so every command's existing `with_context` message survives.
pub async fn apply_ddl(client: &Client, object: &str, sql: &str) -> anyhow::Result<()> {
    refuse_overlong_identifiers(sql)?;
    begin_locked(client, object).await?;
    if let Err(error) = client.batch_execute(sql).await {
        rollback(client).await;
        return Err(error.into());
    }
    commit(client).await
}

/// Opens a transaction holding the provisioning lock for `object`, for the
/// callers whose provisioning is not one statement batch — `seed`'s
/// ownership-marker check and `locking install-touch-trigger`'s preflight
/// both read the catalog and then act on what they read, and a lock that did
/// not span both would leave exactly the check-then-act window this module
/// exists to close.
///
/// Every path out must reach [`commit`] or [`rollback`].
pub async fn begin_locked(client: &Client, object: &str) -> anyhow::Result<()> {
    client
        .batch_execute("BEGIN")
        .await
        .context("opening the provisioning transaction")?;
    if let Err(error) = acquire(client, object).await {
        rollback(client).await;
        return Err(error);
    }
    Ok(())
}

pub async fn commit(client: &Client) -> anyhow::Result<()> {
    client
        .batch_execute("COMMIT")
        .await
        .context("committing the provisioning transaction")
}

/// Best-effort: the transaction is already failing, and a rollback that
/// cannot be sent means the connection is gone, which the caller's own error
/// already says. Its value is leaving the client usable rather than failing
/// every later statement with `25P02` and burying the real message.
pub async fn rollback(client: &Client) {
    let _ = client.batch_execute("ROLLBACK").await;
}

/// Takes the lock, saying by name who holds it when it is contended.
async fn acquire(client: &Client, object: &str) -> anyhow::Result<()> {
    acquire_waiting(client, object, LOCK_WAIT).await
}

/// [`acquire`] with the wait spelled out, so the refusal path can be tested
/// in a quarter of a second instead of [`LOCK_WAIT`]. Not a knob: `acquire`
/// is the only production caller and it always passes [`LOCK_WAIT`]. An
/// untested refusal is a refusal that has never been read.
async fn acquire_waiting(client: &Client, object: &str, wait: &str) -> anyhow::Result<()> {
    let key = lock_key(object);
    let acquired: bool = client
        .query_one(
            "SELECT pg_try_advisory_xact_lock($1::int4, $2::int4)",
            &[&PROVISION_LOCK_CLASS, &key],
        )
        .await
        .context("taking the provisioning lock")?
        .get(0);
    if acquired {
        return Ok(());
    }

    // Contended. Name the holder before waiting: "the CLI hung" is the bug
    // report an unexplained wait produces.
    let holders = describe_holders(client, key).await;
    tracing::warn!(
        object = %object,
        holder = %holders,
        "another provisioning transaction holds the lock for this object; waiting up to {wait} for it (#272)"
    );

    // `SET LOCAL` is undone by the transaction end, and `TO DEFAULT` puts
    // the server's own `lock_timeout` back before the DDL runs — so the DDL
    // waits for its table locks exactly as long as it did before this module
    // existed, and only the advisory-lock wait is bounded.
    client
        .batch_execute(&format!("SET LOCAL lock_timeout = '{wait}'"))
        .await
        .context("bounding the provisioning-lock wait")?;
    let waited = client
        .query_one(
            "SELECT pg_advisory_xact_lock($1::int4, $2::int4)",
            &[&PROVISION_LOCK_CLASS, &key],
        )
        .await;
    client
        .batch_execute("SET LOCAL lock_timeout TO DEFAULT")
        .await
        .ok();

    match waited {
        Ok(_) => Ok(()),
        Err(error) if error.code() == Some(&SqlState::LOCK_NOT_AVAILABLE) => anyhow::bail!(
            "PROVISIONING LOCK BUSY: another process has been provisioning '{object}' against this \
             database for longer than {wait} — {holders}. Nothing was created or altered. \
             This is concurrent administration, not a defect in the DDL: rerun once the other run \
             finishes, and the command will be the same no-op it always is against an \
             already-provisioned database (#272)."
        ),
        Err(error) => Err(anyhow::Error::new(error).context("waiting for the provisioning lock")),
    }
}

/// Who holds the provisioning lock at `key`, for the messages above.
///
/// `pg_locks` records a two-key advisory lock with `objsubid = 2` (the
/// one-key `bigint` space uses `1`), so this can never pick up a production
/// lease. Best-effort: a server that will not answer still gets a message,
/// just a vaguer one.
async fn describe_holders(client: &Client, key: i32) -> String {
    let rows = client
        .query(
            "SELECT l.pid, coalesce(a.application_name, ''), coalesce(a.state, '') \
             FROM pg_locks l LEFT JOIN pg_stat_activity a ON a.pid = l.pid \
             WHERE l.locktype = 'advisory' AND l.objsubid = 2 \
               AND l.classid = $1::int4::oid AND l.objid = $2::int4::oid AND l.granted",
            &[&PROVISION_LOCK_CLASS, &key],
        )
        .await
        .unwrap_or_default();
    if rows.is_empty() {
        return "holder not identifiable from pg_locks".to_string();
    }
    rows.iter()
        .map(|row| {
            format!(
                "backend pid {} (application_name {:?}, state {:?})",
                row.get::<_, i32>(0),
                row.get::<_, &str>(1),
                row.get::<_, &str>(2)
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// How long a GeoPackage provisioning waits for another writer before
/// refusing by name — SQLite's own `busy_timeout`, the same five seconds
/// `tellurion-geopackage`'s connection pool already picks for the serving
/// side, so the two halves of this workspace answer contention the same way.
const SQLITE_BUSY_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Opens a GeoPackage for provisioning.
///
/// ## Why there is no advisory lock here, and no race to lock against
///
/// `#272` is a PostgreSQL catalog race, and SQLite does not have it. Its
/// answer to concurrency is a single writer at a time over the whole
/// database file: a write transaction holds an exclusive lock, and the
/// `CREATE TABLE IF NOT EXISTS` existence check and the `sqlite_master`
/// insert that follows it both happen *inside* that one lock. That is
/// precisely the atomicity PostgreSQL's `IF NOT EXISTS` lacks, so there is
/// no window for two provisionings to both see "absent". SQLite also
/// re-prepares any statement whose schema cookie moved under it, so the
/// loser of a race never executes against a stale schema. Adding a lock of
/// our own would buy nothing and would be a second, weaker copy of a
/// guarantee the file format already gives.
///
/// The symmetry is not total, and this is the part that needed changing:
/// where PostgreSQL's loser *waits* for the lock, SQLite's loser fails
/// immediately with `SQLITE_BUSY` unless a busy timeout is set — and
/// `Connection::open` sets none. Two operators provisioning the same
/// GeoPackage would get "database is locked", which names SQLite's
/// mechanism rather than the other operator. So: wait
/// [`SQLITE_BUSY_WAIT`] like the serving side does, and if that expires,
/// say what is actually happening ([`sqlite_contention`]).
///
/// Measured with `geopackage create-tables`' own DDL, six concurrent
/// writers over fifteen rounds: **zero rounds ended with a wrong or
/// duplicated catalog in either configuration** — that is the claim above,
/// tested. With no busy timeout, 15 of 15 rounds and 73 of 90 writers
/// failed with "database is locked"; with this one, 0 of 15 and 0 of 90.
pub fn open_geopackage(path: &Path) -> anyhow::Result<Connection> {
    let connection =
        Connection::open(path).with_context(|| format!("opening '{}'", path.display()))?;
    connection
        .busy_timeout(SQLITE_BUSY_WAIT)
        .with_context(|| format!("setting the busy timeout on '{}'", path.display()))?;
    Ok(connection)
}

/// The named refusal for a GeoPackage that another process is writing to, or
/// `None` when `error` is anything else.
///
/// Matched on SQLite's own busy/locked result codes rather than on message
/// text: those two codes mean "somebody else holds the write lock" and
/// nothing else, and passing every other failure through unchanged is what
/// keeps a genuine schema error reading as a schema error.
pub fn sqlite_contention(error: &rusqlite::Error, path: &Path) -> Option<String> {
    let rusqlite::Error::SqliteFailure(failure, _) = error else {
        return None;
    };
    if !matches!(
        failure.code,
        rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
    ) {
        return None;
    }
    Some(format!(
        "GEOPACKAGE BUSY: another process held the write lock on '{}' for longer than {} seconds, \
         so nothing was created or altered. SQLite allows one writer at a time over the whole \
         file — most likely another `tellurion-ingest geopackage` run, or a server serving this \
         same file. Rerun once it finishes; provisioning is idempotent (#272).",
        path.display(),
        SQLITE_BUSY_WAIT.as_secs()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of pinning the key function: a different build of
    /// this file — an older operator's binary, a live-test binary — must
    /// compute the same number, or the two stop excluding each other and
    /// `#272` is back with no test failing.
    #[test]
    fn the_lock_key_is_pinned_across_builds_and_versions() {
        assert_eq!(lock_key("registry_tenants"), 1_345_481_640);
        assert_eq!(lock_key(""), -2_128_831_035);
        assert_eq!(lock_key("demo"), lock_key("demo"));
    }

    /// Production provisioning and the `#138` live-test harness protect the
    /// same tables, so they must land on the same key. Asserted against the
    /// harness itself rather than a comment, through the dev-dependency
    /// `#138` added.
    #[test]
    fn provisioning_and_the_live_test_harness_share_one_lock_namespace() {
        assert_eq!(
            PROVISION_LOCK_CLASS,
            tellurion_postgis::test_harness::FIXTURE_LOCK_CLASS,
            "a provisioning lock and a fixture lock must be in the same class, or an operator's \
             create-tables and a live test's fixture DDL race each other"
        );
        assert_eq!(
            REGISTRY_TABLES_OBJECT,
            tellurion_postgis::test_harness::REGISTRY_TABLES_FIXTURE
        );
        for object in [
            "registry_tenants",
            "demo",
            "tellurion_jobs",
            "tellurion_postgis_live_test_items",
            "",
        ] {
            assert_eq!(
                lock_key(object),
                tellurion_postgis::test_harness::fixture_lock_key(object),
                "key functions diverged for '{object}'"
            );
        }
    }

    #[test]
    fn distinct_objects_get_distinct_keys_so_unrelated_provisionings_never_wait() {
        let objects = ["registry_tenants", "tellurion_jobs", "demo", "italy_places"];
        let mut keys: Vec<i32> = objects.iter().copied().map(lock_key).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "lock keys collided: {objects:?}");
    }

    #[test]
    fn an_identifier_past_the_limit_is_refused_by_name() {
        // The exact pair `#272` records: 62 bytes, and 69 once the outbox
        // suffix is derived from it.
        let base = "tellurion_postgis_write_live_test_create_text_id_type_mismatch";
        assert_eq!(base.len(), 62);
        let error = refuse_overlong_identifiers(&format!(
            "CREATE TABLE IF NOT EXISTS {base}_outbox (sequence bigserial PRIMARY KEY);"
        ))
        .expect_err("69 bytes must be refused, not truncated");
        let message = error.to_string();
        assert!(message.contains(&format!("{base}_outbox")), "{message}");
        assert!(message.contains("69 bytes"), "{message}");
        assert!(message.contains("SILENTLY TRUNCATED"), "{message}");
    }

    #[test]
    fn a_quoted_identifier_past_the_limit_is_refused_too() {
        let long = "a".repeat(64);
        assert!(
            refuse_overlong_identifiers(&format!("CREATE TABLE \"{long}\" (id int);")).is_err()
        );
    }

    /// A double-quoted identifier may hold any UTF-8, so byte 63 can land
    /// mid-character. Building the refusal must not panic there — a crash
    /// instead of a refusal is worse than the truncation it was reporting.
    #[test]
    fn a_multibyte_identifier_is_refused_without_panicking_on_a_char_boundary() {
        // 32 two-byte characters, so byte 63 is inside the 32nd.
        let long = "é".repeat(32);
        assert_eq!(long.len(), 64);
        let error = refuse_overlong_identifiers(&format!("CREATE TABLE \"{long}\" (id int);"))
            .expect_err("64 bytes must be refused");
        assert!(error.to_string().contains("64 bytes"), "{error}");
    }

    #[test]
    fn exactly_sixty_three_bytes_is_accepted_because_postgres_stores_it_whole() {
        let at_limit = "b".repeat(63);
        refuse_overlong_identifiers(&format!("CREATE TABLE {at_limit} (id int);"))
            .expect("63 bytes is the limit, not one past it");
    }

    /// A long *value* is not an identifier. Mislabelling one would be the
    /// same failure as truncation, in the other direction: a refusal an
    /// operator cannot act on.
    #[test]
    fn long_string_literals_comments_and_dollar_quoted_bodies_are_not_identifiers() {
        let wkt = "x".repeat(400);
        refuse_overlong_identifiers(&format!(
            "-- {wkt}\n\
             /* {wkt} */\n\
             INSERT INTO gpkg_spatial_ref_sys (definition) VALUES ('{wkt}');\n\
             DO $$ BEGIN RAISE NOTICE '{wkt}'; END $$;"
        ))
        .expect("a long literal, comment or block body is not an identifier");
    }

    /// The registry DDL is the one statement batch in this crate that mixes
    /// a `DO $$ … $$` block with ordinary statements, and every command's
    /// SQL has to survive the scan unchanged.
    #[test]
    fn every_ddl_this_crate_ships_passes_the_identifier_scan() {
        refuse_overlong_identifiers(crate::registry::CREATE_REGISTRY_TABLES_SQL)
            .expect("the registry DDL");
    }

    /// The contended-but-patient path: a second provisioning of the same
    /// object waits for the first and then applies its DDL. This is what
    /// makes the fix a *serialisation* rather than a refusal — the whole
    /// point is that both operators end up with the tables.
    #[tokio::test]
    async fn a_second_provisioning_waits_for_the_first_and_then_succeeds() {
        let Some(url) = tellurion_postgis::test_harness::require_database_url(
            "a_second_provisioning_waits_for_the_first_and_then_succeeds",
        ) else {
            return;
        };
        let object = "tellurion_ingest_provision_wait";
        let holder = crate::db::connect_url(&url).await.expect("holder connects");

        begin_locked(&holder, object)
            .await
            .expect("holder takes it");
        let waiting = tokio::spawn({
            let url = url.clone();
            async move {
                let client = crate::db::connect_url(&url).await.expect("connects");
                apply_ddl(&client, object, "SELECT 1").await
            }
        });
        // Long enough that the waiter has certainly reached the lock and is
        // parked on it — if it were not, this would pass for the wrong
        // reason and the assertion below would prove nothing.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert!(
            !waiting.is_finished(),
            "the second provisioning must WAIT for the first, not fail or skip"
        );

        commit(&holder).await.expect("holder releases it");
        waiting
            .await
            .expect("the waiting task did not panic")
            .expect("the second provisioning succeeds once the first commits");
    }

    /// The refusal `LOCK_WAIT` exists for, exercised at a wait short enough
    /// to test. An operator who hits this must be told another process is
    /// provisioning, by name — not left staring at a CLI that has stopped.
    #[tokio::test]
    async fn a_provisioning_lock_that_never_frees_is_refused_by_name() {
        let Some(url) = tellurion_postgis::test_harness::require_database_url(
            "a_provisioning_lock_that_never_frees_is_refused_by_name",
        ) else {
            return;
        };
        let object = "tellurion_ingest_provision_busy";
        let holder = crate::db::connect_url(&url).await.expect("holder connects");
        begin_locked(&holder, object)
            .await
            .expect("holder takes it");

        let blocked = crate::db::connect_url(&url)
            .await
            .expect("blocked connects");
        blocked.batch_execute("BEGIN").await.expect("opens");
        let error = acquire_waiting(&blocked, object, "250ms")
            .await
            .expect_err("a lock that never frees must be refused, not waited on forever");
        rollback(&blocked).await;

        let message = error.to_string();
        assert!(
            message.contains("PROVISIONING LOCK BUSY"),
            "the refusal must name itself: {message}"
        );
        assert!(
            message.contains(object),
            "the refusal must name the object: {message}"
        );
        assert!(
            message.contains("backend pid"),
            "the refusal must name the holder, or the operator cannot act on it: {message}"
        );
        assert!(
            message.contains("Nothing was created or altered"),
            "the refusal must say the database is untouched: {message}"
        );

        commit(&holder).await.expect("holder releases it");
    }

    #[test]
    fn a_bind_placeholder_is_not_a_dollar_quote() {
        let long = "c".repeat(70);
        // `$1` must not swallow the rest of the statement, or the over-long
        // name after it would go unnoticed.
        assert!(
            refuse_overlong_identifiers(&format!("SELECT $1; CREATE TABLE {long} (id int);"))
                .is_err()
        );
    }
}
