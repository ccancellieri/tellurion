//! Pure-Rust tiled GeoTIFF reading, on top of the `tiff` crate (MIT OR
//! Apache-2.0) — no GDAL, no C dependency. First-slice support only
//! (`#37`): tiled layout (no striped images), 8-bit grayscale/RGB/RGBA/
//! paletted (categorical) samples, and uncompressed/LZW/Deflate compression
//! (paletted narrower still — see this file's own "Paletted (categorical)
//! support" doc below) — anything else is
//! refused by [`CogError::Unsupported`] naming the exact unsupported fact,
//! never a panic or a silently wrong decode. Every overview beyond the main
//! image (IFD 0) is a later IFD in the same file, read via the `tiff`
//! crate's own `next_image`/`more_images` walk — the standard COG
//! convention every generator (`gdaladdo`, `rio cogeo`, ...) already
//! produces, so no separate "is this a COG" sniffing is needed beyond the
//! per-IFD support checks below.
//!
//! CRS scope (`#37`, narrow first slice): only a *Geographic* model whose
//! `GeographicTypeGeoKey` is exactly EPSG:4326 (WGS84 lon/lat degrees) can
//! be related to CRS84 without implementing a real reprojection — a
//! GeoTIFF's raster convention already stores X=longitude, Y=latitude in
//! that pixel order, so EPSG:4326 needs only an identity transform to
//! become a CRS84 extent. A projected CRS, or any other geographic datum,
//! is refused at open time rather than silently served with a wrong extent
//! (see `geokeys.rs`). Reprojection is out of this lane's scope entirely.
//!
//! Every read here re-opens the source and re-walks its IFD chain from the
//! start (`open` for metadata, `read_window` for pixels) rather than
//! keeping one `tiff::decoder::Decoder` alive across requests — correctness
//! and a bounded, obvious lifetime over the added complexity of sharing a
//! stateful, non-`Send`-across-await decoder behind a lock; a handful of IFD
//! headers is cheap to re-walk against the same per-request pixel budget
//! that already bounds tile data itself. For a [`CogSource::Remote`]
//! source this re-walk costs a small, bounded number of ranged HTTP
//! requests per call (coalesced by `remote.rs::HttpRangeReader`'s own
//! window buffering) rather than a filesystem re-open's near-zero cost —
//! accepted rather than adding a byte-range cache across calls, since that
//! would duplicate this workspace's existing byte-budgeted tile cache
//! shape for a cache that would only ever hold header/IFD bytes, not the
//! served tile itself (see `driver.rs`'s module doc for the full decision).
//!
//! ## Paletted (categorical) support
//!
//! `PhotometricInterpretation` = RGBPalette (`Bands::Palette`) needs its own
//! read path, not just its own [`Bands`] arm: this crate's `tiff` dependency
//! (0.11.3) has no RGBPalette case in its `colortype()` accessor at all — it
//! unconditionally returns `UnsupportedError` for that photometric — and
//! *every* one of that crate's own high-level chunk-read APIs
//! (`read_chunk`, `read_chunk_bytes`, `image_chunk_buffer_layout`) calls
//! `colortype()` internally before doing anything else, so none of them can
//! ever be used against a paletted IFD (verified empirically against this
//! exact dependency version, not assumed from its source alone). The tags
//! that DON'T depend on `colortype()` — `PhotometricInterpretation` itself,
//! `TileOffsets`/`TileByteCounts`, `chunk_dimensions()` — work fine
//! regardless, so [`read_raw_chunk`] reads a tile's own file range directly
//! from those tags and inflates it itself (this crate already depends on
//! `flate2` for `author.rs`'s own Deflate output), rather than going
//! through the crate's own chunk-read API at all. `author.rs`'s own source
//! read (`read_source_rows_paletted`) uses the exact same function for the
//! identical reason on the authoring side.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::PathBuf;

use flate2::read::ZlibDecoder;
use tiff::decoder::{ChunkType, Decoder, DecodingResult};
use tiff::tags::Tag;
use tiff::ColorType;

use crate::colormap::ResolvedColormap;
use crate::error::{CogError, Result};
use crate::geokeys::{self, CrsInfo, GeoTransform};
use crate::remote::{HttpRangeReader, OperationContext, RemoteCogSource};

/// TIFF `PhotometricInterpretation` tag value for a paletted (indexed)
/// image — the "this source's samples are class indices, not intensities"
/// signal both this module (serving) and `author::author_cog` (authoring)
/// key off. Shared here rather than duplicated because both sides must
/// agree on the exact same raw tag value.
pub(crate) const PHOTOMETRIC_PALETTE: u16 = 3;

/// TIFF `ColorMap` tag (320) length for an 8-bit palette: 3 planes (all 256
/// Red values, then all 256 Green, then all 256 Blue — never interleaved
/// per index) — TIFF6 Section 8's own layout, and the only depth this
/// crate's authoring path ever writes or this crate's serving path ever
/// accepts (see [`validate_8bit_palette`]).
const COLORMAP_LEN_8BIT: usize = 3 * 256;

/// Compression codes a manually-decoded paletted chunk ([`read_raw_chunk`])
/// can actually handle: uncompressed (1) or Deflate (8) — LZW (5) is
/// serveable for every OTHER band layout via the `tiff` crate's own LZW
/// decoder, but that decoder is private to that crate's `read_chunk`, which
/// paletted IFDs can't use at all (see this module's own doc); a paletted
/// LZW source is refused by name at open time rather than attempted.
const PALETTE_COMPRESSION: [u16; 2] = [1, 8];

/// Reads and validates IFD `ifd_index`'s `ColorMap` tag (320): exactly
/// [`COLORMAP_LEN_8BIT`] raw 16-bit entries. Shared by [`open`] (resolves
/// the values into a lookup table to serve) and `author::author_cog`
/// (carries the same raw values through byte-for-byte to every output
/// level's own `ColorMap` tag).
pub(crate) fn read_and_validate_colormap<R: Read + Seek>(
    decoder: &mut Decoder<R>,
    ifd_index: usize,
) -> Result<Vec<u16>> {
    let raw = decoder
        .get_tag_u16_vec(Tag::ColorMap)
        .map_err(|e| CogError::Decode(e.to_string()))?;
    if raw.len() != COLORMAP_LEN_8BIT {
        return Err(CogError::Unsupported(format!(
            "IFD {ifd_index} is paletted (PhotometricInterpretation = RGBPalette) but its \
             ColorMap tag has {} entries, expected {COLORMAP_LEN_8BIT} (3 planes of 256 for an \
             8-bit palette)",
            raw.len()
        )));
    }
    Ok(raw)
}

/// Validates IFD `ifd_index`'s `BitsPerSample` is exactly 8 — the only
/// palette index depth this crate ever authors or serves, matching this
/// crate's existing 8-bit-only support for every other band layout
/// ([`Bands::from_color_type`]'s own refusal for anything else).
pub(crate) fn validate_8bit_palette<R: Read + Seek>(
    decoder: &mut Decoder<R>,
    ifd_index: usize,
) -> Result<()> {
    let bits: u16 = decoder
        .get_tag_unsigned(Tag::BitsPerSample)
        .map_err(|e| CogError::Decode(e.to_string()))?;
    if bits != 8 {
        return Err(CogError::Unsupported(format!(
            "IFD {ifd_index} is a paletted image with {bits}-bit samples; only 8-bit \
             palette indices are supported"
        )));
    }
    Ok(())
}

/// The largest `byte_count` [`read_raw_chunk`] will ever allocate a buffer
/// for, given a chunk whose real (decompressed) size is `expected_len`.
/// Legitimate Deflate output is never meaningfully larger than the data it
/// encodes — its own worst-case expansion is a handful of bytes of block
/// overhead, nowhere near 2x — and legitimate uncompressed (compression
/// code 1) data is exactly `expected_len` bytes. A `TileByteCounts`/
/// `StripByteCounts` entry claiming more than this is corrupt or hostile,
/// not a real tile, and is refused before a single byte is allocated for
/// it (see [`read_raw_chunk`]'s own doc for the attack this closes).
fn max_plausible_compressed_len(expected_len: usize) -> u64 {
    expected_len.saturating_mul(2).saturating_add(4096) as u64
}

/// Reads `byte_count` raw bytes at `offset` from `reader` and decompresses
/// them per `compression` — the manual counterpart of
/// `tiff::decoder::Decoder::read_chunk` for exactly the one case that
/// crate has no support for at all (see this module's own doc). Returns
/// exactly `expected_len` bytes (`tile_width * tile_height` for an 8-bit,
/// single-band paletted tile) or a named [`CogError`] — never a short or
/// over-long buffer silently accepted.
///
/// Bounded against a hostile or corrupt `TileByteCounts`/`StripByteCounts`
/// entry two ways, both checked BEFORE the work they would otherwise pay
/// for: `byte_count` itself is capped at [`max_plausible_compressed_len`]
/// before the read buffer for it is ever allocated (a claimed compressed
/// size comes straight off the file's own tag, up to ~4.29 GiB, with
/// nothing else validating it before this), and the inflate is capped at
/// `expected_len + 1` bytes via `Read::take` so a small, honest-looking
/// compressed payload that would otherwise decompress into gigabytes (a
/// classic decompression bomb) can never cost more than one byte past the
/// real tile size to detect and refuse.
pub(crate) fn read_raw_chunk<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    byte_count: u64,
    compression: u16,
    expected_len: usize,
) -> Result<Vec<u8>> {
    let max_compressed = max_plausible_compressed_len(expected_len);
    if byte_count > max_compressed {
        return Err(CogError::Unsupported(format!(
            "chunk claims {byte_count} compressed bytes, but its own decompressed size is \
             only {expected_len}; {max_compressed} is a generous ceiling for legitimate \
             Deflate/uncompressed output, so a value beyond it can only be a corrupt or \
             hostile TileByteCounts/StripByteCounts entry — refused before allocating a \
             buffer for it"
        )));
    }

    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|e| CogError::Decode(e.to_string()))?;
    let mut compressed = vec![0u8; byte_count as usize];
    reader
        .read_exact(&mut compressed)
        .map_err(|e| CogError::Decode(e.to_string()))?;
    let out = match compression {
        1 => compressed,
        8 => {
            // One byte past `expected_len` is enough to prove an overshoot
            // (decompression bomb, or a corrupt stream that never reaches
            // its declared end) without ever materializing more than that
            // for it, regardless of how large the payload's real
            // decompressed size actually is.
            let bound = expected_len as u64 + 1;
            let mut inflated = Vec::with_capacity(expected_len);
            ZlibDecoder::new(&compressed[..])
                .take(bound)
                .read_to_end(&mut inflated)
                .map_err(|e| {
                    CogError::Decode(format!("failed to inflate a paletted chunk: {e}"))
                })?;
            if inflated.len() as u64 > expected_len as u64 {
                return Err(CogError::Unsupported(format!(
                    "paletted chunk inflated past its own expected size ({expected_len} \
                     bytes) without reaching end-of-stream — refusing rather than continuing \
                     to decompress an oversized (bomb-shaped) payload"
                )));
            }
            inflated
        }
        other => {
            return Err(CogError::Unsupported(format!(
                "paletted chunk uses compression code {other}; only uncompressed (1) or \
                 Deflate (8) can be manually decoded — this crate's TIFF decoder has no \
                 native RGBPalette support at all"
            )));
        }
    };
    if out.len() != expected_len {
        return Err(CogError::Decode(format!(
            "manually-decoded paletted chunk is {} bytes, expected {expected_len}",
            out.len()
        )));
    }
    Ok(out)
}

/// Where this driver reads GeoTIFF bytes from — a local filesystem path, or
/// a remote `http(s)` object read entirely through ranged GET requests (see
/// `remote.rs`). Built once by `CogDriverFactory::build` (`driver.rs`) from
/// the storage's configured locator string; carried from there into every
/// [`open`]/[`read_window`] call this collection's lifetime makes.
#[derive(Clone)]
pub enum CogSource {
    Local(PathBuf),
    Remote(RemoteCogSource),
}

/// Dispatches `Read`/`Seek` to whichever concrete source [`open_decoder`]
/// built — the one seam `tiff::decoder::Decoder`'s generic parameter needs
/// so the rest of this file never branches on `CogSource` itself.
enum SourceReader {
    Local(BufReader<File>),
    Remote(HttpRangeReader),
}

impl Read for SourceReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            SourceReader::Local(reader) => reader.read(buf),
            SourceReader::Remote(reader) => reader.read(buf),
        }
    }
}

impl Seek for SourceReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match self {
            SourceReader::Local(reader) => reader.seek(pos),
            SourceReader::Remote(reader) => reader.seek(pos),
        }
    }
}

/// Compression codes this driver accepts (TIFF `Compression` tag values):
/// uncompressed (1), LZW (5), Deflate/Adobe Deflate (8). Matches the
/// `lzw`/`deflate` features this crate enables on the `tiff` dependency —
/// see `Cargo.toml`'s own comment for why JPEG/fax are compiled out
/// entirely rather than merely unused.
const SUPPORTED_COMPRESSION: [u16; 3] = [1, 5, 8];

/// This driver's native band layout, always widened to RGBA8 for serving —
/// see `tellurion_core::storage::RasterWindow`'s own doc for why. `Gray`'s
/// own widening is a plain grayscale replicate (`widen_to_rgba` below)
/// UNLESS the collection configures a colormap (`#92`), in which case
/// [`pixel_rgba`] uses that colormap's own resolved lookup instead — see
/// its own doc. `Palette` is `Gray`'s categorical sibling: one 8-bit class
/// index per pixel too, but its RGBA meaning always comes from the file's
/// OWN embedded `ColorMap` tag ([`CogMeta::embedded_colormap`]), never from
/// operator config — see this crate's own doc for the full paletted-support
/// story.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bands {
    Gray,
    Rgb,
    Rgba,
    Palette,
}

impl Bands {
    /// `pub(crate)`: also used by `author.rs` to classify the *input*
    /// GeoTIFF's own band layout — the same 8-bit Gray/RGB/RGBA support set
    /// this crate serves, reused rather than re-implemented. Never called
    /// with a paletted `ColorType` — that photometric is detected from the
    /// raw tag value before `colortype()` is ever invoked, since that
    /// accessor has no RGBPalette case at all (see this crate's own doc).
    pub(crate) fn from_color_type(color: ColorType, ifd_index: usize) -> Result<Self> {
        match color {
            ColorType::Gray(8) => Ok(Bands::Gray),
            ColorType::RGB(8) => Ok(Bands::Rgb),
            ColorType::RGBA(8) => Ok(Bands::Rgba),
            other => Err(CogError::Unsupported(format!(
                "IFD {ifd_index} has color type {other:?}; only 8-bit grayscale/RGB/RGBA is supported"
            ))),
        }
    }

    pub fn channel_count(self) -> usize {
        match self {
            Bands::Gray | Bands::Palette => 1,
            Bands::Rgb => 3,
            Bands::Rgba => 4,
        }
    }

    /// Widens one native pixel (`channel_count()` bytes) to straight RGBA8.
    /// `Palette`'s own defensive fallback (no caller should ever reach this
    /// without a resolved colormap in hand — see [`pixel_rgba`]'s own doc)
    /// replicates the raw index byte exactly like `Gray`, rather than
    /// inventing a meaning for an index with no colormap.
    fn widen_to_rgba(self, sample: &[u8]) -> [u8; 4] {
        match self {
            Bands::Gray | Bands::Palette => [sample[0], sample[0], sample[0], 255],
            Bands::Rgb => [sample[0], sample[1], sample[2], 255],
            Bands::Rgba => [sample[0], sample[1], sample[2], sample[3]],
        }
    }
}

/// One IFD's physical tiling shape — either the main image (IFD 0) or one
/// overview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Level {
    pub ifd_index: usize,
    pub width: u32,
    pub height: u32,
    pub tile_width: u32,
    pub tile_height: u32,
}

/// Everything derived once from a GeoTIFF's own tags/GeoKeys: its band
/// layout, its overview pyramid (sorted finest-first, widest `width`
/// first), its pixel transform and CRS, and the CRS84 extent that transform
/// implies (always `Some`-shaped in practice — [`open`] refuses to return a
/// `CogMeta` at all for a CRS this slice cannot relate to CRS84).
#[derive(Debug, Clone, PartialEq)]
pub struct CogMeta {
    pub bands: Bands,
    pub levels: Vec<Level>,
    pub transform: GeoTransform,
    pub crs: CrsInfo,
    pub total_geo_width_deg: f64,
    pub total_geo_height_deg: f64,
    /// `[minx, miny, maxx, maxy]` in CRS84 (lon/lat, WGS84) order.
    pub extent_crs84: [f64; 4],
    /// The physical collection name this driver reports to `CatalogSource`
    /// — the file stem, matching `tellurion-pmtiles`' own fallback exactly
    /// (a GeoTIFF carries no embedded logical dataset name to prefer over
    /// it).
    pub logical_name: String,
    /// The color-index -> RGBA lookup table this GeoTIFF's own embedded
    /// `ColorMap` tag declares — `Some` exactly when `bands ==
    /// Bands::Palette`, `None` otherwise. A distinct, mutually exclusive
    /// concept from an operator-configured colormap (`#92`) — see
    /// `driver.rs`'s own colormap-resolution doc for how the two never
    /// apply at once. Read once here, from IFD 0 only, the same
    /// "georeferencing tags read once" contract this struct's other fields
    /// already follow.
    pub embedded_colormap: Option<ResolvedColormap>,
}

/// The physical collection name this driver reports for `source` — a local
/// file's stem, or the remote object's opaque display name. Remote locators
/// never enter this driver contract.
fn logical_name_of(source: &CogSource) -> String {
    let stem = match source {
        CogSource::Local(path) => path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_string),
        CogSource::Remote(remote) => Some(remote.display_name().to_owned()),
    };
    stem.unwrap_or_else(|| "cog".to_string())
}

/// Opens a fresh, unwrapped `SourceReader` against `source` — the same
/// source-opening logic [`open_decoder`] wraps in a `tiff::decoder::Decoder`
/// for IFD/tag parsing, factored out so [`read_window`]'s manual paletted
/// chunk path ([`read_raw_chunk`]) can seek and read raw bytes directly,
/// independent of (and never sharing state with) the `Decoder` it also
/// opens against the same source for that call.
fn open_source_reader(source: &CogSource, operation: OperationContext) -> Result<SourceReader> {
    match source {
        CogSource::Local(path) => {
            let file = File::open(path).map_err(|source| CogError::Open {
                path: path.display().to_string(),
                source,
            })?;
            Ok(SourceReader::Local(BufReader::new(file)))
        }
        CogSource::Remote(remote) => Ok(SourceReader::Remote(
            HttpRangeReader::open_with_operation(remote.clone(), operation)?,
        )),
    }
}

fn open_decoder(source: &CogSource, operation: OperationContext) -> Result<Decoder<SourceReader>> {
    let reader = open_source_reader(source, operation)?;
    Decoder::new(reader).map_err(|source| CogError::Decode(source.to_string()))
}

/// Validates and describes the IFD `decoder` currently has open.
fn read_current_level(
    decoder: &mut Decoder<SourceReader>,
    ifd_index: usize,
) -> Result<(Level, Bands)> {
    if decoder.get_chunk_type() != ChunkType::Tile {
        return Err(CogError::Unsupported(format!(
            "IFD {ifd_index} uses a striped layout; only tiled GeoTIFFs are supported"
        )));
    }
    let (width, height) = decoder
        .dimensions()
        .map_err(|e| CogError::Decode(e.to_string()))?;

    let photometric = decoder
        .get_tag_unsigned::<u16>(Tag::PhotometricInterpretation)
        .map_err(|e| CogError::Decode(e.to_string()))?;
    let bands = if photometric == PHOTOMETRIC_PALETTE {
        validate_8bit_palette(decoder, ifd_index)?;
        Bands::Palette
    } else {
        let color = decoder
            .colortype()
            .map_err(|e| CogError::Decode(e.to_string()))?;
        Bands::from_color_type(color, ifd_index)?
    };

    let compression = decoder
        .get_tag_unsigned::<u16>(Tag::Compression)
        .map_err(|e| CogError::Decode(e.to_string()))?;
    if bands == Bands::Palette {
        if !PALETTE_COMPRESSION.contains(&compression) {
            return Err(CogError::Unsupported(format!(
                "IFD {ifd_index} is paletted with compression code {compression}; only \
                 uncompressed (1) or Deflate (8) palette chunks can be decoded (this crate's \
                 TIFF decoder has no native RGBPalette support at all — see this module's own \
                 doc)"
            )));
        }
    } else if !SUPPORTED_COMPRESSION.contains(&compression) {
        return Err(CogError::Unsupported(format!(
            "IFD {ifd_index} uses compression code {compression}; only uncompressed (1), LZW (5), and Deflate (8) are supported"
        )));
    }

    let (tile_width, tile_height) = decoder.chunk_dimensions();
    Ok((
        Level {
            ifd_index,
            width,
            height,
            tile_width,
            tile_height,
        },
        bands,
    ))
}

/// Opens `source`, validates every IFD (main image plus every overview) is
/// within this driver's first-slice support, and derives its georeferencing.
/// Real I/O plus GeoKey parsing — call this from a blocking context (see
/// `driver.rs`'s `spawn_blocking` usage), never directly on an async
/// executor thread. For [`CogSource::Remote`], this is also where a source
/// that doesn't honor ranged GET requests is refused (see
/// `remote.rs::HttpRangeReader::open`'s own doc) — reached from
/// `Router::validate_catalog`'s eager boot sweep or, under `registry.
/// validation: lazy`, this collection's first touch, the same "bad config
/// fails fast" contract a local file's own `CogError::Open`/`Unsupported`
/// already gives.
pub fn open(source: &CogSource) -> Result<CogMeta> {
    let mut decoder = open_decoder(source, OperationContext::new())?;

    let (level0, bands) = read_current_level(&mut decoder, 0)?;
    let embedded_colormap = if bands == Bands::Palette {
        let raw = read_and_validate_colormap(&mut decoder, 0)?;
        Some(ResolvedColormap::from_tiff_colormap(&raw))
    } else {
        None
    };

    let pixel_scale = decoder
        .get_tag_f64_vec(Tag::ModelPixelScaleTag)
        .unwrap_or_default();
    let tiepoint = decoder
        .get_tag_f64_vec(Tag::ModelTiepointTag)
        .unwrap_or_default();
    let transform = geokeys::parse_geo_transform(&pixel_scale, &tiepoint)?;

    let directory = decoder
        .get_tag_u32_vec(Tag::GeoKeyDirectoryTag)
        .unwrap_or_default();
    let crs = geokeys::parse_crs(&directory)?;
    if !crs.is_wgs84_geographic {
        return Err(CogError::Unsupported(format!(
            "CRS is EPSG:{}; this driver only serves EPSG:4326 (WGS84 geographic) GeoTIFFs, not a projected or non-WGS84 geographic CRS",
            crs.epsg.map(|e| e.to_string()).unwrap_or_else(|| "<unset>".to_string())
        )));
    }

    let total_geo_width_deg = f64::from(level0.width) * transform.pixel_scale_x;
    let total_geo_height_deg = f64::from(level0.height) * transform.pixel_scale_y;
    let extent_crs84 = [
        transform.origin_x,
        transform.origin_y - total_geo_height_deg,
        transform.origin_x + total_geo_width_deg,
        transform.origin_y,
    ];

    let mut levels = vec![level0];
    let mut ifd_index = 0usize;
    while decoder.more_images() {
        decoder
            .next_image()
            .map_err(|e| CogError::Decode(e.to_string()))?;
        ifd_index += 1;
        let (level, overview_bands) = read_current_level(&mut decoder, ifd_index)?;
        if overview_bands != bands {
            return Err(CogError::Unsupported(format!(
                "IFD {ifd_index} has a different band layout than the main image (IFD 0)"
            )));
        }
        levels.push(level);
    }
    // Guarantee "finest first" regardless of file order, so
    // `tiling::select_overview`'s monotonic-scan assumption always holds.
    levels.sort_by_key(|level| std::cmp::Reverse(level.width));

    let logical_name = logical_name_of(source);

    Ok(CogMeta {
        bands,
        levels,
        transform,
        crs,
        total_geo_width_deg,
        total_geo_height_deg,
        extent_crs84,
        logical_name,
        embedded_colormap,
    })
}

/// One native `sample` (`bands.channel_count()` bytes) to straight RGBA
/// (`#92`) — pulled out of [`read_window`]'s own per-pixel loop as a pure
/// function so a test can exercise the colormap-vs-plain-widen decision
/// directly, without needing a real GeoTIFF fixture (the same reasoning
/// `driver.rs`'s own `check_pixel_budget` doc gives for pulling that check
/// out too). A colormap only ever applies over `Bands::Gray` (an
/// operator-configured one, `#92`) or `Bands::Palette` (the file's own
/// embedded one — always resolved by the time this is called, since
/// [`open`] builds it from IFD 0 whenever `bands == Palette`) —
/// `driver.rs` refuses a mismatched collection before this is ever reached
/// with a configured `colormap: Some(_)` and `bands` anything else, but
/// this still falls back to a plain widen in that case rather than panic,
/// since a pure function should never assume a caller's own invariant
/// holds.
fn pixel_rgba(bands: Bands, colormap: Option<&ResolvedColormap>, sample: &[u8]) -> [u8; 4] {
    match (bands, colormap) {
        (Bands::Gray, Some(cmap)) | (Bands::Palette, Some(cmap)) => cmap.apply(sample[0]),
        _ => bands.widen_to_rgba(sample),
    }
}

/// Half-open pixel rectangle `[x0, x1) x [y0, y1)` in one overview level's
/// own pixel coordinates, already world-bounds-clamped by the caller
/// (`tiling::plan_window`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelWindow {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

/// Reads exactly the tiles intersecting `window` at `level` and returns a
/// `(x1-x0) x (y1-y0)` straight RGBA8 buffer, row-major — never more source
/// pixels than `window` covers (the caller enforces the pixel budget against
/// `window`'s own area before calling this). `colormap` (`#92`), when
/// `Some`, replaces `Bands::Gray`'s ordinary grayscale-replicate widening
/// with that colormap's own resolved RGBA — see [`pixel_rgba`]. Real I/O;
/// call from a blocking context, same as [`open`].
pub fn read_window(
    source: &CogSource,
    level: &Level,
    bands: Bands,
    window: PixelWindow,
    colormap: Option<&ResolvedColormap>,
) -> Result<Vec<u8>> {
    let operation = OperationContext::new();
    let mut decoder = open_decoder(source, operation.clone())?;
    for _ in 0..level.ifd_index {
        decoder
            .next_image()
            .map_err(|e| CogError::Decode(e.to_string()))?;
    }
    let (actual_width, actual_height) = decoder
        .dimensions()
        .map_err(|e| CogError::Decode(e.to_string()))?;
    if actual_width != level.width || actual_height != level.height {
        return Err(CogError::Decode(format!(
            "IFD {} dimensions changed between metadata parse and read ({actual_width}x{actual_height} vs {}x{})",
            level.ifd_index, level.width, level.height
        )));
    }

    let out_w = window.x1 - window.x0;
    let out_h = window.y1 - window.y0;
    let mut out = vec![0u8; out_w as usize * out_h as usize * 4];

    let src_channels = bands.channel_count();
    let tiles_across = level.width.div_ceil(level.tile_width);
    let chunk_x0 = window.x0 / level.tile_width;
    let chunk_x1 = (window.x1 - 1) / level.tile_width;
    let chunk_y0 = window.y0 / level.tile_height;
    let chunk_y1 = (window.y1 - 1) / level.tile_height;

    // A paletted IFD can't go through `decoder.read_chunk()` at all — this
    // crate's TIFF decoder has no native RGBPalette support (see this
    // module's own doc); read straight from this IFD's own
    // `TileOffsets`/`TileByteCounts` tags and decompress manually instead,
    // via a second, independent raw reader opened just for that.
    let mut manual = if bands == Bands::Palette {
        let tile_offsets = decoder
            .get_tag_u32_vec(Tag::TileOffsets)
            .map_err(|e| CogError::Decode(e.to_string()))?;
        let tile_bytecounts = decoder
            .get_tag_u32_vec(Tag::TileByteCounts)
            .map_err(|e| CogError::Decode(e.to_string()))?;
        let compression = decoder
            .get_tag_unsigned::<u16>(Tag::Compression)
            .map_err(|e| CogError::Decode(e.to_string()))?;
        let raw_reader = open_source_reader(source, operation)?;
        Some((tile_offsets, tile_bytecounts, compression, raw_reader))
    } else {
        None
    };

    for chunk_y in chunk_y0..=chunk_y1 {
        for chunk_x in chunk_x0..=chunk_x1 {
            let chunk_index = chunk_y * tiles_across + chunk_x;
            let (data_w, data_h) = decoder.chunk_data_dimensions(chunk_index);
            let bytes: Vec<u8> =
                if let Some((tile_offsets, tile_bytecounts, compression, raw_reader)) =
                    manual.as_mut()
                {
                    let idx = chunk_index as usize;
                    let offset = *tile_offsets.get(idx).ok_or_else(|| {
                        CogError::Decode(format!(
                            "IFD {} tile {chunk_index} has no TileOffsets entry",
                            level.ifd_index
                        ))
                    })? as u64;
                    let byte_count = *tile_bytecounts.get(idx).ok_or_else(|| {
                        CogError::Decode(format!(
                            "IFD {} tile {chunk_index} has no TileByteCounts entry",
                            level.ifd_index
                        ))
                    })? as u64;
                    let expected_len =
                        level.tile_width as usize * level.tile_height as usize * src_channels;
                    read_raw_chunk(raw_reader, offset, byte_count, *compression, expected_len)?
                } else {
                    let decoded = decoder
                        .read_chunk(chunk_index)
                        .map_err(|e| CogError::Decode(e.to_string()))?;
                    let DecodingResult::U8(bytes) = decoded else {
                        return Err(CogError::Unsupported(format!(
                            "IFD {} chunk {chunk_index} decoded to a non-8-bit sample buffer",
                            level.ifd_index
                        )));
                    };
                    bytes
                };

            // Some `tiff`-crate versions return the full padded tile
            // (`chunk_dimensions`), others exactly the real data extent
            // (`chunk_data_dimensions`) — support both rather than assume.
            let padded_len = level.tile_width as usize * level.tile_height as usize * src_channels;
            let unpadded_len = data_w as usize * data_h as usize * src_channels;
            let (row_stride_px, buf_h) = if bytes.len() == padded_len {
                (level.tile_width, level.tile_height)
            } else if bytes.len() == unpadded_len {
                (data_w, data_h)
            } else {
                return Err(CogError::Decode(format!(
                    "IFD {} chunk {chunk_index} decoded to {} bytes, expected {padded_len} (padded) or {unpadded_len} (unpadded)",
                    level.ifd_index, bytes.len()
                )));
            };

            let chunk_origin_x = chunk_x * level.tile_width;
            let chunk_origin_y = chunk_y * level.tile_height;
            let valid_x1 = chunk_origin_x + data_w.min(row_stride_px);
            let valid_y1 = chunk_origin_y + data_h.min(buf_h);

            let src_x_lo = window.x0.max(chunk_origin_x);
            let src_x_hi = window.x1.min(valid_x1);
            let src_y_lo = window.y0.max(chunk_origin_y);
            let src_y_hi = window.y1.min(valid_y1);

            for src_y in src_y_lo..src_y_hi {
                let local_y = src_y - chunk_origin_y;
                for src_x in src_x_lo..src_x_hi {
                    let local_x = src_x - chunk_origin_x;
                    let src_off = (local_y as usize * row_stride_px as usize + local_x as usize)
                        * src_channels;
                    let Some(sample) = bytes.get(src_off..src_off + src_channels) else {
                        continue;
                    };
                    let rgba = pixel_rgba(bands, colormap, sample);
                    let dst_x = src_x - window.x0;
                    let dst_y = src_y - window.y0;
                    let dst_off = (dst_y as usize * out_w as usize + dst_x as usize) * 4;
                    out[dst_off..dst_off + 4].copy_from_slice(&rgba);
                }
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_a_missing_file_is_an_open_error_not_a_panic() {
        let source = CogSource::Local(PathBuf::from("/does/not/exist.tif"));
        let result = open(&source);
        assert!(matches!(result, Err(CogError::Open { .. })));
    }

    #[test]
    fn logical_name_of_a_remote_object_uses_its_display_name() {
        let variable = "TELLURION_COG_READER_NAME_TEST";
        std::env::set_var(variable, "http://example.invalid/rasters/tiled_rgb.tif");
        let source = CogSource::Remote(RemoteCogSource::administrative_from_env(variable).unwrap());
        std::env::remove_var(variable);
        assert_eq!(logical_name_of(&source), "tiled_rgb");
    }

    #[test]
    fn logical_name_of_a_remote_object_with_no_path_falls_back_to_cog() {
        let variable = "TELLURION_COG_READER_FALLBACK_TEST";
        std::env::set_var(variable, "http://example.invalid");
        let source = CogSource::Remote(RemoteCogSource::administrative_from_env(variable).unwrap());
        std::env::remove_var(variable);
        assert_eq!(logical_name_of(&source), "cog");
    }

    #[test]
    fn eight_bit_gray_maps_to_the_gray_band_layout() {
        assert_eq!(
            Bands::from_color_type(ColorType::Gray(8), 0).unwrap(),
            Bands::Gray
        );
    }

    #[test]
    fn sixteen_bit_gray_is_unsupported_not_silently_downcast() {
        assert!(matches!(
            Bands::from_color_type(ColorType::Gray(16), 0),
            Err(CogError::Unsupported(_))
        ));
    }

    #[test]
    fn widen_to_rgba_fills_full_opacity_for_bands_with_no_alpha() {
        assert_eq!(Bands::Gray.widen_to_rgba(&[100]), [100, 100, 100, 255]);
        assert_eq!(Bands::Rgb.widen_to_rgba(&[10, 20, 30]), [10, 20, 30, 255]);
        assert_eq!(
            Bands::Rgba.widen_to_rgba(&[10, 20, 30, 40]),
            [10, 20, 30, 40]
        );
    }

    /// `Palette`'s own defensive fallback (no caller should ever reach
    /// `widen_to_rgba` for a paletted sample without a resolved colormap in
    /// hand -- `open` always builds one whenever `bands == Palette` -- but a
    /// pure function should never assume that invariant holds, so this
    /// replicates the raw index byte exactly like `Gray` rather than
    /// inventing a meaning for an index with no colormap).
    #[test]
    fn widen_to_rgba_replicates_the_raw_index_for_palette_with_no_resolved_colormap() {
        assert_eq!(Bands::Palette.widen_to_rgba(&[7]), [7, 7, 7, 255]);
    }

    // -- `pixel_rgba` (`#92`) -------------------------------------------------

    fn colormap() -> ResolvedColormap {
        ResolvedColormap::build(&tellurion_core::config::ColormapConf::Stops {
            stops: vec![
                tellurion_core::config::ColormapStop {
                    value: 0.0,
                    rgba: [1, 2, 3, 4],
                },
                tellurion_core::config::ColormapStop {
                    value: 255.0,
                    rgba: [5, 6, 7, 8],
                },
            ],
        })
    }

    #[test]
    fn pixel_rgba_uses_the_colormap_for_gray_samples_when_one_is_configured() {
        let cmap = colormap();
        assert_eq!(pixel_rgba(Bands::Gray, Some(&cmap), &[0]), [1, 2, 3, 4]);
    }

    #[test]
    fn pixel_rgba_falls_back_to_a_plain_widen_when_no_colormap_is_configured() {
        assert_eq!(pixel_rgba(Bands::Gray, None, &[100]), [100, 100, 100, 255]);
    }

    #[test]
    fn pixel_rgba_ignores_a_colormap_for_non_gray_bands() {
        let cmap = colormap();
        assert_eq!(
            pixel_rgba(Bands::Rgb, Some(&cmap), &[10, 20, 30]),
            [10, 20, 30, 255]
        );
    }

    /// A paletted sample resolves through the SAME colormap slot a `#92`
    /// operator-config colormap uses for `Gray` -- by construction it is
    /// always the file's own embedded `ColorMap` here, never an operator
    /// one (`driver.rs` refuses a configured colormap over anything but
    /// `Bands::Gray` before this is ever reached), but `pixel_rgba` itself
    /// doesn't need to know that; it only needs `Some`.
    #[test]
    fn pixel_rgba_uses_the_colormap_for_palette_samples_when_one_is_configured() {
        let cmap = colormap();
        assert_eq!(pixel_rgba(Bands::Palette, Some(&cmap), &[0]), [1, 2, 3, 4]);
    }

    #[test]
    fn pixel_rgba_falls_back_to_a_plain_widen_for_palette_with_no_resolved_colormap() {
        assert_eq!(pixel_rgba(Bands::Palette, None, &[42]), [42, 42, 42, 255]);
    }

    // -- `read_raw_chunk` bounding (a hostile or corrupt TileByteCounts/
    // StripByteCounts entry, or a decompression bomb, must be refused by
    // name -- never attempted) ------------------------------------------

    /// A `TileByteCounts`/`StripByteCounts` entry claiming ~4 GB against a
    /// tile whose real (decompressed) size is 16 bytes must be refused
    /// before a buffer for it is ever allocated. The backing reader here
    /// only ever holds 16 real bytes -- if the bound check didn't fire
    /// first, this would either try to allocate ~4 GB or fail with a
    /// generic short-read `Decode` error from `read_exact` instead; the
    /// specific `Unsupported` variant is what proves the ceiling check
    /// itself is what stopped it.
    #[test]
    fn read_raw_chunk_refuses_an_absurd_byte_count_before_allocating_for_it() {
        let mut cursor = std::io::Cursor::new(vec![0u8; 16]);
        let result = read_raw_chunk(&mut cursor, 0, 4_000_000_000, 8, 16);
        assert!(
            matches!(result, Err(CogError::Unsupported(_))),
            "an absurd TileByteCounts claim must be refused by name, not attempted: {result:?}"
        );
    }

    /// A small, honest-looking Deflate payload that decompresses to far
    /// more than its tile's own declared (expected) size is a
    /// decompression bomb -- one million zero bytes compresses down to a
    /// tiny fraction of a kilobyte, but this call claims (falsely) that
    /// the real tile is only 16 bytes. The inflate cap must catch this
    /// without ever materializing the full million-byte payload.
    #[test]
    fn read_raw_chunk_refuses_a_zlib_payload_that_inflates_past_the_declared_size() {
        let payload = vec![0u8; 1_000_000];
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &payload).unwrap();
        let compressed = encoder.finish().unwrap();
        assert!(
            compressed.len() < 2048,
            "the fixture must actually compress well for this test to mean anything"
        );

        let byte_count = compressed.len() as u64;
        let mut cursor = std::io::Cursor::new(compressed);
        let result = read_raw_chunk(&mut cursor, 0, byte_count, 8, 16);
        assert!(
            matches!(result, Err(CogError::Unsupported(_))),
            "a payload that inflates past its declared size must be refused by name, not              fully materialized: {result:?}"
        );
    }
}
