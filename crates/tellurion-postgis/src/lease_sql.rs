//! Pure builders for the advisory-lock lease (`#193`,
//! `tellurion_core::lease`): a [`LeaseKey`](tellurion_core::LeaseKey) scope
//! in, the `bigint` PostgreSQL keys its lock primitive on plus the SQL text
//! out — same "no I/O, no state, fully unit-testable" discipline
//! `index_sql.rs`/`write_sql.rs` follow.
//!
//! ## Why an advisory lock, and why no table
//!
//! `pg_try_advisory_lock` is mutual exclusion the database already
//! provides: no table, no DDL, no row to garbage-collect, no expiry to
//! renew, and — the part that matters most for a leader lease — **no
//! cleanup path that can fail**. A session-level advisory lock is released
//! by the server the moment the session ends, which is what makes a
//! `SIGKILL`ed pod, a severed network, and a graceful shutdown all
//! converge on the same outcome without anybody writing recovery code for
//! them. A lease table would need a heartbeat, an expiry, a clock
//! assumption, and a story for the row a dead process left behind; this
//! needs none of those, which is precisely why the outbox design doc could
//! call the lease "pure addition".
//!
//! The cost, stated plainly: the lock lives on **one session**, so the
//! leader must hold a dedicated connection for as long as it leads (see
//! `driver.rs`'s `PostgisLease`). It cannot ride the shared pool —
//! `pool.rs` recycles with `RecyclingMethod::Fast`, which never issues
//! `RESET ALL`, so a lock taken on a pooled connection would silently
//! outlive the checkout and leak leadership into an unrelated query.
//!
//! ## The key
//!
//! PostgreSQL keys advisory locks by `bigint`, so the readable scope is
//! hashed down to one. FNV-1a is used rather than [`std::hash`] because
//! this value must be identical across processes, builds, and releases:
//! `DefaultHasher` guarantees none of that, and a rolling upgrade in which
//! two versions compute different keys for the same collection is exactly
//! the split-leadership this exists to prevent.
//!
//! Two distinct scopes colliding onto one `bigint` is a ~2^-64 event, and
//! its consequence is bounded anyway: two collections would share one
//! leader, so their appliers serialize onto a single replica — a
//! throughput degradation, never a correctness one, since the apply path is
//! idempotent and version-gated regardless of who runs it.
//!
//! FNV-1a's avalanche is weak — scopes differing only in their last
//! characters land near each other numerically — and that is fine here, on
//! purpose: this key is compared for *equality* by a lock manager, never
//! bucketed, so only distinctness matters and adjacency costs nothing. The
//! property being bought is reproducibility, not distribution.

/// FNV-1a 64-bit, spelled out here rather than pulled from a dependency so
/// the exact bytes-to-`bigint` mapping is pinned by this crate's own tests
/// and cannot drift under a version bump. See this module's doc for why
/// stability across processes and releases is the whole requirement.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// The `bigint` `pg_try_advisory_lock` competes on for `scope`. Reinterprets
/// the 64 hash bits as `i64` (Postgres has no unsigned `bigint`) — a pure
/// bit cast, so the mapping stays a total, collision-free-per-hash-value
/// function of the hash rather than saturating half the space onto
/// `i64::MAX`.
pub(crate) fn advisory_key(scope: &str) -> i64 {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in scope.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    i64::from_ne_bytes(hash.to_ne_bytes())
}

/// Postgres truncates `application_name` at `NAMEDATALEN - 1` bytes.
const MAX_APPLICATION_NAME: usize = 63;
const LABEL_PREFIX: &str = "tellurion lease ";
/// Marks a label whose scope did not fit, so an operator reading
/// `pg_stat_activity` can tell a shortened scope from a real one.
const LABEL_ELISION: &str = "..";

/// What the leader's dedicated session announces itself as, so "which
/// replica currently leads this collection?" is answerable from
/// `pg_stat_activity` alone — the readable scope is the whole point of
/// having one (`tellurion_core::LeaseKey`'s own doc), and a `bigint` in
/// `pg_locks` answers nothing on its own.
///
/// A scope too long for `application_name` keeps its **tail**, not its
/// head: `LeaseKey`'s components run general to specific
/// (`namespace/consumer/tenant/catalog/collection`), so cutting from the
/// front drops the deployment-wide prefix every one of these labels shares
/// and keeps the collection — the part that actually distinguishes one
/// leader from another. Cutting the other way would leave a whole fleet
/// announcing the same indistinguishable string. Done here, on a character
/// boundary, rather than left to Postgres, so the value this crate sends is
/// exactly the value that lands.
pub(crate) fn session_label(scope: &str) -> String {
    let label = format!("{LABEL_PREFIX}{scope}");
    if label.len() <= MAX_APPLICATION_NAME {
        return label;
    }
    let budget = MAX_APPLICATION_NAME - LABEL_PREFIX.len() - LABEL_ELISION.len();
    let mut start = scope.len() - budget;
    while start < scope.len() && !scope.is_char_boundary(start) {
        start += 1;
    }
    format!("{LABEL_PREFIX}{LABEL_ELISION}{}", &scope[start..])
}

/// The acquisition itself: `$1` is [`advisory_key`]'s `bigint`.
///
/// Deliberately the `try_` variant: a blocking `pg_advisory_lock` would
/// park this connection inside Postgres until the incumbent yields, turning
/// every follower replica into a permanently held session waiting for a
/// tick that the caller's own poll loop already provides for free
/// (`tellurion_core::lease::Lease::try_acquire`'s own contract).
pub(crate) const TRY_ACQUIRE_SQL: &str = "SELECT pg_try_advisory_lock($1::bigint) AS acquired";

/// Run on the winner only, after the lock is held: `$1` is
/// [`session_label`]'s text. A second round trip rather than a column
/// alongside `pg_try_advisory_lock` above, so that a labelled session means
/// exactly one thing — "this session leads". Folding it into the
/// acquisition would also label every *loser*'s short-lived session, and
/// then `SELECT application_name FROM pg_stat_activity` — the one place an
/// operator looks — would intermittently show as many leaders as there are
/// replicas.
pub(crate) const LABEL_LEADER_SQL: &str = "SELECT set_config('application_name', $1::text, false)";

#[cfg(test)]
mod tests {
    use super::*;
    use tellurion_core::{LeaseKey, INDEX_APPLIER_CONSUMER};

    /// The one property the whole lease rests on: every replica, every
    /// build, every release must derive the identical `bigint` for the
    /// identical scope. Pinned as a literal — a change to this value is a
    /// split-leadership incident during a rolling upgrade, so it must
    /// never be silently "fixed" to match a new implementation.
    #[test]
    fn the_advisory_key_is_pinned_across_builds_and_releases() {
        let key =
            LeaseKey::for_collection(None, INDEX_APPLIER_CONSUMER, "public", "default", "demo");
        assert_eq!(key.as_str(), "index-applier/public/default/demo");
        assert_eq!(advisory_key(key.as_str()), advisory_key(key.as_str()));
        assert_eq!(
            advisory_key("index-applier/public/default/demo"),
            -783_035_605_302_961_175
        );
        assert_eq!(advisory_key(""), -3_750_763_034_362_895_579);
    }

    /// Distinct scopes must not share a lock, or two collections would
    /// share one leader. Not a proof of collision-freedom (nothing is), but
    /// it does catch the real failure: a key that ignores part of its input.
    #[test]
    fn distinct_scopes_map_to_distinct_keys() {
        let scopes = [
            "index-applier/public/default/demo",
            "index-applier/public/default/other",
            "index-applier/public/other/demo",
            "index-applier/other/default/demo",
            "tile-invalidation/public/default/demo",
            "staging/index-applier/public/default/demo",
        ];
        let mut keys: Vec<i64> = scopes.iter().copied().map(advisory_key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), scopes.len());
    }

    /// The whole `i64` range is in play. Worth pinning because the obvious
    /// alternative spellings of "make this fit a `bigint`" —
    /// `i64::try_from(hash).unwrap_or(i64::MAX)`, or masking off the top
    /// bit — would fold half the hash space onto a single key and make
    /// unrelated collections share one leader.
    #[test]
    fn keys_use_the_full_signed_range() {
        assert!(advisory_key("index-applier/public/default/demo") < 0);
        assert!(advisory_key("index-applier/public/default/c0") > 0);
    }

    #[test]
    fn the_session_label_names_the_scope_and_fits_application_name() {
        let label = session_label("index-applier/public/default/demo");
        assert_eq!(label, "tellurion lease index-applier/public/default/demo");
        assert!(label.len() <= MAX_APPLICATION_NAME);
    }

    /// A scope too long for `application_name` keeps the part that tells
    /// two leaders apart — the collection — and drops the shared prefix,
    /// marked so the shortening is visible rather than silent.
    #[test]
    fn a_long_scope_keeps_its_collection_and_drops_the_shared_prefix() {
        let label = session_label(
            "a-rather-long-deployment-namespace/index-applier/public/default/buildings",
        );
        assert!(label.len() <= MAX_APPLICATION_NAME);
        assert!(label.starts_with("tellurion lease .."));
        assert!(
            label.ends_with("/buildings"),
            "the collection must survive truncation: {label}"
        );
    }

    /// The cut respects character boundaries, so what lands in
    /// `application_name` is always valid UTF-8 rather than a Postgres-side
    /// truncation through the middle of a code point.
    #[test]
    fn a_long_scope_is_cut_on_a_character_boundary() {
        for extra in 0..8 {
            let label = session_label(&format!(
                "index-applier/public/default/{}{}",
                "é".repeat(40),
                "x".repeat(extra)
            ));
            assert!(label.len() <= MAX_APPLICATION_NAME);
            // Constructing it at all already proves the slice fell on a
            // boundary (a bad cut panics), so this pins the byte budget.
            assert!(std::str::from_utf8(label.as_bytes()).is_ok());
        }
    }
}
