//! XDG path resolution for the plugin dir and per-plugin state dirs, plus
//! `~/` expansion. All under `$XDG_DATA_HOME/rustline` (fallback
//! `$HOME/.local/share/rustline`) per the design.

use std::path::{Path, PathBuf};

/// Expand a leading `~/` to `$HOME`; otherwise return the path as-is.
pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(p)
}

/// `$XDG_DATA_HOME/rustline` (fallback `$HOME/.local/share/rustline`).
pub fn data_root() -> PathBuf {
    std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share")
        })
        .join("rustline")
}

/// Root of per-plugin state dirs: `<data_root>/state`.
pub fn state_root() -> PathBuf {
    data_root().join("state")
}

/// Default plugin discovery dir: `<data_root>/plugins`.
pub fn default_plugin_dir() -> PathBuf {
    data_root().join("plugins")
}

/// Path of the wasmtime compilation-cache config TOML under the state root,
/// creating it if absent — or `None` if it can't be written. Best-effort:
/// `build_plugin` points wasmtime's on-disk compile cache here so a later cold
/// spawn deserializes an unchanged plugin instead of re-running Cranelift; on
/// any failure it simply builds without the cache (invariant N2: a plugin
/// never breaks the bar).
pub fn wasmtime_cache_config_path() -> Option<PathBuf> {
    ensure_wasmtime_cache_config(&state_root())
}

/// Lazily ensure `<root>/wasmtime-cache.toml` exists, pointing wasmtime's
/// compile cache at `<root>/wasmtime-cache/`, and return its path. Returns
/// `None` (never panics) on any I/O failure, or if the cache dir isn't
/// absolute (wasmtime requires an absolute cache directory). Uses the same
/// atomic temp-file + rename convention as `cpu.rs`'s snapshot store. The cache
/// dir is deliberately kept distinct from plugins' own state subdirs
/// (`<root>/<name>/`).
///
/// The cache directory is created up-front so that a `Some(path)` result
/// implies a writable, wasmtime-usable config: an unwritable root fails
/// `create_dir_all` and yields `None`, so `build_plugin` degrades to *no cache*
/// rather than handing wasmtime a config whose `from_file` would fail the
/// build (invariant N2: a plugin never breaks the bar).
pub fn ensure_wasmtime_cache_config(root: &Path) -> Option<PathBuf> {
    let cache_dir = root.join("wasmtime-cache");
    if !cache_dir.is_absolute() {
        return None; // wasmtime rejects a relative cache directory
    }
    // Creating the cache dir (idempotent) also creates `root`, and proves the
    // root is writable — an unwritable one degrades to `None` here.
    std::fs::create_dir_all(&cache_dir).ok()?;
    let config_path = root.join("wasmtime-cache.toml");
    // Fast path: the content is deterministic per root, so an existing file is
    // already correct — avoid rewriting it on every cold spawn.
    if config_path.is_file() {
        return Some(config_path);
    }
    let body = cache_config_toml(&cache_dir);
    let tmp = config_path.with_extension("tmp");
    std::fs::write(&tmp, body).ok()?;
    std::fs::rename(&tmp, &config_path).ok()?;
    Some(config_path)
}

/// Render the wasmtime compile-cache config TOML pointing at `cache_dir`.
///
/// The pinned wasmtime (43.x) parses `[cache]` with `#[serde(deny_unknown_
/// fields)]` and recognizes only `directory` (+ tuning fields) — it has **no**
/// `enabled` field and rejects one. (The W43 spike's doc-based claim that
/// `enabled = true` is required reflects an older cache-config format; the
/// `wasm-e2e` build_plugin tests pin the format actually accepted here.)
fn cache_config_toml(cache_dir: &Path) -> String {
    format!(
        "[cache]\ndirectory = {}\n",
        toml_basic_string(&cache_dir.display().to_string())
    )
}

/// Quote `s` as a TOML basic string (escaping `\` and `"`), so a cache
/// directory with unusual characters still yields valid TOML.
fn toml_basic_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_config_toml_has_cache_section_and_directory() {
        let dir = tempfile::tempdir().unwrap();
        let p = ensure_wasmtime_cache_config(dir.path()).unwrap();
        let toml = std::fs::read_to_string(&p).unwrap();
        assert!(toml.contains("[cache]"), "toml has [cache]: {toml}");
        assert!(toml.contains("directory ="), "toml has directory =: {toml}");
        // The cache dir sits under the root, distinct from any plugin subdir.
        assert!(
            toml.contains("wasmtime-cache"),
            "toml points at the cache dir: {toml}"
        );
        // wasmtime 43's `[cache]` uses `deny_unknown_fields` and has no
        // `enabled` field — emitting one makes `Cache::from_file` reject the
        // config and fail `build()` (the `wasm-e2e` tests pin this). So it must
        // NOT be present, contra the brief's doc-based assumption.
        assert!(
            !toml.contains("enabled"),
            "toml must omit the unsupported `enabled` field: {toml}"
        );
    }

    #[test]
    fn cache_config_none_on_unwritable_root_no_panic() {
        // Root sits *under a regular file*, so `create_dir_all` can't create
        // it: expect `None`, never a panic.
        let f = tempfile::NamedTempFile::new().unwrap();
        let unwritable_root = f.path().join("cannot-exist");
        assert!(ensure_wasmtime_cache_config(&unwritable_root).is_none());
    }

    #[test]
    fn cache_config_is_idempotent() {
        // A second call returns the same path without erroring (fast path:
        // the file already exists).
        let dir = tempfile::tempdir().unwrap();
        let first = ensure_wasmtime_cache_config(dir.path()).unwrap();
        let second = ensure_wasmtime_cache_config(dir.path()).unwrap();
        assert_eq!(first, second);
    }
}
