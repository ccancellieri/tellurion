//! Handlers for the OGC API — Processes surface (`#182`).
//!
//! Every one of them reaches storage through the [`ProcessLane`] this root is
//! layered with — never through a concrete driver, the same DB-free rule every
//! other protocol crate in this workspace follows. The lane carries the two
//! halves `#182` requires together: what this binary can execute
//! (`ProcessRegistry`) and where jobs are durably recorded (`JobLedger`). A
//! deployment missing either never gets this root mounted at all, so no
//! handler here has to answer "what if there is no ledger".
//!
//! Every request runs under a `/{tenant}/processes/catalogs/{catalog}` mount;
//! `tenant`/`catalog` path parameters carry EXTERNAL ids exactly as the client
//! typed them and are resolved to internal ones through
//! `AppContext::current().resolver`, exactly as in
//! `tellurion_features::handlers`. Response bodies echo external ids only —
//! the job scope stored in the ledger is internal ids, and never reaches the
//! wire.
//!
//! # Execution is asynchronous, always
//!
//! Every process this slice ships declares `jobControlOptions:
//! ["async-execute", "dismiss"]`, which makes OGC API — Processes Requirement
//! 25 (`/req/core/process-execute-default-execution-mode`) clause A and
//! Requirement 26 (`/req/core/process-execute-auto-execution-mode`) clause A
//! agree with each other and with this lane: "The server SHALL respond
//! asynchronously if, according to the job control options in the process
//! description, the process can only be executed asynchronously." A `Prefer:
//! respond-async` is therefore honoured trivially and a `Prefer: wait` cannot
//! be, which is why `Preference-Applied` (Recommendation 14) is emitted only
//! for the token actually honoured.
//!
//! Note the status code: `201`, not the `202` `#182`'s own issue text
//! suggests. Requirement 34 (`/req/core/process-execute-success-async`) clause
//! A says "A successful execution of the operation SHALL be reported as a
//! response with a HTTP status code 201", and Table 11 says the same. The
//! Standard wins over the issue's prose.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Extension, OriginalUri, Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use tellurion_core::policy::{self, PolicyDecision, ResourceContext};
use tellurion_core::timefmt::format_rfc3339_millis;
use tellurion_core::{
    AppContext, Credential, Error as CoreError, JobRecord, JobScope, JobStatus, JobSubmission,
    ProcessLane, ProcessTarget, RateCharge, RateCounter, RateVerdict,
};

use crate::conformance::{
    EXCEPTION_NO_SUCH_JOB, EXCEPTION_NO_SUCH_PROCESS, EXCEPTION_RESULT_NOT_READY, JOB_TYPE_PROCESS,
    JSON_MEDIA_TYPE, REL_MONITOR, REL_RESULTS, REL_SELF,
};
use crate::model::{Link, ProcessList, ProcessSummary, StatusInfo};
use crate::problem::{ogc_not_found, ApiError};

/// Mount-less fallbacks, so this crate's own tests can exercise a handler
/// without standing up the server's `/{tenant}/processes/catalogs/{catalog}`
/// nesting — identical convention to `tellurion_records::handlers`.
pub const DEFAULT_TENANT: &str = "public";
pub const DEFAULT_CATALOG: &str = "default";

/// The opt-in enqueue-idempotency header (`JobSubmission::dedup_key`).
///
/// A widely deployed convention rather than anything OGC API — Processes
/// defines — the Standard has no idempotency concept at all — and the same one
/// this workspace's control plane already models as
/// `ControlChangeset::idempotency_key`. Opt-in on purpose: absent, two
/// identical submissions are two jobs, because in general two identical
/// submissions are two deliberate requests. Reusing the execute request body
/// for this instead would mean inventing a member `execute.yaml` does not
/// define.
const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";

/// The `Prefer` token this lane can honour (IETF RFC 7240, cited by clause
/// 7.11.2.3). Spelled as a literal header name because `http::header` has no
/// constant for `Prefer` — it is an RFC 7240 header, not one of the core HTTP
/// set.
const PREFER_HEADER: &str = "prefer";
const PREFER_RESPOND_ASYNC: &str = "respond-async";
/// RFC 7240's own response header naming the preferences that were honoured.
const PREFERENCE_APPLIED_HEADER: &str = "preference-applied";

/// The execute request body — `execute.yaml` (Figure 9), narrowed to the one
/// member this slice reads.
///
/// `outputs`, `response` and `subscriber` are deliberately absent rather than
/// accepted-and-ignored: `outputs`/`response` only shape a *synchronous* or
/// raw-valued answer (Table 11), which this lane never produces, and
/// `subscriber` belongs to the Callback requirements class this crate
/// withholds. `#[serde(default)]` on `inputs` makes a bodyless `{}` legal,
/// which Requirement 27 (`/req/core/process-execute-default-outputs`) treats
/// as "all defined outputs".
#[derive(Debug, Deserialize, Default)]
pub struct ExecuteRequest {
    #[serde(default)]
    pub inputs: Value,
}

fn tenant_of(params: &HashMap<String, String>) -> String {
    params
        .get("tenant")
        .cloned()
        .unwrap_or_else(|| DEFAULT_TENANT.to_string())
}

fn catalog_of(params: &HashMap<String, String>) -> String {
    params
        .get("catalog")
        .cloned()
        .unwrap_or_else(|| DEFAULT_CATALOG.to_string())
}

fn require_param(params: &HashMap<String, String>, name: &str) -> Result<String, ApiError> {
    params
        .get(name)
        .cloned()
        .ok_or(CoreError::NotFound)
        .map_err(ApiError::from)
}

/// Resolves the mount's `(tenant, catalog)` to the INTERNAL ids a job is
/// scoped by. An unresolvable segment is the same `404` every other protocol
/// crate answers with.
async fn resolve_scope(
    ctx: &AppContext,
    params: &HashMap<String, String>,
) -> Result<JobScope, ApiError> {
    let state = ctx.current();
    let tenant_id = state.resolver.resolve_tenant(&tenant_of(params)).await?;
    let catalog_id = state
        .resolver
        .resolve_catalog(&tenant_id, &catalog_of(params))
        .await?;
    Ok(JobScope::new(tenant_id, catalog_id))
}

/// Mirrors `tellurion-server::app`'s own `extract_credential` (duplicated per
/// protocol crate, not shared — `tellurion-core` stays framework-free; see
/// `auth.rs`'s module doc).
fn extract_credential(headers: &HeaderMap) -> Credential {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return Credential::None;
    };
    let Ok(value) = value.to_str() else {
        return Credential::None;
    };
    match value.strip_prefix("Bearer ") {
        Some(token) if !token.is_empty() => Credential::Bearer(token.to_string()),
        _ => Credential::None,
    }
}

/// Whether the request asserts `Prefer: respond-async` (RFC 7240 allows a
/// comma-separated list of preferences, each optionally parameterised).
fn prefers_respond_async(headers: &HeaderMap) -> bool {
    headers
        .get_all(PREFER_HEADER)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|token| {
            token
                .split(';')
                .next()
                .map(str::trim)
                .is_some_and(|token| token.eq_ignore_ascii_case(PREFER_RESPOND_ASYNC))
        })
}

/// The `#34` policy checkpoint for a process submission.
///
/// Only reached when the runner declared a [`ProcessTarget`] — a process that
/// touches no collection has no per-collection grant to check, and the tenant
/// trust boundary (`app::enforce_tenant_auth`, which wraps every route under
/// `/{tenant}`) is then the whole authorization story. When a target IS
/// declared, the grant that governs the process is the grant that governs that
/// collection through that lane: scheduling an index rebuild against a
/// collection is exactly as consequential as writing to it, so a runner
/// declares `PolicyLane::Write` and a read-only subject is refused.
///
/// The returned grant filter is deliberately discarded: a row filter is
/// meaningful for a query, not for "may this subject schedule this job", and
/// silently running a job over a filtered subset would produce a result that
/// looks complete and is not. A target whose lane the subject may only reach
/// under a filter is therefore treated as authorized for the whole job — which
/// is why `lane_supports_filter` is passed `false`, making
/// `policy::authorize_resource` refuse rather than narrow.
async fn authorize_target(
    ctx: &AppContext,
    rate_counter: &dyn RateCounter,
    headers: &HeaderMap,
    scope: &JobScope,
    target: &ProcessTarget,
) -> Result<(), ApiError> {
    let state = ctx.current();
    let collection_id = state
        .resolver
        .resolve_collection(&scope.catalog, &target.collection)
        .await?;
    let Some(authorizer) = state.authorizer.as_ref() else {
        return Ok(());
    };
    let credential = extract_credential(headers);
    let subject = authorizer.subject(&credential).await;
    let visibility = state
        .router
        .effective_visibility(&collection_id)
        .cloned()
        .unwrap_or_default();
    let resource = ResourceContext {
        tenant_id: &scope.tenant,
        catalog_id: &scope.catalog,
        collection_id: &collection_id,
        lane: target.lane,
        visibility: &visibility,
    };
    match policy::authorize_resource(&state.config, &resource, &subject, false)? {
        PolicyDecision::Allow { .. } => {}
        PolicyDecision::Deny => return Err(crate::problem::policy_denied(&credential)),
    }
    if let RateVerdict::Refused(refusal) = policy::enforce_rate_limits(
        &state.config,
        &resource,
        &subject,
        Some(rate_counter),
        RateCharge::Charge,
    )
    .await
    {
        return Err(crate::problem::policy_rate_limited(&refusal));
    }
    Ok(())
}

fn set_content_type(response: &mut Response, media_type: &'static str) {
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(media_type));
}

/// The root of this Processes mount, derived from the request URI by stripping
/// the resource path off it — the same "build every sibling href off one
/// normalized self root" convention `landing::protocol_landing` uses.
fn mount_root(uri_path: &str, suffix: &str) -> String {
    uri_path
        .trim_end_matches('/')
        .strip_suffix(suffix)
        .unwrap_or("")
        .trim_end_matches('/')
        .to_string()
}

fn process_summary(root: &str, description: &tellurion_core::ProcessDescription) -> ProcessSummary {
    ProcessSummary {
        id: description.id.clone(),
        version: description.version.clone(),
        title: description.title.clone(),
        description: description.description.clone(),
        job_control_options: description
            .job_control_options
            .iter()
            .map(|option| option.as_str().to_string())
            .collect(),
        links: vec![Link::new(
            format!("{root}/processes/{}", description.id),
            REL_SELF,
            JSON_MEDIA_TYPE,
        )],
    }
}

/// Projects a ledger row onto `statusInfo.yaml`.
///
/// The `results` link is present only for a job whose results exist: a link to
/// `/results` on a running job would resolve to the `404` Requirement 45
/// mandates, and advertising a link the server knows is dead is the same
/// dishonesty the tenant directory avoids for a disabled protocol root.
fn status_info(root: &str, record: &JobRecord) -> StatusInfo {
    let self_href = format!("{root}/jobs/{}", record.job_id);
    let mut links = vec![Link::new(&self_href, REL_SELF, JSON_MEDIA_TYPE)];
    if record.status == JobStatus::Successful {
        links.push(Link::new(
            format!("{self_href}/results"),
            REL_RESULTS,
            JSON_MEDIA_TYPE,
        ));
    }
    StatusInfo {
        job_id: record.job_id.clone(),
        process_id: record.process_id.clone(),
        type_: JOB_TYPE_PROCESS,
        status: record.status.as_str(),
        message: record.message.clone(),
        created: format_rfc3339_millis(record.created),
        started: record.started.map(format_rfc3339_millis),
        finished: record.finished.map(format_rfc3339_millis),
        updated: format_rfc3339_millis(record.updated),
        links,
    }
}

/// `GET /processes` — Requirement 8 (`/req/core/process-list`) and
/// Requirement 11 (`/req/core/process-list-success`).
///
/// The whole registered set, alphabetically, with no `limit`: the list is
/// bounded by what this binary was compiled with, not by anything a client can
/// page through. That is also precisely why the Core conformance class is
/// withheld — Requirement 9 makes `limit` a SHALL. See `crate::conformance`.
pub async fn list_processes(
    Extension(lane): Extension<Arc<ProcessLane>>,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let root = mount_root(uri.path(), "/processes");
    let processes = lane
        .registry
        .descriptions()
        .iter()
        .map(|description| process_summary(&root, description))
        .collect();
    let body = ProcessList {
        processes,
        // Requirement 12 (`/req/core/pl-links`) clause A asks for `self` and
        // for `alternate` in every other media type the service supports.
        // This server supports exactly one, so there is no alternate to link.
        links: vec![Link::new(
            format!("{root}/processes"),
            REL_SELF,
            JSON_MEDIA_TYPE,
        )],
    };
    let mut response = Json(body).into_response();
    set_content_type(&mut response, JSON_MEDIA_TYPE);
    response
}

/// `GET /processes/{processID}` — Requirement 13 (`/req/core/process`) and
/// Requirement 14 (`/req/core/process-success`). An unknown id is the `404`
/// Requirement 15 mandates, carrying its `no-such-process` exception type.
pub async fn get_process(
    Extension(lane): Extension<Arc<ProcessLane>>,
    Path(params): Path<HashMap<String, String>>,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    let process_id = require_param(&params, "processID")?;
    let root = mount_root(uri.path(), &format!("/processes/{process_id}"));
    let runner = lane
        .registry
        .get(&process_id)
        .ok_or_else(|| no_such_process(&process_id))?;
    let mut response = Json(process_summary(&root, &runner.description())).into_response();
    set_content_type(&mut response, JSON_MEDIA_TYPE);
    Ok(response)
}

fn no_such_process(process_id: &str) -> ApiError {
    ogc_not_found(
        EXCEPTION_NO_SUCH_PROCESS,
        "NoSuchProcess",
        format!("this server offers no process '{process_id}'"),
    )
}

fn no_such_job(job_id: &str) -> ApiError {
    ogc_not_found(
        EXCEPTION_NO_SUCH_JOB,
        "NoSuchJob",
        format!("this catalog has no job '{job_id}'"),
    )
}

/// `POST /processes/{processID}/execution` — Requirement 16
/// (`/req/core/process-execute-op`), answered asynchronously per Requirement
/// 34: `201`, a `Location` header naming the created job, and a `statusInfo`
/// body.
///
/// The order of operations is the contract: resolve the process, validate the
/// inputs, authorize the target, and only then record the job. A job written
/// to the ledger before the authorization check would be work an unauthorized
/// caller had already scheduled, and a `201` returned before the ledger write
/// would promise a job that was never recorded — which is exactly what the
/// named `JobsTableMissing` refusal exists to prevent an operator from
/// experiencing as a silent black hole.
pub async fn execute_process(
    State(ctx): State<Arc<AppContext>>,
    Extension(lane): Extension<Arc<ProcessLane>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Option<Json<ExecuteRequest>>,
) -> Result<Response, ApiError> {
    let process_id = require_param(&params, "processID")?;
    let root = mount_root(uri.path(), &format!("/processes/{process_id}/execution"));
    let scope = resolve_scope(&ctx, &params).await?;
    let runner = lane
        .registry
        .get(&process_id)
        .ok_or_else(|| no_such_process(&process_id))?;

    let inputs = body.map(|Json(request)| request.inputs).unwrap_or_default();
    runner.validate_inputs(&inputs)?;
    if let Some(target) = runner.target(&inputs) {
        authorize_target(&ctx, ctx.rate_counter.as_ref(), &headers, &scope, &target).await?;
    }

    let submission = JobSubmission {
        job_id: uuid::Uuid::new_v4().to_string(),
        process_id: process_id.clone(),
        scope,
        inputs,
        dedup_key: headers
            .get(IDEMPOTENCY_KEY_HEADER)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    };
    let record = lane.ledger.store.enqueue(&submission).await?;

    let location = format!("{root}/jobs/{}", record.job_id);
    let mut body = status_info(&root, &record);
    // `#182` asks for a `rel="monitor"` link alongside the `Location` header;
    // the Standard itself only names that relation on the synchronous path
    // (Requirement 33), so this is an addition a client may use, never a
    // replacement for the header Requirement 34 clause B requires.
    body.links
        .push(Link::new(&location, REL_MONITOR, JSON_MEDIA_TYPE));

    let mut response = (StatusCode::CREATED, Json(body)).into_response();
    set_content_type(&mut response, JSON_MEDIA_TYPE);
    if let Ok(value) = HeaderValue::from_str(&location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    // Recommendation 14 (`/rec/core/process-execute-preference-applied`):
    // state which `Prefer` tokens were honoured. Only emitted when the client
    // actually asserted one — announcing a preference nobody expressed would
    // be noise, and `respond-async` is honoured here whether or not it was
    // asked for, because these processes can only run asynchronously.
    if prefers_respond_async(&headers) {
        response.headers_mut().insert(
            PREFERENCE_APPLIED_HEADER,
            HeaderValue::from_static(PREFER_RESPOND_ASYNC),
        );
    }
    Ok(response)
}

/// `GET /jobs/{jobID}` — Requirement 35 (`/req/core/job`) and Requirement 36
/// (`/req/core/job-success`). An unknown job is the `404` Requirement 37
/// mandates, with its `no-such-job` exception type; so is a job that exists
/// under a different catalog, which from here is the same thing.
pub async fn get_job(
    State(ctx): State<Arc<AppContext>>,
    Extension(lane): Extension<Arc<ProcessLane>>,
    Path(params): Path<HashMap<String, String>>,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    let job_id = require_param(&params, "jobID")?;
    let root = mount_root(uri.path(), &format!("/jobs/{job_id}"));
    let scope = resolve_scope(&ctx, &params).await?;
    let record = lane
        .ledger
        .store
        .get(&scope, &job_id)
        .await?
        .ok_or_else(|| no_such_job(&job_id))?;
    let mut response = Json(status_info(&root, &record)).into_response();
    set_content_type(&mut response, JSON_MEDIA_TYPE);
    Ok(response)
}

/// `GET /jobs/{jobID}/results` — Requirement 38 (`/req/core/job-results`).
///
/// Three distinct refusals, each the one the Standard names:
/// - an unknown job is a `404` with `no-such-job` (Requirement 44);
/// - a job still accepted, running or dismissed is a `404` with
///   `result-not-ready` (Requirement 45 — a `404` rather than a `409`, which
///   reads oddly and is nonetheless exactly what clause 7.13.3 specifies);
/// - a failed job answers with the failure itself (Requirement 46: "a HTTP
///   error code that corresponds to the reason of the failure ... The type of
///   the exception SHALL correspond to the reason of the failure"). This lane
///   has one honest thing to say about a failed job — the message its runner
///   recorded — so it answers `500` with that message rather than inventing a
///   more specific status from text it did not parse.
pub async fn get_job_results(
    State(ctx): State<Arc<AppContext>>,
    Extension(lane): Extension<Arc<ProcessLane>>,
    Path(params): Path<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let job_id = require_param(&params, "jobID")?;
    let scope = resolve_scope(&ctx, &params).await?;
    let record = lane
        .ledger
        .store
        .get(&scope, &job_id)
        .await?
        .ok_or_else(|| no_such_job(&job_id))?;

    match record.status {
        JobStatus::Successful => {
            let results = record.results.unwrap_or_else(|| json!({}));
            let mut response = Json(results).into_response();
            set_content_type(&mut response, JSON_MEDIA_TYPE);
            Ok(response)
        }
        JobStatus::Failed => Err(ApiError::from(CoreError::Storage(Box::new(JobFailure(
            record
                .message
                .unwrap_or_else(|| "the job failed".to_string()),
        ))))),
        JobStatus::Accepted | JobStatus::Running | JobStatus::Dismissed => Err(ogc_not_found(
            EXCEPTION_RESULT_NOT_READY,
            "ResultNotReady",
            format!(
                "job '{job_id}' is '{}'; it has produced no results",
                record.status
            ),
        )),
    }
}

/// A failed job's recorded message, wrapped so it can travel as
/// `CoreError::Storage` and pick up that variant's `500` mapping without this
/// crate inventing a second error type hierarchy.
///
/// `Problem::from_core_error` deliberately does NOT echo a `Storage` error's
/// text to the client (it logs it instead), which is the right call here too:
/// a runner's failure message is written by server-side code and may name
/// internal tables or ids. The job's own `statusInfo` still carries the
/// message, where a caller who can already read the job is entitled to it.
#[derive(Debug)]
struct JobFailure(String);

impl std::fmt::Display for JobFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "job failed: {}", self.0)
    }
}

impl std::error::Error for JobFailure {}

/// `DELETE /jobs/{jobID}` — Requirement 81 (`/req/dismiss/job-dismiss-op`),
/// answered per Requirement 82 with `200` and a `statusInfo` whose status is
/// `dismissed`.
///
/// A job that has ALREADY finished is refused with `409` rather than reported
/// as dismissed: Requirement 82's response must carry `status: "dismissed"`,
/// and the only ways to produce that for a `successful` job are to lie in the
/// response or to rewrite the ledger. Neither is acceptable, and the Standard
/// gives no requirement covering the case — so the honest answer is that the
/// operation could not be applied. Requirement 81 is still met: the operation
/// is supported at that path.
pub async fn dismiss_job(
    State(ctx): State<Arc<AppContext>>,
    Extension(lane): Extension<Arc<ProcessLane>>,
    Path(params): Path<HashMap<String, String>>,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    let job_id = require_param(&params, "jobID")?;
    let root = mount_root(uri.path(), &format!("/jobs/{job_id}"));
    let scope = resolve_scope(&ctx, &params).await?;
    let record = lane
        .ledger
        .store
        .dismiss(&scope, &job_id)
        .await?
        .ok_or_else(|| no_such_job(&job_id))?;
    if record.status != JobStatus::Dismissed {
        return Err(ApiError::from(CoreError::Conflict(format!(
            "job '{job_id}' already finished with status '{}' and cannot be dismissed",
            record.status
        ))));
    }
    let mut response = Json(status_info(&root, &record)).into_response();
    set_content_type(&mut response, JSON_MEDIA_TYPE);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn record(status: JobStatus) -> JobRecord {
        JobRecord {
            job_id: "job-1".to_string(),
            process_id: "index-rebuild".to_string(),
            scope: JobScope::new("t", "c"),
            status,
            message: None,
            inputs: json!({}),
            results: None,
            created: UNIX_EPOCH + Duration::from_secs(1),
            started: None,
            finished: None,
            updated: UNIX_EPOCH + Duration::from_secs(2),
            attempts: 0,
        }
    }

    /// A results link is advertised only when results exist. A `rel="results"`
    /// on a running job would point at the `404` Requirement 45 mandates.
    #[test]
    fn only_a_successful_job_advertises_a_results_link() {
        for status in JobStatus::ALL {
            let info = status_info("/public/processes/catalogs/default", &record(status));
            let has_results = info.links.iter().any(|link| link.rel == REL_RESULTS);
            assert_eq!(
                has_results,
                status == JobStatus::Successful,
                "{status} advertised a results link: {:?}",
                info.links
            );
        }
    }

    /// The three required `statusInfo` members are always present and carry
    /// the Standard's own values.
    #[test]
    fn a_status_document_always_carries_its_three_required_members() {
        let info = status_info("/root", &record(JobStatus::Accepted));
        assert_eq!(info.job_id, "job-1");
        assert_eq!(info.status, "accepted");
        assert_eq!(info.type_, "process");
        // Never fabricated: a job that has not started has no `started`.
        assert!(info.started.is_none());
        assert!(info.finished.is_none());
    }

    /// RFC 7240 allows a list of preferences, each with parameters, in any
    /// case. All four shapes must be recognized, and an unrelated preference
    /// must not be mistaken for this one.
    #[test]
    fn respond_async_is_recognized_in_every_rfc_7240_shape() {
        let cases = [
            ("respond-async", true),
            ("Respond-Async", true),
            ("wait=10, respond-async", true),
            ("respond-async; foo=bar", true),
            ("wait=10", false),
            ("respond-asynchronously", false),
            ("", false),
        ];
        for (value, expected) in cases {
            let mut headers = HeaderMap::new();
            headers.insert(PREFER_HEADER, HeaderValue::from_str(value).unwrap());
            assert_eq!(
                prefers_respond_async(&headers),
                expected,
                "Prefer: {value:?}"
            );
        }
        assert!(!prefers_respond_async(&HeaderMap::new()));
    }

    /// Every href on this root is built off the mount, not off a hardcoded
    /// prefix, so the same handler serves any `(tenant, catalog)` pair.
    #[test]
    fn hrefs_are_derived_from_the_actual_mount() {
        assert_eq!(
            mount_root(
                "/tenant-a/processes/catalogs/cat-b/jobs/job-1",
                "/jobs/job-1"
            ),
            "/tenant-a/processes/catalogs/cat-b"
        );
        assert_eq!(
            mount_root("/t/processes/catalogs/c/processes", "/processes"),
            "/t/processes/catalogs/c"
        );
        // A path that does not end in the expected suffix (only reachable if
        // the route and the handler disagreed) yields an empty root rather
        // than a panic or a half-stripped path.
        assert_eq!(mount_root("/unexpected", "/jobs/job-1"), "");
    }
}
