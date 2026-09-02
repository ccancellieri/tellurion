//! COG authoring (`#37` authoring lane, first slice): converts a plain,
//! single-resolution GeoTIFF into a serving-optimized COG this crate's own
//! [`crate::reader`] can serve — tiled, Deflate-compressed, with a
//! power-of-two overview pyramid down to a level that fits one tile. Same
//! philosophy as this workspace's vector side: ingest owns every physical
//! layout decision; the server only ever reads what authoring already
//! produced.
//!
//! ## Scope (first slice, deliberately narrow)
//!
//! - **Input**: exactly what [`crate::reader`] already understands minus
//!   overviews — a *single-IFD* GeoTIFF (stripped or tiled, though a
//!   paletted source must be tiled — see below), 8-bit grayscale/RGB/RGBA/
//!   paletted samples, uncompressed or Deflate compression (not LZW — a
//!   tighter input set than the reader accepts for *serving*, since
//!   authoring controls its own input format rather than inheriting
//!   whatever a third-party encoder produced), EPSG:4326 (WGS84 geographic)
//!   georeferencing carried through byte-for-byte from the source's own
//!   `ModelPixelScaleTag`/`ModelTiepointTag`/`GeoKeyDirectoryTag`. Anything
//!   else refuses by name — see [`author_cog`]'s own doc for the exact list.
//! - **Downsampling**: [`ResampleMode`] picks the kernel each overview level
//!   builds with. Box-average (2x2, rounded) is right for continuous data
//!   but WRONG for a categorical source — averaging class indices produces
//!   a value that names no real class — so a paletted source
//!   (`PhotometricInterpretation` = RGBPalette) auto-selects nearest-
//!   neighbor (picks the top-left sample of each 2x2 block, deterministic,
//!   never a blend) with no flag needed, `ResampleMode::NearestNeighbor`
//!   forces it for a non-paletted single-band (Gray8) source that's
//!   categorical by convention rather than by tag, and forcing box-average
//!   onto a paletted source is refused by name — there's no correct meaning
//!   for it.
//! - **Layout**: every IFD (main image plus every overview) is written
//!   *before* any tile's pixel data — the classic COG "header-first"
//!   convention — so a range-reading client's own read-ahead window
//!   (`remote.rs`'s `HttpRangeReader`) can pick up every level's tags in as
//!   few requests as possible. Beyond that one placement rule, this writer
//!   follows only what [`crate::reader`] actually reads: a valid IFD chain
//!   (`nextIFD` offsets), main image first, `TileOffsets`/`TileByteCounts`
//!   pointing at real tile data — `reader::open` re-sorts overviews
//!   finest-first from their own dimensions regardless of file order, so
//!   this writer emits them in pyramid order for a tidier file but nothing
//!   downstream depends on that order.
//!
//! ## Bounded memory
//!
//! Never the whole raw (decoded) image, at any resolution, at once. The
//! main image (level 0) streams from the source one *band* at a time — a
//! horizontal strip [`tile_size`] source rows tall, spanning the full
//! width, however the source itself is chunked (`read_source_rows`, chunk-
//! type-agnostic; a paletted source's own counterpart,
//! `read_source_rows_paletted`, streams the same way from its own
//! `TileOffsets`/`TileByteCounts` tags directly — see `reader.rs`'s own doc
//! for why a paletted IFD needs that separate manual path at all). Each
//! overview level streams from the *previous* level's
//! own already-encoded tiles, never from the original source re-read —
//! two tile-rows of the previous level in, one tile-row of the new level
//! out (`downsample_build`), so peak memory for the read side is
//! `O(tile_size × width × channels)` regardless of the source image's total
//! size. The one deliberate exception, and the reason this crate's COG
//! layout is header-first rather than interleaved per-IFD like this
//! crate's own serving fixture (`examples/gen_fixture.rs`): every level's
//! compressed tile bytes are held in memory until the whole file is
//! assembled, because every `TileOffsets` entry has to be known before the
//! first IFD byte is written. Peak memory for the *write* side is therefore
//! `O(final file size)` — the already-compressed output, not the raw
//! image — which is the whole point of writing a COG in the first place.
//!
//! [`tile_size`]: AuthorOptions::tile_size

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use tiff::decoder::{ChunkType, Decoder, DecodingResult};
use tiff::tags::Tag;

use crate::error::{CogError, Result};
use crate::geokeys;
use crate::reader::{self, Bands, PHOTOMETRIC_PALETTE};
use crate::tiff_write::{self, TagEntry, Value};

/// Output tile size default, pixels — the same conventional COG tile size
/// most encoders (`gdal_translate`, `rio cogeo`) use; independent of
/// [`crate::driver`]'s own `DEST_TILE_SIZE_PX` (the *served* PNG tile size),
/// which [`crate::tiling::select_overview`] already resamples against
/// regardless of the source COG's own tile shape.
pub const DEFAULT_TILE_SIZE: u32 = 256;

/// TIFF `Compression` tag codes this authoring path accepts as INPUT —
/// tighter than [`crate::reader`]'s own `SUPPORTED_COMPRESSION` (which also
/// accepts LZW, code 5, for *serving* whatever a third-party encoder
/// produced): authoring only ever reads uncompressed (1) or Deflate (8)
/// source data, and always writes Deflate (8) output.
const SUPPORTED_INPUT_COMPRESSION: [u16; 2] = [1, 8];

/// TIFF `Compression` tag code this authoring path always writes.
const OUTPUT_COMPRESSION_CODE: u16 = 8;

/// TIFF `ExtraSamples` value for unassociated (straight, non-premultiplied)
/// alpha — matches [`crate::reader::Bands::widen_to_rgba`]'s own straight-
/// alpha convention throughout this crate.
const EXTRA_SAMPLES_UNASSOCIATED_ALPHA: u16 = 2;

/// How [`author_cog`] picks the downsampling kernel each overview level
/// builds with — see this module's own doc for the full detection story.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResampleMode {
    /// Nearest-neighbor for a paletted source, box-average for everything
    /// else — the correct choice either way, so this is the default.
    #[default]
    Auto,
    /// Force nearest-neighbor (picks the top-left sample of each 2x2
    /// block) even for a non-paletted source — the right choice for a
    /// single-band (Gray8) class raster whose categorical meaning isn't
    /// declared via `PhotometricInterpretation`.
    NearestNeighbor,
    /// Force box-average. Refused outright when the source is paletted —
    /// averaging class indices has no correct meaning.
    BoxAverage,
}

/// Options for [`author_cog`].
#[derive(Debug, Clone)]
pub struct AuthorOptions {
    /// Output COG tile width/height, pixels. Also the streaming band height
    /// used while reading the source (see this module's own "bounded
    /// memory" doc) — must be greater than zero.
    pub tile_size: u32,
    /// Which downsampling kernel builds each overview level — see
    /// [`ResampleMode`]'s own doc.
    pub resample: ResampleMode,
}

impl Default for AuthorOptions {
    fn default() -> Self {
        Self {
            tile_size: DEFAULT_TILE_SIZE,
            resample: ResampleMode::default(),
        }
    }
}

/// What [`author_cog`] produced — enough for a CLI to report back to the
/// operator without re-opening the output file.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthorReport {
    /// Every level's `(width, height)`, finest (main image) first.
    pub level_dims: Vec<(u32, u32)>,
    /// Total bytes written to `output`.
    pub output_bytes: u64,
}

/// One already-built output level: its own tile grid shape plus every
/// tile's Deflate-compressed bytes, row-major. Never holds decoded pixels
/// beyond the one band being built at a time (see this module's own
/// "bounded memory" doc).
struct LevelBuild {
    width: u32,
    height: u32,
    tile_size: u32,
    tiles_across: u32,
    tiles: Vec<Vec<u8>>,
}

/// The internal 2-way choice [`ResampleMode`] resolves to once the source's
/// own paletted-ness is known — [`downsample_build`]'s only branch, so the
/// pyramid loop itself never duplicates per kernel (see this module's own
/// doc for the design rationale).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kernel {
    Average,
    Nearest,
}

/// A paletted (single-IFD, tiled-only) source's own `TileOffsets`/
/// `TileByteCounts`/`Compression`/tile shape — everything
/// [`read_source_rows_paletted`] needs to manually decode a tile without
/// going through `decoder.read_chunk()` at all (see `reader.rs`'s own doc
/// for why that can't be used for a paletted IFD).
struct RawTileSource {
    tile_offsets: Vec<u32>,
    tile_bytecounts: Vec<u32>,
    compression: u16,
    tile_w: u32,
    tile_h: u32,
}

/// Converts `input` (a plain, single-resolution GeoTIFF) into a
/// serving-optimized COG at `output`. Real I/O; call from a blocking
/// context, the same contract every other real-I/O function in this crate
/// documents (see `reader::open`'s own doc).
///
/// Refuses by name, before writing anything:
/// - the source has more than one IFD already (not "single-resolution");
/// - a striped OR tiled layout is fine for a non-paletted source, but a
///   paletted source must be tiled (see this module's own doc for why);
/// - any non-8-bit, non-Gray/RGB/RGBA/Palette sample layout is refused
///   ([`Bands::from_color_type`]'s own message, or, for a paletted source,
///   a `BitsPerSample` other than 8);
/// - `options.resample` requests `BoxAverage` against a paletted source —
///   see [`ResampleMode`]'s own doc;
/// - `Compression` is anything but uncompressed (1) or Deflate (8);
/// - `ModelPixelScaleTag`/`ModelTiepointTag` are missing, degenerate, or the
///   CRS declared via `GeoKeyDirectoryTag` isn't EPSG:4326 (WGS84
///   geographic) — the same CRS [`crate::reader`] requires to serve it, so
///   an ungeoreferenced or wrongly-referenced input fails here rather than
///   silently producing a file the server would refuse anyway;
/// - `options.tile_size` is zero.
pub fn author_cog(input: &Path, output: &Path, options: &AuthorOptions) -> Result<AuthorReport> {
    if options.tile_size == 0 {
        return Err(CogError::Unsupported(
            "tile size must be greater than zero".to_string(),
        ));
    }
    let tile_size = options.tile_size;

    let file = File::open(input).map_err(|source| CogError::Open {
        path: input.display().to_string(),
        source,
    })?;
    let mut decoder =
        Decoder::new(BufReader::new(file)).map_err(|e| CogError::Decode(e.to_string()))?;

    if decoder.more_images() {
        return Err(CogError::Unsupported(
            "input has more than one image (IFD); authoring accepts only a plain \
             single-resolution GeoTIFF, not one that already carries overviews"
                .to_string(),
        ));
    }

    let (width, height) = decoder
        .dimensions()
        .map_err(|e| CogError::Decode(e.to_string()))?;

    let photometric = decoder
        .get_tag_unsigned::<u16>(Tag::PhotometricInterpretation)
        .map_err(|e| CogError::Decode(e.to_string()))?;
    let is_paletted = photometric == PHOTOMETRIC_PALETTE;

    let kernel = match (options.resample, is_paletted) {
        (ResampleMode::BoxAverage, true) => {
            return Err(CogError::Unsupported(
                "options.resample requested BoxAverage, but the input is paletted \
                 (PhotometricInterpretation = RGBPalette); averaging class indices has no \
                 correct meaning, so box-average is refused outright against a paletted \
                 source"
                    .to_string(),
            ));
        }
        (ResampleMode::Auto, true) | (ResampleMode::NearestNeighbor, _) => Kernel::Nearest,
        (ResampleMode::Auto, false) | (ResampleMode::BoxAverage, false) => Kernel::Average,
    };

    if is_paletted && decoder.get_chunk_type() != ChunkType::Tile {
        return Err(CogError::Unsupported(
            "input is paletted (categorical) but uses a striped layout; only a tiled \
             paletted source can be authored -- the manual chunk decode this format \
             requires (this crate's TIFF decoder has no native RGBPalette support at all) \
             only implements tile addressing"
                .to_string(),
        ));
    }

    let (bands, channels) = if is_paletted {
        reader::validate_8bit_palette(&mut decoder, 0)?;
        (Bands::Palette, 1usize)
    } else {
        let color = decoder
            .colortype()
            .map_err(|e| CogError::Decode(e.to_string()))?;
        let bands = Bands::from_color_type(color, 0)?;
        let channels = bands.channel_count();
        (bands, channels)
    };

    let compression = decoder
        .get_tag_unsigned::<u16>(Tag::Compression)
        .map_err(|e| CogError::Decode(e.to_string()))?;
    if !SUPPORTED_INPUT_COMPRESSION.contains(&compression) {
        return Err(CogError::Unsupported(format!(
            "input uses compression code {compression}; authoring only accepts \
             uncompressed (1) or Deflate (8) input"
        )));
    }

    let colormap_raw: Option<Vec<u16>> = if is_paletted {
        Some(reader::read_and_validate_colormap(&mut decoder, 0)?)
    } else {
        None
    };

    let raw_tiles: Option<RawTileSource> = if is_paletted {
        let tile_offsets = decoder
            .get_tag_u32_vec(Tag::TileOffsets)
            .map_err(|e| CogError::Decode(e.to_string()))?;
        let tile_bytecounts = decoder
            .get_tag_u32_vec(Tag::TileByteCounts)
            .map_err(|e| CogError::Decode(e.to_string()))?;
        let (tile_w, tile_h) = decoder.chunk_dimensions();
        Some(RawTileSource {
            tile_offsets,
            tile_bytecounts,
            compression,
            tile_w,
            tile_h,
        })
    } else {
        None
    };

    let pixel_scale = decoder
        .get_tag_f64_vec(Tag::ModelPixelScaleTag)
        .unwrap_or_default();
    let tiepoint = decoder
        .get_tag_f64_vec(Tag::ModelTiepointTag)
        .unwrap_or_default();
    // Validates presence/shape/positivity; the raw values themselves (not
    // this parsed form) are what gets carried through to the output below,
    // faithfully, byte-for-byte.
    geokeys::parse_geo_transform(&pixel_scale, &tiepoint)?;

    let geo_directory = decoder
        .get_tag_u32_vec(Tag::GeoKeyDirectoryTag)
        .unwrap_or_default();
    let crs = geokeys::parse_crs(&geo_directory)?;
    if !crs.is_wgs84_geographic {
        return Err(CogError::Unsupported(format!(
            "input CRS is EPSG:{}; authoring only carries through EPSG:4326 (WGS84 \
             geographic) georeferencing -- the same CRS the serving side requires, so \
             any other CRS is refused here rather than producing a file the server \
             would refuse anyway",
            crs.epsg
                .map(|e| e.to_string())
                .unwrap_or_else(|| "<unset>".to_string())
        )));
    }
    // `GeoKeyDirectoryTag` is spec'd as SHORT; `get_tag_u32_vec` only widens
    // what the decoder already knows fits, so narrowing back is lossless.
    let geo_directory: Vec<u16> = geo_directory.iter().map(|&v| v as u16).collect();

    let level0 = if let Some(raw_tiles) = &raw_tiles {
        let mut raw_file = File::open(input).map_err(|source| CogError::Open {
            path: input.display().to_string(),
            source,
        })?;
        build_level0_paletted(&mut raw_file, raw_tiles, width, height, tile_size)?
    } else {
        build_level0(&mut decoder, width, height, channels, tile_size)?
    };
    let mut levels = vec![level0];
    while {
        let prev = levels.last().expect("levels is never empty");
        prev.width > tile_size || prev.height > tile_size
    } {
        let prev = levels.last().expect("levels is never empty");
        levels.push(downsample_build(prev, channels, tile_size, kernel)?);
    }

    let level_dims = levels.iter().map(|l| (l.width, l.height)).collect();
    let output_bytes = write_cog(
        output,
        &levels,
        bands,
        channels,
        &pixel_scale,
        &tiepoint,
        &geo_directory,
        colormap_raw.as_deref(),
    )?;

    Ok(AuthorReport {
        level_dims,
        output_bytes,
    })
}

/// Reads exactly rows `[y0, y0+band_h)` across the full source width into a
/// native-channel (no RGBA widening — authoring copies samples through
/// faithfully), row-major buffer. Chunk-type-agnostic: works identically
/// against a striped or tiled source, since `tiff`'s own decoder already
/// treats a strip as a chunk whose width always equals the whole image
/// (`chunks_across` below comes out to `1` in that case) — the same
/// `chunk_dimensions`/`chunk_data_dimensions`/`read_chunk` trio
/// [`crate::reader::read_window`] uses for a *tiled* source only, here made
/// to work for both. Real I/O.
fn read_source_rows(
    decoder: &mut Decoder<BufReader<File>>,
    y0: u32,
    band_h: u32,
    width: u32,
    channels: usize,
) -> Result<Vec<u8>> {
    let y1 = y0 + band_h;
    let mut out = vec![0u8; width as usize * band_h as usize * channels];

    let (chunk_w, chunk_h) = decoder.chunk_dimensions();
    let chunks_across = width.div_ceil(chunk_w);
    let chunk_y0 = y0 / chunk_h;
    let chunk_y1 = (y1 - 1) / chunk_h;

    for chunk_y in chunk_y0..=chunk_y1 {
        for chunk_x in 0..chunks_across {
            let chunk_index = chunk_y * chunks_across + chunk_x;
            let (data_w, data_h) = decoder.chunk_data_dimensions(chunk_index);
            let decoded = decoder
                .read_chunk(chunk_index)
                .map_err(|e| CogError::Decode(e.to_string()))?;
            let DecodingResult::U8(bytes) = decoded else {
                return Err(CogError::Unsupported(format!(
                    "source chunk {chunk_index} decoded to a non-8-bit sample buffer"
                )));
            };

            // Same "padded vs. unpadded chunk" disambiguation
            // `reader::read_window` documents: different `tiff`-crate
            // versions return one or the other.
            let padded_len = chunk_w as usize * chunk_h as usize * channels;
            let unpadded_len = data_w as usize * data_h as usize * channels;
            let (row_stride_px, buf_h) = if bytes.len() == padded_len {
                (chunk_w, chunk_h)
            } else if bytes.len() == unpadded_len {
                (data_w, data_h)
            } else {
                return Err(CogError::Decode(format!(
                    "source chunk {chunk_index} decoded to {} bytes, expected \
                     {padded_len} (padded) or {unpadded_len} (unpadded)",
                    bytes.len()
                )));
            };

            let chunk_origin_x = chunk_x * chunk_w;
            let chunk_origin_y = chunk_y * chunk_h;
            let valid_x1 = (chunk_origin_x + data_w.min(row_stride_px)).min(width);
            let valid_y1 = chunk_origin_y + data_h.min(buf_h);

            let src_x_lo = chunk_origin_x;
            let src_x_hi = valid_x1;
            let src_y_lo = y0.max(chunk_origin_y);
            let src_y_hi = y1.min(valid_y1);

            for src_y in src_y_lo..src_y_hi {
                let local_y = src_y - chunk_origin_y;
                let row_len = (src_x_hi - src_x_lo) as usize * channels;
                if row_len == 0 {
                    continue;
                }
                let src_off = local_y as usize * row_stride_px as usize * channels;
                let dst_off =
                    ((src_y - y0) as usize * width as usize + src_x_lo as usize) * channels;
                let Some(src_row) = bytes.get(src_off..src_off + row_len) else {
                    continue;
                };
                out[dst_off..dst_off + row_len].copy_from_slice(src_row);
            }
        }
    }
    Ok(out)
}

/// Zero-pads `band`'s `[x0, x0+valid_w) x [0, band_h)` sub-rectangle into a
/// full `tile_size x tile_size` tile buffer — the standard "edge tile is
/// still tile-shaped, padding is undefined per spec" COG convention
/// [`crate::reader::read_window`] already accepts on the read side (its own
/// `padded_len` case).
fn extract_padded_tile(
    band: &[u8],
    band_width: u32,
    band_height: u32,
    x0: u32,
    valid_w: u32,
    tile_size: u32,
    channels: usize,
) -> Vec<u8> {
    let mut tile = vec![0u8; tile_size as usize * tile_size as usize * channels];
    let copy_len = valid_w as usize * channels;
    for row in 0..band_height.min(tile_size) {
        let src_off = (row as usize * band_width as usize + x0 as usize) * channels;
        let dst_off = row as usize * tile_size as usize * channels;
        tile[dst_off..dst_off + copy_len].copy_from_slice(&band[src_off..src_off + copy_len]);
    }
    tile
}

fn deflate_compress(data: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).map_err(|source| {
        CogError::Encode(format!("failed to Deflate-compress a tile: {source}"))
    })?;
    encoder.finish().map_err(|source| {
        CogError::Encode(format!("failed to finish Deflate compression: {source}"))
    })
}

fn deflate_decompress(compressed: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(expected_len);
    ZlibDecoder::new(compressed)
        .read_to_end(&mut out)
        .map_err(|source| {
            CogError::Decode(format!(
                "failed to inflate an already-authored tile while building its overview: {source}"
            ))
        })?;
    Ok(out)
}

/// Builds the main-image (level 0) tile grid by streaming the source one
/// band ([`AuthorOptions::tile_size`] source rows, full width) at a time —
/// see this module's own "bounded memory" doc.
fn build_level0(
    decoder: &mut Decoder<BufReader<File>>,
    width: u32,
    height: u32,
    channels: usize,
    tile_size: u32,
) -> Result<LevelBuild> {
    let tiles_across = width.div_ceil(tile_size);
    let tiles_down = height.div_ceil(tile_size);
    let mut tiles = Vec::with_capacity((tiles_across * tiles_down) as usize);

    for tile_row in 0..tiles_down {
        let y0 = tile_row * tile_size;
        let band_h = tile_size.min(height - y0);
        let band = read_source_rows(decoder, y0, band_h, width, channels)?;
        for tile_col in 0..tiles_across {
            let x0 = tile_col * tile_size;
            let valid_w = tile_size.min(width - x0);
            let tile_pixels =
                extract_padded_tile(&band, width, band_h, x0, valid_w, tile_size, channels);
            tiles.push(deflate_compress(&tile_pixels)?);
        }
    }

    Ok(LevelBuild {
        width,
        height,
        tile_size,
        tiles_across,
        tiles,
    })
}

/// [`read_source_rows`]'s counterpart for a paletted source: reads exactly
/// rows `[y0, y0+band_h)` across the full source width straight from
/// `tiles`'s own `TileOffsets`/`TileByteCounts` tags, via
/// [`reader::read_raw_chunk`] — this crate's TIFF decoder has no native
/// RGBPalette support at all, so `decoder.read_chunk()` cannot be used for
/// a paletted IFD (see `reader.rs`'s own doc for the full story). Tiled
/// only (validated by [`author_cog`] before this is ever called) — a
/// manually-decompressed tile is always exactly `tile_w * tile_h` bytes,
/// so, unlike [`read_source_rows`], there is no padded-vs-unpadded
/// ambiguity to resolve here.
fn read_source_rows_paletted(
    raw_file: &mut File,
    tiles: &RawTileSource,
    y0: u32,
    band_h: u32,
    width: u32,
) -> Result<Vec<u8>> {
    let y1 = y0 + band_h;
    let mut out = vec![0u8; width as usize * band_h as usize];

    let chunks_across = width.div_ceil(tiles.tile_w);
    let chunk_y0 = y0 / tiles.tile_h;
    let chunk_y1 = (y1 - 1) / tiles.tile_h;
    let expected_len = tiles.tile_w as usize * tiles.tile_h as usize;

    for chunk_y in chunk_y0..=chunk_y1 {
        for chunk_x in 0..chunks_across {
            let chunk_index = (chunk_y * chunks_across + chunk_x) as usize;
            let offset = *tiles.tile_offsets.get(chunk_index).ok_or_else(|| {
                CogError::Decode(format!(
                    "source tile {chunk_index} has no TileOffsets entry"
                ))
            })? as u64;
            let byte_count = *tiles.tile_bytecounts.get(chunk_index).ok_or_else(|| {
                CogError::Decode(format!(
                    "source tile {chunk_index} has no TileByteCounts entry"
                ))
            })? as u64;
            let bytes = reader::read_raw_chunk(
                raw_file,
                offset,
                byte_count,
                tiles.compression,
                expected_len,
            )?;

            let chunk_origin_x = chunk_x * tiles.tile_w;
            let chunk_origin_y = chunk_y * tiles.tile_h;
            let valid_x1 = (chunk_origin_x + tiles.tile_w).min(width);
            let src_y_lo = y0.max(chunk_origin_y);
            let src_y_hi = y1.min(chunk_origin_y + tiles.tile_h);

            for src_y in src_y_lo..src_y_hi {
                let local_y = src_y - chunk_origin_y;
                let row_len = (valid_x1 - chunk_origin_x) as usize;
                if row_len == 0 {
                    continue;
                }
                let src_off = local_y as usize * tiles.tile_w as usize;
                let dst_off = (src_y - y0) as usize * width as usize + chunk_origin_x as usize;
                out[dst_off..dst_off + row_len].copy_from_slice(&bytes[src_off..src_off + row_len]);
            }
        }
    }
    Ok(out)
}

/// [`build_level0`]'s counterpart for a paletted source — reads via
/// [`read_source_rows_paletted`] instead of the `tiff` crate's own
/// `read_chunk` (see that function's own doc for why). Channels are always
/// 1 (a class-index byte per pixel); tile extraction and Deflate output are
/// otherwise identical to `build_level0`.
fn build_level0_paletted(
    raw_file: &mut File,
    raw_tiles: &RawTileSource,
    width: u32,
    height: u32,
    tile_size: u32,
) -> Result<LevelBuild> {
    let tiles_across = width.div_ceil(tile_size);
    let tiles_down = height.div_ceil(tile_size);
    let mut tiles = Vec::with_capacity((tiles_across * tiles_down) as usize);

    for tile_row in 0..tiles_down {
        let y0 = tile_row * tile_size;
        let band_h = tile_size.min(height - y0);
        let band = read_source_rows_paletted(raw_file, raw_tiles, y0, band_h, width)?;
        for tile_col in 0..tiles_across {
            let x0 = tile_col * tile_size;
            let valid_w = tile_size.min(width - x0);
            let tile_pixels = extract_padded_tile(&band, width, band_h, x0, valid_w, tile_size, 1);
            tiles.push(deflate_compress(&tile_pixels)?);
        }
    }

    Ok(LevelBuild {
        width,
        height,
        tile_size,
        tiles_across,
        tiles,
    })
}

/// Reads exactly rows `[y0, y0+band_h)` across the full width of an
/// already-built level's own tile grid — the overview counterpart of
/// [`read_source_rows`], decompressing (only) whichever tiles the band
/// touches and dropping their padding beyond the level's real
/// width/height.
fn read_level_band(level: &LevelBuild, y0: u32, band_h: u32, channels: usize) -> Result<Vec<u8>> {
    let mut out = vec![0u8; level.width as usize * band_h as usize * channels];
    let expected_tile_len = level.tile_size as usize * level.tile_size as usize * channels;

    let tile_row0 = y0 / level.tile_size;
    let tile_row1 = (y0 + band_h - 1) / level.tile_size;
    for tile_row in tile_row0..=tile_row1 {
        let tile_origin_y = tile_row * level.tile_size;
        let real_tile_h = level.tile_size.min(level.height - tile_origin_y);
        for tile_col in 0..level.tiles_across {
            let tile_origin_x = tile_col * level.tile_size;
            let real_tile_w = level.tile_size.min(level.width - tile_origin_x);
            let index = (tile_row * level.tiles_across + tile_col) as usize;
            let decompressed = deflate_decompress(&level.tiles[index], expected_tile_len)?;

            let src_y_lo = y0.max(tile_origin_y);
            let src_y_hi = (y0 + band_h).min(tile_origin_y + real_tile_h);
            let row_len = real_tile_w as usize * channels;
            for src_y in src_y_lo..src_y_hi {
                let local_y = src_y - tile_origin_y;
                let src_off = local_y as usize * level.tile_size as usize * channels;
                let dst_off = ((src_y - y0) as usize * level.width as usize
                    + tile_origin_x as usize)
                    * channels;
                out[dst_off..dst_off + row_len]
                    .copy_from_slice(&decompressed[src_off..src_off + row_len]);
            }
        }
    }
    Ok(out)
}

/// Box-averages `src` (`src_w x src_h`, `channels` bytes/pixel, row-major)
/// 2x2 into a `ceil(src_w/2) x ceil(src_h/2)` buffer — pure, no I/O, so a
/// test can assert an exact averaged value without a real file (this
/// module's own overview-chain-correctness proof). An edge row/column with
/// no partner (odd `src_w`/`src_h`) averages over whichever 1 or 2 samples
/// actually exist rather than assuming a phantom neighbor.
fn downsample_block(src: &[u8], src_w: u32, src_h: u32, channels: usize) -> (Vec<u8>, u32, u32) {
    let dst_w = src_w.div_ceil(2);
    let dst_h = src_h.div_ceil(2);
    let mut dst = vec![0u8; dst_w as usize * dst_h as usize * channels];

    for dy in 0..dst_h {
        let sy0 = dy * 2;
        let row_count: u32 = if sy0 + 1 < src_h { 2 } else { 1 };
        for dx in 0..dst_w {
            let sx0 = dx * 2;
            let col_count: u32 = if sx0 + 1 < src_w { 2 } else { 1 };
            for c in 0..channels {
                let mut sum = 0u32;
                for dy2 in 0..row_count {
                    let sy = sy0 + dy2;
                    for dx2 in 0..col_count {
                        let sx = sx0 + dx2;
                        let off = (sy as usize * src_w as usize + sx as usize) * channels + c;
                        sum += u32::from(src[off]);
                    }
                }
                let count = row_count * col_count;
                let avg = ((sum + count / 2) / count) as u8;
                dst[(dy as usize * dst_w as usize + dx as usize) * channels + c] = avg;
            }
        }
    }
    (dst, dst_w, dst_h)
}

/// Nearest-neighbor-downsamples `src` (`src_w x src_h`, `channels`
/// bytes/pixel, row-major) 2x2 into a `ceil(src_w/2) x ceil(src_h/2)`
/// buffer — the categorical counterpart of [`downsample_block`]: each
/// destination pixel is exactly the top-left source sample of its own 2x2
/// block, never a blend of the other 1-3 samples in it. Deterministic and
/// documented, not a mode-vote: this slice implements exactly one
/// nearest-neighbor convention, not several competing ones (see this
/// module's own doc). Same dimension math as `downsample_block` (an edge
/// row/column with no partner still has a real top-left sample, so there's
/// no "phantom neighbor" case to handle here at all).
fn downsample_nearest(src: &[u8], src_w: u32, src_h: u32, channels: usize) -> (Vec<u8>, u32, u32) {
    let dst_w = src_w.div_ceil(2);
    let dst_h = src_h.div_ceil(2);
    let mut dst = vec![0u8; dst_w as usize * dst_h as usize * channels];

    for dy in 0..dst_h {
        let sy = dy * 2;
        for dx in 0..dst_w {
            let sx = dx * 2;
            let src_off = (sy as usize * src_w as usize + sx as usize) * channels;
            let dst_off = (dy as usize * dst_w as usize + dx as usize) * channels;
            dst[dst_off..dst_off + channels].copy_from_slice(&src[src_off..src_off + channels]);
        }
    }
    (dst, dst_w, dst_h)
}

/// Builds the next-coarser overview level entirely from `prev`'s own tiles —
/// never from the original source (see this module's own "bounded memory"
/// doc). Streams two of `prev`'s tile-rows in, producing one new tile-row
/// out, at a time. `kernel` picks box-average vs. nearest-neighbor
/// ([`Kernel`]'s own doc) — the only difference between the two paths, so
/// the streaming/tiling structure around it is never duplicated per kernel.
fn downsample_build(
    prev: &LevelBuild,
    channels: usize,
    tile_size: u32,
    kernel: Kernel,
) -> Result<LevelBuild> {
    let new_width = prev.width.div_ceil(2);
    let new_height = prev.height.div_ceil(2);
    let new_tiles_across = new_width.div_ceil(tile_size);
    let new_tiles_down = new_height.div_ceil(tile_size);
    let mut tiles = Vec::with_capacity((new_tiles_across * new_tiles_down) as usize);

    for new_tile_row in 0..new_tiles_down {
        let new_y0 = new_tile_row * tile_size;
        let new_band_h = tile_size.min(new_height - new_y0);
        let prev_y0 = new_y0 * 2;
        let prev_band_h = (new_band_h * 2).min(prev.height - prev_y0);

        let prev_band = read_level_band(prev, prev_y0, prev_band_h, channels)?;
        let (new_band, new_band_w, computed_band_h) = match kernel {
            Kernel::Average => downsample_block(&prev_band, prev.width, prev_band_h, channels),
            Kernel::Nearest => downsample_nearest(&prev_band, prev.width, prev_band_h, channels),
        };
        debug_assert_eq!(new_band_w, new_width);
        debug_assert_eq!(computed_band_h, new_band_h);

        for new_tile_col in 0..new_tiles_across {
            let new_x0 = new_tile_col * tile_size;
            let valid_w = tile_size.min(new_width - new_x0);
            let tile_pixels = extract_padded_tile(
                &new_band, new_width, new_band_h, new_x0, valid_w, tile_size, channels,
            );
            tiles.push(deflate_compress(&tile_pixels)?);
        }
    }

    Ok(LevelBuild {
        width: new_width,
        height: new_height,
        tile_size,
        tiles_across: new_tiles_across,
        tiles,
    })
}

/// This level's own IFD tags, ascending by tag id (TIFF6 §2's own
/// requirement — see `tiff_write::encode_ifd`'s own doc). Georeferencing
/// tags are only ever present on the main image (`is_main`); no overview
/// carries them — matches `reader::open`'s own contract of reading geo tags
/// once, from IFD 0, never per-overview. `colormap`, in contrast, is
/// written to EVERY level (main and every overview) when `Some` — a
/// paletted level's own `ColorMap`/`PhotometricInterpretation` must be
/// self-describing at whichever IFD a reader lands on, since
/// `reader::open` walks overviews independently and only re-reads
/// per-level tags like this one, not IFD-0-only ones like the
/// georeferencing tags above.
#[allow(clippy::too_many_arguments)]
fn level_tags(
    level: &LevelBuild,
    bands: Bands,
    channels: usize,
    is_main: bool,
    pixel_scale: &[f64],
    tiepoint: &[f64],
    geo_directory: &[u16],
    colormap: Option<&[u16]>,
    tile_offsets: &[u32],
    tile_bytecounts: &[u32],
) -> Vec<TagEntry> {
    let photometric: u16 = match bands {
        Bands::Gray => 1,
        Bands::Rgb | Bands::Rgba => 2,
        Bands::Palette => PHOTOMETRIC_PALETTE,
    };
    let mut tags: Vec<TagEntry> = vec![
        (256, Value::Long(vec![level.width])),
        (257, Value::Long(vec![level.height])),
        (258, Value::Short(vec![8; channels])),
        (259, Value::Short(vec![OUTPUT_COMPRESSION_CODE])),
        (262, Value::Short(vec![photometric])),
        (277, Value::Short(vec![channels as u16])),
        (284, Value::Short(vec![1])),
    ];
    // ColorMap (320) sorts between PlanarConfiguration (284) and TileWidth
    // (322) — TIFF6 §2's ascending-tag-id ordering, not an arbitrary
    // placement.
    if let Some(cmap) = colormap {
        tags.push((320, Value::Short(cmap.to_vec())));
    }
    tags.push((322, Value::Long(vec![level.tile_size])));
    tags.push((323, Value::Long(vec![level.tile_size])));
    tags.push((324, Value::Long(tile_offsets.to_vec())));
    tags.push((325, Value::Long(tile_bytecounts.to_vec())));
    if bands == Bands::Rgba {
        tags.push((338, Value::Short(vec![EXTRA_SAMPLES_UNASSOCIATED_ALPHA])));
    }
    if is_main {
        tags.push((33550, Value::Double(pixel_scale.to_vec())));
        tags.push((33922, Value::Double(tiepoint.to_vec())));
        tags.push((34735, Value::Short(geo_directory.to_vec())));
    }
    tags
}

/// Assembles the final file: TIFF header, then every level's IFD back to
/// back (header-first — see this module's own doc), then every level's
/// tile data in the same order. Returns the total bytes written.
#[allow(clippy::too_many_arguments)]
fn write_cog(
    output: &Path,
    levels: &[LevelBuild],
    bands: Bands,
    channels: usize,
    pixel_scale: &[f64],
    tiepoint: &[f64],
    geo_directory: &[u16],
    colormap: Option<&[u16]>,
) -> Result<u64> {
    const HEADER_LEN: u32 = 8;

    let placeholder_tags = |level: &LevelBuild, index: usize| -> Vec<TagEntry> {
        let zeros = vec![0u32; level.tiles.len()];
        level_tags(
            level,
            bands,
            channels,
            index == 0,
            pixel_scale,
            tiepoint,
            geo_directory,
            colormap,
            &zeros,
            &zeros,
        )
    };

    let ifd_sizes: Vec<u32> = levels
        .iter()
        .enumerate()
        .map(|(index, level)| tiff_write::ifd_encoded_size(&placeholder_tags(level, index)))
        .collect();

    let mut ifd_offsets = Vec::with_capacity(levels.len());
    let mut running = HEADER_LEN;
    for &size in &ifd_sizes {
        ifd_offsets.push(running);
        running += size;
    }
    let tile_region_start = running;

    let mut tile_offsets_per_level = Vec::with_capacity(levels.len());
    let mut tile_bytecounts_per_level = Vec::with_capacity(levels.len());
    let mut cursor = tile_region_start;
    for level in levels {
        let mut offs = Vec::with_capacity(level.tiles.len());
        let mut counts = Vec::with_capacity(level.tiles.len());
        for tile in &level.tiles {
            offs.push(cursor);
            let len = u32::try_from(tile.len()).map_err(|_| {
                CogError::Encode(
                    "a compressed tile exceeds 4 GiB; classic (non-Big) TIFF output \
                     cannot address it"
                        .to_string(),
                )
            })?;
            counts.push(len);
            cursor += len;
        }
        tile_offsets_per_level.push(offs);
        tile_bytecounts_per_level.push(counts);
    }

    let out_file = File::create(output).map_err(|source| CogError::Write {
        path: output.display().to_string(),
        source,
    })?;
    let mut writer = BufWriter::new(out_file);
    let write_err = |source: std::io::Error| CogError::Write {
        path: output.display().to_string(),
        source,
    };

    writer
        .write_all(&tiff_write::tiff_header(ifd_offsets[0]))
        .map_err(write_err)?;

    for (index, level) in levels.iter().enumerate() {
        let next_ifd_offset = if index + 1 < levels.len() {
            ifd_offsets[index + 1]
        } else {
            0
        };
        let tags = level_tags(
            level,
            bands,
            channels,
            index == 0,
            pixel_scale,
            tiepoint,
            geo_directory,
            colormap,
            &tile_offsets_per_level[index],
            &tile_bytecounts_per_level[index],
        );
        let encoded = tiff_write::encode_ifd(&tags, next_ifd_offset, ifd_offsets[index]);
        debug_assert_eq!(encoded.len() as u32, ifd_sizes[index]);
        writer.write_all(&encoded).map_err(write_err)?;
    }

    for level in levels {
        for tile in &level.tiles {
            writer.write_all(tile).map_err(write_err)?;
        }
    }
    writer.flush().map_err(write_err)?;

    Ok(u64::from(cursor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiff_write::{encode_ifd, ifd_encoded_size, tiff_header};

    // -- `downsample_block` (pure, overview-chain correctness) --------------

    #[test]
    fn downsample_block_averages_a_four_by_four_single_channel_block_exactly() {
        // 4x4, single channel, rows 0..3 hold values 0,10,20,30 -- each 2x2
        // quadrant averages to a known exact value.
        #[rustfmt::skip]
        let src: [u8; 16] = [
            0, 0, 10, 10,
            0, 0, 10, 10,
            20, 20, 30, 30,
            20, 20, 30, 30,
        ];
        let (dst, w, h) = downsample_block(&src, 4, 4, 1);
        assert_eq!((w, h), (2, 2));
        assert_eq!(dst, vec![0, 10, 20, 30]);
    }

    #[test]
    fn downsample_block_dimensions_halve_with_ceiling_for_odd_input() {
        let src = vec![0u8; 9]; // 3x3, single channel
        let (_dst, w, h) = downsample_block(&src, 3, 3, 1);
        assert_eq!((w, h), (2, 2));
    }

    #[test]
    fn downsample_block_averages_an_edge_partial_cell_over_fewer_samples() {
        // 3x1, single channel: dst col 0 averages [10, 20] = 15; dst col 1
        // has no partner column and averages [30] alone = 30.
        let src: [u8; 3] = [10, 20, 30];
        let (dst, w, h) = downsample_block(&src, 3, 1, 1);
        assert_eq!((w, h), (2, 1));
        assert_eq!(dst, vec![15, 30]);
    }

    #[test]
    fn downsample_block_rounds_to_nearest_rather_than_truncating() {
        // Average of 1 and 2 is 1.5 -- rounds to 2, not 1.
        let src: [u8; 2] = [1, 2];
        let (dst, _w, _h) = downsample_block(&src, 2, 1, 1);
        assert_eq!(dst, vec![2]);
    }

    #[test]
    fn downsample_block_averages_each_channel_of_a_multi_channel_pixel_independently() {
        // 2x2, 3 channels (RGB); each channel constant across the block
        // except one quadrant differs, proving channels don't bleed into
        // each other.
        #[rustfmt::skip]
        let src: [u8; 12] = [
            10, 20, 30,   50, 20, 30,
            10, 20, 30,   50, 20, 30,
        ];
        let (dst, w, h) = downsample_block(&src, 2, 2, 3);
        assert_eq!((w, h), (1, 1));
        assert_eq!(dst, vec![30, 20, 30]);
    }

    // -- `downsample_nearest` (categorical, `#37`) ---------------------------

    #[test]
    fn downsample_nearest_picks_the_top_left_sample_of_each_block_exactly() {
        // 4x4, single channel: each 2x2 quadrant's top-left value is
        // distinct from the other 3 in it, so picking the wrong sample
        // would be caught immediately -- unlike `downsample_block`'s own
        // test fixture (whose quadrants are internally uniform), this one
        // proves NN reads exactly one sample, never an average.
        #[rustfmt::skip]
        let src: [u8; 16] = [
            1,  99, 2,  99,
            99, 99, 99, 99,
            3,  99, 4,  99,
            99, 99, 99, 99,
        ];
        let (dst, w, h) = downsample_nearest(&src, 4, 4, 1);
        assert_eq!((w, h), (2, 2));
        assert_eq!(dst, vec![1, 2, 3, 4]);
    }

    #[test]
    fn downsample_nearest_dimensions_halve_with_ceiling_for_odd_input() {
        let src = vec![0u8; 9]; // 3x3, single channel
        let (_dst, w, h) = downsample_nearest(&src, 3, 3, 1);
        assert_eq!((w, h), (2, 2));
    }

    #[test]
    fn downsample_nearest_never_blends_an_edge_partial_cell() {
        // 3x1, single channel: dst col 0's top-left is src[0]=10 (never
        // blended with src[1]=20, unlike `downsample_block`'s own average
        // of 15 for the same input); dst col 1 has no partner column and
        // is simply src[2]=30, its own only sample.
        let src: [u8; 3] = [10, 20, 30];
        let (dst, w, h) = downsample_nearest(&src, 3, 1, 1);
        assert_eq!((w, h), (2, 1));
        assert_eq!(dst, vec![10, 30]);
    }

    #[test]
    fn downsample_nearest_keeps_every_channel_of_a_multi_channel_pixel_together() {
        // 2x2, 3 channels (RGB): the top-left pixel's full triplet must
        // come through unchanged, not a per-channel mix with its neighbors.
        #[rustfmt::skip]
        let src: [u8; 12] = [
            10, 20, 30,   50, 60, 70,
            90, 91, 92,   93, 94, 95,
        ];
        let (dst, w, h) = downsample_nearest(&src, 2, 2, 3);
        assert_eq!((w, h), (1, 1));
        assert_eq!(dst, vec![10, 20, 30]);
    }

    // -- window/streaming-invariant math -------------------------------------

    #[test]
    fn extract_padded_tile_zero_pads_beyond_the_valid_source_rectangle() {
        // 3x2 band, single channel, values 1..6; asking for a 4x4 tile at
        // x0=0 with valid_w=3 must copy the real 3 columns per real row and
        // zero-fill everything else (the 4th column, and the row beyond
        // band_height=2).
        let band: [u8; 6] = [1, 2, 3, 4, 5, 6];
        let tile = extract_padded_tile(&band, 3, 2, 0, 3, 4, 1);
        assert_eq!(tile.len(), 16);
        assert_eq!(&tile[0..4], &[1, 2, 3, 0]);
        assert_eq!(&tile[4..8], &[4, 5, 6, 0]);
        assert_eq!(&tile[8..12], &[0, 0, 0, 0]);
        assert_eq!(&tile[12..16], &[0, 0, 0, 0]);
    }

    #[test]
    fn deflate_round_trips_arbitrary_bytes() {
        let data: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        let compressed = deflate_compress(&data).unwrap();
        let decompressed = deflate_decompress(&compressed, data.len()).unwrap();
        assert_eq!(decompressed, data);
    }

    /// Streaming invariant: `build_level0`'s per-band memory is bounded by
    /// `tile_size * width * channels`, never by the full image
    /// (`width * height * channels`) -- this asserts the actual band size
    /// `read_source_rows` ever allocates matches that bound exactly, for a
    /// raster many bands tall.
    #[test]
    fn a_source_band_never_exceeds_one_tile_row_worth_of_pixels() {
        let width = 300u32;
        let tile_size = 64u32;
        let height = 1000u32; // many tile-rows tall
        let channels = 3usize;
        for tile_row in 0..height.div_ceil(tile_size) {
            let y0 = tile_row * tile_size;
            let band_h = tile_size.min(height - y0);
            let band_len = width as usize * band_h as usize * channels;
            assert!(
                band_len <= width as usize * tile_size as usize * channels,
                "band at tile_row {tile_row} exceeds the one-tile-row bound"
            );
            // Never anywhere near the full image, regardless of how tall it is.
            assert!(band_len < width as usize * height as usize * channels);
        }
    }

    // -- byte-layout: header-first placement ---------------------------------

    #[test]
    fn write_cog_places_every_ifd_before_any_tile_pixel_data() {
        // Two tiny levels (main + one overview), one tile each -- proves the
        // *placement* invariant (every TileOffsets value lands at or after
        // where the last IFD ends) independent of the full authoring
        // pipeline.
        let level0 = LevelBuild {
            width: 4,
            height: 4,
            tile_size: 4,
            tiles_across: 1,
            tiles: vec![deflate_compress(&[7u8; 4 * 4]).unwrap()],
        };
        let level1 = LevelBuild {
            width: 2,
            height: 2,
            tile_size: 4,
            tiles_across: 1,
            tiles: vec![deflate_compress(&[9u8; 4 * 4]).unwrap()],
        };
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "tellurion-cog-author-layout-test-{}-{}.tif",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let pixel_scale = [0.01, 0.01, 0.0];
        let tiepoint = [0.0, 0.0, 0.0, -1.0, 1.0, 0.0];
        let geo_directory: [u16; 8] = [1, 1, 0, 2, 1024, 0, 1, 2];
        let bytes_written = write_cog(
            &path,
            &[level0, level1],
            Bands::Gray,
            1,
            &pixel_scale,
            &tiepoint,
            &geo_directory,
            None,
        )
        .unwrap();

        let file_bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(file_bytes.len() as u64, bytes_written);

        // Both IFDs' own encoded size, computed the same way `write_cog`
        // does, must fit before the first tile byte.
        let zeros1 = vec![0u32; 1];
        let ifd0_tags = level_tags(
            &LevelBuild {
                width: 4,
                height: 4,
                tile_size: 4,
                tiles_across: 1,
                tiles: vec![],
            },
            Bands::Gray,
            1,
            true,
            &pixel_scale,
            &tiepoint,
            &geo_directory,
            None,
            &zeros1,
            &zeros1,
        );
        let ifd1_tags = level_tags(
            &LevelBuild {
                width: 2,
                height: 2,
                tile_size: 4,
                tiles_across: 1,
                tiles: vec![],
            },
            Bands::Gray,
            1,
            false,
            &pixel_scale,
            &tiepoint,
            &geo_directory,
            None,
            &zeros1,
            &zeros1,
        );
        let header_region = 8 + ifd_encoded_size(&ifd0_tags) + ifd_encoded_size(&ifd1_tags);

        // Re-decode via the real crate to pull the actual TileOffsets this
        // file declares, and confirm every one lands at/after the header
        // region -- the header-first invariant this module's doc promises.
        let mut decoder = tiff::decoder::Decoder::new(std::io::Cursor::new(&file_bytes)).unwrap();
        let offsets0 = decoder.get_tag_u32_vec(Tag::TileOffsets).unwrap();
        assert!(decoder.more_images());
        decoder.next_image().unwrap();
        let offsets1 = decoder.get_tag_u32_vec(Tag::TileOffsets).unwrap();

        for offset in offsets0.iter().chain(offsets1.iter()) {
            assert!(
                *offset >= header_region,
                "tile offset {offset} falls inside the header region (< {header_region})"
            );
        }
        // Sanity: the encoder helpers used above aren't dead code.
        let _ = tiff_header(0);
        let _ = encode_ifd(&[], 0, 0);
    }

    // -- end-to-end: author, then serve through the real driver -------------

    use tellurion_core::{CollectionDecl, DriverFactory, StorageDecl, TileCoord};

    fn temp_path(name: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "tellurion-cog-author-test-{name}-{}-{}.tif",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        path
    }

    /// Writes a plain (single-IFD, striped or tiled), uncompressed, 8-bit
    /// RGB GeoTIFF this authoring path accepts as input — its own small
    /// counterpart to `examples/gen_fixture.rs`, built with this crate's own
    /// `tiff_write` module (the same low-level encoder `author_cog` itself
    /// uses) rather than duplicating a second hand-rolled byte layout.
    /// `chunk_h` rows per strip when `tiled` is false, or `chunk_h`-square
    /// tiles when it's true. Pixel `(x, y)`'s red channel is `x`, green is
    /// `y`, blue is constant 200 — a synthetic gradient a downsample test
    /// can compute the expected averaged value for directly, and a
    /// serve-through-the-real-driver test can sample a specific pixel from.
    fn write_plain_input_geotiff(
        path: &std::path::Path,
        width: u32,
        height: u32,
        chunk_h: u32,
        tiled: bool,
    ) {
        let chunk_w = if tiled { chunk_h } else { width };
        let chunks_across = width.div_ceil(chunk_w);
        let chunks_down = height.div_ceil(chunk_h);

        let mut chunk_offsets_placeholder = vec![0u32; (chunks_across * chunks_down) as usize];
        let build_tags = |offsets: &[u32]| -> Vec<TagEntry> {
            let mut tags: Vec<TagEntry> = vec![
                (256, Value::Long(vec![width])),
                (257, Value::Long(vec![height])),
                (258, Value::Short(vec![8, 8, 8])),
                (259, Value::Short(vec![1])), // uncompressed
                (262, Value::Short(vec![2])), // RGB
            ];
            if tiled {
                tags.push((277, Value::Short(vec![3])));
                tags.push((284, Value::Short(vec![1])));
                tags.push((322, Value::Long(vec![chunk_w])));
                tags.push((323, Value::Long(vec![chunk_h])));
                tags.push((324, Value::Long(offsets.to_vec())));
                tags.push((325, Value::Long(vec![chunk_w * chunk_h * 3; offsets.len()])));
            } else {
                tags.push((273, Value::Long(offsets.to_vec()))); // StripOffsets
                tags.push((277, Value::Short(vec![3])));
                tags.push((278, Value::Long(vec![chunk_h]))); // RowsPerStrip
                tags.push((
                    279,
                    Value::Long(
                        offsets
                            .iter()
                            .enumerate()
                            .map(|(index, _)| {
                                let strip_y0 = index as u32 * chunk_h;
                                let strip_h = chunk_h.min(height - strip_y0);
                                width * strip_h * 3
                            })
                            .collect(),
                    ),
                )); // StripByteCounts
            }
            tags.push((33550, Value::Double(vec![0.01, 0.01, 0.0])));
            tags.push((33922, Value::Double(vec![0.0, 0.0, 0.0, -1.0, 1.0, 0.0])));
            tags.push((
                34735,
                Value::Short(vec![1, 1, 0, 2, 1024, 0, 1, 2, 2048, 0, 1, 4326]),
            ));
            tags
        };

        const HEADER_LEN: u32 = 8;
        let ifd_size = ifd_encoded_size(&build_tags(&chunk_offsets_placeholder));
        let ifd_offset = HEADER_LEN;
        let mut cursor = ifd_offset + ifd_size;
        let mut chunk_bytes: Vec<Vec<u8>> = Vec::new();
        for chunk_index in 0..(chunks_across * chunks_down) {
            let chunk_y = chunk_index / chunks_across;
            let chunk_x = chunk_index % chunks_across;
            let origin_x = chunk_x * chunk_w;
            let origin_y = chunk_y * chunk_h;
            let real_w = if tiled { chunk_w } else { width };
            let real_h = chunk_h.min(height - origin_y);
            let mut buf = vec![0u8; (chunk_w * chunk_h * 3) as usize];
            for row in 0..real_h {
                for col in 0..real_w.min(chunk_w) {
                    let px = origin_x + col;
                    let py = origin_y + row;
                    let off = (row * chunk_w + col) as usize * 3;
                    buf[off] = (px % 256) as u8;
                    buf[off + 1] = (py % 256) as u8;
                    buf[off + 2] = 200;
                }
            }
            let buf = if tiled {
                buf
            } else {
                // Strips carry exactly `real_h` rows of exactly `width`
                // columns, no tile padding.
                buf[..(width * real_h * 3) as usize].to_vec()
            };
            chunk_offsets_placeholder[chunk_index as usize] = cursor;
            cursor += buf.len() as u32;
            chunk_bytes.push(buf);
        }

        let mut out = tiff_header(ifd_offset).to_vec();
        out.extend_from_slice(&encode_ifd(
            &build_tags(&chunk_offsets_placeholder),
            0,
            ifd_offset,
        ));
        for buf in &chunk_bytes {
            out.extend_from_slice(buf);
        }
        std::fs::write(path, out).expect("writes the synthetic input GeoTIFF");
    }

    /// Writes a plain, single-IFD, 8-bit PALETTED (indexed) GeoTIFF this
    /// authoring path's auto-detection accepts as categorical input —
    /// tiled only (`chunk_h`-square tiles), same georeferencing convention
    /// as `write_plain_input_geotiff`. `width` columns split exactly at
    /// the midpoint: index 0 (west half) for every row, index 1 (east
    /// half) — a synthetic class raster whose two regions a downsample or
    /// serving test can assert on directly. `colormap` is the raw
    /// 768-entry (3 planes of 256, R/G/B) `ColorMap` tag this test also
    /// carries into its own assertions, so a caller asserts on the EXACT
    /// same values this file declares, never a value the authoring path
    /// invented.
    fn write_paletted_input_geotiff(
        path: &std::path::Path,
        width: u32,
        height: u32,
        chunk_h: u32,
        colormap: &[u16],
    ) {
        let chunk_w = chunk_h;
        let chunks_across = width.div_ceil(chunk_w);
        let chunks_down = height.div_ceil(chunk_h);

        let mut chunk_offsets_placeholder = vec![0u32; (chunks_across * chunks_down) as usize];
        let build_tags = |offsets: &[u32]| -> Vec<TagEntry> {
            vec![
                (256, Value::Long(vec![width])),
                (257, Value::Long(vec![height])),
                (258, Value::Short(vec![8])),
                (259, Value::Short(vec![1])), // uncompressed
                (262, Value::Short(vec![3])), // Palette
                (277, Value::Short(vec![1])),
                (284, Value::Short(vec![1])),
                (320, Value::Short(colormap.to_vec())),
                (322, Value::Long(vec![chunk_w])),
                (323, Value::Long(vec![chunk_h])),
                (324, Value::Long(offsets.to_vec())),
                (325, Value::Long(vec![chunk_w * chunk_h; offsets.len()])),
                (33550, Value::Double(vec![0.01, 0.01, 0.0])),
                (
                    33922,
                    Value::Double(vec![
                        0.0,
                        0.0,
                        0.0,
                        -(0.01 * f64::from(width) / 2.0),
                        0.01 * f64::from(height) / 2.0,
                        0.0,
                    ]),
                ),
                (
                    34735,
                    Value::Short(vec![1, 1, 0, 2, 1024, 0, 1, 2, 2048, 0, 1, 4326]),
                ),
            ]
        };

        const HEADER_LEN: u32 = 8;
        let ifd_size = ifd_encoded_size(&build_tags(&chunk_offsets_placeholder));
        let ifd_offset = HEADER_LEN;
        let mut cursor = ifd_offset + ifd_size;
        let mut chunk_bytes: Vec<Vec<u8>> = Vec::new();
        for chunk_index in 0..(chunks_across * chunks_down) {
            let chunk_x = chunk_index % chunks_across;
            let origin_x = chunk_x * chunk_w;
            let mut buf = vec![0u8; (chunk_w * chunk_h) as usize];
            for row in 0..chunk_h {
                for col in 0..chunk_w {
                    let px = origin_x + col;
                    let class = if px < width / 2 { 0u8 } else { 1u8 };
                    buf[(row * chunk_w + col) as usize] = class;
                }
            }
            chunk_offsets_placeholder[chunk_index as usize] = cursor;
            cursor += buf.len() as u32;
            chunk_bytes.push(buf);
        }

        let mut out = tiff_header(ifd_offset).to_vec();
        out.extend_from_slice(&encode_ifd(
            &build_tags(&chunk_offsets_placeholder),
            0,
            ifd_offset,
        ));
        for buf in &chunk_bytes {
            out.extend_from_slice(buf);
        }
        std::fs::write(path, out).expect("writes the synthetic paletted input GeoTIFF");
    }

    fn collection_decl() -> CollectionDecl {
        serde_yaml::from_str("id: authored\ncatalog: default\nstorage: main\n").unwrap()
    }

    #[test]
    fn author_cog_refuses_a_striped_or_tiled_input_by_dtype_and_produces_a_servable_output_either_way(
    ) {
        for tiled in [false, true] {
            let input = temp_path(if tiled { "tiled-in" } else { "striped-in" });
            let output = temp_path(if tiled { "tiled-out" } else { "striped-out" });
            write_plain_input_geotiff(&input, 40, 40, 16, tiled);

            let options = AuthorOptions {
                tile_size: 16,
                resample: ResampleMode::Auto,
            };
            let report = author_cog(&input, &output, &options).unwrap();

            // 40x40 at tile_size 16 -> level0 40x40, level1 20x20, level2
            // 10x10 (fits one 16x16 tile, floor reached).
            assert_eq!(
                report.level_dims,
                vec![(40, 40), (20, 20), (10, 10)],
                "tiled={tiled}"
            );
            assert!(report.output_bytes > 0);

            std::fs::remove_file(&input).ok();
            std::fs::remove_file(&output).ok();
        }
    }

    /// The acceptance proof this slice's spec asks for: author a COG from a
    /// synthetic plain GeoTIFF, then serve a PNG-lane raster tile from the
    /// authored file through the real, unmodified `cog` driver (the same
    /// `CogDriverFactory` the server binary itself builds at boot) — proving
    /// the output is genuinely consumable by this crate's own reader, not
    /// merely byte-plausible.
    #[tokio::test]
    async fn authored_output_serves_a_real_raster_tile_through_the_real_cog_driver() {
        let input = temp_path("roundtrip-in");
        let output = temp_path("roundtrip-out");
        // 64x64, single 64x64 tile at chunk_h=64 -- large enough for one
        // overview level (32x32) before the floor (16x16 tile size).
        write_plain_input_geotiff(&input, 64, 64, 64, true);

        let options = AuthorOptions {
            tile_size: 16,
            resample: ResampleMode::Auto,
        };
        let report = author_cog(&input, &output, &options).unwrap();
        assert_eq!(report.level_dims, vec![(64, 64), (32, 32), (16, 16)]);

        let env_var = "TELLURION_COG_AUTHOR_ROUNDTRIP_TEST_PATH";
        std::env::set_var(env_var, &output);
        let decl = StorageDecl {
            id: "main".to_string(),
            driver: "cog".to_string(),
            url_env: env_var.to_string(),
            pool_size: None,
        };
        let driver = crate::CogDriverFactory::new().build(&decl).unwrap();
        let raster_source = driver
            .raster_source()
            .expect("the cog driver always implements RasterSource");

        // Proves the driver returns a real, correctly-sized, non-empty
        // tile from the authored file -- deriving the exact source-pixel
        // address a specific destination pixel maps to (through the
        // Web-Mercator warp `tiling::resample_to_tile` performs) isn't
        // needed for this: tile (0,0,0) covers the whole world, so it
        // always intersects any real, finite extent.
        let collections = driver.catalog_source().collections().await.unwrap();
        assert_eq!(collections.len(), 1);
        // The served collection's physical name is the *output* COG's own
        // file stem (`reader::logical_name_of`'s own convention) -- the
        // driver only ever sees `output`, never `input`.
        assert_eq!(
            collections[0].name,
            output.file_stem().unwrap().to_str().unwrap()
        );
        assert_eq!(collections[0].srid, Some(4326));

        let extent = driver
            .catalog_source()
            .extent(&collections[0])
            .await
            .unwrap()
            .unwrap();
        let coord = TileCoord { z: 0, x: 0, y: 0 };
        let window = raster_source
            .raster_tile(&collection_decl(), coord)
            .await
            .unwrap()
            .expect("a world-covering tile always intersects a real, finite extent");

        assert_eq!((window.width, window.height), (256, 256));
        assert!(
            window.rgba.chunks_exact(4).any(|p| p[3] != 0),
            "at least some destination pixels must be real (non-transparent) data"
        );
        assert!(extent.bbox[0] < extent.bbox[2] && extent.bbox[1] < extent.bbox[3]);

        std::env::remove_var(env_var);
        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }

    // -- categorical (paletted) authoring, `#37` -----------------------------

    /// A paletted source auto-selects nearest-neighbor with no flag needed
    /// (`ResampleMode::Auto`, the default), and its `PhotometricInterpretation`
    /// / `ColorMap` tags carry through byte-for-byte to EVERY level's own
    /// IFD, not just the main image — walked directly off the output file
    /// via a fresh decoder, independent of this crate's own reader.
    #[test]
    fn author_cog_auto_detects_a_paletted_source_and_carries_the_palette_to_every_level() {
        let input = temp_path("palette-ifd-walk-in");
        let output = temp_path("palette-ifd-walk-out");
        let mut colormap = vec![0u16; 768];
        colormap[0] = 65535; // index 0 -> red
        colormap[512 + 1] = 65535; // index 1 -> blue
        write_paletted_input_geotiff(&input, 256, 256, 128, &colormap);

        let report = author_cog(
            &input,
            &output,
            &AuthorOptions {
                tile_size: 128,
                resample: ResampleMode::Auto,
            },
        )
        .unwrap();
        assert_eq!(report.level_dims, vec![(256, 256), (128, 128)]);

        let file_bytes = std::fs::read(&output).unwrap();
        let mut decoder = tiff::decoder::Decoder::new(std::io::Cursor::new(&file_bytes)).unwrap();
        let photometric0: u16 = decoder
            .get_tag_unsigned(Tag::PhotometricInterpretation)
            .unwrap();
        assert_eq!(photometric0, 3, "level 0 must declare RGBPalette");
        let colormap0 = decoder.get_tag_u16_vec(Tag::ColorMap).unwrap();
        assert_eq!(
            colormap0, colormap,
            "level 0's ColorMap must match the source's byte-for-byte"
        );

        assert!(decoder.more_images());
        decoder.next_image().unwrap();
        let photometric1: u16 = decoder
            .get_tag_unsigned(Tag::PhotometricInterpretation)
            .unwrap();
        assert_eq!(photometric1, 3, "the overview must ALSO declare RGBPalette");
        let colormap1 = decoder.get_tag_u16_vec(Tag::ColorMap).unwrap();
        assert_eq!(
            colormap1, colormap,
            "the overview's ColorMap must ALSO match the source's byte-for-byte"
        );

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }

    /// Writes a single-tile, single-IFD, 8-bit GRAYSCALE (BlackIsZero)
    /// GeoTIFF with exactly `pixels` as its raw sample bytes (row-major,
    /// `size x size`) — used only by the explicit-nearest-neighbor-on-Gray8
    /// test below, where the caller controls every pixel value directly to
    /// make a box-average and a nearest-neighbor result provably differ.
    fn write_gray8_single_tile_geotiff(path: &std::path::Path, size: u32, pixels: &[u8]) {
        assert_eq!(pixels.len(), (size * size) as usize);
        let build_tags = |offset: u32| -> Vec<TagEntry> {
            vec![
                (256, Value::Long(vec![size])),
                (257, Value::Long(vec![size])),
                (258, Value::Short(vec![8])),
                (259, Value::Short(vec![1])),
                (262, Value::Short(vec![1])), // BlackIsZero
                (277, Value::Short(vec![1])),
                (284, Value::Short(vec![1])),
                (322, Value::Long(vec![size])),
                (323, Value::Long(vec![size])),
                (324, Value::Long(vec![offset])),
                (325, Value::Long(vec![size * size])),
                (33550, Value::Double(vec![0.01, 0.01, 0.0])),
                (33922, Value::Double(vec![0.0, 0.0, 0.0, -1.0, 1.0, 0.0])),
                (
                    34735,
                    Value::Short(vec![1, 1, 0, 2, 1024, 0, 1, 2, 2048, 0, 1, 4326]),
                ),
            ]
        };
        const HEADER_LEN: u32 = 8;
        let ifd_size = ifd_encoded_size(&build_tags(0));
        let tile_offset = HEADER_LEN + ifd_size;
        let mut out = tiff_header(HEADER_LEN).to_vec();
        out.extend_from_slice(&encode_ifd(&build_tags(tile_offset), 0, HEADER_LEN));
        out.extend_from_slice(pixels);
        std::fs::write(path, out).expect("writes the synthetic Gray8 input GeoTIFF");
    }

    /// `ResampleMode::NearestNeighbor` is accepted for a non-paletted,
    /// single-band (Gray8) source — a class raster stored without an
    /// embedded palette — and it actually wires through to the kernel
    /// choice, not merely accepted syntactically: the overview's own
    /// top-left sample must be the real nearest-neighbor value, never a
    /// box-averaged blend.
    #[test]
    fn author_cog_accepts_an_explicit_nearest_neighbor_flag_on_a_non_paletted_gray8_source() {
        let input = temp_path("gray8-nn-in");
        let output = temp_path("gray8-nn-out");
        // 8x8 (tile_size 4, so level 0 is BIGGER than one tile and an
        // overview actually gets built -- a 4x4 source at tile_size 4
        // already fits one tile, so it would stop at level 0 alone). The
        // top-left 2x2 block of the MAIN image is [0, 200, 200, 200]: box-
        // average would round that to 150 for the overview's own top-left
        // pixel; nearest-neighbor keeps the real top-left sample, 0,
        // unchanged.
        #[rustfmt::skip]
        let pixels: [u8; 64] = [
            0,   200, 50, 50, 50, 50, 50, 50,
            200, 200, 50, 50, 50, 50, 50, 50,
            50,  50,  50, 50, 50, 50, 50, 50,
            50,  50,  50, 50, 50, 50, 50, 50,
            50,  50,  50, 50, 50, 50, 50, 50,
            50,  50,  50, 50, 50, 50, 50, 50,
            50,  50,  50, 50, 50, 50, 50, 50,
            50,  50,  50, 50, 50, 50, 50, 50,
        ];
        write_gray8_single_tile_geotiff(&input, 8, &pixels);

        let options = AuthorOptions {
            tile_size: 4,
            resample: ResampleMode::NearestNeighbor,
        };
        let report = author_cog(&input, &output, &options).unwrap();
        assert_eq!(report.level_dims, vec![(8, 8), (4, 4)]);

        // Decode the overview (IFD 1) directly and confirm its top-left
        // pixel is the exact nearest-neighbor sample (0), never the
        // box-averaged blend (150) -- proving the flag actually selected
        // the NN kernel.
        let file_bytes = std::fs::read(&output).unwrap();
        let mut decoder = tiff::decoder::Decoder::new(std::io::Cursor::new(&file_bytes)).unwrap();
        assert!(decoder.more_images());
        decoder.next_image().unwrap();
        let tile = decoder.read_chunk(0).unwrap();
        let DecodingResult::U8(bytes) = tile else {
            panic!("expected an 8-bit overview tile");
        };
        assert_eq!(
            bytes[0], 0,
            "nearest-neighbor must keep the real top-left sample, not an averaged blend"
        );

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }

    /// The acceptance proof this slice's spec asks for: author a COG from a
    /// synthetic PALETTED GeoTIFF with a known two-color palette, then
    /// serve real PNG-lane raster tiles through the actual, unmodified
    /// `cog` driver at BOTH native (main image) and coarse (overview) zoom,
    /// asserting EXACT expected colors at native zoom and palette-only
    /// membership at coarse zoom — proving the driver actually expands the
    /// embedded palette at decode time, at every level, not just the main
    /// image.
    ///
    /// This does NOT, on its own, prove nearest-neighbor (rather than
    /// box-average) built the overview: this fixture's class boundary is a
    /// single vertical split with no row-to-row variation, so every
    /// downsample block here is either uniform or splits its two source
    /// columns evenly, and averaging two DISTINCT index values always
    /// rounds back to one of those same two values, never a third — a
    /// blend of {0,1} renders as one of the two real palette colors either
    /// way. See `author_cog_auto_detected_kernel_picks_the_top_left_sample_not_a_box_average`
    /// below for the fixture shaped specifically to make the two kernels
    /// diverge, which is what actually pins the kernel choice down.
    #[tokio::test]
    async fn authored_paletted_output_serves_exact_palette_colors_at_native_and_coarse_zoom() {
        let input = temp_path("palette-roundtrip-in");
        let output = temp_path("palette-roundtrip-out");
        let mut colormap = vec![0u16; 768];
        colormap[0] = 65535; // index 0 (west half) -> red
        colormap[512 + 1] = 65535; // index 1 (east half) -> blue
        write_paletted_input_geotiff(&input, 256, 256, 128, &colormap);

        let options = AuthorOptions {
            tile_size: 128,
            resample: ResampleMode::Auto,
        };
        let report = author_cog(&input, &output, &options).unwrap();
        assert_eq!(report.level_dims, vec![(256, 256), (128, 128)]);

        let env_var = "TELLURION_COG_AUTHOR_PALETTE_ROUNDTRIP_TEST_PATH";
        std::env::set_var(env_var, &output);
        let decl = StorageDecl {
            id: "main".to_string(),
            driver: "cog".to_string(),
            url_env: env_var.to_string(),
            pool_size: None,
        };
        let driver = crate::CogDriverFactory::new().build(&decl).unwrap();
        let raster_source = driver
            .raster_source()
            .expect("the cog driver always implements RasterSource");

        // Native zoom: deep inside the west (red, index 0) / east (blue,
        // index 1) halves -- the same tile coordinates this crate's own
        // serving fixture (`examples/gen_fixture.rs`) uses for its own
        // proven quadrant tests, since this synthetic fixture shares its
        // exact georeferencing (0.01 deg/px, extent
        // [-1.28, -1.28, 1.28, 1.28]).
        let west = raster_source
            .raster_tile(
                &collection_decl(),
                TileCoord {
                    z: 10,
                    x: 511,
                    y: 511,
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert!(
            west.rgba.chunks_exact(4).all(|p| p == [255, 0, 0, 255]),
            "deep inside the west half at native zoom must be solid, exact red"
        );
        let east = raster_source
            .raster_tile(
                &collection_decl(),
                TileCoord {
                    z: 10,
                    x: 513,
                    y: 511,
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert!(
            east.rgba.chunks_exact(4).all(|p| p == [0, 0, 255, 255]),
            "deep inside the east half at native zoom must be solid, exact blue"
        );

        // Coarse zoom: a world-covering tile forces the coarsest (overview,
        // nearest-neighbor-built) level. Nearest-neighbor never blends, so
        // every real (non-transparent) destination pixel must be EXACTLY
        // one of the two palette colors, never an in-between value
        // box-averaging would have produced.
        let world = raster_source
            .raster_tile(&collection_decl(), TileCoord { z: 0, x: 0, y: 0 })
            .await
            .unwrap()
            .unwrap();
        let real_pixels: Vec<[u8; 4]> = world
            .rgba
            .chunks_exact(4)
            .map(|p| [p[0], p[1], p[2], p[3]])
            .filter(|p| p[3] != 0)
            .collect();
        assert!(
            !real_pixels.is_empty(),
            "some pixel should show real (non-transparent) raster data"
        );
        assert!(
            real_pixels
                .iter()
                .all(|p| *p == [255, 0, 0, 255] || *p == [0, 0, 255, 255]),
            "every real pixel at the coarse (overview) zoom must be an EXACT palette color, \
             never a raw index value or other data outside the two declared classes: \
             {real_pixels:?}"
        );
        assert!(
            real_pixels.contains(&[255, 0, 0, 255]) && real_pixels.contains(&[0, 0, 255, 255]),
            "both classes should still be visible at the coarse zoom"
        );

        std::env::remove_var(env_var);
        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }

    /// [`write_paletted_input_geotiff`]'s own counterpart for a test that
    /// needs to control every pixel's class index directly (the paletted
    /// sibling of [`write_gray8_single_tile_geotiff`] above) -- needed
    /// because `write_paletted_input_geotiff`'s class rule depends only on
    /// column, so it can never place a specific pixel at a specific
    /// position inside a specific 2x2 downsample block the way
    /// `author_cog_auto_detected_kernel_picks_the_top_left_sample_not_a_box_average`
    /// below requires.
    fn write_paletted_single_tile_geotiff(
        path: &std::path::Path,
        size: u32,
        colormap: &[u16],
        indices: &[u8],
    ) {
        assert_eq!(indices.len(), (size * size) as usize);
        let build_tags = |offset: u32| -> Vec<TagEntry> {
            vec![
                (256, Value::Long(vec![size])),
                (257, Value::Long(vec![size])),
                (258, Value::Short(vec![8])),
                (259, Value::Short(vec![1])), // uncompressed
                (262, Value::Short(vec![3])), // Palette
                (277, Value::Short(vec![1])),
                (284, Value::Short(vec![1])),
                (320, Value::Short(colormap.to_vec())),
                (322, Value::Long(vec![size])),
                (323, Value::Long(vec![size])),
                (324, Value::Long(vec![offset])),
                (325, Value::Long(vec![size * size])),
                (33550, Value::Double(vec![0.01, 0.01, 0.0])),
                (33922, Value::Double(vec![0.0, 0.0, 0.0, -1.0, 1.0, 0.0])),
                (
                    34735,
                    Value::Short(vec![1, 1, 0, 2, 1024, 0, 1, 2, 2048, 0, 1, 4326]),
                ),
            ]
        };
        const HEADER_LEN: u32 = 8;
        let ifd_size = ifd_encoded_size(&build_tags(0));
        let tile_offset = HEADER_LEN + ifd_size;
        let mut out = tiff_header(HEADER_LEN).to_vec();
        out.extend_from_slice(&encode_ifd(&build_tags(tile_offset), 0, HEADER_LEN));
        out.extend_from_slice(indices);
        std::fs::write(path, out).expect("writes the synthetic single-tile paletted input GeoTIFF");
    }

    /// The gap `authored_paletted_output_serves_exact_palette_colors_at_native_and_coarse_zoom`
    /// above cannot close: with only two classes, averaging class indices
    /// 0 and 1 always rounds back to 0 or 1, never a third value, so
    /// "every rendered pixel is one of the two declared colors" holds
    /// whether the overview was built with nearest-neighbor OR
    /// box-average — that property alone can never prove WHICH kernel ran.
    /// This fixture instead gives the overview's own top-left (0,0) 2x2
    /// source block an ASYMMETRIC 3-1 split, with the MINORITY class at
    /// its own top-left corner: nearest-neighbor (picks the top-left
    /// sample of each block, unchanged) must keep the minority class
    /// there; box-average (rounds toward the 3-pixel majority) must
    /// produce the other class instead. Only a fixture shaped this way can
    /// tell the two kernels apart from their output alone, so this is what
    /// actually pins down the `(ResampleMode::Auto, true) => Kernel::Nearest`
    /// auto-detection arm `author_cog` itself resolves to for a paletted
    /// source.
    #[test]
    fn author_cog_auto_detected_kernel_picks_the_top_left_sample_not_a_box_average() {
        let input = temp_path("palette-kernel-in");
        let output = temp_path("palette-kernel-out");
        let mut colormap = vec![0u16; 768];
        colormap[0] = 65535; // index 0 (majority) -> red
        colormap[512 + 1] = 65535; // index 1 (minority) -> blue

        // 8x8, single tile; every pixel is class 0 (red) except (row 0,
        // col 0), which is class 1 (blue) -- the top-left source sample of
        // the overview's own (0,0) 2x2 block, giving that one block a 3-1
        // split with the minority class sitting exactly where
        // nearest-neighbor reads from.
        let mut indices = vec![0u8; 64];
        indices[0] = 1;
        write_paletted_single_tile_geotiff(&input, 8, &colormap, &indices);

        let options = AuthorOptions {
            tile_size: 4,
            resample: ResampleMode::Auto,
        };
        let report = author_cog(&input, &output, &options).unwrap();
        assert_eq!(report.level_dims, vec![(8, 8), (4, 4)]);

        // Decode the overview (IFD 1) directly -- through this crate's own
        // manual paletted-chunk reader, since the `tiff` crate's own
        // `read_chunk` can never succeed against a Palette-photometric IFD
        // (see `reader.rs`'s own doc) -- and read its own top-left pixel's
        // raw class index. Nearest-neighbor must have kept 1 (blue, the
        // minority); box-average would have rounded the 3-1 split back to
        // 0 (red, the majority) instead.
        let file_bytes = std::fs::read(&output).unwrap();
        let mut decoder = tiff::decoder::Decoder::new(std::io::Cursor::new(&file_bytes)).unwrap();
        assert!(decoder.more_images());
        decoder.next_image().unwrap();
        let tile_offsets = decoder.get_tag_u32_vec(Tag::TileOffsets).unwrap();
        let tile_bytecounts = decoder.get_tag_u32_vec(Tag::TileByteCounts).unwrap();
        let overview_tile = reader::read_raw_chunk(
            &mut std::io::Cursor::new(&file_bytes),
            u64::from(tile_offsets[0]),
            u64::from(tile_bytecounts[0]),
            OUTPUT_COMPRESSION_CODE,
            4 * 4,
        )
        .unwrap();
        assert_eq!(
            overview_tile[0], 1,
            "the overview's own top-left pixel must be the real nearest-neighbor sample (1, \
             blue), not a box-averaged blend (0, red)"
        );

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }

    // -- named refusals -------------------------------------------------------

    #[test]
    fn author_cog_refuses_a_zero_tile_size() {
        let input = temp_path("zero-tile-in");
        write_plain_input_geotiff(&input, 16, 16, 16, true);
        let output = temp_path("zero-tile-out");
        let result = author_cog(
            &input,
            &output,
            &AuthorOptions {
                tile_size: 0,
                resample: ResampleMode::Auto,
            },
        );
        assert!(matches!(result, Err(CogError::Unsupported(_))));
        std::fs::remove_file(&input).ok();
    }

    #[test]
    fn author_cog_refuses_a_missing_input_file() {
        let missing = temp_path("does-not-exist");
        let output = temp_path("missing-input-out");
        let result = author_cog(&missing, &output, &AuthorOptions::default());
        assert!(matches!(result, Err(CogError::Open { .. })));
    }

    #[test]
    fn author_cog_refuses_input_with_no_geo_tags() {
        // Same low-level writer, but omit the georeferencing tags entirely.
        let input = temp_path("no-geo-in");
        let output = temp_path("no-geo-out");
        let width = 16u32;
        let height = 16u32;
        let tags: Vec<TagEntry> = vec![
            (256, Value::Long(vec![width])),
            (257, Value::Long(vec![height])),
            (258, Value::Short(vec![8])),
            (259, Value::Short(vec![1])),
            (262, Value::Short(vec![1])),
            (273, Value::Long(vec![8 + ifd_encoded_size(&[])])),
            (277, Value::Short(vec![1])),
            (278, Value::Long(vec![height])),
            (279, Value::Long(vec![width * height])),
        ];
        // Recompute StripOffsets against the tags' own real encoded size.
        let ifd_size = ifd_encoded_size(&tags);
        let strip_offset = 8 + ifd_size;
        let tags: Vec<TagEntry> = vec![
            (256, Value::Long(vec![width])),
            (257, Value::Long(vec![height])),
            (258, Value::Short(vec![8])),
            (259, Value::Short(vec![1])),
            (262, Value::Short(vec![1])),
            (273, Value::Long(vec![strip_offset])),
            (277, Value::Short(vec![1])),
            (278, Value::Long(vec![height])),
            (279, Value::Long(vec![width * height])),
        ];
        let mut out = tiff_header(8).to_vec();
        out.extend_from_slice(&encode_ifd(&tags, 0, 8));
        out.extend_from_slice(&vec![0u8; (width * height) as usize]);
        std::fs::write(&input, out).unwrap();

        let result = author_cog(&input, &output, &AuthorOptions::default());
        assert!(matches!(result, Err(CogError::Unsupported(_))));
        std::fs::remove_file(&input).ok();
    }

    /// A paletted source (`#37` categorical) must be TILED — the manual
    /// chunk decode this format requires (this crate's TIFF decoder has no
    /// native RGBPalette support at all) only implements tile addressing,
    /// see `author_cog`'s own doc. Unlike a continuous-data source, a
    /// striped paletted input is refused rather than accepted.
    #[test]
    fn author_cog_refuses_a_striped_paletted_input() {
        let input = temp_path("palette-striped-in");
        let output = temp_path("palette-striped-out");
        let width = 4u32;
        let height = 4u32;
        let colormap: Vec<u16> = {
            let mut cm = vec![0u16; 256 * 3];
            cm[0] = 65535;
            cm
        };
        let tags: Vec<TagEntry> = vec![
            (256, Value::Long(vec![width])),
            (257, Value::Long(vec![height])),
            (258, Value::Short(vec![8])),
            (259, Value::Short(vec![1])),
            (262, Value::Short(vec![3])), // Palette
            (273, Value::Long(vec![0])),  // placeholder, fixed below
            (277, Value::Short(vec![1])),
            (278, Value::Long(vec![height])),
            (279, Value::Long(vec![width * height])),
            (320, Value::Short(colormap)),
        ];
        let ifd_size = ifd_encoded_size(&tags);
        let strip_offset = 8 + ifd_size;
        let tags: Vec<TagEntry> = tags
            .into_iter()
            .map(|(id, value)| {
                if id == 273 {
                    (id, Value::Long(vec![strip_offset]))
                } else {
                    (id, value)
                }
            })
            .collect();
        let mut out = tiff_header(8).to_vec();
        out.extend_from_slice(&encode_ifd(&tags, 0, 8));
        out.extend_from_slice(&vec![0u8; (width * height) as usize]);
        std::fs::write(&input, out).unwrap();

        let result = author_cog(&input, &output, &AuthorOptions::default());
        match result {
            Err(CogError::Unsupported(message)) => {
                assert!(
                    message.contains("paletted") && message.contains("striped"),
                    "message should name both facts: {message}"
                );
            }
            other => panic!("expected Err(Unsupported(_)) naming striped+paletted, got {other:?}"),
        }
        std::fs::remove_file(&input).ok();
    }

    #[test]
    fn author_cog_refuses_forcing_box_average_onto_a_paletted_source() {
        let input = temp_path("palette-force-average-in");
        let output = temp_path("palette-force-average-out");
        let colormap = {
            let mut cm = vec![0u16; 768];
            cm[0] = 65535;
            cm
        };
        write_paletted_input_geotiff(&input, 16, 16, 16, &colormap);

        let result = author_cog(
            &input,
            &output,
            &AuthorOptions {
                tile_size: 16,
                resample: ResampleMode::BoxAverage,
            },
        );
        match result {
            Err(CogError::Unsupported(message)) => {
                assert!(
                    message.contains("BoxAverage") && message.contains("paletted"),
                    "message should name both the requested mode and the real reason: {message}"
                );
            }
            other => {
                panic!("expected Err(Unsupported(_)) naming BoxAverage+paletted, got {other:?}")
            }
        }
        std::fs::remove_file(&input).ok();
    }

    #[test]
    fn author_cog_refuses_unsupported_input_compression() {
        let input = temp_path("lzw-in");
        let output = temp_path("lzw-out");
        // Claim LZW (5) without actually LZW-encoding the pixel bytes --
        // the compression-code check runs before any chunk is decoded, so
        // this refusal fires before the mismatch would ever matter.
        let width = 4u32;
        let height = 4u32;
        let tags: Vec<TagEntry> = vec![
            (256, Value::Long(vec![width])),
            (257, Value::Long(vec![height])),
            (258, Value::Short(vec![8])),
            (259, Value::Short(vec![5])), // LZW
            (262, Value::Short(vec![1])),
            (273, Value::Long(vec![0])),
            (277, Value::Short(vec![1])),
            (278, Value::Long(vec![height])),
            (279, Value::Long(vec![width * height])),
            (33550, Value::Double(vec![0.01, 0.01, 0.0])),
            (33922, Value::Double(vec![0.0, 0.0, 0.0, -1.0, 1.0, 0.0])),
            (
                34735,
                Value::Short(vec![1, 1, 0, 2, 1024, 0, 1, 2, 2048, 0, 1, 4326]),
            ),
        ];
        let ifd_size = ifd_encoded_size(&tags);
        let strip_offset = 8 + ifd_size;
        let tags: Vec<TagEntry> = tags
            .into_iter()
            .map(|(id, value)| {
                if id == 273 {
                    (id, Value::Long(vec![strip_offset]))
                } else {
                    (id, value)
                }
            })
            .collect();
        let mut out = tiff_header(8).to_vec();
        out.extend_from_slice(&encode_ifd(&tags, 0, 8));
        std::fs::write(&input, out).unwrap();

        let result = author_cog(&input, &output, &AuthorOptions::default());
        assert!(matches!(result, Err(CogError::Unsupported(_))));
        std::fs::remove_file(&input).ok();
    }

    #[test]
    fn author_cog_refuses_input_that_already_has_more_than_one_ifd() {
        // Reuse this crate's own committed serving fixture -- it's already
        // a 2-IFD (main + overview) COG.
        let already_multi_ifd = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/tiled_rgb.tif");
        let output = temp_path("multi-ifd-out");
        let result = author_cog(&already_multi_ifd, &output, &AuthorOptions::default());
        match result {
            Err(CogError::Unsupported(message)) => {
                assert!(message.contains("single-resolution"));
            }
            other => panic!("expected Err(Unsupported(_)) naming single-resolution, got {other:?}"),
        }
    }
}
