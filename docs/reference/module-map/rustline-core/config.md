# `rustline-core` — `config.rs`

> **Read when:** adding a config option, touching `Config` parsing, widget instances (`[instances.*]`), the layout algebra, or the parse-once `resolved_instances` memo.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

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
