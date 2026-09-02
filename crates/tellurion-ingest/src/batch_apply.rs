//! Shared chunked-apply core for `geopackage load` and `postgis load`
//! (`#114`): reads a local GeoJSON source — a plain `FeatureCollection`, or
//! an RFC 8142 GeoJSON Text Sequence — and applies it through a real
//! `WriteSink::apply_batch`, in bounded chunks, exactly the contract the
//! HTTP batch route (`tellurion-features::batch_handlers`) drives, just
//! in-process against a driver this process builds directly rather than
//! through a running server. `geopackage_load.rs`/`postgis_load.rs` each
//! open their own driver and hand it here; this module owns none of the
//! driver-specific setup.
//!
//! RFC 8142 sources are consumed one record at a time, so memory tracks one
//! feature plus one apply chunk rather than the size of the dataset. A plain
//! `FeatureCollection` remains the explicitly small, buffered input shape,
//! matching the HTTP route, with a hard 64 MiB cap. The CLI emits the same
//! per-item outcome and terminal summary shapes as HTTP.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;

use anyhow::Context;
use tellurion_core::{
    stage_batch_feature, validate_geojson_bbox, BatchItemOutcome, BatchOutcomeLine, BatchSummary,
    BatchTerminalCondition, CollectionDecl, Error as CoreError, GeoJsonSequenceDecoder,
    GeoJsonSequenceItem, Mutation, OutboxSource, Problem, RequestedCrs, Sequence, WriteSink,
    DEFAULT_BATCH_MAX_BYTES, GEO_JSON_RECORD_SEPARATOR,
};

pub struct BatchLoadSummary {
    pub applied: u64,
    pub refused: u64,
    pub unapplied: u64,
    pub elapsed: std::time::Duration,
    pub terminal: BatchTerminalCondition,
    pub batch_high_water: Option<u64>,
    pub outbox_high_water: Option<u64>,
}

impl BatchLoadSummary {
    pub fn features_per_second(&self) -> f64 {
        let attempted = (self.applied + self.refused) as f64;
        let seconds = self.elapsed.as_secs_f64();
        if seconds > 0.0 {
            attempted / seconds
        } else {
            0.0
        }
    }
}

/// Reads `source_path`, applies every feature it carries against `sink` in
/// chunks of `chunk_items`, and returns the aggregate outcome. `strict`
/// stops the whole load — no further chunks, no further items within the
/// chunk already in flight past the first refusal — the moment one feature
/// is refused, the same contract `WriteSink::apply_batch`'s own `strict`
/// parameter gives a single chunk, just carried across chunk boundaries
/// too.
pub async fn run(
    sink: &dyn WriteSink,
    outbox: &dyn OutboxSource,
    collection: &CollectionDecl,
    source_path: &Path,
    chunk_items: usize,
    strict: bool,
) -> anyhow::Result<BatchLoadSummary> {
    tracing::info!(
        source = %source_path.display(),
        chunk_items,
        strict,
        "starting batch load"
    );
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    run_with_writer(
        sink,
        outbox,
        collection,
        source_path,
        chunk_items,
        strict,
        &mut output,
    )
    .await
}

async fn run_with_writer(
    sink: &dyn WriteSink,
    outbox: &dyn OutboxSource,
    collection: &CollectionDecl,
    source_path: &Path,
    chunk_items: usize,
    strict: bool,
    output: &mut impl Write,
) -> anyhow::Result<BatchLoadSummary> {
    let mut source = FeatureSource::open(source_path)
        .with_context(|| format!("opening source dataset '{}'", source_path.display()))?;

    run_source_with_writer(
        sink,
        outbox,
        collection,
        &mut source,
        chunk_items,
        strict,
        output,
    )
    .await
}

async fn run_source_with_writer(
    sink: &dyn WriteSink,
    outbox: &dyn OutboxSource,
    collection: &CollectionDecl,
    source: &mut impl BatchFeatureSource,
    chunk_items: usize,
    strict: bool,
    output: &mut impl Write,
) -> anyhow::Result<BatchLoadSummary> {
    let start = std::time::Instant::now();
    let mut applied = 0u64;
    let mut refused = 0u64;
    let mut unapplied = 0u64;
    let mut aborted = false;
    let mut next_index = 0u64;
    let mut terminal = BatchTerminalCondition::Complete;
    let mut terminal_problem = None;
    let mut input_complete = true;
    let mut unknown_tail = false;
    let mut batch_high_water = None;

    'chunks: loop {
        let mut staged = Vec::with_capacity(chunk_items.max(1));
        let mut pending_lines = Vec::with_capacity(chunk_items.max(1));
        let mut source_exhausted = false;
        for _ in 0..chunk_items.max(1) {
            let item = match source.next_feature() {
                Ok(Some(item)) => item,
                Ok(None) => {
                    source_exhausted = true;
                    break;
                }
                Err(error) => {
                    terminal = BatchTerminalCondition::TransportError;
                    terminal_problem = Some(Problem::from_core_error(
                        &CoreError::Invalid(format!("source read failed: {error}")),
                        "batch",
                    ));
                    input_complete = false;
                    unknown_tail = true;
                    source_exhausted = true;
                    break;
                }
            };
            let index = next_index;
            next_index += 1;
            let item_stage = match item {
                SourceItem::Value(value) => stage_one(value, collection),
                SourceItem::Malformed(error) => Err((
                    None,
                    CoreError::Invalid(format!("source record is not valid JSON: {error}")),
                )),
            };
            match item_stage {
                Ok(mutation) => staged.push((index, mutation)),
                Err((id, err)) => {
                    refused += 1;
                    pending_lines.push((
                        index,
                        BatchOutcomeLine::Refused {
                            index,
                            id,
                            problem: Problem::from_core_error(&err, "batch"),
                        },
                    ));
                    if strict {
                        aborted = true;
                        break;
                    }
                }
            }
        }

        if !staged.is_empty() {
            let attempted = staged.len();
            let mutations = staged
                .iter()
                .map(|(_, mutation)| mutation.clone())
                .collect();
            let results = match sink
                .apply_batch(collection, mutations, RequestedCrs::Omitted, strict)
                .await
            {
                Ok(results) => results,
                Err(error) => {
                    for (index, mutation) in staged {
                        unapplied += 1;
                        pending_lines.push((
                            index,
                            BatchOutcomeLine::Unapplied {
                                index,
                                id: Some(mutation.feature_id),
                            },
                        ));
                    }
                    pending_lines.sort_by_key(|(index, _)| *index);
                    for (_, line) in pending_lines {
                        print_line(output, &line)?;
                    }
                    terminal = BatchTerminalCondition::ChunkError;
                    terminal_problem = Some(Problem::from_core_error(&error, "batch"));
                    input_complete = input_complete && source_exhausted;
                    unknown_tail = unknown_tail || !source_exhausted;
                    break 'chunks;
                }
            };
            for ((index, _), result) in staged.iter().zip(&results) {
                match &result.outcome {
                    BatchItemOutcome::Applied(Sequence(sequence)) => {
                        applied += 1;
                        batch_high_water = Some(
                            batch_high_water
                                .map_or(*sequence, |current: u64| current.max(*sequence)),
                        );
                        pending_lines.push((
                            *index,
                            BatchOutcomeLine::Applied {
                                index: *index,
                                id: result.feature_id.clone(),
                                sequence: *sequence,
                            },
                        ));
                    }
                    BatchItemOutcome::Refused(err) => {
                        refused += 1;
                        pending_lines.push((
                            *index,
                            BatchOutcomeLine::Refused {
                                index: *index,
                                id: Some(result.feature_id.clone()),
                                problem: Problem::from_core_error(err, "batch"),
                            },
                        ));
                        if strict {
                            aborted = true;
                        }
                    }
                }
            }
            if strict && results.len() < attempted {
                aborted = true;
                for (index, mutation) in staged.into_iter().skip(results.len()) {
                    unapplied += 1;
                    pending_lines.push((
                        index,
                        BatchOutcomeLine::Unapplied {
                            index,
                            id: Some(mutation.feature_id),
                        },
                    ));
                }
            }
        }
        pending_lines.sort_by_key(|(index, _)| *index);
        for (_, line) in pending_lines {
            print_line(output, &line)?;
        }

        if aborted {
            terminal = BatchTerminalCondition::StrictRefusal;
            loop {
                match source.next_feature() {
                    Ok(Some(item)) => {
                        let id = match item {
                            SourceItem::Value(value) => value.get("id").and_then(|id| match id {
                                serde_json::Value::String(id) => Some(id.clone()),
                                serde_json::Value::Number(id) => Some(id.to_string()),
                                _ => None,
                            }),
                            SourceItem::Malformed(_) => None,
                        };
                        print_line(
                            output,
                            &BatchOutcomeLine::Unapplied {
                                index: next_index,
                                id,
                            },
                        )?;
                        next_index += 1;
                        unapplied += 1;
                    }
                    Ok(None) => break,
                    Err(error) => {
                        terminal = BatchTerminalCondition::TransportError;
                        terminal_problem = Some(Problem::from_core_error(
                            &CoreError::Invalid(format!("source read failed: {error}")),
                            "batch",
                        ));
                        input_complete = false;
                        unknown_tail = true;
                        break;
                    }
                }
            }
            break 'chunks;
        }
        if source_exhausted {
            break 'chunks;
        }
    }

    let (outbox_high_water, watermark_problem) = match outbox.primary_high_water(collection).await {
        Ok(Sequence(sequence)) => (Some(sequence), None),
        Err(error) => (
            None,
            Some(Problem::from_core_error(&error, "batch-watermark")),
        ),
    };
    print_line(
        output,
        &BatchSummary {
            type_: "summary",
            applied,
            refused,
            unapplied,
            strict_aborted: aborted,
            terminal,
            input_complete,
            unknown_tail,
            terminal_problem,
            batch_high_water,
            outbox_high_water,
            watermark_problem,
        },
    )?;

    Ok(BatchLoadSummary {
        applied,
        refused,
        unapplied,
        elapsed: start.elapsed(),
        terminal,
        batch_high_water,
        outbox_high_water,
    })
}

fn print_line(output: &mut impl Write, value: &impl serde::Serialize) -> anyhow::Result<()> {
    serde_json::to_writer(&mut *output, value)?;
    output.write_all(b"\n")?;
    Ok(())
}

/// Checks one already-parsed JSON value's shape, extracts its caller-
/// supplied id, and builds the `Upsert` mutation the same rule the HTTP
/// batch route's own `stage_one` follows: batch ingest never mints a
/// server-assigned id, so a feature carrying none is refused before it ever
/// reaches `WriteSink`.
fn stage_one(
    value: serde_json::Value,
    collection: &CollectionDecl,
) -> std::result::Result<Mutation, (Option<String>, CoreError)> {
    stage_batch_feature(value, collection)
}

enum SourceItem {
    Value(serde_json::Value),
    Malformed(String),
}

trait BatchFeatureSource {
    fn next_feature(&mut self) -> anyhow::Result<Option<SourceItem>>;
}

enum FeatureSource {
    Sequence(GeoJsonSeqReader<BufReader<File>>),
    Buffered(std::vec::IntoIter<serde_json::Value>),
}

impl FeatureSource {
    fn open(path: &Path) -> anyhow::Result<Self> {
        Self::open_with_buffer_limit(path, DEFAULT_BATCH_MAX_BYTES)
    }

    fn open_with_buffer_limit(path: &Path, buffer_limit: u64) -> anyhow::Result<Self> {
        let file = File::open(path)?;
        let mut reader = BufReader::new(file);
        if reader.fill_buf()?.contains(&GEO_JSON_RECORD_SEPARATOR) {
            return Ok(Self::Sequence(GeoJsonSeqReader::new(reader)));
        }

        let mut bytes = Vec::new();
        reader.take(buffer_limit + 1).read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() as u64 <= buffer_limit,
            "FeatureCollection source exceeds the {}-byte buffered-input limit; use an RFC 8142 GeoJSON Text Sequence for larger loads",
            buffer_limit
        );
        Ok(Self::Buffered(
            parse_feature_collection(&bytes)?.into_iter(),
        ))
    }
}

impl BatchFeatureSource for FeatureSource {
    fn next_feature(&mut self) -> anyhow::Result<Option<SourceItem>> {
        match self {
            Self::Sequence(reader) => reader.next_feature(),
            Self::Buffered(features) => Ok(features.next().map(SourceItem::Value)),
        }
    }
}

struct GeoJsonSeqReader<R> {
    reader: R,
    decoder: GeoJsonSequenceDecoder,
    pending_chunk: Vec<u8>,
    pending_offset: usize,
    exhausted: bool,
}

impl<R: Read> GeoJsonSeqReader<R> {
    fn new(reader: R) -> Self {
        Self {
            reader,
            decoder: GeoJsonSequenceDecoder::new(DEFAULT_BATCH_MAX_BYTES as usize),
            pending_chunk: Vec::new(),
            pending_offset: 0,
            exhausted: false,
        }
    }

    fn next_feature(&mut self) -> anyhow::Result<Option<SourceItem>> {
        loop {
            if let Some(item) = self.decoder.next_item() {
                return Ok(Some(match item {
                    GeoJsonSequenceItem::Value(value) => SourceItem::Value(value),
                    GeoJsonSequenceItem::Malformed(error) => SourceItem::Malformed(error),
                }));
            }
            if self.pending_offset < self.pending_chunk.len() {
                self.pending_offset += self
                    .decoder
                    .push(&self.pending_chunk[self.pending_offset..]);
                continue;
            }
            if self.exhausted {
                return Ok(None);
            }
            let mut chunk = [0u8; 8 * 1024];
            let read = self.reader.read(&mut chunk)?;
            if read == 0 {
                self.decoder.finish();
                self.exhausted = true;
            } else {
                self.pending_chunk.clear();
                self.pending_chunk.extend_from_slice(&chunk[..read]);
                self.pending_offset = 0;
            }
        }
    }
}

fn parse_feature_collection(bytes: &[u8]) -> anyhow::Result<Vec<serde_json::Value>> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    anyhow::ensure!(
        value.get("type").and_then(serde_json::Value::as_str) == Some("FeatureCollection"),
        "source 'type' must be 'FeatureCollection'"
    );
    validate_geojson_bbox(&value)?;
    let features = value
        .get("features")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "source must be a GeoJSON FeatureCollection (a 'features' array) or an RFC \
                 8142 GeoJSON Text Sequence"
            )
        })?;
    Ok(features.clone())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::io::Write;

    use super::*;
    use tellurion_core::{BatchItemResult, Result as CoreResult, Sequence};

    struct RefusingSink;

    struct FailingSink;

    struct ReadThenFailSource {
        yielded: bool,
    }

    impl BatchFeatureSource for ReadThenFailSource {
        fn next_feature(&mut self) -> anyhow::Result<Option<SourceItem>> {
            if self.yielded {
                anyhow::bail!("synthetic source read failure");
            }
            self.yielded = true;
            Ok(Some(SourceItem::Value(serde_json::json!({
                "type": "Feature",
                "id": "1",
                "geometry": null,
                "properties": {}
            }))))
        }
    }

    #[async_trait::async_trait]
    impl WriteSink for RefusingSink {
        async fn apply(
            &self,
            _collection: &CollectionDecl,
            _mutation: Mutation,
        ) -> CoreResult<Sequence> {
            unreachable!("the batch loader must use apply_batch")
        }

        async fn apply_batch(
            &self,
            _collection: &CollectionDecl,
            mutations: Vec<Mutation>,
            _requested_crs: RequestedCrs,
            strict: bool,
        ) -> CoreResult<Vec<BatchItemResult>> {
            let mut results = Vec::new();
            for mutation in mutations {
                let refused = mutation.feature_id == "2";
                results.push(BatchItemResult {
                    feature_id: mutation.feature_id,
                    outcome: if refused {
                        BatchItemOutcome::Refused(CoreError::Invalid("dirty row".to_string()))
                    } else {
                        BatchItemOutcome::Applied(Sequence(1))
                    },
                });
                if refused && strict {
                    break;
                }
            }
            Ok(results)
        }
    }

    #[async_trait::async_trait]
    impl OutboxSource for RefusingSink {
        async fn read_after(
            &self,
            _collection: &CollectionDecl,
            _after: Sequence,
            _limit: u32,
        ) -> CoreResult<Vec<tellurion_core::Obligation>> {
            Ok(Vec::new())
        }

        async fn primary_high_water(&self, _collection: &CollectionDecl) -> CoreResult<Sequence> {
            Ok(Sequence(99))
        }
    }

    #[async_trait::async_trait]
    impl WriteSink for FailingSink {
        async fn apply(
            &self,
            _collection: &CollectionDecl,
            _mutation: Mutation,
        ) -> CoreResult<Sequence> {
            unreachable!("the batch loader must use apply_batch")
        }

        async fn apply_batch(
            &self,
            _collection: &CollectionDecl,
            _mutations: Vec<Mutation>,
            _requested_crs: RequestedCrs,
            _strict: bool,
        ) -> CoreResult<Vec<BatchItemResult>> {
            Err(CoreError::Config("synthetic chunk failure".to_string()))
        }
    }

    #[async_trait::async_trait]
    impl OutboxSource for FailingSink {
        async fn read_after(
            &self,
            _collection: &CollectionDecl,
            _after: Sequence,
            _limit: u32,
        ) -> CoreResult<Vec<tellurion_core::Obligation>> {
            Ok(Vec::new())
        }

        async fn primary_high_water(&self, _collection: &CollectionDecl) -> CoreResult<Sequence> {
            Ok(Sequence(99))
        }
    }

    fn parse_source(bytes: &[u8]) -> anyhow::Result<Vec<serde_json::Value>> {
        if bytes.contains(&GEO_JSON_RECORD_SEPARATOR) {
            let mut reader = GeoJsonSeqReader::new(Cursor::new(bytes));
            let mut features = Vec::new();
            while let Some(item) = reader.next_feature()? {
                match item {
                    SourceItem::Value(feature) => features.push(feature),
                    SourceItem::Malformed(error) => anyhow::bail!(error),
                }
            }
            Ok(features)
        } else {
            parse_feature_collection(bytes)
        }
    }

    #[test]
    fn parses_a_feature_collection() {
        let bytes = br#"{"type":"FeatureCollection","features":[{"id":"1"},{"id":"2"}]}"#;
        let features = parse_source(bytes).unwrap();
        assert_eq!(features.len(), 2);
    }

    #[test]
    fn parses_a_geo_json_text_sequence() {
        let mut bytes = Vec::new();
        bytes.push(GEO_JSON_RECORD_SEPARATOR);
        bytes.extend_from_slice(br#"{"id":"1"}"#);
        bytes.push(b'\n');
        bytes.push(GEO_JSON_RECORD_SEPARATOR);
        bytes.extend_from_slice(br#"{"id":"2"}"#);
        bytes.push(b'\n');
        let features = parse_source(&bytes).unwrap();
        assert_eq!(features.len(), 2);
    }

    #[test]
    fn accepts_a_self_delimiting_geo_json_object_without_a_line_feed() {
        let bytes = b"\x1e{\"id\":\"1\"}";
        assert_eq!(parse_source(bytes).unwrap().len(), 1);
    }

    #[test]
    fn cli_sequence_reader_recovers_after_junk_empty_records_and_malformed_json() {
        let bytes = b"junk\x1e\x1e{\"id\":\"1\"}\n\x1enot-json\n\x1e{\"id\":\"2\"}";
        let mut reader = GeoJsonSeqReader::new(Cursor::new(bytes));
        let mut items = Vec::new();
        while let Some(item) = reader.next_feature().unwrap() {
            items.push(item);
        }
        assert_eq!(items.len(), 4);
        assert!(matches!(items[0], SourceItem::Malformed(_)));
        assert!(matches!(&items[1], SourceItem::Value(value) if value["id"] == "1"));
        assert!(matches!(items[2], SourceItem::Malformed(_)));
        assert!(matches!(&items[3], SourceItem::Value(value) if value["id"] == "2"));
    }

    #[test]
    fn feature_collection_buffer_has_a_hard_limit() {
        let mut source = tempfile::NamedTempFile::new().unwrap();
        source
            .write_all(b"{\"type\":\"FeatureCollection\",\"features\":[]}")
            .unwrap();
        let error = FeatureSource::open_with_buffer_limit(source.path(), 8)
            .err()
            .expect("the buffered path must be bounded");
        assert!(error.to_string().contains("buffered-input limit"));
    }

    #[test]
    fn feature_collection_requires_its_type_and_valid_bbox() {
        assert!(parse_feature_collection(br#"{"type":"Feature","features":[]}"#).is_err());
        assert!(parse_feature_collection(
            br#"{"type":"FeatureCollection","bbox":[0,1,2],"features":[]}"#
        )
        .is_err());
    }

    #[test]
    fn stage_one_refuses_a_feature_with_no_id() {
        let value = serde_json::json!({
            "type": "Feature", "geometry": null, "properties": {}
        });
        assert!(stage_one(value, &collection()).is_err());
    }

    #[test]
    fn stage_one_accepts_a_string_id() {
        let value = serde_json::json!({
            "type": "Feature", "id": "42", "geometry": null, "properties": {}
        });
        let mutation = stage_one(value, &collection()).unwrap();
        assert_eq!(mutation.feature_id, "42");
    }

    #[tokio::test]
    async fn strict_cli_load_reports_the_unapplied_sequence_tail() {
        let mut source = tempfile::NamedTempFile::new().unwrap();
        for id in ["1", "2", "3"] {
            writeln!(
                source,
                "\x1e{{\"type\":\"Feature\",\"id\":\"{id}\",\"geometry\":null,\"properties\":{{}}}}"
            )
            .unwrap();
        }
        let collection: CollectionDecl =
            serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap();

        let summary = run(
            &RefusingSink,
            &RefusingSink,
            &collection,
            source.path(),
            3,
            true,
        )
        .await
        .unwrap();
        assert_eq!(summary.applied, 1);
        assert_eq!(summary.refused, 1);
        assert_eq!(summary.unapplied, 1);
        assert_eq!(summary.terminal, BatchTerminalCondition::StrictRefusal);
        assert_eq!(summary.batch_high_water, Some(1));
        assert_eq!(summary.outbox_high_water, Some(99));
    }

    #[tokio::test]
    async fn cli_machine_output_is_only_ndjson() {
        let mut source = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            source,
            "\x1e{{\"type\":\"Feature\",\"id\":\"1\",\"geometry\":null,\"properties\":{{}}}}"
        )
        .unwrap();
        let collection = collection();
        let mut output = Vec::new();
        run_with_writer(
            &RefusingSink,
            &RefusingSink,
            &collection,
            source.path(),
            1,
            false,
            &mut output,
        )
        .await
        .unwrap();

        let text = String::from_utf8(output).unwrap();
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("every stdout line is JSON"))
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["type"], "applied");
        assert_eq!(lines[1]["type"], "summary");
    }

    #[tokio::test]
    async fn chunk_failure_does_not_erase_an_earlier_source_read_failure() {
        let mut source = ReadThenFailSource { yielded: false };
        let mut output = Vec::new();
        run_source_with_writer(
            &FailingSink,
            &FailingSink,
            &collection(),
            &mut source,
            2,
            false,
            &mut output,
        )
        .await
        .unwrap();

        let summary: serde_json::Value = serde_json::from_slice(
            output
                .split(|byte| *byte == b'\n')
                .rfind(|line| !line.is_empty())
                .expect("summary line"),
        )
        .unwrap();
        assert_eq!(summary["terminal"], "chunk_error");
        assert_eq!(summary["input_complete"], false);
        assert_eq!(summary["unknown_tail"], true);
    }

    fn collection() -> CollectionDecl {
        serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap()
    }
}
