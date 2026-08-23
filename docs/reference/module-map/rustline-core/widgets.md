# `rustline-core` — `widget.rs` and the sixteen built-ins

> **Read when:** adding a widget, changing an existing one, touching the `Widget` trait/`Registry`, widget instances, or the shared bar/spark/alert helpers.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- `widget.rs` — `Widget` trait and `Registry` (name → factory; `resolve` skips
  unknown widget names with a `warn!`, never errors). `resolve` now returns
  `Vec<(WidgetName, Box<dyn Widget>)>` (W53) — each built widget paired with the
  layout name it came from, so a caller (e.g. `render_named_region`) never
  re-filters `names` to recover which widget is which.
  `Widget::range_name(&self)
  -> Option<RangeName>` defaults to `None`; a clickable widget returns
  `RangeName::of_widget`'s result via `clickable_range` (see `toggle.rs`
  below).
  `WidgetDescriptor { name, summary, configurable, source: WidgetSource }`
  (`WidgetSource::{Builtin, Plugin, Instance { kind: WidgetKind }}` — the third
  variant, added for the widget manager, labels a `[instances.<name>]`
  descriptor with its declared kind so `widget list`/`widget edit` can show
  "instance of cpu" rather than lumping it in with `Builtin`) describes a
  registered widget independent of building an instance;
  `Registry::register_described` registers a factory alongside its
  descriptor (`register` still works, recording a minimal
  built-in/non-configurable one), and `descriptors()`/`available_names()`
  enumerate them in registration order (W22) — the enabling abstraction the
  widget manager's `widget_placements` (see `config.rs` below) builds on.
  The twelve clickable/format-bearing
  widget structs (see `toggle.rs` below) each now carry a `name: WidgetName`
  field (W46) — their range/toggle identity (invariant #7) — instead of
  hardcoding their kind name in `range_name()`/`active_format`; a base
  built-in sets `name` to its kind (byte-identical output), an
  `[instances.<name>]` entry sets it to the instance name. `widgets/mod.rs`
  gains one `pub(crate) fn build_<kind>(name: &str, o: &<Kind>Opts) -> Box<dyn
  Widget>` helper per clickable kind (`build_datetime`, `build_lan_ip`,
  `build_tailscale_ip`, `build_battery`, `build_cpu`, `build_memory`,
  `build_loadavg`, `build_git`, `build_disk`, `build_uptime`, `build_media`,
  `build_throughput`), reused by both base registration (passes the kind
  name) and instance registration (passes the instance name) — one factory
  body instead of two. `Registry::with_builtins(&Config)` gains a second pass
  over `cfg.resolved_instances()` (W46, parse-once via T5 — see `config.rs`
  below): a `NoKind` entry, an `UnknownKind` (unrecognized, or a
  valid-but-non-clickable kind — `cwd`/`hostname`/`pane_id`/`windows` —
  instanceable but never clickable, so out of scope for this pass), or a name
  already registered (a built-in or an earlier instance) is `warn!`+skipped
  (invariant #3, built-in always wins); otherwise the already-typed
  `InstanceSpec` (parsed once via `instance_opts`, which itself warns once via
  `warn_once` and falls back to `<Kind>Opts::default()` on a type error) is
  registered under the instance name via `build_instance`/its `build_<kind>`
  helper. An instance name that fails `RangeName::parse` (too long, a
  `[A-Za-z0-9_-]` violation, or the reserved `window`) still registers but
  logs a one-time "not click-toggleable" warn (mirrors a too-long plugin
  stem; registration stays permissive — invalid names register but are
  unclickable).
- `widgets/` — the sixteen built-ins: `pane_id`, `hostname`, `windows`, `cwd`,
  `loadavg`, `datetime`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`, `memory`,
  `git`, `disk`, `uptime`, `media`, `throughput`, plus `Registry::with_builtins(&Config)` in `mod.rs`. `net.rs` is the pure
  LAN/Tailscale interface-selection and `{ip}` formatting logic shared by
  `lan_ip`/`tailscale_ip` (no I/O — operates on `Context.interfaces`).
  `alert.rs` is the shared threshold-alert helper used by `cpu`/`memory`/
  `battery`/`loadavg`: `AlertKind { None, Warn, Crit }`, `alert_over`/
  `alert_under` (pure threshold comparisons, a `<= 0` tier is disabled, crit
  beats warn), and `alert_style(kind, &ThemeColors) -> Option<Style>` (the
  inverse alert badge: `bg`=semantic color, `fg`=`bar_bg`, `bold`). `battery.rs`
  is the `battery` widget: pure over `Context.battery`, with a
  level-bucketed, charging-aware Nerd-Font `{icon}` plus `{percent}`/`{state}`
  placeholders, plus `warn_percent`(20)/`crit_percent`(10) threshold config
  (via `alert_under`, discharging only — charging/full/unknown never alerts).
  `bar.rs` is `pub(crate) fn gauge_bar(fraction: f64, width:
  usize) -> String`, a shared pure Unicode block-eighths gauge (full `█`
  cells, one sub-cell partial, `░` track) used by the `{bar}` placeholder in
  both `cpu.rs` and `memory.rs`. It clamps `width` to `MAX_BAR_WIDTH` (256 — a
  status line is never wider), and `history.rs`'s `push_truncate` clamps
  `spark_width` to `MAX_HISTORY_WIDTH` the same way. Both are load-bearing for
  invariant #3, not tidiness: `bar_width`/`spark_width` are unbounded `usize`
  config, so a fat-fingered `bar_width = 1000000000` made every render allocate
  gigabytes, and at `usize::MAX` the internal `width * 8` wrapped and the loop
  effectively never terminated. `render_named_region`'s `catch_unwind` cannot
  save either case — it catches a panic, not an allocation abort or a hang — so
  the clamp has to live in the renderer, where it holds for every caller
  regardless of how the value arrived. The `spark_width` clamp additionally
  bounds a *file*, since the ring is persisted on every render. `spark.rs` is `pub(crate) fn sparkline(samples:
  &[f32], max: f32, width: usize) -> String` (W45), a shared pure Unicode
  block-eighths sparkline (one glyph per historical reading, clamped `0..=1` of
  `max`) backing the `{spark}` placeholder in both `cpu.rs` and `memory.rs`. It
  emits exactly `width` cells — a partly-filled ring is left-padded with
  spaces, newest reading at the right edge, so the widget doesn't grow a cell
  per refresh while the ring fills; the padding is blank rather than a `▁`
  glyph because a zero block would claim a reading that was never taken
  (invariant #6). `width` clamps to `MAX_SPARK_WIDTH` (256), mirroring
  `bar.rs`'s `MAX_BAR_WIDTH`, since `spark_width` is unbounded user config that
  now sizes a `String` allocation as well as the ring; the history itself
  rides `Context.cpu_history`/`mem_history`, populated at the bin's build edge
  only when a widget's `format` **or** `alt_format` references `{spark}` (W56;
  see `history.rs`/`cpu.rs`/
  `memory.rs` in the bin). `width.rs` (**Read when:** changing how a
  byte-rate or a percentage renders, or adding a numeric placeholder that
  must not jitter) is the shared constant-width numeric formatter:
  `format_rate(bytes_per_sec: u64) -> String` (a 4-character right-aligned
  numeral plus a one-character unit, always 5 characters —
  `throughput.rs` appends `/s` for 7) and `pad_percent(percent: u64) ->
  String` (right-aligned in 3 characters), used by `cpu`/`memory`/`battery`/
  `disk`'s `{percent}`. Both functions are pure and take no `Context`;
  deliberately separate from `memory.rs`'s `format_bytes`, which `memory`/
  `disk` still use for `{used}`/`{total}`/`{avail}` and which keeps its
  variable-width output. `cpu.rs` is the `cpu` widget: pure over
  `Context.cpu`, with an nf-md-chip `{icon}`, `{percent}`, `{bar}`, and
  `{spark}` (over `Context.cpu_history`, `spark_width` ring length)
  placeholders, plus `warn_percent`(80)/`crit_percent`(95) threshold config
  (via `alert_over`). `memory.rs` is the `memory` widget: pure over
  `Context.memory`, with an nf-md-memory `{icon}`, `{used}`/`{total}`/
  `{avail}` (human-readable binary sizes via `format_bytes`, e.g. `6.2G`),
  `{percent}`, `{bar}`, and `{spark}` (over `Context.mem_history`) placeholders,
  plus `warn_percent`(80)/
  `crit_percent`(92) threshold config (via `alert_over`). `windows.rs` is the
  `windows` widget: it emits only the window **text** (`{index}{flags}
  {name}`); the pill styling and active/inactive colors are applied
  downstream by the theme-aware renderer (widgets can't see the `Theme`).
  `loadavg.rs` is the `loadavg` widget: pure over `Context.loadavg`, with
  `{load1}`/`{load5}`/`{load15}` placeholders that each accept an inline
  Rust-style precision spec (`{load1:.1}`; default 2 decimals, so the default
  format is byte-identical to the pre-config output), plus
  `alt_format`/`down_format` like the rest of the family, and
  `warn_load`/`crit_load` threshold config on `load1` (both default `0.0` =
  off, since an absolute load threshold needs the core count); a private
  `substitute` scanner does the replacement. All five threshold-aware widgets
  render byte-identically to before this feature whenever no tier is
  crossed (a reading below every threshold, or a tier disabled via `0`).
  `git.rs` is the `git` widget: pure over `Context.git`, with `{branch}`
  (current branch, or the 7-char short SHA when `HEAD` is detached),
  `{ahead}`/`{behind}`/`{staged}`/`{unstaged}` (counts), and `{dirty}`
  (substitutes a configurable `dirty_glyph`, default `*`, iff `staged > 0 ||
  unstaged > 0`, else empty) placeholders; NOT threshold-aware (no
  `alert.rs` use) and NOT in the default layout.
  `disk.rs` is the `disk` widget: pure over `ctx.disks.get(&self.mount)` (W46
  — a `disk` instance's own configured `mount` selects its reading out of the
  per-mount map; the base widget's `mount` is `[widgets.disk].mount`, default
  `/`), with
  `{used}`/`{total}`/`{avail}` (human-readable binary sizes via `memory.rs`'s
  `format_bytes`, now `pub(crate)` and reused rather than duplicated),
  `{percent}`, `{bar}` (via the same shared `bar::gauge_bar` as `cpu`/
  `memory`), and a static `{mount}` (the configured mount string, substituted
  directly from widget config — not read from `Context`) placeholders, plus
  `warn_percent`(85)/`crit_percent`(95) threshold config (via `alert_over`);
  NOT in the default layout.
  `uptime.rs` is the `uptime` widget (W37): pure over `Context.uptime`, a
  single `{uptime}` placeholder rendered via `humanize_uptime` (coarsest
  non-zero unit pair — `3d 4h`, `1h 15m`, `12m`, `<1m`), plus
  `alt_format`/`down_format`; NOT threshold-aware and NOT in the default layout.
  `media.rs` is the `media` widget (W41): pure over `Context.media`, with
  `{artist}`/`{title}`/`{status}` placeholders (default `"{title} — {artist}"`),
  plus `alt_format`/`down_format`; NOT threshold-aware and NOT in the default
  layout.
  `throughput.rs` is the `ThroughputWidget` (W47, named with the `Widget`
  suffix to avoid colliding with the `rustline_abi::Throughput` data type):
  pure over `ctx.throughputs.get(&self.iface_key)` (W46 — `iface_key` is the
  widget's/instance's own configured `interface`, or `""` to aggregate; the
  base widget reads `[widgets.throughput].interface` the same way), with
  `{down}`/`{up}` placeholders (down/up
  bytes-per-sec via `width.rs`'s `format_rate`, a constant-width numeral+unit
  suffixed `/s`, always exactly 7 characters, e.g. ` 1.2K/s`), plus
  `alt_format`/`down_format`; NOT threshold-aware (a rate has
  no universal ceiling) and NOT in the default layout.
  `toggle.rs` holds the shared click-toggle helpers
  `active_format(ctx, name: &WidgetName, format, alt) -> &str`
  (picks `alt` iff it's non-empty AND `name` is in `ctx.toggled`, else
  `format`) and `clickable_range(name: &WidgetName, alt) -> Option<RangeName>`
  (`Some` iff `alt` is non-empty AND `name` parses as a `RangeName` —
  invariant #7's full rule, not just the byte-length check it used to be;
  see `range_name.rs` above);
  the twelve format-bearing widgets (`datetime`, `lan_ip`, `tailscale_ip`,
  `battery`, `cpu`, `memory`, `loadavg`, `git`, `disk`, `uptime`, `media`,
  `throughput`) each
  carry an `alt_format` field and call both helpers from their
  `render`/`range_name`.
