# rustline

A fast, native tmux statusline written in Rust — powerline-style segments for
pane, window, host, and system info, with zero required configuration.

## Features

- Powerline-style segments (pane id, hostname, window list, cwd, cpu, memory,
  load average, date/time) rendered as a single static binary — no shell
  scripts, no external statusline framework.
- Zero-config: sensible defaults out of the box.
- Optional TOML config to reorder widgets or tweak per-widget options.
- Built-in `cpu` and `memory` widgets (in the default right layout) showing
  usage percentage, human-readable sizes, and a Unicode gauge bar (`{bar}`).
- Opt-in `lan_ip` and `tailscale_ip` widgets that show the machine's LAN and
  Tailscale IPv4 addresses.
- Opt-in `battery` widget showing charge percentage, state, and a
  level-bucketed Nerd-Font icon (Linux + macOS).
- Click-to-toggle widget alt views: give a widget an `alt_format` and
  left-clicking it in the status line swaps it to that view (e.g. a compact
  `cpu` reading toggling to one with a gauge bar).
- Instant refresh on pane/window switch via tmux hooks (no waiting for the
  next `status-interval` tick).

## Install

```bash
cargo build --release
```

This produces `target/release/rustline`. Copy or symlink it onto your `PATH`,
e.g.:

```bash
cp target/release/rustline ~/.local/bin/rustline
```

## Enable in tmux

```bash
rustline init >> ~/.tmux.conf
tmux source-file ~/.tmux.conf
```

`rustline init` prints a tmux config block that wires `rustline render` into
`status-left` / `status-right` and the window list, sets `status-interval 1`,
adds `after-select-pane` / `after-select-window` hooks that call
`refresh-client -S` so the bar updates immediately when you switch panes or
windows, not just on the next tick, and binds a left click on any clickable
widget to toggle its `alt_format` (see [Click-to-toggle widget
views](#click-to-toggle-widget-views) below).

> **Font requirement:** the powerline separators are drawn with Powerline
> glyphs (U+E0B0–U+E0B3). Use a Nerd Font or another powerline-patched font
> in your terminal, or the separators will show as boxes/blanks.
>
> **tmux requirement:** click-to-toggle needs tmux **≥ 3.1** (status-line
> click ranges) and `set -g mouse on` in your tmux config — `rustline init`
> does not turn mouse mode on itself, it only binds the click once mouse mode
> is enabled.

## Default layout

With no config file, `rustline` renders:

- **Left:** pane id · hostname — e.g. `0:0.0` · `myhost`
- **Center:** window list, active window emphasized — e.g. `0* zsh  1 vim`
- **Right:** current directory (`$HOME` abbreviated to `~`) · cpu · memory ·
  load average · date/time — e.g. `~/src/rustline` · `󰘚 37%` · `󰍛 6.2G/16G` ·
  `0.31 0.44 0.42` · `Mon < 2026-07-20 < 19:04`

## Configuration

Config is optional TOML at `~/.config/rustline/config.toml` (or
`$XDG_CONFIG_HOME/rustline/config.toml` when set). A missing or invalid file
just falls back to the defaults above — `rustline` never fails to render
because of a bad config.

Widget names available for the `layout` arrays are: `pane_id`, `hostname`,
`windows`, `cwd`, `cpu`, `memory` (see [CPU and memory
widgets](#cpu-and-memory-widgets) below), `loadavg`, `datetime`, and the
opt-in `lan_ip` / `tailscale_ip` (see [IP address widgets](#ip-address-widgets)
below) and `battery` (see [Battery widget](#battery-widget) below).

Example — reorder the right region and change the clock format:

```toml
[layout]
right = ["datetime", "cwd"]

[widgets.datetime]
format = "%H:%M"
```

Run `rustline print-config` to print the fully-resolved effective
configuration (your overrides layered onto the defaults) as TOML.

### IP address widgets

Two opt-in built-ins show the machine's addresses: `lan_ip` (your LAN IPv4) and
`tailscale_ip` (your Tailscale IPv4, the `100.64.0.0/10` address). Neither is in
the default layout — add either to a `layout` region to use it.

Each takes a `format` where `{ip}` is replaced by the address and any
surrounding label or glyph is printed verbatim, and a `down_format` shown when
the address isn't available (default empty — the widget then renders nothing
rather than a stale or fake address). `lan_ip` auto-picks the first private,
non-virtual interface (container/VM bridges like `docker0`/`virbr0` and the
Tailscale interface are skipped); set `interface` to force a specific NIC. Both
also take an `alt_format` for [click-to-toggle](#click-to-toggle-widget-views).

```toml
[layout]
right = ["lan_ip", "tailscale_ip", "cwd", "loadavg", "datetime"]

[widgets.lan_ip]
format = "LAN {ip}"        # {ip} -> 192.168.1.20; or a glyph, e.g. "󰈀 {ip}"
# interface = "wlp3s0"     # optional; omit to auto-pick

[widgets.tailscale_ip]
format = "TS {ip}"
down_format = "TS off"     # shown when Tailscale is down; omit to render nothing
```

### Battery widget

An opt-in `battery` built-in shows charge percentage, state, and a
level-bucketed, charging-aware Nerd-Font icon. It works on Linux (sysfs) and
macOS (`pmset`); on any other platform, or a host with no battery, it renders
nothing by default.

Takes a `format` where `{icon}`, `{percent}`, and `{state}` are replaced, and
a `down_format` shown when there's no battery reading (default empty — same
collapse-to-nothing behavior as the IP widgets' `down_format`), plus an
`alt_format` for [click-to-toggle](#click-to-toggle-widget-views).

```toml
[layout]
right = ["battery", "cwd", "loadavg", "datetime"]

[widgets.battery]
format = "{icon} {percent}%"   # {icon}, {percent}, {state}
down_format = ""               # shown when no battery (desktops); default: nothing
```

### CPU and memory widgets

`cpu` and `memory` are built-in and **in the default right layout** (unlike
the opt-in widgets above) — they show live CPU utilization and memory usage,
each with a Unicode gauge bar.

`cpu` takes a `format` (default `"{icon} {percent}%"`) with `{icon}`
(nf-md-chip), `{percent}`, and `{bar}` placeholders. `memory` takes a `format`
(default `"{icon} {used}/{total}"`) with `{icon}` (nf-md-memory),
`{used}`/`{total}`/`{avail}` (human-readable binary sizes, e.g. `6.2G`),
`{percent}`, and `{bar}` placeholders. `{bar}` is a `bar_width`-cell (default
8) Unicode block-eighths gauge shared by both widgets. Both also take a
`down_format` (default empty) shown on an unsupported platform or a failed
read — same collapse-to-nothing behavior as the `battery` widget's
`down_format` — and an `alt_format` for
[click-to-toggle](#click-to-toggle-widget-views).

```toml
[widgets.cpu]
format = "{icon} {bar} {percent}%"   # default "{icon} {percent}%"
bar_width = 8

[widgets.memory]
format = "{icon} {used}/{total}"     # default; or "{icon} {bar} {percent}%"
bar_width = 8
```

### Load average widget

`loadavg` is built-in and **in the default right layout** — it shows the
1/5/15-minute system load average (nothing on platforms where it can't be read,
rather than fake zeros).

Takes a `format` with `{load1}`/`{load5}`/`{load15}` placeholders, each of which
accepts an inline Rust-style precision spec `:.N` — `{load1:.1}` → `0.4`. A bare
`{load1}` is two decimals (so the default renders exactly like older versions),
and `N` is clamped to 0–10. Also takes a `down_format` (default empty, shown
when the load can't be read) and an `alt_format` for
[click-to-toggle](#click-to-toggle-widget-views).

```toml
[widgets.loadavg]
format      = "{load1} {load5} {load15}"          # default
alt_format  = "{load1:.1} {load5:.1} {load15:.1}" # left-click toggles to this
```

### Click-to-toggle widget views

`datetime`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`, `memory`, and `loadavg`
each take an `alt_format` (default empty). Give a widget a non-empty
`alt_format` and it becomes clickable: left-clicking it in the tmux status
line toggles it between `format` and `alt_format`, e.g. a compact CPU reading
swapping to one with a gauge bar:

```toml
[widgets.cpu]
format     = "{icon} {percent}%"
alt_format = "{icon} {bar} {percent}%"   # left-click toggles to this
```

This needs **tmux ≥ 3.1** and `set -g mouse on` (see [Enable in
tmux](#enable-in-tmux) above) — `rustline init` wires the click handler but
never turns mouse mode on itself. Which widgets are currently toggled is
tracked globally (not per pane/session) in a small state file under
`$XDG_DATA_HOME/rustline`, written by the `rustline click` subcommand the tmux
binding invokes. WASM plugins can support this too: a plugin is clickable when
its name is 15 bytes or less (tmux's status-range name limit), and it decides
for itself whether to honor a click by checking `context.toggled` — the
bundled `weather` example does this via `options.alt_format`. Since a plugin's
name becomes a tmux `range=user|<name>` argument verbatim, pick one that is
≤ 15 bytes, isn't the reserved name `window`, and sticks to `[A-Za-z0-9_-]`.

## Logging

rustline writes logs to `~/.local/share/rustline/rustline.log`
(`$XDG_DATA_HOME/rustline/rustline.log`) at `info` by default, and error-level
messages to stderr. The file rotates to `rustline.log.1` once it exceeds 5 MiB.

Raise the file verbosity with repeated `-v` (file sink only):

| flag    | file level |
|---------|-----------|
| (none)  | info       |
| `-v`    | warn       |
| `-vv`   | info       |
| `-vvv`  | debug      |
| `-vvvv` | trace      |

Override either sink in `config.toml` (`RUST_LOG` is not used):

    [log]
    file_level   = "info"    # off|error|warn|info|debug|trace
    stderr_level = "error"
    file         = "~/.local/share/rustline/rustline.log"   # optional

## Previewing on the command line

Every render command accepts `--preview`, which prints the region in ANSI colour
instead of raw tmux markup — handy for eyeballing the bar without wiring it into
tmux:

```bash
rustline render left --preview --session=0 --window=1 --pane=0 --pane-path="$PWD"
```

A [`just`](https://just.systems) recipe previews the whole bar (left region,
window list, right region) at once — using your live tmux context when run
inside tmux, and sample values otherwise:

```bash
just preview
```

Other recipes: `just build`, `just test`, `just lint`.

## Benchmarking

`rustline bench` (build with `--features bench`, or run `just bench`) times the
render pipeline: a **pure** pass on a fabricated `Context` (no OS reads, so no
`read_cpu` ~120 ms sample) versus a **real-world** pass that pays the real
reads, plus per-widget, per-data-source, and per-plugin breakdowns as tables.

```sh
just bench                       # all groups
just bench --only widgets        # just the per-widget render costs
cargo run --features bench -- bench --format markdown --output bench.md
```

Plugin passes run against the real, preserved plugin state/cache, so a cached
plugin (e.g. `weather`) is measured on its fast cached path rather than
re-fetching every iteration.

## Plugins

Third-party widgets can be added as WASM plugins. A plugin is a small wasm
module (built for `wasm32-unknown-unknown` with the [Extism PDK][extism-pdk])
that exports a `name` function and a `render(context, config) -> Segment[]`
function — the same `Context` in, `Segment`s out contract as a built-in
widget, just crossing the wasm boundary as JSON.

Everything a plugin can touch is capability-gated by the host: network
requests and arbitrary file paths are checked against per-plugin allowlists in
your config (`allowed_urls` / `allowed_paths`, each a glob or a `re:`-prefixed
regex; `re:` patterns are anchored to a full-string match, so include `.*` for a
prefix, e.g. `re:https://wttr\.in/.*`), and each plugin gets its own sandboxed
state directory with a size
quota (`max_state_bytes`, default 50 MB) for caching data between renders. The
host also exposes a TTL-cached HTTP GET, so a plugin can fetch remote data at
most once per interval without managing its own cache — the bundled `weather`
example uses it. A plugin has no ambient access to anything — a disallowed
request is simply refused, and any plugin error, timeout, or crash renders as
an empty segment rather than breaking the status line.

Build and install the bundled `weather` example (a Nerd-Font condition icon +
°F for a configured zip code, fetched from wttr.in at most once per
`refresh_secs`):

```bash
just build-weather
```

Then add it to your layout and give it a URL allowlist:

```toml
[layout]
right = ["weather", "cwd", "loadavg", "datetime"]

[plugins.weather]
allowed_urls = ["https://wttr.in/*"]

[plugins.weather.options]
zip = "48183"
format = "{icon} {temp_f}°F"
refresh_secs = 1800
```

Manage a plugin's allowlists from the command line without hand-editing TOML:

```bash
rustline plugin list
rustline plugin url add weather "https://wttr.in/*"
```

See the [design spec](docs/superpowers/specs/2026-07-20-rustline-wasm-plugins-design.md)
for the full capability model, config schema, and plugin ABI.

[extism-pdk]: https://github.com/extism/rust-pdk

## Design

See the full design specs:
[core statusline](docs/superpowers/specs/2026-07-20-rustline-tmux-statusline-design.md),
[WASM plugins](docs/superpowers/specs/2026-07-20-rustline-wasm-plugins-design.md),
[IP widgets](docs/superpowers/specs/2026-07-20-rustline-ip-widgets-design.md),
[CPU/memory widgets](docs/superpowers/specs/2026-07-21-rustline-cpu-memory-widgets-design.md),
[click-to-toggle widgets](docs/superpowers/specs/2026-07-21-rustline-click-toggle-widgets-design.md).

## License

MIT — see [`LICENSE`](LICENSE).
