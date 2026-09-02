//! Registry DDL + publish (`#42`, relational registry backend; `#143` for
//! tenants). The server never creates or writes `registry_tenants`/
//! `registry_catalogs`/`registry_collections` — this module is the only
//! place either happens: `create_tables` issues the DDL (and always prints
//! it first, so an operator without CLI database access can apply it by
//! hand), and `publish_tenant`/`publish_catalog`/`publish_collection` upsert
//! one `TenantDecl`/`CatalogDecl`/`CollectionDecl` — parsed from the same
//! YAML shape an operator would otherwise paste straight into `config.yaml`
//! — into its table. That's how rows get into any of the three tables at
//! all; there is no other write path.
//!
//! See `tellurion-postgis`'s `registry`/`tenant` modules for the
//! `RegistryReader`/`TenantReader` this schema backs. The two crates don't
//! depend on each other (this crate never depends on a driver crate — see
//! this crate's own top-level doc), so the table/column names below must
//! stay in sync with those modules' own SQL text by hand; there is no
//! shared source of truth to enforce it beyond this comment and the live
//! test in each crate that exercises both sides.

use std::path::{Path, PathBuf};

use anyhow::Context;
use tellurion_core::{CatalogDecl, CollectionDecl, TenantDecl};
use tokio_postgres::Client;

/// `IF NOT EXISTS` on every table: re-running this against an
/// already-provisioned database is a no-op, not an error — the same
/// "idempotent, operator-runnable" property `seed.rs`'s own
/// `DROP TABLE IF EXISTS` gives that command, minus the drop (a registry
/// table is not disposable demo data). No triggers, no partitioning, no
/// indexes beyond the primary key and the one `UNIQUE` constraint each
/// query shape (point lookup, keyset listing) actually needs — see
/// `tellurion-postgis::registry`/`::tenant`'s own docs for which queries
/// each index serves. `registry_tenants` has no scoping column at all
/// (unlike the other two, scoped by tenant/catalog respectively): a tenant
/// has no owning parent, so its `external_id` uniqueness is a plain,
/// unscoped `UNIQUE` rather than the `UNIQUE (scope, external_id)`
/// composite the other two tables use.
pub const CREATE_REGISTRY_TABLES_SQL: &str = "\
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

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'registry_catalogs_tenant_fk'
          AND conrelid = 'registry_catalogs'::regclass
    ) THEN
        BEGIN
            ALTER TABLE registry_catalogs
                ADD CONSTRAINT registry_catalogs_tenant_fk
                FOREIGN KEY (tenant_internal_id) REFERENCES registry_tenants(internal_id) NOT VALID;
        EXCEPTION WHEN duplicate_object THEN
            NULL;
        END;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'registry_collections_catalog_fk'
          AND conrelid = 'registry_collections'::regclass
    ) THEN
        BEGIN
            ALTER TABLE registry_collections
                ADD CONSTRAINT registry_collections_catalog_fk
                FOREIGN KEY (catalog_internal_id) REFERENCES registry_catalogs(internal_id) NOT VALID;
        EXCEPTION WHEN duplicate_object THEN
            NULL;
        END;
    END IF;
END
$$;
";

const PUBLISH_TENANT_SQL: &str = "\
INSERT INTO registry_tenants (internal_id, external_id, decl, updated_at)
VALUES ($1, $2, $3, now())
ON CONFLICT (internal_id) DO UPDATE SET
    external_id = EXCLUDED.external_id,
    decl = EXCLUDED.decl,
    updated_at = now()";

const PUBLISH_CATALOG_SQL: &str = "\
INSERT INTO registry_catalogs (internal_id, external_id, tenant_internal_id, decl, updated_at)
VALUES ($1, $2, $3, $4, now())
ON CONFLICT (internal_id) DO UPDATE SET
    external_id = EXCLUDED.external_id,
    tenant_internal_id = EXCLUDED.tenant_internal_id,
    decl = EXCLUDED.decl,
    updated_at = now()";

const PUBLISH_COLLECTION_SQL: &str = "\
INSERT INTO registry_collections (internal_id, external_id, catalog_internal_id, decl, updated_at)
VALUES ($1, $2, $3, $4, now())
ON CONFLICT (internal_id) DO UPDATE SET
    external_id = EXCLUDED.external_id,
    catalog_internal_id = EXCLUDED.catalog_internal_id,
    decl = EXCLUDED.decl,
    updated_at = now()";

pub struct CreateTablesArgs {
    pub database_url_env: String,
    /// Print the DDL without connecting to a database at all — for an
    /// operator who wants to hand the SQL to someone else (or a migration
    /// tool) rather than let this CLI apply it directly.
    pub dry_run: bool,
}

pub async fn create_tables(args: CreateTablesArgs) -> anyhow::Result<()> {
    // Always printed, dry run or not: a copy-pasteable statement of exactly
    // what this command does (or would do), matching the design's "emit the
    // plain SQL so an operator can apply it by hand" requirement.
    println!("{CREATE_REGISTRY_TABLES_SQL}");
    if args.dry_run {
        return Ok(());
    }

    let client = crate::db::connect(&args.database_url_env).await?;
    // `#272`: two operators — or an operator and a CI job, or a retry
    // overlapping its predecessor — running this command at the same moment
    // both see "absent" from `IF NOT EXISTS` and the loser fails on
    // `pg_type_typname_nsp_index`, an index nobody typed. The three tables
    // are one lockable unit under one name; see
    // `provision::REGISTRY_TABLES_OBJECT` for why it is a constant.
    crate::provision::apply_ddl(
        &client,
        crate::provision::REGISTRY_TABLES_OBJECT,
        CREATE_REGISTRY_TABLES_SQL,
    )
    .await
    .context("creating registry_tenants/registry_catalogs/registry_collections")?;
    tracing::info!(
        "created (or confirmed existing) registry_tenants/registry_catalogs/registry_collections"
    );
    Ok(())
}

pub struct PublishTenantArgs {
    pub path: PathBuf,
    pub database_url_env: String,
}

pub async fn publish_tenant(args: PublishTenantArgs) -> anyhow::Result<()> {
    let decl = load_decl::<TenantDecl>(&args.path)?;
    let client = crate::db::connect(&args.database_url_env).await?;
    publish_tenant_decl(&client, &decl).await?;
    println!(
        "published tenant '{}' (external_id '{}') into registry_tenants",
        decl.id,
        decl.external_id()
    );
    Ok(())
}

pub struct PublishCatalogArgs {
    pub path: PathBuf,
    pub database_url_env: String,
}

pub async fn publish_catalog(args: PublishCatalogArgs) -> anyhow::Result<()> {
    let decl = load_decl::<CatalogDecl>(&args.path)?;
    let client = crate::db::connect(&args.database_url_env).await?;
    publish_catalog_decl(&client, &decl).await?;
    println!(
        "published catalog '{}' (external_id '{}') into registry_catalogs",
        decl.id,
        decl.external_id()
    );
    Ok(())
}

pub struct PublishCollectionArgs {
    pub path: PathBuf,
    pub database_url_env: String,
}

pub async fn publish_collection(args: PublishCollectionArgs) -> anyhow::Result<()> {
    let decl = load_decl::<CollectionDecl>(&args.path)?;
    let client = crate::db::connect(&args.database_url_env).await?;
    publish_collection_decl(&client, &decl).await?;
    println!(
        "published collection '{}' (external_id '{}') into registry_collections",
        decl.id,
        decl.external_id()
    );
    Ok(())
}

fn load_decl<T: serde::de::DeserializeOwned>(path: &Path) -> anyhow::Result<T> {
    let contents =
        std::fs::read_to_string(path).with_context(|| format!("reading '{}'", path.display()))?;
    serde_yaml::from_str(&contents).with_context(|| format!("parsing '{}'", path.display()))
}

/// Upserts `decl` into `registry_tenants`, keyed by its internal `id` — same
/// idempotent-republish shape [`publish_catalog_decl`] gives
/// `registry_catalogs`, minus a scoping column (a tenant has no owning
/// parent).
pub async fn publish_tenant_decl(client: &Client, decl: &TenantDecl) -> anyhow::Result<()> {
    let value = serde_json::to_value(decl).context("serializing TenantDecl to jsonb")?;
    client
        .execute(PUBLISH_TENANT_SQL, &[&decl.id, &decl.external_id(), &value])
        .await
        .context("upserting registry_tenants")?;
    Ok(())
}

/// Upserts `decl` into `registry_catalogs`, keyed by its internal `id` — a
/// republish of the same `id` (e.g. re-running this command after editing
/// the source YAML) updates the existing row in place rather than
/// duplicating it, matching the "publish is idempotent" expectation the
/// `ON CONFLICT` clause encodes.
pub async fn publish_catalog_decl(client: &Client, decl: &CatalogDecl) -> anyhow::Result<()> {
    let value = serde_json::to_value(decl).context("serializing CatalogDecl to jsonb")?;
    client
        .execute(
            PUBLISH_CATALOG_SQL,
            &[&decl.id, &decl.external_id(), &decl.tenant, &value],
        )
        .await
        .context("upserting registry_catalogs")?;
    Ok(())
}

/// Same as [`publish_catalog_decl`], for `registry_collections`.
pub async fn publish_collection_decl(client: &Client, decl: &CollectionDecl) -> anyhow::Result<()> {
    let value = serde_json::to_value(decl).context("serializing CollectionDecl to jsonb")?;
    client
        .execute(
            PUBLISH_COLLECTION_SQL,
            &[&decl.id, &decl.external_id(), &decl.catalog, &value],
        )
        .await
        .context("upserting registry_collections")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddl_is_idempotent_and_matches_the_reader_side_table_shape() {
        assert!(CREATE_REGISTRY_TABLES_SQL.contains("CREATE TABLE IF NOT EXISTS registry_tenants"));
        assert!(CREATE_REGISTRY_TABLES_SQL.contains("CREATE TABLE IF NOT EXISTS registry_catalogs"));
        assert!(
            CREATE_REGISTRY_TABLES_SQL.contains("CREATE TABLE IF NOT EXISTS registry_collections")
        );
        assert!(CREATE_REGISTRY_TABLES_SQL.contains("external_id text NOT NULL UNIQUE"));
        assert!(CREATE_REGISTRY_TABLES_SQL.contains("UNIQUE (tenant_internal_id, external_id)"));
        assert!(CREATE_REGISTRY_TABLES_SQL.contains("UNIQUE (catalog_internal_id, external_id)"));
        assert!(CREATE_REGISTRY_TABLES_SQL.contains("decl jsonb NOT NULL"));
        assert!(!CREATE_REGISTRY_TABLES_SQL
            .to_uppercase()
            .contains("TRIGGER"));
        assert!(CREATE_REGISTRY_TABLES_SQL.contains(
            "FOREIGN KEY (tenant_internal_id) REFERENCES registry_tenants(internal_id) NOT VALID"
        ));
        assert!(CREATE_REGISTRY_TABLES_SQL.contains(
            "FOREIGN KEY (catalog_internal_id) REFERENCES registry_catalogs(internal_id) NOT VALID"
        ));
    }

    #[test]
    fn publish_sql_upserts_on_the_internal_id_conflict() {
        assert!(PUBLISH_TENANT_SQL.contains("ON CONFLICT (internal_id) DO UPDATE"));
        assert!(PUBLISH_CATALOG_SQL.contains("ON CONFLICT (internal_id) DO UPDATE"));
        assert!(PUBLISH_COLLECTION_SQL.contains("ON CONFLICT (internal_id) DO UPDATE"));
    }

    #[test]
    fn loads_a_tenant_decl_from_yaml() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "tellurion-ingest-registry-test-tenant-{}.yaml",
            std::process::id()
        ));
        std::fs::write(&path, "id: acme-internal\nexternal_id: acme\n").unwrap();

        let decl: TenantDecl = load_decl(&path).unwrap();
        assert_eq!(decl.id, "acme-internal");
        assert_eq!(decl.external_id(), "acme");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn loads_a_catalog_decl_from_yaml() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "tellurion-ingest-registry-test-catalog-{}.yaml",
            std::process::id()
        ));
        std::fs::write(&path, "id: default\ntenant: public\n").unwrap();

        let decl: CatalogDecl = load_decl(&path).unwrap();
        assert_eq!(decl.id, "default");
        assert_eq!(decl.tenant, "public");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn loads_a_collection_decl_from_yaml() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "tellurion-ingest-registry-test-collection-{}.yaml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            "id: demo\ncatalog: default\nstorage: main\ntable: demo\ngeometry: geom\npk: id\n",
        )
        .unwrap();

        let decl: CollectionDecl = load_decl(&path).unwrap();
        assert_eq!(decl.id, "demo");
        assert_eq!(decl.catalog, "default");
        assert_eq!(decl.storage, "main");

        let _ = std::fs::remove_file(path);
    }

    /// Live-database test: creates the tables, publishes a tenant, a catalog
    /// and a collection, republishes the catalog with a changed `external_id`
    /// to prove the upsert updates in place rather than duplicating, and
    /// reads every row back with a plain `SELECT`. Skips gracefully unless
    /// `TELLURION_TEST_DATABASE_URL` is set, matching every other live test
    /// in this workspace. Kept as one test covering all three tables (`#143`
    /// folds the tenant round trip into the existing catalog/collection
    /// proof) rather than a separate live-test file per table.
    ///
    /// Never drops the tables: `tellurion-postgis`'s own live registry tests
    /// (`tests/registry_live.rs`/`tests/tenant_live.rs`) read and write the
    /// same tables against the same test database, and `cargo test
    /// --workspace` can run different crates' test binaries concurrently —
    /// a `DROP TABLE` here would race them. This test's own fixture ids
    /// (`ingest-test-*`) are namespaced away from those files' own prefixes,
    /// and every insert is an upsert (`publish_tenant_decl`/
    /// `publish_catalog_decl`/`publish_collection_decl`'s own `ON
    /// CONFLICT`), so a rerun of the suite is idempotent too.
    ///
    /// `CREATE TABLE IF NOT EXISTS` is **not** safe under concurrent
    /// callers, which is what an earlier version of this comment got wrong
    /// and what `#138` reported: it checks and then inserts the catalog rows
    /// without a lock spanning the two, so the loser of a race fails on
    /// `pg_type_typname_nsp_index` (a `CREATE TABLE` makes a composite type
    /// of the same name). The DDL therefore goes through
    /// `tellurion_postgis::test_harness::apply_fixture_ddl` under
    /// `REGISTRY_TABLES_FIXTURE` — the same database-wide advisory lock the
    /// two `tellurion-postgis` files above take for the same DDL.
    #[tokio::test]
    async fn create_tables_and_publish_round_trip_against_a_live_database() {
        let Some(url) = tellurion_postgis::test_harness::require_database_url(
            "create_tables_and_publish_round_trip_against_a_live_database",
        ) else {
            return;
        };

        let client = crate::db::connect_url(&url)
            .await
            .expect("connect to test database");

        tellurion_postgis::test_harness::apply_fixture_ddl(
            &client,
            tellurion_postgis::test_harness::REGISTRY_TABLES_FIXTURE,
            CREATE_REGISTRY_TABLES_SQL,
        )
        .await
        .expect("create (or confirm existing) the registry tables");

        let tenant = TenantDecl {
            id: "ingest-test-tenant-internal".to_string(),
            external_id: Some("ingest-test-tenant-ext".to_string()),
            settings: tellurion_core::SettingsDecl::default(),
        };
        publish_tenant_decl(&client, &tenant)
            .await
            .expect("publish the tenant");

        let tenant_row = client
            .query_one(
                "SELECT external_id, decl FROM registry_tenants WHERE internal_id = $1",
                &[&tenant.id],
            )
            .await
            .expect("the published tenant row exists");
        let tenant_external_id: String = tenant_row.get(0);
        assert_eq!(tenant_external_id, "ingest-test-tenant-ext");
        let tenant_decl: serde_json::Value = tenant_row.get(1);
        let tenant_round_trip: TenantDecl = serde_json::from_value(tenant_decl).unwrap();
        assert_eq!(tenant_round_trip.id, tenant.id);
        assert_eq!(tenant_round_trip.external_id(), tenant_external_id);

        let catalog = CatalogDecl {
            id: "ingest-test-catalog".to_string(),
            external_id: Some("first-external-id".to_string()),
            tenant: tenant.id.clone(),
            settings: tellurion_core::SettingsDecl::default(),
            visibility: tellurion_core::VisibilityDecl::default(),
        };
        publish_catalog_decl(&client, &catalog)
            .await
            .expect("publish the catalog");

        let collection: CollectionDecl = serde_yaml::from_str(
            "id: ingest-test-collection\ncatalog: ingest-test-catalog\nstorage: main\ntable: demo\ngeometry: geom\npk: id\n",
        )
        .unwrap();
        publish_collection_decl(&client, &collection)
            .await
            .expect("publish the collection");

        let row = client
            .query_one(
                "SELECT external_id FROM registry_catalogs WHERE internal_id = $1",
                &[&catalog.id],
            )
            .await
            .expect("the published catalog row exists");
        let external_id: String = row.get(0);
        assert_eq!(external_id, "first-external-id");

        // Republish with a changed external_id — same internal id, so this
        // must update the existing row in place, never insert a second one.
        let renamed = CatalogDecl {
            external_id: Some("renamed-external-id".to_string()),
            ..catalog.clone()
        };
        publish_catalog_decl(&client, &renamed)
            .await
            .expect("republish the catalog with a new external_id");

        let count: i64 = client
            .query_one(
                "SELECT count(*) FROM registry_catalogs WHERE internal_id = $1",
                &[&catalog.id],
            )
            .await
            .expect("count rows for this internal id")
            .get(0);
        assert_eq!(count, 1, "a republish must update in place, not duplicate");

        let row = client
            .query_one(
                "SELECT external_id FROM registry_catalogs WHERE internal_id = $1",
                &[&catalog.id],
            )
            .await
            .expect("the updated catalog row exists");
        let external_id: String = row.get(0);
        assert_eq!(external_id, "renamed-external-id");

        let collection_row = client
            .query_one(
                "SELECT catalog_internal_id, decl FROM registry_collections WHERE internal_id = $1",
                &[&collection.id],
            )
            .await
            .expect("the published collection row exists");
        let catalog_internal_id: String = collection_row.get(0);
        assert_eq!(catalog_internal_id, "ingest-test-catalog");
        let decl_value: serde_json::Value = collection_row.get(1);
        let round_tripped: CollectionDecl = serde_json::from_value(decl_value)
            .expect("the stored decl jsonb round-trips as a CollectionDecl");
        assert_eq!(round_tripped.id, collection.id);
        assert_eq!(round_tripped.storage, "main");
        assert_eq!(round_tripped.catalog, catalog_internal_id);

        let orphan = CatalogDecl {
            id: "ingest-test-orphan-catalog".to_string(),
            external_id: Some("ingest-test-orphan-catalog".to_string()),
            tenant: "ingest-test-missing-tenant".to_string(),
            settings: Default::default(),
            visibility: Default::default(),
        };
        assert!(
            publish_catalog_decl(&client, &orphan).await.is_err(),
            "the relational schema must reject a catalog whose tenant identity does not exist"
        );
    }
}
