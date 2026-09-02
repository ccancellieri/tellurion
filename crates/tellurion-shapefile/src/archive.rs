use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    ops::Range,
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use crc32fast::Hasher;
use tellurion_http_source::{ContentIdentity, RangeObject};
use tokio::{
    io::AsyncWriteExt,
    sync::{Mutex as AsyncMutex, Notify, Semaphore},
};
use zip::{CompressionMethod, ZipArchive};

#[cfg(test)]
use std::sync::atomic::AtomicUsize;

use crate::error::{ArchiveError, Result};

const MIB: u64 = 1024 * 1024;
const EOCD_SIGNATURE: [u8; 4] = *b"PK\x05\x06";
const CENTRAL_SIGNATURE: [u8; 4] = *b"PK\x01\x02";
const LOCAL_SIGNATURE: [u8; 4] = *b"PK\x03\x04";
const ZIP64_EXTRA_ID: u16 = 0x0001;
const MAX_EOCD_TAIL: u64 = 65_557;
const COPY_BUFFER_BYTES: usize = 64 * 1024;
const MAX_COMPRESSED_BYTES: u64 = 48 * MIB;
const MAX_EXPANDED_BYTES: u64 = 256 * MIB;
const MAX_MEMBERS: usize = 32;
const MAX_RATIO: u64 = 100;
const MAX_CONCURRENT: usize = 2;
const MAX_AGGREGATE_BYTES: u64 = 512 * MIB;

#[cfg(test)]
static EXTRACTION_PAUSED: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
static EXTRACTION_STARTED: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
static EXTRACTION_WRITE_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
static EXTRACTION_WRITES: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
static EXTRACTION_WRITES_PAUSED: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
static EXTRACTION_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Hard bounds for one public Shapefile ZIP materialization.
#[derive(Debug, Clone)]
pub struct ArchiveLimits {
    pub max_compressed_bytes: u64,
    pub max_expanded_bytes: u64,
    pub max_members: usize,
    pub max_ratio: u64,
    pub max_concurrent: usize,
    pub max_aggregate_bytes: u64,
    pub expiry: Duration,
    pub deadline: Duration,
    pub range_chunk_bytes: usize,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_compressed_bytes: MAX_COMPRESSED_BYTES,
            max_expanded_bytes: MAX_EXPANDED_BYTES,
            max_members: MAX_MEMBERS,
            max_ratio: MAX_RATIO,
            max_concurrent: MAX_CONCURRENT,
            max_aggregate_bytes: MAX_AGGREGATE_BYTES,
            expiry: Duration::from_secs(15 * 60),
            deadline: Duration::from_secs(2 * 60),
            range_chunk_bytes: COPY_BUFFER_BYTES,
        }
    }
}

/// A materialized, fully validated Shapefile dataset.  Clones keep its
/// private owner-only directory alive until all consumers release it.
#[derive(Clone)]
pub struct ValidatedShapefile {
    pub shp: PathBuf,
    pub shx: PathBuf,
    pub dbf: PathBuf,
    pub prj: Option<PathBuf>,
    pub cpg: Option<PathBuf>,
    _entry: Arc<CachedEntry>,
}

impl std::fmt::Debug for ValidatedShapefile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ValidatedShapefile")
            .field("shp", &self.shp)
            .field("shx", &self.shx)
            .field("dbf", &self.dbf)
            .field("prj", &self.prj)
            .field("cpg", &self.cpg)
            .finish()
    }
}

/// A revision-keyed archive cache with a separately bounded spool lane.
#[derive(Clone)]
pub struct ArchiveSpool {
    root: Arc<PathBuf>,
    limits: ArchiveLimits,
    state: Arc<AsyncMutex<SpoolState>>,
    permits: Arc<Semaphore>,
    quota: Arc<DiskQuota>,
}

struct SpoolState {
    entries: HashMap<CacheKey, Arc<CachedEntry>>,
    in_progress: HashMap<CacheKey, Arc<Notify>>,
}

struct DiskQuota {
    limit: u64,
    used: Mutex<u64>,
}

impl DiskQuota {
    fn try_reserve(&self, bytes: u64) -> bool {
        let mut used = self.used.lock().expect("spool quota mutex poisoned");
        let Some(next) = used.checked_add(bytes) else {
            return false;
        };
        if next > self.limit {
            return false;
        }
        *used = next;
        true
    }

    fn release(&self, bytes: u64) {
        let mut used = self.used.lock().expect("spool quota mutex poisoned");
        *used = used
            .checked_sub(bytes)
            .expect("spool quota reservation underflow");
    }
}

struct DiskReservation {
    quota: Arc<DiskQuota>,
    bytes: u64,
}

impl DiskReservation {
    fn new(quota: Arc<DiskQuota>) -> Self {
        Self { quota, bytes: 0 }
    }

    fn try_grow(&mut self, bytes: u64) -> bool {
        if self.quota.try_reserve(bytes) {
            self.bytes += bytes;
            true
        } else {
            false
        }
    }
}

impl Drop for DiskReservation {
    fn drop(&mut self) {
        self.quota.release(self.bytes);
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct CacheKey {
    source: [u8; 32],
    revision: [u8; 32],
    length: u64,
}

struct FlightGuard {
    state: Arc<AsyncMutex<SpoolState>>,
    key: CacheKey,
    notify: Arc<Notify>,
    active: bool,
    worker_owns_cleanup: bool,
}

impl FlightGuard {
    fn finish(&mut self) {
        self.active = false;
    }
}

impl Drop for FlightGuard {
    fn drop(&mut self) {
        if !self.active || self.worker_owns_cleanup {
            return;
        }
        let state = Arc::clone(&self.state);
        let key = self.key;
        let notify = Arc::clone(&self.notify);
        let removed_immediately = if let Ok(mut guard) = state.try_lock() {
            guard.in_progress.remove(&key);
            notify.notify_waiters();
            true
        } else {
            false
        };
        if !removed_immediately {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    state.lock().await.in_progress.remove(&key);
                    notify.notify_waiters();
                });
            }
        }
    }
}

struct CachedEntry {
    directory: PathBuf,
    last_access: Mutex<Instant>,
    reservation: Option<DiskReservation>,
}

struct WorkerCompletion {
    state: Arc<AsyncMutex<SpoolState>>,
    key: CacheKey,
    notify: Arc<Notify>,
    runtime: tokio::runtime::Handle,
    armed: AtomicBool,
}

impl WorkerCompletion {
    fn new(state: Arc<AsyncMutex<SpoolState>>, key: CacheKey, notify: Arc<Notify>) -> Self {
        Self {
            state,
            key,
            notify,
            runtime: tokio::runtime::Handle::current(),
            armed: AtomicBool::new(true),
        }
    }

    fn disarm(&mut self) {
        self.armed.store(false, Ordering::Release);
    }
}

impl Drop for WorkerCompletion {
    fn drop(&mut self) {
        if !self.armed.load(Ordering::Acquire) {
            return;
        }
        let state = Arc::clone(&self.state);
        let key = self.key;
        let notify = Arc::clone(&self.notify);
        let removed_immediately = if let Ok(mut guard) = state.try_lock() {
            guard.in_progress.remove(&key);
            notify.notify_waiters();
            true
        } else {
            false
        };
        if !removed_immediately {
            self.runtime.spawn(async move {
                state.lock().await.in_progress.remove(&key);
                notify.notify_waiters();
            });
        }
    }
}

struct WorkerResult {
    working: WorkingDirectory,
    outcome: Result<u64>,
    completion: WorkerCompletion,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl CachedEntry {
    fn touch(&self) {
        *self
            .last_access
            .lock()
            .expect("spool timestamp mutex poisoned") = Instant::now();
    }

    fn last_access(&self) -> Instant {
        *self
            .last_access
            .lock()
            .expect("spool timestamp mutex poisoned")
    }
}

impl Drop for CachedEntry {
    fn drop(&mut self) {
        release_after_directory_cleanup(&self.directory, &mut self.reservation);
    }
}

struct WorkingDirectory {
    path: PathBuf,
    retained: bool,
    reservation: Option<DiskReservation>,
}

impl WorkingDirectory {
    fn new(root: &Path, quota: Arc<DiskQuota>) -> Result<Self> {
        fs::create_dir_all(root)?;
        let directory = tempfile::Builder::new()
            .prefix("shapefile-")
            .tempdir_in(root)?;
        set_private_directory(directory.path())?;
        Ok(Self {
            path: directory.keep(),
            retained: false,
            reservation: Some(DiskReservation::new(quota)),
        })
    }

    fn reservation_mut(&mut self) -> &mut DiskReservation {
        self.reservation
            .as_mut()
            .expect("working directory owns its quota reservation")
    }

    fn retained_bytes(&self) -> u64 {
        self.reservation
            .as_ref()
            .expect("working directory owns its quota reservation")
            .bytes
    }

    fn retain(mut self) -> (PathBuf, DiskReservation) {
        self.retained = true;
        let reservation = self
            .reservation
            .take()
            .expect("working directory owns its quota reservation");
        (self.path.clone(), reservation)
    }
}

impl Drop for WorkingDirectory {
    fn drop(&mut self) {
        if !self.retained {
            release_after_directory_cleanup(&self.path, &mut self.reservation);
        } else {
            debug_assert!(self.reservation.is_none());
        }
    }
}

fn release_after_directory_cleanup(path: &Path, reservation: &mut Option<DiskReservation>) {
    let removed = match fs::remove_dir_all(path) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    };
    let Some(reservation) = reservation.take() else {
        return;
    };
    if removed {
        drop(reservation);
    } else {
        // Retain the charge if storage cleanup fails: under-admission is safe,
        // while releasing it would let later writes cross the physical limit.
        std::mem::forget(reservation);
    }
}

impl ArchiveSpool {
    pub fn new(root: impl AsRef<Path>, limits: ArchiveLimits) -> Result<Self> {
        if limits.max_members == 0
            || limits.max_concurrent == 0
            || limits.max_ratio == 0
            || limits.range_chunk_bytes == 0
            || limits.max_compressed_bytes == 0
            || limits.max_expanded_bytes == 0
            || limits.max_aggregate_bytes == 0
            || limits.max_compressed_bytes > MAX_COMPRESSED_BYTES
            || limits.max_expanded_bytes > MAX_EXPANDED_BYTES
            || limits.max_members > MAX_MEMBERS
            || limits.max_ratio > MAX_RATIO
            || limits.max_concurrent > MAX_CONCURRENT
            || limits.max_aggregate_bytes > MAX_AGGREGATE_BYTES
            || limits.range_chunk_bytes > COPY_BUFFER_BYTES
        {
            return Err(ArchiveError::Limit);
        }
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        Ok(Self {
            root: Arc::new(root),
            permits: Arc::new(Semaphore::new(limits.max_concurrent)),
            quota: Arc::new(DiskQuota {
                limit: limits.max_aggregate_bytes,
                used: Mutex::new(0),
            }),
            limits,
            state: Arc::new(AsyncMutex::new(SpoolState {
                entries: HashMap::new(),
                in_progress: HashMap::new(),
            })),
        })
    }

    /// Materializes one immutable `RangeObject` into a validated private
    /// directory. Concurrent requests for the same revision reuse one entry.
    pub async fn materialize(&self, object: Arc<dyn RangeObject>) -> Result<ValidatedShapefile> {
        let key = identity(object.as_ref());
        let expected_length = key.length;
        if expected_length != object.length() || expected_length > self.limits.max_compressed_bytes
        {
            return Err(ArchiveError::Limit);
        }

        let notify = loop {
            let mut state = self.state.lock().await;
            purge_expired(&mut state, self.limits.expiry);
            if let Some(entry) = state.entries.get(&key) {
                entry.touch();
                return Ok(validated(entry.clone()));
            }
            if let Some(notify) = state.in_progress.get(&key) {
                let notified = Arc::clone(notify).notified_owned();
                drop(state);
                notified.await;
            } else {
                let notify = Arc::new(Notify::new());
                state.in_progress.insert(key, Arc::clone(&notify));
                break notify;
            }
        };
        self.materialize_owner(object, key, notify).await
    }

    async fn materialize_owner(
        &self,
        object: Arc<dyn RangeObject>,
        key: CacheKey,
        notify: Arc<Notify>,
    ) -> Result<ValidatedShapefile> {
        let mut flight = FlightGuard {
            state: Arc::clone(&self.state),
            key,
            notify: Arc::clone(&notify),
            active: true,
            worker_owns_cleanup: false,
        };
        let outcome = self.materialize_new(object, key, &mut flight).await;
        let mut state = self.state.lock().await;
        state.in_progress.remove(&key);
        let result = match outcome {
            Ok(entry) => {
                state.entries.insert(key, entry.clone());
                Ok(validated(entry))
            }
            Err(error) => Err(error),
        };
        flight.finish();
        notify.notify_waiters();
        result
    }

    /// Removes expired, unreferenced cache entries. Active readers retain their
    /// entries; quota admission will refuse new work rather than deleting one.
    pub async fn cleanup_expired(&self) {
        let mut state = self.state.lock().await;
        purge_expired(&mut state, self.limits.expiry);
    }

    /// Removes cache entries that no active consumer retains. Public session
    /// reapers call this after dropping expired sources so their private
    /// materializations do not remain on disk until the cache TTL elapses.
    pub async fn cleanup_unused(&self) {
        self.state
            .lock()
            .await
            .entries
            .retain(|_, entry| Arc::strong_count(entry) > 1);
    }

    async fn materialize_new(
        &self,
        object: Arc<dyn RangeObject>,
        key: CacheKey,
        flight: &mut FlightGuard,
    ) -> Result<Arc<CachedEntry>> {
        let permit = self
            .permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ArchiveError::Capacity)?;
        let started = Instant::now();
        let mut working = WorkingDirectory::new(&self.root, Arc::clone(&self.quota))?;
        let archive_path = working.path.join("archive.zip");
        spool_object(
            object.as_ref(),
            &archive_path,
            key.length,
            &self.limits,
            started,
            &self.state,
            working.reservation_mut(),
        )
        .await?;
        if identity(object.as_ref()) != key {
            return Err(ArchiveError::Source(
                tellurion_http_source::SourceError::for_handle(
                    tellurion_http_source::SourceErrorKind::Invalidated,
                    object.handle(),
                ),
            ));
        }
        let limits = self.limits.clone();
        let directory = working.path.clone();
        let state = Arc::clone(&self.state);
        let completion =
            WorkerCompletion::new(Arc::clone(&self.state), key, Arc::clone(&flight.notify));
        flight.worker_owns_cleanup = true;
        let worker = tokio::task::spawn_blocking(move || {
            let mut working = working;
            let outcome = validate_and_extract(
                &archive_path,
                &directory,
                &limits,
                started,
                &state,
                working.reservation_mut(),
            );
            WorkerResult {
                working,
                outcome,
                completion,
                _permit: permit,
            }
        });
        let WorkerResult {
            working,
            outcome,
            mut completion,
            ..
        } = worker.await.map_err(|_| ArchiveError::Worker)?;
        flight.worker_owns_cleanup = false;
        completion.disarm();
        let extracted = outcome?;
        let disk_bytes = key
            .length
            .checked_add(extracted)
            .ok_or(ArchiveError::Limit)?;
        if working.retained_bytes() != disk_bytes {
            return Err(ArchiveError::Limit);
        }
        let (directory, reservation) = working.retain();
        Ok(Arc::new(CachedEntry {
            directory,
            last_access: Mutex::new(Instant::now()),
            reservation: Some(reservation),
        }))
    }
}

fn identity(object: &dyn RangeObject) -> CacheKey {
    match object.identity() {
        ContentIdentity::StrongEtag {
            source_key,
            revision_key,
            length,
            ..
        } => CacheKey {
            source: *source_key,
            revision: *revision_key,
            length: *length,
        },
    }
}

fn validated(entry: Arc<CachedEntry>) -> ValidatedShapefile {
    entry.touch();
    let files = entry.directory.join("files");
    let base = files.join("dataset");
    ValidatedShapefile {
        shp: base.with_extension("shp"),
        shx: base.with_extension("shx"),
        dbf: base.with_extension("dbf"),
        prj: files
            .join("dataset.prj")
            .exists()
            .then(|| base.with_extension("prj")),
        cpg: files
            .join("dataset.cpg")
            .exists()
            .then(|| base.with_extension("cpg")),
        _entry: entry,
    }
}

async fn spool_object(
    object: &dyn RangeObject,
    archive_path: &Path,
    length: u64,
    limits: &ArchiveLimits,
    started: Instant,
    state: &Arc<AsyncMutex<SpoolState>>,
    reservation: &mut DiskReservation,
) -> Result<()> {
    let archive = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(archive_path)?;
    set_private_file(&archive)?;
    let mut archive = tokio::fs::File::from_std(archive);
    let chunk = u64::try_from(limits.range_chunk_bytes).map_err(|_| ArchiveError::Limit)?;
    let mut offset = 0;
    while offset < length {
        check_deadline(started, limits)?;
        let end = offset.saturating_add(chunk).min(length);
        let remaining = limits
            .deadline
            .checked_sub(started.elapsed())
            .ok_or(ArchiveError::Deadline)?;
        let body = tokio::time::timeout(remaining, object.get_range(offset..end))
            .await
            .map_err(|_| ArchiveError::Deadline)?
            .map_err(ArchiveError::Source)?;
        if body.len() != usize::try_from(end - offset).map_err(|_| ArchiveError::Limit)? {
            return Err(ArchiveError::Source(
                tellurion_http_source::SourceError::for_handle(
                    tellurion_http_source::SourceErrorKind::Range,
                    object.handle(),
                ),
            ));
        }
        reserve_disk_async(
            state,
            limits.expiry,
            reservation,
            u64::try_from(body.len()).map_err(|_| ArchiveError::Limit)?,
        )
        .await?;
        archive.write_all(&body).await?;
        offset = end;
    }
    archive.sync_all().await?;
    Ok(())
}

fn validate_and_extract(
    archive_path: &Path,
    directory: &Path,
    limits: &ArchiveLimits,
    started: Instant,
    state: &Arc<AsyncMutex<SpoolState>>,
    reservation: &mut DiskReservation,
) -> Result<u64> {
    let archive_len = fs::metadata(archive_path)?.len();
    if archive_len > limits.max_compressed_bytes {
        return Err(ArchiveError::Limit);
    }
    let central = read_central_directory(archive_path, archive_len, limits)?;
    #[cfg(test)]
    pause_extraction_for_test();
    let output_directory = directory.join("files");
    fs::create_dir(&output_directory)?;
    set_private_directory(&output_directory)?;

    let archive_file = File::open(archive_path)?;
    let mut archive = ZipArchive::new(archive_file).map_err(|_| ArchiveError::UnsafeZip)?;
    if archive.len() != central.len() {
        return Err(ArchiveError::UnsafeZip);
    }
    let total_compressed = central.iter().try_fold(0_u64, |sum, member| {
        sum.checked_add(member.compressed_range.end - member.compressed_range.start)
            .ok_or(ArchiveError::Limit)
    })?;
    let mut total_expanded = 0_u64;
    for member in central {
        check_deadline(started, limits)?;
        let mut input = archive
            .by_index(member.index)
            .map_err(|_| ArchiveError::UnsafeZip)?;
        if input.name_raw() != member.raw_name
            || input.encrypted()
            || input.compression() != member.compression
            || input.compressed_size() != member.compressed
            || input.size() != member.expanded
            || input.crc32() != member.crc32
        {
            return Err(ArchiveError::UnsafeZip);
        }
        let Some(extension) = member.extension else {
            continue;
        };
        let output = output_directory.join(format!("dataset.{extension}"));
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)?;
        set_private_file(&output)?;
        let mut member_expanded = 0_u64;
        let mut hasher = Hasher::new();
        let mut buffer = [0_u8; COPY_BUFFER_BYTES];
        loop {
            check_deadline(started, limits)?;
            let read = input
                .read(&mut buffer)
                .map_err(|_| ArchiveError::Integrity)?;
            if read == 0 {
                break;
            }
            let read = u64::try_from(read).map_err(|_| ArchiveError::Limit)?;
            member_expanded = member_expanded
                .checked_add(read)
                .ok_or(ArchiveError::Limit)?;
            total_expanded = total_expanded
                .checked_add(read)
                .ok_or(ArchiveError::Limit)?;
            if member_expanded > limits.max_expanded_bytes
                || total_expanded > limits.max_expanded_bytes
                || exceeds_ratio(member_expanded, member.compressed, limits.max_ratio)
                || exceeds_ratio(total_expanded, total_compressed, limits.max_ratio)
            {
                return Err(ArchiveError::Limit);
            }
            hasher.update(&buffer[..usize::try_from(read).map_err(|_| ArchiveError::Limit)?]);
            #[cfg(test)]
            EXTRACTION_WRITE_ATTEMPTS.fetch_add(1, Ordering::AcqRel);
            reserve_disk_blocking(state, limits.expiry, reservation, read)?;
            output.write_all(&buffer[..usize::try_from(read).map_err(|_| ArchiveError::Limit)?])?;
            #[cfg(test)]
            {
                EXTRACTION_WRITES.fetch_add(1, Ordering::AcqRel);
                while EXTRACTION_WRITES_PAUSED.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
        if member_expanded != member.expanded || hasher.finalize() != member.crc32 {
            return Err(ArchiveError::Integrity);
        }
    }
    Ok(total_expanded)
}

#[derive(Debug)]
struct CentralMember {
    index: usize,
    raw_name: Vec<u8>,
    extension: Option<&'static str>,
    compression: CompressionMethod,
    compressed: u64,
    compressed_range: Range<u64>,
    expanded: u64,
    crc32: u32,
}

fn read_central_directory(
    path: &Path,
    file_len: u64,
    limits: &ArchiveLimits,
) -> Result<Vec<CentralMember>> {
    let mut file = File::open(path)?;
    let (eocd_offset, central_offset, central_size, entries) = read_eocd(&mut file, file_len)?;
    let entries = usize::from(entries);
    let central_end = central_offset
        .checked_add(central_size)
        .ok_or(ArchiveError::UnsafeZip)?;
    if central_end != eocd_offset || entries > limits.max_members {
        return Err(ArchiveError::Limit);
    }
    file.seek(SeekFrom::Start(central_offset))?;
    let mut central = vec![0_u8; usize::try_from(central_size).map_err(|_| ArchiveError::Limit)?];
    file.read_exact(&mut central)?;
    let mut position = 0_usize;
    let mut names = HashSet::new();
    let mut dataset: Option<(String, String)> = None;
    let mut extensions = HashSet::new();
    let mut members = Vec::with_capacity(entries);
    let mut local_intervals = Vec::with_capacity(entries);
    for index in 0..entries {
        let header = central
            .get(position..position + 46)
            .ok_or(ArchiveError::UnsafeZip)?;
        if header[..4] != CENTRAL_SIGNATURE {
            return Err(ArchiveError::UnsafeZip);
        }
        let flags = le_u16(header, 8)?;
        let method = le_u16(header, 10)?;
        let crc32 = le_u32(header, 16)?;
        let compressed = u64::from(le_u32(header, 20)?);
        let expanded = u64::from(le_u32(header, 24)?);
        let name_len = usize::from(le_u16(header, 28)?);
        let extra_len = usize::from(le_u16(header, 30)?);
        let comment_len = usize::from(le_u16(header, 32)?);
        let disk = le_u16(header, 34)?;
        let external = le_u32(header, 38)?;
        let local_offset = u64::from(le_u32(header, 42)?);
        let data_end = position
            .checked_add(46)
            .and_then(|value| value.checked_add(name_len))
            .and_then(|value| value.checked_add(extra_len))
            .and_then(|value| value.checked_add(comment_len))
            .ok_or(ArchiveError::UnsafeZip)?;
        let name = central
            .get(position + 46..position + 46 + name_len)
            .ok_or(ArchiveError::UnsafeZip)?;
        let extra = central
            .get(position + 46 + name_len..position + 46 + name_len + extra_len)
            .ok_or(ArchiveError::UnsafeZip)?;
        position = data_end;
        if flags & 0b1 != 0
            || flags & 0b1000 != 0
            || flags & 0b1_0000_0000 != 0
            || disk != 0
            || has_zip64_extra(extra)?
        {
            return Err(ArchiveError::UnsafeZip);
        }
        let compression = match method {
            0 => CompressionMethod::Stored,
            8 => CompressionMethod::Deflated,
            _ => return Err(ArchiveError::UnsafeZip),
        };
        let (parent, basename, extension) = validate_name(name, external)?;
        if !names.insert(name.to_ascii_lowercase()) {
            return Err(ArchiveError::InvalidDataset);
        }
        if let Some(extension) = extension {
            if !extensions.insert(extension) {
                return Err(ArchiveError::InvalidDataset);
            }
            match &dataset {
                Some((known_parent, known_base))
                    if known_parent != &parent || known_base != &basename =>
                {
                    return Err(ArchiveError::InvalidDataset)
                }
                None => dataset = Some((parent, basename)),
                _ => {}
            }
        }
        let layout = validate_local_header(
            &mut file,
            local_offset,
            name,
            flags,
            method,
            crc32,
            compressed,
            expanded,
            central_offset,
        )?;
        if expanded > limits.max_expanded_bytes
            || exceeds_ratio(expanded, compressed, limits.max_ratio)
        {
            return Err(ArchiveError::Limit);
        }
        members.push(CentralMember {
            index,
            raw_name: name.to_vec(),
            extension,
            compression,
            compressed,
            compressed_range: layout.data,
            expanded,
            crc32,
        });
        local_intervals.push(layout.interval);
    }
    if position != central.len()
        || !extensions.contains("shp")
        || !extensions.contains("shx")
        || !extensions.contains("dbf")
    {
        return Err(ArchiveError::InvalidDataset);
    }
    local_intervals.sort_unstable_by_key(|interval| interval.start);
    if local_intervals
        .windows(2)
        .any(|pair| pair[1].start < pair[0].end)
    {
        return Err(ArchiveError::UnsafeZip);
    }
    Ok(members)
}

fn read_eocd(file: &mut File, file_len: u64) -> Result<(u64, u64, u64, u16)> {
    let tail_len = file_len.min(MAX_EOCD_TAIL);
    file.seek(SeekFrom::Start(file_len - tail_len))?;
    let mut tail = vec![0_u8; usize::try_from(tail_len).map_err(|_| ArchiveError::Limit)?];
    file.read_exact(&mut tail)?;
    let eocd = tail
        .windows(4)
        .rposition(|window| window == EOCD_SIGNATURE)
        .ok_or(ArchiveError::UnsafeZip)?;
    let header = tail.get(eocd..eocd + 22).ok_or(ArchiveError::UnsafeZip)?;
    let comment_len = usize::from(le_u16(header, 20)?);
    if eocd + 22 + comment_len != tail.len()
        || le_u16(header, 4)? != 0
        || le_u16(header, 6)? != 0
        || le_u16(header, 8)? != le_u16(header, 10)?
    {
        return Err(ArchiveError::UnsafeZip);
    }
    let entries = le_u16(header, 10)?;
    let size = le_u32(header, 12)?;
    let offset = le_u32(header, 16)?;
    if entries == u16::MAX || size == u32::MAX || offset == u32::MAX {
        return Err(ArchiveError::UnsafeZip);
    }
    let absolute_offset =
        file_len - tail_len + u64::try_from(eocd).map_err(|_| ArchiveError::Limit)?;
    Ok((absolute_offset, u64::from(offset), u64::from(size), entries))
}

struct LocalLayout {
    interval: Range<u64>,
    data: Range<u64>,
}

#[allow(clippy::too_many_arguments)]
fn validate_local_header(
    file: &mut File,
    offset: u64,
    expected_name: &[u8],
    flags: u16,
    method: u16,
    crc32: u32,
    compressed: u64,
    expanded: u64,
    central_offset: u64,
) -> Result<LocalLayout> {
    let header_end = offset.checked_add(30).ok_or(ArchiveError::UnsafeZip)?;
    if header_end > central_offset {
        return Err(ArchiveError::UnsafeZip);
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut header = [0_u8; 30];
    file.read_exact(&mut header)?;
    if header[..4] != LOCAL_SIGNATURE
        || le_u16(&header, 6)? != flags
        || le_u16(&header, 8)? != method
        || le_u32(&header, 14)? != crc32
        || u64::from(le_u32(&header, 18)?) != compressed
        || u64::from(le_u32(&header, 22)?) != expanded
    {
        return Err(ArchiveError::UnsafeZip);
    }
    let name_len = usize::from(le_u16(&header, 26)?);
    let extra_len = usize::from(le_u16(&header, 28)?);
    let variable = name_len
        .checked_add(extra_len)
        .ok_or(ArchiveError::UnsafeZip)?;
    let data_start = header_end
        .checked_add(u64::try_from(variable).map_err(|_| ArchiveError::Limit)?)
        .ok_or(ArchiveError::UnsafeZip)?;
    let data_end = data_start
        .checked_add(compressed)
        .ok_or(ArchiveError::UnsafeZip)?;
    if data_end > central_offset || name_len != expected_name.len() {
        return Err(ArchiveError::UnsafeZip);
    }
    let mut name = vec![0_u8; name_len];
    file.read_exact(&mut name)?;
    if name != expected_name {
        return Err(ArchiveError::UnsafeZip);
    }
    let mut extra = vec![0_u8; extra_len];
    file.read_exact(&mut extra)?;
    if has_zip64_extra(&extra)? {
        return Err(ArchiveError::UnsafeZip);
    }
    Ok(LocalLayout {
        interval: offset..data_end,
        data: data_start..data_end,
    })
}

fn validate_name(name: &[u8], external: u32) -> Result<(String, String, Option<&'static str>)> {
    let name = std::str::from_utf8(name).map_err(|_| ArchiveError::InvalidDataset)?;
    if name.is_empty()
        || !name.is_ascii()
        || name.contains('\\')
        || name.contains(':')
        || name.starts_with('/')
        || name.ends_with('/')
    {
        return Err(ArchiveError::InvalidDataset);
    }
    let path = Path::new(name);
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(ArchiveError::InvalidDataset);
    }
    let mut pieces = name.split('/');
    if pieces.clone().any(|piece| {
        piece.is_empty() || piece == "." || piece == ".." || is_windows_reserved(piece)
    }) {
        return Err(ArchiveError::InvalidDataset);
    }
    let filename = pieces.next_back().ok_or(ArchiveError::InvalidDataset)?;
    let (basename, extension) = filename.rsplit_once('.').unwrap_or((filename, ""));
    if basename.is_empty() || basename.contains('\0') {
        return Err(ArchiveError::InvalidDataset);
    }
    let extension = match extension.to_ascii_lowercase().as_str() {
        "shp" => Some("shp"),
        "shx" => Some("shx"),
        "dbf" => Some("dbf"),
        "prj" => Some("prj"),
        "cpg" => Some("cpg"),
        "zip" => return Err(ArchiveError::InvalidDataset),
        _ => None,
    };
    let mode = external >> 16;
    let file_type = mode & 0o170000;
    if (file_type != 0 && file_type != 0o100000) || external & 0x10 != 0 {
        return Err(ArchiveError::UnsafeZip);
    }
    let parent = pieces.collect::<Vec<_>>().join("/").to_lowercase();
    Ok((parent, basename.to_lowercase(), extension))
}

fn is_windows_reserved(piece: &str) -> bool {
    let stem = piece
        .trim_end_matches([' ', '.'])
        .split_once('.')
        .map_or(piece, |(stem, _)| stem)
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
}

fn has_zip64_extra(mut extra: &[u8]) -> Result<bool> {
    while !extra.is_empty() {
        let header = extra.get(..4).ok_or(ArchiveError::UnsafeZip)?;
        let id = le_u16(header, 0)?;
        let len = usize::from(le_u16(header, 2)?);
        extra = extra
            .get(4..)
            .and_then(|value| value.get(len..))
            .ok_or(ArchiveError::UnsafeZip)?;
        if id == ZIP64_EXTRA_ID {
            return Ok(true);
        }
    }
    Ok(false)
}

fn le_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let bytes = bytes
        .get(offset..offset + 2)
        .ok_or(ArchiveError::UnsafeZip)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn le_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let bytes = bytes
        .get(offset..offset + 4)
        .ok_or(ArchiveError::UnsafeZip)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn exceeds_ratio(expanded: u64, compressed: u64, ratio: u64) -> bool {
    compressed == 0 && expanded != 0 || expanded > compressed.saturating_mul(ratio)
}

fn check_deadline(started: Instant, limits: &ArchiveLimits) -> Result<()> {
    if started.elapsed() > limits.deadline {
        Err(ArchiveError::Deadline)
    } else {
        Ok(())
    }
}

#[cfg(test)]
fn pause_extraction_for_test() {
    EXTRACTION_STARTED.store(true, Ordering::Release);
    while EXTRACTION_PAUSED.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn purge_expired(state: &mut SpoolState, expiry: Duration) {
    let now = Instant::now();
    state.entries.retain(|_, entry| {
        Arc::strong_count(entry) > 1 || now.duration_since(entry.last_access()) <= expiry
    });
}

fn reserve_disk(
    state: &mut SpoolState,
    expiry: Duration,
    reservation: &mut DiskReservation,
    incoming: u64,
) -> Result<()> {
    purge_expired(state, expiry);
    loop {
        if reservation.try_grow(incoming) {
            return Ok(());
        }
        let Some(revision) = state
            .entries
            .iter()
            .filter(|(_, entry)| Arc::strong_count(entry) == 1)
            .min_by_key(|(_, entry)| entry.last_access())
            .map(|(revision, _)| *revision)
        else {
            return Err(ArchiveError::Capacity);
        };
        drop(state.entries.remove(&revision));
    }
}

async fn reserve_disk_async(
    state: &Arc<AsyncMutex<SpoolState>>,
    expiry: Duration,
    reservation: &mut DiskReservation,
    incoming: u64,
) -> Result<()> {
    let mut state = state.lock().await;
    reserve_disk(&mut state, expiry, reservation, incoming)
}

fn reserve_disk_blocking(
    state: &Arc<AsyncMutex<SpoolState>>,
    expiry: Duration,
    reservation: &mut DiskReservation,
    incoming: u64,
) -> Result<()> {
    let mut state = state.blocking_lock();
    reserve_disk(&mut state, expiry, reservation, incoming)
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file(file: &File) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file(_file: &File) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Cursor, Write},
        ops::Range,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::Duration,
    };

    use async_trait::async_trait;
    use bytes::Bytes;
    use tellurion_http_source::{ContentIdentity, RangeObject, SourceError, SourceHandle};
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

    use super::{
        ArchiveLimits, ArchiveSpool, EXTRACTION_PAUSED, EXTRACTION_STARTED, EXTRACTION_TEST_LOCK,
        EXTRACTION_WRITES, EXTRACTION_WRITES_PAUSED, EXTRACTION_WRITE_ATTEMPTS,
    };

    struct Fixture {
        bytes: Vec<u8>,
        handle: SourceHandle,
        identity: ContentIdentity,
        reads: Arc<AtomicUsize>,
        before_read: Option<Arc<dyn Fn() + Send + Sync>>,
    }

    impl Fixture {
        fn new(bytes: Vec<u8>, reads: Arc<AtomicUsize>) -> Self {
            let length = bytes.len() as u64;
            Self {
                bytes,
                handle: SourceHandle::new("extraction-cancellation"),
                identity: ContentIdentity::StrongEtag {
                    source_key: [4; 32],
                    revision_key: [5; 32],
                    length,
                },
                reads,
                before_read: None,
            }
        }

        fn with_identity(mut self, source_key: [u8; 32], revision_key: [u8; 32]) -> Self {
            self.identity = ContentIdentity::StrongEtag {
                source_key,
                revision_key,
                length: self.bytes.len() as u64,
            };
            self
        }

        fn before_read(mut self, before_read: Arc<dyn Fn() + Send + Sync>) -> Self {
            self.before_read = Some(before_read);
            self
        }
    }

    #[async_trait]
    impl RangeObject for Fixture {
        fn handle(&self) -> &SourceHandle {
            &self.handle
        }
        fn identity(&self) -> &ContentIdentity {
            &self.identity
        }
        fn length(&self) -> u64 {
            self.bytes.len() as u64
        }
        fn display_name(&self) -> &str {
            "fixture.zip"
        }
        async fn get_range(&self, range: Range<u64>) -> Result<Bytes, SourceError> {
            if let Some(before_read) = &self.before_read {
                before_read();
            }
            self.reads.fetch_add(1, Ordering::Relaxed);
            Ok(Bytes::copy_from_slice(
                &self.bytes[range.start as usize..range.end as usize],
            ))
        }
    }

    struct PauseReset;

    impl Drop for PauseReset {
        fn drop(&mut self) {
            EXTRACTION_PAUSED.store(false, Ordering::Release);
            EXTRACTION_STARTED.store(false, Ordering::Release);
            EXTRACTION_WRITE_ATTEMPTS.store(0, Ordering::Release);
            EXTRACTION_WRITES.store(0, Ordering::Release);
            EXTRACTION_WRITES_PAUSED.store(false, Ordering::Release);
        }
    }

    fn content_bytes(path: &std::path::Path) -> u64 {
        std::fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .map(|path| {
                if path.is_dir() {
                    content_bytes(&path)
                } else {
                    std::fs::metadata(path).unwrap().len()
                }
            })
            .sum()
    }

    fn archive() -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, value) in [
            ("ne.shp", b"shape" as &[u8]),
            ("ne.shx", b"index"),
            ("ne.dbf", b"table"),
        ] {
            writer.start_file(name, options).unwrap();
            writer.write_all(value).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[tokio::test]
    async fn fix_round_one_cached_and_working_bytes_never_cross_aggregate_quota() {
        let _hook_lock = EXTRACTION_TEST_LOCK.lock().await;
        let root = tempfile::tempdir().unwrap();
        let bytes = archive();
        let limit = u64::try_from(bytes.len()).unwrap() * 2 + 5;
        let spool = ArchiveSpool::new(
            root.path(),
            ArchiveLimits {
                max_aggregate_bytes: limit,
                ..ArchiveLimits::default()
            },
        )
        .unwrap();
        let _pause_reset = PauseReset;
        EXTRACTION_WRITE_ATTEMPTS.store(0, Ordering::Release);
        EXTRACTION_WRITES.store(0, Ordering::Release);
        EXTRACTION_WRITES_PAUSED.store(true, Ordering::Release);

        let first = tokio::spawn({
            let spool = spool.clone();
            let object = Arc::new(
                Fixture::new(bytes.clone(), Arc::new(AtomicUsize::new(0)))
                    .with_identity([1; 32], [1; 32]),
            );
            async move { spool.materialize(object).await }
        });
        let second = tokio::spawn({
            let spool = spool.clone();
            let object = Arc::new(
                Fixture::new(bytes, Arc::new(AtomicUsize::new(0))).with_identity([2; 32], [2; 32]),
            );
            async move { spool.materialize(object).await }
        });

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let attempted = EXTRACTION_WRITE_ATTEMPTS.load(Ordering::Acquire) >= 2;
                let settled = EXTRACTION_WRITES.load(Ordering::Acquire) >= 2
                    || first.is_finished()
                    || second.is_finished();
                if attempted && settled {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both workers must reach aggregate admission");

        let observed = content_bytes(root.path());
        assert!(
            observed <= limit,
            "cached plus working content crossed the aggregate quota"
        );

        EXTRACTION_WRITES_PAUSED.store(false, Ordering::Release);
        let outcomes = [first.await.unwrap(), second.await.unwrap()];
        assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    }

    #[tokio::test]
    async fn fix_round_one_failed_materialization_releases_aggregate_quota() {
        let _hook_lock = EXTRACTION_TEST_LOCK.lock().await;
        let root = tempfile::tempdir().unwrap();
        let valid = archive();
        let limit = u64::try_from(valid.len() + 15).unwrap();
        let spool = ArchiveSpool::new(
            root.path(),
            ArchiveLimits {
                max_aggregate_bytes: limit,
                ..ArchiveLimits::default()
            },
        )
        .unwrap();
        let mut corrupt = valid.clone();
        let data = corrupt
            .windows(4)
            .position(|window| window == b"PK\x03\x04")
            .unwrap();
        let name_len = u16::from_le_bytes([corrupt[data + 26], corrupt[data + 27]]) as usize;
        let extra_len = u16::from_le_bytes([corrupt[data + 28], corrupt[data + 29]]) as usize;
        corrupt[data + 30 + name_len + extra_len] ^= 1;

        let failed = Arc::new(
            Fixture::new(corrupt, Arc::new(AtomicUsize::new(0))).with_identity([3; 32], [3; 32]),
        );
        assert!(spool.materialize(failed).await.is_err());
        assert_eq!(content_bytes(root.path()), 0);

        let later = Arc::new(
            Fixture::new(valid, Arc::new(AtomicUsize::new(0))).with_identity([4; 32], [4; 32]),
        );
        assert!(spool.materialize(later).await.is_ok());
    }

    #[tokio::test]
    async fn cancelled_extraction_keeps_the_spool_slot_until_the_worker_exits() {
        let _hook_lock = EXTRACTION_TEST_LOCK.lock().await;
        let root = tempfile::tempdir().unwrap();
        let aggregate_limit = u64::try_from((archive().len() + 15) * 2).unwrap();
        let spool = ArchiveSpool::new(
            root.path(),
            ArchiveLimits {
                max_concurrent: 1,
                max_aggregate_bytes: aggregate_limit,
                ..ArchiveLimits::default()
            },
        )
        .unwrap();
        let _pause_reset = PauseReset;
        EXTRACTION_STARTED.store(false, Ordering::Release);
        EXTRACTION_PAUSED.store(true, Ordering::Release);
        let first_reads = Arc::new(AtomicUsize::new(0));
        let first = Arc::new(Fixture::new(archive(), first_reads).with_identity([4; 32], [5; 32]));
        let task = tokio::spawn({
            let spool = spool.clone();
            async move { spool.materialize(first).await }
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while !EXTRACTION_STARTED.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first extraction must begin");
        task.abort();
        let second_reads = Arc::new(AtomicUsize::new(0));
        let observed_root_entries = Arc::new(AtomicUsize::new(usize::MAX));
        let root_path = root.path().to_path_buf();
        let before_read = Arc::new({
            let observed_root_entries = Arc::clone(&observed_root_entries);
            move || {
                observed_root_entries.store(
                    std::fs::read_dir(&root_path).unwrap().count(),
                    Ordering::Release,
                );
            }
        });
        let second = Arc::new(
            Fixture::new(archive(), Arc::clone(&second_reads))
                .with_identity([6; 32], [7; 32])
                .before_read(before_read),
        );
        let next = tokio::spawn({
            let spool = spool.clone();
            async move { spool.materialize(second).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(second_reads.load(Ordering::Relaxed), 0);
        EXTRACTION_PAUSED.store(false, Ordering::Release);
        let validated = next.await.unwrap().unwrap();
        assert_eq!(observed_root_entries.load(Ordering::Acquire), 1);
        drop(validated);

        let later = Arc::new(
            Fixture::new(archive(), Arc::new(AtomicUsize::new(0))).with_identity([8; 32], [9; 32]),
        );
        assert!(spool.materialize(later).await.is_ok());
    }
}
