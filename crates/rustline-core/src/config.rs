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

/// Default `format` for the IP widgets: the bare address, no label.
fn default_ip_format() -> String {
    "{ip}".into()
}

/// Options for the `lan_ip` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LanIpOpts {
    #[serde(default = "default_ip_format")]
    pub format: String,
    #[serde(default)]
    pub down_format: String,
    #[serde(default)]
    pub interface: Option<String>,
}

impl Default for LanIpOpts {
    fn default() -> Self {
        Self {
            format: default_ip_format(),
            down_format: String::new(),
            interface: None,
        }
    }
}

/// Options for the `tailscale_ip` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TailscaleIpOpts {
    #[serde(default = "default_ip_format")]
    pub format: String,
    #[serde(default)]
    pub down_format: String,
}

impl Default for TailscaleIpOpts {
    fn default() -> Self {
        Self {
            format: default_ip_format(),
            down_format: String::new(),
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
    #[serde(default)]
    pub lan_ip: LanIpOpts,
    #[serde(default)]
    pub tailscale_ip: TailscaleIpOpts,
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

/// Logging configuration: per-sink level thresholds and an optional log-file
/// path override. Level strings are parsed leniently by the binary — an
/// unknown value falls back to that sink's default rather than failing the
/// whole config parse, so `Config::load` stays total (invariant #3). Do NOT
/// promote these to an enum: a `#[derive(Deserialize)]` enum would make a
/// typo in `file_level` discard the entire config (layout, theme, plugins).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogConfig {
    /// File-sink level: off|error|warn|info|debug|trace. Default "info".
    #[serde(default = "default_file_level")]
    pub file_level: String,
    /// stderr-sink level: off|error|warn|info|debug|trace. Default "error".
    #[serde(default = "default_stderr_level")]
    pub stderr_level: String,
    /// Log-file path override (`~/` expanded by the binary). Default:
    /// `$XDG_DATA_HOME/rustline/rustline.log`.
    #[serde(default)]
    pub file: Option<String>,
}

fn default_file_level() -> String {
    "info".into()
}

fn default_stderr_level() -> String {
    "error".into()
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            file_level: default_file_level(),
            stderr_level: default_stderr_level(),
            file: None,
        }
    }
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
    /// File + stderr logging configuration.
    #[serde(default)]
    pub log: LogConfig,
}

impl Config {
    /// Load config from `path`, never failing: a missing file or a parse
    /// error both yield [`Config::default`] (the latter after logging a
    /// warning), so the status line keeps rendering.
    pub fn load(path: &Path) -> Config {
        let (config, warning) = Config::load_reporting(path);
        if let Some(msg) = warning {
            tracing::warn!("{msg}");
        }
        config
    }

    /// Like [`Config::load`] but *returns* the failure message instead of
    /// logging it, so a caller can install its logging subscriber first and
    /// then emit the warning into it. `None` = success or an absent file
    /// (absence is not a warning); `Some(msg)` = a present-but-unparseable
    /// file (config defaulted).
    pub fn load_reporting(path: &Path) -> (Config, Option<String>) {
        let Ok(text) = fs::read_to_string(path) else {
            return (Config::default(), None);
        };
        match toml::from_str(&text) {
            Ok(config) => (config, None),
            Err(error) => (
                Config::default(),
                Some(format!(
                    "invalid config at {}: {error}; using defaults",
                    path.display()
                )),
            ),
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
    use std::io::Write;
    use tempfile::NamedTempFile;

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

    #[test]
    fn ip_widget_opts_parse_with_defaults() {
        let toml = r#"
[widgets.lan_ip]
format = "LAN {ip}"
interface = "wlp3s0"
[widgets.tailscale_ip]
down_format = "TS off"
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.widgets.lan_ip.format, "LAN {ip}");
        assert_eq!(c.widgets.lan_ip.interface.as_deref(), Some("wlp3s0"));
        // omitted -> defaults
        assert_eq!(c.widgets.lan_ip.down_format, "");
        assert_eq!(c.widgets.tailscale_ip.format, "{ip}");
        assert_eq!(c.widgets.tailscale_ip.down_format, "TS off");
    }

    #[test]
    fn ip_widget_opts_default_when_absent() {
        let c = Config::default();
        assert_eq!(c.widgets.lan_ip.format, "{ip}");
        assert_eq!(c.widgets.lan_ip.down_format, "");
        assert_eq!(c.widgets.lan_ip.interface, None);
        assert_eq!(c.widgets.tailscale_ip.format, "{ip}");
    }

    #[test]
    fn log_config_defaults_when_absent() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.log.file_level, "info");
        assert_eq!(c.log.stderr_level, "error");
        assert_eq!(c.log.file, None);
    }

    #[test]
    fn log_config_partial_keeps_other_defaults() {
        let c: Config = toml::from_str("[log]\nfile_level = \"debug\"\n").unwrap();
        assert_eq!(c.log.file_level, "debug");
        assert_eq!(c.log.stderr_level, "error"); // untouched
        assert_eq!(c.log.file, None);
    }

    #[test]
    fn load_reporting_ok_has_no_warning() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "[log]\nfile_level = \"trace\"\n").unwrap();
        let (cfg, warn) = Config::load_reporting(f.path());
        assert_eq!(cfg.log.file_level, "trace");
        assert!(warn.is_none());
    }

    #[test]
    fn load_reporting_bad_file_defaults_with_warning() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "this is = = not valid toml [[[").unwrap();
        let (cfg, warn) = Config::load_reporting(f.path());
        assert_eq!(cfg.log.file_level, "info"); // fell back to default
        assert!(warn.is_some());
    }

    #[test]
    fn load_reporting_absent_file_is_not_a_warning() {
        let (cfg, warn) = Config::load_reporting(Path::new("/no/such/rustline/config.toml"));
        assert_eq!(cfg.log.file_level, "info");
        assert!(warn.is_none());
    }
}
