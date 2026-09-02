//! Per-tenant admission control middleware (`#66`): adapts
//! `tellurion_core::AdmissionRegistry` — the bounded queue, fair-share
//! gates, and metrics — to axum's `Extension`/`Path`/`Next` shape. The
//! registry itself is entirely `tellurion-core`'s concern (driver-
//! independent, no axum dependency at all); this module only wires it into
//! the request path and renders a rejection as the same shared RFC 9457
//! problem+json body every other error path on this server uses.
//!
//! See `app.rs`'s own module doc for where this sits in the full
//! middleware order, and `app::build_with_readiness` for where the
//! registry itself is built once and reconfigured from each atomically
//! published tenant snapshot, using the same global concurrency ceiling
//! every other admission-adjacent knob there reads.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use axum::Extension;

use tellurion_core::{
    AdmissionOutcome, AdmissionRegistry, AdmissionRejection, AppContext, ContextState,
};

use crate::app::problem_response;

/// Layered onto `tenant_scope` (`app::build_with_readiness`), the same
/// scope `enforce_tenant_auth` covers: every route nested under
/// `/{tenant}` queues (bounded, with a deadline) or is honestly rejected
/// here before authorization or a protocol handler ever runs. Reserved
/// top-level segments (`/`, `/metrics`, `/config/effective`, `/healthz`,
/// `/readyz`) live outside `tenant_scope` and never see this layer.
///
/// An unresolvable tenant external id passes through unconditionally — the
/// same "nothing to admit for a tenant that doesn't exist" precedent
/// `enforce_tenant_auth` documents; the eventual handler still answers 404,
/// and admission enforcement never changes the shape of that response.
pub(crate) async fn enforce_tenant_admission(
    Extension(registries): Extension<Arc<ReloadableAdmissionRegistry>>,
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    request: Request,
    next: Next,
) -> Response {
    let state = ctx.current();
    let Some(tenant_ext) = params.get("tenant") else {
        return next.run(request).await;
    };
    let Ok(tenant_id) = state.resolver.resolve_tenant(tenant_ext).await else {
        return next.run(request).await;
    };
    let registry = registries.for_state(&ctx, &state);

    match registry.admit(&tenant_id).await {
        AdmissionOutcome::Admitted(_permit) => next.run(request).await,
        AdmissionOutcome::Rejected(reason) => rejection_response(reason),
    }
}

/// Keeps admission gates generation-aligned with the atomically swapped
/// [`ContextState`] while retaining one stable registry across generations.
/// A current state reconfigures the existing per-tenant gates in place;
/// an in-flight request holding an older state can use those gates but can
/// never roll their configuration back after a newer state was published.
pub(crate) struct ReloadableAdmissionRegistry {
    global_ceiling: usize,
    registry: Arc<AdmissionRegistry>,
    configured_state: Mutex<Weak<ContextState>>,
}

impl ReloadableAdmissionRegistry {
    pub(crate) fn new(global_ceiling: usize) -> Self {
        Self {
            global_ceiling,
            registry: Arc::new(AdmissionRegistry::build(
                &[],
                &Default::default(),
                global_ceiling,
                &[],
            )),
            configured_state: Mutex::new(Weak::new()),
        }
    }

    fn for_state(
        &self,
        ctx: &AppContext,
        observed_state: &Arc<ContextState>,
    ) -> Arc<AdmissionRegistry> {
        let current_state = ctx.current();
        if !Arc::ptr_eq(&current_state, observed_state) {
            return Arc::clone(&self.registry);
        }

        let mut configured = self
            .configured_state
            .lock()
            .expect("admission state lock poisoned");
        let current_state = ctx.current();
        if !Arc::ptr_eq(&current_state, observed_state) {
            return Arc::clone(&self.registry);
        }
        if configured
            .upgrade()
            .is_some_and(|candidate| Arc::ptr_eq(&candidate, observed_state))
        {
            return Arc::clone(&self.registry);
        }
        self.registry.reconfigure(
            &observed_state.tenants,
            &observed_state.config.settings,
            self.global_ceiling,
            &observed_state.config.server.metrics_tenant_allowlist,
        );
        *configured = Arc::downgrade(observed_state);
        Arc::clone(&self.registry)
    }
}

/// Honest backpressure (`#66`): every rejection carries `Retry-After` and
/// the shared problem+json body, naming which bound was actually hit
/// rather than a generic "server is at capacity" — distinct from the
/// tower-level load-shed 503 (`app::LOAD_SHED_BODY`), which fires only once
/// the global concurrency ceiling itself is exhausted, a coarser and rarer
/// condition than one tenant's own small queue filling up or timing out.
fn rejection_response(reason: AdmissionRejection) -> Response {
    let detail = match reason {
        AdmissionRejection::QueueFull => {
            "this tenant's admission queue is at capacity; retry shortly"
        }
        AdmissionRejection::DeadlineExpired => {
            "this tenant's request exceeded the admission queue deadline; retry shortly"
        }
    };
    let mut response = problem_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "ServiceUnavailable",
        detail,
    );
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::Router;
    use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
    use tokio::sync::watch;
    use tower::ServiceExt;

    use tellurion_core::{
        AppConfig, CatalogSource, CollectionDecl, DriverFactory, FeaturePage, FeatureSource,
        FileStyleStore, Filter, ItemsQuery, MokaTileCache, PhysicalCollection, Registry, Resolver,
        Result as CoreResult, Router as CoreRouter, StaticResolver, StorageDecl, StorageDriver,
        StyleStore, TileCache,
    };

    struct EmptyCatalog;

    #[async_trait::async_trait]
    impl CatalogSource for EmptyCatalog {
        async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
            Ok(vec![])
        }
    }

    /// A `FeatureSource` whose `items` call blocks until the test flips
    /// `release` to `true` — a `watch` channel rather than a plain
    /// `Notify` because several calls may still be arriving *after* the
    /// test releases the gate (once a queued admission finally gets a
    /// slot), and `watch::Receiver::changed` correctly observes an
    /// already-published value instead of missing it the way a `Notify`
    /// permit (good for exactly one future waiter) would.
    struct GatedBackend {
        entered: Arc<AtomicUsize>,
        release: watch::Receiver<bool>,
    }

    #[async_trait::async_trait]
    impl FeatureSource for GatedBackend {
        async fn items(
            &self,
            _collection: &CollectionDecl,
            _query: &ItemsQuery,
        ) -> CoreResult<FeaturePage> {
            self.entered.fetch_add(1, Ordering::SeqCst);
            let mut rx = self.release.clone();
            while !*rx.borrow() {
                if rx.changed().await.is_err() {
                    break;
                }
            }
            Ok(FeaturePage {
                features_geojson: vec![],
                number_matched: Some(0),
                next_token: None,
            })
        }

        async fn item(
            &self,
            _collection: &CollectionDecl,
            _id: &str,
            _filter: Option<&Filter>,
        ) -> CoreResult<Option<serde_json::Value>> {
            Ok(None)
        }
    }

    struct GatedDriver {
        entered: Arc<AtomicUsize>,
        release: watch::Receiver<bool>,
    }

    impl StorageDriver for GatedDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(EmptyCatalog)
        }

        fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
            Some(Arc::new(GatedBackend {
                entered: Arc::clone(&self.entered),
                release: self.release.clone(),
            }))
        }
    }

    struct GatedFactory {
        entered: Arc<AtomicUsize>,
        release: watch::Receiver<bool>,
    }

    impl DriverFactory for GatedFactory {
        fn name(&self) -> &str {
            "gated"
        }

        fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
            Ok(Arc::new(GatedDriver {
                entered: Arc::clone(&self.entered),
                release: self.release.clone(),
            }))
        }
    }

    /// Tenant b's backend: returns immediately, so its requests prove
    /// admission rather than measuring backend latency.
    struct FastBackend;

    #[async_trait::async_trait]
    impl FeatureSource for FastBackend {
        async fn items(
            &self,
            _collection: &CollectionDecl,
            _query: &ItemsQuery,
        ) -> CoreResult<FeaturePage> {
            Ok(FeaturePage {
                features_geojson: vec![],
                number_matched: Some(0),
                next_token: None,
            })
        }

        async fn item(
            &self,
            _collection: &CollectionDecl,
            _id: &str,
            _filter: Option<&Filter>,
        ) -> CoreResult<Option<serde_json::Value>> {
            Ok(None)
        }
    }

    struct FastDriver;

    impl StorageDriver for FastDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(EmptyCatalog)
        }

        fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
            Some(Arc::new(FastBackend))
        }
    }

    struct FastFactory;

    impl DriverFactory for FastFactory {
        fn name(&self) -> &str {
            "fast"
        }

        fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
            Ok(Arc::new(FastDriver))
        }
    }

    fn test_metrics_handle() -> PrometheusHandle {
        PrometheusBuilder::new().build_recorder().handle()
    }

    const PATH_A: &str = "/a/features/catalogs/catalog_a/collections/collection_a/items";
    const PATH_B: &str = "/b/features/catalogs/catalog_b/collections/collection_b/items";

    /// Two tenants: `a` (external `a`) gets a small, bounded admission
    /// queue (`queue_capacity: 2`) and a low fair-share weight in front of
    /// a backend this test fully controls; `b` (external `b`) gets a much
    /// higher weight and a backend that never blocks. `server.
    /// max_concurrency` (20) is set well above anything this test ever has
    /// in flight at once (peak: 2 admitted + 2 queued + 1 rejected for `a`,
    /// plus 1 for `b` — 6), so the pre-existing tower-level `concurrency_
    /// limit`/`load_shed` (`app.rs`) never fires; every rejection this test
    /// observes comes from the per-tenant admission layer alone. Weight 1
    /// vs 9 out of a total of 10 gives `a` a fair share of exactly 2 slots
    /// (`floor(20*1/10)`) — small enough to fill with a handful of
    /// requests — and `b` 18, far more than its single request ever needs.
    fn build_fixture(entered: Arc<AtomicUsize>, release: watch::Receiver<bool>) -> Router {
        let config: AppConfig = serde_yaml::from_str(
            r#"
server:
  max_concurrency: 20
storages:
  - { id: storage_a, driver: gated, url_env: DATABASE_URL }
  - { id: storage_b, driver: fast, url_env: DATABASE_URL }
tenants:
  - id: tenant_a
    external_id: a
    settings:
      admission: { queue_capacity: 2, queue_deadline_ms: 60000, weight: 1 }
  - id: tenant_b
    external_id: b
    settings:
      admission: { weight: 9 }
catalogs:
  - { id: catalog_a, tenant: tenant_a }
  - { id: catalog_b, tenant: tenant_b }
collections:
  - id: collection_a
    catalog: catalog_a
    storage: storage_a
    table: demo_a
    geometry: geom
    pk: id
  - id: collection_b
    catalog: catalog_b
    storage: storage_b
    table: demo_b
    geometry: geom
    pk: id
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let mut driver_registry = Registry::new();
        driver_registry.register(Arc::new(GatedFactory { entered, release }));
        driver_registry.register(Arc::new(FastFactory));
        let router = CoreRouter::build(&config, &driver_registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            cache,
            style_store,
        ));
        crate::app::build_with_readiness(
            ctx,
            test_metrics_handle(),
            60,
            crate::readiness::Readiness::new(),
        )
    }

    /// The acceptance test for `#66`: tenant `a` floods past its own fair
    /// share and bounded queue while tenant `b` — sharing nothing but the
    /// same global ceiling — is admitted immediately regardless. Every
    /// assertion is about counts and outcomes (how many of `a`'s requests
    /// were admitted/queued/rejected, whether `b` succeeded at all) rather
    /// than wall-clock latency, and every wait is either immediate
    /// (synchronous, no waiting at all) or released explicitly by the test
    /// — nothing here depends on real elapsed time, so it stays robust on a
    /// busy, shared build host. The whole test is wrapped in a generous
    /// bounded timeout as a safety net against a genuine hang, not as a
    /// performance assertion.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn tenant_b_is_admitted_immediately_while_tenant_a_floods_and_queues() {
        let entered = Arc::new(AtomicUsize::new(0));
        let (release_tx, release_rx) = watch::channel(false);
        let app = build_fixture(Arc::clone(&entered), release_rx);

        let send_a = || {
            let app = app.clone();
            tokio::spawn(async move {
                app.oneshot(
                    HttpRequest::builder()
                        .uri(PATH_A)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            })
        };

        // Fill tenant a's fair share (2 slots) — both calls enter the
        // gated backend and block there.
        let flood_admitted = vec![send_a(), send_a()];
        let barrier_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while entered.load(Ordering::SeqCst) < 2 {
            assert!(
                tokio::time::Instant::now() < barrier_deadline,
                "the first two tenant-a requests never reached the backend"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // Two more fill tenant a's bounded queue (queue_capacity: 2) —
        // these do not error, they simply wait for a slot.
        let flood_queued = vec![send_a(), send_a()];
        // Give them a moment to actually reach the admission layer and
        // register as queued before the next, over-capacity request is
        // sent — a fixed short sleep here only orders test setup, it never
        // gates a pass/fail assertion.
        tokio::time::sleep(Duration::from_millis(20)).await;

        // A fifth tenant-a request arrives once the queue is already full:
        // rejected immediately, no waiting at all.
        let over_capacity = tokio::time::timeout(Duration::from_millis(200), send_a())
            .await
            .expect("a queue-full rejection must return promptly, not hang")
            .unwrap();
        assert_eq!(over_capacity.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            over_capacity.headers().get(header::RETRY_AFTER).unwrap(),
            "1"
        );

        // Tenant b, sharing nothing but the same global ceiling, is
        // admitted immediately despite tenant a's flood and full queue.
        let b_response = tokio::time::timeout(
            Duration::from_millis(200),
            app.clone().oneshot(
                HttpRequest::builder()
                    .uri(PATH_B)
                    .body(Body::empty())
                    .unwrap(),
            ),
        )
        .await
        .expect("tenant b must not be blocked by tenant a's exhausted share")
        .unwrap();
        assert_eq!(b_response.status(), StatusCode::OK);

        // Release tenant a's backend: the two admitted requests complete,
        // freeing their slots for the two queued requests to acquire in
        // turn.
        release_tx.send(true).unwrap();

        for handle in flood_admitted.into_iter().chain(flood_queued) {
            let response = tokio::time::timeout(Duration::from_secs(5), handle)
                .await
                .expect("every tenant-a request should resolve once its slot frees")
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
    }

    #[tokio::test]
    async fn admission_reload_preserves_in_flight_fair_share_capacity() {
        let config: AppConfig = serde_yaml::from_str(
            "tenants:\n  - id: tenant-a\n    settings:\n      admission: { queue_capacity: 1, weight: 1 }\n",
        )
        .unwrap();
        config.validate().unwrap();
        let router = CoreRouter::build(&config, &Registry::new()).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000));
        let styles: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = AppContext::new(config.clone(), router, resolver, None, cache, styles);
        let reloadable = ReloadableAdmissionRegistry::new(1);

        let before = ctx.current();
        let before_registry = reloadable.for_state(&ctx, &before);
        let first = before_registry.admit("tenant-a").await;
        let AdmissionOutcome::Admitted(held_before_reload) = first else {
            panic!("the first request should consume the old generation's only slot");
        };

        let mut reloaded = config;
        reloaded.tenants[0].external_id = Some("tenant-a-renamed".to_string());
        reloaded.tenants[0]
            .settings
            .admission
            .as_mut()
            .expect("the test tenant has admission settings")
            .queue_capacity = Some(0);
        let router = CoreRouter::build(&reloaded, &Registry::new()).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&reloaded));
        ctx.reload(reloaded, router, resolver, None);

        let after = ctx.current();
        let after_registry = reloadable.for_state(&ctx, &after);
        let stale_registry = reloadable.for_state(&ctx, &before);
        assert!(
            Arc::ptr_eq(&before_registry, &after_registry),
            "reload must reconfigure one stable registry rather than double capacity"
        );
        assert!(
            Arc::ptr_eq(&after_registry, &stale_registry),
            "a request holding the old state must not restore its queue settings"
        );
        assert!(matches!(
            after_registry.admit("tenant-a").await,
            AdmissionOutcome::Rejected(AdmissionRejection::QueueFull)
        ));
        drop(held_before_reload);
        assert!(matches!(
            after_registry.admit("tenant-a").await,
            AdmissionOutcome::Admitted(_)
        ));
    }
}
