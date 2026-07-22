# rustline `bench` tool — design

Date: 2026-07-21
Status: approved (via `/ship-it --ask`)

## Goal

A feature-gated benchmarking subcommand, `rustline bench`, that measures the
execution cost of the render pipeline at several granularities and prints a
report of nice tables. It answers three questions:

1. **How fast is pure rendering?** (the CPU cost of turning a `Context` into
   tmux markup, with no OS reads)
2. **What does a real `rustline render …` invocation actually cost?** (the
   end-to-end cost tmux pays every `status-interval`, including the deliberate
   ~120 ms `read_cpu` sample window and every other host read)
3. **Where does the time go?** (per-widget render cost, per data-source read
   cost, and per-plugin render cost, each in isolation)

The whole feature is gated behind a `bench` cargo feature (off by default). With
the feature off, the shipping `rustline` binary is byte-identical to today's and
carries none of the extra dependencies.

## Non-goals (YAGNI)

- **Not** a statistical-rigor tool (no regression tracking, outlier rejection,
  or HTML reports). This is a "rough sense of execution times" tool. We roll a
  small harness rather than pull in criterion/divan (both want to own `fn main`
  and their own output, which fights a custom multi-table report).
- **Not** a microbenchmark of every core helper (`gauge_bar`, `format_bytes`,
  the ANSI transcoder). We bench at the widget / region / read / plugin seams.
- **No** production-path refactor. The render/`build_context` code paths are not
  made generic over a probe trait; the pure pass fabricates a `Context` directly
  (see "The mock seam"). This respects invariant #1 and "never break the bar."
- **No** JSON output. Terminal tables and a Markdown variant (for pasting into
  PRs/reports) cover the stated "nice tables" requirement; both come from the
  same table crate via a preset swap.

## Form factor & feature gating

- New cargo feature in `crates/rustline/Cargo.toml`:
  ```toml
  [features]
  bench = ["dep:comfy-table"]
  ```
  `comfy-table` is added as an **optional** dependency, pulled in only by the
  feature. No other crate in the workspace is touched.
- `crates/rustline/src/main.rs` gains `#[cfg(feature = "bench")] mod bench;`.
- `crates/rustline/src/cli.rs` gains a `#[cfg(feature = "bench")]`
  `Command::Bench(BenchArgs)` variant. clap-derive supports `#[cfg]` on enum
  variants, so the subcommand simply does not exist when the feature is off.
- `main.rs`'s match arm for `Command::Bench` is likewise `#[cfg]`-gated and
  delegates to `bench::run(&args, &cfg, &config_path())`.

The tool lives **inside the `rustline` bin crate** (not a separate crate)
deliberately: the OS reads (`crate::cpu::read_cpu`, `crate::memory::read_memory`,
`crate::battery::read_battery`) and `crate::build_context::build_region_context`
already live there. A gated `bench/` module reaches them directly with zero
extra plumbing; a separate crate would require extracting them into a library
first.

`just bench` (currently a hand-rolled shell timing loop) is repointed at
`cargo run --release --features bench -- bench` so the recipe runs the real tool.

## Architecture

New module tree under `crates/rustline/src/bench/` (all `#[cfg(feature = "bench")]`):

- `mod.rs` — `run(args, cfg, config_path)`: orchestrates the passes selected by
  `--only`, assembles the report, writes it to stdout or `--output`.
- `harness.rs` — the measurement primitives, all pure/testable:
  - `Stats { n, min, median, mean, p95, max }` and
    `summarize(samples: &[Duration]) -> Stats`.
  - `measure(warmup: usize, iters: usize, mut f: impl FnMut()) -> Vec<Duration>`:
    runs `f` `warmup` times discarding results, then `iters` times recording an
    `Instant` delta each. The warmup phase is where any once-per-run expensive
    work (Extism instantiation, a cache-miss fetch, first-run data construction)
    is amortized before the timer starts.
- `fixture.rs` — `fabricated_context(now, layout) -> Context`: a `Context` with
  **every `Option` field populated `Some(...)`** with representative data. This
  is the mock seam (see below).
- `sources.rs` — the **source registry**: an ordered list of
  `(name, fn() -> ())` thunks, one per real OS read, that the "data sources"
  pass times. The registry is the single extension point for future slow reads.
- `plugins.rs` — plugin discovery + the cold / per-tick / warm-in-process
  measurements against real, preserved host state.
- `report.rs` — turns a set of named-and-grouped `Stats` into `comfy-table`
  output (pretty preset for the terminal, `ASCII_MARKDOWN` preset for
  `--format markdown`). Pure over its inputs, testable.

### The two-pass model

The architectural fact that makes this clean: **`Widget::render(&Context)` is
pure** — it reads only from `Context`, never the environment (invariant #1). All
slow work (the 120 ms `/proc/stat` sample, `vm_stat`, `getloadavg`, interface
enumeration, the toggles-file read, plugin network fetches) happens at the
`Context`-build edge, not in `render`.

Therefore:

- **Pure pass** — build a fabricated `Context` once, then time
  `render_named_region(dir, layout, &ctx, &registry, &theme)` /
  `registry.build(name)?.render(&ctx)`. No OS reads → no 120 ms. This measures
  the render pipeline's own CPU cost.
- **Real-world pass** — time `build_region_context(&args, &layout)` **plus** the
  render, paying every real read (including `read_cpu`'s 120 ms when the layout
  names `cpu`). This is the honest per-`status-interval` cost.

The delta between the two ≈ the sum of the reads, which the "data sources" pass
attributes to individual reads.

### The mock seam

`fabricated_context()` **is** the mock. Because render depends only on
`Context`, "mock the 120 ms read" reduces to "hand-build a `Context` whose
`cpu`/`memory`/`battery`/`loadavg`/… fields are already filled" — no read runs,
so no sleep. There is nothing to inject into production code.

Extending this to a future slow read is a two-line change, documented at both
sites:

1. Add the new read to the `sources.rs` registry so it is timed in isolation.
2. Populate its field in `fabricated_context()` so the pure pass keeps skipping
   it.

`fabricated_context` uses a **fixed** `now` (a hard-coded `DateTime<Local>`, as
the existing unit tests do) so the `datetime` widget renders deterministically
and the fixture pulls in no wall-clock read.

## What gets benched — the report

Five table groups, selectable via `--only regions|widgets|sources|plugins|all`
(default `all`). Each row reports `n`, `min`, `median`, `mean`, `p95`, `max`
as human-readable durations (ns/µs/ms). Group by group:

1. **Region render — pure** (fabricated `Context`, no reads)
   Rows: `left`, `right`, `window`, `full-bar` (left+right+window). Isolates the
   render pipeline; no 120 ms anywhere.
2. **Region end-to-end — real-world** (`build_region_context` + render, real reads)
   Same rows. The `right` row visibly carries the ~120 ms because the default
   right layout names `cpu`.
3. **Widget render — pure, isolated**
   One row per built-in widget resolvable from the effective config's registry
   (`pane_id, hostname, windows, cwd, loadavg, datetime, lan_ip, tailscale_ip,
   battery, cpu, memory`), each via `registry.build(name)?.render(&ctx)` on the
   fabricated `Context`.
4. **Data sources — real reads, isolated**
   One row per entry in the source registry: `read_cpu`, `read_memory`,
   `read_battery`, `read_loadavg`, `read_interfaces`, `read_toggles`. Shows which
   sources dominate (`read_cpu` ≈ 120 ms). Uses `--real-iters` (few) samples
   because these do real I/O.
5. **Plugins — opportunistic** (only if `*.wasm` discovered in the plugin dir)
   See "Plugin & state handling". Skipped with a one-line note when the plugin
   dir is absent/empty.

## Plugin & state handling (load-bearing)

Plugins (and any future stateful widget) may do expensive work **once** and
cache it: the `weather` example fetches wttr.in via the host's TTL-cached GET
(`rl_http_get_cached`) and serves from cache within `refresh_secs`. Benching a
plugin against an empty throwaway state dir would make **every** iteration a
cache miss — measuring the network, not the plugin. So:

- **Real, preserved state.** Plugin passes run through the real
  `rustline_wasm::register_plugins` path with the real `CapabilityCtx`: real
  `allowed_urls`/`allowed_paths`, real `state_root()`, real `max_state_bytes`.
  The host TTL cache and per-plugin state dir are **live and persist across every
  timed iteration**; state is never reset between iterations.
- **Warmup amortizes the once-per-run cost.** The `--warmup` phase pays the
  Extism build + first cache-miss fetch + first-run data construction before the
  timer starts.
- **Three honest measurements per discovered plugin:**
  - **cold-start** — fresh instantiate + first render on a cache **miss**. May
    hit the network, so it is **opt-in via `--cold`** and clearly labeled; by
    default the cache is **not** force-cleared, so the tool never triggers the
    expensive call just to benchmark it. 1 sample.
  - **per-tick (real-world)** — fresh Extism instantiation each iteration +
    render against the **warm** disk cache. This is the true cost per real
    `rustline render`, since each render is a new process: in-memory guest state
    resets but the on-disk cache survives. `--real-iters` samples.
  - **warm in-process** — one instance reused, render N times against the warm
    cache — the pure guest-render cost (relevant to a future daemon).
    `--iters` samples.
- **`--state-dir <path>`** overrides the state root for the bench run (default:
  the real `state_root()`), so a user who would rather not touch their live
  cache can point at a pre-warmed copy.

Built-in widgets are all pure/stateless over `Context` today, so this matters
only for plugins now; the harness nonetheless treats "real state available +
warmup amortizes once-per-run work" as the general rule, so a future stateful
built-in gets the same treatment.

## CLI surface

```
rustline bench [OPTIONS]

  --only <GROUP>       regions|widgets|sources|plugins|all   [default: all]
  --iters <N>          samples for fast/pure passes           [default: 1000]
  --real-iters <N>     samples for real-I/O passes            [default: 25]
  --warmup <N>         warmup iterations (discarded)          [default: 50]
  --cold               include plugin cold-start (may hit the network)
  --format <FMT>       table|markdown                         [default: table]
  --output <FILE>      write the report to FILE instead of stdout
  --plugin-dir <DIR>   override plugin discovery (same resolution as render)
  --state-dir <DIR>    override plugin state root (default: real state_root())
```

The effective `Config` is loaded exactly as the render path does, so region and
plugin passes reflect the user's real layout and plugin config.

Bench output goes to **stdout** (or `--output`); logging stays on its normal
sinks. The bench run quiets its own tracing so benchmarking does not spam the
log file.

## Dependencies

- `comfy-table` (optional, `bench` feature only): mature, no `unsafe`, no TLS,
  provides both a pretty Unicode preset and an `ASCII_MARKDOWN` preset, so
  `--format table|markdown` needs no second crate and no hand-rolled formatting.
- No other new dependencies. No `serde_json` (JSON is out of scope).
- `Cargo.lock` is committed with the change.
- rustls-only policy is preserved: `comfy-table` pulls in no TLS. `cargo tree -i
  openssl` / `-i native-tls` stay empty with **and** without `--features bench`.

## Testing strategy (TDD)

Unit tests live beside the code (`#[cfg(all(test, feature = "bench"))]`) and run
via `cargo test -p rustline --features bench`:

- **`summarize`** — min/median/mean/p95/max over known sample sets, incl.
  even/odd length (median), single-element, and unsorted input.
- **`measure`** — with a call-counting closure, assert it invokes `f` exactly
  `warmup + iters` times and returns exactly `iters` samples (warmup discarded).
- **`fabricated_context`** — the load-bearing fixture test: for every built-in
  widget name, `registry.build(name)?.render(&fabricated_context(...))` returns
  **non-empty** segments. This pins the invariant the pure pass depends on: the
  fixture is rich enough that no widget degrades to its `down_format`/empty path,
  so the pure pass exercises the real `format` branch. (Directly addresses the
  "invariant relied on but untested" red flag: the pure pass silently
  mis-measures if a widget falls to `down_format`, so we assert it can't.)
- **source registry** — assert the registry lists exactly the expected read
  names, so adding/removing a read without updating the bench is caught.
- **`report`** — format a fixed set of `Stats` and assert the rendered table
  contains the expected headers/rows (both presets).

Plus a **feature-off build guard**: `just lint` / CI builds the crate both with
and without `--features bench` (the default `cargo build` already exercises
feature-off; the plan adds an explicit `cargo build --features bench` step) to
ensure the gated code compiles and the default binary stays clean.

The wall-clock timing loop itself is **not** asserted on absolute durations
(inherently flaky); only its structural behavior (sample counts, warmup discard)
is tested.

## Invariants this feature depends on / must preserve

- **Depends on invariant #1** ("`Context` is the sole render input"). The pure
  pass is valid *only* because widgets read nothing but `Context`. The
  `fabricated_context` completeness test above is the guard: if a future widget
  reads the environment mid-render, the fixture test still passes but the pure
  number would drift from reality — so the plan notes that any new
  environment-read in a widget is a spec change here, not a silent one.
- **Preserves invariant #3** ("`Config::load` is total"): bench loads config the
  same total way; a bad config yields defaults, never a panic.
- **Preserves the "never break the bar" posture**: the feature adds no code to
  any render/`build_context`/`main` path that runs when the feature is off, and
  mutates no production behavior when on (it only *calls* existing reads/render).
- **Preserves the WASM capability invariants (N1–N4)**: plugin passes go through
  the unchanged `register_plugins`/`CapabilityCtx` path, so allowlists, the
  state sandbox, and per-plugin quota still gate every effect. `--state-dir`
  only relocates the (still-sandboxed) state root; it does not widen any grant.

## Doc updates (part of this branch)

- `CLAUDE.md`: note the `bench` feature + `rustline bench` subcommand, the
  `bench/` module in the module map, and the repointed `just bench` recipe.
- `README.md`: a short "Benchmarking" subsection.
- `justfile`: repoint `bench` at `cargo run --release --features bench -- bench`.
- Roadmap: mark the benchmarking tool done.

## Risks & edge cases

- **`--cold` hits the network.** Mitigated by making it opt-in and off by
  default; the default run never force-clears the cache.
- **Plugin dir absent / no wasm / wasm toolchain not installed.** The plugins
  group is skipped with a printed note; the rest of the report is unaffected.
- **`read_cpu` dominates run time.** With `--real-iters 25` and a 120 ms sample,
  the sources + real-world region passes take a few seconds; documented, and
  tunable via `--real-iters`.
- **Nested-worktree plugin build.** Not triggered by this tool (it discovers
  prebuilt `*.wasm`); building the example plugin remains `just build-weather`.
