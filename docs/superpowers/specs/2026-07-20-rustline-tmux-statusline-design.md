# rustline — a Rust tmux statusline system (v1 design)

**Date:** 2026-07-20
**Status:** Approved (brainstorming → spec)
**Scope:** v1 — core widgets baked in, powerline rendering, usable in tmux today. WASM plugins are *structured for* but not *implemented*.

## 1. Purpose & success criteria

`rustline` renders a tmux status line from Rust. It is primarily for the author's
own use but is designed to be shared and, later, extended by third-party WASM
plugins.

**v1 is successful when:** after `cargo build --release` and sourcing the block
emitted by `rustline init`, tmux shows a live powerline status line with the
widgets below, updating on `status-interval` and instantly on pane/window switch.

Non-goals for v1 (explicitly deferred): the WASM plugin *runtime*, a long-running
daemon, per-widget independent refresh cadences, click handling, and Windows
support. The design keeps the door open for each (see §9) but ships none.

## 2. Invocation model (decided)

tmux drives rendering by **shelling out per region** on `status-interval`:

```tmux
set -g status-interval 1
set -g status-left  "#(rustline render left)"
set -g status-right "#(rustline render right)"
setw -g window-status-format         "#(rustline render window #{window_index} '#{window_name}' '#{window_flags}')"
setw -g window-status-current-format "#(rustline render window --current #{window_index} '#{window_name}' '#{window_flags}')"
```

tmux expands `#{…}` format variables **inside** the `#(…)` command before running
it, so each invocation receives the *current client's* context as CLI arguments —
this is why the client-specific widgets (pane id, window list, cwd) are correct
per client without any tmux round-trip. Instant updates on tmux-internal events
come from hooks, not a daemon:

```tmux
set-hook -g after-select-pane   "refresh-client -S"
set-hook -g after-select-window "refresh-client -S"
```

**Daemon-ready core.** The rendering core is a *pure function* — given a `Context`
snapshot it returns a region's tmux-format string. The CLI is one front-end that
builds a `Context` and calls it once. A future daemon (or WASM host) is just
another front-end building `Context`s from a different source; the core is reused
verbatim. No part of v1 hard-codes the assumption of a short-lived process.

## 3. Project layout (decided)

A Cargo **workspace**, edition 2024, `resolver = "2"`:

```
rustline/
  Cargo.toml               # [workspace]; rustfmt.toml edition=2024
  crates/
    rustline-core/         # lib: pure, front-end-agnostic, serde ABI
    rustline/              # bin: clap CLI + tmux glue
  # future: crates/rustline-wasm/  (WASM host, not in v1)
```

`rustline-core` has three eventual front-ends — the CLI (today), a daemon, and a
WASM host (both later). Splitting core from the binary now makes that seam
explicit at ~zero cost and matches the "structure correctly for plugins" goal.

## 4. Core architecture (`rustline-core`)

All types below are **`serde`-serializable** — that is deliberately the future
WASM ABI (a plugin receives a serialized `Context`, returns serialized
`Segment`s).

### 4.1 `Context`
A snapshot of everything any widget could need, built by a front-end:

```rust
pub struct Context {
    pub session_name: String,     // "0" (#S)
    pub window_index: String,     // "0" (#I)
    pub pane_index: String,       // "0" (#P)
    pub pane_current_path: String,// "/home/steve/src/rustline" (#{pane_current_path})
    pub hostname: String,         // machine short hostname
    pub loadavg: [f64; 3],        // 1/5/15-minute
    pub now: DateTime<Local>,     // wall-clock at render time
    pub window: Option<WindowCtx>,// present only for `render window`
}

pub struct WindowCtx {
    pub index: String,            // #{window_index}
    pub name: String,             // #{window_name}
    pub flags: String,            // #{window_flags}, e.g. "*", "-", "Z"
    pub is_current: bool,         // --current passed
}
```

A field a given render path doesn't need may be cheaply/emptily populated (e.g.
`window` is `None` for left/right renders). Widgets must tolerate empty inputs and
degrade to an empty `Vec<Segment>` rather than panicking.

### 4.2 `Segment` / `Style`
A widget's renderer-agnostic output:

```rust
pub struct Segment { pub text: String, pub style: Style }
pub struct Style { pub fg: Option<Color>, pub bg: Option<Color>, pub bold: bool }
pub enum Color { Named(String), Indexed(u8), Rgb(u8,u8,u8) }  // -> tmux colour spec
```

A widget returns **zero or more** segments. Zero = "render nothing" (graceful
degradation). Multiple segments from one widget are allowed but v1 built-ins each
return exactly one (the `datetime` widget's `<`-separated fields live *inside* one
segment's text — see §5).

### 4.3 `Widget` trait & `Registry`

```rust
pub trait Widget {
    fn render(&self, ctx: &Context) -> Vec<Segment>;
}
```

`Registry` maps a widget **name** to a constructed `Box<dyn Widget>`. Built-ins
register under short names (`pane_id`, `hostname`, `windows`, `cwd`, `loadavg`,
`datetime`). A plugin slot keyed by `owner/repo` is reserved in the type but
returns "unknown widget" in v1 (no WASM runtime yet). Unknown names in config are
**skipped with a `tracing::warn!`**, never fatal.

### 4.4 Powerline renderer
Pure function, the load-bearing piece:

```rust
pub fn render_region(dir: Direction, segments: &[Segment], theme: &Theme) -> String;
pub enum Direction { Left, Right }  // Left = left/center, Right = right region
```

Rules:
- Each segment emits `#[fg=<fg>,bg=<bg>]<space>text<space>`.
- Between two adjacent segments with **different** bg, emit the **hard** separator
  glyph (`` left-facing region, `` right-facing) styled `fg=<prev.bg>,bg=<next.bg>`
  (colors swap by direction).
- Between adjacent segments with the **same** bg, emit the **soft** separator glyph
  (`` / ``) in a muted fg on the shared bg.
- **Edge transitions:** the outermost separator transitions between the segment's
  bg and the status bar's default bg (`fg`/`bg` chosen by direction so the arrow
  points outward).
- Reset with `#[default]` where needed so tmux state doesn't leak between regions.
- An empty segment list renders to the empty string.

Glyphs are configurable in `Theme` (default: `` `` `` ``). This function is the
primary TDD target — see §7.

### 4.5 Config
`serde` types, parsed from TOML at `$XDG_CONFIG_HOME/rustline/config.toml`
(default `~/.config/rustline/config.toml`). **Zero-config works**: a built-in
`Config::default()` reproduces the layout and theme in §5, so a fresh machine
needs no config file.

```toml
# every field optional; shown values are the built-in defaults

[layout]
left   = ["pane_id", "hostname"]
center = ["windows"]
right  = ["cwd", "loadavg", "datetime"]

[theme]
# palette + separator glyphs (indexed/named/rgb colors)
hard_left = ""   # 
hard_right = ""  # 
soft_left = ""   # 
soft_right = ""  # 
# per-segment fg/bg default palette defined here

[widgets.datetime]
format = "%a < %Y-%m-%d < %H:%M"   # `<` is literal text inside the format

[widgets.cwd]
abbreviate_home = true             # $HOME -> ~

# reserved for future WASM plugins; parsed & retained, unused in v1
# [plugins."owner/repo"]
# key = "value"
```

Config load never hard-fails the render: a malformed file logs a warning and
falls back to `Config::default()` so the bar keeps working.

## 5. Built-in widgets (v1)

| widget     | example output          | derivation |
|------------|-------------------------|------------|
| `pane_id`  | `0:0.0`                 | `format!("{session}:{window}.{pane}")` from ctx (`#S:#I.#P`) |
| `hostname` | `scadrial`              | `gethostname`, first label before any `.` |
| `windows`  | `0* name` / `1 other`   | per-window: `#I` + flags + ` ` + `#W`; current window highlighted via `Style` (bold/alt bg), not by literal markup |
| `cwd`      | `~/src/rustline`        | `pane_current_path`, `$HOME` prefix → `~` when `abbreviate_home` |
| `loadavg`  | `0.42 0.31 0.29`        | `getloadavg` → three `{:.2}` values |
| `datetime` | `Mon < 2026-07-20 < 17:49` | `now.format(config.datetime.format)` — `<` is literal in the format string |

Notes:
- **`pane_id`** uses `#S:#I.#P` (session:window.pane), matching tmux's own `0:0.0`.
- **`windows`** is rendered one segment per `render window` invocation (tmux repeats
  the format per window). `--current` sets `is_current`, which selects the active
  style. The `*`/`-`/`Z` flags come straight from `#{window_flags}`.
- **`datetime`**'s `<` separators are literal characters in the strftime format,
  *inside* one segment — they are not powerline separators and not the config's
  separator glyphs. Changing the date format is a `[widgets.datetime]` override.

## 6. CLI (`rustline` bin)

`clap` derive, `#[command(version, about)]`:

| command | behavior |
|---------|----------|
| `render left` / `render right` | build `Context`, render that region (its configured widgets), print |
| `render window [--current] <index> <name> <flags>` | build `Context` with `WindowCtx`, render one window segment, print |
| `init` | print the tmux.conf block (§2) to stdout for the user to source/paste |
| `print-config` | print the effective/default config as TOML |

The **center region is not a `render` subcommand.** It is the tmux window list,
realized through `window-status-format`/`-current-format` calling `render window`
once per window (§2). The config's `center = ["windows"]` (§4.5) documents that the
center holds the `windows` widget; in v1 that is the only supported center widget
and it is driven by the per-window path, not a whole-region render. `render left`
and `render right` resolve their configured widget lists and powerline-join them
via `render_region`.

Context construction (bin-side, not core):
- `hostname` via the `gethostname` crate.
- `loadavg` via `libc::getloadavg` (portable across Linux/macOS/BSD; on Linux this
  reads the same data as `/proc/loadavg`).
- `now` via `chrono::Local::now()`.
- session/window/pane/path from CLI args supplied by tmux's expanded format vars.
  For left/right, `render` accepts optional context flags (e.g. `--pane-path`,
  `--session`, `--window`, `--pane`) that the `init` block wires up from
  `#{session_name}`, `#{window_index}`, `#{pane_index}`, `#{pane_current_path}`;
  when absent, fields default to empty and the affected widget degrades.

Logging: `tracing` + `tracing-subscriber` with `EnvFilter` (`RUST_LOG`, default
`warn` for a per-render CLI so it stays quiet in the status bar). Errors in one
widget are caught and logged; the region still renders.

## 7. Testing strategy (TDD)

Pure functions get unit tests written first. Per the project's spec discipline,
we do **not** skip a test by appealing to a current invariant; the seams below are
pinned directly.

- **Powerline renderer** (`render_region`): the load-bearing test.
  - single segment (edge transitions on both sides, both directions);
  - two segments, different bg → hard separator with swapped fg/bg;
  - two segments, same bg → soft separator, muted fg;
  - empty segment list → empty string;
  - `Left` vs `Right` glyph/orientation differences;
  - `#[default]` reset present so state can't leak.
- **`datetime`**: format a **fixed** `DateTime<Local>` and assert
  `Mon < 2026-07-20 < 17:49`; assert a custom `format` override is honored. (This
  pins the `<`-as-literal behavior at the widget seam, not just in prose.)
- **`loadavg`**: format `[0.42,0.31,0.29]` → `0.42 0.31 0.29` (`{:.2}`), including
  values that round.
- **`pane_id`**: ctx → `0:0.0`.
- **`hostname`**: `"scadrial.example.com"` → `scadrial` (label truncation).
- **`cwd`**: `$HOME/src/x` → `~/src/x` with abbrev on; unchanged with abbrev off;
  path not under `$HOME` unchanged.
- **`windows`**: current vs non-current styling differs; flags rendered; name shown.
- **Config**: parse a TOML overriding layout + datetime format + a theme glyph and
  assert it takes effect; **unknown widget name in `layout` is skipped, not fatal**;
  malformed TOML falls back to `Config::default()`.
- **Registry/layout resolution**: names resolve to the right widgets in order.

Thin glue (real `getloadavg`, real `gethostname`, `init` text, arg parsing) gets
light coverage; a smoke test asserts `render left`/`render right` on a synthetic
context produce non-empty, `#[`-containing output.

**Manual verification (required before "done"):** build release, run `rustline init`,
source it into the live tmux session, reload, and confirm the powerline bar renders
with all widgets and updates on pane/window switch.

## 8. Error handling & degradation

- A widget that cannot produce output returns `vec![]`; the region renders the rest.
- A panic inside a widget is caught (the render loop guards each widget) and logged;
  the bar never dies from one bad widget.
- Missing/short CLI args → affected fields empty → affected widget degrades.
- Config parse failure → warn + `Config::default()`.
- `getloadavg` failure → `loadavg` widget renders nothing (not zeros that lie).

## 9. Invariants this feature depends on (for future changes)

Later work touching these shared funnels must re-check the callers listed:

1. **`Context` is the sole render input.** Every widget reads only from `Context`;
   nothing reaches into the environment mid-render. A daemon/WASM host relies on
   this to build `Context`s from other sources. *If a widget starts reading the
   environment directly, the daemon path breaks silently — add a test at the
   front-end seam.*
2. **`Segment`/`Context`/`Style` are serde-serializable and stay so.** This is the
   WASM ABI. A non-serializable field added here silently forecloses WASM.
3. **Config load is total (never panics/aborts the render).** The status bar must
   survive a bad config. New config parsing must preserve the fallback.
4. **tmux expands `#{…}` inside `#(…)`.** The whole per-client-correctness story
   rests on this; the `init` block and arg wiring depend on it. If a tmux version
   changes this, the arg-passing contract in §6 must be revisited.

## 10. Dependencies (from the crate menu)

- `clap` (derive), `serde` (derive) + `toml`, `chrono` (clock),
  `tracing` + `tracing-subscriber` (EnvFilter), `libc`, `gethostname`,
  `thiserror` (core lib error types), `anyhow` (bin).
- No TLS / DB / network → the OpenSSL-free policy is moot but nothing pulls it in.
- Edition 2024 across both crates; `rustfmt.toml` `edition = "2024"`; commit
  `Cargo.lock` with the dependency-adding change.
