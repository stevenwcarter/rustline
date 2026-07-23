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
  different source and call the same core unchanged. Re-exports the
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
  runtime with six capability-gated host functions (TTL-cached + raw network +
  state + arbitrary-file read/write), per-plugin allowlists and a
  sandboxed/quota-bounded state dir, and discovery of `*.wasm` files into
  `Widget` registrations. Zero ambient authority — guests run with wasi off
  and no built-in Extism HTTP/FS; every effect is host-checked. Reusable
  verbatim by a future daemon front-end.

`plugins/` holds example/third-party plugin sources, each an **excluded**
workspace member (own `Cargo.lock`, built for `wasm32-unknown-unknown`):

- `plugins/weather` — the worked example: a Nerd-Font condition icon + °F for a
  configured zip code from wttr.in, fetched via the host's TTL-cached GET
  (`rl_http_get_cached`) so it hits the network at most once per `refresh_secs`
  (the host owns the cache; the guest no longer manages its own state dir).

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
  `memory: Option<MemInfo>`, `git: Option<GitInfo>`, `os: String`, `arch:
  String`, `toggled: BTreeSet<String>`, `colors: ThemeColors`), `WindowCtx`,
  and `NetIface { name, ipv4: Ipv4Addr }` (one non-loopback IPv4 interface,
  read once at `Context`-build time; the IP widgets select from this list
  rather than touching the OS mid-render). `Battery { percent: u8, state:
  BatteryState }` and `BatteryState { Charging, Discharging, Full, Unknown }`
  (serde `snake_case`) are a battery snapshot read once at `Context`-build
  time; `CpuUsage { percent: f32 }` and `MemInfo { total_bytes, used_bytes,
  available_bytes }` (all bytes as `u64`) are the cpu/memory snapshots,
  likewise read once at `Context`-build time; `GitInfo { branch, ahead: u32,
  behind: u32, staged: u32, unstaged: u32 }` is a git branch/status snapshot,
  read once at `Context`-build time ONLY when `git` is in the active layout
  (mirroring the `cpu`/`memory` read-gating below); `os`/`arch` come from
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
  (a widget's segments plus its optional clickable range name) and
  `render_region_ranged(Direction, &[RangeGroup], &Theme) -> String`, which
  brackets each clickable group in `#[range=user|NAME]…#[norange]` with
  separators/edges kept outside any range — byte-identical to `render_region`
  when every group's range is `None`; `Theme`
  (palette, glyphs, colors, incl. the six `win_*` pill fields and the four
  `success`/`info`/`warning`/`error` semantic colors) with `Default`, plus
  `Theme::colors() -> ThemeColors` (projects the fg/bar_bg/semantic fields for
  `Context.colors`).
- `themes.rs` — `builtin_theme(name) -> Option<Theme>` and
  `builtin_theme_names() -> &[&str]`, the seven built-in themes (`default`,
  `pastel-rainbow`, `nord`, `gruvbox`, `catppuccin-mocha`, `tokyo-night`,
  `dracula`); each
  is a complete `Theme` (all fields, incl. semantics), and every non-`default`
  theme is multi-accent (`palette.len() >= 4`) using truecolor (`Color::Rgb`).
  See Themes under Config for the full list and layering rules.
- `widget.rs` — `Widget` trait and `Registry` (name → factory; `resolve` skips
  unknown widget names with a `warn!`, never errors). `Widget::range_name(&self)
  -> Option<&str>` defaults to `None`; a clickable widget returns `Some(name)`.
- `widgets/` — the twelve built-ins: `pane_id`, `hostname`, `windows`, `cwd`,
  `loadavg`, `datetime`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`, `memory`,
  `git`, plus `Registry::with_builtins(&Config)` in `mod.rs`. `net.rs` is the pure
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
  both `cpu.rs` and `memory.rs`. `cpu.rs` is the `cpu` widget: pure over
  `Context.cpu`, with an nf-md-chip `{icon}`, `{percent}`, and `{bar}`
  placeholders, plus `warn_percent`(80)/`crit_percent`(95) threshold config
  (via `alert_over`). `memory.rs` is the `memory` widget: pure over
  `Context.memory`, with an nf-md-memory `{icon}`, `{used}`/`{total}`/
  `{avail}` (human-readable binary sizes via `format_bytes`, e.g. `6.2G`),
  `{percent}`, and `{bar}` placeholders, plus `warn_percent`(80)/
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
  `substitute` scanner does the replacement. All four threshold-aware widgets
  render byte-identically to before this feature whenever no tier is
  crossed (a reading below every threshold, or a tier disabled via `0`).
  `git.rs` is the `git` widget: pure over `Context.git`, with `{branch}`
  (current branch, or the 7-char short SHA when `HEAD` is detached),
  `{ahead}`/`{behind}`/`{staged}`/`{unstaged}` (counts), and `{dirty}`
  (substitutes a configurable `dirty_glyph`, default `*`, iff `staged > 0 ||
  unstaged > 0`, else empty) placeholders; NOT threshold-aware (no
  `alert.rs` use) and NOT in the default layout.
  `toggle.rs` holds the shared click-toggle helpers
  `active_format(ctx, name, format, alt) -> &str`
  (picks `alt` iff it's non-empty AND `name` is in `ctx.toggled`, else
  `format`) and `clickable_range(name, alt) -> Option<&str>` (`Some(name)` iff
  `alt` is non-empty AND `name.len() <= 15`, tmux's `range=user|X` byte limit);
  the eight format-bearing widgets (`datetime`, `lan_ip`, `tailscale_ip`,
  `battery`, `cpu`, `memory`, `loadavg`, `git`) each carry an `alt_format`
  field and call both helpers from their `render`/`range_name`.
- `assemble.rs` — `assign_palette`, `render_named_region` (panic-guarded per
  widget via `catch_unwind`; now range-wraps via `render_region_ranged`,
  remembering each widget's `range_name()` across the palette-assignment
  flatten/regroup), `render_window` (wraps the `windows` text in a
  themed rounded pill via `render.rs::render_window_pill`, keyed on
  `ctx.window.is_current`; still `catch_unwind`-guarded → `""` on panic/no
  window; the window pill is never clickable — `render window` has no
  `--plugin-dir` and no range wrapping).
- `config.rs` — `Config` (TOML): `layout`, `theme`, `widgets`, a top-level
  `plugin_dir: Option<String>`, and a typed `plugins: HashMap<String,
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
  first and then emit the `"invalid config"` warning into the file.
- `ansi.rs` — `tmux_to_ansi(&str) -> String`: transcodes the tmux markup we emit
  into ANSI SGR (`colourN` → 256-color, `#rrggbb` → truecolor, named → basic)
  for the `--preview` flag.

`rustline-abi`:
- `lib.rs` — `Segment { text, style }`, `Style { fg, bg, bold }`,
  `Color { Named | Indexed(u8) | Rgb(u8,u8,u8) }` (+ `Color::to_tmux()`),
  `ThemeColors { fg, bar_bg, success, info, warning, error }` with `Default`
  (matches `Theme::default()`'s values) — the semantic-color snapshot carried
  on `Context.colors` so widgets and WASM guests can style alert badges
  without seeing `Theme` — and `GitInfo { branch, ahead, behind, staged,
  unstaged }` (chrono-free, so no separate wire mirror is needed; the same
  type rides `Context.git` and `WireContext.git`). The WASM wire types,
  re-exported by `rustline-core`.

`rustline-wasm`:
- `allow.rs` — `AllowSet`/`Pattern`: each `allowed_urls`/`allowed_paths` entry
  is a glob by default or a regex when prefixed `re:`; deny-by-default (empty
  set matches nothing); malformed patterns are logged and skipped.
- `state.rs` — `sanitize_relpath` (rejects absolute/`..` paths for state I/O),
  `normalize_abs` (rejects `..` for arbitrary-file I/O), `dir_size`/`check_cap`
  (state-dir quota accounting via `walkdir`).
- `paths.rs` — `expand_tilde`, `data_root`, `state_root`, `default_plugin_dir`
  (all under `$XDG_DATA_HOME/rustline`, falling back to `$HOME/.local/share/rustline`).
- `abi.rs` — the host↔guest wire types (`HttpResult`, `CachedHttpResult`,
  `ReadResult`, `WriteResult`, `RenderInput`) and `parse_render_output`
  (malformed JSON → empty `Vec`).
- `cache.rs` — pure HTTP-response-cache helpers: FNV-1a URL→path, RFC3339
  freshness (`age_secs`/`is_fresh`), quota-bounded `read_entry`/`write_entry`.
- `capability.rs` — `CapabilityCtx`: one plugin instance's allowlists, state
  root, and quota, built from `PluginConfig` and held in Extism `UserData` so
  each instance only ever sees its own grants.
- `fetch.rs` — `Fetcher` trait + `UreqFetcher` (the real rustls blocking HTTP
  client); the trait seam makes `perform_http_get`'s gating logic testable
  without a network.
- `perform.rs` — the six capability-checked effect functions
  (`perform_http_get`, `perform_http_get_cached` — the TTL-cached GET:
  gate-first, 2xx-only caching, serve-stale — `perform_state_read/write`,
  `perform_file_read/write`); pure enough to unit-test directly, incl. the
  denied-case tests.
- `host.rs` — the `host_fn!` wrappers binding `perform_*` (incl.
  `rl_http_get_cached`) to each plugin's `CapabilityCtx`, `build_plugin`
  (Extism instantiation: wasi off, fuel + timeout + memory caps), and
  `WasmWidget` (wraps an `extism::Plugin`; `Widget::render` degrades to empty
  segments on any error/timeout/malformed output; carries its own `name` and
  implements `range_name` as `Some(name)` iff `name.len() <= 15` — the guest
  itself decides whether to honor `context.toggled`).
- `lib.rs::register_plugins` — discovers `*.wasm` in the plugin dir, and for
  each name in the caller's `needed` list (i.e. actually referenced by a
  layout region — avoids paying wasm cold-start for unused plugins):
  instantiates it, verifies the exported `name()` equals the filename stem
  (mismatch → `warn!` + skip), and registers a `WasmWidget` factory. A stem
  colliding with a built-in is skipped (built-in wins). A stem longer than 15
  bytes gets a one-time `warn!` (not click-toggleable) but still registers.

`plugins/weather` (excluded workspace member, `wasm32-unknown-unknown`):
- `lib.rs` — pure logic (`code_to_icon`, `render_format`, `parse_wttr`,
  `select_weather_format` — the click-toggle exemplar: prefers a non-empty
  `options.alt_format` when the guest's `render` sees its own name, `"weather"`,
  in `context.toggled`) compiled and unit-tested on the host target, plus a
  `#[cfg(target_arch = "wasm32")] mod guest` with the Extism `name`/`render`
  exports and a single `rl_http_get_cached` guest import (the host owns the
  TTL cache).

`rustline` (bin):
- `cli.rs` — `clap` derive. `render`, `plugin`, and `theme` are subcommand
  *groups* (`ThemeCmd { List, Show { name }, Use { name }, Pick, New { name,
  from, force } }`); `init` (`InitArgs { defaults, print }`, both plain flags) is the
  onboarding-wizard subcommand (see CLI below); `click` (`ClickArgs { range,
  button }`, both defaulted so an empty click is a parseable no-op) is a flat
  subcommand invoked by the tmux mouse binding.
- `battery.rs` — `read_battery()`, a `#[cfg(target_os)]` read surface (one of
  three — see `cpu.rs`/`memory.rs` below): a Linux sysfs
  (`/sys/class/power_supply/*/{capacity,status}`) arm and a macOS
  `pmset -g batt` arm, each delegating to a pure parser (`parse_linux`/
  `parse_pmset`) that is `#[cfg(any(target_os = …, test))]`-compiled so both
  are unit-tested on the Linux dev box even though only one reader arm
  compiles per platform. Any other platform, or a failed read, yields `None`.
- `cpu.rs` — `read_cpu()`, a `#[cfg(target_os)]` read surface: Linux takes two
  `/proc/stat` samples ~120 ms apart (`CPU_SAMPLE_WINDOW`) and diffs the
  aggregate `cpu ` line (`parse_proc_stat` + `busy_percent`, a stateless
  two-sample delta — no cross-invocation state); macOS shells out to
  `top -l 2 -n 0` and parses the last `CPU usage:` line (`parse_top_cpu`).
  Both parsers are `#[cfg(any(target_os = …, test))]`-compiled and unit-tested
  on the Linux dev box. Unsupported platform or failed read → `None`.
- `memory.rs` — `read_memory()`, a `#[cfg(target_os)]` read surface: Linux
  reads `/proc/meminfo` (`MemTotal`/`MemAvailable` in kB, `parse_meminfo`);
  macOS shells out to `sysctl -n hw.memsize` + `vm_stat` and derives available
  bytes from free/inactive/speculative pages at the reported page size
  (`parse_macos_memory`). Same cfg-gated pure-parser pattern as
  `battery.rs`/`cpu.rs`. Unsupported platform or failed read → `None`.
- `git.rs` — `read_git(path) -> Option<GitInfo>`, a platform-agnostic (no
  `#[cfg(target_os)]`) shell-out read: runs `git -C <path> status
  --porcelain=v2 --branch`, `None` on ANY failure (`git` missing, non-repo,
  non-zero exit). Delegates to the pure `parse_git_status(&str) -> GitInfo`
  (unconditionally unit-tested, no cfg-gating needed since there's no OS
  branching) — same pure-parser-behind-the-read-surface shape as
  `battery.rs`/`cpu.rs`/`memory.rs`, just keyed on tool availability rather
  than platform.
- `build_context.rs` — builds `Context` from args + `gethostname`,
  `libc::getloadavg` (the only `unsafe`, guarded on `n == 3`), `chrono::Local`,
  `$HOME`, non-loopback IPv4 interfaces via `if-addrs` into
  `Context.interfaces` (a failed read yields an empty `Vec`, never a
  fabricated address — same spirit as `read_loadavg` returning `None`), and
  now also `battery` (via `battery::read_battery()`), `cpu` (via
  `cpu::read_cpu()`), `memory` (via `memory::read_memory()`), `git` (via
  `git::read_git(&pane_current_path)`, gated: only read when `git` is in the
  region's layout, mirroring the `cpu`/`memory` gate), `os`, `arch`
  (from `std::env::consts::OS`/`ARCH`), and `toggled` (via
  `toggles::read_toggles()`, unconditionally — cheap relative to the gated
  cpu/memory/git reads).
- `toggles.rs` — the global click-toggle state file:
  `toggles_path()` (`$XDG_DATA_HOME/rustline/toggles`, reusing
  `rustline_wasm::data_root()`), `parse_toggles`/`serialize_toggles`
  (newline-delimited, total over blanks/whitespace), `apply_toggle` (flips
  membership), `read_toggles` (missing/unreadable file → empty set), and
  `write_toggles` (best-effort atomic temp-file + rename; a write failure
  `warn!`s and never panics — a broken toggle must never break the bar).
- `plugin_cmd.rs` — `rustline plugin …`: `list` reads the effective `Config`;
  `url|path add/remove` mutate the config file in place via `toml_edit`
  (preserving comments/formatting), creating `[plugins.<name>]` if absent.
- `theme_cmd.rs` — `rustline theme …`, mirroring `plugin_cmd.rs`'s `toml_edit`
  approach: `list` prints every built-in and themes-dir `*.toml` stem,
  marking the active one (`cfg.theme.base`, default `"default"`) with `*` and
  a built-in **shadowed** by a same-named file; `show <name>` resolves
  (file-first, then built-in), builds a synthetic `Context` engineered to
  trip warning+error badges, renders the default layout, and prints ANSI via
  `tmux_to_ansi`; `use <name>` validates `<name>` resolves, then sets
  `[theme].base = "<name>"` in the config file via `toml_edit` (refuses to
  write on an unknown name or unparseable existing config); `new <name>
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
  `mouse`, `interval`): the tmux config block `rustline init` emits, incl. a
  `bind -T root MouseDown1Status` block (see CLI below); one-line/mouse-off/
  interval-1 output stays byte-identical to the pre-wizard block. `two_line`
  additionally emits `set -g status 2` plus the author's verbatim two-line
  `status-format[0]`/`[1]` (window list on its own line). `TMUX_BEGIN`/
  `TMUX_END` (`# >>> rustline >>>` / `# <<< rustline <<<`) and
  `upsert_tmux_block(existing, block) -> String`: idempotently insert/replace
  the rustline-managed region in an existing `~/.tmux.conf`, leaving
  surrounding user content untouched.
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
  prompt loop, erroring on non-TTY stdin without a flag). `assets/
  starter-config.toml` (embedded via `include_str!`) is the recommended
  starter template `init.rs` mutates — the shortened `alt_format`s and
  curated layout it seeds live only here, **not** in any widget's code
  `Default` (those stay `""`/unchanged; see Config below).
- `logging.rs` — `init(&LogConfig, verbose)`: installs the two-sink `tracing`
  subscriber (rotated file + stderr), plus the pure helpers `verbosity_to_level`,
  `parse_level`, `resolve_file_level`/`resolve_stderr_level`, `should_rotate`,
  `open_log`, `log_path`. Best-effort: a file that can't be opened degrades to
  stderr-only; never writes stdout.
- `main.rs` — dispatch + the `emit(markup, preview)` helper (raw markup vs
  ANSI) + `resolve_plugin_dir` (`--plugin-dir` flag › config `plugin_dir` ›
  `rustline_wasm::default_plugin_dir()`). Only `render left`/`render right`
  discover and register plugins; `render window` is built-ins only.
  `run_click` handles `Command::Click`: a no-op unless `button == "left"` and
  `range` is non-empty, else flips `range`'s membership via
  `toggles::{read,apply,write}_toggles` — the single choke point for click
  dispatch, so a future `left_click`/`right_click` script-handler mechanism
  extends resolution here rather than adding parallel dispatch elsewhere.
  `themes_dir()` resolves `$XDG_CONFIG_HOME/rustline/themes` (fallback
  `~/.config/rustline/themes`), parallel to `config_path()`; `resolve_theme(&Config)
  -> Theme` is the file-aware layering used by `render`/`init` (`Theme::default()`
  → `resolve_base_theme` → inline `[theme]` overrides via `to_theme_over`), and
  `resolve_base_theme(name) -> Option<Theme>` (now `pub(crate)` so `init.rs` can
  resolve the wizard's chosen theme into `bar_bg`/`fg` for the tmux block)
  resolves a base name themes-dir-file first, then built-in (an
  invalid/missing file falls through to the built-in lookup with a `warn!`) —
  this is where a user's themes-dir file **shadows** a same-named built-in
  (see Themes below). `tmux_conf_path()` resolves the user's tmux config
  (`$HOME/.tmux.conf`), parallel to `config_path()`/`themes_dir()`; `Command::
  Init` dispatches straight to `init::run` with all four resolved paths plus
  the already-resolved `theme`.
- `bench/` (`#[cfg(feature = "bench")]`) — the `rustline bench` subcommand:
  `harness.rs` (`summarize`/`measure`/`Stats`/`Row`/`Group`), `fixture.rs`
  (`fabricated_context` — the pure-pass mock seam), `render_passes.rs` (pure
  widget + pure/real region passes), `sources.rs` (per-read timing + the
  source registry), `plugins.rs` (real-preserved-state plugin timing), and
  `report.rs` (comfy-table pretty/markdown). Gated behind the `bench` cargo
  feature; the default binary is unchanged.

## CLI

A global `-v`/`--verbose` (repeatable) raises the **file** log level:
`-v`=warn, `-vv`=info, `-vvv`=debug, `-vvvv`=trace. Works in any position
(`rustline -vv render left`).

- `rustline render left|right [--session= --window= --pane= --pane-path=] [--preview] [--plugin-dir=]`
- `rustline render window [--current] --index= [--name=] [--flags=] [--preview]`
  (no `--plugin-dir` — windows don't run plugins in v1)
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
  silently. `rustline init --defaults` runs the same two writes
  non-interactively with recommended answers. `rustline init --print` is the
  legacy behavior: prints just the raw one-line tmux block to stdout (using
  `theme.bar_bg`/`fg` for `status-style`) and writes nothing.
- `rustline print-config` — effective config as TOML.
- `rustline plugin list` — discovered/configured plugins with their source,
  allowlists, and state quota.
- `rustline plugin url|path list|add|remove <plugin> [pattern]` — read or
  edit a plugin's `allowed_urls`/`allowed_paths` (`add`/`remove` rewrite the
  config file in place via `toml_edit`, preserving comments/formatting).
- `rustline theme list` — every built-in + themes-dir theme, marking the
  active one and any built-in shadowed by a same-named file.
- `rustline theme show <name>` — ANSI preview of `<name>` (default layout,
  synthetic Context tuned to show warning/error alert badges).
- `rustline theme use <name>` — set `[theme].base = "<name>"` in the config
  file (`toml_edit`, comment-preserving); errors without writing if `<name>`
  doesn't resolve.
- `rustline theme new <name> [--from <seed>] [--force]` — scaffold
  `<themes_dir>/<name>.toml` as a complete, tweakable copy of `<seed>`
  (default `default`); refuses to overwrite an existing file without `--force`.
- `rustline theme pick` — interactively list the themes (active marked,
  themes-dir files tagged `(custom)`), preview any by number (or `a`/`all` for
  every one), then prompt to set one by name or number (blank keeps the
  current theme), writing `[theme].base` via the same path as `theme use`.
  Previews default to a healthy bar (palette only); `t` toggles the
  warning/error alert-badge colors on/off. Requires a terminal — a non-TTY
  invocation prints a hint toward `theme show`/`theme use` and exits non-zero
  without writing.
- `rustline click --range=<name> [--button=left]` — flip `<name>`'s membership
  in the global toggle state file; invoked by the `init`-emitted tmux mouse
  binding. Only `left` acts today; other button values are reserved for a
  future `left_click`/`right_click` script-handler mechanism.
- `rustline bench [--only regions|widgets|sources|plugins|all] [--iters N]
  [--real-iters N] [--warmup N] [--cold] [--format table|markdown]
  [--output FILE] [--plugin-dir DIR] [--state-dir DIR]` — feature-gated
  (`--features bench`) benchmark of the render pipeline: a pure pass
  (fabricated `Context`, no reads) vs a real-world pass (real reads + render),
  plus per-widget, per-read, and per-plugin timing. Plugin passes run against
  real preserved state so cached fast-paths are measured honestly.

`--plugin-dir` overrides plugin discovery for that invocation (see Config
below for the full resolution order).

`--preview` prints a region in ANSI colour on the terminal (for manual
verification) instead of raw tmux markup; without it, stdout is the raw markup
tmux consumes (stdout is the status line — logs always go to stderr).

## tmux integration model

Shell-out per region on `status-interval` (no daemon in v1). The block
`rustline init` writes (via `init_block`) wires `status-left`/`status-right`/
`window-status-format` to `#(rustline render …)` and adds
`after-select-pane`/`after-select-window` → `refresh-client -S` hooks for
instant updates. It also emits a `bind -T root MouseDown1Status` block: a
window-name click still runs the default `select-window`, and any other
non-empty `#{mouse_status_range}` runs `rustline click --range=… --button=left`
then `refresh-client -S`. This requires **tmux ≥ 3.1** (that's when
`range=user|X` status ranges and the `mouse_status_range` format variable were
added) and, at the tmux-config level, `set -g mouse on` — the wizard's mouse
question (`InitAnswers.mouse`) can add that setter for you (`--print` never
does; it always emits the mouse-off, one-line legacy block regardless of
config).

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
`"%a < %Y-%m-%d < %H:%M"` (the `<` are literal). Unknown widget names in a layout
are skipped, not fatal.

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
charging-aware Nerd-Font glyph), `{percent}`, and `{state}` placeholders, and
a `down_format` (default `""`, i.e. render nothing) shown when
`Context.battery` is `None` (no battery, unsupported platform, or a failed
read) — same collapse-placeholders-to-empty behavior as the IP widgets'
`down_format`. It's also **threshold-aware** (see Themes below):
`warn_percent`/`crit_percent` (default 20/10) alert while discharging at or
below those levels.

```toml
[widgets.battery]
format = "{icon} {percent}%"
down_format = ""
warn_percent = 20   # default; badge at/below this % while discharging
crit_percent = 10   # default; 0 disables a tier
```

**CPU and memory widgets:** `cpu` and `memory` are in the **default** right
layout (unlike the opt-in IP/battery widgets above). `cpu` takes a `format`
(default `"{icon} {percent}%"`) with `{icon}` (nf-md-chip), `{percent}`, and
`{bar}` (a `bar_width`-cell Unicode block-eighths gauge, default 8)
placeholders. `memory` takes a `format` (default `"{icon} {used}/{total}"`)
with `{icon}` (nf-md-memory), `{used}`/`{total}`/`{avail}` (human-readable
binary sizes, e.g. `6.2G`), `{percent}`, and `{bar}` (`bar_width`, default 8)
placeholders. Both take a `down_format` (default `""`, i.e. render nothing)
shown when the platform read failed or is unsupported — same
collapse-placeholders-to-empty behavior as `battery`'s `down_format`. Both are
also **threshold-aware** (see Themes below): `warn_percent`/`crit_percent`
(cpu default 80/95, memory default 80/92) alert at or above those levels.

```toml
[widgets.cpu]
format = "{icon} {bar} {percent}%"   # default "{icon} {percent}%"
bar_width = 8
down_format = ""
warn_percent = 80   # default; 0 disables a tier
crit_percent = 95   # default

[widgets.memory]
format = "{icon} {used}/{total}"     # default; or "{icon} {bar} {percent}%"
bar_width = 8
down_format = ""
warn_percent = 80   # default; 0 disables a tier
crit_percent = 92   # default
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

**Click-to-toggle widget views:** the eight format-bearing widgets —
`datetime`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`, `memory`, `loadavg`,
`git` — each take an additional `alt_format` (default `""`, `#[serde(default)]`, so
covered by invariant #3 like every other opt). A non-empty `alt_format` makes
that widget clickable: left-clicking it in the tmux status line toggles it
between `format` and `alt_format`.

```toml
[widgets.cpu]
format     = "{icon} {percent}%"
alt_format = "{icon} {bar} {percent}%"   # left-click toggles to this
```

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
becomes a tmux `range=user|<name>` argument verbatim.

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
The four threshold-aware widgets (`cpu`, `memory`, `battery`, `loadavg` — see
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
`$XDG_DATA_HOME/rustline/plugins`, `~/` expanded) plus a typed
`[plugins.<name>]` table per plugin, keyed by the plugin's name (the `.wasm`
filename stem):

```toml
plugin_dir = "~/.local/share/rustline/plugins"   # optional

[plugins.weather]
source = "steve/rustline-weather"          # optional provenance note
allowed_urls = ["https://wttr.in/*"]        # glob, or "re:<pattern>" for regex
allowed_paths = []
max_state_bytes = 52428800                  # default: 50 MB

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

**Logging:** a `[log]` table controls the two sinks. `rustline` logs to a
rotated file (`$XDG_DATA_HOME/rustline/rustline.log`, default level `info`) and
to stderr (default level `error`). `RUST_LOG` is **not** consulted. Raise the
*file* level with repeated `-v` (`-v`=warn, `-vv`=info, `-vvv`=debug,
`-vvvv`=trace); `-v` never affects stderr. The file rotates to `rustline.log.1`
once it passes 5 MiB (one generation kept). Any level value is `off|error|warn|
info|debug|trace` and is parsed leniently (a typo falls back to the default).

    [log]
    file_level   = "info"
    stderr_level = "error"
    file         = "~/.local/share/rustline/rustline.log"   # optional override

## Invariants (load-bearing — re-check when touching these)

1. **`Context` is the sole render input.** Widgets read only from `Context`,
   never the environment mid-render (keeps the daemon/WASM path viable). `cwd`
   reads `ctx.home`, not `$HOME`.
2. **`Segment`/`Context`/`Style`/`Color` stay serde-serializable** — this is
   the WASM ABI. `Segment`/`Style`/`Color` now live in `rustline-abi`
   (re-exported by `rustline-core`); `Context`/`WindowCtx` stay in
   `rustline-core` (they carry `chrono`).
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
   toggleable — there's no error, just a click that does nothing.

**Platform-specific reads stay at the `Context`-build edge.** `read_battery()`
(`crates/rustline/src/battery.rs`), `read_cpu()` (`crates/rustline/src/cpu.rs`),
and `read_memory()` (`crates/rustline/src/memory.rs`) are the three
`#[cfg(target_os)]` surfaces in the codebase; each OS arm (Linux sysfs/`/proc`,
macOS `pmset`/`top`/`sysctl`+`vm_stat`) delegates to a pure parser
(`parse_linux`/`parse_pmset`, `parse_proc_stat`/`parse_top_cpu`,
`parse_meminfo`/`parse_macos_memory`) that is `#[cfg(any(target_os = …,
test))]`-compiled, so all of them are unit-tested on the Linux dev box even
though only one reader arm per module compiles per platform. Follow this
pattern for any future OS-specific signal. `Context.os`/`Context.arch` (from
`std::env::consts::OS`/`ARCH`) are now available for WASM guests that want to
branch on platform.

**WASM plugin invariants (added by the plugin system — re-check when touching
`rustline-wasm` or `plugins/*`):**

8. **N1. Zero ambient authority.** A guest runs with `with_wasi(false)` and no
   Extism built-in HTTP/FS; every network/filesystem effect goes through a
   host function that checks the plugin's `CapabilityCtx` first. Adding a new
   host capability means adding its gate *and* a denied-case test. The
   TTL-cached GET (`rl_http_get_cached`) gates `allowed_urls` before any fetch
   (gate-first: a denied URL makes no network call and touches no cache),
   with its own denied-case test.
9. **N2. A plugin never breaks the bar.** Any instantiation error, render
   error, timeout, or malformed output degrades to empty segments
   (`WasmWidget::render`), bounded by fuel + wall-clock timeout + memory caps.
   This composes with, not replaces, the existing `catch_unwind` per-widget
   guard in `render_named_region`.
10. **N3. State writes are dir-sandboxed and quota-bounded.** `rl_state_*` is
    confined to `<state_root>/<name>/` (`sanitize_relpath` rejects absolute
    paths and any `..` component) and refuses a write that would push the
    plugin's state dir over `max_state_bytes` (`check_cap`).
11. **N4. Per-plugin capability scope.** Allowlists/state root/quota come from
    that plugin instance's own `CapabilityCtx` (Extism `UserData`); one
    plugin can never use another's grants.

## Development

- **`just`** recipes: `just build`, `just test` (hermetic — no wasm toolchain
  needed), `just lint`, `just preview` (colour preview via `cargo run --`, live
  tmux context when inside tmux, else samples — needs a Nerd/powerline font for
  the glyphs), `just build-weather` (builds `plugins/weather` for
  `wasm32-unknown-unknown` and installs `weather.wasm` into the plugin dir),
  `just test-wasm` (opt-in: builds the weather plugin, then runs the
  feature-gated `rustline-wasm` e2e test and the bin's `wasm_wiring` test —
  needs the wasm target; `just test` never requires it), `just bench [ARGS]`
  (builds the weather plugin, then runs the real `rustline bench` tool via
  `cargo run --release --features bench -- bench {{ARGS}}`).
- Toolchain: Rust 1.97, **edition 2024** in every crate (incl. `rustline-abi`
  and the excluded `plugins/weather`); `rustfmt.toml` is edition 2024. Keep all
  crate editions equal to `rustfmt.toml`.
- Must stay **clippy-clean** (`cargo clippy --all-targets -- -D warnings`) and
  **rustfmt-clean** (`cargo fmt --all --check`). There is **no pre-commit hook**
  in this repo — run `cargo fmt --all` yourself before committing.
- Commit `Cargo.lock` alongside any dependency change.
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
  `plugins/weather`. The `2.3 MB` dynamic binary is
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
- Historical sparkline (last-X-seconds graph) for `cpu`/`memory` — today's
  reads are single-shot, stateless snapshots; a sparkline needs
  cross-invocation sample persistence, deferred to its own spec.
- Plugin auto-download by `owner/repo` (today, `source` is just a config note;
  installing a plugin means putting the `.wasm` in the plugin dir yourself).
- An interactive capability-approval flow (config/CLI allowlist edits are
  manual for now).
- Guest-side logging of state-write failures (currently silent in the guest;
  the host already logs its side).
- Optionally moving `Context`/`WindowCtx` into `rustline-abi` for a fully
  typed guest input (today the guest parses the JSON `Context` by hand).
- Optional daemon front-end for sub-second / push-driven widgets (the pure core
  and the wasm host are already daemon-ready).
- Per-widget richer customization; naming the widget in the panic-guard `warn!`.
- `left_click`/`right_click` script handlers — today only a left click on a
  widget acts (toggling `alt_format`); `ClickArgs.button` already threads
  through other button values for this to extend into later.
- A widget-management TUI/popup (enable/disable/reorder layout widgets,
  writing `config.toml`) — parked in `TODO.md`; distinct from this feature's
  transient click-toggle view state.

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
