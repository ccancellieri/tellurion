//! Regenerates this crate's test fixtures: a small tiled, uncompressed RGB
//! GeoTIFF with one overview (`tests/fixtures/tiled_rgb.tif`), a minimal
//! STRIPED GeoTIFF (`tests/fixtures/striped.tif`) used only to prove boot
//! refuses an unsupported layout, and a tiled single-band GRAY gradient
//! (`tests/fixtures/gray_gradient.tif`, `#174`) — the only band layout an
//! operator-configured colormap may be applied to — and three flat-color
//! single-IFD tiles for the `cog-mosaic` driver (`#254`,
//! `tests/fixtures/mosaic_a_west.tif`, `mosaic_b_east.tif`,
//! `mosaic_c_overlap.tif`). Run with:
//!
//! ```sh
//! cargo run -p tellurion-cog --example gen_fixture --features fixture-gen
//! ```
//!
//! Hand-rolled TIFF bytes, not the `tiff` crate's own encoder: that crate's
//! high-level `ImageEncoder` writes strips only (no tiled output), and this
//! driver is read-only by design (see `src/reader.rs`) — there is no write
//! path to reuse instead. This is the "generator source" the lane's fixture
//! convention asks for: it fully determines the committed binary, and both
//! are kept in the repository together.
//!
//! Classic (non-Big) TIFF, little-endian. Layout: header, IFD0 (the 256x256
//! main image, 2x2 tiles of 128x128 RGB8, uncompressed), IFD0's tile pixel
//! data, IFD1 (a 128x128 single-tile overview), IFD1's tile pixel data.
//! IFD0's `ModelPixelScaleTag`/`ModelTiepointTag`/`GeoKeyDirectoryTag`
//! georeference it to EPSG:4326, centered on `(0, 0)`, spanning exactly
//! `[-1.28, -1.28, 1.28, 1.28]` in CRS84 (0.01 degrees/pixel * 256 pixels).
//! Each main-image tile is a flat color (red/green/blue/yellow, reading
//! order) so a test can assert a specific pixel landed at the expected tile;
//! the overview is flat gray, a color no main-image tile uses, so a test can
//! prove the driver actually read the overview rather than upsampling the
//! main image.

use std::fs;
use std::path::PathBuf;

// -- Tag value encoding (TIFF 6.0 SS2) ---------------------------------------

#[derive(Clone)]
enum Value {
    Short(Vec<u16>),
    Long(Vec<u32>),
    Double(Vec<f64>),
}

impl Value {
    fn type_code(&self) -> u16 {
        match self {
            Value::Short(_) => 3,
            Value::Long(_) => 4,
            Value::Double(_) => 12,
        }
    }

    fn count(&self) -> u32 {
        match self {
            Value::Short(v) => v.len() as u32,
            Value::Long(v) => v.len() as u32,
            Value::Double(v) => v.len() as u32,
        }
    }

    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Value::Short(v) => {
                for x in v {
                    out.extend_from_slice(&x.to_le_bytes());
                }
            }
            Value::Long(v) => {
                for x in v {
                    out.extend_from_slice(&x.to_le_bytes());
                }
            }
            Value::Double(v) => {
                for x in v {
                    out.extend_from_slice(&x.to_le_bytes());
                }
            }
        }
        out
    }
}

/// One (tag id, value) pair, kept sorted ascending by tag id — the order
/// TIFF6 SS2 requires an IFD's entries to appear in.
type Tag = (u16, Value);

/// The byte size of `tags` encoded as one IFD: the entry count (2 bytes),
/// 12 bytes per entry, the "next IFD offset" field (4 bytes), plus every
/// entry whose value doesn't fit inline (> 4 bytes) stored immediately
/// after, back to back (every type used here is an even byte count, so no
/// padding is ever needed to keep offsets word-aligned).
fn ifd_encoded_size(tags: &[Tag]) -> u32 {
    let mut size = 2 + 12 * tags.len() as u32 + 4;
    for (_, value) in tags {
        let len = value.bytes().len();
        if len > 4 {
            size += len as u32;
        }
    }
    size
}

/// Encodes one IFD at `base_offset` (its absolute position in the file):
/// entry count, then one 12-byte entry per tag (values > 4 bytes point at
/// an offset into the external data that follows every entry), then
/// `next_ifd_offset`, then the external data itself.
fn encode_ifd(tags: &[Tag], next_ifd_offset: u32, base_offset: u32) -> Vec<u8> {
    let mut entries = Vec::new();
    let mut external = Vec::new();
    let external_start = base_offset + 2 + 12 * tags.len() as u32 + 4;

    for (tag_id, value) in tags {
        entries.extend_from_slice(&tag_id.to_le_bytes());
        entries.extend_from_slice(&value.type_code().to_le_bytes());
        entries.extend_from_slice(&value.count().to_le_bytes());
        let bytes = value.bytes();
        if bytes.len() <= 4 {
            let mut inline = bytes.clone();
            inline.resize(4, 0);
            entries.extend_from_slice(&inline);
        } else {
            let offset = external_start + external.len() as u32;
            entries.extend_from_slice(&offset.to_le_bytes());
            external.extend_from_slice(&bytes);
        }
    }

    let mut out = Vec::with_capacity(2 + entries.len() + 4 + external.len());
    out.extend_from_slice(&(tags.len() as u16).to_le_bytes());
    out.extend_from_slice(&entries);
    out.extend_from_slice(&next_ifd_offset.to_le_bytes());
    out.extend_from_slice(&external);
    out
}

fn tiff_header(first_ifd_offset: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    out.extend_from_slice(b"II"); // little-endian
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&first_ifd_offset.to_le_bytes());
    out
}

// -- Fixture 1: tiled RGB GeoTIFF with one overview --------------------------

fn solid_tile(rgb: [u8; 3], tile_px: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((tile_px * tile_px) as usize * 3);
    for _ in 0..(tile_px * tile_px) {
        out.extend_from_slice(&rgb);
    }
    out
}

fn geo_key_directory() -> Vec<u16> {
    vec![
        1, 1, 0, 2, // header: version 1.1.0, 2 keys
        1024, 0, 1, 2, // GTModelTypeGeoKey = Geographic
        2048, 0, 1, 4326, // GeographicTypeGeoKey = EPSG:4326
    ]
}

fn build_tiled_rgb_geotiff() -> Vec<u8> {
    const TILE_PX: u32 = 128;
    const MAIN_PX: u32 = 256;
    const TILE_BYTES: u32 = TILE_PX * TILE_PX * 3;
    const PIXEL_SCALE_DEG: f64 = 0.01;

    let tile_red = solid_tile([255, 0, 0], TILE_PX);
    let tile_green = solid_tile([0, 255, 0], TILE_PX);
    let tile_blue = solid_tile([0, 0, 255], TILE_PX);
    let tile_yellow = solid_tile([255, 255, 0], TILE_PX);
    let overview_tile = solid_tile([128, 128, 128], TILE_PX);

    let geo_keys: Vec<u16> = geo_key_directory();

    // IFD0 tags (placeholder TileOffsets — filled in once the layout below
    // resolves where tile data actually lands; the encoded size is
    // unaffected since the value count/type never changes).
    let ifd0_tags_shape = |tile_offsets: [u32; 4]| -> Vec<Tag> {
        vec![
            (256, Value::Short(vec![MAIN_PX as u16])),
            (257, Value::Short(vec![MAIN_PX as u16])),
            (258, Value::Short(vec![8, 8, 8])),
            (259, Value::Short(vec![1])), // Compression: none
            (262, Value::Short(vec![2])), // PhotometricInterpretation: RGB
            (277, Value::Short(vec![3])), // SamplesPerPixel
            (284, Value::Short(vec![1])), // PlanarConfiguration: chunky
            (322, Value::Short(vec![TILE_PX as u16])),
            (323, Value::Short(vec![TILE_PX as u16])),
            (324, Value::Long(tile_offsets.to_vec())),
            (325, Value::Long(vec![TILE_BYTES; 4])),
            (
                33550,
                Value::Double(vec![PIXEL_SCALE_DEG, PIXEL_SCALE_DEG, 0.0]),
            ),
            (
                33922,
                Value::Double(vec![
                    0.0,
                    0.0,
                    0.0,
                    -(PIXEL_SCALE_DEG * f64::from(MAIN_PX) / 2.0),
                    PIXEL_SCALE_DEG * f64::from(MAIN_PX) / 2.0,
                    0.0,
                ]),
            ),
            (34735, Value::Short(geo_keys.clone())),
        ]
    };

    const HEADER_LEN: u32 = 8;
    let ifd0_size = ifd_encoded_size(&ifd0_tags_shape([0, 0, 0, 0]));
    let ifd0_offset = HEADER_LEN;
    let tile_data0_offset = ifd0_offset + ifd0_size;
    let tile_offsets = [
        tile_data0_offset,
        tile_data0_offset + TILE_BYTES,
        tile_data0_offset + 2 * TILE_BYTES,
        tile_data0_offset + 3 * TILE_BYTES,
    ];
    let ifd1_offset = tile_data0_offset + 4 * TILE_BYTES;

    let ifd1_tags_shape = |tile_offset: u32| -> Vec<Tag> {
        vec![
            (256, Value::Short(vec![TILE_PX as u16])),
            (257, Value::Short(vec![TILE_PX as u16])),
            (258, Value::Short(vec![8, 8, 8])),
            (259, Value::Short(vec![1])),
            (262, Value::Short(vec![2])),
            (277, Value::Short(vec![3])),
            (284, Value::Short(vec![1])),
            (322, Value::Short(vec![TILE_PX as u16])),
            (323, Value::Short(vec![TILE_PX as u16])),
            (324, Value::Long(vec![tile_offset])),
            (325, Value::Long(vec![TILE_BYTES])),
        ]
    };
    let ifd1_size = ifd_encoded_size(&ifd1_tags_shape(0));
    let tile_data1_offset = ifd1_offset + ifd1_size;

    let mut out = tiff_header(ifd0_offset);
    out.extend_from_slice(&encode_ifd(
        &ifd0_tags_shape(tile_offsets),
        ifd1_offset,
        ifd0_offset,
    ));
    out.extend_from_slice(&tile_red);
    out.extend_from_slice(&tile_green);
    out.extend_from_slice(&tile_blue);
    out.extend_from_slice(&tile_yellow);
    out.extend_from_slice(&encode_ifd(
        &ifd1_tags_shape(tile_data1_offset),
        0,
        ifd1_offset,
    ));
    out.extend_from_slice(&overview_tile);

    assert_eq!(out.len() as u32, tile_data1_offset + TILE_BYTES);
    out
}

// -- Fixture 2: minimal STRIPED GeoTIFF (boot-refusal fixture) ---------------

fn build_striped_geotiff() -> Vec<u8> {
    const WIDTH: u32 = 16;
    const HEIGHT: u32 = 16;

    let strip = vec![0u8; (WIDTH * HEIGHT) as usize]; // single gray strip

    let tags_shape = |strip_offset: u32| -> Vec<Tag> {
        vec![
            (256, Value::Short(vec![WIDTH as u16])),
            (257, Value::Short(vec![HEIGHT as u16])),
            (258, Value::Short(vec![8])),
            (259, Value::Short(vec![1])),
            (262, Value::Short(vec![1])), // PhotometricInterpretation: BlackIsZero
            (273, Value::Long(vec![strip_offset])), // StripOffsets
            (277, Value::Short(vec![1])),
            (278, Value::Short(vec![HEIGHT as u16])), // RowsPerStrip
            (279, Value::Long(vec![WIDTH * HEIGHT])), // StripByteCounts
        ]
    };

    const HEADER_LEN: u32 = 8;
    let ifd_offset = HEADER_LEN;
    let ifd_size = ifd_encoded_size(&tags_shape(0));
    let strip_offset = ifd_offset + ifd_size;

    let mut out = tiff_header(ifd_offset);
    out.extend_from_slice(&encode_ifd(&tags_shape(strip_offset), 0, ifd_offset));
    out.extend_from_slice(&strip);
    out
}

// -- Fixture 3: tiled single-band GRAY gradient GeoTIFF (colormap fixture) ---

/// A configured `ColormapConf` only ever applies to a single-band
/// Grayscale raster (`src/driver.rs` refuses one over RGB/RGBA or a
/// paletted image), and neither fixture above is one — `tiled_rgb.tif` is
/// RGB and `striped.tif` is refused at boot for its layout. `#174`'s
/// image-level colormap goldens need a raster whose samples span the whole
/// 8-bit domain, so this fixture is deliberately a GRADIENT rather than the
/// flat colors the RGB fixture uses: a flat raster renders one single color
/// under every colormap, and a golden of it would pass whether the colormap
/// was applied correctly, incorrectly, or not at all.
///
/// 32x32 pixels, one 32x32 tile, uncompressed 8-bit BlackIsZero, at
/// 0.08 degrees/pixel — the same `[-1.28, -1.28, 1.28, 1.28]` CRS84 extent
/// centered on `(0, 0)` that `tiled_rgb.tif` already uses, so no test needs
/// a second georeferencing convention to reason about. 1 KiB of pixel data:
/// the smallest raster that still carries all 256 distinct sample values
/// inside a SINGLE Web Mercator quadrant, which is what lets one served
/// tile pin the whole 0..=255 colormap domain (see
/// `tellurion-render/tests/golden_colormaps.rs`).
///
/// The sample at `(x, y)` is `(x % 16) * 16 + (y % 16)`: within each 16x16
/// quadrant that is a bijection onto `0..=255`, so the quadrant one served
/// tile covers contains every possible byte exactly once.
fn build_tiled_gray_geotiff() -> Vec<u8> {
    const SIZE_PX: u32 = 32;
    const TILE_BYTES: u32 = SIZE_PX * SIZE_PX;
    const PIXEL_SCALE_DEG: f64 = 0.08;

    let mut tile = Vec::with_capacity(TILE_BYTES as usize);
    for y in 0..SIZE_PX {
        for x in 0..SIZE_PX {
            tile.push((((x % 16) * 16) + (y % 16)) as u8);
        }
    }

    let tags_shape = |tile_offset: u32| -> Vec<Tag> {
        vec![
            (256, Value::Short(vec![SIZE_PX as u16])),
            (257, Value::Short(vec![SIZE_PX as u16])),
            (258, Value::Short(vec![8])),
            (259, Value::Short(vec![1])), // Compression: none
            (262, Value::Short(vec![1])), // PhotometricInterpretation: BlackIsZero
            (277, Value::Short(vec![1])), // SamplesPerPixel: single band
            (284, Value::Short(vec![1])), // PlanarConfiguration: chunky
            (322, Value::Short(vec![SIZE_PX as u16])),
            (323, Value::Short(vec![SIZE_PX as u16])),
            (324, Value::Long(vec![tile_offset])),
            (325, Value::Long(vec![TILE_BYTES])),
            (
                33550,
                Value::Double(vec![PIXEL_SCALE_DEG, PIXEL_SCALE_DEG, 0.0]),
            ),
            (
                33922,
                Value::Double(vec![
                    0.0,
                    0.0,
                    0.0,
                    -(PIXEL_SCALE_DEG * f64::from(SIZE_PX) / 2.0),
                    PIXEL_SCALE_DEG * f64::from(SIZE_PX) / 2.0,
                    0.0,
                ]),
            ),
            (34735, Value::Short(geo_key_directory())),
        ]
    };

    const HEADER_LEN: u32 = 8;
    let ifd_offset = HEADER_LEN;
    let ifd_size = ifd_encoded_size(&tags_shape(0));
    let tile_offset = ifd_offset + ifd_size;

    let mut out = tiff_header(ifd_offset);
    out.extend_from_slice(&encode_ifd(&tags_shape(tile_offset), 0, ifd_offset));
    out.extend_from_slice(&tile);
    assert_eq!(out.len() as u32, tile_offset + TILE_BYTES);
    out
}

// -- Fixture 4: the `cog-mosaic` constituents (`#254`) ------------------------

/// A 32x32, single-IFD, single-tile, uncompressed RGB GeoTIFF of one flat
/// color, georeferenced to EPSG:4326 with its top-left corner at
/// `(origin_lon, origin_lat)` and 0.04 degrees per pixel — so each fixture
/// spans exactly 1.28 degrees on both axes.
///
/// Flat colors, and single-IFD (no overview), on purpose: a mosaic test is
/// about WHICH source's pixels reached the destination tile and in WHAT
/// order, so every source has to be identifiable from one pixel and nothing
/// else may vary. The three the mosaic uses lay out like this, in CRS84:
///
/// ```text
///   mosaic_a_west     lon [-1.28,  0.00]  lat [-0.64, 0.64]  RED
///   mosaic_b_east     lon [ 0.00,  1.28]  lat [-0.64, 0.64]  GREEN
///   mosaic_c_overlap  lon [-0.64,  0.64]  lat [-0.64, 0.64]  BLUE
/// ```
///
/// `c_overlap` straddles the seam between the other two, and sorts LAST by
/// id — so wherever it covers, the composition order rule ("ascending source
/// id, later paints over earlier") says blue must win. Where it does not
/// cover, west stays red and east stays green. Those three facts, together,
/// pin selection AND ordering to observable pixels.
fn build_flat_rgb_geotiff(rgb: [u8; 3], origin_lon: f64, origin_lat: f64) -> Vec<u8> {
    const SIZE_PX: u32 = 32;
    const PIXEL_SCALE_DEG: f64 = 0.04;
    const TILE_BYTES: u32 = SIZE_PX * SIZE_PX * 3;

    let tile = solid_tile(rgb, SIZE_PX);

    let tags_shape = |tile_offset: u32| -> Vec<Tag> {
        vec![
            (256, Value::Short(vec![SIZE_PX as u16])),
            (257, Value::Short(vec![SIZE_PX as u16])),
            (258, Value::Short(vec![8, 8, 8])),
            (259, Value::Short(vec![1])), // Compression: none
            (262, Value::Short(vec![2])), // PhotometricInterpretation: RGB
            (277, Value::Short(vec![3])), // SamplesPerPixel
            (284, Value::Short(vec![1])), // PlanarConfiguration: chunky
            (322, Value::Short(vec![SIZE_PX as u16])),
            (323, Value::Short(vec![SIZE_PX as u16])),
            (324, Value::Long(vec![tile_offset])),
            (325, Value::Long(vec![TILE_BYTES])),
            (
                33550,
                Value::Double(vec![PIXEL_SCALE_DEG, PIXEL_SCALE_DEG, 0.0]),
            ),
            (
                33922,
                Value::Double(vec![0.0, 0.0, 0.0, origin_lon, origin_lat, 0.0]),
            ),
            (34735, Value::Short(geo_key_directory())),
        ]
    };

    const HEADER_LEN: u32 = 8;
    let ifd_offset = HEADER_LEN;
    let ifd_size = ifd_encoded_size(&tags_shape(0));
    let tile_offset = ifd_offset + ifd_size;

    let mut out = tiff_header(ifd_offset);
    out.extend_from_slice(&encode_ifd(&tags_shape(tile_offset), 0, ifd_offset));
    out.extend_from_slice(&tile);
    assert_eq!(out.len() as u32, tile_offset + TILE_BYTES);
    out
}

fn main() {
    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    fs::create_dir_all(&fixtures_dir).expect("creates the fixtures directory");

    let tiled_path = fixtures_dir.join("tiled_rgb.tif");
    fs::write(&tiled_path, build_tiled_rgb_geotiff()).expect("writes the tiled RGB fixture");
    println!("wrote {}", tiled_path.display());

    let striped_path = fixtures_dir.join("striped.tif");
    fs::write(&striped_path, build_striped_geotiff()).expect("writes the striped fixture");
    println!("wrote {}", striped_path.display());

    let gray_path = fixtures_dir.join("gray_gradient.tif");
    fs::write(&gray_path, build_tiled_gray_geotiff()).expect("writes the gray gradient fixture");
    println!("wrote {}", gray_path.display());

    // `#254`: the three `cog-mosaic` constituents. Their file stems are the
    // manifest source ids `tellurion-ingest cog mosaic` derives, so the
    // `a_`/`b_`/`c_` prefixes are load-bearing -- they ARE the composition
    // order.
    for (name, rgb, origin_lon, origin_lat) in [
        ("mosaic_a_west.tif", [255u8, 0, 0], -1.28, 0.64),
        ("mosaic_b_east.tif", [0u8, 255, 0], 0.0, 0.64),
        ("mosaic_c_overlap.tif", [0u8, 0, 255], -0.64, 0.64),
    ] {
        let path = fixtures_dir.join(name);
        fs::write(&path, build_flat_rgb_geotiff(rgb, origin_lon, origin_lat))
            .unwrap_or_else(|error| panic!("writes {name}: {error}"));
        println!("wrote {}", path.display());
    }
}
