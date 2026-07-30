//! Cross-process dedup for the static-misconfiguration warns (see
//! `rustline_core::diag`). Each distinct warn is recorded as a marker file
//! under `<data_root>/.warn-markers/`; the whole directory is cleared
//! whenever the config file's mtime changes, so each misconfiguration is
//! logged once per config edit rather than once per render tick.
//!
//! `create_new(true)` is atomic on every filesystem we support, so concurrent
//! renderers racing the same key produce exactly one winner without a lock.
//!
//! **Fail open, all the way down.** [`should_emit`] treats any marker I/O
//! failure as "emit" (invariant N2's spirit — a broken dedup cache must
//! never silence a diagnostic). [`reset_if_generation_changed`] goes
//! further: it reports whether the reset actually *completed*, and
//! [`install`] refuses to install the hook at all when it didn't. A wedged
//! marker dir — present, non-empty, but unwritable (a read-only remount, a
//! restrictive ACL, or a foreign-owned state dir) — thus degrades the whole
//! process to no-dedup rather than to permanent silence: `create_dir_all` on
//! an already-existing, unwritable directory returns `Ok`, and that `Ok`
//! must never be read as "the reset happened".
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Records the config generation the current markers belong to.
const GENERATION_FILE: &str = ".generation";

/// A sibling of `state/` under `data_root()`, deliberately **not** a child of
/// `state_root()`. Every per-plugin state dir lives at
/// `state_root().join(<plugin-name>)` (`CapabilityCtx::state_dir()`), and a
/// plugin name is an unvalidated `.wasm` file stem — so any single-segment
/// name reachable via `state_root().join(name)` can collide with some
/// plugin's own state dir. (This directory used to be `state_root().join
/// ("warned")`; a plugin literally named `warned` would have shared its
/// state dir with this module, and `reset_if_generation_changed`'s
/// `remove_dir_all` would have wiped it on every config edit. Do not
/// reintroduce a name under `state_root()`.) Living one level up instead —
/// as a sibling of `state/`, not a descendant of it — makes that collision
/// structurally impossible: this path can never equal
/// `state_root().join(<any name>)` for any plugin name, because the two
/// subtrees don't share a parent.
fn marker_dir() -> PathBuf {
    rustline_wasm::data_root().join(".warn-markers")
}

/// A stable, filesystem-safe name for `key`. Not cryptographic — a collision
/// only means one of two warns is suppressed for one config generation.
fn marker_name(key: &str) -> String {
    let mut h = DefaultHasher::new();
    key.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// The config file's mtime as an opaque generation string; an unreadable
/// config, or one whose mtime predates the epoch, yields a constant, which
/// simply means "don't reset". Nanoseconds-since-epoch rather than
/// `SystemTime`'s `Debug` output, which carries no format stability
/// guarantee across toolchains.
fn generation_of(config_path: &Path) -> String {
    let nanos = std::fs::metadata(config_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos());
    match nanos {
        Some(n) => n.to_string(),
        None => "unknown".to_string(),
    }
}

/// Clear every marker when `generation` differs from the recorded one, then
/// record `generation`. Returns whether the markers are now known to match
/// `generation` — either because they already did, or because the clear +
/// re-stamp both actually succeeded. `false` means the directory is wedged
/// (e.g. unwritable but non-empty): `remove_dir_all` couldn't clear stale
/// markers, or the new stamp couldn't be written, so the caller must not
/// trust anything already recorded in it.
#[must_use = "ignoring this treats a reset that silently failed as one that succeeded \
              — a wedged marker dir would then suppress a warn forever instead of degrading to no-dedup"]
fn reset_if_generation_changed(dir: &Path, generation: &str) -> bool {
    let stamp = dir.join(GENERATION_FILE);
    if std::fs::read_to_string(&stamp).ok().as_deref() == Some(generation) {
        return true;
    }
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        // The first-run path: no markers have ever been written, so there's
        // nothing to clear. Any other error means something in `dir`
        // couldn't be removed, so stale markers may survive — that must
        // propagate as `false`, not be swallowed here.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return false,
    }
    std::fs::create_dir_all(dir).is_ok() && std::fs::write(&stamp, generation).is_ok()
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
///
/// Skips installing the hook entirely when the marker dir couldn't be reset
/// for the current generation: a process that can't trust its own dedup
/// state must not dedup at all for this run (fail open), rather than risk
/// treating a stale, unresettable marker as a reason to stay silent.
pub fn install(config_path: &Path) {
    let dir = marker_dir();
    if !reset_if_generation_changed(&dir, &generation_of(config_path)) {
        return;
    }
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
        assert!(reset_if_generation_changed(&marks, "gen-1"));
        assert!(should_emit(&marks, "k"));
        assert!(!should_emit(&marks, "k"));
        assert!(reset_if_generation_changed(&marks, "gen-2"));
        assert!(should_emit(&marks, "k"), "a config edit re-arms the warn");
    }

    #[test]
    fn an_unchanged_generation_keeps_suppression() {
        let dir = tempfile::tempdir().unwrap();
        let marks = dir.path().join("warned");
        assert!(reset_if_generation_changed(&marks, "gen-1"));
        assert!(should_emit(&marks, "k"));
        assert!(reset_if_generation_changed(&marks, "gen-1"));
        assert!(!should_emit(&marks, "k"));
    }

    /// CRITICAL-1 regression: a marker dir that *exists*, is *non-empty*,
    /// but is unwritable — what a read-only remount, a restrictive ACL, or
    /// (per `doctor.rs`'s foreign-owned-state-dir note) a state dir owned by
    /// another user all look like. The dir alone being read-only isn't
    /// enough to reproduce this: overwriting an *existing* file's content
    /// only checks that file's own permission bits, not its parent
    /// directory's, so the stamp file is chmod'd too.
    ///
    /// Before this fix, `reset_if_generation_changed` had no return value:
    /// `create_dir_all(dir).is_ok()` is `true` for an already-existing
    /// directory regardless of whether anything inside it could actually be
    /// changed, so the reset silently "succeeded" while stale markers
    /// (including this test's `k`) stayed in place — wedging `should_emit`
    /// shut for `k` forever, across every future config edit.
    #[test]
    #[cfg(unix)]
    fn a_wedged_marker_dir_reports_reset_failure() {
        use std::os::unix::fs::PermissionsExt;

        // Unix mode bits are meaningless to root (CAP_DAC_OVERRIDE ignores
        // them), so this test would assert something false under root.
        // There's no existing precedent in this crate for a unix-permission
        // test to follow; euid 0 is the standard guard for this shape.
        if unsafe { libc::geteuid() } == 0 {
            eprintln!("skipping: running as root, permission bits aren't enforced");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let marks = dir.path().join("warned");
        assert!(reset_if_generation_changed(&marks, "gen-1"));
        assert!(should_emit(&marks, "k"));
        assert!(
            !should_emit(&marks, "k"),
            "k is marked after the first call"
        );

        let stamp = marks.join(GENERATION_FILE);
        std::fs::set_permissions(&stamp, std::fs::Permissions::from_mode(0o444)).unwrap();
        std::fs::set_permissions(&marks, std::fs::Permissions::from_mode(0o555)).unwrap();

        let reset_ok = reset_if_generation_changed(&marks, "gen-2");
        let new_key_still_emits = should_emit(&marks, "a-key-never-marked-before");

        // Restore before any assertion below can panic, so the tempdir's
        // own cleanup (which unlinks files inside `marks`) doesn't fail.
        std::fs::set_permissions(&marks, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&stamp, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert!(
            !reset_ok,
            "a wedged marker dir must report that the reset did not complete, \
             so `install` can fall back to no-dedup instead of trusting a \
             reset that silently never happened"
        );
        // `should_emit` alone already fails open for a key it has never
        // marked (EACCES on a brand-new file falls into its catch-all
        // `Err(_) => true` branch) — that path was never broken. The bug
        // was entirely in `reset_if_generation_changed` reporting success
        // while `remove_dir_all` silently failed, which the assertion
        // above pins; `install`'s refusal to install the hook when this
        // returns `false` is what keeps an already-marked key like `k`
        // from staying wedged shut (covered end-to-end by
        // `warn_dedup_disables_when_marker_dir_is_wedged` in
        // `tests/smoke.rs`).
        assert!(new_key_still_emits);
    }
}
