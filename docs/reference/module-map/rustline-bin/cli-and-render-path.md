# `rustline` bin — CLI surface and the render path

> **Read when:** changing argument parsing, `Context` construction and its read gates, toggle state, click dispatch, or top-level dispatch in `main.rs`.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

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

- `click.rs` — click resolution + dispatch (W36): `resolve_click(&Config,
  range, button) -> ClickAction` (`{Toggle, OpenUrl, Run, NoOp}`, pure over the
  config) and `dispatch(action, range, &impl ClickExecutor)`. The
  `ClickExecutor` seam (`RealExecutor` spawns detached `sh -c`/`xdg-open`; a
  recording fake in tests) keeps resolve+dispatch unit-tested without spawning.
  Default (no binding) is byte-identical to the pre-W36 toggle behavior; the
  only text `sh -c` ever sees comes from the user's own config, never the tmux
  `range` value (invariant #4).

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
