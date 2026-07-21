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

/// Total size in bytes of all regular files under `dir` (0 if absent).
pub fn dir_size(dir: &Path) -> u64 {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// Ok iff writing `new_len` bytes to `target` (possibly replacing an existing
/// file) keeps `dir`'s total within `cap`.
pub fn check_cap(dir: &Path, target: &Path, new_len: u64, cap: u64) -> Result<(), String> {
    let current = dir_size(dir);
    let replaced = std::fs::metadata(target).map(|m| m.len()).unwrap_or(0);
    let projected = current.saturating_sub(replaced).saturating_add(new_len);
    if projected > cap {
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
        // replacing 4 bytes with 6 -> projected 6 <= cap 8 OK
        assert!(check_cap(d.path(), &target, 6, 8).is_ok());
        // a brand-new 10-byte file on top of the existing 4 -> 14 > cap 8
        let other = d.path().join("g");
        assert!(check_cap(d.path(), &other, 10, 8).is_err());
    }
}
