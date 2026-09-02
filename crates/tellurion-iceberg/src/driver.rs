//! The `iceberg` `DriverFactory`, and the `CatalogSource` + `FeatureSource`
//! implementation backing it. Read-only: a table is opened for reading
//! only, there is no write path, no DDL, nothing beyond what the driver
//! contract's mandatory `CatalogSource` plus the optional `FeatureSource`
//! capability require. `TileSource` is never implemented — a collection
//! routed to an `iceberg` storage on the tiles lane fails the router's
//! ordinary missing-capability check, exactly like every other driver in
//! this workspace that doesn't claim that capability.
//!
//! ## Table access
//!
//! This driver never opens a table directly off a filesystem path. Every
//! table is looked up through a real Iceberg REST catalog (the
//! `iceberg-catalog-rest` crate's `RestCatalog`, speaking the standard
//! `GET /v1/config` + `GET /v1/namespaces/{ns}/tables/{table}` protocol) —
//! no SQL catalog, no AWS/Glue, no Postgres client of its own. The
//! catalog's `warehouse` property is never set: every `LoadTableResult`
//! response embeds a `metadata-location`, which is all this driver's own
//! `FileIO` needs to resolve every subsequent read (manifests, data
//! files). The catalog object itself is transient: it exists only long
//! enough to resolve one `load_table` call in [`open_rest_table`]; every
//! read after that goes through the returned `Table`'s own `file_io()`.
//!
//! ## Storage backends (`FileIO`)
//!
//! That `FileIO` is this crate's own `fileio::ObjectStoreStorage` (`#123`),
//! which serves the local filesystem (delegated unchanged to `iceberg`'s
//! `LocalFsStorage`) and anything speaking the **S3 protocol** — AWS S3,
//! MinIO, Ceph RGW, R2 — over `tellurion_core`'s existing `S3ObjectStore`
//! and its hand-rolled SigV4 signer, rather than a second S3 client of its
//! own. GCS and ADLS are **not** implemented and are refused BY NAME at
//! table load by [`require_supported_storage`] — the scheme is named, the
//! product is named, and nothing falls back to another backend. See
//! `fileio.rs`'s crate docs for the full reasoning and `location.rs` for
//! the four `s3_*` locator declarations an S3-backed table needs.
//!
//! ## Snapshot pinning
//!
//! The table (and therefore its current snapshot, schema, and manifest
//! list) is loaded exactly once per backend lifetime (`tokio::sync::
//! OnceCell`, the same deferred-open pattern `tellurion-geoparquet` uses for
//! its file header) and cached for the backend's lifetime. A commit made to
//! the table after this driver has already loaded is invisible to every
//! subsequent call on this backend instance — there is no re-read, no
//! staleness check, nothing to invalidate. That is the pin: every fact this
//! driver ever reports, and every file a scan ever plans, is consistent
//! with the one snapshot observed at load time.
//!
//! ## Snapshot-pinned paging
//!
//! [`IcebergBackend::items_inner`]/[`IcebergBackend::item_inner`] hand every
//! caller an opaque paging token shaped
//! `"<snapshot-id>:<filter-fingerprint>:<row-offset>"` (see
//! [`encode_token`]/[`decode_token`]) rather than a bare offset. Because
//! this backend never re-opens the table, a token it minted can only ever
//! be replayed against the same pinned snapshot it was minted from — but
//! encoding the snapshot id explicitly, and refusing a token that names a
//! different one (`IcebergDriverError::TokenSnapshotMismatch`), makes that
//! guarantee checked rather than merely accidental: a token surviving a
//! process restart (or handed to a different backend instance entirely)
//! that has since observed a newer commit is refused outright instead of
//! silently being replayed against the wrong file list. The filter
//! fingerprint ([`query_predicate_fingerprint`]) extends the identical
//! guarantee to whatever CQL2 filter/datetime interval the token's page was
//! read under: a token minted while paging under filter A, replayed against
//! filter B (or no filter at all), is refused with
//! `IcebergDriverError::TokenFilterMismatch` rather than silently resuming
//! at a row offset that means something different under the new filter.
//!
//! ## Declared geometry and bbox columns
//!
//! Iceberg has no native geometry type. The geometry column (WKB bytes) and
//! its four covering bbox columns are pure operator declarations carried in
//! the storage's `url_env` locator (see `location.rs`); at load time this
//! driver checks each declared column exists in the pinned snapshot's schema
//! and has the expected type (`Binary` for geometry, `Double` or `Float` for
//! each bbox column) — a missing or wrong-typed column is refused with a
//! precise `Error::Config` naming the collection, the column, and (for a
//! type mismatch) the actual type found, never silently accepted or
//! papered over.
//!
//! ## pk / cursor mapping
//!
//! Like `flatgeobuf`/`geoparquet`, Iceberg has no relational primary key
//! column this driver can point at: a feature is addressable only by its
//! position in a deterministic, driver-chosen file order (see "Planned-file
//! cache" below). This driver reports the same synthetic
//! [`PRIMARY_KEY_FIELD`] (`"fid"`) those two drivers use for the identical
//! reason, and uses that same flat position, uniformly, as both the
//! GeoJSON `id` and the keyset paging cursor. That flat position is defined
//! once, over the entire table in its deterministic file order, with no
//! bbox/filter/datetime pruning applied — a physical row has exactly one
//! id, and it is the same id on every read surface: an unfiltered `items`
//! page, a `bbox`/CQL2-filtered/`datetime`-filtered `items` page, and
//! `item` with or without a filter of its own all number that row
//! identically. An id harvested from any listing always round-trips
//! through `item` to the same row. See "CQL2 and datetime pushdown" below
//! for how a filtered `items` page upholds this.
//!
//! ## Planned-file cache
//!
//! Resolving which data files a scan touches is real network I/O against
//! the REST catalog's backing storage — a manifest list, then every
//! manifest it references. [`IcebergBackend::planned_tasks`] caches that
//! plan (a `Vec<iceberg::scan::FileScanTask>`, keyed by the query bbox
//! *plus* [`query_predicate_fingerprint`] of whatever CQL2 filter/datetime
//! interval narrowed this scan — see "CQL2 and datetime pushdown" below)
//! the same shape `tellurion-core::router`'s own `descriptor_cache` caches a
//! derived `CollectionDescriptor`: a `moka::future::Cache` bounded by
//! [`PLAN_CACHE_CAPACITY`], with staleness checked manually against
//! [`IcebergLocation::plan_cache_ttl_s`] rather than relying on moka's own
//! per-entry TTL — same shape, not the same mechanism. Given this driver's
//! own "Snapshot pinning" above, a cached plan for a pinned backend can
//! never actually go stale (the snapshot it was planned against never
//! changes for this backend's lifetime); the TTL here is deliberately kept
//! anyway, both to match the shape this workspace already establishes for
//! a derived-and-cached backend fact, and so a future slice that adds
//! snapshot refresh doesn't inherit a cache with no staleness concept at
//! all. `max_capacity` is what actually bounds this cache's memory today.
//! The returned `Vec<FileScanTask>` is always sorted by `data_file_path`
//! before caching — `TableScan::plan_files()` itself gives no ordering
//! guarantee across manifest entries (they're processed concurrently), and
//! this driver's own cursor above depends on a stable order surviving a
//! cache miss. A plan cached for filter A is never served to a request
//! carrying filter B: two different fingerprints are always two different
//! cache keys, even at the identical bbox.
//!
//! ## CQL2 and datetime pushdown
//!
//! [`FeatureSource::filter_capable`] reports `true`: [`compile_predicate`]
//! compiles a `tellurion_core::Filter` into an `iceberg::expr::Predicate`
//! over comparison (`=`, `<>`, `<`, `>`, `<=`, `>=`), `IS [NOT] NULL`,
//! `[NOT] IN`, and `AND`/`OR`/`NOT` on the table's own scalar (boolean,
//! integer, floating-point, or string) columns — the subset `iceberg-rust`'s
//! `Predicate`/`Datum` can express faithfully without inventing a coercion
//! rule the crate doesn't already define. Every other CQL2 construct —
//! `LIKE`/`BETWEEN`/`CASEI`, every spatial predicate (they'd target the WKB
//! geometry column, which has no scalar comparison), and every temporal
//! function predicate (`T_AFTER`/`T_BEFORE`/`T_DURING`/the rest) — is a
//! clean, named [`IcebergDriverError::FilterPropertyUnsupported`] refusal,
//! never a silent drop or a partial pushdown that quietly serves unfiltered
//! rows in its place. [`compile_datetime_predicate`] compiles the standard
//! `datetime` interval query parameter the same way, against this
//! collection's declared `datetime` column (`CollectionDecl::datetime` —
//! Iceberg itself offers no derivable candidate, see "Derived facts" below,
//! so an operator must declare one explicitly), refusing with
//! [`IcebergDriverError::NoDatetimeColumn`] when none is declared and with
//! [`IcebergDriverError::DatetimeColumnWrongType`] when the declared column
//! isn't `timestamptz` (an offset-naive `timestamp` column has no honest
//! way to compare against an RFC 3339 instant that always carries an
//! offset). Both functions still run unconditionally on every `items` call
//! that carries a filter/datetime interval, purely to validate it through
//! this one boundary — but neither compiled `Predicate` is ever pushed into
//! an `items` scan any more (see the next two paragraphs for why).
//!
//! `item` (single-feature lookup) addresses a row by flat position over the
//! *entire* deterministic file order, so pushing a value-based predicate
//! into that same scan would change what "position N" means. It keeps the
//! lookup itself unfiltered — the exact same unfiltered, un-bboxed,
//! whole-table plan `planned_tasks(None, None, ...)` always returns — and
//! evaluates its own optional filter in-process against the one fetched
//! row's own decoded attribute values, via [`evaluate_predicate`]. That
//! function walks the identical `Filter` tree shape `compile_predicate`
//! does, through the same `scalar_field_type`/`literal_datum`/
//! construct-refusal helpers, so the two always agree: a filter `items`
//! accepts is always accepted here too, and one `items` refuses by name is
//! refused here with the identical error. `evaluate_predicate` returns a
//! three-valued (`Option<bool>`) result so `AND`/`OR`/`NOT` compose under
//! SQL's own three-valued logic — a comparison against a `NULL` column
//! value is "unknown", never silently `false`. A row that exists but that
//! the filter excludes comes back `Ok(None)`, indistinguishable from a
//! genuinely absent position (see `FeatureSource::item`'s own contract, and
//! `tellurion-postgis`'s matching "found but filtered looks like not-found"
//! behavior for its own single-item lookup).
//!
//! `items` under a `bbox`-only query (no CQL2 filter, no `datetime`
//! interval) pushes `bbox` into `TableScan::with_filter`, so Iceberg's own
//! manifest evaluator prunes whole data files by column stats before any
//! Parquet read, and `iceberg::arrow::ArrowReader` applies genuine
//! row-level filtering (a Parquet `RowFilter` built from the same
//! predicate) while reading the files that remain — the pre-`#45` shape,
//! unchanged. But as soon as a CQL2 `filter` or `datetime` interval
//! narrows an `items` call, pushing *any* predicate — `bbox` included —
//! into that scan would prune files and discard non-matching rows before
//! this driver ever sees them, which is exactly what breaks the id-space
//! invariant "pk / cursor mapping" above describes: a pruned file's rows
//! never contribute to any later row's cumulative position, and a
//! Parquet-filtered-away row's own position is never observable at all, so
//! neither the file nor the row a `Predicate` push discards can be told
//! apart from one that was never there. So a `filter`/`datetime`-active
//! `items` call reads the exact same unfiltered, un-bboxed plan `item`
//! itself always reads, and decides `bbox`/`filter`/`datetime` in-process
//! per row via [`IcebergBackend::row_matches_query`] — `bbox` through
//! [`row_in_bbox`] (the row-level counterpart to [`bbox_predicate`]),
//! `filter` through the identical `evaluate_predicate` `item` uses, and
//! `datetime` through [`evaluate_datetime_range`] (the row-level
//! counterpart to `compile_datetime_predicate`) — composed under the same
//! three-valued logic. A row's position in that walk, whether or not it
//! matches, is exactly the flat position `item` would use to address it;
//! only a match is ever emitted or counted against `limit`. This means a
//! filtered `items` page never benefits from Iceberg's own manifest/
//! row-group pruning or its Parquet `RowFilter` — it always walks every row
//! of every planned file — trading that efficiency for the guarantee that
//! an id it hands out always means the same row on every other read
//! surface; see "Reading rows" below for the resulting cost.
//!
//! ## Reading rows
//!
//! `items`/`item` read through [`IcebergBackend::planned_tasks`]'s cached
//! plan and feed the files actually needed for this page into
//! `iceberg::arrow::ArrowReaderBuilder` — the crate's own Arrow/Parquet
//! reader, not a hand-rolled one. When no CQL2 filter/datetime interval
//! narrows this call, [`IcebergBackend::read_window`] still skips whole
//! files cheaply using each task's own `record_count` (never re-touching
//! the manifest for a page that starts past the first few files), and
//! `number_matched` is the exact sum of every planned task's `record_count`
//! when all of them report one; `None` otherwise, never a guess. Once a
//! filter/datetime interval is active, `tasks` is always the full,
//! unfiltered, un-bboxed plan (see "CQL2 and datetime pushdown" above), so
//! the same `record_count`-based file skip stays sound for locating the
//! *start* of a resumed page — every task in that plan really does hold
//! exactly `record_count` rows, none pruned — but `number_matched` is still
//! always `None` (an exact match count would require reading every row,
//! which this driver only does when a page is actually requested, never
//! just to report a count) and the window walk visits every row of every
//! task from the start position onward, testing each one via
//! `row_matches_query`; only a match advances `features`/`limit`, but
//! *every* row — matching or not — advances the position counter that
//! becomes its neighbors' cursor and, for a match, its own `id`. This makes
//! a deep page under a filter cost re-reading every file before it on each
//! call, and reading every row of every remaining file even when few
//! match — a known, deliberate cost of the id-space guarantee (see the
//! crate's outstanding work notes) rather than a silently wrong answer.
//!
//! ## Derived facts
//!
//! - `row_estimate`: the pinned snapshot's own summary property
//!   `"total-records"` (the Iceberg spec's standard snapshot-summary key),
//!   parsed as `u64`. `None` when absent or unparseable — never guessed from
//!   a manifest scan.
//! - `extent`: folded from every *live* (non-deleted) data file's
//!   `lower_bounds`/`upper_bounds` on the four declared bbox columns' field
//!   ids, read straight from the pinned snapshot's manifests (no row data
//!   touched). If even one live data file is missing bounds for any of the
//!   four columns (stats disabled at write time, say), the whole derivation
//!   gives up and reports `None` rather than a bbox that silently excludes
//!   that file's actual coverage — an extent this driver reports is always
//!   the true union, never a partial one.
//! - `attribute_schema`: every schema field except the declared geometry and
//!   bbox columns, with each Iceberg primitive type mapped to a conservative
//!   SQL-flavored name (same spirit as `tellurion-geoparquet`'s
//!   `arrow_type_to_sql`); nested types (struct/list/map) fall back to a
//!   lowercased `Display` rendering rather than failing the whole call.
//! - `srid`: always `None`. Nothing in Iceberg table metadata declares a
//!   coordinate reference system for a WKB column, and this slice's
//!   locator has no CRS declaration either — inventing one would violate
//!   the "never invent facts" rule as much as inventing an extent would.
//!
//! ## What this slice does not do
//!
//! `crs_capable` stays at its default `false` — reprojection is out of
//! scope for this slice, same posture as `flatgeobuf`/`geoparquet`. CQL2
//! spatial predicates against the WKB geometry column are refused (see
//! "CQL2 and datetime pushdown" above) — bbox handling is unchanged from
//! the prior slice, on both `items` and `item`. The standard `datetime`
//! interval query parameter has no `item`-level equivalent: OGC API
//! Features never defines a `datetime` parameter on a single-feature
//! lookup, so `compile_datetime_predicate` stays an `items`-only path —
//! `item`'s own filter evaluation covers the `#34` ABAC grant filter only.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arrow_array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Float32Array, Float64Array, Int16Array, Int32Array,
    Int64Array, Int8Array, LargeBinaryArray, LargeStringArray, RecordBatch, StringArray,
    TimestampMicrosecondArray, UInt16Array, UInt32Array, UInt64Array, UInt8Array,
};
use arrow_schema::DataType;
use async_trait::async_trait;
use futures::TryStreamExt;
use iceberg::arrow::ArrowReaderBuilder;
use iceberg::expr::{Predicate, Reference};
use iceberg::scan::{FileScanTask, FileScanTaskStream};
use iceberg::spec::{
    DataContentType, DataFile, Datum, PrimitiveLiteral, PrimitiveType, Schema, SchemaRef, Snapshot,
    SnapshotRef, TableMetadataRef, Type,
};
use iceberg::table::Table;
use iceberg::{Catalog, CatalogBuilder, NamespaceIdent, TableIdent};
use iceberg_catalog_rest::{RestCatalogBuilder, REST_CATALOG_PROP_URI};
use tokio::sync::OnceCell;

use tellurion_core::{
    AttributeColumn, CatalogSource, CollectionDecl, CompareOp, DatetimeRange, DriverFactory,
    FeaturePage, FeatureSource, Filter, ItemsQuery, Literal, PhysicalCollection,
    Result as CoreResult, SpatialExtent, SpatialOp, StorageDecl, StorageDriver, TemporalOp,
};

use crate::error::{IcebergDriverError, Result};
use crate::fileio::{ObjectStoreStorageFactory, S3Connection, StorageRoute};
use crate::location::{BboxColumns, IcebergLocation};

/// The Iceberg spec's standard snapshot-summary property naming the total
/// live row count as of that snapshot. Not every writer populates it (it is
/// optional per spec), hence `row_estimate`'s honest `None` fallback.
const TOTAL_RECORDS_SUMMARY_KEY: &str = "total-records";

/// Synthetic feature-index primary key name — see this module's "pk /
/// cursor mapping" docs. Matches `flatgeobuf`'s/`geoparquet`'s own `fid`
/// convention for the identical "no native key" situation.
const PRIMARY_KEY_FIELD: &str = "fid";

/// Default TTL for [`IcebergBackend::plan_cache`] entries, in seconds — the
/// same default `tellurion-core::config::DEFAULT_DESCRIPTOR_TTL_S` uses for
/// the descriptor cache this driver's own plan cache is modeled on. See
/// `location.rs`'s `plan_cache_ttl_s` for how an operator overrides it.
pub(crate) const DEFAULT_PLAN_CACHE_TTL_S: u64 = 300;

/// Upper bound on distinct bbox keys [`IcebergBackend::plan_cache`] holds
/// for one backend at once — a plain memory bound, not a correctness
/// concern (see this module's "Planned-file cache" docs).
const PLAN_CACHE_CAPACITY: u64 = 256;

/// Registers the `iceberg` driver.
#[derive(Default)]
pub struct IcebergDriverFactory;

impl IcebergDriverFactory {
    pub fn new() -> Self {
        Self
    }
}

impl DriverFactory for IcebergDriverFactory {
    fn name(&self) -> &str {
        "iceberg"
    }

    fn build(&self, decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        let raw = std::env::var(&decl.url_env).map_err(|_| IcebergDriverError::MissingEnvVar {
            storage: decl.id.clone(),
            var: decl.url_env.clone(),
        })?;
        let location = IcebergLocation::parse(&raw)?;
        Ok(Arc::new(IcebergDriverImpl {
            backend: Arc::new(IcebergBackend::new(location)),
        }))
    }
}

struct IcebergDriverImpl {
    backend: Arc<IcebergBackend>,
}

impl StorageDriver for IcebergDriverImpl {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::clone(&self.backend) as Arc<dyn CatalogSource>
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn FeatureSource>)
    }

    // `tile_source`/`volume_source`/`write_sink`/`outbox_source`: all
    // default `None` — this slice never claims those capabilities, see
    // this module's crate docs.
    // `validate_collection`: default accepts everything — a REST catalog
    // URL plus a bare table name has no backend-specific identifier syntax
    // beyond `AppConfig::validate`'s referential-integrity pass.
    // `capacity_hint`: default `None` — this driver holds no connection
    // pool of its own (a fresh, short-lived REST client per table load),
    // same rationale as `tellurion-geoparquet`'s own driver.
}

/// Everything derived from the pinned snapshot, cached once per backend
/// lifetime — see this module's "Snapshot pinning" docs.
struct CachedTable {
    /// The opened table, frozen at whatever metadata was current at load
    /// time. Scans always pin `snapshot_id` explicitly rather than relying
    /// on `table.metadata().current_snapshot()`, but freezing the whole
    /// table (not just the id) is what makes a later commit invisible even
    /// if a future change to this driver ever adds a call that reads "the
    /// current snapshot" without pinning.
    table: Table,
    snapshot_id: i64,
    /// The pinned snapshot's own schema — `snapshot.schema(&table_metadata)`
    /// at load time, stored rather than re-derived so
    /// [`compile_predicate`]/[`compile_datetime_predicate`] always compile
    /// against the exact schema every other derived fact here was computed
    /// from, never a second, independently-fetched copy.
    schema: SchemaRef,
    row_estimate: Option<u64>,
    extent: Option<[f64; 4]>,
    attributes: Vec<AttributeColumn>,
}

/// One cached scan plan — see this module's "Planned-file cache" docs.
#[derive(Clone)]
struct CachedPlan {
    tasks: Arc<Vec<FileScanTask>>,
    computed_at: Instant,
}

impl CachedPlan {
    fn is_stale(&self, ttl: Duration) -> bool {
        self.computed_at.elapsed() >= ttl
    }
}

pub(crate) struct IcebergBackend {
    location: IcebergLocation,
    cached: OnceCell<Arc<CachedTable>>,
    plan_cache: moka::future::Cache<String, CachedPlan>,
    plan_cache_ttl: Duration,
}

impl IcebergBackend {
    pub(crate) fn new(location: IcebergLocation) -> Self {
        let plan_cache_ttl = Duration::from_secs(location.plan_cache_ttl_s);
        Self {
            location,
            cached: OnceCell::new(),
            plan_cache: moka::future::Cache::builder()
                .max_capacity(PLAN_CACHE_CAPACITY)
                .build(),
            plan_cache_ttl,
        }
    }

    async fn table(&self) -> Result<Arc<CachedTable>> {
        let cached = self
            .cached
            .get_or_try_init(|| async { load_cached_table(&self.location).await.map(Arc::new) })
            .await?;
        Ok(Arc::clone(cached))
    }

    /// Returns the cached (or freshly planned) list of data-file scan tasks
    /// for `bbox` AND-combined with `predicate` (already-compiled CQL2
    /// filter/datetime interval, if any) — see this module's "Planned-file
    /// cache" docs. `fingerprint` (`query_predicate_fingerprint(filter,
    /// datetime)`) must be the exact fingerprint `predicate` was compiled
    /// from; it is never derived from `predicate` itself (an
    /// `iceberg::expr::Predicate` carries no stable identity of its own to
    /// hash) — every caller computes both from the same request in one
    /// place, see `items_inner`.
    async fn planned_tasks(
        &self,
        bbox: Option<[f64; 4]>,
        predicate: Option<Predicate>,
        fingerprint: u64,
    ) -> Result<Arc<Vec<FileScanTask>>> {
        let key = plan_cache_key(bbox, fingerprint);
        if let Some(cached) = self.plan_cache.get(&key).await {
            if !cached.is_stale(self.plan_cache_ttl) {
                return Ok(Arc::clone(&cached.tasks));
            }
        }

        let cached_table = self.table().await?;
        let mut builder = cached_table
            .table
            .scan()
            .snapshot_id(cached_table.snapshot_id)
            .select_all();
        let mut combined = bbox.map(|query_bbox| bbox_predicate(&self.location.bbox, query_bbox));
        if let Some(extra) = predicate {
            combined = Some(match combined {
                Some(existing) => existing.and(extra),
                None => extra,
            });
        }
        if let Some(combined) = combined {
            builder = builder.with_filter(combined);
        }
        let scan = builder.build().map_err(IcebergDriverError::Iceberg)?;
        let mut stream = scan
            .plan_files()
            .await
            .map_err(IcebergDriverError::Iceberg)?;

        let mut tasks = Vec::new();
        while let Some(task) = stream
            .try_next()
            .await
            .map_err(IcebergDriverError::Iceberg)?
        {
            tasks.push(task);
        }
        // Deterministic order — see this module's "Planned-file cache" docs.
        tasks.sort_by(|a, b| a.data_file_path.cmp(&b.data_file_path));

        let tasks = Arc::new(tasks);
        self.plan_cache
            .insert(
                key,
                CachedPlan {
                    tasks: Arc::clone(&tasks),
                    computed_at: Instant::now(),
                },
            )
            .await;
        Ok(tasks)
    }

    /// Lists the data files the pinned snapshot's scan plans, optionally
    /// pruned to `bbox` via the four declared bbox columns and/or a compiled
    /// CQL2 `filter` — the path/record-count projection of
    /// [`Self::planned_tasks`]'s cached plan this crate's own tests exercise
    /// directly. Nothing outside this crate's own tests calls it —
    /// `items`/`item` read `planned_tasks`'s `FileScanTask`s directly, since
    /// they need more than a path and a count.
    #[cfg(test)]
    pub(crate) async fn plan_files(
        &self,
        bbox: Option<[f64; 4]>,
        filter: Option<&Filter>,
    ) -> Result<Vec<PlannedFile>> {
        let cached_table = self.table().await?;
        let predicate = filter
            .map(|f| compile_predicate(f, &cached_table.schema, &self.location))
            .transpose()?;
        let fingerprint = query_predicate_fingerprint(filter, None);
        let tasks = self.planned_tasks(bbox, predicate, fingerprint).await?;
        Ok(tasks
            .iter()
            .map(|task| PlannedFile {
                path: task.data_file_path.clone(),
                record_count: task.record_count,
            })
            .collect())
    }

    async fn items_inner(
        &self,
        collection: &CollectionDecl,
        query: &ItemsQuery,
    ) -> Result<FeaturePage> {
        let cached_table = self.table().await?;

        // Compiled purely to validate `filter`/`datetime` through the same
        // construct/property/type boundary `item`'s own filter evaluation
        // uses — neither `Predicate` is pushed into a scan here any more;
        // see this module's "CQL2 and datetime pushdown" docs for why.
        if let Some(filter) = &query.filter {
            compile_predicate(filter, &cached_table.schema, &self.location)?;
        }
        if let Some(range) = &query.datetime {
            compile_datetime_predicate(collection, &cached_table.schema, range)?;
        }
        let predicate_active = query.filter.is_some() || query.datetime.is_some();
        let fingerprint =
            query_predicate_fingerprint(query.filter.as_ref(), query.datetime.as_ref());

        let window_start = match query.token.as_deref() {
            Some(token) => decode_token(token, cached_table.snapshot_id, fingerprint)?,
            None => 0,
        };
        // A `predicate_active` page's cursor lives in the exact same
        // position space `item` always addresses — the fully unfiltered,
        // un-bboxed plan — never a bbox/filter-pruned one, so a row's id
        // means the same thing on every read surface (this module's "pk /
        // cursor mapping" docs). Reusing `query_predicate_fingerprint(None,
        // None)` here also means this always lands on the exact plan-cache
        // entry `item` itself already populates.
        let tasks = if predicate_active {
            let unfiltered_fingerprint = query_predicate_fingerprint(None, None);
            self.planned_tasks(None, None, unfiltered_fingerprint)
                .await?
        } else {
            self.planned_tasks(query.bbox, None, fingerprint).await?
        };
        self.read_window(&cached_table, collection, &tasks, query, window_start)
            .await
    }

    async fn item_inner(
        &self,
        id: &str,
        filter: Option<&Filter>,
    ) -> Result<Option<serde_json::Value>> {
        let cached_table = self.table().await?;
        if let Some(filter) = filter {
            // Validates `filter` against the exact same construct/property/
            // type boundary `items` compiles through — reusing
            // `compile_predicate` itself rather than a parallel check, so a
            // filter `items` accepts is always accepted here too, and one it
            // refuses is refused with the identical named error. Checked
            // unconditionally, even for a non-existent or non-integer id, so
            // a filter this driver can never evaluate is always a clean
            // refusal rather than a sometimes-error depending on whether a
            // row happened to exist. The resulting `Predicate` is discarded
            // — it is never pushed into a scan here (see this module's "CQL2
            // and datetime pushdown" docs) — only its validation matters.
            compile_predicate(filter, &cached_table.schema, &self.location)?;
        }
        let Ok(target) = id.parse::<u64>() else {
            // A non-integer id can never match this driver's flat-position
            // identity — same "honest None" convention flatgeobuf/
            // geoparquet apply to a non-integer id.
            return Ok(None);
        };
        // Same unfiltered, whole-table order `items(bbox: None)` uses — an
        // id lookup has no bbox context of its own, and `filter` is never
        // pushed into this scan.
        let fingerprint = query_predicate_fingerprint(None, None);
        let tasks = self.planned_tasks(None, None, fingerprint).await?;
        self.read_single_row(&cached_table, &tasks, target, filter)
            .await
    }

    /// Reads up to `query.limit` features starting at global row
    /// `window_start` (0-based, over `tasks`' deterministic order) — the
    /// backing read for `items`. `next_token` embeds `fingerprint`
    /// (`query_predicate_fingerprint(filter, datetime)`) alongside the
    /// pinned snapshot id — see `encode_token`.
    ///
    /// `tasks` is always exactly the plan `window_start`/a row's own id are
    /// defined against: the bbox-pruned plan when `predicate_active` is
    /// `false` (the pre-`#45` shape, unchanged), or the fully unfiltered,
    /// un-bboxed plan `item` itself always reads when it's `true` — see
    /// `items_inner` and this module's "CQL2 and datetime pushdown" docs.
    /// Either way every task in `tasks` really does hold exactly its own
    /// `record_count` rows (none pruned out from under it), so
    /// [`locate_unfiltered_start`]'s cheap, `record_count`-based file skip
    /// is always sound for locating the *start* row of a resumed page,
    /// `predicate_active` or not.
    ///
    /// From that start row onward this walks every row of every remaining
    /// task in order. When `predicate_active` is `false`, every row visited
    /// is emitted unconditionally (matching the pre-`#45` shape exactly),
    /// and `number_matched` is the exact sum of every planned task's
    /// `record_count` when all of them report one, `None` otherwise, never
    /// a guess. When `predicate_active` is `true`, [`Self::row_matches_query`]
    /// decides in-process, per row, whether `query`'s bbox/filter/datetime
    /// actually admits it; only a match is pushed into `features` or counted
    /// against `query.limit`, and `number_matched` is always `None` (an
    /// exact match count would require reading every row just to report a
    /// count, which this driver only does when a page is actually
    /// requested). Either way, `consumed` — the position that becomes a
    /// row's own `id` when it matches, and the next page's resume point
    /// regardless — advances for *every* row visited, never just the
    /// matches: that's what keeps a row's id equal to its true flat
    /// position instead of a running count of matches seen so far.
    async fn read_window(
        &self,
        cached_table: &CachedTable,
        collection: &CollectionDecl,
        tasks: &[FileScanTask],
        query: &ItemsQuery,
        window_start: u64,
    ) -> Result<FeaturePage> {
        let limit = query.limit as usize;
        let predicate_active = query.filter.is_some() || query.datetime.is_some();
        let fingerprint =
            query_predicate_fingerprint(query.filter.as_ref(), query.datetime.as_ref());
        let number_matched = if predicate_active {
            None
        } else {
            tasks
                .iter()
                .map(|task| task.record_count)
                .collect::<Option<Vec<u64>>>()
                .map(|counts| counts.into_iter().sum())
        };

        let (start_task_idx, cumulative) = locate_unfiltered_start(tasks, window_start);

        if start_task_idx >= tasks.len() {
            return Ok(FeaturePage {
                features_geojson: Vec::new(),
                number_matched,
                next_token: None,
            });
        }

        let selected: Vec<FileScanTask> = tasks[start_task_idx..].to_vec();
        let file_io = cached_table.table.file_io().clone();
        let task_stream: FileScanTaskStream =
            Box::pin(futures::stream::iter(selected.into_iter().map(Ok)));
        let mut batches = ArrowReaderBuilder::new(file_io)
            .build()
            .read(task_stream)
            .map_err(IcebergDriverError::Iceberg)?;

        let mut skip_remaining = window_start - cumulative;
        let mut features = Vec::with_capacity(limit.min(1024));
        let mut consumed: u64 = window_start;
        let mut has_more = false;

        'outer: while let Some(batch) = batches
            .try_next()
            .await
            .map_err(IcebergDriverError::Iceberg)?
        {
            let rows = batch.num_rows();
            let mut row = 0usize;
            if skip_remaining > 0 {
                let skip_here = skip_remaining.min(rows as u64) as usize;
                row = skip_here;
                skip_remaining -= skip_here as u64;
            }
            while row < rows {
                let matched = if predicate_active {
                    self.row_matches_query(collection, &cached_table.schema, query, &batch, row)?
                } else {
                    true
                };
                if matched {
                    if features.len() >= limit {
                        has_more = true;
                        break 'outer;
                    }
                    features.push(feature_to_geojson(&batch, &self.location, row, consumed)?);
                }
                consumed += 1;
                row += 1;
            }
        }

        let next_token =
            has_more.then(|| encode_token(cached_table.snapshot_id, fingerprint, consumed));
        Ok(FeaturePage {
            features_geojson: features,
            number_matched,
            next_token,
        })
    }

    /// In-process test of one decoded row against `query`'s `bbox`, CQL2
    /// `filter`, and `datetime` interval together — the row-level
    /// counterpart to what a bbox-only `items` call pushes down as a real
    /// `Predicate` (see this module's "CQL2 and datetime pushdown" docs),
    /// used instead whenever a filter or datetime interval is active so
    /// every row `read_window`'s `tasks` plans is actually visited, not
    /// just the ones a scan-level predicate would have let through.
    /// Composes all three constraints under SQL's own three-valued logic —
    /// the same `Tri`/`tri_and` machinery [`evaluate_predicate`] itself
    /// uses — so a `NULL` bbox/filter/datetime column excludes a row for
    /// the same reason it would if pushed into a real Iceberg scan, never a
    /// silent `true`.
    fn row_matches_query(
        &self,
        collection: &CollectionDecl,
        schema: &Schema,
        query: &ItemsQuery,
        batch: &RecordBatch,
        row: usize,
    ) -> Result<bool> {
        let mut acc: Tri = Some(true);
        if let Some(bbox) = query.bbox {
            let in_bbox = row_in_bbox(&self.location.bbox, batch, row, bbox)?;
            acc = tri_and(acc, Some(in_bbox));
        }
        if let Some(filter) = &query.filter {
            acc = tri_and(
                acc,
                evaluate_predicate(filter, schema, &self.location, batch, row)?,
            );
        }
        if let Some(range) = &query.datetime {
            acc = tri_and(
                acc,
                evaluate_datetime_range(collection, schema, range, batch, row)?,
            );
        }
        Ok(acc == Some(true))
    }

    /// Reads the single row at flat position `target` (0-based, over
    /// `tasks`' deterministic order — the same unfiltered whole-table order
    /// `item_inner` always scans) and, when `filter` is `Some`, evaluates it
    /// in-process against that one row's own decoded attribute values via
    /// [`evaluate_predicate`] — see this module's "CQL2 and datetime
    /// pushdown" docs for why `item` never pushes a filter into the scan
    /// itself. `filter` has already passed the same construct/property/type
    /// boundary `compile_predicate` enforces (`item_inner` validates that
    /// unconditionally before calling here); this only evaluates the
    /// *value* semantics against the row actually found at `target`.
    /// Returns `Ok(None)` both when `target` names no row at all and when a
    /// row exists but `filter` excludes it — the two are indistinguishable
    /// on purpose, matching `tellurion-postgis`'s own single-item lookup
    /// (see `FeatureSource::item`'s contract).
    ///
    /// Locates the starting task the same cheap, `record_count`-based way
    /// `read_window`'s own unfiltered walk does (via
    /// [`locate_unfiltered_start`]) rather than sharing `read_window`
    /// itself — `item`'s job (one exact position, stop at the first
    /// candidate regardless of whether it matches `filter`) is meaningfully
    /// different from `items`' windowed, multi-row, tokened paging, and
    /// keeping them separate leaves `read_window`'s own well-tested path
    /// untouched.
    async fn read_single_row(
        &self,
        cached_table: &CachedTable,
        tasks: &[FileScanTask],
        target: u64,
        filter: Option<&Filter>,
    ) -> Result<Option<serde_json::Value>> {
        let (start_task_idx, cumulative) = locate_unfiltered_start(tasks, target);
        if start_task_idx >= tasks.len() {
            return Ok(None);
        }

        let selected: Vec<FileScanTask> = tasks[start_task_idx..].to_vec();
        let file_io = cached_table.table.file_io().clone();
        let task_stream: FileScanTaskStream =
            Box::pin(futures::stream::iter(selected.into_iter().map(Ok)));
        let mut batches = ArrowReaderBuilder::new(file_io)
            .build()
            .read(task_stream)
            .map_err(IcebergDriverError::Iceberg)?;

        let mut skip_remaining = target - cumulative;
        while let Some(batch) = batches
            .try_next()
            .await
            .map_err(IcebergDriverError::Iceberg)?
        {
            let rows = batch.num_rows() as u64;
            if skip_remaining >= rows {
                skip_remaining -= rows;
                continue;
            }
            let row = skip_remaining as usize;
            if let Some(filter) = filter {
                let matched =
                    evaluate_predicate(filter, &cached_table.schema, &self.location, &batch, row)?
                        == Some(true);
                if !matched {
                    return Ok(None);
                }
            }
            return Ok(Some(feature_to_geojson(
                &batch,
                &self.location,
                row,
                target,
            )?));
        }
        Ok(None)
    }
}

/// Locates the task index and cumulative row count to start an unfiltered,
/// whole-table walk at global row `window_start` (0-based) — the cheap,
/// `record_count`-based file skip this module's "Reading rows" docs
/// describe, shared by [`IcebergBackend::read_window`]'s
/// `predicate_active == false` path and [`IcebergBackend::read_single_row`]
/// (which is always this shape — `item` never pushes a filter into the
/// scan). Returns `(tasks.len(), 0)` when `window_start` falls past every
/// planned task, or a task carries no `record_count` (never true for this
/// slice's own whole-file, no-delete-file plans, but handled honestly
/// rather than assumed away).
fn locate_unfiltered_start(tasks: &[FileScanTask], window_start: u64) -> (usize, u64) {
    let mut cumulative: u64 = 0;
    for (idx, task) in tasks.iter().enumerate() {
        let Some(count) = task.record_count else {
            return (idx, cumulative);
        };
        if cumulative + count > window_start {
            return (idx, cumulative);
        }
        cumulative += count;
    }
    (tasks.len(), cumulative)
}

#[async_trait]
impl CatalogSource for IcebergBackend {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        // Forces the load-and-validate sequence (REST catalog lookup,
        // snapshot pinning, geometry/bbox column checks) exactly once, the
        // same "first CatalogSource call triggers the real work" contract
        // `tellurion-geoparquet`'s own `collections()` follows.
        self.table().await?;
        Ok(vec![PhysicalCollection {
            name: self.location.table.clone(),
            geometry_column: Some(self.location.geometry_column.clone()),
            primary_key: Some(PRIMARY_KEY_FIELD.to_string()),
            // Never invented — see this module's "Derived facts" docs.
            srid: None,
            geometry_type: None,
        }])
    }

    async fn extent(&self, _physical: &PhysicalCollection) -> CoreResult<Option<SpatialExtent>> {
        let cached = self.table().await?;
        Ok(cached.extent.map(|bbox| SpatialExtent { bbox }))
    }

    async fn row_estimate(&self, _physical: &PhysicalCollection) -> CoreResult<Option<u64>> {
        let cached = self.table().await?;
        Ok(cached.row_estimate)
    }

    async fn attribute_schema(
        &self,
        _physical: &PhysicalCollection,
    ) -> CoreResult<Option<Vec<AttributeColumn>>> {
        let cached = self.table().await?;
        Ok(Some(cached.attributes.clone()))
    }

    // `temporal_column`: default `None`. Iceberg schema metadata has no
    // semantic marker for "the" datetime column any more than GeoParquet's
    // does — same "zero candidates ever offered" posture
    // `tellurion-geoparquet` already applies for the identical reason.
}

#[async_trait]
impl FeatureSource for IcebergBackend {
    async fn items(
        &self,
        collection: &CollectionDecl,
        query: &ItemsQuery,
    ) -> CoreResult<FeaturePage> {
        self.items_inner(collection, query)
            .await
            .map_err(Into::into)
    }

    // See this module's "CQL2 and datetime pushdown" docs for the exact
    // supported subset `compile_predicate` compiles.
    fn filter_capable(&self) -> bool {
        true
    }

    /// `#105`: this module's "CQL2 and datetime pushdown" doc names the
    /// exact subset `compile_predicate` compiles — comparison, `IS [NOT]
    /// NULL`, `[NOT] IN`, and `AND`/`OR`/`NOT` over the table's own scalar
    /// columns, nothing else. That subset earns Basic CQL2 plus both
    /// encodings (the parser's job, not this compiler's, but no driver
    /// declares an encoding class without the operator class behind it) —
    /// and nothing more: `LIKE`/`BETWEEN` are refused by name even though
    /// `IN` compiles, so `advanced-comparison-operators` needs all three and
    /// stays undeclared; every spatial predicate (`S_INTERSECTS` included —
    /// the WKB geometry column has no scalar comparison `iceberg-rust`'s
    /// `Predicate`/`Datum` can express) and every temporal predicate
    /// (`T_AFTER`/`T_BEFORE`/`T_DURING` included) are refused by name too,
    /// so this driver declares neither `basic-spatial-functions` nor
    /// `spatial-functions` nor `temporal-functions` — a real gap from the
    /// pre-`#105` workspace-wide list, which declared `basic-spatial-
    /// functions` unconditionally without checking this driver actually
    /// compiled `S_INTERSECTS` at all.
    fn cql2_conformance_classes(&self) -> Vec<&'static str> {
        vec![
            tellurion_core::filter::CQL2_CLASS_BASIC,
            tellurion_core::filter::CQL2_CLASS_CQL2_TEXT,
            tellurion_core::filter::CQL2_CLASS_CQL2_JSON,
        ]
    }

    // `item` never pushes `filter` (a `#34` ABAC grant filter) into the
    // scan itself — it evaluates it in-process against the one fetched
    // row's own decoded values instead; see `item_inner`/
    // `read_single_row`/`evaluate_predicate` and this module's "CQL2 and
    // datetime pushdown" docs.
    async fn item(
        &self,
        _collection: &CollectionDecl,
        id: &str,
        filter: Option<&Filter>,
    ) -> CoreResult<Option<serde_json::Value>> {
        self.item_inner(id, filter).await.map_err(Into::into)
    }
}

/// One planned data file — the pinned snapshot's scan-plan output this
/// crate's own tests exercise. `record_count` mirrors `FileScanTask`'s own
/// `Option` (only populated when the task reads the entire file, which is
/// always true for the whole-file, no-projection plans this driver builds).
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlannedFile {
    pub path: String,
    pub record_count: Option<u64>,
}

/// Builds the always-available bbox pushdown predicate — those columns are
/// validated `Double`/`Float` schema columns at load time, so the iceberg
/// crate's own manifest/file-level metrics evaluator can always prune on
/// them.
fn bbox_predicate(columns: &BboxColumns, query: [f64; 4]) -> Predicate {
    let [query_xmin, query_ymin, query_xmax, query_ymax] = query;
    Reference::new(columns.xmin.clone())
        .less_than_or_equal_to(Datum::double(query_xmax))
        .and(
            Reference::new(columns.xmax.clone())
                .greater_than_or_equal_to(Datum::double(query_xmin)),
        )
        .and(Reference::new(columns.ymin.clone()).less_than_or_equal_to(Datum::double(query_ymax)))
        .and(
            Reference::new(columns.ymax.clone())
                .greater_than_or_equal_to(Datum::double(query_ymin)),
        )
}

/// In-process counterpart to [`bbox_predicate`] — tests one decoded row's
/// own four bbox column values against `query`, the same axis-aligned
/// overlap test the pushed-down `Predicate` encodes. Used by
/// [`IcebergBackend::row_matches_query`] whenever a CQL2 filter/datetime
/// interval keeps `bbox` from being pushed into the scan itself — see this
/// module's "CQL2 and datetime pushdown" docs. A row missing any of the
/// four bounds (`NULL`) can't be proven to intersect, so it's excluded
/// rather than assumed to match.
fn row_in_bbox(
    columns: &BboxColumns,
    batch: &RecordBatch,
    row: usize,
    query: [f64; 4],
) -> Result<bool> {
    let [query_xmin, query_ymin, query_xmax, query_ymax] = query;
    let (Some(xmin), Some(ymin), Some(xmax), Some(ymax)) = (
        row_bbox_bound(batch, &columns.xmin, row)?,
        row_bbox_bound(batch, &columns.ymin, row)?,
        row_bbox_bound(batch, &columns.xmax, row)?,
        row_bbox_bound(batch, &columns.ymax, row)?,
    ) else {
        return Ok(false);
    };
    Ok(xmin <= query_xmax && xmax >= query_xmin && ymin <= query_ymax && ymax >= query_ymin)
}

/// One bbox column's decoded value at `row` — `Double` or `Float`, the two
/// types [`require_numeric_column`] validates a declared bbox column into
/// at load time. `Ok(None)` means the cell itself is SQL `NULL`.
fn row_bbox_bound(batch: &RecordBatch, column: &str, row: usize) -> Result<Option<f64>> {
    let array = batch.column_by_name(column).ok_or_else(|| {
        IcebergDriverError::Decode(format!("column '{column}' missing from decoded batch"))
    })?;
    if array.is_null(row) {
        return Ok(None);
    }
    Ok(Some(match array.data_type() {
        DataType::Float64 => downcast::<Float64Array>(array)?.value(row),
        DataType::Float32 => downcast::<Float32Array>(array)?.value(row) as f64,
        other => {
            return Err(IcebergDriverError::Decode(format!(
                "bbox column '{column}' has unsupported array type {other:?}"
            )))
        }
    }))
}

/// `"<snapshot-id>:<filter-fingerprint>:<row-offset>"` — see this module's
/// "Snapshot-pinned paging" docs. `fingerprint` is always
/// `query_predicate_fingerprint(filter, datetime)` for the exact request
/// that minted this token.
fn encode_token(snapshot_id: i64, fingerprint: u64, offset: u64) -> String {
    format!("{snapshot_id}:{fingerprint:016x}:{offset}")
}

fn decode_token(token: &str, pinned_snapshot_id: i64, expected_fingerprint: u64) -> Result<u64> {
    let mut parts = token.split(':');
    let (Some(snapshot_part), Some(fingerprint_part), Some(offset_part), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(IcebergDriverError::InvalidToken(token.to_string()));
    };

    let snapshot_id: i64 = snapshot_part
        .parse()
        .map_err(|_| IcebergDriverError::InvalidToken(token.to_string()))?;
    if snapshot_id != pinned_snapshot_id {
        return Err(IcebergDriverError::TokenSnapshotMismatch {
            expected: pinned_snapshot_id,
            found: snapshot_id,
        });
    }

    let fingerprint = u64::from_str_radix(fingerprint_part, 16)
        .map_err(|_| IcebergDriverError::InvalidToken(token.to_string()))?;
    if fingerprint != expected_fingerprint {
        return Err(IcebergDriverError::TokenFilterMismatch {
            expected: expected_fingerprint,
            found: fingerprint,
        });
    }

    offset_part
        .parse::<u64>()
        .map_err(|_| IcebergDriverError::InvalidToken(token.to_string()))
}

/// Canonical cache key for a query bbox — `None` and a given `Some(bbox)`
/// each always map to the same key, which is all [`IcebergBackend::
/// plan_cache`] needs (bit-exact equality on the same finite `f64`s a
/// parsed query always produces, never a fuzzy/rounded comparison).
fn bbox_cache_key(bbox: Option<[f64; 4]>) -> String {
    match bbox {
        None => "*".to_string(),
        Some([xmin, ymin, xmax, ymax]) => {
            format!("{xmin}:{ymin}:{xmax}:{ymax}")
        }
    }
}

/// Full [`IcebergBackend::plan_cache`] key: `bbox` plus
/// [`query_predicate_fingerprint`] — a plan compiled for one CQL2
/// filter/datetime interval is never handed to a request compiled from a
/// different one, even at the identical bbox (`#45`'s slice-2 fix — see
/// this module's "Planned-file cache" docs).
fn plan_cache_key(bbox: Option<[f64; 4]>, fingerprint: u64) -> String {
    format!("{}#{fingerprint:016x}", bbox_cache_key(bbox))
}

/// A stable, process-local fingerprint of everything besides `bbox` that
/// narrows an `items()`/`plan_files()` scan — the plan-cache-key and
/// paging-token half of `#45`'s slice-2 fix. Reuses
/// [`tellurion_core::Filter::fingerprint`]'s exact hashing approach (the
/// same one `tellurion_core::cache::TileKey::policy_fingerprint` already
/// partitions the tile cache by) rather than inventing a second fingerprint
/// concept, folding the datetime interval — not itself a `Filter` — into the
/// same `DefaultHasher` construction. Two requests that fingerprint
/// identically are guaranteed to compile to the exact same Iceberg
/// predicate; a plan cached under one fingerprint, or a paging token minted
/// under one, must never be served/resumed under a different one — see
/// [`plan_cache_key`]/[`encode_token`]/[`decode_token`].
fn query_predicate_fingerprint(filter: Option<&Filter>, datetime: Option<&DatetimeRange>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match filter {
        Some(f) => {
            1u8.hash(&mut hasher);
            f.fingerprint().hash(&mut hasher);
        }
        None => 0u8.hash(&mut hasher),
    }
    match datetime {
        Some(range) => {
            1u8.hash(&mut hasher);
            range.start.hash(&mut hasher);
            range.end.hash(&mut hasher);
        }
        None => 0u8.hash(&mut hasher),
    }
    hasher.finish()
}

/// Compiles a CQL2 filter (`tellurion_core::Filter`) into an Iceberg
/// `Predicate` — pushable via `TableScan::with_filter` so Iceberg's own
/// planner can prune data files by column stats before any Parquet read and
/// its Arrow reader can apply genuine row-level filtering during the read
/// itself, exactly what this crate's own `plan_files` test helper pushes to
/// exercise that pruning directly. `items`/`item` themselves never push the
/// result any more (see this module's "CQL2 and datetime pushdown" docs for
/// why) — they call this purely to validate `filter` through the one
/// construct/property/type boundary below, then discard the `Predicate`.
/// Every property `filter` references has already passed
/// `tellurion_core::filter::validate` against this collection's derived
/// descriptor by the time it reaches here (a real attribute column, or the
/// collection's geometry/datetime column) — this function's own job is
/// deciding which of those *this driver* can actually turn into a faithful
/// Iceberg predicate, refusing every construct/column combination it cannot
/// with a named, specific [`IcebergDriverError::FilterPropertyUnsupported`]
/// rather than silently dropping it or serving unfiltered rows in its
/// place. See this module's "CQL2 and datetime pushdown" docs for the exact
/// supported boundary.
fn compile_predicate(
    filter: &Filter,
    schema: &Schema,
    location: &IcebergLocation,
) -> Result<Predicate> {
    match filter {
        Filter::Compare {
            property,
            op,
            value,
        } => {
            let field_type = scalar_field_type(schema, location, property)?;
            let datum = literal_datum(&field_type, value, property)?;
            Ok(compare_predicate(property, *op, datum))
        }
        Filter::IsNull { property, negated } => {
            // Type-blind (any scalar column supports IS [NOT] NULL), but
            // still refuses the geometry column / an unknown-to-Iceberg
            // property the same way Compare/In do — one shared gate.
            scalar_field_type(schema, location, property)?;
            let reference = Reference::new(property.clone());
            Ok(if *negated {
                reference.is_not_null()
            } else {
                reference.is_null()
            })
        }
        Filter::In {
            property,
            values,
            negated,
        } => {
            let field_type = scalar_field_type(schema, location, property)?;
            let datums = values
                .iter()
                .map(|value| literal_datum(&field_type, value, property))
                .collect::<Result<Vec<_>>>()?;
            let reference = Reference::new(property.clone());
            Ok(if *negated {
                reference.is_not_in(datums)
            } else {
                reference.is_in(datums)
            })
        }
        Filter::And(items) => {
            let mut acc = Predicate::AlwaysTrue;
            for item in items {
                acc = acc.and(compile_predicate(item, schema, location)?);
            }
            Ok(acc)
        }
        Filter::Or(items) => {
            let mut iter = items.iter();
            let Some(first) = iter.next() else {
                return Ok(Predicate::AlwaysFalse);
            };
            let mut acc = compile_predicate(first, schema, location)?;
            for item in iter {
                acc = acc.or(compile_predicate(item, schema, location)?);
            }
            Ok(acc)
        }
        Filter::Not(inner) => Ok(!compile_predicate(inner, schema, location)?),
        Filter::Like { property, .. } => Err(unsupported_construct(property, "LIKE")),
        Filter::Between { property, .. } => Err(unsupported_construct(property, "BETWEEN")),
        Filter::CaseInsensitiveCompare { property, .. } => {
            Err(unsupported_construct(property, "CASEI"))
        }
        Filter::Intersects { property, .. } => Err(unsupported_construct(property, "S_INTERSECTS")),
        Filter::Spatial { property, op, .. } => {
            Err(unsupported_construct(property, spatial_op_cql2_name(*op)))
        }
        Filter::After { property, .. } => Err(unsupported_construct(property, "T_AFTER")),
        Filter::Before { property, .. } => Err(unsupported_construct(property, "T_BEFORE")),
        Filter::During { property, .. } => Err(unsupported_construct(property, "T_DURING")),
        Filter::Temporal { property, op, .. } => {
            Err(unsupported_construct(property, temporal_op_cql2_name(*op)))
        }
    }
}

/// The Iceberg primitive type behind `property`, refusing (never silently
/// coercing) the declared geometry column, an unknown property, or a
/// non-primitive (struct/list/map) column — none of these have a faithful
/// scalar Iceberg predicate.
fn scalar_field_type(
    schema: &Schema,
    location: &IcebergLocation,
    property: &str,
) -> Result<PrimitiveType> {
    if property == location.geometry_column {
        return Err(IcebergDriverError::FilterPropertyUnsupported {
            property: property.to_string(),
            reason: "the declared geometry column carries WKB bytes, which has no scalar \
                     comparison in Iceberg"
                .to_string(),
        });
    }
    let field = schema.field_by_name(property).ok_or_else(|| {
        IcebergDriverError::FilterPropertyUnsupported {
            property: property.to_string(),
            reason: "column not present in the pinned snapshot's schema".to_string(),
        }
    })?;
    match field.field_type.as_ref() {
        Type::Primitive(primitive) => Ok(primitive.clone()),
        other => Err(IcebergDriverError::FilterPropertyUnsupported {
            property: property.to_string(),
            reason: format!("column type '{other}' is not a scalar Iceberg can compare"),
        }),
    }
}

/// Maps a CQL2 scalar literal onto `field_type`'s exact `Datum` shape —
/// deliberately narrow rather than leaning on `Datum::to`'s own (partial,
/// still-evolving upstream) coercion matrix: boolean/boolean,
/// integer-valued number/int or long (range- and fraction-checked), any
/// number/float or double, and text/string. Every other pairing —
/// including a text literal against a `timestamp(tz)` column, which this
/// driver's dedicated `compile_datetime_predicate` handles instead — is a
/// named refusal, never a guessed conversion.
fn literal_datum(field_type: &PrimitiveType, literal: &Literal, property: &str) -> Result<Datum> {
    match (field_type, literal) {
        (PrimitiveType::Boolean, Literal::Bool(b)) => Ok(Datum::bool(*b)),
        (PrimitiveType::Int, Literal::Number(n)) => {
            if n.fract() != 0.0 || *n < i32::MIN as f64 || *n > i32::MAX as f64 {
                return Err(literal_out_of_range(property, "int", *n));
            }
            Ok(Datum::int(*n as i32))
        }
        (PrimitiveType::Long, Literal::Number(n)) => {
            if n.fract() != 0.0 || *n < i64::MIN as f64 || *n > i64::MAX as f64 {
                return Err(literal_out_of_range(property, "long", *n));
            }
            Ok(Datum::long(*n as i64))
        }
        (PrimitiveType::Float, Literal::Number(n)) => Ok(Datum::float(*n as f32)),
        (PrimitiveType::Double, Literal::Number(n)) => Ok(Datum::double(*n)),
        (PrimitiveType::String, Literal::Text(s)) => Ok(Datum::string(s.clone())),
        _ => Err(IcebergDriverError::FilterPropertyUnsupported {
            property: property.to_string(),
            reason: format!(
                "column type '{field_type}' has no compilable mapping for a {} literal",
                literal_kind_name(literal)
            ),
        }),
    }
}

fn literal_out_of_range(property: &str, iceberg_type: &str, value: f64) -> IcebergDriverError {
    IcebergDriverError::FilterPropertyUnsupported {
        property: property.to_string(),
        reason: format!("numeric literal {value} does not fit Iceberg type '{iceberg_type}'"),
    }
}

fn literal_kind_name(literal: &Literal) -> &'static str {
    match literal {
        Literal::Text(_) => "text",
        Literal::Number(_) => "number",
        Literal::Bool(_) => "boolean",
    }
}

fn unsupported_construct(property: &str, construct: &'static str) -> IcebergDriverError {
    IcebergDriverError::FilterPropertyUnsupported {
        property: property.to_string(),
        reason: format!("'{construct}' has no compilable Iceberg predicate equivalent"),
    }
}

fn spatial_op_cql2_name(op: SpatialOp) -> &'static str {
    match op {
        SpatialOp::Within => "S_WITHIN",
        SpatialOp::Contains => "S_CONTAINS",
        SpatialOp::Disjoint => "S_DISJOINT",
        SpatialOp::Touches => "S_TOUCHES",
        SpatialOp::Overlaps => "S_OVERLAPS",
        SpatialOp::Crosses => "S_CROSSES",
        SpatialOp::Equals => "S_EQUALS",
    }
}

fn temporal_op_cql2_name(op: TemporalOp) -> &'static str {
    match op {
        TemporalOp::Contains => "T_CONTAINS",
        TemporalOp::Disjoint => "T_DISJOINT",
        TemporalOp::Equals => "T_EQUALS",
        TemporalOp::FinishedBy => "T_FINISHEDBY",
        TemporalOp::Finishes => "T_FINISHES",
        TemporalOp::Intersects => "T_INTERSECTS",
        TemporalOp::Meets => "T_MEETS",
        TemporalOp::MetBy => "T_METBY",
        TemporalOp::OverlappedBy => "T_OVERLAPPEDBY",
        TemporalOp::Overlaps => "T_OVERLAPS",
        TemporalOp::StartedBy => "T_STARTEDBY",
        TemporalOp::Starts => "T_STARTS",
    }
}

fn compare_predicate(property: &str, op: CompareOp, datum: Datum) -> Predicate {
    let reference = Reference::new(property.to_string());
    match op {
        CompareOp::Eq => reference.equal_to(datum),
        CompareOp::Ne => reference.not_equal_to(datum),
        CompareOp::Lt => reference.less_than(datum),
        CompareOp::Gt => reference.greater_than(datum),
        CompareOp::Le => reference.less_than_or_equal_to(datum),
        CompareOp::Ge => reference.greater_than_or_equal_to(datum),
    }
}

/// SQL three-valued boolean: `Some(true)`/`Some(false)` for a definite
/// result, `None` for "unknown" — the same value a comparison against a SQL
/// `NULL` operand produces. [`evaluate_predicate`] keeps a row only when its
/// top-level result is exactly `Some(true)`; both `Some(false)` and `None`
/// exclude it, but the two stay distinguishable through this type so
/// `AND`/`OR`/`NOT` still propagate "unknown" the way SQL's own
/// three-valued logic does ([`tri_and`]/[`tri_or`]/[`tri_not`]) rather than
/// collapsing it into `false` too early and giving a wrong answer inside a
/// larger expression.
type Tri = Option<bool>;

fn tri_not(value: Tri) -> Tri {
    value.map(|v| !v)
}

fn tri_and(a: Tri, b: Tri) -> Tri {
    match (a, b) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (Some(true), Some(true)) => Some(true),
        _ => None,
    }
}

fn tri_or(a: Tri, b: Tri) -> Tri {
    match (a, b) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), Some(false)) => Some(false),
        _ => None,
    }
}

/// Evaluates `filter` against one already-decoded row of `batch` — the
/// per-row counterpart to [`compile_predicate`]'s scan-level pushdown, used
/// only by `item`'s single-row lookup ([`IcebergBackend::read_single_row`];
/// see this module's "CQL2 and datetime pushdown" docs). Walks the
/// identical `Filter` tree shape `compile_predicate` does, through the
/// exact same [`scalar_field_type`]/[`literal_datum`]/[`unsupported_construct`]
/// helpers, so every property/type/construct check refuses with the
/// identical named error `compile_predicate` would give the same filter —
/// this function never accepts a filter `compile_predicate` would refuse,
/// and never refuses one it would accept. Returns [`Tri`] rather than
/// `bool` so `And`/`Or`/`Not` compose under SQL's own three-valued logic: a
/// comparison against a `NULL` column value is `None` ("unknown"), not
/// `Some(false)`. The row survives the filter only when this returns
/// exactly `Some(true)`.
fn evaluate_predicate(
    filter: &Filter,
    schema: &Schema,
    location: &IcebergLocation,
    batch: &RecordBatch,
    row: usize,
) -> Result<Tri> {
    match filter {
        Filter::Compare {
            property,
            op,
            value,
        } => {
            let field_type = scalar_field_type(schema, location, property)?;
            let literal = literal_datum(&field_type, value, property)?;
            let row_datum = row_scalar_datum(batch, &field_type, property, row)?;
            Ok(compare_tri(row_datum, *op, &literal))
        }
        Filter::IsNull { property, negated } => {
            // Type-blind, exactly like `compile_predicate`'s own arm — any
            // scalar column supports IS [NOT] NULL, so this only needs the
            // batch's own null bitmap, never a decoded `Datum`.
            scalar_field_type(schema, location, property)?;
            let column = batch.column_by_name(property).ok_or_else(|| {
                IcebergDriverError::Decode(format!(
                    "column '{property}' missing from decoded batch"
                ))
            })?;
            let is_null = column.is_null(row);
            Ok(Some(if *negated { !is_null } else { is_null }))
        }
        Filter::In {
            property,
            values,
            negated,
        } => {
            let field_type = scalar_field_type(schema, location, property)?;
            let datums = values
                .iter()
                .map(|value| literal_datum(&field_type, value, property))
                .collect::<Result<Vec<_>>>()?;
            let row_datum = row_scalar_datum(batch, &field_type, property, row)?;
            // SQL semantics: `NULL IN (...)`/`NULL NOT IN (...)` are both
            // "unknown", never `true`/`false` — a `None` row value stays
            // `None` all the way through, negated or not.
            let membership =
                row_datum.map(|row_value| datums.iter().any(|datum| datum_eq(&row_value, datum)));
            Ok(if *negated {
                tri_not(membership)
            } else {
                membership
            })
        }
        Filter::And(items) => {
            let mut acc: Tri = Some(true);
            for item in items {
                acc = tri_and(acc, evaluate_predicate(item, schema, location, batch, row)?);
            }
            Ok(acc)
        }
        Filter::Or(items) => {
            let mut iter = items.iter();
            let Some(first) = iter.next() else {
                return Ok(Some(false));
            };
            let mut acc = evaluate_predicate(first, schema, location, batch, row)?;
            for item in iter {
                acc = tri_or(acc, evaluate_predicate(item, schema, location, batch, row)?);
            }
            Ok(acc)
        }
        Filter::Not(inner) => Ok(tri_not(evaluate_predicate(
            inner, schema, location, batch, row,
        )?)),
        Filter::Like { property, .. } => Err(unsupported_construct(property, "LIKE")),
        Filter::Between { property, .. } => Err(unsupported_construct(property, "BETWEEN")),
        Filter::CaseInsensitiveCompare { property, .. } => {
            Err(unsupported_construct(property, "CASEI"))
        }
        Filter::Intersects { property, .. } => Err(unsupported_construct(property, "S_INTERSECTS")),
        Filter::Spatial { property, op, .. } => {
            Err(unsupported_construct(property, spatial_op_cql2_name(*op)))
        }
        Filter::After { property, .. } => Err(unsupported_construct(property, "T_AFTER")),
        Filter::Before { property, .. } => Err(unsupported_construct(property, "T_BEFORE")),
        Filter::During { property, .. } => Err(unsupported_construct(property, "T_DURING")),
        Filter::Temporal { property, op, .. } => {
            Err(unsupported_construct(property, temporal_op_cql2_name(*op)))
        }
    }
}

/// Compares a decoded row value against a filter literal, both already
/// [`Datum`]s of the identical [`PrimitiveType`] — reusing `Datum`'s own
/// `PartialOrd` (the same comparison Iceberg's manifest/row evaluators use
/// internally) rather than hand-rolling comparison rules of this function's
/// own, so this always agrees with however Iceberg itself would order the
/// same two values (including its float/NaN ordering). `None` propagates
/// from either a `NULL` row value or a type pairing `Datum` itself refuses
/// to order — both are "unknown", exactly SQL's own three-valued logic for
/// a comparison against `NULL`.
fn compare_tri(row_datum: Option<Datum>, op: CompareOp, literal: &Datum) -> Tri {
    let row_datum = row_datum?;
    let ordering = row_datum.partial_cmp(literal)?;
    Some(match op {
        CompareOp::Eq => ordering.is_eq(),
        CompareOp::Ne => !ordering.is_eq(),
        CompareOp::Lt => ordering.is_lt(),
        CompareOp::Gt => ordering.is_gt(),
        CompareOp::Le => ordering.is_le(),
        CompareOp::Ge => ordering.is_ge(),
    })
}

fn datum_eq(a: &Datum, b: &Datum) -> bool {
    a.partial_cmp(b) == Some(std::cmp::Ordering::Equal)
}

/// Decodes `property`'s value at `row` of `batch` into an Iceberg [`Datum`]
/// of exactly `field_type` — the row-value counterpart to [`literal_datum`],
/// which produces a `Datum` of the same shape for the filter's own literal,
/// so [`Datum`]'s `PartialOrd` compares the two like-for-like. Only ever
/// reached for the six [`PrimitiveType`]s `literal_datum` itself accepts —
/// `Compare`/`In` in [`evaluate_predicate`] always call `literal_datum`
/// first, so an unsupported field type is refused before this ever runs;
/// every other primitive type only ever reaches the type-blind `IsNull` arm,
/// which never calls this. `Ok(None)` means the cell itself is SQL `NULL`.
fn row_scalar_datum(
    batch: &RecordBatch,
    field_type: &PrimitiveType,
    property: &str,
    row: usize,
) -> Result<Option<Datum>> {
    let column = batch.column_by_name(property).ok_or_else(|| {
        IcebergDriverError::Decode(format!("column '{property}' missing from decoded batch"))
    })?;
    if column.is_null(row) {
        return Ok(None);
    }
    let datum = match field_type {
        PrimitiveType::Boolean => Datum::bool(downcast::<BooleanArray>(column)?.value(row)),
        PrimitiveType::Int => Datum::int(downcast::<Int32Array>(column)?.value(row)),
        PrimitiveType::Long => Datum::long(downcast::<Int64Array>(column)?.value(row)),
        PrimitiveType::Float => Datum::float(downcast::<Float32Array>(column)?.value(row)),
        PrimitiveType::Double => Datum::double(downcast::<Float64Array>(column)?.value(row)),
        PrimitiveType::String => match column.data_type() {
            DataType::Utf8 => {
                Datum::string(downcast::<StringArray>(column)?.value(row).to_string())
            }
            DataType::LargeUtf8 => {
                Datum::string(downcast::<LargeStringArray>(column)?.value(row).to_string())
            }
            other => {
                return Err(IcebergDriverError::Decode(format!(
                    "column '{property}' has unsupported string array type {other:?}"
                )))
            }
        },
        other => {
            return Err(IcebergDriverError::Decode(format!(
                "column '{property}' has iceberg type '{other}', which has no row-value decode \
                 path for item-level filter evaluation"
            )))
        }
    };
    Ok(Some(datum))
}

/// Resolves and validates `collection`'s declared `datetime` column against
/// `schema` — shared by [`compile_datetime_predicate`] (the bbox-only,
/// scan-pushdown path) and [`evaluate_datetime_range`] (the in-process,
/// row-level path a `filter`/`datetime`-active `items` call always uses
/// instead — see this module's "CQL2 and datetime pushdown" docs). Refuses
/// with [`IcebergDriverError::NoDatetimeColumn`] when the collection has
/// none declared, and [`IcebergDriverError::DatetimeColumnNotFound`]/
/// [`IcebergDriverError::DatetimeColumnWrongType`] when the declared column
/// doesn't exist or isn't `timestamptz` in the pinned schema (an
/// offset-naive `timestamp` column has no honest way to compare against an
/// RFC 3339 instant, which always carries an offset).
fn resolve_datetime_column<'a>(collection: &'a CollectionDecl, schema: &Schema) -> Result<&'a str> {
    let Some(column) = collection.datetime.as_deref() else {
        return Err(IcebergDriverError::NoDatetimeColumn(collection.id.clone()));
    };
    let field =
        schema
            .field_by_name(column)
            .ok_or_else(|| IcebergDriverError::DatetimeColumnNotFound {
                table: collection.id.clone(),
                column: column.to_string(),
            })?;
    match field.field_type.as_ref() {
        Type::Primitive(PrimitiveType::Timestamptz) => Ok(column),
        other => Err(IcebergDriverError::DatetimeColumnWrongType {
            table: collection.id.clone(),
            column: column.to_string(),
            actual: other.to_string(),
        }),
    }
}

/// Compiles the standard `datetime` interval query parameter
/// (`ItemsQuery::datetime`) against `collection`'s declared `datetime`
/// column into a scan-pushdown `Predicate` — used only by a bbox-only
/// `items` call (see this module's "CQL2 and datetime pushdown" docs); a
/// `filter`/`datetime`-active call still runs this purely to validate
/// `range` through the same boundary, via [`resolve_datetime_column`], and
/// discards the result. Refuses the same way `resolve_datetime_column`
/// does, plus [`IcebergDriverError::InvalidDatetimeLiteral`] when `range`'s
/// own bound text doesn't parse as one.
fn compile_datetime_predicate(
    collection: &CollectionDecl,
    schema: &Schema,
    range: &DatetimeRange,
) -> Result<Predicate> {
    let column = resolve_datetime_column(collection, schema)?;

    let mut predicate = Predicate::AlwaysTrue;
    if let Some(start) = &range.start {
        let datum = datetime_datum(column, start)?;
        predicate =
            predicate.and(Reference::new(column.to_string()).greater_than_or_equal_to(datum));
    }
    if let Some(end) = &range.end {
        let datum = datetime_datum(column, end)?;
        predicate = predicate.and(Reference::new(column.to_string()).less_than_or_equal_to(datum));
    }
    Ok(predicate)
}

/// In-process counterpart to [`compile_datetime_predicate`] — evaluates
/// `range` against one decoded row's own `datetime` column value, returning
/// the same three-valued (`Tri`) shape [`evaluate_predicate`] does, so
/// [`IcebergBackend::row_matches_query`] can compose a CQL2 `filter` and a
/// `datetime` interval under identical SQL three-valued-logic rules. A
/// `NULL` column value is "unknown", exactly `evaluate_predicate`'s own
/// convention — never silently `false`.
fn evaluate_datetime_range(
    collection: &CollectionDecl,
    schema: &Schema,
    range: &DatetimeRange,
    batch: &RecordBatch,
    row: usize,
) -> Result<Tri> {
    let column = resolve_datetime_column(collection, schema)?;
    let array = batch.column_by_name(column).ok_or_else(|| {
        IcebergDriverError::Decode(format!("column '{column}' missing from decoded batch"))
    })?;
    if array.is_null(row) {
        return Ok(None);
    }
    let micros = downcast::<TimestampMicrosecondArray>(array)?.value(row);
    let row_datum = Datum::timestamptz_micros(micros);

    let mut acc: Tri = Some(true);
    if let Some(start) = &range.start {
        let bound = datetime_datum(column, start)?;
        acc = tri_and(acc, row_datum.partial_cmp(&bound).map(|ord| ord.is_ge()));
    }
    if let Some(end) = &range.end {
        let bound = datetime_datum(column, end)?;
        acc = tri_and(acc, row_datum.partial_cmp(&bound).map(|ord| ord.is_le()));
    }
    Ok(acc)
}

fn datetime_datum(column: &str, value: &str) -> Result<Datum> {
    Datum::timestamptz_from_str(value).map_err(|err| IcebergDriverError::InvalidDatetimeLiteral {
        column: column.to_string(),
        value: value.to_string(),
        cause: err.to_string(),
    })
}

/// Resolves the locator's four `s3_*` declarations, plus the two
/// environment variables they NAME, into the connection
/// `fileio::ObjectStoreStorage` reads S3 through.
///
/// `Ok(None)` when the locator declares no complete `s3_*` set — the shape
/// every local-filesystem table has, and the shape that must keep behaving
/// exactly as it did before `#123`. A locator that DOES declare the set but
/// names an unset environment variable refuses here, by name, at load time
/// rather than on the first read of a data file.
///
/// Credentials are read from the process environment and nowhere else:
/// never from `config.yaml`, never from a config struct, and never from the
/// `config` block a REST catalog server returns (see
/// `fileio::ObjectStoreStorageFactory::build`, which deliberately ignores
/// it).
fn resolve_s3_connection(location: &IcebergLocation) -> Result<Option<Arc<S3Connection>>> {
    let Some(declaration) = &location.s3 else {
        return Ok(None);
    };
    let read = |var: &str| -> Result<String> {
        std::env::var(var).map_err(|_| IcebergDriverError::MissingS3Credential {
            table: location.identifier(),
            var: var.to_string(),
        })
    };
    Ok(Some(Arc::new(S3Connection {
        endpoint: declaration.endpoint.clone(),
        region: declaration.region.clone(),
        access_key: read(&declaration.access_key_env)?,
        secret_key: read(&declaration.secret_key_env)?,
    })))
}

/// The boot-time storage check: the table's own metadata says where its
/// files live, and this is where an unsupported location is refused BY
/// NAME — before a snapshot is pinned, before an extent is computed, before
/// a single request is served.
///
/// Three outcomes, no fourth:
///
/// - local filesystem, or S3 with the locator's four `s3_*` declarations
///   present: proceed;
/// - GCS/ADLS (or any scheme this driver does not recognize): refuse,
///   naming the scheme found and saying it is not supported
///   (`fileio::UnsupportedScheme`'s own wording);
/// - S3 with an incomplete `s3_*` set: refuse, naming the missing key.
///
/// There is deliberately no "unknown scheme, try the local filesystem"
/// branch. A silent fallback here would turn a misconfigured object-store
/// location into a confusing file-not-found much later, or — far worse —
/// into a successful read of some unrelated local path.
fn require_supported_storage(location: &IcebergLocation, table: &Table) -> Result<()> {
    let storage_location = table.metadata().location().to_string();
    let route = StorageRoute::resolve(&storage_location).map_err(|refusal| {
        IcebergDriverError::UnsupportedStorageScheme {
            table: location.identifier(),
            location: storage_location.clone(),
            detail: refusal.to_string(),
        }
    })?;
    if route.needs_s3_declaration() {
        if let Some(field) = location.s3_partial.first_missing() {
            return Err(IcebergDriverError::MissingS3Declaration {
                table: location.identifier(),
                location: storage_location,
                field,
            });
        }
    }
    Ok(())
}

/// Opens `location`'s table through the REST catalog — see this module's
/// "Table access" docs. The `RestCatalog` client itself is never persisted:
/// once `load_table` resolves, everything this driver reads afterward goes
/// through the returned `Table`'s own `file_io()`.
async fn open_rest_table(location: &IcebergLocation) -> Result<Table> {
    let s3 = resolve_s3_connection(location)?;
    let catalog = RestCatalogBuilder::default()
        .with_storage_factory(Arc::new(ObjectStoreStorageFactory::new(s3)))
        .load(
            "iceberg",
            HashMap::from([(
                REST_CATALOG_PROP_URI.to_string(),
                location.catalog_uri.clone(),
            )]),
        )
        .await
        .map_err(IcebergDriverError::Iceberg)?;

    let namespace = NamespaceIdent::from_strs(location.namespace.iter().cloned())
        .map_err(IcebergDriverError::Iceberg)?;
    let table_ident = TableIdent::new(namespace, location.table.clone());
    catalog
        .load_table(&table_ident)
        .await
        .map_err(IcebergDriverError::Iceberg)
}

/// Opens the table via the REST catalog, pins its current snapshot,
/// validates the declared geometry/bbox columns against the pinned schema,
/// and eagerly derives `row_estimate`/`extent`/`attributes` — everything
/// this driver will ever report about this backend for its lifetime. See
/// this module's crate docs.
async fn load_cached_table(location: &IcebergLocation) -> Result<CachedTable> {
    let table = open_rest_table(location).await?;
    // Before anything is derived from this table — snapshot, schema,
    // extent, every fact this backend will ever report — check that this
    // driver can actually READ where its files live. See
    // `require_supported_storage`.
    require_supported_storage(location, &table)?;

    let table_metadata: TableMetadataRef = table.metadata_ref();
    let snapshot: SnapshotRef = table_metadata.current_snapshot().cloned().ok_or_else(|| {
        IcebergDriverError::NoCurrentSnapshot {
            table: location.identifier(),
        }
    })?;
    let schema = snapshot
        .schema(&table_metadata)
        .map_err(IcebergDriverError::Iceberg)?;

    require_binary_column(&schema, &location.identifier(), &location.geometry_column)?;
    let bbox_field_ids = [
        require_numeric_column(&schema, &location.identifier(), &location.bbox.xmin)?,
        require_numeric_column(&schema, &location.identifier(), &location.bbox.ymin)?,
        require_numeric_column(&schema, &location.identifier(), &location.bbox.xmax)?,
        require_numeric_column(&schema, &location.identifier(), &location.bbox.ymax)?,
    ];

    let row_estimate = snapshot
        .summary()
        .additional_properties
        .get(TOTAL_RECORDS_SUMMARY_KEY)
        .and_then(|value| value.parse::<u64>().ok());

    let extent = compute_extent(table.file_io(), &table_metadata, &snapshot, bbox_field_ids)
        .await
        .map_err(IcebergDriverError::Iceberg)?;

    let excluded: [&str; 5] = [
        location.geometry_column.as_str(),
        location.bbox.xmin.as_str(),
        location.bbox.ymin.as_str(),
        location.bbox.xmax.as_str(),
        location.bbox.ymax.as_str(),
    ];
    let attributes = schema
        .as_struct()
        .fields()
        .iter()
        .filter(|field| !excluded.contains(&field.name.as_str()))
        .map(|field| AttributeColumn {
            name: field.name.clone(),
            sql_type: iceberg_type_to_sql(&field.field_type),
        })
        .collect();

    Ok(CachedTable {
        table,
        snapshot_id: snapshot.snapshot_id(),
        schema,
        row_estimate,
        extent,
        attributes,
    })
}

fn require_binary_column(schema: &Schema, table: &str, column: &str) -> Result<()> {
    let field =
        schema
            .field_by_name(column)
            .ok_or_else(|| IcebergDriverError::GeometryColumnNotFound {
                table: table.to_string(),
                column: column.to_string(),
            })?;
    match field.field_type.as_ref() {
        Type::Primitive(PrimitiveType::Binary) => Ok(()),
        other => Err(IcebergDriverError::GeometryColumnWrongType {
            table: table.to_string(),
            column: column.to_string(),
            actual: other.to_string(),
        }),
    }
}

/// Validates a declared bbox column is `Double` or `Float` and returns its
/// Iceberg field id (used by [`compute_extent`] to key into a data file's
/// `lower_bounds`/`upper_bounds`).
fn require_numeric_column(schema: &Schema, table: &str, column: &str) -> Result<i32> {
    let field =
        schema
            .field_by_name(column)
            .ok_or_else(|| IcebergDriverError::BboxColumnNotFound {
                table: table.to_string(),
                column: column.to_string(),
            })?;
    match field.field_type.as_ref() {
        Type::Primitive(PrimitiveType::Double) | Type::Primitive(PrimitiveType::Float) => {
            Ok(field.id)
        }
        other => Err(IcebergDriverError::BboxColumnWrongType {
            table: table.to_string(),
            column: column.to_string(),
            actual: other.to_string(),
        }),
    }
}

/// Folds every live (non-deleted) `Data`-content file's bbox-column bounds
/// into one CRS84-agnostic bbox — see this module's "Derived facts" docs for
/// the all-or-nothing honesty rule. Reads the pinned snapshot's manifest
/// list and every manifest it references; no data file content is touched
/// (bounds are footer-level statistics the writer already recorded).
async fn compute_extent(
    file_io: &iceberg::io::FileIO,
    table_metadata: &iceberg::spec::TableMetadata,
    snapshot: &Snapshot,
    bbox_field_ids: [i32; 4],
) -> iceberg::Result<Option<[f64; 4]>> {
    let manifest_list = snapshot.load_manifest_list(file_io, table_metadata).await?;

    let mut overall: Option<[f64; 4]> = None;
    for manifest_file in manifest_list.entries() {
        let manifest = manifest_file.load_manifest(file_io).await?;
        for entry in manifest.entries() {
            if !entry.is_alive() || entry.data_file().content_type() != DataContentType::Data {
                continue;
            }
            let Some(file_bbox) = data_file_bbox(entry.data_file(), bbox_field_ids) else {
                return Ok(None);
            };
            overall = Some(match overall {
                Some(acc) => union_bbox(acc, file_bbox),
                None => file_bbox,
            });
        }
    }
    Ok(overall)
}

fn data_file_bbox(data_file: &DataFile, field_ids: [i32; 4]) -> Option<[f64; 4]> {
    let [xmin_id, ymin_id, xmax_id, ymax_id] = field_ids;
    Some([
        datum_as_f64(data_file.lower_bounds().get(&xmin_id)?)?,
        datum_as_f64(data_file.lower_bounds().get(&ymin_id)?)?,
        datum_as_f64(data_file.upper_bounds().get(&xmax_id)?)?,
        datum_as_f64(data_file.upper_bounds().get(&ymax_id)?)?,
    ])
}

fn datum_as_f64(datum: &Datum) -> Option<f64> {
    match datum.literal() {
        PrimitiveLiteral::Double(value) => Some(value.0),
        PrimitiveLiteral::Float(value) => Some(value.0 as f64),
        _ => None,
    }
}

fn union_bbox(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

/// Broad SQL-flavored type name for an Iceberg primitive type, in the same
/// spirit as `tellurion-geoparquet`'s `arrow_type_to_sql`: approximate,
/// never a full type. Nested types (struct/list/map) fall back to a
/// lowercased `Display` rendering rather than failing the whole call over
/// one exotic column.
fn iceberg_type_to_sql(field_type: &Type) -> String {
    match field_type {
        Type::Primitive(PrimitiveType::Boolean) => "boolean".to_string(),
        Type::Primitive(PrimitiveType::Int) => "integer".to_string(),
        Type::Primitive(PrimitiveType::Long) => "bigint".to_string(),
        Type::Primitive(PrimitiveType::Float) => "real".to_string(),
        Type::Primitive(PrimitiveType::Double) => "double precision".to_string(),
        Type::Primitive(PrimitiveType::Decimal { .. }) => "numeric".to_string(),
        Type::Primitive(PrimitiveType::Date) => "date".to_string(),
        Type::Primitive(PrimitiveType::Time) => "time".to_string(),
        Type::Primitive(PrimitiveType::Timestamp | PrimitiveType::TimestampNs) => {
            "timestamp without time zone".to_string()
        }
        Type::Primitive(PrimitiveType::Timestamptz | PrimitiveType::TimestamptzNs) => {
            "timestamp with time zone".to_string()
        }
        Type::Primitive(PrimitiveType::String) => "text".to_string(),
        Type::Primitive(PrimitiveType::Uuid) => "uuid".to_string(),
        Type::Primitive(PrimitiveType::Fixed(_) | PrimitiveType::Binary) => "bytea".to_string(),
        other => other.to_string().to_lowercase(),
    }
}

/// Downcasts an Arrow `ArrayRef` to a concrete array type. Should never fail
/// in practice — every call site already matched the column's `DataType`
/// before reaching here — but Rust can't statically prove that, so this
/// reports an honest [`IcebergDriverError::Decode`] rather than panicking
/// on a `DataType`/array-type mismatch this driver's own logic has a bug
/// in.
fn downcast<T: 'static>(array: &ArrayRef) -> Result<&T> {
    array.as_any().downcast_ref::<T>().ok_or_else(|| {
        IcebergDriverError::Decode(format!(
            "expected array type {}, got {:?}",
            std::any::type_name::<T>(),
            array.data_type()
        ))
    })
}

/// The WKB bytes for `row` out of a geometry column — `iceberg::arrow`'s
/// own reader widens a schema-declared `Binary` field to Arrow's
/// `LargeBinary` on the way back out (verified against this driver's own
/// fixtures), but a plain `Binary` array is accepted too rather than
/// assuming one specific width forever.
fn geometry_bytes(array: &ArrayRef, row: usize) -> Result<&[u8]> {
    if let Some(large) = array.as_any().downcast_ref::<LargeBinaryArray>() {
        return Ok(large.value(row));
    }
    if let Some(plain) = array.as_any().downcast_ref::<BinaryArray>() {
        return Ok(plain.value(row));
    }
    Err(IcebergDriverError::Decode(format!(
        "geometry column has unsupported array type {:?}",
        array.data_type()
    )))
}

fn json_int(value: i64) -> serde_json::Value {
    serde_json::Value::Number(value.into())
}

fn json_float(value: f64) -> serde_json::Value {
    serde_json::Number::from_f64(value)
        .map(serde_json::Value::Number)
        .unwrap_or(serde_json::Value::Null)
}

/// `"{date} {time} UTC"` (`iceberg::spec::Datum`'s own `Display` for a
/// timestamptz) into RFC 3339 (`"{date}T{time}Z"`) — a plain string
/// transform, not a reparse, so this stays dependency-free (no `chrono` in
/// this crate's own `Cargo.toml`).
fn timestamptz_micros_to_rfc3339(micros: i64) -> String {
    let text = Datum::timestamptz_micros(micros).to_string();
    match text.splitn(3, ' ').collect::<Vec<_>>()[..] {
        [date, time, "UTC"] => format!("{date}T{time}Z"),
        _ => text,
    }
}

/// One property value from one row of a decoded batch. Supports the
/// practical attribute shapes this driver's own fixture (and typical
/// iceberg-rust writers) actually use, plus the `timestamptz` shape
/// `iceberg::arrow::ArrowReader` produces for an Iceberg `Timestamptz`
/// column (`DataType::Timestamp(Microsecond, Some(_))` — always
/// microsecond precision per the Iceberg spec; `TimestampNs`/`TimestamptzNs`
/// have no declarable path onto this driver's own `datetime` column at all,
/// see `compile_datetime_predicate`, so there is no fixture shape that would
/// ever exercise a nanosecond-precision value here) — anything else is an
/// honest [`IcebergDriverError::Decode`] rather than silently emitting a
/// wrong or lossy value, same discipline `tellurion-geoparquet`'s own
/// `arrow_value_to_json` applies.
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
        DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, Some(_)) => {
            let micros = downcast::<TimestampMicrosecondArray>(array)?.value(row);
            serde_json::Value::String(timestamptz_micros_to_rfc3339(micros))
        }
        other => {
            return Err(IcebergDriverError::Decode(format!(
                "unsupported attribute column type: {other:?}"
            )))
        }
    };
    Ok(value)
}

/// `batch`'s non-geometry, non-bbox columns at `row`, keyed by field name —
/// a feature's GeoJSON `properties`. Driven straight off the decoded
/// batch's own Arrow schema (rather than the pinned Iceberg schema this
/// driver also holds) — the two always agree on field names for this
/// slice's whole-column, no-projection reads, and the batch's own
/// `DataType` is what a value actually needs to be decoded against.
fn properties_from_batch(
    batch: &RecordBatch,
    location: &IcebergLocation,
    row: usize,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    let mut properties = serde_json::Map::new();
    for field in batch.schema().fields() {
        let name = field.name();
        if name == &location.geometry_column || location.bbox.as_array().contains(&name.as_str()) {
            continue;
        }
        let column = batch.column_by_name(name).ok_or_else(|| {
            IcebergDriverError::Decode(format!("column '{name}' missing from decoded batch"))
        })?;
        properties.insert(
            name.clone(),
            arrow_value_to_json(column, field.data_type(), row)?,
        );
    }
    Ok(properties)
}

/// Decodes one WKB geometry into a bare GeoJSON geometry object, via the
/// same `geozero::geojson::GeoJsonWriter` `flatgeobuf`'s/`geoparquet`'s own
/// drivers use.
fn geometry_json_from_wkb(wkb: &[u8]) -> Result<serde_json::Value> {
    use geozero::GeozeroGeometry;

    let mut buf: Vec<u8> = Vec::new();
    {
        let mut writer = geozero::geojson::GeoJsonWriter::new(&mut buf);
        geozero::wkb::Wkb(wkb)
            .process_geom(&mut writer)
            .map_err(|err| IcebergDriverError::Decode(err.to_string()))?;
    }
    serde_json::from_slice(&buf).map_err(|err| IcebergDriverError::Decode(err.to_string()))
}

/// Turns one decoded row into a full GeoJSON `Feature` object with `id` set
/// to `pk` (as a string, matching flatgeobuf's/geoparquet's own `pk::text`
/// convention).
fn feature_to_geojson(
    batch: &RecordBatch,
    location: &IcebergLocation,
    row: usize,
    pk: u64,
) -> Result<serde_json::Value> {
    let geometry_column = batch
        .column_by_name(&location.geometry_column)
        .ok_or_else(|| {
            IcebergDriverError::Decode(format!(
                "geometry column '{}' missing from decoded batch",
                location.geometry_column
            ))
        })?;
    let geometry = geometry_json_from_wkb(geometry_bytes(geometry_column, row)?)?;
    let properties = properties_from_batch(batch, location, row)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{point_wkb, S3TestFixture, TestFixture, DATETIME_COLUMN, S3_BUCKET};
    use tellurion_core::Error as CoreError;

    #[tokio::test]
    async fn collections_reports_the_declared_identity() {
        let fixture = TestFixture::build().await;
        let backend = fixture.backend();
        let collections = backend.collections().await.unwrap();
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].name, "points");
        assert_eq!(collections[0].geometry_column.as_deref(), Some("geom"));
        assert_eq!(collections[0].primary_key.as_deref(), Some("fid"));
        assert_eq!(collections[0].srid, None);
    }

    #[tokio::test]
    async fn row_estimate_comes_from_the_snapshot_summary() {
        let fixture = TestFixture::build().await;
        let backend = fixture.backend();
        let physical = &backend.collections().await.unwrap()[0];
        assert_eq!(backend.row_estimate(physical).await.unwrap(), Some(2));
    }

    #[tokio::test]
    async fn extent_is_the_union_of_live_data_file_bounds() {
        let fixture = TestFixture::build().await;
        let backend = fixture.backend();
        let physical = &backend.collections().await.unwrap()[0];
        let extent = backend.extent(physical).await.unwrap().unwrap();
        assert_eq!(extent.bbox, [-3.0, 48.0, -2.0, 49.0]);
    }

    #[tokio::test]
    async fn attribute_schema_excludes_geometry_and_bbox_columns() {
        let fixture = TestFixture::build().await;
        let backend = fixture.backend();
        let physical = &backend.collections().await.unwrap()[0];
        let columns = backend.attribute_schema(physical).await.unwrap().unwrap();
        let names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["id", "name", "observed_at"]);
        assert_eq!(columns[0].sql_type, "integer");
        assert_eq!(columns[1].sql_type, "text");
        assert_eq!(columns[2].sql_type, "timestamp with time zone");
    }

    #[tokio::test]
    async fn plan_files_with_no_bbox_lists_every_live_file() {
        let fixture = TestFixture::build().await;
        let backend = fixture.backend();
        let files = backend.plan_files(None, None).await.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].record_count, Some(2));
    }

    #[tokio::test]
    async fn plan_files_prunes_by_the_declared_bbox_columns() {
        let fixture = TestFixture::two_disjoint_files().await;
        let backend = fixture.backend();
        // Query bbox only overlaps the western file's stats.
        let files = backend
            .plan_files(Some([-5.0, 47.0, -1.0, 49.0]), None)
            .await
            .unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].record_count, Some(1));
    }

    #[tokio::test]
    async fn snapshot_pinning_ignores_a_later_append() {
        let fixture = TestFixture::build().await;
        let backend = fixture.backend();
        // Force the load (pins the snapshot as of the fixture's single append).
        let before = backend.plan_files(None, None).await.unwrap();
        assert_eq!(before.len(), 1);

        fixture.append_more_rows_on_disk_only().await;

        // Same already-loaded backend: still sees only the original file.
        let after = backend.plan_files(None, None).await.unwrap();
        assert_eq!(after, before);
    }

    #[tokio::test]
    async fn items_pages_through_every_row_with_a_stable_cursor() {
        let fixture = TestFixture::two_disjoint_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        let first = backend
            .items(
                &collection,
                &ItemsQuery {
                    limit: 1,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(first.features_geojson.len(), 1);
        assert_eq!(first.number_matched, Some(2));
        let token = first.next_token.clone().expect("a second page remains");

        let second = backend
            .items(
                &collection,
                &ItemsQuery {
                    limit: 1,
                    token: Some(token),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(second.features_geojson.len(), 1);
        assert_eq!(second.next_token, None);

        let first_id = first.features_geojson[0]["id"].as_str().unwrap();
        let second_id = second.features_geojson[0]["id"].as_str().unwrap();
        assert_ne!(first_id, second_id);
    }

    #[tokio::test]
    async fn items_bbox_filters_and_reports_an_exact_number_matched() {
        let fixture = TestFixture::two_disjoint_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        let page = backend
            .items(
                &collection,
                &ItemsQuery {
                    limit: 10,
                    bbox: Some([-5.0, 47.0, -1.0, 49.0]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.features_geojson.len(), 1);
        assert_eq!(page.number_matched, Some(1));
        assert_eq!(page.next_token, None);
        let properties = &page.features_geojson[0]["properties"];
        assert_eq!(properties["name"], serde_json::json!("west"));
    }

    #[tokio::test]
    async fn item_looks_up_a_single_feature_by_flat_position() {
        let fixture = TestFixture::two_disjoint_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        let feature = backend
            .item(&collection, "0", None)
            .await
            .unwrap()
            .expect("row 0 exists");
        assert_eq!(feature["id"], serde_json::json!("0"));

        let missing = backend.item(&collection, "999", None).await.unwrap();
        assert_eq!(missing, None);

        let non_integer = backend
            .item(&collection, "not-a-number", None)
            .await
            .unwrap();
        assert_eq!(non_integer, None);
    }

    #[tokio::test]
    async fn a_token_naming_a_different_snapshot_is_refused() {
        let fixture = TestFixture::build().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        let err = backend
            .items(
                &collection,
                &ItemsQuery {
                    limit: 1,
                    token: Some("999999:0000000000000000:0".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, CoreError::Invalid(ref message) if message.contains("999999")),
            "error was: {err}"
        );
    }

    #[tokio::test]
    async fn a_malformed_token_is_refused() {
        let fixture = TestFixture::build().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        let err = backend
            .items(
                &collection,
                &ItemsQuery {
                    limit: 1,
                    token: Some("not-a-token".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Invalid(_)));
    }

    #[tokio::test]
    async fn datetime_filtering_without_a_declared_column_is_refused() {
        let fixture = TestFixture::build().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();
        let err = backend
            .items(
                &collection,
                &ItemsQuery {
                    datetime: Some(tellurion_core::DatetimeRange {
                        start: Some("2020-01-01T00:00:00Z".to_string()),
                        end: None,
                    }),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Invalid(_)));
    }

    #[tokio::test]
    async fn datetime_filtering_with_a_wrong_typed_column_is_refused() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        // "id" is an Int column, not timestamptz.
        let collection = fixture.collection_decl_with_datetime("id");
        let err = backend
            .items(
                &collection,
                &ItemsQuery {
                    datetime: Some(tellurion_core::DatetimeRange {
                        start: Some("2020-01-01T00:00:00Z".to_string()),
                        end: None,
                    }),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, CoreError::Config(ref message) if message.contains("id") && message.contains("timestamptz")),
            "error was: {err}"
        );
    }

    #[tokio::test]
    async fn datetime_interval_filtering_returns_rows_within_the_range() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl_with_datetime("observed_at");

        let page = backend
            .items(
                &collection,
                &ItemsQuery {
                    limit: 10,
                    datetime: Some(tellurion_core::DatetimeRange {
                        start: Some("2020-05-01T00:00:00Z".to_string()),
                        end: Some("2021-02-01T00:00:00Z".to_string()),
                    }),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.number_matched, None);
        // `["id"]` is the position-based paging cursor, not the "id"
        // attribute — read the decoded attribute value from `properties`.
        let mut ids: Vec<i64> = page
            .features_geojson
            .iter()
            .map(|f| f["properties"]["id"].as_i64().unwrap())
            .collect();
        ids.sort_unstable();
        // id=2 (2020-06-01) and id=3 (2021-01-01) fall inside the window;
        // id=1 (2020-01-01) is too early and id=4 (2021-06-01) too late.
        assert_eq!(ids, vec![2, 3]);
    }

    #[tokio::test]
    async fn a_compare_filter_returns_exactly_the_matching_rows() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        let filter = tellurion_core::Filter::Compare {
            property: "id".to_string(),
            op: tellurion_core::CompareOp::Ge,
            value: tellurion_core::Literal::Number(3.0),
        };
        let page = backend
            .items(
                &collection,
                &ItemsQuery {
                    limit: 10,
                    filter: Some(filter),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.number_matched, None);
        let mut ids: Vec<i64> = page
            .features_geojson
            .iter()
            .map(|f| f["properties"]["id"].as_i64().unwrap())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![3, 4]);
    }

    #[tokio::test]
    async fn an_in_filter_returns_exactly_the_matching_rows() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        let filter = tellurion_core::Filter::In {
            property: "name".to_string(),
            values: vec![
                tellurion_core::Literal::Text("west-a".to_string()),
                tellurion_core::Literal::Text("east-b".to_string()),
            ],
            negated: false,
        };
        let page = backend
            .items(
                &collection,
                &ItemsQuery {
                    limit: 10,
                    filter: Some(filter),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let names: std::collections::BTreeSet<String> = page
            .features_geojson
            .iter()
            .map(|f| f["properties"]["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            names,
            std::collections::BTreeSet::from(["west-a".to_string(), "east-b".to_string()])
        );
    }

    #[tokio::test]
    async fn an_is_null_filter_returns_the_row_with_a_null_property() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        let filter = tellurion_core::Filter::IsNull {
            property: "name".to_string(),
            negated: false,
        };
        let page = backend
            .items(
                &collection,
                &ItemsQuery {
                    limit: 10,
                    filter: Some(filter),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.features_geojson.len(), 1);
        let properties = &page.features_geojson[0]["properties"];
        assert_eq!(properties["id"], serde_json::json!(3));
        assert_eq!(properties["name"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn a_filter_prunes_files_via_column_stats() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        // The "east" file's id stats are [3, 4]; they can never satisfy
        // `id = 1`, so the manifest evaluator prunes the whole file before
        // any Parquet read — only the "west" file's plan survives.
        let filter = tellurion_core::Filter::Compare {
            property: "id".to_string(),
            op: tellurion_core::CompareOp::Eq,
            value: tellurion_core::Literal::Number(1.0),
        };
        let files = backend.plan_files(None, Some(&filter)).await.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].record_count, Some(2));
    }

    #[tokio::test]
    async fn plan_cache_does_not_leak_between_different_filters() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();

        let filter_id_1 = tellurion_core::Filter::Compare {
            property: "id".to_string(),
            op: tellurion_core::CompareOp::Eq,
            value: tellurion_core::Literal::Number(1.0),
        };
        let filter_id_3 = tellurion_core::Filter::Compare {
            property: "id".to_string(),
            op: tellurion_core::CompareOp::Eq,
            value: tellurion_core::Literal::Number(3.0),
        };

        // Same bbox (None) both times — only the filter fingerprint differs.
        let files_for_1 = backend.plan_files(None, Some(&filter_id_1)).await.unwrap();
        assert!(
            files_for_1[0].path.contains("west"),
            "files: {files_for_1:?}"
        );

        let files_for_3 = backend.plan_files(None, Some(&filter_id_3)).await.unwrap();
        assert!(
            files_for_3[0].path.contains("east"),
            "files: {files_for_3:?}"
        );

        // Re-querying filter 1 still returns filter 1's own plan, not a
        // stale entry poisoned by the filter-3 lookup above.
        let files_for_1_again = backend.plan_files(None, Some(&filter_id_1)).await.unwrap();
        assert_eq!(files_for_1_again, files_for_1);
    }

    #[tokio::test]
    async fn an_unsupported_spatial_construct_is_refused_with_a_named_error() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        let filter = tellurion_core::Filter::Intersects {
            property: "geom".to_string(),
            geometry: tellurion_core::GeometryLiteral::Bbox([-1.0, -1.0, 1.0, 1.0]),
        };
        let err = backend
            .items(
                &collection,
                &ItemsQuery {
                    filter: Some(filter),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, CoreError::Invalid(ref message) if message.contains("S_INTERSECTS")),
            "error was: {err}"
        );
    }

    #[tokio::test]
    async fn an_unsupported_like_construct_is_refused_with_a_named_error() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        let filter = tellurion_core::Filter::Like {
            property: "name".to_string(),
            pattern: "west%".to_string(),
            negated: false,
        };
        let err = backend
            .items(
                &collection,
                &ItemsQuery {
                    filter: Some(filter),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, CoreError::Invalid(ref message) if message.contains("LIKE")),
            "error was: {err}"
        );
    }

    #[tokio::test]
    async fn paging_under_a_filter_is_stable_and_complete_across_pages() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        // Matches ids {1, 3, 4} — "west" file's own record_count (2) would
        // wrongly claim both its rows match if the per-file skip shortcut
        // were used, so this also exercises the correctness fix itself.
        let filter = tellurion_core::Filter::Compare {
            property: "id".to_string(),
            op: tellurion_core::CompareOp::Ne,
            value: tellurion_core::Literal::Number(2.0),
        };

        let mut seen: Vec<i64> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let page = backend
                .items(
                    &collection,
                    &ItemsQuery {
                        limit: 2,
                        filter: Some(filter.clone()),
                        token: token.clone(),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            seen.extend(
                page.features_geojson
                    .iter()
                    .map(|f| f["properties"]["id"].as_i64().unwrap()),
            );
            match page.next_token {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        seen.sort_unstable();
        assert_eq!(seen, vec![1, 3, 4]);
    }

    #[tokio::test]
    async fn a_token_minted_under_one_filter_is_refused_under_another() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        let filter_a = tellurion_core::Filter::Compare {
            property: "id".to_string(),
            op: tellurion_core::CompareOp::Ge,
            value: tellurion_core::Literal::Number(1.0),
        };
        let first = backend
            .items(
                &collection,
                &ItemsQuery {
                    limit: 1,
                    filter: Some(filter_a),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let token = first.next_token.expect("more rows remain under filter A");

        // Resume with no filter at all: the fingerprint no longer matches.
        let err = backend
            .items(
                &collection,
                &ItemsQuery {
                    limit: 1,
                    token: Some(token),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Invalid(_)), "error was: {err}");
    }

    #[tokio::test]
    async fn item_filter_matching_row_is_returned() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        // Planned tasks sort by data file path ("data-east-..." before
        // "data-west-..."), so position "0" is the east file's first row,
        // id=3 — this filter matches it.
        let filter = tellurion_core::Filter::Compare {
            property: "id".to_string(),
            op: tellurion_core::CompareOp::Eq,
            value: tellurion_core::Literal::Number(3.0),
        };
        let feature = backend
            .item(&collection, "0", Some(&filter))
            .await
            .unwrap()
            .expect("row 0 satisfies the filter");
        assert_eq!(feature["id"], serde_json::json!("0"));
        assert_eq!(feature["properties"]["id"], serde_json::json!(3));
    }

    #[tokio::test]
    async fn item_filter_excluding_row_is_indistinguishable_from_a_missing_id() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        // Position "0" is the east file's first row, id=3 — this filter
        // does not match it, so the row exists but is excluded.
        let filter = tellurion_core::Filter::Compare {
            property: "id".to_string(),
            op: tellurion_core::CompareOp::Eq,
            value: tellurion_core::Literal::Number(1.0),
        };
        let excluded = backend.item(&collection, "0", Some(&filter)).await.unwrap();

        // A position that names no row at all, under no filter.
        let missing = backend.item(&collection, "999", None).await.unwrap();

        // Both come back as exactly the same value — `Ok(None)` — matching
        // `tellurion-postgis`'s own single-item lookup, where a `WHERE
        // pk = $1 AND (filter)` that a row fails to satisfy and a `WHERE
        // pk = $1` that matches no row both make `query_opt` return no row
        // at all.
        assert_eq!(excluded, None);
        assert_eq!(excluded, missing);
    }

    #[tokio::test]
    async fn item_filter_construct_outside_the_boundary_is_refused_by_name() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        let filter = tellurion_core::Filter::Like {
            property: "name".to_string(),
            pattern: "west%".to_string(),
            negated: false,
        };

        let item_err = backend
            .item(&collection, "0", Some(&filter))
            .await
            .unwrap_err();
        assert!(
            matches!(item_err, CoreError::Invalid(ref message) if message.contains("LIKE")),
            "error was: {item_err}"
        );

        // Parity: `items` refuses the identical filter with the identical
        // message — both paths share the same named-refusal helper.
        let items_err = backend
            .items(
                &collection,
                &ItemsQuery {
                    filter: Some(filter),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(item_err.to_string(), items_err.to_string());
    }

    #[tokio::test]
    async fn item_filter_type_mismatch_is_refused_by_name() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        // "id" is an Int column; a text literal has no compilable mapping
        // onto it.
        let filter = tellurion_core::Filter::Compare {
            property: "id".to_string(),
            op: tellurion_core::CompareOp::Eq,
            value: tellurion_core::Literal::Text("not-a-number".to_string()),
        };

        let item_err = backend
            .item(&collection, "0", Some(&filter))
            .await
            .unwrap_err();
        assert!(
            matches!(item_err, CoreError::Invalid(ref message) if message.contains("id")),
            "error was: {item_err}"
        );

        let items_err = backend
            .items(
                &collection,
                &ItemsQuery {
                    filter: Some(filter),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(item_err.to_string(), items_err.to_string());
    }

    #[tokio::test]
    async fn item_filter_datetime_interval_construct_is_refused_matching_items() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        // `T_DURING` is CQL2's own datetime-column interval predicate
        // (`During { property, start, end }`). `compile_predicate` refuses
        // every temporal function by name (see this module's "CQL2 and
        // datetime pushdown" docs) — `item`'s in-process evaluator must
        // refuse it identically, never accept it just because there is no
        // scan to push it into.
        let filter = tellurion_core::Filter::During {
            property: DATETIME_COLUMN.to_string(),
            start: "2020-01-01T00:00:00Z".to_string(),
            end: "2020-12-31T00:00:00Z".to_string(),
        };

        let item_err = backend
            .item(&collection, "0", Some(&filter))
            .await
            .unwrap_err();
        assert!(
            matches!(item_err, CoreError::Invalid(ref message) if message.contains("T_DURING")),
            "error was: {item_err}"
        );

        let items_err = backend
            .items(
                &collection,
                &ItemsQuery {
                    filter: Some(filter),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(item_err.to_string(), items_err.to_string());
    }

    #[tokio::test]
    async fn item_filter_null_property_row_behavior() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        // Position "0" (id=3, east file's first row) has `name = None`;
        // position "2" (id=1, west file's first row) has `name = "west-a"`.
        let is_null = tellurion_core::Filter::IsNull {
            property: "name".to_string(),
            negated: false,
        };
        let matched = backend
            .item(&collection, "0", Some(&is_null))
            .await
            .unwrap();
        assert!(matched.is_some(), "the null-name row satisfies IS NULL");

        let non_null_row = backend
            .item(&collection, "2", Some(&is_null))
            .await
            .unwrap();
        assert_eq!(non_null_row, None, "a non-null name excludes the row");

        // SQL three-valued logic: a comparison against a NULL column value
        // is "unknown", not `false` — it still excludes the row (`Ok(None)`
        // from `item`'s point of view), but for a different reason than an
        // ordinary non-match. Prove it agrees with what a filtered `items`
        // scan returns for the same rows.
        let compare_against_null = tellurion_core::Filter::Compare {
            property: "name".to_string(),
            op: tellurion_core::CompareOp::Eq,
            value: tellurion_core::Literal::Text("west-a".to_string()),
        };
        let excluded_by_unknown = backend
            .item(&collection, "0", Some(&compare_against_null))
            .await
            .unwrap();
        assert_eq!(excluded_by_unknown, None);

        // Compare by the row's own `id` *attribute* (`properties.id`)
        // rather than `items`' reported GeoJSON `id` — both now name the
        // same flat position as `item` (see this module's "pk / cursor
        // mapping" docs), but the attribute is the more direct way to ask
        // "is this same real row present in both results" here.
        let items_page = backend
            .items(
                &collection,
                &ItemsQuery {
                    limit: 10,
                    filter: Some(compare_against_null),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let matching_attribute_ids: std::collections::BTreeSet<i64> = items_page
            .features_geojson
            .iter()
            .map(|f| f["properties"]["id"].as_i64().unwrap())
            .collect();
        assert!(
            !matching_attribute_ids.contains(&3),
            "items() must also exclude the null-name row (id=3) for this filter"
        );
        assert!(
            matching_attribute_ids.contains(&1),
            "items() matches the one row whose name really is 'west-a' (id=1)"
        );
    }

    #[tokio::test]
    async fn item_filter_visibility_matches_items_filter_membership() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        let filter = tellurion_core::Filter::Compare {
            property: "id".to_string(),
            op: tellurion_core::CompareOp::Ge,
            value: tellurion_core::Literal::Number(3.0),
        };

        // The row's own `id` attribute (`properties.id`) present at each
        // flat position, read unfiltered — the stable identity `item`'s
        // per-position filter result is compared against.
        let mut attribute_id_at_position = Vec::new();
        for position in 0..4u64 {
            let feature = backend
                .item(&collection, &position.to_string(), None)
                .await
                .unwrap()
                .expect("fixture has exactly 4 rows");
            attribute_id_at_position.push(feature["properties"]["id"].as_i64().unwrap());
        }

        let items_page = backend
            .items(
                &collection,
                &ItemsQuery {
                    limit: 10,
                    filter: Some(filter.clone()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let matching_attribute_ids: std::collections::BTreeSet<i64> = items_page
            .features_geojson
            .iter()
            .map(|f| f["properties"]["id"].as_i64().unwrap())
            .collect();

        // Every position this fixture actually has a row at (0..=3) —
        // `item`'s own visibility under `filter` must agree exactly with
        // whether `items(filter)` yielded that same real row.
        for position in 0..4u64 {
            let attribute_id = attribute_id_at_position[position as usize];
            let seen_by_item = backend
                .item(&collection, &position.to_string(), Some(&filter))
                .await
                .unwrap()
                .is_some();
            let seen_by_items = matching_attribute_ids.contains(&attribute_id);
            assert_eq!(
                seen_by_item, seen_by_items,
                "position {position} (id={attribute_id}) disagreed: \
                 item()={seen_by_item} items()={seen_by_items}"
            );
        }
    }

    #[tokio::test]
    async fn unfiltered_items_id_round_trips_through_item() {
        // This driver never has a real relational primary-key column to
        // address a row by (see this module's "pk / cursor mapping" docs)
        // — every id is the synthetic flat-position `fid`, on every read
        // surface. This covers the plain, unfiltered surface: an id
        // harvested from a page with no bbox/filter/datetime at all must
        // round-trip through `item` to the exact same row, exactly like
        // the filtered surfaces the tests below cover.
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        let page = backend
            .items(
                &collection,
                &ItemsQuery {
                    limit: 10,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.features_geojson.len(), 4);

        for feature in &page.features_geojson {
            let id = feature["id"].as_str().unwrap();
            let looked_up = backend
                .item(&collection, id, None)
                .await
                .unwrap()
                .expect("an id items() emitted must resolve via item()");
            assert_eq!(
                looked_up, *feature,
                "item() must return the exact same feature items() emitted for id {id}"
            );
        }
    }

    #[tokio::test]
    async fn filtered_items_id_round_trips_through_item() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        // Matches ids {2, 3} — the east file's first row (flat position 0)
        // and the west file's second row (flat position 3), skipping the
        // two rows in between. A naive matching-row-count id (0, 1) would
        // address a completely different pair of rows than the ones
        // actually returned, so this also exercises the fix itself.
        let filter = tellurion_core::Filter::In {
            property: "id".to_string(),
            values: vec![
                tellurion_core::Literal::Number(3.0),
                tellurion_core::Literal::Number(2.0),
            ],
            negated: false,
        };

        let page = backend
            .items(
                &collection,
                &ItemsQuery {
                    limit: 10,
                    filter: Some(filter),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.features_geojson.len(), 2);

        for feature in &page.features_geojson {
            let id = feature["id"].as_str().unwrap();
            let expected_attribute_id = feature["properties"]["id"].as_i64().unwrap();

            // Feed the id straight back into `item`, unfiltered — it must
            // address the exact same physical row `items` just emitted,
            // asserted by attribute equality (the row's own `id` property,
            // not the position-derived GeoJSON id being compared against
            // itself).
            let looked_up = backend
                .item(&collection, id, None)
                .await
                .unwrap()
                .expect("an id items() just emitted must resolve via item()");
            assert_eq!(
                looked_up["properties"]["id"].as_i64().unwrap(),
                expected_attribute_id,
                "id {id} from the filtered page addressed a different row via item()"
            );
            assert_eq!(looked_up["geometry"], feature["geometry"]);
        }
    }

    #[tokio::test]
    async fn filtered_items_ids_are_stable_and_complete_across_pages() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        let filter = tellurion_core::Filter::Compare {
            property: "id".to_string(),
            op: tellurion_core::CompareOp::Ne,
            value: tellurion_core::Literal::Number(2.0),
        };

        // One page, no paging at all — the reference set of (geojson id,
        // properties.id) pairs a correctly paged walk must reproduce
        // exactly.
        let whole = backend
            .items(
                &collection,
                &ItemsQuery {
                    limit: 10,
                    filter: Some(filter.clone()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let expected: std::collections::BTreeMap<String, i64> = whole
            .features_geojson
            .iter()
            .map(|f| {
                (
                    f["id"].as_str().unwrap().to_string(),
                    f["properties"]["id"].as_i64().unwrap(),
                )
            })
            .collect();
        assert_eq!(expected.len(), 3, "filter matches exactly 3 of the 4 rows");

        // Page size 1 — strictly smaller than the 3-row match count —
        // walking every page.
        let mut paged: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
        let mut token: Option<String> = None;
        let mut page_count = 0;
        loop {
            let page = backend
                .items(
                    &collection,
                    &ItemsQuery {
                        limit: 1,
                        filter: Some(filter.clone()),
                        token: token.clone(),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            page_count += 1;
            assert!(page_count <= 10, "paging never converged");
            for feature in &page.features_geojson {
                let id = feature["id"].as_str().unwrap().to_string();
                let attribute_id = feature["properties"]["id"].as_i64().unwrap();
                assert!(
                    paged.insert(id.clone(), attribute_id).is_none(),
                    "id {id} was emitted more than once across pages"
                );
            }
            match page.next_token {
                Some(next) => token = Some(next),
                None => break,
            }
        }

        assert_eq!(
            page_count, 3,
            "3 matches at page size 1 should take exactly 3 pages"
        );
        assert_eq!(
            paged, expected,
            "paged ids/attributes must exactly match the single-page walk — no skip or duplicate"
        );

        // Every id, harvested from whichever page it appeared on, still
        // round-trips through `item` to the same row.
        for (id, attribute_id) in &paged {
            let looked_up = backend
                .item(&collection, id, None)
                .await
                .unwrap()
                .expect("a paged id must resolve via item()");
            assert_eq!(
                looked_up["properties"]["id"].as_i64().unwrap(),
                *attribute_id
            );
        }
    }

    #[tokio::test]
    async fn datetime_only_filtered_items_id_round_trips_through_item() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl_with_datetime("observed_at");

        // Matches id=3 (2021-01-01, east file's first row, position 0) and
        // id=2 (2020-06-01, west file's second row, position 3) — a
        // `datetime`-only query (no CQL2 `filter`) goes through the same
        // in-process, flat-position-preserving path a `filter`-active call
        // does (see this module's "CQL2 and datetime pushdown" docs).
        let page = backend
            .items(
                &collection,
                &ItemsQuery {
                    limit: 10,
                    datetime: Some(tellurion_core::DatetimeRange {
                        start: Some("2020-05-01T00:00:00Z".to_string()),
                        end: Some("2021-02-01T00:00:00Z".to_string()),
                    }),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.features_geojson.len(), 2);

        for feature in &page.features_geojson {
            let id = feature["id"].as_str().unwrap();
            let expected_attribute_id = feature["properties"]["id"].as_i64().unwrap();
            // `item` has no `datetime` parameter of its own (see this
            // module's "What this slice does not do" docs) — it can't
            // re-verify the interval, only that the id still addresses the
            // same physical row `items` emitted it for.
            let looked_up = backend
                .item(&collection, id, None)
                .await
                .unwrap()
                .expect("an id items() just emitted must resolve via item()");
            assert_eq!(
                looked_up["properties"]["id"].as_i64().unwrap(),
                expected_attribute_id
            );
        }
    }

    #[tokio::test]
    async fn bbox_and_filter_combine_in_process_and_keep_the_flat_position_id() {
        let fixture = TestFixture::four_rows_two_files().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        // `bbox` keeps only the west file's two rows (flat positions 2 and
        // 3); the filter then excludes id=1 (position 2), leaving exactly
        // id=2 (position 3) — proving `bbox` and a CQL2 filter combine
        // correctly in-process, and the surviving row keeps its true flat
        // position as its id rather than being renumbered to "0".
        let filter = tellurion_core::Filter::Compare {
            property: "id".to_string(),
            op: tellurion_core::CompareOp::Ne,
            value: tellurion_core::Literal::Number(1.0),
        };
        let page = backend
            .items(
                &collection,
                &ItemsQuery {
                    limit: 10,
                    bbox: Some([-5.0, 47.0, -1.0, 49.0]),
                    filter: Some(filter),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.features_geojson.len(), 1);
        assert_eq!(page.features_geojson[0]["id"], serde_json::json!("3"));
        assert_eq!(
            page.features_geojson[0]["properties"]["id"],
            serde_json::json!(2)
        );

        let looked_up = backend
            .item(&collection, "3", None)
            .await
            .unwrap()
            .expect("position 3 exists");
        assert_eq!(looked_up["properties"]["id"], serde_json::json!(2));
    }

    #[tokio::test]
    async fn filter_capable_reports_true() {
        let fixture = TestFixture::build().await;
        let backend = fixture.backend();
        assert!(FeatureSource::filter_capable(&backend));
    }

    /// `#105`: this module's "CQL2 and datetime pushdown" doc names the
    /// exact subset `compile_predicate` compiles — comparison, `IS [NOT]
    /// NULL`, `[NOT] IN`, `AND`/`OR`/`NOT` — and refuses every spatial and
    /// temporal predicate, plus `LIKE`/`BETWEEN`, by name. So this driver
    /// declares only Basic CQL2 plus both encodings; every richer class
    /// (including `basic-spatial-functions`, which the pre-`#105`
    /// workspace-wide list declared unconditionally without checking this
    /// driver actually compiled `S_INTERSECTS`) stays undeclared.
    #[tokio::test]
    async fn cql2_conformance_classes_pins_basic_cql2_and_both_encodings_only() {
        let fixture = TestFixture::build().await;
        let backend = fixture.backend();
        let declared = FeatureSource::cql2_conformance_classes(&backend);
        assert_eq!(
            declared,
            vec![
                tellurion_core::filter::CQL2_CLASS_BASIC,
                tellurion_core::filter::CQL2_CLASS_CQL2_TEXT,
                tellurion_core::filter::CQL2_CLASS_CQL2_JSON,
            ]
        );
        assert!(!declared.contains(&tellurion_core::filter::CQL2_CLASS_BASIC_SPATIAL_FUNCTIONS));
        assert!(
            !declared.contains(&tellurion_core::filter::CQL2_CLASS_ADVANCED_COMPARISON_OPERATORS)
        );
        assert!(!declared.contains(&tellurion_core::filter::CQL2_CLASS_SPATIAL_FUNCTIONS));
        assert!(!declared.contains(&tellurion_core::filter::CQL2_CLASS_TEMPORAL_FUNCTIONS));
        assert!(!declared.contains(&tellurion_core::filter::CQL2_CLASS_CASE_INSENSITIVE_COMPARISON));
        assert_eq!(
            FeatureSource::filter_capable(&backend),
            !declared.is_empty()
        );
    }

    #[tokio::test]
    async fn missing_table_is_a_precise_refusal() {
        let fixture = TestFixture::build().await;
        let location = fixture.location_for_missing_table("does-not-exist");
        let backend = IcebergBackend::new(location);
        let err = backend.collections().await.unwrap_err();
        assert!(matches!(err, CoreError::Storage(_)), "error was: {err}");
    }

    #[tokio::test]
    async fn missing_geometry_column_is_a_precise_refusal() {
        let fixture = TestFixture::build().await;
        let location = fixture.location_with_geometry_column("not_a_column");
        let backend = IcebergBackend::new(location);
        let err = backend.collections().await.unwrap_err();
        assert!(
            matches!(err, CoreError::Config(ref message) if message.contains("not_a_column")),
            "error was: {err}"
        );
    }

    #[tokio::test]
    async fn wrong_typed_geometry_column_is_a_precise_refusal() {
        let fixture = TestFixture::build().await;
        let location = fixture.location_with_geometry_column("id");
        let backend = IcebergBackend::new(location);
        let err = backend.collections().await.unwrap_err();
        assert!(
            matches!(err, CoreError::Config(ref message) if message.contains("id") && message.contains("expected 'binary'")),
            "error was: {err}"
        );
    }

    #[tokio::test]
    async fn wrong_typed_bbox_column_is_a_precise_refusal() {
        let fixture = TestFixture::build().await;
        let location = fixture.location_with_bbox_xmin("name");
        let backend = IcebergBackend::new(location);
        let err = backend.collections().await.unwrap_err();
        assert!(
            matches!(err, CoreError::Config(ref message) if message.contains("name") && message.contains("expected 'double' or 'float'")),
            "error was: {err}"
        );
    }

    #[tokio::test]
    async fn missing_bbox_column_is_a_precise_refusal() {
        let fixture = TestFixture::build().await;
        let location = fixture.location_with_bbox_xmin("not_a_column");
        let backend = IcebergBackend::new(location);
        let err = backend.collections().await.unwrap_err();
        assert!(
            matches!(err, CoreError::Config(ref message) if message.contains("not_a_column")),
            "error was: {err}"
        );
    }

    #[test]
    fn point_wkb_is_a_well_formed_iso_wkb_point() {
        // Sanity check on the fixture helper itself: 21 bytes, little-
        // endian point (type 1), coordinates round-trip through geozero.
        let wkb = point_wkb(-3.5, 48.25);
        assert_eq!(wkb.len(), 21);
        let geojson = geometry_json_from_wkb(&wkb).unwrap();
        assert_eq!(geojson["type"], serde_json::json!("Point"));
        assert_eq!(geojson["coordinates"], serde_json::json!([-3.5, 48.25]));
    }

    #[test]
    fn factory_name_matches_the_config_driver_key() {
        assert_eq!(IcebergDriverFactory::new().name(), "iceberg");
    }

    // -----------------------------------------------------------------
    // Object-store `FileIO` (`#123`)
    // -----------------------------------------------------------------

    /// The acceptance claim of `#123`: a locator whose table lives on an
    /// object store opens and SERVES, end to end, with every byte fetched
    /// over the S3 protocol rather than off local disk.
    #[tokio::test]
    async fn a_table_on_s3_is_served_end_to_end_over_the_object_store() {
        let fixture = S3TestFixture::build().await;
        let backend = fixture.backend();
        let collection = fixture.collection_decl();

        // Catalog introspection: identity, extent and row estimate all
        // derive from manifests this driver had to fetch from the store.
        let physical = &backend.collections().await.unwrap()[0];
        assert_eq!(physical.name, "points");
        assert_eq!(backend.row_estimate(physical).await.unwrap(), Some(2));
        assert_eq!(
            backend.extent(physical).await.unwrap().unwrap().bbox,
            [-3.0, 48.0, -2.0, 49.0]
        );

        // And the feature lane: real Parquet row decode, out of a data file
        // read over HTTP with ranged GETs.
        let page = backend
            .items(&collection, &ItemsQuery::default())
            .await
            .unwrap();
        assert_eq!(page.features_geojson.len(), 2);
        assert_eq!(page.number_matched, Some(2));
        // Compared as numbers rather than as `Value`s: GeoJSON writes a
        // whole coordinate as `-3`, which parses back as an integer
        // `Number`, and `Number`'s own `PartialEq` does not equate that
        // with the float `-3.0` a `json!` literal produces.
        let mut points: Vec<(f64, f64)> = page
            .features_geojson
            .iter()
            .map(|feature| {
                let coordinates = &feature["geometry"]["coordinates"];
                (
                    coordinates[0].as_f64().unwrap(),
                    coordinates[1].as_f64().unwrap(),
                )
            })
            .collect();
        points.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(points, vec![(-3.0, 48.0), (-2.0, 49.0)]);

        // None of that could have come off local disk: every object was
        // fetched from the store, under the bucket the table metadata
        // names.
        let requests = fixture.s3_requests();
        assert!(
            !requests.is_empty(),
            "the driver never contacted the object store at all"
        );
        assert!(
            requests
                .iter()
                .all(|request| request.path.starts_with(&format!("/{}/", S3_BUCKET))),
            "every request must address the metadata's own bucket, got: {requests:#?}"
        );
        assert!(
            requests
                .iter()
                .any(|request| request.path.ends_with(".parquet")),
            "a served feature implies a data-file read, got: {requests:#?}"
        );
        // Read-only on the wire too, not merely by intent: this driver
        // never issues a mutating S3 verb against a table's storage.
        assert!(
            requests
                .iter()
                .all(|request| request.method == "GET" || request.method == "HEAD"),
            "the iceberg driver must only ever read; got: {requests:#?}"
        );
    }

    /// Parquet is read with ranged GETs (footer first, then column chunks),
    /// not by pulling whole objects — the property that makes an
    /// object-store table affordable at all, and the one thing a
    /// whole-object-only `FileIO` would silently get wrong.
    #[tokio::test]
    async fn parquet_data_files_are_read_with_ranged_gets() {
        let fixture = S3TestFixture::build().await;
        let backend = fixture.backend();
        backend
            .items(&fixture.collection_decl(), &ItemsQuery::default())
            .await
            .unwrap();

        let ranged: Vec<_> = fixture
            .s3_requests()
            .into_iter()
            .filter(|request| request.path.ends_with(".parquet") && request.range.is_some())
            .collect();
        assert!(
            !ranged.is_empty(),
            "no ranged GET reached the store; the Parquet reader would be pulling whole objects"
        );
        assert!(
            ranged
                .iter()
                .all(|request| request.range.as_deref().unwrap().starts_with("bytes=")),
            "got: {ranged:#?}"
        );
    }

    /// Every request is SigV4-signed, with the credential the locator NAMED
    /// an environment variable for — never a credential from `config.yaml`,
    /// which has no field to hold one.
    #[tokio::test]
    async fn every_object_store_request_is_sigv4_signed_with_the_env_supplied_credential() {
        let fixture = S3TestFixture::build().await;
        let backend = fixture.backend();
        backend
            .items(&fixture.collection_decl(), &ItemsQuery::default())
            .await
            .unwrap();

        let requests = fixture.s3_requests();
        assert!(!requests.is_empty());
        for request in &requests {
            let authorization = request
                .authorization
                .as_deref()
                .unwrap_or_else(|| panic!("unsigned request: {request:#?}"));
            assert!(
                authorization.starts_with("AWS4-HMAC-SHA256 "),
                "got: {authorization}"
            );
            assert!(
                authorization.contains(&format!("Credential={}/", fixture.access_key())),
                "the signature must carry the access key the locator's own env var held, got: \
                 {authorization}"
            );
            assert!(
                authorization.contains(&format!("/{}/s3/aws4_request", fixture.region())),
                "the credential scope must carry the declared region, got: {authorization}"
            );
            assert!(
                authorization.contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date"),
                "got: {authorization}"
            );
        }
    }

    /// THE refusal this slice exists to make impossible to get wrong: a
    /// table on GCS is named and refused at load, before anything is
    /// served. Never a silent fallback to the local filesystem, never a
    /// generic storage error.
    #[tokio::test]
    async fn a_table_on_gcs_is_refused_by_name_at_load() {
        for scheme in ["gs", "gcs"] {
            let fixture = S3TestFixture::on_scheme(scheme).await;
            let backend = fixture.backend();
            let err = backend.collections().await.unwrap_err();
            let CoreError::Config(message) = &err else {
                panic!("a boot-time storage refusal must be an Error::Config, got: {err:?}");
            };
            assert!(
                message.contains(&format!("'{scheme}://'")),
                "the refusal must name the scheme that was configured, got: {message}"
            );
            assert!(
                message.contains("Google Cloud Storage"),
                "the refusal must name the product, got: {message}"
            );
            assert!(
                message.contains("not supported"),
                "the refusal must say it is not supported, got: {message}"
            );
        }
    }

    #[tokio::test]
    async fn a_table_on_adls_is_refused_by_name_at_load() {
        for (scheme, product) in [
            ("abfss", "Azure Data Lake Storage"),
            ("abfs", "Azure Data Lake Storage"),
            ("wasbs", "Azure Blob Storage"),
        ] {
            let fixture = S3TestFixture::on_scheme(scheme).await;
            let backend = fixture.backend();
            let err = backend.collections().await.unwrap_err();
            let CoreError::Config(message) = &err else {
                panic!("a boot-time storage refusal must be an Error::Config, got: {err:?}");
            };
            assert!(
                message.contains(&format!("'{scheme}://'"))
                    && message.contains(product)
                    && message.contains("not supported"),
                "got: {message}"
            );
        }
    }

    /// An unsupported location is refused on EVERY surface, not just the
    /// one that happens to touch storage first — `collections`, `extent`
    /// and `items` all fail the same way, because the refusal sits in the
    /// one load path all three share.
    #[tokio::test]
    async fn an_unsupported_scheme_refuses_on_every_read_surface() {
        let fixture = S3TestFixture::on_scheme("gs").await;
        let backend = fixture.backend();
        assert!(matches!(
            backend.collections().await,
            Err(CoreError::Config(_))
        ));
        assert!(matches!(
            backend
                .items(&fixture.collection_decl(), &ItemsQuery::default())
                .await,
            Err(CoreError::Config(_))
        ));
        assert!(matches!(
            backend.item(&fixture.collection_decl(), "0", None).await,
            Err(CoreError::Config(_))
        ));
    }

    /// A scheme this driver has never heard of is refused too, rather than
    /// being optimistically treated as a filesystem path — which is how a
    /// misconfigured locator turns into a successful read of some unrelated
    /// local file.
    #[tokio::test]
    async fn an_unrecognized_scheme_is_refused_rather_than_read_as_a_local_path() {
        let fixture = S3TestFixture::on_scheme("hdfs").await;
        let err = fixture.backend().collections().await.unwrap_err();
        let CoreError::Config(message) = &err else {
            panic!("got: {err:?}");
        };
        assert!(message.contains("'hdfs://'"), "got: {message}");
    }

    /// A table that IS on S3 with an incomplete `s3_*` locator refuses by
    /// naming the missing key — never by guessing an endpoint or a region.
    #[tokio::test]
    async fn an_s3_table_with_an_incomplete_locator_names_the_missing_key() {
        let fixture = S3TestFixture::build().await;
        for dropped in [
            "s3_endpoint",
            "s3_region",
            "s3_access_key_env",
            "s3_secret_key_env",
        ] {
            let backend = IcebergBackend::new(fixture.location_without(dropped));
            let err = backend.collections().await.unwrap_err();
            let CoreError::Config(message) = &err else {
                panic!("got: {err:?}");
            };
            assert!(
                message.contains(dropped),
                "the refusal must name the missing locator key, got: {message}"
            );
            assert!(
                message.contains("config.yaml"),
                "the refusal must say where the setting belongs, got: {message}"
            );
        }
    }

    /// A locator naming an environment variable that is not set refuses by
    /// naming the VARIABLE — and the message never contains a credential,
    /// because there is none to contain.
    #[tokio::test]
    async fn an_unset_credential_environment_variable_is_refused_by_name() {
        // No fixture, and deliberately an unroutable catalog URI: credential
        // resolution happens BEFORE the catalog is contacted, so this refuses
        // without any server existing at all. Mutate `resolve_s3_connection`
        // to invent a credential instead of reading the named variable and
        // this test fails with a connection error — which is the point.
        let raw = "http://127.0.0.1:1?namespace=geo&table=points&geometry=geom\
                   &bbox=bbox_xmin,bbox_ymin,bbox_xmax,bbox_ymax\
                   &s3_endpoint=http://127.0.0.1:1&s3_region=us-east-1\
                   &s3_access_key_env=TELLURION_ICEBERG_TEST_NEVER_SET_ACCESS_KEY\
                   &s3_secret_key_env=TELLURION_ICEBERG_TEST_NEVER_SET_SECRET_KEY";
        let backend = IcebergBackend::new(IcebergLocation::parse(raw).unwrap());
        let err = backend.collections().await.unwrap_err();
        let CoreError::Config(message) = &err else {
            panic!("got: {err:?}");
        };
        assert!(
            message.contains("TELLURION_ICEBERG_TEST_NEVER_SET_ACCESS_KEY"),
            "got: {message}"
        );
    }

    /// The unchanged-behaviour half of this slice: a local-filesystem table
    /// whose locator declares NO `s3_*` keys still loads and serves exactly
    /// as it did before `#123`, and never touches an object store.
    #[tokio::test]
    async fn a_local_filesystem_table_still_loads_with_no_s3_declarations_at_all() {
        let fixture = TestFixture::build().await;
        assert_eq!(fixture.location().s3, None);
        let backend = fixture.backend();
        let page = backend
            .items(&fixture.collection_decl(), &ItemsQuery::default())
            .await
            .unwrap();
        assert_eq!(page.features_geojson.len(), 2);
    }

    #[test]
    fn build_fails_fast_when_the_env_var_is_unset() {
        let factory = IcebergDriverFactory::new();
        let decl = StorageDecl {
            id: "main".to_string(),
            driver: "iceberg".to_string(),
            url_env: "TELLURION_ICEBERG_TEST_DOES_NOT_EXIST".to_string(),
            pool_size: None,
        };
        std::env::remove_var(&decl.url_env);
        assert!(matches!(factory.build(&decl), Err(CoreError::Config(_))));
    }
}
