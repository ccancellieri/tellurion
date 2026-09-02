use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "remote")]
use std::ops::Range;
#[cfg(feature = "remote")]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(feature = "remote")]
use std::sync::Mutex;

use arrow_array::{ArrayRef, BinaryArray, Float64Array, RecordBatch, StringArray, StructArray};
use arrow_schema::{DataType, Field, Fields, Schema};
#[cfg(feature = "remote")]
use async_trait::async_trait;
#[cfg(feature = "remote")]
use bytes::Bytes;
use geozero::mvt::{Message, Tile};
use geozero::{GeozeroGeometry, ProcessToJson};
use parquet::arrow::ArrowWriter;
use parquet::file::metadata::KeyValue;
use parquet::file::properties::WriterProperties;
use tellurion_core::{
    CatalogSource, CollectionDecl, DriverFactory, Error, StorageDecl, StorageDriver, TileCoord,
    TileSource,
};
#[cfg(feature = "remote")]
use tellurion_geoparquet::GeoparquetInput;
use tellurion_geoparquet::{GeoparquetBackend, GeoparquetDriverFactory};
#[cfg(feature = "remote")]
use tellurion_http_source::{ContentIdentity, RangeObject, SourceError, SourceHandle};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny.parquet")
}

fn configured_driver() -> Arc<dyn StorageDriver> {
    let env_var = "TELLURION_GEOPARQUET_TILES_TEST_FILE";
    std::env::set_var(env_var, fixture_path());
    let driver = GeoparquetDriverFactory::new()
        .build(&StorageDecl {
            id: "main".to_string(),
            driver: "geoparquet".to_string(),
            url_env: env_var.to_string(),
            pool_size: None,
        })
        .unwrap();
    std::env::remove_var(env_var);
    driver
}

fn collection() -> CollectionDecl {
    let mut collection: CollectionDecl = serde_yaml::from_str(
        "id: internal-demo\nexternal_id: public-demo\ncatalog: default\nstorage: main\n",
    )
    .unwrap();
    collection.srid = Some(4326);
    collection.row_estimate = Some(5);
    collection.tile_properties = vec!["name".to_string(), "value".to_string()];
    collection.settings.tile_vertex_budget = Some(500_000);
    collection
}

fn decode(bytes: &[u8]) -> Tile {
    Tile::decode(bytes).expect("GeoParquet returns one valid MVT document")
}

fn crossing_line_fixture() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "tellurion-geoparquet-crossing-line-{}-{stamp}.parquet",
        std::process::id()
    ));
    let mut wkb = Vec::new();
    let mut wkb_writer = geozero::wkb::WkbWriter::new(&mut wkb, geozero::wkb::WkbDialect::Wkb);
    geozero::geojson::GeoJson(r#"{"type":"LineString","coordinates":[[-100.0,20.0],[10.0,20.0]]}"#)
        .process_geom(&mut wkb_writer)
        .unwrap();
    let bbox_fields = Fields::from(vec![
        Field::new("xmin", DataType::Float64, false),
        Field::new("ymin", DataType::Float64, false),
        Field::new("xmax", DataType::Float64, false),
        Field::new("ymax", DataType::Float64, false),
    ]);
    let schema = Arc::new(Schema::new(vec![
        Field::new("geometry", DataType::Binary, false),
        Field::new("bbox", DataType::Struct(bbox_fields.clone()), false),
    ]));
    let batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![
            Arc::new(BinaryArray::from(vec![wkb.as_slice()])) as ArrayRef,
            Arc::new(StructArray::new(
                bbox_fields,
                vec![
                    Arc::new(Float64Array::from(vec![-100.0])) as ArrayRef,
                    Arc::new(Float64Array::from(vec![20.0])) as ArrayRef,
                    Arc::new(Float64Array::from(vec![10.0])) as ArrayRef,
                    Arc::new(Float64Array::from(vec![20.0])) as ArrayRef,
                ],
                None,
            )) as ArrayRef,
        ],
    )
    .unwrap();
    let geo = serde_json::json!({
        "version": "1.1.0",
        "primary_column": "geometry",
        "columns": {
            "geometry": {
                "encoding": "WKB",
                "geometry_types": ["LineString"],
                "bbox": [-100.0, 20.0, 10.0, 20.0],
                "covering": { "bbox": {
                    "xmin": ["bbox", "xmin"], "ymin": ["bbox", "ymin"],
                    "xmax": ["bbox", "xmax"], "ymax": ["bbox", "ymax"]
                }}
            }
        }
    })
    .to_string();
    let properties = WriterProperties::builder()
        .set_key_value_metadata(Some(vec![KeyValue::new("geo".to_string(), geo)]))
        .build();
    let file = std::fs::File::create(&path).unwrap();
    let mut writer = ArrowWriter::try_new(file, schema, Some(properties)).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
    path
}

#[cfg(feature = "remote")]
#[derive(Clone, Copy)]
enum TileCovering {
    Missing,
    Unusable,
    Usable,
}

#[cfg(feature = "remote")]
fn multi_group_tile_fixture(
    covering: TileCovering,
    rows_per_group: usize,
    payload_len: usize,
) -> Vec<u8> {
    let bbox_fields = Fields::from(vec![
        Field::new("xmin", DataType::Float64, false),
        Field::new("ymin", DataType::Float64, false),
        Field::new("xmax", DataType::Float64, false),
        Field::new("ymax", DataType::Float64, false),
    ]);
    let schema = Arc::new(Schema::new(vec![
        Field::new("geometry", DataType::Binary, false),
        Field::new("bbox", DataType::Struct(bbox_fields.clone()), false),
        Field::new("payload", DataType::Utf8, false),
    ]));
    let covering_paths = match covering {
        TileCovering::Missing => None,
        TileCovering::Unusable => Some(serde_json::json!({ "bbox": {
            "xmin": ["missing", "xmin"], "ymin": ["missing", "ymin"],
            "xmax": ["missing", "xmax"], "ymax": ["missing", "ymax"]
        }})),
        TileCovering::Usable => Some(serde_json::json!({ "bbox": {
            "xmin": ["bbox", "xmin"], "ymin": ["bbox", "ymin"],
            "xmax": ["bbox", "xmax"], "ymax": ["bbox", "ymax"]
        }})),
    };
    let mut geometry_metadata = serde_json::json!({ "encoding": "WKB" });
    if let Some(covering_paths) = covering_paths {
        geometry_metadata["covering"] = covering_paths;
    }
    let geo = serde_json::json!({
        "version": "1.1.0",
        "primary_column": "geometry",
        "columns": { "geometry": geometry_metadata }
    })
    .to_string();
    let properties = WriterProperties::builder()
        .set_key_value_metadata(Some(vec![KeyValue::new("geo".to_string(), geo)]))
        .build();
    let mut bytes = Vec::new();
    let mut writer =
        ArrowWriter::try_new(&mut bytes, Arc::clone(&schema), Some(properties)).unwrap();
    let west = point_wkb(-170.0, 30.0);
    let east = point_wkb(170.0, 30.0);
    for _ in 0..2 {
        let geometry: Vec<&[u8]> = (0..rows_per_group)
            .map(|row| {
                if row % 2 == 0 {
                    west.as_slice()
                } else {
                    east.as_slice()
                }
            })
            .collect();
        let x: Vec<f64> = (0..rows_per_group)
            .map(|row| if row % 2 == 0 { -170.0 } else { 170.0 })
            .collect();
        let bbox = StructArray::new(
            bbox_fields.clone(),
            vec![
                Arc::new(Float64Array::from(x.clone())) as ArrayRef,
                Arc::new(Float64Array::from(vec![30.0; rows_per_group])) as ArrayRef,
                Arc::new(Float64Array::from(x)) as ArrayRef,
                Arc::new(Float64Array::from(vec![30.0; rows_per_group])) as ArrayRef,
            ],
            None,
        );
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(BinaryArray::from(geometry)) as ArrayRef,
                Arc::new(bbox) as ArrayRef,
                Arc::new(StringArray::from(vec![
                    "x".repeat(payload_len);
                    rows_per_group
                ])) as ArrayRef,
            ],
        )
        .unwrap();
        writer.write(&batch).unwrap();
        writer.flush().unwrap();
    }
    writer.close().unwrap();
    bytes
}

#[cfg(feature = "remote")]
fn point_wkb(x: f64, y: f64) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut writer = geozero::wkb::WkbWriter::new(&mut bytes, geozero::wkb::WkbDialect::Wkb);
    geozero::geojson::GeoJson(format!(r#"{{"type":"Point","coordinates":[{x},{y}]}}"#).as_str())
        .process_geom(&mut writer)
        .unwrap();
    bytes
}

fn assert_tile_coordinates_bounded(value: &serde_json::Value) {
    match value {
        serde_json::Value::Array(values) => values.iter().for_each(assert_tile_coordinates_bounded),
        serde_json::Value::Number(value) => {
            let coordinate = value.as_f64().unwrap();
            assert!(
                (0.0..=4096.0).contains(&coordinate),
                "unclipped tile coordinate {coordinate}"
            );
        }
        _ => {}
    }
}

#[test]
fn configured_geoparquet_driver_advertises_a_tile_source() {
    let env_var = "TELLURION_GEOPARQUET_TILES_SOURCE_TEST_FILE";
    std::env::set_var(env_var, fixture_path());
    let driver = GeoparquetDriverFactory::new()
        .build(&StorageDecl {
            id: "main".to_string(),
            driver: "geoparquet".to_string(),
            url_env: env_var.to_string(),
            pool_size: None,
        })
        .unwrap();

    assert!(driver.tile_source().is_some());

    std::env::remove_var(env_var);
}

#[tokio::test]
async fn covering_and_empty_tiles_use_the_external_layer_name_and_feature_cap() {
    let tiles: Arc<dyn TileSource> = configured_driver().tile_source().unwrap();
    let mut collection = collection();
    collection.tiles.caps.0.insert(0, 2);

    let bytes = tiles
        .mvt_tile(&collection, TileCoord { z: 0, x: 0, y: 0 }, None)
        .await
        .unwrap()
        .expect("the world tile covers the fixture");
    let tile = decode(&bytes);
    assert_eq!(tile.layers.len(), 1);
    let layer = &tile.layers[0];
    assert_eq!(layer.name, "public-demo");
    assert_eq!(layer.features.len(), 2, "z0 feature cap is enforced");
    assert!(layer.keys.iter().any(|key| key == "name"));
    assert!(layer.keys.iter().any(|key| key == "value"));

    let empty = tiles
        .mvt_tile(&collection, TileCoord { z: 1, x: 0, y: 1 }, None)
        .await
        .unwrap();
    assert!(empty.is_none(), "a valid uncovered tile is empty");
}

#[tokio::test]
async fn vertex_cap_stops_the_bounded_tile_prefix() {
    let tiles = configured_driver().tile_source().unwrap();
    let mut collection = collection();
    collection.settings.tile_vertex_budget = Some(1);

    let bytes = tiles
        .mvt_tile(&collection, TileCoord { z: 0, x: 0, y: 0 }, None)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(decode(&bytes).layers[0].features.len(), 1);
}

#[tokio::test]
async fn crossing_geometry_is_topology_clipped_to_the_requested_tile() {
    let path = crossing_line_fixture();
    let backend =
        GeoparquetBackend::from_input(tellurion_geoparquet::GeoparquetInput::Local(path.clone()));
    let mut collection = collection();
    collection.row_estimate = Some(1);
    collection.tile_properties.clear();
    let bytes = TileSource::mvt_tile(&backend, &collection, TileCoord { z: 2, x: 1, y: 1 }, None)
        .await
        .unwrap()
        .unwrap();
    let mut layer = decode(&bytes).layers.remove(0);
    let decoded: serde_json::Value = serde_json::from_str(&layer.to_json().unwrap()).unwrap();
    assert_tile_coordinates_bounded(&decoded["features"][0]["geometry"]["coordinates"]);

    std::fs::remove_file(path).unwrap();
}

#[cfg(feature = "remote")]
struct NoReadRangeObject {
    handle: SourceHandle,
    identity: ContentIdentity,
    reads: AtomicUsize,
}

#[cfg(feature = "remote")]
impl NoReadRangeObject {
    fn new() -> Self {
        Self {
            handle: SourceHandle::new("must-not-read"),
            identity: ContentIdentity::StrongEtag {
                source_key: [1; 32],
                revision_key: [2; 32],
                length: 1,
            },
            reads: AtomicUsize::new(0),
        }
    }
}

#[cfg(feature = "remote")]
struct CountingRangeObject {
    handle: SourceHandle,
    identity: ContentIdentity,
    bytes: Bytes,
    reads: Mutex<Vec<Range<u64>>>,
}

#[cfg(feature = "remote")]
impl CountingRangeObject {
    fn new(bytes: Vec<u8>) -> Self {
        let length = bytes.len() as u64;
        Self {
            handle: SourceHandle::new("bounded-tile-fixture"),
            identity: ContentIdentity::StrongEtag {
                source_key: [3; 32],
                revision_key: [4; 32],
                length,
            },
            bytes: Bytes::from(bytes),
            reads: Mutex::new(Vec::new()),
        }
    }

    fn read_count(&self) -> usize {
        self.reads.lock().unwrap().len()
    }
}

#[cfg(feature = "remote")]
#[async_trait]
impl RangeObject for CountingRangeObject {
    fn handle(&self) -> &SourceHandle {
        &self.handle
    }

    fn identity(&self) -> &ContentIdentity {
        &self.identity
    }

    fn length(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn display_name(&self) -> &str {
        "bounded-tile-fixture.parquet"
    }

    async fn get_range(&self, range: Range<u64>) -> Result<Bytes, SourceError> {
        self.reads.lock().unwrap().push(range.clone());
        Ok(self.bytes.slice(range.start as usize..range.end as usize))
    }
}

#[cfg(feature = "remote")]
#[async_trait]
impl RangeObject for NoReadRangeObject {
    fn handle(&self) -> &SourceHandle {
        &self.handle
    }

    fn identity(&self) -> &ContentIdentity {
        &self.identity
    }

    fn length(&self) -> u64 {
        1
    }

    fn display_name(&self) -> &str {
        "must-not-read.parquet"
    }

    async fn get_range(&self, _range: Range<u64>) -> Result<Bytes, SourceError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        panic!("unsupported CRS must be refused before source I/O")
    }
}

#[cfg(feature = "remote")]
#[tokio::test]
async fn projected_and_unknown_resolved_crs_are_refused_before_source_io() {
    for srid in [Some(3857), None] {
        let object = Arc::new(NoReadRangeObject::new());
        let backend = GeoparquetBackend::from_input(GeoparquetInput::Remote(object.clone()));
        let mut collection = collection();
        collection.srid = srid;

        let error =
            TileSource::mvt_tile(&backend, &collection, TileCoord { z: 0, x: 0, y: 0 }, None)
                .await
                .unwrap_err();

        assert!(matches!(
            error,
            Error::CapabilityUnsupported { ref capability, .. }
                if capability == "tiles:crs84"
        ));
        assert_eq!(object.reads.load(Ordering::Relaxed), 0);
    }
}

#[cfg(feature = "remote")]
#[tokio::test]
async fn remote_tiles_refuse_multi_group_missing_or_unusable_covering_before_candidate_reads() {
    for covering in [TileCovering::Missing, TileCovering::Unusable] {
        let object = Arc::new(CountingRangeObject::new(multi_group_tile_fixture(
            covering, 1, 16_384,
        )));
        let input: Arc<dyn RangeObject> = Arc::clone(&object) as Arc<dyn RangeObject>;
        let backend = GeoparquetBackend::from_input(GeoparquetInput::Remote(input));
        backend.collections().await.unwrap();
        let reads_before_tile = object.read_count();

        let error = TileSource::mvt_tile(
            &backend,
            &collection(),
            TileCoord { z: 2, x: 2, y: 1 },
            None,
        )
        .await
        .unwrap_err();

        assert!(matches!(
            error,
            Error::CapabilityUnsupported { ref capability, .. }
                if capability == "tiles:covering"
        ));
        assert_eq!(object.read_count(), reads_before_tile);
    }
}

#[cfg(feature = "remote")]
#[tokio::test]
async fn remote_tile_scan_budget_refuses_multi_group_candidates_before_reading_them() {
    let object = Arc::new(CountingRangeObject::new(multi_group_tile_fixture(
        TileCovering::Usable,
        1_025,
        1,
    )));
    let input: Arc<dyn RangeObject> = Arc::clone(&object) as Arc<dyn RangeObject>;
    let backend = GeoparquetBackend::from_input(GeoparquetInput::Remote(input));
    backend.collections().await.unwrap();
    let reads_before_tile = object.read_count();

    let error = TileSource::mvt_tile(
        &backend,
        &collection(),
        TileCoord { z: 2, x: 2, y: 1 },
        None,
    )
    .await
    .unwrap_err();

    assert!(matches!(
        error,
        Error::CapabilityUnsupported { ref capability, .. }
            if capability == "tiles:scan-budget"
    ));
    assert_eq!(object.read_count(), reads_before_tile);
}
