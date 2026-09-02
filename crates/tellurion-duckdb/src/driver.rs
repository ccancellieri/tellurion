//! The `duckdb` `DriverFactory`, and the `CatalogSource` + `FeatureSource`
//! implementation backing it. Read-only: a `.duckdb` file is opened
//! read-only, there is no write path, no DDL, nothing beyond what the driver
//! contract's mandatory `CatalogSource` plus the optional `FeatureSource`
//! capability require. `TileSource` is never implemented — a collection
//! routed to a `duckdb` storage on the `tiles` lane fails at boot with the
//! router's ordinary missing-capability error, the same shape every other
//! read-only file-backed driver in this workspace takes.
//!
//! ## Storage config
//!
//! A `duckdb` storage reuses `StorageDecl.url_env` exactly as `geopackage`/
//! `flatgeobuf`/`geoparquet` do: the named environment variable holds the
//! `.duckdb` file's local filesystem path.
//!
//! ## EXTENSION note: why this driver never loads the `spatial` extension
//!
//! `duckdb-rs`'s `bundled` feature statically compiles DuckDB's own core
//! engine from vendored source — no system `libduckdb`, no network fetch,
//! matching this driver's whole point (a single binary plus a single
//! `.duckdb` file, nothing else installed). It does **not** vendor any
//! DuckDB *extension*: `spatial` (the community extension that would add a
//! native `GEOMETRY` type, `ST_*` functions, and `INSTALL`/`LOAD`-able
//! `.duckdb_extension` binaries) ships and updates independently of DuckDB's
//! own release cycle, fetched over the network from DuckDB's extension
//! repository the first time a deployment runs `INSTALL spatial` — there is
//! no way to compile it statically into this crate the way `bundled` does
//! for the core engine (verified against `duckdb-rs` 1.10504.0's own
//! `Cargo.toml`: its feature list has no `spatial`/geometry-extension
//! feature of any kind, only `json`/`parquet`/`icu`/`autocomplete`, each of
//! those themselves core-adjacent, statically-bundleable extensions —
//! `spatial` is not among them). Depending on it here would mean either a
//! hidden network dependency on first boot (directly against this repo's own
//! "single binary next to a data file, no external service" positioning) or
//! requiring every operator to pre-provision an extension directory by hand
//! before this driver could open their file at all.
//!
//! This driver's decision: **never** load `spatial`, in any form, at any
//! version. A collection's geometry column is a plain `BLOB` holding raw ISO
//! WKB — decoded entirely by this crate's own `geozero` dependency (the same
//! WKB decode path `tellurion-geoparquet` already uses for its own WKB
//! geometry column), never by an engine-side geometry type. Every test in
//! this crate — including the fixture-based driver tests and the
//! server-level end-to-end proof — runs with **zero** network access,
//! unconditionally: there is no spatial-dependent test to gate behind an
//! opt-in environment variable, because no code path in this driver ever
//! attempts to reach one. A future slice could reconsider this once a
//! genuinely offline-installable spatial extension distribution exists;
//! parquet/httpfs attach (this issue's own named follow-ups) would face the
//! identical network-dependency question independently.
//!
//! ## Multi-collection model and geometry-column auto-detection
//!
//! Unlike FlatGeobuf/GeoParquet (one file, one self-describing collection),
//! a `.duckdb` file is a real database that may hold many tables — this
//! driver's `CatalogSource::collections` enumerates every one of them, the
//! same multi-collection-per-storage shape `tellurion-geopackage`/
//! `tellurion-postgis` already take. A plain DuckDB catalog has no
//! `gpkg_geometry_columns`-style registry naming "the" geometry column,
//! though, so this driver applies its own documented convention — see
//! `catalog.rs`'s own module doc for the exact "exactly one `BLOB` column,
//! else ambiguous" rule, and `tellurion_core::descriptor::
//! require_feature_capable`/`merge_descriptor` for how an ambiguous or
//! missing derivation simply requires the operator to pin
//! `CollectionDecl.geometry` explicitly rather than failing outright.
//!
//! ## pk / cursor mapping
//!
//! Unlike FlatGeobuf/GeoParquet's synthetic position-based key, this driver's
//! primary key is a **real** table column — DuckDB tables commonly declare
//! one, and `catalog::primary_key_column` reads it straight from
//! `duckdb_constraints()`. v0.1 assumes a single-column integer primary key
//! (the same limitation `tellurion-geopackage::sql`'s own module doc states
//! for its `INTEGER PRIMARY KEY` assumption): keyset paging compiles to a
//! plain `WHERE pk > ?  ORDER BY pk ASC LIMIT ?`, never an `OFFSET`, and a
//! token is that pk's own decimal text.
//!
//! ## bbox pushdown and CQL2 filtering
//!
//! See `sql.rs`'s own module doc for both: attribute comparisons (CQL2
//! "Basic CQL2", both encodings) compile to a bound-parameter SQL `WHERE`
//! clause; a `bbox` query is an in-process post-filter over decoded WKB,
//! since no spatial index or extension is ever loaded (see the "EXTENSION
//! note" above).
//!
//! ## Datetime
//!
//! `CatalogSource::temporal_column` still reports a single-candidate
//! `TIMESTAMP`/`DATE`-typed column when a table has exactly one — pure
//! introspection, exposed for descriptor richness (`#19`), and a separate,
//! narrower question from *filtering*: `ItemsQuery::datetime` is refused
//! outright (`DuckdbDriverError::DatetimeUnsupported`), the same "introspect
//! yes, filter no" split `tellurion-geoparquet`'s own module doc draws for
//! its identically-shaped decision.
//!
//! ## CRS assumption
//!
//! A WKB blob carries no CRS of its own. This driver assumes every stored
//! coordinate is already CRS84/WGS84 (`srid: Some(4326)`) and never
//! reprojects — the same simplifying assumption `tellurion-flatgeobuf`/
//! `tellurion-geoparquet` make for their own WKB-adjacent geometry columns,
//! except neither of those formats' spec even claims a CRS guarantee the way
//! GeoParquet's absent-`crs`-means-CRS84 convention does; this driver's own
//! assumption is purely operational (the operator's `.duckdb` file is
//! expected to already store CRS84 coordinates) and is not derived from any
//! format-level guarantee.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use duckdb::types::Value;
use duckdb::{Connection, Row};

use tellurion_core::{
    AttributeColumn, CatalogSource, CollectionDecl, DriverFactory, Error as CoreError, FeaturePage,
    FeatureSource, Filter, ItemsQuery, PhysicalCollection, Result as CoreResult, SpatialExtent,
    StorageDecl, StorageDriver,
};

use crate::catalog::{self, TableShape};
use crate::error::{DuckdbDriverError, Result};
use crate::ident::quote_ident;
use crate::pool::{self, ConnectionPool};
use crate::sql;

/// Bounded sample size for `extent`'s approximate bbox fold — see
/// `catalog::extent`'s own doc for why this is a bounded first-N-rows scan,
/// not a full-table one.
const EXTENT_SAMPLE_LIMIT: u32 = 10_000;

/// Registers the `duckdb` driver.
#[derive(Default)]
pub struct DuckdbDriverFactory;

impl DuckdbDriverFactory {
    pub fn new() -> Self {
        Self
    }
}

impl DriverFactory for DuckdbDriverFactory {
    fn name(&self) -> &str {
        "duckdb"
    }

    fn build(&self, decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        let raw = std::env::var(&decl.url_env).map_err(|_| {
            CoreError::Config(format!(
                "storage '{}': environment variable '{}' is not set",
                decl.id, decl.url_env
            ))
        })?;
        let pool = ConnectionPool::open(PathBuf::from(raw))
            .map_err(|e| CoreError::Config(format!("storage '{}': {e}", decl.id)))?;
        Ok(Arc::new(DuckdbDriverImpl {
            backend: Arc::new(DuckdbBackend {
                pool: Arc::new(pool),
                shapes: Mutex::new(HashMap::new()),
            }),
        }))
    }
}

struct DuckdbDriverImpl {
    backend: Arc<DuckdbBackend>,
}

impl StorageDriver for DuckdbDriverImpl {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::clone(&self.backend) as Arc<dyn CatalogSource>
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn FeatureSource>)
    }

    /// Boot-time validation (the issue's own "declared table exists, pk
    /// column exists, geometry column exists and is a WKB `BLOB`" contract):
    /// this trait method has no async counterpart — `Router::build` calls it
    /// as a plain function, never awaited — so this runs one short,
    /// synchronous round trip against the reader pool's first connection,
    /// the same boot-only blocking-DB-call precedent
    /// `tellurion-geopackage::pool::ConnectionPool::open`'s own
    /// `ensure_provisioned` check already sets for this workspace. A
    /// successful resolution is cached so this collection's first real
    /// request pays no repeat introspection cost — see
    /// `DuckdbBackend::resolved_shape`.
    fn validate_collection(&self, decl: &CollectionDecl) -> CoreResult<()> {
        self.backend.validate_and_cache(decl).map_err(Into::into)
    }

    /// The bounded reader pool's own size — mirrors `tellurion-postgis`'s
    /// connection-pool-derived hint, letting the server's admission layer
    /// size its ceiling against what this driver can actually sustain rather
    /// than guessing from CPU count alone.
    fn capacity_hint(&self) -> Option<usize> {
        Some(self.backend.pool.reader_count())
    }
}

struct DuckdbBackend {
    pool: Arc<ConnectionPool>,
    /// Resolved, validated physical shape per collection id — populated at
    /// `validate_collection` boot time (or lazily, on first request, for a
    /// collection under `registry.validation: lazy`) and consulted, never
    /// silently re-derived, by every later `items`/`item` call.
    shapes: Mutex<HashMap<String, Arc<TableShape>>>,
}

/// The physical table name this collection targets: the operator's `table`
/// override when present, else the collection id by convention — the same
/// convention every other driver in this workspace applies (see
/// `tellurion-memory::driver::MemoryBackend::collection_name`).
fn target_table(collection: &CollectionDecl) -> &str {
    collection.table.as_deref().unwrap_or(&collection.id)
}

fn recover_lock<T>(
    guard: std::sync::LockResult<std::sync::MutexGuard<'_, T>>,
) -> std::sync::MutexGuard<'_, T> {
    guard.unwrap_or_else(PoisonError::into_inner)
}

impl DuckdbBackend {
    fn validate_and_cache(&self, decl: &CollectionDecl) -> Result<()> {
        let table = target_table(decl).to_string();
        let collection_id = decl.id.clone();
        let geometry_override = decl.geometry.clone();
        let pk_override = decl.pk.clone();
        let shape = self.pool.with_first_reader_sync(|conn| {
            catalog::resolve_table_shape(
                conn,
                &collection_id,
                &table,
                geometry_override.as_deref(),
                pk_override.as_deref(),
            )
        })?;
        recover_lock(self.shapes.lock()).insert(decl.id.clone(), Arc::new(shape));
        Ok(())
    }

    /// The cached shape from `validate_and_cache`, or — when boot validation
    /// never ran for this collection (`registry.validation: lazy`, or a
    /// resolve that raced ahead of `Router::build`) — a fresh resolution,
    /// cached for every call after this one.
    async fn resolved_shape(&self, decl: &CollectionDecl) -> Result<Arc<TableShape>> {
        if let Some(shape) = recover_lock(self.shapes.lock()).get(&decl.id) {
            return Ok(Arc::clone(shape));
        }
        let table = target_table(decl).to_string();
        let collection_id = decl.id.clone();
        let geometry_override = decl.geometry.clone();
        let pk_override = decl.pk.clone();
        let pool = Arc::clone(&self.pool);
        let shape = pool::with_reader(pool, move |conn| {
            catalog::resolve_table_shape(
                conn,
                &collection_id,
                &table,
                geometry_override.as_deref(),
                pk_override.as_deref(),
            )
        })
        .await?;
        let shape = Arc::new(shape);
        recover_lock(self.shapes.lock()).insert(decl.id.clone(), Arc::clone(&shape));
        Ok(shape)
    }

    async fn collections_inner(&self) -> Result<Vec<PhysicalCollection>> {
        pool::with_reader(Arc::clone(&self.pool), |conn| {
            let mut stmt = conn.prepare(
                "SELECT table_name FROM information_schema.tables \
                 WHERE table_type = 'BASE TABLE' ORDER BY table_name",
            )?;
            let names = stmt.query_map([], |row| row.get::<_, String>(0))?;
            let mut out = Vec::new();
            for name in names {
                let name = name?;
                let shape = catalog::physical_shape(conn, &name)?;
                out.push(PhysicalCollection {
                    name,
                    geometry_column: shape.geometry_column,
                    primary_key: shape.primary_key,
                    // See this module's own "CRS assumption" doc.
                    srid: Some(4326),
                    geometry_type: None,
                });
            }
            Ok(out)
        })
        .await
    }

    async fn extent_inner(&self, physical: &PhysicalCollection) -> Result<Option<SpatialExtent>> {
        let Some(geometry_column) = physical.geometry_column.clone() else {
            return Ok(None);
        };
        let table = physical.name.clone();
        pool::with_reader(Arc::clone(&self.pool), move |conn| {
            Ok(
                catalog::extent(conn, &table, &geometry_column, EXTENT_SAMPLE_LIMIT)?
                    .map(|bbox| SpatialExtent { bbox }),
            )
        })
        .await
    }

    async fn row_estimate_inner(&self, physical: &PhysicalCollection) -> Result<Option<u64>> {
        let table = physical.name.clone();
        pool::with_reader(Arc::clone(&self.pool), move |conn| {
            catalog::row_estimate(conn, &table).map(Some)
        })
        .await
    }

    async fn attribute_schema_inner(
        &self,
        physical: &PhysicalCollection,
    ) -> Result<Option<Vec<AttributeColumn>>> {
        let table = physical.name.clone();
        let geometry_column = physical.geometry_column.clone();
        pool::with_reader(Arc::clone(&self.pool), move |conn| {
            let columns = catalog::list_columns(conn, &table)?;
            Ok(Some(
                columns
                    .into_iter()
                    .filter(|c| Some(c.name.as_str()) != geometry_column.as_deref())
                    .map(|c| AttributeColumn {
                        name: c.name,
                        sql_type: c.sql_type,
                    })
                    .collect(),
            ))
        })
        .await
    }

    async fn temporal_column_inner(&self, physical: &PhysicalCollection) -> Result<Option<String>> {
        let table = physical.name.clone();
        pool::with_reader(Arc::clone(&self.pool), move |conn| {
            Ok(catalog::physical_shape(conn, &table)?.temporal_column)
        })
        .await
    }

    async fn items_inner(
        &self,
        collection: &CollectionDecl,
        query: &ItemsQuery,
    ) -> Result<FeaturePage> {
        if query.datetime.is_some() {
            return Err(DuckdbDriverError::DatetimeUnsupported);
        }
        let shape = self.resolved_shape(collection).await?;
        let token = parse_token(query.token.as_deref())?;
        let bbox = query.bbox;
        let limit = query.limit;
        let filter = query.filter.clone();
        pool::with_reader(Arc::clone(&self.pool), move |conn| match bbox {
            Some(bbox) => read_items_bbox(conn, &shape, filter.as_ref(), bbox, token, limit),
            None => read_items_paged(conn, &shape, filter.as_ref(), token, limit),
        })
        .await
    }

    async fn item_inner(
        &self,
        collection: &CollectionDecl,
        id: &str,
        filter: Option<&Filter>,
    ) -> Result<Option<serde_json::Value>> {
        let Ok(target) = id.parse::<i64>() else {
            // A non-integer id can never match this driver's integer-pk
            // identity — same "honest None" convention every other driver
            // in this workspace applies to a non-integer id.
            return Ok(None);
        };
        let shape = self.resolved_shape(collection).await?;
        let filter = filter.cloned();
        pool::with_reader(Arc::clone(&self.pool), move |conn| {
            read_item_by_pk(conn, &shape, target, filter.as_ref())
        })
        .await
    }
}

#[async_trait]
impl CatalogSource for DuckdbBackend {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        self.collections_inner().await.map_err(Into::into)
    }

    async fn extent(&self, physical: &PhysicalCollection) -> CoreResult<Option<SpatialExtent>> {
        self.extent_inner(physical).await.map_err(Into::into)
    }

    async fn row_estimate(&self, physical: &PhysicalCollection) -> CoreResult<Option<u64>> {
        self.row_estimate_inner(physical).await.map_err(Into::into)
    }

    async fn attribute_schema(
        &self,
        physical: &PhysicalCollection,
    ) -> CoreResult<Option<Vec<AttributeColumn>>> {
        self.attribute_schema_inner(physical)
            .await
            .map_err(Into::into)
    }

    async fn temporal_column(&self, physical: &PhysicalCollection) -> CoreResult<Option<String>> {
        self.temporal_column_inner(physical)
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl FeatureSource for DuckdbBackend {
    async fn items(
        &self,
        collection: &CollectionDecl,
        query: &ItemsQuery,
    ) -> CoreResult<FeaturePage> {
        self.items_inner(collection, query)
            .await
            .map_err(Into::into)
    }

    async fn item(
        &self,
        collection: &CollectionDecl,
        id: &str,
        filter: Option<&Filter>,
    ) -> CoreResult<Option<serde_json::Value>> {
        self.item_inner(collection, id, filter)
            .await
            .map_err(Into::into)
    }

    /// `sql::compile_filter` compiles comparison/`IS [NOT] NULL` and
    /// `AND`/`OR`/`NOT` over the table's own scalar columns — see that
    /// module's own "CQL2 filter scope" doc.
    fn filter_capable(&self) -> bool {
        true
    }

    /// `#105`: this driver earns exactly Basic CQL2 plus both encodings —
    /// mirrors `tellurion-iceberg::driver`'s identical, narrower-than-full
    /// declaration and its own doc for the same reasoning: `LIKE`/`BETWEEN`/
    /// `IN` are all refused by name (so `advanced-comparison-operators` needs
    /// all three and stays undeclared), every spatial predicate is refused
    /// (`S_INTERSECTS` included — bbox pushdown here is an in-process
    /// post-filter, never a compiled SQL predicate, so `basic-spatial-
    /// functions` is never earned), and every temporal predicate is refused
    /// (`ItemsQuery::datetime` too — see this module's own "Datetime" doc),
    /// so neither spatial nor temporal class is declared. `case-insensitive-
    /// comparison` is never implemented at all by this driver (no `CASEI`
    /// support), independent of the general reason every other driver in
    /// this workspace also withholds it (`filter::CQL2_CONFORMANCE_CLASSES`'s
    /// own doc).
    fn cql2_conformance_classes(&self) -> Vec<&'static str> {
        vec![
            tellurion_core::filter::CQL2_CLASS_BASIC,
            tellurion_core::filter::CQL2_CLASS_CQL2_TEXT,
            tellurion_core::filter::CQL2_CLASS_CQL2_JSON,
        ]
    }

    // `crs_capable` stays at the trait default (`false`): this driver
    // performs no reprojection (see this module's own "CRS assumption" doc),
    // so `item_with_crs` also stays at its default (ignore the requested
    // CRS, delegate to `item`) — correct exactly because the caller never
    // sends anything but `RequestedCrs::Omitted` to a driver that declines
    // `crs_capable`.
}

fn parse_token(token: Option<&str>) -> Result<Option<i64>> {
    match token {
        None => Ok(None),
        Some(raw) => raw
            .parse::<i64>()
            .map(Some)
            .map_err(|_| DuckdbDriverError::InvalidToken(raw.to_string())),
    }
}

/// The column list one feature row projects, in select order: this
/// collection's primary key first (index `0`; also this row's GeoJSON `id`
/// and keyset cursor), its geometry column second (index `1`; decoded WKB,
/// excluded from `properties`), then every remaining declared column —
/// including the pk again, as an ordinary property, mirroring
/// `tellurion-geopackage`'s own convention for a real (non-synthetic)
/// relational primary key.
fn select_columns(shape: &TableShape) -> Vec<&str> {
    let mut columns = vec![shape.primary_key.as_str(), shape.geometry_column.as_str()];
    for column in &shape.columns {
        if column.name != shape.primary_key && column.name != shape.geometry_column {
            columns.push(column.name.as_str());
        }
    }
    columns
}

fn quoted_select_list(columns: &[&str]) -> Result<String> {
    let idents: Result<Vec<String>> = columns.iter().map(|c| quote_ident(c)).collect();
    Ok(idents?.join(", "))
}

/// `WHERE` clause plus its bound params for a keyset-paged/point query:
/// `(pk > token)` when resuming a page, ANDed with the compiled attribute
/// filter when one is present. Empty (no `WHERE` at all) when neither
/// applies.
fn build_where(
    pk_ident: &str,
    token: Option<i64>,
    filter: Option<&Filter>,
) -> Result<(String, Vec<Value>)> {
    let mut params = Vec::new();
    let mut clauses = Vec::new();
    if let Some(t) = token {
        clauses.push(format!("({pk_ident} > ?)"));
        params.push(Value::BigInt(t));
    }
    if let Some(f) = filter {
        clauses.push(sql::compile_filter(f, &mut params)?);
    }
    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    Ok((where_sql, params))
}

/// One decoded row -> a full GeoJSON `Feature`, `id` set to this row's pk
/// (as a string, matching every other relational driver's own `pk::text`
/// convention).
fn row_to_geojson(row: &Row<'_>, columns: &[&str]) -> Result<serde_json::Value> {
    let pk: i64 = row.get(0)?;
    let geom_wkb: Option<Vec<u8>> = row.get(1)?;
    let geometry = match &geom_wkb {
        Some(bytes) => sql::geometry_json_from_wkb(bytes)?,
        None => serde_json::Value::Null,
    };

    let mut properties = serde_json::Map::new();
    for (index, name) in columns.iter().enumerate() {
        if index == 1 {
            continue; // the geometry column never appears in `properties`.
        }
        let value: Value = row.get(index)?;
        properties.insert((*name).to_string(), sql::duckdb_value_to_json(value)?);
    }

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

/// Unfiltered-by-bbox listing: `number_matched` comes from a cheap, exact
/// `COUNT(*)` (optionally with the same attribute filter applied — DuckDB
/// answers this from column metadata, not a full scan, per
/// `catalog::row_estimate`'s own doc), and the page itself is a single
/// `LIMIT want+1` query — DuckDB's own query planner stops scanning once
/// enough rows satisfy the limit, so this never reads more of the table than
/// the page (plus attribute filter selectivity) actually requires.
fn read_items_paged(
    conn: &Connection,
    shape: &TableShape,
    filter: Option<&Filter>,
    token: Option<i64>,
    limit: u32,
) -> Result<FeaturePage> {
    let table_ident = quote_ident(&shape.table)?;
    let pk_ident = quote_ident(&shape.primary_key)?;
    let columns = select_columns(shape);
    let select_list = quoted_select_list(&columns)?;

    let (count_where, count_params) = build_where(&pk_ident, None, filter)?;
    let count_sql = format!("SELECT COUNT(*) FROM {table_ident}{count_where}");
    let number_matched: i64 = conn.query_row(
        &count_sql,
        duckdb::params_from_iter(count_params.iter()),
        |row| row.get(0),
    )?;

    let (page_where, mut page_params) = build_where(&pk_ident, token, filter)?;
    let want = limit as usize;
    page_params.push(Value::BigInt(want as i64 + 1));
    let page_sql = format!(
        "SELECT {select_list} FROM {table_ident}{page_where} ORDER BY {pk_ident} ASC LIMIT ?"
    );
    let mut stmt = conn.prepare(&page_sql)?;
    let mut rows = stmt.query(duckdb::params_from_iter(page_params.iter()))?;

    let mut features = Vec::new();
    let mut last_pk: Option<i64> = None;
    let mut has_more = false;
    while let Some(row) = rows.next()? {
        if features.len() >= want {
            has_more = true;
            break;
        }
        let pk: i64 = row.get(0)?;
        features.push(row_to_geojson(row, &columns)?);
        last_pk = Some(pk);
    }

    Ok(FeaturePage {
        features_geojson: features,
        number_matched: Some(u64::try_from(number_matched).unwrap_or(0)),
        next_token: has_more.then(|| last_pk.map(|pk| pk.to_string())).flatten(),
    })
}

/// bbox-filtered listing — see `sql.rs`'s own "bbox pushdown" doc: no
/// spatial index or extension exists to push the bbox test into SQL, so this
/// scans every row an (optional) attribute filter still lets SQL narrow,
/// tests each one's decoded WKB against the query envelope in Rust, and
/// windows the matches by `token`/`limit` exactly like
/// `tellurion-geoparquet::driver::read_items_bbox`'s own no-covering
/// fallback (`number_matched` counts every match across the whole scan, not
/// just what a page happens to return).
fn read_items_bbox(
    conn: &Connection,
    shape: &TableShape,
    filter: Option<&Filter>,
    bbox: [f64; 4],
    token: Option<i64>,
    limit: u32,
) -> Result<FeaturePage> {
    let table_ident = quote_ident(&shape.table)?;
    let pk_ident = quote_ident(&shape.primary_key)?;
    let columns = select_columns(shape);
    let select_list = quoted_select_list(&columns)?;

    let (where_sql, params) = build_where(&pk_ident, None, filter)?;
    let scan_sql =
        format!("SELECT {select_list} FROM {table_ident}{where_sql} ORDER BY {pk_ident} ASC");
    let mut stmt = conn.prepare(&scan_sql)?;
    let mut rows = stmt.query(duckdb::params_from_iter(params.iter()))?;

    let want = limit as usize;
    let mut features = Vec::new();
    let mut matched: u64 = 0;
    let mut has_more = false;
    let mut last_pk: Option<i64> = None;

    while let Some(row) = rows.next()? {
        let pk: i64 = row.get(0)?;
        let geom_wkb: Option<Vec<u8>> = row.get(1)?;
        if !sql::wkb_intersects_bbox(geom_wkb.as_deref(), bbox)? {
            continue;
        }
        matched += 1;
        if token.is_some_and(|t| pk <= t) {
            continue;
        }
        if features.len() < want {
            features.push(row_to_geojson(row, &columns)?);
            last_pk = Some(pk);
        } else {
            has_more = true;
        }
    }

    Ok(FeaturePage {
        features_geojson: features,
        number_matched: Some(matched),
        next_token: has_more.then(|| last_pk.map(|pk| pk.to_string())).flatten(),
    })
}

fn read_item_by_pk(
    conn: &Connection,
    shape: &TableShape,
    target: i64,
    filter: Option<&Filter>,
) -> Result<Option<serde_json::Value>> {
    let table_ident = quote_ident(&shape.table)?;
    let pk_ident = quote_ident(&shape.primary_key)?;
    let columns = select_columns(shape);
    let select_list = quoted_select_list(&columns)?;

    let mut params = vec![Value::BigInt(target)];
    let mut where_sql = format!("{pk_ident} = ?");
    if let Some(f) = filter {
        let filter_sql = sql::compile_filter(f, &mut params)?;
        where_sql = format!("{where_sql} AND {filter_sql}");
    }
    let item_sql = format!("SELECT {select_list} FROM {table_ident} WHERE {where_sql} LIMIT 1");
    let mut stmt = conn.prepare(&item_sql)?;
    let mut rows = stmt.query(duckdb::params_from_iter(params.iter()))?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_geojson(row, &columns)?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE demo (id BIGINT PRIMARY KEY, geom BLOB, name VARCHAR, value BIGINT);",
        )
        .unwrap();
    }

    fn point_wkb(lon: f64, lat: f64) -> Vec<u8> {
        use geozero::GeozeroGeometry;
        let geojson = format!(r#"{{"type":"Point","coordinates":[{lon},{lat}]}}"#);
        let mut buf = Vec::new();
        let mut writer = geozero::wkb::WkbWriter::new(&mut buf, geozero::wkb::WkbDialect::Wkb);
        geozero::geojson::GeoJson(&geojson)
            .process_geom(&mut writer)
            .unwrap();
        buf
    }

    /// The same five points `tellurion-flatgeobuf`/`tellurion-geoparquet`
    /// fixtures use, for family resemblance between the file-driver
    /// fixtures.
    const FEATURES: [(&str, i64, f64, f64); 5] = [
        ("alpha", 1, -4.0, 46.0),
        ("bravo", 2, -2.0, 48.0),
        ("charlie", 3, 0.0, 50.0),
        ("delta", 4, 2.0, 52.0),
        ("echo", 5, 4.0, 54.0),
    ];

    fn seed(conn: &Connection) {
        for (name, value, lon, lat) in FEATURES {
            conn.execute(
                "INSERT INTO demo (id, geom, name, value) VALUES (?, ?, ?, ?)",
                duckdb::params![value, point_wkb(lon, lat), name, value],
            )
            .unwrap();
        }
    }

    fn shape() -> TableShape {
        TableShape {
            table: "demo".to_string(),
            geometry_column: "geom".to_string(),
            primary_key: "id".to_string(),
            columns: vec![
                catalog_column("id", "BIGINT"),
                catalog_column("geom", "BLOB"),
                catalog_column("name", "VARCHAR"),
                catalog_column("value", "BIGINT"),
            ],
        }
    }

    fn catalog_column(name: &str, sql_type: &str) -> crate::catalog::ColumnInfo {
        crate::catalog::ColumnInfo {
            name: name.to_string(),
            sql_type: sql_type.to_string(),
        }
    }

    #[test]
    fn factory_name_matches_the_config_driver_key() {
        assert_eq!(DuckdbDriverFactory::new().name(), "duckdb");
    }

    #[test]
    fn build_fails_fast_when_the_env_var_is_unset() {
        let factory = DuckdbDriverFactory::new();
        let decl = StorageDecl {
            id: "main".to_string(),
            driver: "duckdb".to_string(),
            url_env: "TELLURION_DUCKDB_TEST_DOES_NOT_EXIST".to_string(),
            pool_size: None,
        };
        std::env::remove_var(&decl.url_env);
        assert!(matches!(factory.build(&decl), Err(CoreError::Config(_))));
    }

    #[test]
    fn items_without_a_filter_or_bbox_returns_every_feature_with_an_exact_count() {
        let conn = Connection::open_in_memory().unwrap();
        fixture(&conn);
        seed(&conn);
        let shape = shape();
        let page = read_items_paged(&conn, &shape, None, None, 10).unwrap();
        assert_eq!(page.features_geojson.len(), 5);
        assert_eq!(page.number_matched, Some(5));
        assert_eq!(page.next_token, None);
    }

    #[test]
    fn items_pages_across_at_least_two_pages_with_stable_ids_and_never_uses_offset() {
        let conn = Connection::open_in_memory().unwrap();
        fixture(&conn);
        seed(&conn);
        let shape = shape();

        let page1 = read_items_paged(&conn, &shape, None, None, 2).unwrap();
        assert_eq!(page1.features_geojson.len(), 2);
        assert_eq!(page1.number_matched, Some(5));
        let token1: i64 = page1.next_token.clone().unwrap().parse().unwrap();

        let page2 = read_items_paged(&conn, &shape, None, Some(token1), 2).unwrap();
        assert_eq!(page2.features_geojson.len(), 2);
        let token2: i64 = page2.next_token.clone().unwrap().parse().unwrap();

        let page3 = read_items_paged(&conn, &shape, None, Some(token2), 2).unwrap();
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

    #[test]
    fn items_with_an_attribute_filter_pushes_down_to_sql() {
        let conn = Connection::open_in_memory().unwrap();
        fixture(&conn);
        seed(&conn);
        let shape = shape();
        let filter = Filter::Compare {
            property: "name".to_string(),
            op: tellurion_core::CompareOp::Eq,
            value: tellurion_core::Literal::Text("charlie".to_string()),
        };
        let page = read_items_paged(&conn, &shape, Some(&filter), None, 10).unwrap();
        assert_eq!(page.features_geojson.len(), 1);
        assert_eq!(page.features_geojson[0]["properties"]["name"], "charlie");
    }

    #[test]
    fn items_with_a_bbox_returns_only_matching_features_and_an_exact_count() {
        let conn = Connection::open_in_memory().unwrap();
        fixture(&conn);
        seed(&conn);
        let shape = shape();
        // Covers only the western half of the fixture's extent.
        let page =
            read_items_bbox(&conn, &shape, None, [-5.0, 45.0, -1.0, 55.0], None, 10).unwrap();
        assert!(!page.features_geojson.is_empty());
        assert!(page.features_geojson.len() < 5);
        assert_eq!(
            page.number_matched,
            Some(page.features_geojson.len() as u64)
        );
        for feature in &page.features_geojson {
            let x = feature["geometry"]["coordinates"][0].as_f64().unwrap();
            assert!(x <= -1.0, "feature outside the requested bbox: {feature}");
        }
    }

    #[test]
    fn item_looks_up_a_feature_by_its_real_primary_key() {
        let conn = Connection::open_in_memory().unwrap();
        fixture(&conn);
        seed(&conn);
        let shape = shape();
        let found = read_item_by_pk(&conn, &shape, 3, None).unwrap().unwrap();
        assert_eq!(found["id"], "3");
        assert_eq!(found["properties"]["name"], "charlie");
    }

    #[test]
    fn item_returns_none_for_an_absent_pk() {
        let conn = Connection::open_in_memory().unwrap();
        fixture(&conn);
        seed(&conn);
        let shape = shape();
        assert_eq!(read_item_by_pk(&conn, &shape, 999, None).unwrap(), None);
    }

    #[test]
    fn item_with_a_filter_that_excludes_it_comes_back_as_absent() {
        let conn = Connection::open_in_memory().unwrap();
        fixture(&conn);
        seed(&conn);
        let shape = shape();
        let filter = Filter::Compare {
            property: "name".to_string(),
            op: tellurion_core::CompareOp::Eq,
            value: tellurion_core::Literal::Text("nope".to_string()),
        };
        assert_eq!(
            read_item_by_pk(&conn, &shape, 3, Some(&filter)).unwrap(),
            None
        );
    }

    #[test]
    fn a_null_geometry_row_serves_with_a_null_geometry_and_is_excluded_from_bbox() {
        let conn = Connection::open_in_memory().unwrap();
        fixture(&conn);
        conn.execute(
            "INSERT INTO demo (id, geom, name, value) VALUES (?, NULL, ?, ?)",
            duckdb::params![99i64, "no-geometry", 0i64],
        )
        .unwrap();
        let shape = shape();

        let all = read_items_paged(&conn, &shape, None, None, 10).unwrap();
        let null_feature = all
            .features_geojson
            .iter()
            .find(|f| f["id"] == "99")
            .unwrap();
        assert_eq!(null_feature["geometry"], serde_json::Value::Null);

        let bbox =
            read_items_bbox(&conn, &shape, None, [-180.0, -90.0, 180.0, 90.0], None, 10).unwrap();
        assert!(bbox.features_geojson.iter().all(|f| f["id"] != "99"));
    }

    #[test]
    fn invalid_token_is_rejected() {
        assert!(matches!(
            parse_token(Some("not-a-number")),
            Err(DuckdbDriverError::InvalidToken(_))
        ));
    }

    // `#105`: the invariant that `filter_capable()` and a non-empty
    // `cql2_conformance_classes()` agree, and that the declared set stays
    // basic-only, is exercised against the real trait impl (through a real
    // backend) by `tests/driver_contract.rs::
    // cql2_conformance_classes_stays_basic_only` — no separate unit test
    // here duplicates that check against a hand-copied literal.
}
