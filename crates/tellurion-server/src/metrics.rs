//! Prometheus metrics: an `http_request_duration_seconds` histogram recorded
//! by the outer request boundary, one privacy-bounded slow-request event,
//! and a background-sampled
//! `process_resident_memory_bytes` gauge (design rule: memory is observed,
//! not assumed). `/metrics` renders whatever the installed recorder has
//! accumulated.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use axum::extract::{MatchedPath, RawPathParams, Request, State};
use axum::http::{header, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use tellurion_core::config::MetricCollectionRef;
use tellurion_core::{resolve_effective_settings, AppContext, ContextState, SettingsDecl};

const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4";

/// Installs the process-global Prometheus recorder. Must be called exactly
/// once, before any `metrics::*!` call site runs.
pub fn install_recorder() -> anyhow::Result<PrometheusHandle> {
    Ok(PrometheusBuilder::new().install_recorder()?)
}

pub async fn metrics_handler(Extension(handle): Extension<PrometheusHandle>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, PROMETHEUS_CONTENT_TYPE)],
        handle.render(),
    )
}

/// Observes every request at the outer service boundary. Route templates and
/// resolved public identifiers keep metric cardinality bounded; raw paths,
/// query strings, credentials, internal ids, and backend errors never leave
/// this function.
pub async fn observe_request(
    State(ctx): State<std::sync::Arc<AppContext>>,
    matched_path: Option<MatchedPath>,
    raw_params: Result<RawPathParams, axum::extract::rejection::RawPathParamsRejection>,
    request: Request,
    next: Next,
) -> Response {
    let method = normalize_method(request.method()).to_string();
    let request_id = crate::request_id::current_id(request.headers());
    let route = matched_path
        .as_ref()
        .map(|path| path.as_str().to_string())
        .unwrap_or_else(|| "unmatched".to_string());
    let state = ctx.current();
    let start = Instant::now();
    let ((response, identity), phases) = tellurion_core::observability::scope_request(async {
        let identity =
            resolve_identity(&state, matched_path.as_ref(), raw_params.as_ref().ok()).await;
        let response = next.run(request).await;
        (response, identity)
    })
    .await;
    let elapsed = start.elapsed();
    let status = response.status().as_u16().to_string();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    let lane = classify_lane(matched_path.as_ref().map(MatchedPath::as_str), content_type);

    metrics::histogram!(
        "http_request_duration_seconds",
        "method" => method.clone(),
        "path" => route.clone(),
        "status" => status.clone(),
        "lane" => lane,
        "tenant" => identity.tenant_metric.clone(),
        "collection" => identity.collection_metric.clone(),
    )
    .record(elapsed.as_secs_f64());

    if is_slow(elapsed, identity.slow_threshold) {
        emit_slow_request(
            &method,
            &route,
            lane,
            &status,
            &request_id,
            &identity.tenant_log,
            &identity.catalog_log,
            &identity.collection_log,
            elapsed,
            phases,
        );
    }

    response
}

fn normalize_method(method: &Method) -> &'static str {
    match method.as_str() {
        "GET" => "GET",
        "HEAD" => "HEAD",
        "OPTIONS" => "OPTIONS",
        "POST" => "POST",
        "PUT" => "PUT",
        "PATCH" => "PATCH",
        "DELETE" => "DELETE",
        "TRACE" => "TRACE",
        "CONNECT" => "CONNECT",
        _ => "other",
    }
}

fn classify_lane(route: Option<&str>, content_type: Option<&str>) -> &'static str {
    let Some(route) = route else {
        return "unmatched";
    };
    if route.contains("/{tenant}/features/") {
        "features"
    } else if route.contains("/{tenant}/3dtiles/") {
        "places3d"
    } else if route.contains("/{tenant}/styles/") {
        "styles"
    } else if route.contains("/{tenant}/stac/") {
        "stac"
    } else if route.contains("/{tenant}/tiles/") {
        if route.contains("/styles/{styleId}/map/tiles/") {
            "styled_png"
        } else if route.ends_with("/{tileMatrix}/{tileRow}/{tileCol}") {
            match content_type.unwrap_or_default().split(';').next() {
                Some("application/vnd.mapbox-vector-tile") => "mvt",
                Some("image/png") => "png",
                _ => "tiles",
            }
        } else {
            "tiles"
        }
    } else {
        "control"
    }
}

fn collection_metric_label(
    allowlist: &[MetricCollectionRef],
    collection: Option<(&str, &str, &str)>,
) -> String {
    let Some((tenant, catalog, collection)) = collection else {
        return "none".to_string();
    };
    if allowlist.iter().any(|entry| {
        entry.tenant == tenant && entry.catalog == catalog && entry.collection == collection
    }) {
        format!("{tenant}/{catalog}/{collection}")
    } else {
        "other".to_string()
    }
}

fn tenant_metric_label(allowlist: &[String], tenant: Option<&str>) -> String {
    let Some(tenant) = tenant else {
        return "unknown".to_string();
    };
    if allowlist.iter().any(|entry| entry == tenant) {
        tenant.to_string()
    } else {
        "other".to_string()
    }
}

fn is_slow(elapsed: Duration, threshold: Duration) -> bool {
    elapsed > threshold
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[allow(clippy::too_many_arguments)]
fn emit_slow_request(
    method: &str,
    route: &str,
    lane: &str,
    status: &str,
    request_id: &str,
    tenant: &str,
    catalog: &str,
    collection: &str,
    elapsed: Duration,
    phases: tellurion_core::observability::PhaseSnapshot,
) {
    tracing::warn!(
        event = "slow_request",
        method,
        route,
        lane,
        status,
        request_id,
        tenant,
        catalog,
        collection,
        elapsed_ms = duration_ms(elapsed),
        routing_ms = duration_ms(phases.routing()),
        query_ms = duration_ms(phases.query()),
        cache_ms = duration_ms(phases.cache()),
        encode_ms = duration_ms(phases.encode(elapsed)),
        "slow request"
    );
}

struct RequestIdentity {
    tenant_metric: String,
    collection_metric: String,
    tenant_log: String,
    catalog_log: String,
    collection_log: String,
    slow_threshold: Duration,
}

/// Named profiles (`#111`), keyed by id — the same lookup
/// `Router::build_from_snapshot`/`config_view::effective_config_view`
/// build, needed here too since this module resolves the slow-request
/// threshold independently of `Router`'s own materialized maps whenever a
/// request can't be tied to a routed collection (see both call sites
/// below).
fn profiles_by_id(state: &ContextState) -> HashMap<&str, &SettingsDecl> {
    state
        .config
        .profiles
        .iter()
        .map(|profile| (profile.id.as_str(), &profile.settings))
        .collect()
}

impl RequestIdentity {
    fn control(state: &ContextState) -> Self {
        let threshold = resolve_effective_settings(
            &SettingsDecl::default(),
            &SettingsDecl::default(),
            &SettingsDecl::default(),
            &state.config.settings,
            &profiles_by_id(state),
        )
        .slow_request_ms;
        Self {
            tenant_metric: "none".to_string(),
            collection_metric: "none".to_string(),
            tenant_log: "none".to_string(),
            catalog_log: "none".to_string(),
            collection_log: "none".to_string(),
            slow_threshold: Duration::from_millis(threshold),
        }
    }
}

#[cfg(test)]
pub(crate) fn current_control_slow_threshold(ctx: &AppContext) -> Duration {
    RequestIdentity::control(&ctx.current()).slow_threshold
}

async fn resolve_identity(
    state: &ContextState,
    matched_path: Option<&MatchedPath>,
    raw_params: Option<&RawPathParams>,
) -> RequestIdentity {
    let Some(route) = matched_path.map(MatchedPath::as_str) else {
        return RequestIdentity::control(state);
    };
    if !route.contains("{tenant}") {
        return RequestIdentity::control(state);
    }

    let param = |name: &str| {
        raw_params.and_then(|params| {
            params
                .iter()
                .find_map(|(key, value)| (key == name).then(|| value.to_string()))
        })
    };
    let tenant_raw = param("tenant");
    let catalog_raw = param("catalog");
    let collection_raw = param("cid");

    let mut tenant_internal = None;
    let mut tenant_external = None;
    if let Some(raw) = tenant_raw.as_deref() {
        if let Ok(internal) = state.resolver.resolve_tenant(raw).await {
            tenant_external = state
                .resolver
                .tenant_external_id(&internal)
                .map(str::to_string);
            tenant_internal = Some(internal);
        }
    }

    let mut catalog_internal = None;
    let mut catalog_external = None;
    if let (Some(tenant), Some(raw)) = (tenant_internal.as_deref(), catalog_raw.as_deref()) {
        if let Ok(internal) = state.resolver.resolve_catalog(tenant, raw).await {
            catalog_external = state
                .resolver
                .catalog_external_id(&internal)
                .map(str::to_string);
            catalog_internal = Some(internal);
        }
    }

    let mut collection_internal = None;
    let mut collection_external = None;
    if let (Some(catalog), Some(raw)) = (catalog_internal.as_deref(), collection_raw.as_deref()) {
        if let Ok(internal) = state.resolver.resolve_collection(catalog, raw).await {
            collection_external = state
                .resolver
                .collection_external_id(&internal)
                .map(str::to_string);
            collection_internal = Some(internal);
        }
    }

    let tenant_settings = tenant_internal
        .as_deref()
        .and_then(|id| state.tenants.iter().find(|decl| decl.id == id))
        .map(|decl| &decl.settings);

    let inherited_threshold = resolve_effective_settings(
        &SettingsDecl::default(),
        &SettingsDecl::default(),
        tenant_settings.unwrap_or(&SettingsDecl::default()),
        &state.config.settings,
        &profiles_by_id(state),
    )
    .slow_request_ms;
    let threshold = collection_internal
        .as_deref()
        .and_then(|id| state.router.effective_settings(id))
        .map(|settings| settings.slow_request_ms)
        .unwrap_or(inherited_threshold);

    let has_collection = route.contains("{cid}");
    let qualified = match (
        tenant_external.as_deref(),
        catalog_external.as_deref(),
        collection_external.as_deref(),
    ) {
        (Some(tenant), Some(catalog), Some(collection)) => Some((tenant, catalog, collection)),
        _ => None,
    };
    let collection_metric = if has_collection {
        qualified
            .map(|ids| {
                collection_metric_label(
                    &state.config.server.metrics_collection_allowlist,
                    Some(ids),
                )
            })
            .unwrap_or_else(|| "other".to_string())
    } else {
        "none".to_string()
    };

    RequestIdentity {
        tenant_metric: tenant_metric_label(
            &state.config.server.metrics_tenant_allowlist,
            tenant_external.as_deref(),
        ),
        collection_metric,
        tenant_log: tenant_external.unwrap_or_else(|| "unknown".to_string()),
        catalog_log: if catalog_raw.is_some() {
            catalog_external.unwrap_or_else(|| "unknown".to_string())
        } else {
            "none".to_string()
        },
        collection_log: if has_collection {
            collection_external.unwrap_or_else(|| "unknown".to_string())
        } else {
            "none".to_string()
        },
        slow_threshold: Duration::from_millis(threshold),
    }
}

/// Sets the per-instance config-version gauge (`#110`) — a single,
/// label-free time series whose numeric value is
/// [`ConfigVersion::fingerprint`](tellurion_core::ConfigVersion::fingerprint):
/// a `u64` derived from the version's own content hash, not a metric
/// *label* (a fresh label value on every reload would leave the previous
/// reload's series registered for the rest of the process's life — see
/// that method's own doc for why this shape is the bounded one). Called
/// once at boot (`main.rs`, right after the initial config load) and again
/// on every successful reload (`reload.rs::attempt_reload`), so an
/// operator (or an alerting rule) comparing this instance's fingerprint
/// against the fingerprint a `ConfigStore::write` response reported can
/// tell whether this instance has converged to that change yet — the
/// measurable half of `#110`'s documented staleness bound (see
/// `reload.rs`'s own module doc for the bound itself).
pub fn set_config_version_gauge(version: &tellurion_core::ConfigVersion) {
    metrics::gauge!("tellurion_config_version").set(version.fingerprint() as f64);
}

/// Availability of the *configured* optional L2 tile-cache tier (`#161`):
/// `1` when the last readiness-cadence probe reached it, `0` when it did
/// not. Labeled by the backend the operator selected (`cache.l2.backend`),
/// so an alert can name it; cardinality is one series, since a deployment
/// configures at most one L2 backend.
///
/// Only ever called for a deployment that HAS an L2 tier. A deployment with
/// no `cache.l2` registers no series at all — an absent optimization is not
/// a `0`, and an alert on `tile_cache_l2_available == 0` must never fire for
/// a cache nobody asked for. This is also why availability is its own gauge
/// rather than something derived from the `tile_cache_*` hit/miss counters:
/// those cannot tell "the L2 tier is down" from "the L2 tier is cold".
pub fn set_l2_cache_available(backend: &str, available: bool) {
    metrics::gauge!("tile_cache_l2_available", "backend" => backend.to_string())
        .set(if available { 1.0 } else { 0.0 });
}

pub fn set_control_store_revision(revision: u64) {
    metrics::gauge!("tellurion_control_store_revision").set(revision as f64);
}

pub fn set_control_applied_revision(applied: u64, store: u64) {
    metrics::gauge!("tellurion_control_applied_revision").set(applied as f64);
    metrics::gauge!("tellurion_control_revision_lag").set(store.saturating_sub(applied) as f64);
}

pub fn observe_control_activation(elapsed: Duration) {
    metrics::histogram!("tellurion_control_activation_seconds").record(elapsed.as_secs_f64());
}

/// Counts file-watch/`SIGHUP` reload attempts that were declined because the
/// config document just read hashes identically to the one already serving
/// (`#260`) — see `reload.rs::attempt_reload` for the guard itself and why
/// activating an unchanged document is not free.
///
/// A monotonic counter, not a gauge, and label-free: the interesting
/// question is "how often is this instance being triggered for nothing",
/// which `rate()` answers, and there is no bounded label that would say
/// more than the accompanying log line already does. Paired with
/// `tellurion_config_version`: a counter climbing while that gauge holds
/// still is exactly the signal that something in the config directory is
/// churning without the config itself changing.
pub fn record_reload_skipped_unchanged() {
    metrics::counter!("tellurion_config_reload_skipped_unchanged_total").increment(1);
}

pub fn record_control_poll_failure() {
    metrics::counter!("tellurion_control_poll_failures_total").increment(1);
}

pub fn record_control_activation_failure() {
    metrics::counter!("tellurion_control_activation_failures_total").increment(1);
}

pub fn record_control_refresh_success() {
    let unix_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    metrics::gauge!("tellurion_control_last_successful_refresh_unix_seconds")
        .set(unix_seconds as f64);
}

/// Spawns a background task sampling process RSS into a gauge every
/// `interval`. Linux reads `/proc/self/status`; other platforms have no
/// portable equivalent without a native dependency this crate doesn't
/// otherwise need, so RSS simply isn't reported there (logged once, at
/// startup, rather than on every tick).
pub fn spawn_rss_sampler(interval: Duration) {
    #[cfg(not(target_os = "linux"))]
    tracing::debug!(
        "RSS sampling is only implemented via /proc/self/status (linux); \
         process_resident_memory_bytes will not be reported on this platform"
    );

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            if let Some(bytes) = read_rss_bytes().await {
                metrics::gauge!("process_resident_memory_bytes").set(bytes as f64);
            }
        }
    });
}

#[cfg(target_os = "linux")]
async fn read_rss_bytes() -> Option<u64> {
    let contents = tokio::fs::read_to_string("/proc/self/status").await.ok()?;
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
async fn read_rss_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod request_observation_tests {
    use super::*;
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
        type Writer = CaptureWriter;

        fn make_writer(&'a self) -> Self::Writer {
            CaptureWriter(Arc::clone(&self.0))
        }
    }

    impl Write for CaptureWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn normalizes_methods_to_a_finite_vocabulary() {
        for (raw, expected) in [
            ("GET", "GET"),
            ("HEAD", "HEAD"),
            ("OPTIONS", "OPTIONS"),
            ("POST", "POST"),
            ("PUT", "PUT"),
            ("PATCH", "PATCH"),
            ("DELETE", "DELETE"),
            ("TRACE", "TRACE"),
            ("CONNECT", "CONNECT"),
            ("PURGE", "other"),
        ] {
            let method = Method::from_bytes(raw.as_bytes()).unwrap();
            assert_eq!(normalize_method(&method), expected, "method {raw}");
        }
    }

    #[test]
    fn classifies_every_route_family_without_using_raw_ids() {
        let cases = [
            (None, None, "unmatched"),
            (Some("/"), None, "control"),
            (Some("/metrics"), None, "control"),
            (Some("/ui/{*path}"), Some("text/html"), "control"),
            (Some("/{tenant}/"), Some("application/json"), "control"),
            (
                Some("/{tenant}/features/catalogs/{catalog}/collections/{cid}/items/{fid}"),
                Some("application/geo+json"),
                "features",
            ),
            (
                Some("/{tenant}/tiles/catalogs/{catalog}/collections/{cid}/tiles"),
                Some("application/json"),
                "tiles",
            ),
            (
                Some("/{tenant}/tiles/catalogs/{catalog}/collections/{cid}/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}"),
                Some("application/vnd.mapbox-vector-tile"),
                "mvt",
            ),
            (
                Some("/{tenant}/tiles/catalogs/{catalog}/collections/{cid}/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}"),
                Some("image/png"),
                "png",
            ),
            (
                Some("/{tenant}/tiles/catalogs/{catalog}/collections/{cid}/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}"),
                Some("application/problem+json"),
                "tiles",
            ),
            (
                Some("/{tenant}/tiles/catalogs/{catalog}/collections/{cid}/styles/{styleId}/map/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}"),
                Some("image/png"),
                "styled_png",
            ),
            (
                Some("/{tenant}/3dtiles/catalogs/{catalog}/collections/{cid}/3dtiles"),
                Some("application/json"),
                "places3d",
            ),
            (
                Some("/{tenant}/styles/catalogs/{catalog}/styles/{styleId}"),
                Some("application/json"),
                "styles",
            ),
            (
                Some("/{tenant}/stac/catalogs/{catalog}/search"),
                Some("application/geo+json"),
                "stac",
            ),
        ];

        for (route, content_type, expected) in cases {
            assert_eq!(
                classify_lane(route, content_type),
                expected,
                "route {route:?}"
            );
        }
    }

    #[test]
    fn collection_allowlist_requires_an_exact_fully_qualified_match() {
        let allowlist = vec![tellurion_core::config::MetricCollectionRef {
            tenant: "public".to_string(),
            catalog: "default".to_string(),
            collection: "roads".to_string(),
        }];

        assert_eq!(
            collection_metric_label(&allowlist, Some(("public", "default", "roads"))),
            "public/default/roads"
        );
        assert_eq!(
            collection_metric_label(&allowlist, Some(("public", "default", "roads-2"))),
            "other"
        );
        assert_eq!(
            collection_metric_label(&allowlist, Some(("public-2", "default", "roads"))),
            "other"
        );
        assert_eq!(collection_metric_label(&allowlist, None), "none");
    }

    #[test]
    fn tenant_allowlist_exposes_only_configured_resolved_tenants() {
        let allowlist = vec!["public".to_string()];

        assert_eq!(tenant_metric_label(&allowlist, Some("public")), "public");
        assert_eq!(tenant_metric_label(&allowlist, Some("partner")), "other");
        assert_eq!(tenant_metric_label(&allowlist, None), "unknown");
        assert_eq!(tenant_metric_label(&[], Some("public")), "other");
    }

    #[test]
    fn slow_threshold_is_strict() {
        let threshold = Duration::from_millis(1000);
        assert!(!is_slow(Duration::from_millis(999), threshold));
        assert!(!is_slow(Duration::from_millis(1000), threshold));
        assert!(is_slow(Duration::from_millis(1001), threshold));
    }

    #[test]
    fn slow_event_is_one_json_record_with_only_bounded_public_context() {
        let capture = Capture::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_ansi(false)
            .without_time()
            .with_writer(capture.clone())
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            emit_slow_request(
                "GET",
                "/{tenant}/features/catalogs/{catalog}/collections/{cid}/items/{fid}",
                "features",
                "200",
                "3d1f8f3a-0000-4000-8000-000000000000",
                "public",
                "default",
                "roads",
                Duration::from_millis(1500),
                tellurion_core::observability::PhaseSnapshot::default(),
            );
        });

        let output = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<_> = output.lines().collect();
        assert_eq!(lines.len(), 1, "{output}");
        let json: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        let fields = json["fields"].as_object().unwrap();
        assert_eq!(fields["event"], "slow_request");
        assert_eq!(fields["request_id"], "3d1f8f3a-0000-4000-8000-000000000000");
        assert_eq!(fields["tenant"], "public");
        assert_eq!(fields["collection"], "roads");
        for forbidden in ["uri", "query", "headers", "credential", "error"] {
            assert!(
                !fields.contains_key(forbidden),
                "forbidden field {forbidden}: {output}"
            );
        }
    }
}

/// `#9`: the `/proc/self/status` parsing above has only ever run on Linux
/// CI, where it worked (or didn't) with nothing asserting either way — every
/// other platform takes the no-op branch. This drives the real production
/// pipeline (`spawn_rss_sampler` -> `metrics::gauge!` -> the real
/// `/metrics` handler) against this test binary's own process, in-process,
/// so it needs no database and no separate server to spawn.
#[cfg(all(test, target_os = "linux"))]
mod linux_rss_tests {
    use axum::body::to_bytes;

    use super::*;

    const GAUGE_NAME: &str = "process_resident_memory_bytes";
    const BUFFER_SIZE: usize = 64 * 1024 * 1024;
    const PAGE_STRIDE: usize = 4096;
    const POLL_TIMEOUT: Duration = Duration::from_secs(5);
    const POLL_INTERVAL: Duration = Duration::from_millis(20);

    /// Renders `/metrics` through the exact handler production traffic
    /// hits, then pulls `metric`'s bare (label-free) value off the line it
    /// starts, per the Prometheus text exposition format.
    async fn render_gauge(handle: &PrometheusHandle, metric: &str) -> Option<f64> {
        let response = metrics_handler(Extension(handle.clone()))
            .await
            .into_response();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        text.lines().find_map(|line| {
            line.strip_prefix(metric)
                .and_then(|rest| rest.trim_start().parse::<f64>().ok())
        })
    }

    /// Polls `/metrics` until `metric` clears `threshold`, panicking once
    /// [`POLL_TIMEOUT`] elapses. The sampler ticks on its own schedule, so
    /// the test waits for it rather than assuming any particular tick
    /// already landed.
    async fn wait_for_gauge_above(handle: &PrometheusHandle, metric: &str, threshold: f64) -> f64 {
        let deadline = Instant::now() + POLL_TIMEOUT;
        loop {
            if let Some(value) = render_gauge(handle, metric).await {
                if value > threshold {
                    return value;
                }
            }
            assert!(
                Instant::now() < deadline,
                "{metric} never exceeded {threshold} in /metrics within {POLL_TIMEOUT:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// `install_recorder` sets the process-global recorder exactly once —
    /// the same one `main.rs` installs at boot. Calling it here unguarded is
    /// safe because this is the only test in this binary that does.
    #[tokio::test]
    async fn linux_rss_gauge_appears_and_moves_under_load() {
        let handle = install_recorder().expect("installs the process-global recorder");
        spawn_rss_sampler(POLL_INTERVAL);

        let before = wait_for_gauge_above(&handle, GAUGE_NAME, 0.0).await;
        assert!(
            before > 1024.0 * 1024.0,
            "a running test binary should plausibly use over 1MB of RSS, got {before} bytes"
        );

        // Allocate and touch a 64MB buffer one page at a time so every page
        // actually faults in (a freshly zeroed `Vec` can otherwise be
        // served from a single shared zero page and never become resident)
        // and the writes can't be optimized away.
        let mut buffer = vec![0u8; BUFFER_SIZE];
        for page in buffer.chunks_mut(PAGE_STRIDE) {
            page[0] = 1;
        }
        std::hint::black_box(&buffer);

        let after =
            wait_for_gauge_above(&handle, GAUGE_NAME, before + (BUFFER_SIZE as f64) / 2.0).await;
        assert!(
            after > before,
            "RSS after touching {BUFFER_SIZE} bytes ({after}) must exceed the baseline ({before})"
        );

        drop(buffer);
    }
}
