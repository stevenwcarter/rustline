//! User-facing TOML configuration: layout, per-widget options, and theme
//! overrides.
//!
//! [`Config::load`] is total — a missing file or a parse error both fall
//! back to [`Config::default`] (the spec-defined layout) rather than
//! panicking, so a bad or absent config file never takes down the status
//! line.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use toml::Value;

use crate::Color;
use crate::WidgetName;
use crate::render::Theme;
use crate::widget::{WidgetDescriptor, WidgetSource};

/// Which widgets render in each region of the status bar, by name.
///
/// Names are resolved against a [`crate::widget::Registry`] at render time;
/// an unknown name is skipped there, not a config error.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default = "default_left")]
    pub left: Vec<WidgetName>,
    #[serde(default = "default_center")]
    pub center: Vec<WidgetName>,
    #[serde(default = "default_right")]
    pub right: Vec<WidgetName>,
}

fn default_left() -> Vec<WidgetName> {
    vec![WidgetName::from("pane_id"), WidgetName::from("hostname")]
}

fn default_center() -> Vec<WidgetName> {
    vec![WidgetName::from("windows")]
}

fn default_right() -> Vec<WidgetName> {
    vec![
        WidgetName::from("cwd"),
        WidgetName::from("cpu"),
        WidgetName::from("memory"),
        WidgetName::from("loadavg"),
        WidgetName::from("datetime"),
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

/// Which of the three layout arrays a widget sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Region {
    Left,
    Center,
    Right,
}

impl Region {
    /// Every region, in visual left-to-right order.
    pub const ALL: [Region; 3] = [Region::Left, Region::Center, Region::Right];

    /// The config-key spelling, and what `--region` accepts.
    pub fn as_str(&self) -> &'static str {
        match self {
            Region::Left => "left",
            Region::Center => "center",
            Region::Right => "right",
        }
    }

    /// Parse a `--region` value, case-insensitively. `None` if unrecognized.
    pub fn parse(s: &str) -> Option<Region> {
        match s.to_ascii_lowercase().as_str() {
            "left" => Some(Region::Left),
            "center" => Some(Region::Center),
            "right" => Some(Region::Right),
            _ => None,
        }
    }
}

/// Why a layout edit was refused. Every variant means **nothing was mutated**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutEditError {
    /// The name is already placed, at this region/index. A widget may appear
    /// at most once across all three regions: two copies would share one
    /// click-toggle/range identity (invariant #7).
    AlreadyPresent { region: Region, index: usize },
    /// The name is not in any region.
    NotPresent,
    /// The edit would leave the layout exactly as it is.
    NoOp,
}

impl std::fmt::Display for LayoutEditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutEditError::AlreadyPresent { region, index } => {
                write!(
                    f,
                    "already in the {} region at position {index}",
                    region.as_str()
                )
            }
            LayoutEditError::NotPresent => write!(f, "not in any layout region"),
            LayoutEditError::NoOp => write!(f, "already in that position; nothing to do"),
        }
    }
}

/// A completed layout edit, described so a caller can report it without
/// diffing. `from`/`to` are `None` for an add/remove respectively.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutChange {
    pub name: String,
    pub from: Option<(Region, usize)>,
    pub to: Option<(Region, usize)>,
}

impl Layout {
    /// This region's widget names, in visual left-to-right order (invariant #5).
    pub fn get(&self, r: Region) -> &[WidgetName] {
        match r {
            Region::Left => &self.left,
            Region::Center => &self.center,
            Region::Right => &self.right,
        }
    }

    pub fn get_mut(&mut self, r: Region) -> &mut Vec<WidgetName> {
        match r {
            Region::Left => &mut self.left,
            Region::Center => &mut self.center,
            Region::Right => &mut self.right,
        }
    }

    /// Where `name` currently sits, if anywhere. A name appears at most once.
    pub fn find(&self, name: &str) -> Option<(Region, usize)> {
        Region::ALL.into_iter().find_map(|r| {
            self.get(r)
                .iter()
                .position(|n| n == name)
                .map(|idx| (r, idx))
        })
    }
}

/// Place `name` in `region`, at `at` (clamped to the region's length) or
/// appended when `at` is `None`.
pub fn layout_enable(
    layout: &mut Layout,
    name: &str,
    region: Region,
    at: Option<usize>,
) -> Result<LayoutChange, LayoutEditError> {
    if let Some((region, index)) = layout.find(name) {
        return Err(LayoutEditError::AlreadyPresent { region, index });
    }
    let target = layout.get_mut(region);
    let index = at.unwrap_or(target.len()).min(target.len());
    target.insert(index, WidgetName::from(name));
    Ok(LayoutChange {
        name: name.to_string(),
        from: None,
        to: Some((region, index)),
    })
}

/// Remove `name` from whichever region holds it.
pub fn layout_disable(layout: &mut Layout, name: &str) -> Result<LayoutChange, LayoutEditError> {
    let (region, index) = layout.find(name).ok_or(LayoutEditError::NotPresent)?;
    layout.get_mut(region).remove(index);
    Ok(LayoutChange {
        name: name.to_string(),
        from: Some((region, index)),
        to: None,
    })
}

/// Move `name` to `to`/`to_index` (index clamped to the destination length,
/// so a large index means "append").
pub fn layout_move(
    layout: &mut Layout,
    name: &str,
    to: Region,
    to_index: usize,
) -> Result<LayoutChange, LayoutEditError> {
    let from = layout.find(name).ok_or(LayoutEditError::NotPresent)?;
    layout.get_mut(from.0).remove(from.1);
    // Clamp against the length *after* removal, so a same-region move to the
    // end lands at the last slot rather than out of bounds.
    let dest = layout.get_mut(to);
    let index = to_index.min(dest.len());
    if from == (to, index) {
        // Restore and refuse: nothing would change.
        layout
            .get_mut(from.0)
            .insert(from.1, WidgetName::from(name));
        return Err(LayoutEditError::NoOp);
    }
    layout.get_mut(to).insert(index, WidgetName::from(name));
    Ok(LayoutChange {
        name: name.to_string(),
        from: Some(from),
        to: Some((to, index)),
    })
}

/// Shift `name` by `delta` positions inside its current region. A step past
/// either end is [`LayoutEditError::NoOp`] — never a wrap-around.
pub fn layout_nudge(
    layout: &mut Layout,
    name: &str,
    delta: i32,
) -> Result<LayoutChange, LayoutEditError> {
    let (region, index) = layout.find(name).ok_or(LayoutEditError::NotPresent)?;
    let len = layout.get(region).len();
    let target = i64::from(delta) + index as i64;
    if target < 0 || target >= len as i64 {
        return Err(LayoutEditError::NoOp);
    }
    let target = target as usize;
    let arr = layout.get_mut(region);
    let name_owned = arr.remove(index);
    arr.insert(target, name_owned);
    Ok(LayoutChange {
        name: name.to_string(),
        from: Some((region, index)),
        to: Some((region, target)),
    })
}

/// One row of `widget list` / one entry in the TUI: a selectable widget, what
/// it is, and where (if anywhere) it currently sits in the layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetPlacement {
    pub name: String,
    pub summary: String,
    pub source: WidgetSource,
    /// `None` means "available but not currently in any region".
    pub placement: Option<(Region, usize)>,
}

/// Every widget a user could put in a layout, with its current placement.
///
/// Ordering is built-ins (in registration order) → instances (sorted) →
/// plugin stems (sorted) → placed-but-unrecognized names (sorted), which is
/// also the order `widget list` prints and the
/// TUI's AVAILABLE column shows. This is enforced by partitioning on each
/// candidate's [`WidgetSource`] rather than by which of the four inputs
/// below it arrived through — a candidate's `source` (trustworthy since
/// `Registry::with_builtins` labels every `[instances.<name>]` descriptor
/// `WidgetSource::Instance`, not `Builtin`) decides its group, and only
/// group order is fixed; within the instance/plugin/unknown groups, names are
/// always sorted regardless of `descriptors`' own order (which for instances
/// reflects a `Registry`'s `HashMap<String, Value>` iteration over
/// `cfg.instances`, itself unspecified). An `[instances.<name>]` entry whose
/// name collides with a built-in is skipped — built-in always wins, the same
/// precedence `Registry::with_builtins` and `WidgetKind::parse` enforce.
///
/// The fourth group, [`WidgetSource::Unknown`], is populated differently from
/// the other three: instead of scanning a catalog and looking up its
/// placement, it scans `cfg.layout` itself for any name the first three
/// passes never classified (e.g. a plugin whose `.wasm` is no longer present,
/// or a stale name left behind after `plugin remove`) — so `widget_placements`
/// always accounts for every name actually sitting in `[layout]`, not just
/// the ones a live registry/plugin-dir scan currently recognizes. This is
/// what lets `widget list`/`widget edit` show — and `widget disable`/`widget
/// move` operate on — a placed widget the catalog doesn't otherwise know
/// about; only `widget enable` (adding a *new*, unplaced name) still requires
/// catalog membership, since an unplaced name never reaches this scan.
pub fn widget_placements(
    cfg: &Config,
    descriptors: &[WidgetDescriptor],
    plugin_names: &[String],
) -> Vec<WidgetPlacement> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    // Built-ins keep the order they're first seen in (a `Registry`'s
    // registration order); instances, plugins, and unknowns are sorted by
    // name below, so their intake order here doesn't matter.
    let mut builtins: Vec<(String, String, WidgetSource)> = Vec::new();
    let mut instances: Vec<(String, String, WidgetSource)> = Vec::new();
    let mut plugins: Vec<(String, String, WidgetSource)> = Vec::new();
    let mut unknown: Vec<(String, String, WidgetSource)> = Vec::new();

    // First candidate for a name wins (a later duplicate, e.g. a name
    // appearing in both `descriptors` and `plugin_names`, is dropped);
    // deduped entries are then sorted into one of the four ordering groups
    // by their own `source`, never by which pass produced them.
    let mut classify = |name: String, summary: String, source: WidgetSource| {
        if !seen.insert(name.clone()) {
            return;
        }
        match &source {
            WidgetSource::Builtin => builtins.push((name, summary, source)),
            WidgetSource::Instance { .. } => instances.push((name, summary, source)),
            WidgetSource::Plugin => plugins.push((name, summary, source)),
            WidgetSource::Unknown => unknown.push((name, summary, source)),
        }
    };

    for d in descriptors {
        classify(d.name.clone(), d.summary.clone(), d.source.clone());
    }
    // A dedicated scan over `cfg.instances` covers a caller that passes
    // `descriptors` without having run `Registry::with_builtins` first (e.g.
    // a bare `&[]`) — the instance is still offered, just via this pass
    // instead of arriving pre-labeled.
    let mut instance_entries: Vec<(&WidgetName, &Value)> = cfg.instances.iter().collect();
    instance_entries.sort_by(|a, b| a.0.cmp(b.0));
    for (name, value) in instance_entries {
        if WidgetKind::parse(name.as_str()).is_some() {
            continue;
        }
        let Some(kind) = Config::instance_kind(value) else {
            continue;
        };
        classify(
            name.to_string(),
            format!("{} instance", kind.as_str()),
            WidgetSource::Instance { kind },
        );
    }
    for name in plugin_names {
        classify(
            name.clone(),
            "wasm plugin".to_string(),
            WidgetSource::Plugin,
        );
    }
    // Anything still placed in the layout after the three catalog passes
    // above is a name none of them recognized — surface it rather than
    // silently dropping it (see the doc comment above).
    for region in Region::ALL {
        for name in cfg.layout.get(region) {
            classify(
                name.to_string(),
                "placed but not a recognized widget (unknown)".to_string(),
                WidgetSource::Unknown,
            );
        }
    }

    instances.sort_by(|a, b| a.0.cmp(&b.0));
    plugins.sort_by(|a, b| a.0.cmp(&b.0));
    unknown.sort_by(|a, b| a.0.cmp(&b.0));

    builtins
        .into_iter()
        .chain(instances)
        .chain(plugins)
        .chain(unknown)
        .map(|(name, summary, source)| {
            let placement = cfg.layout.find(&name);
            WidgetPlacement {
                name,
                summary,
                source,
                placement,
            }
        })
        .collect()
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
    /// Log-file path override (`~/` expanded by the binary), decomposed by
    /// the binary into a directory (the path's parent), a filename prefix
    /// (its stem), and a filename suffix (its extension) — the pieces
    /// `tracing-appender`'s daily rotation needs to name each generation
    /// `{prefix}.<date>.{suffix}`. Default: unset, which resolves to
    /// `$XDG_DATA_HOME/rustline/rustline.<date>.log`.
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
/// Capability fields (`allowed_urls`, `allowed_paths`, `allowed_write_paths`,
/// `allowed_commands`, `max_state_bytes`) are enforced by the WASM host,
/// never by the guest. `options` is opaque to the host and forwarded to the
/// plugin verbatim.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginConfig {
    #[serde(default)]
    pub source: Option<PluginSource>,
    #[serde(default)]
    pub allowed_urls: Vec<String>,
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    /// Filesystem paths this plugin may **write**, as globs. Deliberately
    /// separate from `allowed_paths`, which is read-only: the two used to be
    /// one list, so approving a manifest that looked like "reads your aliases
    /// to show a badge" also handed the plugin arbitrary overwrite of those
    /// files. Empty by default — deny by default (invariant N1).
    ///
    /// Migration: an existing `allowed_paths` entry grants read only. A plugin
    /// that was writing through it now fails closed, loudly (a denial record
    /// plus a log line), rather than silently.
    #[serde(default)]
    pub allowed_write_paths: Vec<String>,
    /// Resolve symlinks before matching a path against the allowlists.
    ///
    /// Off by default, in which case a path whose components include a symlink
    /// is denied outright: a grant is otherwise a grant over *names*, and any
    /// symlink planted under a granted prefix — by an extracted archive, a
    /// synced directory, another tool — silently redirects the effect to its
    /// target. Turning this on resolves the path first and matches the
    /// allowlist against the *resolved* location, which is safe but means a
    /// grant follows links out of the directory the user thought they granted.
    #[serde(default)]
    pub resolve_symlinks: bool,
    /// Command allow-patterns for the exec capability. Each entry is a glob by
    /// default, or a regex when prefixed `re:`, matched against the
    /// **canonical argv string** (`rustline_wasm::canonical_argv`) — the whole
    /// command line, not just the program. Empty (the default) matches
    /// nothing: deny by default, like the other two allowlists.
    #[serde(default)]
    pub allowed_commands: Vec<String>,
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
            allowed_write_paths: Vec::new(),
            resolve_symlinks: false,
            allowed_commands: Vec::new(),
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
    /// Where `rustline plugin search` fetches the curated plugin index from;
    /// overrides the built-in default URL. Lets a user point at a self-hosted
    /// or alternate index without a code change.
    #[serde(default)]
    pub plugin_index_url: Option<String>,
    /// Per-plugin config, keyed by plugin name.
    #[serde(default)]
    pub plugins: HashMap<WidgetName, PluginConfig>,
    /// File + stderr logging configuration.
    #[serde(default)]
    pub log: LogConfig,
    /// Extra named widget instances (W46), keyed by instance name. Each value
    /// is the raw `[instances.<name>]` table; `kind` selects the widget type
    /// and the remaining keys are that kind's options (re-parsed per kind at
    /// registration).
    ///
    /// This field is `pub`, but [`Config::resolved_instances`] memoizes its
    /// parse into `resolved` below on first call and never invalidates that
    /// memo. Mutating `instances` after `resolved_instances()` has already
    /// run on this `Config` serves stale specs to every later caller —
    /// production never mutates a `Config` post-construction, so this is
    /// theoretical there, but a test or future caller that edits `instances`
    /// in place and then calls a memo-backed method (`color_overrides`,
    /// `click_map`, `layout_kinds`, etc.) will observe the pre-edit data.
    #[serde(default)]
    pub instances: HashMap<WidgetName, Value>,
    /// Parse-once memo for `[instances.*]` tables (T5), lazily built by
    /// [`Config::resolved_instances`] on first call and reused by every
    /// consumer (`color_overrides`, `click_map`, `layout_kinds`,
    /// `disk_mounts`, `throughput_interfaces`, `spark_referenced_in_layout`,
    /// and `Registry::with_builtins`'s registration pass) — so a table with
    /// several consumers is parsed exactly once, not once per consumer.
    /// Skipped by serde entirely (never round-trips): a cloned or freshly
    /// deserialized `Config` just re-resolves lazily on next use, which is
    /// harmless since the memo is pure derived data.
    #[serde(skip)]
    resolved: OnceLock<BTreeMap<WidgetName, InstanceParse>>,
}

/// The closed set of widget kinds — the sixteen built-ins
/// ([`crate::widget::Registry::with_builtins`]) plus the type a
/// `[instances.<name>]` table's `kind` key must name (T5).
///
/// `#[serde(rename_all = "snake_case")]` covers fourteen of the sixteen
/// variants; [`WidgetKind::DateTime`] and [`WidgetKind::LoadAvg`] each need an
/// explicit `#[serde(rename)]` because plain snake_case would emit
/// `date_time`/`load_avg`, which are NOT the accepted TOML spellings
/// (`datetime`/`loadavg`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetKind {
    PaneId,
    Hostname,
    Windows,
    #[serde(rename = "datetime")]
    DateTime,
    Cwd,
    LanIp,
    TailscaleIp,
    Battery,
    Cpu,
    Memory,
    #[serde(rename = "loadavg")]
    LoadAvg,
    Git,
    Disk,
    Uptime,
    Media,
    Throughput,
}

impl WidgetKind {
    /// Every widget kind, in `Registry::with_builtins`'s registration order —
    /// the same order `BUILTIN_WIDGET_NAMES` used to hand-maintain, now
    /// impossible to drift out of sync with [`WidgetKind::as_str`] since both
    /// live on the enum itself.
    pub const ALL: [WidgetKind; 16] = [
        WidgetKind::PaneId,
        WidgetKind::Hostname,
        WidgetKind::Windows,
        WidgetKind::DateTime,
        WidgetKind::Cwd,
        WidgetKind::LanIp,
        WidgetKind::TailscaleIp,
        WidgetKind::Battery,
        WidgetKind::Cpu,
        WidgetKind::Memory,
        WidgetKind::LoadAvg,
        WidgetKind::Git,
        WidgetKind::Disk,
        WidgetKind::Uptime,
        WidgetKind::Media,
        WidgetKind::Throughput,
    ];

    /// The exact TOML/layout spelling for this kind — also the built-in
    /// widget's own registered name.
    pub fn as_str(self) -> &'static str {
        match self {
            WidgetKind::PaneId => "pane_id",
            WidgetKind::Hostname => "hostname",
            WidgetKind::Windows => "windows",
            WidgetKind::DateTime => "datetime",
            WidgetKind::Cwd => "cwd",
            WidgetKind::LanIp => "lan_ip",
            WidgetKind::TailscaleIp => "tailscale_ip",
            WidgetKind::Battery => "battery",
            WidgetKind::Cpu => "cpu",
            WidgetKind::Memory => "memory",
            WidgetKind::LoadAvg => "loadavg",
            WidgetKind::Git => "git",
            WidgetKind::Disk => "disk",
            WidgetKind::Uptime => "uptime",
            WidgetKind::Media => "media",
            WidgetKind::Throughput => "throughput",
        }
    }

    /// Parse a layout/TOML kind string, if it names one of the sixteen kinds.
    pub fn parse(s: &str) -> Option<WidgetKind> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// The twelve clickable/format-bearing kinds an `[instances.<name>]`
    /// table may declare (`kind = "…"`); `cwd`/`hostname`/`pane_id`/`windows`
    /// are instanceable in name only — they carry no clickable identity to
    /// give an instance, so `Registry::with_builtins` never builds one for
    /// them (see the Config module doc's "Multiple widget instances" note).
    pub fn is_instanceable(self) -> bool {
        !matches!(
            self,
            WidgetKind::PaneId | WidgetKind::Hostname | WidgetKind::Windows | WidgetKind::Cwd
        )
    }
}

/// One `[instances.<name>]` table's parsed, typed options — the twelve
/// instanceable [`WidgetKind`]s' own `<Kind>Opts` struct, wrapped so
/// [`Config::resolved_instances`] can hand back one typed value per instance
/// regardless of kind.
#[derive(Clone, Debug)]
pub enum InstanceSpec {
    DateTime(DateTimeOpts),
    LanIp(LanIpOpts),
    TailscaleIp(TailscaleIpOpts),
    Battery(BatteryOpts),
    Cpu(CpuOpts),
    Memory(MemoryOpts),
    LoadAvg(LoadAvgOpts),
    Git(GitOpts),
    Disk(DiskOpts),
    Uptime(UptimeOpts),
    Media(MediaOpts),
    Throughput(ThroughputOpts),
}

impl InstanceSpec {
    /// Which kind this instance was built from.
    pub fn kind(&self) -> WidgetKind {
        match self {
            InstanceSpec::DateTime(_) => WidgetKind::DateTime,
            InstanceSpec::LanIp(_) => WidgetKind::LanIp,
            InstanceSpec::TailscaleIp(_) => WidgetKind::TailscaleIp,
            InstanceSpec::Battery(_) => WidgetKind::Battery,
            InstanceSpec::Cpu(_) => WidgetKind::Cpu,
            InstanceSpec::Memory(_) => WidgetKind::Memory,
            InstanceSpec::LoadAvg(_) => WidgetKind::LoadAvg,
            InstanceSpec::Git(_) => WidgetKind::Git,
            InstanceSpec::Disk(_) => WidgetKind::Disk,
            InstanceSpec::Uptime(_) => WidgetKind::Uptime,
            InstanceSpec::Media(_) => WidgetKind::Media,
            InstanceSpec::Throughput(_) => WidgetKind::Throughput,
        }
    }

    /// This instance's color override, for [`Config::color_overrides`].
    fn color(&self) -> &ColorOverride {
        match self {
            InstanceSpec::DateTime(o) => &o.color,
            InstanceSpec::LanIp(o) => &o.color,
            InstanceSpec::TailscaleIp(o) => &o.color,
            InstanceSpec::Battery(o) => &o.color,
            InstanceSpec::Cpu(o) => &o.color,
            InstanceSpec::Memory(o) => &o.color,
            InstanceSpec::LoadAvg(o) => &o.color,
            InstanceSpec::Git(o) => &o.color,
            InstanceSpec::Disk(o) => &o.color,
            InstanceSpec::Uptime(o) => &o.color,
            InstanceSpec::Media(o) => &o.color,
            InstanceSpec::Throughput(o) => &o.color,
        }
    }

    /// This instance's `alt_format`, for [`Config::click_map`].
    fn alt_format(&self) -> &str {
        match self {
            InstanceSpec::DateTime(o) => &o.alt_format,
            InstanceSpec::LanIp(o) => &o.alt_format,
            InstanceSpec::TailscaleIp(o) => &o.alt_format,
            InstanceSpec::Battery(o) => &o.alt_format,
            InstanceSpec::Cpu(o) => &o.alt_format,
            InstanceSpec::Memory(o) => &o.alt_format,
            InstanceSpec::LoadAvg(o) => &o.alt_format,
            InstanceSpec::Git(o) => &o.alt_format,
            InstanceSpec::Disk(o) => &o.alt_format,
            InstanceSpec::Uptime(o) => &o.alt_format,
            InstanceSpec::Media(o) => &o.alt_format,
            InstanceSpec::Throughput(o) => &o.alt_format,
        }
    }

    /// This instance's click bindings, for [`Config::click_map`].
    fn click(&self) -> &ClickBindings {
        match self {
            InstanceSpec::DateTime(o) => &o.click,
            InstanceSpec::LanIp(o) => &o.click,
            InstanceSpec::TailscaleIp(o) => &o.click,
            InstanceSpec::Battery(o) => &o.click,
            InstanceSpec::Cpu(o) => &o.click,
            InstanceSpec::Memory(o) => &o.click,
            InstanceSpec::LoadAvg(o) => &o.click,
            InstanceSpec::Git(o) => &o.click,
            InstanceSpec::Disk(o) => &o.click,
            InstanceSpec::Uptime(o) => &o.click,
            InstanceSpec::Media(o) => &o.click,
            InstanceSpec::Throughput(o) => &o.click,
        }
    }
}

/// The outcome of resolving one `[instances.<name>]` table (T5) — the
/// degrade-per-entry counterpart to [`instance_opts`]'s degrade-per-field:
/// one bad instance never takes any other instance, or `Config::load`, down
/// with it (invariant #3).
// `InstanceSpec`'s largest variant (an Opts struct with several `String`
// fields plus `ColorOverride`/`ClickBindings`) makes `Ok` far bigger than
// `UnknownKind`/`NoKind`, but this enum lives in a `BTreeMap` sized by a
// user's handful of configured `[instances.*]` entries, not a hot per-render
// path — and boxing `InstanceSpec` would defeat the direct
// `InstanceParse::Ok(InstanceSpec::Kind(_))` match ergonomics every consumer
// (including the pinning test) relies on.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum InstanceParse {
    /// A resolvable, instanceable `kind` — the typed options are ready to
    /// build a widget from.
    Ok(InstanceSpec),
    /// The table has no `kind` key (or it isn't a string) — nothing to gate
    /// on, so the instance is skipped (registration warns once).
    NoKind,
    /// `kind` is a string, but not one of [`WidgetKind::ALL`]'s sixteen
    /// spellings, OR it names a valid kind that isn't instanceable
    /// (`cwd`/`hostname`/`pane_id`/`windows`) — today's registration pass
    /// (`Registry::with_builtins`) treats both cases identically (the same
    /// "unknown instance kind, skipping" warn), so this variant does too
    /// rather than adding a distinction nothing currently draws.
    UnknownKind(String),
}

/// The single home for the `{spark}` literal check: a widget references the
/// sparkline placeholder in EITHER its `format` or its click-toggle `alt_format`.
fn refs_spark(format: &str, alt_format: &str) -> bool {
    format.contains("{spark}") || alt_format.contains("{spark}")
}

/// Deserialize an `[instances.<name>]` options table, falling back to
/// `T::default()` on a type error — but *reporting* it first.
///
/// The silent `unwrap_or_default()` this replaces threw away the user's whole
/// instance config (format, thresholds, colour override) on one bad value and
/// still registered the name, so `resolve` found it and no "unknown widget"
/// warn fired either: the edit looked accepted and was ignored. Routed through
/// `warn_once` because a misconfiguration persists across every render tick.
pub(crate) fn instance_opts<T: Default + serde::de::DeserializeOwned>(
    name: &str,
    kind: &str,
    v: Value,
) -> T {
    match v.try_into() {
        Ok(o) => o,
        Err(error) => {
            let error = error.to_string();
            crate::diag::warn_once(&format!("instance-opts:{name}:{error}"), || {
                tracing::warn!(
                    instance = %name,
                    kind = %kind,
                    %error,
                    "invalid instance options, using defaults"
                );
            });
            T::default()
        }
    }
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
    /// byte-identical case cheap and the common case). Also projects
    /// `[instances.<name>]` entries with a resolvable `kind` the same way
    /// (via [`Config::resolved_instances`], W46/T5), so a named instance's
    /// color override is included under its own instance name — unless that
    /// name collides with a built-in, in which case the instance is skipped
    /// entirely (built-in wins; see [`WidgetKind::parse`]).
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
        let mut overrides: HashMap<String, ColorOverride> = candidates
            .into_iter()
            .filter(|(_, color)| color.fg.is_some() || color.bg.is_some())
            .map(|(name, color)| (name.to_string(), color.clone()))
            .collect();
        for (name, parse) in self.resolved_instances() {
            if WidgetKind::parse(name.as_str()).is_some() {
                continue;
            }
            let InstanceParse::Ok(spec) = parse else {
                continue;
            };
            let color = spec.color();
            if color.fg.is_some() || color.bg.is_some() {
                overrides.insert(name.to_string(), color.clone());
            }
        }
        overrides
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
    /// projector. Also projects `[instances.<name>]` entries with a
    /// resolvable `kind` the same way (via [`Config::resolved_instances`],
    /// W46/T5), so a named instance's toggleability/bindings are included
    /// under its own instance name — unless that name collides with a
    /// built-in, in which case the instance is skipped entirely (built-in
    /// wins; see [`WidgetKind::parse`]).
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
        let mut map: HashMap<String, WidgetClick> = candidates
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
            .collect();
        for (name, parse) in self.resolved_instances() {
            if WidgetKind::parse(name.as_str()).is_some() {
                continue;
            }
            let InstanceParse::Ok(spec) = parse else {
                continue;
            };
            map.insert(
                name.to_string(),
                WidgetClick {
                    toggleable: !spec.alt_format().is_empty(),
                    bindings: spec.click().clone(),
                },
            );
        }
        map
    }

    /// The `kind` of a `[instances.<name>]` table, if present, a string, and
    /// one of [`WidgetKind::ALL`]'s sixteen spellings.
    pub fn instance_kind(v: &Value) -> Option<WidgetKind> {
        v.get("kind")
            .and_then(Value::as_str)
            .and_then(WidgetKind::parse)
    }

    /// Parse every `[instances.<name>]` table exactly once, memoizing the
    /// result for the lifetime of this `Config` (T5). Every consumer that
    /// used to re-parse a table per call (`color_overrides`, `click_map`,
    /// `layout_kinds`, `disk_mounts`, `throughput_interfaces`,
    /// `spark_referenced_in_layout`, and `Registry::with_builtins`'s
    /// registration pass) now looks up this map instead.
    ///
    /// Degrades per entry, never per `Config` (invariant #3): a table with no
    /// `kind` key is [`InstanceParse::NoKind`]; a `kind` that isn't one of
    /// [`WidgetKind::ALL`]'s sixteen spellings, OR names a valid-but-
    /// non-instanceable kind (`cwd`/`hostname`/`pane_id`/`windows`), is
    /// [`InstanceParse::UnknownKind`] — today's registration pass treats both
    /// identically (see that variant's doc), so this does too; anything else
    /// parses its kind's `<Kind>Opts` via [`instance_opts`] (itself total,
    /// falling back to that kind's defaults and reporting the mismatch once
    /// via `warn_once` on a type error) into [`InstanceParse::Ok`].
    pub fn resolved_instances(&self) -> &BTreeMap<WidgetName, InstanceParse> {
        self.resolved.get_or_init(|| {
            self.instances
                .iter()
                .map(|(name, table)| {
                    let parse = match table.get("kind").and_then(Value::as_str) {
                        None => InstanceParse::NoKind,
                        Some(s) => match WidgetKind::parse(s).filter(|k| k.is_instanceable()) {
                            Some(kind) => InstanceParse::Ok(build_instance_spec(
                                name.as_str(),
                                kind,
                                table.clone(),
                            )),
                            None => InstanceParse::UnknownKind(s.to_string()),
                        },
                    };
                    (name.clone(), parse)
                })
                .collect()
        })
    }

    /// The set of widget *kinds* a `layout` (a region's name list) references —
    /// the kind-aware basis for read-gating in the binary's
    /// `build_region_context` (W46).
    ///
    /// A built-in name always maps to its own [`WidgetKind`], even when
    /// `self.instances` has a colliding key — built-in wins, mirroring the
    /// same precedence [`Config::color_overrides`]/[`Config::click_map`]
    /// apply (invariant #7). For any other name: an `[instances.<name>]`
    /// entry maps to its resolved kind (an instance with no resolvable kind —
    /// [`InstanceParse::NoKind`]/[`InstanceParse::UnknownKind`] — is dropped,
    /// since there's nothing to gate on); any other name (a WASM plugin, or a
    /// stale/unknown name) maps to nothing — a semantic tightening from the
    /// pre-T5 "maps to itself harmlessly," safe because every caller
    /// (`build_context.rs`, `doctor.rs`) only ever probes built-in
    /// [`WidgetKind`] values here, never a plugin/unknown name.
    pub fn layout_kinds(&self, layout: &[WidgetName]) -> BTreeSet<WidgetKind> {
        layout
            .iter()
            .filter_map(|name| {
                if let Some(kind) = WidgetKind::parse(name.as_str()) {
                    return Some(kind);
                }
                match self.resolved_instances().get(name) {
                    Some(InstanceParse::Ok(spec)) => Some(spec.kind()),
                    _ => None,
                }
            })
            .collect()
    }

    /// The distinct filesystem mounts a `layout` needs read: the base
    /// `[widgets.disk].mount` when the built-in `disk` is referenced, plus each
    /// `disk`-kind instance's own `mount`. The binary reads one `DiskInfo` per
    /// mount into `Context.disks` (W46), so two `disk` instances on different
    /// mounts each get a live reading instead of clobbering a single one.
    pub fn disk_mounts(&self, layout: &[WidgetName]) -> BTreeSet<String> {
        let mut mounts: BTreeSet<String> = self
            .instances_of_kind(layout, WidgetKind::Disk)
            .filter_map(|(_, spec)| match spec {
                InstanceSpec::Disk(o) => Some(o.mount.clone()),
                _ => None,
            })
            .collect();
        if layout.iter().any(|name| name == "disk") {
            mounts.insert(self.widgets.disk.mount.clone());
        }
        mounts
    }

    /// The distinct network interfaces a `layout` needs read: the base
    /// `[widgets.throughput].interface` when the built-in `throughput` is
    /// referenced, plus each `throughput`-kind instance's own `interface`.
    /// `None` is the aggregate-every-interface selector (deduped with any other
    /// aggregate request). The binary reads one `Throughput` per entry into
    /// `Context.throughputs`, keyed by `interface.unwrap_or_default()` (W46).
    pub fn throughput_interfaces(&self, layout: &[WidgetName]) -> BTreeSet<Option<String>> {
        let mut ifaces: BTreeSet<Option<String>> = self
            .instances_of_kind(layout, WidgetKind::Throughput)
            .filter_map(|(_, spec)| match spec {
                InstanceSpec::Throughput(o) => Some(o.interface.clone()),
                _ => None,
            })
            .collect();
        if layout.iter().any(|name| name == "throughput") {
            ifaces.insert(self.widgets.throughput.interface.clone());
        }
        ifaces
    }

    /// Does any `cpu`/`memory` widget IN the layout — the base widget OR a
    /// `[instances.<name>]` of that kind — reference `{spark}` in its
    /// `format`/`alt_format`? Gates the shared history read/persist so an
    /// instance-only `{spark}` still accumulates (W57). Non-cpu/memory → false.
    pub fn spark_referenced_in_layout(&self, layout: &[WidgetName], kind: WidgetKind) -> bool {
        let base_hit = layout.iter().any(|n| n == kind.as_str())
            && match kind {
                WidgetKind::Cpu => {
                    refs_spark(&self.widgets.cpu.format, &self.widgets.cpu.alt_format)
                }
                WidgetKind::Memory => {
                    refs_spark(&self.widgets.memory.format, &self.widgets.memory.alt_format)
                }
                _ => false,
            };
        base_hit
            || self
                .instances_of_kind(layout, kind)
                .any(|(_, spec)| match spec {
                    InstanceSpec::Cpu(o) => refs_spark(&o.format, &o.alt_format),
                    InstanceSpec::Memory(o) => refs_spark(&o.format, &o.alt_format),
                    _ => false,
                })
    }

    /// Iterate the `[instances.<name>]` tables a `layout` references that
    /// resolved (via [`Config::resolved_instances`]) to a given `kind`, in
    /// layout order, paired with each instance's own name — the shared spine
    /// of [`Config::disk_mounts`], [`Config::throughput_interfaces`], and
    /// [`Config::spark_referenced_in_layout`].
    fn instances_of_kind<'a>(
        &'a self,
        layout: &'a [WidgetName],
        kind: WidgetKind,
    ) -> impl Iterator<Item = (&'a str, &'a InstanceSpec)> {
        layout.iter().filter_map(move |name| {
            // Built-in-wins precedence: a built-in name never resolves to a
            // same-named `[instances.<name>]` entry (invariant #7), matching the
            // guard `color_overrides`/`click_map`/`layout_kinds` already apply.
            if WidgetKind::parse(name.as_str()).is_some() {
                return None;
            }
            match self.resolved_instances().get(name) {
                Some(InstanceParse::Ok(spec)) if spec.kind() == kind => Some((name.as_str(), spec)),
                _ => None,
            }
        })
    }
}

/// Parse one `[instances.<name>]` table into its kind's typed
/// [`InstanceSpec`] variant, via [`instance_opts`] (total: a type-mismatched
/// table falls back to that kind's defaults, reporting the mismatch once).
/// Only called for an already-confirmed [`WidgetKind::is_instanceable`] kind —
/// see [`Config::resolved_instances`], the sole caller.
fn build_instance_spec(name: &str, kind: WidgetKind, table: Value) -> InstanceSpec {
    let k = kind.as_str();
    match kind {
        WidgetKind::DateTime => InstanceSpec::DateTime(instance_opts(name, k, table)),
        WidgetKind::LanIp => InstanceSpec::LanIp(instance_opts(name, k, table)),
        WidgetKind::TailscaleIp => InstanceSpec::TailscaleIp(instance_opts(name, k, table)),
        WidgetKind::Battery => InstanceSpec::Battery(instance_opts(name, k, table)),
        WidgetKind::Cpu => InstanceSpec::Cpu(instance_opts(name, k, table)),
        WidgetKind::Memory => InstanceSpec::Memory(instance_opts(name, k, table)),
        WidgetKind::LoadAvg => InstanceSpec::LoadAvg(instance_opts(name, k, table)),
        WidgetKind::Git => InstanceSpec::Git(instance_opts(name, k, table)),
        WidgetKind::Disk => InstanceSpec::Disk(instance_opts(name, k, table)),
        WidgetKind::Uptime => InstanceSpec::Uptime(instance_opts(name, k, table)),
        WidgetKind::Media => InstanceSpec::Media(instance_opts(name, k, table)),
        WidgetKind::Throughput => InstanceSpec::Throughput(instance_opts(name, k, table)),
        WidgetKind::PaneId | WidgetKind::Hostname | WidgetKind::Windows | WidgetKind::Cwd => {
            unreachable!("build_instance_spec is only called for an instanceable kind")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// T6 pinning test (written first): `WidgetName` must slot into `Layout`'s
    /// arrays and `Config.plugins`/`Config.instances`'s map keys without
    /// changing a single byte of the TOML shape — a plain array of strings and
    /// plain dotted-table keys, exactly as before the newtype existed.
    #[test]
    fn layout_and_map_keys_keep_their_toml_shape_with_widget_name() {
        let cfg: Config = toml::from_str(
            r#"
        [layout]
        left = ["pane_id", "hostname"]
        [plugins.weather]
        allowed_urls = ["https://wttr.in/*"]
        [instances.clock_utc]
        kind = "datetime"
    "#,
        )
        .unwrap();
        assert_eq!(
            cfg.layout.left,
            vec![WidgetName::from("pane_id"), WidgetName::from("hostname")]
        );
        assert!(cfg.plugins.contains_key("weather"));
        let back = toml::to_string(&cfg).unwrap();
        assert!(back.contains(r#"left = ["pane_id", "hostname"]"#));
    }

    #[test]
    fn widget_kind_toml_spellings_are_the_accepted_ones() {
        // The two explicit renames are load-bearing: snake_case alone would emit
        // date_time / load_avg, which are NOT the accepted TOML spellings.
        for k in WidgetKind::ALL {
            assert_eq!(WidgetKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(WidgetKind::parse("datetime"), Some(WidgetKind::DateTime));
        assert_eq!(WidgetKind::parse("loadavg"), Some(WidgetKind::LoadAvg));
        assert_eq!(WidgetKind::parse("date_time"), None);
        assert_eq!(WidgetKind::parse("load_avg"), None);
        assert_eq!(WidgetKind::ALL.len(), 16);
        assert_eq!(
            WidgetKind::ALL
                .iter()
                .filter(|k| k.is_instanceable())
                .count(),
            12
        );
    }

    #[test]
    fn resolved_instances_parses_each_table_exactly_once_and_degrades_per_entry() {
        let cfg: Config = toml::from_str(
            r#"
            [instances.clock_utc]
            kind = "datetime"
            timezone = "UTC"
            [instances.mystery]
            kind = "not_a_kind"
            [instances.kindless]
            format = "x"
        "#,
        )
        .unwrap();
        let r = cfg.resolved_instances();
        assert!(matches!(
            r.get("clock_utc"),
            Some(InstanceParse::Ok(InstanceSpec::DateTime(_)))
        ));
        assert!(
            matches!(r.get("mystery"), Some(InstanceParse::UnknownKind(k)) if k == "not_a_kind")
        );
        assert!(matches!(r.get("kindless"), Some(InstanceParse::NoKind)));
    }

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
    fn allowed_write_paths_and_resolve_symlinks_default_and_roundtrip() {
        // Absent -> empty/false (deny-by-default, resolution off by default).
        let default: PluginConfig = toml::from_str("").unwrap();
        assert!(default.allowed_write_paths.is_empty());
        assert!(!default.resolve_symlinks);

        let toml = concat!(
            "[plugins.weather]\n",
            "allowed_write_paths = [\"/home/u/notes/*\"]\n",
            "resolve_symlinks = true\n",
        );
        let c: Config = toml::from_str(toml).unwrap();
        let serialized = toml::to_string(&c).unwrap();
        let back: Config = toml::from_str(&serialized).unwrap();
        let w = back.plugins.get("weather").unwrap();
        assert_eq!(w.allowed_write_paths, vec!["/home/u/notes/*".to_string()]);
        assert!(w.resolve_symlinks);
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

    #[test]
    fn color_overrides_and_click_map_include_instances() {
        let mut c = Config::default();
        c.instances.insert(
            "clk".into(),
            toml::from_str("kind='datetime'\nalt_format='%H:%M'\nfg={ Indexed = 5 }").unwrap(),
        );
        assert!(c.color_overrides().contains_key("clk"));
        let cm = c.click_map();
        assert!(cm.get("clk").map(|w| w.toggleable).unwrap_or(false));
    }

    #[test]
    fn color_overrides_excludes_instance_without_color() {
        let mut c = Config::default();
        c.instances.insert(
            "clk".into(),
            toml::from_str("kind='datetime'\nalt_format='%H:%M'").unwrap(),
        );
        // No fg/bg set on the instance -> absent from color_overrides, matching
        // the built-in candidates' "only configured widgets" behavior.
        assert!(!c.color_overrides().contains_key("clk"));
        // It's still toggleable in click_map, independent of color.
        assert!(c.click_map().get("clk").unwrap().toggleable);
    }

    #[test]
    fn parses_instances_table_and_kind() {
        let toml = r#"
[layout]
right = ["clock_utc", "disk_data"]
[instances.clock_utc]
kind = "datetime"
timezone = "UTC"
format = "%H:%MZ"
[instances.disk_data]
kind = "disk"
mount = "/data"
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.instances.len(), 2);
        let utc = &c.instances["clock_utc"];
        assert_eq!(Config::instance_kind(utc), Some(WidgetKind::DateTime));
        assert_eq!(utc.get("timezone").and_then(|v| v.as_str()), Some("UTC"));
    }

    #[test]
    fn absent_instances_is_empty_and_roundtrips() {
        let c: Config = toml::from_str("").unwrap();
        assert!(c.instances.is_empty());
        let back: Config = toml::from_str(&toml::to_string(&c).unwrap()).unwrap();
        assert!(back.instances.is_empty());
    }

    #[test]
    fn layout_kinds_resolves_base_and_instances() {
        let mut c = Config::default();
        c.instances.insert(
            "disk_data".into(),
            toml::from_str("kind = 'disk'\nmount = '/data'").unwrap(),
        );
        // An instance name resolves to its kind; a base/built-in name to itself.
        let kinds = c.layout_kinds(&["disk_data".into(), "cpu".into()]);
        assert!(kinds.contains(&WidgetKind::Disk));
        assert!(kinds.contains(&WidgetKind::Cpu));
        // An instance whose table has no `kind` is dropped (nothing to gate on):
        // its name maps to nothing at all, not even to itself.
        c.instances
            .insert("bogus".into(), toml::from_str("mount = '/x'").unwrap());
        assert!(c.layout_kinds(&["bogus".into()]).is_empty());
    }

    #[test]
    fn disk_mounts_collects_base_and_instance_mounts() {
        let mut c = Config::default();
        c.instances.insert(
            "disk_data".into(),
            toml::from_str("kind = 'disk'\nmount = '/data'").unwrap(),
        );
        // Base `disk` in the layout -> its configured mount (default "/").
        assert!(c.disk_mounts(&["disk".into()]).contains("/"));
        // Instance only -> its own mount, and the base "/" is NOT included.
        let m = c.disk_mounts(&["disk_data".into()]);
        assert!(m.contains("/data"));
        assert!(!m.contains("/"));
        // Both referenced -> both mounts.
        let both = c.disk_mounts(&["disk".into(), "disk_data".into()]);
        assert!(both.contains("/") && both.contains("/data"));
        // A disk-kind instance the layout never references contributes nothing.
        assert!(c.disk_mounts(&[]).is_empty());
    }

    #[test]
    fn throughput_interfaces_collects_base_and_instances() {
        let mut c = Config::default();
        c.widgets.throughput.interface = Some("eth0".into());
        c.instances.insert(
            "net_wlan".into(),
            toml::from_str("kind = 'throughput'\ninterface = 'wlan0'").unwrap(),
        );
        c.instances.insert(
            "net_all".into(),
            toml::from_str("kind = 'throughput'").unwrap(),
        );
        // Base throughput in the layout -> its configured interface.
        assert!(
            c.throughput_interfaces(&["throughput".into()])
                .contains(&Some("eth0".into()))
        );
        // Instances -> their own interfaces; an interface-less instance -> the
        // `None` aggregate selector.
        let ifaces = c.throughput_interfaces(&["net_wlan".into(), "net_all".into()]);
        assert!(ifaces.contains(&Some("wlan0".into())));
        assert!(ifaces.contains(&None));
        // The base interface is absent when the base widget isn't referenced.
        assert!(!ifaces.contains(&Some("eth0".into())));
        // Nothing referenced -> empty.
        assert!(c.throughput_interfaces(&[]).is_empty());
    }

    #[test]
    fn spark_referenced_in_layout_covers_base_and_instances() {
        // Base cpu with {spark} in format.
        let mut cfg = Config::default();
        cfg.widgets.cpu.format = "{icon} {spark} {percent}%".into();
        assert!(cfg.spark_referenced_in_layout(&["cpu".into()], WidgetKind::Cpu));

        // Base default (no spark), but a cpu instance IN the layout has {spark}.
        let mut cfg2 = Config::default();
        let mut t = toml::value::Table::new();
        t.insert("kind".into(), "cpu".into());
        t.insert("format".into(), "{spark}".into());
        cfg2.instances.insert("cpu2".into(), toml::Value::Table(t));
        assert!(
            cfg2.spark_referenced_in_layout(&["cpu2".into()], WidgetKind::Cpu),
            "instance-only {{spark}} counts"
        );
        // Same instance NOT in the layout → false.
        assert!(!cfg2.spark_referenced_in_layout(&["cpu".into()], WidgetKind::Cpu));

        // Neither base nor instance references spark → false; wrong kind → false.
        let cfg3 = Config::default();
        assert!(!cfg3.spark_referenced_in_layout(&["cpu".into()], WidgetKind::Cpu));
        assert!(!cfg3.spark_referenced_in_layout(&["disk".into()], WidgetKind::Disk));

        // Memory base widget with {spark} — the structurally-identical memory arm.
        let mut cfg_mem = Config::default();
        cfg_mem.widgets.memory.format = "{icon} {spark} {percent}%".into();
        assert!(cfg_mem.spark_referenced_in_layout(&["memory".into()], WidgetKind::Memory));
    }

    #[test]
    fn color_overrides_and_click_map_skip_instance_colliding_with_builtin_name() {
        // Regression for the W46 review finding: `[instances.cpu]` must never
        // shadow the built-in `cpu` widget in these two projections — the
        // built-in always wins the name "cpu", mirroring
        // `Registry::with_builtins`'s `registry.contains` skip (invariant #7).
        let mut c = Config::default();
        c.instances.insert(
            "cpu".into(),
            toml::from_str("kind='datetime'\nalt_format='X'\nfg={ Indexed = 1 }").unwrap(),
        );
        // Built-in `[widgets.cpu]` is untouched (no color, no alt_format), so
        // the colliding instance's fg/alt_format must not leak through.
        assert!(!c.color_overrides().contains_key("cpu"));
        assert!(!c.click_map().get("cpu").unwrap().toggleable);
    }

    #[test]
    fn layout_kinds_builtin_name_wins_over_colliding_instance_kind() {
        // Same collision, this time against `layout_kinds`: a built-in name
        // in the layout must resolve to itself, not a same-named instance's
        // (different) declared kind.
        let mut c = Config::default();
        c.instances
            .insert("cpu".into(), toml::from_str("kind = 'disk'").unwrap());
        let kinds = c.layout_kinds(&["cpu".into()]);
        assert!(kinds.contains(&WidgetKind::Cpu));
        assert!(!kinds.contains(&WidgetKind::Disk));
    }

    #[test]
    fn noncolliding_instance_still_projects_after_builtin_guard() {
        // Regression guard for the fix above: a non-colliding instance name
        // must still participate in every projection unchanged.
        let mut c = Config::default();
        c.instances.insert(
            "clock_utc".into(),
            toml::from_str("kind='datetime'\nalt_format='X'\nfg={ Indexed = 2 }").unwrap(),
        );
        assert!(c.color_overrides().contains_key("clock_utc"));
        assert!(c.click_map().get("clock_utc").unwrap().toggleable);
        assert!(
            c.layout_kinds(&["clock_utc".into()])
                .contains(&WidgetKind::DateTime)
        );
    }

    #[test]
    fn disk_mounts_skip_instance_colliding_with_builtin_name() {
        // Built-in-wins precedence in `instances_of_kind` (M7): an
        // `[instances.cpu]` declaring `kind='disk'` must never contribute its
        // mount, because the built-in `cpu` owns the name "cpu" (invariant #7),
        // matching the guard color_overrides/click_map/layout_kinds apply.
        let mut c = Config::default();
        c.instances.insert(
            "cpu".into(),
            toml::from_str("kind = 'disk'\nmount = '/hijack'").unwrap(),
        );
        let mounts = c.disk_mounts(&["cpu".into()]);
        assert!(!mounts.contains("/hijack"));
    }

    fn sample_layout() -> Layout {
        Layout {
            left: vec!["pane_id".into(), "hostname".into()],
            center: vec!["windows".into()],
            right: vec!["cwd".into(), "cpu".into(), "datetime".into()],
        }
    }

    #[test]
    fn region_parse_is_case_insensitive_and_round_trips() {
        assert_eq!(Region::parse("LEFT"), Some(Region::Left));
        assert_eq!(Region::parse("Center"), Some(Region::Center));
        assert_eq!(Region::parse("right"), Some(Region::Right));
        assert_eq!(Region::parse("middle"), None);
        for r in Region::ALL {
            assert_eq!(Region::parse(r.as_str()), Some(r));
        }
    }

    #[test]
    fn find_locates_a_widget_in_any_region() {
        let l = sample_layout();
        assert_eq!(l.find("hostname"), Some((Region::Left, 1)));
        assert_eq!(l.find("windows"), Some((Region::Center, 0)));
        assert_eq!(l.find("datetime"), Some((Region::Right, 2)));
        assert_eq!(l.find("git"), None);
    }

    #[test]
    fn enable_appends_when_index_is_none() {
        let mut l = sample_layout();
        let change = layout_enable(&mut l, "git", Region::Right, None).unwrap();
        assert_eq!(l.right, ["cwd", "cpu", "datetime", "git"]);
        assert_eq!(change.from, None);
        assert_eq!(change.to, Some((Region::Right, 3)));
    }

    #[test]
    fn enable_inserts_at_a_clamped_index() {
        let mut l = sample_layout();
        layout_enable(&mut l, "git", Region::Right, Some(1)).unwrap();
        assert_eq!(l.right, ["cwd", "git", "cpu", "datetime"]);

        let mut l2 = sample_layout();
        layout_enable(&mut l2, "git", Region::Right, Some(99)).unwrap();
        assert_eq!(l2.right, ["cwd", "cpu", "datetime", "git"]);
    }

    #[test]
    fn enable_rejects_a_name_already_present_in_another_region_and_does_not_mutate() {
        let mut l = sample_layout();
        let before = l.clone();
        let err = layout_enable(&mut l, "hostname", Region::Right, None).unwrap_err();
        assert_eq!(
            err,
            LayoutEditError::AlreadyPresent {
                region: Region::Left,
                index: 1
            }
        );
        assert_eq!(l, before, "error path must not mutate");
    }

    #[test]
    fn disable_removes_and_reports_where_it_was() {
        let mut l = sample_layout();
        let change = layout_disable(&mut l, "cpu").unwrap();
        assert_eq!(l.right, ["cwd", "datetime"]);
        assert_eq!(change.from, Some((Region::Right, 1)));
        assert_eq!(change.to, None);
    }

    #[test]
    fn disable_of_an_absent_name_errors_without_mutating() {
        let mut l = sample_layout();
        let before = l.clone();
        assert_eq!(
            layout_disable(&mut l, "git").unwrap_err(),
            LayoutEditError::NotPresent
        );
        assert_eq!(l, before);
    }

    #[test]
    fn move_across_regions_removes_from_the_old_one() {
        let mut l = sample_layout();
        let change = layout_move(&mut l, "hostname", Region::Right, 0).unwrap();
        assert_eq!(l.left, ["pane_id"]);
        assert_eq!(l.right, ["hostname", "cwd", "cpu", "datetime"]);
        assert_eq!(change.from, Some((Region::Left, 1)));
        assert_eq!(change.to, Some((Region::Right, 0)));
    }

    #[test]
    fn move_within_a_region_reindexes_correctly() {
        let mut l = sample_layout();
        layout_move(&mut l, "cwd", Region::Right, 2).unwrap();
        assert_eq!(l.right, ["cpu", "datetime", "cwd"]);
    }

    #[test]
    fn move_clamps_an_out_of_range_index_instead_of_erroring() {
        let mut l = sample_layout();
        layout_move(&mut l, "cwd", Region::Right, 99).unwrap();
        assert_eq!(l.right, ["cpu", "datetime", "cwd"]);
    }

    #[test]
    fn move_to_where_it_already_is_is_a_noop_error() {
        let mut l = sample_layout();
        let before = l.clone();
        assert_eq!(
            layout_move(&mut l, "cpu", Region::Right, 1).unwrap_err(),
            LayoutEditError::NoOp
        );
        assert_eq!(l, before);
    }

    #[test]
    fn nudge_moves_one_step_inside_its_own_region() {
        let mut l = sample_layout();
        layout_nudge(&mut l, "cpu", -1).unwrap();
        assert_eq!(l.right, ["cpu", "cwd", "datetime"]);
        layout_nudge(&mut l, "cpu", 1).unwrap();
        assert_eq!(l.right, ["cwd", "cpu", "datetime"]);
    }

    #[test]
    fn nudge_at_a_region_boundary_is_a_noop_not_a_wraparound() {
        let mut l = sample_layout();
        let before = l.clone();
        assert_eq!(
            layout_nudge(&mut l, "cwd", -1).unwrap_err(),
            LayoutEditError::NoOp
        );
        assert_eq!(
            layout_nudge(&mut l, "datetime", 1).unwrap_err(),
            LayoutEditError::NoOp
        );
        assert_eq!(l, before);
    }

    #[test]
    fn nudge_of_an_absent_name_errors() {
        let mut l = sample_layout();
        assert_eq!(
            layout_nudge(&mut l, "git", 1).unwrap_err(),
            LayoutEditError::NotPresent
        );
    }

    #[test]
    fn placements_mark_where_each_widget_sits_and_leave_the_rest_unplaced() {
        let cfg = Config {
            layout: Layout {
                left: vec!["pane_id".into()],
                center: vec![],
                right: vec!["cpu".into(), "weather".into()],
            },
            ..Default::default()
        };
        let descriptors = vec![
            WidgetDescriptor {
                name: "pane_id".into(),
                summary: "pane id".into(),
                configurable: true,
                source: WidgetSource::Builtin,
            },
            WidgetDescriptor {
                name: "cpu".into(),
                summary: "cpu usage".into(),
                configurable: true,
                source: WidgetSource::Builtin,
            },
            WidgetDescriptor {
                name: "git".into(),
                summary: "git branch".into(),
                configurable: true,
                source: WidgetSource::Builtin,
            },
        ];
        let out = widget_placements(&cfg, &descriptors, &["weather".to_string()]);

        let by = |n: &str| out.iter().find(|p| p.name == n).unwrap().clone();
        assert_eq!(by("pane_id").placement, Some((Region::Left, 0)));
        assert_eq!(by("cpu").placement, Some((Region::Right, 0)));
        assert_eq!(by("git").placement, None);
        assert_eq!(by("weather").placement, Some((Region::Right, 1)));
        assert_eq!(by("weather").source, WidgetSource::Plugin);
    }

    #[test]
    fn placements_include_instances_with_their_kind() {
        let mut cfg = Config {
            layout: Layout {
                left: vec![],
                center: vec![],
                right: vec!["clock_utc".into()],
            },
            ..Default::default()
        };
        cfg.instances.insert(
            "clock_utc".into(),
            toml::from_str::<toml::Value>("kind = \"datetime\"\ntimezone = \"UTC\"").unwrap(),
        );
        let out = widget_placements(&cfg, &[], &[]);
        let e = out.iter().find(|p| p.name == "clock_utc").unwrap();
        assert_eq!(
            e.source,
            WidgetSource::Instance {
                kind: WidgetKind::DateTime
            }
        );
        assert_eq!(e.placement, Some((Region::Right, 0)));
    }

    #[test]
    fn placements_from_a_real_registry_label_instances_correctly() {
        // The exact composition every real caller uses (see
        // `crates/rustline/src/widget_cmd.rs`'s `known_names`/`list`):
        // `widget_placements(cfg, Registry::with_builtins(cfg).descriptors(),
        // &plugins)`. Regression test for a bug where
        // `Registry::with_builtins`'s instance-registration pass reused
        // `builtin_descriptor`, hardcoding `WidgetSource::Builtin` onto every
        // instance's descriptor — so `widget_placements`'s name-based dedup
        // (first source wins) locked each instance in as `Builtin` before the
        // dedicated `cfg.instances` pass further down ever got a chance to
        // relabel it.
        use crate::widget::Registry;

        let mut cfg = Config::default();
        cfg.instances.insert(
            "clock_utc".into(),
            toml::from_str::<toml::Value>("kind = \"datetime\"\ntimezone = \"UTC\"").unwrap(),
        );
        let registry = Registry::with_builtins(&cfg);
        let out = widget_placements(&cfg, registry.descriptors(), &[]);
        let e = out.iter().find(|p| p.name == "clock_utc").unwrap();
        assert_eq!(
            e.source,
            WidgetSource::Instance {
                kind: WidgetKind::DateTime
            }
        );
    }

    #[test]
    fn placements_skip_an_instance_that_collides_with_a_builtin() {
        // Built-in always wins (the W46 precedence guard); an [instances.cpu]
        // entry must never be offered as its own selectable widget.
        let mut cfg = Config::default();
        cfg.instances.insert(
            "cpu".into(),
            toml::from_str::<toml::Value>("kind = \"datetime\"").unwrap(),
        );
        let descriptors = vec![WidgetDescriptor {
            name: "cpu".into(),
            summary: "cpu usage".into(),
            configurable: true,
            source: WidgetSource::Builtin,
        }];
        let out = widget_placements(&cfg, &descriptors, &[]);
        let cpus: Vec<_> = out.iter().filter(|p| p.name == "cpu").collect();
        assert_eq!(cpus.len(), 1, "exactly one 'cpu' entry");
        assert_eq!(cpus[0].source, WidgetSource::Builtin);
    }

    #[test]
    fn placements_are_deduped_and_sorted_builtins_then_instances_then_plugins() {
        // Routed through a real `Registry::with_builtins(&cfg)`, the same as
        // `placements_from_a_real_registry_label_instances_correctly` above —
        // so this also pins the *second* defect: an instance's position in
        // the output must not depend on where in `cfg.instances`'
        // (unspecified) `HashMap` iteration order it landed inside
        // `registry.descriptors()`.
        use crate::widget::Registry;

        let mut cfg = Config::default();
        // Insertion order is deliberately the reverse of sorted order, so a
        // passing assertion can't be accidentally riding iteration order
        // instead of an actual sort.
        cfg.instances.insert(
            "zclock".into(),
            toml::from_str::<toml::Value>("kind = \"datetime\"").unwrap(),
        );
        cfg.instances.insert(
            "aclock".into(),
            toml::from_str::<toml::Value>("kind = \"datetime\"").unwrap(),
        );
        let registry = Registry::with_builtins(&cfg);
        // The same plugin stem listed twice must yield one entry; plugin
        // names are likewise out of sorted order.
        let out = widget_placements(
            &cfg,
            registry.descriptors(),
            &[
                "zplugin".to_string(),
                "aplugin".to_string(),
                "zplugin".to_string(),
            ],
        );
        let names: Vec<&str> = out.iter().map(|p| p.name.as_str()).collect();
        let expected: Vec<&str> = [
            // All sixteen built-ins, in registration order.
            "pane_id",
            "hostname",
            "windows",
            "loadavg",
            "datetime",
            "cwd",
            "lan_ip",
            "tailscale_ip",
            "battery",
            "cpu",
            "memory",
            "git",
            "disk",
            "uptime",
            "media",
            "throughput",
            // Instances, sorted by name.
            "aclock",
            "zclock",
            // Plugin stems, sorted by name (deduped).
            "aplugin",
            "zplugin",
        ]
        .to_vec();
        assert_eq!(
            names, expected,
            "built-ins (registration order), then instances (sorted), then plugins (sorted)"
        );
    }

    /// I4: a layout can name a widget none of the three catalog inputs
    /// recognize — e.g. a plugin whose `.wasm` is no longer present. That
    /// name must still show up, flagged `WidgetSource::Unknown`, with its
    /// real placement, rather than silently vanishing from the list.
    #[test]
    fn placements_surface_a_placed_but_unrecognized_widget_as_unknown() {
        let cfg = Config {
            layout: Layout {
                left: vec!["pane_id".into()],
                center: vec![],
                right: vec!["cwd".into(), "ghostwidget".into()],
            },
            ..Default::default()
        };
        let descriptors = vec![
            WidgetDescriptor {
                name: "pane_id".into(),
                summary: "pane id".into(),
                configurable: true,
                source: WidgetSource::Builtin,
            },
            WidgetDescriptor {
                name: "cwd".into(),
                summary: "current directory".into(),
                configurable: true,
                source: WidgetSource::Builtin,
            },
        ];
        let out = widget_placements(&cfg, &descriptors, &[]);
        let ghost = out
            .iter()
            .find(|p| p.name == "ghostwidget")
            .expect("placed-but-unrecognized name is still offered");
        assert_eq!(ghost.source, WidgetSource::Unknown);
        assert_eq!(ghost.placement, Some((Region::Right, 1)));
    }

    /// The unknown-scan must never re-classify (or duplicate) a name the
    /// earlier catalog passes already recognized — a `cfg.layout` scan alone
    /// can't tell a built-in/instance/plugin apart from an unrecognized name,
    /// so it must defer to `classify`'s existing dedup rather than blindly
    /// re-adding every placed name as `Unknown`.
    #[test]
    fn placements_unknown_scan_does_not_reclassify_an_already_known_name() {
        let cfg = Config {
            layout: Layout {
                left: vec!["pane_id".into()],
                center: vec![],
                right: vec!["weather".into()],
            },
            ..Default::default()
        };
        let descriptors = vec![WidgetDescriptor {
            name: "pane_id".into(),
            summary: "pane id".into(),
            configurable: true,
            source: WidgetSource::Builtin,
        }];
        let out = widget_placements(&cfg, &descriptors, &["weather".to_string()]);
        let matches: Vec<_> = out.iter().filter(|p| p.name == "pane_id").collect();
        assert_eq!(matches.len(), 1, "no duplicate entry for pane_id");
        assert_eq!(matches[0].source, WidgetSource::Builtin);
        let weather = out.iter().find(|p| p.name == "weather").unwrap();
        assert_eq!(
            weather.source,
            WidgetSource::Plugin,
            "still a plugin, not Unknown"
        );
        assert!(out.iter().all(|p| p.source != WidgetSource::Unknown));
    }

    /// Ordering: the unknown group sorts after built-ins/instances/plugins,
    /// by name.
    #[test]
    fn placements_unknown_group_is_sorted_and_ordered_last() {
        let cfg = Config {
            layout: Layout {
                left: vec![],
                center: vec![],
                right: vec!["zghost".into(), "cwd".into(), "aghost".into()],
            },
            ..Default::default()
        };
        let descriptors = vec![WidgetDescriptor {
            name: "cwd".into(),
            summary: "current directory".into(),
            configurable: true,
            source: WidgetSource::Builtin,
        }];
        let out = widget_placements(&cfg, &descriptors, &[]);
        let names: Vec<&str> = out.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["cwd", "aghost", "zghost"]);
    }

    #[test]
    fn plugin_index_url_defaults_to_none_and_round_trips() {
        let cfg: Config = toml::from_str("").expect("empty config parses");
        assert_eq!(cfg.plugin_index_url, None);

        let cfg: Config = toml::from_str(r#"plugin_index_url = "https://example.test/i.json""#)
            .expect("config with an index url parses");
        assert_eq!(
            cfg.plugin_index_url.as_deref(),
            Some("https://example.test/i.json")
        );
    }

    #[test]
    fn a_garbage_plugin_index_url_still_loads_the_config() {
        // Invariant #3: Config::load is total. A bad URL is a failed fetch at
        // search time, never a config-load failure that breaks the bar.
        let cfg: Config = toml::from_str(r#"plugin_index_url = "not a url at all""#)
            .expect("any string value must parse");
        assert!(cfg.plugin_index_url.is_some());
    }
}
