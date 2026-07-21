# Click-to-toggle widget views (transient view switching)

**Date:** 2026-07-21
**Status:** Design approved (brainstorming → ship-it `--ask`)

## Summary

Let a user **left-click a status-line widget** to swap it between its normal
`format` and an `alt_format` — a more-detailed or more-compact view — and click
again to toggle back. The toggle is remembered in a small global state file so
it survives re-renders. tmux detects *which* widget was clicked (status-line
`#[range=user|NAME]` markup + a `MouseDown1Status` binding); rustline stays a
pure per-region shell-out — it never handles the mouse event itself.

Scope decisions (locked during brainstorming):

- **Global** toggle scope — one flat set of toggled widget names, shared across
  every session/window/pane.
- **Left-click** toggles; right-click is left untouched (reserved for the future
  `left_click`/`right_click` script handlers and/or the widget-management menu
  parked in `TODO.md`).
- `init` **respects the user's existing mouse setting** — it never emits
  `set -g mouse on`. The click binding + range markup are inert when mouse is
  off (the markup is invisible control data; the binding simply never fires), so
  "everything renders, it just won't respond to clicks" is the mouse-off
  behavior with zero detection logic.
- **Plugins** honor toggling through the same mechanism as built-ins: the toggle
  set rides in `Context.toggled` (already serialized to guests via
  `RenderInput`), and a plugin reacts by checking whether its own `name()` is in
  that set. The host cannot swap a guest's output, so a plugin *opts in*.

Non-goals (explicitly deferred): script click-handlers (`left_click`/
`right_click = "…"`), a right-click context menu, and the widget-management
enable/disable/reorder TUI (all in `TODO.md`). This feature only builds the
toggle, but shapes the click path so those slot in at one choke point.

## User-facing behavior

### Config

The six format-bearing built-ins gain an optional `alt_format` (default `""`):
`datetime`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`, `memory`. Widgets with no
format-string surface get none: `cwd` (a bool, `abbreviate_home`), `loadavg`
(hardcoded three-float format), and `pane_id`/`hostname`/`windows`. Giving
`cwd`/`loadavg` a toggle would need a format-string refactor first, so they are
deferred to a follow-up.

```toml
[widgets.cpu]
format     = "{icon} {percent}%"        # compact (default)
alt_format = "{icon} {bar} {percent}%"  # detailed; left-click swaps to this
```

A non-empty `alt_format` is the *opt-in signal*: it makes the widget clickable
(emits its range markup) and left-click toggles `format` ↔ `alt_format`. An
empty `alt_format` (the default) means the widget is **not** clickable and its
output is byte-for-byte unchanged from today — including no range markup at all,
so zero-config users and older tmux are entirely unaffected.

No config is needed to *enable* clicks globally: because clickability is
per-widget (driven by `alt_format`), adding an `alt_format` later starts working
immediately with no re-`init`.

### Interaction

1. Left-click a clickable widget in `status-left`/`status-right`.
2. tmux's `MouseDown1Status` binding runs `rustline click --range=<name>` (the
   name comes from the widget's `#[range=user|<name>]` markup), then
   `refresh-client -S`.
3. `rustline click` flips `<name>`'s membership in the global state file.
4. The refreshed render reads the state file into `Context.toggled`; the widget
   sees its name toggled and renders `alt_format`.

Clicking a **window** name still selects that window (the binding re-implements
tmux's default for the `window` range). Clicking a non-clickable region area is
a no-op.

## Architecture

### Data flow (the NAME identity is load-bearing)

One string — the widget's **layout/registry name** (e.g. `"cpu"`, or a plugin's
`.wasm` stem `"weather"`) — is threaded, unchanged, through the entire loop:

```
layout name  ─→ render: #[range=user|NAME]…#[norange]   (assemble.rs → render.rs)
             ─→ tmux:  #{mouse_status_range} == NAME     (init binding)
             ─→ CLI:   rustline click --range=NAME        (click subcommand)
             ─→ state: NAME ∈ toggles file
             ─→ Context.toggled contains NAME             (build_context.rs)
             ─→ widget: ctx.toggled.contains(NAME) ? alt_format : format
```

If any producer of that string diverges from the others, toggling silently
breaks with every suite still green — so the identity is pinned with tests at
each hop (see Testing).

### Core (`rustline-core`) — pure, no I/O

- **`Context.toggled: BTreeSet<String>`** (new field). `#[serde(default)]` so a
  Context JSON lacking the field still deserializes to an empty set (guards
  host/guest version skew and keeps deserialization total). Deterministic
  ordering (BTreeSet) keeps serialization/tests stable. *Ripple:* every
  `Context { … }` struct-literal construction site (build_context + ~6 test
  modules) must add `toggled: Default::default()`.

- **`Widget::range_name(&self) -> Option<&str>`** (new trait method, default
  `None`). Returns `Some(name)` when the widget is clickable, `None` otherwise.
  This single method unifies "is this clickable?" and "what range name?" and
  keeps `alt_format` knowledge inside the widget (assemble needs no Config
  access). It is also the natural extension point for future click affordances.
  - Format-bearing built-ins override it: `Some(NAME)` iff `!alt_format.is_empty()`.
  - `WasmWidget` overrides it: `Some(plugin_name)` (the host can't know whether a
    guest honors the signal, so plugins are always clickable; a guest that
    ignores `toggled` simply doesn't change).
  - **`range=user|X` requires `X` ≤ 15 bytes** (tmux limit). All built-in names
    fit (longest `tailscale_ip` = 12), but a plugin's `.wasm`-stem name is
    user-controlled: `range_name()` returns `None` (with a one-time `warn!`) for
    any name > 15 bytes, so an over-long name degrades to "not clickable" rather
    than emitting a range tmux would reject. A single `name_is_clickable_range`
    helper centralizes the length check for both built-ins and `WasmWidget`.

- **`active_format(ctx, name, format, alt) -> &str`** (new shared helper, e.g.
  `widgets/toggle.rs`): returns `alt` when `!alt.is_empty() && ctx.toggled.contains(name)`,
  else `format`. Each format-bearing widget calls it to pick which string feeds
  its existing placeholder replacement — a tiny per-widget change. Each widget
  carries its name as a per-type `const NAME: &str` matching its registry key
  (e.g. `CpuWidget::NAME = "cpu"`), used by both `range_name` and `active_format`.
  Toggling applies only in the data-present branch; the `down_format` branch is
  unaffected.

- **Range-aware render** (`render.rs` + `assemble.rs`): `render_named_region`
  currently flattens all widgets' segments before one `render_region` call. It
  changes to keep per-widget grouping — for each resolved widget, guarded-render
  its segments and record `(widget.range_name(), segments)`. Palette is assigned
  across the **flattened** region exactly as today (global index cycling,
  unchanged). A range-aware render path then produces markup **identical** to
  `render_region`'s (same hard/soft separators, edge blending, `#[default]`
  terminator) except that each group whose `range_name()` is `Some(NAME)` is
  bracketed by `#[range=user|NAME]` … `#[norange]`, with inter-widget separators
  emitted **outside** the brackets. `render_window` (the center pill) is
  untouched — tmux assigns `window` ranges to the window list itself.
  - **The ABI `Segment` type does NOT gain a `range` field.** Range grouping is
    host-side render metadata, not part of the WASM wire type (a guest must not
    set its own range). Keeps invariant #2's wire types minimal.

### Binary (`rustline`)

- **`toggles.rs`** (new module): the global toggle-state file, at
  `rustline_wasm::data_root().join("toggles")` → `$XDG_DATA_HOME/rustline/toggles`
  (fallback `~/.local/share/rustline/toggles`), reusing the existing XDG
  resolver so there is one base dir (matches where logs/plugins/plugin-state
  already live). `data_root` is re-exported from `rustline-wasm` if not already.
  Format: newline-delimited widget names. Pure, testable helpers:
  - `parse_toggles(&str) -> BTreeSet<String>` — total: trims lines, drops
    empties; a missing/corrupt file yields an empty set.
  - `serialize_toggles(&BTreeSet<String>) -> String` — sorted, newline-joined;
    `parse ∘ serialize == identity`.
  - `apply_toggle(set, name)` — flips membership.
  - `read_toggles()` (read+parse; IO error → empty) and `write_toggles()`
    (atomic: write `toggles.tmp`, create parent dir best-effort, rename).

- **`rustline click --range=<name> [--button=<left>]`** (new subcommand):
  loads nothing heavy; on a non-empty `--range`, flips that name in the state
  file via `apply_toggle` + `write_toggles`. Empty range → no-op. `--button`
  defaults to `left`; other values are accepted and currently no-op
  (forward-compat for right). **Totality:** any IO failure logs a `warn!` and
  exits 0 — a broken click must never break the bar or tmux. This subcommand's
  action resolution is the single choke point where future
  `left_click`/`right_click = "<script>"` handlers plug in (resolve the action
  for `(name, button)` from config; today the only action is the built-in
  toggle). The tmux binding owns `refresh-client -S`, so `click` stays pure of
  tmux and unit-testable.

- **`build_context.rs`**: populate `Context.toggled` from `read_toggles()` at the
  Context-build edge — same "read once at the edge, never mid-render" pattern as
  `loadavg`/`battery`/`cpu`. Applies to `build_region_context` (window rendering
  doesn't use user toggles, but sharing the code path is harmless — the window
  pill ignores `toggled`).

- **`tmux_conf.rs`** — `init_block` additionally emits, injection-safe
  (invariant #4 — `#{q:}` + `--flag=` form):
  ```
  # rustline click-to-toggle a widget's alt view (needs: set -g mouse on)
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
  ```
  `select-window -t=` preserves tmux's default window-click selection. `run-shell`
  (non-backgrounded) completes before `refresh-client -S`, so the toggle is
  written before the refresh; if the implementer finds ordering unreliable on
  the target tmux, fold the refresh into the shell command
  (`… && tmux refresh-client -S`). `mouse_status_range` is q-escaped even though
  it is a rustline-controlled name (defense in depth; it is still a format var).

### Plugins (`rustline-wasm`, `plugins/weather`)

No host-function or capability changes. `Context.toggled` already flows to guests
through the existing `RenderInput { context, config }` serialization. A plugin
opts into toggling by checking `context.toggled` for its own `name()` and
choosing an alternate rendering (e.g. an `options.alt_format`). The `weather`
example is updated to demonstrate this (guest-side format selection mirroring
`active_format`), with its end-to-end coverage staying behind the opt-in
`wasm-e2e` feature; the host-side seam (that `RenderInput` serializes `toggled`)
is pinned by a hermetic host test.

## Invariants this feature depends on

A later, unrelated change that touches one of these could silently kill toggling
while every suite stays green — so each is pinned by a test (see Testing):

1. **NAME identity across all hops** — the range name emitted by render, the
   `#{mouse_status_range}` tmux returns, the `--range` value, the `Context.toggled`
   membership key, and the widget's `active_format`/`range_name` key are all the
   **same** layout/registry name. Producers: `assemble.rs`/`render.rs` (emit),
   `tmux_conf.rs` (binding), `click` subcommand, `toggles.rs`, each widget's
   `const NAME`, `WasmWidget` (plugin name). The emitted range argument is ≤ 15
   bytes (tmux's `user|X` limit); a longer name degrades to not-clickable.
2. **Range injection is non-destructive** — adding `#[range]`/`#[norange]` tokens
   does not change the existing powerline separators, edge blending, palette
   cycling, or `#[default]` terminator.
3. **`RenderInput` serializes the full `Context`** (so `toggled` reaches guests).
4. **`Context` deserialization is total w.r.t. `toggled`** — a JSON without the
   field yields an empty set (`#[serde(default)]`), so host/guest version skew
   never breaks a guest.
5. Existing load-bearing invariants stay intact: #1 (Context is the sole render
   input — `toggled` is read at the build edge, not mid-render), #2 (wire types
   serde-serializable — `toggled` is serde; `Segment` unchanged), #3
   (`Config::load` total — `alt_format` is `#[serde(default)]`), #4 (init
   injection-safe — the new binding uses `#{q:}` + `--flag=`).

## Testing (TDD)

Core (`rustline-core`):
- `active_format`: toggled+non-empty alt → alt; toggled+empty alt → format; not
  toggled → format.
- Per-widget (at least `cpu`): with `alt_format` set and `NAME ∈ ctx.toggled`,
  renders the alt view; else the normal view; `range_name()` is `Some("cpu")`
  only when `alt_format` non-empty, else `None`; a name > 15 bytes yields `None`.
- Range render: a two-clickable-widget region emits `#[range=user|<name>]` …
  `#[norange]` around each; a non-clickable widget emits none. **Non-destructive
  characterization:** stripping the `#[range=…]`/`#[norange]` tokens from the
  output equals the pre-feature `render_region` output (same separators/edges/
  `#[default]`). Separators sit outside ranges.
- `Context` serde: round-trips `toggled`; a JSON lacking `toggled` deserializes
  to an empty set.

Binary (`rustline`):
- `toggles.rs`: `parse_toggles` totality (missing/corrupt/blank-line inputs);
  `parse ∘ serialize == identity`; `apply_toggle` adds then removes.
- `click`: toggling a name adds it, toggling again removes it (via the pure
  helpers, no real FS needed for the logic); empty range is a no-op.
- `init_block`: contains the `MouseDown1Status` binding, the `select-window -t=`
  window branch, `rustline click --range=#{q:mouse_status_range}`, and the mouse
  hint comment — **and still** every prior assertion (all regions/hooks/
  `#{q:}`-escaping). No bare unescaped `mouse_status_range`.
- `build_context`: `Context.toggled` is populated from the state file (smoke).

Host seam (`rustline-wasm`, hermetic): serializing `RenderInput` yields JSON in
which `context.toggled` is present, so a guest can read it.

Plugin (`plugins/weather`): the guest-side format-selection helper picks
`alt_format` when its name is toggled (host-target unit test); full e2e behind
`wasm-e2e`.

External dependency (not unit-testable): tmux ≥ 3.1 resolves `#[range=user|X]`
and returns `X` in `#{mouse_status_range}`. Because range markup is emitted only
for widgets a user opted into (`alt_format` set), pre-3.1 or zero-config users
see byte-identical output. Documented as a requirement; verified manually / via
the wasm-e2e/preview path.

## Documentation updates

Per project convention, update **both** `CLAUDE.md` and `README.md` (widget/
config lists, module map, invariants) to cover: the `alt_format` field, the
`rustline click` subcommand, `Context.toggled`, the `Widget::range_name` seam,
the toggles state file, the new `init` mouse binding, and the tmux ≥ 3.1
requirement. Add a "Roadmap: done" line and keep the `left_click`/`right_click`
+ widget-management TUI as future items.
