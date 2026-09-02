//! `PostgisRegistryReader`: a `RegistryReader` (`tellurion_core::registry`,
//! `#42` second slice) backed by two tables, `registry_catalogs` and
//! `registry_collections`. DDL for those tables belongs to
//! `tellurion-ingest`'s `registry create-tables` subcommand — this driver
//! never issues DDL, matching every other "the server never creates tables"
//! boundary in this workspace; it only ever reads. Rows land in the tables
//! via `tellurion-ingest registry publish`, never through this driver
//! either — the server stays read-only.
//!
//! Each table's shape (see `tellurion-ingest`'s `registry` module for the
//! exact `CREATE TABLE` text):
//!
//! - `internal_id text PRIMARY KEY` — the declaration's own internal id.
//! - `external_id text NOT NULL` — the declaration's public id.
//! - `tenant_internal_id text NOT NULL` (catalogs) /
//!   `catalog_internal_id text NOT NULL` (collections) — the scoping column,
//!   matching `RegistryReader`'s own by-internal-id scoping. No
//!   schema-per-tenant, ever: this is the "tenant separation by internal-id
//!   column in a shared table" this workspace's architecture requires.
//! - `decl jsonb NOT NULL` — the exact `CatalogDecl`/`CollectionDecl` serde
//!   shape an operator would otherwise write in YAML, round-tripped through
//!   `serde_json` instead of `serde_yaml`.
//! - `created_at`/`updated_at timestamptz NOT NULL DEFAULT now()`.
//!
//! A `UNIQUE (tenant_internal_id, external_id)` (resp.
//! `catalog_internal_id`) constraint is the only index beyond the primary
//! key: point lookups (`catalog`/`collection`) are an equality match on both
//! of its columns, and listings (`list_catalogs`/`list_collections`) are a
//! keyset range scan — `WHERE tenant_internal_id = $1 AND external_id > $2
//! ORDER BY external_id LIMIT $3`, fetching one extra row (`LIMIT n+1`) to
//! detect a next page — that same composite index serves both query shapes,
//! the same "walk one entry past the limit, never OFFSET" convention
//! `FileRegistryReader::paginate` and `sql::build_items_plan` both already
//! use.

use std::sync::Arc;

use async_trait::async_trait;
use deadpool_postgres::Pool;
use tokio_postgres::Row;

use tellurion_core::{
    effective_cpu_count, CatalogDecl, CollectionDecl, Page, PageRequest, RegistryReader,
    RelationalRegistryFactory, Result as CoreResult,
};

use crate::cancel::run_cancellable;
use crate::error::{PostgisError, Result};
use crate::pool::build_pool;

const CATALOG_POINT_SQL: &str = "SELECT internal_id, external_id, tenant_internal_id, decl FROM registry_catalogs WHERE tenant_internal_id = $1 AND external_id = $2";
const COLLECTION_POINT_SQL: &str = "SELECT internal_id, external_id, catalog_internal_id, decl FROM registry_collections WHERE catalog_internal_id = $1 AND external_id = $2";

const CATALOG_LIST_SQL: &str = "\
SELECT internal_id, external_id, tenant_internal_id, decl FROM registry_catalogs \
WHERE tenant_internal_id = $1 ORDER BY external_id LIMIT $2";
const CATALOG_LIST_AFTER_SQL: &str = "\
SELECT internal_id, external_id, tenant_internal_id, decl FROM registry_catalogs \
WHERE tenant_internal_id = $1 AND external_id > $2 ORDER BY external_id LIMIT $3";

const COLLECTION_LIST_SQL: &str = "\
SELECT internal_id, external_id, catalog_internal_id, decl FROM registry_collections \
WHERE catalog_internal_id = $1 ORDER BY external_id LIMIT $2";
const COLLECTION_LIST_AFTER_SQL: &str = "\
SELECT internal_id, external_id, catalog_internal_id, decl FROM registry_collections \
WHERE catalog_internal_id = $1 AND external_id > $2 ORDER BY external_id LIMIT $3";

/// A `RegistryReader` reading `registry_catalogs`/`registry_collections`
/// over a pooled connection (`pool.rs` — the same machinery
/// `PostgisDriverFactory` uses, not a second pool concept).
pub struct PostgisRegistryReader {
    pool: Pool,
}

impl PostgisRegistryReader {
    /// Connects to `database_url` and attempts a real connection
    /// immediately (`pool.get()`), so an unreachable database surfaces as an
    /// `Err` from this call rather than from a `RegistryReader` method's
    /// first real query — see `RelationalRegistryFactory::connect`'s own
    /// doc for why a boot/reload caller needs that.
    pub async fn connect(database_url: &str, statement_timeout_ms: u64) -> Result<Self> {
        // This reader has no `StorageDecl` of its own to carry an explicit
        // `pool_size` override (it connects straight from a `database_url`,
        // outside the `storages:` list) — cgroup-derived only, same as the
        // driver pool's own "no override" tier.
        let pool_size = crate::pool::derive_pool_size(None, effective_cpu_count());
        let pool = build_pool(database_url, statement_timeout_ms, pool_size)?;
        // Dropped immediately: this is a liveness probe, not a connection
        // this reader needs to hold onto — the pool hands it straight back.
        drop(pool.get().await.map_err(PostgisError::from)?);
        Ok(Self { pool })
    }

    async fn catalog_inner(
        &self,
        tenant_internal_id: &str,
        catalog_external_id: &str,
    ) -> Result<Option<CatalogDecl>> {
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let tenant = tenant_internal_id.to_string();
        let external_id = catalog_external_id.to_string();
        let row_opt = run_cancellable(client, move |client| async move {
            client
                .query_opt(CATALOG_POINT_SQL, &[&tenant, &external_id])
                .await
        })
        .await?;
        row_opt.map(|row| decode_catalog_row(&row)).transpose()
    }

    async fn collection_inner(
        &self,
        catalog_internal_id: &str,
        collection_external_id: &str,
    ) -> Result<Option<CollectionDecl>> {
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let catalog = catalog_internal_id.to_string();
        let external_id = collection_external_id.to_string();
        let row_opt = run_cancellable(client, move |client| async move {
            client
                .query_opt(COLLECTION_POINT_SQL, &[&catalog, &external_id])
                .await
        })
        .await?;
        row_opt.map(|row| decode_collection_row(&row)).transpose()
    }

    async fn list_catalogs_inner(
        &self,
        tenant_internal_id: &str,
        page: PageRequest,
    ) -> Result<Page<CatalogDecl>> {
        let limit = page.limit.max(1) as i64;
        let fetch_limit = limit.saturating_add(1);
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let tenant = tenant_internal_id.to_string();
        let rows = match page.after {
            Some(after) => {
                run_cancellable(client, move |client| async move {
                    client
                        .query(CATALOG_LIST_AFTER_SQL, &[&tenant, &after, &fetch_limit])
                        .await
                })
                .await?
            }
            None => {
                run_cancellable(client, move |client| async move {
                    client
                        .query(CATALOG_LIST_SQL, &[&tenant, &fetch_limit])
                        .await
                })
                .await?
            }
        };
        decode_page(rows, limit as usize, decode_catalog_row)
    }

    async fn list_collections_inner(
        &self,
        catalog_internal_id: &str,
        page: PageRequest,
    ) -> Result<Page<CollectionDecl>> {
        let limit = page.limit.max(1) as i64;
        let fetch_limit = limit.saturating_add(1);
        let client = self.pool.get().await.map_err(PostgisError::from)?;
        let catalog = catalog_internal_id.to_string();
        let rows = match page.after {
            Some(after) => {
                run_cancellable(client, move |client| async move {
                    client
                        .query(COLLECTION_LIST_AFTER_SQL, &[&catalog, &after, &fetch_limit])
                        .await
                })
                .await?
            }
            None => {
                run_cancellable(client, move |client| async move {
                    client
                        .query(COLLECTION_LIST_SQL, &[&catalog, &fetch_limit])
                        .await
                })
                .await?
            }
        };
        decode_page(rows, limit as usize, decode_collection_row)
    }
}

/// Turns up to `requested_limit + 1` rows (each carrying `decl`/
/// `external_id`) into a `Page`: `has_more` (a row beyond `requested_limit`
/// actually came back) reports the last *returned* row's `external_id` as
/// `next`, mirroring `FileRegistryReader::paginate`'s own "walk one entry
/// past the limit to detect more" shape.
fn decode_page<T>(
    rows: Vec<Row>,
    requested_limit: usize,
    decode: fn(&Row) -> Result<T>,
) -> Result<Page<T>> {
    let has_more = rows.len() > requested_limit;
    let page_rows = if has_more {
        &rows[..requested_limit]
    } else {
        &rows[..]
    };

    let mut items = Vec::with_capacity(page_rows.len());
    let mut last_external_id: Option<String> = None;
    for row in page_rows {
        let decl = decode(row)?;
        let external_id: String = row.try_get("external_id").map_err(PostgisError::from)?;
        items.push(decl);
        last_external_id = Some(external_id);
    }

    Ok(Page {
        items,
        next: if has_more { last_external_id } else { None },
    })
}

fn decode_catalog_row(row: &Row) -> Result<CatalogDecl> {
    let internal_id: String = row.try_get("internal_id").map_err(PostgisError::from)?;
    let external_id: String = row.try_get("external_id").map_err(PostgisError::from)?;
    let tenant_internal_id: String = row
        .try_get("tenant_internal_id")
        .map_err(PostgisError::from)?;
    let value: serde_json::Value = row.try_get("decl").map_err(PostgisError::from)?;
    let decl: CatalogDecl = serde_json::from_value(value).map_err(PostgisError::from)?;
    validate_catalog_identity(&internal_id, &external_id, &tenant_internal_id, &decl)?;
    Ok(decl)
}

fn decode_collection_row(row: &Row) -> Result<CollectionDecl> {
    let internal_id: String = row.try_get("internal_id").map_err(PostgisError::from)?;
    let external_id: String = row.try_get("external_id").map_err(PostgisError::from)?;
    let catalog_internal_id: String = row
        .try_get("catalog_internal_id")
        .map_err(PostgisError::from)?;
    let value: serde_json::Value = row.try_get("decl").map_err(PostgisError::from)?;
    let decl: CollectionDecl = serde_json::from_value(value).map_err(PostgisError::from)?;
    validate_collection_identity(&internal_id, &external_id, &catalog_internal_id, &decl)?;
    Ok(decl)
}

fn validate_catalog_identity(
    internal_id: &str,
    external_id: &str,
    tenant_internal_id: &str,
    decl: &CatalogDecl,
) -> Result<()> {
    if decl.id != internal_id
        || decl.external_id() != external_id
        || decl.tenant != tenant_internal_id
    {
        return Err(PostgisError::MalformedRegistryRow(format!(
            "registry_catalogs identity columns ({internal_id}, {external_id}, {tenant_internal_id}) do not match decl ({}, {}, {})",
            decl.id,
            decl.external_id(),
            decl.tenant
        )));
    }
    Ok(())
}

fn validate_collection_identity(
    internal_id: &str,
    external_id: &str,
    catalog_internal_id: &str,
    decl: &CollectionDecl,
) -> Result<()> {
    if decl.id != internal_id
        || decl.external_id() != external_id
        || decl.catalog != catalog_internal_id
    {
        return Err(PostgisError::MalformedRegistryRow(format!(
            "registry_collections identity columns ({internal_id}, {external_id}, {catalog_internal_id}) do not match decl ({}, {}, {})",
            decl.id,
            decl.external_id(),
            decl.catalog
        )));
    }
    Ok(())
}

#[async_trait]
impl RegistryReader for PostgisRegistryReader {
    async fn catalog(
        &self,
        tenant_internal_id: &str,
        catalog_external_id: &str,
    ) -> CoreResult<Option<CatalogDecl>> {
        self.catalog_inner(tenant_internal_id, catalog_external_id)
            .await
            .map_err(Into::into)
    }

    async fn collection(
        &self,
        catalog_internal_id: &str,
        collection_external_id: &str,
    ) -> CoreResult<Option<CollectionDecl>> {
        self.collection_inner(catalog_internal_id, collection_external_id)
            .await
            .map_err(Into::into)
    }

    async fn list_catalogs(
        &self,
        tenant_internal_id: &str,
        page: PageRequest,
    ) -> CoreResult<Page<CatalogDecl>> {
        self.list_catalogs_inner(tenant_internal_id, page)
            .await
            .map_err(Into::into)
    }

    async fn list_collections(
        &self,
        catalog_internal_id: &str,
        page: PageRequest,
    ) -> CoreResult<Page<CollectionDecl>> {
        self.list_collections_inner(catalog_internal_id, page)
            .await
            .map_err(Into::into)
    }
}

/// Registers the `relational` registry backend (`#42`): connects a
/// [`PostgisRegistryReader`] on demand, the same "driver stays out of core"
/// boundary `PostgisDriverFactory` already draws for storage drivers. The
/// wiring layer (the `tellurion` binary) constructs one and passes it to
/// `tellurion_core::registry::build_registry_reader`.
pub struct PostgisRegistryFactory {
    statement_timeout_ms: u64,
}

impl PostgisRegistryFactory {
    /// `request_timeout_s` mirrors `PostgisDriverFactory::new`'s own
    /// parameter — every pooled connection's `statement_timeout` matches the
    /// server's HTTP request ceiling, so a stuck registry query fails fast
    /// rather than holding a pool connection past what a request would ever
    /// wait for anyway.
    pub fn new(request_timeout_s: u64) -> Self {
        Self {
            statement_timeout_ms: request_timeout_s.saturating_mul(1000),
        }
    }
}

#[async_trait]
impl RelationalRegistryFactory for PostgisRegistryFactory {
    fn name(&self) -> &str {
        crate::RELATIONAL_IMPLEMENTATION_NAME
    }

    async fn connect(&self, database_url: &str) -> CoreResult<Arc<dyn RegistryReader>> {
        let reader = PostgisRegistryReader::connect(database_url, self.statement_timeout_ms)
            .await
            .map_err(Into::<tellurion_core::Error>::into)?;
        Ok(Arc::new(reader))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `#162`: the wire name an operator writes as `registry.implementation`,
    /// pinned. Both relational halves this crate provides must declare the
    /// SAME name (one `registry.implementation` selects both), and that name
    /// is this crate's storage-driver name too — one driver crate, one name,
    /// whichever seam is naming it.
    #[test]
    fn both_relational_factories_declare_this_crates_one_stable_name() {
        use tellurion_core::{DriverFactory, RelationalTenantFactory};

        assert_eq!(crate::RELATIONAL_IMPLEMENTATION_NAME, "postgis");
        assert_eq!(
            PostgisRegistryFactory::new(30).name(),
            crate::RELATIONAL_IMPLEMENTATION_NAME
        );
        assert_eq!(
            crate::PostgisTenantFactory::new(30).name(),
            crate::RELATIONAL_IMPLEMENTATION_NAME
        );
        assert_eq!(
            crate::PostgisDriverFactory::new(30).name(),
            crate::RELATIONAL_IMPLEMENTATION_NAME
        );
    }

    #[test]
    fn queries_scope_point_lookups_to_both_the_tenant_or_catalog_and_the_external_id() {
        assert!(CATALOG_POINT_SQL.contains("tenant_internal_id = $1"));
        assert!(CATALOG_POINT_SQL.contains("external_id = $2"));
        assert!(COLLECTION_POINT_SQL.contains("catalog_internal_id = $1"));
        assert!(COLLECTION_POINT_SQL.contains("external_id = $2"));
    }

    #[test]
    fn list_queries_order_by_external_id_and_never_use_offset() {
        for sql in [
            CATALOG_LIST_SQL,
            CATALOG_LIST_AFTER_SQL,
            COLLECTION_LIST_SQL,
            COLLECTION_LIST_AFTER_SQL,
        ] {
            assert!(sql.contains("ORDER BY external_id"), "sql was: {sql}");
            assert!(!sql.to_uppercase().contains("OFFSET"), "sql was: {sql}");
        }
        assert!(CATALOG_LIST_AFTER_SQL.contains("external_id > $2"));
        assert!(COLLECTION_LIST_AFTER_SQL.contains("external_id > $2"));
    }

    #[test]
    fn decode_page_reports_no_next_when_every_row_fits_the_limit() {
        // No live rows needed: an empty `Vec<Row>` already exercises the
        // "fewer rows than requested" branch without a real connection.
        let page: Page<CatalogDecl> = decode_page(Vec::new(), 10, decode_catalog_row).unwrap();
        assert_eq!(page.items, Vec::new());
        assert_eq!(page.next, None);
    }

    #[test]
    fn postgis_registry_factory_derives_statement_timeout_from_request_timeout() {
        let factory = PostgisRegistryFactory::new(60);
        assert_eq!(factory.statement_timeout_ms, 60_000);
    }

    #[test]
    fn catalog_identity_columns_must_match_the_json_declaration() {
        let decl = CatalogDecl {
            id: "catalog-internal".to_string(),
            external_id: Some("catalog-external".to_string()),
            tenant: "tenant-internal".to_string(),
            settings: Default::default(),
            visibility: Default::default(),
        };
        assert!(validate_catalog_identity(
            "catalog-internal",
            "catalog-external",
            "tenant-internal",
            &decl
        )
        .is_ok());
        assert!(
            validate_catalog_identity("wrong", "catalog-external", "tenant-internal", &decl)
                .is_err()
        );
        assert!(
            validate_catalog_identity("catalog-internal", "wrong", "tenant-internal", &decl)
                .is_err()
        );
        assert!(
            validate_catalog_identity("catalog-internal", "catalog-external", "wrong", &decl)
                .is_err()
        );
    }

    #[test]
    fn collection_identity_columns_must_match_the_json_declaration() {
        let decl: CollectionDecl = serde_yaml::from_str(
            "id: collection-internal\nexternal_id: collection-external\ncatalog: catalog-internal\nstorage: main\ntable: demo\ngeometry: geom\npk: id\n",
        )
        .unwrap();
        assert!(validate_collection_identity(
            "collection-internal",
            "collection-external",
            "catalog-internal",
            &decl
        )
        .is_ok());
        assert!(validate_collection_identity(
            "wrong",
            "collection-external",
            "catalog-internal",
            &decl
        )
        .is_err());
        assert!(validate_collection_identity(
            "collection-internal",
            "wrong",
            "catalog-internal",
            &decl
        )
        .is_err());
        assert!(validate_collection_identity(
            "collection-internal",
            "collection-external",
            "wrong",
            &decl
        )
        .is_err());
    }

    #[test]
    fn connect_bounds_the_checkout_wait_the_same_way_the_driver_pool_does() {
        // `Pool::build` never connects, so a syntactically valid but
        // unreachable URL is enough to construct the pool underneath a
        // `PostgisRegistryReader` (same trick `pool.rs`/`driver.rs`'s own
        // tests use) — `PostgisRegistryReader::connect` itself always
        // attempts a real connection, so it isn't exercised here.
        let pool = build_pool("postgres://localhost/nonexistent", 5_000, 8).unwrap();
        assert_eq!(
            pool.timeouts().wait,
            Some(std::time::Duration::from_millis(5_000))
        );
    }
}
