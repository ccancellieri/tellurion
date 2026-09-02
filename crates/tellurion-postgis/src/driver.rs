//! The `postgis` `DriverFactory`, and the `FeatureSource` + `TileSource`
//! implementation backing it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use bytes::Bytes;
use deadpool_postgres::Pool;
use tokio_postgres::error::SqlState;
use tokio_postgres::types::ToSql;

use tellurion_core::{
    decide_tile_path, effective_cpu_count, heuristics, AssetRecord, AssetRecordStore,
    AttributeColumn, BatchItemOutcome, BatchItemResult, CatalogSource, CollectionDecl,
    DriverFactory, Error as CoreError, FeaturePage, FeatureSizeStats, FeatureSource, Filter,
    FinalizeOutcome, GeometryProfile, IdType, IndexSink, ItemsQuery, JobOutcome, JobRecord,
    JobScope, JobStatus, JobStore, JobSubmission, Lease, LeaseGuard, LeaseHold, LeaseKey, Mutation,
    MutationKind, NewAssetRecord, Obligation, ObligationExtent, OutboxSource, PhysicalCollection,
    PropertyType, RequestedCrs, Result as CoreResult, RowVersion, SearchPage, SearchQuery,
    SearchSource, Sequence, SpatialExtent, StacMetadataSource, StorageDecl, StorageDriver,
    TileCoord, TileMatrixSet, TileSimplificationPath, TileSource, VertexStats, VolumeMesh,
    VolumeSource, WriteSink, DEFAULT_TILE_VERTEX_BUDGET, VERTEX_BUDGET_RETRY_TOLERANCE_FACTOR,
};

use crate::asset_sql;
use crate::cancel::run_cancellable;
use crate::catalog::{
    ATTRIBUTE_SCHEMA_SQL, ATTRIBUTE_SCHEMA_SQL_NO_GEOMETRY, CATALOG_QUERY, ESTIMATED_EXTENT_SQL,
    ROW_ESTIMATE_SQL, TEMPORAL_COLUMN_SQL, VOLUME_GEOMETRY_KIND_SQL,
};
use crate::error::{PostgisError, Result};
use crate::ewkb::decode_solid;
use crate::ident::quote_ident;
use crate::index_sql;
use crate::job_sql;
use crate::lease_sql;
use crate::pool::build_pool;
use crate::sql::{
    build_geometry_profile_plan, build_item_plan, build_items_plan, build_mvt_budgeted_plan,
    build_mvt_plan, build_mvt_vertex_total_plan, build_real_extent_plan, build_volume_plan,
    sample_percentage, CountPlan, PkValue, SqlParam, MVT_EXTENT, WORLD_CRS84_METERS_PER_DEGREE,
};
use crate::stac_sql;
use crate::volume::{
    build_volume_mesh, effective_volume_vertex_cap, TileTransform, VolumeGeometryKind,
};
use crate::write_sql;

fn boxed_params(params: &[SqlParam]) -> Vec<Box<dyn ToSql + Sync + Send>> {
    params.iter().map(SqlParam::boxed).collect()
}

fn param_refs(boxed: &[Box<dyn ToSql + Sync + Send>]) -> Vec<&(dyn ToSql + Sync)> {
    boxed
        .iter()
        .map(|p| p.as_ref() as &(dyn ToSql + Sync))
        .collect()
}

fn postgis_items_budget_error(
    collection: &CollectionDecl,
    feature_id: String,
    cumulative_vertices: u64,
    limit: u64,
) -> PostgisError {
    metrics::counter!("items_vertex_budget_exceeded_total", "backend" => "postgis").increment(1);
    tracing::warn!(
        collection = collection.external_id(),
        feature_id,
        cumulative_vertices,
        limit,
        backend = "postgis",
        "exact item geometry exceeded the configured vertex budget"
    );
    PostgisError::ItemsVertexBudgetExceeded {
        collection: collection.external_id().to_string(),
        feature_id,
        cumulative_vertices,
        limit,
    }
}

/// Reads a pk column out of `row` at `idx`, typed per `id_type` (`#87`,
/// `#94`) — `i64` for `Integer`, `uuid::Uuid` for `Uuid`, `String` for
/// `Text`. The counterpart, on the read side, of `PkValue::as_sql_param` on
/// the bind side: every place this driver moves a pk value between Rust and
/// the wire goes through one of these two, never a raw untyped `try_get`.
fn read_pk_value(row: &tokio_postgres::Row, idx: usize, id_type: IdType) -> Result<PkValue> {
    match id_type {
        IdType::Integer => {
            let value: i64 = row.try_get(idx).map_err(PostgisError::from)?;
            Ok(PkValue::Integer(value))
        }
        IdType::Uuid => {
            let value: uuid::Uuid = row.try_get(idx).map_err(PostgisError::from)?;
            Ok(PkValue::Uuid(value))
        }
        IdType::Text => {
            let value: String = row.try_get(idx).map_err(PostgisError::from)?;
            Ok(PkValue::Text(value))
        }
    }
}

/// Registers the `postgis` driver. `request_timeout_s` is injected here
/// (rather than through `DriverFactory::build`, which only sees a single
/// `StorageDecl`) so every pooled connection's `statement_timeout` matches
/// the server's HTTP request ceiling; the wiring crate constructs this with
/// `AppConfig::server.request_timeout_s` before registering it.
pub struct PostgisDriverFactory {
    statement_timeout_ms: u64,
}

impl PostgisDriverFactory {
    pub fn new(request_timeout_s: u64) -> Self {
        Self {
            statement_timeout_ms: request_timeout_s.saturating_mul(1000),
        }
    }
}

impl DriverFactory for PostgisDriverFactory {
    fn name(&self) -> &str {
        "postgis"
    }

    fn build(&self, decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        let database_url = std::env::var(&decl.url_env).map_err(|_| {
            CoreError::Config(format!(
                "storage '{}': environment variable '{}' is not set",
                decl.id, decl.url_env
            ))
        })?;

        let effective_cores = effective_cpu_count();
        let pool_size = crate::pool::derive_pool_size(decl.pool_size, effective_cores);
        tracing::info!(
            storage = %decl.id,
            pool_size,
            effective_cores,
            explicit_override = decl.pool_size.is_some(),
            "postgis pool: connection pool size (explicit config wins over derived)"
        );

        let pool = build_pool(&database_url, self.statement_timeout_ms, pool_size)
            .map_err(|e| CoreError::Config(format!("storage '{}': {e}", decl.id)))?;

        Ok(Arc::new(PostgisDriverImpl {
            backend: Arc::new(PostgisBackend { pool }),
            lease: Arc::new(PostgisLease {
                database_url,
                connect_timeout: Duration::from_millis(self.statement_timeout_ms),
            }),
            pool_size,
        }))
    }
}

struct PostgisDriverImpl {
    backend: Arc<PostgisBackend>,
    lease: Arc<PostgisLease>,
    pool_size: usize,
}

impl StorageDriver for PostgisDriverImpl {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::clone(&self.backend) as Arc<dyn CatalogSource>
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn FeatureSource>)
    }

    fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn TileSource>)
    }

    /// PostGIS is the primary this workspace's first write slice targets
    /// (`#25`, design doc section 8): the data mutation and the outbox
    /// insert commit in one transaction — see `write_apply_inner`.
    fn write_sink(&self) -> Option<Arc<dyn WriteSink>> {
        Some(Arc::clone(&self.backend) as Arc<dyn WriteSink>)
    }

    /// The read side of this same storage's outbox — a driver that
    /// advertises `write_sink` also advertises this, per the capability
    /// contract's own doc.
    fn outbox_source(&self) -> Option<Arc<dyn OutboxSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn OutboxSource>)
    }

    /// A derived index this driver can apply outbox obligations into
    /// (`#67`) — always advertised, the same driver-wide "can this backend
    /// do X at all" shape `write_sink`/`outbox_source` use; whether a given
    /// collection's `"<table>_index"` table has actually been provisioned
    /// is a request-time question `IndexSink::apply`/`applied_high_water`
    /// answer with `IndexTableMissing`, not this capability check.
    fn index_sink(&self) -> Option<Arc<dyn IndexSink>> {
        Some(Arc::clone(&self.backend) as Arc<dyn IndexSink>)
    }

    /// Freshness-aware search reads over this same derived index (`#67`) —
    /// always advertised, the identical driver-wide "can this backend do X
    /// at all" shape `index_sink` uses right above; whether a given
    /// collection's `routing.search` is even entitled to route here is
    /// `Router::resolve_search`'s config-load provisioning check, not this
    /// method.
    fn search_source(&self) -> Option<Arc<dyn SearchSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn SearchSource>)
    }

    /// `#41`: PostGIS can serve true solid geometry, so it always advertises
    /// this capability — the same driver-wide "can this backend do X at
    /// all" shape `feature_source`/`tile_source` use. This is deliberately a
    /// driver-wide signal, not a per-collection one: whether a *particular*
    /// collection's own geometry column is actually one of the supported 3D
    /// solid types is a fact `Router::resolve_volume` (`#70`) checks against
    /// that collection's own descriptor-derived `geometry_type` before ever
    /// trusting this answer, so a footprint+height `places3d` collection can
    /// share a storage entry with a genuinely solid one and still fall
    /// through to extrusion — no separate `storage:` entry needed for either.
    /// `volume_tile_inner`'s own `volume_geometry_kind` check is what a
    /// misconfigured collection (declares a solid `places3d` fixture but the
    /// backend disagrees at query time) still hits as a named refusal; the
    /// ordinary flat-footprint case never reaches it.
    fn volume_source(&self) -> Option<Arc<dyn VolumeSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn VolumeSource>)
    }

    /// The database-backed asset-records capability (assets-and-object-
    /// storage proposal, first slice) — always advertised, the identical
    /// driver-wide "can this backend do X at all" shape `write_sink`/
    /// `index_sink` use; whether a given collection's `"<table>_assets"`
    /// table has actually been provisioned is a request-time question the
    /// asset methods themselves answer with the named `AssetsTableMissing`,
    /// not this capability check.
    fn asset_record_store(&self) -> Option<Arc<dyn AssetRecordStore>> {
        Some(Arc::clone(&self.backend) as Arc<dyn AssetRecordStore>)
    }

    /// The per-item STAC metadata sidecar capability (`#202`) — always
    /// advertised, the identical driver-wide "can this backend do X at
    /// all" shape `index_sink`/`asset_record_store` use. It costs a
    /// collection that never opted in exactly nothing: `Router::
    /// resolve_stac_metadata` answers `Ok(None)` off `CollectionDecl::
    /// stac_metadata` without ever calling this. Whether an opted-in
    /// collection's `"<table>_stac"` table has actually been provisioned is
    /// a request-time question `StacMetadataSource::stac_metadata` answers
    /// with the named `StacTableMissing`, not this capability check.
    fn stac_metadata_source(&self) -> Option<Arc<dyn StacMetadataSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn StacMetadataSource>)
    }

    /// Single-leader leases via `pg_try_advisory_lock` (`#193`) — always
    /// advertised, the same driver-wide "can this backend do X at all"
    /// shape every capability above uses. Advertising costs nothing: no
    /// session is opened, no lock is taken, and nothing here runs at all
    /// until a consumer actually configured with a lease
    /// (`IndexApplierConfig::lease`) asks for one. See `PostgisLease` for
    /// why this is the one capability that does NOT ride the shared pool.
    fn lease(&self) -> Option<Arc<dyn Lease>> {
        Some(Arc::clone(&self.lease) as Arc<dyn Lease>)
    }

    /// The durable job ledger (`#182`) — always advertised, the identical
    /// driver-wide "can this backend do X at all" shape every capability
    /// above uses. Advertising costs a deployment that never declared
    /// `server.processes` exactly nothing: nothing here is reached until
    /// `Router::resolve_job_store` is asked for it by name, which only the
    /// boot-time Processes wiring ever does. Whether the ledger table has
    /// actually been provisioned is a request-time question the `JobStore`
    /// methods answer with the named `JobsTableMissing`, not this capability
    /// check.
    fn job_store(&self) -> Option<Arc<dyn JobStore>> {
        Some(Arc::clone(&self.backend) as Arc<dyn JobStore>)
    }

    /// The same whitelist `sql.rs` applies lazily on every query, run once
    /// eagerly here so a config typo (e.g. a hyphen in `table`) fails at
    /// `Router::build` time instead of 500-ing every request to this
    /// collection forever.
    fn validate_collection(&self, decl: &CollectionDecl) -> CoreResult<()> {
        validate_collection_identifiers(decl).map_err(Into::into)
    }

    fn capacity_hint(&self) -> Option<usize> {
        Some(self.pool_size)
    }
}

/// Eagerly validates whatever physical fields `decl` overrides — a config
/// typo (e.g. a hyphen in `table`) fails at `Router::build` time instead of
/// only at first request. Fields left `None` for `Router` to derive (`#19`)
/// have nothing to check yet: the value that ends up in a query always comes
/// straight from the backend's own catalog, and `sql.rs` whitelist-quotes it
/// there regardless, at query-build time.
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
    Ok(())
}

/// The `Lease` capability (`#193`): `pg_try_advisory_lock` on a session
/// this driver opens itself and hands to the winner, whose *lifetime is the
/// leadership*.
///
/// A dedicated connection, deliberately, not one from `self.backend.pool`:
/// a session-level advisory lock is released when the session ends, so
/// borrowing a pooled connection would either release the lock the instant
/// the checkout returned (useless) or — because `pool.rs` recycles with
/// `RecyclingMethod::Fast`, which never issues `RESET ALL` — leak the lock
/// into whatever unrelated query checked that connection out next
/// (dangerous). The cost is one extra connection per *leading* applier
/// task; a follower's failed attempt closes its session immediately, so
/// standby replicas hold nothing.
///
/// This is also what makes release free in every failure mode an operator
/// actually hits: a `SIGKILL`, a severed network, and a graceful shutdown
/// all end the session, and Postgres drops the lock for all three without
/// any expiry, heartbeat, or cleanup code on this side.
struct PostgisLease {
    /// Kept out of every log line and error message on purpose — it
    /// carries the password (`error.rs` only ever surfaces the
    /// `tokio_postgres` error, never this).
    database_url: String,
    /// Bound on opening the coordinator session, pinned to the same
    /// request ceiling `pool.rs` gives every other connection: a
    /// coordinator that will not answer within it is unreachable, and
    /// unreachable is an `Err` the caller must not read as "nobody leads"
    /// (`tellurion_core::lease::Lease::try_acquire`'s own contract).
    connect_timeout: Duration,
}

/// The hold whose existence IS the lease: the leader's own session.
/// Dropping it aborts the connection task, which closes the socket, which
/// is what makes Postgres release the advisory lock — there is no release
/// statement to forget, and no path where the lock outlives the value.
struct PostgisLeaseHold {
    client: tokio_postgres::Client,
    connection: tokio::task::JoinHandle<()>,
}

impl LeaseHold for PostgisLeaseHold {
    /// Answered from the client's own already-known state, never a round
    /// trip (`LeaseHold::is_live`'s contract). A closed client means the
    /// session ended, which means Postgres already released the lock and
    /// somebody else may already hold it — exactly the case the applier
    /// must stop draining on.
    fn is_live(&self) -> bool {
        !self.client.is_closed()
    }
}

impl Drop for PostgisLeaseHold {
    fn drop(&mut self) {
        self.connection.abort();
    }
}

impl PostgisLease {
    async fn try_acquire_inner(&self, key: &LeaseKey) -> Result<Option<LeaseGuard>> {
        let (client, connection) = tokio::time::timeout(
            self.connect_timeout,
            tokio_postgres::connect(&self.database_url, tokio_postgres::NoTls),
        )
        .await
        .map_err(|_| {
            PostgisError::LeaseCoordinatorTimeout(
                u64::try_from(self.connect_timeout.as_millis()).unwrap_or(u64::MAX),
            )
        })?
        .map_err(PostgisError::from)?;

        // Built before the query runs so that every early return from here
        // on — including a failed query — closes the session it opened.
        let hold = PostgisLeaseHold {
            client,
            connection: tokio::spawn(async move {
                let _ = connection.await;
            }),
        };

        let advisory_key = lease_sql::advisory_key(key.as_str());
        let row = hold
            .client
            .query_one(lease_sql::TRY_ACQUIRE_SQL, &[&advisory_key])
            .await
            .map_err(PostgisError::from)?;
        let acquired: bool = row.try_get("acquired").map_err(PostgisError::from)?;

        if !acquired {
            // Somebody else leads. Dropping `hold` closes this session
            // right here, so a follower costs the database nothing between
            // polls.
            return Ok(None);
        }

        // Won. Label the session so `pg_stat_activity` answers "who leads
        // this collection?" — and propagate a failure rather than swallow
        // it: this statement can only fail on a session that just broke,
        // which is a session whose lock Postgres has already released, so
        // reporting leadership here would be reporting a lock nobody holds.
        // Dropping `hold` on the way out closes it.
        hold.client
            .execute(
                lease_sql::LABEL_LEADER_SQL,
                &[&lease_sql::session_label(key.as_str())],
            )
            .await
            .map_err(PostgisError::from)?;

        Ok(Some(LeaseGuard::new(key.clone(), Box::new(hold))))
    }
}

#[async_trait]
impl Lease for PostgisLease {
    async fn try_acquire(&self, key: &LeaseKey) -> CoreResult<Option<LeaseGuard>> {
        Ok(self.try_acquire_inner(key).await?)
    }
}

struct PostgisBackend {
    pool: Pool,
}

impl PostgisBackend {
    /// Runs one `job_sql` plan that returns at most one ledger row (`#182`).
    ///
    /// Every `JobStore` method on this backend is exactly that shape — a
    /// single statement whose `RETURNING`/`SELECT` yields zero or one row —
    /// so the pool checkout, the cancellation wrapper, the
    /// `JobsTableMissing` rewrite and the row decode live here once instead
    /// of six times.
    async fn job_query_opt(&self, plan: job_sql::Plan) -> CoreResult<Option<JobRecord>> {
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let boxed = boxed_params(&plan.params);
        let row_opt = run_cancellable(client, move |client| async move {
            let refs = param_refs(&boxed);
            client.query_opt(&plan.sql, &refs).await
        })
        .await
        .map_err(map_jobs_missing)?;
        row_opt
            .as_ref()
            .map(job_sql::row_to_job_record)
            .transpose()
            .map(|row| row.map(|row| row.record))
            .map_err(Into::into)
    }

    async fn catalog_inner(&self) -> Result<Vec<PhysicalCollection>> {
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let rows = run_cancellable(client, |client| async move {
            client.query(CATALOG_QUERY, &[]).await
        })
        .await?;

        let mut collections = Vec::with_capacity(rows.len());
        for row in &rows {
            let name: String = row.try_get("table_name").map_err(PostgisError::from)?;
            let geometry_column: Option<String> =
                row.try_get("geometry_column").map_err(PostgisError::from)?;
            let srid: Option<i32> = row.try_get("srid").map_err(PostgisError::from)?;
            let geometry_type: Option<String> =
                row.try_get("geometry_type").map_err(PostgisError::from)?;
            let primary_key: Option<String> =
                row.try_get("primary_key").map_err(PostgisError::from)?;
            collections.push(PhysicalCollection {
                name,
                geometry_column,
                primary_key,
                srid,
                geometry_type,
            });
        }
        Ok(collections)
    }

    /// `#27`: `ST_EstimatedExtent` first (a `pg_statistic` lookup, no table
    /// scan); falls back to `ST_Extent` (a real scan) on any failure or a
    /// null/no-statistics result — see `catalog::ESTIMATED_EXTENT_SQL`'s doc
    /// comment for why both outcomes land here. Declines (`Ok(None)`) when
    /// the physical row carries no usable SRID/geometry column — an SRID of
    /// `0` means "unset"; `ST_Transform` cannot reproject that meaningfully.
    async fn extent_inner(&self, physical: &PhysicalCollection) -> Result<Option<SpatialExtent>> {
        let Some(geometry_column) = physical.geometry_column.as_deref() else {
            return Ok(None);
        };
        let Some(srid) = physical.srid.filter(|&srid| srid > 0) else {
            return Ok(None);
        };

        let estimated = self
            .extent_row(
                ESTIMATED_EXTENT_SQL,
                vec![
                    SqlParam::Text(physical.name.clone()),
                    SqlParam::Text(geometry_column.to_string()),
                    SqlParam::Int4(srid),
                ],
            )
            .await;
        if let Ok(Some(extent)) = estimated {
            return Ok(Some(extent));
        }

        let (sql, params) = build_real_extent_plan(&physical.name, geometry_column, srid)?;
        self.extent_row(&sql, params).await
    }

    /// Runs an extent query built for the four-column `minx`/`miny`/`maxx`/
    /// `maxy` shape both `ESTIMATED_EXTENT_SQL` and `build_real_extent_plan`
    /// produce, and turns a row of `NULL`s (an empty collection, or — for the
    /// estimated query — no statistics yet) into `Ok(None)` rather than a
    /// meaningless zero-sized bbox.
    async fn extent_row(&self, sql: &str, params: Vec<SqlParam>) -> Result<Option<SpatialExtent>> {
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let boxed = boxed_params(&params);
        let sql = sql.to_string();
        let row = run_cancellable(client, move |client| async move {
            let refs = param_refs(&boxed);
            client.query_one(&sql, &refs).await
        })
        .await?;

        let minx: Option<f64> = row.try_get("minx").map_err(PostgisError::from)?;
        let miny: Option<f64> = row.try_get("miny").map_err(PostgisError::from)?;
        let maxx: Option<f64> = row.try_get("maxx").map_err(PostgisError::from)?;
        let maxy: Option<f64> = row.try_get("maxy").map_err(PostgisError::from)?;

        Ok(match (minx, miny, maxx, maxy) {
            (Some(minx), Some(miny), Some(maxx), Some(maxy)) => Some(SpatialExtent {
                bbox: [minx, miny, maxx, maxy],
            }),
            _ => None,
        })
    }

    async fn items_inner(
        &self,
        collection: &CollectionDecl,
        query: &ItemsQuery,
    ) -> Result<FeaturePage> {
        let plan = build_items_plan(collection, query)?;
        let requested_limit = query.limit as usize;

        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let sql = plan.sql;
        let boxed = boxed_params(&plan.params);
        let rows = run_cancellable(client, move |client| async move {
            let refs = param_refs(&boxed);
            client.query(&sql, &refs).await
        })
        .await?;

        let has_more = rows.len() > requested_limit;
        let page_rows = if has_more {
            &rows[..requested_limit]
        } else {
            &rows[..]
        };

        let mut features = Vec::with_capacity(page_rows.len());
        let mut last_pk: Option<String> = None;
        for row in page_rows {
            let pk_value = read_pk_value(row, 0, collection.id_type)?;
            if let Some(limit) = collection.settings.items_vertex_budget {
                let cumulative_vertices: i64 = row.try_get(2).map_err(PostgisError::from)?;
                let cumulative_vertices = u64::try_from(cumulative_vertices)
                    .map_err(|_| PostgisError::InvalidVertexCount(cumulative_vertices))?;
                if cumulative_vertices > limit {
                    return Err(postgis_items_budget_error(
                        collection,
                        pk_value.to_string(),
                        cumulative_vertices,
                        limit,
                    ));
                }
            }
            let feature: serde_json::Value = row.try_get(1).map_err(PostgisError::from)?;
            features.push(feature);
            last_pk = Some(pk_value.to_string());
        }
        let next_token = has_more.then_some(last_pk).flatten();

        let number_matched = match plan.count {
            // `#1`: the router already resolved this estimate onto the
            // collection decl (`CollectionDecl::row_estimate`, refreshed on
            // its own `descriptor_ttl` cadence) — using it directly needs no
            // additional pool checkout or round trip at all.
            CountPlan::Cached(estimate) => Some(estimate),
            CountPlan::Query(count_sql, count_params) => {
                let count_client = self.pool.get().await.map_err(PostgisError::from)?;
                let boxed = boxed_params(&count_params);
                let row_opt = run_cancellable(count_client, move |client| async move {
                    let refs = param_refs(&boxed);
                    client.query_opt(&count_sql, &refs).await
                })
                .await?;
                row_opt
                    .and_then(|row| row.try_get::<_, i64>(0).ok())
                    .and_then(|v| u64::try_from(v).ok())
            }
            CountPlan::None => None,
        };

        Ok(FeaturePage {
            features_geojson: features,
            number_matched,
            next_token,
        })
    }

    async fn item_inner(
        &self,
        collection: &CollectionDecl,
        id: &str,
        filter: Option<&Filter>,
        requested_crs: RequestedCrs,
    ) -> Result<Option<serde_json::Value>> {
        // `#87`: parsed per this collection's own declared `id_type` — never
        // integer-parse-then-uuid-fallback. An `id` that doesn't fit the
        // declared type can never match, the same way an out-of-range
        // integer id can never match an `Integer` collection (byte-for-byte
        // the pre-`#87` behavior for every `Integer` collection).
        let Some(pk_value) = PkValue::parse(collection.id_type, id) else {
            return Ok(None);
        };

        let (sql, params) = build_item_plan(collection, pk_value, filter, requested_crs)?;
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let boxed = boxed_params(&params);
        let row_opt = run_cancellable(client, move |client| async move {
            let refs = param_refs(&boxed);
            client.query_opt(&sql, &refs).await
        })
        .await?;

        match row_opt {
            Some(row) => {
                if let Some(limit) = collection.settings.items_vertex_budget {
                    let cumulative_vertices: i64 = row.try_get(1).map_err(PostgisError::from)?;
                    let cumulative_vertices = u64::try_from(cumulative_vertices)
                        .map_err(|_| PostgisError::InvalidVertexCount(cumulative_vertices))?;
                    if cumulative_vertices > limit {
                        let feature_id: String = row.try_get(2).map_err(PostgisError::from)?;
                        return Err(postgis_items_budget_error(
                            collection,
                            feature_id,
                            cumulative_vertices,
                            limit,
                        ));
                    }
                }
                Ok(Some(row.try_get(0).map_err(PostgisError::from)?))
            }
            None => Ok(None),
        }
    }

    /// `#19`: `pg_class.reltuples`, the same cheap-estimate approach
    /// `sql.rs`'s unfiltered items count uses. `query_opt` rather than
    /// `query_one`: `to_regclass($1)` returning `NULL` (table genuinely
    /// absent from `search_path`) makes the `WHERE oid = NULL` join match
    /// zero rows rather than one row of `NULL`s — shouldn't happen for a
    /// `physical.name` that came from this driver's own `collections()`, but
    /// there's no reason to panic if it ever does.
    async fn row_estimate_inner(&self, physical: &PhysicalCollection) -> Result<Option<u64>> {
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let table = physical.name.clone();
        let row_opt = run_cancellable(client, move |client| async move {
            client.query_opt(ROW_ESTIMATE_SQL, &[&table]).await
        })
        .await?;
        match row_opt {
            Some(row) => {
                let estimate: i64 = row.try_get("estimate").map_err(PostgisError::from)?;
                Ok(u64::try_from(estimate).ok())
            }
            None => Ok(None),
        }
    }

    /// `#19`: every non-geometry column's name and broad type. Always
    /// answers (`Some`, possibly empty) — unlike `extent`/`row_estimate`,
    /// `information_schema.columns` has no "no statistics yet" failure mode
    /// to fall back from.
    async fn attribute_schema_inner(
        &self,
        physical: &PhysicalCollection,
    ) -> Result<Option<Vec<AttributeColumn>>> {
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let table = physical.name.clone();
        let geometry_column = physical.geometry_column.clone();
        let rows = run_cancellable(client, move |client| async move {
            match &geometry_column {
                Some(geom) => client.query(ATTRIBUTE_SCHEMA_SQL, &[&table, geom]).await,
                None => {
                    client
                        .query(ATTRIBUTE_SCHEMA_SQL_NO_GEOMETRY, &[&table])
                        .await
                }
            }
        })
        .await?;

        let mut columns = Vec::with_capacity(rows.len());
        for row in &rows {
            let name: String = row.try_get("column_name").map_err(PostgisError::from)?;
            let sql_type: String = row.try_get("data_type").map_err(PostgisError::from)?;
            columns.push(AttributeColumn { name, sql_type });
        }
        Ok(Some(columns))
    }

    /// `#19`: a single timestamp/timestamptz/date column derives as this
    /// collection's datetime column; anything but exactly one candidate
    /// (zero, or several) is deliberately treated as "no answer" rather than
    /// guessing — see `CatalogSource::temporal_column`'s doc comment.
    async fn temporal_column_inner(&self, physical: &PhysicalCollection) -> Result<Option<String>> {
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let table = physical.name.clone();
        let rows = run_cancellable(client, move |client| async move {
            client.query(TEMPORAL_COLUMN_SQL, &[&table]).await
        })
        .await?;

        if rows.len() != 1 {
            return Ok(None);
        }
        let name: String = rows[0].try_get("column_name").map_err(PostgisError::from)?;
        Ok(Some(name))
    }

    /// `#101`: a sampled per-collection geometry statistics profile via
    /// `TABLESAMPLE SYSTEM` plus `ST_NPoints`/`ST_Area`/`ST_Length` (design
    /// points 1-2 of the issue) — see `sql::build_geometry_profile_plan`'s
    /// own doc for the query shape and `sql::sample_percentage` for how the
    /// sample size is derived from `row_estimate_inner`. Declines
    /// (`Ok(None)`) when the physical row carries no geometry column to
    /// sample at all, or when the sample itself comes back empty (a
    /// genuinely empty table, or an unlucky block-sampling draw on a small,
    /// never-`ANALYZE`d one — `sample_percentage`'s own doc explains why
    /// that outcome is accepted as a self-describing "no profile" rather
    /// than retried with a wider sample).
    async fn geometry_profile_inner(
        &self,
        physical: &PhysicalCollection,
    ) -> Result<Option<GeometryProfile>> {
        let Some(geometry_column) = physical.geometry_column.as_deref() else {
            return Ok(None);
        };

        let row_estimate = self.row_estimate_inner(physical).await?;
        let sample_pct = sample_percentage(row_estimate);
        let (sql, params) = build_geometry_profile_plan(
            &physical.name,
            geometry_column,
            physical.geometry_type.as_deref(),
            sample_pct,
        )?;

        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let boxed = boxed_params(&params);
        let row = run_cancellable(client, move |client| async move {
            let refs = param_refs(&boxed);
            client.query_one(&sql, &refs).await
        })
        .await?;

        let sample_size: i64 = row.try_get("sample_size").map_err(PostgisError::from)?;
        let Some(sample_size) = u64::try_from(sample_size).ok().filter(|&n| n > 0) else {
            // A sample of zero rows describes nothing — reported as no
            // profile at all rather than a profile of zeroes/NULLs.
            return Ok(None);
        };

        let vertex_mean: Option<f64> = row.try_get("vertex_mean").map_err(PostgisError::from)?;
        let vertex_median: Option<f64> =
            row.try_get("vertex_median").map_err(PostgisError::from)?;
        let vertex_p95: Option<f64> = row.try_get("vertex_p95").map_err(PostgisError::from)?;
        let vertex_max: Option<i64> = row.try_get("vertex_max").map_err(PostgisError::from)?;
        let vertex_sum: Option<i64> = row.try_get("vertex_sum").map_err(PostgisError::from)?;
        let multi_part_fraction: Option<f64> = row
            .try_get("multi_part_fraction")
            .map_err(PostgisError::from)?;
        let mean_ring_count: Option<f64> =
            row.try_get("mean_ring_count").map_err(PostgisError::from)?;
        let size_p50: Option<f64> = row.try_get("size_p50").map_err(PostgisError::from)?;
        let size_p95: Option<f64> = row.try_get("size_p95").map_err(PostgisError::from)?;
        let size_max: Option<f64> = row.try_get("size_max").map_err(PostgisError::from)?;
        let sample_bbox_area: Option<f64> = row
            .try_get("sample_bbox_area")
            .map_err(PostgisError::from)?;

        // Extrapolated from the sample mean times the collection's own row
        // estimate — an estimate, never exact, which is exactly why
        // `sample_size` travels alongside it on `GeometryProfile`.
        let total_estimated = match (vertex_mean, row_estimate) {
            (Some(mean), Some(rows)) => Some((mean * rows as f64).round() as u64),
            _ => None,
        };

        let vertex_density_per_area = match (vertex_sum, sample_bbox_area) {
            (Some(sum), Some(area)) if area > 0.0 => Some(sum as f64 / area),
            _ => None,
        };

        Ok(Some(GeometryProfile {
            sample_size,
            computed_at: SystemTime::now(),
            vertices: VertexStats {
                mean: vertex_mean.unwrap_or(0.0),
                median: vertex_median.unwrap_or(0.0),
                p95: vertex_p95.unwrap_or(0.0),
                max: vertex_max.and_then(|v| u64::try_from(v).ok()).unwrap_or(0),
                total_estimated,
            },
            vertex_density_per_area,
            multi_part_fraction: multi_part_fraction.unwrap_or(0.0),
            mean_ring_count,
            feature_size: FeatureSizeStats {
                p50: size_p50,
                p95: size_p95,
                max: size_max,
            },
        }))
    }

    /// Runs `build_mvt_vertex_total_plan` and returns its single-integer
    /// vertex total as `u64` (`#90`) — the shared probe query
    /// `mvt_tile_inner` issues both for its normal pre-flight check and,
    /// when a retry is warranted, its `#102` raised-tolerance retry; the two
    /// calls differ only in which `tolerance` they probe at.
    #[allow(clippy::too_many_arguments)]
    async fn probe_vertex_total(
        &self,
        collection: &CollectionDecl,
        tms: TileMatrixSet,
        coord: TileCoord,
        tolerance: f64,
        buffer: u32,
        cap: u64,
        filter: Option<&Filter>,
    ) -> Result<u64> {
        let (probe_sql, probe_params) =
            build_mvt_vertex_total_plan(collection, tms, coord, tolerance, buffer, cap, filter)?;
        let probe_client = self.pool.get().await.map_err(PostgisError::from)?;
        let boxed_probe = boxed_params(&probe_params);
        let total_vertices: i64 = run_cancellable(probe_client, move |client| async move {
            let refs = param_refs(&boxed_probe);
            client.query_one(&probe_sql, &refs).await
        })
        .await
        .and_then(|row| row.try_get(0).map_err(PostgisError::from))?;
        Ok(u64::try_from(total_vertices).unwrap_or(u64::MAX))
    }

    /// `#90`/`#102`: composes a per-tile vertex budget on top of the
    /// existing per-zoom feature cap and simplification tolerance, the
    /// latter itself now a function of `collection.geometry_profile` when
    /// one is attached (`descriptor::heuristics::
    /// simplify_tolerance_meters_for_profile`, `#101`/`#102` — `Router::
    /// resolve_tiles`/`resolve_maps` are the only callers that ever attach
    /// one, see `Router::effective_tile_decl`'s own doc). A cheap pre-flight
    /// probe (`build_mvt_vertex_total_plan`) sums `ST_NPoints` over the same
    /// candidate rows `build_mvt_plan` would encode — no geometry crosses
    /// back into Rust for this, just one integer.
    ///
    /// An under-budget probe always takes the untouched `build_mvt_plan`
    /// path at the normal tolerance (`TileSimplificationPath::Normal`), so
    /// its wire bytes stay byte-for-byte what they were before `#90`/`#102`
    /// — true unconditionally for a collection with no profile, since
    /// `simplify_tolerance_meters_for_profile(_, None)` returns exactly
    /// `simplify_tolerance_meters`'s own value.
    ///
    /// An over-budget probe retries once at
    /// `tolerance * VERTEX_BUDGET_RETRY_TOLERANCE_FACTOR`
    /// (`tile_budget::decide_tile_path`'s own doc has the exact bound), but
    /// only when `collection.geometry_profile` is `Some` — a profile-less
    /// collection has no density signal to justify believing a raised
    /// tolerance is safe, so it skips straight to truncation, unchanged
    /// from before `#102`. When the retry probe comes back under budget the
    /// tile serves the raised-tolerance geometry
    /// (`TileSimplificationPath::Adapted`); otherwise (no retry attempted,
    /// or the retry probe is still over budget) this falls back to the
    /// truncating `build_mvt_budgeted_plan`
    /// (`TileSimplificationPath::TruncatedAfterAdapt`), at the raised
    /// tolerance when a retry ran (less needs to be dropped) or the normal
    /// one otherwise — see that function's own doc for why the truncating
    /// path's `ORDER BY` never runs on the common, under-budget path.
    async fn mvt_tile_inner(
        &self,
        collection: &CollectionDecl,
        tms: TileMatrixSet,
        coord: TileCoord,
        filter: Option<&Filter>,
    ) -> Result<Option<Bytes>> {
        // `#190`: the per-zoom heuristics (feature cap, simplification
        // ladder) are calibrated against WebMercatorQuad ground resolution.
        // A WorldCRS84Quad level `z` tile spans HALF the angular extent of
        // a mercator level-`z` tile (two root tiles instead of one), so its
        // equator ground resolution matches mercator level `z + 1` — that
        // is the ladder rung its cap and tolerance are read from, keeping a
        // CRS84 tile's density budget aligned with the mercator tile that
        // covers the same ground. Mercator requests keep `z` untouched.
        let heuristics_zoom = match tms {
            TileMatrixSet::WebMercatorQuad => coord.z,
            TileMatrixSet::WorldCrs84Quad => coord.z.saturating_add(1),
        };
        let cap = heuristics::effective_feature_cap(
            &collection.tiles.caps,
            heuristics_zoom,
            collection.row_estimate,
        );
        let tolerance_meters = heuristics::simplify_tolerance_meters_for_profile(
            heuristics_zoom,
            collection.geometry_profile,
        );
        // `#190`: `sql::build_mvt_candidate_fragment` simplifies in the
        // grid's own CRS units — mercator meters as-is, or CRS84 degrees
        // via the equatorial meters-per-degree the WorldCRS84Quad scale
        // ladder itself is defined with (OGC 17-083r4 SS5.2.1).
        let tolerance = match tms {
            TileMatrixSet::WebMercatorQuad => tolerance_meters,
            TileMatrixSet::WorldCrs84Quad => tolerance_meters / WORLD_CRS84_METERS_PER_DEGREE,
        };
        let buffer = heuristics::tile_buffer_px(MVT_EXTENT);
        let vertex_budget = collection
            .settings
            .tile_vertex_budget
            .unwrap_or(DEFAULT_TILE_VERTEX_BUDGET);

        let total_vertices = self
            .probe_vertex_total(collection, tms, coord, tolerance, buffer, cap, filter)
            .await?;

        // `#102`: the raised-tolerance retry only ever runs when a geometry
        // profile informed `tolerance` above — see `mvt_tile_inner`'s own
        // doc for why a profile-less collection skips straight to
        // truncation instead, keeping its behavior byte-for-byte unchanged.
        let mut raised_tolerance = None;
        let mut retry_total_vertices = None;
        if total_vertices > vertex_budget && collection.geometry_profile.is_some() {
            let raised = tolerance * VERTEX_BUDGET_RETRY_TOLERANCE_FACTOR;
            let retry_total = self
                .probe_vertex_total(collection, tms, coord, raised, buffer, cap, filter)
                .await?;
            raised_tolerance = Some(raised);
            retry_total_vertices = Some(retry_total);
        }

        let path = decide_tile_path(total_vertices, vertex_budget, retry_total_vertices);

        let (sql, params) = match path {
            TileSimplificationPath::Normal => {
                build_mvt_plan(collection, tms, coord, tolerance, buffer, cap, filter)?
            }
            TileSimplificationPath::Adapted => {
                metrics::counter!("tile_vertex_budget_adapted_total", "backend" => "postgis")
                    .increment(1);
                // `raised_tolerance`/`retry_total_vertices` are always
                // `Some` here: `decide_tile_path` only returns `Adapted`
                // when it was passed `Some(retry_total)`, and that only
                // happens on the branch above that also sets
                // `raised_tolerance`.
                let adapted_tolerance = raised_tolerance
                    .expect("Adapted implies the retry probe ran and set raised_tolerance");
                tracing::debug!(
                    collection = %collection.id,
                    z = coord.z,
                    x = coord.x,
                    y = coord.y,
                    vertex_budget,
                    total_vertices,
                    adapted_tolerance,
                    retry_total_vertices = retry_total_vertices
                        .expect("Adapted implies the retry probe ran and set retry_total_vertices"),
                    "tile exceeded its vertex budget at the normal tolerance; served at a \
                     raised tolerance instead of truncating"
                );
                build_mvt_plan(
                    collection,
                    tms,
                    coord,
                    adapted_tolerance,
                    buffer,
                    cap,
                    filter,
                )?
            }
            TileSimplificationPath::TruncatedAfterAdapt => {
                metrics::counter!("tile_vertex_budget_exceeded_total", "backend" => "postgis")
                    .increment(1);
                let effective_tolerance = raised_tolerance.unwrap_or(tolerance);
                tracing::warn!(
                    collection = %collection.id,
                    z = coord.z,
                    x = coord.x,
                    y = coord.y,
                    vertex_budget,
                    total_vertices,
                    retried = raised_tolerance.is_some(),
                    "tile exceeded its vertex budget; dropping the marginal geometry rather than \
                     serving an unbounded encode"
                );
                build_mvt_budgeted_plan(
                    collection,
                    tms,
                    coord,
                    effective_tolerance,
                    buffer,
                    cap,
                    vertex_budget,
                    filter,
                )?
            }
        };

        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let boxed = boxed_params(&params);
        let row_opt = run_cancellable(client, move |client| async move {
            let refs = param_refs(&boxed);
            client.query_opt(&sql, &refs).await
        })
        .await?;

        match row_opt {
            Some(row) => {
                let bytes: Option<Vec<u8>> = row.try_get(0).map_err(PostgisError::from)?;
                Ok(bytes.filter(|b| !b.is_empty()).map(Bytes::from))
            }
            None => Ok(None),
        }
    }

    /// Resolves every incoming property key's [`PropertyType`] (`#25`, `#44`):
    /// a declared schema's own type where `collection.schema` names the key,
    /// else a live `information_schema.columns` lookup (reusing
    /// `attribute_schema_inner`, the same query `#19`'s descriptor
    /// derivation already uses) for anything undeclared — the free-form
    /// collection case. A key found in neither fails with
    /// `UnwritableProperty`: this write path never guesses a cast for a
    /// column it cannot confirm exists.
    async fn resolve_property_types(
        &self,
        collection: &CollectionDecl,
        properties: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<HashMap<String, PropertyType>> {
        let mut types = HashMap::with_capacity(properties.len());
        let mut undeclared: Vec<&str> = Vec::new();

        for key in properties.keys() {
            let declared = collection
                .schema
                .as_ref()
                .and_then(|schema| schema.properties.iter().find(|p| p.name == *key));
            match declared {
                Some(property) => {
                    types.insert(key.clone(), property.type_);
                }
                None => undeclared.push(key.as_str()),
            }
        }

        if !undeclared.is_empty() {
            let physical = PhysicalCollection {
                name: collection.resolved_table().to_string(),
                geometry_column: Some(collection.resolved_geometry().to_string()),
                primary_key: Some(collection.resolved_pk().to_string()),
                srid: None,
                geometry_type: None,
            };
            let attributes = self
                .attribute_schema_inner(&physical)
                .await?
                .unwrap_or_default();
            for key in undeclared {
                let sql_type = attributes
                    .iter()
                    .find(|a| a.name == key)
                    .map(|a| a.sql_type.as_str())
                    .ok_or_else(|| PostgisError::UnwritableProperty(key.to_string()))?;
                types.insert(key.to_string(), PropertyType::from_sql_type(sql_type));
            }
        }

        Ok(types)
    }

    /// The batch lane's counterpart of `resolve_property_types` (`#114`):
    /// resolves whatever it can for the union of every mutation's
    /// properties across a whole chunk, but — unlike `resolve_property_types`
    /// — never fails the WHOLE call over one unwritable key. An unwritable
    /// property must refuse only the individual mutation(s) that actually
    /// reference it, never every OTHER mutation in the same chunk that
    /// never touched it; `build_batch_item_plan`'s own per-property check
    /// (`write_sql::build_upsert_plan`'s `UnwritableProperty` refusal)
    /// already does exactly that, given a `types` map that simply omits an
    /// unresolvable key entirely rather than this method erroring out on it
    /// up front.
    async fn resolve_property_types_lenient(
        &self,
        collection: &CollectionDecl,
        properties: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<HashMap<String, PropertyType>> {
        let mut types = HashMap::with_capacity(properties.len());
        let mut undeclared: Vec<&str> = Vec::new();

        for key in properties.keys() {
            let declared = collection
                .schema
                .as_ref()
                .and_then(|schema| schema.properties.iter().find(|p| p.name == *key));
            match declared {
                Some(property) => {
                    types.insert(key.clone(), property.type_);
                }
                None => undeclared.push(key.as_str()),
            }
        }

        if !undeclared.is_empty() {
            let physical = PhysicalCollection {
                name: collection.resolved_table().to_string(),
                geometry_column: Some(collection.resolved_geometry().to_string()),
                primary_key: Some(collection.resolved_pk().to_string()),
                srid: None,
                geometry_type: None,
            };
            let attributes = self
                .attribute_schema_inner(&physical)
                .await?
                .unwrap_or_default();
            for key in undeclared {
                if let Some(sql_type) = attributes
                    .iter()
                    .find(|a| a.name == key)
                    .map(|a| a.sql_type.as_str())
                {
                    types.insert(key.to_string(), PropertyType::from_sql_type(sql_type));
                }
                // No matching column: leave `key` unresolved rather than
                // refusing here — a mutation that references it refuses on
                // its own, by name, when `build_batch_item_plan` builds
                // that mutation's own upsert plan.
            }
        }

        Ok(types)
    }

    /// `WriteSink::apply` (`#25`): commits the data mutation and the outbox
    /// obligation in one transaction — see `run_write_transaction` for the
    /// atomicity mechanics. Deliberately not routed through `run_cancellable`
    /// the way every read query in this file is: a multi-statement
    /// transaction has nothing sensible for a bare server-side cancel to do
    /// mid-way (the connection would need a full rollback regardless), so
    /// this holds its own pooled connection directly for the transaction's
    /// duration instead.
    ///
    /// `expected` is `WriteSink::apply_conditional`'s `#150` witness. `None`
    /// is the unconditional path, byte-for-byte unchanged, and always
    /// answers `Ok(Some(sequence))`. `Some(version)` compiles the witness
    /// into the data statement's own `WHERE` (`write_sql`'s own doc) and
    /// answers `Ok(None)` when that statement matched no row — somebody else
    /// wrote first, and NOTHING was committed, since the transaction is
    /// dropped without a `commit()`.
    async fn write_apply_inner(
        &self,
        collection: &CollectionDecl,
        mutation: Mutation,
        requested_crs: RequestedCrs,
        expected: Option<&RowVersion>,
    ) -> Result<Option<Sequence>> {
        // `#87`: parsed per this collection's own declared `id_type` — never
        // integer-parse-then-uuid-fallback. Unlike `item_inner`'s read-side
        // `Ok(None)`, a `PUT`/`DELETE` with an id that doesn't fit the
        // declared type is a caller mistake worth naming, not a silent
        // not-found.
        let pk_value = PkValue::parse(collection.id_type, &mutation.feature_id)
            .ok_or_else(|| PostgisError::InvalidFeatureId(mutation.feature_id.clone()))?;

        // `#141`: an upsert's PRIOR extent is only knowable before the row is
        // overwritten, so it is captured inside the same transaction, one
        // statement earlier. A delete needs no such statement — its own
        // `RETURNING` hands back the row it removed (`write_sql::
        // build_delete_plan`'s doc).
        let (statement_sql, statement_params, outbox_payload, prior_plan, returning) =
            match &mutation.kind {
                MutationKind::Upsert(feature) => {
                    let properties = feature
                        .get("properties")
                        .and_then(serde_json::Value::as_object)
                        .cloned()
                        .unwrap_or_default();
                    let geometry = feature.get("geometry");
                    let types = self.resolve_property_types(collection, &properties).await?;
                    let prior = write_sql::build_prior_extent_plan(collection, pk_value.clone())?;
                    let plan = write_sql::build_upsert_plan(
                        collection,
                        pk_value,
                        geometry,
                        &properties,
                        &types,
                        requested_crs,
                        expected,
                    )?;
                    (
                        plan.sql,
                        plan.params,
                        Some(feature.clone()),
                        Some(prior),
                        ReturnedExtent::Current,
                    )
                }
                MutationKind::Delete => {
                    let (sql, params) =
                        write_sql::build_delete_plan(collection, pk_value, expected)?;
                    (sql, params, None, None, ReturnedExtent::Prior)
                }
            };
        let kind_text = match &mutation.kind {
            MutationKind::Upsert(_) => "upsert",
            MutationKind::Delete => "delete",
        };

        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let sequence = run_write_transaction(WriteTransaction {
            client,
            table: collection.resolved_table().to_string(),
            feature_id: mutation.feature_id.clone(),
            kind: kind_text,
            payload: outbox_payload,
            prior_plan: prior_plan.map(|(sql, params)| (sql, boxed_params(&params))),
            returning,
            statement_sql,
            statement_params: boxed_params(&statement_params),
            conditional: expected.is_some(),
        })
        .await?;
        Ok(sequence.map(|sequence| Sequence(sequence as u64)))
    }

    /// `WriteSink::row_version` (`#150`): reads the target row's `xmin` — see
    /// `write_sql::ROW_VERSION_EXPR` for why that is the witness. A table
    /// with no `xmin` at all (a collection whose `table:` names a VIEW rather
    /// than a real relation) refuses by the same capability name the trait
    /// default uses, rather than surfacing a raw SQL error: a view cannot
    /// carry a row version, so this driver genuinely cannot do optimistic
    /// locking for that collection and says so.
    async fn row_version_inner(
        &self,
        collection: &CollectionDecl,
        feature_id: &str,
    ) -> Result<Option<RowVersion>> {
        let Some(pk_value) = PkValue::parse(collection.id_type, feature_id) else {
            // Same "an id that cannot exist has no row" reading `item_inner`
            // gives on the read lane — never a hard failure here, because the
            // caller is about to refuse this request anyway.
            return Ok(None);
        };
        let (sql, params) = write_sql::build_row_version_plan(collection, pk_value)?;
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let boxed = boxed_params(&params);
        let row_opt = run_cancellable(client, move |client| async move {
            let refs = param_refs(&boxed);
            client.query_opt(&sql, &refs).await
        })
        .await
        .map_err(|error| match &error {
            PostgisError::Postgres(query) if query.code() == Some(&SqlState::UNDEFINED_COLUMN) => {
                PostgisError::OptimisticLockingUnsupported(collection.resolved_table().to_string())
            }
            _ => error,
        })?;
        let Some(row) = row_opt else {
            return Ok(None);
        };
        let token: String = row.try_get(0).map_err(PostgisError::from)?;
        Ok(Some(RowVersion::new(token)))
    }

    /// `WriteSink::apply_batch` (`#114`): every mutation in `mutations`
    /// applies inside ONE transaction, each behind its own `SAVEPOINT` — see
    /// `run_batch_transaction` for the per-item commit/rollback mechanics.
    /// Every mutation is an `Upsert` (batch ingest never deletes, see
    /// `WriteSink::apply_batch`'s own doc), so every incoming feature's
    /// property keys are collected into ONE union up front and resolved
    /// through `resolve_property_types` ONCE for the whole chunk — an
    /// undeclared-schema collection would otherwise pay one live catalog
    /// round trip per item instead of one per chunk, which at a few hundred
    /// items per chunk is the difference between a bulk load and a
    /// disguised row-at-a-time one.
    async fn write_apply_batch_inner(
        &self,
        collection: &CollectionDecl,
        mutations: Vec<Mutation>,
        requested_crs: RequestedCrs,
        strict: bool,
    ) -> Result<Vec<BatchItemResult>> {
        let mut union_properties: serde_json::Map<String, serde_json::Value> =
            serde_json::Map::new();
        for mutation in &mutations {
            if let MutationKind::Upsert(feature) = &mutation.kind {
                if let Some(properties) = feature.get("properties").and_then(|v| v.as_object()) {
                    for key in properties.keys() {
                        union_properties
                            .entry(key.clone())
                            .or_insert(serde_json::Value::Null);
                    }
                }
            }
        }
        let types = self
            .resolve_property_types_lenient(collection, &union_properties)
            .await?;

        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let outbox_table = write_sql::outbox_table_name(collection.resolved_table());
        let collection = collection.clone();

        run_batch_transaction(
            client,
            collection,
            outbox_table,
            types,
            requested_crs,
            mutations,
            strict,
        )
        .await
    }

    /// `#87` for `Uuid`, extended to `Text` by `#94`: confirms the pk
    /// column's own physical type actually matches the declared `id_type`
    /// before `create_inner` ever attempts a server-assigned `INSERT` — a
    /// live, per-request catalog lookup (the same `attribute_schema_inner`
    /// query `resolve_property_types` already runs for undeclared
    /// properties), reused here because a mismatch (a collection declaring
    /// `id_type: uuid`/`text` over, say, a `bigint` pk) would otherwise only
    /// surface as an opaque client-side type error when
    /// `run_create_transaction` tries to read the `RETURNING` row back typed,
    /// or as a read/write lane that silently never matches any id. Named and
    /// refused before the row-affecting `INSERT` even runs — the
    /// declaration-validation counterpart of `run_create_transaction`'s own
    /// `PkNotServerAssignable` (a pk column of the RIGHT type but with no
    /// server default), extending that same "refuse the create by name, not
    /// serve a partial mismatch" idiom. A `text` pk column reports as either
    /// `information_schema.columns.data_type` spelling Postgres uses —
    /// `"text"` or `"character varying"` (a declared `varchar(n)` column) —
    /// both accepted. `Integer` never runs this check at all — zero extra
    /// round trips, zero behavior change, for every collection that predates
    /// `#87`.
    async fn validate_id_type_for_create(&self, collection: &CollectionDecl) -> Result<()> {
        let (declared, expected_sql_types): (&'static str, &[&str]) = match collection.id_type {
            IdType::Integer => return Ok(()),
            IdType::Uuid => ("uuid", &["uuid"]),
            IdType::Text => ("text", &["text", "character varying"]),
        };
        let physical = PhysicalCollection {
            name: collection.resolved_table().to_string(),
            geometry_column: Some(collection.resolved_geometry().to_string()),
            primary_key: Some(collection.resolved_pk().to_string()),
            srid: None,
            geometry_type: None,
        };
        let attributes = self
            .attribute_schema_inner(&physical)
            .await?
            .unwrap_or_default();
        let pk = collection.resolved_pk();
        let actual = attributes
            .iter()
            .find(|column| column.name == pk)
            .map(|column| column.sql_type.as_str());
        match actual {
            Some(sql_type) if expected_sql_types.contains(&sql_type) => Ok(()),
            Some(other) => Err(PostgisError::IdTypeMismatch {
                collection: collection.id.clone(),
                pk: pk.to_string(),
                declared,
                actual: other.to_string(),
            }),
            None => Err(PostgisError::IdTypeMismatch {
                collection: collection.id.clone(),
                pk: pk.to_string(),
                declared,
                actual: "column not found".to_string(),
            }),
        }
    }

    /// `WriteSink::create` (`#88`, `id_type: uuid` support added by `#87`,
    /// `id_type: text` by `#94`): a server-assigned INSERT, then the outbox
    /// insert built from the pk, in the SAME transaction — see
    /// `run_create_transaction` for the atomicity mechanics
    /// (`write_apply_inner`'s own doc explains the equivalent for
    /// upsert/delete). `validate_id_type_for_create` runs first for a
    /// `Uuid`/`Text` collection, refusing a pk-type mismatch by name before
    /// the `INSERT` is even built. For `Text`, the pk is CALLER-supplied
    /// (the create-mode inversion `#94` scopes): the feature body's own
    /// top-level `id` becomes a bound `INSERT` column instead of an omitted
    /// one, so a request with no `id` refuses by name (`TextIdRequired`)
    /// before any SQL runs at all — never silently falls through to the
    /// `Integer`/`Uuid` server-minted shape.
    async fn create_inner(
        &self,
        collection: &CollectionDecl,
        feature: serde_json::Value,
        requested_crs: RequestedCrs,
    ) -> Result<(String, Sequence)> {
        self.validate_id_type_for_create(collection).await?;

        let caller_pk = match collection.id_type {
            IdType::Text => {
                let id = feature
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| PostgisError::TextIdRequired(collection.id.clone()))?;
                Some(PkValue::Text(id.to_string()))
            }
            IdType::Integer | IdType::Uuid => None,
        };
        // Captured before `caller_pk` moves into `build_insert_plan` below —
        // `run_create_transaction` needs this purely to name the conflicting
        // id in a `PkConflict` refusal; `None` for `Integer`/`Uuid` also
        // tells it that a `UNIQUE` violation on this INSERT is never a
        // caller-supplied-pk conflict for those (their pk column is always
        // omitted from the INSERT), so their own error handling stays
        // byte-for-byte unchanged.
        let caller_pk_id = caller_pk.as_ref().map(PkValue::to_string);

        let properties = feature
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();
        let geometry = feature.get("geometry");
        let types = self.resolve_property_types(collection, &properties).await?;
        let plan = write_sql::build_insert_plan(
            collection,
            caller_pk,
            geometry,
            &properties,
            &types,
            requested_crs,
        )?;

        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let table = collection.resolved_table().to_string();
        let pk_column = collection.resolved_pk().to_string();
        let outbox_table = write_sql::outbox_table_name(&table);

        let (pk_value, sequence) = run_create_transaction(
            client,
            outbox_table,
            pk_column,
            collection.id_type,
            plan.sql,
            boxed_params(&plan.params),
            table,
            feature,
            caller_pk_id,
        )
        .await?;
        Ok((pk_value.to_string(), Sequence(sequence as u64)))
    }

    async fn read_after_inner(
        &self,
        collection: &CollectionDecl,
        after: Sequence,
        limit: u32,
    ) -> Result<Vec<Obligation>> {
        let table = collection.resolved_table();
        let (sql, params) = write_sql::build_read_after_plan(table, after.0, limit)?;
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let boxed = boxed_params(&params);
        let rows = run_cancellable(client, move |client| async move {
            let refs = param_refs(&boxed);
            client.query(&sql, &refs).await
        })
        .await
        .map_err(|error| map_outbox_missing(error, table))?;

        let mut obligations = Vec::with_capacity(rows.len());
        for row in &rows {
            let sequence: i64 = row.try_get(0).map_err(PostgisError::from)?;
            let feature_id: String = row.try_get(1).map_err(PostgisError::from)?;
            let kind: String = row.try_get(2).map_err(PostgisError::from)?;
            let payload: Option<serde_json::Value> = row.try_get(3).map_err(PostgisError::from)?;
            // `timestamptz` read back straight into `SystemTime` —
            // `postgres-types`' own built-in conversion, no `chrono`/`time`
            // dependency needed (`#115`).
            let committed_at: std::time::SystemTime = row.try_get(4).map_err(PostgisError::from)?;
            // `#141`/`#142`: `NULL` here is an outbox row written before the
            // column existed — `ObligationExtent::Unrecorded`, which the
            // invalidation consumer reads as UNKNOWN and degrades
            // conservatively on, never as "nothing moved".
            let extent: Option<serde_json::Value> = row.try_get(5).map_err(PostgisError::from)?;
            let sequence = Sequence(sequence as u64);
            let kind = match kind.as_str() {
                "delete" => MutationKind::Delete,
                _ => MutationKind::Upsert(payload.unwrap_or(serde_json::Value::Null)),
            };
            obligations.push(Obligation {
                sequence,
                feature_id,
                kind,
                // First slice (`#25`, design doc section 4): version IS the
                // committing sequence.
                version: sequence,
                committed_at,
                extent: write_sql::decode_extent(extent.as_ref()),
            });
        }
        Ok(obligations)
    }

    /// `OutboxSource::prune_before` (`#115`): one bounded batch delete via
    /// `write_sql::build_prune_before_plan`, returning how many rows this
    /// call actually removed.
    async fn prune_before_inner(
        &self,
        collection: &CollectionDecl,
        floor: Sequence,
        batch_size: u32,
    ) -> Result<u64> {
        let table = collection.resolved_table();
        let (sql, params) = write_sql::build_prune_before_plan(table, floor.0, batch_size)?;
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let boxed = boxed_params(&params);
        let removed = run_cancellable(client, move |client| async move {
            let refs = param_refs(&boxed);
            client.execute(&sql, &refs).await
        })
        .await
        .map_err(|error| map_outbox_missing(error, table))?;
        Ok(removed)
    }

    async fn primary_high_water_inner(&self, collection: &CollectionDecl) -> Result<Sequence> {
        let table = collection.resolved_table();
        let sql = write_sql::build_primary_high_water_plan(table)?;
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let row = run_cancellable(client, move |client| async move {
            client.query_one(&sql, &[]).await
        })
        .await
        .map_err(|error| map_outbox_missing(error, table))?;
        let high_water: i64 = row.try_get(0).map_err(PostgisError::from)?;
        Ok(Sequence(high_water as u64))
    }

    /// `IndexSink::apply` (`#67`): a single version-guarded upsert against
    /// `"<table>_index"` — see `index_sql::build_apply_plan`'s own doc for
    /// the idempotency mechanics. One statement, so (unlike
    /// `write_apply_inner`'s two-statement transaction) this runs through
    /// `run_cancellable` like every other single-statement query in this
    /// file.
    async fn index_apply_inner(
        &self,
        collection: &CollectionDecl,
        obligation: &Obligation,
    ) -> Result<()> {
        let table = collection.resolved_table();
        let index_sql::ApplyPlan { sql, params } = index_sql::build_apply_plan(table, obligation)?;
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let boxed = boxed_params(&params);
        run_cancellable(client, move |client| async move {
            let refs = param_refs(&boxed);
            client.execute(&sql, &refs).await
        })
        .await
        .map_err(|error| map_index_missing(error, table))?;
        Ok(())
    }

    async fn index_applied_high_water_inner(
        &self,
        collection: &CollectionDecl,
    ) -> Result<Sequence> {
        let table = collection.resolved_table();
        let sql = index_sql::build_high_water_plan(table)?;
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let row = run_cancellable(client, move |client| async move {
            client.query_one(&sql, &[]).await
        })
        .await
        .map_err(|error| map_index_missing(error, table))?;
        let high_water: i64 = row.try_get(0).map_err(PostgisError::from)?;
        Ok(Sequence(high_water as u64))
    }

    /// `SearchSource::search` (`#67`, free text `#181`): every
    /// non-tombstoned document in `"<table>_index"`, up to `query.limit`,
    /// narrowed by `query.q`'s `tsvector` predicate when present — see
    /// `index_sql::build_search_plan`'s own doc for exactly what this does
    /// and does not support yet. Same missing-table refusal as every other
    /// index operation (`map_index_missing`); a `q` against an index table
    /// provisioned before `#181` (no `search_text` column yet) additionally
    /// maps its undefined-column error to the named `SearchColumnMissing`
    /// refusal — rerunning `tellurion-ingest index create-tables` upgrades
    /// such a table in place, and the server never does DDL itself.
    async fn search_inner(
        &self,
        collection: &CollectionDecl,
        query: &SearchQuery,
    ) -> Result<SearchPage> {
        let table = collection.resolved_table();
        let (sql, params) = index_sql::build_search_plan(table, query.limit, query.q.as_deref())?;
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let boxed = boxed_params(&params);
        let with_q = query.q.is_some();
        let rows = run_cancellable(client, move |client| async move {
            let refs = param_refs(&boxed);
            client.query(&sql, &refs).await
        })
        .await
        .map_err(|error| map_index_missing(error, table))
        .map_err(|error| {
            if with_q {
                map_search_column_missing(error, table)
            } else {
                error
            }
        })?;

        let mut features_geojson = Vec::with_capacity(rows.len());
        for row in &rows {
            let doc: serde_json::Value = row.try_get(0).map_err(PostgisError::from)?;
            features_geojson.push(doc);
        }
        Ok(SearchPage { features_geojson })
    }

    /// `StacMetadataSource::stac_metadata` (`#202`): one batched
    /// `feature_id = ANY($1)` read of this collection's `"<table>_stac"`
    /// sidecar for a whole page of ids — see `stac_sql`'s own doc for why
    /// an array bind rather than a generated `IN` list. An empty page short
    /// -circuits before the pool is even touched, per the trait's own
    /// contract. An undefined relation becomes the named
    /// `StacTableMissing`, the same treatment `map_index_missing` gives the
    /// derived-index table; a `doc` that is not a JSON object becomes the
    /// named `MalformedStacRow`, since there is no member set to merge from
    /// a scalar and silently dropping it would hide a broken populator.
    async fn stac_metadata_inner(
        &self,
        collection: &CollectionDecl,
        feature_ids: &[String],
    ) -> Result<HashMap<String, serde_json::Value>> {
        if feature_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let table = collection.resolved_table();
        let (sql, params) = stac_sql::build_lookup_plan(table, feature_ids)?;
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let boxed = boxed_params(&params);
        let rows = run_cancellable(client, move |client| async move {
            let refs = param_refs(&boxed);
            client.query(&sql, &refs).await
        })
        .await
        .map_err(|error| map_stac_missing(error, table))?;

        let mut docs = HashMap::with_capacity(rows.len());
        for row in &rows {
            let feature_id: String = row.try_get(0).map_err(PostgisError::from)?;
            let doc: serde_json::Value = row.try_get(1).map_err(PostgisError::from)?;
            if !doc.is_object() {
                return Err(PostgisError::MalformedStacRow(feature_id));
            }
            docs.insert(feature_id, doc);
        }
        Ok(docs)
    }

    /// `#41` part 1: fails fast, by name, when `collection`'s geometry
    /// column isn't one of the supported 3D solid types — a named
    /// capability/validation error rather than a confusing per-row EWKB
    /// decode failure. Scoped to one table+column (an indexed
    /// `geometry_columns` lookup), run fresh on every `volume_tile_inner`
    /// call rather than cached: unlike `Router`'s own `CollectionDescriptor`
    /// cache, nothing upstream of `VolumeSource::volume_tile` threads a
    /// descriptor through to this driver to cache alongside, and this
    /// query is cheap enough (a single indexed row read) that re-running it
    /// per request costs far less than the query that follows it.
    async fn volume_geometry_kind(
        &self,
        collection: &CollectionDecl,
    ) -> Result<VolumeGeometryKind> {
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let table = collection.resolved_table().to_string();
        let geometry = collection.resolved_geometry().to_string();
        let row_opt = run_cancellable(client, move |client| async move {
            client
                .query_opt(VOLUME_GEOMETRY_KIND_SQL, &[&table, &geometry])
                .await
        })
        .await?;

        let Some(row) = row_opt else {
            return Err(PostgisError::UnsupportedVolumeGeometryType {
                collection: collection.id.clone(),
                found: "not registered in geometry_columns".to_string(),
            });
        };
        let type_name: String = row.try_get("type").map_err(PostgisError::from)?;
        let coord_dimension: i32 = row.try_get("coord_dimension").map_err(PostgisError::from)?;

        VolumeGeometryKind::from_catalog(&type_name, coord_dimension).ok_or_else(|| {
            PostgisError::UnsupportedVolumeGeometryType {
                collection: collection.id.clone(),
                found: format!("{type_name} (coord_dimension {coord_dimension})"),
            }
        })
    }

    /// `#41`: the `VolumeSource` lane end to end — geometry-type check,
    /// reprojected EWKB fetch (bounded by the same per-zoom row cap
    /// `mvt_tile_inner` uses), EWKB decode, per-solid vertex-budget cap,
    /// face triangulation, and the world-to-tile-local transform. A dropped
    /// solid or degenerate face never fails the tile — only logged, once,
    /// with its counts.
    async fn volume_tile_inner(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
        filter: Option<&Filter>,
    ) -> Result<Option<VolumeMesh>> {
        // Fails fast, by name, before ever touching the row-fetch query
        // below — see `volume_geometry_kind`'s own doc comment. The
        // concrete kind isn't threaded any further: `ewkb::decode_solid`
        // re-derives the actual wire type from each row's own header, so
        // this check exists purely to turn "wrong table for this lane" into
        // a clear error instead of a silent, ever-empty tile.
        self.volume_geometry_kind(collection).await?;

        let cap = heuristics::effective_feature_cap(
            &collection.tiles.caps,
            coord.z,
            collection.row_estimate,
        );
        let vertex_cap = effective_volume_vertex_cap(collection, coord.z);

        let (sql, params) = build_volume_plan(collection, coord, cap, filter)?;
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let boxed = boxed_params(&params);
        let rows = run_cancellable(client, move |client| async move {
            let refs = param_refs(&boxed);
            client.query(&sql, &refs).await
        })
        .await?;

        if rows.is_empty() {
            return Ok(None);
        }

        let mut solids = Vec::with_capacity(rows.len());
        for row in &rows {
            let bytes: Vec<u8> = row.try_get(0).map_err(PostgisError::from)?;
            let solid = decode_solid(&bytes).map_err(|source| match source {
                crate::ewkb::EwkbError::UnexpectedEof => {
                    PostgisError::MalformedEwkb(collection.id.clone())
                }
                crate::ewkb::EwkbError::UnsupportedGeometryType(code) => {
                    PostgisError::UnsupportedEwkbGeometryType(collection.id.clone(), code)
                }
            })?;
            solids.push(solid);
        }

        let transform = TileTransform::for_coord(coord);
        let (mesh, stats) = build_volume_mesh(solids, &transform, vertex_cap);

        if stats.any_dropped() {
            tracing::warn!(
                collection = %collection.id,
                z = coord.z,
                x = coord.x,
                y = coord.y,
                solids_over_budget = stats.solids_over_budget,
                faces_skipped_degenerate = stats.faces_skipped_degenerate,
                "volume tile dropped some geometry rather than failing the whole tile"
            );
        }

        if mesh.positions.is_empty() {
            Ok(None)
        } else {
            Ok(Some(mesh))
        }
    }

    /// `AssetRecordStore::register` (assets-and-object-storage proposal,
    /// first slice): a single `INSERT`, atomic by construction (see
    /// `asset_sql`'s own doc for why registration never needs a
    /// multi-statement transaction the way `write_apply_inner` does). A
    /// `UNIQUE (item_id, asset_key)` violation becomes the named
    /// `AssetKeyConflict`; an undefined-relation error becomes the named
    /// `AssetsTableMissing` — the identical two-error-name treatment
    /// `run_write_transaction`/`map_outbox_missing` give the outbox table.
    async fn assets_register_inner(
        &self,
        collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
        new_record: NewAssetRecord,
    ) -> Result<AssetRecord> {
        let table = collection.resolved_table();
        let plan = asset_sql::build_register_plan(table, item_id, key, new_record.id, &new_record)?;
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let boxed = boxed_params(&plan.params);
        let row = run_cancellable(client, move |client| async move {
            let refs = param_refs(&boxed);
            client.query_one(&plan.sql, &refs).await
        })
        .await
        .map_err(|error| map_assets_error(error, table, key))?;
        asset_sql::row_to_asset_record(&row)
    }

    async fn assets_get_inner(
        &self,
        collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
    ) -> Result<Option<AssetRecord>> {
        let table = collection.resolved_table();
        let plan = asset_sql::build_get_plan(table, item_id, key)?;
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let boxed = boxed_params(&plan.params);
        let row_opt = run_cancellable(client, move |client| async move {
            let refs = param_refs(&boxed);
            client.query_opt(&plan.sql, &refs).await
        })
        .await
        .map_err(|error| map_assets_error(error, table, key))?;
        row_opt
            .as_ref()
            .map(asset_sql::row_to_asset_record)
            .transpose()
    }

    async fn assets_finalize_inner(
        &self,
        collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
        outcome: FinalizeOutcome,
    ) -> Result<AssetRecord> {
        let table = collection.resolved_table();
        let plan = asset_sql::build_finalize_plan(table, item_id, key, &outcome)?;
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let boxed = boxed_params(&plan.params);
        let row_opt = run_cancellable(client, move |client| async move {
            let refs = param_refs(&boxed);
            client.query_opt(&plan.sql, &refs).await
        })
        .await
        .map_err(|error| map_assets_error(error, table, key))?;
        let row = row_opt.ok_or(PostgisError::AssetNotFound)?;
        asset_sql::row_to_asset_record(&row)
    }

    async fn assets_delete_inner(
        &self,
        collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
    ) -> Result<Option<AssetRecord>> {
        let table = collection.resolved_table();
        let plan = asset_sql::build_delete_plan(table, item_id, key)?;
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let boxed = boxed_params(&plan.params);
        let row_opt = run_cancellable(client, move |client| async move {
            let refs = param_refs(&boxed);
            client.query_opt(&plan.sql, &refs).await
        })
        .await
        .map_err(|error| map_assets_error(error, table, key))?;
        row_opt
            .as_ref()
            .map(asset_sql::row_to_asset_record)
            .transpose()
    }

    /// `AssetRecordStore::list` (reconcile surface, `#93`): every row in
    /// this collection's own assets table — see `asset_sql::build_list_plan`'s
    /// own doc for why this is the one asset query with no `WHERE` clause.
    /// An undefined-relation error still becomes the named
    /// `AssetsTableMissing`, the same treatment every other assets query
    /// gets — `map_assets_error`'s own `key` parameter is a placeholder
    /// here (`""`), never surfaced: `AssetsTableMissing`'s own message
    /// never mentions it.
    async fn assets_list_inner(
        &self,
        collection: &CollectionDecl,
    ) -> Result<Vec<tellurion_core::AssetRecordEntry>> {
        let table = collection.resolved_table();
        let plan = asset_sql::build_list_plan(table)?;
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let boxed = boxed_params(&plan.params);
        let rows = run_cancellable(client, move |client| async move {
            let refs = param_refs(&boxed);
            client.query(&plan.sql, &refs).await
        })
        .await
        .map_err(|error| map_assets_error(error, table, ""))?;
        rows.iter()
            .map(asset_sql::row_to_asset_record_entry)
            .collect()
    }

    /// `AssetRecordStore::item_assets` (`#221`): one batched
    /// `item_id = ANY($1)` read of this collection's `"<table>_assets"`
    /// table for a whole page of item ids — see
    /// `asset_sql::build_item_lookup_plan` for why an array bind rather
    /// than a generated `IN` list, and why the collection-level `''`
    /// sentinel is excluded by the statement itself. An empty page
    /// short-circuits before the pool is even touched, per the trait's own
    /// contract. An undefined relation becomes the named
    /// `AssetsTableMissing`, the same treatment every other assets query
    /// gets — `map_assets_error`'s `key` parameter is the same unused
    /// placeholder (`""`) `assets_list_inner` passes.
    async fn assets_item_lookup_inner(
        &self,
        collection: &CollectionDecl,
        item_ids: &[String],
    ) -> Result<Vec<tellurion_core::AssetRecordEntry>> {
        if item_ids.is_empty() {
            return Ok(Vec::new());
        }
        let table = collection.resolved_table();
        let plan = asset_sql::build_item_lookup_plan(table, item_ids)?;
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let boxed = boxed_params(&plan.params);
        let rows = run_cancellable(client, move |client| async move {
            let refs = param_refs(&boxed);
            client.query(&plan.sql, &refs).await
        })
        .await
        .map_err(|error| map_assets_error(error, table, ""))?;
        rows.iter()
            .map(asset_sql::row_to_asset_record_entry)
            .collect()
    }
}

/// Unwraps `write_apply_inner`'s `#150` conditional-outcome shape for the
/// two unconditional callers (`WriteSink::apply`/`apply_with_crs`). They
/// always pass `expected: None`, and `run_write_transaction` only ever
/// answers `Ok(None)` when `conditional` is `true`, so the `None` arm is
/// unreachable rather than merely unlikely — mapping it to a storage fault
/// keeps that a loud, named failure if the invariant is ever broken, without
/// a panic in a write path.
fn unconditional(result: Result<Option<Sequence>>) -> CoreResult<Sequence> {
    match result.map_err(tellurion_core::Error::from)? {
        Some(sequence) => Ok(sequence),
        None => Err(tellurion_core::Error::Config(
            "an unconditional write reported an optimistic-locking conflict it was never given a \
             precondition for"
                .to_string(),
        )),
    }
}

/// Runs the write path's two statements — the data mutation, then the
/// outbox insert — in one transaction on `client`, committing only after
/// both succeed. Dropping the transaction without an explicit `commit()`
/// (every early `?` return below does exactly that) rolls it back: this is
/// what makes the atomicity invariant hold without any manual rollback call
/// — a data mutation that succeeds syntactically but whose outbox insert
/// then fails (the absent-outbox-table case, most commonly) never becomes a
/// durable row.
///
/// Runs on a spawned task the same way `cancel::run_cancellable` runs a
/// single query, for the same reason (the connection stays valid regardless
/// of whether the caller's own future is later dropped) — not reused
/// directly because that helper's signature is shaped for exactly one query,
/// not a transaction spanning two.
///
/// `conditional` (`#150`) says the data statement carries an
/// optimistic-locking predicate in its own `WHERE`, so "matched zero rows"
/// is a real answer rather than an ordinary no-op: this returns `Ok(None)`
/// and never reaches the outbox insert or the `commit()`, leaving the
/// transaction to roll back on drop. The unconditional path (`false`) is
/// untouched — a `DELETE` of an id that does not exist has always been a
/// successful no-op there, and stays one.
async fn run_write_transaction(request: WriteTransaction) -> Result<Option<i64>> {
    let WriteTransaction {
        client,
        table,
        feature_id,
        kind,
        payload,
        prior_plan,
        returning,
        statement_sql,
        statement_params,
        conditional,
    } = request;
    let handle = tokio::spawn(async move {
        let outbox_table = write_sql::outbox_table_name(&table);
        let mut client = client;
        let tx = client.transaction().await.map_err(PostgisError::from)?;

        // `#141`: read where the feature is BEFORE the statement that moves
        // it, in this same transaction, so no concurrent writer can slip
        // between the two.
        let mut prior = None;
        if let Some((prior_sql, prior_params)) = &prior_plan {
            let refs = param_refs(prior_params);
            let row = tx
                .query_opt(prior_sql, &refs)
                .await
                .map_err(PostgisError::from)?;
            prior = row.as_ref().map(read_crs84_extent).transpose()?.flatten();
        }

        let refs = param_refs(&statement_params);
        let rows = tx
            .query(&statement_sql, &refs)
            .await
            .map_err(PostgisError::from)?;
        if conditional && rows.is_empty() {
            // Somebody else wrote between this request's precondition check
            // and this statement. Returning without `commit()` rolls the
            // whole transaction back, so no data change and no outbox
            // obligation ever become durable.
            return Ok(None);
        }
        let returned = rows.first().map(read_crs84_extent).transpose()?.flatten();
        let mut current = None;
        match returning {
            ReturnedExtent::Current => current = returned,
            // A `DELETE`'s `RETURNING` describes the row it removed, and a
            // delete leaves nothing behind: `current` stays `None`, which is
            // a recorded answer, not an unknown one.
            ReturnedExtent::Prior => prior = returned,
        }
        let extent = ObligationExtent::Crs84 { prior, current };

        let (outbox_sql, outbox_params) = write_sql::build_outbox_insert_plan(
            &table,
            &feature_id,
            kind,
            payload.as_ref(),
            extent,
        )?;
        let outbox_boxed = boxed_params(&outbox_params);
        let outbox_refs = param_refs(&outbox_boxed);
        let row = tx
            .query_one(&outbox_sql, &outbox_refs)
            .await
            .map_err(|error| {
                if error.code() == Some(&SqlState::UNDEFINED_TABLE) {
                    PostgisError::OutboxTableMissing(outbox_table.clone())
                } else {
                    map_outbox_extent_column_missing(PostgisError::from(error), &table)
                }
            })?;
        let sequence: i64 = row.try_get(0).map_err(PostgisError::from)?;

        tx.commit().await.map_err(PostgisError::from)?;
        Ok::<Option<i64>, PostgisError>(Some(sequence))
    });

    match handle.await {
        Ok(result) => result,
        Err(join_err) => Err(PostgisError::from(join_err)),
    }
}

/// Which half of an [`ObligationExtent`] a data statement's own `RETURNING`
/// row describes (`#141`/`#142`). An upsert's `RETURNING` is the row it just
/// wrote — the CURRENT extent; a delete's is the row it just removed — the
/// PRIOR one. Both are the same four `double precision` columns
/// (`write_sql::crs84_extent_select_list`); only their meaning differs, and
/// naming that here keeps the two from being confused at the one place they
/// meet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReturnedExtent {
    Current,
    Prior,
}

/// Everything one `run_write_transaction` call needs. A struct rather than
/// eleven positional arguments because `#141` grew the outbox insert out of
/// the caller (it can no longer be pre-built: the extent it records is only
/// known once the data statement has run) and into this function, the same
/// way `run_create_transaction` has always built its own.
struct WriteTransaction {
    client: deadpool_postgres::Client,
    table: String,
    feature_id: String,
    kind: &'static str,
    payload: Option<serde_json::Value>,
    /// `write_sql::build_prior_extent_plan`, for the lanes that need a
    /// pre-read (upsert); `None` where the data statement's own `RETURNING`
    /// already carries the prior extent (delete).
    prior_plan: Option<(String, Vec<Box<dyn ToSql + Sync + Send>>)>,
    returning: ReturnedExtent,
    statement_sql: String,
    statement_params: Vec<Box<dyn ToSql + Sync + Send>>,
    conditional: bool,
}

/// Reads the four `double precision` columns
/// `write_sql::crs84_extent_select_list` emits, from the END of `row` (every
/// statement that carries them puts them last, after any pk the create lane
/// also returns). All four `NULL` — a row whose geometry column is `NULL` —
/// answers `Ok(None)`: the feature genuinely has no extent, which is a
/// recorded answer, not a missing one.
fn read_crs84_extent(row: &tokio_postgres::Row) -> Result<Option<[f64; 4]>> {
    let base = row.len() - write_sql::CRS84_EXTENT_COLUMNS;
    let mut values = [0.0; write_sql::CRS84_EXTENT_COLUMNS];
    for (offset, slot) in values.iter_mut().enumerate() {
        let value: Option<f64> = row.try_get(base + offset).map_err(PostgisError::from)?;
        match value {
            Some(value) => *slot = value,
            None => return Ok(None),
        }
    }
    Ok(Some(values))
}

/// The create path's counterpart of `run_write_transaction` (`#88`): also
/// two statements in one transaction, same drop-rolls-back atomicity, but
/// kept as its own function rather than folded into `run_write_transaction`
/// because the second statement (the outbox insert) can't be built until
/// the first one (the `INSERT ... RETURNING pk`) reports the pk — either
/// server-minted (`Integer`/`Uuid`) or read back exactly as the caller
/// supplied it (`Text`, `#94`) — `run_write_transaction`'s signature takes
/// both statements pre-built by its caller, which a create can't do.
/// `caller_pk_id` is `Some(id)` only for a `Text` create (`create_inner`'s
/// own doc): it names the id for a `PkConflict` refusal and, by being `None`
/// for `Integer`/`Uuid`, keeps their error handling exactly what it was
/// before `#94` — a `UNIQUE` violation is only ever reinterpreted as a
/// caller-supplied-pk conflict when there was a caller-supplied pk to
/// conflict on.
#[allow(clippy::too_many_arguments)]
async fn run_create_transaction(
    client: deadpool_postgres::Client,
    outbox_table: String,
    pk_column: String,
    id_type: IdType,
    insert_sql: String,
    insert_params: Vec<Box<dyn ToSql + Sync + Send>>,
    table: String,
    feature: serde_json::Value,
    caller_pk_id: Option<String>,
) -> Result<(PkValue, i64)> {
    let handle = tokio::spawn(async move {
        let mut client = client;
        let tx = client.transaction().await.map_err(PostgisError::from)?;

        let refs = param_refs(&insert_params);
        let row = tx.query_one(&insert_sql, &refs).await.map_err(|error| {
            let not_null_on_pk = error.as_db_error().is_some_and(|db_error| {
                *db_error.code() == SqlState::NOT_NULL_VIOLATION
                    && db_error.column() == Some(pk_column.as_str())
            });
            if not_null_on_pk {
                PostgisError::PkNotServerAssignable(table.clone())
            } else if let Some(id) = &caller_pk_id {
                if error.code() == Some(&SqlState::UNIQUE_VIOLATION) {
                    PostgisError::PkConflict {
                        table: table.clone(),
                        id: id.clone(),
                    }
                } else {
                    PostgisError::from(error)
                }
            } else {
                PostgisError::from(error)
            }
        })?;
        // `#87`/`#94`: read back the pk typed per `id_type` — `i64` for
        // `Integer` (byte-for-byte the pre-`#87` read), `uuid::Uuid` for
        // `Uuid`, `String` for `Text`. `validate_id_type_for_create` already
        // confirmed the pk column's real type matches before this
        // transaction ever started, so this typed read should never itself
        // hit a client-side type-mismatch error in practice.
        let pk_value = read_pk_value(&row, 0, id_type)?;
        // `#142`: the row's own CRS84 extent, read off the very `RETURNING`
        // that reported the pk. A server-assigned create has no prior row by
        // construction, so `prior: None` here is a recorded answer.
        let extent = ObligationExtent::Crs84 {
            prior: None,
            current: read_crs84_extent(&row)?,
        };

        let (outbox_sql, outbox_params) = write_sql::build_outbox_insert_plan(
            &table,
            &pk_value.to_string(),
            "upsert",
            Some(&feature),
            extent,
        )?;
        let outbox_boxed = boxed_params(&outbox_params);
        let outbox_refs = param_refs(&outbox_boxed);
        let outbox_row = tx
            .query_one(&outbox_sql, &outbox_refs)
            .await
            .map_err(|error| {
                if error.code() == Some(&SqlState::UNDEFINED_TABLE) {
                    PostgisError::OutboxTableMissing(outbox_table.clone())
                } else {
                    map_outbox_extent_column_missing(PostgisError::from(error), &table)
                }
            })?;
        let sequence: i64 = outbox_row.try_get(0).map_err(PostgisError::from)?;

        tx.commit().await.map_err(PostgisError::from)?;
        Ok::<(PkValue, i64), PostgisError>((pk_value, sequence))
    });

    match handle.await {
        Ok(result) => result,
        Err(join_err) => Err(PostgisError::from(join_err)),
    }
}

/// Builds one batch mutation's two statements (data + outbox) exactly the
/// way `write_apply_inner` does for a single `PUT`/`DELETE` — the only
/// difference is `types` arrives pre-resolved for the WHOLE chunk (see
/// `write_apply_batch_inner`'s own doc) instead of being looked up here.
/// Pure and synchronous: every async step (the property-type catalog
/// lookup) already happened before this is ever called, so a caller can run
/// this once per mutation without any extra round trip.
fn build_batch_item_plan(
    collection: &CollectionDecl,
    mutation: &Mutation,
    types: &HashMap<String, PropertyType>,
    requested_crs: RequestedCrs,
) -> Result<BatchItemPlan> {
    let pk_value = PkValue::parse(collection.id_type, &mutation.feature_id)
        .ok_or_else(|| PostgisError::InvalidFeatureId(mutation.feature_id.clone()))?;

    let (statement_sql, statement_params, payload, prior_plan, returning) = match &mutation.kind {
        MutationKind::Upsert(feature) => {
            let properties = feature
                .get("properties")
                .and_then(serde_json::Value::as_object)
                .cloned()
                .unwrap_or_default();
            let geometry = feature.get("geometry");
            // Batch ingest carries no per-item precondition (`WriteSink::
            // apply_batch`'s own doc: every mutation is an idempotent
            // caller-supplied-id upsert), so no `#150` guard applies here.
            let prior = write_sql::build_prior_extent_plan(collection, pk_value.clone())?;
            let plan = write_sql::build_upsert_plan(
                collection,
                pk_value,
                geometry,
                &properties,
                types,
                requested_crs,
                None,
            )?;
            (
                plan.sql,
                plan.params,
                Some(feature.clone()),
                Some(prior),
                ReturnedExtent::Current,
            )
        }
        MutationKind::Delete => {
            let (sql, params) = write_sql::build_delete_plan(collection, pk_value, None)?;
            (sql, params, None, None, ReturnedExtent::Prior)
        }
    };
    let kind_text = match &mutation.kind {
        MutationKind::Upsert(_) => "upsert",
        MutationKind::Delete => "delete",
    };
    Ok(BatchItemPlan {
        prior_plan,
        statement_sql,
        statement_params,
        returning,
        kind: kind_text,
        payload,
    })
}

/// One batch item's statements. The outbox insert is deliberately NOT
/// pre-built here the way it used to be: `#141`/`#142` made its content
/// depend on what the data statement itself returns, so it is built inside
/// `run_batch_transaction` once that is known — the same shape
/// `run_write_transaction` and `run_create_transaction` now share.
struct BatchItemPlan {
    prior_plan: Option<(String, Vec<SqlParam>)>,
    statement_sql: String,
    statement_params: Vec<SqlParam>,
    returning: ReturnedExtent,
    kind: &'static str,
    payload: Option<serde_json::Value>,
}

/// Runs one batch chunk's worth of mutations inside ONE transaction, each
/// mutation behind its own named `SAVEPOINT` (`#114`, `WriteSink::
/// apply_batch`'s own contract). A mutation that fails — an id that doesn't
/// parse, an unwritable property, a constraint violation the database
/// itself catches — rolls back only its own savepoint (`ROLLBACK TO
/// SAVEPOINT`, via dropping the nested `deadpool_postgres::Transaction`
/// without committing it): everything already committed at an earlier
/// savepoint in this same call stays exactly as it was, and the outer
/// transaction is still perfectly healthy to keep going. Only the FINAL
/// `tx.commit()` actually persists anything — dropping `tx` itself without
/// reaching it rolls back the whole chunk, the same all-or-nothing-per-
/// transaction guarantee `run_write_transaction` gives a single mutation.
///
/// `strict`: stops attempting further mutations the instant one is refused,
/// still committing everything already applied — see `WriteSink::
/// apply_batch`'s own doc for the exact contract this implements.
async fn run_batch_transaction(
    client: deadpool_postgres::Client,
    collection: CollectionDecl,
    outbox_table: String,
    types: HashMap<String, PropertyType>,
    requested_crs: RequestedCrs,
    mutations: Vec<Mutation>,
    strict: bool,
) -> Result<Vec<BatchItemResult>> {
    let handle = tokio::spawn(async move {
        let mut client = client;
        let mut tx = client.transaction().await.map_err(PostgisError::from)?;
        let mut results = Vec::with_capacity(mutations.len());

        for (index, mutation) in mutations.into_iter().enumerate() {
            let feature_id = mutation.feature_id.clone();

            let plan = build_batch_item_plan(&collection, &mutation, &types, requested_crs);
            let outcome = match plan {
                Err(err) if err.is_deterministic_batch_refusal() => {
                    BatchItemOutcome::Refused(err.into())
                }
                Err(err) => return Err(err),
                Ok(plan) => {
                    let savepoint = tx
                        .savepoint(format!("batch_item_{index}"))
                        .await
                        .map_err(PostgisError::from)?;

                    // `#141`: the prior extent, read inside this item's own
                    // savepoint immediately before the statement that
                    // overwrites it.
                    let mut prior = None;
                    if let Some((prior_sql, prior_params)) = &plan.prior_plan {
                        let boxed = boxed_params(prior_params);
                        let refs = param_refs(&boxed);
                        match savepoint.query_opt(prior_sql, &refs).await {
                            Ok(row) => {
                                prior = row.as_ref().map(read_crs84_extent).transpose()?.flatten()
                            }
                            Err(error) => {
                                let error = PostgisError::from(error);
                                savepoint.rollback().await.map_err(PostgisError::from)?;
                                return Err(error);
                            }
                        }
                    }

                    let boxed_statement_params = boxed_params(&plan.statement_params);
                    let refs = param_refs(&boxed_statement_params);
                    let written = savepoint.query(&plan.statement_sql, &refs).await;
                    if let Err(error) = written {
                        let error = PostgisError::from(error);
                        savepoint.rollback().await.map_err(PostgisError::from)?;
                        if error.is_deterministic_batch_refusal() {
                            BatchItemOutcome::Refused(error.into())
                        } else {
                            return Err(error);
                        }
                    } else {
                        let rows = written.expect("the error arm returned above");
                        let returned = rows.first().map(read_crs84_extent).transpose()?.flatten();
                        let mut current = None;
                        match plan.returning {
                            ReturnedExtent::Current => current = returned,
                            ReturnedExtent::Prior => prior = returned,
                        }
                        let (outbox_sql, outbox_params) = write_sql::build_outbox_insert_plan(
                            collection.resolved_table(),
                            &feature_id,
                            plan.kind,
                            plan.payload.as_ref(),
                            ObligationExtent::Crs84 { prior, current },
                        )?;
                        let boxed_outbox_params = boxed_params(&outbox_params);
                        let outbox_refs = param_refs(&boxed_outbox_params);
                        let row = match savepoint.query_one(&outbox_sql, &outbox_refs).await {
                            Ok(row) => row,
                            Err(error) => {
                                let error = if error.code() == Some(&SqlState::UNDEFINED_TABLE) {
                                    PostgisError::OutboxTableMissing(outbox_table.clone())
                                } else {
                                    map_outbox_extent_column_missing(
                                        PostgisError::from(error),
                                        collection.resolved_table(),
                                    )
                                };
                                savepoint.rollback().await.map_err(PostgisError::from)?;
                                return Err(error);
                            }
                        };
                        let sequence: i64 = match row.try_get(0) {
                            Ok(sequence) => sequence,
                            Err(error) => {
                                let error = PostgisError::from(error);
                                savepoint.rollback().await.map_err(PostgisError::from)?;
                                return Err(error);
                            }
                        };
                        if sequence < 0 {
                            savepoint.rollback().await.map_err(PostgisError::from)?;
                            return Err(PostgisError::OutboxSequenceInvalid(sequence));
                        }
                        savepoint.commit().await.map_err(PostgisError::from)?;
                        BatchItemOutcome::Applied(Sequence(sequence as u64))
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

        tx.commit().await.map_err(PostgisError::from)?;
        Ok::<Vec<BatchItemResult>, PostgisError>(results)
    });

    match handle.await {
        Ok(result) => result,
        Err(join_err) => Err(PostgisError::from(join_err)),
    }
}

/// Rewrites a plain `Postgres`/undefined-relation error from an outbox query
/// into the named `OutboxTableMissing` error (`#25`) — every other error
/// passes through unchanged. Shared by `read_after_inner`/
/// `primary_high_water_inner`, whose single-statement queries (unlike
/// `run_write_transaction`'s two-statement one) have no ambiguity about
/// which relation a missing-table error refers to.
fn map_outbox_missing(error: PostgisError, outbox_table_source: &str) -> PostgisError {
    match &error {
        PostgisError::Postgres(pg_error) if pg_error.code() == Some(&SqlState::UNDEFINED_TABLE) => {
            PostgisError::OutboxTableMissing(write_sql::outbox_table_name(outbox_table_source))
        }
        _ => map_outbox_extent_column_missing(error, outbox_table_source),
    }
}

/// `map_outbox_missing`'s `#141`/`#142` sibling, and the exact shape
/// `map_search_column_missing` (`#181`) already established for the derived
/// index: the outbox table exists but predates the `extent_crs84` column, so
/// every statement naming that column fails with an undefined-COLUMN error.
///
/// Named and refused rather than worked around. The server does no DDL —
/// rerunning `tellurion-ingest outbox create-tables` adds the column in
/// place (its `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` is idempotent), and
/// an operator who has not done that gets told exactly that instead of a raw
/// SQL error. Quietly writing obligations without the column would be worse
/// than either: the write would look fine and every tile-invalidation
/// decision downstream would silently fall back to whole-collection bumps,
/// with nothing anywhere saying so.
fn map_outbox_extent_column_missing(
    error: PostgisError,
    outbox_table_source: &str,
) -> PostgisError {
    match &error {
        PostgisError::Postgres(pg_error)
            if pg_error.code() == Some(&SqlState::UNDEFINED_COLUMN) =>
        {
            PostgisError::OutboxExtentColumnMissing(write_sql::outbox_table_name(
                outbox_table_source,
            ))
        }
        _ => error,
    }
}

/// The `index_apply_inner`/`index_applied_high_water_inner` counterpart of
/// `map_outbox_missing` (`#67`): rewrites an undefined-relation error into
/// the named `IndexTableMissing`, so a collection configured with
/// `routing.index` but never provisioned via `tellurion-ingest index
/// create-tables` refuses cleanly instead of surfacing a raw SQL error.
fn map_index_missing(error: PostgisError, index_table_source: &str) -> PostgisError {
    match &error {
        PostgisError::Postgres(pg_error) if pg_error.code() == Some(&SqlState::UNDEFINED_TABLE) => {
            PostgisError::IndexTableMissing(index_sql::index_table_name(index_table_source))
        }
        _ => error,
    }
}

/// `map_index_missing`'s `#181` sibling, applied only on the free-text
/// search path: rewrites an undefined-COLUMN error (the index table exists,
/// but predates `#181`'s generated `search_text` column) into the named
/// `SearchColumnMissing`, so a `q` against an unprovisioned text index
/// refuses with an actionable "rerun `tellurion-ingest index create-tables`"
/// message instead of a raw SQL error. Never applied to a `q`-less search —
/// those never touch the column at all, per this crate's "a grown
/// capability changes nothing until asked for" rule.
fn map_search_column_missing(error: PostgisError, index_table_source: &str) -> PostgisError {
    match &error {
        PostgisError::Postgres(pg_error)
            if pg_error.code() == Some(&SqlState::UNDEFINED_COLUMN) =>
        {
            PostgisError::SearchColumnMissing(index_sql::index_table_name(index_table_source))
        }
        _ => error,
    }
}

/// The `stac_metadata_inner` counterpart of `map_index_missing` (`#202`):
/// rewrites an undefined-relation error into the named `StacTableMissing`,
/// so a collection that declares `stac_metadata: true` but was never
/// provisioned via `tellurion-ingest stac create-tables` refuses cleanly
/// instead of surfacing a raw SQL error — and, crucially, instead of an
/// empty sidecar answer, which is indistinguishable from a provisioned
/// sidecar holding no rows for this page.
/// The `job_*_inner` counterpart of `map_index_missing` (`#182`): rewrites an
/// undefined-relation error into the named `JobsTableMissing`, so a deployment
/// that declared `server.processes` without running `tellurion-ingest
/// processes create-tables` refuses cleanly — and, on the submission path,
/// refuses *before* answering `201` for a job the ledger could never have
/// recorded.
fn map_jobs_missing(error: PostgisError) -> PostgisError {
    match &error {
        PostgisError::Postgres(pg_error) if pg_error.code() == Some(&SqlState::UNDEFINED_TABLE) => {
            PostgisError::JobsTableMissing(job_sql::JOBS_TABLE.to_string())
        }
        _ => error,
    }
}

fn map_stac_missing(error: PostgisError, table_source: &str) -> PostgisError {
    match &error {
        PostgisError::Postgres(pg_error) if pg_error.code() == Some(&SqlState::UNDEFINED_TABLE) => {
            PostgisError::StacTableMissing(stac_sql::stac_table_name(table_source))
        }
        _ => error,
    }
}

/// The `assets_*_inner` counterpart of `map_outbox_missing`/
/// `map_index_missing` (assets-and-object-storage proposal, first slice):
/// rewrites an undefined-relation error into the named `AssetsTableMissing`,
/// and a `UNIQUE (item_id, asset_key)` violation into the named
/// `AssetKeyConflict` naming `key` — the two named refusals this driver's
/// asset methods give instead of a raw SQL error.
fn map_assets_error(error: PostgisError, table_source: &str, key: &str) -> PostgisError {
    match &error {
        PostgisError::Postgres(pg_error) if pg_error.code() == Some(&SqlState::UNDEFINED_TABLE) => {
            PostgisError::AssetsTableMissing(asset_sql::assets_table_name(table_source))
        }
        PostgisError::Postgres(pg_error)
            if pg_error.code() == Some(&SqlState::UNIQUE_VIOLATION) =>
        {
            PostgisError::AssetKeyConflict(key.to_string())
        }
        _ => error,
    }
}

#[async_trait]
impl CatalogSource for PostgisBackend {
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

    async fn geometry_profile(
        &self,
        physical: &PhysicalCollection,
    ) -> CoreResult<Option<GeometryProfile>> {
        self.geometry_profile_inner(physical)
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl FeatureSource for PostgisBackend {
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
        self.item_inner(collection, id, filter, RequestedCrs::Omitted)
            .await
            .map_err(Into::into)
    }

    /// PostGIS compiles a `Filter` to SQL with bound parameters (`#33`, see
    /// `sql::compile_filter`) — the only driver in this workspace that does.
    fn filter_capable(&self) -> bool {
        true
    }

    /// The full CQL2 candidate set (`#105`, `filter::CQL2_CONFORMANCE_CLASSES`):
    /// `sql::compile_filter` compiles every `Filter` variant this workspace's
    /// shared parser produces — comparison/`IS NULL`, `LIKE`/`BETWEEN`/`IN`,
    /// `S_INTERSECTS` plus the six wider spatial predicates, and every
    /// temporal predicate including the twelve-op `Filter::Temporal` — with
    /// no named refusal anywhere in that function. `case-insensitive-
    /// comparison` stays excluded even though `Filter::CaseInsensitiveCompare`
    /// itself compiles: `sql::compile_filter`'s own arm folds case via
    /// Postgres's `lower()`, which only ASCII-folds under a `C`/`POSIX`
    /// collation and never performs full Unicode case folding — see
    /// `filter::CQL2_CONFORMANCE_CLASSES`'s own doc for the full reasoning.
    fn cql2_conformance_classes(&self) -> Vec<&'static str> {
        vec![
            tellurion_core::filter::CQL2_CLASS_BASIC,
            tellurion_core::filter::CQL2_CLASS_CQL2_TEXT,
            tellurion_core::filter::CQL2_CLASS_CQL2_JSON,
            tellurion_core::filter::CQL2_CLASS_BASIC_SPATIAL_FUNCTIONS,
            tellurion_core::filter::CQL2_CLASS_ADVANCED_COMPARISON_OPERATORS,
            tellurion_core::filter::CQL2_CLASS_SPATIAL_FUNCTIONS,
            tellurion_core::filter::CQL2_CLASS_TEMPORAL_FUNCTIONS,
        ]
    }

    /// PostGIS reprojects via `ST_Transform`/`ST_FlipCoordinates` in SQL
    /// (Features Part 2 CRS by Reference, see `sql::reprojected_geom_expr`/
    /// `sql::bbox_envelope_sql`) — the only driver in this workspace that
    /// does.
    fn crs_capable(&self) -> bool {
        true
    }

    /// PostGIS brings a `filter`'s own spatial literals into the storage CRS
    /// with the same two SQL primitives (`sql::geometry_literal_expr`), so it
    /// genuinely honours OGC API — Features Part 3 Requirement 8
    /// (`/req/filter/filter-crs-param`) — the requirement `crs_capable` above
    /// makes binding on this driver by satisfying its "Server supports
    /// additional coordinate reference systems" condition. `#217`: before
    /// that arrived, this pair was the workspace's one live conformance
    /// overclaim — Part 3 advertised, `filter-crs` accepted and silently
    /// ignored, and a filter geometry declared in EPSG:4326-by-authority
    /// evaluated in longitude-first order, returning the wrong features with
    /// a 200.
    fn filter_crs_capable(&self) -> bool {
        true
    }

    async fn item_with_crs(
        &self,
        collection: &CollectionDecl,
        id: &str,
        filter: Option<&Filter>,
        requested_crs: RequestedCrs,
    ) -> CoreResult<Option<serde_json::Value>> {
        self.item_inner(collection, id, filter, requested_crs)
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl TileSource for PostgisBackend {
    /// Storage CRS (`#262`): this lane serves a collection stored in ANY
    /// SRID PostGIS can transform, not only 4326. The tile envelope travels
    /// into the storage CRS for the candidate prune and the geometry travels
    /// into the grid's CRS for the clip, both decided by
    /// `sql::tile_envelope_in_storage_crs`/`sql::storage_geom_in_grid_crs`
    /// from `TileMatrixSet::crs_srid()`; a CRS84-equivalent storage emits
    /// neither extra transform and compiles to byte-for-byte the SQL it
    /// always did.
    ///
    /// There is deliberately no `Option` capability accessor here and no
    /// named refusal to go with it, unlike the `bbox`/`filter` lanes
    /// (`#255`/`#247`). A tile declares no CRS on the wire and carries no
    /// parameter a client could have supplied — OGC 17-083r4 makes `crs` a
    /// mandatory part of the *tile matrix set* (Requirement 1, Table 6), and
    /// OGC API — Tiles Part 1 addresses a tile only by
    /// tileMatrixSet/tileMatrix/tileRow/tileCol — so the only two honest
    /// answers are "transform" and "refuse this collection outright". This
    /// driver can transform, so it transforms; `tellurion-geopackage`, which
    /// cannot, already takes the other branch and refuses by name
    /// (`GeopackageError::UnsupportedTileCrs`, `#89`).
    async fn mvt_tile(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
        filter: Option<&Filter>,
    ) -> CoreResult<Option<Bytes>> {
        self.mvt_tile_inner(collection, TileMatrixSet::WebMercatorQuad, coord, filter)
            .await
            .map_err(Into::into)
    }

    /// `#190`: the one driver in this workspace that serves BOTH registered
    /// grids — its tile envelope is computed per request in SQL
    /// (`sql::build_mvt_candidate_fragment`), so a CRS84 envelope costs the
    /// same one query a mercator one does; every archive-native driver
    /// stays at the trait default (WebMercatorQuad only). Kept consistent
    /// with `mvt_tile_in` below, per the trait's own contract.
    fn supports_tile_matrix_set(&self, _tms: TileMatrixSet) -> bool {
        true
    }

    /// `#190`: the grid-parameterized entry point `AppContext::fetch_mvt`
    /// calls — same pipeline as `mvt_tile`, with `tms` threaded down to the
    /// SQL plan builders.
    async fn mvt_tile_in(
        &self,
        collection: &CollectionDecl,
        tms: TileMatrixSet,
        coord: TileCoord,
        filter: Option<&Filter>,
    ) -> CoreResult<Option<Bytes>> {
        self.mvt_tile_inner(collection, tms, coord, filter)
            .await
            .map_err(Into::into)
    }

    /// Same compiler `FeatureSource::filter_capable` documents (`#33`/`#34`)
    /// — `build_mvt_plan` ANDs a `#34` grant filter into the MVT subquery's
    /// own `WHERE` clause via the identical `sql::compile_filter`.
    fn filter_capable(&self) -> bool {
        true
    }
}

#[async_trait]
impl WriteSink for PostgisBackend {
    async fn apply(&self, collection: &CollectionDecl, mutation: Mutation) -> CoreResult<Sequence> {
        unconditional(
            self.write_apply_inner(collection, mutation, RequestedCrs::Omitted, None)
                .await,
        )
    }

    async fn create(
        &self,
        collection: &CollectionDecl,
        feature: serde_json::Value,
    ) -> CoreResult<(String, Sequence)> {
        self.create_inner(collection, feature, RequestedCrs::Omitted)
            .await
            .map_err(Into::into)
    }

    /// PostGIS reprojects an inbound geometry via `ST_SetSRID`/
    /// `ST_FlipCoordinates` in SQL (`write_sql::input_geom_expr`, OGC API
    /// Features Part 4 `/req/features/crs-other-crs`) — the only driver in
    /// this workspace that does, the write-side mirror of
    /// `FeatureSource::crs_capable` above.
    fn crs_capable(&self) -> bool {
        true
    }

    fn features_conformance_classes(&self, _collection: &CollectionDecl) -> Vec<&'static str> {
        vec![tellurion_core::FEATURES_PART4_FEATURES_CLASS]
    }

    /// `apply`/`create` above commit the data mutation and the outbox
    /// obligation in ONE synchronous backend transaction (this impl block's
    /// own module doc) — a `FeatureSource::item` read issued right after
    /// either call reliably reflects exactly what was just committed, which
    /// is the first thing the OGC API Features — Part 4 Optimistic Locking,
    /// ETags class (`#107`) needs to be sound for this driver.
    ///
    /// `#150` is the second: this driver also implements `row_version`/
    /// `apply_conditional`, so the precondition a caller evaluated is
    /// re-verified as a predicate PostgreSQL evaluates atomically with the
    /// write (`xmin`, see `write_sql`'s own doc) rather than in Rust ahead
    /// of it. Without that half, two writers whose `If-Match` checks both
    /// passed before either applied would both commit — the very lost update
    /// this class exists to prevent — so declaring it on synchronous commit
    /// alone would have been an overclaim.
    fn locking_conformance_classes(&self) -> Vec<&'static str> {
        vec![tellurion_core::locking::OPTIMISTIC_LOCKING_ETAGS_CLASS]
    }

    fn update_conformance_classes(&self) -> Vec<&'static str> {
        vec![tellurion_core::outbox::UPDATE_CONFORMANCE_CLASS]
    }

    /// `#150`: the row's `xmin` — see `write_sql::ROW_VERSION_EXPR` and
    /// `row_version_inner`.
    async fn row_version(
        &self,
        collection: &CollectionDecl,
        feature_id: &str,
    ) -> CoreResult<Option<RowVersion>> {
        self.row_version_inner(collection, feature_id)
            .await
            .map_err(Into::into)
    }

    /// `#150`: `apply_with_crs` with the witness compiled into the data
    /// statement's own `WHERE`, so PostgreSQL decides "is this still the row
    /// the caller checked?" and performs the write as one indivisible step.
    /// `Ok(None)` — nothing written, transaction rolled back — is the
    /// "somebody else got there first" answer this trait's own doc
    /// describes.
    async fn apply_conditional(
        &self,
        collection: &CollectionDecl,
        mutation: Mutation,
        requested_crs: RequestedCrs,
        expected: &RowVersion,
    ) -> CoreResult<Option<Sequence>> {
        self.write_apply_inner(collection, mutation, requested_crs, Some(expected))
            .await
            .map_err(Into::into)
    }

    async fn apply_with_crs(
        &self,
        collection: &CollectionDecl,
        mutation: Mutation,
        requested_crs: RequestedCrs,
    ) -> CoreResult<Sequence> {
        unconditional(
            self.write_apply_inner(collection, mutation, requested_crs, None)
                .await,
        )
    }

    async fn create_with_crs(
        &self,
        collection: &CollectionDecl,
        feature: serde_json::Value,
        requested_crs: RequestedCrs,
    ) -> CoreResult<(String, Sequence)> {
        self.create_inner(collection, feature, requested_crs)
            .await
            .map_err(Into::into)
    }

    /// `#114`: every mutation in `mutations` commits (or is cleanly
    /// discarded) inside ONE transaction — see `run_batch_transaction` for
    /// the per-item `SAVEPOINT` mechanics.
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
impl AssetRecordStore for PostgisBackend {
    async fn register(
        &self,
        collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
        new_record: NewAssetRecord,
    ) -> CoreResult<AssetRecord> {
        self.assets_register_inner(collection, item_id, key, new_record)
            .await
            .map_err(Into::into)
    }

    async fn get(
        &self,
        collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
    ) -> CoreResult<Option<AssetRecord>> {
        self.assets_get_inner(collection, item_id, key)
            .await
            .map_err(Into::into)
    }

    async fn finalize(
        &self,
        collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
        outcome: FinalizeOutcome,
    ) -> CoreResult<AssetRecord> {
        self.assets_finalize_inner(collection, item_id, key, outcome)
            .await
            .map_err(Into::into)
    }

    async fn delete(
        &self,
        collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
    ) -> CoreResult<Option<AssetRecord>> {
        self.assets_delete_inner(collection, item_id, key)
            .await
            .map_err(Into::into)
    }

    async fn list(
        &self,
        collection: &CollectionDecl,
    ) -> CoreResult<Vec<tellurion_core::AssetRecordEntry>> {
        self.assets_list_inner(collection).await.map_err(Into::into)
    }

    async fn item_assets(
        &self,
        collection: &CollectionDecl,
        item_ids: &[String],
    ) -> CoreResult<Vec<tellurion_core::AssetRecordEntry>> {
        self.assets_item_lookup_inner(collection, item_ids)
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl OutboxSource for PostgisBackend {
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

#[async_trait]
impl IndexSink for PostgisBackend {
    async fn apply(&self, collection: &CollectionDecl, obligation: &Obligation) -> CoreResult<()> {
        self.index_apply_inner(collection, obligation)
            .await
            .map_err(Into::into)
    }

    async fn applied_high_water(&self, collection: &CollectionDecl) -> CoreResult<Sequence> {
        self.index_applied_high_water_inner(collection)
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl SearchSource for PostgisBackend {
    async fn search(
        &self,
        collection: &CollectionDecl,
        query: &SearchQuery,
    ) -> CoreResult<SearchPage> {
        self.search_inner(collection, query)
            .await
            .map_err(Into::into)
    }

    /// Identical to `IndexSink::applied_high_water` above — both read the
    /// same `"<table>_index"` high-water mark off the same backend; kept as
    /// two trait methods (per `SearchSource`'s own doc) rather than one
    /// shared helper the traits both dispatch to, since PostGIS's own
    /// `index_applied_high_water_inner` already IS that shared helper.
    async fn applied_high_water(&self, collection: &CollectionDecl) -> CoreResult<Sequence> {
        self.index_applied_high_water_inner(collection)
            .await
            .map_err(Into::into)
    }

    /// `#181`: `SearchQuery::q` compiles to the GIN-backed `search_text`
    /// predicate (`index_sql::build_search_plan`), so this backend
    /// genuinely honors free text — whether a given collection's index
    /// table was actually provisioned with the column is a request-time
    /// question `search` answers with the named `SearchColumnMissing`
    /// refusal, not this capability check (the same split
    /// `StorageDriver::search_source`'s own doc describes for the table
    /// itself).
    fn text_search_capable(&self) -> bool {
        true
    }
}

#[async_trait]
impl StacMetadataSource for PostgisBackend {
    async fn stac_metadata(
        &self,
        collection: &CollectionDecl,
        feature_ids: &[String],
    ) -> CoreResult<HashMap<String, serde_json::Value>> {
        self.stac_metadata_inner(collection, feature_ids)
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl VolumeSource for PostgisBackend {
    async fn volume_tile(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
        filter: Option<&Filter>,
    ) -> CoreResult<Option<VolumeMesh>> {
        self.volume_tile_inner(collection, coord, filter)
            .await
            .map_err(Into::into)
    }

    /// `#70`: `build_volume_plan` ANDs a `#34` grant filter into the volume
    /// query's own `WHERE` clause via the same `sql::compile_filter` the
    /// tiles lane's `filter_capable` already documents.
    fn filter_capable(&self) -> bool {
        true
    }
}

/// The durable job ledger (`#182`).
///
/// Every method is one statement over the shared pool, built by `job_sql.rs`
/// and mapped through `map_jobs_missing` so an unprovisioned ledger is the
/// named `JobsTableMissing` rather than a raw SQL error. No transaction is
/// opened anywhere here: each operation is a single atomic statement by
/// construction — the claim is one `UPDATE ... WHERE job_id = (SELECT ... FOR
/// UPDATE SKIP LOCKED)`, which is exactly the shape that needs no explicit
/// transaction to be safe under concurrency.
#[async_trait]
impl JobStore for PostgisBackend {
    async fn enqueue(&self, submission: &JobSubmission) -> CoreResult<JobRecord> {
        let plan = job_sql::build_enqueue_plan(
            &submission.job_id,
            &submission.process_id,
            &submission.scope.tenant,
            &submission.scope.catalog,
            &submission.inputs,
            submission.dedup_key.as_deref(),
        );
        if let Some(row) = self.job_query_opt(plan).await? {
            return Ok(row);
        }
        // The insert conflicted. With a dedup key, that means an equivalent
        // job is already in play and returning it is the whole point of the
        // key. Without one, the only unique constraint left is the primary
        // key — a `job_id` collision, which for a v4 UUID means the caller
        // reused an id, so refuse rather than silently hand back somebody
        // else's job.
        let Some(dedup_key) = submission.dedup_key.as_deref() else {
            return Err(CoreError::Conflict(format!(
                "job id '{}' is already recorded in the ledger",
                submission.job_id
            )));
        };
        let plan = job_sql::build_dedup_lookup_plan(
            &submission.scope.tenant,
            &submission.scope.catalog,
            &submission.process_id,
            dedup_key,
        );
        // A conflict whose incumbent then vanished (dismissed between the two
        // statements) is a genuine race, not a state to invent an answer for.
        self.job_query_opt(plan).await?.ok_or_else(|| {
            CoreError::Conflict(
                "the job holding this idempotency key changed state while it was being read"
                    .to_string(),
            )
        })
    }

    async fn get(&self, scope: &JobScope, job_id: &str) -> CoreResult<Option<JobRecord>> {
        let plan = job_sql::build_get_plan(&scope.tenant, &scope.catalog, job_id);
        self.job_query_opt(plan).await
    }

    async fn claim_next(
        &self,
        process_ids: &[String],
        visibility: Duration,
    ) -> CoreResult<Option<JobRecord>> {
        if process_ids.is_empty() {
            return Ok(None);
        }
        let plan = job_sql::build_claim_plan(process_ids, visibility.as_secs_f64());
        self.job_query_opt(plan).await
    }

    async fn finish(&self, job_id: &str, outcome: JobOutcome) -> CoreResult<Option<JobRecord>> {
        let plan = match &outcome {
            JobOutcome::Succeeded(results) => job_sql::build_finish_plan(
                job_id,
                JobStatus::Successful.as_str(),
                None,
                Some(results),
            ),
            JobOutcome::Failed(message) => {
                job_sql::build_finish_plan(job_id, JobStatus::Failed.as_str(), Some(message), None)
            }
        };
        self.job_query_opt(plan).await
    }

    async fn dismiss(&self, scope: &JobScope, job_id: &str) -> CoreResult<Option<JobRecord>> {
        let plan = job_sql::build_dismiss_plan(
            &scope.tenant,
            &scope.catalog,
            job_id,
            DISMISSED_JOB_MESSAGE,
        );
        if let Some(record) = self.job_query_opt(plan).await? {
            return Ok(Some(record));
        }
        // Zero rows means either "no such job in this scope" or "already
        // terminal". Reading it back tells the caller which, without this
        // driver ever rewriting a `successful` job's recorded status to make
        // a dismissal response look tidy.
        JobStore::get(self, scope, job_id).await
    }
}

/// The `message` a dismissed job carries. The Standard's own dismissal example
/// (Clause 13.2) uses exactly this wording, and it is the server's statement
/// about the job rather than anything a client supplied — no request body is
/// read on the dismiss path at all.
const DISMISSED_JOB_MESSAGE: &str = "Job dismissed";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postgis_driver_reports_the_resolved_pool_size_as_its_capacity_hint() {
        // `Pool::build` never connects, so a syntactically valid but
        // unreachable URL is enough to construct the driver (same trick
        // pool.rs's own tests use).
        let pool = build_pool("postgres://localhost/nonexistent", 5_000, 9).unwrap();
        let driver = PostgisDriverImpl {
            backend: Arc::new(PostgisBackend { pool }),
            lease: Arc::new(PostgisLease {
                database_url: "postgres://localhost/nonexistent".to_string(),
                connect_timeout: Duration::from_millis(5_000),
            }),
            pool_size: 9,
        };
        assert_eq!(driver.capacity_hint(), Some(9));
    }

    /// `#193`: the lease capability is driver-wide and free to advertise —
    /// constructing the driver opens no coordinator session and takes no
    /// lock, so a deployment that never configures a lease pays nothing for
    /// this existing.
    #[test]
    fn the_lease_capability_is_advertised_without_contacting_anything() {
        let pool = build_pool("postgres://localhost/nonexistent", 5_000, 4).unwrap();
        let driver = PostgisDriverImpl {
            backend: Arc::new(PostgisBackend { pool }),
            lease: Arc::new(PostgisLease {
                database_url: "postgres://localhost/nonexistent".to_string(),
                connect_timeout: Duration::from_millis(5_000),
            }),
            pool_size: 4,
        };
        assert!(StorageDriver::lease(&driver).is_some());
    }

    /// `#105`: PostGIS's own `sql::compile_filter` refuses nothing, so its
    /// declared set pins to the full candidate universe
    /// (`filter::CQL2_CONFORMANCE_CLASSES`) minus `case-insensitive-
    /// comparison`, which no driver declares — see this driver's own
    /// `cql2_conformance_classes` doc.
    #[test]
    fn cql2_conformance_classes_pins_the_full_set_minus_casei() {
        let pool = build_pool("postgres://localhost/nonexistent", 5_000, 4).unwrap();
        let backend = PostgisBackend { pool };
        let declared = FeatureSource::cql2_conformance_classes(&backend);
        assert_eq!(
            declared,
            tellurion_core::filter::CQL2_CONFORMANCE_CLASSES.to_vec()
        );
        assert!(!declared.contains(&tellurion_core::filter::CQL2_CLASS_CASE_INSENSITIVE_COMPARISON));
    }

    /// `#217`: this driver reprojects output geometry, which makes OGC API —
    /// Features Part 3 Requirement 8 (`/req/filter/filter-crs-param`)
    /// binding on it — its condition is "Server supports additional
    /// coordinate reference systems". So the two capabilities must be
    /// declared together: `crs_capable` without `filter_crs_capable` is
    /// precisely the overclaim this issue was opened for, and
    /// `Router::filtering_conformance_classes` reads exactly this pair.
    #[test]
    fn a_reprojecting_driver_also_declares_that_it_honours_filter_crs() {
        let pool = build_pool("postgres://localhost/nonexistent", 5_000, 4).unwrap();
        let backend = PostgisBackend { pool };
        assert!(FeatureSource::crs_capable(&backend));
        assert!(
            FeatureSource::filter_crs_capable(&backend),
            "a crs_capable driver that withholds filter_crs_capable folds Part 3 away; one \
             that declares it without transforming filter literals overclaims"
        );
    }

    /// `#107`: `apply`/`create`'s own single-transaction commit (this
    /// impl's own `locking_conformance_classes` doc) makes the ETags class
    /// sound for this driver.
    #[test]
    fn locking_conformance_classes_declares_etags() {
        let pool = build_pool("postgres://localhost/nonexistent", 5_000, 4).unwrap();
        let backend = PostgisBackend { pool };
        let declared = WriteSink::locking_conformance_classes(&backend);
        assert_eq!(
            declared,
            vec![tellurion_core::locking::OPTIMISTIC_LOCKING_ETAGS_CLASS]
        );
    }

    #[test]
    fn update_conformance_classes_declares_json_merge_patch() {
        let pool = build_pool("postgres://localhost/nonexistent", 5_000, 4).unwrap();
        let backend = PostgisBackend { pool };
        assert_eq!(
            WriteSink::update_conformance_classes(&backend),
            vec![tellurion_core::outbox::UPDATE_CONFORMANCE_CLASS]
        );
    }

    /// The two independently-declared capability signals (`#105`'s own
    /// `storage.rs` doc explains why they aren't derived from one another)
    /// must never drift apart on any driver — `filter_capable() == true`
    /// iff `cql2_conformance_classes()` is non-empty.
    #[test]
    fn filter_capable_and_cql2_conformance_classes_agree() {
        let pool = build_pool("postgres://localhost/nonexistent", 5_000, 4).unwrap();
        let backend = PostgisBackend { pool };
        assert_eq!(
            FeatureSource::filter_capable(&backend),
            !FeatureSource::cql2_conformance_classes(&backend).is_empty()
        );
    }

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

    #[test]
    fn validate_collection_identifiers_accepts_omitted_physical_fields() {
        let decl: CollectionDecl =
            serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap();
        assert!(validate_collection_identifiers(&decl).is_ok());
    }
}
