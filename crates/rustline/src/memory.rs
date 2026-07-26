//! Platform-specific memory read, isolated at the `Context`-build edge.
//!
//! Mirrors `battery.rs`: the `#[cfg(target_os)]` readers do the I/O; the pure
//! parsers compile under `test` on any host, so both are unit-tested on the
//! Linux dev box even though only one reader arm compiles per platform.

use std::path::Path;

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
        tracing::debug!(reader = "memory", "unsupported platform");
        None
    }
}

/// State-file name (under `sample_store`'s state dir) the persisted
/// memory-used% `{spark}` history ring is kept at.
const HISTORY_SAMPLE_NAME: &str = "memory-history";

/// Push `current_percent` onto the persisted memory-used% history ring,
/// truncate to the last `spark_width` readings, persist it back, and return
/// the resulting history (oldest first) for `Context.mem_history`. Called
/// only when the `memory` widget's `format` or `alt_format` actually
/// references `{spark}` (see `build_context.rs`) — mirrors `cpu.rs`'s
/// `read_cpu_history`.
pub fn read_memory_history(state_dir: &Path, current_percent: f32, spark_width: usize) -> Vec<f32> {
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
fn read_memory_linux() -> Option<MemInfo> {
    let text = std::fs::read_to_string("/proc/meminfo")
        .inspect_err(
            |error| tracing::debug!(reader = "memory", %error, "failed to read /proc/meminfo"),
        )
        .ok()?;
    let info = parse_meminfo(&text);
    if info.is_none() {
        tracing::debug!(
            reader = "memory",
            "failed to parse /proc/meminfo (missing MemTotal/MemAvailable?)"
        );
    }
    info
}

/// Render a `hw.memsize` byte count as the decimal text `parse_macos_memory`
/// expects. Trivial, but it is the seam that keeps the FFI migration a pure
/// *source* change: the parser's contract is untouched, so everything except
/// the syscall wrapper itself stays unit-tested on any host.
#[cfg(any(target_os = "macos", test))]
fn memsize_to_text(total_bytes: u64) -> String {
    total_bytes.to_string()
}

/// Total physical memory (`hw.memsize`) via `sysctlbyname`, memoized for the
/// process lifetime.
///
/// This replaced a `sysctl -n hw.memsize` subprocess spawn. `memory` is in the
/// default right layout, so that spawn ran on every `render right` — every
/// tmux tick, forever, on every macOS machine — to re-read a value that is
/// constant for the machine's lifetime. Mirrors `cpu.rs`'s
/// `read_mach_cpu_ticks`, which replaced a `top -l 2` shell-out for the same
/// reason; `libc` is already a dependency, so this adds no crate.
///
/// `None` if the sysctl fails — the widget then renders `down_format` rather
/// than a fabricated total (invariant #6). Note this does NOT degrade exactly
/// as the failed spawn did: the `OnceLock` caches the `Option`, so a failure is
/// latched for the process lifetime, where the old per-call spawn could fail
/// once and succeed on the next tick. That is a deliberate trade — `hw.memsize`
/// is installed physical RAM, so a query for it has no transient failure mode
/// the way a fork+exec does under memory or process-table pressure — but under
/// the long-lived daemon it does mean one failure disables the memory widget
/// until restart. Switch to `OnceLock<u64>`, set only on success, if that ever
/// proves reachable.
#[cfg(target_os = "macos")]
fn hw_memsize() -> Option<u64> {
    use std::sync::OnceLock;

    static MEMSIZE: OnceLock<Option<u64>> = OnceLock::new();
    *MEMSIZE.get_or_init(|| {
        let mut value: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        // SAFETY: `sysctlbyname` writes at most `len` bytes into `value`'s
        // storage and updates `len` to what it actually wrote. `value` is a
        // live, correctly-aligned `u64` we own, and `len` starts as exactly
        // its size, so the write cannot overrun. The name is a NUL-terminated
        // literal. We read `value` only when the call returned 0 (success) AND
        // reported writing a full `u64`, so it is always fully initialized
        // before use. No pointer outlives this block.
        let rc = unsafe {
            libc::sysctlbyname(
                c"hw.memsize".as_ptr(),
                (&raw mut value).cast::<libc::c_void>(),
                &raw mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (rc == 0 && len == std::mem::size_of::<u64>()).then_some(value)
    })
}

/// `vm_stat` stays a spawn for now: migrating it to
/// `host_statistics64(HOST_VM_INFO64)` is a separate, higher-risk change that
/// cannot be validated from a Linux dev box (see this file's history).
#[cfg(target_os = "macos")]
fn read_memory_macos() -> Option<MemInfo> {
    let Some(memsize) = hw_memsize() else {
        tracing::debug!(reader = "memory", "sysctlbyname(hw.memsize) failed");
        return None;
    };
    let vm = std::process::Command::new("vm_stat")
        .output()
        .inspect_err(|error| tracing::debug!(reader = "memory", %error, "vm_stat spawn failed"))
        .ok()?;
    let vm = String::from_utf8(vm.stdout)
        .inspect_err(
            |error| tracing::debug!(reader = "memory", %error, "vm_stat output was not utf-8"),
        )
        .ok()?;
    let info = parse_macos_memory(&memsize_to_text(memsize), &vm);
    if info.is_none() {
        tracing::debug!(reader = "memory", "failed to parse sysctl/vm_stat output");
    }
    info
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
    let available_bytes = free
        .saturating_add(inactive)
        .saturating_add(speculative)
        .saturating_mul(page_size);
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
    fn meminfo_missing_total_is_none() {
        assert!(parse_meminfo("MemAvailable:    100 kB\n").is_none());
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
    fn memsize_text_parses_the_same_whatever_produced_it() {
        // parse_macos_memory's contract is unchanged by the sysctl migration: it
        // still takes the total as text. Pinning it here is what makes the FFI
        // swap a *source* change rather than a behaviour change — the only part
        // that cannot be compiled on this box is the syscall wrapper itself.
        let vm = "Mach Virtual Memory Statistics: (page size of 4096 bytes)\n\
                  Pages free:                          1000.\n\
                  Pages inactive:                      2000.\n\
                  Pages speculative:                    500.\n";
        let from_spawn = parse_macos_memory("17179869184\n", vm).expect("parses");
        let from_ffi = parse_macos_memory(&memsize_to_text(17_179_869_184), vm).expect("parses");
        assert_eq!(from_spawn, from_ffi);
        assert_eq!(from_spawn.total_bytes, 17179869184);
    }

    #[test]
    fn memsize_text_round_trips_through_the_ffi_formatting() {
        // The FFI path formats a u64 into the same text parse_macos_memory reads.
        for total in [0u64, 1, 8 * 1024 * 1024 * 1024, u64::MAX] {
            assert_eq!(
                memsize_to_text(total).trim().parse::<u64>().ok(),
                Some(total)
            );
        }
    }

    #[test]
    fn read_memory_never_panics() {
        if let Some(m) = read_memory() {
            assert!(m.used_bytes <= m.total_bytes);
        }
    }

    #[test]
    fn read_memory_history_appends_and_persists_across_calls() {
        let dir = tempfile::tempdir().unwrap();
        let first = read_memory_history(dir.path(), 10.0, 8);
        assert_eq!(first, vec![10.0]);
        let second = read_memory_history(dir.path(), 20.0, 8);
        assert_eq!(second, vec![10.0, 20.0]);
    }

    #[test]
    fn read_memory_history_truncates_to_spark_width() {
        let dir = tempfile::tempdir().unwrap();
        for v in [1.0, 2.0, 3.0, 4.0] {
            read_memory_history(dir.path(), v, 2);
        }
        assert_eq!(read_memory_history(dir.path(), 5.0, 2), vec![4.0, 5.0]);
    }
}
