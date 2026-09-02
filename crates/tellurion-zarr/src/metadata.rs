//! Pure parsing/validation for Zarr v2's two JSON metadata documents —
//! `.zarray` (array shape/chunking/dtype/compressor) and `.zattrs` (this
//! driver's own georeferencing declaration). No I/O here; `reader.rs` reads
//! the bytes off disk and calls into this module, the same split
//! `tellurion-cog` draws between `reader.rs` (I/O) and `geokeys.rs` (pure
//! tag parsing).
//!
//! First-slice support only (`#37`): `zarr_format` 2, `order: "C"` (row-major;
//! Fortran order is refused by name), an empty/absent `filters` pipeline, a
//! `dimension_separator` of `.` or `/`, and a compressor of `null` (raw),
//! `gzip`, or `zlib`. Every array must have rank >= 2 (its last two
//! dimensions are always read as `(y, x)`, the same row-before-column
//! convention `numpy`/`xarray`/rasterio raster arrays already use); any
//! dimensions before those two are addressed by a fixed index per request
//! (`GeoRef::fixed_index`), never varied on the wire in this slice.
//!
//! Zarr v2 has no single standard way to embed a CRS/extent in an array's own
//! metadata (unlike GeoTIFF's GeoKeys), and this driver refuses to guess at
//! one of the several competing, ambiguous conventions (CF, xarray,
//! rioxarray) in the wild. Instead it looks for its own explicit,
//! unambiguous declaration in `.zattrs`: `tellurion:extent_crs84` (required)
//! and `tellurion:fixed_index` (optional, defaults to index 0 on every
//! leading dimension). A store that doesn't declare `tellurion:extent_crs84`
//! is refused by name rather than served with a guessed extent — see this
//! module's own doc and `crate` doc for the full reasoning.
//!
//! ## Multiscale pyramids (`multiscales`)
//!
//! Unlike georeferencing, a resolution pyramid is NOT something this driver
//! invents its own key for: the OME-NGFF ("Next-generation file formats")
//! `multiscales` attribute is already the widely-adopted convention for
//! exactly this — a group's `.zattrs` declares a `multiscales` array, whose
//! first entry's `datasets` list names, in finest-to-coarsest order, the
//! sub-array (relative `path`) holding each resolution level, each required
//! to carry its own `coordinateTransformations`. GeoZarr's own multiscales
//! convention is explicitly built on this same shape rather than replacing
//! it. [`parse_multiscales`] consumes exactly that: the `datasets[].path`
//! list. It deliberately does NOT read or trust `coordinateTransformations`'
//! declared `scale` values for level selection — this driver already derives
//! each level's own resolution from that level's own `.zarray` `shape`
//! (`reader::open`'s per-level parse), the same "derive from the data actually
//! read, not a redundant declared value" choice `tellurion-cog::reader::open`
//! makes for its own overview pyramid (no per-IFD scale factor is trusted
//! there either — see that crate's own `Level` doc). `coordinateTransformations`
//! is still required to be *present* (a non-empty array) on every dataset
//! entry: that's what the spec actually mandates, and checking for it is how
//! this driver tells a genuine OME-NGFF-shaped `multiscales` document apart
//! from an unrelated key that happens to share the name.

use serde::Deserialize;
use serde_json::Value;

use crate::error::{Result, ZarrError};

/// Hard per-chunk element cap (product of every dimension in `.zarray`'s own
/// `chunks` array), checked once here at metadata-parse time rather than per
/// request — a chunk file's decompressed size is fixed by this shape alone
/// (Zarr v2 pads every chunk, including boundary ones, to exactly this many
/// elements), so a misconfigured store whose chunk shape is too large to ever
/// safely decompress fails at open/boot time, the same "bad config fails
/// fast" contract `tellurion-cog`'s own striped-layout refusal uses. Sized so
/// one chunk's own worst-case buffer (this many elements at up to 8 bytes
/// each, `f64`) stays in the tens-of-megabytes range.
pub const MAX_CHUNK_ELEMENTS: u64 = 4_000_000;

/// Hard cap on an array's own rank (`.zarray`'s `shape`/`chunks` length).
/// [`MAX_CHUNK_ELEMENTS`] bounds the PRODUCT of a chunk's own dimensions,
/// not their COUNT — a `.zarray` declaring a huge number of length-1
/// dimensions (`"shape":[1,1,1,...]`) would sail under that budget (product
/// stays 1) while still driving rank-proportional allocations on every
/// request `reader::read_window` serves (its own stride/leading-chunk-index
/// vectors are sized off rank). Real geospatial use never comes close to
/// this: the OME-NGFF `multiscales` convention this crate's own pyramid
/// support consumes (see `crate`'s own doc) caps its own `axes` list at 5
/// entries (time, channel, z, y, x) — this ceiling leaves generous headroom
/// above that for a plain (non-multiscale) array too, while still refusing
/// a rank chosen to be pathological rather than descriptive.
pub const MAX_RANK: usize = 8;

/// This driver's fixed dtype set (`#37`): 8/16/32-bit integers (both
/// signedness), plus 32/64-bit float, always little-endian. A 1-byte dtype's
/// byte-order marker is irrelevant (there is no byte to order) so any marker
/// is accepted for `u1`/`i1`; every multi-byte dtype must declare `<`
/// (little-endian) explicitly. Anything else — unsigned 32-bit, any 64-bit
/// integer, `float16`, a big-endian multi-byte dtype, or a structured/
/// extended dtype — is refused by name rather than silently widened,
/// truncated, or byte-swapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DType {
    U8,
    I8,
    U16,
    I16,
    I32,
    F32,
    F64,
}

impl DType {
    pub fn size_bytes(self) -> usize {
        match self {
            DType::U8 | DType::I8 => 1,
            DType::U16 | DType::I16 => 2,
            DType::I32 | DType::F32 => 4,
            DType::F64 => 8,
        }
    }

    /// Decodes one little-endian sample (`size_bytes()` bytes) to `f64` —
    /// lossless for every dtype in this fixed set except `i32`/`u16`'s own
    /// full 32/16-bit range, both comfortably inside `f64`'s 53-bit mantissa.
    pub fn decode(self, bytes: &[u8]) -> f64 {
        match self {
            DType::U8 => f64::from(bytes[0]),
            DType::I8 => f64::from(bytes[0] as i8),
            DType::U16 => f64::from(u16::from_le_bytes([bytes[0], bytes[1]])),
            DType::I16 => f64::from(i16::from_le_bytes([bytes[0], bytes[1]])),
            DType::I32 => f64::from(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
            DType::F32 => f64::from(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
            DType::F64 => f64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]),
        }
    }

    /// Parses a Zarr v2 dtype descriptor string (numpy `str` convention:
    /// `[byteorder][kind][itemsize]`, e.g. `<f4`, `|u1`) against this
    /// driver's fixed set.
    fn parse(raw: &str) -> Result<Self> {
        let mut chars = raw.chars();
        let byteorder = chars
            .next()
            .ok_or_else(|| ZarrError::Unsupported(format!("dtype '{raw}' is empty")))?;
        let kind = chars
            .next()
            .ok_or_else(|| ZarrError::Unsupported(format!("dtype '{raw}' has no type kind")))?;
        let itemsize: usize = chars.as_str().parse().map_err(|_| {
            ZarrError::Unsupported(format!("dtype '{raw}' has a non-numeric item size"))
        })?;
        if itemsize > 1 && byteorder != '<' {
            return Err(ZarrError::Unsupported(format!(
                "dtype '{raw}' is not little-endian; only an explicit little-endian ('<') byte order is supported for a multi-byte dtype"
            )));
        }
        match (kind, itemsize) {
            ('u', 1) => Ok(DType::U8),
            ('i', 1) => Ok(DType::I8),
            ('u', 2) => Ok(DType::U16),
            ('i', 2) => Ok(DType::I16),
            ('i', 4) => Ok(DType::I32),
            ('f', 4) => Ok(DType::F32),
            ('f', 8) => Ok(DType::F64),
            _ => Err(ZarrError::Unsupported(format!(
                "dtype '{raw}' is not in this driver's supported set (u8/i8/u16/i16/i32/f32/f64, little-endian)"
            ))),
        }
    }
}

/// This driver's fixed compressor set: raw (`.zarray`'s `compressor: null`),
/// or the two numcodecs whose decompression is already pure-Rust in this
/// workspace's dependency graph (`gzip`/`zlib`, both via `flate2`). Anything
/// else (`blosc`, `lz4`, `zstd`, `bz2`, ...) is refused by its own declared
/// `id` rather than silently ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compressor {
    Raw,
    Gzip,
    Zlib,
}

fn parse_compressor(value: &Value) -> Result<Compressor> {
    match value {
        Value::Null => Ok(Compressor::Raw),
        Value::Object(map) => {
            let id = map.get("id").and_then(Value::as_str).ok_or_else(|| {
                ZarrError::Unsupported("compressor object has no string 'id' field".to_string())
            })?;
            match id {
                "gzip" => Ok(Compressor::Gzip),
                "zlib" => Ok(Compressor::Zlib),
                other => Err(ZarrError::Unsupported(format!(
                    "compressor '{other}' is not supported; only raw (null), gzip, and zlib are"
                ))),
            }
        }
        other => Err(ZarrError::Unsupported(format!(
            "'compressor' has an unexpected shape ({other}); expected null or a codec object"
        ))),
    }
}

fn check_no_filters(value: &Value) -> Result<()> {
    match value {
        Value::Null => Ok(()),
        Value::Array(items) if items.is_empty() => Ok(()),
        Value::Array(items) => Err(ZarrError::Unsupported(format!(
            "this array declares {} filter(s); a filter pipeline is not supported",
            items.len()
        ))),
        other => Err(ZarrError::Unsupported(format!(
            "'filters' has an unexpected shape ({other}); expected null or an array"
        ))),
    }
}

fn parse_fill_value(value: &Value) -> Result<f64> {
    match value {
        Value::Null => Ok(0.0),
        Value::Number(n) => n.as_f64().ok_or_else(|| {
            ZarrError::Unsupported(format!(
                "fill_value {n} is not representable as a floating-point number"
            ))
        }),
        other => Err(ZarrError::Unsupported(format!(
            "fill_value {other} is not supported; only null or a plain number is (a string sentinel like \"NaN\" is not)"
        ))),
    }
}

#[derive(Debug, Deserialize)]
struct RawZarray {
    zarr_format: i64,
    shape: Vec<u64>,
    chunks: Vec<u64>,
    dtype: String,
    #[serde(default)]
    compressor: Value,
    #[serde(default)]
    fill_value: Value,
    order: String,
    #[serde(default)]
    filters: Value,
    #[serde(default = "default_separator")]
    dimension_separator: String,
}

fn default_separator() -> String {
    ".".to_string()
}

/// A validated `.zarray` document — this driver's first-slice support only,
/// see this module's own doc.
#[derive(Debug, Clone, PartialEq)]
pub struct ZarrayMeta {
    pub shape: Vec<u64>,
    pub chunks: Vec<u64>,
    pub dtype: DType,
    pub compressor: Compressor,
    pub fill_value: f64,
    pub dimension_separator: String,
}

pub fn parse_zarray(bytes: &[u8]) -> Result<ZarrayMeta> {
    let raw: RawZarray = serde_json::from_slice(bytes)
        .map_err(|error| ZarrError::Decode(format!(".zarray is not valid JSON: {error}")))?;

    if raw.zarr_format != 2 {
        return Err(ZarrError::Unsupported(format!(
            "zarr_format {} is not supported; only Zarr v2 is",
            raw.zarr_format
        )));
    }
    if raw.order != "C" {
        return Err(ZarrError::Unsupported(format!(
            "order '{}' is not supported; only C (row-major) order is",
            raw.order
        )));
    }
    if raw.shape.len() != raw.chunks.len() {
        return Err(ZarrError::Unsupported(format!(
            "shape has rank {} but chunks has rank {}; they must match",
            raw.shape.len(),
            raw.chunks.len()
        )));
    }
    if raw.shape.len() < 2 {
        return Err(ZarrError::Unsupported(format!(
            "array has rank {}; at least 2 dimensions (y, x) are required",
            raw.shape.len()
        )));
    }
    if raw.shape.len() > MAX_RANK {
        return Err(ZarrError::Unsupported(format!(
            "array has rank {}, over this driver's rank budget of {MAX_RANK}; a huge number of \
             dimensions drives rank-proportional allocations on every request regardless of \
             each dimension's own length",
            raw.shape.len()
        )));
    }
    // `reader::ZarrLevel::width`/`height` narrow exactly these two
    // dimensions to `u32` (every pixel coordinate in this crate is `u32`,
    // matching `tellurion_core::TileCoord`'s own width) — a declared shape
    // wider than that would silently wrap (`5_000_000_000 as u32 ==
    // 705_032_704`, not an error), and every downstream computation
    // (`tiling::select_overview`'s `deg_per_px`, `plan_window`'s
    // scale_x/scale_y, world-bounds clamping) would then compute against a
    // wrong extent and serve wrong imagery with no error at all. Refused
    // here, before that cast is ever reached, rather than silently wrapped.
    let rank = raw.shape.len();
    let (height, width) = (raw.shape[rank - 2], raw.shape[rank - 1]);
    if height > u64::from(u32::MAX) || width > u64::from(u32::MAX) {
        return Err(ZarrError::Unsupported(format!(
            "array's trailing (y, x) shape is {height}x{width}, which does not fit in this \
             driver's u32 pixel-coordinate width (max {}); a dimension this large cannot be \
             served",
            u32::MAX
        )));
    }
    if raw.shape.contains(&0) || raw.chunks.contains(&0) {
        return Err(ZarrError::Unsupported(
            "shape/chunks must not contain a zero-length dimension".to_string(),
        ));
    }
    if raw.dimension_separator != "." && raw.dimension_separator != "/" {
        return Err(ZarrError::Unsupported(format!(
            "dimension_separator '{}' is not supported; only '.' or '/' is",
            raw.dimension_separator
        )));
    }
    check_no_filters(&raw.filters)?;
    let dtype = DType::parse(&raw.dtype)?;
    let compressor = parse_compressor(&raw.compressor)?;
    let fill_value = parse_fill_value(&raw.fill_value)?;

    let chunk_elements: u128 = raw.chunks.iter().map(|&c| u128::from(c)).product();
    if chunk_elements > u128::from(MAX_CHUNK_ELEMENTS) {
        return Err(ZarrError::Unsupported(format!(
            "a single chunk has {chunk_elements} elements, over this driver's per-chunk budget of {MAX_CHUNK_ELEMENTS}"
        )));
    }

    Ok(ZarrayMeta {
        shape: raw.shape,
        chunks: raw.chunks,
        dtype,
        compressor,
        fill_value,
        dimension_separator: raw.dimension_separator,
    })
}

#[derive(Debug, Deserialize)]
struct RawZattrs {
    #[serde(rename = "tellurion:extent_crs84")]
    extent_crs84: Option<[f64; 4]>,
    #[serde(rename = "tellurion:fixed_index")]
    fixed_index: Option<Vec<u64>>,
    /// OME-NGFF's own pyramid declaration — see this module's own "Multiscale
    /// pyramids" doc. Lives in the same `.zattrs` document as this driver's
    /// `tellurion:*` keys (composing two conventions in one attributes
    /// document is exactly how OME-NGFF/GeoZarr expect independent
    /// conventions to coexist), so [`parse_multiscales`] parses the same
    /// bytes [`parse_zattrs_georef`] does, rather than a separate document.
    multiscales: Option<Vec<RawMultiscale>>,
}

#[derive(Debug, Deserialize)]
struct RawMultiscale {
    datasets: Vec<RawMultiscaleDataset>,
}

#[derive(Debug, Deserialize)]
struct RawMultiscaleDataset {
    path: String,
    #[serde(rename = "coordinateTransformations", default)]
    coordinate_transformations: Vec<Value>,
}

/// Rejects a `multiscales[0].datasets[].path` that could escape the store
/// root once [`crate::store::ScopedStore`] joins it onto a document/chunk
/// name (`"{path}/{name}"`, plain string concatenation — see that type's own
/// doc). This is the ONE gate for that value, checked here at parse time
/// rather than at every join call site, exactly because the value is
/// untrusted `.zattrs` content the same way `dtype`/`compressor`/
/// `dimension_separator` already are elsewhere in this module.
///
/// Three shapes are refused, each a distinct real escape:
/// - A leading `/` or `\`: [`std::path::Path::join`] documents that joining
///   an ABSOLUTE argument onto a base path REPLACES the base entirely — a
///   local [`crate::store::FsStore`] root would be discarded outright,
///   turning a level "path" into an arbitrary local file read. This also
///   catches a network-path reference (`"//host/path"`, RFC 3986) against a
///   remote store, which keeps the base URL's scheme but swaps its host.
/// - A `:` anywhere: against a remote [`crate::store::RemoteZarrSource`],
///   `Url::join` treats a relative reference that itself carries a scheme
///   (`"http://host/path"`) as an ABSOLUTE URL, ignoring the configured base
///   entirely — a server-side-request-forgery primitive. Also catches a
///   Windows drive letter (`"C:\path"`) for the same reason as the leading
///   backslash case above.
/// - A `..` path segment (split on both `/` and `\`, so `"../x"`, `"a/../
///   ../etc"`, and `"..\\..\\etc"` are all caught): plain string
///   concatenation never normalizes `..` away before the underlying
///   filesystem `open()` or the joined URL's own request, so this alone
///   would walk back out of the store root even with no absolute component
///   at all.
fn validate_dataset_path(path: &str) -> Result<()> {
    if path.starts_with('/') || path.starts_with('\\') {
        return Err(ZarrError::Unsupported(format!(
            "multiscale dataset path '{path}' is absolute (or a network-path reference); only \
             a path relative to the store root is allowed"
        )));
    }
    if path.contains(':') {
        return Err(ZarrError::Unsupported(format!(
            "multiscale dataset path '{path}' contains ':' (a URL scheme or a drive letter); \
             only a plain relative path is allowed"
        )));
    }
    if path.split(['/', '\\']).any(|segment| segment == "..") {
        return Err(ZarrError::Unsupported(format!(
            "multiscale dataset path '{path}' contains a '..' path-traversal component; only a \
             path confined to the store root is allowed"
        )));
    }
    Ok(())
}

/// Parses `.zattrs`'s bytes for an OME-NGFF-shaped `multiscales` pyramid
/// declaration (this module's own doc explains why this driver consumes that
/// convention rather than inventing a private one). `Ok(None)` means the
/// document simply has no `multiscales` key at all — not malformed, just
/// declaring no pyramid; the caller (`reader::open`) decides what that means
/// for a `.zgroup` store (refuse, since neither a single array nor a
/// pyramid was found). `Ok(Some(paths))` is `multiscales[0].datasets[].path`,
/// in the document's own declared order (`reader::open` re-sorts these
/// finest-first by each level's own real `.zarray` shape once opened, the
/// same "never trust declared order" defense
/// `tellurion-cog::reader::open` applies to a COG's own overview IFD walk).
/// Only the FIRST `multiscales` entry is read — this driver serves one
/// array per collection (`crate`'s own doc), so a document declaring more
/// than one multiscale image has nothing further to disambiguate which one
/// this collection means. Every dataset's own `path` is validated by
/// [`validate_dataset_path`] before it is ever returned.
pub fn parse_multiscales(bytes: &[u8]) -> Result<Option<Vec<String>>> {
    let raw: RawZattrs = serde_json::from_slice(bytes)
        .map_err(|error| ZarrError::Decode(format!(".zattrs is not valid JSON: {error}")))?;
    let Some(multiscales) = raw.multiscales else {
        return Ok(None);
    };
    let Some(first) = multiscales.into_iter().next() else {
        return Err(ZarrError::Unsupported(
            "'.zattrs' declares an empty 'multiscales' array; an OME-NGFF multiscale image needs at least one entry".to_string(),
        ));
    };
    if first.datasets.is_empty() {
        return Err(ZarrError::Unsupported(
            "'.zattrs' 'multiscales[0]' declares no 'datasets'; a multiscale pyramid needs at least one resolution level".to_string(),
        ));
    }

    let mut paths = Vec::with_capacity(first.datasets.len());
    for dataset in first.datasets {
        if dataset.path.is_empty() {
            return Err(ZarrError::Unsupported(
                "a 'multiscales[0].datasets' entry has an empty 'path'".to_string(),
            ));
        }
        validate_dataset_path(&dataset.path)?;
        if dataset.coordinate_transformations.is_empty() {
            return Err(ZarrError::Unsupported(format!(
                "multiscale dataset '{}' declares no 'coordinateTransformations'; the OME-NGFF \
                 spec requires at least one entry per dataset (this driver doesn't trust its \
                 declared 'scale' for level selection -- it derives each level's own resolution \
                 from that level's own '.zarray' shape instead -- but requires the field's \
                 presence to tell a genuine OME-NGFF-shaped 'multiscales' document apart from an \
                 unrelated key that happens to share the name)",
                dataset.path
            )));
        }
        paths.push(dataset.path);
    }
    Ok(Some(paths))
}

/// This driver's own georeferencing declaration, read from `.zattrs` — see
/// this module's own doc for why it lives here rather than in `config.yaml`
/// or a guessed-at CF/xarray attribute.
#[derive(Debug, Clone, PartialEq)]
pub struct GeoRef {
    /// `[minx, miny, maxx, maxy]` in CRS84 (lon/lat, WGS84 degrees) — the
    /// extent the array's trailing `(y, x)` dimensions span, edge to edge.
    /// Always related to CRS84 by a plain axis-aligned linear transform (the
    /// same restriction `tellurion-cog` places on an EPSG:4326 GeoTIFF); a
    /// store in any other CRS is out of this slice's scope.
    pub extent_crs84: [f64; 4],
    /// Fixed index selected for each dimension before the trailing `(y, x)`
    /// pair, in the array's own axis order. Empty for a plain 2D array.
    pub fixed_index: Vec<u64>,
}

/// Parses `.zattrs`'s bytes for this driver's own georeferencing keys.
/// `rank` is the array's own dimensionality (from `.zarray`) — needed only to
/// default `fixed_index` to the right length when the key is absent; a
/// present `fixed_index` whose length doesn't match `rank - 2` is refused
/// here, and per-dimension bounds checking (each index against its own
/// dimension's real length) happens in `reader::open`, once `.zarray`'s
/// `shape` is available too.
pub fn parse_zattrs_georef(bytes: &[u8], rank: usize) -> Result<GeoRef> {
    let raw: RawZattrs = serde_json::from_slice(bytes)
        .map_err(|error| ZarrError::Decode(format!(".zattrs is not valid JSON: {error}")))?;

    let extent_crs84 = raw.extent_crs84.ok_or_else(|| {
        ZarrError::Unsupported(
            "'.zattrs' does not declare 'tellurion:extent_crs84'; this driver refuses to guess a Zarr array's georeferencing"
                .to_string(),
        )
    })?;
    if !(extent_crs84[0] < extent_crs84[2] && extent_crs84[1] < extent_crs84[3]) {
        return Err(ZarrError::Unsupported(format!(
            "'tellurion:extent_crs84' {extent_crs84:?} is not a valid [minx, miny, maxx, maxy] box (min must be less than max on each axis)"
        )));
    }

    let leading_rank = rank.saturating_sub(2);
    let fixed_index = raw.fixed_index.unwrap_or_else(|| vec![0; leading_rank]);
    if fixed_index.len() != leading_rank {
        return Err(ZarrError::Unsupported(format!(
            "'tellurion:fixed_index' has {} entries, but this array has {leading_rank} dimension(s) besides its trailing (y, x) pair",
            fixed_index.len()
        )));
    }

    Ok(GeoRef {
        extent_crs84,
        fixed_index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zarray_json(extra: &str) -> String {
        format!(
            r#"{{"zarr_format":2,"shape":[4,4],"chunks":[2,2],"dtype":"<f4","compressor":null,"fill_value":0,"order":"C"{extra}}}"#
        )
    }

    #[test]
    fn parses_a_minimal_well_formed_zarray() {
        let meta = parse_zarray(zarray_json("").as_bytes()).unwrap();
        assert_eq!(meta.shape, vec![4, 4]);
        assert_eq!(meta.chunks, vec![2, 2]);
        assert_eq!(meta.dtype, DType::F32);
        assert_eq!(meta.compressor, Compressor::Raw);
        assert_eq!(meta.fill_value, 0.0);
        assert_eq!(meta.dimension_separator, ".");
    }

    #[test]
    fn rejects_zarr_format_3() {
        let json = zarray_json("").replace("\"zarr_format\":2", "\"zarr_format\":3");
        assert!(matches!(
            parse_zarray(json.as_bytes()),
            Err(ZarrError::Unsupported(msg)) if msg.contains("zarr_format")
        ));
    }

    #[test]
    fn rejects_fortran_order() {
        let json = zarray_json("").replace("\"order\":\"C\"", "\"order\":\"F\"");
        assert!(matches!(
            parse_zarray(json.as_bytes()),
            Err(ZarrError::Unsupported(msg)) if msg.contains("order")
        ));
    }

    #[test]
    fn rejects_a_rank_one_array() {
        let json = zarray_json("").replace("\"shape\":[4,4]", "\"shape\":[4]");
        let json = json.replace("\"chunks\":[2,2]", "\"chunks\":[2]");
        assert!(matches!(
            parse_zarray(json.as_bytes()),
            Err(ZarrError::Unsupported(msg)) if msg.contains("rank")
        ));
    }

    /// A `.zarray` declaring far more leading dimensions than any real
    /// geospatial array needs (each of length 1, so
    /// `MAX_CHUNK_ELEMENTS`'s own product-based budget alone would never
    /// catch this) is refused on rank alone.
    #[test]
    fn rejects_a_pathologically_high_rank_even_with_every_dimension_length_one() {
        let rank = MAX_RANK + 1;
        let shape = format!("[{}]", vec!["1"; rank].join(","));
        let json = zarray_json("")
            .replace("\"shape\":[4,4]", &format!("\"shape\":{shape}"))
            .replace("\"chunks\":[2,2]", &format!("\"chunks\":{shape}"));
        match parse_zarray(json.as_bytes()) {
            Err(ZarrError::Unsupported(msg)) => assert!(msg.contains("rank"), "message was: {msg}"),
            other => panic!("expected a named refusal, got {other:?}"),
        }
    }

    #[test]
    fn rejects_mismatched_shape_and_chunks_rank() {
        let json = zarray_json("").replace("\"chunks\":[2,2]", "\"chunks\":[2,2,2]");
        assert!(matches!(
            parse_zarray(json.as_bytes()),
            Err(ZarrError::Unsupported(_))
        ));
    }

    #[test]
    fn rejects_a_nonempty_filter_pipeline() {
        let json = zarray_json(r#","filters":[{"id":"delta"}]"#);
        assert!(matches!(
            parse_zarray(json.as_bytes()),
            Err(ZarrError::Unsupported(msg)) if msg.contains("filter")
        ));
    }

    #[test]
    fn accepts_an_empty_filter_pipeline() {
        assert!(parse_zarray(zarray_json(r#","filters":[]"#).as_bytes()).is_ok());
    }

    #[test]
    fn rejects_an_unsupported_dimension_separator() {
        let json = zarray_json(",\"dimension_separator\":\"|\"");
        assert!(matches!(
            parse_zarray(json.as_bytes()),
            Err(ZarrError::Unsupported(_))
        ));
    }

    #[test]
    fn accepts_the_slash_dimension_separator() {
        let meta = parse_zarray(zarray_json(r#","dimension_separator":"/""#).as_bytes()).unwrap();
        assert_eq!(meta.dimension_separator, "/");
    }

    #[test]
    fn rejects_a_chunk_shape_over_the_element_budget() {
        let json = zarray_json("")
            .replace("\"shape\":[4,4]", "\"shape\":[10000,10000]")
            .replace("\"chunks\":[2,2]", "\"chunks\":[10000,10000]");
        assert!(matches!(
            parse_zarray(json.as_bytes()),
            Err(ZarrError::Unsupported(msg)) if msg.contains("budget")
        ));
    }

    /// A plausible sub-metre global raster dimension (`5_000_000_000`) is
    /// nowhere near an absurd value, but it overflows `u32` -- if this
    /// weren't refused here, `reader::ZarrLevel::width`/`height`'s own `as
    /// u32` cast would silently wrap it to `705_032_704` instead, and every
    /// downstream computation would proceed against that wrong number with
    /// no error at all. `chunks` is kept tiny and separate from `shape` here
    /// specifically so this refusal is attributable to the shape check
    /// alone, not a chunk-element-budget refusal that happens to also fire.
    #[test]
    fn rejects_a_y_or_x_shape_dimension_that_overflows_u32() {
        let json = zarray_json("").replace("\"shape\":[4,4]", "\"shape\":[5000000000,5000000000]");
        match parse_zarray(json.as_bytes()) {
            Err(ZarrError::Unsupported(msg)) => {
                assert!(msg.contains("u32"), "message was: {msg}");
            }
            other => panic!("expected a named refusal, got {other:?}"),
        }
    }

    #[test]
    fn accepts_a_y_or_x_shape_dimension_right_at_the_u32_boundary() {
        let json = zarray_json("")
            .replace(
                "\"shape\":[4,4]",
                &format!("\"shape\":[{},{}]", u32::MAX, u32::MAX),
            )
            .replace("\"chunks\":[2,2]", "\"chunks\":[1,1]");
        // `u32::MAX` itself must still be accepted -- only a dimension that
        // actually exceeds the pixel-coordinate width is refused.
        assert!(
            parse_zarray(json.as_bytes()).is_ok(),
            "a dimension exactly at u32::MAX must not be refused"
        );
    }

    // -- dtype -----------------------------------------------------------

    #[test]
    fn parses_every_dtype_in_the_fixed_set() {
        for (raw, expected) in [
            ("|u1", DType::U8),
            ("<i1", DType::I8),
            ("|i1", DType::I8),
            ("<u2", DType::U16),
            ("<i2", DType::I16),
            ("<i4", DType::I32),
            ("<f4", DType::F32),
            ("<f8", DType::F64),
        ] {
            assert_eq!(DType::parse(raw).unwrap(), expected, "dtype {raw}");
        }
    }

    #[test]
    fn rejects_big_endian_multibyte_dtypes() {
        assert!(matches!(
            DType::parse(">f4"),
            Err(ZarrError::Unsupported(_))
        ));
        assert!(matches!(
            DType::parse(">i2"),
            Err(ZarrError::Unsupported(_))
        ));
    }

    #[test]
    fn rejects_unsigned_32_bit_even_though_its_neighbors_are_supported() {
        assert!(matches!(
            DType::parse("<u4"),
            Err(ZarrError::Unsupported(_))
        ));
    }

    #[test]
    fn rejects_64_bit_integers_and_float16() {
        assert!(matches!(
            DType::parse("<i8"),
            Err(ZarrError::Unsupported(_))
        ));
        assert!(matches!(
            DType::parse("<u8"),
            Err(ZarrError::Unsupported(_))
        ));
        assert!(matches!(
            DType::parse("<f2"),
            Err(ZarrError::Unsupported(_))
        ));
    }

    #[test]
    fn decodes_little_endian_samples_for_every_dtype() {
        assert_eq!(DType::U8.decode(&[200]), 200.0);
        assert_eq!(DType::I8.decode(&[(-5i8) as u8]), -5.0);
        assert_eq!(DType::U16.decode(&500u16.to_le_bytes()), 500.0);
        assert_eq!(DType::I16.decode(&(-500i16).to_le_bytes()), -500.0);
        assert_eq!(DType::I32.decode(&(-70000i32).to_le_bytes()), -70000.0);
        assert_eq!(DType::F32.decode(&1.5f32.to_le_bytes()), 1.5);
        assert_eq!(DType::F64.decode(&2.5f64.to_le_bytes()), 2.5);
    }

    // -- compressor / filters / fill_value --------------------------------

    #[test]
    fn parses_raw_gzip_and_zlib_compressors() {
        assert_eq!(parse_compressor(&Value::Null).unwrap(), Compressor::Raw);
        assert_eq!(
            parse_compressor(&serde_json::json!({"id": "gzip", "level": 5})).unwrap(),
            Compressor::Gzip
        );
        assert_eq!(
            parse_compressor(&serde_json::json!({"id": "zlib", "level": 5})).unwrap(),
            Compressor::Zlib
        );
    }

    #[test]
    fn rejects_an_unsupported_compressor_by_name() {
        match parse_compressor(&serde_json::json!({"id": "blosc"})) {
            Err(ZarrError::Unsupported(msg)) => assert!(msg.contains("blosc")),
            other => panic!("expected Unsupported naming 'blosc', got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_string_fill_value_sentinel() {
        assert!(matches!(
            parse_fill_value(&Value::String("NaN".to_string())),
            Err(ZarrError::Unsupported(_))
        ));
    }

    #[test]
    fn a_null_fill_value_defaults_to_zero() {
        assert_eq!(parse_fill_value(&Value::Null).unwrap(), 0.0);
    }

    // -- .zattrs georeferencing -------------------------------------------

    #[test]
    fn parses_an_explicit_extent_and_fixed_index() {
        let json =
            r#"{"tellurion:extent_crs84":[-10.0,-5.0,10.0,5.0],"tellurion:fixed_index":[3]}"#;
        let georef = parse_zattrs_georef(json.as_bytes(), 3).unwrap();
        assert_eq!(georef.extent_crs84, [-10.0, -5.0, 10.0, 5.0]);
        assert_eq!(georef.fixed_index, vec![3]);
    }

    #[test]
    fn fixed_index_defaults_to_all_zero_when_absent() {
        let json = r#"{"tellurion:extent_crs84":[-10.0,-5.0,10.0,5.0]}"#;
        let georef = parse_zattrs_georef(json.as_bytes(), 4).unwrap();
        assert_eq!(georef.fixed_index, vec![0, 0]);
    }

    #[test]
    fn a_plain_2d_array_defaults_to_an_empty_fixed_index() {
        let json = r#"{"tellurion:extent_crs84":[-10.0,-5.0,10.0,5.0]}"#;
        let georef = parse_zattrs_georef(json.as_bytes(), 2).unwrap();
        assert!(georef.fixed_index.is_empty());
    }

    #[test]
    fn refuses_a_store_with_no_declared_extent() {
        match parse_zattrs_georef(b"{}", 2) {
            Err(ZarrError::Unsupported(msg)) => assert!(msg.contains("extent_crs84")),
            other => panic!("expected a named refusal, got {other:?}"),
        }
    }

    #[test]
    fn refuses_an_inverted_extent_box() {
        let json = r#"{"tellurion:extent_crs84":[10.0,-5.0,-10.0,5.0]}"#;
        assert!(matches!(
            parse_zattrs_georef(json.as_bytes(), 2),
            Err(ZarrError::Unsupported(_))
        ));
    }

    #[test]
    fn refuses_a_fixed_index_of_the_wrong_length() {
        let json =
            r#"{"tellurion:extent_crs84":[-10.0,-5.0,10.0,5.0],"tellurion:fixed_index":[1,2]}"#;
        match parse_zattrs_georef(json.as_bytes(), 3) {
            Err(ZarrError::Unsupported(msg)) => assert!(msg.contains("fixed_index")),
            other => panic!("expected a named refusal, got {other:?}"),
        }
    }

    // -- `multiscales` (OME-NGFF pyramid declaration) ---------------------

    #[test]
    fn absent_multiscales_key_parses_to_none() {
        let json = r#"{"tellurion:extent_crs84":[-10.0,-5.0,10.0,5.0]}"#;
        assert_eq!(parse_multiscales(json.as_bytes()).unwrap(), None);
    }

    #[test]
    fn parses_an_ome_ngff_shaped_multiscales_declaration_in_document_order() {
        let json = r#"{"multiscales":[{"version":"0.4","axes":[{"name":"y","type":"space"},{"name":"x","type":"space"}],"datasets":[{"path":"0","coordinateTransformations":[{"type":"scale","scale":[1.0,1.0]}]},{"path":"1","coordinateTransformations":[{"type":"scale","scale":[2.0,2.0]}]}]}]}"#;
        let paths = parse_multiscales(json.as_bytes()).unwrap().unwrap();
        assert_eq!(paths, vec!["0".to_string(), "1".to_string()]);
    }

    #[test]
    fn refuses_an_empty_multiscales_array() {
        let json = r#"{"multiscales":[]}"#;
        match parse_multiscales(json.as_bytes()) {
            Err(ZarrError::Unsupported(msg)) => assert!(msg.contains("multiscales")),
            other => panic!("expected a named refusal, got {other:?}"),
        }
    }

    #[test]
    fn refuses_multiscales_with_no_datasets() {
        let json = r#"{"multiscales":[{"axes":[],"datasets":[]}]}"#;
        match parse_multiscales(json.as_bytes()) {
            Err(ZarrError::Unsupported(msg)) => assert!(msg.contains("datasets")),
            other => panic!("expected a named refusal, got {other:?}"),
        }
    }

    #[test]
    fn refuses_a_dataset_with_no_coordinate_transformations() {
        let json = r#"{"multiscales":[{"axes":[],"datasets":[{"path":"0"}]}]}"#;
        match parse_multiscales(json.as_bytes()) {
            Err(ZarrError::Unsupported(msg)) => {
                assert!(
                    msg.contains("coordinateTransformations"),
                    "message was: {msg}"
                );
            }
            other => panic!("expected a named refusal, got {other:?}"),
        }
    }

    #[test]
    fn refuses_a_dataset_with_an_empty_path() {
        let json = r#"{"multiscales":[{"axes":[],"datasets":[{"path":"","coordinateTransformations":[{"type":"scale","scale":[1.0,1.0]}]}]}]}"#;
        match parse_multiscales(json.as_bytes()) {
            Err(ZarrError::Unsupported(msg)) => assert!(msg.contains("path")),
            other => panic!("expected a named refusal, got {other:?}"),
        }
    }

    /// This driver only reads `multiscales[0]` — a second entry is simply
    /// never consulted, never an error (this module's own doc explains why).
    #[test]
    fn only_the_first_multiscale_entry_is_read() {
        let json = r#"{"multiscales":[
            {"axes":[],"datasets":[{"path":"a","coordinateTransformations":[{"type":"scale","scale":[1.0]}]}]},
            {"axes":[],"datasets":[{"path":"z","coordinateTransformations":[{"type":"scale","scale":[1.0]}]}]}
        ]}"#;
        let paths = parse_multiscales(json.as_bytes()).unwrap().unwrap();
        assert_eq!(paths, vec!["a".to_string()]);
    }

    // -- dataset path containment (a `.zattrs`-declared path must never
    // escape the store root once `ScopedStore` string-concatenates it onto a
    // document/chunk name -- see `validate_dataset_path`'s own doc for the
    // three distinct escapes these prove refused, and why each fixture
    // below is refused BY THE PARSE ITSELF, never by "the store then failed
    // to find the file": a fixture that merely didn't exist on disk would
    // pass for the wrong reason, so none of these tests touch a store at
    // all) -----------------------------------------------------------------

    /// Builds via `serde_json::json!` (never raw string formatting) so
    /// `path` is always correctly JSON-escaped regardless of what it
    /// contains — a raw `format!` would produce invalid JSON for the
    /// backslash-traversal fixture below (an unescaped `\` is not a legal
    /// JSON string character).
    fn multiscales_json_with_path(path: &str) -> String {
        serde_json::json!({
            "multiscales": [{
                "axes": [],
                "datasets": [{
                    "path": path,
                    "coordinateTransformations": [{"type": "scale", "scale": [1.0, 1.0]}]
                }]
            }]
        })
        .to_string()
    }

    #[test]
    fn refuses_an_absolute_unix_path() {
        let json = multiscales_json_with_path("/etc");
        match parse_multiscales(json.as_bytes()) {
            Err(ZarrError::Unsupported(msg)) => assert!(msg.contains("/etc"), "message was: {msg}"),
            other => panic!("expected a named refusal, got {other:?}"),
        }
    }

    #[test]
    fn refuses_a_relative_traversal_path() {
        let json = multiscales_json_with_path("../../etc");
        match parse_multiscales(json.as_bytes()) {
            Err(ZarrError::Unsupported(msg)) => {
                assert!(
                    msg.contains("..") && msg.contains("traversal"),
                    "message was: {msg}"
                );
            }
            other => panic!("expected a named refusal, got {other:?}"),
        }
    }

    #[test]
    fn refuses_a_traversal_path_that_starts_inside_the_store() {
        let json = multiscales_json_with_path("a/../../etc");
        match parse_multiscales(json.as_bytes()) {
            Err(ZarrError::Unsupported(msg)) => {
                assert!(msg.contains("traversal"), "message was: {msg}")
            }
            other => panic!("expected a named refusal, got {other:?}"),
        }
    }

    #[test]
    fn refuses_a_path_carrying_its_own_url_scheme() {
        let json = multiscales_json_with_path("http://example.invalid/x");
        match parse_multiscales(json.as_bytes()) {
            Err(ZarrError::Unsupported(msg)) => {
                assert!(msg.contains("scheme"), "message was: {msg}")
            }
            other => panic!("expected a named refusal, got {other:?}"),
        }
    }

    #[test]
    fn refuses_a_network_path_reference() {
        let json = multiscales_json_with_path("//example.invalid/x");
        match parse_multiscales(json.as_bytes()) {
            Err(ZarrError::Unsupported(msg)) => {
                assert!(msg.contains("network-path"), "message was: {msg}");
            }
            other => panic!("expected a named refusal, got {other:?}"),
        }
    }

    #[test]
    fn refuses_a_backslash_traversal_variant() {
        let json = multiscales_json_with_path("..\\..\\etc");
        match parse_multiscales(json.as_bytes()) {
            Err(ZarrError::Unsupported(msg)) => {
                assert!(msg.contains("traversal"), "message was: {msg}")
            }
            other => panic!("expected a named refusal, got {other:?}"),
        }
    }

    /// The validation isn't over-tight: an ordinary top-level dataset name
    /// and a genuinely nested one both still parse.
    #[test]
    fn accepts_ordinary_relative_dataset_paths() {
        assert_eq!(
            parse_multiscales(multiscales_json_with_path("0").as_bytes())
                .unwrap()
                .unwrap(),
            vec!["0".to_string()]
        );
        assert_eq!(
            parse_multiscales(multiscales_json_with_path("level/0").as_bytes())
                .unwrap()
                .unwrap(),
            vec!["level/0".to_string()]
        );
    }
}
