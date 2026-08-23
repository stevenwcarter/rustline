# Config reference

Optional TOML at `~/.config/rustline/config.toml` (or
`$XDG_CONFIG_HOME/rustline/config.toml`). **Zero-config works** — and
`Config::load` is total, so a bad config must never break the bar
(invariant #3). Every option defaults to reproducing the pre-feature output
byte-for-byte.

Default layout: left = `[pane_id, hostname]`, center = `[windows]`,
right = `[cwd, cpu, memory, loadavg, datetime]`.

| File | Read when |
|---|---|
| [`layout-and-instances.md`](layout-and-instances.md) | Changing the default layout, `[layout]` semantics (incl. the inert `center` region), or `[instances.<name>]` multi-instance widgets. |
| [`widgets-default.md`](widgets-default.md) | Options on `hostname`, `pane_id`, `cwd`, `cpu`, `memory` (plus `lan_ip`/`tailscale_ip`/`battery`). |
| [`widgets-optional.md`](widgets-optional.md) | Options on `loadavg`, `git`, `disk`, `uptime`, `media`, `throughput`. |
| [`click-and-color.md`](click-and-color.md) | Per-widget `fg`/`bg`, `alt_format` toggling, per-button click bindings, the global toggle state file. |
| [`themes.md`](themes.md) | Theme layering/resolution, adding a built-in theme, the six `win_*` pill fields, the four semantic colors. |
| [`plugins.md`](plugins.md) | The `[plugins.<name>]` table, `plugin_dir`/`plugin_index_url`, allow-pattern glob/`re:` syntax. |
| [`plugin-capabilities.md`](plugin-capabilities.md) | Read/write path allowlists, the symlink gate, `allowed_commands` and its blast radius. |
| [`plugin-integrity.md`](plugin-integrity.md) | The load-time checksum gate, checksum recording, and what it does/doesn't defend against. |
| [`logging.md`](logging.md) | The `[log]` table, file rotation/naming, `warn_once` dedup. |
