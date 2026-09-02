//! Live round-trip tests against a real PostGIS instance: seed a temp table,
//! exercise `items` (paging + count), `item`, and `mvt_tile` through the
//! actual `PostgisDriverFactory` — the same entry point production code
//! uses. Skipped gracefully unless `TELLURION_TEST_DATABASE_URL` is set, so
//! `cargo test` never needs a database by default.

use std::env;
use std::sync::Arc;

use tellurion_core::{
    AppConfig, CaseInsensitiveCompareOp, CollectionDecl, CompareOp, DriverFactory, Error, Filter,
    GeometryLiteral, ItemsQuery, Literal, Registry, RequestedCrs, Router, SpatialOp, StorageDecl,
    TemporalOp, TemporalValue, TileCoord, WktGeometry,
};
use tellurion_postgis::test_harness;
use tellurion_postgis::PostgisDriverFactory;

const URL_ENV_VAR: &str = "TELLURION_POSTGIS_LIVE_TEST_URL";

/// Bbox-assertion tolerance (degrees) wide enough to absorb
/// `ST_EstimatedExtent`'s `float4`-precision statistics — see
/// `assert_bbox_matches_seeded_points`.
const ESTIMATE_TOLERANCE_DEG: f64 = 0.1;

async fn seed(database_url: &str, table: &str) {
    let client = test_harness::connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326) NOT NULL,
                 observed_at timestamptz,
                 name text
             );
             INSERT INTO {table} (geom, observed_at, name) VALUES
                 (ST_SetSRID(ST_MakePoint(10, 45), 4326), '2020-01-01T00:00:00Z', 'a'),
                 (ST_SetSRID(ST_MakePoint(11, 46), 4326), '2020-06-01T00:00:00Z', 'b'),
                 (ST_SetSRID(ST_MakePoint(12, 47), 4326), '2021-01-01T00:00:00Z', 'c');
             ANALYZE {table};"
        ),
    )
    .await
    .expect("seeds the test table");
}

/// Same fixture as [`seed`] plus an integer and a boolean attribute column —
/// the pair of `Literal` shapes (`Number`, `Bool`) `tellurion-features`'
/// queryable-query-parameter coercion (`#52`) produces that [`seed`]'s
/// `name`-only (`Literal::Text`) table can't exercise; `cql2_filter_narrows_
/// the_result_set_against_a_real_database` already covers `Literal::Text`
/// end to end.
async fn seed_with_typed_attributes(database_url: &str, table: &str) {
    let client = test_harness::connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326) NOT NULL,
                 name text,
                 population integer,
                 active boolean
             );
             INSERT INTO {table} (geom, name, population, active) VALUES
                 (ST_SetSRID(ST_MakePoint(10, 45), 4326), 'a', 10, true),
                 (ST_SetSRID(ST_MakePoint(11, 46), 4326), 'b', 20, true),
                 (ST_SetSRID(ST_MakePoint(12, 47), 4326), 'c', 20, false);
             ANALYZE {table};"
        ),
    )
    .await
    .expect("seeds the typed-attribute test table");
}

/// Same fixture as [`seed`] but deliberately skips `ANALYZE`, so
/// `ST_EstimatedExtent` has no `pg_statistic` row to read from — exercises
/// the `ST_Extent` fallback (`#27`).
async fn seed_without_analyze(database_url: &str, table: &str) {
    let client = test_harness::connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326) NOT NULL
             );
             INSERT INTO {table} (geom) VALUES
                 (ST_SetSRID(ST_MakePoint(10, 45), 4326)),
                 (ST_SetSRID(ST_MakePoint(11, 46), 4326)),
                 (ST_SetSRID(ST_MakePoint(12, 47), 4326));"
        ),
    )
    .await
    .expect("seeds the un-analyzed test table");
}

/// An empty (but `ANALYZE`d) table: `ST_EstimatedExtent` has statistics to
/// read, but there is nothing in them — exercises the "empty collection
/// keeps a null extent without erroring" acceptance criterion (`#27`).
async fn seed_empty(database_url: &str, table: &str) {
    let client = test_harness::connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326) NOT NULL
             );
             ANALYZE {table};"
        ),
    )
    .await
    .expect("creates the empty test table");
}

/// A table whose geometry column's native SRID is 3857 (Web Mercator),
/// unlike every other fixture in this file (all SRID 4326) — the only way to
/// prove `crs=CRS84` triggers a genuine `ST_Transform` reprojection rather
/// than a no-op, since a 4326-native table's default output is already
/// CRS84-shaped. Each point is seeded by transforming a known lon/lat pair
/// into 3857, so the CRS84-reprojected output can be asserted back against
/// the original degrees.
async fn seed_3857(database_url: &str, table: &str) {
    let client = test_harness::connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 3857) NOT NULL,
                 name text
             );
             INSERT INTO {table} (geom, name) VALUES
                 (ST_Transform(ST_SetSRID(ST_MakePoint(10, 45), 4326), 3857), 'a');
             ANALYZE {table};"
        ),
    )
    .await
    .expect("seeds the 3857 test table");
}

/// A 3857 table seeded so that the two readings of one degree bbox select
/// **different, both non-empty** row sets (`#255`) — what [`seed_3857`]'s
/// single row cannot provide, and the only kind of fixture that can tell
/// "right rows" from "wrong rows" rather than merely "something changed".
///
/// Two points, stored in EPSG:3857 metres, seeded from known lon/lat:
///
/// - `in_box` at 10°E 10°N — roughly (1113195, 1118890) m.
/// - `near_origin` at 0.0001°E 0.0001°N — roughly (11.1, 11.1) m, chosen
///   because a point that close to the origin has *metre* coordinates whose
///   numeric values land inside a plausible range of *degrees*.
///
/// Against the bbox `9, 9, 12, 12` read as CRS84 degrees — the only legal
/// reading of a `bbox` carrying no `bbox-crs`:
///
/// - transformed into 3857 the box spans x 1001875.4..1335833.9,
///   y 1006021.1..1345708.4, so it contains `in_box` and excludes
///   `near_origin`;
/// - taken as raw numbers beside a metre column, which is what this crate did
///   before `#255`, the box 9..12 contains `near_origin`'s (11.1, 11.1) and
///   excludes `in_box`.
///
/// The two readings therefore return exactly one row each, and never the same
/// one. That is what makes the assertion decisive: neither a `200` nor a
/// non-empty page is evidence of anything on its own.
async fn seed_3857_disjoint_pair(database_url: &str, table: &str) {
    let client = test_harness::connect(database_url).await;
    test_harness::apply_fixture_ddl(&client, table, &format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 3857) NOT NULL,
                 name text
             );
             INSERT INTO {table} (geom, name) VALUES
                 (ST_Transform(ST_SetSRID(ST_MakePoint(10, 10), 4326), 3857), 'in_box'),
                 (ST_Transform(ST_SetSRID(ST_MakePoint(0.0001, 0.0001), 4326), 3857), 'near_origin');
             ANALYZE {table};"
        ))
        .await
        .expect("seeds the disjoint-pair 3857 test table");
}

fn collection(table: &str) -> CollectionDecl {
    serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\ndatetime: observed_at\n"
    ))
    .expect("valid CollectionDecl yaml")
}

/// A `uuid`-pk table (`#87`): five rows, seeded with explicit, ascending
/// literal ids (`...0001` through `...0005`) rather than `gen_random_uuid()`
/// — a keyset-paging stability test needs to know the exact total order in
/// advance, and canonical lowercase-hyphenated UUID text compares
/// byte-for-byte identically to the `uuid` type's own binary ordering (every
/// byte maps to exactly two hex characters at a fixed position, hyphens sit
/// at fixed positions too), so these literals sort the same way in SQL
/// (`ORDER BY id::uuid`) as they do as plain strings here.
async fn seed_uuid(database_url: &str, table: &str) {
    let client = test_harness::connect(database_url).await;
    test_harness::apply_fixture_ddl(&client, table, &format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id uuid PRIMARY KEY,
                 geom geometry(Point, 4326) NOT NULL,
                 name text
             );
             INSERT INTO {table} (id, geom, name) VALUES
                 ('00000000-0000-0000-0000-000000000001', ST_SetSRID(ST_MakePoint(10, 45), 4326), 'a'),
                 ('00000000-0000-0000-0000-000000000002', ST_SetSRID(ST_MakePoint(11, 46), 4326), 'b'),
                 ('00000000-0000-0000-0000-000000000003', ST_SetSRID(ST_MakePoint(12, 47), 4326), 'c'),
                 ('00000000-0000-0000-0000-000000000004', ST_SetSRID(ST_MakePoint(13, 48), 4326), 'd'),
                 ('00000000-0000-0000-0000-000000000005', ST_SetSRID(ST_MakePoint(14, 49), 4326), 'e');
             ANALYZE {table};"
        ))
        .await
        .expect("seeds the uuid-pk test table");
}

/// Same shape as [`collection`], with `id_type: uuid` declared (`#87`).
fn collection_uuid(table: &str) -> CollectionDecl {
    let mut decl = collection(table);
    decl.id_type = tellurion_core::IdType::Uuid;
    decl
}

/// A `text`-pk table (`#94`): five rows whose ids are deliberately chosen so
/// byte order (`COLLATE "C"`, what `build_items_plan` pins) and a typical
/// locale-aware collation (alphabetic, broadly case-insensitive — what a
/// database's own default collation would give without the pin) disagree.
/// Byte order compares the leading character's own codepoint: `'B'` (0x42)
/// `< 'Z'` (0x5A) `< 'a'` (0x61) `< 'm'` (0x6D) `< 'z'` (0x7A), so `COLLATE
/// "C"` orders these `Banana, Zebra, apple, mango, zoo` — the two capitalized
/// ids sort first, ahead of every lowercase one, regardless of what the
/// words themselves spell. A locale-aware alphabetic collation would instead
/// interleave them by spelling (`apple, Banana, mango, Zebra, zoo`). If
/// keyset paging ever lost its `COLLATE "C"` pin, this fixture is the one
/// most likely to catch it: the two orderings share no adjacent pair, so any
/// reversion produces a visibly different (and just as deterministic, so not
/// a flake) sequence, not a coincidentally-matching one.
async fn seed_text(database_url: &str, table: &str) {
    let client = test_harness::connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id text PRIMARY KEY,
                 geom geometry(Point, 4326) NOT NULL,
                 name text
             );
             INSERT INTO {table} (id, geom, name) VALUES
                 ('Banana', ST_SetSRID(ST_MakePoint(10, 45), 4326), 'a'),
                 ('Zebra', ST_SetSRID(ST_MakePoint(11, 46), 4326), 'b'),
                 ('apple', ST_SetSRID(ST_MakePoint(12, 47), 4326), 'c'),
                 ('mango', ST_SetSRID(ST_MakePoint(13, 48), 4326), 'd'),
                 ('zoo', ST_SetSRID(ST_MakePoint(14, 49), 4326), 'e');
             ANALYZE {table};"
        ),
    )
    .await
    .expect("seeds the text-pk test table");
}

/// Same shape as [`collection`], with `id_type: text` declared (`#94`).
fn collection_text(table: &str) -> CollectionDecl {
    let mut decl = collection(table);
    decl.id_type = tellurion_core::IdType::Text;
    decl
}

#[tokio::test]
async fn items_and_mvt_round_trip_against_a_real_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping items_and_mvt_round_trip_against_a_real_database: TELLURION_TEST_DATABASE_URL not set");
        return;
    };

    let table = "tellurion_postgis_live_test_items";
    seed(&database_url, table).await;

    // Safety: this test binary is single-threaded with respect to env var
    // access at this point (no other test in this file reads/writes env),
    // and this runs before any connection pool spawns worker tasks.
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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    let tiles = driver.tile_source().expect("driver exposes TileSource");

    let collection = collection(table);

    let first_page = features
        .items(
            &collection,
            &ItemsQuery {
                limit: 2,
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("first page succeeds");
    assert_eq!(first_page.features_geojson.len(), 2);
    assert_eq!(first_page.number_matched, Some(3));
    let next_token = first_page
        .next_token
        .clone()
        .expect("a next page exists after 2 of 3 rows");

    let second_page = features
        .items(
            &collection,
            &ItemsQuery {
                limit: 2,
                token: Some(next_token),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("second page succeeds");
    assert_eq!(second_page.features_geojson.len(), 1);
    assert!(second_page.next_token.is_none(), "no third page exists");

    let first_id = first_page.features_geojson[0]["id"]
        .as_str()
        .expect("feature id is a string")
        .to_string();
    let item = features
        .item(&collection, &first_id, None)
        .await
        .expect("item query succeeds");
    assert!(item.is_some());
    assert_eq!(item.unwrap()["id"], first_id);

    let missing = features
        .item(&collection, "999999", None)
        .await
        .expect("missing-item query succeeds without erroring");
    assert!(missing.is_none());

    let non_numeric = features
        .item(&collection, "not-a-number", None)
        .await
        .expect("non-numeric id query succeeds without erroring");
    assert!(non_numeric.is_none());

    let populated_tile = tiles
        .mvt_tile(&collection, TileCoord { z: 0, x: 0, y: 0 }, None)
        .await
        .expect("mvt query succeeds");
    assert!(
        populated_tile.is_some(),
        "z0 covers the whole world and all three seeded points"
    );

    let empty_tile = tiles
        .mvt_tile(&collection, TileCoord { z: 15, x: 0, y: 0 }, None)
        .await
        .expect("empty mvt query succeeds");
    assert!(
        empty_tile.is_none(),
        "a high-zoom tile far from any seeded point should be empty"
    );
}

/// `#87`: keyset paging over a `uuid` pk stays correct and complete across
/// pages — ordering happens over the pk's own real type (`ORDER BY
/// id::uuid`), not its string form pretending to be a number. Walks five
/// rows two at a time (three pages) and proves every row is returned
/// exactly once, in the pk's own ascending order, with the last page's
/// `next_token` absent. Also proves `item()` round-trips a real uuid id and
/// answers `None` (not an error) for a syntactically invalid one, mirroring
/// `items_and_mvt_round_trip_against_a_real_database`'s own `non_numeric`
/// case for the `Integer` id-type.
#[tokio::test]
async fn keyset_paging_over_a_uuid_primary_key_is_stable_and_complete_against_a_real_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping keyset_paging_over_a_uuid_primary_key_is_stable_and_complete_against_a_real_database: TELLURION_TEST_DATABASE_URL not set");
        return;
    };

    let table = "tellurion_postgis_live_test_uuid_paging";
    seed_uuid(&database_url, table).await;

    // Safety: same argument as `items_and_mvt_round_trip_against_a_real_
    // database` — single-threaded env var access at this point, before any
    // connection pool spawns worker tasks.
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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    let tiles = driver.tile_source().expect("driver exposes TileSource");

    let collection = collection_uuid(table);

    let expected_ids: Vec<String> = (1..=5)
        .map(|n| format!("00000000-0000-0000-0000-{n:012}"))
        .collect();

    let mut seen_ids = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let page = features
            .items(
                &collection,
                &ItemsQuery {
                    limit: 2,
                    token: token.clone(),
                    ..ItemsQuery::default()
                },
            )
            .await
            .expect("page query succeeds");
        pages += 1;
        assert!(pages <= 4, "must not loop past the expected 3 pages");
        for feature in &page.features_geojson {
            seen_ids.push(
                feature["id"]
                    .as_str()
                    .expect("feature id is a string")
                    .to_string(),
            );
        }
        match page.next_token {
            Some(next) => token = Some(next),
            None => break,
        }
    }

    assert_eq!(pages, 3, "5 rows at 2 per page must take exactly 3 pages");
    assert_eq!(
        seen_ids, expected_ids,
        "every row must be seen exactly once, in the pk's own ascending order"
    );

    let item = features
        .item(&collection, &expected_ids[0], None)
        .await
        .expect("item query succeeds");
    assert_eq!(item.unwrap()["id"], expected_ids[0]);

    let not_a_uuid = features
        .item(&collection, "not-a-uuid", None)
        .await
        .expect("a syntactically invalid id must not error");
    assert!(not_a_uuid.is_none());

    // `#87`: the tile lane needs no special handling for a uuid pk
    // (`sql.rs`'s `build_mvt_plan` doc/`mvt_plan_is_identical_regardless_
    // of_id_type`) — a real fetch over this uuid-pk collection should
    // succeed and carry the seeded points, exactly like the `Integer`
    // fixture's own tile assertion.
    let populated_tile = tiles
        .mvt_tile(&collection, TileCoord { z: 0, x: 0, y: 0 }, None)
        .await
        .expect("mvt query succeeds against a uuid-pk collection");
    assert!(
        populated_tile.is_some(),
        "z0 covers the whole world and all five seeded points"
    );
}

/// `#94`: keyset paging over a `text` pk pins an explicit `COLLATE "C"`, so
/// ordering is stable and complete regardless of the database's own default
/// collation — proven with adversarial ids (`seed_text`'s own doc) that sort
/// differently under byte order than under a typical locale-aware
/// collation. Walks five rows two at a time (three pages) and proves every
/// row is returned exactly once, in exactly the `COLLATE "C"` byte order,
/// with the last page's `next_token` absent — the `text` counterpart of
/// `keyset_paging_over_a_uuid_primary_key_is_stable_and_complete_against_a_
/// real_database`.
#[tokio::test]
async fn keyset_paging_over_a_text_primary_key_is_stable_and_complete_with_adversarial_ids() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping keyset_paging_over_a_text_primary_key_is_stable_and_complete_with_adversarial_ids: TELLURION_TEST_DATABASE_URL not set");
        return;
    };

    let table = "tellurion_postgis_live_test_text_paging";
    seed_text(&database_url, table).await;

    // Safety: same argument as `items_and_mvt_round_trip_against_a_real_
    // database` — single-threaded env var access at this point, before any
    // connection pool spawns worker tasks.
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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    let tiles = driver.tile_source().expect("driver exposes TileSource");

    let collection = collection_text(table);

    // `COLLATE "C"` byte order — `seed_text`'s own doc derives this from
    // each leading character's codepoint. A locale-aware collation would
    // instead give `apple, Banana, mango, Zebra, zoo`, a different sequence
    // entirely, so this expectation only holds if the pin is real.
    let expected_ids: Vec<String> = ["Banana", "Zebra", "apple", "mango", "zoo"]
        .into_iter()
        .map(str::to_string)
        .collect();

    let mut seen_ids = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let page = features
            .items(
                &collection,
                &ItemsQuery {
                    limit: 2,
                    token: token.clone(),
                    ..ItemsQuery::default()
                },
            )
            .await
            .expect("page query succeeds");
        pages += 1;
        assert!(pages <= 4, "must not loop past the expected 3 pages");
        for feature in &page.features_geojson {
            seen_ids.push(
                feature["id"]
                    .as_str()
                    .expect("feature id is a string")
                    .to_string(),
            );
        }
        match page.next_token {
            Some(next) => token = Some(next),
            None => break,
        }
    }

    assert_eq!(pages, 3, "5 rows at 2 per page must take exactly 3 pages");
    assert_eq!(
        seen_ids, expected_ids,
        "every row must be seen exactly once, in COLLATE \"C\" byte order"
    );

    let item = features
        .item(&collection, &expected_ids[0], None)
        .await
        .expect("item query succeeds");
    assert_eq!(item.unwrap()["id"], expected_ids[0]);

    let missing = features
        .item(&collection, "no-such-id", None)
        .await
        .expect("a valid-but-absent text id must not error");
    assert!(missing.is_none());

    // `#94`: the tile lane needs no special handling for a text pk either
    // (`sql.rs`'s `build_mvt_plan` doc/`mvt_plan_is_identical_regardless_
    // of_id_type`) — a real fetch over this text-pk collection should
    // succeed and carry the seeded points, exactly like the `Integer`/`Uuid`
    // fixtures' own tile assertions.
    let populated_tile = tiles
        .mvt_tile(&collection, TileCoord { z: 0, x: 0, y: 0 }, None)
        .await
        .expect("mvt query succeeds against a text-pk collection");
    assert!(
        populated_tile.is_some(),
        "z0 covers the whole world and all five seeded points"
    );
}

/// `#34`: proves the ABAC grant filter actually reaches the SQL PostGIS
/// runs for both the single-item lookup and the MVT tile query — not just
/// that `sql.rs` compiles the right text (`tellurion-postgis::sql`'s own
/// unit tests already cover that), but that a real database applies it and
/// excludes the rows it says it will.
#[tokio::test]
async fn cql2_filter_narrows_the_item_lookup_and_mvt_tile_against_a_real_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping cql2_filter_narrows_the_item_lookup_and_mvt_tile_against_a_real_database: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_abac_filter";
    seed(&database_url, table).await;

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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    let tiles = driver.tile_source().expect("driver exposes TileSource");
    assert!(
        tiles.filter_capable(),
        "the postgis driver must advertise ABAC filter capability on the tile lane"
    );
    let collection = collection(table);

    // -- single-item lookup --------------------------------------------------

    let all = features
        .items(&collection, &ItemsQuery::default())
        .await
        .expect("unfiltered listing succeeds");
    let point_a_id = all
        .features_geojson
        .iter()
        .find(|f| f["properties"]["name"] == "a")
        .and_then(|f| f["id"].as_str())
        .expect("seeded point 'a' is present")
        .to_string();

    let matching = tellurion_core::filter::parse_text("name = 'a'").unwrap();
    let item_allowed = features
        .item(&collection, &point_a_id, Some(&matching))
        .await
        .expect("filtered item query succeeds");
    assert!(
        item_allowed.is_some(),
        "a filter that matches this row must still return it"
    );

    let excluding = tellurion_core::filter::parse_text("name = 'b'").unwrap();
    let item_excluded = features
        .item(&collection, &point_a_id, Some(&excluding))
        .await
        .expect("excluding item query succeeds without erroring");
    assert!(
        item_excluded.is_none(),
        "a row the filter excludes must come back None, indistinguishable from a genuinely absent id"
    );

    // -- MVT tile --------------------------------------------------------------

    let coord = TileCoord { z: 0, x: 0, y: 0 };
    let unfiltered_tile = tiles
        .mvt_tile(&collection, coord, None)
        .await
        .expect("unfiltered mvt query succeeds")
        .expect("z0 covers the whole world and all three seeded points");

    let one_row_filter = tellurion_core::filter::parse_text("name = 'a'").unwrap();
    let filtered_tile = tiles
        .mvt_tile(&collection, coord, Some(&one_row_filter))
        .await
        .expect("filtered mvt query succeeds")
        .expect("one seeded row still matches this filter");
    assert!(
        filtered_tile.len() < unfiltered_tile.len(),
        "a tile filtered down to one of three points must be smaller than the unfiltered tile \
         ({} filtered bytes vs {} unfiltered)",
        filtered_tile.len(),
        unfiltered_tile.len()
    );

    let no_row_filter = tellurion_core::filter::parse_text("name = 'does-not-exist'").unwrap();
    let empty_tile = tiles
        .mvt_tile(&collection, coord, Some(&no_row_filter))
        .await
        .expect("excluding-everything mvt query succeeds without erroring");
    assert!(
        empty_tile.is_none(),
        "a filter matching no rows must produce an empty tile, not the unfiltered one"
    );
}

#[tokio::test]
async fn bbox_and_datetime_filters_narrow_the_result_set() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping bbox_and_datetime_filters_narrow_the_result_set: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_filters";
    seed(&database_url, table).await;

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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    let collection = collection(table);

    let bbox_page = features
        .items(
            &collection,
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
        "only the first point falls in this bbox"
    );
    assert!(
        bbox_page.number_matched.is_none(),
        "a filtered query must never report a cheap unfiltered estimate"
    );

    let datetime_page = features
        .items(
            &collection,
            &ItemsQuery {
                datetime: Some(tellurion_core::DatetimeRange {
                    start: Some("2020-12-01T00:00:00Z".to_string()),
                    end: None,
                }),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("datetime-filtered query succeeds");
    assert_eq!(
        datetime_page.features_geojson.len(),
        1,
        "only the last point is after 2020-12-01"
    );
}

/// `#33` end to end against a real database: a CQL2 attribute-comparison
/// filter, an `S_INTERSECTS` bbox filter, and a `T_AFTER` temporal filter
/// each narrow the seeded three-row table through the real
/// `PostgisDriverFactory` entry point, exactly like `items_and_mvt_round_trip_
/// against_a_real_database` does for bbox/datetime/paging.
#[tokio::test]
async fn cql2_filter_narrows_the_result_set_against_a_real_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping cql2_filter_narrows_the_result_set_against_a_real_database: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_cql2_filter";
    seed(&database_url, table).await;

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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    assert!(
        features.filter_capable(),
        "the postgis driver must advertise CQL2 filter capability"
    );
    let collection = collection(table);

    // Attribute comparison, parsed from CQL2-text through the same parser
    // the features handler uses, then compiled to bound-parameter SQL.
    let name_filter = tellurion_core::filter::parse_text("name = 'b'").unwrap();
    let name_page = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(name_filter),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("attribute-filtered query succeeds");
    assert_eq!(name_page.features_geojson.len(), 1);
    assert_eq!(name_page.features_geojson[0]["properties"]["name"], "b");
    assert!(
        name_page.number_matched.is_none(),
        "a filtered query must never report a cheap unfiltered estimate"
    );

    // S_INTERSECTS with a bbox literal covering only the first seeded point.
    let intersects_filter =
        tellurion_core::filter::parse_text("S_INTERSECTS(geom, BBOX(9, 44, 10.5, 45.5))").unwrap();
    let intersects_page = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(intersects_filter),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("s_intersects-filtered query succeeds");
    assert_eq!(intersects_page.features_geojson.len(), 1);
    assert_eq!(
        intersects_page.features_geojson[0]["properties"]["name"],
        "a"
    );

    // T_AFTER on the datetime column: only the last seeded point qualifies.
    let after_filter =
        tellurion_core::filter::parse_text("T_AFTER(observed_at, '2020-12-01T00:00:00Z')").unwrap();
    let after_page = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(after_filter),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("t_after-filtered query succeeds");
    assert_eq!(after_page.features_geojson.len(), 1);
    assert_eq!(after_page.features_geojson[0]["properties"]["name"], "c");

    // A filter combined with an ordinary bbox parameter, AND'd together: the
    // bbox alone covers 'b' (11, 46) and 'c' (12, 47) but not 'a' (10, 45);
    // `name <> 'b'` then narrows that pair down to 'c' alone.
    let combined_filter = tellurion_core::filter::parse_text("name <> 'b'").unwrap();
    let combined_page = features
        .items(
            &collection,
            &ItemsQuery {
                bbox: Some([10.5, 45.5, 13.0, 48.0]),
                filter: Some(combined_filter),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("combined bbox+filter query succeeds");
    assert_eq!(
        combined_page.features_geojson.len(),
        1,
        "only 'c' is both inside the bbox and matches name <> 'b'"
    );
    assert_eq!(combined_page.features_geojson[0]["properties"]["name"], "c");
}

/// `#52` end to end against a real database: hand-built `Filter::Compare`
/// nodes carrying `Literal::Number`/`Literal::Bool` — exactly the shape
/// `tellurion-features`' queryable-query-parameter coercion produces for a
/// bare `?population=20`/`?active=false` request
/// (`params::coerce_queryable_value`) — round-trip through the real
/// `PostgisDriverFactory` entry point, individually and ANDed together
/// (the same `Filter::And` shape `params::build_queryable_filter` builds
/// for `?population=20&active=false`), proving this crate's `compile_filter`
/// `::double precision`/`::boolean` column casts actually implement OGC API
/// Features Part 3's "Queryables as Query Parameters" equality semantics
/// against real integer/boolean columns.
/// `cql2_filter_narrows_the_result_set_against_a_real_database` already
/// covers the `Literal::Text` case the same way.
#[tokio::test]
async fn queryable_equality_predicates_narrow_the_result_set_against_a_real_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping queryable_equality_predicates_narrow_the_result_set_against_a_real_database: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_queryable_params";
    seed_with_typed_attributes(&database_url, table).await;

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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    let collection: CollectionDecl = serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\n"
    ))
    .expect("valid CollectionDecl yaml");

    // A single integer-typed equality predicate: population = 20 matches
    // seeded rows 'b' and 'c'.
    let population_filter = Filter::Compare {
        property: "population".to_string(),
        op: CompareOp::Eq,
        value: Literal::Number(20.0),
    };
    let population_page = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(population_filter.clone()),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("population-filtered query succeeds");
    assert_eq!(population_page.features_geojson.len(), 2);

    // A single boolean-typed equality predicate: active = false matches
    // only seeded row 'c'.
    let active_filter = Filter::Compare {
        property: "active".to_string(),
        op: CompareOp::Eq,
        value: Literal::Bool(false),
    };
    let active_page = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(active_filter.clone()),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("active-filtered query succeeds");
    assert_eq!(active_page.features_geojson.len(), 1);
    assert_eq!(active_page.features_geojson[0]["properties"]["name"], "c");

    // ANDed together: only 'c' satisfies both (population = 20 AND
    // active = false) — 'b' also has population = 20 but active = true.
    let combined_filter = Filter::And(vec![population_filter, active_filter]);
    let combined_page = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(combined_filter),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("combined queryable-equality query succeeds");
    assert_eq!(combined_page.features_geojson.len(), 1);
    assert_eq!(combined_page.features_geojson[0]["properties"]["name"], "c");
}

#[tokio::test]
async fn catalog_source_reports_the_seeded_table_s_physical_shape() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping catalog_source_reports_the_seeded_table_s_physical_shape: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_catalog";
    seed(&database_url, table).await;

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
        .expect("catalog introspection succeeds");
    let seeded = collections
        .iter()
        .find(|c| c.name == table)
        .unwrap_or_else(|| panic!("catalog should report the seeded table '{table}'"));

    assert_eq!(seeded.geometry_column.as_deref(), Some("geom"));
    assert_eq!(seeded.primary_key.as_deref(), Some("id"));
    assert_eq!(seeded.srid, Some(4326));
    assert!(
        seeded.geometry_type.is_some(),
        "geometry_columns always reports a type for a registered geometry column"
    );
}

/// `tolerance` matters here: `ST_EstimatedExtent` reads PostGIS's spatial
/// statistics, which are stored at reduced (`float4`) precision — a real,
/// documented approximation, not a bug — so callers exercising that path
/// need a looser bound than callers exercising the exact `ST_Extent` scan.
fn assert_bbox_matches_seeded_points(bbox: [f64; 4], tolerance: f64) {
    let [minx, miny, maxx, maxy] = bbox;
    assert!((minx - 10.0).abs() < tolerance, "minx was {minx}");
    assert!((miny - 45.0).abs() < tolerance, "miny was {miny}");
    assert!((maxx - 12.0).abs() < tolerance, "maxx was {maxx}");
    assert!((maxy - 47.0).abs() < tolerance, "maxy was {maxy}");
}

#[tokio::test]
async fn extent_is_derived_via_estimated_extent_after_analyze() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping extent_is_derived_via_estimated_extent_after_analyze: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_extent_estimated";
    seed(&database_url, table).await;

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
        .unwrap_or_else(|| panic!("catalog should report the seeded table '{table}'"));

    let extent = catalog
        .extent(&physical)
        .await
        .expect("extent query succeeds")
        .expect("an ANALYZEd, non-empty table has an extent");
    assert_bbox_matches_seeded_points(extent.bbox, ESTIMATE_TOLERANCE_DEG);
}

/// `ST_EstimatedExtent` has nothing to read on a table that was never
/// `ANALYZE`d; `extent()` must still come back with the correct bbox via the
/// `ST_Extent` fallback (`#27`). Note: since autovacuum could in principle
/// analyze the table before this test's query runs, this asserts the
/// end-to-end result rather than which code path produced it.
#[tokio::test]
async fn extent_falls_back_to_a_real_scan_when_the_table_was_never_analyzed() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping extent_falls_back_to_a_real_scan_when_the_table_was_never_analyzed: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_extent_fallback";
    seed_without_analyze(&database_url, table).await;

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
        .unwrap_or_else(|| panic!("catalog should report the seeded table '{table}'"));

    let extent = catalog
        .extent(&physical)
        .await
        .expect("extent query succeeds")
        .expect("a non-empty table has an extent even with no statistics");
    assert_bbox_matches_seeded_points(extent.bbox, ESTIMATE_TOLERANCE_DEG);
}

#[tokio::test]
async fn empty_collection_reports_no_extent_without_erroring() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping empty_collection_reports_no_extent_without_erroring: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_extent_empty";
    seed_empty(&database_url, table).await;

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
        .unwrap_or_else(|| panic!("catalog should report the seeded table '{table}'"));

    let extent = catalog
        .extent(&physical)
        .await
        .expect("an empty table's extent query must not error");
    assert!(
        extent.is_none(),
        "an empty collection must report a null extent, not a zero-sized bbox"
    );
}

/// A small, mixed polygon fixture for the geometry-profile tests (`#101`):
/// two simple polygons, one multi-part polygon, and one polygon with a hole
/// — enough to exercise vertex counts, the multi-part fraction, ring counts,
/// and area-based feature-size percentiles all in one sampled table.
async fn seed_polygons_for_geometry_profile(database_url: &str, table: &str) {
    let client = test_harness::connect(database_url).await;

    // Column typed `MultiPolygon`, not the untyped `Geometry` every other
    // seed fixture uses that's free to mix shapes: `geometry_columns.type`
    // (what `build_geometry_profile_plan` picks its feature-size metric
    // from) has to actually say "POLYGON" for this fixture to exercise the
    // area-percentile path — an untyped `Geometry` column reports no single
    // type, which `#101`'s design correctly treats as "no metric applies
    // uniformly." Every row below is wrapped as a (possibly single-part)
    // MultiPolygon so the column stays honestly typed while still letting
    // `ST_NumGeometries` distinguish simple from multi-part per row.
    test_harness::apply_fixture_ddl(&client, table, &format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 the_geo geometry(MultiPolygon, 4326) NOT NULL
             );
             INSERT INTO {table} (the_geo) VALUES
                 (ST_GeomFromText('MULTIPOLYGON(((0 0, 0 1, 1 1, 1 0, 0 0)))', 4326)),
                 (ST_GeomFromText('MULTIPOLYGON(((0 0, 0 2, 2 2, 2 0, 0 0)))', 4326)),
                 (ST_GeomFromText(
                     'MULTIPOLYGON(((10 10, 10 11, 11 11, 11 10, 10 10)), ((12 12, 12 13, 13 13, 13 12, 12 12)))',
                     4326
                 )),
                 (ST_GeomFromText(
                     'MULTIPOLYGON(((20 20, 20 21, 21 21, 21 20, 20 20), (20.25 20.25, 20.25 20.5, 20.5 20.5, 20.5 20.25, 20.25 20.25)))',
                     4326
                 ));
             ANALYZE {table};"
        ))
        .await
        .expect("seeds the geometry-profile polygon fixture");
}

#[tokio::test]
async fn geometry_profile_samples_vertex_and_feature_size_stats_against_a_real_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping geometry_profile_samples_vertex_and_feature_size_stats_against_a_real_database: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_geometry_profile";
    seed_polygons_for_geometry_profile(&database_url, table).await;

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
        .unwrap_or_else(|| panic!("catalog should report the seeded table '{table}'"));
    assert_eq!(physical.geometry_column.as_deref(), Some("the_geo"));

    let profile = catalog
        .geometry_profile(&physical)
        .await
        .expect("geometry profile query succeeds")
        .expect("a non-empty, analyzed table produces a profile");

    // The fixture has 4 rows and an ANALYZEd row estimate, so
    // `sample_percentage` clamps to 100% — `TABLESAMPLE SYSTEM(100)`
    // deterministically includes every block.
    assert_eq!(
        profile.sample_size, 4,
        "a 100% system sample of a 4-row table must read every row"
    );
    assert!(
        profile.vertices.mean > 0.0,
        "every seeded feature has vertices"
    );
    assert!(
        profile.vertices.max >= 5,
        "the seeded 20-unit square (with a hole) has at least 5 vertices in its ring"
    );
    assert!(
        profile.multi_part_fraction > 0.0 && profile.multi_part_fraction < 1.0,
        "exactly one of the four seeded features is a MultiPolygon: {}",
        profile.multi_part_fraction
    );
    assert!(
        profile.mean_ring_count.is_some_and(|r| r > 1.0),
        "the seeded polygon-with-a-hole must raise the mean ring count above 1"
    );
    assert!(
        profile.feature_size.p50.is_some(),
        "a polygon-typed collection must report area-based feature-size percentiles"
    );
    assert!(
        profile.vertex_density_per_area.is_some(),
        "the sampled features' combined bbox has positive area"
    );
    assert!(
        profile
            .computed_at
            .elapsed()
            .is_ok_and(|elapsed| elapsed.as_secs() < 60),
        "computed_at must be a fresh wall-clock timestamp"
    );
}

/// Design point 2 (`#101`): an empty, `ANALYZE`d table must report no
/// profile at all — never a profile of zeroes — mirroring
/// `empty_collection_reports_no_extent_without_erroring`'s own "no data,
/// no error" contract for `extent`.
#[tokio::test]
async fn geometry_profile_reports_none_for_an_empty_table() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping geometry_profile_reports_none_for_an_empty_table: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_geometry_profile_empty";
    seed_empty(&database_url, table).await;

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
        .unwrap_or_else(|| panic!("catalog should report the seeded table '{table}'"));

    let profile = catalog
        .geometry_profile(&physical)
        .await
        .expect("an empty table's geometry-profile query must not error");
    assert!(
        profile.is_none(),
        "an empty collection must report no profile, not a profile of zeroes"
    );
}

/// A single multi-part feature whose hole sits in its SECOND part: part one
/// is a plain square (one ring), part two is a square with a hole (two
/// rings) — three rings total. The original `ST_GeometryN(geom, 1)` proxy
/// only ever looked at the first part, so it would have reported this
/// feature's ring count as 1 (missing both of part two's rings entirely).
/// Full multi-part enumeration must report the true total, 3.
async fn seed_multi_part_hole_in_second_part(database_url: &str, table: &str) {
    let client = test_harness::connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(MultiPolygon, 4326) NOT NULL
             );
             INSERT INTO {table} (geom) VALUES
                 (ST_GeomFromText(
                     'MULTIPOLYGON(((0 0, 0 1, 1 1, 1 0, 0 0)), \
                      ((10 10, 10 12, 12 12, 12 10, 10 10), \
                       (10.5 10.5, 10.5 11, 11 11, 11 10.5, 10.5 10.5)))',
                     4326
                 ));
             ANALYZE {table};"
        ),
    )
    .await
    .expect("seeds the second-part-hole ring-enumeration fixture");
}

#[tokio::test]
async fn geometry_profile_enumerates_rings_in_every_part_not_just_the_first_against_a_real_database(
) {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping geometry_profile_enumerates_rings_in_every_part_not_just_the_first_against_a_real_database: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_geometry_profile_ring_enum";
    seed_multi_part_hole_in_second_part(&database_url, table).await;

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
        .unwrap_or_else(|| panic!("catalog should report the seeded table '{table}'"));

    let profile = catalog
        .geometry_profile(&physical)
        .await
        .expect("geometry profile query succeeds")
        .expect("a non-empty, analyzed table produces a profile");

    assert_eq!(
        profile.sample_size, 1,
        "a 100% system sample of a 1-row table must read the one row"
    );
    assert_eq!(
        profile.mean_ring_count,
        Some(3.0),
        "the feature has 1 ring in its first part and 2 in its second (3 \
         total) — the old first-part-only proxy would have reported 1.0, \
         missing both of the second part's rings"
    );
}

/// A line-typed fixture (`#101`): the geometry column is declared
/// `LineString`, not untyped `Geometry`, so `geometry_columns.type` reports
/// "LINESTRING" and `build_geometry_profile_plan` picks the length-based
/// feature-size metric — the sibling path to the polygon fixture's
/// area-based one.
async fn seed_lines_for_geometry_profile(database_url: &str, table: &str) {
    let client = test_harness::connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(LineString, 4326) NOT NULL
             );
             INSERT INTO {table} (geom) VALUES
                 (ST_GeomFromText('LINESTRING(0 0, 0 1)', 4326)),
                 (ST_GeomFromText('LINESTRING(0 0, 0 5)', 4326)),
                 (ST_GeomFromText('LINESTRING(0 0, 0 10)', 4326)),
                 (ST_GeomFromText('LINESTRING(0 0, 3 4)', 4326));
             ANALYZE {table};"
        ),
    )
    .await
    .expect("seeds the geometry-profile line fixture");
}

/// `#101`: a line-typed collection's profile populates length percentiles
/// and reports no area (the design's own "area for polygons, length for
/// lines, never both" rule) — proving the profile mechanism stays coherent
/// on a shape other than the polygon fixture every other geometry-profile
/// test above uses.
#[tokio::test]
async fn geometry_profile_populates_length_percentiles_for_a_line_collection_against_a_real_database(
) {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping geometry_profile_populates_length_percentiles_for_a_line_collection_against_a_real_database: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_geometry_profile_lines";
    seed_lines_for_geometry_profile(&database_url, table).await;

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
        .unwrap_or_else(|| panic!("catalog should report the seeded table '{table}'"));

    let profile = catalog
        .geometry_profile(&physical)
        .await
        .expect("geometry profile query succeeds")
        .expect("a non-empty, analyzed table produces a profile");

    assert_eq!(
        profile.sample_size, 4,
        "a 100% system sample of a 4-row table must read every row"
    );
    assert!(
        profile.feature_size.p50.is_some(),
        "a line-typed collection must report length-based feature-size percentiles"
    );
    assert!(
        profile.feature_size.max.is_some_and(|max| max >= 9.9),
        "the seeded 10-unit-long line must be the sampled maximum length: {:?}",
        profile.feature_size.max
    );
    assert!(
        profile.mean_ring_count.is_none(),
        "a line-typed collection has no ring concept"
    );
}

/// A point-typed fixture (`#101`): proves the profile stays coherent rather
/// than degenerate on the simplest possible geometry shape — trivial vertex
/// stats (every point has exactly one vertex), no area/length feature-size
/// nonsense, and no ring count.
async fn seed_points_for_geometry_profile(database_url: &str, table: &str) {
    let client = test_harness::connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326) NOT NULL
             );
             INSERT INTO {table} (geom) VALUES
                 (ST_GeomFromText('POINT(0 0)', 4326)),
                 (ST_GeomFromText('POINT(1 1)', 4326)),
                 (ST_GeomFromText('POINT(2 2)', 4326));
             ANALYZE {table};"
        ),
    )
    .await
    .expect("seeds the geometry-profile point fixture");
}

/// `#101`: a point-typed collection's profile is coherent, not degenerate —
/// every vertex stat is trivially 1 (a point has exactly one vertex), no
/// area/length feature-size metric applies, and no ring count applies
/// either.
#[tokio::test]
async fn geometry_profile_stays_coherent_for_a_point_collection_against_a_real_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping geometry_profile_stays_coherent_for_a_point_collection_against_a_real_database: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_geometry_profile_points";
    seed_points_for_geometry_profile(&database_url, table).await;

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
        .unwrap_or_else(|| panic!("catalog should report the seeded table '{table}'"));

    let profile = catalog
        .geometry_profile(&physical)
        .await
        .expect("geometry profile query succeeds")
        .expect("a non-empty, analyzed table produces a profile");

    assert_eq!(
        profile.sample_size, 3,
        "a 100% system sample of a 3-row table must read every row"
    );
    assert_eq!(
        profile.vertices.mean, 1.0,
        "every point has exactly one vertex"
    );
    assert_eq!(profile.vertices.median, 1.0);
    assert_eq!(profile.vertices.p95, 1.0);
    assert_eq!(profile.vertices.max, 1);
    assert_eq!(
        profile.multi_part_fraction, 0.0,
        "a plain Point column has no multi-part features"
    );
    assert!(
        profile.feature_size.p50.is_none()
            && profile.feature_size.p95.is_none()
            && profile.feature_size.max.is_none(),
        "a point-typed collection has no area or length concept: {:?}",
        profile.feature_size
    );
    assert!(
        profile.mean_ring_count.is_none(),
        "a point-typed collection has no ring concept"
    );
}

/// `#19` end to end: a collection whose config declares nothing but routing
/// (no `table`/`geometry`/`pk`) still boots and serves real data — table
/// derives from the collection id by convention, geometry/pk derive from
/// the backend's `CatalogSource`.
#[tokio::test]
async fn router_serves_a_collection_configured_with_only_routing_fields() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping router_serves_a_collection_configured_with_only_routing_fields: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_omitted_fields";
    seed(&database_url, table).await;

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
        .expect("boot derives table/geometry/pk from the backend");

    let (decl, source) = router
        .resolve_features("public", "default", table)
        .await
        .expect("resolves despite the config declaring no physical fields");
    assert_eq!(decl.table.as_deref(), Some(table));
    assert_eq!(decl.geometry.as_deref(), Some("geom"));
    assert_eq!(decl.pk.as_deref(), Some("id"));

    let page = source
        .items(&decl, &ItemsQuery::default())
        .await
        .expect("items query succeeds against the derived physical shape");
    assert_eq!(
        page.features_geojson.len(),
        3,
        "all three seeded rows come back"
    );

    let descriptor = router
        .collection_descriptor("public", "default", table)
        .await
        .expect("descriptor resolves");
    let extent = descriptor
        .extent
        .expect("the seeded, ANALYZEd table has a derived extent");
    assert_bbox_matches_seeded_points(extent.bbox, ESTIMATE_TOLERANCE_DEG);
}

/// `#19` end to end: the TTL-cached descriptor picks up a live backend
/// schema change with no config edit anywhere. Builds a `Router` with a
/// tiny (1s) `descriptor_ttl_s`, serves once (warming the cache — no
/// temporal column exists on the table yet), `ALTER`s a dedicated scratch
/// table to add a timestamp column, waits past the TTL, then asserts the
/// next resolve/descriptor call re-derives and reflects the change:
/// `datetime` moves from `None` to the new column's name (the
/// exactly-one-candidate temporal derivation), and the richer attribute
/// schema lists it too.
#[tokio::test]
async fn descriptor_refresh_picks_up_a_schema_change_after_the_ttl_expires_without_a_config_edit() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping descriptor_refresh_picks_up_a_schema_change_after_the_ttl_expires_without_a_config_edit: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    // A dedicated, uniquely-named scratch table — unlike every other test in
    // this file, this one mutates the table's schema mid-test (the whole
    // point), so it must never be one of the tables other tests rely on, and
    // it is dropped at the end rather than left for the next run to reuse.
    let table = "tellurion_postgis_live_test_descriptor_refresh";
    let client = test_harness::connect(&database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326) NOT NULL,
                 name text
             );
             INSERT INTO {table} (geom, name) VALUES
                 (ST_SetSRID(ST_MakePoint(10, 45), 4326), 'a');
             ANALYZE {table};"
        ),
    )
    .await
    .expect("seeds the scratch table");

    unsafe {
        env::set_var(URL_ENV_VAR, &database_url);
    }

    let config: AppConfig = serde_yaml::from_str(&format!(
        "server: {{ descriptor_ttl_s: 1 }}\n\
         storages: [ {{ id: main, driver: postgis, url_env: {URL_ENV_VAR} }} ]\n\
         tenants: [ {{ id: public }} ]\n\
         catalogs: [ {{ id: default, tenant: public }} ]\n\
         collections:\n  - id: {table}\n    catalog: default\n    storage: main\n"
    ))
    .expect("valid AppConfig yaml with no physical fields declared");
    config.validate().expect("routing-only config is valid");

    let mut registry = Registry::new();
    registry.register(Arc::new(PostgisDriverFactory::new(60)));
    let router = Router::build(&config, &registry).expect("router builds");
    router
        .validate_catalog()
        .await
        .expect("boot derives the physical shape from the freshly seeded table");

    // First serve, warming the TTL cache: no timestamp/date column exists on
    // the table yet, so nothing derives for `datetime`.
    let (decl, source) = router
        .resolve_features("public", "default", table)
        .await
        .expect("resolves against the scratch table");
    assert_eq!(decl.datetime, None);
    let page = source
        .items(&decl, &ItemsQuery::default())
        .await
        .expect("serves the one seeded row");
    assert_eq!(page.features_geojson.len(), 1);

    let attributes_before = router
        .collection_descriptor("public", "default", table)
        .await
        .expect("descriptor resolves")
        .attributes
        .expect("postgis always answers the attribute schema");
    assert!(
        !attributes_before.iter().any(|c| c.name == "observed_at"),
        "the new column does not exist yet"
    );

    // A live schema change with no config edit anywhere: add a single
    // timestamp column, the temporal-column derivation's exactly-one-
    // candidate case (see `CatalogSource::temporal_column`).
    client
        .batch_execute(&format!(
            "ALTER TABLE {table} ADD COLUMN observed_at timestamptz;"
        ))
        .await
        .expect("alters the scratch table");

    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;

    let (decl_after, _source) = router
        .resolve_features("public", "default", table)
        .await
        .expect("resolves again after the TTL expires");
    assert_eq!(
        decl_after.datetime.as_deref(),
        Some("observed_at"),
        "the re-derived descriptor must pick up the new column with no config edit"
    );

    let attributes_after = router
        .collection_descriptor("public", "default", table)
        .await
        .expect("descriptor re-resolves")
        .attributes
        .expect("postgis always answers the attribute schema");
    assert!(
        attributes_after.iter().any(|c| c.name == "observed_at"),
        "the richer attribute schema must list the newly added column too"
    );

    client
        .batch_execute(&format!("DROP TABLE IF EXISTS {table};"))
        .await
        .expect("cleans up the dedicated scratch table");
}

/// A plain byte-substring search: `ST_AsMVT`'s layer name is a
/// length-prefixed UTF-8 string field in the returned protobuf, so this is
/// enough to prove which name landed in a served tile without pulling in a
/// full MVT decoder dependency this crate doesn't otherwise need.
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// `#49`: proves the PostGIS driver embeds a collection's EXTERNAL id —
/// never its internal one — as the real `ST_AsMVT` layer name a client
/// receives over the wire, for a collection whose `external_id` genuinely
/// differs from `id`. Complements `tellurion-postgis::sql`'s own
/// SQL-generation-level test with a real round trip through a live database.
#[tokio::test]
async fn mvt_tile_embeds_the_external_id_as_the_layer_name_when_it_differs_from_the_internal_id() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping mvt_tile_embeds_the_external_id_as_the_layer_name_when_it_differs_from_the_internal_id: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_alias";
    seed(&database_url, table).await;

    // Safety: same single-threaded-with-respect-to-env-var-access argument
    // as `items_and_mvt_round_trip_against_a_real_database` above.
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
    let tiles = driver.tile_source().expect("driver exposes TileSource");

    let collection: CollectionDecl = serde_yaml::from_str(&format!(
        "id: internal-alias-marker\nexternal_id: alias-demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\ndatetime: observed_at\n"
    ))
    .expect("valid CollectionDecl yaml");

    let tile = tiles
        .mvt_tile(&collection, TileCoord { z: 0, x: 0, y: 0 }, None)
        .await
        .expect("mvt query succeeds")
        .expect("z0 covers the whole world and all three seeded points");

    assert!(
        contains_bytes(&tile, b"alias-demo"),
        "the served MVT bytes must carry the external id as the layer name"
    );
    assert!(
        !contains_bytes(&tile, b"internal-alias-marker"),
        "the served MVT bytes must never carry the internal id"
    );
}

/// `#85`: `collection.tile_properties` widens the real `ST_AsMVT` tuple —
/// against a live database, not just `tellurion-postgis::sql`'s own
/// golden-SQL test, so a genuine type mismatch (a column PostgreSQL itself
/// refuses to select this way) would surface here as a query error rather
/// than passing silently. Scalar values (string/integer/boolean) are
/// checked, and the pk-only default is unchanged.
#[tokio::test]
async fn mvt_tile_projects_the_allowlisted_properties_against_a_real_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping mvt_tile_projects_the_allowlisted_properties_against_a_real_database: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_tile_properties";
    seed_with_typed_attributes(&database_url, table).await;

    // Safety: same single-threaded-with-respect-to-env-var-access argument
    // as `items_and_mvt_round_trip_against_a_real_database` above.
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
    let tiles = driver.tile_source().expect("driver exposes TileSource");

    // No `datetime` field — `seed_with_typed_attributes`'s table has no
    // such column, the same shape
    // `queryable_equality_predicates_narrow_the_result_set_against_a_real_database`
    // already uses for this fixture.
    let mut projected: CollectionDecl = serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\n"
    ))
    .expect("valid CollectionDecl yaml");
    projected.tile_properties = vec![
        "name".to_string(),
        "population".to_string(),
        "active".to_string(),
    ];

    let tile = tiles
        .mvt_tile(&projected, TileCoord { z: 0, x: 0, y: 0 }, None)
        .await
        .expect("mvt query succeeds")
        .expect("z0 covers the whole world and all three seeded points");

    // Every allowlisted key name is a length-prefixed UTF-8 string in the
    // returned protobuf's key dictionary regardless of its value's own
    // type, so this alone proves all three columns were actually selected
    // (a genuine type error — e.g. selecting an incompatible column type
    // inside the MVT subquery — would have failed the query above instead
    // of returning bytes at all).
    for key in ["name", "population", "active"] {
        assert!(
            contains_bytes(&tile, key.as_bytes()),
            "served MVT bytes must carry the '{key}' property key"
        );
    }
    // The `text` column's own value round-trips as a readable UTF-8
    // substring — the one value shape `contains_bytes` can check directly
    // without a full protobuf decoder this crate doesn't otherwise need.
    assert!(
        contains_bytes(&tile, b"a") || contains_bytes(&tile, b"b") || contains_bytes(&tile, b"c"),
        "served MVT bytes must carry at least one seeded 'name' value"
    );

    // Pk-only default: the same table with no `tile_properties` never
    // carries the extra keys — unchanged from before `#85`.
    let pk_only: CollectionDecl = serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\n"
    ))
    .expect("valid CollectionDecl yaml");
    assert!(pk_only.tile_properties.is_empty());
    let tile = tiles
        .mvt_tile(&pk_only, TileCoord { z: 0, x: 0, y: 0 }, None)
        .await
        .expect("mvt query succeeds")
        .expect("z0 covers the whole world and all three seeded points");
    assert!(
        !contains_bytes(&tile, b"population"),
        "no tile_properties declared means no property keys beyond the reserved 'id'"
    );
}

/// `#90`: three ordinary points plus one deliberately dense geometry — a
/// 100-vertex zigzag `LineString` whose every vertex alternates ~50m off a
/// near-vertical baseline, so `ST_SimplifyPreserveTopology` cannot collapse
/// it away at any reasonable zoom (each point's perpendicular deviation
/// from its two-neighbor chord is the full ~50m amplitude, far above the
/// z15 tolerance used below). All four rows sit within a couple hundred
/// meters of the equator/prime-meridian origin so a single z15 tile (~1.2km
/// across) holds all of them with no `ST_AsMVTGeom` clipping loss. The
/// dense geometry is inserted last (highest pk), so a tight vertex budget
/// truncates exactly it while the three simple points survive.
async fn seed_dense_and_simple_geometries(database_url: &str, table: &str) {
    let client = test_harness::connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Geometry, 4326) NOT NULL
             );
             INSERT INTO {table} (geom) VALUES
                 (ST_SetSRID(ST_MakePoint(0.00001, 0.00001), 4326)),
                 (ST_SetSRID(ST_MakePoint(0.00002, 0.00001), 4326)),
                 (ST_SetSRID(ST_MakePoint(0.00001, 0.00002), 4326)),
                 (
                     ST_SetSRID(
                         ST_MakeLine(ARRAY(
                             SELECT ST_MakePoint((i % 2) * 0.00045, i * 0.00001)
                             FROM generate_series(0, 99) AS i
                         )),
                         4326
                     )
                 );
             ANALYZE {table};"
        ),
    )
    .await
    .expect("seeds the dense/simple geometry test table");
}

/// [`collection`] plus an explicit per-collection `settings.tile_vertex_budget`
/// override (`#90`).
fn collection_with_vertex_budget(table: &str, budget: u64) -> CollectionDecl {
    serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\nsettings: {{ tile_vertex_budget: {budget} }}\n"
    ))
    .expect("valid CollectionDecl yaml")
}

fn collection_with_items_vertex_budget(table: &str, budget: u64) -> CollectionDecl {
    serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\nsettings: {{ items_vertex_budget: {budget} }}\n"
    ))
    .expect("valid CollectionDecl yaml")
}

#[tokio::test]
async fn exact_items_are_refused_before_encoding_when_the_vertex_budget_is_crossed() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping exact_items_are_refused_before_encoding_when_the_vertex_budget_is_crossed: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_items_vertex_budget";
    seed_dense_and_simple_geometries(&database_url, table).await;
    unsafe {
        env::set_var(URL_ENV_VAR, &database_url);
    }
    let driver = PostgisDriverFactory::new(60)
        .build(&StorageDecl {
            id: "main".to_string(),
            driver: "postgis".to_string(),
            url_env: URL_ENV_VAR.to_string(),
            pool_size: None,
        })
        .expect("driver builds");
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");

    let accepted = features
        .items(
            &collection_with_items_vertex_budget(table, 3),
            &ItemsQuery {
                limit: 3,
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("a page exactly on budget succeeds");
    assert_eq!(accepted.features_geojson.len(), 3);

    let refused = features
        .items(
            &collection_with_items_vertex_budget(table, 2),
            &ItemsQuery {
                limit: 3,
                ..ItemsQuery::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        refused,
        Error::ItemsVertexBudgetExceeded {
            feature_id,
            cumulative_vertices: 3,
            limit: 2,
            ..
        } if feature_id == "3"
    ));

    let refused = features
        .item(&collection_with_items_vertex_budget(table, 50), "4", None)
        .await
        .unwrap_err();
    assert!(matches!(
        refused,
        Error::ItemsVertexBudgetExceeded {
            feature_id,
            cumulative_vertices: 100,
            limit: 50,
            ..
        } if feature_id == "4"
    ));
}

/// Counts the features across every layer of a served MVT tile by decoding
/// it back through the same `geozero::mvt::Tile` protobuf message
/// `tellurion-geopackage`'s own encoder builds — the precise, decoded
/// counterpart of this file's looser `contains_bytes` substring checks,
/// needed here to prove exactly how many (and which) features a
/// vertex-budget decision let through.
fn decoded_feature_count(tile: &[u8]) -> usize {
    use geozero::mvt::Message;
    let decoded = geozero::mvt::Tile::decode(tile).expect("served bytes are a valid MVT tile");
    decoded
        .layers
        .iter()
        .map(|layer| layer.features.len())
        .sum()
}

/// `#90` first slice: a tile whose candidate rows sum well under the
/// effective vertex budget must serve every feature, unaffected — the same
/// content `items_and_mvt_round_trip_against_a_real_database` already
/// proves for this fixture's three ordinary points, checked here with an
/// exact decoded feature count instead of a bare `is_some`, and proven
/// deterministic by fetching the same tile twice and comparing bytes.
#[tokio::test]
async fn mvt_tile_is_unaffected_when_under_the_vertex_budget_against_a_real_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping mvt_tile_is_unaffected_when_under_the_vertex_budget_against_a_real_database: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_vertex_budget_unaffected";
    seed(&database_url, table).await;

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
    let tiles = driver.tile_source().expect("driver exposes TileSource");
    let collection = collection(table);
    let coord = TileCoord { z: 0, x: 0, y: 0 };

    let first = tiles
        .mvt_tile(&collection, coord, None)
        .await
        .expect("mvt query succeeds")
        .expect("z0 covers the whole world and all three seeded points");
    let second = tiles
        .mvt_tile(&collection, coord, None)
        .await
        .expect("mvt query succeeds")
        .expect("z0 covers the whole world and all three seeded points");

    assert_eq!(
        first.as_ref(),
        second.as_ref(),
        "an under-budget tile's wire bytes must be stable/deterministic, the same query \
         `build_mvt_plan` always ran before #90's vertex budget existed"
    );
    assert_eq!(
        decoded_feature_count(&first),
        3,
        "no seeded row is anywhere near the default vertex budget; all three must survive"
    );
}

/// `#90`: with a tight per-collection `tile_vertex_budget`, the dense
/// 100-vertex `LineString` [`seed_dense_and_simple_geometries`] seeds last
/// (highest pk) pushes the running vertex total over budget and is dropped
/// — the three simple points ahead of it (cumulative total 3) still fit and
/// are served untouched. The same table under a generous budget serves all
/// four, proving the drop is genuinely budget-driven and not some other
/// fault swallowing the dense row.
#[tokio::test]
async fn mvt_tile_drops_the_marginal_geometry_when_it_exceeds_the_vertex_budget_against_a_real_database(
) {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping mvt_tile_drops_the_marginal_geometry_when_it_exceeds_the_vertex_budget_against_a_real_database: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_vertex_budget_exceeded";
    seed_dense_and_simple_geometries(&database_url, table).await;

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
    let tiles = driver.tile_source().expect("driver exposes TileSource");
    // z15's ~4.8m simplification tolerance is far below the zigzag's ~50m
    // amplitude (survives intact) and its ~1.2km tile comfortably contains
    // every seeded coordinate (no clipping loss) — see the fixture's own
    // doc for the full margin reasoning. lon=0 sits exactly on the x=2^14
    // tile boundary for any zoom (Web Mercator's antimeridian-anchored X
    // grid); every seeded point's longitude is positive (east of it), so
    // x=2^14 is the tile that actually contains them. Every seeded point's
    // *latitude* is also positive (north of the equator) — the equator is
    // the y=2^14 tile boundary too, but the tile north of it (lower y, XYZ
    // tiling numbers rows top-to-bottom) is y=2^14-1, not y=2^14.
    let coord = TileCoord {
        z: 15,
        x: 16384,
        y: 16383,
    };

    let tight = collection_with_vertex_budget(table, 10);
    let truncated_tile = tiles
        .mvt_tile(&tight, coord, None)
        .await
        .expect("mvt query succeeds")
        .expect("the three simple points still fit under the budget");
    assert_eq!(
        decoded_feature_count(&truncated_tile),
        3,
        "the dense linestring (~100 vertices) must be dropped under a budget of 10, \
         leaving only the three simple points whose combined total (3) still fits"
    );

    let generous = collection_with_vertex_budget(table, 1_000_000);
    let full_tile = tiles
        .mvt_tile(&generous, coord, None)
        .await
        .expect("mvt query succeeds")
        .expect("all four rows fit comfortably under a generous budget");
    assert_eq!(
        decoded_feature_count(&full_tile),
        4,
        "a generous budget must serve every seeded row, proving the truncation above is \
         genuinely budget-driven and not some other fault dropping the dense row"
    );
}

/// Seeds a `PolyhedralSurface Z` table holding one closed, six-faced,
/// axis-aligned "building" (a small footprint near lon 10 / lat 45, height
/// 0-12 meters) — a real fixture for `#41`'s end-to-end `VolumeSource` test,
/// not the synthetic local-coordinate cube `ewkb.rs`'s own unit tests use.
async fn seed_polyhedral_cube(database_url: &str, table: &str) {
    let client = test_harness::connect(database_url).await;
    test_harness::apply_fixture_ddl(&client, table, &format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(PolyhedralSurfaceZ, 4326) NOT NULL
             );
             INSERT INTO {table} (geom) VALUES (ST_SetSRID(
                 'POLYHEDRALSURFACE Z(
                     ((10.0 45.0 0, 10.0 45.0001 0, 10.0001 45.0001 0, 10.0001 45.0 0, 10.0 45.0 0)),
                     ((10.0 45.0 12, 10.0001 45.0 12, 10.0001 45.0001 12, 10.0 45.0001 12, 10.0 45.0 12)),
                     ((10.0 45.0 0, 10.0001 45.0 0, 10.0001 45.0 12, 10.0 45.0 12, 10.0 45.0 0)),
                     ((10.0001 45.0 0, 10.0001 45.0001 0, 10.0001 45.0001 12, 10.0001 45.0 12, 10.0001 45.0 0)),
                     ((10.0001 45.0001 0, 10.0 45.0001 0, 10.0 45.0001 12, 10.0001 45.0001 12, 10.0001 45.0001 0)),
                     ((10.0 45.0001 0, 10.0 45.0 0, 10.0 45.0 12, 10.0 45.0001 12, 10.0 45.0001 0))
                 )'::geometry, 4326)
             );
             ANALYZE {table};"
        ))
        .await
        .expect("seeds the polyhedral surface test table");
}

fn volume_collection(table: &str) -> CollectionDecl {
    serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\n"
    ))
    .expect("valid CollectionDecl yaml")
}

/// `#41` end to end against a real database: a `PolyhedralSurface Z`
/// fixture table, fetched through the real `PostgisDriverFactory`'s
/// `VolumeSource`, produces a mesh with the expected triangle count (six
/// quad faces, two triangles each) and tile-local/real-world bounds — the
/// geometry-type check, EWKB decode, triangulation, and world-to-tile-local
/// transform all exercised together, not just unit-tested in isolation.
#[tokio::test]
async fn volume_tile_serves_a_polyhedral_surface_cube_with_bounded_triangle_count() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping volume_tile_serves_a_polyhedral_surface_cube_with_bounded_triangle_count: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_volume_cube";
    seed_polyhedral_cube(&database_url, table).await;

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
    let volumes = driver
        .volume_source()
        .expect("the postgis driver always advertises VolumeSource");

    let collection = volume_collection(table);
    let mesh = volumes
        .volume_tile(&collection, TileCoord { z: 0, x: 0, y: 0 }, None)
        .await
        .expect("volume query succeeds")
        .expect("z0 covers the whole world and the one seeded solid");

    assert_eq!(
        mesh.indices.len(),
        36,
        "six quad faces * two triangles * three indices each"
    );
    assert_eq!(mesh.positions.len(), 36, "no vertex deduplication");

    // Z passes through untouched, in real-world meters — the seeded solid's
    // exact 0/12 bounds, not some tile-normalized fraction of them.
    let z_min = mesh.positions.iter().map(|p| p[2]).fold(f64::MAX, f64::min);
    let z_max = mesh.positions.iter().map(|p| p[2]).fold(f64::MIN, f64::max);
    assert_eq!(z_min, 0.0);
    assert_eq!(z_max, 12.0);

    // X/Y land in the tile-local unit square, in the quadrant lon>0/lat>0
    // (east of the prime meridian, north of the equator) puts them in:
    // local X = 0.5 + lon/360 (a bit past the map's horizontal center),
    // local Y < 0.5 (Y increases downward, so the northern hemisphere sits
    // in the top half of the tile).
    for p in &mesh.positions {
        assert!(
            (0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1]),
            "vertex {p:?} must land inside the tile-local unit square"
        );
        assert!(
            p[0] > 0.5 && p[0] < 0.6,
            "X should sit just past the map's horizontal center for lon~10: {p:?}"
        );
        assert!(
            p[1] > 0.3 && p[1] < 0.5,
            "Y should sit in the northern half of the tile for lat~45: {p:?}"
        );
    }
}

/// `#41` part 1 end to end: a collection whose geometry column is an
/// ordinary 2D `Point` (the same fixture `seed` builds for every other live
/// test in this file) is a named, request-time refusal from `volume_tile`
/// — never a confusing per-row EWKB decode failure or a silently-always-
/// empty tile.
#[tokio::test]
async fn volume_tile_refuses_a_collection_whose_geometry_is_not_a_supported_3d_type() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping volume_tile_refuses_a_collection_whose_geometry_is_not_a_supported_3d_type: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_volume_unsupported_type";
    seed(&database_url, table).await;

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
    let volumes = driver
        .volume_source()
        .expect("the postgis driver always advertises VolumeSource");

    let collection = volume_collection(table);
    let error = volumes
        .volume_tile(&collection, TileCoord { z: 0, x: 0, y: 0 }, None)
        .await
        .expect_err("a 2D Point column must be refused, not silently emptied");
    assert!(
        matches!(error, tellurion_core::Error::Invalid(_)),
        "the geometry-type mismatch must classify as a request-time Invalid error: {error:?}"
    );
}

/// Two `PolyhedralSurface Z` solids in one table, each tagged with a
/// distinguishing `org` attribute column — `#70`'s own filter-aware volume
/// query fixture. The "acme" solid is the same cube `seed_polyhedral_cube`
/// seeds (top face Z=12); the "globex" solid is a second, distinct cube
/// offset in Z (top face Z=99) so a filtered mesh's own max Z alone proves
/// which solid actually reached the response.
async fn seed_polyhedral_cubes_with_org(database_url: &str, table: &str) {
    let client = test_harness::connect(database_url).await;
    test_harness::apply_fixture_ddl(&client, table, &format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 org text NOT NULL,
                 geom geometry(PolyhedralSurfaceZ, 4326) NOT NULL
             );
             INSERT INTO {table} (org, geom) VALUES ('acme', ST_SetSRID(
                 'POLYHEDRALSURFACE Z(
                     ((10.0 45.0 0, 10.0 45.0001 0, 10.0001 45.0001 0, 10.0001 45.0 0, 10.0 45.0 0)),
                     ((10.0 45.0 12, 10.0001 45.0 12, 10.0001 45.0001 12, 10.0 45.0001 12, 10.0 45.0 12)),
                     ((10.0 45.0 0, 10.0001 45.0 0, 10.0001 45.0 12, 10.0 45.0 12, 10.0 45.0 0)),
                     ((10.0001 45.0 0, 10.0001 45.0001 0, 10.0001 45.0001 12, 10.0001 45.0 12, 10.0001 45.0 0)),
                     ((10.0001 45.0001 0, 10.0 45.0001 0, 10.0 45.0001 12, 10.0001 45.0001 12, 10.0001 45.0001 0)),
                     ((10.0 45.0001 0, 10.0 45.0 0, 10.0 45.0 12, 10.0 45.0001 12, 10.0 45.0001 0))
                 )'::geometry, 4326));
             INSERT INTO {table} (org, geom) VALUES ('globex', ST_SetSRID(
                 'POLYHEDRALSURFACE Z(
                     ((10.001 45.0 0, 10.001 45.0001 0, 10.0011 45.0001 0, 10.0011 45.0 0, 10.001 45.0 0)),
                     ((10.001 45.0 99, 10.0011 45.0 99, 10.0011 45.0001 99, 10.001 45.0001 99, 10.001 45.0 99)),
                     ((10.001 45.0 0, 10.0011 45.0 0, 10.0011 45.0 99, 10.001 45.0 99, 10.001 45.0 0)),
                     ((10.0011 45.0 0, 10.0011 45.0001 0, 10.0011 45.0001 99, 10.0011 45.0 99, 10.0011 45.0 0)),
                     ((10.0011 45.0001 0, 10.001 45.0001 0, 10.001 45.0001 99, 10.0011 45.0001 99, 10.0011 45.0001 0)),
                     ((10.001 45.0001 0, 10.001 45.0 0, 10.001 45.0 99, 10.001 45.0001 99, 10.001 45.0001 0))
                 )'::geometry, 4326));
             ANALYZE {table};"
        ))
        .await
        .expect("seeds the two-org polyhedral surface test table");
}

/// `#70` end to end against a real database: a `#34` grant filter compiled
/// into the volume query excludes the non-matching solid's geometry from
/// the returned mesh entirely, the same "filtered rows never reach the
/// response" guarantee `mvt_tile`'s own filter tests already prove for the
/// 2D lane.
#[tokio::test]
async fn volume_tile_applies_a_grant_filter_and_excludes_the_non_matching_solid() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping volume_tile_applies_a_grant_filter_and_excludes_the_non_matching_solid: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_volume_filter";
    seed_polyhedral_cubes_with_org(&database_url, table).await;

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
    let volumes = driver
        .volume_source()
        .expect("the postgis driver always advertises VolumeSource");
    assert!(
        volumes.filter_capable(),
        "postgis must advertise volume filter capability"
    );

    let collection = volume_collection(table);
    let filter = tellurion_core::filter::parse_text("org = 'acme'").unwrap();
    let mesh = volumes
        .volume_tile(&collection, TileCoord { z: 0, x: 0, y: 0 }, Some(&filter))
        .await
        .expect("filtered volume query succeeds")
        .expect("the acme solid still matches the filter");

    let z_max = mesh.positions.iter().map(|p| p[2]).fold(f64::MIN, f64::max);
    assert_eq!(
        z_max, 12.0,
        "only the acme solid (top face Z=12) must reach the mesh"
    );
    assert!(
        mesh.positions.iter().all(|p| p[2] != 99.0),
        "the globex solid's Z=99 top face must be excluded by the filter"
    );
}

/// `#33` follow-up, advanced comparison operators, end to end against a real
/// database: `LIKE`/`NOT LIKE`, `BETWEEN`, `IN`, and `CASEI` each narrow the
/// typed-attribute seeded table exactly as `cql2_filter_narrows_the_result_
/// set_against_a_real_database` already does for the basic operator set.
#[tokio::test]
async fn advanced_cql2_operators_narrow_the_result_set_against_a_real_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping advanced_cql2_operators_narrow_the_result_set_against_a_real_database: TELLURION_TEST_DATABASE_URL not set");
        return;
    };

    let table = "tellurion_postgis_live_test_advanced_cql2";
    seed_with_typed_attributes(&database_url, table).await;

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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    let collection: CollectionDecl = serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\n"
    ))
    .expect("valid CollectionDecl yaml");

    // LIKE: only 'a' and (population 20, active true) 'b' start with 'a'/'b';
    // seeded rows are named 'a', 'b', 'c' — 'a%' matches only 'a'.
    let like_filter = tellurion_core::filter::parse_text("name LIKE 'a%'").unwrap();
    let like_page = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(like_filter),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("like-filtered query succeeds");
    assert_eq!(like_page.features_geojson.len(), 1);
    assert_eq!(like_page.features_geojson[0]["properties"]["name"], "a");

    // NOT LIKE: everything except 'a'.
    let not_like_filter = tellurion_core::filter::parse_text("name NOT LIKE 'a%'").unwrap();
    let not_like_page = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(not_like_filter),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("not-like-filtered query succeeds");
    assert_eq!(not_like_page.features_geojson.len(), 2);

    // BETWEEN: population BETWEEN 15 AND 20 matches 'b' and 'c' (population
    // 20 each), not 'a' (population 10).
    let between_filter =
        tellurion_core::filter::parse_text("population BETWEEN 15 AND 20").unwrap();
    let between_page = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(between_filter),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("between-filtered query succeeds");
    assert_eq!(between_page.features_geojson.len(), 2);

    // IN: name IN ('a', 'c') matches exactly those two rows.
    let in_filter = tellurion_core::filter::parse_text("name IN ('a', 'c')").unwrap();
    let in_page = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(in_filter),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("in-filtered query succeeds");
    assert_eq!(in_page.features_geojson.len(), 2);
    let mut names: Vec<String> = in_page
        .features_geojson
        .iter()
        .map(|f| f["properties"]["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(names, vec!["a".to_string(), "c".to_string()]);

    // CASEI: an upper-cased literal still matches the lower-cased seeded
    // value via case-insensitive comparison.
    let casei_filter = Filter::CaseInsensitiveCompare {
        property: "name".to_string(),
        op: CaseInsensitiveCompareOp::Eq,
        value: "A".to_string(),
    };
    let casei_page = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(casei_filter),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("casei-filtered query succeeds");
    assert_eq!(casei_page.features_geojson.len(), 1);
    assert_eq!(casei_page.features_geojson[0]["properties"]["name"], "a");
}

/// `#106`: pins the non-ASCII case-folding gap that keeps
/// `case-insensitive-comparison` withheld from `CQL2_CONFORMANCE_CLASSES`
/// (see that constant's own doc in `tellurion_core::filter`). `CASEI`'s
/// compiled `lower()` call performs PostgreSQL's simple, length-preserving
/// case mapping, never the CQL2 standard's full Unicode case folding — the
/// two disagree exactly where a fold changes a string's length: German
/// sharp s (`ß`) case-folds to `ss` under full Unicode folding, but
/// `lower()` leaves it as `ß`, in every locale (verified directly against
/// PostgreSQL: this isn't a `C`/`POSIX`-only failure, it reproduces against
/// whatever locale this test database itself was initialized with).
/// `CASEI(name) = CASEI('STRASSE')` must therefore not match a row holding
/// `straße`.
#[tokio::test]
async fn casei_does_not_fold_full_unicode_case_equivalents_against_a_real_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping casei_does_not_fold_full_unicode_case_equivalents_against_a_real_database: TELLURION_TEST_DATABASE_URL not set");
        return;
    };

    let table = "tellurion_postgis_live_test_casei_unicode_gap";
    let client = test_harness::connect(&database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326) NOT NULL,
                 name text
             );"
        ),
    )
    .await
    .expect("creates the casei-unicode-gap test table");
    client
        .execute(
            &format!(
                "INSERT INTO {table} (geom, name) VALUES (ST_SetSRID(ST_MakePoint(10, 45), 4326), $1)"
            ),
            &[&"straße"],
        )
        .await
        .expect("seeds the straße row");
    client
        .batch_execute(&format!("ANALYZE {table};"))
        .await
        .expect("analyzes the casei-unicode-gap test table");

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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    let collection: CollectionDecl = serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\n"
    ))
    .expect("valid CollectionDecl yaml");

    let filter = Filter::CaseInsensitiveCompare {
        property: "name".to_string(),
        op: CaseInsensitiveCompareOp::Eq,
        value: "STRASSE".to_string(),
    };
    let page = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(filter),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("casei query succeeds even though it matches nothing");
    assert_eq!(
        page.features_geojson.len(),
        0,
        "CASEI(name) = CASEI('STRASSE') matched 'stra\u{df}e' — either this test \
         database's lower() now performs full Unicode case folding (re-evaluate \
         whether case-insensitive-comparison can be re-declared) or the seeded \
         value didn't round-trip as expected"
    );
}

/// `#33` follow-up, spatial functions beyond `S_INTERSECTS`, end to end
/// against a real database: `S_WITHIN` narrows the same three-point table
/// `cql2_filter_narrows_the_result_set_against_a_real_database` uses for
/// `S_INTERSECTS`.
#[tokio::test]
async fn new_spatial_predicates_narrow_the_result_set_against_a_real_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping new_spatial_predicates_narrow_the_result_set_against_a_real_database: TELLURION_TEST_DATABASE_URL not set");
        return;
    };

    let table = "tellurion_postgis_live_test_spatial_ops";
    seed(&database_url, table).await;

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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    let collection = collection(table);

    let within_filter = Filter::Spatial {
        property: "geom".to_string(),
        op: SpatialOp::Within,
        geometry: GeometryLiteral::Bbox([9.0, 44.0, 10.5, 45.5]),
    };
    let within_page = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(within_filter),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("s_within-filtered query succeeds");
    assert_eq!(within_page.features_geojson.len(), 1);
    assert_eq!(within_page.features_geojson[0]["properties"]["name"], "a");

    let disjoint_filter = Filter::Spatial {
        property: "geom".to_string(),
        op: SpatialOp::Disjoint,
        geometry: GeometryLiteral::Bbox([9.0, 44.0, 10.5, 45.5]),
    };
    let disjoint_page = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(disjoint_filter),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("s_disjoint-filtered query succeeds");
    assert_eq!(
        disjoint_page.features_geojson.len(),
        2,
        "'b' and 'c' both fall outside the bbox 'a' is within"
    );
}

/// OGC API Features Part 2 CRS by Reference, end to end against a real
/// database: a table whose geometry column's native SRID is 3857 (not
/// 4326), so `crs=CRS84` triggers a genuine `ST_Transform` — output
/// coordinates come back close to the original seeded lon/lat degrees rather
/// than the raw 3857 meter-scale numbers the default (omitted `crs`) path
/// still produces.
#[tokio::test]
async fn crs84_reprojects_a_non_4326_storage_srid_against_a_real_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping crs84_reprojects_a_non_4326_storage_srid_against_a_real_database: TELLURION_TEST_DATABASE_URL not set");
        return;
    };

    let table = "tellurion_postgis_live_test_crs84_reproject";
    seed_3857(&database_url, table).await;

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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    let mut collection: CollectionDecl = serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\n"
    ))
    .expect("valid CollectionDecl yaml");
    collection.srid = Some(3857);

    let default_page = features
        .items(&collection, &ItemsQuery::default())
        .await
        .expect("default (omitted crs) query succeeds");
    let default_coords = default_page.features_geojson[0]["geometry"]["coordinates"]
        .as_array()
        .unwrap()
        .clone();
    let default_x = default_coords[0].as_f64().unwrap();
    assert!(
        default_x.abs() > 1000.0,
        "an omitted crs must serve the raw 3857 meter-scale coordinate unchanged, got {default_x}"
    );

    let crs84_page = features
        .items(
            &collection,
            &ItemsQuery {
                crs: RequestedCrs::Crs84,
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("crs=CRS84 query succeeds");
    let coords = crs84_page.features_geojson[0]["geometry"]["coordinates"]
        .as_array()
        .unwrap()
        .clone();
    let lon = coords[0].as_f64().unwrap();
    let lat = coords[1].as_f64().unwrap();
    assert!((lon - 10.0).abs() < 0.01, "lon was {lon}");
    assert!((lat - 45.0).abs() < 0.01, "lat was {lat}");

    let item_id = crs84_page.features_geojson[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let item = features
        .item_with_crs(&collection, &item_id, None, RequestedCrs::Crs84)
        .await
        .expect("item_with_crs(Crs84) query succeeds")
        .expect("the seeded row exists");
    let item_coords = item["geometry"]["coordinates"].as_array().unwrap().clone();
    assert!((item_coords[0].as_f64().unwrap() - 10.0).abs() < 0.01);
    assert!((item_coords[1].as_f64().unwrap() - 45.0).abs() < 0.01);
}

/// `true` when a coordinate pair could be degrees at all — the crude,
/// units-only test that is enough to tell CRS84 from any projected CRS a
/// GeoJSON response could plausibly be in. EPSG:3857's easting for longitude
/// 10 is ~1.1 million metres, so nothing here is a near miss.
fn looks_like_degrees(x: f64, y: f64) -> bool {
    x.abs() <= 180.0 && y.abs() <= 90.0
}

/// `#227`'s decisive assertion, against a real database: the `Content-Crs`
/// URI a response would carry names the CRS whose **units the coordinates
/// are actually in** — not merely some CRS, and not whichever one the
/// request happened to mention.
///
/// A 4326 collection cannot tell the fix from the bug: every arm there is
/// CRS84 either way. This one is stored in EPSG:3857, so the two candidate
/// answers are a million apart, and PROJ — not a fixture — decides which
/// numbers come out. Three arms, each asked of the real driver and then of
/// `crs::content_crs_uri` with that driver's own capability:
///
/// - no `crs` at all — `RequestedCrs::Omitted` is defined as "no transform"
///   for **every** driver, PostGIS included (`sql::reprojected_geom_expr`'s
///   own `Omitted => geom` arm), so metres come back and the header must say
///   EPSG:3857. Before `#227` it said CRS84, and this is where that lie was
///   visible: degrees claimed over metre-scale numbers.
/// - `crs=CRS84` — a genuine `ST_Transform` here, so degrees come back and
///   CRS84 is the truth.
/// - `crs=<storage>` — metres again, named as such.
///
/// Written as an equivalence (`stamped == CRS84` iff the numbers could be
/// degrees) rather than as three expected strings, so it cannot be satisfied
/// by a header that merely changed.
#[tokio::test]
async fn the_content_crs_header_names_the_units_the_coordinates_are_in() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping the_content_crs_header_names_the_units_the_coordinates_are_in: TELLURION_TEST_DATABASE_URL not set");
        return;
    };

    let table = "tellurion_postgis_live_test_content_crs_units";
    seed_3857(&database_url, table).await;

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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    let mut collection: CollectionDecl = serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\n"
    ))
    .expect("valid CollectionDecl yaml");
    collection.srid = Some(3857);

    let crs_capable = features.crs_capable();
    let advertised = tellurion_core::crs::advertised_crs(collection.srid, crs_capable);

    for requested in [
        RequestedCrs::Omitted,
        RequestedCrs::Crs84,
        RequestedCrs::Storage,
    ] {
        assert!(
            tellurion_core::crs::can_serve(requested, collection.srid, crs_capable),
            "PostGIS can serve every arm; {requested:?} was refused"
        );

        let page = features
            .items(
                &collection,
                &ItemsQuery {
                    crs: requested,
                    ..ItemsQuery::default()
                },
            )
            .await
            .unwrap_or_else(|err| panic!("{requested:?} query succeeds: {err}"));
        let coords = page.features_geojson[0]["geometry"]["coordinates"]
            .as_array()
            .expect("the seeded row has coordinates");
        let x = coords[0].as_f64().unwrap();
        let y = coords[1].as_f64().unwrap();

        let stamped = tellurion_core::crs::content_crs_uri(requested, collection.srid, crs_capable);

        assert_eq!(
            stamped == tellurion_core::CRS84_URI,
            looks_like_degrees(x, y),
            "{requested:?}: Content-Crs said '{stamped}' over coordinates ({x}, {y}) — \
             the header must name the CRS the numbers are actually in"
        );
        assert!(
            advertised.contains(&stamped),
            "{requested:?}: stamped '{stamped}', which is outside this collection's own \
             advertised crs list {advertised:?}"
        );
    }
}

/// OGC API Features Part 2's classic axis-order trap, end to end against a
/// real database: a storage SRID of 4326 (the common case), requesting
/// `crs=<this collection's own storage CRS URI>` must flip the GeoJSON
/// coordinate order from CRS84's longitude-first to EPSG:4326-by-authority's
/// latitude-first — the same datum, opposite axis order.
#[tokio::test]
async fn storage_crs_flips_axis_order_for_a_4326_srid_against_a_real_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping storage_crs_flips_axis_order_for_a_4326_srid_against_a_real_database: TELLURION_TEST_DATABASE_URL not set");
        return;
    };

    let table = "tellurion_postgis_live_test_axis_order";
    seed(&database_url, table).await;

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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    let mut collection = collection(table);
    collection.srid = Some(4326);

    // Default (omitted crs): CRS84 order, longitude first — point 'a' was
    // seeded at (lon 10, lat 45).
    let default_page = features
        .items(
            &collection,
            &ItemsQuery {
                limit: 1,
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("default query succeeds");
    let default_coords = default_page.features_geojson[0]["geometry"]["coordinates"]
        .as_array()
        .unwrap()
        .clone();
    assert!((default_coords[0].as_f64().unwrap() - 10.0).abs() < f64::EPSILON);
    assert!((default_coords[1].as_f64().unwrap() - 45.0).abs() < f64::EPSILON);

    // crs=<storage CRS> (EPSG:4326 by authority): latitude first.
    let flipped_page = features
        .items(
            &collection,
            &ItemsQuery {
                limit: 1,
                crs: RequestedCrs::Storage,
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("crs=<storage> query succeeds");
    let flipped_coords = flipped_page.features_geojson[0]["geometry"]["coordinates"]
        .as_array()
        .unwrap()
        .clone();
    assert!(
        (flipped_coords[0].as_f64().unwrap() - 45.0).abs() < f64::EPSILON,
        "expected latitude first, coords were {flipped_coords:?}"
    );
    assert!((flipped_coords[1].as_f64().unwrap() - 10.0).abs() < f64::EPSILON);
}

/// `bbox-crs` against a storage SRID of 4326, at the driver layer: by the
/// time a `bbox` reaches `FeatureSource::items`, its four numbers are
/// already axis-normalized to longitude-first order — the axis swap itself
/// (`crs::swap_bbox_axes`, unit-tested in `tellurion_core::crs` and exercised
/// end to end over real HTTP query strings by `tellurion-server`'s own live
/// tests) is `tellurion-features`' handler's job, upstream of this driver.
/// What this crate owns and must prove instead: `bbox_crs: Crs84` and
/// `bbox_crs: Storage` build envelopes that select the identical rows for a
/// storage SRID of 4326, since both paths bind the same (already-normalized)
/// four numbers into the same SRID.
#[tokio::test]
async fn bbox_crs_storage_and_crs84_select_the_same_rows_for_a_4326_srid_against_a_real_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping bbox_crs_axis_order_both_ways_select_the_same_rows_against_a_real_database: TELLURION_TEST_DATABASE_URL not set");
        return;
    };

    let table = "tellurion_postgis_live_test_bbox_crs_axis";
    seed(&database_url, table).await;

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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    let mut collection = collection(table);
    collection.srid = Some(4326);

    // CRS84 order (longitude first) covering only seeded point 'a' (10, 45).
    let crs84_bbox = [9.0, 44.0, 10.5, 45.5];
    let crs84_page = features
        .items(
            &collection,
            &ItemsQuery {
                bbox: Some(crs84_bbox),
                bbox_crs: RequestedCrs::Crs84,
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("bbox-crs=CRS84 query succeeds");
    assert_eq!(crs84_page.features_geojson.len(), 1);
    assert_eq!(crs84_page.features_geojson[0]["properties"]["name"], "a");

    // Same four numbers, `bbox_crs: Storage` instead — for a storage SRID of
    // 4326 this must select the same row: `bbox_envelope_sql`'s `Storage`
    // arm builds the envelope directly at `storage_srid` (4326 here), the
    // same SRID the `Crs84` arm above also lands on since the storage SRID
    // already is 4326 (no transform needed either way).
    let storage_page = features
        .items(
            &collection,
            &ItemsQuery {
                bbox: Some(crs84_bbox),
                bbox_crs: RequestedCrs::Storage,
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("bbox-crs=<storage> query succeeds");
    assert_eq!(
        storage_page.features_geojson.len(),
        1,
        "bbox-crs=<storage> must select the same row bbox-crs=CRS84 did for a 4326 storage SRID"
    );
    assert_eq!(storage_page.features_geojson[0]["properties"]["name"], "a");
}

/// **The `filter-crs` test that can only pass for the right reason** (`#217`,
/// OGC API — Features Part 3: Filtering 19-079r2 Requirement 8,
/// `/req/filter/filter-crs-param`).
///
/// A storage SRID of 4326 — the common case, and the one where the two
/// candidate readings of a filter geometry differ by axis order alone, not
/// by a single coordinate value:
///
/// - CRS84 is longitude-before-latitude (Requirement 7's default, and what
///   this crate's compiler has always assumed).
/// - EPSG:4326 referenced by authority — the URI a `filter-crs` names when
///   it names this collection's own storage CRS — is
///   latitude-before-longitude.
///
/// So the same four numbers are two different rectangles, and the test is a
/// full 2x2 truth table over `(box, filter_crs)` where **every cell differs
/// from its row-mate**: the longitude-first box selects the seeded point
/// under `Crs84` and nothing under `Storage`; the latitude-first box (the
/// same rectangle with each pair swapped) selects nothing under `Crs84` and
/// the same seeded point under `Storage`. A fixture that happened to give
/// the same answer either way cannot produce that table, and neither can an
/// implementation that accepts `filter-crs` and ignores it — which is
/// exactly what this driver did before `#217`, returning the wrong features
/// under a 200.
#[tokio::test]
async fn filter_crs_axis_order_selects_different_rows_for_a_4326_srid_against_a_real_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping filter_crs_axis_order_selects_different_rows_for_a_4326_srid_against_a_real_database: TELLURION_TEST_DATABASE_URL not set");
        return;
    };

    let table = "tellurion_postgis_live_test_filter_crs_axis";
    seed(&database_url, table).await;

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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    assert!(
        features.filter_crs_capable(),
        "PostGIS must declare FeatureSource::filter_crs_capable() before this test means anything"
    );
    let mut collection = collection(table);
    collection.srid = Some(4326);

    // `seed` puts point 'a' at longitude 10, latitude 45. These two boxes
    // are the SAME rectangle on the ground, written in the two axis orders.
    let lon_lat_box = [9.0, 44.0, 10.5, 45.5];
    let lat_lon_box = [44.0, 9.0, 45.5, 10.5];

    async fn matching_names(
        features: &std::sync::Arc<dyn tellurion_core::FeatureSource>,
        collection: &CollectionDecl,
        bbox: [f64; 4],
        filter_crs: RequestedCrs,
    ) -> Vec<String> {
        let page = features
            .items(
                collection,
                &ItemsQuery {
                    filter: Some(Filter::Intersects {
                        property: "geom".to_string(),
                        geometry: GeometryLiteral::Bbox(bbox),
                    }),
                    filter_crs,
                    ..ItemsQuery::default()
                },
            )
            .await
            .unwrap_or_else(|err| panic!("items({bbox:?}, {filter_crs:?}) succeeds: {err}"));
        page.features_geojson
            .iter()
            .map(|f| f["properties"]["name"].as_str().unwrap().to_string())
            .collect()
    }

    // filter-crs=CRS84 reads the numbers longitude-first.
    assert_eq!(
        matching_names(&features, &collection, lon_lat_box, RequestedCrs::Crs84).await,
        vec!["a".to_string()],
        "a longitude-first box over (10, 45) must select point 'a' when filter-crs is CRS84"
    );
    assert!(
        matching_names(&features, &collection, lat_lon_box, RequestedCrs::Crs84)
            .await
            .is_empty(),
        "the latitude-first box read as CRS84 covers longitudes 44-45.5, where nothing is seeded"
    );

    // filter-crs=<this collection's storage CRS> (EPSG:4326 by authority)
    // reads the identical numbers latitude-first — and the answers swap.
    assert!(
        matching_names(&features, &collection, lon_lat_box, RequestedCrs::Storage)
            .await
            .is_empty(),
        "the longitude-first box read as EPSG:4326-by-authority covers longitudes 44-45.5, \
         where nothing is seeded — if this still selects 'a', filter-crs was ignored"
    );
    assert_eq!(
        matching_names(&features, &collection, lat_lon_box, RequestedCrs::Storage).await,
        vec!["a".to_string()],
        "a latitude-first box over (45, 10) must select point 'a' when filter-crs names \
         EPSG:4326 by authority"
    );
}

/// The other half of Requirement 8 against a real database: a genuinely
/// projected storage CRS, where honouring `filter-crs` means a real
/// `ST_Transform` rather than an axis swap. `seed_3857` stores one point at
/// longitude 10 / latitude 45, reprojected into EPSG:3857 metres.
///
/// - `filter-crs=CRS84` with a degree-valued box over that point must select
///   it — the literal is reprojected into 3857 before the comparison.
/// - `filter-crs=<storage>` with the *same numbers* reads them as 3857
///   metres, which lands within ~10 m of the origin off the Gulf of Guinea,
///   nowhere near the seeded point — so it must select nothing.
///
/// Different CRS, different answer, same fixture and same four numbers.
#[tokio::test]
async fn filter_crs_reprojects_a_degree_literal_into_a_3857_storage_against_a_real_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping filter_crs_reprojects_a_degree_literal_into_a_3857_storage_against_a_real_database: TELLURION_TEST_DATABASE_URL not set");
        return;
    };

    let table = "tellurion_postgis_live_test_filter_crs_reproject";
    seed_3857(&database_url, table).await;

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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    let mut collection: CollectionDecl = serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\n"
    ))
    .expect("valid CollectionDecl yaml");
    collection.srid = Some(3857);

    let degrees_box = [9.0, 44.0, 10.5, 45.5];
    let intersects = |filter_crs| ItemsQuery {
        filter: Some(Filter::Intersects {
            property: "geom".to_string(),
            geometry: GeometryLiteral::Bbox(degrees_box),
        }),
        filter_crs,
        ..ItemsQuery::default()
    };

    let crs84 = features
        .items(&collection, &intersects(RequestedCrs::Crs84))
        .await
        .expect("filter-crs=CRS84 against a 3857 storage succeeds");
    assert_eq!(
        crs84.features_geojson.len(),
        1,
        "a CRS84 degree box over the seeded point must select it once the literal is \
         reprojected into the 3857 storage CRS"
    );

    let storage = features
        .items(&collection, &intersects(RequestedCrs::Storage))
        .await
        .expect("filter-crs=<storage> against a 3857 storage succeeds");
    assert!(
        storage.features_geojson.is_empty(),
        "the same four numbers read as EPSG:3857 metres describe a ~1.5 m box next to the \
         projection origin, which the seeded point is nowhere near"
    );

    // `#247`: the third row of the same table, and the one that used to be a
    // `500`. No `filter-crs` at all is Requirement 7 — the geometries SHALL be
    // processed in CRS84 — which is the same statement about the same numbers
    // the `Crs84` row above makes, so it must produce the same answer. See
    // `a_default_spatial_filter_is_processed_in_crs84_against_a_3857_storage`
    // for the full argument and the error text this replaces.
    let omitted = features
        .items(&collection, &intersects(RequestedCrs::Omitted))
        .await
        .expect("a filter with no filter-crs at all against a 3857 storage succeeds");
    assert_eq!(
        omitted.features_geojson.len(),
        1,
        "Requirement 7 makes an omitted filter-crs mean CRS84, so it must select exactly what \
         the explicit CRS84 row selected"
    );
}

/// **The decisive `#247` test.** An ordinary, fully conformant request — a
/// spatial `filter` and **no** `filter-crs` parameter anywhere — against a
/// collection whose PostGIS storage is projected (EPSG:3857).
///
/// Before this slice that request returned a `500`. The literal was bound at
/// CRS84 and handed to `ST_Intersects` untransformed, so PostgreSQL saw a 4326
/// polygon beside a 3857 column and refused outright:
///
/// ```text
/// ST_Intersects: Operation on mixed SRID geometries (Point, 3857) != (Polygon, 4326)
/// ```
///
/// OGC API — Features Part 3 Requirement 7 (`/req/filter/filter-crs-wgs84`)
/// says the server SHALL *process* such a filter's geometries in CRS84.
/// Erroring is not processing them, so the byte-for-byte behaviour `#217`
/// preserved on this branch was an error page — the whole of `#247`'s
/// argument, executed here against a real PostGIS rather than asserted about
/// SQL text.
///
/// Three assertions, and the first two are the ones that fail without the fix:
///
/// 1. the request **succeeds**, and the failure message carries PostGIS's own
///    error text so a regression names itself;
/// 2. it selects the seeded point — the transform is real, not an SRID relabel
///    that would silently select nothing;
/// 3. the answer is identical to the same request carrying an explicit
///    `filter-crs=CRS84`, which is the equivalence Requirement 7 asserts.
///
/// The mirror-image rule — a CRS84 storage is untouched — is pinned in
/// `sql.rs`'s own
/// `items_plan_with_no_filter_crs_is_byte_for_byte_unchanged_on_a_4326_srid`,
/// at the level where "unchanged" can be checked character by character.
#[tokio::test]
async fn a_default_spatial_filter_is_processed_in_crs84_against_a_3857_storage() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping a_default_spatial_filter_is_processed_in_crs84_against_a_3857_storage: TELLURION_TEST_DATABASE_URL not set");
        return;
    };

    let table = "tellurion_postgis_live_test_default_filter_crs_3857";
    seed_3857(&database_url, table).await;

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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    let mut collection: CollectionDecl = serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\n"
    ))
    .expect("valid CollectionDecl yaml");
    collection.srid = Some(3857);

    // Exactly what a client sends when it has read Part 3 and nothing else:
    // CQL2-text, degrees, no CRS parameter of any kind. Parsed through the
    // real grammar so this exercises the whole wire path, not a hand-built
    // `Filter`.
    let filter = tellurion_core::filter::parse_text("S_INTERSECTS(geom, BBOX(9, 44, 10.5, 45.5))")
        .expect("the filter every Part 3 client can write parses");
    let default_query = ItemsQuery {
        filter: Some(filter.clone()),
        ..ItemsQuery::default()
    };
    assert_eq!(
        default_query.filter_crs,
        RequestedCrs::Omitted,
        "this test is only about the request that names no filter-crs at all"
    );

    let page = features
        .items(&collection, &default_query)
        .await
        .unwrap_or_else(|err| {
            panic!(
                "a spatial filter with no filter-crs must not fail against a projected storage; \
                 before `#247` this was the mixed-SRID refusal: {err}"
            )
        });
    assert_eq!(
        page.features_geojson.len(),
        1,
        "the degree box covers the seeded point once its coordinates are transformed into the \
         3857 storage CRS; selecting nothing would mean the SRID was relabelled, not converted"
    );
    assert_eq!(page.features_geojson[0]["properties"]["name"], "a");

    // Requirement 7's default and Requirement 8's CRS84 value say the same
    // thing about the same numbers, so they must select the same rows.
    let explicit = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(filter),
                filter_crs: RequestedCrs::Crs84,
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("filter-crs=CRS84 against a 3857 storage succeeds");
    assert_eq!(
        explicit.features_geojson, page.features_geojson,
        "an omitted filter-crs and an explicit CRS84 one are the same request"
    );
}

/// **The decisive `#255` test.** An ordinary, fully conformant request — a
/// `bbox` and **no** `bbox-crs` parameter anywhere — against a collection
/// whose PostGIS storage is projected (EPSG:3857).
///
/// Before this slice that request returned a `200` with the wrong rows. The
/// envelope was built at CRS84 and compared to a 3857 column with no
/// transform, and unlike `ST_Intersects` (which raised the mixed-SRID `500`
/// `#247` was named for) the `&&` operator does not object:
///
/// ```sql
/// SELECT ST_SetSRID(ST_MakePoint(1,1),3857) && ST_MakeEnvelope(0,0,2,2,4326);
/// -- t
/// ```
///
/// So degrees were compared against metres, silently. Part 1 Requirement 23
/// (`/req/core/fc-bbox-definition`) clause C fixes those numbers as CRS84 and
/// Requirement 24 (`/req/core/fc-bbox-response`) clause A says "Only features
/// that have a spatial geometry that intersects the bounding box SHALL be part
/// of the result set". This request satisfied neither, under a status code
/// that claimed it had.
///
/// [`seed_3857_disjoint_pair`] is what makes this decisive rather than merely
/// different: the two readings of the same four numbers select **one row
/// each, and never the same one**. So the assertions are not "some rows came
/// back" but an exact identity in both directions —
///
/// 1. the request selects `in_box`, the row the box geographically contains;
/// 2. it does **not** select `near_origin`, the row the old untransformed
///    reading selected instead — spelled as its own assertion, because that is
///    the wrong answer this issue exists for, not merely an absence;
/// 3. the answer is identical to the same request carrying an explicit
///    `bbox-crs=CRS84`, which is Part 2 Abstract Test 10
///    (`/conf/crs/bbox-crs-parameter-default`) verbatim: "send the same
///    request, but with no `bbox-crs` parameter … verify that the responses
///    include the same features";
/// 4. and reading those numbers as the *storage* CRS instead — the invented
///    default rule 1 forbids, and the one a client can still ask for by name —
///    selects `near_origin`, proving the wrong answer is genuinely reachable
///    from this fixture and that assertion 2 is not vacuous.
///
/// The mirror-image rule — a CRS84 storage is untouched — is pinned in
/// `sql.rs`'s own
/// `items_plan_with_no_bbox_crs_is_byte_for_byte_unchanged_on_a_4326_srid`, at
/// the level where "unchanged" can be checked character by character.
#[tokio::test]
async fn a_default_bbox_selects_the_right_rows_against_a_3857_storage() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping a_default_bbox_selects_the_right_rows_against_a_3857_storage: TELLURION_TEST_DATABASE_URL not set");
        return;
    };

    let table = "tellurion_postgis_live_test_default_bbox_crs_3857";
    seed_3857_disjoint_pair(&database_url, table).await;

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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    let mut collection: CollectionDecl = serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\n"
    ))
    .expect("valid CollectionDecl yaml");
    collection.srid = Some(3857);

    // Exactly what a client sends when it has read Part 1 and nothing else:
    // four degrees, no CRS parameter of any kind.
    let degree_bbox = [9.0, 9.0, 12.0, 12.0];
    let default_query = ItemsQuery {
        bbox: Some(degree_bbox),
        ..ItemsQuery::default()
    };
    assert_eq!(
        default_query.bbox_crs,
        RequestedCrs::Omitted,
        "this test is only about the request that names no bbox-crs at all"
    );

    let page = features
        .items(&collection, &default_query)
        .await
        .expect("a bbox with no bbox-crs against a projected storage succeeds");
    let names: Vec<&str> = page
        .features_geojson
        .iter()
        .map(|f| f["properties"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["in_box"],
        "the degree box covers exactly the point it geographically contains, once its own \
         coordinates are transformed into the 3857 storage CRS"
    );
    assert!(
        !names.contains(&"near_origin"),
        "and never the point whose METRE coordinates merely look like degrees — selecting it \
         is precisely the `200` with the wrong rows `#255` was opened for"
    );

    // Part 2 Abstract Test 10: an omitted `bbox-crs` and an explicit CRS84
    // one are the same request, so they must include the same features.
    let explicit = features
        .items(
            &collection,
            &ItemsQuery {
                bbox: Some(degree_bbox),
                bbox_crs: RequestedCrs::Crs84,
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("bbox-crs=CRS84 against a 3857 storage succeeds");
    assert_eq!(
        explicit.features_geojson, page.features_geojson,
        "an omitted bbox-crs and an explicit CRS84 one are the same request"
    );

    // ...and the reading a client CAN still ask for by name selects the other
    // row, which is what proves the fixture can express the wrong answer at
    // all. `bbox-crs=<storage>` says "these numbers are metres", and read as
    // metres 9..12 is a three-metre box around the origin.
    let storage = features
        .items(
            &collection,
            &ItemsQuery {
                bbox: Some(degree_bbox),
                bbox_crs: RequestedCrs::Storage,
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("bbox-crs=<storage> against a 3857 storage succeeds");
    let storage_names: Vec<&str> = storage
        .features_geojson
        .iter()
        .map(|f| f["properties"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        storage_names,
        ["near_origin"],
        "the same four numbers read as EPSG:3857 metres describe a box beside the origin, and \
         select the other row — the untransformed answer, now reachable only by asking for it"
    );
}

/// `#247`'s reach past the items lane: a `#34` ABAC grant filter is the one
/// spatial filter a *deployment* authors rather than a client, and it travels
/// with no `filter-crs` by construction — there is no client parameter to
/// carry one. Against a projected storage it hit the identical mixed-SRID
/// `500`, on the two lanes that have no other filter surface to merge into
/// (`FeatureSource::item`, and the MVT tile lane).
///
/// Fixing only the items lane would have been worse than fixing nothing there:
/// the same deployment's own grant would transform on `/items` (where it is
/// AND-merged into the client's query and compiled with the collection's SRID)
/// and 500 on `/items/{id}`, for one collection, with no request parameter
/// distinguishing the two.
#[tokio::test]
async fn a_grant_filter_with_no_filter_crs_is_processed_in_crs84_against_a_3857_storage() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping a_grant_filter_with_no_filter_crs_is_processed_in_crs84_against_a_3857_storage: TELLURION_TEST_DATABASE_URL not set");
        return;
    };

    let table = "tellurion_postgis_live_test_grant_filter_3857";
    seed_3857(&database_url, table).await;

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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    let mut collection: CollectionDecl = serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\n"
    ))
    .expect("valid CollectionDecl yaml");
    collection.srid = Some(3857);

    let grant = Filter::Intersects {
        property: "geom".to_string(),
        geometry: GeometryLiteral::Bbox([9.0, 44.0, 10.5, 45.5]),
    };
    let item = features
        .item(&collection, "1", Some(&grant))
        .await
        .unwrap_or_else(|err| {
            panic!("a grant filter must not fail against a projected storage: {err}")
        });
    let item = item.expect("the grant's CRS84 box covers the seeded point once transformed");
    assert_eq!(item["properties"]["name"], "a");

    // ...and a grant that genuinely excludes the row still excludes it, so the
    // transform is not simply making every grant match.
    let elsewhere = Filter::Intersects {
        property: "geom".to_string(),
        geometry: GeometryLiteral::Bbox([0.0, 0.0, 1.0, 1.0]),
    };
    assert!(
        features
            .item(&collection, "1", Some(&elsewhere))
            .await
            .expect("an excluding grant filter still succeeds")
            .is_none(),
        "a CRS84 box over the Gulf of Guinea must not match a point in northern Italy"
    );
}

/// WKT geometry literal predicates (`#33` epic completion) against a real
/// database: point-in-polygon via `S_WITHIN` and polygon-intersects via
/// `S_INTERSECTS`, both parsed from CQL2-text through the real WKT grammar
/// (`GeometryLiteral::Wkt`) and compiled to a parameter-bound
/// `ST_GeomFromText` call — proves the literal round-trips through the text
/// parser, `sql::compile_filter`, and a real `ST_Within`/`ST_Intersects`
/// evaluation, not just that it compiles to plausible-looking SQL text.
#[tokio::test]
async fn wkt_geometry_literal_predicates_against_a_real_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping wkt_geometry_literal_predicates_against_a_real_database: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_cql2_wkt_geometry";
    seed(&database_url, table).await;

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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    let collection = collection(table);

    // Point-in-polygon: a WKT POLYGON literal tightly enclosing only the
    // first seeded point 'a' (10, 45), parsed from CQL2-text.
    let within_filter = tellurion_core::filter::parse_text(
        "S_WITHIN(geom, POLYGON((9 44, 10.5 44, 10.5 45.5, 9 45.5, 9 44)))",
    )
    .unwrap();
    let within_page = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(within_filter),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("s_within-filtered query succeeds");
    assert_eq!(within_page.features_geojson.len(), 1);
    assert_eq!(within_page.features_geojson[0]["properties"]["name"], "a");

    // Polygon intersects: a larger WKT POLYGON literal spanning seeded
    // points 'b' (11, 46) and 'c' (12, 47) but not 'a'.
    let intersects_filter = tellurion_core::filter::parse_text(
        "S_INTERSECTS(geom, POLYGON((10.5 45.5, 13 45.5, 13 48, 10.5 48, 10.5 45.5)))",
    )
    .unwrap();
    let intersects_page = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(intersects_filter),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("s_intersects-filtered query succeeds");
    assert_eq!(intersects_page.features_geojson.len(), 2);
    let names: Vec<&str> = intersects_page.features_geojson[..]
        .iter()
        .map(|f| f["properties"]["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"b") && names.contains(&"c"),
        "names were: {names:?}"
    );

    // A WKT literal built directly as a `Filter::Spatial` (bypassing the
    // text parser) proves `sql::geometry_literal_expr`'s `Wkt` arm alone,
    // independent of tokenizer/grammar correctness.
    let hand_built = Filter::Intersects {
        property: "geom".to_string(),
        geometry: GeometryLiteral::Wkt(WktGeometry::Point([10.0, 45.0])),
    };
    let point_page = features
        .items(
            &collection,
            &ItemsQuery {
                filter: Some(hand_built),
                ..ItemsQuery::default()
            },
        )
        .await
        .expect("hand-built wkt point filter succeeds");
    assert_eq!(point_page.features_geojson.len(), 1);
    assert_eq!(point_page.features_geojson[0]["properties"]["name"], "a");
}

/// Temporal operator truth table (`#33` epic completion) against a real
/// database: the twelve new `T_*` operators, each parsed from CQL2-text and
/// compiled to the Allen-relation SQL `sql::temporal_op_sql` derives,
/// exercised against three fixture rows at known instants (`seed`: 'a' =
/// 2020-01-01, 'b' = 2020-06-01, 'c' = 2021-01-01) with a literal interval
/// exactly spanning 'a' through 'c'. Proves both the satisfiable relations
/// (Meets/MetBy/Starts/Finishes/Intersects/Disjoint) select the rows Allen's
/// algebra predicts, and the always-false-for-an-instant-column relations
/// (Overlaps and friends — see `sql::temporal_op_sql`'s own doc) genuinely
/// select nothing against real data, not just in an isolated SQL-text
/// assertion.
#[tokio::test]
async fn new_temporal_operators_truth_table_against_a_real_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping new_temporal_operators_truth_table_against_a_real_database: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_cql2_temporal_ops";
    seed(&database_url, table).await;

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
    let features = driver
        .feature_source()
        .expect("driver exposes FeatureSource");
    let collection = collection(table);

    const START: &str = "2020-01-01T00:00:00Z"; // row 'a'
    const END: &str = "2021-01-01T00:00:00Z"; // row 'c'

    async fn names_matching(
        features: &dyn tellurion_core::FeatureSource,
        collection: &CollectionDecl,
        filter: Filter,
    ) -> Vec<String> {
        let page = features
            .items(
                collection,
                &ItemsQuery {
                    filter: Some(filter),
                    ..ItemsQuery::default()
                },
            )
            .await
            .expect("temporal-filtered query succeeds");
        let mut names: Vec<String> = page.features_geojson[..]
            .iter()
            .map(|f| f["properties"]["name"].as_str().unwrap().to_string())
            .collect();
        names.sort();
        names
    }

    let interval = TemporalValue::Interval(START.to_string(), END.to_string());

    // Meets: property equals the interval's start -> row 'a' only.
    assert_eq!(
        names_matching(
            features.as_ref(),
            &collection,
            Filter::Temporal {
                property: "observed_at".to_string(),
                op: TemporalOp::Meets,
                value: interval.clone(),
            }
        )
        .await,
        vec!["a"]
    );

    // Met-by: property equals the interval's end -> row 'c' only.
    assert_eq!(
        names_matching(
            features.as_ref(),
            &collection,
            Filter::Temporal {
                property: "observed_at".to_string(),
                op: TemporalOp::MetBy,
                value: interval.clone(),
            }
        )
        .await,
        vec!["c"]
    );

    // Starts: property equals start and precedes end -> row 'a' only.
    assert_eq!(
        names_matching(
            features.as_ref(),
            &collection,
            Filter::Temporal {
                property: "observed_at".to_string(),
                op: TemporalOp::Starts,
                value: interval.clone(),
            }
        )
        .await,
        vec!["a"]
    );

    // Finishes: property equals end and follows start -> row 'c' only.
    assert_eq!(
        names_matching(
            features.as_ref(),
            &collection,
            Filter::Temporal {
                property: "observed_at".to_string(),
                op: TemporalOp::Finishes,
                value: interval.clone(),
            }
        )
        .await,
        vec!["c"]
    );

    // Intersects: property falls within [start, end] inclusive -> all three
    // rows ('a', 'b', 'c' are all within 2020-01-01..=2021-01-01).
    assert_eq!(
        names_matching(
            features.as_ref(),
            &collection,
            Filter::Temporal {
                property: "observed_at".to_string(),
                op: TemporalOp::Intersects,
                value: interval.clone(),
            }
        )
        .await,
        vec!["a", "b", "c"]
    );

    // Disjoint: the complement of Intersects -> no rows.
    assert_eq!(
        names_matching(
            features.as_ref(),
            &collection,
            Filter::Temporal {
                property: "observed_at".to_string(),
                op: TemporalOp::Disjoint,
                value: interval.clone(),
            }
        )
        .await,
        Vec::<String>::new()
    );

    // Equals against a degenerate (instant) interval matching row 'a'
    // exactly -> row 'a' only.
    assert_eq!(
        names_matching(
            features.as_ref(),
            &collection,
            Filter::Temporal {
                property: "observed_at".to_string(),
                op: TemporalOp::Equals,
                value: TemporalValue::Instant(START.to_string()),
            }
        )
        .await,
        vec!["a"]
    );

    // Overlaps/OverlappedBy/StartedBy/FinishedBy/Contains: none of these can
    // ever match an instant-valued column against a proper interval (see
    // `sql::temporal_op_sql`'s own doc) -> no rows, for every fixture row.
    for op in [
        TemporalOp::Overlaps,
        TemporalOp::OverlappedBy,
        TemporalOp::StartedBy,
        TemporalOp::FinishedBy,
        TemporalOp::Contains,
    ] {
        assert_eq!(
            names_matching(
                features.as_ref(),
                &collection,
                Filter::Temporal {
                    property: "observed_at".to_string(),
                    op,
                    value: interval.clone(),
                }
            )
            .await,
            Vec::<String>::new(),
            "op {op:?} must never match an instant-valued column"
        );
    }
}

// -- geometry_variants (`#104`) ----------------------------------------------

/// A table with two real geometry columns (`geom`, `geom_z6`), each holding
/// one point at a location deliberately far from the other's — the shape
/// [`geometry_variants_selection_...`] below needs to prove which column a
/// tile query actually reads by *presence*, not by decoding MVT bytes: a
/// tile covering one point is empty if the driver reads the column that
/// doesn't have a point there. Also serves the ambiguity test below
/// unmodified — two real geometry columns on one table, no `geometry:` pin,
/// is exactly PostGIS's `geometry_columns` view returning two rows for one
/// table (`#104`, point 1).
async fn seed_two_geometry_columns(database_url: &str, table: &str) {
    let client = test_harness::connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326) NOT NULL,
                 geom_z6 geometry(Point, 4326) NOT NULL
             );
             INSERT INTO {table} (geom, geom_z6) VALUES (
                 ST_SetSRID(ST_MakePoint(10, 45), 4326),
                 ST_SetSRID(ST_MakePoint(-170, -80), 4326)
             );
             ANALYZE {table};"
        ),
    )
    .await
    .expect("seeds the two-geometry-column test table");
}

/// A two-column physical shape whose rows carry deliberately different CRS
/// metadata. This isolates descriptor selection from the geometry-variant
/// fixture above, where equal SRIDs could hide choosing the first catalog row.
async fn seed_two_geometry_columns_with_distinct_srids(database_url: &str, table: &str) {
    let client = test_harness::connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table};
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom_a_4326 geometry(Point, 4326) NOT NULL,
                 geom_b_3857 geometry(Point, 3857) NOT NULL
             );
             INSERT INTO {table} (geom_a_4326, geom_b_3857) VALUES (
                 ST_SetSRID(ST_MakePoint(10, 45), 4326),
                 ST_Transform(ST_SetSRID(ST_MakePoint(10, 45), 4326), 3857)
             );
             ANALYZE {table};"
        ),
    )
    .await
    .expect("seeds the distinct-SRID two-geometry-column test table");
}

/// Standard slippy-map tile math (the same formula OSM/`ST_TileEnvelope`
/// both implement): which `(x, y)` tile at zoom `z` contains `(lon, lat)`.
/// Self-contained here rather than reused from `tellurion-tiles::mercator`
/// (not a dependency of this crate's tests) — a handful of lines, easy to
/// eyeball against the well-known formula.
fn lonlat_to_tile(lon: f64, lat: f64, z: u8) -> (u32, u32) {
    let n = 2f64.powi(z as i32);
    let x = ((lon + 180.0) / 360.0 * n).floor() as u32;
    let lat_rad = lat.to_radians();
    let y = ((1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI) / 2.0 * n)
        .floor() as u32;
    (x, y)
}

/// `#104`, point 1, against a real database: a table with two genuine
/// geometry columns and no `geometry:` pin must refuse to boot rather than
/// silently bind to whichever one `geometry_columns` happens to report
/// first — the live counterpart of `validate_catalog_fails_fast_when_a_
/// table_reports_two_geometry_columns_and_none_is_pinned` (`tellurion-core`'s
/// own fake-catalog unit test).
#[tokio::test]
async fn router_refuses_boot_when_the_catalog_reports_two_geometry_columns_and_none_is_pinned_against_a_real_database(
) {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping router_refuses_boot_when_the_catalog_reports_two_geometry_columns_and_none_is_pinned_against_a_real_database: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_ambiguous_geometry";
    seed_two_geometry_columns(&database_url, table).await;

    unsafe {
        env::set_var(URL_ENV_VAR, &database_url);
    }

    let config: AppConfig = serde_yaml::from_str(&format!(
        "storages: [ {{ id: main, driver: postgis, url_env: {URL_ENV_VAR} }} ]\n\
         tenants: [ {{ id: public }} ]\n\
         catalogs: [ {{ id: default, tenant: public }} ]\n\
         collections:\n  - id: {table}\n    catalog: default\n    storage: main\n"
    ))
    .expect("valid AppConfig yaml with no geometry pin");
    config
        .validate()
        .expect("referential-integrity validation never required a geometry pin");

    let mut registry = Registry::new();
    registry.register(Arc::new(PostgisDriverFactory::new(60)));
    let router = Router::build(&config, &registry).expect("router builds");

    let error = router
        .validate_catalog()
        .await
        .expect_err("two real geometry columns with no pin must refuse boot rather than guess");
    let message = error.to_string();
    assert!(
        message.contains(table),
        "message must name the table: {message}"
    );
    assert!(
        message.contains("geom_z6"),
        "message must name one candidate column: {message}"
    );
    assert!(
        message.contains("geometry"),
        "message must point at the 'geometry' config key: {message}"
    );
}

/// `#129` against a real PostGIS catalog: pinning the second geometry column
/// must select its physical row for descriptor SRID/type metadata, rather
/// than inheriting the first `geometry_columns` row's 4326/POINT facts.
#[tokio::test]
async fn router_descriptor_uses_the_pinned_second_geometry_column_against_a_real_database() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping router_descriptor_uses_the_pinned_second_geometry_column_against_a_real_database: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_pinned_descriptor";
    seed_two_geometry_columns_with_distinct_srids(&database_url, table).await;

    unsafe {
        env::set_var(URL_ENV_VAR, &database_url);
    }

    let config: AppConfig = serde_yaml::from_str(&format!(
        r#"storages: [ {{ id: main, driver: postgis, url_env: {URL_ENV_VAR} }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: {table}
    catalog: default
    storage: main
    geometry: geom_b_3857
    pk: id
"#
    ))
    .expect("valid AppConfig yaml with a pin to the second geometry column");
    config.validate().expect("valid config shape");

    let mut registry = Registry::new();
    registry.register(Arc::new(PostgisDriverFactory::new(60)));
    let router = Router::build(&config, &registry).expect("router builds");
    router
        .validate_catalog()
        .await
        .expect("the pinned geometry column exists and must validate");

    let descriptor = router
        .collection_descriptor("public", "default", table)
        .await
        .expect("the eager sweep caches a descriptor");
    assert_eq!(descriptor.geometry.as_deref(), Some("geom_b_3857"));
    assert_eq!(descriptor.srid, Some(3857));
    assert_eq!(descriptor.geometry_type.as_deref(), Some("POINT"));
}

/// `#104`, points 2/3/5, against a real database: `geom_z6` is declared as a
/// variant for zooms 0-6; the tiles lane must read `geom_z6` at a zoom it
/// covers and fall back to the base `geom` column outside that range.
/// Proven by presence rather than by decoding MVT bytes — `geom` and
/// `geom_z6` hold one point each, at locations far enough apart that the
/// same tile coordinate can never contain both, so whichever column the
/// driver actually queried is visible in which tile comes back populated.
#[tokio::test]
async fn mvt_tile_selects_the_zoom_scoped_geometry_variant_and_falls_back_to_the_base_column_against_a_real_database(
) {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping mvt_tile_selects_the_zoom_scoped_geometry_variant_and_falls_back_to_the_base_column_against_a_real_database: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_geometry_variant";
    seed_two_geometry_columns(&database_url, table).await;

    unsafe {
        env::set_var(URL_ENV_VAR, &database_url);
    }

    let config: AppConfig = serde_yaml::from_str(&format!(
        "storages: [ {{ id: main, driver: postgis, url_env: {URL_ENV_VAR} }} ]\n\
         tenants: [ {{ id: public }} ]\n\
         catalogs: [ {{ id: default, tenant: public }} ]\n\
         collections:\n  - id: {table}\n    catalog: default\n    storage: main\n    \
         geometry: geom\n    pk: id\n    tiles: {{ minzoom: 0, maxzoom: 14 }}\n    \
         geometry_variants:\n      - column: geom_z6\n        minzoom: 0\n        maxzoom: 6\n"
    ))
    .expect("valid AppConfig yaml with a pinned geometry and a declared variant");
    config
        .validate()
        .expect("a variant within the tiles range and a non-empty column name is valid shape");

    let mut registry = Registry::new();
    registry.register(Arc::new(PostgisDriverFactory::new(60)));
    let router = Router::build(&config, &registry).expect("router builds");
    router
        .validate_catalog()
        .await
        .expect("geom_z6 exists and shares geom's srid/geometry type, so boot must succeed");

    let (decl, tiles) = router
        .resolve_tiles("public", "default", table)
        .await
        .expect("resolves the tiles lane");

    let (base_x, base_y) = lonlat_to_tile(10.0, 45.0, 3);
    let (variant_x, variant_y) = lonlat_to_tile(-170.0, -80.0, 3);

    // Zoom 3 falls inside the variant's declared [0, 6] range: only the
    // variant column's own tile is populated, not the base column's.
    let variant_tile_in_range = tiles
        .mvt_tile(
            &decl,
            TileCoord {
                z: 3,
                x: variant_x,
                y: variant_y,
            },
            None,
        )
        .await
        .expect("mvt query succeeds");
    assert!(
        variant_tile_in_range.is_some(),
        "zoom 3 is within geom_z6's range and its tile covers geom_z6's own point"
    );
    let base_tile_at_low_zoom = tiles
        .mvt_tile(
            &decl,
            TileCoord {
                z: 3,
                x: base_x,
                y: base_y,
            },
            None,
        )
        .await
        .expect("mvt query succeeds");
    assert!(
        base_tile_at_low_zoom.is_none(),
        "at zoom 3 the driver must read geom_z6, not geom, so geom's own tile is empty"
    );

    let (base_x_z10, base_y_z10) = lonlat_to_tile(10.0, 45.0, 10);
    let (variant_x_z10, variant_y_z10) = lonlat_to_tile(-170.0, -80.0, 10);

    // Zoom 10 falls outside the variant's declared [0, 6] range: the base
    // column applies, so the relationship flips.
    let base_tile_at_high_zoom = tiles
        .mvt_tile(
            &decl,
            TileCoord {
                z: 10,
                x: base_x_z10,
                y: base_y_z10,
            },
            None,
        )
        .await
        .expect("mvt query succeeds");
    assert!(
        base_tile_at_high_zoom.is_some(),
        "zoom 10 is outside geom_z6's range, so the base geom column applies and its tile is populated"
    );
    let variant_tile_at_high_zoom = tiles
        .mvt_tile(
            &decl,
            TileCoord {
                z: 10,
                x: variant_x_z10,
                y: variant_y_z10,
            },
            None,
        )
        .await
        .expect("mvt query succeeds");
    assert!(
        variant_tile_at_high_zoom.is_none(),
        "at zoom 10 the driver must not read geom_z6, so its own tile is empty"
    );
}

/// **The decisive `#262` test.** An ordinary tile request — no parameter of
/// any kind, because a tile has none to carry — against a collection whose
/// PostGIS storage is projected (EPSG:3857).
///
/// Before this slice `sql::build_mvt_candidate_fragment`'s candidate
/// predicate was `t.<geom> && ST_Transform(tile_env.geom, 4326)`. Against a
/// 3857 column that compares the column's METRES against a box in DEGREES,
/// and `&&` — unlike `ST_Intersects`, which raised the mixed-SRID `500`
/// `#247` was named for — does not object. Verified against this very
/// database:
///
/// ```sql
/// SELECT ST_SetSRID(ST_MakePoint(1113195, 1118890), 3857)
///        && ST_Transform(ST_TileEnvelope(8, 135, 120), 4326);  -- f
/// SELECT ST_SetSRID(ST_MakePoint(11.13, 11.13), 3857)
///        && ST_Transform(ST_TileEnvelope(8, 135, 120), 4326);  -- t
/// ```
///
/// So the tile came back well-formed, `200`, and cached — and wrong. OGC
/// API — Tiles Part 1 Requirement 5 (`/req/core/tc-success`) clause B says
/// the response "SHALL … represent elements inside or intersecting with the
/// spatial extent of the geographical area of the tile identified by the
/// tile matrix, tile row, and tile column of the tileset's tile matrix set",
/// and Requirement 6 clause B permits an empty response only when "the tile
/// has no content due to lack of data in the area". Neither held.
///
/// ## What "wrong" was, exactly — measured, not assumed
///
/// The predicate selected the wrong CANDIDATES, and then `ST_AsMVTGeom`
/// clipped whatever survived against `tile_env.geom` in the GRID's CRS,
/// where those rows are nowhere near the tile. The two errors compose into
/// one symptom on the mercator grid: every tile of a projected collection
/// came back empty, which is exactly what `#142`'s own pin in
/// `invalidation_live.rs` recorded from the outside. Measured against this
/// database on the very tiles below — old candidates for `8/135/120` are
/// `{near_origin}`, old RENDERED content is `{}` — so this test asserts
/// content, and its "must not contain the other row" assertions guard the
/// candidate half that a content-only check would let drift back.
///
/// [`seed_3857_disjoint_pair`] — `#255`'s own fixture, reused unchanged
/// because it was built for exactly this property — is what makes this
/// decisive rather than merely different. Its two points are placed so the
/// two readings of a tile envelope never agree, and the four tiles below
/// were each chosen for a different shape of disagreement, then confirmed
/// against the live database before being written down:
///
/// | grid / z/x/y            | before this slice | after         |
/// |-------------------------|-------------------|---------------|
/// | mercator `8/135/120`    | *empty*           | `in_box`      |
/// | mercator `8/128/127`    | *empty*           | `near_origin` |
/// | mercator `10/543/480`   | *empty*           | *empty*       |
/// | WorldCRS84Quad `8/270/113` | *empty*        | `in_box`      |
///
/// - mercator `8/135/120` covers `in_box`'s real ground (10E 10N). It is the
///   "assert the feature is present" half, and it is what fails first on a
///   revert. Its companion assertion is that `near_origin` — the row whose
///   METRE coordinates (~11.13, ~11.13) the old predicate did select here —
///   is absent, so a future change that reintroduces the wrong candidate set
///   and then happens to keep it through the clip cannot pass either.
/// - mercator `10/543/480` covers ground (about 10.9-11.3E, 10.9-11.2N) that
///   neither row occupies, but whose degree box still contains
///   `near_origin`'s metres — the tile where the wrong reading would have
///   placed a feature, which must come back empty. It is the one assertion
///   that also held before this slice, and it is here so that a "fix" that
///   simply widened the candidate set (or dropped the predicate) would fail
///   it.
/// - mercator `8/128/127` covers `near_origin`'s real ground beside the
///   origin. It proves the other row is reachable at all, so the assertions
///   above are not vacuous.
/// - WorldCRS84Quad `8/270/113` (`[9.84375, 9.84375, 10.546875, 10.546875]`
///   degrees, asserted from `world_crs84_tile_bounds_deg` rather than
///   copied) is the second grid's own half, and the only one where the fix's
///   OTHER side is load-bearing: on the mercator grid the geometry
///   expression was already `ST_Transform(t.<geom>, 3857)`, right for any
///   storage, but `#190`'s CRS84 arm handed `ST_AsMVTGeom` a bare
///   `t.<geom>` beside a 4326 `bounds`. Its exact SQL is pinned in `sql.rs`'s
///   `world_crs84_mvt_plan_transforms_both_sides_on_a_projected_storage`.
///
/// `tile_properties` carries `name` so the identity assertions can be made
/// on the served bytes themselves (`contains_bytes`, the same substring
/// check `mvt_tile_projects_the_allowlisted_properties_against_a_real_
/// database` uses) rather than on a pk this fixture would have to assume
/// the ordering of. `"in_box"` and `"near_origin"` are neither a substring
/// of the other.
///
/// The mirror-image rule — a CRS84 storage is untouched — is pinned in
/// `sql.rs`'s own
/// `mvt_plan_on_a_crs84_storage_is_byte_for_byte_unchanged_on_both_grids`,
/// at the level where "unchanged" can be checked character by character.
#[tokio::test]
async fn a_tile_over_a_projected_storage_renders_the_features_that_are_actually_there() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping a_tile_over_a_projected_storage_renders_the_features_that_are_actually_there: TELLURION_TEST_DATABASE_URL not set");
        return;
    };

    let table = "tellurion_postgis_live_test_projected_tile_3857";
    seed_3857_disjoint_pair(&database_url, table).await;

    // Safety: same single-threaded-with-respect-to-env-var-access argument
    // as `items_and_mvt_round_trip_against_a_real_database` above.
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
    let tiles = driver.tile_source().expect("driver exposes TileSource");

    let mut collection: CollectionDecl = serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\n"
    ))
    .expect("valid CollectionDecl yaml");
    collection.srid = Some(3857);
    collection.tile_properties = vec!["name".to_string()];

    // The tile over `in_box`'s real ground, derived from its own lon/lat
    // rather than written down, so the fixture and the tile can never
    // disagree about where the point is.
    let (in_box_x, in_box_y) = lonlat_to_tile(10.0, 10.0, 8);
    assert_eq!(
        (in_box_x, in_box_y),
        (135, 120),
        "the table above describes this exact tile"
    );
    let over_in_box = tiles
        .mvt_tile(
            &collection,
            TileCoord {
                z: 8,
                x: in_box_x,
                y: in_box_y,
            },
            None,
        )
        .await
        .expect("rendering a tile over a projected collection succeeds")
        .expect("the tile covering this point's real ground must carry it");
    assert_eq!(
        decoded_feature_count(&over_in_box),
        1,
        "exactly the one row this tile's geographical area contains"
    );
    assert!(
        contains_bytes(&over_in_box, b"in_box"),
        "the feature whose ground this tile covers must be in it — Tiles Part 1 Requirement 5 B"
    );
    assert!(
        !contains_bytes(&over_in_box, b"near_origin"),
        "and never the row whose METRE coordinates merely look like this tile's degrees — \
         serving it is precisely the confidently-wrong tile `#262` was opened for"
    );

    // The companion half: the tile where the wrong reading would have put
    // `near_origin`. Its own ground holds neither row, so the only honest
    // answer is an empty tile (Requirement 6 clause B: empty "due to lack of
    // data in the area").
    let where_the_wrong_reading_pointed = tiles
        .mvt_tile(
            &collection,
            TileCoord {
                z: 10,
                x: 543,
                y: 480,
            },
            None,
        )
        .await
        .expect("rendering succeeds");
    assert!(
        where_the_wrong_reading_pointed
            .as_ref()
            .is_none_or(|bytes| bytes.is_empty()),
        "this tile's geographical area holds neither seeded row, so it must be empty — it is \
         where the old predicate DID select `near_origin`, whose metres fall inside its degree box"
    );

    // ...and the other row is genuinely reachable, which is what keeps both
    // assertions above from being satisfied by a driver that simply renders
    // nothing anywhere.
    let (near_origin_x, near_origin_y) = lonlat_to_tile(0.0001, 0.0001, 8);
    assert_eq!(
        (near_origin_x, near_origin_y),
        (128, 127),
        "the table above describes this exact tile"
    );
    let over_near_origin = tiles
        .mvt_tile(
            &collection,
            TileCoord {
                z: 8,
                x: near_origin_x,
                y: near_origin_y,
            },
            None,
        )
        .await
        .expect("rendering succeeds")
        .expect("the tile beside the origin must carry the point that is beside the origin");
    assert!(
        contains_bytes(&over_near_origin, b"near_origin"),
        "before this slice this tile was empty: the row's metres are nowhere near its degrees"
    );
    assert!(
        !contains_bytes(&over_near_origin, b"in_box"),
        "and the far-away row is not in it either"
    );

    // The second grid's own half (`#190`'s WorldCRS84Quad), and the only
    // place where transforming the GEOMETRY — not just the envelope — is
    // load-bearing: that arm handed `ST_AsMVTGeom` a bare `t.<geom>` beside
    // a 4326 `bounds`, so a 3857 column produced neither the right
    // candidates nor a clippable geometry.
    let crs84_tile = TileCoord {
        z: 8,
        x: 270,
        y: 113,
    };
    let [minlon, minlat, maxlon, maxlat] = tellurion_core::world_crs84_tile_bounds_deg(crs84_tile);
    // Derived from the grid registry, never copied: this tile must contain
    // `in_box`'s ground and exclude `near_origin`'s, or it proves nothing.
    assert!(
        minlon < 10.0 && 10.0 < maxlon && minlat < 10.0 && 10.0 < maxlat,
        "the CRS84 tile must cover in_box's ground: [{minlon}, {minlat}, {maxlon}, {maxlat}]"
    );
    assert!(
        minlon > 0.0001 || minlat > 0.0001,
        "and must not cover near_origin's: [{minlon}, {minlat}, {maxlon}, {maxlat}]"
    );
    let crs84_over_in_box = tiles
        .mvt_tile_in(
            &collection,
            tellurion_core::TileMatrixSet::WorldCrs84Quad,
            crs84_tile,
            None,
        )
        .await
        .expect("rendering a WorldCRS84Quad tile over a projected collection succeeds")
        .expect("the CRS84 tile covering this point's real ground must carry it");
    assert!(
        contains_bytes(&crs84_over_in_box, b"in_box"),
        "the CRS84 grid must select and clip a projected geometry as correctly as the mercator \
         grid does — before this slice both halves of that arm assumed 4326 storage"
    );
    assert!(
        !contains_bytes(&crs84_over_in_box, b"near_origin"),
        "and never the row this tile's degrees do not cover"
    );
}
