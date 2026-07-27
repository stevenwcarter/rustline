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
        // Emit *alongside* the record, not instead of it. Without a log line a
        // denial is invisible unless the guest happens to call `rl_log`, and a
        // blank widget looks identical to a plugin that simply returned
        // nothing. `record`'s dedup keeps this to one line per distinct
        // triple, so a persistently-denied plugin does not spam the log.
        if record(&self.path, plugin, kind, target) {
            tracing::warn!(
                %plugin,
                ?kind,
                %target,
                "capability denied; see `rustline plugin denials`"
            );
        }
    }
}

/// Append `(plugin, kind, target)` to `path` unless it's already present.
/// Returns `true` iff the triple was newly recorded — the caller uses that to
/// log the denial exactly once, reusing the dedup that already keeps the JSONL
/// quiet.
///
/// Best-effort: any failure is `warn!`-logged, swallowed, and reported as
/// not-recorded (so a later successful write still logs it once).
fn record(path: &Path, plugin: &str, kind: DenialKind, target: &str) -> bool {
    let already_recorded = read_records(path)
        .iter()
        .any(|d| d.plugin == plugin && d.kind == kind && d.target == target);
    if already_recorded {
        return false;
    }
    let entry = Denial {
        plugin: plugin.to_string(),
        kind,
        target: target.to_string(),
    };
    let Ok(line) = serde_json::to_string(&entry) else {
        return false; // Denial is trivially serializable; kept for totality.
    };
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        tracing::warn!(%error, path = %path.display(), "failed to create denials dir");
        return false;
    }
    let write_result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| {
            use std::io::Write as _;
            writeln!(file, "{line}")
        });
    match write_result {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "failed to write denial record");
            false
        }
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

    // `observe` has no seam to inject a fake logger, so tests capture real
    // `tracing` events with a minimal hand-rolled `Subscriber` (no test-only
    // dep needed — `Subscriber`/`Visit` are already part of the `tracing`
    // crate). Mirrors the identical harness in `lib.rs`/`perform.rs` test
    // modules; not shared because it's private to each and reaching across
    // modules would mean widening visibility purely for a test.
    //
    // Every test that calls `observer.observe(...)` — not just the ones
    // asserting on log content — routes the call through `capture()`, even
    // when the returned events are discarded. `tracing` caches a callsite's
    // "interest" globally per process the first time it fires; if any test
    // hit `observe`'s `tracing::warn!` while the ambient (non-`with_default`)
    // dispatcher was active, that would cache the callsite as uninteresting
    // forever, and `observe_logs_exactly_once_for_a_repeated_denial` below
    // would then see zero events under `cargo test --workspace` even though
    // it passes in isolation — exactly the failure mode this avoids.
    use std::sync::{Arc, Mutex};

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    type CapturedEvent = (Level, Vec<(String, String)>);

    #[derive(Default)]
    struct FieldVisitor(Vec<(String, String)>);

    impl Visit for FieldVisitor {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0
                .push((field.name().to_string(), format!("{value:?}")));
        }
    }

    /// A subscriber that accepts and records every event, purely so a test
    /// can assert what `observe` logged.
    struct RecordingSubscriber(Arc<Mutex<Vec<CapturedEvent>>>);

    impl Subscriber for RecordingSubscriber {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut visitor = FieldVisitor::default();
            event.record(&mut visitor);
            self.0
                .lock()
                .unwrap()
                .push((*event.metadata().level(), visitor.0));
        }

        fn enter(&self, _span: &Id) {}
        fn exit(&self, _span: &Id) {}
    }

    /// Run `f` under a scoped recording subscriber and return every event it
    /// emitted, in order.
    fn capture(f: impl FnOnce()) -> Vec<CapturedEvent> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscriber = RecordingSubscriber(events.clone());
        tracing::subscriber::with_default(subscriber, f);
        events.lock().unwrap().clone()
    }

    fn field<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
        fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn dedup_same_triple_records_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("denials.jsonl");
        let observer = FileDenialObserver::new(path.clone());

        capture(|| {
            for _ in 0..3 {
                observer.observe("weather", DenialKind::Url, "https://evil.example/");
            }
        });

        assert_eq!(read_records(&path).len(), 1);
    }

    #[test]
    fn distinct_triples_each_record_separately() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("denials.jsonl");
        let observer = FileDenialObserver::new(path.clone());

        capture(|| {
            observer.observe("weather", DenialKind::Url, "https://a/");
            observer.observe("weather", DenialKind::Url, "https://b/"); // different target
            observer.observe("weather", DenialKind::Path, "https://a/"); // different kind
            observer.observe("counter", DenialKind::Url, "https://a/"); // different plugin
        });

        assert_eq!(read_records(&path).len(), 4);
    }

    #[test]
    fn read_denials_round_trips_and_filters_by_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("denials.jsonl");
        let observer = FileDenialObserver::new(path.clone());

        capture(|| {
            observer.observe("weather", DenialKind::Url, "https://a/");
            observer.observe("counter", DenialKind::Path, "/etc/passwd");
        });

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

        capture(|| {
            observer.observe("weather", DenialKind::Url, "https://a/"); // must not panic
        });

        assert!(read_records(&path).is_empty());
    }

    #[test]
    fn record_reports_whether_the_triple_was_new() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("denials.jsonl");
        assert!(record(&path, "weather", DenialKind::Url, "https://a/"));
        assert!(!record(&path, "weather", DenialKind::Url, "https://a/"));
        assert!(record(&path, "weather", DenialKind::Url, "https://b/"));
    }

    #[test]
    fn a_failed_write_does_not_claim_the_triple_was_recorded() {
        let blocker = tempfile::NamedTempFile::new().unwrap();
        let path = blocker.path().join("sub").join("denials.jsonl");
        assert!(!record(&path, "weather", DenialKind::Url, "https://a/"));
    }

    #[test]
    fn a_failed_open_does_not_claim_the_triple_was_recorded() {
        // Distinct from the `create_dir_all` failure above: here the parent
        // exists and `record` reaches the final `OpenOptions::open`, which
        // fails because `path` itself is a directory. Pins the write's own
        // `Err` arm returning `false` (not just the earlier bail-outs).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("denials.jsonl");
        std::fs::create_dir(&path).unwrap();
        assert!(!record(&path, "weather", DenialKind::Url, "https://a/"));
    }

    /// Pins the point of `observe`: it must log *exactly when* `record`
    /// reports the triple as newly seen, not unconditionally and not on every
    /// call regardless of dedup. A version that logs before/without checking
    /// `record`'s result would emit 3 events here instead of 1.
    #[test]
    fn observe_logs_exactly_once_for_a_repeated_denial() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("denials.jsonl");
        let observer = FileDenialObserver::new(path);

        let events = capture(|| {
            for _ in 0..3 {
                observer.observe("weather", DenialKind::Url, "https://evil.example/");
            }
        });

        assert_eq!(
            events.len(),
            1,
            "record's dedup must gate the log too, got: {events:?}"
        );
        let (level, fields) = &events[0];
        assert_eq!(*level, Level::WARN);
        assert_eq!(field(fields, "plugin"), Some("weather"));
        assert_eq!(field(fields, "kind"), Some("Url"));
        assert_eq!(field(fields, "target"), Some("https://evil.example/"));
        assert_eq!(
            field(fields, "message"),
            Some("capability denied; see `rustline plugin denials`")
        );
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
