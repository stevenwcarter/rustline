//! User-facing TOML configuration: layout, per-widget options, and theme
//! overrides.
//!
//! [`Config::load`] is total — a missing file or a parse error both fall
//! back to [`Config::default`] (the spec-defined layout) rather than
//! panicking, so a bad or absent config file never takes down the status
//! line.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use toml::Value;

use crate::Color;
use crate::render::Theme;

/// Which widgets render in each region of the status bar, by name.
///
/// Names are resolved against a [`crate::widget::Registry`] at render time;
/// an unknown name is skipped there, not a config error.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default = "default_left")]
    pub left: Vec<String>,
    #[serde(default = "default_center")]
    pub center: Vec<String>,
    #[serde(default = "default_right")]
    pub right: Vec<String>,
}

fn default_left() -> Vec<String> {
    vec!["pane_id".into(), "hostname".into()]
}

fn default_center() -> Vec<String> {
    vec!["windows".into()]
}

fn default_right() -> Vec<String> {
    vec!["cwd".into(), "loadavg".into(), "datetime".into()]
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            left: default_left(),
            center: default_center(),
            right: default_right(),
        }
    }
}

/// Options for the `datetime` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DateTimeOpts {
    #[serde(default = "default_dt_format")]
    pub format: String,
}

fn default_dt_format() -> String {
    "%a < %Y-%m-%d < %H:%M".into()
}

impl Default for DateTimeOpts {
    fn default() -> Self {
        Self {
            format: default_dt_format(),
        }
    }
}

/// Options for the `cwd` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CwdOpts {
    #[serde(default = "default_true")]
    pub abbreviate_home: bool,
}

fn default_true() -> bool {
    true
}

impl Default for CwdOpts {
    fn default() -> Self {
        Self {
            abbreviate_home: true,
        }
    }
}

/// Per-widget option overrides, keyed by widget name.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WidgetOpts {
    #[serde(default)]
    pub datetime: DateTimeOpts,
    #[serde(default)]
    pub cwd: CwdOpts,
}

/// Optional theme overrides layered onto [`Theme::default`] by
/// [`Config::to_theme`]; `None` means "keep the default value".
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ThemeConfig {
    #[serde(default)]
    pub palette: Option<Vec<Color>>,
    #[serde(default)]
    pub fg: Option<Color>,
    #[serde(default)]
    pub bar_bg: Option<Color>,
}

/// The full user-facing configuration, loaded from a `rustline.toml`.
///
/// Every field, and every nested field, is `#[serde(default)]`, so a config
/// file may specify any subset of the tree; anything absent falls back to
/// the spec defaults. Use [`Config::load`] rather than parsing directly to
/// get the total (never-panics) behavior.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub layout: Layout,
    #[serde(default)]
    pub theme: ThemeConfig,
    #[serde(default)]
    pub widgets: WidgetOpts,
    /// Plugin config tables, keyed by `"owner/repo"`, kept as raw TOML for
    /// future WASM plugins to interpret for themselves.
    #[serde(default)]
    pub plugins: HashMap<String, Value>,
}

impl Config {
    /// Load config from `path`, never failing: a missing file or a parse
    /// error both yield [`Config::default`] (the latter after logging a
    /// warning), so the status line keeps rendering.
    pub fn load(path: &Path) -> Config {
        let Ok(text) = fs::read_to_string(path) else {
            return Config::default();
        };
        match toml::from_str(&text) {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "invalid config, using defaults");
                Config::default()
            }
        }
    }

    /// Build a [`Theme`] by layering this config's overrides onto
    /// [`Theme::default`]; unset fields keep the default value.
    pub fn to_theme(&self) -> Theme {
        let mut theme = Theme::default();
        if let Some(palette) = &self.theme.palette {
            theme.palette = palette.clone();
        }
        if let Some(fg) = &self.theme.fg {
            theme.fg = fg.clone();
        }
        if let Some(bar_bg) = &self.theme.bar_bg {
            theme.bar_bg = bar_bg.clone();
        }
        theme
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_matches_spec() {
        let c = Config::default();
        assert_eq!(c.layout.left, vec!["pane_id", "hostname"]);
        assert_eq!(c.layout.center, vec!["windows"]);
        assert_eq!(c.layout.right, vec!["cwd", "loadavg", "datetime"]);
    }

    #[test]
    fn parse_overrides_layout_and_datetime() {
        let toml = r#"
[layout]
right = ["datetime"]
[widgets.datetime]
format = "%H:%M"
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.layout.right, vec!["datetime"]);
        assert_eq!(c.widgets.datetime.format, "%H:%M");
        // unspecified region falls back to default
        assert_eq!(c.layout.left, vec!["pane_id", "hostname"]);
    }

    #[test]
    fn malformed_load_falls_back_to_default() {
        let dir = std::env::temp_dir().join("rustline_test_badcfg");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        std::fs::write(&p, "this is not = valid = toml [[[").unwrap();
        let c = Config::load(&p);
        assert_eq!(c.layout.left, Config::default().layout.left);
    }

    #[test]
    fn missing_file_is_default() {
        let c = Config::load(std::path::Path::new("/no/such/rustline.toml"));
        assert_eq!(c.layout.center, vec!["windows"]);
    }

    #[test]
    fn plugins_table_retained() {
        let toml = r#"
[plugins."owner/repo"]
key = "value"
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert!(c.plugins.contains_key("owner/repo"));
    }
}
