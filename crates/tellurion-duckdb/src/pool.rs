//! Connection management for one open `.duckdb` file: a small round-robin
//! pool of read-only connections, mirroring `tellurion-geopackage::pool`'s
//! own reader half exactly — this driver never writes, so there is no
//! writer-connection counterpart. Every `duckdb::Connection` call is
//! synchronous C++ FFI, so every operation in this module runs on the tokio
//! blocking thread pool, never directly on an async task, the same
//! `spawn_blocking` discipline every file-backed driver in this workspace
//! documents for its own local-file reads.
//!
//! DuckDB allows multiple connections to open the same file concurrently as
//! long as every one of them is read-only (`AccessMode::ReadOnly`) — this
//! driver never opens a read-write connection at all, since it has no write
//! path (see the crate's own top-level docs).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use duckdb::{AccessMode, Config, Connection};

use crate::error::{DuckdbDriverError, Result};

const MIN_READERS: usize = 2;
const MAX_READERS: usize = 8;

/// `clamp(available_parallelism, 2, 8)` — same rationale
/// `tellurion-geopackage::pool::derive_reader_count` documents: every
/// connection here is a handle onto the same local file, where the ceiling
/// is disk/page-cache throughput, not round-trip count.
fn derive_reader_count() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(2)
        .clamp(MIN_READERS, MAX_READERS)
}

fn open_reader(path: &Path) -> Result<Connection> {
    let config = Config::default().access_mode(AccessMode::ReadOnly)?;
    Ok(Connection::open_with_flags(path, config)?)
}

pub(crate) struct ConnectionPool {
    readers: Vec<Mutex<Connection>>,
    next_reader: AtomicUsize,
}

impl ConnectionPool {
    /// Opens `reader_count` read-only connections against `path`. Fails fast
    /// when `path` doesn't already exist as a file — this driver never
    /// creates a `.duckdb` file (no DDL, no write path at all), so a missing
    /// path is always a config problem, not something to paper over by
    /// letting DuckDB create an empty database in its place.
    pub(crate) fn open(path: PathBuf) -> Result<Self> {
        if !path.is_file() {
            return Err(DuckdbDriverError::MissingFile(path.display().to_string()));
        }

        let reader_count = derive_reader_count();
        let mut readers = Vec::with_capacity(reader_count);
        for _ in 0..reader_count {
            readers.push(Mutex::new(open_reader(&path)?));
        }

        Ok(Self {
            readers,
            next_reader: AtomicUsize::new(0),
        })
    }

    /// The bounded reader-pool size — reported by `driver.rs`'s own
    /// `StorageDriver::capacity_hint` so the server's admission layer can
    /// size its ceiling against what this driver can actually sustain,
    /// mirroring `tellurion-postgis`'s own connection-pool-derived hint.
    pub(crate) fn reader_count(&self) -> usize {
        self.readers.len()
    }

    /// Runs `f` against the first reader connection, synchronously, on
    /// whatever thread calls this — used only by `StorageDriver::
    /// validate_collection`, which has no async counterpart (`Router::build`
    /// calls it as a plain function, never awaited) and needs one short,
    /// boot-time-only DB round trip. Every per-request read still goes
    /// through [`with_reader`]'s `spawn_blocking` + round-robin selection;
    /// this exists only for that one boot checkpoint.
    pub(crate) fn with_first_reader_sync<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T>,
    ) -> Result<T> {
        let conn = recover(self.readers[0].lock());
        f(&conn)
    }
}

/// Recovers from a poisoned mutex (a previous blocking closure panicked while
/// holding the lock) rather than propagating the poison forever — same
/// rationale `tellurion-geopackage::pool::recover` documents: one bad
/// request panicking mid-query must not permanently brick every subsequent
/// request against this file.
fn recover<T>(
    guard: std::sync::LockResult<std::sync::MutexGuard<'_, T>>,
) -> std::sync::MutexGuard<'_, T> {
    guard.unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Runs `f` against a round-robin-selected read-only connection, on the
/// blocking thread pool.
pub(crate) async fn with_reader<T, F>(pool: std::sync::Arc<ConnectionPool>, f: F) -> Result<T>
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
    .map_err(DuckdbDriverError::from)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_rejects_a_missing_file() {
        let missing = PathBuf::from("/tmp/tellurion-duckdb-test-does-not-exist.duckdb");
        assert!(matches!(
            ConnectionPool::open(missing),
            Err(DuckdbDriverError::MissingFile(_))
        ));
    }

    #[test]
    fn open_accepts_an_existing_file_and_opens_a_bounded_reader_pool() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.duckdb");
        Connection::open(&path).unwrap();
        let pool = ConnectionPool::open(path).unwrap();
        assert!(pool.reader_count() >= MIN_READERS);
        assert!(pool.reader_count() <= MAX_READERS);
    }

    #[tokio::test]
    async fn with_reader_reaches_the_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.duckdb");
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE t (id INTEGER)")
            .unwrap();
        let pool = std::sync::Arc::new(ConnectionPool::open(path).unwrap());

        let count: i64 = with_reader(std::sync::Arc::clone(&pool), |conn| {
            conn.query_row("SELECT count(*) FROM t", [], |row| row.get(0))
                .map_err(DuckdbDriverError::from)
        })
        .await
        .unwrap();
        assert_eq!(count, 0);
    }
}
