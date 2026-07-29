# TYPECHECK.md — type-system strengthening findings

Last triage: 2026-07-29 against `codehealth/2026-07-26-batch2` @ 7d3ea13. Toolchain: cargo build --workspace / cargo check --workspace --all-targets --all-features / just test.

> **For future sessions reading this file:** when you fix an item listed
> here, strip it from this file in the same commit that fixes it. The list
> is intended to reflect open issues only; resolved items shouldn't linger.
> This keeps the file's signal-to-noise high for the next typecheck pass.

## How to use this file
- Check `[x] execute` on items to run this batch.
- Check `[x] skip` on items to never re-flag (the skill records them in user memory).
- Items left unchecked stay in TYPECHECK.md for the next run.
- Ranking is impact = bug-prevention × blast-radius (effort is shown separately, never folded into the rank).
- When ready, run `/typecheck --execute`.

## Critical

### T1. The widget/plugin/instance name rule is re-checked at six sites with three strictnesses instead of parsed once: `RangeGroup.range / Widget::range_name / clickable_range / plugin_range_name / validate_plugin_name / validate_install_name` (crates/rustline-core/src/render.rs:257)
- Lens: parse-dont-validate
- Impact: 25 (bug-prevention 5 × blast-radius 5)
- Effort: L (34 sites, public-API: yes)
- Risk: medium
- Blast radius: 34 sites across 24 files (crates/rustline-core/src/{render.rs, widget.rs, widgets/toggle.rs, widgets/mod.rs, the 12 clickable widget modules, assemble.rs}; crates/rustline-wasm/src/{host.rs, lib.rs}; crates/rustline/src/{plugin_cmd.rs, plugin_install.rs, click.rs, toggles.rs, plugin_index.rs})
- Proposed type: `pub struct RangeName(String);` in rustline-core beside `RANGE_NAME_MAX_BYTES`, with `RangeName::parse(&str) -> Result<RangeName, NameError>` enforcing the whole invariant-#7 rule once (non-empty, ≤ 15 bytes, `[A-Za-z0-9_-]` only, not the reserved `window`), `Deref<Target=str>`/`Display`/`AsRef<str>`, and NO other constructor. Parse at the three real boundaries: (a) `Registry::with_builtins`' `[instances.<name>]` pass (widgets/mod.rs:380-397) — today it warns on length only and never checks the charset, so a TOML key like `[instances."a#[norange]"]` (11 bytes) registers and is written *unescaped* into `#[range=user|{name}]` at render.rs:304, forging another widget's clickable range — the exact attack `sanitize_text` (invariant #8) defends against for segment text; (b) `register_plugins`' `.wasm` stem check (rustline-wasm/src/lib.rs:230, same length-only warn over an arbitrary filename); (c) the two CLI validators that already implement the rule twice with different outcomes — collapse `validate_plugin_name` (hard-error) and `validate_install_name` (warn) onto `RangeName::parse`, keeping install's warn-don't-refuse by matching `NameError::TooLong`. `Widget::range_name(&self) -> Option<&RangeName>`; `RangeGroup { range: Option<RangeName> }`; `render_region_ranged` takes it so the compiler, not a doc comment, says the interpolated bytes are safe. `clickable_range`/`plugin_range_name` (two copies of the same length comparison) reduce to the `alt.is_empty()` gate + pass-through. An unparseable instance name is warn-and-skip, never a config-load failure (invariant #3); accepted TOML keys unchanged. plugin_index.rs:335-358's hand-rolled re-assertion of the same four rules becomes `RangeName::parse(&e.name).is_ok()`. Relation to T6: RangeName is the *validated clickable* identity; T6's `WidgetName` is the plain identity newtype — if both execute, `WidgetName::fits_tmux_range` delegates to `RangeName::parse`.
- [x] execute   [ ] skip

### T4. Host-side `perform_*` effect functions have no typed outcome; the flat ok/status/error wire structs are hand-assembled at ~37 sites: `perform_http_get / perform_http_get_cached / perform_exec / perform_exec_cached / perform_state_read / perform_state_write / perform_file_read / perform_file_write` (crates/rustline-wasm/src/perform.rs:22)
- Lens: illegal-states
- Impact: 20 (bug-prevention 4 × blast-radius 5)
- Effort: L (43 sites, public-API: yes)
- Risk: low
- Blast radius: 43 sites across 3 files (crates/rustline-wasm/src/perform.rs, crates/rustline-wasm/src/host.rs, crates/rustline-wasm/tests/e2e.rs)
- Proposed type: host-internal sum types in rustline-wasm only, converting to the EXISTING wire structs at the boundary — the JSON must stay byte-identical; `HttpResult`/`ExecResult`/`ReadResult`/`WriteResult`/`CachedHttpResult`/`CachedExecResult` in rustline-abi keep their flat shape and struct-level `#[serde(default)]` untouched so older guests keep decoding. `enum HttpOutcome { Denied { url }, Completed { status, body }, TransportFailed { error } }`; `enum CachedHttpOutcome { Denied, Fresh, Backoff, Refreshed, ServedStale, NoUsableAnswer }` (with per-variant data); `enum ExecOutcome { Denied { candidate }, Ran { status, stdout, stderr, truncated }, CouldNotRun { error } }` (+ cached analogue); `enum ReadOutcome { Denied, Found(String), Absent, Failed(String) }`; `enum WriteOutcome { Denied, Written, Failed(String) }`. Each gets one `impl From<XOutcome> for XResult` — the single author of the `ok`/`error`/`stale` flag combinations, deleting ~37 hand-written literals whose conventions already diverge (`CachedHttpResult`'s backoff return sets `ok: true, stale: true` with a non-empty `error` while `ExecResult`'s doc promises `error` is non-empty iff `!ok` — enforced today only by prose). Each `pub fn perform_*` keeps its signature; logic moves to an inner `fn ..._outcome(...) -> XOutcome` + `outcome.into()`. A round-trip test (`XOutcome -> XResult -> serde_json`) pins the JSON against today's golden strings. The guest/SDK half of this boundary is T17.
- [x] execute   [ ] skip

### T5. Widget `kind` is a bare string: per-kind dispatch re-implemented in five modules, instance tables re-deserialized per consumer, and name/kind params freely swappable: `Config::instance_kind / instance_meta / instance_opts / Registry::with_builtins / WidgetSource::Instance` (crates/rustline-core/src/config.rs:1794)
- Lens: stringly-enum (merged with newtype, illegal-states, parse-dont-validate)
- Impact: 20 (bug-prevention 4 × blast-radius 5)
- Effort: XL (~120 sites, public-API: yes)
- Risk: medium
- Blast radius: ~120 sites across 13 files (crates/rustline-core/src/{config.rs, widget.rs, widgets/mod.rs}; crates/rustline/src/{build_context.rs, doctor.rs, widget_cmd.rs, widget_tui.rs, click.rs, theme_cmd.rs, daemon.rs, main.rs, bench/daemon.rs, bench/render_passes.rs})
- Proposed type: closed 16-value `#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)] #[serde(rename_all = "snake_case")] pub enum WidgetKind { PaneId, Hostname, Windows, #[serde(rename = "datetime")] DateTime, Cwd, LanIp, TailscaleIp, Battery, Cpu, Memory, #[serde(rename = "loadavg")] LoadAvg, Git, Disk, Uptime, Media, Throughput }` (the two explicit renames are load-bearing: snake_case would emit `date_time`/`load_avg`, which are NOT the accepted TOML spellings) + `ALL: [WidgetKind; 16]`, `as_str()`, `parse()`, `is_instanceable()`. TOML shape unchanged: `Config.instances` stays `HashMap<String, toml::Value>` and `kind` stays a raw TOML string — `instance_kind` returns `Option<WidgetKind>` via `WidgetKind::parse`, so an unknown kind funnels into the SAME warn-and-skip arm as today and `Config::load` stays total (invariant #3; do NOT put `#[serde(tag = "kind")]` on the field — one typo'd kind would discard the whole config). Second layer (parse-once, from the lens-3/lens-4 halves of this cluster): `Config::resolved_instances() -> &BTreeMap<String, InstanceParse>` backed by a `OnceLock`, where `InstanceSpec` is a 12-variant enum (`DateTime(DateTimeOpts) … Throughput(ThroughputOpts)`) built by ONE pass of the existing `instance_opts` dispatch, with `NoKind`/`UnknownKind` preserving the warn-once degradation; point `color_overrides`/`click_map`/`layout_kinds`/`disk_mounts`/`throughput_interfaces`/`spark_referenced_in_layout` at it, deleting `instance_meta`'s duplicate 12-arm match — today the same table is re-`try_into`'d at up to ten sites (widget_tui.rs:549 re-runs serde per instance per *keystroke*; click.rs:84 rebuilds the whole click_map per click), and two hand-maintained 12-arm dispatch tables must stay in sync or a kind registers while silently losing its color/click. Third layer (from the lens-2 half): typed `kind` params (`instance_meta(name, kind: WidgetKind)`, `instances_of_kind`, `spark_referenced_in_layout`, `instance_descriptor`) make the adjacent name/kind swap uncompilable; `layout_kinds -> BTreeSet<WidgetKind>`; `WidgetSource::Instance { kind: WidgetKind }` (not Serialize — no wire concern); build_context.rs/doctor.rs gates become `kinds.contains(&WidgetKind::Git)`. Deletes the hand-maintained `BUILTIN_WIDGET_NAMES: [&str; 16]`/`is_builtin_widget_name` mirror (replaced by `WidgetKind::parse(name).is_some()`), retiring the sync characterization test at config.rs:3091. Output strings stay byte-identical: keep `format!("instance of {}", kind.as_str())`, `"instance:{}"`, and `"{} instance"` for the descriptor summary.
- [x] execute   [ ] skip

### T6. The widget identity (registry key = layout entry = tmux range name = toggle key = click/override map key) is a bare `String` threaded through every crate: `Registry::build / Layout.left / Context.toggled / RangeGroup.range` (crates/rustline-core/src/widget.rs:65)
- Lens: newtype
- Impact: 20 (bug-prevention 4 × blast-radius 5)
- Effort: XL (70 sites, public-API: yes)
- Risk: low
- Blast radius: 70 sites across 27 files (crates/rustline-abi/src/lib.rs; crates/rustline-core/src/{widget.rs, config.rs, context.rs, render.rs, assemble.rs, widgets/toggle.rs, the 12 clickable widget modules}; crates/rustline-wasm/src/host.rs; crates/rustline/src/{toggles.rs, click.rs, plugin_index.rs, plugin_install.rs, widget_cmd.rs, widget_tui.rs}; crates/rustline-plugin-sdk/src/lib.rs)
- Proposed type: `#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)] #[serde(transparent)] pub struct WidgetName(String);` in rustline-abi (so host and guest share it), with `From<&str>`, `Display`, `AsRef<str>`, and `fits_tmux_range(&self) -> bool` folding in `RANGE_NAME_MAX_BYTES` (delegate to T1's `RangeName::parse` if both execute). `#[serde(transparent)]` is load-bearing twice: `WireContext.toggled`/`Context.toggled` keep serializing as a JSON array of plain strings (byte-identical wire, invariant #2), and `Layout.left/center/right` as `Vec<WidgetName>` plus the `plugins`/`instances` map keys as `HashMap<WidgetName, _>` keep the accepted TOML shape (`left = ["cpu", "git"]`, `[plugins.weather]`) unchanged. Thread through `Registry::{register, build, contains, resolve}`, `toggle::{active_format, clickable_range}`, `assemble::render_named_region`'s names/overrides, `Widget::range_name`, `toggles::{parse_toggles, apply_toggle}`, and `click::{resolve_click, dispatch, ClickExecutor::toggle}`. The concrete swap this makes uncompilable: `active_format(ctx, name, format, alt)` takes an identity followed immediately by two format-template `&str`s, called identically from twelve widget modules — passing `&self.format` where `&self.name` belongs compiles today and silently makes a widget un-toggleable while emitting a bogus `#[range=user|…]` (invariant #7's silent-failure mode). Renames are NOT part of this — every field/variant name stays as-is.
- [x] execute   [ ] skip

## High

### T7. State-quota accounting passes four role-distinct `u64` byte counts positionally — the measured size and the configured cap are freely swappable: `check_cap / write_entry / projected_size` (crates/rustline-wasm/src/state.rs:228)
- Lens: newtype
- Impact: 16 (bug-prevention 4 × blast-radius 4)
- Effort: M (43 sites, public-API: yes)
- Risk: low
- Blast radius: 43 sites across 5 files (crates/rustline-wasm/src/{state.rs, cache.rs, capability.rs, perform.rs}, crates/rustline-core/src/config.rs)
- Proposed type: `#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)] pub struct StateSize(u64);` (a measured total, with `saturating_add/sub` and `exceeds(Quota) -> bool`), `#[serde(transparent)] pub struct Quota(u64);` (the configured cap — `PluginConfig.max_state_bytes` becomes `Quota` keeping the existing `#[serde(default = "default_max_state_bytes")]`, so `max_state_bytes = N` TOML is unchanged), and a `ByteLen` for content lengths. `check_cap(current: StateSize, target: &Path, new_len: ByteLen, cap: Quota)` and `write_entry(.., current: StateSize, cap: Quota) -> Result<StateSize, String>`. Today the last two arguments of both functions are bare `u64`s of opposite meaning: `check_cap(cap, target, new_len, current)` compiles and inverts the quota decision (invariant N3), and the test call sites already pass raw literals in exactly that position. `CapabilityCtx::state_size()/set_state_size()/max_state_bytes` hand back/accept the newtypes; keep the `AtomicU64` memo internal — its sentinel representation is T23's separately-scoped finding.
- [ ] execute   [ ] skip

### T8. RenderArgsWire unions two disjoint argument sets behind a RegionKind tag, so the daemon silently defaults a required window index: `RenderArgsWire` (crates/rustline/src/daemon_proto.rs:82)
- Lens: illegal-states
- Impact: 16 (bug-prevention 4 × blast-radius 4)
- Effort: L (28 sites, public-API: no)
- Risk: medium
- Blast radius: 28 sites across 5 files (crates/rustline/src/{daemon_proto.rs, main.rs, daemon.rs, daemon_client.rs, bench/daemon.rs})
- Proposed type: `pub enum RenderTarget { Region { dir: RegionDir /* Left|Right */, session: Option<String>, window: Option<String>, pane: Option<String>, pane_path: Option<String> }, Window { index: String, name: String, flags: String, current: bool } }` (window fields plain `String`, matching `cli::WindowArgs` where they are required/defaulted-to-empty — not `Option`), carried by a new `DaemonRequest::RenderV3 { protocol: u32, target: RenderTarget, preview: bool }`; bump `DAEMON_PROTOCOL` to 2. This is exactly the change `DAEMON_PROTOCOL`'s doc was written for: an old daemon cannot deserialize the new variant, the frame errors, that connection drops, and `try_render`'s `None` falls back to the in-process render — fail-closed by construction, the same mechanism the RenderV2 tests already pin. The three `main.rs` call sites stop blanking the other kind's fields with `..RenderArgsWire::default()` and name the variant they mean; `daemon.rs`'s `args.index.clone().unwrap_or_default()` triple disappears — today a client that forgets `index` gets an empty window index rendered into the bar rather than a rejected request. `Ping`/`Shutdown` stay version-free unit variants (their doc explains why). `RenderV2` can be retained one release for graceful skew, or dropped immediately (the client already falls back on any decode failure).
- [ ] execute   [ ] skip

### T9. A manifest's four requested-capability lists and the four config allowlists are all `Vec<String>`, paired only positionally when `plugin approve` writes grants: `PluginManifest.requested_urls / requested_paths / requested_write_paths / requested_commands` (crates/rustline-wasm/src/manifest.rs:36)
- Lens: newtype
- Impact: 15 (bug-prevention 5 × blast-radius 3)
- Effort: M (31 sites, public-API: yes)
- Risk: low
- Blast radius: 31 sites across 5 files (crates/rustline-wasm/src/{manifest.rs, capability.rs, allow.rs}, crates/rustline-core/src/config.rs, crates/rustline/src/plugin_cmd.rs)
- Proposed type: `#[serde(transparent)]` kind-tagged pattern newtypes — `UrlPattern(String)`, `PathPattern<ReadPath>`, `PathPattern<WritePath>`, `CommandPattern(String)`, or one generic `#[serde(transparent)] pub struct Pattern<K: CapKind>(String, PhantomData<K>)` reusing T3's markers. `#[serde(transparent)]` keeps both the manifest TOML (`requested_urls = [...]`) and `[plugins.<name>]`'s allowlists byte-identical (a transparent newtype over `String` deserializes from a plain TOML string, so `Config::load` totality is untouched). The caught bug: `write_grants` (plugin_cmd.rs:549-576) hand-pairs a `Kind` key with a `Vec<String>` four times in a row — `append_unique(allowlist_array(table, plugin, Kind::WritePath.key()), &m.requested_urls)` typechecks today, silently turning an approved read/URL request into a write grant, precisely the exploit the `allowed_paths`/`allowed_write_paths` split was introduced to close. Make `allowlist_array`/`append_unique` generic over `K` so the kind and the list are one argument; `AllowSet::<K>::compile(&[Pattern<K>])` closes the same loop at `CapabilityCtx::from_config`. Pairs with T14 — if both execute, `Pattern<K>`'s constructor is the natural place to also prove compilability.
- [ ] execute   [ ] skip

### T10. `Color::Named(String)` payload is a 16-name closed set matched in two hand-mirrored render tables that nothing pins together: `abi::Color::Named(String)` (crates/rustline-abi/src/lib.rs:36)
- Lens: stringly-enum
- Impact: 12 (bug-prevention 4 × blast-radius 3)
- Effort: M (27 sites, public-API: yes)
- Risk: high
- Blast radius: 27 sites across 6 files (crates/rustline-abi/src/lib.rs, crates/rustline-core/src/{ansi.rs, assemble.rs, config.rs}, crates/rustline/src/{widget_tui.rs, theme_cmd.rs})
- Proposed type: `pub enum NamedColor { Black, Red, Green, Yellow, Blue, Magenta, Cyan, White, BrightBlack, … BrightWhite }` in rustline-abi with `as_str()`/`parse()`; `Color::Named(NamedColor)`. WIRE-CRITICAL: `Color` crosses the WASM ABI (`Style.fg`/`bg` on every `Segment`, `ThemeColors`) and is the TOML theme shape (`{ Named = "cyan" }`) — the encoding must stay byte-identical (`rename_all = "lowercase"` concatenates without separators, so `BrightBlack` → `"brightblack"`; `to_tmux()` keeps returning the same spec). FORWARD-COMPAT IS LOAD-BEARING: a derived `Deserialize` would REJECT an unknown name, and a guest sending `{"Named":"chartreuse"}` would fail the whole `GuestRender` decode and break the bar (invariant N2) — so `NamedColor` needs hand-written serde with an `Other(String)` catch-all (or `#[serde(untagged)]` Known/Other split), preserving today's lenient behavior while the two independently hand-maintained mirrors — `ansi.rs::named_sgr` (8 arms + `bright` prefix strip) and `widget_tui.rs::to_ui_color` (16 arms, its own doc admits it "mirrors ansi.rs exactly" with NO pinning test) — become exhaustive matches plus one explicit `Other` arm (today, adding a name to `named_sgr` silently renders as `UiColor::Reset` in the `widget edit` preview). Also flip the construction site ansi.rs:193 to `NamedColor::parse(spec)`, removing the "construct a Named only if `named_sgr` happened to accept it" coupling.
- [ ] execute   [ ] skip

### T11. Rendered tmux markup and un-sanitized widget text are both bare `String`, so the sanitize boundary (invariant #8) is enforced only by convention: `render_region / render_named_region / DaemonResponse::Markup` (crates/rustline-core/src/render.rs:194)
- Lens: newtype
- Impact: 12 (bug-prevention 4 × blast-radius 3)
- Effort: M (29 sites, public-API: yes)
- Risk: low
- Blast radius: 29 sites across 6 files (crates/rustline-core/src/{render.rs, assemble.rs, ansi.rs}, crates/rustline/src/{daemon_proto.rs, daemon.rs, main.rs})
- Proposed type: `#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)] #[serde(transparent)] pub struct TmuxMarkup(String);` constructible only inside `render.rs`/`assemble.rs` (private field + `pub(crate)` constructor), with `into_inner()`/`Display` for the emit path. The five `-> String` renderers return it; `DaemonResponse::Markup(TmuxMarkup)` keeps its JSON frame byte-identical via `#[serde(transparent)]` — required, that frame is the versioned daemon wire protocol; `tmux_to_ansi` and `main.rs`'s `emit` take `TmuxMarkup`, so the preview path can't be handed raw widget text. Closes a gap the code currently documents in prose: `render_windows` interpolates `wc.index` into `#[range=window|{}]` unescaped (assemble.rs:187-197) with a comment that a future front-end populating `WindowCtx.index` from anywhere else "must uphold that or route through the sanitizer" — a type makes that a compile error instead of a caveat. `Segment.text` stays a plain `String` in rustline-abi (guest-supplied content; exact JSON shape required) — the newtype lives only on the host's output side.
- [ ] execute   [ ] skip

### T12. Cache time values are untyped: TTL vs age as bare `i64`s, guest `ttl_secs` collapses to 0 via `unwrap_or`, and the RFC3339 `now` is re-parsed up to ten times per call: `cache::is_fresh / cache::age_secs / host_fn rl_http_get_cached / rl_exec_cached` (crates/rustline-wasm/src/cache.rs:82)
- Lens: newtype (merged with parse-dont-validate)
- Impact: 12 (bug-prevention 4 × blast-radius 3)
- Effort: M (~45 sites, public-API: yes)
- Risk: medium
- Blast radius: ~45 sites across 5 files (crates/rustline-wasm/src/{cache.rs, perform.rs, host.rs}, crates/rustline-abi/src/lib.rs, crates/rustline-plugin-sdk/src/lib.rs; plugins/cmdrun as evidence)
- Proposed type: `#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)] pub struct AgeSecs(i64);` and `#[serde(transparent)] pub struct TtlSecs(i64);` with `AgeSecs::is_fresh(self, ttl: TtlSecs) -> bool` — today `is_fresh(ttl_secs, age)` compiles and inverts every freshness decision, and the four call sites alternate between an entry age and a since-last-attempt age, visually interchangeable. Parse `ttl_secs` ONCE at the host boundary (host.rs:33/84) with an explicit refusal (`ok: false, error: "invalid ttl_secs"`, the same never-fatal shape `decode_args` already uses for malformed `args_json`) instead of `unwrap_or(0)` — a documented defect (cache.rs:36-41): a guest sending garbage silently loses the `last_attempt_at` backoff and pays a live fetch or a real subprocess spawn on every render forever. Parse `now` once into `chrono::DateTime<FixedOffset>` at the same two host_fn bodies; `age_secs(now, fetched)` takes typed instants with no `Option` and no failure mode (today it re-parses BOTH string arguments at each of ~8 perform.rs call sites, and an unparseable `now` silently routes everything to the treat-as-stale branch). Wire unchanged: both host fns keep their three string params (existing compiled guests keep working); `CachedHttpResult.age_secs`/`CachedExecResult.age_secs` may become `AgeSecs` via `#[serde(transparent)]` (JSON stays a bare number, which the ABI requires). Also unify the SDK's disagreeing wrapper types (`http_get_cached(ttl_secs: i64)` vs `exec_cached(ttl_secs: u64)` for the same host-side value). Behavior note: the explicit ttl refusal is a deliberate guest-visible change for malformed input only. Cross-refs: T18 (typed `CacheEntry` timestamps), T25 (reuse `TtlSecs` in plugin_index if hoisted somewhere both crates see).
- [ ] execute   [ ] skip

### T13. sha256 digests travel as bare `String`/`&str`, adjacent to plugin-name strings at the two sites that record them: `sha256_hex / write_checksum / PluginConfig.checksum` (crates/rustline-wasm/src/integrity.rs:17)
- Lens: newtype
- Impact: 12 (bug-prevention 4 × blast-radius 3)
- Effort: M (30 sites, public-API: yes)
- Risk: low
- Blast radius: 30 sites across 7 files (crates/rustline-wasm/src/{integrity.rs, lib.rs}, crates/rustline-core/src/config.rs, crates/rustline/src/{plugin_checksum.rs, plugin_install.rs, plugin_cmd.rs})
- Proposed type: `#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)] #[serde(transparent)] pub struct Sha256Hex(String);` with a private field, `Sha256Hex::of(bytes)` as the computing constructor and `Sha256Hex::parse_recorded(&str) -> Option<Self>` (the current `normalize`: trim, optional case-insensitive `sha256:` prefix, 64 hex chars). Keep the existing `pub fn sha256_hex` as a thin wrapper returning the newtype so `plugin_install`'s re-export keeps resolving — no public rename. `PluginConfig.checksum: Option<Sha256Hex>` stays transparent-and-PERMISSIVE (do NOT validate inside `Deserialize` — that would change accepted-input semantics and threaten config-load totality; `verify_checksum`'s `Malformed` fail-closed path remains the validator). `ChecksumVerdict::Mismatch { expected: Sha256Hex, actual: Sha256Hex }`. The caught bug: `write_checksum(config_path, checksum, name)` — arguments transposed — compiles today and writes the plugin's own name into the field the load-time gate verifies, which fails closed into `Malformed` and permanently un-loads the plugin. `write_install_record`, `plugin_checksum::status_for`, and `maybe_refresh_stale_checksum` follow.
- [ ] execute   [ ] skip

### T14. Allow-patterns are written into config as unvalidated strings and only compiled at render time, where a malformed one is warn-and-skipped: `PluginConfig.allowed_* / pattern_cmd(Add) / write_grants` (crates/rustline/src/plugin_cmd.rs:442)
- Lens: parse-dont-validate
- Impact: 12 (bug-prevention 3 × blast-radius 4)
- Effort: M (23 sites, public-API: yes)
- Risk: medium
- Blast radius: 23 sites across 6 files (crates/rustline-core/src/config.rs, crates/rustline/src/{plugin_cmd.rs, plugin_install.rs}, crates/rustline-wasm/src/{manifest.rs, allow.rs, capability.rs})
- Proposed type: `#[serde(transparent)] pub struct AllowPattern(String);` in rustline-core, whose `parse(&str) -> Result<AllowPattern, String>` performs the SAME `Glob::new`/`Regex::new` compile `allow::Pattern::compile` does today, discarding the matcher and keeping only the proof it compiles. Today `plugin url add weather 'regex:https://x/.*'` (a plausible typo for the documented `re:` prefix) is accepted, stored, and later compiled as a *glob* that matches nothing — the plugin is silently denied forever; `re:[` is stored then dropped entirely; in both cases the user was already told the grant succeeded. Have `PatternCmd::Add` and `write_grants` call `AllowPattern::parse` first and refuse with the compiler's own error at the moment the user asks. `PluginConfig`'s four allowlists become `Vec<AllowPattern>` with a lenient warn-and-drop `Deserialize` (config load stays never-fatal, invariant #3); TOML stays a bare string via `serde(transparent)`. `AllowSet::compile` takes `&[AllowPattern]` (keep the warn for hand-edited configs). Parse manifests the same way at `resolve_manifest` so `plugin approve` flags an uncompilable request before the user consents. Pairs with T9's kind-tagged `Pattern<K>` — one type family can carry both if executed together.
- [ ] execute   [ ] skip

### T15. Theme names are unparsed strings and `valid_theme_name` guards only 1 of the 6 `themes_dir.join(format!("{name}.toml"))` sites: `theme_cmd::valid_theme_name / ThemeConfig.base / main::resolve_base_theme` (crates/rustline/src/theme_cmd.rs:326)
- Lens: newtype (merged with parse-dont-validate — smart-constructor rule)
- Impact: 12 (bug-prevention 4 × blast-radius 3)
- Effort: M (~26 sites, public-API: yes)
- Risk: high
- Blast radius: ~26 sites across 6 files (crates/rustline/src/{theme_cmd.rs, main.rs, init.rs, cli.rs}, crates/rustline-core/src/{themes.rs, config.rs})
- Proposed type: `pub struct ThemeName(String);` (rustline-core) with `ThemeName::parse(&str)` folding in the existing `valid_theme_name` predicate (non-empty, no `/`, no `\`, no `..`) and a single `fn file_in(&self, dir: &Path) -> PathBuf` as the ONLY way to turn a theme name into a path — deleting the raw `format!("{name}.toml")` idiom so every current and future join goes through the check. Today `valid_theme_name` is called exactly once (from `new_theme`); the other five joins take the name straight from the CLI arg or `[theme].base` — so `rustline theme show ../../some/other` reads and TOML-parses a file outside the themes dir, and `[theme] base = "../.."` does the same on every render. `ThemeConfig.base: Option<ThemeName>` with `#[serde(transparent)]` keeps the TOML byte-identical; parse inside `resolve_base_theme`, falling back to `builtin_theme`/`Theme::default()` with the existing `warn_once` on a rejected name, so no config that loads today stops loading (invariant #3). Risk is high because behavior narrows for escaping names and nothing currently tests `show`/`use`/`resolve_base_theme` with one — write characterization tests first. Known limitation: `new_theme(name, from, …)` takes two theme names in a row and a single `ThemeName` does not catch that swap; the win is the path-escape closure plus one validation site. The builtin-theme registry mirrors are T32, separate.
- [ ] execute   [ ] skip

### T16. LayoutChange encodes add/remove/move as two independent Options, making a `(None, None)` "no change" state constructible: `LayoutChange { from: Option<(Region, usize)>, to: Option<(Region, usize)> }` (crates/rustline-core/src/config.rs:130)
- Lens: illegal-states
- Impact: 12 (bug-prevention 4 × blast-radius 3)
- Effort: L (21 sites, public-API: yes)
- Risk: low
- Blast radius: 21 sites across 3 files (crates/rustline-core/src/config.rs, crates/rustline/src/widget_cmd.rs, crates/rustline/src/widget_tui.rs)
- Proposed type: `pub enum LayoutChange { Added { name: String, to: (Region, usize) }, Removed { name: String, from: (Region, usize) }, Moved { name: String, from: (Region, usize), to: (Region, usize) } }` with `name()`/`to()`/`from()` accessors so the two TUI sites that only want the destination index keep a one-liner. No serde — this type never crosses a wire; the only cost is the cross-crate signature change on the four pub `layout_*` functions, each of which already knows exactly which variant it builds (no `None`/`Some` plumbing survives). `widget_cmd::describe`'s `(None, None) => "{name}: no change"` arm — dead today, reachable only because the type permits it — is deleted by the compiler rather than kept as defensive filler; the three config.rs unit tests asserting `change.from == None` become variant assertions.
- [ ] execute   [ ] skip

## Medium

### T17. The plugin SDK hands guests the flat ok/exists/status wire structs, so every plugin re-derives success with an ad-hoc bool conjunction: `rustline_plugin_sdk::{http_get, http_get_cached, state_read, state_write, file_read, file_write, exec, exec_cached}` (crates/rustline-plugin-sdk/src/lib.rs:315)
- Lens: illegal-states
- Impact: 9 (bug-prevention 3 × blast-radius 3)
- Effort: M (11 SDK sites + 7 evidence sites in plugins/*, public-API: yes)
- Risk: low
- Blast radius: 11 sites in crates/rustline-plugin-sdk/src/lib.rs (+ plugins/httpget, plugins/filewatch, plugins/counter, plugins/cmdrun as evidence — excluded workspace members)
- Proposed type: guest-side enums converting FROM the unchanged wire structs: `HttpOutcome { Response { status, body }, Denied { message }, TransportFailed { message } }` (with `success_body(self) -> Option<String>`, 2xx only), `FileOutcome { Found(String), Absent, Failed(String) }`, `ExecOutcome { Ran { status, stdout, stderr, truncated }, CouldNotRun(String) }`, `Freshness { Fresh, Stale { age_secs } }` — each `impl From<XResult>`, added as typed wrappers (`http_get_typed`, …) alongside the existing decoders. Purely additive: no existing plugin breaks, wire JSON untouched, raw structs stay re-exported. The evidence the flat shape pushes work onto every guest: httpget re-derives 2xx-ness inline, filewatch/counter each write `r.ok && r.exists`, and cmdrun hand-rolls its own `struct Outcome` with two `From` impls just to normalize `ExecResult`/`CachedExecResult` — exactly this SDK type, currently duplicated in a guest. Guest half of T4's boundary.
- [ ] execute   [ ] skip

### T18. CacheEntry uses an empty string as the "never attempted" sentinel for `last_attempt_at` instead of an Option: `CacheEntry { fetched_at: String, last_attempt_at: String }` (crates/rustline-wasm/src/cache.rs:24)
- Lens: illegal-states
- Impact: 9 (bug-prevention 3 × blast-radius 3)
- Effort: M (14 sites, public-API: yes)
- Risk: low
- Blast radius: 14 sites across 2 files (crates/rustline-wasm/src/cache.rs, crates/rustline-wasm/src/perform.rs)
- Proposed type: `CacheEntry { fetched_at: Rfc3339, status: u16, body: String, #[serde(default, skip_serializing_if = "Option::is_none")] last_attempt_at: Option<Rfc3339> }` where `Rfc3339` is a thin newtype over `chrono::DateTime<FixedOffset>` with string encoding — on-disk JSON unchanged for `fetched_at`, and `last_attempt_at` simply absent (rather than `""`) when never attempted; a custom `Deserialize` maps `""` → `None` so entries written by today's build stay readable (matching the existing back-compat test; the cache file is disposable by design, N2, so no further wire dance). Payoff: today `age_secs(now, &e.last_attempt_at)` swallows both "never attempted" and "unparseable garbage" into the same `None`, so the negative-cache backoff silently does not fire in either case — the exact retry-storm the field was added to prevent; and `evict_namespace` falls back to `i64::MIN` on an unparseable `fetched_at`, silently making that entry the first eviction victim. Pairs with T12's typed instants.
- [ ] execute   [ ] skip

### T19. Log-level vocabulary re-implemented as three independent string tables across three crates, already drifted: `perform::perform_log / logging::parse_level / plugin_sdk::LogLevel` (crates/rustline-wasm/src/perform.rs:688)
- Lens: stringly-enum (merged with parse-dont-validate)
- Impact: 9 (bug-prevention 3 × blast-radius 3)
- Effort: M (~29 sites, public-API: yes)
- Risk: low
- Blast radius: ~29 sites across 5 files (crates/rustline-wasm/src/{perform.rs, host.rs}, crates/rustline/src/logging.rs, crates/rustline-plugin-sdk/src/lib.rs, crates/rustline-core/src/config.rs)
- Proposed type: move the existing `rustline_plugin_sdk::LogLevel` (already `Error|Warn|Info|Debug|Trace` with `as_str()`) down into rustline-abi — the crate all three already depend on — adding a case-insensitive, trimmed `FromStr`/`parse`, plus a separate `pub enum LevelThreshold { Off, Level(LogLevel) }` for the config sinks, which additionally accept `"off"`. Wire unchanged: `rl_log(level: String, msg)` keeps its `String` param and `perform_log` keeps its `&str` extism seam — it just matches `LogLevel::parse` instead of an inline 5-arm literal match, preserving unknown → degrade-to-`info`-with-`orig_level` exactly (pinned; invariant N2). SDK re-exports `LogLevel` from abi (guest source-compatible). Do NOT change `LogConfig.file_level`/`stderr_level` away from `String` — config.rs:1244 explicitly forbids it and the lenient fallback is invariant #3; instead `logging::parse_level` returns `Option<LevelThreshold>` built on the shared enum, so the level *names* have one definition. The drift is real today: `parse_level` accepts 6 values including `off`, `perform_log` accepts 5 (a guest logging at `"off"` is silently promoted to INFO), the SDK enum has 5.
- [ ] execute   [ ] skip

> **decision-needed (behavior):** whether `rl_log` should accept `"off"` from a
> guest at all (today it silently degrades to `info`) is a behavior question,
> not a mechanical migration — settle it explicitly when executing T19 rather
> than letting the type unification change it silently.

### T20. Mouse button is a `String` from tmux compared against literals in two crates: `ClickArgs.button / click::resolve_click / ClickBindings::for_button` (crates/rustline/src/cli.rs:466)
- Lens: stringly-enum
- Impact: 9 (bug-prevention 3 × blast-radius 3)
- Effort: M (26 sites, public-API: yes)
- Risk: low
- Blast radius: 26 sites across 6 files (crates/rustline/src/{cli.rs, click.rs, main.rs, tmux_conf.rs}, crates/rustline-core/src/config.rs, crates/rustline/tests/smoke.rs)
- Proposed type: `pub enum MouseButton { Left, Right, Middle }` in rustline-core next to `ClickBindings`, with `as_str()`/`parse()` mirroring the existing `Region` pattern; `ClickBindings::for_button(MouseButton)` becomes an exhaustive 3-arm match with no `_`. Do NOT make `ClickArgs.button` a clap `ValueEnum` — an unrecognized `--button` must stay a silent no-op with exit 0 (tmux is the caller; documented at cli.rs:462-465 and pinned by `unknown_button_is_noop`), not a clap parse error. Keep `pub button: String` and parse once at the single dispatch site (main.rs:176): `MouseButton::parse(&args.button).map_or(ClickAction::NoOp, |b| resolve_click(cfg, &args.range, b))`. Config keys `left_click`/`right_click`/`middle_click` stay three `Option<ClickBinding>` fields — only the lookup key changes type; no serde/TOML change. `tmux_conf.rs`'s emitted `--button=left|middle|right` strings should be generated from `MouseButton::as_str()` so emitter and parser can't drift.
- [ ] execute   [ ] skip

### T21. GitHub release metadata is hand-walked as `serde_json::Value` and the plugin index is re-deserialized from an already-parsed `Value` clone: `Downloader::get_json / select_wasm_asset / plugin_index::parse_index_value` (crates/rustline/src/plugin_install.rs:107)
- Lens: parse-dont-validate
- Impact: 9 (bug-prevention 3 × blast-radius 3)
- Effort: M (12 sites, public-API: yes)
- Risk: low
- Blast radius: 12 sites across 2 files (crates/rustline/src/plugin_install.rs, crates/rustline/src/plugin_index.rs)
- Proposed type: `#[derive(Deserialize)] struct GhRelease { #[serde(default)] tag_name: Option<String>, #[serde(default)] assets: Vec<GhAsset> }` and `struct GhAsset { name: String, browser_download_url: String }` (no `deny_unknown_fields` — the GitHub API adds fields). Widen the seam to a generic `get_json<T: DeserializeOwned>` (or add `get_body -> String`) so each caller deserializes into its own type. `select_wasm_asset` reduces to `release.assets.iter().find(|a| a.name.ends_with(".wasm"))` returning a named `WasmAsset` instead of a `(String, String)` tuple — today transposing the destructured name/url halves silently downloads from the asset *name*, and the five chained `get`/`as_str`/`?` hops give no diagnostic on which hop failed. `plugin_index::parse_index_value`'s `serde_json::from_value(v.clone())` — a full deep clone plus a second deserialization of a document `get_json` already parsed — becomes one `serde_json::from_str::<PluginIndex>`, promoting the currently `#[cfg(test)]`-only `parse_index` to the single production parser. The two `FakeDownloader` test impls update with it.
- [ ] execute   [ ] skip

### T22. Cache namespace and cache key are two adjacent free-form `&str`s on a `pub` file-deleting API guarded only at runtime: `cache::cache_path / namespace_dir` (crates/rustline-wasm/src/cache.rs:63)
- Lens: stringly-enum (merged with newtype)
- Impact: 8 (bug-prevention 4 × blast-radius 2)
- Effort: S (~25 sites, public-API: yes)
- Risk: low
- Blast radius: ~25 sites across 2 files (crates/rustline-wasm/src/cache.rs, crates/rustline-wasm/src/perform.rs)
- Proposed type: `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum CacheNamespace { Http, Exec }` with `ALL`, `as_str()`/`dir_name()` returning the two existing literals `"__http_cache__"`/`"__exec_cache__"` verbatim (on-disk layout stays byte-identical; existing cache dirs keep resolving), and `from_dir_name(&str) -> Option<CacheNamespace>`; `cache_path(state_dir: &Path, ns: CacheNamespace, key: &str)`. Two production call sites pass `::Http`/`::Exec`. Rewrite `namespace_dir` over `from_dir_name` so the recognized-namespace set has exactly one definition — today a new `const FOO_NAMESPACE` compiles fine and every write into it is silently refused forever by `namespace_dir`'s hand-extended disjunction (the plugin then pays a live fetch or real subprocess spawn per render with no diagnostic; the in-file test at cache.rs:654 documents this exact gap). Also splits the adjacent namespace/key params: `cache_path(&dir, url, HTTP_NAMESPACE)` compiles today and lands the write outside the namespace — the exact collision the module doc says namespaces prevent. Keep `write_entry`'s runtime refusal (it takes a `&Path` and defends a caller that built the path another way — the enum shrinks, not removes, that hazard). Optionally `CacheKey<'a>` constructed from a URL/`CanonicalArgv` (T3) folds the key half in.
- [ ] execute   [ ] skip

### T23. CapabilityCtx memoizes state-dir size with `u64::MAX` as an in-band "unseeded" sentinel: `CapabilityCtx.state_size / SIZE_UNSEEDED` (crates/rustline-wasm/src/capability.rs:87)
- Lens: illegal-states
- Impact: 6 (bug-prevention 3 × blast-radius 2)
- Effort: S (12 sites, public-API: no)
- Risk: low
- Blast radius: 12 sites across 2 files (crates/rustline-wasm/src/capability.rs, crates/rustline-wasm/src/perform.rs)
- Proposed type: private `struct SizeMemo { seeded: AtomicBool, bytes: AtomicU64 }` with `get() -> Option<u64>` / `set(u64)` / `clear()` — still `Send + Sync` lock-free, the constraint that ruled out `Cell<Option<u64>>` (CapabilityCtx lives in Extism `UserData`, which requires `Send`). The code comment at capability.rs:15-19 already documents the papered-over hazard — `set_state_size(u64::MAX)` reads back as "unseeded" — and dismisses it as unreachable, but it is reachable through `write_entry`'s `Ok(projected_size(..))` return, and the sentinel also makes an `invalidate`/`set` race indistinguishable from a legitimate maximal size. The existing three memo tests pin the semantics unchanged, including exactly-one-walk. Scoped against T7: T7 covers the public quota-arithmetic signatures; this is only the internal memo representation.
- [ ] execute   [ ] skip

### T24. `merge_config` takes the user's existing config, the generated starter, and a theme name as three consecutive `&str`: `fn merge_config(existing, generated, theme)` (crates/rustline/src/init.rs:113)
- Lens: newtype
- Impact: 6 (bug-prevention 3 × blast-radius 2)
- Effort: S (9 sites, public-API: no)
- Risk: low
- Blast radius: crates/rustline/src/init.rs:113,114,117,130,169,631,657; crates/rustline/src/tmux_conf.rs:192; crates/rustline/src/theme_cmd.rs:43
- Proposed type: the cheapest version needs no new type — hoist the two `.parse::<DocumentMut>()` calls to the caller so `merge_config(existing: DocumentMut, generated: &DocumentMut, theme: &ThemeName)`; the third argument can then no longer be transposed with either document, and `theme` gets its type from T15. Today `merge_config(generated, existing, theme)` compiles and inverts the whole non-destructive merge: the user's `[layout]`/`[widgets.*]` tables become the ones treated as "add only if absent" against the starter, silently discarding hand-written config — non-destructiveness is the function's entire documented contract. One production call site plus `write_config`; `line_diff(old, new)` and `upsert_tmux_block(existing, block)` are the same shape and worth doing in the same pass.
- [ ] execute   [ ] skip

### T25. `index_is_fresh` takes two unix timestamps and a duration as three consecutive bare `u64`s: `fn index_is_fresh(fetched_at, now, ttl_secs)` (crates/rustline/src/plugin_index.rs:109)
- Lens: newtype
- Impact: 6 (bug-prevention 3 × blast-radius 2)
- Effort: S (13 sites, public-API: no)
- Risk: low
- Blast radius: 13 sites across 2 files (crates/rustline/src/plugin_index.rs, crates/rustline/src/sample_store.rs)
- Proposed type: `#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)] #[serde(transparent)] pub struct UnixSecs(u64);` introduced next to `sample_store::now_unix_secs` so the one clock read produces the type; `CachedIndex.fetched_at: UnixSecs` with `#[serde(transparent)]` keeps the on-disk cache JSON a plain number (existing caches still load); `index_is_fresh(fetched_at: UnixSecs, now: UnixSecs, ttl: TtlSecs)`. Today both transpositions compile: `index_is_fresh(now, fetched_at, ttl)` makes every cache look permanently stale (a network fetch on every `plugin search`), and `index_is_fresh(fetched_at, ttl, now)` makes a 24-hour-old cache look fresh forever. Same newtype family as T12 — reuse `TtlSecs` if it is hoisted somewhere both crates can see.
- [ ] execute   [ ] skip

### T26. A GitHub `owner`/`repo` pair travels as an untyped `(String, String)` tuple and an unvalidated `PluginSource::OwnerRepo(String)` slug re-split at every use: `parse_owner_repo / release_api_url / PluginSource::OwnerRepo` (crates/rustline/src/plugin_install.rs:90)
- Lens: newtype (merged with parse-dont-validate — smart-constructor rule)
- Impact: 6 (bug-prevention 3 × blast-radius 2)
- Effort: S (~22 sites, public-API: yes)
- Risk: low
- Blast radius: ~22 sites across 3 files (crates/rustline/src/plugin_install.rs, crates/rustline-core/src/config.rs, crates/rustline/src/plugin_index.rs)
- Proposed type: `pub struct OwnerRepo { owner: String, repo: String }` with `FromStr` enforcing the existing `is_slug` rule (the validation that makes the value safe to interpolate) and `Display` round-tripping `"{owner}/{repo}"`; `release_api_url(&OwnerRepo, tag: Option<&str>)`. Today `release_api_url(&repo, &owner, tag)` compiles and fetches a real-but-wrong GitHub repo's release, whose `.wasm` then installs under the user's chosen plugin name — a supply-chain-shaped failure with a plausible-looking success message. `PluginSource::OwnerRepo` stays a bare `owner/repo` string on the TOML wire: its hand-written `Deserialize` (which today accepts ANY string, deferring failure to `do_install`) uses the `FromStr` with warn-and-fall-back so no existing config stops loading (invariant #3); `Display`/serializer keep emitting the bare form, so round-trips are unaffected. `IndexEntry.source` carries the same slug from `registry/index.json` and should use the same type so `plugin search` can flag a malformed index entry instead of surfacing it as installable. (`select_wasm_asset`'s sibling tuple is covered by T21.)
- [ ] execute   [ ] skip

### T27. The throughput sample is an untyped `(u64, u64, u64)` of two byte counters and a unix timestamp, destructured positionally at every site: `serialize_sample / parse_sample / throughput_rate` (crates/rustline/src/throughput.rs:181)
- Lens: newtype
- Impact: 6 (bug-prevention 3 × blast-radius 2)
- Effort: S (20 sites, public-API: no)
- Risk: low
- Blast radius: 20 sites across 2 files (crates/rustline/src/throughput.rs, crates/rustline/src/sample_store.rs)
- Proposed type: `pub struct IfaceCounters { pub rx_bytes: u64, pub tx_bytes: u64 }` and `pub struct ThroughputSample { pub counters: IfaceCounters, pub taken_at: UnixSecs }`; `serialize_sample(&ThroughputSample)`, `parse_sample(&str) -> Option<ThroughputSample>`, `throughput_rate(prev: IfaceCounters, cur: IfaceCounters, dt: f64) -> Throughput`. The persisted `"{rx} {tx} {ts}\n"` line is written and read only by this module — keep `serialize_sample`'s `format!` unchanged and the on-disk shape is preserved; nothing crosses the WASM ABI or TOML. Today `serialize_sample(rx, ts, tx)` compiles and writes a timestamp into the tx-bytes slot (read back next render as a colossal upload delta), and `throughput_rate(cur, prev, dt)` compiles and reports 0 both directions (the `checked_sub` reset guard swallows it silently). `aggregate`'s `Option<(u64, u64)>` and `parse_proc_net_dev`'s `Vec<(String, u64, u64)>` take `IfaceCounters` in the same pass; `rustline_abi::Throughput` is already named-field and needs no change.
- [ ] execute   [ ] skip

### T28. IndexEntry's `bundled` bool plus `Option<source>` lets an entry be neither installable nor buildable, and lets bundled silently shadow a recorded source: `IndexEntry { source: Option<String>, bundled: bool }` (crates/rustline/src/plugin_index.rs:42)
- Lens: illegal-states
- Impact: 6 (bug-prevention 3 × blast-radius 2)
- Effort: M (14 sites, public-API: no)
- Risk: low
- Blast radius: 14 sites across 4 files (crates/rustline/src/plugin_index.rs, crates/rustline/src/plugin_cmd.rs, crates/rustline/tests/smoke.rs, registry/index.json)
- Proposed type: derived `pub enum IndexAvailability { Bundled, Installable { source: OwnerRepo }, ListingOnly }` computed once in `plugin_index::validate` (the single choke point both the fetch and cache-read paths already traverse) — the serde shape (`source`/`bundled` JSON keys) stays exactly as-is so `registry/index.json` and third-party indexes are byte-identical. `plugin_cmd`'s `match (entry.bundled, entry.source.as_deref())` then matches the enum: `(false, None)` stops being an anonymous silent no-op and becomes a named `ListingOnly` case, and the `(true, _)` wildcard's real hazard surfaces — every entry in the shipped registry has BOTH `bundled: true` AND a recorded `source`, and the wildcard silently discards the source, so a user searching a bundled plugin is never shown an install command even though one is recorded. `validate` warns (not rejects — indexes must stay forward-compatible) on `ListingOnly`, turning a registry data-entry typo into a CI-visible signal.
- [ ] execute   [ ] skip

### T29. ThemeEntryJson pairs a stringly `"builtin"|"file"` source with a `shadowed` bool only meaningful for one of them, and three sites re-derive the active/shadowed rule differently: `ThemeEntryJson / PickEntry` (crates/rustline/src/theme_cmd.rs:158)
- Lens: illegal-states (merged with stringly-enum — the discriminator gates the flag's validity)
- Impact: 6 (bug-prevention 3 × blast-radius 2)
- Effort: M (~14 sites, public-API: no)
- Risk: medium
- Blast radius: ~14 sites across 2 files (crates/rustline/src/theme_cmd.rs, crates/rustline/tests/smoke.rs)
- Proposed type: `enum ThemeOrigin { Builtin { shadowed_by_file: bool }, File }` with a hand-written `Serialize` (or `#[serde(flatten)]` helper) still emitting `"source": "builtin"|"file"` and `"shadowed": bool` so `theme list --json` stays byte-identical (this subsumes the plain two-variant `ThemeSource` enum the stringly lens proposed). One shared `fn classify(name, active, files)` called by both `list_lines` and `theme_list_json` — today they independently re-derive the same rule (the doc on `theme_list_json` admits it is "re-derived, not shared"), and `picker_entries` derives a third, subtly different variant: it sets `active` with no shadow guard, so a file shadowing an active built-in marks BOTH entries active in the picker while `list_lines` marks neither. The enum makes `{ source: "file", shadowed: true }` unconstructible, and lifting `active` to a single `Option<usize>` index makes the two-actives state unconstructible too. Display/JSON-output only — no config or wire changes; the existing shadow+active characterization test is the guard.
- [ ] execute   [ ] skip

## Low

### T30. `[layout]` is parsed twice by two independent parsers — serde in `Config::load` and a hand-walk of the raw `toml_edit` document: `widget_cmd::read_layout / mutate / widget_tui::run` (crates/rustline/src/widget_cmd.rs:48)
- Lens: parse-dont-validate
- Impact: 4 (bug-prevention 2 × blast-radius 2)
- Effort: S (9 sites, public-API: no)
- Risk: low
- Blast radius: crates/rustline/src/widget_cmd.rs:48,79,271,287,372,453,490; crates/rustline/src/widget_tui.rs:857,866
- Proposed type: implement `read_layout` by *deserializing* the `[layout]` item (`toml_edit::Item -> toml::Value -> Layout` via serde) so `Layout`'s serde attributes remain the single definition of how `[layout]` is read; only the format-preserving write path stays hand-written (`write_layout` keeps its `toml_edit` form — that half genuinely needs comment/format preservation and already errors rather than panicking on a non-table `layout`). Today the two parsers must agree on defaults, inline-vs-standard-table handling, and what a non-string array element means, with nothing enforcing agreement — `read_layout` silently drops a non-string entry where serde would surface it, and a future field or default added to `Layout` can be missed by the editor path.
- [ ] execute   [ ] skip

### T31. `PluginConfig.tag` is only meaningful when `source` is an OwnerRepo, but the two are independent Options: `PluginConfig { source: Option<PluginSource>, tag: Option<String> }` (crates/rustline-core/src/config.rs:1382)
- Lens: illegal-states
- Impact: 4 (bug-prevention 2 × blast-radius 2)
- Effort: M (12 sites, public-API: yes)
- Risk: low
- Blast radius: 12 sites across 3 files (crates/rustline-core/src/config.rs, crates/rustline/src/plugin_install.rs, crates/rustline/src/plugin_cmd.rs)
- Proposed type: a derived, host-side view — `pub enum Provenance<'a> { HandInstalled, Released { source: &'a PluginSource, tag: Option<&'a str> } }` returned by `PluginConfig::provenance()` — with the flat `[plugins.<name>]` TOML keys unchanged (user-facing; `plugin install`/`update` round-trip them through `toml_edit` preserving comments and hand-granted allowlists). Route the two readers through it: `do_update` (three-arm match on `pc.source` today) and `plugin_cmd::list` (nests `if let Some(tag)` inside `if let Some(src)`, silently hiding a `tag` recorded without a `source` — reachable via a hand-edit or partial `plugin remove`, then silently unused rather than reported). `checksum` deliberately NOT folded in — pinning a digest for a hand-built plugin is a legitimate source-free configuration.
- [ ] execute   [ ] skip

### T32. Built-in theme lookup and the display-order name list are two hand-maintained mirrors of one 7-value set: `themes::builtin_theme / builtin_theme_names` (crates/rustline-core/src/themes.rs:14)
- Lens: stringly-enum
- Impact: 4 (bug-prevention 2 × blast-radius 2)
- Effort: M (17 sites, public-API: yes)
- Risk: low
- Blast radius: 17 sites across 4 files (crates/rustline-core/src/themes.rs, crates/rustline-core/src/config.rs, crates/rustline/src/main.rs, crates/rustline/src/theme_cmd.rs)
- Proposed type: `pub enum BuiltinTheme { Default, PastelRainbow, Nord, Gruvbox, CatppuccinMocha, TokyoNight, Dracula }` with `ALL: [BuiltinTheme; 7]` (display order), `as_str()` (kebab-case spellings verbatim), `parse()`, and `theme(self) -> Theme`. Keep `pub fn builtin_theme(name: &str) -> Option<Theme>` as a thin wrapper (`parse(name).map(theme)`) — no public rename, and the `[theme].base` vocabulary is genuinely open (a themes-dir file shadows a built-in; file-first resolution in `resolve_base_theme` untouched), so this is NOT a strict config-boundary enum. The value is deleting the duplication: `builtin_theme_names()` becomes `ALL.map(as_str)`, so an 8th built-in automatically appears in `theme list`/`pick`/`new --from`, retiring the one-directional sync test at themes.rs:191. Failure mode today is UX (an existing built-in invisible in listings), not correctness — ranked accordingly. Theme-name validation is T15, separate.
- [ ] execute   [ ] skip

### T33. EditorState's `confirming_quit`/`dirty`/`show_help` bools let a quit confirmation exist with nothing to confirm: `EditorState { dirty, confirming_quit, show_help, status }` (crates/rustline/src/widget_tui.rs:132)
- Lens: illegal-states
- Impact: 4 (bug-prevention 2 × blast-radius 2)
- Effort: M (17 sites, public-API: no)
- Risk: low
- Blast radius: 17 sites in crates/rustline/src/widget_tui.rs
- Proposed type: `enum Overlay { None, Help, ConfirmQuit }` replacing the `show_help`/`confirming_quit` pair (mutually exclusive in the draw loop but independently settable today), optionally plus `enum Buffer { Clean, Dirty { confirming_quit: bool } }` to gate the confirmation behind dirtiness by construction. `show_help()`/`confirming_quit()` become `matches!` accessors so the draw loop's help-height/footer branches keep working unchanged; `on_key`'s "any other key cancels a pending confirmation" prelude becomes one `overlay = Overlay::None`. Deletes: `confirming_quit && !dirty` (currently unreachable only because two independent guards both remember the invariant) and `show_help && confirming_quit` simultaneously (draw loop reserves a help row while the footer renders the confirm prompt). Pure UI state, no serialization; the existing on_key unit tests cover the confirm/write/quit sequence.
- [ ] execute   [ ] skip

### T34. `bench --only` and `--format` are Strings compared against literals, so a typo silently produces an empty or wrong-format report: `BenchArgs.only / BenchArgs.format` (crates/rustline/src/cli.rs:527)
- Lens: stringly-enum
- Impact: 2 (bug-prevention 2 × blast-radius 1)
- Effort: S (12 sites, public-API: yes)
- Risk: low
- Blast radius: 12 sites across 2 files (crates/rustline/src/cli.rs, crates/rustline/src/bench/mod.rs)
- Proposed type: two clap `ValueEnum`s — `enum BenchGroup { All, Regions, Widgets, Sources, Plugins, Daemon }` and `enum ReportFormat { Table, Markdown }` with `default_value_t`. Purely a CLI-surface change on a dev command behind `#[cfg(feature = "bench")]`; `ValueEnum` derives the same lowercase spellings the docs already advertise, so accepted values are unchanged — the new behavior is that `--only=widget` (singular) or `--format=markdwn` errors at parse time instead of emitting a zero-group report or silently falling back to table format. `bench/mod.rs`'s `want` closure and `format == "markdown"` comparison become enum comparisons; the in-file test helper constructing `BenchArgs` directly updates to the enum values.
- [ ] execute   [ ] skip

## Skip (do not re-flag in future runs)
- `PluginConfig.options: toml::Value` / `RenderInput.config: &serde_json::Value` / `GuestRender.config: serde_json::Value` at crates/rustline-core/src/config.rs:1429 — deliberate extensibility seam, reviewed and intentionally not struct-ified: the `[plugins.<name>].options` table is guest-defined, converted verbatim (`serde_json::to_value` at rustline-wasm/src/lib.rs:164) and forwarded unchanged across the ABI; a host-side struct would either reject options a future plugin needs or force an ABI break per new plugin option, and the SDK's design already has each guest parse its own typed options struct once on the far side of the boundary — the seam ends in a proper parse-don't-validate boundary. (The instances re-parsing finding folded into T5 is about *re-parsing*, not openness, and does not touch these.)
