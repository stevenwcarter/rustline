//! Pure helpers for the host-managed TTL caches: cache-file path derivation
//! (FNV-1a of the cache key), RFC3339 freshness, and quota-bounded entry
//! read/write. Used by both `perform_http_get_cached` (the HTTP response
//! cache, keyed on the URL) and `perform_exec_cached` (the exec-result
//! cache, keyed on the canonical argv) — see [`HTTP_NAMESPACE`]/
//! [`EXEC_NAMESPACE`] for how the two stay in separate subdirectories so
//! their keys can never collide.

use std::path::{Path, PathBuf};

use chrono::DateTime;
use serde::{Deserialize, Serialize};

use crate::state::check_cap;

/// A cached entry, shared by both TTL caches. For the HTTP cache `status` is
/// the (2xx) status the body was fetched under and `body` is the response
/// body; for the exec cache `status` is the process exit code and `body` is
/// captured stdout (see `perform_exec_cached`'s doc for that cache's
/// round-trip limitations — stderr and the `truncated` flag aren't
/// persisted). `fetched_at` is the RFC3339 instant either way, used for
/// freshness.
#[derive(Debug, Serialize, Deserialize)]
pub struct CacheEntry {
    pub fetched_at: String,
    pub status: u16,
    pub body: String,
    /// When a refresh was last *attempted*, successful or not — RFC3339, or
    /// empty for an entry written before this field existed (or never
    /// attempted). `fetched_at` only advances on success, so without this a
    /// dead upstream is re-fetched on every single render forever: the
    /// fresh-hit branch can never be taken again once the TTL lapses.
    /// `#[serde(default)]` keeps old on-disk entries readable.
    ///
    /// The backoff this field drives (`is_fresh` in `perform_http_get_cached`/
    /// `perform_exec_cached`) is inert whenever `ttl_secs <= 0`, since
    /// `is_fresh` treats `[0, ttl_secs)` as fresh — an empty range when
    /// `ttl_secs` isn't positive. `host.rs` maps an unparseable guest
    /// `ttl_secs` string to `0` via `unwrap_or(0)`, so a guest sending
    /// garbage silently loses the backoff and gets the pre-fix retry storm
    /// back. Known, not fixed here — see B4's follow-up notes.
    #[serde(default)]
    pub last_attempt_at: String,
}

/// FNV-1a (64-bit) of the cache key (a URL or a canonical argv string) — a
/// deterministic, dependency-free key for a disposable cache file. Not
/// cryptographic; collisions only mean a cache miss.
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// `<state_dir>/<namespace>/<hash>.json` — the cache file for `key` within a
/// namespace. The namespace keeps the HTTP and exec caches in separate
/// subdirectories so a URL and a command line that happen to hash the same can
/// never read each other's entries. Both stay under the plugin's own state
/// dir, so `check_cap`'s quota accounting (invariant N3) covers them unchanged.
pub fn cache_path(state_dir: &Path, namespace: &str, key: &str) -> PathBuf {
    state_dir
        .join(namespace)
        .join(format!("{:016x}.json", fnv1a(key)))
}

/// The HTTP response cache's namespace.
pub const HTTP_NAMESPACE: &str = "__http_cache__";
/// The exec result cache's namespace.
pub const EXEC_NAMESPACE: &str = "__exec_cache__";

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

/// Delete entries from a cache namespace directory until it fits in
/// `keep_bytes`, oldest first. Returns bytes freed.
///
/// Ordering is by the entry's own `fetched_at` when it parses, falling back to
/// mtime — a file we cannot parse is not a usable cache entry, so it sorts as
/// oldest and goes first.
///
/// Best-effort throughout (invariant N2: a cache is disposable). Only ever
/// touches regular files directly inside `dir`, which is always a
/// `HTTP_NAMESPACE`/`EXEC_NAMESPACE` subdirectory of one plugin's own state
/// dir — never the plugin's other state files, and never a subdirectory.
fn evict_namespace(dir: &Path, keep_bytes: u64) -> u64 {
    let Ok(read) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut entries: Vec<(i64, u64, PathBuf)> = read
        .filter_map(Result::ok)
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| {
            let path = e.path();
            let len = e.metadata().ok()?.len();
            let stamp = read_entry(&path)
                .and_then(|c| DateTime::parse_from_rfc3339(&c.fetched_at).ok())
                .map(|d| d.timestamp())
                .unwrap_or(i64::MIN);
            Some((stamp, len, path))
        })
        .collect();

    let mut total: u64 = entries.iter().map(|(_, len, _)| *len).sum();
    if total <= keep_bytes {
        return 0;
    }
    entries.sort_by_key(|(stamp, _, _)| *stamp); // oldest first

    let mut freed: u64 = 0;
    for (_, len, path) in entries {
        if total <= keep_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
            freed = freed.saturating_add(len);
        }
    }
    freed
}

/// Quota-checked write of `content` to `path` (creating the namespace
/// directory `path` sits in — `HTTP_NAMESPACE`/`EXEC_NAMESPACE`, per the
/// caller). `check_cap` accounts against the whole `state_dir` (invariant N3).
pub fn write_entry(state_dir: &Path, path: &Path, content: &str, cap: u64) -> Result<(), String> {
    if let Err(first) = check_cap(state_dir, path, content.len() as u64, cap) {
        // The cache is at quota. Without eviction this returns Err forever:
        // every subsequent render then pays a live fetch or a real subprocess
        // spawn while a full state dir of dead entries sits on disk. Evict the
        // namespace down to a fraction of the cap and re-check once.
        let namespace = path.parent().unwrap_or(state_dir);
        evict_namespace(namespace, cap / 2);
        check_cap(state_dir, path, content.len() as u64, cap).map_err(|_| first)?;
    }
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
        let a = cache_path(dir, HTTP_NAMESPACE, "https://wttr.in/48183?format=j1");
        let b = cache_path(dir, HTTP_NAMESPACE, "https://wttr.in/48183?format=j1");
        let c = cache_path(dir, HTTP_NAMESPACE, "https://wttr.in/90210?format=j1");
        assert_eq!(a, b, "same url -> same path");
        assert_ne!(a, c, "different url -> different path");
        assert!(a.starts_with("/state/weather/__http_cache__"));
        assert_eq!(a.extension().unwrap(), "json");
    }

    #[test]
    fn cache_path_namespaces_never_collide_on_the_same_key() {
        let dir = Path::new("/state/weather");
        let http = cache_path(dir, HTTP_NAMESPACE, "same-key");
        let exec = cache_path(dir, EXEC_NAMESPACE, "same-key");
        assert_ne!(http, exec, "http and exec caches must not share a file");
    }

    #[test]
    fn write_entry_roundtrips_and_enforces_cap() {
        let dir = tempfile::tempdir().unwrap();
        let entry = CacheEntry {
            fetched_at: "2026-07-20T12:00:00-04:00".into(),
            status: 200,
            body: "hello".into(),
            last_attempt_at: "2026-07-20T12:00:00-04:00".into(),
        };
        let content = serde_json::to_string(&entry).unwrap();
        let path = cache_path(dir.path(), HTTP_NAMESPACE, "https://x/y");
        write_entry(dir.path(), &path, &content, 1_000).unwrap();
        let got = read_entry(&path).unwrap();
        assert_eq!(got.status, 200);
        assert_eq!(got.body, "hello");
        // a write that would blow the quota is refused
        let big = "z".repeat(2_000);
        let big_path = cache_path(dir.path(), HTTP_NAMESPACE, "big");
        assert!(write_entry(dir.path(), &big_path, &big, 1_000).is_err());
        // a missing file reads as None
        assert!(read_entry(Path::new("/no/such/file.json")).is_none());
    }

    #[test]
    fn an_entry_without_last_attempt_at_still_deserializes() {
        // Entries written by an older build must keep working (wire-type
        // discipline: additive, defaulted fields only).
        let old = r#"{"fetched_at":"2026-07-20T12:00:00-04:00","status":200,"body":"hi"}"#;
        let e: CacheEntry = serde_json::from_str(old).unwrap();
        assert_eq!(e.body, "hi");
        assert!(e.last_attempt_at.is_empty());
    }

    fn entry_json(fetched_at: &str, body: &str) -> String {
        serde_json::to_string(&CacheEntry {
            fetched_at: fetched_at.to_string(),
            status: 200,
            body: body.to_string(),
            last_attempt_at: fetched_at.to_string(),
        })
        .unwrap()
    }

    #[test]
    fn a_full_cache_evicts_and_the_write_then_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let cap = 1_200;
        // A real plugin state file living directly in `state_dir`, alongside
        // the cache namespace — not cache, and not inside the namespace
        // `write_entry` is told to evict from. If eviction were ever wired to
        // the wrong directory (`state_dir` instead of `path.parent()`), this
        // is exactly the file it would wrongly delete.
        let sibling = dir.path().join("counter-state.json");
        std::fs::write(&sibling, "keep me").unwrap();
        // Fill the namespace past the cap with distinct keys.
        for i in 0..10 {
            let p = cache_path(dir.path(), HTTP_NAMESPACE, &format!("https://x/{i}"));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(
                &p,
                entry_json("2026-07-20T12:00:00-04:00", &"z".repeat(150)),
            )
            .unwrap();
        }
        let fresh = cache_path(dir.path(), HTTP_NAMESPACE, "https://x/new");
        // Before: over cap, so this write would be refused forever.
        assert!(crate::state::dir_size(dir.path()) > cap);
        let result = write_entry(
            dir.path(),
            &fresh,
            &entry_json("2026-07-26T12:00:00-04:00", "new"),
            cap,
        );
        // Checked ahead of `result`'s own assertion so a wiring bug that
        // "successfully" frees room by deleting the sibling is caught even
        // when the write itself would otherwise look like it worked.
        assert_eq!(
            std::fs::read_to_string(&sibling).unwrap(),
            "keep me",
            "eviction triggered by write_entry must not touch a sibling state file"
        );
        result.expect("eviction makes room");
        assert!(read_entry(&fresh).is_some(), "the new entry landed");
    }

    #[test]
    fn eviction_prefers_removing_the_oldest_entries() {
        let dir = tempfile::tempdir().unwrap();
        let ns = dir.path().join(HTTP_NAMESPACE);
        std::fs::create_dir_all(&ns).unwrap();
        let old = ns.join("old.json");
        let new = ns.join("new.json");
        let old_content = entry_json("2020-01-01T00:00:00-00:00", &"z".repeat(400));
        std::fs::write(&old, &old_content).unwrap();
        std::fs::write(
            &new,
            entry_json("2026-07-26T12:00:00-04:00", &"z".repeat(400)),
        )
        .unwrap();
        // Each serialized entry is 511 bytes (111-byte JSON envelope + the
        // 400-byte body); 600 sits strictly between one entry (511) and both
        // (1022), so evicting only the oldest is enough to clear it.
        let freed = evict_namespace(&ns, 600);
        // Pin the exact byte count, not just "something": a freed count that
        // doesn't match what was actually deleted would leave the caller
        // (write_entry's cap/2 headroom target) accounting against a lie.
        assert_eq!(
            freed,
            old_content.len() as u64,
            "freed must equal the deleted entry's size"
        );
        assert!(!old.exists(), "the oldest entry goes first");
        assert!(new.exists(), "the newest entry is retained");
    }

    #[test]
    fn a_cap_too_small_to_ever_fit_still_errors_without_looping() {
        let dir = tempfile::tempdir().unwrap();
        let p = cache_path(dir.path(), HTTP_NAMESPACE, "https://x/y");
        let big = "z".repeat(5_000);
        assert!(write_entry(dir.path(), &p, &big, 10).is_err());
    }

    #[test]
    fn eviction_never_touches_files_outside_the_namespace() {
        let dir = tempfile::tempdir().unwrap();
        let sibling = dir.path().join("counter-state.json");
        std::fs::write(&sibling, "keep me").unwrap();
        let ns = dir.path().join(HTTP_NAMESPACE);
        std::fs::create_dir_all(&ns).unwrap();
        std::fs::write(
            ns.join("a.json"),
            entry_json("2020-01-01T00:00:00-00:00", &"z".repeat(400)),
        )
        .unwrap();
        evict_namespace(&ns, 0);
        assert!(sibling.exists(), "a sibling state file is not cache");
    }

    #[test]
    fn write_entry_never_sacrifices_a_sibling_state_file_to_make_room() {
        // A cap so small that only deleting the (non-cache) sibling file
        // could ever satisfy it — the HTTP namespace doesn't even exist yet,
        // so eviction there frees nothing. If `write_entry` ever evicted
        // against `state_dir` instead of the namespace it actually sits in,
        // this is exactly the failure mode: it would "succeed" by deleting a
        // real plugin state file rather than correctly refusing the write.
        let dir = tempfile::tempdir().unwrap();
        let cap = 20;
        let sibling = dir.path().join("counter-state.json");
        std::fs::write(&sibling, "x".repeat(50)).unwrap();
        let fresh = cache_path(dir.path(), HTTP_NAMESPACE, "https://x/new");

        assert!(
            write_entry(dir.path(), &fresh, "new", cap).is_err(),
            "a cap this small can never be satisfied without deleting real state"
        );
        assert_eq!(
            std::fs::read_to_string(&sibling).unwrap(),
            "x".repeat(50),
            "the sibling state file must survive a refused write"
        );
    }
}
