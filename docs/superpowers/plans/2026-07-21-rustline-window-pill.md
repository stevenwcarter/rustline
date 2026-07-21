# Window-list rounded pill — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move rustline's window-list styling into core as a themeable rounded-cap pill — active window in the accent color, inactive windows as dark-gray pills.

**Architecture:** Add six pill fields to `Theme`, a dedicated `render_window_pill` renderer (rounded caps, colored opposite to the pointed powerline separators used elsewhere), rewire `assemble.rs::render_window` to apply the pill using `ctx.window.is_current`, reduce the `Windows` widget to a text producer, and expose the six fields via the `[theme]` config table.

**Tech Stack:** Rust (edition 2024), serde, existing rustline-core render pipeline.

## Global Constraints

- Edition 2024 in every crate; clippy-clean (`cargo clippy --all-targets -- -D warnings`) and rustfmt-clean (`cargo fmt --all --check`).
- `Config::load` stays total — every new `ThemeConfig` field is `#[serde(default)]` (invariant #3).
- Windows render only from `Context` (invariant #1); the `catch_unwind` guard in `render_window` stays (invariant #6 / N2).
- `hermetic` `just test` must pass without the wasm toolchain.
- Rounded caps: left `\u{e0b6}` (``), right `\u{e0b4}` (``). Defaults: `win_current_bg = Color::Indexed(31)`, `win_current_fg = Color::Indexed(255)`, `win_inactive_bg = Color::Indexed(236)`, `win_inactive_fg = Color::Indexed(250)`. Active pill is bold; inactive is not.

---

### Task 1: Theme pill fields + `render_window_pill`

**Files:**
- Modify: `crates/rustline-core/src/render.rs` (`Theme` struct, `Default`, new fn + tests)

**Interfaces:**
- Produces: `Theme` fields `win_cap_left: String`, `win_cap_right: String`, `win_current_bg: Color`, `win_current_fg: Color`, `win_inactive_bg: Color`, `win_inactive_fg: Color`.
- Produces: `pub fn render_window_pill(text: &str, is_current: bool, theme: &Theme) -> String`.

- [ ] **Step 1: Write failing tests** (append to `render.rs` `mod tests`):

```rust
#[test]
fn window_pill_current_is_accent_bold_rounded() {
    let t = Theme::default();
    let out = render_window_pill("1* shell", true, &t);
    // rounded caps present
    assert!(out.contains('\u{e0b6}'), "left rounded cap: {out}");
    assert!(out.contains('\u{e0b4}'), "right rounded cap: {out}");
    // caps are pill-colored on the bar bg (fg=pill,bg=bar_bg)
    assert!(
        out.contains(&format!(
            "#[fg={},bg={}]\u{e0b6}",
            t.win_current_bg.to_tmux(),
            t.bar_bg.to_tmux()
        )),
        "left cap fg=pill,bg=bar: {out}"
    );
    // body: white bold text on the accent fill, spaced
    assert!(
        out.contains(&format!(
            "#[fg={},bg={},bold] 1* shell ",
            t.win_current_fg.to_tmux(),
            t.win_current_bg.to_tmux()
        )),
        "current body: {out}"
    );
    assert!(out.ends_with("#[default]"), "ends default: {out}");
}

#[test]
fn window_pill_inactive_is_gray_not_bold() {
    let t = Theme::default();
    let out = render_window_pill("2 editor", false, &t);
    assert!(out.contains('\u{e0b6}') && out.contains('\u{e0b4}'), "rounded caps: {out}");
    assert!(
        out.contains(&format!(
            "#[fg={},bg={}] 2 editor ",
            t.win_inactive_fg.to_tmux(),
            t.win_inactive_bg.to_tmux()
        )),
        "inactive body, no bold: {out}"
    );
    assert!(!out.contains("bold"), "inactive not bold: {out}");
}
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p rustline-core render:: 2>&1 | tail -20`
Expected: FAIL — `render_window_pill` not found, missing `Theme` fields.

- [ ] **Step 3: Add the `Theme` fields** — extend the struct (after `soft_fg`):

```rust
    pub soft_fg: Color,
    pub win_cap_left: String,
    pub win_cap_right: String,
    pub win_current_bg: Color,
    pub win_current_fg: Color,
    pub win_inactive_bg: Color,
    pub win_inactive_fg: Color,
```

And in `impl Default for Theme`, after `soft_fg: Color::Indexed(240),`:

```rust
            win_cap_left: "\u{e0b6}".into(),
            win_cap_right: "\u{e0b4}".into(),
            win_current_bg: Color::Indexed(31),
            win_current_fg: Color::Indexed(255),
            win_inactive_bg: Color::Indexed(236),
            win_inactive_fg: Color::Indexed(250),
```

Also update the second `theme()` test helper in `render.rs` `mod tests` (the one that builds a `Theme { … }` literal, around line 160) to include the six new fields (copy the defaults above) so it still compiles.

- [ ] **Step 4: Implement `render_window_pill`** (place after `render_region`):

```rust
/// Render one window as a self-contained rounded "pill": a left rounded cap,
/// the ` text ` body, and a right rounded cap. The caps are colored
/// `fg=<pill>,bg=<bar_bg>` (the opposite of the pointed powerline separators in
/// [`render_region`]), which is what makes them read as rounded ends of the
/// pill rather than arrows into the next segment. The active window uses the
/// accent fill + bold; inactive windows use the gray fill.
pub fn render_window_pill(text: &str, is_current: bool, theme: &Theme) -> String {
    let (pill, fg, bold) = if is_current {
        (&theme.win_current_bg, &theme.win_current_fg, ",bold")
    } else {
        (&theme.win_inactive_bg, &theme.win_inactive_fg, "")
    };
    let (pill, fg) = (pill.to_tmux(), fg.to_tmux());
    let bar = theme.bar_bg.to_tmux();
    format!(
        "#[fg={pill},bg={bar}]{cap_l}#[fg={fg},bg={pill}{bold}] {text} #[fg={pill},bg={bar}]{cap_r}#[default]",
        cap_l = theme.win_cap_left,
        cap_r = theme.win_cap_right,
    )
}
```

- [ ] **Step 5: Run to verify pass + fmt/clippy**

Run: `cargo test -p rustline-core render:: 2>&1 | tail -20 && cargo fmt --all && cargo clippy -p rustline-core --all-targets -- -D warnings 2>&1 | tail -5`
Expected: tests PASS, no clippy warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/rustline-core/src/render.rs
git commit -m "feat(core): themeable rounded window-pill renderer"
```

---

### Task 2: `Windows` widget → text producer

**Files:**
- Modify: `crates/rustline-core/src/widgets/windows.rs`

**Interfaces:**
- Produces: `Windows` widget emits one `Segment` with text `"{index}{flags} {name}"` and `Style::default()`; `None` window → `vec![]`.

- [ ] **Step 1: Update the tests** — replace the two style-asserting tests so they check text + default style only:

```rust
    #[test]
    fn current_window_text_only() {
        let w = ctx(Some(WindowCtx {
            index: "0".into(),
            name: "name".into(),
            flags: "*".into(),
            is_current: true,
        }));
        let out = Windows.render(&w);
        assert_eq!(out[0].text, "0* name");
        // styling now lives in the theme-aware pill renderer, not the widget
        assert_eq!(out[0].style, crate::Style::default());
    }

    #[test]
    fn inactive_window_text_only() {
        let w = ctx(Some(WindowCtx {
            index: "1".into(),
            name: "other".into(),
            flags: "".into(),
            is_current: false,
        }));
        let out = Windows.render(&w);
        assert_eq!(out[0].text, "1 other");
        assert_eq!(out[0].style, crate::Style::default());
    }
```

Keep `no_window_ctx_renders_nothing`.

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p rustline-core windows:: 2>&1 | tail -20`
Expected: FAIL — current still sets bold/bg.

- [ ] **Step 3: Simplify `render`** — replace the body:

```rust
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let Some(w) = &ctx.window else {
            return vec![];
        };
        let text = format!("{}{} {}", w.index, w.flags, w.name);
        vec![Segment::new(text)]
    }
```

Remove the now-unused `Color`/`Style` imports if clippy flags them (keep whatever the file still uses).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p rustline-core windows:: 2>&1 | tail -20 && cargo clippy -p rustline-core --all-targets -- -D warnings 2>&1 | tail -5`
Expected: PASS, no warnings (no unused imports).

- [ ] **Step 5: Commit**

```bash
git add crates/rustline-core/src/widgets/windows.rs
git commit -m "refactor(core): Windows widget emits text; styling moves to pill"
```

---

### Task 3: `render_window` applies the pill

**Files:**
- Modify: `crates/rustline-core/src/assemble.rs` (`render_window` + its doc comment; add tests)

**Interfaces:**
- Consumes: `render_window_pill` (Task 1), `Windows` widget (Task 2).

- [ ] **Step 1: Write failing tests** (add to `assemble.rs` `mod tests`; the module already imports what's needed — add a `WindowCtx` import):

```rust
    #[test]
    fn render_window_current_is_bold_accent_pill() {
        let cfg = Config::default();
        let reg = Registry::with_builtins(&cfg);
        let theme = cfg.to_theme();
        let mut c = ctx();
        c.window = Some(crate::WindowCtx {
            index: "1".into(),
            name: "shell".into(),
            flags: "*".into(),
            is_current: true,
        });
        let out = render_window(&c, &reg, &theme);
        assert!(out.contains('\u{e0b6}') && out.contains('\u{e0b4}'), "rounded caps: {out}");
        assert!(out.contains("1* shell"), "text: {out}");
        assert!(out.contains(",bold]"), "current bold: {out}");
        assert!(out.contains(&format!("bg={}", theme.win_current_bg.to_tmux())), "accent fill: {out}");
    }

    #[test]
    fn render_window_inactive_is_gray_pill() {
        let cfg = Config::default();
        let reg = Registry::with_builtins(&cfg);
        let theme = cfg.to_theme();
        let mut c = ctx();
        c.window = Some(crate::WindowCtx {
            index: "2".into(),
            name: "editor".into(),
            flags: "".into(),
            is_current: false,
        });
        let out = render_window(&c, &reg, &theme);
        assert!(out.contains("2 editor"), "text: {out}");
        assert!(out.contains(&format!("bg={}", theme.win_inactive_bg.to_tmux())), "gray fill: {out}");
        assert!(!out.contains(",bold]"), "inactive not bold: {out}");
    }

    #[test]
    fn render_window_no_window_is_empty() {
        let cfg = Config::default();
        let reg = Registry::with_builtins(&cfg);
        let theme = cfg.to_theme();
        assert_eq!(render_window(&ctx(), &reg, &theme), "");
    }
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p rustline-core assemble:: 2>&1 | tail -20`
Expected: FAIL — `render_window` still uses `render_region` (pointed caps, no gray inactive pill).

- [ ] **Step 3: Rewrite `render_window`** and its doc comment:

```rust
/// Render the single `windows` segment as a rounded pill. Unlike
/// [`render_named_region`], this does not go through [`render_region`]'s pointed
/// separators or `assign_palette`: the window list owns a dedicated rounded-cap
/// pill ([`render_window_pill`]), colored by the theme from the window's
/// current/inactive state. A panicking or absent window degrades to `""`.
pub fn render_window(ctx: &Context, registry: &Registry, theme: &Theme) -> String {
    let widgets = registry.resolve(&["windows".to_string()]);
    let segments: Vec<Segment> = widgets
        .iter()
        .flat_map(|w| render_guarded(w.as_ref(), ctx))
        .collect();
    let Some(seg) = segments.first() else {
        return String::new();
    };
    let is_current = ctx.window.as_ref().is_some_and(|w| w.is_current);
    crate::render::render_window_pill(&seg.text, is_current, theme)
}
```

Update the `use` at the top of `assemble.rs` if needed — it currently imports `render_region`; add `render_window_pill` or reference it via the `crate::render::` path as shown (the fully-qualified path needs no new import).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p rustline-core assemble:: 2>&1 | tail -20 && cargo clippy -p rustline-core --all-targets -- -D warnings 2>&1 | tail -5`
Expected: PASS, no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/rustline-core/src/assemble.rs
git commit -m "feat(core): render_window draws the rounded pill from theme"
```

---

### Task 4: Expose the six fields via `[theme]` config

**Files:**
- Modify: `crates/rustline-core/src/config.rs` (`ThemeConfig` struct, `to_theme`, tests)

**Interfaces:**
- Consumes: `Theme` fields (Task 1).

- [ ] **Step 1: Write failing test** (add to `config.rs` `mod tests`):

```rust
    #[test]
    fn to_theme_maps_window_pill_overrides() {
        use crate::Color;
        let mut cfg = Config::default();
        cfg.theme.win_current_bg = Some(Color::Indexed(60));
        cfg.theme.win_inactive_bg = Some(Color::Indexed(61));
        cfg.theme.win_current_fg = Some(Color::Indexed(62));
        cfg.theme.win_inactive_fg = Some(Color::Indexed(63));
        cfg.theme.win_cap_left = Some("L".into());
        cfg.theme.win_cap_right = Some("R".into());
        let t = cfg.to_theme();
        assert_eq!(t.win_current_bg, Color::Indexed(60));
        assert_eq!(t.win_inactive_bg, Color::Indexed(61));
        assert_eq!(t.win_current_fg, Color::Indexed(62));
        assert_eq!(t.win_inactive_fg, Color::Indexed(63));
        assert_eq!(t.win_cap_left, "L");
        assert_eq!(t.win_cap_right, "R");
    }

    #[test]
    fn to_theme_defaults_window_pill_when_unset() {
        let t = Config::default().to_theme();
        assert_eq!(t.win_current_bg, crate::Color::Indexed(31));
        assert_eq!(t.win_inactive_bg, crate::Color::Indexed(236));
        assert_eq!(t.win_cap_left, "\u{e0b6}");
    }
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p rustline-core config:: 2>&1 | tail -20`
Expected: FAIL — no such `ThemeConfig` fields.

- [ ] **Step 3: Add the `ThemeConfig` fields** (alongside `palette`/`fg`/`bar_bg`, each `#[serde(default)]`):

```rust
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
```

Match whatever attribute style the existing `bar_bg` field uses (if the struct already has a container-level `#[serde(default)]`, the per-field attrs may be redundant — follow the file's existing pattern).

- [ ] **Step 4: Map them in `to_theme`** (after the `bar_bg` block):

```rust
        if let Some(v) = &self.theme.win_cap_left {
            theme.win_cap_left = v.clone();
        }
        if let Some(v) = &self.theme.win_cap_right {
            theme.win_cap_right = v.clone();
        }
        if let Some(v) = &self.theme.win_current_bg {
            theme.win_current_bg = v.clone();
        }
        if let Some(v) = &self.theme.win_current_fg {
            theme.win_current_fg = v.clone();
        }
        if let Some(v) = &self.theme.win_inactive_bg {
            theme.win_inactive_bg = v.clone();
        }
        if let Some(v) = &self.theme.win_inactive_fg {
            theme.win_inactive_fg = v.clone();
        }
```

- [ ] **Step 5: Run to verify pass + full core suite + fmt/clippy**

Run: `cargo test -p rustline-core 2>&1 | tail -20 && cargo fmt --all --check && cargo clippy -p rustline-core --all-targets -- -D warnings 2>&1 | tail -5`
Expected: all PASS, clean.

- [ ] **Step 6: Commit**

```bash
git add crates/rustline-core/src/config.rs
git commit -m "feat(core): expose window-pill colors/caps via [theme] config"
```

---

### Task 5: Update binary smoke tests + docs

**Files:**
- Modify: `crates/rustline/tests/smoke.rs` (any window assertion assuming blue/pointed)
- Modify: `CLAUDE.md` (widget description + Config `[theme]` note)

**Interfaces:**
- Consumes: all prior tasks (end-to-end behavior).

- [ ] **Step 1: Inspect the current window assertions**

Run: `grep -n "render window\|window\|colour31\|e0b2\|e0b0" crates/rustline/tests/smoke.rs`
Read the matched tests. Identify assertions that assume the old blue/pointed-cap format.

- [ ] **Step 2: Run the smoke suite to see what breaks**

Run: `cargo test -p rustline --test smoke 2>&1 | tail -30`
Expected: any window-format assertion that hardcoded the old style FAILS; note which.

- [ ] **Step 3: Update the failing assertions** to the rounded-pill format — assert on stable facts (contains the window text, contains a rounded cap `\u{e0b6}`/`\u{e0b4}`, current is bold) rather than exact colors. Show the concrete edits inline based on Step 1's findings. If no window assertion hardcodes the old style, record "no smoke changes needed" and skip to Step 5.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p rustline --test smoke 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Update `CLAUDE.md`** — in the `widgets/` module map, change the `windows` description to note it emits text and the pill styling/colors come from the theme; update the render-pipeline line that says window segments are self-contained "(no palette)" to mention the rounded pill; and in the Config section, document the six `[theme]` `win_*` overrides (one line + the default values). Keep it concise.

- [ ] **Step 6: Full hermetic suite + fmt/clippy**

Run: `just test 2>&1 | tail -15 && cargo fmt --all --check && cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: all PASS, clean.

- [ ] **Step 7: Commit**

```bash
git add crates/rustline/tests/smoke.rs CLAUDE.md
git commit -m "test+docs: rounded-pill window list"
```

---

## Self-Review

**Spec coverage:** Theme fields (T1), pill renderer (T1), render_window rewrite (T3), widget→text (T2), config exposure (T4), tests across modules (T1–T4), smoke + docs (T5). All spec sections covered. The non-repo tmux.conf revert is handled during branch-finish, outside the plan tasks.

**Placeholder scan:** Task 5 Step 3 intentionally defers exact edits to what Step 1/2 discover (the current smoke assertions aren't known until inspected) but bounds them precisely (assert text + rounded cap + bold, not colors) — not an open-ended placeholder.

**Type consistency:** `render_window_pill(text: &str, is_current: bool, theme: &Theme) -> String` used identically in T1 and T3. `ThemeConfig` field names match `Theme` field names and the `to_theme` mapping. Defaults consistent across T1 and T4 (`31/255/236/250`, caps `\u{e0b6}`/`\u{e0b4}`).
