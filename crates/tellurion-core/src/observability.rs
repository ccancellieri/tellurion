use std::cell::RefCell;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tokio::time::Instant;

use crate::config::{CatalogDecl, CollectionDecl};
use crate::crs::RequestedCrs;
use crate::error::Result;
use crate::filter::Filter;
use crate::registry::{Page, PageRequest, RegistryReader};
use crate::resolver::Resolver;
use crate::storage::{
    FeaturePage, FeatureSource, ItemsQuery, TileCoord, TileSource, VolumeMesh, VolumeSource,
};
use crate::style_store::StyleStore;

tokio::task_local! {
    static REQUEST_PHASES: RefCell<PhaseState>;
}

/// A bounded request-processing phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Routing,
    Query,
    Cache,
    Encode,
}

/// Exclusive request time accumulated by the measured phases.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PhaseSnapshot {
    routing: Duration,
    query: Duration,
    cache: Duration,
}

impl PhaseSnapshot {
    pub const fn routing(&self) -> Duration {
        self.routing
    }

    pub const fn query(&self) -> Duration {
        self.query
    }

    pub const fn cache(&self) -> Duration {
        self.cache
    }

    pub fn encode(&self, total: Duration) -> Duration {
        total.saturating_sub(
            self.routing
                .saturating_add(self.query)
                .saturating_add(self.cache),
        )
    }

    fn record(&mut self, phase: Phase, elapsed: Duration) {
        let target = match phase {
            Phase::Routing => Some(&mut self.routing),
            Phase::Query => Some(&mut self.query),
            Phase::Cache => Some(&mut self.cache),
            Phase::Encode => None,
        };

        if let Some(target) = target {
            *target = target.saturating_add(elapsed);
        }
    }
}

#[derive(Debug)]
struct PhaseFrame {
    id: u64,
    phase: Phase,
    started: Instant,
    child_time: Duration,
}

#[derive(Debug, Default)]
struct PhaseState {
    next_id: u64,
    frames: Vec<PhaseFrame>,
    snapshot: PhaseSnapshot,
}

impl PhaseState {
    fn enter(&mut self, phase: Phase) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.frames.push(PhaseFrame {
            id,
            phase,
            started: Instant::now(),
            child_time: Duration::ZERO,
        });
        id
    }

    fn exit(&mut self, id: u64) {
        let Some(frame) = self.frames.pop() else {
            return;
        };

        if frame.id != id {
            self.frames.push(frame);
            return;
        }

        let elapsed = frame.started.elapsed();
        self.snapshot
            .record(frame.phase, elapsed.saturating_sub(frame.child_time));

        if let Some(parent) = self.frames.last_mut() {
            parent.child_time = parent.child_time.saturating_add(elapsed);
        }
    }

    fn finish(&mut self) -> PhaseSnapshot {
        while let Some(frame) = self.frames.last() {
            self.exit(frame.id);
        }
        self.snapshot
    }
}

#[derive(Debug)]
struct PhaseGuard {
    id: Option<u64>,
}

impl Drop for PhaseGuard {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };

        let _ = REQUEST_PHASES.try_with(|state| state.borrow_mut().exit(id));
    }
}

/// Runs one request with a fresh phase collector and returns its snapshot.
pub async fn scope_request<F>(future: F) -> (F::Output, PhaseSnapshot)
where
    F: Future,
{
    REQUEST_PHASES
        .scope(RefCell::new(PhaseState::default()), async move {
            let output = future.await;
            let snapshot = REQUEST_PHASES.with(|state| state.borrow_mut().finish());
            (output, snapshot)
        })
        .await
}

/// Runs a future in a phase when request collection is active.
pub async fn in_phase<F>(phase: Phase, future: F) -> F::Output
where
    F: Future,
{
    let _guard = enter_phase(phase);
    future.await
}

/// Enters a synchronous phase when request collection is active.
#[must_use = "the phase guard must be held for the work being measured"]
pub fn enter_phase(phase: Phase) -> impl Drop {
    let id = REQUEST_PHASES
        .try_with(|state| state.borrow_mut().enter(phase))
        .ok();
    PhaseGuard { id }
}

#[cfg(test)]
pub(crate) fn active_phase_depth() -> usize {
    REQUEST_PHASES
        .try_with(|state| state.borrow().frames.len())
        .unwrap_or(0)
}

struct ObservedResolver {
    inner: Arc<dyn Resolver>,
}

pub(crate) fn observe_resolver(inner: Arc<dyn Resolver>) -> Arc<dyn Resolver> {
    Arc::new(ObservedResolver { inner })
}

#[async_trait::async_trait]
impl Resolver for ObservedResolver {
    async fn resolve_tenant(&self, tenant_external_id: &str) -> Result<String> {
        in_phase(
            Phase::Routing,
            self.inner.resolve_tenant(tenant_external_id),
        )
        .await
    }

    async fn resolve_catalog(
        &self,
        tenant_internal_id: &str,
        catalog_external_id: &str,
    ) -> Result<String> {
        in_phase(
            Phase::Routing,
            self.inner
                .resolve_catalog(tenant_internal_id, catalog_external_id),
        )
        .await
    }

    async fn resolve_collection(
        &self,
        catalog_internal_id: &str,
        collection_external_id: &str,
    ) -> Result<String> {
        in_phase(
            Phase::Routing,
            self.inner
                .resolve_collection(catalog_internal_id, collection_external_id),
        )
        .await
    }

    fn tenant_external_id(&self, tenant_internal_id: &str) -> Option<&str> {
        let _phase = enter_phase(Phase::Routing);
        self.inner.tenant_external_id(tenant_internal_id)
    }

    fn catalog_external_id(&self, catalog_internal_id: &str) -> Option<&str> {
        let _phase = enter_phase(Phase::Routing);
        self.inner.catalog_external_id(catalog_internal_id)
    }

    fn collection_external_id(&self, collection_internal_id: &str) -> Option<&str> {
        let _phase = enter_phase(Phase::Routing);
        self.inner.collection_external_id(collection_internal_id)
    }

    fn catalogs_for_tenant(&self, tenant_internal_id: &str) -> Vec<(&str, &str)> {
        let _phase = enter_phase(Phase::Routing);
        self.inner.catalogs_for_tenant(tenant_internal_id)
    }

    fn catalog_count(&self) -> usize {
        let _phase = enter_phase(Phase::Routing);
        self.inner.catalog_count()
    }
}

struct ObservedRegistry {
    inner: Arc<dyn RegistryReader>,
}

pub(crate) fn observe_registry(inner: Arc<dyn RegistryReader>) -> Arc<dyn RegistryReader> {
    Arc::new(ObservedRegistry { inner })
}

#[async_trait::async_trait]
impl RegistryReader for ObservedRegistry {
    async fn catalog(
        &self,
        tenant_internal_id: &str,
        catalog_external_id: &str,
    ) -> Result<Option<CatalogDecl>> {
        in_phase(
            Phase::Query,
            self.inner.catalog(tenant_internal_id, catalog_external_id),
        )
        .await
    }

    async fn collection(
        &self,
        catalog_internal_id: &str,
        collection_external_id: &str,
    ) -> Result<Option<CollectionDecl>> {
        in_phase(
            Phase::Query,
            self.inner
                .collection(catalog_internal_id, collection_external_id),
        )
        .await
    }

    async fn list_catalogs(
        &self,
        tenant_internal_id: &str,
        page: PageRequest,
    ) -> Result<Page<CatalogDecl>> {
        in_phase(
            Phase::Query,
            self.inner.list_catalogs(tenant_internal_id, page),
        )
        .await
    }

    async fn list_collections(
        &self,
        catalog_internal_id: &str,
        page: PageRequest,
    ) -> Result<Page<CollectionDecl>> {
        in_phase(
            Phase::Query,
            self.inner.list_collections(catalog_internal_id, page),
        )
        .await
    }
}

struct ObservedStyleStore {
    inner: Arc<dyn StyleStore>,
}

pub(crate) fn observe_style_store(inner: Arc<dyn StyleStore>) -> Arc<dyn StyleStore> {
    Arc::new(ObservedStyleStore { inner })
}

impl StyleStore for ObservedStyleStore {
    fn load(&self, id: &str) -> Result<Option<serde_json::Value>> {
        let _phase = enter_phase(Phase::Query);
        self.inner.load(id)
    }

    fn list(&self) -> Result<Vec<String>> {
        let _phase = enter_phase(Phase::Query);
        self.inner.list()
    }
}

struct ObservedFeatureSource {
    inner: Arc<dyn FeatureSource>,
}

pub(crate) fn observe_feature_source(inner: Arc<dyn FeatureSource>) -> Arc<dyn FeatureSource> {
    Arc::new(ObservedFeatureSource { inner })
}

#[async_trait::async_trait]
impl FeatureSource for ObservedFeatureSource {
    async fn items(&self, collection: &CollectionDecl, query: &ItemsQuery) -> Result<FeaturePage> {
        in_phase(Phase::Query, self.inner.items(collection, query)).await
    }

    async fn item(
        &self,
        collection: &CollectionDecl,
        id: &str,
        filter: Option<&Filter>,
    ) -> Result<Option<serde_json::Value>> {
        in_phase(Phase::Query, self.inner.item(collection, id, filter)).await
    }

    fn filter_capable(&self) -> bool {
        let _phase = enter_phase(Phase::Query);
        self.inner.filter_capable()
    }

    /// Forwards verbatim (`#105`) — an observed source must report the exact
    /// same declared classes its `inner` does; leaving this at the trait
    /// default here would silently report every wrapped driver as
    /// CQL2-incapable regardless of what it actually compiles.
    fn cql2_conformance_classes(&self) -> Vec<&'static str> {
        let _phase = enter_phase(Phase::Query);
        self.inner.cql2_conformance_classes()
    }

    fn crs_capable(&self) -> bool {
        let _phase = enter_phase(Phase::Query);
        self.inner.crs_capable()
    }

    /// Forwards verbatim (`#217`) for the same reason
    /// `cql2_conformance_classes` above does: left at the trait default this
    /// would report every wrapped driver as unable to honour `filter-crs`,
    /// silently folding OGC API — Features Part 3 out of `/conformance` for
    /// a deployment whose driver genuinely honours it.
    fn filter_crs_capable(&self) -> bool {
        let _phase = enter_phase(Phase::Query);
        self.inner.filter_crs_capable()
    }

    async fn item_with_crs(
        &self,
        collection: &CollectionDecl,
        id: &str,
        filter: Option<&Filter>,
        requested_crs: RequestedCrs,
    ) -> Result<Option<serde_json::Value>> {
        in_phase(
            Phase::Query,
            self.inner
                .item_with_crs(collection, id, filter, requested_crs),
        )
        .await
    }
}

struct ObservedTileSource {
    inner: Arc<dyn TileSource>,
}

pub(crate) fn observe_tile_source(inner: Arc<dyn TileSource>) -> Arc<dyn TileSource> {
    Arc::new(ObservedTileSource { inner })
}

#[async_trait::async_trait]
impl TileSource for ObservedTileSource {
    async fn mvt_tile(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
        filter: Option<&Filter>,
    ) -> Result<Option<Bytes>> {
        in_phase(Phase::Query, self.inner.mvt_tile(collection, coord, filter)).await
    }

    fn tile_capable(&self, collection: &CollectionDecl) -> bool {
        let _phase = enter_phase(Phase::Query);
        self.inner.tile_capable(collection)
    }

    /// `#190`: delegated explicitly — the trait default would otherwise
    /// answer for this wrapper (native grid only), silently masking a
    /// wrapped PostGIS source's wider answer.
    fn supports_tile_matrix_set(&self, tms: crate::tms::TileMatrixSet) -> bool {
        self.inner.supports_tile_matrix_set(tms)
    }

    /// Same delegation obligation as `supports_tile_matrix_set` above: the
    /// trait default would refuse every non-native grid regardless of what
    /// `inner` can actually serve.
    async fn mvt_tile_in(
        &self,
        collection: &CollectionDecl,
        tms: crate::tms::TileMatrixSet,
        coord: TileCoord,
        filter: Option<&Filter>,
    ) -> Result<Option<Bytes>> {
        in_phase(
            Phase::Query,
            self.inner.mvt_tile_in(collection, tms, coord, filter),
        )
        .await
    }

    fn filter_capable(&self) -> bool {
        let _phase = enter_phase(Phase::Query);
        self.inner.filter_capable()
    }

    async fn vector_layers(&self, collection: &CollectionDecl) -> Result<Option<Vec<String>>> {
        in_phase(Phase::Query, self.inner.vector_layers(collection)).await
    }
}

struct ObservedVolumeSource {
    inner: Arc<dyn VolumeSource>,
}

pub(crate) fn observe_volume_source(inner: Arc<dyn VolumeSource>) -> Arc<dyn VolumeSource> {
    Arc::new(ObservedVolumeSource { inner })
}

#[async_trait::async_trait]
impl VolumeSource for ObservedVolumeSource {
    async fn volume_tile(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
        filter: Option<&Filter>,
    ) -> Result<Option<VolumeMesh>> {
        in_phase(
            Phase::Query,
            self.inner.volume_tile(collection, coord, filter),
        )
        .await
    }

    fn filter_capable(&self) -> bool {
        let _phase = enter_phase(Phase::Query);
        self.inner.filter_capable()
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;
    use std::panic::AssertUnwindSafe;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::Poll;
    use std::time::Duration;

    use bytes::Bytes;
    use futures::FutureExt;
    use tokio::time::advance;

    use super::*;
    use crate::config::{CatalogDecl, CollectionDecl};
    use crate::crs::RequestedCrs;
    use crate::error::{Error, Result};
    use crate::filter::{parse_text, Filter};
    use crate::registry::{Page, PageRequest, RegistryReader};
    use crate::resolver::Resolver;
    use crate::storage::{
        FeaturePage, FeatureSource, ItemsQuery, TileCoord, TileSource, VolumeMesh, VolumeSource,
    };
    use crate::style_store::StyleStore;

    #[tokio::test(start_paused = true)]
    async fn nested_phases_record_exclusive_time() {
        let (value, snapshot) = scope_request(async {
            in_phase(Phase::Cache, async {
                advance(Duration::from_millis(2)).await;
                in_phase(Phase::Query, advance(Duration::from_millis(3))).await;
                advance(Duration::from_millis(5)).await;
            })
            .await;

            "response"
        })
        .await;

        assert_eq!(value, "response");
        assert_eq!(snapshot.routing(), Duration::ZERO);
        assert_eq!(snapshot.query(), Duration::from_millis(3));
        assert_eq!(snapshot.cache(), Duration::from_millis(7));
        assert_eq!(snapshot.encode(Duration::from_millis(10)), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn an_error_drops_its_phase_before_following_work() {
        let (_, snapshot) = scope_request(async {
            let result: std::result::Result<(), &str> = in_phase(Phase::Cache, async {
                advance(Duration::from_millis(2)).await;
                Err("query failed")
            })
            .await;

            assert_eq!(result, Err("query failed"));
            in_phase(Phase::Routing, advance(Duration::from_millis(3))).await;
        })
        .await;

        assert_eq!(snapshot.routing(), Duration::from_millis(3));
        assert_eq!(snapshot.query(), Duration::ZERO);
        assert_eq!(snapshot.cache(), Duration::from_millis(2));
    }

    #[tokio::test(start_paused = true)]
    async fn a_panicking_nested_phase_restores_its_parent() {
        let (_, snapshot) = scope_request(async {
            let parent = enter_phase(Phase::Cache);
            advance(Duration::from_millis(1)).await;

            let panic = AssertUnwindSafe(in_phase(Phase::Query, async {
                advance(Duration::from_millis(2)).await;
                panic!("phase failed");
            }))
            .catch_unwind()
            .await;

            assert!(panic.is_err());
            advance(Duration::from_millis(3)).await;
            drop(parent);
        })
        .await;

        assert_eq!(snapshot.query(), Duration::from_millis(2));
        assert_eq!(snapshot.cache(), Duration::from_millis(4));
    }

    #[tokio::test(start_paused = true)]
    async fn phase_calls_are_no_ops_outside_a_request_scope() {
        let value = in_phase(Phase::Encode, async {
            advance(Duration::from_millis(1)).await;
            42
        })
        .await;

        let guard = enter_phase(Phase::Routing);
        advance(Duration::from_millis(1)).await;
        drop(guard);

        assert_eq!(value, 42);
    }

    #[tokio::test(start_paused = true)]
    async fn encode_residual_saturates_at_zero() {
        let (_, snapshot) =
            scope_request(in_phase(Phase::Routing, advance(Duration::from_millis(10)))).await;

        assert_eq!(snapshot.encode(Duration::from_millis(5)), Duration::ZERO);
        assert_eq!(
            snapshot.encode(Duration::from_millis(15)),
            Duration::from_millis(5)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_a_request_scope_leaves_no_task_local_state() {
        let mut request = Box::pin(scope_request(in_phase(Phase::Cache, pending::<()>())));
        assert!(matches!(futures::poll!(request.as_mut()), Poll::Pending));

        drop(request);

        assert!(REQUEST_PHASES.try_with(|_| ()).is_err());
        let (_, snapshot) =
            scope_request(in_phase(Phase::Query, advance(Duration::from_millis(3)))).await;
        assert_eq!(snapshot.routing(), Duration::ZERO);
        assert_eq!(snapshot.query(), Duration::from_millis(3));
        assert_eq!(snapshot.cache(), Duration::ZERO);
    }

    #[test]
    fn request_scope_and_phase_futures_are_send_for_send_inputs() {
        fn assert_send<T: Send>(_: T) {}

        assert_send(scope_request(async { "response" }));
        assert_send(in_phase(Phase::Query, async { "query" }));
    }

    fn catalog_decl() -> CatalogDecl {
        serde_yaml::from_str("id: default\ntenant: public").unwrap()
    }

    fn collection_decl() -> CollectionDecl {
        serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main").unwrap()
    }

    struct TestResolver;

    #[async_trait::async_trait]
    impl Resolver for TestResolver {
        async fn resolve_tenant(&self, value: &str) -> Result<String> {
            assert!(matches!(value, "public" | "missing"));
            advance(Duration::from_millis(1)).await;
            (value != "missing")
                .then(|| "tenant-internal".to_string())
                .ok_or(Error::NotFound)
        }

        async fn resolve_catalog(&self, tenant: &str, external: &str) -> Result<String> {
            assert_eq!(tenant, "tenant-internal");
            assert_eq!(external, "default");
            advance(Duration::from_millis(1)).await;
            Ok("catalog-internal".to_string())
        }

        async fn resolve_collection(&self, catalog: &str, external: &str) -> Result<String> {
            assert_eq!(catalog, "catalog-internal");
            assert_eq!(external, "demo");
            advance(Duration::from_millis(1)).await;
            Ok("collection-internal".to_string())
        }

        fn tenant_external_id(&self, value: &str) -> Option<&str> {
            (value == "tenant-internal").then_some("tenant-public")
        }

        fn catalog_external_id(&self, value: &str) -> Option<&str> {
            (value == "catalog-internal").then_some("catalog-public")
        }

        fn collection_external_id(&self, value: &str) -> Option<&str> {
            (value == "collection-internal").then_some("collection-public")
        }

        fn catalogs_for_tenant(&self, tenant: &str) -> Vec<(&str, &str)> {
            assert_eq!(tenant, "tenant-internal");
            vec![("catalog-internal", "catalog-public")]
        }

        fn catalog_count(&self) -> usize {
            1
        }
    }

    #[tokio::test(start_paused = true)]
    async fn observed_resolver_delegates_every_method_and_records_routing() {
        let resolver = observe_resolver(std::sync::Arc::new(TestResolver));
        let (_, snapshot) = scope_request(async {
            assert_eq!(
                resolver.resolve_tenant("public").await.unwrap(),
                "tenant-internal"
            );
            assert!(matches!(
                resolver.resolve_tenant("missing").await,
                Err(Error::NotFound)
            ));
            assert_eq!(
                resolver
                    .resolve_catalog("tenant-internal", "default")
                    .await
                    .unwrap(),
                "catalog-internal"
            );
            assert_eq!(
                resolver
                    .resolve_collection("catalog-internal", "demo")
                    .await
                    .unwrap(),
                "collection-internal"
            );
            assert_eq!(
                resolver.tenant_external_id("tenant-internal"),
                Some("tenant-public")
            );
            assert_eq!(resolver.tenant_external_id("missing"), None);
            assert_eq!(
                resolver.catalog_external_id("catalog-internal"),
                Some("catalog-public")
            );
            assert_eq!(
                resolver.collection_external_id("collection-internal"),
                Some("collection-public")
            );
            assert_eq!(
                resolver.catalogs_for_tenant("tenant-internal"),
                vec![("catalog-internal", "catalog-public")]
            );
            assert_eq!(resolver.catalog_count(), 1);
        })
        .await;

        assert_eq!(snapshot.routing(), Duration::from_millis(4));
    }

    struct TestRegistry;

    #[async_trait::async_trait]
    impl RegistryReader for TestRegistry {
        async fn catalog(&self, tenant: &str, external: &str) -> Result<Option<CatalogDecl>> {
            assert_eq!(tenant, "tenant-internal");
            advance(Duration::from_millis(1)).await;
            match external {
                "missing" => Ok(None),
                "error" => Err(Error::Timeout),
                _ => Ok(Some(catalog_decl())),
            }
        }

        async fn collection(
            &self,
            catalog: &str,
            external: &str,
        ) -> Result<Option<CollectionDecl>> {
            assert_eq!(catalog, "catalog-internal");
            advance(Duration::from_millis(1)).await;
            (external != "missing")
                .then(collection_decl)
                .map(Some)
                .ok_or(Error::Timeout)
        }

        async fn list_catalogs(
            &self,
            tenant: &str,
            page: PageRequest,
        ) -> Result<Page<CatalogDecl>> {
            assert_eq!(tenant, "tenant-internal");
            assert_eq!(page.limit, 7);
            assert_eq!(page.after.as_deref(), Some("catalog-cursor"));
            advance(Duration::from_millis(1)).await;
            Ok(Page {
                items: vec![catalog_decl()],
                next: Some("next-catalog".to_string()),
            })
        }

        async fn list_collections(
            &self,
            catalog: &str,
            page: PageRequest,
        ) -> Result<Page<CollectionDecl>> {
            assert_eq!(catalog, "catalog-internal");
            assert_eq!(page.limit, 3);
            assert_eq!(page.after.as_deref(), Some("collection-cursor"));
            advance(Duration::from_millis(1)).await;
            Err(Error::Timeout)
        }
    }

    struct TestStyleStore;

    impl StyleStore for TestStyleStore {
        fn load(&self, id: &str) -> Result<Option<serde_json::Value>> {
            match id {
                "missing" => Ok(None),
                "error" => Err(Error::Timeout),
                _ => Ok(Some(serde_json::json!({ "id": id }))),
            }
        }

        fn list(&self) -> Result<Vec<String>> {
            Ok(vec!["basic".to_string()])
        }
    }

    #[tokio::test(start_paused = true)]
    async fn observed_registry_and_style_store_preserve_success_null_and_errors() {
        let registry = observe_registry(std::sync::Arc::new(TestRegistry));
        let styles = observe_style_store(std::sync::Arc::new(TestStyleStore));
        let (_, snapshot) = scope_request(async {
            assert!(registry
                .catalog("tenant-internal", "default")
                .await
                .unwrap()
                .is_some());
            assert_eq!(
                registry
                    .catalog("tenant-internal", "missing")
                    .await
                    .unwrap(),
                None
            );
            assert!(matches!(
                registry.catalog("tenant-internal", "error").await,
                Err(Error::Timeout)
            ));
            assert!(registry
                .collection("catalog-internal", "demo")
                .await
                .unwrap()
                .is_some());
            assert!(matches!(
                registry.collection("catalog-internal", "missing").await,
                Err(Error::Timeout)
            ));
            let catalogs = registry
                .list_catalogs(
                    "tenant-internal",
                    PageRequest {
                        limit: 7,
                        after: Some("catalog-cursor".to_string()),
                    },
                )
                .await
                .unwrap();
            assert_eq!(catalogs.next.as_deref(), Some("next-catalog"));
            assert!(matches!(
                registry
                    .list_collections(
                        "catalog-internal",
                        PageRequest {
                            limit: 3,
                            after: Some("collection-cursor".to_string()),
                        },
                    )
                    .await,
                Err(Error::Timeout)
            ));

            assert_eq!(styles.load("basic").unwrap().unwrap()["id"], "basic");
            assert_eq!(styles.load("missing").unwrap(), None);
            assert!(matches!(styles.load("error"), Err(Error::Timeout)));
            assert_eq!(styles.list().unwrap(), vec!["basic"]);
        })
        .await;

        assert_eq!(snapshot.query(), Duration::from_millis(7));
    }

    struct TestFeatureSource {
        saw_grant_filter: AtomicBool,
    }

    #[async_trait::async_trait]
    impl FeatureSource for TestFeatureSource {
        async fn items(
            &self,
            collection: &CollectionDecl,
            query: &ItemsQuery,
        ) -> Result<FeaturePage> {
            assert_eq!(collection.id, "demo");
            assert_eq!(query.limit, 37);
            assert_eq!(query.bbox, Some([1.0, 2.0, 3.0, 4.0]));
            assert_eq!(query.token.as_deref(), Some("item-cursor"));
            assert!(query.filter.is_some());
            advance(Duration::from_millis(1)).await;
            self.saw_grant_filter
                .store(query.filter.is_some(), Ordering::SeqCst);
            Ok(FeaturePage {
                features_geojson: vec![serde_json::json!({ "items": true })],
                number_matched: Some(1),
                next_token: Some("next".to_string()),
            })
        }

        async fn item(
            &self,
            collection: &CollectionDecl,
            id: &str,
            filter: Option<&Filter>,
        ) -> Result<Option<serde_json::Value>> {
            assert_eq!(collection.id, "demo");
            assert_eq!(filter.is_some(), id == "one");
            advance(Duration::from_millis(1)).await;
            self.saw_grant_filter
                .store(filter.is_some(), Ordering::SeqCst);
            match id {
                "missing" => Ok(None),
                "error" => Err(Error::Timeout),
                _ => Ok(Some(serde_json::json!({ "id": id }))),
            }
        }

        fn filter_capable(&self) -> bool {
            true
        }

        fn crs_capable(&self) -> bool {
            true
        }

        async fn item_with_crs(
            &self,
            collection: &CollectionDecl,
            id: &str,
            filter: Option<&Filter>,
            requested_crs: RequestedCrs,
        ) -> Result<Option<serde_json::Value>> {
            assert_eq!(collection.id, "demo");
            assert_eq!(id, "one");
            assert!(filter.is_some());
            advance(Duration::from_millis(1)).await;
            self.saw_grant_filter
                .store(filter.is_some(), Ordering::SeqCst);
            Ok(Some(serde_json::json!({
                "id": id,
                "crs84": requested_crs == RequestedCrs::Crs84
            })))
        }
    }

    struct TestTileSource {
        saw_grant_filter: AtomicBool,
    }

    #[async_trait::async_trait]
    impl TileSource for TestTileSource {
        async fn mvt_tile(
            &self,
            collection: &CollectionDecl,
            coord: TileCoord,
            filter: Option<&Filter>,
        ) -> Result<Option<Bytes>> {
            assert_eq!(collection.id, "demo");
            assert_eq!(coord.z, 7);
            assert_eq!(coord.y, 11);
            assert_eq!(filter.is_some(), coord.x == 0);
            advance(Duration::from_millis(1)).await;
            self.saw_grant_filter
                .store(filter.is_some(), Ordering::SeqCst);
            match coord.x {
                1 => Ok(None),
                2 => Err(Error::Timeout),
                _ => Ok(Some(Bytes::from_static(b"mvt"))),
            }
        }

        fn filter_capable(&self) -> bool {
            true
        }

        async fn vector_layers(&self, collection: &CollectionDecl) -> Result<Option<Vec<String>>> {
            assert_eq!(collection.id, "demo");
            advance(Duration::from_millis(1)).await;
            Ok(Some(vec!["roads".to_string(), "water".to_string()]))
        }
    }

    struct TestVolumeSource;

    #[async_trait::async_trait]
    impl VolumeSource for TestVolumeSource {
        async fn volume_tile(
            &self,
            collection: &CollectionDecl,
            coord: TileCoord,
            filter: Option<&Filter>,
        ) -> Result<Option<VolumeMesh>> {
            assert_eq!(collection.id, "demo");
            assert_eq!(coord.z, 8);
            assert_eq!(coord.y, 12);
            assert_eq!(filter.is_some(), coord.x == 0);
            advance(Duration::from_millis(1)).await;
            match coord.x {
                1 => Ok(None),
                2 => Err(Error::Timeout),
                _ => Ok(Some(VolumeMesh {
                    positions: vec![[1.0, 2.0, 3.0]],
                    indices: vec![],
                })),
            }
        }

        fn filter_capable(&self) -> bool {
            true
        }
    }

    #[tokio::test(start_paused = true)]
    async fn observed_read_sources_delegate_all_methods_capabilities_and_grant_filters() {
        let feature = observe_feature_source(std::sync::Arc::new(TestFeatureSource {
            saw_grant_filter: AtomicBool::new(false),
        }));
        let tile = observe_tile_source(std::sync::Arc::new(TestTileSource {
            saw_grant_filter: AtomicBool::new(false),
        }));
        let volume = observe_volume_source(std::sync::Arc::new(TestVolumeSource));
        let collection = collection_decl();
        let filter = parse_text("id = 'visible'").unwrap();
        let query = ItemsQuery {
            limit: 37,
            bbox: Some([1.0, 2.0, 3.0, 4.0]),
            token: Some("item-cursor".to_string()),
            filter: Some(filter.clone()),
            ..ItemsQuery::default()
        };

        let (_, snapshot) = scope_request(async {
            assert_eq!(
                feature
                    .items(&collection, &query)
                    .await
                    .unwrap()
                    .number_matched,
                Some(1)
            );
            assert_eq!(
                feature
                    .item(&collection, "one", Some(&filter))
                    .await
                    .unwrap()
                    .unwrap()["id"],
                "one"
            );
            assert_eq!(
                feature.item(&collection, "missing", None).await.unwrap(),
                None
            );
            assert!(matches!(
                feature.item(&collection, "error", None).await,
                Err(Error::Timeout)
            ));
            assert!(feature.filter_capable());
            assert!(feature.crs_capable());
            let projected = feature
                .item_with_crs(&collection, "one", Some(&filter), RequestedCrs::Crs84)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(projected["crs84"], true);

            assert_eq!(
                tile.mvt_tile(&collection, TileCoord { z: 7, x: 0, y: 11 }, Some(&filter))
                    .await
                    .unwrap()
                    .unwrap(),
                Bytes::from_static(b"mvt")
            );
            assert_eq!(
                tile.mvt_tile(&collection, TileCoord { z: 7, x: 1, y: 11 }, None)
                    .await
                    .unwrap(),
                None
            );
            assert!(matches!(
                tile.mvt_tile(&collection, TileCoord { z: 7, x: 2, y: 11 }, None)
                    .await,
                Err(Error::Timeout)
            ));
            assert!(tile.filter_capable());
            assert_eq!(
                tile.vector_layers(&collection).await.unwrap().unwrap(),
                vec!["roads", "water"]
            );

            assert!(volume
                .volume_tile(&collection, TileCoord { z: 8, x: 0, y: 12 }, Some(&filter),)
                .await
                .unwrap()
                .is_some());
            assert_eq!(
                volume
                    .volume_tile(&collection, TileCoord { z: 8, x: 1, y: 12 }, None)
                    .await
                    .unwrap(),
                None
            );
            assert!(matches!(
                volume
                    .volume_tile(&collection, TileCoord { z: 8, x: 2, y: 12 }, None)
                    .await,
                Err(Error::Timeout)
            ));
            assert!(volume.filter_capable());
        })
        .await;

        assert_eq!(snapshot.query(), Duration::from_millis(12));
    }
}
