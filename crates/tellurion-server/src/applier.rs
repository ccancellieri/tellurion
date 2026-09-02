//! Config-gated background task wiring for the index applier
//! (`tellurion_core::applier::run_applier`, `#67`): default off
//! (`ServerConfig.index_applier.enabled`), so a deployment that never
//! declares a `routing.index` lane sees no behavior change. One task per
//! collection whose `routing.index` is configured, resolved once from the
//! router `AppContext` was built with at boot — a config reload swaps in a
//! new router/config for future HTTP requests, but does not respin these
//! tasks; re-wiring the applier set on reload is left for a later slice
//! (the applier is a narrow, opt-in background convenience, not something
//! any request path depends on).
//!
//! A collection that names `routing.index` but resolves to a storage
//! lacking the capability, or whose index table was never provisioned,
//! logs and is skipped — spawning is best-effort per collection, never a
//! reason to fail boot.
//!
//! ## The clustered lease (`#193`)
//!
//! With `index_applier.lease` declared, each task is handed a
//! `LeaseBinding` and drains only while it holds its collection's lease, so
//! 2+ replicas keep the outbox design doc's "single ordered consumer per
//! collection" invariant. Without it — the default — nothing changes and no
//! coordinator is contacted.
//!
//! Best-effort spawning has one deliberate exception here. Everywhere else
//! in this module a resolution failure warns and skips, and skipping is the
//! conservative outcome. For the lease it is the *only* conservative
//! outcome: an operator who declared a lease asked for exactly one drainer
//! across the fleet, so a collection whose lease will not resolve is
//! skipped rather than started unleased — starting it would silently
//! produce the second concurrent drainer the declaration exists to prevent.
//! Boot still never fails.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tellurion_core::{AppContext, LeaseBinding, LeaseKey, INDEX_APPLIER_CONSUMER};

/// Spawns one applier task per index-routed collection and returns their
/// join handles, so the caller can bound how long it waits for them to stop
/// during shutdown. Empty when `index_applier.enabled` is `false` (the
/// default).
pub async fn spawn_all(
    ctx: &Arc<AppContext>,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let state = ctx.current();
    let applier_config = state.config.server.index_applier.clone();
    if !applier_config.enabled {
        return Vec::new();
    }

    let tenants_by_catalog: HashMap<&str, &str> = state
        .config
        .catalogs
        .iter()
        .map(|catalog| (catalog.id.as_str(), catalog.tenant.as_str()))
        .collect();

    let poll_interval = Duration::from_millis(applier_config.poll_interval_ms);
    let mut handles = Vec::new();
    for collection in &state.config.collections {
        if collection.routing.index.is_none() {
            continue;
        }
        let Some(&tenant) = tenants_by_catalog.get(collection.catalog.as_str()) else {
            tracing::warn!(
                collection = %collection.id,
                "index applier: collection references an unknown catalog; skipping"
            );
            continue;
        };
        let catalog = collection.catalog.as_str();

        // The RESOLVED declaration, not the raw configured one — see
        // `generation_consumer::spawn_all`'s own note for the full reasoning.
        // A collection that does not spell out `table:`/`geometry:`/`pk:`
        // (the ordinary case: the router derives them from the driver's own
        // catalog) leaves those `None` in `config.collections`, and
        // `CollectionDecl::resolved_table` panics on `None` by design, so
        // handing the raw decl to a driver kills this task on its first
        // drain — permanently, and silently until shutdown.
        let (decl, outbox) = match state
            .router
            .resolve_outbox(tenant, catalog, &collection.id)
            .await
        {
            Ok(resolved) => resolved,
            Err(error) => {
                tracing::warn!(
                    collection = %collection.id,
                    %error,
                    "index applier: could not resolve an outbox source for this collection; skipping"
                );
                continue;
            }
        };
        let index = match state
            .router
            .resolve_index(tenant, catalog, &collection.id)
            .await
        {
            Ok((_, index)) => index,
            Err(error) => {
                tracing::warn!(
                    collection = %collection.id,
                    %error,
                    "index applier: could not resolve an index sink for this collection; skipping"
                );
                continue;
            }
        };

        // Fails closed, unlike every other skip above: see this module's
        // own doc for why an unresolvable lease must not become an
        // unleased applier.
        let lease = match &applier_config.lease {
            None => None,
            Some(decl) => match state.router.resolve_lease(tenant, catalog, &collection.id) {
                Ok(lease) => Some(LeaseBinding::new(
                    lease,
                    LeaseKey::for_collection(
                        decl.namespace.as_deref(),
                        INDEX_APPLIER_CONSUMER,
                        tenant,
                        catalog,
                        &collection.id,
                    ),
                )),
                Err(error) => {
                    tracing::warn!(
                        collection = %collection.id,
                        %error,
                        "index applier: a lease is configured but this collection's storage cannot provide one; skipping rather than draining unleased"
                    );
                    continue;
                }
            },
        };

        tracing::info!(
            collection = %collection.id,
            leased = lease.is_some(),
            "index applier: starting"
        );
        handles.push(tokio::spawn(tellurion_core::run_applier(
            outbox,
            index,
            decl,
            applier_config.batch_size,
            poll_interval,
            lease,
            shutdown.clone(),
        )));
    }
    handles
}
