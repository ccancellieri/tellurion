//! Per-tenant admission control (`#66`): a small, bounded queue with a
//! deadline in front of a per-tenant fair-share concurrency gate, sized as
//! a slice of the global concurrency ceiling (`tellurion-server::app`'s own
//! `derive_max_concurrency`). Sits ahead of that ceiling and before routing
//! ever resolves a catalog or collection — nothing here touches a `Router`,
//! a `StorageDriver`, or any other driver-specific concept, so the same
//! mechanism protects every storage backend equally.
//!
//! **Two independent bounds compose, they don't replace each other.** This
//! module's own [`AdmissionRegistry`] only decides which tenant gets to
//! *attempt* a request right now; the existing tower-level
//! `concurrency_limit`/`load_shed` pair still enforces the true, global
//! hard ceiling downstream regardless of how the tenants split it among
//! themselves. A tenant's fair share is therefore a fairness device, not a
//! second safety ceiling — the floor that guarantees every tenant at least
//! one slot (see [`fair_share`]) can let the sum of every tenant's shares
//! exceed the global ceiling when there are many small tenants, and that is
//! fine: the downstream tower layer still caps total concurrency, honestly,
//! at the real number.
//!
//! **Queueing, not just rejecting.** A request that finds its tenant's
//! share fully in use waits — up to [`AdmissionConfig::queue_deadline`] —
//! for a slot to free, rather than failing immediately. The wait itself is
//! bounded twice over: by the deadline, and by [`AdmissionConfig::
//! queue_capacity`], the maximum number of requests allowed to wait at
//! once. A request arriving once the queue is already at capacity is
//! rejected on the spot, with no wait at all — the "bare 503 storm" the
//! issue calls a last resort, used only once even brief queueing can't help.
//!
//! **Config rides the existing settings chain**, one grouped key
//! (`SettingsDecl::admission`, whole-value-replaces like `colormap`/`stac`)
//! rather than several scalar ones — see [`AdmissionDecl`]'s own doc for
//! why only the platform and tenant levels are ever consulted.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::config::{SettingsDecl, TenantDecl};

/// Fallback per-tenant queue depth ahead of the fair-share gate when no
/// settings level declares one — small on purpose: this is meant to absorb
/// a brief burst, not become a second, unbounded backlog.
pub const DEFAULT_ADMISSION_QUEUE_CAPACITY: u32 = 32;

/// Fallback deadline a queued request waits for a fair-share slot before
/// being rejected, in milliseconds, when no settings level declares one.
pub const DEFAULT_ADMISSION_QUEUE_DEADLINE_MS: u64 = 250;

/// Fallback fair-share weight (equal shares across every tenant) when no
/// settings level declares one.
pub const DEFAULT_ADMISSION_WEIGHT: u32 = 1;

/// Declared, whitelisted per-tenant admission override (`#66`) — one
/// grouped settings key riding the platform -> tenant chain the same way
/// every other `SettingsDecl` field does (`settings.rs`'s module doc): a
/// tenant (or the platform default) that sets ANY field here replaces the
/// WHOLE value outright, the same "whole value replaces, never merged
/// field-by-field across levels" convention `StacConf`/`ColormapConf`
/// follow — a field the winning declaration itself leaves unset falls back
/// to this module's own default, never to a different level's value for
/// that one field. See [`AdmissionConfig`] for the materialized result.
///
/// Only ever consulted at the platform and tenant levels — admission runs
/// before routing resolves a catalog or collection (this module's own
/// doc), so a catalog- or collection-level override is legal to declare
/// (it rides the same whitelisted `SettingsDecl` every level shares) but
/// has no observable effect on enforcement; `AppConfig::validate` refuses
/// one outright rather than silently accepting a value nothing ever reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdmissionDecl {
    /// Bounded per-tenant queue depth ahead of the fair-share concurrency
    /// gate. `None`/absent falls back to [`DEFAULT_ADMISSION_QUEUE_CAPACITY`].
    pub queue_capacity: Option<u32>,
    /// How long a queued request waits for a fair-share slot before being
    /// rejected with `Retry-After` rather than served. `None`/absent falls
    /// back to [`DEFAULT_ADMISSION_QUEUE_DEADLINE_MS`].
    pub queue_deadline_ms: Option<u64>,
    /// Relative weight for this tenant's fair share of the global
    /// concurrency ceiling — equal shares (`weight: 1`) by default; a
    /// higher weight gets a proportionally larger, still bounded, slice.
    /// `None`/absent falls back to [`DEFAULT_ADMISSION_WEIGHT`]. Rejected at
    /// `AppConfig::validate` time if declared as `0` — a weightless tenant
    /// is a config mistake, not a meaningful "opt out."
    pub weight: Option<u32>,
}

impl AdmissionDecl {
    /// Applies this module's own defaults to whichever fields the winning
    /// declaration left unset — see this type's own doc for why an unset
    /// field never falls through to a different settings level instead.
    fn resolve(&self) -> AdmissionConfig {
        AdmissionConfig {
            queue_capacity: self
                .queue_capacity
                .unwrap_or(DEFAULT_ADMISSION_QUEUE_CAPACITY),
            queue_deadline: Duration::from_millis(
                self.queue_deadline_ms
                    .unwrap_or(DEFAULT_ADMISSION_QUEUE_DEADLINE_MS),
            ),
            weight: self.weight.unwrap_or(DEFAULT_ADMISSION_WEIGHT).max(1),
        }
    }
}

/// The materialized result of resolving [`AdmissionDecl`] for one tenant:
/// concrete values, ready to size a [`TenantGate`] without re-checking
/// `Option`s again.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdmissionConfig {
    pub queue_capacity: u32,
    pub queue_deadline: Duration,
    pub weight: u32,
}

impl Default for AdmissionConfig {
    fn default() -> Self {
        AdmissionDecl::default().resolve()
    }
}

/// Nearest-level-wins over exactly two levels (tenant, then platform) — the
/// same rule the four-level settings chain applies, just shallower, since
/// admission is only ever consulted at these two levels (see
/// [`AdmissionDecl`]'s own doc).
fn resolve_admission(tenant: &SettingsDecl, platform: &SettingsDecl) -> AdmissionConfig {
    tenant
        .admission
        .as_ref()
        .or(platform.admission.as_ref())
        .map(AdmissionDecl::resolve)
        .unwrap_or_default()
}

/// Bounds metric label cardinality to the operator's own allowlist,
/// exactly the convention `tellurion-server::metrics::tenant_metric_label`
/// applies to `http_request_duration_seconds` — duplicated here rather
/// than shared, since `tellurion-core` cannot depend back on the server
/// crate; a raw tenant identifier (allowlisted or not) never otherwise
/// reaches a metric label, so a deployment with many tenants never derives
/// unbounded series, and one that never configures an allowlist at all
/// gets every tenant's admission activity folded into one shared `"other"`
/// series rather than leaking tenant identity into `/metrics` by default.
fn tenant_metric_label(allowlist: &[String], external_id: &str) -> String {
    if allowlist.iter().any(|entry| entry == external_id) {
        external_id.to_string()
    } else {
        "other".to_string()
    }
}

/// A tenant's fair share of `global_ceiling`, proportional to its own
/// `weight` out of `total_weight` — floor division, then clamped to
/// `[1, global_ceiling]`: never zero (a tenant with a nonzero weight is
/// never locked out entirely, even split thin across many tenants — see
/// this module's own doc for why the sum of every tenant's floor can still
/// exceed `global_ceiling`, and why that's fine), and never more than the
/// whole ceiling itself (a single tenant, or one with an outsized weight,
/// can't derive a share larger than the ceiling it's a fraction of).
fn fair_share(global_ceiling: usize, weight: u32, total_weight: u64) -> usize {
    let ceiling = global_ceiling.max(1) as u64;
    let weight = u64::from(weight.max(1));
    let total = total_weight.max(1);
    let share = (ceiling * weight) / total;
    share.clamp(1, ceiling) as usize
}

/// Why an [`AdmissionRegistry::admit`] call was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionRejection {
    /// The tenant's bounded wait queue was already at
    /// [`AdmissionConfig::queue_capacity`] — refused immediately, no wait.
    QueueFull,
    /// The request waited for a fair-share slot but
    /// [`AdmissionConfig::queue_deadline`] elapsed first.
    DeadlineExpired,
}

/// The result of an [`AdmissionRegistry::admit`] call.
pub enum AdmissionOutcome {
    /// Holds the tenant's fair-share slot until dropped — the caller must
    /// keep this alive for exactly as long as the admitted request is in
    /// flight, then let it drop to release the slot.
    Admitted(AdmissionPermit),
    Rejected(AdmissionRejection),
}

/// RAII guard for one admitted request's fair-share slot. `None` when
/// `admit` was called for a tenant id the registry has no gate for (see
/// [`AdmissionRegistry::admit`]'s own doc) — nothing to release in that
/// case, but still a real value so the caller never has to special-case it.
pub struct AdmissionPermit(Option<Arc<TenantGate>>);

impl Drop for AdmissionPermit {
    fn drop(&mut self) {
        if let Some(gate) = self.0.take() {
            gate.active.fetch_sub(1, Ordering::SeqCst);
            gate.slot_available.notify_one();
        }
    }
}

/// One tenant's runtime admission gate: a weighted slice of the global
/// concurrency ceiling (`permits`) plus a bounded, deadline-limited wait
/// queue in front of it. Built once per tenant when [`AdmissionRegistry`]
/// is constructed and held across registry reconfiguration — every request
/// the tenant ever sends contends for the same active-count.
struct TenantGate {
    tenant_label: RwLock<String>,
    limit: AtomicUsize,
    active: AtomicUsize,
    queue_capacity: AtomicUsize,
    queue_deadline_ms: AtomicU64,
    queue_depth: AtomicUsize,
    slot_available: Notify,
}

impl TenantGate {
    fn new(tenant_label: String, limit: usize, config: AdmissionConfig) -> Self {
        Self {
            tenant_label: RwLock::new(tenant_label),
            limit: AtomicUsize::new(limit),
            active: AtomicUsize::new(0),
            queue_capacity: AtomicUsize::new(config.queue_capacity as usize),
            queue_deadline_ms: AtomicU64::new(config.queue_deadline.as_millis() as u64),
            queue_depth: AtomicUsize::new(0),
            slot_available: Notify::new(),
        }
    }

    fn reconfigure(&self, tenant_label: String, limit: usize, config: AdmissionConfig) {
        *self
            .tenant_label
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = tenant_label;
        self.limit.store(limit, Ordering::SeqCst);
        self.queue_capacity
            .store(config.queue_capacity as usize, Ordering::SeqCst);
        self.queue_deadline_ms
            .store(config.queue_deadline.as_millis() as u64, Ordering::SeqCst);
        self.slot_available.notify_waiters();
    }

    fn try_acquire(self: &Arc<Self>) -> Option<AdmissionPermit> {
        loop {
            let active = self.active.load(Ordering::SeqCst);
            if active >= self.limit.load(Ordering::SeqCst) {
                return None;
            }
            if self
                .active
                .compare_exchange(active, active + 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return Some(AdmissionPermit(Some(Arc::clone(self))));
            }
        }
    }

    async fn admit(self: &Arc<Self>) -> AdmissionOutcome {
        // Fast path: a free slot right now costs nothing beyond what the
        // existing tower `concurrency_limit` already costs.
        if let Some(permit) = self.try_acquire() {
            self.record_admitted();
            return AdmissionOutcome::Admitted(permit);
        }

        // Slow path: every fair-share slot is in use. Bound the queue
        // itself before ever waiting — a queue already at capacity refuses
        // immediately, no wait, per this module's own "brief queueing,
        // never unbounded" rule.
        let queued = self.queue_depth.fetch_add(1, Ordering::SeqCst) + 1;
        if queued > self.queue_capacity.load(Ordering::SeqCst) {
            self.queue_depth.fetch_sub(1, Ordering::SeqCst);
            self.record_rejected();
            return AdmissionOutcome::Rejected(AdmissionRejection::QueueFull);
        }
        metrics::gauge!("tenant_admission_queue_depth", "tenant" => self.tenant_label())
            .set(queued as f64);

        let deadline = Duration::from_millis(self.queue_deadline_ms.load(Ordering::SeqCst));
        let waited = tokio::time::timeout(deadline, async {
            loop {
                let notified = self.slot_available.notified();
                if let Some(permit) = self.try_acquire() {
                    break permit;
                }
                notified.await;
            }
        })
        .await;
        let remaining = self.queue_depth.fetch_sub(1, Ordering::SeqCst) - 1;
        metrics::gauge!("tenant_admission_queue_depth", "tenant" => self.tenant_label())
            .set(remaining as f64);

        match waited {
            Ok(permit) => {
                self.record_admitted();
                AdmissionOutcome::Admitted(permit)
            }
            Err(_elapsed) => {
                self.record_deadline_expired();
                AdmissionOutcome::Rejected(AdmissionRejection::DeadlineExpired)
            }
        }
    }

    fn tenant_label(&self) -> String {
        self.tenant_label
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn record_admitted(&self) {
        metrics::counter!("tenant_admission_admitted_total", "tenant" => self.tenant_label())
            .increment(1);
    }

    fn record_rejected(&self) {
        metrics::counter!("tenant_admission_rejected_total", "tenant" => self.tenant_label())
            .increment(1);
    }

    fn record_deadline_expired(&self) {
        metrics::counter!("tenant_admission_deadline_expired_total", "tenant" => self.tenant_label())
            .increment(1);
    }
}

/// Every tenant's admission gate, built once (`build`) from the same
/// config snapshot `Router::build_from_snapshot` reads its tenants from,
/// and the global concurrency ceiling `tellurion-server::app` derives —
/// see this module's own doc for why these gates are a fairness device
/// layered ahead of, not a replacement for, that ceiling.
pub struct AdmissionRegistry {
    gates: RwLock<HashMap<String, Arc<TenantGate>>>,
}

impl AdmissionRegistry {
    /// `tenants` is `AppConfig.tenants` (or the equivalent snapshot) —
    /// tenants are always file-declared, never part of the relational
    /// registry snapshot (see `router.rs`'s own "tenants always from file"
    /// doc), so this never needs a second source. `global_ceiling` is the
    /// same concurrency ceiling `tellurion-server::app::derive_max_
    /// concurrency` (or an explicit `server.max_concurrency`) computes —
    /// every tenant's fair share is a fraction of this one number.
    ///
    /// `metrics_tenant_allowlist` is `AppConfig.server.metrics_tenant_
    /// allowlist` — the same allowlist `tellurion-server::metrics` bounds
    /// `http_request_duration_seconds`'s own `tenant` label to; a tenant
    /// not on it gets every one of its admission metrics folded into a
    /// shared `"other"` series instead of exposing its identifier.
    pub fn build(
        tenants: &[TenantDecl],
        platform_settings: &SettingsDecl,
        global_ceiling: usize,
        metrics_tenant_allowlist: &[String],
    ) -> Self {
        let resolved: Vec<(&TenantDecl, AdmissionConfig)> = tenants
            .iter()
            .map(|tenant| {
                (
                    tenant,
                    resolve_admission(&tenant.settings, platform_settings),
                )
            })
            .collect();
        let total_weight: u64 = resolved
            .iter()
            .map(|(_, cfg)| u64::from(cfg.weight))
            .sum::<u64>()
            .max(1);

        let mut gates = HashMap::with_capacity(tenants.len());
        for (tenant, cfg) in resolved {
            let share = fair_share(global_ceiling, cfg.weight, total_weight);
            gates.insert(
                tenant.id.clone(),
                Arc::new(TenantGate::new(
                    tenant_metric_label(metrics_tenant_allowlist, tenant.external_id()),
                    share,
                    cfg,
                )),
            );
        }
        Self {
            gates: RwLock::new(gates),
        }
    }

    /// Reconciles tenant gates in place. A tenant that survives a reload by
    /// internal id keeps the same gate and active-count, so an in-flight
    /// permit remains charged against its reloaded fair share. Lowering a
    /// share never cancels admitted work; it simply prevents another admit
    /// until the active count falls below the new limit.
    pub fn reconfigure(
        &self,
        tenants: &[TenantDecl],
        platform_settings: &SettingsDecl,
        global_ceiling: usize,
        metrics_tenant_allowlist: &[String],
    ) {
        let resolved: Vec<(&TenantDecl, AdmissionConfig)> = tenants
            .iter()
            .map(|tenant| {
                (
                    tenant,
                    resolve_admission(&tenant.settings, platform_settings),
                )
            })
            .collect();
        let total_weight = resolved
            .iter()
            .map(|(_, config)| u64::from(config.weight))
            .sum::<u64>()
            .max(1);
        let tenant_ids: std::collections::HashSet<&str> =
            tenants.iter().map(|tenant| tenant.id.as_str()).collect();
        let mut gates = self
            .gates
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        gates.retain(|tenant_id, _| tenant_ids.contains(tenant_id.as_str()));
        for (tenant, config) in resolved {
            let share = fair_share(global_ceiling, config.weight, total_weight);
            let label = tenant_metric_label(metrics_tenant_allowlist, tenant.external_id());
            match gates.get(&tenant.id) {
                Some(gate) => gate.reconfigure(label, share, config),
                None => {
                    gates.insert(
                        tenant.id.clone(),
                        Arc::new(TenantGate::new(label, share, config)),
                    );
                }
            }
        }
    }

    /// Admits or rejects one request for `tenant_internal_id`. A tenant id
    /// this registry has no gate for — an unresolvable external id, or any
    /// other case where the caller never got as far as resolving one —
    /// admits unconditionally: the same "nothing to admit for a tenant
    /// that doesn't exist" precedent `tellurion-server::app::
    /// enforce_tenant_auth` documents for authorization; the eventual
    /// handler still answers 404, and admission enforcement never changes
    /// the shape of that response.
    pub async fn admit(&self, tenant_internal_id: &str) -> AdmissionOutcome {
        let gate = self
            .gates
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(tenant_internal_id)
            .cloned();
        match gate {
            Some(gate) => gate.admit().await,
            None => AdmissionOutcome::Admitted(AdmissionPermit(None)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration as StdDuration;

    fn decl(
        queue_capacity: Option<u32>,
        queue_deadline_ms: Option<u64>,
        weight: Option<u32>,
    ) -> SettingsDecl {
        SettingsDecl {
            admission: Some(AdmissionDecl {
                queue_capacity,
                queue_deadline_ms,
                weight,
            }),
            ..Default::default()
        }
    }

    fn empty() -> SettingsDecl {
        SettingsDecl::default()
    }

    // -- resolve_admission (settings chain) ----------------------------

    #[test]
    fn tenant_declaration_wins_outright_over_the_platform_default() {
        let tenant = decl(Some(4), Some(100), Some(3));
        let platform = decl(Some(64), Some(9_999), Some(1));
        let resolved = resolve_admission(&tenant, &platform);
        assert_eq!(resolved.queue_capacity, 4);
        assert_eq!(resolved.queue_deadline, StdDuration::from_millis(100));
        assert_eq!(resolved.weight, 3);
    }

    #[test]
    fn platform_default_shows_through_when_the_tenant_declares_nothing() {
        let platform = decl(Some(8), Some(500), Some(2));
        let resolved = resolve_admission(&empty(), &platform);
        assert_eq!(resolved.queue_capacity, 8);
        assert_eq!(resolved.queue_deadline, StdDuration::from_millis(500));
        assert_eq!(resolved.weight, 2);
    }

    #[test]
    fn module_default_applies_when_neither_level_declares_anything() {
        let resolved = resolve_admission(&empty(), &empty());
        assert_eq!(resolved, AdmissionConfig::default());
        assert_eq!(resolved.queue_capacity, DEFAULT_ADMISSION_QUEUE_CAPACITY);
        assert_eq!(resolved.weight, DEFAULT_ADMISSION_WEIGHT);
    }

    /// Whole-value-replace: a tenant declaring only `weight` still gets the
    /// MODULE's own defaults for the other two fields, never the
    /// platform's values for those fields — the same convention
    /// `StacConf`/`ColormapConf` document for their own settings-chain key.
    #[test]
    fn a_partial_tenant_declaration_falls_back_to_module_defaults_not_the_platforms_values() {
        let tenant = decl(None, None, Some(5));
        let platform = decl(Some(99), Some(99_999), Some(1));
        let resolved = resolve_admission(&tenant, &platform);
        assert_eq!(resolved.weight, 5);
        assert_eq!(resolved.queue_capacity, DEFAULT_ADMISSION_QUEUE_CAPACITY);
        assert_eq!(
            resolved.queue_deadline,
            StdDuration::from_millis(DEFAULT_ADMISSION_QUEUE_DEADLINE_MS)
        );
    }

    // -- fair_share ------------------------------------------------------

    #[test]
    fn equal_weights_split_the_ceiling_evenly() {
        assert_eq!(fair_share(10, 1, 2), 5);
        assert_eq!(fair_share(9, 1, 3), 3);
    }

    #[test]
    fn a_heavier_weight_gets_a_proportionally_larger_share() {
        // weight 9 out of a total of 10, ceiling 20 -> 18; weight 1 -> 2.
        assert_eq!(fair_share(20, 9, 10), 18);
        assert_eq!(fair_share(20, 1, 10), 2);
    }

    #[test]
    fn a_share_never_exceeds_the_global_ceiling() {
        assert_eq!(fair_share(4, 1_000, 1_000), 4);
    }

    #[test]
    fn a_share_is_never_zero_even_starved_thin_across_many_tenants() {
        assert_eq!(fair_share(4, 1, 100), 1);
    }

    // -- AdmissionRegistry / TenantGate -----------------------------------

    fn tenant(id: &str, external_id: &str, admission: Option<AdmissionDecl>) -> TenantDecl {
        TenantDecl {
            id: id.to_string(),
            external_id: Some(external_id.to_string()),
            settings: SettingsDecl {
                admission,
                ..Default::default()
            },
        }
    }

    #[tokio::test]
    async fn a_free_slot_admits_immediately_and_records_the_metric() {
        let tenants = vec![tenant("solo", "solo", None)];
        let registry = AdmissionRegistry::build(&tenants, &SettingsDecl::default(), 4, &[]);
        match registry.admit("solo").await {
            AdmissionOutcome::Admitted(_permit) => {}
            AdmissionOutcome::Rejected(_) => panic!("expected an immediate admission"),
        }
    }

    #[tokio::test]
    async fn an_unresolved_tenant_id_admits_unconditionally() {
        let registry = AdmissionRegistry::build(&[], &SettingsDecl::default(), 4, &[]);
        match registry.admit("nonexistent").await {
            AdmissionOutcome::Admitted(_permit) => {}
            AdmissionOutcome::Rejected(_) => panic!("a tenant with no gate must always admit"),
        }
    }

    #[tokio::test]
    async fn a_queue_already_at_capacity_rejects_without_ever_waiting() {
        let tenants = vec![tenant(
            "t",
            "t",
            Some(AdmissionDecl {
                queue_capacity: Some(0),
                queue_deadline_ms: Some(60_000),
                weight: Some(1),
            }),
        )];
        // ceiling 1, single tenant -> exactly one permit; the first admit
        // takes it, the second has nowhere to queue (`queue_capacity: 0`).
        let registry = AdmissionRegistry::build(&tenants, &SettingsDecl::default(), 1, &[]);
        let AdmissionOutcome::Admitted(_first) = registry.admit("t").await else {
            panic!("the first request should admit immediately");
        };

        let outcome = tokio::time::timeout(StdDuration::from_millis(50), registry.admit("t"))
            .await
            .expect("a queue-full rejection must never wait at all");
        assert!(matches!(
            outcome,
            AdmissionOutcome::Rejected(AdmissionRejection::QueueFull)
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn a_queued_request_past_its_deadline_expires_rather_than_waiting_forever() {
        let tenants = vec![tenant(
            "t",
            "t",
            Some(AdmissionDecl {
                queue_capacity: Some(1),
                queue_deadline_ms: Some(50),
                weight: Some(1),
            }),
        )];
        let registry = AdmissionRegistry::build(&tenants, &SettingsDecl::default(), 1, &[]);
        let AdmissionOutcome::Admitted(_holder) = registry.admit("t").await else {
            panic!("the first request should admit immediately");
        };

        // The one held slot never frees during this test, so the second
        // request queues, then the paused clock fast-forwards straight to
        // its deadline.
        let outcome = registry.admit("t").await;
        assert!(matches!(
            outcome,
            AdmissionOutcome::Rejected(AdmissionRejection::DeadlineExpired)
        ));
    }

    /// The whole point of a per-tenant gate: tenant A exhausting its own
    /// share must never delay tenant B's admission, even though both
    /// gates were built from the very same global ceiling.
    #[tokio::test]
    async fn one_tenants_exhausted_share_never_blocks_a_different_tenants_admission() {
        let tenants = vec![
            tenant(
                "a",
                "a",
                Some(AdmissionDecl {
                    queue_capacity: Some(0),
                    queue_deadline_ms: None,
                    weight: Some(1),
                }),
            ),
            tenant("b", "b", None),
        ];
        let registry = AdmissionRegistry::build(&tenants, &SettingsDecl::default(), 2, &[]);

        let AdmissionOutcome::Admitted(_a_holder) = registry.admit("a").await else {
            panic!("tenant a's first request should admit immediately");
        };
        // Tenant a's own single-slot share is now exhausted and its queue
        // capacity is zero, so a second request from a rejects immediately
        // ...
        assert!(matches!(
            registry.admit("a").await,
            AdmissionOutcome::Rejected(AdmissionRejection::QueueFull)
        ));
        // ... while tenant b, sharing nothing with a's gate, still admits.
        assert!(matches!(
            registry.admit("b").await,
            AdmissionOutcome::Admitted(_)
        ));
    }

    #[tokio::test]
    async fn reconfigure_preserves_an_in_flight_tenants_fair_share_capacity() {
        let tenants = vec![tenant(
            "t",
            "before-reload",
            Some(AdmissionDecl {
                queue_capacity: Some(0),
                queue_deadline_ms: Some(60_000),
                weight: Some(1),
            }),
        )];
        let registry = AdmissionRegistry::build(&tenants, &SettingsDecl::default(), 1, &[]);
        let AdmissionOutcome::Admitted(held_before_reload) = registry.admit("t").await else {
            panic!("the pre-reload request should consume the tenant's only slot");
        };

        let reloaded = vec![tenant(
            "t",
            "after-reload",
            Some(AdmissionDecl {
                queue_capacity: Some(0),
                queue_deadline_ms: Some(60_000),
                weight: Some(1),
            }),
        )];
        registry.reconfigure(&reloaded, &SettingsDecl::default(), 1, &[]);

        assert!(matches!(
            registry.admit("t").await,
            AdmissionOutcome::Rejected(AdmissionRejection::QueueFull)
        ));
        drop(held_before_reload);
        assert!(matches!(
            registry.admit("t").await,
            AdmissionOutcome::Admitted(_)
        ));
    }
}
