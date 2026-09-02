//! Handler tests for `POST /collections/{cid}/items/batch` (`#114`): an
//! in-memory `WriteSink::apply_batch` fixture driven through the real
//! `tellurion_core::Router` and the real axum router this crate exports —
//! no database involved, same shape as `handlers.rs`'s own write-lane
//! tests.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::Response;
use serde_json::{json, Value};
use tower::ServiceExt;

use tellurion_core::{
    AppConfig, AppContext, AttributeColumn, BatchItemOutcome, BatchItemResult, CatalogSource,
    CollectionDecl, DriverFactory, Error as CoreError, FileStyleStore, MokaTileCache, Mutation,
    Obligation, OutboxSource, PhysicalCollection, Registry, RequestedCrs, Resolver,
    Result as CoreResult, Router as CoreRouter, Sequence, SpatialExtent, StaticResolver,
    StorageDecl, StorageDriver, StyleStore, TileCache, WriteSink,
};

struct EmptyCatalog;

#[async_trait::async_trait]
impl CatalogSource for EmptyCatalog {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        Ok(vec![])
    }
}

/// Reports the `demo` table's physical shape — the `main` (features/read)
/// storage's catalog, checked at boot by `Router::validate_catalog` against
/// this collection's explicit `table`/`geometry`/`pk`. The `writable`
/// storage below is a different lane (`routing.write`), never subject to
/// this same physical cross-check — see `Router::resolve_write`'s own doc.
struct DemoCatalog;

#[async_trait::async_trait]
impl CatalogSource for DemoCatalog {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        Ok(vec![PhysicalCollection {
            name: "demo".to_string(),
            geometry_column: Some("geom".to_string()),
            primary_key: Some("id".to_string()),
            srid: Some(4326),
            geometry_type: None,
        }])
    }

    async fn attribute_schema(
        &self,
        _physical: &PhysicalCollection,
    ) -> CoreResult<Option<Vec<AttributeColumn>>> {
        Ok(Some(vec![AttributeColumn {
            name: "name".to_string(),
            sql_type: "text".to_string(),
        }]))
    }

    async fn extent(&self, _physical: &PhysicalCollection) -> CoreResult<Option<SpatialExtent>> {
        Ok(None)
    }
}

struct MainDriver;

impl StorageDriver for MainDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(DemoCatalog)
    }
}

struct MainFactory;

impl DriverFactory for MainFactory {
    fn name(&self) -> &str {
        "batch-fake-main"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(MainDriver))
    }
}

/// In-memory `WriteSink::apply_batch` fixture: `refuse_ids` names every
/// feature id this fake refuses (an `Invalid` error, the same variant a
/// real driver's own named refusals map to), everything else applies with a
/// freshly minted, monotonically increasing sequence.
struct FakeBatchWriteSink {
    refuse_ids: HashSet<String>,
    fail_batch: bool,
    sequence: AtomicU64,
}

#[async_trait::async_trait]
impl WriteSink for FakeBatchWriteSink {
    async fn apply(
        &self,
        _collection: &CollectionDecl,
        _mutation: Mutation,
    ) -> CoreResult<Sequence> {
        Ok(Sequence(self.sequence.fetch_add(1, Ordering::SeqCst)))
    }

    async fn apply_batch(
        &self,
        _collection: &CollectionDecl,
        mutations: Vec<Mutation>,
        _requested_crs: RequestedCrs,
        strict: bool,
    ) -> CoreResult<Vec<BatchItemResult>> {
        if self.fail_batch {
            return Err(CoreError::Timeout);
        }
        let mut results = Vec::new();
        for mutation in mutations {
            let refused = self.refuse_ids.contains(&mutation.feature_id);
            let outcome = if refused {
                BatchItemOutcome::Refused(CoreError::Invalid(format!(
                    "fake refusal for '{}'",
                    mutation.feature_id
                )))
            } else {
                let sequence = self.sequence.fetch_add(1, Ordering::SeqCst);
                BatchItemOutcome::Applied(Sequence(sequence))
            };
            let stop_here = strict && matches!(outcome, BatchItemOutcome::Refused(_));
            results.push(BatchItemResult {
                feature_id: mutation.feature_id,
                outcome,
            });
            if stop_here {
                break;
            }
        }
        Ok(results)
    }
}

#[async_trait::async_trait]
impl OutboxSource for FakeBatchWriteSink {
    async fn read_after(
        &self,
        _collection: &CollectionDecl,
        _after: Sequence,
        _limit: u32,
    ) -> CoreResult<Vec<Obligation>> {
        Ok(vec![])
    }

    async fn primary_high_water(&self, _collection: &CollectionDecl) -> CoreResult<Sequence> {
        if self.refuse_ids.contains("__watermark__") {
            return Err(CoreError::Timeout);
        }
        Ok(Sequence(self.sequence.load(Ordering::SeqCst)))
    }
}

struct WritableDriver {
    sink: Arc<FakeBatchWriteSink>,
}

impl StorageDriver for WritableDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(EmptyCatalog)
    }

    fn write_sink(&self) -> Option<Arc<dyn WriteSink>> {
        Some(self.sink.clone() as Arc<dyn WriteSink>)
    }

    fn outbox_source(&self) -> Option<Arc<dyn OutboxSource>> {
        Some(self.sink.clone() as Arc<dyn OutboxSource>)
    }
}

struct WritableFactory {
    refuse_ids: HashSet<String>,
    fail_batch: bool,
}

impl DriverFactory for WritableFactory {
    fn name(&self) -> &str {
        "batch-fake-write"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(WritableDriver {
            sink: Arc::new(FakeBatchWriteSink {
                refuse_ids: self.refuse_ids.clone(),
                fail_batch: self.fail_batch,
                sequence: AtomicU64::new(1),
            }),
        }))
    }
}

const CONFIG: &str = r#"
storages:
  - { id: main, driver: batch-fake-main, url_env: DATABASE_URL }
  - { id: writable, driver: batch-fake-write, url_env: DATABASE_URL }
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing: { write: writable }
settings:
  batch: { max_bytes: 1000000, max_items: 100, chunk_items: 2 }
"#;

fn build_app(refuse_ids: &[&str]) -> axum::Router {
    build_app_with_batch_failure(refuse_ids, false)
}

fn build_app_with_batch_failure(refuse_ids: &[&str], fail_batch: bool) -> axum::Router {
    build_app_from_config(CONFIG, refuse_ids, fail_batch)
}

fn build_app_from_config(config_yaml: &str, refuse_ids: &[&str], fail_batch: bool) -> axum::Router {
    let config: AppConfig = serde_yaml::from_str(config_yaml).unwrap();
    config.validate().unwrap();

    let mut registry = Registry::new();
    registry.register(Arc::new(MainFactory));
    registry.register(Arc::new(WritableFactory {
        refuse_ids: refuse_ids.iter().map(|s| s.to_string()).collect(),
        fail_batch,
    }));

    let core_router = CoreRouter::build(&config, &registry).unwrap();
    let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1024));
    let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
    let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
    let authorizer = tellurion_core::build_authorizer(&config.auth)
        .expect("no bearer principal in this fixture reads a token_env");
    let ctx = Arc::new(AppContext::new(
        config,
        core_router,
        resolver,
        authorizer,
        cache,
        style_store,
    ));
    tellurion_features::router().with_state(ctx)
}

fn geojson_seq_body(features: &[Value]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for feature in features {
        bytes.push(0x1E);
        bytes.extend_from_slice(serde_json::to_string(feature).unwrap().as_bytes());
        bytes.push(b'\n');
    }
    bytes
}

async fn post_seq(app: &axum::Router, features: &[Value], strict: bool) -> Response {
    let path = if strict {
        "/collections/demo/items/batch?strict=true"
    } else {
        "/collections/demo/items/batch"
    };
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(header::CONTENT_TYPE, "application/geo+json-seq")
                .body(Body::from(geojson_seq_body(features)))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn body_lines(response: Response) -> Vec<Value> {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec())
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn feature(id: &str) -> Value {
    json!({
        "type": "Feature",
        "id": id,
        "geometry": {"type": "Point", "coordinates": [1.0, 2.0]},
        "properties": {"name": id}
    })
}

#[tokio::test]
async fn a_streamed_batch_of_clean_features_applies_every_item() {
    let app = build_app(&[]);
    let features = vec![feature("1"), feature("2"), feature("3")];
    let response = post_seq(&app, &features, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/x-ndjson")
    );

    let lines = body_lines(response).await;
    assert_eq!(lines.len(), 4, "3 outcomes + 1 summary");
    for (index, line) in lines.iter().take(3).enumerate() {
        assert_eq!(line["type"], "applied");
        assert_eq!(line["id"], (index + 1).to_string());
    }
    let summary = &lines[3];
    assert_eq!(summary["type"], "summary");
    assert_eq!(summary["applied"], 3);
    assert_eq!(summary["refused"], 0);
    assert_eq!(summary["unapplied"], 0);
    assert_eq!(summary["strict_aborted"], false);
    assert_eq!(summary["terminal"], "complete");
    assert_eq!(summary["batch_high_water"], 3);
    assert_eq!(summary["outbox_high_water"], 4);
}

#[tokio::test]
async fn a_dirty_row_is_refused_by_name_while_clean_siblings_still_apply() {
    let app = build_app(&["2"]);
    let features = vec![feature("1"), feature("2"), feature("3")];
    let response = post_seq(&app, &features, false).await;
    assert_eq!(response.status(), StatusCode::OK);

    let lines = body_lines(response).await;
    assert_eq!(lines[0]["type"], "applied");
    assert_eq!(lines[1]["type"], "refused");
    assert_eq!(lines[1]["id"], "2");
    assert_eq!(lines[1]["problem"]["code"], "InvalidParameter");
    assert_eq!(lines[2]["type"], "applied");
    let summary = &lines[3];
    assert_eq!(summary["applied"], 2);
    assert_eq!(summary["refused"], 1);
}

#[tokio::test]
async fn a_chunk_transaction_failure_reports_each_staged_item() {
    let app = build_app_with_batch_failure(&[], true);
    let response = post_seq(&app, &[feature("1"), feature("2"), feature("3")], false).await;
    assert_eq!(response.status(), StatusCode::OK);

    let lines = body_lines(response).await;
    assert_eq!(
        lines.len(),
        3,
        "two unapplied chunk items and one terminal summary"
    );
    for (index, line) in lines.iter().take(2).enumerate() {
        assert_eq!(line["type"], "unapplied");
        assert_eq!(line["index"], index);
        assert_eq!(line["id"], (index + 1).to_string());
    }
    assert_eq!(lines[2]["type"], "summary");
    assert_eq!(lines[2]["refused"], 0);
    assert_eq!(lines[2]["unapplied"], 2);
    assert_eq!(lines[2]["terminal"], "chunk_error");
    assert_eq!(lines[2]["input_complete"], false);
    assert_eq!(lines[2]["unknown_tail"], true);
    assert_eq!(lines[2]["terminal_problem"]["code"], "Timeout");
}

#[tokio::test]
async fn a_chunk_transaction_failure_is_not_a_strict_abort() {
    let app = build_app_with_batch_failure(&[], true);
    let response = post_seq(&app, &[feature("1"), feature("2")], true).await;
    assert_eq!(response.status(), StatusCode::OK);

    let lines = body_lines(response).await;
    assert_eq!(lines[2]["type"], "summary");
    assert_eq!(lines[2]["strict_aborted"], false);
}

#[tokio::test]
async fn strict_mode_reports_the_remainder_as_unapplied() {
    let app = build_app(&["2"]);
    let features = vec![feature("1"), feature("2"), feature("3")];
    let response = post_seq(&app, &features, true).await;
    assert_eq!(response.status(), StatusCode::OK);

    let lines = body_lines(response).await;
    assert_eq!(lines[0]["type"], "applied");
    assert_eq!(lines[1]["type"], "refused");
    assert_eq!(lines[2]["type"], "unapplied");
    assert_eq!(lines[2]["id"], "3");
    let summary = &lines[3];
    assert_eq!(summary["applied"], 1);
    assert_eq!(summary["refused"], 1);
    assert_eq!(summary["unapplied"], 1);
    assert_eq!(summary["strict_aborted"], true);
}

#[tokio::test]
async fn a_feature_missing_a_top_level_id_is_refused_without_ever_reaching_the_driver() {
    let app = build_app(&[]);
    let no_id = json!({
        "type": "Feature",
        "geometry": {"type": "Point", "coordinates": [1.0, 2.0]},
        "properties": {"name": "no-id"}
    });
    let response = post_seq(&app, &[no_id], false).await;
    assert_eq!(response.status(), StatusCode::OK);

    let lines = body_lines(response).await;
    assert_eq!(lines[0]["type"], "refused");
    assert_eq!(lines[0]["id"], Value::Null);
    assert!(lines[0]["problem"]["detail"]
        .as_str()
        .unwrap()
        .contains("top-level 'id'"));
    assert_eq!(lines[1]["applied"], 0);
    assert_eq!(lines[1]["refused"], 1);
}

#[tokio::test]
async fn a_feature_collection_body_applies_through_the_same_lane() {
    let app = build_app(&[]);
    let body = json!({
        "type": "FeatureCollection",
        "features": [feature("1"), feature("2")]
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/collections/demo/items/batch")
                .header(header::CONTENT_TYPE, "application/geo+json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let lines = body_lines(response).await;
    assert_eq!(lines[0]["type"], "applied");
    assert_eq!(lines[1]["type"], "applied");
    assert_eq!(lines[2]["applied"], 2);
}

#[tokio::test]
async fn an_unrecognized_media_type_is_refused_before_any_streaming_begins() {
    let app = build_app(&[]);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/collections/demo/items/batch")
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from(b"not geojson".to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn exceeding_the_item_budget_is_reported_in_band() {
    let config = CONFIG.replace("max_items: 100", "max_items: 2");
    let app = build_app_from_config(&config, &[], false);
    let response = post_seq(&app, &[feature("1"), feature("2"), feature("3")], false).await;
    assert_eq!(response.status(), StatusCode::OK);

    let lines = body_lines(response).await;
    assert_eq!(lines[0]["type"], "applied");
    assert_eq!(lines[1]["type"], "applied");
    assert_eq!(lines[2]["type"], "unapplied");
    assert_eq!(lines[2]["index"], 2);
    assert_eq!(lines[3]["type"], "summary");
    assert_eq!(lines[3]["applied"], 2);
    assert_eq!(lines[3]["refused"], 0);
    assert_eq!(lines[3]["unapplied"], 1);
    assert_eq!(lines[3]["terminal"], "item_limit");
    assert_eq!(lines[3]["unknown_tail"], true);
}

#[tokio::test]
async fn exceeding_the_byte_budget_is_reported_in_band() {
    let config = CONFIG.replace("max_bytes: 1000000", "max_bytes: 1");
    let app = build_app_from_config(&config, &[], false);
    let response = post_seq(&app, &[feature("1")], false).await;
    assert_eq!(response.status(), StatusCode::OK);

    let lines = body_lines(response).await;
    assert_eq!(lines[0]["type"], "summary");
    assert_eq!(lines[0]["terminal"], "byte_limit");
    assert_eq!(lines[0]["input_complete"], false);
    assert_eq!(lines[0]["unknown_tail"], true);
    assert!(lines[0]["terminal_problem"]["detail"]
        .as_str()
        .unwrap()
        .contains("batch byte budget"));
    assert_eq!(lines[0]["applied"], 0);
    assert_eq!(lines[0]["refused"], 0);
}

#[tokio::test]
async fn byte_termination_remains_incomplete_when_the_staged_chunk_also_fails() {
    let mut first = geojson_seq_body(&[feature("1")]);
    first.push(0x1e);
    let config = CONFIG.replace("max_bytes: 1000000", &format!("max_bytes: {}", first.len()));
    let app = build_app_from_config(&config, &[], true);
    let chunks = vec![
        Ok::<_, std::io::Error>(bytes::Bytes::from(first)),
        Ok(bytes::Bytes::from_static(b"\x1e{}\n")),
    ];
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/collections/demo/items/batch")
                .header(header::CONTENT_TYPE, "application/geo+json-seq")
                .body(Body::from_stream(futures::stream::iter(chunks)))
                .unwrap(),
        )
        .await
        .unwrap();
    let lines = body_lines(response).await;
    let summary = lines.last().unwrap();
    assert_eq!(summary["terminal"], "chunk_error");
    assert_eq!(summary["input_complete"], false);
    assert_eq!(summary["unknown_tail"], true);
}

#[tokio::test]
async fn media_types_are_case_insensitive() {
    let app = build_app(&[]);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/collections/demo/items/batch")
                .header(
                    header::CONTENT_TYPE,
                    "Application/Geo+Json-Seq; Charset=UTF-8",
                )
                .body(Body::from(geojson_seq_body(&[feature("1")])))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_lines(response).await[0]["type"], "applied");
}

#[tokio::test]
async fn watermark_failures_are_explicit_and_do_not_erase_the_batch_high_water() {
    let app = build_app(&["__watermark__"]);
    let response = post_seq(&app, &[feature("1")], false).await;
    let lines = body_lines(response).await;
    let summary = lines.last().unwrap();
    assert_eq!(summary["batch_high_water"], 1);
    assert_eq!(summary["outbox_high_water"], Value::Null);
    assert_eq!(summary["watermark_problem"]["code"], "Timeout");
}

#[tokio::test]
async fn batch_route_enforces_the_write_lane_auth_gate() {
    let config = format!(
        "{CONFIG}\nauth:\n  bearer_tokens:\n    - {{ token: member-token, tenants: [public] }}\n"
    );
    let app = build_app_from_config(&config, &[], false);
    let body = geojson_seq_body(&[feature("1")]);

    let unauthenticated = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/collections/demo/items/batch")
                .header(header::CONTENT_TYPE, "application/geo+json-seq")
                .body(Body::from(body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let authenticated = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/collections/demo/items/batch")
                .header(header::CONTENT_TYPE, "application/geo+json-seq")
                .header(header::AUTHORIZATION, "Bearer member-token")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(authenticated.status(), StatusCode::OK);
}
