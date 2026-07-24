# whats-next bundle #5 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship five small, additive/output-only improvements — WASM guest ABI (W54), wasmtime cache self-heal (W55), `{spark}` alt_format gating (W56), theme error hints (W18), and `--json` on all read-only list commands (W40).

**Architecture:** Each item is an isolated change to one or two files with its own TDD unit test. No item alters the default rendered bar. W54 is a purely-additive wire-struct change; W55 is cache plumbing; W56 flips one gate predicate; W18 is an error-message helper; W40 adds a `--json` flag + pure JSON-builder helpers behind each list command.

**Tech Stack:** Rust edition 2024, serde/serde_json (already deps of the bin), clap derive, tempfile for tests. No new dependencies.

## Global Constraints

- **Edition 2024** in every crate; keep all crate editions equal to `rustfmt.toml`.
- **clippy-clean:** `cargo clippy --all-targets -- -D warnings` must pass.
- **rustfmt-clean:** run `cargo fmt --all` before every commit (no pre-commit hook exists).
- **`just test` stays hermetic** — no wasm toolchain. Every new test is a pure host-side unit test (`cargo test`).
- **No default-output change:** an unconfigured user's rendered bar is byte-identical; `--json`-absent output is byte-identical (invariant #3).
- **Additive wire contract:** no `deny_unknown_fields`; `ABI_VERSION` stays `1` (invariant #2).
- **Best-effort I/O never panics** (invariants N2, #3).
- **Doc-list sync:** the final task updates the widget/CLI/plugin lists in **both** `CLAUDE.md` and `README.md` (standing project rule).
- No `Cargo.lock` change expected (no dependency changes).

---

### Task 1: W54 — Hoist five fields into `WireContext`

**Files:**
- Modify: `crates/rustline-abi/src/lib.rs` (the `WireContext` struct, ~l242-266)
- Modify/Test: `crates/rustline-wasm/src/abi.rs` (the `sample_context()` fixture and the `wire_context_round_trips_host_context_bytes` seam test, ~l40-160)

**Interfaces:**
- Consumes: `rustline_abi::{Throughput, MediaInfo}` (already defined in the same crate).
- Produces: `WireContext.throughput/uptime/media/cpu_history/mem_history` — new public fields a guest can read.

- [ ] **Step 1: Update the seam test to assert the five fields (failing test)**

In `crates/rustline-wasm/src/abi.rs`, in `sample_context()`, change the
throughput/history fields to carry real values so the round-trip is meaningful,
and delete the two stale "not (yet) mirrored" comments (the `disks`/`throughputs`
comment stays). Set:

```rust
            // Per-instance maps (W46) are NOT mirrored on `WireContext` — a
            // guest sees only the singular base reading (invariant #2).
            disks: BTreeMap::new(),
            throughput: Some(Throughput {
                down_bytes_per_sec: 1_200_000,
                up_bytes_per_sec: 64_000,
            }),
            throughputs: BTreeMap::new(),
            os: "linux".into(),
            arch: "x86_64".into(),
            uptime: Some(86_400 * 3 + 3600 * 4), // 3d 4h
            media: Some(MediaInfo {
                artist: "Radiohead".into(),
                title: "Karma Police".into(),
                status: "Playing".into(),
            }),
            cpu_history: vec![0.1, 0.5, 0.9],
            mem_history: vec![0.2, 0.4, 0.6],
```

(Add `Throughput` to the test module's `use` imports if not already present.)

Then, in `wire_context_round_trips_host_context_bytes`, after the existing
`assert_eq!(wire.disk, ctx.disk);` line, add:

```rust
        assert_eq!(wire.throughput, ctx.throughput);
        assert_eq!(wire.uptime, ctx.uptime);
        assert_eq!(wire.media, ctx.media);
        assert_eq!(wire.cpu_history, ctx.cpu_history);
        assert_eq!(wire.mem_history, ctx.mem_history);
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rustline-wasm wire_context_round_trips_host_context_bytes`
Expected: FAIL to compile — `no field `throughput` on type `rustline_abi::WireContext`` (and the other four).

- [ ] **Step 3: Add the five fields to `WireContext`**

In `crates/rustline-abi/src/lib.rs`, inside `pub struct WireContext`, insert the
new fields to mirror `Context`'s order — `cpu_history` after `cpu`, `mem_history`
after `memory`, and `throughput`/`uptime`/`media` after `disk`. All five get
`#[serde(default)]` (matching `Context`):

```rust
    pub cpu: Option<CpuUsage>,
    #[serde(default)]
    pub cpu_history: Vec<f32>,
    pub memory: Option<MemInfo>,
    #[serde(default)]
    pub mem_history: Vec<f32>,
    pub git: Option<GitInfo>,
    pub disk: Option<DiskInfo>,
    #[serde(default)]
    pub throughput: Option<Throughput>,
    #[serde(default)]
    pub uptime: Option<u64>,
    #[serde(default)]
    pub media: Option<MediaInfo>,
    pub os: String,
```

Update the doc comment on `WireContext` if it enumerates the omitted fields
(remove any "except throughput/uptime/media/histories" wording — they're now
mirrored; `disks`/`throughputs` remain the only host-only fields).

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p rustline-wasm wire_context_round_trips_host_context_bytes`
Expected: PASS.

- [ ] **Step 5: Verify the existing wire-defaults test still passes and lint**

Run: `cargo test -p rustline-abi && cargo test -p rustline-wasm`
Expected: PASS (the "minimal WireContext JSON omitting toggled/colors" test at `lib.rs:410` still decodes — the new fields are `#[serde(default)]`).
Run: `cargo fmt --all && cargo clippy -p rustline-abi -p rustline-wasm --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/rustline-abi/src/lib.rs crates/rustline-wasm/src/abi.rs
git commit -m "feat(abi): mirror throughput/uptime/media/histories into WireContext (W54)"
```

---

### Task 2: W55 — Self-heal `wasmtime-cache.toml` on content mismatch

**Files:**
- Modify/Test: `crates/rustline-wasm/src/paths.rs` (`ensure_wasmtime_cache_config`, ~l60-79; tests, ~l101-145)

**Interfaces:**
- Consumes: `cache_config_toml(&Path) -> String` (already private in this file).
- Produces: no signature change to `ensure_wasmtime_cache_config`; behavior hardened.

- [ ] **Step 1: Write the failing test**

In `crates/rustline-wasm/src/paths.rs`'s `mod tests`, add:

```rust
    #[test]
    fn cache_config_rewrites_stale_content() {
        // A pre-existing config in a now-invalid schema (e.g. an old wasmtime's
        // `enabled` key) must be rewritten to the current format, not returned
        // verbatim — else a wasmtime bump could be handed a config it rejects
        // and drop every plugin (N2 upgrade hazard).
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("wasmtime-cache")).unwrap();
        let config_path = dir.path().join("wasmtime-cache.toml");
        std::fs::write(&config_path, "[cache]\nenabled = true\n").unwrap();

        let p = ensure_wasmtime_cache_config(dir.path()).unwrap();
        assert_eq!(p, config_path);
        let now = std::fs::read_to_string(&p).unwrap();
        assert!(!now.contains("enabled"), "stale key rewritten away: {now}");
        assert_eq!(now, cache_config_toml(&dir.path().join("wasmtime-cache")));
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rustline-wasm cache_config_rewrites_stale_content`
Expected: FAIL — the current fast path returns the stale file verbatim, so `now` still contains `enabled`.

- [ ] **Step 3: Implement compare-and-rewrite**

Replace the body of `ensure_wasmtime_cache_config` from the `config_path` line
onward so the intended `body` is computed first and the fast path only keeps a
byte-matching file:

```rust
    std::fs::create_dir_all(&cache_dir).ok()?;
    let config_path = root.join("wasmtime-cache.toml");
    let body = cache_config_toml(&cache_dir);
    // Fast path: keep an existing file only if its bytes already match what we
    // would write. A stale file (e.g. an old `[cache]` schema from a prior
    // wasmtime) self-heals by rewriting, so wasmtime is never handed a config
    // this binary wouldn't itself produce (guards the N2 upgrade hazard).
    // Reading a ~60-byte file is cheap relative to the Cranelift compile the
    // cache saves.
    if std::fs::read_to_string(&config_path).ok().as_deref() == Some(body.as_str()) {
        return Some(config_path);
    }
    let tmp = config_path.with_extension("tmp");
    std::fs::write(&tmp, &body).ok()?;
    std::fs::rename(&tmp, &config_path).ok()?;
    Some(config_path)
```

Update the doc comment's "Fast path: ... an existing file is already correct"
sentence to reflect the new compare-and-rewrite behavior.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rustline-wasm cache_config_`
Expected: PASS — `cache_config_rewrites_stale_content`, plus the existing
`..._has_cache_section_and_directory`, `..._none_on_unwritable_root_no_panic`,
and `..._is_idempotent` all still pass.

- [ ] **Step 5: fmt + clippy**

Run: `cargo fmt --all && cargo clippy -p rustline-wasm --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/rustline-wasm/src/paths.rs
git commit -m "fix(wasm): self-heal stale wasmtime-cache.toml on content mismatch (W55)"
```

---

### Task 3: W56 — Gate `{spark}` history on `alt_format` too

**Files:**
- Modify/Test: `crates/rustline/src/build_context.rs` (the two history gates, ~l141-168; tests, ~l448-473)
- Modify (docs): `CLAUDE.md`, `README.md` (remove the `{spark}` caveat)

**Interfaces:**
- Consumes: `Config.widgets.cpu.{format,alt_format,spark_width}` and `.memory.{...}` (all existing).
- Produces: new private `fn spark_referenced(format: &str, alt_format: &str) -> bool`.

- [ ] **Step 1: Write the failing test**

In `crates/rustline/src/build_context.rs`'s `mod tests`, add (mirroring
`cpu_mem_history_populated_only_when_format_references_spark`):

```rust
    #[test]
    fn cpu_mem_history_populated_when_spark_only_in_alt_format() {
        // {spark} present ONLY in the click-toggle alt_format (format is the
        // default, {spark}-free) must still populate the history ring (W56) —
        // otherwise the sparkline renders permanently empty.
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.widgets.cpu.alt_format = "{icon} {spark} {percent}%".into();
        cfg.widgets.memory.alt_format = "{icon} {spark} {percent}%".into();
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: serialized by `ENV_LOCK` against the other tests here that
        // also mutate this var (history I/O routes through state_root()).
        unsafe {
            std::env::set_var("XDG_DATA_HOME", tmp.path());
        }
        let layout = ["cpu".to_string(), "memory".to_string()];
        let ctx = build_region_context(&RegionArgs::default(), &layout, &Theme::default(), &cfg);
        // SAFETY: matches the set above; restores the process env for other tests.
        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        drop(guard);
        assert_eq!(ctx.cpu_history.len(), 1);
        assert_eq!(ctx.mem_history.len(), 1);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rustline cpu_mem_history_populated_when_spark_only_in_alt_format`
Expected: FAIL — the gate checks only `format`, so both histories stay empty (`len() == 0`, not `1`).

- [ ] **Step 3: Implement the predicate and update both gates**

In `crates/rustline/src/build_context.rs`, add a private free function near the
top of the file (after the imports):

```rust
/// A widget accumulates `{spark}` history when EITHER its `format` or its
/// click-toggle `alt_format` references `{spark}` — a `{spark}` that only
/// appears in `alt_format` (a compact default that expands on click) must still
/// populate the ring, else it renders permanently empty (W56).
fn spark_referenced(format: &str, alt_format: &str) -> bool {
    format.contains("{spark}") || alt_format.contains("{spark}")
}
```

Change the `cpu_history` gate guard from
`cfg.widgets.cpu.format.contains("{spark}")` to:

```rust
    let cpu_history = match cpu {
        Some(c) if spark_referenced(&cfg.widgets.cpu.format, &cfg.widgets.cpu.alt_format) => {
            crate::cpu::read_cpu_history(
                &rustline_wasm::state_root(),
                c.percent,
                cfg.widgets.cpu.spark_width,
            )
        }
        _ => Vec::new(),
    };
```

And the `mem_history` gate guard from
`cfg.widgets.memory.format.contains("{spark}")` to
`spark_referenced(&cfg.widgets.memory.format, &cfg.widgets.memory.alt_format)`
(leave the rest of that `match` arm — the percent computation and
`read_memory_history` call — unchanged).

Update the doc comment on `build_region_context` (the "`format` contains the
literal `{spark}`" sentence) to say "`format` **or** `alt_format` references
`{spark}`".

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rustline cpu_mem_history`
Expected: PASS — the new test plus the existing
`cpu_mem_history_empty_when_spark_absent_from_format` (default formats/alt_formats
reference neither `{spark}` → still empty) and
`cpu_mem_history_populated_only_when_format_references_spark`.

- [ ] **Step 5: Remove the obsolete `{spark}` caveat from the docs**

In `CLAUDE.md`: delete the "`{spark}` history caveat (important):" paragraph
(the one stating a `{spark}` in `alt_format` alone never populates history), and
in the `build_context.rs` module-map bullet change "only read ... when the
`cpu`/`memory` widget's `format` contains the literal `{spark}`" to "when the
widget's `format` **or** `alt_format` references `{spark}`". Also remove the
roadmap line that recorded W56 as an open follow-up if present.

In `README.md`: find and delete the equivalent `{spark}`-only-in-`alt_format`
caveat note (grep `README.md` for `spark` and remove the caveat paragraph;
leave the `{spark}` feature description itself).

- [ ] **Step 6: fmt + clippy + commit**

Run: `cargo fmt --all && cargo clippy -p rustline --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/rustline/src/build_context.rs CLAUDE.md README.md
git commit -m "feat(spark): populate history when {spark} is only in alt_format (W56)"
```

---

### Task 4: W18 — `theme use`/`theme show` error lists custom themes

**Files:**
- Modify/Test: `crates/rustline/src/theme_cmd.rs` (`use_theme` error ~l66, `show` error ~l263, new helper + tests)

**Interfaces:**
- Consumes: `builtin_theme_names()` (imported), `theme_files(themes_dir) -> Vec<String>` (already `pub(crate)` in this file).
- Produces: `fn available_themes_line(themes_dir: &Path) -> String`.

- [ ] **Step 1: Write the failing test**

In `crates/rustline/src/theme_cmd.rs`'s `mod tests`, add:

```rust
    #[test]
    fn available_themes_line_lists_builtins_and_custom_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("my-nord.toml"), "").unwrap();
        let line = available_themes_line(dir.path());
        assert!(line.contains("nord"), "lists a built-in: {line}");
        assert!(line.contains("default"), "lists a built-in: {line}");
        assert!(line.contains("my-nord"), "lists the custom themes-dir stem: {line}");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rustline available_themes_line_lists_builtins_and_custom_files`
Expected: FAIL to compile — `cannot find function `available_themes_line``.

- [ ] **Step 3: Implement the helper and wire it into both error sites**

Add near `theme_files` in `crates/rustline/src/theme_cmd.rs`:

```rust
/// The comma-joined list of every theme a user can name: built-ins plus the
/// themes-dir `*.toml` stems. Used by `theme use`/`theme show`'s "unknown theme"
/// error so a user's own scaffolded custom themes are shown, not just built-ins
/// (W18). A custom stem that shadows a built-in may appear twice — acceptable
/// for an error hint, matching what `theme list` shows.
fn available_themes_line(themes_dir: &Path) -> String {
    let mut names: Vec<String> = builtin_theme_names().iter().map(|s| s.to_string()).collect();
    names.extend(theme_files(themes_dir));
    names.join(", ")
}
```

In `use_theme`, replace the error block:

```rust
    if !resolvable(name, themes_dir) {
        eprintln!(
            "unknown theme: {name}\navailable: {}",
            available_themes_line(themes_dir)
        );
        std::process::exit(1);
    }
```

In `show`, replace the `None =>` error block:

```rust
        None => {
            eprintln!(
                "unknown theme: {name}\navailable: {}",
                available_themes_line(themes_dir)
            );
            std::process::exit(1);
        }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p rustline available_themes_line_lists_builtins_and_custom_files`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

Run: `cargo fmt --all && cargo clippy -p rustline --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/rustline/src/theme_cmd.rs
git commit -m "feat(theme): list custom themes-dir themes in unknown-theme error (W18)"
```

---

### Task 5: W40a — `--json` for `theme list`

**Files:**
- Modify: `crates/rustline/src/cli.rs` (`ThemeCmd::List` variant, ~l114)
- Modify/Test: `crates/rustline/src/theme_cmd.rs` (`run` dispatch ~l20, `list` ~l143, new `theme_list_json` + test)

**Interfaces:**
- Consumes: `builtin_theme_names()`, `theme_files`, `Config.theme.base`.
- Produces: `pub(crate) fn theme_list_json(active: &str, files: &[String]) -> String`.

- [ ] **Step 1: Write the failing test**

In `crates/rustline/src/theme_cmd.rs`'s `mod tests`, add:

```rust
    #[test]
    fn theme_list_json_shape_marks_active_and_source() {
        let files = vec!["my-nord".to_string()];
        let json = theme_list_json("nord", &files);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let arr = v.as_array().unwrap();
        // built-ins first, then the file stem
        let nord = arr.iter().find(|e| e["name"] == "nord").unwrap();
        assert_eq!(nord["active"], true);
        assert_eq!(nord["source"], "builtin");
        let deflt = arr.iter().find(|e| e["name"] == "default").unwrap();
        assert_eq!(deflt["active"], false);
        let custom = arr.iter().find(|e| e["name"] == "my-nord").unwrap();
        assert_eq!(custom["source"], "file");
        assert_eq!(custom["shadowed"], false);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rustline theme_list_json_shape_marks_active_and_source`
Expected: FAIL to compile — `cannot find function `theme_list_json``.

- [ ] **Step 3: Implement `theme_list_json` and the `--json` flag**

In `crates/rustline/src/theme_cmd.rs`, add the builder near `list_lines`:

```rust
#[derive(serde::Serialize)]
struct ThemeEntryJson {
    name: String,
    active: bool,
    source: &'static str, // "builtin" | "file"
    shadowed: bool,
}

/// The `theme list --json` payload: one entry per built-in (in registration
/// order) then per themes-dir file. Shares the active/shadowed logic with
/// `list_lines` so the human and JSON views can't drift.
pub(crate) fn theme_list_json(active: &str, files: &[String]) -> String {
    let mut entries = Vec::new();
    for name in builtin_theme_names() {
        let shadowed = files.iter().any(|f| f == name);
        entries.push(ThemeEntryJson {
            name: (*name).to_string(),
            active: *name == active && !shadowed,
            source: "builtin",
            shadowed,
        });
    }
    for f in files {
        entries.push(ThemeEntryJson {
            name: f.clone(),
            active: f == active,
            source: "file",
            shadowed: false,
        });
    }
    serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".to_string())
}
```

Change `list` to honor the flag:

```rust
fn list(config_path: &Path, themes_dir: &Path, json: bool) {
    let cfg = Config::load(config_path);
    let active = cfg.theme.base.as_deref().unwrap_or("default");
    let files = theme_files(themes_dir);
    if json {
        println!("{}", theme_list_json(active, &files));
    } else {
        for line in list_lines(active, &files) {
            println!("{line}");
        }
    }
}
```

Update the `run` dispatch arm: `ThemeCmd::List { json } => list(config_path, themes_dir, json),`.

In `crates/rustline/src/cli.rs`, change the `ThemeCmd::List` variant:

```rust
    /// List built-in and themes-dir themes (marks the active one).
    List {
        /// Emit the list as a JSON array instead of human-readable text.
        #[arg(long)]
        json: bool,
    },
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rustline theme_list`
Expected: PASS — the new `theme_list_json_shape_...` test plus the existing
`list_lines_*` tests (unchanged human path).

- [ ] **Step 5: Manual smoke + fmt + clippy**

Run: `cargo run -p rustline -- theme list --json`
Expected: a JSON array of theme entries printed to stdout.
Run: `cargo run -p rustline -- theme list`
Expected: unchanged human output.
Run: `cargo fmt --all && cargo clippy -p rustline --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/rustline/src/cli.rs crates/rustline/src/theme_cmd.rs
git commit -m "feat(theme): --json for theme list (W40)"
```

---

### Task 6: W40b — `--json` for `plugin list`, `plugin url/path list`, `plugin denials`

**Files:**
- Modify: `crates/rustline/src/cli.rs` (`PluginCmd::List`, `PluginCmd::Denials`, `PatternCmd::List` variants)
- Modify/Test: `crates/rustline/src/plugin_cmd.rs` (`run`/`pattern_cmd`/`denials_cmd`/`list` dispatch + three new JSON builders + tests)

**Interfaces:**
- Consumes: `Config.plugins` (`PluginConfig` fields `source`/`tag`/`allowed_urls`/`allowed_paths`/`max_state_bytes`), `resolve_manifest`, `rustline_wasm::{Denial, DenialKind, read_denials}`, `denial_kind_label`.
- Produces: `plugin_list_json`, `pattern_list_json`, `denials_json` (all private, pure).

- [ ] **Step 1: Write the failing tests**

In `crates/rustline/src/plugin_cmd.rs`'s `mod tests`, add:

```rust
    #[test]
    fn pattern_list_json_none_is_empty_array_some_is_array() {
        // Absent/empty allowlist → valid empty JSON array (stdout stays
        // parseable in CI), never a human "no such plugin" string.
        assert_eq!(pattern_list_json(None), "[]");
        let patterns = vec!["https://wttr.in/*".to_string()];
        let json = pattern_list_json(Some(&patterns));
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0], "https://wttr.in/*");
    }

    #[test]
    fn denials_json_shape() {
        let denials = vec![
            rustline_wasm::Denial { kind: DenialKind::Url, target: "https://evil.example/".into() },
            rustline_wasm::Denial { kind: DenialKind::Path, target: "/etc/passwd".into() },
        ];
        let json = denials_json(&denials);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v[0]["kind"], "url");
        assert_eq!(v[0]["target"], "https://evil.example/");
        assert_eq!(v[1]["kind"], "path");
    }

    #[test]
    fn plugin_list_json_has_expected_fields() {
        let mut cfg = Config::default();
        let mut pc = rustline_core::PluginConfig::default();
        pc.allowed_urls = vec!["https://wttr.in/*".to_string()];
        cfg.plugins.insert("weather".to_string(), pc);
        // A path with no manifest sidecar → has_manifest false.
        let json = plugin_list_json(&cfg, std::path::Path::new("/nonexistent-plugin-dir"));
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let w = v.as_array().unwrap().iter().find(|e| e["name"] == "weather").unwrap();
        assert_eq!(w["allowed_urls"][0], "https://wttr.in/*");
        assert_eq!(w["has_manifest"], false);
        assert!(w.get("max_state_bytes").is_some());
    }
```

(Confirm `rustline_core::PluginConfig` is importable in this test module; if it
lives under a different path, adjust the constructor — the goal is a `Config`
with one plugin entry. If `PluginConfig` fields aren't all public/defaultable,
build the `Config` by parsing a small TOML string via `Config` instead.)

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rustline pattern_list_json_none_is_empty_array_some_is_array denials_json_shape plugin_list_json_has_expected_fields`
Expected: FAIL to compile — the three builder functions don't exist yet.

- [ ] **Step 3: Implement the three JSON builders**

In `crates/rustline/src/plugin_cmd.rs`, add:

```rust
#[derive(serde::Serialize)]
struct PluginEntryJson<'a> {
    name: &'a str,
    source: Option<String>,
    tag: Option<&'a str>,
    allowed_urls: &'a [String],
    allowed_paths: &'a [String],
    max_state_bytes: u64,
    has_manifest: bool,
}

/// The `plugin list --json` payload — one entry per configured plugin, same
/// fields the human `list` prints, plus `has_manifest` (whether a capability
/// manifest resolves). An empty plugins map serializes to `[]`.
fn plugin_list_json(cfg: &Config, plugin_dir: &Path) -> String {
    let entries: Vec<PluginEntryJson> = cfg
        .plugins
        .iter()
        .map(|(name, pc)| PluginEntryJson {
            name,
            source: pc.source.as_ref().map(|s| s.to_string()),
            tag: pc.tag.as_deref(),
            allowed_urls: &pc.allowed_urls,
            allowed_paths: &pc.allowed_paths,
            max_state_bytes: pc.max_state_bytes,
            has_manifest: resolve_manifest(plugin_dir, name).is_some(),
        })
        .collect();
    serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".to_string())
}

/// The `plugin url|path list --json` payload: a JSON array of the allowlist
/// strings. An absent plugin or empty list → `[]` (stdout stays valid JSON in a
/// CI audit; the human path's "no such plugin" distinction is intentionally
/// dropped in JSON mode — callers check `plugin list --json` for existence).
fn pattern_list_json(patterns: Option<&Vec<String>>) -> String {
    match patterns {
        Some(list) => serde_json::to_string_pretty(list).unwrap_or_else(|_| "[]".to_string()),
        None => "[]".to_string(),
    }
}

#[derive(serde::Serialize)]
struct DenialEntryJson {
    kind: &'static str,
    target: String,
}

/// The `plugin denials --json` payload: one entry per recorded denial, `kind`
/// as the same lowercase label the human path prints.
fn denials_json(denials: &[rustline_wasm::Denial]) -> String {
    let entries: Vec<DenialEntryJson> = denials
        .iter()
        .map(|d| DenialEntryJson {
            kind: denial_kind_label(d.kind),
            target: d.target.clone(),
        })
        .collect();
    serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".to_string())
}
```

(Note: `denial_kind_label(d.kind)` takes `DenialKind` by value; `DenialKind` is
a fieldless enum and should be `Copy`, so `d.kind` on a `&Denial` copies. If it
is not `Copy`, the implementer may derive `Copy` on `DenialKind` in
`rustline-wasm` or clone — verify and adjust minimally.)

- [ ] **Step 4: Wire the `--json` flag into the CLI and dispatch**

In `crates/rustline/src/cli.rs`:

```rust
    /// List configured plugins and their allowlists/caps.
    List {
        /// Emit the list as a JSON array instead of human-readable text.
        #[arg(long)]
        json: bool,
    },
    // ...
    /// List a plugin's persisted capability denials ...
    Denials {
        /// The plugin name (its `.wasm` stem).
        name: String,
        /// Emit the denials as a JSON array instead of human-readable text.
        #[arg(long)]
        json: bool,
    },
```

and in `PatternCmd::List`:

```rust
    /// List the plugin's patterns.
    List {
        plugin: String,
        /// Emit the patterns as a JSON array of strings instead of one per line.
        #[arg(long)]
        json: bool,
    },
```

In `crates/rustline/src/plugin_cmd.rs`, update `run`'s arms and the handlers:

```rust
        PluginCmd::List { json } => list(config_path, plugin_dir, json),
        // ...
        PluginCmd::Denials { name, json } => denials_cmd(&name, json),
```

```rust
fn list(config_path: &Path, plugin_dir: &Path, json: bool) {
    let cfg = Config::load(config_path);
    if json {
        println!("{}", plugin_list_json(&cfg, plugin_dir));
        return;
    }
    if cfg.plugins.is_empty() {
        println!("no plugins configured");
        return;
    }
    // ... existing human loop unchanged ...
}
```

```rust
fn denials_cmd(name: &str, json: bool) {
    let denials = rustline_wasm::read_denials(name);
    if json {
        println!("{}", denials_json(&denials));
        return;
    }
    if denials.is_empty() {
        println!("no recorded denials for {name}");
        return;
    }
    for d in denials {
        println!("{} denied: {}", denial_kind_label(d.kind), d.target);
    }
}
```

In `pattern_cmd`, the `List` arm:

```rust
        PatternCmd::List { plugin, json } => {
            let cfg = Config::load(config_path);
            let patterns = cfg.plugins.get(&plugin).map(|p| match kind {
                Kind::Url => &p.allowed_urls,
                Kind::Path => &p.allowed_paths,
            });
            if json {
                println!("{}", pattern_list_json(patterns));
            } else {
                match patterns {
                    Some(list) if !list.is_empty() => list.iter().for_each(|p| println!("{p}")),
                    Some(_) => println!("(none)"),
                    None => println!("no such plugin: {plugin}"),
                }
            }
        }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rustline pattern_list_json denials_json plugin_list_json`
Expected: PASS.
Run: `cargo test -p rustline` (full bin suite — no regressions in existing plugin tests)
Expected: PASS.

- [ ] **Step 6: Manual smoke + fmt + clippy**

Run: `cargo run -p rustline -- plugin list --json` and `cargo run -p rustline -- plugin url list weather --json`
Expected: valid JSON arrays (likely `[]` on a clean machine).
Run: `cargo fmt --all && cargo clippy -p rustline --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/rustline/src/cli.rs crates/rustline/src/plugin_cmd.rs
git commit -m "feat(plugin): --json for plugin list, url/path list, and denials (W40)"
```

---

### Task 7: Docs sync, roadmap entry, and full-suite verification

**Files:**
- Modify: `CLAUDE.md` (CLI section: `--json` on the five list commands; module-map W54 note that WireContext now mirrors the five fields; roadmap "bundle #5" entry)
- Modify: `README.md` (CLI/list-command docs: `--json`)

**Interfaces:** none (documentation + verification only).

- [ ] **Step 1: Update `CLAUDE.md`**

- In the CLI section, append `[--json]` to the five list commands' synopses:
  `rustline plugin list`, `rustline plugin url|path list`, `rustline plugin denials`,
  `rustline theme list` — one clause each noting `--json` emits a machine-readable array.
- In the `rustline-abi` / `WireContext` module-map description, update the
  "field-for-field identical except ..." wording: `WireContext` now also mirrors
  `throughput`/`uptime`/`media`/`cpu_history`/`mem_history`; only the
  `disks`/`throughputs` per-instance maps remain host-only (invariant #2).
- Update invariant #2's parenthetical that lists which fields are omitted from
  `WireContext` — remove `uptime`/`media`/`throughput`/`cpu_history`/`mem_history`
  from the "could be omitted" list, leaving only `disks`/`throughputs`.
- Add a roadmap entry:

```markdown
- Done (whats-next bundle #5, branch `whats-next/2026-07-24-execute` — see the
  [design spec](docs/superpowers/specs/2026-07-24-rustline-whatsnext-bundle-5-design.md)
  / [plan](docs/superpowers/plans/2026-07-24-rustline-whatsnext-bundle-5.md)):
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
```

- Add the spec/plan links to the "Design docs" list at the bottom.

- [ ] **Step 2: Update `README.md`**

Add `--json` mentions to the list commands' documentation in `README.md`
(mirroring wherever `plugin list`/`theme list` are described), one line each.
Confirm the `{spark}` caveat removal from Task 3 is reflected (it should already
be, from Task 3 Step 5 — verify no stale caveat remains).

- [ ] **Step 3: Full-suite verification**

Run: `cargo fmt --all --check`
Expected: no diff.
Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean across the workspace.
Run: `just test` (or `cargo test --workspace`)
Expected: all tests pass, hermetically (no wasm toolchain needed).

- [ ] **Step 4: Confirm no default-output drift**

Run: `cargo run -p rustline -- render right --preview` and
`cargo run -p rustline -- render left --preview`
Expected: renders without error; output unchanged from before the bundle (spot check).

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md README.md
git commit -m "docs: sync CLAUDE.md + README.md for whats-next bundle #5 (W54/W55/W56/W18/W40)"
```

---

## Self-Review

**Spec coverage:**
- W54 → Task 1 (WireContext fields + seam test). ✓
- W55 → Task 2 (compare-and-rewrite + stale-content test). ✓
- W56 → Task 3 (`spark_referenced` gate + alt_format test + caveat removal). ✓
- W18 → Task 4 (`available_themes_line` + both error sites + test). ✓
- W40 → Tasks 5 (theme list) + 6 (plugin list/pattern/denials), all five surfaces. ✓
- Docs sync + roadmap (standing rule + spec's "Docs" sections) → Task 7. ✓
- Out-of-scope (W42, `disks`/`throughputs` mirroring) → not tasked, per spec. ✓

**Placeholder scan:** No TBD/TODO; every code step shows the actual code. The
two "verify/adjust" notes (Task 6 `PluginConfig` constructor path; `DenialKind`
`Copy`) are explicit, bounded verification instructions, not vague hand-waves.

**Type consistency:** `theme_list_json(active: &str, files: &[String]) -> String`,
`plugin_list_json(&Config, &Path) -> String`, `pattern_list_json(Option<&Vec<String>>) -> String`,
`denials_json(&[rustline_wasm::Denial]) -> String`, `available_themes_line(&Path) -> String`,
`spark_referenced(&str, &str) -> bool` — each defined once and called with
matching signatures. CLI variants gain `json: bool`, matched in the same task's
dispatch update.
