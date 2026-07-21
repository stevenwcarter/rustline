//! Pure helpers for the host-managed HTTP response cache: cache-file path
//! derivation (FNV-1a of the URL), RFC3339 freshness, and quota-bounded
//! entry read/write. Used by `perform_http_get_cached`.

use std::path::{Path, PathBuf};

use chrono::DateTime;
use serde::{Deserialize, Serialize};

use crate::state::check_cap;

/// A cached HTTP response. `status` is the (2xx) status the body was fetched
/// under; `fetched_at` is the RFC3339 instant, used for freshness.
#[derive(Debug, Serialize, Deserialize)]
pub struct CacheEntry {
    pub fetched_at: String,
    pub status: u16,
    pub body: String,
}

/// FNV-1a (64-bit) of the URL — a deterministic, dependency-free key for a
/// disposable cache file. Not cryptographic; collisions only mean a cache miss.
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// `<state_dir>/__http_cache__/<hash>.json` — the cache file for `url`.
pub fn cache_path(state_dir: &Path, url: &str) -> PathBuf {
    state_dir
        .join("__http_cache__")
        .join(format!("{:016x}.json", fnv1a(url)))
}

/// Age in seconds of `fetched` relative to `now` if both parse, else `None`.
pub fn age_secs(now_rfc3339: &str, fetched_rfc3339: &str) -> Option<i64> {
    let now = DateTime::parse_from_rfc3339(now_rfc3339).ok()?;
    let fetched = DateTime::parse_from_rfc3339(fetched_rfc3339).ok()?;
    Some(now.timestamp() - fetched.timestamp())
}

/// Fresh iff `age` is within `[0, ttl_secs)` (negative age = clock skew = stale).
pub fn is_fresh(age_secs: i64, ttl_secs: i64) -> bool {
    (0..ttl_secs).contains(&age_secs)
}

/// Read and deserialize a cache entry; any absence/parse error → `None`.
pub fn read_entry(path: &Path) -> Option<CacheEntry> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Quota-checked write of `content` to `path` (creating `__http_cache__/`).
/// `check_cap` accounts against the whole `state_dir` (invariant N3).
pub fn write_entry(state_dir: &Path, path: &Path, content: &str, cap: u64) -> Result<(), String> {
    check_cap(state_dir, path, content.len() as u64, cap)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, content).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_respects_ttl_and_unparseable() {
        let now = "2026-07-20T12:30:00-04:00";
        let recent = "2026-07-20T12:10:00-04:00"; // 20 min ago
        let old = "2026-07-20T11:00:00-04:00"; // 90 min ago
        assert!(is_fresh(age_secs(now, recent).unwrap(), 1800)); // 1200 < 1800
        assert!(!is_fresh(age_secs(now, old).unwrap(), 1800)); // 5400 > 1800
        assert!(age_secs(now, "garbage").is_none()); // unparseable
        // negative age (clock skew: fetched in the future) is not fresh
        let future = "2026-07-20T13:00:00-04:00";
        assert!(!is_fresh(age_secs(now, future).unwrap(), 1800));
    }

    #[test]
    fn cache_path_is_deterministic_and_scoped() {
        let dir = Path::new("/state/weather");
        let a = cache_path(dir, "https://wttr.in/48183?format=j1");
        let b = cache_path(dir, "https://wttr.in/48183?format=j1");
        let c = cache_path(dir, "https://wttr.in/90210?format=j1");
        assert_eq!(a, b, "same url -> same path");
        assert_ne!(a, c, "different url -> different path");
        assert!(a.starts_with("/state/weather/__http_cache__"));
        assert_eq!(a.extension().unwrap(), "json");
    }

    #[test]
    fn write_entry_roundtrips_and_enforces_cap() {
        let dir = tempfile::tempdir().unwrap();
        let entry = CacheEntry {
            fetched_at: "2026-07-20T12:00:00-04:00".into(),
            status: 200,
            body: "hello".into(),
        };
        let content = serde_json::to_string(&entry).unwrap();
        let path = cache_path(dir.path(), "https://x/y");
        write_entry(dir.path(), &path, &content, 1_000).unwrap();
        let got = read_entry(&path).unwrap();
        assert_eq!(got.status, 200);
        assert_eq!(got.body, "hello");
        // a write that would blow the quota is refused
        let big = "z".repeat(2_000);
        assert!(write_entry(dir.path(), &cache_path(dir.path(), "big"), &big, 1_000).is_err());
        // a missing file reads as None
        assert!(read_entry(Path::new("/no/such/file.json")).is_none());
    }
}
