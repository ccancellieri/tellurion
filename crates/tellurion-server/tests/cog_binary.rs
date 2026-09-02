//! The `#37` acceptance proof: the real `tellurion` binary, built with the
//! bundled database driver compiled out (`--no-default-features --features
//! cog`), serves `/collections` and a PNG raster tile through the abstract
//! driver contract backed by nothing but a local, tiled GeoTIFF. Mirrors
//! `tellurion-pmtiles`' own `pmtiles_binary.rs` proof, adapted to the
//! raster-tiles lane.
//!
//! Run exactly the invocation this proves:
//!
//! ```sh
//! cargo test -p tellurion --no-default-features --features cog
//! cargo tree -p tellurion --no-default-features --features cog -e normal
//! ```
//!
//! The second command is the other half of the proof (checked separately,
//! not by this file): it must list `tellurion-cog`/`tiff` and no
//! `postgres`/`postgis`/`deadpool`/GDAL crate anywhere in the graph.
//!
//! Gated two ways: `required-features = ["cog"]` in `Cargo.toml` skips
//! building this file entirely under the default feature set, and the inner
//! `#![cfg]` below additionally requires `postgis` to be *off* — see
//! `pmtiles_binary.rs`'s own doc comment for why `required-features` alone
//! can't express that.

#![cfg(all(feature = "cog", not(feature = "postgis")))]

mod common;

use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::process::Command;

use common::{http_get, ServerProcess};

const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// The committed GeoTIFF fixture lives in `tellurion-cog`'s own test tree
/// (`crates/tellurion-cog/tests/fixtures/tiled_rgb.tif`) — one file, reused
/// by that crate's own tests and this real-binary proof rather than
/// duplicated. Resolved relative to the workspace root regardless of this
/// test binary's own working directory.
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tellurion-cog/tests/fixtures/tiled_rgb.tif")
}

fn write_temp_config(env_var: &str) -> PathBuf {
    let mut path = common::unique_temp_path("tellurion-server-cog-binary-test");
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
    driver: cog
    url_env: {env_var}
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: tiled_rgb
    catalog: default
    storage: main
    tiles: {{ minzoom: 0, maxzoom: 12, caps: {{}} }}
"#
    );
    std::fs::write(&path, common::legacy_config(&yaml)).expect("writes the throwaway config");
    path
}

/// Builds the command and delegates to [`common::spawn_server`] for the
/// listen-and-wait plumbing — this file's own thin adapter from a
/// `(config_path, env_var, storage_value)` triple to a `Command`.
fn spawn_server(
    config_path: &PathBuf,
    env_var: &str,
    storage_value: impl AsRef<std::ffi::OsStr>,
) -> (ServerProcess, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env(env_var, storage_value)
        .env("TELLURION_CONFIG", config_path)
        .env("PORT", "0");
    let (process, addr, _stderr_log) = common::spawn_server(command);
    (process, addr)
}

/// Like [`spawn_server`], but for a boot that's expected to fail (a remote
/// storage the eager validation sweep refuses) rather than reach "listening"
/// at all: polls `try_wait` instead of blocking on the "listening" log line
/// that will never come, capturing everything the binary wrote to stdout
/// (where `init_tracing`'s JSON lines land) along the way so the caller can
/// assert on the real refusal reason. Can't route through
/// [`common::spawn_server`] itself — that helper's own wait panics on
/// exactly the outcome this one is looking for — but still honors the same
/// `TELLURION_TEST_STARTUP_TIMEOUT_SECS` ceiling and poll interval, and
/// pipes (rather than inherits) stderr so a wrong refusal reason can be
/// diagnosed from the captured lines instead of only the terminal.
fn spawn_server_expecting_boot_failure(
    config_path: &PathBuf,
    env_var: &str,
    storage_value: impl AsRef<std::ffi::OsStr>,
) -> (std::process::ExitStatus, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion"));
    command
        .env(env_var, storage_value)
        .env("TELLURION_CONFIG", config_path)
        .env("PORT", "0")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = command.spawn().expect("spawns the tellurion binary");
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let _stderr_log = common::spawn_stderr_relay(stderr);
    let capture = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = BufReader::new(stdout).read_to_string(&mut buf);
        buf
    });

    let mut process = ServerProcess { child };
    let start = std::time::Instant::now();
    let ceiling = common::startup_timeout();
    let status = loop {
        if let Some(status) = process
            .child
            .try_wait()
            .expect("try_wait never errors on a live child")
        {
            break status;
        }
        assert!(
            start.elapsed() <= ceiling,
            "expected the binary to fail boot within the startup timeout instead of starting to listen"
        );
        std::thread::sleep(common::STARTUP_POLL_INTERVAL);
    };
    let output = capture
        .join()
        .expect("the stdout capture thread never panics");
    (status, output)
}

/// Decodes `png` (assumed to be produced by `tiny_skia`, straight after
/// `encode_rgba_to_png`'s premultiplied round trip) and returns the pixel at
/// `(x, y)` as straight, non-premultiplied RGBA — enough to assert an exact
/// color without pulling in an image-decoding dependency this test crate
/// doesn't otherwise need (the raw PNG chunk layout is walked by hand: IHDR
/// for dimensions, then a single IDAT is not assumed — instead this uses the
/// same `tiny_skia::Pixmap::decode_png` every other PNG test in this
/// workspace already relies on).
fn decode_png_pixel(png: &[u8], x: u32, y: u32) -> [u8; 4] {
    let pixmap = tiny_skia::Pixmap::decode_png(png).expect("valid PNG bytes");
    let pixel = pixmap.pixel(x, y).expect("pixel in bounds").demultiply();
    [pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()]
}

/// The proof, end to end: `/collections` lists the GeoTIFF-backed collection
/// with its real, tag-derived CRS84 extent and CRS, and its tiles lane
/// serves a real PNG raster tile for an addressed coordinate deep inside the
/// fixture's solid-yellow quadrant — all with zero database involvement (the
/// binary this test spawns was built with `postgis` compiled out). MVT is
/// refused as an unsupported capability, never a stub; a tile on the far
/// side of the globe from the fixture's tiny extent comes back empty (204).
#[test]
fn real_cog_binary_serves_collections_and_a_real_raster_tile_with_no_database_driver() {
    let env_var = "TELLURION_COG_BINARY_TEST_PATH";
    let config_path = write_temp_config(env_var);
    let (process, addr) = spawn_server(&config_path, env_var, fixture_path());

    let landing = http_get(&addr, "/");
    assert_eq!(landing.status, 200, "landing page should return 200");

    let collections = http_get(&addr, "/public/features/catalogs/default/collections");
    assert_eq!(collections.status, 200, "/collections should return 200");
    let body: serde_json::Value =
        serde_json::from_slice(&collections.body).expect("valid JSON body");
    let list = body["collections"].as_array().expect("collections array");
    assert_eq!(list.len(), 1, "exactly the one cog-backed collection");
    assert_eq!(list[0]["id"], "tiled_rgb");
    assert_eq!(
        list[0]["extent"]["spatial"]["bbox"][0],
        serde_json::json!([-1.28, -1.28, 1.28, 1.28]),
        "extent must come straight from the GeoTIFF tags, no database involved"
    );
    assert!(
        list[0]["links"]
            .as_array()
            .unwrap()
            .iter()
            .all(|link| link["rel"] != "items"),
        "a raster-only collection must not advertise an items link"
    );

    // z10/x513/y513 (path order tileRow=y before tileCol=x) sits entirely
    // inside the fixture's solid-yellow quadrant, away from every internal
    // tile boundary and the raster's own edge — see
    // `tellurion-cog/examples/gen_fixture.rs`'s doc for the fixture layout.
    let tile = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/tiled_rgb/tiles/WebMercatorQuad/10/513/513.png",
    );
    assert_eq!(
        tile.status, 200,
        "an addressed in-coverage tile should return 200"
    );
    assert_eq!(tile.content_type.as_deref(), Some("image/png"));
    assert_eq!(&tile.body[0..8], &PNG_MAGIC);
    assert_eq!(
        decode_png_pixel(&tile.body, 128, 128),
        [255, 255, 0, 255],
        "deep inside the yellow quadrant, every pixel should be solid yellow"
    );

    // MVT is a clean capability refusal, never a stub or a 500.
    let mvt = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/tiled_rgb/tiles/WebMercatorQuad/10/513/513.mvt",
    );
    assert_eq!(
        mvt.status, 400,
        "MVT on a raster collection must be refused, not served"
    );
    assert_eq!(
        mvt.content_type.as_deref(),
        Some("application/problem+json")
    );

    // Far side of the globe from the fixture's tiny [-1.28,-1.28,1.28,1.28]
    // extent — an in-range coordinate the raster never covers comes back
    // empty (204), the same empty-tile semantics every other driver's
    // TileSource uses.
    let far_tile = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/tiled_rgb/tiles/WebMercatorQuad/2/0/0.png",
    );
    assert_eq!(
        far_tile.status, 204,
        "a tile far outside the raster's coverage must come back empty"
    );

    drop(process);
    let _ = std::fs::remove_file(config_path);
}

/// A loopback HTTP/1.1 range server for the two remote-source tests below —
/// built on `axum`/`tokio`, both already real (non-dev) dependencies of this
/// crate (it's an axum server itself), so proving the remote `cog` path end
/// to end needs no new dependency here. Serves `fixture_path()`'s bytes
/// under `/tiled_rgb.tif`; `honor_range` selects between real 206/
/// `Content-Range` semantics and the "ignores `Range`, always 200" fixture
/// the refusal test needs.
mod loopback {
    use std::net::SocketAddr;
    use std::sync::Arc;

    use axum::extract::State;
    use axum::http::{header, HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;

    pub(super) async fn spawn(body: Vec<u8>, honor_range: bool) -> SocketAddr {
        let state = Arc::new((body, honor_range));
        let app = axum::Router::new()
            .route("/tiled_rgb.tif", get(serve))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds a loopback port");
        let addr = listener
            .local_addr()
            .expect("bound listener has a local addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        addr
    }

    async fn serve(State(state): State<Arc<(Vec<u8>, bool)>>, headers: HeaderMap) -> Response {
        let (body, honor_range) = state.as_ref();
        if *honor_range {
            if let Some(range) = headers
                .get(header::RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| parse_range(v, body.len()))
            {
                let (start, end) = range;
                return (
                    StatusCode::PARTIAL_CONTENT,
                    [(
                        header::CONTENT_RANGE,
                        format!("bytes {start}-{end}/{}", body.len()),
                    )],
                    body[start..=end].to_vec(),
                )
                    .into_response();
            }
        }
        // No `Range` header honored: either this is the "always 200"
        // refusal fixture, or a request this hand-rolled parser doesn't
        // recognize — both fall back to the whole body, exactly what a
        // range-ignorant server would do.
        (StatusCode::OK, body.clone()).into_response()
    }

    /// This fixture's own client (`tellurion-cog`'s `HttpRangeReader`)
    /// always sends `bytes=start-end` with both bounds present — no need to
    /// support an open-ended range here.
    fn parse_range(value: &str, total: usize) -> Option<(usize, usize)> {
        let spec = value.trim().strip_prefix("bytes=")?;
        let (start, end) = spec.split_once('-')?;
        let start: usize = start.parse().ok()?;
        let end: usize = end.parse().ok().unwrap_or(total.saturating_sub(1));
        Some((start, end.min(total.saturating_sub(1))))
    }
}

/// The `#37` slice 2 acceptance proof: the same collection as the local-file
/// test above, but with `url_env` pointing at a loopback ranged-HTTP
/// listener instead of a filesystem path — no local file involved at all.
/// Reuses the local test's own coverage-proving pixel/MVT/far-tile
/// assertions rather than repeating the full spread; the point here is the
/// transport, not re-proving the decode.
#[tokio::test]
async fn real_cog_binary_serves_a_remote_range_backed_raster_tile_with_no_local_file() {
    let fixture_bytes = std::fs::read(fixture_path()).expect("reads the committed fixture");
    let addr = loopback::spawn(fixture_bytes, true).await;
    let remote_url = format!("http://{addr}/tiled_rgb.tif");

    tokio::task::spawn_blocking(move || {
        let env_var = "TELLURION_COG_BINARY_TEST_REMOTE_URL";
        let config_path = write_temp_config(env_var);
        let (process, addr) = spawn_server(&config_path, env_var, &remote_url);

        let collections = http_get(&addr, "/public/features/catalogs/default/collections");
        assert_eq!(collections.status, 200, "/collections should return 200");
        let body: serde_json::Value =
            serde_json::from_slice(&collections.body).expect("valid JSON body");
        let list = body["collections"].as_array().expect("collections array");
        assert_eq!(list.len(), 1, "exactly the one remote cog-backed collection");
        assert_eq!(list[0]["id"], "tiled_rgb");
        assert_eq!(
            list[0]["extent"]["spatial"]["bbox"][0],
            serde_json::json!([-1.28, -1.28, 1.28, 1.28]),
            "extent must come straight from the GeoTIFF tags, fetched over ranged HTTP GET"
        );

        let tile = http_get(
            &addr,
            "/public/tiles/catalogs/default/collections/tiled_rgb/tiles/WebMercatorQuad/10/513/513.png",
        );
        assert_eq!(
            tile.status, 200,
            "an addressed in-coverage tile should return 200 over the remote source"
        );
        assert_eq!(tile.content_type.as_deref(), Some("image/png"));
        assert_eq!(&tile.body[0..8], &PNG_MAGIC);
        assert_eq!(
            decode_png_pixel(&tile.body, 128, 128),
            [255, 255, 0, 255],
            "deep inside the yellow quadrant, every pixel should be solid yellow"
        );

        drop(process);
        let _ = std::fs::remove_file(config_path);
    })
    .await
    .expect("the blocking subprocess/HTTP driving closure never panics");
}

/// `#37` slice 2: a remote storage that ignores `Range` and always answers
/// `200 OK` with the whole body is refused cleanly at boot (the default
/// eager `registry.validation` sweep) — the binary never starts listening,
/// and never falls back to downloading the object whole.
#[tokio::test]
async fn real_cog_binary_refuses_a_remote_source_that_ignores_range_requests() {
    let fixture_bytes = std::fs::read(fixture_path()).expect("reads the committed fixture");
    let addr = loopback::spawn(fixture_bytes, false).await;
    let remote_url = format!("http://{addr}/tiled_rgb.tif");

    tokio::task::spawn_blocking(move || {
        let env_var = "TELLURION_COG_BINARY_TEST_REMOTE_REFUSAL_URL";
        let config_path = write_temp_config(env_var);
        let (status, output) =
            spawn_server_expecting_boot_failure(&config_path, env_var, &remote_url);

        assert!(
            !status.success(),
            "a range-refusing remote source must fail boot, not start listening"
        );
        assert!(
            output.contains("range"),
            "the boot failure should name the real reason (no Range support), not a generic error; got: {output}"
        );

        let _ = std::fs::remove_file(config_path);
    })
    .await
    .expect("the blocking subprocess/HTTP driving closure never panics");
}

// -- `#254`: the bounded COG mosaic, through the same real binary -------------

/// The three committed mosaic constituents, staged into a fresh temp
/// directory with a manifest authored beside them by the very function
/// `tellurion-ingest cog mosaic` calls. Returns the manifest's path.
///
/// Staged rather than referenced in place because the manifest records each
/// source's path relative to its OWN directory — the arrangement that lets a
/// mosaic move as one directory — so authoring it into the worktree would
/// both write into the source tree and prove the wrong thing.
fn stage_mosaic(prefix: &str) -> (PathBuf, PathBuf) {
    let dir = common::unique_temp_path(prefix);
    std::fs::create_dir_all(&dir).expect("creates the mosaic directory");
    let fixtures =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tellurion-cog/tests/fixtures");
    let mut inputs = Vec::new();
    for name in [
        "mosaic_a_west.tif",
        "mosaic_b_east.tif",
        "mosaic_c_overlap.tif",
    ] {
        let dest = dir.join(name);
        std::fs::copy(fixtures.join(name), &dest).expect("stages a mosaic constituent");
        inputs.push(dest);
    }
    let manifest = dir.join("smoke_mosaic.yaml");
    tellurion_cog::author_mosaic_manifest(&inputs, &manifest).expect("authors the manifest");
    (dir, manifest)
}

fn write_temp_mosaic_config(env_var: &str) -> PathBuf {
    let mut path = common::unique_temp_path("tellurion-server-cog-mosaic-binary-test");
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
    driver: cog-mosaic
    url_env: {env_var}
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: smoke_mosaic
    catalog: default
    storage: main
    tiles: {{ minzoom: 0, maxzoom: 12, caps: {{}} }}
"#
    );
    std::fs::write(&path, common::legacy_config(&yaml)).expect("writes the throwaway config");
    path
}

/// The `#254` acceptance proof, end to end and with no database driver in the
/// binary at all: a `cog-mosaic` storage, pointed at a manifest sidecar that
/// `ingest`'s own authoring function measured out of three real GeoTIFFs,
/// lists its composed extent and serves composed PNG raster tiles — with the
/// composition order visible in the decoded pixels.
///
/// The fixture layout makes each assertion decisive on ONE pixel: the west
/// constituent is red over lon [-1.28, 0], the east one green over [0, 1.28],
/// and the overlapping one blue over [-0.64, 0.64] — and `mosaic_c_overlap`
/// sorts LAST, so wherever it covers, blue must win. Column 509 is west-only
/// (red), 514 east-only (green), and 511/512 are the two columns either side
/// of the seam, each composed from a DIFFERENT pair of sources and each
/// necessarily blue.
///
/// MVT is refused as an unsupported capability, exactly as it is for the
/// single-COG driver — a mosaic advertises `RasterSource` and nothing else.
#[test]
fn real_cog_mosaic_binary_serves_composed_tiles_and_refuses_mvt_with_no_database_driver() {
    let env_var = "TELLURION_COG_MOSAIC_BINARY_TEST_MANIFEST";
    let (dir, manifest) = stage_mosaic("tellurion-server-cog-mosaic-binary-test-dir");
    let config_path = write_temp_mosaic_config(env_var);
    let (process, addr) = spawn_server(&config_path, env_var, &manifest);

    let collections = http_get(&addr, "/public/features/catalogs/default/collections");
    assert_eq!(collections.status, 200, "/collections should return 200");
    let body: serde_json::Value =
        serde_json::from_slice(&collections.body).expect("valid JSON body");
    let list = body["collections"].as_array().expect("collections array");
    assert_eq!(list.len(), 1, "exactly the one mosaic-backed collection");
    assert_eq!(list[0]["id"], "smoke_mosaic");
    assert_eq!(
        list[0]["extent"]["spatial"]["bbox"][0],
        serde_json::json!([-1.28, -0.64, 1.28, 0.64]),
        "the extent must be the union of the three constituents' own measured bboxes"
    );

    // Path order is `/{z}/{tileRow}/{tileCol}`, i.e. y before x.
    let tile_at = |column: u32| {
        let tile = http_get(
            &addr,
            &format!(
                "/public/tiles/catalogs/default/collections/smoke_mosaic/tiles/WebMercatorQuad/10/511/{column}.png"
            ),
        );
        assert_eq!(tile.status, 200, "column {column} should serve a tile");
        assert_eq!(tile.content_type.as_deref(), Some("image/png"));
        assert_eq!(&tile.body[0..8], &PNG_MAGIC);
        decode_png_pixel(&tile.body, 128, 128)
    };

    assert_eq!(
        tile_at(509),
        [255, 0, 0, 255],
        "a tile only the western constituent covers is that constituent's own red"
    );
    assert_eq!(
        tile_at(514),
        [0, 255, 0, 255],
        "a tile only the eastern constituent covers is that constituent's own green"
    );
    assert_eq!(
        tile_at(511),
        [0, 0, 255, 255],
        "west + overlap: 'mosaic_c_overlap' sorts last, so it paints over 'mosaic_a_west'"
    );
    assert_eq!(
        tile_at(512),
        [0, 0, 255, 255],
        "east + overlap: the same rule, from the other pair of sources"
    );

    // MVT is a clean capability refusal, never a stub or a 500.
    let mvt = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/smoke_mosaic/tiles/WebMercatorQuad/10/511/511.mvt",
    );
    assert_eq!(
        mvt.status, 400,
        "MVT on a mosaic collection must be refused, not served"
    );
    assert_eq!(
        mvt.content_type.as_deref(),
        Some("application/problem+json")
    );

    // A coordinate no constituent covers is empty, not a fabricated blank.
    let far_tile = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/smoke_mosaic/tiles/WebMercatorQuad/2/0/0.png",
    );
    assert_eq!(far_tile.status, 204);

    drop(process);
    let _ = std::fs::remove_file(config_path);
    let _ = std::fs::remove_dir_all(dir);
}

/// The provenance half, through the real binary: a manifest whose recorded
/// SHA-256 no longer matches the object it names refuses the BOOT — the
/// default eager `registry.validation` sweep — naming the source. It never
/// starts listening, and it never quietly drops the source it could not
/// vouch for.
#[test]
fn real_cog_mosaic_binary_refuses_to_boot_when_a_sources_sha256_no_longer_matches() {
    let env_var = "TELLURION_COG_MOSAIC_BINARY_TEST_TAMPERED";
    let (dir, manifest) = stage_mosaic("tellurion-server-cog-mosaic-binary-tamper-dir");

    // Change the OBJECT, not the manifest: appending a byte to a constituent
    // is exactly the drift the recorded digest exists to catch, and it leaves
    // the manifest itself untouched and well-formed.
    let victim = dir.join("mosaic_c_overlap.tif");
    let mut bytes = std::fs::read(&victim).expect("reads the staged constituent");
    bytes.push(0);
    std::fs::write(&victim, bytes).expect("rewrites the staged constituent");

    let config_path = write_temp_mosaic_config(env_var);
    let (status, output) = spawn_server_expecting_boot_failure(&config_path, env_var, &manifest);

    assert!(
        !status.success(),
        "a mosaic whose source no longer matches its recorded provenance must fail boot"
    );
    assert!(
        output.contains("mosaic_c_overlap") && output.contains("byte_length"),
        "the boot failure must name the source and what disagreed; got: {output}"
    );

    let _ = std::fs::remove_file(config_path);
    let _ = std::fs::remove_dir_all(dir);
}

/// A mosaic proves every constituent before it starts serving, but an
/// operator can still remove a previously valid local source afterwards.
/// That is an operational failure, not an empty raster tile: the real
/// binary must keep running and return a structured 5xx response rather
/// than panic or silently compose the remaining sources.
#[test]
fn real_cog_mosaic_binary_returns_a_problem_when_a_validated_source_disappears() {
    let env_var = "TELLURION_COG_MOSAIC_BINARY_TEST_REMOVED_SOURCE";
    let (dir, manifest) = stage_mosaic("tellurion-server-cog-mosaic-removed-source");
    let config_path = write_temp_mosaic_config(env_var);
    let (process, addr) = spawn_server(&config_path, env_var, &manifest);

    let collections = http_get(&addr, "/public/features/catalogs/default/collections");
    assert_eq!(
        collections.status, 200,
        "the server must boot and validate every manifest source before the runtime removal"
    );

    let removed = dir.join("mosaic_c_overlap.tif");
    std::fs::remove_file(&removed).expect("removes a source only after successful startup");

    // z10/x511/y511 intersects both the western and overlap sources. The
    // latter was validated at boot, but this is the first request for this
    // cache key, so it must attempt the now-missing read rather than return
    // a stale PNG.
    let response = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/smoke_mosaic/tiles/WebMercatorQuad/10/511/511.png",
    );
    assert_eq!(
        response.status, 500,
        "a missing selected source must fail the whole mosaic tile, never become transparent"
    );
    assert_eq!(
        response.content_type.as_deref(),
        Some("application/problem+json"),
        "runtime storage failures must be consumable as RFC 9457 problems"
    );
    let problem: serde_json::Value = serde_json::from_slice(&response.body)
        .expect("the 5xx response is JSON, not an empty body");
    assert_eq!(problem["status"], 500);
    assert_eq!(problem["code"], "InternalServerError");

    drop(process);
    let _ = std::fs::remove_file(config_path);
    let _ = std::fs::remove_dir_all(dir);
}
