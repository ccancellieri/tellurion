//! Test-only Zarr v2 fixture stores, built entirely by hand in a private
//! temp directory — no network, no committed binary fixture, mirroring
//! `tellurion-cog::test_support`'s own role for that crate's tests. Used by
//! `driver.rs`'s and `reader.rs`'s own test modules.
//!
//! [`MockDirServer`] additionally serves a [`FixtureStore`]'s own directory
//! over loopback HTTP (`#37` remote-store follow-up) — a tiny hand-rolled
//! HTTP/1.1 server, the same "no second HTTP server crate in this crate's
//! own dev-dependencies" choice `tellurion-cog::test_support::MockServer`
//! makes, adapted to this crate's own store shape: a whole directory of
//! small documents/chunks, dispatched by request path, rather than one blob
//! served with `Range` semantics. `error_paths` lets a test make a specific
//! relative path answer `500` regardless of what's on disk, so a test can
//! prove this driver's "any other non-2xx is a named error, never a missing-
//! chunk fill value" contract without needing the fixture itself to be
//! broken.

use std::io::{BufRead, BufReader, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::error::Result;
use crate::store::ZarrStore;

/// A private, self-cleaning temp directory holding one hand-built Zarr v2
/// array store.
pub struct FixtureStore {
    dir: TempDir,
}

impl FixtureStore {
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// A plain 8x8, single-band, `u8`, raw (uncompressed) array chunked
    /// 4x4 (four chunks), every sample the constant `100`. Declares
    /// `tellurion:extent_crs84 = [-2, -2, 2, 2]` and no `fixed_index` (a
    /// plain 2D array needs none).
    pub fn plain_2d() -> Self {
        let dir = TempDir::new("plain-2d");
        write_zarray(
            dir.path(),
            r#"{"zarr_format":2,"shape":[8,8],"chunks":[4,4],"dtype":"|u1","compressor":null,"fill_value":0,"order":"C"}"#,
        );
        write_zattrs(
            dir.path(),
            r#"{"tellurion:extent_crs84":[-2.0,-2.0,2.0,2.0]}"#,
        );
        for chunk_y in 0..2u64 {
            for chunk_x in 0..2u64 {
                write_chunk(dir.path(), &format!("{chunk_y}.{chunk_x}"), &[100u8; 16]);
            }
        }
        Self { dir }
    }

    /// The same shape/extent as [`plain_2d`](Self::plain_2d), but with no
    /// `.zattrs` at all — this driver must refuse to guess a store's
    /// georeferencing rather than serve a default extent.
    pub fn missing_georef() -> Self {
        let dir = TempDir::new("missing-georef");
        write_zarray(
            dir.path(),
            r#"{"zarr_format":2,"shape":[8,8],"chunks":[4,4],"dtype":"|u1","compressor":null,"fill_value":0,"order":"C"}"#,
        );
        Self { dir }
    }

    /// A 3D array — leading `time` dimension of length 2 (chunked 1, so each
    /// time step is its own chunk), trailing 8x8 `(y, x)` (chunked 4x4).
    /// Time step 0 is the constant `50`, time step 1 the constant `200`.
    /// `tellurion:fixed_index = [1]` selects time step 1.
    pub fn with_leading_time_dimension() -> Self {
        let dir = TempDir::new("leading-time-dim");
        write_zarray(
            dir.path(),
            r#"{"zarr_format":2,"shape":[2,8,8],"chunks":[1,4,4],"dtype":"|u1","compressor":null,"fill_value":0,"order":"C"}"#,
        );
        write_zattrs(
            dir.path(),
            r#"{"tellurion:extent_crs84":[-2.0,-2.0,2.0,2.0],"tellurion:fixed_index":[1]}"#,
        );
        for (time, value) in [(0u64, 50u8), (1u64, 200u8)] {
            for chunk_y in 0..2u64 {
                for chunk_x in 0..2u64 {
                    write_chunk(
                        dir.path(),
                        &format!("{time}.{chunk_y}.{chunk_x}"),
                        &[value; 16],
                    );
                }
            }
        }
        Self { dir }
    }

    /// A two-level OME-NGFF-shaped `multiscales` pyramid: dataset `"0"`
    /// (finest, native 8x8 resolution, chunked 4x4) is the constant `10`
    /// everywhere; dataset `"1"` (coarsest, 4x4, one whole 4x4 chunk) is the
    /// constant `200` everywhere. The two levels hold DIFFERENT constants,
    /// deliberately, rather than one being a real downsample of the other --
    /// a served tile's own color (and, more directly, which level's own
    /// chunk file actually got fetched -- see `RecordingStore`) proves WHICH
    /// level was read, not merely that "a" plausible-looking value came
    /// back. Declares the same `tellurion:extent_crs84 = [-2,-2,2,2]`
    /// [`plain_2d`](Self::plain_2d) does, at the group's own root `.zattrs`,
    /// alongside the OME-NGFF `multiscales` declaration; `.zgroup` marks
    /// this directory as a Zarr v2 group rather than a single array (see
    /// `reader`'s own "Multiscale pyramids" doc for why this shape, not a
    /// private one).
    pub fn pyramid_2d() -> Self {
        let dir = TempDir::new("pyramid-2d");
        std::fs::write(dir.path().join(".zgroup"), r#"{"zarr_format":2}"#).unwrap();
        write_zattrs(
            dir.path(),
            r#"{"tellurion:extent_crs84":[-2.0,-2.0,2.0,2.0],"multiscales":[{"version":"0.4","axes":[{"name":"y","type":"space"},{"name":"x","type":"space"}],"datasets":[{"path":"0","coordinateTransformations":[{"type":"scale","scale":[1.0,1.0]}]},{"path":"1","coordinateTransformations":[{"type":"scale","scale":[2.0,2.0]}]}]}]}"#,
        );

        let level0 = dir.path().join("0");
        std::fs::create_dir_all(&level0).unwrap();
        write_zarray(
            &level0,
            r#"{"zarr_format":2,"shape":[8,8],"chunks":[4,4],"dtype":"|u1","compressor":null,"fill_value":0,"order":"C"}"#,
        );
        for chunk_y in 0..2u64 {
            for chunk_x in 0..2u64 {
                write_chunk(&level0, &format!("{chunk_y}.{chunk_x}"), &[10u8; 16]);
            }
        }

        let level1 = dir.path().join("1");
        std::fs::create_dir_all(&level1).unwrap();
        write_zarray(
            &level1,
            r#"{"zarr_format":2,"shape":[4,4],"chunks":[4,4],"dtype":"|u1","compressor":null,"fill_value":0,"order":"C"}"#,
        );
        write_chunk(&level1, "0.0", &[200u8; 16]);

        Self { dir }
    }
}

fn write_zarray(root: &Path, json: &str) {
    std::fs::write(root.join(".zarray"), json).unwrap();
}

fn write_zattrs(root: &Path, json: &str) {
    std::fs::write(root.join(".zattrs"), json).unwrap();
}

fn write_chunk(root: &Path, key: &str, bytes: &[u8]) {
    let mut file = std::fs::File::create(root.join(key)).unwrap();
    file.write_all(bytes).unwrap();
}

struct TempDir {
    path: PathBuf,
}

/// Disambiguates two [`TempDir::new`] calls that land on the same wall-clock
/// tick — `std::process::id()` is identical across every thread in one test
/// binary, so under enough parallel test threads (this crate's own remote
/// tests, `#37` follow-up, made this materially likely rather than
/// theoretical) two same-labeled fixtures could otherwise collide on the
/// same directory name and race each other's `Drop::drop` cleanup mid-test.
static NEXT_TEMP_DIR_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl TempDir {
    fn new(label: &str) -> Self {
        let mut path = std::env::temp_dir();
        let unique = NEXT_TEMP_DIR_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        path.push(format!(
            "tellurion-zarr-fixture-{label}-{}-{}-{unique}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A `reqwest::Client` with proxy lookup disabled — every test using this
/// targets `127.0.0.1` already; this just keeps that hermetic regardless of
/// an `HTTP_PROXY`/`http_proxy` environment variable in whatever runs the
/// test suite. Same role `tellurion-cog::test_support::test_client` plays
/// for that crate's own remote tests.
pub(crate) fn test_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("builds a plain client with no TLS/proxy config to fail on")
}

/// Serves a [`FixtureStore`]'s (or any directory's) files over loopback
/// HTTP: `GET /name` answers `200` with `root.join(name)`'s bytes, or `404`
/// if that path doesn't exist — the same fact this driver's
/// `store::RemoteZarrSource` already maps to "chunk/document missing."
/// `error_paths` (relative, no leading `/`) always answer `500` instead,
/// regardless of what's on disk — this module's own doc explains why a test
/// needs that independent of the fixture's real file layout.
pub(crate) struct MockDirServer {
    addr: SocketAddr,
}

impl MockDirServer {
    pub(crate) fn serve(root: PathBuf, error_paths: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds a loopback port");
        let addr = listener
            .local_addr()
            .expect("bound listener has a local addr");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                handle_connection(stream, &root, &error_paths);
            }
        });
        Self { addr }
    }

    /// This server's own base URL, always carrying a trailing `/` — matches
    /// the shape `driver.rs::parse_source` normalizes every remote locator
    /// into before building a real [`crate::store::RemoteZarrSource`].
    pub(crate) fn base_url(&self) -> reqwest::Url {
        reqwest::Url::parse(&format!("http://{}/", self.addr)).expect("valid loopback URL")
    }
}

fn handle_connection(mut stream: TcpStream, root: &Path, error_paths: &[String]) {
    let mut reader = BufReader::new(stream.try_clone().expect("clones the loopback stream"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    // Drain the rest of the request's headers (this fixture never inspects
    // them) so a keep-alive-minded client doesn't hang waiting for them to
    // be read before the response arrives.
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        if line.trim_end().is_empty() {
            break;
        }
    }

    let Some(path) = request_line
        .split_whitespace()
        .nth(1)
        .map(|raw| raw.trim_start_matches('/').to_string())
    else {
        return;
    };

    let (status_line, body): (&str, Vec<u8>) = if error_paths.iter().any(|p| p == &path) {
        ("HTTP/1.1 500 Internal Server Error", Vec::new())
    } else {
        match std::fs::read(root.join(&path)) {
            Ok(bytes) => ("HTTP/1.1 200 OK", bytes),
            Err(_) => ("HTTP/1.1 404 Not Found", Vec::new()),
        }
    };

    let response = format!(
        "{status_line}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(&body);
}

/// A [`ZarrStore`] decorator that records every metadata document name and
/// chunk key it's asked for, in request order, before delegating the real
/// read to `inner`. Serving the right PIXEL VALUES from a pyramid fixture
/// only proves the right value came back -- it doesn't rule out a fixture
/// coincidence (e.g. every level happening to agree at the one pixel a test
/// checks). This is the stronger proof: it lets a test assert the exact
/// on-disk paths this driver actually opened, so "the coarse level was
/// selected" means "its own `.zarray`/chunk files were the ones fetched,"
/// not just "the output looked plausible."
pub(crate) struct RecordingStore {
    inner: Arc<dyn ZarrStore>,
    log: Mutex<Vec<String>>,
}

impl RecordingStore {
    pub(crate) fn wrap(inner: Arc<dyn ZarrStore>) -> Self {
        Self {
            inner,
            log: Mutex::new(Vec::new()),
        }
    }

    /// Every metadata/chunk name this store was asked to read, in request
    /// order — a plain single-array store's own names pass through
    /// unprefixed; a `multiscales` level's own names are prefixed with that
    /// level's own directory (`"0/..."`, `"1/..."`, see `ScopedStore`'s own
    /// doc), which is exactly the signal a test needs to tell which level
    /// was actually opened.
    pub(crate) fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

impl ZarrStore for RecordingStore {
    fn read_metadata(&self, name: &str) -> Result<Option<Vec<u8>>> {
        self.log.lock().unwrap().push(name.to_string());
        self.inner.read_metadata(name)
    }

    fn read_chunk(&self, key: &str, cap_bytes: u64) -> Result<Option<Vec<u8>>> {
        self.log.lock().unwrap().push(key.to_string());
        self.inner.read_chunk(key, cap_bytes)
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn logical_name(&self) -> String {
        self.inner.logical_name()
    }
}
