use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use arrow_array::{ArrayRef, BinaryArray, Float64Array, RecordBatch, StringArray, StructArray};
use arrow_schema::{DataType, Field, Fields, Schema};
use async_trait::async_trait;
use bytes::Bytes;
use geozero::GeozeroGeometry;
use parquet::arrow::ArrowWriter;
use parquet::file::metadata::{KeyValue, ParquetMetaDataReader};
use parquet::file::properties::WriterProperties;
use tellurion_core::{CatalogSource, CollectionDecl, FeatureSource, ItemsQuery};
use tellurion_geoparquet::{GeoparquetBackend, GeoparquetInput};
use tellurion_http_source::{
    ContentIdentity, RangeObject, SourceError, SourceErrorKind, SourceHandle,
};

const LOCATOR: &str = "https://secret.example.test/private/tiny.parquet";

struct FixtureRangeObject {
    handle: SourceHandle,
    identities: [ContentIdentity; 2],
    identity_index: AtomicUsize,
    bytes: Bytes,
    requested_ranges: Mutex<Vec<Range<u64>>>,
    fail_after_requests: Option<usize>,
    switch_revision_after_requests: Option<usize>,
    data_failure: Option<(u64, u64, SourceErrorKind)>,
}

impl FixtureRangeObject {
    fn new(bytes: Vec<u8>) -> Self {
        Self::with_behavior(bytes, None, None)
    }

    fn with_budget_exhaustion(bytes: Vec<u8>, fail_after_requests: usize) -> Self {
        Self::with_behavior(bytes, Some(fail_after_requests), None)
    }

    fn with_revision_switch(bytes: Vec<u8>, switch_after_requests: usize) -> Self {
        Self::with_behavior(bytes, None, Some(switch_after_requests))
    }

    fn with_behavior(
        bytes: Vec<u8>,
        fail_after_requests: Option<usize>,
        switch_revision_after_requests: Option<usize>,
    ) -> Self {
        let length = bytes.len() as u64;
        Self {
            handle: SourceHandle::new("remote-geoparquet-fixture"),
            identities: [
                ContentIdentity::StrongEtag {
                    source_key: [1; 32],
                    revision_key: [2; 32],
                    length,
                },
                ContentIdentity::StrongEtag {
                    source_key: [1; 32],
                    revision_key: [3; 32],
                    length,
                },
            ],
            identity_index: AtomicUsize::new(0),
            bytes: Bytes::from(bytes),
            requested_ranges: Mutex::new(Vec::new()),
            fail_after_requests,
            switch_revision_after_requests,
            data_failure: None,
        }
    }

    fn fetched_bytes(&self) -> u64 {
        self.requested_ranges
            .lock()
            .unwrap()
            .iter()
            .map(|range| range.end - range.start)
            .sum()
    }

    fn with_data_failure(mut self, start: u64, end: u64, kind: SourceErrorKind) -> Self {
        self.data_failure = Some((start, end, kind));
        self
    }

    fn requested_ranges(&self) -> Vec<Range<u64>> {
        self.requested_ranges.lock().unwrap().clone()
    }
}

#[async_trait]
impl RangeObject for FixtureRangeObject {
    fn handle(&self) -> &SourceHandle {
        &self.handle
    }

    fn identity(&self) -> &ContentIdentity {
        &self.identities[self.identity_index.load(Ordering::Acquire)]
    }

    fn length(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn display_name(&self) -> &str {
        LOCATOR
    }

    async fn get_range(&self, range: Range<u64>) -> Result<Bytes, SourceError> {
        let request_count = {
            let mut requested = self.requested_ranges.lock().unwrap();
            requested.push(range.clone());
            requested.len()
        };
        if self
            .fail_after_requests
            .is_some_and(|limit| request_count > limit)
        {
            return Err(SourceError::for_handle(
                SourceErrorKind::Budget,
                &self.handle,
            ));
        }
        if let Some((start, end, kind)) = self.data_failure {
            if range.start >= start && range.end <= end {
                return Err(SourceError::for_handle(kind, &self.handle));
            }
        }
        if self
            .switch_revision_after_requests
            .is_some_and(|limit| request_count >= limit)
        {
            self.identity_index.store(1, Ordering::Release);
        }
        Ok(self.bytes.slice(range.start as usize..range.end as usize))
    }
}

fn fixture_bytes() -> Vec<u8> {
    std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny.parquet"),
    )
    .unwrap()
}

fn decl() -> CollectionDecl {
    serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap()
}

struct TwoGroupFixture {
    bytes: Vec<u8>,
    second_group_start: u64,
    data_end: u64,
    second_group_payload: Range<u64>,
}

fn point_wkb(x: f64, y: f64) -> Vec<u8> {
    let geojson = format!(r#"{{"type":"Point","coordinates":[{x},{y}]}}"#);
    let mut bytes = Vec::new();
    let mut writer = geozero::wkb::WkbWriter::new(&mut bytes, geozero::wkb::WkbDialect::Wkb);
    geozero::geojson::GeoJson(&geojson)
        .process_geom(&mut writer)
        .unwrap();
    bytes
}

fn two_group_fixture() -> TwoGroupFixture {
    two_group_fixture_with_first_group(vec![
        (-4.0, 46.0, "first-payload".repeat(64)),
        (-2.0, 48.0, "second-payload".repeat(64)),
    ])
}

fn two_group_fixture_with_first_group(first_group: Vec<(f64, f64, String)>) -> TwoGroupFixture {
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
    let geo = serde_json::json!({
        "version": "1.1.0",
        "primary_column": "geometry",
        "columns": {
            "geometry": {
                "encoding": "WKB",
                "covering": { "bbox": {
                    "xmin": ["bbox", "xmin"], "ymin": ["bbox", "ymin"],
                    "xmax": ["bbox", "xmax"], "ymax": ["bbox", "ymax"]
                }}
            }
        }
    })
    .to_string();
    let mut bytes = Vec::new();
    let properties = WriterProperties::builder()
        .set_key_value_metadata(Some(vec![KeyValue::new("geo".to_string(), geo)]))
        .build();
    let mut writer =
        ArrowWriter::try_new(&mut bytes, Arc::clone(&schema), Some(properties)).unwrap();
    for rows in [
        first_group,
        vec![(0.0, 50.0, "count-only-payload".repeat(64))],
    ] {
        let x: Vec<f64> = rows.iter().map(|(x, _, _)| *x).collect();
        let y: Vec<f64> = rows.iter().map(|(_, y, _)| *y).collect();
        let geometry: Vec<Vec<u8>> = rows.iter().map(|(x, y, _)| point_wkb(*x, *y)).collect();
        let geometry_refs: Vec<&[u8]> = geometry.iter().map(Vec::as_slice).collect();
        let payload: Vec<String> = rows.into_iter().map(|(_, _, payload)| payload).collect();
        let bbox = StructArray::new(
            bbox_fields.clone(),
            vec![
                Arc::new(Float64Array::from(x.clone())) as ArrayRef,
                Arc::new(Float64Array::from(y.clone())) as ArrayRef,
                Arc::new(Float64Array::from(x)) as ArrayRef,
                Arc::new(Float64Array::from(y)) as ArrayRef,
            ],
            None,
        );
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(BinaryArray::from(geometry_refs)) as ArrayRef,
                Arc::new(bbox) as ArrayRef,
                Arc::new(StringArray::from(payload)) as ArrayRef,
            ],
        )
        .unwrap();
        writer.write(&batch).unwrap();
        writer.flush().unwrap();
    }
    writer.close().unwrap();

    let footer_length =
        u32::from_le_bytes(bytes[bytes.len() - 8..bytes.len() - 4].try_into().unwrap()) as u64;
    let data_end = bytes.len() as u64 - footer_length - 8;
    let mut metadata_reader = ParquetMetaDataReader::new();
    metadata_reader
        .try_parse_sized(&Bytes::from(bytes.clone()), bytes.len() as u64)
        .unwrap();
    let metadata = metadata_reader.finish().unwrap();
    let second = metadata.row_group(1);
    let second_group_start = second
        .columns()
        .iter()
        .map(|column| column.data_page_offset() as u64)
        .min()
        .unwrap();
    let payload = second.column(5);
    let payload_start = payload.data_page_offset() as u64;
    let second_group_payload = payload_start..payload_start + payload.compressed_size() as u64;

    TwoGroupFixture {
        bytes,
        second_group_start,
        data_end,
        second_group_payload,
    }
}

#[tokio::test]
async fn remote_limit_one_keeps_the_first_stable_id_and_avoids_a_full_download() {
    let bytes = fixture_bytes();
    let object = Arc::new(FixtureRangeObject::new(bytes.clone()));
    let backend = GeoparquetBackend::from_input(GeoparquetInput::Remote(object.clone()));

    let page = backend
        .items(
            &decl(),
            &ItemsQuery {
                limit: 1,
                ..ItemsQuery::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(page.features_geojson.len(), 1);
    assert_eq!(page.features_geojson[0]["id"], "0");
    assert_eq!(page.number_matched, Some(5));
    assert!(object.fetched_bytes() < bytes.len() as u64);
}

#[tokio::test]
async fn remote_bbox_pages_with_physical_ids_and_an_exact_count() {
    let backend = GeoparquetBackend::from_input(GeoparquetInput::Remote(Arc::new(
        FixtureRangeObject::new(fixture_bytes()),
    )));
    let first = backend
        .items(
            &decl(),
            &ItemsQuery {
                bbox: Some([-5.0, 45.0, -1.0, 55.0]),
                limit: 1,
                ..ItemsQuery::default()
            },
        )
        .await
        .unwrap();
    let token = first
        .next_token
        .clone()
        .expect("bbox page has a next cursor");
    let second = backend
        .items(
            &decl(),
            &ItemsQuery {
                bbox: Some([-5.0, 45.0, -1.0, 55.0]),
                limit: 1,
                token: Some(token),
                ..ItemsQuery::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(first.features_geojson[0]["id"], "0");
    assert_eq!(second.features_geojson[0]["id"], "1");
    assert_eq!(first.number_matched, Some(2));
    assert_eq!(second.number_matched, Some(2));
}

#[tokio::test]
async fn remote_identity_change_is_rejected() {
    let backend = GeoparquetBackend::from_input(GeoparquetInput::Remote(Arc::new(
        FixtureRangeObject::with_revision_switch(fixture_bytes(), 1),
    )));

    let error = backend.collections().await.unwrap_err();

    assert!(error.to_string().contains("source is invalidated"));
    assert!(!error.to_string().contains(LOCATOR));
}

#[tokio::test]
async fn malformed_remote_geo_metadata_is_a_storage_error() {
    let mut bytes = fixture_bytes();
    let metadata = bytes
        .windows(b"{\"columns\"".len())
        .position(|window| window == b"{\"columns\"")
        .expect("fixture has geo metadata");
    bytes[metadata] = b'!';
    let backend = GeoparquetBackend::from_input(GeoparquetInput::Remote(Arc::new(
        FixtureRangeObject::new(bytes),
    )));

    let error = backend.collections().await.unwrap_err();

    assert!(error.to_string().contains("malformed JSON"));
}

#[tokio::test]
async fn remote_budget_exhaustion_is_redacted() {
    let backend = GeoparquetBackend::from_input(GeoparquetInput::Remote(Arc::new(
        FixtureRangeObject::with_budget_exhaustion(fixture_bytes(), 1),
    )));

    let error = backend.collections().await.unwrap_err();

    assert!(error.to_string().contains("source budget exceeded"));
    assert!(!error.to_string().contains(LOCATOR));
}

#[tokio::test]
async fn completed_bbox_page_omits_count_when_only_the_remaining_count_hits_budget() {
    let fixture = two_group_fixture();
    let failure_start = fixture.second_group_start;
    let data_end = fixture.data_end;
    let object = Arc::new(FixtureRangeObject::new(fixture.bytes).with_data_failure(
        failure_start,
        data_end,
        SourceErrorKind::Budget,
    ));
    let backend = GeoparquetBackend::from_input(GeoparquetInput::Remote(object));

    let page = backend
        .items(
            &decl(),
            &ItemsQuery {
                bbox: Some([-5.0, 45.0, 1.0, 51.0]),
                limit: 1,
                ..ItemsQuery::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(page.features_geojson[0]["id"], "0");
    assert_eq!(
        page.features_geojson[0]["properties"]["payload"],
        "first-payload".repeat(64)
    );
    assert_eq!(page.next_token.as_deref(), Some("0"));
    assert_eq!(page.number_matched, None);
}

#[tokio::test]
async fn completed_bbox_page_keeps_an_optimistic_cursor_when_count_hits_budget_before_a_match() {
    let fixture =
        two_group_fixture_with_first_group(vec![(-4.0, 46.0, "first-payload".repeat(64))]);
    let failure_start = fixture.second_group_start;
    let data_end = fixture.data_end;
    let backend = GeoparquetBackend::from_input(GeoparquetInput::Remote(Arc::new(
        FixtureRangeObject::new(fixture.bytes).with_data_failure(
            failure_start,
            data_end,
            SourceErrorKind::Budget,
        ),
    )));

    let page = backend
        .items(
            &decl(),
            &ItemsQuery {
                bbox: Some([-5.0, 45.0, 1.0, 51.0]),
                limit: 1,
                ..ItemsQuery::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(page.features_geojson[0]["id"], "0");
    assert_eq!(page.next_token.as_deref(), Some("0"));
    assert_eq!(page.number_matched, None);
}

#[tokio::test]
async fn bbox_failure_before_the_requested_page_is_complete_propagates() {
    let fixture = two_group_fixture();
    let data_end = fixture.data_end;
    let backend = GeoparquetBackend::from_input(GeoparquetInput::Remote(Arc::new(
        FixtureRangeObject::new(fixture.bytes).with_data_failure(
            4,
            data_end,
            SourceErrorKind::Budget,
        ),
    )));

    let error = backend
        .items(
            &decl(),
            &ItemsQuery {
                bbox: Some([-5.0, 45.0, 1.0, 51.0]),
                limit: 1,
                ..ItemsQuery::default()
            },
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("source budget exceeded"));
}

#[tokio::test]
async fn completed_bbox_page_propagates_non_budget_count_failures() {
    let fixture = two_group_fixture();
    let failure_start = fixture.second_group_start;
    let data_end = fixture.data_end;
    let backend = GeoparquetBackend::from_input(GeoparquetInput::Remote(Arc::new(
        FixtureRangeObject::new(fixture.bytes).with_data_failure(
            failure_start,
            data_end,
            SourceErrorKind::Transport,
        ),
    )));

    let error = backend
        .items(
            &decl(),
            &ItemsQuery {
                bbox: Some([-5.0, 45.0, 1.0, 51.0]),
                limit: 1,
                ..ItemsQuery::default()
            },
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("source transport failed"));
}

#[tokio::test]
async fn count_only_bbox_continuation_skips_unrelated_attribute_chunks() {
    let fixture = two_group_fixture();
    let payload_range = fixture.second_group_payload.clone();
    let data_end = fixture.data_end;
    let object = Arc::new(FixtureRangeObject::new(fixture.bytes));
    let backend = GeoparquetBackend::from_input(GeoparquetInput::Remote(object.clone()));

    let page = backend
        .items(
            &decl(),
            &ItemsQuery {
                bbox: Some([-5.0, 45.0, 1.0, 51.0]),
                limit: 1,
                ..ItemsQuery::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(page.number_matched, Some(3));
    let requested = object.requested_ranges();
    assert!(
        requested
            .iter()
            .filter(|range| range.end <= data_end)
            .all(|range| { range.end <= payload_range.start || range.start >= payload_range.end }),
        "payload range {payload_range:?}, requested {requested:?}"
    );
}
