//! Zarr v2 store reading: opens either one plain array directory or a
//! `multiscales` resolution pyramid (local or remote — see `store.rs`'s own
//! doc for the seam that abstracts which), combines `.zarray`/`.zattrs` into
//! a single [`ZarrMeta`], and reads a bounded pixel window from one chosen
//! level — never a whole chunk grid, never a whole array.
//!
//! ## Multiscale pyramids (`#37` overview/pyramid follow-up)
//!
//! A store is opened one of two ways, decided purely by which metadata
//! document sits at its own root:
//!
//! - A `.zarray` at the root is a plain single-resolution array — this
//!   driver's original (`#37` first slice) behavior, entirely unchanged: one
//!   [`ZarrLevel`], read at native resolution every time, world-bounds-
//!   clamped. A store that was never authored as a pyramid keeps working
//!   exactly as it always has; nothing about this file makes that case pay
//!   for the pyramid path it doesn't use.
//! - A `.zgroup` at the root, with a `.zattrs` declaring an OME-NGFF-shaped
//!   `multiscales` (`metadata::parse_multiscales`'s own doc explains why
//!   this driver consumes that convention rather than inventing one), opens
//!   every declared dataset's own `.zarray` (through [`crate::store::
//!   ScopedStore`], rooted at that dataset's own subdirectory) into its own
//!   [`ZarrLevel`], then sorts them finest-first by real pixel width —
//!   mirroring `tellurion-cog::reader::open`'s own "guarantee finest first
//!   regardless of file order" defense for a COG's overview IFD chain,
//!   applied here to `multiscales.datasets`' own declared order instead.
//!
//! Either way, `driver.rs`'s own `tiling::select_overview` picks which level
//! a given request reads — the exact "pick the coarsest level that's still
//! fine enough for the destination tile" policy `tellurion-cog::tiling::
//! select_overview` already established for COG overviews, duplicated here
//! rather than shared for the same "driver crates in this workspace never
//! depend on one another" reason `driver.rs`'s own module doc gives.
//!
//! A pyramid's every level shares one georeferencing declaration
//! (`tellurion:extent_crs84`/`tellurion:fixed_index`, read once from the
//! group's own root `.zattrs` — never per level) and one `fixed_index`, so
//! every level's own leading (non y/x) dimensions must agree in length; a
//! pyramid whose levels disagree there is refused by name at open time
//! rather than silently reading the wrong slice of whichever level gets
//! selected per request.
//!
//! `open`/`read_window` each re-open and re-read whatever documents/chunks
//! they need rather than keeping a decoder or handle alive across requests —
//! the same "re-open per call, correctness and a bounded obvious lifetime
//! over sharing stateful I/O across awaits" choice `tellurion-cog::reader`'s
//! own doc makes, and for the same reason: metadata is a handful of small
//! JSON documents, cheap to re-fetch, and every pixel read is already
//! bounded by this module's own budgets. For a remote store this re-fetch
//! costs a small, bounded number of whole-object `GET` requests per call —
//! accepted rather than adding a byte cache across calls, the same trade-off
//! `tellurion-cog::driver`'s own module doc makes for its remote source.

use std::io::Read;

use crate::error::{Result, ZarrError};
use crate::metadata::{self, Compressor, DType};
use crate::store::{ScopedStore, ZarrStore};
use crate::tiling::Transform;

/// Hard per-request cap on the total number of chunk elements this driver
/// will decompress, summed across every chunk a window touches — the
/// aggregate counterpart of [`metadata::MAX_CHUNK_ELEMENTS`]'s per-chunk cap.
/// A single chunk can be small on its own yet a request could still touch a
/// huge number of them (a pathologically small chunk shape under a large
/// window); this bounds that case too, refused before any chunk is read.
const MAX_REQUEST_DECODE_ELEMENTS: u64 = 4_000_000;

/// One resolution level's own `.zarray` shape/chunking/dtype/compressor — a
/// plain single-array store has exactly one ([`ZarrMeta::levels`] is a
/// single-element `Vec`, `path` empty); a `multiscales` pyramid has one per
/// declared dataset. Mirrors `tellurion-cog::reader::Level`'s own role
/// (physical tiling shape for one IFD/overview) for this crate's own chunk
/// grid instead of TIFF tiles.
#[derive(Debug, Clone, PartialEq)]
pub struct ZarrLevel {
    /// Full rank shape, e.g. `[time, y, x]`.
    pub shape: Vec<u64>,
    /// Full rank chunk shape, same length as `shape`.
    pub chunks: Vec<u64>,
    pub dtype: DType,
    pub compressor: Compressor,
    pub fill_value: f64,
    pub dimension_separator: String,
    /// This level's own subdirectory, relative to the store root — empty for
    /// a plain single-array store (whose `.zarray`/chunks live at the root
    /// itself), or a `multiscales.datasets[].path` (e.g. `"0"`, `"1"`) for
    /// one dataset inside a pyramid. [`crate::store::ScopedStore`] roots
    /// every metadata/chunk read onto this before it reaches the underlying
    /// store.
    pub path: String,
}

impl ZarrLevel {
    pub(crate) fn width(&self) -> u32 {
        self.shape[self.shape.len() - 1] as u32
    }

    pub(crate) fn height(&self) -> u32 {
        self.shape[self.shape.len() - 2] as u32
    }
}

/// Everything derived once from a Zarr store's own metadata — its resolution
/// level(s) (from one or more `.zarray` documents; see this module's own
/// "Multiscale pyramids" doc), sorted finest-first (widest `width` first,
/// the same convention `tellurion-cog::reader::CogMeta::levels` uses), plus
/// this driver's own georeferencing declaration (from the store's root
/// `.zattrs`, see `metadata`'s own doc). `Clone` so a request can move an
/// owned copy into `spawn_blocking` while [`crate::driver::ZarrBackend`]
/// keeps its own cached copy.
#[derive(Debug, Clone, PartialEq)]
pub struct ZarrMeta {
    pub levels: Vec<ZarrLevel>,
    /// Fixed index for every dimension before the trailing `(y, x)` pair —
    /// the SAME index into every level (this module's own doc explains why
    /// a pyramid's levels must agree on their leading-dimension lengths).
    pub fixed_index: Vec<u64>,
    /// The pixel -> CRS84 transform for the FINEST level (`levels[0]`) —
    /// `tiling::plan_window` derives every other level's own transform from
    /// this plus that level's own pixel width/height, the same "one shared
    /// extent, many pixel counts" shape `tellurion-cog::tiling::plan_window`
    /// already uses for a COG's own overview pyramid.
    pub transform: Transform,
    /// `[minx, miny, maxx, maxy]` in CRS84 — the raw `.zattrs`-declared
    /// extent, kept alongside `transform` (rather than re-derived from it)
    /// so `CatalogSource::extent` reports exactly what the store declared,
    /// with no floating-point round trip through the pixel transform.
    pub extent_crs84: [f64; 4],
    pub total_geo_width_deg: f64,
    pub total_geo_height_deg: f64,
    /// The physical collection name this driver reports to `CatalogSource`
    /// — the array directory's own final path component (no embedded
    /// logical dataset name to prefer over it, the same fallback
    /// `tellurion-cog`'s `logical_name_of` uses for a GeoTIFF).
    pub logical_name: String,
}

/// Opens `store` (an array directory or a `multiscales` pyramid group, local
/// or remote — see `store.rs`'s own doc), validates it against this driver's
/// supported shapes, and returns the combined metadata. Real I/O; call from
/// a blocking context, mirroring `tellurion-cog::reader::open`'s own
/// contract (and, for a remote store, also where a request that can't reach
/// the server at all first surfaces — see `store::ZarrStore`'s own doc).
pub fn open(store: &dyn ZarrStore) -> Result<ZarrMeta> {
    match store.read_metadata(".zarray")? {
        Some(zarray_bytes) => open_single_array(store, zarray_bytes),
        None => open_pyramid(store),
    }
}

/// The plain single-resolution case — this driver's original (`#37` first
/// slice) behavior, entirely unchanged: one level, read at native resolution
/// every time. A store that was never authored as a pyramid keeps serving
/// exactly as it always has.
fn open_single_array(store: &dyn ZarrStore, zarray_bytes: Vec<u8>) -> Result<ZarrMeta> {
    let zarray = metadata::parse_zarray(&zarray_bytes)?;
    let zattrs_bytes = read_required_zattrs(store)?;
    let georef = metadata::parse_zattrs_georef(&zattrs_bytes, zarray.shape.len())?;
    check_fixed_index_bounds(&georef.fixed_index, &zarray.shape)?;

    let level = ZarrLevel {
        shape: zarray.shape,
        chunks: zarray.chunks,
        dtype: zarray.dtype,
        compressor: zarray.compressor,
        fill_value: zarray.fill_value,
        dimension_separator: zarray.dimension_separator,
        path: String::new(),
    };
    build_meta(store, vec![level], georef.fixed_index, georef.extent_crs84)
}

/// The `multiscales` pyramid case (this module's own doc). Reached only when
/// no `.zarray` sits at the store's own root.
fn open_pyramid(store: &dyn ZarrStore) -> Result<ZarrMeta> {
    if store.read_metadata(".zgroup")?.is_none() {
        return Err(ZarrError::Unsupported(format!(
            "'{}' has no readable .zarray; this driver serves either a single array directory \
             (containing .zarray/.zattrs) or a multiscale pyramid group (.zgroup plus a \
             .zattrs declaring 'multiscales'), not a bare or missing store",
            store.describe()
        )));
    }
    let zattrs_bytes = read_required_zattrs(store)?;
    let Some(dataset_paths) = metadata::parse_multiscales(&zattrs_bytes)? else {
        return Err(ZarrError::Unsupported(format!(
            "'{}' is a Zarr group (.zgroup present) whose .zattrs declares no 'multiscales' \
             pyramid; this driver serves one array directory directly, or a multiscale pyramid \
             group, never a bare hierarchical group with neither",
            store.describe()
        )));
    };

    let mut levels = Vec::with_capacity(dataset_paths.len());
    for path in &dataset_paths {
        let scoped = ScopedStore::new(store, path);
        let zarray_bytes = scoped.read_metadata(".zarray")?.ok_or_else(|| {
            ZarrError::Unsupported(format!(
                "multiscale dataset '{path}' has no readable .zarray"
            ))
        })?;
        let zarray = metadata::parse_zarray(&zarray_bytes)?;
        levels.push(ZarrLevel {
            shape: zarray.shape,
            chunks: zarray.chunks,
            dtype: zarray.dtype,
            compressor: zarray.compressor,
            fill_value: zarray.fill_value,
            dimension_separator: zarray.dimension_separator,
            path: path.clone(),
        });
    }

    let rank = levels[0].shape.len();
    for level in &levels {
        if level.shape.len() != rank {
            return Err(ZarrError::Unsupported(format!(
                "multiscale dataset '{}' has rank {}, but dataset '{}' has rank {rank}; every \
                 level of a pyramid must share the same rank",
                level.path,
                level.shape.len(),
                levels[0].path
            )));
        }
    }
    // Guarantee "finest first" regardless of the document's own declared
    // order, so `tiling::select_overview`'s monotonic-scan assumption always
    // holds — the same defense `tellurion-cog::reader::open` applies to a
    // COG's own overview IFD chain.
    levels.sort_by_key(|level| std::cmp::Reverse(level.width()));

    // Every level shares one `fixed_index` into its own leading dimensions
    // (this module's own doc) -- refuse rather than silently read the wrong
    // slice if the levels disagree on those dimensions' own lengths.
    let leading_rank = rank.saturating_sub(2);
    for level in &levels[1..] {
        if level.shape[..leading_rank] != levels[0].shape[..leading_rank] {
            return Err(ZarrError::Unsupported(format!(
                "multiscale dataset '{}' has leading (non y/x) dimensions {:?}, but dataset \
                 '{}' has {:?}; every level of a pyramid must agree on every dimension besides \
                 its trailing (y, x) pair, since this driver reads the same 'tellurion:fixed_index' \
                 into every level",
                level.path,
                &level.shape[..leading_rank],
                levels[0].path,
                &levels[0].shape[..leading_rank],
            )));
        }
    }

    let georef = metadata::parse_zattrs_georef(&zattrs_bytes, rank)?;
    for level in &levels {
        check_fixed_index_bounds(&georef.fixed_index, &level.shape)?;
    }

    build_meta(store, levels, georef.fixed_index, georef.extent_crs84)
}

fn read_required_zattrs(store: &dyn ZarrStore) -> Result<Vec<u8>> {
    store.read_metadata(".zattrs")?.ok_or_else(|| {
        ZarrError::Unsupported(format!(
            "'{}' has no readable .zattrs declaring 'tellurion:extent_crs84'; this driver refuses to guess a Zarr array's georeferencing",
            store.describe()
        ))
    })
}

fn check_fixed_index_bounds(fixed_index: &[u64], shape: &[u64]) -> Result<()> {
    for (dim, (&index, &length)) in fixed_index.iter().zip(shape.iter()).enumerate() {
        if index >= length {
            return Err(ZarrError::Unsupported(format!(
                "'tellurion:fixed_index'[{dim}] = {index} is out of bounds for dimension {dim} of length {length}"
            )));
        }
    }
    Ok(())
}

fn build_meta(
    store: &dyn ZarrStore,
    levels: Vec<ZarrLevel>,
    fixed_index: Vec<u64>,
    extent_crs84: [f64; 4],
) -> Result<ZarrMeta> {
    let finest = &levels[0];
    let width = finest.width() as f64;
    let height = finest.height() as f64;
    let transform = Transform {
        origin_x: extent_crs84[0],
        origin_y: extent_crs84[3],
        pixel_scale_x: (extent_crs84[2] - extent_crs84[0]) / width,
        pixel_scale_y: (extent_crs84[3] - extent_crs84[1]) / height,
    };
    let total_geo_width_deg = extent_crs84[2] - extent_crs84[0];
    let total_geo_height_deg = extent_crs84[3] - extent_crs84[1];

    Ok(ZarrMeta {
        levels,
        fixed_index,
        transform,
        extent_crs84,
        total_geo_width_deg,
        total_geo_height_deg,
        logical_name: store.logical_name(),
    })
}

/// Half-open pixel rectangle `[x0, x1) x [y0, y1)` in the array's own
/// trailing `(y, x)` pixel coordinates, already world-bounds-clamped by the
/// caller (`tiling::plan_window`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelWindow {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

/// Builds a chunk's key (its file/path name relative to the array root) from
/// its per-dimension chunk indices and `.zarray`'s own `dimension_separator`
/// — `"."` joins them into one filename component (`"0.1.2"`); `"/"` joins
/// them into nested path segments (`"0/1/2"`), both valid Zarr v2 layouts.
fn chunk_key(indices: &[u64], separator: &str) -> String {
    indices
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(separator)
}

/// Bytes of slack allowed above a chunk's own declared decompressed size
/// (`expected_len`) when fetching its RAW (possibly still-compressed) bytes
/// — covers a compressor's own container overhead (a gzip/zlib header and
/// trailer, and DEFLATE's own worst-case per-block store-mode expansion for
/// near-incompressible data) without ever detaching the cap from the
/// chunk's own already-budgeted size ([`metadata::MAX_CHUNK_ELEMENTS`]): the
/// slack scales with `expected_len` itself (one byte per 4096, the same
/// order of magnitude as DEFLATE's documented per-32KiB-block overhead, with
/// a wide safety margin) plus a flat floor generous enough for a tiny
/// chunk's own gzip/zlib header/trailer bytes. [`store::ZarrStore::
/// read_chunk`]'s HTTP implementation refuses (never silently truncates)
/// once a fetch's raw bytes exceed this — the same "cap first, refuse
/// rather than balloon" idiom this function's own bomb guard applies to a
/// chunk's DECOMPRESSED size, applied here to the bytes fetched over the
/// wire before decompression even starts. A local [`store::FsStore`] read
/// ignores this cap entirely (see that type's own doc for why).
pub(crate) fn chunk_raw_byte_cap(expected_len: usize) -> u64 {
    let len = expected_len as u64;
    len + len / 4096 + 1024
}

/// Decompresses one chunk's raw bytes. Bounded regardless of what the
/// compressed stream itself claims: `expected_len` (known in advance from
/// `.zarray`'s own `chunks`/`dtype`, already capped by
/// [`metadata::MAX_CHUNK_ELEMENTS`] at open time) caps the read so a
/// corrupted or adversarial chunk — a decompression bomb — can never balloon
/// past the size this driver already committed to, even transiently.
fn decompress(compressor: Compressor, raw: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    let bytes = match compressor {
        Compressor::Raw => raw.to_vec(),
        Compressor::Gzip => {
            let mut out = Vec::with_capacity(expected_len);
            flate2::read::GzDecoder::new(raw)
                .take(expected_len as u64 + 1)
                .read_to_end(&mut out)
                .map_err(|error| {
                    ZarrError::Decode(format!("gzip decompression failed: {error}"))
                })?;
            out
        }
        Compressor::Zlib => {
            let mut out = Vec::with_capacity(expected_len);
            flate2::read::ZlibDecoder::new(raw)
                .take(expected_len as u64 + 1)
                .read_to_end(&mut out)
                .map_err(|error| {
                    ZarrError::Decode(format!("zlib decompression failed: {error}"))
                })?;
            out
        }
    };
    if bytes.len() != expected_len {
        return Err(ZarrError::Decode(format!(
            "chunk decoded to {} bytes, expected exactly {expected_len}",
            bytes.len()
        )));
    }
    Ok(bytes)
}

/// Reads exactly the chunks intersecting `window` and returns a
/// `(x1-x0) x (y1-y0)` `f64` sample buffer, row-major, at `level` (one entry
/// of `ZarrMeta::levels` — the caller, `driver.rs`, already picked which via
/// `tiling::select_overview`) and `fixed_index`'s own fixed leading-dimension
/// index — never more chunks, and never more decompressed elements in
/// aggregate, than this module's own budgets allow (checked before any chunk
/// is fetched). Every metadata/chunk name is read through [`ScopedStore`],
/// rooted at `level.path` — empty for a plain single-array store (every name
/// passes through unchanged) or a `multiscales` dataset's own subdirectory,
/// so this function never itself branches on which. A chunk that doesn't
/// exist — locally, a missing file; remotely, a `404` (`store::ZarrStore::
/// read_chunk`'s own contract) — is legitimate under the Zarr v2 spec (an
/// unwritten chunk stands for `fill_value` everywhere) and is treated as
/// such here, not as an error. Real I/O; call from a blocking context, same
/// as [`open`].
pub fn read_window(
    store: &dyn ZarrStore,
    level: &ZarrLevel,
    fixed_index: &[u64],
    window: PixelWindow,
) -> Result<Vec<f64>> {
    let scoped = ScopedStore::new(store, &level.path);
    let rank = level.shape.len();
    let leading_rank = rank - 2;
    let chunk_h = level.chunks[rank - 2];
    let chunk_w = level.chunks[rank - 1];
    // `PixelWindow`/`TileCoord`-derived pixel coordinates are `u32`
    // throughout this crate (matching `tellurion-core::TileCoord`'s own
    // width); the chunk shape itself is `u64` (matching `.zarray`'s own
    // JSON number width) since it also feeds the flat in-chunk offset math
    // below, which can exceed `u32` for a chunk near `MAX_CHUNK_ELEMENTS`.
    let chunk_h32 = chunk_h as u32;
    let chunk_w32 = chunk_w as u32;
    let chunk_elements: u64 = level.chunks.iter().product();
    let dtype_size = level.dtype.size_bytes();

    let out_w = (window.x1 - window.x0) as usize;
    let out_h = (window.y1 - window.y0) as usize;
    let mut out = vec![level.fill_value; out_w * out_h];

    // C-order stride for every dimension, computed across the FULL chunk
    // shape (not the array shape) -- one chunk buffer is always exactly
    // `chunks`-shaped once decompressed, per the Zarr v2 boundary-padding
    // rule this module's own doc cites, so this is the same stride math
    // regardless of which chunk (interior or edge) is being read.
    let mut strides = vec![1u64; rank];
    for dim in (0..rank - 1).rev() {
        strides[dim] = strides[dim + 1] * level.chunks[dim + 1];
    }

    // The leading dimensions' own chunk coordinate and in-chunk flat offset
    // never change across the chunks this window touches -- `fixed_index`
    // is the same for every one of them.
    let mut leading_chunk_index = Vec::with_capacity(leading_rank);
    let mut leading_base_offset = 0u64;
    for (dim, &stride) in strides.iter().enumerate().take(leading_rank) {
        let global_index = fixed_index[dim];
        let chunk_len = level.chunks[dim];
        leading_chunk_index.push(global_index / chunk_len);
        leading_base_offset += (global_index % chunk_len) * stride;
    }

    let chunk_x0 = window.x0 / chunk_w32;
    let chunk_x1 = (window.x1 - 1) / chunk_w32;
    let chunk_y0 = window.y0 / chunk_h32;
    let chunk_y1 = (window.y1 - 1) / chunk_h32;

    let touched_chunks = u64::from(chunk_x1 - chunk_x0 + 1) * u64::from(chunk_y1 - chunk_y0 + 1);
    let decode_elements = touched_chunks.saturating_mul(chunk_elements);
    if decode_elements > MAX_REQUEST_DECODE_ELEMENTS {
        return Err(ZarrError::DecodeBudgetExceeded {
            elements: decode_elements,
            budget: MAX_REQUEST_DECODE_ELEMENTS,
        });
    }

    for chunk_y in chunk_y0..=chunk_y1 {
        for chunk_x in chunk_x0..=chunk_x1 {
            let mut indices = leading_chunk_index.clone();
            indices.push(u64::from(chunk_y));
            indices.push(u64::from(chunk_x));
            let key = chunk_key(&indices, &level.dimension_separator);

            let chunk_origin_x = chunk_x * chunk_w32;
            let chunk_origin_y = chunk_y * chunk_h32;
            let src_x_lo = window.x0.max(chunk_origin_x);
            let src_x_hi = window.x1.min(chunk_origin_x + chunk_w32);
            let src_y_lo = window.y0.max(chunk_origin_y);
            let src_y_hi = window.y1.min(chunk_origin_y + chunk_h32);

            let expected_len = chunk_elements as usize * dtype_size;
            let raw = match scoped.read_chunk(&key, chunk_raw_byte_cap(expected_len))? {
                Some(bytes) => bytes,
                None => continue,
            };
            let decoded = decompress(level.compressor, &raw, expected_len)?;

            for src_y in src_y_lo..src_y_hi {
                let local_y = u64::from(src_y - chunk_origin_y);
                for src_x in src_x_lo..src_x_hi {
                    let local_x = u64::from(src_x - chunk_origin_x);
                    let flat = leading_base_offset + local_y * chunk_w + local_x;
                    let byte_off = flat as usize * dtype_size;
                    let sample = level
                        .dtype
                        .decode(&decoded[byte_off..byte_off + dtype_size]);
                    let dst_x = (src_x - window.x0) as usize;
                    let dst_y = (src_y - window.y0) as usize;
                    out[dst_y * out_w + dst_x] = sample;
                }
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::path::{Path, PathBuf};

    use crate::store::FsStore;

    /// A local store rooted at `dir` — every existing local-store test below
    /// exercises [`read_window`]/[`open`] through this rather than a bare
    /// `&Path`, now that both take `&dyn ZarrStore` (see `store.rs`'s own
    /// doc for why).
    fn fs(dir: &TempDir) -> FsStore {
        FsStore::new(dir.path().to_path_buf())
    }

    /// A bare [`ZarrLevel`] for [`read_window`]'s own tests, which exercise
    /// chunk/window math directly and never go through [`open`] — `path`
    /// empty, the same "root of the store" shape a plain single-array store
    /// uses (see `ZarrLevel::path`'s own doc). `fixed_index` is a separate
    /// argument now that [`read_window`] takes it apart from a level, rather
    /// than bundled into a full `ZarrMeta`.
    fn base_level(shape: Vec<u64>, chunks: Vec<u64>) -> ZarrLevel {
        ZarrLevel {
            shape,
            chunks,
            dtype: DType::U8,
            compressor: Compressor::Raw,
            fill_value: 0.0,
            dimension_separator: ".".to_string(),
            path: String::new(),
        }
    }

    /// Writes one raw (uncompressed) chunk file under `root`, named per
    /// `indices`/`separator` -- `bytes.len()` must already equal the full
    /// chunk-shape element count times the dtype size (this helper does no
    /// padding of its own, matching how a real Zarr v2 writer must already
    /// have produced full-shaped chunk files).
    fn write_chunk(root: &Path, indices: &[u64], separator: &str, bytes: &[u8]) {
        let key = chunk_key(indices, separator);
        let path = root.join(&key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(bytes).unwrap();
    }

    #[test]
    fn chunk_key_joins_with_the_configured_separator() {
        assert_eq!(chunk_key(&[0, 1, 2], "."), "0.1.2");
        assert_eq!(chunk_key(&[0, 1, 2], "/"), "0/1/2");
    }

    /// A 4x4 u8 array, chunked 2x2 (four chunks): each chunk is filled with
    /// its own chunk-index-derived byte value so a window spanning all four
    /// proves the chunk grid + per-pixel placement, not just a single-chunk
    /// read.
    #[test]
    fn read_window_assembles_across_multiple_chunks() {
        let dir = tempfile_dir();
        let level = base_level(vec![4, 4], vec![2, 2]);
        write_chunk(dir.path(), &[0, 0], ".", &[1, 1, 1, 1]);
        write_chunk(dir.path(), &[0, 1], ".", &[2, 2, 2, 2]);
        write_chunk(dir.path(), &[1, 0], ".", &[3, 3, 3, 3]);
        write_chunk(dir.path(), &[1, 1], ".", &[4, 4, 4, 4]);

        let window = PixelWindow {
            x0: 0,
            y0: 0,
            x1: 4,
            y1: 4,
        };
        let samples = read_window(&fs(&dir), &level, &[], window).unwrap();
        // Row-major: row 0 = [1,1,2,2], row 1 = [1,1,2,2], row 2 = [3,3,4,4], row 3 = [3,3,4,4]
        assert_eq!(
            samples,
            vec![1.0, 1.0, 2.0, 2.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 4.0, 4.0, 3.0, 3.0, 4.0, 4.0]
        );
    }

    #[test]
    fn read_window_treats_a_missing_chunk_file_as_fill_value() {
        let dir = tempfile_dir();
        let mut level = base_level(vec![4, 4], vec![2, 2]);
        level.fill_value = 9.0;
        write_chunk(dir.path(), &[0, 0], ".", &[1, 1, 1, 1]);
        // chunk (0,1) is never written -- an unwritten chunk under the Zarr
        // v2 spec means "every element is fill_value", not an error.

        let window = PixelWindow {
            x0: 0,
            y0: 0,
            x1: 4,
            y1: 2,
        };
        let samples = read_window(&fs(&dir), &level, &[], window).unwrap();
        assert_eq!(samples, vec![1.0, 1.0, 9.0, 9.0, 1.0, 1.0, 9.0, 9.0]);
    }

    #[test]
    fn read_window_reads_only_the_requested_sub_rectangle_of_a_larger_chunk() {
        let dir = tempfile_dir();
        let level = base_level(vec![4, 4], vec![4, 4]);
        #[rustfmt::skip]
        let chunk: [u8; 16] = [
            0, 1, 2, 3,
            4, 5, 6, 7,
            8, 9, 10, 11,
            12, 13, 14, 15,
        ];
        write_chunk(dir.path(), &[0, 0], ".", &chunk);

        let window = PixelWindow {
            x0: 1,
            y0: 1,
            x1: 3,
            y1: 3,
        };
        let samples = read_window(&fs(&dir), &level, &[], window).unwrap();
        assert_eq!(samples, vec![5.0, 6.0, 9.0, 10.0]);
    }

    /// A 3D array (leading `level` dim of length 2, chunked 1) with
    /// `fixed_index = [1]` must read only level 1's own chunk, never level
    /// 0's -- proves the leading-dimension chunk-coordinate + in-chunk
    /// offset math, not just the 2D case.
    #[test]
    fn read_window_selects_the_fixed_leading_dimension_index() {
        let dir = tempfile_dir();
        let level = base_level(vec![2, 2, 2], vec![1, 2, 2]);
        write_chunk(dir.path(), &[0, 0, 0], ".", &[1, 1, 1, 1]);
        write_chunk(dir.path(), &[1, 0, 0], ".", &[7, 7, 7, 7]);

        let window = PixelWindow {
            x0: 0,
            y0: 0,
            x1: 2,
            y1: 2,
        };
        let samples = read_window(&fs(&dir), &level, &[1], window).unwrap();
        assert_eq!(samples, vec![7.0, 7.0, 7.0, 7.0]);
    }

    /// A leading dimension chunked coarser than 1 (chunk length 2, so index
    /// 1 sits at in-chunk local offset 1 rather than selecting a distinct
    /// chunk file) proves the in-chunk offset half of the leading-dimension
    /// math, complementing the chunk-selection half the test above proves.
    #[test]
    fn read_window_selects_the_right_in_chunk_offset_for_a_coarsely_chunked_leading_dim() {
        let dir = tempfile_dir();
        let level = base_level(vec![2, 2, 2], vec![2, 2, 2]);
        // One chunk covering both leading-dim indices: level 0 is all 1s,
        // level 1 (in-chunk offset 1 along the leading dim) is all 9s.
        #[rustfmt::skip]
        let chunk: [u8; 8] = [
            1, 1, 1, 1,
            9, 9, 9, 9,
        ];
        write_chunk(dir.path(), &[0, 0, 0], ".", &chunk);

        let window = PixelWindow {
            x0: 0,
            y0: 0,
            x1: 2,
            y1: 2,
        };
        let samples = read_window(&fs(&dir), &level, &[1], window).unwrap();
        assert_eq!(samples, vec![9.0, 9.0, 9.0, 9.0]);
    }

    #[test]
    fn read_window_decompresses_a_gzip_chunk() {
        use flate2::write::GzEncoder;
        use flate2::Compression;

        let dir = tempfile_dir();
        let mut level = base_level(vec![2, 2], vec![2, 2]);
        level.compressor = Compressor::Gzip;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&[5, 6, 7, 8]).unwrap();
        let compressed = encoder.finish().unwrap();
        write_chunk(dir.path(), &[0, 0], ".", &compressed);

        let window = PixelWindow {
            x0: 0,
            y0: 0,
            x1: 2,
            y1: 2,
        };
        let samples = read_window(&fs(&dir), &level, &[], window).unwrap();
        assert_eq!(samples, vec![5.0, 6.0, 7.0, 8.0]);
    }

    #[test]
    fn read_window_decompresses_a_zlib_chunk() {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;

        let dir = tempfile_dir();
        let mut level = base_level(vec![2, 2], vec![2, 2]);
        level.compressor = Compressor::Zlib;

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&[11, 12, 13, 14]).unwrap();
        let compressed = encoder.finish().unwrap();
        write_chunk(dir.path(), &[0, 0], ".", &compressed);

        let window = PixelWindow {
            x0: 0,
            y0: 0,
            x1: 2,
            y1: 2,
        };
        let samples = read_window(&fs(&dir), &level, &[], window).unwrap();
        assert_eq!(samples, vec![11.0, 12.0, 13.0, 14.0]);
    }

    #[test]
    fn read_window_refuses_a_gzip_bomb_that_decompresses_past_the_expected_length() {
        use flate2::write::GzEncoder;
        use flate2::Compression;

        let dir = tempfile_dir();
        let mut level = base_level(vec![2, 2], vec![2, 2]);
        level.compressor = Compressor::Gzip;

        // Compresses 100 bytes into a chunk file this array's own metadata
        // declares should hold exactly 4 (2x2, u8) -- decompression must be
        // bounded to (roughly) the expected size rather than draining the
        // whole stream, and the length mismatch must still surface as a
        // named decode error, not a silent truncation.
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&[0u8; 100]).unwrap();
        let compressed = encoder.finish().unwrap();
        write_chunk(dir.path(), &[0, 0], ".", &compressed);

        let window = PixelWindow {
            x0: 0,
            y0: 0,
            x1: 2,
            y1: 2,
        };
        assert!(matches!(
            read_window(&fs(&dir), &level, &[], window),
            Err(ZarrError::Decode(_))
        ));
    }

    #[test]
    fn read_window_refuses_a_request_that_would_touch_too_many_chunks() {
        let dir = tempfile_dir();
        // 1-element chunks over a window this driver's own window budget
        // would normally allow, but whose chunk *count* is pathological.
        let level = base_level(vec![4000, 4000], vec![1, 1]);

        let window = PixelWindow {
            x0: 0,
            y0: 0,
            x1: 4000,
            y1: 4000,
        };
        match read_window(&fs(&dir), &level, &[], window) {
            Err(ZarrError::DecodeBudgetExceeded { elements, budget }) => {
                assert!(elements > budget);
            }
            other => panic!("expected DecodeBudgetExceeded, got {other:?}"),
        }
    }

    #[test]
    fn decode_budget_exceeded_maps_to_error_invalid() {
        let error: tellurion_core::Error = ZarrError::DecodeBudgetExceeded {
            elements: 100,
            budget: 10,
        }
        .into();
        assert!(matches!(error, tellurion_core::Error::Invalid(_)));
    }

    // -- remote (`http(s)`) store, driven through a loopback `MockDirServer`
    // -- proves `read_window`'s own contract holds identically whether
    // `store` is an `FsStore` or a `RemoteZarrSource`: the tests above and
    // below build the exact same fixture shapes, only swapping which store
    // reads them.

    use crate::store::RemoteZarrSource;
    use crate::test_support::{test_client, MockDirServer};

    /// Every remote test here drives [`read_window`] from inside
    /// `spawn_blocking`, the same contract this driver's own production code
    /// is bound to (`store.rs`'s own doc) — calling it directly from an
    /// async test body would try to `Handle::block_on` a runtime from a
    /// thread that runtime already owns, which panics.
    async fn in_blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        tokio::task::spawn_blocking(f).await.unwrap()
    }

    fn remote(server: &MockDirServer) -> RemoteZarrSource {
        RemoteZarrSource {
            client: test_client(),
            base_url: server.base_url(),
        }
    }

    /// The same proof as `read_window_assembles_across_multiple_chunks`, but
    /// reading through a loopback HTTP server instead of the local
    /// filesystem.
    #[tokio::test]
    async fn remote_read_window_assembles_across_multiple_chunks() {
        let dir = tempfile_dir();
        let level = base_level(vec![4, 4], vec![2, 2]);
        write_chunk(dir.path(), &[0, 0], ".", &[1, 1, 1, 1]);
        write_chunk(dir.path(), &[0, 1], ".", &[2, 2, 2, 2]);
        write_chunk(dir.path(), &[1, 0], ".", &[3, 3, 3, 3]);
        write_chunk(dir.path(), &[1, 1], ".", &[4, 4, 4, 4]);
        let server = MockDirServer::serve(dir.path().to_path_buf(), vec![]);
        let store = remote(&server);

        let window = PixelWindow {
            x0: 0,
            y0: 0,
            x1: 4,
            y1: 4,
        };
        let samples = in_blocking(move || read_window(&store, &level, &[], window))
            .await
            .unwrap();
        assert_eq!(
            samples,
            vec![1.0, 1.0, 2.0, 2.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 4.0, 4.0, 3.0, 3.0, 4.0, 4.0]
        );
    }

    /// The same proof as `read_window_treats_a_missing_chunk_file_as_fill_value`,
    /// but the "missing" fact this time is a `404` from the remote server —
    /// `store::ZarrStore::read_chunk`'s own contract treats it identically to
    /// a local not-found file.
    #[tokio::test]
    async fn remote_read_window_treats_a_missing_chunk_over_404_as_fill_value() {
        let dir = tempfile_dir();
        let mut level = base_level(vec![4, 4], vec![2, 2]);
        level.fill_value = 9.0;
        write_chunk(dir.path(), &[0, 0], ".", &[1, 1, 1, 1]);
        // chunk (0,1) is never written -- the mock server answers 404 for
        // it, same as this crate's own `store::ZarrStore::read_chunk`
        // contract requires.
        let server = MockDirServer::serve(dir.path().to_path_buf(), vec![]);
        let store = remote(&server);

        let window = PixelWindow {
            x0: 0,
            y0: 0,
            x1: 4,
            y1: 2,
        };
        let samples = in_blocking(move || read_window(&store, &level, &[], window))
            .await
            .unwrap();
        assert_eq!(samples, vec![1.0, 1.0, 9.0, 9.0, 1.0, 1.0, 9.0, 9.0]);
    }

    /// A remote chunk fetch that answers `500` (any non-2xx other than the
    /// `404` "missing chunk" case) must never be treated as fill value —
    /// that would silently fabricate data for a chunk that might genuinely
    /// exist. It surfaces as a named [`ZarrError::RemoteOpen`] instead.
    #[tokio::test]
    async fn remote_read_window_refuses_a_server_error_as_a_named_error_not_fill_value() {
        let dir = tempfile_dir();
        let level = base_level(vec![2, 2], vec![2, 2]);
        write_chunk(dir.path(), &[0, 0], ".", &[1, 1, 1, 1]);
        let server = MockDirServer::serve(dir.path().to_path_buf(), vec!["0.0".to_string()]);
        let store = remote(&server);

        let window = PixelWindow {
            x0: 0,
            y0: 0,
            x1: 2,
            y1: 2,
        };
        let error = in_blocking(move || read_window(&store, &level, &[], window))
            .await
            .unwrap_err();
        assert!(
            matches!(error, ZarrError::RemoteOpen { .. }),
            "expected RemoteOpen (never a silently fabricated fill value), got {error:?}"
        );
    }

    /// The same proof as
    /// `read_window_refuses_a_gzip_bomb_that_decompresses_past_the_expected_length`,
    /// but the compressed chunk bytes are fetched over HTTP first — the bomb
    /// guard lives in `decompress` itself, downstream of the fetch, so it
    /// must trip identically regardless of transport.
    #[tokio::test]
    async fn remote_read_window_refuses_a_gzip_bomb_over_http() {
        use flate2::write::GzEncoder;
        use flate2::Compression;

        let dir = tempfile_dir();
        let mut level = base_level(vec![2, 2], vec![2, 2]);
        level.compressor = Compressor::Gzip;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&[0u8; 100]).unwrap();
        let compressed = encoder.finish().unwrap();
        write_chunk(dir.path(), &[0, 0], ".", &compressed);
        let server = MockDirServer::serve(dir.path().to_path_buf(), vec![]);
        let store = remote(&server);

        let window = PixelWindow {
            x0: 0,
            y0: 0,
            x1: 2,
            y1: 2,
        };
        let error = in_blocking(move || read_window(&store, &level, &[], window))
            .await
            .unwrap_err();
        assert!(
            matches!(error, ZarrError::Decode(_)),
            "expected the same Decode bomb-guard refusal a local read trips, got {error:?}"
        );
    }

    /// The same proof as
    /// `read_window_refuses_a_request_that_would_touch_too_many_chunks`, over
    /// a remote store — this budget is checked purely from the requested
    /// window against `level`, before any chunk is fetched, so it must
    /// refuse identically (and without ever reaching the network) regardless
    /// of which store backs the request.
    #[tokio::test]
    async fn remote_read_window_refuses_a_request_that_would_touch_too_many_chunks() {
        let dir = tempfile_dir();
        let level = base_level(vec![4000, 4000], vec![1, 1]);
        let server = MockDirServer::serve(dir.path().to_path_buf(), vec![]);
        let store = remote(&server);

        let window = PixelWindow {
            x0: 0,
            y0: 0,
            x1: 4000,
            y1: 4000,
        };
        let error = in_blocking(move || read_window(&store, &level, &[], window))
            .await
            .unwrap_err();
        assert!(
            matches!(error, ZarrError::DecodeBudgetExceeded { .. }),
            "expected DecodeBudgetExceeded, got {error:?}"
        );
    }

    // -- fixture helper ----------------------------------------------------

    /// A private, self-cleaning temp directory -- this crate has no
    /// `tempfile` dev-dependency (every other fixture-needing driver crate
    /// in this workspace writes its own fixtures directly under `std::env::
    /// temp_dir()` too; `tellurion-geopackage`'s own binary test is the
    /// precedent this mirrors), removed on drop so a failing assertion still
    /// can't leak files across test runs.
    struct TempDir {
        path: PathBuf,
    }

    /// Disambiguates two [`TempDir::new`] calls that land on the same
    /// wall-clock tick — `std::process::id()` is identical across every
    /// thread in one test binary, and every call here shares the same
    /// `"reader"` label (`tempfile_dir`), so under enough parallel test
    /// threads two fixtures could otherwise collide on the same directory
    /// name and race each other's `Drop::drop` cleanup mid-test.
    static NEXT_TEMP_DIR_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    impl TempDir {
        fn new(label: &str) -> Self {
            let mut path = std::env::temp_dir();
            let unique = NEXT_TEMP_DIR_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            path.push(format!(
                "tellurion-zarr-test-{label}-{}-{}-{unique}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn tempfile_dir() -> TempDir {
        TempDir::new("reader")
    }
}
