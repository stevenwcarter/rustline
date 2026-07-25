//! Plugin binary integrity (W19): the sha256 digest helper plus the pure
//! decision function a plugin load is gated on.
//!
//! Kept deliberately free of I/O, Extism, and config types so the whole policy
//! — including every rejection path — is unit-testable without a real `.wasm`.

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

/// Lowercase hex sha256 of `bytes` (64 chars) — the `checksum` recorded for an
/// installed plugin, and the value verified against at load time.
///
/// This is the single definition; `rustline`'s `plugin_install` calls it from
/// here rather than keeping its own copy, so the digest written at install time
/// and the digest checked at load time can never drift apart.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().fold(String::with_capacity(64), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// The outcome of checking a plugin's bytes against its recorded `checksum`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChecksumVerdict {
    /// No digest recorded (or the field was cleared) — load as before.
    NotRecorded,
    /// Recorded digest matches the bytes on disk.
    Match,
    /// Recorded digest is well-formed but describes different bytes.
    Mismatch { expected: String, actual: String },
    /// Recorded digest could not be parsed as a sha256 hex string.
    ///
    /// Treated as a refusal (**fail closed**): a user who recorded a checksum
    /// is asking for verification, and a value we cannot parse is one we cannot
    /// verify against.
    Malformed { recorded: String },
}

impl ChecksumVerdict {
    /// Whether a plugin with this verdict may be registered.
    pub fn allows_load(&self) -> bool {
        matches!(self, Self::NotRecorded | Self::Match)
    }
}

/// Normalize a recorded digest for comparison: trim, drop an optional
/// `sha256:` prefix, lowercase. `None` when the result is not 64 hex digits.
fn normalize(recorded: &str) -> Option<String> {
    let trimmed = recorded.trim();
    let trimmed = trimmed.strip_prefix("sha256:").unwrap_or(trimmed).trim();
    if trimmed.len() != 64 || !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

/// Check `bytes` against a plugin's recorded `checksum`.
///
/// An absent or blank `recorded` yields [`ChecksumVerdict::NotRecorded`]; an
/// unparseable one yields [`ChecksumVerdict::Malformed`] and fails closed.
pub fn verify_checksum(recorded: Option<&str>, bytes: &[u8]) -> ChecksumVerdict {
    let Some(recorded) = recorded else {
        return ChecksumVerdict::NotRecorded;
    };
    if recorded.trim().is_empty() {
        return ChecksumVerdict::NotRecorded;
    }
    let Some(expected) = normalize(recorded) else {
        return ChecksumVerdict::Malformed {
            recorded: recorded.to_string(),
        };
    };
    let actual = sha256_hex(bytes);
    if actual == expected {
        ChecksumVerdict::Match
    } else {
        ChecksumVerdict::Mismatch { expected, actual }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// sha256 of the empty input, from the FIPS 180-4 test vectors.
    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    /// sha256 of b"hello".
    const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[test]
    fn sha256_hex_matches_known_vectors() {
        assert_eq!(sha256_hex(b""), EMPTY_SHA256);
        assert_eq!(sha256_hex(b"hello"), HELLO_SHA256);
    }

    #[test]
    fn sha256_hex_is_64_lowercase_hex_chars() {
        let h = sha256_hex(b"anything");
        assert_eq!(h.len(), 64);
        assert!(
            h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn absent_checksum_is_not_recorded() {
        assert_eq!(
            verify_checksum(None, b"hello"),
            ChecksumVerdict::NotRecorded
        );
    }

    #[test]
    fn empty_or_blank_checksum_counts_as_not_recorded() {
        // A user clearing the field means "don't verify", not "verify against
        // nothing" — this must not brick the plugin.
        assert_eq!(
            verify_checksum(Some(""), b"hello"),
            ChecksumVerdict::NotRecorded
        );
        assert_eq!(
            verify_checksum(Some("   "), b"hello"),
            ChecksumVerdict::NotRecorded
        );
    }

    #[test]
    fn matching_checksum_verifies() {
        assert_eq!(
            verify_checksum(Some(HELLO_SHA256), b"hello"),
            ChecksumVerdict::Match
        );
    }

    #[test]
    fn matching_checksum_is_case_insensitive() {
        let upper = HELLO_SHA256.to_ascii_uppercase();
        assert_eq!(
            verify_checksum(Some(&upper), b"hello"),
            ChecksumVerdict::Match
        );
    }

    #[test]
    fn sha256_prefix_is_accepted() {
        let prefixed = format!("sha256:{HELLO_SHA256}");
        assert_eq!(
            verify_checksum(Some(&prefixed), b"hello"),
            ChecksumVerdict::Match
        );
    }

    #[test]
    fn surrounding_whitespace_is_tolerated() {
        let padded = format!("  {HELLO_SHA256}  ");
        assert_eq!(
            verify_checksum(Some(&padded), b"hello"),
            ChecksumVerdict::Match
        );
    }

    #[test]
    fn differing_checksum_is_a_mismatch_reporting_both_digests() {
        let verdict = verify_checksum(Some(EMPTY_SHA256), b"hello");
        assert_eq!(
            verdict,
            ChecksumVerdict::Mismatch {
                expected: EMPTY_SHA256.to_string(),
                actual: HELLO_SHA256.to_string(),
            }
        );
    }

    #[test]
    fn too_short_checksum_is_malformed() {
        assert_eq!(
            verify_checksum(Some("abc123"), b"hello"),
            ChecksumVerdict::Malformed {
                recorded: "abc123".to_string()
            }
        );
    }

    #[test]
    fn too_long_checksum_is_malformed() {
        let long = format!("{HELLO_SHA256}ff");
        assert_eq!(
            verify_checksum(Some(&long), b"hello"),
            ChecksumVerdict::Malformed { recorded: long }
        );
    }

    #[test]
    fn non_hex_checksum_of_correct_length_is_malformed() {
        let bad = "z".repeat(64);
        assert_eq!(
            verify_checksum(Some(&bad), b"hello"),
            ChecksumVerdict::Malformed { recorded: bad }
        );
    }

    #[test]
    fn allows_load_permits_only_absent_and_matching() {
        assert!(ChecksumVerdict::NotRecorded.allows_load());
        assert!(ChecksumVerdict::Match.allows_load());
        assert!(
            !ChecksumVerdict::Mismatch {
                expected: EMPTY_SHA256.to_string(),
                actual: HELLO_SHA256.to_string(),
            }
            .allows_load()
        );
        assert!(
            !ChecksumVerdict::Malformed {
                recorded: "x".to_string()
            }
            .allows_load()
        );
    }
}
