//! Platform-specific CPU-utilization read, isolated at the `Context`-build edge.
//!
//! CPU usage is a delta between two cumulative snapshots. The Linux reader
//! prefers a *cross-invocation* delta: it diffs the current `/proc/stat`
//! against a persisted prior snapshot (zero sleep) when that snapshot is fresh,
//! and only falls back to taking both samples across a short sleep on the first
//! run or a stale snapshot (see `CPU_SNAPSHOT_STALENESS_SECS`). macOS reads the
//! same cumulative counters from the mach kernel (`host_statistics`) and shares
//! that identical snapshot-delta path. Mirrors `battery.rs`: the pure parsers compile
//! under `test` on any host and carry the unit tests, while only the file-read /
//! subprocess / sleep / snapshot-persistence is `#[cfg]`-gated.

use std::path::Path;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::path::PathBuf;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rustline_core::CpuUsage;

/// Two-read sampling window for the cold/stale-snapshot fallback: short enough
/// to keep `render` snappy, long enough to be a stable reading. Used by both
/// the Linux (`/proc/stat`) and macOS (`host_statistics`) readers.
#[cfg(any(target_os = "linux", target_os = "macos"))]
const CPU_SAMPLE_WINDOW: Duration = Duration::from_millis(120);

/// Upper bound (seconds) on how old the persisted `/proc/stat` snapshot may be
/// and still be used for a zero-sleep delta read. The reader can't see tmux's
/// `status-interval`, so this is a fixed conservative bound: within it, the
/// prior snapshot yields a busy% averaged over the real elapsed wall-clock
/// (coarser than the 120 ms window but a fine steady-state reading); past it,
/// the average would span too long to represent "current" load (or the box was
/// idle/suspended), so `read_cpu` re-primes via the classic two-sample path.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
const CPU_SNAPSHOT_STALENESS_SECS: u64 = 60;

/// Read CPU utilization, or `None` if the platform is unsupported or the read
/// failed. Called once at Context-build time.
pub fn read_cpu() -> Option<CpuUsage> {
    #[cfg(target_os = "linux")]
    {
        read_cpu_linux()
    }
    #[cfg(target_os = "macos")]
    {
        read_cpu_macos()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// State-file name (under `sample_store`'s state dir) the persisted cpu%
/// `{spark}` history ring is kept at — distinct from `SAMPLE_NAME` above
/// (the linux-only two-field delta snapshot). This one is platform-agnostic:
/// it just needs a completed [`CpuUsage`] reading, not a platform-specific
/// delta, so it's written/read the same way on every OS.
const HISTORY_SAMPLE_NAME: &str = "cpu-history";

/// Push `current_percent` onto the persisted cpu% history ring, truncate to
/// the last `spark_width` readings, persist it back, and return the
/// resulting history (oldest first) for `Context.cpu_history`. Called only
/// when the `cpu` widget's `format` actually references `{spark}` (see
/// `build_context.rs`) — an unreferenced history costs a read+write for
/// nothing, keeping `{spark}`-absent output byte-identical (no I/O at all).
pub fn read_cpu_history(state_dir: &Path, current_percent: f32, spark_width: usize) -> Vec<f32> {
    let mut history = crate::sample_store::read_sample(state_dir, HISTORY_SAMPLE_NAME)
        .as_deref()
        .map(crate::history::parse_history)
        .unwrap_or_default();
    crate::history::push_truncate(&mut history, current_percent, spark_width);
    crate::sample_store::write_sample(
        state_dir,
        HISTORY_SAMPLE_NAME,
        &crate::history::serialize_history(&history),
    );
    history
}

#[cfg(target_os = "linux")]
fn read_cpu_linux() -> Option<CpuUsage> {
    read_cpu_linux_in(&rustline_wasm::state_root())
}

/// [`read_cpu_linux`]'s body, parameterized on the snapshot-cache directory so
/// tests can inject a `tempfile::tempdir()` instead of touching the real XDG
/// state dir. Production always calls this via `read_cpu_linux`'s
/// `state_root()` default.
#[cfg(target_os = "linux")]
fn read_cpu_linux_in(state_dir: &Path) -> Option<CpuUsage> {
    let cur_times = parse_proc_stat(&std::fs::read_to_string("/proc/stat").ok()?)?;
    let current = CpuSnapshot {
        idle: cur_times.idle,
        total: cur_times.total,
        ts: now_unix_secs(),
    };

    // Fast path (default): a fresh persisted snapshot gives busy% with no sleep.
    // Fallback: no recent prior -> take the second sample the classic way.
    let percent = match load_snapshot(state_dir)
        .and_then(|prev| busy_from_snapshots(&prev, &current, CPU_SNAPSHOT_STALENESS_SECS))
    {
        Some(p) => p,
        None => {
            std::thread::sleep(CPU_SAMPLE_WINDOW);
            let second = parse_proc_stat(&std::fs::read_to_string("/proc/stat").ok()?)?;
            busy_percent(cur_times, second)
        }
    };

    // Always persist the current reading so the next invocation can take the
    // fast path. Best-effort: a write failure just warns and is ignored.
    store_snapshot(state_dir, &current);
    Some(CpuUsage { percent })
}

/// One `/proc/stat` aggregate reading plus the wall-clock instant it was taken,
/// persisted across invocations so the next `read_cpu` can compute a delta
/// without the sampling sleep. Also the sample history a future sparkline can
/// consume.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq)]
struct CpuSnapshot {
    idle: u64,
    total: u64,
    /// Unix timestamp (seconds) at which the counters were read.
    ts: u64,
}

/// Busy % between a persisted prior snapshot and the current one, or `None`
/// when the prior can't yield a trustworthy delta (so the caller falls back to
/// a fresh two-sample read). `None` when: age (`now.ts - prev.ts`) is `<= 0`
/// (same instant or a backward clock) or `> staleness_secs`, or the total-jiffy
/// delta is `<= 0` (idle interval or backward counters after suspend/resume).
/// Otherwise the same busy% formula as [`busy_percent`], clamped `0..=100`.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn busy_from_snapshots(prev: &CpuSnapshot, now: &CpuSnapshot, staleness_secs: u64) -> Option<f32> {
    let age = now.ts.checked_sub(prev.ts)?;
    if age == 0 || age > staleness_secs {
        return None;
    }
    let dt = now.total.checked_sub(prev.total).filter(|&d| d > 0)?;
    let didle = now.idle.saturating_sub(prev.idle);
    Some((dt.saturating_sub(didle) as f32 / dt as f32 * 100.0).clamp(0.0, 100.0))
}

/// Serialize a snapshot to a single `total idle ts` line (trailing newline),
/// the same plain-text, total-on-parse-failure discipline as the toggles file.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn serialize_snapshot(snap: &CpuSnapshot) -> String {
    format!("{} {} {}\n", snap.total, snap.idle, snap.ts)
}

/// Parse a `total idle ts` line back into a snapshot. Total: any
/// missing/short/non-numeric content yields `None` (treated as absent →
/// fallback), never a panic. Extra trailing tokens are ignored.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn parse_snapshot(text: &str) -> Option<CpuSnapshot> {
    let mut fields = text.lines().next()?.split_whitespace();
    let total = fields.next()?.parse().ok()?;
    let idle = fields.next()?.parse().ok()?;
    let ts = fields.next()?.parse().ok()?;
    Some(CpuSnapshot { idle, total, ts })
}

/// State-file path for the persisted snapshot: `<state_dir>/cpu-sample`.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn snapshot_path(state_dir: &Path) -> PathBuf {
    state_dir.join("cpu-sample")
}

/// Current wall clock as unix seconds; a pre-epoch clock degrades to `0`
/// (which makes any prior snapshot read as backward-clock → fallback).
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Load the persisted snapshot; a missing/unreadable/corrupt file → `None`
/// (treated as absent, so the caller falls back to the two-sample read).
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn load_snapshot(state_dir: &Path) -> Option<CpuSnapshot> {
    parse_snapshot(&std::fs::read_to_string(snapshot_path(state_dir)).ok()?)
}

/// Best-effort atomic persist (temp file + rename); logs a warning on failure
/// and never panics — a broken cache must never break the bar.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn store_snapshot(state_dir: &Path, snap: &CpuSnapshot) {
    let path = snapshot_path(state_dir);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("tmp");
    if let Err(error) = std::fs::write(&tmp, serialize_snapshot(snap)) {
        tracing::warn!(%error, "failed to write cpu-sample temp file");
        return;
    }
    if let Err(error) = std::fs::rename(&tmp, &path) {
        tracing::warn!(%error, "failed to rename cpu-sample file");
    }
}

#[cfg(target_os = "macos")]
fn read_cpu_macos() -> Option<CpuUsage> {
    read_cpu_macos_in(&rustline_wasm::state_root())
}

/// [`read_cpu_macos`]'s body, parameterized on the snapshot-cache dir so tests
/// can inject a `tempfile::tempdir()` (the same seam as [`read_cpu_linux_in`]).
///
/// Reads cumulative CPU ticks from the mach kernel
/// (`host_statistics(HOST_CPU_LOAD_INFO)`) and takes the exact same zero-sleep
/// cross-invocation delta as the Linux reader, falling back to a short
/// two-sample read only on a cold/stale snapshot. This replaces the old
/// `top -l 2 -n 0` shell-out, which sleeps ~1 s internally and made every
/// `render right` a >1 s call — under the daemon's shared render lock that
/// stalled the whole refresh burst and dropped queued clients with EPIPE.
#[cfg(target_os = "macos")]
fn read_cpu_macos_in(state_dir: &Path) -> Option<CpuUsage> {
    let cur_times = read_mach_cpu_ticks()?;
    let current = CpuSnapshot {
        idle: cur_times.idle,
        total: cur_times.total,
        ts: now_unix_secs(),
    };

    // Fast path (default): a fresh persisted snapshot gives busy% with no sleep.
    // Fallback: no recent prior -> take the second sample the classic way.
    let percent = match load_snapshot(state_dir)
        .and_then(|prev| busy_from_snapshots(&prev, &current, CPU_SNAPSHOT_STALENESS_SECS))
    {
        Some(p) => p,
        None => {
            std::thread::sleep(CPU_SAMPLE_WINDOW);
            let second = read_mach_cpu_ticks()?;
            busy_percent(cur_times, second)
        }
    };

    // Always persist so the next invocation can take the fast path. Best-effort.
    store_snapshot(state_dir, &current);
    Some(CpuUsage { percent })
}

/// Cumulative CPU ticks since boot from the mach kernel, in the same
/// `(idle, total)` shape Linux derives from `/proc/stat`: `total` is the sum of
/// the user/system/idle/nice tick counters, `idle` the idle counter alone.
/// `None` if the `host_statistics` call fails. The only `unsafe` in this file's
/// macOS path, isolated here.
///
/// `libc` deprecates its mach bindings in favor of the `mach2` crate, but these
/// symbols remain present and functional; we keep the dependency graph minimal
/// (rustls-only, no new crates — see the `cargo tree` invariants) and scope the
/// `allow(deprecated)` to this one FFI shim.
#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn read_mach_cpu_ticks() -> Option<CpuTimes> {
    use std::sync::OnceLock;

    // `mach_host_self()` hands back a send right and bumps a user-reference
    // count on the host port every call. Cache it once so the long-running
    // daemon (which calls this once per `render right`) doesn't accrue a uref
    // per read — one stable right for the process, released at exit.
    // SAFETY: `mach_host_self` has no preconditions and returns the caller's
    // host name port; caching it for the process lifetime is the standard idiom.
    static HOST: OnceLock<libc::mach_port_t> = OnceLock::new();
    let host = *HOST.get_or_init(|| unsafe { libc::mach_host_self() });

    // SAFETY: on `KERN_SUCCESS`, `host_statistics` fills `info` with
    // `HOST_CPU_LOAD_INFO_COUNT` (4) `natural_t` counters — exactly the length
    // of `host_cpu_load_info`'s `cpu_ticks` array — so the struct is fully
    // initialized before we read it. We read it only on success.
    unsafe {
        let mut info = std::mem::MaybeUninit::<libc::host_cpu_load_info>::uninit();
        let mut count: libc::mach_msg_type_number_t = libc::HOST_CPU_LOAD_INFO_COUNT;
        let kr = libc::host_statistics(
            host,
            libc::HOST_CPU_LOAD_INFO,
            info.as_mut_ptr() as libc::host_info_t,
            &mut count,
        );
        if kr != libc::KERN_SUCCESS {
            return None;
        }
        let ticks = info.assume_init().cpu_ticks;
        let user = u64::from(ticks[libc::CPU_STATE_USER as usize]);
        let system = u64::from(ticks[libc::CPU_STATE_SYSTEM as usize]);
        let idle = u64::from(ticks[libc::CPU_STATE_IDLE as usize]);
        let nice = u64::from(ticks[libc::CPU_STATE_NICE as usize]);
        Some(CpuTimes {
            idle,
            total: user + system + idle + nice,
        })
    }
}

#[derive(Clone, Copy)]
#[cfg(any(target_os = "linux", target_os = "macos", test))]
struct CpuTimes {
    idle: u64,
    total: u64,
}

/// Parse the aggregate `cpu ` line of `/proc/stat` into `(idle+iowait,
/// sum-of-all-fields)`. Ignores the per-core `cpuN` lines. Missing/unparseable
/// → `None`.
#[cfg(any(target_os = "linux", test))]
fn parse_proc_stat(text: &str) -> Option<CpuTimes> {
    let line = text
        .lines()
        .find(|l| l.split_whitespace().next() == Some("cpu"))?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .map(|f| f.parse::<u64>().ok())
        .collect::<Option<Vec<u64>>>()?;
    if fields.len() < 4 {
        return None;
    }
    let total: u64 = fields.iter().sum();
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
    Some(CpuTimes { idle, total })
}

/// Busy % over the interval between two cumulative snapshots. `dt == 0` or
/// backward counters (suspend/resume) → `0.0`, never negative or `NaN`.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn busy_percent(prev: CpuTimes, cur: CpuTimes) -> f32 {
    let dt = cur.total.saturating_sub(prev.total);
    let didle = cur.idle.saturating_sub(prev.idle);
    if dt == 0 {
        return 0.0;
    }
    (dt.saturating_sub(didle) as f32 / dt as f32 * 100.0).clamp(0.0, 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_stat_parses_aggregate_line() {
        let s = "cpu  100 0 100 700 100 0 0 0 0 0\ncpu0 50 0 50 350 50 0 0 0 0 0\n";
        let t = parse_proc_stat(s).unwrap();
        assert_eq!(t.idle, 800); // idle(700) + iowait(100)
        assert_eq!(t.total, 1000); // sum of all ten fields
    }

    #[test]
    fn proc_stat_no_cpu_line_is_none() {
        assert!(parse_proc_stat("intr 1 2 3\n").is_none());
    }

    #[test]
    fn proc_stat_rejects_too_few_and_nonnumeric_fields() {
        assert!(parse_proc_stat("cpu 1 2\n").is_none()); // < 4 fields
        assert!(parse_proc_stat("cpu 1 x 3 4\n").is_none()); // non-numeric field
    }

    #[test]
    fn busy_percent_over_interval() {
        let prev = CpuTimes {
            idle: 800,
            total: 1000,
        };
        let cur = CpuTimes {
            idle: 1000,
            total: 1400,
        };
        assert_eq!(busy_percent(prev, cur), 50.0); // dt=400, didle=200
    }

    #[test]
    fn busy_percent_zero_and_backward() {
        let x = CpuTimes { idle: 5, total: 10 };
        assert_eq!(busy_percent(x, x), 0.0); // dt == 0
        let hi = CpuTimes {
            idle: 1000,
            total: 2000,
        };
        let lo = CpuTimes { idle: 0, total: 0 };
        assert_eq!(busy_percent(hi, lo), 0.0); // backward -> saturates, no NaN
    }

    #[test]
    fn busy_percent_backward_idle_saturates() {
        // dt > 0 but idle went backwards: didle saturates to 0 -> 100% busy, finite (no NaN).
        let prev = CpuTimes {
            idle: 500,
            total: 1000,
        };
        let cur = CpuTimes {
            idle: 100,
            total: 2000,
        };
        let b = busy_percent(prev, cur);
        assert_eq!(b, 100.0); // dt=1000, didle=saturating_sub(100,500)=0 -> (1000-0)/1000*100
        assert!(b.is_finite());
    }

    // Linux and macOS both take the state-dir-injecting path so this never
    // touches the real XDG state dir (`read_cpu()` itself always writes to the
    // real one — see `read_cpu_linux_in`/`read_cpu_macos_in`). Other platforms
    // don't persist any snapshot, so calling `read_cpu()` directly is hermetic.
    #[test]
    #[cfg(target_os = "linux")]
    fn read_cpu_never_panics() {
        let dir = tempfile::tempdir().unwrap();
        if let Some(c) = read_cpu_linux_in(dir.path()) {
            assert!((0.0..=100.0).contains(&c.percent));
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn read_cpu_never_panics() {
        let dir = tempfile::tempdir().unwrap();
        if let Some(c) = read_cpu_macos_in(dir.path()) {
            assert!((0.0..=100.0).contains(&c.percent));
        }
    }

    // The regression guard for the daemon EPIPE storm: with a fresh persisted
    // snapshot, the macOS reader must take the zero-sleep delta path — no
    // `top -l 2` (~1.4 s) and no ~120 ms two-sample fallback — so `render right`
    // stops pinning the daemon's shared render lock for over a second.
    #[test]
    #[cfg(target_os = "macos")]
    fn read_cpu_macos_warm_path_takes_no_sleep() {
        let dir = tempfile::tempdir().unwrap();
        // Seed a fresh prior snapshot a couple seconds in the past with tiny
        // cumulative counters, so the current (huge, since-boot) mach reading is
        // strictly larger: age is within staleness and Δtotal > 0, so the delta
        // path is taken rather than the sleeping fallback.
        let prev = CpuSnapshot {
            total: 1,
            idle: 1,
            ts: now_unix_secs().saturating_sub(2),
        };
        store_snapshot(dir.path(), &prev);
        let start = std::time::Instant::now();
        let c = read_cpu_macos_in(dir.path()).expect("mach cpu read");
        let elapsed = start.elapsed();
        assert!((0.0..=100.0).contains(&c.percent));
        // Comfortably under the 120 ms fallback window: proves no sleep occurred.
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "warm path took {elapsed:?}; expected the no-sleep delta path"
        );
    }

    #[test]
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn read_cpu_never_panics() {
        if let Some(c) = read_cpu() {
            assert!((0.0..=100.0).contains(&c.percent));
        }
    }

    fn snap(total: u64, idle: u64, ts: u64) -> CpuSnapshot {
        CpuSnapshot { total, idle, ts }
    }

    #[test]
    fn busy_from_snapshots_computes_delta_percent() {
        // Δtotal=1000, Δidle=800 -> (1000-800)/1000*100 = 20%.
        let prev = snap(1000, 800, 100);
        let now = snap(2000, 1600, 105);
        let b = busy_from_snapshots(&prev, &now, CPU_SNAPSHOT_STALENESS_SECS).unwrap();
        assert!((b - 20.0).abs() < 0.01, "got {b}");
    }

    #[test]
    fn busy_from_snapshots_respects_staleness_boundary() {
        let prev = snap(1000, 500, 0);
        // age == staleness is still fresh (inclusive upper bound).
        let at_bound = snap(2000, 1000, CPU_SNAPSHOT_STALENESS_SECS);
        assert!(busy_from_snapshots(&prev, &at_bound, CPU_SNAPSHOT_STALENESS_SECS).is_some());
        // age one past the bound -> stale -> None (caller falls back to two-sample).
        let past_bound = snap(2000, 1000, CPU_SNAPSHOT_STALENESS_SECS + 1);
        assert!(busy_from_snapshots(&prev, &past_bound, CPU_SNAPSHOT_STALENESS_SECS).is_none());
    }

    #[test]
    fn busy_from_snapshots_zero_or_backward_age_is_none() {
        let prev = snap(1000, 500, 200);
        // Same instant: age == 0 -> None.
        assert!(
            busy_from_snapshots(&prev, &snap(2000, 1000, 200), CPU_SNAPSHOT_STALENESS_SECS)
                .is_none()
        );
        // Clock went backwards: now.ts < prev.ts -> None.
        assert!(
            busy_from_snapshots(&prev, &snap(2000, 1000, 199), CPU_SNAPSHOT_STALENESS_SECS)
                .is_none()
        );
    }

    #[test]
    fn busy_from_snapshots_zero_or_backward_total_is_none() {
        let prev = snap(1000, 500, 100);
        // Δtotal == 0 -> None (nothing to divide by; fall back).
        assert!(
            busy_from_snapshots(&prev, &snap(1000, 500, 105), CPU_SNAPSHOT_STALENESS_SECS)
                .is_none()
        );
        // Counters went backwards (suspend/resume) -> None.
        assert!(
            busy_from_snapshots(&prev, &snap(400, 200, 105), CPU_SNAPSHOT_STALENESS_SECS).is_none()
        );
    }

    #[test]
    fn busy_from_snapshots_backward_idle_saturates() {
        // dt > 0 but idle went backwards: didle saturates to 0 -> 100% busy, finite (no NaN).
        let prev = snap(1000, 500, 100);
        let now = snap(2000, 100, 105);
        let b = busy_from_snapshots(&prev, &now, CPU_SNAPSHOT_STALENESS_SECS).unwrap();
        assert_eq!(b, 100.0); // dt=1000, didle=saturating_sub(100,500)=0 -> (1000-0)/1000*100
        assert!(b.is_finite());
    }

    #[test]
    fn snapshot_serialize_parse_round_trips() {
        let s = snap(123_456, 78_900, 1_700_000_000);
        assert_eq!(parse_snapshot(&serialize_snapshot(&s)), Some(s));
    }

    #[test]
    fn parse_snapshot_is_total_over_corrupt_input() {
        // Missing, empty, truncated, non-numeric, and garbage all yield None
        // (absent -> fallback), never a panic. Mirrors toggles' total-read.
        for bad in [
            "",
            "   ",
            "\n",
            "1000",
            "1000 800",
            "x y z",
            "1000 800 abc",
            "garbage line",
        ] {
            assert!(parse_snapshot(bad).is_none(), "expected None for {bad:?}");
        }
        // Extra trailing junk on the line is ignored, not fatal.
        assert_eq!(
            parse_snapshot("1000 800 42 leftover\n"),
            Some(snap(1000, 800, 42))
        );
    }

    #[test]
    fn read_cpu_history_appends_and_persists_across_calls() {
        let dir = tempfile::tempdir().unwrap();
        let first = read_cpu_history(dir.path(), 10.0, 8);
        assert_eq!(first, vec![10.0]);
        let second = read_cpu_history(dir.path(), 20.0, 8);
        assert_eq!(second, vec![10.0, 20.0]);
    }

    #[test]
    fn read_cpu_history_truncates_to_spark_width() {
        let dir = tempfile::tempdir().unwrap();
        for v in [1.0, 2.0, 3.0, 4.0] {
            read_cpu_history(dir.path(), v, 2);
        }
        // Only the most recent 2 readings survive the ring.
        assert_eq!(read_cpu_history(dir.path(), 5.0, 2), vec![4.0, 5.0]);
    }
}
