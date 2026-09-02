//! Golden-image cases for zoom-driven MapLibre paint expressions (`#174`):
//! one style document, one MVT tile, rendered at several zoom levels, with
//! a committed golden per zoom.
//!
//! This is the image-level half of the `#174` scope item "evaluate and pin
//! `step` and `interpolate` expressions across the zoom range in
//! `resolve_layer_paints`, rather than resolving only the first stop". The
//! unit tests in `src/maplibre.rs` pin the resolved *numbers*; these pin
//! what actually reaches a client — that a wider `line-width` really draws
//! a wider line, that an interpolated `fill-color` really lands between its
//! two stop colors, and that a `step` breakpoint really flips the whole
//! picture at the zoom it says it does.
//!
//! ## Why these goldens cannot pass for the wrong reason
//!
//! A golden that renders the same bytes under every input proves nothing.
//! Every case below therefore renders the SAME scene and the SAME style at
//! more than one zoom and asserts the encoded bytes DIFFER before comparing
//! any of them against a fixture ([`assert_zooms_render_differently`]).
//! That assertion is the load-bearing one: if `resolve_layer_paints` ever
//! goes back to ignoring its `zoom` argument, these tests fail on the
//! `assert_ne!` — immediately and with a readable message — rather than
//! quietly comparing three identical images against three identical
//! goldens forever.
//!
//! ## Determinism
//!
//! Same three guarantees as the rest of this crate's golden suite (see
//! `tests/common/mod.rs`): scalar (non-SIMD) tiny-skia, no font/glyph
//! rendering anywhere, fixed single-threaded PNG encoding. Nothing here
//! adds a new source of variance: the zoom levels are literal `f64`
//! constants in this file, and every interpolated value is computed in
//! `f64` and rounded once (`maplibre::blend_rgba`), so no platform's
//! floating-point stack has room to disagree.

mod common;

use std::collections::BTreeMap;

use serde_json::{json, Value};
use tellurion_render::{render_mvt_to_png_styled, resolve_layer_paints, LayerPaint};

const TILE_SIZE: u32 = 48;
const EXTENT: u32 = 48;
const FIXTURE_BUDGET_BYTES: u64 = 8 * 1024;

/// The zoom levels each golden pins. Deliberately one BELOW every stop
/// range (`Z_BELOW`), one strictly inside it that also sits exactly on the
/// `step` breakpoint (`Z_MID`), and one ABOVE every stop range (`Z_ABOVE`)
/// — the two clamped ends and the interpolating middle, which is the whole
/// "across the zoom range" the issue asks for.
const Z_BELOW: f64 = 2.0;
const Z_MID: f64 = 8.0;
const Z_ABOVE: f64 = 14.0;

/// One style document exercising all four zoom-driven shapes at once: an
/// interpolated color, an interpolated number, a stepped color and a
/// stepped number. Colors are hex literals, not names, so nothing here
/// depends on a CSS color table.
///
/// - `landuse` (fill): `fill-color` interpolates `#1b3a5c` -> `#e8c39e`
///   linearly over zoom 4..12.
/// - `roads` (line): `line-width` interpolates 1 -> 9 linearly over 4..12,
///   at a fixed opaque black.
/// - `poi` (circle): `circle-color`/`circle-radius` both `step` at zoom 8.
fn style_doc() -> Value {
    json!({
        "layers": [
            {
                "id": "landuse-fill",
                "type": "fill",
                "source-layer": "landuse",
                "paint": {
                    "fill-color": [
                        "interpolate", ["linear"], ["zoom"],
                        4, "#1b3a5c",
                        12, "#e8c39e"
                    ],
                },
            },
            {
                "id": "roads-line",
                "type": "line",
                "source-layer": "roads",
                "paint": {
                    "line-color": "#000000",
                    "line-width": ["interpolate", ["linear"], ["zoom"], 4, 1.0, 12, 9.0],
                },
            },
            {
                "id": "poi-circle",
                "type": "circle",
                "source-layer": "poi",
                "paint": {
                    "circle-color": ["step", ["zoom"], "#b2182b", 8, "#2166ac"],
                    "circle-radius": ["step", ["zoom"], 2.0, 8, 7.0],
                },
            },
        ],
    })
}

/// A full-tile background rectangle (`landuse`), a horizontal line across
/// the middle (`roads`), and one point left of centre (`poi`) — the three
/// geometry kinds the three paint properties above actually apply to,
/// nothing more. Pixel-aligned to the tile grid (extent == tile size), so
/// no case below depends on sub-pixel coordinate rounding.
fn scene() -> Vec<u8> {
    let size = TILE_SIZE as i32;
    common::tile_bytes(vec![
        common::layer(
            "landuse",
            EXTENT,
            vec![common::rect_feature(0, 0, size, size)],
        ),
        common::layer(
            "roads",
            EXTENT,
            vec![common::feature(
                geozero::mvt::tile::GeomType::Linestring,
                [common::move_to(0, size / 2), common::line_to(&[(size, 0)])].concat(),
            )],
        ),
        common::layer("poi", EXTENT, vec![common::point_feature(16, 16)]),
    ])
}

/// Renders [`scene`] with [`style_doc`] resolved at `zoom`. The only thing
/// that varies between calls is `zoom` — same MVT bytes, same style JSON,
/// same tile size — so any byte difference in the result is attributable to
/// zoom-expression evaluation and to nothing else.
fn render_at(zoom: f64) -> Vec<u8> {
    let paints: BTreeMap<String, LayerPaint> = resolve_layer_paints(&style_doc(), zoom);
    let png = render_mvt_to_png_styled(&scene(), &paints, None, TILE_SIZE).unwrap();
    common::assert_is_png(&png);
    png
}

/// The anti-decoration check: two renders that differ ONLY in zoom must
/// differ in bytes. Called before every golden comparison below.
fn assert_zooms_render_differently(a: (f64, &[u8]), b: (f64, &[u8])) {
    assert_ne!(
        a.1, b.1,
        "z{} and z{} rendered byte-identical output — the zoom expressions in \
         this scene's style are not reaching the rasterizer, so the goldens \
         below would pin nothing",
        a.0, b.0
    );
}

/// Zoom below every stop: MapLibre clamps to the first stop rather than
/// extrapolating, so this is the "widest scale end" of every ramp — and,
/// not coincidentally, the only value the pre-`#174` first-stop reading
/// ever produced at any zoom.
#[test]
fn zoom_below_every_stop_clamps_to_the_first_stop() {
    let png = render_at(Z_BELOW);
    let pixmap = tiny_skia::Pixmap::decode_png(&png).unwrap();

    // Background: exactly the first `fill-color` stop, not a blend.
    assert_eq!(
        common::pixel_rgba(&pixmap, 2, 2),
        [0x1b, 0x3a, 0x5c, 255],
        "below the first stop the fill must be the first stop's color exactly"
    );
    // `circle-color` below the step breakpoint is the base output.
    assert_eq!(
        common::pixel_rgba(&pixmap, 16, 16),
        [0xb2, 0x18, 0x2b, 255],
        "below the step breakpoint the circle must take the base output"
    );

    assert_zooms_render_differently((Z_BELOW, &png), (Z_MID, &render_at(Z_MID)));
    common::assert_golden("zoom_expr_z2.png", &png);
    common::assert_fixture_is_small("zoom_expr_z2.png", FIXTURE_BUDGET_BYTES);
}

/// Mid-range zoom: the interpolated properties land strictly between their
/// stops, and the `step` breakpoint at exactly this zoom has already
/// flipped (MapLibre's `step` takes the class at or above its input).
#[test]
fn zoom_inside_the_stop_range_interpolates_and_takes_the_step_class() {
    let png = render_at(Z_MID);
    let pixmap = tiny_skia::Pixmap::decode_png(&png).unwrap();

    // z8 is exactly halfway through 4..12. Each channel is the rounded
    // midpoint of `#1b3a5c` and `#e8c39e`: 0x1b/0xe8 -> 129.5 -> 130 (not
    // 129: rounded, never truncated), 0x3a/0xc3 -> 126.5 -> 127,
    // 0x5c/0x9e -> 125 exactly.
    assert_eq!(
        common::pixel_rgba(&pixmap, 2, 2),
        [130, 127, 125, 255],
        "halfway through the stop range the fill must be the blended color"
    );
    // The breakpoint is AT z8, so the class above it is already in force.
    assert_eq!(
        common::pixel_rgba(&pixmap, 16, 16),
        [0x21, 0x66, 0xac, 255],
        "a zoom exactly on a step breakpoint must take the class above it"
    );

    // Either side of the breakpoint, a hair apart: the picture must change.
    // This is the "zoom either side of a breakpoint" check stated as an
    // assertion instead of a one-off manual experiment.
    let just_below = render_at(7.999);
    assert_zooms_render_differently((7.999, &just_below), (Z_MID, &png));

    assert_zooms_render_differently((Z_MID, &png), (Z_ABOVE, &render_at(Z_ABOVE)));
    common::assert_golden("zoom_expr_z8.png", &png);
    common::assert_fixture_is_small("zoom_expr_z8.png", FIXTURE_BUDGET_BYTES);
}

/// Zoom above every stop: clamped to the last stop, never extrapolated past
/// it — a `line-width` that kept growing past its final stop would swallow
/// the whole tile.
#[test]
fn zoom_above_every_stop_clamps_to_the_last_stop() {
    let png = render_at(Z_ABOVE);
    let pixmap = tiny_skia::Pixmap::decode_png(&png).unwrap();

    assert_eq!(
        common::pixel_rgba(&pixmap, 2, 2),
        [0xe8, 0xc3, 0x9e, 255],
        "above the last stop the fill must be the last stop's color exactly"
    );

    // The 9px-wide black road is centred on row 24: rows 20..=28 are solid
    // black, and row 18 is still background. At `Z_BELOW`'s 1px width the
    // same rows would be background, which is what makes the two goldens
    // structurally different rather than merely differently tinted.
    assert_eq!(
        common::pixel_rgba(&pixmap, 40, 24),
        [0, 0, 0, 255],
        "the road's own centre line must be opaque black at every width"
    );
    assert_eq!(
        common::pixel_rgba(&pixmap, 40, 21),
        [0, 0, 0, 255],
        "3px off-centre must still be inside a 9px-wide road"
    );
    assert_eq!(
        common::pixel_rgba(&pixmap, 40, 18),
        [0xe8, 0xc3, 0x9e, 255],
        "6px off-centre must be outside a 9px-wide road"
    );

    assert_zooms_render_differently((Z_ABOVE, &png), (Z_BELOW, &render_at(Z_BELOW)));
    common::assert_golden("zoom_expr_z14.png", &png);
    common::assert_fixture_is_small("zoom_expr_z14.png", FIXTURE_BUDGET_BYTES);
}

/// All three goldens above must be pairwise distinct — stated once,
/// directly, so a future change that collapses two of them is caught here
/// by name rather than only as three separate golden mismatches.
#[test]
fn every_pinned_zoom_renders_a_distinct_image() {
    let below = render_at(Z_BELOW);
    let mid = render_at(Z_MID);
    let above = render_at(Z_ABOVE);
    assert_zooms_render_differently((Z_BELOW, &below), (Z_MID, &mid));
    assert_zooms_render_differently((Z_MID, &mid), (Z_ABOVE, &above));
    assert_zooms_render_differently((Z_BELOW, &below), (Z_ABOVE, &above));
}

/// A style with no zoom expression in it at all must render identically at
/// every zoom — the other half of the claim above. Without this, "the
/// picture changes with zoom" could be true for some reason that has
/// nothing to do with the expressions (a zoom leaking into the rasterizer's
/// own coordinate math, say), and every golden here would still pass.
#[test]
fn a_style_without_zoom_expressions_renders_identically_at_every_zoom() {
    let doc = json!({
        "layers": [{
            "id": "landuse-fill",
            "type": "fill",
            "source-layer": "landuse",
            "paint": { "fill-color": "#1b3a5c" },
        }],
    });
    let render = |zoom: f64| {
        let paints = resolve_layer_paints(&doc, zoom);
        render_mvt_to_png_styled(&scene(), &paints, None, TILE_SIZE).unwrap()
    };
    assert_eq!(
        render(Z_BELOW),
        render(Z_ABOVE),
        "a literal-only style must be zoom-independent: rendering for a \
         deployment whose style declares no zoom expression has to be \
         byte-identical at every zoom"
    );
}
