# rustline whats-next bundle #4 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship three forward-looking features on one branch — init-wizard seeding + confirm (W33), multiple widget instances per layout (W46), and an optional persistent daemon front-end (W48).

**Architecture:** W33 is contained to `init.rs`. W46 adds an `[instances.<name>]` config table whose entries register into the existing name-keyed `Registry` (each clickable widget gains a `name` field so its range/toggle identity is the instance name), plus two additive `Context` maps for the parameterized `disk`/`throughput` reads. W48 keeps a warm `DaemonState` (config, theme, registry incl. instantiated WASM plugins) alive behind a Unix socket; render clients try the socket and fall back to the existing in-process path on any failure.

**Tech Stack:** Rust edition 2024, `serde`/`toml`/`serde_json`, `std::os::unix::net::UnixListener` (no tokio), `chrono`, `extism` (existing). rustls-only policy unchanged; no new TLS.

## Global Constraints

- Edition 2024 in every crate; keep all crate editions equal to `rustfmt.toml`.
- Must stay clippy-clean (`cargo clippy --all-targets -- -D warnings`) and rustfmt-clean (`cargo fmt --all --check`). No pre-commit hook — run `cargo fmt --all` before each commit.
- `just test` stays hermetic (no wasm toolchain). Daemon/instance tests use builtins only + tempdir sockets/state.
- Invariants (re-verify per task): #1 Context is the sole render input; #2 wire types additive, no `deny_unknown_fields`, new `Context.disks`/`throughputs` are NOT mirrored into `WireContext`; #3 `Config::load` total (bad `[instances]` → warn+skip); #4 init output injection-safe; #7 the click-toggle NAME is one identity end-to-end (instance name, not kind).
- `RANGE_NAME_MAX_BYTES` (= 15) from `rustline_core` is the click-toggle name length cap; reuse it, don't hardcode 15.
- Commit `Cargo.lock` with any dependency change (none expected).
- Consult `~/.claude/rust-crate-decisions.md` before adding any dependency.

---

## W33 — init wizard: seed from config + confirm

### Task 1: Seed wizard answers from an existing config

**Files:**
- Modify: `crates/rustline/src/init.rs`
- Test: `crates/rustline/src/init.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `fn clock_from_format(fmt: &str) -> Option<ClockStyle>`; `fn seed_answers(config_path: &Path) -> InitAnswers`.
- Consumes: existing `ClockStyle::formats()`, `defaults()`, `InitAnswers`, `rustline_core::Config`, `crate::battery::read_battery`.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn clock_from_format_reverses_presets_else_none() {
    assert_eq!(clock_from_format("%a %Y-%m-%d %H:%M"), Some(ClockStyle::TwentyFour));
    assert_eq!(clock_from_format("%a %Y-%m-%d %H:%M:%S"), Some(ClockStyle::TwentyFourSeconds));
    assert_eq!(clock_from_format("%a %Y-%m-%d %I:%M %p"), Some(ClockStyle::Twelve));
    assert_eq!(clock_from_format("%a %Y-%m-%d %I:%M:%S %p"), Some(ClockStyle::TwelveSeconds));
    // zero-config default and garbage do not match a preset:
    assert_eq!(clock_from_format("%a < %Y-%m-%d < %H:%M"), None);
    assert_eq!(clock_from_format("nonsense"), None);
}

#[test]
fn seed_answers_from_existing_config_recovers_config_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    std::fs::write(&path, concat!(
        "[theme]\nbase = \"nord\"\n",
        "[layout]\nleft = [\"pane_id\", \"hostname\", \"lan_ip\"]\n",
        "right = [\"cwd\", \"cpu\", \"memory\", \"battery\", \"loadavg\", \"datetime\"]\n",
        "[widgets.datetime]\nformat = \"%a %Y-%m-%d %I:%M %p\"\n",
    )).unwrap();
    let a = seed_answers(&path);
    assert_eq!(a.theme, "nord");
    assert!(a.lan_ip);
    assert!(a.battery);
    assert!(!a.tailscale);
    assert_eq!(a.clock, ClockStyle::Twelve);
    // tmux-only answers keep recommended defaults (config-only seed):
    assert_eq!(a.mouse, defaults().mouse);
    assert_eq!(a.interval, defaults().interval);
    assert_eq!(a.two_line, defaults().two_line);
}

#[test]
fn seed_answers_missing_file_is_defaults_shaped() {
    let a = seed_answers(std::path::Path::new("/no/such/config.toml"));
    assert_eq!(a.theme, defaults().theme);
    assert!(!a.lan_ip && !a.tailscale);
    // battery may be true or false depending on the host probe; assert it does not panic
    let _ = a.battery;
}
```

- [ ] **Step 2: Run to confirm failure** — `cargo test -p rustline init::tests::clock_from_format`. Expected: unresolved names.

- [ ] **Step 3: Implement**

```rust
/// Reverse-map a datetime `format` string to the wizard preset that produced
/// it, or `None` if it matches no preset (so seeding keeps the default clock).
fn clock_from_format(fmt: &str) -> Option<ClockStyle> {
    [
        ClockStyle::TwentyFour,
        ClockStyle::TwentyFourSeconds,
        ClockStyle::Twelve,
        ClockStyle::TwelveSeconds,
    ]
    .into_iter()
    .find(|c| c.formats().0 == fmt)
}

/// Pre-fill wizard answers from an existing config.toml. Recovers only what
/// config stores (theme, optional-widget membership, clock); the tmux-only
/// answers (mouse/two_line/interval) keep their recommended defaults.
fn seed_answers(config_path: &Path) -> InitAnswers {
    use rustline_core::Config;
    if !config_path.exists() {
        let mut a = defaults();
        a.battery = crate::battery::read_battery().is_some();
        return a;
    }
    let cfg = Config::load(config_path);
    let in_layout = |name: &str| {
        cfg.layout.left.iter().chain(&cfg.layout.center).chain(&cfg.layout.right)
            .any(|w| w == name)
    };
    InitAnswers {
        theme: cfg.theme.base.clone().unwrap_or_else(|| defaults().theme),
        two_line: defaults().two_line,
        mouse: defaults().mouse,
        battery: in_layout("battery"),
        tailscale: in_layout("tailscale_ip"),
        lan_ip: in_layout("lan_ip"),
        clock: clock_from_format(&cfg.widgets.datetime.format).unwrap_or_else(|| defaults().clock),
        interval: defaults().interval,
    }
}
```

- [ ] **Step 4: Run tests** — `cargo test -p rustline init`. Expected: PASS.
- [ ] **Step 5: fmt + clippy, commit** — `cargo fmt --all && cargo clippy -p rustline --all-targets -- -D warnings`. Commit `feat(init): seed wizard answers from existing config (W33)`.

### Task 2: Wire seeding into the prompt + add confirm-before-write

**Files:**
- Modify: `crates/rustline/src/init.rs`
- Test: `crates/rustline/src/init.rs`

**Interfaces:**
- Consumes: `seed_answers` (Task 1), existing `prompt_answers`, `ask`, `preview`, `dry_run_config`, `dry_run_tmux_block`, `line_diff`, `apply`, `run`.
- Produces: `fn summarize_answers(a: &InitAnswers) -> String`; `prompt_answers(themes_dir: &Path, seed: &InitAnswers) -> InitAnswers`.

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn summarize_answers_lists_every_answer() {
    let a = InitAnswers {
        theme: "nord".into(), two_line: true, mouse: false, battery: true,
        tailscale: false, lan_ip: true, clock: ClockStyle::Twelve, interval: 5,
    };
    let s = summarize_answers(&a);
    for needle in ["nord", "two-line", "mouse", "battery", "LAN", "12-hour", "5s"] {
        assert!(s.contains(needle), "missing {needle:?} in:\n{s}");
    }
}
```

- [ ] **Step 2: Run to confirm failure** — `cargo test -p rustline init::tests::summarize_answers`. Expected: unresolved name.

- [ ] **Step 3: Implement `summarize_answers`**

```rust
/// A human summary of the collected answers for the pre-write confirmation.
fn summarize_answers(a: &InitAnswers) -> String {
    use std::fmt::Write as _;
    let clock = match a.clock {
        ClockStyle::TwentyFour => "24-hour",
        ClockStyle::TwentyFourSeconds => "24-hour + seconds",
        ClockStyle::Twelve => "12-hour",
        ClockStyle::TwelveSeconds => "12-hour + seconds",
    };
    let on = |b: bool| if b { "on" } else { "off" };
    let mut s = String::new();
    let _ = writeln!(s, "  theme:        {}", a.theme);
    let _ = writeln!(s, "  status lines: {}", if a.two_line { "two-line" } else { "one-line" });
    let _ = writeln!(s, "  mouse:        {}", on(a.mouse));
    let _ = writeln!(s, "  battery:      {}", on(a.battery));
    let _ = writeln!(s, "  Tailscale IP: {}", on(a.tailscale));
    let _ = writeln!(s, "  LAN IP:       {}", on(a.lan_ip));
    let _ = writeln!(s, "  clock:        {clock}");
    let _ = writeln!(s, "  refresh:      {}s", a.interval);
    s
}
```

- [ ] **Step 4: Thread the seed + confirm into the I/O shell (no unit test — compile + manual).**

Change `prompt_answers(themes_dir: &Path)` to `prompt_answers(themes_dir: &Path, seed: &InitAnswers)`. Inside, replace `let mut a = defaults();` with `let mut a = seed.clone();`. Use the seed as each shown default:
- Theme menu default index = `themes.iter().position(|t| *t == seed.theme).unwrap_or(0)` (pass that to `parse_menu_choice` as the default and pre-select in the prompt text).
- Battery/tailscale/lan/two_line/mouse: `ask("…", seed.X)` (drop the inline `read_battery()` call — the seed carries it).
- Clock default index = `clocks.iter().position(|(_, c)| *c == seed.clock).unwrap_or(0)`.
- Interval: `a.interval = if ask("Fast refresh (1s)? (No = 5s)", seed.interval == 1) { 1 } else { 5 };`.

In `run`, the interactive branch becomes `prompt_answers(themes_dir, &seed_answers(config_path))`. Then, still in `run`, for the interactive (non-`--defaults`, non-`--dry-run`) path, gate `apply` behind a confirm:

```rust
// after `let answers = ...` and the `if args.dry_run { preview(...); return; }` guard,
// but ONLY for the interactive path (track whether answers came from the prompt):
if interactive {
    eprintln!("\nAbout to write:\n{}", summarize_answers(&answers));
    preview(&answers, config_path, tmux_conf_path, binary); // reuse dry-run diff output
    if !ask("Write these changes?", true) {
        eprintln!("Aborted; nothing written.");
        return;
    }
}
apply(&answers, config_path, tmux_conf_path, binary);
```

Introduce a local `let interactive = !args.defaults;` computed where `answers` is chosen (the `--defaults` branch sets it false; the interactive branch true). `--defaults` therefore writes without a confirm, as today.

- [ ] **Step 5: Run tests + build** — `cargo test -p rustline init` and `cargo build -p rustline`. Expected: PASS/OK. Update any existing `prompt_answers` call sites.
- [ ] **Step 6: fmt + clippy, commit** — `feat(init): confirm-before-write + seeded prompt defaults (W33)`.

---

## W46 — multiple widget instances

### Task 3: `[instances]` config table

**Files:**
- Modify: `crates/rustline-core/src/config.rs`
- Test: `crates/rustline-core/src/config.rs`

**Interfaces:**
- Produces: `Config.instances: HashMap<String, toml::Value>`; `Config::instance_kind(v: &toml::Value) -> Option<&str>` (associated fn).

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn parses_instances_table_and_kind() {
    let toml = r#"
[layout]
right = ["clock_utc", "disk_data"]
[instances.clock_utc]
kind = "datetime"
timezone = "UTC"
format = "%H:%MZ"
[instances.disk_data]
kind = "disk"
mount = "/data"
"#;
    let c: Config = toml::from_str(toml).unwrap();
    assert_eq!(c.instances.len(), 2);
    let utc = &c.instances["clock_utc"];
    assert_eq!(Config::instance_kind(utc), Some("datetime"));
    assert_eq!(utc.get("timezone").and_then(|v| v.as_str()), Some("UTC"));
}

#[test]
fn absent_instances_is_empty_and_roundtrips() {
    let c: Config = toml::from_str("").unwrap();
    assert!(c.instances.is_empty());
    let back: Config = toml::from_str(&toml::to_string(&c).unwrap()).unwrap();
    assert!(back.instances.is_empty());
}
```

- [ ] **Step 2: Run to confirm failure.**
- [ ] **Step 3: Implement** — add to `Config`:

```rust
/// Extra named widget instances (W46), keyed by instance name. Each value is
/// the raw `[instances.<name>]` table; `kind` selects the widget type and the
/// remaining keys are that kind's options (re-parsed per kind at registration).
#[serde(default)]
pub instances: HashMap<String, toml::Value>,
```

and an associated fn:

```rust
impl Config {
    /// The `kind` of a `[instances.<name>]` table, if present and a string.
    pub fn instance_kind(v: &toml::Value) -> Option<&str> {
        v.get("kind").and_then(Value::as_str)
    }
}
```

- [ ] **Step 4: Run tests.** `cargo test -p rustline-core config`. PASS.
- [ ] **Step 5: fmt + clippy, commit** — `feat(config): [instances] table for multi-instance widgets (W46)`.

### Task 4: Thread instance `name` into clickable widgets + `build_<kind>` helpers

**Files:**
- Modify: the 12 clickable widget modules under `crates/rustline-core/src/widgets/` (`datetime`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`, `memory`, `loadavg`, `git`, `disk`, `uptime`, `media`, `throughput`)
- Modify: `crates/rustline-core/src/widgets/mod.rs` (extract `build_<kind>` helpers; base registration passes the kind name)
- Test: each modified widget module + `widgets/mod.rs`

**Interfaces:**
- Produces: each of the 12 structs gains `pub name: String`; `range_name`/`render` use `self.name` via `clickable_range(&self.name, …)` / `active_format(ctx, &self.name, …)`. New `pub(crate) fn build_<kind>(name: &str, o: &<Kind>Opts) -> Box<dyn Widget>` in `mod.rs` for each kind (reused by base + instance registration).

- [ ] **Step 1: Write failing test** (in `widgets/mod.rs` tests) proving a renamed instance's range/toggle identity is its own name:

```rust
#[test]
fn datetime_instance_uses_its_own_name_for_range_and_toggle() {
    // Build a datetime widget under a non-kind name and confirm its range name
    // and toggle key are that name, not "datetime".
    let w = super::build_datetime("clock_utc", &crate::config::DateTimeOpts {
        alt_format: "%H:%M".into(),
        ..Default::default()
    });
    assert_eq!(w.range_name(), Some("clock_utc"));
}
```

- [ ] **Step 2: Run to confirm failure** — unresolved `build_datetime`.

- [ ] **Step 3: Implement.** For each of the 12 structs, add `pub name: String` and replace the hardcoded kind literal in `range_name`/`render`. Example for `datetime.rs`:

```rust
pub struct DateTime {
    pub name: String,
    pub format: String,
    pub alt_format: String,
    pub timezone: Option<String>,
}
impl Widget for DateTime {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let fmt = active_format(ctx, &self.name, &self.format, &self.alt_format);
        /* …unchanged rendering with `fmt`… */
    }
    fn range_name(&self) -> Option<&str> { clickable_range(&self.name, &self.alt_format) }
}
```

In `mod.rs`, extract each base registration body into a helper and call it with the kind name. Example:

```rust
pub(crate) fn build_datetime(name: &str, o: &DateTimeOpts) -> Box<dyn Widget> {
    Box::new(DateTime {
        name: name.to_string(),
        format: o.format.clone(),
        alt_format: o.alt_format.clone(),
        timezone: o.timezone.clone(),
    })
}
// base registration:
let datetime = cfg.widgets.datetime.clone();
registry.register_described(
    builtin_descriptor("datetime", "The current time, `chrono` strftime-formatted", true),
    Box::new(move || build_datetime("datetime", &datetime)),
);
```

Repeat for all 12 kinds. Update every direct struct-literal construction in existing tests to add `name: "<kind>".into()`.

- [ ] **Step 4: Run tests** — `cargo test -p rustline-core`. All existing widget tests must still pass (base output byte-identical), plus the new one.
- [ ] **Step 5: fmt + clippy, commit** — `refactor(widgets): carry instance name for range/toggle identity (W46)`.

### Task 5: Register `[instances]` into the registry

**Files:**
- Modify: `crates/rustline-core/src/widgets/mod.rs` (`with_builtins` second pass)
- Test: `crates/rustline-core/src/widgets/mod.rs`

**Interfaces:**
- Consumes: `Config.instances`, `Config::instance_kind`, the `build_<kind>` helpers (Task 4), `RANGE_NAME_MAX_BYTES`.
- Produces: instances registered into the `Registry` under their instance names.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn two_datetime_instances_render_distinct_timezones() {
    let mut cfg = Config::default();
    cfg.instances.insert("clock_utc".into(),
        toml::from_str("kind='datetime'\ntimezone='UTC'\nformat='%H'").unwrap());
    cfg.instances.insert("clock_ny".into(),
        toml::from_str("kind='datetime'\ntimezone='America/New_York'\nformat='%H'").unwrap());
    let reg = Registry::with_builtins(&cfg);
    assert!(reg.contains("clock_utc") && reg.contains("clock_ny"));
    // Both resolve and render (values differ by tz; assert both non-empty).
    let out = reg.resolve(&["clock_utc".into(), "clock_ny".into()]);
    assert_eq!(out.len(), 2);
}

#[test]
fn unknown_kind_and_builtin_collision_are_skipped() {
    let mut cfg = Config::default();
    cfg.instances.insert("bogus".into(), toml::from_str("kind='nope'").unwrap());
    cfg.instances.insert("cpu".into(), toml::from_str("kind='datetime'").unwrap()); // collides
    let reg = Registry::with_builtins(&cfg);
    assert!(!reg.contains("bogus"));
    // "cpu" stays the built-in cpu widget, not the datetime instance:
    assert!(reg.contains("cpu"));
}

#[test]
fn instance_range_name_and_toggle_use_instance_name() {
    let mut cfg = Config::default();
    cfg.instances.insert("clk".into(),
        toml::from_str("kind='datetime'\nalt_format='%H:%M'").unwrap());
    let reg = Registry::with_builtins(&cfg);
    let w = reg.build("clk").unwrap();
    assert_eq!(w.range_name(), Some("clk"));
}
```

- [ ] **Step 2: Run to confirm failure.**

- [ ] **Step 3: Implement** the second pass at the end of `with_builtins`, before `registry`:

```rust
for (name, table) in &cfg.instances {
    let Some(kind) = Config::instance_kind(table) else {
        tracing::warn!(instance = %name, "instance missing `kind`, skipping");
        continue;
    };
    if registry.contains(name) {
        tracing::warn!(instance = %name, "instance name collides with an existing widget, skipping");
        continue;
    }
    if name.len() > RANGE_NAME_MAX_BYTES {
        tracing::warn!(instance = %name, "instance name > 15 bytes; not click-toggleable");
    }
    let t = table.clone();
    let factory: Option<Factory> = match kind {
        "datetime" => { let o: DateTimeOpts = t.try_into().unwrap_or_default();
            let n = name.clone(); Some(Box::new(move || build_datetime(&n, &o))) }
        "disk" => { let o: DiskOpts = t.try_into().unwrap_or_default();
            let n = name.clone(); Some(Box::new(move || build_disk(&n, &o))) }
        // …one arm per registerable kind (all 12 clickable kinds + cwd/hostname/pane_id)…
        other => { tracing::warn!(instance = %name, kind = %other, "unknown instance kind, skipping"); None }
    };
    if let Some(factory) = factory {
        registry.register_described(
            builtin_descriptor(name, &format!("{kind} instance"), true),
            factory,
        );
    }
}
```

(Import `Config`, `DateTimeOpts`, etc., and `crate::widget::Factory` — expose `Factory` as `pub(crate)` in `widget.rs` if not already.) Include arms for `cwd`/`hostname`/`pane_id` (via their own `build_*` helpers, added in Task 4 or here) so any configurable kind can be instanced; non-clickable ones simply never emit a range.

- [ ] **Step 4: Run tests** — `cargo test -p rustline-core`. PASS.
- [ ] **Step 5: fmt + clippy, commit** — `feat(widgets): register [instances] into the widget registry (W46)`.

### Task 6: Include instances in `color_overrides` + `click_map`

**Files:**
- Modify: `crates/rustline-core/src/config.rs`
- Test: `crates/rustline-core/src/config.rs`

**Interfaces:**
- Produces: `Config::instance_meta(kind: &str, v: &toml::Value) -> Option<(ColorOverride, String, ClickBindings)>` (color, alt_format, click); `color_overrides()`/`click_map()` extended to include instance names.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn color_overrides_and_click_map_include_instances() {
    let mut c = Config::default();
    c.instances.insert("clk".into(), toml::from_str(
        "kind='datetime'\nalt_format='%H:%M'\nfg={ Indexed = 5 }").unwrap());
    assert!(c.color_overrides().contains_key("clk"));
    let cm = c.click_map();
    assert!(cm.get("clk").map(|w| w.toggleable).unwrap_or(false));
}
```

- [ ] **Step 2: Run to confirm failure.**

- [ ] **Step 3: Implement** `instance_meta` (dispatch on kind → parse the kind's Opts → project its `.color`, `.alt_format`, `.click`; kinds without those fields return defaults/empty), then in `color_overrides()` iterate `self.instances` appending `(name, color)` when `fg`/`bg` set, and in `click_map()` insert a `WidgetClick { toggleable: !alt_format.is_empty(), bindings }` per instance. Keep the existing built-in candidate tables unchanged.

- [ ] **Step 4: Run tests.** PASS.
- [ ] **Step 5: fmt + clippy, commit** — `feat(config): project instances into color/click maps (W46)`.

### Task 7: Per-instance reads — `Context.disks`/`throughputs` + widget lookups

**Files:**
- Modify: `crates/rustline-core/src/context.rs` (new fields + `Context::default`)
- Modify: `crates/rustline-core/src/widgets/disk.rs`, `throughput.rs` (read from the maps; carry the lookup key)
- Modify: `crates/rustline-core/src/widgets/mod.rs` (`build_disk`/`build_throughput` set the key)
- Test: `disk.rs`, `throughput.rs`

**Interfaces:**
- Produces: `Context.disks: BTreeMap<String, DiskInfo>`, `Context.throughputs: BTreeMap<String, Throughput>` (both `#[serde(default)]`, empty in `Context::default`); `DiskWidget` gains `mount` used as `ctx.disks.get(&self.mount)`; `ThroughputWidget` gains `iface_key` used as `ctx.throughputs.get(&self.iface_key)`.
- Note: NOT mirrored into `WireContext` (invariant #2). `Context.disk`/`throughput` stay as the base reading.

- [ ] **Step 1: Write failing tests**

```rust
// disk.rs
#[test]
fn disk_widget_reads_its_mount_from_disks_map() {
    let mut c = Context::default();
    let g = 1024u64.pow(3);
    c.disks.insert("/data".into(), DiskInfo { total_bytes: 10*g, used_bytes: 4*g, available_bytes: 6*g });
    let w = crate::widgets::build_disk("disk_data", &crate::config::DiskOpts {
        mount: "/data".into(), ..Default::default() });
    let texts: Vec<String> = w.render(&c).into_iter().map(|s| s.text).collect();
    assert_eq!(texts, vec![" 4.0G/10G".to_string()]); // default " {used}/{total}"
}
```

- [ ] **Step 2: Run to confirm failure.**
- [ ] **Step 3: Implement.** Add the two maps to `Context` (and `..Default::default()` covers most sites; add empty maps in `Context::default`). Change `DiskWidget` to hold `mount: String` and read `ctx.disks.get(&self.mount)` instead of `ctx.disk`; `ThroughputWidget` to hold `iface_key: String` (the configured interface or `""`) and read `ctx.throughputs.get(&self.iface_key)`. `build_disk`/`build_throughput` set those from the Opts. Spell out the two new fields at any test/fixture construction site that doesn't use `..Default::default()`.
- [ ] **Step 4: Run tests** — `cargo test -p rustline-core`. PASS.
- [ ] **Step 5: fmt + clippy, commit** — `feat(context): per-instance disk/throughput reading maps (W46)`.

### Task 8: Instance-aware context build + per-interface throughput samples + main wiring

**Files:**
- Modify: `crates/rustline-core/src/config.rs` (kind/mount/interface layout helpers)
- Modify: `crates/rustline/src/build_context.rs` (`build_region_context` signature + instance-aware reads)
- Modify: `crates/rustline/src/throughput.rs` (per-interface sample filename)
- Modify: `crates/rustline/src/main.rs` and `crates/rustline/src/bench/*` (call sites)
- Test: `config.rs`, `build_context.rs`, `throughput.rs`

**Interfaces:**
- Produces (core): `Config::layout_kinds(&self, layout: &[String]) -> BTreeSet<String>`; `Config::disk_mounts(&self, layout: &[String]) -> BTreeSet<String>`; `Config::throughput_interfaces(&self, layout: &[String]) -> BTreeSet<Option<String>>`.
- Produces (bin): `build_region_context(args: &RegionArgs, layout: &[String], theme: &Theme, cfg: &Config) -> Context` (drops the `disk_mount`/`throughput_interface`/`SparkOpts` params — derived from `cfg` internally).

- [ ] **Step 1: Write failing tests**

```rust
// config.rs
#[test]
fn layout_kinds_and_params_resolve_instances() {
    let mut c = Config::default();
    c.instances.insert("disk_data".into(), toml::from_str("kind='disk'\nmount='/data'").unwrap());
    let layout = ["disk_data".to_string()];
    assert!(c.layout_kinds(&layout).contains("disk"));
    assert!(c.disk_mounts(&layout).contains("/data"));
    // base name form also resolves:
    let base = ["disk".to_string()];
    assert!(c.disk_mounts(&base).contains("/")); // [widgets.disk].mount default
}
```

```rust
// build_context.rs
#[test]
fn two_disk_instances_populate_disks_map_with_both_mounts() {
    let mut cfg = Config::default();
    cfg.instances.insert("disk_data".into(), toml::from_str("kind='disk'\nmount='/'").unwrap());
    let layout = ["disk".to_string(), "disk_data".to_string()];
    let ctx = build_region_context(&RegionArgs::default(), &layout, &Theme::default(), &cfg);
    assert!(ctx.disks.contains_key("/")); // read fired for the mount via the instance/base
}
```

- [ ] **Step 2: Run to confirm failure.**
- [ ] **Step 3: Implement core helpers** in `config.rs`: `layout_kinds` maps each layout entry to a kind (built-in name → itself; else `instances[name].kind`); `disk_mounts` collects the base `[widgets.disk].mount` when `disk` is in the layout plus each disk-kind instance's `mount` (default `/`); `throughput_interfaces` collects the base interface plus each throughput-kind instance's `interface` (aggregate = `None`).
- [ ] **Step 4: Implement `build_region_context`** to take `cfg: &Config`: gate each read on `cfg.layout_kinds(layout).contains(kind)`; for disk, `read_disk` each mount in `cfg.disk_mounts(layout)` into `ctx.disks` and set `ctx.disk = ctx.disks.get(base_mount)`; for throughput, `read_throughput` each interface in `cfg.throughput_interfaces(layout)` into `ctx.throughputs` and set `ctx.throughput`; derive spark formats from `cfg.widgets.cpu/memory`. Remove `SparkOpts`/`spark_opts` from the public call path (keep internal if convenient). Update `main.rs`'s two render arms and the `bench` fixtures to the new signature.
- [ ] **Step 5: Per-interface throughput sample file** in `crates/rustline/src/throughput.rs`: change the sample filename from `throughput-sample` to `throughput-sample` for `None` and `throughput-sample-<sanitized>` for a named interface (replace non-`[A-Za-z0-9_-]` with `_`). Add a test that two different interfaces persist to different files (each returns `Some` on its own second call).
- [ ] **Step 6: Run tests** — `cargo test -p rustline-core -p rustline`. PASS. Run `cargo build -p rustline --features bench` to catch bench call sites.
- [ ] **Step 7: fmt + clippy, commit** — `feat(build_context): instance-aware reads + per-interface throughput samples (W46)`.

---

## W48 — optional persistent daemon

### Task 9: Daemon wire protocol + framing

**Files:**
- Create: `crates/rustline/src/daemon_proto.rs`
- Modify: `crates/rustline/src/main.rs` (`mod daemon_proto;`)
- Test: `crates/rustline/src/daemon_proto.rs`

**Interfaces:**
- Produces: `enum DaemonRequest { Render { region: RegionKind, args: RenderArgsWire }, Ping, Shutdown }`; `enum RegionKind { Left, Right, Window }`; `struct RenderArgsWire { session, window, pane, pane_path: Option<String>, index, name, flags: Option<String>, current: bool, preview: bool }`; `enum DaemonResponse { Markup(String), Pong, ShuttingDown }`; `fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()>`; `fn read_frame<R: Read, T: DeserializeOwned>(r: &mut R) -> io::Result<T>` (u32-LE length prefix + JSON; a length over a sane cap, e.g. 8 MiB, is an `Err`, not an allocation).

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn frame_roundtrips_each_variant() {
    for req in [DaemonRequest::Ping, DaemonRequest::Shutdown,
                DaemonRequest::Render { region: RegionKind::Right, args: RenderArgsWire::default() }] {
        let mut buf = Vec::new();
        write_frame(&mut buf, &req).unwrap();
        let back: DaemonRequest = read_frame(&mut &buf[..]).unwrap();
        assert_eq!(back, req);
    }
}
#[test]
fn truncated_frame_is_err_not_panic() {
    let bytes = [0u8, 0, 0, 10, b'{']; // claims 10 bytes, has 1
    let r: io::Result<DaemonRequest> = read_frame(&mut &bytes[..]);
    assert!(r.is_err());
}
```

- [ ] **Step 2: Run to confirm failure.**
- [ ] **Step 3: Implement** the types (derive `Serialize, Deserialize, PartialEq, Debug`, and `Default` for `RenderArgsWire`) and framing (`write_all(&(len as u32).to_le_bytes())` + JSON bytes; read 4-byte length, cap-check, `read_exact`, `serde_json::from_slice`).
- [ ] **Step 4: Run tests.** PASS.
- [ ] **Step 5: fmt + clippy, commit** — `feat(daemon): wire protocol + length-prefixed framing (W48)`.

### Task 10: Daemon client with in-process fallback

**Files:**
- Create: `crates/rustline/src/daemon_client.rs`
- Modify: `crates/rustline/src/main.rs` (`mod daemon_client;`)
- Test: `crates/rustline/src/daemon_client.rs`

**Interfaces:**
- Produces: `fn daemon_socket_path() -> PathBuf` (`$XDG_RUNTIME_DIR/rustline/daemon.sock`, fallback `rustline_wasm::state_root().join("daemon.sock")`); `fn try_render(region: RegionKind, args: RenderArgsWire) -> Option<String>`.
- Consumes: `daemon_proto` (Task 9).

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn try_render_returns_none_with_no_socket() {
    // Point at a socket path that does not exist.
    assert!(try_render_at(&PathBuf::from("/no/such/rustline.sock"),
        RegionKind::Right, RenderArgsWire::default()).is_none());
}
#[test]
fn try_render_reads_markup_from_a_fake_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("d.sock");
    let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    let h = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let _req: DaemonRequest = crate::daemon_proto::read_frame(&mut s).unwrap();
        crate::daemon_proto::write_frame(&mut s, &DaemonResponse::Markup("OK".into())).unwrap();
    });
    let out = try_render_at(&sock, RegionKind::Right, RenderArgsWire::default());
    h.join().unwrap();
    assert_eq!(out.as_deref(), Some("OK"));
}
```

- [ ] **Step 2: Run to confirm failure.**
- [ ] **Step 3: Implement.** Split `try_render` = `try_render_at(&daemon_socket_path(), …)`. `try_render_at`: if the path doesn't exist → `None`; else `UnixStream::connect`, set short read/write timeouts, `write_frame(DaemonRequest::Render{…})`, `read_frame`, return `Some(markup)` for `DaemonResponse::Markup`, `None` otherwise / on any error.
- [ ] **Step 4: Run tests.** PASS.
- [ ] **Step 5: fmt + clippy, commit** — `feat(daemon): client with in-process fallback (W48)`.

### Task 11: Daemon server

**Files:**
- Create: `crates/rustline/src/daemon.rs`
- Modify: `crates/rustline/src/main.rs` (`mod daemon;`)
- Test: `crates/rustline/src/daemon.rs`

**Interfaces:**
- Produces: `struct DaemonState { config, theme, plugin_dir, registry, config_mtime }` with `fn build(config_path, plugin_dir) -> DaemonState`, `fn reload_if_changed(&mut self, config_path: &Path) -> bool`, `fn render(&self, region: RegionKind, args: &RenderArgsWire) -> String`; `fn serve(config_path: &Path, plugin_dir: PathBuf) -> io::Result<()>`; `fn status() -> bool`; `fn stop() -> io::Result<()>`.
- Consumes: `daemon_proto`, `build_context::{build_region_context, build_window_context}`, `rustline_core::{render_named_region, render_window, Direction}`, `resolve_theme`, `resolve_plugin_dir`, `Registry::with_builtins`, `rustline_wasm::register_plugins`.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn reload_if_changed_only_on_mtime_advance() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    std::fs::write(&cfg, "").unwrap();
    let mut st = DaemonState::build(&cfg, dir.path().join("plugins"));
    assert!(!st.reload_if_changed(&cfg)); // unchanged
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(&cfg, "[layout]\nright=[\"datetime\"]\n").unwrap();
    filetime_bump(&cfg); // ensure mtime advances on fast filesystems
    assert!(st.reload_if_changed(&cfg)); // reloaded
}
#[test]
fn daemon_render_matches_in_process() {
    // Build a builtins-only DaemonState and a matching in-process render over
    // the same RenderArgsWire; assert identical markup for RegionKind::Right.
    // (Fixture: empty config → default layout; no wasm.)
}
```

- [ ] **Step 2: Run to confirm failure.**
- [ ] **Step 3: Implement.** `DaemonState::build` parses config, resolves theme + plugin_dir, builds `Registry::with_builtins` then `register_plugins` over the union `cfg.layout.left ∪ cfg.layout.right`, records the config mtime. `reload_if_changed` re-stats the mtime and rebuilds on change (returns whether it rebuilt). `render` maps `RegionKind` → the region layout + `Direction`, calls `build_region_context`/`build_window_context` + `render_named_region`/`render_window`, returns markup. `serve` binds `UnixListener` (removing a stale socket file first; `create_dir_all` the parent), prints a keep-alive hint, accept-loops with a `Arc<Mutex<DaemonState>>`; each connection thread: `read_frame` → on `Render` lock+`reload_if_changed`+`render`+reply `Markup`; `Ping`→`Pong`; `Shutdown`→reply `ShuttingDown`, remove the socket, `std::process::exit(0)`. `status` connects + `Ping`. `stop` connects + `Shutdown`. For the mtime test helper, either set the file's mtime forward explicitly or sleep; do not add a `filetime` dependency — use `std::fs` + a longer sleep, or read+rewrite and compare stored mtime.
- [ ] **Step 4: Run tests** — `cargo test -p rustline daemon`. PASS.
- [ ] **Step 5: fmt + clippy, commit** — `feat(daemon): warm-state server with config-mtime reload (W48)`.

### Task 12: CLI wiring + render fallback + doctor note

**Files:**
- Modify: `crates/rustline/src/cli.rs` (`Command::Daemon(DaemonCmd)`)
- Modify: `crates/rustline/src/main.rs` (dispatch + the three render arms try the daemon first)
- Modify: `crates/rustline/src/doctor.rs` (advisory socket check)
- Test: `crates/rustline/tests/smoke.rs`

**Interfaces:**
- Consumes: `daemon::{serve, status, stop}`, `daemon_client::try_render`, `daemon_proto::{RegionKind, RenderArgsWire}`.

- [ ] **Step 1: Write failing test** (smoke): `rustline daemon status` with no daemon running exits non-zero and prints "not running" (spawn the built binary via `assert_cmd` or the existing smoke harness).
- [ ] **Step 2: Run to confirm failure.**
- [ ] **Step 3: Implement.** Add `Command::Daemon(DaemonCmd)` (`DaemonCmd { Run(DaemonRunArgs { plugin_dir: Option<String> }), Status, Stop }`; bare `daemon` = `Run`). Dispatch in `main.rs`: `Run` → `daemon::serve(&cfg_path, resolve_plugin_dir(...))`; `Status` → `exit(if daemon::status() {0} else {1})`; `Stop` → `daemon::stop()`. Wrap each render arm:

```rust
let args_wire = /* build RenderArgsWire from `args` */;
match daemon_client::try_render(RegionKind::Right, args_wire) {
    Some(markup) => emit(&markup, args.preview),
    None => { /* existing in-process block, verbatim */ }
}
```

Add a `doctor` advisory row: whether `daemon_client::daemon_socket_path()` exists + a `Ping` succeeds (informational only, never a failing check).
- [ ] **Step 4: Run tests** — `cargo test -p rustline` + `just test`. PASS.
- [ ] **Step 5: fmt + clippy, commit** — `feat(cli): rustline daemon run|status|stop + render fallback (W48)`.

---

## Docs + finish

### Task 13: Documentation sync + WHATS-NEXT strip

**Files:**
- Modify: `CLAUDE.md`, `README.md`
- Modify: `WHATS-NEXT.md` (strip W33/W46/W48 — they ship here)

- [ ] **Step 1:** Update `CLAUDE.md`: the `[instances.<name>]` config section + the new `Config.instances`/`instance_kind`/`layout_kinds`/`disk_mounts`/`throughput_interfaces`; the widget `name`-field change; `Context.disks`/`throughputs` (note: NOT in `WireContext`); the `daemon` command group, `daemon_proto`/`daemon_client`/`daemon.rs` modules, the socket path, and the reserved `daemon.sock` + `throughput-sample-<iface>` state names; the init seeding/confirm behavior.
- [ ] **Step 2:** Update `README.md`: the multi-instance `[instances]` example (dual clocks, multiple mounts), the init confirm/seed note, and a new "Daemon (optional)" section (what it caches, `daemon run|status|stop`, an example systemd user unit + tmux `run-shell` line, the in-process-fallback guarantee).
- [ ] **Step 3:** Strip W33, W46, W48 (incl. their in-flight markers) from `WHATS-NEXT.md`.
- [ ] **Step 4: Commit** — `docs: sync CLAUDE.md + README.md for whats-next bundle #4; strip shipped items`.

### Task 14: Final review + finish branch

- [ ] Run the full gate: `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `just test`.
- [ ] Dispatch a final code-reviewer over the whole branch diff; address Critical/Important findings.
- [ ] Invoke `superpowers:finishing-a-development-branch`.

---

## Self-review

**Spec coverage:** W33 seeding (Task 1) + confirm (Task 2); W46 config (Task 3), widget-name identity (Task 4), registration (Task 5), color/click projection (Task 6), per-instance reads (Tasks 7–8); W48 protocol (Task 9), client+fallback (Task 10), server (Task 11), CLI+doctor (Task 12); docs + strip (Task 13); final review (Task 14). All spec sections map to a task.

**Placeholder scan:** Task 5's registration match and Task 8's helper bodies are described by exact behavior with representative arms shown; each is a mechanical repeat of the shown pattern across kinds (acceptable — the pattern and the per-kind Opts types are concrete). No TBD/TODO.

**Type consistency:** `build_<kind>(name, &Opts) -> Box<dyn Widget>` is used identically in Tasks 4/5/7. `build_region_context(args, layout, theme, cfg)` is the single new signature used in Tasks 8/11 and `main.rs`. `RegionKind`/`RenderArgsWire`/`DaemonRequest`/`DaemonResponse` are defined in Task 9 and consumed unchanged in Tasks 10–12. `Context.disks`/`throughputs` defined in Task 7, filled in Task 8.
