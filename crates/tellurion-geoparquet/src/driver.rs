//! The `geoparquet` `DriverFactory`, and the `CatalogSource` + `FeatureSource`
//! implementation backing it. Read-only: a file is opened for reading only,
//! there is no write path (the fixture generator in `examples/gen_geoparquet_fixture.rs`
//! is dev tooling, never linked into the driver itself), no DDL, nothing
//! beyond what the driver contract's mandatory `CatalogSource` plus the
//! optional `FeatureSource` capability require. `TileSource` is available
//! only for collections resolved to CRS84/EPSG:4326; projected or unknown
//! source CRSs remain Features-only because this driver does not reproject.
//!
//! ## Dependency choice
//!
//! Two shapes were on the table: a geoarrow-ecosystem crate (`geoparquet` on
//! crates.io, part of the `geoarrow-rs` project) versus hand-rolling on the
//! official Apache `parquet`/`arrow-*` crates plus `geozero` for WKB. The
//! geoarrow `geoparquet` crate was rejected — its own dependency manifest
//! pulls `arrow-arith`, `arrow-ord`, `geoarrow-array`, `geoarrow-schema`,
//! `geo`, `geo-traits`, `wkt`, `indexmap`, and `serde_with` on top of the
//! same `parquet`/`arrow-array`/`arrow-schema` this driver needs anyway,
//! across a still-fragmented, frequently-breaking 0.x multi-crate API
//! surface. Hand-rolling on `parquet` (official Apache Arrow Rust project,
//! the same "smallest maintained option" bias `tellurion-flatgeobuf` already
//! applies to its own `flatgeobuf`/`geozero` pins) plus the two granular
//! `arrow-array`/`arrow-schema` crates it already resolves transitively, and
//! `geozero` pinned to the exact version `tellurion-flatgeobuf` already
//! carries, adds no dependency this workspace doesn't already trust and
//! keeps the new transitive footprint to what's structurally unavoidable for
//! reading Parquet's Thrift-encoded footer and Arrow's columnar batches. See
//! `Cargo.toml` for the exact feature selection (`parquet`'s `"arrow"` +
//! `"async"` + `"snap"` only — no zstd/brotli/gzip codecs or object-store
//! SDK). The configured driver keeps its local path convention; callers with
//! an authorized range object use [`GeoparquetBackend::from_input`].
//!
//! ## Storage config
//!
//! A `geoparquet` storage reuses `StorageDecl.url_env` exactly as
//! `flatgeobuf`/`postgis`/`pmtiles` do: the named environment variable holds
//! the file's local filesystem path.
//!
//! ## The "geo" metadata contract
//!
//! GeoParquet is a convention layered on plain Parquet: a JSON document
//! under the file-level key-value metadata key `"geo"` names the primary
//! geometry column, its WKB encoding, geometry type(s), bbox, and CRS — see
//! `geo_metadata.rs`. A file with no `"geo"` entry is not a valid GeoParquet
//! file and this driver refuses it outright
//! (`GeoparquetDriverError::MissingGeoMetadata`) rather than guessing a
//! geometry column by name or type; there is no scan-and-detect fallback,
//! matching this contract's general "backend reports what it actually
//! knows" ethos rather than a best-effort heuristic.
//!
//! ## pk / cursor mapping
//!
//! Like `flatgeobuf`, GeoParquet has no relational primary key column: this
//! driver uses a row's **global position in the file** (row-group order,
//! then in-group order — the same order a full unfiltered scan visits rows
//! in) as both the GeoJSON `id` and the keyset paging cursor, uniformly for
//! filtered and unfiltered queries alike. Filtering (a bbox query, or `id`
//! lookup) never renumbers: a row's id is fixed by its physical position
//! regardless of which rows a particular query happened to match.
//!
//! ## Row-group pruning via GeoParquet 1.1 `covering`
//!
//! When the primary column's metadata carries a `covering.bbox` block (four
//! dotted paths to a per-row bbox struct column's `xmin`/`ymin`/`xmax`/
//! `ymax` float children — see `geo_metadata.rs`), this driver resolves
//! those paths to physical Parquet leaf-column indices once at header
//! warm-up time and reads each row group's own min/max column *statistics*
//! on them (`RowGroupMetaData::column(..).statistics()`) — free, already
//! sitting in the footer, no row data touched. A row group whose stats
//! bbox cannot possibly intersect the query bbox is skipped without
//! decoding a single row (see [`covering_bbox_might_intersect`]). Surviving
//! row groups still decode every row, but even there the covering struct
//! column (already loaded as part of the batch) gives a per-row bbox
//! straight from Arrow arrays — no WKB parse — for the row-level test (see
//! [`row_bbox_from_batch`]). A file with no covering metadata at all falls
//! back to decoding each row's actual WKB geometry and folding its
//! coordinates into a bbox via [`BboxCollector`] — correct, just without the
//! free statistics shortcut.
//!
//! ## Counting
//!
//! An unfiltered `items()` call reports `number_matched` straight from the
//! file metadata's row count — free, and exact regardless of how the
//! request happens to page through it (mirrors flatgeobuf's header
//! `features_count`). A bbox-filtered call has no such shortcut (no spatial
//! index gives a cheap global match count) — [`read_items_bbox`] scans every
//! surviving row group in full to produce an exact `number_matched`, paying
//! the scan cost the same way a full table scan would; the driver contract
//! explicitly allows this ("count while scanning is acceptable") rather than
//! reporting an estimate.
//!
//! ## Datetime filtering
//!
//! Not implemented, for the same reason `flatgeobuf` refuses it: nothing in
//! GeoParquet's own metadata or this driver marks "the" datetime column the
//! way `CollectionDecl.datetime` does for `postgis`. A `datetime` query
//! filter is refused with `Error::Invalid` rather than silently ignored.
//! `CatalogSource::temporal_column` (introspection, not filtering) is a
//! separate, narrower question — see its own doc comment below.
//!
//! ## CRS assumption
//!
//! GeoParquet's `crs` field, absent or JSON `null`, means OGC:CRS84 per
//! spec — this driver reports that as `srid: Some(4326)`. A present `crs` is
//! a full PROJJSON document; deriving an EPSG code from an arbitrary
//! datum+projection tree is out of scope for v0.1 — only the common
//! straightforward case (a top-level `id: {authority, code}` member, what
//! GDAL/DuckDB/GeoPandas emit for a plain EPSG CRS) is recognized, see
//! [`srid_from_crs`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Float32Array, Float64Array, Int16Array, Int32Array,
    Int64Array, Int8Array, LargeStringArray, RecordBatch, StringArray, StructArray, UInt16Array,
    UInt32Array, UInt64Array, UInt8Array,
};
use arrow_schema::{DataType, Schema};
use async_trait::async_trait;
use futures::TryStreamExt;
use parquet::arrow::arrow_reader::{ArrowReaderMetadata, ArrowReaderOptions};
use parquet::arrow::async_reader::{
    AsyncFileReader, ParquetRecordBatchStream, ParquetRecordBatchStreamBuilder,
};
use parquet::arrow::ProjectionMask;
use parquet::errors::ParquetError;
use parquet::file::metadata::RowGroupMetaData;
use parquet::file::statistics::Statistics;
use parquet::schema::types::SchemaDescriptor;
use tokio::sync::OnceCell;

use tellurion_core::{
    heuristics, AttributeColumn, CatalogSource, CollectionDecl, DriverFactory, Error as CoreError,
    FeaturePage, FeatureSource, ItemsQuery, PhysicalCollection, Result as CoreResult,
    SpatialExtent, StorageDecl, StorageDriver, TileCoord, TileSource, DEFAULT_TILE_VERTEX_BUDGET,
};
use tellurion_vector_tile::{
    encode_tile, tile_envelope_3857, SourceCrs, TileFeature, TileRequest, TileScalar,
};

use crate::error::{GeoparquetDriverError, Result};
use crate::geo_metadata::{parse_geo_metadata, CoveringPaths, GEO_METADATA_KEY};
use crate::GeoparquetInput;

#[cfg(feature = "remote")]
use tellurion_http_source::{SourceError, SourceErrorKind};

/// Synthetic feature-index primary key name — see this module's "pk / cursor
/// mapping" docs. Matches `flatgeobuf`'s own `fid` convention (itself the
/// OGR/GDAL synthesized-key name) for the same "no native key" situation.
const PRIMARY_KEY_FIELD: &str = "fid";

/// MVT's conventional tile-local coordinate extent, shared with the other
/// vector drivers in this workspace.
const MVT_EXTENT: u32 = 4096;

/// Keeps Arrow batches bounded while a tile searches bbox candidates in a
/// large row group. Parquet column chunks remain range-read by the reviewed
/// async input adapter; this limits only decoded in-memory rows per batch.
const TILE_SCAN_BATCH_SIZE: usize = 1024;

/// A tile may decode at most this many rows across all candidate row groups.
/// The bound applies before opening a row group, so a broad covering index
/// cannot turn a valid empty-tile request into a full-file remote scan.
const TILE_SCAN_ROW_BUDGET: u64 = 2_048;

/// Registers the `geoparquet` driver.
#[derive(Default)]
pub struct GeoparquetDriverFactory;

impl GeoparquetDriverFactory {
    pub fn new() -> Self {
        Self
    }
}

impl DriverFactory for GeoparquetDriverFactory {
    fn name(&self) -> &str {
        "geoparquet"
    }

    fn build(&self, decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        let raw = std::env::var(&decl.url_env).map_err(|_| {
            CoreError::Config(format!(
                "storage '{}': environment variable '{}' is not set",
                decl.id, decl.url_env
            ))
        })?;
        Ok(Arc::new(GeoparquetDriverImpl {
            backend: Arc::new(GeoparquetBackend::new(PathBuf::from(raw))),
        }))
    }
}

struct GeoparquetDriverImpl {
    backend: Arc<GeoparquetBackend>,
}

impl StorageDriver for GeoparquetDriverImpl {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::clone(&self.backend) as Arc<dyn CatalogSource>
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn FeatureSource>)
    }

    fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn TileSource>)
    }

    // `capacity_hint`: default `None` — a single local file has no pool-like
    // concurrency ceiling worth reporting, same rationale as flatgeobuf.
    // `validate_collection`: default accepts everything.
}

/// Resolved leaf-column identity for a GeoParquet 1.1 `covering.bbox` block
/// — both the physical Parquet leaf-column indices row-group statistics are
/// read from ([`row_group_covering_bbox`]) and the Arrow struct/field names
/// a decoded batch's own bbox column is read from ([`row_bbox_from_batch`]).
struct CoveringColumns {
    /// Top-level struct column name (e.g. `"bbox"`) — excluded from
    /// `attribute_schema` and from a feature's `properties`, same as the
    /// geometry column itself.
    struct_field: String,
    xmin_field: String,
    ymin_field: String,
    xmax_field: String,
    ymax_field: String,
    xmin_leaf: usize,
    ymin_leaf: usize,
    xmax_leaf: usize,
    ymax_leaf: usize,
}

/// Metadata cached once per backend lifetime (`tokio::sync::OnceCell`,
/// matching `flatgeobuf`'s own deferred-open pattern) — a fresh async reader
/// is opened per query, but the footer (schema, "geo" JSON, and per-row-group
/// statistics) is only read once per backend.
struct CachedHeader {
    name: String,
    geometry_column: String,
    num_rows: u64,
    /// Row count of each row group, in file order — lets paging skip whole
    /// groups before a token/limit window without decoding them (both the
    /// unfiltered and bbox-filtered read paths).
    row_group_counts: Vec<u64>,
    covering: Option<CoveringColumns>,
    /// Per-row-group CRS84 bbox, precomputed once from the covering
    /// column's Parquet statistics — index-aligned with `row_group_counts`.
    /// `None` overall when the file has no covering metadata at all;
    /// `Some(None)` for one row group whose statistics happen to be missing
    /// (cannot safely prune that group, so it's treated as "might
    /// intersect" — see [`covering_bbox_might_intersect`]).
    covering_row_group_bboxes: Option<Vec<Option<[f64; 4]>>>,
    geometry_type: Option<String>,
    envelope: Option<[f64; 4]>,
    srid: Option<i32>,
    schema: Arc<Schema>,
    parquet_schema: Arc<SchemaDescriptor>,
    reader_metadata: ArrowReaderMetadata,
}

pub struct GeoparquetBackend {
    input: GeoparquetInput,
    header: OnceCell<Arc<CachedHeader>>,
}

impl GeoparquetBackend {
    fn new(path: PathBuf) -> Self {
        Self::from_input(GeoparquetInput::Local(path))
    }

    /// Builds a backend over a local path or a broker-authorized range object.
    pub fn from_input(input: GeoparquetInput) -> Self {
        Self {
            input,
            header: OnceCell::new(),
        }
    }

    async fn header(&self) -> Result<Arc<CachedHeader>> {
        let cached = self
            .header
            .get_or_try_init(|| async {
                let input = self.input.clone();
                let display_name = input.display_name();
                read_cached_header(input, &display_name).await.map(Arc::new)
            })
            .await?;
        Ok(Arc::clone(cached))
    }

    async fn catalog_inner(&self) -> Result<Vec<PhysicalCollection>> {
        let header = self.header().await?;
        Ok(vec![PhysicalCollection {
            name: header.name.clone(),
            geometry_column: Some(header.geometry_column.clone()),
            primary_key: Some(PRIMARY_KEY_FIELD.to_string()),
            srid: header.srid,
            geometry_type: header.geometry_type.clone(),
        }])
    }

    async fn extent_inner(&self) -> Result<Option<SpatialExtent>> {
        let header = self.header().await?;
        Ok(header.envelope.map(|bbox| SpatialExtent { bbox }))
    }

    /// `#19`: the file metadata's own row count — exact, and free (no row
    /// data touched), unlike PostGIS's planner-estimate `reltuples`.
    async fn row_estimate_inner(&self) -> Result<Option<u64>> {
        let header = self.header().await?;
        Ok(Some(header.num_rows))
    }

    /// `#19`: every Arrow schema field except the geometry column and (when
    /// present) the covering bbox helper column — both are structural, not
    /// user-facing attributes, mirroring how `postgis`'s own
    /// `attribute_schema` excludes just the named geometry column.
    async fn attribute_schema_inner(&self) -> Result<Option<Vec<AttributeColumn>>> {
        let header = self.header().await?;
        let covering_field = header.covering.as_ref().map(|c| c.struct_field.as_str());
        let columns = header
            .schema
            .fields()
            .iter()
            .filter(|field| {
                field.name() != &header.geometry_column
                    && Some(field.name().as_str()) != covering_field
            })
            .map(|field| AttributeColumn {
                name: field.name().clone(),
                sql_type: arrow_type_to_sql(field.data_type()),
            })
            .collect();
        Ok(Some(columns))
    }

    async fn items_inner(&self, query: &ItemsQuery) -> Result<FeaturePage> {
        if query.datetime.is_some() {
            return Err(GeoparquetDriverError::DatetimeUnsupported);
        }
        let token = parse_token(query.token.as_deref())?;
        let header = self.header().await?;
        let bbox = query.bbox;
        let limit = query.limit;
        let input = self.input.clone();

        match bbox {
            Some(bbox) => read_items_bbox(input, &header, bbox, token, limit).await,
            None => read_items_all(input, &header, token, limit).await,
        }
    }

    async fn item_inner(&self, id: &str) -> Result<Option<serde_json::Value>> {
        let Ok(target) = id.parse::<u64>() else {
            // A non-integer id can never match this driver's row-position
            // identity — same "honest None" convention flatgeobuf/postgis
            // apply to a non-integer id.
            return Ok(None);
        };
        let header = self.header().await?;
        if target >= header.num_rows {
            return Ok(None);
        }
        read_item_by_index(self.input.clone(), &header, target).await
    }

    async fn mvt_tile_inner(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
    ) -> CoreResult<Option<bytes::Bytes>> {
        if collection.srid != Some(4326) {
            return Err(unsupported_tile_crs(collection));
        }

        let tile_envelope =
            tile_envelope_3857(coord).map_err(|error| CoreError::Invalid(error.to_string()))?;
        let header = self.header().await.map_err(CoreError::from)?;
        if header.srid != Some(4326) {
            return Err(unsupported_tile_crs(collection));
        }

        let query_bbox = web_mercator_envelope_to_crs84(tile_envelope);
        if !tile_covering_is_usable(&header) {
            return Err(unsupported_tile_covering(collection));
        }
        if !tile_scan_within_budget(&header, query_bbox) {
            return Err(unsupported_tile_scan_budget(collection));
        }
        let cap = heuristics::effective_feature_cap(
            &collection.tiles.caps,
            coord.z,
            collection.row_estimate,
        );
        let feature_cap = usize::try_from(cap).unwrap_or(usize::MAX);
        let features = read_tile_features_bbox(
            self.input.clone(),
            &header,
            query_bbox,
            &collection.tile_properties,
            feature_cap,
        )
        .await
        .map_err(CoreError::from)?;
        let request = TileRequest::new(
            coord,
            collection.external_id(),
            collection.tile_properties.clone(),
            feature_cap,
            collection
                .settings
                .tile_vertex_budget
                .unwrap_or(DEFAULT_TILE_VERTEX_BUDGET),
            MVT_EXTENT,
            SourceCrs::Crs84,
        );

        encode_tile(request, features.into_iter().map(Ok))
            .map_err(|error| CoreError::Storage(Box::new(error)))
    }
}

#[async_trait]
impl CatalogSource for GeoparquetBackend {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        self.catalog_inner().await.map_err(Into::into)
    }

    async fn extent(&self, _physical: &PhysicalCollection) -> CoreResult<Option<SpatialExtent>> {
        self.extent_inner().await.map_err(Into::into)
    }

    async fn row_estimate(&self, _physical: &PhysicalCollection) -> CoreResult<Option<u64>> {
        self.row_estimate_inner().await.map_err(Into::into)
    }

    async fn attribute_schema(
        &self,
        _physical: &PhysicalCollection,
    ) -> CoreResult<Option<Vec<AttributeColumn>>> {
        self.attribute_schema_inner().await.map_err(Into::into)
    }

    // `temporal_column`: default `None`. GeoParquet's "geo" metadata has no
    // per-column semantic marker for "the" datetime column (only geometry
    // columns get a dedicated metadata entry) and this driver never guesses
    // from an Arrow `Timestamp` column's name — same deliberate-dumb
    // "exactly one candidate, else unknown" posture the trait's own doc
    // comment asks for, applied here as "zero candidates are ever offered."
}

#[async_trait]
impl FeatureSource for GeoparquetBackend {
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

#[async_trait]
impl TileSource for GeoparquetBackend {
    async fn mvt_tile(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
        _filter: Option<&tellurion_core::Filter>,
    ) -> CoreResult<Option<bytes::Bytes>> {
        self.mvt_tile_inner(collection, coord).await
    }

    fn tile_capable(&self, collection: &CollectionDecl) -> bool {
        collection.srid == Some(4326)
    }
}

fn unsupported_tile_crs(collection: &CollectionDecl) -> CoreError {
    CoreError::CapabilityUnsupported {
        collection: collection.id.clone(),
        capability: "tiles:crs84".to_string(),
    }
}

fn unsupported_tile_covering(collection: &CollectionDecl) -> CoreError {
    CoreError::CapabilityUnsupported {
        collection: collection.id.clone(),
        capability: "tiles:covering".to_string(),
    }
}

fn unsupported_tile_scan_budget(collection: &CollectionDecl) -> CoreError {
    CoreError::CapabilityUnsupported {
        collection: collection.id.clone(),
        capability: "tiles:scan-budget".to_string(),
    }
}

fn web_mercator_envelope_to_crs84([minx, miny, maxx, maxy]: [f64; 4]) -> [f64; 4] {
    const WEB_MERCATOR_RADIUS: f64 = 6_378_137.0;
    let inverse = |x: f64, y: f64| {
        let lon = x.to_degrees() / WEB_MERCATOR_RADIUS;
        let lat = (2.0 * (y / WEB_MERCATOR_RADIUS).exp().atan() - std::f64::consts::FRAC_PI_2)
            .to_degrees();
        (lon, lat)
    };
    let (min_lon, min_lat) = inverse(minx, miny);
    let (max_lon, max_lat) = inverse(maxx, maxy);
    [min_lon, min_lat, max_lon, max_lat]
}

fn parse_token(token: Option<&str>) -> Result<Option<u64>> {
    match token {
        None => Ok(None),
        Some(raw) => raw
            .parse::<u64>()
            .map(Some)
            .map_err(|_| GeoparquetDriverError::InvalidToken(raw.to_string())),
    }
}

/// The collection name this file reports. Unlike FlatGeobuf's header, plain
/// Parquet has no dataset-name field at all (GeoParquet's "geo" metadata
/// doesn't add one either), so this always falls back to the file stem —
/// matched against `CollectionDecl::table`/`id` by `Router::validate_catalog`.
fn header_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("dataset")
        .to_string()
}

/// Reduces GeoParquet's `geometry_types` list to a single conventional
/// uppercase name, the same "exactly one candidate, else unknown" way
/// `CatalogSource::temporal_column` treats column ambiguity elsewhere in
/// this contract: zero entries (any/mixed type, legal per spec) or more
/// than one both report `None` rather than guessing. A 3D suffix (spec
/// syntax `"Point Z"`) is stripped to match flatgeobuf's own bare
/// `"POINT"`/`"POLYGON"`/... naming.
fn geometry_type_name(geometry_types: &[String]) -> Option<String> {
    match geometry_types {
        [single] => single.split_whitespace().next().map(str::to_uppercase),
        _ => None,
    }
}

/// See this module's "CRS assumption" docs.
fn srid_from_crs(crs: Option<&serde_json::Value>) -> Option<i32> {
    match crs {
        None | Some(serde_json::Value::Null) => Some(4326),
        Some(value) => {
            let id = value.get("id")?;
            let authority = id.get("authority")?.as_str()?;
            if !authority.eq_ignore_ascii_case("EPSG") {
                return None;
            }
            i32::try_from(id.get("code")?.as_i64()?).ok()
        }
    }
}

fn tile_covering_is_usable(header: &CachedHeader) -> bool {
    header
        .covering_row_group_bboxes
        .as_ref()
        .is_some_and(|bboxes| {
            bboxes
                .iter()
                .all(|bbox| bbox.is_some_and(tile_covering_bbox_is_usable))
        })
}

fn tile_covering_bbox_is_usable([xmin, ymin, xmax, ymax]: [f64; 4]) -> bool {
    [xmin, ymin, xmax, ymax]
        .iter()
        .all(|value| value.is_finite())
        && xmin <= xmax
        && ymin <= ymax
}

fn tile_scan_within_budget(header: &CachedHeader, query_bbox: [f64; 4]) -> bool {
    header
        .row_group_counts
        .iter()
        .zip(
            header
                .covering_row_group_bboxes
                .as_ref()
                .expect("usable covering metadata was checked before tile scan"),
        )
        .filter(|(_, bbox)| covering_bbox_might_intersect(**bbox, query_bbox))
        .try_fold(0u64, |rows, (&group_rows, _)| rows.checked_add(group_rows))
        .is_some_and(|rows| rows <= TILE_SCAN_ROW_BUDGET)
}

/// Broad SQL-flavored type name for an Arrow column, in the spirit of
/// `tellurion-postgis`'s own `attribute_schema` (which reports Postgres'
/// `information_schema.columns.data_type` strings): approximate, never a
/// full type (no length/precision). Arrow types with no direct SQL
/// equivalent (nested/list/struct/etc.) fall back to a lowercased Arrow
/// debug name rather than failing the whole call over one exotic column —
/// `attribute_schema` only reports names/shapes, nothing downstream parses
/// this string.
fn arrow_type_to_sql(data_type: &DataType) -> String {
    let name = match data_type {
        DataType::Boolean => "boolean",
        DataType::Int8 | DataType::Int16 | DataType::Int32 => "integer",
        DataType::Int64 => "bigint",
        DataType::UInt8 | DataType::UInt16 | DataType::UInt32 => "integer",
        DataType::UInt64 => "bigint",
        DataType::Float16 | DataType::Float32 => "real",
        DataType::Float64 => "double precision",
        DataType::Utf8 | DataType::LargeUtf8 => "text",
        DataType::Binary | DataType::LargeBinary => "bytea",
        DataType::Date32 | DataType::Date64 => "date",
        DataType::Timestamp(_, Some(_)) => "timestamp with time zone",
        DataType::Timestamp(_, None) => "timestamp without time zone",
        other => return format!("{other:?}").to_lowercase(),
    };
    name.to_string()
}

/// Resolves a GeoParquet 1.1 `covering.bbox` block's dotted column paths
/// (e.g. `xmin: ["bbox", "xmin"]`) against the file's physical leaf-column
/// list. `None` when any path doesn't resolve (malformed metadata, or a
/// covering that references columns this particular file doesn't actually
/// have) — silently falls back to decode-and-filter rather than failing the
/// whole file over one optional optimization hint.
fn resolve_covering(
    schema_descr: &SchemaDescriptor,
    covering: &CoveringPaths,
) -> Option<CoveringColumns> {
    let leaf_index = |path: &[String]| -> Option<usize> {
        schema_descr
            .columns()
            .iter()
            .position(|column| column.path().parts() == path)
    };
    let field_name = |path: &[String]| -> Option<String> { path.get(1).cloned() };

    Some(CoveringColumns {
        struct_field: covering.xmin.first()?.clone(),
        xmin_field: field_name(&covering.xmin)?,
        ymin_field: field_name(&covering.ymin)?,
        xmax_field: field_name(&covering.xmax)?,
        ymax_field: field_name(&covering.ymax)?,
        xmin_leaf: leaf_index(&covering.xmin)?,
        ymin_leaf: leaf_index(&covering.ymin)?,
        xmax_leaf: leaf_index(&covering.xmax)?,
        ymax_leaf: leaf_index(&covering.ymax)?,
    })
}

fn stat_min(row_group: &RowGroupMetaData, leaf: usize) -> Option<f64> {
    match row_group.column(leaf).statistics() {
        Some(Statistics::Double(stats)) => stats.min_opt().copied(),
        _ => None,
    }
}

fn stat_max(row_group: &RowGroupMetaData, leaf: usize) -> Option<f64> {
    match row_group.column(leaf).statistics() {
        Some(Statistics::Double(stats)) => stats.max_opt().copied(),
        _ => None,
    }
}

/// One row group's CRS84 bbox, read straight from its own column
/// statistics — `None` when any of the four leaf columns lacks statistics
/// (e.g. written with stats disabled), meaning this group can't be safely
/// pruned. See this module's "Row-group pruning" docs.
fn row_group_covering_bbox(
    row_group: &RowGroupMetaData,
    covering: &CoveringColumns,
) -> Option<[f64; 4]> {
    Some([
        stat_min(row_group, covering.xmin_leaf)?,
        stat_min(row_group, covering.ymin_leaf)?,
        stat_max(row_group, covering.xmax_leaf)?,
        stat_max(row_group, covering.ymax_leaf)?,
    ])
}

fn bbox_intersects(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] <= b[2] && a[2] >= b[0] && a[1] <= b[3] && a[3] >= b[1]
}

/// `None` (missing/unreadable statistics) always answers `true` — cannot
/// safely prune, so the row group is decoded rather than silently dropped.
fn covering_bbox_might_intersect(group_bbox: Option<[f64; 4]>, query_bbox: [f64; 4]) -> bool {
    match group_bbox {
        Some(bbox) => bbox_intersects(bbox, query_bbox),
        None => true,
    }
}

/// Minimal `geozero::GeomProcessor` that only tracks the enclosing 2D bbox
/// of every coordinate it sees — the decode-and-filter fallback
/// [`row_bbox_from_batch`] uses when a file has no GeoParquet 1.1 covering
/// column. Every other `GeomProcessor` callback keeps its no-op default; a
/// bbox is a fold over `xy`/`coordinate` alone.
#[derive(Default)]
struct BboxCollector {
    bbox: Option<[f64; 4]>,
}

impl BboxCollector {
    fn accumulate(&mut self, x: f64, y: f64) {
        self.bbox = Some(match self.bbox {
            Some([minx, miny, maxx, maxy]) => [minx.min(x), miny.min(y), maxx.max(x), maxy.max(y)],
            None => [x, y, x, y],
        });
    }
}

impl geozero::GeomProcessor for BboxCollector {
    fn xy(&mut self, x: f64, y: f64, _idx: usize) -> geozero::error::Result<()> {
        self.accumulate(x, y);
        Ok(())
    }

    fn coordinate(
        &mut self,
        x: f64,
        y: f64,
        _z: Option<f64>,
        _m: Option<f64>,
        _t: Option<f64>,
        _tm: Option<u64>,
        _idx: usize,
    ) -> geozero::error::Result<()> {
        self.accumulate(x, y);
        Ok(())
    }

    /// The trait's default errors ("output doesn't support empty Points") —
    /// a bbox fold has no such restriction; an empty point just contributes
    /// nothing to the running bbox.
    fn empty_point(&mut self, _idx: usize) -> geozero::error::Result<()> {
        Ok(())
    }
}

/// Downcasts an Arrow `ArrayRef` to a concrete array type. Should never fail
/// in practice — every call site already matched the column's `DataType`
/// before reaching here — but Rust can't statically prove that, so this
/// reports an honest [`GeoparquetDriverError::Decode`] rather than
/// panicking on a `DataType`/array-type mismatch this driver's own logic
/// has a bug in.
fn downcast<T: 'static>(array: &ArrayRef) -> Result<&T> {
    array.as_any().downcast_ref::<T>().ok_or_else(|| {
        GeoparquetDriverError::Decode(format!(
            "expected array type {}, got {:?}",
            std::any::type_name::<T>(),
            array.data_type()
        ))
    })
}

fn json_int(value: i64) -> serde_json::Value {
    serde_json::Value::Number(value.into())
}

fn json_float(value: f64) -> serde_json::Value {
    serde_json::Number::from_f64(value)
        .map(serde_json::Value::Number)
        .unwrap_or(serde_json::Value::Null)
}

/// One property value from one row of a decoded batch. Supports GeoParquet's
/// common scalar attribute types (bool/int/uint/float/utf8) — anything else
/// is an honest [`GeoparquetDriverError::Decode`] rather than silently
/// emitting a wrong or lossy value; v0.1's scope is the practical GeoParquet
/// attribute shapes this driver's own fixture (and typical GDAL/GeoPandas
/// exports) actually use.
fn arrow_value_to_json(
    array: &ArrayRef,
    data_type: &DataType,
    row: usize,
) -> Result<serde_json::Value> {
    if array.is_null(row) {
        return Ok(serde_json::Value::Null);
    }
    let value = match data_type {
        DataType::Boolean => serde_json::Value::Bool(downcast::<BooleanArray>(array)?.value(row)),
        DataType::Int8 => json_int(downcast::<Int8Array>(array)?.value(row) as i64),
        DataType::Int16 => json_int(downcast::<Int16Array>(array)?.value(row) as i64),
        DataType::Int32 => json_int(downcast::<Int32Array>(array)?.value(row) as i64),
        DataType::Int64 => json_int(downcast::<Int64Array>(array)?.value(row)),
        DataType::UInt8 => json_int(downcast::<UInt8Array>(array)?.value(row) as i64),
        DataType::UInt16 => json_int(downcast::<UInt16Array>(array)?.value(row) as i64),
        DataType::UInt32 => json_int(downcast::<UInt32Array>(array)?.value(row) as i64),
        DataType::UInt64 => {
            serde_json::Value::Number(downcast::<UInt64Array>(array)?.value(row).into())
        }
        DataType::Float32 => json_float(downcast::<Float32Array>(array)?.value(row) as f64),
        DataType::Float64 => json_float(downcast::<Float64Array>(array)?.value(row)),
        DataType::Utf8 => {
            serde_json::Value::String(downcast::<StringArray>(array)?.value(row).to_string())
        }
        DataType::LargeUtf8 => {
            serde_json::Value::String(downcast::<LargeStringArray>(array)?.value(row).to_string())
        }
        other => {
            return Err(GeoparquetDriverError::Decode(format!(
                "unsupported attribute column type: {other:?}"
            )))
        }
    };
    Ok(value)
}

/// `batch`'s non-geometry, non-covering columns at `row`, keyed by field
/// name — a feature's GeoJSON `properties`.
fn properties_from_batch(
    batch: &RecordBatch,
    header: &CachedHeader,
    row: usize,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    let covering_field = header.covering.as_ref().map(|c| c.struct_field.as_str());
    let mut properties = serde_json::Map::new();
    for field in header.schema.fields() {
        let name = field.name();
        if name == &header.geometry_column || Some(name.as_str()) == covering_field {
            continue;
        }
        let column = batch.column_by_name(name).ok_or_else(|| {
            GeoparquetDriverError::Decode(format!("column '{name}' missing from decoded batch"))
        })?;
        properties.insert(
            name.clone(),
            arrow_value_to_json(column, field.data_type(), row)?,
        );
    }
    Ok(properties)
}

/// Decodes one WKB geometry (GeoParquet's fixed encoding — plain ISO WKB,
/// never EWKB) into a bare GeoJSON geometry object, via the same
/// `geozero::geojson::GeoJsonWriter` `flatgeobuf`'s driver uses — except
/// here the writer is driven by `GeozeroGeometry::process_geom` directly
/// (there is no `FeatureAccess`-style combined geometry+properties source
/// the way a `FgbFeature` is; properties come from the Arrow batch instead,
/// assembled separately in [`properties_from_batch`]).
fn geometry_json_from_wkb(wkb: &[u8]) -> Result<serde_json::Value> {
    use geozero::GeozeroGeometry;

    let mut buf: Vec<u8> = Vec::new();
    {
        let mut writer = geozero::geojson::GeoJsonWriter::new(&mut buf);
        geozero::wkb::Wkb(wkb).process_geom(&mut writer)?;
    }
    Ok(serde_json::from_slice(&buf)?)
}

/// `batch`'s row `row_in_batch`'s bbox — the fast path reads it straight
/// from the decoded covering struct column (already loaded as part of the
/// batch, no extra I/O or WKB parse); the fallback decodes the row's actual
/// WKB geometry through [`BboxCollector`]. See this module's "Row-group
/// pruning" docs.
fn row_bbox_from_batch(batch: &RecordBatch, header: &CachedHeader, row: usize) -> Result<[f64; 4]> {
    use geozero::GeozeroGeometry;

    if let Some(covering) = &header.covering {
        let struct_column = batch
            .column_by_name(&covering.struct_field)
            .ok_or_else(|| {
                GeoparquetDriverError::Decode(format!(
                    "covering column '{}' missing from decoded batch",
                    covering.struct_field
                ))
            })?;
        let struct_array = downcast::<StructArray>(struct_column)?;
        let field_value = |field_name: &str| -> Result<f64> {
            let child = struct_array.column_by_name(field_name).ok_or_else(|| {
                GeoparquetDriverError::Decode(format!(
                    "covering bbox field '{field_name}' missing from struct column"
                ))
            })?;
            Ok(downcast::<Float64Array>(child)?.value(row))
        };
        Ok([
            field_value(&covering.xmin_field)?,
            field_value(&covering.ymin_field)?,
            field_value(&covering.xmax_field)?,
            field_value(&covering.ymax_field)?,
        ])
    } else {
        let geometry_column = batch
            .column_by_name(&header.geometry_column)
            .ok_or_else(|| {
                GeoparquetDriverError::Decode(format!(
                    "geometry column '{}' missing from decoded batch",
                    header.geometry_column
                ))
            })?;
        let binary = downcast::<BinaryArray>(geometry_column)?;
        let mut collector = BboxCollector::default();
        geozero::wkb::Wkb(binary.value(row)).process_geom(&mut collector)?;
        collector
            .bbox
            .ok_or_else(|| GeoparquetDriverError::Decode("empty geometry has no bbox".to_string()))
    }
}

/// Turns one decoded row into a full GeoJSON `Feature` object with `id` set
/// to `pk` (as a string, matching flatgeobuf's/postgis's own `pk::text`
/// convention).
fn feature_to_geojson(
    batch: &RecordBatch,
    header: &CachedHeader,
    row: usize,
    pk: u64,
) -> Result<serde_json::Value> {
    let geometry_column = batch
        .column_by_name(&header.geometry_column)
        .ok_or_else(|| {
            GeoparquetDriverError::Decode(format!(
                "geometry column '{}' missing from decoded batch",
                header.geometry_column
            ))
        })?;
    let binary = downcast::<BinaryArray>(geometry_column)?;
    let geometry = geometry_json_from_wkb(binary.value(row))?;
    let properties = properties_from_batch(batch, header, row)?;

    let mut feature = serde_json::Map::new();
    feature.insert(
        "type".to_string(),
        serde_json::Value::String("Feature".to_string()),
    );
    feature.insert("id".to_string(), serde_json::Value::String(pk.to_string()));
    feature.insert("geometry".to_string(), geometry);
    feature.insert(
        "properties".to_string(),
        serde_json::Value::Object(properties),
    );
    Ok(serde_json::Value::Object(feature))
}

async fn read_cached_header(input: GeoparquetInput, display_name: &str) -> Result<CachedHeader> {
    let reader = input.into_async_reader().await?;
    let builder = ParquetRecordBatchStreamBuilder::new(reader).await?;
    let metadata = Arc::clone(builder.metadata());
    let reader_metadata =
        ArrowReaderMetadata::try_new(Arc::clone(&metadata), ArrowReaderOptions::new())?;
    let schema = Arc::clone(builder.schema());
    let file_metadata = metadata.file_metadata();
    let parquet_schema = Arc::new(file_metadata.schema_descr().clone());

    let geo_raw = file_metadata
        .key_value_metadata()
        .and_then(|entries| entries.iter().find(|entry| entry.key == GEO_METADATA_KEY))
        .and_then(|entry| entry.value.as_deref())
        .ok_or(GeoparquetDriverError::MissingGeoMetadata)?;
    let geo = parse_geo_metadata(geo_raw)?;

    let num_rows = file_metadata.num_rows().max(0) as u64;
    let row_group_counts: Vec<u64> = metadata
        .row_groups()
        .iter()
        .map(|group| group.num_rows().max(0) as u64)
        .collect();

    let covering = geo
        .covering
        .as_ref()
        .and_then(|paths| resolve_covering(&parquet_schema, paths));
    let covering_row_group_bboxes = covering.as_ref().map(|covering| {
        metadata
            .row_groups()
            .iter()
            .map(|group| row_group_covering_bbox(group, covering))
            .collect()
    });

    Ok(CachedHeader {
        name: header_name(Path::new(display_name)),
        geometry_column: geo.primary_column,
        num_rows,
        row_group_counts,
        covering,
        covering_row_group_bboxes,
        geometry_type: geometry_type_name(&geo.geometry_types),
        envelope: geo.bbox,
        srid: srid_from_crs(geo.crs.as_ref()),
        schema,
        parquet_schema,
        reader_metadata,
    })
}

/// Opens a fresh async reader scoped to exactly `row_group_indices`. The
/// projection is explicit even though this version returns every user-visible
/// field: it keeps the I/O boundary ready for the driver's computed column
/// selection rather than asking Parquet to infer one after reads begin.
async fn open_row_groups(
    input: GeoparquetInput,
    header: &CachedHeader,
    row_group_indices: Vec<usize>,
    batch_size: usize,
    projection: ProjectionMask,
) -> parquet::errors::Result<ParquetRecordBatchStream<Box<dyn AsyncFileReader>>> {
    let reader = input.into_async_reader().await?;
    let builder =
        ParquetRecordBatchStreamBuilder::new_with_metadata(reader, header.reader_metadata.clone());
    let stream = builder
        .with_row_groups(row_group_indices)
        .with_batch_size(batch_size.max(1))
        .with_projection(projection)
        .build()?;
    Ok(stream)
}

fn bbox_projection(header: &CachedHeader, materialize_features: bool) -> Result<ProjectionMask> {
    if materialize_features {
        return Ok(ProjectionMask::all());
    }
    if let Some(covering) = &header.covering {
        return Ok(ProjectionMask::leaves(
            &header.parquet_schema,
            [
                covering.xmin_leaf,
                covering.ymin_leaf,
                covering.xmax_leaf,
                covering.ymax_leaf,
            ],
        ));
    }
    let geometry_root = header
        .schema
        .fields()
        .iter()
        .position(|field| field.name() == &header.geometry_column)
        .ok_or_else(|| {
            GeoparquetDriverError::Decode(format!(
                "geometry column '{}' missing from Arrow schema",
                header.geometry_column
            ))
        })?;
    Ok(ProjectionMask::roots(
        &header.parquet_schema,
        [geometry_root],
    ))
}

fn tile_projection(
    header: &CachedHeader,
    selected_properties: &[String],
) -> Result<ProjectionMask> {
    let mut roots = Vec::with_capacity(selected_properties.len() + 2);
    let mut include = |name: &str| -> Result<()> {
        let root = header
            .schema
            .fields()
            .iter()
            .position(|field| field.name() == name)
            .ok_or_else(|| {
                GeoparquetDriverError::Decode(format!(
                    "tile column '{name}' missing from Arrow schema"
                ))
            })?;
        if !roots.contains(&root) {
            roots.push(root);
        }
        Ok(())
    };

    include(&header.geometry_column)?;
    if let Some(covering) = &header.covering {
        include(&covering.struct_field)?;
    }
    for property in selected_properties {
        include(property)?;
    }
    Ok(ProjectionMask::roots(&header.parquet_schema, roots))
}

fn arrow_value_to_tile_scalar(
    array: &ArrayRef,
    data_type: &DataType,
    row: usize,
) -> Result<TileScalar> {
    if array.is_null(row) {
        return Ok(TileScalar::Null);
    }
    match data_type {
        DataType::Boolean => Ok(TileScalar::Bool(
            downcast::<BooleanArray>(array)?.value(row),
        )),
        DataType::Int8 => Ok(TileScalar::Signed(
            downcast::<Int8Array>(array)?.value(row) as i64
        )),
        DataType::Int16 => Ok(TileScalar::Signed(
            downcast::<Int16Array>(array)?.value(row) as i64,
        )),
        DataType::Int32 => Ok(TileScalar::Signed(
            downcast::<Int32Array>(array)?.value(row) as i64,
        )),
        DataType::Int64 => Ok(TileScalar::Signed(
            downcast::<Int64Array>(array)?.value(row),
        )),
        DataType::UInt8 => Ok(TileScalar::Unsigned(
            downcast::<UInt8Array>(array)?.value(row) as u64,
        )),
        DataType::UInt16 => Ok(TileScalar::Unsigned(
            downcast::<UInt16Array>(array)?.value(row) as u64,
        )),
        DataType::UInt32 => Ok(TileScalar::Unsigned(
            downcast::<UInt32Array>(array)?.value(row) as u64,
        )),
        DataType::UInt64 => Ok(TileScalar::Unsigned(
            downcast::<UInt64Array>(array)?.value(row),
        )),
        DataType::Float32 => Ok(TileScalar::Float(
            downcast::<Float32Array>(array)?.value(row) as f64,
        )),
        DataType::Float64 => Ok(TileScalar::Float(
            downcast::<Float64Array>(array)?.value(row),
        )),
        DataType::Utf8 => Ok(TileScalar::String(
            downcast::<StringArray>(array)?.value(row).to_string(),
        )),
        DataType::LargeUtf8 => Ok(TileScalar::String(
            downcast::<LargeStringArray>(array)?.value(row).to_string(),
        )),
        other => Err(GeoparquetDriverError::Decode(format!(
            "unsupported tile attribute column type: {other:?}"
        ))),
    }
}

fn tile_feature_from_batch(
    batch: &RecordBatch,
    header: &CachedHeader,
    selected_properties: &[String],
    row: usize,
    pk: u64,
) -> Result<TileFeature> {
    use geozero::ToGeo;

    let geometry_column = batch
        .column_by_name(&header.geometry_column)
        .ok_or_else(|| {
            GeoparquetDriverError::Decode(format!(
                "geometry column '{}' missing from decoded batch",
                header.geometry_column
            ))
        })?;
    let geometry =
        geozero::wkb::Wkb(downcast::<BinaryArray>(geometry_column)?.value(row)).to_geo()?;
    let mut properties = Vec::with_capacity(selected_properties.len());
    for name in selected_properties {
        let field = header.schema.field_with_name(name)?;
        let column = batch.column_by_name(name).ok_or_else(|| {
            GeoparquetDriverError::Decode(format!(
                "tile property column '{name}' missing from decoded batch"
            ))
        })?;
        properties.push((
            name.clone(),
            arrow_value_to_tile_scalar(column, field.data_type(), row)?,
        ));
    }
    Ok(TileFeature::new(pk.to_string(), geometry, properties))
}

/// Reads only bbox candidates, stopping at the configured feature cap. Row
/// groups are pruned from footer statistics and rows are pruned from their
/// covering bbox (or decoded WKB bbox fallback); exact intersection and
/// clipping remain the shared encoder's job.
async fn read_tile_features_bbox(
    input: GeoparquetInput,
    header: &CachedHeader,
    bbox: [f64; 4],
    selected_properties: &[String],
    feature_cap: usize,
) -> Result<Vec<TileFeature>> {
    if feature_cap == 0 {
        return Ok(Vec::new());
    }
    let projection = tile_projection(header, selected_properties)?;
    let mut features = Vec::with_capacity(feature_cap.min(TILE_SCAN_BATCH_SIZE));
    let mut global_offset = 0u64;

    'groups: for (group_idx, &group_rows) in header.row_group_counts.iter().enumerate() {
        let might_intersect = match &header.covering_row_group_bboxes {
            Some(group_bboxes) => covering_bbox_might_intersect(group_bboxes[group_idx], bbox),
            None => true,
        };
        if !might_intersect {
            global_offset += group_rows;
            continue;
        }

        let mut reader = open_row_groups(
            input.clone(),
            header,
            vec![group_idx],
            TILE_SCAN_BATCH_SIZE,
            projection.clone(),
        )
        .await?;
        let mut local_row = 0u64;
        while let Some(batch) = reader.try_next().await? {
            for row_in_batch in 0..batch.num_rows() {
                if !bbox_intersects(row_bbox_from_batch(&batch, header, row_in_batch)?, bbox) {
                    continue;
                }
                features.push(tile_feature_from_batch(
                    &batch,
                    header,
                    selected_properties,
                    row_in_batch,
                    global_offset + local_row + row_in_batch as u64,
                )?);
                if features.len() >= feature_cap {
                    break 'groups;
                }
            }
            local_row += batch.num_rows() as u64;
        }
        global_offset += group_rows;
    }
    Ok(features)
}

fn incomplete_bbox_page(
    features_geojson: Vec<serde_json::Value>,
    last_idx: Option<u64>,
) -> FeaturePage {
    FeaturePage {
        features_geojson,
        number_matched: None,
        next_token: last_idx.map(|value| value.to_string()),
    }
}

#[cfg(feature = "remote")]
fn is_budget_exhaustion(error: &ParquetError) -> bool {
    matches!(
        error,
        ParquetError::External(source)
            if source
                .downcast_ref::<SourceError>()
                .is_some_and(|source| source.kind() == SourceErrorKind::Budget)
    )
}

#[cfg(not(feature = "remote"))]
fn is_budget_exhaustion(_error: &ParquetError) -> bool {
    false
}

/// Unfiltered listing. `number_matched` is free and exact from the header's
/// own row count; paging skips whole row groups entirely before the
/// `token`/`limit` window (no decode, just arithmetic over
/// `row_group_counts`) and stops as soon as one extra row past `limit` is
/// seen, exactly like flatgeobuf's own `select_all` early-terminate.
async fn read_items_all(
    input: GeoparquetInput,
    header: &CachedHeader,
    token: Option<u64>,
    limit: u32,
) -> Result<FeaturePage> {
    let want = limit as usize;
    let window_start = token.map(|t| t + 1).unwrap_or(0);

    let mut features = Vec::new();
    let mut has_more = false;
    let mut last_idx: Option<u64> = None;
    let mut global_offset: u64 = 0;

    'groups: for (group_idx, &group_rows) in header.row_group_counts.iter().enumerate() {
        if global_offset + group_rows <= window_start {
            global_offset += group_rows;
            continue;
        }

        let mut reader = open_row_groups(
            input.clone(),
            header,
            vec![group_idx],
            group_rows as usize,
            ProjectionMask::all(),
        )
        .await?;
        let mut local_row: u64 = 0;
        while let Some(batch) = reader.try_next().await? {
            for row_in_batch in 0..batch.num_rows() {
                let global_idx = global_offset + local_row + row_in_batch as u64;
                if global_idx < window_start {
                    continue;
                }
                if features.len() >= want {
                    has_more = true;
                    break 'groups;
                }
                features.push(feature_to_geojson(
                    &batch,
                    header,
                    row_in_batch,
                    global_idx,
                )?);
                last_idx = Some(global_idx);
            }
            local_row += batch.num_rows() as u64;
        }
        global_offset += group_rows;
    }

    Ok(FeaturePage {
        features_geojson: features,
        number_matched: Some(header.num_rows),
        next_token: has_more.then(|| last_idx.map(|v| v.to_string())).flatten(),
    })
}

/// Bbox-filtered listing — see this module's "Row-group pruning" and
/// "Counting" docs for the pruning strategy and the exact-but-full-scan
/// count this always pays for.
async fn read_items_bbox(
    input: GeoparquetInput,
    header: &CachedHeader,
    bbox: [f64; 4],
    token: Option<u64>,
    limit: u32,
) -> Result<FeaturePage> {
    let want = limit as usize;
    let mut features = Vec::new();
    let mut matched: u64 = 0;
    let mut has_more = false;
    let mut last_idx: Option<u64> = None;
    let mut global_offset: u64 = 0;

    for (group_idx, &group_rows) in header.row_group_counts.iter().enumerate() {
        let might_intersect = match &header.covering_row_group_bboxes {
            Some(group_bboxes) => covering_bbox_might_intersect(group_bboxes[group_idx], bbox),
            None => true,
        };
        if !might_intersect {
            global_offset += group_rows;
            continue;
        }

        let materialize_features = features.len() < want;
        let projection = bbox_projection(header, materialize_features)?;
        let mut reader = match open_row_groups(
            input.clone(),
            header,
            vec![group_idx],
            group_rows as usize,
            projection,
        )
        .await
        {
            Ok(reader) => reader,
            Err(error) if !materialize_features && is_budget_exhaustion(&error) => {
                return Ok(incomplete_bbox_page(features, last_idx));
            }
            Err(error) => return Err(error.into()),
        };
        let mut local_row: u64 = 0;
        while let Some(batch) = match reader.try_next().await {
            Ok(batch) => batch,
            Err(error) if features.len() >= want && is_budget_exhaustion(&error) => {
                return Ok(incomplete_bbox_page(features, last_idx));
            }
            Err(error) => return Err(error.into()),
        } {
            for row_in_batch in 0..batch.num_rows() {
                let global_idx = global_offset + local_row + row_in_batch as u64;
                let row_bbox = row_bbox_from_batch(&batch, header, row_in_batch)?;
                if !bbox_intersects(row_bbox, bbox) {
                    continue;
                }
                matched += 1;
                if token.is_some_and(|t| global_idx <= t) {
                    continue;
                }
                if features.len() < want {
                    features.push(feature_to_geojson(
                        &batch,
                        header,
                        row_in_batch,
                        global_idx,
                    )?);
                    last_idx = Some(global_idx);
                } else {
                    has_more = true;
                }
            }
            local_row += batch.num_rows() as u64;
        }
        global_offset += group_rows;
    }

    Ok(FeaturePage {
        features_geojson: features,
        number_matched: Some(matched),
        next_token: has_more.then(|| last_idx.map(|v| v.to_string())).flatten(),
    })
}

async fn read_item_by_index(
    input: GeoparquetInput,
    header: &CachedHeader,
    target: u64,
) -> Result<Option<serde_json::Value>> {
    let mut group_offset: u64 = 0;
    for (group_idx, &group_rows) in header.row_group_counts.iter().enumerate() {
        if target >= group_offset + group_rows {
            group_offset += group_rows;
            continue;
        }

        let local_target = target - group_offset;
        let mut reader = open_row_groups(
            input,
            header,
            vec![group_idx],
            group_rows as usize,
            ProjectionMask::all(),
        )
        .await?;
        let mut local_row: u64 = 0;
        while let Some(batch) = reader.try_next().await? {
            let batch_rows = batch.num_rows() as u64;
            if local_target >= local_row && local_target < local_row + batch_rows {
                let row_in_batch = (local_target - local_row) as usize;
                return Ok(Some(feature_to_geojson(
                    &batch,
                    header,
                    row_in_batch,
                    target,
                )?));
            }
            local_row += batch_rows;
        }
        return Ok(None);
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny.parquet")
    }

    fn backend() -> GeoparquetBackend {
        GeoparquetBackend::new(fixture_path())
    }

    fn decl() -> CollectionDecl {
        serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap()
    }

    #[test]
    fn factory_name_matches_the_config_driver_key() {
        assert_eq!(GeoparquetDriverFactory::new().name(), "geoparquet");
    }

    #[test]
    fn out_of_range_projjson_epsg_code_cannot_alias_crs84() {
        let crs = serde_json::json!({
            "id": { "authority": "EPSG", "code": 4_294_971_622_i64 }
        });
        assert_eq!(srid_from_crs(Some(&crs)), None);
    }

    #[test]
    fn tile_covering_bbox_requires_finite_ordered_bounds() {
        assert!(tile_covering_bbox_is_usable([0.0, 1.0, 2.0, 3.0]));
        assert!(!tile_covering_bbox_is_usable([f64::NAN, 1.0, 2.0, 3.0]));
        assert!(!tile_covering_bbox_is_usable([2.0, 1.0, 0.0, 3.0]));
        assert!(!tile_covering_bbox_is_usable([0.0, 3.0, 2.0, 1.0]));
    }

    #[test]
    fn build_fails_fast_when_the_env_var_is_unset() {
        let factory = GeoparquetDriverFactory::new();
        let decl = StorageDecl {
            id: "main".to_string(),
            driver: "geoparquet".to_string(),
            url_env: "TELLURION_GEOPARQUET_TEST_DOES_NOT_EXIST".to_string(),
            pool_size: None,
        };
        std::env::remove_var(&decl.url_env);
        assert!(matches!(factory.build(&decl), Err(CoreError::Config(_))));
    }

    #[tokio::test]
    async fn collections_reports_the_geo_metadata_derived_identity() {
        let backend = backend();
        let collections = backend.collections().await.unwrap();
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].name, "tiny");
        assert_eq!(collections[0].geometry_column.as_deref(), Some("geometry"));
        assert_eq!(collections[0].primary_key.as_deref(), Some("fid"));
        assert_eq!(collections[0].geometry_type.as_deref(), Some("POINT"));
        assert_eq!(collections[0].srid, Some(4326));
    }

    #[tokio::test]
    async fn extent_comes_from_the_geo_metadata_bbox() {
        let backend = backend();
        let physical = &backend.collections().await.unwrap()[0];
        let extent = backend.extent(physical).await.unwrap().unwrap();
        assert_eq!(extent.bbox, [-4.0, 46.0, 4.0, 54.0]);
    }

    #[tokio::test]
    async fn row_estimate_is_exact_from_file_metadata() {
        let backend = backend();
        let physical = &backend.collections().await.unwrap()[0];
        assert_eq!(backend.row_estimate(physical).await.unwrap(), Some(5));
    }

    #[tokio::test]
    async fn attribute_schema_excludes_geometry_and_the_covering_helper_column() {
        let backend = backend();
        let physical = &backend.collections().await.unwrap()[0];
        let columns = backend.attribute_schema(physical).await.unwrap().unwrap();
        let names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["name", "value"]);
        assert_eq!(columns[0].sql_type, "text");
        assert_eq!(columns[1].sql_type, "bigint");
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
    async fn items_with_a_bbox_prunes_row_groups_and_returns_only_matching_features() {
        let backend = backend();
        // Covers only the western half of the fixture's extent.
        let query = ItemsQuery {
            bbox: Some([-5.0, 45.0, -1.0, 55.0]),
            ..ItemsQuery::default()
        };
        let page = backend.items(&decl(), &query).await.unwrap();
        assert!(!page.features_geojson.is_empty());
        assert!(page.features_geojson.len() < 5);
        assert_eq!(
            page.number_matched,
            Some(page.features_geojson.len() as u64),
            "the tiny fixture fits in one row group, so number_matched equals what's returned"
        );
        for feature in &page.features_geojson {
            let x = feature["geometry"]["coordinates"][0].as_f64().unwrap();
            assert!(x <= -1.0, "feature outside the requested bbox: {feature}");
        }
    }

    #[tokio::test]
    async fn item_looks_up_a_feature_by_its_row_position_id() {
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
    /// — CQL2 filtering is out of scope for this lane, same as
    /// `filter_capable` staying `false`.
    #[test]
    fn cql2_conformance_classes_stays_empty() {
        let backend = backend();
        let declared = FeatureSource::cql2_conformance_classes(&backend);
        assert!(declared.is_empty());
        assert_eq!(
            FeatureSource::filter_capable(&backend),
            !declared.is_empty()
        );
    }
}
