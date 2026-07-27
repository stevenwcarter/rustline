//! The one atomic file-write primitive.
//!
//! Every persistence site in this workspace stages into a temp file and
//! renames onto the target, because rename-within-a-directory is atomic: a
//! concurrent reader sees either the old contents or the new ones, never a
//! partial write.
//!
//! The staging name must be unique **per writer**, not just per target. tmux
//! runs `render left` and `render right` as separate processes and spawns a
//! job per client/session, so two rustline processes routinely stage the same
//! file in the same tick. A shared `<name>.tmp` gives atomicity against a
//! crash and none at all against a concurrent writer: `fs::write` is
//! create+O_TRUNC+write, so B truncates the temp file A already filled and A
//! renames a torn file into place. The parsers downstream are all total, so
//! the corruption is silent — a lost CPU sample costs a 120 ms sleep on the
//! next render, a lost throughput delta renders nothing, a `{spark}` ring
//! visibly drops samples.
//!
//! Uniquely-named staging carries three trade-offs, all accepted:
//!
//! - **Crash leftovers accumulate.** The old shared `.tmp` left at most one
//!   stale file per target and reused it next tick — self-limiting. A unique
//!   name per call leaves one orphan per process killed between its write and
//!   its rename, and nothing reaps them. `rustline-wasm`'s cache namespaces
//!   sweep them in `evict_namespace` (an unparseable file sorts oldest, so it
//!   evicts first), but a plugin's state-dir root and
//!   `~/.local/state/rustline/` have no such sweep, and `state::dir_size`
//!   counts an orphan against `max_state_bytes` forever.
//! - **`NAME_MAX`.** The ~33-char staging suffix can push an already-long
//!   target name over the filesystem's name-length limit where a plain
//!   `fs::write` would have succeeded (measured: a 220-char filename writes
//!   fine, 240 fails with `File name too long`, os error 36). Only reachable
//!   through `perform_state_write`, since `sanitize_relpath` imposes no
//!   length bound on a plugin-supplied relpath; the other five call sites all
//!   write bounded names. Degrades to an error string, never a panic.
//! - **Transient double-count against the quota.** `check_cap` projects a
//!   write as an in-place replacement, but `write_atomic` briefly holds both
//!   the old file and the staging file, so peak on-disk usage can exceed the
//!   accounted projection by the replaced file's size. It can't cause the
//!   write that triggers it to be rejected, but a *concurrent* process's
//!   `check_cap` can observe the inflated total and refuse a legitimate
//!   write of its own. New with this change — the sites this replaced used
//!   to truncate in place.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Per-process, monotonically increasing tiebreaker appended to the staging
/// name alongside the wall-clock nanos. Two calls landing in the same
/// nanosecond (a coarse clock, or just bad luck) would otherwise share a
/// staging path and reintroduce the exact collision this module exists to
/// prevent; the counter makes every call from this process unique regardless
/// of clock resolution.
static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A staging path next to `path`, unique to this process and this call:
/// `<path>.<pid>.<nanos>.<counter>.tmp`. Same directory as the target, so the
/// rename that follows is atomic.
fn staging_path(path: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{}.{nanos}.{seq}.tmp", std::process::id()));
    PathBuf::from(name)
}

/// Write `contents` to `path` atomically: stage into a per-writer temp file in
/// the same directory, then rename onto the target. Last writer wins on the
/// final path, which is the intended semantics for every caller here. The temp
/// file is removed on the error path so a failed write leaves no litter.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let tmp = staging_path(path);
    if let Err(e) = std::fs::write(&tmp, contents) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        write_atomic(&p, b"hello").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello");
    }

    #[test]
    fn a_successful_write_leaves_no_temp_behind() {
        let dir = tempfile::tempdir().unwrap();
        write_atomic(&dir.path().join("f"), b"hello").unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
    }

    #[test]
    fn staging_names_are_unique_per_call() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        let a = staging_path(&p);
        let b = staging_path(&p);
        assert_ne!(a, b, "two writers must not share a staging path");
        assert!(
            a.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(&std::process::id().to_string()),
            "staging name carries the pid"
        );
        assert_eq!(
            a.parent(),
            p.parent(),
            "staged in the same dir, so rename is atomic"
        );
    }

    #[test]
    fn overwrites_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        write_atomic(&p, b"old").unwrap();
        write_atomic(&p, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
    }

    /// Four writers race on the same target while a reader polls
    /// concurrently. Checking only the *final* content (after every writer
    /// has finished) isn't load-bearing: with a shared staging name, a
    /// truncate that lands mid-race can leave the target briefly torn or
    /// empty, but whichever writer runs last always leaves a clean file
    /// behind, healing the damage before anyone reads it — so a version of
    /// this test that reads once at the end would pass even against the old
    /// `path.with_extension("tmp")` bug (verified empirically: the read-at-
    /// end shape survived tens of thousands of racing iterations against
    /// that bug without ever observing corruption). A reader sampling *while
    /// the race is still running* is what actually catches it, matching the
    /// real failure mode: a concurrent render process reading the file
    /// between two others' writes.
    ///
    /// Needs at least 2 CPU cores to actually exercise the race: measured
    /// against the shared-staging-name bug, a single-core runner never
    /// preempts a writer mid-truncate, so this passes 10/10 there without
    /// detecting anything (2 cores: 9/10 catch it; 4 cores: 10/10). It never
    /// flakes against the fixed implementation at any core count, so a green
    /// result here is only meaningful coverage on a multi-core runner.
    #[test]
    fn concurrent_writers_always_leave_a_complete_file() {
        use std::sync::atomic::AtomicBool;

        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        let bodies: Vec<String> = (*b"abcd")
            .into_iter()
            .map(|c| (c as char).to_string().repeat(4096))
            .collect();
        write_atomic(&p, bodies[0].as_bytes()).unwrap();

        let stop = AtomicBool::new(false);
        let torn = AtomicBool::new(false);
        std::thread::scope(|s| {
            let writers: Vec<_> = bodies
                .iter()
                .map(|body| {
                    s.spawn(|| {
                        for _ in 0..3000 {
                            let _ = write_atomic(&p, body.as_bytes());
                        }
                    })
                })
                .collect();
            s.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    if let Ok(got) = std::fs::read_to_string(&p)
                        && !bodies.contains(&got)
                    {
                        torn.store(true, Ordering::Relaxed);
                    }
                }
            });
            for writer in writers {
                writer.join().unwrap();
            }
            stop.store(true, Ordering::Relaxed);
        });

        assert!(
            !torn.load(Ordering::Relaxed),
            "a concurrent reader must never see a torn or empty file"
        );
        let got = std::fs::read_to_string(&p).unwrap();
        assert!(
            bodies.contains(&got),
            "the file must settle on one writer's complete content"
        );
    }

    #[test]
    fn a_write_error_leaves_no_temp_behind() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope").join("f");
        assert!(write_atomic(&missing, b"x").is_err());
        assert!(!dir.path().join("nope").exists());
    }

    /// The staging file is written successfully (so it genuinely exists) and
    /// only the *rename* fails, unlike `a_write_error_leaves_no_temp_behind`
    /// where the write itself fails before any temp file is created. This is
    /// the case that actually exercises `write_atomic`'s second cleanup call.
    #[test]
    fn a_rename_failure_leaves_no_temp_behind() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        // A directory at the target path: `fs::write` to the staging file
        // succeeds, but renaming a regular file onto an existing directory
        // always fails.
        std::fs::create_dir(&p).unwrap();
        assert!(write_atomic(&p, b"x").is_err());
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left after a rename failure: {leftovers:?}"
        );
    }
}
