# Host-managed cached HTTP fetch (`rl_http_get_cached`)

**Date:** 2026-07-20
**Status:** Approved (via `/ship-it --ask`)
**Scope:** TODO item #1 — move TTL/state bookkeeping out of the weather plugin
and into a capability-gated host function; simplify the example.

## Problem

Today the `weather` guest carries all of its own TTL cache bookkeeping:
`is_fresh()` (parse `fetched_at`, compare to `now` + `refresh_secs`, key on
zip), `read_cache()`/`write_cache()` (serialize a cache struct through
`rl_state_read`/`rl_state_write`), and a stale-on-failure fallback branch that
must itself re-check the zip to avoid showing one zip's weather under another's
label. That is ~70 lines of state-management logic every remote-data plugin
would have to re-implement. The host only exposes raw primitives
(`rl_http_get`, `rl_state_read`, `rl_state_write`).

## Goal

Add one host capability — a **TTL-cached HTTP GET** — that owns fetch +
freshness + persistence + serve-stale-on-failure. The weather guest collapses to
"build URL → call → parse → format," dropping `is_fresh`, `read_cache`,
`write_cache`, the `fetched_at` bookkeeping, and the stale-fallback branch. The
freshness logic moves host-side (essentially the old `is_fresh`, verbatim).

## The host function

```
rl_http_get_cached(url: String, ttl_secs: String, now: String) -> String
```

All three parameters are strings (`ttl_secs` and `now` parsed host-side),
matching the existing all-`PTR`/all-`String` host-function ABI exactly — no new
Extism value-type marshaling. `now` is RFC3339 (the guest forwards
`Context.now`); `ttl_secs` is the plugin's `refresh_secs`.

Returns a JSON-encoded:

```rust
pub struct CachedHttpResult {
    pub ok: bool,       // a usable body is present (fresh OR stale)
    pub status: u16,    // last fetch HTTP status (0 if never fetched)
    pub body: String,   // the response body (fresh or last-good stale)
    pub error: String,
    pub stale: bool,    // body came from cache after a failed/denied refresh
    pub age_secs: i64,  // now - fetched_at for the returned body (0 if fresh fetch)
}
```

`ok` means **"here is data you can use"** (fresh or stale), not "transport
succeeded." `stale` distinguishes the two. This is the natural contract for a
cache and is all the guest needs (`if r.ok { parse r.body }`). `status`/`age_secs`
are exposed for observability and future guests; weather ignores them.

## Host behavior (the TTL logic, moved here)

`perform_http_get_cached(ctx, url, ttl_secs, now, fetcher) -> CachedHttpResult`
in `crates/rustline-wasm/src/perform.rs`, using pure helpers in a new
`crates/rustline-wasm/src/cache.rs`:

1. **Gate first (deny-by-default, matches N1).** If `!ctx.allowed_urls.allows(url)`:
   return `ok:false, error:"url not allowed: <url>"` **immediately** — no network
   call, and no cache read or write. Revoking a URL grant fully silences the
   widget ("no network grant ⇒ no data"). This is the load-bearing denied-case.
2. **Fresh hit.** Read the cache entry for `url`; if it exists and
   `now − fetched_at` is in `0..ttl_secs`, return it (`ok:true, stale:false,
   age_secs`) **without calling the fetcher**.
3. **Refresh** via the existing `Fetcher` seam:
   - **Transport Ok + 2xx:** persist `{fetched_at: now, status, body}` and return
     `ok:true, stale:false, status, age_secs:0`. A cache *write* failure (e.g.
     quota) never fails the call — the freshly-fetched body still returns
     `ok:true`; the host `warn!`s the write failure.
   - **Transport Ok + non-2xx** *or* **Transport Err:** the refresh failed.
     If a cache entry exists → serve it: `ok:true, stale:true, status, age_secs`.
     Otherwise → `ok:false` with `error` = the transport error or `"http status N"`.

Only **2xx** responses are cached, so a transient 500 never overwrites a good
last-good body (a regression the current guest avoids only incidentally, via
`parse_wttr` failing).

## Storage & capability gating

- Cache entries live under the plugin's own state dir at
  `<state_dir>/__http_cache__/<hash>.json`, where `<hash>` is an inline FNV-1a
  of the URL (deterministic, dependency-free; a disposable cache key). The host
  writes this path directly (it controls it — it does not route through
  `sanitize_relpath`, which is for guest-supplied relpaths).
- Because the cache lives inside the state dir, it counts against
  `max_state_bytes` via the existing `dir_size`/`check_cap` walk (**N3** quota
  preserved) and is per-plugin (**N4** preserved). A cache write that would
  exceed quota is refused by `check_cap`; per step 3 the fetched body is still
  returned, just unpersisted.
- No new config knob: the capability is gated by the existing `allowed_urls`.
- Keying by full URL means a zip change is a *different cache entry*, so
  cross-zip stale leakage is structurally impossible (no per-entry zip re-check
  needed).

## Guest simplification (`plugins/weather/src/lib.rs`)

- **Remove** `is_fresh` (+ its unit test), `read_cache`, `write_cache`, and the
  three imports `rl_http_get` / `rl_state_read` / `rl_state_write`.
- **Add** the single import
  `fn rl_http_get_cached(url: String, ttl_secs: String, now: String) -> String;`.
- `render()` becomes: read `zip`/`format`/`refresh_secs`/`api_base` and `now`
  (as today) → build `{api_base}/{zip}?format=j1` → call
  `rl_http_get_cached(url, refresh_secs.to_string(), now)` → if `ok`,
  `parse_wttr(body)` and format one `Segment`, else empty.
- **Keep** the pure fns `code_to_icon`, `render_format`, `parse_wttr` and their
  unit tests unchanged.

Net: the guest no longer tracks any TTL state.

## What does NOT change

- `rl_http_get`, `rl_state_read`, `rl_state_write`, `rl_file_read`,
  `rl_file_write` all remain, registered for every plugin. Weather simply stops
  using three of them. (Host count goes 5 → 6 capability-gated functions.)
- `CapabilityCtx`, `AllowSet`, `sanitize_relpath`, `normalize_abs`,
  `check_cap`/`dir_size` are reused unchanged.
- The render pipeline, `WasmWidget::render`'s degrade-to-empty, fuel/timeout/
  memory caps — all unchanged (**N2** intact).

## Invariants this feature depends on

- **N1 (zero ambient authority / every network effect gated):** the new
  capability must gate `allowed_urls` *before* any fetch. Guarded by the
  denied-case test below (fetcher never invoked on a denied URL).
- **N3 (state writes quota-bounded):** the cache reuses `check_cap`; guarded by
  the quota test.
- **N4 (per-plugin scope):** the cache path derives from the instance's own
  `CapabilityCtx.state_dir()`.
- **Invariant #1 (Context is the sole render input):** the guest forwards
  `Context.now` as the freshness clock rather than the host reading a wall
  clock, keeping render a pure function of `Context` and the host freshness
  logic unit-testable without a clock seam.

## Testing

**Host unit tests** (`perform.rs` / `cache.rs`, no network, via the `Fetcher`
seam + a hit-counting/`panic`-on-2nd-call fake):

1. **Denied URL** → `ok:false`, fetcher **never called**, and no cache file
   created (load-bearing N1 denied-case).
2. **First call** populates the cache and returns the fresh body (`stale:false`).
3. **Within TTL** returns the cached body **without calling the fetcher**
   (counting fetcher asserts exactly one network call across two calls).
4. **Expired TTL** re-fetches and updates the cache.
5. **Transport failure with an existing entry** → `ok:true, stale:true`, body =
   last-good.
6. **Transport failure with no entry** → `ok:false`.
7. **Non-2xx response** does not overwrite an existing good entry (serves stale);
   with no entry → `ok:false`.
8. **Quota-exceeding body** → `check_cap` refuses the write but the fetched body
   is still returned `ok:true` (unpersisted).
9. **Freshness helper** (`cache::is_fresh`-equivalent): fresh / expired /
   unparseable-timestamp (→ not fresh), mirroring the old guest test now that the
   logic lives host-side.

**Guest unit tests:** `code_to_icon`, `render_format`, `parse_wttr` kept; the
`is_fresh` guest test is removed (the function moved host-side).

**Opt-in e2e (`wasm-e2e`, `just test-wasm`)** — rewritten for the host cache:

- `crates/rustline-wasm/tests/e2e.rs`:
  - *caches within refresh window → exactly one HTTP call* — already exercises a
    live mock + allowlisted localhost; passes under the host cache essentially
    unchanged (two renders in-window ⇒ 1 hit).
  - *stale on failure* — a mock that accepts one connection then closes: render
    at T0 caches; render at T0+expired hits the now-dead endpoint → serves stale
    (same URL/key). No guest-format seeding.
  - *no cross-zip leakage* — two `weather` widgets sharing one state root and the
    same live mock: widget A (zip 48183) fetches+caches; widget B (zip 90210)
    pointed at a dead endpoint renders empty — it never sees A's entry (different
    URL key).
- `crates/rustline/tests/wasm_wiring.rs`: rewrite to prove wiring with an
  in-process mock + `allowed_urls = ["http://127.0.0.1:*/*"]` and `api_base`
  set to the mock (one real fetch → temp rendered), instead of the old
  unallowlisted pre-seeded `weather.json` (which the gate-first semantics would
  now — correctly — refuse to serve).

## Docs to update (same branch)

- `CLAUDE.md`: host-function count 5 → 6; module map (`perform.rs` gains
  `perform_http_get_cached`, new `cache.rs`, `abi.rs` gains `CachedHttpResult`,
  `host.rs` gains `rl_http_get_cached`); the weather description (no longer
  hand-rolls caching); guest-imports list; N1 note (new gated capability + its
  denied-case test); Roadmap "Done" line.
- `README.md`: if it enumerates host functions / describes the weather plugin,
  add the cached-fetch capability.
- `TODO.md`: check off item #1.

## Out of scope (YAGNI)

- A generic (non-HTTP) TTL key/value cache. Chosen against in brainstorming;
  the HTTP-cached-GET is the canonical statusline primitive and yields the most
  simplification.
- A configurable max-stale bound (stale is served indefinitely on failure, as
  today).
- Guest-side i64 marshaling of `ttl_secs` (kept as string for ABI uniformity).
