//! Connection management for one open `.gpkg` file: a single writer
//! connection plus a small round-robin pool of read-only connections, all
//! sharing SQLite's WAL journal mode so reads never block behind a write and
//! the write never blocks behind a slow reader (SQLite's own WAL contract:
//! one writer, many concurrent readers, on one file) — see the crate's own
//! top-level docs for why writes are still serialized through the *single*
//! writer connection regardless (SQLite allows only one writer at a time no
//! matter how many connections ask). Every `rusqlite::Connection` call is
//! synchronous C FFI, so every operation in this module runs on the tokio
//! blocking thread pool, never directly on an async task — the same
//! `spawn_blocking` discipline `tellurion-flatgeobuf`'s `run_blocking` helper
//! documents for its own local-file reads.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OpenFlags};

use crate::error::{GeopackageError, Result};
use crate::functions;

const MIN_READERS: usize = 2;
const MAX_READERS: usize = 8;

/// `clamp(available_parallelism, 2, 8)` — deliberately smaller than
/// `tellurion-postgis::pool`'s `clamp(cores * 2, 4, 32)`: a network
/// connection pool amortizes round-trip latency by fanning out further, but
/// every one of these connections is a handle onto the *same local file*,
/// where the ceiling is disk/page-cache throughput, not round-trip count.
fn derive_reader_count() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(2)
        .clamp(MIN_READERS, MAX_READERS)
}

fn open_connection(path: &Path, read_only: bool) -> Result<Connection> {
    let flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
    } else {
        // Deliberately no `SQLITE_OPEN_CREATE`: the server never runs DDL,
        // and never creates the file a config typo or an unprovisioned path
        // points at either — `ConnectionPool::open`'s own existence check
        // (below) is the fast-fail a missing file gets before this is even
        // called.
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX
    };
    let conn = Connection::open_with_flags(path, flags).map_err(GeopackageError::from)?;
    functions::register(&conn).map_err(GeopackageError::from)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(GeopackageError::from)?;
    // Makes `LIKE` case-sensitive (SQLite's own default folds ASCII case),
    // matching CQL2's/PostgreSQL's default `LIKE` semantics — `sql.rs`'s
    // `compile_filter` doc explains why this matters for `Filter::Like`.
    conn.pragma_update(None, "case_sensitive_like", true)
        .map_err(GeopackageError::from)?;
    Ok(conn)
}

/// `SELECT 1 FROM gpkg_contents LIMIT 1` — the driver-wide "is this even a
/// provisioned GeoPackage" check every `DriverFactory::build` call runs
/// eagerly (fail fast at boot, the same contract every other driver's config
/// typo/missing-table check follows) rather than deferring to the first
/// request. `gpkg_contents` is present in every conformant GeoPackage
/// regardless of whether any table has been provisioned into it yet, so an
/// absent table means either a non-GeoPackage SQLite file or a `.gpkg` this
/// driver's own provisioning subcommand never touched.
fn ensure_provisioned(conn: &Connection, path: &Path) -> Result<()> {
    let exists: bool = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'gpkg_contents'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .map_err(GeopackageError::from)?;
    if !exists {
        return Err(GeopackageError::NotAGeoPackage(path.display().to_string()));
    }
    Ok(())
}

pub(crate) struct ConnectionPool {
    writer: Mutex<Connection>,
    readers: Vec<Mutex<Connection>>,
    next_reader: AtomicUsize,
}

impl ConnectionPool {
    /// Opens `path` as the writer connection, sets WAL journal mode once
    /// (a persistent, file-level property — readers opened afterward simply
    /// observe it), verifies this is a provisioned GeoPackage, then opens the
    /// reader pool. Fails fast (a config-error-shaped [`GeopackageError`])
    /// when `path` doesn't exist or isn't provisioned — never falls back to
    /// creating one.
    pub(crate) fn open(path: PathBuf) -> Result<Self> {
        if !path.is_file() {
            return Err(GeopackageError::NotAGeoPackage(path.display().to_string()));
        }

        let writer = open_connection(&path, false)?;
        writer
            .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))
            .map_err(GeopackageError::from)?;
        ensure_provisioned(&writer, &path)?;

        let reader_count = derive_reader_count();
        let mut readers = Vec::with_capacity(reader_count);
        for _ in 0..reader_count {
            readers.push(Mutex::new(open_connection(&path, true)?));
        }

        Ok(Self {
            writer: Mutex::new(writer),
            readers,
            next_reader: AtomicUsize::new(0),
        })
    }

    pub(crate) fn reader_count(&self) -> usize {
        self.readers.len()
    }
}

/// Recovers from a poisoned mutex (a previous blocking closure panicked
/// while holding the lock) rather than propagating the poison forever: one
/// bad request panicking mid-query must not permanently brick every
/// subsequent request against this file, the same "a handler panic fails one
/// request, not the process" stance the server's own middleware stack takes.
fn recover<'a, T>(
    guard: std::sync::LockResult<std::sync::MutexGuard<'a, T>>,
) -> std::sync::MutexGuard<'a, T> {
    guard.unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Runs `f` against a round-robin-selected read-only connection, on the
/// blocking thread pool.
pub(crate) async fn with_reader<T, F>(pool: Arc<ConnectionPool>, f: F) -> Result<T>
where
    F: FnOnce(&Connection) -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let idx = pool.next_reader.fetch_add(1, Ordering::Relaxed) % pool.readers.len();
        let conn = recover(pool.readers[idx].lock());
        f(&conn)
    })
    .await
    .map_err(GeopackageError::from)?
}

/// Runs `f` against the single writer connection, on the blocking thread
/// pool — every mutation (and its outbox insert, in the same transaction)
/// serializes through this one connection, per the crate's own top-level
/// "one writer" doc.
pub(crate) async fn with_writer<T, F>(pool: Arc<ConnectionPool>, f: F) -> Result<T>
where
    F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let mut conn = recover(pool.writer.lock());
        f(&mut conn)
    })
    .await
    .map_err(GeopackageError::from)?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provisioned_fixture() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.gpkg");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE gpkg_contents (table_name TEXT PRIMARY KEY);
             CREATE TABLE gpkg_spatial_ref_sys (srs_id INTEGER PRIMARY KEY);",
        )
        .unwrap();
        drop(conn);
        (dir, path)
    }

    #[test]
    fn open_rejects_a_missing_file() {
        let missing = PathBuf::from("/tmp/tellurion-geopackage-test-does-not-exist.gpkg");
        assert!(matches!(
            ConnectionPool::open(missing),
            Err(GeopackageError::NotAGeoPackage(_))
        ));
    }

    #[test]
    fn open_rejects_a_sqlite_file_with_no_gpkg_contents_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.sqlite");
        Connection::open(&path)
            .unwrap()
            .execute("CREATE TABLE not_a_gpkg (id INTEGER)", [])
            .unwrap();
        assert!(matches!(
            ConnectionPool::open(path),
            Err(GeopackageError::NotAGeoPackage(_))
        ));
    }

    #[test]
    fn open_accepts_a_provisioned_file_and_sets_wal_mode() {
        let (_dir, path) = provisioned_fixture();
        let pool = ConnectionPool::open(path).unwrap();
        assert!(pool.reader_count() >= MIN_READERS);
        let mode: String = pool
            .writer
            .lock()
            .unwrap()
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");
    }

    #[tokio::test]
    async fn with_reader_and_with_writer_both_reach_the_same_file() {
        let (_dir, path) = provisioned_fixture();
        let pool = Arc::new(ConnectionPool::open(path).unwrap());

        with_writer(Arc::clone(&pool), |conn| {
            conn.execute("INSERT INTO gpkg_contents (table_name) VALUES ('demo')", [])
                .map_err(GeopackageError::from)?;
            Ok(())
        })
        .await
        .unwrap();

        let count: i64 = with_reader(Arc::clone(&pool), |conn| {
            conn.query_row("SELECT count(*) FROM gpkg_contents", [], |row| row.get(0))
                .map_err(GeopackageError::from)
        })
        .await
        .unwrap();
        assert_eq!(count, 1);
    }
}
