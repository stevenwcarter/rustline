# rustline

A tmux statusline system written in Rust. Most widgets are baked in, plus a
real WASM plugin host that runs third-party widgets under a capability-gated
sandbox. Primary target is Linux; the author's own use is the main driver, but
it's meant to be shareable.

## Architecture

Cargo **workspace**, edition 2024, `resolver = "2"`:

- `crates/rustline-core` — the **pure, front-end-agnostic core**. Given a
  `Context` snapshot it produces a tmux-format `String`. No I/O, no environment
  reads at render time. This is deliberately the reuse seam: today's only
  front-end is the CLI, but a future daemon would build `Context`s from a
  different source and call the same core unchanged — the optional W48
  render daemon (`crates/rustline/src/daemon.rs`) is a first cut at this: it
  still lives inside the `rustline` bin and still builds `Context`s the same
  way the CLI does per request, just keeping the config/theme/registry warm
  between requests rather than rebuilding them cold each time; a more
  ambitious future daemon (push-driven Context construction, sub-second
  widgets) remains open. Re-exports the
  `rustline-abi` types (`Segment`, `Style`, `Color`) so `rustline_core::Segment`
  etc. keep working unchanged.
- `crates/rustline-abi` — a small, **serde-only** crate holding the
  WASM-boundary output types (`Segment`, `Style`, `Color`). No I/O, no chrono.
  Split out of `rustline-core` so a wasm guest plugin can depend on just the
  wire types without pulling in core's heavier deps (config parsing, the
  render pipeline, etc.).
- `crates/rustline` — the **tmux front-end binary**. A `clap` CLI that builds a
  `Context` from CLI args + local system reads, discovers/registers WASM
  plugins, calls the core, and prints.
- `crates/rustline-wasm` — the **WASM plugin host**: an Extism (wasmtime)
  runtime with **nine** host functions — **eight** capability-gated
  (TTL-cached + raw network + state + arbitrary-file read/write + **command
  exec, cached and uncached**) plus one capability-free guest logger,
  `rl_log` — per-plugin allowlists and a sandboxed/quota-bounded state
  dir, and discovery of `*.wasm` files into `Widget` registrations. Zero
  ambient authority — guests run with wasi off and no built-in Extism
  HTTP/FS; every effect is host-checked (`rl_log` is the sole intentional
  exception — see invariant N1). Reusable verbatim by a future daemon
  front-end — the W48 render daemon already does this, building its warm
  `Registry` via the same `register_plugins`.
- `crates/rustline-plugin-sdk` — the **guest-side SDK** a WASM plugin depends
  on (W39): one crate bundling typed host-capability wrappers (`http_get`,
  `http_get_cached`, `state_read`/`state_write`, `file_read`/`file_write`,
  `log`), re-exports of the shared wire types (`GuestRender`, `WireContext`,
  `Segment`/`Style`/`Color` from `rustline-abi`), the `active_format` toggle
  helper, and the `export_plugin!` macro (wires the `name`/`render`/
  `abi_version` Extism exports in one line). Links the real Extism host imports
  **only on `wasm32`**; on the host target the wrappers degrade to
  `HostError::Unavailable` so a plugin's pure logic still compiles and
  unit-tests under `cargo test`. All five example plugins depend on it.

`plugins/` holds example/third-party plugin sources, each an **excluded**
workspace member (own `Cargo.lock`, built for `wasm32-unknown-unknown`):

- `plugins/weather` — the worked example: a Nerd-Font condition icon + °F for a
  configured zip code from wttr.in, fetched via the host's TTL-cached GET
  (`rl_http_get_cached`) so it hits the network at most once per `refresh_secs`
  (the host owns the cache; the guest no longer manages its own state dir).
- `plugins/counter` — a state-backed counter: reads its previous count via
  `rl_state_read`, increments it, writes it back via `rl_state_write`, and
  renders it, demonstrating the sandboxed-state capability (no network at
  all) plus `rl_log` on a failed write.
- `plugins/filewatch` — reads a configured file via `rl_file_read` (gated by
  the user's `allowed_paths`) and renders a first-line/line-count summary,
  demonstrating the arbitrary-file-read capability; a denied/missing file
  logs why via `rl_log` and falls back to `down_format`.
- `plugins/httpget` — a plain (uncached) `rl_http_get` widget that fetches a
  configured URL and renders a snippet of the body, contrasting with
  `weather`'s TTL-cached path; a non-2xx status or transport error logs why
  via `rl_log` and falls back to `down_format`.
- `plugins/cmdrun` — runs a configured `program` + `args` via the host's
  `rl_exec`/`rl_exec_cached` and renders a snippet of stdout, demonstrating
  the capability-gated exec capability; a denial, timeout, or non-zero exit
  logs why via `rl_log` and falls back to `down_format`.

### Render pipeline

`Context` → each widget's `render(&Context) -> Vec<Segment>` → `assign_palette`
fills segment backgrounds → `render_region` joins segments with powerline
separators into tmux `#[fg=..,bg=..]` markup. For the window list, tmux calls
`render window` once per window and each window is rendered as a self-contained
**rounded pill** (`render_window_pill`, not `render_region`/`assign_palette`):
rounded caps (`` / ``) colored `fg=pill,bg=bar_bg`, the active window in the
accent fill + bold and inactive windows in a gray fill — all six colors/glyphs
themeable via `[theme]` (see Config). WASM plugins implement the same `Widget` trait as
built-ins (via `WasmWidget`) and are resolved into the same registry, so they
flow through this pipeline unchanged.

A widget that opts into click-to-toggle (a non-empty `alt_format` and a name
that fits tmux's 15-byte range limit) reports `Widget::range_name() ->
Some(name)`; `render_named_region` then calls `render_region_ranged` instead of
`render_region`, wrapping that widget's cells in `#[range=user|NAME]…#[norange]`
so a tmux `MouseDown1Status` binding can tell which widget was clicked (see CLI
below: `rustline click`). With every widget's range `None`, output is
byte-identical to `render_region`.

The core types (`Context`, `WindowCtx`) live in `rustline-core` (they carry
`chrono`); the output types (`Segment`, `Style`, `Color`) live in
`rustline-abi` and are re-exported by `rustline-core`. All derive
`serde::Serialize + Deserialize` **on purpose** — that is the WASM plugin ABI.
A plugin's `render` crosses the Extism boundary as a JSON string (WebAssembly
can only pass scalars + linear memory); the JSON is just the serde encoding of
these shared types, not a design shortcut. Keep them serializable.

## Module map

`rustline-core`:
- `segment.rs` — `pub use rustline_abi::{Color, Segment, Style};` — a
  re-export module so existing `rustline_core::segment::…` paths keep
  resolving now that the types themselves live in `rustline-abi`.
- `context.rs` — `Context` (session/window/pane ids, `pane_current_path`,
  `home`, `hostname`, `loadavg: Option<[f64;3]>`, `now: DateTime<Local>`,
  `window: Option<WindowCtx>`, `interfaces: Vec<NetIface>`,
  `battery: Option<Battery>`, `cpu: Option<CpuUsage>`,
  `cpu_history: Vec<f32>`, `memory: Option<MemInfo>`, `mem_history: Vec<f32>`,
  `git: Option<GitInfo>`, `disk: Option<DiskInfo>`,
  `throughput: Option<Throughput>`, `uptime: Option<u64>` (seconds),
  `media: Option<MediaInfo>`,
  `disks: BTreeMap<String, DiskInfo>` (keyed by mount) and
  `throughputs: BTreeMap<String, Throughput>` (keyed by interface, `""` =
  aggregate) — the per-instance reading maps multiple `disk`/`throughput`
  widget **instances** (W46, see `config.rs` below) each consult by their own
  `mount`/`iface_key`; both `#[serde(default)]` and the only two `Context`
  fields NOT mirrored into `WireContext` (invariant #2 — `throughput`/
  `uptime`/`media`/`cpu_history`/`mem_history` ARE mirrored as of W54, see
  below) — `Context.disk`/`Context.throughput` stay populated with
  just the *base* widget's reading (the WASM mirror; a guest never sees the
  maps),
  `os: String`, `arch:
  String`, `toggled: BTreeSet<WidgetName>`, `colors: ThemeColors`), plus
  `Context::default()` (an empty, epoch-timestamped instance, so test/synthetic
  construction sites can use struct-update syntax instead of spelling out
  every field as the type grows), and `WindowCtx`. `NetIface`, `Battery`/
  `BatteryState`, `CpuUsage`, and `MemInfo` all now live in `rustline-abi`
  (chrono-free, so a WASM guest can share them directly) and are re-exported
  here — the same `Segment`/`Style`/`Color` precedent as `segment.rs` — so
  existing `rustline_core::context::…`/`rustline_core::…` paths keep
  resolving. `NetIface { name, ipv4: Ipv4Addr }` (one non-loopback IPv4 interface,
  read once at `Context`-build time; the IP widgets select from this list
  rather than touching the OS mid-render). `Battery { percent: u8, state:
  BatteryState }` and `BatteryState { Charging, Discharging, Full, Unknown }`
  (serde `snake_case`) are a battery snapshot read once at `Context`-build
  time; `CpuUsage { percent: f32 }` and `MemInfo { total_bytes, used_bytes,
  available_bytes }` (all bytes as `u64`) are the cpu/memory snapshots,
  likewise read once at `Context`-build time; `GitInfo { branch, ahead: u32,
  behind: u32, staged: u32, unstaged: u32 }` is a git branch/status snapshot,
  read once at `Context`-build time ONLY when `git` is in the active layout
  (mirroring the `cpu`/`memory` read-gating below); `DiskInfo { total_bytes,
  used_bytes, available_bytes }` (all bytes as `u64`) is a filesystem-usage
  snapshot for a configured mount, likewise read once at `Context`-build time
  ONLY when `disk` is in the active layout; `throughput` (a `Throughput`
  down/up bytes-per-sec snapshot) is read once at build time, gated the same
  way on `throughput` being in the active layout, and is `None` on the first
  invocation (a rate is a delta — nothing yet to diff against; invariant #6);
  `cpu_history`/`mem_history` (`Vec<f32>`, `#[serde(default)]`) are the recent
  cpu%/memory-used% readings (oldest first) feeding the `{spark}` placeholder,
  populated at build time from a persisted ring ONLY when the respective
  widget's `format` **or** `alt_format` references `{spark}` (W56; empty
  otherwise — no history I/O);
  `uptime` (seconds since boot) and
  `media` (a `MediaInfo` now-playing snapshot) are read once at build time and
  gated the same way on their widget being in the active layout. `throughput`/
  `uptime`/`media`/`cpu_history`/`mem_history` are all mirrored into
  `WireContext` (W54, purely additive) — a WASM guest now sees a
  field-for-field `Context` mirror minus only `disks`/`throughputs` (the
  per-instance maps above); `os`/`arch` come from
  `std::env::consts::OS`/`ARCH`; `toggled` (`#[serde(default)]`) is the set of
  widget/plugin names the user has click-toggled to their `alt_format` view,
  read once at `Context`-build time from the toggles state file (invariant #1)
  and serialized to WASM guests; `colors` (`#[serde(default)]`) is the
  resolved theme's `fg`/`bar_bg` plus its four semantic colors
  (`success`/`info`/`warning`/`error`), copied in at `Context`-build time so
  threshold-aware widgets and WASM guests can style alert badges without
  seeing `Theme` (see Themes under Config).
- `render.rs` — `render_region(Direction, &[Segment], &Theme) -> String`, the
  load-bearing powerline renderer (hard `` `` / soft `` `` separators, edge
  blending to `bar_bg`); `render_window_pill(text, is_current, &Theme) ->
  String`, the window-list rounded-pill renderer (rounded `` / `` caps colored
  `fg=pill,bg=bar_bg` — the *opposite* of a pointed separator); `RangeGroup`
  (a widget's segments plus its optional clickable range name, typed
  `range: Option<RangeName>` — see `range_name.rs` below — so the bytes
  interpolated into `#[range=user|…]` are safe by construction rather than
  re-checked here) and
  `render_region_ranged(Direction, &[RangeGroup], &Theme) -> String`, which
  brackets each clickable group in `#[range=user|NAME]…#[norange]` with
  separators/edges kept outside any range — byte-identical to `render_region`
  when every group's range is `None`; `Theme`
  (palette, glyphs, colors, incl. the six `win_*` pill fields and the four
  `success`/`info`/`warning`/`error` semantic colors) with `Default`, plus
  `Theme::colors() -> ThemeColors` (projects the fg/bar_bg/semantic fields for
  `Context.colors`). Also the private `sanitize_text` — the single
  markup-escaping choke point (invariant #8): it doubles `#` and replaces C0
  control characters, and is applied by all three of `render_region`,
  `render_region_ranged` and `render_window_pill` to segment text and nothing
  else. It borrows when there is nothing to change, so the common path
  allocates nothing.
- `themes.rs` — `builtin_theme(name) -> Option<Theme>` and
  `builtin_theme_names() -> &[&str]`, the seven built-in themes (`default`,
  `pastel-rainbow`, `nord`, `gruvbox`, `catppuccin-mocha`, `tokyo-night`,
  `dracula`); each
  is a complete `Theme` (all fields, incl. semantics), and every non-`default`
  theme is multi-accent (`palette.len() >= 4`) using truecolor (`Color::Rgb`).
  See Themes under Config for the full list and layering rules.
- `range_name.rs` — `RangeName`/`NameError`: the one definition of invariant
  #7's clickable-name rule (non-empty, at most `RANGE_NAME_MAX_BYTES` bytes —
  moved here from `render.rs` — `[A-Za-z0-9_-]` only, and not the reserved
  name `"window"`). `RangeName::parse` is the sole constructor, checking (in
  order) empty → bad char → reserved → too long, so a name that is both
  charset-invalid and too long always reports the charset violation, never
  masked by the length check; `RangeName::of_widget(&WidgetName) ->
  Option<RangeName>` bridges a widget's identity into its clickable form.
  Holding a `RangeName` is proof its bytes are safe to interpolate into
  `#[range=user|…]` verbatim — nothing downstream re-checks length or
  charset. Closes the range-forging hole: a charset-violating name (e.g.
  carrying a `#`) could previously still pass the old length-only check and
  reach `#[range=user|…]` markup unescaped.
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
  &[f32], max: f32) -> String` (W45), a shared pure Unicode block-eighths
  sparkline (one glyph per historical reading, clamped `0..=1` of `max`) backing
  the `{spark}` placeholder in both `cpu.rs` and `memory.rs`; the history itself
  rides `Context.cpu_history`/`mem_history`, populated at the bin's build edge
  only when a widget's `format` **or** `alt_format` references `{spark}` (W56;
  see `history.rs`/`cpu.rs`/
  `memory.rs` in the bin). `cpu.rs` is the `cpu` widget: pure over
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
  bytes-per-sec as human-readable `format_bytes` sizes suffixed `/s`, e.g.
  `1.2M/s`), plus `alt_format`/`down_format`; NOT threshold-aware (a rate has
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
- `assemble.rs` — `assign_palette`, `render_named_region` (panic-guarded per
  widget via `catch_unwind`; now range-wraps via `render_region_ranged`,
  remembering each widget's `range_name()` across the palette-assignment
  flatten/regroup, and keying each widget's color override off the `(name,
  widget)` pairs `Registry::resolve` now returns (W53) — the old second
  `resolved_names`/`registry.contains` re-filter is gone), `render_window`
  (wraps the `windows` text in a
  themed rounded pill via `render.rs::render_window_pill`, keyed on
  `ctx.window.is_current`; still `catch_unwind`-guarded → `""` on panic/no
  window; the window pill is never clickable — `render window` has no
  `--plugin-dir` and no range wrapping), and `render_windows(windows:
  &[WindowCtx], &Registry, &Theme) -> String` (W42, the batched two-line
  window-list render). `render_window` and `render_windows` now share a
  private `window_pill` helper (one window → one pill, factored out of
  `render_window`'s prior body); unlike the singular path, `render_windows`
  DOES wrap each pill in `#[range=window|<index>]…#[norange]` — it re-emits
  the same `range=window|IDX` markers tmux's own `#{W:}` loop used to (see
  tmux integration model below), so window-select clicks keep working. Pills
  are joined with no separator (each is self-contained).
- `config.rs` — `Config` (TOML): `layout`, `theme`, `widgets`, a top-level
  `plugin_dir: Option<String>`, and a typed `plugins: HashMap<WidgetName,
  PluginConfig>` table (see Config below). `Config::load` is **total**
  (missing/invalid file → `warn!` + defaults). `ThemeConfig` is now a **full
  optional mirror of `Theme`** (every field, incl. the separators and the four
  semantic colors) plus a `base: Option<String>` selector; `apply_to(&self,
  &mut Theme)` applies each `Some` field onto a theme (skipping `base`) and
  `from_theme(&Theme) -> ThemeConfig` produces an all-`Some` mirror (used by
  `theme new` to scaffold a file). `Config::to_theme_over(base: Theme) ->
  Theme` layers the inline `[theme]` overrides onto an already-resolved base
  (used by the binary, which resolves `base` themes-dir-file-first — see
  Themes below); `Config::to_theme()` keeps its old signature for existing
  callers/tests but now also resolves a **built-in** `base` name before
  applying overrides (unknown/absent `base` falls back to `Theme::default()`).
  `Config::load_reporting` returns the load-failure
  message instead of logging it, so the binary can install its log subscriber
  first and then emit the `"invalid config"` warning into the file. Two
  per-widget option groups are `#[serde(flatten)]`ed into every clickable
  widget's `[widgets.<name>]` table: `ColorOverride { fg, bg: Option<Color> }`
  (W29, an explicit color pin applied centrally in `render_named_region`; see
  Config) and `ClickBindings { left_click, right_click, middle_click:
  Option<ClickBinding> }` (W36, the config-value `ClickBinding` enum —
  `{ toggle = bool } | { open_url } | { run }`). `Config::click_map() ->
  HashMap<&str, WidgetClick>` projects each widget's toggleability + bindings
  for the binary's `resolve_click`; `WidgetClick` distinguishes a known-but-
  not-toggleable built-in from an absent name (a plugin/unknown range) so the
  pre-W36 plugin-flip behavior is preserved (invariant #7). `datetime` gains a
  `timezone: Option<String>` (W30, an IANA zone via `chrono-tz`). `PluginConfig`
  gains `source: Option<PluginSource>` (a typed enum that still accepts a bare
  `owner/repo` string) plus `checksum`/`tag: Option<String>` recorded by
  `plugin install` (W38). `WidgetOpts` gains a `throughput: ThroughputOpts`
  (W47 — `format`/`alt_format`/`down_format`/`interface`/color/click, mirroring
  the other opt-in widgets), and `CpuOpts`/`MemoryOpts` each gain a
  `spark_width` (W45, default 8 — the `{spark}` history ring length, consulted
  when `format` **or** `alt_format` references `{spark}`, W56). `Config` also
  gains `pub
  instances: HashMap<WidgetName, Value>` (`#[serde(default)]`, W46) — one
  raw `[instances.<name>]` table per named widget instance; the entry's
  `kind` key names one of a closed, 16-variant `WidgetKind` enum (T5 — serde
  `snake_case` with explicit `datetime`/`loadavg` renames, since plain
  snake_case would emit `date_time`/`load_avg`; `WidgetKind::ALL`/`as_str`/
  `parse`/`is_instanceable`), and the rest of the table is that kind's own
  Opts (re-parsed per kind via `try_into` rather than a typed
  `WidgetInstance`, so the existing `<Kind>Opts` structs are reused verbatim —
  see Config below for the TOML shape). `pub fn instance_kind(v: &Value) ->
  Option<WidgetKind>` reads and parses the `kind` key. `pub fn
  resolved_instances(&self) -> &BTreeMap<WidgetName, InstanceParse>` (T5) is
  the parse-once memo (a `OnceLock`, lazily built and reused for the
  `Config`'s lifetime) that replaced the old per-consumer `instance_meta`
  dispatch: `InstanceParse::{Ok(InstanceSpec), NoKind, UnknownKind(String)}`,
  with `InstanceSpec` a 12-variant enum wrapping the existing `<Kind>Opts`
  structs one per instanceable kind. `color_overrides`/`click_map`/
  `layout_kinds`/`disk_mounts`/`throughput_interfaces`/
  `spark_referenced_in_layout`/`instances_of_kind`, AND
  `Registry::with_builtins`'s registration pass, all consume this memo, so a
  table with several consumers deserializes exactly once per `Config`, not
  once per consumer per render/keystroke. `pub fn
  layout_kinds(&self, layout: &[WidgetName]) -> BTreeSet<WidgetKind>` maps each
  layout entry to its kind (a built-in name maps to itself; an instance name
  maps to its declared `kind`) for kind-aware read-gating; `pub fn
  disk_mounts(&self, layout: &[WidgetName]) -> BTreeSet<String>` and `pub fn
  throughput_interfaces(&self, layout: &[WidgetName]) -> BTreeSet<Option<String>>`
  collect the distinct mounts/interfaces the layout's `disk`/`throughput`
  entries (base + every instance) actually reference, for the per-mount/
  per-interface reads below. `pub fn spark_referenced_in_layout(&self, layout:
  &[WidgetName], kind: WidgetKind) -> bool` (W57) is the `{spark}`-gate's own kind-aware
  scan: true iff the layout's base `cpu`/`memory` widget OR any
  `[instances.<name>]` of that `kind` in the layout has `{spark}` in its
  `format`/`alt_format` (shared private `refs_spark` helper) — `build_context.rs`
  calls this instead of checking only the base widget, so a `{spark}` that
  appears only on an instance (no base widget of that kind in the layout at
  all) still populates the shared `Context.cpu_history`/`mem_history` ring
  (supersedes W56's base-only check). The built-in-wins
  precedence guard — `color_overrides()`/`click_map()`/`layout_kinds` all skip
  a `[instances.<name>]` entry whose name collides with a built-in, mirroring
  `Registry`'s own collision skip (W46 review fix — an instance was initially
  able to silently hijack a same-named built-in's projected color/click) — is
  now `WidgetKind::parse(name).is_some()` (T5); the hand-maintained `const
  BUILTIN_WIDGET_NAMES: [&str; 16]` array and its `is_builtin_widget_name`
  helper are gone, since `WidgetKind::ALL` is the same information already.
  `color_overrides()`/`click_map()` (previously widgets-only) now also
  project each non-colliding instance via `resolved_instances()`'s
  `InstanceParse::Ok(spec)`, keyed by the
  instance name, so instance color overrides and click bindings flow through
  `render_named_region`/`resolve_click` unchanged. `PluginConfig` also gains
  `allowed_commands: Vec<String>` (the exec capability's allowlist, same
  glob-or-`re:` shape as `allowed_urls`/`allowed_paths`, matched against the
  **canonical argv string** — see `rustline-wasm`'s `argv.rs` below — not just
  the program name; empty denies by default like the other two). The widget
  manager's pure layout algebra lives here too: `Region { Left, Center,
  Right }` (`Region::ALL`, `as_str`/`parse`); `Layout::{get, get_mut, find}`
  (`find` locates a name's current `(Region, usize)`, since a widget may sit
  in at most one region — invariant #7); `LayoutEditError { AlreadyPresent,
  NotPresent, NoOp }` (every variant means nothing was mutated) and
  `LayoutChange { name, from, to }` (`from`/`to` are `None` for an add/remove
  respectively); `layout_enable`/`layout_disable`/`layout_move`/`layout_nudge`
  are the four pure mutators (insert/remove/relocate/shift-in-place), each
  returning a `LayoutChange` or refusing with a `LayoutEditError` — the one
  definition of a legal edit shared by `widget_cmd.rs` (CLI) and
  `widget_tui.rs` (the interactive editor). `WidgetPlacement { name, summary,
  source, placement }` and `widget_placements(cfg, descriptors, plugin_names)
  -> Vec<WidgetPlacement>` enumerate every widget a user could place — built-ins
  (registration order) → instances (sorted) → plugin stems (sorted), a
  candidate's own `WidgetSource` deciding its group — with its current
  placement if any; this is `widget list`/`widget edit`'s one source of truth
  for "what exists and where is it."
- `ansi.rs` — two renderers over one scanner, for the two kinds of sink that
  consume tmux markup. `tmux_to_ansi(&str) -> String` transcodes it to ANSI SGR
  (`colourN` → 256-color, `#rrggbb` → truecolor, named → basic) for a sink the
  **terminal** interprets: `render … --preview`, `theme show`/`pick`.
  `parse_markup(&str) -> Vec<StyledSpan>` resolves the same markup into
  `StyledSpan { style: Style, text: String }` runs for a sink that carries its
  own style type and never interprets escapes — a `ratatui` cell buffer draws
  an ANSI escape as *literal visible characters* and applies no styling, so
  `widget_tui.rs`'s preview strip uses this and maps each span onto a ratatui
  `Style`. tmux directives are stateful (`#[bg=blue]` leaves an earlier
  `fg=red` alone), which a terminal tracks for itself but a cell buffer can't,
  so `parse_markup` accumulates the running style and snapshots it per run;
  `fg=default`/`bg=default` reset that channel to `None` ("the terminal's own
  default", not black). Both share a private `scan` so where a directive
  starts/ends — and what an unterminated `#[` means — has one definition.
- `atomic_write.rs` — `write_atomic(path, contents: &[u8]) -> io::Result<()>`,
  the workspace's **one** atomic-write primitive (B15+B17): stage into a
  per-writer, per-call temp file in the same directory
  (`<path>.<pid>.<nanos>.<counter>.tmp`) and `rename` onto the target, so a
  concurrent reader always sees either the old contents or the new ones,
  never a torn write — and, being uniquely named per call rather than a
  shared `<name>.tmp`, two rustline processes (tmux spawns `render left`/
  `render right` as separate processes every tick) staging the same target
  can no longer truncate each other's temp file. Every persistence site in
  the workspace goes through this: `sample_store.rs`, `toggles.rs`, `cpu.rs`
  (below), and `rustline-wasm`'s `perform::perform_state_write`, `paths.rs`,
  and `cache.rs`. Trade-offs accepted and documented on the module: a
  crash-orphaned staging file is swept inside the WASM cache namespaces
  (`evict_namespace`) but not under a plugin's state-dir root or
  `~/.local/state/rustline/` (open finding); the ~33-char staging suffix can
  push an already-long target name over the filesystem's name limit (only
  reachable via a plugin's state relpath, which has no length bound); and the
  brief on-disk overlap of the old file and the staging file can transiently
  over-count against a plugin's state-dir quota.
- `diag.rs` — the `warn_once` seam: `warn_once(key: &str, emit: impl Fn())`
  calls `emit()` unless a process-wide hook, installed once via
  `set_warn_once_hook`, has already seen `key`. **Fails open**: with no hook
  installed (every unit test, `rustline-core` used standalone) it always
  emits, so a dedup layer can never be the reason a diagnostic goes missing.
  The hook itself lives in the `rustline` bin crate's `warn_once.rs` (below) —
  the only thing that ever calls `set_warn_once_hook`. Every `warn!`/`info!`
  site here and in `rustline-wasm` that describes a *static misconfiguration*
  (one that would otherwise re-fire once per render, forever, at up to
  ~86,400 lines/day) routes through this function instead of calling
  `tracing::warn!`/`info!` directly; a genuine per-occurrence runtime failure
  (a plugin render failing, a cache write failing, a denial) never does. `key`
  must identify both the site and its varying payload (e.g.
  `"unknown-widget:memroy"`) so two distinct misconfigurations each warn once
  rather than one suppressing the other.

`rustline-abi`:
- `lib.rs` — `Segment { text, style }`, `Style { fg, bg, bold }`,
  `Color { Named | Indexed(u8) | Rgb(u8,u8,u8) }` (+ `Color::to_tmux()`),
  `ThemeColors { fg, bar_bg, success, info, warning, error }` with `Default`
  (matches `Theme::default()`'s values) — the semantic-color snapshot carried
  on `Context.colors` so widgets and WASM guests can style alert badges
  without seeing `Theme`; `NetIface { name, ipv4: Ipv4Addr }`; `Battery
  { percent, state: BatteryState }`/`BatteryState { Charging, Discharging,
  Full, Unknown }`; `CpuUsage { percent: f32 }`; and `MemInfo { total_bytes,
  used_bytes, available_bytes }` — all moved here from
  `rustline-core::context` (chrono-free, so a WASM guest can share them
  directly) and re-exported by `rustline-core` — and `GitInfo { branch,
  ahead, behind, staged, unstaged }` (chrono-free, so no separate wire mirror
  is needed; the same type rides `Context.git` and `WireContext.git`), plus
  `DiskInfo { total_bytes, used_bytes, available_bytes }` (same chrono-free,
  no-separate-wire-mirror shape, riding `Context.disk`/`WireContext.disk`).
  Also `WireWindowCtx` (a chrono-free mirror of `WindowCtx`) and `WireContext`
  (the guest-side, typed mirror of `Context` itself — field-for-field
  identical except `now` is a plain RFC3339 `String` rather than
  `DateTime<Local>`), plus `GuestRender { context: WireContext, config:
  serde_json::Value }` — the whole shape a guest's `render` export
  deserializes (W26), so a plugin no longer hand-walks an untyped
  `serde_json::Value` for its `Context` input; `plugin new`'s scaffold and the
  `counter`/`filewatch`/`httpget` examples all use these typed wire structs.
  Also `MediaInfo { artist, title, status }` (W41, a now-playing snapshot
  riding `Context.media`; NOT in `WireContext` — not exposed to guests, like
  `Context.uptime`), `Throughput { down_bytes_per_sec, up_bytes_per_sec }`
  (W47, a network-rate snapshot riding `Context.throughput`; also NOT in
  `WireContext`, same as `MediaInfo`), and `pub const ABI_VERSION: u32 = 1` (W32) — the host↔guest
  ABI version the host stamps onto `RenderInput.abi_version` and a guest may
  echo from its optional `abi_version()` export (see `rustline-wasm`'s
  `abi_decision`).
  Also the four host-effect wire-result types (`HttpResult`,
  `CachedHttpResult`, `ReadResult`, `WriteResult`), moved here (W51) as the
  single canonical definition a host function's JSON response decodes into —
  previously duplicated between `rustline-wasm`'s `abi.rs` and
  `rustline-plugin-sdk`, kept in sync only by the e2e test; each carries a
  struct-level `#[serde(default)]` so a guest's decode stays forward-compatible
  with a host that adds/omits a field. `rustline-wasm` and the SDK re-export
  them. Two more joined them for the exec capability: `ExecResult { ok,
  status: i32, stdout, stderr, error, truncated }` (`rl_exec`'s result — `ok`
  means "allowed and ran to completion", NOT "succeeded"; a non-zero exit is
  `ok: true` with a non-zero `status`, `status: -1` on a signal kill or
  timeout) and `CachedExecResult` (`rl_exec_cached`'s result, adding
  `stale`/`age_secs` — same "usable result present, fresh or stale"
  convention as `CachedHttpResult`). Same struct-level `#[serde(default)]`
  forward-compatibility as the other four.
  Also `WidgetName` (T6, `#[serde(transparent)]`, `Borrow<str>`) — the
  identity newtype invariant #7 keys off: `Context.toggled`/
  `WireContext.toggled: BTreeSet<WidgetName>`, `Layout`'s three region
  `Vec<WidgetName>`s, and `Config.plugins`/`Config.instances`'s
  `HashMap<WidgetName, _>` keys all use it; `Registry::resolve` returns
  `Vec<(WidgetName, Box<dyn Widget>)>`; the twelve clickable widget structs
  plus `WasmWidget` carry a `name: WidgetName` field; `rustline-core`'s
  `RangeName::of_widget` bridges a `WidgetName` into the render/click-toggle
  boundary's `RangeName`. Wire JSON and TOML shapes stay byte-identical.
  The WASM wire types, re-exported by `rustline-core`.

`rustline-wasm`:
- `allow.rs` — `AllowSet<K: CapKind>`/`Pattern`: each `allowed_urls`/
  `allowed_paths`/`allowed_write_paths` entry is a glob by default or a regex
  when prefixed `re:`; deny-by-default (empty set matches nothing); malformed
  patterns are logged and skipped. `AllowSet` is phantom-tagged per capability
  family (T3): marker types `UrlCap`/`ReadPathCap`/`WritePathCap`/`CommandCap`
  each carry a `DENIAL: DenialKind` and a typed subject (`Url<'a>`/
  `ResolvedPath`/`CanonicalArgv`), so a URL can never be checked against a
  path (or command) allowlist, or vice versa — compiler-enforced, not just a
  runtime convention. `allowed_commands` (the exec capability's allowlist)
  compiles through this same `AllowSet<CommandCap>`. `ReadPathCap`/
  `WritePathCap` additionally implement the `PathCap`-bounded `check_path`,
  which consumes a `ResolvedPath` and mints the `AllowedPath` token (see
  `state.rs` below) on a match — the only way to obtain one.
- `argv.rs` — `canonical_argv(program, args) -> CanonicalArgv`, the exec
  capability's single **matching key** an `allowed_commands` pattern is checked
  against — never executed, since the host always spawns `program` + `args`
  directly with no shell. `CanonicalArgv` is a newtype (T3) whose sole
  constructor is this function, so the `allowed_commands` gate can never be
  checked against a hand-built string that bypassed it — that's type-enforced.
  The exec cache key is only derived from a `CanonicalArgv` by convention, at
  its single call site (`cache_path` in `cache.rs` still takes a bare `&str`
  key) — typing that parameter is open finding T22. Quotes an argument
  (single-quoted, POSIX-style, with
  an embedded `'` escaped as `'\''`) whenever it contains whitespace, a quote,
  or a backslash, or is empty, so two different argv vectors (e.g.
  `["log", "--author=a b"]` vs `["log", "--author=a", "b"]`) can never collapse
  onto the same canonical string and silently share a grant.
- `state.rs` — `sanitize_relpath -> SandboxRelPath` (rejects absolute/`..`
  paths for state I/O), `normalize_abs`/`resolve_for_allowlist -> ResolvedPath`
  (the symlink gate for arbitrary-file I/O, consulted before either read/write
  allowlist: by default denies outright on any symlink path component; when
  the plugin's `resolve_symlinks` is set, canonicalizes first — falling back
  to canonicalizing the parent for a not-yet-existing write target — and
  matches the allowlist against the *resolved* location, hard-denying rather
  than falling through if canonicalization fails; see the Config section
  below), and `dir_size`/`check_cap` (the quota primitives — `dir_size` is no
  longer walked per write; see `capability.rs`'s `state_size` memo below).
  `ResolvedPath`/`SandboxRelPath` are constructible only in this module (T2);
  `AllowedPath` (also here) is minted only by an allowlist's `check_path` on a
  match (see `allow.rs` above). The four filesystem-effect sites in
  `perform.rs` accept only these types, so "resolve → match → act" is a
  type-level fact rather than statement order.
- `paths.rs` — `expand_tilde`, `data_root`, `state_root`, `default_plugin_dir`
  (all under `$XDG_DATA_HOME/rustline`, falling back to `$HOME/.local/share/rustline`),
  plus `wasmtime_cache_config_path`/`ensure_wasmtime_cache_config` (W43): lazily
  writes `<state_root>/wasmtime-cache.toml` (via the same atomic temp-file +
  rename as `cpu.rs`'s snapshot store) — a `[cache]` block with ONLY
  `directory = <state_root>/wasmtime-cache/` and **no** `enabled` key (wasmtime
  43.x's `[cache]` is `deny_unknown_fields` and rejects `enabled`) — and returns
  its path, or `None` (best-effort, never panics) on any I/O failure or an
  unwritable root, so `build_plugin` degrades to no-cache rather than handing
  wasmtime an unusable config (N2). The cache dir is kept distinct from plugins'
  own state subdirs.
- `abi.rs` — `RenderInput` and `parse_render_output` (malformed JSON → empty
  `Vec`). The host-effect wire-result types (`HttpResult`, `CachedHttpResult`,
  `ReadResult`, `WriteResult`, and now `ExecResult`/`CachedExecResult`) all
  live in `rustline-abi` (W51, plus the exec pair added alongside `rl_exec`/
  `rl_exec_cached`) and are re-exported here, so existing `crate::abi::HttpResult`
  paths keep resolving (the same precedent as `rustline_core::segment`'s
  `Segment` re-export).
- `cache.rs` — pure helpers shared by both TTL caches: FNV-1a URL/argv→path,
  RFC3339 freshness (`age_secs`/`is_fresh`), quota-bounded `read_entry`/
  `write_entry`. `CacheEntry` carries `last_attempt_at` (when a refresh was
  last *tried*, success or not) alongside `fetched_at` (when one last
  *succeeded*). `is_fresh` itself is unchanged; the backoff is a second check
  in `perform.rs`'s cached-fetch paths (`perform_http_get_cached:90-102`,
  `perform_exec_cached:295-307`) that reuses `is_fresh` against
  `last_attempt_at` — when a last-good entry exists, a failing endpoint is
  retried at most once per TTL instead of every render. An endpoint that has
  *never* succeeded has no entry to back off against, so it is still fetched
  on every render — deliberately unchanged (see `perform.rs:172-175`).
  `write_entry` no longer wedges once its namespace hits quota:
  `evict_namespace` deletes entries oldest-`fetched_at`-
  first (a file that fails to parse sorts as oldest) until the namespace fits,
  and `write_entry` calls it only after `check_cap` first refuses the write —
  so a full state dir self-heals instead of failing every write from then on.
  `cache_path(state_dir, namespace, key)` takes a `namespace` argument —
  `HTTP_NAMESPACE`/`EXEC_NAMESPACE` — so the HTTP-response cache and the exec
  capability's own TTL cache (see `perform.rs` below) live in separate
  `<state_dir>/<namespace>/` subdirectories, both for collision-avoidance and
  so eviction in one never touches the other's entries.
- `capability.rs` — `CapabilityCtx`: one plugin instance's allowlists —
  `allowed_urls: AllowSet<UrlCap>`, `allowed_paths: AllowSet<ReadPathCap>`
  (read), `allowed_write_paths: AllowSet<WritePathCap>` (write, separate from
  `allowed_paths` — see the Config section below), `allowed_commands:
  AllowSet<CommandCap>` (the exec capability's gate) — each its own
  phantom-tagged type (T3) rather than four instances of the same untyped
  set — plus `resolve_symlinks`,
  state root, and quota, built from `PluginConfig` and held in Extism
  `UserData` so each instance only ever sees its own grants. `DenialKind`
  gains a `Command` variant alongside `Url`/`Path` (a denied write is also
  recorded as `Path`), so a denied exec call is recorded and reported
  (`rustline plugin denials`) the same way as a denied URL/path. `state_size`
  is an `AtomicU64` memo of the state dir's size: seeded by one `dir_size`
  walk on first use, then updated from each write's own return value rather
  than re-walked — `set_state_size`/`invalidate_state_size` (the latter
  called on any write path that fails, so a failed write can't leave the
  memo believing an eviction happened that didn't) keep it honest. `log_calls`
  is an `AtomicU32` counting `rl_log` calls toward `perform::
  MAX_GUEST_LOG_CALLS` — see `perform.rs`'s `perform_log` below for what
  "per process" means for a long-lived daemon instance.
- `fetch.rs` — `Fetcher` trait + `UreqFetcher` (the real rustls blocking HTTP
  client); the trait seam makes `perform_http_get`'s gating logic testable
  without a network.
- `run.rs` — `Runner` trait + `ProcessRunner`, the exec capability's
  counterpart to `fetch.rs`'s `Fetcher` seam: `Runner::run(program, args) ->
  Result<(exit_code, stdout, stderr), String>` lets every gate test in
  `perform.rs` run without spawning anything (the capability decision is made
  before `run` is ever called). `ProcessRunner` is the sole production impl —
  it spawns `program` directly with `args`, **no shell anywhere in the path**;
  stdin is closed (`Stdio::null()`, so a child reading stdin gets EOF instead
  of hanging), stdout/stderr are piped and capped at `MAX_OUTPUT_BYTES` (64
  KiB per stream, flagging `truncated`), and the whole run is bounded by
  `EXEC_TIMEOUT` (5 s, comfortably under Extism's 10 s plugin timeout) plus,
  at most, two short `OUTPUT_GRACE` periods (250 ms each) spent collecting
  output afterward. The child runs in its own process group
  (`process_group(0)`, the file's one
  `unsafe`) so a timeout/output-collection deadline kills the **group**, not
  just the immediate pid — otherwise a backgrounded descendant (`sh -c "long
  &"`) could keep the piped stdout/stderr open indefinitely and hang the
  render even after its immediate parent exits cleanly. `kill_group` only
  reaches that group, though: a descendant that escapes it via
  `setsid`/`setpgid` (a properly-daemonizing program, e.g. `emacs --daemon`,
  `ssh -f`, `tmux new-session -d`) keeps the piped fds open past the kill —
  `OUTPUT_GRACE` is what keeps that case bounded (the escaped descendant's
  own output is dropped, empty, rather than hanging the render thread and,
  transitively, the daemon's shared render lock).
- `perform.rs` — the eight capability-checked effect functions
  (`perform_http_get`, `perform_http_get_cached` — the TTL-cached GET:
  gate-first, 2xx-only caching, serve-stale, and now backing off to at most
  one refresh attempt per TTL window on a failure (see `cache.rs` above) —
  `perform_state_read/write`, `perform_file_read/write`, and
  `perform_exec`/`perform_exec_cached`); pure enough to unit-test directly,
  incl. the denied-case tests. Each of the eight delegates to a private
  outcome enum (`HttpOutcome`/`CachedHttpOutcome`/`ExecOutcome`/
  `CachedExecOutcome`/`ReadOutcome`/`WriteOutcome`, T4) with exactly one
  `From` impl converting it to its wire struct — the single author of every
  ok/error/stale flag combination, so a caller can't hand-assemble an
  inconsistent wire result; the wire JSON itself is unchanged, pinned by a
  `wire_pins` test module. `perform_file_read` gates on `allowed_paths`
  and `perform_file_write` on the separate `allowed_write_paths` — an entry in
  one never authorizes the other — both resolved through
  `state::resolve_for_allowlist` first (see `state.rs` above); `perform_exec`
  gates on `canonical_argv(program, args)` — the **whole** command line, not
  just `program` — against `allowed_commands` before ever touching `runner`
  (gate-first, invariant N1); a non-zero exit is `ok: true` (the process ran;
  its own exit status is data, not a host-level error). `perform_exec_cached`
  mirrors `perform_http_get_cached`'s shape with one deliberate difference:
  only a **zero-exit** run is written to the cache, and a **non-zero exit is
  returned as fresh data** rather than triggering a stale-serve fallback (it
  genuinely ran and reported that status) — only a run that couldn't happen
  at all (denied, spawn failure, timeout) falls back to the last-good cached
  entry with `stale: true`. Plus `perform_log(plugin, level, msg)` (W7): the
  one intentional **capability-free** host function — it only ever writes to
  the host's `tracing` subscriber, so unlike the eight above it has no
  `CapabilityCtx` allowlist to check and no denied-case test; an unrecognized
  `level` string degrades to `info` (keeping the original as a field) rather
  than dropping the message or panicking. Capability-free does not mean
  unbounded, though: a guest message is truncated to `MAX_GUEST_LOG_BYTES` (2
  KiB, the truncation marker budgeted inside that cap, not added on top), and
  calls are capped at `MAX_GUEST_LOG_CALLS` (256) per plugin instance per
  process — neither is a capability gate (nothing is denied, no
  `observe_denial` fires), just a size/rate bound so a buggy or hostile guest
  can't fill the user's disk. "Per process" means per render on the CLI path,
  but per **daemon lifetime** once a plugin instance is warm there (see
  `capability.rs`'s `log_calls` above), so a well-behaved plugin logging once
  a render goes quiet after `MAX_GUEST_LOG_CALLS` renders until the daemon
  restarts or reloads — a one-shot `warn!` at the boundary leaves a breadcrumb
  explaining the silence.
- `host.rs` — the `host_fn!` wrappers binding `perform_*` (incl.
  `rl_http_get_cached`, `rl_exec`/`rl_exec_cached`, and, W7, `rl_log`) to each
  plugin's `CapabilityCtx`, `build_plugin` (Extism instantiation: wasi off,
  fuel + timeout + memory caps, all **nine** host functions bound), `build_plugin_with_cache` +
  `CompileCache { Enabled, Disabled }` (W43): `build_plugin` now points
  wasmtime at an on-disk compile cache under the state root via
  `PluginBuilder::with_cache_config` (from `wasmtime_cache_config_path`) so a
  later cold spawn deserializes an unchanged plugin's precompiled artifact
  instead of re-running Cranelift — best-effort (no config producible → no
  cache, never a failed build or dropped plugin, N2; guest authority N1–N4
  untouched); `CompileCache::Disabled` (`with_cache_disabled`) is the bench's
  A/B toggle that forces a full compile. And `WasmWidget` (wraps an
  `extism::Plugin`; `Widget::render` degrades to empty segments on any
  error/timeout/malformed output; carries its own `name: WidgetName` and
  implements `range_name` via `RangeName::parse` (rejecting a
  charset-violating `.wasm` stem too, not just an over-length one — the same
  forgery hole `RangeName` closes elsewhere, see `range_name.rs` above) —
  the guest itself decides whether to honor `context.toggled`).
- `manifest.rs` — plugin capability *manifests* (W24): `PluginManifest
  { name, version, requested_urls, requested_paths, requested_write_paths,
  requested_commands }` (D3 added `requested_write_paths`, parsed separately
  from the read-only `requested_paths`) and `resolve_manifest(plugin_dir,
  name) -> Option<PluginManifest>`, which
  resolves a sidecar `<plugin_dir>/<name>.toml` first (primary; supersedes
  unconditionally, even if malformed) or else an embedded `rustline-manifest`
  wasm custom section (fallback, via a hand-rolled `find_custom_section`
  reader — no wasm-parsing dependency), else `None`. A manifest never grants
  anything itself — it's just a declaration `rustline plugin approve` turns
  into an allowlist write; a malformed manifest from either source is logged
  and treated as absent, never breaking discovery (N2).
- `denials.rs` — `FileDenialObserver` (W28), the production `DenialObserver`
  wired as the default in `register_plugins`: it dedupes `(plugin, kind,
  target)` and, for each newly-seen triple, both appends a JSON line to
  `<data_root>/denials.jsonl` (best-effort — a write failure `warn!`s, never
  panics, per N2) **and** logs a `tracing::warn!` ("capability denied; see
  `rustline plugin denials <plugin>`") — the JSONL file is no longer the only
  place a denial surfaces, so one shows up in the log the first time it
  happens instead of only on a manual `plugin denials` lookup; the log line
  reuses the same dedup as the record, so a repeated denial still logs once.
  `denials_path()`/`read_denials_at`/`read_denials` back `rustline plugin
  denials <name>`. NOTE: the record has no quota/rotation yet — a guest that
  varies its `target` defeats the dedup and grows the file unbounded (a
  follow-up; see WHATS-NEXT).
- `integrity.rs` — plugin binary integrity (W19): `sha256_hex(bytes: &[u8]) ->
  String`, the single definition of the digest both `plugin_install.rs` (at
  install time) and `register_plugins` (at load time) now share — the prior
  duplicate copy in `plugin_install.rs` is gone, so the two can never drift
  apart. `ChecksumVerdict { NotRecorded, Match, Mismatch { expected, actual },
  Malformed { recorded } }` and `verify_checksum(recorded: Option<&str>,
  bytes: &[u8]) -> ChecksumVerdict` accept 64 hex chars (case-insensitive), an
  optional `sha256:` prefix, and surrounding whitespace; an absent or blank
  `recorded` is `NotRecorded` (nothing to verify, loads as before), not a
  refusal. `register_plugins` — the real gate the bar and the daemon go
  through — matches the four `ChecksumVerdict` variants
  directly rather than calling `allows_load()`, so it can log a distinct
  `warn!` message per rejection reason: `NotRecorded`/`Match` permit
  registration; `Mismatch`/`Malformed` skip the plugin — a value this build
  cannot even parse as a digest **fails closed** (treated as a rejection, the
  same as a proven mismatch, never as "effectively unpinned").
  `ChecksumVerdict::allows_load()` is a convenience predicate with a single
  caller, `instantiate_named` (the `plugin run` dev harness): it uses the
  verdict only to decide whether to `warn!` before loading anyway (a harness
  iterating on a plugin you just rebuilt expects a stale digest), never to
  refuse — the harness always instantiates regardless of the verdict. Pure
  and I/O-free, so every rejection path is unit-tested without touching a
  real `.wasm`. Re-exported at the crate root as
  `rustline_wasm::{sha256_hex, verify_checksum, ChecksumVerdict}`.
- `lib.rs::{abi_decision, register_plugins, instantiate_named,
  discover_plugin_names}` — `pub fn discover_plugin_names(plugin_dir: &Path)
  -> Vec<String>` is the discovery half of `register_plugins` split out on its
  own: the sorted `.wasm` stems present in `plugin_dir`, **without reading or
  instantiating any of them** — a missing/unreadable dir yields an empty
  `Vec`, never an error. This is what lets `rustline widget list`/`widget edit`
  show plugin widgets in the AVAILABLE column without paying wasm cold-start
  or running guest code just to render a picker.
  `abi_decision(host: u32, guest: Option<u32>) -> AbiDecision`
  (`{Register, RegisterLegacy, Skip}`, W32) is the pure ABI-version handshake:
  a guest declaring the host's version registers; a guest with no
  `abi_version` export registers as legacy (existing plugins keep working); a
  mismatched version is **skipped, never registered**. `register_plugins`
  discovers `*.wasm` in the plugin dir, and for
  each name in the caller's `needed` list (i.e. actually referenced by a
  layout region — avoids paying wasm cold-start for unused plugins): skips a
  stem colliding with a built-in (built-in wins) or an unreadable file, then
  — right after reading the file's bytes and before building a
  `CapabilityCtx` or handing anything to wasmtime — verifies those bytes
  against `[plugins.<name>].checksum` (W19, `integrity::verify_checksum`): a
  `Mismatch` or `Malformed` verdict skips with a `warn!`, exactly like every
  other skip site here (a rejected plugin never reaches `build_plugin`,
  invariant N1); `NotRecorded`/`Match` proceed. Only then does it instantiate
  the plugin, run `abi_decision`, and verify the exported `name()` equals the
  filename stem (mismatch → `warn!` + skip), before registering a
  `WasmWidget` factory (each instance getting its own `FileDenialObserver`).
  A stem longer than 15 bytes gets a one-time `warn!` (not click-toggleable)
  but still registers.
  `instantiate_named(plugin_dir, name, &PluginConfig, observer)` builds a
  single named plugin (reusing `build_plugin` + `with_observer`) for the
  read-only `rustline plugin run` dev harness, capturing denials via a
  `CollectingObserver`. It deliberately runs the checksum gate **warn-only**:
  a bad verdict is logged but the plugin still instantiates and runs, because
  this harness is used right after rebuilding a plugin, where a recorded
  digest legitimately mismatches on every build. The real gate — the bar and
  the daemon — always goes through `register_plugins` above, which does not
  bypass it.

`rustline-plugin-sdk`:
- `lib.rs` — the guest-side SDK (W39). Typed host-capability wrappers
  (`http_get`, `http_get_cached`, `state_read`/`state_write`,
  `file_read`/`file_write`, `exec`/`exec_cached`, `log`) that call the host
  functions and decode their JSON responses into result structs, returning
  `Result<_, HostError>` (`{Call, Decode, Unavailable}`) instead of an untyped
  `serde_json::Value`; `exec`/`exec_cached` are thin wrappers over
  `rl_exec`/`rl_exec_cached` — `exec(program, args)` encodes `args` as JSON
  for the host boundary and decodes an `ExecResult`; `exec_cached` adds
  `ttl_secs`/`now` and decodes a `CachedExecResult`, documented (matching
  `perform_exec_cached`'s own doc comment precisely) to return a non-zero
  exit as fresh, uncached data and fall back to a stale cached entry only on
  a run that couldn't happen at all (denied, spawn failure, timeout); re-exports
  of `rustline_abi::{Color, GuestRender, Segment, Style, WireContext}` and the
  host-effect result types (`HttpResult`, `CachedHttpResult`, `ReadResult`,
  `WriteResult`, `ExecResult`, `CachedExecResult`); the `active_format` toggle
  helper and `LogLevel` enum; and the
  `export_plugin!` macro, which emits the `name`/`render`/`abi_version` Extism
  exports (the last returning the real `rustline_abi::ABI_VERSION`) from one
  line. The capability wrappers link the Extism PDK **only on `wasm32`**; on the
  host target they return `HostError::Unavailable` so a plugin's pure logic
  compiles and unit-tests under `cargo test`. The four original host-effect
  wire-result types (`HttpResult`/`CachedHttpResult`/`ReadResult`/`WriteResult`)
  are re-exported from `rustline-abi` (W51) rather than re-declared here — the
  previous SDK-local copy (kept in sync only by the e2e test) is gone, removing
  the drift risk; `ExecResult`/`CachedExecResult` were added there directly,
  so the exec capability never had a duplicate-copy problem to begin with.

`plugins/weather` (excluded workspace member, `wasm32-unknown-unknown`):
- `lib.rs` — pure logic (`code_to_icon`, `render_format`, `parse_wttr`,
  `select_weather_format` — the click-toggle exemplar: prefers a non-empty
  `options.alt_format` when the guest's `render` sees its own name, `"weather"`,
  in `context.toggled`) compiled and unit-tested on the host target, plus a
  `#[cfg(target_arch = "wasm32")] mod guest` with the Extism `name`/`render`
  exports and a single `rl_http_get_cached` guest import (the host owns the
  TTL cache).

`plugins/counter`, `plugins/filewatch`, `plugins/httpget`, `plugins/cmdrun`
(excluded workspace members, `wasm32-unknown-unknown`, same shape as
`plugins/weather` — pure logic unit-tested on the host target plus a
`#[cfg(target_arch = "wasm32")] mod guest`): four more worked examples, each
covering a host capability `weather` doesn't touch, and each logging its one
failure path via `rl_log` (W7) rather than staying silent:
- `plugins/counter/lib.rs` — `parse_count`/`next_count`/`render_format`; the
  guest reads its previous count via `rl_state_read`, increments it,
  persists the new value via `rl_state_write` (a failed write is `rl_log`ged
  and otherwise ignored — the count just won't have advanced next render),
  and renders it. No network capability at all.
- `plugins/filewatch/lib.rs` — `summarize`/`render_format`; the guest reads
  a configured `path` via `rl_file_read` (gated by the plugin's
  `allowed_paths`) and renders the first line + line count. A denial,
  missing file, or malformed read result is `rl_log`ged and falls back to
  `down_format` (empty → renders nothing, same convention as the built-in
  widgets' `down_format`).
- `plugins/httpget/lib.rs` — `extract_snippet`/`render_format`; the guest
  fetches a configured `url` via the **plain, uncached** `rl_http_get` (the
  deliberate contrast with `weather`'s `rl_http_get_cached`), checks the
  response is 2xx itself (`ok` on the wire only means "the transport
  completed", not "succeeded" — unlike the cached path, nothing upstream
  filters non-2xx for it), and falls back to `down_format` (same convention),
  `rl_log`ging the failure reason.
- `plugins/cmdrun/lib.rs` — `extract_snippet`/`render_format`/`select_mode`;
  the guest runs a configured `program`/`args` via the **plain** `rl_exec`
  (`ttl_secs == 0`, the default) or the **TTL-cached** `rl_exec_cached`
  (`ttl_secs > 0`) — the exec counterpart to `httpget`'s plain-vs-cached HTTP
  contrast — and renders the first line of stdout (capped at 60 chars). A
  denial, spawn failure, timeout, or non-zero exit is `rl_log`ged and falls
  back to `down_format` (same convention). Ships a sidecar `cmdrun.toml`
  manifest requesting `["uname", "uname *"]` (program `uname`, with or
  without arguments — not a `uname*` prefix glob, which would also match a
  program named `unamex`), so `rustline plugin approve cmdrun`
  demonstrates the exec-specific approval warning end to end.

`rustline` (bin):
- `cli.rs` — `clap` derive. A global `--config <path>` flag (W35, alongside
  `-v`) overrides the config-file path for every subcommand that reads/writes
  it. `render`, `config`, `plugin`, `theme`, and `widget` are subcommand
  *groups*; the `plugin` group now spans `list`, `url|path|cmd`, `approve`,
  `new`, `build` (W31), `run` (W34), `install`/`update`/`remove` (W38), and
  `denials` (W28) — `cmd` (`PluginCmd::Cmd(PatternCmd)`) is `plugin cmd
  list|add|remove`, the exec capability's `allowed_commands` counterpart to
  `url`/`path`, sharing the same `PatternCmd`/`pattern_cmd` plumbing (see
  `plugin_cmd.rs` below). `WidgetCmd { List { json }, Enable { name, region,
  index }, Disable { name }, Move { name, region, index }, Edit }` is the
  widget-manager group (see `widget_cmd.rs` below and CLI below).
  `init` (`InitArgs`) is the onboarding-wizard subcommand (see CLI below);
  `click` (`ClickArgs { range, button }`, both defaulted so an empty click is a
  parseable no-op) is a flat subcommand invoked by the tmux mouse binding
  (`MouseDown{1,2,3}Status`). `Command::Daemon(DaemonArgs)` (W48) wraps an
  `Option<DaemonCmd>` so a bare `rustline daemon` (no subcommand) parses and
  defaults to running the server, same as `rustline daemon run`; `DaemonCmd
  { Run(DaemonRunArgs { plugin_dir }), Status, Stop, Install(DaemonInstallArgs
  { write_only, binary }), Uninstall }` — `Run`'s `--plugin-dir` overrides
  discovery like `render`'s does, and `Install`'s `--write-only`/`--binary`
  drive the per-OS service installer (Linux systemd / macOS launchd — see
  `daemon_service.rs` below and CLI below).
- `battery.rs` — `read_battery()`, a `#[cfg(target_os)]` read surface (one of
  three — see `cpu.rs`/`memory.rs` below): a Linux sysfs
  (`/sys/class/power_supply/*/{capacity,status}`) arm and a macOS
  `pmset -g batt` arm, each delegating to a pure parser (`parse_linux`/
  `parse_pmset`) that is `#[cfg(any(target_os = …, test))]`-compiled so both
  are unit-tested on the Linux dev box even though only one reader arm
  compiles per platform. Any other platform, or a failed read, yields `None`.
- `cpu.rs` — `read_cpu()`, a `#[cfg(target_os)]` read surface. Linux now
  persists a `<state_root>/cpu-sample` snapshot (`CpuSnapshot { idle, total,
  ts }`) across invocations (W11): the fast path reads the current
  `/proc/stat` line and diffs it against that persisted snapshot if one
  exists and is fresh (`busy_from_snapshots`, within
  `CPU_SNAPSHOT_STALENESS_SECS` = 60s), returning with **no sleep**; only
  when there's no fresh persisted snapshot (first run, a stale one, or a
  backward clock) does it fall back to the classic two-sample read
  (`parse_proc_stat` + `busy_percent`, sampling `CPU_SAMPLE_WINDOW` ~120 ms
  apart). Either way the current reading is persisted afterward
  (`store_snapshot`, via the shared `write_atomic` primitive — see
  `rustline-core`'s `atomic_write.rs` above) so the *next*
  call can take the fast path — this is no longer a stateless delta. **macOS now
  shares this exact snapshot-delta path**: `read_cpu_macos_in` reads cumulative
  CPU ticks from the mach kernel via `read_mach_cpu_ticks`
  (`host_statistics(HOST_CPU_LOAD_INFO)`, the file's only `unsafe`, a scoped
  `#[allow(deprecated)]` over libc's mach bindings so the dep graph stays
  minimal) into the same `(idle, total)` `CpuTimes` shape Linux derives from
  `/proc/stat`, then takes the same zero-sleep cross-invocation delta (fast
  path) or the `CPU_SAMPLE_WINDOW` ~120 ms two-sample fallback, persisting to
  the same `<state_root>/cpu-sample` file — so the snapshot machinery
  (`CpuSnapshot`/`busy_from_snapshots`/`serialize_snapshot`/`parse_snapshot`/
  `load_snapshot`/`store_snapshot`/`busy_percent`) is now
  `#[cfg(any(target_os = "linux", target_os = "macos", test))]`. This
  **replaced the old `top -l 2 -n 0` shell-out** (`parse_top_cpu`, now gone),
  which slept ~1.4 s internally on every read: under the render daemon's shared
  render lock that pinned the whole refresh burst and dropped queued clients
  with EPIPE (see `daemon.rs`'s benign-disconnect logging). All the pure
  helpers (`parse_proc_stat`/`busy_percent`/`busy_from_snapshots`/
  `parse_snapshot`/`serialize_snapshot`) are
  `#[cfg(any(target_os = …, test))]`-compiled and unit-tested on the dev box,
  with the snapshot-cache tests injecting a tempdir rather than touching the
  real state dir. Unsupported platform or failed read → `None`. Also `read_cpu_history(state_dir, current_percent,
  spark_width) -> Vec<f32>` (W45): a SEPARATE persisted ring at
  `<state_root>/cpu-history` (distinct from the `cpu-sample` snapshot above) —
  load via `sample_store` + `history::parse_history`, push the current reading,
  truncate to `spark_width`, persist, and return — feeding the `cpu` widget's
  `{spark}` placeholder. Only called from the build edge when `cpu`'s `format`
  **or** `alt_format` references `{spark}` (W56).
- `memory.rs` — `read_memory()`, a `#[cfg(target_os)]` read surface: Linux
  reads `/proc/meminfo` (`MemTotal`/`MemAvailable` in kB, `parse_meminfo`);
  macOS reads the total via `libc::sysctlbyname("hw.memsize")` — memoized in a
  `OnceLock` (installed RAM can't change while the process lives), the file's
  one `unsafe`, replacing a per-render `sysctl -n hw.memsize` spawn the same way
  `cpu.rs` replaced its `top -l 2` shell-out — and still shells out to `vm_stat`
  for the page counters, deriving available bytes from free/inactive/speculative
  pages at the reported page size (`parse_macos_memory`). So, like `cpu.rs`,
  macOS memory is now **half FFI, half text parser** rather than the pure
  shell-out pair the cfg-gated-parser pattern describes: `memsize_to_text`
  renders the FFI `u64` back into the decimal text `parse_macos_memory` already
  took, which keeps the parser's contract — and its Linux-run unit tests —
  unchanged. Same cfg-gated pure-parser pattern as `battery.rs`/`cpu.rs`
  otherwise. Unsupported platform or failed read → `None` (the `sysctlbyname`
  failure is latched by the `OnceLock` for the process lifetime; see the
  function's doc comment). Also
  `read_memory_history(state_dir, current_percent, spark_width) -> Vec<f32>`
  (W45), the `memory` counterpart to `cpu.rs`'s `read_cpu_history`, persisting a
  `<state_root>/memory-history` ring for `memory`'s `{spark}`.
- `git.rs` — `read_git(path) -> Option<GitInfo>`, a platform-agnostic (no
  `#[cfg(target_os)]`) shell-out read: runs `git -C <path> status
  --porcelain=v2 --branch`, `None` on ANY failure (`git` missing, non-repo,
  non-zero exit, **or the spawn exceeding its timeout**). Delegates to the pure
  `parse_git_status(&str) -> GitInfo`
  (unconditionally unit-tested, no cfg-gating needed since there's no OS
  branching) — same pure-parser-behind-the-read-surface shape as
  `battery.rs`/`cpu.rs`/`memory.rs`, just keyed on tool availability rather
  than platform.
  This spawn, and the siblings in `media.rs`/`windows.rs`, go through
  `rustline_wasm::run::run_bounded` rather than a bare `Command::output()`:
  `pane_current_path` comes straight from tmux, so with the pane sitting on an
  unresponsive NFS/sshfs mount a blocking `git status` never returns, the
  `#()` job never completes, and the region goes permanently blank — and under
  the daemon it pins the shared render lock for the process's life, so every
  other client times out and falls back in-process onto the same hang. This is
  the protection the exec capability always had (`run.rs`'s `EXEC_TIMEOUT` +
  process-group kill, documented there as guarding exactly that lock); the
  built-in readers simply never got it. A timeout is treated like the existing
  non-zero-exit arm. Caveat worth knowing: `run_bounded` also inherits that
  capability's 64 KiB per-stream output cap, which was sized for plugin
  commands — a repo with roughly 600+ changed *tracked* files can undercount
  `{staged}`/`{unstaged}`. Porcelain v2 emits its headers first, so
  `{branch}`/`{ahead}`/`{behind}` are unaffected, and `tmux list-windows` /
  `playerctl` are nowhere near the cap.
- `disk.rs` — `read_disk(mount) -> Option<DiskInfo>`, a `statvfs(2)` read.
  Unlike `battery`/`cpu`/`memory`, the syscall itself is POSIX and needs no
  `#[cfg(target_os)]` split at all (it's available unconditionally on Linux
  and macOS); only the pure derivation, `disk_info_from_statvfs(f_blocks,
  f_bfree, f_bavail, f_frsize) -> DiskInfo` (all `u64`, saturating
  arithmetic), is `#[cfg(any(target_os = …, test))]`-gated, matching the
  platforms it's exercised on. `None` on any failure: a nul byte in `mount`,
  or the `statvfs` call itself failing (e.g. a nonexistent mount).
- `uptime.rs` — `read_uptime() -> Option<u64>` (W37), a `#[cfg(target_os)]`
  read surface following the `battery.rs`/`cpu.rs` pattern: Linux parses
  `/proc/uptime` (`parse_proc_uptime`), macOS derives it from `sysctl -n
  kern.boottime` vs now (`parse_kern_boottime`); both pure parsers are
  `#[cfg(any(target_os = …, test))]`-compiled and unit-tested on Linux.
- `media.rs` — `read_media() -> Option<MediaInfo>` (W41), a Linux shell-out to
  `playerctl metadata` behind the pure `parse_playerctl`; `None` on any failure
  (`playerctl` missing, no player, malformed output). Non-Linux → `None`.
- `throughput.rs` — `read_throughput(state_dir, interface) -> Option<Throughput>`
  (W47), a Linux read surface at the `Context`-build edge. A rate is a delta,
  so — mirroring `cpu.rs`'s persisted-snapshot pattern — it diffs the current
  `/proc/net/dev` counters against a prior `(rx, tx, ts)` sample persisted at
  a per-interface sample file (W46 — `fn sample_name(interface:
  Option<&str>) -> String`: `throughput-sample` for the aggregate read
  (`None`), `throughput-sample-<sanitized-iface>` for a named interface, any
  non-`[A-Za-z0-9_-]` character replaced so multiple simultaneous
  `throughput` instances on different interfaces read/write distinct files
  instead of clobbering one shared sample) via `sample_store` (no sleep
  across two live reads). `None` on the first invocation (nothing to diff
  against), an
  unsupported platform, or a read failure — never a fabricated `0` (invariant
  #6). The pure `parse_proc_net_dev`/`throughput_rate`/`aggregate` helpers are
  `#[cfg(any(target_os = "linux", test))]`-compiled (same as `cpu.rs`), so
  they're unit-tested on any dev box; `interface` pins the read to one named
  interface, `None` aggregates all non-loopback interfaces.
- `windows.rs` — `read_windows(session: Option<&str>) -> Vec<WindowCtx>` (W42),
  a platform-agnostic shell-out to `tmux list-windows -F
  "#{window_index}\t#{window_active}\t#{window_flags}\t#{window_name}"` (name
  last so a tab inside a window name can't misalign the earlier fields; the
  pure `parse_list_windows` does the `splitn(4, '\t')` parse, unconditionally
  unit-tested — no cfg-gating needed, same shape as `git.rs`'s tool-shell-out
  pattern). Empty `Vec` on ANY failure (`tmux` missing, bad session, non-zero
  exit) — never a fabricated window (invariant #6). Backs `render windows`
  (the batched two-line window-list render; called directly from `main.rs`,
  not routed through `build_context.rs`).
- `sample_store.rs` — shared best-effort atomic per-widget state persistence
  (W52): `read_sample`/`write_sample` read/write a small text file under a state
  dir via `write_atomic` (`rustline-core`'s `atomic_write.rs`), `warn!`ing
  (never panicking) on I/O failure. A generalization of `cpu.rs`'s
  pre-existing `cpu-sample` dance, reused by
  `throughput.rs` and the `{spark}` history reads; each caller keeps its own
  sample serialization/parsing.
- `history.rs` — pure sparkline-history ring helpers (W45): `parse_history`/
  `serialize_history` over a single space-separated line of `f32` readings
  (oldest first, total over corrupt tokens), plus push/truncate — the ring's
  own shape, split from `sample_store`'s generic file I/O the same way
  `cpu.rs`'s `parse_snapshot`/`serialize_snapshot` are. Shared by `cpu.rs`'s and
  `memory.rs`'s `{spark}` history reads.
- `sample_context.rs` — the one shared synthetic-`Context` builder (W52):
  `sample_context(show_alerts) -> Context`, a representative fully-populated
  fixture. The three previously near-identical hand-rolled fixtures
  (`theme_cmd.rs`'s preview `sample_context`, `bench/fixture.rs`'s
  `fabricated_context`, `plugin_cmd.rs`'s `plugin run` harness) now delegate
  here via struct-update, so they no longer drift as `Context` grows.
  `theme show`/`theme pick`'s rendered preview is the load-bearing consumer —
  every field its preview layout can observe keeps theme_cmd's original
  pre-consolidation value verbatim (pinned byte-identical by a characterization
  test); the extra superset fields carry synthetic data no preview widget reads.
- `daemon_proto.rs` — the optional daemon's wire protocol (W48), pure and
  self-contained (no socket/listener/client here): `DaemonRequest {
  RenderV2 { protocol: u32, region: RegionKind, args: RenderArgsWire }, Ping,
  Shutdown }` plus `pub const DAEMON_PROTOCOL: u32` — the version handshake that
  keeps a **stale daemon from silently serving a new client** (the daemon is
  long-lived and `reload_if_changed` watches only the *config* file's mtime,
  never the binary's, so after a `cargo install` the old daemon would otherwise
  keep answering with old widget code and the client would emit that markup as
  its own). It is fail-closed **by construction** in both directions, not by a
  check: the request enum is externally tagged, so an old daemon simply cannot
  deserialize `RenderV2` (and a new daemon cannot deserialize the old `Render`)
  — the frame errors, that one connection is dropped, and `try_render`'s `None`
  falls back to the in-process render. A protocol number that merely *differs*
  is refused explicitly (`daemon.rs`), and `DaemonResponse`'s client-side match
  is exhaustive with no `_` arm so a future variant is a compile error rather
  than an accidental accept. `Ping`/`Shutdown` are deliberately left unversioned
  as unit variants so `daemon status`/`daemon stop` keep working across a skew.
  Note the `V2` in the variant name is the second generation of the *variant*
  and is independent of `DAEMON_PROTOCOL`'s value — a protocol bump to 2 does
  **not** rename it to `RenderV3`. Bumping `DAEMON_PROTOCOL` whenever render
  semantics change is a hand-maintained discipline; forgetting it reproduces the
  original silent-skew bug.
  `RegionKind { Left, Right, Window }` (mirrors `cli::Render`'s three
  variants), `RenderArgsWire` (combines `cli::RegionArgs`'s
  `session`/`window`/`pane`/`pane_path` and `cli::WindowArgs`'s
  `index`/`name`/`flags`/`current` into one struct covering all three
  `RegionKind`s — fields that don't apply to the requested kind are
  absent/default; derives `Default`), and `DaemonResponse { Markup(String),
  Pong, ShuttingDown }`. `fn write_frame<W: Write>(w, &impl Serialize) ->
  io::Result<()>`/`fn read_frame<R: Read, T: DeserializeOwned>(r) ->
  io::Result<T>` frame each message as a 4-byte little-endian length prefix
  + JSON, with an 8 MiB cap checked **before** allocating (an oversize or
  truncated frame is an `io::Error`, never a panic).
- `daemon_client.rs` — the daemon client (W48): `fn daemon_socket_path() ->
  PathBuf` resolves `$XDG_RUNTIME_DIR/rustline/daemon.sock`, falling back to
  `<state_root>/daemon.sock` when `XDG_RUNTIME_DIR` is unset; `fn
  try_render(region: RegionKind, args: RenderArgsWire) -> Option<String>`
  is the safety seam every render call site goes through: one `stat` first
  (socket file absent → `None` immediately, near-zero overhead when the
  daemon isn't running), else connect with a short (250 ms)
  read/write timeout, `write_frame` the request, `read_frame` the response,
  and return the markup **only** on `DaemonResponse::Markup` — `None` on
  *any* other outcome (connect/timeout/decode error, or a `Pong`/
  `ShuttingDown` reply), so the caller always has a safe in-process render to
  fall back to (invariant N2 extended to the daemon path — "never break the
  bar"). It also now reports *why* it fell back: a missing socket stays
  silent (not installed is not a problem), but every failure past that
  `stat` — a timeout setter, the write, the read, an unexpected response —
  logs at `debug` (cheap, per-tick, `-vvv` detail), except a socket that
  *exists but refuses the connection* (a dead daemon left it behind), which is
  `warn_once`'d instead (`"daemon-stale-socket:<path>"`) since that case is
  actionable and persists across ticks rather than being a one-off, unlike a
  per-tick `debug!`. `daemon.rs`'s `reload_if_changed` gets a matching
  `info!` — one line per config edit (it only runs when the mtime actually
  changed, so no dedup is needed), the only confirmation a user gets that a
  warm daemon picked up their edit.
- `daemon.rs` — the daemon server (W48): `DaemonState { config, theme,
  plugin_dir, registry, config_mtime }` holds everything warm across
  requests — parsed `Config`, resolved `Theme`, a built `Registry` (built
  over the **union** of both regions' layouts so plugins referenced by
  either side are warm), and the config file's last-seen mtime — with a
  `reload_if_changed` that re-parses/re-resolves/rebuilds only when
  `config.toml`'s mtime has actually advanced (the common case reuses
  everything, including live WASM plugin instances). `pub fn serve(config_path:
  &Path, plugin_dir: PathBuf) -> io::Result<()>` binds a `UnixListener` at
  `daemon_client::daemon_socket_path()` (removing a stale socket file first),
  then accepts connections on their own thread against a shared
  `Arc<Mutex<DaemonState>>`; a `Render` request builds the *same* `Context` via
  `build_region_context`/`build_window_context` and renders via the *same*
  `render_named_region`/`render_window` the in-process path uses, so daemon
  output is byte-identical; `Ping` → `Pong`; `Shutdown` → reply
  `ShuttingDown`, remove the socket, and exit. Renders serialize behind the
  state `Mutex` (fine — renders are fast, and it also satisfies
  `extism::Plugin`'s `!Sync` constraint: only one thread ever renders through
  a given plugin instance at a time), and the mutex is never held across a
  blocking socket read/write, so one slow/hung client can't stall the others
  or deadlock a reload. `pub fn status() -> bool` (connect + `Ping`, used by
  both `rustline daemon status` and `doctor`'s advisory check) and `pub fn
  stop() -> io::Result<()>` (connect + `Shutdown`) round out the client-side
  control operations.
- `daemon_service.rs` — `rustline daemon install|uninstall`: makes the daemon
  survive logins using each platform's native per-user service mechanism, with
  the OS backend chosen at **compile time**. The public `install(binary,
  write_only)`/`uninstall()` are platform-agnostic dispatchers — `main.rs`
  names neither the path nor the service manager; an unsupported platform
  prints a notice and does nothing. **Linux → systemd user unit** at
  `$XDG_CONFIG_HOME/systemd/user/rustline-daemon.service` (fallback
  `~/.config/systemd/user/`, `service_unit_path()`); `service_unit(binary)`
  (pure) renders `ExecStart=<binary> daemon run` + `Restart=on-failure`, wired
  via the `Systemctl` trait (`available`/`daemon_reload`/`enable_now`/
  `disable_now`), `RealSystemctl` the only production impl. **macOS → launchd
  LaunchAgent** at `~/Library/LaunchAgents/rustline-daemon.plist`
  (`plist_path()`); `plist_contents(binary)` (pure) renders the plist with
  `Label=rustline-daemon`, `ProgramArguments=[<binary>, daemon, run]`,
  `RunAtLoad=true`, and `KeepAlive={SuccessfulExit=false}` (restart on failure
  only — the launchd analogue of `Restart=on-failure`; a clean `daemon stop`
  stays stopped), wired via the `Launchctl` trait (`available`/`bootstrap`/
  `bootout`), `RealLaunchctl` the only production impl (`launchctl
  bootstrap|bootout gui/$UID <plist>`, `gui_domain()` via `libc::getuid`, the
  sole macOS `unsafe`). Both backends share `command_on_path` and a
  stdio-silenced `run_silent`; both traits are the same recording-fake seam
  `click.rs`'s `ClickExecutor` uses, so the pure builders + `install_*`/
  `uninstall_*` flows are unit-tested (via `FakeSystemctl`/`FakeLaunchctl` on a
  tempdir path) on both the Linux CI box and macOS — the pure fns/inner flows
  are `#[cfg(any(target_os = …, test))]`-compiled, the `Real*` impls
  `#[cfg(target_os = …)]`. `install` writes the unit/plist (overwriting/
  reporting a replace), then either prints the manual load command
  (`--write-only`, or the manager absent from `PATH`) or reloads/enables it
  (macOS `bootout`s any stale instance first so reinstall is idempotent);
  `uninstall` best-effort unloads and removes the file, safe to re-run. Every
  real invocation is best-effort (spawn failure/non-zero exit → `false`, never
  a panic) and neither operation treats a manager failure as fatal — the
  unit/plist file is the durable part.
- `click.rs` — click resolution + dispatch (W36): `resolve_click(&Config,
  range, button) -> ClickAction` (`{Toggle, OpenUrl, Run, NoOp}`, pure over the
  config) and `dispatch(action, range, &impl ClickExecutor)`. The
  `ClickExecutor` seam (`RealExecutor` spawns detached `sh -c`/`xdg-open`; a
  recording fake in tests) keeps resolve+dispatch unit-tested without spawning.
  Default (no binding) is byte-identical to the pre-W36 toggle behavior; the
  only text `sh -c` ever sees comes from the user's own config, never the tmux
  `range` value (invariant #4).
- `plugin_install.rs` — `plugin install/update/remove` (W38): a `Downloader`
  seam (`UreqDownloader`, rustls, redirect-limited, User-Agent) over the GitHub
  releases API, pure `parse_owner_repo`/`select_wasm_asset`. `sha256_hex`
  itself now lives in `rustline-wasm::integrity` (W19); this module re-exports
  it (`pub use rustline_wasm::sha256_hex;`) so existing `plugin_install::
  sha256_hex` call sites keep resolving, rather than keeping its own
  duplicate copy as before. Install downloads a repo's `.wasm` into the plugin
  dir and records `source`/`tag`/`checksum` — granting **no** capabilities
  (TOFU at write time: it records the hash of what it downloaded, doesn't
  verify against an external pin; that recorded digest IS verified later, at
  load time, by `register_plugins` — see `integrity.rs`/Config below).
- `plugin_index.rs` — the curated plugin index (W49) backing `rustline
  plugin search`: the wire types (`PluginIndex { schema_version, plugins:
  Vec<IndexEntry> }`, `IndexEntry { name, description, source, bundled,
  capabilities }` — `capabilities` is advertising copy only, consulted by
  nothing in the host and granting nothing), `parse_index_value`/`validate`
  (rejects a `schema_version` this build doesn't understand rather than
  misreading it; checked identically on both the fresh-fetch path and the
  cache-read path, so a cache written by a different-schema build can never
  be served unvalidated), `filter_entries` (case-insensitive substring match
  over name + description; no/blank query returns every entry),
  `index_is_fresh` (24h TTL via `INDEX_TTL_SECS`; a clock that moved
  backwards counts as stale, not fresh forever), `read_cache`/`write_cache`
  (best-effort, atomic, via `sample_store::{read_sample, write_sample}`),
  `load_index` (a fresh cache is served as-is; otherwise fetch, cache, and
  return; a failed fetch falls back to the last-known-good cache flagged
  `stale`, erroring only when nothing is cached at all), and `search_json`/
  `LoadedIndex`. `DEFAULT_INDEX_URL` points at `registry/index.json` on this
  repo's `main` branch (overridable via `Config.plugin_index_url`);
  `INDEX_CACHE_FILE` is `plugin-index.json` under the state root (see the
  host-owned state-file names list under Config below).
- `build_context.rs` — builds `Context` from args + `gethostname`,
  `libc::getloadavg` (the only `unsafe` in this file — `disk.rs`'s `statvfs`
  call is its own `unsafe`, isolated there — guarded on `n == 3`),
  `chrono::Local`,
  `$HOME`, non-loopback IPv4 interfaces via `if-addrs` into
  `Context.interfaces` (a failed read yields an empty `Vec`, never a
  fabricated address — same spirit as `read_loadavg` returning `None`), and
  now also `battery` (via `battery::read_battery()`), `cpu` (via
  `cpu::read_cpu()`), `memory` (via `memory::read_memory()`), `git` (via
  `git::read_git(&pane_current_path)`, gated: only read when `git` is in the
  region's layout, mirroring the `cpu`/`memory` gate), `throughput` (via
  `throughput::read_throughput`, gated the same way on `throughput` being in
  the layout, W47), the `{spark}`-gated `cpu_history`/`mem_history` (only read —
  via `cpu::read_cpu_history`/`memory::read_memory_history` — when the base
  `cpu`/`memory` widget OR any `cpu`/`memory`-kind `[instances.<name>]` in the
  layout references `{spark}` in its `format`/`alt_format`
  (`Config::spark_referenced_in_layout`, W57 — supersedes W56's base-only
  check); a `spark` struct threaded alongside the layout carries each widget's
  `format` + `spark_width`, W45), `os`, `arch`
  (from `std::env::consts::OS`/`ARCH`), and `toggled` (via
  `toggles::read_toggles()`, unconditionally — cheap relative to the gated
  cpu/memory/git/disk reads). `pub fn build_region_context(args: &RegionArgs,
  layout: &[WidgetName], theme: &Theme, cfg: &Config) -> Context` (W46 — the
  signature dropped the old separate `disk_mount` parameter; `cfg` carries
  everything needed) is now **kind-aware**: it resolves `cfg.layout_kinds(layout)`
  once and gates every read (`git`/`battery`/`cpu`/`memory`/`uptime`/`media`/
  interfaces/etc.) off that kind set instead of the raw layout names, so an
  `[instances.<name>]` widget of a given kind fires that kind's read even when
  the *base* name (e.g. plain `"disk"`) isn't itself in the layout. `disk` and
  `throughput` fan out over every **distinct** mount/interface the layout
  references — `cfg.disk_mounts(layout)`/`cfg.throughput_interfaces(layout)` —
  calling `read_disk`/`read_throughput` once per distinct value and collecting
  the results into `ctx.disks`/`ctx.throughputs`; `ctx.disk`/`ctx.throughput`
  are then set to just the *base* widget's mount/interface entry (for the WASM
  mirror). A private `layout_needs(layout, name) -> bool` is the one predicate
  behind every one of these gates, now also covering
  `battery` and the IP-widgets' interface scan (W5; both used to be read
  unconditionally). `build_window_context` builds the minimal `Context`
  `render window` needs — just `Context.window`, via `..Context::default()`
  — skipping every other read entirely (`getloadavg`, the toggles-file read,
  `gethostname`, `$HOME`, cpu/memory/git/disk/battery/interfaces), since tmux
  calls it once per window per refresh and the window-pill render path never
  touches anything else (W8): it no longer routes through
  `build_region_context` at all.
- `toggles.rs` — the global click-toggle state file:
  `toggles_path()` (`$XDG_DATA_HOME/rustline/toggles`, reusing
  `rustline_wasm::data_root()`), `parse_toggles`/`serialize_toggles`
  (newline-delimited, total over blanks/whitespace), `apply_toggle` (flips
  membership), `read_toggles` (missing/unreadable file → empty set), and
  `write_toggles` (best-effort atomic write via `write_atomic`; a write
  failure `warn!`s and never panics — a broken toggle must never break the
  bar).
- `plugin_cmd.rs` — `rustline plugin …`: `list` reads the effective `Config`;
  `list`/`url|path|write-path|cmd list`/`denials` each take a `--json` flag
  (W40, `plugin_list_json`/`pattern_list_json`/`denials_json`) emitting a
  `serde_json`-pretty array instead of the human text, human output
  unchanged when the flag is absent — `list --json`'s per-plugin object now
  also carries `allowed_write_paths` and `resolve_symlinks` (D3, so a plugin
  holding a write grant can't render as read-only in machine-readable
  output), alongside the existing `allowed_urls`/`allowed_paths`/
  `allowed_commands`; the human `list` output prints `resolve_symlinks` too,
  so the doc comment's "same fields the human list prints" claim stays true.
  `url|path|write-path|cmd add/remove` all share one `pattern_cmd` dispatch
  keyed by a private `Kind { Url, Path, WritePath, Command }` (`Kind::key()`
  -> `"allowed_urls"`/`"allowed_paths"`/`"allowed_write_paths"`/
  `"allowed_commands"`) that mutates the config file in place via `toml_edit`
  (preserving comments/formatting), creating `[plugins.<name>]` if absent;
  `new <name> [--path] [--force]` scaffolds a ready-to-build WASM guest
  plugin crate from embedded templates (`assets/plugin-cargo.toml.tmpl`/
  `plugin-lib.rs.tmpl`, mirroring `init.rs`'s `include_str!` approach):
  `validate_plugin_name` enforces the same `[A-Za-z0-9_-]`/≤15-byte/not-
  `window` rules as a widget's click-toggle range name (invariant #7), and
  the generated `src/lib.rs` deserializes the typed `GuestRender`/
  `WireContext` (not a hand-walked `serde_json::Value`). Refuses to
  overwrite an existing `<name>/` dir without `--force`; prints the
  `cargo build --target wasm32-unknown-unknown` + install step and a
  starter `[plugins.<name>]` config snippet afterward. `approve <name>
  [--yes]` (W24) resolves the plugin's manifest via `resolve_manifest`,
  prints what it requests via `manifest_report` — labeling the read line
  `allowed_paths (read-only)` and, when `requested_write_paths` is
  non-empty, appending its own overwrite-danger banner ("lets this plugin
  overwrite these files with any content"), mirroring the existing warning
  that fires when `requested_commands` is non-empty (that
  `allowed_commands` runs real programs with the user's own environment and
  permissions) — approve only patterns you understand — and, after an
  interactive y/N confirmation (or unconditionally with `--yes`), writes
  **exactly** those requested URL/read-path/write-path/command patterns into
  `[plugins.<name>]`'s allowlists (idempotent append, never a wider grant);
  the "nothing to approve" guard checks all four requested lists, so a
  manifest requesting only `requested_write_paths` is still approvable.
  Both warnings are part of `manifest_report`'s printed text, not the
  confirmation prompt, so they still show up under `--yes` and in captured
  output/logs, not just an interactive session. `list` also now shows a `run
  \`plugin approve <name>\`` hint when a manifest resolves for that plugin.
  Also handles `build` (W31, any
  cdylib crate → `.wasm` in the plugin dir; pure `wasm_artifact_path`/
  `cargo_build_args`/`package_name`), `run` (W34, the read-only dev harness via
  `rustline_wasm::instantiate_named` + `format_run_output`, printing segments
  and captured denials), `denials` (W28, read-only over
  `rustline_wasm::denials::read_denials`), `search` (W49, thin CLI wiring
  only: resolves `cfg.plugin_index_url`/the state root, calls
  `plugin_index::load_index`, filters via `plugin_index::filter_entries`, and
  prints human text or `plugin_index::search_json`; the fetch/parse/cache
  logic itself lives in `plugin_index.rs` above), and delegates
  `install`/`update`/`remove` to `plugin_install.rs`.
- `widget_cmd.rs` — `rustline widget …`: `read_layout`/`write_layout` are a
  `toml_edit` reader/writer pair over `[layout]` (handling both a standard
  `[layout]` table and an inline `layout = { ... }` form alike, and falling
  back per-region to `Config::load`'s defaults when a region is absent) —
  the same in-place-edit approach `plugin_cmd.rs`/`theme_cmd.rs` use.
  Every mutation goes through `rustline-core`'s pure `layout_enable`/
  `layout_disable`/`layout_move` (see `config.rs` above) via a shared
  `mutate` helper: load config → validate the name against `known_names`
  (built-ins + non-colliding instances + discovered plugin stems, via
  `widget_placements`) → apply the edit → write + `println!` a human summary
  (`describe`) → best-effort `tmux refresh-client -S` when inside tmux. A
  refused edit (unknown name, unknown region, or a `LayoutEditError`) prints
  to stderr and exits `1` **without writing anything**. `--region center` is
  the one case that's legal but inert here (see the module doc and invariant
  discussion in `config.rs`/CENTER-region above): `warn_if_center` prints a
  note rather than refusing, since the CLI path must still be able to
  round-trip the default `center = ["windows"]`. `list [--json]` renders
  `widget_placements` as `<region>[<index>] name summary (source)` lines, or
  `placements_json` — `{name, summary, source, region, index}` per row,
  `source` one of `"builtin"`/`"plugin"`/`"instance of <kind>"`
  (`"instance:<kind>"` in JSON). `Edit` dispatches to `widget_tui::run`.
- `widget_tui.rs` — the interactive `rustline widget edit` full-screen editor
  (`ratatui` + `crossterm`), built the same way `theme_cmd.rs`'s `run_picker`
  is: a pure state machine (`EditorState::on_key(KeyKind) -> EditorAction`,
  unit-tested with no terminal involved) plus a thin terminal-owning shell
  (`run`, which needs a TTY — mirroring `theme pick`'s guard — and errors with
  a hint toward `widget list` on a non-interactive invocation). Four
  `Column`s (`Left`, `Center`, `Right`, `Available`) are drawn side by side;
  arrow keys or vim keys (`h`/`j`/`k`/`l`) move focus/cursor, `space`/`Enter`
  adds a selected AVAILABLE widget to RIGHT (the only region AVAILABLE
  placement ever appends into — `last_region` is unconditionally RIGHT once
  focus reaches AVAILABLE, since `Column::ALL`'s `[Left, Center, Right,
  Available]` adjacency always passes through RIGHT immediately beforehand)
  or removes a placed one back to AVAILABLE, `J`/`K` nudge a widget's
  position within its region (via `layout_nudge`), `H`/`L` (shifted,
  mirroring `J`/`K`; both the `Char('H')` and `Char('h')`+SHIFT reporting
  forms) move a *placed* widget directly to the other editable region (via
  `layout_move` + the free `adjacent_editable_region` helper, which skips
  over CENTER rather than stopping there — the only way LEFT is reachable
  from RIGHT at all) — CENTER is still refused as a source (focus a widget
  in CENTER and press `H`/`L`, same as `space`/`J`/`K`, and it's refused
  with `CENTER_FIXED_STATUS`), `w` writes the in-memory `Layout` to
  `config.toml` (same `read_layout`/`write_layout` as `widget_cmd.rs`), `q`
  quits (prompting to confirm first if there are unsaved changes — `dirty`
  tracking — else quitting immediately), and `?` toggles a help overlay. A
  live preview strip along the bottom re-renders the regions (via
  `render_named_region` + `ansi.rs`'s `parse_markup`, mapped onto ratatui
  `Style`s by `to_ui_style`/`to_ui_color`) against the **in-memory,
  possibly-unsaved** layout, so a move/add/remove is visible immediately,
  before `w`. It deliberately does NOT use `tmux_to_ansi`: ratatui writes
  cells and never interprets ANSI escapes, so escape bytes handed to it are
  drawn as literal text with no colour at all. The strip is one line when the
  regions fit side by side and otherwise stacks one region per line —
  `preview_layout(widths, available)` decides, measuring **display width**
  (`unicode-width`, since the bar is full of multi-byte and double-width
  glyphs) plus a 3-column gap per adjacent pair; stacked rows keep each
  region's real alignment (LEFT left, CENTER centred, RIGHT right) so a
  narrow terminal still conveys where each region sits. `Column::
  Center` stays focusable/visible (so a user can see the window list lives
  there) but every edit targeting it is refused outright with an
  explanatory status line (`CENTER_FIXED_STATUS`) — unlike `widget_cmd.rs`'s
  CLI path, which still writes a center edit with a warning (round-tripping
  the default config matters there; a "wrote but nothing happened" surprise
  is cheaper to prevent up front in an interactive editor). `TerminalGuard`
  (a `Drop` impl) restores raw mode and leaves the alternate screen on every
  exit path, including a panic unwinding through the draw loop (this
  workspace's default `panic = "unwind"`, so `Drop` still runs).
- `theme_cmd.rs` — `rustline theme …`, mirroring `plugin_cmd.rs`'s `toml_edit`
  approach: `list` prints every built-in and themes-dir `*.toml` stem,
  marking the active one (`cfg.theme.base`, default `"default"`) with `*` and
  a built-in **shadowed** by a same-named file; `list --json` (W40) emits
  `theme_list_json` instead — one `{name, active, source, shadowed}` entry per
  theme (`source` `"builtin"`/`"file"`), built-ins first then themes-dir
  files, human output unchanged when the flag is absent; `show <name>` resolves
  (file-first, then built-in), builds a synthetic `Context` engineered to
  trip warning+error badges, renders the default layout, and prints ANSI via
  `tmux_to_ansi`; `use <name>` validates `<name>` resolves, then sets
  `[theme].base = "<name>"` in the config file via `toml_edit` (refuses to
  write on an unknown name or unparseable existing config). Both `use` and
  `show`'s unknown-name error lists every resolvable theme via the shared
  `available_themes_line(themes_dir)` (W18) — built-ins AND themes-dir custom
  stems, not just built-ins (the pre-W18 hint). `new <name>
  [--from <seed>] [--force]` resolves `<seed>` (default `"default"`) to a
  full `Theme`, converts it via `ThemeConfig::from_theme` to an all-`Some`
  config, and writes `<themes_dir>/<name>.toml` with a header comment
  (refuses to overwrite without `--force`; rejects a `name` containing `/`,
  `\`, `..`, or empty); `pick` is the interactive browse-and-set command,
  layered on top of the same helpers: `picker_entries` builds the ordered,
  name-deduped `PickEntry { name, active, from_file }` list (built-ins first,
  then themes-dir-only stems), `parse_preview_input`/`parse_set_input` are
  pure parsers for the two prompt loops (the preview loop also accepts
  `t`/`toggle` → `PreviewCmd::ToggleAlerts`), and `run_picker` (reader/writer-
  generic over `BufRead`/`Write`, so it's unit-tested with byte-slice
  reader/writer — no real TTY needed) drives preview-then-set and returns the
  chosen name or `None` to keep the current theme. Its previews default to a
  **healthy** synthetic bar (palette only, no alert badges — what a normal
  status line looks like); `t` toggles the warning/error alert colors on to
  sample a theme's semantic colors, and immediately **re-renders the last
  previewed item** (tracked as `LastPreview::{One(idx),All}`) via the shared
  `render_one`/`render_all`/`replay_preview` helpers, so the toggle's effect is
  visible without re-typing a number (a toggle before any preview just prints
  the status line). The healthy-vs-pegged synthetic readings
  are chosen by a `show_alerts` bool threaded through
  `sample_context`/`preview_theme_ansi`/`preview_named` (`theme show` and the
  `init` wizard's one-shot preview pass `true`, keeping their alert-badge
  demo). `pick` itself requires a
  TTY (`stdin().is_terminal()`; a non-interactive invocation prints a hint
  toward `theme show`/`theme use` and exits non-zero, writing nothing) and,
  on a choice, reuses `use_theme` for the actual config write.
- `tmux_conf.rs` — `init_block(&InitBlockOpts)` (`bar_bg`, `fg`, `two_line`,
  `mouse`, `interval`, `binary`): the tmux config block `rustline init` emits,
  incl. `bind -T root MouseDown{1,2,3}Status` blocks — left (window-select
  default preserved, else `click --button=left`), plus middle/right dispatching
  `click --button=middle`/`--button=right` for W36 config bindings (see CLI
  below); one-line/mouse-off/interval-1 output stays otherwise unchanged. `two_line`
  additionally emits `set -g status 2` plus the author's verbatim two-line
  `status-format[0]`/`[1]` (window list on its own line). `TMUX_BEGIN`/
  `TMUX_END` (`# >>> rustline >>>` / `# <<< rustline <<<`) and
  `upsert_tmux_block(existing, block) -> String`: idempotently insert/replace
  the rustline-managed region in an existing `~/.tmux.conf`, leaving
  surrounding user content untouched. `BlockAnswers { two_line, mouse,
  interval: Option<u32> }` + `parse_block_answers(existing) ->
  Option<BlockAnswers>` are the **inverse of `init_block`** for the three
  wizard answers config never stores, so `init::seed_answers` can show them
  back as a re-run's defaults; `None` when no complete managed region is
  present, and `interval` is `Option` so the *caller* owns the fallback rather
  than this module knowing the wizard's defaults. Matching is deliberately
  **whole-line** (trimmed): the block emits the discoverability comment
  `(needs: set -g mouse on)` unconditionally, so a substring search would
  report `mouse: true` even for a mouse-off block. Its round-trip test drives
  `init_block` rather than a hand-written block, so changing an emitted
  setter line without teaching the parser fails loudly instead of silently
  resetting a user's choice.
- `init.rs` — the `rustline init` wizard shell: `InitAnswers`/`ClockStyle`
  (the collected answers + four datetime presets), `starter_config_toml(&
  InitAnswers) -> String` (mutates the embedded starter template — theme,
  layout arrays, datetime format, pruning unselected optional widgets'
  `[widgets.*]` sections), `merge_config(existing, generated, theme) ->
  Result<String, String>` (non-destructive merge: `[theme].base` always set,
  `[layout]`/each `[widgets.<name>]` added only if absent), `write_config`
  (backs up an existing file to `<path>.rustline.bak` first), `defaults()`
  (the `--defaults`/recommended answer set), `parse_menu_choice`/
  `parse_yes_no` (pure prompt-parsing, unit-tested), and `run`/`prompt_answers`
  (the I/O shell: `--print` wins and emits the legacy one-line block via the
  caller's already-resolved theme; else `--defaults` or the interactive
  prompt loop, erroring on non-TTY stdin without a flag). Also (W33):
  `fn seed_answers(config_path: &Path, tmux_conf_path: &Path) -> InitAnswers`
  pre-fills a re-run's answers from **both** places a previous run wrote them.
  From `config.toml`: `theme` from `cfg.theme.base`,
  `battery`/`lan_ip`/`tailscale` from whether that name appears anywhere in
  `cfg.layout.{left,center,right}`, and `clock` reverse-mapped from
  `cfg.widgets.datetime.format` via the pure `fn clock_from_format(fmt: &str)
  -> Option<ClockStyle>` (exact match against the four presets, `None` on no
  match). The three tmux-only answers (`two_line`/`mouse`/`interval`) are
  tmux settings that config never stores, so they're recovered by
  reverse-parsing `~/.tmux.conf`'s managed block via `tmux_conf::
  parse_block_answers` (see `tmux_conf.rs` below) and overlaid on top. That
  overlay is load-bearing for `two_line` specifically: its recommended
  default is one-line, so the original config-only seed meant an
  Enter-through re-run silently rewrote a two-line status bar back to one
  line (`mouse`/`interval` merely *happened* to match their defaults for a
  user who took the recommendations). Each source degrades independently — a
  missing config file keeps `defaults()` (with `battery` still
  hardware-probed via `battery::read_battery()`, preserving the pre-W33
  fresh-run default), and a missing or blockless tmux conf keeps the
  recommended tmux answers.
  `prompt_answers` gains a `seed: &InitAnswers` parameter and shows each
  seeded value as that question's default. `fn summarize_answers(a:
  &InitAnswers) -> String` renders one line per collected answer for the new
  confirm step: after gathering answers (interactive runs only —
  `--defaults`/`--dry-run`/`--print`/`--uninstall`/non-TTY are unchanged),
  `run` prints the summary plus the existing dry-run line-diff machinery
  (`dry_run_config`/`dry_run_tmux_block`/`line_diff`) against both files'
  current contents, asks `ask("Write these changes?", true)`, and only calls
  `apply` on yes — a no prints "Aborted; nothing written." and writes
  nothing. `assets/
  starter-config.toml` (embedded via `include_str!`) is the recommended
  starter template `init.rs` mutates — the shortened `alt_format`s and
  curated layout it seeds live only here, **not** in any widget's code
  `Default` (those stay `""`/unchanged; see Config below).
- `logging.rs` — `init(&LogConfig, verbose)`: installs the two-sink `tracing`
  subscriber (a daily-rotated file, via `tracing_appender`'s
  `RollingFileAppender`, plus stderr), and the pure helpers
  `verbosity_to_level`, `parse_level`, `resolve_file_level`/
  `resolve_stderr_level`, `log_dir`/`log_file_prefix`/`log_file_suffix`
  (decompose `[log].file` into the directory + filename prefix/suffix the
  appender needs to name each day's generation
  `<prefix>.<YYYY-MM-DD>.<suffix>`), `current_log_path` (today's file, in
  UTC — the appender rotates on a UTC day boundary, so this must not use
  local time, or `doctor`'s reported path would disagree with what the
  appender actually writes for anyone off UTC+0), and `build_appender` (the
  one builder chain both production `init` and the test suite call, so they
  can't drift). `open_log`/`log_path`/`should_rotate` from the old
  size-capped, single-generation rotator no longer exist. Best-effort: a log
  dir that can't be created or opened degrades to stderr-only; never writes
  stdout.
- `warn_once.rs` — the real cross-process dedup layer behind
  `rustline-core`'s `diag::warn_once` seam (above). `install(config_path)`,
  called once from `main` immediately after `logging::init`, installs a hook
  that claims a marker file per distinct `warn_once` key under
  `<data_root>/.warn-markers/` (a sibling of `state/`, deliberately not a
  descendant of it — see the module doc for the collision an unvalidated
  plugin stem like `.warn-markers` could otherwise cause) via
  `OpenOptions::create_new(true)`, atomic on every filesystem this project
  supports, so concurrent renderers racing the same key still produce exactly
  one winner with no lock. The whole marker directory is cleared whenever the
  config file's mtime changes (`reset_if_generation_changed`), so each
  misconfiguration is logged once per config edit rather than once per render
  tick. **Fails open, all the way down**: a marker I/O failure is treated as
  "emit", and a marker dir that exists but is wedged (present, non-empty,
  unwritable) makes `install` skip installing the hook at all for that run —
  degrading to no dedup rather than to permanent silence.
- `main.rs` — dispatch + the `emit(markup, preview)` helper (raw markup vs
  ANSI) + `resolve_plugin_dir` (`--plugin-dir` flag › config `plugin_dir` ›
  `rustline_wasm::default_plugin_dir()`). Only `render left`/`render right`
  discover and register plugins; `render window` is built-ins only.
  `run_click` handles `Command::Click` by delegating to
  `click::resolve_click(&cfg, range, button)` + `click::dispatch` with the
  production `RealExecutor` (W36) — the single choke point for click dispatch;
  the default (no configured binding) still flips `range`'s toggle-set
  membership exactly as before. A global `--config <path>` (W35) is resolved
  once into `effective_config_path` and threaded into every subcommand that
  reads/writes the config.
  `themes_dir()` resolves `$XDG_CONFIG_HOME/rustline/themes` (fallback
  `~/.config/rustline/themes`), parallel to `config_path()`; `pub(crate) fn
  resolve_theme(&Config)
  -> Theme` is the file-aware layering used by `render`/`init`/**`daemon.rs`**
  (W48 — `resolve_theme` stayed `pub(crate)` for `init.rs` but now also backs
  `DaemonState::build`/`reload_if_changed`, since a warm daemon must resolve
  the theme the same way a cold render does) (`Theme::default()`
  → `resolve_base_theme` → inline `[theme]` overrides via `to_theme_over`), and
  `resolve_base_theme(name) -> Option<Theme>` resolves a base name
  themes-dir-file first, then built-in (an
  invalid/missing file falls through to the built-in lookup with a `warn!`) —
  this is where a user's themes-dir file **shadows** a same-named built-in
  (see Themes below). `tmux_conf_path()` resolves the user's tmux config
  (`$HOME/.tmux.conf`), parallel to `config_path()`/`themes_dir()`; `Command::
  Init` dispatches straight to `init::run` with all four resolved paths plus
  the already-resolved `theme`. The three `render`
  arms (`left`/`right`/`window`, W48) now each try `daemon_client::try_render`
  first — build the same `RenderArgsWire`, call `try_render(RegionKind::…,
  wire)` — and only fall through to the existing in-process path (unchanged,
  byte-identical) on `None`; `--preview` is applied client-side by `emit` in
  both cases, since the daemon always returns raw markup. `Command::Daemon`
  dispatches `DaemonCmd::Run` to `daemon::serve` (foreground; prints an error
  and doesn't panic on failure), `DaemonCmd::Status` to `daemon::status()`
  (prints running/not-running, exit code reflects it), and
  `DaemonCmd::Stop` to `daemon::stop()`.
- `bench/` (`#[cfg(feature = "bench")]`) — the `rustline bench` subcommand:
  `harness.rs` (`summarize`/`measure`/`Stats`/`Row`/`Group`), `fixture.rs`
  (`fabricated_context` — the pure-pass mock seam, now a thin wrapper over the
  shared `sample_context`, W52), `render_passes.rs` (pure
  widget + pure/real region passes), `sources.rs` (per-read timing + the
  source registry), `plugins.rs` (real-preserved-state plugin timing, plus
  `bench_plugin_builds` (W43): a per-plugin `build_plugin` A/B — compile cache
  OFF (`with_cache_disabled`, a full Cranelift compile every build) vs ON (warm
  deserialize) — the cold-start compile cost the preserved-state pass doesn't
  isolate; measured ~13× faster (~48→~3.7 ms, ~45 ms/plugin saved)), `daemon.rs`
  (`bench_daemon`, gated on `crate::daemon::status_at`): a daemon-vs-in-process
  A/B for `left`/`right`, quantifying the win a warm persistent daemon (W48)
  gives by keeping WASM plugins instantiated — the in-process side rebuilds the
  registry and re-runs `rustline_wasm::register_plugins` fresh every sample
  (unlike `render_passes.rs`'s real pass, which never registers plugins at
  all), while the daemon side is a real round-trip via
  `daemon_client::try_render_at`; skips with an informational note (never
  spawns a daemon) when none is reachable at the resolved socket, and reports
  a per-region speedup line in the `Group`'s `note` (median-based). And
  `report.rs` (comfy-table pretty/markdown). Gated behind the `bench` cargo
  feature; the default binary is unchanged.

## CLI

A global `-v`/`--verbose` (repeatable) raises the **file** log level:
`-v`=warn, `-vv`=info, `-vvv`=debug, `-vvvv`=trace. Works in any position
(`rustline -vv render left`). A global `--config <path>` (W35) overrides the
config-file path for every subcommand that reads or writes it (default:
`$XDG_CONFIG_HOME/rustline/config.toml`, falling back to
`~/.config/rustline/config.toml`).

- `rustline render left|right [--session= --window= --pane= --pane-path=] [--preview] [--plugin-dir=]`
- `rustline render window [--current] --index= [--name=] [--flags=] [--preview]`
  (no `--plugin-dir` — windows don't run plugins in v1)
- `rustline render windows [--session=<s>] [--preview]` (W42) — renders a
  whole session's window list in one call (batched; backs the two-line
  `status-format[0]`); shells out to `tmux list-windows` client-side; runs
  **in-process, NOT daemon-routed** (a systemd/launchd daemon has no `$TMUX`
  to run `tmux list-windows` against). One-line mode is unchanged — it still
  renders per-window via `render window`.
- `rustline widget list [--json]` — every widget (built-ins, `[instances.*]`,
  and discovered plugin stems) and where, if anywhere, it currently sits in
  `[layout]`; `--json` emits `{name, summary, source, region, index}` per row
  (`source` `"builtin"`/`"plugin"`/`"instance:<kind>"`; `region`/`index` are
  `null` for an unplaced widget) instead of human text.
- `rustline widget enable <name> [--region left|center|right] [--index N]` —
  add a widget to a layout region (default `right`, appended unless
  `--index` is given); refuses (writing nothing, exit `1`) if `<name>` is
  unknown or already placed somewhere.
- `rustline widget disable <name>` — remove a widget from whichever region
  holds it; refuses if it isn't placed anywhere.
- `rustline widget move <name> --region <r> [--index N]` — relocate a placed
  widget to another region and/or position.
- `rustline widget edit` — open the interactive full-screen `ratatui` editor
  (needs a TTY): four columns (LEFT/CENTER/RIGHT/AVAILABLE), arrow or vim
  keys to move focus, `space`/`Enter` to add/remove, `J`/`K` to reorder,
  `w` to write, `q` to quit (confirms first if there are unsaved changes),
  `?` for a help overlay, plus a live colour preview strip of the layout so
  an edit's effect is visible before you save — one line when the regions fit
  side by side, otherwise one region per line. See `widget_tui.rs` above.
  `enable`/`move --region center` still write and exit `0` (with a stderr
  note) since nothing renders `[layout].center` — see the Config section's
  CENTER-region note below; `widget edit`'s TUI refuses a CENTER edit
  outright instead, with an on-screen explanation.
- `rustline init` — interactive onboarding wizard (needs a TTY): asks theme
  (with preview), one-/two-line status, mouse/click-to-toggle, machine-type
  widgets (laptop → `battery`, Tailscale → `tailscale_ip`, LAN → `lan_ip`),
  clock style (12h/24h ± seconds), and refresh interval, then writes
  `~/.config/rustline/config.toml` (non-destructive merge — `[theme].base` is
  always set; existing `[layout]`/`[widgets.*]` are left alone; a
  `<path>.rustline.bak` backup is written first if the file already existed)
  and upserts an idempotent `# >>> rustline >>>` / `# <<< rustline <<<` block
  into `~/.tmux.conf` (also backed up first, to `~/.tmux.conf.rustline.bak`).
  A non-TTY invocation without a flag errors with a hint instead of writing
  silently. **Re-running the wizard (W33) seeds every question's shown default
  from your previous choices** instead of resetting to the recommended
  answers, reading each answer back from wherever it was actually recorded:
  your existing `config.toml` for theme/optional-widget membership/clock, and
  the managed block in `~/.tmux.conf` for the three tmux settings config never
  stores (one-/two-line, mouse, refresh interval) — see `init.rs`'s
  `seed_answers` above — and, before writing anything, prints
  a one-line-per-answer summary plus a line diff of both files against their
  current contents and asks "Write these changes?"; answering no aborts with
  nothing written. `rustline init --defaults` runs the same two writes
  non-interactively with recommended answers. `rustline init --print` is the
  legacy behavior: prints just the raw one-line tmux block to stdout (using
  `theme.bar_bg`/`fg` for `status-style`) and writes nothing. `rustline init
  --dry-run` previews, without touching disk, what a real run would write —
  the config.toml and tmux block, each with a line diff against any existing
  file — using answers gathered the same way (`--defaults` or the
  interactive wizard); `--print` still wins over it. `rustline init
  --uninstall` strips the managed tmux block from `~/.tmux.conf` (backing it
  up first) and prints the reload command, never touching `config.toml` and
  needing no TTY; combined with `--dry-run` it only *previews* the removal
  and writes nothing at all (no file, no backup). `rustline init --binary
  <path>` overrides the binary path baked into the tmux block's `#(...)`
  calls (default: the running binary's own resolved absolute path via
  `current_exe()`) — see the tmux integration model below for why the block
  calls an absolute path rather than bare `rustline`.
- `rustline print-config` — effective config as TOML.
- `rustline config path` — print the resolved config file path.
- `rustline config edit` — open the config file in `$EDITOR` (needs a TTY);
  creates it from the starter template first if it doesn't exist yet.
- `rustline config validate` — strictly parse the config file and report any
  error with its location (unlike the total `Config::load`, which silently
  falls back to defaults); a missing file is not an error.
- `rustline doctor` — diagnose the documented prerequisites (tmux ≥ 3.1,
  `set -g mouse on` when checkable from inside a running session, a
  truecolor terminal, `rustline` on `$PATH`, and the managed tmux-conf block)
  as pass/warn/fail, plus the resolved config/themes/plugin/log paths; only
  reads and prints, never writes; exits non-zero only if any check outright
  fails (a `warn` is advisory). Also (W48) an advisory-only "render daemon"
  row: `warn` (not `fail` — the daemon is opt-in) when
  `daemon_client::daemon_socket_path()` has no socket file at all ("not
  running"), `warn` when a socket exists but doesn't answer a `Ping`
  ("present but not reachable — a stale socket from a killed daemon?"), or
  `pass` when it's reachable; never affects `doctor`'s exit code. Also
  another advisory-only row: `pass` at tmux ≥ 3.2 ("display-popup available
  (prefix + W opens the widget manager)"), `warn` below 3.2 ("prefix + W
  widget manager needs tmux >= 3.2 (display-popup); the status line itself
  works") — the widget-manager popup binding needs `display-popup` (tmux
  ≥ 3.2), one version tier above the `range=user|` floor (≥ 3.1) the status
  line itself needs; this row never affects the exit code either. Also
  another advisory-only row, "plugin checksums": hashes every configured
  `[plugins.*]` entry's installed `.wasm` (via `plugin_checksum::status_for`
  — the same read-and-verify helper backing `plugin list`'s `checksum_status`
  field, below) and reports a summary — `"<N> verified, <M> unpinned"`, plus,
  only when there's something actionable, `"; checksum mismatch: <names>"` /
  `"; malformed checksum: <names>"` / `"; .wasm not found: <names>"` — or
  `"no plugins configured"` when `[plugins.*]` is empty. **Always `ok`/`warn`,
  never `fail`**: a checksum problem is real and worth surfacing, but
  `doctor`'s exit code is reserved for setup that's outright broken, and the
  checksum gate itself already degrades a bad plugin to "widget missing,"
  never to a broken bar (N2) — structurally, `run()`'s exit-code count only
  ever tallies `CheckStatus::Fail` entries, and this row is coded to never
  produce one. Finally, another advisory-only row, **"widget readers"**
  (`check_readers`): for each reader kind actually present in the configured
  layout (via `cfg.layout_kinds`, so both regions and `[instances.*]` count),
  it calls that reader and reports `warn` naming any that yield `None`. This
  is the diagnostic channel those readers otherwise lack — they each collapse
  every failure into `None`, so a missing `git`/`playerctl` binary, a
  configured `mount` that no longer exists, or a failing `tmux list-windows`
  produced a widget that silently vanished with nothing in the log. `warn`,
  never `fail`, like the rows above. Two deliberate exclusions: `throughput`
  is omitted because it legitimately returns `None` on a first invocation (a
  rate is a delta — invariant #6), which would warn on every fresh state dir;
  and `read_git` is probed against `doctor`'s own CWD rather than a pane path,
  so running `doctor` outside a repo reports git as unavailable even though
  the widget works fine in a pane that is inside one.
- `rustline completions <bash|zsh|fish>` — print a shell-completion script
  (via `clap_complete`) to stdout.
- `rustline plugin list [--json]` — discovered/configured plugins with their
  source, allowlists, state quota, and checksum status; `--json` emits a JSON
  array of `{name, source, tag, allowed_urls, allowed_paths,
  allowed_write_paths, resolve_symlinks, allowed_commands, max_state_bytes,
  has_manifest, checksum_status}` instead of human text (the human path
  prints the same values, incl. `resolve_symlinks`, as `checksum: `/other
  lines per plugin). `allowed_paths` is **read-only**; `allowed_write_paths`
  (globs, empty = deny) is the separate write allowlist — see the Config
  section's filesystem-capability paragraph above. `checksum_status` is one
  of `"verified"`/`"unpinned"`/
  `"mismatch"`/`"malformed"`/`"missing"` (`plugin_checksum::
  PluginChecksumStatus`, the same read-and-verify helper backing `doctor`'s
  "plugin checksums" row above) — read-only and purely informational: it
  reports what `register_plugins`' real load-time gate would decide, but
  doesn't gate `list` itself, so a mismatching/malformed/missing plugin still
  appears with its full allowlists.
- `rustline plugin search [QUERY] [--json] [--refresh]` — browse the curated
  plugin index (W49) for widgets you can install; omitting `QUERY` lists
  every entry, otherwise a case-insensitive substring filter over name and
  description. Fetches `plugin_index_url` (default: `registry/index.json` on
  this repo's `main` branch) into a 24h-TTL cache at
  `<state_root>/plugin-index.json`; `--refresh` bypasses the cache. A fetch
  failure serves the last-known-good cached copy with a staleness warning on
  stderr (so `--json`'s stdout stays clean); a fetch failure with nothing
  cached is a hard error. Each row marks whether the plugin is already
  installed (via `discover_plugin_names`, without instantiating anything)
  and shows a `just build-plugin <name>` hint for a bundled entry or
  `rustline plugin install <owner/repo>` for an installable one. `--json`
  follows the repo's existing convention (a pretty-printed array). Strictly
  read-only — writes neither config nor the toggles file.
- `rustline plugin url|path|write-path|cmd list [--json]|add|remove <plugin>
  [pattern]` — read or edit a plugin's `allowed_urls`/`allowed_paths`
  (read)/`allowed_write_paths` (write)/`allowed_commands` (`add`/`remove`
  rewrite the config file in place via `toml_edit`, preserving
  comments/formatting); `list --json` emits the patterns as a JSON array of
  strings (`[]` for an absent/empty plugin) instead of one per line.
  `write-path` is `path`'s write-only counterpart — a grant in one never
  authorizes the other (see the Config section's filesystem-capability
  paragraph above). `cmd` is the exec capability's counterpart to
  `url`/`path`: its patterns are matched against the whole canonical argv
  (`rustline_wasm::canonical_argv`), not just the program name.
- `rustline plugin new <name> [--path <dir>] [--force]` — scaffold a
  ready-to-build WASM guest plugin crate at `<dir or cwd>/<name>/`
  (`Cargo.toml` with an empty `[workspace]` table + edition 2024 + cdylib,
  and a `src/lib.rs` skeleton using the typed `GuestRender`/`WireContext`),
  and print the build/install step plus a starter `[plugins.<name>]` config
  snippet. `<name>` must be `[A-Za-z0-9_-]`, ≤15 bytes, and not `window`
  (same rule as a widget's click-toggle range name); refuses to overwrite an
  existing `<name>/` directory without `--force`.
- `rustline plugin approve <name> [--yes]` — resolve `<name>`'s declared
  capability manifest (a sidecar `<name>.toml` in the plugin dir, or an
  embedded `rustline-manifest` wasm custom section), print what it requests —
  labeling the read line `allowed_paths (read-only)`, and including, when the
  manifest asks for `requested_write_paths`, an explicit overwrite-danger
  warning, and when it asks for `requested_commands`, an explicit warning
  that granting `allowed_commands` runs real programs with the user's own
  environment and permissions (both part of the printed report, so they show
  up under `--yes` too, not just an interactive prompt) — and, after an
  interactive y/N confirmation (or unconditionally with `--yes`), write
  exactly those requested URL/read-path/write-path/command patterns into
  `[plugins.<name>]`'s allowlists; declines (writing nothing) without
  confirmation, and does nothing if the plugin has no manifest or if it
  requests no capabilities at all.
- `rustline plugin build <dir> [--release] [--plugin-dir <d>] [--yes]` —
  build any WASM guest plugin crate (any `cdylib`-for-`wasm32-unknown-unknown`
  crate, not just this repo's `plugins/*`) and install the resulting `.wasm`
  into the plugin dir. Errors (never panics) on a missing crate/artifact.
  After a successful install, if that plugin has a recorded
  `[plugins.<name>].checksum` that no longer matches the freshly built bytes,
  it offers to refresh it: on a TTY it asks `Update the recorded checksum?
  [Y/n]` (Enter means yes — unlike `plugin approve`'s confirm, which defaults
  to no, since this is just re-pinning a hash to bytes you built yourself,
  not a new capability grant); `--yes` refreshes non-interactively (for
  scripts/CI). On a **non-TTY** run without `--yes`, it deliberately does
  **not** prompt and does **not** rewrite the checksum — it prints a notice
  and leaves the stale digest in place, since silently re-pinning in a
  non-interactive context would let a compromised local toolchain bless its
  own output. Stays silent whenever no checksum is recorded or the bytes
  already match. `just build-plugin` (which routes through this command)
  surfaces the same prompt/notice.
- `rustline plugin run <name> [--plugin-dir <d>]` — dev harness: instantiate
  one plugin, render it against a fabricated sample `Context`, and print its
  segments plus any capability denials it triggered. Read-only — touches
  neither config nor the toggles file.
- `rustline plugin install <owner/repo> [--name <n>] [--tag <t>] [--plugin-dir
  <d>]` — download a plugin's `.wasm` from its GitHub release into the plugin
  dir and record `source`/`tag`/`checksum` in `[plugins.<name>]`, granting
  **no** capabilities (run `approve` or `url|path add` afterward).
- `rustline plugin update <name> [--plugin-dir <d>]` — re-resolve the latest
  release for a recorded `owner/repo` source, re-download, and refresh the
  recorded `checksum`/`tag`.
- `rustline plugin remove <name> [--yes] [--plugin-dir <d>]` — delete an
  installed plugin's `.wasm`; with `--yes` also drop its `[plugins.<name>]`
  config entry.
- `rustline plugin denials <name> [--json]` — list a plugin's persisted
  capability denials (every distinct `(kind, target)` it was actually denied,
  recorded by the host's `FileDenialObserver` in `<data_root>/denials.jsonl`).
  Read-only; `--json` emits a JSON array of `{kind, target}` instead of human
  text.
- `rustline theme list [--json]` — every built-in + themes-dir theme, marking
  the active one and any built-in shadowed by a same-named file; `--json`
  emits a JSON array of `{name, active, source, shadowed}` (`source` is
  `"builtin"` or `"file"`) instead of human text.
- `rustline theme show <name>` — ANSI preview of `<name>` (default layout,
  synthetic Context tuned to show warning/error alert badges).
- `rustline theme use <name>` — set `[theme].base = "<name>"` in the config
  file (`toml_edit`, comment-preserving); errors without writing if `<name>`
  doesn't resolve.
- `rustline theme new <name> [--from <seed>] [--force] [--edit]` — scaffold
  `<themes_dir>/<name>.toml` as a complete, tweakable copy of `<seed>`
  (default `default`); refuses to overwrite an existing file without `--force`.
  `--edit` opens the new file in `$EDITOR` afterward (needs a TTY) and prints
  the `theme use <name>` follow-up either way.
- `rustline theme pick` — interactively list the themes (active marked,
  themes-dir files tagged `(custom)`), preview any by number (or `a`/`all` for
  every one), then prompt to set one by name or number (blank keeps the
  current theme), writing `[theme].base` via the same path as `theme use`.
  Previews default to a healthy bar (palette only); `t` toggles the
  warning/error alert-badge colors on/off. Requires a terminal — a non-TTY
  invocation prints a hint toward `theme show`/`theme use` and exits non-zero
  without writing.
- `rustline click --range=<name> [--button=left|middle|right]` — resolve the
  click via `click::resolve_click` and dispatch it (W36): the default
  (unconfigured) action is a left-click toggle of `<name>`'s membership in the
  global toggle state file; a `[widgets.<name>.click]` binding
  (`left_click`/`right_click`/`middle_click`) overrides per button with a
  toggle/`open_url`/`run` action. Invoked by the `init`-emitted
  `MouseDown{1,2,3}Status` tmux bindings (left/middle/right).
- `rustline daemon [run] [--plugin-dir <d>] | status | stop` (W48) — the
  optional persistent daemon. `rustline daemon` and `rustline daemon run` are
  equivalent (bare form defaults to `run`): runs the server in the
  foreground, keeping the parsed config, resolved theme, built widget
  registry, and instantiated WASM plugins warm across renders instead of
  rebuilding them on every tmux refresh; a supervisor (systemd unit, tmux
  `run-shell … &`) backgrounds it — rustline never self-daemonizes/forks.
  `rustline daemon status` connects and pings, printing running/not-running
  with the exit code reflecting it; `rustline daemon stop` connects and asks
  it to shut down. Render clients (`render left|right|window`) try the
  daemon's socket first and **transparently fall back to the normal
  in-process render** on any failure (including the daemon simply not
  running) — the daemon is purely an optional speedup, never a dependency.
  One caveat: a per-invocation `render --plugin-dir=X` is ignored while a
  daemon is reachable (the wire protocol carries no `plugin_dir`; the daemon
  always uses whatever `--plugin-dir` it was started with).
- `rustline daemon install [--write-only] [--binary <path>]` — install the
  daemon as a per-user service that starts at login, using the platform's
  native mechanism (chosen at compile time; `ExecStart`/`ProgramArguments`
  `<binary>` defaults to the running binary's own resolved absolute path, same
  resolution `init` uses). **On Linux** it writes a systemd **user** unit to
  `$XDG_CONFIG_HOME/systemd/user/rustline-daemon.service` (fallback
  `~/.config/systemd/user/`) and, unless `--write-only` or `systemctl` isn't on
  `PATH`, runs `systemctl --user daemon-reload` + `enable --now
  rustline-daemon.service`. **On macOS** it writes a launchd **LaunchAgent** to
  `~/Library/LaunchAgents/rustline-daemon.plist`
  (`RunAtLoad`, `KeepAlive={SuccessfulExit=false}` = restart on failure only)
  and, unless `--write-only` or `launchctl` isn't on `PATH`, loads it via
  `launchctl bootout` (clear any stale instance) + `bootstrap gui/$UID
  <plist>`. Either way it reports a replaced file and prints the manual load
  command as a fallback; a `systemctl`/`launchctl` failure is never fatal — the
  unit/plist file is already written.
- `rustline daemon uninstall` — best-effort unload the service (Linux
  `systemctl --user disable --now`, macOS `launchctl bootout`, ignoring "not
  loaded"), remove the installed unit/plist file if present (Linux also reloads
  the user daemon-reload cache); prints what it did, including a "nothing to
  remove" note if it was already gone. Safe to run more than once.
- `rustline bench [--only regions|widgets|sources|plugins|daemon|all] [--iters N]
  [--real-iters N] [--warmup N] [--cold] [--format table|markdown]
  [--output FILE] [--plugin-dir DIR] [--state-dir DIR]` — feature-gated
  (`--features bench`) benchmark of the render pipeline: a pure pass
  (fabricated `Context`, no reads) vs a real-world pass (real reads + render),
  plus per-widget, per-read, and per-plugin timing. Plugin passes run against
  real preserved state so cached fast-paths are measured honestly. `--only
  daemon` (included in `all`) benches `left`/`right` in-process (cold,
  re-instantiating any WASM plugins) against a real round-trip to a reachable
  `rustline daemon run` — skipped with a note (never spawning one) when no
  daemon is reachable; see `bench/daemon.rs` above.

`--plugin-dir` overrides plugin discovery for that invocation (see Config
below for the full resolution order).

`--preview` prints a region in ANSI colour on the terminal (for manual
verification) instead of raw tmux markup; without it, stdout is the raw markup
tmux consumes (stdout is the status line — logs always go to stderr).

## tmux integration model

Shell-out per region on `status-interval`. Each `#(<binary> render …)` spawn
is still a fresh process either way — the difference an optional `rustline
daemon` (W48, see CLI above) makes is what happens *inside* that process:
with a reachable daemon, the spawn is a thin Unix-socket round-trip against
already-warm state instead of a cold rebuild of the config/theme/registry/
plugins. Nothing about the tmux wiring itself changes; `init` doesn't wire up
the daemon (no wizard question for it yet — start/manage it yourself, see
README). The block
`rustline init` writes (via `init_block`) wires `status-left`/`status-right`/
`window-status-format` to `#(<binary> render …)` — `<binary>` is the
resolved, shell-quoted absolute path to the running binary
(`std::env::current_exe()`, overridable with `init --binary <path>`), not a
bare `rustline`, since tmux's `#(...)` shells out via the *tmux server's*
`/bin/sh`, whose `$PATH` may not include wherever the user installed it
(e.g. `~/.local/bin`) — a bare name there can silently resolve to nothing and
leave the bar empty — and adds `after-select-pane`/`after-select-window` →
`refresh-client -S` hooks for instant updates. It also emits three
`bind -T root MouseDown{1,2,3}Status` blocks (left/middle/right — tmux button
numbering): the **left** binding keeps the default `select-window` on a
window-name click and otherwise runs `rustline click --range=… --button=left`;
the **middle**/**right** bindings (W36) have no window-list default to preserve,
so they simply dispatch any non-empty `#{mouse_status_range}` as
`--button=middle`/`--button=right` — the action per (widget, button) is chosen
by `[widgets.<name>.click]`, which is why right/middle previously shipped inert
(they resolved but had no tmux binding to fire them). Each block ends with
`refresh-client -S`. All three follow the same injection-safe
`--range=#{q:mouse_status_range}` form (invariant #4). This requires
**tmux ≥ 3.1** (that's when
`range=user|X` status ranges and the `mouse_status_range` format variable were
added) and, at the tmux-config level, `set -g mouse on` — the wizard's mouse
question (`InitAnswers.mouse`) can add that setter for you (`--print` never
does; it always emits the mouse-off, one-line legacy block regardless of
config).

**Widget-manager popup (`prefix + W`).** The block also emits
`bind-key W display-popup -E -w 80% -h 80% "<binary> widget edit"` —
unconditionally, the same way the three `MouseDown*Status` blocks are, and
with no tmux format variable interpolated (nothing to `#{q:}`-escape). This
needs **tmux ≥ 3.2** for `display-popup` (one version tier above the ≥ 3.1
the status line itself needs for `range=user|`); `doctor` carries an
advisory-only row for it (see CLI above). Because `init_block` emits this
line the same way it emits the mouse bindings, `rustline init --print`'s
legacy one-line block **also** carries the `bind-key W` line now — `--print`
was never special-cased to omit it, and no test pins the legacy block
byte-for-byte, so this is the correct, consistent outcome rather than a gap.
`init --uninstall` removes it along with the rest of the managed block, same
as every other binding.

**Two-line window list is batched (W42).** In two-line mode, `status-format[0]`
(the window-list line) used to invoke tmux's own `#{W:...}` loop, which expanded
to a `#{T:window-status-format}` per-window `#(<binary> render window …)` call —
one process spawn per window, every refresh. It's now a single `#(<binary>
render windows --session=#{q:session_name})` call: `rustline` itself shells out
to `tmux list-windows -F` (`windows.rs`, client-side — not another tmux format
expansion) and renders every window's pill in one process, re-emitting each
one's `#[range=window|IDX]` so tmux's built-in window-select click keeps
working (N spawns per refresh → 1). Injection-safe: the only interpolated tmux
var is the `#{q:}`-escaped `session_name`; window names/flags come back from
`tmux list-windows -F`, never through the shell. This batched path is
deliberately **in-process only** — unlike the singular `render window` (and
`render left`/`render right`), it does not try the daemon first, since a
systemd/launchd-managed daemon has no `$TMUX` in its environment to run `tmux
list-windows` against. **One-line mode is unchanged** —
`window-status-format`/`window-status-current-format` still call `render
window` once per window.

**Injection safety (critical):** tmux expands `#{…}` inside `#(…)` *before*
`/bin/sh -c` and does not shell-escape. So the `init` block passes every tmux var
as `--flag=#{q:VAR}` — the `#{q:}` modifier escapes it and the `--flag=` form is
empty-safe. Never emit a bare `'#{window_name}'` or `'#{pane_current_path}'`.
This is why `render window` takes named args, not positional; the click
binding's `--range=#{q:mouse_status_range}` follows the same rule. See
`tmux_conf.rs`.

## Config

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

**Per-widget color override (`fg`/`bg`):** every format-bearing widget accepts
optional `fg`/`bg` keys (W29) that pin its foreground/background color,
flattened directly into its `[widgets.<name>]` table. Applied centrally in
`render_named_region` — after the widget renders, before `assign_palette` fills
the cycling palette — so widgets stay `Context`-only (invariant #1). `bg` only
takes effect on a segment that doesn't already carry an explicit background
(the same rule `assign_palette` follows for an alert badge); `fg` applies
wherever set. Both default to `None`, so an unset override is byte-identical to
before (invariant #3). Colors are `Color` enums (`{ Indexed = N }` /
`{ Named = "cyan" }` / `{ Rgb = [r,g,b] }`).

```toml
[widgets.cwd]
fg = { Indexed = 250 }
bg = { Rgb = [40, 44, 52] }
```

**Click-to-toggle widget views:** the twelve format-bearing widgets —
`datetime`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`, `memory`, `loadavg`,
`git`, `disk`, `uptime`, `media`, `throughput` — each take an additional `alt_format` (default `""`, `#[serde(default)]`, so
covered by invariant #3 like every other opt). A non-empty `alt_format` makes
that widget clickable: left-clicking it in the tmux status line toggles it
between `format` and `alt_format`.

```toml
[widgets.cpu]
format     = "{icon} {percent}%"
alt_format = "{icon} {bar} {percent}%"   # left-click toggles to this
```

**Per-button click bindings (`[widgets.<name>.click]`):** beyond the default
left-click toggle, any widget can bind a specific mouse button to a
`toggle`/`open_url`/`run` action (W36), flattened as
`left_click`/`right_click`/`middle_click` into its `[widgets.<name>]` table.
The default action fires only when no binding matches, so an unconfigured
widget is byte-identical to before (invariant #3).

```toml
[widgets.cpu]
right_click = { run = "tmux display-popup -E htop" }
middle_click = { open_url = "https://grafana.example/host" }
# left_click = { toggle = false }   # explicitly disable the default left toggle
```

**Important — a `run`/`open_url` binding needs the widget to emit a clickable
range.** A widget only becomes a tmux click target when it emits
`#[range=user|NAME]`, which today happens iff it has a non-empty `alt_format`
(or it's a plugin/unknown name). So a `run`/`open_url` binding on a widget with
no `alt_format` resolves correctly but never fires — tmux never sends a click
for a widget that emits no range. Give such a widget an `alt_format` (even a
throwaway one) to make it clickable, or wait for a future range-on-binding
change. Left-click `run`/`open_url` on an `alt_format` widget works today;
right/middle now fire via the new `MouseDown2/3Status` bindings.

Toggle state is **global**, not per-widget-instance or per-session: one flat,
newline-delimited set of toggled widget/plugin names at
`$XDG_DATA_HOME/rustline/toggles` (fallback `~/.local/share/rustline/toggles`),
read once into `Context.toggled` at Context-build time and flipped by
`rustline click --range=<name> [--button=left]` (see CLI above). WASM plugins
participate the same way — `Context.toggled` rides the JSON boundary to the
guest, and a plugin honors toggling by checking whether its own `name()` is a
member; the `weather` example demonstrates this via `options.alt_format`. Any
widget or plugin name longer than 15 bytes is simply not clickable (tmux's
`range=user|X` byte cap — see tmux integration model above).

A plugin author should pick a name (the `.wasm` stem) that is ≤ 15 bytes,
avoids the reserved name `window`, and sticks to `[A-Za-z0-9_-]`, since it
becomes a tmux `range=user|<name>` argument verbatim. Also avoid the handful
of host-owned state-file names the CLI writes flat under `<state_root>`, each
of which would collide with a plugin's own same-named state directory:
`cpu-sample` (the `cpu` widget's snapshot cache), `cpu-history`/
`memory-history` (the `{spark}` history rings, W45), `throughput-sample`/
`throughput-sample-<iface>` (the `throughput` widget's prior counter sample —
one file for the aggregate read, one per named interface once multiple
`throughput` instances exist, W46/W47), `wasmtime-cache`/
`wasmtime-cache.toml` (the WASM compile cache dir + its config, W43 — kept
deliberately distinct from any plugin state subdir), `plugin-index.json` (the
24h-TTL cached copy of the curated plugin index, W49 — see
`plugin_index.rs` above), and `daemon.sock` (the
optional persistent daemon's Unix socket under `$XDG_RUNTIME_DIR/rustline/`,
falling back to `<state_root>/daemon.sock`, W48). Every collision degrades
gracefully (a failed create/rename is `warn!`-logged, never panics), but is
still best avoided.

**Window pill (`[theme]`):** the window list renders as a rounded pill (see
Render pipeline). Six optional `[theme]` fields override the defaults — active
pill in the accent color, inactive pills gray, rounded caps:

```toml
[theme]
win_current_bg = { Indexed = 31 }    # active fill (accent); default colour31
win_current_fg = { Indexed = 255 }   # active text (bold); default colour255
win_inactive_bg = { Indexed = 236 }  # inactive fill (gray); default colour236
win_inactive_fg = { Indexed = 250 }  # inactive text; default colour250
win_cap_left = ""                    # rounded left cap; default U+E0B6
win_cap_right = ""                   # rounded right cap; default U+E0B4
```

Active is always bold (intrinsic, not a field). Colors are `Color` enums
(`{ Indexed = N }`, `{ Named = "cyan" }`, or `{ Rgb = [r,g,b] }`); caps are
strings. All `#[serde(default)]`, so they stay covered by invariant #3.

**Themes:** `[theme].base` names a starting theme; resolution layers
`Theme::default()` → the selected base → the inline `[theme]` field overrides
above (`win_*`, `palette`, `fg`, `bar_bg`, the separators, the semantics
below) on top:

```toml
[theme]
base  = "nord"                # a built-in name, or a *.toml stem in the themes dir
error = { Named = "red" }     # per-field overrides still apply on top of base
```

Seven built-ins ship in `rustline-core::themes` (`builtin_theme_names()`):
`default`, `pastel-rainbow` (the flagship multi-accent), `nord`, `gruvbox`,
`catppuccin-mocha`, `tokyo-night`, `dracula`. Every theme but `default` is multi-accent
(`palette.len() >= 4`) and uses **truecolor** (`Color::Rgb`) — a
truecolor-capable terminal and tmux `RGB`/`Tc` are required to see the exact
colors (see README). `base` is resolved **themes-dir file first**
(`$XDG_CONFIG_HOME/rustline/themes/<name>.toml`, fallback
`~/.config/rustline/themes`), then the built-in registry — a user file
**shadows** a same-named built-in. An unknown/unresolvable `base`, or an
unparseable theme file, `warn!`s and falls back to `default` (invariant #3).

Every theme also sets four **semantic colors** — `success`/`info`/`warning`/
`error` — which reach both built-in widgets and WASM plugins via
`Context.colors: ThemeColors` (see `context.rs` above), not `Theme` directly.
The five threshold-aware widgets (`cpu`, `memory`, `battery`, `loadavg`, `disk` — see
their config blocks above for each widget's `warn_*`/`crit_*` field and
default) use them for an inverse alert badge (`bg`=semantic, `fg`=`bar_bg`,
bold) when a reading crosses a threshold; `0` (or `0.0`) disables a tier, and
a widget with every tier disabled or every reading below threshold renders
byte-identically to before this feature.

Manage themes from the command line (`rustline theme list|show|use|new|pick` —
see CLI above) instead of hand-writing `[theme]`/theme files. See the
[design spec](docs/superpowers/specs/2026-07-21-rustline-themes-theme-picker-design.md)
for the full layering rules, the six themes' exact color values, and the
threshold-badge contrast rationale.

**Plugins:** an optional top-level `plugin_dir` (default
`$XDG_DATA_HOME/rustline/plugins`, `~/` expanded), an optional top-level
`plugin_index_url` (W49 — overrides where `rustline plugin search` fetches
the curated plugin index from; default: `registry/index.json` on this
repo's `main` branch, via raw.githubusercontent.com), plus a typed
`[plugins.<name>]` table per plugin, keyed by the plugin's name (the `.wasm`
filename stem):

```toml
plugin_dir       = "~/.local/share/rustline/plugins"     # optional
plugin_index_url = "https://example.com/my-index.json"   # optional; overrides the built-in default

[plugins.weather]
source = "steve/rustline-weather"          # owner/repo; consumed by `plugin update`
allowed_urls = ["https://wttr.in/*"]        # glob, or "re:<pattern>" for regex
allowed_paths = []                          # READ-only; see the filesystem-capability paragraph below
allowed_write_paths = []                    # WRITE-only, globs; empty = deny by default
resolve_symlinks = false                    # default; see below for what `true` trades away
allowed_commands = []                       # glob/"re:", matched against the WHOLE argv (see below)
max_state_bytes = 52428800                  # default: 50 MB
# tag = "v1.2.0"                            # recorded by `plugin install/update`
# checksum = "<sha256-hex>"                 # recorded by `plugin install/update`; VERIFIED at load time (W19, see below)

[plugins.weather.options]
zip = "48183"
format = "{icon} {temp_f}°F"
refresh_secs = 1800
```

Every `PluginConfig` field is `#[serde(default)]`, so the whole table stays
covered by invariant #3. `options` is an opaque TOML table forwarded to the
plugin's `render` call verbatim. Allow-pattern entries are a **glob** by
default (matched against the full URL/path string), or a **regex** when
prefixed `re:` — regex entries are **anchored to a full-string match** (uniform
with globs); to match a prefix/substring, include `.*` in the pattern (e.g.
`re:https://wttr\.in/.*`).

**Filesystem capability (`allowed_paths` / `allowed_write_paths` /
`resolve_symlinks`):** `rl_file_read`/`perform_file_read` is gated by
`allowed_paths`, which is **read-only**, and `rl_file_write`/
`perform_file_write` by the separate `allowed_write_paths`, which is
**write-only** — an entry in one never authorizes the other. Both are empty
(deny-by-default) unless populated, same glob-or-`re:` shape as the other
allowlists. **This is a change from before this batch**, when one
`allowed_paths` list authorized both directions: an existing `allowed_paths`
entry now grants read only, and a plugin that was writing through it fails
closed — a denial record plus a log line — rather than silently. Every
read/write request is resolved through `resolve_for_allowlist`
(`rustline-wasm`'s `state.rs`, see above) before either list is consulted,
which also closes a symlink escape: a grant is otherwise a grant over
**names**, and a symlink planted under a granted prefix (an extracted
archive, a synced directory, another tool) would silently redirect the
effect to wherever it points — past the subtree the user thought they'd
granted. By default (`resolve_symlinks = false`) a path with any symlink
component is denied outright, closing that name-vs-subtree gap for good;
setting `resolve_symlinks = true` on a plugin trades that closure away — the
path is canonicalized first (falling back to canonicalizing the parent for a
not-yet-existing write target, so a first write to a granted directory still
works) and the allowlist is matched against the **resolved** location, so a
grant then legitimately follows symlinks out of the directory the user
thought they were granting. Canonicalization failing is a hard denial, never
a silent fall-through to the raw path.

A plugin may also declare a capability **manifest** — a sidecar
`<plugin_dir>/<name>.toml` (or an embedded `rustline-manifest` wasm custom
section) listing `requested_urls`/`requested_paths`/`requested_write_paths`/
`requested_commands` — which `rustline plugin approve <name>` turns into
exactly those allowlist entries above, after confirmation (see CLI above). A
manifest alone grants nothing; only `approve` (or hand-editing the config)
ever widens an allowlist. `plugin approve` prints an explicit
overwrite-danger warning when a manifest requests write paths, and a
separate explicit warning when it requests commands — see the exec-capability
paragraph below.

`source` is a typed `PluginSource` that still accepts a bare `owner/repo`
string; `rustline plugin install <owner/repo>` (W38) downloads the plugin's
`.wasm` from its GitHub release into the plugin dir and records
`source`/`tag`/`checksum` here — but grants **no** capabilities (the same
allowlist-widening rule: run `approve` or `url|path|cmd add` afterward). The
recorded `checksum` is TOFU at write time (trust-on-first-use: it records the
hash of what it downloaded, not verified against any external pin). `plugin
update` re-resolves the latest release for that `source` and refreshes
`checksum`/`tag`.

**The recorded `checksum` is verified at load time (W19) — it is no longer
"recorded, never checked."** `register_plugins` (`rustline-wasm::integrity::
verify_checksum`) hashes a discovered `.wasm`'s bytes and compares them
against `[plugins.<name>].checksum` right after reading the file and before
the plugin ever reaches `CapabilityCtx`/wasmtime (invariant N1: a rejected
plugin never receives a capability object). Three outcomes: an **absent** (or
blank) checksum loads exactly as before — nothing recorded means nothing to
verify, so this is fully backward compatible; a well-formed digest that
**mismatches** the bytes on disk is skipped with a `warn!` (a swapped or
tampered `.wasm` can no longer load silently); and a **malformed** value —
anything other than 64 hex chars, tolerating an optional `sha256:` prefix and
surrounding whitespace — **fails closed**, also skipped, on the reasoning
that a checksum we can't even parse is a verification request we cannot
honor, not evidence the plugin is unpinned. `rustline plugin run`
(`instantiate_named`) is the one deliberate exception: it's **warn-only** on
a bad verdict — it logs and still runs the plugin — since that dev harness is
used right after rebuilding a plugin, where a recorded digest legitimately
mismatches every build; the bar and the daemon (which rebuilds its registry
via this same `register_plugins` on a config-file mtime change) both go
through the real, non-bypassable gate. `plugin list` is not gated by this at
all — a checksum-failing (or entirely missing) plugin still appears in the
list exactly like a healthy one, with its full allowlists — but it now
*surfaces* the same read for visibility: both the human and `--json` output
include a per-plugin checksum status (`unpinned`/`verified`/`mismatch`/
`malformed`/`missing`, via `plugin_checksum::status_for` — the same
read-and-verify helper backing `doctor`'s "plugin checksums" row; see the CLI
section above). Purely informational: `list` reads the `.wasm` bytes to
compute it, but the verdict never changes what gets listed.

**Rebuilding an installed plugin's stale checksum is handled by `plugin
build` itself, not left as a hazard.** `checksum` is written **only** by
`plugin install`/`plugin update` (grep confirms `plugin_install.rs` is the
sole writer of that field) — `plugin build` (and `just build-plugin`, which
now routes through it) just installs the freshly built `.wasm`. But after a
successful install, `build_plugin` calls `plugin_cmd.rs`'s
`maybe_refresh_stale_checksum`: if that plugin has a recorded checksum and
the rebuilt bytes no longer verify (`!ChecksumVerdict::allows_load()`), it
offers to fix it right there — on a TTY, `confirm_checksum_refresh` asks
`Update the recorded checksum? [Y/n]` (Enter means yes, unlike `plugin
approve`'s no-defaulting confirm, since this just re-pins a hash to bytes
the user built themselves rather than granting a new capability); `--yes`
(`BuildArgs::yes`) does the same non-interactively.
On a **non-TTY** run without `--yes` — the CI/script case — it deliberately
does **neither**: no prompt, no rewrite, just `stale_checksum_notice`'s
printed reminder, so a compromised local toolchain can't silently bless its
own rebuilt output by re-pinning the checksum on its own. It stays quiet
whenever no checksum is recorded or the bytes already match. For that
non-TTY case, or a `plugin build` run before this refresh existed, the manual
recovery still works: clear `[plugins.<name>].checksum` (opts that plugin
back out of verification) or, if the plugin has a recorded `owner/repo`
`source`, run `rustline plugin update <name>` — though that re-downloads the
published release and overwrites your local rebuild rather than hashing it,
so it's only the right recovery when you're fine reverting to what's
published.

**What checksum verification does and does not defend against.** It catches
accidental swaps, truncated/corrupt downloads, stale build artifacts, and a
plugin dir writable by a different principal than the config — genuinely
useful failure modes. It is **not** a privilege boundary: anyone who can write
`~/.local/share/rustline/plugins/*.wasm` can equally edit
`~/.config/rustline/config.toml` and delete the `checksum` line, and there is
no strict/"require checksum" mode that would stop them. It is also **not** a
pin against a compromised upstream: `plugin install`/`plugin update` both
*rewrite* the digest from whatever they just downloaded (trust-on-first-use;
see above) — a poisoned release is recorded and then verified against itself,
not caught.

**Exec capability (`allowed_commands`):** a plugin can ask the host to run a
program on its behalf via `rl_exec` (plain, every render) or `rl_exec_cached`
(TTL-cached, keyed on the whole command line — `cmdrun`'s worked example,
see the Architecture section above), gated by `[plugins.<name>].allowed_commands`
(glob by default, `re:` for regex, same shape as `allowed_urls`/`allowed_paths`,
empty = deny by default). Four properties make this capability's blast radius
explicit rather than implicit:
- **No shell, ever.** The host spawns `program` plus an explicit `args` array
  directly (`std::process::Command`) — nothing re-parses a string, so there is
  no quoting/word-splitting surface. A plugin that wants a shell must be
  granted one explicitly and visibly, e.g. `allowed_commands = ["sh -c *"]`;
  the host never introduces one on its own.
- **The gate matches the whole canonical argv, not just the program.** A grant
  for `git status*` does **not** cover `git push --force` — `rustline_wasm::
  canonical_argv(program, args)` renders `program` + every argument
  (quoting one that contains whitespace/quotes/is empty, so two different
  argv vectors can never collapse onto the same matched string) and the
  *whole* rendered string is what a pattern is checked against.
- **Bounded and capped.** A 5 s wall-clock timeout (under Extism's 10 s plugin
  timeout, so a hung command surfaces as a renderable failure rather than
  killing the whole plugin render), a 64 KiB cap per stdout/stderr stream
  (flagged `truncated` past it, not silently dropped), and stdin closed
  (`Stdio::null()`, so a command that tries to read stdin gets EOF instead of
  hanging). The child runs in its own process group so a backgrounded
  descendant (e.g. `sh -c "long_thing &"`) can't hold the piped output open
  past the timeout and hang the render — and even a descendant that escapes
  that group entirely (`setsid`/`setpgid`) is bounded by a short output-
  collection grace period afterward, so its own output is dropped rather than
  hanging the render (`rustline-wasm::run`'s `OUTPUT_GRACE`).
- **Environment and working directory are inherited from the host process —
  plainly, not as a footnote.** This is why `playerctl`/`git`/etc. work
  without extra plumbing, and it's a real part of what you're granting: an
  approved command sees your shell's environment variables and runs from
  wherever `rustline` itself runs, with your user's permissions. Approve
  `allowed_commands` patterns you actually understand.

Cache semantics for `rl_exec_cached` mirror `rl_http_get_cached`'s shape with
one deliberate difference: only a **zero-exit** run is written to the cache,
and a **non-zero exit is returned immediately as fresh data** (the command
genuinely ran and reported that status — it isn't a transient failure to
paper over with stale data); only a run that couldn't happen at all (denied,
spawn failure, or timeout) falls back to the last-good cached entry with
`stale: true`.

**Logging:** a `[log]` table controls the two sinks. `rustline` logs to a
**daily-rotated** file (default dir `$XDG_DATA_HOME/rustline`, default level
`info`) and to stderr (default level `error`). `RUST_LOG` is **not**
consulted. Raise the *file* level with repeated `-v` (`-v`=warn, `-vv`=info,
`-vvv`=debug, `-vvvv`=trace); `-v` never affects stderr. Today's file is
`<dir>/rustline.<YYYY-MM-DD>.log` (e.g. `rustline.2026-07-27.log`) — **not**
`rustline.log.<date>` — computed in **UTC**, matching the boundary
`tracing-appender`'s `RollingFileAppender` itself rotates on; **7** daily
generations are kept (`MAX_LOG_FILES`), each pruned as the eighth appears.
This replaced the old size-capped rotator: there is no 5 MiB cap and no
single `.1` backup anymore — `[log].file`, if set, no longer names one fixed
path. Instead it's decomposed into a directory (its parent), a filename
prefix (its stem), and a filename suffix (its extension), which the daily
appender recombines as `<prefix>.<date>.<suffix>` each day. Any level value
is `off|error|warn|info|debug|trace` and is parsed leniently (a typo falls
back to the default).

    [log]
    file_level   = "info"
    stderr_level = "error"
    file         = "~/.local/share/rustline/rustline.log"   # optional override; decomposed into dir/prefix/suffix — see above

**Warn dedup:** a `warn!`/`info!` that describes a *static misconfiguration*
(an unparseable theme file, an unknown widget name, a plugin with no
`abi_version` export, …) — one that would otherwise re-fire on every render
tick, forever — is logged at most once per config generation via
`rustline_core::diag::warn_once` (see `diag.rs`/`warn_once.rs` in the module
map above). The dedup state lives at `<data_root>/.warn-markers/`
(`~/.local/share/rustline/.warn-markers`), a sibling of `denials.jsonl`,
`toggles`, `plugin-index.json`, and the log directory under the same
`data_root()`; it is cleared whenever `config.toml`'s mtime changes, so
editing the config re-arms every warning for one more render. A genuine
per-occurrence runtime failure (a plugin render failing, a cache write
failing, a denial) is deliberately NOT routed through this — only a
misconfiguration that stays constant between renders is.

## Invariants (load-bearing — re-check when touching these)

1. **`Context` is the sole render input.** Widgets read only from `Context`,
   never the environment mid-render (keeps the daemon/WASM path viable). `cwd`
   reads `ctx.home`, not `$HOME`.
2. **`Segment`/`Context`/`Style`/`Color` stay serde-serializable** — this is
   the WASM ABI. `Segment`/`Style`/`Color` now live in `rustline-abi`
   (re-exported by `rustline-core`); `Context`/`WindowCtx` stay in
   `rustline-core` (they carry `chrono`). `rustline_abi::ABI_VERSION` (W32,
   currently `1`) versions this wire contract: the host stamps it onto every
   `RenderInput`, and `abi_decision` **skips** a guest that declares a
   *different* version (a guest with no `abi_version` export still registers as
   legacy). Keep the wire types **additive** — no `deny_unknown_fields`, so an
   older guest keeps deserializing a newer `Context` (this is why W54 could
   mirror `uptime`/`media`/`throughput`/`cpu_history`/`mem_history` into
   `WireContext` as a plain field addition, breaking nothing). W46's
   `Context.disks`/`Context.throughputs` maps (the per-instance disk/
   throughput readings) are the only fields still NOT mirrored into
   `WireContext`; a WASM guest still sees only the
   base `Context.disk`/`Context.throughput` singular reading, unaffected by
   how many `disk`/`throughput` instances exist in the layout).
3. **`Config::load` is total** — a bad config must never break the bar.
4. **`init` output must be injection-safe** (`#{q:}` + `--flag=` form).
5. **`render_region` puts `segments[0]` leftmost regardless of `Direction`.** The
   caller passes widgets in visual left-to-right order (e.g. `cfg.layout.right`),
   which is not reversed.
6. **`loadavg` is `Option`** — a failed `getloadavg` renders nothing, never fake
   zeros. A panicking widget degrades to empty via the `catch_unwind` guard.
7. **The click-toggle NAME is one identity end-to-end.** The range name
   `render_region_ranged` emits (`#[range=user|NAME]`), tmux's
   `#{mouse_status_range}`, the `--range` value `rustline click` receives, the
   `Context.toggled` key, and a widget's/plugin's own `range_name()`/
   `active_format` key must all be the *same* layout/registry name. Break that
   chain anywhere and the widget silently stops being clickable or
   toggleable — there's no error, just a click that does nothing. W46
   (multiple widget instances) extends this: for a named `[instances.<name>]`
   entry, that identity chain runs on the **instance** name, not its `kind` —
   each of the twelve clickable widget structs now carries its own `name:
   WidgetName` field precisely so `render`/`range_name`/`active_format` all key
   off the instance name instead of a hardcoded kind literal. This also
   means an instance can never be allowed to shadow a built-in of the same
   name (it would silently steal that built-in's click/toggle identity) —
   `Registry::with_builtins` skips a colliding instance outright (built-in
   wins), and `Config::color_overrides()`/`click_map()`/`layout_kinds` apply
   the same `WidgetKind::parse(name).is_some()` precedence check so a colliding
   instance's config can't hijack the built-in's projected color/click
   binding either (a W46 review-caught bug — see `config.rs` above).
8. **Segment text is never interpolated into tmux markup unsanitized.** tmux
   parses `#[...]` in the output of a `#()` job — that is exactly how this
   renderer colours the bar — so a `#` in *content* is a live directive, and
   content reaching the renderer is routinely external and untrusted: a tmux
   window name, a git branch from a cloned repo, an MPRIS now-playing title, a
   directory name, or a WASM guest's `Segment.text`. `render.rs`'s private
   `sanitize_text` doubles `#` (tmux's documented literal-`#` escape) and
   replaces C0 control characters, and it is applied at the **three and only
   three** sites that write segment text: `render_region`,
   `render_region_ranged`, and `render_window_pill`. Without it a
   zero-capability plugin can emit `#[norange]#[range=user|cpu]`, forge another
   widget's clickable range, and have `rustline click` dispatch that widget's
   configured `run =` binding through `sh -c` — authority `CapabilityCtx` never
   granted (N1) and a direct break of invariant #7's identity chain. A newline
   is just as damaging without any injection intent: tmux inserts only the LAST
   line of a `#()` job's output, so a `\n` in a path silently deletes every
   widget to its left. Sanitizing at the render boundary rather than in each
   reader is deliberate — it is the only place that also covers plugin-supplied
   `Segment.text`, which no reader touches. `ansi.rs`'s scanner collapses `##`
   back to a literal `#` so `--preview`, `theme show` and the `widget edit`
   preview strip agree with what tmux actually draws. **Adding a fourth site
   that writes non-renderer-produced text into markup means routing it through
   `sanitize_text` too.** The one deliberate exemption is
   `assemble.rs`'s `#[range=window|{index}]`, whose only production feed is
   tmux's own integer `#{window_index}` (documented at that call site). Note
   this is the *output* direction; invariant #4 covers the opposite one (tmux
   format vars into rustline's argv), and `#{q:}` escapes for `/bin/sh`, not
   for tmux markup, so the two are unrelated defences.

**Platform-specific reads stay at the `Context`-build edge.** `read_battery()`
(`crates/rustline/src/battery.rs`), `read_cpu()` (`crates/rustline/src/cpu.rs`),
and `read_memory()` (`crates/rustline/src/memory.rs`) are the three
`#[cfg(target_os)]` surfaces in the codebase; each OS arm (Linux sysfs/`/proc`,
macOS `pmset`/`sysctl`+`vm_stat`, and cpu's mach `host_statistics`) delegates to
a pure parser where the source is text (`parse_linux`/`parse_pmset`,
`parse_proc_stat`, `parse_meminfo`/`parse_macos_memory`) that is
`#[cfg(any(target_os = …, test))]`-compiled, so those are unit-tested on the
dev box even though only one reader arm per module compiles per platform. (The
cpu reader is the one exception to the pure-text-parser shape: its macOS arm is
a mach FFI call returning counters directly, so it feeds the *shared*
snapshot-delta helpers rather than a bespoke parser — see `cpu.rs` above.) Follow this
pattern for any future OS-specific signal. `Context.os`/`Context.arch` (from
`std::env::consts::OS`/`ARCH`) are now available for WASM guests that want to
branch on platform.

**WASM plugin invariants (added by the plugin system — re-check when touching
`rustline-wasm` or `plugins/*`):**

9. **N1. Zero ambient authority.** A guest runs with `with_wasi(false)` and no
   Extism built-in HTTP/FS/process-spawn; every network/filesystem/exec
   effect goes through a host function that checks the plugin's
   `CapabilityCtx` first. Adding a new host capability means adding its gate
   *and* a denied-case test. The
   TTL-cached GET (`rl_http_get_cached`) gates `allowed_urls` before any fetch
   (gate-first: a denied URL makes no network call and touches no cache),
   with its own denied-case test. The exec capability (`rl_exec`/
   `rl_exec_cached`) is the most recent worked instance of this pattern:
   `perform_exec`/`perform_exec_cached` gate on `canonical_argv(program,
   args)` — the whole quoted command line, not just `program` — against
   `allowed_commands` before `runner.run` is ever called, with denied-case
   tests proving a denied argv never reaches a spawn (and, for the cached
   variant, never touches its own cache file either). Every deny site also
   calls `observe_denial` (before returning `ok:false`) so the default
   `FileDenialObserver` both records the `(kind, target)` for `rustline
   plugin denials` (W28) and logs a deduped `warn!` pointing at that command
   — recording (and now logging) is best-effort and never changes the gate
   outcome. `perform_file_read`/`perform_file_write` are the filesystem
   capability's instance of this pattern: they gate on the separate
   `allowed_paths` (read) / `allowed_write_paths` (write) allowlists, after
   first resolving the path through `resolve_for_allowlist`'s symlink gate —
   see the Config section's filesystem-capability paragraph.
10. **N2. A plugin never breaks the bar.** Any instantiation error, render
   error, timeout, or malformed output degrades to empty segments
   (`WasmWidget::render`), bounded by fuel + wall-clock timeout + memory caps.
   This composes with, not replaces, the existing `catch_unwind` per-widget
   guard in `render_named_region`.
11. **N3. State writes are dir-sandboxed and quota-bounded.** `rl_state_*` is
    confined to `<state_root>/<name>/` (`sanitize_relpath` rejects absolute
    paths and any `..` component) and refuses a write that would push the
    plugin's state dir over `max_state_bytes` (`check_cap`).
12. **N4. Per-plugin capability scope.** Allowlists/state root/quota come from
    that plugin instance's own `CapabilityCtx` (Extism `UserData`); one
    plugin can never use another's grants.

## Development

- **`just`** recipes: `just build`, `just test` (hermetic — no wasm toolchain
  needed), `just lint`, `just check-lock` (verifies all **six** committed
  `Cargo.lock` files — the workspace plus each of the five excluded
  `plugins/*` — are up to date, via `cargo metadata --locked`, which resolves
  without compiling and so runs in about a second; deliberately its own recipe
  rather than `--locked` bolted onto `test`/`lint`, so a local `just test`
  keeps auto-refreshing the lock as before), `just preview` (colour preview via `cargo run --`, live
  tmux context when inside tmux, else samples — needs a Nerd/powerline font for
  the glyphs), `just build-plugin NAME` (builds `plugins/<NAME>` for
  `wasm32-unknown-unknown` and installs `<NAME>.wasm` into the plugin dir —
  generic across all five example plugins, e.g. `just build-plugin cmdrun`),
  `just build-weather` (an alias: `build-plugin "weather"`), `just test-wasm`
  (opt-in: builds the weather plugin, then runs the feature-gated
  `rustline-wasm` e2e test, the bin's `wasm_wiring` test, and the bin's
  `plugin_build_wasm` test (the four `plugin build` tests that cargo-compile
  a throwaway crate for `wasm32-unknown-unknown`, moved out of `smoke.rs` so
  the default suite doesn't need that target) — needs the wasm target; `just
  test` never requires it), `just bench [ARGS]` (builds the
  weather plugin, then runs the real `rustline bench` tool via `cargo run
  --release --features bench -- bench {{ARGS}}`), `just lint-plugins` (the
  five excluded `plugins/*` are invisible to the root `cargo fmt`/`clippy`/
  `cargo test --workspace`, so this loops over them: `cargo fmt --check` +
  host clippy + a **wasm32** clippy pass — load-bearing, not redundant, since
  each plugin's guest code sits behind `#[cfg(target_arch = "wasm32")] mod
  guest`, invisible to a host-only pass), `just test-plugins` (per-plugin
  `cargo test` over the same five, 22 tests total). Both share one `plugins`
  justfile variable so the two loops can't drift out of sync when a plugin
  is added.
- Toolchain: Rust 1.97, **edition 2024** in every crate (incl. `rustline-abi`
  and the excluded `plugins/weather`, `plugins/counter`, `plugins/filewatch`,
  `plugins/httpget`, `plugins/cmdrun`); `rustfmt.toml` is edition 2024. Keep
  all crate editions equal to `rustfmt.toml`.
- `ratatui` `0.30` (default features, pulling `ratatui-crossterm` so
  `ratatui::crossterm` re-exports crossterm — no separate direct crossterm
  dependency) backs the `rustline widget edit` interactive editor
  (`widget_tui.rs`). Deliberately **not** feature-gated (unlike `bench`):
  `rustline init` emits the `prefix + W` `display-popup` binding
  unconditionally, so the subcommand it points at (`widget edit`) must
  always exist in every build, not just an opt-in one. `unicode-width` (also
  already in the tree via `ratatui-core`, so no new node) measures the
  preview strip's display width — byte or `char` length would both misjudge
  the powerline/Nerd Font glyphs the bar is made of.
- Must stay **clippy-clean** (`cargo clippy --all-targets -- -D warnings`) and
  **rustfmt-clean** (`cargo fmt --all --check`). There is **no pre-commit hook**
  in this repo — run `cargo fmt --all` yourself before committing.
- **CI** (`.github/workflows/ci.yml`, greenfield — the repo had no CI before
  this branch): runs on every pull request and on a push to `main`, cancelling
  a superseded run on the same ref. **Five** independent jobs. Four are a thin
  wrapper around an existing `just` recipe so the gate and the documented
  local command can't drift apart: `lint` → `just lint`, `test` → `just test`,
  `plugins` → `just lint-plugins` + `just test-plugins`, and `wasm-e2e` →
  `just test-wasm`. The fifth, **`macos-check`**, is the deliberate exception:
  it runs `cargo check --workspace --all-targets` on `macos-latest` directly,
  NOT through a `just` recipe, because there is no equivalent command a
  developer on Linux could run — a recipe would be a local no-op pretending to
  be a check. It exists because every other job is ubuntu-only, so nothing in
  CI ever compiled the `#[cfg(target_os = "macos")]` arms of `battery.rs`,
  `cpu.rs` (mach FFI), `memory.rs` (`sysctlbyname` FFI), `uptime.rs` or
  `daemon_service.rs` (launchd); the only macOS build anywhere was
  `release.yml`'s `aarch64-apple-darwin` leg on a `v*` tag push, so a compile
  error in a Mac-gated arm first surfaced at tag time — the same
  discovered-too-late shape `just check-lock` exists to prevent for
  `Cargo.lock`. It is check-only on purpose: it catches the compile break that
  would reach a release, and does NOT prove the FFI behaves correctly at
  runtime, which still needs a real Mac.
  The `test` job runs `just check-lock` ahead of `just test`:
  `release.yml` builds with `--locked` and nothing else did, so before this a
  stale `Cargo.lock` passed every PR check and first failed at tag-push time —
  the most expensive moment to discover it. Toolchain via
  `dtolnay/rust-toolchain` + `Swatinem/rust-cache`; `just` itself via
  `extractions/setup-just`.
  Deliberately no `rust-toolchain.toml` (would change every developer's local
  toolchain resolution) and no workspace-level `RUSTFLAGS: -D warnings`
  (redundant — `just lint`'s own clippy `-D warnings` already enforces the
  zero-warning bar). A `v*` tag push instead triggers
  `.github/workflows/release.yml` — see the Roadmap entry below for what it
  builds/publishes and README's install section for verifying a downloaded
  tarball against `SHA256SUMS`.
- **Actions are pinned to full commit SHAs** in both workflows, each with a
  trailing `# vX.Y.Z` comment, and `.github/dependabot.yml` (github-actions
  ecosystem, weekly) opens PRs to refresh them — a pin nothing refreshes is
  just a frozen unpatched version. Dependabot is deliberately **not** given a
  `cargo` entry: Rust bumps go through the `/upgrade-packages` flow, and cargo
  PRs would bury the action refreshes. One wrinkle worth knowing before
  touching those pins: **`dtolnay/rust-toolchain`'s ref selects the
  toolchain** — each branch (`stable`, `nightly`, `1.90.0`) ships an
  `action.yml` whose `toolchain` input defaults to that branch's own name — so
  its pin is the tip of the **`stable` branch**, and repointing it at a commit
  from another branch would silently change the Rust version with nothing in
  the diff to say so. It publishes no release tags, so dependabot cannot bump
  that one; refresh it by hand. Note this pins the *action code*, not the Rust
  version: `stable` still resolves to whatever stable is on the day a job runs,
  so a new stable release can still redden CI with no repo change.
- Commit `Cargo.lock` alongside any dependency change — `just check-lock` (run
  in CI's `test` job) fails the build if you forget, for the workspace lock and
  all five plugin locks alike.
- Tests are TDD unit tests in each core module (incl. the powerline renderer and
  the ANSI transcoder) plus `crates/rustline/tests/smoke.rs` integration tests.
  `rustline-wasm` adds unit tests for allow-patterns, path sandboxing, and
  quota accounting (denied-case tests are load-bearing, not just the happy
  path); the opt-in `wasm-e2e` feature gates the real-wasm end-to-end tests.
- Follows the user's global Rust defaults in `~/.claude/rust-crate-decisions.md`
  and the `rust-developer` agent (clap, serde, chrono, tracing, thiserror/anyhow).
  **rustls-only, still true with the plugin host**: `rustline-wasm`'s HTTP
  capability uses `ureq` with `default-features = false` + the `tls`/`json`
  features (rustls), and `extism` is built with `default-features = false`
  (its built-in HTTP client is deliberately dropped — `rl_http_get` and
  `rl_http_get_cached` are the only network paths). `cargo tree -i openssl` /
  `-i native-tls` stay empty across the whole graph, including
  `plugins/weather`, `plugins/counter`, `plugins/filewatch`, `plugins/httpget`,
  and `plugins/cmdrun` (`httpget` is the only other example plugin that
  touches the network at all, via the plain `rl_http_get` host fn — still
  rustls under the hood, same as `weather`'s cached path; `cmdrun` touches
  neither TLS nor the network — it spawns a subprocess). The `2.3 MB`
  dynamic binary is
  fine here — the musl/`scratch` Docker policy is for server images, not this
  local CLI. `if-addrs` (host interface enumeration for the IP widgets, in
  `crates/rustline`) is a thin syscall wrapper with no TLS involved, so it
  doesn't disturb this either.

## Roadmap

- Done: WASM plugins — a real Extism host, capability-gated network/filesystem
  access, and the `weather` example plugin, plus a host-managed TTL-cached
  fetch capability (`rl_http_get_cached`) that plugins use instead of
  hand-rolling caches.
- Done: `battery` widget — `Context.battery`/`os`/`arch`, the ninth built-in,
  and the platform-specific-read pattern (see Invariants above) that any
  future OS-specific signal should follow.
- Done: window-list rounded pill — `render_window_pill`, the six themeable
  `win_*` `Theme`/`[theme]` fields (active accent + bold, inactive gray, rounded
  `` / `` caps); the `windows` widget reduced to a text producer.
- Done: `cpu` + `memory` widgets — `Context.cpu`/`Context.memory`
  (`CpuUsage`/`MemInfo`), the tenth/eleventh built-ins and now in the
  **default** right layout; the shared `gauge_bar` Unicode block-eighths
  renderer (`widgets/bar.rs`) backing both widgets' `{bar}` placeholder;
  `read_cpu`/`read_memory` following the `read_battery` platform-read pattern.
- Done: click-to-toggle widget alt views — `alt_format` on the six
  format-bearing widgets, `Context.toggled` + the global toggles state file,
  `Widget::range_name`/`render_region_ranged`'s `#[range=user|NAME]` markup,
  the `rustline click` subcommand, `init`'s `MouseDown1Status` binding, and
  plugin participation via `Context.toggled` (the `weather` example).
- Done: themes, semantic colors, and the theme-picker CLI — seven built-in
  themes (`themes.rs`), `[theme].base` layering with themes-dir precedence,
  `ThemeConfig` as a full optional mirror of `Theme`, `Context.colors:
  ThemeColors` (`rustline-abi`) carrying the four semantic colors to widgets
  and WASM guests, threshold-aware alert badges on `cpu`/`memory`/`battery`/
  `loadavg` (`widgets/alert.rs`), and `rustline theme list|show|use|new`
  (`theme_cmd.rs`).
- Done: `rustline bench` benchmarking tool — feature-gated subcommand timing
  regions/widgets/data-sources/plugins, pure (fabricated Context) vs real-world
  passes, real preserved plugin state. See
  `docs/superpowers/specs/2026-07-21-rustline-bench-tool-design.md` /
  `docs/superpowers/plans/2026-07-21-rustline-bench-tool.md`.
- Done: `rustline init` onboarding wizard — an interactive prompt (theme with
  preview, one-/two-line status, mouse/click-to-toggle, machine-type widgets,
  clock style, refresh interval) that writes a tailored, non-destructively
  merged `config.toml` plus an idempotent marker-block upsert into
  `~/.tmux.conf` (each backed up first); `--defaults` for the same writes
  non-interactively and `--print` keeps the old raw-block-to-stdout behavior.
  See `docs/superpowers/specs/2026-07-22-rustline-init-onboarding-wizard-design.md`
  / `docs/superpowers/plans/2026-07-22-rustline-init-onboarding-wizard.md`.
- Done: `rustline theme pick` — an interactive browse-and-set command
  (`theme_cmd.rs`'s `run_picker`, reader/writer-generic and unit-tested,
  reusing `use_theme` for the write); requires a TTY. See
  `docs/superpowers/specs/2026-07-22-rustline-theme-pick-design.md` /
  `docs/superpowers/plans/2026-07-22-rustline-theme-pick.md`.
- Done: `git` widget — `Context.git`/`GitInfo` (`rustline-abi`), the twelfth
  built-in, and `crates/rustline/src/git.rs`'s platform-agnostic shell-out
  read pattern (`git status --porcelain=v2 --branch`, gated by layout like
  `cpu`/`memory`); branch/short-SHA, ahead/behind/staged/unstaged counts, and
  a `{dirty}` marker, opt-in and click-toggleable like the other
  format-bearing widgets, but NOT threshold-aware.
- Done: `disk` widget — `Context.disk`/`DiskInfo` (`rustline-abi`), the
  thirteenth built-in, and `crates/rustline/src/disk.rs`'s `statvfs(2)` read
  (POSIX, no `#[cfg(target_os)]` split needed on the syscall itself — only
  the pure `disk_info_from_statvfs` derivation is platform/test-gated),
  gated by layout like `cpu`/`memory`/`git`; used/total/avail/percent/bar for
  a configured `mount`, reusing `memory`'s `format_bytes`/`bar::gauge_bar`
  rather than duplicating them, opt-in, click-toggleable, and
  threshold-aware (`warn_percent`(85)/`crit_percent`(95)) like `cpu`/`memory`.
- Done: three more worked-example plugins — `plugins/counter`
  (`rl_state_read`/`rl_state_write`), `plugins/filewatch` (`rl_file_read`),
  and `plugins/httpget` (plain, uncached `rl_http_get`, contrasting with
  `weather`'s TTL-cached path) — rounding out the capability set `weather`
  alone didn't exercise, each also demonstrating `rl_log` on its one failure
  path, plus a generic `just build-plugin NAME` recipe (`build-weather` is
  now an alias for it).
- Done: widget-config polish — `cwd` path shortening (`max_depth`/`max_len`/
  `abbreviate`, layered onto the existing `abbreviate_home`), a `format`
  option on `hostname`/`pane_id` (previously fixed strings), and an `icon`
  override on `battery`/`cpu`/`memory` that replaces the computed glyph
  (`None` default keeps it) — every new option defaults to reproduce the
  pre-feature output byte-for-byte.
- Done: `Context::default()` (an empty, epoch-timestamped instance for
  struct-update-syntax test/synthetic construction) and `Registry`
  `WidgetDescriptor`/`WidgetSource` + `descriptors()`/`available_names()`/
  `register_described` — enumerable widget metadata without building an
  instance, not yet exposed as its own CLI subcommand.
- Done: typed WASM guest input — `rustline-abi::{WireContext, WireWindowCtx,
  GuestRender}` (chrono-free mirrors of `Context`/`WindowCtx` plus the whole
  `render` input shape), so a guest deserializes a typed struct instead of
  hand-walking `serde_json::Value`; `plugin new`'s scaffold and the
  `counter`/`filewatch`/`httpget` examples all use them.
- Done: read-gating by layout — `battery` and the IP-widgets' interface scan
  now go through the same `layout_needs` gate as `cpu`/`memory`/`git`/`disk`
  (previously always read), and `build_window_context` is a lean,
  minimal-`Context` builder (`Context.window` only, via `..Context::default()`)
  that skips every other read `render window` never needs — it no longer
  routes through `build_region_context` at all.
- Done: cpu sample cache — Linux's `read_cpu` now persists a
  `<state_root>/cpu-sample` snapshot across invocations and takes a
  zero-sleep fast path when it's fresh (within 60s), falling back to the
  classic two-sample ~120 ms read only on a cold/stale cache; see `cpu.rs`
  above (this replaces the prior "stateless two-sample delta" description).
- Done: `rustline doctor` — pass/warn/fail diagnostics for tmux ≥ 3.1, mouse
  mode, a truecolor terminal, `rustline` on `$PATH`, and the managed
  tmux-conf block, plus the resolved config/themes/plugin/log paths;
  read-only, never writes.
- Done: `rustline completions <shell>` — shell-completion scripts via
  `clap_complete`.
- Done: `rustline config path|edit|validate` — resolved-path printing,
  `$EDITOR` integration (scaffolding the file from the starter template if
  absent), and strict validation that surfaces `toml`'s own parse error
  instead of `Config::load`'s silent fallback.
- Done: `init --dry-run` (preview config.toml + the tmux block, each with a
  line diff against any existing file, writing nothing), `init --uninstall`
  (strip the managed tmux block, backing it up first), and `init --binary
  <path>` (override the binary path baked into the tmux block, default the
  running binary's own resolved absolute path) — the tmux block now calls
  that absolute path rather than a bare `rustline` (see tmux integration
  model above).
- Done: `theme new --edit` — open the freshly scaffolded theme file in
  `$EDITOR` and print the `theme use <name>` follow-up.
- Done: `rl_log` — a capability-free guest-logging host function (the one
  intentional exception to invariant N1) so a plugin can log its own
  failures through the host's `tracing` subscriber; `counter`/`filewatch`/
  `httpget` all use it on their one failure path.
- Done: plugin capability manifests + `rustline plugin approve` —
  `PluginManifest`/`resolve_manifest` (a sidecar `<name>.toml`, or an
  embedded `rustline-manifest` wasm custom section) declares a plugin's
  wanted `allowed_urls`/`allowed_paths`; `approve` turns that declaration
  into an allowlist write after confirmation (or `--yes`), never widening
  beyond what's declared. See the [design
  spec](docs/superpowers/specs/2026-07-22-rustline-whatsnext-bundle-design.md)
  / [plan](docs/superpowers/plans/2026-07-22-rustline-whatsnext-bundle.md) for
  the full 22-item, 5-phase bundle these entries summarize.
- Done (whats-next bundle #2, branch `whats-next/2026-07-23` — see the
  [design spec](docs/superpowers/specs/2026-07-23-rustline-whatsnext-bundle-2-design.md)
  / [plan](docs/superpowers/plans/2026-07-23-rustline-whatsnext-bundle-2.md)):
  - `uptime` widget (W37, the fourteenth built-in) — `Context.uptime`,
    `read_uptime` (`/proc/uptime` / `kern.boottime`), a humanized `{uptime}`.
  - `media`/now-playing widget (W41, the fifteenth built-in) —
    `Context.media`/`MediaInfo`, `read_media` via `playerctl metadata`,
    `{artist}`/`{title}`/`{status}`. Both `uptime`/`media` are opt-in and
    layout-gated; at the time deliberately NOT mirrored into `WireContext`
    (bundle #5's W54 later mirrored them in — see below).
  - datetime `timezone` (W30) — an IANA zone name via `chrono-tz`, default
    `None` = local time unchanged; an unknown name falls back to local.
  - per-widget `fg`/`bg` color override (W29) — `ColorOverride` flattened into
    every widget's table, applied centrally in `render_named_region`.
  - per-button click bindings (W36) — `ClickBinding`/`ClickBindings` +
    `Config::click_map`, and `click.rs`'s `resolve_click`/`dispatch`/
    `ClickExecutor`; `MouseDown{2,3}Status` tmux bindings (T15) so
    middle/right actually fire (they previously resolved but shipped inert).
  - ABI version negotiation (W32) — `rustline_abi::ABI_VERSION` + `abi_decision`
    (skip a mismatched guest, register a legacy one).
  - `rustline-plugin-sdk` crate (W39) — the guest-side SDK bundling typed
    capability wrappers, wire re-exports, and `export_plugin!`; all four example
    plugins migrated onto it.
  - plugin lifecycle CLI — `plugin build` (W31), `plugin run` dev harness (W34),
    `plugin install`/`update`/`remove` by `owner/repo` with recorded
    `source`/`tag`/`checksum` (W38, granting no capabilities), and `plugin
    denials` over the host's `FileDenialObserver` record (W28).
  - global `--config <path>` (W35) — override the config-file path for every
    subcommand.
  - W43 (compiled-module cache) was spiked, not implemented — a small
    `PluginBuilder::with_cache_config` seam, gated on a `build_plugin`-timing
    measurement; see the
    [feasibility note](docs/superpowers/notes/2026-07-23-w43-compiled-module-cache-feasibility.md).
    (Now implemented in bundle #3, below.)
- Done (whats-next bundle #3, branch `whats-next/2026-07-23-followups` — see the
  [design spec](docs/superpowers/specs/2026-07-23-rustline-whatsnext-bundle-3-design.md)
  / [plan](docs/superpowers/plans/2026-07-23-rustline-whatsnext-bundle-3.md)):
  - `throughput` widget (W47, the sixteenth built-in) — `Context.throughput`/
    `rustline_abi::Throughput`, `throughput::read_throughput` (`/proc/net/dev`
    counters diffed against a persisted prior sample; `None` until one exists),
    `{down}`/`{up}` rates; opt-in, layout-gated, click-toggleable, NOT
    threshold-aware; at the time NOT mirrored into `WireContext` (bundle #5's
    W54 later mirrored it in — see below).
  - `{spark}` sparkline (W45) on `cpu`/`memory` — `widgets/spark.rs`'s
    `sparkline`, `Context.cpu_history`/`mem_history` (persisted rings read at
    the build edge ONLY when `format` references `{spark}`; at the time a
    `{spark}` placed solely in `alt_format` rendered empty — bundle #5's W56
    later closed that gap, see below), and the `spark_width` option. Shared
    `history.rs`/`sample_store.rs` bin helpers back the persistence.
  - W51 — the four host-effect wire-result types (`HttpResult`/
    `CachedHttpResult`/`ReadResult`/`WriteResult`) hoisted into `rustline-abi`
    as one canonical definition; `rustline-wasm` and the SDK re-export them
    (the SDK's duplicate copy deleted).
  - W52 — the single shared `sample_context` synthetic-`Context` builder (bin;
    theme/bench/plugin-run fixtures delegate to it) and the shared
    `sample_store` best-effort per-widget state helper (reused by `throughput`
    + the sparkline history).
  - W53 — `Registry::resolve` now returns `(name, widget)` pairs; `assemble.rs`
    dropped the second `resolved_names`/`registry.contains` re-filter.
  - W43 — the wasmtime compile-cache seam in `build_plugin`
    (`PluginBuilder::with_cache_config` → `<state_root>/wasmtime-cache.toml`,
    a `[cache] directory`-only TOML with no `enabled`; best-effort/N2) plus a
    `bench` `build_plugin`-timing pass measuring the ~13× / ~45 ms-per-plugin
    cold-start compile win.
- Done (whats-next bundle #4, branch `whats-next/2026-07-23-execute` — see the
  [design spec](docs/superpowers/specs/2026-07-23-rustline-whatsnext-bundle-4-design.md)
  / [plan](docs/superpowers/plans/2026-07-23-rustline-whatsnext-bundle-4.md)):
  - W33 — `rustline init` re-run seeding + confirm: `init.rs`'s
    `seed_answers`/`clock_from_format` pre-fill the wizard's shown defaults
    from an existing `config.toml` instead of always the recommended answers,
    and `summarize_answers` + a line-diff + "Write these changes?" confirm
    gates the actual write on interactive runs (`--defaults`/`--dry-run`/
    `--print`/`--uninstall`/non-TTY unchanged). Config-only, as shipped here:
    later extended to also recover `two_line`/`mouse`/`interval` from the
    managed tmux block via `tmux_conf::parse_block_answers` (config stores
    none of the three, so seeding them from `defaults()` meant an
    Enter-through re-run rewrote a two-line bar back to one line — see
    `init.rs`/`tmux_conf.rs` above).
  - W46 — multiple widget instances: a top-level `[instances.<name>]` table
    (`Config.instances: HashMap<String, toml::Value>` at the time — since
    retyped `HashMap<WidgetName, Value>` with a closed `WidgetKind` `kind`,
    see T5/T6 in `config.rs` above — re-parsed per `kind`
    into that kind's existing Opts struct) lets any of the twelve clickable
    widget kinds appear more than once in a layout with distinct options
    (dual clocks in different timezones, multiple disk mounts, per-interface
    throughput). Each of those twelve widget structs now carries a `name`
    field (`WidgetName` as of T6) so its click-toggle/range identity is the
    *instance* name, not the
    kind (invariant #7); `Context.disks`/`Context.throughputs` (keyed by
    mount/interface) let the parameterized `disk`/`throughput` kinds serve
    distinct instances without clobbering each other, and per-interface
    throughput sample files (`throughput-sample-<iface>`) prevent the same
    for persisted state. An instance can never shadow a built-in name
    (`BUILTIN_WIDGET_NAMES`/`is_builtin_widget_name` at the time, applied
    uniformly across registration, `color_overrides()`, `click_map()`, and
    `layout_kinds` — since deleted and superseded by T5's
    `WidgetKind::parse(name).is_some()`).
  - W48 — optional persistent daemon: `rustline daemon run|status|stop`
    (`daemon.rs`/`daemon_client.rs`/`daemon_proto.rs`) keeps the parsed
    config, resolved theme, warm widget registry, and instantiated WASM
    plugins alive across tmux refreshes behind a length-prefixed-JSON Unix
    socket protocol; it reuses the exact same `build_region_context`/
    `render_named_region` path as the in-process render, so output is
    byte-identical, reloads on a config-file mtime change, and every render
    client (`render left|right|window`) tries the socket first and falls
    back to the normal in-process render on **any** failure (daemon not
    running included) — never a dependency, purely a speedup. No
    self-daemonization (`daemon run` is foreground-only; a supervisor
    backgrounds it) and no auto-spawn. `doctor` gained an advisory-only
    daemon-reachability row.
- Done (whats-next bundle #5, branch `whats-next/2026-07-24-execute` — see the
  [design spec](docs/superpowers/specs/2026-07-24-rustline-whatsnext-bundle-5-design.md)
  / [plan](docs/superpowers/plans/2026-07-24-rustline-whatsnext-bundle-5.md)):
  - W54 — `WireContext` now mirrors `throughput`/`uptime`/`media`/`cpu_history`/
    `mem_history` (purely additive; `disks`/`throughputs` stay host-only).
  - W55 — `ensure_wasmtime_cache_config` self-heals a stale `wasmtime-cache.toml`
    (compare-and-rewrite on content mismatch), guarding the N2 upgrade hazard.
  - W56 — `{spark}` history now populates when `{spark}` is in `format` OR
    `alt_format` (the prior `alt_format`-only caveat is gone).
  - W18 — `theme use`/`theme show` "unknown theme" errors now list themes-dir
    custom themes, not just built-ins.
  - W40 — `--json` on every read-only list surface (`plugin list`, `theme list`,
    `plugin url|path list`, `plugin denials`).
- Done (branch `whats-next/2026-07-24-execute-2` — see the
  [design spec](docs/superpowers/specs/2026-07-24-rustline-batched-window-render-design.md)
  / [plan](docs/superpowers/plans/2026-07-24-rustline-batched-window-render.md)):
  - W42 — batched two-line window-list render: `rustline render windows
    [--session=<s>] [--preview]` (`windows.rs`'s `read_windows` +
    `rustline-core`'s `render_windows`) shells out to `tmux list-windows -F`
    once and renders the whole window list in a single process call instead
    of tmux's own per-window `#{W:}` loop (N spawns per refresh → 1); the
    two-line `status-format[0]` now calls `#(<binary> render windows
    --session=#{q:session_name})`, re-emitting each pill's
    `#[range=window|IDX]` so click-to-select keeps working. In-process only,
    not daemon-routed (no `$TMUX` inside a systemd/launchd daemon to run
    `tmux list-windows`); one-line mode is unchanged (still per-window
    `render window`).
  - W57 — the `{spark}` history gate (`Config::spark_referenced_in_layout`)
    now scans the base `cpu`/`memory` widget OR any layout
    `[instances.<name>]` of that kind, not just the base (W56) — an
    instance-only `{spark}` now populates `Context.cpu_history`/
    `mem_history` too.
- Done (branch `feat/2026-07-24-widget-manager-exec` — see the
  [design spec](docs/superpowers/specs/2026-07-24-rustline-widget-manager-and-exec-capability-design.md)
  / [plan](docs/superpowers/plans/2026-07-24-rustline-widget-manager-and-exec-capability.md)):
  - **Widget manager (W44).** `rustline widget list|enable|disable|move|edit`
    (`widget_cmd.rs`) — a `toml_edit`-in-place CLI mirroring `plugin_cmd.rs`/
    `theme_cmd.rs`, built over `rustline-core`'s new pure layout algebra
    (`Region`, `LayoutEditError`, `LayoutChange`, `Layout::{get, get_mut,
    find}`, `layout_enable`/`layout_disable`/`layout_move`/`layout_nudge`,
    `WidgetPlacement`/`widget_placements` — see `config.rs` above) — plus
    `rustline widget edit` (`widget_tui.rs`), a full-screen `ratatui` editor
    (four columns, arrow/vim keys, `space` to add/remove, `J`/`K` to
    reorder, `w` to write, `q` to quit with an unsaved-changes confirm, `?`
    for help, a live colour preview strip via the same `render_named_region`
    pipeline, resolved into ratatui styles by `ansi.rs`'s `parse_markup`).
    `rustline init` gained a `prefix + W`
    `display-popup`-into-`widget edit` binding (tmux ≥ 3.2; `doctor` gained
    an advisory-only row for it), and `[layout].center`'s pre-existing
    inertness (nothing renders it — see the Config section above) is now
    documented and handled honestly instead of silently: the TUI refuses a
    CENTER edit, the CLI still writes one (round-tripping the default
    config) but warns.
  - **Exec capability.** Two new capability-gated host functions, `rl_exec`
    and `rl_exec_cached` (bringing the host to nine host functions total,
    eight capability-gated — see the Architecture section above), gated by a
    new `allowed_commands` allowlist matched against the **whole canonical
    argv** (`rustline_wasm::canonical_argv`, `argv.rs`) via a `Runner` seam
    (`run.rs`) mirroring `fetch.rs`'s `Fetcher` — no shell anywhere in the
    path, a 5 s timeout, 64 KiB output caps, closed stdin, and
    process-group-wide kill on timeout so a backgrounded descendant can't
    hang the render. `perform_exec_cached` caches only a zero-exit run,
    returning a non-zero exit as fresh (not stale) data. Manifests gained
    `requested_commands`; `plugin approve` prints an explicit
    execution-blast-radius warning (part of the report, so it shows under
    `--yes` too) when a manifest requests commands. New CLI: `rustline
    plugin cmd list|add|remove` (parallel to `url`/`path`); `plugin list
    [--json]` now includes `allowed_commands`. New worked example:
    `plugins/cmdrun`, the fifth example plugin (joining `weather`, `counter`,
    `filewatch`, `httpget`), demonstrating exec with a sidecar `cmdrun.toml`
    manifest.
- Done (branch `feat/2026-07-25-plugin-integrity-registry-ci` — see the
  [design spec](docs/superpowers/specs/2026-07-25-rustline-plugin-integrity-registry-ci-design.md)
  / [plan](docs/superpowers/plans/2026-07-25-rustline-plugin-integrity-registry-ci.md)):
  - **Plugin checksum verification at load time (W19).**
    `rustline-wasm::integrity` — `sha256_hex`, `ChecksumVerdict { NotRecorded,
    Match, Mismatch, Malformed }`, `verify_checksum` — plus a
    `register_plugins` gate that hashes a discovered `.wasm`'s bytes and
    checks them against `[plugins.<name>].checksum` right after the file read
    and before any `CapabilityCtx`/wasmtime involvement (invariant N1: a
    rejected plugin never receives a capability object). No recorded
    checksum loads exactly as before (fully backward compatible); a
    well-formed mismatch skips with a `warn!`; a malformed value **fails
    closed** (also skipped) rather than being treated as unpinned.
    `instantiate_named` (the `plugin run` dev harness) is deliberately
    warn-only on a bad verdict — it still runs the plugin, since that command
    is used right after rebuilding a plugin, where a recorded digest
    legitimately mismatches every build; the bar and the daemon (which
    rebuilds its registry via this same `register_plugins` on a config-file
    mtime change) always go through the real, non-bypassable gate.
    `sha256_hex`'s prior duplicate in `plugin_install.rs` is gone — the bin
    now re-exports the one definition from `rustline-wasm`, so the digest
    written at install time and the one checked at load time can never drift
    apart.
  - **Curated plugin index + `rustline plugin search` (W49).** A committed,
    schema-versioned `registry/index.json` (seeded with the five bundled
    example plugins, each `bundled: true`) plus `plugin_index.rs` (wire
    types, parse/filter/freshness logic, a 24h-TTL cache at
    `<state_root>/plugin-index.json`) backs `rustline plugin search [QUERY]
    [--json] [--refresh]` — a case-insensitive filter over name/description,
    marking already-installed entries and printing a build or install hint
    per row. A fetch failure serves the last-known-good cache with a stderr
    staleness warning; a corrupt cache, or one whose `schema_version` this
    build can't read, falls through to a fetch rather than being fatal. A new
    top-level `plugin_index_url` config key overrides the built-in default
    URL (which points at `registry/index.json` on `main`). `capabilities` in
    an index entry is advertising copy only — informational, granting
    nothing; only `plugin approve`/a hand edit ever widens an allowlist.
    Strictly read-only: `search` never writes config or the toggle state.
  - **Greenfield CI and release workflows** — not previously tracked here as
    a roadmap item, since the repo had no CI at all before this branch.
    `.github/workflows/ci.yml` runs `lint`/`test`/`plugins`/`wasm-e2e` plus
    `macos-check` as five independent jobs on every PR and `main` push; the
    first four are each routed through an existing `just` recipe, and
    `macos-check` deliberately is not (see the Development section above).
    `.github/workflows/release.yml` builds three native targets
    (`x86_64-unknown-linux-{gnu,musl}`, `aarch64-apple-darwin`) on a
    `v*` tag push and publishes a GitHub pre-release with a per-target
    tarball (binary + shell completions + README + LICENSE) plus a
    `SHA256SUMS` file. A `workflow_dispatch` trigger allows a dry run; the
    publish step is gated on a real tag push so a dispatch builds and uploads
    artifacts without creating a release. There is deliberately **no**
    `x86_64-apple-darwin` leg — GitHub's `macos-13` label no longer picks up
    jobs (a dry run sat queued indefinitely while the other legs finished),
    and since `publish` declares `needs: build`, a never-scheduled leg would
    hang the entire release rather than fail it. A **`Verify tag matches
    binary version`** step sits between Build and Package on every leg:
    tarballs are named from `GITHUB_REF_NAME` while the binary reports
    `CARGO_PKG_VERSION`, so nothing otherwise stopped a `v0.2.0` tag shipping a
    binary that identifies itself as `0.1.0`. It asks the freshly built binary
    (`"$bin" --version`) rather than re-reading `Cargo.toml`, so it checks the
    artifact actually being shipped — free, since Package already runs that
    binary to emit completions. Gated on `github.ref_type == 'tag'`, because a
    `workflow_dispatch` dry run is on a *branch* whose name is not a version
    and would otherwise fail every time.
- Per-widget richer customization; naming the widget in the panic-guard `warn!`.
- Range-on-binding — today a `run`/`open_url` click binding only fires on a
  widget that already emits a clickable range (i.e. has a non-empty
  `alt_format`); making any widget with a configured binding emit a range would
  let a `run`/`open_url` binding fire without a throwaway `alt_format`.

## Design docs

- Spec: `docs/superpowers/specs/2026-07-20-rustline-tmux-statusline-design.md`
- Plan: `docs/superpowers/plans/2026-07-20-rustline-tmux-statusline.md`
- Spec (WASM plugins): `docs/superpowers/specs/2026-07-20-rustline-wasm-plugins-design.md`
- Plan (WASM plugins): `docs/superpowers/plans/2026-07-20-rustline-wasm-plugins.md`
- Spec (IP widgets): `docs/superpowers/specs/2026-07-20-rustline-ip-widgets-design.md`
- Plan (IP widgets): `docs/superpowers/plans/2026-07-20-rustline-ip-widgets.md`
- Spec (cpu/memory widgets): `docs/superpowers/specs/2026-07-21-rustline-cpu-memory-widgets-design.md`
- Plan (cpu/memory widgets): `docs/superpowers/plans/2026-07-21-rustline-cpu-memory-widgets.md`
- Spec (click-toggle widgets): `docs/superpowers/specs/2026-07-21-rustline-click-toggle-widgets-design.md`
- Plan (click-toggle widgets): `docs/superpowers/plans/2026-07-21-rustline-click-toggle-widgets.md`
- Spec (themes/theme picker): `docs/superpowers/specs/2026-07-21-rustline-themes-theme-picker-design.md`
- Plan (themes/theme picker): `docs/superpowers/plans/2026-07-21-rustline-themes-theme-picker.md`
- Spec (whats-next bundle): `docs/superpowers/specs/2026-07-22-rustline-whatsnext-bundle-design.md`
- Plan (whats-next bundle): `docs/superpowers/plans/2026-07-22-rustline-whatsnext-bundle.md`
- Spec (whats-next bundle #2): `docs/superpowers/specs/2026-07-23-rustline-whatsnext-bundle-2-design.md`
- Plan (whats-next bundle #2): `docs/superpowers/plans/2026-07-23-rustline-whatsnext-bundle-2.md`
- Spec (whats-next bundle #3): `docs/superpowers/specs/2026-07-23-rustline-whatsnext-bundle-3-design.md`
- Plan (whats-next bundle #3): `docs/superpowers/plans/2026-07-23-rustline-whatsnext-bundle-3.md`
- Note (W43 compiled-module cache feasibility): `docs/superpowers/notes/2026-07-23-w43-compiled-module-cache-feasibility.md`
- Spec (whats-next bundle #4): `docs/superpowers/specs/2026-07-23-rustline-whatsnext-bundle-4-design.md`
- Plan (whats-next bundle #4): `docs/superpowers/plans/2026-07-23-rustline-whatsnext-bundle-4.md`
- Spec (whats-next bundle #5): `docs/superpowers/specs/2026-07-24-rustline-whatsnext-bundle-5-design.md`
- Plan (whats-next bundle #5): `docs/superpowers/plans/2026-07-24-rustline-whatsnext-bundle-5.md`
- Spec (batched window render): `docs/superpowers/specs/2026-07-24-rustline-batched-window-render-design.md`
- Plan (batched window render): `docs/superpowers/plans/2026-07-24-rustline-batched-window-render.md`
- Spec (widget manager + exec capability): `docs/superpowers/specs/2026-07-24-rustline-widget-manager-and-exec-capability-design.md`
- Plan (widget manager + exec capability): `docs/superpowers/plans/2026-07-24-rustline-widget-manager-and-exec-capability.md`
- Spec (plugin integrity, registry, CI/CD): `docs/superpowers/specs/2026-07-25-rustline-plugin-integrity-registry-ci-design.md`
- Plan (plugin integrity, registry, CI/CD): `docs/superpowers/plans/2026-07-25-rustline-plugin-integrity-registry-ci.md`
