//! Bounded audit trail for authenticated configuration mutations (`#110`):
//! one record per applied change — principal, timestamp, the expected and
//! resulting version, and a short human-readable summary of what changed
//! (`tellurion-server::config_mutation::summarize_change`). Held in memory
//! only, capped at a fixed capacity ([`ConfigAuditLog::DEFAULT_CAPACITY`])
//! so a long-running server's mutation history can never grow unbounded —
//! the oldest record is evicted once a new one would exceed capacity, the
//! same "keep the recent tail, not the whole history" bound this project's
//! own byte-budgeted tile cache and `auth::JwksCache` already apply to
//! their own unbounded-growth risks elsewhere.
//!
//! Deliberately not persisted: a process restart starts a fresh, empty
//! trail. This is an operational observability aid ("who changed what,
//! recently"), not a system of record — the config document's own version
//! history (whatever the backend keeps, if anything) is that.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// One applied configuration change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
    /// A human-identifiable actor — never the raw credential value (see
    /// `tellurion_core::auth`'s "never logs or echoes" rule). See
    /// `auth::PlatformAdminDecision::Allow`'s own doc for how this is
    /// derived.
    pub principal: String,
    /// Milliseconds since the Unix epoch when this change was applied.
    pub applied_unix_ms: u128,
    pub expected_version: String,
    pub new_version: String,
    /// A short, human-readable summary of what changed — never the whole
    /// before/after document (unbounded, and largely redundant with the
    /// version tokens themselves).
    pub summary: String,
    /// `#215`: the effective scope this mutation was authorised at, as a
    /// `ControlScope::resource_key` — `platform` for the whole-document
    /// mutation lane, which is the only administrative mutation this server
    /// serves today.
    pub effective_scope: String,
    /// `#215`: why the request was allowed, in the policy engine's own
    /// vocabulary (`ControlDecisionContext::summary`) — statement ids,
    /// contributing roles and the basis clause. Never a credential, never a
    /// claim value; see `control_policy`'s own doc for what a decision
    /// context is allowed to carry.
    ///
    /// `not_engaged` for a deployment whose active snapshot declares no
    /// statement mentioning this path, which is every deployment written
    /// before `#215`: the platform-admin gate alone authorised the write,
    /// and the record says exactly that rather than naming a policy that
    /// does not exist.
    pub decision: String,
}

/// A fixed-capacity, most-recent-wins ring buffer of [`AuditRecord`]s.
pub struct ConfigAuditLog {
    capacity: usize,
    records: Mutex<VecDeque<AuditRecord>>,
}

impl ConfigAuditLog {
    /// Default retention: recent changes only, not full history — see this
    /// module's own bounded-growth doc.
    pub const DEFAULT_CAPACITY: usize = 200;

    /// `capacity` is clamped up to at least 1 — a log that could hold zero
    /// records would silently discard every entry, which is a different
    /// (and strictly worse) thing than "bounded."
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            records: Mutex::new(VecDeque::new()),
        }
    }

    /// Appends one record, evicting the oldest entry first if the log is
    /// already at capacity.
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &self,
        principal: impl Into<String>,
        expected_version: impl Into<String>,
        new_version: impl Into<String>,
        summary: impl Into<String>,
        effective_scope: impl Into<String>,
        decision: impl Into<String>,
    ) {
        let record = AuditRecord {
            principal: principal.into(),
            applied_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
            expected_version: expected_version.into(),
            new_version: new_version.into(),
            summary: summary.into(),
            effective_scope: effective_scope.into(),
            decision: decision.into(),
        };
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if records.len() >= self.capacity {
            records.pop_front();
        }
        records.push_back(record);
    }

    /// Every retained record, most recent first.
    pub fn recent(&self) -> Vec<AuditRecord> {
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .rev()
            .cloned()
            .collect()
    }

    /// How many records are currently retained — never more than the
    /// configured capacity.
    pub fn len(&self) -> usize {
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for ConfigAuditLog {
    fn default() -> Self {
        Self::new(Self::DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_are_returned_most_recent_first() {
        let log = ConfigAuditLog::new(10);
        log.record(
            "alice",
            "v1",
            "v2",
            "changed settings",
            "platform",
            "not_engaged",
        );
        log.record(
            "bob",
            "v2",
            "v3",
            "changed tenants",
            "platform",
            "not_engaged",
        );

        let recent = log.recent();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].principal, "bob");
        assert_eq!(recent[1].principal, "alice");
    }

    /// The whole point of this type (`#110`): a long-running server that
    /// applies far more changes than the configured capacity never grows
    /// its retained history past that bound.
    #[test]
    fn the_log_never_grows_past_its_configured_capacity() {
        let log = ConfigAuditLog::new(3);
        for i in 0..10 {
            log.record(
                format!("principal-{i}"),
                "v",
                "v",
                "change",
                "platform",
                "not_engaged",
            );
        }
        assert_eq!(log.len(), 3);
        let recent = log.recent();
        // Only the three most recent survive; the oldest seven were
        // evicted.
        assert_eq!(recent[0].principal, "principal-9");
        assert_eq!(recent[1].principal, "principal-8");
        assert_eq!(recent[2].principal, "principal-7");
    }

    #[test]
    fn a_zero_capacity_is_clamped_up_to_one() {
        let log = ConfigAuditLog::new(0);
        log.record("alice", "v1", "v2", "change", "platform", "not_engaged");
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn a_fresh_log_is_empty() {
        let log = ConfigAuditLog::default();
        assert!(log.is_empty());
        assert!(log.recent().is_empty());
    }

    #[test]
    fn a_record_carries_the_shape_the_mutation_endpoint_needs() {
        let log = ConfigAuditLog::default();
        log.record(
            "token:abc123",
            "old-version",
            "new-version",
            "settings, tenants",
            "platform",
            "scope=platform basis=explicit_allow statements=[platform-write] roles=[sysadmin]",
        );

        let record = &log.recent()[0];
        assert_eq!(record.principal, "token:abc123");
        assert_eq!(record.expected_version, "old-version");
        assert_eq!(record.new_version, "new-version");
        assert_eq!(record.summary, "settings, tenants");
        assert_eq!(record.effective_scope, "platform");
        assert!(record.decision.contains("explicit_allow"));
        assert!(record.applied_unix_ms > 0);
    }
}
