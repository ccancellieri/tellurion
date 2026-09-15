//! Handler tests: the real axum router this crate exports, the real
//! `tellurion_core::Router`, the real policy layer, and a ledger backed by
//! `tellurion_core::InMemoryJobStore` — which deliberately reproduces every
//! invariant the `tellurion_jobs` table enforces (read that type's own doc; a
//! fixture that enforces less than the real store would let these tests pass
//! for the wrong reason).
//!
//! What is NOT covered here, on purpose, because a fake could only fake it:
//! claim exclusivity under concurrency. That is decided by the SQL text —
//! `FOR UPDATE SKIP LOCKED`, the visibility predicate, the partial dedup index
//! — and is asserted against that text in `tellurion-postgis::job_sql`'s own
//! tests and `tellurion-ingest::processes`'s DDL tests.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::Response;
use serde_json::{json, Value};
use tower::ServiceExt;

use tellurion_core::{
    AppConfig, AppContext, CatalogSource, DriverFactory, FileStyleStore, InMemoryJobStore,
    JobControlOption, JobLedger, JobOutcome, JobRecord, JobScope, JobStore, MokaTileCache,
    PhysicalCollection, PolicyLane, ProcessDescription, ProcessLane, ProcessRegistry,
    ProcessRunner, ProcessTarget, Registry, Resolver, Result as CoreResult, Router as CoreRouter,
    StaticResolver, StorageDecl, StorageDriver, StyleStore, TileCache,
};

const CONFIG: &str = r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
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

/// A driver that advertises nothing. These tests never read a collection —
/// the Processes lane touches storage only through its ledger — so this exists
/// to satisfy `Router::build` and, deliberately, to prove the lane needs no
/// feature/tile capability at all. Notably it advertises no `job_store`
/// either: this lane's ledger is injected, exactly as `process_lane::build`
/// resolves it once at boot rather than per request.
struct FakeDriver;

#[async_trait::async_trait]
impl CatalogSource for FakeDriver {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        Ok(vec![])
    }
}

impl StorageDriver for FakeDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(FakeDriver)
    }
}

struct FakeFactory;

impl DriverFactory for FakeFactory {
    fn name(&self) -> &str {
        "fake"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(FakeDriver))
    }
}

/// A runner with no side effects, so these tests exercise the HTTP surface
/// rather than any particular process. Its `target` mirrors the real
/// `index-rebuild` runner's: a collection named by an input, authorized
/// through the write lane.
struct EchoRunner;

#[async_trait::async_trait]
impl ProcessRunner for EchoRunner {
    fn description(&self) -> ProcessDescription {
        ProcessDescription {
            id: "echo".to_string(),
            version: "1.0.0".to_string(),
            title: Some("Echo".to_string()),
            description: None,
            job_control_options: vec![JobControlOption::AsyncExecute, JobControlOption::Dismiss],
        }
    }

    fn validate_inputs(&self, inputs: &Value) -> CoreResult<()> {
        if inputs.get("reject").is_some() {
            return Err(tellurion_core::Error::Invalid(
                "input 'reject' is not accepted".to_string(),
            ));
        }
        Ok(())
    }

    fn target(&self, inputs: &Value) -> Option<ProcessTarget> {
        inputs
            .get("collection")
            .and_then(Value::as_str)
            .map(|collection| ProcessTarget {
                collection: collection.to_string(),
                lane: PolicyLane::Write,
            })
    }

    async fn execute(&self, job: &JobRecord) -> CoreResult<Value> {
        Ok(json!({ "echoed": job.inputs.clone() }))
    }
}

fn build_ctx(config_yaml: &str) -> Arc<AppContext> {
    let config: AppConfig = serde_yaml::from_str(config_yaml).unwrap();
    config.validate().unwrap();
    let mut registry = Registry::new();
    registry.register(Arc::new(FakeFactory));
    let core_router = CoreRouter::build(&config, &registry).unwrap();
    let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1024));
    let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
    let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
    // Built from the config, never hardcoded to `None`: a config with no
    // `auth:` yields `None` (and the policy checkpoint is skipped, exactly as
    // in production), while the policy test below gets a real authorizer. A
    // fixture that always passed `None` would make that test pass for the
    // wrong reason — it would prove nothing about the grant check.
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

fn build_app_with(config_yaml: &str) -> (axum::Router, Arc<InMemoryJobStore>) {
    let store = Arc::new(InMemoryJobStore::new());
    let mut registry = ProcessRegistry::new();
    registry.register(Arc::new(EchoRunner));
    let lane = Arc::new(ProcessLane::new(
        registry,
        JobLedger::new(
            Arc::clone(&store) as Arc<dyn JobStore>,
            Duration::from_secs(60),
        ),
    ));
    let app = tellurion_processes::router()
        .layer(axum::Extension(lane))
        .with_state(build_ctx(config_yaml));
    (app, store)
}

fn build_app() -> (axum::Router, Arc<InMemoryJobStore>) {
    build_app_with(CONFIG)
}

async fn get(app: &axum::Router, uri: &str) -> Response {
    app.clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn delete(app: &axum::Router, uri: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn post(app: &axum::Router, uri: &str, body: Value, headers: &[(&str, &str)]) -> Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    app.clone()
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

async fn json_body(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn the_process_list_carries_exactly_what_this_binary_registered() {
    let (app, _) = build_app();
    let response = get(&app, "/processes").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let processes = body["processes"].as_array().unwrap();
    assert_eq!(processes.len(), 1);
    assert_eq!(processes[0]["id"], "echo");
    assert_eq!(processes[0]["version"], "1.0.0");
    // Requirement 25 reads these to decide a `Prefer`-less request's mode, so
    // an over-generous declaration would make the server's own answer wrong.
    assert_eq!(
        processes[0]["jobControlOptions"],
        json!(["async-execute", "dismiss"])
    );
    assert!(body["links"].as_array().unwrap().iter().any(|link| {
        link["rel"] == "self" && link["href"].as_str().unwrap().ends_with("/processes")
    }));
}

#[tokio::test]
async fn configured_public_base_is_used_for_process_links_and_job_location() {
    let config = format!(
        "{CONFIG}\nserver: {{ public_base_url: 'https://maps.example.test/tellurion/' }}\n"
    );
    let (app, _) = build_app_with(&config);

    let list = json_body(get(&app, "/processes").await).await;
    assert_eq!(
        list["links"][0]["href"],
        "https://maps.example.test/tellurion/processes"
    );
    assert_eq!(
        list["processes"][0]["links"][0]["href"],
        "https://maps.example.test/tellurion/processes/echo"
    );

    let response = post(
        &app,
        "/processes/echo/execution",
        json!({"inputs": {}}),
        &[],
    )
    .await;
    let location = response
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(location.starts_with("https://maps.example.test/tellurion/jobs/"));
    let body = json_body(response).await;
    assert!(body["links"].as_array().unwrap().iter().all(|link| {
        link["href"]
            .as_str()
            .unwrap()
            .starts_with("https://maps.example.test/tellurion/jobs/")
    }));
}

#[tokio::test]
async fn an_unknown_process_is_the_standards_own_no_such_process_exception() {
    let (app, _) = build_app();
    let response = get(&app, "/processes/nope").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap(),
        "application/problem+json"
    );
    let body = json_body(response).await;
    assert_eq!(
        body["type"],
        "http://www.opengis.net/def/exceptions/ogcapi-processes-1/1.0/no-such-process"
    );
}

/// Requirement 34 (`/req/core/process-execute-success-async`): `201`, a
/// `Location` header naming the job, and a `statusInfo` body. Not `202` — the
/// issue's prose says `202`, clause A and Table 11 both say `201`.
#[tokio::test]
async fn an_execute_request_creates_a_job_and_names_it_in_location() {
    let (app, store) = build_app();
    let response = post(
        &app,
        "/processes/echo/execution",
        json!({"inputs": {}}),
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let location = response
        .headers()
        .get(header::LOCATION)
        .expect("Requirement 34 clause B requires a Location header")
        .to_str()
        .unwrap()
        .to_string();
    // No `Prefer` was sent, so no `Preference-Applied` is claimed.
    assert!(response.headers().get("preference-applied").is_none());

    let body = json_body(response).await;
    assert_eq!(body["status"], "accepted");
    assert_eq!(body["type"], "process");
    assert_eq!(body["processID"], "echo");
    let job_id = body["jobID"].as_str().unwrap().to_string();
    assert!(location.ends_with(&format!("/jobs/{job_id}")));
    assert!(body["links"]
        .as_array()
        .unwrap()
        .iter()
        .any(|link| link["rel"] == "monitor"));
    // A job that has not started reports no `started`/`finished` rather than
    // a fabricated one, and no results link.
    assert!(body.get("started").is_none());
    assert!(body.get("finished").is_none());
    assert!(!body["links"]
        .as_array()
        .unwrap()
        .iter()
        .any(|link| link["rel"].as_str().unwrap().ends_with("/results")));

    // It really is in the ledger, in the mount's own scope.
    let scope = JobScope::new("public", "default");
    assert!(store.get(&scope, &job_id).await.unwrap().is_some());
}

#[tokio::test]
async fn a_respond_async_preference_is_reported_as_applied() {
    let (app, _) = build_app();
    let response = post(
        &app,
        "/processes/echo/execution",
        json!({"inputs": {}}),
        &[("prefer", "respond-async")],
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        response
            .headers()
            .get("preference-applied")
            .unwrap()
            .to_str()
            .unwrap(),
        "respond-async"
    );
}

/// Requirement 24 puts input validation on the execute request, not on the
/// job: a refused input must never become a job that exists only to fail.
#[tokio::test]
async fn refused_inputs_never_become_a_job() {
    let (app, store) = build_app();
    let response = post(
        &app,
        "/processes/echo/execution",
        json!({"inputs": {"reject": true}}),
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(store
        .claim_next(&["echo".to_string()], Duration::from_secs(60))
        .await
        .unwrap()
        .is_none());
}

/// The `Idempotency-Key` header is the whole enqueue-idempotency contract:
/// with it, a resubmission returns the first job; without it, two identical
/// submissions are two jobs.
#[tokio::test]
async fn an_idempotency_key_returns_the_job_it_already_created() {
    // One app instance throughout, so every submission below reaches the same
    // ledger — a second instance would have its own store and the dedup could
    // not be observed at all.
    let (app, _) = build_app();
    let first = post(
        &app,
        "/processes/echo/execution",
        json!({"inputs": {}}),
        &[("idempotency-key", "k")],
    )
    .await;
    let second = post(
        &app,
        "/processes/echo/execution",
        json!({"inputs": {}}),
        &[("idempotency-key", "k")],
    )
    .await;
    let first_id = json_body(first).await["jobID"]
        .as_str()
        .unwrap()
        .to_string();
    let second_id = json_body(second).await["jobID"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(first_id, second_id);

    let unkeyed_a = json_body(
        post(
            &app,
            "/processes/echo/execution",
            json!({"inputs": {}}),
            &[],
        )
        .await,
    )
    .await["jobID"]
        .as_str()
        .unwrap()
        .to_string();
    let unkeyed_b = json_body(
        post(
            &app,
            "/processes/echo/execution",
            json!({"inputs": {}}),
            &[],
        )
        .await,
    )
    .await["jobID"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(unkeyed_a, unkeyed_b);
}

#[tokio::test]
async fn an_unknown_job_is_the_standards_own_no_such_job_exception() {
    let (app, _) = build_app();
    for uri in ["/jobs/nope", "/jobs/nope/results"] {
        let response = get(&app, uri).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
        let body = json_body(response).await;
        assert_eq!(
            body["type"],
            "http://www.opengis.net/def/exceptions/ogcapi-processes-1/1.0/no-such-job",
            "{uri}"
        );
    }
}

/// Requirement 45 (`/req/core/job-results-exception/results-not-ready`): a
/// job that has not produced results answers `404` with its own exception
/// type — distinguishable, by that type, from a job that does not exist.
#[tokio::test]
async fn results_of_an_unfinished_job_are_not_ready_rather_than_missing() {
    let (app, _) = build_app();
    let created = post(
        &app,
        "/processes/echo/execution",
        json!({"inputs": {}}),
        &[],
    )
    .await;
    let job_id = json_body(created).await["jobID"]
        .as_str()
        .unwrap()
        .to_string();

    let response = get(&app, &format!("/jobs/{job_id}/results")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        json_body(response).await["type"],
        "http://www.opengis.net/def/exceptions/ogcapi-processes-1/1.0/result-not-ready"
    );
}

/// The whole round trip through the real ledger: submit, claim, finish,
/// read status, read results.
#[tokio::test]
async fn a_finished_job_serves_its_results_and_advertises_them() {
    let (app, store) = build_app();
    let created = post(
        &app,
        "/processes/echo/execution",
        json!({"inputs": {"a": 1}}),
        &[],
    )
    .await;
    let job_id = json_body(created).await["jobID"]
        .as_str()
        .unwrap()
        .to_string();

    let claimed = store
        .claim_next(&["echo".to_string()], Duration::from_secs(60))
        .await
        .unwrap()
        .expect("the submitted job is claimable");
    assert_eq!(claimed.job_id, job_id);
    store
        .finish(
            &job_id,
            JobOutcome::Succeeded(json!({ "echoed": {"a": 1} })),
        )
        .await
        .unwrap()
        .expect("the claimant records the outcome");

    let status = json_body(get(&app, &format!("/jobs/{job_id}")).await).await;
    assert_eq!(status["status"], "successful");
    assert!(status["started"].is_string());
    assert!(status["finished"].is_string());
    assert!(status["links"]
        .as_array()
        .unwrap()
        .iter()
        .any(|link| link["rel"].as_str().unwrap().ends_with("/results")));

    let results = get(&app, &format!("/jobs/{job_id}/results")).await;
    assert_eq!(results.status(), StatusCode::OK);
    assert_eq!(json_body(results).await, json!({ "echoed": {"a": 1} }));
}

/// Requirement 82 (`/req/dismiss/job-dismiss-success`): a dismissal answers
/// `200` with a `statusInfo` whose status is `dismissed`.
#[tokio::test]
async fn a_job_can_be_dismissed_while_it_is_still_in_play() {
    let (app, _) = build_app();
    let created = post(
        &app,
        "/processes/echo/execution",
        json!({"inputs": {}}),
        &[],
    )
    .await;
    let job_id = json_body(created).await["jobID"]
        .as_str()
        .unwrap()
        .to_string();

    let response = delete(&app, &format!("/jobs/{job_id}")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["status"], "dismissed");
}

/// A finished job is refused rather than reported as dismissed: the only ways
/// to answer Requirement 82's mandated `status: "dismissed"` for a
/// `successful` job are to lie in the response or to rewrite the ledger.
#[tokio::test]
async fn a_finished_job_is_refused_rather_than_reported_as_dismissed() {
    let (app, store) = build_app();
    let created = post(
        &app,
        "/processes/echo/execution",
        json!({"inputs": {}}),
        &[],
    )
    .await;
    let job_id = json_body(created).await["jobID"]
        .as_str()
        .unwrap()
        .to_string();
    store
        .claim_next(&["echo".to_string()], Duration::from_secs(60))
        .await
        .unwrap();
    store
        .finish(&job_id, JobOutcome::Succeeded(json!({})))
        .await
        .unwrap();

    let response = delete(&app, &format!("/jobs/{job_id}")).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    // The ledger still says `successful`.
    let status = json_body(get(&app, &format!("/jobs/{job_id}")).await).await;
    assert_eq!(status["status"], "successful");
}

/// A failed job's results are the failure, not an empty document — and the
/// runner's own message is not echoed to the client (it is logged, and the
/// job's own status document carries it for a caller already entitled to it).
#[tokio::test]
async fn a_failed_jobs_results_report_the_failure() {
    let (app, store) = build_app();
    let created = post(
        &app,
        "/processes/echo/execution",
        json!({"inputs": {}}),
        &[],
    )
    .await;
    let job_id = json_body(created).await["jobID"]
        .as_str()
        .unwrap()
        .to_string();
    store
        .claim_next(&["echo".to_string()], Duration::from_secs(60))
        .await
        .unwrap();
    store
        .finish(&job_id, JobOutcome::Failed("internal detail".to_string()))
        .await
        .unwrap();

    let response = get(&app, &format!("/jobs/{job_id}/results")).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = json_body(response).await;
    assert!(
        !body["detail"].as_str().unwrap().contains("internal detail"),
        "a runner's failure text must not be echoed to the client: {body}"
    );

    let status = json_body(get(&app, &format!("/jobs/{job_id}")).await).await;
    assert_eq!(status["status"], "failed");
    assert_eq!(status["message"], "internal detail");
}

/// `#34`: with `auth:` configured, a subject whose grants do not reach the
/// target collection's write lane cannot schedule a process against it — and
/// a read-only grant is not enough.
#[tokio::test]
async fn a_read_only_subject_cannot_schedule_a_process_against_a_collection() {
    let config = format!(
        "{CONFIG}\n{}",
        r#"
auth:
  bearer_tokens:
    - { token: reader-token, tenants: [public], roles: { public: [reader] } }
    - { token: writer-token, tenants: [public], roles: { public: [writer] } }
policy:
  roles:
    - name: reader
      grants:
        - scope: { collections: [demo] }
          lanes: [features]
    - name: writer
      grants:
        - scope: { collections: [demo] }
          lanes: [features, write]
"#
    );
    let (app, _) = build_app_with(&config);
    let body = json!({"inputs": {"collection": "demo"}});

    let refused = post(
        &app,
        "/processes/echo/execution",
        body.clone(),
        &[("authorization", "Bearer reader-token")],
    )
    .await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);

    let allowed = post(
        &app,
        "/processes/echo/execution",
        body,
        &[("authorization", "Bearer writer-token")],
    )
    .await;
    assert_eq!(allowed.status(), StatusCode::CREATED);
}
