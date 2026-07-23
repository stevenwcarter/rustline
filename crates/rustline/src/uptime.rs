//! System-uptime read surface, isolated at the `Context`-build edge.
//!
//! `read_uptime` is a `#[cfg(target_os)]` surface (see the `battery.rs`/
//! `cpu.rs` pattern): each OS arm delegates to a pure parser
//! (`parse_proc_uptime`/`parse_kern_boottime`) that compiles under `test` on
//! any host, so both are unit-tested on the Linux dev box even though only
//! one reader arm compiles per platform.

/// Parse the first (uptime, in seconds) field of `/proc/uptime`'s contents.
/// Truncates the fractional part. Missing/non-numeric content -> `None`.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_proc_uptime(s: &str) -> Option<u64> {
    s.split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .map(|f| f as u64)
}

/// Parse `sysctl -n kern.boottime`'s stdout (`{ sec = N, usec = M }`) into a
/// boot-time unix timestamp, then return `now_epoch - sec`. A boot time in
/// the future (a backward/skewed clock) or unparseable output -> `None`.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn parse_kern_boottime(sysctl_output: &str, now_epoch: u64) -> Option<u64> {
    let after = sysctl_output.split("sec = ").nth(1)?;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    let boot_secs: u64 = digits.parse().ok()?;
    now_epoch.checked_sub(boot_secs)
}

/// Read system uptime in seconds, or `None` if the platform is unsupported or
/// the read failed. Called once at Context-build time.
#[cfg(target_os = "linux")]
pub fn read_uptime() -> Option<u64> {
    parse_proc_uptime(&std::fs::read_to_string("/proc/uptime").ok()?)
}

#[cfg(target_os = "macos")]
pub fn read_uptime() -> Option<u64> {
    let output = std::process::Command::new("sysctl")
        .args(["-n", "kern.boottime"])
        .output()
        .ok()?;
    let stdout = String::from_utf8(output.stdout).ok()?;
    let now_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    parse_kern_boottime(&stdout, now_epoch)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn read_uptime() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_uptime_first_float() {
        assert_eq!(parse_proc_uptime("12345.67 98765.43\n"), Some(12345));
        assert_eq!(parse_proc_uptime(""), None);
        assert_eq!(parse_proc_uptime("garbage"), None);
    }

    #[test]
    fn parses_kern_boottime() {
        let out = "{ sec = 1700000000, usec = 123456 }\n";
        assert_eq!(parse_kern_boottime(out, 1_700_003_600), Some(3600));
    }

    #[test]
    fn kern_boottime_rejects_garbage_and_future_boot() {
        assert!(parse_kern_boottime("garbage", 1_700_000_000).is_none());
        // Boot time after "now": a skewed/backward clock -> None, not a panic
        // or an underflowed huge number.
        let out = "{ sec = 1700000000, usec = 0 }\n";
        assert!(parse_kern_boottime(out, 1_699_999_999).is_none());
    }

    #[test]
    fn read_uptime_never_panics() {
        // Host-dependent value; only assert it does not panic and is sane.
        if let Some(secs) = read_uptime() {
            assert!(secs < u64::MAX);
        }
    }
}
