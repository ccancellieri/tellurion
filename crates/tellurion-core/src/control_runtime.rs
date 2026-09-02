use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::control_model::ControlRevision;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ControlRuntimeSnapshot {
    pub store_revision: ControlRevision,
    pub applied_revision: ControlRevision,
    pub lag: ControlRevision,
    pub last_successful_refresh_unix_ms: Option<u64>,
    pub poll_failures: u64,
    pub activation_failures: u64,
}

pub struct ControlRuntimeStatus {
    store_revision: AtomicU64,
    applied_revision: AtomicU64,
    last_successful_refresh_unix_ms: AtomicU64,
    poll_failures: AtomicU64,
    activation_failures: AtomicU64,
}

impl ControlRuntimeStatus {
    pub fn new(initial_revision: ControlRevision) -> Self {
        Self {
            store_revision: AtomicU64::new(initial_revision),
            applied_revision: AtomicU64::new(initial_revision),
            last_successful_refresh_unix_ms: AtomicU64::new(0),
            poll_failures: AtomicU64::new(0),
            activation_failures: AtomicU64::new(0),
        }
    }

    pub fn observe_store_revision(&self, revision: ControlRevision) {
        self.store_revision.store(revision, Ordering::Relaxed);
    }

    pub fn observe_applied_revision(&self, revision: ControlRevision) {
        self.applied_revision.store(revision, Ordering::Relaxed);
    }

    pub fn record_refresh_success(&self) {
        let unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        self.last_successful_refresh_unix_ms
            .store(unix_ms.max(1), Ordering::Relaxed);
    }

    pub fn record_poll_failure(&self) {
        self.poll_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_activation_failure(&self) {
        self.activation_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> ControlRuntimeSnapshot {
        let store_revision = self.store_revision.load(Ordering::Relaxed);
        let applied_revision = self.applied_revision.load(Ordering::Relaxed);
        let last_successful_refresh_unix_ms =
            self.last_successful_refresh_unix_ms.load(Ordering::Relaxed);

        ControlRuntimeSnapshot {
            store_revision,
            applied_revision,
            lag: store_revision.saturating_sub(applied_revision),
            last_successful_refresh_unix_ms: (last_successful_refresh_unix_ms != 0)
                .then_some(last_successful_refresh_unix_ms),
            poll_failures: self.poll_failures.load(Ordering::Relaxed),
            activation_failures: self.activation_failures.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ControlRuntimeStatus;

    #[test]
    fn initial_snapshot_uses_the_initial_revision_for_both_local_revisions() {
        let snapshot = ControlRuntimeStatus::new(4).snapshot();

        assert_eq!(snapshot.store_revision, 4);
        assert_eq!(snapshot.applied_revision, 4);
        assert_eq!(snapshot.lag, 0);
    }

    #[test]
    fn snapshot_reports_local_revision_lag_and_failures() {
        let status = ControlRuntimeStatus::new(4);
        status.observe_store_revision(7);
        status.record_poll_failure();
        status.record_activation_failure();

        let snapshot = status.snapshot();

        assert_eq!(snapshot.applied_revision, 4);
        assert_eq!(snapshot.store_revision, 7);
        assert_eq!(snapshot.lag, 3);
        assert_eq!(snapshot.poll_failures, 1);
        assert_eq!(snapshot.activation_failures, 1);
    }

    #[test]
    fn snapshot_lag_saturates_when_applied_revision_is_ahead_of_store_revision() {
        let status = ControlRuntimeStatus::new(9);
        status.observe_store_revision(4);
        status.observe_applied_revision(11);

        assert_eq!(status.snapshot().lag, 0);
    }

    #[test]
    fn refresh_success_makes_the_last_success_timestamp_present() {
        let status = ControlRuntimeStatus::new(0);
        assert_eq!(status.snapshot().last_successful_refresh_unix_ms, None);

        status.record_refresh_success();

        assert!(status.snapshot().last_successful_refresh_unix_ms.is_some());
    }

    #[test]
    fn failure_counters_increment_only_the_matching_failure_field() {
        let status = ControlRuntimeStatus::new(0);
        status.record_poll_failure();

        let after_poll_failure = status.snapshot();
        assert_eq!(after_poll_failure.poll_failures, 1);
        assert_eq!(after_poll_failure.activation_failures, 0);

        status.record_activation_failure();

        let after_activation_failure = status.snapshot();
        assert_eq!(after_activation_failure.poll_failures, 1);
        assert_eq!(after_activation_failure.activation_failures, 1);
    }
}
