# rustline `cpu` + `memory` widgets (+ a shared gauge bar) — design

**Status:** approved (brainstorm, 2026-07-21)
**Depends on:** the v1 core (`docs/superpowers/specs/2026-07-20-rustline-tmux-statusline-design.md`)
and the platform-read pattern from the battery widget
(`docs/superpowers/specs/2026-07-21-rustline-battery-widget-design.md`)
**Scope:** Add two built-in widgets, `cpu` and `memory`, to the **default** right
region. Each is backed by a platform-specific host read (Linux `/proc`, macOS
`top`/`sysctl`+`vm_stat`) captured at the `Context`-build edge, and each exposes
its values through a format string of placeholders — including a shared
Unicode **gauge bar** — so a user includes/excludes exactly what they want.

## 1. Purpose & success criteria

rustline should show live CPU utilization and memory pressure next to the load
average. Both are classic status-line metrics; both are system reads that differ
per OS, so they reuse the battery widget's answer verbatim: the
**platform-specific read lives at the `Context`-build edge in the binary**,
captured into a typed `Context` field; the **widgets are pure** and read only
from `Context`. Each OS-specific surface is one small function with
`#[cfg(target_os = …)]` arms, each delegating to a **pure,
unconditionally-compiled parser** unit-tested on the Linux dev box.

Two facets are new relative to battery:

- **CPU is a *delta*, not a snapshot.** `/proc/stat` (and macOS's per-CPU tick
  counters) are cumulative since boot; a utilization percentage requires two
  readings separated in time. rustline is one-shot-per-refresh with no daemon,
  so `read_cpu()` takes **both** samples within the one invocation (a short
  sleep on Linux; a single `top -l 2` that samples internally on macOS). This is
  **stateless** — no on-disk snapshot, no ring buffer, no first-run-blank.
- **These are default widgets**, unlike battery. `cpu` and `memory` join the
  default right layout, so zero-config users see them. On an unsupported OS the
  `Context` field is `None` and the widget renders `down_format` (default `""` →
  nothing), exactly like battery — the bar degrades, never breaks.

**Success when:**

1. Adding `"cpu"` / `"memory"` to a layout (they are in the default right region)
   renders live values, e.g. `󰘚 37%` and `󰍛 6.2G/16G`.
2. Each value the user asked for is a **format-string placeholder**, substituted
   verbatim into surrounding literal text:
   - `cpu`: `{percent}` (busy %, integer), `{bar}` (current-usage gauge),
     `{icon}`. Default `format = "{icon} {percent}%"`.
   - `memory`: `{used}` (current usage), `{total}` (installed / "max"),
     `{avail}` (available), `{percent}` (used %, integer), `{bar}`
     (used/total gauge), `{icon}`. Default `format = "{icon} {used}/{total}"`.
3. `{bar}` renders a fixed-width Unicode block-eighths gauge (default width `8`,
   `bar_width` per-widget option): filled cells `█` with a sub-cell partial
   (`▏‥▉`) at the boundary over a `░` track — a pure `gauge_bar(fraction, width)`
   shared by both widgets and unit-tested at its boundaries.
4. `{used}`/`{total}`/`{avail}` render human-readable binary sizes via a pure
   `format_bytes(u64)` (e.g. `6.2G`, `512M`, `16G`), unit-tested.
5. On Linux, CPU comes from two `/proc/stat` reads ~120 ms apart and memory from
   `/proc/meminfo`; on macOS, CPU from `top -l 2 -n 0` and memory from
   `sysctl -n hw.memsize` + `vm_stat`. The **parsers** for every format are pure
   and unit-tested on the (Linux) dev host; only the file-read / subprocess-spawn
   / sleep is the untestable `#[cfg]` edge.
6. On any OS other than Linux or macOS — or when a read fails — the field is
   `None` and the widget renders `down_format` (default `""` → skipped). No
   fabricated `0%`/`0B` ever appears (invariant #6).
7. Zero-config still works and now includes `cpu`+`memory`; a bad/partial
   `[widgets.cpu]` / `[widgets.memory]` never breaks the bar (invariant #3); no
   new dependency enters the graph (`std::fs`/`std::process`/`std::thread` only —
   `cargo tree -i openssl`/`-i native-tls` stay empty); `just test` stays
   hermetic; clippy/fmt clean.

**Non-goals (deferred):**

- **Historical sparkline** (the "last X seconds" graph) — its own follow-up spec.
  It requires cross-invocation sample persistence (a ring-buffer state file with
  gap/staleness handling), which this stateless v1 deliberately avoids. When it
  lands it adds a **new** placeholder (e.g. `{graph}`) to these same widgets.
- **Per-level bar color** (green→amber→red thresholds) — needs multi-segment /
  theme plumbing; the v1 bar is a single monochrome segment. Natural follow-up.
- **Configurable CPU sample window** — the Linux window is a fixed const in v1
  (see §4); a `sample_ms` knob is a trivial later addition if wanted.
- Per-core CPU breakdown; swap / cached / buffers memory breakdown; load-based
  CPU (that is what `loadavg` is for); Windows/BSD reads (the `#[cfg]` fallback
  yields `None`, so they build and degrade cleanly — a later arm can add them).

## 2. Architecture overview

```
crates/
  rustline-core/     pure. Gains:
                       - context.rs:  MemInfo + CpuUsage types; Context gains
                         `memory: Option<MemInfo>`, `cpu: Option<CpuUsage>`
                       - widgets/bar.rs:    gauge_bar(fraction, width) — shared,
                         pure (mirrors widgets/net.rs as shared widget logic)
                       - widgets/cpu.rs:    pure CpuWidget (reads ONLY Context.cpu)
                       - widgets/memory.rs: pure MemoryWidget + format_bytes()
                         (reads ONLY Context.memory)
                       - config.rs:   CpuOpts + MemoryOpts under WidgetOpts;
                         default right layout gains "cpu","memory"
                       - widgets/mod.rs: register "cpu" and "memory"
                     NO new dependency. NO I/O.
  rustline/          bin. Gains:
                       - cpu.rs:    read_cpu() (#[cfg] arms) + pure parsers
                         parse_proc_stat()/busy_percent()/parse_top_cpu()
                       - memory.rs: read_memory() (#[cfg] arms) + pure parsers
                         parse_meminfo()/parse_macos_memory()
                       - build_context.rs: call read_cpu()/read_memory()
                       - main.rs:   declare `mod cpu; mod memory;`
                     NO new dependency (/proc = std::fs; top/sysctl/vm_stat =
                     std::process; the CPU sample sleep = std::thread::sleep).
```

The **reads** live in the binary, keeping `rustline-core` I/O-free. The
**decision logic** — delta→percent, byte humanization, bar rendering, format
substitution — is pure logic in core, operating on `Context.cpu`/`Context.memory`,
fully unit-testable with no hardware and reusable verbatim by a future daemon.

This upholds **invariant #1** (widgets read only from `Context`, never the
environment mid-render): both snapshots are captured into `Context` at build
time — including the ~120 ms CPU sampling sleep, which happens **at the build
edge, not mid-render** — and the widgets never touch the OS.

**Why the parsers are their own pure functions.** `read_cpu`/`read_memory` are
`#[cfg]`-split, so on the Linux dev box the macOS arms never compile and their
logic would otherwise be untested. Each arm is a thin platform-gated reader (does
the I/O) plus an unconditionally-compiled pure parser (`&str` → `Option<…>`), so
`parse_top_cpu`/`parse_macos_memory` are compiled and tested on Linux even though
`read_cpu_macos`/`read_memory_macos` are not — the exact pattern battery
established.

## 3. Data model

New types in `rustline-core/src/context.rs` (alongside `Battery`/`NetIface`):

```rust
/// A memory snapshot captured at Context-build time. All values are bytes.
/// `used_bytes = total_bytes - available_bytes` (saturating). None when the
/// platform is unsupported or the read failed — never a fabricated 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
}

/// A CPU-utilization snapshot: the busy fraction measured over a short sampling
/// window, as a percentage clamped to 0.0..=100.0.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CpuUsage {
    pub percent: f32,
}
```

`Context` gains two fields:

```rust
pub memory: Option<MemInfo>,   // None: unsupported OS / read failed
pub cpu: Option<CpuUsage>,     // None: unsupported OS / read failed
```

Both are serde-serializable, so **invariant #2** holds; pinned by the round-trip
test. The fields are **additive** — existing WASM guests that hand-parse
`Context` JSON ignore unknown keys. `CpuUsage` derives `PartialEq` but not `Eq`
(it carries an `f32`); `MemInfo` derives `Eq` (all `u64`). `Context` already has
no `Eq`, so this changes nothing for it.

`memory`/`cpu` follow `loadavg`/`battery`: absence is represented, never faked
(invariant #6). Every construction site of `Context` (fixtures in `context.rs`,
`loadavg.rs`, `battery.rs`, `mod.rs`, the per-widget test fixtures, the render /
assemble tests, and `crates/rustline/tests/smoke.rs`, plus the real builder in
`build_context.rs`) must set the two new fields (`memory: None, cpu: None` in
fixtures).

## 4. Platform reads

### 4a. CPU (`rustline/src/cpu.rs`)

```rust
use std::time::Duration;
use rustline_core::CpuUsage;

/// Sampling window for the Linux two-read delta. Short enough to keep
/// `render right` snappy, long enough to be a stable reading. macOS uses
/// `top`'s own internal sample instead (≈1 s; see below).
const CPU_SAMPLE_WINDOW: Duration = Duration::from_millis(120);

pub fn read_cpu() -> Option<CpuUsage> {
    #[cfg(target_os = "linux")] { read_cpu_linux() }
    #[cfg(target_os = "macos")] { read_cpu_macos() }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))] { None }
}
```

**Linux** (`#[cfg(target_os = "linux")]`): read `/proc/stat`, parse the aggregate
`cpu ` line, `std::thread::sleep(CPU_SAMPLE_WINDOW)`, read it again, parse, and
compute `busy_percent(prev, cur)`. Any unreadable/unparseable read → `None`.

**macOS** (`#[cfg(target_os = "macos")]`): run `top -l 2 -n 0` (`-l 2` = two
samples so the second is a real delta over top's ~1 s interval; `-n 0` = list no
processes, for speed), capture stdout, hand it to `parse_top_cpu`. A spawn
failure / non-UTF-8 → `None`.

**Pure parsers (compiled on all targets, unit-tested):**

```rust
#[derive(Clone, Copy)]
pub(crate) struct CpuTimes { idle: u64, total: u64 }

/// Parse the aggregate `cpu ` line of /proc/stat into (idle+iowait, sum-of-all).
pub(crate) fn parse_proc_stat(s: &str) -> Option<CpuTimes>;

/// Busy % over the interval between two cumulative snapshots.
pub(crate) fn busy_percent(prev: CpuTimes, cur: CpuTimes) -> f32;

/// macOS: full `top -l 2 -n 0` stdout -> busy %.
pub(crate) fn parse_top_cpu(output: &str) -> Option<f32>;
```

- `parse_proc_stat`: take the first line whose first whitespace-delimited token is
  exactly `cpu` (the aggregate, **not** `cpu0`). The remaining tokens are
  `user nice system idle iowait irq softirq steal guest guest_nice` (a variable
  count across kernels — sum whatever numeric fields are present). `total` = the
  sum; `idle` = the 4th field (`idle`) + the 5th (`iowait`) when present. Missing
  `cpu ` line or unparseable fields → `None`.
- `busy_percent`: `dt = cur.total.saturating_sub(prev.total)`;
  `didle = cur.idle.saturating_sub(prev.idle)`; if `dt == 0` → `0.0`; else
  `((dt - didle) as f32 / dt as f32 * 100.0).clamp(0.0, 100.0)`. Counters going
  backwards (suspend/resume) saturate to `0`, never negative or `NaN`.
- `parse_top_cpu`: take the **last** line containing `CPU usage:`, read the number
  immediately preceding `% idle`, and return `(100.0 - idle).clamp(0.0, 100.0)`.
  No such line / no idle figure → `None`. Reference line the parser must handle
  (captured as a test fixture — two of them, only the last counts):
  ```
  CPU usage: 12.50% user, 6.25% sys, 81.25% idle
  ```

**Sampling trade-off (documented for reviewers).** The Linux path adds
`CPU_SAMPLE_WINDOW` (~120 ms) of sleep to the `render right` process; only that
region pays it, and tmux updates the status line asynchronously (it shows the
prior value until the new output arrives — no UI freeze). The macOS path costs
top's ~1 s internal sample for the same reason and with the same async behavior;
a user who dislikes the latency removes `cpu` from their layout. A stateless
double-sample was chosen over a persisted-snapshot delta because history is
deferred (so persistence buys nothing now) and because macOS's `top` is
inherently a double-sample — one mental model across both platforms.

### 4b. Memory (`rustline/src/memory.rs`)

```rust
use rustline_core::MemInfo;

pub fn read_memory() -> Option<MemInfo> {
    #[cfg(target_os = "linux")] { read_memory_linux() }
    #[cfg(target_os = "macos")] { read_memory_macos() }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))] { None }
}
```

**Linux** (`#[cfg(target_os = "linux")]`): read `/proc/meminfo`, hand it to
`parse_meminfo`. Unreadable → `None`.

**macOS** (`#[cfg(target_os = "macos")]`): `sysctl -n hw.memsize` (total bytes) +
`vm_stat` (page size + page counts), hand both strings to `parse_macos_memory`.
Spawn failure / non-UTF-8 → `None`.

**Pure parsers (compiled on all targets, unit-tested):**

```rust
/// Linux: /proc/meminfo contents -> MemInfo. Needs MemTotal + MemAvailable
/// (both kB); missing either -> None.
pub(crate) fn parse_meminfo(s: &str) -> Option<MemInfo>;

/// macOS: (`hw.memsize` stdout, `vm_stat` stdout) -> MemInfo.
pub(crate) fn parse_macos_memory(memsize: &str, vm_stat: &str) -> Option<MemInfo>;
```

- `parse_meminfo`: scan lines for `MemTotal:` and `MemAvailable:` (values in kB);
  `total_bytes = MemTotal_kB * 1024`, `available_bytes = MemAvailable_kB * 1024`,
  `used_bytes = total_bytes.saturating_sub(available_bytes)`. `MemAvailable` has
  existed since Linux 3.14 (2014); absence of either field → `None`. Sample
  fixture:
  ```
  MemTotal:       16077216 kB
  MemFree:         1048576 kB
  MemAvailable:    9800000 kB
  ...
  ```
- `parse_macos_memory`: `total_bytes` = the integer in `memsize`. From `vm_stat`,
  read the page size from the header (`… (page size of 16384 bytes)`) and the
  `Pages free`, `Pages inactive`, and `Pages speculative` counts;
  `available_bytes = (free + inactive + speculative) * page_size` (an
  approximation of "available" adequate for a status line);
  `used_bytes = total_bytes.saturating_sub(available_bytes)`. A missing total or
  page size → `None`. Sample fixture:
  ```
  Mach Virtual Memory Statistics: (page size of 16384 bytes)
  Pages free:                          123456.
  Pages active:                        654321.
  Pages inactive:                      222222.
  Pages speculative:                    11111.
  Pages wired down:                    333333.
  ...
  ```

Neither reader's I/O is testable off-platform, so each is exercised only by a
"never panics" smoke assertion; all format handling is in the pure parsers.

## 5. Shared gauge bar (`rustline-core/src/widgets/bar.rs`)

A new module holding the one function both widgets use (mirrors `widgets/net.rs`
as shared pure widget logic):

```rust
/// Render `fraction` (clamped 0.0..=1.0) as a `width`-cell horizontal bar using
/// Unicode block-eighths for sub-cell precision: full cells `█`, one partial
/// boundary cell (`▏▎▍▌▋▊▉`), the remainder a `░` track. `width == 0` -> "".
pub fn gauge_bar(fraction: f64, width: usize) -> String;
```

Algorithm: `eighths = (fraction.clamp(0.0, 1.0) * (width * 8) as f64).round() as usize`;
`full = eighths / 8`, `rem = eighths % 8`; emit `full`×`█`, then when `rem > 0`
the partial glyph for `rem` (`1→▏ … 7→▉`), then `width - full - (rem>0)` × `░`.

- `U+2588 █` full; partials `U+258F ▏`(1) `U+258E ▎`(2) `U+258D ▍`(3)
  `U+258C ▌`(4) `U+258B ▋`(5) `U+258A ▊`(6) `U+2589 ▉`(7); `U+2591 ░` track.
- Glyphs are standard Unicode block elements (no Nerd Font needed for the bar
  itself), already within the project's monospace-font rendering assumptions.

## 6. Widgets

### 6a. `memory` (`rustline-core/src/widgets/memory.rs`)

```rust
pub struct MemoryWidget {
    pub format: String,       // default "{icon} {used}/{total}"
    pub down_format: String,  // default ""
    pub bar_width: usize,     // default 8
}
```

`render(&Context) -> Vec<Segment>`:

1. `Some(m)` → one `Segment`; substitute `{used}`→`format_bytes(m.used_bytes)`,
   `{total}`→`format_bytes(m.total_bytes)`, `{avail}`→`format_bytes(m.available_bytes)`,
   `{percent}`→used-percent integer (`(used/total*100).round()`, `0` when
   `total==0`), `{bar}`→`gauge_bar(used/total, bar_width)` (fraction `0.0` when
   `total==0`), `{icon}`→the memory glyph.
2. `None` → empty `down_format` → `vec![]`; else one `Segment` of `down_format`
   with all five placeholders collapsed to empty (no stray token, no fake value).

**`format_bytes(bytes: u64) -> String`** — pure, unit-tested. Binary units
(1024): pick the largest of `B/K/M/G/T` where the scaled value ≥ 1; one decimal
when that value `< 10` (`6.2G`, `1.5G`), otherwise integer (`16G`, `512M`); the
`B` unit is always integer (`0B`).

**Icon:** a Nerd-Font RAM/memory glyph (nf-md-memory `󰍛` `U+F035B`). Exact
codepoint finalized in implementation from a Nerd Font chart; the icon is a
constant, so the test simply asserts `{icon}` substitutes to it.

### 6b. `cpu` (`rustline-core/src/widgets/cpu.rs`)

```rust
pub struct CpuWidget {
    pub format: String,       // default "{icon} {percent}%"
    pub down_format: String,  // default ""
    pub bar_width: usize,     // default 8
}
```

`render(&Context) -> Vec<Segment>`:

1. `Some(c)` → one `Segment`; substitute `{percent}`→`c.percent.round()` as an
   integer (already `0..=100`), `{bar}`→`gauge_bar(c.percent as f64 / 100.0,
   bar_width)`, `{icon}`→the CPU glyph.
2. `None` → empty `down_format` → `vec![]`; else `down_format` with
   `{percent}`/`{bar}`/`{icon}` collapsed to empty.

**Icon:** a Nerd-Font CPU/chip glyph (nf-md-chip `󰘚` `U+F061A`), same
finalize-in-impl note as memory.

Substitution in both widgets is plain literal-token replacement, like the IP and
battery widgets.

## 7. Config (`rustline-core/src/config.rs`)

Two new opt structs added to `WidgetOpts`; every field `#[serde(default)]`
(**invariant #3**, total load). The default right layout gains the two names.

```rust
fn default_cpu_format() -> String { "{icon} {percent}%".into() }
fn default_memory_format() -> String { "{icon} {used}/{total}".into() }
fn default_bar_width() -> usize { 8 }

pub struct CpuOpts {
    #[serde(default = "default_cpu_format")]  pub format: String,
    #[serde(default)]                          pub down_format: String,
    #[serde(default = "default_bar_width")]    pub bar_width: usize,
}
pub struct MemoryOpts {
    #[serde(default = "default_memory_format")] pub format: String,
    #[serde(default)]                            pub down_format: String,
    #[serde(default = "default_bar_width")]      pub bar_width: usize,
}

pub struct WidgetOpts {
    // ...existing datetime, cwd, lan_ip, tailscale_ip, battery...
    #[serde(default)] pub cpu: CpuOpts,
    #[serde(default)] pub memory: MemoryOpts,
}

// default_right() gains "cpu","memory" grouped just before loadavg:
fn default_right() -> Vec<String> {
    vec!["cwd".into(), "cpu".into(), "memory".into(),
         "loadavg".into(), "datetime".into()]
}
```

Example config:

```toml
[widgets.cpu]
format = "{icon} {bar} {percent}%"   # default "{icon} {percent}%"
down_format = ""
bar_width = 8

[widgets.memory]
format = "{icon} {used}/{total}"     # default; or "{icon} {bar} {percent}%"
down_format = ""
bar_width = 10
```

## 8. Wiring

- `rustline-core/src/widgets/mod.rs`: declare `mod bar; pub mod cpu; pub mod
  memory;`; `pub use` the two widget structs; `Registry::with_builtins` registers
  `"cpu"` and `"memory"`, their factories capturing `cfg.widgets.cpu` /
  `cfg.widgets.memory` (mirrors battery). Doc comment updates the built-in count
  (nine → eleven).
- `rustline/src/cpu.rs`, `rustline/src/memory.rs`: new modules (see §4), declared
  in `main.rs`.
- `rustline/src/build_context.rs`: `build_region_context` sets
  `cpu: crate::cpu::read_cpu()`, `memory: crate::memory::read_memory()`.
  `build_window_context` reuses it and so inherits them (windows won't normally
  show cpu/memory; a uniform `Context` is cheap and correct — but note the
  ~120 ms/~1 s CPU sample now also runs for `render window`; acceptable, and the
  default window path is built-ins with no cpu widget).
- No `Cargo.toml` change: `/proc` = `std::fs`; top/sysctl/vm_stat =
  `std::process::Command`; the sample sleep = `std::thread::sleep` — all std. The
  dependency graph, and the rustls/OpenSSL-free invariant, are unchanged.
  `Cargo.lock` needs no change.

## 9. Testing (TDD — load-bearing first)

Pure parsers (`rustline/src/{cpu,memory}.rs`, compiled & tested on Linux):

- `parse_proc_stat`: `"cpu  100 0 100 700 100 0 0 0 0 0\n..."` → `idle = 800`
  (`700+100`), `total = 1000` (sum of all ten fields); a leading `cpu0 …` line is
  ignored (aggregate only); no `cpu ` line → `None`.
- `busy_percent`: `prev{idle:800,total:1000}` + `cur{idle:1000,total:1400}` →
  `50.0` (`dt=400, didle=200`); `dt == 0` → `0.0`; backward counters
  (`cur < prev`) saturate to `0.0`, never `NaN`/negative.
- `parse_top_cpu`: two `CPU usage:` lines → the **last** one's `100 - idle`;
  no line → `None`.
- `parse_meminfo`: the §4b fixture → `total = 16077216*1024`,
  `available = 9800000*1024`, `used = total - available`; missing `MemAvailable`
  → `None`.
- `parse_macos_memory`: the §4b fixtures → `total` from memsize,
  `available = (free+inactive+speculative)*page_size`, `used` = the saturating
  difference; missing total/page-size → `None`.
- `read_cpu()` / `read_memory()` return without panicking (host-dependent value;
  assert only no-panic and, if `Some`, `cpu.percent <= 100.0` /
  `mem.used_bytes <= mem.total_bytes`).

Shared bar (`widgets/bar.rs`):

- `gauge_bar(0.0, 8)` → `"░░░░░░░░"`; `gauge_bar(1.0, 8)` → `"████████"`;
  `gauge_bar(0.5, 8)` → `"████░░░░"`; `gauge_bar(0.5, 4)` → `"██░░"`.
- Sub-cell: `gauge_bar(0.3125, 8)` → `"██▌░░░░░"` (`20` eighths → 2 full + `▌` +
  5 track); every output has exactly `width` display cells.
- Clamp: `gauge_bar(1.5, 4)` → `"████"`; `gauge_bar(-0.2, 4)` → `"░░░░"`;
  `gauge_bar(0.5, 0)` → `""`.

Byte formatting (`widgets/memory.rs`):

- `format_bytes(16*1024^3)` → `"16G"`; `≈6.2 GiB` → `"6.2G"`; `512*1024^2` →
  `"512M"`; `1536*1024^2` → `"1.5G"`; `0` → `"0B"`.

Widgets (`widgets/{cpu,memory}.rs`):

- `cpu`: `format` substitution of `{percent}`/`{bar}`/`{icon}` with surrounding
  literals preserved; `Some(CpuUsage{percent:37.4})` → `{percent}`→`"37"`,
  `{bar}` a width-8 gauge; `None` + empty `down_format` → `vec![]`; `None` +
  non-empty `down_format` → text with placeholders collapsed.
- `memory`: `Some(MemInfo{ … })` → `{used}`/`{total}`/`{avail}`/`{percent}`/`{bar}`
  substituted (percent = used/total rounded); `total == 0` guard → `{percent}`
  `"0"` and `{bar}` empty-fraction, no divide-by-zero; `None` branches as for cpu.

Config (`config.rs`):

- `[widgets.cpu]` / `[widgets.memory]` partial tables → defaults fill the rest;
  absent → the default formats and `bar_width == 8`.
- Total-load fallback: a malformed `[widgets.cpu]` (e.g. `bar_width = "wide"`)
  table → `Config::default`.
- `default_right()` now equals `["cwd","cpu","memory","loadavg","datetime"]`
  (updates the existing `default_layout_matches_spec` test).

Cross-cutting:

- `Context` serde round-trip includes `cpu`/`memory` (invariant #2).
- Smoke test (`crates/rustline/tests/smoke.rs`): `"cpu"`/`"memory"` resolve in the
  registry; a synthesized `Context` with known `Some(CpuUsage)`/`Some(MemInfo)`
  renders the expected default-format text; with `None` + default `down_format`
  the region omits them. Update any existing smoke assertion that pins the default
  right region's contents to include the two new widgets.

## 10. Invariants this feature depends on / touches

- **#1 (Context is the sole render input):** upheld — cpu/memory snapshots
  (including the CPU sampling sleep) are captured into `Context` at build time;
  the widgets never read the OS. Load-bearing test: the smoke test rendering from
  a synthesized `Context`.
- **#2 (serde-serializable Context/Segment):** upheld — `MemInfo`/`CpuUsage`
  derive and round-trip serde; pinned by the round-trip test.
- **#3 (Config::load is total):** upheld — `CpuOpts`/`MemoryOpts` fields are all
  `#[serde(default)]`; pinned by the malformed-table fallback test.
- **#5 (segments[0] leftmost):** the new default right order
  `[cwd, cpu, memory, loadavg, datetime]` is passed left-to-right unchanged.
- **#6 (Option semantics, no fake data; panicking widget → empty):** the
  unsupported-OS / failed-read case renders `down_format` (default empty →
  nothing), never a fabricated `0%`/`0B`; pinned by the `None`-branch widget
  tests. `read_cpu`/`read_memory` returning `None` off Linux/macOS keeps the
  binary building and degrading cleanly.
- **Platform-read pattern:** each `#[cfg(target_os)]` arm delegates to a pure,
  unconditionally-compiled parser unit-tested on the Linux dev box — extending
  the surface `read_battery` established (now three such surfaces: battery, cpu,
  memory).

## 11. Documentation

Update `CLAUDE.md`: module map (`widgets/bar.rs`, `widgets/cpu.rs`,
`widgets/memory.rs`, the `MemInfo`/`CpuUsage`/`cpu`/`memory` additions to
`context.rs`, the new `rustline/src/cpu.rs` + `memory.rs` and their calls from
`build_context.rs`), the built-in widget count (nine → eleven), the default
right layout (`cwd, cpu, memory, loadavg, datetime`), the "platform-specific
reads stay at the `Context`-build edge" note (battery **and now cpu/memory** are
the `#[cfg(target_os)]` surfaces), the Config section (the `[widgets.cpu]` /
`[widgets.memory]` tables, their placeholders, and `bar_width`), and the Roadmap
(mark cpu/memory done; add the deferred **history sparkline** as a future item).
Update `README.md`'s widget list and add the `[widgets.cpu]` / `[widgets.memory]`
config examples. Link this spec from the Design-docs list.
