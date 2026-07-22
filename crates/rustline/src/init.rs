//! `rustline init` onboarding wizard: gathers a few answers and writes a
//! tailored `config.toml` plus an idempotent tmux marker-block. Pure helpers
//! (template mutation, config merge, prompt parsing) are unit-tested; the
//! interactive prompt loop is a thin I/O shell over them.

use std::fs;
use std::io::{IsTerminal, Write as _};
use std::path::{Path, PathBuf};

use rustline_core::builtin_theme_names;
use toml_edit::{Array, DocumentMut, Item, Table, value};

use crate::cli::InitArgs;
use crate::tmux_conf::{self, InitBlockOpts};

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
    let mut doc: DocumentMut = STARTER_TEMPLATE
        .parse()
        .expect("embedded template is valid TOML");

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

/// Merge the generated starter into an existing config **non-destructively**:
/// `[theme].base` is always (re)set to `theme`; `[layout]` and each
/// `[widgets.<name>]` table are added only if absent. Returns the merged TOML,
/// or `Err` if `existing` is not valid TOML, or if it has a `theme` key that
/// isn't a table (either way, the caller must not overwrite it).
pub fn merge_config(existing: &str, generated: &str, theme: &str) -> Result<String, String> {
    let mut doc: DocumentMut = existing
        .parse()
        .map_err(|e| format!("existing config is not valid TOML: {e}"))?;
    let generated_doc: DocumentMut = generated
        .parse()
        .map_err(|e| format!("generated config invalid (bug): {e}"))?;

    // `theme_cmd::set_base` hard-exits the process if `[theme]` exists but
    // isn't a table (e.g. a scalar `theme = "dark"`); that would bypass this
    // function's `Result` contract, so reject that case ourselves first.
    if let Some(existing_theme) = doc.get("theme")
        && !existing_theme.is_table()
    {
        return Err("existing [theme] is not a table".to_string());
    }

    crate::theme_cmd::set_base(&mut doc, theme);

    if doc.get("layout").is_none()
        && let Some(layout) = generated_doc.get("layout")
    {
        doc["layout"] = layout.clone();
    }

    if let Some(generated_widgets) = generated_doc.get("widgets").and_then(Item::as_table) {
        let existing_widgets = doc.entry("widgets").or_insert(Item::Table(Table::new()));
        if let Some(existing_widgets) = existing_widgets.as_table_mut() {
            existing_widgets.set_implicit(false);
            for (name, table) in generated_widgets.iter() {
                if !existing_widgets.contains_key(name) {
                    existing_widgets.insert(name, table.clone());
                }
            }
        }
    }

    Ok(doc.to_string())
}

/// Sibling backup path `<config_path>.rustline.bak`, e.g.
/// `config.toml.rustline.bak`.
fn backup_path(config_path: &Path) -> PathBuf {
    let mut name = config_path.as_os_str().to_owned();
    name.push(".rustline.bak");
    PathBuf::from(name)
}

/// Write the tailored config to `config_path`. A missing file gets the full
/// generated starter (parent dirs created as needed). An existing, readable
/// file is first backed up to `<config_path>.rustline.bak`, then overwritten
/// with the non-destructive merge (see [`merge_config`]), returning the
/// backup path (an empty `PathBuf` when there was no pre-existing file). Any
/// other read failure (permission denied, non-UTF8 contents, an unparseable
/// `[theme]`/TOML) is propagated as `Err` and the file is left untouched —
/// this must never silently clobber a config it couldn't safely read.
pub fn write_config(a: &InitAnswers, config_path: &Path) -> std::io::Result<PathBuf> {
    let generated = starter_config_toml(a);
    match fs::read_to_string(config_path) {
        Ok(existing) => {
            let merged = merge_config(&existing, &generated, &a.theme)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            let backup = backup_path(config_path);
            fs::write(&backup, &existing)?;
            fs::write(config_path, merged)?;
            Ok(backup)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = config_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(config_path, generated)?;
            Ok(PathBuf::new())
        }
        Err(e) => Err(e),
    }
}

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

/// Entry point for `rustline init`. `--print` wins (emit the legacy raw
/// one-line block, write nothing) using the caller's already-resolved
/// `current_theme` (`[theme].base` plus any inline `[theme]` overrides), so
/// its `status-style` colors stay byte-identical to today's `rustline init`.
/// Else gather answers (`--defaults` or the interactive prompt), then write
/// both files. A non-interactive invocation (stdin not a TTY) without a flag
/// errors rather than writing silently.
pub fn run(
    args: &InitArgs,
    config_path: &Path,
    themes_dir: &Path,
    tmux_conf_path: &Path,
    current_theme: &rustline_core::Theme,
) {
    if args.print {
        let bar_bg = current_theme.bar_bg.to_tmux();
        let fg = current_theme.fg.to_tmux();
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
    apply(&answers, config_path, tmux_conf_path);
}

/// Write `config.toml` (non-destructive) and upsert the tmux block, backing up
/// each existing file first. Prints a summary + next step to stderr. A present
/// but unreadable `tmux_conf_path` (e.g. non-UTF8 contents) aborts rather than
/// collapsing the read error to empty, which would silently skip the backup
/// and overwrite the file the caller couldn't safely read.
fn apply(a: &InitAnswers, config_path: &Path, tmux_conf_path: &Path) {
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
    let existing = match fs::read_to_string(tmux_conf_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            eprintln!(
                "failed to read {}: {e}; refusing to overwrite",
                tmux_conf_path.display()
            );
            std::process::exit(1);
        }
    };
    if !existing.is_empty() {
        let mut bak = tmux_conf_path.as_os_str().to_owned();
        bak.push(".rustline.bak");
        if let Err(e) = fs::write(PathBuf::from(bak), &existing) {
            eprintln!("failed to back up {}: {e}", tmux_conf_path.display());
            std::process::exit(1);
        }
    }
    let updated = tmux_conf::upsert_tmux_block(&existing, &block);
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

/// Read a line from stdin (untrimmed — callers trim via `parse_menu_choice`/
/// `parse_yes_no`); empty string on EOF.
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
    let mut themes: Vec<String> = builtin_theme_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    for f in crate::theme_cmd::theme_files(themes_dir) {
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
    a.battery = ask(
        "Laptop — show battery?",
        crate::battery::read_battery().is_some(),
    );
    a.tailscale = ask("On a Tailscale network — show Tailscale IP?", false);
    a.lan_ip = ask("Show LAN IP?", false);

    // Clock
    let clocks = [
        ("24-hour            (14:05)", ClockStyle::TwentyFour),
        (
            "24-hour + seconds  (14:05:09)",
            ClockStyle::TwentyFourSeconds,
        ),
        ("12-hour            (02:05 PM)", ClockStyle::Twelve),
        (
            "12-hour + seconds  (02:05:09 PM)",
            ClockStyle::TwelveSeconds,
        ),
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

    a.interval = if ask("Fast refresh (1s)? (No = 5s)", true) {
        1
    } else {
        5
    };
    a
}

/// Ask a yes/no on stderr with a shown default; returns the parsed answer.
fn ask(question: &str, default: bool) -> bool {
    let d = if default { "Y/n" } else { "y/N" };
    eprint!("{question} [{d}]: ");
    let _ = std::io::stderr().flush();
    parse_yes_no(&read_line(), default)
}

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
        assert_eq!(
            ClockStyle::TwentyFour.formats(),
            ("%a %Y-%m-%d %H:%M", "%m-%d %H:%M")
        );
        assert_eq!(
            ClockStyle::TwentyFourSeconds.formats(),
            ("%a %Y-%m-%d %H:%M:%S", "%m-%d %H:%M:%S")
        );
        assert_eq!(
            ClockStyle::Twelve.formats(),
            ("%a %Y-%m-%d %I:%M %p", "%m-%d %I:%M %p")
        );
        assert_eq!(
            ClockStyle::TwelveSeconds.formats(),
            ("%a %Y-%m-%d %I:%M:%S %p", "%m-%d %I:%M:%S %p")
        );
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
        assert_eq!(
            cfg.layout.right,
            vec!["cwd", "cpu", "memory", "battery", "loadavg", "datetime"]
        );
        assert_eq!(cfg.layout.left, vec!["pane_id", "hostname", "tailscale_ip"]);
    }

    #[test]
    fn unselected_optional_widget_sections_are_pruned() {
        let a = base_answers(); // all optional off
        let toml = starter_config_toml(&a);
        assert!(
            !toml.contains("[widgets.battery]"),
            "battery pruned: {toml}"
        );
        assert!(!toml.contains("[widgets.lan_ip]"), "lan_ip pruned: {toml}");
        assert!(
            !toml.contains("[widgets.tailscale_ip]"),
            "tailscale pruned: {toml}"
        );
        // required widget sections remain
        assert!(toml.contains("[widgets.cpu]"));
    }

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
    fn merge_config_rejects_non_table_theme() {
        // A scalar `theme = "dark"` isn't a table `set_base` can mutate; if
        // this ran through to `set_base` it would `process::exit(1)` instead
        // of returning an `Err` — running at all (rather than aborting the
        // test process) proves that path is pre-empted.
        let out = merge_config(
            "theme = \"dark\"\n",
            &starter_config_toml(&base_answers()),
            "nord",
        );
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

    #[test]
    fn write_config_does_not_clobber_unreadable_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let original = [0xff_u8, 0xfe, 0x00];
        std::fs::write(&path, original).unwrap();
        let result = write_config(&base_answers(), &path);
        assert!(result.is_err(), "non-UTF8 read failure must propagate");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            original,
            "existing file must be untouched"
        );
        let bak = super::backup_path(&path);
        assert!(!bak.exists(), "no backup should be written: {bak:?}");
    }

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
}
