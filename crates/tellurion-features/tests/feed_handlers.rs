//! `GET /collections/{cid}/changes` (`#115`) integration tests: a fake, in-
//! memory `OutboxSource` driven through the real `tellurion_core::Router`
//! and the real axum router this crate exports — no database involved.
//! Mirrors `handlers.rs`'s own test harness shape (`FakeDriver`/
//! `FakeFactory`/`build_ctx`), a separate file since this lane needs a
//! `routing.write` outbox rather than a `FeatureSource`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::Response;
use serde_json::{json, Value};
use tower::ServiceExt;

use tellurion_core::{
    AppConfig, AppContext, CatalogSource, CollectionDecl, DriverFactory, MokaTileCache,
    MutationKind, Obligation, PhysicalCollection, Registry, Resolver, Result as CoreResult,
    Router as CoreRouter, Sequence, StaticResolver, StorageDecl, StorageDriver, StyleStore,
    TileCache, WriteSink,
};

struct EmptyCatalog;

#[async_trait::async_trait]
impl CatalogSource for EmptyCatalog {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        Ok(vec![])
    }
}

fn upsert_at(sequence: u64, feature_id: &str) -> Obligation {
    Obligation {
        sequence: Sequence(sequence),
        feature_id: feature_id.to_string(),
        kind: MutationKind::Upsert(json!({"type": "Feature", "properties": {"secret": "nope"}})),
        version: Sequence(sequence),
        committed_at: SystemTime::UNIX_EPOCH + Duration::from_secs(sequence),
        extent: tellurion_core::ObligationExtent::Unrecorded,
    }
}

fn delete_at(sequence: u64, feature_id: &str) -> Obligation {
    Obligation {
        sequence: Sequence(sequence),
        feature_id: feature_id.to_string(),
        kind: MutationKind::Delete,
        version: Sequence(sequence),
        committed_at: SystemTime::UNIX_EPOCH + Duration::from_secs(sequence),
        extent: tellurion_core::ObligationExtent::Unrecorded,
    }
}

/// In-memory `OutboxSource` fixture — a fixed, ordered obligation log, the
/// same shape `tellurion-core`'s own applier/invalidation fake outboxes use.
struct FakeOutbox {
    obligations: Vec<Obligation>,
}

#[async_trait::async_trait]
impl tellurion_core::OutboxSource for FakeOutbox {
    async fn read_after(
        &self,
        _collection: &CollectionDecl,
        after: Sequence,
        limit: u32,
    ) -> CoreResult<Vec<Obligation>> {
        Ok(self
            .obligations
            .iter()
            .filter(|o| o.sequence > after)
            .take(limit as usize)
            .cloned()
            .collect())
    }

    async fn primary_high_water(&self, _collection: &CollectionDecl) -> CoreResult<Sequence> {
        Ok(self
            .obligations
            .last()
            .map(|o| o.sequence)
            .unwrap_or(Sequence(0)))
    }
}

/// A minimal `WriteSink` — never actually exercised by these tests (the
/// feed lane only ever reads), present only because `routing.write` needs
/// SOME driver capability declared to resolve at all in a couple of
/// `Router::build` code paths that assume a write-routed collection.
struct NoopWriteSink;

#[async_trait::async_trait]
impl WriteSink for NoopWriteSink {
    async fn apply(
        &self,
        _collection: &CollectionDecl,
        _mutation: tellurion_core::Mutation,
    ) -> CoreResult<Sequence> {
        Ok(Sequence(1))
    }
}

struct FakeOutboxDriver {
    outbox: Arc<FakeOutbox>,
}

impl StorageDriver for FakeOutboxDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(EmptyCatalog)
    }

    fn write_sink(&self) -> Option<Arc<dyn WriteSink>> {
        Some(Arc::new(NoopWriteSink))
    }

    fn outbox_source(&self) -> Option<Arc<dyn tellurion_core::OutboxSource>> {
        Some(self.outbox.clone() as Arc<dyn tellurion_core::OutboxSource>)
    }
}

struct FakeOutboxFactory {
    outbox: Arc<FakeOutbox>,
}

impl DriverFactory for FakeOutboxFactory {
    fn name(&self) -> &str {
        "fake-outbox"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(FakeOutboxDriver {
            outbox: self.outbox.clone(),
        }))
    }
}

const FEED_CONFIG: &str = r#"
storages: [ { id: main, driver: fake-outbox, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing:
      write: main
"#;

fn build_ctx(config_yaml: &str, obligations: Vec<Obligation>) -> Arc<AppContext> {
    let config: AppConfig = serde_yaml::from_str(config_yaml).unwrap();
    config.validate().unwrap();

    let outbox = Arc::new(FakeOutbox { obligations });
    let mut registry = Registry::new();
    registry.register(Arc::new(FakeOutboxFactory { outbox }));

    let core_router = CoreRouter::build(&config, &registry).unwrap();
    let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1024));
    let style_store: Arc<dyn StyleStore> = Arc::new(tellurion_core::FileStyleStore::new(&[]));
    let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
    let authorizer = tellurion_core::build_authorizer(&config.auth)
        .expect("no bearer principal in this fixture reads a token_env");
    Arc::new(AppContext::new(
        config,
        core_router,
        resolver,
        authorizer,
        cache,
        style_store,
    ))
}

fn build_app(config_yaml: &str, obligations: Vec<Obligation>) -> axum::Router {
    tellurion_features::router().with_state(build_ctx(config_yaml, obligations))
}

async fn body_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn find_link<'a>(body: &'a Value, rel: &str) -> Option<&'a Value> {
    body["links"].as_array()?.iter().find(|l| l["rel"] == rel)
}

async fn get(app: &axum::Router, uri: impl AsRef<str>) -> Response {
    get_with_bearer(app, uri, None).await
}

async fn get_with_bearer(
    app: &axum::Router,
    uri: impl AsRef<str>,
    token: Option<&str>,
) -> Response {
    let mut request = Request::builder().uri(uri.as_ref());
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn changes_reports_entries_in_ascending_order_with_no_payload_field() {
    let app = build_app(
        FEED_CONFIG,
        vec![upsert_at(1, "a"), delete_at(2, "b"), upsert_at(3, "c")],
    );
    let response = get(&app, "/collections/demo/changes").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let changes = body["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[0]["sequence"], 1);
    assert_eq!(changes[0]["operation"], "upsert");
    assert_eq!(changes[0]["itemId"], "a");
    assert_eq!(changes[0]["schemaVersion"], 1);
    assert!(changes[0]["committedAt"].as_str().unwrap().ends_with('Z'));
    // The envelope never carries the obligation's own payload — the
    // upsert's "secret" property must never appear anywhere in the body.
    assert!(!body.to_string().contains("secret"));
    assert_eq!(changes[1]["operation"], "delete");
    assert_eq!(changes[1]["sequence"], 2);
    assert!(find_link(&body, "next").is_none());
}

#[tokio::test]
async fn changes_paginates_with_a_keyset_cursor_never_offset() {
    let app = build_app(
        FEED_CONFIG,
        vec![upsert_at(1, "a"), upsert_at(2, "b"), upsert_at(3, "c")],
    );

    let first = get(&app, "/collections/demo/changes?limit=2").await;
    assert_eq!(first.status(), StatusCode::OK);
    let body = body_json(first).await;
    assert_eq!(body["changes"].as_array().unwrap().len(), 2);
    let next_href = find_link(&body, "next").expect("a full page should carry a next link")["href"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(next_href.contains("since=2"), "next href was: {next_href}");

    let second = get(&app, &next_href).await;
    assert_eq!(second.status(), StatusCode::OK);
    let body2 = body_json(second).await;
    let changes2 = body2["changes"].as_array().unwrap();
    assert_eq!(changes2.len(), 1);
    assert_eq!(changes2[0]["sequence"], 3);
    assert!(
        find_link(&body2, "next").is_none(),
        "a short page must never carry a next link"
    );
}

#[tokio::test]
async fn changes_defaults_to_an_empty_page_when_fully_caught_up() {
    let app = build_app(FEED_CONFIG, vec![upsert_at(1, "a")]);
    let response = get(&app, "/collections/demo/changes?since=1").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["changes"].as_array().unwrap().len(), 0);
    assert!(find_link(&body, "next").is_none());
}

#[tokio::test]
async fn a_malformed_cursor_is_a_400_problem_json() {
    let app = build_app(FEED_CONFIG, vec![upsert_at(1, "a")]);
    let response = get(&app, "/collections/demo/changes?since=not-a-cursor").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        tellurion_features::PROBLEM_JSON
    );
}

#[tokio::test]
async fn a_collection_with_no_write_routing_refuses_the_feed_with_a_named_404() {
    const NO_OUTBOX_CONFIG: &str = r#"
storages: [ { id: main, driver: fake-outbox, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
"#;
    let app = build_app(NO_OUTBOX_CONFIG, vec![]);
    let response = get(&app, "/collections/demo/changes").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = body_json(response).await;
    assert_eq!(body["code"], "NotFound");
    assert!(
        body["detail"].as_str().unwrap().contains("outbox"),
        "detail should name the missing 'outbox' capability, was: {body}"
    );
}

// ---- policy (`#34`/`#115`) --------------------------------------------

const POLICY_FEED_CONFIG: &str = r#"
storages: [ { id: main, driver: fake-outbox, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing:
      write: main
auth:
  bearer_tokens:
    - token: no-role-token
      tenants: [public]
    - token: reader-token
      tenants: [public]
      roles:
        public: [reader]
policy:
  roles:
    - name: reader
      grants:
        - scope: {}
          lanes: [feed]
"#;

#[tokio::test]
async fn no_credential_against_a_private_collection_is_401_when_auth_is_configured() {
    let app = build_app(POLICY_FEED_CONFIG, vec![upsert_at(1, "a")]);
    let response = get_with_bearer(&app, "/collections/demo/changes", None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_member_with_no_grant_for_the_feed_lane_is_403() {
    let app = build_app(POLICY_FEED_CONFIG, vec![upsert_at(1, "a")]);
    let response = get_with_bearer(&app, "/collections/demo/changes", Some("no-role-token")).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_member_with_an_unconditional_feed_grant_reads_the_feed() {
    let app = build_app(POLICY_FEED_CONFIG, vec![upsert_at(1, "a")]);
    let response = get_with_bearer(&app, "/collections/demo/changes", Some("reader-token")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["changes"].as_array().unwrap().len(), 1);
}
