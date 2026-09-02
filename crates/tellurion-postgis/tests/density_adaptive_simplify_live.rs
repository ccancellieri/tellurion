//! Live proof for `#102` (density-adaptive simplification tolerance) against
//! a real PostGIS instance.
//!
//! Honesty note: `tellurion-render`'s golden-image harness renders locally
//! built scenes — the geometry never passes through PostGIS, so a golden
//! image can never observe a `ST_SimplifyPreserveTopology` tolerance change.
//! This file is the layer that actually can: it exercises the same
//! `TileSource::mvt_tile` entry point production code uses, against a real
//! database, and decodes the served MVT bytes back to prove the tolerance
//! (and the vertex-budget retry) genuinely changed what got served — not
//! just that a formula returns a different number in isolation (already
//! covered by `tellurion-core`'s unit tests).
//!
//! Every collection here is built directly (never through `Router`), the
//! same way `live.rs`'s own fixtures are — `CollectionDecl::geometry_profile`
//! is set by hand where a test needs one, standing in for what `Router::
//! effective_tile_decl` would otherwise attach; this file only needs to
//! prove `PostgisBackend::mvt_tile_inner`'s own behavior, not `Router`'s
//! profile-fetch wiring (already covered by `tellurion-core::router`'s own
//! tests).
//!
//! Skipped gracefully unless `TELLURION_TEST_DATABASE_URL` is set, so
//! `cargo test` never needs a database by default.

use std::env;
use std::time::SystemTime;

use tellurion_core::{
    CollectionDecl, DriverFactory, FeatureSizeStats, GeometryProfile, StorageDecl, TileCoord,
    VertexStats,
};
use tellurion_postgis::test_harness;
use tellurion_postgis::PostgisDriverFactory;

const URL_ENV_VAR: &str = "TELLURION_POSTGIS_DENSITY_ADAPTIVE_LIVE_TEST_URL";

/// A `GeometryProfile` fixture whose mean vertex count per feature is
/// several orders of magnitude above `descriptor::heuristics`'s own
/// (private) density reference — this deliberately clamps the profile-
/// driven tolerance scale at its documented ceiling
/// (`MAX_DENSITY_TOLERANCE_SCALE`, 4x the zoom-only baseline) regardless of
/// that reference's exact value, so the fixtures below only need to reason
/// about one scale factor. Every other field is a placeholder the tolerance
/// formula never reads.
fn dense_geometry_profile() -> GeometryProfile {
    GeometryProfile {
        sample_size: 100,
        computed_at: SystemTime::now(),
        vertices: VertexStats {
            mean: 100_000.0,
            median: 100_000.0,
            p95: 100_000.0,
            max: 100_000,
            total_estimated: None,
        },
        vertex_density_per_area: None,
        multi_part_fraction: 0.0,
        mean_ring_count: None,
        feature_size: FeatureSizeStats::default(),
    }
}

fn collection(table: &str) -> CollectionDecl {
    serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\n"
    ))
    .expect("valid CollectionDecl yaml")
}

fn collection_with_profile(table: &str) -> CollectionDecl {
    let mut decl = collection(table);
    decl.geometry_profile = Some(dense_geometry_profile());
    decl
}

fn collection_with_vertex_budget(table: &str, budget: u64) -> CollectionDecl {
    serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\nsettings: {{ tile_vertex_budget: {budget} }}\n"
    ))
    .expect("valid CollectionDecl yaml")
}

fn collection_with_vertex_budget_and_profile(table: &str, budget: u64) -> CollectionDecl {
    let mut decl = collection_with_vertex_budget(table, budget);
    decl.geometry_profile = Some(dense_geometry_profile());
    decl
}

/// Counts the features across every layer of a served MVT tile — the same
/// decode `live.rs`'s own vertex-budget tests use.
fn decoded_feature_count(tile: &[u8]) -> usize {
    use geozero::mvt::Message;
    let decoded = geozero::mvt::Tile::decode(tile).expect("served bytes are a valid MVT tile");
    decoded
        .layers
        .iter()
        .map(|layer| layer.features.len())
        .sum()
}

/// z15's zoom-only tolerance is ~4.78m; scaled by the [`dense_geometry_
/// profile`]'s clamped 4x ceiling it's ~19.11m (`descriptor::heuristics::
/// simplify_tolerance_meters_for_profile`'s own doc has the exact formula).
/// One `LineString` zigzags with a ~10m perpendicular deviation per vertex
/// (comfortably above the zoom-only tolerance, comfortably below the
/// profile-scaled one) — small enough to survive `ST_SimplifyPreserveTopology`
/// untouched at the zoom-only baseline, large enough to collapse to its two
/// endpoints once a profile raises the tolerance past it. All coordinates
/// sit within a few dozen meters of the equator/prime-meridian origin, well
/// inside a single z15 tile (~1.2km across), so `ST_AsMVTGeom` clipping
/// never touches this fixture.
async fn seed_coarser_when_profiled(database_url: &str, table: &str) {
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
                 (
                     ST_SetSRID(
                         ST_MakeLine(ARRAY(
                             SELECT ST_MakePoint((i % 2) * 0.00009, i * 0.00001)
                             FROM generate_series(0, 59) AS i
                         )),
                         4326
                     )
                 );
             ANALYZE {table};"
        ),
    )
    .await
    .expect("seeds the coarser-when-profiled test table");
}

/// Same shape as [`seed_coarser_when_profiled`]'s zigzag, three ordinary
/// points, and a tight `tile_vertex_budget`, tuned so the pre-flight probe
/// blows the budget at the profile-scaled tolerance (~19.11m at z15) — the
/// ~60-vertex zigzag's ~27m deviation survives that tolerance untouched —
/// but fits comfortably after one `#102` retry at twice that tolerance
/// (~38.22m), where the same zigzag collapses to its two endpoints. The
/// dense geometry is seeded last (highest pk), matching `live.rs`'s own
/// `seed_dense_and_simple_geometries` convention so a tight budget with no
/// profile at all (today's behavior) truncates exactly it.
async fn seed_adapts_after_one_retry(database_url: &str, table: &str) {
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
                             SELECT ST_MakePoint((i % 2) * 0.000243, i * 0.00001)
                             FROM generate_series(0, 59) AS i
                         )),
                         4326
                     )
                 );
             ANALYZE {table};"
        ),
    )
    .await
    .expect("seeds the retry-then-adapt test table");
}

fn z15_origin_coord() -> TileCoord {
    // Every seeded coordinate above sits east of lon=0 and north of lat=0;
    // lon=0/lat=0 is the tile boundary at x=2^14/y=2^14 for any zoom (Web
    // Mercator's antimeridian-anchored X grid, equator-anchored Y grid), and
    // XYZ tiling numbers rows top-to-bottom, so the tile that actually
    // contains them is x=2^14, y=2^14-1 — the same coordinate `live.rs`'s
    // own dense/simple fixture uses at z15.
    TileCoord {
        z: 15,
        x: 16384,
        y: 16383,
    }
}

/// `#101`/`#102`, bullet 1 in isolation: a profile raises the simplification
/// tolerance at a given zoom without any vertex-budget pressure at all (the
/// default budget is far above this fixture's ~60 vertices either way) — the
/// served feature is the same one in both cases (no truncation), but the
/// profiled tile's geometry is measurably coarser.
#[tokio::test]
async fn mvt_tile_serves_coarser_geometry_for_a_dense_profile_at_the_same_zoom() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping mvt_tile_serves_coarser_geometry_for_a_dense_profile_at_the_same_zoom: \
             TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_density_coarser";
    seed_coarser_when_profiled(&database_url, table).await;

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
    let coord = z15_origin_coord();

    let unprofiled = tiles
        .mvt_tile(&collection(table), coord, None)
        .await
        .expect("mvt query succeeds")
        .expect("the zigzag sits inside this tile");
    let profiled = tiles
        .mvt_tile(&collection_with_profile(table), coord, None)
        .await
        .expect("mvt query succeeds")
        .expect("the zigzag sits inside this tile");

    assert_eq!(
        decoded_feature_count(&unprofiled),
        1,
        "the zoom-only tolerance (~4.78m) is well under the zigzag's ~10m deviation, so \
         nothing is dropped by ST_SimplifyPreserveTopology"
    );
    assert_eq!(
        decoded_feature_count(&profiled),
        1,
        "the profile changes tolerance, not which features are candidates — this must still \
         be the same one feature, never a truncation effect"
    );
    assert!(
        profiled.len() < unprofiled.len(),
        "a dense profile's raised tolerance (~19.11m, past the zigzag's ~10m deviation) must \
         collapse the linestring to its two endpoints, producing a strictly smaller encoded \
         tile than the untouched zoom-only geometry (unprofiled: {} bytes, profiled: {} bytes)",
        unprofiled.len(),
        profiled.len()
    );
}

/// `#102`, bullet 2: a tile whose pre-flight probe blows the vertex budget
/// at the profile-scaled tolerance retries once at a raised tolerance and
/// serves every feature simplified, rather than truncating — proven by
/// running the exact same fixture and budget with and without a profile
/// attached. Without one, there is no density signal to retry with, so this
/// is exactly `live.rs`'s own `mvt_tile_drops_the_marginal_geometry_when_
/// it_exceeds_the_vertex_budget_against_a_real_database` shape: the dense
/// geometry is dropped, only the three simple points survive — the
/// unprofiled-collection-unchanged half of `#102`'s scope, pinned here
/// alongside the profiled behavior it contrasts with.
#[tokio::test]
async fn mvt_tile_adapts_after_one_retry_instead_of_truncating_when_a_profile_is_attached() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping mvt_tile_adapts_after_one_retry_instead_of_truncating_when_a_profile_is_attached: \
             TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };

    let table = "tellurion_postgis_live_test_density_retry";
    seed_adapts_after_one_retry(&database_url, table).await;

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
    let coord = z15_origin_coord();

    let without_profile = collection_with_vertex_budget(table, 10);
    let truncated = tiles
        .mvt_tile(&without_profile, coord, None)
        .await
        .expect("mvt query succeeds")
        .expect("the three simple points still fit under the budget");
    assert_eq!(
        decoded_feature_count(&truncated),
        3,
        "no profile means no retry probe ever runs — this must match today's immediate-\
         truncation behavior exactly: the dense zigzag is dropped, the three simple points \
         (cumulative total 3) survive"
    );

    let with_profile = collection_with_vertex_budget_and_profile(table, 10);
    let adapted = tiles
        .mvt_tile(&with_profile, coord, None)
        .await
        .expect("mvt query succeeds")
        .expect("a retry at twice the profile-scaled tolerance fits the budget");
    assert_eq!(
        decoded_feature_count(&adapted),
        4,
        "a profile lets the pre-flight probe retry at a raised tolerance; the zigzag's ~27m \
         deviation collapses to its two endpoints past the retry tolerance (~38.22m), fitting \
         the budget of 10 — all four rows must survive, none truncated"
    );
}
