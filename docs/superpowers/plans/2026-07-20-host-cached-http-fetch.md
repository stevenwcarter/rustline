# Host-Managed Cached HTTP Fetch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a capability-gated `rl_http_get_cached` host function that owns TTL fetch/cache/persistence/serve-stale, and collapse the `weather` plugin onto it.

**Architecture:** New pure cache helpers (`cache.rs`) + a sixth effect function (`perform_http_get_cached` in `perform.rs`) reusing the existing `Fetcher`, `CapabilityCtx`, and `check_cap` quota. A new wire type `CachedHttpResult` (`abi.rs`) and a `host_fn!` wrapper (`host.rs`). The weather guest drops all self-managed caching and calls the one host function.

**Tech Stack:** Rust edition 2024, Extism/wasmtime host, `ureq` (rustls) fetch seam, `serde`/`serde_json`, `chrono` RFC3339 parsing, `walkdir` quota accounting.

## Global Constraints

- Edition 2024 in every crate; keep all crate editions equal to `rustfmt.toml`.
- Must stay clippy-clean (`cargo clippy --all-targets -- -D warnings`) and rustfmt-clean (`cargo fmt --all --check`). No pre-commit hook — run `cargo fmt --all` before committing.
- rustls-only: no `openssl`/`native-tls` may enter the graph. `chrono` added as a normal dep must use `default-features = false` (no extra transitive TLS/backends).
- Commit `Cargo.lock` alongside any dependency change.
- `cargo test --workspace` must stay hermetic (no wasm toolchain). WASM e2e stays behind the `wasm-e2e` feature (`just test-wasm`).
- Invariants (re-verify): **N1** every network effect gated (gate-first, denied → no fetcher call); **N2** a plugin never breaks the bar; **N3** state writes quota-bounded (`check_cap`); **N4** per-plugin scope (cache under the instance's own `state_dir()`); **#1** `Context` is the sole render input (guest forwards `Context.now`).

---

## Task 1: Pure cache helpers (`cache.rs`) + `chrono` dependency

**Files:**
- Create: `crates/rustline-wasm/src/cache.rs`
- Modify: `crates/rustline-wasm/src/lib.rs` (add `pub mod cache;`)
- Modify: `crates/rustline-wasm/Cargo.toml` (promote `chrono` to a normal dep)
- Test: inline `#[cfg(test)] mod tests` in `cache.rs`

**Interfaces:**
- Produces:
  - `pub struct CacheEntry { pub fetched_at: String, pub status: u16, pub body: String }` (`Serialize + Deserialize`)
  - `pub fn cache_path(state_dir: &std::path::Path, url: &str) -> std::path::PathBuf`
  - `pub fn age_secs(now_rfc3339: &str, fetched_rfc3339: &str) -> Option<i64>`
  - `pub fn is_fresh(age_secs: i64, ttl_secs: i64) -> bool`
  - `pub fn read_entry(path: &std::path::Path) -> Option<CacheEntry>`
  - `pub fn write_entry(state_dir: &std::path::Path, path: &std::path::Path, content: &str, cap: u64) -> Result<(), String>`

- [ ] **Step 1: Add `chrono` as a normal dependency**

In `crates/rustline-wasm/Cargo.toml`, under `[dependencies]` (leave the existing `[dev-dependencies]` `chrono` line as-is; cargo unifies the `clock` feature for tests):

```toml
chrono = { version = "0.4", default-features = false }
```

- [ ] **Step 2: Write the failing tests**

Create `crates/rustline-wasm/src/cache.rs` with only the tests first (helpers referenced but not yet defined):

```rust
//! Pure helpers for the host-managed HTTP response cache: cache-file path
//! derivation (FNV-1a of the URL), RFC3339 freshness, and quota-bounded
//! entry read/write. Used by `perform_http_get_cached`.

use std::path::{Path, PathBuf};

use chrono::DateTime;
use serde::{Deserialize, Serialize};

use crate::state::check_cap;

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
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p rustline-wasm cache:: 2>&1 | tail -20`
Expected: FAIL — `cannot find function ...`/`cannot find type CacheEntry`.

- [ ] **Step 4: Implement the helpers**

Insert above the `#[cfg(test)]` module in `cache.rs`:

```rust
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
```

Then add to `crates/rustline-wasm/src/lib.rs` in the module list (keep alphabetical placement after `allow`):

```rust
pub mod cache;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rustline-wasm cache:: 2>&1 | tail -20`
Expected: PASS (3 tests).

- [ ] **Step 6: Lint + commit**

```bash
cargo fmt --all
cargo clippy -p rustline-wasm --all-targets -- -D warnings
git add crates/rustline-wasm/src/cache.rs crates/rustline-wasm/src/lib.rs crates/rustline-wasm/Cargo.toml Cargo.lock
git commit -m "feat(wasm): pure HTTP-cache helpers (path, freshness, quota rw)

Claude-Session: https://claude.ai/code/session_01BGumPU94zWKj1fnzW2Vdzj"
```

---

## Task 2: `CachedHttpResult` wire type + `perform_http_get_cached`

**Files:**
- Modify: `crates/rustline-wasm/src/abi.rs` (add `CachedHttpResult`)
- Modify: `crates/rustline-wasm/src/perform.rs` (add `perform_http_get_cached` + tests)

**Interfaces:**
- Consumes: `cache::{CacheEntry, cache_path, age_secs, is_fresh, read_entry, write_entry}` (Task 1); `CapabilityCtx::{allowed_urls, state_dir, max_state_bytes}`; `fetch::Fetcher`.
- Produces:
  - `pub struct CachedHttpResult { pub ok: bool, pub status: u16, pub body: String, pub error: String, pub stale: bool, pub age_secs: i64 }` (`Default + Serialize + Deserialize`)
  - `pub fn perform_http_get_cached(ctx: &CapabilityCtx, url: &str, ttl_secs: i64, now: &str, fetcher: &dyn Fetcher) -> CachedHttpResult`

- [ ] **Step 1: Add the wire type**

In `crates/rustline-wasm/src/abi.rs`, after `WriteResult`:

```rust
/// Result of a TTL-cached HTTP GET. `ok` means "a usable body is present"
/// (fresh OR stale), not "transport succeeded"; `stale` distinguishes them.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CachedHttpResult {
    pub ok: bool,
    pub status: u16,
    pub body: String,
    pub error: String,
    pub stale: bool,
    pub age_secs: i64,
}
```

- [ ] **Step 2: Write the failing tests**

In `crates/rustline-wasm/src/perform.rs`, extend the `#[cfg(test)] mod tests`. Add a call-counting fetcher and a status-controllable fetcher near the existing fakes:

```rust
use std::sync::atomic::{AtomicUsize, Ordering};

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
```

Then the behavior tests:

```rust
#[test]
fn cached_denied_url_makes_no_request_and_no_cache_file() {
    let root = tempfile::tempdir().unwrap();
    let ctx = ctx_with(&[], root.path().to_path_buf()); // empty allowlist
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let f = CountingFetcher { calls: calls.clone(), status: 200, body: "x" };
    let r = perform_http_get_cached(&ctx, "https://wttr.in/48183", 1800, "2026-07-20T12:00:00-04:00", &f);
    assert!(!r.ok);
    assert!(r.error.contains("not allowed"));
    assert_eq!(calls.load(Ordering::SeqCst), 0, "denied url must not hit the network");
    // gate-first: no cache dir/file was created either
    assert!(!ctx.state_dir().join("__http_cache__").exists());
}

#[test]
fn cached_first_fetch_populates_then_serves_within_ttl_without_refetch() {
    let root = tempfile::tempdir().unwrap();
    let ctx = ctx_with(&["https://wttr.in/*"], root.path().to_path_buf());
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let f = CountingFetcher { calls: calls.clone(), status: 200, body: "sunny-72" };
    let url = "https://wttr.in/48183";
    // first call at T0: fetch + cache
    let r1 = perform_http_get_cached(&ctx, url, 1800, "2026-07-20T12:00:00-04:00", &f);
    assert!(r1.ok && !r1.stale);
    assert_eq!(r1.body, "sunny-72");
    // second call 10 min later: served from cache, NO new fetch
    let r2 = perform_http_get_cached(&ctx, url, 1800, "2026-07-20T12:10:00-04:00", &f);
    assert!(r2.ok && !r2.stale);
    assert_eq!(r2.body, "sunny-72");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "exactly one network call within the window");
    assert_eq!(r2.age_secs, 600);
}

#[test]
fn cached_expired_ttl_refetches() {
    let root = tempfile::tempdir().unwrap();
    let ctx = ctx_with(&["https://wttr.in/*"], root.path().to_path_buf());
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let f = CountingFetcher { calls: calls.clone(), status: 200, body: "b" };
    let url = "https://wttr.in/48183";
    perform_http_get_cached(&ctx, url, 1800, "2026-07-20T12:00:00-04:00", &f);
    // 2h later -> expired -> refetch
    perform_http_get_cached(&ctx, url, 1800, "2026-07-20T14:00:00-04:00", &f);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn cached_transport_failure_serves_stale_then_empty() {
    let root = tempfile::tempdir().unwrap();
    let ctx = ctx_with(&["https://wttr.in/*"], root.path().to_path_buf());
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
    let miss = perform_http_get_cached(&ctx, "https://wttr.in/90210", 1800, "2026-07-20T15:00:00-04:00", &DeadFetcher);
    assert!(!miss.ok);
}

#[test]
fn cached_non_2xx_does_not_overwrite_good_entry() {
    let root = tempfile::tempdir().unwrap();
    let ctx = ctx_with(&["https://wttr.in/*"], root.path().to_path_buf());
    let url = "https://wttr.in/48183";
    perform_http_get_cached(&ctx, url, 1800, "2026-07-20T09:00:00-04:00", &FakeFetcher(200, "good"));
    // expired + a 500 response -> must NOT cache the error; serves the good stale body
    let r = perform_http_get_cached(&ctx, url, 1, "2026-07-20T12:00:00-04:00", &FakeFetcher(500, "error-page"));
    assert!(r.ok && r.stale);
    assert_eq!(r.body, "good");
}

#[test]
fn cached_write_over_quota_still_returns_body() {
    let root = tempfile::tempdir().unwrap();
    // ctx_with sets max_state_bytes = 16; a 40-byte body can't be cached
    let ctx = ctx_with(&["https://wttr.in/*"], root.path().to_path_buf());
    let body = "0123456789012345678901234567890123456789"; // 40 bytes > 16
    let r = perform_http_get_cached(&ctx, "https://wttr.in/48183", 1800, "2026-07-20T12:00:00-04:00", &FakeFetcher(200, body));
    assert!(r.ok, "fetched body is returned even if it can't be cached");
    assert_eq!(r.body, body);
    // nothing persisted -> a second call refetches (cache miss)
    assert!(crate::cache::read_entry(&crate::cache::cache_path(&ctx.state_dir(), "https://wttr.in/48183")).is_none());
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p rustline-wasm perform::tests::cached 2>&1 | tail -20`
Expected: FAIL — `cannot find function perform_http_get_cached`.

- [ ] **Step 4: Implement `perform_http_get_cached`**

At the top of `crates/rustline-wasm/src/perform.rs`, extend imports:

```rust
use crate::abi::{CachedHttpResult, HttpResult, ReadResult, WriteResult};
use crate::cache::{CacheEntry, age_secs, cache_path, is_fresh, read_entry, write_entry};
```

Add the function (below `perform_http_get`):

```rust
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
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rustline-wasm perform:: 2>&1 | tail -20`
Expected: PASS (existing perform tests + the 6 new `cached_*` tests).

- [ ] **Step 6: Lint + commit**

```bash
cargo fmt --all
cargo clippy -p rustline-wasm --all-targets -- -D warnings
git add crates/rustline-wasm/src/abi.rs crates/rustline-wasm/src/perform.rs
git commit -m "feat(wasm): perform_http_get_cached + CachedHttpResult wire type

Gate-first TTL-cached GET: fresh-hit serve, 2xx-only caching, serve-stale
on failed/non-2xx refresh. All logic host-side and unit-tested.

Claude-Session: https://claude.ai/code/session_01BGumPU94zWKj1fnzW2Vdzj"
```

---

## Task 3: Host-function wiring (`rl_http_get_cached`)

**Files:**
- Modify: `crates/rustline-wasm/src/host.rs` (add the `host_fn!` + register it)

**Interfaces:**
- Consumes: `perform::perform_http_get_cached` (Task 2); `fetch::UreqFetcher`.
- Produces: a registered Extism host function `rl_http_get_cached(url: String, ttl_secs: String, now: String) -> String` bound to each plugin's `CapabilityCtx`.

- [ ] **Step 1: Add the `host_fn!` wrapper**

In `crates/rustline-wasm/src/host.rs`, extend the `perform::` import and add the wrapper after `rl_http_get`:

```rust
use crate::perform::{
    perform_file_read, perform_file_write, perform_http_get, perform_http_get_cached,
    perform_state_read, perform_state_write,
};
```

```rust
host_fn!(rl_http_get_cached(user_data: CapabilityCtx; url: String, ttl_secs: String, now: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    let ttl: i64 = ttl_secs.parse().unwrap_or(0);
    Ok(json(&perform_http_get_cached(&ctx, &url, ttl, &now, &UreqFetcher)))
});
```

- [ ] **Step 2: Register it in `build_plugin`**

In `build_plugin`, add the registration alongside the others (after `rl_http_get`):

```rust
        .with_function(
            "rl_http_get_cached",
            [PTR, PTR, PTR],
            [PTR],
            ud.clone(),
            rl_http_get_cached,
        )
```

Also update the doc-comment on `build_plugin`: "five capability-gated host functions" → "six capability-gated host functions".

- [ ] **Step 3: Verify it compiles and the hermetic suite still passes**

Run: `cargo test --workspace 2>&1 | grep -E "test result:|error\[" | tail -20`
Expected: all green, no errors. (No new unit test here — this glue is proven end-to-end in Task 5.)

- [ ] **Step 4: Lint + commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add crates/rustline-wasm/src/host.rs
git commit -m "feat(wasm): register rl_http_get_cached host function

Claude-Session: https://claude.ai/code/session_01BGumPU94zWKj1fnzW2Vdzj"
```

---

## Task 4: Collapse the weather guest onto the host cache

**Files:**
- Modify: `plugins/weather/src/lib.rs` (remove self-caching; use `rl_http_get_cached`)

**Interfaces:**
- Consumes: the runtime host import `rl_http_get_cached(url, ttl_secs, now) -> String` (Task 3). The guest compiles standalone — it only declares the extern import.
- Keeps unchanged: `code_to_icon`, `render_format`, `parse_wttr`, `Wttr` and their unit tests.

- [ ] **Step 1: Remove the guest-side cache logic and the `is_fresh` test**

Delete from `plugins/weather/src/lib.rs`:
- the `pub fn is_fresh(...) -> bool { ... }` function (lines ~35-55) and its doc comment,
- the `#[test] fn freshness_respects_interval_and_zip()` test.

(`chrono::DateTime` and `use chrono::DateTime;` become unused once `is_fresh` is gone — remove the `use chrono::DateTime;` import too. `serde::Deserialize` is still used by `WttrJson`, keep it.)

- [ ] **Step 2: Rewrite the guest `render` + imports**

In `mod guest`, replace the three host imports:

```rust
    #[host_fn]
    extern "ExtismHost" {
        fn rl_http_get_cached(url: String, ttl_secs: String, now: String) -> String;
    }
```

Replace `render` (and delete `read_cache`/`write_cache`; keep `segment`):

```rust
    #[plugin_fn]
    pub fn render(input: String) -> FnResult<Json<Vec<Segment>>> {
        let v: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
        let now = v["context"]["now"].as_str().unwrap_or_default().to_string();
        let cfg = &v["config"];
        let zip = cfg["zip"].as_str().unwrap_or("48183").to_string();
        let format = cfg["format"]
            .as_str()
            .unwrap_or("{icon} {temp_f}°F")
            .to_string();
        let refresh_secs = cfg["refresh_secs"].as_i64().unwrap_or(1800);
        let api_base = cfg["api_base"]
            .as_str()
            .unwrap_or("https://wttr.in")
            .to_string();

        // The host owns the TTL cache: fetch at most once per refresh_secs,
        // serving a fresh or last-good-stale body. Keyed by URL, so a zip
        // change is a different cache entry (no cross-zip leakage).
        let url = format!("{api_base}/{zip}?format=j1");
        let seg = unsafe { rl_http_get_cached(url, refresh_secs.to_string(), now) }
            .ok()
            .and_then(|raw| {
                let r: Value = serde_json::from_str(&raw).ok()?;
                if r["ok"].as_bool().unwrap_or(false) {
                    parse_wttr(r["body"].as_str().unwrap_or_default())
                } else {
                    None
                }
            })
            .map(|w| segment(&format, &w.code, &w.temp_f, &w.desc, &zip))
            .unwrap_or_default();
        Ok(Json(seg))
    }
```

- [ ] **Step 3: Verify the host-target unit tests still pass**

Run: `cargo test --manifest-path plugins/weather/Cargo.toml 2>&1 | tail -15`
Expected: PASS — `icon_maps_known_and_unknown_codes`, `format_substitutes_placeholders_and_passes_unknowns`, `parse_wttr_extracts_current_condition` (the `freshness_*` test is gone).

- [ ] **Step 4: Verify the guest still builds to wasm**

Run: `just build-weather 2>&1 | tail -15`
Expected: builds `weather.wasm` with no errors/warnings and installs it into the plugin dir.

- [ ] **Step 5: Lint + commit**

```bash
cargo fmt --manifest-path plugins/weather/Cargo.toml
cargo clippy --manifest-path plugins/weather/Cargo.toml --target wasm32-unknown-unknown -- -D warnings
git add plugins/weather/src/lib.rs plugins/weather/Cargo.lock
git commit -m "refactor(weather): use host rl_http_get_cached; drop self-managed cache

Removes is_fresh/read_cache/write_cache and the rl_http_get/rl_state_*
imports; render() is now build-url -> cached-get -> parse -> format.

Claude-Session: https://claude.ai/code/session_01BGumPU94zWKj1fnzW2Vdzj"
```

---

## Task 5: Rewrite the opt-in WASM e2e tests for the host cache

**Files:**
- Modify: `crates/rustline-wasm/tests/e2e.rs`
- Modify: `crates/rustline/tests/wasm_wiring.rs`

**Interfaces:**
- Consumes: the real `weather.wasm` built in Task 4; `WasmWidget`, `build_plugin`, `CapabilityCtx`.

- [ ] **Step 1: Rewrite `e2e.rs` stale + cross-zip tests**

Keep `caches_within_refresh_window_one_http_call` as-is (it already proves the host cache: two in-window renders → 1 hit). Replace the two seeding-based tests. Add a one-shot mock helper near `spawn_mock`:

```rust
/// A mock that serves exactly one successful response then closes its
/// listener, so a later fetch to the same URL fails (connection refused).
fn spawn_mock_once() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Some(Ok(mut s)) = listener.incoming().next() {
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                WTTR_BODY.len(),
                WTTR_BODY
            );
            let _ = s.write_all(resp.as_bytes());
        }
        // listener dropped here -> port closed
    });
    format!("http://{addr}")
}
```

Delete the `seed_cache` helper and both `stale_cache_used_when_fetch_fails` /
`cross_zip_fetch_fails_renders_empty` bodies, replacing them with:

```rust
#[test]
fn stale_cache_served_when_refresh_fails() {
    let state = tempfile::tempdir().unwrap();
    let base = spawn_mock_once();
    let w = build_widget(&base, state.path().to_path_buf(), "48183");
    // T0: fetch + cache (temp 72)
    let s1 = w.render(&ctx_now("2026-07-20T12:00:00-04:00"));
    assert!(s1[0].text.contains("72"), "first render fetched: {s1:?}");
    // 6h later: cache expired, endpoint now dead -> host serves stale
    let s2 = w.render(&ctx_now("2026-07-20T18:00:00-04:00"));
    assert!(s2[0].text.contains("72"), "stale body served on refresh failure: {s2:?}");
}

#[test]
fn cross_zip_isolation_no_leak() {
    let state = tempfile::tempdir().unwrap();
    let base = spawn_mock(Arc::new(AtomicUsize::new(0)));
    // widget A (48183) fetches + caches into the shared state root
    let a = build_widget(&base, state.path().to_path_buf(), "48183");
    let sa = a.render(&ctx_now("2026-07-20T12:00:00-04:00"));
    assert!(sa[0].text.contains("72"));
    // widget B (90210) points at a dead endpoint but shares the state root.
    // Its URL (different zip AND host) has no cache entry -> empty. It must
    // never surface A's cached entry.
    let b = build_widget("http://127.0.0.1:1", state.path().to_path_buf(), "90210");
    let sb = b.render(&ctx_now("2026-07-20T12:05:00-04:00"));
    assert!(sb.is_empty(), "no entry for 90210 + failed fetch -> empty: {sb:?}");
}
```

- [ ] **Step 2: Rewrite `wasm_wiring.rs` to use an allowlisted in-process mock**

The old test pre-seeded an unallowlisted `weather.json`; gate-first semantics
now (correctly) refuse to serve a body without a URL grant, so prove wiring with
one real fetch to a local mock instead. Replace the whole test body:

```rust
#![cfg(feature = "wasm-e2e")]
//! Positive end-to-end proof that plugin registration is wired into the
//! `rustline` binary: renders a real `weather.wasm` through
//! `main.rs -> register_plugins -> WasmWidget -> guest`, which makes one
//! capability-allowed fetch to an in-process mock and renders the temp.
//! Run via `just test-wasm` (needs `just build-weather` first).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;

const WTTR_BODY: &str = r#"{"current_condition":[{"temp_F":"72","weatherCode":"113","weatherDesc":[{"value":"Sunny"}]}]}"#;

fn spawn_mock() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                WTTR_BODY.len(),
                WTTR_BODY
            );
            let _ = s.write_all(resp.as_bytes());
        }
    });
    format!("http://{addr}")
}

#[test]
fn render_right_with_weather_plugin_fetches_and_renders_temp() {
    let wasm_src = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/weather/target/wasm32-unknown-unknown/release/weather.wasm"
    );
    if !std::path::Path::new(wasm_src).exists() {
        panic!("run `just build-weather` first");
    }

    let base = spawn_mock();
    let tmp = tempfile::tempdir().unwrap();
    let cfg_home = tmp.path().join("cfg");
    let data_home = tmp.path().join("data");

    let cfg_dir = cfg_home.join("rustline");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("config.toml"),
        format!(
            r#"[layout]
right = ["weather"]
[plugins.weather]
allowed_urls = ["http://127.0.0.1:*/*"]
[plugins.weather.options]
zip = "48183"
format = "{{temp_f}}"
api_base = "{base}"
"#
        ),
    )
    .unwrap();

    let plugin_dir = data_home.join("rustline").join("plugins");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::copy(wasm_src, plugin_dir.join("weather.wasm")).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_rustline"))
        .args(["render", "right"])
        .env("XDG_CONFIG_HOME", &cfg_home)
        .env("XDG_DATA_HOME", &data_home)
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("72"),
        "temp rendered via register_plugins -> WasmWidget -> guest -> cached fetch: {stdout}"
    );
}
```

- [ ] **Step 3: Run the opt-in wasm e2e suite**

Run: `just test-wasm 2>&1 | tail -30`
Expected: PASS — `caches_within_refresh_window_one_http_call`,
`stale_cache_served_when_refresh_fails`, `cross_zip_isolation_no_leak`, and
`render_right_with_weather_plugin_fetches_and_renders_temp`.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
git add crates/rustline-wasm/tests/e2e.rs crates/rustline/tests/wasm_wiring.rs
git commit -m "test(wasm): rewrite e2e + wiring for host-owned cache

Stale/cross-zip use live/one-shot mocks instead of seeding the guest's
old cache format; wiring test proves gate-first fetch through an
allowlisted in-process mock.

Claude-Session: https://claude.ai/code/session_01BGumPU94zWKj1fnzW2Vdzj"
```

---

## Task 6: Documentation

**Files:**
- Modify: `CLAUDE.md`, `README.md`, `crates/rustline-wasm/Cargo.toml` (comment), `TODO.md`

- [ ] **Step 1: Update `CLAUDE.md`**

- Architecture (`crates/rustline-wasm` bullet): "five capability-gated host functions (network + state + arbitrary-file read/write)" → "six capability-gated host functions (TTL-cached + raw network + state + arbitrary-file read/write)".
- Module map `abi.rs`: add `CachedHttpResult` to the wire-types list.
- Add a `cache.rs` module-map line: "`cache.rs` — pure HTTP-response-cache helpers: FNV-1a URL→path, RFC3339 freshness (`age_secs`/`is_fresh`), quota-bounded `read_entry`/`write_entry`."
- Module map `perform.rs`: "the five capability-checked effect functions" → "six", and add `perform_http_get_cached` (the TTL-cached GET: gate-first, 2xx-only caching, serve-stale) to the parenthetical list.
- Module map `host.rs`: add `rl_http_get_cached` to the host_fn list.
- `plugins/weather` bullet: replace the `rl_http_get`/`rl_state_read`/`rl_state_write` guest-imports note with "a single `rl_http_get_cached` guest import (the host owns the TTL cache); pure logic `code_to_icon`/`render_format`/`parse_wttr`".
- Invariant **N1**: append a sentence — "The TTL-cached GET (`rl_http_get_cached`) gates `allowed_urls` before any fetch (gate-first: a denied URL makes no network call and touches no cache), with its own denied-case test."
- Development / rustls paragraph and line 280: "`rl_http_get` is the only network path" → "`rl_http_get` and `rl_http_get_cached` are the only network paths".
- Roadmap "Done" line: append "plus a host-managed TTL-cached fetch capability (`rl_http_get_cached`) that plugins use instead of hand-rolling caches."

- [ ] **Step 2: Update `crates/rustline-wasm/Cargo.toml` comment**

Line ~18: "our `rl_http_get` is the only network path." → "our `rl_http_get` / `rl_http_get_cached` are the only network paths."

- [ ] **Step 3: Update `README.md`**

In the Plugins section, after the sandboxed-state-dir sentence, add: "The host also exposes a TTL-cached HTTP GET, so a plugin can fetch remote data at most once per interval without managing its own cache — the bundled `weather` example uses it." (The existing weather description stays accurate.)

- [ ] **Step 4: Check off `TODO.md` item #1**

Change the first line of `TODO.md` from `- [ ] Move the logic to manage a TTL-based state ...` to `- [x] Move the logic to manage a TTL-based state ...`.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md README.md crates/rustline-wasm/Cargo.toml TODO.md
git commit -m "docs: document rl_http_get_cached capability; check off TODO #1

Claude-Session: https://claude.ai/code/session_01BGumPU94zWKj1fnzW2Vdzj"
```

---

## Self-Review

**Spec coverage:**
- Host fn `rl_http_get_cached` + `CachedHttpResult` → Tasks 2, 3. ✓
- Gate-first / fresh-hit / 2xx-only / serve-stale semantics → Task 2 impl + tests. ✓
- FNV-1a URL keying, `__http_cache__/` storage, quota reuse → Task 1. ✓
- Guest collapse (drop is_fresh/read_cache/write_cache/imports) → Task 4. ✓
- All 9 host test cases from the spec → Task 1 (freshness) + Task 2 (8 behavior cases incl. denied/within-ttl/expired/stale/no-entry/non-2xx/quota). ✓
- e2e + wiring rewrite → Task 5. ✓
- Docs (CLAUDE.md/README/TODO + Cargo comment) → Task 6. ✓
- Invariants N1/N3/N4/#1 → asserted in Task 2 tests + Task 1 quota + guest forwards `Context.now`. ✓

**Placeholder scan:** none — every step has concrete code/commands.

**Type consistency:** `perform_http_get_cached(ctx, url, ttl_secs: i64, now: &str, fetcher)` consistent across Tasks 2/3; `CachedHttpResult` fields (`ok/status/body/error/stale/age_secs`) consistent across abi/perform/guest; `cache_path`/`age_secs`/`is_fresh`/`read_entry`/`write_entry`/`CacheEntry` signatures consistent between Task 1 definitions and Task 2 use. Host import name `rl_http_get_cached(url, ttl_secs, now)` matches guest extern in Task 4. ✓
