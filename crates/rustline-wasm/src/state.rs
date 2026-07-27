//! Filesystem sandboxing + state-dir quota accounting. Pure helpers used by
//! the state/file host functions.

use std::fmt;
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

/// Why [`resolve_for_allowlist`] (or the [`normalize_abs`] it starts with)
/// refused a path.
///
/// [`NotAbsolute`](Self::NotAbsolute) and [`Traversal`](Self::Traversal) are
/// malformed-input rejections — true for any path regardless of a plugin's
/// config — so callers must not route them to a [`crate::capability::DenialObserver`].
/// [`SymlinkDenied`](Self::SymlinkDenied) is the one variant that names an
/// actual policy decision: it fires exactly when `resolve_symlinks` is off,
/// so flipping that one config value changes the outcome. It carries the
/// normalized path so a caller that must observe the denial doesn't need to
/// recompute it. [`Unresolvable`](Self::Unresolvable) and
/// [`NotUtf8`](Self::NotUtf8) are local resolution failures under
/// `resolve_symlinks = true` (a dangling parent, an unreadable intermediate
/// directory, non-UTF-8 output) rather than a policy refusal either.
#[derive(Debug)]
pub enum PathResolveError {
    /// Not an absolute path.
    NotAbsolute,
    /// Contained a `..` component.
    Traversal,
    /// A path component is a symlink and `resolve_symlinks` is off for this
    /// plugin — this IS the policy `resolve_symlinks` toggles.
    SymlinkDenied(String),
    /// `resolve_symlinks = true` but the path (or its parent) could not be
    /// canonicalized. Carries the ready-to-display message, since the two
    /// sites that raise this (a missing parent, a failed `canonicalize`) want
    /// different detail.
    Unresolvable(String),
    /// The canonicalized path is not valid UTF-8.
    NotUtf8,
}

impl fmt::Display for PathResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAbsolute => write!(f, "path must be absolute"),
            Self::Traversal => write!(f, "path traversal not allowed"),
            Self::SymlinkDenied(_) => write!(
                f,
                "symlink not allowed; set resolve_symlinks = true for this plugin"
            ),
            Self::Unresolvable(msg) => f.write_str(msg),
            Self::NotUtf8 => write!(f, "resolved path is not valid UTF-8"),
        }
    }
}

/// Normalize an absolute path for allowlist matching: require absolute, reject
/// any `..` component. Returns the path as a string (matched against globs).
pub fn normalize_abs(path: &str) -> Result<String, PathResolveError> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(PathResolveError::NotAbsolute);
    }
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(PathResolveError::Traversal);
    }
    Ok(path.to_string())
}

/// Resolve a plugin-supplied absolute path into the string the allowlists are
/// matched against.
///
/// `normalize_abs` alone matches against the path as *written*, so a grant is a
/// grant over names rather than over a filesystem subtree: with
/// `allowed_paths = ["/home/u/notes/*"]`, a symlink at `~/notes/todo` pointing
/// at `~/.bashrc` passes the gate and the effect lands on `.bashrc`. The `..`
/// rejection is complete for literal traversal and does nothing about this.
///
/// Default (`resolve_symlinks = false`): any existing symlink component is
/// rejected outright. With `resolve_symlinks = true`: the path is canonicalized
/// and the *canonical* string is returned, so the allowlist is matched against
/// where the write will actually land. Either way, resolution happens strictly
/// before the allowlist check.
///
/// This closes the symlink-*component* gap, not every TOCTOU window: there is
/// no `O_NOFOLLOW`/`openat` here, so a symlink swapped in between this
/// resolution and the eventual open would not be caught. Closing that would
/// need `openat`-style APIs, which is a larger change than this function.
pub fn resolve_for_allowlist(
    path: &str,
    resolve_symlinks: bool,
) -> Result<String, PathResolveError> {
    let norm = normalize_abs(path)?;
    let p = Path::new(&norm);
    if !resolve_symlinks {
        // Rebuild through `components()` first: it discards a trailing
        // separator and a lone trailing `.`, whereas `Path::ancestors` walks
        // the string as written. Without this, a path ending "/link/" or
        // "/link/." would have `ancestors` yield that trailing-separator form
        // (on which `symlink_metadata` *follows* the link rather than
        // reporting it, per POSIX lstat semantics) and then jump straight to
        // the grandparent — never yielding the bare "/dir/link" that would
        // catch it. `normalize_abs` already rejected any `..`, so this can't
        // reintroduce traversal.
        let clean: PathBuf = p.components().collect();
        // Walk every ancestor; a component that does not exist cannot be a
        // symlink, and a write target legitimately may not exist yet.
        for ancestor in clean.ancestors() {
            match std::fs::symlink_metadata(ancestor) {
                Ok(m) if m.file_type().is_symlink() => {
                    return Err(PathResolveError::SymlinkDenied(norm));
                }
                _ => {}
            }
        }
        return Ok(norm);
    }
    // Resolve. A not-yet-existing target resolves through its parent so a
    // first write to a granted directory still works.
    let resolved = match std::fs::canonicalize(p) {
        Ok(c) => c,
        Err(_) => {
            let parent = p
                .parent()
                .ok_or_else(|| PathResolveError::Unresolvable("cannot resolve path".to_string()))?;
            let name = p
                .file_name()
                .ok_or_else(|| PathResolveError::Unresolvable("cannot resolve path".to_string()))?;
            std::fs::canonicalize(parent)
                .map_err(|e| PathResolveError::Unresolvable(format!("cannot resolve path: {e}")))?
                .join(name)
        }
    };
    resolved
        .to_str()
        .map(str::to_string)
        .ok_or(PathResolveError::NotUtf8)
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
    fn resolve_for_allowlist_keeps_normalize_abs_rules() {
        assert!(resolve_for_allowlist("relative/x", false).is_err());
        assert!(resolve_for_allowlist("/ok/../escape", false).is_err());
    }

    /// `resolve_for_allowlist(path, true)` falls back to canonicalizing the
    /// parent when `path` itself doesn't exist yet (so a first write to a
    /// granted directory still works — see the doc comment). When that
    /// fallback *also* fails — the parent doesn't exist either — there is no
    /// resolved location to match the allowlist against, and this must be a
    /// hard denial, never a silent fall-through to the raw (unresolved)
    /// string: that would defeat `resolve_symlinks` for exactly the
    /// not-yet-existing-target case the feature exists to handle safely.
    #[test]
    fn resolve_for_allowlist_denies_rather_than_falls_through_when_canonicalization_fails() {
        let dir = tempfile::tempdir().unwrap();
        let unreachable = dir.path().join("missing").join("also_missing.txt");
        assert!(
            resolve_for_allowlist(unreachable.to_str().unwrap(), true).is_err(),
            "canonicalization failing must deny, not fall through to the raw path"
        );
    }

    #[test]
    fn resolve_for_allowlist_rejects_a_symlink_component_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let under = link.join("f");
        assert!(
            resolve_for_allowlist(under.to_str().unwrap(), false).is_err(),
            "a symlinked parent component escapes a name-based grant"
        );
    }

    /// `lstat`/`symlink_metadata` on a path ending in a trailing separator
    /// *follows* the symlink (POSIX semantics — a trailing slash asserts "this
    /// must be a directory"), and `Path::ancestors` walks the string as
    /// written, so without normalizing first the ancestors walk would check
    /// the trailing-slash form (not a symlink, by the above) and then jump
    /// straight to the grandparent, never checking the bare directory itself.
    #[test]
    fn resolve_for_allowlist_rejects_a_symlink_named_with_a_trailing_separator() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let trailing = format!("{}/", link.display());
        assert!(
            resolve_for_allowlist(&trailing, false).is_err(),
            "a trailing separator must not let the symlink slip past the ancestors walk"
        );
    }

    /// Same gap as above, for a trailing `/.` instead of a trailing `/`.
    #[test]
    fn resolve_for_allowlist_rejects_a_symlink_named_with_a_trailing_curdir() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let trailing = format!("{}/.", link.display());
        assert!(
            resolve_for_allowlist(&trailing, false).is_err(),
            "a trailing `.` component must not let the symlink slip past the ancestors walk"
        );
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
