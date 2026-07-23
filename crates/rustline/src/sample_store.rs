//! Shared best-effort atomic per-widget state-file persistence: read/write a
//! small text file under a state dir via a temp-file + rename, `warn!`ing
//! (never panicking) on any I/O failure — a broken cache must never break the
//! bar. Mirrors `cpu.rs`'s pre-existing `cpu-sample` snapshot store,
//! generalized so any read surface that wants a cross-invocation sample cache
//! (`throughput.rs` today; a future sparkline history widget tomorrow) can
//! reuse the same file-handling code instead of re-implementing the
//! atomic-write dance. Serialization/parsing of the sample's own shape stays
//! with each caller, same as `cpu.rs`'s `serialize_snapshot`/`parse_snapshot`.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// State-file path for `name` under `state_dir`: `<state_dir>/<name>`.
fn sample_path(state_dir: &Path, name: &str) -> PathBuf {
    state_dir.join(name)
}

/// Read the persisted sample at `<state_dir>/<name>` as raw text. A
/// missing/unreadable file yields `None` (treated as absent — the caller
/// falls back to its own cold-start behavior), never a panic. The caller
/// parses the contents into its own sample type.
pub fn read_sample(state_dir: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(sample_path(state_dir, name)).ok()
}

/// Best-effort atomic persist (temp file + rename) of `contents` at
/// `<state_dir>/<name>`; logs a warning and returns without panicking on any
/// I/O failure.
pub fn write_sample(state_dir: &Path, name: &str, contents: &str) {
    let path = sample_path(state_dir, name);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("tmp");
    if let Err(error) = std::fs::write(&tmp, contents) {
        tracing::warn!(%error, %name, "failed to write sample-store temp file");
        return;
    }
    if let Err(error) = std::fs::rename(&tmp, &path) {
        tracing::warn!(%error, %name, "failed to rename sample-store file");
    }
}

/// Current wall clock as unix seconds; a pre-epoch clock degrades to `0`
/// (which makes any prior sample read as maximally stale / backward-clock).
/// Shared so per-widget sample caches (`cpu.rs`'s own `now_unix_secs` predates
/// this module; `throughput.rs` uses this one) don't each reimplement it.
pub fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        write_sample(dir.path(), "throughput-sample", "1 2 3\n");
        assert_eq!(
            read_sample(dir.path(), "throughput-sample").as_deref(),
            Some("1 2 3\n")
        );
    }

    #[test]
    fn read_missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_sample(dir.path(), "nope").is_none());
    }

    #[test]
    fn write_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("nested/state");
        write_sample(&state_dir, "foo", "hi");
        assert_eq!(read_sample(&state_dir, "foo").as_deref(), Some("hi"));
    }

    #[test]
    fn write_overwrites_existing_sample() {
        let dir = tempfile::tempdir().unwrap();
        write_sample(dir.path(), "s", "old");
        write_sample(dir.path(), "s", "new");
        assert_eq!(read_sample(dir.path(), "s").as_deref(), Some("new"));
    }

    #[test]
    fn different_names_do_not_collide() {
        let dir = tempfile::tempdir().unwrap();
        write_sample(dir.path(), "a", "1");
        write_sample(dir.path(), "b", "2");
        assert_eq!(read_sample(dir.path(), "a").as_deref(), Some("1"));
        assert_eq!(read_sample(dir.path(), "b").as_deref(), Some("2"));
    }

    #[test]
    fn now_unix_secs_is_recent() {
        // Sanity: well after this feature's own epoch, never a panic.
        assert!(now_unix_secs() > 1_700_000_000); // 2023-11-14
    }
}
