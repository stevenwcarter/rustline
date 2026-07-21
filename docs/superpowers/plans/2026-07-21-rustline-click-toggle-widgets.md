# Click-to-toggle widget views Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Left-click a status-line widget to swap it between its `format` and an `alt_format`, remembered in a global state file, with rustline staying a pure per-region shell-out.

**Architecture:** Toggle state rides in `Context.toggled` (a `BTreeSet<String>`), read once at the Context-build edge. A widget's `render` picks `alt_format` vs `format` via a shared `active_format` helper keyed on its own name. The assemble/render layer wraps each clickable widget's cells in tmux `#[range=user|NAME]…#[norange]` markup; `rustline init` binds `MouseDown1Status` to `rustline click --range=…`, which flips that name in the state file, then `refresh-client -S`. Plugins receive `Context.toggled` through the existing `RenderInput` serialization and opt in by checking their own name.

**Tech Stack:** Rust (edition 2024), workspace crates `rustline-core` / `rustline-abi` / `rustline` / `rustline-wasm`, `clap` derive CLI, `serde`/`serde_json`, `toml`, `tracing`, `extism` host, `plugins/weather` (wasm32).

## Global Constraints

- **Edition 2024** in every crate; `rustfmt.toml` is edition 2024 — keep all crate editions equal.
- Must stay **clippy-clean** (`cargo clippy --all-targets -- -D warnings`) and **rustfmt-clean** (`cargo fmt --all --check`). No pre-commit hook — run `cargo fmt --all` before each commit.
- `just test` is **hermetic** (no wasm toolchain). Wasm/e2e work is gated behind the opt-in `wasm-e2e` feature / `just test-wasm`; never make `just test` require the wasm target.
- **Invariant #1:** widgets read only from `Context`, never the environment mid-render. `toggled` is read at the `build_context` edge.
- **Invariant #2:** `Segment`/`Style`/`Color` (in `rustline-abi`) and `Context`/`WindowCtx` stay serde-serializable. **Do NOT add a `range` field to the ABI `Segment`** — range grouping is host-side render metadata.
- **Invariant #3:** `Config::load` is total — every new config field is `#[serde(default)]`.
- **Invariant #4:** `init` output is injection-safe — interpolated tmux vars use `#{q:…}` and `--flag=value` form.
- **Invariant #6:** a failed/absent read renders nothing, never fake values.
- **tmux `range=user|X` caps `X` at 15 bytes.** A widget/plugin name longer than 15 bytes must degrade to not-clickable (`range_name()` → `None`), never emit an over-long range.
- **The NAME identity is load-bearing:** the range name emitted by render, `#{mouse_status_range}`, the `--range` value, the `Context.toggled` key, and each widget's `active_format`/`range_name` key are all the same layout/registry name string.
- **Commit `Cargo.lock`** alongside any dependency change (this plan adds no deps).

---

### Task 1: `Context.toggled` field

**Files:**
- Modify: `crates/rustline-core/src/context.rs` (add field + import + serde test)
- Modify: every `Context { … }` struct-literal site so the workspace compiles (all `#[cfg(test)]` `ctx()` helpers in `rustline-core`, plus `crates/rustline/src/build_context.rs`, plus any test in `crates/rustline/tests/` and `crates/rustline-wasm`).

**Interfaces:**
- Produces: `Context.toggled: std::collections::BTreeSet<String>` (`#[serde(default)]`). Later tasks read it via `ctx.toggled.contains(name)` and populate it in `build_context`.

- [ ] **Step 1: Write the failing test** — in `context.rs` `mod tests`, add:

```rust
#[test]
fn context_toggled_survives_serde_and_defaults_empty() {
    let mut ctx = sample();
    ctx.toggled = std::collections::BTreeSet::from(["cpu".to_string()]);
    let json = serde_json::to_string(&ctx).unwrap();
    let back: Context = serde_json::from_str(&json).unwrap();
    assert!(back.toggled.contains("cpu"));

    // A Context JSON lacking `toggled` must deserialize to an empty set
    // (guards host/guest version skew; keeps deserialization total).
    let without = json.replace(r#","toggled":["cpu"]"#, "");
    assert_ne!(without, json, "sanity: the toggled key was present to strip");
    let back2: Context = serde_json::from_str(&without).unwrap();
    assert!(back2.toggled.is_empty());
}
```

- [ ] **Step 2: Run it, expect a COMPILE failure** (no `toggled` field yet)

Run: `cargo test -p rustline-core --lib context:: 2>&1 | head -30`
Expected: FAIL — `no field toggled on type Context` (and the crate's other `Context {…}` literals also fail to compile).

- [ ] **Step 3: Add the field and fix all construction sites**

In `context.rs`, add the import near the top:

```rust
use std::collections::BTreeSet;
```

Add the field to `struct Context` (after `arch`, or logically after `window`):

```rust
    /// Widgets the user has click-toggled to their `alt_format` view. Read once
    /// at Context-build time from the toggles state file (invariant #1). Keyed by
    /// widget/plugin name; also serialized to WASM guests so a plugin can honor
    /// toggling by checking its own name.
    #[serde(default)]
    pub toggled: BTreeSet<String>,
```

Then make the crate compile: run `cargo build --workspace --all-targets 2>&1 | grep "missing field"` to list every `Context {…}` literal, and add `toggled: BTreeSet::new(),` (import `std::collections::BTreeSet` in each test module, or write `toggled: Default::default(),` to avoid an import) to each. In `context.rs`'s own `sample()` add `toggled: BTreeSet::new(),`. In `crates/rustline/src/build_context.rs::build_region_context`, add `toggled: std::collections::BTreeSet::new(),` (Task 9 replaces this with the real read).

- [ ] **Step 4: Run the workspace tests, expect PASS**

Run: `cargo test --workspace 2>&1 | tail -20`
Expected: PASS (all crates compile; the new serde test passes).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add -A && git commit -m "feat(core): add Context.toggled (serde-default set of toggled widget names)

Claude-Session: https://claude.ai/code/session_01Fddrk6VgQAPGvRg71F2eB1"
```

---

### Task 2: `active_format` helper + `Widget::range_name`

**Files:**
- Create: `crates/rustline-core/src/widgets/toggle.rs`
- Modify: `crates/rustline-core/src/widgets/mod.rs` (add `mod toggle;` and re-export helpers `pub(crate)`)
- Modify: `crates/rustline-core/src/widget.rs` (add `range_name` default method + test)

**Interfaces:**
- Produces:
  - `pub(crate) fn active_format<'a>(ctx: &Context, name: &str, format: &'a str, alt: &'a str) -> &'a str`
  - `pub(crate) fn clickable_range<'a>(name: &'a str, alt: &str) -> Option<&'a str>`
  - `Widget::range_name(&self) -> Option<&str>` (default `None`)

- [ ] **Step 1: Write the failing test** — create `crates/rustline-core/src/widgets/toggle.rs`:

```rust
//! Shared click-toggle helpers: which format string is active given the
//! toggle set, and whether a widget is a clickable range.

use crate::Context;

/// The active format string for a widget: its `alt` view when the widget has a
/// non-empty `alt` AND its `name` is in `ctx.toggled`, else its normal `format`.
pub(crate) fn active_format<'a>(ctx: &Context, name: &str, format: &'a str, alt: &'a str) -> &'a str {
    if !alt.is_empty() && ctx.toggled.contains(name) {
        alt
    } else {
        format
    }
}

/// A widget's clickable range name: `Some(name)` when it has a non-empty `alt`
/// view AND `name` fits tmux's 15-byte `range=user|X` limit; else `None` (the
/// widget is not clickable and emits no range markup).
pub(crate) fn clickable_range<'a>(name: &'a str, alt: &str) -> Option<&'a str> {
    if !alt.is_empty() && name.len() <= 15 {
        Some(name)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};
    use std::collections::BTreeSet;

    fn ctx(toggled: &[&str]) -> Context {
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
            memory: None,
            os: String::new(),
            arch: String::new(),
            toggled: toggled.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>(),
        }
    }

    #[test]
    fn active_format_picks_alt_only_when_toggled_and_alt_nonempty() {
        assert_eq!(active_format(&ctx(&["cpu"]), "cpu", "F", "A"), "A");
        assert_eq!(active_format(&ctx(&[]), "cpu", "F", "A"), "F"); // not toggled
        assert_eq!(active_format(&ctx(&["cpu"]), "cpu", "F", ""), "F"); // empty alt
        assert_eq!(active_format(&ctx(&["mem"]), "cpu", "F", "A"), "F"); // other toggled
    }

    #[test]
    fn clickable_range_requires_alt_and_fits_15_bytes() {
        assert_eq!(clickable_range("cpu", "A"), Some("cpu"));
        assert_eq!(clickable_range("cpu", ""), None); // no alt -> not clickable
        assert_eq!(clickable_range("this_name_is_16b", "A"), None); // 16 bytes > 15
        assert_eq!(clickable_range("fifteen_bytes__", "A"), Some("fifteen_bytes__")); // exactly 15
    }
}
```

- [ ] **Step 2: Wire the module + trait method, run to verify test fails first**

In `crates/rustline-core/src/widgets/mod.rs` add near the other `mod` lines:

```rust
mod toggle;
pub(crate) use toggle::{active_format, clickable_range};
```

In `crates/rustline-core/src/widget.rs`, add to the `Widget` trait (below `render`):

```rust
    /// The clickable status-line range name for this widget, if it opts into
    /// click-to-toggle. Default `None` (not clickable). A widget returns
    /// `Some(name)` only when it has an alternate view and `name` fits tmux's
    /// 15-byte `range=user|X` limit; the assemble layer wraps its cells in
    /// `#[range=user|<name>]…#[norange]` when so.
    fn range_name(&self) -> Option<&str> {
        None
    }
```

Run: `cargo test -p rustline-core --lib widgets::toggle 2>&1 | tail -15`
Expected: PASS (the helpers are defined). Then verify the trait default:

- [ ] **Step 3: Add the trait-default test** in `widget.rs` `mod tests`:

```rust
#[test]
fn range_name_defaults_to_none() {
    assert_eq!(Fixed("x").range_name(), None);
}
```

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p rustline-core --lib widget:: 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add -A && git commit -m "feat(core): active_format helper + Widget::range_name seam

Claude-Session: https://claude.ai/code/session_01Fddrk6VgQAPGvRg71F2eB1"
```

---

### Task 3: Range-aware region render

**Files:**
- Modify: `crates/rustline-core/src/render.rs` (add `RangeGroup` + `render_region_ranged` + tests)
- Modify: `crates/rustline-core/src/assemble.rs` (rewrite `render_named_region` to group + wrap; test)

**Interfaces:**
- Consumes: `Widget::range_name` (Task 2), `assign_palette` (existing).
- Produces:
  - `pub struct RangeGroup { pub range: Option<String>, pub segments: Vec<Segment> }`
  - `pub fn render_region_ranged(dir: Direction, groups: &[RangeGroup], theme: &Theme) -> String`
  - `render_named_region` now wraps each clickable widget's cells in range markup.

- [ ] **Step 1: Write the failing tests** in `render.rs` `mod tests`:

```rust
fn group(range: Option<&str>, segs: Vec<Segment>) -> RangeGroup {
    RangeGroup { range: range.map(str::to_string), segments: segs }
}

#[test]
fn ranged_wraps_clickable_group_and_leaves_separators_outside() {
    let groups = vec![
        group(Some("cpu"), vec![seg("a", 31)]),
        group(None, vec![seg("b", 238)]),
    ];
    let out = render_region_ranged(Direction::Left, &groups, &theme());
    // clickable group is bracketed; non-clickable group is not.
    assert!(out.contains("#[range=user|cpu]"), "opens range: {out}");
    assert!(out.contains("#[norange]"), "closes range: {out}");
    // the hard separator between the two groups sits OUTSIDE the range
    // (norange precedes the separator glyph).
    let sep = "#[fg=colour31,bg=colour238]\u{e0b0}";
    let nr = out.find("#[norange]").unwrap();
    let sp = out.find(sep).unwrap();
    assert!(nr < sp, "norange before separator: {out}");
}

#[test]
fn ranged_all_none_is_byte_identical_to_render_region() {
    let segs = vec![seg("a", 31), seg("b", 238)];
    let groups = vec![
        group(None, vec![segs[0].clone()]),
        group(None, vec![segs[1].clone()]),
    ];
    assert_eq!(
        render_region_ranged(Direction::Left, &groups, &theme()),
        render_region(Direction::Left, &segs, &theme()),
    );
}

#[test]
fn ranged_stripping_range_tokens_equals_render_region() {
    // Non-destructive: with a clickable group, stripping the range tokens
    // reproduces the plain powerline output.
    let segs = vec![seg("a", 31), seg("b", 238)];
    let groups = vec![
        group(Some("cpu"), vec![segs[0].clone()]),
        group(Some("memory"), vec![segs[1].clone()]),
    ];
    let ranged = render_region_ranged(Direction::Left, &groups, &theme());
    let stripped = ranged
        .replace("#[range=user|cpu]", "")
        .replace("#[range=user|memory]", "")
        .replace("#[norange]", "");
    assert_eq!(stripped, render_region(Direction::Left, &segs, &theme()));
}

#[test]
fn ranged_empty_is_empty() {
    assert_eq!(render_region_ranged(Direction::Left, &[], &theme()), "");
}
```

- [ ] **Step 2: Run, expect FAIL** (`render_region_ranged` undefined)

Run: `cargo test -p rustline-core --lib render::tests::ranged 2>&1 | head -20`
Expected: FAIL — cannot find `render_region_ranged` / `RangeGroup`.

- [ ] **Step 3: Implement `RangeGroup` + `render_region_ranged`** in `render.rs` (after `render_region`):

```rust
/// A widget's rendered segments plus its optional clickable range name. The
/// assemble layer builds these so `render_region_ranged` can bracket clickable
/// widgets in `#[range=user|NAME]…#[norange]` while keeping every other byte of
/// output identical to `render_region`.
pub struct RangeGroup {
    pub range: Option<String>,
    pub segments: Vec<Segment>,
}

/// Like `render_region`, but bracket each group whose `range` is `Some(name)` in
/// `#[range=user|name]…#[norange]`. Inter-widget separators and both outer edge
/// glyphs are emitted OUTSIDE any range. With every group's `range == None` the
/// output is byte-identical to `render_region` over the flattened segments.
pub fn render_region_ranged(dir: Direction, groups: &[RangeGroup], theme: &Theme) -> String {
    let flat: Vec<&Segment> = groups.iter().flat_map(|g| g.segments.iter()).collect();
    let (Some(&first), Some(&last)) = (flat.first(), flat.last()) else {
        return String::new();
    };

    let mut out = String::new();
    let first_bg = eff_bg(first, theme);
    if first_bg != &theme.bar_bg {
        write_hard(&mut out, theme, dir, &theme.bar_bg, first_bg);
    }

    let mut prev_bg: Option<&Color> = None;
    let mut open_range = false;
    for group in groups {
        for (i, s) in group.segments.iter().enumerate() {
            let cur_bg = eff_bg(s, theme);
            if let Some(prev_bg) = prev_bg {
                // At a group boundary, close the previous range BEFORE the
                // separator so the separator glyph is not clickable.
                if i == 0 && open_range {
                    out.push_str("#[norange]");
                    open_range = false;
                }
                if prev_bg != cur_bg {
                    write_hard(&mut out, theme, dir, prev_bg, cur_bg);
                } else {
                    let _ = write!(
                        out,
                        "#[fg={},bg={}]{}",
                        theme.soft_fg.to_tmux(),
                        cur_bg.to_tmux(),
                        theme.soft(dir),
                    );
                }
            }
            if i == 0 && let Some(name) = &group.range {
                let _ = write!(out, "#[range=user|{name}]");
                open_range = true;
            }
            let bold = if s.style.bold { ",bold" } else { "" };
            let _ = write!(
                out,
                "#[fg={},bg={}{bold}] {} ",
                eff_fg(s, theme).to_tmux(),
                cur_bg.to_tmux(),
                s.text,
            );
            prev_bg = Some(cur_bg);
        }
    }
    if open_range {
        out.push_str("#[norange]");
    }

    let last_bg = eff_bg(last, theme);
    if last_bg != &theme.bar_bg {
        write_hard(&mut out, theme, dir, last_bg, &theme.bar_bg);
    }
    out.push_str("#[default]");
    out
}
```

- [ ] **Step 4: Run render tests, expect PASS**

Run: `cargo test -p rustline-core --lib render:: 2>&1 | tail -15`
Expected: PASS (new ranged tests + all existing render tests).

- [ ] **Step 5: Rewrite `render_named_region` to group + wrap** in `assemble.rs`.

Replace the body of `render_named_region` with:

```rust
pub fn render_named_region(
    dir: Direction,
    names: &[String],
    ctx: &Context,
    registry: &Registry,
    theme: &Theme,
) -> String {
    use crate::render::{RangeGroup, render_region_ranged};

    let widgets = registry.resolve(names);
    // Render each widget (panic-guarded), keeping its clickable range name.
    let rendered: Vec<(Option<String>, Vec<Segment>)> = widgets
        .iter()
        .map(|w| {
            (
                w.range_name().map(str::to_string),
                render_guarded(w.as_ref(), ctx),
            )
        })
        .collect();

    // Assign palette across the FLATTENED region (unchanged global cycling),
    // then regroup by remembered lengths so range markup can bracket each widget.
    let names: Vec<Option<String>> = rendered.iter().map(|(n, _)| n.clone()).collect();
    let lens: Vec<usize> = rendered.iter().map(|(_, s)| s.len()).collect();
    let mut flat: Vec<Segment> = rendered.into_iter().flat_map(|(_, s)| s).collect();
    assign_palette(&mut flat, theme);

    let mut it = flat.into_iter();
    let groups: Vec<RangeGroup> = names
        .into_iter()
        .zip(lens)
        .map(|(range, len)| RangeGroup {
            range,
            segments: (&mut it).take(len).collect(),
        })
        .collect();

    render_region_ranged(dir, &groups, theme)
}
```

Add an assemble-level test in `assemble.rs` `mod tests`:

```rust
#[test]
fn named_region_wraps_clickable_widget_range() {
    use crate::{Segment, Widget};
    struct Clicky;
    impl Widget for Clicky {
        fn render(&self, _c: &Context) -> Vec<Segment> {
            vec![Segment::new("hi")]
        }
        fn range_name(&self) -> Option<&str> {
            Some("clicky")
        }
    }
    let mut reg = Registry::with_builtins(&Config::default());
    reg.register("clicky", Box::new(|| Box::new(Clicky)));
    let out = render_named_region(
        Direction::Left,
        &["clicky".into(), "hostname".into()],
        &ctx(),
        &reg,
        &Theme::default(),
    );
    assert!(out.contains("#[range=user|clicky]"), "wraps clickable: {out}");
    assert!(out.contains("#[norange]"), "closes range: {out}");
    assert!(out.contains("hi"), "text present: {out}");
}
```

- [ ] **Step 6: Run, expect PASS; fmt + clippy + commit**

Run: `cargo test -p rustline-core 2>&1 | tail -15` — Expected: PASS.

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add -A && git commit -m "feat(core): range-aware region render (#[range=user|NAME] per clickable widget)

Claude-Session: https://claude.ai/code/session_01Fddrk6VgQAPGvRg71F2eB1"
```

---

### Task 4: `cpu` widget `alt_format` (exemplar)

**Files:**
- Modify: `crates/rustline-core/src/config.rs` (`CpuOpts.alt_format`)
- Modify: `crates/rustline-core/src/widgets/cpu.rs` (`alt_format` field, `NAME`, `range_name`, `active_format`)
- Modify: `crates/rustline-core/src/widgets/mod.rs` (`with_builtins` wiring for cpu)

**Interfaces:**
- Consumes: `active_format`, `clickable_range` (Task 2).
- Produces: `CpuWidget { format, alt_format, down_format, bar_width }`, `CpuWidget::NAME == "cpu"`. Pattern reused verbatim by Tasks 5–6.

- [ ] **Step 1: Write failing tests** in `cpu.rs` `mod tests`. Update the `w` helper and add cases:

```rust
fn w2(format: &str, alt: &str, down: &str) -> CpuWidget {
    CpuWidget { format: format.into(), alt_format: alt.into(), down_format: down.into(), bar_width: 8 }
}

#[test]
fn toggled_uses_alt_format() {
    let mut c = ctx(Some(CpuUsage { percent: 50.0 }));
    c.toggled.insert("cpu".to_string());
    let out = w2("{percent}%", "{icon} {bar} {percent}%", "").render(&c);
    assert_eq!(out[0].text, "\u{f061a} ████░░░░ 50%");
    // untoggled -> normal format
    let out = w2("{percent}%", "{icon} {bar} {percent}%", "").render(&ctx(Some(CpuUsage { percent: 50.0 })));
    assert_eq!(out[0].text, "50%");
}

#[test]
fn range_name_some_only_with_alt_format() {
    assert_eq!(w2("{percent}%", "{bar}", "").range_name(), Some("cpu"));
    assert_eq!(w2("{percent}%", "", "").range_name(), None);
}
```

Also update the existing `w(format, down)` helper to set `alt_format: String::new()` so prior tests still construct `CpuWidget`.

- [ ] **Step 2: Run, expect FAIL** (`no field alt_format`)

Run: `cargo test -p rustline-core --lib widgets::cpu 2>&1 | head -20`
Expected: FAIL — missing field `alt_format` / no method `range_name` returning `Some`.

- [ ] **Step 3: Implement.**

In `config.rs` `struct CpuOpts`, add after `down_format`:

```rust
    #[serde(default)]
    pub alt_format: String,
```

and in its `Default`: `alt_format: String::new(),`.

In `cpu.rs`, add the field + const + methods:

```rust
pub struct CpuWidget {
    pub format: String,
    pub alt_format: String,
    pub down_format: String,
    pub bar_width: usize,
}

impl CpuWidget {
    /// Registry/layout name; the toggle key threaded through render + click.
    pub const NAME: &'static str = "cpu";
}
```

In `impl Widget for CpuWidget`, change the `Some(c)` branch's `let text = self.format` to select via `active_format`, and add `range_name`:

```rust
impl Widget for CpuWidget {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        match ctx.cpu {
            Some(c) => {
                let percent = c.percent.round() as u64;
                let fmt = crate::widgets::active_format(ctx, Self::NAME, &self.format, &self.alt_format);
                let text = fmt
                    .replace("{percent}", &percent.to_string())
                    .replace("{bar}", &bar::gauge_bar(c.percent as f64 / 100.0, self.bar_width))
                    .replace("{icon}", CPU_ICON);
                vec![Segment::new(text)]
            }
            None => { /* unchanged down_format branch */ }
        }
    }

    fn range_name(&self) -> Option<&str> {
        crate::widgets::clickable_range(Self::NAME, &self.alt_format)
    }
}
```

In `mod.rs::with_builtins`, update the cpu factory to pass `alt_format`:

```rust
                Box::new(CpuWidget {
                    format: cpu.format.clone(),
                    alt_format: cpu.alt_format.clone(),
                    down_format: cpu.down_format.clone(),
                    bar_width: cpu.bar_width,
                })
```

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p rustline-core --lib widgets::cpu 2>&1 | tail -12`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add -A && git commit -m "feat(core): cpu widget alt_format (click-toggle exemplar)

Claude-Session: https://claude.ai/code/session_01Fddrk6VgQAPGvRg71F2eB1"
```

---

### Task 5: `memory` + `battery` `alt_format`

**Files:**
- Modify: `crates/rustline-core/src/config.rs` (`MemoryOpts.alt_format`, `BatteryOpts.alt_format`)
- Modify: `crates/rustline-core/src/widgets/memory.rs`, `crates/rustline-core/src/widgets/battery.rs`
- Modify: `crates/rustline-core/src/widgets/mod.rs` (memory + battery factories)

**Interfaces:**
- Produces: `MemoryWidget { format, alt_format, down_format, bar_width }` (`MemoryWidget::NAME == "memory"`), `BatteryWidget { format, alt_format, down_format }` (`BatteryWidget::NAME == "battery"`).

- [ ] **Step 1: Write failing tests.**

`memory.rs` `mod tests` — update the `w(format, down)` helper to add `alt_format: String::new()`, and add:

```rust
#[test]
fn memory_toggled_uses_alt_format() {
    let g = 1024u64.pow(3);
    let mut c = ctx(mem(16 * g, 8 * g, 8 * g));
    c.toggled.insert("memory".to_string());
    let out = MemoryWidget {
        format: "{percent}%".into(),
        alt_format: "{icon} {bar}".into(),
        down_format: String::new(),
        bar_width: 8,
    }.render(&c);
    assert_eq!(out[0].text, "\u{f035b} ████░░░░");
}

#[test]
fn memory_range_name_tracks_alt() {
    let base = MemoryWidget { format: "x".into(), alt_format: String::new(), down_format: String::new(), bar_width: 8 };
    assert_eq!(base.range_name(), None);
    let alt = MemoryWidget { alt_format: "{bar}".into(), ..MemoryWidget { format: "x".into(), alt_format: String::new(), down_format: String::new(), bar_width: 8 } };
    assert_eq!(alt.range_name(), Some("memory"));
}
```

`battery.rs` `mod tests` — update `w()` to add `alt_format: String::new()`, and add:

```rust
#[test]
fn battery_toggled_uses_alt_format() {
    let mut c = ctx(bat(73, BatteryState::Discharging));
    c.toggled.insert("battery".to_string());
    let out = BatteryWidget {
        format: "{percent}%".into(),
        alt_format: "{icon} {percent}% {state}".into(),
        down_format: String::new(),
    }.render(&c);
    assert_eq!(out[0].text, "\u{f0080} 73% discharging");
}

#[test]
fn battery_range_name_tracks_alt() {
    assert_eq!(
        BatteryWidget { format: "x".into(), alt_format: String::new(), down_format: String::new() }.range_name(),
        None
    );
    assert_eq!(
        BatteryWidget { format: "x".into(), alt_format: "{state}".into(), down_format: String::new() }.range_name(),
        Some("battery")
    );
}
```

- [ ] **Step 2: Run, expect FAIL**

Run: `cargo test -p rustline-core --lib "widgets::memory" "widgets::battery" 2>&1 | head -20`
Expected: FAIL — missing field `alt_format`.

- [ ] **Step 3: Implement** — apply the Task-4 pattern to both.

`config.rs`: add `#[serde(default)] pub alt_format: String,` to `MemoryOpts` and `BatteryOpts` (+ `alt_format: String::new(),` in each `Default`).

`memory.rs`: add `pub alt_format: String,` to `MemoryWidget` (after `format`); add `impl MemoryWidget { pub const NAME: &'static str = "memory"; }`; in the `Some(m)` branch replace `self.format` with `let fmt = crate::widgets::active_format(ctx, Self::NAME, &self.format, &self.alt_format);` and chain the `.replace(...)` calls on `fmt`; add `fn range_name(&self) -> Option<&str> { crate::widgets::clickable_range(Self::NAME, &self.alt_format) }`.

`battery.rs`: same shape — add `pub alt_format: String,`, `const NAME: &'static str = "battery"`, select `fmt` via `active_format` in the `Some(b)` branch, add `range_name`.

`mod.rs::with_builtins`: add `alt_format: memory.alt_format.clone(),` and `alt_format: battery.alt_format.clone(),` to the respective factories.

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p rustline-core --lib "widgets::memory" "widgets::battery" 2>&1 | tail -12`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add -A && git commit -m "feat(core): memory + battery alt_format

Claude-Session: https://claude.ai/code/session_01Fddrk6VgQAPGvRg71F2eB1"
```

---

### Task 6: `datetime` + `lan_ip` + `tailscale_ip` `alt_format`

**Files:**
- Modify: `crates/rustline-core/src/config.rs` (`DateTimeOpts`, `LanIpOpts`, `TailscaleIpOpts` — add `alt_format`)
- Modify: `crates/rustline-core/src/widgets/datetime.rs`, `lan_ip.rs`, `tailscale_ip.rs`
- Modify: `crates/rustline-core/src/widgets/mod.rs` (three factories)

**Interfaces:**
- Produces: `DateTime { format, alt_format }` (`NAME == "datetime"`), `LanIp { format, alt_format, down_format, interface }` (`NAME == "lan_ip"`), `TailscaleIp { format, alt_format, down_format }` (`NAME == "tailscale_ip"`). `lan_ip`/`tailscale_ip` pass the `active_format` result as the `format` arg to `net::render_ip`.

- [ ] **Step 1: Write failing tests.**

`datetime.rs` — add:

```rust
#[test]
fn datetime_toggled_uses_alt_format() {
    let mut c = ctx_at();
    c.toggled.insert("datetime".to_string());
    let w = DateTime { format: "%H:%M".into(), alt_format: "%Y-%m-%d %H:%M".into() };
    assert_eq!(w.render(&c)[0].text, "2026-07-20 17:49");
    // untoggled
    let w = DateTime { format: "%H:%M".into(), alt_format: "%Y-%m-%d %H:%M".into() };
    assert_eq!(w.render(&ctx_at())[0].text, "17:49");
}

#[test]
fn datetime_range_name_tracks_alt() {
    assert_eq!(DateTime { format: "%H:%M".into(), alt_format: String::new() }.range_name(), None);
    assert_eq!(DateTime { format: "%H:%M".into(), alt_format: "%c".into() }.range_name(), Some("datetime"));
}
```

Update the two existing `DateTime { format: … }` literals in `datetime.rs` tests to add `alt_format: String::new()`.

`lan_ip.rs` — update the four `LanIp { … }` literals to add `alt_format: String::new()`, and add:

```rust
#[test]
fn lan_ip_toggled_uses_alt_format() {
    let mut c = ctx(vec![ifc("eth0", "192.168.1.20")]);
    c.toggled.insert("lan_ip".to_string());
    let w = LanIp { format: "{ip}".into(), alt_format: "LAN {ip}".into(), down_format: String::new(), interface: None };
    assert_eq!(w.render(&c)[0].text, "LAN 192.168.1.20");
}
```

`tailscale_ip.rs` — update the three `TailscaleIp { … }` literals to add `alt_format: String::new()`, and add:

```rust
#[test]
fn tailscale_toggled_uses_alt_format() {
    let mut c = ctx(vec![ifc("tailscale0", "100.101.4.7")]);
    c.toggled.insert("tailscale_ip".to_string());
    let w = TailscaleIp { format: "{ip}".into(), alt_format: "TS {ip}".into(), down_format: String::new() };
    assert_eq!(w.render(&c)[0].text, "TS 100.101.4.7");
}
```

- [ ] **Step 2: Run, expect FAIL**

Run: `cargo test -p rustline-core --lib "widgets::datetime" "widgets::lan_ip" "widgets::tailscale_ip" 2>&1 | head -20`
Expected: FAIL — missing field `alt_format`.

- [ ] **Step 3: Implement.**

`config.rs`: add `#[serde(default)] pub alt_format: String,` to `DateTimeOpts`, `LanIpOpts`, `TailscaleIpOpts` (+ `alt_format: String::new(),` in each `Default`).

`datetime.rs`:
```rust
pub struct DateTime { pub format: String, pub alt_format: String }
impl DateTime { pub const NAME: &'static str = "datetime"; }
impl Default for DateTime {
    fn default() -> Self { Self { format: "%a < %Y-%m-%d < %H:%M".into(), alt_format: String::new() } }
}
impl Widget for DateTime {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let fmt = crate::widgets::active_format(ctx, Self::NAME, &self.format, &self.alt_format);
        vec![Segment::new(ctx.now.format(fmt).to_string())]
    }
    fn range_name(&self) -> Option<&str> { crate::widgets::clickable_range(Self::NAME, &self.alt_format) }
}
```

`lan_ip.rs`: add `pub alt_format: String,` to `LanIp`; `impl LanIp { pub const NAME: &'static str = "lan_ip"; }`; render:
```rust
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let ip = net::pick_lan(&ctx.interfaces, self.interface.as_deref());
        let fmt = crate::widgets::active_format(ctx, Self::NAME, &self.format, &self.alt_format);
        net::render_ip(fmt, ip, &self.down_format)
    }
    fn range_name(&self) -> Option<&str> { crate::widgets::clickable_range(Self::NAME, &self.alt_format) }
```

`tailscale_ip.rs`: add `pub alt_format: String,` to `TailscaleIp`; `impl TailscaleIp { pub const NAME: &'static str = "tailscale_ip"; }`; render mirrors `lan_ip` using `net::pick_tailscale` and `active_format`; add `range_name`.

`mod.rs::with_builtins`: add `alt_format: <opts>.alt_format.clone(),` to the `datetime`, `lan_ip`, and `tailscale_ip` factories (the `datetime` factory currently clones only `format` — clone `cfg.widgets.datetime.clone()` or add a second captured `alt`).

- [ ] **Step 4: Run full core suite, expect PASS**

Run: `cargo test -p rustline-core 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add -A && git commit -m "feat(core): datetime + lan_ip + tailscale_ip alt_format

Claude-Session: https://claude.ai/code/session_01Fddrk6VgQAPGvRg71F2eB1"
```

---

### Task 7: `toggles.rs` state module (binary)

**Files:**
- Create: `crates/rustline/src/toggles.rs`
- Modify: `crates/rustline/src/main.rs` (add `mod toggles;`)
- Modify: `crates/rustline-wasm/src/lib.rs` (re-export `data_root` if not already: `pub use paths::data_root;`)

**Interfaces:**
- Produces:
  - `pub fn parse_toggles(contents: &str) -> BTreeSet<String>`
  - `pub fn serialize_toggles(set: &BTreeSet<String>) -> String`
  - `pub fn apply_toggle(set: &mut BTreeSet<String>, name: &str)`
  - `pub fn read_toggles() -> BTreeSet<String>` (IO error → empty)
  - `pub fn write_toggles(set: &BTreeSet<String>)` (best-effort, atomic)
  - `pub fn toggles_path() -> PathBuf` → `$XDG_DATA_HOME/rustline/toggles`

- [ ] **Step 1: Write failing tests** — create `crates/rustline/src/toggles.rs`:

```rust
//! The global click-toggle state file: which widgets are currently showing
//! their `alt_format`. Read once at Context-build time, written by `rustline
//! click`. Newline-delimited widget names under `$XDG_DATA_HOME/rustline/toggles`.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// Path to the toggles state file (reuses the wasm crate's XDG data-root resolver
/// so there is one base dir: `$XDG_DATA_HOME/rustline`, fallback
/// `~/.local/share/rustline`).
pub fn toggles_path() -> PathBuf {
    rustline_wasm::data_root().join("toggles")
}

/// Parse newline-delimited names into a set. Total: trims each line, drops
/// blanks; any malformed/partial content simply yields fewer names.
pub fn parse_toggles(contents: &str) -> BTreeSet<String> {
    contents
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Serialize a set to sorted, newline-delimited text (trailing newline).
pub fn serialize_toggles(set: &BTreeSet<String>) -> String {
    let mut s = String::new();
    for name in set {
        s.push_str(name);
        s.push('\n');
    }
    s
}

/// Flip `name`'s membership.
pub fn apply_toggle(set: &mut BTreeSet<String>, name: &str) {
    if !set.remove(name) {
        set.insert(name.to_string());
    }
}

/// Read the toggle set; a missing/unreadable file yields an empty set.
pub fn read_toggles() -> BTreeSet<String> {
    match std::fs::read_to_string(toggles_path()) {
        Ok(text) => parse_toggles(&text),
        Err(_) => BTreeSet::new(),
    }
}

/// Best-effort atomic write (temp file + rename); logs a warning on failure and
/// never panics — a broken toggle must never break the bar.
pub fn write_toggles(set: &BTreeSet<String>) {
    let path = toggles_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("tmp");
    if let Err(error) = std::fs::write(&tmp, serialize_toggles(set)) {
        tracing::warn!(%error, "failed to write toggles temp file");
        return;
    }
    if let Err(error) = std::fs::rename(&tmp, &path) {
        tracing::warn!(%error, "failed to rename toggles file");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_is_total_over_blanks_and_whitespace() {
        let set = parse_toggles("cpu\n\n  memory  \n\n");
        assert_eq!(set, BTreeSet::from(["cpu".to_string(), "memory".to_string()]));
        assert!(parse_toggles("").is_empty());
    }

    #[test]
    fn parse_serialize_round_trips() {
        let set = BTreeSet::from(["battery".to_string(), "cpu".to_string()]);
        assert_eq!(parse_toggles(&serialize_toggles(&set)), set);
    }

    #[test]
    fn apply_toggle_flips_membership() {
        let mut set = BTreeSet::new();
        apply_toggle(&mut set, "cpu");
        assert!(set.contains("cpu"));
        apply_toggle(&mut set, "cpu");
        assert!(!set.contains("cpu"));
    }
}
```

- [ ] **Step 2: Wire the module + re-export; run to verify the FAIL→PASS**

In `crates/rustline/src/main.rs` add `mod toggles;` with the other `mod` lines.
In `crates/rustline-wasm/src/lib.rs`, confirm `data_root` is re-exported at crate root; if only `default_plugin_dir`/`expand_tilde` are, add `pub use paths::data_root;` (or widen the existing `pub use paths::{…};`).

Run: `cargo test -p rustline --lib toggles:: 2>&1 | tail -12`
Expected: PASS.

- [ ] **Step 3: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add -A && git commit -m "feat(rustline): toggles state module (global click-toggle set)

Claude-Session: https://claude.ai/code/session_01Fddrk6VgQAPGvRg71F2eB1"
```

---

### Task 8: `rustline click` subcommand

**Files:**
- Modify: `crates/rustline/src/cli.rs` (add `Click(ClickArgs)` + `ClickArgs`)
- Modify: `crates/rustline/src/main.rs` (dispatch + `run_click`)

**Interfaces:**
- Consumes: `toggles::{read_toggles, apply_toggle, write_toggles}` (Task 7).
- Produces: `Command::Click(ClickArgs { range: String, button: String })`; `fn run_click(args: &ClickArgs)`. The `(range, button)` action resolution is the single choke point future `left_click`/`right_click` script handlers extend.

- [ ] **Step 1: Write the failing test** — in `main.rs` add `run_click` and a unit test of the pure toggle effect (drive it through the `toggles` helpers so no real FS is needed):

In `cli.rs`, add to `enum Command`:

```rust
    /// Toggle a widget's alt view (invoked by the tmux MouseDown1Status binding).
    Click(ClickArgs),
```

and:

```rust
/// Arguments for `rustline click`, sourced from the tmux mouse binding.
#[derive(Args)]
pub struct ClickArgs {
    /// The clicked widget's range name (tmux `#{mouse_status_range}`); empty = no-op.
    #[arg(long, default_value = "")]
    pub range: String,
    /// Which mouse button (currently only `left` acts; others are reserved).
    #[arg(long, default_value = "left")]
    pub button: String,
}
```

In `main.rs`, add:

```rust
/// Handle `rustline click`: on a left-click with a non-empty range, flip that
/// widget's membership in the toggle state file. Any other button, or an empty
/// range, is a no-op. Never fails the process (invariant: never break the bar).
fn run_click(args: &cli::ClickArgs) {
    if args.button != "left" || args.range.is_empty() {
        return;
    }
    let mut set = toggles::read_toggles();
    toggles::apply_toggle(&mut set, &args.range);
    toggles::write_toggles(&set);
}
```

and add a `#[cfg(test)] mod tests` in `main.rs` (or extend one) — since `read/write_toggles` hit a real path, test the resolver logic via the pure helpers instead:

```rust
#[cfg(test)]
mod tests {
    use rustline::_never(); // placeholder — see note
}
```

> Note: `main.rs` has no test module today and the binary crate is awkward to unit-test. Put the *logic* test where it belongs — in Task 7's `toggles.rs` (already covers `apply_toggle`). For `run_click`, assert behavior in the integration smoke test instead (next step), which can point `$XDG_DATA_HOME` at a temp dir.

- [ ] **Step 2: Add an integration test** in `crates/rustline/tests/smoke.rs`:

```rust
#[test]
fn click_toggles_state_file() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = assert_cmd::Command::cargo_bin("rustline").unwrap();
    cmd.env("XDG_DATA_HOME", tmp.path())
        .args(["click", "--range=cpu", "--button=left"])
        .assert()
        .success();
    let toggles = std::fs::read_to_string(tmp.path().join("rustline/toggles")).unwrap();
    assert!(toggles.contains("cpu"), "cpu toggled on: {toggles:?}");

    // second click toggles off
    assert_cmd::Command::cargo_bin("rustline").unwrap()
        .env("XDG_DATA_HOME", tmp.path())
        .args(["click", "--range=cpu", "--button=left"])
        .assert()
        .success();
    let toggles = std::fs::read_to_string(tmp.path().join("rustline/toggles")).unwrap_or_default();
    assert!(!toggles.contains("cpu"), "cpu toggled off: {toggles:?}");
}
```

(Confirm `assert_cmd` + `tempfile` are already dev-deps of `crates/rustline` — the existing `smoke.rs` uses them; if not, add them under `[dev-dependencies]` and commit `Cargo.lock`.)

- [ ] **Step 3: Run, expect FAIL then wire dispatch**

Run: `cargo test -p rustline --test smoke click_toggles 2>&1 | head -20`
Expected: FAIL (no `click` subcommand). Then in `main.rs` `match cli.command` add:

```rust
        Command::Click(args) => run_click(&args),
```

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p rustline --test smoke click_toggles 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add -A && git commit -m "feat(rustline): rustline click subcommand (flip a widget's toggle)

Claude-Session: https://claude.ai/code/session_01Fddrk6VgQAPGvRg71F2eB1"
```

---

### Task 9: Wire `Context.toggled` in `build_context`

**Files:**
- Modify: `crates/rustline/src/build_context.rs`

**Interfaces:**
- Consumes: `toggles::read_toggles` (Task 7).

- [ ] **Step 1: Write the failing test** in `build_context.rs` `mod tests`:

```rust
#[test]
fn build_region_context_reads_toggles_from_state_file() {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: single-threaded test process; set the data-root env for read_toggles.
    unsafe { std::env::set_var("XDG_DATA_HOME", tmp.path()); }
    std::fs::create_dir_all(tmp.path().join("rustline")).unwrap();
    std::fs::write(tmp.path().join("rustline/toggles"), "cpu\nmemory\n").unwrap();
    let ctx = build_region_context(&RegionArgs::default(), &[]);
    assert!(ctx.toggled.contains("cpu") && ctx.toggled.contains("memory"));
    unsafe { std::env::remove_var("XDG_DATA_HOME"); }
}
```

(If `tempfile` is not a dev-dep of `crates/rustline`, it is via the existing smoke tests; add if needed.)

- [ ] **Step 2: Run, expect FAIL** (field still `BTreeSet::new()`)

Run: `cargo test -p rustline --lib build_context 2>&1 | head -20`
Expected: FAIL — `toggled` empty.

- [ ] **Step 3: Implement** — in `build_region_context`, replace the placeholder line from Task 1 with:

```rust
        toggled: crate::toggles::read_toggles(),
```

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p rustline --lib build_context 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add -A && git commit -m "feat(rustline): read toggles into Context.toggled at build edge

Claude-Session: https://claude.ai/code/session_01Fddrk6VgQAPGvRg71F2eB1"
```

---

### Task 10: `init` mouse binding

**Files:**
- Modify: `crates/rustline/src/tmux_conf.rs` (`init_block` + tests)

**Interfaces:**
- Produces: the `MouseDown1Status` binding wired to `rustline click`, injection-safe.

- [ ] **Step 1: Write the failing tests** in `tmux_conf.rs` `mod tests`:

```rust
#[test]
fn init_block_wires_click_toggle_binding() {
    let b = init_block("colour234", "colour255");
    assert!(b.contains("MouseDown1Status"), "binds status click: {b}");
    // preserves default window-click selection
    assert!(b.contains("select-window -t="), "keeps window selection: {b}");
    // dispatches to rustline click with the q-escaped range (invariant #4)
    assert!(
        b.contains("rustline click --range=#{q:mouse_status_range}"),
        "click dispatch q-escaped: {b}"
    );
    // never a bare, unescaped mouse_status_range in the click arg
    assert!(!b.contains("--range=#{mouse_status_range}"), "must q-escape: {b}");
    // discoverability hint
    assert!(b.contains("set -g mouse on"), "mentions mouse-on hint: {b}");
    assert!(b.contains("refresh-client -S"), "refreshes after toggle: {b}");
}
```

- [ ] **Step 2: Run, expect FAIL**

Run: `cargo test -p rustline --lib tmux_conf 2>&1 | head -20`
Expected: FAIL — no `MouseDown1Status`.

- [ ] **Step 3: Implement** — append to the `block` string in `init_block`, before returning:

```rust
    block.push_str(
        r##"# rustline click-to-toggle a widget's alt view (needs: set -g mouse on)
bind -T root MouseDown1Status {
    if -F "#{==:#{mouse_status_range},window}" {
        select-window -t=
    } {
        if -F "#{mouse_status_range}" {
            run-shell "rustline click --range=#{q:mouse_status_range} --button=left"
            refresh-client -S
        }
    }
}
"##,
    );
```

> The comment line contains the literal `set -g mouse on` so the hint test passes and the user learns clicks need mouse mode. `select-window -t=` reproduces tmux's default window-click behavior that our binding overrides. `#{q:mouse_status_range}` is q-escaped and passed in `--range=` form (invariant #4).

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p rustline --lib tmux_conf 2>&1 | tail -10`
Expected: PASS (new test + the two existing init tests unchanged).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add -A && git commit -m "feat(rustline): init emits MouseDown1Status click-toggle binding

Claude-Session: https://claude.ai/code/session_01Fddrk6VgQAPGvRg71F2eB1"
```

---

### Task 11: `WasmWidget::range_name` + registration guard + host-seam test

**Files:**
- Modify: `crates/rustline-wasm/src/host.rs` (`WasmWidget` gains a `name`, implements `range_name`; host-seam test)
- Modify: `crates/rustline-wasm/src/lib.rs` (`register_plugins` passes the name; one-time `warn!` when a plugin name > 15 bytes won't be clickable)

**Interfaces:**
- Consumes: `Widget::range_name` (Task 2), `Context.toggled` (Task 1), `RenderInput` (existing).
- Produces: a plugin is a clickable range (`Some(name)`) when its name ≤ 15 bytes.

- [ ] **Step 1: Write the failing tests** in `host.rs` `mod tests`:

```rust
#[test]
fn render_input_serializes_toggled_for_guests() {
    use crate::abi::RenderInput;
    use rustline_core::Context;
    // Build a minimal Context with a toggled entry and assert the guest payload
    // carries it — this is the seam a plugin depends on to honor toggling.
    let json = serde_json::to_string(&RenderInput {
        context: &sample_ctx_with_toggle("weather"),
        config: &serde_json::json!({}),
    })
    .unwrap();
    assert!(json.contains("\"toggled\""), "payload carries toggled: {json}");
    assert!(json.contains("weather"), "payload carries the toggled name: {json}");
}
```

Add a small `sample_ctx_with_toggle` helper in the test module constructing a `Context` (copy the field list from `rustline-core`'s test ctx) with `toggled: std::collections::BTreeSet::from([name.to_string()])`.

- [ ] **Step 2: Run, expect FAIL / compile error**, then implement.

Add a `name` field to `WasmWidget` and thread it through `new`:

```rust
#[derive(Clone)]
pub struct WasmWidget {
    plugin: Arc<Mutex<extism::Plugin>>,
    options: Arc<serde_json::Value>,
    name: Arc<str>,
}

impl WasmWidget {
    pub fn new(plugin: extism::Plugin, options: serde_json::Value, name: &str) -> Self {
        Self {
            plugin: Arc::new(Mutex::new(plugin)),
            options: Arc::new(options),
            name: Arc::from(name),
        }
    }
}
```

Add to `impl Widget for WasmWidget`:

```rust
    fn range_name(&self) -> Option<&str> {
        // A plugin is clickable when its name fits tmux's 15-byte user-range
        // limit; the guest decides whether to honor `context.toggled`.
        (self.name.len() <= 15).then_some(&*self.name)
    }
```

In `crates/rustline-wasm/src/lib.rs::register_plugins`, update the `WasmWidget::new(plugin, options)` call to pass the plugin's name, and after verifying the exported name, add:

```rust
    if name.len() > 15 {
        tracing::warn!(plugin = %name, "plugin name > 15 bytes; not click-toggleable");
    }
```

(Find the exact construction site with `rg "WasmWidget::new" crates/rustline-wasm`.)

- [ ] **Step 3: Run, expect PASS**

Run: `cargo test -p rustline-wasm 2>&1 | tail -15`
Expected: PASS (hermetic; no wasm target needed).

- [ ] **Step 4: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add -A && git commit -m "feat(wasm): plugins are clickable ranges; RenderInput carries toggled

Claude-Session: https://claude.ai/code/session_01Fddrk6VgQAPGvRg71F2eB1"
```

---

### Task 12: `weather` plugin honors `toggled` (demo)

**Files:**
- Modify: `plugins/weather/src/lib.rs` (pure host-tested format-selection helper + guest wiring)

**Interfaces:**
- Consumes: the guest's hand-parsed `context.toggled` JSON array + its own name `"weather"`.

- [ ] **Step 1: Write the failing test** in `plugins/weather/src/lib.rs` (host-target `#[cfg(test)]`, no wasm needed):

```rust
/// Pick the active weather format given whether this plugin is toggled and its
/// configured `alt_format` (mirrors the host's `active_format`).
pub fn select_weather_format<'a>(toggled: bool, format: &'a str, alt_format: &'a str) -> &'a str {
    if toggled && !alt_format.is_empty() { alt_format } else { format }
}

#[cfg(test)]
mod toggle_tests {
    use super::select_weather_format;
    #[test]
    fn toggled_prefers_nonempty_alt() {
        assert_eq!(select_weather_format(true, "{icon} {temp_f}", "{icon} {temp_f}°F {city}"), "{icon} {temp_f}°F {city}");
        assert_eq!(select_weather_format(false, "F", "A"), "F");
        assert_eq!(select_weather_format(true, "F", ""), "F");
    }
}
```

- [ ] **Step 2: Run the host-target unit test, expect PASS**

Run: `cd plugins/weather && cargo test select_weather_format 2>&1 | tail -10 ; cd -`
Expected: PASS (compiles pure logic on the host target).

- [ ] **Step 3: Wire the guest** — in the `#[cfg(target_arch = "wasm32")] mod guest`, when parsing the render input, compute:

```rust
// `context.toggled` is a JSON array of names; this plugin is toggled when it
// contains "weather".
let toggled = input
    .get("context")
    .and_then(|c| c.get("toggled"))
    .and_then(|t| t.as_array())
    .is_some_and(|a| a.iter().any(|v| v.as_str() == Some("weather")));
let alt_format = /* options.alt_format, default "" */;
let format = select_weather_format(toggled, &format, &alt_format);
```

Read the existing guest render body to slot this in where `format` is currently taken from options (match the existing option-parsing style). This path compiles only for wasm; verify with `just build-weather` if the wasm target is installed (opt-in — not required for `just test`).

- [ ] **Step 4: Confirm hermetic suite still green**

Run: `just test 2>&1 | tail -15`
Expected: PASS. (Optionally `just build-weather` if the wasm target is present.)

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add -A && git commit -m "feat(weather): honor Context.toggled via options.alt_format (demo)

Claude-Session: https://claude.ai/code/session_01Fddrk6VgQAPGvRg71F2eB1"
```

---

### Task 13: Documentation

**Files:**
- Modify: `CLAUDE.md`, `README.md`

**Interfaces:** none (docs only).

- [ ] **Step 1: Update `CLAUDE.md`** — reflect the feature:
  - Module map: `Context.toggled` field; `widgets/toggle.rs` (`active_format`/`clickable_range`); `Widget::range_name`; `render_region_ranged` + `RangeGroup` in `render.rs`; `render_named_region` now range-wraps; binary `toggles.rs`; `rustline click`; the `init` mouse binding.
  - Config: `alt_format` for the six format-bearing widgets (with an example); note cwd/loadavg deferred.
  - CLI: add `rustline click --range=… [--button=left]`.
  - Invariants: add the NAME-identity + 15-byte range invariants; note `Segment` unchanged.
  - Roadmap: add a "Done: click-to-toggle alt views" line; keep `left_click`/`right_click` script handlers + widget-management TUI (in `TODO.md`) as future items.
  - Design docs list: add the spec + this plan.
  - Requirements: note tmux ≥ 3.1 for click ranges.

- [ ] **Step 2: Update `README.md`** — user-facing: the `alt_format` config example, how click-to-toggle works (needs `set -g mouse on`), and the tmux ≥ 3.1 note. Keep the widget list in sync.

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md README.md
git commit -m "docs: click-to-toggle widget alt views (CLAUDE.md + README)

Claude-Session: https://claude.ai/code/session_01Fddrk6VgQAPGvRg71F2eB1"
```

---

### Task 14: Final review + finish branch

- [ ] **Step 1: Full hermetic verification**

Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && just test 2>&1 | tail -20`
Expected: clean fmt, no clippy warnings, all tests PASS.

- [ ] **Step 2: Manual preview sanity** (optional, needs a Nerd font): set an `alt_format` on `cpu` in a temp config and run `just preview` / `cargo run -- render right --preview` to eyeball the region still renders (range markup is invisible in ANSI preview — this only checks nothing regressed).

- [ ] **Step 3:** Dispatch the final code-reviewer, then invoke `superpowers:finishing-a-development-branch`.

## Self-Review (author)

- **Spec coverage:** `alt_format` config (Tasks 4–6) ✓; `Context.toggled` (Task 1) ✓; `active_format`/`range_name`/15-byte limit (Task 2) ✓; range render (Task 3) ✓; toggles file (Task 7) ✓; `rustline click` + resolver seam (Task 8) ✓; build-edge read (Task 9) ✓; init binding + window-select preservation + injection safety (Task 10) ✓; plugin clickability + host seam (Task 11) ✓; weather demo (Task 12) ✓; docs (Task 13) ✓. All five "Invariants this feature depends on" are pinned: NAME identity (Tasks 3/8/10 tests), non-destructive render (Task 3 strip test), RenderInput carries toggled (Task 11), Context total deser (Task 1), init injection-safe (Task 10).
- **Placeholder scan:** the only `/* … */` are "unchanged branch" markers pointing at existing code shown in the source files, not missing logic. The `main.rs` test-module note explicitly redirects the test to smoke.rs (not a TODO).
- **Type consistency:** `active_format(ctx, name, format, alt)`, `clickable_range(name, alt)`, `range_name() -> Option<&str>`, `RangeGroup { range, segments }`, `render_region_ranged(dir, groups, theme)`, `CpuWidget::NAME`/`MemoryWidget::NAME`/etc., `toggles::{parse_toggles, serialize_toggles, apply_toggle, read_toggles, write_toggles, toggles_path}`, `ClickArgs { range, button }` — consistent across tasks.
