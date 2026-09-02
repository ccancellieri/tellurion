//! Settings for the batch ingest lane (`#114`): a whole-value-replaces
//! grouped key riding the platform -> tenant -> catalog -> collection
//! settings chain, resolved through [`crate::settings::resolve_field`] the
//! same way `SettingsDecl::colormap`/`::stac` already are — nearest level
//! wins, and a level that sets ANY field here replaces the WHOLE value, the
//! same convention [`crate::admission::AdmissionDecl`] follows. Unlike
//! `admission` (deliberately restricted to the platform/tenant levels
//! because admission control runs before routing ever resolves a catalog or
//! collection), `batch` rides the FULL four-level chain — the same one
//! `max_request_body_bytes` uses — because a batch request always already
//! has a resolved collection by the time its budget is checked.

use std::collections::VecDeque;

use bytes::{Buf, BytesMut};
use serde::{Deserialize, Serialize};

use crate::{CollectionDecl, Error, Mutation, MutationKind, Problem};

/// RFC 7464/8142 record separator.
pub const GEO_JSON_RECORD_SEPARATOR: u8 = 0x1e;

/// One recoverable parser result from a GeoJSON text sequence.
#[derive(Debug, PartialEq)]
pub enum GeoJsonSequenceItem {
    Value(serde_json::Value),
    Malformed(String),
}

/// Incremental RFC 7464 parser used by both HTTP and CLI batch ingest.
/// Empty records are ignored, malformed records recover at the next RS,
/// and memory is bounded to one record.
pub struct GeoJsonSequenceDecoder {
    buffer: BytesMut,
    pending: VecDeque<GeoJsonSequenceItem>,
    max_record_bytes: usize,
    started: bool,
    discarding: bool,
    finished: bool,
}

impl GeoJsonSequenceDecoder {
    pub fn new(max_record_bytes: usize) -> Self {
        Self {
            buffer: BytesMut::new(),
            pending: VecDeque::new(),
            max_record_bytes: max_record_bytes.max(1),
            started: false,
            discarding: false,
            finished: false,
        }
    }

    /// Consumes as much of `bytes` as is needed to materialize at most one
    /// item, returning the consumed prefix length. Callers retain and offer
    /// the unconsumed suffix after draining [`Self::next_item`].
    pub fn push(&mut self, bytes: &[u8]) -> usize {
        if self.finished || bytes.is_empty() {
            return 0;
        }
        if !self.pending.is_empty() {
            return 0;
        }

        let mut consumed = 0;
        while consumed < bytes.len() && self.pending.is_empty() {
            let remaining = &bytes[consumed..];
            let through_separator = remaining
                .iter()
                .position(|byte| *byte == GEO_JSON_RECORD_SEPARATOR)
                .map_or(remaining.len(), |position| position + 1);
            let capacity = self
                .max_record_bytes
                .saturating_add(1)
                .saturating_sub(self.buffer.len())
                .max(1);
            let take = through_separator.min(capacity);
            self.buffer.extend_from_slice(&remaining[..take]);
            consumed += take;
            self.process(false);
        }
        consumed
    }

    pub fn finish(&mut self) {
        if !self.finished {
            self.finished = true;
            if self.pending.is_empty() {
                self.process(true);
            }
        }
    }

    pub fn next_item(&mut self) -> Option<GeoJsonSequenceItem> {
        let item = self.pending.pop_front();
        if item.is_some() && self.pending.is_empty() {
            self.process(self.finished);
        }
        item
    }

    fn process(&mut self, eof: bool) {
        loop {
            if self.discarding {
                if let Some(rs) = self
                    .buffer
                    .iter()
                    .position(|byte| *byte == GEO_JSON_RECORD_SEPARATOR)
                {
                    self.buffer.advance(rs + 1);
                    self.started = true;
                    self.discarding = false;
                } else {
                    self.buffer.clear();
                    return;
                }
            }

            if !self.started {
                match self
                    .buffer
                    .iter()
                    .position(|byte| *byte == GEO_JSON_RECORD_SEPARATOR)
                {
                    Some(rs) => {
                        let prefix = self.buffer.split_to(rs);
                        self.buffer.advance(1);
                        self.started = true;
                        if !prefix.iter().all(u8::is_ascii_whitespace) {
                            self.pending.push_back(GeoJsonSequenceItem::Malformed(
                                "bytes before the first record separator are not a GeoJSON text sequence record"
                                    .to_string(),
                            ));
                            return;
                        }
                    }
                    None if eof => {
                        if !self.buffer.iter().all(u8::is_ascii_whitespace) {
                            self.pending.push_back(GeoJsonSequenceItem::Malformed(
                                "GeoJSON text sequence contains no record separator".to_string(),
                            ));
                        }
                        self.buffer.clear();
                        return;
                    }
                    None if self.buffer.len() > self.max_record_bytes => {
                        self.pending
                            .push_back(GeoJsonSequenceItem::Malformed(format!(
                                "GeoJSON text sequence prefix exceeds the {}-byte record limit",
                                self.max_record_bytes
                            )));
                        self.buffer.clear();
                        self.discarding = true;
                        return;
                    }
                    None => return,
                }
            }

            if let Some(rs) = self
                .buffer
                .iter()
                .position(|byte| *byte == GEO_JSON_RECORD_SEPARATOR)
            {
                let record = self.buffer.split_to(rs);
                self.buffer.advance(1);
                if self.parse_record(&record) {
                    return;
                }
                continue;
            }

            if eof {
                let record = self.buffer.split();
                self.parse_record(&record);
                return;
            }
            if self.buffer.len() > self.max_record_bytes {
                self.pending
                    .push_back(GeoJsonSequenceItem::Malformed(format!(
                        "GeoJSON text sequence record exceeds the {}-byte record limit",
                        self.max_record_bytes
                    )));
                self.buffer.clear();
                self.discarding = true;
            }
            return;
        }
    }

    fn parse_record(&mut self, record: &[u8]) -> bool {
        let record = trim_ascii_whitespace(record);
        if record.is_empty() {
            return false;
        }
        self.pending
            .push_back(match serde_json::from_slice(record) {
                Ok(value) => GeoJsonSequenceItem::Value(value),
                Err(error) => GeoJsonSequenceItem::Malformed(error.to_string()),
            });
        true
    }
}

fn trim_ascii_whitespace(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

/// Validates and stages one batch upsert identically for HTTP and CLI.
pub fn stage_batch_feature(
    value: serde_json::Value,
    decl: &CollectionDecl,
) -> std::result::Result<Mutation, (Option<String>, Error)> {
    if !value.is_object() {
        return Err((
            None,
            Error::Invalid("batch item must be a JSON object (a GeoJSON Feature)".to_string()),
        ));
    }
    if value.get("type").and_then(serde_json::Value::as_str) != Some("Feature") {
        return Err((
            None,
            Error::Invalid("batch item 'type' must be 'Feature'".to_string()),
        ));
    }
    validate_geojson_bbox(&value).map_err(|error| (None, error))?;
    match value.get("geometry") {
        None => {
            return Err((
                None,
                Error::Invalid("GeoJSON Feature is missing its 'geometry' member".to_string()),
            ))
        }
        Some(geometry) if !geometry.is_null() => {
            serde_json::from_value::<geojson::Geometry>(geometry.clone()).map_err(|error| {
                (
                    None,
                    Error::Invalid(format!("feature 'geometry' is not valid GeoJSON: {error}")),
                )
            })?;
        }
        Some(_) => {}
    }
    let properties = match value.get("properties") {
        None => {
            return Err((
                None,
                Error::Invalid("GeoJSON Feature is missing its 'properties' member".to_string()),
            ))
        }
        Some(serde_json::Value::Null) => serde_json::Map::new(),
        Some(serde_json::Value::Object(map)) => map.clone(),
        Some(_) => {
            return Err((
                None,
                Error::Invalid("feature 'properties' must be a JSON object".to_string()),
            ))
        }
    };
    let feature_id = match value.get("id") {
        Some(serde_json::Value::String(id)) => id.clone(),
        Some(serde_json::Value::Number(id)) => id.to_string(),
        _ => {
            return Err((
                None,
                Error::Invalid(
                    "feature is missing a top-level 'id' member; batch upsert requires a caller-supplied id per item"
                        .to_string(),
                ),
            ))
        }
    };
    if let Some(schema) = &decl.schema {
        if let Err(error) = schema.validate_feature_properties(&properties) {
            return Err((Some(feature_id), error));
        }
    }
    Ok(Mutation {
        feature_id,
        kind: MutationKind::Upsert(value),
    })
}

/// Validates RFC 7946's bbox numeric shape for a GeoJSON object.
pub fn validate_geojson_bbox(value: &serde_json::Value) -> std::result::Result<(), Error> {
    let Some(bbox) = value.get("bbox") else {
        return Ok(());
    };
    let Some(values) = bbox.as_array() else {
        return Err(Error::Invalid(
            "GeoJSON 'bbox' must be an array".to_string(),
        ));
    };
    if !matches!(values.len(), 4 | 6) {
        return Err(Error::Invalid(
            "GeoJSON 'bbox' must contain exactly 4 or 6 numbers".to_string(),
        ));
    }
    if values
        .iter()
        .any(|value| !value.as_f64().is_some_and(|number| number.is_finite()))
    {
        return Err(Error::Invalid(
            "GeoJSON 'bbox' values must be finite numbers".to_string(),
        ));
    }
    Ok(())
}

/// One item outcome shared by HTTP streaming responses and CLI loads.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BatchOutcomeLine {
    Applied {
        index: u64,
        id: String,
        sequence: u64,
    },
    Refused {
        index: u64,
        id: Option<String>,
        problem: Problem,
    },
    Unapplied {
        index: u64,
        id: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BatchTerminalCondition {
    Complete,
    StrictRefusal,
    ItemLimit,
    ByteLimit,
    TransportError,
    ChunkError,
}

/// Terminal line shared by HTTP and CLI. `batch_high_water` only covers
/// sequences returned by this invocation; `outbox_high_water` is the
/// separately queried current primary watermark.
#[derive(Debug, Clone, Serialize)]
pub struct BatchSummary {
    #[serde(rename = "type")]
    pub type_: &'static str,
    pub applied: u64,
    pub refused: u64,
    pub unapplied: u64,
    pub strict_aborted: bool,
    pub terminal: BatchTerminalCondition,
    pub input_complete: bool,
    pub unknown_tail: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_problem: Option<Problem>,
    pub batch_high_water: Option<u64>,
    pub outbox_high_water: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub watermark_problem: Option<Problem>,
}

/// Fallback cumulative byte budget for one batch ingest request when no
/// settings level declares one — 64 MiB, an order of magnitude above
/// `settings::DEFAULT_MAX_REQUEST_BODY_BYTES` (sized for a single feature):
/// generous enough for a real bulk load while still bounded, so a batch
/// request can never force the server to hold an unbounded amount of
/// inbound data, streamed or not.
pub const DEFAULT_BATCH_MAX_BYTES: u64 = 67_108_864;

/// Fallback item-count budget for one batch ingest request when no settings
/// level declares one — independent of `max_bytes`: a request built from
/// many tiny features could stay well under the byte budget while still
/// being an unreasonable number of rows to accept in one call.
pub const DEFAULT_BATCH_MAX_ITEMS: u64 = 1_000_000;

/// Fallback chunk size — how many items [`crate::outbox::WriteSink::
/// apply_batch`] is asked to commit in one backend transaction — when no
/// settings level declares one. Small enough that one chunk's transaction
/// never holds a lock for long, large enough that per-transaction overhead
/// stays amortized across a meaningful number of rows.
pub const DEFAULT_BATCH_CHUNK_ITEMS: u32 = 500;

/// Declared, whitelisted batch-ingest override (`#114`) — one grouped
/// settings key riding the same chain `max_request_body_bytes` does. A
/// level that sets ANY field here replaces the WHOLE value outright, the
/// same "whole value replaces, never merged field-by-field across levels"
/// convention `StacConf`/`ColormapConf`/`AdmissionDecl` all follow; a field
/// the winning declaration itself leaves unset falls back to this module's
/// own default, never to a different level's value for that one field.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BatchDecl {
    /// Cumulative request-body byte budget for one batch ingest call.
    /// `None`/absent falls back to [`DEFAULT_BATCH_MAX_BYTES`]. Checked
    /// against the streamed length as a `geo+json-seq` body is consumed
    /// incrementally, the same "never buffer-then-measure" discipline
    /// `max_request_body_bytes` already follows for a single-item write.
    pub max_bytes: Option<u64>,
    /// Item-count budget for one batch ingest call. `None`/absent falls
    /// back to [`DEFAULT_BATCH_MAX_ITEMS`].
    pub max_items: Option<u64>,
    /// How many items `WriteSink::apply_batch` commits per backend
    /// transaction. `None`/absent falls back to
    /// [`DEFAULT_BATCH_CHUNK_ITEMS`]. Rejected at `AppConfig::validate` time
    /// if declared as `0` — a chunk that commits nothing is a config
    /// mistake, not a meaningful "opt out."
    pub chunk_items: Option<u32>,
}

/// The materialized result of resolving [`BatchDecl`] for one collection:
/// concrete values, ready to enforce without re-checking `Option`s again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BatchConfig {
    pub max_bytes: u64,
    pub max_items: u64,
    pub chunk_items: u32,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_BATCH_MAX_BYTES,
            max_items: DEFAULT_BATCH_MAX_ITEMS,
            chunk_items: DEFAULT_BATCH_CHUNK_ITEMS,
        }
    }
}

impl BatchDecl {
    /// Applies this module's own defaults to whichever fields the winning
    /// declaration left unset — see this type's own doc for why an unset
    /// field never falls through to a different settings level instead.
    pub(crate) fn resolve(&self) -> BatchConfig {
        BatchConfig {
            max_bytes: self.max_bytes.unwrap_or(DEFAULT_BATCH_MAX_BYTES),
            max_items: self.max_items.unwrap_or(DEFAULT_BATCH_MAX_ITEMS),
            chunk_items: self.chunk_items.unwrap_or(DEFAULT_BATCH_CHUNK_ITEMS),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(chunks: &[&[u8]]) -> Vec<GeoJsonSequenceItem> {
        let mut decoder = GeoJsonSequenceDecoder::new(1_024);
        let mut items = Vec::new();
        for chunk in chunks {
            let mut offset = 0;
            while offset < chunk.len() {
                offset += decoder.push(&chunk[offset..]);
                while let Some(item) = decoder.next_item() {
                    items.push(item);
                }
            }
        }
        decoder.finish();
        while let Some(item) = decoder.next_item() {
            items.push(item);
        }
        items
    }

    #[test]
    fn sequence_decoder_ignores_consecutive_separators_and_reassembles_chunks() {
        let items = decode(&[
            b"\x1e\x1e{\n  \"type\": \"Feature\",\n  \"id\": \"1\"",
            b",\n  \"geometry\": null,\n  \"properties\": {}\n}\n\x1e\x1e",
        ]);
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0], GeoJsonSequenceItem::Value(value) if value["id"] == "1"));
    }

    #[test]
    fn sequence_decoder_reports_junk_and_malformed_middle_then_recovers() {
        let items = decode(&[
            b"junk-prefix\x1e{\"type\":\"Feature\",\"id\":\"1\"}\n\x1enot-json\n\x1e",
            b"{\"type\":\"Feature\",\"id\":\"2\"}",
        ]);
        assert_eq!(items.len(), 4);
        assert!(matches!(items[0], GeoJsonSequenceItem::Malformed(_)));
        assert!(matches!(&items[1], GeoJsonSequenceItem::Value(value) if value["id"] == "1"));
        assert!(matches!(items[2], GeoJsonSequenceItem::Malformed(_)));
        assert!(matches!(&items[3], GeoJsonSequenceItem::Value(value) if value["id"] == "2"));
    }

    #[test]
    fn sequence_decoder_materializes_only_one_item_from_a_large_single_frame() {
        let mut frame = Vec::new();
        for id in 0..1_000 {
            frame.extend_from_slice(
                format!("\x1e{{\"type\":\"Feature\",\"id\":{id}}}\n").as_bytes(),
            );
        }
        let mut decoder = GeoJsonSequenceDecoder::new(frame.len());
        let consumed = decoder.push(&frame);
        assert!(consumed < frame.len());
        assert_eq!(decoder.pending.len(), 1);

        let mut count = 0;
        let mut offset = consumed;
        while offset < frame.len() {
            if decoder.next_item().is_some() {
                count += 1;
            }
            offset += decoder.push(&frame[offset..]);
        }
        decoder.finish();
        while decoder.next_item().is_some() {
            count += 1;
        }
        assert_eq!(count, 1_000);
    }

    fn collection() -> crate::CollectionDecl {
        serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap()
    }

    #[test]
    fn staging_requires_a_feature_and_valid_geojson_geometry() {
        let decl = collection();
        let wrong_type = serde_json::json!({
            "type": "GeometryCollection", "id": "1", "geometry": null, "properties": {}
        });
        assert!(stage_batch_feature(wrong_type, &decl).is_err());

        let missing_geometry = serde_json::json!({
            "type": "Feature", "id": "1", "properties": {}
        });
        assert!(stage_batch_feature(missing_geometry, &decl).is_err());

        let missing_properties = serde_json::json!({
            "type": "Feature", "id": "1", "geometry": null
        });
        assert!(stage_batch_feature(missing_properties, &decl).is_err());

        let invalid_geometry = serde_json::json!({
            "type": "Feature", "id": "1",
            "geometry": {"type": "Point", "coordinates": [1]}, "properties": {}
        });
        assert!(stage_batch_feature(invalid_geometry, &decl).is_err());

        let valid = serde_json::json!({
            "type": "Feature", "id": "1",
            "geometry": {"type": "Point", "coordinates": [1, 2]}, "properties": {}
        });
        assert!(stage_batch_feature(valid, &decl).is_ok());

        let invalid_bbox = serde_json::json!({
            "type": "Feature", "id": "1", "bbox": [0, 1, 2],
            "geometry": null, "properties": {}
        });
        assert!(stage_batch_feature(invalid_bbox, &decl).is_err());

        let non_numeric_bbox = serde_json::json!({
            "type": "Feature", "id": "1", "bbox": [0, 1, 2, "east"],
            "geometry": null, "properties": {}
        });
        assert!(stage_batch_feature(non_numeric_bbox, &decl).is_err());

        let valid_3d_bbox = serde_json::json!({
            "type": "Feature", "id": "1", "bbox": [0, 1, 2, 3, 4, 5],
            "geometry": null, "properties": {}
        });
        assert!(stage_batch_feature(valid_3d_bbox, &decl).is_ok());
    }

    #[test]
    fn resolve_applies_every_default_when_nothing_is_declared() {
        assert_eq!(BatchDecl::default().resolve(), BatchConfig::default());
    }

    #[test]
    fn resolve_keeps_an_explicit_value_and_still_defaults_the_rest() {
        let decl = BatchDecl {
            max_bytes: Some(1_000),
            max_items: None,
            chunk_items: None,
        };
        let resolved = decl.resolve();
        assert_eq!(resolved.max_bytes, 1_000);
        assert_eq!(resolved.max_items, DEFAULT_BATCH_MAX_ITEMS);
        assert_eq!(resolved.chunk_items, DEFAULT_BATCH_CHUNK_ITEMS);
    }
}
