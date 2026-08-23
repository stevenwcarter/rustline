# Config — default-layout widgets

> **Read when:** changing options on a widget that ships in the default layout: `hostname`, `pane_id`, `cwd`, `cpu`, `memory` (plus the opt-in `lan_ip`/`tailscale_ip`/`battery` that sit alongside them).
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

**Hostname and pane_id widgets:** both are in the default layout (`left =
[pane_id, hostname]`) and each now take a `format` option (previously a fixed
string). `hostname`'s `format` (default `"{host}"`) substitutes `{host}` (the
hostname truncated at the first `.`); `pane_id`'s `format` (default
`"{session}:{window}.{pane}"`) substitutes `{session}`/`{window}`/`{pane}`.
Any other text (e.g. a Nerd-Font icon or label) is emitted verbatim; unknown
placeholders pass through untouched. Every default reproduces the pre-config
output byte-for-byte.

```toml
[widgets.hostname]
format = "{host}"   # default

[widgets.pane_id]
format = "{session}:{window}.{pane}"   # default
```

**Cwd widget:** `cwd` is in the default right layout. It takes a `format`
(default `"{path}"`) whose `{path}` placeholder is replaced by the (possibly
shortened) working directory, plus the existing `abbreviate_home` (default
`true`, a leading `$HOME` component becomes `~`) and three new shortening
options applied in a fixed order — home-abbreviation, then `abbreviate`, then
`max_depth`, then `max_len`, then the `format` substitution: `abbreviate`
(default `false`, fish-shell style — every path component but the last
shrinks to its first `char`), `max_depth` (default `0` = unlimited; keeps
only the last N `/`-separated components, prefixing a leading `…/` when
components are dropped), and `max_len` (default `0` = unlimited; left-
truncates the result to at most N characters, prefixing a leading `…`).
Every option at its default reproduces the pre-feature output byte-for-byte.

```toml
[widgets.cwd]
format          = "{path}"   # default
abbreviate_home = true       # default; leading $HOME -> ~
abbreviate      = false      # default; fish-style: ~/src/rustline -> ~/s/rustline
max_depth       = 0          # default (0 = unlimited); keep last N components, prefix "…/"
max_len         = 0          # default (0 = unlimited); left-truncate to N chars, prefix "…"
```

**IP widgets:** `lan_ip` and `tailscale_ip` are opt-in — neither is in the
default layout. Both take a `format` (default `"{ip}"`) whose `{ip}`
placeholder is replaced by the selected address (any surrounding label/glyph
text is kept verbatim), and a `down_format` (default `""`, i.e. render
nothing) shown when no matching address is found — a `{ip}` inside
`down_format` collapses to empty rather than showing a stale/fake address.
`lan_ip` additionally takes an `interface` override: an exact interface name
that wins unconditionally over auto-selection, even a virtual/public NIC.

```toml
[widgets.lan_ip]
format = "LAN {ip}"
down_format = ""
interface = "wlp3s0"          # optional; overrides auto-selection

[widgets.tailscale_ip]
format = "TS {ip}"
down_format = "TS off"
```

**Battery widget:** `battery` is opt-in — not in the default layout. It takes
a `format` (default `"{icon} {percent}%"`) with `{icon}` (level-bucketed,
charging-aware Nerd-Font glyph, or the `icon` override below), `{percent}`,
and `{state}` placeholders, and a `down_format` (default `""`, i.e. render
nothing) shown when `Context.battery` is `None` (no battery, unsupported
platform, or a failed read) — same collapse-placeholders-to-empty behavior as
the IP widgets' `down_format`. It's also **threshold-aware** (see Themes
below): `warn_percent`/`crit_percent` (default 20/10) alert while discharging
at or below those levels. An optional `icon` overrides `{icon}` with a fixed
glyph, replacing the computed one entirely (`None`, the default, keeps the
computed glyph) — handy for non-Nerd-Font terminals.

```toml
[widgets.battery]
format = "{icon} {percent}%"
down_format = ""
warn_percent = 20   # default; badge at/below this % while discharging
crit_percent = 10   # default; 0 disables a tier
# icon = "BAT"      # optional; overrides the computed level/charging glyph
```

**CPU and memory widgets:** `cpu` and `memory` are in the **default** right
layout (unlike the opt-in IP/battery widgets above). `cpu` takes a `format`
(default `"{icon} {percent}%"`) with `{icon}` (nf-md-chip, or the `icon`
override below), `{percent}`, `{bar}` (a `bar_width`-cell Unicode
block-eighths gauge, default 8), and `{spark}` (a Unicode block-eighths
sparkline of the last `spark_width` readings, default 8 — see below)
placeholders. `memory` takes a `format`
(default `"{icon} {used}/{total}"`) with `{icon}` (nf-md-memory, or its own
`icon` override), `{used}`/`{total}`/`{avail}` (human-readable binary sizes,
e.g. `6.2G`), `{percent}`, `{bar}` (`bar_width`, default 8), and `{spark}`
(`spark_width`, default 8) placeholders.
Both take a `down_format` (default `""`, i.e. render nothing) shown when the
platform read failed or is unsupported — same collapse-placeholders-to-empty
behavior as `battery`'s `down_format`. Both are also **threshold-aware** (see
Themes below): `warn_percent`/`crit_percent` (cpu default 80/95, memory
default 80/92) alert at or above those levels. Each also takes an optional
`icon` that overrides `{icon}` with a fixed glyph instead of the built-in
Nerd-Font one (`None`, the default, keeps the built-in glyph).

```toml
[widgets.cpu]
format = "{icon} {spark} {percent}%" # default "{icon} {percent}%"
bar_width = 8
spark_width = 8     # {spark} history-ring length (last N readings)
down_format = ""
warn_percent = 80   # default; 0 disables a tier
crit_percent = 95   # default
# icon = "CPU"      # optional; overrides the built-in Nerd-Font glyph

[widgets.memory]
format = "{icon} {used}/{total}"     # default; or "{icon} {spark} {percent}%"
bar_width = 8
spark_width = 8
down_format = ""
warn_percent = 80   # default; 0 disables a tier
crit_percent = 92   # default
# icon = "MEM"      # optional; overrides the built-in Nerd-Font glyph
```

