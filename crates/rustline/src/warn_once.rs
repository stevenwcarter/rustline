//! Cross-process dedup for the static-misconfiguration warns (see
//! `rustline_core::diag`). Each distinct warn is recorded as a marker file
//! under `<state_root>/warned/`; the whole directory is cleared whenever the
//! config file's mtime changes, so each misconfiguration is logged once per
//! config edit rather than once per render tick.
//!
//! `create_new(true)` is atomic on every filesystem we support, so concurrent
//! renderers racing the same key produce exactly one winner without a lock.
//!
//! Best-effort throughout: any I/O failure means "emit" (invariant N2's spirit
//! — a broken dedup cache must never silence a diagnostic).

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

/// Records the config generation the current markers belong to.
const GENERATION_FILE: &str = ".generation";

fn marker_dir() -> PathBuf {
    rustline_wasm::state_root().join("warned")
}

/// A stable, filesystem-safe name for `key`. Not cryptographic — a collision
/// only means one of two warns is suppressed for one config generation.
fn marker_name(key: &str) -> String {
    let mut h = DefaultHasher::new();
    key.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// The config file's mtime as an opaque generation string; an unreadable
/// config yields a constant, which simply means "don't reset".
fn generation_of(config_path: &Path) -> String {
    std::fs::metadata(config_path)
        .and_then(|m| m.modified())
        .map(|t| format!("{t:?}"))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Clear every marker when `generation` differs from the recorded one, then
/// record `generation`. Best-effort: a failure leaves markers in place, which
/// at worst suppresses a warn for one more config edit.
fn reset_if_generation_changed(dir: &Path, generation: &str) {
    let stamp = dir.join(GENERATION_FILE);
    if std::fs::read_to_string(&stamp).ok().as_deref() == Some(generation) {
        return;
    }
    let _ = std::fs::remove_dir_all(dir);
    if std::fs::create_dir_all(dir).is_ok() {
        let _ = std::fs::write(&stamp, generation);
    }
}

/// `true` iff `key` has not been marked in `dir` yet. Claims the marker as a
/// side effect. Any I/O error returns `true` (fail open).
fn should_emit(dir: &Path, key: &str) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return true;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(marker_name(key)))
    {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(_) => true,
    }
}

/// Install the real dedup hook. Call once, from `main`, immediately after
/// `logging::init` — every warn site below this crate routes through it.
pub fn install(config_path: &Path) {
    let dir = marker_dir();
    reset_if_generation_changed(&dir, &generation_of(config_path));
    rustline_core::diag::set_warn_once_hook(Box::new(move |key, emit| {
        if should_emit(&dir, key) {
            emit();
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn first_call_emits_and_second_suppresses() {
        let dir = tempfile::tempdir().unwrap();
        let marks = dir.path().join("warned");
        let hits = AtomicUsize::new(0);
        let bump = || {
            hits.fetch_add(1, Ordering::Relaxed);
        };
        assert!(should_emit(&marks, "k"));
        bump();
        assert!(!should_emit(&marks, "k"));
        assert_eq!(hits.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn distinct_keys_both_emit() {
        let dir = tempfile::tempdir().unwrap();
        let marks = dir.path().join("warned");
        assert!(should_emit(&marks, "a"));
        assert!(should_emit(&marks, "b"));
    }

    #[test]
    fn an_unwritable_marker_dir_fails_open() {
        // A regular file where the marker directory should be makes every
        // marker write fail; a dedup layer must never silence diagnostics.
        let blocker = tempfile::NamedTempFile::new().unwrap();
        let marks = blocker.path().join("warned");
        assert!(should_emit(&marks, "k"));
        assert!(should_emit(&marks, "k"), "still emits when markers fail");
    }

    #[test]
    fn a_changed_generation_rearms_every_key() {
        let dir = tempfile::tempdir().unwrap();
        let marks = dir.path().join("warned");
        reset_if_generation_changed(&marks, "gen-1");
        assert!(should_emit(&marks, "k"));
        assert!(!should_emit(&marks, "k"));
        reset_if_generation_changed(&marks, "gen-2");
        assert!(should_emit(&marks, "k"), "a config edit re-arms the warn");
    }

    #[test]
    fn an_unchanged_generation_keeps_suppression() {
        let dir = tempfile::tempdir().unwrap();
        let marks = dir.path().join("warned");
        reset_if_generation_changed(&marks, "gen-1");
        assert!(should_emit(&marks, "k"));
        reset_if_generation_changed(&marks, "gen-1");
        assert!(!should_emit(&marks, "k"));
    }
}
