# rustline whats-next bundle — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Rust-writing tasks are dispatched to the `rust-developer` agent (model **sonnet** by default; **opus** for the tasks marked `[opus]`: T3, T12, T21).

**Goal:** Ship the 22 selected whats-next opportunities (spec:
`docs/superpowers/specs/2026-07-22-rustline-whatsnext-bundle-design.md`) as
TDD'd, clippy/fmt-clean Rust on one branch, without regressing any behavior or
invariant.

**Architecture:** Five dependency-ordered phases on `feature/whats-next-bundle`:
(1) foundational refactors, (2) widgets, (3) perf, (4) CLI, (5) WASM ecosystem.
A targeted full code review runs at each phase boundary (orchestrator-run, not a
user pause). New system signals are read at the Context-build edge; new wire
types keep the WASM JSON byte-identical.

**Tech Stack:** Rust edition 2024, cargo workspace; clap (derive) + `toml_edit` +
`clap_complete`; serde/serde_json; chrono; tracing; extism (wasmtime) host;
`libc` (existing); ureq/rustls. `just` recipes.

## Global Constraints

- Edition 2024 in every crate incl. new plugin members; keep all editions == `rustfmt.toml`.
- New excluded plugin members carry an empty `[workspace]` table.
- Clippy-clean (`cargo clippy --all-targets -- -D warnings`) + rustfmt-clean (`cargo fmt --all --check`).
- rustls-only / openssl-free: `cargo tree -i openssl` and `-i native-tls` stay empty.
- Commit `Cargo.lock` with any dependency change (T14 adds `clap_complete`).
- Every new config field is `#[serde(default)]` (invariant #3: `Config::load` stays total).
- Context is the sole render input (invariant #1); new signals read once at build edge.
- `Segment`/`Context`/`Style`/`Color` stay serde-serializable; W26 keeps wire JSON byte-identical (invariant #2).
- `init` output stays injection-safe: `--flag=#{q:VAR}` (invariant #4).
- Platform reads: `#[cfg(target_os)]` surface + `#[cfg(any(target_os=…, test))]` pure parser, unit-tested on the Linux dev box.
- WASM invariants N1–N4: zero ambient authority, a plugin never breaks the bar, sandboxed+quota state, per-plugin capability scope.
- `just test` stays hermetic (no wasm toolchain); wasm-only tests stay behind the opt-in `wasm-e2e` feature / `just test-wasm`.
- When adding a widget/plugin, sync widget/plugin lists in **both** CLAUDE.md and README.md (final doc task T24, and each widget task notes its own doc line).
- Run `cargo fmt --all` before each commit; run `just test` + `just lint` before each phase-boundary review.

---

# Phase 1 — Foundational refactors

### Task 1 (W3): `Context::default()` + churn-site migration

**Files:**
- Modify: `crates/rustline-core/src/context.rs` (add `impl Default for Context`)
- Modify (test fixtures → `..Default::default()`): `crates/rustline-core/src/context.rs` (`tests::sample`), `crates/rustline-core/src/widget.rs` (`tests::ctx`), `crates/rustline-core/src/assemble.rs` (test ctxs), `crates/rustline-wasm/src/host.rs` (test ctx), `crates/rustline/src/theme_cmd.rs` (synthetic `Context`s used by `theme show`/picker sampling)
- Leave explicit: `crates/rustline/src/build_context.rs`

**Interfaces:**
- Produces: `impl Default for Context` — `now` = `Local.timestamp_opt(0,0).single().unwrap()`, all `String` empty, all `Option` `None`, `interfaces` empty `Vec`, `toggled`/`colors` default.

**Steps:**
- [ ] Step 1: Write failing test `default_context_is_empty_and_epoch` in `context.rs` asserting `Context::default().now.timestamp() == 0`, `session_name.is_empty()`, `battery.is_none()`, `interfaces.is_empty()`.
- [ ] Step 2: Run `cargo test -p rustline-core context::` — expect FAIL (no `Default`).
- [ ] Step 3: Implement `impl Default for Context`.
- [ ] Step 4: Migrate the listed test/synthetic construction sites to `Context { <overrides>, ..Default::default() }`. Do NOT touch `build_context.rs`.
- [ ] Step 5: `cargo test` (workspace) + `cargo clippy --all-targets -- -D warnings` + `cargo fmt --all` — all green, existing serde round-trips still pass.
- [ ] Step 6: Commit `feat(core): add Context::default() and use it at churn-prone construction sites (W3)`.

### Task 2 (W26a): Move chrono-free nested types to `rustline-abi`

**Files:**
- Modify: `crates/rustline-abi/src/lib.rs` (add `BatteryState`, `Battery`, `MemInfo`, `CpuUsage`, `NetIface` — verbatim definitions moved from `context.rs`, incl. `#[serde(rename_all="snake_case")]` on `BatteryState`)
- Modify: `crates/rustline-core/src/context.rs` (delete the moved defs; `pub use rustline_abi::{Battery, BatteryState, MemInfo, CpuUsage, NetIface};`)
- Modify: `crates/rustline-core/src/segment.rs` if it is the canonical re-export module — extend it or re-export from `context.rs`/`lib.rs` so `rustline_core::Battery` etc. keep resolving (mirror the existing `Segment`/`Style`/`Color` re-export precedent)
- Verify compile across `crates/rustline` (`battery.rs` constructs `Battery`/`BatteryState`; `build_context.rs`; the platform reads) — import paths via the re-export must keep working unchanged.

**Interfaces:**
- Produces: `rustline_abi::{Battery, BatteryState, MemInfo, CpuUsage, NetIface}`, re-exported as `rustline_core::…` (unchanged call sites).
- `NetIface.ipv4: std::net::Ipv4Addr` stays (std, serde-string — no chrono).

**Steps:**
- [ ] Step 1: Move the five type defs into `rustline-abi/src/lib.rs`; add the re-export in `rustline-core`.
- [ ] Step 2: `cargo build --workspace` — resolve any path breakage via re-exports only (no call-site edits beyond imports).
- [ ] Step 3: Keep existing `context.rs` serde tests (`battery`/`cpu`/`memory`/`interfaces` survive serde) green — they now exercise the abi types through `Context`.
- [ ] Step 4: Add abi-local unit test `battery_state_serializes_snake_case` (moved/duplicated) to keep the snake_case contract pinned in abi.
- [ ] Step 5: `cargo test` + clippy + fmt.
- [ ] Step 6: Commit `refactor(abi): move Battery/MemInfo/CpuUsage/NetIface to rustline-abi, re-export from core (W26)`.

### Task 3 (W26b) `[opus]`: `WireContext` + guest migration + round-trip seam test

**Files:**
- Modify: `crates/rustline-abi/src/lib.rs` — add `WireWindowCtx`, `WireContext` (field-for-field mirror of `WindowCtx`/`Context` with `now: String`; every other field the same, referencing the abi nested types; `#[serde(default)]` on `toggled`/`colors`; **include `git`/`disk` fields once T8/T9 add them — for now mirror the current Context fields**), and `GuestRender { context: WireContext, config: serde_json::Value }` (`#[derive(Deserialize)]`).
- Modify: `crates/rustline-wasm/src/abi.rs` — add the round-trip seam test module.
- Modify: `plugins/weather/src/lib.rs` — the `guest::render` deserializes `GuestRender`/`WireContext` instead of `serde_json::Value` hand-walking for `now` and `toggled` (keep `config` as `Value` for plugin-specific keys).

**Interfaces:**
- Consumes: the abi nested types from Task 2.
- Produces: `rustline_abi::WireContext { session_name, window_index, pane_index, pane_current_path, home, hostname, loadavg: Option<[f64;3]>, now: String, window: Option<WireWindowCtx>, interfaces: Vec<NetIface>, battery: Option<Battery>, cpu: Option<CpuUsage>, memory: Option<MemInfo>, os: String, arch: String, #[serde(default)] toggled: BTreeSet<String>, #[serde(default)] colors: ThemeColors }`; `rustline_abi::GuestRender`.

**Steps:**
- [ ] Step 1: Write the failing seam test in `rustline-wasm/src/abi.rs`: build a representative `Context` (via `Context::default()` + overrides incl. non-empty `now`, `battery`, `interfaces`, `toggled`, `colors`), `let json = serde_json::to_string(&ctx)`, `let wire: rustline_abi::WireContext = serde_json::from_str(&json).unwrap()`, and assert each field round-trips — `wire.now` parses (`DateTime::parse_from_rfc3339`) back to `ctx.now`, `wire.battery == ctx.battery`, `wire.interfaces == ctx.interfaces`, `wire.toggled == ctx.toggled`, `wire.colors == ctx.colors`, `wire.session_name == ctx.session_name`.
- [ ] Step 2: Run it — expect FAIL (no `WireContext`).
- [ ] Step 3: Add `WireWindowCtx`/`WireContext`/`GuestRender` to abi. Add an abi-local unit test that `WireContext` deserializes a representative literal JSON and that JSON omitting `toggled`/`colors` defaults them.
- [ ] Step 4: Run the seam test + abi tests — expect PASS.
- [ ] Step 5: Migrate `plugins/weather` `guest::render`: parse `GuestRender`; `now` = `input.context.now`; `toggled` = `input.context.toggled.contains("weather")`; config keys stay via `input.config`. Keep the pure `select_weather_format`/`parse_wttr` unit tests unchanged.
- [ ] Step 6: `cargo test` (host-side, hermetic) + clippy + fmt. Confirm the on-wire JSON is unchanged (the host still serializes `Context`; the seam test proves `WireContext` parses it).
- [ ] Step 7: Commit `feat(abi): typed WireContext for WASM guests; migrate weather guest; byte-identical wire (W26)`.

### Task 4 (W22): Registry widget enumeration + descriptors

**Files:**
- Modify: `crates/rustline-core/src/widget.rs` — `WidgetDescriptor`, `WidgetSource`, ordered `Vec<WidgetDescriptor>` in `Registry`, `register` back-compat, `register_described`, `descriptors()`, `available_names()`.
- Modify: `crates/rustline-core/src/widgets/mod.rs` — `with_builtins` uses `register_described` with a one-line summary + `configurable` flag for each of the 11 built-ins.
- Modify: `crates/rustline-wasm/src/lib.rs` — plugin registration records a `WidgetDescriptor { source: Plugin, summary: "WASM plugin", configurable: true }`.

**Interfaces:**
- Produces: `pub struct WidgetDescriptor { pub name: String, pub summary: String, pub configurable: bool, pub source: WidgetSource }`, `pub enum WidgetSource { Builtin, Plugin }`, `Registry::descriptors(&self) -> &[WidgetDescriptor]`, `Registry::available_names(&self) -> impl Iterator<Item = &str>`, `Registry::register_described(&mut self, desc: WidgetDescriptor, factory: Factory)`. `register(name, factory)` keeps working (derives a minimal Builtin descriptor).

**Steps:**
- [ ] Step 1: Write failing tests in `widget.rs`: `descriptors_list_registrations_in_order` (register a, b → descriptors names == [a,b]); `register_backcompat_minimal_descriptor` (`register` yields a descriptor with `name==name`, `configurable==false`, `source==Builtin`); a `with_builtins` test that `descriptors()` contains all 11 built-in names and that `configurable` is true for `cpu`/`datetime` and false for `pane_id`.
- [ ] Step 2: Run — expect FAIL.
- [ ] Step 3: Implement the descriptor storage + accessors + `register_described`; wire `with_builtins`; wire plugin registration.
- [ ] Step 4: Add a `rustline-wasm` test that a registered plugin's descriptor has `source == Plugin`.
- [ ] Step 5: `cargo test` + clippy + fmt; existing `resolve`/`contains`/`build` tests unchanged.
- [ ] Step 6: Commit `feat(core): Registry widget enumeration + descriptors (W22)`.

### Phase 1 review checkpoint
- [ ] Orchestrator runs a targeted full review (requesting-code-review, adversarially verified) over T1–T4: ABI byte-identical guarantee, `Default` sentinel correctness, registry back-compat. Fix loop until clean. **Do not pause for the user.**

---

# Phase 2 — Widgets

### Task 5 (W4): `format` on hostname + pane_id

**Files:**
- Modify: `crates/rustline-core/src/widgets/hostname.rs`, `pane_id.rs`
- Modify: `crates/rustline-core/src/config.rs` (add `format` to hostname/pane_id opts — create the opts structs if they don't exist yet, `#[serde(default)]`)
- Modify: `crates/rustline-core/src/widgets/mod.rs` (factories thread `format`)

**Interfaces:**
- Produces: `hostname` honors `{host}`; `pane_id` honors `{session}`/`{window}`/`{pane}`. Defaults reproduce current output byte-identical (read the current render first and set the default `format` to match exactly).

**Steps:**
- [ ] Step 1: Characterization tests — `hostname_default_format_matches_current` and `pane_id_default_format_matches_current` (assert current output for a sample Context), then `hostname_custom_label_prepends` / `pane_id_custom_format`.
- [ ] Step 2: Run — new custom-format tests FAIL.
- [ ] Step 3: Add `format` fields (default = current output) + pure substitution (unknown placeholders pass through).
- [ ] Step 4: Run — PASS; defaults unchanged.
- [ ] Step 5: clippy + fmt.
- [ ] Step 6: Commit `feat(widgets): format string on hostname and pane_id (W4)`.

### Task 6 (W1 + cwd W4): cwd shortening + format

**Files:**
- Modify: `crates/rustline-core/src/widgets/cwd.rs`, `crates/rustline-core/src/config.rs` (`CwdOpts`: add `format` default `"{path}"`, `max_depth: usize` default 0, `max_len: usize` default 0, `abbreviate: bool` default false; keep `abbreviate_home`), `widgets/mod.rs` (factory).

**Interfaces:**
- Consumes: nothing new.
- Produces: `cwd` renders `format` with `{path}` after applying: home-abbrev (existing) → `abbreviate` (fish-style, all but last component → first char) → `max_depth` (keep last N components, prefix `…/`) → `max_len` (left-truncate with leading `…`). Defaults leave output byte-identical.

**Steps:**
- [ ] Step 1: Tests: `cwd_default_unchanged` (characterization); `cwd_abbreviate_shortens_components`; `cwd_max_depth_keeps_last_n_with_ellipsis`; `cwd_max_len_left_truncates`; `cwd_format_wraps_path`.
- [ ] Step 2: Run — FAIL for new behavior.
- [ ] Step 3: Implement the pure transforms + format substitution in `cwd.rs`; extend `CwdOpts`.
- [ ] Step 4: Run — PASS; default characterization holds.
- [ ] Step 5: clippy + fmt.
- [ ] Step 6: Commit `feat(widgets): cwd path shortening (max_depth/max_len/abbreviate) + format (W1)`.

### Task 7 (W14): configurable icon glyphs for cpu/memory/battery

**Files:**
- Modify: `crates/rustline-core/src/widgets/cpu.rs`, `memory.rs`, `battery.rs`, `config.rs` (add `icon: Option<String>` `#[serde(default)]` to each opts), `widgets/mod.rs`.

**Interfaces:**
- Produces: when `icon: Some(s)`, `{icon}` renders `s`; `None` = current computed glyph (battery: overriding replaces the whole bucketed/charging glyph).

**Steps:**
- [ ] Step 1: Tests: `cpu_icon_override_replaces_glyph`, `memory_icon_override_replaces_glyph`, `battery_icon_override_replaces_bucketed_glyph`, and `*_icon_none_uses_default` characterizations.
- [ ] Step 2: Run — override tests FAIL.
- [ ] Step 3: Add the `icon` field + override logic in each widget's `{icon}` substitution.
- [ ] Step 4: Run — PASS.
- [ ] Step 5: clippy + fmt.
- [ ] Step 6: Commit `feat(widgets): configurable icon glyph for cpu/memory/battery (W14)`.

### Task 8 (W12): git widget

**Files:**
- Create: `crates/rustline/src/git.rs` (`read_git(path) -> Option<GitInfo>` shell-out; pure `parse_git_status(&str) -> GitInfo`)
- Modify: `crates/rustline-abi/src/lib.rs` (`GitInfo`), `crates/rustline-core/src/context.rs` (re-export `GitInfo`, add `pub git: Option<GitInfo>` to `Context`), `crates/rustline-abi` `WireContext` (add `git: Option<GitInfo>`), `crates/rustline/src/build_context.rs` (call `read_git(&ctx.pane_current_path)` — gated by layout in T10; for now read when `git` in layout), `crates/rustline/src/main.rs`/`lib.rs` mod decl
- Create: `crates/rustline-core/src/widgets/git.rs` (widget), Modify: `config.rs` (`GitOpts`), `widgets/mod.rs` (register with descriptor)

**Interfaces:**
- Produces: `rustline_abi::GitInfo { branch: String, ahead: u32, behind: u32, staged: u32, unstaged: u32 }`; `Context.git`/`WireContext.git`; `git` widget with `{branch}`/`{ahead}`/`{behind}`/`{staged}`/`{unstaged}`/`{dirty}` (configurable `dirty_glyph` default `*`), `format` default `" {branch}{dirty}"`, `down_format` default `""`, `alt_format` (clickable, name `git` ≤15 bytes).

**Steps:**
- [ ] Step 1: Tests for `parse_git_status` over `git status --porcelain=v2 --branch` fixtures: clean-on-branch, ahead 2/behind 1, staged+unstaged counts, detached HEAD (no branch → `branch` empty or `"HEAD"`). Widget tests: each placeholder; `down_format` when `Context.git == None`; default-format characterization.
- [ ] Step 2: Run — FAIL.
- [ ] Step 3: Implement `parse_git_status` (pure, `#[cfg(any(unix, test))]`-safe), `read_git` (shell-out `git -C <path> status --porcelain=v2 --branch`, `None` on any failure), `GitInfo` in abi + Context/WireContext fields, the widget, config, registration.
- [ ] Step 4: Run — PASS. Ensure `cargo tree -i openssl` stays empty (no new dep).
- [ ] Step 5: Update the T3 seam test if `git` field addition needs it (it will now round-trip `git`); clippy + fmt.
- [ ] Step 6: Add the `git` widget line to CLAUDE.md + README.md widget lists (this task's doc obligation).
- [ ] Step 7: Commit `feat(widgets): git branch/status widget via git shell-out (W12)`.

### Task 9 (W20): disk widget

**Files:**
- Create: `crates/rustline/src/disk.rs` (`read_disk(mount) -> Option<DiskInfo>` via `libc::statvfs`; pure derivation)
- Modify: `crates/rustline-abi/src/lib.rs` (`DiskInfo`), `context.rs` (re-export + `pub disk: Option<DiskInfo>`), `WireContext` (add `disk`), `build_context.rs` (gated read, `mount` from config, default `/`), mod decl
- Create: `crates/rustline-core/src/widgets/disk.rs`, Modify: `config.rs` (`DiskOpts`: `mount` default `/`, `format` default `" {used}/{total}"`, `bar_width` default 8, `down_format`, `warn_percent` 85, `crit_percent` 95, `alt_format`), `widgets/mod.rs`

**Interfaces:**
- Consumes: `format_bytes`, `gauge_bar`, `alert.rs` helpers.
- Produces: `rustline_abi::DiskInfo { total_bytes, used_bytes, available_bytes }`; `Context.disk`/`WireContext.disk`; `disk` widget with `{used}`/`{total}`/`{avail}`/`{percent}`/`{bar}`/`{mount}`, threshold-aware via `alert_over`.

**Steps:**
- [ ] Step 1: Tests: pure `statvfs`-fields→`DiskInfo` derivation (fabricated `f_blocks`/`f_bfree`/`f_bavail`/`f_frsize`); widget render (bytes format, bar, percent); warn/crit badge; below-threshold no badge; `down_format` when `None`.
- [ ] Step 2: Run — FAIL.
- [ ] Step 3: Implement `read_disk` + pure derivation (`#[cfg(any(target_os="linux", target_os="macos", test))]`), `DiskInfo` in abi + Context/WireContext, widget, config, registration.
- [ ] Step 4: Run — PASS; extend the T3 seam test to round-trip `disk`.
- [ ] Step 5: clippy + fmt; `cargo tree -i openssl` empty.
- [ ] Step 6: Add `disk` to CLAUDE.md + README.md widget lists.
- [ ] Step 7: Commit `feat(widgets): disk-usage widget via statvfs (W20)`.

### Phase 2 review checkpoint
- [ ] Targeted full review over T5–T9: byte-identical defaults for touched widgets, the two new platform reads follow the cfg pattern, seam test now covers `git`/`disk`. Fix loop until clean. **No user pause.**

---

# Phase 3 — Perf

### Task 10 (W5): gate battery/interface/git/disk reads by layout

**Files:**
- Modify: `crates/rustline/src/build_context.rs`

**Interfaces:**
- Produces: a pure predicate helper, e.g. `fn layout_needs(layout: &[String], name: &str) -> bool`, used to gate `read_interfaces` (lan_ip/tailscale_ip), `read_battery` (battery), `read_git` (git), `read_disk` (disk). Mirrors the existing cpu/memory gating. `loadavg` stays ungated.

**Steps:**
- [ ] Step 1: Test `layout_needs` (true when name present, false when absent) and that `build_region_context` with a battery-less/ip-less layout yields `battery: None`, `interfaces: []`.
- [ ] Step 2: Run — FAIL.
- [ ] Step 3: Extract/implement `layout_needs`; wrap the four reads.
- [ ] Step 4: Run — PASS.
- [ ] Step 5: clippy + fmt.
- [ ] Step 6: Commit `perf(context): gate battery/interface/git/disk reads by layout (W5)`.

### Task 11 (W8): lean window-context path

**Files:**
- Modify: `crates/rustline/src/build_context.rs` (`build_window_context` builds a minimal `Context` via `Context::default()` + only pill-relevant fields), `crates/rustline/src/main.rs` if the window dispatch calls the region builder.

**Interfaces:**
- Consumes: `Context::default()` (T1). Produces: `build_window_context` no longer reads battery/interfaces/loadavg/toggles/hostname.

**Steps:**
- [ ] Step 1: Verify which fields `render_window`/`render_window_pill` consume (read the code). Test: `build_window_context` sets `window` + `colors` (+ any consumed field) and leaves battery/interfaces/hostname at default/None; a render-window smoke test still yields the same pill markup (characterization).
- [ ] Step 2: Run — FAIL.
- [ ] Step 3: Implement the lean builder.
- [ ] Step 4: Run — PASS (pill markup unchanged).
- [ ] Step 5: clippy + fmt.
- [ ] Step 6: Commit `perf(context): lean window-context path skips unused reads (W8)`.

### Task 12 (W11) `[opus]`: cross-invocation /proc/stat cache

**Files:**
- Modify: `crates/rustline/src/cpu.rs` (+ a snapshot-cache helper, e.g. `cpu_cache.rs`)

**Interfaces:**
- Consumes: `rustline_wasm::state_root()` for the state-file dir; existing `parse_proc_stat` + `busy_percent`.
- Produces: `read_cpu()` uses a persisted `/proc/stat` snapshot (`{ jiffies-snapshot, unix_ts }`) when fresh; else falls back to the current two-sample 120ms read; always persists the new snapshot. Pure `busy_from_snapshots(prev, now, prev_ts, now_ts, staleness_secs) -> Option<f32>`.

**Steps:**
- [ ] Step 1: Tests (pure): delta of two fabricated snapshots yields correct busy%; a stale/absent prior returns `None` (→ caller falls back); snapshot serialize/parse round-trips; a corrupt/missing state file yields the fallback and never panics (mirror the toggles-file total-read discipline).
- [ ] Step 2: Run — FAIL.
- [ ] Step 3: Implement the snapshot persistence + freshness (fixed conservative staleness bound, e.g. 60s — documented) + fallback; keep `parse_proc_stat`/`busy_percent` unchanged; best-effort file I/O.
- [ ] Step 4: Run — PASS.
- [ ] Step 5: clippy + fmt.
- [ ] Step 6: Commit `perf(cpu): cross-invocation /proc/stat sample cache, default-on with two-sample fallback (W11)`.

### Phase 3 review checkpoint
- [ ] Targeted full review over T10–T12: cpu-cache delta math + staleness + total-on-failure; gating skips reads without behavior change. Fix loop. **No user pause.**

---

# Phase 4 — CLI

### Task 13 (W6): tmux block uses absolute binary path

**Files:**
- Modify: `crates/rustline/src/tmux_conf.rs` (`InitBlockOpts` gains `binary: String`; `#(...)` calls use it), `init.rs` + `main.rs` (resolve `std::env::current_exe()`), `cli.rs` (`InitArgs.binary: Option<String>`)

**Interfaces:**
- Produces: `init_block` emits `#(<binary> render …)`; `init --binary <path>` overrides; `--print` uses the resolved binary. Injection shape (`--flag=#{q:VAR}`) unchanged.

**Steps:**
- [ ] Step 1: Update the byte-identical baseline test(s) to the new expected block (absolute path); add `block_uses_binary_path`, `binary_flag_overrides`, and an assertion the `#{q:}`/`--flag=` shape is preserved.
- [ ] Step 2: Run — FAIL.
- [ ] Step 3: Thread `binary` through `InitBlockOpts` + resolve `current_exe` in the caller; add the `--binary` flag.
- [ ] Step 4: Run — PASS.
- [ ] Step 5: clippy + fmt.
- [ ] Step 6: Commit `feat(init): tmux block calls the absolute binary path + --binary override (W6)`.

### Task 14 (W9): shell completions

**Files:**
- Modify: `crates/rustline/Cargo.toml` (+ `clap_complete`), `cli.rs` (Completions subcommand `{ shell: clap_complete::Shell }`), `main.rs` (dispatch: `generate` to stdout), commit `Cargo.lock`.

**Steps:**
- [ ] Step 1: Integration test in `tests/smoke.rs`: `rustline completions bash` exits 0 and stdout contains `rustline` (repeat for zsh/fish).
- [ ] Step 2: Run — FAIL.
- [ ] Step 3: Add the dep + subcommand + dispatch.
- [ ] Step 4: Run — PASS.
- [ ] Step 5: clippy + fmt; `cargo tree -i openssl` empty; commit `Cargo.lock`.
- [ ] Step 6: Commit `feat(cli): shell completions via clap_complete (W9)`.

### Task 15 (W10): `init --dry-run`

**Files:**
- Modify: `crates/rustline/src/init.rs`, `cli.rs` (`InitArgs.dry_run: bool`)

**Interfaces:**
- Consumes: existing `starter_config_toml`/`merge_config`/`init_block`/`upsert_tmux_block`.
- Produces: `init --dry-run` prints the config.toml + tmux block that would be written (with a per-file header and, when target files exist, a diff), touches no disk.

**Steps:**
- [ ] Step 1: Integration test: `init --dry-run --defaults` writes nothing (target files absent/unchanged) and prints both artifacts.
- [ ] Step 2: Run — FAIL.
- [ ] Step 3: Implement dry-run branch (reuse generation; print; skip writes).
- [ ] Step 4: Run — PASS.
- [ ] Step 5: clippy + fmt.
- [ ] Step 6: Commit `feat(init): --dry-run previews config + tmux block without writing (W10)`.

### Task 16 (W13): `rustline doctor`

**Files:**
- Create: `crates/rustline/src/doctor.rs` (pure check fns + I/O shell), Modify: `cli.rs` (Doctor subcommand), `main.rs` (dispatch)

**Interfaces:**
- Produces: `Check { name, status: CheckStatus (Ok|Warn|Fail), detail }`; pure `parse_tmux_version(&str) -> Option<(u32,u32)>`, `truecolor_from_env(colorterm: Option<&str>) -> bool`, `block_installed(tmux_conf: &str) -> bool`. Exit non-zero if any `Fail`.

**Steps:**
- [ ] Step 1: Tests for the pure parsers: `tmux 3.1` ≥ pass, `tmux 3.0a` fail, garbage → warn; truecolor from `COLORTERM=truecolor`; block-present over a sample conf. Integration: `rustline doctor` runs and prints resolved paths.
- [ ] Step 2: Run — FAIL.
- [ ] Step 3: Implement pure checks + the thin I/O shell (`tmux -V`, `tmux show -gv mouse` when in tmux, env, `~/.tmux.conf`, dir existence) + exit code.
- [ ] Step 4: Run — PASS.
- [ ] Step 5: clippy + fmt.
- [ ] Step 6: Commit `feat(cli): rustline doctor prerequisite diagnostics (W13)`.

### Task 17 (W15): `init --uninstall`

**Files:**
- Modify: `crates/rustline/src/tmux_conf.rs` (`remove_tmux_block(existing: &str) -> String`), `init.rs` (uninstall path: back up, strip, print reload), `cli.rs` (`InitArgs.uninstall: bool`)

**Interfaces:**
- Consumes: `TMUX_BEGIN`/`TMUX_END`, `find_region` internals.
- Produces: `remove_tmux_block` strips exactly the managed region, leaving surrounding content byte-identical; idempotent (no block → unchanged input).

**Steps:**
- [ ] Step 1: Tests: `remove_tmux_block` strips the region and preserves surroundings; idempotent; `upsert` then `remove` returns the original (round-trip).
- [ ] Step 2: Run — FAIL.
- [ ] Step 3: Implement `remove_tmux_block` + the `--uninstall` path (backup to `~/.tmux.conf.rustline.bak`, print reload).
- [ ] Step 4: Run — PASS.
- [ ] Step 5: clippy + fmt.
- [ ] Step 6: Commit `feat(init): --uninstall removes the managed tmux block (W15)`.

### Task 18 (W16): `theme new --edit`

**Files:**
- Modify: `crates/rustline/src/theme_cmd.rs`, `cli.rs` (`ThemeCmd::New` gains `edit: bool`)

**Interfaces:**
- Produces: after writing the scaffold, `theme new` prints the `rustline theme use <name>` follow-up; with `--edit` and `$EDITOR` set + TTY, spawns the editor on the file; without `$EDITOR`, prints a hint (no spawn).

**Steps:**
- [ ] Step 1: Test (capture stdout): the follow-up `theme use <name>` line is printed with the written path; `--edit` without `$EDITOR` degrades to a hint (no spawn) — factor the "should spawn?" decision into a pure helper to test it.
- [ ] Step 2: Run — FAIL.
- [ ] Step 3: Implement the follow-up print + the `--edit` spawn/hint.
- [ ] Step 4: Run — PASS.
- [ ] Step 5: clippy + fmt.
- [ ] Step 6: Commit `feat(theme): theme new --edit + use follow-up hint (W16)`.

### Task 19 (W21): `config` command group

**Files:**
- Create: `crates/rustline/src/config_cmd.rs`, Modify: `cli.rs` (`Config` group: `Path`/`Edit`/`Validate`), `main.rs` (dispatch)

**Interfaces:**
- Produces: `config path` prints `config_path()`; `config edit` opens `$EDITOR` (create-from-starter if absent); `config validate` parses strictly (not the total `Config::load`) and reports errors with location, exit≠0 on failure, "ok" + path on success. `Config::load` totality unchanged.

**Steps:**
- [ ] Step 1: Integration tests: `config path` prints the resolved path; `config validate` on a good temp config → exit0/"ok"; on a malformed one → exit≠0 + error message (use `--config` if available, else a `CONFIG` env/temp — reuse existing test harness in `tests/smoke.rs`).
- [ ] Step 2: Run — FAIL.
- [ ] Step 3: Implement the group (strict parse via `toml::from_str` with error surfacing).
- [ ] Step 4: Run — PASS.
- [ ] Step 5: clippy + fmt.
- [ ] Step 6: Commit `feat(cli): config path/edit/validate group (W21)`.

### Phase 4 review checkpoint
- [ ] Targeted full review over T13–T19: injection-safety of the touched tmux block (W6/W15/W10), the byte-identical baseline update, and `config validate` not weakening `Config::load` totality. Fix loop. **No user pause.**

---

# Phase 5 — WASM ecosystem

### Task 20 (W7): `rl_log` guest logging host function

**Files:**
- Modify: `crates/rustline-wasm/src/perform.rs` (`perform_log(level, msg)` → tracing), `host.rs` (`rl_log` `host_fn!` wrapper), `abi.rs` if a type is needed (likely none — fire-and-forget)

**Interfaces:**
- Produces: guest import `rl_log(level: String, msg: String)`; capability-free (no allowlist gate); maps level strings (`error`/`warn`/`info`/`debug`/`trace`) to `tracing`.

**Steps:**
- [ ] Step 1: Tests: `perform_log` maps level strings to the correct level (pure), and asserts it performs no network/fs (capability-free). (Guest-wiring e2e stays behind `wasm-e2e`.)
- [ ] Step 2: Run — FAIL.
- [ ] Step 3: Implement `perform_log` + `rl_log` wrapper (mirror the six existing `host_fn!` wrappers, minus the capability gate).
- [ ] Step 4: Run — PASS.
- [ ] Step 5: clippy + fmt.
- [ ] Step 6: Commit `feat(wasm): rl_log capability-free guest logging host function (W7)`.

### Task 21 (W24) `[opus]`: plugin manifest (sidecar ⊃ embedded) + `plugin approve`

**Files:**
- Create: `crates/rustline-wasm/src/manifest.rs` (`PluginManifest`, sidecar + embedded resolver), expose from `lib.rs`
- Modify: `crates/rustline/src/plugin_cmd.rs` (`approve` + `list` note), `cli.rs` (`plugin approve <name> [--yes]`)

**Interfaces:**
- Produces: `PluginManifest { name: String, version: String, requested_urls: Vec<String>, requested_paths: Vec<String> }`; `resolve_manifest(plugin_dir, name) -> Option<PluginManifest>` — sidecar `<name>.toml` first (supersedes), else embedded wasm custom section `rustline-manifest` (walk custom sections via `wasmparser`, already in the tree via wasmtime/extism; else a minimal hand-walk), else `None`. `plugin approve` writes `requested_urls`/`requested_paths` into `[plugins.<name>].allowed_urls`/`allowed_paths` via the existing `toml_edit` path — and no more (N4).

**Steps:**
- [ ] Step 1: Tests: parse a sidecar `.toml` manifest; parse an embedded custom section (craft a tiny wasm with a `rustline-manifest` custom section, or unit-test the section-walk over crafted bytes); **sidecar supersedes embedded** when both present; missing → `None`; malformed → skipped-with-warn (never breaks discovery, N2). `approve` writes exactly the requested entries (toml_edit, comment-preserving) and nothing else.
- [ ] Step 2: Run — FAIL.
- [ ] Step 3: Implement `manifest.rs` + the resolver precedence + `plugin approve` (interactive consent or `--yes`) + the `plugin list` "declared capabilities" note.
- [ ] Step 4: Run — PASS.
- [ ] Step 5: clippy + fmt; `cargo tree -i openssl` empty.
- [ ] Step 6: Commit `feat(plugin): capability manifest (sidecar ⊃ embedded) + plugin approve (W24)`.

### Task 22 (W23): `plugin new` scaffold

**Files:**
- Modify: `crates/rustline/src/plugin_cmd.rs` (`new` subcommand), `cli.rs` (`plugin new <name> [--path] [--force]`), add an embedded template (`include_str!` assets under `crates/rustline/src/assets/` mirroring `starter-config.toml`)

**Interfaces:**
- Consumes: the W26 `WireContext` (the scaffold's `lib.rs` deserializes it).
- Produces: a guest crate dir — `Cargo.toml` (edition 2024, `crate-type=["cdylib"]`, deps `extism-pdk`+`serde`+`serde_json`+`rustline-abi`, **empty `[workspace]` table**), `src/lib.rs` skeleton exporting `name()`/`render()` using `GuestRender`/`WireContext`, and a printed `[plugins.<name>]` snippet. Name validation: `[A-Za-z0-9_-]`, ≤15 bytes, not `window`; refuse overwrite without `--force`.

**Steps:**
- [ ] Step 1: Tests: name validation (rejects `/`, `..`, >15 bytes, `window`, empty); scaffold writes `Cargo.toml` (contains empty `[workspace]`) + `src/lib.rs` (references `WireContext`); refuses overwrite without `--force`.
- [ ] Step 2: Run — FAIL.
- [ ] Step 3: Implement the templates + `new` command.
- [ ] Step 4: Run — PASS.
- [ ] Step 5: clippy + fmt.
- [ ] Step 6: Commit `feat(plugin): plugin new scaffold template (W23)`.

### Task 23 (W27): additional example plugins + generic build recipe

**Files:**
- Create: `plugins/counter/` (state via `rl_state_read`/`rl_state_write`), `plugins/filewatch/` (`rl_file_read`), `plugins/httpget/` (plain `rl_http_get`) — each excluded member, edition 2024, empty `[workspace]`, pure logic + `#[cfg(target_arch="wasm32")]` guest using `WireContext` (and demoing `rl_log`).
- Modify: `justfile` (generic `build-plugin NAME` recipe alongside `build-weather`).

**Steps:**
- [ ] Step 1: For each plugin, write host-target unit tests for its pure logic (formatting/parsing/state-key logic).
- [ ] Step 2: Run — FAIL.
- [ ] Step 3: Implement the three plugins + the generic build recipe.
- [ ] Step 4: Run the host-side unit tests — PASS (`just test` stays hermetic; guest builds via `build-plugin`).
- [ ] Step 5: clippy + fmt.
- [ ] Step 6: Add the three plugins to the plugin list in CLAUDE.md + README.md.
- [ ] Step 7: Commit `feat(plugins): counter/filewatch/httpget examples + generic build recipe (W27)`.

### Phase 5 review checkpoint
- [ ] Targeted full review over T20–T23: N1–N4 (rl_log capability-free, approve never over-grants, manifest precedence, examples honor the sandbox), doc-list sync. Fix loop. **No user pause.**

---

### Task 24: Docs sweep, WHATS-NEXT reconciliation, finish branch

**Files:**
- Modify: `CLAUDE.md`, `README.md` (module map + Config sections: new widgets `git`/`disk`; new subcommands `doctor`, `config`, `completions`, `plugin new`/`approve`; `init --binary/--dry-run/--uninstall`; `rl_log`; `WireContext`; `Registry` descriptors; `Context::default`; cpu cache; new plugins), `WHATS-NEXT.md` (strip the 22 in-flight items — they ship in this branch).

**Steps:**
- [ ] Step 1: Sweep the spec's "Documentation updates" section; update CLAUDE.md + README.md so no stale statement remains and every new surface is documented (concise pointer + spec link).
- [ ] Step 2: Strip the 22 in-flight `W*` entries (and their `> in-flight` markers) from `WHATS-NEXT.md` per the standing instruction (shipped items removed in the shipping change). Note: `WHATS-NEXT.md` is gitignored via `.git/info/exclude`, so this edit is bookkeeping only (not committed).
- [ ] Step 3: `just test` + `just lint` (+ `just test-wasm` if the wasm target is available) — all green.
- [ ] Step 4: Commit the doc updates `docs: document whats-next bundle features (widgets, CLI, WASM ecosystem)`.
- [ ] Step 5: Final whole-branch review (requesting-code-review, adversarial). Fix loop until clean.
- [ ] Step 6: `superpowers:finishing-a-development-branch` to integrate.

## Self-Review (done)

- **Spec coverage:** every spec item W1–W27-in-bundle maps to a task (W3→T1, W26→T2+T3, W22→T4, W4→T5+T6, W1→T6, W14→T7, W12→T8, W20→T9, W5→T10, W8→T11, W11→T12, W6→T13, W9→T14, W10→T15, W13→T16, W15→T17, W16→T18, W21→T19, W7→T20, W24→T21, W23→T22, W27→T23; docs/finish→T24). No gaps.
- **Type consistency:** `GitInfo`/`DiskInfo` defined in abi (T8/T9), added to `Context` + `WireContext`; `WireContext` (T3) is extended by T8/T9 and re-verified by the seam test; `WidgetDescriptor`/`WidgetSource` (T4) names consistent across core + wasm.
- **Ordering:** T1 (`Default`) precedes T8/T9/T11 that rely on it; T2 precedes T3; T3's seam test is extended by T8/T9; T10 gates reads added through T9. Phase reviews gate each boundary.
