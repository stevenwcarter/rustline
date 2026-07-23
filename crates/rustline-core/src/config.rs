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
    vec![
        "cwd".into(),
        "cpu".into(),
        "memory".into(),
        "loadavg".into(),
        "datetime".into(),
    ]
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
    #[serde(default)]
    pub alt_format: String,
}

fn default_dt_format() -> String {
    "%a < %Y-%m-%d < %H:%M".into()
}

impl Default for DateTimeOpts {
    fn default() -> Self {
        Self {
            format: default_dt_format(),
            alt_format: String::new(),
        }
    }
}

/// Default `format` for the `hostname` widget: the bare truncated hostname,
/// reproducing the pre-config output byte-for-byte.
fn default_hostname_format() -> String {
    "{host}".into()
}

/// Options for the `hostname` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostnameOpts {
    #[serde(default = "default_hostname_format")]
    pub format: String,
}

impl Default for HostnameOpts {
    fn default() -> Self {
        Self {
            format: default_hostname_format(),
        }
    }
}

/// Default `format` for the `pane_id` widget: `session:window.pane`,
/// reproducing the pre-config output byte-for-byte.
fn default_pane_id_format() -> String {
    "{session}:{window}.{pane}".into()
}

/// Options for the `pane_id` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaneIdOpts {
    #[serde(default = "default_pane_id_format")]
    pub format: String,
}

impl Default for PaneIdOpts {
    fn default() -> Self {
        Self {
            format: default_pane_id_format(),
        }
    }
}

/// Default `format` for the `cwd` widget: the bare (home-abbreviated) path,
/// reproducing the pre-config output byte-for-byte.
fn default_cwd_format() -> String {
    "{path}".into()
}

/// Options for the `cwd` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CwdOpts {
    #[serde(default = "default_true")]
    pub abbreviate_home: bool,
    #[serde(default = "default_cwd_format")]
    pub format: String,
    /// Keep only the last N `/`-separated path components (`0` = unlimited),
    /// prefixing a leading `…/` when components are dropped.
    #[serde(default)]
    pub max_depth: usize,
    /// Left-truncate the rendered path to at most N characters (`0` =
    /// unlimited), prefixing a leading `…`.
    #[serde(default)]
    pub max_len: usize,
    /// Fish-shell-style shortening: every path component but the last is
    /// reduced to its first character.
    #[serde(default)]
    pub abbreviate: bool,
}

fn default_true() -> bool {
    true
}

impl Default for CwdOpts {
    fn default() -> Self {
        Self {
            abbreviate_home: true,
            format: default_cwd_format(),
            max_depth: 0,
            max_len: 0,
            abbreviate: false,
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
    pub alt_format: String,
    #[serde(default)]
    pub down_format: String,
    #[serde(default)]
    pub interface: Option<String>,
}

impl Default for LanIpOpts {
    fn default() -> Self {
        Self {
            format: default_ip_format(),
            alt_format: String::new(),
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
    pub alt_format: String,
    #[serde(default)]
    pub down_format: String,
}

impl Default for TailscaleIpOpts {
    fn default() -> Self {
        Self {
            format: default_ip_format(),
            alt_format: String::new(),
            down_format: String::new(),
        }
    }
}

/// Default `format` for the `battery` widget.
fn default_battery_format() -> String {
    "{icon} {percent}%".into()
}

/// Default battery `warn_percent`/`crit_percent`: lower is worse, so warn
/// fires above crit.
fn default_bat_warn() -> f64 {
    20.0
}

fn default_bat_crit() -> f64 {
    10.0
}

/// Options for the `battery` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BatteryOpts {
    #[serde(default = "default_battery_format")]
    pub format: String,
    #[serde(default)]
    pub down_format: String,
    #[serde(default)]
    pub alt_format: String,
    #[serde(default = "default_bat_warn")]
    pub warn_percent: f64,
    #[serde(default = "default_bat_crit")]
    pub crit_percent: f64,
    /// Overrides `{icon}` with a fixed glyph, replacing the level-bucketed,
    /// charging-aware computed icon entirely. `None` (the default) keeps the
    /// computed glyph, for non-Nerd-Font users to substitute their own.
    #[serde(default)]
    pub icon: Option<String>,
}

impl Default for BatteryOpts {
    fn default() -> Self {
        Self {
            format: default_battery_format(),
            down_format: String::new(),
            alt_format: String::new(),
            warn_percent: default_bat_warn(),
            crit_percent: default_bat_crit(),
            icon: None,
        }
    }
}

/// Default `format` for the `cpu` widget.
fn default_cpu_format() -> String {
    "{icon} {percent}%".into()
}

/// Default `format` for the `memory` widget.
fn default_memory_format() -> String {
    "{icon} {used}/{total}".into()
}

/// Default width (cells) of the `{bar}` gauge for cpu/memory.
fn default_bar_width() -> usize {
    8
}

/// Default cpu `warn_percent`/`crit_percent`.
fn default_cpu_warn() -> f64 {
    80.0
}

fn default_cpu_crit() -> f64 {
    95.0
}

/// Default memory `warn_percent`/`crit_percent`.
fn default_mem_warn() -> f64 {
    80.0
}

fn default_mem_crit() -> f64 {
    92.0
}

/// Options for the `cpu` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CpuOpts {
    #[serde(default = "default_cpu_format")]
    pub format: String,
    #[serde(default)]
    pub down_format: String,
    #[serde(default)]
    pub alt_format: String,
    #[serde(default = "default_bar_width")]
    pub bar_width: usize,
    #[serde(default = "default_cpu_warn")]
    pub warn_percent: f64,
    #[serde(default = "default_cpu_crit")]
    pub crit_percent: f64,
    /// Overrides `{icon}` with a fixed glyph instead of the built-in
    /// Nerd-Font chip icon. `None` (the default) keeps the built-in glyph,
    /// for non-Nerd-Font users to substitute their own.
    #[serde(default)]
    pub icon: Option<String>,
}

impl Default for CpuOpts {
    fn default() -> Self {
        Self {
            format: default_cpu_format(),
            down_format: String::new(),
            alt_format: String::new(),
            bar_width: default_bar_width(),
            warn_percent: default_cpu_warn(),
            crit_percent: default_cpu_crit(),
            icon: None,
        }
    }
}

/// Options for the `memory` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryOpts {
    #[serde(default = "default_memory_format")]
    pub format: String,
    #[serde(default)]
    pub down_format: String,
    #[serde(default)]
    pub alt_format: String,
    #[serde(default = "default_bar_width")]
    pub bar_width: usize,
    #[serde(default = "default_mem_warn")]
    pub warn_percent: f64,
    #[serde(default = "default_mem_crit")]
    pub crit_percent: f64,
    /// Overrides `{icon}` with a fixed glyph instead of the built-in
    /// Nerd-Font memory icon. `None` (the default) keeps the built-in glyph,
    /// for non-Nerd-Font users to substitute their own.
    #[serde(default)]
    pub icon: Option<String>,
}

impl Default for MemoryOpts {
    fn default() -> Self {
        Self {
            format: default_memory_format(),
            down_format: String::new(),
            alt_format: String::new(),
            bar_width: default_bar_width(),
            warn_percent: default_mem_warn(),
            crit_percent: default_mem_crit(),
            icon: None,
        }
    }
}

/// Default `format` for the `loadavg` widget: 1/5/15-min values at 2 decimals,
/// reproducing the pre-config output byte-for-byte.
fn default_loadavg_format() -> String {
    "{load1} {load5} {load15}".into()
}

/// Options for the `loadavg` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoadAvgOpts {
    #[serde(default = "default_loadavg_format")]
    pub format: String,
    #[serde(default)]
    pub alt_format: String,
    #[serde(default)]
    pub down_format: String,
    /// `load1` threshold to badge as a warning. `0.0` (the default) disables
    /// this tier — load average has no fixed "healthy" ceiling across
    /// machines, so alerting is opt-in here unlike cpu/memory/battery.
    #[serde(default)]
    pub warn_load: f64,
    /// `load1` threshold to badge as critical. `0.0` (the default) disables
    /// this tier.
    #[serde(default)]
    pub crit_load: f64,
}

impl Default for LoadAvgOpts {
    fn default() -> Self {
        Self {
            format: default_loadavg_format(),
            alt_format: String::new(),
            down_format: String::new(),
            warn_load: 0.0,
            crit_load: 0.0,
        }
    }
}

/// Default `format` for the `git` widget: a Nerd-Font branch glyph, the
/// branch name, and a trailing dirty marker.
fn default_git_format() -> String {
    "\u{e0a0} {branch}{dirty}".into()
}

/// Default `{dirty}` glyph: a bare asterisk, for terminals without a Nerd Font.
fn default_dirty_glyph() -> String {
    "*".into()
}

/// Options for the `git` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitOpts {
    #[serde(default = "default_git_format")]
    pub format: String,
    #[serde(default)]
    pub down_format: String,
    #[serde(default)]
    pub alt_format: String,
    /// Substituted for `{dirty}` when the repo has any staged or unstaged
    /// change; the empty string when clean.
    #[serde(default = "default_dirty_glyph")]
    pub dirty_glyph: String,
}

impl Default for GitOpts {
    fn default() -> Self {
        Self {
            format: default_git_format(),
            down_format: String::new(),
            alt_format: String::new(),
            dirty_glyph: default_dirty_glyph(),
        }
    }
}

/// Default mount for the `disk` widget: the root filesystem.
fn default_disk_mount() -> String {
    "/".into()
}

/// Default `format` for the `disk` widget: used/total, no icon placeholder.
fn default_disk_format() -> String {
    " {used}/{total}".into()
}

/// Default disk `warn_percent`/`crit_percent`.
fn default_disk_warn() -> f64 {
    85.0
}

fn default_disk_crit() -> f64 {
    95.0
}

/// Options for the `disk` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiskOpts {
    #[serde(default = "default_disk_mount")]
    pub mount: String,
    #[serde(default = "default_disk_format")]
    pub format: String,
    #[serde(default = "default_bar_width")]
    pub bar_width: usize,
    #[serde(default)]
    pub down_format: String,
    #[serde(default = "default_disk_warn")]
    pub warn_percent: f64,
    #[serde(default = "default_disk_crit")]
    pub crit_percent: f64,
    #[serde(default)]
    pub alt_format: String,
}

impl Default for DiskOpts {
    fn default() -> Self {
        Self {
            mount: default_disk_mount(),
            format: default_disk_format(),
            bar_width: default_bar_width(),
            down_format: String::new(),
            warn_percent: default_disk_warn(),
            crit_percent: default_disk_crit(),
            alt_format: String::new(),
        }
    }
}

/// Default `format` for the `uptime` widget: the bare humanized reading.
fn default_uptime_format() -> String {
    "{uptime}".into()
}

/// Options for the `uptime` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UptimeOpts {
    #[serde(default = "default_uptime_format")]
    pub format: String,
    #[serde(default)]
    pub alt_format: String,
    #[serde(default)]
    pub down_format: String,
}

impl Default for UptimeOpts {
    fn default() -> Self {
        Self {
            format: default_uptime_format(),
            alt_format: String::new(),
            down_format: String::new(),
        }
    }
}

/// Per-widget option overrides, keyed by widget name.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WidgetOpts {
    #[serde(default)]
    pub hostname: HostnameOpts,
    #[serde(default)]
    pub pane_id: PaneIdOpts,
    #[serde(default)]
    pub datetime: DateTimeOpts,
    #[serde(default)]
    pub cwd: CwdOpts,
    #[serde(default)]
    pub lan_ip: LanIpOpts,
    #[serde(default)]
    pub tailscale_ip: TailscaleIpOpts,
    #[serde(default)]
    pub battery: BatteryOpts,
    #[serde(default)]
    pub cpu: CpuOpts,
    #[serde(default)]
    pub memory: MemoryOpts,
    #[serde(default)]
    pub loadavg: LoadAvgOpts,
    #[serde(default)]
    pub git: GitOpts,
    #[serde(default)]
    pub disk: DiskOpts,
    #[serde(default)]
    pub uptime: UptimeOpts,
}

/// Optional theme overrides layered onto a base [`Theme`] by
/// [`ThemeConfig::apply_to`]; `None` means "keep the base value". A complete
/// mirror of every [`Theme`] field, plus `base` (a selector, not a color).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
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
        let base = self
            .theme
            .base
            .as_deref()
            .and_then(crate::builtin_theme)
            .unwrap_or_default();
        self.to_theme_over(base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn to_theme_resolves_builtin_base_and_inline_override_wins() {
        use crate::Color;
        // base only
        let mut cfg = Config::default();
        cfg.theme.base = Some("nord".into());
        let t = cfg.to_theme();
        assert_eq!(t, crate::builtin_theme("nord").unwrap()); // no inline overrides -> exactly nord
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
        cfg.theme.soft_fg = Some(Color::Indexed(77));
        cfg.theme.error = Some(Color::Indexed(88));
        let t = cfg.to_theme();
        assert_eq!(t.win_current_bg, Color::Indexed(60));
        assert_eq!(t.win_inactive_bg, Color::Indexed(61));
        assert_eq!(t.win_current_fg, Color::Indexed(62));
        assert_eq!(t.win_inactive_fg, Color::Indexed(63));
        assert_eq!(t.win_cap_left, "L");
        assert_eq!(t.win_cap_right, "R");
        assert_eq!(t.soft_fg, Color::Indexed(77));
        assert_eq!(t.error, Color::Indexed(88));
    }

    #[test]
    fn to_theme_defaults_window_pill_when_unset() {
        let t = Config::default().to_theme();
        assert_eq!(t.win_current_bg, crate::Color::Indexed(31));
        assert_eq!(t.win_inactive_bg, crate::Color::Indexed(236));
        assert_eq!(t.win_cap_left, "\u{e0b6}");
    }

    #[test]
    fn default_layout_matches_spec() {
        let c = Config::default();
        assert_eq!(c.layout.left, vec!["pane_id", "hostname"]);
        assert_eq!(c.layout.center, vec!["windows"]);
        assert_eq!(
            c.layout.right,
            vec!["cwd", "cpu", "memory", "loadavg", "datetime"]
        );
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
    fn hostname_pane_id_opts_parse_with_defaults() {
        let toml = r#"
[widgets.hostname]
format = "host: {host}"
[widgets.pane_id]
format = "{session}/{window}/{pane}"
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.widgets.hostname.format, "host: {host}");
        assert_eq!(c.widgets.pane_id.format, "{session}/{window}/{pane}");
    }

    #[test]
    fn hostname_pane_id_opts_default_when_absent() {
        let c = Config::default();
        assert_eq!(c.widgets.hostname.format, "{host}");
        assert_eq!(c.widgets.pane_id.format, "{session}:{window}.{pane}");
    }

    #[test]
    fn malformed_hostname_table_falls_back_to_default() {
        let dir = std::env::temp_dir().join("rustline_test_badhostname");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        // format must be a string; an integer makes the table invalid.
        std::fs::write(&p, "[widgets.hostname]\nformat = 5\n").unwrap();
        let c = Config::load(&p);
        assert_eq!(c.widgets.hostname.format, "{host}");
        assert_eq!(c.layout.left, Config::default().layout.left);
    }

    #[test]
    fn cwd_opts_parse_with_defaults() {
        let toml = r#"
[widgets.cwd]
format = "cwd: {path}"
max_depth = 3
max_len = 40
abbreviate = true
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.widgets.cwd.format, "cwd: {path}");
        assert_eq!(c.widgets.cwd.max_depth, 3);
        assert_eq!(c.widgets.cwd.max_len, 40);
        assert!(c.widgets.cwd.abbreviate);
        assert!(c.widgets.cwd.abbreviate_home); // omitted -> default
    }

    #[test]
    fn cwd_opts_default_when_absent() {
        let c = Config::default();
        assert!(c.widgets.cwd.abbreviate_home);
        assert_eq!(c.widgets.cwd.format, "{path}");
        assert_eq!(c.widgets.cwd.max_depth, 0);
        assert_eq!(c.widgets.cwd.max_len, 0);
        assert!(!c.widgets.cwd.abbreviate);
    }

    #[test]
    fn malformed_cwd_table_falls_back_to_default() {
        let dir = std::env::temp_dir().join("rustline_test_badcwd");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        // max_depth must be an integer; a string makes the table invalid.
        std::fs::write(&p, "[widgets.cwd]\nmax_depth = \"deep\"\n").unwrap();
        let c = Config::load(&p);
        assert_eq!(c.widgets.cwd.max_depth, 0);
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

    #[test]
    fn battery_opts_parse_with_defaults() {
        let toml = "[widgets.battery]\nformat = \"{percent}% {state}\"\n";
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.widgets.battery.format, "{percent}% {state}");
        assert_eq!(c.widgets.battery.down_format, ""); // omitted -> default
    }

    #[test]
    fn battery_opts_default_when_absent() {
        let c = Config::default();
        assert_eq!(c.widgets.battery.format, "{icon} {percent}%");
        assert_eq!(c.widgets.battery.down_format, "");
    }

    #[test]
    fn malformed_battery_table_falls_back_to_default() {
        let dir = std::env::temp_dir().join("rustline_test_badbattery");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        // format must be a string; an integer makes the table invalid.
        std::fs::write(&p, "[widgets.battery]\nformat = 5\n").unwrap();
        let c = Config::load(&p);
        assert_eq!(c.widgets.battery.format, "{icon} {percent}%");
        assert_eq!(c.layout.left, Config::default().layout.left);
    }

    #[test]
    fn cpu_memory_opts_parse_with_defaults() {
        let toml = r#"
[widgets.cpu]
format = "{bar} {percent}%"
[widgets.memory]
bar_width = 12
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.widgets.cpu.format, "{bar} {percent}%");
        assert_eq!(c.widgets.cpu.bar_width, 8); // omitted -> default
        assert_eq!(c.widgets.memory.format, "{icon} {used}/{total}"); // omitted -> default
        assert_eq!(c.widgets.memory.bar_width, 12);
        assert_eq!(c.widgets.memory.down_format, "");
    }

    #[test]
    fn cpu_memory_opts_default_when_absent() {
        let c = Config::default();
        assert_eq!(c.widgets.cpu.format, "{icon} {percent}%");
        assert_eq!(c.widgets.cpu.bar_width, 8);
        assert_eq!(c.widgets.memory.format, "{icon} {used}/{total}");
        assert_eq!(c.widgets.memory.bar_width, 8);
    }

    #[test]
    fn icon_override_defaults_to_none_and_parses() {
        let c = Config::default();
        assert_eq!(c.widgets.cpu.icon, None);
        assert_eq!(c.widgets.memory.icon, None);
        assert_eq!(c.widgets.battery.icon, None);

        let toml = r#"
[widgets.cpu]
icon = "C"
[widgets.memory]
icon = "M"
[widgets.battery]
icon = "B"
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.widgets.cpu.icon.as_deref(), Some("C"));
        assert_eq!(c.widgets.memory.icon.as_deref(), Some("M"));
        assert_eq!(c.widgets.battery.icon.as_deref(), Some("B"));
    }

    #[test]
    fn malformed_cpu_table_falls_back_to_default() {
        let dir = std::env::temp_dir().join("rustline_test_badcpu");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        // bar_width must be an integer; a string makes the table invalid.
        std::fs::write(&p, "[widgets.cpu]\nbar_width = \"wide\"\n").unwrap();
        let c = Config::load(&p);
        assert_eq!(c.widgets.cpu.bar_width, 8);
        assert_eq!(c.layout.left, Config::default().layout.left);
    }

    #[test]
    fn loadavg_opts_parse_with_defaults() {
        let toml = r#"
[widgets.loadavg]
format = "L {load1:.1}"
alt_format = "{load1} {load5} {load15}"
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.widgets.loadavg.format, "L {load1:.1}");
        assert_eq!(c.widgets.loadavg.alt_format, "{load1} {load5} {load15}");
        assert_eq!(c.widgets.loadavg.down_format, ""); // omitted -> default
    }

    #[test]
    fn loadavg_opts_default_when_absent() {
        let c = Config::default();
        assert_eq!(c.widgets.loadavg.format, "{load1} {load5} {load15}");
        assert_eq!(c.widgets.loadavg.alt_format, "");
        assert_eq!(c.widgets.loadavg.down_format, "");
    }

    #[test]
    fn malformed_loadavg_table_falls_back_to_default() {
        let dir = std::env::temp_dir().join("rustline_test_badloadavg");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        // format must be a string; an integer makes the table invalid.
        std::fs::write(&p, "[widgets.loadavg]\nformat = 5\n").unwrap();
        let c = Config::load(&p);
        assert_eq!(c.widgets.loadavg.format, "{load1} {load5} {load15}");
        assert_eq!(c.layout.left, Config::default().layout.left);
    }

    #[test]
    fn git_opts_parse_with_defaults() {
        let toml = r#"
[widgets.git]
format = "{branch}{dirty}"
dirty_glyph = "!"
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.widgets.git.format, "{branch}{dirty}");
        assert_eq!(c.widgets.git.dirty_glyph, "!");
        assert_eq!(c.widgets.git.down_format, ""); // omitted -> default
        assert_eq!(c.widgets.git.alt_format, ""); // omitted -> default
    }

    #[test]
    fn git_opts_default_when_absent() {
        let c = Config::default();
        assert_eq!(c.widgets.git.format, "\u{e0a0} {branch}{dirty}");
        assert_eq!(c.widgets.git.dirty_glyph, "*");
        assert_eq!(c.widgets.git.down_format, "");
        assert_eq!(c.widgets.git.alt_format, "");
    }

    #[test]
    fn malformed_git_table_falls_back_to_default() {
        let dir = std::env::temp_dir().join("rustline_test_badgit");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        // format must be a string; an integer makes the table invalid.
        std::fs::write(&p, "[widgets.git]\nformat = 5\n").unwrap();
        let c = Config::load(&p);
        assert_eq!(c.widgets.git.format, "\u{e0a0} {branch}{dirty}");
        assert_eq!(c.layout.left, Config::default().layout.left);
    }

    #[test]
    fn disk_opts_parse_with_defaults() {
        let toml = r#"
[widgets.disk]
mount = "/home"
format = "{bar} {percent}%"
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.widgets.disk.mount, "/home");
        assert_eq!(c.widgets.disk.format, "{bar} {percent}%");
        assert_eq!(c.widgets.disk.bar_width, 8); // omitted -> default
        assert_eq!(c.widgets.disk.down_format, ""); // omitted -> default
        assert_eq!(c.widgets.disk.alt_format, ""); // omitted -> default
        assert_eq!(c.widgets.disk.warn_percent, 85.0); // omitted -> default
        assert_eq!(c.widgets.disk.crit_percent, 95.0); // omitted -> default
    }

    #[test]
    fn disk_opts_default_when_absent() {
        let c = Config::default();
        assert_eq!(c.widgets.disk.mount, "/");
        assert_eq!(c.widgets.disk.format, " {used}/{total}");
        assert_eq!(c.widgets.disk.bar_width, 8);
        assert_eq!(c.widgets.disk.down_format, "");
        assert_eq!(c.widgets.disk.alt_format, "");
        assert_eq!(c.widgets.disk.warn_percent, 85.0);
        assert_eq!(c.widgets.disk.crit_percent, 95.0);
    }

    #[test]
    fn malformed_disk_table_falls_back_to_default() {
        let dir = std::env::temp_dir().join("rustline_test_baddisk");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        // mount must be a string; an integer makes the table invalid.
        std::fs::write(&p, "[widgets.disk]\nmount = 5\n").unwrap();
        let c = Config::load(&p);
        assert_eq!(c.widgets.disk.mount, "/");
        assert_eq!(c.layout.left, Config::default().layout.left);
    }

    #[test]
    fn theme_config_full_mirror_apply_and_from_theme_round_trip() {
        use crate::Color;
        // apply_to sets only Some fields, leaving others at the base value.
        let cfg = ThemeConfig {
            error: Some(Color::Rgb(9, 9, 9)),
            soft_fg: Some(Color::Indexed(99)),
            ..Default::default()
        };
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
        let base = crate::Theme {
            fg: Color::Indexed(200),
            error: Color::Indexed(160),
            ..crate::Theme::default()
        };
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

    #[test]
    fn threshold_knobs_default_and_parse() {
        let c = Config::default();
        assert_eq!(c.widgets.cpu.warn_percent, 80.0);
        assert_eq!(c.widgets.cpu.crit_percent, 95.0);
        assert_eq!(c.widgets.memory.crit_percent, 92.0);
        assert_eq!(c.widgets.battery.warn_percent, 20.0);
        assert_eq!(c.widgets.loadavg.warn_load, 0.0); // off by default
        let parsed: Config = toml::from_str(
            "[widgets.cpu]\nwarn_percent = 70\n[widgets.loadavg]\ncrit_load = 8.0\n",
        )
        .unwrap();
        assert_eq!(parsed.widgets.cpu.warn_percent, 70.0);
        assert_eq!(parsed.widgets.cpu.crit_percent, 95.0); // untouched default
        assert_eq!(parsed.widgets.loadavg.crit_load, 8.0);
    }
}
