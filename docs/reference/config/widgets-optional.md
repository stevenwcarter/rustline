# Config — opt-in widgets

> **Read when:** changing options on an opt-in widget: `loadavg`, `git`, `disk`, `uptime`, `media`, `throughput`.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

**Load average widget:** `loadavg` is in the **default** right layout. It takes
a `format` (default `"{load1} {load5} {load15}"`) with `{load1}`/`{load5}`/
`{load15}` placeholders (1/5/15-minute averages), each accepting an inline
precision spec `:.N` (e.g. `{load1:.1}`; bare `{loadN}` is 2 decimals, `N`
clamped to 0–10). Also takes an `alt_format` (click-toggle) and a `down_format`
(shown when `getloadavg` fails; default empty → renders nothing). It's also
**threshold-aware** on `load1` (see Themes below) via `warn_load`/`crit_load` —
unlike the other three widgets, both default to `0.0` (off), since an absolute
load threshold depends on core count.

    [widgets.loadavg]
    format      = "{load1} {load5} {load15}"   # default
    alt_format  = "{load1:.1} {load5:.1} {load15:.1}"   # left-click toggles to this
    down_format = ""
    warn_load   = 0.0   # default (off); e.g. 4.0 on a 4-core box
    crit_load   = 0.0   # default (off)

**Git widget:** `git` is opt-in — not in the default layout. It reads the
pane's git branch/status by shelling out to `git status --porcelain=v2
--branch` (`crates/rustline/src/git.rs`); `Context.git` is `None` (and the
widget renders nothing/`down_format`) when `git` is missing, the pane isn't
inside a repository, or the read fails — never a fabricated "clean" reading
(invariant #6). Takes a `format` (default `" {branch}{dirty}"` — a
Nerd-Font branch glyph, U+E0A0) with `{branch}` (current branch, or the
7-char short SHA when `HEAD` is detached), `{ahead}`/`{behind}`/`{staged}`/
`{unstaged}` (counts), and `{dirty}` placeholders, a `dirty_glyph` (default
`"*"`, substituted for `{dirty}` iff `staged > 0 || unstaged > 0`, else
empty), and a `down_format` (default `""`, i.e. render nothing) — same
collapse-placeholders-to-empty behavior as the other widgets' `down_format`.
NOT threshold-aware (no semantic-color alert badge).

```toml
[widgets.git]
format = " {branch}{dirty}"   # U+E0A0 branch glyph
dirty_glyph = "*"
down_format = ""
```

**Disk widget:** `disk` is opt-in — not in the default layout. It reads
filesystem usage for a configured `mount` (default `"/"`) via `statvfs(2)`
(`crates/rustline/src/disk.rs`); `Context.disk` is `None` (and the widget
renders nothing/`down_format`) when the mount can't be `statvfs`'d — never a
fabricated `0` reading (invariant #6). Takes a `format` (default
`" {used}/{total}"`, no icon placeholder — out of scope for this widget) with
`{used}`/`{total}`/`{avail}` (human-readable binary sizes, reusing `memory`'s
`format_bytes`), `{percent}`, `{bar}` (a `bar_width`-cell gauge, default 8,
reusing the same shared bar as `cpu`/`memory`), and a static `{mount}` (the
configured mount string itself, not a live reading) placeholders, and a
`down_format` (default `""`, i.e. render nothing) — same
collapse-placeholders-to-empty behavior as the other widgets' `down_format`.
It's also threshold-aware (see Themes below): `warn_percent`/`crit_percent`
(default 85/95) alert at or above those levels.

```toml
[widgets.disk]
mount = "/"                 # default; any statvfs-able path
format = " {used}/{total}"  # default
bar_width = 8
down_format = ""
warn_percent = 85   # default; 0 disables a tier
crit_percent = 95   # default
```

**Uptime widget:** `uptime` is opt-in — not in the default layout. It reads
system uptime once at `Context`-build time (`/proc/uptime` on Linux,
`kern.boottime` on macOS; `Context.uptime` is `None`/renders `down_format` on
any failure). Takes a `format` (default `"{uptime}"`) whose `{uptime}`
placeholder is the humanized coarsest unit pair (`3d 4h`, `1h 15m`, `12m`,
`<1m`), plus `alt_format`/`down_format`. NOT threshold-aware.

```toml
[widgets.uptime]
format      = "up {uptime}"   # default "{uptime}"
down_format = ""
```

**Media widget:** `media` is opt-in — not in the default layout. It reads the
current now-playing track by shelling out to `playerctl metadata` (Linux only;
`Context.media` is `None`/renders `down_format` when `playerctl` is missing, no
player is running, or the read fails — never a faked "not playing" reading).
Takes a `format` (default `"{title} — {artist}"`) with `{artist}`/`{title}`/
`{status}` placeholders, plus `alt_format`/`down_format`. NOT threshold-aware.

```toml
[widgets.media]
format      = "{title} — {artist}"   # default
alt_format  = "{status}: {title}"    # left-click toggles to this
down_format = ""
```

**Throughput widget:** `throughput` is opt-in — not in the default layout. It
reads per-interface `/proc/net/dev` byte counters once at `Context`-build time
(Linux only; `crates/rustline/src/throughput.rs`) and diffs them against a
prior sample persisted under the state dir to compute down/up rates.
`Context.throughput` is `None` (and the widget renders nothing/`down_format`)
on the **first invocation** (a rate is a delta — nothing yet to diff against),
on a non-Linux platform, or when the read fails — never a fabricated `0` rate
(invariant #6). Takes a `format` (default `" {down} {up}"`, no icon
placeholder) with `{down}`/`{up}` (human-readable binary sizes suffixed `/s`,
reusing `memory`'s `format_bytes`, e.g. `1.2M/s`) placeholders, an optional
`interface` (pin to one named interface; `None`, the default, aggregates every
non-loopback interface), and a `down_format` (default `""`) — same
collapse-placeholders-to-empty behavior as the other widgets' `down_format`.
NOT threshold-aware (a rate has no universal ceiling).

```toml
[widgets.throughput]
format = " {down} {up}"     # default
# interface = "eth0"        # optional; omit to aggregate all non-loopback NICs
down_format = ""
```

