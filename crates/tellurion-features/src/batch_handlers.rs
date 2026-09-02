//! `POST /collections/{cid}/items/batch` (`#114`): a pacing layer over the
//! same write lane `write_handlers.rs` already exposes — never a second
//! write path with its own semantics. Every mutation this route accepts is
//! a caller-supplied-id upsert (`WriteSink::apply_batch`'s own doc): there
//! is no server-assigned create here (a batch has no per-item URL to mint
//! an id under the way `POST /items` does) and no delete (GeoJSON itself
//! has no delete representation to accept one through), so every batch item
//! is a GeoJSON Feature carrying its own top-level `id` member.
//!
//! ## Route shape and prior art
//!
//! OGC API Features — Part 4 (the requirements class `write_handlers.rs`
//! implements) is deliberately single-resource: one `PUT`/`POST`/`DELETE`
//! per feature, with no bulk-transaction requirement class at all as of
//! this writing. Rather than inventing a shape from nothing, this endpoint
//! follows the two closest prior arts for "many operations, one request,
//! partial success":
//!
//! - **WFS-T's `Transaction` request** (OGC 09-025r2 section 15): a single
//!   request batching many `Insert`/`Update`/`Delete` operations, answered
//!   by one `TransactionResponse` naming which operations succeeded — the
//!   "many ops in, one bounded call, named per-operation outcome" shape
//!   this endpoint borrows, without WFS-T's XML envelope or its op-level
//!   XML elements (this endpoint's wire format is plain GeoJSON throughout).
//! - **Document-oriented bulk APIs** (Elasticsearch's/OpenSearch's `_bulk`
//!   endpoint is the most widely deployed example): a streamed sequence of
//!   per-document operations in, a streamed sequence of per-document
//!   outcomes out, `id` and any per-item error named individually rather
//!   than one all-or-nothing verdict for the whole call. This endpoint's
//!   per-item outcome stream (`BatchOutcomeLine`, below) follows that
//!   shape directly: one JSON object per line (`application/x-ndjson`, the
//!   same de facto newline-delimited-JSON convention `_bulk`'s own response
//!   format popularized — not an IANA-registered type, but not one this
//!   endpoint invented either), each line self-contained and independently
//!   parseable as the client reads them.
//!
//! Neither prior art's request wire format applies unchanged, since
//! neither is GeoJSON: the request side instead uses [RFC 8142](
//! https://www.rfc-editor.org/rfc/rfc8142) "GeoJSON Text Sequences"
//! (`application/geo+json-seq`, registered — the exact string this module
//! uses, never a made-up media type) for the streamed-input case, since
//! that RFC exists precisely to let a GeoJSON `Feature` stream arrive one
//! record at a time without ever wrapping the whole sequence in a JSON
//! array (which would force buffering it whole to find the closing `]`).
//! A small payload may instead send one ordinary `application/geo+json`
//! `FeatureCollection` body — buffered, but still capped by the same batch
//! byte budget (`GeoJsonSeqReader`'s own doc explains why only the
//! streamed path needs incremental enforcement).
//!
//! ## Budget, chunking, and the outbox high-water
//!
//! The batch byte/item budget (`settings.batch`, `tellurion_core::batch`)
//! resolves through the identical platform -> tenant -> catalog ->
//! collection settings chain `settings.max_request_body_bytes` uses
//! (`tellurion_core::settings`) — see that module's own doc. Items apply in
//! bounded chunks of `settings.batch.chunk_items`, each chunk committing
//! through [`tellurion_core::WriteSink::apply_batch`] in ONE backend
//! transaction. The completion summary's `batch_high_water` is the maximum
//! sequence returned by this request, while `outbox_high_water` is a fresh
//! [`tellurion_core::OutboxSource::primary_high_water`] read taken once
//! after the last chunk commits — the true "everything committed so far"
//! watermark, including concurrent writes outside this batch. A failed
//! watermark read is reported explicitly rather than collapsed into an
//! unexplained null.
//!
//! ## Strict mode
//!
//! `?strict=true` stops attempting further items the instant one is
//! refused (`stage_chunk`'s own doc): every item already staged before the
//! first refusal (in original request order) still gets its own outcome
//! line, the first refusal itself is reported, and the unread tail is
//! consumed only within the declared budgets to report it as `unapplied`.
//! The terminal summary distinguishes a complete strict stop from a budget,
//! transport, or transaction stop and says when the remaining tail is
//! unknown.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use bytes::{Buf, Bytes};
use futures::StreamExt;
use serde::Serialize;

use tellurion_core::problem::Problem;
use tellurion_core::{
    stage_batch_feature, validate_geojson_bbox, AppContext, BatchConfig, BatchOutcomeLine,
    BatchSummary, BatchTerminalCondition, CollectionDecl, ContextState, Error as CoreError,
    GeoJsonSequenceDecoder, GeoJsonSequenceItem, Mutation, RequestedCrs, Sequence, WriteSink,
};

use crate::problem::ApiError;
use crate::write_handlers;

/// RFC 8142 — GeoJSON Text Sequences: the streamed-input media type.
const GEO_JSON_SEQ_MEDIA_TYPE: &str = "application/geo+json-seq";
/// The single-item write lane's own GeoJSON media type, reused here for the
/// small-payload `FeatureCollection` body — a `FeatureCollection` is still
/// plain GeoJSON, no new type to register.
const GEO_JSON_MEDIA_TYPE: &str = "application/geo+json";
/// The response's own media type — see this module's own doc for why this
/// is an established convention, not an invented one.
const NDJSON_MEDIA_TYPE: &str = "application/x-ndjson";
// RFC 8142's encoder format is `RS json-text LF`; the shared RFC 7464
// parser also recovers malformed records at the next RS and accepts a
// self-delimiting GeoJSON object at EOF.

/// `POST /collections/{cid}/items/batch`. Resolves and authorizes exactly
/// like `write_handlers::create_item` up through `Router::resolve_write` and
/// the `Content-Crs` checks — this is still the write lane, just paced
/// differently — then commits to a streamed `200` response: from that point
/// on, any failure (a budget overrun, a malformed record, strict mode
/// aborting) is reported IN BAND as a line in the response body, never as a
/// change of HTTP status, since the status and headers are already on the
/// wire by the time this handler could know about it.
pub async fn batch_items(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Response, ApiError> {
    let (tenant_id, catalog_id) = write_handlers::resolve_tenant_catalog(&ctx, &params).await?;
    let cid = write_handlers::require_param(&params, "cid")?;
    let state = ctx.current();
    let collection_id = state.resolver.resolve_collection(&catalog_id, &cid).await?;
    write_handlers::authorize_write_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &tenant_id,
        &catalog_id,
        &collection_id,
    )
    .await?;

    let (decl, sink) = state
        .router
        .resolve_write(&tenant_id, &catalog_id, &collection_id)
        .await?;
    let resolved_crs = write_handlers::resolve_content_crs(&headers, decl.srid)?;
    write_handlers::refuse_unreprojectable_content_crs(
        resolved_crs,
        decl.srid,
        sink.crs_capable(),
        &cid,
    )?;

    let batch_config = state
        .router
        .effective_settings(&collection_id)
        .map(|settings| settings.batch)
        .unwrap_or_default();
    let strict = query.get("strict").is_some_and(|v| v == "true");

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    let source = match media_type.as_str() {
        GEO_JSON_SEQ_MEDIA_TYPE => {
            BatchSource::Streamed(GeoJsonSeqReader::new(body, batch_config.max_bytes))
        }
        GEO_JSON_MEDIA_TYPE => {
            let bytes = write_handlers::read_capped_body(body, batch_config.max_bytes).await?;
            let features = parse_feature_collection(&bytes)?;
            if features.len() as u64 > batch_config.max_items {
                return Err(ApiError::from(CoreError::Invalid(format!(
                    "batch carries {} features, over this collection's {}-item batch budget",
                    features.len(),
                    batch_config.max_items
                ))));
            }
            BatchSource::Buffered(features.into_iter())
        }
        other => {
            return Err(ApiError::from(CoreError::UnsupportedMediaType(
                other.to_string(),
            )))
        }
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel::<std::result::Result<Bytes, std::io::Error>>(4);
    tokio::spawn(run_batch(
        state,
        tenant_id,
        catalog_id,
        collection_id,
        decl,
        sink,
        resolved_crs,
        batch_config,
        strict,
        source,
        tx,
    ));

    let stream = futures::stream::poll_fn(move |cx| rx.poll_recv(cx));
    let mut response = Response::new(axum::body::Body::from_stream(stream));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(NDJSON_MEDIA_TYPE),
    );
    Ok(response)
}

/// A `FeatureCollection`'s `features` array, parsed whole — the
/// small-payload path's only shape check beyond what `stage_one` re-checks
/// per feature anyway (kept minimal deliberately: a malformed individual
/// feature inside an otherwise well-formed collection still gets its own
/// per-item refusal from `stage_one`, not a whole-request `400`).
fn parse_feature_collection(bytes: &[u8]) -> Result<Vec<serde_json::Value>, ApiError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| CoreError::Invalid(format!("request body is not valid JSON: {e}")))?;
    if value.get("type").and_then(serde_json::Value::as_str) != Some("FeatureCollection") {
        return Err(ApiError::from(CoreError::Invalid(
            "request body 'type' must be 'FeatureCollection'".to_string(),
        )));
    }
    validate_geojson_bbox(&value).map_err(ApiError::from)?;
    let features = value
        .get("features")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            CoreError::Invalid(
                "request body must be a GeoJSON FeatureCollection (a 'features' array)".to_string(),
            )
        })?;
    Ok(features.clone())
}

/// One raw item pulled off a [`BatchSource`], before `stage_one` has
/// checked its shape at all.
enum NextItem {
    /// The source has nothing left at all.
    Eof,
    /// The streamed source's cumulative byte or item budget ran out —
    /// terminal: nothing further is ever read.
    BudgetExceeded,
    /// A genuine read failure on the underlying request body — terminal,
    /// same as `BudgetExceeded`.
    TransportError,
    /// One record, already valid JSON.
    Value(serde_json::Value),
    /// One record whose raw bytes were not valid JSON at all — carries no
    /// id (parsing never got far enough to find one).
    Malformed(String),
}

/// Either input shape this route accepts, unified behind one `next` method
/// so the chunking loop in `run_batch` never needs to know which one it's
/// reading from.
enum BatchSource {
    Streamed(GeoJsonSeqReader),
    Buffered(std::vec::IntoIter<serde_json::Value>),
}

impl BatchSource {
    async fn next(&mut self) -> NextItem {
        match self {
            BatchSource::Buffered(iter) => match iter.next() {
                Some(value) => NextItem::Value(value),
                None => NextItem::Eof,
            },
            BatchSource::Streamed(reader) => reader.next_item().await,
        }
    }
}

/// Incrementally splits an `application/geo+json-seq` (RFC 8142) request
/// body into one `serde_json::Value` per record, pulling more bytes from
/// the underlying connection only as needed — the actual "never buffer the
/// whole batch" mechanism this route's own module doc promises. `buffer`
/// only ever holds, at most, one in-flight (not-yet-terminated) record's
/// worth of bytes plus whatever partial chunk the transport just handed
/// back: every time a complete record is found, the buffer drains up to
/// (but not including) the RS that starts the next one, so memory use
/// tracks one record's size, never the request's total size.
struct GeoJsonSeqReader {
    stream: axum::body::BodyDataStream,
    decoder: GeoJsonSequenceDecoder,
    pending_chunk: Bytes,
    total_bytes: u64,
    max_bytes: u64,
    exhausted: bool,
}

impl GeoJsonSeqReader {
    fn new(body: axum::body::Body, max_bytes: u64) -> Self {
        Self {
            stream: body.into_data_stream(),
            decoder: GeoJsonSequenceDecoder::new(usize::try_from(max_bytes).unwrap_or(usize::MAX)),
            pending_chunk: Bytes::new(),
            total_bytes: 0,
            max_bytes,
            exhausted: false,
        }
    }

    async fn next_item(&mut self) -> NextItem {
        loop {
            if let Some(item) = self.decoder.next_item() {
                return match item {
                    GeoJsonSequenceItem::Value(value) => NextItem::Value(value),
                    GeoJsonSequenceItem::Malformed(reason) => NextItem::Malformed(reason),
                };
            }
            if !self.pending_chunk.is_empty() {
                let consumed = self.decoder.push(&self.pending_chunk);
                self.pending_chunk.advance(consumed);
                continue;
            }
            if self.exhausted {
                return NextItem::Eof;
            }
            match self.stream.next().await {
                Some(Ok(chunk)) => {
                    self.total_bytes += chunk.len() as u64;
                    if self.total_bytes > self.max_bytes {
                        self.exhausted = true;
                        return NextItem::BudgetExceeded;
                    }
                    self.pending_chunk = chunk;
                }
                Some(Err(_)) => {
                    self.exhausted = true;
                    return NextItem::TransportError;
                }
                None => {
                    self.decoder.finish();
                    self.exhausted = true;
                }
            }
        }
    }
}

/// One item's identity plus what to do with it, as `stage_chunk` works
/// through one chunk's worth of raw input — the pre-flight half of what
/// `write_handlers::put_item` does inline for a single feature (parse,
/// extract the id, schema-validate), just producing a value this module can
/// batch up rather than acting on immediately.
enum StagedKind {
    Mutation(Mutation),
    Refused(CoreError),
}

struct StagedEntry {
    index: u64,
    feature_id: Option<String>,
    kind: StagedKind,
}

/// Checks one already-parsed JSON value's shape, extracts its caller-
/// supplied id, and schema-validates it exactly like `write_handlers::
/// put_item` does before ever calling `WriteSink::apply` — the same named
/// refusals (`Error::Invalid` for a malformed shape or a missing id,
/// whatever `SchemaDecl::validate_feature_properties` itself gives for a
/// declared-schema collection) a single `PUT` would give the identical bad
/// input, since this is the same write lane paced differently, never a
/// second validation vocabulary of its own.
fn stage_one(
    value: serde_json::Value,
    decl: &CollectionDecl,
) -> std::result::Result<Mutation, (Option<String>, CoreError)> {
    stage_batch_feature(value, decl)
}

/// What staging one chunk found: up to `chunk_items` entries in original
/// order, plus whether the input is now exhausted (end of stream/list,
/// budget exceeded, or a transport error — all treated the same: stop
/// reading).
struct ChunkStaging {
    entries: Vec<StagedEntry>,
    source_exhausted: bool,
    termination: Option<SourceTermination>,
}

#[derive(Clone, Copy)]
enum SourceTermination {
    ByteLimit,
    TransportError,
}

/// Pulls up to `chunk_items` items off `source`, staging each one
/// (`stage_one`) as it arrives, in original request order. In `strict`
/// mode, staging itself stops the instant one entry is refused — the
/// caller never even reads further input for this chunk, let alone sends
/// anything past that point to `WriteSink::apply_batch` — so "stops
/// attempting further mutations the moment one is refused" holds at the
/// granularity of a single item, not merely a whole chunk.
async fn stage_chunk(
    source: &mut BatchSource,
    decl: &CollectionDecl,
    chunk_items: u32,
    next_index: &mut u64,
    strict: bool,
) -> ChunkStaging {
    let mut entries = Vec::new();
    let mut source_exhausted = false;
    let mut termination = None;

    for _ in 0..chunk_items {
        let item = source.next().await;
        let (feature_id, kind) = match item {
            NextItem::Eof => {
                source_exhausted = true;
                break;
            }
            NextItem::BudgetExceeded => {
                source_exhausted = true;
                termination = Some(SourceTermination::ByteLimit);
                break;
            }
            NextItem::TransportError => {
                source_exhausted = true;
                termination = Some(SourceTermination::TransportError);
                break;
            }
            NextItem::Malformed(reason) => (
                None,
                StagedKind::Refused(CoreError::Invalid(format!(
                    "request body is not valid JSON: {reason}"
                ))),
            ),
            NextItem::Value(value) => match stage_one(value, decl) {
                Ok(mutation) => (
                    Some(mutation.feature_id.clone()),
                    StagedKind::Mutation(mutation),
                ),
                Err((feature_id, err)) => (feature_id, StagedKind::Refused(err)),
            },
        };
        let refused = matches!(kind, StagedKind::Refused(_));
        let index = *next_index;
        *next_index += 1;
        entries.push(StagedEntry {
            index,
            feature_id,
            kind,
        });
        if refused && strict {
            break;
        }
    }

    ChunkStaging {
        entries,
        source_exhausted,
        termination,
    }
}

fn encode_line(value: &impl Serialize) -> Bytes {
    let mut bytes = serde_json::to_vec(value).unwrap_or_default();
    bytes.push(b'\n');
    Bytes::from(bytes)
}

/// Drives one whole batch request to completion, chunk by chunk, sending
/// each output line down `tx` as soon as it's known — this is the actual
/// body of the streamed response `batch_items` already returned by the time
/// this runs. A send failure (the client went away) simply stops the loop
/// early; there is nobody left to report anything to.
#[allow(clippy::too_many_arguments)]
async fn run_batch(
    state: Arc<ContextState>,
    tenant_id: String,
    catalog_id: String,
    collection_id: String,
    decl: CollectionDecl,
    sink: Arc<dyn WriteSink>,
    requested_crs: RequestedCrs,
    batch_config: BatchConfig,
    strict: bool,
    mut source: BatchSource,
    tx: tokio::sync::mpsc::Sender<std::result::Result<Bytes, std::io::Error>>,
) {
    let mut applied = 0u64;
    let mut refused = 0u64;
    let mut unapplied = 0u64;
    let mut strict_aborted = false;
    let mut next_index = 0u64;
    let mut items_seen = 0u64;
    let mut terminal = BatchTerminalCondition::Complete;
    let mut input_complete = true;
    let mut unknown_tail = false;
    let mut terminal_problem = None;
    let mut batch_high_water = None;

    'outer: loop {
        if items_seen >= batch_config.max_items {
            let overflow = source.next().await;
            match overflow {
                NextItem::Eof => {}
                NextItem::BudgetExceeded => {
                    terminal = BatchTerminalCondition::ByteLimit;
                    input_complete = false;
                    unknown_tail = true;
                    terminal_problem = Some(Problem::from_core_error(
                        &CoreError::Invalid(
                            "request body exceeds this collection's batch byte budget".to_string(),
                        ),
                        "batch",
                    ));
                }
                NextItem::TransportError => {
                    terminal = BatchTerminalCondition::TransportError;
                    input_complete = false;
                    unknown_tail = true;
                    terminal_problem = Some(Problem::from_core_error(
                        &CoreError::Invalid(
                            "request body ended with a transport error".to_string(),
                        ),
                        "batch",
                    ));
                }
                overflow => {
                    let feature_id = match overflow {
                        NextItem::Value(value) => value.get("id").and_then(|id| match id {
                            serde_json::Value::String(id) => Some(id.clone()),
                            serde_json::Value::Number(id) => Some(id.to_string()),
                            _ => None,
                        }),
                        _ => None,
                    };
                    unapplied += 1;
                    let line = BatchOutcomeLine::Unapplied {
                        index: next_index,
                        id: feature_id,
                    };
                    if tx.send(Ok(encode_line(&line))).await.is_err() {
                        return;
                    }
                    terminal = BatchTerminalCondition::ItemLimit;
                    input_complete = false;
                    unknown_tail = true;
                    terminal_problem = Some(Problem::from_core_error(
                        &CoreError::Invalid(format!(
                            "batch carries more than this collection's {}-item batch budget",
                            batch_config.max_items
                        )),
                        "batch",
                    ));
                }
            }
            break;
        }
        let remaining_budget = batch_config.max_items - items_seen;
        let chunk_items = batch_config
            .chunk_items
            .min(remaining_budget.min(u32::MAX as u64) as u32);
        if chunk_items == 0 {
            break;
        }

        let staging = stage_chunk(&mut source, &decl, chunk_items, &mut next_index, strict).await;
        items_seen += staging.entries.len() as u64;
        let source_exhausted = staging.source_exhausted;
        if let Some(source_termination) = staging.termination {
            input_complete = false;
            unknown_tail = true;
            let (condition, detail) = match source_termination {
                SourceTermination::ByteLimit => (
                    BatchTerminalCondition::ByteLimit,
                    "request body exceeds this collection's batch byte budget",
                ),
                SourceTermination::TransportError => (
                    BatchTerminalCondition::TransportError,
                    "request body ended with a transport error",
                ),
            };
            terminal = condition;
            terminal_problem = Some(Problem::from_core_error(
                &CoreError::Invalid(detail.to_string()),
                "batch",
            ));
        }

        let mut mutations = Vec::new();
        let mut mutation_positions = Vec::new();
        for (position, entry) in staging.entries.iter().enumerate() {
            if let StagedKind::Mutation(mutation) = &entry.kind {
                mutations.push(mutation.clone());
                mutation_positions.push(position);
            }
        }

        let apply_results = if mutations.is_empty() {
            Ok(Vec::new())
        } else {
            sink.apply_batch(&decl, mutations, requested_crs, strict)
                .await
        };

        let outcomes_by_position: HashMap<usize, tellurion_core::BatchItemOutcome> =
            match apply_results {
                Ok(results) => mutation_positions
                    .into_iter()
                    .zip(results)
                    .map(|(position, result)| (position, result.outcome))
                    .collect(),
                Err(err) => {
                    let transaction_problem = Problem::from_core_error(&err, "batch");
                    for entry in staging.entries {
                        let line = match entry.kind {
                            StagedKind::Mutation(_) => {
                                unapplied += 1;
                                BatchOutcomeLine::Unapplied {
                                    index: entry.index,
                                    id: entry.feature_id,
                                }
                            }
                            StagedKind::Refused(err) => {
                                refused += 1;
                                BatchOutcomeLine::Refused {
                                    index: entry.index,
                                    id: entry.feature_id,
                                    problem: Problem::from_core_error(&err, "batch"),
                                }
                            }
                        };
                        if tx.send(Ok(encode_line(&line))).await.is_err() {
                            return;
                        }
                    }
                    terminal = BatchTerminalCondition::ChunkError;
                    terminal_problem = Some(transaction_problem);
                    input_complete = input_complete && source_exhausted;
                    unknown_tail = unknown_tail || !source_exhausted;
                    break 'outer;
                }
            };

        let mut chunk_strict_stop = false;
        for (position, entry) in staging.entries.into_iter().enumerate() {
            if chunk_strict_stop {
                unapplied += 1;
                let line = encode_line(&BatchOutcomeLine::Unapplied {
                    index: entry.index,
                    id: entry.feature_id,
                });
                if tx.send(Ok(line)).await.is_err() {
                    return;
                }
                continue;
            }
            let line = match entry.kind {
                StagedKind::Refused(err) => {
                    refused += 1;
                    if strict {
                        strict_aborted = true;
                        chunk_strict_stop = true;
                    }
                    BatchOutcomeLine::Refused {
                        index: entry.index,
                        id: entry.feature_id,
                        problem: Problem::from_core_error(&err, "batch"),
                    }
                }
                StagedKind::Mutation(_) => match outcomes_by_position.get(&position) {
                    Some(tellurion_core::BatchItemOutcome::Applied(Sequence(sequence))) => {
                        applied += 1;
                        batch_high_water = Some(
                            batch_high_water
                                .map_or(*sequence, |current: u64| current.max(*sequence)),
                        );
                        BatchOutcomeLine::Applied {
                            index: entry.index,
                            id: entry.feature_id.unwrap_or_default(),
                            sequence: *sequence,
                        }
                    }
                    Some(tellurion_core::BatchItemOutcome::Refused(err)) => {
                        refused += 1;
                        if strict {
                            strict_aborted = true;
                            chunk_strict_stop = true;
                        }
                        BatchOutcomeLine::Refused {
                            index: entry.index,
                            id: entry.feature_id,
                            problem: Problem::from_core_error(err, "batch"),
                        }
                    }
                    None => {
                        // Strict mode stopped `apply_batch` before reaching
                        // this mutation — never attempted.
                        unapplied += 1;
                        strict_aborted = true;
                        BatchOutcomeLine::Unapplied {
                            index: entry.index,
                            id: entry.feature_id,
                        }
                    }
                },
            };
            if tx.send(Ok(encode_line(&line))).await.is_err() {
                return;
            }
        }

        if strict_aborted {
            terminal = BatchTerminalCondition::StrictRefusal;
            if !source_exhausted {
                let tail = report_unapplied_tail(
                    &mut source,
                    &tx,
                    &mut next_index,
                    &mut items_seen,
                    batch_config.max_items,
                )
                .await;
                unapplied += tail.count;
                input_complete = tail.complete;
                unknown_tail = !tail.complete;
                if let Some(source_termination) = tail.termination {
                    let (condition, detail) = match source_termination {
                        TailTermination::ItemLimit => (
                            BatchTerminalCondition::ItemLimit,
                            "batch tail exceeds this collection's item budget",
                        ),
                        TailTermination::ByteLimit => (
                            BatchTerminalCondition::ByteLimit,
                            "request body exceeds this collection's batch byte budget",
                        ),
                        TailTermination::TransportError => (
                            BatchTerminalCondition::TransportError,
                            "request body ended with a transport error",
                        ),
                    };
                    terminal = condition;
                    terminal_problem = Some(Problem::from_core_error(
                        &CoreError::Invalid(detail.to_string()),
                        "batch",
                    ));
                }
            }
            break 'outer;
        }
        if source_exhausted {
            break;
        }
    }

    let (outbox_high_water, watermark_problem) =
        match read_outbox_high_water(&state, &tenant_id, &catalog_id, &collection_id).await {
            Ok(sequence) => (Some(sequence), None),
            Err(error) => (
                None,
                Some(Problem::from_core_error(&error, "batch-watermark")),
            ),
        };
    let summary = BatchSummary {
        type_: "summary",
        applied,
        refused,
        unapplied,
        strict_aborted,
        terminal,
        input_complete,
        unknown_tail,
        terminal_problem,
        batch_high_water,
        outbox_high_water,
        watermark_problem,
    };
    let _ = tx.send(Ok(encode_line(&summary))).await;
}

struct TailReport {
    count: u64,
    complete: bool,
    termination: Option<TailTermination>,
}

enum TailTermination {
    ItemLimit,
    ByteLimit,
    TransportError,
}

async fn report_unapplied_tail(
    source: &mut BatchSource,
    tx: &tokio::sync::mpsc::Sender<std::result::Result<Bytes, std::io::Error>>,
    next_index: &mut u64,
    items_seen: &mut u64,
    max_items: u64,
) -> TailReport {
    let mut unapplied = 0;
    while *items_seen < max_items {
        let item = source.next().await;
        let id = match item {
            NextItem::Eof => {
                return TailReport {
                    count: unapplied,
                    complete: true,
                    termination: None,
                }
            }
            NextItem::BudgetExceeded => {
                return TailReport {
                    count: unapplied,
                    complete: false,
                    termination: Some(TailTermination::ByteLimit),
                }
            }
            NextItem::TransportError => {
                return TailReport {
                    count: unapplied,
                    complete: false,
                    termination: Some(TailTermination::TransportError),
                }
            }
            NextItem::Value(value) => value.get("id").and_then(|id| match id {
                serde_json::Value::String(id) => Some(id.clone()),
                serde_json::Value::Number(id) => Some(id.to_string()),
                _ => None,
            }),
            NextItem::Malformed(_) => None,
        };
        unapplied += 1;
        let line = encode_line(&BatchOutcomeLine::Unapplied {
            index: *next_index,
            id,
        });
        *next_index += 1;
        *items_seen += 1;
        if tx.send(Ok(line)).await.is_err() {
            break;
        }
    }
    let overflow = source.next().await;
    match overflow {
        NextItem::Eof => TailReport {
            count: unapplied,
            complete: true,
            termination: None,
        },
        NextItem::BudgetExceeded => TailReport {
            count: unapplied,
            complete: false,
            termination: Some(TailTermination::ByteLimit),
        },
        NextItem::TransportError => TailReport {
            count: unapplied,
            complete: false,
            termination: Some(TailTermination::TransportError),
        },
        item => {
            let id = match item {
                NextItem::Value(value) => value.get("id").and_then(|id| match id {
                    serde_json::Value::String(id) => Some(id.clone()),
                    serde_json::Value::Number(id) => Some(id.to_string()),
                    _ => None,
                }),
                _ => None,
            };
            let _ = tx
                .send(Ok(encode_line(&BatchOutcomeLine::Unapplied {
                    index: *next_index,
                    id,
                })))
                .await;
            TailReport {
                count: unapplied + 1,
                complete: false,
                termination: Some(TailTermination::ItemLimit),
            }
        }
    }
}

async fn read_outbox_high_water(
    state: &ContextState,
    tenant_id: &str,
    catalog_id: &str,
    collection_id: &str,
) -> std::result::Result<u64, CoreError> {
    let (decl, outbox) = state
        .router
        .resolve_outbox(tenant_id, catalog_id, collection_id)
        .await?;
    outbox
        .primary_high_water(&decl)
        .await
        .map(|Sequence(sequence)| sequence)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Body` that yields `chunks` one at a time, one poll per chunk —
    /// simulates a slow/segmented network delivery so `GeoJsonSeqReader`'s
    /// own claim (never buffers more than one in-flight record) is
    /// exercised across a real chunk boundary, not just against a body that
    /// happens to arrive all at once.
    fn chunked_body(chunks: Vec<&'static [u8]>) -> axum::body::Body {
        let stream = futures::stream::iter(
            chunks
                .into_iter()
                .map(|c| Ok::<Bytes, std::io::Error>(Bytes::from_static(c))),
        );
        axum::body::Body::from_stream(stream)
    }

    async fn collect_records(reader: &mut GeoJsonSeqReader) -> Vec<String> {
        let mut records = Vec::new();
        loop {
            match reader.next_item().await {
                NextItem::Value(value) => records.push(value.to_string()),
                NextItem::Malformed(reason) => records.push(format!("MALFORMED:{reason}")),
                NextItem::Eof => break,
                NextItem::BudgetExceeded => {
                    records.push("BUDGET".to_string());
                    break;
                }
                NextItem::TransportError => {
                    records.push("TRANSPORT".to_string());
                    break;
                }
            }
        }
        records
    }

    #[tokio::test]
    async fn splits_two_records_delivered_in_one_chunk() {
        let body = chunked_body(vec![b"\x1E{\"a\":1}\n\x1E{\"a\":2}\n"]);
        let mut reader = GeoJsonSeqReader::new(body, 1_000_000);
        let records = collect_records(&mut reader).await;
        assert_eq!(records, vec!["{\"a\":1}", "{\"a\":2}"]);
    }

    /// The same two records, but the second record's `RS` and its JSON text
    /// arrive in SEPARATE network chunks — proving reassembly across a
    /// chunk boundary the previous test's single-chunk delivery can't prove
    /// on its own.
    #[tokio::test]
    async fn reassembles_a_record_split_across_chunk_boundaries() {
        let body = chunked_body(vec![b"\x1E{\"a\":1}\n\x1E{\"a", b"\":2}\n"]);
        let mut reader = GeoJsonSeqReader::new(body, 1_000_000);
        let records = collect_records(&mut reader).await;
        assert_eq!(records, vec!["{\"a\":1}", "{\"a\":2}"]);
    }

    #[tokio::test]
    async fn a_final_record_with_no_trailing_rs_is_still_recognized() {
        let body = chunked_body(vec![b"\x1E{\"a\":1}\n"]);
        let mut reader = GeoJsonSeqReader::new(body, 1_000_000);
        let records = collect_records(&mut reader).await;
        assert_eq!(records, vec!["{\"a\":1}"]);
    }

    #[tokio::test]
    async fn parser_recovers_a_self_delimiting_final_object_without_a_line_feed() {
        let body = chunked_body(vec![b"\x1E{\"a\":1}"]);
        let mut reader = GeoJsonSeqReader::new(body, 1_000_000);
        let records = collect_records(&mut reader).await;
        assert_eq!(records, vec!["{\"a\":1}"]);
    }

    #[tokio::test]
    async fn ignores_consecutive_separators_and_recovers_after_junk() {
        let body = chunked_body(vec![
            b"junk\x1E\x1E{\n  \"a\": 1\n}\n\x1Enot-json\n\x1E",
            b"{\"a\":2}",
        ]);
        let mut reader = GeoJsonSeqReader::new(body, 1_000_000);
        let records = collect_records(&mut reader).await;
        assert_eq!(records.len(), 4);
        assert!(records[0].starts_with("MALFORMED:"));
        assert_eq!(records[1], "{\"a\":1}");
        assert!(records[2].starts_with("MALFORMED:"));
        assert_eq!(records[3], "{\"a\":2}");
    }

    #[tokio::test]
    async fn a_malformed_record_is_reported_without_aborting_the_rest_of_the_stream() {
        let body = chunked_body(vec![b"\x1Enot-json\n\x1E{\"a\":2}\n"]);
        let mut reader = GeoJsonSeqReader::new(body, 1_000_000);
        let records = collect_records(&mut reader).await;
        assert_eq!(records.len(), 2);
        assert!(records[0].starts_with("MALFORMED:"));
        assert_eq!(records[1], "{\"a\":2}");
    }

    #[tokio::test]
    async fn exceeding_the_byte_budget_stops_the_stream_in_band() {
        let body = chunked_body(vec![b"\x1E{\"a\":1}\n", b"\x1E{\"a\":2}\n"]);
        // A budget smaller than the whole body but big enough to admit the
        // first chunk — the second chunk's read is what trips it.
        let mut reader = GeoJsonSeqReader::new(body, 9);
        let records = collect_records(&mut reader).await;
        assert_eq!(records.last().map(String::as_str), Some("BUDGET"));
    }

    #[test]
    fn stage_one_extracts_a_string_id() {
        let decl: CollectionDecl =
            serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap();
        let feature = serde_json::json!({
            "type": "Feature",
            "id": "42",
            "geometry": null,
            "properties": {}
        });
        let mutation = stage_one(feature, &decl).expect("stages cleanly");
        assert_eq!(mutation.feature_id, "42");
        assert!(matches!(
            mutation.kind,
            tellurion_core::MutationKind::Upsert(_)
        ));
    }

    #[test]
    fn stage_one_coerces_a_numeric_id_to_a_string() {
        let decl: CollectionDecl =
            serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap();
        let feature = serde_json::json!({
            "type": "Feature", "id": 42, "geometry": null, "properties": {}
        });
        let mutation = stage_one(feature, &decl).expect("stages cleanly");
        assert_eq!(mutation.feature_id, "42");
    }

    #[test]
    fn stage_one_refuses_a_feature_with_no_id_and_names_neither_id_nor_anything_fabricated() {
        let decl: CollectionDecl =
            serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap();
        let feature = serde_json::json!({
            "type": "Feature", "geometry": null, "properties": {}
        });
        let (id, err) = stage_one(feature, &decl).expect_err("must refuse");
        assert_eq!(id, None);
        assert!(matches!(err, CoreError::Invalid(_)));
    }

    #[test]
    fn stage_one_refuses_a_non_object_top_level_value() {
        let decl: CollectionDecl =
            serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap();
        let (id, err) = stage_one(serde_json::json!([1, 2, 3]), &decl).expect_err("must refuse");
        assert_eq!(id, None);
        assert!(matches!(err, CoreError::Invalid(_)));
    }

    #[test]
    fn stage_one_refuses_non_feature_types_and_invalid_geometry() {
        let decl: CollectionDecl =
            serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap();
        let wrong_type = serde_json::json!({
            "type": "Point", "id": "1", "coordinates": [1, 2]
        });
        assert!(stage_one(wrong_type, &decl).is_err());

        let bad_geometry = serde_json::json!({
            "type": "Feature", "id": "1", "geometry": {
                "type": "Point", "coordinates": [1]
            }, "properties": {}
        });
        assert!(stage_one(bad_geometry, &decl).is_err());
    }

    #[test]
    fn feature_collection_requires_its_type_and_valid_bbox() {
        let wrong_type = serde_json::to_vec(&serde_json::json!({
            "type": "Feature", "features": []
        }))
        .unwrap();
        assert!(parse_feature_collection(&wrong_type).is_err());

        let invalid_bbox = serde_json::to_vec(&serde_json::json!({
            "type": "FeatureCollection", "bbox": [0, 1, 2], "features": []
        }))
        .unwrap();
        assert!(parse_feature_collection(&invalid_bbox).is_err());
    }
}
