//! cgroup v2 (unified hierarchy) resource-limit detection: `memory.max` and
//! `cpu.max`, both read from one directory (`DEFAULT_ROOT` in production, a
//! tempdir fixture tree in tests) — the v2 counterpart to `cgroup_v1.rs`'s
//! single-file `memory.limit_in_bytes` reader. v2 unifies every controller
//! under one hierarchy, so both limits share the same root here, unlike v1
//! where memory lives under its own `memory/` subtree one level down.

use std::path::Path;

/// Default root of the v2 unified hierarchy on a Linux container host.
pub(crate) const DEFAULT_ROOT: &str = "/sys/fs/cgroup";

/// Reads the v2 memory limit (`{root}/memory.max`). `None` covers every case
/// that should fall through to the next detection tier: the file is absent
/// (not a v2 host, or v2 isn't mounted at this root), its contents don't
/// parse, or it reports `"max"` — v2's own "no limit" sentinel, spelled as
/// text rather than v1's numeric one (`cgroup_v1`'s own `NO_LIMIT_SENTINEL`).
pub(crate) fn read_memory_limit_bytes(root: &Path) -> Option<u64> {
    let raw = std::fs::read_to_string(root.join("memory.max")).ok()?;
    let raw = raw.trim();
    if raw == "max" {
        return None;
    }
    raw.parse().ok()
}

/// Reads the v2 CPU quota (`{root}/cpu.max`) as a fractional core count.
/// Contents are the space-separated pair `"<quota> <period>"` in
/// microseconds — e.g. `"200000 100000"` is 2 whole CPUs, `"150000
/// 100000"` is 1.5. `None` covers an absent or malformed file, a missing
/// second field, and the unconstrained case (`quota` is the literal
/// `"max"`, or `period` is zero, which would otherwise divide by zero).
pub(crate) fn read_cpu_quota_cores(root: &Path) -> Option<f64> {
    let raw = std::fs::read_to_string(root.join("cpu.max")).ok()?;
    let mut parts = raw.split_whitespace();
    let quota = parts.next()?;
    let period: f64 = parts.next()?.parse().ok()?;
    if quota == "max" || period <= 0.0 {
        return None;
    }
    let quota: f64 = quota.parse().ok()?;
    Some(quota / period)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    // A counter, not just pid+timestamp: tests in this module run in
    // parallel and macOS's clock resolution isn't fine-grained enough to
    // keep concurrent `SystemTime::now()` calls from colliding (same
    // rationale as `cgroup_v1`'s own fixture helper).
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn fixture_root() -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "tellurion-core-cgroup-v2-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn write(root: &Path, name: &str, contents: &str) {
        std::fs::write(root.join(name), contents).unwrap();
    }

    #[test]
    fn parses_an_explicit_memory_limit() {
        let root = fixture_root();
        write(&root, "memory.max", "536870912\n");
        assert_eq!(read_memory_limit_bytes(&root), Some(536_870_912));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn treats_max_as_no_memory_limit() {
        let root = fixture_root();
        write(&root, "memory.max", "max\n");
        assert_eq!(read_memory_limit_bytes(&root), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_memory_max_is_no_limit() {
        let root = fixture_root();
        assert_eq!(read_memory_limit_bytes(&root), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unparsable_memory_max_is_no_limit() {
        let root = fixture_root();
        write(&root, "memory.max", "not-a-number\n");
        assert_eq!(read_memory_limit_bytes(&root), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn parses_a_whole_cpu_quota() {
        let root = fixture_root();
        write(&root, "cpu.max", "200000 100000\n");
        assert_eq!(read_cpu_quota_cores(&root), Some(2.0));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn parses_a_fractional_cpu_quota() {
        let root = fixture_root();
        write(&root, "cpu.max", "150000 100000\n");
        assert_eq!(read_cpu_quota_cores(&root), Some(1.5));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn treats_max_quota_as_unconstrained() {
        let root = fixture_root();
        write(&root, "cpu.max", "max 100000\n");
        assert_eq!(read_cpu_quota_cores(&root), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_cpu_max_is_unconstrained() {
        let root = fixture_root();
        assert_eq!(read_cpu_quota_cores(&root), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_cpu_max_is_unconstrained() {
        let root = fixture_root();
        write(&root, "cpu.max", "not-a-number\n");
        assert_eq!(read_cpu_quota_cores(&root), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cpu_max_missing_the_period_field_is_unconstrained() {
        let root = fixture_root();
        write(&root, "cpu.max", "200000\n");
        assert_eq!(read_cpu_quota_cores(&root), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cpu_max_with_a_zero_period_is_unconstrained() {
        let root = fixture_root();
        write(&root, "cpu.max", "200000 0\n");
        assert_eq!(read_cpu_quota_cores(&root), None);
        let _ = std::fs::remove_dir_all(root);
    }
}
