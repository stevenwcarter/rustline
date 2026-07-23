//! Persisted denial recorder: appends deduped capability-denial records to a
//! JSONL file under the data dir, so `rustline plugin denials <name>` can
//! show a user what a plugin has actually been denied. Task 8
//! (`capability::DenialObserver`) built the seam; this is the real,
//! persisted implementation wired into `register_plugins`'s production path.
//!
//! Best-effort, matching `toggles::write_toggles`/the cpu-sample-cache
//! discipline elsewhere in this codebase: any I/O failure is `warn!`-logged
//! and swallowed — a broken denial log must never break rendering
//! (invariant N2).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::capability::{DenialKind, DenialObserver};
use crate::paths::data_root;

/// One recorded denial: which plugin, what kind of capability, and the
/// denied target (URL or path), verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Denial {
    pub plugin: String,
    pub kind: DenialKind,
    pub target: String,
}

/// The default persisted-denial record file: `<data_root>/denials.jsonl`.
pub fn denials_path() -> PathBuf {
    data_root().join("denials.jsonl")
}

/// A [`DenialObserver`] that appends each denial to a JSONL file, deduping on
/// the exact `(plugin, kind, target)` triple so a repeatedly-denied
/// URL/path records once rather than once per render tick.
pub struct FileDenialObserver {
    path: PathBuf,
}

impl FileDenialObserver {
    /// Record into `path` (production callers pass [`denials_path`]; tests
    /// point directly at a tempdir file).
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl DenialObserver for FileDenialObserver {
    fn observe(&self, plugin: &str, kind: DenialKind, target: &str) {
        record(&self.path, plugin, kind, target);
    }
}

/// Append `(plugin, kind, target)` to `path` unless it's already present.
/// Best-effort: any failure (can't create the parent dir, can't open/write
/// the file) is `warn!`-logged and swallowed — never panics.
fn record(path: &Path, plugin: &str, kind: DenialKind, target: &str) {
    let already_recorded = read_records(path)
        .iter()
        .any(|d| d.plugin == plugin && d.kind == kind && d.target == target);
    if already_recorded {
        return;
    }
    let entry = Denial {
        plugin: plugin.to_string(),
        kind,
        target: target.to_string(),
    };
    let Ok(line) = serde_json::to_string(&entry) else {
        return; // Denial is trivially serializable; kept for totality.
    };
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        tracing::warn!(%error, path = %path.display(), "failed to create denials dir");
        return;
    }
    let write_result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| {
            use std::io::Write as _;
            writeln!(file, "{line}")
        });
    if let Err(error) = write_result {
        tracing::warn!(%error, path = %path.display(), "failed to write denial record");
    }
}

/// Parse every well-formed line in `path` as a [`Denial`]; a missing/unreadable
/// file or a malformed line is skipped rather than erroring — same total-read
/// discipline as `toggles::read_toggles`.
fn read_records(path: &Path) -> Vec<Denial> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// `name`'s recorded denials from the record file at `path`, in the order
/// recorded.
fn read_denials_at(path: &Path, name: &str) -> Vec<Denial> {
    read_records(path)
        .into_iter()
        .filter(|d| d.plugin == name)
        .collect()
}

/// `name`'s recorded denials from the default record file (empty if none, or
/// the file is absent/unreadable).
pub fn read_denials(name: &str) -> Vec<Denial> {
    read_denials_at(&denials_path(), name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_same_triple_records_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("denials.jsonl");
        let observer = FileDenialObserver::new(path.clone());

        for _ in 0..3 {
            observer.observe("weather", DenialKind::Url, "https://evil.example/");
        }

        assert_eq!(read_records(&path).len(), 1);
    }

    #[test]
    fn distinct_triples_each_record_separately() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("denials.jsonl");
        let observer = FileDenialObserver::new(path.clone());

        observer.observe("weather", DenialKind::Url, "https://a/");
        observer.observe("weather", DenialKind::Url, "https://b/"); // different target
        observer.observe("weather", DenialKind::Path, "https://a/"); // different kind
        observer.observe("counter", DenialKind::Url, "https://a/"); // different plugin

        assert_eq!(read_records(&path).len(), 4);
    }

    #[test]
    fn read_denials_round_trips_and_filters_by_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("denials.jsonl");
        let observer = FileDenialObserver::new(path.clone());

        observer.observe("weather", DenialKind::Url, "https://a/");
        observer.observe("counter", DenialKind::Path, "/etc/passwd");

        assert_eq!(
            read_denials_at(&path, "weather"),
            vec![Denial {
                plugin: "weather".to_string(),
                kind: DenialKind::Url,
                target: "https://a/".to_string(),
            }]
        );
        assert_eq!(
            read_denials_at(&path, "counter"),
            vec![Denial {
                plugin: "counter".to_string(),
                kind: DenialKind::Path,
                target: "/etc/passwd".to_string(),
            }]
        );
        assert!(read_denials_at(&path, "nonexistent").is_empty());
    }

    #[test]
    fn write_failure_is_swallowed_not_panicking() {
        // A regular file standing where a parent directory is expected makes
        // `create_dir_all` fail — exercises the best-effort I/O-failure path
        // (invariant N2: a broken denial log must never break rendering).
        let blocker = tempfile::NamedTempFile::new().unwrap();
        let path = blocker.path().join("sub").join("denials.jsonl");
        let observer = FileDenialObserver::new(path.clone());

        observer.observe("weather", DenialKind::Url, "https://a/"); // must not panic

        assert!(read_records(&path).is_empty());
    }

    #[test]
    fn read_records_skips_malformed_lines_and_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("denials.jsonl");
        std::fs::write(
            &path,
            "not json\n{\"plugin\":\"w\",\"kind\":\"url\",\"target\":\"x\"}\n\n",
        )
        .unwrap();

        let records = read_records(&path);

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].plugin, "w");
        assert!(read_records(&dir.path().join("nope.jsonl")).is_empty());
    }
}
