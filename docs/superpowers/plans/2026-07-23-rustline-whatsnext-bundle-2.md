# rustline whats-next bundle #2 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship 12 selected whats-next features (plus one time-boxed feasibility spike) across rustline's widgets, config/CLI, click handling, plugin dev-experience, ABI, capability observability, and plugin distribution.

**Architecture:** One phased plan over the existing Cargo workspace (`rustline-core`, `rustline-abi`, `rustline-wasm`, `rustline` bin, excluded `plugins/*`). New platform reads follow the `#[cfg(target_os = …, test)]` pure-parser-behind-a-read-surface pattern; new config defaults to byte-identical output; new HTTP is rustls-only. Tasks are dependency-ordered: ABI version (Task 8) before the SDK (Task 12); the denial seam (Task 10) before the plugin-run harness (Task 11).

**Tech Stack:** Rust edition 2024, clap v4 derive, serde/toml/toml_edit, chrono + chrono-tz, ureq (rustls), sha2, extism 1.x (wasmtime), tracing, tempfile (tests).

## Global Constraints

- **Edition 2024** in every crate; keep all crate editions == `rustfmt.toml`.
- **rustls-only:** any new HTTP uses `ureq { default-features = false, features = ["tls","json"] }`. `cargo tree -i openssl` and `cargo tree -i native-tls` MUST stay empty across the whole graph.
- **Clippy/fmt clean:** `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all --check` pass. No pre-commit hook — run `cargo fmt --all` before each commit.
- **`Config::load` is total** (invariant #3) — every new config field is `#[serde(default)]` and a bad value degrades, never panics.
- **Widgets read only `Context`** (invariant #1); a failed platform read is `Option::None`, never a fabricated value (invariant #6).
- **Click path injection-safe** (invariant #4): tmux vars pass as `--flag=#{q:VAR}`; command text is config-owned, never built from tmux data.
- **Click-toggle name identity** (invariant #7): the layout/registry name == range name == toggle-set key == binding-lookup key.
- **WASM: zero ambient authority** (N1), **a plugin never breaks the bar** (N2), **per-plugin capability scope** (N4).
- **`Cargo.lock` committed** with any dependency change.
- **Byte-identical at defaults:** every new option's default reproduces current output exactly. New widgets are opt-in (not in the default layout).
- Commit message trailer on every commit: `Claude-Session: https://claude.ai/code/session_01Fddrk6VgQAPGvRg71F2eB1`.

---

## Task 1: W37 — Uptime widget (read surface + widget)

**Files:**
- Create: `crates/rustline/src/uptime.rs`
- Modify: `crates/rustline/src/main.rs` (add `mod uptime;`)
- Modify: `crates/rustline-core/src/context.rs` (add `uptime: Option<u64>`)
- Create: `crates/rustline-core/src/widgets/uptime.rs`
- Modify: `crates/rustline-core/src/widgets/mod.rs` (mod + `pub use` + register + count test)
- Modify: `crates/rustline/src/build_context.rs` (gated read)
- Modify: `crates/rustline-core/src/config.rs` (`UptimeOpts` in `WidgetOpts`)

**Interfaces:**
- Produces: `rustline::uptime::read_uptime() -> Option<u64>` (seconds); pure `parse_proc_uptime(&str) -> Option<u64>`; `rustline_core::widgets::uptime::Uptime` widget; `Context.uptime: Option<u64>`; `humanize_uptime(secs: u64) -> String`.
- Consumes: existing `layout_needs`, `builtin_descriptor`, `active_format`/`clickable_range` (toggle helpers), `Context::default()`.

- [ ] **Step 1: Failing test for the pure parser + humanizer** in `crates/rustline/src/uptime.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_proc_uptime_first_float() {
        assert_eq!(parse_proc_uptime("12345.67 98765.43\n"), Some(12345));
        assert_eq!(parse_proc_uptime(""), None);
        assert_eq!(parse_proc_uptime("garbage"), None);
    }
}
```

And in `crates/rustline-core/src/widgets/uptime.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn humanizes_uptime_buckets() {
        assert_eq!(humanize_uptime(0), "<1m");
        assert_eq!(humanize_uptime(59), "<1m");
        assert_eq!(humanize_uptime(60), "1m");
        assert_eq!(humanize_uptime(60 * 75), "1h 15m");
        assert_eq!(humanize_uptime(86_400 * 3 + 3600 * 4), "3d 4h");
        assert_eq!(humanize_uptime(86_400), "1d 0h");
    }
}
```

- [ ] **Step 2: Run tests, verify they fail** — `cargo test -p rustline uptime` and `cargo test -p rustline-core uptime`. Expected: FAIL (unresolved names).

- [ ] **Step 3: Implement.** `uptime.rs`:

```rust
//! System uptime read surface (see cpu.rs/battery.rs pattern).
#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_proc_uptime(s: &str) -> Option<u64> {
    s.split_whitespace().next()?.parse::<f64>().ok().map(|f| f as u64)
}

#[cfg(target_os = "linux")]
pub fn read_uptime() -> Option<u64> {
    parse_proc_uptime(&std::fs::read_to_string("/proc/uptime").ok()?)
}
#[cfg(target_os = "macos")]
pub fn read_uptime() -> Option<u64> {
    // sysctl -n kern.boottime -> "{ sec = N, usec = M }"; now - sec.
    // Parse via parse_kern_boottime; return None on any failure.
    /* implement with std::process::Command; delegate to a pure parser */
    None
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn read_uptime() -> Option<u64> { None }
```

`humanize_uptime` in the widget module:

```rust
pub(crate) fn humanize_uptime(secs: u64) -> String {
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3600;
    let m = (secs % 3600) / 60;
    if d > 0 { format!("{d}d {h}h") }
    else if h > 0 { format!("{h}h {m}m") }
    else if m > 0 { format!("{m}m") }
    else { "<1m".to_string() }
}
```

Widget `Uptime { opts: UptimeOpts }` renders over `ctx.uptime` (Some → substitute `{uptime}` with `humanize_uptime`; None → `down_format`), honoring `active_format`/`clickable_range` like `loadavg`. `UptimeOpts { format: String = "{uptime}", alt_format: String = "", down_format: String = "" }` added to `config.rs`'s `WidgetOpts`.

Context field: add `pub uptime: Option<u64>` with `#[serde(default)]` to `Context` and to `Context::default()`.

Gated read in `build_context.rs`: `let uptime = if layout_needs(layout, "uptime") { crate::uptime::read_uptime() } else { None };` and set it in the `Context { … }` literal.

Register in `with_builtins`: a `register_described(builtin_descriptor("uptime", "system uptime", true), …)` block.

- [ ] **Step 4: Update the registry-count test.** In `widgets/mod.rs` change the `assert_eq!(names.len(), 13)` to `14`.

- [ ] **Step 5: Run all tests** — `just test`. Expected: PASS. Then `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all`.

- [ ] **Step 6: Commit** — `git add -A && git commit -m "feat(uptime): uptime widget + /proc/uptime read (W37)"`.

---

## Task 2: W30 — Per-widget timezone for `datetime`

**Files:**
- Modify: `crates/rustline-core/Cargo.toml` (add `chrono-tz`)
- Modify: `crates/rustline-core/src/widgets/datetime.rs`
- Modify: `crates/rustline-core/src/config.rs` (`DateTimeOpts.timezone`)
- Modify: `Cargo.lock`

**Interfaces:**
- Produces: `DateTimeOpts.timezone: Option<String>`; timezone-aware render.
- Consumes: `chrono_tz::Tz` (`FromStr`), existing `ctx.now: DateTime<Local>`.

- [ ] **Step 1: Failing test** in `datetime.rs` tests:

```rust
#[test]
fn renders_configured_timezone() {
    use chrono::TimeZone;
    // A fixed instant: 2026-01-01 00:30:00 UTC.
    let now = chrono::Local.timestamp_opt(1_767_227_400, 0).unwrap();
    let ctx = Context { now, ..Context::default() };
    let utc = DateTime { opts: DateTimeOpts { format: "%H".into(), timezone: Some("UTC".into()), ..Default::default() } };
    let local = DateTime { opts: DateTimeOpts { format: "%H".into(), timezone: None, ..Default::default() } };
    // UTC hour is deterministic; local depends on the box, so only assert UTC + no-panic on bad zone.
    assert_eq!(text_of(utc.render(&ctx)), "00");
    let bad = DateTime { opts: DateTimeOpts { format: "%H".into(), timezone: Some("Not/AZone".into()), ..Default::default() } };
    let _ = bad.render(&ctx); // must not panic; falls back to Local
    let _ = local.render(&ctx);
}
```

(Provide a small `text_of(Vec<Segment>) -> String` test helper if not present.)

- [ ] **Step 2: Run, verify fail** — `cargo test -p rustline-core datetime`. Expected: FAIL (no `timezone` field / no `chrono-tz`).

- [ ] **Step 3: Implement.** Add `chrono-tz = "0.10"` (or current) to `rustline-core`. Add `timezone: Option<String>` (`#[serde(default)]`) to `DateTimeOpts`. In render:

```rust
let formatted = match self.opts.timezone.as_deref().and_then(|z| z.parse::<chrono_tz::Tz>().ok()) {
    Some(tz) => ctx.now.with_timezone(&tz).format(&fmt).to_string(),
    None => {
        if self.opts.timezone.as_deref().is_some_and(|z| !z.is_empty()) {
            tracing::warn!(zone = self.opts.timezone.as_deref(), "unknown timezone; using Local");
        }
        ctx.now.format(&fmt).to_string()
    }
};
```

- [ ] **Step 4: Run tests** — `just test`; clippy; fmt.

- [ ] **Step 5: Verify rustls/dep hygiene** — `cargo tree -i openssl` empty. Commit `Cargo.lock`.

- [ ] **Step 6: Commit** — `git commit -am "feat(datetime): optional IANA timezone via chrono-tz (W30)"`.

---

## Task 3: W41 — Now-playing / media widget (playerctl shell-out)

**Files:**
- Modify: `crates/rustline-abi/src/lib.rs` (`MediaInfo`)
- Modify: `crates/rustline-core/src/context.rs` (`media: Option<MediaInfo>`, re-export `MediaInfo`)
- Create: `crates/rustline/src/media.rs`
- Modify: `crates/rustline/src/main.rs` (`mod media;`)
- Create: `crates/rustline-core/src/widgets/media.rs`
- Modify: `crates/rustline-core/src/widgets/mod.rs` (register + count test → 15)
- Modify: `crates/rustline/src/build_context.rs` (gated read)
- Modify: `crates/rustline-core/src/config.rs` (`MediaOpts`)

**Interfaces:**
- Produces: `rustline_abi::MediaInfo { artist, title, status }`; `Context.media: Option<MediaInfo>`; `rustline::media::read_media() -> Option<MediaInfo>`; pure `parse_playerctl(&str) -> Option<MediaInfo>`.
- Consumes: shell-out pattern from `git.rs`; toggle helpers.

- [ ] **Step 1: Failing test** in `media.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_tab_separated() {
        let m = parse_playerctl("Radiohead\tKarma Police\tPlaying\n").unwrap();
        assert_eq!(m.artist, "Radiohead");
        assert_eq!(m.title, "Karma Police");
        assert_eq!(m.status, "Playing");
        assert!(parse_playerctl("").is_none());
        // Missing fields tolerated as empty strings, still Some if a title exists.
        let m2 = parse_playerctl("\tOnly Title\t").unwrap();
        assert_eq!(m2.artist, "");
        assert_eq!(m2.title, "Only Title");
    }
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p rustline media`. Expected: FAIL.

- [ ] **Step 3: Implement.** `MediaInfo` in `rustline-abi` (`#[derive(Clone, Debug, Default, Serialize, Deserialize)]`), re-exported by `rustline-core`. `Context.media: Option<MediaInfo>` (`#[serde(default)]`) + in `Context::default()`.

```rust
// media.rs
pub(crate) fn parse_playerctl(s: &str) -> Option<rustline_core::MediaInfo> {
    let line = s.lines().next()?;
    if line.trim().is_empty() { return None; }
    let mut it = line.splitn(3, '\t');
    let artist = it.next().unwrap_or("").to_string();
    let title = it.next().unwrap_or("").to_string();
    let status = it.next().unwrap_or("").to_string();
    if artist.is_empty() && title.is_empty() { return None; }
    Some(rustline_core::MediaInfo { artist, title, status })
}
#[cfg(target_os = "linux")]
pub fn read_media() -> Option<rustline_core::MediaInfo> {
    let out = std::process::Command::new("playerctl")
        .args(["metadata", "--format", "{{artist}}\t{{title}}\t{{status}}"])
        .output().ok()?;
    if !out.status.success() { return None; }
    parse_playerctl(&String::from_utf8_lossy(&out.stdout))
}
#[cfg(not(target_os = "linux"))]
pub fn read_media() -> Option<rustline_core::MediaInfo> { None }
```

Widget `Media { opts: MediaOpts }` over `ctx.media`: substitute `{artist}/{title}/{status}`; `down_format` when `None`; toggle-aware. `MediaOpts { format: String = "{title} — {artist}", down_format = "", alt_format = "" }`. Gated read `layout_needs(layout, "media")`. Register + count test → 15.

- [ ] **Step 4: Run** — `just test`; clippy; fmt.
- [ ] **Step 5: Commit** — `git commit -am "feat(media): now-playing widget via playerctl (W41)"`.

---

## Task 4: W35 — Global `--config <path>` override

**Files:**
- Modify: `crates/rustline/src/cli.rs` (global `--config`)
- Modify: `crates/rustline/src/main.rs` (`effective_config_path`, thread into dispatch)
- Modify: `crates/rustline/src/plugin_cmd.rs`, `theme_cmd.rs`, `init.rs` (accept the resolved path instead of calling `config_path()` directly)
- Test: `crates/rustline/tests/smoke.rs`

**Interfaces:**
- Produces: `Cli.config: Option<PathBuf>`; `effective_config_path(&Option<PathBuf>) -> PathBuf`.
- Consumes: existing `config_path()`.

- [ ] **Step 1: Failing smoke test** — invoke the built binary with `--config <tmp>` + `print-config` and assert it reflects the alternate file; `config path` prints the override.

```rust
#[test]
fn config_flag_overrides_path() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("alt.toml");
    std::fs::write(&cfg, "layout.left = [\"hostname\"]\n").unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_rustline"))
        .args(["--config", cfg.to_str().unwrap(), "config", "path"])
        .output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("alt.toml"));
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p rustline config_flag_overrides_path`. Expected: FAIL (unknown `--config`).
- [ ] **Step 3: Implement.** Add `#[arg(long = "config", global = true)] pub config: Option<PathBuf>` to `Cli`. In `main`, compute `let cfg_path = effective_config_path(&cli.config);` once and pass it everywhere `config_path()` is used today (dispatch load, `config path|edit|validate`, `print-config`, and the `plugin_cmd`/`theme_cmd`/`init` entry points — add a `config_path: &Path` parameter to those functions). `effective_config_path` = `flag.clone().unwrap_or_else(config_path)`.
- [ ] **Step 4: Run** — `just test`; clippy; fmt.
- [ ] **Step 5: Commit** — `git commit -am "feat(cli): global --config path override (W35)"`.

---

## Task 5: W29 — Explicit per-widget color override

**Files:**
- Modify: `crates/rustline-core/src/config.rs` (shared `ColorOverride { fg, bg }`, embedded per widget; a `Config::color_overrides() -> HashMap<String, ColorOverride>` projector)
- Modify: `crates/rustline-core/src/assemble.rs` (`render_named_region` applies overrides pre-`assign_palette`)
- Modify: `crates/rustline/src/main.rs` (pass the map into the render call)
- Test: `crates/rustline-core/src/assemble.rs` tests

**Interfaces:**
- Produces: `ColorOverride { fg: Option<Color>, bg: Option<Color> }`; `render_named_region(..., overrides: &HashMap<String, ColorOverride>)`.
- Consumes: existing `assign_palette` (already skips segments carrying an explicit bg).

- [ ] **Step 1: Failing test** — a widget named `datetime` with `bg = Some(Color::Named("blue"))` keeps that bg through `assign_palette`; other widgets still get palette colors; an empty override map yields byte-identical output to today.

```rust
#[test]
fn per_widget_color_override_pins_bg() {
    let mut overrides = std::collections::HashMap::new();
    overrides.insert("datetime".to_string(), ColorOverride { fg: None, bg: Some(Color::Named("blue".into())) });
    // render a two-widget region; assert datetime's segment bg == blue, other == palette[0/1].
}
#[test]
fn empty_overrides_are_byte_identical() {
    // same region rendered with empty map == render_named_region without the param (characterization).
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p rustline-core color_override`. Expected: FAIL.
- [ ] **Step 3: Implement.** `ColorOverride` (both `Option<Color>`, `#[serde(default)]`), embedded into each format-bearing widget's opts via `#[serde(default, flatten)] color: ColorOverride` (or a shared field). `Config::color_overrides()` walks the layout-relevant widget opts into a name→override map. In `render_named_region`, after each widget renders and before `assign_palette`, for a widget whose name has an override: set `bg` on segments that lack an explicit bg, set `fg` where specified. Thread the map from `main` (built via `cfg.color_overrides()`).
- [ ] **Step 4: Run** — `just test`; clippy; fmt.
- [ ] **Step 5: Commit** — `git commit -am "feat(theme): per-widget fg/bg color override (W29)"`.

---

## Task 6: W36 — Configurable click bindings

**Files:**
- Modify: `crates/rustline-core/src/config.rs` (`ClickBinding`, `ClickAction`, per-widget bindings)
- Create: `crates/rustline/src/click.rs` (`resolve_click`, `ClickExecutor` trait, real executor)
- Modify: `crates/rustline/src/main.rs` (`run_click` → resolver + dispatch)
- Test: `crates/rustline/src/click.rs` tests

**Interfaces:**
- Produces: `ClickAction { Toggle, OpenUrl(String), Run(String), NoOp }`; `resolve_click(cfg: &Config, range: &str, button: &str) -> ClickAction`; `trait ClickExecutor`.
- Consumes: existing `toggles::{read,apply,write}_toggles`, `ClickArgs`.

- [ ] **Step 1: Failing test** in `click.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_default_and_configured() {
        let cfg = /* Config with datetime.alt_format set, no bindings */;
        assert!(matches!(resolve_click(&cfg, "datetime", "left"), ClickAction::Toggle));
        assert!(matches!(resolve_click(&cfg, "datetime", "right"), ClickAction::NoOp));
        let cfg2 = /* Config: [widgets.cpu] right_click = { run = "htop" } */;
        assert!(matches!(resolve_click(&cfg2, "cpu", "right"), ClickAction::Run(ref c) if c == "htop"));
        assert!(matches!(resolve_click(&cfg2, "cpu", "middle"), ClickAction::NoOp));
    }
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p rustline resolve`. Expected: FAIL.
- [ ] **Step 3: Implement.** Config: `ClickAction` serde enum (`{ toggle = true }` | `{ open_url = "…" }` | `{ run = "…" }`); a per-widget optional `left_click`/`right_click`/`middle_click: Option<ClickAction>` (all `#[serde(default)]`). `resolve_click`: if a binding exists for `(range, button)` return it; else default — `left` on a widget with a non-empty `alt_format` → `Toggle`, otherwise `NoOp` (byte-identical to today). Dispatch: `Toggle` → existing toggles flip; `OpenUrl` → spawn `xdg-open`/`open`; `Run` → `sh -c` detached; all spawn failures `warn!`-logged, never fatal. Executor behind a `ClickExecutor` trait so tests don't spawn.
- [ ] **Step 4: Run** — `just test`; clippy; fmt.
- [ ] **Step 5: Commit** — `git commit -am "feat(click): configurable per-widget/per-button bindings (W36)"`.

---

## Task 7: W32 — Host/guest ABI version negotiation

**Files:**
- Modify: `crates/rustline-abi/src/lib.rs` (`ABI_VERSION`)
- Modify: `crates/rustline-wasm/src/abi.rs` (`RenderInput.abi_version`)
- Modify: `crates/rustline-wasm/src/lib.rs` (check guest `abi_version()` alongside `name()`)
- Modify: `crates/rustline-wasm/src/host.rs` (pass version in `RenderInput`)
- Test: `crates/rustline-wasm/src/lib.rs` tests

**Interfaces:**
- Produces: `rustline_abi::ABI_VERSION: u32 = 1`; `RenderInput.abi_version: u32`; `abi_decision(host: u32, guest: Option<u32>) -> AbiDecision { Register, RegisterLegacy, Skip }`.
- Consumes: existing `name()` verification path.

- [ ] **Step 1: Failing test** for the pure decision:

```rust
#[test]
fn abi_decision_matrix() {
    assert!(matches!(abi_decision(1, Some(1)), AbiDecision::Register));
    assert!(matches!(abi_decision(1, None), AbiDecision::RegisterLegacy));
    assert!(matches!(abi_decision(1, Some(2)), AbiDecision::Skip));
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p rustline-wasm abi_decision`. Expected: FAIL.
- [ ] **Step 3: Implement.** `ABI_VERSION` in abi; `abi_version: u32` on `RenderInput` (set to `ABI_VERSION` in `host.rs`). During registration, after `name()` matches, `plugin.call::<&str,&str>("abi_version", "")`: parse to `u32` → `Some(v)`; a missing/erroring export → `None`. Feed `abi_decision(ABI_VERSION, guest)`: `Register`/`RegisterLegacy` (info-log) → register; `Skip` → `warn!(name, host=…, guest=…)` + skip. Existing plugins (no export) hit `RegisterLegacy` and keep working.
- [ ] **Step 4: Run** — `just test`; clippy; fmt. (Optionally `just test-wasm` if a guest is updated to export it.)
- [ ] **Step 5: Commit** — `git commit -am "feat(wasm): host/guest ABI version negotiation (W32)"`.

---

## Task 8: W28 (part A) — Capability denial-observation seam

**Files:**
- Modify: `crates/rustline-wasm/src/capability.rs` (`DenialObserver`, `CapabilityCtx.observe_denial`)
- Modify: `crates/rustline-wasm/src/perform.rs` (call `observe_denial` at each deny site)
- Test: `crates/rustline-wasm/src/perform.rs` tests (extend denied-case tests)

**Interfaces:**
- Produces: `trait DenialObserver { fn observe(&self, plugin: &str, kind: DenialKind, target: &str); }`; `DenialKind { Url, Path }`; `CapabilityCtx.observe_denial(kind, target)`; a `NoopObserver` default.
- Consumes: existing `perform_*` deny sites; `ctx.name`.

- [ ] **Step 1: Failing test** — a spy observer records `(name, kind, target)` when `perform_http_get`/`perform_file_read`/`perform_file_write`/`perform_http_get_cached` deny:

```rust
#[test]
fn denied_http_notifies_observer() {
    let spy = SpyObserver::default();
    let ctx = CapabilityCtx::test_with_observer(/* empty allowlists */, spy.clone());
    let r = perform_http_get(&ctx, "https://blocked.example/x", &UreqFetcher);
    assert!(!r.ok);
    assert_eq!(spy.records(), vec![("plug".into(), DenialKind::Url, "https://blocked.example/x".into())]);
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p rustline-wasm denied_http_notifies_observer`. Expected: FAIL.
- [ ] **Step 3: Implement.** Add a boxed `Arc<dyn DenialObserver + Send + Sync>` to `CapabilityCtx` (default `NoopObserver`). At each of the four deny sites, call `ctx.observe_denial(kind, target)` immediately **before** the existing `ok:false` return — no change to gating (N1). Keep `perform_*` signatures pure; the observer is on `ctx`.
- [ ] **Step 4: Run** — `just test`; clippy; fmt.
- [ ] **Step 5: Commit** — `git commit -am "feat(wasm): denial-observation seam on CapabilityCtx (W28a)"`.

---

## Task 9: W34 — Local `plugin run <name>` dev harness

**Files:**
- Modify: `crates/rustline/src/plugin_cmd.rs` (`run` subcommand)
- Modify: `crates/rustline/src/cli.rs` (`PluginCmd::Run { name, plugin_dir }`)
- Modify: `crates/rustline/src/build_context.rs` or a small local fixture (sample Context)
- Test: `crates/rustline/src/plugin_cmd.rs` tests (formatting helper)

**Interfaces:**
- Produces: `plugin run` that prints one plugin's `Vec<Segment>` + captured denials.
- Consumes: `register_plugins`/`build_plugin`, Task 8's `DenialObserver` (a collecting observer), a fabricated `Context`.

- [ ] **Step 1: Failing test** — a pure `format_run_output(segments: &[Segment], denials: &[Denial]) -> String` renders a readable dump; assert it lists segment text and any denial lines.
- [ ] **Step 2: Run, verify fail** — `cargo test -p rustline format_run_output`. Expected: FAIL.
- [ ] **Step 3: Implement.** `PluginCmd::Run { name, plugin_dir }`. Build a sample `Context` (reuse the bench `fabricated_context` if reachable, else a small builder). Instantiate the named plugin with a **collecting** `DenialObserver`; call `render`; print `format_run_output`. Read-only; no config writes.
- [ ] **Step 4: Run** — `just test`; clippy; fmt. (Manual/`wasm-e2e` for the real wasm path.)
- [ ] **Step 5: Commit** — `git commit -am "feat(plugin): plugin run dev harness (W34)"`.

---

## Task 10: W31 — Generic `plugin build <dir>`

**Files:**
- Modify: `crates/rustline/src/cli.rs` (`PluginCmd::Build { dir, release, plugin_dir }`)
- Modify: `crates/rustline/src/plugin_cmd.rs` (build + install)
- Test: `crates/rustline/src/plugin_cmd.rs` tests (artifact-path resolver)

**Interfaces:**
- Produces: `plugin build <dir>`; pure `wasm_artifact_path(target_dir, stem, release) -> PathBuf`.
- Consumes: `resolve_plugin_dir`.

- [ ] **Step 1: Failing test** — `wasm_artifact_path` returns `<target>/wasm32-unknown-unknown/release/<stem>.wasm` for release, `.../debug/...` otherwise.
- [ ] **Step 2: Run, verify fail** — `cargo test -p rustline wasm_artifact_path`. Expected: FAIL.
- [ ] **Step 3: Implement.** `plugin build <dir> [--release]`: shell `cargo build --target wasm32-unknown-unknown [--release]` in `<dir>`; resolve the artifact via `wasm_artifact_path`; copy to the resolved plugin dir as `<stem>.wasm`. Non-zero cargo exit or missing artifact → clear `anyhow` error (non-zero process exit), no panic.
- [ ] **Step 4: Run** — `just test`; clippy; fmt.
- [ ] **Step 5: Commit** — `git commit -am "feat(plugin): generic plugin build <dir> (W31)"`.

---

## Task 11: W39 — `rustline-plugin-sdk` guest crate (+ migrate examples)

**Files:**
- Create: `crates/rustline-plugin-sdk/{Cargo.toml, src/lib.rs}`
- Modify: workspace root `Cargo.toml` (add member)
- Modify: `plugins/{weather,counter,filewatch,httpget}/{Cargo.toml,src/lib.rs}` (depend on + use the SDK)
- Modify: `crates/rustline/assets/plugin-lib.rs.tmpl`, `plugin-cargo.toml.tmpl` (scaffold uses the SDK)
- Test: `crates/rustline-plugin-sdk/src/lib.rs` host-target tests

**Interfaces:**
- Produces: `rustline_plugin_sdk`: typed wrappers `http_get`, `http_get_cached`, `state_read`, `state_write`, `file_read`, `file_write`, `log`; re-exports of `GuestRender`/`WireContext`/`Segment`/`Style`/`Color`; toggle helper `active_format(ctx, name, format, alt) -> &str`; `export_plugin!` macro emitting `name`/`render`/`abi_version` (via `rustline_abi::ABI_VERSION`).
- Consumes: `rustline-abi` (path), Task 7's `ABI_VERSION`.

- [ ] **Step 1: Failing test** — host-target unit tests for the toggle helper and for the wrappers' request/response encode-decode against the `rustline-abi` wire types (no wasm needed for the pure pieces).
- [ ] **Step 2: Run, verify fail** — `cargo test -p rustline-plugin-sdk`. Expected: FAIL (crate absent).
- [ ] **Step 3: Implement.** New edition-2024 lib crate. `#[cfg(target_arch = "wasm32")]` extern block for the seven host fns + safe wrappers returning `Result` over the abi result types; host-target stubs so pure logic tests compile. `export_plugin!` macro wires the exports. Migrate all four example plugins to `use rustline_plugin_sdk::*` + `export_plugin!`, deleting their hand-rolled extern blocks and `v["context"]…` walking. Update the two scaffold templates.
- [ ] **Step 4: Run** — `just test`; then `just build-plugin weather && just build-plugin counter && just build-plugin filewatch && just build-plugin httpget` (needs the wasm target); `just test-wasm` for at least weather. Clippy; fmt.
- [ ] **Step 5: Verify dep hygiene** — `cargo tree -i openssl` empty; commit `Cargo.lock`s.
- [ ] **Step 6: Commit** — `git commit -am "feat(sdk): rustline-plugin-sdk guest crate + migrate examples (W39)"`.

---

## Task 12: W28 (part B) — Persisted denial recorder + `plugin denials` CLI

**Files:**
- Modify: `crates/rustline-wasm/src/capability.rs` or a new `denials.rs` (persisted recorder + dedup)
- Modify: `crates/rustline-wasm/src/host.rs`/`lib.rs` (wire the real recorder into `build_plugin`)
- Modify: `crates/rustline/src/plugin_cmd.rs` (`denials` subcommand + `approve` hint)
- Modify: `crates/rustline/src/cli.rs` (`PluginCmd::Denials { name }`)
- Test: recorder round-trip + dedup unit tests

**Interfaces:**
- Produces: `FileDenialObserver` (append deduped records under `<data_root>/denials.jsonl`); `read_denials(name) -> Vec<Denial>`; `plugin denials <name>`.
- Consumes: Task 8's `DenialObserver` trait; `data_root()`.

- [ ] **Step 1: Failing test** — writing the same `(plugin, kind, target)` twice yields one record; `read_denials` round-trips; a write failure is swallowed (no panic).
- [ ] **Step 2: Run, verify fail** — `cargo test -p rustline-wasm denials`. Expected: FAIL.
- [ ] **Step 3: Implement.** `FileDenialObserver` implementing `DenialObserver`: append-with-dedup to a JSONL file under the data dir (best-effort; a write error `warn!`s, per the toggles-file discipline). Wire it as the default observer in `build_plugin`. CLI `plugin denials <name>` lists them; `plugin approve <name>` prints a "recorded denials: run `plugin denials <name>`" hint when any exist.
- [ ] **Step 4: Run** — `just test`; clippy; fmt.
- [ ] **Step 5: Commit** — `git commit -am "feat(plugin): persisted denial record + plugin denials CLI (W28b)"`.

---

## Task 13: W38 — Plugin install by `owner/repo`

**Files:**
- Modify: `crates/rustline/Cargo.toml` (add `ureq` rustls + `sha2`)
- Create: `crates/rustline/src/plugin_install.rs` (`Downloader` seam, GitHub resolve, sha256, config write)
- Modify: `crates/rustline-core/src/config.rs` (`PluginSource` typed enum + back-compat deserialize; `checksum`, `tag` fields)
- Modify: `crates/rustline/src/cli.rs` (`PluginCmd::{Install, Update, Remove}`)
- Modify: `crates/rustline/src/plugin_cmd.rs` (dispatch)
- Modify: `Cargo.lock`
- Test: `crates/rustline/src/plugin_install.rs` tests

**Interfaces:**
- Produces: `trait Downloader { fn get_json(&self, url) -> Result<serde_json::Value>; fn get_bytes(&self, url) -> Result<Vec<u8>>; }`; `select_wasm_asset(&Value) -> Option<(String /*name*/, String /*url*/)>`; `parse_owner_repo(&str) -> Option<(String,String)>`; `sha256_hex(&[u8]) -> String`; the install/update/remove flows generic over `Downloader`.
- Consumes: `toml_edit` config write pattern from `plugin_cmd.rs`; `resolve_plugin_dir`.

- [ ] **Step 1: Failing tests** (all pure, no network):

```rust
#[test]
fn parses_owner_repo() {
    assert_eq!(parse_owner_repo("steve/rustline-weather"), Some(("steve".into(),"rustline-weather".into())));
    assert_eq!(parse_owner_repo("nope"), None);
}
#[test]
fn selects_wasm_asset_from_release_json() {
    let json: serde_json::Value = serde_json::from_str(SAMPLE_RELEASE_JSON).unwrap();
    let (name, url) = select_wasm_asset(&json).unwrap();
    assert!(name.ends_with(".wasm"));
    assert!(url.starts_with("https://"));
}
#[test]
fn sha256_is_stable() {
    assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}
#[test]
fn install_writes_source_tag_checksum() {
    // Fake Downloader returns SAMPLE_RELEASE_JSON + known bytes; run install into a tempdir;
    // assert [plugins.<name>] has source="owner/repo", tag, checksum=sha256(bytes).
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p rustline plugin_install`. Expected: FAIL.
- [ ] **Step 3: Implement.** Add `ureq { default-features = false, features = ["tls","json"] }` and `sha2` to the bin. `UreqDownloader` (rustls, follows redirects for asset bytes; sends a `User-Agent` for the GitHub API). `PluginSource` enum (`OwnerRepo(String)`, `Url(String)`, `Path(String)`) with a `Deserialize` that accepts a bare string as `OwnerRepo` (keeps existing `source = "steve/rustline-weather"` configs parsing). Add `checksum: Option<String>`, `tag: Option<String>` to `PluginConfig` (`#[serde(default)]`). `install`: resolve `releases/latest` or `tags/<t>`, `select_wasm_asset`, `get_bytes`, `sha256_hex`, write `.wasm` to plugin dir, write `[plugins.<name>]` via `toml_edit`. `update`: re-resolve latest. `remove`: delete `.wasm` + (with `--yes`) the config entry. **Install grants no capabilities** — note it in the printed follow-up (point at `plugin approve`).
- [ ] **Step 4: Run** — `just test`; `cargo tree -i openssl` empty; clippy; fmt; commit `Cargo.lock`.
- [ ] **Step 5: Commit** — `git commit -am "feat(plugin): install/update/remove by owner/repo + checksum record (W38)"`.

---

## Task 14: W43 — Compiled-module cache feasibility spike (investigation)

**Files:**
- Create: `docs/superpowers/notes/2026-07-23-w43-compiled-module-cache-feasibility.md`
- (Conditional) Modify: `crates/rustline-wasm/src/host.rs` (only if the prototype proves out)

**Interfaces:** none committed unless feasible.

- [ ] **Step 1: Investigate** Extism 1.x's public API for precompiled/serialized modules usable across cold spawns (`CompiledPlugin`, any wasmtime `Module::serialize`/`deserialize` reachability without bypassing the capability-gated host). Time-box it.
- [ ] **Step 2: Write the findings doc** — API surface examined, verdict (feasible / infeasible), and, if infeasible, exactly what would unblock it (daemon W48 or an upstream Extism precompile API).
- [ ] **Step 3 (conditional):** If feasible, prototype behind a seam in `host.rs` (hash `.wasm` bytes → cache precompiled artifact under the state dir → load on next cold spawn), with `rustline bench --cold` before/after numbers, TDD where the cache-key/path logic is pure. If infeasible, no code — annotate W43 in `WHATS-NEXT.md`.
- [ ] **Step 4: Commit** — `git commit -m "spike(wasm): W43 compiled-module cache feasibility finding"` (doc always; code only if feasible).

---

## Task 15: Docs sync + final green + WHATS-NEXT maintenance

**Files:**
- Modify: `CLAUDE.md`, `README.md`
- Modify: `WHATS-NEXT.md` (strip shipped items — note it is gitignored, so this is local bookkeeping)

- [ ] **Step 1:** Update `CLAUDE.md` (module map, CLI, Config, Invariants, Roadmap) and `README.md` for every shipped item: `uptime` + `media` widgets; `datetime.timezone`; per-widget `fg`/`bg`; click bindings; ABI versioning; `plugin build`/`run`/`install`/`update`/`remove`/`denials`; global `--config`; the `rustline-plugin-sdk` crate. Move the corresponding Roadmap entries to Done.
- [ ] **Step 2:** Strip the shipped items from `WHATS-NEXT.md` (W28/29/30/31/32/34/35/36/37/38/39/41; W43 stays, annotated with the spike finding).
- [ ] **Step 3: Full green** — `just test`; `cargo clippy --all-targets -- -D warnings`; `cargo fmt --all --check`; `cargo tree -i openssl` and `-i native-tls` empty; `just test-wasm` (guest/SDK changes).
- [ ] **Step 4: Commit** — `git commit -am "docs: sync CLAUDE.md + README.md for whats-next bundle #2"`.

---

## Self-Review

**Spec coverage:** W37→T1, W30→T2, W41→T3, W35→T4, W29→T5, W36→T6, W32→T7, W28→T8+T12, W34→T9, W31→T10, W39→T11, W38→T13, W43→T14, cross-cutting/docs→T15. All 13 selected IDs mapped.

**Dependency order:** T7 (ABI) before T11 (SDK) ✓. T8 (denial seam) before T9 (plugin run consumes it) ✓ and before T12 (persisted recorder builds on the seam) ✓. Widgets (T1/T3) independently bump the count test 13→14→15 — implementers must apply them in order or reconcile the count; noted in each task.

**Placeholder scan:** Config/CLI plumbing tasks (T4/T5/T6/T9/T10) intentionally give exact files + interfaces + test-first anchors rather than every line, since the load-bearing logic (parsers, resolvers, decision matrices, asset selection, sha256, dedup) is where the concrete test code lives — that is where bugs hide and where TDD earns its keep. No `TODO`/`TBD` left in implementation steps.

**Type consistency:** `MediaInfo`, `ColorOverride`, `ClickAction`, `AbiDecision`/`abi_decision`, `DenialObserver`/`DenialKind`, `PluginSource`, `Downloader`/`select_wasm_asset`/`parse_owner_repo`/`sha256_hex` are named identically across the tasks that define and consume them.
