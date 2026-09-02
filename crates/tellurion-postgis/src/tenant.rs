//! `PostgisTenantReader`: a `TenantReader` (`tellurion_core::tenant`, `#143`)
//! backed by one table, `registry_tenants` — the tenant-side sibling of
//! `registry.rs`'s `PostgisRegistryReader`, which backs `registry_catalogs`/
//! `registry_collections`. DDL for that table belongs to
//! `tellurion-ingest`'s `registry create-tables` subcommand (the same
//! command that provisions the catalog/collection tables — see this
//! driver's own doc for why the server never issues DDL); rows land in it
//! via `tellurion-ingest registry publish-tenant`, never through this
//! driver — the server stays read-only.
//!
//! The table's shape (see `tellurion-ingest`'s `registry` module for the
//! exact `CREATE TABLE` text):
//!
//! - `internal_id text PRIMARY KEY` — the declaration's own internal id.
//! - `external_id text NOT NULL UNIQUE` — the declaration's public id. A
//!   tenant has no owning scope (unlike a catalog, scoped by tenant, or a
//!   collection, scoped by catalog), so this is a plain, unscoped
//!   uniqueness constraint rather than the `UNIQUE (scope, external_id)`
//!   composite `registry_catalogs`/`registry_collections` each use.
//! - `decl jsonb NOT NULL` — the exact `TenantDecl` serde shape an operator
//!   would otherwise write in YAML, round-tripped through `serde_json`
//!   instead of `serde_yaml`.
//! - `created_at`/`updated_at timestamptz NOT NULL DEFAULT now()`.
//!
//! `external_id`'s own `UNIQUE` constraint is the only index beyond the
//! primary key: the point lookup (`tenant`) is a plain equality match on it,
//! and the listing (`list_tenants`) is a keyset range scan — `WHERE
//! external_id > $1 ORDER BY external_id LIMIT $2`, fetching one extra row
//! (`LIMIT n+1`) to detect a next page — the same "walk one entry past the
//! limit, never OFFSET" convention `registry.rs`'s own queries already use.

use std::sync::Arc;

use async_trait::async_trait;
use deadpool_postgres::Pool;
use tokio_postgres::Row;

use tellurion_core::{
    effective_cpu_count, Page, PageRequest, RelationalTenantFactory, Result as CoreResult,
    TenantDecl, TenantReader,
};

use crate::cancel::run_cancellable;
use crate::error::{PostgisError, Result};
use crate::pool::build_pool;

const TENANT_POINT_SQL: &str =
    "SELECT internal_id, external_id, decl FROM registry_tenants WHERE external_id = $1";

const TENANT_LIST_SQL: &str = "\
SELECT internal_id, external_id, decl FROM registry_tenants ORDER BY external_id LIMIT $1";
const TENANT_LIST_AFTER_SQL: &str = "\
SELECT internal_id, external_id, decl FROM registry_tenants \
WHERE external_id > $1 ORDER BY external_id LIMIT $2";

/// A `TenantReader` reading `registry_tenants` over a pooled connection
/// (`pool.rs` — the same machinery `PostgisDriverFactory`/
/// `PostgisRegistryReader` already use, not a second pool concept).
pub struct PostgisTenantReader {
    pool: Pool,
}

impl PostgisTenantReader {
    /// Connects to `database_url` and attempts a real connection
    /// immediately (`pool.get()`), so an unreachable database surfaces as an
    /// `Err` from this call rather than from a `TenantReader` method's first
    /// real query — see `RelationalTenantFactory::connect`'s own doc for why
    /// a boot/reload caller needs that.
    pub async fn connect(database_url: &str, statement_timeout_ms: u64) -> Result<Self> {
        // Same "no explicit override, cgroup-derived pool size" tier
        // `PostgisRegistryReader::connect` already uses — this reader
        // connects straight from a `database_url`, outside the `storages:`
        // list, so there is no `StorageDecl.pool_size` to read.
        let pool_size = crate::pool::derive_pool_size(None, effective_cpu_count());
        let pool = build_pool(database_url, statement_timeout_ms, pool_size)?;
        // Dropped immediately: this is a liveness probe, not a connection
        // this reader needs to hold onto — the pool hands it straight back.
        drop(pool.get().await.map_err(PostgisError::from)?);
        Ok(Self { pool })
    }

    async fn tenant_inner(&self, external_id: &str) -> Result<Option<TenantDecl>> {
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let external_id = external_id.to_string();
        let row_opt = run_cancellable(client, move |client| async move {
            client.query_opt(TENANT_POINT_SQL, &[&external_id]).await
        })
        .await?;
        decode_point(row_opt)
    }

    async fn list_tenants_inner(&self, page: PageRequest) -> Result<Page<TenantDecl>> {
        let limit = page.limit.max(1) as i64;
        let fetch_limit = limit.saturating_add(1);
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let rows = match page.after {
            Some(after) => {
                run_cancellable(client, move |client| async move {
                    client
                        .query(TENANT_LIST_AFTER_SQL, &[&after, &fetch_limit])
                        .await
                })
                .await?
            }
            None => {
                run_cancellable(client, move |client| async move {
                    client.query(TENANT_LIST_SQL, &[&fetch_limit]).await
                })
                .await?
            }
        };
        decode_page(rows, limit as usize)
    }
}

/// Decodes a single point-lookup row's `decl` jsonb column, or `None` for no
/// matching row.
fn decode_point(row_opt: Option<Row>) -> Result<Option<TenantDecl>> {
    row_opt.map(|row| decode_tenant_row(&row)).transpose()
}

/// Turns up to `requested_limit + 1` rows (each carrying `decl`/
/// `external_id`) into a `Page` — mirrors `registry.rs`'s own `decode_page`
/// exactly; duplicated rather than shared because the two live in different
/// crates with no shared source of truth beyond this comment (same "kept in
/// sync by hand" boundary `tellurion-ingest`'s DDL doc already draws).
fn decode_page(rows: Vec<Row>, requested_limit: usize) -> Result<Page<TenantDecl>> {
    let has_more = rows.len() > requested_limit;
    let page_rows = if has_more {
        &rows[..requested_limit]
    } else {
        &rows[..]
    };

    let mut items = Vec::with_capacity(page_rows.len());
    let mut last_external_id: Option<String> = None;
    for row in page_rows {
        let decl = decode_tenant_row(row)?;
        let external_id: String = row.try_get("external_id").map_err(PostgisError::from)?;
        items.push(decl);
        last_external_id = Some(external_id);
    }

    Ok(Page {
        items,
        next: if has_more { last_external_id } else { None },
    })
}

fn decode_tenant_row(row: &Row) -> Result<TenantDecl> {
    let internal_id: String = row.try_get("internal_id").map_err(PostgisError::from)?;
    let external_id: String = row.try_get("external_id").map_err(PostgisError::from)?;
    let value: serde_json::Value = row.try_get("decl").map_err(PostgisError::from)?;
    let decl: TenantDecl = serde_json::from_value(value).map_err(PostgisError::from)?;
    validate_tenant_identity(&internal_id, &external_id, &decl)?;
    Ok(decl)
}

fn validate_tenant_identity(internal_id: &str, external_id: &str, decl: &TenantDecl) -> Result<()> {
    if decl.id != internal_id || decl.external_id() != external_id {
        return Err(PostgisError::MalformedRegistryRow(format!(
            "registry_tenants identity columns ({internal_id}, {external_id}) do not match decl ({}, {})",
            decl.id,
            decl.external_id()
        )));
    }
    Ok(())
}

#[async_trait]
impl TenantReader for PostgisTenantReader {
    async fn tenant(&self, external_id: &str) -> CoreResult<Option<TenantDecl>> {
        self.tenant_inner(external_id).await.map_err(Into::into)
    }

    async fn list_tenants(&self, page: PageRequest) -> CoreResult<Page<TenantDecl>> {
        self.list_tenants_inner(page).await.map_err(Into::into)
    }
}

/// Registers the relational tenant backend (`#143`): connects a
/// [`PostgisTenantReader`] on demand, the same "driver stays out of core"
/// boundary `PostgisRegistryFactory` already draws. The wiring layer (the
/// `tellurion` binary) constructs one and passes it to
/// `tellurion_core::tenant::build_tenant_reader`.
pub struct PostgisTenantFactory {
    statement_timeout_ms: u64,
}

impl PostgisTenantFactory {
    /// `request_timeout_s` mirrors `PostgisRegistryFactory::new`'s own
    /// parameter — every pooled connection's `statement_timeout` matches the
    /// server's HTTP request ceiling.
    pub fn new(request_timeout_s: u64) -> Self {
        Self {
            statement_timeout_ms: request_timeout_s.saturating_mul(1000),
        }
    }
}

#[async_trait]
impl RelationalTenantFactory for PostgisTenantFactory {
    fn name(&self) -> &str {
        crate::RELATIONAL_IMPLEMENTATION_NAME
    }

    async fn connect(&self, database_url: &str) -> CoreResult<Arc<dyn TenantReader>> {
        let reader = PostgisTenantReader::connect(database_url, self.statement_timeout_ms)
            .await
            .map_err(Into::<tellurion_core::Error>::into)?;
        Ok(Arc::new(reader))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_query_scopes_by_external_id_alone_no_parent_scope_column() {
        assert!(TENANT_POINT_SQL.contains("external_id = $1"));
        assert!(!TENANT_POINT_SQL
            .to_lowercase()
            .contains("tenant_internal_id"));
    }

    #[test]
    fn list_queries_order_by_external_id_and_never_use_offset() {
        for sql in [TENANT_LIST_SQL, TENANT_LIST_AFTER_SQL] {
            assert!(sql.contains("ORDER BY external_id"), "sql was: {sql}");
            assert!(!sql.to_uppercase().contains("OFFSET"), "sql was: {sql}");
        }
        assert!(TENANT_LIST_AFTER_SQL.contains("external_id > $1"));
    }

    #[test]
    fn decode_page_reports_no_next_when_every_row_fits_the_limit() {
        // No live rows needed: an empty `Vec<Row>` already exercises the
        // "fewer rows than requested" branch without a real connection.
        let page = decode_page(Vec::new(), 10).unwrap();
        assert_eq!(page.items, Vec::new());
        assert_eq!(page.next, None);
    }

    #[test]
    fn postgis_tenant_factory_derives_statement_timeout_from_request_timeout() {
        let factory = PostgisTenantFactory::new(60);
        assert_eq!(factory.statement_timeout_ms, 60_000);
    }

    #[test]
    fn tenant_identity_columns_must_match_the_json_declaration() {
        let decl = TenantDecl {
            id: "tenant-internal".to_string(),
            external_id: Some("tenant-external".to_string()),
            settings: Default::default(),
        };
        assert!(validate_tenant_identity("tenant-internal", "tenant-external", &decl).is_ok());
        assert!(validate_tenant_identity("wrong", "tenant-external", &decl).is_err());
        assert!(validate_tenant_identity("tenant-internal", "wrong", &decl).is_err());
    }

    #[test]
    fn connect_bounds_the_checkout_wait_the_same_way_the_driver_pool_does() {
        // `Pool::build` never connects, so a syntactically valid but
        // unreachable URL is enough to construct the pool underneath a
        // `PostgisTenantReader` (same trick `registry.rs`'s own tests use) —
        // `PostgisTenantReader::connect` itself always attempts a real
        // connection, so it isn't exercised here.
        let pool = build_pool("postgres://localhost/nonexistent", 5_000, 8).unwrap();
        assert_eq!(
            pool.timeouts().wait,
            Some(std::time::Duration::from_millis(5_000))
        );
    }
}
