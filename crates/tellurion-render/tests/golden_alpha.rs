//! Golden-image cases for alpha compositing (`#65`): a single semi-
//! transparent fill over a basemap-less (fully transparent) tile must
//! decode back to its declared alpha exactly, and two overlapping
//! semi-transparent fills must composite by the ordinary Porter-Duff
//! "source over" rule rather than some premultiplication surprise (alpha
//! clamping wrong, colors mixing in the wrong space, ...).

mod common;

use std::collections::BTreeMap;

use tellurion_render::{render_mvt_to_png_styled, LayerPaint};

const FIXTURE_BUDGET_BYTES: u64 = 50 * 1024;

fn paint(fill_rgba: [u8; 4]) -> LayerPaint {
    LayerPaint {
        fill_rgba,
        stroke_rgba: fill_rgba,
        stroke_width: 0.0,
        point_radius: 1.0,
    }
}

/// tiny-skia's `Pixmap` always stores premultiplied color internally
/// (`Pixmap::decode_png`'s own doc), so every straight-alpha color this
/// crate paints gets premultiplied on the way in and demultiplied on the
/// way back out for pixel inspection — a round trip through `u8` rounding
/// twice. Declaring alpha = 204 (0.8 of 255) and RGB channels that are
/// multiples of 5 keeps `channel * 204 / 255` an exact integer at both ends
/// of that round trip (204/255 reduces to 4/5, and 4/5 of a multiple of 5
/// is always a whole number), so this case's exact-equality assertion is
/// checking this crate's compositing, not absorbing unrelated rounding
/// slack from the premultiply/demultiply round trip itself.
const FILL: [u8; 4] = [100, 150, 200, 204];

#[test]
fn a_single_semi_transparent_fill_keeps_its_declared_alpha_over_a_transparent_tile() {
    const TILE_SIZE: u32 = 40;
    const EXTENT: u32 = 40;

    let mvt = common::tile_bytes(vec![common::layer(
        "fill",
        EXTENT,
        vec![common::rect_feature(5, 5, 35, 35)],
    )]);
    let mut paints = BTreeMap::new();
    paints.insert("fill".to_string(), paint(FILL));

    let png = render_mvt_to_png_styled(&mvt, &paints, None, TILE_SIZE).unwrap();
    common::assert_is_png(&png);
    let pixmap = tiny_skia::Pixmap::decode_png(&png).unwrap();

    assert_eq!(
        common::pixel_rgba(&pixmap, 20, 20),
        FILL,
        "a pixel well inside the fill must decode back to exactly the declared straight RGBA"
    );
    assert_eq!(
        common::pixel_rgba(&pixmap, 1, 1)[3],
        0,
        "outside the fill, over no basemap, alpha must stay exactly 0"
    );

    common::assert_golden("alpha_single_fill.png", &png);
    common::assert_fixture_is_small("alpha_single_fill.png", FIXTURE_BUDGET_BYTES);
}

/// Two overlapping semi-transparent fills, painted in layer order (`under`
/// first, `over` second) — the shape a real style with a translucent fill
/// layer plus a translucent halo/overlay would produce. Standard
/// Porter-Duff "source over" compositing makes the combined alpha strictly
/// greater than either individual layer's own alpha whenever both are
/// non-trivially transparent (`result = a_over + a_under * (1 - a_over)`,
/// a convex combination of the two coverages) — an architecture-independent
/// property to assert directly, unlike the exact composited byte value,
/// which the golden comparison below pins instead of a hand-derived
/// expectation (working out the exact double round-tripped premultiply
/// arithmetic by hand here would be more likely to encode a mistake than to
/// catch one).
#[test]
fn two_overlapping_semi_transparent_fills_composite_instead_of_replacing() {
    const TILE_SIZE: u32 = 40;
    const EXTENT: u32 = 40;
    const UNDER: [u8; 4] = [200, 0, 0, 204];
    const OVER: [u8; 4] = [0, 0, 200, 120];

    let mvt = common::tile_bytes(vec![
        common::layer("under", EXTENT, vec![common::rect_feature(2, 2, 30, 30)]),
        common::layer("over", EXTENT, vec![common::rect_feature(10, 10, 38, 38)]),
    ]);
    let mut paints = BTreeMap::new();
    paints.insert("under".to_string(), paint(UNDER));
    paints.insert("over".to_string(), paint(OVER));

    let png = render_mvt_to_png_styled(&mvt, &paints, None, TILE_SIZE).unwrap();
    common::assert_is_png(&png);
    let pixmap = tiny_skia::Pixmap::decode_png(&png).unwrap();

    // (20, 20) sits inside both rectangles' overlap.
    let overlap_alpha = common::pixel_rgba(&pixmap, 20, 20)[3];
    assert!(
        overlap_alpha > UNDER[3] && overlap_alpha > OVER[3],
        "compositing two translucent layers must increase alpha beyond either \
         one alone (got {overlap_alpha}, under={}, over={})",
        UNDER[3],
        OVER[3]
    );

    // (5, 5) sits in `under` only.
    assert_eq!(
        common::pixel_rgba(&pixmap, 5, 5),
        UNDER,
        "outside the overlap, `under` alone must still show its own declared color"
    );

    common::assert_golden("alpha_stacked_fills.png", &png);
    common::assert_fixture_is_small("alpha_stacked_fills.png", FIXTURE_BUDGET_BYTES);
}
