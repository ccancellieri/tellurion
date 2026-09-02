//! The `geopackage` `DriverFactory`, and the `CatalogSource`, `FeatureSource`,
//! `TileSource`, `WriteSink`, and `OutboxSource` implementation backing it.
//! See the crate's own top-level docs for the embedded, self-contained
//! positioning; this module is where every capability trait meets
//! `pool.rs`'s connection management, `catalog.rs`'s introspection,
//! `sql.rs`'s read queries, and `write_sql.rs`'s mutations.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use rusqlite::{Connection, OptionalExtension, ToSql};

use tellurion_core::{
    heuristics, AttributeColumn, BatchItemOutcome, BatchItemResult, CatalogSource, CollectionDecl,
    DriverFactory, Error as CoreError, FeaturePage, FeatureSource, Filter, IdType, ItemsQuery,
    Mutation, MutationKind, Obligation, ObligationExtent, OutboxSource, PhysicalCollection,
    RequestedCrs, Result as CoreResult, Sequence, SpatialExtent, StorageDecl, StorageDriver,
    TileCoord, TileSource, WriteSink, DEFAULT_TILE_VERTEX_BUDGET,
};
use tellurion_vector_tile::{
    encode_tile_with_outcome, tile_envelope_3857, SourceCrs, TileFeature, TileRequest, TileScalar,
};

use crate::catalog;
use crate::error::{GeopackageError, Result};
use crate::gpb;
use crate::ident::quote_ident;
use crate::intersects::{self, IntersectsCheck};
use crate::pool::{self, ConnectionPool};
use crate::sql::{self, SqlParam};
use crate::write_sql;

/// MVT encoding grid resolution — matches `tellurion-postgis::sql::
/// MVT_EXTENT` (`4096`), the de facto standard tile-internal coordinate
/// space every MVT consumer in this workspace already expects.
const MVT_EXTENT: u32 = 4096;

fn boxed_param_refs(params: &[SqlParam]) -> Vec<&dyn ToSql> {
    params.iter().map(|p| p as &dyn ToSql).collect()
}

/// Batch size for the candidate-row scan behind an active `S_INTERSECTS`
/// exact post-filter (`items_with_exact_intersects`). The R*Tree bbox
/// pushdown already narrows candidates to the query geometry's own bounding
/// box before any row reaches this scan, so this only needs to be large
/// enough that a normal page resolves in a small handful of round trips even
/// when most bbox candidates turn out not to intersect exactly (e.g. a thin
/// diagonal query polygon against a densely bbox-overlapping table).
const INTERSECTS_SCAN_BATCH: u32 = 512;

/// Hard ceiling on how many candidate rows one `items()` call will decode
/// and exact-test while hunting for a page under an active `S_INTERSECTS`
/// filter — bounds one request's work regardless of how sparse real matches
/// are inside the bbox candidate set. Reaching it ends the page early
/// (fewer than the requested `limit`, `next_token` still set to resume the
/// scan) rather than continuing without limit — never a wrong page, only a
/// possibly short one; the next request picks up exactly where this one
/// stopped, so no candidate row is ever skipped or double-counted, only
/// deferred to a later call.
const INTERSECTS_SCAN_ROW_CAP: usize = 20_000;

/// Registers the `geopackage` driver.
#[derive(Default)]
pub struct GeopackageDriverFactory;

impl GeopackageDriverFactory {
    pub fn new() -> Self {
        Self
    }
}

impl DriverFactory for GeopackageDriverFactory {
    fn name(&self) -> &str {
        "geopackage"
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

        Ok(Arc::new(GeopackageDriverImpl {
            backend: Arc::new(GeopackageBackend {
                pool: Arc::new(pool),
            }),
        }))
    }
}

struct GeopackageDriverImpl {
    backend: Arc<GeopackageBackend>,
}

impl StorageDriver for GeopackageDriverImpl {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::clone(&self.backend) as Arc<dyn CatalogSource>
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn FeatureSource>)
    }

    fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn TileSource>)
    }

    /// This driver's own write lane (issue `#73`): the data mutation and the
    /// outbox insert commit in one SQLite transaction on the single writer
    /// connection — see `write_apply_inner`.
    fn write_sink(&self) -> Option<Arc<dyn WriteSink>> {
        Some(Arc::clone(&self.backend) as Arc<dyn WriteSink>)
    }

    /// The read side of this same storage's outbox — advertised alongside
    /// `write_sink`, per the capability contract's own doc.
    fn outbox_source(&self) -> Option<Arc<dyn OutboxSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn OutboxSource>)
    }

    /// The same whitelist `ident.rs` applies lazily on every query, run once
    /// eagerly here so a config typo (e.g. a hyphen in `table`) fails at
    /// `Router::build` time instead of 500-ing every request to this
    /// collection forever — mirrors `tellurion-postgis::driver::
    /// validate_collection_identifiers` exactly.
    fn validate_collection(&self, decl: &CollectionDecl) -> CoreResult<()> {
        validate_collection_identifiers(decl).map_err(Into::into)
    }

    /// The reader pool's own size — how many concurrent read requests this
    /// file can genuinely serve without queuing behind each other (writes
    /// serialize through the single writer connection regardless; see
    /// `pool.rs`'s own top-level doc).
    fn capacity_hint(&self) -> Option<usize> {
        Some(self.backend.pool.reader_count())
    }
}

fn validate_collection_identifiers(decl: &CollectionDecl) -> Result<()> {
    if let Some(table) = &decl.table {
        quote_ident(table)?;
    }
    if let Some(geometry) = &decl.geometry {
        quote_ident(geometry)?;
    }
    if let Some(pk) = &decl.pk {
        quote_ident(pk)?;
    }
    if let Some(datetime) = &decl.datetime {
        quote_ident(datetime)?;
    }
    // `#104`: a declared variant column reaches this driver's SQL exactly
    // like the base geometry column does (`sql::build_tile_plan`'s SELECT
    // list), so it earns the same eager whitelist check — otherwise a
    // hyphenated variant name would boot fine and then 500 every tile
    // request inside the zoom range it covers, and only there.
    for variant in &decl.geometry_variants {
        quote_ident(&variant.column)?;
    }
    Ok(())
}

struct GeopackageBackend {
    pool: Arc<ConnectionPool>,
}

/// `column_names(...)`'s reported list at query-plan time, plus the two
/// indices this driver's row->GeoJSON conversion always needs — computed
/// once per prepared statement, not per row.
struct RowShape {
    columns: Vec<String>,
    pk_idx: usize,
    geom_idx: usize,
}

fn row_shape(stmt: &rusqlite::Statement<'_>, pk: &str, geom: &str) -> Result<RowShape> {
    let columns: Vec<String> = stmt.column_names().into_iter().map(String::from).collect();
    let pk_idx = columns.iter().position(|c| c == pk).ok_or_else(|| {
        GeopackageError::MalformedGeometry(format!("column '{pk}' missing from result set"))
    })?;
    let geom_idx = columns.iter().position(|c| c == geom).ok_or_else(|| {
        GeopackageError::MalformedGeometry(format!("column '{geom}' missing from result set"))
    })?;
    Ok(RowShape {
        columns,
        pk_idx,
        geom_idx,
    })
}

/// One non-geometry, non-pk column value as JSON. A `BLOB` in any other
/// column is refused rather than silently dropped or lossily stringified —
/// this driver's v0.1 read model is scalar properties plus one geometry
/// column, the same flat model its write path accepts (`write_sql.rs`'s own
/// doc).
fn value_ref_to_json(
    value: rusqlite::types::ValueRef<'_>,
    column: &str,
) -> Result<serde_json::Value> {
    use rusqlite::types::ValueRef;
    Ok(match value {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Integer(i) => serde_json::Value::Number(i.into()),
        ValueRef::Real(f) => serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        ValueRef::Text(t) => serde_json::Value::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(_) => {
            return Err(GeopackageError::UnsupportedColumnValue(column.to_string()))
        }
    })
}

/// One allowlisted tile column as the shared encoder's owned scalar value.
/// Unlike the general GeoJSON read path above, a non-finite SQLite `REAL`
/// stays a float here so the encoder can reject it by column name instead of
/// silently changing it to JSON `null`.
fn tile_scalar(value: rusqlite::types::ValueRef<'_>, column: &str) -> Result<TileScalar> {
    use rusqlite::types::ValueRef;
    Ok(match value {
        ValueRef::Null => TileScalar::Null,
        ValueRef::Integer(value) => TileScalar::Signed(value),
        ValueRef::Real(value) => TileScalar::Float(value),
        ValueRef::Text(value) => TileScalar::String(String::from_utf8_lossy(value).into_owned()),
        ValueRef::Blob(_) => {
            return Err(GeopackageError::UnsupportedColumnValue(column.to_string()))
        }
    })
}

/// One row of a `SELECT *`-shaped statement into `(pk_value, GeoJSON
/// Feature)` — the pk becomes the string `id` (v0.1's integer-pk
/// convention, matching `tellurion-postgis`'s own `pk::text`), the geometry
/// column decodes through `gpb::geometry_to_geojson`, and every other
/// column becomes one `properties` entry.
fn row_to_feature(row: &rusqlite::Row<'_>, shape: &RowShape) -> Result<(i64, serde_json::Value)> {
    let pk_value: i64 = row.get(shape.pk_idx)?;
    let geometry = match row.get_ref(shape.geom_idx)? {
        rusqlite::types::ValueRef::Null => serde_json::Value::Null,
        rusqlite::types::ValueRef::Blob(bytes) => gpb::geometry_to_geojson(bytes)?,
        _ => {
            return Err(GeopackageError::MalformedGeometry(
                "geometry column is neither NULL nor a BLOB".to_string(),
            ))
        }
    };

    let mut properties = serde_json::Map::new();
    for (idx, name) in shape.columns.iter().enumerate() {
        if idx == shape.pk_idx || idx == shape.geom_idx {
            continue;
        }
        properties.insert(name.clone(), value_ref_to_json(row.get_ref(idx)?, name)?);
    }

    let feature = serde_json::json!({
        "type": "Feature",
        "id": pk_value.to_string(),
        "geometry": geometry,
        "properties": properties,
    });
    Ok((pk_value, feature))
}

/// Tests one already-fetched row's geometry column against an active
/// `S_INTERSECTS` check, without paying for a full `row_to_feature` GeoJSON
/// conversion first — every caller here only needs that conversion for a row
/// that actually matches. A `NULL` or non-`BLOB` geometry can never
/// intersect anything, so it short-circuits to `false` rather than reaching
/// `intersects::row_intersects` at all.
fn row_matches_intersects(
    row: &rusqlite::Row<'_>,
    shape: &RowShape,
    check: &IntersectsCheck,
) -> Result<bool> {
    match row.get_ref(shape.geom_idx)? {
        rusqlite::types::ValueRef::Blob(bytes) => {
            let decoded = gpb::decode(bytes)?;
            Ok(!decoded.is_empty && intersects::row_intersects(decoded.wkb, check)?)
        }
        _ => Ok(false),
    }
}

/// Fills one `items()` page when the compiled filter carries an active
/// `S_INTERSECTS` predicate (`sql::ItemsPlan::intersects_check`) — the case
/// `items_inner`'s own single-query path can't handle, because the R*Tree
/// bbox pushdown it already applied is only a *candidate* pre-filter: some
/// bbox-overlapping rows will fail the exact geometry test, so a plain
/// `LIMIT requested_limit + 1` can return fewer real matches than the page
/// needs, or none at all, even though more candidates remain beyond the
/// fetched window.
///
/// Scans forward in bounded batches (`INTERSECTS_SCAN_BATCH` candidate rows
/// per round trip, up to `INTERSECTS_SCAN_ROW_CAP` total — see both
/// constants' own docs), re-running `sql::build_items_plan` each time with
/// the previous batch's last-seen pk as the new keyset token, decoding and
/// exact-testing every candidate as it streams in, until either
/// `requested_limit + 1` rows have matched (a full page plus the "is there
/// more" probe every other query in this driver already uses) or the table
/// runs out of candidates.
///
/// `next_token` follows the *same convention `items_inner`'s own plain path
/// already uses* for its own "+1" probe row: when a `(requested_limit +
/// 1)`-th match is found, that row is never returned or counted, only used
/// to prove a next page exists — the token resumes strictly after the last
/// row actually *kept* on this page, so the probe row (and everything after
/// it) is re-examined, never skipped, on the next call. When this call
/// instead stops because [`INTERSECTS_SCAN_ROW_CAP`] was reached before
/// finding enough matches, there is no held-back probe row to fall back to,
/// so the token resumes after the last candidate row this call examined at
/// all (matched or not) — still correct, since every row up to that pk was
/// already fully decided one way or the other. Either way, a row this call
/// rejected is never re-offered and a row it accepted is never re-examined,
/// regardless of how many rejected candidates fall between two accepted ones
/// or straddle the page boundary itself.
#[allow(clippy::too_many_arguments)]
fn items_with_exact_intersects(
    conn: &Connection,
    table: &str,
    pk: &str,
    geom: &str,
    datetime_col: Option<&str>,
    collection_id: &str,
    requested_limit: usize,
    initial_token: Option<&str>,
    bbox: Option<[f64; 4]>,
    datetime: Option<(&Option<String>, &Option<String>)>,
    filter: Option<&Filter>,
    check: &IntersectsCheck,
) -> Result<FeaturePage> {
    let mut matched: Vec<(i64, serde_json::Value)> = Vec::new();
    let mut cursor: Option<String> = initial_token.map(str::to_string);
    let mut last_scanned_pk: Option<i64> = None;
    let mut scanned = 0usize;

    // `Some(pk)` once the scan below decides there may be more data past
    // `pk`; `None` once it has proven no candidate remains beyond what's
    // already in `matched`.
    let resume_after: Option<i64> = 'scan: loop {
        if scanned >= INTERSECTS_SCAN_ROW_CAP {
            break 'scan last_scanned_pk;
        }

        let plan = sql::build_items_plan(
            table,
            pk,
            geom,
            datetime_col,
            collection_id,
            INTERSECTS_SCAN_BATCH,
            cursor.as_deref(),
            bbox,
            datetime,
            filter,
        )?;
        let mut stmt = conn.prepare(&plan.sql)?;
        let shape = row_shape(&stmt, pk, geom)?;
        let refs = boxed_param_refs(&plan.params);
        let mut rows = stmt.query(refs.as_slice())?;

        let mut got_in_batch = 0usize;
        while let Some(row) = rows.next()? {
            got_in_batch += 1;
            scanned += 1;
            let pk_value: i64 = row.get(shape.pk_idx)?;
            last_scanned_pk = Some(pk_value);

            if row_matches_intersects(row, &shape, check)? {
                if matched.len() == requested_limit {
                    // This row is the `requested_limit + 1`-th match: the
                    // probe row, never returned — resume strictly after the
                    // last *kept* match (`matched` is already exactly
                    // `requested_limit` items long here), so this row and
                    // everything after it is re-examined next call.
                    break 'scan matched.last().map(|(kept_pk, _)| *kept_pk);
                }
                let (_, feature) = row_to_feature(row, &shape)?;
                matched.push((pk_value, feature));
            }
        }

        // `build_items_plan` itself over-fetches by one row (its own
        // "detect a next page without a second round trip" convention);
        // fewer than that back means this batch drained the table.
        if got_in_batch <= INTERSECTS_SCAN_BATCH as usize {
            break 'scan None;
        }
        cursor = last_scanned_pk.map(|v| v.to_string());
    };

    Ok(FeaturePage {
        features_geojson: matched.into_iter().map(|(_, f)| f).collect(),
        // Same honesty every other active-filter query in this driver
        // already applies (`sql::ItemsPlan::count_sql` is `None` too): an
        // exact count under a predicate this driver can't push fully into
        // SQL would need the same unbounded scan this batching already
        // bounds away from.
        number_matched: None,
        next_token: resume_after.map(|v| v.to_string()),
    })
}

/// Rewrites a plain "no such table" SQLite error into the named
/// `OutboxTableMissing` — SQLite reports a missing relation as a generic
/// `SQLITE_ERROR` with a stable, well-known message text rather than a
/// structured error code the way PostgreSQL's `SqlState::UNDEFINED_TABLE`
/// does, so text matching against that stable message is this driver's
/// counterpart of `tellurion-postgis::driver::map_outbox_missing`. Every
/// other error passes through unchanged.
///
/// A missing COLUMN (`#141`/`#142`) is matched the same way, under BOTH
/// phrasings SQLite uses for it: an `INSERT` naming an absent column reports
/// "table X has no column named Y", while a `SELECT` of one reports "no such
/// column: Y". Both are the same fact — this outbox predates the extent
/// column — and both must reach the same named refusal, or the write lane and
/// the drain lane would tell an operator two different stories about one
/// missing migration.
fn map_outbox_missing(error: rusqlite::Error, table: &str) -> GeopackageError {
    let column = write_sql::OUTBOX_EXTENT_COLUMN;
    let message = error.to_string();
    if message.contains("no such table") {
        GeopackageError::OutboxTableMissing(write_sql::outbox_table_name(table))
    } else if message.contains(&format!("no such column: {column}"))
        || message.contains(&format!("has no column named {column}"))
    {
        // `#141`/`#142`: the outbox table predates the extent column. Named
        // and refused rather than worked around — the server does no DDL,
        // and rerunning `tellurion-ingest geopackage create-tables` adds the
        // column in place.
        GeopackageError::OutboxExtentColumnMissing(write_sql::outbox_table_name(table))
    } else {
        GeopackageError::from(error)
    }
}

/// One feature's stored geometry expressed in CRS84 (`#141`/`#142`), read
/// through `conn` at the moment this is called — before the mutation for the
/// prior extent, after it for the current one.
///
/// Three distinguishable answers, and keeping them apart is the whole point:
/// `Ok(Ok(Some(bbox)))` (the feature is there, here is where),
/// `Ok(Ok(None))` (the feature is not there, or has no geometry — a recorded
/// answer), and `Ok(Err(()))` (this driver cannot express `srid` in CRS84 —
/// an honest "cannot say", which the caller turns into
/// `ObligationExtent::Unrecorded` for the WHOLE obligation rather than
/// letting it pass for an empty extent).
fn stored_crs84_extent(
    conn: &rusqlite::Connection,
    table: &str,
    pk: &str,
    geometry_column: &str,
    pk_value: i64,
    srid: i32,
) -> Result<std::result::Result<Option<[f64; 4]>, ()>> {
    let (sql, params) =
        write_sql::build_stored_geometry_plan(table, pk, geometry_column, pk_value)?;
    let refs = boxed_param_refs(&params);
    let blob: Option<Option<Vec<u8>>> = conn
        .query_row(&sql, refs.as_slice(), |row| {
            row.get::<_, Option<Vec<u8>>>(0)
        })
        .optional()
        .map_err(GeopackageError::from)?;
    let Some(Some(blob)) = blob else {
        // No such row, or a `NULL` geometry column: nothing there, and this
        // driver knows that for a fact whatever the storage CRS is.
        return Ok(Ok(None));
    };
    let Some(envelope) = gpb::envelope_of_blob(&blob)? else {
        return Ok(Ok(None));
    };
    Ok(crate::crs::bbox_to_crs84(srid, envelope)
        .map(Some)
        .ok_or(()))
}

/// Folds a prior/current pair of [`stored_crs84_extent`] answers into the
/// [`ObligationExtent`] the outbox records. Either side saying "cannot say"
/// makes the whole obligation `Unrecorded`: a half-known extent would
/// invalidate the half it knows and silently skip the half it does not,
/// which is exactly the stale-tile failure `#141`/`#142` exist to close.
fn fold_extent(
    prior: std::result::Result<Option<[f64; 4]>, ()>,
    current: std::result::Result<Option<[f64; 4]>, ()>,
) -> ObligationExtent {
    match (prior, current) {
        (Ok(prior), Ok(current)) => ObligationExtent::Crs84 { prior, current },
        _ => ObligationExtent::Unrecorded,
    }
}

/// Builds one batch mutation's two statements (data + outbox) exactly the
/// way `write_apply_inner` does for a single `PUT`/`DELETE` (`#114`) — the
/// only difference is `known_columns` arrives pre-resolved for the WHOLE
/// chunk (one `catalog::attribute_columns` lookup, not one per item) rather
/// than being looked up here. Pure: no I/O, no `Connection` reference, so a
/// caller can run this once per mutation with no extra round trip.
fn build_batch_item_plan(
    table: &str,
    pk: &str,
    geometry_column: &str,
    srid: i32,
    mutation: &Mutation,
    known_columns: &HashSet<String>,
    requested_crs: RequestedCrs,
) -> Result<BatchItemPlan> {
    let pk_value: i64 = mutation
        .feature_id
        .parse()
        .map_err(|_| GeopackageError::InvalidFeatureId(mutation.feature_id.clone()))?;

    let (statement_sql, statement_params, outbox_payload) = match &mutation.kind {
        MutationKind::Upsert(feature) => {
            let properties = feature
                .get("properties")
                .and_then(serde_json::Value::as_object)
                .cloned()
                .unwrap_or_default();
            let geometry = feature.get("geometry");
            let plan = write_sql::build_upsert_plan(
                table,
                pk,
                geometry_column,
                srid,
                pk_value,
                geometry,
                &properties,
                known_columns,
                requested_crs,
            )?;
            (plan.sql, plan.params, Some(feature.clone()))
        }
        MutationKind::Delete => {
            let (sql, params) = write_sql::build_delete_plan(table, pk, pk_value)?;
            (sql, params, None)
        }
    };
    let kind_text = match &mutation.kind {
        MutationKind::Upsert(_) => "upsert",
        MutationKind::Delete => "delete",
    };
    Ok(BatchItemPlan {
        pk_value,
        statement_sql,
        statement_params,
        kind: kind_text,
        payload: outbox_payload,
        deletes: matches!(mutation.kind, MutationKind::Delete),
    })
}

/// One batch item's data statement plus what the outbox insert will need.
/// The outbox insert is deliberately NOT pre-built here the way it used to
/// be: `#141`/`#142` made its content depend on what the file holds
/// immediately before and after the data statement, so it is built inside
/// the transaction once both are known — the same shape `write_apply_inner`
/// now follows.
struct BatchItemPlan {
    pk_value: i64,
    statement_sql: String,
    statement_params: Vec<SqlParam>,
    kind: &'static str,
    payload: Option<serde_json::Value>,
    deletes: bool,
}

impl GeopackageBackend {
    async fn catalog_inner(&self) -> Result<Vec<PhysicalCollection>> {
        pool::with_reader(Arc::clone(&self.pool), |conn: &Connection| {
            let tables = catalog::list_feature_tables(conn)?;
            Ok(tables
                .into_iter()
                .map(|t| PhysicalCollection {
                    name: t.table_name,
                    geometry_column: Some(t.geometry_column),
                    primary_key: t.primary_key,
                    srid: t.srid,
                    geometry_type: t.geometry_type,
                })
                .collect())
        })
        .await
    }

    /// `Ok(None)` unless `physical.srid` is exactly `4326` — reporting a
    /// CRS84 extent for any other native SRID (most commonly `3857`, the
    /// SRID the tiles lane itself requires) would need a reprojection this
    /// driver deliberately does not perform (see the crate's own top-level
    /// "out of scope" doc); `4326`'s own coordinate order already matches
    /// CRS84, the same simplifying assumption `tellurion-flatgeobuf` makes
    /// for its own header envelope.
    async fn extent_inner(&self, physical: &PhysicalCollection) -> Result<Option<SpatialExtent>> {
        let Some(geometry_column) = physical.geometry_column.clone() else {
            return Ok(None);
        };
        if physical.srid != Some(4326) {
            return Ok(None);
        }
        let table = physical.name.clone();
        pool::with_reader(Arc::clone(&self.pool), move |conn| {
            Ok(catalog::extent(conn, &table, &geometry_column)?.map(|bbox| SpatialExtent { bbox }))
        })
        .await
    }

    async fn row_estimate_inner(&self, physical: &PhysicalCollection) -> Result<Option<u64>> {
        let table = physical.name.clone();
        pool::with_reader(Arc::clone(&self.pool), move |conn| {
            catalog::row_estimate(conn, &table)
        })
        .await
    }

    async fn attribute_schema_inner(
        &self,
        physical: &PhysicalCollection,
    ) -> Result<Option<Vec<AttributeColumn>>> {
        let Some(geometry_column) = physical.geometry_column.clone() else {
            return Ok(None);
        };
        let table = physical.name.clone();
        pool::with_reader(Arc::clone(&self.pool), move |conn| {
            let columns = catalog::attribute_columns(conn, &table, &geometry_column)?;
            Ok(Some(
                columns
                    .into_iter()
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
        let Some(geometry_column) = physical.geometry_column.clone() else {
            return Ok(None);
        };
        let table = physical.name.clone();
        pool::with_reader(Arc::clone(&self.pool), move |conn| {
            catalog::temporal_column(conn, &table, &geometry_column)
        })
        .await
    }

    async fn items_inner(
        &self,
        collection: &CollectionDecl,
        query: &ItemsQuery,
    ) -> Result<FeaturePage> {
        let table = collection.resolved_table().to_string();
        let pk = collection.resolved_pk().to_string();
        let geom = collection.resolved_geometry().to_string();
        let datetime_col = collection.datetime.clone();
        let collection_id = collection.id.clone();
        let requested_limit = query.limit as usize;
        let start = query.datetime.as_ref().and_then(|d| d.start.clone());
        let end = query.datetime.as_ref().and_then(|d| d.end.clone());
        let has_datetime = query.datetime.is_some();
        let bbox = query.bbox;
        let token = query.token.clone();
        let filter = query.filter.clone();
        let limit = query.limit;

        let plan = sql::build_items_plan(
            &table,
            &pk,
            &geom,
            datetime_col.as_deref(),
            &collection_id,
            limit,
            token.as_deref(),
            bbox,
            has_datetime.then_some((&start, &end)),
            filter.as_ref(),
        )?;

        pool::with_reader(Arc::clone(&self.pool), move |conn| {
            let Some(check) = &plan.intersects_check else {
                let mut stmt = conn.prepare(&plan.sql)?;
                let shape = row_shape(&stmt, &pk, &geom)?;
                let refs = boxed_param_refs(&plan.params);
                let mut rows = stmt.query(refs.as_slice())?;

                let mut collected: Vec<(i64, serde_json::Value)> = Vec::new();
                while let Some(row) = rows.next()? {
                    collected.push(row_to_feature(row, &shape)?);
                }
                drop(rows);
                drop(stmt);

                let has_more = collected.len() > requested_limit;
                if has_more {
                    collected.truncate(requested_limit);
                }
                let next_token = has_more
                    .then(|| collected.last().map(|(pk, _)| pk.to_string()))
                    .flatten();

                let number_matched = match &plan.count_sql {
                    Some(count_sql) => {
                        let count: i64 = conn.query_row(count_sql, [], |row| row.get(0))?;
                        u64::try_from(count).ok()
                    }
                    None => None,
                };

                return Ok(FeaturePage {
                    features_geojson: collected.into_iter().map(|(_, f)| f).collect(),
                    number_matched,
                    next_token,
                });
            };

            // Active `S_INTERSECTS`: `plan.sql`/`plan.params` above already
            // narrow every candidate to a bbox overlap, but SQL alone can't
            // finish the job — see `items_with_exact_intersects`'s own doc
            // for why filling a page under an exact per-row predicate needs
            // its own batched scan rather than the single query above.
            items_with_exact_intersects(
                conn,
                &table,
                &pk,
                &geom,
                datetime_col.as_deref(),
                &collection_id,
                requested_limit,
                token.as_deref(),
                bbox,
                has_datetime.then_some((&start, &end)),
                filter.as_ref(),
                check,
            )
        })
        .await
    }

    async fn item_inner(
        &self,
        collection: &CollectionDecl,
        id: &str,
        filter: Option<&Filter>,
    ) -> Result<Option<serde_json::Value>> {
        // `#87`: unlike PostGIS, this is never a live per-table question —
        // the GeoPackage format mandates an `INTEGER PRIMARY KEY` feature id
        // column, so any other declared `id_type` is wrong unconditionally.
        // Named refusal, not the silent `Ok(None)` a merely-unparseable id
        // gets below.
        if collection.id_type != IdType::Integer {
            return Err(GeopackageError::IdTypeUnsupported(collection.id.clone()));
        }
        let Ok(pk_value) = id.parse::<i64>() else {
            // A non-integer id can never match a v0.1 (integer-pk) collection.
            return Ok(None);
        };
        let table = collection.resolved_table().to_string();
        let pk = collection.resolved_pk().to_string();
        let geom = collection.resolved_geometry().to_string();
        let filter = filter.cloned();

        let (query_sql, params, intersects_check) =
            sql::build_item_plan(&table, &pk, pk_value, filter.as_ref())?;

        pool::with_reader(Arc::clone(&self.pool), move |conn| {
            let mut stmt = conn.prepare(&query_sql)?;
            let shape = row_shape(&stmt, &pk, &geom)?;
            let refs = boxed_param_refs(&params);
            let mut rows = stmt.query(refs.as_slice())?;
            let Some(row) = rows.next()? else {
                return Ok(None);
            };
            // An `S_INTERSECTS` predicate got no R*Tree pushdown of its own
            // in this single-row lookup (`sql::build_item_plan`'s own doc —
            // a `pk = ?1` seek has nothing left for a bbox subquery to
            // prune), so the exact test happens here instead. A row that
            // exists but fails it comes back `Ok(None)`, indistinguishable
            // from a missing id — the same contract `FeatureSource::item`'s
            // own doc states for every other filter shape.
            if let Some(check) = &intersects_check {
                let is_match = row_matches_intersects(row, &shape, check)?;
                if !is_match {
                    return Ok(None);
                }
            }
            Ok(Some(row_to_feature(row, &shape)?.1))
        })
        .await
    }

    /// `Ok(None)` when the requested tile carries no matching features
    /// (either genuinely empty or `pool::with_reader`'s query found none) —
    /// the same "empty tile is valid, not an error" convention every
    /// `TileSource` in this workspace follows.
    ///
    /// SRID handling (`#89`): a `3857`-stored collection takes the original
    /// native path — the R*Tree query window is the tile's own bounds, and
    /// each row's coordinates enter the shared encoder without reprojection. A
    /// `4326`-stored collection reprojects: the R*Tree query window travels
    /// *into* degrees first (`web_mercator_to_lonlat`, so bbox pruning still
    /// runs against the stored coordinates, never a decode-then-filter scan),
    /// then every row this file's own candidate scan returns gets its
    /// vertices reprojected *out* to meters (`lonlat_to_web_mercator`) right
    /// before MVT encoding — geometry never crosses CRSs anywhere else, and
    /// the exact `S_INTERSECTS` test above still runs against the row's own
    /// stored (degrees) geometry, matching the needle bbox's own units. Any
    /// other SRID is refused by name rather than serving a distorted tile.
    ///
    /// Geometry variants (`#104`, wired into this lane by `#200`): the column
    /// each row's bytes come from is `CollectionDecl::resolved_geometry_for_
    /// zoom(coord.z)`, not the base geometry column, so a collection whose
    /// operator has already produced a pre-generalized column for this zoom
    /// range pays that column's cost instead of full resolution. This driver
    /// still simplifies nothing itself — it only reads a different, already
    /// existing column, exactly the contract `GeometryVariantDecl` states.
    /// Two consequences worth naming, both of which follow from "the tile
    /// serves the variant": every downstream step here (the vertex budget,
    /// the 4326 reprojection, the exact `S_INTERSECTS` post-filter) sees the
    /// variant's own geometry, so a feature is carried by this tile iff what
    /// the tile would actually render passes them; and the R*Tree prune that
    /// selected the candidate rows still ran against the base column's index
    /// (`sql::build_tile_plan`'s own doc). A collection declaring no variants
    /// resolves to the base column at every zoom and is byte-for-byte
    /// unaffected.
    async fn mvt_tile_inner(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
        filter: Option<&Filter>,
    ) -> Result<Option<Bytes>> {
        let storage_srid = collection.srid;
        if !matches!(storage_srid, Some(3857) | Some(4326)) {
            return Err(GeopackageError::UnsupportedTileCrs {
                collection: collection.id.clone(),
                found: storage_srid,
            });
        }

        // Always the tile's own EPSG:3857 bounds — the CRS the workspace's
        // WebMercatorQuad grid is defined in, and what the shared encoder
        // needs regardless of storage SRID.
        let tile_envelope = tile_envelope_3857(coord)?;
        // The R*Tree bbox pushdown's own query window: the tile envelope
        // as-is when storage already matches it, or reprojected into the
        // collection's own storage CRS (degrees, for 4326) so pruning still
        // compares like units against the R*Tree's stored coordinates.
        let query_envelope = if storage_srid == Some(4326) {
            let [minx, miny, maxx, maxy] = tile_envelope;
            let (min_lon, min_lat) = crate::crs::web_mercator_to_lonlat(minx, miny);
            let (max_lon, max_lat) = crate::crs::web_mercator_to_lonlat(maxx, maxy);
            [min_lon, min_lat, max_lon, max_lat]
        } else {
            tile_envelope
        };
        let cap = heuristics::effective_feature_cap(
            &collection.tiles.caps,
            coord.z,
            collection.row_estimate,
        );

        let table = collection.resolved_table().to_string();
        let pk = collection.resolved_pk().to_string();
        // `#104`/`#200`: the tiles lane reads whichever declared
        // `geometry_variants` column covers this tile's own zoom, and the
        // base `geometry` column when none does — the same per-zoom
        // selection `tellurion-postgis::sql::build_mvt_candidate_fragment`
        // already applies, now driver-neutral. Every other lane in this file
        // (items/item/write/outbox) stays on `resolved_geometry()`: a variant
        // is a tile-rendering detail, never what a feature response returns.
        // The bbox pushdown keeps pruning against the *base* column's R*Tree
        // regardless — see `sql::build_tile_plan`'s own doc for why the read
        // column and the indexed column are allowed to differ.
        let geom = collection.resolved_geometry_for_zoom(coord.z).to_string();
        let index_geom = collection.resolved_geometry().to_string();
        let layer_name = collection.external_id().to_string();
        let filter = filter.cloned();
        let tile_properties = collection.tile_properties.clone();

        let (query_sql, params, intersects_check) = sql::build_tile_plan(
            &table,
            &pk,
            &geom,
            &index_geom,
            &tile_properties,
            query_envelope,
            cap,
            filter.as_ref(),
        )?;

        let vertex_budget = collection
            .settings
            .tile_vertex_budget
            .unwrap_or(DEFAULT_TILE_VERTEX_BUDGET);
        let request = TileRequest::new(
            coord,
            layer_name,
            tile_properties.clone(),
            usize::try_from(cap).unwrap_or(usize::MAX),
            vertex_budget,
            MVT_EXTENT,
            if storage_srid == Some(4326) {
                SourceCrs::Crs84
            } else {
                SourceCrs::WebMercator
            },
        )
        // GeoPackage's established wire contract encoded crossing geometry
        // without topology clipping. Other adapters use the request's safe
        // clipped default unless they explicitly share that legacy contract.
        .preserve_unclipped_geometry();
        let tile_properties_for_read = tile_properties.clone();
        let outcome = pool::with_reader(Arc::clone(&self.pool), move |conn| {
            use geozero::ToGeo;

            let mut stmt = conn.prepare(&query_sql)?;
            let refs = boxed_param_refs(&params);
            let mut rows = stmt.query(refs.as_slice())?;
            let features = std::iter::from_fn(|| loop {
                let row = match rows.next() {
                    Ok(Some(row)) => row,
                    Ok(None) => return None,
                    Err(error) => return Some(Err(error.into())),
                };
                let pk_value: i64 = match row.get(0) {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error.into())),
                };
                let blob: Vec<u8> = match row.get(1) {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error.into())),
                };
                let decoded = match gpb::decode(&blob) {
                    Ok(decoded) => decoded,
                    Err(error) => return Some(Err(error)),
                };
                if decoded.is_empty {
                    continue;
                }
                // The R*Tree pushdown only narrows candidates to a bbox
                // overlap. Keep the exact predicate in this adapter and run
                // it only for rows the bounded encoder actually pulls.
                if let Some(check) = &intersects_check {
                    match intersects::row_intersects(decoded.wkb, check) {
                        Ok(true) => {}
                        Ok(false) => continue,
                        Err(error) => return Some(Err(error)),
                    }
                }
                let geometry = match geozero::wkb::Wkb(decoded.wkb).to_geo() {
                    Ok(geometry) => geometry,
                    Err(error) => return Some(Err(GeopackageError::Geozero(error))),
                };
                let mut properties = Vec::with_capacity(tile_properties_for_read.len());
                for (offset, name) in tile_properties_for_read.iter().enumerate() {
                    let value = match row.get_ref(2 + offset) {
                        Ok(value) => value,
                        Err(error) => return Some(Err(error.into())),
                    };
                    match tile_scalar(value, name) {
                        Ok(value) => properties.push((name.clone(), value)),
                        Err(error) => return Some(Err(error)),
                    }
                }
                return Some(Ok(TileFeature::new(
                    pk_value.to_string(),
                    geometry,
                    properties,
                )));
            });
            encode_tile_with_outcome(request, features)
        })
        .await?;

        if outcome.vertex_limit_exceeded {
            metrics::counter!("tile_vertex_budget_exceeded_total", "backend" => "geopackage")
                .increment(1);
            tracing::warn!(
                collection = %collection.id,
                z = coord.z,
                x = coord.x,
                y = coord.y,
                vertex_budget,
                spent = outcome.vertices_used,
                "tile exceeded its vertex budget; dropping the marginal geometry rather than \
                 serving an unbounded encode"
            );
        }

        Ok(outcome.tile)
    }

    /// `WriteSink::apply`: commits the data mutation and the outbox
    /// obligation in one SQLite transaction on the single writer connection
    /// — see `pool.rs`'s own top-level doc for why writes serialize through
    /// exactly one connection regardless of WAL's multi-reader concurrency.
    async fn write_apply_inner(
        &self,
        collection: &CollectionDecl,
        mutation: Mutation,
        requested_crs: RequestedCrs,
    ) -> Result<Sequence> {
        // `#87`: same unconditional refusal as `item_inner` — see that
        // guard's own doc.
        if collection.id_type != IdType::Integer {
            return Err(GeopackageError::IdTypeUnsupported(collection.id.clone()));
        }
        let pk_value: i64 = mutation
            .feature_id
            .parse()
            .map_err(|_| GeopackageError::InvalidFeatureId(mutation.feature_id.clone()))?;

        let has_geometry = matches!(
            &mutation.kind,
            MutationKind::Upsert(feature)
                if feature.get("geometry").is_some_and(|geometry| !geometry.is_null())
        );
        let table = collection.resolved_table().to_string();
        let pk = collection.resolved_pk().to_string();
        let geom = collection.resolved_geometry().to_string();
        let srid = crate::crs::ensure_write_srid(
            &collection.id,
            collection.srid,
            requested_crs,
            has_geometry,
        )?;
        let declared: HashSet<String> = collection
            .schema
            .as_ref()
            .map(|s| s.properties.iter().map(|p| p.name.clone()).collect())
            .unwrap_or_default();

        pool::with_writer(Arc::clone(&self.pool), move |conn| {
            let tx = conn.transaction().map_err(GeopackageError::from)?;

            let (statement_sql, statement_params, outbox_payload) = match &mutation.kind {
                MutationKind::Upsert(feature) => {
                    let properties = feature
                        .get("properties")
                        .and_then(serde_json::Value::as_object)
                        .cloned()
                        .unwrap_or_default();
                    let geometry = feature.get("geometry");

                    let mut known_columns = declared.clone();
                    known_columns.extend(
                        catalog::attribute_columns(&tx, &table, &geom)?
                            .into_iter()
                            .map(|c| c.name),
                    );

                    let plan = write_sql::build_upsert_plan(
                        &table,
                        &pk,
                        &geom,
                        srid,
                        pk_value,
                        geometry,
                        &properties,
                        &known_columns,
                        requested_crs,
                    )?;
                    (plan.sql, plan.params, Some(feature.clone()))
                }
                MutationKind::Delete => {
                    let (sql, params) = write_sql::build_delete_plan(&table, &pk, pk_value)?;
                    (sql, params, None)
                }
            };
            let kind_text = match &mutation.kind {
                MutationKind::Upsert(_) => "upsert",
                MutationKind::Delete => "delete",
            };

            // `#141`: where the feature is BEFORE the statement that moves
            // it, read inside this same transaction so nothing can slip
            // between the two.
            let prior = stored_crs84_extent(&tx, &table, &pk, &geom, pk_value, srid)?;

            let statement_refs = boxed_param_refs(&statement_params);
            tx.execute(&statement_sql, statement_refs.as_slice())
                .map_err(GeopackageError::from)?;

            // `#142`: and where it is after — read back off the file, so the
            // recorded extent describes what was actually stored rather than
            // what the request body said in a CRS the outbox never records.
            // A delete leaves nothing behind, which is a recorded `None`.
            let current = match &mutation.kind {
                MutationKind::Upsert(_) => {
                    stored_crs84_extent(&tx, &table, &pk, &geom, pk_value, srid)?
                }
                MutationKind::Delete => Ok(None),
            };

            let (outbox_sql, outbox_params) = write_sql::build_outbox_insert_plan(
                &table,
                &mutation.feature_id,
                kind_text,
                outbox_payload.as_ref(),
                fold_extent(prior, current),
            )?;
            let outbox_refs = boxed_param_refs(&outbox_params);
            tx.execute(&outbox_sql, outbox_refs.as_slice())
                .map_err(|e| map_outbox_missing(e, &table))?;
            let sequence = tx.last_insert_rowid();

            tx.commit().map_err(GeopackageError::from)?;
            Ok(Sequence(sequence as u64))
        })
        .await
    }

    /// `WriteSink::apply_batch` (`#114`): every mutation applies inside ONE
    /// SQLite transaction on the single writer connection, each behind its
    /// own named `SAVEPOINT` — the embedded-driver counterpart of
    /// `tellurion-postgis`'s per-item savepoint inside one backend
    /// transaction, just synchronous throughout since this whole call
    /// already runs on the blocking thread pool via `pool::with_writer`. A
    /// `#87` non-integer `id_type` collection refuses the WHOLE batch up
    /// front, the same unconditional guard `write_apply_inner` applies per
    /// single item — the GeoPackage format itself has no other primary-key
    /// value-space to accept a batch item's id under.
    async fn write_apply_batch_inner(
        &self,
        collection: &CollectionDecl,
        mutations: Vec<Mutation>,
        requested_crs: RequestedCrs,
        strict: bool,
    ) -> Result<Vec<BatchItemResult>> {
        if collection.id_type != IdType::Integer {
            return Err(GeopackageError::IdTypeUnsupported(collection.id.clone()));
        }

        let has_geometry = mutations.iter().any(|mutation| {
            matches!(
                &mutation.kind,
                MutationKind::Upsert(feature)
                    if feature.get("geometry").is_some_and(|geometry| !geometry.is_null())
            )
        });
        let table = collection.resolved_table().to_string();
        let pk = collection.resolved_pk().to_string();
        let geom = collection.resolved_geometry().to_string();
        let srid = crate::crs::ensure_write_srid(
            &collection.id,
            collection.srid,
            requested_crs,
            has_geometry,
        )?;
        let declared: HashSet<String> = collection
            .schema
            .as_ref()
            .map(|s| s.properties.iter().map(|p| p.name.clone()).collect())
            .unwrap_or_default();

        pool::with_writer(Arc::clone(&self.pool), move |conn| {
            let mut known_columns = declared.clone();
            known_columns.extend(
                catalog::attribute_columns(conn, &table, &geom)?
                    .into_iter()
                    .map(|c| c.name),
            );

            let mut tx = conn.transaction().map_err(GeopackageError::from)?;
            let mut results = Vec::with_capacity(mutations.len());

            for (index, mutation) in mutations.into_iter().enumerate() {
                let feature_id = mutation.feature_id.clone();

                let plan = build_batch_item_plan(
                    &table,
                    &pk,
                    &geom,
                    srid,
                    &mutation,
                    &known_columns,
                    requested_crs,
                );
                let outcome = match plan {
                    Err(err) if err.is_deterministic_batch_refusal() => {
                        BatchItemOutcome::Refused(err.into())
                    }
                    Err(err) => return Err(err),
                    Ok(plan) => {
                        let mut savepoint = tx
                            .savepoint_with_name(format!("batch_item_{index}"))
                            .map_err(GeopackageError::from)?;

                        // `#141`: where this feature is before the statement
                        // that moves it, inside its own savepoint.
                        let prior = stored_crs84_extent(
                            &savepoint,
                            &table,
                            &pk,
                            &geom,
                            plan.pk_value,
                            srid,
                        )?;

                        let refs = boxed_param_refs(&plan.statement_params);
                        if let Err(error) = savepoint.execute(&plan.statement_sql, refs.as_slice())
                        {
                            let error = GeopackageError::from(error);
                            savepoint.rollback().map_err(GeopackageError::from)?;
                            if error.is_deterministic_batch_refusal() {
                                savepoint.commit().map_err(GeopackageError::from)?;
                                BatchItemOutcome::Refused(error.into())
                            } else {
                                return Err(error);
                            }
                        } else {
                            let current = if plan.deletes {
                                Ok(None)
                            } else {
                                stored_crs84_extent(
                                    &savepoint,
                                    &table,
                                    &pk,
                                    &geom,
                                    plan.pk_value,
                                    srid,
                                )?
                            };
                            let (outbox_sql, outbox_params) = write_sql::build_outbox_insert_plan(
                                &table,
                                &feature_id,
                                plan.kind,
                                plan.payload.as_ref(),
                                fold_extent(prior, current),
                            )?;
                            let outbox_refs = boxed_param_refs(&outbox_params);
                            if let Err(error) =
                                savepoint.execute(&outbox_sql, outbox_refs.as_slice())
                            {
                                let error = map_outbox_missing(error, &table);
                                savepoint.rollback().map_err(GeopackageError::from)?;
                                return Err(error);
                            }
                            let sequence = savepoint.last_insert_rowid();
                            let sequence = u64::try_from(sequence)
                                .map_err(|_| GeopackageError::OutboxSequenceInvalid(sequence))?;
                            savepoint.commit().map_err(GeopackageError::from)?;
                            BatchItemOutcome::Applied(Sequence(sequence))
                        }
                    }
                };

                let refused = matches!(outcome, BatchItemOutcome::Refused(_));
                results.push(BatchItemResult {
                    feature_id,
                    outcome,
                });
                if refused && strict {
                    break;
                }
            }

            tx.commit().map_err(GeopackageError::from)?;
            Ok(results)
        })
        .await
    }

    async fn read_after_inner(
        &self,
        collection: &CollectionDecl,
        after: Sequence,
        limit: u32,
    ) -> Result<Vec<Obligation>> {
        let table = collection.resolved_table().to_string();
        let (query_sql, params) = write_sql::build_read_after_plan(&table, after.0, limit)?;
        let table_for_error = table.clone();

        pool::with_reader(Arc::clone(&self.pool), move |conn| {
            let mut stmt = conn
                .prepare(&query_sql)
                .map_err(|e| map_outbox_missing(e, &table_for_error))?;
            let refs = boxed_param_refs(&params);
            let mut rows = stmt
                .query(refs.as_slice())
                .map_err(|e| map_outbox_missing(e, &table_for_error))?;

            let mut obligations = Vec::new();
            while let Some(row) = rows.next()? {
                let sequence: i64 = row.get(0)?;
                let feature_id: String = row.get(1)?;
                let kind: String = row.get(2)?;
                let payload_text: Option<String> = row.get(3)?;
                let committed_at_text: String = row.get(4)?;
                // `#141`/`#142`: `NULL` here is an outbox row written before
                // the column existed — `ObligationExtent::Unrecorded`, read
                // by the invalidation consumer as UNKNOWN (conservative
                // whole-collection bump), never as "nothing moved".
                let extent_text: Option<String> = row.get(5)?;
                // This driver's own fixed `strftime` shape (`write_sql`'s
                // own doc) — `#115`.
                let committed_at = tellurion_core::parse_utc_datetime_text(&committed_at_text)
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            4,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                let sequence = Sequence(sequence as u64);
                let kind = match kind.as_str() {
                    "delete" => MutationKind::Delete,
                    _ => {
                        let payload = match payload_text {
                            Some(text) => serde_json::from_str(&text)?,
                            None => serde_json::Value::Null,
                        };
                        MutationKind::Upsert(payload)
                    }
                };
                obligations.push(Obligation {
                    sequence,
                    feature_id,
                    kind,
                    version: sequence,
                    committed_at,
                    extent: write_sql::decode_extent(extent_text.as_deref()),
                });
            }
            Ok(obligations)
        })
        .await
    }

    async fn primary_high_water_inner(&self, collection: &CollectionDecl) -> Result<Sequence> {
        let table = collection.resolved_table().to_string();
        let sql = write_sql::build_primary_high_water_plan(&table)?;
        let table_for_error = table.clone();

        pool::with_reader(Arc::clone(&self.pool), move |conn| {
            let high_water: i64 = conn
                .query_row(&sql, [], |row| row.get(0))
                .map_err(|e| map_outbox_missing(e, &table_for_error))?;
            Ok(Sequence(high_water as u64))
        })
        .await
    }

    /// `OutboxSource::prune_before` (`#160`): removes one bounded prefix
    /// from the outbox through the same single writer connection every
    /// GeoPackage mutation uses. The retention worker computes and logs the
    /// consumer-aware floor; this driver only applies that supplied bound.
    async fn prune_before_inner(
        &self,
        collection: &CollectionDecl,
        floor: Sequence,
        batch_size: u32,
    ) -> Result<u64> {
        let table = collection.resolved_table().to_string();
        let (sql, params) = write_sql::build_prune_before_plan(&table, floor.0, batch_size)?;
        let table_for_error = table.clone();

        pool::with_writer(Arc::clone(&self.pool), move |conn| {
            let refs = boxed_param_refs(&params);
            let removed = conn
                .execute(&sql, refs.as_slice())
                .map_err(|error| map_outbox_missing(error, &table_for_error))?;
            Ok(removed as u64)
        })
        .await
    }
}

#[async_trait]
impl CatalogSource for GeopackageBackend {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        self.catalog_inner().await.map_err(Into::into)
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
impl FeatureSource for GeopackageBackend {
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

    /// This driver compiles a `Filter` to bound-parameter SQLite SQL (`sql::
    /// compile_filter`), refusing by name any construct the dialect cannot
    /// faithfully express (exact spatial predicates beyond `bbox`) rather
    /// than dropping or approximating it — see that module's own doc.
    fn filter_capable(&self) -> bool {
        true
    }

    /// `#105`: `sql::compile_filter` compiles comparison/`IS NULL`,
    /// `LIKE`/`BETWEEN`/`IN`, `S_INTERSECTS`, and every temporal predicate
    /// (including the twelve-op `Filter::Temporal`) — but refuses
    /// `Filter::Spatial` (the six wider spatial predicates beyond
    /// `S_INTERSECTS`) by name (`GeopackageError::SpatialPredicateUnsupported`,
    /// see that module's own "bbox pushdown" doc): this dialect has no
    /// geometry engine loaded to evaluate them exactly, and reusing the
    /// R*Tree's own coarse bbox test in their place would be silently wrong
    /// for anything but a bbox-shaped literal. `case-insensitive-comparison`
    /// stays excluded for the same reason `tellurion-postgis` excludes it —
    /// see `filter::CQL2_CONFORMANCE_CLASSES`'s own doc.
    ///
    /// ## `basic-spatial-functions` is withheld as well (`#134`)
    ///
    /// `#105` kept this class declared for GeoPackage on the strength of
    /// `S_INTERSECTS` being compiled at all, inheriting the pre-`#105`
    /// workspace-wide stance rather than testing it. Tested against the
    /// published CQL2 standard (OGC 21-065r2, verified 2026-08 against
    /// `docs.ogc.org/is/21-065r2/21-065r2.html`), the stance does not hold:
    /// this driver compiles `S_INTERSECTS` only in a *restricted positional*
    /// form — at most one per filter, and never beneath `OR`/`NOT`
    /// (`sql::collect_intersects_check`) — and the class is defined in terms
    /// of the general form.
    ///
    /// The chain, in the standard's own words:
    ///
    /// 1. Requirements Class "Basic Spatial Functions" names Requirements
    ///    Class "Basic CQL2" as a **Dependency**. Basic CQL2's Requirement 1
    ///    (`/req/basic-cql2/cql2-filter`, clause A) reads: "A server SHALL
    ///    support a CQL2 filter expression composed of a logically connected
    ///    series of one or more predicates as described by the BNF rule
    ///    `booleanExpression` in CQL2 BNF with the exception that the rules
    ///    `isLikePredicate`, `isBetweenPredicate`, `isInListPredicate`,
    ///    **`spatialPredicate`**, `temporalPredicate`, `arrayPredicate`,
    ///    `function` and `arithmeticExpression` ... do not have to be
    ///    supported." Declaring `basic-spatial-functions` is precisely what
    ///    lifts `spatialPredicate` out of that exception list: the promise is
    ///    `booleanExpression` *including* the spatial predicate, not the
    ///    spatial predicate in isolation.
    ///
    /// 2. The BNF (normative Annex B) puts no positional or count limit on
    ///    where a predicate may sit: `predicate = comparisonPredicate |
    ///    spatialPredicate | temporalPredicate | arrayPredicate;`
    ///    `booleanPrimary = function | predicate | booleanLiteral | "("
    ///    booleanExpression ")";` `booleanFactor = ["NOT"] booleanPrimary;`
    ///    `booleanTerm = booleanFactor [ {"AND" booleanFactor} ];`
    ///    `booleanExpression = booleanTerm [ {"OR" booleanTerm} ];`. A
    ///    `spatialPredicate` is a `predicate`, so it may appear under `NOT`,
    ///    in any `OR` branch, and any number of times.
    ///
    /// 3. The class enumerates exactly two narrowings a server may take, and
    ///    both are about *operands*, never about position or count:
    ///    Permission 6 (`/per/basic-spatial-functions/spatial-predicates`) —
    ///    "The server MAY not support a `spatialInstance` as the first
    ///    operand (rule `geomExpression`) in rule `spatialPredicate`" and
    ///    "... a `propertyName` as the second operand ..." — and Permission 7
    ///    (`/per/basic-spatial-functions/spatial-data-types`) — "The server
    ///    MAY only support `pointTaggedText` and `bboxTaggedText` in rule
    ///    `spatialInstance`". A positional restriction is not among them.
    ///
    /// 4. The normative Abstract Test Suite makes it executable rather than a
    ///    reading — clause 2: "Conformance with this standard shall be
    ///    checked using **all** the relevant tests specified in Annex A of
    ///    this document." Conformance Test 26
    ///    (`/conf/basic-spatial-functions/test-data`, Requirements: "all
    ///    requirements") asserts an exact item count for, among others,
    ///    `S_INTERSECTS(geom,BBOX(0,40,10,50)) and
    ///    S_INTERSECTS(geom,BBOX(5,50,10,60))`,
    ///    `S_INTERSECTS(geom,BBOX(0,40,10,50)) and not
    ///    S_INTERSECTS(geom,BBOX(5,50,10,60))`, and
    ///    `S_INTERSECTS(geom,BBOX(0,40,10,50)) or
    ///    S_INTERSECTS(geom,BBOX(-90,40,-60,50))` — the two-predicate, the
    ///    `NOT` and the `OR` shape, all three of which this driver refuses.
    ///    Conformance Test 27 (`/conf/basic-spatial-functions/logical`) then
    ///    evaluates `((NOT {p1} AND {p2}) OR ({p3} and NOT {p4}) or not ({p1}
    ///    AND {p4}))` over the predicates Conformance Test 25 stored — which
    ///    is exactly where the `S_INTERSECTS` predicates come from.
    ///
    /// (Two identifiers in that Abstract Test Suite do not resolve, so the
    /// requirements above are cited by their real ids or in prose rather than
    /// through Test 25's: Conformance Test 25's "Requirements" line names
    /// `/req/basic-spatial-functions/spatial-predicate` and
    /// `/req/basic-spatial-functions/spatial-data-types`, but the document
    /// numbers the first `/req/basic-spatial-operators/spatial-predicate`
    /// (Requirement 10) and states the second only as Permission 7, never as
    /// a requirement of this class. Test 25's method also evaluates a fourth
    /// expression — `S_INTERSECTS(...) AND S_INTERSECTS(...)` — that its
    /// "Then" clause then makes no assertion about; Conformance Test 26's
    /// table covers that shape regardless, with an expected count.)
    ///
    /// So the class is withheld. What this does *not* change: the restricted
    /// form still works and is still answered exactly (R*Tree bbox prune plus
    /// the in-process `geo::Intersects` test), and the general form is still
    /// refused by name — `GeopackageError::IntersectsUnsupported`, a 400
    /// `InvalidParameter` — never silently answered by the coarse bbox test.
    /// This narrows an advertisement, not a capability. `basic-cql2` itself is
    /// untouched: Requirement 1 excepts `spatialPredicate` by name, so
    /// declaring Basic CQL2 while supporting only a restricted `S_INTERSECTS`
    /// is exactly what that exception permits.
    ///
    /// Re-earning it is a real change to `sql.rs`, not a re-reading: the
    /// restriction is a soundness consequence of the bbox-prune-then-exact
    /// strategy (see `sql::collect_intersects_check`'s own doc), and lifting
    /// it means evaluating the whole boolean tree per candidate row instead
    /// of ANDing one R*Tree subquery into the SQL. Widening the literal
    /// grammar cannot substitute for it — `basic-spatial-functions-plus`
    /// (Requirement 12) *depends on* this class, so the positional work comes
    /// first either way. `intersects_general_form_and_declared_class_agree`
    /// in `tests/driver_contract.rs` ties the two sides together so neither
    /// can move without the other.
    fn cql2_conformance_classes(&self) -> Vec<&'static str> {
        vec![
            tellurion_core::filter::CQL2_CLASS_BASIC,
            tellurion_core::filter::CQL2_CLASS_CQL2_TEXT,
            tellurion_core::filter::CQL2_CLASS_CQL2_JSON,
            tellurion_core::filter::CQL2_CLASS_ADVANCED_COMPARISON_OPERATORS,
            tellurion_core::filter::CQL2_CLASS_TEMPORAL_FUNCTIONS,
        ]
    }

    // Read-side `crs_capable` stays at the trait default (`false`): the
    // narrow pure-Rust transform is a tile/write capability, not OGC API
    // Features Part 2 response reprojection. The independent WriteSink
    // implementation below advertises its own write-side capability.
}

#[async_trait]
impl TileSource for GeopackageBackend {
    async fn mvt_tile(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
        filter: Option<&Filter>,
    ) -> CoreResult<Option<Bytes>> {
        self.mvt_tile_inner(collection, coord, filter)
            .await
            .map_err(Into::into)
    }

    /// Same compiler `FeatureSource::filter_capable` documents — `sql::
    /// build_tile_plan` ANDs a `#34` grant filter into the tile query's own
    /// `WHERE` clause via the identical `sql::compile_filter`.
    fn filter_capable(&self) -> bool {
        true
    }
}

#[async_trait]
impl WriteSink for GeopackageBackend {
    async fn apply(&self, collection: &CollectionDecl, mutation: Mutation) -> CoreResult<Sequence> {
        self.write_apply_inner(collection, mutation, RequestedCrs::Omitted)
            .await
            .map_err(Into::into)
    }

    fn crs_capable(&self) -> bool {
        true
    }

    fn features_conformance_classes(&self, collection: &CollectionDecl) -> Vec<&'static str> {
        if matches!(collection.srid, Some(4326 | 3857)) {
            vec![tellurion_core::FEATURES_PART4_FEATURES_CLASS]
        } else {
            Vec::new()
        }
    }

    async fn apply_with_crs(
        &self,
        collection: &CollectionDecl,
        mutation: Mutation,
        requested_crs: RequestedCrs,
    ) -> CoreResult<Sequence> {
        self.write_apply_inner(collection, mutation, requested_crs)
            .await
            .map_err(Into::into)
    }

    /// Withheld (`#150`), deliberately. This driver previously declared the
    /// OGC API Features — Part 4 Optimistic Locking, ETags class on the
    /// strength of `apply` committing the data mutation and the outbox
    /// obligation in ONE SQLite transaction. That is necessary but not
    /// sufficient: it makes a `FeatureSource::item` read issued right after a
    /// write reflect exactly what committed, but it says nothing about the
    /// gap between the read the `If-Match` guard hashes and the write that
    /// guard protects. Two writers whose preconditions both pass inside that
    /// gap both commit, and the second silently clobbers the first — which is
    /// precisely the lost update the class exists to prevent, so declaring it
    /// was an overclaim.
    ///
    /// Closing that gap needs a per-row version the backend can compare
    /// INSIDE the write statement (`WriteSink::row_version`/
    /// `apply_conditional`; PostGIS uses `xmin`). SQLite has no such thing:
    /// no system row-version column, and `PRAGMA data_version` is
    /// database-wide, so it would refuse a write over any unrelated
    /// concurrent change to the file. A real witness would need a version
    /// COLUMN, and the server never issues DDL — that is `tellurion-ingest`'s
    /// to own, and nothing in this workspace provisions one today.
    ///
    /// So this driver reports honestly that it cannot, and
    /// `Router::locking_conformance_classes`'s fold stops advertising the
    /// class for a deployment whose write lane is GeoPackage. A conditional
    /// write against such a collection is refused BY NAME (both trait methods
    /// keep their `CapabilityUnsupported` defaults), never silently
    /// downgraded to the racy pre-transaction check. A request carrying no
    /// precondition is unaffected in every respect.
    fn locking_conformance_classes(&self) -> Vec<&'static str> {
        Vec::new()
    }

    fn update_conformance_classes(&self) -> Vec<&'static str> {
        vec![tellurion_core::outbox::UPDATE_CONFORMANCE_CLASS]
    }

    /// `#114`: every mutation in `mutations` commits (or is cleanly
    /// discarded) inside ONE SQLite transaction — see
    /// `write_apply_batch_inner` for the per-item `SAVEPOINT` mechanics.
    async fn apply_batch(
        &self,
        collection: &CollectionDecl,
        mutations: Vec<Mutation>,
        requested_crs: RequestedCrs,
        strict: bool,
    ) -> CoreResult<Vec<BatchItemResult>> {
        self.write_apply_batch_inner(collection, mutations, requested_crs, strict)
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl OutboxSource for GeopackageBackend {
    async fn read_after(
        &self,
        collection: &CollectionDecl,
        after: Sequence,
        limit: u32,
    ) -> CoreResult<Vec<Obligation>> {
        self.read_after_inner(collection, after, limit)
            .await
            .map_err(Into::into)
    }

    async fn primary_high_water(&self, collection: &CollectionDecl) -> CoreResult<Sequence> {
        self.primary_high_water_inner(collection)
            .await
            .map_err(Into::into)
    }

    async fn prune_before(
        &self,
        collection: &CollectionDecl,
        floor: Sequence,
        batch_size: u32,
    ) -> CoreResult<u64> {
        self.prune_before_inner(collection, floor, batch_size)
            .await
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED_WEB_MERCATOR_ORIGIN: f64 = 20_037_508.342_789_244;

    fn collection_with_table(table: &str) -> CollectionDecl {
        serde_yaml::from_str(&format!(
            "id: demo\ncatalog: default\nstorage: main\ntable: \"{table}\"\ngeometry: geom\npk: id\n"
        ))
        .unwrap()
    }

    #[test]
    fn validate_collection_identifiers_accepts_a_well_formed_decl() {
        assert!(validate_collection_identifiers(&collection_with_table("demo")).is_ok());
    }

    #[test]
    fn validate_collection_identifiers_rejects_a_hyphenated_table_name() {
        assert!(validate_collection_identifiers(&collection_with_table("my-table")).is_err());
    }

    fn collection_with_variant_column(column: &str) -> CollectionDecl {
        let mut decl = collection_with_table("demo");
        decl.geometry_variants = vec![tellurion_core::GeometryVariantDecl {
            column: column.to_string(),
            minzoom: 0,
            maxzoom: 6,
        }];
        decl
    }

    /// `#104`: a declared variant column reaches the tiles lane's SELECT
    /// list, so it goes through the same eager whitelist as `table`/
    /// `geometry`/`pk`/`datetime` — a typo fails `Router::build` instead of
    /// only the tile requests inside the variant's own zoom range.
    #[test]
    fn validate_collection_identifiers_accepts_a_well_formed_variant_column() {
        assert!(
            validate_collection_identifiers(&collection_with_variant_column("geom_z6")).is_ok()
        );
    }

    #[test]
    fn validate_collection_identifiers_rejects_a_hyphenated_variant_column() {
        assert!(
            validate_collection_identifiers(&collection_with_variant_column("geom-z6")).is_err()
        );
    }

    #[test]
    fn tile_envelope_zoom_zero_covers_the_whole_projected_world() {
        let envelope = tile_envelope_3857(TileCoord { z: 0, x: 0, y: 0 }).unwrap();
        assert!((envelope[0] - (-EXPECTED_WEB_MERCATOR_ORIGIN)).abs() < 1e-6);
        assert!((envelope[1] - (-EXPECTED_WEB_MERCATOR_ORIGIN)).abs() < 1e-6);
        assert!((envelope[2] - EXPECTED_WEB_MERCATOR_ORIGIN).abs() < 1e-6);
        assert!((envelope[3] - EXPECTED_WEB_MERCATOR_ORIGIN).abs() < 1e-6);
    }

    #[test]
    fn tile_envelope_zoom_one_quadrant_is_a_quarter_of_the_world() {
        let envelope = tile_envelope_3857(TileCoord { z: 1, x: 0, y: 0 }).unwrap();
        assert!((envelope[0] - (-EXPECTED_WEB_MERCATOR_ORIGIN)).abs() < 1e-6);
        assert!((envelope[2] - 0.0).abs() < 1e-6);
        assert!((envelope[3] - EXPECTED_WEB_MERCATOR_ORIGIN).abs() < 1e-6);
    }
}
