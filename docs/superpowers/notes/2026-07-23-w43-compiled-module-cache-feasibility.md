# W43 — WASM compiled-module cache: feasibility spike

**Date:** 2026-07-23
**Task:** Time-boxed feasibility spike (investigation only — no code change).
**Question:** rustline cold-spawns a fresh process per tmux `status-interval`
(left + right + one per window). Each spawn JIT-compiles every needed `.wasm`
plugin from raw bytes. Can we cache a **precompiled** artifact to disk so a
later cold spawn *deserializes* instead of recompiling, cutting cold-start —
**without** bypassing Extism's capability-gated host?

## Verdict

**FEASIBLE-WITH-CAVEATS.** Extism 1.30's Rust SDK already exposes wasmtime's
on-disk compilation cache via a first-class builder method
(`PluginBuilder::with_cache_config`). It stays entirely inside Extism's
capability-gated instantiation path — no `wasmtime` dependency, no host bypass,
no daemon required. The caveats are (1) it caches *compiled Cranelift artifacts
keyed by wasmtime version + flags + module hash*, so it invalidates itself
across an extism/wasmtime bump (safe, one-time recompile), and (2) the net win
depends on module size and must be measured; deserialization is far cheaper than
Cranelift but not free. This is a small, contained change — recommend promoting
W43 from "spike" to a real (low-risk) implementation task, gated on a
before/after measurement.

## Current instantiation path (grounding)

- **Versions in use** (`Cargo.lock`): `extism = 1.30.0`, `wasmtime = 43.0.2`.
  Declared as `extism = { version = "1", default-features = false, features =
  ["wasmtime-default-features"] }` — `crates/rustline-wasm/Cargo.toml:30-32`.
- **The compile-every-spawn seam:** `build_plugin` —
  `crates/rustline-wasm/src/host.rs:76-110`. It builds
  `Manifest::new([Wasm::data(wasm.to_vec())])` then
  `PluginBuilder::new(manifest).with_wasi(false).with_fuel_limit(...)
  .with_function(...×7).build()` (`host.rs:78-109`). `PluginBuilder::build()`
  runs wasmtime's Cranelift compiler over the raw bytes **every call**.
- **Called once per cold spawn:** `register_plugins` reads the `.wasm` off disk
  (`lib.rs:82`) and calls `host::build_plugin(&wasm, ctx)` (`lib.rs:88`) for each
  *needed* plugin. Every real `rustline render left|right` / `render window` is a
  separate OS process, so this is one fresh Cranelift compile per plugin **per
  refresh tick** — exactly the cost W43 targets.

## Evidence — the Extism 1.30 Rust API

Confirmed on `docs.rs/extism/1.30.0`:

- **`PluginBuilder::with_cache_config(self, dir: impl Into<PathBuf>) -> Self`** —
  doc: *"Set wasmtime compilation cache config path"*. This is the disk-persisted,
  **cross-process** hook we need. It points wasmtime at a cache-config TOML;
  wasmtime then hashes each module and, on a hit, deserializes the precompiled
  artifact instead of recompiling.
  (https://docs.rs/extism/1.30.0/extism/struct.PluginBuilder.html)
- **`PluginBuilder::with_cache_disabled(self) -> Self`** — *"Turn wasmtime
  compilation caching off"* (the A/B toggle for benchmarking).
- **Cache config format & precedence** (Extism runtime README):
  `EXTISM_CACHE_CONFIG=path/to/config.toml` enables it globally; a
  `$HOME/.config/wasmtime/config.toml` is honored; **`with_cache_config` overrides
  the env var per-plugin**. The config TOML is:
  ```toml
  [cache]
  enabled   = true          # still required by extism's wasmtime
  directory = "/some/path"   # where compiled artifacts live
  ```
  (https://github.com/extism/extism/blob/main/runtime/README.md)
- **The in-process-only contrast:** Extism 1.30 also exports `CompiledPlugin`
  (`PluginBuilder::compile(self) -> Result<CompiledPlugin, Error>`), which
  pre-compiles once and lets you spin up many `Plugin` instances from it
  *within one process*. There is **no** `CompiledPlugin` serialize-to-disk /
  `new_from_compiled(bytes)` in the public SDK. So `CompiledPlugin` helps a
  **warm/daemon** model (W48) but does **nothing** for cold-spawn-per-refresh —
  a new process must rebuild the `CompiledPlugin` from source. The only
  disk-persisted path the SDK surfaces is `with_cache_config`.
  (https://docs.rs/extism/1.30.0/extism/index.html)

### Why this satisfies the "no host bypass" constraint

`with_cache_config` is a method **on `PluginBuilder`** — it slots into the exact
chain `build_plugin` already uses. `with_wasi(false)`, the fuel/timeout/memory
caps, and all seven capability-gated host functions stay bound unchanged. The
cache changes only *how the module is compiled* (Cranelift vs deserialize), never
*what the guest can do*. Invariants N1–N4 are untouched: still zero ambient
authority, still per-plugin `CapabilityCtx`. No `wasmtime` crate is added to the
graph, so the rustls-only policy and `cargo tree -i openssl` emptiness are
undisturbed.

## Prototype sketch (do NOT implement here)

A ~15-line seam in `crates/rustline-wasm/src/host.rs`:

1. **One-time cache-config setup** (lazy, best-effort — never fatal): ensure a
   wasmtime cache-config TOML exists under the state root, e.g.
   `state_root()/wasmtime-cache.toml`, whose `[cache].directory` points at
   `state_root()/wasmtime-cache/`. Reuse the existing atomic-write convention
   (`cpu.rs`'s temp-file + rename). Keep the artifact dir *distinct* from any
   plugin state subdir (plugins own `state_root()/<name>/`), mirroring the
   `cpu-sample` collision note in CLAUDE.md.
2. **Wire it in `build_plugin`** (`host.rs:81-82`):
   ```rust
   let mut b = PluginBuilder::new(manifest).with_wasi(false);
   if let Some(cfg) = wasmtime_cache_config_path() {   // best-effort Option
       b = b.with_cache_config(cfg);
   }
   b.with_fuel_limit(500_000_000).with_function(...)...build()
   ```
   wasmtime does the hashing/keying (`.wasm` bytes + wasmtime version + flags)
   itself — we do **not** hand-roll an FNV hash → path scheme (unlike
   `cache.rs`'s HTTP-response cache); the wasmtime cache is content-addressed
   internally, so first spawn compiles + writes, every later cold spawn for an
   unchanged plugin deserializes.
3. **No ABI/wire changes, no new capability, no denied-case test** — this is a
   pure compile-provenance optimization behind the existing gate.

### What to measure before committing

- `rustline bench --cold` (`crates/rustline/src/bench/plugins.rs`) already
  exercises the cold-start *data* path — its `--cold` flag clears the plugin's
  **state/HTTP cache** dir (`plugins.rs:77-88`) to force a genuine cache-miss
  render. **Caveat found during the spike:** the plugins bench compiles each
  module *once* in `register_plugins` (`plugins.rs:63`) and then only *clones*
  the shared `Arc<Mutex<Plugin>>` per iteration (`registry.build(stem)`), so the
  Cranelift compile cost W43 targets is **outside** its timed windows. To
  measure this feature honestly, add either (a) a `build_plugin`-only timing pass
  that re-instantiates from bytes each iteration with cache on vs
  `with_cache_disabled()`, or (b) an external wall-clock A/B —
  `hyperfine 'rustline render right'` with a warm cache dir vs
  `EXTISM_CACHE_CONFIG=""` — which captures the true per-process cold spawn.
- Decision rule: ship it only if the deserialize path beats a cold Cranelift
  compile by a margin that matters at a 1–5 s `status-interval` (small plugins
  like `weather` may show a modest win; the gain scales with module size).

## Caveats / risks to record

- **Self-invalidating across upgrades:** wasmtime keys cache entries on its own
  version + compile flags. An extism/wasmtime bump silently misses and recompiles
  once per plugin — correct, just a one-refresh blip after upgrade. No manual
  cache-busting needed.
- **`enabled = true` quirk:** the key is nominally optional upstream but *still
  required* by extism's wasmtime build, or the config fails to parse. The
  generated TOML must include it.
- **Deserialize isn't free:** it's mmap + validation of native code, not a no-op.
  Hence the measurement gate above.
- **Writable dir required:** the cache dir must be writable at spawn time; a
  read-only/again-full state root should degrade to "no cache" (best-effort),
  never break the bar (invariant N2 spirit).

## Recommendation

Reclassify W43 in `WHATS-NEXT.md` from an open spike to **FEASIBLE-WITH-CAVEATS —
small implementation task**, annotated: *"Use
`PluginBuilder::with_cache_config` (Extism 1.30) to point wasmtime's on-disk
compilation cache at the state root; ~15-line seam in `host.rs::build_plugin`;
gate on a before/after cold-spawn measurement (add a `build_plugin`-timing bench
pass or `hyperfine` A/B — the current `bench --cold` measures the data-cache
path, not module compile). Contrast: `CompiledPlugin`/W48-daemon is the
in-process warm path and does not help cold spawns."* The daemon front-end (W48)
remains the orthogonal, larger lever for keeping instances warm; it is **not**
required for W43.
