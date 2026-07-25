//! The capability-checked effect functions. Each returns a structured result
//! and never panics — the host_fn wrappers just serialize these to JSON.
//!
//! `perform_log` is the one exception to "capability-checked": it is
//! **capability-free by design** (see invariant N1) — it only ever writes to
//! the host's `tracing` subscriber, never the network or filesystem, so
//! there is no allowlist to check and no denied-case test for it.

use crate::abi::{
    CachedExecResult, CachedHttpResult, ExecResult, HttpResult, ReadResult, WriteResult,
};
use crate::argv::canonical_argv;
use crate::cache::{
    CacheEntry, EXEC_NAMESPACE, HTTP_NAMESPACE, age_secs, cache_path, is_fresh, read_entry,
    write_entry,
};
use crate::capability::{CapabilityCtx, DenialKind};
use crate::fetch::Fetcher;
use crate::run::{MAX_OUTPUT_BYTES, Runner};
use crate::state::{check_cap, normalize_abs, sanitize_relpath};

pub fn perform_http_get(ctx: &CapabilityCtx, url: &str, fetcher: &dyn Fetcher) -> HttpResult {
    if !ctx.allowed_urls.allows(url) {
        ctx.observe_denial(DenialKind::Url, url);
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
        ctx.observe_denial(DenialKind::Url, url);
        return CachedHttpResult {
            ok: false,
            error: format!("url not allowed: {url}"),
            ..Default::default()
        };
    }

    let dir = ctx.state_dir();
    let path = cache_path(&dir, HTTP_NAMESPACE, url);
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

/// Run a command on the guest's behalf, gated by `allowed_commands`.
///
/// **Gate first (invariant N1):** the whole canonical argv (program + args,
/// quoted — see `argv::canonical_argv`) is checked against the allowlist
/// before anything is spawned, so a denied call never touches `runner` at
/// all. The gate is on the *whole* argv, not just the program — a grant for
/// `git status*` does not cover `git push`. Once allowed, `program`/`args`
/// are passed straight through to `runner`, never assembled into a shell
/// string.
pub fn perform_exec(
    ctx: &CapabilityCtx,
    program: &str,
    args: &[String],
    runner: &dyn Runner,
) -> ExecResult {
    let candidate = canonical_argv(program, args);
    if !ctx.allowed_commands.allows(&candidate) {
        ctx.observe_denial(DenialKind::Command, &candidate);
        return ExecResult {
            ok: false,
            status: -1,
            error: format!("command not allowed: {candidate}"),
            ..Default::default()
        };
    }
    match runner.run(program, args) {
        Ok((status, stdout, stderr)) => ExecResult {
            ok: true,
            status,
            truncated: is_truncated(&stdout, &stderr),
            stdout,
            stderr,
            error: String::new(),
        },
        Err(error) => ExecResult {
            ok: false,
            status: -1,
            error,
            ..Default::default()
        },
    }
}

/// TTL-cached command run, closely mirroring [`perform_http_get_cached`]'s
/// shape with "2xx" replaced by "exit 0" — but with one deliberate
/// difference: a **non-zero exit is real, fresh data** (the command ran and
/// genuinely reported that status), not a transient failure, so unlike an
/// HTTP non-2xx it is returned as-is rather than triggering a fall back to a
/// stale cached entry. Only a run that couldn't happen at all (denied, spawn
/// failure, or timeout) falls back to the last-good entry, stale, if one
/// exists — a denied run touches no cache file at all, under its own
/// `EXEC_NAMESPACE` so it can never collide with the HTTP cache (see
/// `cache::cache_path`). A fresh cache hit is served without re-running; only
/// a **zero-exit** run is ever written to the cache.
pub fn perform_exec_cached(
    ctx: &CapabilityCtx,
    program: &str,
    args: &[String],
    ttl_secs: i64,
    now: &str,
    runner: &dyn Runner,
) -> CachedExecResult {
    let candidate = canonical_argv(program, args);
    if !ctx.allowed_commands.allows(&candidate) {
        ctx.observe_denial(DenialKind::Command, &candidate);
        return CachedExecResult {
            ok: false,
            status: -1,
            error: format!("command not allowed: {candidate}"),
            ..Default::default()
        };
    }

    let dir = ctx.state_dir();
    let path = cache_path(&dir, EXEC_NAMESPACE, &candidate);
    let entry = read_entry(&path);

    if let Some(e) = &entry
        && let Some(age) = age_secs(now, &e.fetched_at)
        && is_fresh(age, ttl_secs)
    {
        return CachedExecResult {
            ok: true,
            status: i32::from(e.status),
            stdout: e.body.clone(),
            // `CacheEntry` doesn't persist stderr (only the successful
            // stdout body is worth caching); a served entry always reports
            // it empty, fresh or stale.
            stderr: String::new(),
            error: String::new(),
            stale: false,
            age_secs: age,
            truncated: false,
        };
    }

    match runner.run(program, args) {
        Ok((0, stdout, stderr)) => {
            let content = serde_json::to_string(&CacheEntry {
                fetched_at: now.to_string(),
                status: 0,
                body: stdout.clone(),
            })
            .unwrap_or_default();
            if let Err(error) = write_entry(&dir, &path, &content, ctx.max_state_bytes) {
                tracing::warn!(%error, %candidate, "exec cache write failed; returning body unpersisted");
            }
            CachedExecResult {
                ok: true,
                status: 0,
                truncated: is_truncated(&stdout, &stderr),
                stdout,
                stderr,
                error: String::new(),
                stale: false,
                age_secs: 0,
            }
        }
        // Ran, but a non-zero exit is data, not an error: return it as-is
        // without caching it (only a successful run is worth serving stale
        // later).
        Ok((status, stdout, stderr)) => CachedExecResult {
            ok: true,
            status,
            truncated: is_truncated(&stdout, &stderr),
            stdout,
            stderr,
            error: String::new(),
            stale: false,
            age_secs: 0,
        },
        // Couldn't run at all (denied is already handled above; this is a
        // spawn failure or timeout): serve the last-good entry stale if one
        // exists, exactly like a failed HTTP refresh.
        Err(error) => match entry {
            Some(e) => {
                let age = age_secs(now, &e.fetched_at).unwrap_or(0);
                CachedExecResult {
                    ok: true,
                    status: i32::from(e.status),
                    stdout: e.body,
                    stderr: String::new(),
                    error,
                    stale: true,
                    age_secs: age,
                    truncated: false,
                }
            }
            None => CachedExecResult {
                ok: false,
                error,
                ..Default::default()
            },
        },
    }
}

/// True iff either stream hit [`MAX_OUTPUT_BYTES`] and was truncated. See
/// `run::MAX_OUTPUT_BYTES`'s doc comment for why comparing the lossily
/// UTF-8-converted string's length this way is still a safe (no
/// false-negative) signal.
fn is_truncated(stdout: &str, stderr: &str) -> bool {
    stdout.len() >= MAX_OUTPUT_BYTES || stderr.len() >= MAX_OUTPUT_BYTES
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
        ctx.observe_denial(DenialKind::Path, &norm);
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
        ctx.observe_denial(DenialKind::Path, &norm);
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

/// Emit a guest log message through the host's `tracing` subscriber.
///
/// Unlike every other function in this module, `rl_log` is the **one
/// intentional capability-free host function** (invariant N1): it never
/// touches the network or filesystem, so there is no `CapabilityCtx`
/// allowlist to check and — unlike `perform_http_get`/`perform_state_read`/
/// etc. — no "denied" case to test. `plugin` tags the log line so
/// multi-plugin output stays attributable; an unrecognized `level` string
/// degrades to `info` (keeping the original string as a field) rather than
/// dropping the message or panicking, matching invariant N2 (a plugin must
/// never break the bar).
pub fn perform_log(plugin: &str, level: &str, msg: &str) {
    match level {
        "error" => tracing::error!(target: "rustline_wasm::guest", %plugin, "{msg}"),
        "warn" => tracing::warn!(target: "rustline_wasm::guest", %plugin, "{msg}"),
        "info" => tracing::info!(target: "rustline_wasm::guest", %plugin, "{msg}"),
        "debug" => tracing::debug!(target: "rustline_wasm::guest", %plugin, "{msg}"),
        "trace" => tracing::trace!(target: "rustline_wasm::guest", %plugin, "{msg}"),
        _ => {
            tracing::info!(target: "rustline_wasm::guest", %plugin, orig_level = %level, "{msg}")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::capability::{CapabilityCtx, DenialKind, DenialObserver};
    use rustline_core::PluginConfig;

    /// A spy [`DenialObserver`] that records every `(plugin, kind, target)`
    /// it's told about, so a denied-case test can assert the seam fired
    /// alongside the existing `ok:false`/no-side-effect assertions.
    #[derive(Default, Clone)]
    struct SpyObserver(Arc<Mutex<Vec<(String, DenialKind, String)>>>);

    impl DenialObserver for SpyObserver {
        fn observe(&self, plugin: &str, kind: DenialKind, target: &str) {
            self.0
                .lock()
                .unwrap()
                .push((plugin.to_string(), kind, target.to_string()));
        }
    }

    impl SpyObserver {
        fn records(&self) -> Vec<(String, DenialKind, String)> {
            self.0.lock().unwrap().clone()
        }
    }

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
        let spy = SpyObserver::default();
        let ctx = ctx_with(&[], std::env::temp_dir()).with_observer(Arc::new(spy.clone()));
        let r = perform_http_get(&ctx, "https://wttr.in/48183", &FakeFetcher(200, "hi"));
        assert!(!r.ok);
        assert!(r.error.contains("not allowed"));
        assert_eq!(
            spy.records(),
            vec![(
                "weather".to_string(),
                DenialKind::Url,
                "https://wttr.in/48183".to_string()
            )]
        );
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
        let spy = SpyObserver::default();
        let ctx = ctx_with(&[], std::env::temp_dir()).with_observer(Arc::new(spy.clone()));
        let r = perform_file_read(&ctx, "/etc/hostname");
        assert!(!r.ok);
        assert!(r.error.contains("not allowed"));
        assert_eq!(
            spy.records(),
            vec![(
                "weather".to_string(),
                DenialKind::Path,
                "/etc/hostname".to_string()
            )]
        );
    }

    #[test]
    fn file_write_denied_when_not_allowlisted() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("should_not_be_written.txt");
        let spy = SpyObserver::default();
        let ctx = ctx_with(&[], std::env::temp_dir()).with_observer(Arc::new(spy.clone()));
        let w = perform_file_write(&ctx, target.to_str().unwrap(), "secret");
        assert!(!w.ok);
        assert!(w.error.contains("not allowed"));
        assert!(!target.exists(), "denied write must not create the file");
        assert_eq!(
            spy.records(),
            vec![(
                "weather".to_string(),
                DenialKind::Path,
                target.to_str().unwrap().to_string()
            )]
        );
    }

    #[test]
    fn cached_denied_url_makes_no_request_and_no_cache_file() {
        let root = tempfile::tempdir().unwrap();
        let spy = SpyObserver::default();
        let ctx = ctx_with(&[], root.path().to_path_buf()).with_observer(Arc::new(spy.clone())); // empty allowlist
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
        assert_eq!(
            spy.records(),
            vec![(
                "weather".to_string(),
                DenialKind::Url,
                "https://wttr.in/48183".to_string()
            )]
        );
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
                crate::cache::HTTP_NAMESPACE,
                "https://wttr.in/48183"
            ))
            .is_none()
        );
    }

    /// Fixed instants for the exec-cache TTL tests below, mirroring the
    /// literal RFC3339 strings the HTTP-cache tests above use directly:
    /// `LATER` is two hours after `NOW`, comfortably past every TTL those
    /// tests exercise (60s and 3600s).
    const NOW: &str = "2026-07-20T12:00:00-04:00";
    const LATER: &str = "2026-07-20T14:00:00-04:00";

    /// Records every run it is asked to perform, so a test can assert a
    /// denied call never reached the runner at all.
    struct RecordingRunner {
        calls: Mutex<Vec<(String, Vec<String>)>>,
        reply: Result<(i32, String, String), String>,
    }

    impl RecordingRunner {
        fn ok(status: i32, stdout: &str) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                reply: Ok((status, stdout.to_string(), String::new())),
            }
        }
        fn failing(message: &str) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                reply: Err(message.to_string()),
            }
        }
        fn calls(&self) -> Vec<(String, Vec<String>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Runner for RecordingRunner {
        fn run(&self, program: &str, args: &[String]) -> Result<(i32, String, String), String> {
            self.calls
                .lock()
                .unwrap()
                .push((program.to_string(), args.to_vec()));
            self.reply.clone()
        }
    }

    fn argv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// The command-allowlist counterpart to `ctx_with`/`ctx_with_cap` above:
    /// a `CapabilityCtx` whose `allowed_commands` is `patterns`, paired with a
    /// [`SpyObserver`] so a denied-case test can assert the denial fired.
    fn ctx_with_commands(patterns: &[&str]) -> (CapabilityCtx, SpyObserver) {
        ctx_with_commands_in(patterns, &std::env::temp_dir())
    }

    fn ctx_with_commands_in(
        patterns: &[&str],
        root: &std::path::Path,
    ) -> (CapabilityCtx, SpyObserver) {
        let pc = PluginConfig {
            allowed_commands: patterns.iter().map(|s| s.to_string()).collect(),
            ..PluginConfig::default()
        };
        let spy = SpyObserver::default();
        let ctx = CapabilityCtx::from_config("p", &pc, root.to_path_buf())
            .with_observer(Arc::new(spy.clone()));
        (ctx, spy)
    }

    #[test]
    fn exec_denied_never_reaches_the_runner_and_records_the_denial() {
        let (ctx, observer) = ctx_with_commands(&[]);
        let runner = RecordingRunner::ok(0, "should not happen");
        let out = perform_exec(&ctx, "playerctl", &argv(&["metadata"]), &runner);

        assert!(!out.ok);
        assert!(out.error.contains("playerctl metadata"), "{}", out.error);
        assert!(
            runner.calls().is_empty(),
            "gate-first: no spawn on a denied argv"
        );
        assert_eq!(
            observer.records(),
            vec![(
                "p".to_string(),
                DenialKind::Command,
                "playerctl metadata".to_string()
            )]
        );
    }

    #[test]
    fn exec_gates_on_the_whole_argv_not_just_the_program() {
        let (ctx, _o) = ctx_with_commands(&["git status*"]);
        let runner = RecordingRunner::ok(0, "");
        // Same program, different subcommand -> denied.
        let out = perform_exec(&ctx, "git", &argv(&["push", "--force"]), &runner);
        assert!(
            !out.ok,
            "a grant for `git status*` must not cover `git push`"
        );
        assert!(runner.calls().is_empty());

        let out = perform_exec(&ctx, "git", &argv(&["status", "--porcelain"]), &runner);
        assert!(out.ok);
        assert_eq!(runner.calls().len(), 1);
    }

    #[test]
    fn exec_allowed_returns_status_and_streams_verbatim() {
        let (ctx, _o) = ctx_with_commands(&["echo*"]);
        let runner = RecordingRunner::ok(0, "hello\n");
        let out = perform_exec(&ctx, "echo", &argv(&["hello"]), &runner);
        assert!(out.ok);
        assert_eq!(out.status, 0);
        assert_eq!(out.stdout, "hello\n");
        assert!(out.error.is_empty());
        // The spawn got the program and args untouched -- no shell in between.
        assert_eq!(runner.calls(), vec![("echo".to_string(), argv(&["hello"]))]);
    }

    #[test]
    fn exec_a_nonzero_exit_is_data_not_an_error() {
        let (ctx, _o) = ctx_with_commands(&["false*"]);
        let runner = RecordingRunner::ok(1, "");
        let out = perform_exec(&ctx, "false", &[], &runner);
        assert!(out.ok, "the process ran; `ok` is not about its exit code");
        assert_eq!(out.status, 1);
    }

    #[test]
    fn exec_a_spawn_failure_is_reported_without_panicking() {
        let (ctx, _o) = ctx_with_commands(&["missing*"]);
        let runner = RecordingRunner::failing("no such file");
        let out = perform_exec(&ctx, "missing", &[], &runner);
        assert!(!out.ok);
        assert_eq!(out.status, -1);
        assert!(out.error.contains("no such file"));
    }

    #[test]
    fn exec_cached_denied_touches_neither_the_runner_nor_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _o) = ctx_with_commands_in(&[], dir.path());
        let runner = RecordingRunner::ok(0, "x");
        let out = perform_exec_cached(&ctx, "date", &[], 60, NOW, &runner);
        assert!(!out.ok);
        assert!(runner.calls().is_empty());
        assert!(
            !ctx.state_dir().join(EXEC_NAMESPACE).exists(),
            "a denied call must not create the cache dir"
        );
    }

    #[test]
    fn exec_cached_serves_a_fresh_entry_without_rerunning() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _o) = ctx_with_commands_in(&["date*"], dir.path());
        let runner = RecordingRunner::ok(0, "first");
        let a = perform_exec_cached(&ctx, "date", &[], 3600, NOW, &runner);
        assert_eq!(a.stdout, "first");
        assert!(!a.stale);

        let runner2 = RecordingRunner::ok(0, "second");
        let b = perform_exec_cached(&ctx, "date", &[], 3600, NOW, &runner2);
        assert_eq!(b.stdout, "first", "served from cache");
        assert!(runner2.calls().is_empty(), "fresh hit must not re-run");
    }

    #[test]
    fn exec_cached_reruns_once_the_ttl_has_expired() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _o) = ctx_with_commands_in(&["date*"], dir.path());
        perform_exec_cached(&ctx, "date", &[], 60, NOW, &RecordingRunner::ok(0, "first"));

        let runner = RecordingRunner::ok(0, "second");
        let out = perform_exec_cached(&ctx, "date", &[], 60, LATER, &runner);
        assert_eq!(out.stdout, "second");
        assert_eq!(runner.calls().len(), 1);
    }

    #[test]
    fn exec_cached_does_not_cache_a_nonzero_exit() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _o) = ctx_with_commands_in(&["flaky*"], dir.path());
        perform_exec_cached(
            &ctx,
            "flaky",
            &[],
            3600,
            NOW,
            &RecordingRunner::ok(3, "bad"),
        );

        // Nothing was cached, so the next call runs again rather than serving `bad`.
        let runner = RecordingRunner::ok(0, "good");
        let out = perform_exec_cached(&ctx, "flaky", &[], 3600, NOW, &runner);
        assert_eq!(out.stdout, "good");
        assert_eq!(runner.calls().len(), 1);
    }

    #[test]
    fn exec_cached_serves_the_last_good_entry_stale_when_a_refresh_fails() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _o) = ctx_with_commands_in(&["date*"], dir.path());
        perform_exec_cached(&ctx, "date", &[], 60, NOW, &RecordingRunner::ok(0, "good"));

        let out = perform_exec_cached(
            &ctx,
            "date",
            &[],
            60,
            LATER,
            &RecordingRunner::failing("boom"),
        );
        assert!(out.ok, "a usable (stale) body is present");
        assert!(out.stale);
        assert_eq!(out.stdout, "good");
        assert!(out.age_secs > 0);
    }

    #[test]
    fn exec_and_http_caches_never_collide_on_the_same_key_string() {
        let dir = tempfile::tempdir().unwrap();
        let http = crate::cache::cache_path(dir.path(), HTTP_NAMESPACE, "same-key");
        let exec = crate::cache::cache_path(dir.path(), EXEC_NAMESPACE, "same-key");
        assert_ne!(http, exec);
    }
}

/// `perform_log` has no `CapabilityCtx` to pass through a fake `Fetcher`, so
/// these tests capture real `tracing` events with a minimal hand-rolled
/// `Subscriber` (no test-only dep needed — `Subscriber`/`Visit` are already
/// part of the `tracing` crate) rather than reusing the fixtures above.
#[cfg(test)]
mod log_tests {
    use std::sync::{Arc, Mutex};

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    use super::perform_log;

    /// One captured event: its severity plus every field (incl. the implicit
    /// `message` field carrying the formatted log text) as `(name, debug)`.
    type CapturedEvent = (Level, Vec<(String, String)>);

    #[derive(Default)]
    struct FieldVisitor(Vec<(String, String)>);

    impl Visit for FieldVisitor {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0
                .push((field.name().to_string(), format!("{value:?}")));
        }
    }

    /// A subscriber that accepts every event (no level filtering) and
    /// records it, purely so a test can assert what `perform_log` emitted.
    struct RecordingSubscriber(Arc<Mutex<Vec<CapturedEvent>>>);

    impl Subscriber for RecordingSubscriber {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut visitor = FieldVisitor::default();
            event.record(&mut visitor);
            self.0
                .lock()
                .unwrap()
                .push((*event.metadata().level(), visitor.0));
        }

        fn enter(&self, _span: &Id) {}
        fn exit(&self, _span: &Id) {}
    }

    /// Run `f` under a scoped recording subscriber and return every event it
    /// emitted, in order. Reads the shared buffer back out through the
    /// `Mutex` rather than `Arc::try_unwrap`-ing it: `tracing`'s dispatcher
    /// machinery can still be holding its own clone of the `Arc` briefly
    /// after `with_default` returns, so asserting a unique strong count here
    /// is not reliable.
    fn capture(f: impl FnOnce()) -> Vec<CapturedEvent> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscriber = RecordingSubscriber(events.clone());
        tracing::subscriber::with_default(subscriber, f);
        events.lock().unwrap().clone()
    }

    fn field<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
        fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn maps_each_known_level_string_to_the_matching_tracing_level() {
        for (level_str, expected) in [
            ("error", Level::ERROR),
            ("warn", Level::WARN),
            ("info", Level::INFO),
            ("debug", Level::DEBUG),
            ("trace", Level::TRACE),
        ] {
            let events = capture(|| perform_log("weather", level_str, "hello"));
            assert_eq!(events.len(), 1, "level {level_str}");
            let (level, fields) = &events[0];
            assert_eq!(*level, expected, "level {level_str}");
            assert_eq!(field(fields, "message"), Some("hello"));
            assert_eq!(field(fields, "plugin"), Some("weather"));
        }
    }

    #[test]
    fn unrecognized_level_degrades_to_info_without_panicking() {
        let events = capture(|| perform_log("weather", "bogus", "still logged"));
        assert_eq!(events.len(), 1);
        let (level, fields) = &events[0];
        assert_eq!(*level, Level::INFO);
        assert_eq!(field(fields, "message"), Some("still logged"));
        assert_eq!(field(fields, "orig_level"), Some("bogus"));
    }

    #[test]
    fn every_level_string_logs_with_no_network_or_filesystem_effect() {
        // N1: rl_log is the one intentional capability-free host fn. There is
        // no `CapabilityCtx` allowlist here to deny against, so — unlike the
        // other six `perform_*` functions — there is no "denied" case to
        // test; this just pins that every level (incl. an unknown one)
        // completes with no side effect beyond the emitted tracing event.
        for level in ["error", "warn", "info", "debug", "trace", "unknown"] {
            let events = capture(|| perform_log("no-capability-needed", level, "logged"));
            assert_eq!(events.len(), 1, "level {level}");
        }
    }
}
