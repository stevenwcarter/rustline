# Config — layout and widget instances

> **Read when:** changing the default layout, touching `[layout]` semantics (including the inert `center` region), or working with `[instances.<name>]` multi-instance widgets.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

Optional TOML at `~/.config/rustline/config.toml` (or
`$XDG_CONFIG_HOME/rustline/config.toml`). Zero-config works. Default layout:
left = `[pane_id, hostname]`, center = `[windows]`,
right = `[cwd, cpu, memory, loadavg, datetime]`. Default datetime format
`"%a < %Y-%m-%d < %H:%M"` (the `<` are literal). `datetime` also takes an
optional `timezone` (an IANA zone name, e.g. `"America/New_York"`; default
`None` renders `ctx.now` in the local zone, unchanged from before this
option) that formats in that zone instead via `chrono-tz`; an unrecognized
name is logged and falls back to local time rather than erroring. Unknown
widget names in a layout are skipped, not fatal.

**`[layout].center` is inert.** Nothing in the render pipeline reads it:
`assemble.rs`'s `window_pill` (backing both `render_window` and
`render_windows`) hardcodes `registry.resolve(&["windows".to_string()])`
regardless of what `center` contains — tmux's window list is rendered by
that dedicated path, not by resolving the center layout array. The array
still exists and still round-trips (the default config is `center =
["windows"]`, and it must stay writable), and `rustline widget
enable|disable|move --region center` still writes and exits `0` — but it
prints a stderr note that the change won't affect the rendered bar, since a
silent no-op would be worse than an explained one. The interactive `widget
edit` TUI goes further and refuses a CENTER edit outright (with an
explanatory status line), since a "wrote but nothing happened" surprise is
costlier to discover at save time in an editor than at CLI-invocation time.
This is a known, deliberate limitation — not a bug to "fix" by making
`center` do something without first deciding what a configurable window
list would even mean.

**Multiple widget instances (`[instances.<name>]`):** the same widget *kind*
can appear more than once in a layout with distinct options — e.g. two
clocks in different timezones, or usage bars for two different disk mounts —
via a top-level `[instances.<name>]` table per instance. `kind` selects the
built-in widget kind to build; every other key in the table is that kind's
own option set (`format`, `alt_format`, `fg`/`bg`, click bindings, and
whatever else that kind takes — e.g. `timezone` for a `datetime` instance,
`mount` for a `disk` instance). Add the instance's own name (not the kind
name) to a `layout` array to place it.

```toml
[layout]
right = ["cwd", "clock_utc", "datetime", "disk_data"]

[instances.clock_utc]
kind = "datetime"
timezone = "UTC"
format = "%H:%MZ"

[instances.disk_data]
kind = "disk"
mount = "/data"
format = " {used}/{total}"
```

Only the twelve click-toggleable/format-bearing kinds (`datetime`, `lan_ip`,
`tailscale_ip`, `battery`, `cpu`, `memory`, `loadavg`, `git`, `disk`,
`uptime`, `media`, `throughput`) can be instanced this way; `cwd`, `hostname`,
`pane_id`, and `windows` are not (they carry no clickable identity to give an
instance). An instance name follows the same range-safe rule as a widget's
click-toggle name (`[A-Za-z0-9_-]`, at most 15 bytes, to stay a valid tmux
`range=user|NAME`), and **may not shadow a built-in widget's name** — an
`[instances.cpu]` entry, say, is always skipped (with a `warn!`) in favor of
the real `cpu` built-in, the same "built-in wins" precedence a same-named
WASM plugin loses to. An instance with an unknown/missing `kind`, or a name
colliding with a built-in or an earlier instance, is likewise skipped rather
than breaking config load (invariant #3). Each instance's own `name` becomes
its click-toggle/range identity end-to-end (invariant #7) — its
`Context.toggled` key, its `range=user|NAME`, and its `active_format` lookup
are all the *instance* name, not the kind.

Note: a per-invocation `rustline render --plugin-dir=X` is silently ignored
while the optional daemon (see below) is reachable — the daemon always
serves with whatever `--plugin-dir` it was started with, since the wire
protocol carries no per-request override.

