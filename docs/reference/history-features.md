# History — individually shipped features

> **Read when:** you need the rationale behind one of the early standalone features (widgets, themes, bench, wizard, plugin manifests).
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

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
  spec](../superpowers/specs/2026-07-22-rustline-whatsnext-bundle-design.md)
  / [plan](../superpowers/plans/2026-07-22-rustline-whatsnext-bundle.md) for
  the full 22-item, 5-phase bundle these entries summarize.
