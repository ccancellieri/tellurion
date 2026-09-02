//! Golden-image comparison harness shared by this crate's `tests/golden_*.rs`
//! files (`#65`): renders a deterministic scene to PNG, compares it against a
//! small, checked-in reference image under `tests/fixtures/`, and — on a
//! mismatch — writes the actual bytes out for inspection instead of just
//! failing blind.
//!
//! `tests/common/mod.rs` is a plain module, not its own test target: cargo
//! only auto-discovers `tests/*.rs` files directly under `tests/`, so this
//! file is compiled into whichever test binary declares `mod common;`,
//! matching the convention `tellurion-server/tests/common/mod.rs` already
//! established for this workspace's other multi-file integration suites.
//!
//! ## Determinism
//!
//! A golden here is compared **byte-for-byte** against the checked-in PNG —
//! not decoded and compared pixel-by-pixel with slack — because every input
//! this harness renders is fully controlled (hand-built MVT bytes or a
//! computed RGBA buffer, never a font, a clock, or a thread pool), so two
//! renders of the same scene have no legitimate reason to differ at all.
//! Three things had to be pinned for that to actually hold:
//!
//! - **tiny-skia's SIMD feature is off** (see the workspace `Cargo.toml`'s
//!   own comment on its `tiny-skia` dependency): SIMD blend/rasterization
//!   there takes a different instruction path per CPU architecture, so
//!   leaving it on risks a golden generated on one architecture failing on
//!   another for a one-bit rounding difference nobody introduced. The
//!   scalar path is the one code path every architecture shares.
//! - **No font rendering.** None of these scenes draw text/labels — this
//!   crate's rasterizer (`src/raster.rs`) doesn't shape glyphs at all, so
//!   there is no font-hinting or subpixel-AA variance to worry about here.
//!   `#174` re-checked this and it still holds: there is no glyph
//!   rasterizer, no font dependency and no text paint property anywhere in
//!   this workspace, so that issue's "once label rendering exists, add
//!   fixed-font label goldens" item has no subject yet — label clipping and
//!   seam continuity for text cannot be covered until text can be drawn.
//!   The refusal is pinned as a test rather than left as prose: see
//!   `maplibre::tests::a_symbol_layer_contributes_no_paint_because_labels_are_not_rendered`,
//!   which is what fails on the day label rendering lands. Pulling a font
//!   into this workspace purely to have a label golden would trade one
//!   hazard for a worse one — a golden resolved against a SYSTEM font is
//!   not portable between machines at all, which is exactly what this
//!   section exists to prevent.
//! - **PNG encoding is single-threaded and uses fixed settings.**
//!   `tiny_skia::Pixmap::encode_png` (via the `png` crate, pure Rust,
//!   `miniz_oxide` deflate — no system zlib whose version could vary
//!   between machines) always encodes with the same compression settings;
//!   nothing in this harness or the render path spreads work across
//!   threads, so there is no output-order nondeterminism to inherit either.
//!
//! ## Re-blessing a golden
//!
//! A failing comparison writes the actual PNG bytes next to the target
//! directory (see [`assert_golden`]'s panic message for the exact path) so
//! it can be opened and inspected before doing anything else. Once the new
//! output has been reviewed and judged correct, regenerate the checked-in
//! fixture with:
//!
//! ```sh
//! UPDATE_GOLDENS=1 cargo test -p tellurion-render --test <test file, e.g. golden_ramps>
//! ```
//!
//! then `git diff`/view the updated PNG under `tests/fixtures/` before
//! committing it — re-blessing intentionally never happens silently.

#![allow(dead_code)]

use std::path::PathBuf;

/// Set to re-bless goldens: writes `actual` over the checked-in fixture
/// instead of comparing against it. Named after the same `UPDATE_GOLDENS`
/// shape used by other Rust golden-test setups (e.g. `insta`'s `INSTA_UPDATE`)
/// — this workspace had no prior env-var convention of its own to match, so
/// [`assert_golden`]'s own panic message is the source of truth for the exact
/// invocation.
pub const UPDATE_GOLDENS_ENV: &str = "UPDATE_GOLDENS";

/// A pixel-comparison budget for [`assert_golden_with_tolerance`]. Two
/// numbers, matching the issue's own deliverable ("per-channel epsilon + max
/// differing-pixel budget"): `max_channel_diff` bounds how far any single
/// R/G/B/A byte may drift for a pixel to still count as matching, and
/// `max_differing_pixels` bounds how many pixels in the whole image are
/// allowed to differ by that much before the comparison fails.
/// [`Tolerance::EXACT`] sets both to zero, and [`assert_golden`] — every case
/// in this crate's golden suite today — uses exactly that: this workspace's
/// render inputs are fully controlled, so there is no legitimate source of
/// per-pixel slack to budget for yet. The non-zero path exists because the
/// issue asks for it as part of the harness's shape, for a future case where
/// it's actually needed (e.g. a rasterizer swap that only ever nudges
/// anti-aliased edge pixels by one shade).
#[derive(Debug, Clone, Copy)]
pub struct Tolerance {
    pub max_channel_diff: u8,
    pub max_differing_pixels: usize,
}

impl Tolerance {
    /// Zero slack: every byte of every pixel must match. The default for
    /// every golden in this crate.
    pub const EXACT: Tolerance = Tolerance {
        max_channel_diff: 0,
        max_differing_pixels: 0,
    };
}

/// This crate's own `tests/fixtures/` directory — same relative location the
/// other driver crates in this workspace already use for their committed
/// binary test fixtures (`tellurion-cog`, `tellurion-pmtiles`, ...).
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Where a mismatching render's actual bytes get written for inspection:
/// under the shared `target/` directory (honoring `CARGO_TARGET_DIR` when
/// set, the same override `cargo` itself respects), never inside the source
/// tree — a failed comparison must never be able to leave a stray file for
/// `git status` to notice.
fn actual_output_dir() -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    target.join("golden-actual")
}

/// Byte-exact comparison against the checked-in golden named `name` (a
/// filename under `tests/fixtures/`, e.g. `"ramp_continuous.png"`) —
/// shorthand for [`assert_golden_with_tolerance`] with [`Tolerance::EXACT`],
/// which is every case this crate has today.
pub fn assert_golden(name: &str, actual_png: &[u8]) {
    assert_golden_with_tolerance(name, actual_png, Tolerance::EXACT);
}

/// Compares `actual_png` against the checked-in golden named `name` under
/// [`fixtures_dir`], within `tolerance`.
///
/// With [`UPDATE_GOLDENS_ENV`] set, this never compares at all: it writes
/// `actual_png` straight to the fixture path and returns, which is the only
/// supported way to change a checked-in golden (see this module's own doc
/// for the full re-bless workflow).
///
/// A missing fixture is treated as a mismatch with the same "go inspect the
/// actual output" message, rather than a separate error — a golden that
/// doesn't exist yet is bootstrapped through the exact same re-bless step as
/// updating one that does.
pub fn assert_golden_with_tolerance(name: &str, actual_png: &[u8], tolerance: Tolerance) {
    let golden_path = fixtures_dir().join(name);

    if std::env::var_os(UPDATE_GOLDENS_ENV).is_some() {
        std::fs::create_dir_all(golden_path.parent().expect("fixtures dir has a parent"))
            .expect("creates tests/fixtures");
        std::fs::write(&golden_path, actual_png).expect("writes the golden fixture");
        return;
    }

    let expected = std::fs::read(&golden_path).ok();
    let mismatch_reason = match &expected {
        None => Some("no golden fixture exists yet".to_string()),
        Some(expected) => compare_pngs(expected, actual_png, tolerance),
    };

    let Some(reason) = mismatch_reason else {
        return;
    };

    let actual_path = write_actual_output(name, actual_png);
    panic!(
        "golden mismatch for `{name}`: {reason}\n\
         \n\
         golden:  {golden}\n\
         actual:  {actual}\n\
         \n\
         Inspect the actual PNG above (it is NOT part of the git tree) and, \
         once the new output is confirmed correct, re-bless the golden with:\n\
         \n\
         \x20   UPDATE_GOLDENS=1 cargo test -p tellurion-render --test <test file>\n\
         \n\
         then review `git diff --stat tests/fixtures/` before committing it.",
        golden = golden_path.display(),
        actual = actual_path.display(),
    );
}

/// `None` when `actual` matches `expected` within `tolerance`; `Some(reason)`
/// otherwise. Byte-exact fast path first (every golden in this crate today
/// takes it): identical bytes always match regardless of tolerance, so a
/// [`Tolerance::EXACT`] comparison never has to decode either PNG at all.
fn compare_pngs(expected: &[u8], actual: &[u8], tolerance: Tolerance) -> Option<String> {
    if expected == actual {
        return None;
    }
    if tolerance.max_channel_diff == 0 && tolerance.max_differing_pixels == 0 {
        return Some(format!(
            "byte-exact comparison failed ({} expected bytes, {} actual bytes)",
            expected.len(),
            actual.len()
        ));
    }
    compare_within_tolerance(expected, actual, tolerance)
}

/// Decodes both PNGs and compares pixel-by-pixel, budgeting `tolerance`.
fn compare_within_tolerance(
    expected: &[u8],
    actual: &[u8],
    tolerance: Tolerance,
) -> Option<String> {
    let expected_pixmap = tiny_skia::Pixmap::decode_png(expected)
        .unwrap_or_else(|err| panic!("golden fixture is not a decodable PNG: {err}"));
    let actual_pixmap = tiny_skia::Pixmap::decode_png(actual)
        .unwrap_or_else(|err| panic!("rendered output is not a decodable PNG: {err}"));

    if expected_pixmap.width() != actual_pixmap.width()
        || expected_pixmap.height() != actual_pixmap.height()
    {
        return Some(format!(
            "dimensions differ: golden is {}x{}, actual is {}x{}",
            expected_pixmap.width(),
            expected_pixmap.height(),
            actual_pixmap.width(),
            actual_pixmap.height()
        ));
    }

    let mut differing_pixels = 0usize;
    let mut worst_channel_diff = 0u8;
    for (expected_pixel, actual_pixel) in expected_pixmap
        .pixels()
        .iter()
        .zip(actual_pixmap.pixels().iter())
    {
        let e = expected_pixel.demultiply();
        let a = actual_pixel.demultiply();
        let channel_diff = [
            e.red().abs_diff(a.red()),
            e.green().abs_diff(a.green()),
            e.blue().abs_diff(a.blue()),
            e.alpha().abs_diff(a.alpha()),
        ]
        .into_iter()
        .max()
        .unwrap_or(0);
        if channel_diff > tolerance.max_channel_diff {
            differing_pixels += 1;
            worst_channel_diff = worst_channel_diff.max(channel_diff);
        }
    }

    if differing_pixels > tolerance.max_differing_pixels {
        return Some(format!(
            "{differing_pixels} pixels exceeded the per-channel tolerance of \
             {} (worst observed diff: {worst_channel_diff}), budget was {} pixels",
            tolerance.max_channel_diff, tolerance.max_differing_pixels
        ));
    }
    None
}

/// Writes `actual_png` under [`actual_output_dir`] so a failing test leaves
/// something to open, then returns the path written.
fn write_actual_output(name: &str, actual_png: &[u8]) -> PathBuf {
    let dir = actual_output_dir();
    std::fs::create_dir_all(&dir).expect("creates the target-relative actual-output dir");
    let path = dir.join(name);
    std::fs::write(&path, actual_png).expect("writes the actual PNG for inspection");
    path
}

/// Convenience for tests that build up a small canvas by hand
/// (`encode_rgba_to_png` callers): writes straight (non-premultiplied) RGBA
/// at `(x, y)` on a `width`-wide buffer.
pub fn set_pixel(rgba: &mut [u8], width: u32, x: u32, y: u32, color: [u8; 4]) {
    let idx = ((y * width + x) * 4) as usize;
    rgba[idx..idx + 4].copy_from_slice(&color);
}

/// Linearly interpolates two straight-RGBA colors at `t` (clamped to
/// `[0, 1]`), rounding each channel rather than truncating — the same
/// "round, don't truncate" idiom this workspace's colormap drivers already
/// use for continuous ramps (`tellurion-zarr::colormap::lerp_rgba`,
/// `tellurion-cog::colormap`'s own equivalent). Deliberately reimplemented
/// here, not imported: this crate has no dependency on either driver crate,
/// and this helper only builds a synthetic test scene for
/// [`tellurion_render::encode_rgba_to_png`] — it is not a claim that this
/// pins those crates' own ramp math, which is their own unit tests' job (see
/// the PR body's remaining-cases list for that gap).
pub fn lerp_rgba(from: [u8; 4], to: [u8; 4], t: f64) -> [u8; 4] {
    let t = t.clamp(0.0, 1.0);
    let mut out = [0u8; 4];
    for i in 0..4 {
        let a = f64::from(from[i]);
        let b = f64::from(to[i]);
        out[i] = (a + (b - a) * t).round() as u8;
    }
    out
}

pub const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

pub fn assert_is_png(bytes: &[u8]) {
    assert_eq!(&bytes[0..8], &PNG_MAGIC, "not a PNG: missing magic bytes");
}

/// True if `path` under [`fixtures_dir`] is at most `max_bytes` — the "small
/// golden" budget the issue asks for, checked once per fixture right where
/// it's produced rather than trusted to stay true by convention.
pub fn assert_fixture_is_small(name: &str, max_bytes: u64) {
    let path = fixtures_dir().join(name);
    let len = std::fs::metadata(&path)
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()))
        .len();
    assert!(
        len <= max_bytes,
        "{name} is {len} bytes, budget is {max_bytes} bytes — shrink the scene \
         (smaller tile size, fewer features, flatter colors) rather than raising this budget"
    );
}

// -- MVT scene builders -------------------------------------------------
//
// Same hand-rolled MVT encoding this crate's own unit tests already use
// (`src/raster.rs`, `src/styled.rs`, `src/window.rs`) — kept here once so
// the golden tests in this directory share one copy instead of four.

use geozero::mvt::{tile, Message, Tile};

/// Encodes an MVT geometry command header: 3 low bits are the command id
/// (1 = MoveTo, 2 = LineTo, 7 = ClosePath), the rest is the repeat count.
pub fn cmd(id: u32, count: u32) -> u32 {
    id | (count << 3)
}

/// Zigzag-encodes a signed delta the way MVT geometry parameters are packed
/// (`vector_tile.proto`'s `sint32` convention).
pub fn zz(n: i32) -> u32 {
    ((n << 1) ^ (n >> 31)) as u32
}

pub fn move_to(dx: i32, dy: i32) -> Vec<u32> {
    vec![cmd(1, 1), zz(dx), zz(dy)]
}

pub fn line_to(deltas: &[(i32, i32)]) -> Vec<u32> {
    let mut v = vec![cmd(2, deltas.len() as u32)];
    for (dx, dy) in deltas {
        v.push(zz(*dx));
        v.push(zz(*dy));
    }
    v
}

pub fn close_path() -> Vec<u32> {
    vec![cmd(7, 1)]
}

pub fn layer(name: &str, extent: u32, features: Vec<tile::Feature>) -> tile::Layer {
    tile::Layer {
        version: 2,
        name: name.to_string(),
        extent: Some(extent),
        features,
        ..Default::default()
    }
}

pub fn tile_bytes(layers: Vec<tile::Layer>) -> Vec<u8> {
    Tile { layers }.encode_to_vec()
}

pub fn feature(geom_type: tile::GeomType, geometry: Vec<u32>) -> tile::Feature {
    let mut feature = tile::Feature {
        geometry,
        ..Default::default()
    };
    feature.set_type(geom_type);
    feature
}

/// A single-vertex point feature at raw MVT coordinates `(dx, dy)` from the
/// tile origin — the shape every classed-paint golden case in this suite
/// needs, spelled out once instead of at each call site.
pub fn point_feature(dx: i32, dy: i32) -> tile::Feature {
    feature(tile::GeomType::Point, move_to(dx, dy))
}

/// A single filled rectangle `[x0, y0] .. [x1, y1]` (raw MVT coordinates),
/// wound counter-clockwise so it renders as a normal (non-hole) exterior
/// ring under the even-odd fill rule this crate's rasterizer uses.
pub fn rect_feature(x0: i32, y0: i32, x1: i32, y1: i32) -> tile::Feature {
    let geometry = [
        move_to(x0, y0),
        line_to(&[(x1 - x0, 0), (0, y1 - y0), (x0 - x1, 0)]),
        close_path(),
    ]
    .concat();
    feature(tile::GeomType::Polygon, geometry)
}

pub fn pixel_rgba(pixmap: &tiny_skia::Pixmap, x: u32, y: u32) -> [u8; 4] {
    let p = pixmap.pixel(x, y).expect("pixel in bounds").demultiply();
    [p.red(), p.green(), p.blue(), p.alpha()]
}
