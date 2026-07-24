# rustline widget manager + plugin exec capability — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship (A) a `rustline widget` command group plus a ratatui editor and tmux popup binding for managing the status-line layout, and (B) `rl_exec`/`rl_exec_cached` — capability-gated subprocess execution for WASM plugins, granted through the existing manifest/approve flow.

**Architecture:** Part A puts all layout mutation logic in `rustline-core::config` as pure functions over `Layout`, wraps it in a `toml_edit` writer in the bin (mirroring `plugin_cmd.rs`), and puts a pure `EditorState` state machine behind a thin ratatui draw loop. Part B adds two `perform_*` functions in `rustline-wasm` behind a `Runner` trait seam (mirroring `Fetcher`), gated on the full canonical argv string via the existing `AllowSet`, spawned directly with no shell.

**Tech Stack:** Rust edition 2024, `ratatui` 0.30 (bin only, default features — re-exports crossterm as `ratatui::crossterm`), `toml_edit`, `extism`, existing `AllowSet`/`cache.rs`/`FileDenialObserver`.

**Spec:** `docs/superpowers/specs/2026-07-24-rustline-widget-manager-and-exec-capability-design.md`

## Global Constraints

- **Edition 2024** in every crate, including the new `plugins/cmdrun`. `rustfmt.toml` is edition 2024; keep all crate editions equal to it.
- Must stay **clippy-clean** (`cargo clippy --all-targets -- -D warnings`) and **rustfmt-clean** (`cargo fmt --all --check`). There is **no pre-commit hook** — run `cargo fmt --all` before every commit.
- **rustls-only.** `cargo tree -i openssl` and `cargo tree -i native-tls` must match no packages across the whole graph.
- **Commit `Cargo.lock`** alongside any dependency change.
- `just test` must stay **hermetic** — no wasm toolchain required. Anything needing the wasm target goes behind the `wasm-e2e` feature or `just test-wasm`.
- **Invariant #3:** `Config::load` is total; no new code path may make a bad config break the bar.
- **Invariant #7:** a widget's layout name is its click-toggle/range identity end-to-end. A widget appears **at most once** across all three layout regions.
- **Invariant N1:** every capability gate checks *before* any effect and calls `observe_denial` at each deny site. A new capability requires a denied-case test.
- **Invariant N2:** a plugin never breaks the bar — every failure mode is a populated result struct, never a panic.
- **Invariant #4:** `init`-emitted tmux output is injection-safe (`#{q:}` + `--flag=` form; `@BINARY@` is shell-quoted).
- **`ABI_VERSION` stays `1`.** Part B is purely additive to the wire contract.
- Plugin/instance names: `[A-Za-z0-9_-]`, ≤ 15 bytes, not `window`.

---

# Part A — widget management

### Task 1: Layout algebra in `rustline-core`

Pure, I/O-free layout mutation. Everything else in Part A calls this.

**Files:**
- Modify: `crates/rustline-core/src/config.rs` (add near the existing `Layout` struct at line ~27 and the layout helpers `layout_kinds`/`disk_mounts`)
- Modify: `crates/rustline-core/src/lib.rs` (re-export the new public items)
- Test: `crates/rustline-core/src/config.rs` (its existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: the existing `pub struct Layout { pub left: Vec<String>, pub center: Vec<String>, pub right: Vec<String> }`.
- Produces: `Region`, `LayoutEditError`, `LayoutChange`, `Layout::{get, get_mut, find}`, `layout_enable`, `layout_disable`, `layout_move`, `layout_nudge` — all `pub` from `rustline_core`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/rustline-core/src/config.rs`'s `mod tests`:

```rust
fn sample_layout() -> Layout {
    Layout {
        left: vec!["pane_id".into(), "hostname".into()],
        center: vec!["windows".into()],
        right: vec!["cwd".into(), "cpu".into(), "datetime".into()],
    }
}

#[test]
fn region_parse_is_case_insensitive_and_round_trips() {
    assert_eq!(Region::parse("LEFT"), Some(Region::Left));
    assert_eq!(Region::parse("Center"), Some(Region::Center));
    assert_eq!(Region::parse("right"), Some(Region::Right));
    assert_eq!(Region::parse("middle"), None);
    for r in Region::ALL {
        assert_eq!(Region::parse(r.as_str()), Some(r));
    }
}

#[test]
fn find_locates_a_widget_in_any_region() {
    let l = sample_layout();
    assert_eq!(l.find("hostname"), Some((Region::Left, 1)));
    assert_eq!(l.find("windows"), Some((Region::Center, 0)));
    assert_eq!(l.find("datetime"), Some((Region::Right, 2)));
    assert_eq!(l.find("git"), None);
}

#[test]
fn enable_appends_when_index_is_none() {
    let mut l = sample_layout();
    let change = layout_enable(&mut l, "git", Region::Right, None).unwrap();
    assert_eq!(l.right, ["cwd", "cpu", "datetime", "git"]);
    assert_eq!(change.from, None);
    assert_eq!(change.to, Some((Region::Right, 3)));
}

#[test]
fn enable_inserts_at_a_clamped_index() {
    let mut l = sample_layout();
    layout_enable(&mut l, "git", Region::Right, Some(1)).unwrap();
    assert_eq!(l.right, ["cwd", "git", "cpu", "datetime"]);

    let mut l2 = sample_layout();
    layout_enable(&mut l2, "git", Region::Right, Some(99)).unwrap();
    assert_eq!(l2.right, ["cwd", "cpu", "datetime", "git"]);
}

#[test]
fn enable_rejects_a_name_already_present_in_another_region_and_does_not_mutate() {
    let mut l = sample_layout();
    let before = l.clone();
    let err = layout_enable(&mut l, "hostname", Region::Right, None).unwrap_err();
    assert_eq!(
        err,
        LayoutEditError::AlreadyPresent { region: Region::Left, index: 1 }
    );
    assert_eq!(l, before, "error path must not mutate");
}

#[test]
fn disable_removes_and_reports_where_it_was() {
    let mut l = sample_layout();
    let change = layout_disable(&mut l, "cpu").unwrap();
    assert_eq!(l.right, ["cwd", "datetime"]);
    assert_eq!(change.from, Some((Region::Right, 1)));
    assert_eq!(change.to, None);
}

#[test]
fn disable_of_an_absent_name_errors_without_mutating() {
    let mut l = sample_layout();
    let before = l.clone();
    assert_eq!(layout_disable(&mut l, "git").unwrap_err(), LayoutEditError::NotPresent);
    assert_eq!(l, before);
}

#[test]
fn move_across_regions_removes_from_the_old_one() {
    let mut l = sample_layout();
    let change = layout_move(&mut l, "hostname", Region::Right, 0).unwrap();
    assert_eq!(l.left, ["pane_id"]);
    assert_eq!(l.right, ["hostname", "cwd", "cpu", "datetime"]);
    assert_eq!(change.from, Some((Region::Left, 1)));
    assert_eq!(change.to, Some((Region::Right, 0)));
}

#[test]
fn move_within_a_region_reindexes_correctly() {
    let mut l = sample_layout();
    layout_move(&mut l, "cwd", Region::Right, 2).unwrap();
    assert_eq!(l.right, ["cpu", "datetime", "cwd"]);
}

#[test]
fn move_clamps_an_out_of_range_index_instead_of_erroring() {
    let mut l = sample_layout();
    layout_move(&mut l, "cwd", Region::Right, 99).unwrap();
    assert_eq!(l.right, ["cpu", "datetime", "cwd"]);
}

#[test]
fn move_to_where_it_already_is_is_a_noop_error() {
    let mut l = sample_layout();
    let before = l.clone();
    assert_eq!(
        layout_move(&mut l, "cpu", Region::Right, 1).unwrap_err(),
        LayoutEditError::NoOp
    );
    assert_eq!(l, before);
}

#[test]
fn nudge_moves_one_step_inside_its_own_region() {
    let mut l = sample_layout();
    layout_nudge(&mut l, "cpu", -1).unwrap();
    assert_eq!(l.right, ["cpu", "cwd", "datetime"]);
    layout_nudge(&mut l, "cpu", 1).unwrap();
    assert_eq!(l.right, ["cwd", "cpu", "datetime"]);
}

#[test]
fn nudge_at_a_region_boundary_is_a_noop_not_a_wraparound() {
    let mut l = sample_layout();
    let before = l.clone();
    assert_eq!(layout_nudge(&mut l, "cwd", -1).unwrap_err(), LayoutEditError::NoOp);
    assert_eq!(layout_nudge(&mut l, "datetime", 1).unwrap_err(), LayoutEditError::NoOp);
    assert_eq!(l, before);
}

#[test]
fn nudge_of_an_absent_name_errors() {
    let mut l = sample_layout();
    assert_eq!(layout_nudge(&mut l, "git", 1).unwrap_err(), LayoutEditError::NotPresent);
}
```

`Layout` must `derive(Clone, PartialEq)` for these — check its current derives and add what's missing.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rustline-core config::tests 2>&1 | tail -20`
Expected: compile errors — `cannot find type Region`, `cannot find function layout_enable`, etc.

- [ ] **Step 3: Implement the layout algebra**

Add to `crates/rustline-core/src/config.rs`, immediately after the `Layout` struct and its `default_*` fns:

```rust
/// Which of the three layout arrays a widget sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Region {
    Left,
    Center,
    Right,
}

impl Region {
    /// Every region, in visual left-to-right order.
    pub const ALL: [Region; 3] = [Region::Left, Region::Center, Region::Right];

    /// The config-key spelling, and what `--region` accepts.
    pub fn as_str(&self) -> &'static str {
        match self {
            Region::Left => "left",
            Region::Center => "center",
            Region::Right => "right",
        }
    }

    /// Parse a `--region` value, case-insensitively. `None` if unrecognized.
    pub fn parse(s: &str) -> Option<Region> {
        match s.to_ascii_lowercase().as_str() {
            "left" => Some(Region::Left),
            "center" => Some(Region::Center),
            "right" => Some(Region::Right),
            _ => None,
        }
    }
}

/// Why a layout edit was refused. Every variant means **nothing was mutated**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutEditError {
    /// The name is already placed, at this region/index. A widget may appear
    /// at most once across all three regions: two copies would share one
    /// click-toggle/range identity (invariant #7).
    AlreadyPresent { region: Region, index: usize },
    /// The name is not in any region.
    NotPresent,
    /// The edit would leave the layout exactly as it is.
    NoOp,
}

impl std::fmt::Display for LayoutEditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutEditError::AlreadyPresent { region, index } => {
                write!(f, "already in the {} region at position {index}", region.as_str())
            }
            LayoutEditError::NotPresent => write!(f, "not in any layout region"),
            LayoutEditError::NoOp => write!(f, "already in that position; nothing to do"),
        }
    }
}

/// A completed layout edit, described so a caller can report it without
/// diffing. `from`/`to` are `None` for an add/remove respectively.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutChange {
    pub name: String,
    pub from: Option<(Region, usize)>,
    pub to: Option<(Region, usize)>,
}

impl Layout {
    /// This region's widget names, in visual left-to-right order (invariant #5).
    pub fn get(&self, r: Region) -> &[String] {
        match r {
            Region::Left => &self.left,
            Region::Center => &self.center,
            Region::Right => &self.right,
        }
    }

    pub fn get_mut(&mut self, r: Region) -> &mut Vec<String> {
        match r {
            Region::Left => &mut self.left,
            Region::Center => &mut self.center,
            Region::Right => &mut self.right,
        }
    }

    /// Where `name` currently sits, if anywhere. A name appears at most once.
    pub fn find(&self, name: &str) -> Option<(Region, usize)> {
        Region::ALL.into_iter().find_map(|r| {
            self.get(r)
                .iter()
                .position(|n| n == name)
                .map(|idx| (r, idx))
        })
    }
}

/// Place `name` in `region`, at `at` (clamped to the region's length) or
/// appended when `at` is `None`.
pub fn layout_enable(
    layout: &mut Layout,
    name: &str,
    region: Region,
    at: Option<usize>,
) -> Result<LayoutChange, LayoutEditError> {
    if let Some((region, index)) = layout.find(name) {
        return Err(LayoutEditError::AlreadyPresent { region, index });
    }
    let target = layout.get_mut(region);
    let index = at.unwrap_or(target.len()).min(target.len());
    target.insert(index, name.to_string());
    Ok(LayoutChange {
        name: name.to_string(),
        from: None,
        to: Some((region, index)),
    })
}

/// Remove `name` from whichever region holds it.
pub fn layout_disable(layout: &mut Layout, name: &str) -> Result<LayoutChange, LayoutEditError> {
    let (region, index) = layout.find(name).ok_or(LayoutEditError::NotPresent)?;
    layout.get_mut(region).remove(index);
    Ok(LayoutChange {
        name: name.to_string(),
        from: Some((region, index)),
        to: None,
    })
}

/// Move `name` to `to`/`to_index` (index clamped to the destination length,
/// so a large index means "append").
pub fn layout_move(
    layout: &mut Layout,
    name: &str,
    to: Region,
    to_index: usize,
) -> Result<LayoutChange, LayoutEditError> {
    let from = layout.find(name).ok_or(LayoutEditError::NotPresent)?;
    layout.get_mut(from.0).remove(from.1);
    // Clamp against the length *after* removal, so a same-region move to the
    // end lands at the last slot rather than out of bounds.
    let dest = layout.get_mut(to);
    let index = to_index.min(dest.len());
    if from == (to, index) {
        // Restore and refuse: nothing would change.
        layout.get_mut(from.0).insert(from.1, name.to_string());
        return Err(LayoutEditError::NoOp);
    }
    layout.get_mut(to).insert(index, name.to_string());
    Ok(LayoutChange {
        name: name.to_string(),
        from: Some(from),
        to: Some((to, index)),
    })
}

/// Shift `name` by `delta` positions inside its current region. A step past
/// either end is [`LayoutEditError::NoOp`] — never a wrap-around.
pub fn layout_nudge(
    layout: &mut Layout,
    name: &str,
    delta: i32,
) -> Result<LayoutChange, LayoutEditError> {
    let (region, index) = layout.find(name).ok_or(LayoutEditError::NotPresent)?;
    let len = layout.get(region).len();
    let target = i64::from(delta) + index as i64;
    if target < 0 || target >= len as i64 {
        return Err(LayoutEditError::NoOp);
    }
    let target = target as usize;
    let arr = layout.get_mut(region);
    let name_owned = arr.remove(index);
    arr.insert(target, name_owned);
    Ok(LayoutChange {
        name: name.to_string(),
        from: Some((region, index)),
        to: Some((region, target)),
    })
}
```

Ensure `Layout` derives `Clone` and `PartialEq` (add them to its existing derive list if absent).

- [ ] **Step 4: Re-export from the crate root**

In `crates/rustline-core/src/lib.rs`, add to the existing `pub use config::{...}` list:

```rust
pub use config::{
    LayoutChange, LayoutEditError, Region, layout_disable, layout_enable, layout_move, layout_nudge,
};
```

(Merge into the existing `config::` re-export rather than adding a second `pub use` line.)

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rustline-core config::tests 2>&1 | tail -20`
Expected: all tests pass, `0 failed`.

- [ ] **Step 6: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add crates/rustline-core/src/config.rs crates/rustline-core/src/lib.rs
git commit -m "feat(core): pure layout algebra — Region, enable/disable/move/nudge"
```

---

### Task 2: Widget placement/availability + plugin discovery without instantiation

What `widget list` and the TUI need to answer "what widgets exist and where are they?" without running any guest code.

**Files:**
- Modify: `crates/rustline-core/src/widget.rs` (add `WidgetSource::Instance`)
- Modify: `crates/rustline-core/src/config.rs` (add `WidgetPlacement`, `widget_placements`)
- Modify: `crates/rustline-core/src/lib.rs` (re-export)
- Modify: `crates/rustline-wasm/src/lib.rs` (add `discover_plugin_names`, use it in `register_plugins`)
- Modify: `crates/rustline/src/plugin_cmd.rs` (any `WidgetSource` match site, if one exists)
- Test: the `#[cfg(test)] mod tests` in `config.rs` and `lib.rs` (rustline-wasm)

**Interfaces:**
- Consumes: Task 1's `Region`, `Layout::find`; the existing `WidgetSource`, `Registry::descriptors()`, `Config::instances`, `Config::instance_kind`, `is_builtin_widget_name`.
- Produces:
  - `pub enum WidgetSource { Builtin, Plugin, Instance { kind: String } }`
  - `pub struct WidgetPlacement { pub name: String, pub summary: String, pub source: WidgetSource, pub placement: Option<(Region, usize)> }`
  - `pub fn widget_placements(cfg: &Config, descriptors: &[WidgetDescriptor], plugin_names: &[String]) -> Vec<WidgetPlacement>`
  - `pub fn rustline_wasm::discover_plugin_names(plugin_dir: &Path) -> Vec<String>`

- [ ] **Step 1: Write the failing tests**

In `crates/rustline-core/src/config.rs`'s `mod tests`:

```rust
#[test]
fn placements_mark_where_each_widget_sits_and_leave_the_rest_unplaced() {
    let mut cfg = Config::default();
    cfg.layout = Layout {
        left: vec!["pane_id".into()],
        center: vec![],
        right: vec!["cpu".into(), "weather".into()],
    };
    let descriptors = vec![
        WidgetDescriptor {
            name: "pane_id".into(),
            summary: "pane id".into(),
            configurable: true,
            source: WidgetSource::Builtin,
        },
        WidgetDescriptor {
            name: "cpu".into(),
            summary: "cpu usage".into(),
            configurable: true,
            source: WidgetSource::Builtin,
        },
        WidgetDescriptor {
            name: "git".into(),
            summary: "git branch".into(),
            configurable: true,
            source: WidgetSource::Builtin,
        },
    ];
    let out = widget_placements(&cfg, &descriptors, &["weather".to_string()]);

    let by = |n: &str| out.iter().find(|p| p.name == n).unwrap().clone();
    assert_eq!(by("pane_id").placement, Some((Region::Left, 0)));
    assert_eq!(by("cpu").placement, Some((Region::Right, 0)));
    assert_eq!(by("git").placement, None);
    assert_eq!(by("weather").placement, Some((Region::Right, 1)));
    assert_eq!(by("weather").source, WidgetSource::Plugin);
}

#[test]
fn placements_include_instances_with_their_kind() {
    let mut cfg = Config::default();
    cfg.layout = Layout {
        left: vec![],
        center: vec![],
        right: vec!["clock_utc".into()],
    };
    cfg.instances.insert(
        "clock_utc".to_string(),
        toml::from_str::<toml::Value>("kind = \"datetime\"\ntimezone = \"UTC\"").unwrap(),
    );
    let out = widget_placements(&cfg, &[], &[]);
    let e = out.iter().find(|p| p.name == "clock_utc").unwrap();
    assert_eq!(e.source, WidgetSource::Instance { kind: "datetime".into() });
    assert_eq!(e.placement, Some((Region::Right, 0)));
}

#[test]
fn placements_skip_an_instance_that_collides_with_a_builtin() {
    // Built-in always wins (the W46 precedence guard); an [instances.cpu]
    // entry must never be offered as its own selectable widget.
    let mut cfg = Config::default();
    cfg.instances.insert(
        "cpu".to_string(),
        toml::from_str::<toml::Value>("kind = \"datetime\"").unwrap(),
    );
    let descriptors = vec![WidgetDescriptor {
        name: "cpu".into(),
        summary: "cpu usage".into(),
        configurable: true,
        source: WidgetSource::Builtin,
    }];
    let out = widget_placements(&cfg, &descriptors, &[]);
    let cpus: Vec<_> = out.iter().filter(|p| p.name == "cpu").collect();
    assert_eq!(cpus.len(), 1, "exactly one 'cpu' entry");
    assert_eq!(cpus[0].source, WidgetSource::Builtin);
}

#[test]
fn placements_are_deduped_and_sorted_builtins_then_instances_then_plugins() {
    let cfg = Config::default();
    let descriptors = vec![WidgetDescriptor {
        name: "cpu".into(),
        summary: "cpu usage".into(),
        configurable: true,
        source: WidgetSource::Builtin,
    }];
    // The same plugin stem listed twice must yield one entry.
    let out = widget_placements(&cfg, &descriptors, &["w".to_string(), "w".to_string()]);
    let names: Vec<&str> = out.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["cpu", "w"]);
}
```

In `crates/rustline-wasm/src/lib.rs`'s `mod tests`:

```rust
#[test]
fn discover_plugin_names_lists_wasm_stems_sorted_and_ignores_other_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("zulu.wasm"), b"\0asm").unwrap();
    std::fs::write(dir.path().join("alpha.wasm"), b"\0asm").unwrap();
    std::fs::write(dir.path().join("alpha.toml"), b"name = 'alpha'").unwrap();
    std::fs::write(dir.path().join("notes.txt"), b"hi").unwrap();

    assert_eq!(discover_plugin_names(dir.path()), ["alpha", "zulu"]);
}

#[test]
fn discover_plugin_names_on_a_missing_dir_is_empty_not_an_error() {
    assert!(discover_plugin_names(std::path::Path::new("/nonexistent-plugin-dir")).is_empty());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rustline-core config::tests::placements 2>&1 | tail -10`
Run: `cargo test -p rustline-wasm discover_plugin 2>&1 | tail -10`
Expected: both fail to compile — `cannot find function widget_placements` / `discover_plugin_names`.

- [ ] **Step 3: Add the `Instance` variant to `WidgetSource`**

In `crates/rustline-core/src/widget.rs`, change the enum:

```rust
/// Where a registered widget came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WidgetSource {
    /// Compiled into rustline.
    Builtin,
    /// Discovered as a `.wasm` plugin.
    Plugin,
    /// A named `[instances.<name>]` entry of the given built-in `kind` (W46).
    Instance { kind: String },
}
```

It previously derived `Copy`; the `String` payload makes that impossible. Fix every resulting error by cloning or borrowing — expect hits in `widget.rs` itself and possibly `plugin_cmd.rs`. Compile after this step to find them:

Run: `cargo build --workspace 2>&1 | grep -E "^error" | head -20`

- [ ] **Step 4: Implement `widget_placements`**

Add to `crates/rustline-core/src/config.rs`, after the layout algebra from Task 1:

```rust
/// One row of `widget list` / one entry in the TUI: a selectable widget, what
/// it is, and where (if anywhere) it currently sits in the layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetPlacement {
    pub name: String,
    pub summary: String,
    pub source: WidgetSource,
    /// `None` means "available but not currently in any region".
    pub placement: Option<(Region, usize)>,
}

/// Every widget a user could put in a layout, with its current placement.
///
/// Ordering is built-ins (in registration order) → instances (sorted) →
/// plugin stems (sorted), which is also the order `widget list` prints and the
/// TUI's AVAILABLE column shows. An `[instances.<name>]` entry whose name
/// collides with a built-in is skipped — built-in always wins, the same
/// precedence `Registry::with_builtins` and `is_builtin_widget_name` enforce.
pub fn widget_placements(
    cfg: &Config,
    descriptors: &[WidgetDescriptor],
    plugin_names: &[String],
) -> Vec<WidgetPlacement> {
    let mut out: Vec<WidgetPlacement> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    let mut push = |name: String, summary: String, source: WidgetSource| {
        if seen.insert(name.clone()) {
            let placement = cfg.layout.find(&name);
            out.push(WidgetPlacement { name, summary, source, placement });
        }
    };

    for d in descriptors {
        push(d.name.clone(), d.summary.clone(), d.source.clone());
    }
    let mut instances: Vec<(&String, &toml::Value)> = cfg.instances.iter().collect();
    instances.sort_by(|a, b| a.0.cmp(b.0));
    for (name, value) in instances {
        if is_builtin_widget_name(name) {
            continue;
        }
        let Some(kind) = Config::instance_kind(value) else {
            continue;
        };
        push(
            name.clone(),
            format!("{kind} instance"),
            WidgetSource::Instance { kind: kind.to_string() },
        );
    }
    let mut plugins: Vec<&String> = plugin_names.iter().collect();
    plugins.sort();
    for name in plugins {
        push(name.clone(), "wasm plugin".to_string(), WidgetSource::Plugin);
    }
    out
}
```

Import `WidgetDescriptor`/`WidgetSource` into `config.rs` (`use crate::widget::{WidgetDescriptor, WidgetSource};`) and `BTreeSet` if not already imported.

Re-export from `crates/rustline-core/src/lib.rs`: add `WidgetPlacement, widget_placements` to the `config::` re-export list.

- [ ] **Step 5: Implement `discover_plugin_names` and use it in `register_plugins`**

In `crates/rustline-wasm/src/lib.rs`:

```rust
/// The `.wasm` stems present in `plugin_dir`, sorted, **without reading or
/// instantiating any of them**. This is the discovery half of
/// `register_plugins`, split out so `rustline widget list`/`widget edit` can
/// show plugin widgets without paying wasm cold-start or running guest code.
/// A missing or unreadable directory yields an empty vec, never an error.
pub fn discover_plugin_names(plugin_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(plugin_dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("wasm") {
                return None;
            }
            path.file_stem().and_then(|s| s.to_str()).map(str::to_string)
        })
        .collect();
    names.sort();
    names
}
```

Then rewrite `register_plugins`'s directory scan to consume it, so there is one definition of "what plugins exist". Replace the `let Ok(entries) = std::fs::read_dir(plugin_dir) else { return; };` + `for entry in entries.flatten() { ... }` header with:

```rust
    for stem in discover_plugin_names(plugin_dir) {
        let path = plugin_dir.join(format!("{stem}.wasm"));
        let stem = stem.as_str();
```

and delete the now-dead `path.extension()` / `file_stem()` guards inside the loop, keeping every subsequent check (`needed`, `reg.contains`, config lookup, read, build) exactly as it is.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p rustline-core config::tests 2>&1 | tail -5`
Run: `cargo test -p rustline-wasm 2>&1 | tail -5`
Expected: all pass.

- [ ] **Step 7: Run the full suite to catch the `WidgetSource` fallout**

Run: `cargo test --workspace 2>&1 | tail -20`
Expected: all pass. If a test asserted `WidgetSource` equality by `Copy` semantics, adjust it to clone/borrow.

- [ ] **Step 8: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add -A
git commit -m "feat(core,wasm): widget placement enumeration + instantiation-free plugin discovery"
```

---

### Task 3: `rustline widget list|enable|disable|move` — CLI + `toml_edit` writer

**Files:**
- Create: `crates/rustline/src/widget_cmd.rs`
- Modify: `crates/rustline/src/cli.rs` (add `Command::Widget(WidgetCmd)` and the `WidgetCmd` enum)
- Modify: `crates/rustline/src/main.rs` (declare the module, dispatch the arm)
- Test: `crates/rustline/src/widget_cmd.rs`'s own `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: Task 1's `Region`/`layout_*`/`LayoutEditError`; Task 2's `widget_placements`/`discover_plugin_names`; existing `Registry::with_builtins`, `Config::load`, `main.rs`'s `resolve_plugin_dir`.
- Produces: `pub fn run(cmd: WidgetCmd, config_path: &Path, plugin_dir: &Path) -> i32` (the process exit code), and `pub(crate) fn read_layout(doc: &DocumentMut) -> Layout` / `pub(crate) fn write_layout(doc: &mut DocumentMut, layout: &Layout)` used by Task 5's TUI.

- [ ] **Step 1: Write the failing tests**

Create `crates/rustline/src/widget_cmd.rs` with only the test module first (implementation comes in step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn doc_of(text: &str) -> DocumentMut {
        text.parse::<DocumentMut>().unwrap()
    }

    #[test]
    fn read_layout_falls_back_to_defaults_when_the_table_is_absent() {
        let layout = read_layout(&doc_of("[widgets.cpu]\nformat = \"x\"\n"));
        // Config::load's defaults, not empty arrays.
        assert_eq!(layout.left, ["pane_id", "hostname"]);
        assert_eq!(layout.center, ["windows"]);
        assert!(layout.right.contains(&"datetime".to_string()));
    }

    #[test]
    fn read_layout_falls_back_per_region() {
        // Only `right` is specified; left/center must still come from defaults.
        let layout = read_layout(&doc_of("[layout]\nright = [\"cwd\"]\n"));
        assert_eq!(layout.left, ["pane_id", "hostname"]);
        assert_eq!(layout.center, ["windows"]);
        assert_eq!(layout.right, ["cwd"]);
    }

    #[test]
    fn write_layout_preserves_comments_and_unrelated_tables() {
        let mut doc = doc_of(
            "# my config\n[layout]\nleft = [\"pane_id\"]\n\n[widgets.cpu]\nformat = \"{percent}\"\n",
        );
        let layout = Layout {
            left: vec!["pane_id".into(), "hostname".into()],
            center: vec!["windows".into()],
            right: vec!["cwd".into()],
        };
        write_layout(&mut doc, &layout);
        let text = doc.to_string();
        assert!(text.contains("# my config"), "comment preserved: {text}");
        assert!(text.contains("[widgets.cpu]"), "other table preserved: {text}");
        assert!(text.contains("format = \"{percent}\""));
    }

    #[test]
    fn write_layout_creates_the_table_when_absent() {
        let mut doc = doc_of("[widgets.cpu]\nformat = \"x\"\n");
        let layout = Layout {
            left: vec!["pane_id".into()],
            center: vec![],
            right: vec!["cwd".into()],
        };
        write_layout(&mut doc, &layout);
        let text = doc.to_string();
        assert!(text.contains("[layout]"), "{text}");
    }

    #[test]
    fn every_written_document_reparses_under_the_strict_parser() {
        // A write that only survives Config::load's total fallback is a bug:
        // it must parse strictly, the way `rustline config validate` does.
        let mut doc = doc_of("# hi\n[layout]\nleft = [\"pane_id\"]\n");
        let layout = Layout {
            left: vec!["pane_id".into(), "hostname".into()],
            center: vec!["windows".into()],
            right: vec!["cwd".into(), "cpu".into()],
        };
        write_layout(&mut doc, &layout);
        let parsed: Config = toml::from_str(&doc.to_string()).expect("strict parse");
        assert_eq!(parsed.layout, layout);
    }

    #[test]
    fn write_layout_round_trips_an_empty_region() {
        let mut doc = doc_of("");
        let layout = Layout { left: vec![], center: vec![], right: vec!["cwd".into()] };
        write_layout(&mut doc, &layout);
        let parsed: Config = toml::from_str(&doc.to_string()).unwrap();
        assert!(parsed.layout.left.is_empty());
        assert!(parsed.layout.center.is_empty());
        assert_eq!(parsed.layout.right, ["cwd"]);
    }

    #[test]
    fn resolve_name_accepts_builtins_instances_and_plugins_and_rejects_the_unknown() {
        let known = ["cpu".to_string(), "clock_utc".to_string(), "weather".to_string()];
        assert!(resolve_name("cpu", &known).is_ok());
        assert!(resolve_name("weather", &known).is_ok());
        let err = resolve_name("nope", &known).unwrap_err();
        assert!(err.contains("nope"), "names the bad widget: {err}");
        assert!(err.contains("cpu"), "lists what is available: {err}");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rustline widget_cmd 2>&1 | tail -10`
Expected: the module isn't declared yet — `error[E0583]` or no tests found. Add `mod widget_cmd;` to `main.rs` first if needed, then expect compile errors about missing `read_layout`/`write_layout`/`resolve_name`.

- [ ] **Step 3: Implement the writer and command dispatch**

Prepend to `crates/rustline/src/widget_cmd.rs` (before the test module):

```rust
//! `rustline widget …`: list, enable, disable, and reorder the widgets in the
//! `[layout]` arrays, editing `config.toml` in place with `toml_edit` so
//! comments and formatting survive — the same approach `plugin_cmd.rs` uses
//! for allowlists and `theme_cmd.rs` for `[theme].base`.
//!
//! Every mutation goes through `rustline-core`'s pure layout algebra
//! (`layout_enable`/`layout_disable`/`layout_move`), so the CLI and the TUI
//! (`widget_tui.rs`) share one definition of a legal edit. A refused edit
//! writes nothing at all and exits non-zero.

use std::io::Write;
use std::path::Path;

use rustline_core::{
    Config, Layout, LayoutChange, Region, WidgetPlacement, WidgetSource, layout_disable,
    layout_enable, layout_move, widget_placements,
};
use rustline_wasm::discover_plugin_names;
use toml_edit::{Array, DocumentMut, Item, Table, value};

use crate::cli::WidgetCmd;

/// Read the layout as it exists in the *file*. A missing `[layout]` table, or
/// a missing array within it, falls back to `Config::load`'s default for that
/// region — so an edit against a zero-config install writes three complete,
/// correct arrays rather than orphaning the two the user never mentioned.
pub(crate) fn read_layout(doc: &DocumentMut) -> Layout {
    let defaults = Layout::default();
    let table = doc.get("layout").and_then(Item::as_table);
    let region = |key: &str, fallback: &[String]| -> Vec<String> {
        table
            .and_then(|t| t.get(key))
            .and_then(Item::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_else(|| fallback.to_vec())
    };
    Layout {
        left: region("left", &defaults.left),
        center: region("center", &defaults.center),
        right: region("right", &defaults.right),
    }
}

/// Write all three arrays back into `[layout]`, creating the table if absent.
/// Only the three arrays are touched; everything else in the document —
/// comments, ordering, other tables — is left exactly as it was.
pub(crate) fn write_layout(doc: &mut DocumentMut, layout: &Layout) {
    let table = doc
        .entry("layout")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .expect("[layout] is a table");
    for region in Region::ALL {
        let mut arr = Array::new();
        for name in layout.get(region) {
            arr.push(name.as_str());
        }
        table.insert(region.as_str(), value(arr));
    }
}

/// Every name a layout entry may legally take: registered built-ins,
/// non-colliding `[instances.<name>]` entries, and discovered plugin stems.
fn known_names(cfg: &Config, plugin_dir: &Path) -> Vec<String> {
    let registry = rustline_core::Registry::with_builtins(cfg);
    let plugins = discover_plugin_names(plugin_dir);
    widget_placements(cfg, registry.descriptors(), &plugins)
        .into_iter()
        .map(|p| p.name)
        .collect()
}

/// Accept `name` iff it is a known widget; otherwise an error message listing
/// what *is* available — the same shape as `theme use`'s unknown-name error.
fn resolve_name(name: &str, known: &[String]) -> Result<(), String> {
    if known.iter().any(|k| k == name) {
        return Ok(());
    }
    Err(format!(
        "unknown widget `{name}`\navailable: {}",
        known.join(", ")
    ))
}

/// Human line for a completed edit.
fn describe(change: &LayoutChange) -> String {
    match (change.from, change.to) {
        (None, Some((r, i))) => format!("enabled {} in {}[{i}]", change.name, r.as_str()),
        (Some((r, _)), None) => format!("disabled {} (was in {})", change.name, r.as_str()),
        (Some((fr, _)), Some((tr, ti))) => format!(
            "moved {} from {} to {}[{ti}]",
            change.name,
            fr.as_str(),
            tr.as_str()
        ),
        (None, None) => format!("{}: no change", change.name),
    }
}

/// Ask tmux to redraw the status line, so an edit is visible immediately.
/// Best-effort: outside tmux, or if the spawn fails, this is a silent no-op —
/// a failed refresh must never turn a successful config write into an error.
fn refresh_tmux() {
    if std::env::var_os("TMUX").is_none() {
        return;
    }
    if let Err(error) = std::process::Command::new("tmux")
        .args(["refresh-client", "-S"])
        .status()
    {
        tracing::warn!(%error, "tmux refresh-client failed");
    }
}

/// Dispatch a `rustline widget …` invocation. Returns the process exit code:
/// `0` on success, `1` on a refused edit (which writes nothing).
pub fn run(cmd: WidgetCmd, config_path: &Path, plugin_dir: &Path) -> i32 {
    match cmd {
        WidgetCmd::List { json } => {
            list(config_path, plugin_dir, json);
            0
        }
        WidgetCmd::Enable { name, region, index } => {
            let region = match Region::parse(&region) {
                Some(r) => r,
                None => {
                    eprintln!("unknown region `{region}` (expected left, center, or right)");
                    return 1;
                }
            };
            mutate(config_path, plugin_dir, &name, |layout| {
                layout_enable(layout, &name, region, index)
            })
        }
        WidgetCmd::Disable { name } => mutate(config_path, plugin_dir, &name, |layout| {
            layout_disable(layout, &name)
        }),
        WidgetCmd::Move { name, region, index } => {
            let region = match Region::parse(&region) {
                Some(r) => r,
                None => {
                    eprintln!("unknown region `{region}` (expected left, center, or right)");
                    return 1;
                }
            };
            mutate(config_path, plugin_dir, &name, |layout| {
                layout_move(layout, &name, region, index.unwrap_or(usize::MAX))
            })
        }
        WidgetCmd::Edit => crate::widget_tui::run(config_path, plugin_dir),
    }
}

/// Load → validate the name → apply `edit` → write + report. Any failure
/// short-circuits before the write, so `config.toml` is untouched.
fn mutate(
    config_path: &Path,
    plugin_dir: &Path,
    name: &str,
    edit: impl FnOnce(&mut Layout) -> Result<LayoutChange, rustline_core::LayoutEditError>,
) -> i32 {
    let cfg = Config::load(config_path);
    let known = known_names(&cfg, plugin_dir);
    if let Err(message) = resolve_name(name, &known) {
        eprintln!("{message}");
        return 1;
    }
    let text = std::fs::read_to_string(config_path).unwrap_or_default();
    let mut doc = match text.parse::<DocumentMut>() {
        Ok(doc) => doc,
        Err(error) => {
            eprintln!("config file is not valid TOML; refusing to edit it: {error}");
            return 1;
        }
    };
    let mut layout = read_layout(&doc);
    let change = match edit(&mut layout) {
        Ok(change) => change,
        Err(error) => {
            eprintln!("{name}: {error}");
            return 1;
        }
    };
    write_layout(&mut doc, &layout);
    if let Some(parent) = config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = std::fs::write(config_path, doc.to_string()) {
        eprintln!("failed to write {}: {error}", config_path.display());
        return 1;
    }
    println!("{}", describe(&change));
    refresh_tmux();
    0
}

/// `widget list`: every widget, marked with where it sits.
fn list(config_path: &Path, plugin_dir: &Path, json: bool) {
    let cfg = Config::load(config_path);
    let registry = rustline_core::Registry::with_builtins(&cfg);
    let plugins = discover_plugin_names(plugin_dir);
    let rows = widget_placements(&cfg, registry.descriptors(), &plugins);
    if json {
        println!("{}", placements_json(&rows));
        return;
    }
    let mut out = std::io::stdout().lock();
    for row in &rows {
        let placed = match row.placement {
            Some((r, i)) => format!("{}[{i}]", r.as_str()),
            None => "-".to_string(),
        };
        let source = match &row.source {
            WidgetSource::Builtin => "builtin".to_string(),
            WidgetSource::Plugin => "plugin".to_string(),
            WidgetSource::Instance { kind } => format!("instance of {kind}"),
        };
        let _ = writeln!(out, "{placed:<10} {:<16} {} ({source})", row.name, row.summary);
    }
}

/// `widget list --json`, matching W40's convention on the other list surfaces.
fn placements_json(rows: &[WidgetPlacement]) -> String {
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "name": r.name,
                "summary": r.summary,
                "source": match &r.source {
                    WidgetSource::Builtin => "builtin".to_string(),
                    WidgetSource::Plugin => "plugin".to_string(),
                    WidgetSource::Instance { kind } => format!("instance:{kind}"),
                },
                "region": r.placement.map(|(reg, _)| reg.as_str()),
                "index": r.placement.map(|(_, i)| i),
            })
        })
        .collect();
    serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string())
}
```

Note: `Layout::default()` must exist. `Layout` currently gets its defaults through serde's `#[serde(default = "default_left")]` attributes — add a manual `impl Default for Layout` in `config.rs` returning `Layout { left: default_left(), center: default_center(), right: default_right() }` if one isn't already derived.

- [ ] **Step 4: Add the clap surface**

In `crates/rustline/src/cli.rs`, add to `enum Command` (after `Theme`):

```rust
    /// List, enable, disable, and reorder the widgets in the status line's
    /// `[layout]` regions, or open the interactive editor.
    #[command(subcommand)]
    Widget(WidgetCmd),
```

and the enum itself, next to `ThemeCmd`:

```rust
#[derive(Subcommand)]
pub enum WidgetCmd {
    /// List every widget (built-in, instance, plugin) and where it sits.
    List {
        /// Emit the list as a JSON array instead of human-readable text.
        #[arg(long)]
        json: bool,
    },
    /// Add a widget to a layout region.
    Enable {
        name: String,
        /// Which region to add it to. Default: `right`.
        #[arg(long, default_value = "right")]
        region: String,
        /// Insert position within the region (clamped). Default: append.
        #[arg(long)]
        index: Option<usize>,
    },
    /// Remove a widget from whichever region holds it.
    Disable { name: String },
    /// Move a widget to another region and/or position.
    Move {
        name: String,
        /// Destination region.
        #[arg(long)]
        region: String,
        /// Destination position within the region (clamped). Default: append.
        #[arg(long)]
        index: Option<usize>,
    },
    /// Open the interactive widget editor (needs a TTY).
    Edit,
}
```

- [ ] **Step 5: Dispatch from `main.rs`**

Add `mod widget_cmd;` and `mod widget_tui;` alongside the other module declarations, and the dispatch arm alongside `Command::Theme(...)`:

```rust
        Command::Widget(cmd) => {
            let plugin_dir = resolve_plugin_dir(None, &cfg);
            std::process::exit(widget_cmd::run(cmd, &effective_config_path, &plugin_dir));
        }
```

Match the surrounding arms' exact style for how `cfg`/`effective_config_path`/`resolve_plugin_dir` are named and obtained — read the neighbouring `Command::Plugin` arm and follow it.

Create a placeholder `crates/rustline/src/widget_tui.rs` so this compiles; Task 4 fills it in:

```rust
//! The interactive widget editor (filled in by the next task).

use std::path::Path;

pub fn run(_config_path: &Path, _plugin_dir: &Path) -> i32 {
    eprintln!("the widget editor is not implemented yet");
    1
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p rustline widget_cmd 2>&1 | tail -10`
Expected: all 7 tests pass.

- [ ] **Step 7: Verify the CLI end to end against a temp config**

```bash
cargo build 2>&1 | tail -3
TMP=$(mktemp -d)
printf '# keep me\n[layout]\nright = ["cwd"]\n' > "$TMP/config.toml"
./target/debug/rustline --config "$TMP/config.toml" widget enable git
./target/debug/rustline --config "$TMP/config.toml" widget enable git   # expect refusal, exit 1
./target/debug/rustline --config "$TMP/config.toml" widget list | head -20
./target/debug/rustline --config "$TMP/config.toml" widget disable nope  # expect refusal, exit 1
cat "$TMP/config.toml"
```

Expected: the first `enable` prints `enabled git in right[1]`; the second prints a refusal and exits 1; `# keep me` is still in the file; `list` shows `right[1] git`.

- [ ] **Step 8: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add -A
git commit -m "feat(cli): rustline widget list|enable|disable|move over a toml_edit writer"
```

---

### Task 4: `EditorState` — the TUI's pure state machine

No terminal, no files. This is where the editor's behavior is actually tested.

**Files:**
- Modify: `crates/rustline/src/widget_tui.rs` (replace the placeholder with the state machine; the ratatui shell lands in Task 5)
- Test: `crates/rustline/src/widget_tui.rs`'s own `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: Task 1's `Region`/`Layout`/`layout_*`; Task 2's `WidgetPlacement`.
- Produces: `Column`, `KeyKind`, `EditorAction`, `EditorState::{new, on_key, columns, selected, is_dirty, layout, status}`.

- [ ] **Step 1: Write the failing tests**

In `crates/rustline/src/widget_tui.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn placement(name: &str) -> WidgetPlacement {
        WidgetPlacement {
            name: name.to_string(),
            summary: format!("{name} widget"),
            source: WidgetSource::Builtin,
            placement: None,
        }
    }

    fn state() -> EditorState {
        let layout = Layout {
            left: vec!["pane_id".into()],
            center: vec!["windows".into()],
            right: vec!["cwd".into(), "cpu".into()],
        };
        let all = vec![
            placement("pane_id"),
            placement("windows"),
            placement("cwd"),
            placement("cpu"),
            placement("git"),
            placement("disk"),
        ];
        EditorState::new(layout, all)
    }

    #[test]
    fn available_holds_exactly_the_unplaced_widgets() {
        let s = state();
        assert_eq!(s.column_items(Column::Available), ["disk", "git"]);
    }

    #[test]
    fn focus_cycles_across_the_four_columns() {
        let mut s = state();
        assert_eq!(s.column(), Column::Left);
        s.on_key(KeyKind::Right);
        assert_eq!(s.column(), Column::Center);
        s.on_key(KeyKind::Right);
        s.on_key(KeyKind::Right);
        assert_eq!(s.column(), Column::Available);
        s.on_key(KeyKind::Right);
        assert_eq!(s.column(), Column::Available, "no wrap past the last column");
        s.on_key(KeyKind::Left);
        assert_eq!(s.column(), Column::Right);
    }

    #[test]
    fn cursor_moves_within_a_column_and_clamps_at_the_ends() {
        let mut s = state();
        s.on_key(KeyKind::Right);
        s.on_key(KeyKind::Right); // Right region: [cwd, cpu]
        assert_eq!(s.selected(), Some("cwd"));
        s.on_key(KeyKind::Down);
        assert_eq!(s.selected(), Some("cpu"));
        s.on_key(KeyKind::Down);
        assert_eq!(s.selected(), Some("cpu"), "clamps at the bottom");
        s.on_key(KeyKind::Up);
        s.on_key(KeyKind::Up);
        assert_eq!(s.selected(), Some("cwd"), "clamps at the top");
    }

    #[test]
    fn space_in_available_places_the_widget_in_the_last_focused_region() {
        let mut s = state();
        s.on_key(KeyKind::Right);
        s.on_key(KeyKind::Right); // focus Right
        s.on_key(KeyKind::Right); // focus Available
        assert_eq!(s.selected(), Some("disk"));
        s.on_key(KeyKind::Space);
        assert_eq!(s.layout().right, ["cwd", "cpu", "disk"]);
        assert_eq!(s.column_items(Column::Available), ["git"]);
        assert!(s.is_dirty());
    }

    #[test]
    fn space_in_a_region_returns_the_widget_to_available() {
        let mut s = state();
        s.on_key(KeyKind::Right);
        s.on_key(KeyKind::Right); // Right region, cursor on cwd
        s.on_key(KeyKind::Space);
        assert_eq!(s.layout().right, ["cpu"]);
        assert_eq!(s.column_items(Column::Available), ["cwd", "disk", "git"]);
        assert!(s.is_dirty());
    }

    #[test]
    fn removing_the_last_item_clamps_the_cursor_rather_than_dangling() {
        let mut s = state();
        // Left region holds only pane_id; remove it.
        s.on_key(KeyKind::Space);
        assert!(s.layout().left.is_empty());
        assert_eq!(s.selected(), None);
        // A further key must not panic.
        s.on_key(KeyKind::Down);
        s.on_key(KeyKind::Space);
    }

    #[test]
    fn nudge_reorders_within_the_region_and_follows_the_widget() {
        let mut s = state();
        s.on_key(KeyKind::Right);
        s.on_key(KeyKind::Right);
        s.on_key(KeyKind::Down); // Right region, cursor on cpu (index 1)
        s.on_key(KeyKind::NudgeUp);
        assert_eq!(s.layout().right, ["cpu", "cwd"]);
        assert_eq!(s.selected(), Some("cpu"), "cursor follows the moved widget");
    }

    #[test]
    fn nudge_at_the_boundary_changes_nothing_and_does_not_dirty() {
        let mut s = state();
        s.on_key(KeyKind::Right);
        s.on_key(KeyKind::Right); // cursor on cwd (index 0)
        s.on_key(KeyKind::NudgeUp);
        assert_eq!(s.layout().right, ["cwd", "cpu"]);
        assert!(!s.is_dirty(), "a refused edit must not mark the buffer dirty");
    }

    #[test]
    fn nudge_in_the_available_column_is_ignored() {
        let mut s = state();
        for _ in 0..3 {
            s.on_key(KeyKind::Right);
        }
        assert_eq!(s.column(), Column::Available);
        s.on_key(KeyKind::NudgeDown);
        assert!(!s.is_dirty());
    }

    #[test]
    fn write_requests_a_write_and_clears_dirty_when_confirmed() {
        let mut s = state();
        s.on_key(KeyKind::Space); // remove pane_id -> dirty
        assert!(s.is_dirty());
        assert_eq!(s.on_key(KeyKind::Write), EditorAction::Write);
        s.mark_written();
        assert!(!s.is_dirty());
    }

    #[test]
    fn quit_while_dirty_asks_first_then_quits_on_the_second_press() {
        let mut s = state();
        s.on_key(KeyKind::Space);
        assert_eq!(s.on_key(KeyKind::Quit), EditorAction::ConfirmQuit);
        assert_eq!(s.on_key(KeyKind::Quit), EditorAction::Quit);
    }

    #[test]
    fn quit_while_clean_quits_immediately() {
        let mut s = state();
        assert_eq!(s.on_key(KeyKind::Quit), EditorAction::Quit);
    }

    #[test]
    fn any_key_after_a_confirm_prompt_cancels_the_quit() {
        let mut s = state();
        s.on_key(KeyKind::Space);
        assert_eq!(s.on_key(KeyKind::Quit), EditorAction::ConfirmQuit);
        s.on_key(KeyKind::Down);
        // Still dirty, and quitting asks again rather than exiting.
        assert_eq!(s.on_key(KeyKind::Quit), EditorAction::ConfirmQuit);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rustline widget_tui 2>&1 | tail -10`
Expected: compile errors — `cannot find type EditorState`, `Column`, `KeyKind`.

- [ ] **Step 3: Implement the state machine**

Replace the placeholder body of `crates/rustline/src/widget_tui.rs` (keep the `pub fn run` stub for now — Task 5 replaces it):

```rust
//! The interactive widget editor: a pure state machine ([`EditorState`]) plus
//! a thin ratatui draw loop (Task 5). Everything interesting — focus, add and
//! remove, reorder, dirty tracking, quit confirmation — lives in `on_key`,
//! which takes a [`KeyKind`] and returns an [`EditorAction`]. No terminal is
//! involved, so the behavior is unit-tested directly, the same split
//! `theme_cmd.rs`'s reader/writer-generic `run_picker` uses.

use std::path::Path;

use rustline_core::{
    Layout, Region, WidgetPlacement, WidgetSource, layout_disable, layout_enable, layout_nudge,
};

/// The four focusable columns: the three layout regions plus the pool of
/// widgets that aren't currently placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Left,
    Center,
    Right,
    Available,
}

impl Column {
    const ALL: [Column; 4] = [
        Column::Left,
        Column::Center,
        Column::Right,
        Column::Available,
    ];

    /// The layout region this column edits, or `None` for `Available`.
    fn region(self) -> Option<Region> {
        match self {
            Column::Left => Some(Region::Left),
            Column::Center => Some(Region::Center),
            Column::Right => Some(Region::Right),
            Column::Available => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Column::Left => "LEFT",
            Column::Center => "CENTER",
            Column::Right => "RIGHT",
            Column::Available => "AVAILABLE",
        }
    }
}

/// A key press, already mapped out of crossterm's `KeyEvent` so the state
/// machine has no terminal dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Up,
    Down,
    Left,
    Right,
    Space,
    NudgeUp,
    NudgeDown,
    Write,
    Quit,
    Help,
    Other,
}

/// What the draw loop should do after a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorAction {
    /// Redraw and keep going.
    Redraw,
    /// Write the layout to disk, then keep going.
    Write,
    /// Exit the editor.
    Quit,
    /// Unsaved changes: show the confirm prompt instead of quitting.
    ConfirmQuit,
}

/// Everything the editor knows. Pure — no terminal, no files.
pub struct EditorState {
    layout: Layout,
    /// Every selectable widget, by name, with its metadata. The layout is the
    /// source of truth for placement; this is only for names/summaries.
    catalog: Vec<WidgetPlacement>,
    column: Column,
    /// Cursor index within each column, kept per column so focus changes
    /// don't lose your place.
    cursors: [usize; 4],
    /// Which region a widget taken from AVAILABLE goes into.
    last_region: Column,
    dirty: bool,
    /// True while the quit confirmation is showing.
    confirming_quit: bool,
    status: String,
    show_help: bool,
}

impl EditorState {
    pub fn new(layout: Layout, catalog: Vec<WidgetPlacement>) -> Self {
        Self {
            layout,
            catalog,
            column: Column::Left,
            cursors: [0; 4],
            last_region: Column::Right,
            dirty: false,
            confirming_quit: false,
            status: String::new(),
            show_help: false,
        }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn column(&self) -> Column {
        self.column
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn show_help(&self) -> bool {
        self.show_help
    }

    pub fn confirming_quit(&self) -> bool {
        self.confirming_quit
    }

    /// Look up a catalog entry, for the source badge the draw loop shows.
    pub fn source_of(&self, name: &str) -> Option<&WidgetSource> {
        self.catalog
            .iter()
            .find(|p| p.name == name)
            .map(|p| &p.source)
    }

    /// The names shown in `column`, in display order. AVAILABLE is every
    /// catalog entry not currently placed, sorted by name.
    pub fn column_items(&self, column: Column) -> Vec<String> {
        match column.region() {
            Some(region) => self.layout.get(region).to_vec(),
            None => {
                let mut names: Vec<String> = self
                    .catalog
                    .iter()
                    .filter(|p| self.layout.find(&p.name).is_none())
                    .map(|p| p.name.clone())
                    .collect();
                names.sort();
                names
            }
        }
    }

    /// All four columns' items, for one draw pass.
    pub fn columns(&self) -> Vec<(Column, Vec<String>)> {
        Column::ALL
            .into_iter()
            .map(|c| (c, self.column_items(c)))
            .collect()
    }

    fn cursor(&self) -> usize {
        self.cursors[Column::ALL.iter().position(|c| *c == self.column).unwrap()]
    }

    fn set_cursor(&mut self, index: usize) {
        let slot = Column::ALL.iter().position(|c| *c == self.column).unwrap();
        self.cursors[slot] = index;
    }

    /// The widget under the cursor in the focused column, if any.
    pub fn selected(&self) -> Option<&str> {
        let items = self.column_items(self.column);
        let index = self.cursor().min(items.len().saturating_sub(1));
        if items.is_empty() {
            return None;
        }
        // Re-borrow from the layout/catalog rather than the temporary vec.
        match self.column.region() {
            Some(region) => self.layout.get(region).get(index).map(String::as_str),
            None => self
                .catalog
                .iter()
                .find(|p| p.name == items[index])
                .map(|p| p.name.as_str()),
        }
    }

    /// The cursor's row index within the focused column, clamped to its length.
    pub fn cursor_index(&self) -> usize {
        let len = self.column_items(self.column).len();
        self.cursor().min(len.saturating_sub(1))
    }

    /// Handle one key. The single entry point the draw loop calls.
    pub fn on_key(&mut self, key: KeyKind) -> EditorAction {
        // Any key other than a second Quit cancels a pending confirmation.
        if self.confirming_quit && key != KeyKind::Quit {
            self.confirming_quit = false;
            self.status.clear();
        }
        match key {
            KeyKind::Left => {
                self.shift_column(-1);
                EditorAction::Redraw
            }
            KeyKind::Right => {
                self.shift_column(1);
                EditorAction::Redraw
            }
            KeyKind::Up => {
                self.move_cursor(-1);
                EditorAction::Redraw
            }
            KeyKind::Down => {
                self.move_cursor(1);
                EditorAction::Redraw
            }
            KeyKind::Space => {
                self.toggle_selected();
                EditorAction::Redraw
            }
            KeyKind::NudgeUp => {
                self.nudge(-1);
                EditorAction::Redraw
            }
            KeyKind::NudgeDown => {
                self.nudge(1);
                EditorAction::Redraw
            }
            KeyKind::Write => EditorAction::Write,
            KeyKind::Help => {
                self.show_help = !self.show_help;
                EditorAction::Redraw
            }
            KeyKind::Quit => {
                if self.dirty && !self.confirming_quit {
                    self.confirming_quit = true;
                    self.status = "unsaved changes — press q again to discard, w to write".into();
                    return EditorAction::ConfirmQuit;
                }
                EditorAction::Quit
            }
            KeyKind::Other => EditorAction::Redraw,
        }
    }

    /// Called by the draw loop after a successful write.
    pub fn mark_written(&mut self) {
        self.dirty = false;
        self.confirming_quit = false;
        self.status = "written".into();
    }

    /// Called by the draw loop when a write failed.
    pub fn mark_write_failed(&mut self, error: &str) {
        self.status = format!("write failed: {error}");
    }

    fn shift_column(&mut self, delta: i32) {
        let current = Column::ALL.iter().position(|c| *c == self.column).unwrap() as i32;
        let next = (current + delta).clamp(0, Column::ALL.len() as i32 - 1) as usize;
        if self.column.region().is_some() {
            self.last_region = self.column;
        }
        self.column = Column::ALL[next];
    }

    fn move_cursor(&mut self, delta: i32) {
        let len = self.column_items(self.column).len();
        if len == 0 {
            self.set_cursor(0);
            return;
        }
        let next = (self.cursor_index() as i32 + delta).clamp(0, len as i32 - 1) as usize;
        self.set_cursor(next);
    }

    /// Space: AVAILABLE → append to the last focused region; a region → back
    /// to AVAILABLE.
    fn toggle_selected(&mut self) {
        let Some(name) = self.selected().map(str::to_string) else {
            return;
        };
        let result = match self.column.region() {
            Some(_) => layout_disable(&mut self.layout, &name),
            None => {
                let region = self.last_region.region().unwrap_or(Region::Right);
                layout_enable(&mut self.layout, &name, region, None)
            }
        };
        match result {
            Ok(_) => {
                self.dirty = true;
                self.status = String::new();
                self.clamp_cursor();
            }
            Err(error) => self.status = format!("{name}: {error}"),
        }
    }

    fn nudge(&mut self, delta: i32) {
        if self.column.region().is_none() {
            return;
        }
        let Some(name) = self.selected().map(str::to_string) else {
            return;
        };
        match layout_nudge(&mut self.layout, &name, delta) {
            Ok(change) => {
                self.dirty = true;
                self.status = String::new();
                if let Some((_, index)) = change.to {
                    self.set_cursor(index);
                }
            }
            // A boundary nudge is a no-op, not an error worth surfacing.
            Err(_) => {}
        }
    }

    fn clamp_cursor(&mut self) {
        let len = self.column_items(self.column).len();
        let clamped = self.cursor().min(len.saturating_sub(1));
        self.set_cursor(clamped);
    }
}

pub fn run(_config_path: &Path, _plugin_dir: &Path) -> i32 {
    eprintln!("the widget editor is not implemented yet");
    1
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rustline widget_tui 2>&1 | tail -10`
Expected: all 13 tests pass.

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add -A
git commit -m "feat(tui): pure EditorState state machine for the widget editor"
```

---

### Task 5: ratatui shell, live preview, and terminal lifecycle

**Files:**
- Modify: `crates/rustline/Cargo.toml` (add `ratatui = "0.30"`)
- Modify: `Cargo.lock`
- Modify: `crates/rustline/src/widget_tui.rs` (replace the `run` stub; add `map_key`, `draw`, `preview_line`, the terminal guard)
- Test: `crates/rustline/src/widget_tui.rs` (`map_key` + `preview_line` unit tests)

**Interfaces:**
- Consumes: Task 4's `EditorState`/`Column`/`KeyKind`/`EditorAction`; Task 3's `read_layout`/`write_layout`; existing `crate::sample_context::sample_context`, `crate::resolve_theme`, `rustline_core::{Registry, render_named_region, tmux_to_ansi}`.
- Produces: `pub fn run(config_path: &Path, plugin_dir: &Path) -> i32`.

- [ ] **Step 1: Add the dependency**

In `crates/rustline/Cargo.toml`, under `[dependencies]`:

```toml
# Interactive widget editor (`rustline widget edit`). Default features pull
# `ratatui-crossterm`, which re-exports crossterm as `ratatui::crossterm` — so
# crossterm is not a separate direct dependency. Deliberately NOT behind a
# cargo feature (unlike `bench`): `init` writes a `prefix + W` tmux binding
# unconditionally, so the subcommand it points at must always exist.
ratatui = "0.30"
```

Run: `cargo build 2>&1 | tail -3` and `cargo tree -i openssl; cargo tree -i native-tls`
Expected: builds; both `cargo tree` calls report "did not match any packages".

- [ ] **Step 2: Write the failing tests**

Add to `widget_tui.rs`'s `mod tests`:

```rust
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[test]
fn map_key_covers_both_arrow_and_vim_bindings() {
    let ev = |code, mods| KeyEvent::new(code, mods);
    assert_eq!(map_key(ev(KeyCode::Up, KeyModifiers::NONE)), KeyKind::Up);
    assert_eq!(map_key(ev(KeyCode::Char('k'), KeyModifiers::NONE)), KeyKind::Up);
    assert_eq!(map_key(ev(KeyCode::Down, KeyModifiers::NONE)), KeyKind::Down);
    assert_eq!(map_key(ev(KeyCode::Char('j'), KeyModifiers::NONE)), KeyKind::Down);
    assert_eq!(map_key(ev(KeyCode::Left, KeyModifiers::NONE)), KeyKind::Left);
    assert_eq!(map_key(ev(KeyCode::Char('h'), KeyModifiers::NONE)), KeyKind::Left);
    assert_eq!(map_key(ev(KeyCode::Right, KeyModifiers::NONE)), KeyKind::Right);
    assert_eq!(map_key(ev(KeyCode::Char('l'), KeyModifiers::NONE)), KeyKind::Right);
}

#[test]
fn map_key_distinguishes_shifted_nudges_from_plain_motion() {
    // Terminals report Shift+j either as 'J' or as 'j' with the SHIFT modifier.
    assert_eq!(
        map_key(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT)),
        KeyKind::NudgeDown
    );
    assert_eq!(
        map_key(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT)),
        KeyKind::NudgeUp
    );
    assert_eq!(
        map_key(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::NONE)),
        KeyKind::NudgeDown
    );
}

#[test]
fn map_key_maps_the_command_keys_and_ignores_the_rest() {
    assert_eq!(map_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE)), KeyKind::Write);
    assert_eq!(map_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)), KeyKind::Quit);
    assert_eq!(map_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)), KeyKind::Quit);
    assert_eq!(map_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)), KeyKind::Space);
    assert_eq!(map_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), KeyKind::Space);
    assert_eq!(map_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE)), KeyKind::Help);
    assert_eq!(map_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE)), KeyKind::Other);
}

#[test]
fn preview_renders_builtins_and_shows_plugins_as_a_static_chip() {
    let layout = Layout {
        left: vec!["hostname".into()],
        center: vec![],
        right: vec!["weather".into()],
    };
    let catalog = vec![
        WidgetPlacement {
            name: "hostname".into(),
            summary: "host".into(),
            source: WidgetSource::Builtin,
            placement: None,
        },
        WidgetPlacement {
            name: "weather".into(),
            summary: "weather".into(),
            source: WidgetSource::Plugin,
            placement: None,
        },
    ];
    let state = EditorState::new(layout, catalog);
    let line = preview_line(&state, &Config::default());
    // The built-in rendered its real text; the plugin is a placeholder chip and
    // was never instantiated.
    assert!(line.contains("[weather]"), "plugin chip present: {line}");
    assert!(!line.is_empty());
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p rustline widget_tui 2>&1 | tail -10`
Expected: `cannot find function map_key` / `preview_line`.

- [ ] **Step 4: Implement the shell**

Add to `widget_tui.rs` (replacing the `run` stub) — imports first:

```rust
use std::io::{self, IsTerminal, Stdout};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Direction as LayoutDirection, Layout as UiLayout};
use ratatui::style::{Modifier, Style as UiStyle};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::{Terminal, backend::CrosstermBackend};
use rustline_core::{Config, Registry, render_named_region, tmux_to_ansi};
use toml_edit::DocumentMut;
```

Then:

```rust
/// Map a crossterm key event onto the editor's own [`KeyKind`]. Arrow keys and
/// their vim equivalents are interchangeable; `J`/`K` (shifted, however the
/// terminal reports it) reorder.
fn map_key(key: KeyEvent) -> KeyKind {
    match key.code {
        KeyCode::Up => KeyKind::Up,
        KeyCode::Down => KeyKind::Down,
        KeyCode::Left => KeyKind::Left,
        KeyCode::Right => KeyKind::Right,
        KeyCode::Enter | KeyCode::Char(' ') => KeyKind::Space,
        KeyCode::Esc => KeyKind::Quit,
        KeyCode::Char('J') => KeyKind::NudgeDown,
        KeyCode::Char('K') => KeyKind::NudgeUp,
        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::SHIFT) => KeyKind::NudgeDown,
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::SHIFT) => KeyKind::NudgeUp,
        KeyCode::Char('j') => KeyKind::Down,
        KeyCode::Char('k') => KeyKind::Up,
        KeyCode::Char('h') => KeyKind::Left,
        KeyCode::Char('l') => KeyKind::Right,
        KeyCode::Char('w') => KeyKind::Write,
        KeyCode::Char('q') => KeyKind::Quit,
        KeyCode::Char('?') => KeyKind::Help,
        _ => KeyKind::Other,
    }
}

/// The ANSI preview strip: the current (possibly unsaved) layout rendered
/// through the real pipeline — `sample_context` + the resolved theme +
/// `render_named_region` + `tmux_to_ansi`, exactly as `theme show` does.
///
/// **Plugins are drawn as a static `[name]` chip, never instantiated.**
/// Instantiating a WASM guest on every keystroke would be slow and would run
/// third-party code inside a config editor; the placeholder makes the widget's
/// position visible without that cost.
fn preview_line(state: &EditorState, cfg: &Config) -> String {
    let theme = crate::resolve_theme(cfg);
    let ctx = crate::sample_context::sample_context(false);
    let registry = Registry::with_builtins(cfg);
    let mut parts: Vec<String> = Vec::new();
    for region in Region::ALL {
        // Only names the registry knows render for real; anything else (a
        // plugin stem, an unresolvable instance) becomes a chip.
        let names: Vec<String> = state
            .layout()
            .get(region)
            .iter()
            .filter(|n| registry.contains(n.as_str()))
            .cloned()
            .collect();
        let chips: Vec<String> = state
            .layout()
            .get(region)
            .iter()
            .filter(|n| !registry.contains(n.as_str()))
            .map(|n| format!("[{n}]"))
            .collect();
        let markup = render_named_region(
            rustline_core::Direction::Left,
            &names,
            &registry,
            &theme,
            cfg,
            &ctx,
        );
        let mut rendered = tmux_to_ansi(&markup);
        if !chips.is_empty() {
            rendered.push(' ');
            rendered.push_str(&chips.join(" "));
        }
        if !rendered.trim().is_empty() {
            parts.push(rendered);
        }
    }
    parts.join("   ")
}
```

**Note for the implementer:** `render_named_region`'s exact signature and
`Direction`'s variants must be read from `crates/rustline-core/src/assemble.rs`
and matched — call it the same way `main.rs`'s `render left` path does, and
adjust the call above to fit. The behavior to preserve is: built-in names
render for real, non-registry names become `[name]` chips, and no plugin is
instantiated.

Then the terminal lifecycle and loop:

```rust
/// Restores the terminal on every exit path — normal return, `?`, or a panic
/// unwinding through the draw loop. Without this a panic inside ratatui leaves
/// the user's shell in raw mode with no echo.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        Terminal::new(CrosstermBackend::new(stdout))
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

/// Open the interactive widget editor. Requires a TTY; a non-interactive
/// invocation prints a hint toward the scriptable subcommands and exits
/// non-zero without drawing anything (mirroring `theme pick`'s guard).
pub fn run(config_path: &Path, plugin_dir: &Path) -> i32 {
    if !io::stdin().is_terminal() {
        eprintln!(
            "`widget edit` needs a terminal; use `rustline widget list`, \
             `widget enable <name>`, or `widget disable <name>` instead"
        );
        return 1;
    }
    let cfg = Config::load(config_path);
    let text = std::fs::read_to_string(config_path).unwrap_or_default();
    let mut doc = match text.parse::<DocumentMut>() {
        Ok(doc) => doc,
        Err(error) => {
            eprintln!("config file is not valid TOML; refusing to edit it: {error}");
            return 1;
        }
    };
    let layout = crate::widget_cmd::read_layout(&doc);
    let catalog = rustline_core::widget_placements(
        &cfg,
        Registry::with_builtins(&cfg).descriptors(),
        &rustline_wasm::discover_plugin_names(plugin_dir),
    );
    let mut state = EditorState::new(layout, catalog);

    // The guard must outlive the loop so a panic inside it still restores.
    let _guard = TerminalGuard;
    let mut terminal = match TerminalGuard::enter() {
        Ok(t) => t,
        Err(error) => {
            eprintln!("failed to start the editor: {error}");
            return 1;
        }
    };

    loop {
        if terminal.draw(|frame| draw(frame, &state, &cfg)).is_err() {
            return 1;
        }
        let Ok(Event::Key(key)) = event::read() else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match state.on_key(map_key(key)) {
            EditorAction::Redraw | EditorAction::ConfirmQuit => {}
            EditorAction::Write => {
                crate::widget_cmd::write_layout(&mut doc, state.layout());
                match std::fs::write(config_path, doc.to_string()) {
                    Ok(()) => state.mark_written(),
                    Err(error) => state.mark_write_failed(&error.to_string()),
                }
            }
            EditorAction::Quit => break,
        }
    }
    0
}
```

Finally the `draw` function — four columns, the preview strip, and the footer.
Implement it with ratatui's `Layout`/`List`/`Paragraph` widgets: a horizontal
split into four equal columns each rendered as a `List` with the focused
column's border highlighted and the cursor row in reverse video, a
one-line `Paragraph` showing `preview_line(&state, cfg)`, and a footer
`Paragraph` with the key hints (`←→ region  ↑↓ select  space add/remove  J/K
reorder  w write  q quit`), replaced by `state.status()` when it is non-empty.
Keep `draw` free of business logic — it reads `state.columns()`,
`state.column()`, `state.cursor_index()`, `state.status()`, and
`state.show_help()`, and renders. Nothing in `draw` mutates state.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rustline widget_tui 2>&1 | tail -10`
Expected: all pass (Task 4's 13 plus the 4 new ones).

- [ ] **Step 6: Verify the non-TTY guard and, manually, the editor**

```bash
cargo build 2>&1 | tail -3
./target/debug/rustline widget edit < /dev/null; echo "exit=$?"
```
Expected: the hint message, `exit=1`, and the terminal is NOT left in raw mode.

Then manually, in a real terminal: `./target/debug/rustline --config /tmp/wt/config.toml widget edit` — move between columns, add/remove with space, reorder with `J`/`K`, press `w`, then `q`. Confirm the file changed and the terminal is restored.

- [ ] **Step 7: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add -A
git commit -m "feat(tui): ratatui widget editor with live preview and restore-on-panic guard"
```

---

### Task 6: tmux `prefix + W` binding and the `doctor` tmux ≥ 3.2 row

**Files:**
- Modify: `crates/rustline/src/tmux_conf.rs` (add the binding to `init_block`; update characterization tests)
- Modify: `crates/rustline/src/doctor.rs` (add the advisory `display-popup` row)
- Test: both files' existing `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `init_block`'s existing `@BINARY@` blanket replace and `InitBlockOpts`.
- Produces: no new public API — only emitted-text and report changes.

- [ ] **Step 1: Write the failing tests**

In `crates/rustline/src/tmux_conf.rs`'s `mod tests`:

```rust
#[test]
fn block_binds_the_widget_manager_popup_to_prefix_w() {
    let block = init_block(&InitBlockOpts {
        bar_bg: "colour234",
        fg: "colour252",
        two_line: false,
        mouse: false,
        interval: 5,
        binary: "/opt/bin/rustline",
    });
    assert!(
        block.contains(r#"bind-key W display-popup -E -w 80% -h 80% "/opt/bin/rustline widget edit""#),
        "prefix+W binding present with the resolved binary: {block}"
    );
    // No tmux format variable is interpolated into it (nothing to `#{q:}`).
    let line = block
        .lines()
        .find(|l| l.starts_with("bind-key W"))
        .expect("binding line");
    assert!(!line.contains("#{"), "no format var in the popup binding: {line}");
}

#[test]
fn the_popup_binding_is_emitted_regardless_of_the_mouse_answer() {
    for mouse in [false, true] {
        let block = init_block(&InitBlockOpts {
            bar_bg: "colour234",
            fg: "colour252",
            two_line: false,
            mouse,
            interval: 5,
            binary: "rl",
        });
        assert!(block.contains("bind-key W display-popup"), "mouse={mouse}");
    }
}
```

In `crates/rustline/src/doctor.rs`'s `mod tests`:

```rust
#[test]
fn popup_support_passes_at_3_2_and_warns_below() {
    assert_eq!(popup_status(Some((3, 2))), CheckStatus::Ok);
    assert_eq!(popup_status(Some((3, 4))), CheckStatus::Ok);
    assert_eq!(popup_status(Some((3, 1))), CheckStatus::Warn);
    assert_eq!(popup_status(None), CheckStatus::Warn);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rustline tmux_conf 2>&1 | tail -10`
Run: `cargo test -p rustline doctor 2>&1 | tail -10`
Expected: the tmux test fails on the missing binding; the doctor test fails to compile (`cannot find function popup_status`).

- [ ] **Step 3: Emit the binding**

In `crates/rustline/src/tmux_conf.rs`, add a const alongside the existing binding blocks:

```rust
/// The widget-manager popup binding. Emitted unconditionally, like the three
/// `MouseDown*Status` bindings — the `mouse` answer only controls
/// `set -g mouse on`, not whether bindings exist. No tmux format variable is
/// interpolated here, so there is nothing to `#{q:}`-escape; `@BINARY@` is
/// substituted with the shell-quoted absolute path by `init_block`'s blanket
/// replace (invariant #4). Needs tmux >= 3.2 for `display-popup`.
const WIDGET_POPUP_BINDING: &str = r##"
# rustline widget manager (prefix + W)
bind-key W display-popup -E -w 80% -h 80% "@BINARY@ widget edit"
"##;
```

and push it into the block in `init_block`, immediately after the mouse-binding
stanzas and before the block is returned:

```rust
    block.push_str(WIDGET_POPUP_BINDING);
```

Follow the surrounding code's exact ordering and spacing conventions — read
`init_block` and insert consistently. The `@BINARY@` replace at the end of
`init_block` already covers this const.

- [ ] **Step 4: Add the doctor row**

In `crates/rustline/src/doctor.rs`:

```rust
/// tmux version at which `display-popup` became available — what the
/// `prefix + W` widget-manager binding needs. Advisory only: the status line
/// itself works on 3.1.
const MIN_POPUP_TMUX_VERSION: (u32, u32) = (3, 2);

/// Advisory status for the widget-manager popup binding: `Ok` at tmux >= 3.2,
/// `Warn` below it or when the version can't be determined. Never `Fail` — a
/// missing popup does not break the status line, so this must not affect
/// doctor's exit code (the same shape as the daemon-reachability row).
fn popup_status(version: Option<(u32, u32)>) -> CheckStatus {
    match version {
        Some(v) if v >= MIN_POPUP_TMUX_VERSION => CheckStatus::Ok,
        _ => CheckStatus::Warn,
    }
}
```

and emit the row in `run`, reusing the already-parsed tmux version, with the
detail text: `"prefix + W widget manager needs tmux >= 3.2 (display-popup); the status line itself works"` on `Warn`, and `"display-popup available (prefix + W opens the widget manager)"` on `Ok`. Follow how the neighbouring rows are pushed and formatted, and make sure this row cannot flip the exit code (only `Fail` does that).

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rustline 2>&1 | tail -10`
Expected: all pass. Existing `init_block` characterization tests that assert on the whole block will need their expected text updated to include the new line — that is the intended signal, not a regression.

- [ ] **Step 6: Verify `--print` is still the legacy block**

Run: `cargo test -p rustline tmux_conf 2>&1 | grep -i print`
and confirm the `--print`/legacy characterization test still passes unchanged. `--print` emits no bindings, so it must be unaffected.

- [ ] **Step 7: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add -A
git commit -m "feat(init,doctor): prefix+W widget-manager popup binding and its tmux>=3.2 advisory row"
```

---

# Part B — the plugin exec capability

### Task 7: Wire types, config, and the canonical argv gate key

**Files:**
- Modify: `crates/rustline-abi/src/lib.rs` (add `ExecResult`, `CachedExecResult`)
- Modify: `crates/rustline-core/src/config.rs` (add `PluginConfig::allowed_commands`)
- Modify: `crates/rustline-wasm/src/capability.rs` (add `allowed_commands`, `DenialKind::Command`)
- Modify: `crates/rustline-wasm/src/abi.rs` (re-export the two new types)
- Create: `crates/rustline-wasm/src/argv.rs` (the `canonical_argv` function)
- Modify: `crates/rustline-wasm/src/lib.rs` (declare `mod argv;`, re-export)
- Test: `crates/rustline-wasm/src/argv.rs` and `capability.rs`'s test modules

**Interfaces:**
- Consumes: existing `AllowSet::compile`, `PluginConfig`, `DenialKind`.
- Produces:
  - `rustline_abi::{ExecResult, CachedExecResult}` (re-exported by `rustline_core` and `rustline_wasm::abi`)
  - `PluginConfig.allowed_commands: Vec<String>`
  - `CapabilityCtx.allowed_commands: AllowSet`
  - `DenialKind::Command` (serde `"command"`)
  - `pub fn rustline_wasm::canonical_argv(program: &str, args: &[String]) -> String`

- [ ] **Step 1: Write the failing tests**

Create `crates/rustline-wasm/src/argv.rs` with its test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn plain_args_join_with_single_spaces() {
        assert_eq!(canonical_argv("playerctl", &s(&["metadata"])), "playerctl metadata");
        assert_eq!(canonical_argv("git", &s(&["status", "--porcelain"])), "git status --porcelain");
        assert_eq!(canonical_argv("uname", &[]), "uname");
    }

    #[test]
    fn an_arg_containing_whitespace_is_quoted_so_it_cannot_masquerade_as_two_args() {
        // Without quoting, this would render identically to
        // canonical_argv("git", ["log", "--author=a", "b"]) and could match a
        // pattern written for that different call.
        assert_eq!(
            canonical_argv("git", &s(&["log", "--author=a b"])),
            "git log '--author=a b'"
        );
        assert_ne!(
            canonical_argv("git", &s(&["log", "--author=a b"])),
            canonical_argv("git", &s(&["log", "--author=a", "b"]))
        );
    }

    #[test]
    fn tabs_and_newlines_are_treated_as_whitespace_needing_quotes() {
        assert_eq!(canonical_argv("x", &s(&["a\tb"])), "x 'a\tb'");
        assert_eq!(canonical_argv("x", &s(&["a\nb"])), "x 'a\nb'");
    }

    #[test]
    fn an_embedded_single_quote_is_escaped_not_dropped() {
        assert_eq!(canonical_argv("x", &s(&["it's here"])), r#"x 'it'\''s here'"#);
    }

    #[test]
    fn an_empty_arg_becomes_empty_quotes_rather_than_vanishing() {
        assert_eq!(canonical_argv("x", &s(&["", "y"])), "x '' y");
    }

    #[test]
    fn a_program_needing_quotes_is_quoted_too() {
        assert_eq!(canonical_argv("/opt/my tools/bin", &[]), "'/opt/my tools/bin'");
    }
}
```

Add to `crates/rustline-wasm/src/capability.rs`'s `mod tests`:

```rust
#[test]
fn denial_kind_command_serializes_as_snake_case() {
    let json = serde_json::to_string(&DenialKind::Command).unwrap();
    assert_eq!(json, "\"command\"");
    let back: DenialKind = serde_json::from_str("\"command\"").unwrap();
    assert_eq!(back, DenialKind::Command);
}

#[test]
fn capability_ctx_compiles_the_command_allowlist_and_denies_by_default() {
    let pc = PluginConfig::default();
    let ctx = CapabilityCtx::from_config("p", &pc, std::path::PathBuf::from("/tmp"));
    assert!(
        !ctx.allowed_commands.allows("anything"),
        "an empty allowlist matches nothing (deny by default)"
    );

    let pc = PluginConfig {
        allowed_commands: vec!["playerctl metadata*".to_string()],
        ..PluginConfig::default()
    };
    let ctx = CapabilityCtx::from_config("p", &pc, std::path::PathBuf::from("/tmp"));
    assert!(ctx.allowed_commands.allows("playerctl metadata"));
    assert!(ctx.allowed_commands.allows("playerctl metadata --format x"));
    assert!(!ctx.allowed_commands.allows("playerctl play"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rustline-wasm argv 2>&1 | tail -10`
Expected: module not found / `canonical_argv` undefined.

- [ ] **Step 3: Add the wire types**

In `crates/rustline-abi/src/lib.rs`, after `CachedHttpResult`:

```rust
/// Result of a host-executed subprocess (`rl_exec`).
///
/// `ok` means "the command was allowed and the process ran to completion" —
/// **not** that it succeeded. A non-zero exit is data, not an error, so a
/// guest can render a fallback from `status`/`stderr` rather than losing the
/// distinction between "denied" and "ran and failed". Shared by both sides of
/// the WASM boundary; `#[serde(default)]` keeps a guest's decode
/// forward-compatible with a host that adds or omits a field.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExecResult {
    pub ok: bool,
    /// The process exit code, or `-1` when killed by a signal or timed out.
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
    /// Non-empty iff `ok` is false: denied, malformed args, spawn failure, or
    /// timeout.
    pub error: String,
    /// True iff stdout or stderr hit the host's output cap and was truncated.
    pub truncated: bool,
}

/// Result of a TTL-cached subprocess run (`rl_exec_cached`). `ok` means "a
/// usable result is present" (fresh OR stale) — the same convention as
/// [`CachedHttpResult`]; `stale` distinguishes them.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CachedExecResult {
    pub ok: bool,
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
    pub error: String,
    pub stale: bool,
    pub age_secs: i64,
    pub truncated: bool,
}
```

Re-export from `crates/rustline-core/src/lib.rs` alongside the other abi
re-exports, and from `crates/rustline-wasm/src/abi.rs`'s existing
`pub use rustline_abi::{...}` line.

- [ ] **Step 4: Add the config field**

In `crates/rustline-core/src/config.rs`'s `PluginConfig`:

```rust
    /// Command allow-patterns for the exec capability. Each entry is a glob by
    /// default, or a regex when prefixed `re:`, matched against the
    /// **canonical argv string** (`rustline_wasm::canonical_argv`) — the whole
    /// command line, not just the program. Empty (the default) matches
    /// nothing: deny by default, like the other two allowlists.
    #[serde(default)]
    pub allowed_commands: Vec<String>,
```

and add `allowed_commands: Vec::new(),` to its `impl Default`.

- [ ] **Step 5: Add `canonical_argv`**

Prepend to `crates/rustline-wasm/src/argv.rs`:

```rust
//! The canonical argv string: the single key an `allowed_commands` pattern is
//! matched against.
//!
//! This is a **matching key only** — it is never executed. The host always
//! spawns `program` with the `args` vector directly, with no shell anywhere in
//! the path, so nothing ever re-parses this string. The quoting exists purely
//! so two *different* argv vectors can never render to the same string: without
//! it, `["log", "--author=a b"]` and `["log", "--author=a", "b"]` would look
//! identical to a pattern, and a grant written for one would silently cover the
//! other.

/// Characters that force an argument to be quoted in the canonical form.
fn needs_quotes(s: &str) -> bool {
    s.is_empty()
        || s.chars()
            .any(|c| c.is_whitespace() || matches!(c, '\'' | '"' | '\\'))
}

/// Render one argument: bare when unambiguous, else single-quoted with any
/// embedded single quote escaped as `'\''` (POSIX-style, for readability in
/// config files and denial records).
fn quote(s: &str) -> String {
    if !needs_quotes(s) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Render `program` + `args` to the string an `allowed_commands` pattern is
/// matched against. See the module docs for why this is quoted.
pub fn canonical_argv(program: &str, args: &[String]) -> String {
    let mut out = quote(program);
    for arg in args {
        out.push(' ');
        out.push_str(&quote(arg));
    }
    out
}
```

Declare it in `crates/rustline-wasm/src/lib.rs` (`mod argv;` + `pub use argv::canonical_argv;`).

- [ ] **Step 6: Extend `CapabilityCtx` and `DenialKind`**

In `crates/rustline-wasm/src/capability.rs`:

```rust
pub enum DenialKind {
    /// An HTTP GET (cached or uncached) denied by `allowed_urls`.
    Url,
    /// A file read/write denied by `allowed_paths`.
    Path,
    /// A subprocess run (cached or uncached) denied by `allowed_commands`.
    Command,
}
```

Add `pub allowed_commands: AllowSet,` to `CapabilityCtx` and
`allowed_commands: AllowSet::compile(&pc.allowed_commands),` to `from_config`.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p rustline-wasm 2>&1 | tail -10`
Run: `cargo test --workspace 2>&1 | tail -10`
Expected: all pass. Fix any non-exhaustive `match` on `DenialKind` the new variant surfaces (expect one in `denials.rs` or `plugin_cmd.rs`).

- [ ] **Step 8: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add -A
git commit -m "feat(abi,wasm): ExecResult wire types, allowed_commands, and the canonical argv gate key"
```

---

### Task 8: `perform_exec` / `perform_exec_cached` + the `Runner` seam

The load-bearing security task. Every denied-case test here is required by invariant N1.

**Files:**
- Create: `crates/rustline-wasm/src/run.rs` (`Runner` trait + `ProcessRunner`)
- Modify: `crates/rustline-wasm/src/cache.rs` (namespace `cache_path`)
- Modify: `crates/rustline-wasm/src/perform.rs` (the two effect functions)
- Modify: `crates/rustline-wasm/src/lib.rs` (`mod run;`, re-export)
- Test: `run.rs` and `perform.rs` test modules

**Interfaces:**
- Consumes: Task 7's `canonical_argv`, `CapabilityCtx.allowed_commands`, `DenialKind::Command`, `ExecResult`, `CachedExecResult`; existing `cache::{CacheEntry, age_secs, is_fresh, read_entry, write_entry}`, `check_cap`.
- Produces:
  - `pub trait Runner { fn run(&self, program: &str, args: &[String]) -> Result<(i32, String, String), String>; }`
  - `pub struct ProcessRunner;` implementing it
  - `pub fn perform_exec(ctx, program, args, runner) -> ExecResult`
  - `pub fn perform_exec_cached(ctx, program, args, ttl_secs, now, runner) -> CachedExecResult`
  - `cache::cache_path(state_dir, namespace, key)` (signature change — update the http call site)

- [ ] **Step 1: Write the failing tests**

In `crates/rustline-wasm/src/perform.rs`'s `mod tests`, add (following the file's existing test-context helpers — read them and reuse the same `CapabilityCtx` construction):

```rust
/// Records every run it is asked to perform, so a test can assert a denied
/// call never reached the runner at all.
struct RecordingRunner {
    calls: std::sync::Mutex<Vec<(String, Vec<String>)>>,
    reply: Result<(i32, String, String), String>,
}

impl RecordingRunner {
    fn ok(status: i32, stdout: &str) -> Self {
        Self {
            calls: std::sync::Mutex::new(Vec::new()),
            reply: Ok((status, stdout.to_string(), String::new())),
        }
    }
    fn failing(message: &str) -> Self {
        Self {
            calls: std::sync::Mutex::new(Vec::new()),
            reply: Err(message.to_string()),
        }
    }
    fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.calls.lock().unwrap().clone()
    }
}

impl Runner for RecordingRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<(i32, String, String), String> {
        self.calls
            .lock()
            .unwrap()
            .push((program.to_string(), args.to_vec()));
        self.reply.clone()
    }
}

fn argv(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn exec_denied_never_reaches_the_runner_and_records_the_denial() {
    let (ctx, observer) = ctx_with_commands(&[], /* recording observer */);
    let runner = RecordingRunner::ok(0, "should not happen");
    let out = perform_exec(&ctx, "playerctl", &argv(&["metadata"]), &runner);

    assert!(!out.ok);
    assert!(out.error.contains("playerctl metadata"), "{}", out.error);
    assert!(runner.calls().is_empty(), "gate-first: no spawn on a denied argv");
    assert_eq!(
        observer.seen(),
        vec![("p".to_string(), DenialKind::Command, "playerctl metadata".to_string())]
    );
}

#[test]
fn exec_gates_on_the_whole_argv_not_just_the_program() {
    let (ctx, _o) = ctx_with_commands(&["git status*"]);
    let runner = RecordingRunner::ok(0, "");
    // Same program, different subcommand → denied.
    let out = perform_exec(&ctx, "git", &argv(&["push", "--force"]), &runner);
    assert!(!out.ok, "a grant for `git status*` must not cover `git push`");
    assert!(runner.calls().is_empty());

    let out = perform_exec(&ctx, "git", &argv(&["status", "--porcelain"]), &runner);
    assert!(out.ok);
    assert_eq!(runner.calls().len(), 1);
}

#[test]
fn exec_allowed_returns_status_and_streams_verbatim() {
    let (ctx, _o) = ctx_with_commands(&["echo*"]);
    let runner = RecordingRunner::ok(0, "hello\n");
    let out = perform_exec(&ctx, "echo", &argv(&["hello"]), &runner);
    assert!(out.ok);
    assert_eq!(out.status, 0);
    assert_eq!(out.stdout, "hello\n");
    assert!(out.error.is_empty());
    // The spawn got the program and args untouched — no shell in between.
    assert_eq!(runner.calls(), vec![("echo".to_string(), argv(&["hello"]))]);
}

#[test]
fn exec_a_nonzero_exit_is_data_not_an_error() {
    let (ctx, _o) = ctx_with_commands(&["false*"]);
    let runner = RecordingRunner::ok(1, "");
    let out = perform_exec(&ctx, "false", &[], &runner);
    assert!(out.ok, "the process ran; `ok` is not about its exit code");
    assert_eq!(out.status, 1);
}

#[test]
fn exec_a_spawn_failure_is_reported_without_panicking() {
    let (ctx, _o) = ctx_with_commands(&["missing*"]);
    let runner = RecordingRunner::failing("no such file");
    let out = perform_exec(&ctx, "missing", &[], &runner);
    assert!(!out.ok);
    assert_eq!(out.status, -1);
    assert!(out.error.contains("no such file"));
}

#[test]
fn exec_cached_denied_touches_neither_the_runner_nor_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _o) = ctx_with_commands_in(&[], dir.path());
    let runner = RecordingRunner::ok(0, "x");
    let out = perform_exec_cached(&ctx, "date", &[], 60, NOW, &runner);
    assert!(!out.ok);
    assert!(runner.calls().is_empty());
    assert!(
        !ctx.state_dir().join("__exec_cache__").exists(),
        "a denied call must not create the cache dir"
    );
}

#[test]
fn exec_cached_serves_a_fresh_entry_without_rerunning() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _o) = ctx_with_commands_in(&["date*"], dir.path());
    let runner = RecordingRunner::ok(0, "first");
    let a = perform_exec_cached(&ctx, "date", &[], 3600, NOW, &runner);
    assert_eq!(a.stdout, "first");
    assert!(!a.stale);

    let runner2 = RecordingRunner::ok(0, "second");
    let b = perform_exec_cached(&ctx, "date", &[], 3600, NOW, &runner2);
    assert_eq!(b.stdout, "first", "served from cache");
    assert!(runner2.calls().is_empty(), "fresh hit must not re-run");
}

#[test]
fn exec_cached_reruns_once_the_ttl_has_expired() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _o) = ctx_with_commands_in(&["date*"], dir.path());
    perform_exec_cached(&ctx, "date", &[], 60, NOW, &RecordingRunner::ok(0, "first"));

    let runner = RecordingRunner::ok(0, "second");
    let out = perform_exec_cached(&ctx, "date", &[], 60, LATER, &runner);
    assert_eq!(out.stdout, "second");
    assert_eq!(runner.calls().len(), 1);
}

#[test]
fn exec_cached_does_not_cache_a_nonzero_exit() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _o) = ctx_with_commands_in(&["flaky*"], dir.path());
    perform_exec_cached(&ctx, "flaky", &[], 3600, NOW, &RecordingRunner::ok(3, "bad"));

    // Nothing was cached, so the next call runs again rather than serving `bad`.
    let runner = RecordingRunner::ok(0, "good");
    let out = perform_exec_cached(&ctx, "flaky", &[], 3600, NOW, &runner);
    assert_eq!(out.stdout, "good");
    assert_eq!(runner.calls().len(), 1);
}

#[test]
fn exec_cached_serves_the_last_good_entry_stale_when_a_refresh_fails() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _o) = ctx_with_commands_in(&["date*"], dir.path());
    perform_exec_cached(&ctx, "date", &[], 60, NOW, &RecordingRunner::ok(0, "good"));

    let out = perform_exec_cached(&ctx, "date", &[], 60, LATER, &RecordingRunner::failing("boom"));
    assert!(out.ok, "a usable (stale) body is present");
    assert!(out.stale);
    assert_eq!(out.stdout, "good");
    assert!(out.age_secs > 0);
}

#[test]
fn exec_and_http_caches_never_collide_on_the_same_key_string() {
    let dir = tempfile::tempdir().unwrap();
    let http = crate::cache::cache_path(dir.path(), "__http_cache__", "same-key");
    let exec = crate::cache::cache_path(dir.path(), "__exec_cache__", "same-key");
    assert_ne!(http, exec);
}
```

The implementer must adapt `ctx_with_commands`/`ctx_with_commands_in`/`NOW`/`LATER` to the helpers `perform.rs`'s existing tests already use for the http/file cases — read them first and follow the same construction, adding a recording `DenialObserver` if one isn't already there.

Create `crates/rustline-wasm/src/run.rs` with its own tests:

```rust
#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    #[test]
    fn process_runner_propagates_a_zero_exit_and_stdout() {
        let (status, stdout, _stderr) = ProcessRunner
            .run("echo", &["hi".to_string()])
            .expect("echo runs");
        assert_eq!(status, 0);
        assert_eq!(stdout.trim_end(), "hi");
    }

    #[test]
    fn process_runner_propagates_a_nonzero_exit() {
        let (status, _out, _err) = ProcessRunner.run("false", &[]).expect("false runs");
        assert_ne!(status, 0);
    }

    #[test]
    fn process_runner_captures_stderr_separately() {
        let (_status, stdout, stderr) = ProcessRunner
            .run("sh", &["-c".to_string(), "echo out; echo err >&2".to_string()])
            .expect("sh runs");
        assert_eq!(stdout.trim_end(), "out");
        assert_eq!(stderr.trim_end(), "err");
    }

    #[test]
    fn process_runner_reports_a_missing_program_as_an_error_not_a_panic() {
        let err = ProcessRunner
            .run("definitely-not-a-real-program-xyz", &[])
            .unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn process_runner_kills_a_command_that_outlives_the_timeout() {
        let start = std::time::Instant::now();
        let out = ProcessRunner.run("sleep", &["30".to_string()]);
        assert!(out.is_err(), "a timed-out run is an error: {out:?}");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(15),
            "returned promptly rather than waiting out the sleep"
        );
    }

    #[test]
    fn process_runner_truncates_output_beyond_the_cap() {
        // 200 KiB of 'x' — well past MAX_OUTPUT_BYTES.
        let (_status, stdout, _stderr) = ProcessRunner
            .run(
                "sh",
                &["-c".to_string(), "yes x | head -c 204800".to_string()],
            )
            .expect("sh runs");
        assert!(stdout.len() <= MAX_OUTPUT_BYTES, "capped: {}", stdout.len());
    }

    #[test]
    fn process_runner_gives_a_stdin_reader_eof_rather_than_hanging() {
        let start = std::time::Instant::now();
        let (_status, stdout, _stderr) = ProcessRunner
            .run("sh", &["-c".to_string(), "cat".to_string()])
            .expect("cat runs");
        assert!(stdout.is_empty());
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rustline-wasm 2>&1 | tail -15`
Expected: compile errors for `Runner`, `ProcessRunner`, `perform_exec`, `perform_exec_cached`.

- [ ] **Step 3: Namespace the cache path**

In `crates/rustline-wasm/src/cache.rs`:

```rust
/// `<state_dir>/<namespace>/<hash>.json` — the cache file for `key` within a
/// namespace. The namespace keeps the HTTP and exec caches in separate
/// subdirectories so a URL and a command line that happen to hash the same can
/// never read each other's entries. Both stay under the plugin's own state
/// dir, so `check_cap`'s quota accounting (invariant N3) covers them unchanged.
pub fn cache_path(state_dir: &Path, namespace: &str, key: &str) -> PathBuf {
    state_dir
        .join(namespace)
        .join(format!("{:016x}.json", fnv1a(key)))
}

/// The HTTP response cache's namespace.
pub const HTTP_NAMESPACE: &str = "__http_cache__";
/// The exec result cache's namespace.
pub const EXEC_NAMESPACE: &str = "__exec_cache__";
```

Update `perform_http_get_cached`'s call site to `cache_path(&dir, HTTP_NAMESPACE, url)` and any existing test referencing the old two-arg form.

- [ ] **Step 4: Implement the runner**

Prepend to `crates/rustline-wasm/src/run.rs`:

```rust
//! Subprocess execution for the exec capability.
//!
//! `Runner` is the seam (mirroring `fetch::Fetcher`) that lets every gate test
//! in `perform.rs` run without spawning anything: the capability decision is
//! made before `run` is ever called, and a recording fake proves it.
//!
//! `ProcessRunner` is the only production implementation. It spawns the
//! program **directly — there is no shell anywhere in this path**, so nothing
//! re-parses the arguments and there is no quoting or word-splitting surface.
//! A guest that wants a shell must be granted one explicitly and visibly
//! (`allowed_commands = ["sh -c *"]`); the host never introduces one.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Wall-clock bound on one child process. Deliberately under Extism's 10 s
/// plugin timeout so a hung command surfaces to the guest as a renderable
/// failure result rather than killing the whole plugin render (invariant N2).
pub const EXEC_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-stream output cap. Beyond this the stream is truncated and the result
/// is flagged `truncated`.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// How the host runs a command. The `perform_exec*` gate decides *whether* to
/// call this; the implementation decides *how*.
pub trait Runner {
    /// Run `program` with `args`. `Ok((exit_code, stdout, stderr))` when the
    /// process ran to completion (whatever its exit code); `Err(message)` on a
    /// spawn failure or a timeout.
    fn run(&self, program: &str, args: &[String]) -> Result<(i32, String, String), String>;
}

/// The production runner: direct spawn, no shell, piped stdout/stderr, stdin
/// closed, inherited environment and working directory, wall-clock bounded,
/// output capped.
pub struct ProcessRunner;

impl Runner for ProcessRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<(i32, String, String), String> {
        let mut child = Command::new(program)
            .args(args)
            // A child that reads stdin gets EOF immediately instead of
            // blocking until the timeout.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to spawn {program}: {e}"))?;

        // Drain both pipes on their own threads: a child that fills a pipe
        // buffer would otherwise block forever while we wait on it.
        let mut stdout_pipe = child.stdout.take();
        let mut stderr_pipe = child.stderr.take();
        let out_handle = std::thread::spawn(move || read_capped(&mut stdout_pipe));
        let err_handle = std::thread::spawn(move || read_capped(&mut stderr_pipe));

        let deadline = Instant::now() + EXEC_TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(format!(
                            "{program} exceeded the {}s exec timeout",
                            EXEC_TIMEOUT.as_secs()
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => return Err(format!("failed to wait on {program}: {e}")),
            }
        };

        let stdout = out_handle.join().unwrap_or_default();
        let stderr = err_handle.join().unwrap_or_default();
        Ok((status.code().unwrap_or(-1), stdout, stderr))
    }
}

/// Read at most [`MAX_OUTPUT_BYTES`] from a pipe, lossily as UTF-8 so binary
/// output degrades to replacement characters instead of failing the whole run.
fn read_capped<R: Read>(pipe: &mut Option<R>) -> String {
    let Some(reader) = pipe.as_mut() else {
        return String::new();
    };
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    while buf.len() < MAX_OUTPUT_BYTES {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    buf.truncate(MAX_OUTPUT_BYTES);
    String::from_utf8_lossy(&buf).into_owned()
}
```

Declare it in `lib.rs` (`mod run;` + `pub use run::{ProcessRunner, Runner};`).

- [ ] **Step 5: Implement the two effect functions**

Add to `crates/rustline-wasm/src/perform.rs`:

```rust
/// Run a command on the guest's behalf, gated by `allowed_commands`.
///
/// **Gate first (invariant N1):** a denied argv spawns nothing. The gate is on
/// the whole canonical argv — `git status*` does not cover `git push` — and the
/// spawn passes `program`/`args` straight through, with no shell.
pub fn perform_exec(
    ctx: &CapabilityCtx,
    program: &str,
    args: &[String],
    runner: &dyn Runner,
) -> ExecResult {
    let candidate = canonical_argv(program, args);
    if !ctx.allowed_commands.allows(&candidate) {
        ctx.observe_denial(DenialKind::Command, &candidate);
        return ExecResult {
            ok: false,
            status: -1,
            error: format!("command not allowed: {candidate}"),
            ..Default::default()
        };
    }
    match runner.run(program, args) {
        Ok((status, stdout, stderr)) => ExecResult {
            ok: true,
            status,
            truncated: stdout.len() >= MAX_OUTPUT_BYTES || stderr.len() >= MAX_OUTPUT_BYTES,
            stdout,
            stderr,
            error: String::new(),
        },
        Err(error) => ExecResult {
            ok: false,
            status: -1,
            error,
            ..Default::default()
        },
    }
}

/// TTL-cached command run. Gate-first (a denied argv runs nothing and touches
/// no cache); a fresh cache hit is served without running; only a **zero-exit**
/// run is cached; on a failed or non-zero refresh the last-good entry is served
/// stale if one exists. Exactly [`perform_http_get_cached`]'s shape, with "2xx"
/// replaced by "exit 0".
pub fn perform_exec_cached(
    ctx: &CapabilityCtx,
    program: &str,
    args: &[String],
    ttl_secs: i64,
    now: &str,
    runner: &dyn Runner,
) -> CachedExecResult {
    let candidate = canonical_argv(program, args);
    if !ctx.allowed_commands.allows(&candidate) {
        ctx.observe_denial(DenialKind::Command, &candidate);
        return CachedExecResult {
            ok: false,
            status: -1,
            error: format!("command not allowed: {candidate}"),
            ..Default::default()
        };
    }
    let dir = ctx.state_dir();
    let path = cache_path(&dir, EXEC_NAMESPACE, &candidate);
    let cached = read_entry(&path);
    if let Some(entry) = &cached {
        if let Some(age) = age_secs(now, &entry.fetched_at) {
            if is_fresh(age, ttl_secs) {
                return CachedExecResult {
                    ok: true,
                    status: entry.status as i32,
                    stdout: entry.body.clone(),
                    stale: false,
                    age_secs: age,
                    ..Default::default()
                };
            }
        }
    }
    match runner.run(program, args) {
        Ok((0, stdout, stderr)) => {
            let entry = CacheEntry {
                fetched_at: now.to_string(),
                status: 0,
                body: stdout.clone(),
            };
            if let Ok(serialized) = serde_json::to_string(&entry) {
                let _ = write_entry(&dir, &path, &serialized, ctx.max_state_bytes);
            }
            CachedExecResult {
                ok: true,
                status: 0,
                truncated: stdout.len() >= MAX_OUTPUT_BYTES || stderr.len() >= MAX_OUTPUT_BYTES,
                stdout,
                stderr,
                stale: false,
                age_secs: 0,
                error: String::new(),
            }
        }
        // Ran, but non-zero: don't cache it; return it as-is (it's still data).
        Ok((status, stdout, stderr)) => CachedExecResult {
            ok: true,
            status,
            truncated: stdout.len() >= MAX_OUTPUT_BYTES || stderr.len() >= MAX_OUTPUT_BYTES,
            stdout,
            stderr,
            stale: false,
            age_secs: 0,
            error: String::new(),
        },
        // Couldn't run at all: serve the last-good entry stale if we have one.
        Err(error) => match cached {
            Some(entry) => {
                let age = age_secs(now, &entry.fetched_at).unwrap_or(0);
                CachedExecResult {
                    ok: true,
                    status: entry.status as i32,
                    stdout: entry.body,
                    stale: true,
                    age_secs: age,
                    ..Default::default()
                }
            }
            None => CachedExecResult {
                ok: false,
                status: -1,
                error,
                ..Default::default()
            },
        },
    }
}
```

Add the needed imports at the top of `perform.rs`: `crate::argv::canonical_argv`,
`crate::cache::{EXEC_NAMESPACE, ...}`, `crate::run::{MAX_OUTPUT_BYTES, Runner}`,
and `rustline_abi::{CachedExecResult, ExecResult}` (via the crate's existing
`abi` re-exports, matching how `HttpResult` is imported there).

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p rustline-wasm 2>&1 | tail -15`
Expected: all pass, including every denied-case test.

- [ ] **Step 7: Run the full suite**

Run: `cargo test --workspace 2>&1 | tail -10`
Expected: all pass.

- [ ] **Step 8: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add -A
git commit -m "feat(wasm): perform_exec/perform_exec_cached behind a Runner seam, gated on the whole argv"
```

---

### Task 9: Bind `rl_exec` / `rl_exec_cached` as host functions

**Files:**
- Modify: `crates/rustline-wasm/src/host.rs` (two `host_fn!` wrappers, register them in `build_plugin_with_cache`)
- Test: `crates/rustline-wasm/src/host.rs`'s test module (argv-decode helper)

**Interfaces:**
- Consumes: Task 8's `perform_exec`/`perform_exec_cached`/`ProcessRunner`.
- Produces: the `rl_exec` and `rl_exec_cached` guest imports.

- [ ] **Step 1: Write the failing test**

In `crates/rustline-wasm/src/host.rs`'s `mod tests` (create the module if absent):

```rust
#[test]
fn decode_args_accepts_a_json_array_and_rejects_anything_else() {
    assert_eq!(
        decode_args(r#"["metadata","--format","{{title}}"]"#).unwrap(),
        vec![
            "metadata".to_string(),
            "--format".to_string(),
            "{{title}}".to_string()
        ]
    );
    assert_eq!(decode_args("[]").unwrap(), Vec::<String>::new());
    assert!(decode_args("not json").is_err());
    assert!(decode_args(r#"{"a":1}"#).is_err());
    assert!(decode_args(r#"[1,2]"#).is_err(), "numbers are not args");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rustline-wasm host 2>&1 | tail -10`
Expected: `cannot find function decode_args`.

- [ ] **Step 3: Implement the host functions**

In `crates/rustline-wasm/src/host.rs`:

```rust
/// Decode the JSON array a guest passes as its `args`. Extism host functions
/// carry scalars and strings, so a vector crosses the boundary encoded — the
/// same "encode it, parse host-side" shape as `rl_http_get_cached`'s
/// `ttl_secs: String`. A malformed value is an error, never a panic and never
/// a spawn.
fn decode_args(json: &str) -> Result<Vec<String>, String> {
    serde_json::from_str::<Vec<String>>(json).map_err(|e| format!("invalid args: {e}"))
}

host_fn!(rl_exec(user_data: CapabilityCtx; program: String, args_json: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    let result = match decode_args(&args_json) {
        Ok(args) => perform_exec(&ctx, &program, &args, &ProcessRunner),
        // Malformed args never reach the gate or a spawn.
        Err(error) => ExecResult { ok: false, status: -1, error, ..Default::default() },
    };
    Ok(json(&result))
});

host_fn!(rl_exec_cached(user_data: CapabilityCtx; program: String, args_json: String, ttl_secs: String, now: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    let ttl: i64 = ttl_secs.parse().unwrap_or(0);
    let result = match decode_args(&args_json) {
        Ok(args) => perform_exec_cached(&ctx, &program, &args, ttl, &now, &ProcessRunner),
        Err(error) => CachedExecResult { ok: false, status: -1, error, ..Default::default() },
    };
    Ok(json(&result))
});
```

Add the imports (`perform_exec`, `perform_exec_cached`, `ProcessRunner`,
`ExecResult`, `CachedExecResult`) to `host.rs`'s existing `use` lists, and
register both wrappers in `build_plugin_with_cache`'s `.with_function(...)`
chain alongside the existing seven — follow the exact call shape used there.

Update the module doc comment and `build_plugin`'s doc comment: **nine** host
functions (eight capability-gated plus the capability-free `rl_log`).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rustline-wasm 2>&1 | tail -10`
Expected: all pass.

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add -A
git commit -m "feat(wasm): bind rl_exec and rl_exec_cached host functions"
```

---

### Task 10: Manifest `requested_commands`, `plugin approve` warning, `plugin cmd` group

**Files:**
- Modify: `crates/rustline-wasm/src/manifest.rs` (add `requested_commands`)
- Modify: `crates/rustline/src/plugin_cmd.rs` (`Kind::Command`, `print_manifest`, `write_grants`, the approve warning, `list`'s output + JSON)
- Modify: `crates/rustline/src/cli.rs` (add `PluginCmd::Cmd(PatternCmd)`)
- Test: both files' test modules

**Interfaces:**
- Consumes: Task 7's `PluginConfig.allowed_commands`; existing `PatternCmd`, `Kind`, `mutate`, `append_unique`, `write_grants`.
- Produces: `PluginManifest.requested_commands: Vec<String>`; the `plugin cmd list|add|remove` subcommand.

- [ ] **Step 1: Write the failing tests**

In `crates/rustline-wasm/src/manifest.rs`'s `mod tests`:

```rust
#[test]
fn a_manifest_parses_requested_commands() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("p.toml"),
        "name = \"p\"\nversion = \"1\"\nrequested_commands = [\"playerctl metadata*\"]\n",
    )
    .unwrap();
    let m = resolve_manifest(dir.path(), "p").unwrap();
    assert_eq!(m.requested_commands, ["playerctl metadata*"]);
    assert!(m.requested_urls.is_empty());
}

#[test]
fn a_manifest_without_requested_commands_still_parses() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("p.toml"), "requested_urls = [\"https://a/*\"]\n").unwrap();
    let m = resolve_manifest(dir.path(), "p").unwrap();
    assert!(m.requested_commands.is_empty());
}
```

In `crates/rustline/src/plugin_cmd.rs`'s `mod tests`:

```rust
#[test]
fn write_grants_writes_requested_commands_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    std::fs::write(&cfg, "[plugins.w]\n").unwrap();

    let mut m = manifest(&[], &[]);
    m.requested_commands = vec!["playerctl metadata*".to_string()];
    write_grants(&cfg, "w", &m);

    let text = std::fs::read_to_string(&cfg).unwrap();
    assert_eq!(list_of(&text, "w", "allowed_commands"), ["playerctl metadata*"]);
    // Nothing was widened beyond what was requested.
    assert!(!text.contains("allowed_urls"), "{text}");
}

#[test]
fn approving_commands_twice_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    std::fs::write(&cfg, "[plugins.w]\n").unwrap();
    let mut m = manifest(&[], &[]);
    m.requested_commands = vec!["git status*".to_string()];
    write_grants(&cfg, "w", &m);
    write_grants(&cfg, "w", &m);
    let text = std::fs::read_to_string(&cfg).unwrap();
    assert_eq!(list_of(&text, "w", "allowed_commands"), ["git status*"]);
}

#[test]
fn the_command_kind_maps_to_the_allowed_commands_key() {
    assert_eq!(Kind::Command.key(), "allowed_commands");
}

#[test]
fn a_manifest_requesting_commands_prints_the_execution_warning() {
    let mut m = manifest(&[], &[]);
    m.requested_commands = vec!["git status*".to_string()];
    let text = manifest_report(&m);
    assert!(text.contains("allowed_commands"), "{text}");
    assert!(text.contains("git status*"), "{text}");
    assert!(
        text.to_lowercase().contains("runs real programs"),
        "the exec warning is shown: {text}"
    );
}

#[test]
fn a_manifest_without_commands_prints_no_execution_warning() {
    let m = manifest(&["https://a/*"], &[]);
    let text = manifest_report(&m);
    assert!(!text.to_lowercase().contains("runs real programs"), "{text}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rustline-wasm manifest 2>&1 | tail -10`
Run: `cargo test -p rustline plugin_cmd 2>&1 | tail -10`
Expected: `requested_commands` field missing; `Kind::Command` and `manifest_report` undefined.

- [ ] **Step 3: Extend the manifest**

In `crates/rustline-wasm/src/manifest.rs`'s `PluginManifest`:

```rust
    /// Command allow-patterns the plugin asks the user to approve. Written
    /// verbatim into `allowed_commands` by `plugin approve` — never widened.
    #[serde(default)]
    pub requested_commands: Vec<String>,
```

- [ ] **Step 4: Extend the approve/list flow**

In `crates/rustline/src/plugin_cmd.rs`:

```rust
enum Kind {
    Url,
    Path,
    Command,
}

impl Kind {
    fn key(&self) -> &'static str {
        match self {
            Kind::Url => "allowed_urls",
            Kind::Path => "allowed_paths",
            Kind::Command => "allowed_commands",
        }
    }
}
```

Refactor `print_manifest` into a pure `manifest_report(&PluginManifest) -> String`
that `approve` prints, so the warning is testable:

```rust
/// The text `approve` shows before its confirmation prompt: the plugin's
/// identity, exactly what it requests, and — when it asks for commands — a
/// warning that this capability is categorically different from reading a URL
/// or a file. The warning is part of the report (not the prompt) so it also
/// lands in a `--yes` run's output and in logs.
fn manifest_report(m: &PluginManifest) -> String {
    let name = if m.name.is_empty() { "?" } else { &m.name };
    let version = if m.version.is_empty() { "?" } else { &m.version };
    let mut out = format!("plugin {name} (version {version}) requests:\n");
    out.push_str(&requests_block("allowed_urls", &m.requested_urls));
    out.push_str(&requests_block("allowed_paths", &m.requested_paths));
    out.push_str(&requests_block("allowed_commands", &m.requested_commands));
    if !m.requested_commands.is_empty() {
        out.push_str(
            "\n  ! allowed_commands runs real programs on your machine with your\n\
             \x20 ! environment and permissions. Approve only patterns you understand.\n",
        );
    }
    out
}

/// One requested-capability list under `label`, or `(none)`.
fn requests_block(label: &str, entries: &[String]) -> String {
    let mut out = format!("  {label}:\n");
    if entries.is_empty() {
        out.push_str("    (none)\n");
    } else {
        for e in entries {
            out.push_str(&format!("    {e}\n"));
        }
    }
    out
}
```

Replace `print_manifest(&manifest)` in `approve` with `print!("{}", manifest_report(&manifest));`,
extend `approve`'s "requests no capabilities" early-return to also check
`requested_commands.is_empty()`, and add the third `append_unique` call to
`write_grants`:

```rust
    if !m.requested_commands.is_empty() {
        append_unique(
            allowlist_array(table, plugin, Kind::Command.key()),
            &m.requested_commands,
        );
    }
```

In `list`/`plugin_list_json`, add `allowed_commands` alongside the other two allowlists.

In `run`'s dispatch, add `PluginCmd::Cmd(pc) => pattern_cmd(pc, Kind::Command, config_path),`.

- [ ] **Step 5: Add the CLI subcommand**

In `crates/rustline/src/cli.rs`'s `PluginCmd`, after `Path(PatternCmd)`:

```rust
    /// Manage a plugin's command allowlist (the exec capability). Patterns are
    /// matched against the whole canonical argv, not just the program.
    #[command(subcommand)]
    Cmd(PatternCmd),
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --workspace 2>&1 | tail -10`
Expected: all pass.

- [ ] **Step 7: Verify the approve flow end to end**

```bash
cargo build 2>&1 | tail -3
TMP=$(mktemp -d); PLUG=$(mktemp -d)
printf 'name = "demo"\nversion = "0.1"\nrequested_commands = ["uname -a"]\n' > "$PLUG/demo.toml"
printf '' > "$TMP/config.toml"
./target/debug/rustline --config "$TMP/config.toml" plugin approve demo --plugin-dir "$PLUG" --yes
cat "$TMP/config.toml"
./target/debug/rustline --config "$TMP/config.toml" plugin cmd list demo
./target/debug/rustline --config "$TMP/config.toml" plugin cmd remove demo "uname -a"
cat "$TMP/config.toml"
```

Expected: the approve output contains the `! allowed_commands runs real programs` warning; the config gains `allowed_commands = ["uname -a"]`; `cmd list` prints it; `cmd remove` takes it back out. (If `plugin approve` has no `--plugin-dir` flag, use the resolution the command actually supports — read `main.rs`'s dispatch arm.)

- [ ] **Step 8: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add -A
git commit -m "feat(plugin): requested_commands manifests, an exec warning on approve, and plugin cmd list|add|remove"
```

---

### Task 11: SDK `exec` / `exec_cached` wrappers

**Files:**
- Modify: `crates/rustline-plugin-sdk/src/lib.rs`
- Test: its own `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: Task 7's `ExecResult`/`CachedExecResult`; Task 9's `rl_exec`/`rl_exec_cached` imports.
- Produces: `pub fn exec(program: &str, args: &[&str]) -> Result<ExecResult, HostError>` and `pub fn exec_cached(program: &str, args: &[&str], ttl_secs: u64, now: &str) -> Result<CachedExecResult, HostError>`, plus re-exports of the two result types.

- [ ] **Step 1: Write the failing tests**

In `crates/rustline-plugin-sdk/src/lib.rs`'s `mod tests`:

```rust
#[test]
fn exec_on_the_host_target_is_unavailable_not_a_panic() {
    // On non-wasm there is no host to call; the wrappers degrade so a
    // plugin's pure logic still compiles and unit-tests under `cargo test`.
    assert!(matches!(exec("echo", &["hi"]), Err(HostError::Unavailable)));
    assert!(matches!(
        exec_cached("echo", &["hi"], 60, "2026-07-24T00:00:00Z"),
        Err(HostError::Unavailable)
    ));
}

#[test]
fn exec_result_decodes_a_host_json_response() {
    let json = r#"{"ok":true,"status":0,"stdout":"hi\n","stderr":"","error":"","truncated":false}"#;
    let out: ExecResult = serde_json::from_str(json).unwrap();
    assert!(out.ok);
    assert_eq!(out.stdout, "hi\n");
}

#[test]
fn exec_result_decode_tolerates_a_host_that_omits_fields() {
    // Forward-compat: struct-level #[serde(default)] (invariant #2).
    let out: ExecResult = serde_json::from_str(r#"{"ok":true}"#).unwrap();
    assert!(out.ok);
    assert_eq!(out.status, 0);
    assert!(out.stdout.is_empty());
}

#[test]
fn cached_exec_result_decodes_the_stale_fields() {
    let json = r#"{"ok":true,"status":0,"stdout":"x","stale":true,"age_secs":42}"#;
    let out: CachedExecResult = serde_json::from_str(json).unwrap();
    assert!(out.stale);
    assert_eq!(out.age_secs, 42);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rustline-plugin-sdk 2>&1 | tail -10`
Expected: `cannot find function exec`.

- [ ] **Step 3: Implement the wrappers**

In the `#[cfg(target_arch = "wasm32")] mod raw`'s `extern "ExtismHost"` block, add:

```rust
        fn rl_exec(program: String, args_json: String) -> String;
        fn rl_exec_cached(program: String, args_json: String, ttl_secs: String, now: String) -> String;
```

and the raw wrappers:

```rust
    pub fn exec(program: &str, args_json: &str) -> Result<String, HostError> {
        call(unsafe { rl_exec(program.to_string(), args_json.to_string()) })
    }

    pub fn exec_cached(
        program: &str,
        args_json: &str,
        ttl_secs: &str,
        now: &str,
    ) -> Result<String, HostError> {
        call(unsafe {
            rl_exec_cached(
                program.to_string(),
                args_json.to_string(),
                ttl_secs.to_string(),
                now.to_string(),
            )
        })
    }
```

with matching `Err(HostError::Unavailable)` stubs in the
`#[cfg(not(target_arch = "wasm32"))] mod raw`.

Then the typed public wrappers, next to `http_get`/`http_get_cached` (follow
their exact decode shape):

```rust
/// Run a command through the host, if the plugin's `allowed_commands` permits
/// it. The host spawns the program directly — there is no shell — so `args`
/// are passed through verbatim and never re-parsed.
///
/// `Ok(result)` with `result.ok == false` means the host refused or the spawn
/// failed; check `result.error`. A non-zero exit is `ok == true` with a
/// non-zero `status`.
pub fn exec(program: &str, args: &[&str]) -> Result<ExecResult, HostError> {
    let args_json = serde_json::to_string(args).map_err(|e| HostError::Decode(e.to_string()))?;
    let raw = raw::exec(program, &args_json)?;
    serde_json::from_str(&raw).map_err(|e| HostError::Decode(e.to_string()))
}

/// [`exec`] with a host-managed TTL cache keyed on the whole command line, so
/// a slow command isn't re-run on every status-line refresh. `now` is an
/// RFC3339 instant (the host uses it for freshness). Only a zero-exit run is
/// cached; a failed refresh serves the last good result with `stale = true`.
pub fn exec_cached(
    program: &str,
    args: &[&str],
    ttl_secs: u64,
    now: &str,
) -> Result<CachedExecResult, HostError> {
    let args_json = serde_json::to_string(args).map_err(|e| HostError::Decode(e.to_string()))?;
    let raw = raw::exec_cached(program, &args_json, &ttl_secs.to_string(), now)?;
    serde_json::from_str(&raw).map_err(|e| HostError::Decode(e.to_string()))
}
```

Add `ExecResult, CachedExecResult` to the crate's `pub use rustline_abi::{...}` re-export list.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rustline-plugin-sdk 2>&1 | tail -10`
Expected: all pass.

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add -A
git commit -m "feat(sdk): typed exec/exec_cached guest wrappers"
```

---

### Task 12: `plugins/cmdrun` worked example

**Files:**
- Create: `plugins/cmdrun/Cargo.toml`, `plugins/cmdrun/src/lib.rs`, `plugins/cmdrun/cmdrun.toml` (the sidecar manifest)
- Modify: `Cargo.toml` (add `plugins/cmdrun` to `exclude`)
- Test: `plugins/cmdrun/src/lib.rs`'s own `#[cfg(test)] mod tests` (host-target, pure logic)

**Interfaces:**
- Consumes: Task 11's SDK `exec`/`exec_cached`, `active_format`, `export_plugin!`; `rustline_abi::{GuestRender, Segment, Style}`.
- Produces: a `cmdrun.wasm` installable via `just build-plugin cmdrun`.

- [ ] **Step 1: Create the crate manifest**

`plugins/cmdrun/Cargo.toml`:

```toml
# An empty [workspace] table is REQUIRED: without it, building this excluded
# member from inside a git worktree nested under the repo makes cargo walk up
# and try to join the parent workspace, which fails.
[workspace]

[package]
name = "cmdrun"
version = "0.1.0"
edition = "2024"
license = "MIT"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
rustline-plugin-sdk = { path = "../../crates/rustline-plugin-sdk" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

Copy the exact dependency set and structure from `plugins/httpget/Cargo.toml` and adjust the name — read it first rather than assuming.

Add `"plugins/cmdrun",` to the workspace `exclude` list in the root `Cargo.toml`.

- [ ] **Step 2: Write the failing tests**

`plugins/cmdrun/src/lib.rs` (test module first):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_takes_the_first_line_and_trims_it() {
        assert_eq!(extract_snippet("hello\nworld\n"), "hello");
        assert_eq!(extract_snippet("  spaced  \n"), "spaced");
        assert_eq!(extract_snippet(""), "");
        assert_eq!(extract_snippet("\n\n"), "");
    }

    #[test]
    fn snippet_is_capped_so_one_long_line_cannot_swamp_the_bar() {
        let long = "x".repeat(500);
        let out = extract_snippet(&long);
        assert!(out.chars().count() <= MAX_SNIPPET_CHARS, "capped: {}", out.chars().count());
    }

    #[test]
    fn snippet_caps_on_characters_not_bytes() {
        let long = "é".repeat(500);
        let out = extract_snippet(&long);
        assert!(out.chars().count() <= MAX_SNIPPET_CHARS);
    }

    #[test]
    fn render_format_substitutes_out_and_status() {
        assert_eq!(render_format("{out}", "hi", 0), "hi");
        assert_eq!(render_format("[{status}] {out}", "hi", 3), "[3] hi");
        assert_eq!(render_format("no placeholders", "hi", 0), "no placeholders");
    }

    #[test]
    fn render_format_leaves_unknown_placeholders_alone() {
        assert_eq!(render_format("{nope} {out}", "hi", 0), "{nope} hi");
    }

    #[test]
    fn options_parse_with_sensible_defaults() {
        let o: Options = serde_json::from_str("{}").unwrap();
        assert!(o.program.is_empty());
        assert!(o.args.is_empty());
        assert_eq!(o.ttl_secs, 0);
        assert_eq!(o.format, "{out}");
        assert!(o.down_format.is_empty());
    }

    #[test]
    fn options_parse_a_full_table() {
        let o: Options = serde_json::from_str(
            r#"{"program":"git","args":["status","-s"],"ttl_secs":30,"format":"g {out}","down_format":"g ?"}"#,
        )
        .unwrap();
        assert_eq!(o.program, "git");
        assert_eq!(o.args, ["status", "-s"]);
        assert_eq!(o.ttl_secs, 30);
        assert_eq!(o.format, "g {out}");
        assert_eq!(o.down_format, "g ?");
    }

    #[test]
    fn a_zero_ttl_selects_the_plain_uncached_host_fn() {
        assert_eq!(select_mode(0), Mode::Plain);
        assert_eq!(select_mode(1), Mode::Cached);
        assert_eq!(select_mode(3600), Mode::Cached);
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cd plugins/cmdrun && cargo test 2>&1 | tail -10`
Expected: compile errors for the missing items.

- [ ] **Step 4: Implement the plugin**

Prepend to `plugins/cmdrun/src/lib.rs`:

```rust
//! `cmdrun` — the worked example for the host's exec capability.
//!
//! Runs a configured `program` + `args` through `rl_exec` (or `rl_exec_cached`
//! when `ttl_secs > 0`) and renders a snippet of its stdout. This is the exec
//! counterpart to `httpget`'s plain-HTTP example: the same "configured input,
//! rendered snippet, `down_format` on any failure" shape, exercising the one
//! capability the other four examples don't touch.
//!
//! Every failure path — denied by `allowed_commands`, spawn failure, timeout,
//! or a non-zero exit — is logged via `rl_log` with the reason and falls back
//! to `down_format` (empty by default, i.e. render nothing), the same
//! convention as the built-in widgets.

use rustline_plugin_sdk::{
    Color, GuestRender, LogLevel, Segment, Style, active_format, exec, exec_cached, export_plugin,
    log,
};
use serde::Deserialize;

/// Cap on the rendered snippet, in characters. A command that prints a very
/// long first line must not be able to swamp the status line.
pub const MAX_SNIPPET_CHARS: usize = 60;

/// This plugin's `[plugins.cmdrun.options]` table.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Options {
    /// The program to run. No shell is involved — this is argv[0].
    pub program: String,
    /// Its arguments, passed through verbatim.
    pub args: Vec<String>,
    /// Cache TTL in seconds. `0` (the default) uses the plain, uncached host
    /// function — the deliberate contrast this example demonstrates.
    pub ttl_secs: u64,
    /// Render format. `{out}` is the stdout snippet; `{status}` the exit code.
    #[serde(default = "default_format")]
    pub format: String,
    /// Shown when the command couldn't be run. Empty renders nothing.
    pub down_format: String,
    /// Click-toggle alternate view.
    pub alt_format: String,
}

fn default_format() -> String {
    "{out}".to_string()
}

/// Which host function a given TTL selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `rl_exec` — runs every render.
    Plain,
    /// `rl_exec_cached` — the host caches for `ttl_secs`.
    Cached,
}

/// `ttl_secs == 0` means "no caching"; anything else caches.
pub fn select_mode(ttl_secs: u64) -> Mode {
    if ttl_secs == 0 { Mode::Plain } else { Mode::Cached }
}

/// The command's first line, trimmed and capped at [`MAX_SNIPPET_CHARS`]
/// characters (not bytes — truncating mid-codepoint would panic).
pub fn extract_snippet(stdout: &str) -> String {
    let first = stdout.lines().next().unwrap_or("").trim();
    first.chars().take(MAX_SNIPPET_CHARS).collect()
}

/// Substitute `{out}` and `{status}`. Unknown placeholders pass through
/// untouched, the same convention as the built-in widgets' formats.
pub fn render_format(format: &str, out: &str, status: i32) -> String {
    format
        .replace("{out}", out)
        .replace("{status}", &status.to_string())
}
```

Then the guest half — `Deserialize` the `GuestRender` input, read `Options`
from `config`, pick the mode, call `exec`/`exec_cached`, and build the
segments. Model it directly on `plugins/httpget/src/lib.rs`'s guest module:
read that file and follow its structure for `active_format` use, `rl_log` on
failure, `down_format` fallback, and `export_plugin!`. The failure branches to
cover, each logged with its reason: a `HostError`, `result.ok == false` (the
denial/spawn/timeout case — log `result.error`), and a non-zero `status`.

- [ ] **Step 5: Write the sidecar manifest**

`plugins/cmdrun/cmdrun.toml`:

```toml
# Capability manifest for the `cmdrun` example plugin.
#
# A manifest GRANTS NOTHING on its own — it is a declaration `rustline plugin
# approve cmdrun` turns into exactly these `allowed_commands` entries, after
# you confirm. Patterns match the whole canonical argv, so this one permits
# `uname` with any arguments and nothing else.
name = "cmdrun"
version = "0.1.0"
requested_commands = ["uname*"]
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cd plugins/cmdrun && cargo test 2>&1 | tail -10`
Expected: all 8 tests pass.

- [ ] **Step 7: Build it for wasm and run it through the real host**

```bash
just build-plugin cmdrun
cargo build 2>&1 | tail -3
TMP=$(mktemp -d)
printf '[plugins.cmdrun]\nallowed_commands = ["uname*"]\n\n[plugins.cmdrun.options]\nprogram = "uname"\nargs = ["-s"]\n' > "$TMP/config.toml"
./target/debug/rustline --config "$TMP/config.toml" plugin run cmdrun
# Now with no grant: expect a denial to be recorded and down_format rendered.
printf '[plugins.cmdrun]\n\n[plugins.cmdrun.options]\nprogram = "uname"\nargs = ["-s"]\n' > "$TMP/config.toml"
./target/debug/rustline --config "$TMP/config.toml" plugin run cmdrun
```

Expected: the granted run prints a segment containing the OS name; the ungranted run prints no segment text and reports a `command` denial for `uname -s`.

- [ ] **Step 8: Verify the hermetic suite still passes without the wasm toolchain**

Run: `cargo test --workspace 2>&1 | tail -5`
Expected: passes. `plugins/cmdrun` is excluded from the workspace, so `just test` never needs the wasm target.

- [ ] **Step 9: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cd plugins/cmdrun && cargo fmt && cargo clippy --all-targets -- -D warnings && cd ../..
git add -A
git commit -m "feat(plugins): cmdrun worked example for the exec capability"
```

---

### Task 13: Documentation, roadmap, and final verification

**Files:**
- Modify: `CLAUDE.md`, `README.md`, `TODO.md`, `WHATS-NEXT.md`
- Modify: `crates/rustline-wasm/src/lib.rs` (invariant N1's doc comment, if it states the host-function count)

**Interfaces:** none — documentation only.

- [ ] **Step 1: Update `CLAUDE.md`**

Make every one of these edits:

1. **Architecture / `rustline-wasm`:** "seven host functions — six capability-gated … plus one capability-free guest logger" becomes "**nine** host functions — **eight** capability-gated (TTL-cached + raw network + state + arbitrary-file read/write + **command exec, cached and uncached**) plus one capability-free guest logger, `rl_log`".
2. **`plugins/` list:** add `plugins/cmdrun` — "runs a configured `program` + `args` via the host's `rl_exec`/`rl_exec_cached` and renders a snippet of stdout, demonstrating the capability-gated exec capability; a denial, timeout, or non-zero exit logs why via `rl_log` and falls back to `down_format`."
3. **Module map, `rustline-core`:** `config.rs` gains `Region`, `LayoutEditError`, `LayoutChange`, `Layout::{get,get_mut,find}`, the four `layout_*` ops, `WidgetPlacement`, and `widget_placements`; `widget.rs`'s `WidgetSource` gains the `Instance { kind }` variant.
4. **Module map, `rustline-wasm`:** new `argv.rs` (`canonical_argv`) and `run.rs` (`Runner`/`ProcessRunner`/`EXEC_TIMEOUT`/`MAX_OUTPUT_BYTES`); `cache.rs`'s `cache_path` is now namespaced; `perform.rs` gains `perform_exec`/`perform_exec_cached`; `capability.rs` gains `allowed_commands` and `DenialKind::Command`; `manifest.rs` gains `requested_commands`; `lib.rs` gains `discover_plugin_names`.
5. **Module map, `rustline` (bin):** new `widget_cmd.rs` and `widget_tui.rs`, described the way the neighbouring entries are.
6. **Module map, `rustline-abi`:** `ExecResult`/`CachedExecResult`.
7. **CLI section:** the whole `rustline widget list|enable|disable|move|edit` group, and `rustline plugin cmd list|add|remove`; note `plugin list --json` now carries `allowed_commands`.
8. **tmux integration model:** the `prefix + W` `display-popup` binding and its tmux ≥ 3.2 requirement; note `doctor`'s new advisory row.
9. **Config section:** `allowed_commands` in the `[plugins.<name>]` example, the `requested_commands` manifest field, and a new "**exec capability**" paragraph covering: no shell ever, the whole-argv gate, the 5 s timeout / 64 KiB caps / null stdin, and that **environment and cwd are inherited**.
10. **Invariants:** N1's wording ("Adding a new host capability means adding its gate *and* a denied-case test") gains exec as a worked instance; note that the exec gate is on the canonical argv.
11. **Development section:** `ratatui` in the dependency list with the "not feature-gated, because `init` emits the binding unconditionally" rationale.
12. **Roadmap:** a "Done" entry for both parts, linking this spec and plan.
13. **Design docs list:** add the spec and plan paths.

- [ ] **Step 2: Update `README.md`**

Mirror the user-facing subset: the `widget` command group with an example, the `prefix + W` popup and its tmux ≥ 3.2 note, `cmdrun` in the example-plugins list, `allowed_commands`/`requested_commands` in the plugin-config and manifest sections, and a **plugin security** paragraph stating plainly that an approved command runs with the user's environment and permissions, that patterns match the whole command line, and that no shell is involved unless one is explicitly granted.

- [ ] **Step 3: Strip the delivered items from the parking lists**

Remove the whole "## Widget-management TUI / modal" section from `TODO.md` (it is delivered). Leave the other TODO items untouched.

In `WHATS-NEXT.md`: strip the W44 entry per that file's own strip-on-ship rule, and add a dated "Shipped" line to the header block in the same style as the existing ones, naming the branch and what shipped.

- [ ] **Step 4: Full verification**

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
cargo tree -i openssl; cargo tree -i native-tls
just test
```

Expected: fmt clean, clippy clean, all tests pass, both `cargo tree` calls report "did not match any packages", `just test` green without a wasm toolchain.

- [ ] **Step 5: Verify the docs match reality**

Grep for stale counts that the change invalidates:

```bash
grep -rn "seven host functions\|six capability-gated" CLAUDE.md README.md crates/
grep -rn "four example plugins\|four worked example" CLAUDE.md README.md
```

Expected: no hits. Fix any that remain.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "docs: widget manager + exec capability across CLAUDE.md, README.md, TODO.md, WHATS-NEXT.md"
```

---

## Self-review notes

- **Spec coverage:** A1→Task 1; A2→Task 3; A3→Tasks 2+3; A4→Tasks 4+5; A5→Task 6; A6→Task 5. B1→Task 7; B2→Task 7; B3→Task 8; B4→Task 9; B5→Task 10; B6→Task 11; B7→Task 12. Invariants/testing/documentation→Task 13 plus each task's own tests.
- **Naming consistency:** `canonical_argv`, `allowed_commands`, `requested_commands`, `DenialKind::Command`, `EXEC_NAMESPACE`, `MAX_OUTPUT_BYTES`, `EXEC_TIMEOUT`, `EditorState::on_key`, `read_layout`/`write_layout`, `widget_placements`, `discover_plugin_names` are used identically in every task that references them.
- **Known signature risk:** `render_named_region`'s exact parameter list is not reproduced here; Task 5 Step 4 explicitly instructs the implementer to read `assemble.rs` and match `main.rs`'s call. Same for `build_plugin_with_cache`'s `.with_function` chain in Task 9 and the `perform.rs` test helpers in Task 8.
