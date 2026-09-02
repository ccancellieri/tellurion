//! `ConfigStore` is the seam between "where config lives" and everything that
//! consumes it. v0.1 ships a file-backed YAML store; a DB- or Valkey-backed
//! store implements the same trait later without touching the router or the
//! protocol crates.
//!
//! `load` is synchronous: config is read once at process startup (never on
//! the request path — see the "no runtime DDL / behavior in config" rules),
//! so there is no benefit to an async signature and every call site stays
//! simpler without one. A future networked store can still do blocking I/O
//! inside `load` (or spawn its own runtime) without changing this trait.
//!
//! `load_versioned` (`#110`) is the read half of a versioned contract:
//! [`VersionedConfig`] pairs the document with an opaque [`ConfigVersion`]
//! token identifying the exact revision it was read from — the handle a
//! compare-and-swap [`write`](ConfigStore::write) presents back, so a write
//! against a document that changed since this read is a named conflict
//! ([`Error::VersionConflict`]) instead of a silent lost update.
//! `FileConfigStore` builds its token on the same byte-for-byte freshness
//! check the operator CLI's own atomic-replace machinery already performs
//! before swapping the file in (`ensure_source_unchanged`,
//! `crates/tellurion-ingest/src/operator.rs`) — a digest of the raw bytes
//! read, rather than holding the whole document in memory to compare
//! later. `FileConfigStore::write` follows the same technique (read ->
//! compare -> atomic temp-file-then-rename), reimplemented here rather than
//! called into from that CLI crate: `tellurion-core` sits *below*
//! `tellurion-ingest` in the workspace's dependency graph (drivers/binaries
//! depend on core, never the reverse), so the two can share a pattern but
//! never share code across that boundary.
//!
//! `write` validates the whole candidate document
//! ([`AppConfig::validate`](crate::config::AppConfig::validate)) before ever
//! touching disk — the same validate-then-swap contract `#47`'s reload
//! pipeline already applies to a file edited by hand, now also the gate a
//! programmatic caller (the control-lane mutation endpoint,
//! `tellurion-server::config_mutation`) goes through. A single in-process
//! [`std::sync::Mutex`] serializes every call to one `FileConfigStore`
//! instance's `write`: the version check-then-replace is otherwise a
//! classic TOCTOU window between two concurrent writers in the same
//! process (each could pass its own version check before either renames) —
//! the mutex collapses that race to "one writer proceeds at a time," so the
//! version check any writer performs is always accurate relative to what a
//! sibling call could have done concurrently. It does not protect against a
//! *second process* (or a human editor) racing this same file — that
//! residual window is the same one `ensure_source_unchanged`'s own
//! before-the-rename re-check already narrows for the operator CLI, and is
//! why `write` re-checks immediately before renaming too, rather than
//! relying on the mutex alone.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::config::AppConfig;
use crate::error::{Error, Result};
use crate::sigv4::sha256_hex;

/// An opaque token identifying one revision of a config document (`#110`).
/// Two tokens are equal exactly when they were derived from byte-identical
/// documents; callers must never parse, format for a human, or order by
/// this value — only compare it for equality, or hand it back unchanged to
/// a future compare-and-swap write. `Display` exists solely so a token can
/// appear in a log line or an HTTP response body without a caller having to
/// reach into this type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConfigVersion(String);

impl ConfigVersion {
    /// SHA-256 over `bytes`, hex-encoded — reuses `sigv4`'s own digest
    /// helper (already in this crate's dependency graph for SigV4 request
    /// signing) rather than adding a second hashing call site. Byte-
    /// identical input always produces the same token; any single-byte
    /// change (a comment, a trailing newline) produces a different one.
    /// `pub(crate)` (not private): `context.rs` reuses this to derive a
    /// fallback version for a `ContextState` built from an `AppConfig` that
    /// never went through a versioned read (every test in this workspace,
    /// which builds `AppConfig` straight from a YAML literal) — see that
    /// module's own `derive_config_version` doc.
    pub(crate) fn from_bytes(bytes: &[u8]) -> Self {
        Self(sha256_hex(bytes))
    }

    /// Reconstructs a token from its own [`Display`](std::fmt::Display)
    /// rendering (`#110`) — how a client's own copy of a previously issued
    /// version (an HTTP header, a JSON field) becomes a real token again
    /// for a subsequent [`ConfigStore::write`] call. Deliberately not
    /// `FromStr` with any validation of shape (hex, length, ...): a
    /// token's equality is defined entirely by whichever `ConfigStore`
    /// produced it, not by this type, so accepting any string here and
    /// letting `write` decide whether it matches its current one is the
    /// right contract — a caller quoting the WRONG value back just gets
    /// the same [`Error::VersionConflict`] any other mismatch produces,
    /// never a distinct "malformed token" error that would leak
    /// backend-specific format details.
    pub fn from_wire(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// A `u64` fingerprint of this token (`#110`), for use as a Prometheus
    /// gauge value (`tellurion-server::metrics`'s per-instance
    /// config-version gauge): the first 12 hex characters (48 bits) of the
    /// digest, parsed as a plain integer. Deliberately not the full hex
    /// string exposed as a metric *label* — a fresh label value on every
    /// reload would leave the previous reload's time series registered for
    /// the rest of the process's life, an unbounded cardinality leak over a
    /// long-running server's many reloads. A single, label-free gauge whose
    /// numeric VALUE changes on each reload has no such cost. 48 bits keeps
    /// collision risk astronomically low for this purpose (detecting
    /// whether two instances converged to the same config) while fitting
    /// an `f64` gauge exactly, with no precision loss (an `f64` mantissa
    /// holds 53 bits).
    pub fn fingerprint(&self) -> u64 {
        let prefix = &self.0[..self.0.len().min(12)];
        u64::from_str_radix(prefix, 16).unwrap_or(0)
    }
}

impl std::fmt::Display for ConfigVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The result of a versioned config read (`#110`): the parsed, validated
/// document plus the opaque token identifying the exact bytes it was
/// parsed from.
#[derive(Debug, Clone)]
pub struct VersionedConfig {
    pub config: AppConfig,
    pub version: ConfigVersion,
}

pub trait ConfigStore: Send + Sync {
    fn load(&self) -> Result<AppConfig>;

    /// Same document [`load`](Self::load) returns, paired with the opaque
    /// [`ConfigVersion`] identifying this exact revision (`#110`). Every
    /// `ConfigStore` implementation is responsible for a token whose
    /// equality tracks the document's content — see `FileConfigStore`'s own
    /// implementation for how the file backend does it.
    fn load_versioned(&self) -> Result<VersionedConfig>;

    /// Compare-and-swap write (`#110`): validates `config` as a whole
    /// document (the same [`AppConfig::validate`](crate::config::AppConfig::validate)
    /// boot/reload already runs — an invalid document is refused, named,
    /// and never reaches disk) and, only if `expected` still matches the
    /// store's current version, persists it and returns the new version.
    /// A mismatched `expected` is [`Error::VersionConflict`] — a concurrent
    /// writer already moved the document on, never a silent lost update.
    /// Implementations must perform the version check and the persist as
    /// one atomic unit from an external reader's point of view: a reader
    /// must never observe a partially-written document, and two concurrent
    /// writers against the same starting version must never both succeed.
    fn write(&self, expected: &ConfigVersion, config: &AppConfig) -> Result<ConfigVersion>;
}

pub struct FileConfigStore {
    path: PathBuf,
    /// Serializes every `write` call against this instance — see this
    /// module's own doc for why a single in-process lock is the right
    /// amount of protection here (and what it does *not* protect against).
    write_lock: Mutex<()>,
}

impl FileConfigStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            write_lock: Mutex::new(()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads the file's current raw bytes and derives its
    /// [`ConfigVersion`] — the comparison [`write`](Self::write) checks
    /// `expected` against, both up front and again immediately before the
    /// rename. Split out of `write` itself so both checks share one
    /// implementation.
    fn current_version(&self) -> Result<ConfigVersion> {
        let raw = std::fs::read(&self.path).map_err(|source| {
            Error::Config(format!("reading '{}': {source}", self.path.display()))
        })?;
        Ok(ConfigVersion::from_bytes(&raw))
    }

    fn ensure_version_matches(&self, expected: &ConfigVersion) -> Result<()> {
        let current = self.current_version()?;
        if current != *expected {
            return Err(Error::VersionConflict {
                expected: expected.to_string(),
                current: current.to_string(),
            });
        }
        Ok(())
    }
}

impl ConfigStore for FileConfigStore {
    fn load(&self) -> Result<AppConfig> {
        let contents = std::fs::read_to_string(&self.path).map_err(|source| {
            Error::Config(format!("reading '{}': {source}", self.path.display()))
        })?;
        let config: AppConfig = serde_yaml::from_str(&contents).map_err(|source| {
            Error::Config(format!("parsing '{}': {source}", self.path.display()))
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Reads the file's raw bytes once (to derive the version token) and
    /// delegates the parse-and-validate half entirely to
    /// [`load`](Self::load) — a second, independent read rather than a
    /// refactor of `load` itself, so this method can never change `load`'s
    /// own behavior or error text. Two reads of a small config document is
    /// not a hot-path cost: this is a control-lane operation, never called
    /// per request (see this module's own doc).
    fn load_versioned(&self) -> Result<VersionedConfig> {
        let raw = std::fs::read(&self.path).map_err(|source| {
            Error::Config(format!("reading '{}': {source}", self.path.display()))
        })?;
        let version = ConfigVersion::from_bytes(&raw);
        let config = self.load()?;
        Ok(VersionedConfig { config, version })
    }

    /// Validates `config` first (so an invalid document never touches disk
    /// at all — the same order `load` already implies: no `AppConfig` this
    /// module ever hands out or accepts is unvalidated), then serializes it
    /// to YAML and swaps it in over the atomic-replace technique this
    /// module's own doc describes: one rolling backup (`<path>.bak`,
    /// overwritten every call — not timestamped, so a long-running
    /// server's many mutations never accumulate an unbounded pile of
    /// backup files the way the operator CLI's one-shot, human-triggered
    /// tool can afford to), a uniquely-named temporary file fsynced before
    /// the rename, a second version check immediately before the rename to
    /// narrow the window a concurrent writer could exploit, and a final
    /// fsync of the containing directory so the rename itself is durable.
    fn write(&self, expected: &ConfigVersion, config: &AppConfig) -> Result<ConfigVersion> {
        config.validate()?;
        let serialized = serde_yaml::to_string(config)
            .map_err(|source| Error::Config(format!("serializing new configuration: {source}")))?;

        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        self.ensure_version_matches(expected)?;

        let metadata = std::fs::metadata(&self.path).map_err(|source| {
            Error::Config(format!(
                "reading existing config metadata '{}': {source}",
                self.path.display()
            ))
        })?;

        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let filename = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                Error::Config(format!(
                    "config path '{}' has no valid filename",
                    self.path.display()
                ))
            })?;

        // One rolling backup, not a timestamped one — see this method's
        // own doc for why boundedness rules out the operator CLI's
        // keep-every-revision approach for a live, repeatedly-mutated
        // server.
        let backup = parent.join(format!("{filename}.bak"));
        std::fs::copy(&self.path, &backup).map_err(|source| {
            Error::Config(format!("creating backup '{}': {source}", backup.display()))
        })?;

        let unique = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(".{filename}.{unique}.tmp"));
        let write_result: Result<()> = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|source| {
                    Error::Config(format!(
                        "creating temporary config '{}': {source}",
                        temporary.display()
                    ))
                })?;
            file.write_all(serialized.as_bytes()).map_err(|source| {
                Error::Config(format!(
                    "writing temporary config '{}': {source}",
                    temporary.display()
                ))
            })?;
            file.set_permissions(metadata.permissions())
                .map_err(|source| {
                    Error::Config(format!(
                        "preserving permissions on '{}': {source}",
                        temporary.display()
                    ))
                })?;
            file.sync_all().map_err(|source| {
                Error::Config(format!(
                    "syncing temporary config '{}': {source}",
                    temporary.display()
                ))
            })?;
            // Narrows (never eliminates — see this module's own doc) the
            // window a concurrent writer racing this same file could land
            // in between the check above and this rename.
            self.ensure_version_matches(expected)?;
            std::fs::rename(&temporary, &self.path).map_err(|source| {
                Error::Config(format!(
                    "replacing config '{}': {source}",
                    self.path.display()
                ))
            })?;
            if let Ok(dir) = std::fs::File::open(parent) {
                let _ = dir.sync_all();
            }
            Ok(())
        })();
        if write_result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        write_result?;

        Ok(ConfigVersion::from_bytes(serialized.as_bytes()))
    }
}

/// Per-process uniqueness for [`FileConfigStore::write`]'s temporary
/// filename — the same "an atomic counter can never collide, regardless of
/// timing" fix this module's own tests already apply to their temp-file
/// naming (see `write_temp_yaml`'s doc below) rather than a wall-clock
/// timestamp, which can coincide under two writes issued close together.
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal, valid document shared by every test below that only
    /// needs *some* parseable config, not a specific shape — keeps the
    /// versioned-read tests (`#110`) from each repeating the same YAML.
    const VALID_CONFIG: &str = r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
"#;

    #[test]
    fn loads_and_validates_a_file() {
        let path = write_temp_yaml(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
"#,
        );
        let store = FileConfigStore::new(&path);
        let config = store.load().unwrap();
        assert_eq!(config.collections.len(), 1);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn missing_file_is_a_config_error() {
        let store = FileConfigStore::new("/nonexistent/path/tellurion.yaml");
        assert!(matches!(store.load(), Err(Error::Config(_))));
    }

    #[test]
    fn invalid_yaml_is_a_config_error() {
        let path = write_temp_yaml("not: [valid: yaml: at: all");
        let store = FileConfigStore::new(&path);
        assert!(matches!(store.load(), Err(Error::Config(_))));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn referential_error_surfaces_from_load() {
        let path = write_temp_yaml(
            r#"
collections:
  - id: demo
    catalog: missing
    storage: missing
    table: demo
    geometry: geom
    pk: id
"#,
        );
        let store = FileConfigStore::new(&path);
        assert!(matches!(store.load(), Err(Error::Config(_))));
        let _ = std::fs::remove_file(path);
    }

    // -- versioned read (`#110`) ---------------------------------------

    /// Same document, read twice: the token must compare equal both times —
    /// a caller diffing two reads of an unchanged file must see "nothing
    /// changed," not two tokens that happen to differ for reasons unrelated
    /// to content (a timestamp, a process id, ...).
    #[test]
    fn load_versioned_yields_the_same_token_for_the_same_document() {
        let path = write_temp_yaml(VALID_CONFIG);
        let store = FileConfigStore::new(&path);

        let first = store.load_versioned().unwrap();
        let second = store.load_versioned().unwrap();
        assert_eq!(first.version, second.version);
        let _ = std::fs::remove_file(path);
    }

    /// A byte-level change to the document — even one that leaves the
    /// parsed `AppConfig` semantically similar — must change the token, so
    /// a future compare-and-swap write can never mistake a changed file for
    /// an unchanged one.
    #[test]
    fn load_versioned_yields_a_different_token_after_the_document_changes() {
        let path = write_temp_yaml(VALID_CONFIG);
        let store = FileConfigStore::new(&path);
        let before = store.load_versioned().unwrap();

        std::fs::write(
            &path,
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
  - id: demo2
    catalog: default
    storage: main
    table: demo2
    geometry: geom
    pk: id
"#,
        )
        .unwrap();
        let after = store.load_versioned().unwrap();

        assert_ne!(before.version, after.version);
        let _ = std::fs::remove_file(path);
    }

    /// `load_versioned`'s `config` half must be identical to what plain
    /// `load` returns for the same document — the version token is an
    /// addition, never a second, possibly-divergent read path.
    #[test]
    fn load_versioned_config_matches_plain_load() {
        let path = write_temp_yaml(VALID_CONFIG);
        let store = FileConfigStore::new(&path);

        let versioned = store.load_versioned().unwrap();
        let plain = store.load().unwrap();
        assert_eq!(versioned.config.collections.len(), plain.collections.len());
        assert_eq!(versioned.config.collections[0].id, plain.collections[0].id);
        let _ = std::fs::remove_file(path);
    }

    /// The token is opaque: nothing outside this module can construct one
    /// from raw material or read its bytes back out — the type offers only
    /// equality and `Display`. This test documents that contract by relying
    /// on it rather than asserting on the token's internal shape (there is
    /// no public accessor to assert on).
    #[test]
    fn load_versioned_token_is_opaque_but_stringifiable_for_logging() {
        let path = write_temp_yaml(VALID_CONFIG);
        let store = FileConfigStore::new(&path);

        let versioned = store.load_versioned().unwrap();
        // `Display` is the only way to observe a token's contents, and
        // exists solely for a log line or a response body — never parsed
        // back or compared to anything but another `ConfigVersion`.
        let rendered = versioned.version.to_string();
        assert!(!rendered.is_empty());
        let _ = std::fs::remove_file(path);
    }

    /// A missing file fails `load_versioned` the same named way it fails
    /// plain `load` — the version half never masks a read error.
    #[test]
    fn load_versioned_missing_file_is_a_config_error() {
        let store = FileConfigStore::new("/nonexistent/path/tellurion.yaml");
        assert!(matches!(store.load_versioned(), Err(Error::Config(_))));
    }

    /// Builds a path under the OS temp dir that is unique to this test
    /// binary process and this call, without reading the wall clock. The
    /// previous implementation suffixed the path with a `SystemTime`
    /// nanosecond reading; under heavy host load — many test threads
    /// launching close together, coarser effective clock resolution under
    /// scheduler pressure — two of this module's tests could occasionally
    /// land on the same path, so one test's `write`/`remove_file` raced the
    /// other's `read_to_string`. A per-process atomic counter can never
    /// collide, regardless of timing, which removes the flake outright
    /// instead of just making it less likely.
    fn write_temp_yaml(contents: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let mut path = std::env::temp_dir();
        path.push(format!(
            "tellurion-core-test-{}-{unique}.yaml",
            std::process::id()
        ));
        std::fs::write(&path, contents).unwrap();
        path
    }

    // -- compare-and-swap write (`#110`) -------------------------------

    const OTHER_VALID_CONFIG: &str = r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
  - id: demo2
    catalog: default
    storage: main
    table: demo2
    geometry: geom
    pk: id
"#;

    fn parsed(yaml: &str) -> AppConfig {
        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate().unwrap();
        config
    }

    /// A write against the current version succeeds, persists the new
    /// document, and returns a version that matches what a subsequent
    /// `load_versioned` reports — the store's own token is never a second,
    /// possibly-divergent bookkeeping value.
    #[test]
    fn write_with_the_current_version_succeeds_and_persists() {
        let path = write_temp_yaml(VALID_CONFIG);
        let store = FileConfigStore::new(&path);
        let before = store.load_versioned().unwrap();

        let new_version = store
            .write(&before.version, &parsed(OTHER_VALID_CONFIG))
            .unwrap();

        let after = store.load_versioned().unwrap();
        assert_eq!(after.version, new_version);
        assert_ne!(after.version, before.version);
        assert_eq!(after.config.collections.len(), 2);
        let _ = std::fs::remove_file(path.with_extension("yaml.bak"));
        let _ = std::fs::remove_file(path);
    }

    /// A write against a stale version is refused as `VersionConflict`
    /// naming both the expected and the actual current version, and the
    /// file on disk is left completely unchanged — never a lost update.
    #[test]
    fn write_with_a_stale_version_is_refused_as_a_named_conflict() {
        let path = write_temp_yaml(VALID_CONFIG);
        let store = FileConfigStore::new(&path);
        let stale = store.load_versioned().unwrap().version;

        // Someone else's write lands first.
        let first = store
            .write(&stale, &parsed(OTHER_VALID_CONFIG))
            .expect("the first writer, using the still-current version, succeeds");

        // Retrying with the now-stale version must be refused, never
        // silently applied over the first writer's change.
        let result = store.write(&stale, &parsed(VALID_CONFIG));
        match result {
            Err(Error::VersionConflict { expected, current }) => {
                assert_eq!(expected, stale.to_string());
                assert_eq!(current, first.to_string());
            }
            other => panic!("expected Err(VersionConflict {{ .. }}), got {other:?}"),
        }

        let after = store.load_versioned().unwrap();
        assert_eq!(
            after.version, first,
            "the rejected write must not have touched the file"
        );
        let _ = std::fs::remove_file(path.with_extension("yaml.bak"));
        let _ = std::fs::remove_file(path);
    }

    /// An invalid candidate document is refused before it ever reaches
    /// disk — the file keeps its previous content and version, exactly the
    /// `#47` "a bad edit is refused by name, the old config keeps serving"
    /// contract, now also enforced on the programmatic write path.
    #[test]
    fn write_of_an_invalid_document_never_touches_disk() {
        let path = write_temp_yaml(VALID_CONFIG);
        let store = FileConfigStore::new(&path);
        let before = store.load_versioned().unwrap();

        // Referentially broken: references a catalog nothing declares.
        let invalid: AppConfig = serde_yaml::from_str(
            r#"
collections:
  - id: broken
    catalog: nonexistent
    storage: nonexistent
    table: demo
    geometry: geom
    pk: id
"#,
        )
        .unwrap();

        let result = store.write(&before.version, &invalid);
        assert!(matches!(result, Err(Error::Config(_))));

        let after = store.load_versioned().unwrap();
        assert_eq!(
            after.version, before.version,
            "an invalid write must leave the file byte-for-byte unchanged"
        );
        assert_eq!(
            after.config.collections.len(),
            before.config.collections.len()
        );
        let _ = std::fs::remove_file(path);
    }

    /// `#110`: exactly one rolling backup file, overwritten on every
    /// successful write — never a timestamped pile that would grow without
    /// bound over a long-running server's many mutations.
    #[test]
    fn write_maintains_exactly_one_rolling_backup() {
        let path = write_temp_yaml(VALID_CONFIG);
        let store = FileConfigStore::new(&path);
        let backup = path.with_extension("yaml.bak");

        let v1 = store.load_versioned().unwrap().version;
        store.write(&v1, &parsed(OTHER_VALID_CONFIG)).unwrap();
        assert!(
            backup.exists(),
            "a backup should exist after the first write"
        );
        let backup_contents_after_first = std::fs::read_to_string(&backup).unwrap();

        let v2 = store.load_versioned().unwrap().version;
        store.write(&v2, &parsed(VALID_CONFIG)).unwrap();
        let backup_contents_after_second = std::fs::read_to_string(&backup).unwrap();
        assert_ne!(
            backup_contents_after_first, backup_contents_after_second,
            "the single backup file should roll to reflect the most recent prior revision"
        );

        let _ = std::fs::remove_file(backup);
        let _ = std::fs::remove_file(path);
    }

    /// A token round-tripped through [`ConfigVersion::from_wire`] compares
    /// equal to the original — the mutation endpoint's own use: a client
    /// hands back a version it read earlier, verbatim, and it must still
    /// match the store's current one.
    #[test]
    fn from_wire_round_trips_a_previously_issued_token() {
        let path = write_temp_yaml(VALID_CONFIG);
        let store = FileConfigStore::new(&path);
        let issued = store.load_versioned().unwrap().version;

        let rebuilt = ConfigVersion::from_wire(issued.to_string());
        assert_eq!(rebuilt, issued);
        let _ = std::fs::remove_file(path);
    }

    /// The fingerprint is a deterministic function of content: the same
    /// document always yields the same `u64`, and a changed document
    /// yields a different one — the property the config-version gauge
    /// depends on.
    #[test]
    fn fingerprint_is_deterministic_and_content_sensitive() {
        let a = ConfigVersion::from_bytes(b"one document");
        let b = ConfigVersion::from_bytes(b"one document");
        let c = ConfigVersion::from_bytes(b"a different document");
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_ne!(a.fingerprint(), c.fingerprint());
    }
}
