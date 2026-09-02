//! Container-aware resource-budget detection: one entry point per resource
//! (memory limit, effective CPU count) that tries the cgroup v2 unified
//! hierarchy first — the default on every modern kernel and orchestrator —
//! falls back to v1, then to host-level totals when neither cgroup
//! filesystem is mounted (macOS/dev, or a bare-metal Linux host). Callers
//! (the tile cache's byte budget in `cache.rs`, the PostGIS pool size, the
//! request concurrency ceiling) never special-case v1 vs v2 themselves —
//! they call [`effective_cpu_count`] or the memory detector below and get
//! back a number that is honest about the box it's actually running on.
//! Detection never spams the log: at most one `tracing::debug!` per call,
//! same as before this module existed.
//!
//! cgroup v1's CPU controller (`cpu.cfs_quota_us`/`cpu.cfs_period_us`) is
//! deliberately not read here — v2 has been the default across every major
//! distro and orchestrator for years, so a v1-only host is an increasingly
//! rare case, and adding it later is a small, self-contained follow-up.
//! Until then, a v1-only host's CPU-derived budgets fall back to the host
//! core count, exactly as they did before this module existed.

use std::path::Path;

use crate::{cgroup_v1, cgroup_v2};

/// Detects the memory limit at `root` (the v2 unified hierarchy's own
/// root, e.g. `/sys/fs/cgroup`): v2's `memory.max` first, then v1's
/// `memory/memory.limit_in_bytes` under that same root (identical to v1's
/// production path, since v1 also mounts at `/sys/fs/cgroup` by default).
/// Returns the limit plus a short label naming which tier answered, for the
/// caller's own startup log.
fn detect_cgroup_memory_limit_bytes(root: &Path) -> Option<(u64, &'static str)> {
    if let Some(limit) = cgroup_v2::read_memory_limit_bytes(root) {
        return Some((limit, "cgroup v2"));
    }
    let v1_path = cgroup_v1::path_under(root);
    if let Some(limit) = cgroup_v1::read_limit_bytes(&v1_path) {
        return Some((limit, "cgroup v1"));
    }
    None
}

/// Same v2-then-v1-then-none shape as memory, but v1 has no CPU quota
/// reader (see this module's own doc), so its middle tier is always empty.
fn detect_cgroup_cpu_quota_cores(root: &Path) -> Option<(f64, &'static str)> {
    cgroup_v2::read_cpu_quota_cores(root).map(|quota| (quota, "cgroup v2"))
}

/// Production entry point for the tile cache's byte budget
/// (`cache::MokaTileCache::from_memory_percent`): the detected cgroup
/// memory limit, or total system RAM when neither cgroup version is
/// mounted at the default root. Never absent — there is always *some*
/// number to budget a percentage of.
pub(crate) fn detect_memory_limit_bytes() -> u64 {
    detect_memory_limit_bytes_at(Path::new(cgroup_v2::DEFAULT_ROOT))
}

fn detect_memory_limit_bytes_at(root: &Path) -> u64 {
    if let Some((limit, source)) = detect_cgroup_memory_limit_bytes(root) {
        tracing::debug!(limit, source, "cgroup memory limit detected");
        return limit;
    }

    let mut system = sysinfo::System::new();
    system.refresh_memory();
    let total = system.total_memory();
    tracing::debug!(
        limit = total,
        "no cgroup memory limit found, using total system RAM"
    );
    total
}

/// Effective CPU parallelism for deriving pool/concurrency budgets: the
/// smaller of the host's reported core count and any cgroup v2 CPU quota —
/// a container throttled to e.g. 2 CPUs on a 32-core host must derive its
/// budgets from 2, not 32. Fractional quotas round up (a 1.5-CPU quota
/// still gets a whole thread of headroom); the result is never less than 1
/// and never more than the host's own reported core count, so a misread
/// quota can never derive a *larger* budget than the box actually has.
///
/// This is the shared "derived" tier every caller's own explicit-config
/// override takes precedence over — see `tellurion-postgis`'s
/// `pool::derive_pool_size` and the `tellurion` server's
/// `app::derive_max_concurrency` for how each applies that precedence.
pub fn effective_cpu_count() -> usize {
    effective_cpu_count_at(Path::new(cgroup_v2::DEFAULT_ROOT))
}

fn effective_cpu_count_at(root: &Path) -> usize {
    let host_cores = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(2);

    match detect_cgroup_cpu_quota_cores(root) {
        Some((quota, source)) => {
            let derived = (quota.ceil() as usize).clamp(1, host_cores);
            tracing::debug!(quota, cores = derived, source, "cgroup cpu quota detected");
            derived
        }
        None => host_cores,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn fixture_root() -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "tellurion-core-resources-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn host_cores() -> usize {
        std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(2)
    }

    #[test]
    fn v2_memory_limit_wins_when_both_versions_are_present() {
        let root = fixture_root();
        std::fs::write(root.join("memory.max"), "1073741824\n").unwrap();
        let v1_dir = root.join("memory");
        std::fs::create_dir_all(&v1_dir).unwrap();
        std::fs::write(v1_dir.join("memory.limit_in_bytes"), "536870912\n").unwrap();

        assert_eq!(
            detect_memory_limit_bytes_at(&root),
            1_073_741_824,
            "v2 must be tried before v1"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn falls_back_to_v1_when_v2_is_absent() {
        let root = fixture_root();
        let v1_dir = root.join("memory");
        std::fs::create_dir_all(&v1_dir).unwrap();
        std::fs::write(v1_dir.join("memory.limit_in_bytes"), "536870912\n").unwrap();

        assert_eq!(detect_memory_limit_bytes_at(&root), 536_870_912);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn v2_memory_max_of_max_falls_through_to_v1() {
        let root = fixture_root();
        std::fs::write(root.join("memory.max"), "max\n").unwrap();
        let v1_dir = root.join("memory");
        std::fs::create_dir_all(&v1_dir).unwrap();
        std::fs::write(v1_dir.join("memory.limit_in_bytes"), "536870912\n").unwrap();

        assert_eq!(detect_memory_limit_bytes_at(&root), 536_870_912);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn falls_back_to_host_ram_when_no_cgroup_fs_is_mounted() {
        let root = fixture_root(); // empty directory, no cgroup files at all
        let mut system = sysinfo::System::new();
        system.refresh_memory();

        assert_eq!(detect_memory_limit_bytes_at(&root), system.total_memory());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_memory_max_falls_through_to_v1() {
        let root = fixture_root();
        std::fs::write(root.join("memory.max"), "not-a-number\n").unwrap();
        let v1_dir = root.join("memory");
        std::fs::create_dir_all(&v1_dir).unwrap();
        std::fs::write(v1_dir.join("memory.limit_in_bytes"), "536870912\n").unwrap();

        assert_eq!(detect_memory_limit_bytes_at(&root), 536_870_912);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cpu_quota_never_derives_more_than_the_host_core_count() {
        let root = fixture_root();
        // A quota far above whatever this test machine actually has must
        // never derive a *larger* budget than the box can back.
        std::fs::write(root.join("cpu.max"), "10000000 100000\n").unwrap();

        assert_eq!(effective_cpu_count_at(&root), host_cores());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cpu_quota_rounds_a_fractional_core_up() {
        let root = fixture_root();
        std::fs::write(root.join("cpu.max"), "150000 100000\n").unwrap(); // 1.5 cores

        let expected = (1.5f64.ceil() as usize).clamp(1, host_cores());
        assert_eq!(effective_cpu_count_at(&root), expected);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn falls_back_to_host_cores_when_cpu_max_is_absent() {
        let root = fixture_root();
        assert_eq!(effective_cpu_count_at(&root), host_cores());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_cpu_max_falls_back_to_host_cores() {
        let root = fixture_root();
        std::fs::write(root.join("cpu.max"), "garbage\n").unwrap();
        assert_eq!(effective_cpu_count_at(&root), host_cores());
        let _ = std::fs::remove_dir_all(root);
    }
}
