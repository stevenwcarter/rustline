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
  `memory: Option<MemInfo>`, `os: String`, `arch: String`), `WindowCtx`, and
  `NetIface { name, ipv4: Ipv4Addr }` (one non-loopback IPv4 interface, read
  once at `Context`-build time; the IP widgets select from this list rather
  than touching the OS mid-render). `Battery { percent: u8, state:
  BatteryState }` and `BatteryState { Charging, Discharging, Full, Unknown }`
  (serde `snake_case`) are a battery snapshot read once at `Context`-build
  time; `CpuUsage { percent: f32 }` and `MemInfo { total_bytes, used_bytes,
  available_bytes }` (all bytes as `u64`) are the cpu/memory snapshots,
  likewise read once at `Context`-build time; `os`/`arch` come from
  `std::env::consts::OS`/`ARCH`.
- `render.rs` — `render_region(Direction, &[Segment], &Theme) -> String`, the
  load-bearing powerline renderer (hard `` `` / soft `` `` separators, edge
  blending to `bar_bg`); `render_window_pill(text, is_current, &Theme) ->
  String`, the window-list rounded-pill renderer (rounded `` / `` caps colored
  `fg=pill,bg=bar_bg` — the *opposite* of a pointed separator); `Theme`
  (palette, glyphs, colors, incl. the six `win_*` pill fields) with `Default`.
- `widget.rs` — `Widget` trait and `Registry` (name → factory; `resolve` skips
  unknown widget names with a `warn!`, never errors).
- `widgets/` — the eleven built-ins: `pane_id`, `hostname`, `windows`, `cwd`,
  `loadavg`, `datetime`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`, `memory`,
  plus `Registry::with_builtins(&Config)` in `mod.rs`. `net.rs` is the pure
  LAN/Tailscale interface-selection and `{ip}` formatting logic shared by
  `lan_ip`/`tailscale_ip` (no I/O — operates on `Context.interfaces`).
  `battery.rs` is the `battery` widget: pure over `Context.battery`, with a
  level-bucketed, charging-aware Nerd-Font `{icon}` plus `{percent}`/`{state}`
  placeholders. `bar.rs` is `pub(crate) fn gauge_bar(fraction: f64, width:
  usize) -> String`, a shared pure Unicode block-eighths gauge (full `█`
  cells, one sub-cell partial, `░` track) used by the `{bar}` placeholder in
  both `cpu.rs` and `memory.rs`. `cpu.rs` is the `cpu` widget: pure over
  `Context.cpu`, with an nf-md-chip `{icon}`, `{percent}`, and `{bar}`
  placeholders. `memory.rs` is the `memory` widget: pure over
  `Context.memory`, with an nf-md-memory `{icon}`, `{used}`/`{total}`/
  `{avail}` (human-readable binary sizes via `format_bytes`, e.g. `6.2G`),
  `{percent}`, and `{bar}` placeholders. `windows.rs` is the `windows` widget:
  it emits only the window **text** (`{index}{flags} {name}`); the pill
  styling and active/inactive colors are applied downstream by the
  theme-aware renderer (widgets can't see the `Theme`).
- `assemble.rs` — `assign_palette`, `render_named_region` (panic-guarded per
  widget via `catch_unwind`), `render_window` (wraps the `windows` text in a
  themed rounded pill via `render.rs::render_window_pill`, keyed on
  `ctx.window.is_current`; still `catch_unwind`-guarded → `""` on panic/no
  window).
- `config.rs` — `Config` (TOML): `layout`, `theme`, `widgets`, a top-level
  `plugin_dir: Option<String>`, and a typed `plugins: HashMap<String,
  PluginConfig>` table (see Config below). `Config::load` is **total**
  (missing/invalid file → `warn!` + defaults); `to_theme()` maps overrides onto
  `Theme::default()`. `Config::load_reporting` returns the load-failure
  message instead of logging it, so the binary can install its log subscriber
  first and then emit the `"invalid config"` warning into the file.
- `ansi.rs` — `tmux_to_ansi(&str) -> String`: transcodes the tmux markup we emit
  into ANSI SGR (`colourN` → 256-color, `#rrggbb` → truecolor, named → basic)
  for the `--preview` flag.

`rustline-abi`:
- `lib.rs` — `Segment { text, style }`, `Style { fg, bg, bold }`,
  `Color { Named | Indexed(u8) | Rgb(u8,u8,u8) }` (+ `Color::to_tmux()`). The
  WASM wire types, re-exported by `rustline-core`.

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
  segments on any error/timeout/malformed output).
- `lib.rs::register_plugins` — discovers `*.wasm` in the plugin dir, and for
  each name in the caller's `needed` list (i.e. actually referenced by a
  layout region — avoids paying wasm cold-start for unused plugins):
  instantiates it, verifies the exported `name()` equals the filename stem
  (mismatch → `warn!` + skip), and registers a `WasmWidget` factory. A stem
  colliding with a built-in is skipped (built-in wins).

`plugins/weather` (excluded workspace member, `wasm32-unknown-unknown`):
- `lib.rs` — pure logic (`code_to_icon`, `render_format`, `parse_wttr`)
  compiled and unit-tested on the host target, plus a
  `#[cfg(target_arch = "wasm32")] mod guest` with the Extism `name`/`render`
  exports and a single `rl_http_get_cached` guest import (the host owns the
  TTL cache).

`rustline` (bin):
- `cli.rs` — `clap` derive. `render` and `plugin` are subcommand *groups*.
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
- `build_context.rs` — builds `Context` from args + `gethostname`,
  `libc::getloadavg` (the only `unsafe`, guarded on `n == 3`), `chrono::Local`,
  `$HOME`, non-loopback IPv4 interfaces via `if-addrs` into
  `Context.interfaces` (a failed read yields an empty `Vec`, never a
  fabricated address — same spirit as `read_loadavg` returning `None`), and
  now also `battery` (via `battery::read_battery()`), `cpu` (via
  `cpu::read_cpu()`), `memory` (via `memory::read_memory()`), `os`, and `arch`
  (from `std::env::consts::OS`/`ARCH`).
- `plugin_cmd.rs` — `rustline plugin …`: `list` reads the effective `Config`;
  `url|path add/remove` mutate the config file in place via `toml_edit`
  (preserving comments/formatting), creating `[plugins.<name>]` if absent.
- `tmux_conf.rs` — `init_block(bar_bg, fg)`: the tmux config `rustline init`
  emits.
- `logging.rs` — `init(&LogConfig, verbose)`: installs the two-sink `tracing`
  subscriber (rotated file + stderr), plus the pure helpers `verbosity_to_level`,
  `parse_level`, `resolve_file_level`/`resolve_stderr_level`, `should_rotate`,
  `open_log`, `log_path`. Best-effort: a file that can't be opened degrades to
  stderr-only; never writes stdout.
- `main.rs` — dispatch + the `emit(markup, preview)` helper (raw markup vs
  ANSI) + `resolve_plugin_dir` (`--plugin-dir` flag › config `plugin_dir` ›
  `rustline_wasm::default_plugin_dir()`). Only `render left`/`render right`
  discover and register plugins; `render window` is built-ins only.

## CLI

A global `-v`/`--verbose` (repeatable) raises the **file** log level:
`-v`=warn, `-vv`=info, `-vvv`=debug, `-vvvv`=trace. Works in any position
(`rustline -vv render left`).

- `rustline render left|right [--session= --window= --pane= --pane-path=] [--preview] [--plugin-dir=]`
- `rustline render window [--current] --index= [--name=] [--flags=] [--preview]`
  (no `--plugin-dir` — windows don't run plugins in v1)
- `rustline init` — prints the tmux config block (uses `theme.bar_bg`/`fg` for
  `status-style`).
- `rustline print-config` — effective config as TOML.
- `rustline plugin list` — discovered/configured plugins with their source,
  allowlists, and state quota.
- `rustline plugin url|path list|add|remove <plugin> [pattern]` — read or
  edit a plugin's `allowed_urls`/`allowed_paths` (`add`/`remove` rewrite the
  config file in place via `toml_edit`, preserving comments/formatting).

`--plugin-dir` overrides plugin discovery for that invocation (see Config
below for the full resolution order).

`--preview` prints a region in ANSI colour on the terminal (for manual
verification) instead of raw tmux markup; without it, stdout is the raw markup
tmux consumes (stdout is the status line — logs always go to stderr).

## tmux integration model

Shell-out per region on `status-interval` (no daemon in v1). `rustline init`
wires `status-left`/`status-right`/`window-status-format` to `#(rustline render …)`
and adds `after-select-pane`/`after-select-window` → `refresh-client -S` hooks
for instant updates.

**Injection safety (critical):** tmux expands `#{…}` inside `#(…)` *before*
`/bin/sh -c` and does not shell-escape. So the `init` block passes every tmux var
as `--flag=#{q:VAR}` — the `#{q:}` modifier escapes it and the `--flag=` form is
empty-safe. Never emit a bare `'#{window_name}'` or `'#{pane_current_path}'`.
This is why `render window` takes named args, not positional. See `tmux_conf.rs`.

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
`down_format`.

```toml
[widgets.battery]
format = "{icon} {percent}%"
down_format = ""
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
collapse-placeholders-to-empty behavior as `battery`'s `down_format`.

```toml
[widgets.cpu]
format = "{icon} {bar} {percent}%"   # default "{icon} {percent}%"
bar_width = 8
down_format = ""

[widgets.memory]
format = "{icon} {used}/{total}"     # default; or "{icon} {bar} {percent}%"
bar_width = 8
down_format = ""
```

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

7. **N1. Zero ambient authority.** A guest runs with `with_wasi(false)` and no
   Extism built-in HTTP/FS; every network/filesystem effect goes through a
   host function that checks the plugin's `CapabilityCtx` first. Adding a new
   host capability means adding its gate *and* a denied-case test. The
   TTL-cached GET (`rl_http_get_cached`) gates `allowed_urls` before any fetch
   (gate-first: a denied URL makes no network call and touches no cache),
   with its own denied-case test.
8. **N2. A plugin never breaks the bar.** Any instantiation error, render
   error, timeout, or malformed output degrades to empty segments
   (`WasmWidget::render`), bounded by fuel + wall-clock timeout + memory caps.
   This composes with, not replaces, the existing `catch_unwind` per-widget
   guard in `render_named_region`.
9. **N3. State writes are dir-sandboxed and quota-bounded.** `rl_state_*` is
   confined to `<state_root>/<name>/` (`sanitize_relpath` rejects absolute
   paths and any `..` component) and refuses a write that would push the
   plugin's state dir over `max_state_bytes` (`check_cap`).
10. **N4. Per-plugin capability scope.** Allowlists/state root/quota come from
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
  needs the wasm target; `just test` never requires it).
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

## Design docs

- Spec: `docs/superpowers/specs/2026-07-20-rustline-tmux-statusline-design.md`
- Plan: `docs/superpowers/plans/2026-07-20-rustline-tmux-statusline.md`
- Spec (WASM plugins): `docs/superpowers/specs/2026-07-20-rustline-wasm-plugins-design.md`
- Plan (WASM plugins): `docs/superpowers/plans/2026-07-20-rustline-wasm-plugins.md`
- Spec (IP widgets): `docs/superpowers/specs/2026-07-20-rustline-ip-widgets-design.md`
- Plan (IP widgets): `docs/superpowers/plans/2026-07-20-rustline-ip-widgets.md`
- Spec (cpu/memory widgets): `docs/superpowers/specs/2026-07-21-rustline-cpu-memory-widgets-design.md`
- Plan (cpu/memory widgets): `docs/superpowers/plans/2026-07-21-rustline-cpu-memory-widgets.md`
