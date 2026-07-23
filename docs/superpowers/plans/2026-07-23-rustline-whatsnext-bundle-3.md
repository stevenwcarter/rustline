# rustline whats-next bundle #3 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship 6 whats-next items — three unblock-debt refactors (W51/W52/W53), two system widgets (W47 throughput, W45 cpu/memory sparkline), and the WASM compile-module cache + a timing measurement (W43).

**Architecture:** Existing Cargo workspace. Phase 1 refactors are behavior-preserving (byte-identical output, pinned with characterization tests). Phase 2 widgets do all cross-invocation sample I/O at the `Context`-build edge (the bin) via a shared `sample_store`, keeping widgets pure over `Context`. Phase 3 adds an Extism cache-config seam + a bench pass. No new dependencies.

**Tech Stack:** Rust edition 2024, serde, extism 1.30 (`PluginBuilder::with_cache_config`), std `/proc` reads, tracing.

## Global Constraints

- **No new dependencies.** `/proc/net/dev` is a std file read; the wasmtime cache uses the existing `extism` API; the sparkline is std. `cargo tree -i openssl` / `-i native-tls` MUST stay empty.
- **Byte-identical / behavior-preserving:** Phase-1 refactors change no observable output; new widget options default byte-identical; new widgets opt-in. Pin the load-bearing ones with characterization tests (W53 renderer, W52 theme preview).
- **Widgets read only `Context`** (invariant #1); sample I/O for W45/W47 is at the `Context`-build edge, never in a widget. A failed read is `Option::None`, never fabricated (invariant #6). `Config::load` total — new config `#[serde(default)]` (invariant #3).
- **Segment order (#5) + range-name identity (#7)** preserved by W53.
- **N1–N4 / N2:** W43's cache is compile-provenance only and best-effort (an unwritable cache degrades to no-cache, never breaks the bar).
- Edition 2024; clippy `-D warnings` clean; `cargo fmt --all` clean; `Cargo.lock` committed if changed.
- Commit trailer on every commit: `Claude-Session: https://claude.ai/code/session_01Fddrk6VgQAPGvRg71F2eB1`.

---

## Task 1: W51 — Hoist SDK wire-result types into `rustline-abi`

**Files:**
- Modify: `crates/rustline-abi/src/lib.rs` (add the 4 structs)
- Modify: `crates/rustline-wasm/src/abi.rs` (remove definitions, `pub use rustline_abi::{…}`)
- Modify: `crates/rustline-plugin-sdk/src/lib.rs` (delete duplicate defs, use abi's)

**Interfaces:**
- Produces: `rustline_abi::{HttpResult, CachedHttpResult, ReadResult, WriteResult}`; unchanged wire format.
- Consumes: nothing new.

- [ ] **Step 1: Failing round-trip test** in `rustline-abi` (add after moving is drafted — write it first asserting the types exist in abi):

```rust
#[test]
fn wire_result_types_round_trip_host_json() {
    let h: HttpResult = serde_json::from_str(r#"{"ok":true,"status":200,"body":"x","error":""}"#).unwrap();
    assert!(h.ok && h.status == 200 && h.body == "x");
    let c: CachedHttpResult = serde_json::from_str(r#"{"ok":true,"status":200,"body":"x","error":"","stale":false,"age_secs":0}"#).unwrap();
    assert!(c.ok && !c.stale);
    let r: ReadResult = serde_json::from_str(r#"{"ok":true,"exists":true,"contents":"y","error":""}"#).unwrap();
    assert!(r.ok && r.exists);
    let w: WriteResult = serde_json::from_str(r#"{"ok":true,"error":""}"#).unwrap();
    assert!(w.ok);
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p rustline-abi wire_result_types_round_trip`. Expected: FAIL (types not in abi yet).
- [ ] **Step 3: Move.** Cut the four `#[derive(Debug, Default, Serialize, Deserialize)]` structs (`HttpResult`, `CachedHttpResult`, `ReadResult`, `WriteResult`) verbatim from `rustline-wasm/src/abi.rs` into `rustline-abi/src/lib.rs` (preserve every field name/order/type/serde attr). In `rustline-wasm/src/abi.rs` replace them with `pub use rustline_abi::{HttpResult, CachedHttpResult, ReadResult, WriteResult};` so existing `crate::abi::HttpResult` paths resolve. In `rustline-plugin-sdk/src/lib.rs` delete its duplicate declarations and `use`/re-export `rustline_abi::{…}` instead (keep the SDK's public surface identical).
- [ ] **Step 4: Run** — `cargo test -p rustline-abi wire_result_types_round_trip` (PASS); `just test` (host-target green); `just test-wasm` (the real host→SDK boundary still green).
- [ ] **Step 5: Verify** `cargo clippy --all-targets -- -D warnings`; `cargo fmt --all`; `cargo tree -i openssl` empty.
- [ ] **Step 6: Commit** — `git commit -am "refactor(abi): hoist wire-result types into rustline-abi (W51)"`.

---

## Task 2: W52 — Consolidate the three fabricated-`Context` builders

**Files:**
- Create: `crates/rustline/src/sample_context.rs`
- Modify: `crates/rustline/src/main.rs` (`mod sample_context;`)
- Modify: `crates/rustline/src/bench/fixture.rs`, `theme_cmd.rs`, `plugin_cmd.rs` (delegate)
- Test: `crates/rustline/src/sample_context.rs` tests + a theme-preview characterization test

**Interfaces:**
- Produces: `sample_context(show_alerts: bool) -> rustline_core::Context`.
- Consumes: `Context`, the theme/alert reading shapes.

- [ ] **Step 1: Characterization test FIRST (pin current behavior).** Before refactoring, capture the exact `theme show`-style default-layout render for the CURRENT `theme_cmd::sample_context(true)` and `(false)` as golden strings in a new test, so the refactor is provably output-preserving. (If `sample_context` isn't directly callable from a test, render via the same path `theme show` uses and snapshot the markup.)

```rust
#[test]
fn sample_context_render_is_unchanged_by_consolidation() {
    // golden = the pre-refactor rendered markup for show_alerts=true and =false.
    // Assert the new shared sample_context reproduces both byte-for-byte.
}
```

- [ ] **Step 2: Run, verify it passes on current code** (it's a characterization pin, not RED) — this is the guard the refactor must not break.
- [ ] **Step 3: Extract.** Create `sample_context(show_alerts: bool) -> Context` in `sample_context.rs` reproducing the fields the three builders set (superset; synthetic values). `show_alerts = true` pegs cpu/mem/battery/loadavg/disk to trip warning+error badges (copy `theme_cmd`'s current pegged values exactly); `false` = healthy. Point `theme_cmd::sample_context`, `bench::fixture::fabricated_context`, and `plugin_cmd`'s run fixture at it (each may wrap it if it needs a tweak, but the shared body is one place). Keep `theme_cmd`'s public callers unchanged.
- [ ] **Step 4: Run** — the characterization test from Step 1 PASSES unchanged; `just test` green (incl. bench tests: `cargo test -p rustline --features bench`).
- [ ] **Step 5:** clippy `-D warnings`; `cargo fmt --all`.
- [ ] **Step 6: Commit** — `git commit -am "refactor: single shared sample_context builder (W52)"`.

---

## Task 3: W53 — `Registry::resolve` → `(name, widget)` pairs

**Files:**
- Modify: `crates/rustline-core/src/widget.rs` (`resolve` signature)
- Modify: `crates/rustline-core/src/assemble.rs` (use returned names; delete `resolved_names` re-filter)
- Test: `widget.rs` + `assemble.rs` tests

**Interfaces:**
- Produces: `Registry::resolve(&[String]) -> Vec<(String, Box<dyn Widget>)>`.
- Consumes: existing `Widget::range_name`, the W29 `ColorOverride` map.

- [ ] **Step 1: Failing tests.** `resolve` returns name+widget pairs, skipping unknowns:

```rust
#[test]
fn resolve_returns_name_widget_pairs_skipping_unknown() {
    let reg = Registry::with_builtins(&Config::default());
    let out = reg.resolve(&["datetime".into(), "nope".into(), "cwd".into()]);
    let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["datetime", "cwd"]); // unknown skipped, order preserved
}
```

And a byte-identical characterization test in `assemble.rs`: render a multi-widget region (incl. an `alt_format`/clickable widget and an unknown name) — assert the markup equals a captured golden that matches the pre-change output.

- [ ] **Step 2: Run, verify fail** — `cargo test -p rustline-core resolve_returns_name_widget_pairs`. Expected: FAIL (signature is `Vec<Box<dyn Widget>>`).
- [ ] **Step 3: Implement.** Change `resolve` to push `(name.clone(), widget)` for each name that builds, skipping unknowns with the existing `warn!`. In `render_named_region`, consume the pairs directly for the widget name (range-wrapping + `overrides.get(name)`); DELETE the `resolved_names`/`registry.contains` second traversal. Update any other `resolve` caller.
- [ ] **Step 4: Run** — both tests PASS; the existing W29 color-override + range-wrapping tests still pass unchanged (`cargo test -p rustline-core assemble`); `just test` green.
- [ ] **Step 5:** clippy `-D warnings`; `cargo fmt --all`.
- [ ] **Step 6: Commit** — `git commit -am "refactor(core): Registry::resolve returns (name, widget) pairs (W53)"`.

---

## Task 4: W47 — Network throughput widget (+ shared `sample_store`)

**Files:**
- Create: `crates/rustline/src/sample_store.rs` (shared, reused by Task 5)
- Create: `crates/rustline/src/throughput.rs`
- Modify: `crates/rustline/src/main.rs` (`mod sample_store; mod throughput;`)
- Modify: `crates/rustline-abi/src/lib.rs` (`Throughput`)
- Modify: `crates/rustline-core/src/context.rs` (`throughput` field + re-export)
- Create: `crates/rustline-core/src/widgets/throughput.rs`
- Modify: `crates/rustline-core/src/widgets/mod.rs` (register + count 15→16)
- Modify: `crates/rustline/src/build_context.rs` (gated read)
- Modify: `crates/rustline-core/src/config.rs` (`ThroughputOpts`)
- Test: `throughput.rs`, `widgets/throughput.rs`, `tests/smoke.rs`

**Interfaces:**
- Produces: `sample_store::{read_sample, write_sample}` (best-effort per-widget state file under a state dir); `rustline_abi::Throughput { down_bytes_per_sec: u64, up_bytes_per_sec: u64 }`; `Context.throughput: Option<Throughput>`; `rustline::throughput::read_throughput(state_dir, interface) -> Option<Throughput>`; pure `parse_proc_net_dev(&str) -> Vec<(String,u64,u64)>` and `throughput_rate(prev, cur, dt_secs) -> Throughput`.
- Consumes: existing `format_bytes` (memory.rs, `pub(crate)`), `layout_needs`, toggle helpers, state-dir paths.

- [ ] **Step 1: Failing tests (pure):**

```rust
// throughput.rs
#[test]
fn parses_proc_net_dev_excluding_loopback() {
    let s = "Inter-|   Receive ...\n face |bytes ...\n    lo: 100 0 0 0 0 0 0 0 200 0 ...\n  eth0: 1000 5 0 0 0 0 0 0 2000 7 ...\n";
    let v = parse_proc_net_dev(s);
    assert_eq!(v, vec![("eth0".to_string(), 1000u64, 2000u64)]); // lo excluded, (rx,tx)
}
#[test]
fn rate_from_two_samples() {
    // prev rx=1000 tx=2000 @ t=0 ; cur rx=3000 tx=6000 @ t=2s → 1000/s down, 2000/s up
    let r = throughput_rate((1000, 2000), (3000, 6000), 2.0);
    assert_eq!(r.down_bytes_per_sec, 1000);
    assert_eq!(r.up_bytes_per_sec, 2000);
    // counter reset (cur < prev) → zero, not a huge saturating number
    let z = throughput_rate((5000, 5000), (10, 10), 1.0);
    assert_eq!((z.down_bytes_per_sec, z.up_bytes_per_sec), (0, 0));
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p rustline throughput`. Expected: FAIL.
- [ ] **Step 3: Implement.** `sample_store` (best-effort atomic temp-file+rename read/write of a small state file, `warn!` on failure). `Throughput` in `rustline-abi` (`#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]`), re-exported by core; `Context.throughput: Option<Throughput>` (`#[serde(default)]`, in `Context::default()`). `throughput.rs`: `parse_proc_net_dev`, `throughput_rate` (saturating, reset/backward-clock → 0), and `read_throughput(state_dir, interface)` that reads `/proc/net/dev` (Linux), aggregates non-loopback (or the configured interface), loads the prior `(rx,tx,ts)` via `sample_store`, computes the rate, stores the current sample, and returns `Some(Throughput)` (or `None` on first run / non-Linux / read failure). Widget `Throughput { opts }` over `ctx.throughput`: `{down}`/`{up}` via `format_bytes(bytes)+"/s"`, `down_format` on `None`, toggle-aware. `ThroughputOpts { format (default " {down} {up}"), interface: Option<String>, down_format, alt_format }`. Gated read `layout_needs(layout, "throughput")`. Register + count test 15→16 (+"throughput" in expected names).
- [ ] **Step 4: Smoke test** — add `render_right_with_throughput_renders_gracefully` in `tests/smoke.rs` (mirror the git/disk sibling: `[layout] right = ["throughput"]`, assert exit success even on first run / no prior sample).
- [ ] **Step 5: Run** — `just test` green; clippy `-D warnings`; `cargo fmt --all`.
- [ ] **Step 6: Commit** — `git commit -am "feat(throughput): network rx/tx rate widget + shared sample_store (W47)"`.

---

## Task 5: W45 — cpu/memory sparkline (reuses `sample_store`)

**Files:**
- Modify: `crates/rustline-abi/src/lib.rs` or `context.rs` (`cpu_history`/`mem_history` on `Context`)
- Create: `crates/rustline-core/src/widgets/spark.rs` (or extend `bar.rs`) — pure `sparkline`
- Modify: `crates/rustline-core/src/widgets/cpu.rs`, `memory.rs` (`{spark}` placeholder)
- Modify: `crates/rustline/src/cpu.rs`, `memory.rs` (read+append+persist history at build edge)
- Modify: `crates/rustline/src/build_context.rs` (populate histories, gated on `{spark}`)
- Modify: `crates/rustline-core/src/config.rs` (`spark_width` on cpu/memory opts)
- Test: `spark.rs`, `widgets/cpu.rs`/`memory.rs`

**Interfaces:**
- Produces: `sparkline(samples: &[f32], max: f32) -> String`; `Context.cpu_history: Vec<f32>`, `Context.mem_history: Vec<f32>`.
- Consumes: Task 4's `sample_store`, `gauge_bar`'s module neighborhood.

- [ ] **Step 1: Failing test** for the pure glyph mapper:

```rust
#[test]
fn sparkline_maps_readings_to_blocks() {
    assert_eq!(sparkline(&[], 100.0), "");
    assert_eq!(sparkline(&[0.0], 100.0), "▁");
    assert_eq!(sparkline(&[100.0], 100.0), "█");
    // a ramp spans low→high blocks; equal readings render equal glyphs
    let s = sparkline(&[0.0, 50.0, 100.0], 100.0);
    assert_eq!(s.chars().count(), 3);
    assert_eq!(s.chars().next().unwrap(), '▁');
    assert_eq!(s.chars().last().unwrap(), '█');
    // clamp above max
    assert_eq!(sparkline(&[200.0], 100.0), "█");
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p rustline-core sparkline`. Expected: FAIL.
- [ ] **Step 3: Implement.** `sparkline` maps each reading's fraction (`(v/max).clamp(0,1)`) to one of `▁▂▃▄▅▆▇█`. Add `cpu_history: Vec<f32>` / `mem_history: Vec<f32>` to `Context` (`#[serde(default)]`, in `Context::default()`). `{spark}` in cpu/memory format renders `sparkline(&ctx.cpu_history, 100.0)` / mem-percent history. `spark_width` (default e.g. 8) added to `CpuOpts`/`MemoryOpts`. At the build edge (`crate::cpu`/`crate::memory` read surfaces, or `build_context`), WHEN the widget's format contains `{spark}`: load the persisted ring via `sample_store`, push the current cpu%/mem% reading, truncate to `spark_width`, persist, and set the history on `Context`. No history I/O when `{spark}` absent (byte-identical).
- [ ] **Step 4: Run** — `cargo test -p rustline-core cpu memory spark`; a byte-identical test that cpu/memory render is unchanged when `{spark}` not in format; `just test` green; clippy; fmt.
- [ ] **Step 5: Commit** — `git commit -am "feat(cpu,memory): {spark} sparkline from persisted history (W45)"`.

---

## Task 6: W43 — `with_cache_config` seam + build-timing bench pass

**Files:**
- Modify: `crates/rustline-wasm/src/host.rs` (`build_plugin` seam + cache-config setup)
- Modify: `crates/rustline-wasm/src/paths.rs` (a cache-dir/config-path helper) or keep in host.rs
- Modify: `crates/rustline/src/bench/plugins.rs` (build-timing pass)
- Test: `host.rs`/`paths.rs` pure tests; `wasm-e2e`/manual for cache population

**Interfaces:**
- Produces: `wasmtime_cache_config_path() -> Option<PathBuf>` (best-effort; ensures the `[cache] enabled=true, directory=…` TOML exists under the state root); a `bench` build-timing pass.
- Consumes: `state_root()`, existing bench harness (`summarize`/`Row`/`Group`).

- [ ] **Step 1: Failing test (pure).** The cache-config TOML generator writes `[cache]\nenabled = true\ndirectory = "…"` for a given root, and `wasmtime_cache_config_path` returns `None` (never panics) when the root is unwritable:

```rust
#[test]
fn cache_config_toml_has_enabled_and_directory() {
    let dir = tempfile::tempdir().unwrap();
    let p = ensure_wasmtime_cache_config(dir.path()).unwrap();
    let toml = std::fs::read_to_string(&p).unwrap();
    assert!(toml.contains("[cache]") && toml.contains("enabled = true") && toml.contains("directory ="));
}
#[test]
fn cache_config_none_on_unwritable_root_no_panic() {
    // point at a path that can't be created (e.g. under a file); expect None, no panic.
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p rustline-wasm cache_config`. Expected: FAIL.
- [ ] **Step 3: Implement seam.** `ensure_wasmtime_cache_config(root) -> Option<PathBuf>` (atomic temp-file+rename; best-effort → `None` on any failure). `wasmtime_cache_config_path()` wraps it over `state_root()`. In `build_plugin`, `let mut b = PluginBuilder::new(manifest).with_wasi(false); if let Some(cfg) = wasmtime_cache_config_path() { b = b.with_cache_config(cfg); }` then the existing `.with_fuel_limit(...).with_function(...)....build()`. NEVER fail the build on a cache error (N2). No other builder change.
- [ ] **Step 4: Bench pass.** In `bench/plugins.rs` add a `build_plugin`-from-bytes timing pass measuring cache-ON vs `with_cache_disabled()` per plugin, reported via the existing harness (so `rustline bench --only plugins` shows the compile-vs-deserialize delta). Keep the existing preserved-state pass.
- [ ] **Step 5: Run** — `just test` green; `just test-wasm` (build still works with cache on); `cargo run --release --features bench -- bench --only plugins` runs and reports both timings (report the numbers in the task report); `cargo tree -i openssl` empty; clippy; fmt.
- [ ] **Step 6: Commit** — `git commit -am "feat(wasm): wasmtime compile-cache seam + build-timing bench (W43)"`.

---

## Task 7: Docs sync + count + WHATS-NEXT + full green

**Files:**
- Modify: `CLAUDE.md`, `README.md`
- Modify: `WHATS-NEXT.md` (gitignored — local bookkeeping)

- [ ] **Step 1:** Update `CLAUDE.md` + `README.md` for: the `throughput` widget (module map, widget list — count now **16**, Config, Roadmap→Done); the `{spark}` placeholder on `cpu`/`memory` + `spark_width`; the wire-result types now living in `rustline-abi` (module map: `rustline-abi` gains them, `rustline-wasm`/`SDK` re-export); `Registry::resolve` now returns `(name, widget)` pairs and `assemble.rs` dropped the `resolved_names` re-filter; the shared `sample_context`/`sample_store` helpers; the WASM compile-cache seam + the new `bench` plugins timing pass. Move W43/W45/W47/W51/W52/W53 Roadmap entries to Done.
- [ ] **Step 2:** Strip W43/W45/W47/W51/W52/W53 from `WHATS-NEXT.md`.
- [ ] **Step 3: Full green** — `just test`; `cargo clippy --all-targets -- -D warnings`; `cargo fmt --all --check`; `cargo tree -i openssl`/`-i native-tls` empty; `just test-wasm`.
- [ ] **Step 4: Commit** — `git commit -am "docs: sync CLAUDE.md + README.md for whats-next bundle #3"`.

---

## Self-Review

**Spec coverage:** W51→T1, W52→T2, W53→T3, W47→T4, W45→T5, W43→T6, docs/count→T7. All 6 IDs mapped.

**Dependency order:** T2 (W52, single builder) before T4/T5 (which add `Context` fields → one builder to update) ✓. T4 (W47) creates `sample_store` before T5 (W45) reuses it ✓. T4 and T5 share `config.rs`/`context.rs`/`build_context.rs` — sequential. T1 (abi/wasm/sdk), T3 (core assemble), T6 (wasm host/bench) are mutually disjoint and could parallelize with read-only reviews.

**Placeholder scan:** load-bearing pure logic (parse_proc_net_dev, throughput_rate, sparkline, cache-config gen, resolve-pairs) has concrete failing-test code; the byte-identical W52/W53 characterization tests are the guards for the behavior-preserving refactors. No `TODO`/`TBD` in steps.

**Type consistency:** `Throughput`, `parse_proc_net_dev`/`throughput_rate`, `sparkline`, `read_sample`/`write_sample`, `wasmtime_cache_config_path`/`ensure_wasmtime_cache_config`, `resolve → Vec<(String, Box<dyn Widget>)>` named identically across the tasks that define and consume them.
