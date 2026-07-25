//! Shared plugin-`.wasm` checksum status, used by both `rustline doctor` and
//! `rustline plugin list` to surface what `register_plugins`' load-time gate
//! (`crates/rustline-wasm/src/lib.rs`) silently enforces. Both read-only
//! surfaces read the installed `.wasm` off disk and check it via
//! [`rustline_wasm::verify_checksum`] — never re-implementing that
//! comparison, so they can't drift from the gate they're reporting on.

use std::path::Path;

use rustline_wasm::ChecksumVerdict;

/// One plugin's checksum status: the read-plus-verify outcome a diagnostic
/// surface reports, one step removed from [`ChecksumVerdict`] itself so it can
/// also represent "the `.wasm` wasn't even readable" (not yet built/installed,
/// or removed out from under a still-configured plugin) — a case
/// `verify_checksum` has no opinion on, since it never touches the filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PluginChecksumStatus {
    /// No checksum recorded (or blank) — nothing to verify.
    Unpinned,
    /// Recorded checksum matches the bytes on disk.
    Verified,
    /// Recorded checksum is well-formed but doesn't match the bytes on disk.
    Mismatch,
    /// Recorded checksum isn't a parseable sha256 digest.
    Malformed,
    /// The plugin's `.wasm` couldn't be read at all.
    Missing,
}

impl PluginChecksumStatus {
    /// A short, stable label shared by `doctor`'s detail text and `plugin
    /// list`'s human/JSON output.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Unpinned => "unpinned",
            Self::Verified => "verified",
            Self::Mismatch => "mismatch",
            Self::Malformed => "malformed",
            Self::Missing => "missing",
        }
    }
}

/// Read `<plugin_dir>/<name>.wasm` and check it against `checksum` via
/// [`rustline_wasm::verify_checksum`]. Never panics or errors: an unreadable
/// file (missing, permissions, not yet built) is
/// [`PluginChecksumStatus::Missing`], a diagnostic value rather than a failure
/// — both callers are read-only diagnostics that must never abort on a bad
/// plugin file (invariant N2).
pub(crate) fn status_for(
    plugin_dir: &Path,
    name: &str,
    checksum: Option<&str>,
) -> PluginChecksumStatus {
    match std::fs::read(plugin_dir.join(format!("{name}.wasm"))) {
        Ok(bytes) => match rustline_wasm::verify_checksum(checksum, &bytes) {
            ChecksumVerdict::NotRecorded => PluginChecksumStatus::Unpinned,
            ChecksumVerdict::Match => PluginChecksumStatus::Verified,
            ChecksumVerdict::Mismatch { .. } => PluginChecksumStatus::Mismatch,
            ChecksumVerdict::Malformed { .. } => PluginChecksumStatus::Malformed,
        },
        Err(_) => PluginChecksumStatus::Missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_wasm_file_is_missing_status() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            status_for(dir.path(), "nope", None),
            PluginChecksumStatus::Missing
        );
    }

    #[test]
    fn no_checksum_recorded_is_unpinned() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("w.wasm"), b"bytes").unwrap();
        assert_eq!(
            status_for(dir.path(), "w", None),
            PluginChecksumStatus::Unpinned
        );
    }

    #[test]
    fn blank_checksum_is_unpinned() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("w.wasm"), b"bytes").unwrap();
        assert_eq!(
            status_for(dir.path(), "w", Some("   ")),
            PluginChecksumStatus::Unpinned
        );
    }

    #[test]
    fn matching_checksum_is_verified() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("w.wasm"), b"bytes").unwrap();
        let sum = rustline_wasm::sha256_hex(b"bytes");
        assert_eq!(
            status_for(dir.path(), "w", Some(&sum)),
            PluginChecksumStatus::Verified
        );
    }

    #[test]
    fn wrong_checksum_is_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("w.wasm"), b"bytes").unwrap();
        let sum = rustline_wasm::sha256_hex(b"other bytes entirely");
        assert_eq!(
            status_for(dir.path(), "w", Some(&sum)),
            PluginChecksumStatus::Mismatch
        );
    }

    #[test]
    fn unparseable_checksum_is_malformed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("w.wasm"), b"bytes").unwrap();
        assert_eq!(
            status_for(dir.path(), "w", Some("not-a-real-digest")),
            PluginChecksumStatus::Malformed
        );
    }

    #[test]
    fn labels_are_stable() {
        assert_eq!(PluginChecksumStatus::Unpinned.label(), "unpinned");
        assert_eq!(PluginChecksumStatus::Verified.label(), "verified");
        assert_eq!(PluginChecksumStatus::Mismatch.label(), "mismatch");
        assert_eq!(PluginChecksumStatus::Malformed.label(), "malformed");
        assert_eq!(PluginChecksumStatus::Missing.label(), "missing");
    }
}
