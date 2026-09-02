//! Per-principal request-rate ceilings as policy grant conditions (`#188`).
//!
//! Admission control (`admission.rs`) fair-shares *concurrency* per tenant:
//! how many requests a tenant may have in flight at once. Nothing there
//! bounds one principal, role, or tenant over *time* — a single token can
//! stay inside its tenant's fair share forever and still issue an unbounded
//! number of requests. This module is the temporal half: a
//! [`RateLimitDecl`] rides on a policy grant
//! ([`GrantDecl::rate`](crate::config::GrantDecl::rate)), and
//! `policy::enforce_rate_limits` charges it once per authorized request.
//!
//! ## Nothing configured, nothing changed
//!
//! A grant with no `rate:` block declares no ceiling, and a tenant whose
//! role table declares no ceiling anywhere never reaches a counter at all —
//! `policy::enforce_rate_limits` returns [`RateVerdict::Permitted`] after a
//! table scan that touches no shared state. This is the same rule the whole
//! policy layer follows (`policy.rs`'s "RBAC inactive for this tenant"
//! doc): a deployment that never declares a ceiling behaves exactly as it
//! did before this module existed.
//!
//! ## Fixed windows, and what that costs
//!
//! The window is *fixed*, not sliding: a `window_seconds: 60` ceiling
//! resets on wall-clock minute boundaries derived from the Unix epoch
//! (`now - now % window_seconds`), so a client can spend its whole ceiling
//! in the last instant of one window and again in the first instant of the
//! next — up to twice the ceiling across a window-length span straddling
//! the boundary. That is the well-known fixed-window burst, and it is
//! accepted here deliberately: a fixed window needs one counter and one
//! integer per key, which a fleet-atomic backend can implement as a single
//! `INCR`/`EXPIRE` pair, while a sliding window needs a timestamp log or a
//! second counter per key. `Retry-After` is likewise exact for a fixed
//! window (the time to the next boundary) and only ever an estimate for a
//! sliding one.
//!
//! ## Two backends' worth of seam, one backend's worth of code
//!
//! [`RateCounter`] is the counting seam, and it is `async` even though the
//! only implementation this slice ships ([`InProcessRateCounter`]) never
//! awaits anything: the issue names a fleet-atomic Valkey counter as the
//! next backend, and a synchronous seam would have to break to admit one.
//! Per the external-components rule, neither backend is ever a boot
//! requirement — the in-process counter needs no configuration and no
//! external process, and a deployment with no counter wired at all is
//! treated exactly like one whose counter is unreachable (see below).
//!
//! ## The failure posture is declared, never inferred
//!
//! Every [`RateLimitDecl`] names what happens when its counter cannot
//! answer — [`CounterPosture::Strict`] (refuse the request) or
//! [`CounterPosture::Graceful`] (serve it, and log). There is no default:
//! the field is required in the document, because "what should happen when
//! the bound cannot be enforced" is exactly the kind of availability-versus-
//! safety judgment only an operator can make. Three distinct situations all
//! resolve through that one declaration, and all three are named:
//!
//! 1. No counter backend is wired into this deployment at all.
//! 2. The wired backend answered with [`CounterUnavailable`] — including
//!    [`InProcessRateCounter`] refusing to grow past its own bounded key
//!    capacity.
//! 3. The condition's [`RateScope`] cannot be keyed for this subject — a
//!    `principal`-scoped ceiling against a subject carrying no principal
//!    identity (see [`Subject::principal`](crate::auth::Subject::principal)).
//!
//! A silent fallback in any of the three would turn a declared ceiling into
//! a decoration, which is precisely what naming the posture avoids.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// How many distinct counter keys [`InProcessRateCounter`] tracks before it
/// starts refusing to grow — see that type's own doc for why the bound
/// exists and what crossing it means.
pub const DEFAULT_RATE_COUNTER_KEY_CAPACITY: usize = 100_000;

/// Which counter a grant's ceiling is charged against (`#188`). Deliberately
/// a closed vocabulary, and deliberately without a default: a ceiling that
/// doesn't say what it bounds is not a ceiling anyone can reason about.
///
/// Every key is additionally scoped to the *resource's own tenant*,
/// including [`Principal`](Self::Principal) — see [`CounterKey`]'s own doc
/// for why a per-principal ceiling is still per-tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RateScope {
    /// One counter per calling principal: the identity
    /// [`Subject::principal`](crate::auth::Subject::principal) carries. The
    /// issue's headline case — bounding a single token or user.
    Principal,
    /// One counter per role name held in the resource's tenant: every
    /// subject holding that role shares the ceiling. A per-role ceiling on
    /// an expensive role ("bulk-export") bounds the role's aggregate cost
    /// regardless of how many tokens hold it.
    Role,
    /// One counter for the resource's whole tenant: every subject, every
    /// role, one ceiling. The temporal counterpart of `admission.rs`'s
    /// per-tenant concurrency fair share.
    Tenant,
}

impl RateScope {
    /// The stable, low-cardinality spelling used as a metric label — see
    /// this module's own metrics discipline note on
    /// `policy::enforce_rate_limits`.
    pub fn as_str(self) -> &'static str {
        match self {
            RateScope::Principal => "principal",
            RateScope::Role => "role",
            RateScope::Tenant => "tenant",
        }
    }
}

/// The operator-declared failure posture for one [`RateLimitDecl`] (`#188`)
/// — what to do when the ceiling cannot be evaluated at all. Required, never
/// defaulted: see this module's own doc for the three distinct situations
/// this single declaration governs, and why none of them may silently pick
/// a side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CounterPosture {
    /// Refuse the request. The bound is a safety property this deployment
    /// will not serve without.
    Strict,
    /// Serve the request, and record that the ceiling went unenforced (a
    /// `WARN` log line and a named metric outcome). Availability wins; the
    /// unenforced window stays visible rather than invisible.
    Graceful,
}

impl CounterPosture {
    fn as_str(self) -> &'static str {
        match self {
            CounterPosture::Strict => "strict",
            CounterPosture::Graceful => "graceful",
        }
    }
}

/// One grant's fixed-window rate condition (`#188`) — the `rate:` block on
/// [`GrantDecl`](crate::config::GrantDecl).
///
/// Every field is required. There is no `#[serde(default)]` anywhere in
/// this struct on purpose: a partially-declared ceiling would have to
/// invent the missing half (how long a window? how many requests? refuse or
/// serve when the counter is down?), and every one of those inventions is a
/// policy decision this crate has no standing to make. A grant either
/// declares a complete ceiling or declares none at all.
///
/// ```yaml
/// policy:
///   roles:
///     - name: reader
///       grants:
///         - lanes: [features, stac]
///           rate:
///             scope: principal
///             window_seconds: 60
///             ceiling: 600
///             on_counter_unavailable: graceful
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateLimitDecl {
    pub scope: RateScope,
    /// Fixed-window length in seconds. Must be at least 1 and at most
    /// [`MAX_RATE_WINDOW_SECONDS`] — see [`validate`](Self::validate).
    pub window_seconds: u64,
    /// How many requests this window admits before the ceiling refuses.
    /// Must be at least 1: a `ceiling: 0` grant would refuse every request
    /// it also authorizes, which is a config mistake (state no grant, or a
    /// grant for fewer lanes) rather than a meaningful "allow nothing."
    pub ceiling: u64,
    pub on_counter_unavailable: CounterPosture,
}

/// The longest fixed window this slice accepts, in seconds (7 days).
///
/// Not an arbitrary round number: past roughly this length a fixed window
/// stops behaving like a rate limit and starts behaving like a *quota*, and
/// a quota needs durable counting that survives a restart — the very next
/// slice the issue describes, and something [`InProcessRateCounter`]
/// explicitly cannot provide. Refusing the shape at config load is how this
/// module says "not yet" without letting an operator believe a month-long
/// budget is being enforced across restarts.
pub const MAX_RATE_WINDOW_SECONDS: u64 = 7 * 24 * 60 * 60;

impl RateLimitDecl {
    /// Config-load shape check — `AppConfig::validate` runs this for every
    /// grant that declares a `rate:` block. `context` is the caller's own
    /// path into the document (`policy.roles['reader']`, ...), so the
    /// message points at the declaration rather than at this module.
    pub fn validate(&self, context: &str) -> Result<()> {
        if self.window_seconds == 0 {
            return Err(Error::Config(format!(
                "{context}: rate.window_seconds must be at least 1 — a zero-length window can never admit a request"
            )));
        }
        if self.window_seconds > MAX_RATE_WINDOW_SECONDS {
            return Err(Error::Config(format!(
                "{context}: rate.window_seconds is {} but the longest supported window is {MAX_RATE_WINDOW_SECONDS} seconds — a longer budget is a quota, which needs durable counting this build does not have",
                self.window_seconds
            )));
        }
        if self.ceiling == 0 {
            return Err(Error::Config(format!(
                "{context}: rate.ceiling must be at least 1 — a zero ceiling would refuse every request the same grant authorizes; remove the grant instead"
            )));
        }
        Ok(())
    }
}

/// One counter's identity (`#188`). Two grants that declare the *same*
/// ceiling for the same subject share one counter deliberately — a ceiling
/// declared twice is one ceiling, not two — while any difference in scope,
/// window, or ceiling gets its own, so a tight ceiling can never be
/// diluted by a looser one that happens to match the same request.
///
/// `tenant_id` is part of every key, whatever the [`RateScope`], including
/// [`RateScope::Principal`]. A ceiling lives in some tenant's role table
/// (platform-shared roles are still *evaluated* against one tenant at a
/// time — see `policy::resolve_role_table`), and letting tenant A's
/// declaration consume the same counter that throttles the same principal's
/// reads in tenant B would be exactly the cross-tenant influence
/// authorization directive 6 forbids everywhere else in the policy layer.
/// So a principal reading two tenants holds two independent budgets, one
/// per tenant, each sized by that tenant's own document.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CounterKey {
    pub tenant_id: String,
    pub scope: RateScope,
    /// The scope's own value: the principal identity, the role name, or the
    /// tenant id again (for [`RateScope::Tenant`], where `tenant_id` alone
    /// already identifies the bucket).
    pub scope_value: String,
    pub window_seconds: u64,
    pub ceiling: u64,
}

/// What a [`RateCounter`] saw for one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateObservation {
    /// The number of requests charged to this key in the current window,
    /// *including* the one being observed — so the very first request of a
    /// window reports `1`, and a `ceiling: 1` grant admits it.
    pub count: u64,
    /// Whole seconds until the current fixed window ends, always at least
    /// 1 — a `Retry-After: 0` would invite an immediate retry into the same
    /// exhausted window.
    pub reset_in_seconds: u64,
}

/// A counter that could not answer (`#188`). Carries a fixed, non-formatted
/// reason for the log line only; it is never echoed into a response body,
/// which reports the refusal in the operator-neutral terms
/// [`RateRefusal`] carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CounterUnavailable {
    pub reason: &'static str,
}

/// The counting seam (`#188`) — see this module's doc for why it is `async`
/// with only a synchronous backend in the tree.
///
/// An implementation must charge exactly one request per `observe` call and
/// report the resulting count, or report [`CounterUnavailable`]. It must
/// never panic and never block indefinitely: the caller is on the request
/// path, holding no lock of its own, and a counter that hangs would turn a
/// bound meant to protect the server into a way to stall it.
#[async_trait::async_trait]
pub trait RateCounter: Send + Sync {
    async fn observe(
        &self,
        key: &CounterKey,
    ) -> std::result::Result<RateObservation, CounterUnavailable>;
}

/// One tracked key's current fixed window.
#[derive(Debug, Clone, Copy)]
struct WindowState {
    /// Epoch second the current window began at — `now - now % window`.
    window_start: u64,
    /// Requests charged since `window_start`.
    count: u64,
}

/// The single-replica [`RateCounter`] (`#188`): a `HashMap` of fixed-window
/// counters behind one `Mutex`, correct for exactly one process and honest
/// about being nothing more.
///
/// **Single-replica only, by construction.** Each replica counts only what
/// it served, so a fleet of N replicas behind a load balancer enforces a
/// ceiling of roughly `N * ceiling` in aggregate. That is not a bug to be
/// worked around with a fudge factor — it is why the issue names a
/// fleet-atomic backend as the next step, and why this type's own name says
/// "in process." Counters are also process-lifetime: a restart begins every
/// window fresh, which is fine for a rate limit (windows are short) and is
/// exactly why a durable *quota* is out of scope here (see
/// [`MAX_RATE_WINDOW_SECONDS`]).
///
/// **Bounded, and it says so when the bound is reached.** An unbounded map
/// keyed by principal is a memory leak an anonymous flood could drive, so
/// the tracked key set is capped. Inserting a new key past the cap first
/// drops every key whose window has already elapsed (which is what
/// naturally happens as principals go quiet); if the map is still full of
/// *live* windows, the observation fails with [`CounterUnavailable`] rather
/// than growing — and each condition's declared [`CounterPosture`] then
/// decides whether that means refuse or serve. Reporting the bound is the
/// point: silently evicting a live counter would under-count the very
/// principals that filled the map.
pub struct InProcessRateCounter {
    windows: Mutex<HashMap<CounterKey, WindowState>>,
    key_capacity: usize,
}

impl InProcessRateCounter {
    /// A counter tracking up to [`DEFAULT_RATE_COUNTER_KEY_CAPACITY`] keys.
    pub fn new() -> Self {
        Self::with_key_capacity(DEFAULT_RATE_COUNTER_KEY_CAPACITY)
    }

    /// A counter tracking up to `key_capacity` distinct keys — see this
    /// type's own doc for what happens at the bound. A capacity of `0` is
    /// raised to `1`: a counter that can hold nothing would report every
    /// single observation unavailable, which is a configuration mistake
    /// rather than a meaningful setting.
    pub fn with_key_capacity(key_capacity: usize) -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
            key_capacity: key_capacity.max(1),
        }
    }

    /// [`observe`](RateCounter::observe) against a caller-supplied clock —
    /// the whole of this type's logic, with `now_epoch_seconds` injected so
    /// its window arithmetic can be tested without sleeping. The trait impl
    /// is this method plus a real clock read.
    pub fn observe_at(
        &self,
        key: &CounterKey,
        now_epoch_seconds: u64,
    ) -> std::result::Result<RateObservation, CounterUnavailable> {
        let window = key.window_seconds.max(1);
        let elapsed_in_window = now_epoch_seconds % window;
        let window_start = now_epoch_seconds - elapsed_in_window;
        // Always at least 1: see `RateObservation::reset_in_seconds`.
        let reset_in_seconds = (window - elapsed_in_window).max(1);

        let mut windows = self
            .windows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(state) = windows.get_mut(key) {
            if state.window_start == window_start {
                state.count += 1;
            } else {
                // A window that elapsed while this key stayed resident:
                // start the new one at this request.
                *state = WindowState {
                    window_start,
                    count: 1,
                };
            }
            return Ok(RateObservation {
                count: state.count,
                reset_in_seconds,
            });
        }

        if windows.len() >= self.key_capacity {
            windows.retain(|resident_key, state| {
                state.window_start + resident_key.window_seconds.max(1) > now_epoch_seconds
            });
            if windows.len() >= self.key_capacity {
                return Err(CounterUnavailable {
                    reason: "the in-process rate counter is at its tracked-key capacity",
                });
            }
        }
        windows.insert(
            key.clone(),
            WindowState {
                window_start,
                count: 1,
            },
        );
        Ok(RateObservation {
            count: 1,
            reset_in_seconds,
        })
    }

    /// How many keys are currently resident — for tests and diagnostics.
    pub fn tracked_keys(&self) -> usize {
        self.windows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

impl Default for InProcessRateCounter {
    fn default() -> Self {
        Self::new()
    }
}

/// Wall-clock seconds since the Unix epoch. A clock before the epoch (only
/// reachable on a badly misconfigured host) reads as `0` rather than
/// panicking: every window then starts at the epoch, which throttles
/// nothing incorrectly — it merely resets the current window once, exactly
/// as a restart would.
fn now_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[async_trait::async_trait]
impl RateCounter for InProcessRateCounter {
    async fn observe(
        &self,
        key: &CounterKey,
    ) -> std::result::Result<RateObservation, CounterUnavailable> {
        self.observe_at(key, now_epoch_seconds())
    }
}

/// Why a rate condition refused a request — the two causes are kept apart
/// because they mean genuinely different things to whoever reads the log or
/// the problem body: one is the client using its declared budget, the other
/// is the server unable to tell and an operator having said "then refuse."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateRefusalCause {
    /// The counter answered, and the count crossed the declared ceiling.
    CeilingReached,
    /// The counter could not answer (or the scope could not be keyed for
    /// this subject) and the condition declared
    /// [`CounterPosture::Strict`].
    CounterUnavailable,
}

impl RateRefusalCause {
    fn as_str(self) -> &'static str {
        match self {
            RateRefusalCause::CeilingReached => "ceiling_reached",
            RateRefusalCause::CounterUnavailable => "counter_unavailable",
        }
    }
}

/// One refused request's full, already-decided story (`#188`) — everything a
/// caller needs to render `429` with `Retry-After` without re-reading the
/// policy document. Carries no principal identity, no role name and no
/// tenant id: this travels into a response body, and which *bucket* was
/// exhausted is not a client's business (a shared `role`- or `tenant`-scoped
/// ceiling would otherwise let one client learn about another's traffic).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateRefusal {
    pub scope: RateScope,
    pub window_seconds: u64,
    pub ceiling: u64,
    /// What to put in `Retry-After`, in seconds. For
    /// [`RateRefusalCause::CeilingReached`] this is exact — the time to the
    /// current fixed window's boundary. For
    /// [`RateRefusalCause::CounterUnavailable`] there is no boundary to
    /// report (no counter answered), so it is the declared window length:
    /// the longest a client would ever have had to wait had the counter
    /// been working, which is the honest conservative answer.
    pub retry_after_seconds: u64,
    pub cause: RateRefusalCause,
}

impl RateRefusal {
    /// The client-facing sentence for a `429` problem body. Names the
    /// declared ceiling and window (an operator-published fact the client is
    /// entitled to, and the only way for it to back off intelligently) and,
    /// for the unavailable case, says plainly that the bound could not be
    /// evaluated rather than implying the client exhausted anything.
    pub fn detail(&self) -> String {
        match self.cause {
            RateRefusalCause::CeilingReached => format!(
                "the applicable {}-scoped rate limit of {} requests per {} seconds is exhausted; retry after {} seconds",
                self.scope.as_str(),
                self.ceiling,
                self.window_seconds,
                self.retry_after_seconds,
            ),
            RateRefusalCause::CounterUnavailable => format!(
                "the applicable {}-scoped rate limit could not be evaluated and this deployment declares a strict failure posture; retry after {} seconds",
                self.scope.as_str(),
                self.retry_after_seconds,
            ),
        }
    }
}

/// `policy::enforce_rate_limits`' verdict (`#188`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RateVerdict {
    /// Every applicable ceiling admitted this request — including the case
    /// where no ceiling applied at all.
    Permitted,
    Refused(RateRefusal),
}

/// Whether a policy checkpoint should charge the applicable ceilings, or is
/// merely asking what a subject *could* see (`#188`).
///
/// Not every `authorize_resource` call is a request: a collections listing
/// runs the same checkpoint once per candidate collection purely to decide
/// which entries to include, and charging one request's ceiling N times for
/// one cheap metadata response would make a 600-per-minute ceiling behave
/// like a 6-per-minute one on a 100-collection catalog. Callers therefore
/// say which they are doing, by name, at the call site — a boolean would
/// have made the two indistinguishable while reading a handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateCharge {
    /// This checkpoint stands for one served request: charge it.
    Charge,
    /// This checkpoint is a visibility probe inside a listing or fan-out
    /// that another checkpoint already charged (or will charge): evaluate
    /// nothing and touch no counter.
    Skip,
}

/// One condition's outcome, before the conjunction across every matching
/// grant that `policy::enforce_rate_limits` applies.
pub(crate) enum ConditionOutcome {
    Permitted,
    Refused(RateRefusal),
}

/// Charges one condition and reports whether it admits this request
/// (`#188`). Shared by `policy::enforce_rate_limits`; kept here so the
/// counter interaction, the posture handling, and the metric all live
/// beside the types they are about.
///
/// `scope_value` is `None` when the scope could not be keyed for this
/// subject at all — the third of the three situations this module's doc
/// lists, routed through the very same declared posture as an unreachable
/// counter, because from the operator's point of view they are the same
/// question: the bound could not be evaluated, now what?
pub(crate) async fn evaluate_condition(
    decl: &RateLimitDecl,
    tenant_id: &str,
    scope_value: Option<String>,
    counter: Option<&dyn RateCounter>,
) -> ConditionOutcome {
    let unavailable = |reason: &'static str| -> ConditionOutcome {
        record_outcome(
            decl.scope,
            match decl.on_counter_unavailable {
                CounterPosture::Strict => "counter_unavailable_strict",
                CounterPosture::Graceful => "counter_unavailable_graceful",
            },
        );
        match decl.on_counter_unavailable {
            CounterPosture::Graceful => {
                tracing::warn!(
                    scope = decl.scope.as_str(),
                    posture = decl.on_counter_unavailable.as_str(),
                    reason,
                    "a policy rate condition went unenforced for one request"
                );
                ConditionOutcome::Permitted
            }
            CounterPosture::Strict => {
                tracing::warn!(
                    scope = decl.scope.as_str(),
                    posture = decl.on_counter_unavailable.as_str(),
                    reason,
                    "a policy rate condition refused a request it could not evaluate"
                );
                ConditionOutcome::Refused(RateRefusal {
                    scope: decl.scope,
                    window_seconds: decl.window_seconds,
                    ceiling: decl.ceiling,
                    retry_after_seconds: decl.window_seconds,
                    cause: RateRefusalCause::CounterUnavailable,
                })
            }
        }
    };

    let Some(scope_value) = scope_value else {
        return unavailable("the subject carries no identity for this condition's scope");
    };
    let Some(counter) = counter else {
        return unavailable("no rate-counter backend is wired into this deployment");
    };

    let key = CounterKey {
        tenant_id: tenant_id.to_string(),
        scope: decl.scope,
        scope_value,
        window_seconds: decl.window_seconds,
        ceiling: decl.ceiling,
    };
    match counter.observe(&key).await {
        Err(CounterUnavailable { reason }) => unavailable(reason),
        Ok(observation) if observation.count <= decl.ceiling => {
            record_outcome(decl.scope, "permitted");
            ConditionOutcome::Permitted
        }
        Ok(observation) => {
            record_outcome(decl.scope, RateRefusalCause::CeilingReached.as_str());
            ConditionOutcome::Refused(RateRefusal {
                scope: decl.scope,
                window_seconds: decl.window_seconds,
                ceiling: decl.ceiling,
                retry_after_seconds: observation.reset_in_seconds,
                cause: RateRefusalCause::CeilingReached,
            })
        }
    }
}

/// The one metric this module emits. Both labels are closed vocabularies
/// fixed at compile time (three scopes, four outcomes), so the series count
/// is bounded at twelve no matter how many tenants, principals or roles a
/// deployment has — which is why neither a tenant id nor a principal
/// appears here at all. `admission.rs` folds unlisted tenants into a shared
/// `"other"` label because its metric is *about* tenants; this one is about
/// how the mechanism behaved, and adding an identity label would leak
/// exactly the per-principal traffic shape `RateRefusal` deliberately keeps
/// out of the response body.
fn record_outcome(scope: RateScope, outcome: &'static str) {
    metrics::counter!(
        "policy_rate_limit_decisions_total",
        "scope" => scope.as_str(),
        "outcome" => outcome,
    )
    .increment(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(scope_value: &str, window_seconds: u64, ceiling: u64) -> CounterKey {
        CounterKey {
            tenant_id: "tenant-a".to_string(),
            scope: RateScope::Principal,
            scope_value: scope_value.to_string(),
            window_seconds,
            ceiling,
        }
    }

    // -- fixed-window arithmetic ------------------------------------------

    #[test]
    fn the_first_request_of_a_window_counts_one() {
        let counter = InProcessRateCounter::new();
        let observation = counter.observe_at(&key("alice", 60, 10), 1_000).unwrap();
        assert_eq!(observation.count, 1);
    }

    #[test]
    fn requests_within_one_window_accumulate() {
        let counter = InProcessRateCounter::new();
        let k = key("alice", 60, 10);
        for expected in 1..=5 {
            let observation = counter.observe_at(&k, 1_000 + expected).unwrap();
            assert_eq!(observation.count, expected);
        }
    }

    #[test]
    fn crossing_a_window_boundary_resets_the_count() {
        let counter = InProcessRateCounter::new();
        let k = key("alice", 60, 10);
        // 1_000 % 60 == 40, so this window began at 960 and ends at 1_020.
        assert_eq!(counter.observe_at(&k, 1_000).unwrap().count, 1);
        assert_eq!(counter.observe_at(&k, 1_019).unwrap().count, 2);
        assert_eq!(
            counter.observe_at(&k, 1_020).unwrap().count,
            1,
            "the first request of the next fixed window starts over"
        );
    }

    #[test]
    fn reset_in_seconds_counts_down_to_the_window_boundary_and_never_reaches_zero() {
        let counter = InProcessRateCounter::new();
        let k = key("alice", 60, 10);
        assert_eq!(counter.observe_at(&k, 960).unwrap().reset_in_seconds, 60);
        assert_eq!(counter.observe_at(&k, 1_000).unwrap().reset_in_seconds, 20);
        assert_eq!(
            counter.observe_at(&k, 1_019).unwrap().reset_in_seconds,
            1,
            "the last second of a window must still advertise a whole second"
        );
    }

    #[test]
    fn distinct_scope_values_never_share_a_counter() {
        let counter = InProcessRateCounter::new();
        assert_eq!(
            counter
                .observe_at(&key("alice", 60, 10), 1_000)
                .unwrap()
                .count,
            1
        );
        assert_eq!(
            counter
                .observe_at(&key("bob", 60, 10), 1_000)
                .unwrap()
                .count,
            1,
            "one principal's traffic must never charge another's ceiling"
        );
    }

    #[test]
    fn a_different_ceiling_for_the_same_subject_gets_its_own_counter() {
        let counter = InProcessRateCounter::new();
        assert_eq!(
            counter
                .observe_at(&key("alice", 60, 10), 1_000)
                .unwrap()
                .count,
            1
        );
        assert_eq!(
            counter
                .observe_at(&key("alice", 60, 100), 1_000)
                .unwrap()
                .count,
            1,
            "a tighter ceiling must not be diluted by a looser one that matched the same request"
        );
    }

    #[test]
    fn the_same_declared_ceiling_in_two_tenants_stays_two_counters() {
        let counter = InProcessRateCounter::new();
        let mut other_tenant = key("alice", 60, 10);
        other_tenant.tenant_id = "tenant-b".to_string();
        assert_eq!(
            counter
                .observe_at(&key("alice", 60, 10), 1_000)
                .unwrap()
                .count,
            1
        );
        assert_eq!(
            counter.observe_at(&other_tenant, 1_000).unwrap().count,
            1,
            "one tenant's document must never consume another tenant's budget for the same principal"
        );
    }

    // -- the bounded key set ----------------------------------------------

    #[test]
    fn an_elapsed_windows_key_is_reclaimed_to_make_room() {
        let counter = InProcessRateCounter::with_key_capacity(1);
        assert!(counter.observe_at(&key("alice", 60, 10), 1_000).is_ok());
        // 1_120 is two windows past alice's, so her key is dead weight and
        // is dropped to admit bob's.
        assert!(counter.observe_at(&key("bob", 60, 10), 1_120).is_ok());
        assert_eq!(counter.tracked_keys(), 1);
    }

    #[test]
    fn a_full_map_of_live_windows_reports_unavailable_rather_than_growing() {
        let counter = InProcessRateCounter::with_key_capacity(1);
        assert!(counter.observe_at(&key("alice", 60, 10), 1_000).is_ok());
        let refused = counter.observe_at(&key("bob", 60, 10), 1_001);
        assert_eq!(
            refused,
            Err(CounterUnavailable {
                reason: "the in-process rate counter is at its tracked-key capacity"
            }),
            "a live counter must never be evicted to make room — the bound is reported instead"
        );
        assert_eq!(counter.tracked_keys(), 1);
        // The already-tracked key keeps counting: the capacity bound only
        // ever refuses NEW keys.
        assert_eq!(
            counter
                .observe_at(&key("alice", 60, 10), 1_001)
                .unwrap()
                .count,
            2
        );
    }

    // -- declaration validation -------------------------------------------

    fn decl(window_seconds: u64, ceiling: u64) -> RateLimitDecl {
        RateLimitDecl {
            scope: RateScope::Principal,
            window_seconds,
            ceiling,
            on_counter_unavailable: CounterPosture::Strict,
        }
    }

    #[test]
    fn a_well_formed_declaration_validates() {
        assert!(decl(60, 100).validate("ctx").is_ok());
        assert!(decl(1, 1).validate("ctx").is_ok());
        assert!(decl(MAX_RATE_WINDOW_SECONDS, 1).validate("ctx").is_ok());
    }

    #[test]
    fn a_zero_window_is_refused_by_name() {
        let err = decl(0, 100).validate("policy.roles['r']").unwrap_err();
        assert!(
            err.to_string()
                .contains("window_seconds must be at least 1"),
            "{err}"
        );
    }

    #[test]
    fn a_zero_ceiling_is_refused_by_name() {
        let err = decl(60, 0).validate("policy.roles['r']").unwrap_err();
        assert!(
            err.to_string().contains("ceiling must be at least 1"),
            "{err}"
        );
    }

    #[test]
    fn a_window_past_the_quota_boundary_is_refused_by_name() {
        let err = decl(MAX_RATE_WINDOW_SECONDS + 1, 100)
            .validate("policy.roles['r']")
            .unwrap_err();
        assert!(err.to_string().contains("quota"), "{err}");
    }

    // -- the declared failure posture -------------------------------------

    struct BrokenCounter;

    #[async_trait::async_trait]
    impl RateCounter for BrokenCounter {
        async fn observe(
            &self,
            _key: &CounterKey,
        ) -> std::result::Result<RateObservation, CounterUnavailable> {
            Err(CounterUnavailable {
                reason: "the test counter is deliberately broken",
            })
        }
    }

    #[tokio::test]
    async fn a_strict_condition_refuses_when_the_counter_is_broken() {
        let mut declaration = decl(60, 100);
        declaration.on_counter_unavailable = CounterPosture::Strict;
        let outcome = evaluate_condition(
            &declaration,
            "tenant-a",
            Some("alice".to_string()),
            Some(&BrokenCounter),
        )
        .await;
        match outcome {
            ConditionOutcome::Refused(refusal) => {
                assert_eq!(refusal.cause, RateRefusalCause::CounterUnavailable);
                assert_eq!(
                    refusal.retry_after_seconds, 60,
                    "with no window boundary to report, the whole declared window is the honest wait"
                );
            }
            ConditionOutcome::Permitted => panic!("a strict posture must refuse"),
        }
    }

    #[tokio::test]
    async fn a_graceful_condition_serves_when_the_counter_is_broken() {
        let mut declaration = decl(60, 100);
        declaration.on_counter_unavailable = CounterPosture::Graceful;
        let outcome = evaluate_condition(
            &declaration,
            "tenant-a",
            Some("alice".to_string()),
            Some(&BrokenCounter),
        )
        .await;
        assert!(matches!(outcome, ConditionOutcome::Permitted));
    }

    #[tokio::test]
    async fn no_wired_backend_at_all_takes_the_same_declared_posture() {
        let mut strict = decl(60, 100);
        strict.on_counter_unavailable = CounterPosture::Strict;
        assert!(matches!(
            evaluate_condition(&strict, "tenant-a", Some("alice".to_string()), None).await,
            ConditionOutcome::Refused(_)
        ));

        let mut graceful = decl(60, 100);
        graceful.on_counter_unavailable = CounterPosture::Graceful;
        assert!(matches!(
            evaluate_condition(&graceful, "tenant-a", Some("alice".to_string()), None).await,
            ConditionOutcome::Permitted
        ));
    }

    #[tokio::test]
    async fn an_unkeyable_scope_takes_the_same_declared_posture() {
        let counter = InProcessRateCounter::new();
        let mut strict = decl(60, 100);
        strict.on_counter_unavailable = CounterPosture::Strict;
        assert!(
            matches!(
                evaluate_condition(&strict, "tenant-a", None, Some(&counter)).await,
                ConditionOutcome::Refused(_)
            ),
            "a principal-scoped ceiling against a subject with no principal cannot be counted"
        );
        assert_eq!(
            counter.tracked_keys(),
            0,
            "an unkeyable condition must never invent a key to charge"
        );
    }

    #[tokio::test]
    async fn a_condition_refuses_only_once_the_ceiling_is_actually_crossed() {
        let counter = InProcessRateCounter::new();
        let declaration = decl(60, 2);
        for _ in 0..2 {
            assert!(matches!(
                evaluate_condition(
                    &declaration,
                    "tenant-a",
                    Some("alice".to_string()),
                    Some(&counter)
                )
                .await,
                ConditionOutcome::Permitted
            ));
        }
        match evaluate_condition(
            &declaration,
            "tenant-a",
            Some("alice".to_string()),
            Some(&counter),
        )
        .await
        {
            ConditionOutcome::Refused(refusal) => {
                assert_eq!(refusal.cause, RateRefusalCause::CeilingReached);
                assert_eq!(refusal.ceiling, 2);
                assert!(refusal.retry_after_seconds >= 1);
            }
            ConditionOutcome::Permitted => panic!("the third request crosses a ceiling of 2"),
        }
    }

    #[test]
    fn a_refusal_detail_names_the_declared_ceiling_without_naming_a_bucket() {
        let refusal = RateRefusal {
            scope: RateScope::Role,
            window_seconds: 60,
            ceiling: 100,
            retry_after_seconds: 12,
            cause: RateRefusalCause::CeilingReached,
        };
        let detail = refusal.detail();
        assert!(detail.contains("100"), "{detail}");
        assert!(detail.contains("60"), "{detail}");
        assert!(detail.contains("12"), "{detail}");
        assert!(detail.contains("role"), "{detail}");
    }
}
