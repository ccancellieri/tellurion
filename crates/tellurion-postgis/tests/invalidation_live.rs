//! Live tests for write-reactive tile-cache invalidation (`#113`) through
//! the actual `PostgisDriverFactory` entry point, against a real PostGIS
//! instance. Skipped gracefully unless `TELLURION_TEST_DATABASE_URL` is set,
//! matching every other live test in this workspace.
//!
//! Unlike the derived-index lane (`index_live.rs`), this consumer needs no
//! new table at all — `tellurion_core::invalidation::GenerationStore` is a
//! purely in-process structure fed by the same `OutboxSource` the write
//! lane already commits to, so the only DDL these tests need is the data
//! table plus the existing outbox table.

use std::env;

use serde_json::json;
use tellurion_core::{
    drain_once_for_generations, CollectionDecl, DriverFactory, GenerationStore, Mutation,
    MutationKind, Sequence, StorageDecl, StorageDriver,
};
use tellurion_postgis::test_harness;
use tellurion_postgis::PostgisDriverFactory;

const URL_ENV_VAR: &str = "TELLURION_POSTGIS_INVALIDATION_LIVE_TEST_URL";

async fn connect(database_url: &str) -> tokio_postgres::Client {
    test_harness::connect(database_url).await
}

async fn seed_data_table(database_url: &str, table: &str) {
    let client = connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table} CASCADE;
             DROP TABLE IF EXISTS {table}_outbox;
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326),
                 name text
             );"
        ),
    )
    .await
    .expect("seeds the data table");
}

/// Matches `tellurion-ingest::outbox::create_outbox_table_sql` exactly —
/// same convention `index_live.rs`/`write_live.rs` already follow.
async fn seed_outbox_table(database_url: &str, table: &str) {
    let client = connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "CREATE TABLE IF NOT EXISTS {table}_outbox (
                 sequence bigserial PRIMARY KEY,
                 feature_id text NOT NULL,
                 kind text NOT NULL CHECK (kind IN ('upsert', 'delete')),
                 payload jsonb,
                 committed_at timestamptz NOT NULL DEFAULT now(),
                 extent_crs84 jsonb
             );"
        ),
    )
    .await
    .expect("seeds the outbox table");
}

fn collection(table: &str) -> CollectionDecl {
    serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\n"
    ))
    .expect("valid CollectionDecl yaml")
}

async fn build_driver(database_url: &str) -> std::sync::Arc<dyn StorageDriver> {
    // Safety: this test binary sets this one env var exactly once per test
    // process before any connection pool spawns worker tasks, matching
    // `write_live.rs`/`index_live.rs`'s own documented safety argument for
    // the same pattern.
    unsafe {
        env::set_var(URL_ENV_VAR, database_url);
    }
    let factory = PostgisDriverFactory::new(60);
    let decl = StorageDecl {
        id: "main".to_string(),
        driver: "postgis".to_string(),
        url_env: URL_ENV_VAR.to_string(),
        pool_size: None,
    };
    factory.build(&decl).expect("driver builds")
}

async fn upsert_point(
    sink: &dyn tellurion_core::WriteSink,
    collection: &CollectionDecl,
    id: &str,
    lon: f64,
    lat: f64,
) {
    sink.apply(
        collection,
        Mutation {
            feature_id: id.to_string(),
            kind: MutationKind::Upsert(json!({
                "type": "Feature",
                "geometry": {"type": "Point", "coordinates": [lon, lat]},
                "properties": {"name": id}
            })),
        },
    )
    .await
    .expect("upsert succeeds");
}

async fn delete_feature(
    sink: &dyn tellurion_core::WriteSink,
    collection: &CollectionDecl,
    id: &str,
) {
    sink.apply(
        collection,
        Mutation {
            feature_id: id.to_string(),
            kind: MutationKind::Delete,
        },
    )
    .await
    .expect("delete succeeds");
}

/// The pyramid tile coordinate, at `zoom`, covering `(lon, lat)` — small,
/// self-contained Web Mercator math, independent of both
/// `tellurion_core::invalidation` (private) and `tellurion-tiles::mercator`
/// (a different crate) so this test asserts against its own from-scratch
/// computation rather than the exact code under test.
fn lonlat_to_tile(lon: f64, lat: f64, zoom: u8) -> (u32, u32) {
    let earth_radius_m = 6_378_137.0_f64;
    let origin = earth_radius_m * std::f64::consts::PI;
    let x = earth_radius_m * lon.to_radians();
    let y = earth_radius_m
        * (std::f64::consts::FRAC_PI_4 + lat.to_radians() / 2.0)
            .tan()
            .ln();
    let side = 1u32 << zoom;
    let tile_size = 2.0 * origin / f64::from(side);
    let col = (((x + origin) / tile_size).floor() as u32).min(side - 1);
    let row = (((origin - y) / tile_size).floor() as u32).min(side - 1);
    (col, row)
}

/// (a)/(b) end to end, against real committed-and-reread outbox rows: an
/// upsert bumps the bucket its real, round-tripped-through-Postgres JSONB
/// geometry falls in; a later delete on that same feature bumps it again,
/// using this consumer's own remembered bbox (the delete's own outbox row
/// carries no geometry at all — proving that really is true against a real
/// driver, not just the in-memory fakes `invalidation.rs`'s own unit tests
/// use).
#[tokio::test]
async fn a_real_upsert_then_delete_bump_the_same_bucket_through_the_real_driver() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping a_real_upsert_then_delete_bump_the_same_bucket_through_the_real_driver: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_invalidation_live_test_bump";
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let write = driver.write_sink().expect("driver exposes WriteSink");
    let outbox = driver.outbox_source().expect("driver exposes OutboxSource");
    let collection = collection(table);
    let store = GenerationStore::new(4, ["demo".to_string()]);

    let (lon, lat) = (10.0, 45.0);
    let (col, row) = lonlat_to_tile(lon, lat, 4);
    let (far_col, far_row) = lonlat_to_tile(-170.0, -80.0, 4);

    upsert_point(write.as_ref(), &collection, "1", lon, lat).await;
    let applied = drain_once_for_generations(outbox.as_ref(), &store, &collection, 100)
        .await
        .expect("first drain succeeds");
    assert_eq!(applied, 1);

    let after_upsert = store.generation_for_tile("demo", 4, col, row);
    assert!(after_upsert > 0, "the written bucket must be bumped");
    assert_eq!(
        store.generation_for_tile("demo", 4, far_col, far_row),
        0,
        "a distant bucket must stay untouched"
    );

    delete_feature(write.as_ref(), &collection, "1").await;
    let applied = drain_once_for_generations(outbox.as_ref(), &store, &collection, 100)
        .await
        .expect("second drain succeeds");
    assert_eq!(applied, 1);

    let after_delete = store.generation_for_tile("demo", 4, col, row);
    assert!(
        after_delete > after_upsert,
        "the delete must bump the same bucket again, using the remembered old bbox"
    );
}

/// Restart-safety through the real driver: killing the drain loop mid-batch
/// and resuming from `GenerationStore::cursor` alone (no separate cursor
/// table) must land on the exact same end state as one uninterrupted drain.
#[tokio::test]
async fn drain_resumes_from_the_stores_own_cursor_against_a_real_outbox() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping drain_resumes_from_the_stores_own_cursor_against_a_real_outbox: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_invalidation_live_test_resume";
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let write = driver.write_sink().expect("driver exposes WriteSink");
    let outbox = driver.outbox_source().expect("driver exposes OutboxSource");
    let collection = collection(table);
    let store = GenerationStore::new(4, ["demo".to_string()]);

    upsert_point(write.as_ref(), &collection, "1", 1.0, 1.0).await;
    upsert_point(write.as_ref(), &collection, "2", 2.0, 2.0).await;
    upsert_point(write.as_ref(), &collection, "3", 3.0, 3.0).await;

    // Bounded batch of 2 — simulates a crash after partial progress.
    let applied = drain_once_for_generations(outbox.as_ref(), &store, &collection, 2)
        .await
        .expect("first drain succeeds");
    assert_eq!(applied, 2);
    assert_eq!(store.cursor("demo"), Sequence(2));

    let applied = drain_once_for_generations(outbox.as_ref(), &store, &collection, 100)
        .await
        .expect("second drain succeeds");
    assert_eq!(applied, 1);
    assert_eq!(store.cursor("demo"), Sequence(3));

    let applied = drain_once_for_generations(outbox.as_ref(), &store, &collection, 100)
        .await
        .expect("third drain succeeds");
    assert_eq!(applied, 0, "a fully caught-up pass reads nothing new");
}

// ---------------------------------------------------------------------------
// `#142` / `#141`: the decisive end-to-end pair, through the real driver, a
// real projected collection, and the real MVT renderer.
// ---------------------------------------------------------------------------

/// A projected (EPSG:3857) point collection plus its outbox — the `#142`
/// fixture. Same table shape as `seed_data_table`, one SRID different, which
/// is the whole difference the defect turned on.
async fn seed_projected_table(database_url: &str, table: &str) {
    let client = connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table} CASCADE;
             DROP TABLE IF EXISTS {table}_outbox;
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 3857),
                 name text
             );"
        ),
    )
    .await
    .expect("seeds the projected data table");
}

/// `CollectionDecl::srid` is `#[serde(skip)]` — never operator-configured;
/// `Router::resolve_features`/`resolve_tiles` fill it in from the driver's own
/// catalog. These driver-level tests have no router, so they set it directly,
/// exactly as `tellurion-geopackage`'s own contract tests do.
fn projected_collection(table: &str) -> CollectionDecl {
    let mut decl = collection(table);
    decl.srid = Some(3857);
    decl
}

/// Rome, in CRS84 and in the EPSG:3857 metres a `Content-Crs`-declared write
/// against the projected collection puts on the wire (and therefore verbatim
/// into the outbox payload).
const ROME_LON: f64 = 12.49;
const ROME_LAT: f64 = 41.90;
const ROME_X_M: f64 = 1_390_330.0;
const ROME_Y_M: f64 = 5_146_501.0;

/// Whether an MVT tile body carries any feature at all — a layer with no
/// features encodes to a handful of bytes, one with a point to noticeably
/// more, and the layer name this driver writes is the collection id. Kept
/// deliberately crude: what these tests assert is "the rendering changed",
/// not the protobuf's internal shape.
fn renders_a_feature(tile: &Option<bytes::Bytes>) -> bool {
    tile.as_ref().is_some_and(|bytes| !bytes.is_empty())
}

/// `#142`, decisively: a write submitted under a `Content-Crs` naming the
/// collection's own projected storage CRS must invalidate the bucket whose
/// tiles genuinely render it.
///
/// Before this issue the consumer read the obligation payload's coordinates
/// as CRS84. Those coordinates are EPSG:3857 metres — `1390330, 5146501` for
/// Rome — so read as lon/lat they clamp to the antimeridian and the Web
/// Mercator latitude limit and bump the bucket in the far south-east corner
/// of the grid. Rome's own bucket, the one every tile rendering this feature
/// hangs off, kept its old generation: the same cache key, the same cached
/// bytes, a `200` forever. Both halves are asserted here — the tile that
/// renders the feature gets a new generation, and the corner bucket does not.
#[tokio::test]
async fn a_projected_crs_write_invalidates_the_bucket_whose_tiles_render_it() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping a_projected_crs_write_invalidates_the_bucket_whose_tiles_render_it: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_invalidation_live_test_projected";
    seed_projected_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let write = driver.write_sink().expect("driver exposes WriteSink");
    let outbox = driver.outbox_source().expect("driver exposes OutboxSource");
    let tiles = driver.tile_source().expect("driver exposes TileSource");
    let collection = projected_collection(table);
    let store = GenerationStore::new(4, ["demo".to_string()]);

    let (col, row) = lonlat_to_tile(ROME_LON, ROME_LAT, 4);
    // Where reading the payload's metres as degrees lands: clamped to the
    // far corner of the grid.
    let (wrong_col, wrong_row) = lonlat_to_tile(180.0, 85.0, 4);
    assert_ne!(
        (col, row),
        (wrong_col, wrong_row),
        "the fixture only means anything if the two buckets differ"
    );

    // A `Content-Crs`-declared write: the body is in the storage CRS, so its
    // coordinates are metres — exactly what the outbox stores verbatim.
    write
        .apply_with_crs(
            &collection,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(json!({
                    "type": "Feature",
                    "geometry": {"type": "Point", "coordinates": [ROME_X_M, ROME_Y_M]},
                    "properties": {"name": "rome"}
                })),
            },
            tellurion_core::RequestedCrs::Storage,
        )
        .await
        .expect("a storage-CRS upsert succeeds");

    let applied = drain_once_for_generations(outbox.as_ref(), &store, &collection, 100)
        .await
        .expect("drain succeeds");
    assert_eq!(applied, 1);

    assert!(
        store.generation_for_tile("demo", 4, col, row) > 0,
        "the bucket whose tiles render this feature must be invalidated"
    );
    assert_eq!(
        store.generation_for_tile("demo", 4, wrong_col, wrong_row),
        0,
        "the bucket a CRS-blind read of the payload would have bumped must stay untouched"
    );

    // `#262`, which is the tightening the pin that stood here asked its
    // future fixer for, by name. What this comment used to say — that the
    // tile over Rome does NOT render this feature, because `sql::build_mvt_
    // candidate_fragment`'s envelope predicate was
    // `t.<geom> && ST_Transform(tile_env.geom, 4326)` and against a 3857
    // column that compares metres to degrees and matches nothing — was true
    // when it was written and is no longer. The predicate now transforms the
    // tile envelope into the collection's own storage CRS, so a projected
    // collection served through PostGIS renders the features it actually
    // contains, and this assertion says so instead of pinning the emptiness.
    //
    // That makes `#142`'s two halves finally assertable in one place and
    // through one driver: the write bumped the bucket whose tiles render
    // this feature (above), and those tiles do render it (here). Before
    // `#262` the rendering half could only be shown elsewhere — `scripts/
    // demo-smoke.sh` phase 11, over real HTTP against the GeoPackage
    // driver's own 3857 fast path, which is still a gate and still worth
    // having, and `a_delete_after_a_restart_invalidates_only_the_bucket_the_
    // feature_occupied` below for a CRS84 collection.
    let rome_tile = tiles
        .mvt_tile(
            &collection,
            tellurion_core::TileCoord {
                z: 4,
                x: col,
                y: row,
            },
            None,
        )
        .await
        .expect("rendering the tile over Rome succeeds");
    assert!(
        renders_a_feature(&rome_tile),
        "the bucket this write invalidated must be the bucket whose tiles genuinely render the \
         feature — an invalidated generation over a permanently empty tile would prove nothing"
    );
    // ...and the corner the CRS-blind read pointed at renders nothing,
    // because nothing is there. Both halves, same shape as the generation
    // assertions above.
    let corner_tile = tiles
        .mvt_tile(
            &collection,
            tellurion_core::TileCoord {
                z: 4,
                x: wrong_col,
                y: wrong_row,
            },
            None,
        )
        .await
        .expect("rendering the corner tile succeeds");
    assert!(
        !renders_a_feature(&corner_tile),
        "the far corner of the grid holds no feature, so its tile must be empty"
    );
}

/// `#141`, decisively, and specifically its restart case: a consumer with no
/// memory whatsoever of a feature must still invalidate exactly the bucket
/// the feature used to occupy when it is deleted — because the DELETE
/// statement itself recorded that extent, in the same transaction, on its own
/// `RETURNING`.
///
/// The restart is real, not simulated in Rust: the upsert is drained by one
/// store, then pruned out of the outbox entirely, and a FRESH store drains
/// what is left. That store has never seen this feature. Before `#141` its
/// only honest option was the whole-grid `floor` bump, so a distant bucket
/// moved too — which is what this test pins shut.
#[tokio::test]
async fn a_delete_after_a_restart_invalidates_only_the_bucket_the_feature_occupied() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping a_delete_after_a_restart_invalidates_only_the_bucket_the_feature_occupied: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_invalidation_live_test_restart_delete";
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let write = driver.write_sink().expect("driver exposes WriteSink");
    let outbox = driver.outbox_source().expect("driver exposes OutboxSource");
    let tiles = driver.tile_source().expect("driver exposes TileSource");
    let collection = collection(table);

    let (col, row) = lonlat_to_tile(ROME_LON, ROME_LAT, 4);
    let (far_col, far_row) = lonlat_to_tile(-170.0, -80.0, 4);

    upsert_point(write.as_ref(), &collection, "1", ROME_LON, ROME_LAT).await;

    // The tile over Rome renders the feature right now.
    let before = tiles
        .mvt_tile(
            &collection,
            tellurion_core::TileCoord {
                z: 4,
                x: col,
                y: row,
            },
            None,
        )
        .await
        .expect("rendering succeeds");
    assert!(
        renders_a_feature(&before),
        "the fixture must actually render before it is deleted"
    );

    // Drain the upsert into a store that is then thrown away, and prune it
    // out of the outbox: whatever drains next starts from nothing, exactly
    // like a server that has just restarted.
    let doomed = GenerationStore::new(4, ["demo".to_string()]);
    drain_once_for_generations(outbox.as_ref(), &doomed, &collection, 100)
        .await
        .expect("the pre-restart drain succeeds");
    let high_water = outbox
        .primary_high_water(&collection)
        .await
        .expect("high water reads");
    outbox
        .prune_before(&collection, high_water, 100)
        .await
        .expect("pruning the drained prefix succeeds");
    drop(doomed);

    delete_feature(write.as_ref(), &collection, "1").await;

    let restarted = GenerationStore::new(4, ["demo".to_string()]);
    let applied = drain_once_for_generations(outbox.as_ref(), &restarted, &collection, 100)
        .await
        .expect("the post-restart drain succeeds");
    assert_eq!(applied, 1, "only the delete is left in the outbox");

    assert!(
        restarted.generation_for_tile("demo", 4, col, row) > 0,
        "the bucket the deleted feature occupied must be invalidated, \
         or its tile keeps rendering a feature that no longer exists"
    );
    assert_eq!(
        restarted.generation_for_tile("demo", 4, far_col, far_row),
        0,
        "and a distant bucket must NOT be — a whole-grid fallback here would \
         mean the delete's own prior extent was never recorded"
    );

    // The new state really is different: the tile no longer renders it.
    let after = tiles
        .mvt_tile(
            &collection,
            tellurion_core::TileCoord {
                z: 4,
                x: col,
                y: row,
            },
            None,
        )
        .await
        .expect("rendering succeeds");
    assert_ne!(
        before, after,
        "the tile that previously rendered the feature must now render the new state"
    );
}

/// The `#141` case the issue calls "updates to features it has no bbox
/// memory of": a feature that MOVES must invalidate the bucket it left as
/// well as the one it arrived in. Same real restart as above.
#[tokio::test]
async fn a_move_after_a_restart_invalidates_both_the_old_and_the_new_bucket() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping a_move_after_a_restart_invalidates_both_the_old_and_the_new_bucket: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_invalidation_live_test_restart_move";
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let write = driver.write_sink().expect("driver exposes WriteSink");
    let outbox = driver.outbox_source().expect("driver exposes OutboxSource");
    let collection = collection(table);

    let (from_col, from_row) = lonlat_to_tile(ROME_LON, ROME_LAT, 4);
    let (to_col, to_row) = lonlat_to_tile(-74.0, 40.7, 4);
    let (far_col, far_row) = lonlat_to_tile(-170.0, -80.0, 4);
    assert_ne!((from_col, from_row), (to_col, to_row));

    upsert_point(write.as_ref(), &collection, "1", ROME_LON, ROME_LAT).await;
    let high_water = outbox
        .primary_high_water(&collection)
        .await
        .expect("high water reads");
    outbox
        .prune_before(&collection, high_water, 100)
        .await
        .expect("pruning the drained prefix succeeds");

    // The move, seen by a consumer that has never heard of this feature.
    upsert_point(write.as_ref(), &collection, "1", -74.0, 40.7).await;

    let restarted = GenerationStore::new(4, ["demo".to_string()]);
    drain_once_for_generations(outbox.as_ref(), &restarted, &collection, 100)
        .await
        .expect("the post-restart drain succeeds");

    assert!(
        restarted.generation_for_tile("demo", 4, from_col, from_row) > 0,
        "the bucket the feature LEFT must be invalidated, or its tile keeps \
         rendering the feature at a position it has moved away from"
    );
    assert!(
        restarted.generation_for_tile("demo", 4, to_col, to_row) > 0,
        "the bucket the feature ARRIVED in must be invalidated"
    );
    assert_eq!(
        restarted.generation_for_tile("demo", 4, far_col, far_row),
        0,
        "and a distant bucket must not be — this is precise invalidation, not a fallback"
    );
}

/// Campaign rule, executed rather than asserted: a CRS84 deployment's bucket
/// SET must be exactly what it was. The reference here is deliberately NOT
/// this code — it is the geometry the caller submitted, mapped by this test
/// file's own from-scratch Web Mercator math, which is precisely what the
/// pre-`#142` consumer computed off the payload.
#[tokio::test]
async fn a_crs84_collection_invalidates_exactly_the_buckets_it_always_did() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping a_crs84_collection_invalidates_exactly_the_buckets_it_always_did: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_invalidation_live_test_crs84_pin";
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let write = driver.write_sink().expect("driver exposes WriteSink");
    let outbox = driver.outbox_source().expect("driver exposes OutboxSource");
    let collection = collection(table);
    let store = GenerationStore::new(4, ["demo".to_string()]);

    let points = [(ROME_LON, ROME_LAT), (9.19, 45.46), (-74.0, 40.7)];
    for (index, (lon, lat)) in points.iter().enumerate() {
        upsert_point(
            write.as_ref(),
            &collection,
            &(index + 1).to_string(),
            *lon,
            *lat,
        )
        .await;
    }
    drain_once_for_generations(outbox.as_ref(), &store, &collection, 100)
        .await
        .expect("drain succeeds");

    // Every bucket in the 16x16 grid, checked against the expectation
    // computed independently from the submitted coordinates: exactly the
    // three the points fall in are non-zero, and nothing else moved.
    let expected: std::collections::HashSet<(u32, u32)> = points
        .iter()
        .map(|(lon, lat)| lonlat_to_tile(*lon, *lat, 4))
        .collect();
    for row in 0..16u32 {
        for col in 0..16u32 {
            let generation = store.generation_for_tile("demo", 4, col, row);
            if expected.contains(&(col, row)) {
                assert!(
                    generation > 0,
                    "bucket ({col}, {row}) should be invalidated"
                );
            } else {
                assert_eq!(
                    generation, 0,
                    "bucket ({col}, {row}) must not move for a CRS84 collection"
                );
            }
        }
    }
}

/// Campaign rule 4, executed: ingest owns all DDL, and the server refuses BY
/// NAME when the column it needs is absent — it does not quietly write an
/// obligation nobody can map, and it does not create the column itself.
///
/// The fixture is a pre-`#141` outbox table, provisioned exactly as
/// `tellurion-ingest outbox create-tables` used to emit it.
#[tokio::test]
async fn a_write_against_a_pre_extent_outbox_is_refused_by_name() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping a_write_against_a_pre_extent_outbox_is_refused_by_name: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_invalidation_live_test_legacy_outbox";
    seed_data_table(&database_url, table).await;
    let client = connect(&database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "CREATE TABLE {table}_outbox (
                 sequence bigserial PRIMARY KEY,
                 feature_id text NOT NULL,
                 kind text NOT NULL CHECK (kind IN ('upsert', 'delete')),
                 payload jsonb,
                 committed_at timestamptz NOT NULL DEFAULT now()
             );"
        ),
    )
    .await
    .expect("seeds a pre-#141 outbox table");

    let driver = build_driver(&database_url).await;
    let write = driver.write_sink().expect("driver exposes WriteSink");
    let outbox = driver.outbox_source().expect("driver exposes OutboxSource");
    let collection = collection(table);

    let error = write
        .apply(
            &collection,
            Mutation {
                feature_id: "1".to_string(),
                kind: MutationKind::Upsert(json!({
                    "type": "Feature",
                    "geometry": {"type": "Point", "coordinates": [ROME_LON, ROME_LAT]},
                    "properties": {"name": "rome"}
                })),
            },
        )
        .await
        .expect_err("a write against a pre-#141 outbox must be refused");
    let message = error.to_string();
    assert!(
        message.contains("extent_crs84")
            && message.contains("tellurion-ingest outbox create-tables"),
        "the refusal must name the column and the command that supplies it; got: {message}"
    );

    // Nothing was written: the refusal happens inside the transaction, which
    // is dropped without a commit.
    let rows = connect(&database_url)
        .await
        .query_one(&format!("SELECT count(*)::bigint FROM {table}"), &[])
        .await
        .expect("counts the data table");
    assert_eq!(
        rows.get::<_, i64>(0),
        0,
        "a refused write must leave no data row behind either"
    );

    // The drain side refuses under the same name rather than looping on a
    // raw SQL error nobody can act on.
    let store = GenerationStore::new(4, ["demo".to_string()]);
    let error = drain_once_for_generations(outbox.as_ref(), &store, &collection, 100)
        .await
        .expect_err("draining a pre-#141 outbox must be refused");
    assert!(
        error.to_string().contains("extent_crs84"),
        "the drain refusal must name the column too; got: {error}"
    );
}

/// The other half of the same rule: outbox rows written BEFORE the column
/// existed must not break the applier. Once the column is added they read
/// back `NULL`, which is `ObligationExtent::Unrecorded` — unknown, not empty
/// — so the batch degrades to the conservative whole-collection bump and the
/// drain cursor advances past it exactly as it always did.
#[tokio::test]
async fn a_legacy_outbox_row_drains_conservatively_instead_of_breaking() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping a_legacy_outbox_row_drains_conservatively_instead_of_breaking: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_invalidation_live_test_legacy_row";
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    // A row exactly as the pre-`#141` write path left it: a payload, and the
    // column that did not exist yet defaulting to NULL.
    connect(&database_url)
        .await
        .execute(
            &format!(
                "INSERT INTO {table}_outbox (feature_id, kind, payload) VALUES ($1, $2, $3::text::jsonb)"
            ),
            &[
                &"1",
                &"upsert",
                &json!({
                    "type": "Feature",
                    "geometry": {"type": "Point", "coordinates": [ROME_LON, ROME_LAT]},
                    "properties": {}
                })
                .to_string(),
            ],
        )
        .await
        .expect("inserts a legacy outbox row");

    let driver = build_driver(&database_url).await;
    let outbox = driver.outbox_source().expect("driver exposes OutboxSource");
    let collection = collection(table);
    let store = GenerationStore::new(4, ["demo".to_string()]);

    let applied = drain_once_for_generations(outbox.as_ref(), &store, &collection, 100)
        .await
        .expect("a legacy row must drain, not fail");
    assert_eq!(applied, 1);
    assert_eq!(
        store.cursor("demo"),
        Sequence(1),
        "the cursor advances past it"
    );

    // Conservative, not precise, and not silent: every bucket moves, because
    // the row records no extent and this consumer refuses to guess one from
    // a payload whose CRS it cannot know.
    let (col, row) = lonlat_to_tile(ROME_LON, ROME_LAT, 4);
    let (far_col, far_row) = lonlat_to_tile(-170.0, -80.0, 4);
    assert!(store.generation_for_tile("demo", 4, col, row) > 0);
    assert!(
        store.generation_for_tile("demo", 4, far_col, far_row) > 0,
        "an unrecorded extent must bump the whole grid, never nothing"
    );
}

/// `#151` beside `#142`/`#141`: the opt-in `modified_column` touch trigger
/// stamps `now()` on every write to the DATA table, including writes that
/// never reach this crate's write lane. It fires `BEFORE INSERT OR UPDATE`,
/// which is upstream of everything this slice reads — so this asserts, rather
/// than argues, that the two do not interact.
///
/// Three ways they could have, and all three are checked here:
///
/// 1. `RETURNING` is evaluated against the tuple a `BEFORE` trigger left
///    behind, so a trigger that touched the geometry would silently change the
///    recorded extent. This one assigns only its own column — asserted by the
///    extent still being Rome.
/// 2. A `BEFORE` trigger returning `NULL` suppresses its row, which would make
///    `RETURNING` produce nothing and (on the `#150` path) read as "somebody
///    else got there first". This one always `RETURN NEW` — asserted by the
///    extent being recorded at all, on both an insert and an update.
/// 3. The trigger is installed on the data table, never on `"<table>_outbox"`,
///    so the outbox insert is untouched — asserted by the obligation reading
///    back with the extent it was written with.
///
/// The trigger DDL is written out by hand here rather than shelled out to
/// `tellurion-ingest touch-trigger install` (no dependency edge exists from a
/// driver's tests to the ingest binary crate), kept in sync with
/// `tellurion-ingest::touch_trigger::touch_trigger_sql` the same way the
/// outbox DDL at the top of this file is kept in sync with
/// `tellurion-ingest::outbox`.
#[tokio::test]
async fn a_modified_column_touch_trigger_does_not_disturb_the_recorded_extent() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping a_modified_column_touch_trigger_does_not_disturb_the_recorded_extent: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    // `#272`: shortened from `..._invalidation_live_test_touch_trigger` (54
    // bytes). This fixture derives a trigger-function name from it —
    // `{table}_modified_touch`, 69 bytes — which PostgreSQL was silently
    // TRUNCATING to 63 rather than rejecting. `apply_fixture_ddl` now
    // refuses that by name, and this is one of the three names in the
    // workspace it found; the derived name must fit, not just the table's.
    let table = "tellurion_postgis_inval_live_test_touch";
    let client = connect(&database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table} CASCADE;
             DROP TABLE IF EXISTS {table}_outbox;
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326),
                 name text,
                 modified timestamptz
             );"
        ),
    )
    .await
    .expect("seeds the data table");
    seed_outbox_table(&database_url, table).await;
    // The trigger function is named for the table, so the same fixture lock
    // covers it (`#138`): a concurrent run's `DROP TABLE ... CASCADE` above
    // would otherwise take this function out from under the `CREATE`.
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "CREATE OR REPLACE FUNCTION {table}_modified_touch() RETURNS trigger
             LANGUAGE plpgsql AS $tellurion_touch$
             BEGIN
                 NEW.modified := now();
                 RETURN NEW;
             END;
             $tellurion_touch$;
             CREATE OR REPLACE TRIGGER {table}_modified_touch_trg
                 BEFORE INSERT OR UPDATE ON {table}
                 FOR EACH ROW EXECUTE FUNCTION {table}_modified_touch();"
        ),
    )
    .await
    .expect("installs the #151 touch trigger");

    let driver = build_driver(&database_url).await;
    let write = driver.write_sink().expect("driver exposes WriteSink");
    let outbox = driver.outbox_source().expect("driver exposes OutboxSource");
    let mut collection = collection(table);
    collection.modified_column = Some("modified".to_string());
    let store = GenerationStore::new(4, ["demo".to_string()]);

    let (col, row) = lonlat_to_tile(ROME_LON, ROME_LAT, 4);
    let (far_col, far_row) = lonlat_to_tile(-170.0, -80.0, 4);

    // An INSERT under the trigger.
    upsert_point(write.as_ref(), &collection, "1", ROME_LON, ROME_LAT).await;
    // And an UPDATE under it, which is the arm that also re-reads the prior
    // extent one statement earlier.
    upsert_point(write.as_ref(), &collection, "1", ROME_LON, ROME_LAT).await;

    let applied = drain_once_for_generations(outbox.as_ref(), &store, &collection, 100)
        .await
        .expect("draining a touch-triggered table succeeds");
    assert_eq!(applied, 2);

    assert!(
        store.generation_for_tile("demo", 4, col, row) > 0,
        "the trigger must not cost this write its recorded extent"
    );
    assert_eq!(
        store.generation_for_tile("demo", 4, far_col, far_row),
        0,
        "and must not push it onto the conservative whole-grid path either"
    );

    // The trigger really did fire — otherwise the assertions above would pass
    // for the trivial reason that nothing was installed.
    let stamped: i64 = connect(&database_url)
        .await
        .query_one(
            &format!("SELECT count(*)::bigint FROM {table} WHERE modified IS NOT NULL"),
            &[],
        )
        .await
        .expect("counts stamped rows")
        .get(0);
    assert_eq!(stamped, 1, "the #151 trigger must have stamped the row");

    // And a delete under the trigger still records where the row was.
    delete_feature(write.as_ref(), &collection, "1").await;
    let obligations = outbox
        .read_after(&collection, Sequence(2), 10)
        .await
        .expect("reads the delete obligation back");
    assert_eq!(obligations.len(), 1);
    match obligations[0].extent {
        tellurion_core::ObligationExtent::Crs84 { prior, current } => {
            assert_eq!(current, None);
            let prior = prior.expect("a delete under the trigger still records its prior extent");
            assert!((prior[0] - ROME_LON).abs() < 1.0e-9 && (prior[1] - ROME_LAT).abs() < 1.0e-9);
        }
        other => panic!("expected a recorded CRS84 extent, got {other:?}"),
    }

    connect(&database_url)
        .await
        .batch_execute(&format!(
            "DROP FUNCTION IF EXISTS {table}_modified_touch() CASCADE"
        ))
        .await
        .expect("cleans up the trigger function");
}
