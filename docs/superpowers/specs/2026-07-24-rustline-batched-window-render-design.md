# rustline — batched window render (W42) + spark-gate-over-instances (W57) design

Date: 2026-07-24
Branch: `whats-next/2026-07-24-execute-2`
Source: `/whats-next --execute #2` handoff (WHATS-NEXT.md items W42, W57).

Two items. W42 is the substantial one (a new tmux integration path); W57 is a
small follow-up closing a gap the bundle #5 review found. They are independent
(different files, no shared surface) but shipped together.

## Global invariants this work depends on / must preserve

- **#4 Injection safety:** every tmux `#{…}` var interpolated into a `#(…)`
  shell call is wrapped `#{q:…}` and passed `--flag=`. Untrusted window
  names/flags must NEVER reach `/bin/sh`.
- **#5 `render_region` ordering:** segments[0] is leftmost; the caller passes
  visual left-to-right order.
- **#7 The click identity is one chain:** the `#[range=…|NAME]` markup, tmux's
  `#{mouse_status_range}`, and the click handler must agree. For window ranges
  the identity is `window|<window_index>` and the handler is tmux's built-in
  `select-window -t=`.
- **No default-output change** for W57: a config not using `{spark}` renders
  byte-identically.
- **Never break the bar:** any tmux/subprocess failure degrades to empty
  output, never a panic.

---

## W42 — Batched window render (`rustline render windows`, two-line mode)

### Problem

`window-status-format`/`window-status-current-format` (and, in two-line mode,
the `#{W:…}` loop in `STATUS_FORMAT_0` via `#{T:window-status-format}`) shell out
to `rustline render window` **once per window** on every status refresh. Each is
a fresh process (fork+exec+dynamic-link+clap-parse). A 10–20 window session pays
10–20 process spawns per refresh purely to draw pills.

(Note: `build_window_context` is already lean — W8 — so those spawns do NOT do
battery/getifaddrs/cpu reads; the cost is the N cold process starts + N tmux
`#()` shell invocations, not expensive per-window I/O. With the W48 daemon
running, each spawn is a warm socket round-trip rather than a cold start, so the
daemon already absorbs the cold-start cost; batching's remaining win is
N shell-spawns + N round-trips → 1 each.)

### Scope (decided in brainstorming)

- **Two-line mode only.** Batch the `STATUS_FORMAT_0` window loop. One-line mode
  keeps its per-window path (a noted follow-up — see Out of scope).
- **Mechanism: rustline queries `tmux list-windows`** (not `#{W:}`-builds-an-arg).
  Verified building blocks; window names never transit the shell.
- **In-process, NOT daemon-routed.** Unlike `render left/right/window`, the
  batched path does not try the W48 daemon socket — a systemd/launchd-started
  daemon has no `$TMUX` in its environment and could not run `tmux list-windows`
  for the right server/session, whereas the client (spawned by tmux) can. The
  batched path is already cheap (lean `WindowCtx`s, no plugins, no registry
  cold-build, no expensive reads), so in-process is both correct and simplest.

### New CLI subcommand

`rustline render windows [--session=<s>] [--preview]`

- Added as a `Render::Windows(WindowsArgs)` variant (`cli.rs`), alongside
  `Left`/`Right`/`Window`. `WindowsArgs { session: Option<String>, preview: bool }`.
- **No `--plugin-dir`** — windows never run plugins (same as `render window`).

### `crates/rustline/src/windows.rs` (new bin module) — the read surface

Follows the `git.rs`/`media.rs` shell-out pattern (a tool shell-out behind a
pure parser):

```rust
use std::process::Command;
use rustline_core::WindowCtx;

/// Enumerate a tmux session's windows via `tmux list-windows -F`. `None`-session
/// lists the server's current session. Empty Vec on ANY failure (tmux missing,
/// bad session, non-zero exit) — never a panic, never a fabricated window.
pub fn read_windows(session: Option<&str>) -> Vec<WindowCtx> {
    let mut cmd = Command::new("tmux");
    cmd.arg("list-windows");
    if let Some(s) = session {
        cmd.args(["-t", s]);
    }
    // Tab-separated; NAME LAST so a tab inside a name can't misalign earlier
    // fields (parse_list_windows uses splitn(4)). tmux strips newlines from
    // window names, so one window per line is safe.
    cmd.args(["-F", "#{window_index}\t#{window_active}\t#{window_flags}\t#{window_name}"]);
    let Ok(out) = cmd.output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_list_windows(&String::from_utf8_lossy(&out.stdout))
}

/// Pure parse of `list-windows -F` output into WindowCtx. Tolerant: a line with
/// fewer than the three leading tab fields is skipped (never a partial/garbled
/// window); the name is the `splitn(4)` remainder so it may contain tabs.
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

`parse_list_windows` is unconditionally unit-tested (no cfg-gating — no OS
branching, like `git.rs::parse_git_status`).

### `crates/rustline-core/src/assemble.rs` — the batched renderer

Refactor `render_window` to share a private per-window pill helper, and add
`render_windows`:

```rust
/// One window's rounded pill (no range wrapping). The existing single-window
/// path (`render window`) and the batched path share this.
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
/// (MouseDown1Status → `select-window -t=`) keeps working — the batched output
/// re-emits the exact `range=window|IDX` markers tmux's own `#{W:}` loop used to.
/// Joined with no separator (the pills are self-contained rounded pills, matching
/// the current `window-status-separator ""`).
pub fn render_windows(windows: &[WindowCtx], registry: &Registry, theme: &Theme) -> String {
    windows
        .iter()
        .map(|wc| {
            let ctx = Context { window: Some(wc.clone()), ..Context::default() };
            let pill = window_pill(&ctx, registry, theme);
            if pill.is_empty() {
                String::new()
            } else {
                // window_index is tmux-numeric, safe verbatim in range=window|N.
                format!("#[range=window|{}]{}#[norange]", wc.index, pill)
            }
        })
        .collect()
}
```

Notes:
- The active-vs-inactive distinction (`is_current`) is the only per-window style
  rustline applies — same as today. tmux's `window-status-*` bell/activity/last
  styles are already overridden by `render_window_pill`'s explicit pill fg/bg in
  the *current* per-window design, so batching is visually equivalent (no
  bell/activity regression to worry about — that distinction isn't visible today).

### `crates/rustline/src/main.rs` — dispatch

```rust
Command::Render(Render::Windows(args)) => {
    // In-process (NOT daemon-routed — see spec: daemon lacks $TMUX).
    let registry = Registry::with_builtins(&cfg); // builtins only; windows widget
    let windows = crate::windows::read_windows(args.session.as_deref());
    let markup = rustline_core::render_windows(&windows, &registry, &theme);
    emit(&markup, args.preview);
}
```

(`render window` singular is unchanged and stays daemon-aware for one-line mode.)

### `crates/rustline/src/tmux_conf.rs` — `STATUS_FORMAT_0`

Replace the entire `#{W:…}` per-window window-loop in the two-line
`STATUS_FORMAT_0` const with a single batched `#()` call. Keep the leading
list-marker/alignment prefix verbatim (`#[list=on align=#{status-justify}]#[list=left-marker]<#[list=right-marker]>#[list=on]`
— these tell tmux where the scrollable window list sits and its truncation
markers, and still apply to a single-`#()` blob); replace everything from `#{W:`
onward. The whole new const:

```rust
const STATUS_FORMAT_0: &str = r##"set -g status-format[0] "#[list=on align=#{status-justify}]#[list=left-marker]<#[list=right-marker]>#[list=on]#(@BINARY@ render windows --session=#{q:session_name})""##;
```

- `@BINARY@` is the existing shell-quoted absolute-path placeholder. It was NOT
  previously present in `STATUS_FORMAT_0` (which had no `#()` calls); adding it
  here means `init_block`'s final blanket `block.replace("@BINARY@", …)` now also
  substitutes it inside `STATUS_FORMAT_0` — the intended mechanism (same as the
  shared block).
- **Injection-safe:** the only tmux var is `--session=#{q:session_name}`
  (q-escaped, `=`-form). Window names/flags come from rustline's own
  `tmux list-windows` subprocess, never through `/bin/sh` (invariant #4). The
  `two_line_formats_contain_no_shell_calls` test must be updated — the batched
  form DOES introduce a `#(` into `STATUS_FORMAT_0`, so that test's assertion is
  narrowed to "the only `#(` is the batched `render windows` call, still
  injection-safe" (it is no longer true that STATUS_FORMAT_0 has zero shell
  calls; the new invariant is that its one shell call is q-escaped).
- The shared block's `setw -g window-status-format`/`window-status-current-format`
  lines (the per-window `#(rustline render window …)`) STAY — they are still used
  by one-line mode and by any client not on the two-line format. In two-line mode
  they are simply not consulted (STATUS_FORMAT_0 no longer references
  `#{T:window-status-format}`). Leaving them keeps one-line unchanged.
- The `MouseDown1Status` binding is UNCHANGED — it already dispatches on
  `#{mouse_status_range}==window` → `select-window -t=`, and rustline now emits
  those `range=window|IDX` markers.

### Tests (W42)

- `parse_list_windows`: well-formed multi-window; a name containing a tab
  (survives via `splitn(4)` remainder); a malformed line (< 3 tabs) skipped; empty
  input → empty Vec; `active=="1"` → `is_current`.
- `render_windows` (core): N windows → N `#[range=window|IDX]` groups; the active
  window's pill differs from an inactive one (accent vs gray); an empty-name
  window still renders; empty slice → empty string. Assert `#[norange]` closes
  each group and the `range=window|<index>` index matches each WindowCtx.
- `render_window` (single) still byte-identical after the `window_pill` refactor
  (characterization: same input → same output as before).
- `tmux_conf` STATUS_FORMAT_0: the batched `#(… render windows --session=#{q:session_name})`
  is present; `#{q:session_name}` q-escaped; no bare `#{window_name}`/`#{window_index}`
  interpolated into a shell call in the window loop; `@BINARY@` substituted. Update
  `two_line_formats_contain_no_shell_calls` per above.

### Live verification (load-bearing — done during SDD/finish, not a unit test)

Against the user's running tmux (or a throwaway session): apply the new
`STATUS_FORMAT_0`, confirm (1) all window pills render, active highlighted, and
(2) **clicking a window still selects it** (`#{mouse_status_range}==window` path).
The click-select cannot be asserted in a unit test (needs a real mouse event), so
it is a manual gate before the branch is called done. `rustline render windows
--preview` gives a non-interactive render check.

---

## W57 — `{spark}` history gate scans cpu/memory instances

### Problem

`build_region_context` gates the cpu/mem `{spark}`-history read (W56) on the
**base** widget only: `spark_referenced(&cfg.widgets.cpu.format,
&cfg.widgets.cpu.alt_format)`. A `[instances.<name>]` of kind `cpu`/`memory`
(W46) with `{spark}` in its format won't populate the shared
`Context.cpu_history`/`mem_history` unless the base widget also references
`{spark}` — so an instance-only sparkline renders permanently empty, the same
silent-empty failure W56 closed one layer down. Flagged in the bundle #5 final
review.

### Change

Move the `{spark}`-reference predicate into `rustline-core::Config` (so it can see
`cfg.instances`, which the bin's `build_context.rs` predicate can't reach cleanly)
and have it scan the base widget **plus** every layout instance of the kind.
Mirrors the existing `disk_mounts`/`throughput_interfaces` instance-scan pattern:

```rust
// config.rs — private predicate (one home for the {spark} literal check)
fn refs_spark(format: &str, alt_format: &str) -> bool {
    format.contains("{spark}") || alt_format.contains("{spark}")
}

impl Config {
    /// Does any `cpu`/`memory` widget IN the layout — the base widget or a
    /// `[instances.<name>]` of that kind — reference `{spark}` in its
    /// `format`/`alt_format`? Gates the history read/persist so an instance-only
    /// `{spark}` still accumulates (W57). Only `"cpu"`/`"memory"` are meaningful;
    /// any other kind returns false.
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
}
```

In `build_context.rs`, replace the W56 gate:

```rust
// cpu
let cpu_history = match cpu {
    Some(c) if cfg.spark_referenced_in_layout(layout, "cpu") =>
        crate::cpu::read_cpu_history(&rustline_wasm::state_root(), c.percent, cfg.widgets.cpu.spark_width),
    _ => Vec::new(),
};
// memory: guard becomes cfg.spark_referenced_in_layout(layout, "memory")
```

and delete `build_context.rs`'s now-subsumed private `spark_referenced` helper
(W56) — its one-line predicate now lives in `config.rs::refs_spark`, applied to
base + instances.

Note on `spark_width`: the history ring length still comes from the **base**
widget's `spark_width` (`cfg.widgets.cpu.spark_width`), unchanged — a per-instance
`spark_width` is out of scope (the history is a single shared ring per kind; the
instances feed/consume the same `Context.cpu_history`).

### Tests (W57)

- `spark_referenced_in_layout` (config, unit): base `{spark}` in `format` → true;
  base `{spark}` in `alt_format` only → true; a `[instances.cpu]` with `{spark}`
  in its `format` while the base has none, and the instance name in the layout →
  true; the same instance NOT in the layout → false; neither → false; a `disk`
  kind → false.
- `build_context.rs`: extend the existing history tests — a cpu instance-only
  `{spark}` (base default, `[instances.cpu2]` with `{spark}`, `cpu2` in layout)
  now populates `ctx.cpu_history` (len 1). Keep the byte-identical-default test
  (no `{spark}` anywhere → empty).

---

## Cross-cutting

- **Branch:** `whats-next/2026-07-24-execute-2` (off `main` @ f36c276).
- **Toolchain:** clippy-clean (`-D warnings`), rustfmt-clean, edition 2024,
  hermetic `just test` (W42's `read_windows` shell-out isn't unit-tested — only
  the pure `parse_list_windows` is, matching `git.rs`).
- **Docs:** update CLAUDE.md (CLI `render windows`, tmux integration model — the
  two-line window loop is now one batched call; module map for `windows.rs`,
  `render_windows`, `spark_referenced_in_layout`) + README (the batched window
  behavior) + roadmap entry. Strip W42/W57 from WHATS-NEXT.md on finish.
- **rustls / deps:** no new dependencies (`std::process::Command` for the tmux
  shell-out; no TLS).

## Out of scope (explicitly)

- **One-line mode batching** — a follow-up (a net-new explicit `status-format[0]`
  for one-line; more wiring + verification surface).
- **Daemon-routing the batched path** — deliberately in-process (daemon lacks
  `$TMUX`). A future design could have the client fetch the window list and pass
  it over the daemon protocol, but that's more scope for a marginal win.
- **Per-instance `spark_width`** — the `{spark}` history is a single shared ring
  per kind.
- **bell/activity/last per-window styling** — not shown today (rustline's pill
  fg/bg overrides tmux's window styles), so batching preserves current visuals.
