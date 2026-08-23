# History — shipped bundles and branches

> **Read when:** you need the rationale behind a multi-item bundle (whats-next #2–#5, batched window render, widget manager + exec, plugin integrity/registry/CI, standalone plugin repos).
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- Done (whats-next bundle #2, branch `whats-next/2026-07-23` — see the
  [design spec](../superpowers/specs/2026-07-23-rustline-whatsnext-bundle-2-design.md)
  / [plan](../superpowers/plans/2026-07-23-rustline-whatsnext-bundle-2.md)):
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
    [feasibility note](../superpowers/notes/2026-07-23-w43-compiled-module-cache-feasibility.md).
    (Now implemented in bundle #3, below.)
- Done (whats-next bundle #3, branch `whats-next/2026-07-23-followups` — see the
  [design spec](../superpowers/specs/2026-07-23-rustline-whatsnext-bundle-3-design.md)
  / [plan](../superpowers/plans/2026-07-23-rustline-whatsnext-bundle-3.md)):
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
  [design spec](../superpowers/specs/2026-07-23-rustline-whatsnext-bundle-4-design.md)
  / [plan](../superpowers/plans/2026-07-23-rustline-whatsnext-bundle-4.md)):
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
  [design spec](../superpowers/specs/2026-07-24-rustline-whatsnext-bundle-5-design.md)
  / [plan](../superpowers/plans/2026-07-24-rustline-whatsnext-bundle-5.md)):
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
  [design spec](../superpowers/specs/2026-07-24-rustline-batched-window-render-design.md)
  / [plan](../superpowers/plans/2026-07-24-rustline-batched-window-render.md)):
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
  [design spec](../superpowers/specs/2026-07-24-rustline-widget-manager-and-exec-capability-design.md)
  / [plan](../superpowers/plans/2026-07-24-rustline-widget-manager-and-exec-capability.md)):
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
  [design spec](../superpowers/specs/2026-07-25-rustline-plugin-integrity-registry-ci-design.md)
  / [plan](../superpowers/plans/2026-07-25-rustline-plugin-integrity-registry-ci.md)):
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
- Done (branch `feat/2026-07-30-standalone-plugin-repos` — see the
  [design spec](../superpowers/specs/2026-07-30-standalone-plugin-repos-design.md)
  / [plan](../superpowers/plans/2026-07-30-standalone-plugin-repos.md)):
  **Four standalone `rustline-*` plugin repos as the out-of-tree exemplar** —
  `rustline-updates` (pending package updates, exec + state TTL),
  `rustline-pubip` (public/WAN IP, TTL-cached HTTP), `rustline-kube`
  (Kubernetes context/namespace, gated file read), and `rustline-ticker`
  (CoinGecko coin price, TTL-cached HTTP), each its own repo (scaffolded via
  `rustline plugin new`, `rustline-plugin-sdk` pulled as a git dependency
  pinned to a `v0.1.0` tag rather than a workspace path dep, its own CI, and
  a tag-triggered release publishing the installable `<name>.wasm`) —
  released as real `v0.1.0` GitHub releases and installed/exercised
  end-to-end against this repo's own `plugin install`/`approve`/`run`. Three
  small fixes in this repo made the pattern actually work: `plugin_install.rs`'s
  `do_install` now defaults the installed name to the release **asset's own
  stem** rather than the repo name (so a `rustline-<name>` repo releasing
  `<name>.wasm` installs findably under `<name>`, matching the plugin's
  exported `name()` — see `plugin_install.rs` above); each example embeds its
  `manifest.toml` in the `.wasm` via `#[unsafe(link_section =
  "rustline-manifest")]` rather than shipping a sidecar file, since a sidecar
  doesn't survive `plugin install`'s single-asset download (`manifest.rs`'s
  embedded-section fallback path, previously untested against a real
  standalone repo, is now exercised for real); and `plugin new`'s scaffolded
  `Cargo.toml` template gained a commented-out git-dependency line showing the
  out-of-tree form (`rustline-plugin-sdk = { git = "…", tag = "v0.1.0" }`)
  alongside the default in-workspace path dep. `registry/index.json` gained
  the four as `bundled: false` entries (see `plugin_index.rs` above), and
  README gained a "Writing an out-of-tree plugin" section pointing at all
  four as worked examples.
