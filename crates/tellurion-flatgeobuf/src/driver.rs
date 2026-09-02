//! The `flatgeobuf` `DriverFactory`, and the `CatalogSource` + `FeatureSource`
//! implementation backing it. Read-only: a file is opened for reading only,
//! there is no write path, no DDL, nothing beyond what the driver contract's
//! mandatory `CatalogSource` plus the optional `FeatureSource` capability
//! require. `TileSource` is never implemented — a collection routed to a
//! `flatgeobuf` storage on the `tiles` lane fails at boot with the router's
//! ordinary missing-capability error, exactly like any other driver that
//! doesn't claim a capability (mirrors `tellurion-pmtiles`' own
//! `FeatureSource`-shaped omission, inverted).
//!
//! ## Storage config
//!
//! A `flatgeobuf` storage reuses `StorageDecl.url_env` exactly as `postgis`
//! and `pmtiles` do: the named environment variable holds the file's
//! location. Today that's always a local filesystem path (`PathBuf::from` on
//! the raw string), but the shape is deliberately future-compatible — the
//! same field could later hold an `http(s)://` URL and dispatch to a
//! different `flatgeobuf` backend (the crate's own `http` feature, disabled
//! here — see `Cargo.toml`) without any change to `StorageDecl` or config
//! shape. Implementing that dispatch is out of scope here.
//!
//! ## pk / cursor mapping (the central design decision)
//!
//! FlatGeobuf has no relational "primary key column" concept at all: a
//! feature is addressable only by its position in the file's on-disk order,
//! which — because every writer Hilbert-sorts features before laying them
//! out — is exactly the same order the packed R-tree's leaf level indexes
//! (`packed_r_tree::SearchResultItem::index`). This driver uses that
//! position, uniformly, as both the GeoJSON `id` and the keyset paging
//! cursor:
//!
//! - An unfiltered `items()` call streams the file with `FgbReader::
//!   select_all()`; the natural 0-based count of `.next()` calls *is* that
//!   position, no R-tree involved.
//! - A bbox-filtered call searches the **in-memory** packed R-tree cached in
//!   [`CachedHeader::rtree`] (built once, lazily, alongside the header — see
//!   [`FlatgeobufBackend::header`]) to get the matched `SearchResultItem`
//!   list, which is guaranteed ascending by `offset` (hence by `index`, since
//!   offset strictly increases with on-disk position) — see the crate's own
//!   `debug_assert` on this in `select_bbox`/`select_bbox_seq`. That list is
//!   windowed by `token`/`limit` in memory (no I/O), and only then does a
//!   *second*, fresh file open decode the actual matched features via
//!   `FgbReader::select_bbox()` — the crate's own R-tree-accelerated reader,
//!   which seeks past non-matching byte ranges instead of a full scan.
//!
//! This second open re-parses the on-disk index a driver-internal request
//! already has in memory — a deliberate, documented inefficiency: no public
//! `flatgeobuf` API exposes "hand back the real `&FgbFeature` for this
//! `SearchResultItem` without a fresh, safe `FgbReader::open()`" (the type's
//! constructor is crate-private), and reimplementing that decode path here
//! would duplicate real logic (column-type decoding, FlatBuffers
//! verification) for a local, typically-small file where the extra open is
//! cheap. The alternative — giving up the R-tree acceleration and just
//! `select_all()`-scanning with an in-Rust bbox filter — was rejected: it
//! throws away exactly the performance property FlatGeobuf was chosen for
//! (`#20`).
//!
//! **Upstream quirk this design routes around:** `FgbReader::select_bbox()`
//! (v6.0.1) searches its index using a *hardcoded* `PackedRTree::
//! DEFAULT_NODE_SIZE`, not the file's own `header.index_node_size()` (see
//! `flatgeobuf`'s `file_reader.rs`). For the overwhelmingly common case —
//! every writer, including this crate's own `examples/gen_flatgeobuf_fixture.rs`,
//! defaults `index_node_size` to that same constant — the two agree. To keep
//! this driver's in-memory `SearchResultItem` list (built via
//! `PackedRTree::from_buf`) in exact lock-step with what `select_bbox()`
//! itself will traverse, [`read_rtree`] also always passes
//! `PackedRTree::DEFAULT_NODE_SIZE`, and [`read_cached_header`] refuses to
//! build a tree at all (falling back to `rtree: None`, so a bbox query comes
//! back honestly empty rather than reading a mismatched node layout) when
//! the header declares any other `index_node_size`.
//!
//! ## geometry / pk physical identity
//!
//! `require_feature_capable` (`tellurion-core`'s descriptor enforcement)
//! demands a `FeatureSource`-capable driver report *some* stable
//! `geometry_column`/`primary_key` so a collection can pass boot validation
//! — see `descriptor.rs`. Neither concept is a real, named thing in a
//! FlatGeobuf file (geometry is a single embedded, structural field per
//! feature; the pk is the positional index above, not a schema column), so
//! this driver reports conventional, synthetic names
//! ([`GEOMETRY_FIELD`]/[`PRIMARY_KEY_FIELD`]) that satisfy the contract but
//! are never consulted by this driver's own query logic — there is nothing
//! to look up "by name" the way `tellurion-postgis`'s `sql.rs` looks up a
//! configured column identifier. A FlatGeobuf column schema *can* mark one
//! of its own columns `primary_key: true`; v0.1 does not special-case that
//! (every collection uses the same feature-index identity regardless), to
//! keep cursor semantics uniform across every `.fgb` file this driver might
//! ever open.
//!
//! ## Datetime filtering
//!
//! Not implemented: a FlatGeobuf column schema has no notion of "the"
//! datetime column the way `CollectionDecl.datetime` names one for
//! `postgis`, and this driver never inspects column types to guess one. A
//! `datetime` query filter is refused with `Error::Invalid` — the same
//! honest-rejection shape `postgis`'s `NoDatetimeColumn` takes when a
//! collection has no datetime column configured — rather than silently
//! ignoring the filter (which would look like an empty-but-wrong result) or
//! attempting a best-effort match.
//!
//! ## CQL2 attribute filtering (`#33`)
//!
//! Also not implemented, deliberately, for this lane: `FeatureSource::
//! filter_capable` stays at its default (`false`), so a `filter` query
//! parameter against a FlatGeobuf-backed collection is refused at the
//! protocol layer before this driver's `items` is ever called with one — see
//! `tellurion-features`' handler. `items_inner` below still checks
//! `query.filter` defensively for any caller that reaches this backend
//! directly, the same belt-and-suspenders shape as the `datetime` check
//! above.
//!
//! ## CRS assumption
//!
//! The header's `envelope` is reported as `SpatialExtent` (CRS84) without
//! reprojection — this driver assumes the file's native CRS already *is*
//! CRS84/WGS84, same simplifying assumption `tellurion-pmtiles` makes for
//! PMTiles' spec-guaranteed CRS84 header bounds, except FlatGeobuf's spec
//! does *not* guarantee that (a file can declare any CRS via its own
//! `header.crs()`). A future iteration could reproject when `crs()` names a
//! non-4326 EPSG code; v0.1 does not.

use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use flatgeobuf::{FallibleStreamingIterator, FgbReader, GeometryType};
use tokio::sync::OnceCell;

use tellurion_core::{
    CatalogSource, CollectionDecl, DriverFactory, Error as CoreError, FeaturePage, FeatureSource,
    ItemsQuery, PhysicalCollection, Result as CoreResult, SpatialExtent, StorageDecl,
    StorageDriver,
};

use crate::error::{FlatgeobufDriverError, Result};

/// Structural, conventional geometry field name — see this module's
/// "geometry / pk physical identity" docs. Never consulted by this driver's
/// own query logic.
const GEOMETRY_FIELD: &str = "geometry";

/// Synthetic feature-index primary key name — see this module's "pk / cursor
/// mapping" and "geometry / pk physical identity" docs. Matches the "fid"
/// convention OGR/GDAL readers commonly synthesize for formats with no
/// native key.
const PRIMARY_KEY_FIELD: &str = "fid";

/// Registers the `flatgeobuf` driver.
#[derive(Default)]
pub struct FlatgeobufDriverFactory;

impl FlatgeobufDriverFactory {
    pub fn new() -> Self {
        Self
    }
}

impl DriverFactory for FlatgeobufDriverFactory {
    fn name(&self) -> &str {
        "flatgeobuf"
    }

    fn build(&self, decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        let raw = std::env::var(&decl.url_env).map_err(|_| {
            CoreError::Config(format!(
                "storage '{}': environment variable '{}' is not set",
                decl.id, decl.url_env
            ))
        })?;
        Ok(Arc::new(FlatgeobufDriverImpl {
            backend: Arc::new(FlatgeobufBackend::new(PathBuf::from(raw))),
        }))
    }
}

struct FlatgeobufDriverImpl {
    backend: Arc<FlatgeobufBackend>,
}

impl StorageDriver for FlatgeobufDriverImpl {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::clone(&self.backend) as Arc<dyn CatalogSource>
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn FeatureSource>)
    }

    // `tile_source`: default `None` — this driver never implements it.
    // `capacity_hint`: default `None` — a single local file has no pool-like
    // concurrency ceiling worth reporting, same rationale as pmtiles.
    // `validate_collection`: default accepts everything — there is no
    // operator-declared physical identifier syntax for this driver to check.
}

/// Header-derived metadata cached once per backend lifetime (`tokio::sync::
/// OnceCell`, matching `tellurion-pmtiles`' deferred-open pattern) — a fresh
/// `File` handle is still opened per query (see `driver.rs`'s module docs on
/// why), but the header itself, and the optional in-memory R-tree, are only
/// ever read from disk once.
struct CachedHeader {
    name: String,
    features_count: u64,
    geometry_type: Option<String>,
    envelope: Option<[f64; 4]>,
    srid: Option<i32>,
    /// `None` when the file has no spatial index at all (`index_node_size ==
    /// 0`, e.g. written with `write_index: false`) or declares a non-default
    /// `index_node_size` this driver cannot safely correlate against
    /// `FgbReader::select_bbox()`'s hardcoded branching factor — see the
    /// "upstream quirk" section of this module's docs. Either way, a
    /// bbox-filtered `items()` call against such a file comes back honestly
    /// empty rather than risking a mismatched read.
    rtree: Option<flatgeobuf::packed_r_tree::PackedRTree>,
}

struct FlatgeobufBackend {
    path: PathBuf,
    header: OnceCell<Arc<CachedHeader>>,
}

impl FlatgeobufBackend {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            header: OnceCell::new(),
        }
    }

    async fn header(&self) -> Result<Arc<CachedHeader>> {
        let cached = self
            .header
            .get_or_try_init(|| async {
                let path = self.path.clone();
                run_blocking(move || read_cached_header(&path).map(Arc::new)).await
            })
            .await?;
        Ok(Arc::clone(cached))
    }

    async fn catalog_inner(&self) -> Result<Vec<PhysicalCollection>> {
        let header = self.header().await?;
        Ok(vec![PhysicalCollection {
            name: header.name.clone(),
            geometry_column: Some(GEOMETRY_FIELD.to_string()),
            primary_key: Some(PRIMARY_KEY_FIELD.to_string()),
            srid: header.srid,
            geometry_type: header.geometry_type.clone(),
        }])
    }

    async fn extent_inner(&self) -> Result<Option<SpatialExtent>> {
        let header = self.header().await?;
        Ok(header.envelope.map(|bbox| SpatialExtent { bbox }))
    }

    async fn items_inner(&self, query: &ItemsQuery) -> Result<FeaturePage> {
        if query.datetime.is_some() {
            return Err(FlatgeobufDriverError::DatetimeUnsupported);
        }
        // `#33`: this driver never overrides `FeatureSource::filter_capable`
        // (stays at the trait default, `false`), so `tellurion-features`'
        // handler already refuses a `filter` request before `items` is ever
        // called. This check is defense in depth for any caller that reaches
        // this backend directly (this file's own tests do exactly that) —
        // same belt-and-suspenders shape as the `datetime` check above.
        if query.filter.is_some() {
            return Err(FlatgeobufDriverError::FilterUnsupported);
        }
        let token = parse_token(query.token.as_deref())?;
        let header = self.header().await?;
        let path = self.path.clone();
        let bbox = query.bbox;
        let limit = query.limit;

        run_blocking(move || match bbox {
            Some(bbox) => read_items_bbox(&path, header.rtree.as_ref(), bbox, token, limit),
            None => read_items_all(&path, header.features_count, token, limit),
        })
        .await
    }

    async fn item_inner(&self, id: &str) -> Result<Option<serde_json::Value>> {
        let Ok(target) = id.parse::<u64>() else {
            // A non-integer id can never match this driver's feature-index
            // identity — same "honest None" convention postgis's integer-pk
            // `item_inner` applies to a non-integer id.
            return Ok(None);
        };
        let header = self.header().await?;
        if target >= header.features_count {
            return Ok(None);
        }
        let path = self.path.clone();
        run_blocking(move || read_item_by_index(&path, target)).await
    }
}

#[async_trait]
impl CatalogSource for FlatgeobufBackend {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        self.catalog_inner().await.map_err(Into::into)
    }

    async fn extent(&self, _physical: &PhysicalCollection) -> CoreResult<Option<SpatialExtent>> {
        self.extent_inner().await.map_err(Into::into)
    }
}

#[async_trait]
impl FeatureSource for FlatgeobufBackend {
    async fn items(
        &self,
        _collection: &CollectionDecl,
        query: &ItemsQuery,
    ) -> CoreResult<FeaturePage> {
        self.items_inner(query).await.map_err(Into::into)
    }

    // `filter` is always `None` here in practice: this driver never
    // overrides `filter_capable` (stays at the trait default, `false`), so
    // `#34`'s policy checkpoint never hands it a grant filter to begin with
    // — attribute filtering is out of scope for this lane (`#33`'s own
    // module doc). The parameter still has to exist to satisfy
    // `FeatureSource`.
    async fn item(
        &self,
        _collection: &CollectionDecl,
        id: &str,
        _filter: Option<&tellurion_core::Filter>,
    ) -> CoreResult<Option<serde_json::Value>> {
        self.item_inner(id).await.map_err(Into::into)
    }
}

/// Runs `f` on the blocking thread pool and flattens the `JoinError` layer —
/// every actual file read in this driver is synchronous (`flatgeobuf`
/// offers no local-file async reader without the `http` feature this crate
/// disables), so nothing here may run directly on the async runtime.
async fn run_blocking<T, F>(f: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(result) => result,
        Err(join_err) => Err(FlatgeobufDriverError::from(join_err)),
    }
}

fn parse_token(token: Option<&str>) -> Result<Option<u64>> {
    match token {
        None => Ok(None),
        Some(raw) => raw
            .parse::<u64>()
            .map(Some)
            .map_err(|_| FlatgeobufDriverError::InvalidToken(raw.to_string())),
    }
}

/// The collection name this file reports: the header's own `name` (the
/// dataset name every `FgbWriter::create` caller supplies — see
/// `examples/gen_flatgeobuf_fixture.rs`) when present and non-empty, else the file
/// stem. Matched against `CollectionDecl::table`/`id` by `Router::
/// validate_catalog`, exactly as postgis's reported table name and
/// pmtiles' reported archive name are.
fn header_name(name: Option<&str>, path: &Path) -> String {
    name.filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("dataset")
                .to_string()
        })
}

fn geometry_type_name(geometry_type: GeometryType) -> Option<String> {
    let name = match geometry_type {
        GeometryType::Point => "POINT",
        GeometryType::LineString => "LINESTRING",
        GeometryType::Polygon => "POLYGON",
        GeometryType::MultiPoint => "MULTIPOINT",
        GeometryType::MultiLineString => "MULTILINESTRING",
        GeometryType::MultiPolygon => "MULTIPOLYGON",
        GeometryType::GeometryCollection => "GEOMETRYCOLLECTION",
        // `Unknown` (a dataset mixing geometry types) and the SQL-MM curve
        // types have no single conventional uppercase name this workspace
        // otherwise reports; `None` here matches `PhysicalCollection::
        // geometry_type`'s own "backend cannot answer" contract.
        _ => return None,
    };
    Some(name.to_string())
}

fn read_cached_header(path: &Path) -> Result<CachedHeader> {
    let file = File::open(path).map_err(flatgeobuf::Error::from)?;
    let reader = FgbReader::open(BufReader::new(file))?;
    let header = reader.header();

    let name = header_name(header.name(), path);
    let features_count = header.features_count();
    let index_node_size = header.index_node_size();
    let geometry_type = geometry_type_name(header.geometry_type());
    let envelope = header.envelope().and_then(|vector| {
        let values: Vec<f64> = vector.iter().collect();
        <[f64; 4]>::try_from(values).ok()
    });
    let srid = header.crs().map(|crs| crs.code()).filter(|&code| code != 0);

    // See this module's "upstream quirk" docs: only build (and later use) an
    // in-memory R-tree when the header's branching factor matches what
    // `FgbReader::select_bbox()` itself hardcodes.
    let rtree = if features_count > 0
        && index_node_size == flatgeobuf::packed_r_tree::PackedRTree::DEFAULT_NODE_SIZE
    {
        Some(read_rtree(path, features_count as usize)?)
    } else {
        None
    };

    Ok(CachedHeader {
        name,
        features_count,
        geometry_type,
        envelope,
        srid,
        rtree,
    })
}

/// Loads the packed R-tree index into memory from a *fresh* file handle,
/// positioned at the start of the index section by [`skip_to_index`]. Used
/// only at header-cache warm-up time (once per backend lifetime), not per
/// query — see [`CachedHeader::rtree`].
fn read_rtree(
    path: &Path,
    features_count: usize,
) -> Result<flatgeobuf::packed_r_tree::PackedRTree> {
    let mut reader = BufReader::new(File::open(path).map_err(flatgeobuf::Error::from)?);
    skip_to_index(&mut reader)?;
    flatgeobuf::packed_r_tree::PackedRTree::from_buf(
        &mut reader,
        features_count,
        flatgeobuf::packed_r_tree::PackedRTree::DEFAULT_NODE_SIZE,
    )
    .map_err(Into::into)
}

/// Positions `reader` at the start of the packed R-tree index section: right
/// after the 8-byte magic prefix and the size-prefixed FlatBuffers header.
/// No public `flatgeobuf` API exposes "the reader `FgbReader::open` already
/// validated, still positioned right after the header" for reuse elsewhere
/// (its only two continuations, `select_all`/`select_bbox`, both consume
/// `self` and leave the index behind or already parsed) — so this
/// reimplements only that outer framing, never any FlatBuffers parsing.
/// Safe because [`read_cached_header`] already opened and FlatBuffers-
/// verified this same file moments earlier in the same warm-up call: a
/// format change that broke this framing would also break that earlier
/// `FgbReader::open` call, so there is no silent-corruption path unique to
/// this function.
fn skip_to_index<R: Read>(reader: &mut R) -> Result<()> {
    let mut magic = [0u8; 8];
    reader
        .read_exact(&mut magic)
        .map_err(flatgeobuf::Error::from)?;
    if &magic[0..3] != b"fgb" {
        return Err(flatgeobuf::Error::MissingMagicBytes.into());
    }
    let mut size_buf = [0u8; 4];
    reader
        .read_exact(&mut size_buf)
        .map_err(flatgeobuf::Error::from)?;
    let header_size = u64::from(u32::from_le_bytes(size_buf));
    io::copy(&mut reader.take(header_size), &mut io::sink()).map_err(flatgeobuf::Error::from)?;
    Ok(())
}

/// Turns one decoded feature into a full GeoJSON `Feature` object with `id`
/// set to `pk` (as a string, matching postgis's own `pk::text` convention).
/// `FgbFeature::process` (via `geozero::FeatureAccess`'s default impl) walks
/// properties then geometry through a `GeoJsonWriter`, producing correctly
/// typed properties (numbers as numbers, not postgis's own stringly
/// `to_jsonb` cast, since there's no relational cast happening here at all)
/// — note this does *not* go through `flatgeobuf`'s `ToJson` trait, whose
/// blanket impl over `GeozeroGeometry` only serializes the geometry, not a
/// full feature (see this crate's own doc examples for that distinction).
fn feature_to_geojson(feature: &flatgeobuf::FgbFeature, pk: u64) -> Result<serde_json::Value> {
    use flatgeobuf::FeatureAccess;

    let mut buf: Vec<u8> = Vec::new();
    {
        let mut writer = geozero::geojson::GeoJsonWriter::new(&mut buf);
        feature.process(&mut writer, 0)?;
    }

    let mut value: serde_json::Value = serde_json::from_slice(&buf)?;
    if let serde_json::Value::Object(map) = &mut value {
        map.insert("id".to_string(), serde_json::Value::String(pk.to_string()));
    }
    Ok(value)
}

/// Unfiltered listing: streams the file sequentially with `select_all()`,
/// no R-tree involved. The running `.next()` count *is* the global pk (see
/// this module's "pk / cursor mapping" docs); `token` skips a prefix,
/// `limit` bounds the page, and one extra iteration beyond `limit` detects
/// whether a next page exists without a second pass.
fn read_items_all(
    path: &Path,
    features_count: u64,
    token: Option<u64>,
    limit: u32,
) -> Result<FeaturePage> {
    let file = File::open(path).map_err(flatgeobuf::Error::from)?;
    let mut iter = FgbReader::open(BufReader::new(file))?.select_all()?;

    let want = limit as usize;
    let mut features = Vec::new();
    let mut last_idx: Option<u64> = None;
    let mut has_more = false;
    let mut idx: u64 = 0;

    while let Some(feature) = iter.next()? {
        let this_idx = idx;
        idx += 1;
        if token.is_some_and(|t| this_idx <= t) {
            continue;
        }
        if features.len() >= want {
            has_more = true;
            break;
        }
        features.push(feature_to_geojson(feature, this_idx)?);
        last_idx = Some(this_idx);
    }

    Ok(FeaturePage {
        features_geojson: features,
        // Exact, not an estimate: `features_count` is a header field, free
        // to report regardless of paging state.
        number_matched: Some(features_count),
        next_token: has_more.then(|| last_idx.map(|v| v.to_string())).flatten(),
    })
}

/// Bbox-filtered listing — see this module's "pk / cursor mapping" docs for
/// the two-pass in-memory-search-then-decode strategy this implements.
fn read_items_bbox(
    path: &Path,
    rtree: Option<&flatgeobuf::packed_r_tree::PackedRTree>,
    bbox: [f64; 4],
    token: Option<u64>,
    limit: u32,
) -> Result<FeaturePage> {
    let Some(rtree) = rtree else {
        // No usable index for this file (see `CachedHeader::rtree`'s docs)
        // — an honestly empty result, not an error: the same "this driver
        // cannot answer, but the request itself is valid" shape a
        // never-addressed pmtiles tile coordinate takes.
        return Ok(FeaturePage {
            features_geojson: Vec::new(),
            number_matched: Some(0),
            next_token: None,
        });
    };

    let [minx, miny, maxx, maxy] = bbox;
    let matches = rtree.search(minx, miny, maxx, maxy)?;
    let number_matched = matches.len() as u64;

    let want = limit as usize;
    let start = match token {
        Some(t) => matches.partition_point(|item| (item.index as u64) <= t),
        None => 0,
    };
    let end = (start + want).min(matches.len());
    let has_more = end < matches.len();

    if start >= end {
        return Ok(FeaturePage {
            features_geojson: Vec::new(),
            number_matched: Some(number_matched),
            next_token: None,
        });
    }

    let file = File::open(path).map_err(flatgeobuf::Error::from)?;
    let mut iter = FgbReader::open(BufReader::new(file))?.select_bbox(minx, miny, maxx, maxy)?;

    let mut features = Vec::with_capacity(end - start);
    let mut pos = 0usize;
    while pos < end {
        let Some(feature) = iter.next()? else {
            break;
        };
        if pos >= start {
            let global_index = matches[pos].index as u64;
            features.push(feature_to_geojson(feature, global_index)?);
        }
        pos += 1;
    }

    let last_idx = matches[end - 1].index as u64;
    Ok(FeaturePage {
        features_geojson: features,
        number_matched: Some(number_matched),
        next_token: has_more.then(|| last_idx.to_string()),
    })
}

fn read_item_by_index(path: &Path, target: u64) -> Result<Option<serde_json::Value>> {
    let file = File::open(path).map_err(flatgeobuf::Error::from)?;
    let mut iter = FgbReader::open(BufReader::new(file))?.select_all()?;

    let mut idx: u64 = 0;
    while let Some(feature) = iter.next()? {
        if idx == target {
            return Ok(Some(feature_to_geojson(feature, idx)?));
        }
        idx += 1;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny.fgb")
    }

    fn backend() -> FlatgeobufBackend {
        FlatgeobufBackend::new(fixture_path())
    }

    fn decl() -> CollectionDecl {
        serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap()
    }

    #[test]
    fn factory_name_matches_the_config_driver_key() {
        assert_eq!(FlatgeobufDriverFactory::new().name(), "flatgeobuf");
    }

    #[test]
    fn build_fails_fast_when_the_env_var_is_unset() {
        let factory = FlatgeobufDriverFactory::new();
        let decl = StorageDecl {
            id: "main".to_string(),
            driver: "flatgeobuf".to_string(),
            url_env: "TELLURION_FLATGEOBUF_TEST_DOES_NOT_EXIST".to_string(),
            pool_size: None,
        };
        std::env::remove_var(&decl.url_env);
        assert!(matches!(factory.build(&decl), Err(CoreError::Config(_))));
    }

    #[tokio::test]
    async fn collections_reports_the_header_derived_identity() {
        let backend = backend();
        let collections = backend.collections().await.unwrap();
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].name, "demo");
        assert_eq!(collections[0].geometry_column.as_deref(), Some("geometry"));
        assert_eq!(collections[0].primary_key.as_deref(), Some("fid"));
        assert_eq!(collections[0].geometry_type.as_deref(), Some("POINT"));
        assert_eq!(collections[0].srid, Some(4326));
    }

    #[tokio::test]
    async fn extent_comes_from_the_header_envelope() {
        let backend = backend();
        let physical = &backend.collections().await.unwrap()[0];
        let extent = backend.extent(physical).await.unwrap().unwrap();
        assert_eq!(extent.bbox, [-4.0, 46.0, 4.0, 54.0]);
    }

    #[tokio::test]
    async fn items_without_a_filter_returns_every_feature_with_an_exact_count() {
        let backend = backend();
        let page = backend
            .items(&decl(), &ItemsQuery::default())
            .await
            .unwrap();
        assert_eq!(page.features_geojson.len(), 5);
        assert_eq!(page.number_matched, Some(5));
        assert_eq!(page.next_token, None);
    }

    #[tokio::test]
    async fn items_pages_across_at_least_two_pages_with_stable_ids() {
        let backend = backend();
        let query = ItemsQuery {
            limit: 2,
            ..ItemsQuery::default()
        };
        let page1 = backend.items(&decl(), &query).await.unwrap();
        assert_eq!(page1.features_geojson.len(), 2);
        assert_eq!(page1.number_matched, Some(5));
        let token1 = page1
            .next_token
            .clone()
            .expect("first page has a next token");

        let query2 = ItemsQuery {
            limit: 2,
            token: Some(token1),
            ..ItemsQuery::default()
        };
        let page2 = backend.items(&decl(), &query2).await.unwrap();
        assert_eq!(page2.features_geojson.len(), 2);
        let token2 = page2
            .next_token
            .clone()
            .expect("second page has a next token");

        let query3 = ItemsQuery {
            limit: 2,
            token: Some(token2),
            ..ItemsQuery::default()
        };
        let page3 = backend.items(&decl(), &query3).await.unwrap();
        assert_eq!(page3.features_geojson.len(), 1);
        assert_eq!(page3.next_token, None);

        // No id repeats across the three pages.
        let mut ids: Vec<String> = [&page1, &page2, &page3]
            .iter()
            .flat_map(|p| p.features_geojson.iter())
            .map(|f| f["id"].as_str().unwrap().to_string())
            .collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 5);
    }

    #[tokio::test]
    async fn items_with_a_bbox_uses_the_rtree_and_returns_only_matching_features() {
        let backend = backend();
        // Covers only the western half of the fixture's extent.
        let query = ItemsQuery {
            bbox: Some([-5.0, 45.0, -1.0, 55.0]),
            ..ItemsQuery::default()
        };
        let page = backend.items(&decl(), &query).await.unwrap();
        assert!(!page.features_geojson.is_empty());
        assert!(page.features_geojson.len() < 5);
        for feature in &page.features_geojson {
            let x = feature["geometry"]["coordinates"][0].as_f64().unwrap();
            assert!(x <= -1.0, "feature outside the requested bbox: {feature}");
        }
    }

    #[tokio::test]
    async fn item_looks_up_a_feature_by_its_feature_index_id() {
        let backend = backend();
        let listing = backend
            .items(&decl(), &ItemsQuery::default())
            .await
            .unwrap();
        let first_id = listing.features_geojson[0]["id"].as_str().unwrap();

        let fetched = backend
            .item(&decl(), first_id, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched["id"], listing.features_geojson[0]["id"]);
        assert_eq!(fetched["geometry"], listing.features_geojson[0]["geometry"]);
    }

    #[tokio::test]
    async fn item_returns_none_for_a_non_integer_id() {
        let backend = backend();
        assert_eq!(
            backend.item(&decl(), "not-a-number", None).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn item_returns_none_for_an_out_of_range_index() {
        let backend = backend();
        assert_eq!(backend.item(&decl(), "999", None).await.unwrap(), None);
    }

    #[tokio::test]
    async fn datetime_filter_is_refused_honestly() {
        let backend = backend();
        let query = ItemsQuery {
            datetime: Some(tellurion_core::DatetimeRange {
                start: Some("2020-01-01T00:00:00Z".to_string()),
                end: None,
            }),
            ..ItemsQuery::default()
        };
        assert!(matches!(
            backend.items(&decl(), &query).await,
            Err(CoreError::Invalid(_))
        ));
    }

    /// `#33`: this driver's `FeatureSource::filter_capable` stays at the
    /// trait default (`false`); this exercises the defensive check in
    /// `items_inner` directly (`tellurion-features`' handler is the layer
    /// that ordinarily refuses the request before `items` is ever called —
    /// covered separately in that crate's own tests).
    #[tokio::test]
    async fn filter_is_refused_honestly() {
        let backend = backend();
        assert!(!FeatureSource::filter_capable(&backend));
        let query = ItemsQuery {
            filter: Some(tellurion_core::Filter::IsNull {
                property: "name".to_string(),
                negated: false,
            }),
            ..ItemsQuery::default()
        };
        assert!(matches!(
            backend.items(&decl(), &query).await,
            Err(CoreError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn invalid_token_is_rejected() {
        let backend = backend();
        let query = ItemsQuery {
            token: Some("not-a-number".to_string()),
            ..ItemsQuery::default()
        };
        assert!(matches!(
            backend.items(&decl(), &query).await,
            Err(CoreError::Invalid(_))
        ));
    }

    /// `#105`: this driver never overrides `FeatureSource::
    /// cql2_conformance_classes` either (stays at the trait default, empty)
    /// — CQL2 filtering is out of scope for this lane, `#33`, and empty
    /// necessarily excludes `case-insensitive-comparison` too.
    #[tokio::test]
    async fn cql2_conformance_classes_stays_empty() {
        let backend = backend();
        let declared = FeatureSource::cql2_conformance_classes(&backend);
        assert!(declared.is_empty());
        assert_eq!(
            FeatureSource::filter_capable(&backend),
            !declared.is_empty()
        );
    }
}
