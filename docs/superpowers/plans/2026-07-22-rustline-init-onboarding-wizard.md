# rustline `init` onboarding wizard — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn `rustline init` into an interactive onboarding wizard that writes a tailored `config.toml` and an idempotent tmux marker-block from a few questions, with `--defaults` (non-interactive) and `--print` (legacy raw block) escape hatches.

**Architecture:** A thin I/O shell (`init.rs`) over pure, unit-tested helpers. `tmux_conf::init_block` is extended to a `&InitBlockOpts` (one/two-line, mouse, interval). Starter widget/theme defaults live in an embedded `assets/starter-config.toml` (`include_str!`) that the wizard mutates via `toml_edit`. Config writes merge non-destructively; the tmux block is upserted between `# >>> rustline >>>` markers.

**Tech Stack:** Rust (edition 2024), clap derive, `toml_edit` 0.25, `toml` 0.9, `std::io::IsTerminal`, `tempfile` (dev).

## Global Constraints

- Edition 2024 in every crate; `rustfmt.toml` is edition 2024. Keep clippy-clean (`cargo clippy --all-targets -- -D warnings`) and rustfmt-clean (`cargo fmt --all --check`).
- **Widget `alt_format` code `Default`s MUST stay `""`.** Shortened forms live ONLY in `assets/starter-config.toml`. (Invariant: existing users' output + byte-identical `alt_format` tests unchanged.)
- **Invariant #4 (init injection-safety):** every interpolated tmux var stays `#{q:...}` + `--flag=` form, in both one- and two-line blocks.
- **`--print` output byte-identical to today's `rustline init`** (one-line, mouse-off, interval 1).
- `Config::load` totality (invariant #3) untouched — this feature writes config, never changes how it's read.
- Commit `Cargo.lock` alongside any dependency change (none expected here).
- Run `cargo fmt --all` before each commit.

---

### Task 1: Extend `init_block` to `InitBlockOpts` (one/two-line, mouse, interval)

**Files:**
- Modify: `crates/rustline/src/tmux_conf.rs` (replace `init_block(bar_bg, fg)` with `init_block(&InitBlockOpts)`; add the struct + two-line constants)
- Modify: `crates/rustline/src/main.rs:153-156` (update the `Command::Init` arm to build a one-line-default `InitBlockOpts` so it compiles; Task 6 rewrites this arm)
- Test: `crates/rustline/src/tmux_conf.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `pub struct InitBlockOpts<'a> { pub bar_bg: &'a str, pub fg: &'a str, pub two_line: bool, pub mouse: bool, pub interval: u32 }` and `pub fn init_block(opts: &InitBlockOpts) -> String`.

- [ ] **Step 1: Write the failing tests**

Replace the three existing tests' `init_block("colour234", "colour255")` calls with a one-line opts helper, and add new coverage. Put this at the top of the `tests` mod and update the existing asserts to use `one_line(...)`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn one_line<'a>(bar_bg: &'a str, fg: &'a str) -> InitBlockOpts<'a> {
        InitBlockOpts { bar_bg, fg, two_line: false, mouse: false, interval: 1 }
    }

    #[test]
    fn one_line_default_is_byte_identical_to_legacy() {
        // Characterization: the one-line / mouse-off / interval-1 block is EXACTLY
        // the legacy `rustline init` output (pins the `alt_format`-defaults-stay-empty
        // seam at the tmux-block boundary — `--print` must not drift).
        let b = init_block(&one_line("colour234", "colour255"));
        assert!(b.starts_with("# rustline statusline\nset -g status on\nset -g status-interval 1\nset -g status-justify centre\n"));
        assert!(b.contains("set -g status-style bg=colour234,fg=colour255\n"));
        assert!(!b.contains("set -g mouse on"), "mouse-off omits the setter: {b}");
        assert!(!b.contains("set -g status 2"), "one-line has no two-line formats: {b}");
        assert!(b.contains("#(rustline render left"));
        assert!(b.contains("MouseDown1Status"));
    }

    #[test]
    fn interval_is_honored() {
        let mut o = one_line("colour234", "colour255");
        o.interval = 5;
        assert!(init_block(&o).contains("set -g status-interval 5\n"));
    }

    #[test]
    fn mouse_on_emits_setter() {
        let mut o = one_line("colour234", "colour255");
        o.mouse = true;
        let b = init_block(&o);
        assert!(b.contains("set -g mouse on\n"), "mouse on emits setter: {b}");
    }

    #[test]
    fn two_line_emits_status_two_and_formats() {
        let mut o = one_line("colour234", "colour255");
        o.two_line = true;
        let b = init_block(&o);
        assert!(b.contains("set -g status 2\n"), "two-line count: {b}");
        assert!(b.contains("set -g status-format[0]"), "top format: {b}");
        assert!(b.contains("set -g status-format[1]"), "bottom format: {b}");
        // both formats reference the shared status-left/right and window list
        assert!(b.contains("#{T:window-status-format}"));
        assert!(b.contains(":status-right}"));
        // shared wiring still present
        assert!(b.contains("#(rustline render left"));
    }
}
```

Also update the three legacy tests (`init_block_wires_all_regions_and_hooks`, `init_block_escapes_untrusted_vars_and_sets_status_style`, `init_block_wires_click_toggle_binding`) to call `init_block(&one_line("colour234", "colour255"))`. Their assertions are unchanged and must still pass (injection-safety is preserved).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rustline --lib tmux_conf`
Expected: FAIL to compile (`init_block` still takes two `&str`; `InitBlockOpts` undefined).

- [ ] **Step 3: Implement the extended `init_block`**

Replace the whole `init_block` function (keep the doc comment about injection safety; add a line noting the struct). New body:

```rust
use std::fmt::Write as _;

/// Options controlling the tmux block `rustline init` emits.
///
/// `two_line` renders the window list on its own line above status-left/right
/// (the author's layout); `mouse` adds `set -g mouse on` so click-to-toggle
/// works; `interval` sets `status-interval`.
pub struct InitBlockOpts<'a> {
    pub bar_bg: &'a str,
    pub fg: &'a str,
    pub two_line: bool,
    pub mouse: bool,
    pub interval: u32,
}

/// Verbatim two-line `status-format[0]` (centered per-window list) and
/// `status-format[1]` (status-left/right), copied from the author's proven
/// `~/.tmux.conf`. These contain no `#(...)` shell calls — they reference the
/// already-`#{q:}`-escaped `status-left`/`status-right`/`window-status-format`
/// options the shared block sets, so injection-safety (invariant #4) holds.
const STATUS_FORMAT_0: &str = r##"set -g status-format[0] "#[list=on align=#{status-justify}]#[list=left-marker]<#[list=right-marker]>#[list=on]#{W:#[range=window|#{window_index} #{E:window-status-style}#{?#{&&:#{window_last_flag},#{!=:#{E:window-status-last-style},default}}, #{E:window-status-last-style},}#{?#{&&:#{window_bell_flag},#{!=:#{E:window-status-bell-style},default}}, #{E:window-status-bell-style},#{?#{&&:#{||:#{window_activity_flag},#{window_silence_flag}},#{!=:#{E:window-status-activity-style},default}}, #{E:window-status-activity-style},}}]#[push-default]#{T:window-status-format}#[pop-default]#[norange default]#{?loop_last_flag,,#{E:window-status-separator}},#[range=window|#{window_index} list=focus #{?#{!=:#{E:window-status-current-style},default},#{E:window-status-current-style},#{E:window-status-style}}#{?#{&&:#{window_last_flag},#{!=:#{E:window-status-last-style},default}}, #{E:window-status-last-style},}#{?#{&&:#{window_bell_flag},#{!=:#{E:window-status-bell-style},default}}, #{E:window-status-bell-style},#{?#{&&:#{||:#{window_activity_flag},#{window_silence_flag}},#{!=:#{E:window-status-activity-style},default}}, #{E:window-status-activity-style},}}]#[push-default]#{T:window-status-current-format}#[pop-default]#[norange list=on default]#{?loop_last_flag,,#{E:window-status-separator}}}""##;

const STATUS_FORMAT_1: &str = r##"set -g status-format[1] "#[align=left range=left #{E:status-left-style}]#[push-default]#{T;=/#{status-left-length}:status-left}#[pop-default]#[norange default]#[nolist align=right range=right #{E:status-right-style}]#[push-default]#{T;=/#{status-right-length}:status-right}#[pop-default]#[norange default]""##;

pub fn init_block(opts: &InitBlockOpts) -> String {
    let mut block = String::from("# rustline statusline\nset -g status on\n");
    let _ = writeln!(block, "set -g status-interval {}", opts.interval);
    block.push_str("set -g status-justify centre\n");
    let _ = writeln!(block, "set -g status-style bg={},fg={}", opts.bar_bg, opts.fg);
    if opts.mouse {
        block.push_str("set -g mouse on\n");
    }
    block.push_str(
        r##"set -g status-left-length 100
set -g status-right-length 200
set -g status-left  "#(rustline render left --session=#{q:session_name} --window=#{q:window_index} --pane=#{q:pane_index} --pane-path=#{q:pane_current_path})"
set -g status-right "#(rustline render right --session=#{q:session_name} --window=#{q:window_index} --pane=#{q:pane_index} --pane-path=#{q:pane_current_path})"
set -g window-status-separator ""
setw -g window-status-format         "#(rustline render window --index=#{q:window_index} --name=#{q:window_name} --flags=#{q:window_flags})"
setw -g window-status-current-format "#(rustline render window --current --index=#{q:window_index} --name=#{q:window_name} --flags=#{q:window_flags})"
set-hook -g after-select-pane   "refresh-client -S"
set-hook -g after-select-window "refresh-client -S"
"##,
    );
    block.push_str(
        r##"# rustline click-to-toggle a widget's alt view (needs: set -g mouse on)
bind -T root MouseDown1Status {
    if -F "#{==:#{mouse_status_range},window}" {
        select-window -t=
    } {
        if -F "#{mouse_status_range}" {
            run-shell "rustline click --range=#{q:mouse_status_range} --button=left"
            refresh-client -S
        }
    }
}
"##,
    );
    if opts.two_line {
        block.push_str("set -g status 2\n");
        block.push_str(STATUS_FORMAT_0);
        block.push('\n');
        block.push_str(STATUS_FORMAT_1);
        block.push('\n');
    }
    block
}
```

Update `main.rs` `Command::Init` arm (temporary; Task 6 replaces it):

```rust
        Command::Init => {
            let bar_bg = theme.bar_bg.to_tmux();
            let fg = theme.fg.to_tmux();
            let opts = tmux_conf::InitBlockOpts {
                bar_bg: &bar_bg,
                fg: &fg,
                two_line: false,
                mouse: false,
                interval: 1,
            };
            print!("{}", tmux_conf::init_block(&opts));
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rustline --lib tmux_conf && cargo build -p rustline`
Expected: PASS; build OK.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy -p rustline --all-targets -- -D warnings
git add crates/rustline/src/tmux_conf.rs crates/rustline/src/main.rs
git commit -m "feat(init): parameterize init_block via InitBlockOpts (two-line, mouse, interval)"
```

---

### Task 2: Embedded starter template + `starter_config_toml`

**Files:**
- Create: `crates/rustline/assets/starter-config.toml`
- Create: `crates/rustline/src/init.rs`
- Modify: `crates/rustline/src/main.rs:1-12` (add `mod init;` in the module list)
- Test: `crates/rustline/src/init.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `pub struct InitAnswers { pub theme: String, pub two_line: bool, pub mouse: bool, pub battery: bool, pub tailscale: bool, pub lan_ip: bool, pub clock: ClockStyle, pub interval: u32 }`; `pub enum ClockStyle { TwentyFour, TwentyFourSeconds, Twelve, TwelveSeconds }` with `pub fn formats(&self) -> (&'static str, &'static str)`; `pub fn starter_config_toml(a: &InitAnswers) -> String`.

- [ ] **Step 1: Create the embedded starter template**

`crates/rustline/assets/starter-config.toml`:

```toml
# rustline config — generated by `rustline init`.
# Edit freely; re-running `rustline init` only ADDS sections you don't have and
# (re)sets [theme].base — it never overwrites options you've changed.
# Weather and other WASM plugins are opt-in: see the project README.

[layout]
left = ["pane_id", "hostname"]
center = ["windows"]
right = ["cwd", "cpu", "memory", "loadavg", "datetime"]

[theme]
base = "default"

[widgets.cwd]
abbreviate_home = true

[widgets.datetime]
format = "%a %Y-%m-%d %H:%M"
alt_format = "%m-%d %H:%M"

[widgets.cpu]
format = "{icon} {bar} {percent}%"
alt_format = "{icon} {percent}%"
bar_width = 6

[widgets.memory]
format = "{icon} {used}/{total}"
alt_format = "{icon} {used}"

[widgets.loadavg]
alt_format = "LD {load1:.1}"

[widgets.battery]
format = "{icon} {percent}%"
alt_format = "{icon}"

[widgets.lan_ip]
format = "󰌗 {ip}"
alt_format = "󰌗"

[widgets.tailscale_ip]
format = "󰖂 {ip}"
alt_format = "󰖂"
down_format = "TS off"
```

- [ ] **Step 2: Write the failing tests**

Create `crates/rustline/src/init.rs` with only the tests first (types referenced will fail to compile — that's the RED):

```rust
//! `rustline init` onboarding wizard: gathers a few answers and writes a
//! tailored `config.toml` plus an idempotent tmux marker-block. Pure helpers
//! (template mutation, config merge, prompt parsing) are unit-tested; the
//! interactive prompt loop is a thin I/O shell over them.

#[cfg(test)]
mod tests {
    use super::*;
    use rustline_core::Config;

    fn base_answers() -> InitAnswers {
        InitAnswers {
            theme: "nord".into(),
            two_line: false,
            mouse: true,
            battery: false,
            tailscale: false,
            lan_ip: false,
            clock: ClockStyle::Twelve,
            interval: 1,
        }
    }

    #[test]
    fn clock_formats_cover_all_presets() {
        assert_eq!(ClockStyle::TwentyFour.formats(), ("%a %Y-%m-%d %H:%M", "%m-%d %H:%M"));
        assert_eq!(ClockStyle::TwentyFourSeconds.formats(), ("%a %Y-%m-%d %H:%M:%S", "%m-%d %H:%M:%S"));
        assert_eq!(ClockStyle::Twelve.formats(), ("%a %Y-%m-%d %I:%M %p", "%m-%d %I:%M %p"));
        assert_eq!(ClockStyle::TwelveSeconds.formats(), ("%a %Y-%m-%d %I:%M:%S %p", "%m-%d %I:%M:%S %p"));
    }

    #[test]
    fn starter_parses_and_reflects_theme_and_clock() {
        let toml = starter_config_toml(&base_answers());
        let cfg: Config = toml::from_str(&toml).expect("valid config");
        assert_eq!(cfg.theme.base.as_deref(), Some("nord"));
        assert_eq!(cfg.widgets.datetime.format, "%a %Y-%m-%d %I:%M %p");
        assert_eq!(cfg.widgets.datetime.alt_format, "%m-%d %I:%M %p");
        // shortened alt_formats from the template survive
        assert_eq!(cfg.widgets.cpu.alt_format, "{icon} {percent}%");
        assert_eq!(cfg.widgets.loadavg.alt_format, "LD {load1:.1}");
    }

    #[test]
    fn layout_includes_only_selected_optional_widgets() {
        let mut a = base_answers();
        a.battery = true;
        a.tailscale = true;
        a.lan_ip = false;
        let cfg: Config = toml::from_str(&starter_config_toml(&a)).unwrap();
        assert!(cfg.layout.right.contains(&"battery".to_string()));
        assert!(cfg.layout.left.contains(&"tailscale_ip".to_string()));
        assert!(!cfg.layout.left.contains(&"lan_ip".to_string()));
        // required widgets always present, in order
        assert_eq!(cfg.layout.right, vec!["cwd", "cpu", "memory", "battery", "loadavg", "datetime"]);
        assert_eq!(cfg.layout.left, vec!["pane_id", "hostname", "tailscale_ip"]);
    }

    #[test]
    fn unselected_optional_widget_sections_are_pruned() {
        let a = base_answers(); // all optional off
        let toml = starter_config_toml(&a);
        assert!(!toml.contains("[widgets.battery]"), "battery pruned: {toml}");
        assert!(!toml.contains("[widgets.lan_ip]"), "lan_ip pruned: {toml}");
        assert!(!toml.contains("[widgets.tailscale_ip]"), "tailscale pruned: {toml}");
        // required widget sections remain
        assert!(toml.contains("[widgets.cpu]"));
    }
}
```

- [ ] **Step 3: Add `mod init;` and implement the types + `starter_config_toml`**

In `main.rs`, add `mod init;` to the module list near the top (alphabetical, after `mod if-...`/before `mod logging;` — place it after `mod cpu;`/wherever alphabetical: add the line `mod init;`).

Prepend to `init.rs` (above the `tests` mod):

```rust
use toml_edit::{Array, DocumentMut, value};

/// The recommended starter config, embedded at build time.
const STARTER_TEMPLATE: &str = include_str!("../assets/starter-config.toml");

/// A datetime preset the wizard offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockStyle {
    TwentyFour,
    TwentyFourSeconds,
    Twelve,
    TwelveSeconds,
}

impl ClockStyle {
    /// `(format, alt_format)` strftime patterns for this preset.
    pub fn formats(&self) -> (&'static str, &'static str) {
        match self {
            ClockStyle::TwentyFour => ("%a %Y-%m-%d %H:%M", "%m-%d %H:%M"),
            ClockStyle::TwentyFourSeconds => ("%a %Y-%m-%d %H:%M:%S", "%m-%d %H:%M:%S"),
            ClockStyle::Twelve => ("%a %Y-%m-%d %I:%M %p", "%m-%d %I:%M %p"),
            ClockStyle::TwelveSeconds => ("%a %Y-%m-%d %I:%M:%S %p", "%m-%d %I:%M:%S %p"),
        }
    }
}

/// Answers collected by the wizard (or the `--defaults` set).
#[derive(Clone, Debug)]
pub struct InitAnswers {
    pub theme: String,
    pub two_line: bool,
    pub mouse: bool,
    pub battery: bool,
    pub tailscale: bool,
    pub lan_ip: bool,
    pub clock: ClockStyle,
    pub interval: u32,
}

/// Build the generated `config.toml` text from the embedded template + answers:
/// set `[theme].base`, the layout arrays (selected optional widgets), the
/// datetime format/alt, and prune the option sections of unselected optional
/// widgets. Comments in the template are preserved by `toml_edit`.
pub fn starter_config_toml(a: &InitAnswers) -> String {
    // The template is a compile-time constant known-valid; parse can't fail.
    let mut doc: DocumentMut = STARTER_TEMPLATE.parse().expect("embedded template is valid TOML");

    doc["theme"]["base"] = value(a.theme.as_str());

    let mut left = Array::new();
    left.push("pane_id");
    left.push("hostname");
    if a.lan_ip {
        left.push("lan_ip");
    }
    if a.tailscale {
        left.push("tailscale_ip");
    }
    doc["layout"]["left"] = value(left);

    let mut right = Array::new();
    for w in ["cwd", "cpu", "memory"] {
        right.push(w);
    }
    if a.battery {
        right.push("battery");
    }
    right.push("loadavg");
    right.push("datetime");
    doc["layout"]["right"] = value(right);

    let (fmt, alt) = a.clock.formats();
    doc["widgets"]["datetime"]["format"] = value(fmt);
    doc["widgets"]["datetime"]["alt_format"] = value(alt);

    if let Some(w) = doc["widgets"].as_table_mut() {
        if !a.battery {
            w.remove("battery");
        }
        if !a.lan_ip {
            w.remove("lan_ip");
        }
        if !a.tailscale {
            w.remove("tailscale_ip");
        }
    }

    doc.to_string()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rustline --lib init`
Expected: PASS (4 tests).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy -p rustline --all-targets -- -D warnings
git add crates/rustline/assets/starter-config.toml crates/rustline/src/init.rs crates/rustline/src/main.rs
git commit -m "feat(init): embedded starter template + starter_config_toml"
```

---

### Task 3: Non-destructive config merge + write

**Files:**
- Modify: `crates/rustline/src/init.rs` (add `merge_config` pure fn + `write_config` I/O fn)
- Modify: `crates/rustline/src/theme_cmd.rs:37` (change `fn set_base` to `pub(crate) fn set_base`)
- Test: `crates/rustline/src/init.rs`

**Interfaces:**
- Consumes: `crate::theme_cmd::set_base(&mut DocumentMut, &str)` (now `pub(crate)`); `starter_config_toml` (Task 2).
- Produces: `pub fn merge_config(existing: &str, generated: &str, theme: &str) -> Result<String, String>`; `pub fn write_config(a: &InitAnswers, config_path: &std::path::Path) -> std::io::Result<std::path::PathBuf>` (returns the backup path written, if any — `PathBuf::new()` when none).

- [ ] **Step 1: Write the failing tests**

Add to `init.rs` `tests`:

```rust
    #[test]
    fn merge_into_empty_uses_generated_and_sets_theme() {
        let generated = starter_config_toml(&base_answers());
        let out = merge_config("", &generated, "nord").unwrap();
        let cfg: Config = toml::from_str(&out).unwrap();
        assert_eq!(cfg.theme.base.as_deref(), Some("nord"));
        assert_eq!(cfg.widgets.cpu.alt_format, "{icon} {percent}%");
    }

    #[test]
    fn merge_preserves_user_widget_and_layout_but_overrides_theme() {
        let existing = r#"
[layout]
right = ["datetime"]
[theme]
base = "gruvbox"
[widgets.cpu]
format = "USER {percent}%"
"#;
        let generated = starter_config_toml(&base_answers());
        let out = merge_config(existing, &generated, "tokyo-night").unwrap();
        let cfg: Config = toml::from_str(&out).unwrap();
        // theme.base is actively re-set to the chosen theme
        assert_eq!(cfg.theme.base.as_deref(), Some("tokyo-night"));
        // user's existing layout + cpu format are preserved (not clobbered)
        assert_eq!(cfg.layout.right, vec!["datetime"]);
        assert_eq!(cfg.widgets.cpu.format, "USER {percent}%");
        // a widget section the user lacked (memory) is added from the starter
        assert_eq!(cfg.widgets.memory.alt_format, "{icon} {used}");
    }

    #[test]
    fn merge_rejects_invalid_existing_config() {
        let out = merge_config("this is = = not valid [[[", "", "nord");
        assert!(out.is_err());
    }

    #[test]
    fn write_config_fresh_creates_file_no_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rustline").join("config.toml");
        let bak = write_config(&base_answers(), &path).unwrap();
        assert!(path.is_file());
        assert_eq!(bak, std::path::PathBuf::new(), "no backup for a fresh file");
        let cfg: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(cfg.theme.base.as_deref(), Some("nord"));
    }

    #[test]
    fn write_config_existing_backs_up_and_merges() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "[widgets.cpu]\nformat = \"USER\"\n").unwrap();
        let bak = write_config(&base_answers(), &path).unwrap();
        assert!(bak.is_file(), "backup written: {bak:?}");
        let cfg: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(cfg.widgets.cpu.format, "USER"); // preserved
        assert_eq!(cfg.theme.base.as_deref(), Some("nord")); // set
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rustline --lib init`
Expected: FAIL to compile (`merge_config`/`write_config` undefined).

- [ ] **Step 3: Implement `merge_config` + `write_config`; expose `set_base`**

In `theme_cmd.rs`, change `fn set_base(` to `pub(crate) fn set_base(`.

Add to `init.rs` (imports: extend the `use` line and add `std::fs`, `std::path`):

```rust
use std::fs;
use std::path::{Path, PathBuf};
use toml_edit::{Array, DocumentMut, Item, Table, value};
```

```rust
/// Merge the generated starter into an existing config **non-destructively**:
/// `[theme].base` is always (re)set to `theme`; `[layout]` and each
/// `[widgets.<name>]` table are added only if absent. Returns the merged TOML,
/// or `Err` if `existing` is not valid TOML (caller must not overwrite it).
pub fn merge_config(existing: &str, generated: &str, theme: &str) -> Result<String, String> {
    let mut doc: DocumentMut = existing
        .parse()
        .map_err(|e| format!("existing config is not valid TOML: {e}"))?;
    let gen: DocumentMut = generated
        .parse()
        .map_err(|e| format!("generated config invalid (bug): {e}"))?;

    crate::theme_cmd::set_base(&mut doc, theme);

    if doc.get("layout").is_none() {
        if let Some(layout) = gen.get("layout") {
            doc["layout"] = layout.clone();
        }
    }

    if let Some(gw) = gen.get("widgets").and_then(Item::as_table) {
        let existing_w = doc
            .entry("widgets")
            .or_insert(Item::Table(Table::new()));
        if let Some(ew) = existing_w.as_table_mut() {
            ew.set_implicit(false);
            for (k, v) in gw.iter() {
                if !ew.contains_key(k) {
                    ew.insert(k, v.clone());
                }
            }
        }
    }

    Ok(doc.to_string())
}

/// Sibling backup path `config.toml.rustline.bak`.
fn backup_path(config_path: &Path) -> PathBuf {
    let mut s = config_path.as_os_str().to_owned();
    s.push(".rustline.bak");
    PathBuf::from(s)
}

/// Write the tailored config to `config_path`. Fresh file → the full generated
/// starter. Existing file → back it up to `<path>.rustline.bak`, then write the
/// non-destructive merge. Returns the backup path (empty `PathBuf` if none).
pub fn write_config(a: &InitAnswers, config_path: &Path) -> std::io::Result<PathBuf> {
    let generated = starter_config_toml(a);
    match fs::read_to_string(config_path) {
        Ok(existing) => {
            let merged = merge_config(&existing, &generated, &a.theme)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            let bak = backup_path(config_path);
            fs::write(&bak, &existing)?;
            fs::write(config_path, merged)?;
            Ok(bak)
        }
        Err(_) => {
            if let Some(parent) = config_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(config_path, generated)?;
            Ok(PathBuf::new())
        }
    }
}
```

Note: remove the now-duplicate `use toml_edit::{Array, DocumentMut, value};` line from Task 2 if present — keep a single merged `use toml_edit::{...}` importing `Array, DocumentMut, Item, Table, value`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rustline --lib init && cargo test -p rustline --lib theme_cmd`
Expected: PASS (init: 9 tests; theme_cmd unchanged).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy -p rustline --all-targets -- -D warnings
git add crates/rustline/src/init.rs crates/rustline/src/theme_cmd.rs
git commit -m "feat(init): non-destructive config merge + write_config with backup"
```

---

### Task 4: Idempotent tmux marker-block upsert

**Files:**
- Modify: `crates/rustline/src/tmux_conf.rs` (add `upsert_tmux_block` + `strip_region` + marker consts)
- Test: `crates/rustline/src/tmux_conf.rs`

**Interfaces:**
- Produces: `pub const TMUX_BEGIN: &str`; `pub const TMUX_END: &str`; `pub fn upsert_tmux_block(existing: &str, block: &str) -> String`.

- [ ] **Step 1: Write the failing tests**

Add to `tmux_conf.rs` `tests`:

```rust
    #[test]
    fn upsert_appends_when_no_markers() {
        let out = upsert_tmux_block("set -g mouse on\n", "BLOCK");
        assert!(out.contains("set -g mouse on"), "keeps user content: {out}");
        assert!(out.contains(TMUX_BEGIN) && out.contains(TMUX_END));
        assert!(out.contains("\nBLOCK\n"), "wraps block: {out}");
    }

    #[test]
    fn upsert_into_empty_is_just_the_wrapped_block() {
        let out = upsert_tmux_block("", "BLOCK");
        assert_eq!(out, format!("{TMUX_BEGIN}\nBLOCK\n{TMUX_END}\n"));
    }

    #[test]
    fn upsert_replaces_existing_region_and_preserves_surroundings() {
        let first = upsert_tmux_block("user before\n", "OLD");
        let second = upsert_tmux_block(&first, "NEW");
        assert!(second.contains("user before"), "keeps content before markers");
        assert!(second.contains("NEW") && !second.contains("OLD"), "replaced: {second}");
        // exactly one marker pair
        assert_eq!(second.matches(TMUX_BEGIN).count(), 1);
        assert_eq!(second.matches(TMUX_END).count(), 1);
    }

    #[test]
    fn upsert_is_idempotent() {
        let once = upsert_tmux_block("user before\n", "BLOCK");
        let twice = upsert_tmux_block(&once, "BLOCK");
        assert_eq!(once, twice, "re-running with same block is a no-op");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rustline --lib tmux_conf`
Expected: FAIL to compile (`upsert_tmux_block`/consts undefined).

- [ ] **Step 3: Implement the upsert**

Add to `tmux_conf.rs` (above `#[cfg(test)]`):

```rust
/// Marker lines bracketing the rustline-managed region in `~/.tmux.conf`, so
/// re-running `rustline init` replaces that region instead of appending a
/// duplicate.
pub const TMUX_BEGIN: &str = "# >>> rustline >>>";
pub const TMUX_END: &str = "# <<< rustline <<<";

/// Remove an existing `TMUX_BEGIN..=TMUX_END` region (if present), returning the
/// surrounding content joined and right-trimmed.
fn strip_region(s: &str) -> String {
    if let (Some(b), Some(e)) = (s.find(TMUX_BEGIN), s.find(TMUX_END)) {
        if e >= b {
            let end = e + TMUX_END.len();
            let before = s[..b].trim_end();
            let after = &s[end..];
            return format!("{before}{after}");
        }
    }
    s.to_string()
}

/// Insert or replace the rustline-managed block in an existing `~/.tmux.conf`.
/// Idempotent: `upsert(upsert(x, b), b) == upsert(x, b)`. Content outside the
/// markers is preserved; the block is separated from prior content by a blank
/// line.
pub fn upsert_tmux_block(existing: &str, block: &str) -> String {
    let base = strip_region(existing);
    let wrapped = format!("{TMUX_BEGIN}\n{}\n{TMUX_END}\n", block.trim_end_matches('\n'));
    let base = base.trim_end_matches('\n');
    if base.trim().is_empty() {
        wrapped
    } else {
        format!("{base}\n\n{wrapped}")
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rustline --lib tmux_conf`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy -p rustline --all-targets -- -D warnings
git add crates/rustline/src/tmux_conf.rs
git commit -m "feat(init): idempotent tmux marker-block upsert"
```

---

### Task 5: Prompt parsers + `defaults`

**Files:**
- Modify: `crates/rustline/src/init.rs` (add `parse_menu_choice`, `parse_yes_no`, `defaults`)
- Test: `crates/rustline/src/init.rs`

**Interfaces:**
- Produces: `pub fn parse_menu_choice(input: &str, n: usize, default: usize) -> Option<usize>`; `pub fn parse_yes_no(input: &str, default: bool) -> bool`; `pub fn defaults() -> InitAnswers`.

- [ ] **Step 1: Write the failing tests**

Add to `init.rs` `tests`:

```rust
    #[test]
    fn menu_choice_blank_is_default_else_one_based() {
        assert_eq!(parse_menu_choice("", 6, 2), Some(2)); // blank → default (0-based)
        assert_eq!(parse_menu_choice("  ", 6, 0), Some(0));
        assert_eq!(parse_menu_choice("1", 6, 0), Some(0)); // 1-based input → 0-based
        assert_eq!(parse_menu_choice("6", 6, 0), Some(5));
        assert_eq!(parse_menu_choice("7", 6, 0), None); // out of range
        assert_eq!(parse_menu_choice("0", 6, 0), None);
        assert_eq!(parse_menu_choice("x", 6, 0), None);
    }

    #[test]
    fn yes_no_reads_first_letter_else_default() {
        assert!(parse_yes_no("y", false));
        assert!(parse_yes_no("Yes", false));
        assert!(!parse_yes_no("n", true));
        assert!(!parse_yes_no("NO", true));
        assert!(parse_yes_no("", true)); // blank → default
        assert!(!parse_yes_no("  ", false));
        assert!(parse_yes_no("garbage", true)); // unrecognized → default
    }

    #[test]
    fn defaults_are_recommended_set() {
        let d = defaults();
        assert_eq!(d.theme, "default");
        assert!(!d.two_line);
        assert!(d.mouse);
        assert!(!d.battery && !d.tailscale && !d.lan_ip);
        assert_eq!(d.clock, ClockStyle::TwentyFour);
        assert_eq!(d.interval, 1);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rustline --lib init`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the parsers + defaults**

Add to `init.rs`:

```rust
/// Parse a 1-based menu selection into a 0-based index. Blank → `default`
/// (already 0-based); out-of-range or non-numeric → `None` (caller re-asks).
pub fn parse_menu_choice(input: &str, n: usize, default: usize) -> Option<usize> {
    let t = input.trim();
    if t.is_empty() {
        return Some(default);
    }
    match t.parse::<usize>() {
        Ok(k) if (1..=n).contains(&k) => Some(k - 1),
        _ => None,
    }
}

/// Parse a yes/no answer by its first letter (case-insensitive). Blank or
/// unrecognized → `default`.
pub fn parse_yes_no(input: &str, default: bool) -> bool {
    match input.trim().chars().next().map(|c| c.to_ascii_lowercase()) {
        Some('y') => true,
        Some('n') => false,
        _ => default,
    }
}

/// The recommended answer set used by `--defaults` (and as interactive
/// prompt defaults, except the battery question, which is pre-filled from
/// hardware detection by the interactive path).
pub fn defaults() -> InitAnswers {
    InitAnswers {
        theme: "default".into(),
        two_line: false,
        mouse: true,
        battery: false,
        tailscale: false,
        lan_ip: false,
        clock: ClockStyle::TwentyFour,
        interval: 1,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rustline --lib init`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy -p rustline --all-targets -- -D warnings
git add crates/rustline/src/init.rs
git commit -m "feat(init): prompt parsers + recommended defaults"
```

---

### Task 6: Wire the wizard — CLI args, `run`/`apply`/prompts, main dispatch

**Files:**
- Modify: `crates/rustline/src/cli.rs:32-33` (`Init` → `Init(InitArgs)`, add `InitArgs`)
- Modify: `crates/rustline/src/main.rs` (make `resolve_base_theme` `pub(crate)`; add `tmux_conf_path()`; dispatch `Command::Init(args) => init::run(...)`)
- Modify: `crates/rustline/src/theme_cmd.rs` (add `pub(crate) fn preview_named(name, themes_dir) -> Option<String>` reusing existing resolution)
- Modify: `crates/rustline/src/init.rs` (add `run`, `apply`, `prompt_answers`)
- Test: `crates/rustline/tests/smoke.rs` (integration)

**Interfaces:**
- Consumes: `tmux_conf::{init_block, InitBlockOpts, upsert_tmux_block}`, `write_config`, `defaults`, parsers, `crate::resolve_base_theme`, `crate::theme_cmd::preview_named`.
- Produces: `pub struct InitArgs { pub defaults: bool, pub print: bool }`; `pub fn run(args: &InitArgs, config_path: &Path, themes_dir: &Path, tmux_conf_path: &Path)`.

- [ ] **Step 1: Write the failing integration tests**

Add to `crates/rustline/tests/smoke.rs`:

```rust
#[test]
fn init_print_emits_block_and_writes_nothing() {
    let tmp = tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.arg("init").arg("--print");
    isolate(&mut cmd, tmp.path());
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg"));
    let out = cmd.output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("#(rustline render left"), "prints block: {s}");
    assert!(!s.contains("set -g status 2"), "one-line by default");
    // wrote no config file
    assert!(!tmp.path().join("cfg").join("rustline").join("config.toml").exists());
}

#[test]
fn init_defaults_writes_config_and_tmux_marker_block() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let run = |tmp: &Path| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
        cmd.arg("init").arg("--defaults");
        cmd.env("HOME", &home)
            .env("XDG_DATA_HOME", tmp.join("data"))
            .env("XDG_CONFIG_HOME", tmp.join("cfg"))
            .env_remove("RUST_LOG");
        cmd.output().unwrap()
    };
    let out = run(tmp.path());
    assert!(out.status.success(), "init --defaults ok: {out:?}");
    let cfg_path = tmp.path().join("cfg").join("rustline").join("config.toml");
    let cfg_text = fs::read_to_string(&cfg_path).expect("config written");
    assert!(cfg_text.contains("[theme]"), "has theme: {cfg_text}");
    let tmux_path = home.join(".tmux.conf");
    let tmux_text = fs::read_to_string(&tmux_path).expect("tmux.conf written");
    assert!(tmux_text.contains("# >>> rustline >>>"), "marker block: {tmux_text}");
    assert!(tmux_text.contains("#(rustline render left"));

    // Idempotent: a user edit outside the markers survives; the region is unchanged.
    fs::write(&tmux_path, format!("# my own line\n{tmux_text}")).unwrap();
    let before = fs::read_to_string(&tmux_path).unwrap();
    let _ = run(tmp.path());
    let after = fs::read_to_string(&tmux_path).unwrap();
    assert!(after.contains("# my own line"), "user edit preserved");
    assert_eq!(after.matches("# >>> rustline >>>").count(), 1, "no duplicate block");
    assert_eq!(before, after, "second --defaults run is a no-op on tmux.conf");
}

#[test]
fn init_non_tty_without_flags_errors() {
    let tmp = tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.arg("init"); // stdin is not a TTY under Command
    isolate(&mut cmd, tmp.path());
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg"));
    let out = cmd.output().unwrap();
    assert!(!out.status.success(), "errors without a TTY and no --defaults/--print");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--defaults") || err.contains("--print"), "hints flags: {err}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rustline --test smoke init_`
Expected: FAIL to compile (`--print`/`--defaults` unknown args; dispatch not wired).

- [ ] **Step 3: Add `InitArgs` to the CLI**

In `cli.rs`, replace the `Init` variant and add the args struct:

```rust
    /// Onboarding wizard: write config.toml + a tmux marker-block. `--defaults`
    /// runs non-interactively; `--print` emits the raw tmux block (legacy).
    Init(InitArgs),
```

```rust
/// Arguments for `rustline init`.
#[derive(Args, Default)]
pub struct InitArgs {
    /// Non-interactive: use recommended defaults and write both files.
    #[arg(long)]
    pub defaults: bool,
    /// Print the raw one-line tmux block to stdout and write nothing (legacy).
    #[arg(long)]
    pub print: bool,
}
```

- [ ] **Step 4: Expose helpers and wire main dispatch**

In `theme_cmd.rs`, add a `pub(crate)` preview resolver (factor the file-first logic already in `show`):

```rust
/// Resolve and ANSI-render a preview for theme `name` (themes-dir file first,
/// then built-in). `None` if unknown or the file is invalid.
pub(crate) fn preview_named(name: &str, themes_dir: &Path) -> Option<String> {
    let file = themes_dir.join(format!("{name}.toml"));
    if let Ok(text) = std::fs::read_to_string(&file) {
        if let Ok(tc) = toml::from_str::<ThemeConfig>(&text) {
            let mut t = Theme::default();
            tc.apply_to(&mut t);
            return Some(preview_theme_ansi(&t));
        }
        return None;
    }
    preview_ansi(name)
}
```

In `main.rs`: change `fn resolve_base_theme` to `pub(crate) fn resolve_base_theme`; add a tmux path helper; and dispatch init.

```rust
/// The user's tmux config file: `$HOME/.tmux.conf`.
fn tmux_conf_path() -> PathBuf {
    PathBuf::from(env::var("HOME").unwrap_or_default()).join(".tmux.conf")
}
```

Replace the `Command::Init` arm (from Task 1) with:

```rust
        Command::Init(args) => {
            init::run(&args, &config_path(), &themes_dir(), &tmux_conf_path());
        }
```

- [ ] **Step 5: Implement `run`, `apply`, `prompt_answers` in `init.rs`**

Add imports at the top of `init.rs`:

```rust
use std::io::{IsTerminal, Write as _};

use crate::cli::InitArgs;
use crate::tmux_conf::{self, InitBlockOpts};
```

```rust
/// Entry point for `rustline init`. `--print` wins (emit the legacy raw
/// one-line block, write nothing). Else gather answers (`--defaults` or the
/// interactive prompt), then write both files. A non-interactive invocation
/// (stdin not a TTY) without a flag errors rather than writing silently.
pub fn run(args: &InitArgs, config_path: &Path, themes_dir: &Path, tmux_conf_path: &Path) {
    if args.print {
        let theme = crate::resolve_base_theme("default").unwrap_or_default();
        let bar_bg = theme.bar_bg.to_tmux();
        let fg = theme.fg.to_tmux();
        print!(
            "{}",
            tmux_conf::init_block(&InitBlockOpts {
                bar_bg: &bar_bg,
                fg: &fg,
                two_line: false,
                mouse: false,
                interval: 1,
            })
        );
        return;
    }
    let answers = if args.defaults {
        defaults()
    } else if std::io::stdin().is_terminal() {
        prompt_answers(themes_dir)
    } else {
        eprintln!(
            "rustline init needs a terminal for the interactive wizard.\n\
             Use `rustline init --defaults` for recommended settings, or \
             `rustline init --print` to emit the raw tmux block."
        );
        std::process::exit(2);
    };
    apply(&answers, config_path, themes_dir, tmux_conf_path);
}

/// Write `config.toml` (non-destructive) and upsert the tmux block, backing up
/// each existing file first. Prints a summary + next step to stderr.
fn apply(a: &InitAnswers, config_path: &Path, _themes_dir: &Path, tmux_conf_path: &Path) {
    match write_config(a, config_path) {
        Ok(bak) => {
            eprint!("Wrote {}", config_path.display());
            if !bak.as_os_str().is_empty() {
                eprint!(" (backup: {})", bak.display());
            }
            eprintln!();
        }
        Err(e) => {
            eprintln!("failed to write {}: {e}", config_path.display());
            std::process::exit(1);
        }
    }

    let theme = crate::resolve_base_theme(&a.theme).unwrap_or_default();
    let bar_bg = theme.bar_bg.to_tmux();
    let fg = theme.fg.to_tmux();
    let block = tmux_conf::init_block(&InitBlockOpts {
        bar_bg: &bar_bg,
        fg: &fg,
        two_line: a.two_line,
        mouse: a.mouse,
        interval: a.interval,
    });
    let existing = fs::read_to_string(tmux_conf_path).unwrap_or_default();
    if !existing.is_empty() {
        let mut bak = tmux_conf_path.as_os_str().to_owned();
        bak.push(".rustline.bak");
        if let Err(e) = fs::write(PathBuf::from(bak), &existing) {
            eprintln!("failed to back up {}: {e}", tmux_conf_path.display());
            std::process::exit(1);
        }
    }
    let updated = upsert_tmux_block_for(&existing, &block);
    if let Err(e) = fs::write(tmux_conf_path, updated) {
        eprintln!("failed to write {}: {e}", tmux_conf_path.display());
        std::process::exit(1);
    }
    eprintln!(
        "Updated {}. Reload tmux with:  tmux source-file {}",
        tmux_conf_path.display(),
        tmux_conf_path.display()
    );
}

/// Thin alias so `apply` reads cleanly (and to keep the import local).
fn upsert_tmux_block_for(existing: &str, block: &str) -> String {
    crate::tmux_conf::upsert_tmux_block(existing, block)
}

/// Read a single trimmed line from stdin (empty string on EOF).
fn read_line() -> String {
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
    s
}

/// Prompt on stderr; loop the theme menu until a valid pick, then ask the
/// remaining questions. I/O-heavy; the parsing/defaulting it delegates to is
/// unit-tested (`parse_menu_choice`/`parse_yes_no`).
fn prompt_answers(themes_dir: &Path) -> InitAnswers {
    let mut a = defaults();
    let stderr = std::io::stderr();

    // Theme
    let mut themes: Vec<String> =
        rustline_core::builtin_theme_names().iter().map(|s| s.to_string()).collect();
    for f in crate::theme_cmd_theme_files(themes_dir) {
        if !themes.contains(&f) {
            themes.push(f);
        }
    }
    loop {
        let mut w = stderr.lock();
        let _ = writeln!(w, "\nChoose a theme:");
        for (i, name) in themes.iter().enumerate() {
            let _ = writeln!(w, "  {}) {name}", i + 1);
        }
        let _ = write!(w, "Theme [1]: ");
        let _ = w.flush();
        drop(w);
        if let Some(idx) = parse_menu_choice(&read_line(), themes.len(), 0) {
            a.theme = themes[idx].clone();
            if let Some(preview) = crate::theme_cmd::preview_named(&a.theme, themes_dir) {
                eprintln!("\n{preview}\n");
            }
            if ask("Use this theme?", true) {
                break;
            }
        } else {
            eprintln!("Please enter a number from the list.");
        }
    }

    a.two_line = ask("Two-line status (window list on its own line)?", false);
    a.mouse = ask("Enable mouse for click-to-toggle widgets?", true);
    a.battery = ask("Laptop — show battery?", crate::battery::read_battery().is_some());
    a.tailscale = ask("On a Tailscale network — show Tailscale IP?", false);
    a.lan_ip = ask("Show LAN IP?", false);

    // Clock
    let clocks = [
        ("24-hour            (14:05)", ClockStyle::TwentyFour),
        ("24-hour + seconds  (14:05:09)", ClockStyle::TwentyFourSeconds),
        ("12-hour            (02:05 PM)", ClockStyle::Twelve),
        ("12-hour + seconds  (02:05:09 PM)", ClockStyle::TwelveSeconds),
    ];
    loop {
        let mut w = stderr.lock();
        let _ = writeln!(w, "\nClock style:");
        for (i, (label, _)) in clocks.iter().enumerate() {
            let _ = writeln!(w, "  {}) {label}", i + 1);
        }
        let _ = write!(w, "Clock [1]: ");
        let _ = w.flush();
        drop(w);
        if let Some(idx) = parse_menu_choice(&read_line(), clocks.len(), 0) {
            a.clock = clocks[idx].1;
            break;
        }
        eprintln!("Please enter a number from the list.");
    }

    a.interval = if ask("Fast refresh (1s)? (No = 5s)", true) { 1 } else { 5 };
    a
}

/// Ask a yes/no on stderr with a shown default; returns the parsed answer.
fn ask(question: &str, default: bool) -> bool {
    let d = if default { "Y/n" } else { "y/N" };
    eprint!("{question} [{d}]: ");
    let _ = std::io::stderr().flush();
    parse_yes_no(&read_line(), default)
}
```

Note: `crate::theme_cmd_theme_files` above is a placeholder for reusing the themes-dir listing — instead call the existing `crate::theme_cmd`'s file lister. `theme_cmd::theme_files` is currently private; add `pub(crate)` to its signature (`pub(crate) fn theme_files(themes_dir: &Path) -> Vec<String>`) and call `crate::theme_cmd::theme_files(themes_dir)` here (replace the `theme_cmd_theme_files` call). Also add `mod battery;` is already present in main; `crate::battery::read_battery()` is accessible.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p rustline`
Expected: PASS (lib unit tests + smoke integration, incl. the three new `init_*` tests). If clippy flags `upsert_tmux_block_for` as needless, inline it and call `crate::tmux_conf::upsert_tmux_block` directly.

- [ ] **Step 7: fmt + clippy + full test + commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test
git add crates/rustline/src/cli.rs crates/rustline/src/main.rs crates/rustline/src/theme_cmd.rs crates/rustline/src/init.rs crates/rustline/tests/smoke.rs
git commit -m "feat(init): interactive onboarding wizard wired to \`rustline init\`"
```

---

### Task 7: Documentation

**Files:**
- Modify: `README.md` ("Enable in tmux" section, ~lines 44-66)
- Modify: `CLAUDE.md` (CLI section `rustline init`; Module map `main.rs`/`tmux_conf.rs`/new `init.rs`/`assets/`; Roadmap "Done" entry)
- Modify: `/home/steve/.claude/projects/-home-steve-src-rustline/memory/MEMORY.md` + the widget/plugin-list memory (only if the doc-list convention applies — verify)

**Interfaces:** none (docs only).

- [ ] **Step 1: Update README "Enable in tmux"**

Replace the code block + prose (README.md:44-66) with wizard-first guidance:

````markdown
## Enable in tmux

Run the onboarding wizard — it asks a few questions (theme, one- or two-line
status, mouse/click-to-toggle, which widgets, clock style, refresh rate), then
writes `~/.config/rustline/config.toml` and adds a managed block to
`~/.tmux.conf` (backed up first):

```bash
rustline init
tmux source-file ~/.tmux.conf
```

- `rustline init --defaults` — non-interactive; recommended settings.
- `rustline init --print` — just print the raw one-line tmux block to stdout and
  write nothing (the pre-wizard behavior, handy for scripting:
  `rustline init --print >> ~/.tmux.conf`).

The tmux block is wrapped in `# >>> rustline >>>` / `# <<< rustline <<<` markers,
so re-running `rustline init` replaces that region instead of appending a
duplicate; your edits outside the markers are preserved.
````

Keep the existing Font/tmux requirement callouts. Update the tmux-requirement note: the wizard can enable `set -g mouse on` for you (the mouse/click-to-toggle question).

- [ ] **Step 2: Update CLAUDE.md**

- CLI section: change the `rustline init` bullet to describe the wizard + `--defaults`/`--print`, the two files it writes, and the marker block.
- Module map: note `tmux_conf.rs` now exposes `init_block(&InitBlockOpts)` (one/two-line, mouse, interval) and `upsert_tmux_block` (idempotent marker block); add `init.rs` (the wizard shell + `starter_config_toml`/`merge_config`/`write_config`/prompt parsers) and `assets/starter-config.toml` (embedded starter defaults, `include_str!`); note `main.rs` gains `tmux_conf_path()` and `resolve_base_theme` is `pub(crate)`.
- Roadmap: add a "Done: `init` onboarding wizard" entry linking the spec/plan.

- [ ] **Step 3: Verify + commit**

```bash
grep -n "rustline init" README.md CLAUDE.md
cargo test   # docs change nothing, but confirm still green
git add README.md CLAUDE.md
git commit -m "docs: init onboarding wizard (README + CLAUDE.md)"
```

---

## Self-Review

**Spec coverage:**
- Wizard becomes `init`, `--defaults`, `--print`, non-TTY error → Task 6.
- Six question set (theme, one/two-line, mouse, machine-type widgets, clock, interval) → prompts in Task 6; answer type + defaults in Tasks 2/5.
- Embedded starter template + mutations (theme/layout/clock/prune) → Task 2.
- Non-destructive config merge (theme always set; layout/widgets add-if-absent) + backup → Task 3.
- Extended `init_block` (one/two-line, mouse, interval) with injection-safety + byte-identical one-line → Task 1.
- Idempotent tmux marker block + backup → Task 4 (upsert) + Task 6 (backup/write).
- Theme preview during selection → Task 6 (`preview_named`).
- Docs → Task 7.

**Placeholder scan:** The `theme_cmd_theme_files` reference in Task 6 Step 5 is explicitly called out in the following note as a stand-in for `crate::theme_cmd::theme_files` (made `pub(crate)`); resolve it there. No other placeholders.

**Type consistency:** `InitAnswers`/`ClockStyle`/`InitBlockOpts` field names and `starter_config_toml`/`merge_config`/`write_config`/`upsert_tmux_block`/`parse_menu_choice`/`parse_yes_no`/`defaults`/`run` signatures are consistent across tasks. `set_base`, `theme_files`, `preview_named`, `resolve_base_theme` visibility bumps to `pub(crate)` are each specified in the task that first needs them.
