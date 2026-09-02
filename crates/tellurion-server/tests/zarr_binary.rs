//! The `#37` Zarr acceptance proof: the real `tellurion` binary, built with
//! the bundled database driver compiled out (`--no-default-features
//! --features zarr`), serves `/collections` and a PNG raster tile through
//! the abstract driver contract backed by nothing but a local, hand-built
//! Zarr v2 array store. Mirrors `tellurion-cog`'s own `cog_binary.rs` proof,
//! adapted to a Zarr-backed collection; no committed binary fixture is
//! needed here (unlike a GeoTIFF, a small Zarr v2 store is cheap to build
//! byte-for-byte inside the test itself).
//!
//! The remote-store follow-up test below reuses the exact same fixture
//! through a loopback HTTP server instead of a local path — mirroring
//! `cog_binary.rs`'s own `loopback` module and remote acceptance test,
//! adapted to whole-object `GET` (any path under the served directory)
//! rather than ranged reads, since a Zarr chunk is the atomic on-wire unit
//! (see `tellurion-zarr::store`'s own doc).
//!
//! Run exactly the invocation this proves:
//!
//! ```sh
//! cargo test -p tellurion --no-default-features --features zarr
//! cargo tree -p tellurion --no-default-features --features zarr -e normal
//! ```
//!
//! The second command is the other half of the proof (checked separately,
//! not by this file): it must list `tellurion-zarr`/`flate2` and no
//! `postgres`/`postgis`/`deadpool`/GDAL/`zarrs` crate anywhere in the graph.
//!
//! Gated two ways: `required-features = ["zarr"]` in `Cargo.toml` skips
//! building this file entirely under the default feature set, and the inner
//! `#![cfg]` below additionally requires `postgis` to be *off* — see
//! `pmtiles_binary.rs`'s own doc comment for why `required-features` alone
//! can't express that.

#![cfg(all(feature = "zarr", not(feature = "postgis")))]

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{http_get, ServerProcess};

const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// A private, self-cleaning temp directory holding a hand-built Zarr v2
/// array store — a plain 8x8 single-band `u8` array, chunked 4x4, every
/// sample the constant `100`, declaring `tellurion:extent_crs84 =
/// [-2, -2, 2, 2]` in its `.zattrs`. No network, no committed binary
/// fixture: the whole store is a handful of small files written directly by
/// this test.
struct FixtureStore {
    parent: PathBuf,
    path: PathBuf,
}

impl FixtureStore {
    /// The array directory's own final path component becomes this driver's
    /// reported physical collection name (`reader::open`'s own
    /// `logical_name` fallback, mirroring `tellurion-cog`'s file-stem
    /// convention) — named `demo` here specifically so it matches this
    /// test's own config `collections: - id: demo` below.
    fn build() -> Self {
        let parent = common::unique_temp_path("tellurion-server-zarr-binary-test");
        let path = parent.join("demo");
        std::fs::create_dir_all(&path).expect("creates the fixture store directory");

        std::fs::write(
            path.join(".zarray"),
            r#"{"zarr_format":2,"shape":[8,8],"chunks":[4,4],"dtype":"|u1","compressor":null,"fill_value":0,"order":"C"}"#,
        )
        .expect("writes .zarray");
        std::fs::write(
            path.join(".zattrs"),
            r#"{"tellurion:extent_crs84":[-2.0,-2.0,2.0,2.0]}"#,
        )
        .expect("writes .zattrs");
        for chunk_y in 0..2 {
            for chunk_x in 0..2 {
                let mut file =
                    std::fs::File::create(path.join(format!("{chunk_y}.{chunk_x}"))).unwrap();
                file.write_all(&[100u8; 16]).unwrap();
            }
        }

        Self { parent, path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for FixtureStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.parent);
    }
}

fn write_temp_config(env_var: &str) -> PathBuf {
    let mut path = common::unique_temp_path("tellurion-server-zarr-binary-test-config");
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
    driver: zarr
    url_env: {env_var}
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: demo
    catalog: default
    storage: main
    tiles: {{ minzoom: 0, maxzoom: 8, caps: {{}} }}
    settings:
      colormap: {{ kind: ramp, ramp: grayscale, min: 0.0, max: 255.0 }}
"#
    );
    std::fs::write(&path, common::legacy_config(&yaml)).expect("writes the throwaway config");
    path
}

/// Builds the command and delegates to [`common::spawn_server`] for the
/// listen-and-wait plumbing.
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

/// Decodes `png` and returns the pixel at `(x, y)` as straight,
/// non-premultiplied RGBA — same helper `cog_binary.rs` uses.
fn decode_png_pixel(png: &[u8], x: u32, y: u32) -> [u8; 4] {
    let pixmap = tiny_skia::Pixmap::decode_png(png).expect("valid PNG bytes");
    let pixel = pixmap.pixel(x, y).expect("pixel in bounds").demultiply();
    [pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()]
}

/// The proof, end to end: `/collections` lists the Zarr-backed collection
/// with its `.zattrs`-declared CRS84 extent, its tiles lane serves a real
/// PNG raster tile whose in-coverage pixels reflect the array's own constant
/// sample colored through the configured grayscale ramp, dataType reports
/// "map" (never "vector"), and MVT is refused as an unsupported capability
/// — all with zero database involvement (the binary this test spawns was
/// built with `postgis` compiled out).
#[test]
fn real_zarr_binary_serves_collections_and_a_real_raster_tile_with_no_database_driver() {
    let store = FixtureStore::build();
    let env_var = "TELLURION_ZARR_BINARY_TEST_PATH";
    let config_path = write_temp_config(env_var);
    let (process, addr) = spawn_server(&config_path, env_var, store.path());

    let landing = http_get(&addr, "/");
    assert_eq!(landing.status, 200, "landing page should return 200");

    let collections = http_get(&addr, "/public/features/catalogs/default/collections");
    assert_eq!(collections.status, 200, "/collections should return 200");
    let body: serde_json::Value =
        serde_json::from_slice(&collections.body).expect("valid JSON body");
    let list = body["collections"].as_array().expect("collections array");
    assert_eq!(list.len(), 1, "exactly the one zarr-backed collection");
    assert_eq!(list[0]["id"], "demo");
    assert_eq!(
        list[0]["extent"]["spatial"]["bbox"][0],
        serde_json::json!([-2.0, -2.0, 2.0, 2.0]),
        "extent must come straight from the store's own .zattrs declaration"
    );
    assert!(
        list[0]["links"]
            .as_array()
            .unwrap()
            .iter()
            .all(|link| link["rel"] != "items"),
        "a raster-only collection must not advertise an items link"
    );

    // The single-tileset resource reports dataType "map" for a raster-only
    // collection, the same shared resolver `cog_binary.rs`'s own coverage
    // already proves generically for any RasterSource-backed collection.
    let tileset = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad",
    );
    assert_eq!(tileset.status, 200);
    let tileset_body: serde_json::Value =
        serde_json::from_slice(&tileset.body).expect("valid JSON body");
    assert_eq!(tileset_body["dataType"], "map");

    // z0/x0/y0 covers the whole world, so it fully contains the fixture's
    // tiny [-2,-2,2,2] extent; its center pixel sits deep inside real data.
    let tile = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/0/0/0.png",
    );
    assert_eq!(
        tile.status, 200,
        "a world-covering tile intersecting the array's tiny extent should return 200"
    );
    assert_eq!(tile.content_type.as_deref(), Some("image/png"));
    assert_eq!(&tile.body[0..8], &PNG_MAGIC);
    // Every sample in the fixture is the constant 100; under the grayscale
    // ramp [0, 255] that resolves to mid-gray, opaque.
    assert_eq!(
        decode_png_pixel(&tile.body, 128, 128),
        [100, 100, 100, 255],
        "the tile's center pixel should show the array's own constant value colored by the ramp"
    );
    // Far from the array's own tiny extent, well within the same
    // world-covering tile, must stay transparent rather than a guessed
    // color.
    assert_eq!(
        decode_png_pixel(&tile.body, 10, 10)[3],
        0,
        "pixels far outside the array's tiny extent should stay transparent"
    );

    // MVT is a clean capability refusal, never a stub or a 500.
    let mvt = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/0/0/0.mvt",
    );
    assert_eq!(
        mvt.status, 400,
        "MVT on a raster collection must be refused, not served"
    );
    assert_eq!(
        mvt.content_type.as_deref(),
        Some("application/problem+json")
    );

    drop(process);
    let _ = std::fs::remove_file(config_path);
}

/// `#37`: a Zarr collection with no configured colormap refuses PNG tile
/// requests by name (a raw Zarr sample has no inherent visual meaning),
/// rather than serving a guessed grayscale replicate — this is a real
/// behavioral difference from `tellurion-cog`, where an unconfigured
/// single-band GeoTIFF still renders (grayscale replicate). Named, not a 500.
#[test]
fn real_zarr_binary_refuses_a_tile_request_without_a_configured_colormap() {
    let store = FixtureStore::build();
    let env_var = "TELLURION_ZARR_BINARY_TEST_NO_COLORMAP_PATH";
    let mut path = common::unique_temp_path("tellurion-server-zarr-binary-test-no-colormap-config");
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
    driver: zarr
    url_env: {env_var}
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: demo
    catalog: default
    storage: main
    tiles: {{ minzoom: 0, maxzoom: 8, caps: {{}} }}
"#
    );
    std::fs::write(&path, common::legacy_config(&yaml)).expect("writes the throwaway config");

    let (process, addr) = spawn_server(&path, env_var, store.path());
    let tile = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/0/0/0.png",
    );
    assert_eq!(
        tile.status, 400,
        "a Zarr collection without a configured colormap must refuse PNG tiles by name, not serve a guessed color"
    );
    assert_eq!(
        tile.content_type.as_deref(),
        Some("application/problem+json")
    );
    let body: serde_json::Value = serde_json::from_slice(&tile.body).expect("valid JSON body");
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("colormap"),
        "the refusal should name the real reason: {body}"
    );

    drop(process);
    let _ = std::fs::remove_file(path);
}

/// A loopback HTTP/1.1 server for the remote-store test below — built on
/// `axum`/`tokio`, both already real (non-dev) dependencies of this crate
/// (it's an axum server itself), so proving the remote `zarr` path end to
/// end needs no new dependency here. Unlike `cog_binary.rs`'s own `loopback`
/// module (one file at one fixed route, read with `Range`), this serves
/// *any* path under `root` whole — a Zarr store is a directory of small
/// documents/chunks, and this driver's own `RemoteZarrSource` never sends a
/// ranged request (see `tellurion-zarr::store`'s own doc for why a Zarr
/// chunk needs none).
mod loopback {
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use std::sync::Arc;

    use axum::extract::{Path as AxumPath, State};
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;

    pub(super) async fn spawn(root: PathBuf) -> SocketAddr {
        let app = axum::Router::new()
            .route("/{*path}", get(serve))
            .with_state(Arc::new(root));
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

    async fn serve(State(root): State<Arc<PathBuf>>, AxumPath(path): AxumPath<String>) -> Response {
        match std::fs::read(root.join(&path)) {
            Ok(bytes) => (StatusCode::OK, bytes).into_response(),
            Err(_) => StatusCode::NOT_FOUND.into_response(),
        }
    }
}

/// The `#37` remote-store follow-up acceptance proof: the same collection as
/// the local-directory test above, but with `url_env` pointing at a loopback
/// HTTP listener instead of a filesystem path — no local file read at all
/// once the server is up. Reuses the local test's own coverage-proving
/// pixel/tileset/colormap assertions rather than repeating the full spread;
/// the point here is the transport, not re-proving the decode.
#[tokio::test]
async fn real_zarr_binary_serves_a_remote_store_backed_raster_tile_with_no_local_file() {
    let store = FixtureStore::build();
    // Serves the fixture's own *parent* directory, so the array directory's
    // own name ("demo") stays part of the URL path this test points the
    // driver at — the same shape a real deployment's locator has, and what
    // `RemoteZarrSource::logical_name` needs to report the right physical
    // collection name (this config's own `collections: - id: demo` must
    // match it).
    let addr = loopback::spawn(
        store
            .path()
            .parent()
            .expect("the fixture's own array directory has a parent")
            .to_path_buf(),
    )
    .await;
    let remote_url = format!("http://{addr}/demo/");

    tokio::task::spawn_blocking(move || {
        let env_var = "TELLURION_ZARR_BINARY_TEST_REMOTE_URL";
        let config_path = write_temp_config(env_var);
        let (process, addr) = spawn_server(&config_path, env_var, &remote_url);

        let collections = http_get(&addr, "/public/features/catalogs/default/collections");
        assert_eq!(collections.status, 200, "/collections should return 200");
        let body: serde_json::Value =
            serde_json::from_slice(&collections.body).expect("valid JSON body");
        let list = body["collections"].as_array().expect("collections array");
        assert_eq!(list.len(), 1, "exactly the one remote zarr-backed collection");
        assert_eq!(list[0]["id"], "demo");
        assert_eq!(
            list[0]["extent"]["spatial"]["bbox"][0],
            serde_json::json!([-2.0, -2.0, 2.0, 2.0]),
            "extent must come straight from the store's own .zattrs, fetched over HTTP GET"
        );

        let tileset = http_get(
            &addr,
            "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad",
        );
        assert_eq!(tileset.status, 200);
        let tileset_body: serde_json::Value =
            serde_json::from_slice(&tileset.body).expect("valid JSON body");
        assert_eq!(tileset_body["dataType"], "map");

        let tile = http_get(
            &addr,
            "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/0/0/0.png",
        );
        assert_eq!(
            tile.status, 200,
            "a world-covering tile intersecting the array's tiny extent should return 200 over the remote store"
        );
        assert_eq!(tile.content_type.as_deref(), Some("image/png"));
        assert_eq!(&tile.body[0..8], &PNG_MAGIC);
        assert_eq!(
            decode_png_pixel(&tile.body, 128, 128),
            [100, 100, 100, 255],
            "the tile's center pixel should show the array's own constant value colored by the ramp"
        );
        assert_eq!(
            decode_png_pixel(&tile.body, 10, 10)[3],
            0,
            "pixels far outside the array's tiny extent should stay transparent"
        );

        drop(process);
        let _ = std::fs::remove_file(config_path);
    })
    .await
    .expect("the blocking subprocess/HTTP driving closure never panics");
}
