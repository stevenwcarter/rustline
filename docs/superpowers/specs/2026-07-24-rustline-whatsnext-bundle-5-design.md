# rustline — whats-next bundle #5 (design)

Date: 2026-07-24
Branch: `whats-next/2026-07-24-execute`
Source: `/whats-next --execute` handoff (WHATS-NEXT.md items W54, W55, W56, W18, W40).

Five small-to-medium, mostly-independent improvements surfaced by prior
whats-next/review passes. W42 (batched window render) was checked in the same
triage but **split out** of this bundle — it rewrites the tmux integration
model (the verbatim two-line `STATUS_FORMAT_0`, per-window click ranges) and
needs live multi-window verification, so it gets its own spec+build later.

All five items are additive or output-only; none change the default rendered
bar output. The unifying constraint: **no change to any existing byte-identical
default output**, and every wire/ABI change stays additive (invariant #2).

---

## W54 — Hoist the remaining `Context` fields into `WireContext`

### Problem

`rustline_abi::WireContext` is the typed struct a WASM guest deserializes as its
render input. It mirrors `Context` field-for-field **except** five fields that
were never added when each landed: `throughput`, `uptime`, `media`,
`cpu_history`, `mem_history`. Because the wire contract is additive (no
`deny_unknown_fields`), the host serializes these on `Context` and the guest
simply drops them on decode — so a guest can never branch on network
throughput, uptime, now-playing media, or the sparkline histories. This was
flagged across the W41/W47/W45 reviews as a purely-additive gap.

### Change

Add these five fields to `WireContext` (`crates/rustline-abi/src/lib.rs`),
positioned to mirror `Context`'s field order for readability:

```rust
pub cpu: Option<CpuUsage>,
#[serde(default)]
pub cpu_history: Vec<f32>,            // NEW
pub memory: Option<MemInfo>,
#[serde(default)]
pub mem_history: Vec<f32>,            // NEW
pub git: Option<GitInfo>,
pub disk: Option<DiskInfo>,
#[serde(default)]
pub throughput: Option<Throughput>,  // NEW
#[serde(default)]
pub uptime: Option<u64>,             // NEW
#[serde(default)]
pub media: Option<MediaInfo>,        // NEW
pub os: String,
```

- **All five new fields get `#[serde(default)]`, exactly matching `Context`.**
  Verified against `crates/rustline-core/src/context.rs`: `cpu_history` (l59),
  `mem_history` (l67), `throughput` (l95), `uptime` (l114), and `media` (l121)
  are each `#[serde(default)]` there. Mirroring the attribute keeps host/guest
  version skew total (a host that omits any key still decodes), consistent with
  every other `WireContext` field.
- **Do NOT add** `disks`/`throughputs` (the W46 per-instance maps). They remain
  deliberately host-only — a guest sees only the singular base reading
  (invariant #2 explicitly calls this out). This bundle does not change that.
- `ABI_VERSION` stays `1`: this is additive, so `abi_decision` still registers
  every existing guest unchanged.

### Invariants this feature depends on

- **The host serializes `&Context` verbatim as the guest's render input**
  (`RenderInput` in `rustline-wasm`). W54 relies on those bytes already
  carrying these five fields (they do — `Context` has them), so adding the
  fields to `WireContext` makes the existing bytes decode into them. If that
  serialization seam ever changed (e.g. a projection step), this feature would
  silently see defaults. Pinned by the seam test below.

### Test

Extend the load-bearing seam test
`wire_context_round_trips_host_context_bytes` (`crates/rustline-wasm/src/abi.rs`):

- Give the test's `sample_context()` a **non-default** `throughput`
  (`Some(Throughput { down_bytes_per_sec: 1_200_000, up_bytes_per_sec: 64_000 })`)
  and non-empty `cpu_history`/`mem_history` (e.g. `vec![0.1, 0.5, 0.9]`), so the
  new assertions actually exercise the round-trip rather than comparing
  `None`/empty to `None`/empty. `uptime`/`media` are already set to real values.
- Add assertions: `assert_eq!(wire.throughput, ctx.throughput)`,
  `wire.uptime == ctx.uptime`, `wire.media == ctx.media`,
  `wire.cpu_history == ctx.cpu_history`, `wire.mem_history == ctx.mem_history`.
- Remove the two stale comments (`abi.rs:95-98`, `abi.rs:110-114`) that say
  these are "not (yet) mirrored"; `disks`/`throughputs` keep their host-only
  comment.

This test lives behind the `wasm-e2e`/normal test set as it already does — it's
a pure serde round-trip, no wasm toolchain needed.

---

## W55 — Self-heal `wasmtime-cache.toml` across a wasmtime schema change

### Problem

`ensure_wasmtime_cache_config` (`crates/rustline-wasm/src/paths.rs`) has a fast
path (line ~71): if `wasmtime-cache.toml` already exists, it returns the path
**without inspecting the content**. The file's *content* is version-agnostic —
rustline vN writes it in whatever `[cache]` schema its bundled wasmtime accepts.
A future wasmtime whose `[cache]` schema changes (exactly the `enabled`-key
breakage this codebase already hit) would then be handed a now-stale config by
rustline vN+1, `Cache::from_file`/`build()` would reject it, and **every plugin
would be dropped** (an N2 regression). Not reachable today (the code only ever
writes the current valid format), but a latent upgrade hazard flagged in the
T6/W43 review.

### Change

Compute the intended `body` first, then compare the on-disk content against it;
keep the file only on an exact match, else rewrite via the existing atomic
temp-file + rename:

```rust
pub fn ensure_wasmtime_cache_config(root: &Path) -> Option<PathBuf> {
    let cache_dir = root.join("wasmtime-cache");
    if !cache_dir.is_absolute() {
        return None;
    }
    std::fs::create_dir_all(&cache_dir).ok()?;
    let config_path = root.join("wasmtime-cache.toml");
    let body = cache_config_toml(&cache_dir);
    // Fast path: keep an existing file only if its bytes already match what we
    // would write. A stale file (e.g. an old [cache] schema from a prior
    // wasmtime) is self-healed by rewriting, so wasmtime is never handed a
    // config this binary wouldn't itself produce (guards the N2 upgrade
    // hazard). Reading a ~60-byte file is cheap relative to the Cranelift
    // compile the cache saves.
    if std::fs::read_to_string(&config_path).ok().as_deref() == Some(body.as_str()) {
        return Some(config_path);
    }
    let tmp = config_path.with_extension("tmp");
    std::fs::write(&tmp, &body).ok()?;
    std::fs::rename(&tmp, &config_path).ok()?;
    Some(config_path)
}
```

Behavior is otherwise unchanged: best-effort, never panics, `None` on any I/O
failure or a relative cache dir (invariant N2). The idempotence contract holds
— a second call reads the just-written matching file and takes the fast path.

### Test

- Keep the existing tests (`cache_config_toml_has_cache_section_and_directory`,
  `cache_config_none_on_unwritable_root_no_panic`, `cache_config_is_idempotent`)
  — all still pass.
- Add `cache_config_rewrites_stale_content`: pre-create the cache dir, write a
  **wrong** `wasmtime-cache.toml` (e.g. `"[cache]\nenabled = true\n"`), call
  `ensure_wasmtime_cache_config`, and assert the file now equals
  `cache_config_toml(&cache_dir)` (no longer contains `enabled`). Proves the
  self-heal.

---

## W56 — Gate `{spark}` history on `alt_format` too

### Problem

`build_region_context` (`crates/rustline/src/build_context.rs`, lines ~142 and
~155) reads and persists the cpu/mem sparkline history **only** when the
widget's `format` contains the literal `{spark}`. A user who puts `{spark}`
solely in the click-toggle `alt_format` (a compact default that expands to a
sparkline on click) never accumulates history — the sparkline renders
permanently empty, with no error. Currently documented as a caveat; the real
fix is to also gate on `alt_format`. Flagged in the T5/W45 review.

### Change

In both gates, replace `.format.contains("{spark}")` with a helper predicate
that checks both `format` and `alt_format`:

```rust
// cpu
let cpu_history = match cpu {
    Some(c) if spark_referenced(&cfg.widgets.cpu.format, &cfg.widgets.cpu.alt_format) =>
        crate::cpu::read_cpu_history(&rustline_wasm::state_root(), c.percent, cfg.widgets.cpu.spark_width),
    _ => Vec::new(),
};
// memory: same predicate over cfg.widgets.memory.{format,alt_format}
```

with a small private free function in `build_context.rs`:

```rust
/// A widget accumulates `{spark}` history when EITHER its `format` or its
/// click-toggle `alt_format` references `{spark}` — a `{spark}` that only
/// appears in `alt_format` (a compact default that expands on click) must
/// still populate the ring, else it renders permanently empty (W56).
fn spark_referenced(format: &str, alt_format: &str) -> bool {
    format.contains("{spark}") || alt_format.contains("{spark}")
}
```

Both `CpuOpts` and `MemoryOpts` already carry an `alt_format: String` field
(confirmed at `config.rs:357`/`config.rs:404`), so no config change is needed.

The default config's formats (`{icon} {percent}%` / `{icon} {used}/{total}`)
and empty default `alt_format`s reference neither `{spark}` → history stays
empty → output byte-identical to before (the W45 byte-identical-by-default
case is preserved).

### Test

Add `cpu_mem_history_populated_when_spark_only_in_alt_format` (mirrors the
existing `cpu_mem_history_populated_only_when_format_references_spark`): set
`cfg.widgets.cpu.alt_format`/`cfg.widgets.memory.alt_format` to
`"{icon} {spark} {percent}%"` while leaving `format` at its `{spark}`-free
default, with `XDG_DATA_HOME` redirected to a tempdir under the existing
`ENV_LOCK`, and assert `ctx.cpu_history.len() == 1` and
`ctx.mem_history.len() == 1`.

### Docs

Remove the now-obsolete "`{spark}` history caveat" paragraph from both
`CLAUDE.md` and `README.md` (and the roadmap line noting the follow-up), since
the gap is now fixed. Update the `build_context.rs` gating description in
`CLAUDE.md`'s module map from "only when the widget's `format` references
`{spark}`" to "when the widget's `format` **or** `alt_format` references
`{spark}`".

---

## W18 — `theme use` / `theme show` error lists custom themes

### Problem

`theme_cmd.rs`'s `use_theme` prints, on an unresolvable name, only the built-in
names (`use_theme`, line ~66: `available built-ins: {builtin_theme_names}`). A
user who scaffolded `my-nord` with `theme new` and then typos it in `theme use`
is told only the built-ins are "available", hiding their own custom themes-dir
files and implying the file is unusable. `theme show`'s unknown-name error
(line ~263) has the identical gap.

### Change

Both error messages already have `themes_dir` in scope, and
`theme_files(themes_dir) -> Vec<String>` (line ~101, already used by `list` and
the picker) enumerates the themes-dir `*.toml` stems. Extend both errors to list
built-ins and custom stems:

```rust
fn available_themes_line(themes_dir: &Path) -> String {
    let mut names: Vec<String> = builtin_theme_names().iter().map(|s| s.to_string()).collect();
    names.extend(theme_files(themes_dir));   // themes-dir stems, in the order theme_files returns
    names.join(", ")
}
```

Used by both `use_theme`'s and `show`'s "unknown theme" branch:
`eprintln!("unknown theme: {name}\navailable: {}", available_themes_line(themes_dir));`.
Both already receive `themes_dir` (verified: `run` calls `use_theme(&name,
config_path, themes_dir)` and `show(&name, themes_dir)`), so no signature
change is needed.

A custom stem that duplicates a built-in name (a shadowing file) may appear
twice; that's acceptable (it's an error-hint list, not a set) and matches what
`list` already shows. No dedup needed — keep it simple.

### Test

- `theme use` unknown-name: add a test that writes a `my-custom.toml` into a
  temp themes dir, calls the resolution/error helper (or asserts via a captured
  message if the code is refactored to return the line), and asserts the line
  contains both a built-in (`nord`) and `my-custom`. If the existing test
  structure captures stderr, follow it; otherwise unit-test
  `available_themes_line` directly (pure, given a temp themes dir).

---

## W40 — `--json` on all read-only list surfaces

### Problem

The list commands emit only ad-hoc human text. The primary users are developers
who script around rustline (drive theme selection from a picker, audit plugin
allowlists in CI), and must brittle-parse that text. `bench` already offers
`--format`/`--output`; extending structured output to the list commands is a
consistent, scripting-friendly affordance.

### Scope

Add a `--json` boolean flag to every read-only "list" surface:

- `rustline plugin list --json`
- `rustline theme list --json`
- `rustline plugin url list <plugin> --json`
- `rustline plugin path list <plugin> --json`
- `rustline plugin denials <plugin> --json`

Human output is **byte-identical** when `--json` is absent (invariant #3 spirit
— an unset flag changes nothing).

### CLI wiring

- `PluginCmd::List` becomes `List { #[arg(long)] json: bool }` (currently a unit
  variant).
- `ThemeCmd::List` becomes `List { #[arg(long)] json: bool }`.
- `PatternCmd::List { plugin }` gains `#[arg(long)] json: bool`.
- `PluginCmd::Denials { name }` gains `#[arg(long)] json: bool`.

### Output schemas (serde-serializable structs, `serde_json::to_string_pretty`)

Define small `#[derive(Serialize)]` structs local to the command modules
(`plugin_cmd.rs`, `theme_cmd.rs`). No need to derive `Deserialize` in
production; tests deserialize into `serde_json::Value` (or a mirror struct) to
assert shape.

```jsonc
// plugin list --json  → array
[
  {
    "name": "weather",
    "source": "steve/rustline-weather",   // string or null
    "tag": "v1.2.0",                       // string or null
    "allowed_urls": ["https://wttr.in/*"],
    "allowed_paths": [],
    "max_state_bytes": 52428800,
    "has_manifest": true                   // whether resolve_manifest succeeds
  }
]

// theme list --json  → array
[
  { "name": "default", "active": true,  "source": "builtin", "shadowed": false },
  { "name": "nord",    "active": false, "source": "builtin", "shadowed": true  },
  { "name": "my-nord", "active": false, "source": "file",    "shadowed": false }
]

// plugin url|path list <plugin> --json  → array of strings
["https://wttr.in/*"]
// (a nonexistent plugin: the human path prints "no such plugin"; --json emits
//  an empty array [] and a warning is not printed to stdout — stdout stays
//  valid JSON. Decision: emit [] for an absent/empty allowlist; this keeps
//  stdout parseable in CI. The "no such plugin" distinction is dropped in JSON
//  mode — a caller checks `plugin list --json` for existence.)

// plugin denials <plugin> --json  → array
[ { "kind": "http", "target": "https://evil.example/" } ]
// (kind serialized as its existing lowercase label — reuse denial_kind_label,
//  or serialize the DenialKind enum with serde rename to match. Prefer reusing
//  the same label string the human path prints, for consistency.)
```

Notes:
- `theme list --json` reuses the same source data as `list_lines`
  (`builtin_theme_names()` + `theme_files`, active = `cfg.theme.base`, shadowed
  = a built-in name that also has a themes-dir file). Factor the *data*
  (a `Vec<ThemeEntry>`) out of `list_lines` so both the human and JSON paths
  consume it — avoids the human/JSON views drifting.
- `plugin list --json` reuses the same `PluginConfig` fields the human `list`
  reads; `has_manifest` calls the same `resolve_manifest(plugin_dir, name)` the
  human path's hint uses.
- Output goes to **stdout** (like the human list); `serde_json::to_string_pretty`
  for readability, trailing newline. Logs stay on stderr, so stdout is clean
  JSON.

### Tests

Per command, a test that builds the effective config/inputs, captures the JSON
string (call a pure `..._json(...) -> String` helper rather than the
stdout-printing wrapper), and asserts it parses via `serde_json::from_str::<
serde_json::Value>` (or a mirror struct) with the expected fields/values. The
key structural guarantee: **`--json` output is always valid JSON** even for the
empty/absent cases (empty array, not a human string).

### Docs

Update `CLAUDE.md` and `README.md` CLI sections: note `--json` on the five list
commands (one line each, pointing at this spec). Add a roadmap "bundle #5" done
entry summarizing all five items.

---

## Cross-cutting

- **Branch:** `whats-next/2026-07-24-execute` (already created off `main`).
- **Toolchain hygiene:** clippy-clean (`cargo clippy --all-targets -- -D
  warnings`), rustfmt-clean (`cargo fmt --all`), edition 2024. `just test` stays
  hermetic (no wasm toolchain) — every new test is a pure host-side unit test.
- **No default-output change:** none of the five items alters the rendered bar
  for a default/unconfigured user. W54 is guest-visible only; W55 is
  cache-plumbing; W56 only fires when a user opts into `{spark}` in `alt_format`;
  W18 is an error-message; W40 is behind `--json`.
- **Doc-list sync (memory):** the final step updates the widget/CLI/plugin lists
  in **both** `CLAUDE.md` and `README.md`, per the standing project memory.

## Out of scope (explicitly)

- **W42** (batched window render) — split out; its own spec+build with live
  multi-window tmux verification.
- Mirroring `disks`/`throughputs` maps into `WireContext` (W54 keeps them
  host-only, unchanged).
- `--json` for non-list commands (`print-config` already emits TOML; `config
  path`, etc. are single values).
- Deduping the theme error list (W18) or paginating it.
