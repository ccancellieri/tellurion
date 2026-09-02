//! Golden-image case for tile-seam consistency (`#65`): "two adjacent tiles
//! rendered separately must be seam-consistent — seam bugs only show at
//! tile borders and never in single-tile tests." A single-tile test can
//! never catch this class of bug by construction, so this renders a 2x2
//! block of tiles as four **independent** [`render_mvt_to_png`] calls (the
//! same shape a real client sees — each tile served over its own HTTP
//! request), decodes each PNG back, and stitches them into one canvas by
//! hand before comparing. That stitching step is the whole point: it's
//! standing in for what a map client does when it lays adjacent tiles next
//! to each other, so any per-tile coordinate or anti-aliasing artifact that
//! only shows up at a tile's own edge column/row becomes visible here even
//! though each individual tile render looks unremarkable on its own.
//!
//! One real-world line, straight and unbroken, crosses two of this block's
//! three internal seams: a horizontal line crosses the TL/TR vertical
//! boundary, and a vertical line crosses the TL/BL horizontal boundary —
//! each expressed as its own tile-local slice of the same line, the way an
//! MVT source actually clips one real feature per covering tile.

mod common;

use tellurion_render::{render_mvt_to_png, RenderStyle};

const TILE_SIZE: u32 = 50;
const EXTENT: u32 = 50;
const STROKE_WIDTH: f32 = 3.0;

fn style() -> RenderStyle {
    // Opaque black stroke, transparent fill: only the lines' own coverage
    // is visible, so every non-transparent pixel in the stitched image is
    // unambiguously part of a line.
    RenderStyle::new("#00000000", "#000000ff", STROKE_WIDTH, 1.0).unwrap()
}

fn horizontal_line() -> Vec<u32> {
    [common::move_to(0, 25), common::line_to(&[(50, 0)])].concat()
}

fn vertical_line() -> Vec<u32> {
    [common::move_to(25, 0), common::line_to(&[(0, 50)])].concat()
}

/// Renders one tile from a set of raw line geometries, each its own
/// LineString feature in a single `"lines"` layer — a real MVT source would
/// split these across layers/features by attribute, but seam continuity
/// depends only on where the strokes land, not on layer bookkeeping.
fn render_tile(geometries: Vec<Vec<u32>>) -> tiny_skia::Pixmap {
    let features = geometries
        .into_iter()
        .map(|g| common::feature(geozero::mvt::tile::GeomType::Linestring, g))
        .collect();
    let mvt = common::tile_bytes(vec![common::layer("lines", EXTENT, features)]);
    let png = render_mvt_to_png(&mvt, &style(), TILE_SIZE).unwrap();
    common::assert_is_png(&png);
    tiny_skia::Pixmap::decode_png(&png).unwrap()
}

/// Copies `src`'s straight RGBA pixels into `dst` (a `dst_width`-wide
/// buffer) at `(x_off, y_off)`.
fn blit(dst: &mut [u8], dst_width: u32, x_off: u32, y_off: u32, src: &tiny_skia::Pixmap) {
    for y in 0..src.height() {
        for x in 0..src.width() {
            let color = common::pixel_rgba(src, x, y);
            common::set_pixel(dst, dst_width, x_off + x, y_off + y, color);
        }
    }
}

/// Non-transparent pixel count in column `x`, rows `y_range` — a cheap
/// stand-in for "how thick does the stroke look here," compared across a
/// seam rather than trusted to match by construction.
fn column_coverage(rgba: &[u8], width: u32, x: u32, y_range: std::ops::Range<u32>) -> usize {
    y_range
        .filter(|&y| {
            let idx = ((y * width + x) * 4) as usize;
            rgba[idx + 3] > 0
        })
        .count()
}

/// Non-transparent pixel count in row `y`, columns `x_range`.
fn row_coverage(rgba: &[u8], width: u32, y: u32, x_range: std::ops::Range<u32>) -> usize {
    x_range
        .filter(|&x| {
            let idx = ((y * width + x) * 4) as usize;
            rgba[idx + 3] > 0
        })
        .count()
}

#[test]
fn a_line_crossing_two_tile_boundaries_stays_continuous_when_tiles_are_stitched() {
    let top_left = render_tile(vec![horizontal_line(), vertical_line()]);
    let top_right = render_tile(vec![horizontal_line()]);
    let bottom_left = render_tile(vec![vertical_line()]);
    let bottom_right = render_tile(vec![]);

    let combined_width = TILE_SIZE * 2;
    let combined_height = TILE_SIZE * 2;
    let mut combined = vec![0u8; (combined_width * combined_height * 4) as usize];
    blit(&mut combined, combined_width, 0, 0, &top_left);
    blit(&mut combined, combined_width, TILE_SIZE, 0, &top_right);
    blit(&mut combined, combined_width, 0, TILE_SIZE, &bottom_left);
    blit(
        &mut combined,
        combined_width,
        TILE_SIZE,
        TILE_SIZE,
        &bottom_right,
    );

    // -- Vertical seam (TL | TR), horizontal line at y=25 --------------
    // Coverage just left and just right of the seam must match each other
    // and an interior column untouched by any tile edge: a gap or a
    // doubled-up stroke at the boundary would show up as a mismatch here.
    let seam_left = column_coverage(&combined, combined_width, TILE_SIZE - 1, 20..31);
    let seam_right = column_coverage(&combined, combined_width, TILE_SIZE, 20..31);
    let interior = column_coverage(&combined, combined_width, 10, 20..31);
    assert_eq!(
        (seam_left, seam_right),
        (interior, interior),
        "the horizontal line's thickness must be identical on both sides of the \
         seam and match an interior column (left={seam_left}, right={seam_right}, interior={interior})"
    );

    // -- Horizontal seam (TL | BL), vertical line at x=25 ---------------
    let seam_top = row_coverage(&combined, combined_width, TILE_SIZE - 1, 20..31);
    let seam_bottom = row_coverage(&combined, combined_width, TILE_SIZE, 20..31);
    let interior = row_coverage(&combined, combined_width, 10, 20..31);
    assert_eq!(
        (seam_top, seam_bottom),
        (interior, interior),
        "the vertical line's thickness must be identical on both sides of the \
         seam and match an interior row (top={seam_top}, bottom={seam_bottom}, interior={interior})"
    );

    // Bottom-right quadrant carries no geometry at all: it must stay fully
    // transparent, the same "nothing to draw is still a valid image"
    // convention every other render in this crate already follows.
    let br_pixel = common::pixel_rgba(
        &tiny_skia::Pixmap::decode_png(
            &tellurion_render::encode_rgba_to_png(&combined, combined_width, combined_height)
                .unwrap(),
        )
        .unwrap(),
        75,
        75,
    );
    assert_eq!(
        br_pixel[3], 0,
        "the empty quadrant must stay fully transparent in the stitched image"
    );

    let png =
        tellurion_render::encode_rgba_to_png(&combined, combined_width, combined_height).unwrap();
    common::assert_is_png(&png);
    common::assert_golden("seam_2x2_block.png", &png);
    common::assert_fixture_is_small("seam_2x2_block.png", 50 * 1024);
}
