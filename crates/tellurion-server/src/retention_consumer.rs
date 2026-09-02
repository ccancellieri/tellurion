//! Config-gated background task wiring for the consumer-aware outbox
//! retention floor (`tellurion_core::retention::compute_floor`, `#115`):
//! default off (`ServerConfig.outbox_retention.enabled`), so a deployment
//! that never turns this on keeps every outbox growing forever — byte-for-
//! byte today's behavior. Mirrors `generation_consumer.rs`'s/`applier.rs`'s
//! own shape: one task per collection with a resolvable outbox, polling on
//! a fixed interval, best-effort per collection, never a reason to fail
//! boot.
//!
//! Each tick folds in every consumer this deployment actually has REGISTERED
//! for that collection right now:
//!
//! - the index applier's own `IndexSink::applied_high_water`, when
//!   `routing.index` is configured AND `index_applier.enabled`;
//! - the tile-generation consumer's own drain cursor
//!   (`GenerationStore::cursor`), when `tile_invalidation.enabled` AND this
//!   collection opted in;
//! - every webhook subscription's own per-collection cursor
//!   (`WebhookSubscriptionRuntime::cursor`), for every subscription whose
//!   scope matched this collection (`webhook_consumer::WebhookRegistry`).
//!
//! A consumer that is configured/enabled but whose own resolve call fails
//! right now (a missing index table, ...) is folded in at `Sequence(0)` —
//! see `tellurion_core::retention`'s own module doc for why that is the
//! honest, conservative representation rather than simply omitting it.
//! [`compute_floor`] does the pure arithmetic; this module's own job is
//! gathering the inputs and acting on the result: pruning at most
//! `prune_batch_size` rows via `OutboxSource::prune_before`, and naming (in
//! a log line, and a per-consumer lag gauge) whichever consumer is
//! currently the bottleneck whenever the floor lands short of the primary's
//! own high-water mark.

use std::sync::Arc;
use std::time::Duration;

use tellurion_core::{compute_floor, AppContext, GenerationStore, Sequence};

use crate::webhook_consumer::WebhookRegistry;

/// Spawns one retention task per collection with a resolvable outbox,
/// returning their join handles. Empty when `outbox_retention.enabled` is
/// `false` (the default).
pub async fn spawn_all(
    ctx: &Arc<AppContext>,
    generations: Arc<GenerationStore>,
    webhooks: Arc<WebhookRegistry>,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let state = ctx.current();
    let conf = state.config.server.outbox_retention;
    if !conf.enabled {
        return Vec::new();
    }

    let tenants_by_catalog: std::collections::HashMap<&str, &str> = state
        .config
        .catalogs
        .iter()
        .map(|catalog| (catalog.id.as_str(), catalog.tenant.as_str()))
        .collect();

    let poll_interval = Duration::from_millis(conf.poll_interval_ms);
    let mut handles = Vec::new();

    for collection in &state.config.collections {
        let Some(&tenant) = tenants_by_catalog.get(collection.catalog.as_str()) else {
            tracing::warn!(
                collection = %collection.id,
                "outbox retention: collection references an unknown catalog; skipping"
            );
            continue;
        };
        let catalog = collection.catalog.as_str();

        // The RESOLVED declaration, not the raw configured one — see
        // `generation_consumer::spawn_all`'s own note. A collection that does
        // not spell out `table:` leaves it `None` in `config.collections`,
        // and `CollectionDecl::resolved_table` panics on `None` by design, so
        // handing the raw decl to a driver kills this task on its first
        // prune.
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
                    "outbox retention: could not resolve an outbox source for this collection; skipping"
                );
                continue;
            }
        };

        tracing::info!(collection = %collection.id, "outbox retention: starting");
        handles.push(tokio::spawn(run_retention_consumer(
            Arc::clone(ctx),
            outbox,
            decl,
            tenant.to_string(),
            catalog.to_string(),
            Arc::clone(&generations),
            Arc::clone(&webhooks),
            conf.prune_batch_size,
            poll_interval,
            shutdown.clone(),
        )));
    }
    handles
}

/// Every currently-registered consumer's `(name, cursor)` pair for
/// `collection` — the exact input `tellurion_core::compute_floor` folds
/// into the floor, gathered fresh on every tick (an index/generation
/// consumer's own enablement, or which collections a webhook subscription
/// matches, could in principle change across a reload).
async fn registered_consumers(
    ctx: &AppContext,
    collection: &tellurion_core::CollectionDecl,
    tenant: &str,
    catalog: &str,
    generations: &GenerationStore,
    webhooks: &WebhookRegistry,
) -> Vec<(String, Sequence)> {
    let state = ctx.current();
    let mut consumers = Vec::new();

    if collection.routing.index.is_some() && state.config.server.index_applier.enabled {
        match state
            .router
            .resolve_index(tenant, catalog, &collection.id)
            .await
        {
            Ok((_, index)) => match index.applied_high_water(collection).await {
                Ok(cursor) => consumers.push(("index-applier".to_string(), cursor)),
                Err(error) => {
                    tracing::warn!(
                        collection = %collection.id,
                        %error,
                        "outbox retention: index applier's own high-water mark is unresolvable right now; blocking pruning for this collection"
                    );
                    consumers.push(("index-applier".to_string(), Sequence(0)));
                }
            },
            Err(error) => {
                tracing::warn!(
                    collection = %collection.id,
                    %error,
                    "outbox retention: index applier is configured but its sink no longer resolves; blocking pruning for this collection"
                );
                consumers.push(("index-applier".to_string(), Sequence(0)));
            }
        }
    }

    if state.config.server.tile_invalidation.enabled && collection.tile_invalidation {
        consumers.push((
            "tile-generation".to_string(),
            generations.cursor(&collection.id),
        ));
    }

    for subscription in webhooks.values() {
        if subscription
            .registered_collections()
            .into_iter()
            .any(|id| id == collection.id)
        {
            consumers.push((
                format!("webhook:{}", subscription.id()),
                subscription.cursor(&collection.id),
            ));
        }
    }

    consumers
}

/// Runs one collection's retention pass on a fixed `poll_interval` until
/// `shutdown` reports `true` — gathers this tick's registered consumers,
/// computes the floor, prunes at most `prune_batch_size` rows, and emits
/// `outbox_retention_floor`/`outbox_retention_consumer_lag` (both labeled
/// `collection`, the latter also by `consumer`). A failed pass is logged
/// and retried next tick, the same "a stalled background lane degrades, it
/// does not stop" treatment every other consumer in this workspace gives.
#[allow(clippy::too_many_arguments)]
async fn run_retention_consumer(
    ctx: Arc<AppContext>,
    outbox: Arc<dyn tellurion_core::OutboxSource>,
    collection: tellurion_core::CollectionDecl,
    tenant: String,
    catalog: String,
    generations: Arc<GenerationStore>,
    webhooks: Arc<WebhookRegistry>,
    prune_batch_size: u32,
    poll_interval: Duration,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        match outbox.primary_high_water(&collection).await {
            Ok(high_water) => {
                let consumers = registered_consumers(
                    &ctx,
                    &collection,
                    &tenant,
                    &catalog,
                    generations.as_ref(),
                    webhooks.as_ref(),
                )
                .await;
                let result = compute_floor(high_water, &consumers);

                metrics::gauge!(
                    "outbox_retention_floor",
                    "collection" => collection.id.clone()
                )
                .set(result.floor.0 as f64);
                for lag in &result.lags {
                    metrics::gauge!(
                        "outbox_retention_consumer_lag",
                        "collection" => collection.id.clone(),
                        "consumer" => lag.name.clone()
                    )
                    .set(lag.lag as f64);
                }

                if result.floor < high_water {
                    if let Some(slowest) = result.lags.iter().max_by_key(|lag| lag.lag) {
                        tracing::info!(
                            collection = %collection.id,
                            floor = result.floor.0,
                            high_water = high_water.0,
                            slowest_consumer = %slowest.name,
                            slowest_lag = slowest.lag,
                            "outbox retention: floor held back by the slowest registered consumer"
                        );
                    }
                }

                match outbox
                    .prune_before(&collection, result.floor, prune_batch_size)
                    .await
                {
                    Ok(removed) if removed > 0 => {
                        tracing::info!(
                            collection = %collection.id,
                            removed,
                            floor = result.floor.0,
                            "outbox retention: pruned obligations at or below the computed floor"
                        );
                    }
                    Ok(_) => {}
                    Err(error) => {
                        // Deliberately `debug`, not `warn`: the common case
                        // here is simply "this driver never implemented
                        // `prune_before`" (the trait's own default refusal),
                        // which would otherwise repeat every tick forever
                        // for a driver that never will. The floor/lag
                        // metrics above are already reported regardless.
                        tracing::debug!(
                            collection = %collection.id,
                            %error,
                            "outbox retention: pruning did not run this tick (unsupported by this driver, or the attempt failed); floor still computed and reported"
                        );
                    }
                }
            }
            Err(error) => {
                tracing::error!(
                    collection = %collection.id,
                    %error,
                    "outbox retention pass failed to read the primary high-water mark; retrying next tick"
                );
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(poll_interval) => {}
            result = shutdown.changed() => {
                if result.is_err() || *shutdown.borrow() {
                    return;
                }
            }
        }
    }
}
