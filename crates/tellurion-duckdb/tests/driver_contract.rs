//! Hermetic, driver-level contract tests: a real temp-file `.duckdb`
//! fixture, provisioned by this file's own DDL through the `duckdb` crate
//! directly, driven entirely through
//! `tellurion_core::{DriverFactory, StorageDriver}` — no `Router`, no HTTP,
//! no server process. Covers catalog introspection (including geometry-
//! column auto-detection and its ambiguous-column refusal), boot-time
//! `validate_collection`, paginated/bbox/CQL2-filtered reads, refusal cases,
//! and item lookup — per the crate's own testing obligations. Runs with
//! **zero** network access — see `driver.rs`'s own "EXTENSION note" for why
//! that is unconditional for this crate, not just this test file.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use duckdb::Connection;
use tellurion_core::{
    CollectionDecl, CompareOp, DriverFactory, Error as CoreError, Filter, ItemsQuery, Literal,
    StorageDecl, StorageDriver,
};
use tellurion_duckdb::DuckdbDriverFactory;

fn temp_duckdb_path(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "tellurion-duckdb-contract-test-{}-{}-{name}.duckdb",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    path
}

fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
}

fn point_wkb(lon: f64, lat: f64) -> Vec<u8> {
    use geozero::GeozeroGeometry;
    let geojson = format!(r#"{{"type":"Point","coordinates":[{lon},{lat}]}}"#);
    let mut buf = Vec::new();
    let mut writer = geozero::wkb::WkbWriter::new(&mut buf, geozero::wkb::WkbDialect::Wkb);
    geozero::geojson::GeoJson(&geojson)
        .process_geom(&mut writer)
        .unwrap();
    buf
}

/// The same five points `tellurion-flatgeobuf`/`tellurion-geoparquet`
/// fixtures use, for family resemblance between the file-driver fixtures.
const FEATURES: [(&str, i64, f64, f64); 5] = [
    ("alpha", 1, -4.0, 46.0),
    ("bravo", 2, -2.0, 48.0),
    ("charlie", 3, 0.0, 50.0),
    ("delta", 4, 2.0, 52.0),
    ("echo", 5, 4.0, 54.0),
];

/// Provisions `path` with one well-formed feature table (`demo`: `id` BIGINT
/// PRIMARY KEY, `geom` BLOB, `name` VARCHAR, `population` BIGINT), seeded
/// with [`FEATURES`] plus one `NULL`-geometry row, and a second,
/// deliberately ambiguous table (`two_blobs`) with two `BLOB` columns and no
/// declared primary key — the fixtures every refusal test below needs,
/// alongside the well-formed one every happy-path test uses.
fn provision(path: &Path) {
    let conn = Connection::open(path).expect("creates the .duckdb file");
    conn.execute_batch(
        "CREATE TABLE demo (
             id BIGINT PRIMARY KEY, geom BLOB, name VARCHAR, population BIGINT
         );
         CREATE TABLE two_blobs (id BIGINT PRIMARY KEY, a BLOB, b BLOB);
         CREATE TABLE no_pk (id BIGINT, geom BLOB);",
    )
    .expect("provisions the fixture tables");

    for (name, population, lon, lat) in FEATURES {
        conn.execute(
            "INSERT INTO demo (id, geom, name, population) VALUES (?, ?, ?, ?)",
            duckdb::params![population, point_wkb(lon, lat), name, population],
        )
        .expect("seeds one fixture feature");
    }
    conn.execute(
        "INSERT INTO demo (id, geom, name, population) VALUES (99, NULL, 'no-geometry', 0)",
        [],
    )
    .expect("seeds the null-geometry row");
}

fn build_driver(path: &Path) -> Arc<dyn StorageDriver> {
    let env_var = format!(
        "TELLURION_DUCKDB_CONTRACT_TEST_{}",
        path.file_name()
            .unwrap()
            .to_string_lossy()
            .replace(['-', '.'], "_")
    );
    std::env::set_var(&env_var, path);
    let decl = StorageDecl {
        id: "main".to_string(),
        driver: "duckdb".to_string(),
        url_env: env_var,
        pool_size: None,
    };
    DuckdbDriverFactory::new()
        .build(&decl)
        .expect("builds the driver against a provisioned fixture")
}

fn collection(table: &str) -> CollectionDecl {
    serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\n"
    ))
    .unwrap()
}

fn collection_with_overrides(
    table: &str,
    geometry: Option<&str>,
    pk: Option<&str>,
) -> CollectionDecl {
    let mut decl = collection(table);
    decl.geometry = geometry.map(str::to_string);
    decl.pk = pk.map(str::to_string);
    decl
}

#[tokio::test]
async fn catalog_reports_the_auto_detected_shape_for_the_well_formed_table() {
    let path = temp_duckdb_path("catalog");
    provision(&path);
    let driver = build_driver(&path);

    let collections = driver.catalog_source().collections().await.unwrap();
    let demo = collections.iter().find(|c| c.name == "demo").unwrap();
    assert_eq!(demo.geometry_column.as_deref(), Some("geom"));
    assert_eq!(demo.primary_key.as_deref(), Some("id"));
    assert_eq!(demo.srid, Some(4326));

    // The ambiguous table (two BLOB columns) reports no geometry column at
    // all — an honest "cannot answer" rather than a guess.
    let ambiguous = collections.iter().find(|c| c.name == "two_blobs").unwrap();
    assert_eq!(ambiguous.geometry_column, None);
    assert_eq!(ambiguous.primary_key.as_deref(), Some("id"));

    cleanup(&path);
}

#[tokio::test]
async fn extent_and_row_estimate_and_attribute_schema_are_reported() {
    let path = temp_duckdb_path("descriptor");
    provision(&path);
    let driver = build_driver(&path);
    let catalog = driver.catalog_source();
    let physical = catalog
        .collections()
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.name == "demo")
        .unwrap();

    let extent = catalog.extent(&physical).await.unwrap().unwrap();
    assert_eq!(extent.bbox, [-4.0, 46.0, 4.0, 54.0]);

    assert_eq!(catalog.row_estimate(&physical).await.unwrap(), Some(6));

    let attributes = catalog.attribute_schema(&physical).await.unwrap().unwrap();
    let names: Vec<&str> = attributes.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["id", "name", "population"]);

    cleanup(&path);
}

#[tokio::test]
async fn validate_collection_passes_for_a_well_formed_declaration() {
    let path = temp_duckdb_path("validate-ok");
    provision(&path);
    let driver = build_driver(&path);
    assert!(driver.validate_collection(&collection("demo")).is_ok());
    cleanup(&path);
}

#[tokio::test]
async fn validate_collection_fails_fast_on_a_missing_table() {
    let path = temp_duckdb_path("validate-missing-table");
    provision(&path);
    let driver = build_driver(&path);
    assert!(matches!(
        driver.validate_collection(&collection("nope")),
        Err(CoreError::Config(_))
    ));
    cleanup(&path);
}

#[tokio::test]
async fn validate_collection_fails_fast_on_an_ambiguous_geometry_column_with_no_override() {
    let path = temp_duckdb_path("validate-ambiguous");
    provision(&path);
    let driver = build_driver(&path);
    assert!(matches!(
        driver.validate_collection(&collection("two_blobs")),
        Err(CoreError::Config(_))
    ));
    cleanup(&path);
}

#[tokio::test]
async fn validate_collection_passes_when_an_explicit_geometry_override_disambiguates() {
    let path = temp_duckdb_path("validate-pinned");
    provision(&path);
    let driver = build_driver(&path);
    let decl = collection_with_overrides("two_blobs", Some("a"), None);
    assert!(driver.validate_collection(&decl).is_ok());
    cleanup(&path);
}

#[tokio::test]
async fn validate_collection_fails_fast_on_a_table_with_no_primary_key() {
    let path = temp_duckdb_path("validate-no-pk");
    provision(&path);
    let driver = build_driver(&path);
    let decl = collection_with_overrides("no_pk", Some("geom"), None);
    assert!(matches!(
        driver.validate_collection(&decl),
        Err(CoreError::Config(_))
    ));
    cleanup(&path);
}

#[tokio::test]
async fn items_pages_across_every_feature_exactly_once_with_an_exact_count() {
    let path = temp_duckdb_path("paging");
    provision(&path);
    let driver = build_driver(&path);
    let features = driver.feature_source().unwrap();
    let decl = collection("demo");

    let mut ids = std::collections::HashSet::new();
    let mut token: Option<String> = None;
    loop {
        let query = ItemsQuery {
            limit: 2,
            token: token.clone(),
            ..ItemsQuery::default()
        };
        let page = features.items(&decl, &query).await.unwrap();
        assert_eq!(page.number_matched, Some(6));
        for feature in &page.features_geojson {
            let id = feature["id"].as_str().unwrap().to_string();
            assert!(ids.insert(id), "an id repeated across pages");
        }
        match page.next_token {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    assert_eq!(ids.len(), 6);
    cleanup(&path);
}

#[tokio::test]
async fn a_null_geometry_row_serves_with_null_geometry_and_is_excluded_from_bbox() {
    let path = temp_duckdb_path("null-geometry");
    provision(&path);
    let driver = build_driver(&path);
    let features = driver.feature_source().unwrap();
    let decl = collection("demo");

    let item = features.item(&decl, "99", None).await.unwrap().unwrap();
    assert_eq!(item["geometry"], serde_json::Value::Null);

    let query = ItemsQuery {
        bbox: Some([-180.0, -90.0, 180.0, 90.0]),
        limit: 100,
        ..ItemsQuery::default()
    };
    let page = features.items(&decl, &query).await.unwrap();
    assert!(page.features_geojson.iter().all(|f| f["id"] != "99"));
    cleanup(&path);
}

#[tokio::test]
async fn bbox_query_returns_only_matching_features() {
    let path = temp_duckdb_path("bbox");
    provision(&path);
    let driver = build_driver(&path);
    let features = driver.feature_source().unwrap();
    let decl = collection("demo");

    let query = ItemsQuery {
        bbox: Some([-5.0, 45.0, -1.0, 55.0]),
        limit: 100,
        ..ItemsQuery::default()
    };
    let page = features.items(&decl, &query).await.unwrap();
    assert!(!page.features_geojson.is_empty());
    assert!(page.features_geojson.len() < 6);
    for feature in &page.features_geojson {
        let x = feature["geometry"]["coordinates"][0].as_f64().unwrap();
        assert!(x <= -1.0, "feature outside the requested bbox: {feature}");
    }
    cleanup(&path);
}

#[tokio::test]
async fn cql2_filter_narrows_items_and_refuses_an_unsupported_construct() {
    let path = temp_duckdb_path("cql2");
    provision(&path);
    let driver = build_driver(&path);
    let features = driver.feature_source().unwrap();
    let decl = collection("demo");

    assert!(features.filter_capable());

    let filter = Filter::Compare {
        property: "name".to_string(),
        op: CompareOp::Eq,
        value: Literal::Text("charlie".to_string()),
    };
    let query = ItemsQuery {
        filter: Some(filter),
        limit: 10,
        ..ItemsQuery::default()
    };
    let page = features.items(&decl, &query).await.unwrap();
    assert_eq!(page.features_geojson.len(), 1);
    assert_eq!(page.features_geojson[0]["properties"]["name"], "charlie");

    let like = Filter::Like {
        property: "name".to_string(),
        pattern: "c%".to_string(),
        negated: false,
    };
    let like_query = ItemsQuery {
        filter: Some(like),
        limit: 10,
        ..ItemsQuery::default()
    };
    assert!(matches!(
        features.items(&decl, &like_query).await,
        Err(CoreError::Invalid(_))
    ));

    cleanup(&path);
}

#[tokio::test]
async fn datetime_filter_is_refused_honestly() {
    let path = temp_duckdb_path("datetime");
    provision(&path);
    let driver = build_driver(&path);
    let features = driver.feature_source().unwrap();
    let decl = collection("demo");

    let query = ItemsQuery {
        datetime: Some(tellurion_core::DatetimeRange {
            start: Some("2020-01-01T00:00:00Z".to_string()),
            end: None,
        }),
        ..ItemsQuery::default()
    };
    assert!(matches!(
        features.items(&decl, &query).await,
        Err(CoreError::Invalid(_))
    ));
    cleanup(&path);
}

#[tokio::test]
async fn item_lookup_round_trips_a_feature_by_its_real_primary_key() {
    let path = temp_duckdb_path("item");
    provision(&path);
    let driver = build_driver(&path);
    let features = driver.feature_source().unwrap();
    let decl = collection("demo");

    let found = features.item(&decl, "3", None).await.unwrap().unwrap();
    assert_eq!(found["id"], "3");
    assert_eq!(found["properties"]["name"], "charlie");
    assert_eq!(found["geometry"]["type"], "Point");

    assert_eq!(
        features.item(&decl, "not-a-number", None).await.unwrap(),
        None
    );
    assert_eq!(features.item(&decl, "999", None).await.unwrap(), None);
    cleanup(&path);
}

#[tokio::test]
async fn cql2_conformance_classes_stays_basic_only() {
    let path = temp_duckdb_path("conformance");
    provision(&path);
    let driver = build_driver(&path);
    let features = driver.feature_source().unwrap();
    let declared = features.cql2_conformance_classes();
    assert_eq!(
        declared,
        vec![
            tellurion_core::filter::CQL2_CLASS_BASIC,
            tellurion_core::filter::CQL2_CLASS_CQL2_TEXT,
            tellurion_core::filter::CQL2_CLASS_CQL2_JSON,
        ]
    );
    assert_eq!(features.filter_capable(), !declared.is_empty());
    cleanup(&path);
}

#[test]
fn build_fails_fast_when_the_configured_path_does_not_exist() {
    let env_var = "TELLURION_DUCKDB_CONTRACT_TEST_MISSING_FILE";
    std::env::set_var(
        env_var,
        "/tmp/tellurion-duckdb-contract-test-does-not-exist.duckdb",
    );
    let decl = StorageDecl {
        id: "main".to_string(),
        driver: "duckdb".to_string(),
        url_env: env_var.to_string(),
        pool_size: None,
    };
    assert!(matches!(
        DuckdbDriverFactory::new().build(&decl),
        Err(CoreError::Config(_))
    ));
}
