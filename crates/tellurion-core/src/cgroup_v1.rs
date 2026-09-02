//! cgroup v1 memory limit detection (`memory.limit_in_bytes`). The file
//! path is injectable — `crate::resources` (this crate's unified v1/v2
//! detector) passes the real cgroup path in production and a throwaway
//! fixture file in tests, and both run the same parsing code even on a host
//! (e.g. macOS) with no cgroup mount at all.

use std::path::{Path, PathBuf};

/// v1's memory limit lives one level down from the unified-hierarchy root
/// `cgroup_v2::DEFAULT_ROOT` also uses — `{root}/memory/memory.limit_in_bytes`
/// — since v1 splits each controller into its own subtree, unlike v2's
/// single flat directory.
pub(crate) fn path_under(root: &Path) -> PathBuf {
    root.join("memory").join("memory.limit_in_bytes")
}

/// v1's "no limit" sentinel: `LONG_MAX` rounded down to the host page size
/// (4096 on every target we ship for). An unconstrained v1 cgroup reports
/// this value instead of leaving the file absent.
const NO_LIMIT_SENTINEL: u64 = 9_223_372_036_854_771_712;

/// Reads a cgroup v1 memory limit at `path`. `None` covers every case that
/// should fall through to system RAM: the file is absent (not a v1 host),
/// its contents don't parse, or it reports the "no limit" sentinel.
pub(crate) fn read_limit_bytes(path: &Path) -> Option<u64> {
    let raw = std::fs::read_to_string(path).ok()?;
    let limit: u64 = raw.trim().parse().ok()?;
    (limit != NO_LIMIT_SENTINEL).then_some(limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    // A counter, not just pid+timestamp: tests in this module run in
    // parallel and macOS's clock resolution isn't fine-grained enough to
    // keep concurrent `SystemTime::now()` calls from colliding.
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn write_temp(contents: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "tellurion-core-cgroup-v1-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn parses_an_explicit_limit() {
        let path = write_temp("536870912\n");
        assert_eq!(read_limit_bytes(&path), Some(536_870_912));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn treats_the_no_limit_sentinel_as_no_limit() {
        let path = write_temp("9223372036854771712\n");
        assert_eq!(read_limit_bytes(&path), None);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn missing_file_is_no_limit() {
        assert_eq!(
            read_limit_bytes(Path::new("/nonexistent/tellurion-cgroup-v1-test-path")),
            None
        );
    }

    #[test]
    fn unparsable_contents_is_no_limit() {
        let path = write_temp("not-a-number\n");
        assert_eq!(read_limit_bytes(&path), None);
        let _ = std::fs::remove_file(path);
    }
}
