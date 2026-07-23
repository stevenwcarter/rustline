# rustline whats-next bundle #3 — design spec

Date: 2026-07-23
Status: approved (brainstorm), ready for plan

The third whats-next `--execute` bundle: 6 items — three unblock-debt refactors
captured during bundle #2's reviews (W51/W52/W53), two deferred system widgets
(W45 sparkline, W47 network throughput), and the now-feasible WASM compile-module
cache (W43, whose bundle-#2 spike proved `PluginBuilder::with_cache_config`
viable). One phased spec, three phases, executed via subagent-driven-development.

## Selected items

| ID  | Title                                             | Lens         | Phase |
|-----|---------------------------------------------------|--------------|-------|
| W51 | Hoist SDK wire-result types into `rustline-abi`   | unblock-debt | 1     |
| W52 | Consolidate the 3 fabricated-`Context` builders   | unblock-debt | 1     |
| W53 | `Registry::resolve` → `(name, widget)` pairs      | unblock-debt | 1     |
| W47 | Network throughput (rx/tx) widget                 | feature-gap  | 2     |
| W45 | Historical sparkline for cpu/memory               | feature-gap  | 2     |
| W43 | WASM compiled-module cache + timing measurement   | scale-perf   | 3     |

## Global principles (apply to every item)

1. **Byte-identical at defaults / behavior-preserving refactors.** New widget
   options default so existing output is reproduced byte-for-byte; new widgets
   are opt-in; the three Phase-1 refactors change no observable output.
2. **TDD.** Pure parsers/samplers/glyph-mappers get unit tests first; the W53
   renderer change is pinned with a byte-identical characterization test.
3. **Widgets read only `Context`** (invariant #1). Persisted-sample I/O for
   W45/W47 happens at the `Context`-build edge (the bin), never inside a widget.
4. **`Config::load` total** (invariant #3): new config fields `#[serde(default)]`.
5. **`Option` reads** (invariant #6): a failed platform read renders nothing /
   `down_format`, never a fabricated value.
6. **rustls-only holds:** this bundle adds **no new dependencies** (`/proc/net/dev`
   is a std file read; the wasmtime cache uses the existing `extism` API; the
   sparkline is std). `cargo tree -i openssl`/`-i native-tls` stay empty.
7. **N2 (a plugin never breaks the bar):** W43's cache is best-effort — an
   unwritable/failed cache degrades to "no cache", never fatal.
8. Edition 2024; clippy/fmt clean; `Cargo.lock` committed if it changes.

---

## Phase 1 — Refactors (unblock-debt; behavior-preserving)

### W51 — Hoist SDK wire-result types into `rustline-abi`

**What.** One shared definition of the four host↔guest wire result structs.

**Design.**
- Move `HttpResult`, `CachedHttpResult`, `ReadResult`, `WriteResult` (currently
  `#[derive(Debug, Default, Serialize, Deserialize)]` structs in
  `crates/rustline-wasm/src/abi.rs`) into `crates/rustline-abi/src/lib.rs`
  (serde-only, chrono-free — same home as `WireContext`/`GuestRender`).
- `rustline-wasm` re-exports them (`pub use rustline_abi::{…}`) so existing
  `rustline_wasm::abi::HttpResult` paths keep resolving (the `segment.rs`
  re-export precedent).
- `crates/rustline-plugin-sdk` **deletes its duplicate declarations** and uses
  `rustline-abi`'s (it already depends on `rustline-abi`); keep its public
  re-exports so guest code is unchanged.
- Wire format is byte-identical (same field names/order/types/serde attrs) — the
  weather e2e boundary test still guards it.

**Tests.** A serde round-trip unit test in `rustline-abi` for each moved type
(the exact JSON the host emits decodes into the type); `just test-wasm` still
green (the real host→SDK path). No behavior change.

**Invariants depended on.** ABI wire compatibility (N-invariants); the SDK e2e
seam test.

### W52 — Consolidate the three fabricated-`Context` builders

**What.** One shared synthetic-`Context` builder replacing three near-duplicates.

**Design.**
- The three: `crates/rustline/src/bench/fixture.rs::fabricated_context` (bench,
  `#[cfg(feature = "bench")]`), `theme_cmd.rs::sample_context` (theme previews;
  already threads a `show_alerts` bool to peg warning/error readings), and
  `plugin_cmd.rs`'s `plugin run` harness fixture (W34).
- New **non-feature-gated** bin module `crates/rustline/src/sample_context.rs`
  exposing `sample_context(show_alerts: bool) -> Context` (populated, realistic
  fields; `show_alerts = true` pegs cpu/mem/battery/loadavg/disk to trip the
  warning+error badges, `false` = healthy). All three call sites delegate to it.
- **Load-bearing constraint:** the consolidation must be OUTPUT-preserving for
  each consumer — `theme show`/`theme pick`/`init` preview markup must stay
  byte-identical, and the bench fixture must remain equivalent. Pin the theme
  preview with a characterization test (render before == after). If a consumer
  needs a field the others don't, the single builder sets it for all (the fields
  are synthetic, so a superset is fine) unless it changes a consumer's output —
  then parameterize.

**Tests.** `sample_context(true)` trips warning+error badges when rendered;
`sample_context(false)` is healthy; a characterization test that the default-
layout `theme show`-style render is unchanged vs the pre-refactor builder.

**Invariants depended on.** #1 (still a synthetic `Context`, no env reads).

### W53 — `Registry::resolve` → `(name, widget)` pairs

**What.** Return each resolved widget alongside its layout name; delete
`assemble.rs`'s second registry traversal.

**Design.**
- `Registry::resolve(&[String]) -> Vec<(String, Box<dyn Widget>)>` (was
  `Vec<Box<dyn Widget>>`); it still skips unknown names with a `warn!` (an
  unknown name simply isn't in the returned pairs).
- `render_named_region` (`crates/rustline-core/src/assemble.rs`) uses the
  returned name directly for range-wrapping (`range_name()`) and the W29
  color-override lookup, **deleting** the `resolved_names`/`registry.contains`
  re-filter (`assemble.rs:~90`) and its implicit "the two traversals stay in the
  same order" invariant.
- Update every `resolve` caller (assemble + any tests). Output must be
  **byte-identical** — the names now come straight from `resolve` in the same
  order the widgets were built.

**Tests.** `resolve` returns `(name, widget)` pairs, skipping an unknown name;
a byte-identical characterization test of `render_named_region` over a
multi-widget region (incl. a clickable/`alt_format` widget and an unknown name)
before == after; the existing W29 color-override tests still pass unchanged.

**Invariants depended on.** #5 (segment left-to-right order unchanged), #7
(range name identity preserved — it now flows straight from `resolve`).

---

## Phase 2 — Persisted-sample widgets (feature-gap; opt-in)

**Shared plumbing.** Both widgets persist small state across cold spawns under
`state_root` (like the `cpu-sample` cache and the toggles file). Add ONE small
shared helper (a bin module, e.g. `crates/rustline/src/sample_store.rs`) for
best-effort atomic read/write of a per-widget state file (temp-file + rename,
`warn!` on failure — never fatal), reused by W45 and W47 rather than each
re-rolling temp-file+rename.

### W47 — Network throughput widget (`throughput`)

**What.** A `throughput` built-in showing up/down byte rates.

**Design.**
- New read surface `crates/rustline/src/throughput.rs`: `read_throughput(state_dir,
  interface: Option<&str>) -> Option<Throughput>`. Linux arm reads
  `/proc/net/dev`, parses via a pure `parse_proc_net_dev(&str) -> Vec<(iface, rx_bytes,
  tx_bytes)>`. Aggregate non-loopback interfaces (or the configured `interface`).
  Persist the prior `(rx, tx, ts)` sample via the shared `sample_store`; diff
  against it to compute `down`/`up` bytes-per-second (`(cur - prev)/(now - prev_ts)`,
  saturating; a counter reset / backward clock → treat as no-rate). Store the
  current sample afterward. First run (no prior) → `None`-rate. Non-Linux → `None`.
- `rustline-abi`: `Throughput { down_bytes_per_sec: u64, up_bytes_per_sec: u64 }`
  (chrono-free, re-exported by `rustline-core`). `Context.throughput: Option<Throughput>`,
  `#[serde(default)]`, in `Context::default()`; gated read on `layout_needs(layout,
  "throughput")`.
- Widget `crates/rustline-core/src/widgets/throughput.rs`: `ThroughputOpts { format
  (default e.g. " {down} {up}"), interface: Option<String>, down_format, alt_format }`;
  placeholders `{down}`/`{up}` (human-readable rates via `memory`'s
  `format_bytes` + `/s`, e.g. `1.2M/s`) and `{rx}`/`{tx}` as aliases if desired.
  `down_format` when `None`. Opt-in, click-toggleable. Register + bump the
  registry-count test **15 → 16**; add a `smoke.rs` graceful-render test.
- Name `throughput` avoids colliding with `net.rs` (the existing IP-selection
  helper, not a widget).

**Tests.** `parse_proc_net_dev` (well-formed, loopback excluded, malformed →
skip); rate computation from two synthetic samples (incl. counter-reset → no
rate, first-run → None); widget render (Some / None→down_format / placeholder
substitution); `smoke.rs` graceful render.

**Invariants depended on.** #1, #6 (no fabricated rate — `None` until a prior
sample exists), #3.

### W45 — Historical sparkline for cpu/memory

**What.** A `{spark}` placeholder on the `cpu`/`memory` widgets showing a
last-N-readings trend.

**Design.**
- Persistence at the `Context`-build edge (bin): when a widget's format contains
  `{spark}`, `build_context` reads that widget's persisted ring-buffer of the
  last-N readings via the shared `sample_store`, pushes the current reading,
  truncates to N, writes it back, and puts the history into `Context`. (Gated on
  `{spark}` presence so no I/O when unused.)
- `Context` carries the histories chrono-free: `Context.cpu_history: Vec<f32>`
  and `Context.mem_history: Vec<f32>` (percentages; `#[serde(default)]`, in
  `Context::default()`), so the pure widget reads them (invariant #1). Separate
  from cpu's existing single-sample fast-path snapshot (different purpose).
- A shared pure `sparkline(samples: &[f32], max: f32) -> String` (in a shared
  core module alongside `bar::gauge_bar`) maps each reading's fraction to one of
  `▁▂▃▄▅▆▇█` (8 levels). `cpu.rs`/`memory.rs` render `{spark}` from their
  history via it. `spark_width` config (default N, e.g. 8) bounds the ring and
  the rendered width.
- **Byte-identical** when `{spark}` is absent from the format (no history I/O,
  no output change).

**Tests.** `sparkline` glyph mapping (empty, single, ramp, all-equal, clamp
above max); ring-buffer push/truncate to N; `{spark}` render from a history;
byte-identical cpu/memory render when `{spark}` not in format.

**Invariants depended on.** #1 (history read at the build edge, widget reads
`Context`), #3.

---

## Phase 3 — WASM compiled-module cache + measurement (scale-perf; W43)

### W43 — `with_cache_config` seam + build-timing bench pass

**What.** Cache wasmtime's compiled module to disk so a later cold spawn
deserializes instead of recompiling, plus a bench pass that quantifies the win.

**Design (seam).**
- In `crates/rustline-wasm/src/host.rs::build_plugin`, insert
  `PluginBuilder::with_cache_config(<cfg>)` into the existing builder chain
  (before `.build()`), where `<cfg>` is a wasmtime cache-config TOML under the
  state root. A best-effort `wasmtime_cache_config_path() -> Option<PathBuf>`
  lazily ensures `<state_root>/wasmtime-cache.toml` exists (its `[cache]
  enabled = true` / `directory = <state_root>/wasmtime-cache/`), using the
  existing atomic temp-file+rename convention. On ANY failure (unwritable dir,
  etc.) it returns `None` and `build_plugin` proceeds WITHOUT the cache — never
  fatal (N2). The cache dir is kept distinct from any plugin state subdir.
- No `wasmtime` dependency added; no host bypass; all seven host functions +
  `with_wasi(false)` + fuel/timeout/memory caps stay bound unchanged. The cache
  changes only how the module is compiled, never guest authority (N1–N4 intact).
- Self-invalidating across an extism/wasmtime bump (wasmtime keys entries on its
  version + flags + module hash) — a correct one-time recompile after upgrade.

**Design (measurement).**
- Add a `build_plugin`-timing pass to `crates/rustline/src/bench/plugins.rs`
  (the current `--cold` reuses one compiled `Arc<Plugin>` and does NOT isolate
  the Cranelift-compile cost). The new pass times a full `build_plugin(bytes)`
  from raw bytes, cache-ON vs `with_cache_disabled()`, per plugin, reporting via
  the existing harness (`summarize`/`Row`/`Group`). This makes the deserialize-vs-
  compile delta measurable so the win is quantified rather than assumed.

**Tests.** Pure: `wasmtime_cache_config_path` behavior and the cache-config TOML
generation (content + best-effort None on an unwritable root). Integration
(`wasm-e2e` / manual): after a `build_plugin`, the cache dir is populated; the
bench pass runs and reports both timings. `cargo tree -i openssl` stays empty.

**Invariants depended on.** N1–N4 (guest authority unchanged — cache is compile
provenance only), N2 (best-effort, never breaks the bar).

---

## Cross-cutting work (final task)

1. **Registry-count test** → 16 (the new `throughput` widget). W45 adds no widget.
2. **Docs:** `CLAUDE.md` + `README.md` (both — standing rule) updated for: the
   `throughput` widget; the `{spark}` placeholder on `cpu`/`memory`; the wire-type
   hoist (module map now lists them under `rustline-abi`); the `Registry::resolve`
   pair change and the removed `resolved_names` re-filter; the shared
   `sample_context`/`sample_store` helpers; the WASM compile cache + the new bench
   pass. Move W43/W45/W47/W51/W52/W53 Roadmap entries to Done and strip them from
   `WHATS-NEXT.md` at branch-finish.
3. **Full green:** `just test`; `cargo clippy --all-targets -- -D warnings`;
   `cargo fmt --all --check`; `cargo tree -i openssl`/`-i native-tls` empty;
   `just test-wasm` (W43/W51 touch the guest/host path).

## Out of scope (explicitly not in this bundle)

- W50 (denials.jsonl quota) — a sibling follow-up, not selected.
- W48 daemon front-end (W43's `CompiledPlugin` in-process cache belongs there).
- W46 multiple widget instances, W44 widget CLI, W42 batched window render, and
  the other unchecked `WHATS-NEXT.md` items.

## Invariants this bundle depends on (pin with tests, don't assume)

- **#1** widgets read only `Context` — W45/W47 do all sample I/O at the build edge.
- **#3** `Config::load` total — W47/W45 new options `#[serde(default)]`.
- **#5 / #7** segment order + range-name identity — W53 must preserve both
  (byte-identical characterization).
- **#6** `Option` reads — W47 renders no fabricated rate before a prior sample.
- **N1–N4 / N2** — W43's cache is compile-provenance only and best-effort.
- **Wire compatibility** — W51 keeps the four result types byte-identical across
  the JSON boundary (host emits, guest decodes); the e2e test is the pin.
