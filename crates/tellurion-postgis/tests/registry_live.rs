//! Live round-trip tests for `PostgisRegistryReader` (`#42`, relational
//! registry backend) against a real PostGIS instance: point lookups, a
//! keyset paging walk across several pages, an empty page for an unknown
//! scope, and the `RelationalRegistryFactory::connect` failure path. The
//! second half of this file (`#42`, third slice) goes one level up: booting
//! `Router`/`Resolver` from a live `PostgisRegistryReader` via
//! `build_router_and_resolver`, proving a collection published only to the
//! database — never declared in any YAML this test writes — actually routes
//! and serves over the same seam an HTTP request resolves through. Skipped
//! gracefully unless `TELLURION_TEST_DATABASE_URL` is set, matching every
//! other live test in this workspace (`tests/live.rs`).
//!
//! Table DDL is duplicated here from `tellurion-ingest`'s `registry` module
//! rather than shared — the two crates deliberately don't depend on each
//! other (see that module's own doc comment for why); this file is the
//! `tellurion-postgis` side's own proof that its queries match what that
//! DDL actually creates. Rows are inserted with direct SQL mirroring
//! `tellurion-ingest registry publish-tenant`/`publish-catalog`/
//! `publish-collection` rather
//! than by shelling out to that CLI — the same choice this file's own
//! `seed`/`seed_collections` helpers already made for the tests above.

use std::env;
use std::sync::Arc;

use async_trait::async_trait;
use tellurion_core::{
    build_router_and_resolver, snapshot_from_registry_with_page_size, snapshot_tenants, AppConfig,
    CatalogDecl, CatalogSource, CollectionDecl, DriverFactory, FeaturePage, FeatureSource,
    ItemsQuery, PageRequest, PhysicalCollection, Registry, RegistryReader,
    RelationalRegistryFactory, Result, SettingsDecl, StorageDecl, StorageDriver,
};
use tellurion_postgis::test_harness;
use tellurion_postgis::{PostgisRegistryFactory, PostgisRegistryReader, PostgisTenantReader};
use tokio::sync::OnceCell;

static REGISTRY_TABLES_READY: OnceCell<()> = OnceCell::const_new();

const CREATE_TABLES_SQL: &str = "\
CREATE TABLE IF NOT EXISTS registry_tenants (
    internal_id text PRIMARY KEY,
    external_id text NOT NULL UNIQUE,
    decl jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS registry_catalogs (
    internal_id text PRIMARY KEY,
    external_id text NOT NULL,
    tenant_internal_id text NOT NULL,
    decl jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (tenant_internal_id, external_id)
);

CREATE TABLE IF NOT EXISTS registry_collections (
    internal_id text PRIMARY KEY,
    external_id text NOT NULL,
    catalog_internal_id text NOT NULL,
    decl jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (catalog_internal_id, external_id)
);
";

async fn connect_raw(database_url: &str) -> tokio_postgres::Client {
    test_harness::connect(database_url).await
}

fn catalog_decl(id: &str, external_id: &str, tenant: &str) -> CatalogDecl {
    CatalogDecl {
        id: id.to_string(),
        external_id: Some(external_id.to_string()),
        tenant: tenant.to_string(),
        settings: SettingsDecl::default(),
        visibility: Default::default(),
    }
}

fn collection_decl(id: &str, external_id: &str, catalog: &str) -> CollectionDecl {
    serde_yaml::from_str(&format!(
        "id: {id}\nexternal_id: {external_id}\ncatalog: {catalog}\nstorage: main\ntable: demo\ngeometry: geom\npk: id\n"
    ))
    .expect("valid CollectionDecl yaml")
}

/// Creates the tables if they don't already exist and upserts fixtures by
/// `internal_id` — deliberately never `DROP`s: every test in this file
/// shares the same three tables (there is only ever one registry table set
/// per database, matching production), so tests run
/// concurrently by default (`cargo test`'s own parallelism) and a `DROP`
/// here would race another test's queries. Each test uses its own
/// tenant/catalog id prefix, so concurrent seeding never cross-contaminates
/// another test's scoped assertions, and the upsert makes a rerun of the
/// whole suite idempotent rather than failing on a leftover primary key from
/// a previous run.
///
/// The DDL is applied through [`test_harness::apply_fixture_ddl`] (`#138`),
/// not `batch_execute`: `CREATE TABLE IF NOT EXISTS` is **not** safe under
/// concurrent callers — it checks and then inserts catalog rows without a
/// lock spanning the two, so two sessions racing it both see "absent" and
/// the loser fails on `pg_type_typname_nsp_index`. Three binaries in this
/// workspace issue this exact DDL (`tests/tenant_live.rs` and
/// `tellurion-ingest`'s `registry` module test are the other two) and
/// `cargo test --workspace` runs them at once. The process-wide cell below
/// still spares the redundant round trips *within* this binary, but it is
/// the database-side advisory lock — not the cell — that makes the DDL
/// safe, because a cell cannot reach another process. All three sites lock
/// [`test_harness::REGISTRY_TABLES_FIXTURE`]; locking different names would
/// race exactly as before.
async fn seed(
    client: &tokio_postgres::Client,
    tenant: &str,
    catalog_prefix: &str,
    catalog_count: usize,
) {
    REGISTRY_TABLES_READY
        .get_or_init(|| async {
            test_harness::apply_fixture_ddl(
                client,
                test_harness::REGISTRY_TABLES_FIXTURE,
                CREATE_TABLES_SQL,
            )
            .await
            .expect("create (or confirm existing) the registry tables");
        })
        .await;

    let tenant_decl = tellurion_core::TenantDecl {
        id: tenant.to_string(),
        external_id: Some(tenant.to_string()),
        settings: Default::default(),
    };
    let tenant_value = serde_json::to_value(&tenant_decl).unwrap();
    client
        .execute(
            "INSERT INTO registry_tenants (internal_id, external_id, decl) \
             VALUES ($1, $2, $3) \
             ON CONFLICT (internal_id) DO UPDATE SET \
                 external_id = EXCLUDED.external_id, decl = EXCLUDED.decl",
            &[&tenant_decl.id, &tenant_decl.external_id(), &tenant_value],
        )
        .await
        .expect("upsert seeded tenant");

    for i in 0..catalog_count {
        let id = format!("{catalog_prefix}-{i}");
        // Zero-padded so lexicographic ORDER BY external_id matches
        // numeric order for every count this test file uses.
        let external_id = format!("{catalog_prefix}-ext-{i:03}");
        let decl = catalog_decl(&id, &external_id, tenant);
        let value = serde_json::to_value(&decl).unwrap();
        client
            .execute(
                "INSERT INTO registry_catalogs (internal_id, external_id, tenant_internal_id, decl) \
                 VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (internal_id) DO UPDATE SET \
                     external_id = EXCLUDED.external_id, \
                     tenant_internal_id = EXCLUDED.tenant_internal_id, \
                     decl = EXCLUDED.decl",
                &[&decl.id, &external_id, &tenant.to_string(), &value],
            )
            .await
            .expect("upsert seeded catalog");
    }
}

async fn seed_collections(
    client: &tokio_postgres::Client,
    catalog: &str,
    prefix: &str,
    count: usize,
) {
    for i in 0..count {
        let id = format!("{prefix}-{i}");
        let external_id = format!("{prefix}-ext-{i:03}");
        let decl = collection_decl(&id, &external_id, catalog);
        let value = serde_json::to_value(&decl).unwrap();
        client
            .execute(
                "INSERT INTO registry_collections (internal_id, external_id, catalog_internal_id, decl) \
                 VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (internal_id) DO UPDATE SET \
                     external_id = EXCLUDED.external_id, \
                     catalog_internal_id = EXCLUDED.catalog_internal_id, \
                     decl = EXCLUDED.decl",
                &[&decl.id, &external_id, &catalog.to_string(), &value],
            )
            .await
            .expect("upsert seeded collection");
    }
}

#[tokio::test]
async fn point_lookups_hit_and_miss_against_a_live_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping point_lookups_hit_and_miss_against_a_live_database: TELLURION_TEST_DATABASE_URL not set");
        return;
    };

    let raw = connect_raw(&database_url).await;
    seed(&raw, "point-tenant", "point-catalog", 1).await;
    seed_collections(&raw, "point-catalog-0", "point-collection", 1).await;

    let reader = PostgisRegistryReader::connect(&database_url, 60_000)
        .await
        .expect("connects");

    let hit = reader
        .catalog("point-tenant", "point-catalog-ext-000")
        .await
        .expect("catalog query succeeds")
        .expect("the seeded catalog is found");
    assert_eq!(hit.id, "point-catalog-0");
    assert_eq!(hit.tenant, "point-tenant");

    let miss = reader
        .catalog("point-tenant", "nonexistent-external-id")
        .await
        .expect("catalog query succeeds even for an unknown external id");
    assert!(miss.is_none());

    let wrong_tenant = reader
        .catalog("some-other-tenant", "point-catalog-ext-000")
        .await
        .expect("catalog query succeeds");
    assert!(
        wrong_tenant.is_none(),
        "a catalog must not be visible under a tenant it isn't scoped to"
    );

    let collection_hit = reader
        .collection("point-catalog-0", "point-collection-ext-000")
        .await
        .expect("collection query succeeds")
        .expect("the seeded collection is found");
    assert_eq!(collection_hit.id, "point-collection-0");
    assert_eq!(collection_hit.catalog, "point-catalog-0");

    let collection_miss = reader
        .collection("point-catalog-0", "nonexistent")
        .await
        .expect("collection query succeeds");
    assert!(collection_miss.is_none());
}

#[tokio::test]
async fn keyset_paging_walks_every_catalog_exactly_once_in_order() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping keyset_paging_walks_every_catalog_exactly_once_in_order: TELLURION_TEST_DATABASE_URL not set");
        return;
    };

    let raw = connect_raw(&database_url).await;
    seed(&raw, "paging-tenant", "paging-catalog", 5).await;

    let reader = PostgisRegistryReader::connect(&database_url, 60_000)
        .await
        .expect("connects");

    let mut collected = Vec::new();
    let mut after: Option<String> = None;
    let mut pages = 0;
    loop {
        let page = reader
            .list_catalogs(
                "paging-tenant",
                PageRequest {
                    limit: 2,
                    after: after.clone(),
                },
            )
            .await
            .expect("list_catalogs succeeds");
        pages += 1;
        assert!(
            page.items.len() <= 2,
            "a page must never exceed the requested limit"
        );
        collected.extend(page.items.into_iter().map(|c| c.id));
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
        assert!(pages <= 10, "runaway pagination loop");
    }

    assert_eq!(
        collected,
        vec![
            "paging-catalog-0",
            "paging-catalog-1",
            "paging-catalog-2",
            "paging-catalog-3",
            "paging-catalog-4",
        ]
    );
    assert_eq!(pages, 3, "5 items at a limit of 2 is 3 pages (2, 2, 1)");
}

#[tokio::test]
async fn list_collections_reports_no_next_page_when_everything_fits_and_empty_for_an_unknown_catalog(
) {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping list_collections_reports_no_next_page_when_everything_fits_and_empty_for_an_unknown_catalog: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let raw = connect_raw(&database_url).await;
    seed(&raw, "empty-tenant", "empty-catalog", 1).await;
    seed_collections(&raw, "empty-catalog-0", "empty-collection", 3).await;

    let reader = PostgisRegistryReader::connect(&database_url, 60_000)
        .await
        .expect("connects");

    let full_page = reader
        .list_collections(
            "empty-catalog-0",
            PageRequest {
                limit: 10,
                after: None,
            },
        )
        .await
        .expect("list_collections succeeds");
    assert_eq!(full_page.items.len(), 3);
    assert_eq!(full_page.next, None);

    let empty_page = reader
        .list_collections(
            "nonexistent-catalog-internal-id",
            PageRequest {
                limit: 10,
                after: None,
            },
        )
        .await
        .expect("list_collections succeeds for an unknown catalog rather than erroring");
    assert_eq!(empty_page.items, Vec::new());
    assert_eq!(empty_page.next, None);
}

/// `RelationalRegistryFactory::connect` against a database that's genuinely
/// unreachable (a closed local port, not a DNS failure that could hang) must
/// fail — this is the boot/reload failure path the wiring layer relies on to
/// refuse a bad boot / keep the previous registry on a reload. Doesn't need
/// `TELLURION_TEST_DATABASE_URL` at all: the whole point is that the
/// connection never succeeds.
#[tokio::test]
async fn connecting_to_an_unreachable_database_fails_fast() {
    let factory = PostgisRegistryFactory::new(5);
    let result = factory
        .connect("postgres://localhost:1/nonexistent-registry-test")
        .await;
    assert!(
        result.is_err(),
        "an unreachable database must be a connect error, never a lazily-broken reader"
    );
}

// == `build_router_and_resolver` against a live relational registry ========
// (`#42`, third slice)

/// A `StorageDriver` that reports no physical tables and answers every
/// features query with an empty page — enough to exercise routing/resolving
/// without a real "demo" table, since every `CollectionDecl` this file seeds
/// already overrides `table`/`geometry`/`pk` (see `collection_decl`), so
/// `Router::effective_decl`'s fast path never actually queries
/// `catalog_source`.
struct RoutingFakeDriver;

#[async_trait]
impl CatalogSource for RoutingFakeDriver {
    async fn collections(&self) -> Result<Vec<PhysicalCollection>> {
        Ok(vec![])
    }
}

#[async_trait]
impl FeatureSource for RoutingFakeDriver {
    async fn items(
        &self,
        _collection: &CollectionDecl,
        _query: &ItemsQuery,
    ) -> Result<FeaturePage> {
        Ok(FeaturePage {
            features_geojson: vec![],
            number_matched: Some(0),
            next_token: None,
        })
    }

    async fn item(
        &self,
        _collection: &CollectionDecl,
        _id: &str,
        _filter: Option<&tellurion_core::Filter>,
    ) -> Result<Option<serde_json::Value>> {
        Ok(None)
    }
}

impl StorageDriver for RoutingFakeDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(RoutingFakeDriver)
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(Arc::new(RoutingFakeDriver))
    }
}

struct RoutingFakeFactory;

impl DriverFactory for RoutingFakeFactory {
    fn name(&self) -> &str {
        "routing-fake"
    }

    fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
        Ok(Arc::new(RoutingFakeDriver))
    }
}

fn routing_fake_driver_registry() -> Registry {
    let mut registry = Registry::new();
    registry.register(Arc::new(RoutingFakeFactory));
    registry
}

/// The operator's own config for the tests below: `registry.backend:
/// relational` names `main`; per the double-source rule, the config declares
/// no tenants/catalogs/collections of its own at all.
fn routing_operator_config() -> AppConfig {
    let config: AppConfig = serde_yaml::from_str(
        r#"
storages: [ { id: main, driver: routing-fake, url_env: DATABASE_URL } ]
registry: { backend: relational, storage: main }
"#,
    )
    .unwrap();
    config.validate().unwrap();
    config
}

/// The requirement `#42`'s third slice exists for: a collection published
/// only into `registry_collections` — never declared in any YAML this test
/// writes — routes AND resolves against a live `PostgisRegistryReader`,
/// exactly like a YAML-declared one would.
#[tokio::test]
async fn build_router_and_resolver_relational_backend_routes_and_serves_a_collection_not_in_yaml() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping build_router_and_resolver_relational_backend_routes_and_serves_a_collection_not_in_yaml: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let raw = connect_raw(&database_url).await;
    seed(&raw, "router-live-tenant", "router-live-catalog", 1).await;
    seed_collections(&raw, "router-live-catalog-0", "router-live-collection", 1).await;

    let reader = PostgisRegistryReader::connect(&database_url, 60_000)
        .await
        .expect("connects");

    let operator_config = routing_operator_config();
    let driver_registry = routing_fake_driver_registry();
    let tenant_reader = PostgisTenantReader::connect(&database_url, 60_000)
        .await
        .expect("connects the relational tenant reader");

    let (router, resolver, _tenants) =
        build_router_and_resolver(&operator_config, &driver_registry, &reader, &tenant_reader)
            .await
            .expect("walks the live registry and builds successfully");

    let tenant = resolver
        .resolve_tenant("router-live-tenant")
        .await
        .expect("the tenant sourced from the relational reader must resolve");
    let catalog = resolver
        .resolve_catalog(&tenant, "router-live-catalog-ext-000")
        .await
        .expect("the catalog sourced from the live registry must resolve");
    let collection = resolver
        .resolve_collection(&catalog, "router-live-collection-ext-000")
        .await
        .expect("the collection sourced from the live registry must resolve");

    let (decl, _source) = router
        .resolve_features(&tenant, &catalog, &collection)
        .await
        .expect(
            "the resolved internal ids must be routable against the live-registry-built router",
        );
    assert_eq!(decl.external_id(), "router-live-collection-ext-000");
}

/// The pagination requirement (`#42`, third slice), proven end to end
/// against a real database rather than the in-memory `FileRegistryReader`
/// `tellurion-core`'s own unit tests already cover: a `page_size` small
/// enough that both the catalog listing and one catalog's collection
/// listing cross more than one page, walked via
/// `snapshot_from_registry_with_page_size` (the test-only page-size
/// override — see that function's own doc) rather than seeding thousands of
/// rows to force it out of the production default.
#[tokio::test]
async fn snapshot_walk_against_a_live_database_crosses_multiple_pages() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping snapshot_walk_against_a_live_database_crosses_multiple_pages: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let raw = connect_raw(&database_url).await;
    seed(&raw, "snapshot-live-tenant", "snapshot-live-catalog", 5).await;
    seed_collections(
        &raw,
        "snapshot-live-catalog-0",
        "snapshot-live-collection-a",
        3,
    )
    .await;
    seed_collections(
        &raw,
        "snapshot-live-catalog-1",
        "snapshot-live-collection-b",
        2,
    )
    .await;

    let reader = PostgisRegistryReader::connect(&database_url, 60_000)
        .await
        .expect("connects");
    let tenant_reader = PostgisTenantReader::connect(&database_url, 60_000)
        .await
        .expect("connects the relational tenant reader");
    let mut tenants = snapshot_tenants(&tenant_reader)
        .await
        .expect("walks the live tenant snapshot");
    tenants.retain(|tenant| tenant.id == "snapshot-live-tenant");
    assert_eq!(tenants.len(), 1, "the seeded tenant must be present");

    let snapshot = snapshot_from_registry_with_page_size(&tenants, &reader, 2)
        .await
        .expect("walks across multiple pages against the live database");

    assert_eq!(snapshot.catalogs.len(), 5);
    // 3 collections under catalog 0, 2 under catalog 1, 0 under the rest.
    assert_eq!(snapshot.collections.len(), 5);
}
