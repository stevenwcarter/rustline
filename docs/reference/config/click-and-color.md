# Config — colors, click-to-toggle, click bindings

> **Read when:** changing per-widget `fg`/`bg` overrides, `alt_format` toggling, per-button click bindings, or the global toggle state file.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

**Per-widget color override (`fg`/`bg`):** every format-bearing widget accepts
optional `fg`/`bg` keys (W29) that pin its foreground/background color,
flattened directly into its `[widgets.<name>]` table. Applied centrally in
`render_named_region` — after the widget renders, before `assign_palette` fills
the cycling palette — so widgets stay `Context`-only (invariant #1). `bg` only
takes effect on a segment that doesn't already carry an explicit background
(the same rule `assign_palette` follows for an alert badge); `fg` applies
wherever set. Both default to `None`, so an unset override is byte-identical to
before (invariant #3). Colors are `Color` enums (`{ Indexed = N }` /
`{ Named = "cyan" }` / `{ Rgb = [r,g,b] }`).

```toml
[widgets.cwd]
fg = { Indexed = 250 }
bg = { Rgb = [40, 44, 52] }
```

**Click-to-toggle widget views:** the twelve format-bearing widgets —
`datetime`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`, `memory`, `loadavg`,
`git`, `disk`, `uptime`, `media`, `throughput` — each take an additional `alt_format` (default `""`, `#[serde(default)]`, so
covered by invariant #3 like every other opt). A non-empty `alt_format` makes
that widget clickable: left-clicking it in the tmux status line toggles it
between `format` and `alt_format`.

```toml
[widgets.cpu]
format     = "{icon} {percent}%"
alt_format = "{icon} {bar} {percent}%"   # left-click toggles to this
```

**Per-button click bindings (`[widgets.<name>.click]`):** beyond the default
left-click toggle, any widget can bind a specific mouse button to a
`toggle`/`open_url`/`run` action (W36), flattened as
`left_click`/`right_click`/`middle_click` into its `[widgets.<name>]` table.
The default action fires only when no binding matches, so an unconfigured
widget is byte-identical to before (invariant #3).

```toml
[widgets.cpu]
right_click = { run = "tmux display-popup -E htop" }
middle_click = { open_url = "https://grafana.example/host" }
# left_click = { toggle = false }   # explicitly disable the default left toggle
```

**Important — a `run`/`open_url` binding needs the widget to emit a clickable
range.** A widget only becomes a tmux click target when it emits
`#[range=user|NAME]`, which today happens iff it has a non-empty `alt_format`
(or it's a plugin/unknown name). So a `run`/`open_url` binding on a widget with
no `alt_format` resolves correctly but never fires — tmux never sends a click
for a widget that emits no range. Give such a widget an `alt_format` (even a
throwaway one) to make it clickable, or wait for a future range-on-binding
change. Left-click `run`/`open_url` on an `alt_format` widget works today;
right/middle now fire via the new `MouseDown2/3Status` bindings.

Toggle state is **global**, not per-widget-instance or per-session: one flat,
newline-delimited set of toggled widget/plugin names at
`$XDG_DATA_HOME/rustline/toggles` (fallback `~/.local/share/rustline/toggles`),
read once into `Context.toggled` at Context-build time and flipped by
`rustline click --range=<name> [--button=left]` (see CLI above). WASM plugins
participate the same way — `Context.toggled` rides the JSON boundary to the
guest, and a plugin honors toggling by checking whether its own `name()` is a
member; the `weather` example demonstrates this via `options.alt_format`. Any
widget or plugin name longer than 15 bytes is simply not clickable (tmux's
`range=user|X` byte cap — see tmux integration model above).

A plugin author should pick a name (the `.wasm` stem) that is ≤ 15 bytes,
avoids the reserved name `window`, and sticks to `[A-Za-z0-9_-]`, since it
becomes a tmux `range=user|<name>` argument verbatim. Also avoid the handful
of host-owned state-file names the CLI writes flat under `<state_root>`, each
of which would collide with a plugin's own same-named state directory:
`cpu-sample` (the `cpu` widget's snapshot cache), `cpu-history`/
`memory-history` (the `{spark}` history rings, W45), `throughput-sample`/
`throughput-sample-<iface>` (the `throughput` widget's prior counter sample —
one file for the aggregate read, one per named interface once multiple
`throughput` instances exist, W46/W47), `wasmtime-cache`/
`wasmtime-cache.toml` (the WASM compile cache dir + its config, W43 — kept
deliberately distinct from any plugin state subdir), `plugin-index.json` (the
24h-TTL cached copy of the curated plugin index, W49 — see
`plugin_index.rs` above), and `daemon.sock` (the
optional persistent daemon's Unix socket under `$XDG_RUNTIME_DIR/rustline/`,
falling back to `<state_root>/daemon.sock`, W48). Every collision degrades
gracefully (a failed create/rename is `warn!`-logged, never panics), but is
still best avoided.

