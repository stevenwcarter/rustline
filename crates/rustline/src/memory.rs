//! Platform-specific memory read, isolated at the `Context`-build edge.
//!
//! Mirrors `battery.rs`: the `#[cfg(target_os)]` readers do the I/O; the pure
//! parsers compile under `test` on any host, so both are unit-tested on the
//! Linux dev box even though only one reader arm compiles per platform.

use rustline_core::MemInfo;

/// Read host memory, or `None` if the platform is unsupported or the read
/// failed. Called once at Context-build time.
pub fn read_memory() -> Option<MemInfo> {
    #[cfg(target_os = "linux")]
    {
        read_memory_linux()
    }
    #[cfg(target_os = "macos")]
    {
        read_memory_macos()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
fn read_memory_linux() -> Option<MemInfo> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_meminfo(&text)
}

#[cfg(target_os = "macos")]
fn read_memory_macos() -> Option<MemInfo> {
    let memsize = std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    let vm = std::process::Command::new("vm_stat").output().ok()?;
    let memsize = String::from_utf8(memsize.stdout).ok()?;
    let vm = String::from_utf8(vm.stdout).ok()?;
    parse_macos_memory(&memsize, &vm)
}

/// Parse `/proc/meminfo`. Needs `MemTotal` + `MemAvailable` (both kB);
/// missing either → `None`. `MemAvailable` has existed since Linux 3.14.
#[cfg(any(target_os = "linux", test))]
fn parse_meminfo(text: &str) -> Option<MemInfo> {
    fn field_kb(text: &str, key: &str) -> Option<u64> {
        let rest = text.lines().find_map(|l| l.strip_prefix(key))?;
        rest.split_whitespace().next()?.parse::<u64>().ok()
    }
    let total_bytes = field_kb(text, "MemTotal:")?.saturating_mul(1024);
    let available_bytes = field_kb(text, "MemAvailable:")?.saturating_mul(1024);
    Some(MemInfo {
        total_bytes,
        used_bytes: total_bytes.saturating_sub(available_bytes),
        available_bytes,
    })
}

/// Parse (`hw.memsize` stdout, `vm_stat` stdout). `available ≈ (free + inactive
/// + speculative) * page_size`; `used = total - available`. Missing total or
/// page size → `None`.
#[cfg(any(target_os = "macos", test))]
fn parse_macos_memory(memsize: &str, vm_stat: &str) -> Option<MemInfo> {
    let total_bytes = memsize.trim().parse::<u64>().ok()?;
    let page_size = vm_stat
        .lines()
        .next()?
        .split("page size of")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    fn pages(vm_stat: &str, key: &str) -> u64 {
        vm_stat
            .lines()
            .find_map(|l| l.trim().strip_prefix(key))
            .and_then(|rest| rest.trim().trim_end_matches('.').parse::<u64>().ok())
            .unwrap_or(0)
    }
    let free = pages(vm_stat, "Pages free:");
    let inactive = pages(vm_stat, "Pages inactive:");
    let speculative = pages(vm_stat, "Pages speculative:");
    let available_bytes = (free + inactive + speculative).saturating_mul(page_size);
    Some(MemInfo {
        total_bytes,
        used_bytes: total_bytes.saturating_sub(available_bytes),
        available_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_meminfo_parses_total_avail_used() {
        let text = "MemTotal:       16077216 kB\n\
                    MemFree:         1048576 kB\n\
                    MemAvailable:    9800000 kB\n\
                    Buffers:          200000 kB\n";
        let m = parse_meminfo(text).unwrap();
        assert_eq!(m.total_bytes, 16_077_216 * 1024);
        assert_eq!(m.available_bytes, 9_800_000 * 1024);
        assert_eq!(m.used_bytes, (16_077_216 - 9_800_000) * 1024);
    }

    #[test]
    fn linux_meminfo_missing_available_is_none() {
        assert!(parse_meminfo("MemTotal: 100 kB\n").is_none());
    }

    #[test]
    fn macos_memory_parses_from_sysctl_and_vm_stat() {
        let memsize = "17179869184\n";
        let vm = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
                  Pages free:                          100000.\n\
                  Pages active:                        200000.\n\
                  Pages inactive:                       50000.\n\
                  Pages speculative:                    10000.\n\
                  Pages wired down:                     80000.\n";
        let m = parse_macos_memory(memsize, vm).unwrap();
        assert_eq!(m.total_bytes, 17_179_869_184);
        assert_eq!(m.available_bytes, (100_000 + 50_000 + 10_000) * 16384);
        assert_eq!(
            m.used_bytes,
            17_179_869_184 - (100_000 + 50_000 + 10_000) * 16384
        );
    }

    #[test]
    fn macos_memory_missing_total_is_none() {
        assert!(parse_macos_memory("nope", "(page size of 4096 bytes)\n").is_none());
    }

    #[test]
    fn macos_memory_missing_page_size_is_none() {
        // Valid hw.memsize, but the vm_stat header lacks "page size of N bytes" -> None.
        let vm = "Mach Virtual Memory Statistics:\nPages free:                          100.\n";
        assert!(parse_macos_memory("17179869184", vm).is_none());
    }

    #[test]
    fn read_memory_never_panics() {
        if let Some(m) = read_memory() {
            assert!(m.used_bytes <= m.total_bytes);
        }
    }
}
