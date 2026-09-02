//! Golden-image cases for color ramps (`#65`): an interval (classed/stepped)
//! ramp and a continuous (interpolated) ramp, the two flavors the issue
//! calls out by name — "a value equal to a class edge must land in a
//! defined class, consistently" for the interval case, and "no silent
//! integer truncation anywhere" for the continuous one.
//!
//! Neither case reuses a driver crate's own ramp-classification code
//! (`tellurion-zarr`/`tellurion-cog`'s `colormap` modules, which already
//! have their own unit tests for exactly that math): this crate has no
//! dependency on either driver and has no ramp concept of its own — its
//! public surface is "paint one flat color per MVT layer"
//! ([`tellurion_render::render_mvt_to_png_styled`]) and "encode an
//! already-computed RGBA buffer" ([`tellurion_render::encode_rgba_to_png`]).
//! Both cases below are built from those two primitives, so what's actually
//! pinned here is "this crate's raster path renders/encodes a ramp-shaped
//! scene byte-for-byte the same every time" — the driver crates' own ramp
//! math getting an end-to-end golden of its own is one of the named
//! follow-ups in the PR body.

mod common;

use tellurion_render::{render_mvt_to_png_styled, LayerPaint};

const FIXTURE_BUDGET_BYTES: u64 = 50 * 1024;

/// `stroke_rgba` fully transparent, not merely absent: `polygon_end`
/// (`src/raster.rs`) always draws a stroke as well as a fill, and an
/// alpha-0 stroke is the only way to make that a no-op — anything else
/// (even the same flat color) draws a second, independently anti-aliased
/// pass exactly on top of each rectangle's own edge, which is what caused
/// this case's boundary columns to blend with their neighbor the first
/// time this was written. A width-0 fill-only swatch is also what a real
/// interval/classed legend actually looks like.
fn opaque_paint(rgba: [u8; 4]) -> LayerPaint {
    LayerPaint {
        fill_rgba: rgba,
        stroke_rgba: [0, 0, 0, 0],
        stroke_width: 0.0,
        point_radius: 1.0,
    }
}

/// Five adjacent, pixel-aligned 12px-wide bands across a 60x60 tile, each
/// its own MVT layer painted a distinct flat color through
/// [`render_mvt_to_png_styled`] — a graduated/interval legend, the shape a
/// real classed-color style actually renders as at this crate's layer.
/// Band edges land exactly on pixel columns (0/12/24/36/48/60), so there is
/// no anti-aliased blending to reason about at a class boundary: the
/// boundary pixel test below is checking this crate's own coordinate
/// mapping, not tiny-skia's edge coverage math.
#[test]
fn interval_ramp_bands_land_in_exactly_one_class_at_each_boundary() {
    const TILE_SIZE: u32 = 60;
    const EXTENT: u32 = 60;
    const BAND_WIDTH: i32 = 12;

    let bands: [(&str, [u8; 4]); 5] = [
        ("band0", [178, 24, 43, 255]), // lowest class
        ("band1", [239, 138, 98, 255]),
        ("band2", [253, 219, 199, 255]),
        ("band3", [146, 197, 222, 255]),
        ("band4", [33, 102, 172, 255]), // highest class
    ];

    let layers: Vec<_> = bands
        .iter()
        .enumerate()
        .map(|(i, (name, _))| {
            let x0 = i as i32 * BAND_WIDTH;
            common::layer(
                name,
                EXTENT,
                vec![common::rect_feature(
                    x0,
                    0,
                    x0 + BAND_WIDTH,
                    TILE_SIZE as i32,
                )],
            )
        })
        .collect();
    let mvt = common::tile_bytes(layers);

    let mut paints = std::collections::BTreeMap::new();
    for (name, rgba) in &bands {
        paints.insert(name.to_string(), opaque_paint(*rgba));
    }

    let png = render_mvt_to_png_styled(&mvt, &paints, None, TILE_SIZE).unwrap();
    common::assert_is_png(&png);
    let pixmap = tiny_skia::Pixmap::decode_png(&png).unwrap();
    assert_eq!((pixmap.width(), pixmap.height()), (TILE_SIZE, TILE_SIZE));

    // Mid-band pixels: each class renders its own declared color, not some
    // blend with a neighbor.
    for (i, (_, rgba)) in bands.iter().enumerate() {
        let mid_x = i as u32 * BAND_WIDTH as u32 + (BAND_WIDTH as u32 / 2);
        assert_eq!(
            common::pixel_rgba(&pixmap, mid_x, 30),
            *rgba,
            "band {i} mid-point must be exactly its declared class color"
        );
    }

    // The band0/band1 boundary sits at x=12: column 11 is the last column
    // that belongs to band0, column 12 the first that belongs to band1.
    // Each must land unambiguously in its own class, never a blend of both.
    assert_eq!(
        common::pixel_rgba(&pixmap, 11, 30),
        bands[0].1,
        "one pixel left of the boundary must still be band0's color"
    );
    assert_eq!(
        common::pixel_rgba(&pixmap, 12, 30),
        bands[1].1,
        "the boundary column itself must already be band1's color"
    );

    common::assert_golden("ramp_interval.png", &png);
    common::assert_fixture_is_small("ramp_interval.png", FIXTURE_BUDGET_BYTES);
}

/// A 101x8 continuous ramp built by hand-computing `t = x / (width - 1)` per
/// column and linearly interpolating two straight-RGBA colors
/// ([`common::lerp_rgba`]), then encoding through
/// [`tellurion_render::encode_rgba_to_png`] — the same raster-tile encode
/// path a driver crate's own resolved ramp values would go through. Width
/// 101 makes `x = 50` land exactly on `t = 0.5`, and the two colors below
/// are chosen so that midpoint's red channel (`32 + (255-32)*0.5 = 143.5`)
/// is a genuine tie: rounding gives 144, truncating would silently give
/// 143. Asserting the rounded value directly (not just via the golden byte
/// comparison) is what pins "no silent integer truncation" as a readable
/// claim rather than something only a full-image diff would notice.
#[test]
fn continuous_ramp_interpolates_without_truncating_fractional_channels() {
    const WIDTH: u32 = 101;
    const HEIGHT: u32 = 8;
    const FROM: [u8; 4] = [32, 64, 96, 255];
    const TO: [u8; 4] = [255, 140, 0, 255];

    let mut rgba = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
    for x in 0..WIDTH {
        let t = f64::from(x) / f64::from(WIDTH - 1);
        let color = common::lerp_rgba(FROM, TO, t);
        for y in 0..HEIGHT {
            common::set_pixel(&mut rgba, WIDTH, x, y, color);
        }
    }

    let png = tellurion_render::encode_rgba_to_png(&rgba, WIDTH, HEIGHT).unwrap();
    common::assert_is_png(&png);
    let pixmap = tiny_skia::Pixmap::decode_png(&png).unwrap();
    assert_eq!((pixmap.width(), pixmap.height()), (WIDTH, HEIGHT));

    assert_eq!(
        common::pixel_rgba(&pixmap, 0, 4),
        FROM,
        "t=0 must be exactly the ramp's starting color"
    );
    assert_eq!(
        common::pixel_rgba(&pixmap, WIDTH - 1, 4),
        TO,
        "t=1 must be exactly the ramp's ending color"
    );
    assert_eq!(
        common::pixel_rgba(&pixmap, 50, 4)[0],
        144,
        "t=0.5's red channel (143.5) must round to 144, not truncate to 143"
    );

    common::assert_golden("ramp_continuous.png", &png);
    common::assert_fixture_is_small("ramp_continuous.png", FIXTURE_BUDGET_BYTES);
}
