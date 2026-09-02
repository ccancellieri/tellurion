//! Proves the PostGIS driver reads a declaratively range-partitioned table
//! transparently: `#43`. A collection pointed at a partitioned parent (three
//! yearly `RANGE` partitions, a `GiST` index on the parent that PostgreSQL
//! propagates to each partition automatically) serves items, bbox filtering,
//! datetime filtering, and MVT tiles exactly the way it would against a
//! plain table, through the same `Router`/`PostgisDriverFactory` entry
//! points `live.rs` exercises for an unpartitioned one. A separate test
//! proves the point of the exercise: a `datetime`-filtered query prunes to
//! the one matching partition rather than scanning all three, verified with
//! `EXPLAIN` on the same connection style the driver itself uses.
//!
//! Skipped gracefully unless `TELLURION_TEST_DATABASE_URL` is set, matching
//! every other database-backed test in this workspace.

use std::env;
use std::sync::Arc;

use tellurion_core::{
    AppConfig, DatetimeRange, DriverFactory, ItemsQuery, Registry, Router, StorageDecl, TileCoord,
};
use tellurion_postgis::test_harness;
use tellurion_postgis::PostgisDriverFactory;

const URL_ENV_VAR: &str = "TELLURION_POSTGIS_PARTITION_TEST_URL";

/// Three yearly range partitions on `observed_at`, one seeded point in each
/// (`a`/2020, `b`/2021, `c`/2022 — same coordinates/names `live.rs`'s own
/// `seed` fixture uses, so the two are easy to compare by eye). Declarative
/// partitioning requires the partition key to be part of any primary key
/// declared on the parent, hence `PRIMARY KEY (id, observed_at)` rather than
/// `id` alone — this driver only ever reads `id` as the pk column (see
/// `sql.rs`'s crate-level doc on single-column integer keys), and
/// `CATALOG_QUERY` already reports just the first column of a composite key
/// (`catalog.rs`'s own doc comment), so `id` is still what `pk: id` names
/// and what catalog derivation reports.
///
/// The `GiST` index is declared once on the parent; PostgreSQL creates a
/// matching index on every current (and future) partition itself — this is
/// what satisfies "a GiST index per partition" without four separate
/// `CREATE INDEX` statements.
async fn seed_partitioned(database_url: &str, table: &str) {
    let client = test_harness::connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table} CASCADE;
             CREATE TABLE {table} (
                 id bigserial,
                 geom geometry(Point, 4326) NOT NULL,
                 observed_at timestamptz NOT NULL,
                 name text,
                 PRIMARY KEY (id, observed_at)
             ) PARTITION BY RANGE (observed_at);
             CREATE TABLE {table}_2020 PARTITION OF {table}
                 FOR VALUES FROM ('2020-01-01') TO ('2021-01-01');
             CREATE TABLE {table}_2021 PARTITION OF {table}
                 FOR VALUES FROM ('2021-01-01') TO ('2022-01-01');
             CREATE TABLE {table}_2022 PARTITION OF {table}
                 FOR VALUES FROM ('2022-01-01') TO ('2023-01-01');
             CREATE INDEX ON {table} USING GIST (geom);
             INSERT INTO {table} (geom, observed_at, name) VALUES
                 (ST_SetSRID(ST_MakePoint(10, 45), 4326), '2020-06-01T00:00:00Z', 'a'),
                 (ST_SetSRID(ST_MakePoint(11, 46), 4326), '2021-06-01T00:00:00Z', 'b'),
                 (ST_SetSRID(ST_MakePoint(12, 47), 4326), '2022-06-01T00:00:00Z', 'c');
             ANALYZE {table};"
        ),
    )
    .await
    .expect("seeds the partitioned test table");
}

fn app_config(table: &str) -> AppConfig {
    serde_yaml::from_str(&format!(
        "storages: [ {{ id: main, driver: postgis, url_env: {URL_ENV_VAR} }} ]\ntenants: [ {{ id: public }} ]\ncatalogs: [ {{ id: default, tenant: public }} ]\ncollections:\n  - id: {table}\n    catalog: default\n    storage: main\n    table: {table}\n    geometry: geom\n    pk: id\n    datetime: observed_at\n"
    ))
    .expect("valid AppConfig yaml naming the partitioned parent")
}

/// `#43` end to end through the normal routing path: items (unfiltered),
/// bbox filtering, datetime filtering, and MVT tiles all served against a
/// collection whose `table` names the partitioned parent — via
/// `Router::resolve_features`/`resolve_tiles`, exactly as
/// `tellurion-features`/`tellurion-tiles` call it for any other collection.
/// No partitioning-aware code exists anywhere in this driver; this is what
/// "transparent" means in practice.
#[tokio::test]
async fn items_bbox_datetime_and_tiles_serve_transparently_over_a_partitioned_table() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping items_bbox_datetime_and_tiles_serve_transparently_over_a_partitioned_table: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_partition_test_items";
    seed_partitioned(&database_url, table).await;

    // Safety: matches `live.rs`'s own justification — every test in this
    // file sets the same env var to the same value, and no connection pool
    // has spawned worker tasks yet at the point this runs.
    unsafe {
        env::set_var(URL_ENV_VAR, &database_url);
    }

    let config = app_config(table);
    config
        .validate()
        .expect("referential-integrity validation passes for an explicit table override");

    let mut registry = Registry::new();
    registry.register(Arc::new(PostgisDriverFactory::new(60)));
    let router = Router::build(&config, &registry).expect("router builds");
    router
        .validate_catalog()
        .await
        .expect("boot validates the declared table against the partitioned parent's catalog entry");

    let (decl, features) = router
        .resolve_features("public", "default", table)
        .await
        .expect("resolves the features lane");

    let page = features
        .items(&decl, &ItemsQuery::default())
        .await
        .expect("unfiltered items query succeeds against the partitioned parent");
    assert_eq!(
        page.features_geojson.len(),
        3,
        "all three seeded rows, spread across three partitions, come back through the parent"
    );

    let bbox_page = features
        .items(
            &decl,
            &ItemsQuery {
                bbox: Some([9.0, 44.0, 10.5, 45.5]),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("bbox-filtered query succeeds");
    assert_eq!(
        bbox_page.features_geojson.len(),
        1,
        "only the 2020 point ('a') falls in this bbox"
    );
    assert_eq!(bbox_page.features_geojson[0]["properties"]["name"], "a");

    let datetime_page = features
        .items(
            &decl,
            &ItemsQuery {
                datetime: Some(DatetimeRange {
                    start: Some("2021-01-01T00:00:00Z".to_string()),
                    end: Some("2021-12-31T00:00:00Z".to_string()),
                }),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("datetime-filtered query succeeds");
    assert_eq!(
        datetime_page.features_geojson.len(),
        1,
        "only the 2021 point ('b') falls inside the 2021 partition's range"
    );
    assert_eq!(datetime_page.features_geojson[0]["properties"]["name"], "b");

    let (tiles_decl, tiles) = router
        .resolve_tiles("public", "default", table)
        .await
        .expect("resolves the tiles lane");

    let populated_tile = tiles
        .mvt_tile(&tiles_decl, TileCoord { z: 0, x: 0, y: 0 }, None)
        .await
        .expect("mvt query succeeds");
    assert!(
        populated_tile.is_some(),
        "z0 covers the whole world and all three seeded points across all three partitions"
    );

    let empty_tile = tiles
        .mvt_tile(&tiles_decl, TileCoord { z: 15, x: 0, y: 0 }, None)
        .await
        .expect("empty mvt query succeeds");
    assert!(
        empty_tile.is_none(),
        "a high-zoom tile far from any seeded point should be empty"
    );
}

/// `#43` caveat 1 (catalog derivation): `geometry_columns` reports the
/// partitioned parent as its own entry with the same geometry
/// column/srid/type a plain table would — but it *also* reports every
/// partition individually, each as its own physical collection. That is
/// harmless today (no code path auto-registers every `collections()` entry
/// as a served collection; a collection is only ever served if config names
/// its `table` explicitly or derives one from the collection id — see
/// `Router::validate_catalog`), but it does mean an operator must never name
/// a collection after one of its own partition's physical table names, and
/// any future "list what this storage can serve" feature will need to
/// filter partition children out explicitly. Documented in
/// `docs/partitioned-tables.md`.
#[tokio::test]
async fn catalog_source_reports_the_parent_and_each_partition_as_separate_physical_collections() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping catalog_source_reports_the_parent_and_each_partition_as_separate_physical_collections: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_partition_test_catalog";
    seed_partitioned(&database_url, table).await;

    unsafe {
        env::set_var(URL_ENV_VAR, &database_url);
    }

    let factory = PostgisDriverFactory::new(60);
    let decl = StorageDecl {
        id: "main".to_string(),
        driver: "postgis".to_string(),
        url_env: URL_ENV_VAR.to_string(),
        pool_size: None,
    };
    let driver = factory.build(&decl).expect("driver builds");
    let catalog = driver.catalog_source();

    let collections = catalog
        .collections()
        .await
        .expect("catalog introspection succeeds against a partitioned table");

    let parent = collections
        .iter()
        .find(|c| c.name == table)
        .unwrap_or_else(|| panic!("catalog should report the partitioned parent '{table}'"));
    assert_eq!(parent.geometry_column.as_deref(), Some("geom"));
    assert_eq!(parent.primary_key.as_deref(), Some("id"));
    assert_eq!(parent.srid, Some(4326));
    assert_eq!(parent.geometry_type.as_deref(), Some("POINT"));

    for suffix in ["_2020", "_2021", "_2022"] {
        let child_name = format!("{table}{suffix}");
        assert!(
            collections.iter().any(|c| c.name == child_name),
            "each partition surfaces as its own catalog entry too: expected to find '{child_name}'"
        );
    }
}

/// `#43` caveat 2 (extent/stats derivation): `ST_EstimatedExtent` (the fast,
/// statistics-only path `#27` prefers) errors on a partitioned parent —
/// PostGIS reads a `pg_statistic` row for a physical relation, and a
/// partitioned table has no storage of its own to read. `driver.rs`'s
/// existing `Err` fallback (already there for the "never `ANALYZE`d" case,
/// not added for this issue) already covers this: it falls through to the
/// `ST_Extent` real-scan plan, which runs an ordinary `SELECT ... FROM
/// parent` and gets the correct cross-partition bbox via Postgres's own
/// Append. Row-count estimation takes the opposite path: `pg_class.reltuples`
/// on the parent *is* populated (PostgreSQL aggregates partition statistics
/// up to the parent on `ANALYZE`, since PG 14), so `row_estimate` answers
/// directly with no fallback needed.
#[tokio::test]
async fn extent_and_row_estimate_derive_correctly_for_the_partitioned_parent() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping extent_and_row_estimate_derive_correctly_for_the_partitioned_parent: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_partition_test_extent";
    seed_partitioned(&database_url, table).await;

    unsafe {
        env::set_var(URL_ENV_VAR, &database_url);
    }

    let factory = PostgisDriverFactory::new(60);
    let decl = StorageDecl {
        id: "main".to_string(),
        driver: "postgis".to_string(),
        url_env: URL_ENV_VAR.to_string(),
        pool_size: None,
    };
    let driver = factory.build(&decl).expect("driver builds");
    let catalog = driver.catalog_source();

    let physical = catalog
        .collections()
        .await
        .expect("catalog introspection succeeds")
        .into_iter()
        .find(|c| c.name == table)
        .unwrap_or_else(|| panic!("catalog should report the partitioned parent '{table}'"));

    let extent = catalog
        .extent(&physical)
        .await
        .expect("extent query succeeds via the ST_Extent fallback")
        .expect("a non-empty partitioned table has an extent");
    let [minx, miny, maxx, maxy] = extent.bbox;
    assert!((minx - 10.0).abs() < 1e-9, "minx was {minx}");
    assert!((miny - 45.0).abs() < 1e-9, "miny was {miny}");
    assert!((maxx - 12.0).abs() < 1e-9, "maxx was {maxx}");
    assert!((maxy - 47.0).abs() < 1e-9, "maxy was {maxy}");

    let row_estimate = catalog
        .row_estimate(&physical)
        .await
        .expect("row estimate query succeeds against the partitioned parent");
    assert_eq!(
        row_estimate,
        Some(3),
        "pg_class.reltuples on the parent aggregates its partitions' statistics after ANALYZE"
    );

    let temporal_column = catalog
        .temporal_column(&physical)
        .await
        .expect("temporal column query succeeds");
    assert_eq!(temporal_column.as_deref(), Some("observed_at"));
}

/// `#43` end to end for the "routing-only" collection style (`#19`): a
/// collection declaring nothing but `catalog`/`storage`, table/geometry/pk/
/// datetime all derived from the partitioned parent's own catalog entry —
/// same derivation path `live.rs`'s
/// `router_serves_a_collection_configured_with_only_routing_fields` proves
/// for a plain table, run here against a partitioned one.
#[tokio::test]
async fn router_derives_the_physical_shape_of_a_routing_only_collection_over_a_partitioned_parent()
{
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping router_derives_the_physical_shape_of_a_routing_only_collection_over_a_partitioned_parent: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_partition_test_derived";
    seed_partitioned(&database_url, table).await;

    unsafe {
        env::set_var(URL_ENV_VAR, &database_url);
    }

    let config: AppConfig = serde_yaml::from_str(&format!(
        "storages: [ {{ id: main, driver: postgis, url_env: {URL_ENV_VAR} }} ]\n\
         tenants: [ {{ id: public }} ]\n\
         catalogs: [ {{ id: default, tenant: public }} ]\n\
         collections:\n  - id: {table}\n    catalog: default\n    storage: main\n"
    ))
    .expect("valid AppConfig yaml with no physical fields declared");
    config
        .validate()
        .expect("referential-integrity validation never required table/geometry/pk");

    let mut registry = Registry::new();
    registry.register(Arc::new(PostgisDriverFactory::new(60)));
    let router = Router::build(&config, &registry).expect("router builds");
    router
        .validate_catalog()
        .await
        .expect("boot derives table/geometry/pk/datetime from the partitioned parent");

    let (decl, source) = router
        .resolve_features("public", "default", table)
        .await
        .expect("resolves despite the config declaring no physical fields");
    assert_eq!(decl.table.as_deref(), Some(table));
    assert_eq!(decl.geometry.as_deref(), Some("geom"));
    assert_eq!(decl.pk.as_deref(), Some("id"));
    assert_eq!(
        decl.datetime.as_deref(),
        Some("observed_at"),
        "observed_at is the sole timestamptz candidate, same single-candidate derivation rule as an unpartitioned table"
    );

    let page = source
        .items(&decl, &ItemsQuery::default())
        .await
        .expect("items query succeeds against the derived physical shape");
    assert_eq!(
        page.features_geojson.len(),
        3,
        "all three seeded rows come back through the derived decl"
    );
}

/// `#43` deliverable 2, the pruning proof. This SQL text mirrors exactly
/// what `sql::build_items_plan` emits for a datetime-range-only filter (no
/// bbox/token/CQL2) — see that module's `items_plan_with_open_datetime_
/// start_only`/`items_plan_with_all_filters_orders_params_token_bbox_
/// datetime_limit` golden tests for the shape this is copied from. It can't
/// be built by calling the real function: `sql.rs` is a private module, so
/// an integration test (a separate crate, from `cargo`'s point of view) has
/// no way to reach it — this reconstructs the query text by hand instead. A
/// future change to that shape needs a matching update here.
///
/// `#278` left this shape alone: `app_config` above pins `table`,
/// `geometry` and `pk` all three, so this collection takes
/// `Router::effective_decl`'s fully-pinned fast path, derives no descriptor,
/// carries no attribute list, and keeps the whole-row `to_jsonb`
/// projection. A collection whose physical shape *is* derived gets
/// `jsonb_build_object` over the named columns instead — see `sql.rs`'s
/// `properties_expr`. Partition pruning, which is what this test is about,
/// is unaffected either way.
fn items_query_sql(table: &str) -> String {
    format!(
        "SELECT \"id\"::bigint AS pk_value, json_build_object('type','Feature','id',\"id\"::text,'geometry',ST_AsGeoJSON(\"geom\")::json,'properties',to_jsonb(t) - 'geom' - 'id') AS feature FROM \"{table}\" AS t WHERE \"observed_at\" >= $1::text::timestamptz AND \"observed_at\" <= $2::text::timestamptz ORDER BY \"id\"::bigint ASC LIMIT $3"
    )
}

/// `#43` deliverable 2: `EXPLAIN` on the same connection style the driver
/// itself uses (`tokio_postgres`'s extended protocol, `.query()` with bound
/// parameters — never a literal-interpolated string) proves a
/// datetime-filtered query prunes to exactly the one matching partition,
/// while the same query with no datetime filter touches all three. The
/// assertion is version-robust the way the issue asks: it checks which
/// partition table *names* appear as scan targets in the plan text, not a
/// specific "Subplans Removed" wording that could vary across PostgreSQL
/// minor versions — though that phrasing (stable since PG 11's run-time
/// partition pruning) does appear too, and is asserted as a second signal.
#[tokio::test]
async fn datetime_filtered_query_prunes_to_the_matching_partition_only() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping datetime_filtered_query_prunes_to_the_matching_partition_only: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_partition_test_pruning";
    seed_partitioned(&database_url, table).await;

    let client = test_harness::connect(&database_url).await;

    let sql = items_query_sql(table);

    // The functional result first: the query itself returns exactly the
    // 2021 row, independent of whatever plan produces it.
    let rows = client
        .query(
            &sql,
            &[
                &"2021-01-01T00:00:00Z".to_string(),
                &"2021-12-31T00:00:00Z".to_string(),
                &11i64,
            ],
        )
        .await
        .expect("datetime-filtered items query succeeds");
    assert_eq!(rows.len(), 1, "only the 2021 partition's row matches");

    // Now the plan: EXPLAIN (no ANALYZE — this is a plan-shape assertion,
    // not a timing one) of the exact same query, same bound parameters.
    let explain_rows = client
        .query(
            &format!("EXPLAIN {sql}"),
            &[
                &"2021-01-01T00:00:00Z".to_string(),
                &"2021-12-31T00:00:00Z".to_string(),
                &11i64,
            ],
        )
        .await
        .expect("EXPLAIN of the datetime-filtered query succeeds");
    let plan: String = explain_rows
        .iter()
        .map(|row| row.get::<_, String>(0))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        plan.contains(&format!("{table}_2021")),
        "the matching partition must appear as a scan target:\n{plan}"
    );
    assert!(
        !plan.contains(&format!("{table}_2020")),
        "the 2020 partition must be pruned, not scanned:\n{plan}"
    );
    assert!(
        !plan.contains(&format!("{table}_2022")),
        "the 2022 partition must be pruned, not scanned:\n{plan}"
    );
    assert!(
        plan.contains("Subplans Removed"),
        "PostgreSQL's own pruning marker should also be present:\n{plan}"
    );

    // Baseline contrast: the unfiltered query touches all three partitions —
    // proves the absence assertions above aren't vacuously true because the
    // plan never mentions partition names at all.
    let unfiltered_explain = client
        .query(&format!("EXPLAIN SELECT id FROM \"{table}\""), &[])
        .await
        .expect("EXPLAIN of the unfiltered query succeeds");
    let unfiltered_plan: String = unfiltered_explain
        .iter()
        .map(|row| row.get::<_, String>(0))
        .collect::<Vec<_>>()
        .join("\n");
    for suffix in ["_2020", "_2021", "_2022"] {
        assert!(
            unfiltered_plan.contains(&format!("{table}{suffix}")),
            "the unfiltered baseline must touch every partition, '{suffix}' missing:\n{unfiltered_plan}"
        );
    }

    client
        .batch_execute(&format!("DROP TABLE IF EXISTS {table} CASCADE;"))
        .await
        .expect("cleans up the partitioned test table");
}
