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
  usage percentage, human-readable sizes, a Unicode gauge bar (`{bar}`), and an
  optional history sparkline (`{spark}`).
- Opt-in `throughput` widget showing network download/upload rates
  (`/proc/net/dev`, Linux).
- Opt-in `lan_ip` and `tailscale_ip` widgets that show the machine's LAN and
  Tailscale IPv4 addresses.
- Opt-in `battery` widget showing charge percentage, state, and a
  level-bucketed Nerd-Font icon (Linux + macOS).
- Opt-in `git` widget showing the current branch (or short SHA when
  detached), a dirty marker, and ahead/behind/staged/unstaged counts, read via
  a `git status` shell-out.
- Opt-in `disk` widget showing filesystem usage (used/total/available, a
  percentage, and a gauge bar) for a configured mount, read via `statvfs(2)`.
- Opt-in `uptime` widget (humanized, e.g. `3d 4h`) and `media` now-playing
  widget (artist/title/status, via `playerctl`).
- Click-to-toggle widget alt views: give a widget an `alt_format` and
  left-clicking it in the status line swaps it to that view (e.g. a compact
  `cpu` reading toggling to one with a gauge bar). You can also bind a specific
  mouse button (left/middle/right) to open a URL or run a command.
- Per-widget color overrides (`fg`/`bg`) to pin a widget's colors.
- Run the same widget more than once with different options via
  `[instances.<name>]` — e.g. two clocks in different timezones, or gauges
  for more than one disk mount.
- Manage your layout without hand-editing TOML: `rustline widget
  list|enable|disable|move`, or `rustline widget edit` for a full-screen
  interactive editor with a live preview — bound to `prefix + W` in tmux out
  of the box. See [Widget manager](#widget-manager) below.
- Optional persistent daemon (`rustline daemon run`) that keeps your config,
  theme, widget registry, and WASM plugins warm across tmux refreshes;
  renders fall back to the normal in-process path automatically whenever
  it's not running.
- Install third-party WASM plugins straight from GitHub
  (`rustline plugin install owner/repo`), then grant only the capabilities they
  need — including, if you choose to, running a local command
  (`allowed_commands`). See [Plugins](#plugins) below.
- Seven built-in themes (a `default` plus six multi-accent, truecolor curated
  themes) selectable via `rustline theme use`, browsable interactively with
  `rustline theme pick`, plus a `theme new` scaffolder for tweaking your own
  — see [Themes](#themes) below.
- Semantic colors (`success`/`info`/`warning`/`error`) reach both built-in
  widgets and WASM plugins; `cpu`, `memory`, `battery`, `loadavg`, and `disk`
  turn into an alert badge when a configurable threshold is crossed.
- Instant refresh on pane/window switch via tmux hooks (no waiting for the
  next `status-interval` tick).
- `rustline doctor` checks your setup (tmux version, mouse mode, a truecolor
  terminal, `rustline` on `$PATH`, the managed tmux-conf block) and reports
  pass/warn/fail; `rustline completions <bash|zsh|fish>` prints a
  shell-completion script.

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
window list — in two-line mode the window list renders in a single batched
`rustline render windows --session=<s>` call instead of one process per window
(one-line mode still renders each window separately) — sets `status-interval`
(1s or 5s, your choice), adds
`after-select-pane` / `after-select-window` hooks that call `refresh-client -S`
so the bar updates immediately when you switch panes or windows, and — if you
opt in — binds a left click on any clickable widget to toggle its `alt_format`
(see [Click-to-toggle widget views](#click-to-toggle-widget-views) below) and
turns on `set -g mouse on` for you. It also binds `prefix + W` to open the
interactive widget manager in a tmux popup — see [Widget
manager](#widget-manager) below.

- `rustline init --defaults` — non-interactive; recommended settings.
- `rustline init --print` — just print the raw one-line tmux block to stdout
  and write nothing (the pre-wizard behavior, handy for scripting:
  `rustline init --print >> ~/.tmux.conf`).
- `rustline init --dry-run` — preview the config.toml and tmux block a real
  run would write (with a line diff against any existing file), without
  touching disk.
- `rustline init --uninstall` — remove the managed tmux block from
  `~/.tmux.conf` (backing it up first) and print the reload command; never
  touches `config.toml`. Combine with `--dry-run` to preview the removal
  instead of performing it.
- `rustline init --binary <path>` — override the binary path baked into the
  tmux block (default: the running binary's own resolved absolute path).

The tmux block is wrapped in `# >>> rustline >>>` / `# <<< rustline <<<`
markers, so re-running `rustline init` replaces that region instead of
appending a duplicate; your edits outside the markers are preserved. The
generated `config.toml` is merged non-destructively too: `[theme].base` is
always set to your pick, but existing `[layout]`/`[widgets.*]` sections you've
already customized are left alone. Note the tmux block calls `rustline` by its
resolved absolute path rather than a bare name, since the tmux server's own
shell may not share your interactive shell's `$PATH` — run `rustline doctor`
if the bar ever comes up empty.

Re-running `rustline init` later (say, to turn on mouse mode or switch
themes) seeds every question's shown default from what you chose last time
instead of resetting to the recommended answers, so you can hold Enter
through the questions you don't want to change. Your theme, clock style, and
which optional widgets you use come from your existing `config.toml`; the
one-/two-line, mouse, and refresh-interval answers are tmux settings that
config never stores, so those are read back out of the managed block in your
`~/.tmux.conf`. Then, before writing anything, it shows a summary of what you
answered plus a diff of both files against their current contents and asks
you to confirm.



> **Font requirement:** the powerline separators are drawn with Powerline
> glyphs (U+E0B0–U+E0B3). Use a Nerd Font or another powerline-patched font
> in your terminal, or the separators will show as boxes/blanks.
>
> **tmux requirement:** click-to-toggle needs tmux **≥ 3.1** (status-line
> click ranges) and `set -g mouse on` in your tmux config — the wizard's mouse
> question can turn that on for you, or you can set it yourself. The `prefix +
> W` widget-manager popup needs tmux **≥ 3.2** (`display-popup`); the status
> line itself still only needs ≥ 3.1. `rustline doctor` checks both and warns
> (never fails) if either is missing.

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

Widget names available for the `layout` arrays are: `pane_id` / `hostname`
(see [Hostname and pane ID widgets](#hostname-and-pane-id-widgets) below),
`windows`, `cwd` (see [Cwd widget](#cwd-widget) below), `cpu`, `memory` (see
[CPU and memory widgets](#cpu-and-memory-widgets) below), `loadavg`,
`datetime`, and the opt-in `lan_ip` / `tailscale_ip` (see [IP address
widgets](#ip-address-widgets) below), `battery` (see [Battery
widget](#battery-widget) below), `git` (see [Git widget](#git-widget)
below), `disk` (see [Disk widget](#disk-widget) below), `uptime` / `media`
(see [Uptime and media widgets](#uptime-and-media-widgets) below), and
`throughput` (see [Throughput widget](#throughput-widget) below). Most of
these can also appear more than once in your layout under a name of your
choosing — see [Multiple widget instances](#multiple-widget-instances) below.

Example — reorder the right region and change the clock format:

```toml
[layout]
right = ["datetime", "cwd"]

[widgets.datetime]
format = "%H:%M"
# timezone = "America/New_York"   # optional IANA zone; default renders local time
```

`datetime` also takes an optional `timezone` (an IANA zone name, e.g.
`"Europe/Berlin"`) that renders the clock in that zone instead of local time;
an unrecognized name is logged and falls back to local time rather than
erroring.

Run `rustline print-config` to print the fully-resolved effective
configuration (your overrides layered onto the defaults) as TOML.
`rustline config path` prints the resolved config file's path; `rustline
config edit` opens it in `$EDITOR` (creating it from the starter template
first if it doesn't exist); `rustline config validate` strictly parses it
and reports any error with its location, unlike `Config::load`'s silent
fallback to defaults. A global `--config <path>` flag points any subcommand
at a different config file.

### Widget manager

Edit which widgets are enabled and where they sit without hand-editing
`[layout]`:

```bash
rustline widget list                        # every widget + where it sits
rustline widget list --json                  # same, as JSON, for scripting
rustline widget enable git --region right    # add git to the right region
rustline widget disable loadavg              # remove a widget
rustline widget move git --region left       # relocate a placed widget
rustline widget edit                         # interactive full-screen editor
```

`enable`/`move` insert at the end of the target region by default; pass
`--index N` to place it elsewhere. An edit that would leave the layout
unchanged, target an already-placed widget, or name something unknown is
refused with an explanation and writes nothing.

`rustline widget edit` opens a full-screen `ratatui` editor (needs a
terminal): four columns — LEFT, CENTER, RIGHT, and AVAILABLE (everything not
currently placed) — navigated with arrow keys or vim keys (`h`/`j`/`k`/`l`).
`space`/`Enter` moves the selected widget between AVAILABLE and RIGHT (the
only region AVAILABLE placement appends into); `J`/`K` reorders it within
its current region; `H`/`L` (shifted, mirroring `J`/`K`) move a placed
widget directly to the other editable region (LEFT ↔ RIGHT — this is how
you get a widget into LEFT). A live preview strip along
the bottom shows the region you're editing, rendered exactly as it would
appear in tmux, updating as you make changes — before you've saved anything.
`w` writes your changes to `config.toml`; `q` quits (asking to confirm first
if you have unsaved changes); `?` shows a help overlay.

In tmux, `prefix + W` opens this editor in a popup (`rustline init` wires
this up for you — see [Enable in tmux](#enable-in-tmux) above), so you can
tweak your layout without leaving your session.

**CENTER is fixed.** tmux renders the window list there itself — nothing in
`rustline` reads `[layout].center` at render time — so the interactive editor
refuses to edit it (with an on-screen explanation) and the CLI (`widget
enable|move --region center`) still writes the change, to keep the default
`center = ["windows"]` round-trippable, but prints a note that it won't
affect what you see on the bar.

### Multiple widget instances

The same widget can appear more than once in your layout with different
options via a top-level `[instances.<name>]` table — pick any name you like,
set `kind` to the widget kind you want, and the rest of the table is that
kind's usual options. Add the instance's name (not the kind) to a `layout`
array to place it:

```toml
[layout]
right = ["clock_utc", "datetime", "disk_data", "disk"]

[instances.clock_utc]
kind      = "datetime"
timezone  = "UTC"
format    = "%H:%MZ"

[instances.disk_data]
kind  = "disk"
mount = "/data"
```

This works for any of the twelve widgets that support click-to-toggle
(`datetime`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`, `memory`, `loadavg`,
`git`, `disk`, `uptime`, `media`, `throughput`) — handy for a second clock in
another timezone, usage bars for more than one disk mount, or per-interface
throughput. An instance name follows the same rule as a click-toggle name
(letters/digits/`_`/`-`, at most 15 bytes) and can't reuse a built-in
widget's name — that name always wins, and a colliding or malformed instance
is skipped with a log warning rather than breaking your config.

### Hostname and pane ID widgets

`hostname` and `pane_id` are both in the default left layout. Each takes a
`format` (previously a fixed string): `hostname`'s (default `"{host}"`)
substitutes `{host}`, the hostname truncated at the first `.`; `pane_id`'s
(default `"{session}:{window}.{pane}"`) substitutes `{session}`/`{window}`/
`{pane}`. Any other text — e.g. a Nerd-Font icon or label — is printed
verbatim, and both defaults reproduce the pre-config output byte-for-byte.

```toml
[widgets.hostname]
format = " {host}"   # default "{host}"

[widgets.pane_id]
format = " {session}/{window}/{pane}"   # default "{session}:{window}.{pane}"
```

### Cwd widget

`cwd` is in the default right layout. Beyond the existing `abbreviate_home`
(default `true`, a leading `$HOME` becomes `~`), it now takes a `format`
(default `"{path}"`, substituting the — possibly shortened — path) plus two
more shortening options, applied in order (home-abbreviation, `abbreviate`,
`max_depth`, `max_len`, then the `format` substitution): `abbreviate`
(default `false`, fish-shell style — every path component but the last
shrinks to its first character, e.g. `~/src/rustline` → `~/s/rustline`),
`max_depth` (default `0` = unlimited; keeps only the last N `/`-separated
components, prefixing a leading `…/` when components are dropped), and
`max_len` (default `0` = unlimited; left-truncates the result to at most N
characters, prefixing a leading `…`). Every option at its default reproduces
the pre-feature output byte-for-byte.

```toml
[widgets.cwd]
format          = "{path}"   # default
abbreviate_home = true       # default
abbreviate      = true       # fish-style shortening of all but the last component
max_depth       = 3          # keep only the last 3 path components
max_len         = 30         # left-truncate to 30 characters
```

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
alert while discharging at or below those levels. An optional `icon`
overrides `{icon}` with a fixed glyph instead of the computed one (default:
`None`, keep the computed glyph) — handy without a Nerd Font.

```toml
[layout]
right = ["battery", "cwd", "loadavg", "datetime"]

[widgets.battery]
format = "{icon} {percent}%"   # {icon}, {percent}, {state}
down_format = ""               # shown when no battery (desktops); default: nothing
warn_percent = 20              # default; alert badge at/below this % while discharging
crit_percent = 10              # default; 0 disables a tier
# icon = "BAT"                 # optional; overrides the computed level/charging glyph
```

### CPU and memory widgets

`cpu` and `memory` are built-in and **in the default right layout** (unlike
the opt-in widgets above) — they show live CPU utilization and memory usage,
each with a Unicode gauge bar.

`cpu` takes a `format` (default `"{icon} {percent}%"`) with `{icon}`
(nf-md-chip), `{percent}`, `{bar}`, and `{spark}` placeholders. `memory` takes
a `format` (default `"{icon} {used}/{total}"`) with `{icon}` (nf-md-memory),
`{used}`/`{total}`/`{avail}` (human-readable binary sizes, e.g. `6.2G`),
`{percent}`, `{bar}`, and `{spark}` placeholders. `{bar}` is a `bar_width`-cell
(default 8) Unicode block-eighths gauge shared by both widgets; `{spark}` is a
Unicode sparkline of the last `spark_width` readings (default 8), persisted
across refreshes. Both also take a
`down_format` (default empty) shown on an unsupported platform or a failed
read — same collapse-to-nothing behavior as the `battery` widget's
`down_format` — and an `alt_format` for
[click-to-toggle](#click-to-toggle-widget-views). Both are also
[threshold-aware](#themes): `warn_percent`/`crit_percent` (cpu default 80/95,
memory default 80/92) alert at or above those levels. Each also takes its
own optional `icon` that overrides `{icon}` with a fixed glyph instead of
the built-in Nerd-Font one (default: `None`, keep the built-in glyph).

```toml
[widgets.cpu]
format = "{icon} {spark} {percent}%" # default "{icon} {percent}%"
bar_width = 8
spark_width = 8     # {spark} history length (last N readings)
warn_percent = 80   # default; 0 disables a tier
crit_percent = 95   # default
# icon = "CPU"       # optional; overrides the built-in Nerd-Font glyph

[widgets.memory]
format = "{icon} {used}/{total}"     # default; or "{icon} {spark} {percent}%"
bar_width = 8
spark_width = 8
warn_percent = 80   # default; 0 disables a tier
crit_percent = 92   # default
# icon = "MEM"       # optional; overrides the built-in Nerd-Font glyph
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

### Uptime and media widgets

Two more opt-in built-ins, neither in the default layout:

`uptime` shows system uptime, humanized to the coarsest unit pair (`3d 4h`,
`1h 15m`, `12m`, `<1m`). Its `format` (default `"{uptime}"`) substitutes
`{uptime}`; on a platform where uptime can't be read it renders nothing (or its
`down_format`).

`media` shows the current now-playing track via `playerctl` (Linux). Its
`format` (default `"{title} — {artist}"`) substitutes `{artist}`, `{title}`,
and `{status}`; when `playerctl` is missing or no player is running it renders
nothing (or its `down_format`) rather than a fake "not playing". Both also take
an `alt_format` for [click-to-toggle](#click-to-toggle-widget-views).

```toml
[layout]
right = ["media", "uptime", "cwd", "datetime"]

[widgets.uptime]
format = "up {uptime}"

[widgets.media]
format      = "♪ {title} — {artist}"
down_format = ""                      # nothing when nothing is playing
```

### Throughput widget

An opt-in `throughput` built-in shows network download/upload rates, read from
`/proc/net/dev` (Linux). Not in the default layout — add it to a `layout`
region to use it. A rate is a delta between two counter samples, so it renders
nothing on the **first** refresh (nothing yet to diff against), and nothing on
a non-Linux platform or a failed read — never a fake `0`.

Takes a `format` (default `" {down} {up}"`, no icon) with `{down}`/`{up}`
placeholders (human-readable binary sizes suffixed `/s`, e.g. `1.2M/s`), an
optional `interface` (pin to a single NIC; omit to aggregate every non-loopback
interface), a `down_format` (default empty), and an `alt_format` for
[click-to-toggle](#click-to-toggle-widget-views).

```toml
[layout]
right = ["throughput", "cwd", "datetime"]

[widgets.throughput]
format      = " {down} {up}"   # default
# interface = "eth0"           # optional; omit to aggregate all NICs
down_format = ""
```

### Per-widget colors

Any format-bearing widget accepts optional `fg`/`bg` keys to pin its colors,
using the same `Color` shapes as a theme (`{ Indexed = N }`,
`{ Named = "cyan" }`, or `{ Rgb = [r, g, b] }`). `fg` always applies; `bg` only
applies where the widget doesn't already have an explicit background (so it
won't fight a threshold alert badge). Leave them unset to use the theme's
cycling palette as before.

```toml
[widgets.cwd]
fg = { Indexed = 250 }
bg = { Rgb = [40, 44, 52] }
```

### Per-button click actions

Beyond the default left-click toggle, a widget can bind a specific mouse button
to open a URL or run a command, as `left_click` / `right_click` /
`middle_click` under its `[widgets.<name>]` table:

```toml
[widgets.cpu]
right_click  = { run = "tmux display-popup -E htop" }
middle_click = { open_url = "https://grafana.example/host" }
# left_click = { toggle = false }   # disable the default left-click toggle
```

The command in a `run` binding comes only from your own config (never from
anything tmux passes in), and runs detached via `sh -c`. **One catch:** a
widget only receives clicks once it emits a clickable range, which today
happens when it has a non-empty `alt_format`. So a `run`/`open_url` binding on
a widget with no `alt_format` won't fire — give the widget an `alt_format`
(even a throwaway one) to make it clickable. This needs the same
`MouseDown{1,2,3}Status` bindings the `rustline init` wizard writes and
`set -g mouse on`.

### Click-to-toggle widget views

`datetime`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`, `memory`, `loadavg`,
`git`, `disk`, `uptime`, `media`, and `throughput` each take an `alt_format` (default empty). Give a widget a non-empty
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
rustline theme list --json           # same list as a JSON array, for scripting
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
edit any of them, then `rustline theme use <name>`. Add `--edit` to open the
new file in `$EDITOR` right away (needs a TTY); either way it prints the
`theme use <name>` follow-up. A themes-dir file always
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
re-fetching every iteration. A separate build-timing pass measures each
plugin's cold `build_plugin` with the on-disk wasmtime compile cache off vs on
— on a warm cache a cold spawn deserializes the precompiled module instead of
re-running the compiler (measured ~13× faster per plugin).

`just bench --only daemon` (included in the default `all` run) A/B's a
plugin-heavy layout's `left`/`right` render in-process (cold — a fresh
registry, re-instantiating every WASM plugin, same as a daemon-less render)
against a real round-trip to a reachable [daemon](#daemon-optional). It never
starts a daemon itself: with none reachable, it prints a note telling you to
start one (`rustline daemon run`) instead of measuring nothing. The win is
biggest on a layout with plugins in it — a layout with none may show little
or even a slight loss (socket round-trip overhead) once you account for it.

## Daemon (optional)

By default every `rustline render` invocation is a fresh, self-contained
process — it parses your config, resolves your theme, builds the widget
registry, and (for `left`/`right`) discovers/instantiates any WASM plugins,
every single time tmux fires `status-interval`. That's normally fast enough
not to notice, but an optional persistent daemon can keep all of that warm
across refreshes instead, so a render becomes a quick socket round-trip
against already-built state:

```bash
rustline daemon run      # or just `rustline daemon`; runs in the foreground
rustline daemon status   # is it up and reachable?
rustline daemon stop     # ask it to shut down
```

`rustline daemon run` does **not** background itself — run it under a
supervisor. The easiest way is the built-in installer, which uses your
platform's native per-user service manager (**systemd** on Linux, **launchd**
on macOS) so the daemon starts at login:

```bash
rustline daemon install            # write the service file, load + start it now
rustline daemon install --write-only   # write the file only; you load it
rustline daemon uninstall          # unload/stop it and remove the file
```

On **Linux**, `rustline daemon install` generates a systemd **user** unit at
`$XDG_CONFIG_HOME/systemd/user/rustline-daemon.service` (falling back to
`~/.config/systemd/user/`) whose `ExecStart` calls the running binary's own
resolved absolute path (override with `--binary <path>`), then runs
`systemctl --user daemon-reload` and `enable --now` for you. If `systemctl`
isn't on `PATH` (or you passed `--write-only`), it prints the manual
`systemctl --user enable --now rustline-daemon.service` command instead.

On **macOS**, it writes a launchd **LaunchAgent** at
`~/Library/LaunchAgents/rustline-daemon.plist` (`RunAtLoad` starts it at every
login; `KeepAlive` restarts it only if it crashes — a clean `daemon stop`
stays stopped), then loads it with `launchctl bootstrap gui/$UID`. If
`launchctl` isn't on `PATH` (or you passed `--write-only`), it prints the
manual `launchctl bootstrap gui/$(id -u) …` command instead.

Either way the service file is written first and a `systemctl`/`launchctl`
failure is never fatal. `rustline daemon uninstall` best-effort unloads the
service and removes the file; running it again once already uninstalled is a
no-op.

For reference, on Linux the unit it writes looks like this:

```ini
# Managed by `rustline daemon install`.
[Unit]
Description=rustline render daemon

[Service]
ExecStart=/home/you/.local/bin/rustline daemon run
Restart=on-failure

[Install]
WantedBy=default.target
```

…and on macOS, the LaunchAgent plist:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Managed by `rustline daemon install`. -->
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>rustline-daemon</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Users/you/.local/bin/rustline</string>
        <string>daemon</string>
        <string>run</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
</dict>
</plist>
```

If you'd rather manage it by hand, write the appropriate file yourself (with
your own binary path) and run:

```bash
systemctl --user enable --now rustline-daemon.service            # Linux
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/rustline-daemon.plist  # macOS
```

Or start it from tmux itself with a `run-shell` line in `~/.tmux.conf`
(outside the managed `rustline init` block, so it isn't removed by a
future re-run):

```tmux
run-shell "rustline daemon status >/dev/null 2>&1 || rustline daemon run >/dev/null 2>&1 &"
```

**You never have to think about whether it's running.** Every render call
tries the daemon's Unix socket first and transparently falls back to the
normal in-process render on *any* failure — the socket doesn't exist, the
daemon isn't listening, a request times out, whatever. There's no wizard
question for this yet and no auto-spawn: it's entirely opt-in, and
`rustline doctor` reports whether a daemon is currently reachable
(advisory only — never a failing check). Want numbers instead of a vibe? Start
one and run `rustline bench --only daemon` (see [Benchmarking](#benchmarking))
to A/B a render against it. The daemon reloads its config
automatically when `config.toml`'s modification time changes, so editing
your config doesn't require a restart. One thing it does *not* pick up
live: a per-invocation `render --plugin-dir=X` is ignored while a daemon is
reachable, since the daemon always serves with whatever `--plugin-dir` it
was started with — restart it if you need to point it at a different
plugin directory.

## Plugins

Third-party widgets can be added as WASM plugins. A plugin is a small wasm
module (built for `wasm32-unknown-unknown` with the [Extism PDK][extism-pdk])
that exports a `name` function and a `render(context, config) -> Segment[]`
function — the same `Context` in, `Segment`s out contract as a built-in
widget, just crossing the wasm boundary as JSON.

Everything a plugin can touch is capability-gated by the host: network
requests, arbitrary file paths, and running local commands are checked
against per-plugin allowlists in your config (`allowed_urls` / `allowed_paths`
/ `allowed_commands`, each a glob or a `re:`-prefixed regex; `re:` patterns
are anchored to a full-string match, so include `.*` for a
prefix, e.g. `re:https://wttr\.in/.*`), and each plugin gets its own sandboxed
state directory with a size
quota (`max_state_bytes`, default 50 MB) for caching data between renders. The
host also exposes a TTL-cached HTTP GET (and a TTL-cached exec — see below),
so a plugin can fetch remote data or run a slow command at most once per
interval without managing its own cache — the bundled `weather`
example uses the former. A plugin has no ambient access to anything — a
disallowed request is simply refused, and any plugin error, timeout, or
crash renders as an empty segment rather than breaking the status line.

**Plugin security, plainly:** an approved `allowed_commands` entry lets a
plugin run a real program with **your own environment variables and user
permissions** — the host does not sandbox the command itself, only *whether*
it can run and *which* command lines. A pattern matches the **whole command
line** (program plus every argument, not just the program name), so a grant
for `git status*` does not cover `git push --force`. And there is **no shell**
involved unless you explicitly grant one (`allowed_commands = ["sh -c *"]`)
— the host always spawns the program directly, so nothing re-parses or
word-splits what a plugin sends it. Only approve `allowed_commands` patterns
you actually understand.

Five worked examples ship under `plugins/`, each demonstrating a different
host capability:

| Plugin       | Capability                          | Renders                                  |
|--------------|--------------------------------------|-------------------------------------------|
| `weather`    | `rl_http_get_cached` (TTL-cached GET) | a condition icon + °F for a zip code      |
| `counter`    | `rl_state_read`/`rl_state_write`      | a count that persists across renders      |
| `filewatch`  | `rl_file_read`                        | a configured file's first line + line count |
| `httpget`    | `rl_http_get` (plain, uncached)       | a snippet of a fetched URL's body         |
| `cmdrun`     | `rl_exec`/`rl_exec_cached`            | a snippet of a command's stdout           |

Build and install any of them with the generic recipe:

```bash
just build-plugin weather    # or counter / filewatch / httpget / cmdrun
```

(`just build-weather` still works too — it's now just an alias for
`just build-plugin weather`.)

Then add one to your layout and grant it whatever it needs. For `weather`,
that's a URL allowlist:

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

`counter` needs no allowlist at all (it only touches its own sandboxed state
dir):

```toml
[layout]
right = ["counter", "cwd", "datetime"]

[plugins.counter.options]
format = "seen {count}x"
```

`filewatch` needs a path allowlist (`allowed_paths`), and `httpget` a URL
allowlist, same shape as `weather`'s:

```toml
[plugins.filewatch]
allowed_paths = ["/home/steve/.cache/build-status"]

[plugins.filewatch.options]
path = "/home/steve/.cache/build-status"
format = "{first_line} ({lines}L)"

[plugins.httpget]
allowed_urls = ["https://example.com/status"]

[plugins.httpget.options]
url = "https://example.com/status"
format = "status: {body}"
max_chars = 40
```

`cmdrun` needs a command allowlist (`allowed_commands`), matched against the
whole command line:

```toml
[layout]
right = ["cmdrun", "cwd", "datetime"]

[plugins.cmdrun]
allowed_commands = ["uname", "uname *"]  # program "uname", any args (globs match "*" literally, not a prefix)

[plugins.cmdrun.options]
program = "uname"
args = ["-sr"]
format = "{out}"
```

All four of the newer examples also demonstrate `rl_log`: each logs a
`warn`-level message through the host's `tracing` subscriber on its one
failure path (a denied/failed state write, an unreadable file, a non-2xx HTTP
response, or a denied/failed/non-zero-exit command) rather than staying
silent.

Manage a plugin's allowlists from the command line without hand-editing TOML:

```bash
rustline plugin list
rustline plugin list --json                        # same info as a JSON array, for scripting
rustline plugin url add weather "https://wttr.in/*"
rustline plugin url list weather --json             # its allowed_urls as a JSON array of strings
rustline plugin cmd add cmdrun "uname *"             # allowed_commands works the same way
```

A plugin can also declare a capability *manifest* — a sidecar
`<plugin_dir>/<name>.toml` (or an embedded `rustline-manifest` wasm custom
section) listing the `allowed_urls`/`allowed_paths`/`allowed_commands` it
wants — so you don't have to hand-write them yourself:

```bash
rustline plugin approve weather        # shows what it requests, asks to confirm
rustline plugin approve weather --yes  # non-interactive; for scripts
rustline plugin approve cmdrun         # commands get an extra, explicit warning
```

`approve` writes exactly the requested patterns into
`[plugins.<name>]`'s allowlists (never more than what's declared), and does
nothing if the plugin has no manifest.

Install a published plugin straight from a GitHub release instead of building
it yourself:

```bash
rustline plugin install owner/repo          # downloads the .wasm into your plugin dir
rustline plugin update my-widget            # re-fetch the latest release
rustline plugin remove my-widget --yes      # delete the .wasm and its config entry
```

`install` downloads the plugin's `.wasm`, records its `source`/`tag`/`checksum`
in `[plugins.<name>]`, and grants **no** capabilities — you still run
`rustline plugin approve` (or `plugin url|path add`) to allow anything. If a
plugin is ever denied something it asked for, `rustline plugin denials
<name>` lists every capability it was refused, so you can decide whether to
grant it — add `--json` for a machine-readable `{kind, target}` array.

Scaffold a new plugin crate instead of copying `weather` by hand:

```bash
rustline plugin new my-widget --path plugins
```

This writes `plugins/my-widget/Cargo.toml` (edition 2024, `crate-type =
["cdylib"]`, an empty `[workspace]` table so it builds standalone rather than
joining rustline's own workspace) and `plugins/my-widget/src/lib.rs` (a
`name`/`render` skeleton built on the `rustline-plugin-sdk` crate, which bundles
the typed host-capability wrappers, the `GuestRender`/`WireContext` wire types,
and an `export_plugin!` macro so you write one crate instead of hand-rolling
the Extism glue), then prints the `cargo build --target wasm32-unknown-unknown`
+ install command and a starter `[plugins.my-widget]` config snippet. The
plugin name must be `[A-Za-z0-9_-]`, at most 15 bytes, and not `window` — the
same rule as a widget's [click-to-toggle](#click-to-toggle-widget-views) range
name, since it becomes a tmux `range=user|<name>` argument verbatim. Pass
`--force` to overwrite an existing scaffold.

Build any plugin crate and install its `.wasm` into your plugin dir in one
step with `rustline plugin build <dir>`, or dry-run one against a sample
`Context` (printing its segments and any capability denials, without touching
your config) with `rustline plugin run <name>`. Plugins negotiate a small ABI
version with the host, so a plugin built against an incompatible future ABI is
skipped rather than loaded — existing plugins keep working.

See the [design spec](docs/superpowers/specs/2026-07-20-rustline-wasm-plugins-design.md)
for the full capability model, config schema, and plugin ABI, and the
[widget manager + exec capability design
spec](docs/superpowers/specs/2026-07-24-rustline-widget-manager-and-exec-capability-design.md)
for the exec capability's gating/bounds detail and the layout-editing CLI/TUI.

[extism-pdk]: https://github.com/extism/rust-pdk

## Design

See the full design specs:
[core statusline](docs/superpowers/specs/2026-07-20-rustline-tmux-statusline-design.md),
[WASM plugins](docs/superpowers/specs/2026-07-20-rustline-wasm-plugins-design.md),
[IP widgets](docs/superpowers/specs/2026-07-20-rustline-ip-widgets-design.md),
[CPU/memory widgets](docs/superpowers/specs/2026-07-21-rustline-cpu-memory-widgets-design.md),
[click-to-toggle widgets](docs/superpowers/specs/2026-07-21-rustline-click-toggle-widgets-design.md),
[themes/theme picker](docs/superpowers/specs/2026-07-21-rustline-themes-theme-picker-design.md),
[whats-next bundle](docs/superpowers/specs/2026-07-22-rustline-whatsnext-bundle-design.md),
[widget manager + exec capability](docs/superpowers/specs/2026-07-24-rustline-widget-manager-and-exec-capability-design.md).

## License

MIT — see [`LICENSE`](LICENSE).
