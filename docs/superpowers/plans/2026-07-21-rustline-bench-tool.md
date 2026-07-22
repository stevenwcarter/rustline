# rustline `bench` tool Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a feature-gated `rustline bench` subcommand that times the render pipeline at region / widget / data-source / plugin granularity — a pure pass (fabricated `Context`, no OS reads, no 120 ms) and a real-world pass (real reads + render) — and prints the results as tables.

**Architecture:** Everything lives in the `rustline` bin crate under a new `#[cfg(feature = "bench")] mod bench`, so the reads (`crate::cpu::read_cpu`, …) and `build_region_context` are reachable directly. `Widget::render` is pure over `Context`, so the pure pass just hand-builds a `Context` (the mock seam); the real pass calls the real `build_region_context`. A tiny custom harness (`summarize`/`measure`) plus `comfy-table` produce the report. Plugin passes run through the unchanged `register_plugins`/`CapabilityCtx` path against the real, preserved state root so cached fast-paths are measured honestly.

**Tech Stack:** Rust edition 2024, clap derive, `comfy-table` (optional, `bench` feature), `std::time::{Instant, Duration}`.

## Global Constraints

- Edition 2024 in every crate; keep editions equal to `rustfmt.toml`.
- Must stay clippy-clean (`cargo clippy --all-targets -- -D warnings`) and rustfmt-clean (`cargo fmt --all --check`). **No `#[allow(dead_code)]`** — each task wires its new functions into `run()` in the same task so nothing is unused.
- The `bench` feature is **off by default**; with it off the binary and its dep graph are unchanged. `comfy-table` is an **optional** dependency pulled in only by the feature.
- rustls-only: `comfy-table` introduces no TLS. `cargo tree -i openssl` / `-i native-tls` must stay empty with and without `--features bench`.
- Commit `Cargo.lock` alongside the dependency change.
- Invariant #1 (Context is the sole render input) is what makes the pure pass valid — Task 2's fixture-completeness test guards it. Invariant #3 (`Config::load` total) and the WASM capability invariants (N1–N4) are preserved (bench only *calls* existing code paths).
- Tests for gated code run under `cargo test -p rustline --features bench`; `cargo test --workspace` (feature off) must stay green and hermetic.
- End every commit message with:
  `Claude-Session: https://claude.ai/code/session_01B1JNr3WhigAPa9Hx25SxHj`

## File structure

- `crates/rustline/Cargo.toml` — add `[features] bench = ["dep:comfy-table"]` and the optional `comfy-table` dep.
- `crates/rustline/src/cli.rs` — add gated `Command::Bench(BenchArgs)` variant + `BenchArgs`.
- `crates/rustline/src/main.rs` — add gated `mod bench;` + gated match arm.
- `crates/rustline/src/build_context.rs` — widen `read_loadavg`/`read_interfaces` to `pub(crate)` (Task 3).
- `crates/rustline/src/bench/mod.rs` — `run()` orchestration + module declarations.
- `crates/rustline/src/bench/harness.rs` — `Stats`, `summarize`, `measure`, `Row`, `Group`.
- `crates/rustline/src/bench/report.rs` — `render_report`, `fmt_dur`.
- `crates/rustline/src/bench/fixture.rs` — `fabricated_context`.
- `crates/rustline/src/bench/render_passes.rs` — `bench_widgets`, `bench_regions_pure`, `bench_regions_real`.
- `crates/rustline/src/bench/sources.rs` — `source_registry`, `bench_sources`.
- `crates/rustline/src/bench/plugins.rs` — `discover_wasm_stems`, `bench_plugins`.
- `justfile`, `CLAUDE.md`, `README.md` — Task 5.

---

### Task 1: Feature + CLI + harness + report + widgets pass (vertical slice)

Delivers a working `cargo run --features bench -- bench --only widgets` that prints a table.

**Files:**
- Modify: `crates/rustline/Cargo.toml`
- Modify: `crates/rustline/src/cli.rs`
- Modify: `crates/rustline/src/main.rs`
- Create: `crates/rustline/src/bench/mod.rs`
- Create: `crates/rustline/src/bench/harness.rs`
- Create: `crates/rustline/src/bench/report.rs`
- Create: `crates/rustline/src/bench/fixture.rs`
- Create: `crates/rustline/src/bench/render_passes.rs`

**Interfaces:**
- Produces:
  - `harness::Stats { n: usize, min/median/mean/p95/max: Duration }` (derive `Clone, Copy, Debug`)
  - `harness::summarize(samples: &[Duration]) -> Stats`
  - `harness::measure(warmup: usize, iters: usize, f: impl FnMut()) -> Vec<Duration>`
  - `harness::Row { label: String, stats: Stats }` (derive `Clone`), `harness::Group { title: String, note: Option<String>, rows: Vec<Row> }` (derive `Clone`)
  - `report::render_report(groups: &[Group], markdown: bool) -> String`
  - `fixture::fabricated_context() -> rustline_core::Context`
  - `render_passes::bench_widgets(cfg: &Config, iters: usize, warmup: usize) -> Group`
  - `render_passes::BUILTIN_WIDGETS: [&str; 11]`
  - `cli::BenchArgs` (gated), `cli::Command::Bench(BenchArgs)` (gated)
  - `bench::run(args: &BenchArgs, cfg: &Config)`

- [ ] **Step 1: Add the feature and dependency**

In `crates/rustline/Cargo.toml`, add to `[dependencies]`:
```toml
comfy-table = { version = "7", optional = true }
```
And to `[features]` (below the existing `wasm-e2e = []`):
```toml
# Opt-in `rustline bench` subcommand + its report deps. Off by default: the
# shipping binary and its dep graph are unchanged.
bench = ["dep:comfy-table"]
```

- [ ] **Step 2: Add the gated CLI surface**

In `crates/rustline/src/cli.rs`, add a variant to `enum Command` (after `Click(ClickArgs),`):
```rust
    /// Benchmark the render pipeline (feature `bench`).
    #[cfg(feature = "bench")]
    Bench(BenchArgs),
```
And append the args struct at the end of the file:
```rust
/// Arguments for `rustline bench` (feature `bench`).
#[cfg(feature = "bench")]
#[derive(Args, Debug)]
pub struct BenchArgs {
    /// Which group to bench: regions|widgets|sources|plugins|all.
    #[arg(long, default_value = "all")]
    pub only: String,
    /// Samples for the fast/pure passes.
    #[arg(long, default_value_t = 1000)]
    pub iters: usize,
    /// Samples for the real-I/O passes (reads, real-world regions, plugin per-tick).
    #[arg(long = "real-iters", default_value_t = 25)]
    pub real_iters: usize,
    /// Warmup iterations (discarded) for the pure passes.
    #[arg(long, default_value_t = 50)]
    pub warmup: usize,
    /// Include plugin cold-start (clears the plugin's cache; may hit the network).
    #[arg(long)]
    pub cold: bool,
    /// Output format: table|markdown.
    #[arg(long, default_value = "table")]
    pub format: String,
    /// Write the report to a file instead of stdout.
    #[arg(long)]
    pub output: Option<String>,
    /// Override the plugin discovery directory (same resolution as render).
    #[arg(long = "plugin-dir")]
    pub plugin_dir: Option<String>,
    /// Override the plugin state root (default: real state_root()).
    #[arg(long = "state-dir")]
    pub state_dir: Option<String>,
}
```

- [ ] **Step 3: Wire the module and match arm in main.rs**

In `crates/rustline/src/main.rs`, add after the other `mod` lines (e.g. after `mod battery;`… keep alphabetical-ish; put it with the others):
```rust
#[cfg(feature = "bench")]
mod bench;
```
Add a match arm to the `match cli.command { … }` block (after `Command::Click(args) => run_click(&args),`):
```rust
        #[cfg(feature = "bench")]
        Command::Bench(args) => bench::run(&args, &cfg),
```

- [ ] **Step 4: Write the harness with failing tests**

Create `crates/rustline/src/bench/harness.rs`:
```rust
//! Tiny timing harness: run a closure N times, summarize the sample durations.
//! No statistical rigor — a "rough sense" tool (see the bench spec).

use std::time::{Duration, Instant};

/// Summary statistics over a set of timing samples.
#[derive(Clone, Copy, Debug)]
pub struct Stats {
    pub n: usize,
    pub min: Duration,
    pub median: Duration,
    pub mean: Duration,
    pub p95: Duration,
    pub max: Duration,
}

/// One labelled result row.
#[derive(Clone)]
pub struct Row {
    pub label: String,
    pub stats: Stats,
}

/// A titled group of rows (one report table).
#[derive(Clone)]
pub struct Group {
    pub title: String,
    pub note: Option<String>,
    pub rows: Vec<Row>,
}

/// Summarize samples (min/median/mean/p95/max). Empty input → all-zero, `n = 0`.
pub fn summarize(samples: &[Duration]) -> Stats {
    if samples.is_empty() {
        return Stats {
            n: 0,
            min: Duration::ZERO,
            median: Duration::ZERO,
            mean: Duration::ZERO,
            p95: Duration::ZERO,
            max: Duration::ZERO,
        };
    }
    let mut nanos: Vec<u128> = samples.iter().map(Duration::as_nanos).collect();
    nanos.sort_unstable();
    let n = nanos.len();
    let sum: u128 = nanos.iter().sum();
    let mean = sum / n as u128;
    let median = if n % 2 == 1 {
        nanos[n / 2]
    } else {
        (nanos[n / 2 - 1] + nanos[n / 2]) / 2
    };
    // nearest-rank p95
    let idx = (((n as f64) * 0.95).ceil() as usize).saturating_sub(1).min(n - 1);
    let ns = |v: u128| Duration::from_nanos(v.min(u64::MAX as u128) as u64);
    Stats {
        n,
        min: ns(nanos[0]),
        median: ns(median),
        mean: ns(mean),
        p95: ns(nanos[idx]),
        max: ns(nanos[n - 1]),
    }
}

/// Run `f` `warmup` times (discarded), then `iters` times recording each
/// wall-clock duration. Returns exactly `iters` samples. The warmup phase is
/// where once-per-run expensive work (instantiation, a cache-miss fetch) is
/// amortized before timing starts.
pub fn measure(warmup: usize, iters: usize, mut f: impl FnMut()) -> Vec<Duration> {
    for _ in 0..warmup {
        f();
    }
    let mut samples = Vec::with_capacity(iters);
    for _ in 0..iters {
        let start = Instant::now();
        f();
        samples.push(start.elapsed());
    }
    samples
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(x: u64) -> Duration {
        Duration::from_millis(x)
    }

    #[test]
    fn summarize_sorted_odd() {
        let s = summarize(&[ms(10), ms(20), ms(30)]);
        assert_eq!(s.n, 3);
        assert_eq!(s.min, ms(10));
        assert_eq!(s.max, ms(30));
        assert_eq!(s.median, ms(20));
        assert_eq!(s.mean, ms(20));
        assert_eq!(s.p95, ms(30)); // ceil(0.95*3)=3 -> idx 2
    }

    #[test]
    fn summarize_even_median_and_unsorted() {
        assert_eq!(summarize(&[ms(10), ms(20), ms(30), ms(40)]).median, ms(25));
        let s = summarize(&[ms(30), ms(10), ms(20)]);
        assert_eq!(s.min, ms(10));
        assert_eq!(s.max, ms(30));
        assert_eq!(s.median, ms(20));
    }

    #[test]
    fn summarize_single_and_empty() {
        let s = summarize(&[ms(7)]);
        assert_eq!((s.min, s.median, s.p95, s.max), (ms(7), ms(7), ms(7), ms(7)));
        let e = summarize(&[]);
        assert_eq!(e.n, 0);
        assert_eq!(e.max, Duration::ZERO);
    }

    #[test]
    fn measure_runs_warmup_plus_iters_returns_iters() {
        let mut count = 0u32;
        let samples = measure(3, 5, || count += 1);
        assert_eq!(count, 8);
        assert_eq!(samples.len(), 5);
    }
}
```

- [ ] **Step 5: Create the report renderer**

Create `crates/rustline/src/bench/report.rs`:
```rust
//! Render `Group`s as tables via comfy-table (pretty preset for the terminal,
//! ASCII_MARKDOWN preset for `--format markdown`).

use std::time::Duration;

use comfy_table::{Table, presets};

use super::harness::Group;

/// Human-readable duration (ns/µs/ms/s).
pub fn fmt_dur(d: Duration) -> String {
    let ns = d.as_nanos();
    if ns < 1_000 {
        format!("{ns} ns")
    } else if ns < 1_000_000 {
        format!("{:.2} µs", ns as f64 / 1_000.0)
    } else if ns < 1_000_000_000 {
        format!("{:.2} ms", ns as f64 / 1_000_000.0)
    } else {
        format!("{:.3} s", ns as f64 / 1_000_000_000.0)
    }
}

/// Render all groups to a single string.
pub fn render_report(groups: &[Group], markdown: bool) -> String {
    let mut out = String::new();
    for g in groups {
        out.push('\n');
        out.push_str(&g.title);
        out.push('\n');
        if let Some(note) = &g.note {
            out.push_str("  ");
            out.push_str(note);
            out.push('\n');
        }
        if g.rows.is_empty() {
            continue;
        }
        let mut table = Table::new();
        table.load_preset(if markdown {
            presets::ASCII_MARKDOWN
        } else {
            presets::UTF8_FULL_CONDENSED
        });
        table.set_header(["pass", "n", "min", "median", "mean", "p95", "max"]);
        for row in &g.rows {
            table.add_row([
                row.label.clone(),
                row.stats.n.to_string(),
                fmt_dur(row.stats.min),
                fmt_dur(row.stats.median),
                fmt_dur(row.stats.mean),
                fmt_dur(row.stats.p95),
                fmt_dur(row.stats.max),
            ]);
        }
        out.push_str(&table.to_string());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::harness::{Row, summarize};

    fn group() -> Group {
        Group {
            title: "T".into(),
            note: None,
            rows: vec![Row {
                label: "cpu".into(),
                stats: summarize(&[Duration::from_millis(120)]),
            }],
        }
    }

    #[test]
    fn fmt_dur_units() {
        assert_eq!(fmt_dur(Duration::from_nanos(500)), "500 ns");
        assert!(fmt_dur(Duration::from_micros(3)).contains("µs"));
        assert!(fmt_dur(Duration::from_millis(120)).contains("ms"));
        assert!(fmt_dur(Duration::from_secs(2)).contains('s'));
    }

    #[test]
    fn report_contains_label_and_value() {
        let out = render_report(&[group()], false);
        assert!(out.contains("cpu"));
        assert!(out.contains("120"));
    }

    #[test]
    fn markdown_preset_uses_pipes() {
        let out = render_report(&[group()], true);
        assert!(out.contains('|'));
    }
}
```

- [ ] **Step 6: Create the fixture (the mock seam)**

Create `crates/rustline/src/bench/fixture.rs`:
```rust
//! The fabricated `Context` used by the pure passes. Because `Widget::render`
//! reads only from `Context` (invariant #1), a hand-built Context with every
//! `Option` field populated bypasses ALL OS reads — including `read_cpu`'s
//! ~120 ms sample. This IS the "mock": a future slow read is skipped by the
//! pure pass simply by filling its field here.

use chrono::{Local, TimeZone};
use rustline_core::{
    Battery, BatteryState, Context, CpuUsage, MemInfo, NetIface, WindowCtx,
};

/// A representative, fully-populated `Context`. Every widget renders its real
/// `format` branch on it (see the completeness test) — so no widget degrades to
/// `down_format`, which would make the pure numbers meaningless.
///
/// Interfaces carry both a LAN address (`192.168.1.42` on a non-virtual NIC, so
/// `pick_lan` selects it) and a Tailscale CGNAT address (`100.101.4.7`, so
/// `pick_tailscale` selects it) — see `rustline-core/src/widgets/net.rs`.
pub fn fabricated_context() -> Context {
    Context {
        session_name: "0".into(),
        window_index: "1".into(),
        pane_index: "0".into(),
        pane_current_path: "/home/steve/src/rustline".into(),
        home: "/home/steve".into(),
        hostname: "benchbox".into(),
        loadavg: Some([0.42, 0.37, 0.30]),
        now: Local
            .with_ymd_and_hms(2026, 7, 21, 12, 0, 0)
            .single()
            .expect("fixed timestamp is valid"),
        window: Some(WindowCtx {
            index: "1".into(),
            name: "editor".into(),
            flags: "*".into(),
            is_current: true,
        }),
        interfaces: vec![
            NetIface {
                name: "eth0".into(),
                ipv4: "192.168.1.42".parse().expect("valid ipv4"),
            },
            NetIface {
                name: "tailscale0".into(),
                ipv4: "100.101.4.7".parse().expect("valid ipv4"),
            },
        ],
        battery: Some(Battery {
            percent: 76,
            state: BatteryState::Discharging,
        }),
        cpu: Some(CpuUsage { percent: 23.5 }),
        memory: Some(MemInfo {
            total_bytes: 16 * 1024 * 1024 * 1024,
            used_bytes: 6 * 1024 * 1024 * 1024,
            available_bytes: 10 * 1024 * 1024 * 1024,
        }),
        os: "linux".into(),
        arch: "x86_64".into(),
        toggled: Default::default(),
    }
}
```
> If any field name/shape here disagrees with `rustline-core/src/context.rs`, follow the struct there — this is the single source of truth for `Context`.

- [ ] **Step 7: Write the failing widgets-pass + fixture-completeness tests**

Create `crates/rustline/src/bench/render_passes.rs` with the widgets pass and its tests (region passes are added in Task 2):
```rust
//! Timing passes over the render pipeline. Pure passes use `fabricated_context`
//! (no reads); the real-world region pass (Task 2) uses `build_region_context`.

use rustline_core::{Config, Registry};

use super::fixture::fabricated_context;
use super::harness::{Group, Row, measure, summarize};

/// The built-in widget names, benched individually. Kept explicit so a missing
/// registration is caught by the completeness test.
pub const BUILTIN_WIDGETS: [&str; 11] = [
    "pane_id",
    "hostname",
    "windows",
    "cwd",
    "loadavg",
    "datetime",
    "lan_ip",
    "tailscale_ip",
    "battery",
    "cpu",
    "memory",
];

/// Time each built-in widget's `render` in isolation on the fabricated Context.
pub fn bench_widgets(cfg: &Config, iters: usize, warmup: usize) -> Group {
    let registry = Registry::with_builtins(cfg);
    let ctx = fabricated_context();
    let mut rows = Vec::new();
    for name in BUILTIN_WIDGETS {
        let Some(widget) = registry.build(name) else {
            continue;
        };
        let samples = measure(warmup, iters, || {
            let _ = widget.render(&ctx);
        });
        rows.push(Row {
            label: name.to_string(),
            stats: summarize(&samples),
        });
    }
    Group {
        title: "Widget render — pure, isolated".into(),
        note: None,
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_renders_nonempty_on_fixture() {
        // Load-bearing: pins invariant #1's usefulness. If any widget degraded
        // to its empty/down_format path on the fixture, the pure numbers would
        // be measuring the wrong branch — so assert none does.
        let cfg = Config::default();
        let registry = Registry::with_builtins(&cfg);
        let ctx = fabricated_context();
        for name in BUILTIN_WIDGETS {
            let widget = registry
                .build(name)
                .unwrap_or_else(|| panic!("built-in {name} not registered"));
            assert!(
                !widget.render(&ctx).is_empty(),
                "widget {name} rendered empty on the fabricated context"
            );
        }
    }

    #[test]
    fn bench_widgets_has_a_row_per_builtin() {
        let g = bench_widgets(&Config::default(), 2, 0);
        assert_eq!(g.rows.len(), BUILTIN_WIDGETS.len());
        assert!(g.rows.iter().all(|r| r.stats.n == 2));
        assert!(g.rows.iter().any(|r| r.label == "cpu"));
    }
}
```

- [ ] **Step 8: Create the orchestrator `mod.rs`**

Create `crates/rustline/src/bench/mod.rs`:
```rust
//! `rustline bench`: time the render pipeline and print tables. Feature-gated.

mod fixture;
mod harness;
mod render_passes;
mod report;

use rustline_core::Config;

use crate::cli::BenchArgs;
use harness::Group;

/// Entry point for `rustline bench`.
pub fn run(args: &BenchArgs, cfg: &Config) {
    // `--state-dir` relocates state_root()/data_root() (both key off
    // XDG_DATA_HOME). Set before any read/plugin instantiation.
    if let Some(dir) = &args.state_dir {
        // SAFETY: set once at the very top of the bench command, before any
        // reads or wasm host threads are spawned; the process is single-threaded
        // here. This is a bench-only tool.
        unsafe {
            std::env::set_var("XDG_DATA_HOME", dir);
        }
    }

    let only = args.only.as_str();
    let want = |g: &str| only == "all" || only == g;
    let mut groups: Vec<Group> = Vec::new();

    if want("widgets") {
        groups.push(render_passes::bench_widgets(cfg, args.iters, args.warmup));
    }

    let markdown = args.format == "markdown";
    let text = report::render_report(&groups, markdown);
    match &args.output {
        Some(path) => match std::fs::write(path, &text) {
            Ok(()) => println!("wrote report to {path}"),
            Err(error) => {
                eprintln!("failed to write {path}: {error}");
                print!("{text}");
            }
        },
        None => print!("{text}"),
    }
}
```

- [ ] **Step 9: Verify feature-off and feature-on both build; run tests**

Run:
```bash
cargo build -p rustline
cargo build -p rustline --features bench
cargo test -p rustline --features bench bench:: 2>&1 | tail -20
```
Expected: both builds succeed; the harness/report/render_passes tests pass. Then a smoke run:
```bash
cargo run -q -p rustline --features bench -- bench --only widgets
```
Expected: a "Widget render — pure, isolated" table with 11 rows.

- [ ] **Step 10: Lint and commit**

```bash
cargo fmt --all
cargo clippy -p rustline --all-targets --features bench -- -D warnings
cargo clippy --all-targets -- -D warnings
git add -A
git commit -m "$(cat <<'EOF'
feat(bench): bench feature, harness, report, and widgets pass

Gated `rustline bench` subcommand. Custom summarize/measure harness,
comfy-table report, the fabricated-Context mock seam (with a completeness
test pinning invariant #1), and the per-built-in-widget pure pass.

Claude-Session: https://claude.ai/code/session_01B1JNr3WhigAPa9Hx25SxHj
EOF
)"
```

---

### Task 2: Region passes (pure + real-world) + run() integration test

**Files:**
- Modify: `crates/rustline/src/bench/render_passes.rs`
- Modify: `crates/rustline/src/bench/mod.rs`

**Interfaces:**
- Consumes: `fixture::fabricated_context`, `harness::{measure, summarize, Group, Row}`, `crate::build_context::{build_region_context, build_window_context}`, `crate::cli::{RegionArgs, WindowArgs}`, `rustline_core::{Direction, Registry, render_named_region, render_window}`.
- Produces: `render_passes::bench_regions_pure(cfg, iters, warmup) -> Group`, `render_passes::bench_regions_real(cfg, real_iters, warmup) -> Group`.

- [ ] **Step 1: Write failing region-pass tests**

Add to the `tests` module in `render_passes.rs`:
```rust
    #[test]
    fn regions_pure_has_expected_rows() {
        let g = bench_regions_pure(&Config::default(), 2, 0);
        let labels: Vec<&str> = g.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["left", "right", "window", "full-bar"]);
        assert!(g.rows.iter().all(|r| r.stats.n == 2));
    }

    #[test]
    fn regions_real_has_region_rows() {
        // real_iters=1 keeps this quick despite read_cpu's ~120ms.
        let g = bench_regions_real(&Config::default(), 1, 0);
        let labels: Vec<&str> = g.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["left", "right", "window"]);
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rustline --features bench regions_ 2>&1 | tail`
Expected: FAIL — `bench_regions_pure`/`bench_regions_real` not found.

- [ ] **Step 3: Implement the region passes**

Add to `render_passes.rs` (imports at top: extend the `rustline_core` use and add the two others):
```rust
use rustline_core::{Config, Direction, Registry, render_named_region, render_window};

use crate::build_context::{build_region_context, build_window_context};
use crate::cli::{RegionArgs, WindowArgs};
```
And the functions:
```rust
/// Pure render of each region on the fabricated Context (no OS reads).
pub fn bench_regions_pure(cfg: &Config, iters: usize, warmup: usize) -> Group {
    let registry = Registry::with_builtins(cfg);
    let theme = cfg.to_theme();
    let ctx = fabricated_context();
    let left = cfg.layout.left.clone();
    let right = cfg.layout.right.clone();
    let row = |label: &str, f: &mut dyn FnMut()| Row {
        label: label.to_string(),
        stats: summarize(&measure(warmup, iters, f)),
    };
    let mut rows = Vec::new();
    rows.push(row("left", &mut || {
        let _ = render_named_region(Direction::Left, &left, &ctx, &registry, &theme);
    }));
    rows.push(row("right", &mut || {
        let _ = render_named_region(Direction::Right, &right, &ctx, &registry, &theme);
    }));
    rows.push(row("window", &mut || {
        let _ = render_window(&ctx, &registry, &theme);
    }));
    rows.push(row("full-bar", &mut || {
        let _ = render_named_region(Direction::Left, &left, &ctx, &registry, &theme);
        let _ = render_named_region(Direction::Right, &right, &ctx, &registry, &theme);
        let _ = render_window(&ctx, &registry, &theme);
    }));
    Group {
        title: "Region render — pure (fabricated Context, no reads)".into(),
        note: None,
        rows,
    }
}

/// Real-world render of each region: real `build_region_context` (all OS reads,
/// incl. read_cpu's ~120 ms when the layout names `cpu`) + render.
pub fn bench_regions_real(cfg: &Config, real_iters: usize, warmup: usize) -> Group {
    let registry = Registry::with_builtins(cfg);
    let theme = cfg.to_theme();
    let left = cfg.layout.left.clone();
    let right = cfg.layout.right.clone();
    let region_args = RegionArgs::default();
    let win_args = WindowArgs {
        current: true,
        index: "1".into(),
        name: "editor".into(),
        flags: "*".into(),
        preview: false,
    };
    let mut rows = Vec::new();
    rows.push(Row {
        label: "left".into(),
        stats: summarize(&measure(warmup, real_iters, || {
            let ctx = build_region_context(&region_args, &left);
            let _ = render_named_region(Direction::Left, &left, &ctx, &registry, &theme);
        })),
    });
    rows.push(Row {
        label: "right".into(),
        stats: summarize(&measure(warmup, real_iters, || {
            let ctx = build_region_context(&region_args, &right);
            let _ = render_named_region(Direction::Right, &right, &ctx, &registry, &theme);
        })),
    });
    rows.push(Row {
        label: "window".into(),
        stats: summarize(&measure(warmup, real_iters, || {
            let ctx = build_window_context(&win_args);
            let _ = render_window(&ctx, &registry, &theme);
        })),
    });
    Group {
        title: "Region end-to-end — real-world (build_context + render)".into(),
        note: Some("`right` includes read_cpu's ~120ms sample window".into()),
        rows,
    }
}
```
> `WindowArgs` has no `Default` — construct it literally as shown. If its fields differ from `cli.rs`, match that file.

- [ ] **Step 4: Wire into run() and add the integration test**

In `bench/mod.rs`, extend the dispatch (after the widgets block):
```rust
    if want("regions") {
        groups.push(render_passes::bench_regions_pure(cfg, args.iters, args.warmup));
        // Real passes: small fixed warmup (each `right` build pays ~120ms).
        groups.push(render_passes::bench_regions_real(cfg, args.real_iters, 2));
    }
```
Add a `#[cfg(test)]` module at the bottom of `bench/mod.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::BenchArgs;

    fn args(only: &str, out: &str) -> BenchArgs {
        BenchArgs {
            only: only.into(),
            iters: 2,
            real_iters: 1,
            warmup: 0,
            cold: false,
            format: "markdown".into(),
            output: Some(out.into()),
            plugin_dir: None,
            state_dir: None,
        }
    }

    #[test]
    fn run_writes_widget_report_to_output_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("report.md");
        run(&args("widgets", path.to_str().unwrap()), &Config::default());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("cpu"));
        assert!(content.contains('|')); // markdown table
    }
}
```
> `tempfile` is already a dev-dependency of `rustline`.

- [ ] **Step 5: Run tests, lint, commit**

```bash
cargo test -p rustline --features bench bench:: 2>&1 | tail -20
cargo fmt --all
cargo clippy -p rustline --all-targets --features bench -- -D warnings
git add -A
git commit -m "$(cat <<'EOF'
feat(bench): region passes (pure + real-world) and run() dispatch

Adds pure region render (fabricated Context) and real-world end-to-end
(build_context + render) passes, wires `--only regions`, and an integration
test that runs the command to a temp file.

Claude-Session: https://claude.ai/code/session_01B1JNr3WhigAPa9Hx25SxHj
EOF
)"
```

---

### Task 3: Data-source pass

**Files:**
- Modify: `crates/rustline/src/build_context.rs`
- Create: `crates/rustline/src/bench/sources.rs`
- Modify: `crates/rustline/src/bench/mod.rs`

**Interfaces:**
- Consumes: `crate::cpu::read_cpu`, `crate::memory::read_memory`, `crate::battery::read_battery`, `crate::build_context::{read_loadavg, read_interfaces}`, `crate::toggles::read_toggles`, `harness::{measure, summarize, Group, Row}`.
- Produces: `sources::source_registry() -> Vec<(&'static str, fn())>`, `sources::bench_sources(real_iters, warmup) -> Group`.

- [ ] **Step 1: Widen two reads to `pub(crate)`**

In `crates/rustline/src/build_context.rs`, change:
```rust
fn read_loadavg() -> Option<[f64; 3]> {
```
to
```rust
pub(crate) fn read_loadavg() -> Option<[f64; 3]> {
```
and
```rust
fn read_interfaces() -> Vec<NetIface> {
```
to
```rust
pub(crate) fn read_interfaces() -> Vec<NetIface> {
```

- [ ] **Step 2: Write failing source tests**

Create `crates/rustline/src/bench/sources.rs`:
```rust
//! Timing the individual OS reads that populate `Context`. The single extension
//! point for future slow reads: add the read here (and fill its field in
//! `fixture::fabricated_context` so the pure passes keep skipping it).

use super::harness::{Group, Row, measure, summarize};

/// Named thunks over each real read, timed in isolation.
pub fn source_registry() -> Vec<(&'static str, fn())> {
    vec![
        ("read_cpu", || {
            let _ = crate::cpu::read_cpu();
        }),
        ("read_memory", || {
            let _ = crate::memory::read_memory();
        }),
        ("read_battery", || {
            let _ = crate::battery::read_battery();
        }),
        ("read_loadavg", || {
            let _ = crate::build_context::read_loadavg();
        }),
        ("read_interfaces", || {
            let _ = crate::build_context::read_interfaces();
        }),
        ("read_toggles", || {
            let _ = crate::toggles::read_toggles();
        }),
    ]
}

/// Time each read in the source registry.
pub fn bench_sources(real_iters: usize, warmup: usize) -> Group {
    let rows = source_registry()
        .into_iter()
        .map(|(name, f)| Row {
            label: name.to_string(),
            stats: summarize(&measure(warmup, real_iters, f)),
        })
        .collect();
    Group {
        title: "Data sources — real reads, isolated".into(),
        note: Some("read_cpu sleeps ~120ms sampling /proc/stat".into()),
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lists_expected_reads() {
        let names: Vec<&str> = source_registry().iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            [
                "read_cpu",
                "read_memory",
                "read_battery",
                "read_loadavg",
                "read_interfaces",
                "read_toggles",
            ]
        );
    }

    #[test]
    fn bench_sources_rows_match_registry() {
        let g = bench_sources(1, 0);
        assert_eq!(g.rows.len(), 6);
        assert_eq!(g.rows[0].label, "read_cpu");
    }
}
```

- [ ] **Step 3: Run to verify failure, then wire the module**

Run: `cargo test -p rustline --features bench sources 2>&1 | tail`
Expected: FAIL — `bench::sources` module not declared.

In `bench/mod.rs`, add `mod sources;` to the module list and extend dispatch (after the regions block):
```rust
    if want("sources") {
        groups.push(sources::bench_sources(args.real_iters, 2));
    }
```

- [ ] **Step 4: Run tests, lint, commit**

```bash
cargo test -p rustline --features bench bench:: 2>&1 | tail -20
cargo fmt --all
cargo clippy -p rustline --all-targets --features bench -- -D warnings
cargo clippy --all-targets -- -D warnings   # feature-off still clean (pub(crate) reads now used only under feature)
git add -A
git commit -m "$(cat <<'EOF'
feat(bench): isolated data-source read pass

Times read_cpu/read_memory/read_battery/read_loadavg/read_interfaces/
read_toggles individually. Widens read_loadavg/read_interfaces to pub(crate).
The source registry is the extension point for future slow reads.

Claude-Session: https://claude.ai/code/session_01B1JNr3WhigAPa9Hx25SxHj
EOF
)"
```
> If the feature-off clippy warns that `read_loadavg`/`read_interfaces` are never used without the feature: they ARE still used by `build_region_context` in normal builds, so no warning is expected. If one appears, it means the call site changed — re-check `build_context.rs`.

---

### Task 4: Plugin pass (real, preserved state)

**Files:**
- Create: `crates/rustline/src/bench/plugins.rs`
- Modify: `crates/rustline/src/bench/mod.rs`

**Interfaces:**
- Consumes: `crate::resolve_plugin_dir` (crate-root fn), `rustline_wasm::{register_plugins, state_root}`, `rustline_core::Registry`, `fixture::fabricated_context`, `harness::{measure, summarize, Group, Row}`, `crate::cli::BenchArgs`.
- Produces: `plugins::bench_plugins(cfg: &Config, args: &BenchArgs, iters: usize, real_iters: usize) -> Group`.

- [ ] **Step 1: Write the failing skip-path test**

Create `crates/rustline/src/bench/plugins.rs`:
```rust
//! Plugin timing against real, preserved host state so cached fast-paths are
//! measured honestly (an empty state dir would make every render a cache miss).
//! Three measurements per discovered plugin:
//!   - first call: fresh instantiate + first render (cold if the cache is stale)
//!   - per-tick:   fresh instantiate + render vs the WARM disk cache (the true
//!                 cost per real `rustline render`, which is a new process)
//!   - warm:       one instance reused, render only (future-daemon cost)
//! `--cold` additionally clears the plugin's state dir to force a genuine miss.

use std::path::{Path, PathBuf};

use rustline_core::{Config, Registry};

use super::fixture::fabricated_context;
use super::harness::{Group, Row, measure, summarize};
use crate::cli::BenchArgs;

/// The `*.wasm` filename stems in `dir` (empty if the dir is missing).
fn discover_wasm_stems(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension().and_then(|x| x.to_str()) == Some("wasm"))
                .then(|| p.file_stem().and_then(|s| s.to_str()).map(str::to_string))
                .flatten()
        })
        .collect()
}

/// Bench each discovered plugin. Skips with a note when none are present.
pub fn bench_plugins(cfg: &Config, args: &BenchArgs, iters: usize, real_iters: usize) -> Group {
    let plugin_dir: PathBuf = crate::resolve_plugin_dir(args.plugin_dir.as_deref(), cfg);
    let stems = discover_wasm_stems(&plugin_dir);
    if stems.is_empty() {
        return Group {
            title: "Plugins".into(),
            note: Some(format!(
                "no *.wasm in {} — skipped (build one with `just build-weather`)",
                plugin_dir.display()
            )),
            rows: Vec::new(),
        };
    }

    let mut registry = Registry::new();
    rustline_wasm::register_plugins(&mut registry, cfg, &plugin_dir, &stems);
    let ctx = fabricated_context();
    let mut rows = Vec::new();

    for stem in &stems {
        if !registry.contains(stem) {
            rows.push(Row {
                label: format!("{stem} (failed to instantiate — skipped)"),
                stats: summarize(&[]),
            });
            continue;
        }

        // Optional cold-start: clear the plugin's state dir to force a miss.
        if args.cold {
            let _ = std::fs::remove_dir_all(rustline_wasm::state_root().join(stem));
            let first = measure(0, 1, || {
                if let Some(w) = registry.build(stem) {
                    let _ = w.render(&ctx);
                }
            });
            rows.push(Row {
                label: format!("{stem} (cold-start: cache cleared, may hit network)"),
                stats: summarize(&first),
            });
        }

        // Warm the disk cache before the timed passes.
        if let Some(w) = registry.build(stem) {
            for _ in 0..2 {
                let _ = w.render(&ctx);
            }
        }

        // per-tick: fresh instantiate + render each iteration, warm cache.
        let per_tick = measure(0, real_iters, || {
            if let Some(w) = registry.build(stem) {
                let _ = w.render(&ctx);
            }
        });
        rows.push(Row {
            label: format!("{stem} (per-tick: instantiate + render, warm cache)"),
            stats: summarize(&per_tick),
        });

        // warm in-process: reuse one instance.
        if let Some(w) = registry.build(stem) {
            let warm = measure(2, iters, || {
                let _ = w.render(&ctx);
            });
            rows.push(Row {
                label: format!("{stem} (warm in-process render)"),
                stats: summarize(&warm),
            });
        }
    }

    Group {
        title: "Plugins — real preserved state".into(),
        note: None,
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bench_args_with_plugin_dir(dir: &str) -> BenchArgs {
        BenchArgs {
            only: "plugins".into(),
            iters: 1,
            real_iters: 1,
            warmup: 0,
            cold: false,
            format: "table".into(),
            output: None,
            plugin_dir: Some(dir.into()),
            state_dir: None,
        }
    }

    #[test]
    fn empty_plugin_dir_is_skipped_with_note() {
        let tmp = tempfile::tempdir().unwrap();
        let args = bench_args_with_plugin_dir(tmp.path().to_str().unwrap());
        let g = bench_plugins(&Config::default(), &args, 1, 1);
        assert!(g.rows.is_empty());
        assert!(g.note.as_deref().unwrap().contains("no *.wasm"));
    }
}
```
> Confirm `rustline_wasm::state_root()` is re-exported (it is, per `rustline-wasm/src/lib.rs`). If `--cold`'s `remove_dir_all` target doesn't cover the HTTP cache (see `rustline-wasm/src/cache.rs`/`paths.rs` for where cached GETs are stored), widen the cleared path to match — cold-start must start from a genuine miss. This does not affect the hermetic test.

- [ ] **Step 2: Run to verify failure, wire the module**

Run: `cargo test -p rustline --features bench plugins 2>&1 | tail`
Expected: FAIL — `bench::plugins` not declared.

In `bench/mod.rs`, add `mod plugins;` and extend dispatch (after the sources block):
```rust
    if want("plugins") {
        groups.push(plugins::bench_plugins(cfg, args, args.iters, args.real_iters));
    }
```

- [ ] **Step 3: Run tests, lint, commit**

```bash
cargo test -p rustline --features bench bench:: 2>&1 | tail -20
cargo fmt --all
cargo clippy -p rustline --all-targets --features bench -- -D warnings
git add -A
git commit -m "$(cat <<'EOF'
feat(bench): plugin pass against real preserved state

Discovers *.wasm, registers via the real register_plugins/CapabilityCtx path
(real state_root, warm cache), and measures first-call / per-tick / warm
in-process render. `--cold` clears the plugin's state dir to force a miss.
Skips with a note when no plugins are present.

Claude-Session: https://claude.ai/code/session_01B1JNr3WhigAPa9Hx25SxHj
EOF
)"
```

- [ ] **Step 4: Manual real-plugin verification (documented, not hermetic)**

```bash
just build-weather
cargo run -q -p rustline --features bench -- bench --only plugins
```
Expected: a "Plugins — real preserved state" table with `weather (per-tick …)` and `weather (warm in-process …)` rows. (Requires network only on a cache miss; the warm rows should be far faster than a cold fetch.)

---

### Task 5: justfile, docs, and final verification

**Files:**
- Modify: `justfile`
- Modify: `CLAUDE.md`
- Modify: `README.md`

**Interfaces:** none (docs + recipe only).

- [ ] **Step 1: Repoint the `just bench` recipe**

Replace the entire `bench:` recipe in `justfile` (the shell timing loop) with:
```makefile
# Benchmark the render pipeline (regions, widgets, data sources, plugins).
# Pure passes use a fabricated Context (no OS reads); real-world passes pay the
# real reads incl. read_cpu's ~120ms sample. See `rustline bench --help`.
bench *ARGS: build-weather
    cargo run -q --release --features bench -- bench {{ARGS}}
```
> `build-weather` is a dependency so the plugins group has something to measure; it is a no-op reinstall if already built. If the wasm target isn't installed, `build-weather` adds it. To bench without plugins, run `just bench --only widgets` etc.

- [ ] **Step 2: Update CLAUDE.md**

In the module map under `rustline (bin):`, add a bullet:
```markdown
- `bench/` (`#[cfg(feature = "bench")]`) — the `rustline bench` subcommand:
  `harness.rs` (`summarize`/`measure`/`Stats`/`Row`/`Group`), `fixture.rs`
  (`fabricated_context` — the pure-pass mock seam), `render_passes.rs` (pure
  widget + pure/real region passes), `sources.rs` (per-read timing + the
  source registry), `plugins.rs` (real-preserved-state plugin timing), and
  `report.rs` (comfy-table pretty/markdown). Gated behind the `bench` cargo
  feature; the default binary is unchanged.
```
In the CLI section, add:
```markdown
- `rustline bench [--only regions|widgets|sources|plugins|all] [--iters N]
  [--real-iters N] [--warmup N] [--cold] [--format table|markdown]
  [--output FILE] [--plugin-dir DIR] [--state-dir DIR]` — feature-gated
  (`--features bench`) benchmark of the render pipeline: a pure pass
  (fabricated `Context`, no reads) vs a real-world pass (real reads + render),
  plus per-widget, per-read, and per-plugin timing. Plugin passes run against
  real preserved state so cached fast-paths are measured honestly.
```
In the Development `just` recipes list, update the `bench` mention (or add): `just bench [ARGS]` runs the real tool. In the Roadmap, add a "Done:" bullet for the benchmarking tool and link the spec/plan:
```markdown
- Done: `rustline bench` benchmarking tool — feature-gated subcommand timing
  regions/widgets/data-sources/plugins, pure (fabricated Context) vs real-world
  passes, real preserved plugin state. See
  `docs/superpowers/specs/2026-07-21-rustline-bench-tool-design.md` /
  `docs/superpowers/plans/2026-07-21-rustline-bench-tool.md`.
```

- [ ] **Step 3: Update README.md**

Add a short "Benchmarking" subsection (place it near the development/usage section — match the file's existing heading style):
```markdown
## Benchmarking

`rustline bench` (build with `--features bench`, or run `just bench`) times the
render pipeline: a **pure** pass on a fabricated `Context` (no OS reads, so no
`read_cpu` ~120 ms sample) versus a **real-world** pass that pays the real
reads, plus per-widget, per-data-source, and per-plugin breakdowns as tables.

```sh
just bench                       # all groups
just bench --only widgets        # just the per-widget render costs
cargo run --features bench -- bench --format markdown --output bench.md
```

Plugin passes run against the real, preserved plugin state/cache, so a cached
plugin (e.g. `weather`) is measured on its fast cached path rather than
re-fetching every iteration.
```

- [ ] **Step 4: Full verification (feature off and on)**

Run and confirm all succeed:
```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo clippy -p rustline --all-targets --features bench -- -D warnings
cargo test --workspace
cargo test -p rustline --features bench bench::
cargo build -p rustline           # feature-off binary builds
cargo tree -i openssl 2>&1 | head -1        # expect: nothing / "not found"
cargo tree -i native-tls --features bench 2>&1 | head -1   # expect: nothing / "not found"
```
Expected: fmt clean, both clippy runs clean, workspace tests green, bench tests green, no openssl/native-tls in the graph.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
docs(bench): justfile recipe, CLAUDE.md, README; final verification

Repoints `just bench` at the real tool, documents the subcommand + module map,
and adds a README Benchmarking section.

Claude-Session: https://claude.ai/code/session_01B1JNr3WhigAPa9Hx25SxHj
EOF
)"
```

---

## Self-Review

**Spec coverage:**
- Feature-gated subcommand, custom harness, comfy-table → Tasks 1, 5. ✅
- Two-pass (pure fabricated Context vs real build_context) for regions → Task 2. ✅
- Per-widget isolated pass → Task 1. ✅
- Per-data-source (read_cpu etc.) isolated pass + future-read extension point → Task 3. ✅
- Plugin pass with real preserved state, first/per-tick/warm, `--cold`, `--state-dir` → Task 4 (state-dir handled in `run()`, Task 1). ✅
- Nice tables + markdown + `--output` → Tasks 1, 2. ✅
- Fixture-completeness test guarding invariant #1 → Task 1. ✅
- Feature-off binary unchanged + rustls-only checks → Task 5. ✅
- Docs (CLAUDE.md, README, justfile, roadmap) → Task 5. ✅

**Placeholder scan:** No TBD/TODO; every code step has complete code. ✅

**Type consistency:** `Stats`/`Row`/`Group` field names and `summarize`/`measure`/`render_report`/`fabricated_context`/`bench_*` signatures are consistent across tasks. `BenchArgs` field set is identical in the CLI definition (Task 1) and every test constructor (Tasks 2, 4). Fixture field names cross-checked against `rustline-core/src/context.rs` shapes and the IP selectors in `net.rs`. ✅
