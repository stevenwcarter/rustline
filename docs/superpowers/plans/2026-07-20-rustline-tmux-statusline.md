# rustline tmux statusline — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Rust tmux statusline (`rustline`) that renders left/center/right powerline regions from built-in widgets, usable in a live tmux session today.

**Architecture:** Cargo workspace: `rustline-core` (pure, serde-serializable core — `Context` in, tmux-format `String` out) and `rustline` (clap CLI front-end that builds a `Context` per tmux shell-out and prints a region). The pure render core is the reused seam for a future daemon and WASM host.

**Tech Stack:** Rust edition 2024, clap (derive), serde + toml, chrono, tracing + tracing-subscriber, libc (`getloadavg`), gethostname, thiserror, anyhow.

## Global Constraints

- **Edition 2024** in every crate `Cargo.toml`; workspace ships `rustfmt.toml` with `edition = "2024"`; all crate editions match it.
- Workspace `resolver = "2"`.
- Every core type that crosses the (future) plugin boundary — `Context`, `WindowCtx`, `Segment`, `Style`, `Color` — derives `serde::Serialize + Deserialize`. This is the WASM ABI; keep it serializable.
- **Widgets read only from `Context`.** No widget reads env/filesystem/clock at render time; the front-end populates `Context`. (Spec §9 invariant #1.)
- **Rendering is total.** Config load, a failing widget, or a panicking widget must never abort a region render — degrade to empty output + `tracing::warn!`. (Spec §8, §9 invariant #3.)
- Code must be clippy-clean (`cargo clippy --all-targets -- -D warnings`) and rustfmt-clean (`cargo fmt --all --check`).
- Commit `Cargo.lock` in the same commit that adds/changes dependencies.
- Add deps with `cargo add` (latest stable); `default-features = false` where practical.

---

## File Structure

```
rustline/
  Cargo.toml                         # [workspace], resolver=2, release profile
  rustfmt.toml                       # edition = "2024"
  crates/
    rustline-core/
      Cargo.toml
      src/
        lib.rs                       # re-exports; module wiring
        segment.rs                   # Segment, Style, Color (+ Color::to_tmux)
        context.rs                   # Context, WindowCtx
        render.rs                    # Direction, Theme, render_region
        widget.rs                    # Widget trait, Registry
        assemble.rs                  # assign_palette, render_named_region (panic-guarded)
        config.rs                    # Config, Layout, ThemeConfig, widget opts, load()
        widgets/
          mod.rs                     # re-exports + Registry::with_builtins
          datetime.rs
          loadavg.rs
          pane_id.rs
          hostname.rs
          cwd.rs
          windows.rs
    rustline/
      Cargo.toml
      src/
        main.rs                      # clap dispatch
        cli.rs                       # clap derive structs
        build_context.rs             # Context from CLI args + system reads
        tmux_conf.rs                 # `init` tmux.conf block text
```

---

### Task 1: Workspace scaffold

**Files:**
- Create: `Cargo.toml` (workspace), `rustfmt.toml`
- Create: `crates/rustline-core/Cargo.toml`, `crates/rustline-core/src/lib.rs`
- Create: `crates/rustline/Cargo.toml`, `crates/rustline/src/main.rs`

**Interfaces:**
- Produces: a building workspace with both crates and all dependencies declared.

- [ ] **Step 1: Write workspace `Cargo.toml`**

```toml
[workspace]
resolver = "2"
members = ["crates/rustline-core", "crates/rustline"]

[workspace.package]
edition = "2024"
version = "0.1.0"
license = "MIT"

[profile.release]
codegen-units = 1
lto = "thin"
opt-level = 3
```

- [ ] **Step 2: Write `rustfmt.toml`**

```toml
edition = "2024"
```

- [ ] **Step 3: Write `crates/rustline-core/Cargo.toml`**

```toml
[package]
name = "rustline-core"
edition.workspace = true
version.workspace = true
license.workspace = true

[dependencies]
serde = { version = "1", features = ["derive"] }
chrono = { version = "0.4", default-features = false, features = ["clock", "serde"] }
toml = "0.9"
tracing = "0.1"
thiserror = "2"

[dev-dependencies]
serde_json = "1"
```

- [ ] **Step 4: Write `crates/rustline/Cargo.toml`**

```toml
[package]
name = "rustline"
edition.workspace = true
version.workspace = true
license.workspace = true

[[bin]]
name = "rustline"
path = "src/main.rs"

[dependencies]
rustline-core = { path = "../rustline-core" }
clap = { version = "4", features = ["derive"] }
chrono = { version = "0.4", default-features = false, features = ["clock"] }
libc = "0.2"
gethostname = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
anyhow = "1"
```

- [ ] **Step 5: Stub `crates/rustline-core/src/lib.rs`**

```rust
//! rustline-core: pure, front-end-agnostic status line rendering.
```

- [ ] **Step 6: Stub `crates/rustline/src/main.rs`**

```rust
fn main() {
    println!("rustline");
}
```

- [ ] **Step 7: Build and verify**

Run: `cargo build && cargo clippy --all-targets -- -D warnings && cargo fmt --all --check`
Expected: builds clean, no clippy warnings, no fmt diff. (Run `cargo add` per crate instead of hand-editing versions if a version fails to resolve; then re-run.)

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "chore: scaffold rustline workspace (core lib + bin)"
```

---

### Task 2: Core data types — Segment, Style, Color, Context, WindowCtx

**Files:**
- Create: `crates/rustline-core/src/segment.rs`
- Create: `crates/rustline-core/src/context.rs`
- Modify: `crates/rustline-core/src/lib.rs` (add `pub mod segment; pub mod context;` and re-exports)
- Test: inline `#[cfg(test)]` in each file

**Interfaces:**
- Produces:
  - `Color { Named(String), Indexed(u8), Rgb(u8,u8,u8) }` with `fn to_tmux(&self) -> String`
  - `Style { fg: Option<Color>, bg: Option<Color>, bold: bool }` (derives `Default`)
  - `Segment { text: String, style: Style }` with `fn new(text: impl Into<String>) -> Self`
  - `Context { session_name, window_index, pane_index, pane_current_path, home, hostname: String; loadavg: Option<[f64;3]>; now: DateTime<Local>; window: Option<WindowCtx> }`
  - `WindowCtx { index: String, name: String, flags: String, is_current: bool }`
  - All the above derive `Serialize, Deserialize, Clone, Debug` (+ `PartialEq` where useful).

- [ ] **Step 1: Write failing test for `Color::to_tmux` (`segment.rs`)**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_to_tmux_named_indexed_rgb() {
        assert_eq!(Color::Named("cyan".into()).to_tmux(), "cyan");
        assert_eq!(Color::Indexed(236).to_tmux(), "colour236");
        assert_eq!(Color::Rgb(0x1a, 0x2b, 0x3c).to_tmux(), "#1a2b3c");
    }

    #[test]
    fn segment_new_defaults_style() {
        let s = Segment::new("hi");
        assert_eq!(s.text, "hi");
        assert_eq!(s.style, Style::default());
    }
}
```

- [ ] **Step 2: Run test, verify it fails**

Run: `cargo test -p rustline-core segment`
Expected: FAIL (types not defined).

- [ ] **Step 3: Implement `segment.rs`**

```rust
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Color {
    Named(String),
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    pub fn to_tmux(&self) -> String {
        match self {
            Color::Named(n) => n.clone(),
            Color::Indexed(i) => format!("colour{i}"),
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    #[serde(default)]
    pub bold: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub text: String,
    pub style: Style,
}

impl Segment {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), style: Style::default() }
    }

    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self { text: text.into(), style }
    }
}
```

- [ ] **Step 4: Write failing test for `Context` serde round-trip (`context.rs`)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};

    fn sample() -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/home/steve/src/rustline".into(),
            home: "/home/steve".into(),
            hostname: "scadrial".into(),
            loadavg: Some([0.42, 0.31, 0.29]),
            now: Local.with_ymd_and_hms(2026, 7, 20, 17, 49, 0).single().unwrap(),
            window: None,
        }
    }

    #[test]
    fn context_serde_round_trip() {
        let ctx = sample();
        let json = serde_json::to_string(&ctx).unwrap();
        let back: Context = serde_json::from_str(&json).unwrap();
        assert_eq!(back.session_name, ctx.session_name);
        assert_eq!(back.loadavg, ctx.loadavg);
        assert_eq!(back.now, ctx.now);
    }
}
```

- [ ] **Step 5: Run test, verify it fails**

Run: `cargo test -p rustline-core context`
Expected: FAIL (types not defined).

- [ ] **Step 6: Implement `context.rs`**

```rust
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowCtx {
    pub index: String,
    pub name: String,
    pub flags: String,
    pub is_current: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Context {
    pub session_name: String,
    pub window_index: String,
    pub pane_index: String,
    pub pane_current_path: String,
    pub home: String,
    pub hostname: String,
    pub loadavg: Option<[f64; 3]>,
    pub now: DateTime<Local>,
    pub window: Option<WindowCtx>,
}
```

- [ ] **Step 7: Wire modules in `lib.rs`**

```rust
//! rustline-core: pure, front-end-agnostic status line rendering.
pub mod context;
pub mod segment;

pub use context::{Context, WindowCtx};
pub use segment::{Color, Segment, Style};
```

- [ ] **Step 8: Run tests, verify pass; lint**

Run: `cargo test -p rustline-core && cargo clippy --all-targets -- -D warnings && cargo fmt --all --check`
Expected: PASS, clean.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat(core): Segment/Style/Color and Context/WindowCtx serde types"
```

---

### Task 3: Powerline renderer (`render.rs`)

**Files:**
- Create: `crates/rustline-core/src/render.rs`
- Modify: `crates/rustline-core/src/lib.rs` (add `pub mod render;` + re-exports)
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Consumes: `Segment`, `Style`, `Color` (Task 2).
- Produces:
  - `enum Direction { Left, Right }`
  - `struct Theme { palette: Vec<Color>, fg: Color, bar_bg: Color, hard_left: String, hard_right: String, soft_left: String, soft_right: String, soft_fg: Color }` with `Default` matching spec glyphs (`` `` `` ``).
  - `fn render_region(dir: Direction, segments: &[Segment], theme: &Theme) -> String`

**Contract (implement exactly):** each segment is emitted as `#[fg=<F>,bg=<B>]<space>text<space>` where `F = style.fg.unwrap_or(theme.fg)`, `B = style.bg.unwrap_or(theme.bar_bg)`. Between adjacent segments a **separator** is emitted:
- if the two effective bgs differ → **hard** glyph (`hard_left` for `Direction::Left`, `hard_right` for `Right`) styled `#[fg=<prev.bg>,bg=<next.bg>]` (for `Left`; for `Right` the roles mirror so the arrow points right→outward, i.e. `#[fg=<next.bg>,bg=<prev.bg>]`).
- if equal → **soft** glyph in `#[fg=soft_fg,bg=<shared.bg>]`.
Edges: emit an outer hard glyph transitioning the first (for `Left`) / last (for `Right`) segment's bg to/from `bar_bg`, **only when** that segment's effective bg differs from `bar_bg`; when equal, emit no edge glyph. End the whole string with `#[default]`. Empty `segments` → `String::new()`.

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Color, Segment, Style};

    fn theme() -> Theme {
        Theme {
            palette: vec![Color::Indexed(31), Color::Indexed(238)],
            fg: Color::Indexed(255),
            bar_bg: Color::Indexed(234),
            hard_left: "\u{e0b0}".into(),
            hard_right: "\u{e0b2}".into(),
            soft_left: "\u{e0b1}".into(),
            soft_right: "\u{e0b3}".into(),
            soft_fg: Color::Indexed(240),
        }
    }

    fn seg(text: &str, bg: u8) -> Segment {
        Segment::styled(text, Style { fg: None, bg: Some(Color::Indexed(bg)), bold: false })
    }

    #[test]
    fn empty_is_empty() {
        assert_eq!(render_region(Direction::Left, &[], &theme()), "");
    }

    #[test]
    fn single_segment_has_text_and_default_reset() {
        let out = render_region(Direction::Left, &[seg("hi", 31)], &theme());
        assert!(out.contains("hi"), "text present: {out}");
        assert!(out.contains("bg=colour31"), "seg bg: {out}");
        assert!(out.ends_with("#[default]"), "reset: {out}");
        // seg bg (31) != bar_bg (234) => trailing edge arrow to bar bg
        assert!(out.contains("\u{e0b0}"), "edge glyph: {out}");
    }

    #[test]
    fn different_bg_uses_hard_separator() {
        let out = render_region(Direction::Left, &[seg("a", 31), seg("b", 238)], &theme());
        // hard separator between them, fg=prev.bg bg=next.bg
        assert!(out.contains("#[fg=colour31,bg=colour238]\u{e0b0}"), "hard sep: {out}");
    }

    #[test]
    fn same_bg_uses_soft_separator() {
        let out = render_region(Direction::Left, &[seg("a", 31), seg("b", 31)], &theme());
        assert!(out.contains("#[fg=colour240,bg=colour31]\u{e0b1}"), "soft sep: {out}");
    }

    #[test]
    fn right_direction_uses_right_glyphs() {
        let out = render_region(Direction::Right, &[seg("a", 31), seg("b", 238)], &theme());
        assert!(out.contains("\u{e0b2}"), "right hard glyph: {out}");
    }

    #[test]
    fn bg_equal_bar_bg_has_no_edge_glyph() {
        let s = seg("plain", 234); // == bar_bg
        let out = render_region(Direction::Left, &[s], &theme());
        assert!(!out.contains("\u{e0b0}"), "no edge glyph when bg==bar_bg: {out}");
        assert!(out.contains("plain"));
    }
}
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `cargo test -p rustline-core render`
Expected: FAIL (not defined).

- [ ] **Step 3: Implement `render.rs`**

Implement `Direction`, `Theme` (+ `Default` with the glyphs/colors above), and `render_region` per the Contract. Reference implementation:

```rust
use crate::{Color, Segment};
use std::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction { Left, Right }

#[derive(Clone, Debug)]
pub struct Theme {
    pub palette: Vec<Color>,
    pub fg: Color,
    pub bar_bg: Color,
    pub hard_left: String,
    pub hard_right: String,
    pub soft_left: String,
    pub soft_right: String,
    pub soft_fg: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            palette: vec![Color::Indexed(31), Color::Indexed(238)],
            fg: Color::Indexed(255),
            bar_bg: Color::Indexed(234),
            hard_left: "\u{e0b0}".into(),
            hard_right: "\u{e0b2}".into(),
            soft_left: "\u{e0b1}".into(),
            soft_right: "\u{e0b3}".into(),
            soft_fg: Color::Indexed(240),
        }
    }
}

impl Theme {
    fn hard(&self, dir: Direction) -> &str {
        match dir { Direction::Left => &self.hard_left, Direction::Right => &self.hard_right }
    }
    fn soft(&self, dir: Direction) -> &str {
        match dir { Direction::Left => &self.soft_left, Direction::Right => &self.soft_right }
    }
}

pub fn render_region(dir: Direction, segments: &[Segment], theme: &Theme) -> String {
    if segments.is_empty() {
        return String::new();
    }
    let eff_fg = |s: &Segment| s.style.fg.clone().unwrap_or_else(|| theme.fg.clone());
    let eff_bg = |s: &Segment| s.style.bg.clone().unwrap_or_else(|| theme.bar_bg.clone());

    let mut out = String::new();

    // Leading edge (Left only): bar_bg -> first.bg when they differ.
    if dir == Direction::Left {
        let first_bg = eff_bg(&segments[0]);
        if first_bg != theme.bar_bg {
            let _ = write!(out, "#[fg={},bg={}]{}",
                theme.bar_bg.to_tmux(), first_bg.to_tmux(), theme.hard(dir));
        }
    }

    for (i, s) in segments.iter().enumerate() {
        // separator before this segment (except first)
        if i > 0 {
            let prev_bg = eff_bg(&segments[i - 1]);
            let cur_bg = eff_bg(s);
            if prev_bg != cur_bg {
                let (f, b) = match dir {
                    Direction::Left => (prev_bg.to_tmux(), cur_bg.to_tmux()),
                    Direction::Right => (cur_bg.to_tmux(), prev_bg.to_tmux()),
                };
                let _ = write!(out, "#[fg={f},bg={b}]{}", theme.hard(dir));
            } else {
                let _ = write!(out, "#[fg={},bg={}]{}",
                    theme.soft_fg.to_tmux(), cur_bg.to_tmux(), theme.soft(dir));
            }
        }
        let bold = if s.style.bold { ",bold" } else { "" };
        let _ = write!(out, "#[fg={},bg={}{bold}] {} ",
            eff_fg(s).to_tmux(), eff_bg(s).to_tmux(), s.text);
    }

    // Trailing edge (Left): last.bg -> bar_bg when they differ.
    if dir == Direction::Left {
        let last_bg = eff_bg(segments.last().unwrap());
        if last_bg != theme.bar_bg {
            let _ = write!(out, "#[fg={},bg={}]{}",
                last_bg.to_tmux(), theme.bar_bg.to_tmux(), theme.hard(dir));
        }
    }

    out.push_str("#[default]");
    out
}
```

> Note: the `Direction::Right` leading/trailing edge handling mirrors the above (leading edge on the right region transitions `bar_bg`→`last`). Keep the two failing right-direction tests green; add the mirrored edge writes so a right region also blends at its outer edge. The implementer must ensure all Step 1 tests pass — extend edge handling for `Right` as needed without breaking `Left`.

- [ ] **Step 4: Run tests, verify pass; lint**

Run: `cargo test -p rustline-core render && cargo clippy --all-targets -- -D warnings`
Expected: PASS, clean.

- [ ] **Step 5: Wire + commit**

Add `pub mod render; pub use render::{render_region, Direction, Theme};` to `lib.rs`.
```bash
cargo fmt --all && git add -A
git commit -m "feat(core): powerline render_region with hard/soft separators + edges"
```

---

### Task 4: Widget trait + Registry (`widget.rs`)

**Files:**
- Create: `crates/rustline-core/src/widget.rs`
- Modify: `crates/rustline-core/src/lib.rs`
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Consumes: `Context`, `Segment` (Task 2).
- Produces:
  - `trait Widget { fn render(&self, ctx: &Context) -> Vec<Segment>; }`
  - `struct Registry` with `fn new() -> Self`, `fn register(&mut self, name: &str, factory: Box<dyn Fn() -> Box<dyn Widget> + Send + Sync>)`, and `fn build(&self, name: &str) -> Option<Box<dyn Widget>>`.
  - `fn resolve(&self, names: &[String]) -> Vec<Box<dyn Widget>>` — builds each; **unknown names are skipped with `tracing::warn!`**.

- [ ] **Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Segment};
    use chrono::{Local, TimeZone};

    struct Fixed(&'static str);
    impl Widget for Fixed {
        fn render(&self, _ctx: &Context) -> Vec<Segment> { vec![Segment::new(self.0)] }
    }

    fn ctx() -> Context {
        Context {
            session_name: "0".into(), window_index: "0".into(), pane_index: "0".into(),
            pane_current_path: "/".into(), home: "/home/steve".into(), hostname: "h".into(),
            loadavg: None, now: Local.with_ymd_and_hms(2026,7,20,17,49,0).single().unwrap(),
            window: None,
        }
    }

    #[test]
    fn resolve_skips_unknown_and_keeps_order() {
        let mut r = Registry::new();
        r.register("a", Box::new(|| Box::new(Fixed("A"))));
        r.register("b", Box::new(|| Box::new(Fixed("B"))));
        let widgets = r.resolve(&["a".into(), "missing".into(), "b".into()]);
        let texts: Vec<String> = widgets.iter()
            .flat_map(|w| w.render(&ctx())).map(|s| s.text).collect();
        assert_eq!(texts, vec!["A".to_string(), "B".to_string()]);
    }
}
```

- [ ] **Step 2: Run test, verify fail**

Run: `cargo test -p rustline-core widget`
Expected: FAIL.

- [ ] **Step 3: Implement `widget.rs`**

```rust
use crate::{Context, Segment};
use std::collections::HashMap;

pub trait Widget {
    fn render(&self, ctx: &Context) -> Vec<Segment>;
}

type Factory = Box<dyn Fn() -> Box<dyn Widget> + Send + Sync>;

#[derive(Default)]
pub struct Registry {
    factories: HashMap<String, Factory>,
}

impl Registry {
    pub fn new() -> Self { Self::default() }

    pub fn register(&mut self, name: &str, factory: Factory) {
        self.factories.insert(name.to_string(), factory);
    }

    pub fn build(&self, name: &str) -> Option<Box<dyn Widget>> {
        self.factories.get(name).map(|f| f())
    }

    pub fn resolve(&self, names: &[String]) -> Vec<Box<dyn Widget>> {
        names.iter().filter_map(|n| match self.build(n) {
            Some(w) => Some(w),
            None => { tracing::warn!(widget = %n, "unknown widget, skipping"); None }
        }).collect()
    }
}
```

- [ ] **Step 4: Run test, verify pass; lint. Wire `pub mod widget; pub use widget::{Registry, Widget};` in `lib.rs`.**

Run: `cargo test -p rustline-core widget && cargo clippy --all-targets -- -D warnings`

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && git add -A
git commit -m "feat(core): Widget trait + Registry with unknown-widget skip"
```

---

### Task 5: `datetime` + `loadavg` widgets

**Files:**
- Create: `crates/rustline-core/src/widgets/mod.rs` (module hub), `crates/rustline-core/src/widgets/datetime.rs`, `crates/rustline-core/src/widgets/loadavg.rs`
- Modify: `crates/rustline-core/src/lib.rs` (`pub mod widgets;`)
- Test: inline

**Interfaces:**
- Consumes: `Context` (`now`, `loadavg`), `Widget`, `Segment`.
- Produces:
  - `DateTime { format: String }` impl `Widget` — `Default` format `"%a < %Y-%m-%d < %H:%M"`.
  - `LoadAvg` impl `Widget`.
  - `widgets/mod.rs` re-exports both. (`Registry::with_builtins` is added in Task 9.)

- [ ] **Step 1: Failing tests**

`datetime.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Widget};
    use chrono::{Local, TimeZone};

    fn ctx_at() -> Context {
        Context {
            session_name: "0".into(), window_index: "0".into(), pane_index: "0".into(),
            pane_current_path: "/".into(), home: "/h".into(), hostname: "h".into(),
            loadavg: None,
            now: Local.with_ymd_and_hms(2026, 7, 20, 17, 49, 0).single().unwrap(),
            window: None,
        }
    }

    #[test]
    fn default_format_renders_expected() {
        let w = DateTime::default();
        assert_eq!(w.render(&ctx_at())[0].text, "Mon < 2026-07-20 < 17:49");
    }

    #[test]
    fn custom_format_honored() {
        let w = DateTime { format: "%H:%M".into() };
        assert_eq!(w.render(&ctx_at())[0].text, "17:49");
    }
}
```

`loadavg.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Widget};
    use chrono::{Local, TimeZone};

    fn ctx_load(l: Option<[f64;3]>) -> Context {
        Context {
            session_name: "0".into(), window_index: "0".into(), pane_index: "0".into(),
            pane_current_path: "/".into(), home: "/h".into(), hostname: "h".into(),
            loadavg: l, now: Local.with_ymd_and_hms(2026,7,20,17,49,0).single().unwrap(),
            window: None,
        }
    }

    #[test]
    fn formats_three_values() {
        let out = LoadAvg.render(&ctx_load(Some([0.42, 0.31, 0.296])));
        assert_eq!(out[0].text, "0.42 0.31 0.30");
    }

    #[test]
    fn none_renders_nothing() {
        assert!(LoadAvg.render(&ctx_load(None)).is_empty());
    }
}
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test -p rustline-core widgets::`
Expected: FAIL.

- [ ] **Step 3: Implement**

`datetime.rs`:
```rust
use crate::{Context, Segment, Widget};

pub struct DateTime { pub format: String }

impl Default for DateTime {
    fn default() -> Self { Self { format: "%a < %Y-%m-%d < %H:%M".into() } }
}

impl Widget for DateTime {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        vec![Segment::new(ctx.now.format(&self.format).to_string())]
    }
}
```

`loadavg.rs`:
```rust
use crate::{Context, Segment, Widget};

pub struct LoadAvg;

impl Widget for LoadAvg {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        match ctx.loadavg {
            Some([a, b, c]) => vec![Segment::new(format!("{a:.2} {b:.2} {c:.2}"))],
            None => vec![],
        }
    }
}
```

`widgets/mod.rs`:
```rust
pub mod datetime;
pub mod loadavg;

pub use datetime::DateTime;
pub use loadavg::LoadAvg;
```

- [ ] **Step 4: Run tests, verify pass; lint.** Add `pub mod widgets;` to `lib.rs`.

Run: `cargo test -p rustline-core && cargo clippy --all-targets -- -D warnings`

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && git add -A
git commit -m "feat(core): datetime and loadavg widgets"
```

---

### Task 6: `pane_id` + `hostname` + `cwd` widgets

**Files:**
- Create: `crates/rustline-core/src/widgets/pane_id.rs`, `hostname.rs`, `cwd.rs`
- Modify: `crates/rustline-core/src/widgets/mod.rs`
- Test: inline

**Interfaces:**
- Consumes: `Context` (`session_name/window_index/pane_index`, `hostname`, `pane_current_path`, `home`).
- Produces:
  - `PaneId` → `"{session}:{window}.{pane}"`.
  - `Hostname` → hostname truncated at first `.`.
  - `Cwd { abbreviate_home: bool }` (`Default` = true) → path with `home` prefix replaced by `~` when enabled.

- [ ] **Step 1: Failing tests** (one `#[cfg(test)]` per file; construct `Context` via a shared helper pattern like Task 5)

```rust
// pane_id.rs
#[test]
fn pane_id_formats_session_window_pane() {
    // ctx with session_name="0", window_index="0", pane_index="0"
    assert_eq!(PaneId.render(&ctx())[0].text, "0:0.0");
}

// hostname.rs
#[test]
fn hostname_truncates_at_first_dot() {
    // ctx.hostname = "scadrial.example.com"
    assert_eq!(Hostname.render(&ctx())[0].text, "scadrial");
}

// cwd.rs
#[test]
fn cwd_abbreviates_home() {
    // ctx.home="/home/steve", pane_current_path="/home/steve/src/rustline"
    assert_eq!(Cwd::default().render(&ctx())[0].text, "~/src/rustline");
}
#[test]
fn cwd_no_abbrev_when_disabled() {
    let w = Cwd { abbreviate_home: false };
    assert_eq!(w.render(&ctx())[0].text, "/home/steve/src/rustline");
}
#[test]
fn cwd_unchanged_outside_home() {
    // ctx.home="/home/steve", pane_current_path="/etc"
    assert_eq!(Cwd::default().render(&ctx())[0].text, "/etc");
}
```

- [ ] **Step 2: Run, verify fail.** `cargo test -p rustline-core widgets::`

- [ ] **Step 3: Implement**

```rust
// pane_id.rs
use crate::{Context, Segment, Widget};
pub struct PaneId;
impl Widget for PaneId {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        vec![Segment::new(format!("{}:{}.{}", ctx.session_name, ctx.window_index, ctx.pane_index))]
    }
}
```
```rust
// hostname.rs
use crate::{Context, Segment, Widget};
pub struct Hostname;
impl Widget for Hostname {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let short = ctx.hostname.split('.').next().unwrap_or(&ctx.hostname);
        vec![Segment::new(short.to_string())]
    }
}
```
```rust
// cwd.rs
use crate::{Context, Segment, Widget};
pub struct Cwd { pub abbreviate_home: bool }
impl Default for Cwd { fn default() -> Self { Self { abbreviate_home: true } } }
impl Widget for Cwd {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let path = &ctx.pane_current_path;
        let text = if self.abbreviate_home && !ctx.home.is_empty() {
            match path.strip_prefix(&ctx.home) {
                Some(rest) => format!("~{rest}"),
                None => path.clone(),
            }
        } else {
            path.clone()
        };
        vec![Segment::new(text)]
    }
}
```
Add the three modules + re-exports to `widgets/mod.rs`.

- [ ] **Step 4: Run tests, verify pass; lint.**

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && git add -A
git commit -m "feat(core): pane_id, hostname, cwd widgets"
```

---

### Task 7: `windows` widget

**Files:**
- Create: `crates/rustline-core/src/widgets/windows.rs`
- Modify: `crates/rustline-core/src/widgets/mod.rs`
- Test: inline

**Interfaces:**
- Consumes: `Context.window: Option<WindowCtx>`, `Style`, `Color`.
- Produces: `Windows` impl `Widget`. Output: one segment `"{index}{flags} {name}"` (flags appended directly, e.g. `0* name`). When `is_current`, segment carries an emphasized `Style` (`bold: true`, `bg: Some(accent)`); when not current, `Style::default()` (text-only → blends with bar). When `ctx.window` is `None`, returns `vec![]`.

- [ ] **Step 1: Failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, WindowCtx, Widget};
    use chrono::{Local, TimeZone};

    fn ctx(win: Option<WindowCtx>) -> Context {
        Context {
            session_name: "0".into(), window_index: "0".into(), pane_index: "0".into(),
            pane_current_path: "/".into(), home: "/h".into(), hostname: "h".into(),
            loadavg: None, now: Local.with_ymd_and_hms(2026,7,20,17,49,0).single().unwrap(),
            window: win,
        }
    }

    #[test]
    fn current_window_text_and_emphasis() {
        let w = ctx(Some(WindowCtx { index: "0".into(), name: "name".into(), flags: "*".into(), is_current: true }));
        let out = Windows.render(&w);
        assert_eq!(out[0].text, "0* name");
        assert!(out[0].style.bold);
        assert!(out[0].style.bg.is_some());
    }

    #[test]
    fn inactive_window_is_plain() {
        let w = ctx(Some(WindowCtx { index: "1".into(), name: "other".into(), flags: "".into(), is_current: false }));
        let out = Windows.render(&w);
        assert_eq!(out[0].text, "1 other");
        assert!(!out[0].style.bold);
        assert!(out[0].style.bg.is_none());
    }

    #[test]
    fn no_window_ctx_renders_nothing() {
        assert!(Windows.render(&ctx(None)).is_empty());
    }
}
```

- [ ] **Step 2: Run, verify fail.**

- [ ] **Step 3: Implement `windows.rs`**

```rust
use crate::{Color, Context, Segment, Style, Widget};

pub struct Windows;

impl Widget for Windows {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let Some(w) = &ctx.window else { return vec![] };
        let text = format!("{}{} {}", w.index, w.flags, w.name);
        let style = if w.is_current {
            Style { fg: None, bg: Some(Color::Indexed(31)), bold: true }
        } else {
            Style::default()
        };
        vec![Segment::styled(text, style)]
    }
}
```
Add module + re-export to `widgets/mod.rs`.

- [ ] **Step 4: Run tests, verify pass; lint.**

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && git add -A
git commit -m "feat(core): windows widget with current-window emphasis"
```

---

### Task 8: Config (`config.rs`)

**Files:**
- Create: `crates/rustline-core/src/config.rs`
- Modify: `crates/rustline-core/src/lib.rs`
- Test: inline

**Interfaces:**
- Consumes: `Color`, `Theme` (Task 3).
- Produces:
  - `Config { layout: Layout, theme: ThemeConfig, widgets: WidgetOpts, plugins: HashMap<String, toml::Value> }` deriving `Serialize, Deserialize` with `#[serde(default)]` throughout.
  - `Layout { left: Vec<String>, center: Vec<String>, right: Vec<String> }` — `Default` = spec defaults (`["pane_id","hostname"]`, `["windows"]`, `["cwd","loadavg","datetime"]`).
  - `WidgetOpts { datetime: DateTimeOpts, cwd: CwdOpts }` (each `#[serde(default)]`).
  - `impl Config`: `fn default() -> Self`, `fn load(path: &Path) -> Config` (missing file → default; parse error → `warn!` + default), `fn to_theme(&self) -> Theme`.

- [ ] **Step 1: Failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_matches_spec() {
        let c = Config::default();
        assert_eq!(c.layout.left, vec!["pane_id", "hostname"]);
        assert_eq!(c.layout.center, vec!["windows"]);
        assert_eq!(c.layout.right, vec!["cwd", "loadavg", "datetime"]);
    }

    #[test]
    fn parse_overrides_layout_and_datetime() {
        let toml = r#"
[layout]
right = ["datetime"]
[widgets.datetime]
format = "%H:%M"
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.layout.right, vec!["datetime"]);
        assert_eq!(c.widgets.datetime.format, "%H:%M");
        // unspecified region falls back to default
        assert_eq!(c.layout.left, vec!["pane_id", "hostname"]);
    }

    #[test]
    fn malformed_load_falls_back_to_default() {
        let dir = std::env::temp_dir().join("rustline_test_badcfg");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        std::fs::write(&p, "this is not = valid = toml [[[").unwrap();
        let c = Config::load(&p);
        assert_eq!(c.layout.left, Config::default().layout.left);
    }

    #[test]
    fn missing_file_is_default() {
        let c = Config::load(std::path::Path::new("/no/such/rustline.toml"));
        assert_eq!(c.layout.center, vec!["windows"]);
    }

    #[test]
    fn plugins_table_retained() {
        let toml = r#"
[plugins."owner/repo"]
key = "value"
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert!(c.plugins.contains_key("owner/repo"));
    }
}
```

- [ ] **Step 2: Run, verify fail.** `cargo test -p rustline-core config`

- [ ] **Step 3: Implement `config.rs`**

```rust
use crate::{Color, render::Theme};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default = "default_left")] pub left: Vec<String>,
    #[serde(default = "default_center")] pub center: Vec<String>,
    #[serde(default = "default_right")] pub right: Vec<String>,
}
fn default_left() -> Vec<String> { vec!["pane_id".into(), "hostname".into()] }
fn default_center() -> Vec<String> { vec!["windows".into()] }
fn default_right() -> Vec<String> { vec!["cwd".into(), "loadavg".into(), "datetime".into()] }
impl Default for Layout {
    fn default() -> Self { Self { left: default_left(), center: default_center(), right: default_right() } }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DateTimeOpts { #[serde(default = "default_dt_format")] pub format: String }
fn default_dt_format() -> String { "%a < %Y-%m-%d < %H:%M".into() }
impl Default for DateTimeOpts { fn default() -> Self { Self { format: default_dt_format() } } }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CwdOpts { #[serde(default = "default_true")] pub abbreviate_home: bool }
fn default_true() -> bool { true }
impl Default for CwdOpts { fn default() -> Self { Self { abbreviate_home: true } } }

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WidgetOpts {
    #[serde(default)] pub datetime: DateTimeOpts,
    #[serde(default)] pub cwd: CwdOpts,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ThemeConfig {
    // Optional overrides; None => Theme::default() value. (Full palette wiring
    // may be added later; v1 supports glyph + core color overrides.)
    #[serde(default)] pub palette: Option<Vec<Color>>,
    #[serde(default)] pub fg: Option<Color>,
    #[serde(default)] pub bar_bg: Option<Color>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)] pub layout: Layout,
    #[serde(default)] pub theme: ThemeConfig,
    #[serde(default)] pub widgets: WidgetOpts,
    #[serde(default)] pub plugins: HashMap<String, toml::Value>,
}

impl Config {
    pub fn load(path: &Path) -> Config {
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(c) => c,
                Err(e) => { tracing::warn!(error = %e, "invalid config, using defaults"); Config::default() }
            },
            Err(_) => Config::default(),
        }
    }

    pub fn to_theme(&self) -> Theme {
        let mut t = Theme::default();
        if let Some(p) = &self.theme.palette { t.palette = p.clone(); }
        if let Some(fg) = &self.theme.fg { t.fg = fg.clone(); }
        if let Some(bg) = &self.theme.bar_bg { t.bar_bg = bg.clone(); }
        t
    }
}
```

> `Default for Config` via derive requires `Layout`/`WidgetOpts` `Default` (provided) — the derive uses each field's `Default`, which matches spec defaults. Verify `Config::default().layout` equals the spec layout in the test.

- [ ] **Step 4: Run tests, verify pass; lint.** Wire `pub mod config; pub use config::Config;` in `lib.rs`.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && git add -A
git commit -m "feat(core): TOML config with defaults, total load, theme mapping"
```

---

### Task 9: Region assembly + `Registry::with_builtins` (`assemble.rs`)

**Files:**
- Create: `crates/rustline-core/src/assemble.rs`
- Modify: `crates/rustline-core/src/widgets/mod.rs` (add `with_builtins`), `crates/rustline-core/src/lib.rs`
- Test: inline

**Interfaces:**
- Consumes: `Registry`, `Widget`, `Config`, `Theme`, `render_region`, all widgets.
- Produces:
  - `fn Registry::with_builtins(cfg: &Config) -> Registry` — registers `pane_id`, `hostname`, `windows`, `cwd` (using `cfg.widgets.cwd`), `loadavg`, `datetime` (using `cfg.widgets.datetime`).
  - `fn assign_palette(segments: &mut [Segment], theme: &Theme)` — for each segment whose `style.bg` is `None`, set it to `theme.palette[i % palette.len()]` (skip if palette empty).
  - `fn render_named_region(dir: Direction, names: &[String], ctx: &Context, registry: &Registry, theme: &Theme) -> String` — resolve widgets, render each **guarded by `std::panic::catch_unwind`** (a panicking widget yields no segments + `warn!`), flatten segments, `assign_palette`, then `render_region`.
  - `fn render_window(ctx: &Context, registry: &Registry, theme: &Theme) -> String` — renders the single `windows` segment via `render_region(Direction::Left, ...)` **without** `assign_palette` (window widget owns its style).

- [ ] **Step 1: Failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, Context, Direction};
    use chrono::{Local, TimeZone};

    fn ctx() -> Context {
        Context {
            session_name: "0".into(), window_index: "0".into(), pane_index: "0".into(),
            pane_current_path: "/home/steve/x".into(), home: "/home/steve".into(),
            hostname: "scadrial".into(), loadavg: Some([0.1,0.2,0.3]),
            now: Local.with_ymd_and_hms(2026,7,20,17,49,0).single().unwrap(), window: None,
        }
    }

    #[test]
    fn render_left_default_layout_contains_widgets() {
        let cfg = Config::default();
        let reg = Registry::with_builtins(&cfg);
        let theme = cfg.to_theme();
        let out = render_named_region(Direction::Left, &cfg.layout.left, &ctx(), &reg, &theme);
        assert!(out.contains("0:0.0"), "pane_id: {out}");
        assert!(out.contains("scadrial"), "hostname: {out}");
        assert!(out.contains("#["), "styled: {out}");
    }

    #[test]
    fn assign_palette_fills_missing_bg_alternating() {
        let theme = crate::Theme::default(); // palette len 2
        let mut segs = vec![crate::Segment::new("a"), crate::Segment::new("b"), crate::Segment::new("c")];
        assign_palette(&mut segs, &theme);
        assert_eq!(segs[0].style.bg, Some(theme.palette[0].clone()));
        assert_eq!(segs[1].style.bg, Some(theme.palette[1].clone()));
        assert_eq!(segs[2].style.bg, Some(theme.palette[0].clone()));
    }

    #[test]
    fn panicking_widget_does_not_break_region() {
        use crate::{Segment, Widget};
        struct Boom;
        impl Widget for Boom { fn render(&self, _c: &Context) -> Vec<Segment> { panic!("boom") } }
        let mut reg = Registry::with_builtins(&Config::default());
        reg.register("boom", Box::new(|| Box::new(Boom)));
        let theme = Theme::default();
        let names = vec!["boom".to_string(), "hostname".to_string()];
        let out = render_named_region(Direction::Left, &names, &ctx(), &reg, &theme);
        assert!(out.contains("scadrial"), "surviving widget still renders: {out}");
    }
}
```

- [ ] **Step 2: Run, verify fail.**

- [ ] **Step 3: Implement `assemble.rs` + `with_builtins`**

```rust
// assemble.rs
use crate::{Context, Registry, Segment, Widget};
use crate::render::{Direction, Theme, render_region};

pub fn assign_palette(segments: &mut [Segment], theme: &Theme) {
    if theme.palette.is_empty() { return; }
    for (i, s) in segments.iter_mut().enumerate() {
        if s.style.bg.is_none() {
            s.style.bg = Some(theme.palette[i % theme.palette.len()].clone());
        }
    }
}

fn render_guarded(w: &dyn Widget, ctx: &Context) -> Vec<Segment> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| w.render(ctx))) {
        Ok(segs) => segs,
        Err(_) => { tracing::warn!("widget panicked, skipping"); vec![] }
    }
}

pub fn render_named_region(
    dir: Direction, names: &[String], ctx: &Context, registry: &Registry, theme: &Theme,
) -> String {
    let widgets = registry.resolve(names);
    let mut segments: Vec<Segment> = widgets.iter()
        .flat_map(|w| render_guarded(w.as_ref(), ctx)).collect();
    assign_palette(&mut segments, theme);
    render_region(dir, &segments, theme)
}

pub fn render_window(ctx: &Context, registry: &Registry, theme: &Theme) -> String {
    let widgets = registry.resolve(&["windows".to_string()]);
    let segments: Vec<Segment> = widgets.iter()
        .flat_map(|w| render_guarded(w.as_ref(), ctx)).collect();
    render_region(Direction::Left, &segments, theme)
}
```

`widgets/mod.rs` — add:
```rust
use crate::{Config, Registry};
use crate::widgets::{DateTime, LoadAvg, Windows, cwd::Cwd, hostname::Hostname, pane_id::PaneId};

impl Registry {
    pub fn with_builtins(cfg: &Config) -> Registry {
        let mut r = Registry::new();
        r.register("pane_id", Box::new(|| Box::new(PaneId)));
        r.register("hostname", Box::new(|| Box::new(Hostname)));
        r.register("windows", Box::new(|| Box::new(Windows)));
        r.register("loadavg", Box::new(|| Box::new(LoadAvg)));
        let dt = cfg.widgets.datetime.format.clone();
        r.register("datetime", Box::new(move || Box::new(DateTime { format: dt.clone() })));
        let abbrev = cfg.widgets.cwd.abbreviate_home;
        r.register("cwd", Box::new(move || Box::new(Cwd { abbreviate_home: abbrev })));
        r
    }
}
```
Wire `pub mod assemble; pub use assemble::{assign_palette, render_named_region, render_window};` in `lib.rs`.

- [ ] **Step 4: Run tests, verify pass; full core test + lint.**

Run: `cargo test -p rustline-core && cargo clippy --all-targets -- -D warnings`

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && git add -A
git commit -m "feat(core): region assembly, palette, builtins registry, panic guard"
```

---

### Task 10: CLI front-end (`rustline` bin)

**Files:**
- Create: `crates/rustline/src/cli.rs`, `crates/rustline/src/build_context.rs`, `crates/rustline/src/tmux_conf.rs`
- Modify: `crates/rustline/src/main.rs`
- Test: inline in `tmux_conf.rs` and `build_context.rs`; a smoke test for a region render.

**Interfaces:**
- Consumes: `rustline_core::{Config, Context, WindowCtx, Direction, Registry, render_named_region, render_window}`.
- Produces the `rustline` binary with subcommands (spec §6):
  - `render left|right [--session S --window W --pane P --pane-path PATH]`
  - `render window [--current] <index> <name> <flags>`
  - `init`, `print-config`

- [ ] **Step 1: Failing tests**

`tmux_conf.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn init_block_wires_all_regions_and_hooks() {
        let b = init_block();
        assert!(b.contains("status-interval 1"));
        assert!(b.contains("#(rustline render left"));
        assert!(b.contains("#(rustline render right"));
        assert!(b.contains("rustline render window"));
        assert!(b.contains("after-select-pane"));
        assert!(b.contains("refresh-client -S"));
    }
}
```

`build_context.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn home_from_env_used_when_present() {
        // build_context reads $HOME; assert the field is populated non-empty
        let ctx = build_region_context(&RegionArgs::default());
        assert!(!ctx.home.is_empty() || std::env::var("HOME").is_err());
    }
}
```

- [ ] **Step 2: Run, verify fail.** `cargo test -p rustline`

- [ ] **Step 3: Implement**

`cli.rs` (clap derive):
```rust
use clap::{Parser, Subcommand, Args};

#[derive(Parser)]
#[command(version, about = "Rust tmux statusline")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Render a region or a single window segment.
    #[command(subcommand)]
    Render(Render),
    /// Print the tmux.conf block to enable rustline.
    Init,
    /// Print the effective config as TOML.
    PrintConfig,
}

#[derive(Subcommand)]
pub enum Render {
    Left(RegionArgs),
    Right(RegionArgs),
    Window(WindowArgs),
}

#[derive(Args, Default)]
pub struct RegionArgs {
    #[arg(long)] pub session: Option<String>,
    #[arg(long)] pub window: Option<String>,
    #[arg(long)] pub pane: Option<String>,
    #[arg(long)] pub pane_path: Option<String>,
}

#[derive(Args)]
pub struct WindowArgs {
    #[arg(long)] pub current: bool,
    pub index: String,
    pub name: String,
    pub flags: String,
}
```

`build_context.rs`:
```rust
use crate::cli::{RegionArgs, WindowArgs};
use rustline_core::{Context, WindowCtx};

fn read_loadavg() -> Option<[f64; 3]> {
    let mut out = [0f64; 3];
    // SAFETY: getloadavg writes up to 3 doubles into `out`.
    let n = unsafe { libc::getloadavg(out.as_mut_ptr(), 3) };
    if n == 3 { Some(out) } else { None }
}

fn hostname() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

pub fn build_region_context(a: &RegionArgs) -> Context {
    Context {
        session_name: a.session.clone().unwrap_or_default(),
        window_index: a.window.clone().unwrap_or_default(),
        pane_index: a.pane.clone().unwrap_or_default(),
        pane_current_path: a.pane_path.clone().unwrap_or_default(),
        home: std::env::var("HOME").unwrap_or_default(),
        hostname: hostname(),
        loadavg: read_loadavg(),
        now: chrono::Local::now(),
        window: None,
    }
}

pub fn build_window_context(a: &WindowArgs) -> Context {
    let mut ctx = build_region_context(&RegionArgs::default());
    ctx.window = Some(WindowCtx {
        index: a.index.clone(), name: a.name.clone(),
        flags: a.flags.clone(), is_current: a.current,
    });
    ctx
}
```

`tmux_conf.rs`:
```rust
pub fn init_block() -> String {
    r#"# rustline statusline
set -g status on
set -g status-interval 1
set -g status-left-length 100
set -g status-right-length 200
set -g status-left  "#(rustline render left --session '#{session_name}' --window '#{window_index}' --pane '#{pane_index}' --pane-path '#{pane_current_path}')"
set -g status-right "#(rustline render right --pane-path '#{pane_current_path}')"
set -g window-status-separator ""
setw -g window-status-format         "#(rustline render window '#{window_index}' '#{window_name}' '#{window_flags}')"
setw -g window-status-current-format "#(rustline render window --current '#{window_index}' '#{window_name}' '#{window_flags}')"
set-hook -g after-select-pane   "refresh-client -S"
set-hook -g after-select-window "refresh-client -S"
"#.to_string()
}
```

`main.rs`:
```rust
mod build_context;
mod cli;
mod tmux_conf;

use build_context::{build_region_context, build_window_context};
use clap::Parser;
use cli::{Cli, Command, Render};
use rustline_core::{Config, Direction, Registry, render_named_region, render_window};
use tracing_subscriber::{EnvFilter, fmt};

fn config_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"));
    base.join("rustline").join("config.toml")
}

fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    fmt().with_env_filter(filter).with_writer(std::io::stderr).init();

    let cli = Cli::parse();
    let cfg = Config::load(&config_path());
    let reg = Registry::with_builtins(&cfg);
    let theme = cfg.to_theme();

    match cli.command {
        Command::Render(Render::Left(a)) => {
            let ctx = build_region_context(&a);
            print!("{}", render_named_region(Direction::Left, &cfg.layout.left, &ctx, &reg, &theme));
        }
        Command::Render(Render::Right(a)) => {
            let ctx = build_region_context(&a);
            print!("{}", render_named_region(Direction::Right, &cfg.layout.right, &ctx, &reg, &theme));
        }
        Command::Render(Render::Window(a)) => {
            let ctx = build_window_context(&a);
            print!("{}", render_window(&ctx, &reg, &theme));
        }
        Command::Init => print!("{}", tmux_conf::init_block()),
        Command::PrintConfig => {
            match toml::to_string_pretty(&cfg) {
                Ok(s) => print!("{s}"),
                Err(e) => eprintln!("failed to serialize config: {e}"),
            }
        }
    }
}
```
> `main.rs` uses `toml` for `print-config`; add `toml = "0.9"` to `crates/rustline/Cargo.toml` dependencies (Step in this task). Ensure `render` is a subcommand group so `render left`/`render window` parse.

- [ ] **Step 4: Add smoke test for a region render (`main.rs` or a `tests/` integration test)**

```rust
// crates/rustline/tests/smoke.rs
use std::process::Command;

#[test]
fn render_left_produces_styled_output() {
    let out = Command::new(env!("CARGO_BIN_EXE_rustline"))
        .args(["render", "left", "--session", "0", "--window", "0", "--pane", "0"])
        .output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("0:0.0"), "pane id present: {s}");
    assert!(s.contains("#["), "styled: {s}");
}

#[test]
fn init_prints_block() {
    let out = Command::new(env!("CARGO_BIN_EXE_rustline")).arg("init").output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("status-interval 1"));
}
```

- [ ] **Step 5: Run all tests, lint, fmt**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --all --check`
Expected: all PASS, clean.

- [ ] **Step 6: Commit (include Cargo.lock)**

```bash
cargo fmt --all && git add -A
git commit -m "feat(cli): rustline render/init/print-config front-end + tmux glue"
```

---

### Task 11: README + docs

**Files:**
- Modify: `README.md`

**Interfaces:** none (docs only).

- [ ] **Step 1: Write `README.md`** covering: what rustline is; install (`cargo build --release`, copy `target/release/rustline` onto `PATH`); enable (`rustline "$(rustline init)"` → actually `rustline init >> ~/.tmux.conf` then `tmux source-file ~/.tmux.conf`); the default widgets; config location `~/.config/rustline/config.toml` with an example; note that WASM plugins (keyed by `owner/repo`) are planned, not yet implemented; link to the spec at `docs/superpowers/specs/2026-07-20-rustline-tmux-statusline-design.md`.

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: README with install, enable, and config usage"
```

---

## Post-implementation (orchestrator, not a subagent task)

- Final code review across the branch (spec-compliance + quality).
- **Manual verification (spec §7):** `cargo build --release`, then in the live tmux: `./target/release/rustline init >> ~/.tmux.conf` (or source into current session), `tmux source-file`, confirm the powerline bar shows pane id, hostname, window list, cwd, loadavg, date, and updates on pane/window switch.
- `superpowers:finishing-a-development-branch`.

---

## Self-Review

**Spec coverage:**
- §2 invocation / init block → Task 10 (`tmux_conf`, hooks, per-region shell-out). ✅
- §3 workspace layout → Task 1. ✅
- §4.1 Context/WindowCtx → Task 2. ✅
- §4.2 Segment/Style/Color → Task 2. ✅
- §4.3 Widget/Registry + unknown-skip → Task 4; builtins → Task 9. ✅
- §4.4 powerline renderer → Task 3. ✅
- §4.5 config (defaults, total load, plugins table) → Task 8. ✅
- §5 six widgets → Tasks 5–7. ✅
- §6 CLI → Task 10. ✅
- §7 tests → each task's TDD steps + Task 10 smoke; manual verification noted post-impl. ✅
- §8 degradation (empty widget, panic guard, config fallback, loadavg None) → Tasks 8, 9 (panic guard), 5 (loadavg None). ✅
- §9 invariants: Context-only input (widgets take `&Context` only), serde ABI (Task 2 round-trip test), total config load (Task 8), tmux `#{}` expansion (Task 10 init block). ✅

**Placeholder scan:** no TBD/TODO; all code steps show code. The only intentional "extend as needed" is the `Direction::Right` edge mirroring in Task 3 — its tests pin the required behavior.

**Type consistency:** `render_named_region`, `render_window`, `render_region`, `Registry::with_builtins`, `Config::load`/`to_theme`, `Context` fields (incl. `home`, `loadavg: Option`) used consistently across Tasks 2, 3, 8, 9, 10. `Segment::new`/`styled` and `Style { fg, bg, bold }` consistent. Widget struct names (`PaneId`, `Hostname`, `Cwd`, `LoadAvg`, `DateTime`, `Windows`) consistent between Tasks 5–7 and 9.
