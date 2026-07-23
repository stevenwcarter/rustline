//! Filesystem-usage read, isolated at the `Context`-build edge, mirroring
//! `battery.rs`/`cpu.rs`/`memory.rs`/`git.rs`: `read_disk` calls `statvfs(2)`
//! on Linux/macOS and `disk_info_from_statvfs` is a pure derivation,
//! unit-tested independently of any real mount. `statvfs` is POSIX
//! (available on both Linux and macOS) so both platforms share one reader
//! arm; any other platform degrades to `None` the same way `read_battery`/
//! `read_cpu`/`read_memory` do.

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::ffi::CString;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::mem::MaybeUninit;

use rustline_core::DiskInfo;

/// Read filesystem usage for `mount`, or `None` if the platform is
/// unsupported, `mount` contains a nul byte, or the call itself fails (e.g.
/// the path doesn't exist) — never a fabricated `0` reading (invariant #6).
/// Called once at Context-build time, only when the `disk` widget is in the
/// active layout (see `build_context.rs`).
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn read_disk(mount: &str) -> Option<DiskInfo> {
    let path = CString::new(mount).ok()?;
    let mut stat = MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: `path` is a valid, nul-terminated C string and `stat` is a
    // properly aligned, zero-initialized buffer sized for `libc::statvfs`;
    // `statvfs` is documented to fill it in on success and leave it untouched
    // (return nonzero) on failure, which we check before reading it.
    let rc = unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: `rc == 0` means `statvfs` filled in every field.
    let stat = unsafe { stat.assume_init() };
    // `libc::statvfs`'s block-count fields are narrower on some platforms
    // (e.g. `u32` on macOS) than others (`u64` on Linux glibc), so the `as
    // u64` widening is only sometimes a no-op — kept unconditional here for
    // portability rather than cfg-splitting the cast per platform.
    #[allow(clippy::unnecessary_cast)]
    let (blocks, bfree, bavail, frsize) = (
        stat.f_blocks as u64,
        stat.f_bfree as u64,
        stat.f_bavail as u64,
        stat.f_frsize as u64,
    );
    Some(disk_info_from_statvfs(blocks, bfree, bavail, frsize))
}

/// `statvfs(2)` (and thus `libc::statvfs`) isn't available on this platform,
/// so filesystem usage is simply unsupported here — same `None` degradation
/// as `read_battery`/`read_cpu`/`read_memory` on an unsupported platform,
/// never a fabricated `0` reading (invariant #6).
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn read_disk(_mount: &str) -> Option<DiskInfo> {
    None
}

/// Derive a [`DiskInfo`] from raw `statvfs(2)` fields, already widened to
/// `u64` by the caller (the underlying field types differ between Linux and
/// macOS). Pure; unit-tested directly, independent of any real mount.
///
/// `total_bytes = f_blocks * f_frsize`, `available_bytes = f_bavail *
/// f_frsize` (both what's actually usable, respecting any root-reserved
/// blocks), and `used_bytes = (f_blocks - f_bfree) * f_frsize`. All
/// arithmetic is saturating, so a hostile/corrupt reading can't overflow or
/// panic.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
pub fn disk_info_from_statvfs(
    f_blocks: u64,
    f_bfree: u64,
    f_bavail: u64,
    f_frsize: u64,
) -> DiskInfo {
    DiskInfo {
        total_bytes: f_blocks.saturating_mul(f_frsize),
        used_bytes: f_blocks.saturating_sub(f_bfree).saturating_mul(f_frsize),
        available_bytes: f_bavail.saturating_mul(f_frsize),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_case_computes_total_used_available() {
        // 1000 blocks of 4096 bytes: 100 free (12 reserved-for-root beyond
        // bavail's 88 available), so used = 900 blocks.
        let info = disk_info_from_statvfs(1000, 100, 88, 4096);
        assert_eq!(info.total_bytes, 1000 * 4096);
        assert_eq!(info.used_bytes, 900 * 4096);
        assert_eq!(info.available_bytes, 88 * 4096);
    }

    #[test]
    fn used_greater_than_zero_case() {
        let info = disk_info_from_statvfs(2048, 512, 400, 1024);
        assert_eq!(info.total_bytes, 2048 * 1024);
        assert_eq!(info.used_bytes, 1536 * 1024);
        assert_eq!(info.available_bytes, 400 * 1024);
    }

    #[test]
    fn full_disk_zero_free_and_available() {
        let info = disk_info_from_statvfs(500, 0, 0, 4096);
        assert_eq!(info.total_bytes, 500 * 4096);
        assert_eq!(info.used_bytes, 500 * 4096);
        assert_eq!(info.available_bytes, 0);
    }

    #[test]
    fn empty_disk_all_blocks_free() {
        let info = disk_info_from_statvfs(500, 500, 500, 4096);
        assert_eq!(info.total_bytes, 500 * 4096);
        assert_eq!(info.used_bytes, 0);
        assert_eq!(info.available_bytes, 500 * 4096);
    }

    #[test]
    fn read_disk_on_real_root_mount_returns_sane_values() {
        // Host-dependent (needs a readable "/"); only assert Some + sane
        // shape rather than exact bytes, since disk size varies per box.
        let info = read_disk("/").expect("root mount must be statvfs-able");
        assert!(info.total_bytes > 0);
        assert!(info.used_bytes <= info.total_bytes);
    }

    #[test]
    fn read_disk_bogus_path_is_none() {
        assert!(read_disk("/nonexistent/bogus/mount/path").is_none());
    }

    #[test]
    fn read_disk_nul_byte_in_mount_is_none() {
        // A nul byte makes `CString::new` fail before any syscall happens.
        assert!(read_disk("/has\0nul").is_none());
    }
}
