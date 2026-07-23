//! User-facing TOML configuration: layout, per-widget options, and theme
//! overrides.
//!
//! [`Config::load`] is total — a missing file or a parse error both fall
//! back to [`Config::default`] (the spec-defined layout) rather than
//! panicking, so a bad or absent config file never takes down the status
//! line.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::Path;

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
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
    /// An IANA zone name (e.g. `"America/New_York"`) to render in, instead
    /// of the local timezone. `None` (the default) keeps the pre-feature
    /// behavior of formatting `ctx.now` as-is. An unrecognized name is
    /// logged and falls back to local time rather than erroring.
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default, flatten)]
    pub color: ColorOverride,
    #[serde(default, flatten)]
    pub click: ClickBindings,
}

fn default_dt_format() -> String {
    "%a < %Y-%m-%d < %H:%M".into()
}

impl Default for DateTimeOpts {
    fn default() -> Self {
        Self {
            format: default_dt_format(),
            alt_format: String::new(),
            timezone: None,
            color: ColorOverride::default(),
            click: ClickBindings::default(),
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
    #[serde(default, flatten)]
    pub color: ColorOverride,
}

impl Default for HostnameOpts {
    fn default() -> Self {
        Self {
            format: default_hostname_format(),
            color: ColorOverride::default(),
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
    #[serde(default, flatten)]
    pub color: ColorOverride,
}

impl Default for PaneIdOpts {
    fn default() -> Self {
        Self {
            format: default_pane_id_format(),
            color: ColorOverride::default(),
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
    #[serde(default, flatten)]
    pub color: ColorOverride,
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
            color: ColorOverride::default(),
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
    #[serde(default, flatten)]
    pub color: ColorOverride,
    #[serde(default, flatten)]
    pub click: ClickBindings,
}

impl Default for LanIpOpts {
    fn default() -> Self {
        Self {
            format: default_ip_format(),
            alt_format: String::new(),
            down_format: String::new(),
            interface: None,
            color: ColorOverride::default(),
            click: ClickBindings::default(),
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
    #[serde(default, flatten)]
    pub color: ColorOverride,
    #[serde(default, flatten)]
    pub click: ClickBindings,
}

impl Default for TailscaleIpOpts {
    fn default() -> Self {
        Self {
            format: default_ip_format(),
            alt_format: String::new(),
            down_format: String::new(),
            color: ColorOverride::default(),
            click: ClickBindings::default(),
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
    #[serde(default, flatten)]
    pub color: ColorOverride,
    #[serde(default, flatten)]
    pub click: ClickBindings,
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
            color: ColorOverride::default(),
            click: ClickBindings::default(),
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

/// Default length (readings) of the `{spark}` history ring for cpu/memory —
/// also the persisted ring's max length (see `crates/rustline/src/cpu.rs`'s
/// `read_cpu_history`/`memory.rs`'s `read_memory_history`).
fn default_spark_width() -> usize {
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
    /// Max length (readings) of the `{spark}` history ring. Only consulted
    /// when `format` references `{spark}` — see `Context::cpu_history`.
    #[serde(default = "default_spark_width")]
    pub spark_width: usize,
    #[serde(default = "default_cpu_warn")]
    pub warn_percent: f64,
    #[serde(default = "default_cpu_crit")]
    pub crit_percent: f64,
    /// Overrides `{icon}` with a fixed glyph instead of the built-in
    /// Nerd-Font chip icon. `None` (the default) keeps the built-in glyph,
    /// for non-Nerd-Font users to substitute their own.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default, flatten)]
    pub color: ColorOverride,
    #[serde(default, flatten)]
    pub click: ClickBindings,
}

impl Default for CpuOpts {
    fn default() -> Self {
        Self {
            format: default_cpu_format(),
            down_format: String::new(),
            alt_format: String::new(),
            bar_width: default_bar_width(),
            spark_width: default_spark_width(),
            warn_percent: default_cpu_warn(),
            crit_percent: default_cpu_crit(),
            icon: None,
            color: ColorOverride::default(),
            click: ClickBindings::default(),
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
    /// Max length (readings) of the `{spark}` history ring. Only consulted
    /// when `format` references `{spark}` — see `Context::mem_history`.
    #[serde(default = "default_spark_width")]
    pub spark_width: usize,
    #[serde(default = "default_mem_warn")]
    pub warn_percent: f64,
    #[serde(default = "default_mem_crit")]
    pub crit_percent: f64,
    /// Overrides `{icon}` with a fixed glyph instead of the built-in
    /// Nerd-Font memory icon. `None` (the default) keeps the built-in glyph,
    /// for non-Nerd-Font users to substitute their own.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default, flatten)]
    pub color: ColorOverride,
    #[serde(default, flatten)]
    pub click: ClickBindings,
}

impl Default for MemoryOpts {
    fn default() -> Self {
        Self {
            format: default_memory_format(),
            down_format: String::new(),
            alt_format: String::new(),
            bar_width: default_bar_width(),
            spark_width: default_spark_width(),
            warn_percent: default_mem_warn(),
            crit_percent: default_mem_crit(),
            icon: None,
            color: ColorOverride::default(),
            click: ClickBindings::default(),
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
    #[serde(default, flatten)]
    pub color: ColorOverride,
    #[serde(default, flatten)]
    pub click: ClickBindings,
}

impl Default for LoadAvgOpts {
    fn default() -> Self {
        Self {
            format: default_loadavg_format(),
            alt_format: String::new(),
            down_format: String::new(),
            warn_load: 0.0,
            crit_load: 0.0,
            color: ColorOverride::default(),
            click: ClickBindings::default(),
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
    #[serde(default, flatten)]
    pub color: ColorOverride,
    #[serde(default, flatten)]
    pub click: ClickBindings,
}

impl Default for GitOpts {
    fn default() -> Self {
        Self {
            format: default_git_format(),
            down_format: String::new(),
            alt_format: String::new(),
            dirty_glyph: default_dirty_glyph(),
            color: ColorOverride::default(),
            click: ClickBindings::default(),
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
    #[serde(default, flatten)]
    pub color: ColorOverride,
    #[serde(default, flatten)]
    pub click: ClickBindings,
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
            color: ColorOverride::default(),
            click: ClickBindings::default(),
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
    #[serde(default, flatten)]
    pub color: ColorOverride,
    #[serde(default, flatten)]
    pub click: ClickBindings,
}

impl Default for UptimeOpts {
    fn default() -> Self {
        Self {
            format: default_uptime_format(),
            alt_format: String::new(),
            down_format: String::new(),
            color: ColorOverride::default(),
            click: ClickBindings::default(),
        }
    }
}

/// Default `format` for the `media` widget: title em-dash artist.
fn default_media_format() -> String {
    "{title} — {artist}".into()
}

/// Options for the `media` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MediaOpts {
    #[serde(default = "default_media_format")]
    pub format: String,
    #[serde(default)]
    pub alt_format: String,
    #[serde(default)]
    pub down_format: String,
    #[serde(default, flatten)]
    pub color: ColorOverride,
    #[serde(default, flatten)]
    pub click: ClickBindings,
}

impl Default for MediaOpts {
    fn default() -> Self {
        Self {
            format: default_media_format(),
            alt_format: String::new(),
            down_format: String::new(),
            color: ColorOverride::default(),
            click: ClickBindings::default(),
        }
    }
}

/// Default `format` for the `throughput` widget: down/up rates, no icon
/// placeholder (mirrors `disk`'s icon-less default).
fn default_throughput_format() -> String {
    " {down} {up}".into()
}

/// Options for the `throughput` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThroughputOpts {
    #[serde(default = "default_throughput_format")]
    pub format: String,
    #[serde(default)]
    pub alt_format: String,
    #[serde(default)]
    pub down_format: String,
    /// Pin the read to a single named network interface instead of
    /// aggregating every non-loopback interface. `None` (the default)
    /// aggregates.
    #[serde(default)]
    pub interface: Option<String>,
    #[serde(default, flatten)]
    pub color: ColorOverride,
    #[serde(default, flatten)]
    pub click: ClickBindings,
}

impl Default for ThroughputOpts {
    fn default() -> Self {
        Self {
            format: default_throughput_format(),
            alt_format: String::new(),
            down_format: String::new(),
            interface: None,
            color: ColorOverride::default(),
            click: ClickBindings::default(),
        }
    }
}

/// An explicit per-widget foreground/background color pin (W29), surfaced as
/// `fg`/`bg` keys flattened into a `[widgets.<name>]` table alongside that
/// widget's other options.
///
/// Applied centrally by
/// [`render_named_region`](crate::assemble::render_named_region) — after a
/// widget renders and before
/// [`assign_palette`](crate::assemble::assign_palette) fills in the cycling
/// palette color — never inside a widget itself, so widgets stay
/// `Context`-only (invariant #1). `bg` only takes effect on a segment that
/// doesn't already carry an explicit background (the same rule
/// `assign_palette` itself follows, e.g. for an alert badge); `fg` applies
/// unconditionally wherever set. Both default to `None` (no override), so an
/// absent/default config renders byte-identically to before this feature
/// (invariant #3).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ColorOverride {
    #[serde(default)]
    pub fg: Option<Color>,
    #[serde(default)]
    pub bg: Option<Color>,
}

/// One configured click action for a widget button (W36). Serde shape, one
/// per button field: `{ toggle = true }` | `{ open_url = "…" }` |
/// `{ run = "…" }`.
///
/// This is the *config-value* type (what a `left_click`/`right_click`/
/// `middle_click` field holds); the binary's `resolve_click` maps it to the
/// runtime `ClickAction` it dispatches on. The `toggle` payload is a bool so
/// the TOML `{ toggle = true }` shape round-trips (`false` explicitly disables
/// the default toggle).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClickBinding {
    /// Toggle the widget's `alt_format` view (the default left-click action).
    Toggle(bool),
    /// Open a URL with the OS opener (`xdg-open`/`open`).
    OpenUrl(String),
    /// Run a shell command (`sh -c <cmd>`), detached.
    Run(String),
}

/// Per-widget, per-button click bindings, flattened into each clickable
/// widget's `[widgets.<name>]` table (W36) — the same flatten pattern as
/// [`ColorOverride`]. All optional; an absent button falls back to the
/// default click behavior, so an unconfigured widget is byte-identical to
/// before this feature (invariant #3).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClickBindings {
    #[serde(default)]
    pub left_click: Option<ClickBinding>,
    #[serde(default)]
    pub right_click: Option<ClickBinding>,
    #[serde(default)]
    pub middle_click: Option<ClickBinding>,
}

impl ClickBindings {
    /// The configured binding for a mouse-button name, if any. The button
    /// string is a boundary value (tmux `MouseDown1Status` sends `left`
    /// today); an unrecognized button yields `None`, so a click with no
    /// matching binding falls through to the default behavior.
    pub fn for_button(&self, button: &str) -> Option<&ClickBinding> {
        match button {
            "left" => self.left_click.as_ref(),
            "right" => self.right_click.as_ref(),
            "middle" => self.middle_click.as_ref(),
            _ => None,
        }
    }
}

/// A widget's click-relevant configuration, projected by [`Config::click_map`]
/// and consumed by the binary's `resolve_click`: whether the widget is
/// click-toggleable (has a non-empty `alt_format`) and its per-button
/// bindings. Distinguishing a *known* built-in that isn't toggleable (→ no-op
/// on a default left-click) from a name absent from the map (a plugin, whose
/// bindings live under `[plugins.*]`, or an unknown range) is what lets
/// `resolve_click` preserve the pre-W36 plugin/unknown flip behavior
/// (invariant #7).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WidgetClick {
    /// True when the widget has a non-empty `alt_format`, so a default
    /// left-click toggles its view.
    pub toggleable: bool,
    /// Per-button overrides; a set button wins over the default action.
    pub bindings: ClickBindings,
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
    #[serde(default)]
    pub media: MediaOpts,
    #[serde(default)]
    pub throughput: ThroughputOpts,
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

/// Where a plugin's `.wasm` came from, recorded in `[plugins.<name>].source`
/// (W38). `rustline plugin install <owner/repo>` writes an
/// [`PluginSource::OwnerRepo`]; the `Url`/`Path` variants are reserved for a
/// future install-by-URL / install-by-path.
///
/// Deserialization accepts a **bare string** as [`PluginSource::OwnerRepo`], so
/// pre-W38 configs (`source = "steve/rustline-weather"`) keep parsing unchanged
/// — load-bearing back-compat, so a pre-existing config never fails to load
/// (invariant #3). The `Url`/`Path` variants take an inline table
/// (`{ url = "…" }` / `{ path = "…" }`); `{ owner_repo = "…" }` is also
/// accepted for symmetry. `OwnerRepo` serializes back to a bare string so a
/// round-trip is stable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginSource {
    /// A GitHub `owner/repo` slug — what `plugin install` records.
    OwnerRepo(String),
    /// A direct URL to a `.wasm` (reserved for a future install-by-URL).
    Url(String),
    /// A local filesystem path to a `.wasm` (reserved for install-by-path).
    Path(String),
}

impl fmt::Display for PluginSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PluginSource::OwnerRepo(s) => write!(f, "{s}"),
            PluginSource::Url(s) => write!(f, "url: {s}"),
            PluginSource::Path(s) => write!(f, "path: {s}"),
        }
    }
}

impl Serialize for PluginSource {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            // Bare string keeps parity with pre-W38 configs and re-parses as
            // OwnerRepo, so a round-trip is stable.
            PluginSource::OwnerRepo(s) => serializer.serialize_str(s),
            PluginSource::Url(s) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("url", s)?;
                map.end()
            }
            PluginSource::Path(s) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("path", s)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for PluginSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SourceVisitor;

        impl<'de> Visitor<'de> for SourceVisitor {
            type Value = PluginSource;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(
                    "an \"owner/repo\" string or a { owner_repo | url | path = \"…\" } table",
                )
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<PluginSource, E> {
                Ok(PluginSource::OwnerRepo(v.to_string()))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<PluginSource, A::Error> {
                let Some((key, val)) = map.next_entry::<String, String>()? else {
                    return Err(de::Error::custom("empty plugin source table"));
                };
                let source = match key.as_str() {
                    "owner_repo" => PluginSource::OwnerRepo(val),
                    "url" => PluginSource::Url(val),
                    "path" => PluginSource::Path(val),
                    other => {
                        return Err(de::Error::unknown_field(
                            other,
                            &["owner_repo", "url", "path"],
                        ));
                    }
                };
                Ok(source)
            }
        }

        deserializer.deserialize_any(SourceVisitor)
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
    pub source: Option<PluginSource>,
    #[serde(default)]
    pub allowed_urls: Vec<String>,
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    #[serde(default = "default_max_state_bytes")]
    pub max_state_bytes: u64,
    /// sha256 hex of the installed `.wasm`, recorded by `plugin install`/
    /// `update` so a later integrity check can verify the file on disk.
    #[serde(default)]
    pub checksum: Option<String>,
    /// The resolved release tag `plugin install`/`update` pinned (e.g.
    /// `"v1.2.0"`); `None` for a hand-installed plugin.
    #[serde(default)]
    pub tag: Option<String>,
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
            checksum: None,
            tag: None,
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

    /// Project this config's per-widget `fg`/`bg` overrides into a
    /// name→[`ColorOverride`] map, keyed by the same widget name used in
    /// `layout.*` — the shape
    /// [`render_named_region`](crate::assemble::render_named_region) consumes
    /// to pin a widget's segment colors ahead of `assign_palette` (W29). Only
    /// widgets that actually set `fg` and/or `bg` are included, so an
    /// unconfigured widget's entry is simply absent (keeping the empty-map,
    /// byte-identical case cheap and the common case).
    pub fn color_overrides(&self) -> HashMap<String, ColorOverride> {
        let candidates: [(&str, &ColorOverride); 15] = [
            ("hostname", &self.widgets.hostname.color),
            ("pane_id", &self.widgets.pane_id.color),
            ("datetime", &self.widgets.datetime.color),
            ("cwd", &self.widgets.cwd.color),
            ("lan_ip", &self.widgets.lan_ip.color),
            ("tailscale_ip", &self.widgets.tailscale_ip.color),
            ("battery", &self.widgets.battery.color),
            ("cpu", &self.widgets.cpu.color),
            ("memory", &self.widgets.memory.color),
            ("loadavg", &self.widgets.loadavg.color),
            ("git", &self.widgets.git.color),
            ("disk", &self.widgets.disk.color),
            ("uptime", &self.widgets.uptime.color),
            ("media", &self.widgets.media.color),
            ("throughput", &self.widgets.throughput.color),
        ];
        candidates
            .into_iter()
            .filter(|(_, color)| color.fg.is_some() || color.bg.is_some())
            .map(|(name, color)| (name.to_string(), color.clone()))
            .collect()
    }

    /// Project the clickable built-in widgets into a name→[`WidgetClick`] map,
    /// keyed by the same widget name used in `layout.*` — the shape the
    /// binary's `resolve_click` consumes to decide a click's action (W36).
    ///
    /// Every click-*candidate* built-in (the format-bearing widgets that carry
    /// an `alt_format`) is included, even with no binding configured, so
    /// `resolve_click` can tell a *known* non-toggleable widget (→ no-op on a
    /// default left-click) from a name absent from the map (a WASM plugin,
    /// configured under `[plugins.*]` not `[widgets.*]`, or an unknown range),
    /// which it treats as toggleable to preserve the pre-W36 flip behavior
    /// (invariant #7). Mirrors [`Config::color_overrides`]'s candidate-table
    /// projector.
    pub fn click_map(&self) -> HashMap<String, WidgetClick> {
        let candidates: [(&str, &str, &ClickBindings); 12] = [
            (
                "datetime",
                &self.widgets.datetime.alt_format,
                &self.widgets.datetime.click,
            ),
            (
                "lan_ip",
                &self.widgets.lan_ip.alt_format,
                &self.widgets.lan_ip.click,
            ),
            (
                "tailscale_ip",
                &self.widgets.tailscale_ip.alt_format,
                &self.widgets.tailscale_ip.click,
            ),
            (
                "battery",
                &self.widgets.battery.alt_format,
                &self.widgets.battery.click,
            ),
            ("cpu", &self.widgets.cpu.alt_format, &self.widgets.cpu.click),
            (
                "memory",
                &self.widgets.memory.alt_format,
                &self.widgets.memory.click,
            ),
            (
                "loadavg",
                &self.widgets.loadavg.alt_format,
                &self.widgets.loadavg.click,
            ),
            ("git", &self.widgets.git.alt_format, &self.widgets.git.click),
            (
                "disk",
                &self.widgets.disk.alt_format,
                &self.widgets.disk.click,
            ),
            (
                "uptime",
                &self.widgets.uptime.alt_format,
                &self.widgets.uptime.click,
            ),
            (
                "media",
                &self.widgets.media.alt_format,
                &self.widgets.media.click,
            ),
            (
                "throughput",
                &self.widgets.throughput.alt_format,
                &self.widgets.throughput.click,
            ),
        ];
        candidates
            .into_iter()
            .map(|(name, alt_format, bindings)| {
                (
                    name.to_string(),
                    WidgetClick {
                        toggleable: !alt_format.is_empty(),
                        bindings: bindings.clone(),
                    },
                )
            })
            .collect()
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
        assert_eq!(
            w.source,
            Some(PluginSource::OwnerRepo("steve/rustline-weather".into()))
        );
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
    fn plugin_source_bare_string_is_owner_repo() {
        // Load-bearing back-compat (invariant #3): a pre-W38 config that writes
        // `source` as a bare string must keep parsing, now as an OwnerRepo.
        let toml = "[plugins.weather]\nsource = \"steve/rustline-weather\"\n";
        let c: Config = toml::from_str(toml).unwrap();
        let w = c.plugins.get("weather").unwrap();
        assert_eq!(
            w.source,
            Some(PluginSource::OwnerRepo("steve/rustline-weather".into()))
        );
    }

    #[test]
    fn plugin_source_table_forms_and_roundtrip() {
        // The Url/Path variants take an inline table, and OwnerRepo round-trips
        // back to a bare string through serialize→parse.
        let toml = concat!(
            "[plugins.a]\nsource = { url = \"https://x/y.wasm\" }\n",
            "[plugins.b]\nsource = { path = \"/opt/z.wasm\" }\n",
            "[plugins.c]\nsource = \"o/r\"\n",
        );
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(
            c.plugins["a"].source,
            Some(PluginSource::Url("https://x/y.wasm".into()))
        );
        assert_eq!(
            c.plugins["b"].source,
            Some(PluginSource::Path("/opt/z.wasm".into()))
        );
        // OwnerRepo serializes to a bare string, so it re-parses unchanged.
        let owner = PluginSource::OwnerRepo("o/r".into());
        let text = toml::to_string(&Wrapper { v: owner.clone() }).unwrap();
        assert_eq!(text.trim(), "v = \"o/r\"");
        let round: Wrapper = toml::from_str(&text).unwrap();
        assert_eq!(round.v, owner);
    }

    #[derive(Serialize, Deserialize)]
    struct Wrapper {
        v: PluginSource,
    }

    #[test]
    fn plugin_checksum_and_tag_default_and_parse() {
        // Absent -> None (invariant #3); present -> captured.
        let none: PluginConfig = toml::from_str("").unwrap();
        assert_eq!(none.checksum, None);
        assert_eq!(none.tag, None);

        let toml = concat!(
            "[plugins.weather]\n",
            "source = \"steve/rustline-weather\"\n",
            "tag = \"v1.2.0\"\n",
            "checksum = \"deadbeef\"\n",
        );
        let c: Config = toml::from_str(toml).unwrap();
        let w = c.plugins.get("weather").unwrap();
        assert_eq!(w.tag.as_deref(), Some("v1.2.0"));
        assert_eq!(w.checksum.as_deref(), Some("deadbeef"));
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
    fn cpu_memory_spark_width_parses_and_defaults() {
        let c = Config::default();
        assert_eq!(c.widgets.cpu.spark_width, 8);
        assert_eq!(c.widgets.memory.spark_width, 8);

        let toml = r#"
[widgets.cpu]
spark_width = 12
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.widgets.cpu.spark_width, 12);
        assert_eq!(c.widgets.memory.spark_width, 8); // omitted -> default
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

    #[test]
    fn media_opts_parse_with_defaults() {
        let toml = r#"
[widgets.media]
format = "{artist} - {title}"
alt_format = "{status}: {title}"
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.widgets.media.format, "{artist} - {title}");
        assert_eq!(c.widgets.media.alt_format, "{status}: {title}");
        assert_eq!(c.widgets.media.down_format, ""); // omitted -> default
    }

    #[test]
    fn media_opts_default_when_absent() {
        let c = Config::default();
        assert_eq!(c.widgets.media.format, "{title} — {artist}");
        assert_eq!(c.widgets.media.alt_format, "");
        assert_eq!(c.widgets.media.down_format, "");
    }

    #[test]
    fn malformed_media_table_falls_back_to_default() {
        let dir = std::env::temp_dir().join("rustline_test_badmedia");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        // format must be a string; an integer makes the table invalid.
        std::fs::write(&p, "[widgets.media]\nformat = 5\n").unwrap();
        let c = Config::load(&p);
        assert_eq!(c.widgets.media.format, "{title} — {artist}");
        assert_eq!(c.layout.left, Config::default().layout.left);
    }

    #[test]
    fn throughput_opts_parse_with_defaults() {
        let toml = r#"
[widgets.throughput]
format = "{down} {up}"
interface = "eth0"
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.widgets.throughput.format, "{down} {up}");
        assert_eq!(c.widgets.throughput.interface.as_deref(), Some("eth0"));
        assert_eq!(c.widgets.throughput.down_format, ""); // omitted -> default
        assert_eq!(c.widgets.throughput.alt_format, ""); // omitted -> default
    }

    #[test]
    fn throughput_opts_default_when_absent() {
        let c = Config::default();
        assert_eq!(c.widgets.throughput.format, " {down} {up}");
        assert_eq!(c.widgets.throughput.interface, None);
        assert_eq!(c.widgets.throughput.down_format, "");
        assert_eq!(c.widgets.throughput.alt_format, "");
    }

    #[test]
    fn malformed_throughput_table_falls_back_to_default() {
        let dir = std::env::temp_dir().join("rustline_test_badthroughput");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        // format must be a string; an integer makes the table invalid.
        std::fs::write(&p, "[widgets.throughput]\nformat = 5\n").unwrap();
        let c = Config::load(&p);
        assert_eq!(c.widgets.throughput.format, " {down} {up}");
        assert_eq!(c.layout.left, Config::default().layout.left);
    }

    #[test]
    fn color_override_defaults_to_none_and_flattens_into_widget_tables() {
        let c = Config::default();
        assert_eq!(c.widgets.datetime.color, ColorOverride::default());
        assert_eq!(c.widgets.cpu.color.fg, None);
        assert_eq!(c.widgets.cpu.color.bg, None);

        let toml = r#"
[widgets.datetime]
format = "%H:%M"
fg = { Named = "black" }
bg = { Named = "blue" }
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.widgets.datetime.format, "%H:%M"); // sibling field untouched
        assert_eq!(
            c.widgets.datetime.color.fg,
            Some(Color::Named("black".into()))
        );
        assert_eq!(
            c.widgets.datetime.color.bg,
            Some(Color::Named("blue".into()))
        );
    }

    #[test]
    fn malformed_color_override_falls_back_to_default() {
        let dir = std::env::temp_dir().join("rustline_test_badcoloroverride");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        // `bg` must be a Color table, not a bare string.
        std::fs::write(&p, "[widgets.cpu]\nbg = \"blue\"\n").unwrap();
        let c = Config::load(&p);
        assert_eq!(c.widgets.cpu.color, ColorOverride::default());
        assert_eq!(c.layout.left, Config::default().layout.left);
    }

    #[test]
    fn color_overrides_projects_only_configured_widgets() {
        let mut cfg = Config::default();
        cfg.widgets.datetime.color.bg = Some(Color::Named("blue".into()));
        cfg.widgets.cpu.color.fg = Some(Color::Indexed(1));
        let overrides = cfg.color_overrides();
        assert_eq!(overrides.len(), 2);
        assert_eq!(
            overrides.get("datetime").unwrap().bg,
            Some(Color::Named("blue".into()))
        );
        assert_eq!(overrides.get("cpu").unwrap().fg, Some(Color::Indexed(1)));
        assert!(!overrides.contains_key("hostname"));
    }

    #[test]
    fn color_overrides_is_empty_by_default() {
        assert!(Config::default().color_overrides().is_empty());
    }

    #[test]
    fn click_bindings_default_to_none_and_parse_per_button() {
        let c = Config::default();
        assert_eq!(c.widgets.cpu.click, ClickBindings::default());
        assert!(c.widgets.cpu.click.left_click.is_none());
        assert!(c.widgets.cpu.click.right_click.is_none());
        assert!(c.widgets.cpu.click.middle_click.is_none());

        let toml = r#"
[widgets.cpu]
right_click = { run = "htop" }
[widgets.datetime]
left_click = { toggle = true }
middle_click = { open_url = "https://example.com" }
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(
            c.widgets.cpu.click.right_click,
            Some(ClickBinding::Run("htop".into()))
        );
        assert_eq!(
            c.widgets.datetime.click.left_click,
            Some(ClickBinding::Toggle(true))
        );
        assert_eq!(
            c.widgets.datetime.click.middle_click,
            Some(ClickBinding::OpenUrl("https://example.com".into()))
        );
        // sibling widget fields are untouched by the flattened bindings
        assert_eq!(c.widgets.cpu.format, "{icon} {percent}%");
        assert!(c.widgets.datetime.click.right_click.is_none());
    }

    #[test]
    fn click_bindings_serialize_round_trip() {
        // Guards the `print-config` path: a config carrying click bindings must
        // survive `toml::to_string` → re-parse (flattened enum-of-table values).
        let mut cfg = Config::default();
        cfg.widgets.cpu.click.right_click = Some(ClickBinding::Run("htop".into()));
        cfg.widgets.datetime.click.left_click = Some(ClickBinding::Toggle(true));
        cfg.widgets.datetime.click.middle_click =
            Some(ClickBinding::OpenUrl("https://example.com".into()));
        let serialized = toml::to_string(&cfg).expect("serialize config with click bindings");
        let back: Config = toml::from_str(&serialized).expect("re-parse serialized config");
        assert_eq!(
            back.widgets.cpu.click.right_click,
            Some(ClickBinding::Run("htop".into()))
        );
        assert_eq!(
            back.widgets.datetime.click.left_click,
            Some(ClickBinding::Toggle(true))
        );
        assert_eq!(
            back.widgets.datetime.click.middle_click,
            Some(ClickBinding::OpenUrl("https://example.com".into()))
        );
    }

    #[test]
    fn malformed_click_binding_falls_back_to_default() {
        let dir = std::env::temp_dir().join("rustline_test_badclick");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        // `run` must be a string; an integer makes the binding (and so the
        // whole config) invalid — Config::load must still stay total.
        std::fs::write(&p, "[widgets.cpu]\nright_click = { run = 5 }\n").unwrap();
        let c = Config::load(&p);
        assert!(c.widgets.cpu.click.right_click.is_none());
        assert_eq!(c.layout.left, Config::default().layout.left);
    }

    #[test]
    fn for_button_maps_only_known_buttons() {
        let bindings = ClickBindings {
            left_click: Some(ClickBinding::Toggle(true)),
            right_click: Some(ClickBinding::Run("htop".into())),
            middle_click: None,
        };
        assert_eq!(
            bindings.for_button("left"),
            Some(&ClickBinding::Toggle(true))
        );
        assert_eq!(
            bindings.for_button("right"),
            Some(&ClickBinding::Run("htop".into()))
        );
        assert_eq!(bindings.for_button("middle"), None);
        assert_eq!(bindings.for_button("scroll"), None); // unknown button
    }

    #[test]
    fn click_map_reports_toggleable_and_bindings() {
        let mut cfg = Config::default();
        cfg.widgets.datetime.alt_format = "%H:%M".into();
        cfg.widgets.cpu.click.right_click = Some(ClickBinding::Run("htop".into()));
        let map = cfg.click_map();

        // datetime is toggleable via its non-empty alt_format, no binding set.
        let datetime = map.get("datetime").unwrap();
        assert!(datetime.toggleable);
        assert_eq!(datetime.bindings, ClickBindings::default());

        // cpu is NOT toggleable (empty alt_format) but carries a right-click.
        let cpu = map.get("cpu").unwrap();
        assert!(!cpu.toggleable);
        assert_eq!(
            cpu.bindings.right_click,
            Some(ClickBinding::Run("htop".into()))
        );

        // non-clickable-candidate built-ins (no alt_format) are absent, so
        // resolve_click can distinguish them from plugins/unknown ranges.
        assert!(!map.contains_key("hostname"));
        assert!(!map.contains_key("windows"));
    }

    #[test]
    fn click_map_covers_all_twelve_clickable_widgets_by_default() {
        let map = Config::default().click_map();
        for name in [
            "datetime",
            "lan_ip",
            "tailscale_ip",
            "battery",
            "cpu",
            "memory",
            "loadavg",
            "git",
            "disk",
            "uptime",
            "media",
            "throughput",
        ] {
            let wc = map.get(name).unwrap_or_else(|| panic!("{name} in map"));
            assert!(!wc.toggleable, "{name} not toggleable by default");
            assert_eq!(wc.bindings, ClickBindings::default());
        }
    }
}
