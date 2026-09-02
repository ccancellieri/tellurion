//! `Lease` — the single-leader seam for outbox consumers (`#193`, closing
//! the transactional-outbox design doc's own deferred item in section 9).
//! The design doc's ordering invariant (section 2, rule 4) is "a *single
//! ordered consumer per collection*"; one process makes that trivial, and
//! two processes make it a coordination problem. This seam is how the
//! second process learns to stand down — and, per that doc's own wording,
//! it is "pure addition ... changing no invariant here": nothing below
//! alters what a consumer does once it leads.
//!
//! Three rules, deliberately narrow:
//!
//! - **Losing is an ordinary answer, not an error.**
//!   [`try_acquire`](Lease::try_acquire) returns `Ok(None)` for "another
//!   replica leads right now" and reserves `Err` for "I could not find
//!   out" (the coordinator was unreachable). A caller that conflates the
//!   two either logs an error on every poll tick of a healthy two-replica
//!   deployment, or — far worse — treats an unreachable coordinator as
//!   permission to lead. Same discipline `LinkContributor` applies by
//!   being infallible by design (`links.rs`): the return type, not a
//!   convention, is what keeps callers honest.
//! - **The lease is a value, not a callback.** Leadership lasts exactly as
//!   long as the returned [`LeaseGuard`] is alive; dropping it releases.
//!   That makes "stop leading on shutdown" a consequence of the task
//!   returning rather than something a consumer has to remember to do, and
//!   it makes a lost lease observable ([`LeaseGuard::is_live`]) rather
//!   than assumed — a coordinator connection that died has already handed
//!   leadership to somebody else whether or not this process noticed.
//! - **No implementation is required.** `StorageDriver::lease` is
//!   `Option`-shaped like every other capability, and a consumer takes
//!   `Option<LeaseBinding>`: absent, the consumer runs exactly as it did
//!   before this module existed. A single-binary-plus-PostgreSQL
//!   deployment pays nothing, which is the whole point of putting
//!   coordination in the database a write deployment already has instead
//!   of in a new mandatory component.
//!
//! What this is NOT: a fencing token. A [`LeaseGuard`] is advisory — it
//! coordinates cooperating replicas of *this* server, and it cannot stop a
//! process that never asked for the lease at all. That is enough here
//! precisely because the consumers it gates are idempotent by
//! construction: two appliers briefly overlapping re-apply version-gated
//! obligations into the same derived index (`IndexSink::apply`'s own
//! contract) and converge. The lease buys efficiency and ordering
//! stability, never correctness that the apply path does not already own.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::Result;

/// What a replica competes for: a stable, human-readable scope string that
/// every replica of the same deployment derives identically, and that two
/// unrelated deployments never derive the same value for.
///
/// The scope is the identity, not a hash of it: a backend whose locking
/// primitive is keyed by something narrower (a PostgreSQL advisory lock is
/// keyed by a `bigint`) derives that itself, from [`as_str`](Self::as_str),
/// and owns the consequences of doing so — see
/// `tellurion-postgis::lease_sql`. Keeping the readable form authoritative
/// is what lets a lease appear verbatim in a log line an operator has to
/// reason about ("who leads `index-applier/public/default/demo`?").
///
/// `namespace` is the one part an operator supplies
/// (`IndexApplierConfig::lease`): two independent deployments sharing one
/// physical database — a staging and a preview stack, the ordinary way a
/// small team runs — would otherwise derive identical scopes for their
/// identically-named collections and fight over a leadership neither
/// actually shares any state with.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LeaseKey {
    scope: String,
}

impl LeaseKey {
    /// Builds the scope for one consumer of one collection:
    /// `"[<namespace>/]<consumer>/<tenant>/<catalog>/<collection>"`.
    ///
    /// Per-collection rather than per-process on purpose: leadership then
    /// spreads across replicas instead of pinning every collection's
    /// consumer onto whichever process won a single global race, and a
    /// collection whose apply path is wedged cannot stall the others by
    /// holding one shared lease.
    pub fn for_collection(
        namespace: Option<&str>,
        consumer: &str,
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> Self {
        let scope = match namespace {
            Some(namespace) => {
                format!("{namespace}/{consumer}/{tenant}/{catalog}/{collection}")
            }
            None => format!("{consumer}/{tenant}/{catalog}/{collection}"),
        };
        Self { scope }
    }

    /// The scope verbatim — what a backend derives its own narrower key
    /// from, and what a log line names.
    pub fn as_str(&self) -> &str {
        &self.scope
    }
}

impl fmt::Display for LeaseKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.scope)
    }
}

/// The consumer name [`LeaseKey::for_collection`] scopes the index
/// applier's lease under (`crate::applier::run_applier`). Named once here
/// so the key an operator sees in a log line, and the key every replica
/// derives, come from the same place — a second consumer gaining a lease
/// later (tile invalidation, webhooks, retention) adds its own constant
/// beside this one rather than re-spelling a string literal.
pub const INDEX_APPLIER_CONSUMER: &str = "index-applier";

/// The backend-owned resource whose *existence* is the leadership: a
/// dedicated database session, a renewed Kubernetes `Lease` object, a file
/// lock. Implementors release in `Drop`; there is no `release` method
/// because a release a caller can forget to call is a leadership that
/// outlives the process that stopped doing the work.
pub trait LeaseHold: Send + Sync {
    /// `false` once the resource backing this hold has gone away — a
    /// dropped connection, an expired object — meaning leadership has
    /// already been lost regardless of whether the guard is still in hand.
    ///
    /// Holders answer from state they already have (a socket's closed
    /// flag), never by a round trip: this is polled on the consumer's hot
    /// path, and a check that can itself block or fail would just move the
    /// original problem. A holder with no cheap way to tell answers
    /// `true`, and the ordinary apply path stays as safe as it was — see
    /// this module's "not a fencing token" note.
    fn is_live(&self) -> bool;
}

/// Proof of leadership for one [`LeaseKey`], valid while it is alive.
/// Opaque on purpose: a consumer may only ask whether it still holds
/// ([`is_live`](Self::is_live)) and drop it, never reach through to
/// whatever the backend used.
pub struct LeaseGuard {
    hold: Box<dyn LeaseHold>,
    key: LeaseKey,
}

impl LeaseGuard {
    /// Wraps a backend's own hold. Called by [`Lease`] implementations
    /// only — constructing one does not acquire anything, it asserts that
    /// the caller already did.
    pub fn new(key: LeaseKey, hold: Box<dyn LeaseHold>) -> Self {
        Self { hold, key }
    }

    /// The key this guard leads.
    pub fn key(&self) -> &LeaseKey {
        &self.key
    }

    /// Whether leadership is still held — see [`LeaseHold::is_live`].
    pub fn is_live(&self) -> bool {
        self.hold.is_live()
    }
}

impl fmt::Debug for LeaseGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeaseGuard")
            .field("key", &self.key)
            .field("live", &self.is_live())
            .finish()
    }
}

/// A coordinator that can hand out single-leader leases. Advertised as an
/// `Option`-shaped [`StorageDriver`](crate::router::StorageDriver)
/// capability, so the database a write deployment already has becomes the
/// coordinator and clustering adds no mandatory component (`#193`).
#[async_trait]
pub trait Lease: Send + Sync {
    /// Tries once, without blocking, to become the leader for `key`.
    ///
    /// - `Ok(Some(guard))` — this caller leads until `guard` drops.
    /// - `Ok(None)` — somebody else leads right now. An ordinary answer:
    ///   the caller is expected to keep polling, not to log an error and
    ///   not to give up. See this module's own doc.
    /// - `Err(_)` — the coordinator could not be asked. Distinct from
    ///   `Ok(None)` because a caller must never read "I don't know" as
    ///   "nobody leads".
    ///
    /// Never waits for the incumbent to yield: a consumer's poll loop is
    /// already the retry mechanism, and a blocking acquire would hold a
    /// coordinator resource for the entire wait just to arrive at the same
    /// place one tick later.
    async fn try_acquire(&self, key: &LeaseKey) -> Result<Option<LeaseGuard>>;
}

/// Everything a leased consumer needs, in one value: the coordinator and
/// the key it competes for. Passed as `Option<LeaseBinding>` so "leased"
/// and "unleased" are the only two states expressible — a lease with no
/// key, or a key with no lease, cannot be constructed and therefore cannot
/// be a case anyone has to handle.
///
/// The key lives here rather than being derived inside the consumer
/// because it is a *deployment* fact — tenant, catalog, collection, and
/// the operator's configured namespace — known to the wiring layer that
/// resolved the routing, not to a pump that only ever sees one
/// `CollectionDecl`.
#[derive(Clone)]
pub struct LeaseBinding {
    pub lease: Arc<dyn Lease>,
    pub key: LeaseKey,
}

impl LeaseBinding {
    pub fn new(lease: Arc<dyn Lease>, key: LeaseKey) -> Self {
        Self { lease, key }
    }

    /// [`Lease::try_acquire`] for this binding's own key.
    pub async fn try_acquire(&self) -> Result<Option<LeaseGuard>> {
        self.lease.try_acquire(&self.key).await
    }
}

impl fmt::Debug for LeaseBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeaseBinding")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_is_stable_and_readable_so_two_replicas_derive_the_same_key() {
        let key =
            LeaseKey::for_collection(None, INDEX_APPLIER_CONSUMER, "public", "default", "demo");
        assert_eq!(key.as_str(), "index-applier/public/default/demo");
        assert_eq!(key.to_string(), "index-applier/public/default/demo");
        assert_eq!(
            key,
            LeaseKey::for_collection(None, INDEX_APPLIER_CONSUMER, "public", "default", "demo")
        );
    }

    /// The namespace exists for exactly one reason: two deployments
    /// sharing one physical database must not contend for each other's
    /// leadership. Pin that it actually separates them.
    #[test]
    fn a_namespace_separates_deployments_sharing_one_database() {
        let staging = LeaseKey::for_collection(
            Some("staging"),
            INDEX_APPLIER_CONSUMER,
            "public",
            "default",
            "demo",
        );
        let preview = LeaseKey::for_collection(
            Some("preview"),
            INDEX_APPLIER_CONSUMER,
            "public",
            "default",
            "demo",
        );
        let unnamespaced =
            LeaseKey::for_collection(None, INDEX_APPLIER_CONSUMER, "public", "default", "demo");
        assert_ne!(staging, preview);
        assert_ne!(staging, unnamespaced);
        assert_eq!(
            staging.as_str(),
            "staging/index-applier/public/default/demo"
        );
    }

    /// Leadership is per (consumer, tenant, catalog, collection): none of
    /// those four may collapse into another's key, or one collection's
    /// wedged consumer would stall an unrelated one.
    #[test]
    fn every_scope_component_separates_leadership() {
        let base = LeaseKey::for_collection(None, "index-applier", "public", "default", "demo");
        for other in [
            LeaseKey::for_collection(None, "tile-invalidation", "public", "default", "demo"),
            LeaseKey::for_collection(None, "index-applier", "other", "default", "demo"),
            LeaseKey::for_collection(None, "index-applier", "public", "other", "demo"),
            LeaseKey::for_collection(None, "index-applier", "public", "default", "other"),
        ] {
            assert_ne!(base, other);
        }
    }
}
