//! `rustline init` onboarding wizard: gathers a few answers and writes a
//! tailored `config.toml` plus an idempotent tmux marker-block. Pure helpers
//! (template mutation, config merge, prompt parsing) are unit-tested; the
//! interactive prompt loop is a thin I/O shell over them.

use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{Array, DocumentMut, Item, Table, value};

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
}
