//! Durable polling consumer for dynamic control-store revisions.
//!
//! Notifications are intentionally absent: every refresh starts from the
//! durable revision/outbox contract, so Pgpool session changes and missed
//! wake-ups cannot lose configuration updates.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tellurion_core::{
    ConfigVersion, ControlEventCursor, ControlRevision, ControlRuntimeStatus, ControlStore,
    Registry, RelationalRegistryFactories, RelationalTenantFactories, VersionedControlSnapshot,
};

use crate::metrics;
use crate::readiness::Readiness;
use crate::runtime_activation;

const EVENT_PAGE_SIZE: u32 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshOutcome {
    NoChange,
    Applied(ControlRevision),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshFailureKind {
    Poll,
    Activation,
}

#[derive(Debug)]
struct RefreshFailure {
    kind: RefreshFailureKind,
    error: anyhow::Error,
}

impl RefreshFailure {
    fn poll(error: impl Into<anyhow::Error>) -> Self {
        Self {
            kind: RefreshFailureKind::Poll,
            error: error.into(),
        }
    }

    fn activation(error: impl Into<anyhow::Error>) -> Self {
        Self {
            kind: RefreshFailureKind::Activation,
            error: error.into(),
        }
    }
}

async fn refresh_once<F, Fut, T>(
    store: &dyn ControlStore,
    applied_revision: ControlRevision,
    status: &ControlRuntimeStatus,
    activate: F,
) -> Result<RefreshOutcome, RefreshFailure>
where
    F: FnOnce(VersionedControlSnapshot) -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let Some(store_revision) = store
        .current_revision()
        .await
        .map_err(RefreshFailure::poll)?
    else {
        return Err(RefreshFailure::poll(anyhow::anyhow!(
            "control store became uninitialized"
        )));
    };
    metrics::set_control_store_revision(store_revision);
    metrics::set_control_applied_revision(applied_revision, store_revision);
    status.observe_store_revision(store_revision);
    if store_revision <= applied_revision {
        return Ok(RefreshOutcome::NoChange);
    }

    // Consume every ordered durable event through the revision observed
    // above. A missing page or any revision gap never causes guessing:
    // load_snapshot below is the authoritative recovery path and resumes
    // from the snapshot's own revision.
    let mut cursor = ControlEventCursor {
        revision: applied_revision,
        ordinal: u32::MAX,
    };
    let mut last_revision = applied_revision;
    let mut gap = false;
    while last_revision < store_revision {
        let events = store
            .changes_since(Some(cursor), EVENT_PAGE_SIZE)
            .await
            .map_err(RefreshFailure::poll)?;
        tellurion_core::validate_control_event_page(Some(cursor), &events)
            .map_err(RefreshFailure::poll)?;
        if events.is_empty() {
            gap = true;
            break;
        }
        for event in &events {
            if event.revision > last_revision.saturating_add(1) {
                gap = true;
                break;
            }
            last_revision = last_revision.max(event.revision);
        }
        cursor = events.last().expect("non-empty event page").cursor();
        if gap {
            break;
        }
    }
    if gap {
        tracing::warn!(
            applied_revision,
            store_revision,
            "control consumer: durable event gap; recovering from current snapshot"
        );
    }

    let candidate = store.load_snapshot().await.map_err(RefreshFailure::poll)?;
    let started = Instant::now();
    let activation = activate(candidate.clone()).await;
    metrics::observe_control_activation(started.elapsed());
    activation.map_err(RefreshFailure::activation)?;
    metrics::set_control_applied_revision(candidate.revision, candidate.revision);
    status.observe_applied_revision(candidate.revision);
    Ok(RefreshOutcome::Applied(candidate.revision))
}

pub struct ControlConsumerContext {
    ctx: Arc<tellurion_core::AppContext>,
    registry: Arc<Registry>,
    relational_registry_factories: Arc<RelationalRegistryFactories>,
    relational_tenant_factories: Arc<RelationalTenantFactories>,
    readiness: Readiness,
}

impl ControlConsumerContext {
    pub fn new(
        ctx: Arc<tellurion_core::AppContext>,
        registry: Arc<Registry>,
        relational_registry_factories: Arc<RelationalRegistryFactories>,
        relational_tenant_factories: Arc<RelationalTenantFactories>,
        readiness: Readiness,
    ) -> Self {
        Self {
            ctx,
            registry,
            relational_registry_factories,
            relational_tenant_factories,
            readiness,
        }
    }
}

pub async fn run_control_consumer(
    runtime: ControlConsumerContext,
    store: Arc<dyn ControlStore>,
    poll_interval: Duration,
    applied_revision: ControlRevision,
    status: Arc<ControlRuntimeStatus>,
) {
    let runtime = &runtime;
    run_refresh_loop(
        store,
        poll_interval,
        applied_revision,
        status,
        |candidate| async move {
            let version =
                ConfigVersion::from_wire(format!("control-revision-{}", candidate.revision));
            let snapshot = candidate.snapshot;
            runtime_activation::activate_config(
                &runtime.ctx,
                runtime_activation::RuntimeCandidate {
                    config: snapshot.config,
                    role_bindings: snapshot.role_bindings,
                    control_revision: Some(candidate.revision),
                    // `#215`: convergence carries the statements the same
                    // way it already carries the bindings, so a policy edit
                    // reaches every replica through the one durable path.
                    path_policies: snapshot.path_policies,
                },
                version,
                &runtime.registry,
                &runtime.relational_registry_factories,
                &runtime.relational_tenant_factories,
                &runtime.readiness,
            )
            .await
        },
    )
    .await;
}

async fn run_refresh_loop<F, Fut, T>(
    store: Arc<dyn ControlStore>,
    poll_interval: Duration,
    mut applied_revision: ControlRevision,
    status: Arc<ControlRuntimeStatus>,
    mut activate: F,
) where
    F: FnMut(VersionedControlSnapshot) -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    metrics::set_control_applied_revision(applied_revision, applied_revision);
    let mut consecutive_failures = 0u32;
    let jitter_seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
        ^ u64::from(std::process::id());
    let mut poll_sequence = 0u64;
    loop {
        let result = refresh_once(
            store.as_ref(),
            applied_revision,
            status.as_ref(),
            &mut activate,
        )
        .await;
        match result {
            Ok(RefreshOutcome::Applied(revision)) => {
                applied_revision = revision;
                consecutive_failures = 0;
                metrics::record_control_refresh_success();
                status.record_refresh_success();
            }
            Ok(RefreshOutcome::NoChange) => {
                consecutive_failures = 0;
                metrics::record_control_refresh_success();
                status.record_refresh_success();
            }
            Err(failure) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                match failure.kind {
                    RefreshFailureKind::Poll => {
                        metrics::record_control_poll_failure();
                        status.record_poll_failure();
                    }
                    RefreshFailureKind::Activation => {
                        metrics::record_control_activation_failure();
                        status.record_activation_failure();
                    }
                }
                tracing::error!(
                    error = %failure.error,
                    failure_kind = ?failure.kind,
                    applied_revision,
                    "control consumer: refresh failed; retaining last known-good snapshot"
                );
            }
        }
        poll_sequence = poll_sequence.wrapping_add(1);
        tokio::time::sleep(retry_interval(
            poll_interval,
            consecutive_failures,
            jitter_seed ^ applied_revision ^ poll_sequence,
        ))
        .await;
    }
}

fn retry_interval(base: Duration, consecutive_failures: u32, entropy: u64) -> Duration {
    let multiplier = 1u32 << consecutive_failures.min(5);
    let backoff = base.saturating_mul(multiplier).min(Duration::from_secs(30));
    jittered(backoff, entropy)
}

fn jittered(base: Duration, entropy: u64) -> Duration {
    // Per-process seed plus poll sequence, bounded to 0–10%. Correctness
    // never depends on it; it only prevents replicas from polling in
    // permanent lockstep.
    let percent = entropy.wrapping_mul(37) % 11;
    base.saturating_add(base.saturating_mul(percent as u32) / 100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use tellurion_core::{
        authorize_control_mutation, initialize_control_store, AppConfig, AuthenticatedSubject,
        AuthorizedControlMutation, BootstrapOutcome, ControlAuditRecord, ControlBootstrapMode,
        ControlChangeSet, ControlCommit, ControlEvent, ControlRouteDescriptor,
        ControlRouteRegistry, ControlRuntimeStatus, ControlScope, ControlSnapshot,
        InMemoryControlStore, PrincipalIdentity, RoleBinding,
    };

    struct ObservedStore {
        inner: InMemoryControlStore,
        hide_events: bool,
        event_reads: AtomicUsize,
    }

    impl ObservedStore {
        fn new(hide_events: bool) -> Self {
            Self {
                inner: InMemoryControlStore::new(),
                hide_events,
                event_reads: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl ControlStore for ObservedStore {
        async fn bootstrap_if_empty(
            &self,
            seed: &ControlSnapshot,
            actor: &PrincipalIdentity,
            mode: ControlBootstrapMode,
        ) -> tellurion_core::Result<BootstrapOutcome> {
            self.inner.bootstrap_if_empty(seed, actor, mode).await
        }

        async fn current_revision(&self) -> tellurion_core::Result<Option<ControlRevision>> {
            self.inner.current_revision().await
        }

        async fn load_snapshot(&self) -> tellurion_core::Result<VersionedControlSnapshot> {
            self.inner.load_snapshot().await
        }

        async fn transact(
            &self,
            authorization: &AuthorizedControlMutation,
            changes: &ControlChangeSet,
        ) -> tellurion_core::Result<ControlCommit> {
            self.inner.transact(authorization, changes).await
        }

        async fn changes_since(
            &self,
            after: Option<ControlEventCursor>,
            limit: u32,
        ) -> tellurion_core::Result<Vec<ControlEvent>> {
            self.event_reads.fetch_add(1, Ordering::SeqCst);
            if self.hide_events {
                Ok(Vec::new())
            } else {
                self.inner.changes_since(after, limit).await
            }
        }

        async fn audit_since(
            &self,
            after: ControlRevision,
            limit: u32,
        ) -> tellurion_core::Result<Vec<ControlAuditRecord>> {
            self.inner.audit_since(after, limit).await
        }
    }

    fn actor() -> PrincipalIdentity {
        PrincipalIdentity {
            issuer: "https://issuer.example".to_string(),
            subject: "operator".to_string(),
        }
    }

    fn seed(port: u16) -> ControlSnapshot {
        let mut config: AppConfig = serde_yaml::from_str(
            "auth:\n  trusted_issuers:\n    - { issuer: https://issuer.example, audience: tellurion-test, claims: { tenants: tenants } }",
        )
        .unwrap();
        config.server.port = port;
        ControlSnapshot {
            config,
            role_bindings: vec![RoleBinding {
                principal: actor(),
                role: "sysadmin".to_string(),
                scope: ControlScope::Platform,
            }],
            path_policies: Vec::new(),
            tombstoned_resources: Vec::new(),
        }
    }

    async fn replace_platform(store: &dyn ControlStore, expected: ControlRevision, port: u16) {
        let versioned = store.load_snapshot().await.unwrap();
        assert_eq!(versioned.revision, expected);
        let path = "/_control/v1/platform/import";
        let route = ControlRouteDescriptor::PlatformBatchImport;
        let registry = ControlRouteRegistry::new([route]).unwrap();
        let changes = ControlChangeSet {
            idempotency_key: None,
            operations: vec![tellurion_core::VersionedControlOperation {
                expected_entity_version: None,
                operation: tellurion_core::ControlOperation::ReplacePlatformSettings(
                    seed(port).config,
                ),
            }],
        };
        let authorization = authorize_control_mutation(
            &AuthenticatedSubject {
                principal: actor(),
                claims: std::collections::HashMap::new(),
            },
            "POST",
            path.as_bytes(),
            route.template(),
            &registry,
            "",
            &versioned,
            &changes,
            &format!("revision-{}", expected + 1),
        )
        .unwrap();
        store.transact(&authorization, &changes).await.unwrap();
    }

    async fn advance_poll_loop(duration: Duration) {
        tokio::time::advance(duration).await;
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn no_change_and_duplicate_polls_do_not_activate() {
        let store = InMemoryControlStore::new();
        initialize_control_store(&store, Some(&seed(8_000)), &actor())
            .await
            .unwrap();
        let status = ControlRuntimeStatus::new(1);
        for _ in 0..2 {
            let outcome = refresh_once(&store, 1, &status, |_| async { Ok(()) })
                .await
                .unwrap();
            assert_eq!(outcome, RefreshOutcome::NoChange);
        }
    }

    #[tokio::test]
    async fn failed_activation_retains_revision_and_later_recovers() {
        let store = InMemoryControlStore::new();
        initialize_control_store(&store, Some(&seed(8_000)), &actor())
            .await
            .unwrap();
        let changed = seed(8_001);
        replace_platform(&store, 1, changed.config.server.port).await;
        let status = ControlRuntimeStatus::new(1);

        assert!(refresh_once(&store, 1, &status, |_| async {
            Err::<(), _>(anyhow::anyhow!("invalid candidate"))
        })
        .await
        .is_err());
        assert_eq!(
            refresh_once(&store, 1, &status, |_| async { Ok(()) })
                .await
                .unwrap(),
            RefreshOutcome::Applied(2)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn paused_polling_skips_unchanged_and_duplicate_revisions() {
        let store = Arc::new(InMemoryControlStore::new());
        initialize_control_store(store.as_ref(), Some(&seed(8_000)), &actor())
            .await
            .unwrap();
        let status = Arc::new(ControlRuntimeStatus::new(1));
        let activations = Arc::new(AtomicUsize::new(0));
        let activations_in_loop = Arc::clone(&activations);
        let store_in_loop: Arc<dyn ControlStore> = store;
        let task = tokio::spawn(run_refresh_loop(
            store_in_loop,
            Duration::from_secs(1),
            1,
            Arc::clone(&status),
            move |_| {
                let activations = Arc::clone(&activations_in_loop);
                async move {
                    activations.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
        ));

        tokio::task::yield_now().await;
        advance_poll_loop(Duration::from_secs(10)).await;
        assert_eq!(activations.load(Ordering::SeqCst), 0);
        let snapshot = status.snapshot();
        assert_eq!(snapshot.store_revision, 1);
        assert_eq!(snapshot.applied_revision, 1);
        assert_eq!(snapshot.lag, 0);
        assert!(snapshot.last_successful_refresh_unix_ms.is_some());
        assert_eq!(snapshot.poll_failures, 0);
        assert_eq!(snapshot.activation_failures, 0);
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn paused_polling_coalesces_missed_revisions_and_does_not_reapply() {
        let store = Arc::new(InMemoryControlStore::new());
        initialize_control_store(store.as_ref(), Some(&seed(8_000)), &actor())
            .await
            .unwrap();
        let status = Arc::new(ControlRuntimeStatus::new(1));
        let activations = Arc::new(AtomicUsize::new(0));
        let latest = Arc::new(AtomicU64::new(0));
        let activations_in_loop = Arc::clone(&activations);
        let latest_in_loop = Arc::clone(&latest);
        let store_in_loop: Arc<dyn ControlStore> = store.clone();
        let task = tokio::spawn(run_refresh_loop(
            store_in_loop,
            Duration::from_secs(1),
            1,
            status,
            move |candidate| {
                let activations = Arc::clone(&activations_in_loop);
                let latest = Arc::clone(&latest_in_loop);
                async move {
                    activations.fetch_add(1, Ordering::SeqCst);
                    latest.store(candidate.revision, Ordering::SeqCst);
                    Ok(())
                }
            },
        ));

        tokio::task::yield_now().await;
        replace_platform(store.as_ref(), 1, 8_001).await;
        replace_platform(store.as_ref(), 2, 8_002).await;
        advance_poll_loop(Duration::from_secs(2)).await;
        assert_eq!(activations.load(Ordering::SeqCst), 1);
        assert_eq!(latest.load(Ordering::SeqCst), 3);
        advance_poll_loop(Duration::from_secs(2)).await;
        assert_eq!(activations.load(Ordering::SeqCst), 1);
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn paused_polling_retries_invalid_candidate_without_advancing() {
        let store = Arc::new(InMemoryControlStore::new());
        initialize_control_store(store.as_ref(), Some(&seed(8_000)), &actor())
            .await
            .unwrap();
        let status = Arc::new(ControlRuntimeStatus::new(1));
        let attempts = Arc::new(AtomicUsize::new(0));
        let applied = Arc::new(AtomicU64::new(0));
        let attempts_in_loop = Arc::clone(&attempts);
        let applied_in_loop = Arc::clone(&applied);
        let store_in_loop: Arc<dyn ControlStore> = store.clone();
        let task = tokio::spawn(run_refresh_loop(
            store_in_loop,
            Duration::from_secs(1),
            1,
            Arc::clone(&status),
            move |candidate| {
                let attempts = Arc::clone(&attempts_in_loop);
                let applied = Arc::clone(&applied_in_loop);
                async move {
                    if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                        anyhow::bail!("invalid candidate")
                    }
                    applied.store(candidate.revision, Ordering::SeqCst);
                    Ok(())
                }
            },
        ));

        tokio::task::yield_now().await;
        advance_poll_loop(Duration::from_secs(1)).await;
        let last_successful_refresh = status
            .snapshot()
            .last_successful_refresh_unix_ms
            .expect("initial no-change poll succeeds");
        replace_platform(store.as_ref(), 1, 8_001).await;
        advance_poll_loop(Duration::from_secs(2)).await;
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(applied.load(Ordering::SeqCst), 0);
        let failed_snapshot = status.snapshot();
        assert_eq!(failed_snapshot.store_revision, 2);
        assert_eq!(failed_snapshot.applied_revision, 1);
        assert_eq!(failed_snapshot.lag, 1);
        assert_eq!(
            failed_snapshot.last_successful_refresh_unix_ms,
            Some(last_successful_refresh)
        );
        assert_eq!(failed_snapshot.poll_failures, 0);
        assert_eq!(failed_snapshot.activation_failures, 1);
        advance_poll_loop(Duration::from_secs(3)).await;
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(applied.load(Ordering::SeqCst), 2);
        let recovered_snapshot = status.snapshot();
        assert_eq!(recovered_snapshot.store_revision, 2);
        assert_eq!(recovered_snapshot.applied_revision, 2);
        assert_eq!(recovered_snapshot.lag, 0);
        assert!(
            recovered_snapshot
                .last_successful_refresh_unix_ms
                .expect("successful activation records its refresh")
                > last_successful_refresh,
            "successful activation must replace the earlier no-change refresh timestamp"
        );
        assert_eq!(recovered_snapshot.poll_failures, 0);
        assert_eq!(recovered_snapshot.activation_failures, 1);
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn paused_polling_records_a_poll_failure() {
        let store: Arc<dyn ControlStore> = Arc::new(InMemoryControlStore::new());
        let status = Arc::new(ControlRuntimeStatus::new(1));
        let task = tokio::spawn(run_refresh_loop(
            store,
            Duration::from_secs(1),
            1,
            Arc::clone(&status),
            |_| async { Ok(()) },
        ));

        tokio::task::yield_now().await;
        advance_poll_loop(Duration::from_secs(1)).await;

        let snapshot = status.snapshot();
        assert_eq!(snapshot.store_revision, 1);
        assert_eq!(snapshot.applied_revision, 1);
        assert_eq!(snapshot.lag, 0);
        assert_eq!(snapshot.last_successful_refresh_unix_ms, None);
        assert_eq!(snapshot.poll_failures, 1);
        assert_eq!(snapshot.activation_failures, 0);
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn paused_polling_recovers_missing_events_from_the_authoritative_snapshot() {
        let store = Arc::new(ObservedStore::new(true));
        initialize_control_store(store.as_ref(), Some(&seed(8_000)), &actor())
            .await
            .unwrap();
        let status = Arc::new(ControlRuntimeStatus::new(1));
        let applied = Arc::new(AtomicU64::new(0));
        let applied_in_loop = Arc::clone(&applied);
        let store_in_loop: Arc<dyn ControlStore> = store.clone();
        let task = tokio::spawn(run_refresh_loop(
            store_in_loop,
            Duration::from_secs(1),
            1,
            status,
            move |candidate| {
                let applied = Arc::clone(&applied_in_loop);
                async move {
                    applied.store(candidate.revision, Ordering::SeqCst);
                    Ok(())
                }
            },
        ));

        tokio::task::yield_now().await;
        replace_platform(store.as_ref(), 1, 8_001).await;
        advance_poll_loop(Duration::from_secs(2)).await;
        assert_eq!(applied.load(Ordering::SeqCst), 2);
        assert!(store.event_reads.load(Ordering::SeqCst) >= 1);
        task.abort();
    }

    #[tokio::test]
    async fn ordered_catch_up_reads_every_outbox_page() {
        let store = ObservedStore::new(false);
        initialize_control_store(&store, Some(&seed(8_000)), &actor())
            .await
            .unwrap();
        for expected in 1..=1_001 {
            replace_platform(&store, expected, 8_000 + (expected % 1_000) as u16).await;
        }
        let status = ControlRuntimeStatus::new(1);

        assert_eq!(
            refresh_once(&store, 1, &status, |_| async { Ok(()) })
                .await
                .unwrap(),
            RefreshOutcome::Applied(1_002)
        );
        assert!(store.event_reads.load(Ordering::SeqCst) >= 2);
    }

    #[cfg(feature = "control-postgres")]
    #[tokio::test]
    #[ignore = "requires TELLURION_TEST_CONTROL_DATABASE_URL"]
    async fn postgres_refresh_reads_the_revision_after_a_max_ordinal_cursor() {
        let Ok(database_url) = std::env::var("TELLURION_TEST_CONTROL_DATABASE_URL") else {
            eprintln!("skipping PostgreSQL consumer test: database URL is not configured");
            return;
        };
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let schema = format!(
            "tellurion_consumer_test_{}_{}",
            std::process::id(),
            timestamp
        );
        let store = tellurion_control_postgres::PostgresControlStore::connect_in_schema(
            &database_url,
            &schema,
        )
        .await
        .unwrap();
        initialize_control_store(&store, Some(&seed(8_000)), &actor())
            .await
            .unwrap();
        replace_platform(&store, 1, 8_001).await;
        let status = ControlRuntimeStatus::new(1);

        assert_eq!(
            refresh_once(&store, 1, &status, |_| async { Ok(()) })
                .await
                .unwrap(),
            RefreshOutcome::Applied(2)
        );

        drop(store);
        let (client, connection) = tokio_postgres::connect(&database_url, tokio_postgres::NoTls)
            .await
            .unwrap();
        tokio::spawn(async move {
            let _ = connection.await;
        });
        client
            .batch_execute(&format!("DROP SCHEMA \"{schema}\" CASCADE"))
            .await
            .unwrap();
    }

    #[test]
    fn polling_jitter_is_bounded_to_ten_percent() {
        let base = Duration::from_secs(1);
        for revision in 0..100 {
            let value = jittered(base, revision);
            assert!(value >= base);
            assert!(value <= Duration::from_millis(1_100));
        }
    }

    #[test]
    fn failed_refresh_backoff_is_bounded_to_thirty_three_seconds() {
        let base = Duration::from_secs(1);
        for failures in 1..100 {
            let value = retry_interval(base, failures, u64::from(failures));
            assert!(value <= Duration::from_secs(33));
        }
    }
}
