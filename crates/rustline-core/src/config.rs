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

/// Per-plugin configuration, keyed by plugin name in [`Config::plugins`].
///
/// Capability fields (`allowed_urls`, `allowed_paths`, `max_state_bytes`) are
/// enforced by the WASM host, never by the guest. `options` is opaque to the
/// host and forwarded to the plugin verbatim.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginConfig {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub allowed_urls: Vec<String>,
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    #[serde(default = "default_max_state_bytes")]
    pub max_state_bytes: u64,
    #[serde(default = "empty_table")]
    pub options: Value,
}

/// 50 MB — the default per-plugin state-directory quota.
fn default_max_state_bytes() -> u64 {
    52_428_800
}

fn empty_table() -> Value {
    Value::Table(toml::map::Map::new())
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            source: None,
            allowed_urls: Vec::new(),
            allowed_paths: Vec::new(),
            max_state_bytes: default_max_state_bytes(),
            options: empty_table(),
        }
    }
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
    /// Directory to discover `*.wasm` plugins from; overrides the default
    /// `$XDG_DATA_HOME/rustline/plugins`. A `--plugin-dir` CLI flag overrides
    /// this in turn.
    #[serde(default)]
    pub plugin_dir: Option<String>,
    /// Per-plugin config, keyed by plugin name.
    #[serde(default)]
    pub plugins: HashMap<String, PluginConfig>,
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
    fn plugin_config_typed_parse_with_defaults() {
        let toml = r#"
plugin_dir = "~/.local/share/rustline/plugins"
[plugins.weather]
source = "steve/rustline-weather"
allowed_urls = ["https://wttr.in/*"]
[plugins.weather.options]
zip = "48183"
format = "{icon} {temp_f}°F"
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(
            c.plugin_dir.as_deref(),
            Some("~/.local/share/rustline/plugins")
        );
        let w = c.plugins.get("weather").expect("weather entry");
        assert_eq!(w.source.as_deref(), Some("steve/rustline-weather"));
        assert_eq!(w.allowed_urls, vec!["https://wttr.in/*".to_string()]);
        assert!(w.allowed_paths.is_empty());
        // omitted -> default 50 MB
        assert_eq!(w.max_state_bytes, 52_428_800);
        assert_eq!(w.options.get("zip").and_then(Value::as_str), Some("48183"));
    }

    #[test]
    fn plugin_config_roundtrip_preserves_options() {
        let src = r#"
[plugins.weather]
allowed_urls = ["https://wttr.in/*"]
max_state_bytes = 100
[plugins.weather.options]
zip = "48183"
"#;
        let c: Config = toml::from_str(src).unwrap();
        let serialized = toml::to_string(&c).unwrap();
        let back: Config = toml::from_str(&serialized).unwrap();
        let w = back.plugins.get("weather").unwrap();
        assert_eq!(w.max_state_bytes, 100);
        assert_eq!(w.allowed_urls, vec!["https://wttr.in/*".to_string()]);
        assert_eq!(w.options.get("zip").and_then(Value::as_str), Some("48183"));
    }

    #[test]
    fn malformed_plugins_table_falls_back_to_default() {
        let dir = std::env::temp_dir().join("rustline_test_badplugins");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        // max_state_bytes must be an integer; a string makes the table invalid
        std::fs::write(&p, "[plugins.weather]\nmax_state_bytes = \"lots\"\n").unwrap();
        let c = Config::load(&p);
        assert!(c.plugins.is_empty());
        assert_eq!(c.layout.left, Config::default().layout.left);
    }
}
