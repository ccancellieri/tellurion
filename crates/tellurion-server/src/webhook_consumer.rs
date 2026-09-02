//! Config-gated background task wiring for webhook delivery
//! (`tellurion_core::webhooks::run_webhook_consumer`, `#115`): default off
//! (`ServerConfig.webhook_delivery.enabled`), so a deployment that never
//! turns this on — or never declares any `webhooks:` subscriptions at all —
//! sees no behavior change from this module existing. Mirrors
//! `generation_consumer.rs`'s own shape and lifecycle exactly (see that
//! module's doc for the reasoning restated only where this differs):
//!
//! - One [`tellurion_core::WebhookSubscriptionRuntime`] per enabled,
//!   secret-resolvable subscription, rebound whenever the atomically-swapped
//!   config generation changes. Unchanged declarations retain their cursor
//!   and dead-letter state; removed pairs are cancelled and stop holding the
//!   retention floor back.
//! - One delivery supervisor per (subscription, collection) pair. It
//!   resolves the current outbox and retries resolution after failures, so
//!   a transient routing/backend outage cannot silently turn a declared
//!   consumer into an unprotected retention gap.
//! - A subscription whose `secret_env` is not set in the process
//!   environment is skipped entirely (logged by name) — the same "set-but-
//!   unreachable is a failed dependency, unset-required is a boot-time
//!   named refusal for a REQUIRED backend" split other `url_env`/`secret_env`
//!   consumers draw, except here the whole feature stays opt-in per
//!   subscription (a webhook target the operator hasn't finished wiring a
//!   secret for should not block every other one), so a missing secret is a
//!   skip, not a boot failure.
//!
//! Also returns the built subscription registry itself (there being no
//! other natural owner for it) — `main.rs` hands it to
//! `retention_consumer::spawn_all` so the retention floor can fold each
//! subscription's own cursor in as a registered consumer.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tellurion_core::{
    AppContext, CollectionDecl, ReqwestDeliverer, Sequence, StorageDecl, WebhookConsumerSettings,
    WebhookDeliverer, WebhookRetryPolicy, WebhookSubscriptionDecl, WebhookSubscriptionRuntime,
};
use tokio::sync::watch;
use tokio::task::JoinSet;

type SecretResolver = Arc<dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync>;

/// Every subscription this deployment built a runtime for, keyed by its own
/// declared id — what `retention_consumer::spawn_all` reads to fold webhook
/// cursors into the floor computation, and what `webhook_admin` reads for
/// dead-letter inspection.
#[derive(Default)]
pub struct WebhookRegistry {
    runtimes: RwLock<HashMap<String, Arc<WebhookSubscriptionRuntime>>>,
}

impl WebhookRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: &str) -> Option<Arc<WebhookSubscriptionRuntime>> {
        self.runtimes
            .read()
            .expect("webhook registry lock is never held across a panic")
            .get(id)
            .cloned()
    }

    pub fn values(&self) -> Vec<Arc<WebhookSubscriptionRuntime>> {
        self.runtimes
            .read()
            .expect("webhook registry lock is never held across a panic")
            .values()
            .cloned()
            .collect()
    }

    pub(crate) fn replace(&self, runtimes: HashMap<String, Arc<WebhookSubscriptionRuntime>>) {
        *self
            .runtimes
            .write()
            .expect("webhook registry lock is never held across a panic") = runtimes;
    }
}

struct ManagedSubscription {
    declaration: WebhookSubscriptionDecl,
    dead_letter_capacity: usize,
    runtime: Arc<WebhookSubscriptionRuntime>,
    pair_identities: HashMap<String, DeliveryIdentity>,
}

#[derive(Clone, PartialEq)]
struct DeliveryIdentity {
    tenant: String,
    collection: CollectionDecl,
    write_storage: Option<StorageDecl>,
    resolved_storage_url: Option<String>,
}

struct DeliveryPair {
    ctx: Arc<AppContext>,
    tenant: String,
    catalog: String,
    collection_id: String,
    runtime: Arc<WebhookSubscriptionRuntime>,
    deliverer: Arc<dyn WebhookDeliverer>,
    settings: WebhookConsumerSettings,
}

fn delivery_identity(
    config: &tellurion_core::AppConfig,
    tenant: &str,
    collection: &CollectionDecl,
) -> DeliveryIdentity {
    let write_storage_id = collection
        .routing
        .write
        .as_ref()
        .and_then(|lane| lane.0.first())
        .unwrap_or(&collection.storage);
    let write_storage = config
        .storages
        .iter()
        .find(|storage| &storage.id == write_storage_id)
        .cloned();
    let resolved_storage_url = write_storage
        .as_ref()
        .and_then(|storage| std::env::var(&storage.url_env).ok());
    DeliveryIdentity {
        tenant: tenant.to_string(),
        collection: collection.clone(),
        write_storage,
        resolved_storage_url,
    }
}

fn reconcile_cursor(
    runtime: &WebhookSubscriptionRuntime,
    previous_runtime: Option<&Arc<WebhookSubscriptionRuntime>>,
    collection_id: &str,
    identity_unchanged: bool,
) {
    if !identity_unchanged
        && runtime
            .registered_collections()
            .iter()
            .any(|registered| registered == collection_id)
    {
        runtime.remove_collection(collection_id);
    }
    if !runtime
        .registered_collections()
        .iter()
        .any(|registered| registered == collection_id)
    {
        let initial = if identity_unchanged
            && previous_runtime.is_some_and(|previous| {
                previous
                    .registered_collections()
                    .iter()
                    .any(|registered| registered == collection_id)
            }) {
            previous_runtime
                .expect("checked above")
                .cursor(collection_id)
        } else {
            // Config activation and manager observation are not one atomic
            // operation. A later high-water seed would lose writes in that
            // window; zero conservatively replays all retained history.
            Sequence(0)
        };
        runtime.ensure_collection(collection_id, initial);
    }
}

/// Spawns one manager that binds a delivery task per matching, resolvable
/// (subscription, collection) pair and rebinds those tasks after an atomic
/// config-generation swap. Returns the live [`WebhookRegistry`] with the
/// manager handle so the caller can bound shutdown just like its other
/// background consumers. The registry stays empty and no delivery task is
/// bound while webhook delivery is disabled or no subscription is enabled.
pub async fn spawn_all(
    ctx: &Arc<AppContext>,
    shutdown: watch::Receiver<bool>,
) -> (Arc<WebhookRegistry>, Vec<tokio::task::JoinHandle<()>>) {
    let secrets: SecretResolver = Arc::new(|name| std::env::var(name).ok().map(String::into_bytes));
    spawn_all_with_secret_resolver(ctx, shutdown, secrets).await
}

async fn spawn_all_with_secret_resolver(
    ctx: &Arc<AppContext>,
    shutdown: watch::Receiver<bool>,
    secrets: SecretResolver,
) -> (Arc<WebhookRegistry>, Vec<tokio::task::JoinHandle<()>>) {
    let registry = Arc::new(WebhookRegistry::new());
    let mut managed = HashMap::new();
    let mut tasks = JoinSet::new();
    // `main` starts retention only after this returns, so every declared
    // cursor must already participate in the floor before pruning can run.
    let bound_version = rebind(
        ctx,
        &registry,
        &mut managed,
        &mut tasks,
        shutdown.clone(),
        &secrets,
    )
    .await;
    let handle = tokio::spawn(run_manager(
        Arc::clone(ctx),
        Arc::clone(&registry),
        managed,
        tasks,
        bound_version,
        shutdown,
        secrets,
    ));
    (registry, vec![handle])
}

async fn run_manager(
    ctx: Arc<AppContext>,
    registry: Arc<WebhookRegistry>,
    mut managed: HashMap<String, ManagedSubscription>,
    mut tasks: JoinSet<()>,
    mut bound_version: String,
    mut shutdown: watch::Receiver<bool>,
    secrets: SecretResolver,
) {
    loop {
        let version = ctx.current().config_version.to_string();
        if bound_version != version {
            bound_version = rebind(
                &ctx,
                &registry,
                &mut managed,
                &mut tasks,
                shutdown.clone(),
                &secrets,
            )
            .await;
        }

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            result = shutdown.changed() => {
                if result.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                if let Err(error) = result {
                    tracing::error!(%error, "webhook delivery task failed");
                }
            }
        }
    }

    stop_delivery_tasks(&mut tasks).await;
    // Retention receives the same shutdown edge and may still be completing
    // a pass. Keep the last cursors as its conservative floor until process
    // exit drops every owner.
}

async fn stop_delivery_tasks(tasks: &mut JoinSet<()>) {
    // Reconfiguration must never leave the old and new bindings draining
    // the same cursor concurrently. Aborting is safe under the documented
    // at-least-once contract: an interrupted batch either retained its old
    // cursor and is replayed, or had already advanced and resumes after it.
    tasks.abort_all();
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            if !error.is_cancelled() {
                tracing::error!(%error, "webhook delivery task failed while stopping");
            }
        }
    }
}

async fn rebind(
    ctx: &Arc<AppContext>,
    registry: &Arc<WebhookRegistry>,
    managed: &mut HashMap<String, ManagedSubscription>,
    tasks: &mut JoinSet<()>,
    shutdown: watch::Receiver<bool>,
    secrets: &SecretResolver,
) -> String {
    stop_delivery_tasks(tasks).await;

    let state = ctx.current();
    let bound_version = state.config_version.to_string();
    let conf = state.config.server.webhook_delivery;
    if !conf.enabled {
        managed.clear();
        registry.replace(HashMap::new());
        return bound_version;
    }

    let tenants_by_catalog: HashMap<&str, &str> = state
        .config
        .catalogs
        .iter()
        .map(|catalog| (catalog.id.as_str(), catalog.tenant.as_str()))
        .collect();

    let deliverer: Arc<dyn WebhookDeliverer> = Arc::new(ReqwestDeliverer::new(
        Duration::from_millis(conf.request_timeout_ms),
    ));
    let retry = WebhookRetryPolicy {
        max_attempts: conf.max_attempts,
        base_backoff_ms: conf.base_backoff_ms,
        max_backoff_ms: conf.max_backoff_ms,
    };
    let settings = WebhookConsumerSettings {
        batch_size: conf.batch_size,
        retry,
        poll_interval: Duration::from_millis(conf.poll_interval_ms),
    };

    let mut previous = std::mem::take(managed);
    let mut next = HashMap::new();

    for subscription_decl in &state.config.webhooks {
        if !subscription_decl.enabled {
            continue;
        }
        let Some(secret) = secrets(&subscription_decl.secret_env) else {
            tracing::warn!(
                subscription = %subscription_decl.id,
                secret_env = %subscription_decl.secret_env,
                "webhook delivery: secret_env is not set in the environment; skipping this subscription"
            );
            continue;
        };

        let matched_collection_ids: Vec<String> = state
            .config
            .collections
            .iter()
            .filter(|collection| {
                subscription_decl
                    .scope
                    .matches(&collection.catalog, &collection.id)
            })
            .map(|collection| collection.id.clone())
            .collect();

        let existing = previous.remove(&subscription_decl.id);
        let previous_runtime = existing
            .as_ref()
            .map(|subscription| Arc::clone(&subscription.runtime));
        let previous_pair_identities = existing
            .as_ref()
            .map(|subscription| subscription.pair_identities.clone())
            .unwrap_or_default();
        let runtime = match existing {
            Some(existing)
                if existing.declaration == *subscription_decl
                    && existing.dead_letter_capacity == conf.dead_letter_capacity =>
            {
                existing.runtime
            }
            _ => Arc::new(WebhookSubscriptionRuntime::new(
                subscription_decl.id.clone(),
                subscription_decl.url.clone(),
                secret,
                subscription_decl.operations.clone(),
                Vec::new(),
                conf.dead_letter_capacity,
            )),
        };

        for collection_id in runtime.registered_collections() {
            if !matched_collection_ids.contains(&collection_id) {
                runtime.remove_collection(&collection_id);
            }
        }
        let mut pair_identities = HashMap::new();
        for collection_id in &matched_collection_ids {
            let Some(collection) = state
                .config
                .collections
                .iter()
                .find(|c| &c.id == collection_id)
            else {
                continue;
            };
            let Some(&tenant) = tenants_by_catalog.get(collection.catalog.as_str()) else {
                tracing::warn!(
                    subscription = %subscription_decl.id,
                    collection = %collection.id,
                    "webhook delivery: collection references an unknown catalog; skipping"
                );
                continue;
            };
            let identity = delivery_identity(&state.config, tenant, collection);
            let identity_unchanged =
                previous_pair_identities.get(&collection.id) == Some(&identity);
            reconcile_cursor(
                &runtime,
                previous_runtime.as_ref(),
                &collection.id,
                identity_unchanged,
            );
            pair_identities.insert(collection.id.clone(), identity);

            tasks.spawn(
                DeliveryPair {
                    ctx: Arc::clone(ctx),
                    tenant: tenant.to_string(),
                    catalog: collection.catalog.clone(),
                    collection_id: collection.id.clone(),
                    runtime: Arc::clone(&runtime),
                    deliverer: Arc::clone(&deliverer),
                    settings,
                }
                .run(shutdown.clone()),
            );
        }

        next.insert(
            subscription_decl.id.clone(),
            ManagedSubscription {
                declaration: subscription_decl.clone(),
                dead_letter_capacity: conf.dead_letter_capacity,
                runtime,
                pair_identities,
            },
        );
    }

    registry.replace(
        next.iter()
            .map(|(id, subscription)| (id.clone(), Arc::clone(&subscription.runtime)))
            .collect(),
    );
    *managed = next;
    bound_version
}

impl DeliveryPair {
    async fn run(self, mut shutdown: watch::Receiver<bool>) {
        let retry_interval = self.settings.poll_interval.max(Duration::from_secs(5));
        loop {
            if *shutdown.borrow() {
                return;
            }
            let state = self.ctx.current();
            match state
                .router
                .resolve_outbox(&self.tenant, &self.catalog, &self.collection_id)
                .await
            {
                Ok((collection, outbox)) => {
                    tracing::info!(
                        subscription = %self.runtime.id(),
                        collection = %collection.id,
                        "webhook delivery: starting"
                    );
                    tellurion_core::run_webhook_consumer(
                        outbox,
                        self.runtime,
                        collection,
                        self.deliverer,
                        self.settings,
                        shutdown,
                    )
                    .await;
                    return;
                }
                Err(error) => {
                    tracing::warn!(
                        subscription = %self.runtime.id(),
                        collection = %self.collection_id,
                        %error,
                        "webhook delivery: could not resolve an outbox source; retrying"
                    );
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(retry_interval) => {}
                result = shutdown.changed() => {
                    if result.is_err() || *shutdown.borrow() {
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tellurion_core::{
        build_authorizer, AppConfig, CatalogSource, CollectionDecl, DriverFactory, FileStyleStore,
        MokaTileCache, Mutation, Obligation, OutboxSource, PhysicalCollection, Registry, Resolver,
        Result, Router, StaticResolver, StorageDecl, StorageDriver, StyleStore, TileCache,
        WriteSink,
    };

    struct EmptyCatalog;

    #[async_trait::async_trait]
    impl CatalogSource for EmptyCatalog {
        async fn collections(&self) -> Result<Vec<PhysicalCollection>> {
            Ok(Vec::new())
        }
    }

    struct EmptyOutbox;

    #[async_trait::async_trait]
    impl OutboxSource for EmptyOutbox {
        async fn read_after(
            &self,
            _collection: &CollectionDecl,
            _after: Sequence,
            _limit: u32,
        ) -> Result<Vec<Obligation>> {
            Ok(Vec::new())
        }

        async fn primary_high_water(&self, _collection: &CollectionDecl) -> Result<Sequence> {
            Ok(Sequence(0))
        }
    }

    struct EmptyWriteSink;

    #[async_trait::async_trait]
    impl WriteSink for EmptyWriteSink {
        async fn apply(
            &self,
            _collection: &CollectionDecl,
            _mutation: Mutation,
        ) -> Result<Sequence> {
            Ok(Sequence(0))
        }
    }

    struct TestDriver;

    impl StorageDriver for TestDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(EmptyCatalog)
        }

        fn write_sink(&self) -> Option<Arc<dyn WriteSink>> {
            Some(Arc::new(EmptyWriteSink))
        }

        fn outbox_source(&self) -> Option<Arc<dyn OutboxSource>> {
            Some(Arc::new(EmptyOutbox))
        }
    }

    struct TestFactory;

    impl DriverFactory for TestFactory {
        fn name(&self) -> &str {
            "test"
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(TestDriver))
        }
    }

    fn test_config() -> AppConfig {
        let config: AppConfig = serde_yaml::from_str(
            r#"
server:
  webhook_delivery:
    enabled: true
    poll_interval_ms: 60000
storages: [ { id: main, driver: test, url_env: UNUSED } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
webhooks:
  - id: alerts
    url: https://example.test/hook
    secret_env: TEST_WEBHOOK_SECRET
    scope: { collections: [demo] }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        config
    }

    fn test_context() -> Arc<AppContext> {
        let config = test_config();
        let mut drivers = Registry::new();
        drivers.register(Arc::new(TestFactory));
        let router = Router::build(&config, &drivers).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let authorizer = build_authorizer(&config.auth)
            .expect("no bearer principal in this fixture reads a token_env");
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let styles: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        Arc::new(AppContext::new(
            config, router, resolver, authorizer, cache, styles,
        ))
    }

    #[test]
    fn repointed_outbox_identity_resets_the_pair_cursor() {
        let runtime = Arc::new(WebhookSubscriptionRuntime::new(
            "alerts".to_string(),
            "https://example.test/hook".to_string(),
            b"secret".to_vec(),
            Vec::new(),
            Vec::new(),
            10,
        ));
        runtime.ensure_collection("demo", Sequence(42));

        reconcile_cursor(&runtime, Some(&runtime), "demo", false);

        assert_eq!(runtime.cursor("demo"), Sequence(0));
    }

    #[test]
    fn changed_write_storage_changes_the_delivery_identity() {
        let before = test_config();
        let mut after = before.clone();
        after.collections[0].table = Some("replacement_table".to_string());
        let before_identity = delivery_identity(&before, "public", &before.collections[0]);
        let after_identity = delivery_identity(&after, "public", &after.collections[0]);

        assert!(before_identity != after_identity);
    }

    #[tokio::test]
    async fn initial_bind_precedes_return_and_shutdown_keeps_the_retention_floor() {
        let ctx = test_context();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let secrets: SecretResolver =
            Arc::new(|name| (name == "TEST_WEBHOOK_SECRET").then(|| b"test-secret".to_vec()));

        let (registry, handles) = spawn_all_with_secret_resolver(&ctx, shutdown_rx, secrets).await;
        let runtime = registry
            .get("alerts")
            .expect("the initial bind must populate the registry before returning");
        assert!(runtime
            .registered_collections()
            .contains(&"demo".to_string()));
        assert_eq!(runtime.cursor("demo"), Sequence(0));

        shutdown_tx.send(true).unwrap();
        for handle in handles {
            handle.await.unwrap();
        }
        assert!(
            registry.get("alerts").is_some(),
            "shutdown must retain the conservative cursor floor"
        );
    }
}
