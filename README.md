# rustline

A fast, native tmux statusline written in Rust — powerline-style segments for
pane, window, host, and system info, with zero required configuration.

## Features

- Powerline-style segments (pane id, hostname, window list, cwd, load average,
  date/time) rendered as a single static binary — no shell scripts, no
  external statusline framework.
- Zero-config: sensible defaults out of the box.
- Optional TOML config to reorder widgets or tweak per-widget options.
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
`windows`, `cwd`, `loadavg`, `datetime`.

Example — reorder the right region and change the clock format:

```toml
[layout]
right = ["datetime", "cwd"]

[widgets.datetime]
format = "%H:%M"
```

Run `rustline print-config` to print the fully-resolved effective
configuration (your overrides layered onto the defaults) as TOML.

## Roadmap: WASM plugins

Third-party widgets via WASM plugins are **planned but not yet implemented**.
The config format already reserves a table for them, keyed by the plugin's
GitHub repo:

```toml
# reserved for future WASM plugins; parsed and retained, unused today
[plugins."owner/repo"]
```

The core `Context`/`Segment`/`Style` types are already serde-serializable so
they can serve as the future plugin ABI, but there is no WASM runtime yet —
this is a roadmap item, not a current feature.

## Design

See the full design spec:
[`docs/superpowers/specs/2026-07-20-rustline-tmux-statusline-design.md`](docs/superpowers/specs/2026-07-20-rustline-tmux-statusline-design.md).

## License

MIT — see [`LICENSE`](LICENSE).
