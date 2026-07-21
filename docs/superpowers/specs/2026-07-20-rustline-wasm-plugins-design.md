# rustline WASM plugin system + weather widget — design

**Status:** approved (brainstorm, 2026-07-20)
**Depends on:** the v1 core (`docs/superpowers/specs/2026-07-20-rustline-tmux-statusline-design.md`)
**Scope:** Turn the reserved WASM seam into a working plugin runtime with a
capability-gated host, and ship a `weather` plugin as the worked example.

## 1. Purpose & success criteria

rustline v1 baked all widgets in and *reserved* — but did not build — a WASM
plugin seam. This project builds that seam: a real WebAssembly host that loads
plugins from a directory, renders them exactly like built-in widgets
(`Context` in → `Segment`s out — the serde types are already the ABI), and hands
each plugin a set of **capability-gated host functions** with **zero ambient
authority**. All network and filesystem access is mediated and checked by the
host ("the main runtime does the check").

**Success when:**

1. `just build-weather` compiles the example plugin to `wasm32-unknown-unknown`
   and drops `weather.wasm` in the plugin dir.
2. With `weather` in a layout region and a config entry, `rustline render right`
   shows a Nerd-Font weather icon + °F for zip `48183`, fetched from wttr.in.
3. The plugin hits the network **at most once per 30 min**, caching to its
   per-plugin state dir; a fetch failure falls back to the stale cache, and a
   bad/absent cache never breaks the bar.
4. A plugin can reach **only** the URLs/paths its config allows; a disallowed
   `rl_http_get`/`rl_file_*` returns an error to the guest and makes no request.
5. `rustline plugin url|path list|add|remove <plugin> [pattern]` and
   `rustline plugin list` manage allowlists by editing the config in place.
6. The whole dependency graph stays OpenSSL-free (`cargo tree -i openssl` empty);
   `just test` is hermetic (no wasm toolchain required); clippy/fmt clean.

**Non-goals (deferred):** plugin auto-download by `owner/repo`; plugin-declared
capability *requests* with an interactive approval flow (the config is edited
manually / via CLI for now); a long-running daemon; compiled-module on-disk
caching; per-read/per-write path allowlist split; Windows.

## 2. Architecture overview

```
crates/
  rustline-core/     pure: types (Context/Segment/…), Widget trait, Registry,
                     render pipeline, and the (now typed) Config schema.
                     NO wasmtime, NO I/O. Unchanged except config.rs.
  rustline-wasm/     NEW native host: Extism runtime, capability enforcement,
                     host functions, plugin discovery → Widget registration.
                     Reused verbatim by a future daemon.
  rustline/          bin: wires rustline-wasm into discovery+registration,
                     adds `plugin` CLI subcommands, `--plugin-dir` flag.
plugins/
  weather/           NEW, EXCLUDED from the root workspace (own Cargo.lock,
                     wasm32-unknown-unknown, extism-pdk). The worked example.
```

`rustline-core` stays the pure, front-end-agnostic core. The WASM **host** is a
new sibling crate so wasmtime/network/FS deps never enter core, and so a future
daemon front-end can reuse the same host. The example **plugin** is a separate,
excluded crate because a wasm-only guest cannot build for the host target.

### 2.1 Crate dependencies (host)

`rustline-wasm`: `extism` (`default-features = false` — we do **not** use its
built-in HTTP), `rustline-core`, `ureq` (`rustls`, `json`) for the HTTP
capability, `globset` + `regex` for allowlist matching, `walkdir` for state-dir
size accounting, `serde` + `serde_json`, `thiserror`, `tracing`.

`rustline` (bin) additionally: `toml_edit` (in-place config mutation for the CLI).

`plugins/weather`: `extism-pdk`, `serde`, `serde_json`. No other deps — it has
no clock, no network, no FS except through host functions.

**Policy checks (crate-decisions):** `default-features = false` everywhere;
verify `cargo tree -i openssl` and `cargo tree -i native-tls` are empty after the
graph settles (ureq→rustls, extism→wasmtime, neither pulls OpenSSL). Commit
`Cargo.lock` with the dependency change. `plugins/weather` is excluded from the
root workspace via `[workspace] exclude = ["plugins/weather"]`.

## 3. Plugin ABI (Extism)

The guest is a core wasm module (Extism, not the Component Model). Bytes cross
the boundary; the host and guest agree on JSON payloads of the existing serde
types.

### 3.1 Guest exports (host → guest, via `plugin.call`)

| Export   | Input (bytes)                              | Output (bytes)          |
|----------|--------------------------------------------|-------------------------|
| `name`   | *(empty)*                                  | UTF-8 name, e.g. `weather` |
| `render` | JSON `RenderInput { context, config }`     | JSON `Vec<Segment>`     |

`RenderInput` (defined in `rustline-wasm`, mirrored in the guest):

```jsonc
{
  "context": { /* rustline_core::Context, verbatim */ },
  "config":  { /* the [plugins.<name>.options] sub-table as JSON; {} if absent */ }
}
```

**Plugin identity (one name from three sources that must agree):**
- The **`.wasm` filename stem** is the candidate/registered **widget name** — the
  string the user types in a layout (`right = ["weather"]`) and the config key
  (`[plugins.weather]`). It is known *without* instantiating (cheap), so the host
  can decide whether a plugin is even needed before paying wasm cold-start.
- The **exported `name()`** is authoritative for the **state subdir** and is
  verified to equal the filename stem. **On mismatch the plugin is skipped with a
  `warn!`** — a predictable, fail-safe rule (no guessing which identity wins).
- Normal case: `weather.wasm` exports `"weather"`, config `[plugins.weather]`,
  layout `"weather"` — all equal, state dir `…/state/weather/`.

### 3.2 Host functions (guest → host, imported)

All defined with Extism's `host_fn!` macro; each plugin instance is built with a
`UserData<CapabilityCtx>` carrying **that plugin's** allowlists, state dir, and
size cap (per-plugin scoping falls out of per-instance `UserData`). Every host
function returns a **JSON string result** and never hard-aborts the guest — the
guest inspects `ok`/`error` and degrades gracefully.

| Host fn                        | Result JSON                                  | Gate |
|--------------------------------|----------------------------------------------|------|
| `rl_http_get(url)`             | `{ ok, status, body, error }`                | `allowed_urls` |
| `rl_state_read(relpath)`       | `{ ok, exists, contents, error }`            | own state dir only |
| `rl_state_write(relpath, s)`   | `{ ok, error }`                              | own state dir, size cap |
| `rl_file_read(path)`           | `{ ok, exists, contents, error }`            | `allowed_paths` |
| `rl_file_write(path, s)`       | `{ ok, error }`                              | `allowed_paths` |

Contents are UTF-8 strings (sufficient for the JSON caches these are for; binary
is a future extension via base64). The guest runs with **wasi off**, a wasmtime
**fuel limit** and a **`Manifest::with_timeout`** wall-clock kill, and
`with_memory_max`. Extism's built-in HTTP (`with_allowed_host`) is **not**
enabled — `rl_http_get` is our own function so the rustls client, the allowlist
check, and future regex pre-approval are entirely ours.

## 4. Capability enforcement (host)

### 4.1 Allow patterns (URLs and paths)

Each `allowed_urls` / `allowed_paths` entry is a **glob by default**, or a
**regex if prefixed `re:`** — covering "regex or globs" uniformly for both
surfaces and matching the future "plugin-declared regexes for pre-approval" path.

- **URL gate:** the requested URL is matched against every `allowed_urls`
  pattern; on no match, `rl_http_get` returns `{ ok:false, error:"url not
  allowed" }` and **makes no request**. Globs match the full URL string
  (`https://wttr.in/*`); `re:` entries are anchored (`is_match` on the full URL).
- **Path gate:** the path is normalized (lexically resolved, no `..`
  traversal beyond an allowed prefix) and matched against `allowed_paths` for
  both `rl_file_read` and `rl_file_write` (single list gates both in v1).

Patterns are compiled once per plugin instance; a malformed pattern is logged
and skipped (never fatal — config totality).

### 4.2 State dir (unrestricted within own dir, size-capped)

`rl_state_*` ignore the allowlists. The effective path is
`state_root/<name>/<relpath>` where `relpath` is sanitized: reject absolute
paths and any component that is `..` (no escape from the plugin's own dir).
Parent dirs are created on write. Before a write commits, the host sums the
plugin's state dir with `walkdir`; if `existing_total − replaced_file_size +
new_size > max_state_bytes`, the write is refused with
`{ ok:false, error:"state quota exceeded" }`. Default cap **50 MB**
(`52428800`), configurable per plugin.

`state_root` = `$XDG_DATA_HOME/rustline/state` (fallback
`$HOME/.local/share/rustline/state`). Rationale: the user specified
`~/.local/share`; XDG_STATE_HOME is noted as the "more correct for volatile
state" alternative in a code comment but not used, to honor the stated path.

## 5. Config schema (`rustline-core/config.rs`)

`plugins` changes from `HashMap<String, toml::Value>` to a typed map keyed by
**plugin name**, and a top-level `plugin_dir` is added:

```rust
pub struct Config {
    // …existing: layout, theme, widgets…
    pub plugin_dir: Option<String>,          // NEW; overrides discovery default
    pub plugins: HashMap<String, PluginConfig>,  // was HashMap<String, toml::Value>
}

pub struct PluginConfig {
    #[serde(default)] pub source: Option<String>,     // provenance, e.g. "owner/repo"
    #[serde(default)] pub allowed_urls: Vec<String>,  // glob or `re:` regex
    #[serde(default)] pub allowed_paths: Vec<String>, // glob or `re:` regex
    #[serde(default = "default_max_state_bytes")] pub max_state_bytes: u64, // 50 MB
    #[serde(default)] pub options: toml::Value,        // opaque; forwarded to guest
}
```

Example (the shipped default for `weather`):

```toml
plugin_dir = "~/.local/share/rustline/plugins"   # optional

[plugins.weather]
source = "steve/rustline-weather"
allowed_urls = ["https://wttr.in/*"]
allowed_paths = []
max_state_bytes = 52428800

[plugins.weather.options]
zip = "48183"
format = "{icon} {temp_f}°F"
refresh_secs = 1800
```

Every field is `#[serde(default)]`, so **`Config::load` stays total** (invariant
#3). A leftover reserved `[plugins."owner/repo"]` table parses without error
(unknown inner keys ignored); it simply won't drive a plugin unless a `.wasm`
with that name exists. The config serde **round-trip test** is extended to the
typed `plugins` map. `~` in `plugin_dir` is expanded to `$HOME` by the host.

## 6. Discovery, registration, degradation (`rustline-wasm` + bin)

- **Plugin dir** resolution order: `--plugin-dir` flag › config `plugin_dir` ›
  `$XDG_DATA_HOME/rustline/plugins` (fallback `$HOME/.local/share/rustline/plugins`).
- Discovery lists `*.wasm` and derives each candidate name from its **filename
  stem** (no instantiation). To avoid paying wasm cold-start for unused plugins on
  every shell-out, **only plugins whose stem appears in the region's layout
  names** are instantiated (the bin has that list before building the registry).
- For each needed plugin: build an Extism `Plugin` (`with_wasi(false)`, fuel +
  timeout + memory caps, the five host functions bound with that plugin's
  `CapabilityCtx`), call `name` and **verify it equals the filename stem**
  (mismatch → `warn!` + skip, per §3.1), then register a `WasmWidget` factory
  under the stem. Built-ins are registered first; a plugin whose stem collides
  with a built-in is dropped with a `warn!` (built-in wins).
- `WasmWidget` holds `Arc<Mutex<extism::Plugin>>` + the plugin's options JSON.
  Its `Widget::render` serializes `RenderInput`, calls `render`, deserializes
  `Vec<Segment>`; **any error, timeout, or malformed output → empty `Vec`**
  (degrade). This composes with the existing `catch_unwind` per-widget guard
  (invariant #6): the bar never breaks because of a plugin.

The host exposes one entry point the bin calls:
`rustline_wasm::register_plugins(&mut Registry, &Config, plugin_dir, needed: &[String])`.

## 7. Weather plugin (`plugins/weather`)

`name` → `"weather"`. `render(RenderInput)`:

1. Read options: `zip` (default `"48183"`), `format` (default `"{icon} {temp_f}°F"`),
   `refresh_secs` (default `1800`).
2. `rl_state_read("weather.json")` → cached
   `{ fetched_at: RFC3339, zip, temp_f, code, desc }`. **Fresh** iff
   `context.now − fetched_at < refresh_secs` **and** `cache.zip == zip` → use it.
3. Otherwise `rl_http_get("https://wttr.in/<zip>?format=j1")`; on `ok && 2xx`,
   parse `current_condition[0]` → `temp_F`, `weatherCode`, `weatherDesc`, then
   `rl_state_write("weather.json", …)` stamped with `context.now`.
4. On fetch failure, **fall back to the stale cache** if present; else return an
   empty `Vec<Segment>`.
5. Map `weatherCode` (WWO codes) → a Nerd-Font weather glyph. Substitute
   `{icon}`, `{temp_f}`, `{conditions}` (and `{zip}`) into `format`. Return one
   `Segment`.

The plugin gets **`now` from `Context`** (it has no clock). Its pure functions —
`code_to_icon`, `render_format`, `is_fresh(now, fetched_at, refresh_secs, zip)` —
are written host-testable (no host-fn dependency) so they unit-test on the host
target without wasm.

## 8. CLI (`rustline` bin)

```
rustline plugin list                                 # plugins found + allowlists + caps
rustline plugin url  list   <plugin>                 # list URL allow patterns
rustline plugin url  add    <plugin> <pattern>       # append (idempotent)
rustline plugin url  remove <plugin> <pattern>       # remove exact match
rustline plugin path list|add|remove <plugin> [pat]  # same for path patterns
rustline render left|right … [--plugin-dir <dir>]    # flag overrides config
```

`add`/`remove` load the config with `toml_edit`, mutate the target plugin's array
(creating `[plugins.<plugin>]` if absent), and write back — **comments and
formatting preserved**. `list` reads the effective `Config`. `plugin list` also
reports which discovered `.wasm` names have a matching config entry (and warns on
names with no entry / entries with no `.wasm`).

## 9. Build & tooling (`justfile`)

- `just build-weather` — `cargo build --release --target wasm32-unknown-unknown`
  in `plugins/weather`, copy the artifact to
  `$XDG_DATA_HOME/rustline/plugins/weather.wasm` (and a repo `plugins/dist/`
  copy for local `just preview`).
- `just test` stays hermetic (host crates only; no wasm target needed).
- `just test-wasm` — build the guest test fixtures + weather plugin and run the
  end-to-end host tests (opt-in; requires the wasm target + `wiremock`).
- Existing `just build/test/lint/preview` extended to include `rustline-wasm`.

## 10. Testing

**Pure host logic (unit, no wasm):**
- Allow-pattern match: glob hit/miss and `re:` hit/miss, for both URL and path.
- **Denied-case is the load-bearing test** — assert a non-matching URL/path is
  refused (per spec-discipline: the gate, not just the happy path).
- Path sanitization: `..`, absolute paths, and nested-escape attempts rejected.
- Size-cap accounting: a write that would exceed `max_state_bytes` is refused; a
  replace within cap succeeds.
- `~` expansion for `plugin_dir`/state root; plugin-dir resolution precedence
  (flag › config › default).

**Config (unit):**
- Typed `plugins` map parses; defaults fill for omitted fields; `max_state_bytes`
  default is 50 MB; serde **round-trip** preserves a `PluginConfig` incl. the raw
  `options` table. Totality: a malformed `plugins` table → `Config::default`.

**Weather guest (unit, host target):**
- `code_to_icon` covers the mapped WWO codes + an unknown-code fallback.
- `render_format` substitutes each placeholder; unknown placeholders pass through.
- `is_fresh`: within/after `refresh_secs`; zip change forces stale.

**End-to-end (opt-in `just test-wasm`):**
- A committed **tiny fixture `.wasm`** exercises each host fn: allowed vs denied
  URL, state write under/over cap, `..` path escape refused, `allowed_paths` hit/miss.
- The real `weather.wasm` against a `wiremock`ed wttr.in: first render fetches +
  writes cache; second render within `refresh_secs` makes **no** HTTP call
  (assert the mock got exactly one hit); fetch error falls back to stale cache.

**Bin integration (`tests/smoke.rs`):**
- `plugin url add/remove` mutates a temp config and round-trips through
  `toml_edit` preserving an unrelated comment.
- `render right` with a plugin name but **no `.wasm`** present degrades to the
  rest of the region (no panic, no error exit).

## 11. Invariants

**New (this feature introduces — re-check when touching):**
- **N1. Guest has zero ambient authority.** Every network/FS effect goes through
  a host function that checks the plugin's config first. `with_wasi(false)`; no
  Extism built-in HTTP. *A new host capability must add its gate + a denied-case
  test.*
- **N2. A plugin never breaks the bar.** Errors/timeouts/malformed output →
  empty segments. Fuel + timeout + memory caps bound a runaway guest.
- **N3. State writes are quota-bounded and dir-sandboxed.** `<state>/<name>/`
  only, `..` rejected, `max_state_bytes` enforced pre-commit.
- **N4. Per-plugin capability scope.** Allowlists/caps come from the *instance's*
  `UserData`; plugin A can never use plugin B's grants.

**Preserved (from v1 — this feature must not regress):**
- Core stays pure & serde-serializable (the ABI); host/FS/wasm live outside core.
- `Config::load` stays total (typed `plugins` map is all-`#[serde(default)]`).
- `render_region` ordering; `loadavg` Option; the `catch_unwind` per-widget guard
  (which `WasmWidget` composes with, not replaces).

## 12. Invariants this feature depends on

(For a future change touching these funnels — grep for these before altering.)

- **The `rl_http_get` allowlist is the *only* network path for guests.** The
  weather plugin's 30-min rate-limit correctness assumes it cannot fetch except
  through the gated, cached path. If a second network capability is added, it
  must route through the same allowlist + honor caching, or the rate limit is
  silently bypassed.
- **`Context.now` is the guest's only clock.** `is_fresh` freshness is computed
  from `context.now`; if rendering ever stops populating `now` truthfully (e.g. a
  daemon reusing a stale `Context`), the cache-freshness logic breaks. Pinned by
  the wiremock "exactly one hit" test at the host↔guest seam.
- **`Segment`/`Context` JSON is the wire format.** The end-to-end test pins the
  round-trip; a non-serializable field added to either type breaks every plugin.

## 13. Rollout / order of work

1. Config schema (typed `plugins` + `plugin_dir`) in core, with round-trip/total
   tests. (Isolated; unblocks everything.)
2. `rustline-wasm` skeleton: `CapabilityCtx`, allow-pattern matcher, path
   sandbox, size accounting — all pure, all unit-tested first (TDD).
3. Host functions (`host_fn!`) + Extism plugin build/instantiate + `WasmWidget`
   + `register_plugins`. Fixture-wasm end-to-end tests.
4. Bin wiring: discovery, `--plugin-dir`, `plugin` CLI subcommands (`toml_edit`).
5. `plugins/weather` guest + its pure-fn unit tests; `just build-weather`,
   `just test-wasm`; wiremock e2e.
6. Docs: `CLAUDE.md` (module map, invariants, config, CLI), `README`, this spec
   link.
