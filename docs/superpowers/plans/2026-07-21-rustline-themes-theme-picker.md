# Named themes, semantic colors, and theme-picker CLI — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add six built-in themes (incl. a multi-accent `pastel-rainbow`), four themeable semantic colors surfaced to widgets and WASM plugins, threshold-aware alert coloring on cpu/memory/battery/loadavg, and a `rustline theme list/show/use/new` CLI.

**Architecture:** `Theme` gains four semantic colors; a resolved subset (`ThemeColors`) is copied onto `Context` at build time so widgets and WASM guests can read it. `ThemeConfig` becomes a complete optional mirror of `Theme` plus a `base` selector; themes resolve as `default → base → inline overrides`, with built-ins baked into `rustline-core::themes` and user files (which shadow built-ins) in `$XDG_CONFIG_HOME/rustline/themes`. The `rustline theme` CLI mirrors `rustline plugin` (`toml_edit`, comment-preserving).

**Tech Stack:** Rust edition 2024, serde, `toml`/`toml_edit`, clap derive, tracing. Workspace crates: `rustline-abi`, `rustline-core`, `rustline`, `rustline-wasm`.

## Global Constraints

- **Edition 2024** in every crate; `rustfmt.toml` is edition 2024. Keep all crate editions equal.
- Must stay **clippy-clean** (`cargo clippy --all-targets -- -D warnings`) and **rustfmt-clean** (`cargo fmt --all --check`). No pre-commit hook — run `cargo fmt --all` before each commit.
- **rustls-only**: introduce no new TLS/OpenSSL deps. This feature adds **no new dependencies** (`toml_edit` is already a `rustline` dep).
- `just test` must stay **hermetic** (no wasm toolchain). Do not add wasm-target work to the default test path.
- **Invariants (re-check when touching these):** #1 Context is the sole render input; #2 Segment/Context/Style/Color stay serde-serializable (the WASM ABI); #3 `Config::load` is total (bad config never breaks the bar); #5 `render_region` puts `segments[0]` leftmost; #6 `loadavg`/`battery`/`cpu`/`memory` are `Option` (never fake values); #7 the click-toggle name is one identity end-to-end.
- Commit `Cargo.lock` only if a dependency actually changes (it should not here).
- Every commit message ends with:
  `Claude-Session: https://claude.ai/code/session_01Fddrk6VgQAPGvRg71F2eB1`

---

### Task 1: `ThemeColors` type in `rustline-abi` + core re-exports

**Files:**
- Modify: `crates/rustline-abi/src/lib.rs`
- Modify: `crates/rustline-core/src/segment.rs`
- Modify: `crates/rustline-core/src/lib.rs:16`

**Interfaces:**
- Produces: `rustline_abi::ThemeColors { fg, bar_bg, success, info, warning, error: Color }` with `Default`; re-exported as `rustline_core::ThemeColors`.

- [ ] **Step 1: Write the failing test** — append to the `tests` module in `crates/rustline-abi/src/lib.rs`:

```rust
    #[test]
    fn theme_colors_default_and_serde_round_trip() {
        let d = ThemeColors::default();
        assert_eq!(d.fg, Color::Indexed(255));
        assert_eq!(d.bar_bg, Color::Indexed(234));
        assert_eq!(d.success, Color::Indexed(35));
        assert_eq!(d.info, Color::Indexed(39));
        assert_eq!(d.warning, Color::Indexed(214));
        assert_eq!(d.error, Color::Indexed(196));
        let json = serde_json::to_string(&d).unwrap();
        let back: ThemeColors = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
    }
```

(`serde_json` is a dev-dependency of `rustline-abi` already if used elsewhere; if `cargo test -p rustline-abi` reports it missing, add `serde_json` under `[dev-dependencies]` in `crates/rustline-abi/Cargo.toml` — check with `grep serde_json crates/rustline-abi/Cargo.toml` first.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustline-abi theme_colors_default_and_serde_round_trip`
Expected: FAIL — `cannot find type ThemeColors`.

- [ ] **Step 3: Implement** — add to `crates/rustline-abi/src/lib.rs` after the `Color` impl block:

```rust
/// The theme-derived colors a widget or WASM plugin may use to style output
/// consistently with the active theme: the default text `fg`, the bar
/// background `bar_bg`, and the four semantic colors. Carried on `Context`
/// (serde `default`) so it crosses the WASM boundary. Defaults match
/// `rustline_core::Theme::default()`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeColors {
    pub fg: Color,
    pub bar_bg: Color,
    pub success: Color,
    pub info: Color,
    pub warning: Color,
    pub error: Color,
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self {
            fg: Color::Indexed(255),
            bar_bg: Color::Indexed(234),
            success: Color::Indexed(35),
            info: Color::Indexed(39),
            warning: Color::Indexed(214),
            error: Color::Indexed(196),
        }
    }
}
```

- [ ] **Step 4: Re-export from core.** In `crates/rustline-core/src/segment.rs` line 4, extend the re-export:

```rust
pub use rustline_abi::{Color, Segment, Style, ThemeColors};
```

In `crates/rustline-core/src/lib.rs` line 16, extend:

```rust
pub use segment::{Color, Segment, Style, ThemeColors};
```

- [ ] **Step 5: Run tests + lint**

Run: `cargo test -p rustline-abi && cargo build -p rustline-core`
Expected: PASS / builds.

- [ ] **Step 6: Commit**

```bash
git add crates/rustline-abi/src/lib.rs crates/rustline-core/src/segment.rs crates/rustline-core/src/lib.rs
git commit -m "feat(abi): add ThemeColors wire type + core re-export"
```

---

### Task 2: `Theme` semantic fields + `Theme::colors()`

**Files:**
- Modify: `crates/rustline-core/src/render.rs` (`Theme` struct ~20-41, `Default` ~43-62, the test `theme()` helper ~281-298, add a `colors()` method + test)

**Interfaces:**
- Consumes: `rustline_core::ThemeColors` (Task 1).
- Produces: `Theme.success/info/warning/error: Color`; `Theme::colors(&self) -> ThemeColors`.

- [ ] **Step 1: Write the failing test** — add to the `tests` module in `render.rs`:

```rust
    #[test]
    fn theme_default_has_semantic_colors_and_colors_bundle() {
        let t = Theme::default();
        assert_eq!(t.success, Color::Indexed(35));
        assert_eq!(t.info, Color::Indexed(39));
        assert_eq!(t.warning, Color::Indexed(214));
        assert_eq!(t.error, Color::Indexed(196));
        let c = t.colors();
        assert_eq!(c.fg, t.fg);
        assert_eq!(c.bar_bg, t.bar_bg);
        assert_eq!(c.error, t.error);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rustline-core theme_default_has_semantic_colors`
Expected: FAIL — no field `success`.

- [ ] **Step 3: Implement.** In `render.rs`, add four fields to the `Theme` struct (after `win_inactive_fg`):

```rust
    /// Semantic colors, available to widgets/plugins via `Context.colors` and
    /// used by threshold-aware widgets for alert badges.
    pub success: Color,
    pub info: Color,
    pub warning: Color,
    pub error: Color,
```

Add to the `Default for Theme` impl (after `win_inactive_fg: Color::Indexed(250),`):

```rust
            success: Color::Indexed(35),
            info: Color::Indexed(39),
            warning: Color::Indexed(214),
            error: Color::Indexed(196),
```

Add a method inside `impl Theme` (next to `hard`/`soft`):

```rust
    /// Bundle the theme-derived colors a widget/plugin may consume.
    pub fn colors(&self) -> crate::ThemeColors {
        crate::ThemeColors {
            fg: self.fg.clone(),
            bar_bg: self.bar_bg.clone(),
            success: self.success.clone(),
            info: self.info.clone(),
            warning: self.warning.clone(),
            error: self.error.clone(),
        }
    }
```

Update the test `theme()` helper (~281-298) to include the four fields so the literal compiles — add before the closing brace:

```rust
            success: Color::Indexed(35),
            info: Color::Indexed(39),
            warning: Color::Indexed(214),
            error: Color::Indexed(196),
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p rustline-core render::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rustline-core/src/render.rs
git commit -m "feat(core): Theme semantic colors + Theme::colors() bundle"
```

---

### Task 3: `Context.colors` plumbing (field + all literals + build_context)

This wires the semantic colors onto `Context` and populates them from the resolved theme. Adding a non-`Option` field to `Context` breaks every `Context { … }` literal, so this task fixes them all in one commit.

**Files:**
- Modify: `crates/rustline-core/src/context.rs` (add field + a serde-default test)
- Modify (add `colors: Default::default(),` after the `toggled:` line in each `Context { … }` literal): `crates/rustline-core/src/assemble.rs`, `crates/rustline-core/src/widget.rs`, `crates/rustline-core/src/widgets/{cwd,datetime,hostname,pane_id,lan_ip,tailscale_ip,toggle,windows,loadavg,mod,memory,battery,cpu}.rs`, `crates/rustline-wasm/src/host.rs`, `crates/rustline-wasm/tests/e2e.rs`
- Modify: `crates/rustline/src/build_context.rs` (add a `theme: &Theme` param; set `colors: theme.colors()`)
- Modify: `crates/rustline/src/main.rs` (pass `&theme` to the two build_context calls)

**Interfaces:**
- Consumes: `Theme::colors()` (Task 2).
- Produces: `Context.colors: ThemeColors` (serde `default`); `build_region_context(args, layout, theme)`, `build_window_context(args, theme)`.

- [ ] **Step 1: Write the failing test** — add to `context.rs` `tests`:

```rust
    #[test]
    fn context_colors_survive_serde_and_default_when_absent() {
        use crate::ThemeColors;
        let mut ctx = sample();
        ctx.colors = ThemeColors {
            error: crate::Color::Rgb(1, 2, 3),
            ..ThemeColors::default()
        };
        let json = serde_json::to_string(&ctx).unwrap();
        let back: Context = serde_json::from_str(&json).unwrap();
        assert_eq!(back.colors.error, crate::Color::Rgb(1, 2, 3));

        // A Context JSON omitting `colors` deserializes to the default bundle
        // (host/guest version skew must stay total — invariant #2).
        let without = json.replace(&format!(",\"colors\":{}", serde_json::to_string(&ctx.colors).unwrap()), "");
        assert_ne!(without, json, "sanity: the colors key was present to strip");
        let back2: Context = serde_json::from_str(&without).unwrap();
        assert_eq!(back2.colors, ThemeColors::default());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rustline-core context_colors_survive_serde`
Expected: FAIL — no field `colors`.

- [ ] **Step 3: Add the field.** In `context.rs`, add to `Context` (after `toggled`):

```rust
    /// Theme-derived colors (default text fg, bar background, and the four
    /// semantic colors) copied from the resolved theme at build time, so
    /// widgets and WASM guests can style consistently. `#[serde(default)]` keeps
    /// deserialization total across host/guest version skew (invariant #2).
    #[serde(default)]
    pub colors: crate::ThemeColors,
```

Update the `sample()` helper in `context.rs` `tests` to add `colors: Default::default(),` after `toggled: BTreeSet::new(),`.

- [ ] **Step 4: Fix every other `Context` literal.** Run `grep -rln --include='*.rs' 'toggled:' crates` and, in each `Context { … }` literal, add `colors: Default::default(),` immediately after the `toggled: …,` line. The complete list (all test helpers except the one real constructor): `assemble.rs`, `widget.rs`, `widgets/cwd.rs`, `widgets/datetime.rs`, `widgets/hostname.rs`, `widgets/pane_id.rs`, `widgets/lan_ip.rs`, `widgets/tailscale_ip.rs`, `widgets/toggle.rs` (2 literals), `widgets/windows.rs`, `widgets/loadavg.rs`, `widgets/mod.rs`, `widgets/memory.rs`, `widgets/battery.rs`, `widgets/cpu.rs`, `crates/rustline-wasm/src/host.rs` (2 literals), `crates/rustline-wasm/tests/e2e.rs`.

Note: `plugins/weather/src/lib.rs` has its **own** local deserialization struct, not `rustline_core::Context` — do **not** modify it (serde ignores the extra JSON key).

- [ ] **Step 5: Thread the theme into build_context.** In `crates/rustline/src/build_context.rs`:
  - Add `Theme` to the import: `use rustline_core::{Context, NetIface, Theme, WindowCtx};`
  - Change `build_region_context` signature to `pub fn build_region_context(args: &RegionArgs, layout: &[String], theme: &Theme) -> Context` and add, after `toggled: crate::toggles::read_toggles(),`:

```rust
        colors: theme.colors(),
```

  - Change `build_window_context` to `pub fn build_window_context(args: &WindowArgs, theme: &Theme) -> Context` and update its internal call to `build_region_context(&RegionArgs::default(), &[], theme)`.
  - Update this file's own tests: each `build_region_context(&RegionArgs::default(), &[])` call becomes `build_region_context(&RegionArgs::default(), &[], &Theme::default())`; the `build_window_context(&WindowArgs{…})` call gets `, &Theme::default()`.

- [ ] **Step 6: Update main.rs call sites.** In `crates/rustline/src/main.rs`, the three render arms call build_context; pass `&theme`:
  - `Render::Left`/`Render::Right`: `let ctx = build_region_context(&args, &cfg.layout.left, &theme);` (and `.right` respectively).
  - `Render::Window`: `let ctx = build_window_context(&args, &theme);`

- [ ] **Step 7: Run tests + build the binary**

Run: `cargo test -p rustline-core && cargo build -p rustline`
Expected: PASS / builds (all literals fixed).

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(core): Context.colors from resolved theme; thread theme into build_context"
```

---

### Task 4: `ThemeConfig` full mirror + `apply_to`/`from_theme`/`to_theme_over`

Extends `ThemeConfig` to a complete optional mirror of `Theme` plus `base`, and refactors theme resolution around a reusable merge. `base` is parsed but not yet *resolved* (Task 5 adds built-in resolution).

**Files:**
- Modify: `crates/rustline-core/src/config.rs` (`ThemeConfig` ~291-311, `to_theme` ~453-483, add helpers + tests)
- Modify: `crates/rustline-core/src/lib.rs:13` (re-export `ThemeConfig`)

**Interfaces:**
- Produces: `ThemeConfig` fields `base, hard_left, hard_right, soft_left, soft_right, soft_fg, success, info, warning, error` (all `Option`, `#[serde(default)]`); `ThemeConfig::apply_to(&self, &mut Theme)`; `ThemeConfig::from_theme(&Theme) -> ThemeConfig`; `Config::to_theme_over(&self, Theme) -> Theme`; `Config::to_theme(&self) -> Theme` (unchanged signature).

- [ ] **Step 1: Write failing tests** — add to `config.rs` `tests`:

```rust
    #[test]
    fn theme_config_full_mirror_apply_and_from_theme_round_trip() {
        use crate::Color;
        // apply_to sets only Some fields, leaving others at the base value.
        let mut cfg = ThemeConfig::default();
        cfg.error = Some(Color::Rgb(9, 9, 9));
        cfg.soft_fg = Some(Color::Indexed(99));
        let mut t = crate::Theme::default();
        cfg.apply_to(&mut t);
        assert_eq!(t.error, Color::Rgb(9, 9, 9));
        assert_eq!(t.soft_fg, Color::Indexed(99));
        assert_eq!(t.fg, crate::Theme::default().fg); // untouched

        // from_theme is all-Some and round-trips through apply_to onto default.
        let src = crate::Theme::default();
        let mirror = ThemeConfig::from_theme(&src);
        assert!(mirror.palette.is_some() && mirror.warning.is_some() && mirror.hard_left.is_some());
        let mut rebuilt = crate::Theme::default();
        mirror.apply_to(&mut rebuilt);
        assert_eq!(rebuilt.warning, src.warning);
        assert_eq!(rebuilt.win_current_bg, src.win_current_bg);
    }

    #[test]
    fn to_theme_over_applies_inline_overrides_onto_base() {
        use crate::Color;
        let mut cfg = Config::default();
        cfg.theme.error = Some(Color::Rgb(1, 2, 3));
        let mut base = crate::Theme::default();
        base.fg = Color::Indexed(200);
        base.error = Color::Indexed(160);
        let t = cfg.to_theme_over(base);
        assert_eq!(t.fg, Color::Indexed(200)); // from base, no inline override
        assert_eq!(t.error, Color::Rgb(1, 2, 3)); // inline override wins
    }

    #[test]
    fn theme_config_parses_base_separators_and_semantics() {
        let toml = r#"
[theme]
base = "nord"
soft_fg = { Indexed = 99 }
error = { Named = "red" }
hard_left = "X"
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.theme.base.as_deref(), Some("nord"));
        assert_eq!(c.theme.soft_fg, Some(crate::Color::Indexed(99)));
        assert_eq!(c.theme.error, Some(crate::Color::Named("red".into())));
        assert_eq!(c.theme.hard_left.as_deref(), Some("X"));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rustline-core theme_config_full_mirror`
Expected: FAIL — no `base`/`apply_to`/`from_theme`/`to_theme_over`.

- [ ] **Step 3: Extend `ThemeConfig`.** Replace the struct body (keep `#[derive(...)]` line) with the full mirror:

```rust
pub struct ThemeConfig {
    /// Name of a base theme to start from (a built-in, or a `*.toml` stem in the
    /// themes dir). Only meaningful in the main config's `[theme]`; ignored
    /// inside a theme file. Resolution is done by the binary (themes-dir first,
    /// then built-ins); core's `to_theme` resolves built-ins only.
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub palette: Option<Vec<Color>>,
    #[serde(default)]
    pub fg: Option<Color>,
    #[serde(default)]
    pub bar_bg: Option<Color>,
    #[serde(default)]
    pub hard_left: Option<String>,
    #[serde(default)]
    pub hard_right: Option<String>,
    #[serde(default)]
    pub soft_left: Option<String>,
    #[serde(default)]
    pub soft_right: Option<String>,
    #[serde(default)]
    pub soft_fg: Option<Color>,
    #[serde(default)]
    pub win_cap_left: Option<String>,
    #[serde(default)]
    pub win_cap_right: Option<String>,
    #[serde(default)]
    pub win_current_bg: Option<Color>,
    #[serde(default)]
    pub win_current_fg: Option<Color>,
    #[serde(default)]
    pub win_inactive_bg: Option<Color>,
    #[serde(default)]
    pub win_inactive_fg: Option<Color>,
    #[serde(default)]
    pub success: Option<Color>,
    #[serde(default)]
    pub info: Option<Color>,
    #[serde(default)]
    pub warning: Option<Color>,
    #[serde(default)]
    pub error: Option<Color>,
}
```

- [ ] **Step 4: Add `apply_to`, `from_theme`, and refactor `to_theme`.** Add an `impl ThemeConfig` block (near the struct) and replace `Config::to_theme`:

```rust
impl ThemeConfig {
    /// Apply each `Some` field onto `theme`, leaving unset fields unchanged.
    /// `base` is a selector, not a color, so it is not applied here.
    pub fn apply_to(&self, theme: &mut Theme) {
        macro_rules! set {
            ($field:ident) => {
                if let Some(v) = &self.$field {
                    theme.$field = v.clone();
                }
            };
        }
        set!(palette);
        set!(fg);
        set!(bar_bg);
        set!(hard_left);
        set!(hard_right);
        set!(soft_left);
        set!(soft_right);
        set!(soft_fg);
        set!(win_cap_left);
        set!(win_cap_right);
        set!(win_current_bg);
        set!(win_current_fg);
        set!(win_inactive_bg);
        set!(win_inactive_fg);
        set!(success);
        set!(info);
        set!(warning);
        set!(error);
    }

    /// An all-`Some` config mirroring `theme` (with `base = None`). Used to
    /// scaffold a fully-populated theme file (`rustline theme new`).
    pub fn from_theme(theme: &Theme) -> ThemeConfig {
        ThemeConfig {
            base: None,
            palette: Some(theme.palette.clone()),
            fg: Some(theme.fg.clone()),
            bar_bg: Some(theme.bar_bg.clone()),
            hard_left: Some(theme.hard_left.clone()),
            hard_right: Some(theme.hard_right.clone()),
            soft_left: Some(theme.soft_left.clone()),
            soft_right: Some(theme.soft_right.clone()),
            soft_fg: Some(theme.soft_fg.clone()),
            win_cap_left: Some(theme.win_cap_left.clone()),
            win_cap_right: Some(theme.win_cap_right.clone()),
            win_current_bg: Some(theme.win_current_bg.clone()),
            win_current_fg: Some(theme.win_current_fg.clone()),
            win_inactive_bg: Some(theme.win_inactive_bg.clone()),
            win_inactive_fg: Some(theme.win_inactive_fg.clone()),
            success: Some(theme.success.clone()),
            info: Some(theme.info.clone()),
            warning: Some(theme.warning.clone()),
            error: Some(theme.error.clone()),
        }
    }
}
```

Replace `Config::to_theme` with:

```rust
    /// Apply this config's inline `[theme]` overrides on top of an
    /// already-resolved `base` theme.
    pub fn to_theme_over(&self, base: Theme) -> Theme {
        let mut theme = base;
        self.theme.apply_to(&mut theme);
        theme
    }

    /// Resolve the effective theme using BUILT-IN themes only (no themes-dir
    /// lookup). Callers with a themes dir (the binary) resolve the base
    /// themselves and use `to_theme_over`.
    pub fn to_theme(&self) -> Theme {
        // Task 5 wires the built-in `base` here; until then, default base.
        self.to_theme_over(Theme::default())
    }
```

Update the existing `to_theme_maps_window_pill_overrides` test to also set + assert the new keys — add before `let t = cfg.to_theme();`:

```rust
        cfg.theme.soft_fg = Some(Color::Indexed(77));
        cfg.theme.error = Some(Color::Indexed(88));
```

and after the existing asserts:

```rust
        assert_eq!(t.soft_fg, Color::Indexed(77));
        assert_eq!(t.error, Color::Indexed(88));
```

Add `ThemeConfig` to the core re-export in `crates/rustline-core/src/lib.rs:13`:

```rust
pub use config::{Config, LogConfig, PluginConfig, ThemeConfig};
```

- [ ] **Step 5: Run tests + lint**

Run: `cargo test -p rustline-core config:: && cargo clippy -p rustline-core --all-targets -- -D warnings`
Expected: PASS / clean.

- [ ] **Step 6: Commit**

```bash
git add crates/rustline-core/src/config.rs crates/rustline-core/src/lib.rs
git commit -m "feat(core): ThemeConfig full mirror + apply_to/from_theme/to_theme_over"
```

---

### Task 5: Built-in theme registry (`themes.rs`) + `to_theme` base resolution

**Files:**
- Create: `crates/rustline-core/src/themes.rs`
- Modify: `crates/rustline-core/src/lib.rs` (add `pub mod themes;` + re-export)
- Modify: `crates/rustline-core/src/config.rs` (`to_theme` resolves a built-in `base`)

**Interfaces:**
- Produces: `rustline_core::builtin_theme(name: &str) -> Option<Theme>`; `rustline_core::builtin_theme_names() -> &'static [&'static str]`.

- [ ] **Step 1: Write failing tests** — create `crates/rustline-core/src/themes.rs` with a `tests` module first (implementation stubbed to compile against in Step 3):

```rust
//! Built-in named themes. Each is a complete `Theme`; non-`default` themes are
//! multi-accent (palette length >= 4). Curated schemes use truecolor (RGB).
//! Every theme's `fg` is chosen to contrast with its palette accents; the
//! threshold alert badge (widgets) always uses `bar_bg` as its text color, so
//! semantic colors only need to be brighter than `bar_bg`.

use crate::{Color, Theme};

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

/// Look up a built-in theme by name; `None` for an unknown name.
pub fn builtin_theme(name: &str) -> Option<Theme> {
    Some(match name {
        "default" => Theme::default(),
        "pastel-rainbow" => pastel_rainbow(),
        "nord" => nord(),
        "gruvbox" => gruvbox(),
        "catppuccin-mocha" => catppuccin_mocha(),
        "tokyo-night" => tokyo_night(),
        _ => return None,
    })
}

/// The built-in theme names, in display order.
pub fn builtin_theme_names() -> &'static [&'static str] {
    &[
        "default",
        "pastel-rainbow",
        "nord",
        "gruvbox",
        "catppuccin-mocha",
        "tokyo-night",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_resolves_and_unknown_is_none() {
        for name in builtin_theme_names() {
            assert!(builtin_theme(name).is_some(), "missing built-in: {name}");
        }
        assert!(builtin_theme("nope").is_none());
        assert_eq!(builtin_theme_names().len(), 6);
    }

    #[test]
    fn non_default_themes_are_multi_accent_and_distinct() {
        for name in builtin_theme_names().iter().filter(|n| **n != "default") {
            let t = builtin_theme(name).unwrap();
            assert!(t.palette.len() >= 4, "{name} not multi-accent");
            // Not accidentally the default theme (a real override happened).
            assert_ne!(t.bar_bg, Theme::default().bar_bg, "{name} == default bg");
        }
    }

    #[test]
    fn themes_keep_default_separators() {
        // Curated themes inherit the powerline separators/caps from default.
        let t = nord();
        assert_eq!(t.hard_left, Theme::default().hard_left);
        assert_eq!(t.win_cap_left, Theme::default().win_cap_left);
    }
}
```

- [ ] **Step 2: Register the module + re-export.** In `crates/rustline-core/src/lib.rs`, add `pub mod themes;` (after `pub mod segment;`) and add to the re-exports:

```rust
pub use themes::{builtin_theme, builtin_theme_names};
```

Run: `cargo test -p rustline-core themes::`
Expected: FAIL — `pastel_rainbow`/`nord`/… not defined.

- [ ] **Step 3: Implement the six theme constructors** in `themes.rs` (between `rgb` and the `tests` module). Each inherits separators/caps via `..Theme::default()`:

```rust
fn pastel_rainbow() -> Theme {
    Theme {
        palette: vec![
            rgb(244, 166, 184),
            rgb(246, 199, 169),
            rgb(245, 230, 163),
            rgb(184, 230, 196),
            rgb(169, 211, 240),
            rgb(208, 189, 240),
        ],
        fg: rgb(43, 43, 58),
        bar_bg: rgb(42, 42, 54),
        soft_fg: rgb(122, 122, 138),
        win_current_bg: rgb(195, 169, 238),
        win_current_fg: rgb(43, 43, 58),
        win_inactive_bg: rgb(207, 207, 218),
        win_inactive_fg: rgb(85, 85, 106),
        success: rgb(168, 224, 176),
        info: rgb(169, 211, 240),
        warning: rgb(243, 217, 139),
        error: rgb(242, 161, 161),
        ..Theme::default()
    }
}

fn nord() -> Theme {
    Theme {
        palette: vec![
            rgb(94, 129, 172),
            rgb(129, 161, 193),
            rgb(163, 190, 140),
            rgb(180, 142, 173),
            rgb(208, 135, 112),
        ],
        fg: rgb(216, 222, 233),
        bar_bg: rgb(46, 52, 64),
        soft_fg: rgb(76, 86, 106),
        win_current_bg: rgb(136, 192, 208),
        win_current_fg: rgb(46, 52, 64),
        win_inactive_bg: rgb(59, 66, 82),
        win_inactive_fg: rgb(216, 222, 233),
        success: rgb(163, 190, 140),
        info: rgb(136, 192, 208),
        warning: rgb(235, 203, 139),
        error: rgb(191, 97, 106),
        ..Theme::default()
    }
}

fn gruvbox() -> Theme {
    Theme {
        palette: vec![
            rgb(215, 153, 33),
            rgb(152, 151, 26),
            rgb(69, 133, 136),
            rgb(177, 98, 134),
            rgb(214, 93, 14),
        ],
        fg: rgb(235, 219, 178),
        bar_bg: rgb(40, 40, 40),
        soft_fg: rgb(102, 92, 84),
        win_current_bg: rgb(250, 189, 47),
        win_current_fg: rgb(40, 40, 40),
        win_inactive_bg: rgb(60, 56, 54),
        win_inactive_fg: rgb(235, 219, 178),
        success: rgb(184, 187, 38),
        info: rgb(131, 165, 152),
        warning: rgb(250, 189, 47),
        error: rgb(251, 73, 52),
        ..Theme::default()
    }
}

fn catppuccin_mocha() -> Theme {
    Theme {
        palette: vec![
            rgb(137, 180, 250),
            rgb(245, 194, 231),
            rgb(166, 227, 161),
            rgb(249, 226, 175),
            rgb(250, 179, 135),
        ],
        fg: rgb(17, 17, 27),
        bar_bg: rgb(30, 30, 46),
        soft_fg: rgb(88, 91, 112),
        win_current_bg: rgb(203, 166, 247),
        win_current_fg: rgb(17, 17, 27),
        win_inactive_bg: rgb(49, 50, 68),
        win_inactive_fg: rgb(205, 214, 244),
        success: rgb(166, 227, 161),
        info: rgb(137, 220, 235),
        warning: rgb(249, 226, 175),
        error: rgb(243, 139, 168),
        ..Theme::default()
    }
}

fn tokyo_night() -> Theme {
    Theme {
        palette: vec![
            rgb(122, 162, 247),
            rgb(187, 154, 247),
            rgb(125, 207, 255),
            rgb(158, 206, 106),
            rgb(224, 175, 104),
        ],
        fg: rgb(22, 22, 30),
        bar_bg: rgb(26, 27, 38),
        soft_fg: rgb(86, 95, 137),
        win_current_bg: rgb(122, 162, 247),
        win_current_fg: rgb(22, 22, 30),
        win_inactive_bg: rgb(41, 46, 66),
        win_inactive_fg: rgb(192, 202, 245),
        success: rgb(158, 206, 106),
        info: rgb(125, 207, 255),
        warning: rgb(224, 175, 104),
        error: rgb(247, 118, 142),
        ..Theme::default()
    }
}
```

- [ ] **Step 4: Wire built-in base into `to_theme`.** In `config.rs`, replace the placeholder body of `to_theme` (from Task 4) with real resolution and add a test:

```rust
    pub fn to_theme(&self) -> Theme {
        let base = self
            .theme
            .base
            .as_deref()
            .and_then(crate::builtin_theme)
            .unwrap_or_default();
        self.to_theme_over(base)
    }
```

Add to `config.rs` `tests`:

```rust
    #[test]
    fn to_theme_resolves_builtin_base_and_inline_override_wins() {
        use crate::Color;
        // base only
        let mut cfg = Config::default();
        cfg.theme.base = Some("nord".into());
        let t = cfg.to_theme();
        assert_eq!(t, {
            let n = crate::builtin_theme("nord").unwrap();
            n
        }); // no inline overrides -> exactly nord
        // base + override
        cfg.theme.error = Some(Color::Rgb(1, 2, 3));
        let t = cfg.to_theme();
        assert_eq!(t.error, Color::Rgb(1, 2, 3));
        assert_eq!(t.fg, crate::builtin_theme("nord").unwrap().fg);
        // unknown base -> default (total)
        let mut bad = Config::default();
        bad.theme.base = Some("nope".into());
        assert_eq!(bad.to_theme().bar_bg, crate::Theme::default().bar_bg);
    }
```

Note: this `assert_eq!` on `Theme` requires `Theme: PartialEq`. Add `PartialEq` to `Theme`'s derive in `render.rs` (`#[derive(Clone, Debug, PartialEq)]`). `Color`/`String`/`Vec` are all `PartialEq`, so this derives cleanly.

- [ ] **Step 5: Run tests + fmt + lint**

Run: `cargo fmt --all && cargo test -p rustline-core && cargo clippy -p rustline-core --all-targets -- -D warnings`
Expected: PASS / clean.

- [ ] **Step 6: Commit**

```bash
git add crates/rustline-core/src/themes.rs crates/rustline-core/src/lib.rs crates/rustline-core/src/config.rs crates/rustline-core/src/render.rs
git commit -m "feat(core): six built-in themes + to_theme base resolution"
```

---

### Task 6: Alert helper (`widgets/alert.rs`) + threshold config knobs

**Files:**
- Create: `crates/rustline-core/src/widgets/alert.rs`
- Modify: `crates/rustline-core/src/widgets/mod.rs` (add `mod alert;` + re-export)
- Modify: `crates/rustline-core/src/config.rs` (`CpuOpts`, `MemoryOpts`, `BatteryOpts`, `LoadAvgOpts` + defaults + tests)

**Interfaces:**
- Produces: `widgets::alert::{AlertKind, alert_over, alert_under, alert_style}`; config fields `cpu.warn_percent/crit_percent`, `memory.warn_percent/crit_percent`, `battery.warn_percent/crit_percent`, `loadavg.warn_load/crit_load`.
- `AlertKind::None|Warn|Crit`; `alert_over(value: f64, warn: f64, crit: f64) -> AlertKind` (higher is worse); `alert_under(value: f64, warn: f64, crit: f64) -> AlertKind` (lower is worse); `alert_style(kind, colors: &ThemeColors) -> Option<Style>` (badge: `bg=semantic, fg=bar_bg, bold`). A threshold `<= 0.0` disables that tier.

- [ ] **Step 1: Write failing tests** — create `alert.rs` with tests:

```rust
//! Shared threshold-alert helper for the numeric widgets (cpu/memory/battery/
//! loadavg). A crossed threshold turns the widget's cell into an inverse alert
//! badge: `bg = <semantic color>`, `fg = bar_bg` (dark in every theme, so the
//! badge always contrasts), `bold`. A threshold of `0` (or less) disables that
//! tier — so a widget with both tiers at `0` renders byte-identically to before.

use crate::{Style, ThemeColors};

/// Which alert tier a reading falls in. `Crit` outranks `Warn`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AlertKind {
    None,
    Warn,
    Crit,
}

/// "Higher is worse" (cpu %, memory %, load1): `value >= crit` -> `Crit`,
/// `value >= warn` -> `Warn`. A tier threshold `<= 0` is disabled.
pub(crate) fn alert_over(value: f64, warn: f64, crit: f64) -> AlertKind {
    if crit > 0.0 && value >= crit {
        AlertKind::Crit
    } else if warn > 0.0 && value >= warn {
        AlertKind::Warn
    } else {
        AlertKind::None
    }
}

/// "Lower is worse" (battery %): `value <= crit` -> `Crit`, `value <= warn` ->
/// `Warn`. A tier threshold `<= 0` is disabled.
pub(crate) fn alert_under(value: f64, warn: f64, crit: f64) -> AlertKind {
    if crit > 0.0 && value <= crit {
        AlertKind::Crit
    } else if warn > 0.0 && value <= warn {
        AlertKind::Warn
    } else {
        AlertKind::None
    }
}

/// The alert badge style for `kind`, or `None` when not alerting (leaving the
/// segment to normal palette assignment).
pub(crate) fn alert_style(kind: AlertKind, colors: &ThemeColors) -> Option<Style> {
    let bg = match kind {
        AlertKind::None => return None,
        AlertKind::Warn => colors.warning.clone(),
        AlertKind::Crit => colors.error.clone(),
    };
    Some(Style {
        fg: Some(colors.bar_bg.clone()),
        bg: Some(bg),
        bold: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Color;

    #[test]
    fn over_thresholds_and_disabled_tiers() {
        assert_eq!(alert_over(50.0, 80.0, 95.0), AlertKind::None);
        assert_eq!(alert_over(80.0, 80.0, 95.0), AlertKind::Warn); // boundary inclusive
        assert_eq!(alert_over(95.0, 80.0, 95.0), AlertKind::Crit); // crit beats warn
        assert_eq!(alert_over(99.0, 0.0, 0.0), AlertKind::None); // both disabled
        assert_eq!(alert_over(99.0, 0.0, 95.0), AlertKind::Crit); // warn off, crit on
    }

    #[test]
    fn under_thresholds_for_battery() {
        assert_eq!(alert_under(50.0, 20.0, 10.0), AlertKind::None);
        assert_eq!(alert_under(20.0, 20.0, 10.0), AlertKind::Warn);
        assert_eq!(alert_under(10.0, 20.0, 10.0), AlertKind::Crit);
        assert_eq!(alert_under(5.0, 0.0, 0.0), AlertKind::None); // disabled
    }

    #[test]
    fn style_uses_semantic_bg_and_bar_bg_fg_bold() {
        let colors = ThemeColors {
            warning: Color::Indexed(214),
            error: Color::Indexed(196),
            bar_bg: Color::Indexed(234),
            ..ThemeColors::default()
        };
        assert_eq!(alert_style(AlertKind::None, &colors), None);
        let warn = alert_style(AlertKind::Warn, &colors).unwrap();
        assert_eq!(warn.bg, Some(Color::Indexed(214)));
        assert_eq!(warn.fg, Some(Color::Indexed(234)));
        assert!(warn.bold);
        let crit = alert_style(AlertKind::Crit, &colors).unwrap();
        assert_eq!(crit.bg, Some(Color::Indexed(196)));
    }
}
```

- [ ] **Step 2: Register the module.** In `widgets/mod.rs` add `mod alert;` (near `mod bar;`) and, after the `use ... toggle::{...}` line:

```rust
pub(crate) use alert::{AlertKind, alert_over, alert_style, alert_under};
```

Run: `cargo test -p rustline-core alert::`
Expected: PASS (helper is self-contained).

- [ ] **Step 3: Add threshold config knobs + tests.** In `config.rs`, add fields to the four opts structs and their `Default` impls, with default fns:

```rust
fn default_cpu_warn() -> f64 { 80.0 }
fn default_cpu_crit() -> f64 { 95.0 }
fn default_mem_warn() -> f64 { 80.0 }
fn default_mem_crit() -> f64 { 92.0 }
fn default_bat_warn() -> f64 { 20.0 }
fn default_bat_crit() -> f64 { 10.0 }
```

Add to `CpuOpts` (fields + Default): `#[serde(default = "default_cpu_warn")] pub warn_percent: f64,` and `#[serde(default = "default_cpu_crit")] pub crit_percent: f64,`. Same shape for `MemoryOpts` (`default_mem_warn`/`default_mem_crit`), `BatteryOpts` (`default_bat_warn`/`default_bat_crit`). For `LoadAvgOpts` add `#[serde(default)] pub warn_load: f64,` and `#[serde(default)] pub crit_load: f64,` (default `0.0` = off; `f64::default()` is `0.0`, so plain `#[serde(default)]` works — set them to `0.0` in the `Default` impl).

Update each struct's `Default` impl to set the new fields (e.g. `warn_percent: default_cpu_warn(), crit_percent: default_cpu_crit(),`; loadavg `warn_load: 0.0, crit_load: 0.0,`).

Add tests to `config.rs`:

```rust
    #[test]
    fn threshold_knobs_default_and_parse() {
        let c = Config::default();
        assert_eq!(c.widgets.cpu.warn_percent, 80.0);
        assert_eq!(c.widgets.cpu.crit_percent, 95.0);
        assert_eq!(c.widgets.memory.crit_percent, 92.0);
        assert_eq!(c.widgets.battery.warn_percent, 20.0);
        assert_eq!(c.widgets.loadavg.warn_load, 0.0); // off by default
        let parsed: Config =
            toml::from_str("[widgets.cpu]\nwarn_percent = 70\n[widgets.loadavg]\ncrit_load = 8.0\n")
                .unwrap();
        assert_eq!(parsed.widgets.cpu.warn_percent, 70.0);
        assert_eq!(parsed.widgets.cpu.crit_percent, 95.0); // untouched default
        assert_eq!(parsed.widgets.loadavg.crit_load, 8.0);
    }
```

- [ ] **Step 4: Run tests + fmt**

Run: `cargo fmt --all && cargo test -p rustline-core alert:: config::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rustline-core/src/widgets/alert.rs crates/rustline-core/src/widgets/mod.rs crates/rustline-core/src/config.rs
git commit -m "feat(core): threshold alert helper + per-widget warn/crit knobs"
```

---

### Task 7: `cpu` threshold wiring

**Files:**
- Modify: `crates/rustline-core/src/widgets/cpu.rs`
- Modify: `crates/rustline-core/src/widgets/mod.rs` (pass `warn_percent`/`crit_percent` into `CpuWidget`)

**Interfaces:**
- Consumes: `alert_over`, `alert_style` (Task 6); `Context.colors` (Task 3); `cpu.warn_percent/crit_percent` (Task 6).
- Produces: `CpuWidget` gains `pub warn_percent: f64, pub crit_percent: f64`.

- [ ] **Step 1: Write failing tests** — add to `cpu.rs` `tests` (update the `w`/`w2` constructors to set the new fields, `warn_percent: 80.0, crit_percent: 95.0,`), then:

```rust
    #[test]
    fn below_threshold_is_plain_segment() {
        // Characterization: no alert -> default (unstyled) segment, as before.
        let out = w("{percent}%", "").render(&ctx(Some(CpuUsage { percent: 50.0 })));
        assert_eq!(out[0].text, "50%");
        assert_eq!(out[0].style, crate::Style::default());
    }

    #[test]
    fn warn_and_crit_apply_badge_style() {
        let mut c = ctx(Some(CpuUsage { percent: 85.0 }));
        c.colors = crate::ThemeColors {
            warning: crate::Color::Indexed(214),
            error: crate::Color::Indexed(196),
            bar_bg: crate::Color::Indexed(234),
            ..Default::default()
        };
        let out = w("{percent}%", "").render(&c);
        assert_eq!(out[0].style.bg, Some(crate::Color::Indexed(214))); // warn
        assert_eq!(out[0].style.fg, Some(crate::Color::Indexed(234)));
        assert!(out[0].style.bold);

        c.cpu = Some(CpuUsage { percent: 96.0 });
        let out = w("{percent}%", "").render(&c);
        assert_eq!(out[0].style.bg, Some(crate::Color::Indexed(196))); // crit
    }

    #[test]
    fn thresholds_disabled_never_alert() {
        let mut c = ctx(Some(CpuUsage { percent: 100.0 }));
        c.colors = crate::ThemeColors::default();
        let mut widget = w("{percent}%", "");
        widget.warn_percent = 0.0;
        widget.crit_percent = 0.0;
        let out = widget.render(&c);
        assert_eq!(out[0].style, crate::Style::default());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rustline-core cpu::`
Expected: FAIL — no `warn_percent` field / style default mismatch.

- [ ] **Step 3: Implement.** In `cpu.rs`, add the two fields to `CpuWidget`:

```rust
    pub warn_percent: f64,
    pub crit_percent: f64,
```

Replace the `Some(c) => { … vec![Segment::new(text)] }` arm body so it builds a styled segment when alerting:

```rust
            Some(c) => {
                let percent = c.percent.round() as u64;
                let fmt =
                    crate::widgets::active_format(ctx, Self::NAME, &self.format, &self.alt_format);
                let text = fmt
                    .replace("{percent}", &percent.to_string())
                    .replace(
                        "{bar}",
                        &bar::gauge_bar(c.percent as f64 / 100.0, self.bar_width),
                    )
                    .replace("{icon}", CPU_ICON);
                let kind = crate::widgets::alert_over(
                    c.percent as f64,
                    self.warn_percent,
                    self.crit_percent,
                );
                match crate::widgets::alert_style(kind, &ctx.colors) {
                    Some(style) => vec![Segment::styled(text, style)],
                    None => vec![Segment::new(text)],
                }
            }
```

In `widgets/mod.rs`, extend the `cpu` registration closure with the two fields:

```rust
                Box::new(CpuWidget {
                    format: cpu.format.clone(),
                    alt_format: cpu.alt_format.clone(),
                    down_format: cpu.down_format.clone(),
                    bar_width: cpu.bar_width,
                    warn_percent: cpu.warn_percent,
                    crit_percent: cpu.crit_percent,
                })
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p rustline-core cpu:: widgets::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rustline-core/src/widgets/cpu.rs crates/rustline-core/src/widgets/mod.rs
git commit -m "feat(cpu): threshold alert badge from warn/crit percent"
```

---

### Task 8: `memory` threshold wiring

**Files:**
- Modify: `crates/rustline-core/src/widgets/memory.rs`
- Modify: `crates/rustline-core/src/widgets/mod.rs`

**Interfaces:**
- Produces: `MemoryWidget` gains `pub warn_percent: f64, pub crit_percent: f64`. Alert compares the used-percentage (`fraction * 100`).

- [ ] **Step 1: Write failing tests** — update `memory.rs` `w()` constructor to add `warn_percent: 80.0, crit_percent: 92.0,`, then add:

```rust
    #[test]
    fn below_threshold_plain_over_threshold_badge() {
        let g = 1024u64.pow(3);
        // 8/16 = 50% -> plain
        let out = w("{percent}%", "").render(&ctx(mem(16 * g, 8 * g, 8 * g)));
        assert_eq!(out[0].style, crate::Style::default());
        // 15/16 ~= 94% -> crit
        let mut c = ctx(mem(16 * g, 15 * g, 1 * g));
        c.colors = crate::ThemeColors {
            error: crate::Color::Indexed(196),
            warning: crate::Color::Indexed(214),
            bar_bg: crate::Color::Indexed(234),
            ..Default::default()
        };
        let out = w("{percent}%", "").render(&c);
        assert_eq!(out[0].style.bg, Some(crate::Color::Indexed(196)));
        assert!(out[0].style.bold);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rustline-core memory::`
Expected: FAIL.

- [ ] **Step 3: Implement.** Add the two fields to `MemoryWidget`. In the `Some(m) =>` arm, after computing `fraction`/`percent`/`text`, replace the `vec![Segment::new(text)]` with:

```rust
                let kind = crate::widgets::alert_over(
                    fraction * 100.0,
                    self.warn_percent,
                    self.crit_percent,
                );
                match crate::widgets::alert_style(kind, &ctx.colors) {
                    Some(style) => vec![Segment::styled(text, style)],
                    None => vec![Segment::new(text)],
                }
```

In `widgets/mod.rs`, add `warn_percent: memory.warn_percent, crit_percent: memory.crit_percent,` to the `MemoryWidget { … }` closure.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rustline-core memory:: widgets::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rustline-core/src/widgets/memory.rs crates/rustline-core/src/widgets/mod.rs
git commit -m "feat(memory): threshold alert badge from used-percent"
```

---

### Task 9: `battery` threshold wiring (discharge-only)

**Files:**
- Modify: `crates/rustline-core/src/widgets/battery.rs`
- Modify: `crates/rustline-core/src/widgets/mod.rs`

**Interfaces:**
- Produces: `BatteryWidget` gains `pub warn_percent: f64, pub crit_percent: f64`. Alerts via `alert_under` **only when `state == Discharging`**.

- [ ] **Step 1: Write failing tests** — update the `battery.rs` `w()` helper (add `warn_percent: 20.0, crit_percent: 10.0,`) and the inline `BatteryWidget { … }` literals in that test module (there are several — add the two fields to each), then add:

```rust
    #[test]
    fn low_discharging_alerts_but_charging_does_not() {
        let mut c = ctx(bat(15, BatteryState::Discharging));
        c.colors = crate::ThemeColors {
            warning: crate::Color::Indexed(214),
            error: crate::Color::Indexed(196),
            bar_bg: crate::Color::Indexed(234),
            ..Default::default()
        };
        let out = w().render(&c); // 15% <= warn(20) -> warn
        assert_eq!(out[0].style.bg, Some(crate::Color::Indexed(214)));

        let mut c2 = ctx(bat(15, BatteryState::Charging));
        c2.colors = c.colors.clone();
        let out = w().render(&c2); // charging -> no alert
        assert_eq!(out[0].style, crate::Style::default());

        let mut c3 = ctx(bat(8, BatteryState::Discharging));
        c3.colors = c.colors.clone();
        let out = w().render(&c3); // 8% <= crit(10) -> crit
        assert_eq!(out[0].style.bg, Some(crate::Color::Indexed(196)));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rustline-core battery::`
Expected: FAIL.

- [ ] **Step 3: Implement.** Add the two fields to `BatteryWidget`. In the `Some(b) =>` arm, after building `text`, replace `vec![Segment::new(text)]` with:

```rust
                let kind = if b.state == BatteryState::Discharging {
                    crate::widgets::alert_under(
                        b.percent as f64,
                        self.warn_percent,
                        self.crit_percent,
                    )
                } else {
                    crate::widgets::AlertKind::None
                };
                match crate::widgets::alert_style(kind, &ctx.colors) {
                    Some(style) => vec![Segment::styled(text, style)],
                    None => vec![Segment::new(text)],
                }
```

In `widgets/mod.rs`, add `warn_percent: battery.warn_percent, crit_percent: battery.crit_percent,` to the `BatteryWidget { … }` closure.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rustline-core battery:: widgets::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rustline-core/src/widgets/battery.rs crates/rustline-core/src/widgets/mod.rs
git commit -m "feat(battery): low-battery alert badge (discharge-only)"
```

---

### Task 10: `loadavg` threshold wiring (off by default)

**Files:**
- Modify: `crates/rustline-core/src/widgets/loadavg.rs`
- Modify: `crates/rustline-core/src/widgets/mod.rs`

**Interfaces:**
- Produces: `LoadAvg` gains `pub warn_load: f64, pub crit_load: f64`. Alerts via `alert_over(load1, warn_load, crit_load)`; defaults `0.0` ⇒ off ⇒ byte-identical to today.

- [ ] **Step 1: Write failing tests** — update the `loadavg.rs` `w()` helper (add `warn_load: 0.0, crit_load: 0.0,`), then add:

```rust
    #[test]
    fn default_thresholds_off_no_style() {
        // Load-bearing: default (0/0) -> plain segment, unchanged output.
        let out = w("{load1}", "", "").render(&ctx_load(Some([9.9, 0.0, 0.0])));
        assert_eq!(out[0].text, "9.90");
        assert_eq!(out[0].style, crate::Style::default());
    }

    #[test]
    fn configured_thresholds_alert_on_load1() {
        let mut c = ctx_load(Some([6.0, 1.0, 1.0]));
        c.colors = crate::ThemeColors {
            warning: crate::Color::Indexed(214),
            error: crate::Color::Indexed(196),
            bar_bg: crate::Color::Indexed(234),
            ..Default::default()
        };
        let mut widget = w("{load1}", "", "");
        widget.warn_load = 4.0;
        widget.crit_load = 8.0;
        let out = widget.render(&c); // 6 >= warn(4) -> warn
        assert_eq!(out[0].style.bg, Some(crate::Color::Indexed(214)));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rustline-core loadavg::`
Expected: FAIL.

- [ ] **Step 3: Implement.** Add the two fields to `LoadAvg`. In the `Some(vals) =>` arm, replace `vec![Segment::new(substitute(fmt, Some(vals)))]` with:

```rust
                let text = substitute(fmt, Some(vals));
                let kind = crate::widgets::alert_over(vals[0], self.warn_load, self.crit_load);
                match crate::widgets::alert_style(kind, &ctx.colors) {
                    Some(style) => vec![Segment::styled(text, style)],
                    None => vec![Segment::new(text)],
                }
```

In `widgets/mod.rs`, add `warn_load: loadavg.warn_load, crit_load: loadavg.crit_load,` to the `LoadAvg { … }` closure.

- [ ] **Step 4: Run full core tests + lint**

Run: `cargo fmt --all && cargo test -p rustline-core && cargo clippy -p rustline-core --all-targets -- -D warnings`
Expected: PASS / clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rustline-core/src/widgets/loadavg.rs crates/rustline-core/src/widgets/mod.rs
git commit -m "feat(loadavg): optional load1 alert badge (off by default)"
```

---

### Task 11: CLI scaffolding — `theme` group, themes dir, base resolution, `theme list`

**Files:**
- Modify: `crates/rustline/src/cli.rs` (add `Theme(ThemeCmd)` + `ThemeCmd`)
- Create: `crates/rustline/src/theme_cmd.rs`
- Modify: `crates/rustline/src/main.rs` (add `mod theme_cmd;`, `themes_dir()`, `resolve_theme()`, `resolve_base_theme()`, dispatch `Command::Theme`, and switch `let theme = resolve_theme(&cfg);`)

**Interfaces:**
- Consumes: `builtin_theme`, `builtin_theme_names`, `ThemeConfig`, `Config::to_theme_over` (Tasks 4-5).
- Produces: `rustline theme list`; `theme_cmd::run(cmd, config_path, themes_dir)`; `main::themes_dir()`, `main::resolve_theme(&Config) -> Theme`.

- [ ] **Step 1: Write the failing test** — create `crates/rustline/tests/theme_cli.rs` (integration test; uses the built binary via `assert_cmd`? Check `grep assert_cmd crates/rustline/Cargo.toml` — if absent, test the library-style function instead). Prefer a unit test in `theme_cmd.rs` for `list`'s formatting. Add to `theme_cmd.rs` a testable pure formatter and test:

```rust
    #[test]
    fn list_lines_mark_active_and_shadowed() {
        // built-ins: default active; a "nord" file shadows the built-in nord.
        let files = vec!["nord".to_string(), "mine".to_string()];
        let lines = super::list_lines("pastel-rainbow", &files);
        assert!(lines.iter().any(|l| l.contains("pastel-rainbow") && l.contains('*')));
        assert!(lines.iter().any(|l| l.contains("nord") && l.contains("shadowed")));
        assert!(lines.iter().any(|l| l.contains("mine") && l.contains("file")));
    }
```

- [ ] **Step 2: Add the CLI types.** In `cli.rs`, add a variant to `Command`:

```rust
    /// List, preview, select, or scaffold themes.
    #[command(subcommand)]
    Theme(ThemeCmd),
```

and the subcommand enum:

```rust
/// Manage themes: list, preview, select, and scaffold new ones.
#[derive(Subcommand)]
pub enum ThemeCmd {
    /// List built-in and themes-dir themes (marks the active one).
    List,
    /// Print an ANSI colour preview of a theme.
    Show { name: String },
    /// Select a theme by writing `[theme].base` into the config file.
    Use { name: String },
    /// Scaffold a new tweakable theme file seeded from an existing theme.
    New {
        name: String,
        /// Seed theme to copy from (built-in or themes-dir stem). Default: `default`.
        #[arg(long, default_value = "default")]
        from: String,
        /// Overwrite an existing theme file.
        #[arg(long)]
        force: bool,
    },
}
```

- [ ] **Step 3: Implement `theme_cmd.rs` (list only for now).**

```rust
//! `rustline theme …` — list/preview/select/scaffold themes. Config mutations
//! (`use`) go through `toml_edit` so comments/formatting survive, mirroring
//! `plugin_cmd`.

use std::path::Path;

use rustline_core::builtin_theme_names;

use crate::cli::ThemeCmd;

/// Dispatch a `rustline theme …` invocation.
pub fn run(cmd: ThemeCmd, config_path: &Path, themes_dir: &Path) {
    match cmd {
        ThemeCmd::List => list(config_path, themes_dir),
        ThemeCmd::Show { name } => { let _ = (&name, themes_dir); /* Task 12 */ }
        ThemeCmd::Use { name } => { let _ = (&name, config_path); /* Task 13 */ }
        ThemeCmd::New { name, from, force } => { let _ = (&name, &from, force, themes_dir); /* Task 14 */ }
    }
}

/// Read the themes-dir `*.toml` stems (empty on any error).
fn theme_files(themes_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(themes_dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension()?.to_str()? == "toml")
                .then(|| p.file_stem()?.to_str().map(str::to_string))
                .flatten()
        })
        .collect();
    names.sort();
    names
}

/// Build the `list` output lines. `active` is the current base (or "default").
fn list_lines(active: &str, files: &[String]) -> Vec<String> {
    let mut lines = Vec::new();
    for name in builtin_theme_names() {
        let mark = if *name == active { " *" } else { "" };
        let shadowed = if files.iter().any(|f| f == name) {
            "  (shadowed by file)"
        } else {
            ""
        };
        lines.push(format!("{name}  (built-in){mark}{shadowed}"));
    }
    for f in files {
        let mark = if f == active { " *" } else { "" };
        lines.push(format!("{f}  (file){mark}"));
    }
    lines
}

fn list(config_path: &Path, themes_dir: &Path) {
    let cfg = rustline_core::Config::load(config_path);
    let active = cfg.theme.base.as_deref().unwrap_or("default");
    for line in list_lines(active, &theme_files(themes_dir)) {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    // (test from Step 1 goes here)
}
```

- [ ] **Step 4: Wire main.rs.** Add `mod theme_cmd;` to the module list. Add the two path/resolution helpers:

```rust
/// Resolve the themes dir: `$XDG_CONFIG_HOME/rustline/themes` (fallback
/// `~/.config/rustline/themes`), parallel to `config_path`.
fn themes_dir() -> PathBuf {
    let base = env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env::var("HOME").unwrap_or_default()).join(".config"));
    base.join("rustline").join("themes")
}

/// Resolve a base-theme name to a full `Theme`: a themes-dir `*.toml` file wins
/// over a same-named built-in (so a user file can shadow/override a built-in).
fn resolve_base_theme(name: &str) -> Option<rustline_core::Theme> {
    let file = themes_dir().join(format!("{name}.toml"));
    if let Ok(text) = std::fs::read_to_string(&file) {
        match toml::from_str::<rustline_core::ThemeConfig>(&text) {
            Ok(tc) => {
                let mut t = rustline_core::Theme::default();
                tc.apply_to(&mut t);
                return Some(t);
            }
            Err(e) => tracing::warn!("invalid theme file {}: {e}", file.display()),
        }
    }
    rustline_core::builtin_theme(name)
}

/// Resolve the effective theme: default → base (file-first, then built-in) →
/// inline `[theme]` overrides. An unresolvable base warns and falls back.
fn resolve_theme(cfg: &Config) -> rustline_core::Theme {
    let base = match cfg.theme.base.as_deref() {
        Some(name) => resolve_base_theme(name).unwrap_or_else(|| {
            tracing::warn!("unknown theme base {name:?}; using default");
            rustline_core::Theme::default()
        }),
        None => rustline_core::Theme::default(),
    };
    cfg.to_theme_over(base)
}
```

Replace `let theme = cfg.to_theme();` (line ~80) with `let theme = resolve_theme(&cfg);`. Add the dispatch arm:

```rust
        Command::Theme(cmd) => theme_cmd::run(cmd, &config_path(), &themes_dir()),
```

Also add `Theme` to the `rustline_core` import (`use rustline_core::{… Theme …};`) if the `resolve_*` helpers reference `rustline_core::Theme` unqualified — they use the fully-qualified path above, so no import change is strictly required. Keep it qualified for clarity.

- [ ] **Step 5: Run tests + build**

Run: `cargo test -p rustline theme && cargo build -p rustline`
Expected: PASS / builds. Manually: `cargo run -p rustline -- theme list` prints the six built-ins with `default *`.

- [ ] **Step 6: Commit**

```bash
git add crates/rustline/src/cli.rs crates/rustline/src/theme_cmd.rs crates/rustline/src/main.rs
git commit -m "feat(cli): rustline theme group + list + themes-dir base resolution"
```

---

### Task 12: `theme show` — ANSI preview

**Files:**
- Modify: `crates/rustline/src/theme_cmd.rs` (implement `show`)

**Interfaces:**
- Consumes: `resolve_base_theme`-equivalent lookup; `render_named_region`, `render_window`, `tmux_to_ansi`, `Registry::with_builtins`, `Config::default`.
- Produces: `rustline theme show <name>` prints a colored sample bar; exits non-zero on unknown name.

- [ ] **Step 1: Write the failing test** — add to `theme_cmd.rs` `tests`:

```rust
    #[test]
    fn preview_ansi_is_nonempty_and_colored_for_builtin() {
        let out = super::preview_ansi("nord").expect("known theme");
        assert!(out.contains('\u{1b}'), "contains ANSI escape: {out:?}");
        assert!(out.contains("RIGHT"), "labels the right region");
        assert!(super::preview_ansi("nope").is_none());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rustline preview_ansi`
Expected: FAIL — no `preview_ansi`.

- [ ] **Step 3: Implement.** Add to `theme_cmd.rs` (resolving name via built-in only for the pure/testable core; the file-shadow lookup is done by the caller passing the resolved theme — but for a self-contained testable unit, resolve built-ins here and let `show` also try the themes dir):

```rust
use rustline_core::{
    Config, Context, Direction, Registry, ThemeColors, Theme, WindowCtx, builtin_theme,
    render_named_region, render_window, tmux_to_ansi,
};
use chrono::Local;

/// A representative synthetic Context that trips warning+error badges so a
/// preview exercises the semantic colors. `colors` come from `theme`.
fn sample_context(theme: &Theme) -> Context {
    Context {
        session_name: "0".into(),
        window_index: "1".into(),
        pane_index: "0".into(),
        pane_current_path: "/home/steve/src/rustline".into(),
        home: "/home/steve".into(),
        hostname: "scadrial".into(),
        loadavg: Some([0.42, 0.31, 0.30]),
        now: Local::now(),
        window: None,
        interfaces: Vec::new(),
        battery: Some(rustline_core::Battery {
            percent: 15,
            state: rustline_core::BatteryState::Discharging,
        }),
        cpu: Some(rustline_core::CpuUsage { percent: 96.0 }),
        memory: Some(rustline_core::MemInfo {
            total_bytes: 16 * 1024u64.pow(3),
            used_bytes: 14 * 1024u64.pow(3),
            available_bytes: 2 * 1024u64.pow(3),
        }),
        os: "linux".into(),
        arch: "x86_64".into(),
        toggled: Default::default(),
        colors: theme.colors(),
    }
}

/// Render a labelled, ANSI-coloured preview of `name` (built-in), or `None` if
/// unknown. Uses the default layout + `battery` in the right region so the
/// badge colors show.
fn preview_ansi(name: &str) -> Option<String> {
    let theme = builtin_theme(name)?;
    let cfg = Config::default();
    let reg = Registry::with_builtins(&cfg);
    let mut ctx = sample_context(&theme);
    let right = vec![
        "cwd".to_string(),
        "cpu".to_string(),
        "memory".to_string(),
        "battery".to_string(),
        "loadavg".to_string(),
        "datetime".to_string(),
    ];
    let left = render_named_region(Direction::Left, &cfg.layout.left, &ctx, &reg, &theme);
    let right_out = render_named_region(Direction::Right, &right, &ctx, &reg, &theme);
    ctx.window = Some(WindowCtx {
        index: "1".into(),
        name: "shell".into(),
        flags: "*".into(),
        is_current: true,
    });
    let win_active = render_window(&ctx, &reg, &theme);
    ctx.window = Some(WindowCtx {
        index: "2".into(),
        name: "editor".into(),
        flags: String::new(),
        is_current: false,
    });
    let win_inactive = render_window(&ctx, &reg, &theme);
    Some(format!(
        "LEFT   : {}\nCENTER : {}{}\nRIGHT  : {}",
        tmux_to_ansi(&left),
        tmux_to_ansi(&win_active),
        tmux_to_ansi(&win_inactive),
        tmux_to_ansi(&right_out),
    ))
}
```

Note: `battery` is not in the default right layout, so the preview passes an explicit `right` list that includes it. The `colors: theme.colors()` line requires `theme.colors()` (Task 2). Remove the now-unused `ThemeColors` import if clippy flags it.

Wire `show` in `run`:

```rust
        ThemeCmd::Show { name } => show(&name, themes_dir),
```

and add `show`, which prefers a themes-dir file (parse → resolve) then falls back to `preview_ansi` for built-ins:

```rust
fn show(name: &str, themes_dir: &Path) {
    // A themes-dir file shadows a built-in of the same name.
    let file = themes_dir.join(format!("{name}.toml"));
    if let Ok(text) = std::fs::read_to_string(&file) {
        match toml::from_str::<rustline_core::ThemeConfig>(&text) {
            Ok(tc) => {
                let mut t = Theme::default();
                tc.apply_to(&mut t);
                print!("{}", preview_theme_ansi(&t));
                println!();
                return;
            }
            Err(e) => {
                eprintln!("invalid theme file {}: {e}", file.display());
                std::process::exit(1);
            }
        }
    }
    match preview_ansi(name) {
        Some(s) => {
            println!("{s}");
        }
        None => {
            eprintln!(
                "unknown theme: {name}\navailable: {}",
                builtin_theme_names().join(", ")
            );
            std::process::exit(1);
        }
    }
}
```

Refactor `preview_ansi` to delegate to a `preview_theme_ansi(&Theme) -> String` (so both a built-in and a file theme reuse the rendering). i.e. `fn preview_ansi(name) -> Option<String> { Some(preview_theme_ansi(&builtin_theme(name)?)) }` and move the render body into `preview_theme_ansi(theme: &Theme)`.

- [ ] **Step 4: Run tests + manual check**

Run: `cargo test -p rustline theme && cargo run -p rustline -- theme show pastel-rainbow`
Expected: PASS; a colored 3-line preview prints (needs a truecolor terminal + Nerd font for glyphs, but ANSI escapes are present regardless).

- [ ] **Step 5: Commit**

```bash
git add crates/rustline/src/theme_cmd.rs
git commit -m "feat(cli): rustline theme show — ANSI preview with alert badges"
```

---

### Task 13: `theme use` — write `[theme].base`

**Files:**
- Modify: `crates/rustline/src/theme_cmd.rs` (implement `use_theme`)

**Interfaces:**
- Produces: `rustline theme use <name>` sets `[theme].base = "<name>"` via `toml_edit` after validating the name resolves.

- [ ] **Step 1: Write the failing test** — add to `theme_cmd.rs` `tests`:

```rust
    #[test]
    fn set_base_preserves_comments_and_creates_theme_table() {
        use toml_edit::DocumentMut;
        let mut doc = "# my config\n[layout]\nright = [\"datetime\"]\n"
            .parse::<DocumentMut>()
            .unwrap();
        super::set_base(&mut doc, "nord");
        let s = doc.to_string();
        assert!(s.contains("# my config"), "comment preserved: {s}");
        assert!(s.contains("[theme]"), "theme table created: {s}");
        assert!(s.contains("base = \"nord\""), "base set: {s}");
        // idempotent overwrite
        super::set_base(&mut doc, "gruvbox");
        assert!(doc.to_string().contains("base = \"gruvbox\""));
        assert!(!doc.to_string().contains("nord"));
    }

    #[test]
    fn resolvable_accepts_builtin_and_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("mine.toml"), "fg = { Indexed = 1 }\n").unwrap();
        assert!(super::resolvable("nord", tmp.path()));
        assert!(super::resolvable("mine", tmp.path()));
        assert!(!super::resolvable("nope", tmp.path()));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rustline set_base resolvable`
Expected: FAIL.

- [ ] **Step 3: Implement.** Add to `theme_cmd.rs`:

```rust
use toml_edit::{DocumentMut, Item, Table, value};

/// Whether `name` resolves to a themes-dir file or a built-in.
fn resolvable(name: &str, themes_dir: &Path) -> bool {
    themes_dir.join(format!("{name}.toml")).is_file() || builtin_theme(name).is_some()
}

/// Set `[theme].base = name` in `doc`, creating `[theme]` if absent. Other
/// keys/comments are untouched.
fn set_base(doc: &mut DocumentMut, name: &str) {
    let theme = doc
        .entry("theme")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .expect("theme is a table");
    theme.set_implicit(false);
    theme["base"] = value(name);
}

fn use_theme(name: &str, config_path: &Path, themes_dir: &Path) {
    if !resolvable(name, themes_dir) {
        eprintln!(
            "unknown theme: {name}\navailable built-ins: {}",
            builtin_theme_names().join(", ")
        );
        std::process::exit(1);
    }
    // Reuse plugin_cmd's read/parse/refuse-to-clobber-invalid guard shape.
    let mut doc = match std::fs::read_to_string(config_path) {
        Ok(text) => match text.parse::<DocumentMut>() {
            Ok(doc) => doc,
            Err(_) => {
                eprintln!(
                    "config error: {} is not valid TOML; refusing to overwrite",
                    config_path.display()
                );
                std::process::exit(1);
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DocumentMut::new(),
        Err(e) => {
            eprintln!("config error: cannot read {}: {e}", config_path.display());
            std::process::exit(1);
        }
    };
    set_base(&mut doc, name);
    if let Some(parent) = config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(config_path, doc.to_string()) {
        eprintln!("failed to write config: {e}");
        std::process::exit(1);
    }
    println!("theme set to {name}");
}
```

Wire `run`: `ThemeCmd::Use { name } => use_theme(&name, config_path, themes_dir),`.

- [ ] **Step 4: Run tests + manual**

Run: `cargo test -p rustline theme`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rustline/src/theme_cmd.rs
git commit -m "feat(cli): rustline theme use — set [theme].base (comment-preserving)"
```

---

### Task 14: `theme new` — scaffold a theme file from a seed

**Files:**
- Modify: `crates/rustline/src/theme_cmd.rs` (implement `new_theme` + a `Color`→inline-value helper)

**Interfaces:**
- Produces: `rustline theme new <name> [--from <seed>] [--force]` writes `<themes_dir>/<name>.toml` (a complete, commented, inline-formatted `ThemeConfig`).

- [ ] **Step 1: Write the failing tests** — add to `theme_cmd.rs` `tests`:

```rust
    #[test]
    fn valid_name_rejects_path_traversal() {
        assert!(super::valid_theme_name("my-pastel"));
        assert!(!super::valid_theme_name(""));
        assert!(!super::valid_theme_name("a/b"));
        assert!(!super::valid_theme_name(".."));
        assert!(!super::valid_theme_name("a\\b"));
    }

    #[test]
    fn scaffold_round_trips_to_seed_theme() {
        // The written file re-parses to a ThemeConfig whose apply_to(default)
        // reproduces the seed built-in theme.
        let toml = super::scaffold_toml("my-nord", "nord", &rustline_core::builtin_theme("nord").unwrap());
        assert!(toml.starts_with("# rustline theme"), "has header: {toml}");
        let tc: rustline_core::ThemeConfig = toml::from_str(&toml).unwrap();
        let mut t = rustline_core::Theme::default();
        tc.apply_to(&mut t);
        assert_eq!(t, rustline_core::builtin_theme("nord").unwrap());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rustline valid_name scaffold`
Expected: FAIL.

- [ ] **Step 3: Implement.** Add to `theme_cmd.rs`:

```rust
use rustline_core::{Color, ThemeConfig};
use toml_edit::{Array, InlineTable, Value as EditValue};

/// A theme name is a bare filename stem: non-empty, no path separators or `..`.
fn valid_theme_name(name: &str) -> bool {
    !name.is_empty()
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
}

/// A `Color` as a `toml_edit` inline value in the documented enum form, e.g.
/// `{ Rgb = [42, 42, 54] }`, `{ Indexed = 31 }`, `{ Named = "cyan" }`.
fn color_value(c: &Color) -> EditValue {
    let mut t = InlineTable::new();
    match c {
        Color::Named(n) => {
            t.insert("Named", n.as_str().into());
        }
        Color::Indexed(i) => {
            t.insert("Indexed", (*i as i64).into());
        }
        Color::Rgb(r, g, b) => {
            let mut arr = Array::new();
            arr.push(*r as i64);
            arr.push(*g as i64);
            arr.push(*b as i64);
            t.insert("Rgb", EditValue::Array(arr));
        }
    }
    EditValue::InlineTable(t)
}

/// Build the full theme-file TOML text for `theme`, with a header comment.
fn scaffold_toml(name: &str, from: &str, theme: &rustline_core::Theme) -> String {
    let tc = ThemeConfig::from_theme(theme);
    let mut doc = toml_edit::DocumentMut::new();
    // Colors as inline tables; strings/arrays as their natural forms.
    macro_rules! put_color {
        ($k:literal, $v:expr) => {
            if let Some(c) = &$v {
                doc[$k] = toml_edit::Item::Value(color_value(c));
            }
        };
    }
    macro_rules! put_str {
        ($k:literal, $v:expr) => {
            if let Some(s) = &$v {
                doc[$k] = toml_edit::value(s.as_str());
            }
        };
    }
    if let Some(palette) = &tc.palette {
        let mut arr = Array::new();
        for c in palette {
            arr.push(color_value(c));
        }
        doc["palette"] = toml_edit::Item::Value(EditValue::Array(arr));
    }
    put_color!("fg", tc.fg);
    put_color!("bar_bg", tc.bar_bg);
    put_str!("hard_left", tc.hard_left);
    put_str!("hard_right", tc.hard_right);
    put_str!("soft_left", tc.soft_left);
    put_str!("soft_right", tc.soft_right);
    put_color!("soft_fg", tc.soft_fg);
    put_str!("win_cap_left", tc.win_cap_left);
    put_str!("win_cap_right", tc.win_cap_right);
    put_color!("win_current_bg", tc.win_current_bg);
    put_color!("win_current_fg", tc.win_current_fg);
    put_color!("win_inactive_bg", tc.win_inactive_bg);
    put_color!("win_inactive_fg", tc.win_inactive_fg);
    put_color!("success", tc.success);
    put_color!("info", tc.info);
    put_color!("warning", tc.warning);
    put_color!("error", tc.error);
    format!(
        "# rustline theme \"{name}\" (seeded from \"{from}\")\n# select with: rustline theme use {name}\n{doc}"
    )
}

fn new_theme(name: &str, from: &str, force: bool, themes_dir: &Path) {
    if !valid_theme_name(name) {
        eprintln!("invalid theme name: {name:?} (no empty, `/`, `\\`, or `..`)");
        std::process::exit(1);
    }
    // Resolve the seed: themes-dir file first, then built-in.
    let seed = {
        let file = themes_dir.join(format!("{from}.toml"));
        if let Ok(text) = std::fs::read_to_string(&file) {
            match toml::from_str::<ThemeConfig>(&text) {
                Ok(tc) => {
                    let mut t = Theme::default();
                    tc.apply_to(&mut t);
                    t
                }
                Err(e) => {
                    eprintln!("invalid seed theme file {}: {e}", file.display());
                    std::process::exit(1);
                }
            }
        } else {
            match builtin_theme(from) {
                Some(t) => t,
                None => {
                    eprintln!("unknown seed theme: {from}");
                    std::process::exit(1);
                }
            }
        }
    };
    let dest = themes_dir.join(format!("{name}.toml"));
    if dest.exists() && !force {
        eprintln!("{} already exists (use --force to overwrite)", dest.display());
        std::process::exit(1);
    }
    if let Err(e) = std::fs::create_dir_all(themes_dir) {
        eprintln!("cannot create themes dir {}: {e}", themes_dir.display());
        std::process::exit(1);
    }
    if let Err(e) = std::fs::write(&dest, scaffold_toml(name, from, &seed)) {
        eprintln!("failed to write {}: {e}", dest.display());
        std::process::exit(1);
    }
    println!("wrote {}", dest.display());
}
```

Wire `run`: `ThemeCmd::New { name, from, force } => new_theme(&name, &from, force, themes_dir),`.

- [ ] **Step 4: Run tests + lint + fmt + manual**

Run: `cargo fmt --all && cargo test -p rustline && cargo clippy -p rustline --all-targets -- -D warnings`
Then manual round-trip: `XDG_CONFIG_HOME=/tmp/rl-cfg cargo run -p rustline -- theme new my-nord --from nord && cat /tmp/rl-cfg/rustline/themes/my-nord.toml`
Expected: PASS / clean; the file shows inline `{ Rgb = [..] }` colors + the header.

- [ ] **Step 5: Commit**

```bash
git add crates/rustline/src/theme_cmd.rs
git commit -m "feat(cli): rustline theme new — scaffold a tweakable theme file"
```

---

### Task 15: Documentation

**Files:**
- Modify: `CLAUDE.md`
- Modify: `README.md`

**Interfaces:** none (docs only).

- [ ] **Step 1: Update `CLAUDE.md`.** Make these edits (concise — one line per pointer, link the spec):
  - **Module map (`rustline-core`):** add `themes.rs` (`builtin_theme`/`builtin_theme_names`, the six themes); note `Theme` now carries `success/info/warning/error` + `Theme::colors()`; `widgets/alert.rs` (the shared threshold badge helper); `ThemeConfig` is now a full optional mirror + `base`, with `apply_to`/`from_theme`/`to_theme_over`.
  - **Module map (`rustline-abi`):** add `ThemeColors`.
  - **`context.rs`:** add `colors: ThemeColors` to the `Context` field list.
  - **Module map (`rustline` bin):** add `theme_cmd.rs`; note `main.rs` gains `themes_dir()` + `resolve_theme`/`resolve_base_theme`.
  - **CLI section:** add the four `rustline theme list|show|use|new` lines.
  - **Config section:** add a **Themes** subsection — the six built-in names, `[theme].base` layering (`default → base → overrides`), themes-dir precedence (file shadows built-in) at `$XDG_CONFIG_HOME/rustline/themes`, the four semantic colors, and the per-widget `warn_percent`/`crit_percent` (and loadavg `warn_load`/`crit_load`, off by default) threshold knobs with `0 = off`.
  - **Widgets bullet:** note cpu/memory/battery/loadavg are now threshold-aware (alert badge = `bg=semantic, fg=bar_bg, bold`).
  - **Roadmap:** add a "Done" entry for this feature linking the spec/plan; add the spec/plan paths to **Design docs**.
- [ ] **Step 2: Update `README.md`.** Add a **Themes** section: list the six built-ins; show `rustline theme list/show/use/new` with a short example each; document `[theme].base`, the semantic colors, and per-widget thresholds; note curated themes use **truecolor** (require a truecolor-capable terminal + tmux `RGB`/`Tc`). Also update any widget/feature list to mention semantic colors + threshold alerts (keep parity with CLAUDE.md).
- [ ] **Step 3: Verify no stale claims.** `grep -n "one theme\|single accent\|to_theme" README.md CLAUDE.md` and fix anything the feature contradicts.
- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md README.md
git commit -m "docs: themes, semantic colors, theme CLI, threshold alerts"
```

---

## Final verification (before finishing the branch)

- [ ] `cargo fmt --all --check` — clean.
- [ ] `cargo clippy --all-targets -- -D warnings` — clean across the workspace.
- [ ] `just test` (or `cargo test --workspace`) — all pass, hermetic (no wasm toolchain).
- [ ] Manual: `cargo run -p rustline -- theme list`, `theme show pastel-rainbow`, `theme show nord`, and `render right` with `[theme].base` set — confirm the six themes read legibly and the cpu/memory/battery badges are visible and contrasting (the visual-check step from the spec; tweak any low-contrast color value in `themes.rs` if needed and re-commit).
- [ ] Confirm `render right` with a default (alerts-below-threshold) Context is unchanged from before the feature.

## Self-Review notes (author)

- **Spec coverage:** §1 base layering → Tasks 4/5/11; §2 types → Tasks 1/2/4; §3 Context.colors → Task 3; §4 thresholds → Tasks 6-10; §5 six themes → Task 5; §6 CLI → Tasks 11-14; §7 file format → Task 14. Testing/Docs → per-task + Task 15.
- **Deviation from spec (justified):** the spec mentioned a one-line `weather` guest demonstration reading `context.colors`. Dropped from the plan — plugin availability is fully delivered by the serialized `Context.colors` and verified by the Context serde round-trip (Task 3); touching the wasm guest would add non-hermetic build surface for no additional guarantee. Docs (Task 15) note plugins can read `context.colors`.
- **Type consistency:** `alert_over`/`alert_under`/`alert_style`/`AlertKind` used identically in Tasks 6-10; `ThemeColors`/`Theme::colors()`/`ThemeConfig::apply_to`/`from_theme`/`to_theme_over` names consistent across tasks; `Theme` gains `PartialEq` in Task 5 (needed by Task 5's `assert_eq!`).
