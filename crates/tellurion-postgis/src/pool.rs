//! Pool construction. Size is derived from available CPU parallelism —
//! `StorageDecl` carries no per-storage override field yet, so there is
//! nothing to override from (see the crate-level docs for the follow-up). A
//! `post_create` hook pins `statement_timeout` to the server's HTTP request
//! ceiling once per physical connection; `RecyclingMethod::Fast` (the
//! default) never issues `RESET ALL`, so the GUC set here survives every
//! checkout for the life of the connection. The checkout wait is bounded to
//! the same ceiling so pool exhaustion fails fast (`PoolError::Timeout`,
//! mapped to `Error::Timeout`) instead of silently queuing for however long
//! the outer HTTP timeout allows.

use std::time::Duration;

use deadpool_postgres::{
    Config as DbConfig, Hook, HookError, ManagerConfig, Pool, PoolConfig, RecyclingMethod, Runtime,
    Timeouts,
};
use tokio_postgres::NoTls;

use crate::error::{PostgisError, Result};

const MIN_POOL_SIZE: usize = 4;
const MAX_POOL_SIZE: usize = 32;

/// Resolves `StorageDecl.pool_size` precedence: an explicit override wins
/// outright (never clamped — an operator who pins a number gets exactly
/// that number, not a value this driver second-guesses); absent that,
/// `clamp(effective_cores * 2, MIN_POOL_SIZE, MAX_POOL_SIZE)`, where
/// `effective_cores` is the caller's already-resolved, cgroup-aware CPU
/// count (`tellurion_core::resources::effective_cpu_count`) — doubled for
/// connection headroom, bounded to a sane range regardless of how small or
/// large the box turns out to be. Kept as a pure function of two plain
/// numbers (no filesystem access of its own) so the precedence itself is
/// unit-testable without a real or fixture cgroup tree; `effective_cores`
/// detection is `tellurion-core`'s own concern, tested there.
pub(crate) fn derive_pool_size(explicit: Option<usize>, effective_cores: usize) -> usize {
    explicit.unwrap_or_else(|| (effective_cores * 2).clamp(MIN_POOL_SIZE, MAX_POOL_SIZE))
}

pub(crate) fn build_pool(
    database_url: &str,
    statement_timeout_ms: u64,
    pool_size: usize,
) -> Result<Pool> {
    let mut config = DbConfig::new();
    config.url = Some(database_url.to_string());
    config.manager = Some(ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    });
    config.pool = Some(PoolConfig {
        timeouts: Timeouts {
            wait: Some(Duration::from_millis(statement_timeout_ms)),
            ..Timeouts::new()
        },
        ..PoolConfig::new(pool_size)
    });

    let statement = format!("SET statement_timeout = {statement_timeout_ms}");

    let pool = config
        .builder(NoTls)
        .map_err(PostgisError::from)?
        .post_create(Hook::async_fn(move |client, _metrics| {
            let statement = statement.clone();
            Box::pin(async move {
                client
                    .batch_execute(&statement)
                    .await
                    .map_err(HookError::Backend)
            })
        }))
        .runtime(Runtime::Tokio1)
        .build()
        .map_err(PostgisError::from)?;

    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_pool_size_is_within_bounds_across_a_wide_core_range() {
        for cores in [1, 2, 4, 8, 16, 64, 256] {
            let size = derive_pool_size(None, cores);
            assert!(
                (MIN_POOL_SIZE..=MAX_POOL_SIZE).contains(&size),
                "cores={cores} produced out-of-bounds pool size {size}"
            );
        }
    }

    /// Pins the precedence the config docs promise: explicit config wins
    /// over the cgroup-derived value, which in turn wins over the
    /// hardcoded clamp bounds — never the reverse.
    #[test]
    fn explicit_pool_size_wins_over_derived_which_wins_over_hardcoded_bounds() {
        // An explicit override applies even when it defies the derived
        // clamp bounds entirely (below MIN, above MAX) -- the operator's
        // number is never second-guessed.
        assert_eq!(derive_pool_size(Some(2), 16), 2);
        assert_eq!(derive_pool_size(Some(100), 2), 100);

        // No explicit override: derived from cores, then clamped to the
        // hardcoded bounds when cores alone would fall outside them.
        assert_eq!(derive_pool_size(None, 8), 16); // 8 * 2 = 16, already in bounds
        assert_eq!(derive_pool_size(None, 1), MIN_POOL_SIZE); // 1 * 2 = 2, clamped up
        assert_eq!(derive_pool_size(None, 100), MAX_POOL_SIZE); // clamped down
    }

    #[test]
    fn build_pool_bounds_the_checkout_wait() {
        // `Pool::build` never connects, so a syntactically valid but
        // unreachable URL is enough to exercise the config it constructs.
        let pool = build_pool("postgres://localhost/nonexistent", 5_000, 8).unwrap();
        assert_eq!(pool.timeouts().wait, Some(Duration::from_millis(5_000)));
    }

    #[test]
    fn build_pool_uses_the_resolved_pool_size() {
        let pool = build_pool("postgres://localhost/nonexistent", 5_000, 7).unwrap();
        assert_eq!(pool.status().max_size, 7);
    }
}
