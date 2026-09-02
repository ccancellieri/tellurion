//! Liveness and stateful dependency readiness for the HTTP serving process.
//!
//! # Mandatory dependencies versus optional tiers (`#161`)
//!
//! This module probes two categories of thing, and the distinction is
//! structural here, not a branch inside one error handler:
//!
//! * **Mandatory dependencies** — the registry and every routed storage.
//!   Without them the process cannot answer a request correctly at all, so
//!   one of them failing makes readiness *false* and the orchestrator pulls
//!   this replica out of the load balancer.
//! * **Optional tiers** — today just the `cache.l2` tile cache. A cache is a
//!   latency optimization: with it down, every request is still answered,
//!   and answered *correctly*, from L1 plus the origin storage. So an
//!   unavailable L2 tier leaves the process **ready** and is reported as a
//!   named degradation instead.
//!
//! Choosing not-ready for a down cache would be actively harmful, not merely
//! conservative: it would remove serving capacity from the pool at exactly
//! the moment the cache tier stopped absorbing load, converting a slower
//! service into an unavailable one, across every replica at once (they all
//! share the cache, so they all fail the probe together). The
//! production-observability design doc already stated this policy — "an
//! optional L2 cache outage does not make the process unready" — and this
//! module now makes it observable rather than silent.
//!
//! The two probes therefore run as two independently-deadlined futures with
//! two different result types ([`ProbeFailures`] versus [`L2Report`]), joined
//! but never merged. A hanging L2 backend cannot consume the mandatory
//! probe's deadline, and no `?`/`Err` path can accidentally promote a cache
//! degradation into an unreadiness.
//!
//! # What the response says
//!
//! Absence of an unconfigured optimization is not a degradation. A
//! deployment with no `cache.l2` has no L2 tier
//! ([`TileCache::l2_tier`](tellurion_core::TileCache::l2_tier) is `None`),
//! is never probed for one, and gets the byte-for-byte empty `200` it always
//! got. So does a deployment whose configured tier is healthy. Only a
//! configured-and-unavailable tier adds a body, and that body names the
//! component and the backend — `cache.l2` / `valkey` — plus a stable reason
//! code, never a generic "degraded".
//!
//! The reason *detail* (the backend's own error text, which can carry a host
//! or DSN) stays in the log, where `log_redact` scrubs credentials, and out
//! of the unauthenticated HTTP body — the same rule the `503` body already
//! follows.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::Extension;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use tellurion_core::problem::{Problem, PROBLEM_JSON};
use tellurion_core::{AppContext, L2Tier, PageRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum ReadinessStatus {
    Initial = 0,
    Ready = 1,
    Failed = 2,
    Draining = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transition {
    Changed(ReadinessStatus),
    Unchanged,
    IgnoredDraining,
    IgnoredStale,
}

#[derive(Debug)]
struct ProbeFailures(Vec<String>);

impl std::fmt::Display for ProbeFailures {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0.join("; "))
    }
}

/// What this process can currently say about its optional L2 tile-cache tier
/// (`#161`). Deliberately three states, not two: "the operator configured no
/// cache" and "the cache the operator configured is missing" are different
/// facts, and a shape that could not tell them apart would either invent a
/// degradation for a deployment that never asked for a cache, or hide a real
/// outage behind "nothing configured".
///
/// None of these states influences [`ReadinessStatus`] — see the module doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum L2Report {
    /// No `cache.l2` in this deployment's config. Reported nowhere, counted
    /// nowhere, exactly as before `#161`.
    NotConfigured,
    /// A configured tier that answered its last probe.
    Available { backend: String },
    /// A configured tier that did not. Named, with a reason.
    Unavailable {
        backend: String,
        reason: L2Unavailable,
    },
}

/// Why a configured L2 tier is unavailable, as a closed set of stable codes.
/// A code, not the backend's raw error text: this is the part that reaches
/// an unauthenticated HTTP body, and an operator alerting on it needs a
/// value that does not change wording with the client library's version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum L2Unavailable {
    /// Configured, but the backend never connected at boot; this process is
    /// L1-only until it restarts (`main::build_cache`).
    NeverConnected,
    /// The backend is wired in but refused or failed the probe.
    Unreachable,
    /// The backend did not answer within the readiness probe timeout.
    ProbeTimedOut,
}

impl L2Unavailable {
    fn code(self) -> &'static str {
        match self {
            Self::NeverConnected => "never-connected-at-boot",
            Self::Unreachable => "unreachable",
            Self::ProbeTimedOut => "probe-timeout",
        }
    }
}

/// The `200` body a *degraded but serving* process returns. Absent entirely
/// (empty body, byte-for-byte the pre-`#161` response) unless there is a
/// named degradation to report.
#[derive(Debug, Serialize)]
struct ReadinessReport {
    status: &'static str,
    degradations: Vec<Degradation>,
}

/// One named degradation. `component` is the config path the operator wrote
/// (`cache.l2`), `backend` the selection they wrote there (`valkey`), and
/// `reason` an [`L2Unavailable`] code — enough to act on without leaking an
/// address, DSN, or internal error chain.
#[derive(Debug, Serialize)]
struct Degradation {
    component: &'static str,
    backend: String,
    reason: &'static str,
}

impl L2Report {
    /// The degradation this report contributes to a `200` body, if any.
    /// `NotConfigured` and `Available` both contribute nothing — an
    /// optimization that was never asked for and one that is working are
    /// equally not degradations.
    fn degradation(&self) -> Option<Degradation> {
        match self {
            Self::NotConfigured | Self::Available { .. } => None,
            Self::Unavailable { backend, reason } => Some(Degradation {
                component: "cache.l2",
                backend: backend.clone(),
                reason: reason.code(),
            }),
        }
    }
}

/// One cloneable readiness signal shared by HTTP handlers, dependency
/// polling, and graceful shutdown. Draining is a terminal process state.
#[derive(Debug, Clone)]
pub(crate) struct Readiness {
    state: Arc<Mutex<ReadinessState>>,
}

#[derive(Debug)]
struct ReadinessState {
    status: ReadinessStatus,
    generation: u64,
    /// `#161`. Starts `NotConfigured` — the truthful statement for a process
    /// that has not yet looked, and the permanent one for every deployment
    /// with no `cache.l2`, so nothing is ever reported for a cache the
    /// operator never configured.
    l2: L2Report,
}

impl Readiness {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ReadinessState {
                status: ReadinessStatus::Initial,
                generation: 0,
                l2: L2Report::NotConfigured,
            })),
        }
    }

    #[cfg(test)]
    pub(crate) fn l2(&self) -> L2Report {
        self.state
            .lock()
            .expect("readiness state lock poisoned")
            .l2
            .clone()
    }

    /// Records the latest optional-tier report, returning the previous one
    /// so the caller can log only on an actual transition — the same
    /// "log once on state change, not on every orchestration poll" rule the
    /// mandatory dependency probe follows.
    ///
    /// Never touches `status`: that is the whole `#161` decision, expressed
    /// as a method that structurally cannot change readiness rather than as
    /// a branch that happens not to.
    fn record_l2(&self, report: L2Report) -> L2Report {
        let mut state = self.state.lock().expect("readiness state lock poisoned");
        std::mem::replace(&mut state.l2, report)
    }

    pub(crate) fn status(&self) -> ReadinessStatus {
        self.state
            .lock()
            .expect("readiness state lock poisoned")
            .status
    }

    #[cfg(test)]
    pub(crate) fn is_ready(&self) -> bool {
        self.status() == ReadinessStatus::Ready
    }

    /// One consistent read of both facts a readiness response is built from,
    /// under a single lock: the status, and the optional tier's degradation
    /// if it has one. Taking two locks would let a response pair one poll's
    /// verdict with a different poll's report.
    fn snapshot(&self) -> (ReadinessStatus, Option<Degradation>) {
        let state = self.state.lock().expect("readiness state lock poisoned");
        (state.status, state.l2.degradation())
    }

    pub(crate) fn begin_draining(&self) {
        let _ = self.begin_draining_transition();
    }

    /// Applies a synchronous context swap and invalidates its previous
    /// dependency result as one readiness handoff. Readers cannot observe
    /// the new context while the previous generation still reports ready.
    pub(crate) fn reload_and_invalidate<F>(&self, swap: F)
    where
        F: FnOnce(),
    {
        let mut state = self.state.lock().expect("readiness state lock poisoned");
        swap();
        state.generation = state.generation.wrapping_add(1);
        if state.status != ReadinessStatus::Draining {
            state.status = ReadinessStatus::Initial;
        }
    }

    fn probe_generation(&self) -> u64 {
        self.state
            .lock()
            .expect("readiness state lock poisoned")
            .generation
    }

    fn begin_draining_transition(&self) -> Transition {
        self.transition(None, ReadinessStatus::Draining)
    }

    #[cfg(test)]
    fn record_success(&self) -> Transition {
        self.transition(None, ReadinessStatus::Ready)
    }

    #[cfg(test)]
    fn record_failure(&self) -> Transition {
        self.transition(None, ReadinessStatus::Failed)
    }

    fn record_success_for(&self, generation: u64) -> Transition {
        self.transition(Some(generation), ReadinessStatus::Ready)
    }

    fn record_failure_for(&self, generation: u64) -> Transition {
        self.transition(Some(generation), ReadinessStatus::Failed)
    }

    fn transition(&self, generation: Option<u64>, target: ReadinessStatus) -> Transition {
        let mut state = self.state.lock().expect("readiness state lock poisoned");
        if generation.is_some_and(|generation| generation != state.generation) {
            return Transition::IgnoredStale;
        }
        if state.status == ReadinessStatus::Draining {
            return if target == ReadinessStatus::Draining {
                Transition::Unchanged
            } else {
                Transition::IgnoredDraining
            };
        }
        if state.status == target {
            return Transition::Unchanged;
        }
        let previous = state.status;
        state.status = target;
        Transition::Changed(previous)
    }
}

impl Default for Readiness {
    fn default() -> Self {
        Self::new()
    }
}

/// Probes the optional L2 tile-cache tier under its OWN deadline, and
/// reports it — never returning anything the caller could mistake for a
/// readiness verdict (`#161`).
///
/// `tier` is `None` for every deployment that configured no `cache.l2`: no
/// probe runs, no metric is emitted, and the answer is
/// [`L2Report::NotConfigured`]. There is no default to invent here — a
/// deployment that asked for no cache has no cache state to describe.
/// The `detail` returned alongside the report is the backend's own error
/// text. It is carried OUT of this function rather than logged here so the
/// caller can honour the module's "log once on state transition, not on
/// every orchestration poll" rule, and it never enters [`L2Report`] itself:
/// the report is compared for equality to detect transitions, and a detail
/// that varies between two otherwise identical failures would re-log a
/// state that never changed.
async fn probe_l2(tier: Option<Arc<L2Tier>>, timeout: Duration) -> (L2Report, Option<String>) {
    let Some(tier) = tier else {
        return (L2Report::NotConfigured, None);
    };
    let backend = tier.backend().to_string();
    // Which failure this tier would report is decided by which tier it IS,
    // read up front — not inferred later from whatever an error happens to
    // look like.
    let boot_down = matches!(tier.state(), tellurion_core::L2TierState::NeverConnected(_));
    match tokio::time::timeout(timeout, tier.probe()).await {
        Ok(Ok(())) => (L2Report::Available { backend }, None),
        Ok(Err(error)) => (
            L2Report::Unavailable {
                backend,
                reason: if boot_down {
                    L2Unavailable::NeverConnected
                } else {
                    L2Unavailable::Unreachable
                },
            },
            Some(error.to_string()),
        ),
        Err(_) => (
            L2Report::Unavailable {
                backend,
                reason: L2Unavailable::ProbeTimedOut,
            },
            Some(format!(
                "no answer within the {}ms readiness probe timeout",
                timeout.as_millis()
            )),
        ),
    }
}

/// Applies an [`L2Report`], logging only on a transition — the same "log
/// once on state change, not on every orchestration poll" rule the mandatory
/// dependency probe follows — and keeping the per-backend availability gauge
/// in step.
///
/// Emits nothing at all for [`L2Report::NotConfigured`]: not a log line, and
/// in particular not a `0` on the gauge. A deployment with no `cache.l2`
/// must not grow a time series claiming its nonexistent cache is down.
///
/// The failure `detail` goes to the log, where `log_redact` scrubs
/// credentials out of anything a client library rendered into it. It never
/// reaches the HTTP body.
fn apply_l2_report(readiness: &Readiness, report: L2Report, detail: Option<String>) {
    let previous = readiness.record_l2(report.clone());
    let changed = previous != report;
    match &report {
        L2Report::NotConfigured => {}
        L2Report::Available { backend } => {
            crate::metrics::set_l2_cache_available(backend, true);
            if changed {
                tracing::info!(
                    backend = %backend,
                    "cache.l2: the configured L2 tile-cache tier is available"
                );
            }
        }
        L2Report::Unavailable { backend, reason } => {
            crate::metrics::set_l2_cache_available(backend, false);
            if changed {
                tracing::error!(
                    backend = %backend,
                    reason = reason.code(),
                    detail = detail.as_deref().unwrap_or(""),
                    "cache.l2: the configured L2 tile-cache tier is unavailable; serving L1-only, readiness stays ready"
                );
            }
        }
    }
}

/// Runs one bounded probe against exactly one current context snapshot.
///
/// Two probes, two deadlines, two result types (`#161`): the mandatory
/// dependency probe decides readiness, the optional-tier probe decides only
/// what the report says. They are joined so the cadence cost stays one
/// timeout wide, never chained — a wedged cache backend must not be able to
/// eat the deadline the registry and storages are judged by.
pub(crate) async fn probe_once(ctx: &AppContext, readiness: &Readiness, timeout: Duration) {
    let state = ctx.current();
    let probe_state = Arc::clone(&state);
    let generation = readiness.probe_generation();
    let probe = async move {
        let mut failures = Vec::new();
        for tenant in &probe_state.tenants {
            if let Err(error) = probe_state
                .registry
                .list_catalogs(
                    &tenant.id,
                    PageRequest {
                        limit: 1,
                        after: None,
                    },
                )
                .await
            {
                failures.push(format!("registry dependency: {error}"));
            }
        }
        if let Err(error) = probe_state.router.probe_storages().await {
            failures.push(format!("storage dependency: {error}"));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(ProbeFailures(failures))
        }
    };

    let (mandatory, (l2_report, l2_detail)) = tokio::join!(
        tokio::time::timeout(timeout, probe),
        probe_l2(ctx.cache.l2_tier(), timeout),
    );
    apply_l2_report(readiness, l2_report, l2_detail);

    match mandatory {
        Ok(Ok(())) => match record_probe_success(ctx, readiness, &state, generation) {
            Transition::Changed(ReadinessStatus::Failed) => {
                tracing::info!("readiness dependency probe recovered");
            }
            Transition::Changed(_) => {
                tracing::info!("readiness dependency probe succeeded");
            }
            Transition::Unchanged | Transition::IgnoredDraining | Transition::IgnoredStale => {}
        },
        Ok(Err(error)) => {
            if matches!(
                record_probe_failure(ctx, readiness, &state, generation),
                Transition::Changed(_)
            ) {
                tracing::error!(error = %error, "readiness dependency probe failed");
            }
        }
        Err(_) => {
            if matches!(
                record_probe_failure(ctx, readiness, &state, generation),
                Transition::Changed(_)
            ) {
                tracing::error!(
                    timeout_ms = timeout.as_millis(),
                    "readiness dependency probe timed out"
                );
            }
        }
    }
}

fn record_probe_success(
    ctx: &AppContext,
    readiness: &Readiness,
    state: &Arc<tellurion_core::ContextState>,
    generation: u64,
) -> Transition {
    if Arc::ptr_eq(state, &ctx.current()) {
        readiness.record_success_for(generation)
    } else {
        Transition::IgnoredStale
    }
}

fn record_probe_failure(
    ctx: &AppContext,
    readiness: &Readiness,
    state: &Arc<tellurion_core::ContextState>,
    generation: u64,
) -> Transition {
    if Arc::ptr_eq(state, &ctx.current()) {
        readiness.record_failure_for(generation)
    } else {
        Transition::IgnoredStale
    }
}

/// Polls immediately, then waits the configured interval before each next
/// dependency check. The process owner controls this task's lifetime.
pub(crate) async fn run(
    ctx: Arc<AppContext>,
    readiness: Readiness,
    interval: Duration,
    timeout: Duration,
) {
    while readiness.status() != ReadinessStatus::Draining {
        probe_once(&ctx, &readiness, timeout).await;
        if readiness.status() == ReadinessStatus::Draining {
            break;
        }
        tokio::time::sleep(interval).await;
    }
}

pub(crate) async fn healthz() -> StatusCode {
    StatusCode::OK
}

/// `200` while the latest mandatory dependency probe succeeded and the
/// process is not draining; `503` with the shared problem+json shape
/// otherwise — both unchanged.
///
/// `#161` adds exactly one thing: when the process is ready AND a
/// *configured* optional tier is unavailable, the `200` carries a body
/// naming it. Every other case — no `cache.l2` configured, or a configured
/// one that is healthy — returns the same empty `200` it always did, down to
/// the byte and the absence of a `Content-Type`. An orchestrator reading
/// only the status code sees no change in any case, which is the point: a
/// degraded cache must not evict a replica that is still serving correctly.
pub(crate) async fn readyz(Extension(readiness): Extension<Readiness>) -> Response {
    let (status, degradation) = readiness.snapshot();
    if status == ReadinessStatus::Ready {
        let Some(degradation) = degradation else {
            return StatusCode::OK.into_response();
        };
        return (
            StatusCode::OK,
            Json(ReadinessReport {
                status: "degraded",
                degradations: vec![degradation],
            }),
        )
            .into_response();
    }

    let problem = Problem::new(
        StatusCode::SERVICE_UNAVAILABLE.as_u16(),
        "ServiceUnavailable",
        "server is not ready",
    );
    let mut response = (StatusCode::SERVICE_UNAVAILABLE, Json(problem)).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(PROBLEM_JSON));
    response
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use axum::body::to_bytes;
    use bytes::Bytes;
    use tellurion_core::{
        AppConfig, AppContext, CatalogDecl, CatalogSource, CollectionDecl, DriverFactory,
        Error as CoreError, FileStyleStore, L2Cache, L2CacheAdapter, L2Tier, LayeredCache,
        MetricsTileCache, MokaTileCache, Page, PageRequest, PhysicalCollection, Registry,
        RegistryReader, Resolver, Result as CoreResult, Router, StaticResolver, StorageDecl,
        StorageDriver, StyleStore, TileCache, TileKey,
    };

    use super::*;

    type ProbeRequest = (String, u32, Option<String>);
    type ProbeRequests = Arc<Mutex<Vec<ProbeRequest>>>;
    type ProbeFixture = (
        Arc<AppContext>,
        Arc<AtomicUsize>,
        ProbeRequests,
        Arc<AtomicUsize>,
    );

    #[test]
    fn readiness_starts_initial_and_not_ready() {
        let readiness = Readiness::new();

        assert_eq!(readiness.status(), ReadinessStatus::Initial);
        assert!(!readiness.is_ready());
    }

    #[test]
    fn reload_swap_and_invalidation_are_one_reader_atomic_handoff() {
        let readiness = Readiness::new();
        readiness.record_success();

        let (swap_started_tx, swap_started_rx) = std::sync::mpsc::channel();
        let (release_swap_tx, release_swap_rx) = std::sync::mpsc::channel();
        let reload_readiness = readiness.clone();
        let reload = std::thread::spawn(move || {
            reload_readiness.reload_and_invalidate(|| {
                swap_started_tx.send(()).unwrap();
                release_swap_rx.recv().unwrap();
            });
        });
        swap_started_rx.recv().unwrap();

        let (observed_tx, observed_rx) = std::sync::mpsc::channel();
        let reader_readiness = readiness.clone();
        let reader = std::thread::spawn(move || {
            observed_tx.send(reader_readiness.is_ready()).unwrap();
        });
        assert!(
            observed_rx.recv_timeout(Duration::from_millis(25)).is_err(),
            "a readiness reader must wait until the context swap is invalidated"
        );

        release_swap_tx.send(()).unwrap();
        reload.join().unwrap();
        assert!(!observed_rx.recv().unwrap());
        reader.join().unwrap();
    }

    #[test]
    fn readiness_can_fail_recover_and_drain_irreversibly() {
        let readiness = Readiness::new();

        assert_eq!(
            readiness.record_failure(),
            Transition::Changed(ReadinessStatus::Initial)
        );
        assert_eq!(readiness.status(), ReadinessStatus::Failed);
        assert_eq!(
            readiness.record_success(),
            Transition::Changed(ReadinessStatus::Failed)
        );
        assert_eq!(readiness.status(), ReadinessStatus::Ready);
        assert_eq!(
            readiness.begin_draining_transition(),
            Transition::Changed(ReadinessStatus::Ready)
        );
        assert_eq!(readiness.status(), ReadinessStatus::Draining);
        assert_eq!(readiness.record_success(), Transition::IgnoredDraining);
        assert_eq!(readiness.record_failure(), Transition::IgnoredDraining);
        assert_eq!(readiness.begin_draining_transition(), Transition::Unchanged);
        assert_eq!(readiness.status(), ReadinessStatus::Draining);
    }

    #[test]
    fn repeated_probe_results_do_not_report_another_transition() {
        let readiness = Readiness::new();

        assert_eq!(
            readiness.record_failure(),
            Transition::Changed(ReadinessStatus::Initial)
        );
        assert_eq!(readiness.record_failure(), Transition::Unchanged);
        assert_eq!(
            readiness.record_success(),
            Transition::Changed(ReadinessStatus::Failed)
        );
        assert_eq!(readiness.record_success(), Transition::Unchanged);
    }

    #[derive(Clone, Copy)]
    enum RegistryBehavior {
        Success,
        Failure,
        Slow,
    }

    struct ProbeRegistry {
        behavior: RegistryBehavior,
        calls: Arc<AtomicUsize>,
        requests: ProbeRequests,
    }

    #[async_trait::async_trait]
    impl RegistryReader for ProbeRegistry {
        async fn catalog(
            &self,
            _tenant_internal_id: &str,
            _catalog_external_id: &str,
        ) -> CoreResult<Option<CatalogDecl>> {
            Ok(None)
        }

        async fn collection(
            &self,
            _catalog_internal_id: &str,
            _collection_external_id: &str,
        ) -> CoreResult<Option<CollectionDecl>> {
            Ok(None)
        }

        async fn list_catalogs(
            &self,
            tenant_internal_id: &str,
            page: PageRequest,
        ) -> CoreResult<Page<CatalogDecl>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.requests.lock().unwrap().push((
                tenant_internal_id.to_string(),
                page.limit,
                page.after,
            ));
            match self.behavior {
                RegistryBehavior::Success => Ok(Page {
                    items: Vec::new(),
                    next: None,
                }),
                RegistryBehavior::Failure => Err(CoreError::Storage(Box::new(io::Error::other(
                    "registry unavailable",
                )))),
                RegistryBehavior::Slow => {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    Ok(Page {
                        items: Vec::new(),
                        next: None,
                    })
                }
            }
        }

        async fn list_collections(
            &self,
            _catalog_internal_id: &str,
            _page: PageRequest,
        ) -> CoreResult<Page<CollectionDecl>> {
            Ok(Page {
                items: Vec::new(),
                next: None,
            })
        }
    }

    struct ProbeCatalog {
        fail: bool,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CatalogSource for ProbeCatalog {
        async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                Err(CoreError::Storage(Box::new(io::Error::other(
                    "storage unavailable",
                ))))
            } else {
                Ok(Vec::new())
            }
        }
    }

    struct ProbeDriver {
        fail: bool,
        calls: Arc<AtomicUsize>,
    }

    impl StorageDriver for ProbeDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(ProbeCatalog {
                fail: self.fail,
                calls: Arc::clone(&self.calls),
            })
        }
    }

    struct ProbeFactory {
        fail: bool,
        calls: Arc<AtomicUsize>,
    }

    impl DriverFactory for ProbeFactory {
        fn name(&self) -> &str {
            "probe"
        }

        fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
            Ok(Arc::new(ProbeDriver {
                fail: self.fail,
                calls: Arc::clone(&self.calls),
            }))
        }
    }

    /// An `L2Cache` backend double whose reachability a test can flip while
    /// the process keeps running — the only way to exercise outage and
    /// recovery on the readiness cadence without a real Valkey.
    struct TestL2 {
        unreachable: AtomicBool,
        /// When set, every call hangs forever — a wedged backend, which is
        /// materially different from one that errors promptly.
        hang: AtomicBool,
        calls: Arc<AtomicUsize>,
    }

    impl TestL2 {
        fn reachable() -> Arc<Self> {
            Arc::new(Self {
                unreachable: AtomicBool::new(false),
                hang: AtomicBool::new(false),
                calls: Arc::new(AtomicUsize::new(0)),
            })
        }

        fn unreachable() -> Arc<Self> {
            let backend = Self::reachable();
            backend.unreachable.store(true, Ordering::SeqCst);
            backend
        }

        fn wedged() -> Arc<Self> {
            let backend = Self::reachable();
            backend.hang.store(true, Ordering::SeqCst);
            backend
        }

        fn set_unreachable(&self, unreachable: bool) {
            self.unreachable.store(unreachable, Ordering::SeqCst);
        }
    }

    #[async_trait::async_trait]
    impl L2Cache for TestL2 {
        async fn get(&self, _key: &TileKey) -> CoreResult<Option<Bytes>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.hang.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            if self.unreachable.load(Ordering::SeqCst) {
                return Err(CoreError::Storage(Box::new(io::Error::other(
                    "connection refused",
                ))));
            }
            Ok(None)
        }

        async fn put(&self, _key: TileKey, _value: Bytes, _ttl: Duration) -> CoreResult<()> {
            Ok(())
        }
    }

    /// The three states a deployment's tile cache can actually be in, each
    /// composed exactly the way `main::build_cache` composes it. Kept as
    /// three distinct variants on purpose: a fixture that could not tell
    /// `NotConfigured` from `NeverConnected` would let the very bug `#161`
    /// is about — reporting a cache nobody configured as degraded, or a
    /// configured-but-dead cache as absent — pass every test in this file.
    enum L2Fixture {
        /// No `cache.l2` at all — the live Render demo, and the default.
        NotConfigured,
        /// `cache.l2.backend: valkey`, connected at boot.
        Connected(Arc<TestL2>),
        /// `cache.l2.backend: valkey`, configured but never connected.
        NeverConnected,
    }

    impl L2Fixture {
        fn build(self) -> Arc<dyn TileCache> {
            let l1: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000));
            match self {
                Self::NotConfigured => Arc::new(MetricsTileCache::new(l1)),
                Self::Connected(backend) => {
                    let backend: Arc<dyn L2Cache> = backend;
                    let l2 = Arc::new(L2CacheAdapter::new(
                        Arc::clone(&backend),
                        Duration::from_secs(60),
                    ));
                    let layered = LayeredCache::with_l2_tier(
                        vec![l1, l2 as Arc<dyn TileCache>],
                        L2Tier::connected("valkey", backend),
                    );
                    Arc::new(MetricsTileCache::new(Arc::new(layered)))
                }
                Self::NeverConnected => {
                    let layered = LayeredCache::with_l2_tier(
                        vec![l1],
                        L2Tier::never_connected("valkey", "cache.l2: failed to connect to valkey"),
                    );
                    Arc::new(MetricsTileCache::new(Arc::new(layered)))
                }
            }
        }
    }

    async fn readyz_body(readiness: &Readiness) -> (StatusCode, String) {
        let response = readyz(Extension(readiness.clone())).await;
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    fn probe_context(registry_behavior: RegistryBehavior, storage_failure: bool) -> ProbeFixture {
        probe_context_with_l2(registry_behavior, storage_failure, L2Fixture::NotConfigured)
    }

    fn probe_context_with_l2(
        registry_behavior: RegistryBehavior,
        storage_failure: bool,
        l2: L2Fixture,
    ) -> ProbeFixture {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: probe, url_env: UNUSED } ]
tenants: [ { id: alpha }, { id: beta } ]
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let storage_calls = Arc::new(AtomicUsize::new(0));
        let mut drivers = Registry::new();
        drivers.register(Arc::new(ProbeFactory {
            fail: storage_failure,
            calls: Arc::clone(&storage_calls),
        }));
        let router = Router::build(&config, &drivers).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let registry_calls = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let registry: Arc<dyn RegistryReader> = Arc::new(ProbeRegistry {
            behavior: registry_behavior,
            calls: Arc::clone(&registry_calls),
            requests: Arc::clone(&requests),
        });
        let cache: Arc<dyn TileCache> = l2.build();
        let styles: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let tenants = config.tenants.clone();
        let ctx = Arc::new(AppContext::new_with_registry(
            config, tenants, router, resolver, None, registry, cache, styles,
        ));

        (ctx, registry_calls, requests, storage_calls)
    }

    #[tokio::test]
    async fn successful_probe_checks_each_tenant_with_one_entry_then_every_storage() {
        let (ctx, registry_calls, requests, storage_calls) =
            probe_context(RegistryBehavior::Success, false);
        let readiness = Readiness::new();

        probe_once(&ctx, &readiness, Duration::from_secs(1)).await;

        assert_eq!(readiness.status(), ReadinessStatus::Ready);
        assert_eq!(registry_calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            *requests.lock().unwrap(),
            vec![
                ("alpha".to_string(), 1, None),
                ("beta".to_string(), 1, None),
            ]
        );
        assert_eq!(storage_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn registry_failure_marks_failed_after_attempting_every_dependency() {
        let (ctx, registry_calls, _, storage_calls) =
            probe_context(RegistryBehavior::Failure, false);
        let readiness = Readiness::new();

        probe_once(&ctx, &readiness, Duration::from_secs(1)).await;

        assert_eq!(readiness.status(), ReadinessStatus::Failed);
        assert_eq!(registry_calls.load(Ordering::SeqCst), 2);
        assert_eq!(storage_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn storage_failure_marks_readiness_failed() {
        let (ctx, _, _, storage_calls) = probe_context(RegistryBehavior::Success, true);
        let readiness = Readiness::new();

        probe_once(&ctx, &readiness, Duration::from_secs(1)).await;

        assert_eq!(readiness.status(), ReadinessStatus::Failed);
        assert_eq!(storage_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn one_timeout_bounds_the_whole_dependency_probe() {
        let (ctx, _, _, storage_calls) = probe_context(RegistryBehavior::Slow, false);
        let readiness = Readiness::new();

        let started = tokio::time::Instant::now();
        probe_once(&ctx, &readiness, Duration::from_millis(10)).await;

        assert_eq!(readiness.status(), ReadinessStatus::Failed);
        assert_eq!(storage_calls.load(Ordering::SeqCst), 0);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn polling_runs_immediately_and_then_on_the_configured_interval() {
        let (ctx, registry_calls, _, _) = probe_context(RegistryBehavior::Success, false);
        let readiness = Readiness::new();
        let task = tokio::spawn(run(
            ctx,
            readiness,
            Duration::from_millis(30),
            Duration::from_millis(10),
        ));

        tokio::task::yield_now().await;
        assert_eq!(registry_calls.load(Ordering::SeqCst), 2);
        tokio::time::advance(Duration::from_millis(10)).await;
        assert_eq!(registry_calls.load(Ordering::SeqCst), 2);
        tokio::time::advance(Duration::from_millis(20)).await;
        tokio::task::yield_now().await;
        assert_eq!(registry_calls.load(Ordering::SeqCst), 4);

        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn polling_stops_after_drain_begins() {
        let (ctx, registry_calls, _, _) = probe_context(RegistryBehavior::Success, false);
        let readiness = Readiness::new();
        let task = tokio::spawn(run(
            ctx,
            readiness.clone(),
            Duration::from_millis(30),
            Duration::from_millis(10),
        ));

        tokio::task::yield_now().await;
        assert_eq!(registry_calls.load(Ordering::SeqCst), 2);

        readiness.begin_draining();
        tokio::time::advance(Duration::from_millis(30)).await;
        tokio::task::yield_now().await;

        assert!(task.is_finished());
        assert_eq!(registry_calls.load(Ordering::SeqCst), 2);
    }

    // --- `#161`: optional L2 tile-cache tier ---------------------------------

    /// The campaign's "no invented defaults" rule, made falsifiable: a
    /// deployment that configured no `cache.l2` must produce the byte-for-
    /// byte readiness response it produced before this feature existed —
    /// an empty `200` with no body and no `Content-Type` — and must record
    /// no cache state at all. If a future change ever reports absence as a
    /// degradation, this fails.
    #[tokio::test]
    async fn an_unconfigured_l2_is_never_probed_and_leaves_readiness_byte_identical() {
        let (ctx, _, _, _) =
            probe_context_with_l2(RegistryBehavior::Success, false, L2Fixture::NotConfigured);
        let readiness = Readiness::new();

        probe_once(&ctx, &readiness, Duration::from_secs(1)).await;

        assert_eq!(readiness.status(), ReadinessStatus::Ready);
        assert_eq!(
            readiness.l2(),
            L2Report::NotConfigured,
            "a cache the operator never configured must not be described at all"
        );
        let response = readyz(Extension(readiness.clone())).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers().get(header::CONTENT_TYPE).is_none(),
            "an unconfigured deployment's 200 must carry no body at all"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(body.is_empty(), "expected an empty 200, got {body:?}");
    }

    /// The healthy-but-configured case: still an empty `200` — a working
    /// optimization is not news — but the recorded state now says which
    /// backend answered, which is what makes the outage test below able to
    /// prove a transition rather than an absence.
    #[tokio::test]
    async fn a_reachable_configured_l2_is_recorded_available_with_no_degradation_body() {
        let backend = TestL2::reachable();
        let (ctx, _, _, _) = probe_context_with_l2(
            RegistryBehavior::Success,
            false,
            L2Fixture::Connected(Arc::clone(&backend)),
        );
        let readiness = Readiness::new();

        probe_once(&ctx, &readiness, Duration::from_secs(1)).await;

        assert_eq!(readiness.status(), ReadinessStatus::Ready);
        assert_eq!(
            readiness.l2(),
            L2Report::Available {
                backend: "valkey".to_string()
            }
        );
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            1,
            "the tier must be probed exactly once per readiness cadence tick"
        );
        let (status, body) = readyz_body(&readiness).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.is_empty(), "a healthy tier reports nothing: {body}");
    }

    /// The decisive test for both halves of `#161`'s truthfulness claim.
    ///
    /// It fails if readiness reports a bare "ready" while a configured cache
    /// is down (the body would be empty), and it fails if a down cache is
    /// allowed to make the process unready (the status would be `Failed`
    /// and the response a `503`). It also fails if the report is a generic
    /// "degraded" that does not name the component and backend.
    #[tokio::test]
    async fn an_unreachable_configured_l2_stays_ready_and_names_the_backend() {
        let (ctx, _, _, _) = probe_context_with_l2(
            RegistryBehavior::Success,
            false,
            L2Fixture::Connected(TestL2::unreachable()),
        );
        let readiness = Readiness::new();

        probe_once(&ctx, &readiness, Duration::from_secs(1)).await;

        assert_eq!(
            readiness.status(),
            ReadinessStatus::Ready,
            "a cache outage must never pull a still-correct replica out of the load balancer"
        );
        assert_eq!(
            readiness.l2(),
            L2Report::Unavailable {
                backend: "valkey".to_string(),
                reason: L2Unavailable::Unreachable,
            }
        );
        let (status, body) = readyz_body(&readiness).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            r#"{"status":"degraded","degradations":[{"component":"cache.l2","backend":"valkey","reason":"unreachable"}]}"#
        );
    }

    /// The boot-down case. A tier that never connected is a configured tier,
    /// reported by name — never collapsed into the same silence an
    /// unconfigured deployment gets. Contrast with
    /// `an_unconfigured_l2_is_never_probed_and_leaves_readiness_byte_identical`:
    /// same L1-only serving stack, deliberately different report.
    #[tokio::test]
    async fn a_never_connected_l2_is_named_rather_than_reported_as_absent() {
        let (ctx, _, _, _) =
            probe_context_with_l2(RegistryBehavior::Success, false, L2Fixture::NeverConnected);
        let readiness = Readiness::new();

        probe_once(&ctx, &readiness, Duration::from_secs(1)).await;

        assert_eq!(readiness.status(), ReadinessStatus::Ready);
        assert_eq!(
            readiness.l2(),
            L2Report::Unavailable {
                backend: "valkey".to_string(),
                reason: L2Unavailable::NeverConnected,
            },
            "a configured tier that never connected must not look like no tier"
        );
        let (_, body) = readyz_body(&readiness).await;
        assert!(
            body.contains(r#""reason":"never-connected-at-boot""#),
            "the boot-down case needs its own reason code: {body}"
        );
    }

    /// Outage and recovery on the readiness cadence: the report follows the
    /// backend both ways, and readiness never moves in either direction.
    #[tokio::test]
    async fn an_l2_outage_and_recovery_move_only_the_report_never_readiness() {
        let backend = TestL2::reachable();
        let (ctx, _, _, _) = probe_context_with_l2(
            RegistryBehavior::Success,
            false,
            L2Fixture::Connected(Arc::clone(&backend)),
        );
        let readiness = Readiness::new();

        probe_once(&ctx, &readiness, Duration::from_secs(1)).await;
        assert!(matches!(readiness.l2(), L2Report::Available { .. }));
        assert_eq!(readiness.status(), ReadinessStatus::Ready);

        backend.set_unreachable(true);
        probe_once(&ctx, &readiness, Duration::from_secs(1)).await;
        assert!(matches!(readiness.l2(), L2Report::Unavailable { .. }));
        assert_eq!(readiness.status(), ReadinessStatus::Ready);

        backend.set_unreachable(false);
        probe_once(&ctx, &readiness, Duration::from_secs(1)).await;
        assert_eq!(
            readiness.l2(),
            L2Report::Available {
                backend: "valkey".to_string()
            },
            "a recovered tier must clear its degradation with no restart"
        );
        assert_eq!(readiness.status(), ReadinessStatus::Ready);
        let (_, body) = readyz_body(&readiness).await;
        assert!(body.is_empty(), "recovery must clear the body: {body}");
    }

    /// A wedged cache backend is the case where "just add it to the existing
    /// probe" would have silently made readiness false: sharing one deadline
    /// would let the cache consume it and time the whole probe out. The two
    /// probes are separately deadlined, so the mandatory verdict stands on
    /// its own and the cache is reported as timing out.
    #[tokio::test]
    async fn a_wedged_l2_backend_cannot_consume_the_mandatory_probe_deadline() {
        let (ctx, registry_calls, _, storage_calls) = probe_context_with_l2(
            RegistryBehavior::Success,
            false,
            L2Fixture::Connected(TestL2::wedged()),
        );
        let readiness = Readiness::new();

        probe_once(&ctx, &readiness, Duration::from_millis(50)).await;

        assert_eq!(readiness.status(), ReadinessStatus::Ready);
        assert_eq!(registry_calls.load(Ordering::SeqCst), 2);
        assert_eq!(storage_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            readiness.l2(),
            L2Report::Unavailable {
                backend: "valkey".to_string(),
                reason: L2Unavailable::ProbeTimedOut,
            }
        );
    }

    /// A cache degradation must not soften a real dependency failure: the
    /// `503` keeps its existing problem+json shape and reveals nothing about
    /// the cache either.
    #[tokio::test]
    async fn an_unreachable_l2_does_not_mask_or_alter_a_mandatory_probe_failure() {
        let (ctx, _, _, _) = probe_context_with_l2(
            RegistryBehavior::Failure,
            false,
            L2Fixture::Connected(TestL2::unreachable()),
        );
        let readiness = Readiness::new();

        probe_once(&ctx, &readiness, Duration::from_secs(1)).await;

        assert_eq!(readiness.status(), ReadinessStatus::Failed);
        let (status, body) = readyz_body(&readiness).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            !body.contains("valkey"),
            "the 503 body stays generic: {body}"
        );
        assert!(body.contains("server is not ready"), "{body}");
    }
}
