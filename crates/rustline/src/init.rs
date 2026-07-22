//! `rustline init` onboarding wizard: gathers a few answers and writes a
//! tailored `config.toml` plus an idempotent tmux marker-block. Pure helpers
//! (template mutation, config merge, prompt parsing) are unit-tested; the
//! interactive prompt loop is a thin I/O shell over them.

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
}
