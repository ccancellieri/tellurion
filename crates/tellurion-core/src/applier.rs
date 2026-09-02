//! The driver-agnostic pump between an [`OutboxSource`] and an [`IndexSink`]
//! (`#67`, the derived-index half of the transactional-outbox design doc,
//! section 3.3): read obligations after the sink's own durable high-water,
//! apply them to the sink in ascending order, repeat. It owns no business
//! logic and no cursor of its own — [`IndexSink::applied_high_water`] IS the
//! cursor, which is what makes this restart-safe with no extra bookkeeping:
//! a fresh [`drain_once`] call after a crash simply resumes from wherever
//! the sink itself last durably landed.
//!
//! A poison obligation (one `IndexSink::apply` rejects) is never skipped:
//! [`drain_once`] returns the error and stops mid-batch, so
//! `applied_high_water` does not advance past it. [`run_applier`] logs the
//! failure and retries on the next poll tick — the design doc's "a stalled
//! index is a degradation, not an outage" failure mode (section 6).
//!
//! ## Running more than one replica (`#193`)
//!
//! [`run_applier`] optionally takes a [`LeaseBinding`]: with one, a pass
//! only runs while this process holds that collection's lease, so the
//! design doc's "single ordered consumer per collection" (section 2, rule
//! 4) survives a clustered deployment. Without one — the default, and every
//! deployment that never configures a lease — nothing below changes: no new
//! branch runs, no coordinator is contacted, the loop is the loop it always
//! was.
//!
//! Failover costs nothing here precisely because this pump owns no cursor:
//! [`IndexSink::applied_high_water`] IS the cursor, so a follower promoted
//! mid-stream resumes from whatever the *sink* last durably landed, exactly
//! the way a restart does. That is what makes the lease "pure addition ...
//! changing no invariant" (the design doc's section 9) rather than a
//! rework — there is no leader-local state a takeover could lose. A brief
//! overlap between an outgoing and an incoming leader is likewise harmless:
//! `IndexSink::apply` is version-gated and idempotent by contract, so the
//! lease buys ordering stability and wasted-work avoidance, never
//! correctness the apply path does not already own (`lease.rs`'s own "not a
//! fencing token" note).

use std::sync::Arc;
use std::time::Duration;

use crate::config::CollectionDecl;
use crate::error::Result;
use crate::lease::{LeaseBinding, LeaseGuard};
use crate::outbox::{IndexSink, OutboxSource};

/// One drain pass: reads `index`'s own `applied_high_water` as the resume
/// cursor, pulls at most `batch_size` obligations strictly after it from
/// `outbox`, and applies each to `index` in the ascending order
/// `OutboxSource::read_after` returns them in. Returns how many obligations
/// were applied. Stops at (and returns) the first error `IndexSink::apply`
/// raises, without applying anything after it — see this module's own doc
/// for why that is the whole point, not a shortcoming.
pub async fn drain_once(
    outbox: &dyn OutboxSource,
    index: &dyn IndexSink,
    collection: &CollectionDecl,
    batch_size: u32,
) -> Result<usize> {
    let cursor = index.applied_high_water(collection).await?;
    let obligations = outbox.read_after(collection, cursor, batch_size).await?;
    for obligation in &obligations {
        index.apply(collection, obligation).await?;
    }
    Ok(obligations.len())
}

/// Runs [`drain_once`] on a fixed `poll_interval` until `shutdown` reports
/// `true`, then returns — the background-task shape `tellurion-server`'s
/// config-gated applier wiring spawns one of per index-routed collection.
/// A failed pass is logged and retried on the next tick rather than ending
/// the loop: per the design doc, a stalled index degrades search freshness,
/// it is not a reason to stop draining forever (an operator fixing the
/// underlying cause needs the loop to still be there to resume).
///
/// `lease` is `None` for a single-process deployment — the default, and the
/// path this loop always took: no coordinator is contacted and every tick
/// drains. `Some` makes the task a candidate leader instead: it drains only
/// while it holds the collection's lease, and otherwise keeps polling as a
/// follower so a takeover is one tick away (`#193`, and this module's own
/// doc for why a takeover needs nothing else). Leadership is released when
/// this function returns — shutdown, in practice — because the guard lives
/// in the loop's own state and drops with it, so the successor does not
/// have to wait out a timeout that nobody is going to renew.
pub async fn run_applier(
    outbox: Arc<dyn OutboxSource>,
    index: Arc<dyn IndexSink>,
    collection: CollectionDecl,
    batch_size: u32,
    poll_interval: Duration,
    lease: Option<LeaseBinding>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut leadership = Leadership::default();
    loop {
        if leads_this_pass(lease.as_ref(), &mut leadership, &collection).await {
            if let Err(error) =
                drain_once(outbox.as_ref(), index.as_ref(), &collection, batch_size).await
            {
                tracing::error!(
                    collection = %collection.id,
                    %error,
                    "index applier pass failed; resuming from the last durable high-water on the next tick"
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

/// One leased applier task's view of its own leadership, kept across ticks
/// so the lease is acquired once and held rather than re-raced every poll
/// interval. `announced_follower` exists only so a healthy standby replica
/// logs "somebody else leads" once instead of once per tick, for as long as
/// that stays true.
#[derive(Default)]
struct Leadership {
    held: Option<LeaseGuard>,
    announced_follower: bool,
}

/// Whether this task may run a drain pass right now. Unleased (`lease:
/// None`) is unconditionally `true` — the whole single-process path, with
/// no branch below ever evaluated.
async fn leads_this_pass(
    lease: Option<&LeaseBinding>,
    leadership: &mut Leadership,
    collection: &CollectionDecl,
) -> bool {
    let Some(lease) = lease else {
        return true;
    };

    // Still holding a live lease: keep leading, ask the coordinator
    // nothing. Sticky leadership is the point — a lease re-raced every tick
    // would hand the collection back and forth between replicas and lose
    // the ordering stability it exists to provide.
    if leadership.held.as_ref().is_some_and(LeaseGuard::is_live) {
        return true;
    }

    // A guard whose backing resource died is a stale belief, not
    // leadership: somebody else may already have taken over. Drop it and
    // re-race rather than drain on its strength.
    if leadership.held.take().is_some() {
        leadership.announced_follower = false;
        tracing::warn!(
            collection = %collection.id,
            lease = %lease.key,
            "index applier: the leader lease is no longer held; re-acquiring before the next pass"
        );
    }

    match lease.try_acquire().await {
        Ok(Some(guard)) => {
            tracing::info!(
                collection = %collection.id,
                lease = %guard.key(),
                "index applier: acquired the leader lease; draining"
            );
            leadership.held = Some(guard);
            leadership.announced_follower = false;
            true
        }
        Ok(None) => {
            if !leadership.announced_follower {
                leadership.announced_follower = true;
                tracing::info!(
                    collection = %collection.id,
                    lease = %lease.key,
                    "index applier: another replica holds the leader lease; polling as a follower"
                );
            }
            false
        }
        Err(error) => {
            // "I could not ask" is never "nobody leads" — see
            // `Lease::try_acquire`'s own contract. Skipping the pass costs
            // index freshness until the coordinator is reachable again,
            // which is the same degradation a failed pass already costs.
            leadership.announced_follower = false;
            tracing::warn!(
                collection = %collection.id,
                lease = %lease.key,
                %error,
                "index applier: could not reach the lease coordinator; skipping this pass"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;
    use crate::lease::{Lease, LeaseHold, LeaseKey, INDEX_APPLIER_CONSUMER};
    use crate::outbox::{MutationKind, Obligation, Sequence};

    fn collection() -> CollectionDecl {
        serde_yaml::from_str(
            r#"
id: demo
catalog: default
storage: main
table: demo
geometry: geom
pk: id
"#,
        )
        .unwrap()
    }

    /// In-memory `OutboxSource` fixture: a fixed, ordered obligation log.
    struct FakeOutbox {
        obligations: Vec<Obligation>,
    }

    #[async_trait]
    impl OutboxSource for FakeOutbox {
        async fn read_after(
            &self,
            _collection: &CollectionDecl,
            after: Sequence,
            limit: u32,
        ) -> Result<Vec<Obligation>> {
            Ok(self
                .obligations
                .iter()
                .filter(|o| o.sequence > after)
                .take(limit as usize)
                .cloned()
                .collect())
        }

        async fn primary_high_water(&self, _collection: &CollectionDecl) -> Result<Sequence> {
            Ok(self
                .obligations
                .last()
                .map(|o| o.sequence)
                .unwrap_or(Sequence(0)))
        }
    }

    /// In-memory `IndexSink` fixture: version-gated upsert per `feature_id`,
    /// applied-count tracked so ordering/idempotence tests can assert on it
    /// directly, and an optional "reject this sequence" hook to exercise the
    /// poison-obligation halt behavior.
    #[derive(Default)]
    struct FakeIndex {
        documents: Mutex<HashMap<String, (Sequence, MutationKind)>>,
        applied_order: Mutex<Vec<Sequence>>,
        reject: Option<Sequence>,
    }

    #[async_trait]
    impl IndexSink for FakeIndex {
        async fn apply(&self, _collection: &CollectionDecl, obligation: &Obligation) -> Result<()> {
            if self.reject == Some(obligation.sequence) {
                return Err(crate::error::Error::Storage(Box::new(
                    std::io::Error::other("poison obligation"),
                )));
            }
            let mut documents = self.documents.lock().unwrap();
            let entry = documents.get(&obligation.feature_id);
            if entry.is_none_or(|(stored, _)| obligation.version > *stored) {
                documents.insert(
                    obligation.feature_id.clone(),
                    (obligation.version, obligation.kind.clone()),
                );
            }
            self.applied_order.lock().unwrap().push(obligation.sequence);
            Ok(())
        }

        async fn applied_high_water(&self, _collection: &CollectionDecl) -> Result<Sequence> {
            Ok(self
                .documents
                .lock()
                .unwrap()
                .values()
                .map(|(version, _)| *version)
                .max()
                .unwrap_or(Sequence(0)))
        }
    }

    fn upsert(sequence: u64, feature_id: &str) -> Obligation {
        Obligation {
            sequence: Sequence(sequence),
            feature_id: feature_id.to_string(),
            kind: MutationKind::Upsert(serde_json::json!({"id": feature_id})),
            version: Sequence(sequence),
            committed_at: std::time::SystemTime::UNIX_EPOCH,
            extent: crate::outbox::ObligationExtent::Unrecorded,
        }
    }

    #[tokio::test]
    async fn drain_once_applies_every_obligation_in_ascending_order() {
        let outbox = FakeOutbox {
            obligations: vec![upsert(1, "a"), upsert(2, "b"), upsert(3, "a")],
        };
        let index = FakeIndex::default();
        let applied = drain_once(&outbox, &index, &collection(), 100)
            .await
            .unwrap();
        assert_eq!(applied, 3);
        assert_eq!(
            *index.applied_order.lock().unwrap(),
            vec![Sequence(1), Sequence(2), Sequence(3)]
        );
        assert_eq!(
            index.applied_high_water(&collection()).await.unwrap(),
            Sequence(3)
        );
    }

    #[tokio::test]
    async fn drain_once_resumes_from_the_sinks_own_high_water_restart_safe() {
        let outbox = FakeOutbox {
            obligations: vec![upsert(1, "a"), upsert(2, "b"), upsert(3, "c")],
        };
        let index = FakeIndex::default();

        // First pass only sees a small batch — simulates a crash after
        // partial progress.
        let applied = drain_once(&outbox, &index, &collection(), 2).await.unwrap();
        assert_eq!(applied, 2);
        assert_eq!(
            index.applied_high_water(&collection()).await.unwrap(),
            Sequence(2)
        );

        // "Restart": a fresh drain call against the SAME sink resumes past
        // what was already durably applied, never re-reading sequence 1/2.
        let applied = drain_once(&outbox, &index, &collection(), 100)
            .await
            .unwrap();
        assert_eq!(applied, 1);
        assert_eq!(
            *index.applied_order.lock().unwrap(),
            vec![Sequence(1), Sequence(2), Sequence(3)]
        );
    }

    #[tokio::test]
    async fn drain_once_applying_the_same_batch_twice_is_harmless() {
        let outbox = FakeOutbox {
            obligations: vec![upsert(1, "a"), upsert(2, "a")],
        };
        let index = FakeIndex::default();
        drain_once(&outbox, &index, &collection(), 100)
            .await
            .unwrap();
        // Re-draining after the cursor already caught up reads nothing new
        // (`read_after` is exclusive of `after`) — at-least-once redelivery
        // at the outbox layer itself is `OutboxSource`'s own concern; what
        // this proves is that the sink's version-gated `apply` makes a
        // repeat delivery of an already-applied obligation a no-op rather
        // than corrupting state, exercised directly here.
        let obligation = upsert(2, "a");
        index.apply(&collection(), &obligation).await.unwrap();
        index.apply(&collection(), &obligation).await.unwrap();
        let documents = index.documents.lock().unwrap();
        assert_eq!(documents.get("a").unwrap().0, Sequence(2));
    }

    #[tokio::test]
    async fn drain_once_halts_at_a_poison_obligation_without_skipping_it() {
        let outbox = FakeOutbox {
            obligations: vec![upsert(1, "a"), upsert(2, "b"), upsert(3, "c")],
        };
        let index = FakeIndex {
            reject: Some(Sequence(2)),
            ..Default::default()
        };
        let err = drain_once(&outbox, &index, &collection(), 100)
            .await
            .unwrap_err();
        assert!(matches!(err, crate::error::Error::Storage(_)));
        // Sequence 1 landed; 2 and 3 did not — the high-water never crosses
        // the poison obligation.
        assert_eq!(
            index.applied_high_water(&collection()).await.unwrap(),
            Sequence(1)
        );

        // Fixing the sink and re-draining resumes exactly at the stalled
        // sequence, never skipping it.
        let index = FakeIndex {
            documents: index.documents,
            applied_order: index.applied_order,
            reject: None,
        };
        let applied = drain_once(&outbox, &index, &collection(), 100)
            .await
            .unwrap();
        assert_eq!(applied, 2);
        assert_eq!(
            *index.applied_order.lock().unwrap(),
            vec![Sequence(1), Sequence(2), Sequence(3)]
        );
    }

    #[tokio::test]
    async fn run_applier_stops_promptly_on_shutdown() {
        let outbox = Arc::new(FakeOutbox {
            obligations: vec![upsert(1, "a")],
        });
        let index: Arc<FakeIndex> = Arc::new(FakeIndex::default());
        let (tx, rx) = tokio::sync::watch::channel(false);
        let handle = tokio::spawn(run_applier(
            outbox,
            Arc::clone(&index) as Arc<dyn IndexSink>,
            collection(),
            10,
            Duration::from_secs(3600),
            None,
            rx,
        ));
        // Give the first pass a chance to run before signalling shutdown.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            index.applied_high_water(&collection()).await.unwrap(),
            Sequence(1)
        );
        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("run_applier should return promptly after shutdown")
            .unwrap();
    }

    // ---- the clustered lease (`#193`) ----

    /// A scripted coordinator: each `try_acquire` consumes the next planned
    /// outcome (the last one repeats once the script runs out), and every
    /// call is counted so a test can pin that a held lease is NOT re-raced
    /// every tick.
    struct FakeLease {
        script: Mutex<VecDeque<Outcome>>,
        calls: AtomicUsize,
    }

    enum Outcome {
        /// Leadership granted; the returned hold stays live until the
        /// shared flag is flipped (a coordinator connection dying under
        /// the leader's feet).
        Granted(Arc<AtomicBool>),
        /// Somebody else leads right now — an ordinary answer.
        Taken,
        /// The coordinator could not be asked at all.
        Unreachable,
    }

    struct FakeHold {
        live: Arc<AtomicBool>,
    }

    impl LeaseHold for FakeHold {
        fn is_live(&self) -> bool {
            self.live.load(Ordering::SeqCst)
        }
    }

    impl FakeLease {
        fn scripted(outcomes: impl IntoIterator<Item = Outcome>) -> Arc<Self> {
            Arc::new(Self {
                script: Mutex::new(outcomes.into_iter().collect()),
                calls: AtomicUsize::new(0),
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl Lease for FakeLease {
        async fn try_acquire(&self, key: &LeaseKey) -> Result<Option<LeaseGuard>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut script = self.script.lock().unwrap();
            let outcome = if script.len() > 1 {
                script.pop_front().unwrap()
            } else {
                match script.front() {
                    Some(Outcome::Granted(live)) => Outcome::Granted(Arc::clone(live)),
                    Some(Outcome::Taken) => Outcome::Taken,
                    Some(Outcome::Unreachable) | None => Outcome::Unreachable,
                }
            };
            match outcome {
                Outcome::Granted(live) => Ok(Some(LeaseGuard::new(
                    key.clone(),
                    Box::new(FakeHold { live }),
                ))),
                Outcome::Taken => Ok(None),
                Outcome::Unreachable => Err(crate::error::Error::Storage(Box::new(
                    std::io::Error::other("coordinator unreachable"),
                ))),
            }
        }
    }

    fn binding(lease: Arc<FakeLease>) -> LeaseBinding {
        LeaseBinding::new(
            lease as Arc<dyn Lease>,
            LeaseKey::for_collection(None, INDEX_APPLIER_CONSUMER, "public", "default", "demo"),
        )
    }

    /// The default posture: no lease configured means no coordinator is
    /// ever consulted and every pass runs — the single-process behavior
    /// this loop always had, unchanged by `#193` existing.
    #[tokio::test]
    async fn without_a_lease_every_pass_runs() {
        let mut leadership = Leadership::default();
        for _ in 0..3 {
            assert!(leads_this_pass(None, &mut leadership, &collection()).await);
        }
        assert!(leadership.held.is_none());
    }

    /// Leadership is sticky: acquired once, then held across ticks. A lease
    /// re-raced every poll interval would bounce the collection between
    /// replicas and throw away the ordering stability it exists to buy.
    #[tokio::test]
    async fn a_held_lease_is_not_re_raced_every_tick() {
        let live = Arc::new(AtomicBool::new(true));
        let lease = FakeLease::scripted([Outcome::Granted(Arc::clone(&live))]);
        let binding = binding(Arc::clone(&lease));
        let mut leadership = Leadership::default();
        for _ in 0..5 {
            assert!(leads_this_pass(Some(&binding), &mut leadership, &collection()).await);
        }
        assert_eq!(lease.calls(), 1);
    }

    /// A follower skips its passes — and takes over on the very tick the
    /// incumbent lets go, with no cursor handover of any kind: the sink's
    /// own `applied_high_water` is the only cursor there is.
    #[tokio::test]
    async fn a_follower_skips_passes_and_takes_over_the_moment_the_lease_frees_up() {
        let live = Arc::new(AtomicBool::new(true));
        let lease = FakeLease::scripted([
            Outcome::Taken,
            Outcome::Taken,
            Outcome::Granted(Arc::clone(&live)),
        ]);
        let binding = binding(Arc::clone(&lease));
        let mut leadership = Leadership::default();

        assert!(!leads_this_pass(Some(&binding), &mut leadership, &collection()).await);
        assert!(!leads_this_pass(Some(&binding), &mut leadership, &collection()).await);
        assert!(leads_this_pass(Some(&binding), &mut leadership, &collection()).await);
        assert_eq!(lease.calls(), 3);
    }

    /// Holding a guard whose backing resource died is a stale belief, not
    /// leadership: the pass is skipped and the lease re-raced, because
    /// somebody else may already have taken over.
    #[tokio::test]
    async fn a_lease_lost_under_the_leaders_feet_stops_the_pass_and_is_re_raced() {
        let live = Arc::new(AtomicBool::new(true));
        let lease = FakeLease::scripted([
            Outcome::Granted(Arc::clone(&live)),
            Outcome::Taken,
            Outcome::Granted(Arc::new(AtomicBool::new(true))),
        ]);
        let binding = binding(Arc::clone(&lease));
        let mut leadership = Leadership::default();

        assert!(leads_this_pass(Some(&binding), &mut leadership, &collection()).await);

        // The coordinator connection dies; the guard is still in hand but
        // no longer means anything.
        live.store(false, Ordering::SeqCst);
        assert!(!leads_this_pass(Some(&binding), &mut leadership, &collection()).await);
        assert!(leadership.held.is_none());

        // Re-raced and won back on the next tick.
        assert!(leads_this_pass(Some(&binding), &mut leadership, &collection()).await);
        assert_eq!(lease.calls(), 3);
    }

    /// The rule that makes the whole seam safe: an unreachable coordinator
    /// is "I don't know", never "nobody leads". A pass must not run on it.
    #[tokio::test]
    async fn an_unreachable_coordinator_is_never_permission_to_lead() {
        let lease = FakeLease::scripted([Outcome::Unreachable]);
        let binding = binding(Arc::clone(&lease));
        let mut leadership = Leadership::default();
        for _ in 0..3 {
            assert!(!leads_this_pass(Some(&binding), &mut leadership, &collection()).await);
        }
        assert!(leadership.held.is_none());
    }

    /// End to end through the actual loop: a task that never wins the
    /// lease drains nothing at all, yet still stops promptly on shutdown
    /// (a follower is a live standby, not a parked task).
    #[tokio::test]
    async fn a_leased_run_applier_that_never_leads_applies_nothing() {
        let outbox = Arc::new(FakeOutbox {
            obligations: vec![upsert(1, "a")],
        });
        let index: Arc<FakeIndex> = Arc::new(FakeIndex::default());
        let (tx, rx) = tokio::sync::watch::channel(false);
        let handle = tokio::spawn(run_applier(
            outbox,
            Arc::clone(&index) as Arc<dyn IndexSink>,
            collection(),
            10,
            Duration::from_millis(5),
            Some(binding(FakeLease::scripted([Outcome::Taken]))),
            rx,
        ));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            index.applied_high_water(&collection()).await.unwrap(),
            Sequence(0)
        );
        assert!(index.applied_order.lock().unwrap().is_empty());
        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("a follower still stops promptly on shutdown")
            .unwrap();
    }

    /// The same loop, leased and winning: it drains exactly as the
    /// unleased one does — the lease gates whether a pass runs, never what
    /// a pass does.
    #[tokio::test]
    async fn a_leased_run_applier_that_leads_drains_exactly_as_the_unleased_one_does() {
        let outbox = Arc::new(FakeOutbox {
            obligations: vec![upsert(1, "a"), upsert(2, "b")],
        });
        let index: Arc<FakeIndex> = Arc::new(FakeIndex::default());
        let (tx, rx) = tokio::sync::watch::channel(false);
        let handle = tokio::spawn(run_applier(
            outbox,
            Arc::clone(&index) as Arc<dyn IndexSink>,
            collection(),
            10,
            Duration::from_millis(5),
            Some(binding(FakeLease::scripted([Outcome::Granted(Arc::new(
                AtomicBool::new(true),
            ))]))),
            rx,
        ));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            index.applied_high_water(&collection()).await.unwrap(),
            Sequence(2)
        );
        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("run_applier should return promptly after shutdown")
            .unwrap();
    }
}
