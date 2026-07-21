//! The capability-checked effect functions. Each returns a structured result
//! and never panics — the host_fn wrappers just serialize these to JSON.

use crate::abi::{CachedHttpResult, HttpResult, ReadResult, WriteResult};
use crate::cache::{CacheEntry, age_secs, cache_path, is_fresh, read_entry, write_entry};
use crate::capability::CapabilityCtx;
use crate::fetch::Fetcher;
use crate::state::{check_cap, normalize_abs, sanitize_relpath};

pub fn perform_http_get(ctx: &CapabilityCtx, url: &str, fetcher: &dyn Fetcher) -> HttpResult {
    if !ctx.allowed_urls.allows(url) {
        return HttpResult {
            ok: false,
            error: format!("url not allowed: {url}"),
            ..Default::default()
        };
    }
    match fetcher.get(url) {
        Ok((status, body)) => HttpResult {
            ok: true,
            status,
            body,
            error: String::new(),
        },
        Err(error) => HttpResult {
            ok: false,
            error,
            ..Default::default()
        },
    }
}

/// TTL-cached HTTP GET. Gate-first (denied → no fetch, no cache touch); fresh
/// cache hit served without fetching; on a failed/non-2xx refresh, serve the
/// last-good entry stale if present. Only 2xx responses are cached.
pub fn perform_http_get_cached(
    ctx: &CapabilityCtx,
    url: &str,
    ttl_secs: i64,
    now: &str,
    fetcher: &dyn Fetcher,
) -> CachedHttpResult {
    // 1) gate first (invariant N1): a denied url makes no network call and
    //    touches no cache file.
    if !ctx.allowed_urls.allows(url) {
        return CachedHttpResult {
            ok: false,
            error: format!("url not allowed: {url}"),
            ..Default::default()
        };
    }

    let dir = ctx.state_dir();
    let path = cache_path(&dir, url);
    let entry = read_entry(&path);

    // 2) fresh hit → serve without fetching.
    if let Some(e) = &entry
        && let Some(age) = age_secs(now, &e.fetched_at)
        && is_fresh(age, ttl_secs)
    {
        return CachedHttpResult {
            ok: true,
            status: e.status,
            body: e.body.clone(),
            error: String::new(),
            stale: false,
            age_secs: age,
        };
    }

    // 3) refresh.
    match fetcher.get(url) {
        Ok((status, body)) if (200..300).contains(&status) => {
            let content = serde_json::to_string(&CacheEntry {
                fetched_at: now.to_string(),
                status,
                body: body.clone(),
            })
            .unwrap_or_default();
            if let Err(error) = write_entry(&dir, &path, &content, ctx.max_state_bytes) {
                tracing::warn!(%error, %url, "http cache write failed; returning body unpersisted");
            }
            CachedHttpResult {
                ok: true,
                status,
                body,
                error: String::new(),
                stale: false,
                age_secs: 0,
            }
        }
        // non-2xx or transport error → refresh failed.
        other => {
            let error = match other {
                Ok((status, _)) => format!("http status {status}"),
                Err(e) => e,
            };
            match entry {
                // serve last-good stale (no egress beyond the failed attempt).
                Some(e) => {
                    let age = age_secs(now, &e.fetched_at).unwrap_or(0);
                    CachedHttpResult {
                        ok: true,
                        status: e.status,
                        body: e.body,
                        error,
                        stale: true,
                        age_secs: age,
                    }
                }
                None => CachedHttpResult {
                    ok: false,
                    error,
                    ..Default::default()
                },
            }
        }
    }
}

pub fn perform_state_read(ctx: &CapabilityCtx, relpath: &str) -> ReadResult {
    let rel = match sanitize_relpath(relpath) {
        Ok(r) => r,
        Err(error) => {
            return ReadResult {
                ok: false,
                error,
                ..Default::default()
            };
        }
    };
    let full = ctx.state_dir().join(rel);
    match std::fs::read_to_string(&full) {
        Ok(contents) => ReadResult {
            ok: true,
            exists: true,
            contents,
            error: String::new(),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ReadResult {
            ok: true,
            exists: false,
            ..Default::default()
        },
        Err(e) => ReadResult {
            ok: false,
            error: e.to_string(),
            ..Default::default()
        },
    }
}

pub fn perform_state_write(ctx: &CapabilityCtx, relpath: &str, contents: &str) -> WriteResult {
    let rel = match sanitize_relpath(relpath) {
        Ok(r) => r,
        Err(error) => return WriteResult { ok: false, error },
    };
    let dir = ctx.state_dir();
    let full = dir.join(rel);
    if let Err(error) = check_cap(&dir, &full, contents.len() as u64, ctx.max_state_bytes) {
        return WriteResult { ok: false, error };
    }
    if let Some(parent) = full.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return WriteResult {
            ok: false,
            error: e.to_string(),
        };
    }
    match std::fs::write(&full, contents.as_bytes()) {
        Ok(()) => WriteResult {
            ok: true,
            error: String::new(),
        },
        Err(e) => WriteResult {
            ok: false,
            error: e.to_string(),
        },
    }
}

pub fn perform_file_read(ctx: &CapabilityCtx, path: &str) -> ReadResult {
    let norm = match normalize_abs(path) {
        Ok(p) => p,
        Err(error) => {
            return ReadResult {
                ok: false,
                error,
                ..Default::default()
            };
        }
    };
    if !ctx.allowed_paths.allows(&norm) {
        return ReadResult {
            ok: false,
            error: format!("path not allowed: {norm}"),
            ..Default::default()
        };
    }
    match std::fs::read_to_string(&norm) {
        Ok(contents) => ReadResult {
            ok: true,
            exists: true,
            contents,
            error: String::new(),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ReadResult {
            ok: true,
            exists: false,
            ..Default::default()
        },
        Err(e) => ReadResult {
            ok: false,
            error: e.to_string(),
            ..Default::default()
        },
    }
}

pub fn perform_file_write(ctx: &CapabilityCtx, path: &str, contents: &str) -> WriteResult {
    let norm = match normalize_abs(path) {
        Ok(p) => p,
        Err(error) => return WriteResult { ok: false, error },
    };
    if !ctx.allowed_paths.allows(&norm) {
        return WriteResult {
            ok: false,
            error: format!("path not allowed: {norm}"),
        };
    }
    match std::fs::write(&norm, contents.as_bytes()) {
        Ok(()) => WriteResult {
            ok: true,
            error: String::new(),
        },
        Err(e) => WriteResult {
            ok: false,
            error: e.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::capability::CapabilityCtx;
    use rustline_core::PluginConfig;

    struct FakeFetcher(u16, &'static str);
    impl crate::fetch::Fetcher for FakeFetcher {
        fn get(&self, _url: &str) -> Result<(u16, String), String> {
            Ok((self.0, self.1.to_string()))
        }
    }
    struct DeadFetcher;
    impl crate::fetch::Fetcher for DeadFetcher {
        fn get(&self, _url: &str) -> Result<(u16, String), String> {
            Err("connection refused".into())
        }
    }

    struct CountingFetcher {
        calls: std::sync::Arc<AtomicUsize>,
        status: u16,
        body: &'static str,
    }
    impl crate::fetch::Fetcher for CountingFetcher {
        fn get(&self, _url: &str) -> Result<(u16, String), String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok((self.status, self.body.to_string()))
        }
    }

    fn ctx_with(urls: &[&str], root: std::path::PathBuf) -> CapabilityCtx {
        ctx_with_cap(urls, root, 16)
    }

    // The cache tests wrap each body in a JSON envelope (fetched_at + status +
    // body), so even a short body's entry is well over `ctx_with`'s 16-byte
    // cap; tests that need a write to actually persist use a roomy cap here,
    // leaving `ctx_with`'s tight cap for the tests that exercise it directly.
    fn ctx_with_cap(
        urls: &[&str],
        root: std::path::PathBuf,
        max_state_bytes: u64,
    ) -> CapabilityCtx {
        let pc = PluginConfig {
            allowed_urls: urls.iter().map(|s| s.to_string()).collect(),
            max_state_bytes,
            ..PluginConfig::default()
        };
        CapabilityCtx::from_config("weather", &pc, root)
    }

    #[test]
    fn http_denied_when_not_allowlisted_makes_no_request() {
        let ctx = ctx_with(&[], std::env::temp_dir());
        let r = perform_http_get(&ctx, "https://wttr.in/48183", &FakeFetcher(200, "hi"));
        assert!(!r.ok);
        assert!(r.error.contains("not allowed"));
    }

    #[test]
    fn http_allowed_returns_body() {
        let ctx = ctx_with(&["https://wttr.in/*"], std::env::temp_dir());
        let r = perform_http_get(&ctx, "https://wttr.in/48183", &FakeFetcher(200, "sunny"));
        assert!(r.ok);
        assert_eq!(r.status, 200);
        assert_eq!(r.body, "sunny");
    }

    #[test]
    fn http_transport_error_reports_not_ok() {
        let ctx = ctx_with(&["https://wttr.in/*"], std::env::temp_dir());
        let r = perform_http_get(&ctx, "https://wttr.in/48183", &DeadFetcher);
        assert!(!r.ok);
        assert!(r.error.contains("refused"));
    }

    #[test]
    fn state_write_then_read_roundtrips_and_enforces_cap() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with(&[], root.path().to_path_buf());
        let w = perform_state_write(&ctx, "weather.json", "0123456789"); // 10 bytes, cap 16
        assert!(w.ok, "{:?}", w.error);
        let r = perform_state_read(&ctx, "weather.json");
        assert!(r.ok && r.exists);
        assert_eq!(r.contents, "0123456789");
        // read of an absent file: ok but exists=false
        let miss = perform_state_read(&ctx, "nope.json");
        assert!(miss.ok && !miss.exists);
        // a second big write over cap is refused
        let over = perform_state_write(&ctx, "big.json", "0123456789ABCDEF01"); // 18 > 16
        assert!(!over.ok);
        assert!(over.error.contains("quota"));
    }

    #[test]
    fn state_write_rejects_traversal() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with(&[], root.path().to_path_buf());
        let w = perform_state_write(&ctx, "../escape", "x");
        assert!(!w.ok);
        assert!(w.error.contains("traversal"));
    }

    #[test]
    fn file_read_denied_when_not_allowlisted() {
        let ctx = ctx_with(&[], std::env::temp_dir());
        let r = perform_file_read(&ctx, "/etc/hostname");
        assert!(!r.ok);
        assert!(r.error.contains("not allowed"));
    }

    #[test]
    fn file_write_denied_when_not_allowlisted() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("should_not_be_written.txt");
        let ctx = ctx_with(&[], std::env::temp_dir());
        let w = perform_file_write(&ctx, target.to_str().unwrap(), "secret");
        assert!(!w.ok);
        assert!(w.error.contains("not allowed"));
        assert!(!target.exists(), "denied write must not create the file");
    }

    #[test]
    fn cached_denied_url_makes_no_request_and_no_cache_file() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with(&[], root.path().to_path_buf()); // empty allowlist
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let f = CountingFetcher {
            calls: calls.clone(),
            status: 200,
            body: "x",
        };
        let r = perform_http_get_cached(
            &ctx,
            "https://wttr.in/48183",
            1800,
            "2026-07-20T12:00:00-04:00",
            &f,
        );
        assert!(!r.ok);
        assert!(r.error.contains("not allowed"));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "denied url must not hit the network"
        );
        // gate-first: no cache dir/file was created either
        assert!(!ctx.state_dir().join("__http_cache__").exists());
    }

    #[test]
    fn cached_first_fetch_populates_then_serves_within_ttl_without_refetch() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with_cap(&["https://wttr.in/*"], root.path().to_path_buf(), 1_000_000);
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let f = CountingFetcher {
            calls: calls.clone(),
            status: 200,
            body: "sunny-72",
        };
        let url = "https://wttr.in/48183";
        // first call at T0: fetch + cache
        let r1 = perform_http_get_cached(&ctx, url, 1800, "2026-07-20T12:00:00-04:00", &f);
        assert!(r1.ok && !r1.stale);
        assert_eq!(r1.body, "sunny-72");
        // second call 10 min later: served from cache, NO new fetch
        let r2 = perform_http_get_cached(&ctx, url, 1800, "2026-07-20T12:10:00-04:00", &f);
        assert!(r2.ok && !r2.stale);
        assert_eq!(r2.body, "sunny-72");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "exactly one network call within the window"
        );
        assert_eq!(r2.age_secs, 600);
    }

    #[test]
    fn cached_expired_ttl_refetches() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with_cap(&["https://wttr.in/*"], root.path().to_path_buf(), 1_000_000);
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let f = CountingFetcher {
            calls: calls.clone(),
            status: 200,
            body: "b",
        };
        let url = "https://wttr.in/48183";
        perform_http_get_cached(&ctx, url, 1800, "2026-07-20T12:00:00-04:00", &f);
        // 2h later -> expired -> refetch
        perform_http_get_cached(&ctx, url, 1800, "2026-07-20T14:00:00-04:00", &f);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn cached_transport_failure_serves_stale_then_empty() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with_cap(&["https://wttr.in/*"], root.path().to_path_buf(), 1_000_000);
        let url = "https://wttr.in/48183";
        // seed a good entry via a live fetch
        let live = FakeFetcher(200, "good-55");
        perform_http_get_cached(&ctx, url, 1800, "2026-07-20T09:00:00-04:00", &live);
        // 6h later fetch fails -> serve stale
        let r = perform_http_get_cached(&ctx, url, 1800, "2026-07-20T15:00:00-04:00", &DeadFetcher);
        assert!(r.ok && r.stale, "stale served on failure: {r:?}");
        assert_eq!(r.body, "good-55");
        assert!(r.age_secs > 0);
        // a *different* url that has never been cached -> not ok
        let miss = perform_http_get_cached(
            &ctx,
            "https://wttr.in/90210",
            1800,
            "2026-07-20T15:00:00-04:00",
            &DeadFetcher,
        );
        assert!(!miss.ok);
    }

    #[test]
    fn cached_non_2xx_does_not_overwrite_good_entry() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with_cap(&["https://wttr.in/*"], root.path().to_path_buf(), 1_000_000);
        let url = "https://wttr.in/48183";
        perform_http_get_cached(
            &ctx,
            url,
            1800,
            "2026-07-20T09:00:00-04:00",
            &FakeFetcher(200, "good"),
        );
        // expired + a 500 response -> must NOT cache the error; serves the good stale body
        let r = perform_http_get_cached(
            &ctx,
            url,
            1,
            "2026-07-20T12:00:00-04:00",
            &FakeFetcher(500, "error-page"),
        );
        assert!(r.ok && r.stale);
        assert_eq!(r.body, "good");
    }

    #[test]
    fn cached_write_over_quota_still_returns_body() {
        let root = tempfile::tempdir().unwrap();
        // ctx_with sets max_state_bytes = 16; a 40-byte body can't be cached
        let ctx = ctx_with(&["https://wttr.in/*"], root.path().to_path_buf());
        let body = "0123456789012345678901234567890123456789"; // 40 bytes > 16
        let r = perform_http_get_cached(
            &ctx,
            "https://wttr.in/48183",
            1800,
            "2026-07-20T12:00:00-04:00",
            &FakeFetcher(200, body),
        );
        assert!(r.ok, "fetched body is returned even if it can't be cached");
        assert_eq!(r.body, body);
        // nothing persisted -> a second call refetches (cache miss)
        assert!(
            crate::cache::read_entry(&crate::cache::cache_path(
                &ctx.state_dir(),
                "https://wttr.in/48183"
            ))
            .is_none()
        );
    }
}
