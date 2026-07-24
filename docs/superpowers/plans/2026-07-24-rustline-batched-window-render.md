# Batched window render (W42) + spark-gate-over-instances (W57) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render a two-line session's whole window list in one `rustline render windows` process call instead of one spawn per window (W42), and populate `{spark}` history when a cpu/memory *instance* references `{spark}` (W57).

**Architecture:** W42 adds a `render windows` subcommand that shells out to `tmux list-windows -F` (client-side, where `$TMUX` is correct), parses it, and renders every pill wrapped in `#[range=window|IDX]` so click-to-select survives; the two-line `STATUS_FORMAT_0` calls it once. In-process (not daemon-routed). W57 moves the `{spark}` gate into a `Config` method that scans the base widget plus layout instances.

**Tech Stack:** Rust edition 2024, `std::process::Command` (tmux shell-out), existing `render_window_pill`/`Registry`/`Config` machinery. No new dependencies.

## Global Constraints

- **Edition 2024**; clippy-clean (`cargo clippy --all-targets -- -D warnings`); rustfmt-clean (`cargo fmt --all` — no pre-commit hook, run it yourself).
- **`just test` stays hermetic** — no wasm toolchain. W42's `read_windows` shell-out is NOT unit-tested (only the pure `parse_list_windows` is, matching `git.rs::parse_git_status`).
- **Injection safety (invariant #4):** the only tmux var in the new `#()` is `--session=#{q:session_name}` (q-escaped, `=`-form). Window names/flags come from rustline's own `tmux list-windows` subprocess, never through `/bin/sh`.
- **Click identity (invariant #7):** the batched output must re-emit `#[range=window|<window_index>]…#[norange]` per pill so `#{mouse_status_range}==window` → `select-window -t=` still works. The `MouseDown1Status` binding is unchanged.
- **Never break the bar:** any tmux/subprocess failure → empty output, never a panic.
- **No default-output change (W57):** a config with no `{spark}` renders byte-identically; the existing spark history tests must still pass.
- No new dependencies; no `Cargo.lock` change expected.

---

### Task 1: W42 batched-render engine (`render_windows` + `read_windows`/`parse_list_windows`)

**Files:**
- Modify: `crates/rustline-core/src/assemble.rs` (refactor `render_window`, add `render_windows`)
- Modify: `crates/rustline-core/src/lib.rs` (re-export `render_windows` alongside `render_window`)
- Create: `crates/rustline/src/windows.rs` (`read_windows` + pure `parse_list_windows`)

**Interfaces:**
- Consumes: `WindowCtx`, `Context::default()`, `Registry`, `Theme`, `render_window_pill`, `render_guarded` (all existing in `rustline-core`).
- Produces: `rustline_core::render_windows(&[WindowCtx], &Registry, &Theme) -> String`; `crate::windows::read_windows(Option<&str>) -> Vec<WindowCtx>`.

- [ ] **Step 1: Write failing core test for `render_windows`**

In `crates/rustline-core/src/assemble.rs` `mod tests`, add:

```rust
#[test]
fn render_windows_wraps_each_pill_in_window_range() {
    let reg = Registry::with_builtins(&rustline_core::Config::default());
    let theme = Theme::default();
    let windows = vec![
        WindowCtx { index: "0".into(), name: "shell".into(), flags: "*".into(), is_current: true },
        WindowCtx { index: "1".into(), name: "edit".into(), flags: "-".into(), is_current: false },
    ];
    let out = render_windows(&windows, &reg, &theme);
    // Each window is wrapped in its own window-range group closed by #[norange].
    assert!(out.contains("#[range=window|0]"), "range for window 0: {out}");
    assert!(out.contains("#[range=window|1]"), "range for window 1: {out}");
    assert_eq!(out.matches("#[norange]").count(), 2, "one norange per pill: {out}");
    // The active window's pill differs from the inactive one (accent vs gray).
    let single_active = render_windows(&windows[..1], &reg, &theme);
    let single_inactive = render_windows(&windows[1..], &reg, &theme);
    assert_ne!(single_active, single_inactive, "active vs inactive pill differ");
    // Empty slice → empty string.
    assert_eq!(render_windows(&[], &reg, &theme), "");
}
```

(Confirm the test module's imports cover `WindowCtx`, `Registry`, `Theme`; add `use` lines if the module doesn't already have them.)

- [ ] **Step 2: Run the core test to verify it fails**

Run: `cargo test -p rustline-core render_windows_wraps_each_pill_in_window_range`
Expected: FAIL to compile — `cannot find function render_windows`.

- [ ] **Step 3: Implement the `window_pill` refactor + `render_windows`**

In `crates/rustline-core/src/assemble.rs`, replace the current `render_window` with a shared private helper plus the public `render_window`/`render_windows`:

```rust
/// One window's rounded pill (no range wrapping). Shared by the single-window
/// (`render window`) and batched (`render windows`) paths.
fn window_pill(ctx: &Context, registry: &Registry, theme: &Theme) -> String {
    let widgets = registry.resolve(&["windows".to_string()]);
    let segments: Vec<Segment> = widgets
        .iter()
        .flat_map(|(_, w)| render_guarded(w.as_ref(), ctx))
        .collect();
    let Some(seg) = segments.first() else {
        return String::new();
    };
    let is_current = ctx.window.as_ref().is_some_and(|w| w.is_current);
    render_window_pill(&seg.text, is_current, theme)
}

pub fn render_window(ctx: &Context, registry: &Registry, theme: &Theme) -> String {
    window_pill(ctx, registry, theme)
}

/// Render every window's pill in one pass, each wrapped in
/// `#[range=window|<index>]…#[norange]` so tmux's built-in window-select click
/// keeps working (the batched output re-emits the same `range=window|IDX`
/// markers tmux's own `#{W:}` loop used to). Joined with no separator (pills are
/// self-contained rounded pills, matching `window-status-separator ""`).
pub fn render_windows(windows: &[WindowCtx], registry: &Registry, theme: &Theme) -> String {
    windows
        .iter()
        .map(|wc| {
            let ctx = Context { window: Some(wc.clone()), ..Context::default() };
            let pill = window_pill(&ctx, registry, theme);
            if pill.is_empty() {
                String::new()
            } else {
                format!("#[range=window|{}]{}#[norange]", wc.index, pill)
            }
        })
        .collect()
}
```

In `crates/rustline-core/src/lib.rs`, add `render_windows` to the same
`pub use assemble::{…}` (or `pub use` block) that already re-exports
`render_window`.

- [ ] **Step 4: Run the core test to verify it passes**

Run: `cargo test -p rustline-core render_windows`
Expected: PASS.

- [ ] **Step 5: Characterization — `render_window` still byte-identical**

Run: `cargo test -p rustline-core render_window`
Expected: PASS — every existing `render_window`/window-pill test still passes (the refactor to `window_pill` is behavior-preserving).

- [ ] **Step 6: Write the failing `parse_list_windows` test**

Create `crates/rustline/src/windows.rs` with the pure parser's tests first:

```rust
//! Batched window-list read: `tmux list-windows -F` behind the pure
//! `parse_list_windows`, mirroring `git.rs`'s tool-shell-out pattern.

use std::process::Command;

use rustline_core::WindowCtx;

// (read_windows added in step 8)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_list_windows_parses_fields_and_active() {
        let out = "0\t1\t*\tshell\n1\t0\t-\tmy editor\n";
        let ws = parse_list_windows(out);
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[0].index, "0");
        assert!(ws[0].is_current, "active=1 → is_current");
        assert_eq!(ws[0].flags, "*");
        assert_eq!(ws[0].name, "shell");
        assert_eq!(ws[1].name, "my editor", "name may contain spaces");
        assert!(!ws[1].is_current);
    }

    #[test]
    fn parse_list_windows_name_may_contain_tab_and_skips_malformed() {
        // splitn(4) makes the name the remainder, so a tab inside it survives.
        let out = "2\t0\t-\tna\tme\nBADLINE\n";
        let ws = parse_list_windows(out);
        assert_eq!(ws.len(), 1, "malformed line skipped: {ws:?}");
        assert_eq!(ws[0].name, "na\tme");
    }

    #[test]
    fn parse_list_windows_empty_is_empty() {
        assert!(parse_list_windows("").is_empty());
    }
}
```

Also declare the module: add `mod windows;` to `crates/rustline/src/main.rs`
(near the other `mod` lines).

- [ ] **Step 7: Run the parser test to verify it fails**

Run: `cargo test -p rustline parse_list_windows`
Expected: FAIL to compile — `cannot find function parse_list_windows`.

- [ ] **Step 8: Implement `read_windows` + `parse_list_windows`**

In `crates/rustline/src/windows.rs`, above the tests:

```rust
/// Enumerate a tmux session's windows via `tmux list-windows -F`. A `None`
/// session lists the server's current session. Empty Vec on ANY failure (tmux
/// missing, bad session, non-zero exit) — never a panic, never a fabricated
/// window (invariant: never break the bar).
pub fn read_windows(session: Option<&str>) -> Vec<WindowCtx> {
    let mut cmd = Command::new("tmux");
    cmd.arg("list-windows");
    if let Some(s) = session {
        cmd.args(["-t", s]);
    }
    // Name LAST so a tab inside it can't misalign earlier fields (splitn(4)).
    cmd.args(["-F", "#{window_index}\t#{window_active}\t#{window_flags}\t#{window_name}"]);
    let Ok(out) = cmd.output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_list_windows(&String::from_utf8_lossy(&out.stdout))
}

/// Pure parse of `list-windows -F` output. Tolerant: a line missing any of the
/// three leading tab fields is skipped; the name is the `splitn(4)` remainder.
fn parse_list_windows(s: &str) -> Vec<WindowCtx> {
    s.lines()
        .filter_map(|line| {
            let mut it = line.splitn(4, '\t');
            let index = it.next()?.to_string();
            let active = it.next()?;
            let flags = it.next()?.to_string();
            let name = it.next().unwrap_or("").to_string();
            Some(WindowCtx { index, name, flags, is_current: active == "1" })
        })
        .collect()
}
```

- [ ] **Step 9: Run the parser tests to verify they pass**

Run: `cargo test -p rustline parse_list_windows`
Expected: PASS (3 tests).

- [ ] **Step 10: fmt + clippy + commit**

Run: `cargo fmt --all && cargo clippy -p rustline-core -p rustline --all-targets -- -D warnings`
Expected: clean. (`read_windows` is unused until Task 2 wires it — if clippy flags dead_code, add a narrow `#[allow(dead_code)]` on `read_windows` with a `// wired in Task 2` comment, to be removed in Task 2. `parse_list_windows` is exercised by tests so it won't warn.)

```bash
git add crates/rustline-core/src/assemble.rs crates/rustline-core/src/lib.rs crates/rustline/src/windows.rs crates/rustline/src/main.rs
git commit -m "feat(render): batched window-render engine — render_windows + read_windows (W42)"
```

---

### Task 2: W42 wiring (`render windows` CLI + dispatch + tmux `STATUS_FORMAT_0`)

**Files:**
- Modify: `crates/rustline/src/cli.rs` (`Render::Windows` variant + `WindowsArgs`)
- Modify: `crates/rustline/src/main.rs` (dispatch arm; remove any Task-1 `#[allow(dead_code)]`)
- Modify: `crates/rustline/src/tmux_conf.rs` (`STATUS_FORMAT_0` + test update)

**Interfaces:**
- Consumes: `rustline_core::render_windows` (Task 1), `crate::windows::read_windows` (Task 1), `Registry::with_builtins`, existing `emit`.
- Produces: the `rustline render windows [--session] [--preview]` command; the batched two-line `STATUS_FORMAT_0`.

- [ ] **Step 1: Add the `Render::Windows` variant**

In `crates/rustline/src/cli.rs`, add to the `Render` enum (after `Window`):

```rust
    /// Render a whole session's window list in one call (batched; for the
    /// two-line status-format[0]). Shells out to `tmux list-windows`.
    Windows(WindowsArgs),
```

and the args struct (near `WindowArgs`):

```rust
/// Arguments for `rustline render windows` (the batched window-list render).
#[derive(Args, Default)]
pub struct WindowsArgs {
    /// tmux session to list windows for (tmux `#{session_name}`); omitted =
    /// the server's current session.
    #[arg(long)]
    pub session: Option<String>,
    /// Print the rendered list in ANSI colour instead of raw tmux markup.
    #[arg(long)]
    pub preview: bool,
}
```

- [ ] **Step 2: Add the dispatch arm + a compile/smoke check**

In `crates/rustline/src/main.rs`, add after the `Render::Window` arm:

```rust
        Command::Render(Render::Windows(args)) => {
            // In-process, NOT daemon-routed: a systemd/launchd daemon has no
            // $TMUX to run `tmux list-windows`, and the batched path is already
            // cheap (lean WindowCtxs, builtins only, no plugins). See spec.
            let registry = Registry::with_builtins(&cfg);
            let windows = crate::windows::read_windows(args.session.as_deref());
            let markup = rustline_core::render_windows(&windows, &registry, &theme);
            emit(&markup, args.preview);
        }
```

Remove the Task-1 `#[allow(dead_code)]` on `read_windows` if you added one (it's
now used).

Run (compile + no-tmux-safe smoke — if not inside tmux this prints empty, never panics):
`cargo run -q -p rustline -- render windows --preview`
Expected: compiles and runs; inside tmux it prints the batched pills, outside it prints nothing (empty) — either way exit 0, no panic.

- [ ] **Step 3: Write the failing `tmux_conf` test**

In `crates/rustline/src/tmux_conf.rs` `mod tests`, add:

```rust
#[test]
fn two_line_status_format_0_batches_window_render() {
    let mut o = one_line("colour234", "colour255");
    o.two_line = true;
    let b = init_block(&o);
    // The two-line window line is now ONE batched call, injection-safe.
    assert!(
        b.contains("#('/usr/bin/rustline' render windows --session=#{q:session_name})"),
        "batched window render wired: {b}"
    );
    // No per-window shell call remains inside the window line (the old #{W:} loop
    // called #{T:window-status-format} = #(rustline render window …) per window).
    assert!(
        !b.contains("#{W:"),
        "the #{{W:}} per-window loop is gone from STATUS_FORMAT_0: {b}"
    );
    // Injection safety: session is q-escaped, no bare window vars in a shell call.
    assert!(b.contains("--session=#{q:session_name}"), "session q-escaped: {b}");
}
```

Also UPDATE the existing `two_line_formats_contain_no_shell_calls` test: it
currently asserts `STATUS_FORMAT_0` has NO `#(`. That is no longer true — narrow
it to assert the ONLY shell call is the batched, q-escaped `render windows`
(`STATUS_FORMAT_1` still has none):

```rust
#[test]
fn two_line_formats_shell_calls_are_injection_safe() {
    // STATUS_FORMAT_1 (status-left/right line) still has no shell call.
    assert!(!STATUS_FORMAT_1.contains("#("), "STATUS_FORMAT_1 has no shell call");
    // STATUS_FORMAT_0 now has exactly one: the batched window render, and its
    // only interpolated tmux var is q-escaped (invariant #4).
    assert_eq!(STATUS_FORMAT_0.matches("#(").count(), 1, "one batched call");
    assert!(STATUS_FORMAT_0.contains("render windows --session=#{q:session_name}"));
    assert!(!STATUS_FORMAT_0.contains("#{window_name}"), "no bare untrusted var");
}
```

(Delete the old `two_line_formats_contain_no_shell_calls` — it is superseded.)

- [ ] **Step 4: Run the tmux_conf tests to verify the new ones fail**

Run: `cargo test -p rustline two_line_status_format_0_batches_window_render two_line_formats_shell_calls_are_injection_safe`
Expected: FAIL — the current `STATUS_FORMAT_0` still contains `#{W:` and no `render windows`.

- [ ] **Step 5: Rewrite `STATUS_FORMAT_0`**

In `crates/rustline/src/tmux_conf.rs`, replace the entire `STATUS_FORMAT_0` const with the batched form (keep the list-marker/alignment prefix; replace everything from `#{W:` onward with the single `#()`):

```rust
/// Two-line `status-format[0]`: the window list, now rendered by ONE batched
/// `rustline render windows` call instead of a per-window `#{W:}` loop (W42).
/// The list-marker prefix (`#[list=…]`) still marks the scrollable window
/// region. Injection-safe: the only tmux var is `--session=#{q:session_name}`;
/// `@BINARY@` is substituted by `init_block`'s blanket replace (this const now
/// carries a `#()`, unlike before).
const STATUS_FORMAT_0: &str = r##"set -g status-format[0] "#[list=on align=#{status-justify}]#[list=left-marker]<#[list=right-marker]>#[list=on]#(@BINARY@ render windows --session=#{q:session_name})""##;
```

- [ ] **Step 6: Run the tmux_conf tests + full bin suite**

Run: `cargo test -p rustline tmux_conf` (or the two named tests) then `cargo test -p rustline`
Expected: PASS — the two new/updated tests pass; the other `init_block`/two-line tests still pass (adjust any that asserted the old `#{T:window-status-format}` presence inside STATUS_FORMAT_0, if any — the shared `setw -g window-status-format` lines are untouched, so `two_line_emits_status_two_and_formats`'s `#{T:window-status-format}` assertion, which checks the shared block not STATUS_FORMAT_0, still holds; verify).

- [ ] **Step 7: fmt + clippy + commit**

Run: `cargo fmt --all && cargo clippy -p rustline --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/rustline/src/cli.rs crates/rustline/src/main.rs crates/rustline/src/tmux_conf.rs
git commit -m "feat(render): wire `render windows` + batch two-line STATUS_FORMAT_0 (W42)"
```

---

### Task 3: W57 — `{spark}` gate scans cpu/memory instances

**Files:**
- Modify: `crates/rustline-core/src/config.rs` (`refs_spark` + `Config::spark_referenced_in_layout`)
- Modify: `crates/rustline/src/build_context.rs` (use the new gate; remove the W56 `spark_referenced`)

**Interfaces:**
- Consumes: `Config::instances_of_kind` (existing private), `CpuOpts`/`MemoryOpts` (existing).
- Produces: `Config::spark_referenced_in_layout(&self, layout: &[String], kind: &str) -> bool`.

- [ ] **Step 1: Write the failing config test**

In `crates/rustline-core/src/config.rs` `mod tests`, add:

```rust
#[test]
fn spark_referenced_in_layout_covers_base_and_instances() {
    // Base cpu with {spark} in format.
    let mut cfg = Config::default();
    cfg.widgets.cpu.format = "{icon} {spark} {percent}%".into();
    assert!(cfg.spark_referenced_in_layout(&["cpu".into()], "cpu"));

    // Base default (no spark), but a cpu instance IN the layout has {spark}.
    let mut cfg2 = Config::default();
    let mut t = toml::value::Table::new();
    t.insert("kind".into(), "cpu".into());
    t.insert("format".into(), "{spark}".into());
    cfg2.instances.insert("cpu2".into(), toml::Value::Table(t));
    assert!(
        cfg2.spark_referenced_in_layout(&["cpu2".into()], "cpu"),
        "instance-only {{spark}} counts"
    );
    // Same instance NOT in the layout → false.
    assert!(!cfg2.spark_referenced_in_layout(&["cpu".into()], "cpu"));

    // Neither base nor instance references spark → false; wrong kind → false.
    let cfg3 = Config::default();
    assert!(!cfg3.spark_referenced_in_layout(&["cpu".into()], "cpu"));
    assert!(!cfg3.spark_referenced_in_layout(&["disk".into()], "disk"));
}
```

- [ ] **Step 2: Run the config test to verify it fails**

Run: `cargo test -p rustline-core spark_referenced_in_layout_covers_base_and_instances`
Expected: FAIL to compile — `no method spark_referenced_in_layout`.

- [ ] **Step 3: Implement `refs_spark` + `spark_referenced_in_layout`**

In `crates/rustline-core/src/config.rs`, add a module-level private helper and the `Config` method (place the method near `disk_mounts`/`throughput_interfaces`, which it mirrors):

```rust
/// The single home for the `{spark}` literal check: a widget references the
/// sparkline placeholder in EITHER its `format` or its click-toggle `alt_format`.
fn refs_spark(format: &str, alt_format: &str) -> bool {
    format.contains("{spark}") || alt_format.contains("{spark}")
}
```

```rust
    /// Does any `cpu`/`memory` widget IN the layout — the base widget OR a
    /// `[instances.<name>]` of that kind — reference `{spark}` in its
    /// `format`/`alt_format`? Gates the shared history read/persist so an
    /// instance-only `{spark}` still accumulates (W57). Non-cpu/memory → false.
    pub fn spark_referenced_in_layout(&self, layout: &[String], kind: &str) -> bool {
        let base_hit = layout.iter().any(|n| n == kind)
            && match kind {
                "cpu" => refs_spark(&self.widgets.cpu.format, &self.widgets.cpu.alt_format),
                "memory" => refs_spark(&self.widgets.memory.format, &self.widgets.memory.alt_format),
                _ => false,
            };
        base_hit
            || self.instances_of_kind(layout, kind).any(|table| match kind {
                "cpu" => {
                    let o: CpuOpts = table.clone().try_into().unwrap_or_default();
                    refs_spark(&o.format, &o.alt_format)
                }
                "memory" => {
                    let o: MemoryOpts = table.clone().try_into().unwrap_or_default();
                    refs_spark(&o.format, &o.alt_format)
                }
                _ => false,
            })
    }
```

- [ ] **Step 4: Run the config test to verify it passes**

Run: `cargo test -p rustline-core spark_referenced_in_layout`
Expected: PASS.

- [ ] **Step 5: Write the failing build_context test**

In `crates/rustline/src/build_context.rs` `mod tests`, add (mirrors the existing spark history tests; uses `ENV_LOCK` + `XDG_DATA_HOME` tempdir):

```rust
#[test]
fn cpu_history_populated_when_a_cpu_instance_references_spark() {
    // Base cpu format is the default (no {spark}); a cpu INSTANCE in the layout
    // has {spark}. History must still populate (W57) — the shared ring.
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    let mut t = toml::value::Table::new();
    t.insert("kind".into(), "cpu".into());
    t.insert("format".into(), "{icon} {spark} {percent}%".into());
    cfg.instances.insert("cpu2".into(), toml::Value::Table(t));
    let layout = ["cpu2".to_string()];
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: serialized by ENV_LOCK against the other env-mutating tests.
    unsafe { std::env::set_var("XDG_DATA_HOME", tmp.path()); }
    let ctx = build_region_context(&RegionArgs::default(), &layout, &Theme::default(), &cfg);
    // SAFETY: matches the set above.
    unsafe { std::env::remove_var("XDG_DATA_HOME"); }
    drop(guard);
    assert_eq!(ctx.cpu_history.len(), 1, "instance-only spark populates history");
}
```

- [ ] **Step 6: Run the build_context test to verify it fails**

Run: `cargo test -p rustline cpu_history_populated_when_a_cpu_instance_references_spark`
Expected: FAIL — the current gate reads only the base widget, so `ctx.cpu_history` is empty (`len 0`, not `1`). (It compiles only after Step 7 swaps the gate; if it references the old removed helper it won't — so write the test to call the new path. It compiles against the existing build_context, fails on the assertion because the gate ignores the instance.)

- [ ] **Step 7: Swap the gate in `build_context.rs`**

Replace the W56 `spark_referenced(&cfg.widgets.cpu.format, …)` guards with the
new `Config` method, for BOTH cpu and memory:

```rust
    let cpu_history = match cpu {
        Some(c) if cfg.spark_referenced_in_layout(layout, "cpu") => crate::cpu::read_cpu_history(
            &rustline_wasm::state_root(),
            c.percent,
            cfg.widgets.cpu.spark_width,
        ),
        _ => Vec::new(),
    };
    // …
    let mem_history = match memory {
        Some(m) if cfg.spark_referenced_in_layout(layout, "memory") => {
            let percent = if m.total_bytes == 0 {
                0.0
            } else {
                (m.used_bytes as f64 / m.total_bytes as f64 * 100.0) as f32
            };
            crate::memory::read_memory_history(
                &rustline_wasm::state_root(),
                percent,
                cfg.widgets.memory.spark_width,
            )
        }
        _ => Vec::new(),
    };
```

Delete the now-unused private `fn spark_referenced(format, alt_format)` (W56)
from `build_context.rs` — its predicate now lives in `config.rs::refs_spark`.

- [ ] **Step 8: Run the build_context spark tests to verify they pass**

Run: `cargo test -p rustline cpu_mem_history cpu_history_populated_when_a_cpu_instance_references_spark`
Expected: PASS — the new instance test, the W56 alt_format test, AND the byte-identical-default test (`cpu_mem_history_empty_when_spark_absent_from_format`) all pass.

- [ ] **Step 9: fmt + clippy + commit**

Run: `cargo fmt --all && cargo clippy -p rustline-core -p rustline --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/rustline-core/src/config.rs crates/rustline/src/build_context.rs
git commit -m "feat(spark): gate {spark} history over cpu/memory instances, not just base (W57)"
```

---

### Task 4: Docs sync + full verification

**Files:**
- Modify: `CLAUDE.md`, `README.md`

- [ ] **Step 1: Update `CLAUDE.md`**

- **CLI section:** add `rustline render windows [--session=<s>] [--preview]` — "renders a whole session's window list in one call (batched, two-line status-format[0]); shells out to `tmux list-windows`; in-process, not daemon-routed."
- **tmux integration model:** note the two-line `status-format[0]` window list is now ONE batched `#(rustline render windows --session=#{q:session_name})` instead of a per-window `#{W:}` loop — N spawns → 1; injection-safe (session q-escaped, window names via tmux -F not the shell); click-to-select preserved (rustline re-emits `#[range=window|IDX]`). One-line mode unchanged (per-window).
- **Module map:** `crates/rustline/src/windows.rs` (`read_windows`/`parse_list_windows`); `assemble.rs` gains `render_windows` (+ the shared `window_pill`); `config.rs` gains `spark_referenced_in_layout` (W57 — the `{spark}` gate now scans base + layout instances). Update the `build_context.rs` `{spark}` gating bullet to "base widget OR any cpu/memory instance in the layout references `{spark}`".
- **Roadmap:** add a done entry (branch `whats-next/2026-07-24-execute-2`) summarizing W42 + W57, with spec/plan links. Add the spec + plan to the Design-docs list.

- [ ] **Step 2: Update `README.md`**

Add one line to the window/tmux section that the two-line window list renders in a single batched call (mention `render windows` if the README documents subcommands).

- [ ] **Step 3: Full-suite verification**

Run: `cargo fmt --all --check` → no diff.
Run: `cargo clippy --all-targets -- -D warnings` → clean workspace.
Run: `just test` (or `cargo test --workspace`) → all pass, hermetic.

- [ ] **Step 4: Non-interactive render check (no default-output drift for the region paths)**

Run: `cargo run -q -p rustline -- render windows --preview` (inside tmux: prints pills; the raw form `render windows` prints markup with `#[range=window|IDX]` groups).
Run: `cargo run -q -p rustline -- render left --preview` and `render right --preview` — unchanged.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md README.md
git commit -m "docs: sync CLAUDE.md + README.md for batched window render (W42) + W57"
```

---

### Task 5: Final review + live tmux verification + finish branch

**Files:** none (review + manual verification + integration).

- [ ] **Step 1: Dispatch the final whole-branch code reviewer** (opus) over the merge-base..HEAD range, with the accumulated Minors list.
- [ ] **Step 2: One consolidated fix-wave** for any Critical/Important findings (plus triaged Minors).
- [ ] **Step 3: LIVE tmux verification (load-bearing, cannot be unit-tested):** apply the new two-line `STATUS_FORMAT_0` to a tmux session (throwaway or the running one, with user consent since it touches their live bar), confirm (a) all window pills render with the active one highlighted, and (b) **clicking a window selects it** (`#{mouse_status_range}==window` → `select-window -t=`). This click-select gate is the reason W42 was split out — do not call the branch done until it's confirmed. If a live click can't be driven programmatically, ask the user to click and confirm.
- [ ] **Step 4:** Strip W42 + W57 from `WHATS-NEXT.md`; `superpowers:finishing-a-development-branch` (present merge/PR/cleanup options).

---

## Self-Review

**Spec coverage:**
- W42 engine (render_windows + read_windows/parse) → Task 1. ✓
- W42 CLI + dispatch + tmux STATUS_FORMAT_0 → Task 2. ✓
- W42 injection-safety + click-range preservation → Task 2 tests + Task 5 live check. ✓
- W57 config gate + build_context swap → Task 3. ✓
- Docs + roadmap → Task 4. ✓
- Final review + live click verification + finish → Task 5. ✓
- Out-of-scope (one-line batching, daemon-routing, per-instance spark_width) → not tasked, per spec. ✓

**Placeholder scan:** no TBD/TODO; every code step carries the actual code. The one conditional (`#[allow(dead_code)]` in Task 1 Step 10 if clippy flags `read_windows`) is a bounded, explicit instruction removed in Task 2.

**Type consistency:** `render_windows(&[WindowCtx], &Registry, &Theme) -> String`, `read_windows(Option<&str>) -> Vec<WindowCtx>`, `parse_list_windows(&str) -> Vec<WindowCtx>`, `spark_referenced_in_layout(&[String], &str) -> bool`, `refs_spark(&str,&str) -> bool` — each defined once and called with matching signatures. `Render::Windows(WindowsArgs)` matched in the same task's dispatch.
