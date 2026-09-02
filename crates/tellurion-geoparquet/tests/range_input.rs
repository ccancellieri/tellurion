use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use arrow_array::{Int32Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use bytes::Bytes;
use parquet::arrow::arrow_reader::ArrowReaderOptions;
use parquet::arrow::async_reader::AsyncFileReader;
use parquet::arrow::ArrowWriter;
use parquet::errors::ParquetError;
use parquet::file::metadata::{PageIndexPolicy, ParquetStatisticsPolicy};
use parquet::file::properties::WriterProperties;
use tellurion_geoparquet::RemoteParquetReader;
use tellurion_http_source::{
    ContentIdentity, RangeObject, SourceError, SourceErrorKind, SourceHandle,
};

struct FixtureRangeObject {
    handle: SourceHandle,
    identities: [ContentIdentity; 2],
    identity_index: AtomicUsize,
    bytes: Bytes,
    requested_ranges: Mutex<Vec<Range<u64>>>,
    switch_revision_on_request: Option<usize>,
}

impl FixtureRangeObject {
    fn new(bytes: Vec<u8>) -> Self {
        Self::with_revision_switch(bytes, None)
    }

    fn switch_revision_after_first_request(bytes: Vec<u8>) -> Self {
        Self::with_revision_switch(bytes, Some(1))
    }

    fn with_revision_switch(bytes: Vec<u8>, switch_revision_on_request: Option<usize>) -> Self {
        let length = bytes.len() as u64;
        Self {
            handle: SourceHandle::new("geoparquet-range-fixture"),
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
            switch_revision_on_request,
        }
    }

    fn requested_ranges(&self) -> Vec<Range<u64>> {
        self.requested_ranges.lock().unwrap().clone()
    }

    fn fetched_bytes(&self) -> u64 {
        self.requested_ranges()
            .into_iter()
            .map(|range| range.end - range.start)
            .sum()
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
        "tiny.parquet"
    }

    async fn get_range(&self, range: Range<u64>) -> Result<Bytes, SourceError> {
        let request_count = {
            let mut requested_ranges = self.requested_ranges.lock().unwrap();
            requested_ranges.push(range.clone());
            requested_ranges.len()
        };
        if self.switch_revision_on_request == Some(request_count) {
            self.identity_index.store(1, Ordering::Release);
        }
        Ok(self.bytes.slice(range.start as usize..range.end as usize))
    }
}

fn parquet_with_page_indexes() -> Vec<u8> {
    let schema = Arc::new(Schema::new(vec![Field::new(
        "value",
        DataType::Int32,
        false,
    )]));
    let batch =
        RecordBatch::try_new(schema.clone(), vec![Arc::new(Int32Array::from(vec![1]))]).unwrap();
    let properties = WriterProperties::builder()
        .set_offset_index_disabled(false)
        .build();
    let mut bytes = Vec::new();
    let mut writer = ArrowWriter::try_new(&mut bytes, schema, Some(properties)).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
    bytes
}

#[tokio::test]
async fn remote_reader_delegates_the_exact_requested_range() {
    let object = Arc::new(FixtureRangeObject::new((0_u8..40).collect()));
    let mut reader = RemoteParquetReader::new(object.clone());

    let bytes = reader.get_bytes(10..20).await.unwrap();

    assert_eq!(
        bytes,
        Bytes::from_static(&[10, 11, 12, 13, 14, 15, 16, 17, 18, 19])
    );
    assert_eq!(object.requested_ranges(), vec![10..20]);
    assert_eq!(object.fetched_bytes(), 10);
}

#[tokio::test]
async fn remote_reader_loads_and_caches_metadata_from_footer_ranges() {
    let fixture =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny.parquet");
    let bytes = std::fs::read(fixture).unwrap();
    let length = bytes.len() as u64;
    let footer_metadata_length =
        u32::from_le_bytes(bytes[bytes.len() - 8..bytes.len() - 4].try_into().unwrap()) as u64;
    let object = Arc::new(FixtureRangeObject::new(bytes));
    let mut reader = RemoteParquetReader::new(object.clone());

    let metadata = reader.get_metadata(None).await.unwrap();
    let cached_metadata = reader.get_metadata(None).await.unwrap();

    assert_eq!(metadata.num_row_groups(), 1);
    assert!(Arc::ptr_eq(&metadata, &cached_metadata));
    assert_eq!(
        object.requested_ranges(),
        vec![
            length - 8..length,
            length - footer_metadata_length - 8..length - 8,
        ]
    );
}

#[tokio::test]
async fn remote_reader_refuses_metadata_when_the_revision_changes_during_a_footer_fetch() {
    let fixture =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny.parquet");
    let object = Arc::new(FixtureRangeObject::switch_revision_after_first_request(
        std::fs::read(fixture).unwrap(),
    ));
    let mut reader = RemoteParquetReader::new(object.clone());

    let error = reader.get_metadata(None).await.unwrap_err();

    match error {
        ParquetError::External(error) => assert_eq!(
            error.downcast_ref::<SourceError>().unwrap().kind(),
            SourceErrorKind::Invalidated
        ),
        other => panic!("expected a redacted invalidation error, got {other:?}"),
    }
    assert_eq!(object.requested_ranges().len(), 1);
}

#[tokio::test]
async fn remote_reader_caches_metadata_per_page_index_policy() {
    let object = Arc::new(FixtureRangeObject::new(parquet_with_page_indexes()));
    let mut reader = RemoteParquetReader::new(object);
    let without_indexes = reader
        .get_metadata(Some(&ArrowReaderOptions::new()))
        .await
        .unwrap();
    let with_indexes = reader
        .get_metadata(Some(
            &ArrowReaderOptions::new().with_page_index_policy(PageIndexPolicy::Required),
        ))
        .await
        .unwrap();

    assert!(without_indexes.column_index().is_none());
    assert!(without_indexes.offset_index().is_none());
    assert!(with_indexes.column_index().is_some());
    assert!(with_indexes.offset_index().is_some());
}

#[tokio::test]
async fn remote_reader_uses_canonical_metadata_options_for_all_callers() {
    let fixture =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny.parquet");
    let object = Arc::new(FixtureRangeObject::new(std::fs::read(fixture).unwrap()));
    let mut reader = RemoteParquetReader::new(object);

    reader
        .get_metadata(Some(
            &ArrowReaderOptions::new().with_encoding_stats_policy(ParquetStatisticsPolicy::SkipAll),
        ))
        .await
        .unwrap();
    let metadata = reader
        .get_metadata(Some(&ArrowReaderOptions::new()))
        .await
        .unwrap();

    assert!(metadata
        .row_group(0)
        .column(0)
        .page_encoding_stats_mask()
        .is_some());
}
