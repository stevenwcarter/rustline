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
- Opt-in `git` widget showing the current branch (or short SHA when
  detached), a dirty marker, and ahead/behind/staged/unstaged counts, read via
  a `git status` shell-out.
- Opt-in `disk` widget showing filesystem usage (used/total/available, a
  percentage, and a gauge bar) for a configured mount, read via `statvfs(2)`.
- Click-to-toggle widget alt views: give a widget an `alt_format` and
  left-clicking it in the status line swaps it to that view (e.g. a compact
  `cpu` reading toggling to one with a gauge bar).
- Seven built-in themes (a `default` plus six multi-accent, truecolor curated
  themes) selectable via `rustline theme use`, browsable interactively with
  `rustline theme pick`, plus a `theme new` scaffolder for tweaking your own
  — see [Themes](#themes) below.
- Semantic colors (`success`/`info`/`warning`/`error`) reach both built-in
  widgets and WASM plugins; `cpu`, `memory`, `battery`, `loadavg`, and `disk`
  turn into an alert badge when a configurable threshold is crossed.
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

Run the onboarding wizard — it asks a few questions (theme, one- or two-line
status, mouse/click-to-toggle, which widgets, clock style, refresh rate), then
writes `~/.config/rustline/config.toml` and adds a managed block to
`~/.tmux.conf` (backed up first):

```bash
rustline init
tmux source-file ~/.tmux.conf
```

The wizard wires `rustline render` into `status-left` / `status-right` and the
window list, sets `status-interval` (1s or 5s, your choice), adds
`after-select-pane` / `after-select-window` hooks that call `refresh-client -S`
so the bar updates immediately when you switch panes or windows, and — if you
opt in — binds a left click on any clickable widget to toggle its `alt_format`
(see [Click-to-toggle widget views](#click-to-toggle-widget-views) below) and
turns on `set -g mouse on` for you.

- `rustline init --defaults` — non-interactive; recommended settings.
- `rustline init --print` — just print the raw one-line tmux block to stdout
  and write nothing (the pre-wizard behavior, handy for scripting:
  `rustline init --print >> ~/.tmux.conf`).

The tmux block is wrapped in `# >>> rustline >>>` / `# <<< rustline <<<`
markers, so re-running `rustline init` replaces that region instead of
appending a duplicate; your edits outside the markers are preserved. The
generated `config.toml` is merged non-destructively too: `[theme].base` is
always set to your pick, but existing `[layout]`/`[widgets.*]` sections you've
already customized are left alone.

> **Font requirement:** the powerline separators are drawn with Powerline
> glyphs (U+E0B0–U+E0B3). Use a Nerd Font or another powerline-patched font
> in your terminal, or the separators will show as boxes/blanks.
>
> **tmux requirement:** click-to-toggle needs tmux **≥ 3.1** (status-line
> click ranges) and `set -g mouse on` in your tmux config — the wizard's mouse
> question can turn that on for you, or you can set it yourself.

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
below), `battery` (see [Battery widget](#battery-widget) below), `git`
(see [Git widget](#git-widget) below), and `disk` (see [Disk
widget](#disk-widget) below).

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
`alt_format` for [click-to-toggle](#click-to-toggle-widget-views). It's also
[threshold-aware](#themes): `warn_percent`/`crit_percent` (default 20/10)
alert while discharging at or below those levels.

```toml
[layout]
right = ["battery", "cwd", "loadavg", "datetime"]

[widgets.battery]
format = "{icon} {percent}%"   # {icon}, {percent}, {state}
down_format = ""               # shown when no battery (desktops); default: nothing
warn_percent = 20              # default; alert badge at/below this % while discharging
crit_percent = 10              # default; 0 disables a tier
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
[click-to-toggle](#click-to-toggle-widget-views). Both are also
[threshold-aware](#themes): `warn_percent`/`crit_percent` (cpu default 80/95,
memory default 80/92) alert at or above those levels.

```toml
[widgets.cpu]
format = "{icon} {bar} {percent}%"   # default "{icon} {percent}%"
bar_width = 8
warn_percent = 80   # default; 0 disables a tier
crit_percent = 95   # default

[widgets.memory]
format = "{icon} {used}/{total}"     # default; or "{icon} {bar} {percent}%"
bar_width = 8
warn_percent = 80   # default; 0 disables a tier
crit_percent = 92   # default
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
[click-to-toggle](#click-to-toggle-widget-views). It's also
[threshold-aware](#themes) on `load1` via `warn_load`/`crit_load` — unlike the
other numeric widgets, both default to `0.0` (off), since an absolute load
threshold depends on core count.

```toml
[widgets.loadavg]
format      = "{load1} {load5} {load15}"          # default
alt_format  = "{load1:.1} {load5:.1} {load15:.1}" # left-click toggles to this
warn_load   = 0.0   # default (off); e.g. 4.0 on a 4-core box
crit_load   = 0.0   # default (off)
```

### Git widget

An opt-in `git` built-in shows the current branch (or a 7-character short SHA
when `HEAD` is detached), a dirty marker, and ahead/behind/staged/unstaged
counts for the pane's working directory, read by shelling out to `git status
--porcelain=v2 --branch`. Not in the default layout — add it to a `layout`
region to use it. When `git` is missing, the pane isn't inside a repository,
or the read fails, it renders nothing by default.

Takes a `format` with `{branch}`, `{ahead}`, `{behind}`, `{staged}`,
`{unstaged}`, and `{dirty}` placeholders (`{dirty}` substitutes `dirty_glyph`
when there's any staged or unstaged change, else nothing), a `down_format`
shown when there's no git reading (default empty — same collapse-to-nothing
behavior as the other widgets' `down_format`), and an `alt_format` for
[click-to-toggle](#click-to-toggle-widget-views).

```toml
[layout]
right = ["git", "cwd", "loadavg", "datetime"]

[widgets.git]
format      = " {branch}{dirty}"   # default: Nerd-Font branch glyph
dirty_glyph = "*"                        # default
down_format = ""                         # shown outside a repo; default: nothing
```

### Disk widget

An opt-in `disk` built-in shows filesystem usage for a configured mount
(default `/`), read via `statvfs(2)`. Not in the default layout — add it to a
`layout` region to use it. When the mount can't be `statvfs`'d, it renders
nothing by default.

Takes a `mount` (default `/`), a `format` (default `" {used}/{total}"`, no
icon) with `{used}`/`{total}`/`{avail}` (human-readable binary sizes, e.g.
`6.2G`), `{percent}`, `{bar}` (a `bar_width`-cell, default 8, Unicode
gauge — the same one `cpu`/`memory` use), and `{mount}` (the configured mount
string itself) placeholders, a `down_format` shown when there's no disk
reading (default empty — same collapse-to-nothing behavior as the other
widgets' `down_format`), and an `alt_format` for
[click-to-toggle](#click-to-toggle-widget-views). It's also
[threshold-aware](#themes): `warn_percent`/`crit_percent` (default 85/95)
alert at or above those levels.

```toml
[layout]
right = ["disk", "cwd", "loadavg", "datetime"]

[widgets.disk]
mount       = "/"                 # default
format      = " {used}/{total}"   # default
bar_width   = 8
down_format = ""                  # shown when the mount can't be read; default: nothing
warn_percent = 85   # default; alert badge at/above this %
crit_percent = 95   # default; 0 disables a tier
```

### Click-to-toggle widget views

`datetime`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`, `memory`, `loadavg`,
`git`, and `disk` each take an `alt_format` (default empty). Give a widget a non-empty
`alt_format` and it becomes clickable: left-clicking it in the tmux status
line toggles it between `format` and `alt_format`, e.g. a compact CPU reading
swapping to one with a gauge bar:

```toml
[widgets.cpu]
format     = "{icon} {percent}%"
alt_format = "{icon} {bar} {percent}%"   # left-click toggles to this
```

This needs **tmux ≥ 3.1** and `set -g mouse on` (see [Enable in
tmux](#enable-in-tmux) above) — the `rustline init` wizard can turn mouse mode
on for you if you opt in, or `rustline init --print` just wires the click
handler and leaves mouse mode to you. Which widgets are currently toggled is
tracked globally (not per pane/session) in a small state file under
`$XDG_DATA_HOME/rustline`, written by the `rustline click` subcommand the tmux
binding invokes. WASM plugins can support this too: a plugin is clickable when
its name is 15 bytes or less (tmux's status-range name limit), and it decides
for itself whether to honor a click by checking `context.toggled` — the
bundled `weather` example does this via `options.alt_format`. Since a plugin's
name becomes a tmux `range=user|<name>` argument verbatim, pick one that is
≤ 15 bytes, isn't the reserved name `window`, and sticks to `[A-Za-z0-9_-]`.

## Themes

rustline ships seven built-in themes, selectable from the command line, plus
per-widget threshold alerts that use each theme's semantic colors.

> **Truecolor requirement:** the six curated themes (everything but
> `default`) use truecolor (24-bit RGB) values. You need a truecolor-capable
> terminal and tmux's `RGB`/`Tc` terminal feature enabled — e.g.
> `set -as terminal-features ",xterm-256color:RGB"` (tmux ≥ 3.2), or
> `set -ga terminal-overrides ",*256col*:Tc"` on older tmux — otherwise the
> colors will be approximated or look wrong.

- **`default`** — the original two-accent palette (unchanged).
- **`pastel-rainbow`** — the flagship: a six-color pastel palette with dark
  text.
- **`nord`**, **`gruvbox`**, **`catppuccin-mocha`**, **`tokyo-night`**,
  **`dracula`** — curated multi-accent ports of the popular color schemes.

```bash
rustline theme list                  # built-ins + your themes-dir files, active marked *
rustline theme show pastel-rainbow   # ANSI preview (with sample alert badges)
rustline theme use nord              # sets [theme].base = "nord" in config.toml
rustline theme new my-nord --from nord   # scaffold a tweakable copy to edit by hand
rustline theme pick                  # interactively browse previews, then set one
```

`theme pick` lists the themes (active marked, themes-dir files tagged
`(custom)`), lets you preview any by number (or `a`/`all` for every one), then
prompts you to set one by name or number (blank keeps the current theme).
Previews default to a **healthy** status line — just the theme's palette, the
way you'll actually see it day to day. Press `t` to toggle the warning/error
alert-badge colors on, so you can sample a theme's semantic colors, and `t`
again to turn them back off; toggling immediately re-shows the theme you last
previewed, so you see the change right away. It needs a terminal — a
non-interactive invocation prints a hint to use `theme show`/`theme use`
instead and exits non-zero without writing anything.

`theme new` writes a complete, commented theme file to
`$XDG_CONFIG_HOME/rustline/themes/<name>.toml` (fallback
`~/.config/rustline/themes`) with every field set to the seed theme's values —
edit any of them, then `rustline theme use <name>`. A themes-dir file always
**shadows** a built-in of the same name, so `rustline theme new nord` followed
by `rustline theme use nord` uses your tweaked copy.

Under the hood, `[theme].base` in `config.toml` selects the starting theme;
any individual `[theme]` field — the six window-pill fields (`win_current_bg`/
`win_current_fg`/`win_inactive_bg`/`win_inactive_fg`/`win_cap_left`/
`win_cap_right`), `palette`, `fg`, `bar_bg`, the separators, or the semantic
colors below — still overrides on top:

```toml
[theme]
base  = "nord"                 # a built-in name, or a *.toml stem in your themes dir
warning = { Named = "yellow" } # per-field overrides still apply on top of base
```

Every theme defines four **semantic colors** — `success`, `info`, `warning`,
`error` — available to widgets and WASM plugins alike. The `cpu`, `memory`,
`battery`, `loadavg`, and `disk` widgets use them for **threshold alerts**: cross a
configured `warn_*`/`crit_*` level (see each widget's section above) and the
whole segment flips to an inverse badge (bold text in the theme's `bar_bg`,
background in the semantic color) — critical always wins over warning. Set a
threshold to `0` to turn that tier off; `loadavg`'s thresholds default off
since a meaningful absolute load number depends on your core count.

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
[click-to-toggle widgets](docs/superpowers/specs/2026-07-21-rustline-click-toggle-widgets-design.md),
[themes/theme picker](docs/superpowers/specs/2026-07-21-rustline-themes-theme-picker-design.md).

## License

MIT — see [`LICENSE`](LICENSE).
