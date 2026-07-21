//! Platform-specific CPU-utilization read, isolated at the `Context`-build edge.
//!
//! CPU usage is a delta between two cumulative snapshots. The Linux reader takes
//! both across a short sleep; macOS uses `top`'s own internal sample. Mirrors
//! `battery.rs`: the pure parsers compile under `test` on any host and carry the
//! unit tests, while only the file-read / subprocess / sleep is `#[cfg]`-gated.

#[cfg(target_os = "linux")]
use std::time::Duration;

use rustline_core::CpuUsage;

/// Linux two-read sampling window: short enough to keep `render` snappy, long
/// enough to be a stable reading. (macOS uses `top`'s own ~1 s sample instead.)
#[cfg(target_os = "linux")]
const CPU_SAMPLE_WINDOW: Duration = Duration::from_millis(120);

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

#[cfg(target_os = "linux")]
fn read_cpu_linux() -> Option<CpuUsage> {
    let prev = parse_proc_stat(&std::fs::read_to_string("/proc/stat").ok()?)?;
    std::thread::sleep(CPU_SAMPLE_WINDOW);
    let cur = parse_proc_stat(&std::fs::read_to_string("/proc/stat").ok()?)?;
    Some(CpuUsage {
        percent: busy_percent(prev, cur),
    })
}

#[cfg(target_os = "macos")]
fn read_cpu_macos() -> Option<CpuUsage> {
    let output = std::process::Command::new("top")
        .args(["-l", "2", "-n", "0"])
        .output()
        .ok()?;
    let stdout = String::from_utf8(output.stdout).ok()?;
    parse_top_cpu(&stdout).map(|percent| CpuUsage { percent })
}

#[derive(Clone, Copy)]
#[cfg(any(target_os = "linux", test))]
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
#[cfg(any(target_os = "linux", test))]
fn busy_percent(prev: CpuTimes, cur: CpuTimes) -> f32 {
    let dt = cur.total.saturating_sub(prev.total);
    let didle = cur.idle.saturating_sub(prev.idle);
    if dt == 0 {
        return 0.0;
    }
    (dt.saturating_sub(didle) as f32 / dt as f32 * 100.0).clamp(0.0, 100.0)
}

/// Parse `top -l 2 -n 0` stdout: from the **last** `CPU usage:` line take the
/// number before `% idle` and return `100 - idle`. No such line → `None`.
#[cfg(any(target_os = "macos", test))]
fn parse_top_cpu(output: &str) -> Option<f32> {
    let line = output.lines().rfind(|l| l.contains("CPU usage:"))?;
    let idle_str = line
        .split("% idle")
        .next()?
        .rsplit(|c: char| !(c.is_ascii_digit() || c == '.'))
        .next()?;
    let idle: f32 = idle_str.parse().ok()?;
    Some((100.0 - idle).clamp(0.0, 100.0))
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

    #[test]
    fn top_cpu_uses_last_sample() {
        let out = "Processes: 400 total\n\
                   CPU usage: 3.00% user, 2.00% sys, 95.00% idle\n\
                   CPU usage: 12.50% user, 6.25% sys, 81.25% idle\n";
        let p = parse_top_cpu(out).unwrap();
        assert!((p - 18.75).abs() < 0.01); // 100 - 81.25 (the last line)
    }

    #[test]
    fn top_cpu_no_line_is_none() {
        assert!(parse_top_cpu("nothing here").is_none());
    }

    #[test]
    fn read_cpu_never_panics() {
        if let Some(c) = read_cpu() {
            assert!((0.0..=100.0).contains(&c.percent));
        }
    }
}
