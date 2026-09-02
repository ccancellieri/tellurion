//! Config-gated background task wiring for the write-reactive tile-cache
//! invalidation consumer (`tellurion_core::invalidation::
//! run_generation_consumer`, `#113`): default off
//! (`ServerConfig.tile_invalidation.enabled`), so a deployment that never
//! turns this on — or never opts a collection in via `CollectionDecl.
//! tile_invalidation` — sees no behavior change from this module existing at
//! all. Mirrors `applier.rs`'s own shape and lifecycle exactly (same doc
//! reasoning applies here, restated only where this differs):
//!
//! - One task per collection that BOTH opts in AND resolves a `routing.write`
//!   outbox, built once from the `AppContext` snapshot at boot. A config
//!   reload swaps in a new router/config for future HTTP requests but does
//!   not respin this set — the same narrow, opt-in background convenience
//!   `applier.rs` already documents.
//! - A collection that opts in but has no `routing.write`, or whose named
//!   storage doesn't advertise `outbox_source`, logs and is skipped —
//!   spawning is best-effort per collection, never a reason to fail boot.
//!
//! Unlike the index applier, this wiring also builds and returns the
//! `GenerationStore` itself (there being no other natural owner for it) —
//! `main.rs` wires the returned store into `AppContext::set_generations`
//! before serving any request, so every tile fetch's generation lookup sees
//! it from the very first request onward.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tellurion_core::{AppContext, GenerationStore};

/// Spawns one generation-consumer task per opted-in, outbox-resolvable
/// collection, returning the `GenerationStore` they feed together with
/// their join handles (so the caller can bound how long it waits for them
/// to stop during shutdown, the same as `applier::spawn_all`). The store is
/// `GenerationStore::empty()` — every lookup answers generation `0` — when
/// `tile_invalidation.enabled` is `false` (the default) or no collection
/// opted in, which is what keeps every tile fetch's cache key byte-for-byte
/// identical to before `#113` existed.
pub async fn spawn_all(
    ctx: &Arc<AppContext>,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> (Arc<GenerationStore>, Vec<tokio::task::JoinHandle<()>>) {
    let state = ctx.current();
    let conf = state.config.server.tile_invalidation;
    if !conf.enabled {
        return (Arc::new(GenerationStore::empty()), Vec::new());
    }

    let opted_in: Vec<_> = state
        .config
        .collections
        .iter()
        .filter(|collection| collection.tile_invalidation)
        .collect();

    let store = Arc::new(GenerationStore::new(
        conf.bucket_zoom,
        opted_in.iter().map(|collection| collection.id.clone()),
    ));

    let tenants_by_catalog: HashMap<&str, &str> = state
        .config
        .catalogs
        .iter()
        .map(|catalog| (catalog.id.as_str(), catalog.tenant.as_str()))
        .collect();

    let poll_interval = Duration::from_millis(conf.poll_interval_ms);
    let mut handles = Vec::new();
    for collection in opted_in {
        let Some(&tenant) = tenants_by_catalog.get(collection.catalog.as_str()) else {
            tracing::warn!(
                collection = %collection.id,
                "tile-generation invalidation: collection references an unknown catalog; skipping"
            );
            continue;
        };
        let catalog = collection.catalog.as_str();

        // The RESOLVED declaration, not the raw configured one. A collection
        // that does not spell out `table:`/`geometry:`/`pk:` — the ordinary
        // case, since the router derives them from the driver's own catalog —
        // leaves those fields `None` in `config.collections`, and
        // `CollectionDecl::resolved_table` panics on `None` by design ("must
        // be resolved by Router before reaching a driver"). Handing the raw
        // decl to a driver therefore killed this consumer's task on its very
        // first drain, for the whole life of the process, with the panic
        // surfacing only as a shutdown-time log line: no invalidation at all,
        // every tile served from a generation frozen at boot. Exactly the
        // silence `#142`/`#141` exist to remove, so it is removed here too —
        // `resolve_outbox` already returns the resolved decl beside the
        // source, and this now uses it (`applier::spawn_all` resolves its own
        // the same way).
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
                    "tile-generation invalidation: could not resolve an outbox source for this collection; skipping"
                );
                continue;
            }
        };

        tracing::info!(collection = %collection.id, "tile-generation invalidation consumer: starting");
        handles.push(tokio::spawn(tellurion_core::run_generation_consumer(
            outbox,
            Arc::clone(&store),
            decl,
            conf.batch_size,
            poll_interval,
            shutdown.clone(),
        )));
    }
    (store, handles)
}
