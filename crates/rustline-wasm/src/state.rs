//! Filesystem sandboxing + state-dir quota accounting. Pure helpers used by
//! the state/file host functions.

use std::path::{Component, Path, PathBuf};

/// Sanitize a plugin-supplied relative path for use under its own state dir.
/// Rejects absolute paths and any `..` traversal; strips `.`; requires a
/// non-empty result.
pub fn sanitize_relpath(relpath: &str) -> Result<PathBuf, String> {
    let p = Path::new(relpath);
    if p.is_absolute() {
        return Err("absolute path not allowed".into());
    }
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("path traversal not allowed".into());
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err("empty path".into());
    }
    Ok(out)
}

/// Normalize an absolute path for allowlist matching: require absolute, reject
/// any `..` component. Returns the path as a string (matched against globs).
pub fn normalize_abs(path: &str) -> Result<String, String> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err("path must be absolute".into());
    }
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("path traversal not allowed".into());
    }
    Ok(path.to_string())
}

#[cfg(test)]
thread_local! {
    /// Test-only instrumentation: counts calls to [`dir_size`] on the current
    /// thread, so a test can assert *how many* full-directory walks a
    /// sequence of operations actually paid — the load-bearing claim behind
    /// this module's memoization (see `CapabilityCtx::state_size`) is "N
    /// writes cost one walk," which is otherwise invisible from the outside.
    /// Thread-local rather than a shared global so tests running
    /// concurrently on other threads can't interfere with each other's
    /// counts.
    static WALK_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Total size in bytes of all regular files under `dir` (0 if absent).
///
/// **This is the expensive operation this module exists to avoid paying on
/// every write.** Callers on a hot per-render path (`check_cap`'s callers)
/// must go through a memo (`CapabilityCtx::state_size`) instead of calling
/// this directly.
pub fn dir_size(dir: &Path) -> u64 {
    #[cfg(test)]
    WALK_COUNT.with(|c| c.set(c.get() + 1));
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// Number of [`dir_size`] calls on the current thread since the last
/// [`reset_walk_count`]. Test-only.
#[cfg(test)]
pub(crate) fn walk_count() -> usize {
    WALK_COUNT.with(std::cell::Cell::get)
}

/// Zero this thread's [`dir_size`] call counter. Test-only.
#[cfg(test)]
pub(crate) fn reset_walk_count() {
    WALK_COUNT.with(|c| c.set(0));
}

/// The state dir's total size after writing `new_len` bytes on top of a
/// `current` total, where `replaced` is how many of those current bytes
/// belong to the file being overwritten (0 for a brand-new file).
///
/// Shared by [`check_cap`] (the pass/fail decision) and
/// `cache::write_entry` (which needs the same number to size how much to
/// evict) so the two can never drift apart — before this helper existed,
/// `write_entry` had to duplicate this exact formula inline to size its
/// eviction target.
pub(crate) fn projected_size(current: u64, replaced: u64, new_len: u64) -> u64 {
    current.saturating_sub(replaced).saturating_add(new_len)
}

/// Ok iff writing `new_len` bytes to `target` (possibly replacing an existing
/// file) keeps a state dir of `current_size` bytes within `cap`.
///
/// **Pure by design.** This used to call `dir_size` itself, so every write —
/// three per render on a state-backed plugin — paid a full recursive walk of a
/// directory that includes both unbounded cache namespaces. The caller now
/// supplies the size from a memo (see `CapabilityCtx::state_size`); the
/// decision itself is unchanged, and stays strictly before the write
/// (invariant N3). The single `metadata(target)` stat stays — it is O(1) and
/// the accounting needs it.
pub fn check_cap(current_size: u64, target: &Path, new_len: u64, cap: u64) -> Result<(), String> {
    let replaced = std::fs::metadata(target).map(|m| m.len()).unwrap_or(0);
    if projected_size(current_size, replaced, new_len) > cap {
        Err("state quota exceeded".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn sanitize_rejects_absolute_and_parent() {
        assert!(sanitize_relpath("/etc/passwd").is_err());
        assert!(sanitize_relpath("../secrets").is_err());
        assert!(sanitize_relpath("a/../../b").is_err());
        assert!(sanitize_relpath("").is_err());
        assert_eq!(
            sanitize_relpath("weather.json").unwrap(),
            std::path::PathBuf::from("weather.json")
        );
        assert_eq!(
            sanitize_relpath("./sub/x").unwrap(),
            std::path::PathBuf::from("sub/x")
        );
    }

    #[test]
    fn normalize_abs_requires_absolute_and_rejects_parent() {
        assert!(normalize_abs("relative/x").is_err());
        assert!(normalize_abs("/ok/../escape").is_err());
        assert_eq!(normalize_abs("/var/lib/x").unwrap(), "/var/lib/x");
    }

    #[test]
    fn dir_size_sums_files() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("a"), b"12345").unwrap();
        fs::create_dir(d.path().join("sub")).unwrap();
        fs::write(d.path().join("sub/b"), b"678").unwrap();
        assert_eq!(dir_size(d.path()), 8);
    }

    #[test]
    fn check_cap_refuses_over_and_allows_replace_within() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("f");
        fs::write(&target, b"aaaa").unwrap(); // 4 bytes existing
        let current = dir_size(d.path());
        // replacing 4 bytes with 6 -> projected 6 <= cap 8 OK
        assert!(check_cap(current, &target, 6, 8).is_ok());
        // a brand-new 10-byte file on top of the existing 4 -> 14 > cap 8
        let other = d.path().join("g");
        assert!(check_cap(current, &other, 10, 8).is_err());
    }

    #[test]
    fn check_cap_is_pure_and_does_not_walk() {
        // A directory that does not exist has no size to walk, so `dir_size`
        // on it would return 0 regardless — that alone doesn't prove
        // purity, since the old (pre-fix) implementation would have passed
        // this too. The walk counter makes the claim in the test's name
        // actually true.
        reset_walk_count();
        assert!(check_cap(0, Path::new("/no/such/f"), 5, 10).is_ok());
        assert!(check_cap(9, Path::new("/no/such/f"), 5, 10).is_err());
        assert_eq!(walk_count(), 0, "check_cap must never call dir_size itself");
    }
}
