//! Live round-trip test for `PostgisTenantReader` (`#143`, relational tenant
//! backend) against a real PostGIS instance — the tenant-side sibling of
//! `tests/registry_live.rs`. Kept to one focused test (point lookup hit/miss
//! plus a keyset paging walk) rather than a full suite, matching that file's
//! own per-query coverage but folded together here since the driver itself
//! is a single, narrow query surface (no scope column to test either side
//! of). Skips gracefully unless `TELLURION_TEST_DATABASE_URL` is set,
//! matching every other live test in this workspace.
//!
//! Table DDL is duplicated here from `tellurion-ingest`'s `registry` module
//! rather than shared — the two crates deliberately don't depend on each
//! other (see that module's own doc comment for why); this file is the
//! `tellurion-postgis` side's own proof that its queries match what that DDL
//! actually creates.

use std::env;

use tellurion_core::{PageRequest, TenantDecl, TenantReader};
use tellurion_postgis::test_harness;
use tellurion_postgis::PostgisTenantReader;

const CREATE_TENANTS_TABLE_SQL: &str = "\
CREATE TABLE IF NOT EXISTS registry_tenants (
    internal_id text PRIMARY KEY,
    external_id text NOT NULL UNIQUE,
    decl jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
";

async fn connect_raw(database_url: &str) -> tokio_postgres::Client {
    test_harness::connect(database_url).await
}

/// Creates the table if it doesn't already exist and upserts fixtures by
/// `internal_id` — never `DROP`s, since `tellurion-ingest`'s own live test
/// shares this same table against the same test database (see
/// `registry_live.rs`'s own `seed` doc for the full rationale). This test's
/// own fixture ids (`tenant-live-*`) are namespaced away from every other
/// live test's prefixes.
///
/// The DDL goes through [`test_harness::apply_fixture_ddl`] under
/// [`test_harness::REGISTRY_TABLES_FIXTURE`] — the same lock name
/// `registry_live.rs` and `tellurion-ingest`'s `registry` test take, which
/// is what actually keeps the three of them from racing each other's
/// `CREATE TABLE IF NOT EXISTS` (`#138`; `IF NOT EXISTS` alone does not).
async fn seed(client: &tokio_postgres::Client, prefix: &str, count: usize) {
    test_harness::apply_fixture_ddl(
        client,
        test_harness::REGISTRY_TABLES_FIXTURE,
        CREATE_TENANTS_TABLE_SQL,
    )
    .await
    .expect("create (or confirm existing) registry_tenants");

    for i in 0..count {
        let id = format!("{prefix}-{i}");
        let external_id = format!("{prefix}-ext-{i:03}");
        let decl = TenantDecl {
            id: id.clone(),
            external_id: Some(external_id.clone()),
            settings: tellurion_core::SettingsDecl::default(),
        };
        let value = serde_json::to_value(&decl).unwrap();
        client
            .execute(
                "INSERT INTO registry_tenants (internal_id, external_id, decl) \
                 VALUES ($1, $2, $3) \
                 ON CONFLICT (internal_id) DO UPDATE SET \
                     external_id = EXCLUDED.external_id, \
                     decl = EXCLUDED.decl",
                &[&id, &external_id, &value],
            )
            .await
            .expect("upsert seeded tenant");
    }
}

#[tokio::test]
async fn point_lookup_and_keyset_paging_against_a_live_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping point_lookup_and_keyset_paging_against_a_live_database: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let raw = connect_raw(&database_url).await;
    seed(&raw, "tenant-live", 3).await;

    let reader = PostgisTenantReader::connect(&database_url, 60_000)
        .await
        .expect("connects");

    let hit = reader
        .tenant("tenant-live-ext-000")
        .await
        .expect("tenant query succeeds")
        .expect("the seeded tenant is found");
    assert_eq!(hit.id, "tenant-live-0");

    let miss = reader
        .tenant("nonexistent-external-id")
        .await
        .expect("tenant query succeeds even for an unknown external id");
    assert!(miss.is_none());

    let mut collected = Vec::new();
    let mut after: Option<String> = None;
    let mut pages = 0;
    loop {
        let page = reader
            .list_tenants(PageRequest {
                limit: 2,
                after: after.clone(),
            })
            .await
            .expect("list_tenants succeeds");
        pages += 1;
        assert!(
            page.items.len() <= 2,
            "a page must never exceed the requested limit"
        );
        collected.extend(
            page.items
                .into_iter()
                .map(|t| t.id)
                .filter(|id| id.starts_with("tenant-live-")),
        );
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
        assert!(pages <= 20, "runaway pagination loop");
    }

    assert_eq!(
        collected,
        vec!["tenant-live-0", "tenant-live-1", "tenant-live-2"]
    );
}
