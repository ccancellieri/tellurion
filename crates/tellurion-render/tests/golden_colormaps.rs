//! Golden-image cases for the raster drivers' colormap classification
//! (`#174`): a served PNG tile, produced by the real
//! `RasterSource::raster_tile` of the Zarr and COG drivers over a
//! deterministic gradient fixture, compared byte-for-byte against a
//! committed reference.
//!
//! `#65`'s ramp goldens (`golden_ramps.rs`) deliberately did NOT run either
//! driver's own ramp math — its own doc names that as the remaining gap,
//! and this file closes it. What the cases below pin is the whole path a
//! client's PNG actually takes: sample decode -> colormap classification ->
//! resample onto the tile grid -> [`tellurion_render::encode_rgba_to_png`].
//!
//! ## Why these goldens cannot pass for the wrong reason
//!
//! Three ways a colormap golden can be decoration, each closed here by an
//! assertion that runs before the golden comparison:
//!
//! 1. **It renders nothing.** Every case asserts the tile actually carries
//!    a large number of DISTINCT colors ([`assert_is_a_real_gradient`]) —
//!    a blank, single-color or all-transparent tile fails there first.
//! 2. **It renders the same thing under every colormap.** Every case
//!    renders the SAME fixture under a second, different colormap and
//!    asserts the encoded bytes differ, and additionally that a color only
//!    the other colormap can produce is absent from this one.
//! 3. **It renders something unrelated to the configured colormap.** Each
//!    case asserts specific colors the configured colormap must produce for
//!    sample values the fixture is known to contain (`viridis`' own
//!    endpoints, an explicit stop's own RGBA) are present in the image.
//!
//! ## Determinism
//!
//! Both fixtures are integer sample grids with no interpolation on the read
//! path: the Zarr store is written by this file byte-for-byte, and the COG
//! is a committed 1 KiB fixture whose generator is checked in beside it
//! (`tellurion-cog/examples/gen_fixture.rs`). Colormap classification
//! itself is `f64` arithmetic rounded once per channel, and the resample is
//! nearest-neighbour index math — no thread pool, no clock, no font, no
//! locale. Everything `tests/common/mod.rs` says about scalar tiny-skia and
//! fixed PNG encoding applies unchanged; nothing here decodes through
//! tiny-skia's rasterizer at all, only through its PNG encoder.

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use tellurion_core::{
    CollectionDecl, ColorRamp, ColormapConf, ColormapStop, DriverFactory, RasterWindow,
    StorageDecl, TileCoord,
};

/// These encode to 14-21 KiB each — larger than this crate's other goldens
/// (~1-2 KiB) for a reason worth stating rather than hiding behind a round
/// number. The tile size is not this file's to choose: both drivers render
/// a fixed 256x256 `DEST_TILE_SIZE_PX` window, so the only lever left is
/// how much the image compresses, and what makes these compress poorly is
/// exactly what makes them worth pinning — 256 distinct classified colors,
/// the whole 8-bit colormap domain in one picture. Cutting the fixture's
/// value resolution would halve the bytes and sample a sixteenth of the
/// domain. The budget is set just above the largest of the four so that a
/// change which accidentally turned one into a full-detail photographic
/// image still fails here rather than landing in git.
const FIXTURE_BUDGET_BYTES: u64 = 24 * 1024;

/// `viridis` linearly across the full 8-bit domain. Both fixtures carry
/// every value in `0..=255`, so the ramp's own two endpoint colors must
/// appear verbatim in every tile rendered with it.
fn viridis_full_range() -> ColormapConf {
    ColormapConf::Ramp {
        ramp: ColorRamp::Viridis,
        min: 0.0,
        max: 255.0,
    }
}

/// Three explicit stops at values both fixtures are known to contain
/// exactly, in primary colors no built-in ramp ever produces — so "this
/// tile was classified by THIS colormap" is checkable by looking for one
/// specific RGBA, not merely by the bytes differing from something else.
fn primary_stops() -> ColormapConf {
    ColormapConf::Stops {
        stops: vec![
            ColormapStop {
                value: 0.0,
                rgba: [255, 0, 0, 255],
            },
            ColormapStop {
                value: 128.0,
                rgba: [0, 255, 0, 255],
            },
            ColormapStop {
                value: 255.0,
                rgba: [0, 0, 255, 255],
            },
        ],
    }
}

/// `viridis`' own control points at `t = 0` and `t = 1`
/// (`tellurion-zarr::colormap`/`tellurion-cog::colormap` both declare the
/// same table). A tile whose samples span `0..=255` under
/// [`viridis_full_range`] must contain both, exactly.
const VIRIDIS_MIN_RGBA: [u8; 4] = [68, 1, 84, 255];
const VIRIDIS_MAX_RGBA: [u8; 4] = [253, 231, 37, 255];

// -- driver plumbing ---------------------------------------------------

/// A `CollectionDecl` for `collection_id` carrying `colormap` in its
/// settings — built by serializing the real `ColormapConf` into the same
/// YAML shape an operator writes, rather than by constructing the decl
/// struct field by field, so these cases go through the config surface a
/// deployment actually uses.
fn decl_with_colormap(collection_id: &str, colormap: &ColormapConf) -> CollectionDecl {
    let colormap_yaml = serde_yaml::to_string(colormap).expect("serializes the colormap conf");
    let indented: String = colormap_yaml
        .lines()
        // A document-start marker, if this serde_yaml ever emits one, is not
        // a mapping key and must not be indented into the block below.
        .filter(|line| line.trim() != "---" && !line.trim().is_empty())
        .map(|line| format!("    {line}\n"))
        .collect();
    let doc = format!(
        "id: {collection_id}\ncatalog: default\nstorage: main\nsettings:\n  colormap:\n{indented}"
    );
    serde_yaml::from_str(&doc).unwrap_or_else(|err| panic!("collection decl: {err}\n{doc}"))
}

/// Builds `factory`'s driver over `locator` and asks it for `coord`'s
/// raster window, through the same public `DriverFactory` ->
/// `StorageDriver` -> `RasterSource` contract the server itself routes
/// through.
///
/// The locator reaches the driver through an environment variable because
/// that is the only way a `StorageDecl` names one. Each call uses its own
/// uniquely-numbered variable name and never removes it, so nothing here
/// depends on ordering against another test in this binary; the write and
/// the read that follows it are additionally serialized under [`ENV_LOCK`],
/// since cargo runs this binary's tests on several threads at once and the
/// process environment is one shared table regardless of how distinct the
/// names in it are.
async fn raster_tile(
    factory: &dyn DriverFactory,
    locator: &str,
    decl: &CollectionDecl,
    coord: TileCoord,
) -> RasterWindow {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    let url_env = format!(
        "TELLURION_GOLDEN_COLORMAP_SRC_{}",
        NEXT.fetch_add(1, Ordering::SeqCst)
    );
    let storage = StorageDecl {
        id: "main".to_string(),
        driver: factory.name().to_string(),
        url_env: url_env.clone(),
        pool_size: None,
    };

    // `build` reads `url_env` synchronously and keeps the resolved locator,
    // so the variable only has to exist for the length of this critical
    // section — nothing later in the request path reads the environment.
    let driver = {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::env::set_var(&url_env, locator);
        factory.build(&storage).expect("builds the driver")
    };
    let raster = driver
        .raster_source()
        .expect("both raster drivers advertise RasterSource");
    raster
        .raster_tile(decl, coord)
        .await
        .expect("raster_tile must not fail for a covered tile")
        .expect("the requested tile must be covered by the fixture's own extent")
}

fn encode(window: &RasterWindow) -> Vec<u8> {
    let png = tellurion_render::encode_rgba_to_png(&window.rgba, window.width, window.height)
        .expect("encodes the raster window");
    common::assert_is_png(&png);
    png
}

// -- anti-decoration checks --------------------------------------------

fn distinct_colors(window: &RasterWindow) -> BTreeSet<[u8; 4]> {
    window
        .rgba
        .chunks_exact(4)
        .map(|p| [p[0], p[1], p[2], p[3]])
        .collect()
}

/// A colormap golden is only worth anything if the tile it pins actually
/// shows the fixture's gradient. `min_distinct` is the number of separate
/// colors the classification must have produced; a blank, flat or fully
/// transparent tile — the classic golden that passes forever and proves
/// nothing — fails here, before any byte comparison.
fn assert_is_a_real_gradient(window: &RasterWindow, min_distinct: usize, what: &str) {
    let colors = distinct_colors(window);
    assert!(
        colors.len() >= min_distinct,
        "{what}: the rendered tile has only {} distinct colors (expected at least \
         {min_distinct}) — a tile this flat cannot distinguish one colormap from \
         another, so a golden of it would pin nothing",
        colors.len()
    );
}

fn assert_contains_color(window: &RasterWindow, rgba: [u8; 4], why: &str) {
    assert!(
        distinct_colors(window).contains(&rgba),
        "{why}: expected the color {rgba:?} somewhere in the tile, and it is absent"
    );
}

fn assert_lacks_color(window: &RasterWindow, rgba: [u8; 4], why: &str) {
    assert!(
        !distinct_colors(window).contains(&rgba),
        "{why}: the color {rgba:?} must NOT appear in this tile"
    );
}

/// The load-bearing check for every family here: the same fixture rendered
/// under a different colormap must produce different bytes.
fn assert_colormaps_render_differently(a: &[u8], b: &[u8], what: &str) {
    assert_ne!(
        a, b,
        "{what}: two different colormaps produced byte-identical tiles — the \
         configured colormap is not reaching the render path, so the goldens \
         here would pin nothing about classification"
    );
}

// -- Zarr ---------------------------------------------------------------

/// A private, self-cleaning temp directory holding a hand-built Zarr v2
/// store: a 16x16 single-band `u8` array chunked 8x8, raw (uncompressed),
/// whose sample at `(y, x)` is `y * 16 + x` — a bijection onto `0..=255`,
/// so one tile over this array's whole extent carries every possible byte
/// value exactly once and pins the colormap's entire domain.
///
/// The array declares the Web Mercator world extent in its `.zattrs`, so
/// the `z0/x0/y0` tile covers it exactly: the rendered tile is entirely
/// data, with no transparent margin to dilute what it pins. Built here
/// rather than committed as a binary fixture for the same reason
/// `tellurion-server/tests/zarr_binary.rs` builds its own: a small Zarr v2
/// store is a handful of tiny files, cheaper to write than to store in git.
struct ZarrFixture {
    dir: PathBuf,
}

impl ZarrFixture {
    fn build() -> Self {
        let dir = unique_temp_dir("tellurion-render-golden-zarr");
        std::fs::create_dir_all(&dir).expect("creates the fixture store directory");
        std::fs::write(
            dir.join(".zarray"),
            r#"{"zarr_format":2,"shape":[16,16],"chunks":[8,8],"dtype":"|u1","compressor":null,"fill_value":0,"order":"C"}"#,
        )
        .expect("writes .zarray");
        std::fs::write(
            dir.join(".zattrs"),
            r#"{"tellurion:extent_crs84":[-180.0,-85.0511287798066,180.0,85.0511287798066]}"#,
        )
        .expect("writes .zattrs");
        for chunk_y in 0..2u32 {
            for chunk_x in 0..2u32 {
                let mut chunk = Vec::with_capacity(64);
                for row in 0..8u32 {
                    for col in 0..8u32 {
                        let y = chunk_y * 8 + row;
                        let x = chunk_x * 8 + col;
                        chunk.push((y * 16 + x) as u8);
                    }
                }
                std::fs::write(dir.join(format!("{chunk_y}.{chunk_x}")), &chunk)
                    .expect("writes a chunk");
            }
        }
        Self { dir }
    }

    fn locator(&self) -> String {
        self.dir.to_string_lossy().into_owned()
    }
}

impl Drop for ZarrFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ))
}

/// The whole-world tile: with [`ZarrFixture`]'s own declared extent this is
/// exactly the array's own footprint.
const WORLD_TILE: TileCoord = TileCoord { z: 0, x: 0, y: 0 };

async fn zarr_tile(fixture: &ZarrFixture, colormap: &ColormapConf) -> RasterWindow {
    raster_tile(
        &tellurion_zarr::ZarrDriverFactory::new(),
        &fixture.locator(),
        &decl_with_colormap("demo", colormap),
        WORLD_TILE,
    )
    .await
}

#[tokio::test]
async fn zarr_ramp_colormap_classifies_a_full_range_gradient() {
    let fixture = ZarrFixture::build();
    let window = zarr_tile(&fixture, &viridis_full_range()).await;
    assert_eq!((window.width, window.height), (256, 256));

    assert_is_a_real_gradient(&window, 64, "zarr viridis");
    assert_contains_color(
        &window,
        VIRIDIS_MIN_RGBA,
        "the array contains sample 0, which viridis maps to its own first control point",
    );
    assert_contains_color(
        &window,
        VIRIDIS_MAX_RGBA,
        "the array contains sample 255, which viridis maps to its own last control point",
    );

    let png = encode(&window);
    let other = encode(&zarr_tile(&fixture, &primary_stops()).await);
    assert_colormaps_render_differently(&png, &other, "zarr");

    common::assert_golden("colormap_zarr_viridis.png", &png);
    common::assert_fixture_is_small("colormap_zarr_viridis.png", FIXTURE_BUDGET_BYTES);
}

#[tokio::test]
async fn zarr_stops_colormap_classifies_the_same_gradient_differently() {
    let fixture = ZarrFixture::build();
    let window = zarr_tile(&fixture, &primary_stops()).await;
    assert_eq!((window.width, window.height), (256, 256));

    assert_is_a_real_gradient(&window, 64, "zarr stops");
    // Each declared stop lands on a sample value the array actually has, so
    // its own RGBA must appear verbatim rather than only as a blend.
    assert_contains_color(&window, [255, 0, 0, 255], "the stop declared at value 0");
    assert_contains_color(&window, [0, 255, 0, 255], "the stop declared at value 128");
    assert_contains_color(&window, [0, 0, 255, 255], "the stop declared at value 255");
    assert_lacks_color(
        &window,
        VIRIDIS_MIN_RGBA,
        "no built-in ramp's color may appear in a tile classified by explicit stops",
    );

    let png = encode(&window);
    let other = encode(&zarr_tile(&fixture, &viridis_full_range()).await);
    assert_colormaps_render_differently(&png, &other, "zarr");

    common::assert_golden("colormap_zarr_stops.png", &png);
    common::assert_fixture_is_small("colormap_zarr_stops.png", FIXTURE_BUDGET_BYTES);
}

// -- COG ----------------------------------------------------------------

/// `tellurion-cog`'s own committed single-band gradient fixture — see that
/// crate's `examples/gen_fixture.rs` for the generator that produces it.
/// Referenced across crates by relative path, the same way
/// `tellurion-server/tests/cog_binary.rs` already reaches for
/// `tiled_rgb.tif`, rather than committing a second copy of the bytes here.
fn gray_gradient_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tellurion-cog/tests/fixtures/gray_gradient.tif")
}

/// The Web Mercator quadrant `lon [0, 1.40625], lat [~-1.405, 0]`. The
/// fixture spans `[-1.28, 1.28]` in both axes, so this tile covers its
/// bottom-right quadrant — source pixels `x, y in 16..32`, which is exactly
/// the 16x16 block carrying all 256 sample values (see the generator's own
/// doc) — plus a transparent margin where the tile reaches past the
/// raster's own edge. That margin is deliberate: it keeps the "outside the
/// data is transparent, never a guessed fill color" contract inside the
/// same golden.
const COG_QUADRANT_TILE: TileCoord = TileCoord {
    z: 8,
    x: 128,
    y: 128,
};

async fn cog_tile(colormap: &ColormapConf) -> RasterWindow {
    raster_tile(
        &tellurion_cog::CogDriverFactory::new(),
        &gray_gradient_fixture().to_string_lossy(),
        &decl_with_colormap("gray_gradient", colormap),
        COG_QUADRANT_TILE,
    )
    .await
}

#[tokio::test]
async fn cog_ramp_colormap_classifies_a_full_range_gradient() {
    let window = cog_tile(&viridis_full_range()).await;
    assert_eq!((window.width, window.height), (256, 256));

    assert_is_a_real_gradient(&window, 64, "cog viridis");
    assert_contains_color(
        &window,
        VIRIDIS_MIN_RGBA,
        "the covered quadrant contains sample 0",
    );
    assert_contains_color(
        &window,
        VIRIDIS_MAX_RGBA,
        "the covered quadrant contains sample 255",
    );
    assert_contains_color(
        &window,
        [0, 0, 0, 0],
        "the tile reaches past the raster's own edge, and outside it must be transparent",
    );

    let png = encode(&window);
    let other = encode(&cog_tile(&primary_stops()).await);
    assert_colormaps_render_differently(&png, &other, "cog");

    common::assert_golden("colormap_cog_viridis.png", &png);
    common::assert_fixture_is_small("colormap_cog_viridis.png", FIXTURE_BUDGET_BYTES);
}

#[tokio::test]
async fn cog_stops_colormap_classifies_the_same_gradient_differently() {
    let window = cog_tile(&primary_stops()).await;
    assert_eq!((window.width, window.height), (256, 256));

    assert_is_a_real_gradient(&window, 64, "cog stops");
    assert_contains_color(&window, [255, 0, 0, 255], "the stop declared at value 0");
    assert_contains_color(&window, [0, 255, 0, 255], "the stop declared at value 128");
    assert_contains_color(&window, [0, 0, 255, 255], "the stop declared at value 255");
    assert_lacks_color(
        &window,
        VIRIDIS_MIN_RGBA,
        "no built-in ramp's color may appear in a tile classified by explicit stops",
    );

    let png = encode(&window);
    let other = encode(&cog_tile(&viridis_full_range()).await);
    assert_colormaps_render_differently(&png, &other, "cog");

    common::assert_golden("colormap_cog_stops.png", &png);
    common::assert_fixture_is_small("colormap_cog_stops.png", FIXTURE_BUDGET_BYTES);
}
