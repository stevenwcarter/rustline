# rustline `cpu` + `memory` widgets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add two built-in widgets — `cpu` and `memory` — to the default right region, each backed by a platform-specific host read captured at the `Context`-build edge and rendered through format-string placeholders (including a shared Unicode gauge bar).

**Architecture:** `rustline-core` gains two pure `Context` fields (`cpu: Option<CpuUsage>`, `memory: Option<MemInfo>`), two pure widgets, and a shared `gauge_bar` helper. The `rustline` binary gains two platform readers (`cpu.rs`, `memory.rs`) that follow the existing `battery.rs` pattern: thin `#[cfg(target_os)]` I/O wrappers delegating to pure, unconditionally-compiled parsers that carry the unit tests. CPU utilization is a stateless double-sample delta (Linux: two `/proc/stat` reads ~120 ms apart; macOS: one `top -l 2` that samples internally).

**Tech Stack:** Rust edition 2024, serde, `std::fs` / `std::process` / `std::thread` only (no new dependency). Nerd-Font glyphs for the icons; standard Unicode block-elements for the bar.

## Global Constraints

- **Edition 2024** in every crate; `rustfmt.toml` is edition 2024. Keep all editions equal.
- **Must stay clippy-clean** (`cargo clippy --all-targets -- -D warnings`) and **rustfmt-clean** (`cargo fmt --all --check`). No pre-commit hook — run `cargo fmt --all` before each commit.
- **No new dependency.** `/proc` reads use `std::fs`; macOS reads use `std::process::Command`; the CPU sample sleep uses `std::thread::sleep`. `cargo tree -i openssl` / `-i native-tls` must stay empty. `Cargo.lock` needs no change.
- **Invariant #1:** widgets read only from `Context`; all OS reads (including the CPU sampling sleep) happen at the `Context`-build edge in the binary.
- **Invariant #2:** `Context` / `Segment` / new types stay serde-serializable (the WASM ABI).
- **Invariant #3:** `Config::load` is total — every new config field is `#[serde(default)]`; a malformed table falls back to defaults.
- **Invariant #6:** a `None` field renders nothing / `down_format`, never a fabricated `0%` / `0B`.
- **Platform-read pattern:** each `#[cfg(target_os)]` arm delegates to a pure, unconditionally-compiled parser (`#[cfg(any(target_os = …, test))]`) unit-tested on the Linux dev box.
- `just test` must stay **hermetic** (no wasm toolchain). Commit `Cargo.lock` only if it changes (it won't here).
- Spec: `docs/superpowers/specs/2026-07-21-rustline-cpu-memory-widgets-design.md`.

---

### Task 1: Shared gauge bar (`gauge_bar`)

A pure, fixed-width Unicode block-eighths meter used by both widgets. Self-contained — no `Context`, no other new code. Mirrors the private-`mod net;` shared-logic pattern.

**Files:**
- Create: `crates/rustline-core/src/widgets/bar.rs`
- Modify: `crates/rustline-core/src/widgets/mod.rs` (add `mod bar;` next to `mod net;`)
- Test: inline `#[cfg(test)] mod tests` in `bar.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub(crate) fn gauge_bar(fraction: f64, width: usize) -> String` in `crate::widgets::bar`.

- [ ] **Step 1: Write the failing test**

Create `crates/rustline-core/src/widgets/bar.rs` with only the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_full() {
        assert_eq!(gauge_bar(0.0, 8), "░░░░░░░░");
        assert_eq!(gauge_bar(1.0, 8), "████████");
    }

    #[test]
    fn half_at_various_widths() {
        assert_eq!(gauge_bar(0.5, 8), "████░░░░");
        assert_eq!(gauge_bar(0.5, 4), "██░░");
    }

    #[test]
    fn sub_cell_partial() {
        // 0.3125 * 64 = 20 eighths -> 2 full + 4/8 partial (▌) + 5 track
        assert_eq!(gauge_bar(0.3125, 8), "██▌░░░░░");
    }

    #[test]
    fn clamps_and_zero_width() {
        assert_eq!(gauge_bar(1.5, 4), "████");
        assert_eq!(gauge_bar(-0.2, 4), "░░░░");
        assert_eq!(gauge_bar(0.5, 0), "");
    }

    #[test]
    fn always_width_cells() {
        for f in [0.0, 0.1, 0.37, 0.5, 0.99, 1.0] {
            assert_eq!(gauge_bar(f, 8).chars().count(), 8, "f={f}");
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustline-core --lib widgets::bar`
Expected: FAIL to compile — `cannot find function 'gauge_bar'`.

- [ ] **Step 3: Write minimal implementation**

Prepend to `crates/rustline-core/src/widgets/bar.rs` (above the test module):

```rust
//! Shared pure rendering for the cpu/memory "gauge" bar: a fixed-width
//! horizontal meter drawn with Unicode block-eighths. No I/O; called by the
//! `cpu`/`memory` widgets. Stays private (`mod bar;`) with a `pub(crate)` helper.

/// Partial block-eighth glyphs indexed by remainder `1..=7` (index 0 unused).
const PARTIALS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];

/// Render `fraction` (clamped to `0.0..=1.0`) as a `width`-cell horizontal bar:
/// full cells `█`, one sub-cell partial (`▏`..`▉`) at the boundary, the rest a
/// `░` track. `width == 0` yields an empty string.
pub(crate) fn gauge_bar(fraction: f64, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let eighths = (fraction.clamp(0.0, 1.0) * (width * 8) as f64).round() as usize;
    let full = eighths / 8;
    let rem = eighths % 8;
    let mut out = String::with_capacity(width * 3);
    for _ in 0..full {
        out.push('█');
    }
    if rem > 0 {
        out.push_str(PARTIALS[rem]);
    }
    let track = width - full - usize::from(rem > 0);
    for _ in 0..track {
        out.push('░');
    }
    out
}
```

Add the module declaration to `crates/rustline-core/src/widgets/mod.rs` next to the existing `mod net;` line:

```rust
mod bar;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rustline-core --lib widgets::bar`
Expected: PASS (5 tests).

Then `cargo clippy -p rustline-core --all-targets -- -D warnings` and `cargo fmt --all`.

- [ ] **Step 5: Commit**

```bash
git add crates/rustline-core/src/widgets/bar.rs crates/rustline-core/src/widgets/mod.rs
git commit -m "feat(core): shared gauge_bar meter for cpu/memory widgets"
```

---

### Task 2: `Context` data model (`MemInfo`, `CpuUsage`, new fields)

Add the two typed snapshots and the two `Option` fields, then update **every** `Context` construction site in the workspace so it compiles. Absence is `None`, never faked.

**Files:**
- Modify: `crates/rustline-core/src/context.rs` (types, fields, sample fixture, round-trip test)
- Modify (add `memory: None, cpu: None` to each `Context { … }` literal):
  - `crates/rustline-core/src/widget.rs`
  - `crates/rustline-core/src/assemble.rs`
  - `crates/rustline-core/src/widgets/mod.rs`
  - `crates/rustline-core/src/widgets/pane_id.rs`
  - `crates/rustline-core/src/widgets/hostname.rs`
  - `crates/rustline-core/src/widgets/cwd.rs`
  - `crates/rustline-core/src/widgets/datetime.rs`
  - `crates/rustline-core/src/widgets/loadavg.rs`
  - `crates/rustline-core/src/widgets/windows.rs`
  - `crates/rustline-core/src/widgets/lan_ip.rs`
  - `crates/rustline-core/src/widgets/tailscale_ip.rs`
  - `crates/rustline-core/src/widgets/battery.rs`
  - `crates/rustline/src/build_context.rs` (the real builder — set both to `None` here; Tasks 6 & 7 flip them to real reads)
  - `crates/rustline-wasm/tests/e2e.rs` (behind the `wasm-e2e` feature)
- Test: round-trip assertions in `context.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub struct MemInfo { pub total_bytes: u64, pub used_bytes: u64, pub available_bytes: u64 }`; `pub struct CpuUsage { pub percent: f32 }`; `Context.memory: Option<MemInfo>`; `Context.cpu: Option<CpuUsage>`.

- [ ] **Step 1: Write the failing test**

In `crates/rustline-core/src/context.rs`, add a test to the `tests` module (references the not-yet-existing fields, so it fails to compile — the red state):

```rust
#[test]
fn context_cpu_memory_survive_serde() {
    let mut ctx = sample();
    ctx.cpu = Some(CpuUsage { percent: 37.5 });
    ctx.memory = Some(MemInfo {
        total_bytes: 16 * 1024 * 1024 * 1024,
        used_bytes: 6 * 1024 * 1024 * 1024,
        available_bytes: 10 * 1024 * 1024 * 1024,
    });
    let json = serde_json::to_string(&ctx).unwrap();
    let back: Context = serde_json::from_str(&json).unwrap();
    assert_eq!(back.cpu, ctx.cpu);
    assert_eq!(back.memory, ctx.memory);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo build -p rustline-core`
Expected: FAIL — `no field 'cpu' on type '&Context'` (and `MemInfo`/`CpuUsage` unresolved).

- [ ] **Step 3: Add the types and fields**

In `crates/rustline-core/src/context.rs`, add the two types near `Battery` (after the `Battery` struct):

```rust
/// A memory snapshot captured at Context-build time. All values are bytes;
/// `used_bytes = total_bytes - available_bytes` (saturating). `Context::memory`
/// is `None` on unsupported platforms or when the read failed — never a
/// fabricated `0` (invariant #6), mirroring `loadavg`/`battery`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
}

/// A CPU-utilization snapshot: the busy fraction measured over a short sampling
/// window at Context-build time, as a percentage clamped to `0.0..=100.0`.
/// `Context::cpu` is `None` on unsupported platforms or when the read failed.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CpuUsage {
    pub percent: f32,
}
```

Add the two fields to `Context` (after the `battery` field, before `os`):

```rust
    /// CPU-utilization snapshot read once at build time; `None` when
    /// absent/unsupported.
    pub cpu: Option<CpuUsage>,
    /// Memory snapshot read once at build time; `None` when absent/unsupported.
    pub memory: Option<MemInfo>,
```

Update the `sample()` fixture in the same file's `tests` module to set them (place after `battery: Some(...)`):

```rust
            cpu: Some(CpuUsage { percent: 12.5 }),
            memory: Some(MemInfo {
                total_bytes: 16 * 1024 * 1024 * 1024,
                used_bytes: 6 * 1024 * 1024 * 1024,
                available_bytes: 10 * 1024 * 1024 * 1024,
            }),
```

- [ ] **Step 4: Update every other construction site**

In each file listed under **Files** above (all except `context.rs`), find each `Context { … }` literal and add these two lines among its fields (next to `battery:`):

```rust
            cpu: None,
            memory: None,
```

Then in `crates/rustline/src/build_context.rs`, `build_region_context` returns a `Context { … }` — add the same two `None` lines there (Tasks 6 & 7 replace them with real reads).

Find them all and confirm none are missed:

Run: `grep -rn 'loadavg:' crates/ | wc -l` (expect 17 sites) — every one must now also have `cpu:` and `memory:`. Verify with: `grep -rLn 'cpu:' $(grep -rl 'loadavg:' crates/)` (should print nothing).

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo build --workspace` then `cargo test -p rustline-core --lib context`
Expected: PASS, including `context_cpu_memory_survive_serde`.

Run: `cargo test --workspace` (all existing tests still green after the fixture sweep).
Then `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all`.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(core): add cpu/memory snapshots to Context"
```

---

### Task 3: `memory` widget + `format_bytes`

Pure widget over `Context.memory`, plus the human-size formatter it uses.

**Files:**
- Create: `crates/rustline-core/src/widgets/memory.rs`
- Modify: `crates/rustline-core/src/widgets/mod.rs` (add `pub mod memory;` and `pub use memory::MemoryWidget;`)
- Test: inline `#[cfg(test)] mod tests` in `memory.rs`

**Interfaces:**
- Consumes: `bar::gauge_bar` (Task 1); `MemInfo`, `Context` (Task 2).
- Produces: `pub struct MemoryWidget { pub format: String, pub down_format: String, pub bar_width: usize }` implementing `Widget`.

- [ ] **Step 1: Write the failing test**

Create `crates/rustline-core/src/widgets/memory.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, MemInfo, Widget};
    use chrono::{Local, TimeZone};

    fn ctx(memory: Option<MemInfo>) -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/".into(),
            home: "/h".into(),
            hostname: "h".into(),
            loadavg: None,
            now: Local.with_ymd_and_hms(2026, 7, 21, 12, 0, 0).single().unwrap(),
            window: None,
            interfaces: Vec::new(),
            battery: None,
            cpu: None,
            memory,
            os: String::new(),
            arch: String::new(),
        }
    }

    fn mem(total: u64, used: u64, avail: u64) -> Option<MemInfo> {
        Some(MemInfo { total_bytes: total, used_bytes: used, available_bytes: avail })
    }

    fn w(format: &str, down: &str) -> MemoryWidget {
        MemoryWidget { format: format.into(), down_format: down.into(), bar_width: 8 }
    }

    #[test]
    fn format_bytes_humanizes() {
        assert_eq!(format_bytes(16 * 1024u64.pow(3)), "16G");
        assert_eq!(format_bytes((6.2 * 1024f64.powi(3)) as u64), "6.2G");
        assert_eq!(format_bytes(512 * 1024u64.pow(2)), "512M");
        assert_eq!(format_bytes(1536 * 1024u64.pow(2)), "1.5G");
        assert_eq!(format_bytes(0), "0B");
    }

    #[test]
    fn renders_used_total_percent() {
        let g = 1024u64.pow(3);
        let out = w("{used}/{total} {percent}%", "").render(&ctx(mem(16 * g, 6 * g, 10 * g)));
        assert_eq!(out[0].text, "6.0G/16G 38%"); // 6/16 = 37.5 -> 38
    }

    #[test]
    fn renders_bar_and_icon() {
        let g = 1024u64.pow(3);
        let out = w("{icon} {bar}", "").render(&ctx(mem(16 * g, 8 * g, 8 * g)));
        // 8/16 = 0.5 over width 8 -> "████░░░░", icon prefixed
        assert_eq!(out[0].text, "\u{f035b} ████░░░░");
    }

    #[test]
    fn zero_total_does_not_divide_by_zero() {
        let out = w("{percent}% {bar}", "").render(&ctx(mem(0, 0, 0)));
        assert_eq!(out[0].text, "0% ░░░░░░░░");
    }

    #[test]
    fn none_empty_down_skips() {
        assert!(w("{used}", "").render(&ctx(None)).is_empty());
    }

    #[test]
    fn none_down_format_collapses_placeholders() {
        let out = w("{used}", "n/a {used}{total}{bar}{percent}{icon}").render(&ctx(None));
        assert_eq!(out[0].text, "n/a ");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustline-core --lib widgets::memory`
Expected: FAIL to compile — `MemoryWidget` / `format_bytes` not found.

- [ ] **Step 3: Write minimal implementation**

Prepend to `crates/rustline-core/src/widgets/memory.rs` (above the test module):

```rust
use crate::widgets::bar;
use crate::{Context, Segment, Widget};

/// Nerd-Font memory/RAM glyph (nf-md-memory 󰍛).
const MEMORY_ICON: &str = "\u{f035b}";

/// Renders memory usage from `Context::memory`. Pure — reads only that field.
pub struct MemoryWidget {
    pub format: String,
    pub down_format: String,
    pub bar_width: usize,
}

/// Human-readable binary size (1024-based): the largest of `B/K/M/G/T` where the
/// scaled value is `>= 1`, one decimal below 10 and none at/above 10 (bytes are
/// always integer). E.g. `6.2 GiB -> "6.2G"`, `512 MiB -> "512M"`, `0 -> "0B"`.
fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes}B")
    } else if value < 10.0 {
        format!("{value:.1}{}", UNITS[unit])
    } else {
        format!("{value:.0}{}", UNITS[unit])
    }
}

impl Widget for MemoryWidget {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        match ctx.memory {
            Some(m) => {
                let fraction = if m.total_bytes == 0 {
                    0.0
                } else {
                    m.used_bytes as f64 / m.total_bytes as f64
                };
                let percent = (fraction * 100.0).round() as u64;
                let text = self
                    .format
                    .replace("{used}", &format_bytes(m.used_bytes))
                    .replace("{total}", &format_bytes(m.total_bytes))
                    .replace("{avail}", &format_bytes(m.available_bytes))
                    .replace("{percent}", &percent.to_string())
                    .replace("{bar}", &bar::gauge_bar(fraction, self.bar_width))
                    .replace("{icon}", MEMORY_ICON);
                vec![Segment::new(text)]
            }
            None => {
                if self.down_format.is_empty() {
                    return vec![];
                }
                let text = self
                    .down_format
                    .replace("{used}", "")
                    .replace("{total}", "")
                    .replace("{avail}", "")
                    .replace("{percent}", "")
                    .replace("{bar}", "")
                    .replace("{icon}", "");
                vec![Segment::new(text)]
            }
        }
    }
}
```

Add to `crates/rustline-core/src/widgets/mod.rs` (module list and re-exports):

```rust
pub mod memory;
```
```rust
pub use memory::MemoryWidget;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rustline-core --lib widgets::memory`
Expected: PASS (6 tests).
Then `cargo clippy -p rustline-core --all-targets -- -D warnings` and `cargo fmt --all`.

- [ ] **Step 5: Commit**

```bash
git add crates/rustline-core/src/widgets/memory.rs crates/rustline-core/src/widgets/mod.rs
git commit -m "feat(core): memory widget + format_bytes"
```

---

### Task 4: `cpu` widget

Pure widget over `Context.cpu`.

**Files:**
- Create: `crates/rustline-core/src/widgets/cpu.rs`
- Modify: `crates/rustline-core/src/widgets/mod.rs` (add `pub mod cpu;` and `pub use cpu::CpuWidget;`)
- Test: inline `#[cfg(test)] mod tests` in `cpu.rs`

**Interfaces:**
- Consumes: `bar::gauge_bar` (Task 1); `CpuUsage`, `Context` (Task 2).
- Produces: `pub struct CpuWidget { pub format: String, pub down_format: String, pub bar_width: usize }` implementing `Widget`.

- [ ] **Step 1: Write the failing test**

Create `crates/rustline-core/src/widgets/cpu.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, CpuUsage, Widget};
    use chrono::{Local, TimeZone};

    fn ctx(cpu: Option<CpuUsage>) -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/".into(),
            home: "/h".into(),
            hostname: "h".into(),
            loadavg: None,
            now: Local.with_ymd_and_hms(2026, 7, 21, 12, 0, 0).single().unwrap(),
            window: None,
            interfaces: Vec::new(),
            battery: None,
            cpu,
            memory: None,
            os: String::new(),
            arch: String::new(),
        }
    }

    fn w(format: &str, down: &str) -> CpuWidget {
        CpuWidget { format: format.into(), down_format: down.into(), bar_width: 8 }
    }

    #[test]
    fn renders_percent_rounded() {
        let out = w("{percent}%", "").render(&ctx(Some(CpuUsage { percent: 37.4 })));
        assert_eq!(out[0].text, "37%");
        let out = w("{percent}%", "").render(&ctx(Some(CpuUsage { percent: 37.6 })));
        assert_eq!(out[0].text, "38%");
    }

    #[test]
    fn renders_bar_and_icon() {
        // 50% over width 8 -> "████░░░░"
        let out = w("{icon} {bar}", "").render(&ctx(Some(CpuUsage { percent: 50.0 })));
        assert_eq!(out[0].text, "\u{f061a} ████░░░░");
    }

    #[test]
    fn none_empty_down_skips() {
        assert!(w("{percent}%", "").render(&ctx(None)).is_empty());
    }

    #[test]
    fn none_down_format_collapses_placeholders() {
        let out = w("{percent}%", "cpu? {percent}{bar}{icon}").render(&ctx(None));
        assert_eq!(out[0].text, "cpu? ");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustline-core --lib widgets::cpu`
Expected: FAIL to compile — `CpuWidget` not found.

- [ ] **Step 3: Write minimal implementation**

Prepend to `crates/rustline-core/src/widgets/cpu.rs` (above the test module):

```rust
use crate::widgets::bar;
use crate::{Context, Segment, Widget};

/// Nerd-Font CPU/chip glyph (nf-md-chip 󰘚).
const CPU_ICON: &str = "\u{f061a}";

/// Renders CPU utilization from `Context::cpu`. Pure — reads only that field.
pub struct CpuWidget {
    pub format: String,
    pub down_format: String,
    pub bar_width: usize,
}

impl Widget for CpuWidget {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        match ctx.cpu {
            Some(c) => {
                let percent = c.percent.round() as u64;
                let text = self
                    .format
                    .replace("{percent}", &percent.to_string())
                    .replace("{bar}", &bar::gauge_bar(c.percent as f64 / 100.0, self.bar_width))
                    .replace("{icon}", CPU_ICON);
                vec![Segment::new(text)]
            }
            None => {
                if self.down_format.is_empty() {
                    return vec![];
                }
                let text = self
                    .down_format
                    .replace("{percent}", "")
                    .replace("{bar}", "")
                    .replace("{icon}", "");
                vec![Segment::new(text)]
            }
        }
    }
}
```

Add to `crates/rustline-core/src/widgets/mod.rs`:

```rust
pub mod cpu;
```
```rust
pub use cpu::CpuWidget;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rustline-core --lib widgets::cpu`
Expected: PASS (4 tests).
Then `cargo clippy -p rustline-core --all-targets -- -D warnings` and `cargo fmt --all`.

- [ ] **Step 5: Commit**

```bash
git add crates/rustline-core/src/widgets/cpu.rs crates/rustline-core/src/widgets/mod.rs
git commit -m "feat(core): cpu widget"
```

---

### Task 5: Config options, default layout, registry wiring

Add `CpuOpts` / `MemoryOpts`, place `cpu`/`memory` in the default right layout, and register both widget factories.

**Files:**
- Modify: `crates/rustline-core/src/config.rs` (opt structs, `WidgetOpts` fields, `default_right`, tests)
- Modify: `crates/rustline-core/src/widgets/mod.rs` (`with_builtins` registers `"cpu"`/`"memory"`; update the built-in-count doc comment nine → eleven)
- Test: config tests in `config.rs`; a registry-resolution test in `widgets/mod.rs`

**Interfaces:**
- Consumes: `MemoryWidget` (Task 3), `CpuWidget` (Task 4), `MemInfo`/`CpuUsage` (Task 2).
- Produces: `WidgetOpts.cpu: CpuOpts`, `WidgetOpts.memory: MemoryOpts`; the names `"cpu"`/`"memory"` resolvable in `Registry::with_builtins`; `default_right() == ["cwd","cpu","memory","loadavg","datetime"]`.

- [ ] **Step 1: Write the failing tests**

In `crates/rustline-core/src/config.rs` `tests` module, add:

```rust
#[test]
fn cpu_memory_opts_parse_with_defaults() {
    let toml = r#"
[widgets.cpu]
format = "{bar} {percent}%"
[widgets.memory]
bar_width = 12
"#;
    let c: Config = toml::from_str(toml).unwrap();
    assert_eq!(c.widgets.cpu.format, "{bar} {percent}%");
    assert_eq!(c.widgets.cpu.bar_width, 8); // omitted -> default
    assert_eq!(c.widgets.memory.format, "{icon} {used}/{total}"); // omitted -> default
    assert_eq!(c.widgets.memory.bar_width, 12);
    assert_eq!(c.widgets.memory.down_format, "");
}

#[test]
fn cpu_memory_opts_default_when_absent() {
    let c = Config::default();
    assert_eq!(c.widgets.cpu.format, "{icon} {percent}%");
    assert_eq!(c.widgets.cpu.bar_width, 8);
    assert_eq!(c.widgets.memory.format, "{icon} {used}/{total}");
    assert_eq!(c.widgets.memory.bar_width, 8);
}

#[test]
fn malformed_cpu_table_falls_back_to_default() {
    let dir = std::env::temp_dir().join("rustline_test_badcpu");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("config.toml");
    // bar_width must be an integer; a string makes the table invalid.
    std::fs::write(&p, "[widgets.cpu]\nbar_width = \"wide\"\n").unwrap();
    let c = Config::load(&p);
    assert_eq!(c.widgets.cpu.bar_width, 8);
    assert_eq!(c.layout.left, Config::default().layout.left);
}
```

Update the existing `default_layout_matches_spec` test's right assertion:

```rust
    assert_eq!(
        c.layout.right,
        vec!["cwd", "cpu", "memory", "loadavg", "datetime"]
    );
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rustline-core --lib config`
Expected: FAIL — `no field 'cpu' on WidgetOpts` and the default-layout assertion mismatch.

- [ ] **Step 3: Write minimal implementation**

In `crates/rustline-core/src/config.rs`, add default helpers and the two opt structs (place near `BatteryOpts`):

```rust
/// Default `format` for the `cpu` widget.
fn default_cpu_format() -> String {
    "{icon} {percent}%".into()
}

/// Default `format` for the `memory` widget.
fn default_memory_format() -> String {
    "{icon} {used}/{total}".into()
}

/// Default width (cells) of the `{bar}` gauge for cpu/memory.
fn default_bar_width() -> usize {
    8
}

/// Options for the `cpu` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CpuOpts {
    #[serde(default = "default_cpu_format")]
    pub format: String,
    #[serde(default)]
    pub down_format: String,
    #[serde(default = "default_bar_width")]
    pub bar_width: usize,
}

impl Default for CpuOpts {
    fn default() -> Self {
        Self {
            format: default_cpu_format(),
            down_format: String::new(),
            bar_width: default_bar_width(),
        }
    }
}

/// Options for the `memory` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryOpts {
    #[serde(default = "default_memory_format")]
    pub format: String,
    #[serde(default)]
    pub down_format: String,
    #[serde(default = "default_bar_width")]
    pub bar_width: usize,
}

impl Default for MemoryOpts {
    fn default() -> Self {
        Self {
            format: default_memory_format(),
            down_format: String::new(),
            bar_width: default_bar_width(),
        }
    }
}
```

Add the fields to `WidgetOpts` (after `battery`):

```rust
    #[serde(default)]
    pub cpu: CpuOpts,
    #[serde(default)]
    pub memory: MemoryOpts,
```

Update `default_right`:

```rust
fn default_right() -> Vec<String> {
    vec![
        "cwd".into(),
        "cpu".into(),
        "memory".into(),
        "loadavg".into(),
        "datetime".into(),
    ]
}
```

- [ ] **Step 4: Register the widgets**

In `crates/rustline-core/src/widgets/mod.rs`, inside `Registry::with_builtins`, add (after the `battery` registration):

```rust
        let cpu = cfg.widgets.cpu.clone();
        registry.register(
            "cpu",
            Box::new(move || {
                Box::new(CpuWidget {
                    format: cpu.format.clone(),
                    down_format: cpu.down_format.clone(),
                    bar_width: cpu.bar_width,
                })
            }),
        );

        let memory = cfg.widgets.memory.clone();
        registry.register(
            "memory",
            Box::new(move || {
                Box::new(MemoryWidget {
                    format: memory.format.clone(),
                    down_format: memory.down_format.clone(),
                    bar_width: memory.bar_width,
                })
            }),
        );
```

Update the `with_builtins` doc comment: "all nine built-in widgets" → "all eleven built-in widgets".

Add a registry-resolution test to the `tests` module in `widgets/mod.rs`:

```rust
#[test]
fn cpu_memory_registered_and_render_from_context() {
    use crate::{CpuUsage, MemInfo};
    let cfg = Config::default();
    let reg = Registry::with_builtins(&cfg);
    assert!(reg.contains("cpu") && reg.contains("memory"));

    let mut c = ctx(Vec::new());
    c.cpu = Some(CpuUsage { percent: 50.0 });
    let g = 1024u64.pow(3);
    c.memory = Some(MemInfo { total_bytes: 16 * g, used_bytes: 8 * g, available_bytes: 8 * g });
    let texts: Vec<String> = reg
        .resolve(&["cpu".into(), "memory".into()])
        .iter()
        .flat_map(|w| w.render(&c))
        .map(|s| s.text)
        .collect();
    // cpu default "{icon} {percent}%" and memory default "{icon} {used}/{total}"
    assert_eq!(texts, vec!["\u{f061a} 50%".to_string(), "\u{f035b} 8.0G/16G".to_string()]);
}
```

(The `ctx(Vec::new())` helper already exists in this test module from the IP/battery tests; it now sets `cpu: None, memory: None` after Task 2.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rustline-core --lib config` then `cargo test -p rustline-core --lib widgets`
Expected: PASS, including the new config, default-layout, and registry tests.
Then `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all`.

- [ ] **Step 6: Commit**

```bash
git add crates/rustline-core/src/config.rs crates/rustline-core/src/widgets/mod.rs
git commit -m "feat(core): cpu/memory config opts, default layout, registry"
```

---

### Task 6: Memory platform read (`rustline/src/memory.rs`)

The `#[cfg(target_os)]` reader + pure parsers, wired into `build_context`.

**Files:**
- Create: `crates/rustline/src/memory.rs`
- Modify: `crates/rustline/src/main.rs` (add `mod memory;` next to `mod battery;`)
- Modify: `crates/rustline/src/build_context.rs` (`build_region_context`: `memory: crate::memory::read_memory()`)
- Test: inline `#[cfg(test)] mod tests` in `memory.rs`

**Interfaces:**
- Consumes: `MemInfo` (Task 2).
- Produces: `pub fn read_memory() -> Option<MemInfo>`.

- [ ] **Step 1: Write the failing test**

Create `crates/rustline/src/memory.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_meminfo_parses_total_avail_used() {
        let text = "MemTotal:       16077216 kB\n\
                    MemFree:         1048576 kB\n\
                    MemAvailable:    9800000 kB\n\
                    Buffers:          200000 kB\n";
        let m = parse_meminfo(text).unwrap();
        assert_eq!(m.total_bytes, 16_077_216 * 1024);
        assert_eq!(m.available_bytes, 9_800_000 * 1024);
        assert_eq!(m.used_bytes, (16_077_216 - 9_800_000) * 1024);
    }

    #[test]
    fn linux_meminfo_missing_available_is_none() {
        assert!(parse_meminfo("MemTotal: 100 kB\n").is_none());
    }

    #[test]
    fn macos_memory_parses_from_sysctl_and_vm_stat() {
        let memsize = "17179869184\n";
        let vm = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
                  Pages free:                          100000.\n\
                  Pages active:                        200000.\n\
                  Pages inactive:                       50000.\n\
                  Pages speculative:                    10000.\n\
                  Pages wired down:                     80000.\n";
        let m = parse_macos_memory(memsize, vm).unwrap();
        assert_eq!(m.total_bytes, 17_179_869_184);
        assert_eq!(m.available_bytes, (100_000 + 50_000 + 10_000) * 16384);
        assert_eq!(m.used_bytes, 17_179_869_184 - (100_000 + 50_000 + 10_000) * 16384);
    }

    #[test]
    fn macos_memory_missing_total_is_none() {
        assert!(parse_macos_memory("nope", "(page size of 4096 bytes)\n").is_none());
    }

    #[test]
    fn read_memory_never_panics() {
        if let Some(m) = read_memory() {
            assert!(m.used_bytes <= m.total_bytes);
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustline memory`
Expected: FAIL to compile — `read_memory` / `parse_meminfo` / `parse_macos_memory` not found.

- [ ] **Step 3: Write minimal implementation**

Prepend to `crates/rustline/src/memory.rs`:

```rust
//! Platform-specific memory read, isolated at the `Context`-build edge.
//!
//! Mirrors `battery.rs`: the `#[cfg(target_os)]` readers do the I/O; the pure
//! parsers compile under `test` on any host, so both are unit-tested on the
//! Linux dev box even though only one reader arm compiles per platform.

use rustline_core::MemInfo;

/// Read host memory, or `None` if the platform is unsupported or the read
/// failed. Called once at Context-build time.
pub fn read_memory() -> Option<MemInfo> {
    #[cfg(target_os = "linux")]
    {
        read_memory_linux()
    }
    #[cfg(target_os = "macos")]
    {
        read_memory_macos()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
fn read_memory_linux() -> Option<MemInfo> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_meminfo(&text)
}

#[cfg(target_os = "macos")]
fn read_memory_macos() -> Option<MemInfo> {
    let memsize = std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    let vm = std::process::Command::new("vm_stat").output().ok()?;
    let memsize = String::from_utf8(memsize.stdout).ok()?;
    let vm = String::from_utf8(vm.stdout).ok()?;
    parse_macos_memory(&memsize, &vm)
}

/// Parse `/proc/meminfo`. Needs `MemTotal` + `MemAvailable` (both kB);
/// missing either → `None`. `MemAvailable` has existed since Linux 3.14.
#[cfg(any(target_os = "linux", test))]
fn parse_meminfo(text: &str) -> Option<MemInfo> {
    fn field_kb(text: &str, key: &str) -> Option<u64> {
        let rest = text.lines().find_map(|l| l.strip_prefix(key))?;
        rest.split_whitespace().next()?.parse::<u64>().ok()
    }
    let total_bytes = field_kb(text, "MemTotal:")?.saturating_mul(1024);
    let available_bytes = field_kb(text, "MemAvailable:")?.saturating_mul(1024);
    Some(MemInfo {
        total_bytes,
        used_bytes: total_bytes.saturating_sub(available_bytes),
        available_bytes,
    })
}

/// Parse (`hw.memsize` stdout, `vm_stat` stdout). `available ≈ (free + inactive
/// + speculative) * page_size`; `used = total - available`. Missing total or
/// page size → `None`.
#[cfg(any(target_os = "macos", test))]
fn parse_macos_memory(memsize: &str, vm_stat: &str) -> Option<MemInfo> {
    let total_bytes = memsize.trim().parse::<u64>().ok()?;
    let page_size = vm_stat
        .lines()
        .next()?
        .split("page size of")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    fn pages(vm_stat: &str, key: &str) -> u64 {
        vm_stat
            .lines()
            .find_map(|l| l.trim().strip_prefix(key))
            .and_then(|rest| rest.trim().trim_end_matches('.').parse::<u64>().ok())
            .unwrap_or(0)
    }
    let free = pages(vm_stat, "Pages free:");
    let inactive = pages(vm_stat, "Pages inactive:");
    let speculative = pages(vm_stat, "Pages speculative:");
    let available_bytes = (free + inactive + speculative).saturating_mul(page_size);
    Some(MemInfo {
        total_bytes,
        used_bytes: total_bytes.saturating_sub(available_bytes),
        available_bytes,
    })
}
```

Add `mod memory;` to `crates/rustline/src/main.rs` (next to `mod battery;`).

In `crates/rustline/src/build_context.rs`, change the `memory: None,` line in `build_region_context` to:

```rust
        memory: crate::memory::read_memory(),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rustline memory`
Expected: PASS (5 tests).
Then `cargo clippy -p rustline --all-targets -- -D warnings` and `cargo fmt --all`.

- [ ] **Step 5: Commit**

```bash
git add crates/rustline/src/memory.rs crates/rustline/src/main.rs crates/rustline/src/build_context.rs
git commit -m "feat(rustline): platform memory read wired into Context"
```

---

### Task 7: CPU platform read (`rustline/src/cpu.rs`)

The `#[cfg(target_os)]` reader (stateless double-sample) + pure parsers, wired into `build_context`.

**Files:**
- Create: `crates/rustline/src/cpu.rs`
- Modify: `crates/rustline/src/main.rs` (add `mod cpu;`)
- Modify: `crates/rustline/src/build_context.rs` (`build_region_context`: `cpu: crate::cpu::read_cpu()`)
- Test: inline `#[cfg(test)] mod tests` in `cpu.rs`

**Interfaces:**
- Consumes: `CpuUsage` (Task 2).
- Produces: `pub fn read_cpu() -> Option<CpuUsage>`.

- [ ] **Step 1: Write the failing test**

Create `crates/rustline/src/cpu.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_stat_parses_aggregate_line() {
        let s = "cpu  100 0 100 700 100 0 0 0 0 0\ncpu0 50 0 50 350 50 0 0 0 0 0\n";
        let t = parse_proc_stat(s).unwrap();
        assert_eq!(t.idle, 800); // idle(700) + iowait(100)
        assert_eq!(t.total, 1000); // sum of all ten fields
    }

    #[test]
    fn proc_stat_no_cpu_line_is_none() {
        assert!(parse_proc_stat("intr 1 2 3\n").is_none());
    }

    #[test]
    fn busy_percent_over_interval() {
        let prev = CpuTimes { idle: 800, total: 1000 };
        let cur = CpuTimes { idle: 1000, total: 1400 };
        assert_eq!(busy_percent(prev, cur), 50.0); // dt=400, didle=200
    }

    #[test]
    fn busy_percent_zero_and_backward() {
        let x = CpuTimes { idle: 5, total: 10 };
        assert_eq!(busy_percent(x, x), 0.0); // dt == 0
        let hi = CpuTimes { idle: 1000, total: 2000 };
        let lo = CpuTimes { idle: 0, total: 0 };
        assert_eq!(busy_percent(hi, lo), 0.0); // backward -> saturates, no NaN
    }

    #[test]
    fn top_cpu_uses_last_sample() {
        let out = "Processes: 400 total\n\
                   CPU usage: 3.00% user, 2.00% sys, 95.00% idle\n\
                   CPU usage: 12.50% user, 6.25% sys, 81.25% idle\n";
        let p = parse_top_cpu(out).unwrap();
        assert!((p - 18.75).abs() < 0.01); // 100 - 81.25 (the last line)
    }

    #[test]
    fn top_cpu_no_line_is_none() {
        assert!(parse_top_cpu("nothing here").is_none());
    }

    #[test]
    fn read_cpu_never_panics() {
        if let Some(c) = read_cpu() {
            assert!((0.0..=100.0).contains(&c.percent));
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustline cpu`
Expected: FAIL to compile — `read_cpu` / `parse_proc_stat` / `busy_percent` / `parse_top_cpu` / `CpuTimes` not found.

- [ ] **Step 3: Write minimal implementation**

Prepend to `crates/rustline/src/cpu.rs`:

```rust
//! Platform-specific CPU-utilization read, isolated at the `Context`-build edge.
//!
//! CPU usage is a delta between two cumulative snapshots. The Linux reader takes
//! both across a short sleep; macOS uses `top`'s own internal sample. Mirrors
//! `battery.rs`: the pure parsers compile under `test` on any host and carry the
//! unit tests, while only the file-read / subprocess / sleep is `#[cfg]`-gated.

use std::time::Duration;

use rustline_core::CpuUsage;

/// Linux two-read sampling window: short enough to keep `render` snappy, long
/// enough to be a stable reading. (macOS uses `top`'s own ~1 s sample instead.)
#[cfg(target_os = "linux")]
const CPU_SAMPLE_WINDOW: Duration = Duration::from_millis(120);

/// Read CPU utilization, or `None` if the platform is unsupported or the read
/// failed. Called once at Context-build time.
pub fn read_cpu() -> Option<CpuUsage> {
    #[cfg(target_os = "linux")]
    {
        read_cpu_linux()
    }
    #[cfg(target_os = "macos")]
    {
        read_cpu_macos()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
fn read_cpu_linux() -> Option<CpuUsage> {
    let prev = parse_proc_stat(&std::fs::read_to_string("/proc/stat").ok()?)?;
    std::thread::sleep(CPU_SAMPLE_WINDOW);
    let cur = parse_proc_stat(&std::fs::read_to_string("/proc/stat").ok()?)?;
    Some(CpuUsage {
        percent: busy_percent(prev, cur),
    })
}

#[cfg(target_os = "macos")]
fn read_cpu_macos() -> Option<CpuUsage> {
    let output = std::process::Command::new("top")
        .args(["-l", "2", "-n", "0"])
        .output()
        .ok()?;
    let stdout = String::from_utf8(output.stdout).ok()?;
    parse_top_cpu(&stdout).map(|percent| CpuUsage { percent })
}

#[derive(Clone, Copy)]
#[cfg(any(target_os = "linux", test))]
struct CpuTimes {
    idle: u64,
    total: u64,
}

/// Parse the aggregate `cpu ` line of `/proc/stat` into `(idle+iowait,
/// sum-of-all-fields)`. Ignores the per-core `cpuN` lines. Missing/unparseable
/// → `None`.
#[cfg(any(target_os = "linux", test))]
fn parse_proc_stat(text: &str) -> Option<CpuTimes> {
    let line = text
        .lines()
        .find(|l| l.split_whitespace().next() == Some("cpu"))?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .map(|f| f.parse::<u64>().ok())
        .collect::<Option<Vec<u64>>>()?;
    if fields.len() < 4 {
        return None;
    }
    let total: u64 = fields.iter().sum();
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
    Some(CpuTimes { idle, total })
}

/// Busy % over the interval between two cumulative snapshots. `dt == 0` or
/// backward counters (suspend/resume) → `0.0`, never negative or `NaN`.
#[cfg(any(target_os = "linux", test))]
fn busy_percent(prev: CpuTimes, cur: CpuTimes) -> f32 {
    let dt = cur.total.saturating_sub(prev.total);
    let didle = cur.idle.saturating_sub(prev.idle);
    if dt == 0 {
        return 0.0;
    }
    (dt.saturating_sub(didle) as f32 / dt as f32 * 100.0).clamp(0.0, 100.0)
}

/// Parse `top -l 2 -n 0` stdout: from the **last** `CPU usage:` line take the
/// number before `% idle` and return `100 - idle`. No such line → `None`.
#[cfg(any(target_os = "macos", test))]
fn parse_top_cpu(output: &str) -> Option<f32> {
    let line = output.lines().filter(|l| l.contains("CPU usage:")).last()?;
    let idle_str = line
        .split("% idle")
        .next()?
        .rsplit(|c: char| !(c.is_ascii_digit() || c == '.'))
        .next()?;
    let idle: f32 = idle_str.parse().ok()?;
    Some((100.0 - idle).clamp(0.0, 100.0))
}
```

Add `mod cpu;` to `crates/rustline/src/main.rs` (next to `mod battery;` / `mod memory;`).

In `crates/rustline/src/build_context.rs`, change the `cpu: None,` line in `build_region_context` to:

```rust
        cpu: crate::cpu::read_cpu(),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rustline cpu`
Expected: PASS (7 tests; `read_cpu_never_panics` incurs the ~120 ms sleep once).
Then `cargo clippy -p rustline --all-targets -- -D warnings` and `cargo fmt --all`.

- [ ] **Step 5: Commit**

```bash
git add crates/rustline/src/cpu.rs crates/rustline/src/main.rs crates/rustline/src/build_context.rs
git commit -m "feat(rustline): platform cpu read wired into Context"
```

---

### Task 8: Binary integration test (end-to-end render)

Prove the `build_context → read → Context → widget` wiring through the real binary, host-independently, mirroring `render_right_with_battery_renders_gracefully`.

**Files:**
- Modify: `crates/rustline/tests/smoke.rs` (add one test)

**Interfaces:**
- Consumes: the whole pipeline (Tasks 1–7).
- Produces: nothing (test only).

- [ ] **Step 1: Write the test**

Add to `crates/rustline/tests/smoke.rs`, mirroring `render_right_with_battery_renders_gracefully` verbatim (same `env!("CARGO_BIN_EXE_rustline")` + `XDG_CONFIG_HOME` + `isolate` mechanism — no `assert_cmd`):

```rust
#[test]
fn render_right_with_cpu_memory_renders_gracefully() {
    // `cpu`/`memory` in a layout must render alongside built-ins and exit 0 on
    // ANY host. Proves the build_context -> read_cpu/read_memory -> Context ->
    // widgets wiring does not crash; deterministic formatting is pinned by the
    // widgets' own unit tests. On Linux the /proc reads succeed and the widgets
    // render live values; elsewhere they skip via empty down_format — either way
    // `datetime` guarantees non-empty tmux markup.
    let tmp = tempfile::tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    std::fs::write(
        cfgdir.join("config.toml"),
        "[layout]\nright = [\"cpu\", \"memory\", \"datetime\"]\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["render", "right"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("#["), "built-ins still render: {s}");
}
```

- [ ] **Step 2: Run the test to verify it passes**

Run: `cargo test -p rustline --test smoke render_right_with_cpu_memory_renders_gracefully`
Expected: PASS.

- [ ] **Step 3: Full suite + lints**

Run: `just test` (hermetic) — all green.
Run: `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all --check`.

- [ ] **Step 4: Commit**

```bash
git add crates/rustline/tests/smoke.rs
git commit -m "test(rustline): end-to-end cpu/memory render smoke test"
```

---

### Task 9: Documentation

Sync the living docs — `CLAUDE.md` and `README.md` — with the two new widgets. (Per the project rule: update the widget lists in **both** files.)

**Files:**
- Modify: `CLAUDE.md`
- Modify: `README.md`

**Interfaces:**
- Consumes: everything.
- Produces: nothing (docs only).

- [ ] **Step 1: Update `CLAUDE.md`**

Make these edits:
- **Module map** (`rustline-core` → `widgets/`): add `bar.rs` (shared gauge
  renderer), `cpu.rs`, `memory.rs`; note `context.rs` now also carries
  `MemInfo`/`CpuUsage` and `Context.cpu`/`Context.memory`; bump "the nine
  built-ins" → "the eleven built-ins" and add `cpu`, `memory` to the enumerated
  list.
- **Module map** (`rustline` bin): add `cpu.rs` and `memory.rs` as new
  `#[cfg(target_os)]` read surfaces (Linux `/proc`; macOS `top` / `sysctl` +
  `vm_stat`), each delegating to pure parsers; note `build_context.rs` now also
  sets `cpu`/`memory`.
- **"Platform-specific reads stay at the `Context`-build edge"** note: change
  "`read_battery()` … is the only `#[cfg(target_os)]` surface" to name all three
  (`read_battery`, `read_cpu`, `read_memory`).
- **Config section:** add the `[widgets.cpu]` / `[widgets.memory]` tables, their
  placeholders (`cpu`: `{percent}`, `{bar}`, `{icon}`; `memory`: `{used}`,
  `{total}`, `{avail}`, `{percent}`, `{bar}`, `{icon}`), and `bar_width`
  (default 8). Update the default right layout to
  `[cwd, cpu, memory, loadavg, datetime]`.
- **Render pipeline / CLI default layout** mentions of `right = [cwd, loadavg,
  datetime]`: update to include `cpu, memory`.
- **Roadmap:** add a "Done: cpu + memory widgets …" line and a new future item
  "historical sparkline (last-X-seconds graph) for cpu/memory — needs
  cross-invocation sample persistence; deferred to its own spec". Link this
  spec from the Design-docs list.

- [ ] **Step 2: Update `README.md`**

- **Features** list: add a bullet for the built-in `cpu` and `memory` widgets
  (usage %, human sizes, and a Unicode gauge bar; in the default right layout).
- **Widget names** line (~66–69): add `cpu` and `memory` to the enumerated
  available names.
- **Default layout → Right** (~56): update the example to include CPU and memory,
  e.g. `… · cpu `󰘚 37%` · memory `󰍛 6.2G/16G` · load average · date/time`.
- **Configuration:** add a short "CPU and memory widgets" subsection with a
  config example:

```toml
[widgets.cpu]
format = "{icon} {bar} {percent}%"   # default "{icon} {percent}%"
bar_width = 8

[widgets.memory]
format = "{icon} {used}/{total}"     # default; or "{icon} {bar} {percent}%"
bar_width = 8
```

  Note the placeholders and that on an unsupported platform / failed read the
  widget renders its `down_format` (default empty → nothing), same as battery.
- **Design** links: add the cpu/memory spec.

- [ ] **Step 3: Verify docs match reality**

Run: `grep -n 'cpu\|memory' CLAUDE.md README.md | head` and eyeball that the
default-layout, widget-count, and config statements are consistent with the code.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md README.md
git commit -m "docs: cpu + memory widgets (module map, config, README)"
```

---

## Self-Review

**1. Spec coverage** (each spec section → task):
- §3 data model (`MemInfo`/`CpuUsage`, fields) → Task 2. ✓
- §4a CPU read + parsers → Task 7. ✓  §4b memory read + parsers → Task 6. ✓
- §5 `gauge_bar` → Task 1. ✓
- §6a memory widget + `format_bytes` → Task 3. ✓  §6b cpu widget → Task 4. ✓
- §7 config opts + default layout → Task 5. ✓
- §8 wiring (registry, build_context, main.rs mod decls) → Tasks 5–7. ✓
- §9 tests: pure-parser + widget + bar + config tests distributed across their
  tasks; smoke → Task 8. ✓
- §10 invariants: #1/#6 (reads at edge, None→nothing) Tasks 6–7; #2 (serde)
  Task 2; #3 (total load) Task 5; #5 (default right order) Task 5. ✓
- §11 docs → Task 9. ✓

**2. Placeholder scan:** every code step shows complete code; every test step
shows the assertions; every command shows expected result. No TBD/"similar to".
The one soft spot — Task 8's config-path plumbing — is explicitly flagged with a
fallback instruction to copy the sibling smoke test's mechanism. ✓

**3. Type consistency:** `MemInfo { total_bytes, used_bytes, available_bytes }`
and `CpuUsage { percent: f32 }` are used identically in Tasks 2/3/4/5/6/7.
`gauge_bar(fraction: f64, width: usize) -> String` and `format_bytes(u64) ->
String` signatures match every call site. `CpuTimes { idle, total }` is internal
to `cpu.rs` (Task 7 only). Widget structs `{ format, down_format, bar_width }`
match the registry factories in Task 5. Icons: memory `\u{f035b}`, cpu
`\u{f061a}` — consistent between widget impl (Tasks 3/4) and the registry test
(Task 5). ✓
