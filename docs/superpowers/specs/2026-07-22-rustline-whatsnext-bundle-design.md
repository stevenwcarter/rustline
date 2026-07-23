# rustline whats-next bundle — design spec

**Date:** 2026-07-22
**Branch:** `feature/whats-next-bundle` (single branch for the whole bundle)
**Source:** `/whats-next --execute` handoff — 22 user-selected items from
`WHATS-NEXT.md` triaged 2026-07-22.

This spec covers **22 items** organized into **5 phases**. The phases are an
internal sequencing + review structure on **one branch** — each phase gets a
targeted full code review at its boundary; there is no merge or user check-in
between phases. Dependency-ordered: Phase 1 lands the enabling refactors the
later widgets/guests build on.

## Goals

Ship all 22 selected opportunities as coherent, TDD'd, clippy/fmt-clean Rust,
without regressing any existing behavior or invariant.

## Non-goals / out of scope (deferred whats-next items, not in this bundle)

- W2, W17, W18, W19, W25, W28–W49 (unchecked at execute time).
- W44 widget-command CLI (only its enabling refactor, W22, is here).
- W39 plugin SDK crate (W23 scaffold + W26 typed context are here; they are
  designed to *compose* with a future SDK, not to require it).
- W45 sparkline (W11's persisted sample is designed to *enable* it later).

## Cross-cutting constraints (load-bearing — every task honors these)

1. **Context is the sole render input** (invariant #1). New widgets read only
   from `Context`; new signals (`git`, `disk`) are read once at the
   Context-build edge, never mid-render.
2. **`Segment`/`Context`/`Style`/`Color` stay serde-serializable** (invariant
   #2). W26 keeps the on-wire JSON byte-identical.
3. **`Config::load` stays total** (invariant #3). Every new config field is
   `#[serde(default)]`. W21's `config validate` is a *separate explicit
   surface*, not a behavior change to `load`.
4. **`init` output stays injection-safe** (`#{q:}` + `--flag=`) (invariant #4).
   W6/W10/W13/W15 touch the tmux block.
5. **Platform reads stay `#[cfg(target_os)]` with `cfg(test)`-compiled pure
   parsers**, unit-tested on the Linux dev box (W11/W12/W20 follow the
   `read_battery`/`read_cpu` pattern).
6. **WASM invariants N1–N4 hold** (W7/W23/W24/W27): zero ambient authority, a
   plugin never breaks the bar, sandboxed+quota state, per-plugin capability
   scope.
7. **Edition 2024** in every crate incl. new plugin members; keep all crate
   editions equal to `rustfmt.toml`. New excluded plugin members carry an empty
   `[workspace]` table (else nested-worktree builds break).
8. **Clippy-clean + rustfmt-clean**; commit `Cargo.lock` with any dep change
   (W9 adds `clap_complete`; no other new host deps — W12 shells out, W20 uses
   the existing `libc`).
9. **rustls-only / openssl-free** stays true: `cargo tree -i openssl` and
   `-i native-tls` stay empty across the whole graph.
10. **When adding a widget/plugin**, sync the widget/plugin lists in **both**
    `CLAUDE.md` and `README.md` as the final step (W12/W20/W23/W27).
11. **TDD throughout.** The "no test needed because invariant X" justification
    is a red flag — write the load-bearing seam test, especially W26's
    host↔guest round-trip.

---

# Phase 1 — Foundational refactors (W3, W22, W26)

## W3 — `Context::default()`

**What:** Give `Context` a hand-written `Default` so construction sites can use
struct-update syntax instead of spelling all ~18 fields.

**Design:**
- Implement `Default for Context` by hand (can't derive: `DateTime<Local>` and
  the `[f64;3]` inside `Option` don't compose a clean derive, and `now` needs a
  sentinel). Defaults: all `String` empty, all `Option` `None`, `interfaces`
  empty, `toggled`/`colors` default, `now` = Unix epoch in `Local`
  (`Local.timestamp_opt(0, 0).single().unwrap()`).
- Migrate the *churn-prone* construction sites to `Context { <overrides>,
  ..Default::default() }`: the `#[cfg(test)]` fixtures in `context.rs`,
  `widget.rs`, `assemble.rs`, `rustline-wasm/src/host.rs` tests, and the
  synthetic `Context`s in `theme_cmd.rs`. **`build_context.rs` stays fully
  explicit** (it is the real construction site; being explicit there is
  correct).

**Files:** `crates/rustline-core/src/context.rs` (+ the listed test/synthetic
sites).

**Tests:** `default_context_is_empty_and_epoch` — asserts the sentinel `now`,
empty strings, `None` options. Existing serde round-trip tests must still pass
(behavior-preserving).

## W22 — Registry widget enumeration + descriptor

**What:** Let the registry answer "what widgets exist?" and describe each,
without instantiating — the enabling abstraction for the future `widget`
command group (W44, not in this bundle).

**Design:**
- New public struct in `widget.rs`:
  ```rust
  pub struct WidgetDescriptor {
      pub name: String,
      pub summary: String,      // one-line human description
      pub configurable: bool,   // carries a [widgets.<name>] options table
      pub source: WidgetSource, // Builtin | Plugin
  }
  pub enum WidgetSource { Builtin, Plugin }
  ```
- `Registry` keeps its `HashMap<String, Factory>` for `build`/`resolve`
  (unchanged) **and** gains an ordered `Vec<WidgetDescriptor>` populated at
  registration time.
- `register(name, factory)` keeps working (back-compat: derives a minimal
  `WidgetDescriptor { name, summary: name, configurable: false, source:
  Builtin }`). Add `register_described(descriptor, factory)` used by
  `with_builtins`. Plugin registration in `rustline-wasm` uses a variant that
  sets `source: Plugin` and a generic summary.
- New accessors: `descriptors(&self) -> &[WidgetDescriptor]` (registration
  order) and `available_names(&self) -> impl Iterator<Item=&str>`.
- `with_builtins` supplies a one-line `summary` and correct `configurable` flag
  for each of the 11 built-ins.

**Files:** `crates/rustline-core/src/widget.rs`,
`crates/rustline-core/src/widgets/mod.rs`,
`crates/rustline-wasm/src/lib.rs` (plugin registration path).

**Tests:** `descriptors_list_all_builtins_in_order`,
`descriptor_marks_configurable_widgets`, `register_backcompat_minimal_descriptor`,
and (rustline-wasm) a plugin descriptor carries `source: Plugin`. Existing
`resolve`/`contains`/`build` tests unchanged.

## W26 — Typed `Context`/`WindowCtx` wire types in `rustline-abi`

**What:** Give WASM guests a typed `Context` to deserialize instead of
hand-walking `serde_json::Value`, with **the on-wire JSON byte-identical** to
today's.

**Design (safest byte-identical approach):**
- **Host serialization is UNCHANGED.** `RenderInput { context: &Context,
  config: &Value }` still serializes the real `Context` (whose `now:
  DateTime<Local>` chrono-serializes to an RFC3339 string exactly as today). No
  wire bytes change.
- **Move the chrono-free nested types to `rustline-abi`** and re-export them
  from `rustline-core` (the `Segment`/`Style`/`Color` precedent, so
  `rustline_core::Battery` etc. keep resolving): `BatteryState`, `Battery`,
  `MemInfo`, `CpuUsage`, `NetIface`. `Context`/`WindowCtx` **stay in
  rustline-core** (they carry chrono via `now`).
- **Add `WireContext`/`WireWindowCtx` to `rustline-abi`**: field-for-field
  mirrors of `Context`/`WindowCtx` with `now: String` (RFC3339) instead of
  `DateTime<Local>`; every other field references the now-shared abi nested
  types, so the two shapes are guaranteed to match. Both carry the same
  `#[serde(default)]` on `toggled`/`colors`.
- **Guest input helper in abi:** a `#[derive(Deserialize)] struct GuestRender {
  context: WireContext, config: serde_json::Value }` (or documented shape) so a
  guest does `let input: GuestRender = serde_json::from_str(&s)?;`.
- **Migrate the weather guest** (`plugins/weather/src/lib.rs`) to deserialize
  `WireContext` instead of `v["context"]["now"].as_str()` / the manual
  `toggled` array walk. `config` stays a `Value` (plugin-specific keys).

**Load-bearing seam test (invariant #2 / the "no test because invariant" rule):**
In `rustline-wasm` (the crate that owns `RenderInput` and depends on both core
and abi): build a representative `Context`, serialize it exactly as the host
does, deserialize the `context` field as `rustline_abi::WireContext`, and assert
**every field round-trips** (`now` string parses back to the same instant,
`battery`/`cpu`/`memory`/`interfaces`/`toggled`/`colors` equal). This pins the
host `Context` shape and the guest `WireContext` shape together, so a later
rename/retype of a `Context` field fails this test instead of silently breaking
guests. Add an abi-local unit test that `WireContext` deserializes a
representative literal JSON and that a missing `toggled`/`colors` defaults.

**Files:** `crates/rustline-abi/src/lib.rs` (new types),
`crates/rustline-core/src/context.rs` (re-export nested types from abi),
`crates/rustline-core/src/segment.rs` (extend the re-export module if needed),
`crates/rustline-wasm/src/abi.rs` (+ seam test), `plugins/weather/src/lib.rs`.

**Phase 1 review checkpoint:** full review focused on the ABI byte-identical
guarantee (diff the guest JSON before/after), the Default sentinel, and registry
back-compat.

---

# Phase 2 — Widgets (W1, W4, W14, W12, W20)

## W1 — cwd path shortening

**What:** Bound the rendered cwd so a deep path doesn't push the right region
off a narrow pane.

**Design:** Extend the `cwd` widget config (`CwdOpts`, currently
`abbreviate_home`) with:
- `max_depth: usize` (0 = unlimited; keep the last N path components, prefix a
  leading `…/`),
- `max_len: usize` (0 = unlimited; if the rendered string exceeds it, truncate
  from the left with a leading `…`),
- `abbreviate: bool` (fish-style: shorten every component except the last to its
  first char, `~/s/r/rustline`).
- A `format` string (see W4) with `{path}`.
- Precedence: abbreviate → max_depth → max_len, all pure string ops in
  `cwd.rs`. **Defaults leave output byte-identical** to today (all off,
  `abbreviate_home` preserved).

**Files:** `crates/rustline-core/src/widgets/cwd.rs`,
`crates/rustline-core/src/config.rs` (`CwdOpts`), `widgets/mod.rs` (factory).

**Tests:** each transform (abbrev, max_depth ellipsis, max_len left-truncate),
combined, and a "defaults = current output" characterization test.

## W4 — `format`/label + icon on cwd, hostname, pane_id

**What:** Let the three fixed-output widgets carry a `format` so users can add a
Nerd-Font icon/label.

**Design:** Add a `format` field to each widget's config:
- `hostname`: `{host}` (default `"{host}"` → byte-identical).
- `pane_id`: `{session}:{window}.{pane}` (default reproduces current output
  exactly — verify the current format first and mirror it).
- `cwd`: `{path}` (default `"{path}"`), composing with W1's shortening (shorten,
  then substitute into `format`).
- Pure `replace`-style substitution; unknown placeholders pass through (matching
  the datetime/loadavg convention).

**Files:** `crates/rustline-core/src/widgets/hostname.rs`, `pane_id.rs`,
`cwd.rs`; `config.rs` (add `format` to each opts struct); `widgets/mod.rs`.

**Tests:** default format = current output (characterization, all three);
custom label/icon prepend; unknown placeholder passthrough.

## W14 — Configurable icon glyphs for cpu/memory/battery

**What:** Let users override the hardcoded Nerd-Font `{icon}` glyphs.

**Design:** Add an optional `icon: Option<String>` to `cpu`/`memory` config:
when `Some`, it replaces the computed `{icon}` glyph. For `battery` (whose glyph
is level-bucketed + charging-aware), add `icon: Option<String>` that, when set,
overrides the whole bucketed glyph with the literal (documented: overriding
disables the level buckets). Defaults `None` → byte-identical.

**Files:** `crates/rustline-core/src/widgets/cpu.rs`, `memory.rs`, `battery.rs`;
`config.rs`.

**Tests:** override replaces glyph; `None` = current bucketed/hardcoded glyph.

## W12 — Git branch/status widget (new built-in)

**What:** A `git` widget showing branch + dirty + ahead/behind.

**Design (shell-out, chosen):**
- New read surface `crates/rustline/src/git.rs`: `read_git(path: &str) ->
  Option<GitInfo>` runs `git -C <path> status --porcelain=v2 --branch` (and
  falls back to `None` on any failure / not-a-repo / no `git`). A pure
  `parse_git_status(output) -> GitInfo` function is `#[cfg(any(unix, test))]`
  (really platform-agnostic; just keep it pure + unit-tested), matching the
  `read_battery`/`parse_*` pattern.
- `GitInfo { branch: String, ahead: u32, behind: u32, staged: u32, unstaged:
  u32 }` (derive `dirty = staged>0 || unstaged>0`). `GitInfo` is chrono-free, so
  define it in `rustline-abi` (like `Battery`/`MemInfo`), re-export from
  `rustline-core`, and include `pub git: Option<GitInfo>` in **both** `Context`
  and `WireContext` (consistent with `disk`; keeps the round-trip test
  complete). Read once at Context-build time from `pane_current_path`
  (invariant #1).
- New widget `crates/rustline-core/src/widgets/git.rs`, pure over `Context.git`,
  placeholders `{branch}`, `{ahead}`, `{behind}`, `{dirty}` (a configurable
  glyph, default `*` when dirty else empty), `{staged}`, `{unstaged}`. `format`
  default e.g. `" {branch}{dirty}"` (Nerd-Font branch glyph). `down_format`
  (default `""`) when `Context.git` is `None`. Opt-in (not in default layout).
  Register in `with_builtins` with a descriptor.
- Threshold-aware: not applicable (no numeric threshold). Click-toggle:
  `alt_format` support like the format-bearing family (fits the existing
  `active_format`/`clickable_range` helpers; `git` is ≤15 bytes so clickable).

**Files:** `crates/rustline/src/git.rs` (read surface),
`crates/rustline/src/build_context.rs` (call `read_git`, gated on `git` in
layout — see W5), `crates/rustline-core/src/context.rs` (+ `git` field),
`crates/rustline-abi/src/lib.rs` (+ `GitInfo` mirror in `WireContext`),
`crates/rustline-core/src/widgets/git.rs` (new), `config.rs` (GitOpts),
`widgets/mod.rs`. Docs: CLAUDE.md + README.md widget lists.

**Tests:** `parse_git_status` over real `--porcelain=v2 --branch` fixtures
(clean, dirty, ahead/behind, detached HEAD, staged+unstaged); widget render with
each placeholder; `down_format` when `None`; default format characterization.

## W20 — Disk-usage widget (new built-in)

**What:** A `disk` widget showing filesystem usage.

**Design:**
- New read surface `crates/rustline/src/disk.rs`: `read_disk(mount: &str) ->
  Option<DiskInfo>` via `libc::statvfs` (already have `libc`; no new dep).
  `#[cfg(any(target_os="linux", target_os="macos", test))]` pure derivation of
  bytes from `statvfs` fields; `None` on failure/unsupported. `DiskInfo {
  total_bytes, used_bytes, available_bytes }` (same shape as `MemInfo`).
- Add `pub disk: Option<DiskInfo>` to `Context` (+ abi mirror in `WireContext`,
  chrono-free). Read once at build time; `mount` from `[widgets.disk].mount`
  (default `/`).
- New widget `crates/rustline-core/src/widgets/disk.rs`: reuse `format_bytes`
  and shared `gauge_bar`. Placeholders `{used}`/`{total}`/`{avail}`/`{percent}`/
  `{bar}`, plus a `{mount}` label. Threshold-aware via `alert.rs`
  (`warn_percent`/`crit_percent`, e.g. default 85/95, `alert_over`). `format`
  default `" {used}/{total}"`, `down_format` default `""`. Opt-in. Click-toggle
  `alt_format`. Register with a descriptor.

**Files:** `crates/rustline/src/disk.rs`, `build_context.rs` (gated read),
`context.rs` (+ `disk`), `rustline-abi` (mirror), `widgets/disk.rs` (new),
`config.rs` (DiskOpts), `widgets/mod.rs`. Docs: CLAUDE.md + README.md.

**Tests:** pure `statvfs`→`DiskInfo` derivation over fabricated field values;
widget render (bytes formatting, bar, percent); threshold badge at warn/crit;
below-threshold = no badge; `down_format` when `None`.

**Phase 2 review checkpoint:** full review focused on byte-identical defaults for
the touched widgets and the two new platform reads following the cfg pattern.

---

# Phase 3 — Perf (W5, W8, W11)

## W5 — Gate battery + interface reads by layout

**What:** Read battery + interfaces only when the layout references them, like
cpu/memory already are.

**Design:** In `build_region_context` (`build_context.rs`), wrap
`read_interfaces()` in `layout references lan_ip||tailscale_ip` and
`read_battery()` in `layout references battery` (mirroring the existing
cpu/memory `layout.iter().any(...)` gating). Also gate the new `git`/`disk`
reads (W12/W20) the same way. `loadavg` (cheap `getloadavg`) stays ungated.

**Files:** `crates/rustline/src/build_context.rs`.

**Tests:** a layout without battery/ip produces a Context with `battery: None`,
`interfaces: []` **without** having called the reads (assert via a seam: extract
the gating predicate as a pure `needs_*` helper and unit-test the predicate; the
read-skipping itself is covered by the predicate).

## W8 — Lean window-context path

**What:** The per-window invocation (`render window`, once per window) shouldn't
pay for battery/interfaces/loadavg/toggles/hostname it never renders.

**Design:** Give `build_window_context` its own minimal construction (via W3's
`Context::default()` + only the fields the pill needs: `window`, `colors`, and
whatever `render_window_pill` reads) instead of routing through
`build_region_context`. Verify against `render_window`/`render_window_pill` which
fields are actually consumed.

**Files:** `crates/rustline/src/build_context.rs`,
`crates/rustline/src/main.rs` (window dispatch, if it calls the region builder).

**Tests:** `build_window_context` yields the pill-relevant fields set and the
unused ones at default/None; a render-window smoke test still produces the same
pill markup (characterization).

## W11 — Cross-invocation /proc/stat cache (kill the ~120ms sleep)

**What:** Remove the mandatory 120ms two-sample sleep on every render by
persisting the prior `/proc/stat` snapshot.

**Design (default-on with fallback, chosen):**
- On Linux, persist the last `/proc/stat` aggregate snapshot + a timestamp to a
  small state file under `rustline_wasm::state_root()` (reuse the existing
  state-dir plumbing; best-effort, a write/read failure just falls back).
- On read: if a prior snapshot exists and is fresh (age below a staleness bound
  ≈ the refresh interval; use a fixed conservative bound, e.g. 60s, since the
  interval isn't known to the read — document it), compute the busy delta
  against it (**no sleep**) and persist the new snapshot. If absent/stale, do
  the current two-sample read (120ms) and persist. Pure `parse_proc_stat` +
  `busy_percent` unchanged; the persistence + freshness is new and pure-testable
  (inject the prior snapshot + now).
- Behavior: transparent, no config. First-ever render / stale → same as today;
  steady-state → 0ms. The persisted snapshot is also the sample history a future
  sparkline (W45) can consume.

**Files:** `crates/rustline/src/cpu.rs` (+ a small snapshot-cache helper,
possibly `cpu_cache.rs`).

**Tests:** delta-vs-prior-snapshot yields correct busy% (pure, fabricated two
snapshots + interval); stale/absent prior falls back; snapshot serialize/parse
round-trips; a corrupt/missing state file yields the fallback (never panics —
mirrors the toggles-file total-read discipline).

**Phase 3 review checkpoint:** full review focused on the cpu-cache correctness
(delta math, staleness, total-on-failure) and that gating skips reads.

---

# Phase 4 — CLI (W6, W9, W10, W13, W15, W16, W21)

## W6 — tmux block uses the absolute binary path

**What:** The emitted tmux block should call the resolved binary path, not bare
`rustline` (which tmux's `/bin/sh` PATH may not find).

**Design:** In `tmux_conf.rs::init_block`, replace the bare `rustline` in each
`#(...)` with the absolute path from `std::env::current_exe()` (resolved by the
caller in `main.rs`/`init.rs` and threaded into `InitBlockOpts` as a `binary:
String`). Add `init --binary <path>` to override. **This changes the block's
output**, so update the byte-identical baseline tests to the new expected form
(the injection-safety `#{q:}`/`--flag=` shape is unchanged). `--print` uses the
same resolved binary.

**Files:** `crates/rustline/src/tmux_conf.rs`, `init.rs`, `cli.rs`
(`InitArgs.binary`), `main.rs` (resolve `current_exe`).

**Tests:** block contains the absolute path; `--binary` overrides; injection
shape preserved (still `--flag=#{q:...}`); the byte-identical one-line/mouse-off
baseline updated and asserted.

## W9 — shell completions

**What:** `rustline completions <bash|zsh|fish>`.

**Design:** New subcommand using `clap_complete` (new dep — commit `Cargo.lock`).
Generate to stdout for the given shell from the derived `clap::Command`.

**Files:** `crates/rustline/src/cli.rs` (Completions subcommand), `main.rs`
(dispatch), `Cargo.toml` (+ `clap_complete`).

**Tests:** a smoke test that generation for each shell produces non-empty output
containing the binary name (integration test in `tests/smoke.rs`).

## W10 — `init --dry-run`

**What:** Preview the config.toml + tmux block `init` would write, without
touching disk.

**Design:** Add `--dry-run` to `InitArgs`. When set (and not `--print`), run the
same generation as a real `init` (config TOML via `starter_config_toml`/merge;
tmux block via `init_block`+`upsert`), print both to stdout with a header per
file and, when the target files exist, a diff (reuse a minimal unified-diff or
show old/new); write nothing.

**Files:** `crates/rustline/src/init.rs`, `cli.rs`.

**Tests:** dry-run writes nothing (assert files unchanged/absent), prints both
artifacts; interacts correctly with `--defaults`.

## W13 — `rustline doctor`

**What:** Diagnose the documented prerequisites.

**Design:** New `doctor` subcommand + `crates/rustline/src/doctor.rs` with pure
check functions returning a `Vec<Check { name, status: Ok|Warn|Fail, detail }>`
rendered to a table/list. Checks: tmux present & version ≥ 3.1 (parse `tmux -V`);
`mouse on` (parse `tmux show -gv mouse` when inside tmux); truecolor
(`$COLORTERM`/tmux `RGB`/`Tc`); `rustline` resolvable on tmux's PATH; the
`>>> rustline >>>` block present in `~/.tmux.conf`; config/themes/plugin/log dirs
exist. Print resolved paths (config, themes, plugins, log). Exit non-zero if any
`Fail`.

**Files:** `crates/rustline/src/doctor.rs` (new, pure checks + I/O shell),
`cli.rs`, `main.rs`.

**Tests:** pure parsers — `tmux -V` version parse (≥3.1 pass, `3.0a` fail,
garbage warn), truecolor detection from env, block-present detection over a
sample `~/.tmux.conf`. The I/O shell stays thin.

## W15 — `init --uninstall`

**What:** Remove the managed tmux block.

**Design:** Add `--uninstall` to `InitArgs`. Locate the `TMUX_BEGIN..TMUX_END`
region in `~/.tmux.conf` (reuse the existing `find_region`/upsert internals),
strip it, back up to `~/.tmux.conf.rustline.bak` first, print the reload command.
Writes nothing else. Non-TTY safe (it's non-interactive).

**Files:** `crates/rustline/src/init.rs`, `tmux_conf.rs` (expose a
`remove_tmux_block(existing) -> String` helper), `cli.rs`.

**Tests:** `remove_tmux_block` strips exactly the managed region and leaves
surrounding content byte-identical; idempotent (no block → unchanged);
round-trips with `upsert_tmux_block`.

## W16 — `theme new --edit`

**What:** Open the scaffolded theme file in `$EDITOR` and print the follow-up.

**Design:** Add `--edit` to the `theme new` args. After writing the file (existing
path), if `--edit` and `$EDITOR` set and stdin is a TTY, spawn the editor on the
file; always print the `rustline theme use <name>` follow-up line alongside the
written path.

**Files:** `crates/rustline/src/theme_cmd.rs`, `cli.rs`.

**Tests:** the follow-up line is printed with the correct name/path (capture
stdout); `--edit` without `$EDITOR` degrades gracefully (prints a hint, no spawn)
— pure/whatever is testable without a real editor.

## W21 — `config` command group

**What:** `config path`, `config edit`, `config validate`.

**Design:** New `Config` subcommand group in `cli.rs` + `config_cmd.rs`:
- `config path`: print the resolved `config_path()`.
- `config edit`: open it in `$EDITOR` (create from the starter template if
  absent), print the path if no `$EDITOR`.
- `config validate`: parse the file with the *strict* parser (not the total
  `Config::load`) and report errors with line/column where `toml` provides them;
  exit non-zero on failure, print "ok" + resolved path on success. This is the
  explicit surface that turns `load`'s silent fallback into an actionable
  message (invariant #3 unchanged — `load` still total).

**Files:** `crates/rustline/src/config_cmd.rs` (new), `cli.rs`, `main.rs`.

**Tests:** `validate` on a good file → ok/exit0; on a malformed file → error
message + exit≠0 (integration in `tests/smoke.rs`); `path` prints the resolved
path.

**Phase 4 review checkpoint:** full review focused on injection-safety of the
touched tmux block (W6/W10/W15), the byte-identical baseline update (W6), and
`config validate` not weakening `Config::load`'s totality.

---

# Phase 5 — WASM plugin ecosystem (W7, W23, W24, W27)

## W7 — `rl_log` guest logging host function

**What:** A seventh, capability-free host function so guests can surface
messages.

**Design:** Add `perform_log(level: &str, msg: &str)` in `perform.rs` (routes to
`tracing` at the mapped level; capability-free — no allowlist gate, it only
writes to the host log) and a `rl_log` `host_fn!` wrapper in `host.rs` bound like
the other six. Guests import `rl_log(level, msg)`. Document it; the weather guest
(or an example in W27) can demonstrate it.

**Files:** `crates/rustline-wasm/src/perform.rs`, `host.rs`, `abi.rs` (if a
result type is needed — likely fire-and-forget, returns nothing/unit).

**Tests:** `perform_log` maps level strings to the right tracing level (pure);
a wiring test that a guest calling `rl_log` doesn't error (opt-in wasm-e2e).
N1 note: capability-free by design — no denied-case test applies, but assert it
performs no network/fs.

## W23 — `rustline plugin new <name>` scaffold

**What:** Scaffold a ready-to-build guest crate.

**Design:** New `plugin new <name>` under the `plugin` group. Emits a directory
(default under CWD or a `--path`) with: a `Cargo.toml` (edition 2024,
`crate-type = ["cdylib"]`, deps `extism-pdk`, `serde`, `serde_json`,
`rustline-abi`, an **empty `[workspace]` table**), a `src/lib.rs` skeleton that
exports `name()`/`render()` and deserializes the **W26 `WireContext`** (typed,
not hand-walked), and a starter `[plugins.<name>]` config snippet printed to
stdout. Validate `<name>` (`[A-Za-z0-9_-]`, ≤15 bytes, not `window`), refuse to
overwrite without `--force`.

**Files:** `crates/rustline/src/plugin_cmd.rs`, `cli.rs`, and an embedded
template (via `include_str!` assets, mirroring `init`'s `starter-config.toml`).

**Tests:** name validation (rejects `/`, `..`, >15 bytes, `window`); scaffold
writes the expected files incl. the empty `[workspace]` table; refuses overwrite
without `--force`. (The generated crate's *compilation* is covered manually /
by the existing `just build-weather`-style flow, not in hermetic `just test`.)

## W24 — Plugin capability manifest + `plugin approve`

**What:** A plugin declares the URLs/paths it needs; `plugin approve` shows them
and writes the allowlist on consent.

**Design (sidecar ⊃ embedded, per the answer):**
- **Manifest shape:** `PluginManifest { name, version, requested_urls: Vec,
  requested_paths: Vec }` (serde).
- **Resolution order:** for `<name>.wasm`, first look for a sidecar
  `<name>.toml` in the plugin dir and parse it; **the sidecar supersedes** an
  embedded manifest. If no sidecar, parse an embedded wasm **custom section**
  named `rustline-manifest` (walk the wasm custom sections — use `wasmparser`,
  already in the tree via wasmtime/extism, or a minimal hand-walk). If neither,
  no declared capabilities.
- **`rustline plugin approve <name>`:** resolve the manifest, print the
  requested urls/paths, and (interactive consent, or `--yes`) write them into
  `[plugins.<name>].allowed_urls`/`allowed_paths` via the existing `toml_edit`
  in-place mutation path in `plugin_cmd.rs`. Never widens beyond what the
  manifest requests (N4: per-plugin scope; approval writes an allowlist, it does
  not grant ambient authority). Deny-by-default is unchanged until approved.
- **`plugin list`** gains a "declared capabilities" note when a manifest exists.

**Files:** `crates/rustline-wasm/src/` (manifest type + sidecar/embedded
resolver, e.g. `manifest.rs`; expose from `lib.rs`),
`crates/rustline/src/plugin_cmd.rs` (`approve` + list note), `cli.rs`.

**Tests:** parse a sidecar `.toml` manifest; parse an embedded custom section;
**sidecar supersedes embedded** when both present; missing → no caps; `approve`
writes exactly the requested urls/paths into config (toml_edit, comment
preserving) and no more; malformed manifest is skipped with a warn (never
breaks discovery — N2).

## W27 — Additional worked example plugins

**What:** Grow the ecosystem's worked examples beyond `weather`.

**Design:** Three new excluded workspace members under `plugins/`, each edition
2024, `wasm32-unknown-unknown`, empty `[workspace]` table, pure logic
unit-tested on host + a `#[cfg(target_arch="wasm32")]` guest:
- `plugins/counter` — a state-backed counter using `rl_state_read`/`rl_state_write`
  (demonstrates the state sandbox + quota).
- `plugins/filewatch` — reads a configured file via `rl_file_read` and shows a
  line/summary (demonstrates arbitrary-file read under `allowed_paths`).
- `plugins/httpget` — a plain `rl_http_get` widget (uncached GET, contrast with
  weather's cached path).
Each uses the **W26 `WireContext`** typed input and may demonstrate **W7
`rl_log`**. Add a generic build note / extend `justfile` (a `build-plugin NAME`
recipe) so they build the same way as weather.

**Files:** `plugins/counter/`, `plugins/filewatch/`, `plugins/httpget/` (new),
`justfile` (generic build recipe), docs (plugin list in CLAUDE.md + README.md).

**Tests:** each plugin's pure logic (formatting/parsing) unit-tested on the host
target; the guest glue is wasm-only (built via the opt-in wasm flow, not
hermetic `just test`).

**Phase 5 review checkpoint:** full review focused on N1–N4 (rl_log
capability-free correctness, approve never over-grants, manifest resolution
precedence, examples honoring the sandbox), and doc-list sync.

---

# Testing strategy (all phases)

- **TDD per task**: write the failing test(s) named in each item first, then the
  implementation. Pure functions (parsers, path shortening, delta math, manifest
  resolution) get direct unit tests; CLI surfaces get `tests/smoke.rs`
  integration tests.
- **Characterization tests guard byte-identical defaults**: W1/W4/W14 (widget
  output unchanged when new opts unset), W6 (updated baseline), W8 (window pill
  markup unchanged), W26 (wire JSON unchanged + round-trip).
- **`just test` stays hermetic** (no wasm toolchain): all host-side pure logic
  and CLI tests run there. The wasm-e2e (`rl_log` wiring, guest `WireContext`
  parse against a real build) stays behind the opt-in `wasm-e2e` feature /
  `just test-wasm`.
- **Full clippy + fmt** run before each phase-boundary review and before the
  final finish.

# Review checkpoints

One targeted full code review at each of the five phase boundaries (run by the
orchestrator via `superpowers:requesting-code-review`, adversarially verified),
plus a final whole-branch review before `finishing-a-development-branch`. No
user check-in between phases.

# Documentation updates

- CLAUDE.md + README.md widget lists gain `git` and `disk`; plugin list gains
  `counter`/`filewatch`/`httpget`; the module map gains the new
  files/subcommands (`doctor`, `config`, `git.rs`, `disk.rs`, `completions`,
  `plugin new`/`approve`, `rl_log`, `WireContext`).
- Config section documents the new opts (`cwd` shortening, `format`/`icon`,
  `git`, `disk`, `init --binary/--dry-run/--uninstall`).
- On branch finish, strip the 22 in-flight items from `WHATS-NEXT.md` (the
  standing-instruction "shipped items are removed in the change that ships
  them").
