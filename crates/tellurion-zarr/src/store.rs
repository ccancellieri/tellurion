//! Store-access seam for a Zarr v2 array: fetching a metadata document
//! (`.zarray`/`.zattrs`/`.zgroup`) and fetching a chunk by its on-disk/
//! on-wire key, abstracted behind [`ZarrStore`] so `reader.rs`'s parsing and
//! window-assembly logic never branches on local vs. remote itself — the
//! same role `tellurion-cog::reader::CogSource` plays for that crate,
//! adapted to this crate's own access pattern: a Zarr chunk is always
//! fetched *whole*, never ranged — the chunk itself (not some larger
//! container file) is the atomic on-wire unit, unlike a COG's byte-range-
//! addressable tile stream inside one large TIFF — so this seam is "fetch
//! this whole document/chunk by name," not `Read + Seek`.
//!
//! [`FsStore`] is this driver's original (`#37` first slice) local-directory
//! behavior, pulled out of `reader.rs` unchanged. [`RemoteZarrSource`] is the
//! new `http(s)` implementation (`#37` follow-up): built once by
//! `ZarrDriverFactory::build` (`driver.rs`) from the storage's configured
//! locator, and read entirely through whole-object `GET` requests.
//! [`ScopedStore`] (`#37` overview/pyramid follow-up) is neither a new
//! transport nor a new store — it's a thin view that roots every
//! document/chunk name onto one subdirectory of an existing store, so
//! `reader.rs` can read a `multiscales` pyramid's per-level `.zarray`/chunk
//! files (each level lives in its own directory, e.g. `"0"`, `"1"`, relative
//! to the group root) through the exact same `read_window`/`open`-adjacent
//! code a plain single-array store already uses, without either of those
//! ever branching on "is this a pyramid level or the store's own root."
//!
//! A missing
//! key (HTTP `404`) maps to `Ok(None)` — this trait's own "does not exist"
//! signal, which `reader::open` treats as a structural refusal for a
//! metadata document and `reader::read_window` treats as `fill_value` for a
//! chunk (a missing chunk file and a `404` are the same fact under the Zarr
//! v2 spec: an unwritten chunk). Any other non-2xx status, or a transport
//! failure, is a named [`ZarrError::RemoteOpen`] — never silently treated as
//! a missing chunk, which would fabricate fill-value data for what might be
//! a real, merely-unreachable one.
//!
//! Built on the *async* `reqwest::Client`, bridged to this trait's
//! synchronous methods via `tokio::runtime::Handle::block_on` — the exact
//! choice `tellurion-cog::remote`'s own module doc explains and justifies
//! (never `reqwest::blocking`, which owns a second, nested Tokio runtime
//! that panics on drop from inside this workspace's own async runtime, which
//! a config-reload-swapped backend or plain process shutdown always is).
//! Every call here must run from inside the `tokio::task::spawn_blocking`
//! context `reader::open`/`reader::read_window` already require
//! (`driver.rs`'s own `spawn_blocking` usage) — that's where
//! `Handle::current()` finds the runtime it bridges into.

use std::io;
use std::path::PathBuf;

use reqwest::{Client, Response, StatusCode, Url};
use tokio::runtime::Handle;

use crate::error::{Result, ZarrError};

/// Cap on a metadata document (`.zarray`/`.zattrs`/`.zgroup`) fetched over
/// HTTP. These are always a handful of small JSON files in real stores —
/// generous enough that none ever approaches this, small enough that a
/// misbehaving remote can never make this driver buffer an unbounded
/// response body just opening a store.
const METADATA_CAP_BYTES: u64 = 1024 * 1024;

/// Where this driver reads a Zarr v2 store's document/chunk bytes from — a
/// local directory ([`FsStore`]), or a remote `http(s)` object tree
/// ([`RemoteZarrSource`]) read entirely through whole-object `GET` requests.
/// Built once by `ZarrDriverFactory::build` (`driver.rs`) from the storage's
/// configured locator string; carried from there (behind an `Arc`) into
/// every `reader::open`/`reader::read_window` call this collection's
/// lifetime makes.
pub trait ZarrStore: Send + Sync {
    /// Reads a metadata document (`.zarray`, `.zattrs`, or `.zgroup`) whole.
    /// `Ok(None)` means the document does not exist — `reader::open` decides
    /// what that means (a genuinely missing store, a `.zgroup` hierarchy
    /// that declares no `multiscales` pyramid this driver could serve
    /// either, or a store with no declared georeferencing); any other `Err`
    /// is a real failure to reach or read the document, never conflated with
    /// "does not exist."
    fn read_metadata(&self, name: &str) -> Result<Option<Vec<u8>>>;

    /// Reads chunk `key` whole, refusing (never silently truncating) if the
    /// raw bytes fetched exceed `cap_bytes` — the caller
    /// (`reader::read_window`) derives that cap from the chunk's own
    /// already-budgeted decompressed size (`reader::chunk_raw_byte_cap`),
    /// the same "bound relative to a size already committed to, not an
    /// independent flat ceiling" idiom `metadata::MAX_CHUNK_ELEMENTS`
    /// applies to a chunk's decompressed element count. `Ok(None)` means the
    /// chunk does not exist — a legitimate, unwritten Zarr v2 chunk under
    /// the spec, which `reader::read_window` already treats as `fill_value`.
    fn read_chunk(&self, key: &str, cap_bytes: u64) -> Result<Option<Vec<u8>>>;

    /// A human-readable label for this store (a local path or a remote base
    /// URL) — used only in refusal messages, so an operator can tell which
    /// store a boot-time refusal came from.
    fn describe(&self) -> String;

    /// The physical collection name `reader::open` reports to
    /// `CatalogSource` for this store — a local directory's own final path
    /// component, or a remote locator's own last non-empty path segment
    /// (both "no embedded logical dataset name to prefer over it," the same
    /// fallback `tellurion-cog`'s own `logical_name_of` uses for either kind
    /// of GeoTIFF source).
    fn logical_name(&self) -> String;
}

/// This driver's original local-directory store (`#37` first slice) — reads
/// every document/chunk straight off disk, never capped by the `cap_bytes`
/// [`ZarrStore::read_chunk`] passes: a local file's size is already bounded
/// by the disk it lives on, not a remote attack surface, so this keeps this
/// driver's pre-HTTP local behavior exactly unchanged.
pub struct FsStore {
    root: PathBuf,
}

impl FsStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

fn read_local(path: &std::path::Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ZarrError::Open {
            path: path.display().to_string(),
            source,
        }),
    }
}

impl ZarrStore for FsStore {
    fn read_metadata(&self, name: &str) -> Result<Option<Vec<u8>>> {
        read_local(&self.root.join(name))
    }

    fn read_chunk(&self, key: &str, _cap_bytes: u64) -> Result<Option<Vec<u8>>> {
        read_local(&self.root.join(key))
    }

    fn describe(&self) -> String {
        self.root.display().to_string()
    }

    fn logical_name(&self) -> String {
        self.root
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| "zarr".to_string())
    }
}

/// One remote Zarr v2 store's connection details — built once by
/// `ZarrDriverFactory::build` (`driver.rs`) from the storage's configured
/// `http(s)://` locator, and cloned into every request this collection ever
/// makes (`Client` is internally `Arc`-backed, so cloning shares one
/// connection pool rather than dialing fresh each time). `base_url` always
/// carries a trailing `/` (enforced by `driver.rs::parse_source` before this
/// is built) so joining a document/chunk name onto it never drops the
/// locator's own final path segment — `Url::join`'s ordinary relative-
/// reference resolution rule (RFC 3986) would otherwise treat the locator's
/// last segment as a filename to replace, not a directory to read inside.
#[derive(Clone, Debug)]
pub struct RemoteZarrSource {
    pub client: Client,
    pub base_url: Url,
}

impl ZarrStore for RemoteZarrSource {
    fn read_metadata(&self, name: &str) -> Result<Option<Vec<u8>>> {
        self.fetch(name, METADATA_CAP_BYTES)
    }

    fn read_chunk(&self, key: &str, cap_bytes: u64) -> Result<Option<Vec<u8>>> {
        self.fetch(key, cap_bytes)
    }

    fn describe(&self) -> String {
        self.base_url.to_string()
    }

    fn logical_name(&self) -> String {
        self.base_url
            .path_segments()
            .into_iter()
            .flatten()
            .rfind(|segment| !segment.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| "zarr".to_string())
    }
}

impl RemoteZarrSource {
    /// Fetches `relative` (a document or chunk name/key, joined onto
    /// `base_url`) with a plain whole-object `GET` — never a ranged request
    /// (this module's own doc explains why a Zarr chunk needs none). Must
    /// run inside a `tokio::task::spawn_blocking` context (this module's own
    /// doc) — that's where [`Handle::current`] finds the runtime it bridges
    /// into.
    fn fetch(&self, relative: &str, cap_bytes: u64) -> Result<Option<Vec<u8>>> {
        let handle = Handle::current();
        let url = self
            .base_url
            .join(relative)
            .map_err(|error| ZarrError::RemoteOpen {
                url: format!("{}{relative}", self.base_url),
                message: format!("could not build a request URL: {error}"),
            })?;
        handle.block_on(async {
            let response = self.client.get(url.clone()).send().await.map_err(|error| {
                ZarrError::RemoteOpen {
                    url: url.to_string(),
                    message: error.to_string(),
                }
            })?;
            match response.status() {
                StatusCode::OK => {
                    let bytes = read_capped_body(response, cap_bytes)
                        .await
                        .map_err(|message| ZarrError::RemoteOpen {
                            url: url.to_string(),
                            message,
                        })?;
                    Ok(Some(bytes))
                }
                StatusCode::NOT_FOUND => Ok(None),
                status => Err(ZarrError::RemoteOpen {
                    url: url.to_string(),
                    message: format!("unexpected HTTP status {status}"),
                }),
            }
        })
    }
}

/// Reads `response`'s body, refusing with a named error (never silently
/// truncating) the moment it would exceed `cap_bytes` — this crate's own
/// gzip/zlib decompression bomb guard (`reader::decompress`) applies the
/// same "cap first, refuse rather than balloon" idiom to a chunk's
/// DECOMPRESSED size; this is that idiom applied to the RAW bytes fetched
/// over the wire, before decompression even starts, so a misbehaving or
/// malicious remote can never make this crate buffer an unbounded response
/// body in the first place. Stops reading (never buffers past the breach) so
/// a body that would exceed the cap is never held in memory whole.
async fn read_capped_body(
    mut response: Response,
    cap_bytes: u64,
) -> std::result::Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        buf.extend_from_slice(&chunk);
        if buf.len() as u64 > cap_bytes {
            return Err(format!(
                "response body exceeded this fetch's {cap_bytes}-byte budget"
            ));
        }
    }
    Ok(buf)
}

/// A view over `inner`, rooted at one `multiscales` pyramid level's own
/// subdirectory (`level_path`, e.g. `"0"`, relative to the group root) —
/// see this module's own doc for why this exists. An empty `level_path`
/// (a plain, non-pyramid single-array store, whose `.zarray`/chunks already
/// live at the store's own root) makes every name pass through unchanged, so
/// `reader::open`/`read_window` can wrap in this unconditionally rather than
/// branching on "is there a pyramid" themselves.
pub(crate) struct ScopedStore<'a> {
    inner: &'a dyn ZarrStore,
    prefix: String,
}

impl<'a> ScopedStore<'a> {
    pub(crate) fn new(inner: &'a dyn ZarrStore, level_path: &str) -> Self {
        let prefix = if level_path.is_empty() {
            String::new()
        } else {
            format!("{level_path}/")
        };
        Self { inner, prefix }
    }

    fn joined(&self, name: &str) -> String {
        format!("{}{name}", self.prefix)
    }
}

impl ZarrStore for ScopedStore<'_> {
    fn read_metadata(&self, name: &str) -> Result<Option<Vec<u8>>> {
        self.inner.read_metadata(&self.joined(name))
    }

    fn read_chunk(&self, key: &str, cap_bytes: u64) -> Result<Option<Vec<u8>>> {
        self.inner.read_chunk(&self.joined(key), cap_bytes)
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn logical_name(&self) -> String {
        self.inner.logical_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RecordingStore {
        seen: std::sync::Mutex<Vec<String>>,
    }

    impl ZarrStore for RecordingStore {
        fn read_metadata(&self, name: &str) -> Result<Option<Vec<u8>>> {
            self.seen.lock().unwrap().push(name.to_string());
            Ok(None)
        }

        fn read_chunk(&self, key: &str, _cap_bytes: u64) -> Result<Option<Vec<u8>>> {
            self.seen.lock().unwrap().push(key.to_string());
            Ok(None)
        }

        fn describe(&self) -> String {
            "recording".to_string()
        }

        fn logical_name(&self) -> String {
            "recording".to_string()
        }
    }

    #[test]
    fn an_empty_level_path_leaves_names_unchanged() {
        let inner = RecordingStore {
            seen: std::sync::Mutex::new(Vec::new()),
        };
        let scoped = ScopedStore::new(&inner, "");
        scoped.read_metadata(".zarray").unwrap();
        scoped.read_chunk("0.0", 1024).unwrap();
        assert_eq!(inner.seen.into_inner().unwrap(), vec![".zarray", "0.0"]);
    }

    #[test]
    fn a_level_path_is_joined_as_a_directory_prefix() {
        let inner = RecordingStore {
            seen: std::sync::Mutex::new(Vec::new()),
        };
        let scoped = ScopedStore::new(&inner, "1");
        scoped.read_metadata(".zarray").unwrap();
        scoped.read_chunk("0.0", 1024).unwrap();
        assert_eq!(inner.seen.into_inner().unwrap(), vec!["1/.zarray", "1/0.0"]);
    }
}
