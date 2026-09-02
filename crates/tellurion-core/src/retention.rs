//! Consumer-aware outbox retention (`#115`): the floor pruning may never
//! advance past, and the per-consumer lag a metric exposes alongside it.
//!
//! One outbox, several independent readers at their own pace — the index
//! applier (`#67`), the write-reactive tile-cache invalidation consumer
//! (`#113`), the change feed's own named consumers, and webhook delivery
//! cursors (`crate::webhooks`). Trimming that would orphan any one of them
//! must be a named, logged decision, never a quiet data loss: [`compute_floor`]
//! is the pure arithmetic half of that rule — given the primary's own
//! high-water mark and every REGISTERED consumer's current cursor, the floor
//! pruning may advance to is the minimum of all of them, never the high
//! water alone. "Registered" means a consumer this deployment actually has
//! running for the collection in question (opted in, resolvable, spawned) —
//! a consumer nobody turned on contributes nothing to this list at all,
//! exactly mirroring `GenerationStore`'s own "an untracked collection never
//! participates" precedent (`tellurion-server`'s consumer-spawn wiring
//! builds this list; this module only ever consumes it).
//!
//! A consumer whose own cursor cannot currently be resolved (its resolve
//! call failed, its backing table is missing, ...) is the caller's
//! responsibility to represent honestly here — reporting it at
//! [`Sequence(0)`](crate::outbox::Sequence) is the conservative choice
//! (blocks all pruning for that collection until the consumer either
//! resolves or is deliberately removed), never simply omitting it (which
//! would silently let retention advance past a consumer that is nominally
//! still registered).

use crate::outbox::Sequence;

/// One registered consumer's current cursor and its lag behind the
/// collection's own primary high-water mark — the per-consumer half of
/// [`RetentionFloor`], and exactly what a lag metric/log line reports one of
/// per registered consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumerLag {
    pub name: String,
    pub cursor: Sequence,
    pub lag: u64,
}

/// The result of one floor computation for one collection: the primary's
/// own high-water mark, the floor pruning may advance to (never past any
/// registered consumer, and never past `high_water` itself), and each
/// registered consumer's own lag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionFloor {
    pub high_water: Sequence,
    pub floor: Sequence,
    pub lags: Vec<ConsumerLag>,
}

/// Computes the retention floor for one collection: `high_water` is the
/// primary's own [`crate::outbox::OutboxSource::primary_high_water`];
/// `consumers` is every registered consumer's `(name, cursor)` pair for this
/// same collection — see the module doc for what "registered" and an
/// unresolvable cursor's own honest representation both mean. No registered
/// consumers at all means nothing constrains pruning: the floor is
/// `high_water` itself (an outbox nobody reads may be pruned in full).
pub fn compute_floor(high_water: Sequence, consumers: &[(String, Sequence)]) -> RetentionFloor {
    let floor = consumers
        .iter()
        .map(|(_, cursor)| *cursor)
        .min()
        .unwrap_or(high_water)
        .min(high_water);
    let lags = consumers
        .iter()
        .map(|(name, cursor)| ConsumerLag {
            name: name.clone(),
            cursor: *cursor,
            lag: high_water.0.saturating_sub(cursor.0),
        })
        .collect();
    RetentionFloor {
        high_water,
        floor,
        lags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_registered_consumers_leaves_the_floor_at_the_high_water_mark() {
        let result = compute_floor(Sequence(100), &[]);
        assert_eq!(result.floor, Sequence(100));
        assert!(result.lags.is_empty());
    }

    #[test]
    fn a_single_caught_up_consumer_does_not_hold_the_floor_back() {
        let consumers = vec![("index".to_string(), Sequence(100))];
        let result = compute_floor(Sequence(100), &consumers);
        assert_eq!(result.floor, Sequence(100));
        assert_eq!(result.lags[0].lag, 0);
    }

    #[test]
    fn a_single_lagging_consumer_pulls_the_floor_down_to_its_own_cursor() {
        let consumers = vec![("index".to_string(), Sequence(40))];
        let result = compute_floor(Sequence(100), &consumers);
        assert_eq!(result.floor, Sequence(40));
        assert_eq!(result.lags[0].lag, 60);
    }

    /// The scenario `#115` explicitly calls for: an index applier and a
    /// tile-generation consumer are both nearly caught up, but one webhook
    /// subscription has fallen far behind — the floor must track the
    /// slowest of the three (the webhook), not an average or a majority.
    #[test]
    fn a_lagging_webhook_subscription_holds_the_floor_back_below_every_other_consumer() {
        let consumers = vec![
            ("index-applier".to_string(), Sequence(98)),
            ("tile-generation".to_string(), Sequence(95)),
            ("webhook:alerts".to_string(), Sequence(12)),
        ];
        let result = compute_floor(Sequence(100), &consumers);
        assert_eq!(
            result.floor,
            Sequence(12),
            "the lagging webhook subscription alone should determine the floor"
        );
        let webhook_lag = result
            .lags
            .iter()
            .find(|lag| lag.name == "webhook:alerts")
            .expect("the webhook consumer's own lag entry should be present");
        assert_eq!(webhook_lag.lag, 88);
        let index_lag = result
            .lags
            .iter()
            .find(|lag| lag.name == "index-applier")
            .unwrap();
        assert_eq!(index_lag.lag, 2);
    }

    #[test]
    fn the_floor_never_exceeds_the_high_water_mark_even_with_a_stale_reading() {
        // Defensive: a consumer cursor somehow ahead of a freshly-read
        // high-water mark (a benign race between the two reads) must never
        // push the floor past what is actually safe to keep.
        let consumers = vec![("index".to_string(), Sequence(150))];
        let result = compute_floor(Sequence(100), &consumers);
        assert_eq!(result.floor, Sequence(100));
    }

    #[test]
    fn an_unresolvable_consumer_reported_at_zero_blocks_all_pruning() {
        let consumers = vec![
            ("index".to_string(), Sequence(90)),
            ("webhook:broken".to_string(), Sequence(0)),
        ];
        let result = compute_floor(Sequence(100), &consumers);
        assert_eq!(result.floor, Sequence(0));
    }
}
