# typecheck execution spec — Critical bucket (T1–T6)

Date: 2026-07-29. Branch: `codehealth/2026-07-26-batch2` @ 7d3ea13.
Source: `TYPECHECK.md` (triage of 2026-07-29); the user checked `[x] execute` on
all six Critical findings and nothing else. This spec contains only those items.

Toolchain (all must be green before every commit):
- build: `cargo build --workspace`
- typecheck: `cargo check --workspace --all-targets --all-features`
- lint: `cargo clippy --all-targets --all-features -- -D warnings` + `cargo fmt --all --check`
- test: `just test` (full suite at milestone boundaries; per-task `cargo test -p <crate>` is fine)

## Execution order and why

Dependency-driven, not ID order:

1. **T2** — `ResolvedPath`/`SandboxRelPath`/`AllowedPath` in `rustline-wasm::state`
   (T3 consumes these as its `ReadPath`/`WritePath` subject types).
2. **T3** — phantom-tagged `AllowSet<K>` + `CanonicalArgv` in `rustline-wasm`
   (builds on T2's path types).
3. **T4** — host-side outcome enums in `rustline-wasm::perform` (same files as
   T2/T3; land after the gates are typed so the outcome refactor doesn't churn).
4. **T1** — `RangeName` in `rustline-core` (render boundary, registration
   boundaries, CLI validators).
5. **T5** — `WidgetKind` enum + parse-once `resolved_instances()` in
   `rustline-core::config` / `widgets`.
6. **T6** — `WidgetName` identity newtype in `rustline-abi`, threaded
   everywhere (largest blast radius; delegates `fits_tmux_range` to T1's
   `RangeName`, and lands last so T1–T5's churn is already settled).

One commit per finding, format `typecheck(<lens>): <summary> [T<n>]`, and each
commit strips its finding from `TYPECHECK.md` (non-negotiable). Renames of
public symbols are never applied — if a migration turns out to require one,
convert that finding to a `decision-needed` marker in `TYPECHECK.md` and skip it.

## Invariants this work depends on (must hold before AND after every task)

- **#2 wire ABI byte-identical:** everything serialized across the WASM boundary
  (`Segment`, `Style`, `Color`, `WireContext.toggled`, the six host-effect
  result structs) must encode/decode to byte-identical JSON. `#[serde(transparent)]`
  newtypes and boundary-only conversions are the tools; nothing gains
  `deny_unknown_fields`.
- **#3 `Config::load` is total:** accepted TOML shapes are unchanged; a bad
  value degrades warn-and-skip, never a config-load failure. Never
  `#[serde(tag = "kind")]` on `Config.instances`.
- **#7 one identity end-to-end:** the range name emitted, the `--range` value,
  the toggled key, and `range_name()`/`active_format` key stay the same string
  for every widget/instance/plugin.
- **#8 sanitize boundary:** `sanitize_text` remains applied at exactly its three
  sites; T1 additionally makes range-name bytes safe by construction.
- **N1 gate-first:** a denied URL/path/argv never reaches a fetch/fs/spawn.
  Denied-case tests exist and must keep passing; T2/T3 strengthen this from
  convention to compile-time fact.
- **N2 never break the bar:** every degradation path (warn-and-skip, empty
  segments, fallback) is preserved behavior, not collateral.

Per the spec-discipline rule ("no test needed because of invariant X" is a red
flag): where a task's recipe pins an invariant (T4's golden JSON, T5's output
strings, T6's wire/TOML shapes), that pinning test is load-bearing and is part
of the task, written BEFORE the migration lands.

## T2. Sandbox path resolution returns bare `String`/`PathBuf` (impact 20, effort M, risk low)

Symbol: `state::resolve_for_allowlist / sanitize_relpath / normalize_abs`
(`crates/rustline-wasm/src/state.rs:110`). ~35 sites across
`state.rs, perform.rs, capability.rs, allow.rs, cache.rs`.

- `pub struct ResolvedPath(String);` — private field; constructible only via
  `resolve_for_allowlist`/`normalize_abs`; carried by
  `PathResolveError::SymlinkDenied` too.
- `pub struct SandboxRelPath(PathBuf);` — only via `sanitize_relpath`.
- `pub struct AllowedPath(ResolvedPath);` — returned by
  `AllowSet::check(&self, ResolvedPath) -> Result<AllowedPath, ResolvedPath>`;
  the allowlist consumes a resolved path and produces the only token the
  filesystem effect accepts.
- The four effect sites (`perform.rs:547` read, `perform.rs:601` write, and the
  two `ctx.state_dir().join(rel)` sites at `perform.rs:421/452`) take
  `&AllowedPath`/`&SandboxRelPath`, making "resolve → match → act" a type-level
  fact instead of statement order.
- `SymlinkDenied` → `observe_denial` routing unchanged. No wire/TOML/guest
  change. Existing denied-case tests must keep passing unchanged in meaning.

## T3. Four interchangeable `AllowSet` fields on CapabilityCtx (impact 20, effort L, risk low)

Symbol: `CapabilityCtx.allowed_urls / allowed_paths / allowed_write_paths /
allowed_commands` (`crates/rustline-wasm/src/capability.rs:64`). ~42 sites
across `capability.rs, allow.rs, perform.rs, argv.rs, denials.rs, cache.rs`.

- Phantom-tagged `pub struct AllowSet<K: CapKind>(Vec<Pattern>, PhantomData<K>);`
  with zero-sized markers `Url`, `ReadPath`, `WritePath`, `Command`; each
  `impl CapKind` carries its `DenialKind` so `observe_denial`'s second
  hand-paired argument becomes `K::DENIAL`.
- Subject newtypes per marker: `Url<'a>(&'a str)`, T2's `ResolvedPath`, and
  `CanonicalArgv(String)` (sole constructor `CanonicalArgv::of(program, args)`,
  replacing `canonical_argv`'s bare-`String` return — the old `pub fn` may stay
  as a thin wrapper to avoid a rename).
- `CanonicalArgv` is required by both `AllowSet<Command>::allows` AND the exec
  cache key, so `allows(program)` (whole-argv-gate bypass) and
  `cache_path(.., program)` (cache-entry collapse) stop compiling.
- `CapabilityCtx::from_config` then only typechecks when each field is fed its
  own `PluginConfig` list. No serde anywhere; `PluginConfig` TOML untouched.

## T4. `perform_*` hand-assemble flat wire structs at ~37 sites (impact 20, effort L, risk low)

Symbol: the eight `perform_*` fns (`crates/rustline-wasm/src/perform.rs:22`).
43 sites across `perform.rs`, `host.rs`, `tests/e2e.rs`.

- Host-internal outcome enums ONLY (wire structs in `rustline-abi` keep their
  flat shape + struct-level `#[serde(default)]`): `HttpOutcome`,
  `CachedHttpOutcome { Denied, Fresh, Backoff, Refreshed, ServedStale,
  NoUsableAnswer }` (per-variant data), `ExecOutcome` + cached analogue,
  `ReadOutcome { Denied, Found(String), Absent, Failed(String) }`,
  `WriteOutcome { Denied, Written, Failed(String) }`.
- One `impl From<XOutcome> for XResult` per pair — the single author of the
  `ok`/`error`/`stale` flag combinations (today's ~37 hand-written literals
  have already diverged in convention between `CachedHttpResult` and
  `ExecResult`).
- Each `pub fn perform_*` keeps its exact signature; the logic moves to an inner
  `fn ..._outcome(...) -> XOutcome` and returns `outcome.into()`.
- **Load-bearing test, written first:** a round-trip test per pair
  (`XOutcome -> XResult -> serde_json`) pinning the JSON against golden strings
  captured from today's behavior, proving the guest-visible bytes are unchanged.

## T1. Name rule re-checked at six sites with three strictnesses (impact 25, effort L, risk medium)

Symbol: `RangeGroup.range / Widget::range_name / clickable_range /
plugin_range_name / validate_plugin_name / validate_install_name`
(`crates/rustline-core/src/render.rs:257`). 34 sites across 24 files.

- `pub struct RangeName(String);` in `rustline-core` beside
  `RANGE_NAME_MAX_BYTES`; `RangeName::parse(&str) -> Result<RangeName, NameError>`
  enforcing the whole invariant-#7 rule once (non-empty, ≤ 15 bytes,
  `[A-Za-z0-9_-]` only, not `window`); `Deref<Target=str>`/`Display`/
  `AsRef<str>`; NO other constructor.
- Parse at the three real boundaries: (a) `Registry::with_builtins`'
  `[instances.<name>]` pass — today length-only, no charset check, so
  `[instances."a#[norange]"]` registers and is interpolated unescaped into
  `#[range=user|{name}]`, forging another widget's clickable range; (b)
  `register_plugins`' `.wasm` stem check (`rustline-wasm/src/lib.rs:230`); (c)
  the two CLI validators — collapse `validate_plugin_name` (hard-error) and
  `validate_install_name` (warn) onto `RangeName::parse`, keeping install's
  warn-don't-refuse by matching `NameError::TooLong`.
- `Widget::range_name(&self) -> Option<&RangeName>`;
  `RangeGroup { range: Option<RangeName> }`; `render_region_ranged` takes it.
  `clickable_range`/`plugin_range_name` reduce to the `alt.is_empty()` gate +
  pass-through.
- Unparseable instance name = warn-and-skip, never a config-load failure.
  Accepted TOML keys unchanged. `plugin_index.rs:335-358`'s hand-rolled
  re-assertion becomes `RangeName::parse(&e.name).is_ok()`.
- **Risk medium → characterization first:** before migrating, add tests pinning
  (1) today's warn-and-skip on an over-long instance name / plugin stem, and
  (2) a NEW denied-case test that a charset-violating instance name does NOT
  reach `#[range=user|…]` output after the change (the security payoff).

## T5. Widget `kind` is a bare string across five modules (impact 20, effort XL, risk medium)

Symbol: `Config::instance_kind / instance_meta / instance_opts /
Registry::with_builtins / WidgetSource::Instance`
(`crates/rustline-core/src/config.rs:1794`). ~120 sites across 13 files.

Three layers, one commit:
1. **The enum.** Closed 16-value `WidgetKind` with
   `#[serde(rename_all = "snake_case")]` plus the two load-bearing explicit
   renames `#[serde(rename = "datetime")]` and `#[serde(rename = "loadavg")]`
   (snake_case alone would emit `date_time`/`load_avg` — NOT the accepted TOML
   spellings); `ALL: [WidgetKind; 16]`, `as_str()`, `parse()`,
   `is_instanceable()`. TOML shape unchanged: `Config.instances` stays
   `HashMap<String, toml::Value>`, `kind` stays a raw string;
   `instance_kind` returns `Option<WidgetKind>` so unknown kinds hit the SAME
   warn-and-skip arm; `Config::load` stays total (never `#[serde(tag)]`).
2. **Parse-once.** `Config::resolved_instances() -> &BTreeMap<String, InstanceParse>`
   backed by `OnceLock`; `InstanceSpec` = 12-variant enum
   (`DateTime(DateTimeOpts) … Throughput(ThroughputOpts)`) built by ONE pass of
   the existing `instance_opts` dispatch, `NoKind`/`UnknownKind` preserving
   warn-once degradation. Point `color_overrides`/`click_map`/`layout_kinds`/
   `disk_mounts`/`throughput_interfaces`/`spark_referenced_in_layout` at it;
   delete `instance_meta`'s duplicate 12-arm match. (Today `widget_tui.rs:549`
   re-runs serde per instance per keystroke; `click.rs:84` rebuilds the click
   map per click.)
3. **Typed params.** `instance_meta(name, kind: WidgetKind)`,
   `instances_of_kind`, `spark_referenced_in_layout`, `instance_descriptor`
   take `WidgetKind`; `layout_kinds -> BTreeSet<WidgetKind>`;
   `WidgetSource::Instance { kind: WidgetKind }` (not Serialize — no wire
   concern); `build_context.rs`/`doctor.rs` gates become
   `kinds.contains(&WidgetKind::Git)`.

Deletes `BUILTIN_WIDGET_NAMES`/`is_builtin_widget_name` (replaced by
`WidgetKind::parse(name).is_some()`), retiring the sync characterization test at
`config.rs:3091`. **Load-bearing pins, written first:** output strings stay
byte-identical — `format!("instance of {}", …)`, `"instance:{}"`,
`"{} instance"` — and the TOML spellings `datetime`/`loadavg` round-trip through
the enum.

## T6. Widget identity is a bare `String` threaded through every crate (impact 20, effort XL, risk low)

Symbol: `Registry::build / Layout.left / Context.toggled / RangeGroup.range`
(`crates/rustline-core/src/widget.rs:65`). 70 sites across 27 files.

- `#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize,
  Deserialize)] #[serde(transparent)] pub struct WidgetName(String);` in
  `rustline-abi` (host and guest share it), with `From<&str>`, `Display`,
  `AsRef<str>`, and `fits_tmux_range(&self) -> bool` delegating to T1's
  `RangeName::parse`.
- `#[serde(transparent)]` is load-bearing twice: `WireContext.toggled`/
  `Context.toggled` keep serializing as a JSON array of plain strings
  (invariant #2), and `Layout.{left,center,right}: Vec<WidgetName>` plus the
  `plugins`/`instances` map keys keep the accepted TOML shape unchanged.
- Thread through `Registry::{register, build, contains, resolve}`,
  `toggle::{active_format, clickable_range}`, `render_named_region`'s
  names/overrides, `Widget::range_name`, `toggles::{parse_toggles,
  apply_toggle}`, `click::{resolve_click, dispatch, ClickExecutor::toggle}`.
- The concrete swap made uncompilable: `active_format(ctx, name, format, alt)` —
  identity followed by two format-template `&str`s, called from twelve widget
  modules.
- Renames are NOT part of this; every field/variant name stays as-is.
- **Load-bearing pins, written first:** wire JSON for `WireContext.toggled` and
  TOML round-trip for `Layout`/`[plugins.*]`/`[instances.*]` byte-identical
  before/after.

## Milestones

- After T2+T3+T4 (the rustline-wasm cluster): full `just test` +
  `just lint` + `just check-lock`; optionally `just test-wasm` if the wasm
  target is available (these tasks touch the host/guest seam).
- After T1+T5+T6 (the core/bin cluster): full suite again, plus
  `cargo fmt --all --check`.
- Every commit strips its finding from `TYPECHECK.md`.
