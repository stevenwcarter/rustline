# rustline

A fast, native tmux statusline written in Rust — powerline-style segments for
pane, window, host, and system info, with zero required configuration.

## Features

- Powerline-style segments (pane id, hostname, window list, cwd, load average,
  date/time) rendered as a single static binary — no shell scripts, no
  external statusline framework.
- Zero-config: sensible defaults out of the box.
- Optional TOML config to reorder widgets or tweak per-widget options.
- Opt-in `lan_ip` and `tailscale_ip` widgets that show the machine's LAN and
  Tailscale IPv4 addresses.
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
and adds `after-select-pane` / `after-select-window` hooks that call
`refresh-client -S` so the bar updates immediately when you switch panes or
windows, not just on the next tick.

> **Font requirement:** the powerline separators are drawn with Powerline
> glyphs (U+E0B0–U+E0B3). Use a Nerd Font or another powerline-patched font
> in your terminal, or the separators will show as boxes/blanks.

## Default layout

With no config file, `rustline` renders:

- **Left:** pane id · hostname — e.g. `0:0.0` · `myhost`
- **Center:** window list, active window emphasized — e.g. `0* zsh  1 vim`
- **Right:** current directory (`$HOME` abbreviated to `~`) · load average ·
  date/time — e.g. `~/src/rustline` · `0.31 0.44 0.42` · `Mon < 2026-07-20 < 19:04`

## Configuration

Config is optional TOML at `~/.config/rustline/config.toml` (or
`$XDG_CONFIG_HOME/rustline/config.toml` when set). A missing or invalid file
just falls back to the defaults above — `rustline` never fails to render
because of a bad config.

Widget names available for the `layout` arrays are: `pane_id`, `hostname`,
`windows`, `cwd`, `loadavg`, `datetime`, and the opt-in `lan_ip` /
`tailscale_ip` (see [IP address widgets](#ip-address-widgets) below).

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
Tailscale interface are skipped); set `interface` to force a specific NIC.

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
[IP widgets](docs/superpowers/specs/2026-07-20-rustline-ip-widgets-design.md).

## License

MIT — see [`LICENSE`](LICENSE).
