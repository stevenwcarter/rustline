//! rustline-abi: the serde-serializable types that cross the WASM plugin
//! boundary. No I/O, no chrono — the wire-format ABI.
//!
//! Beyond the original output types (`Segment`/`Style`/`Color`/`ThemeColors`),
//! this crate now also holds every other chrono-free type shared between the
//! host and a guest: the snapshot types moved here from
//! `rustline-core::context` (`NetIface`, `Battery`/`BatteryState`,
//! `CpuUsage`, `MemInfo`), `GitInfo`/`DiskInfo`, and the typed guest-input
//! wire types (`WireContext`, `WireWindowCtx`, `GuestRender`) a plugin
//! deserializes instead of hand-walking a `serde_json::Value`.
use std::collections::BTreeSet;
use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

/// The wire-format ABI version. Bump this only when `WireContext`/
/// `GuestRender`'s shape changes in a way that breaks an existing guest
/// (removing/renaming a field, changing a type) — a purely additive change (a
/// new `#[serde(default)]` field) doesn't need a bump, since serde already
/// tolerates that on both sides.
///
/// A guest may export its own `abi_version() -> String` (returning this same
/// number, stringified); the host compares it against this constant during
/// plugin registration (`rustline_wasm::abi_decision`) and warns instead of
/// silently misbehaving when they disagree. A guest with no such export
/// registers anyway (the legacy path) so existing plugins keep working.
pub const ABI_VERSION: u32 = 1;

/// A terminal color, expressible in the ways tmux understands colors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Color {
    Named(String),
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    /// Render this color as a tmux-style color spec (e.g. `cyan`,
    /// `colour236`, `#1a2b3c`).
    pub fn to_tmux(&self) -> String {
        match self {
            Color::Named(n) => n.clone(),
            Color::Indexed(i) => format!("colour{i}"),
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        }
    }
}

/// The theme-derived colors a widget or WASM plugin may use to style output
/// consistently with the active theme: the default text `fg`, the bar
/// background `bar_bg`, and the four semantic colors. Carried on `Context`
/// (serde `default`) so it crosses the WASM boundary. Defaults match
/// `rustline_core::Theme::default()`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeColors {
    pub fg: Color,
    pub bar_bg: Color,
    pub success: Color,
    pub info: Color,
    pub warning: Color,
    pub error: Color,
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self {
            fg: Color::Indexed(255),
            bar_bg: Color::Indexed(234),
            success: Color::Indexed(35),
            info: Color::Indexed(39),
            warning: Color::Indexed(214),
            error: Color::Indexed(196),
        }
    }
}

/// Visual styling for a [`Segment`]: foreground/background color and
/// boldness.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    #[serde(default)]
    pub bold: bool,
}

/// A single piece of rendered status line text with its style.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub text: String,
    pub style: Style,
}

impl Segment {
    /// Create a segment with the default (unstyled) style.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::default(),
        }
    }

    /// Create a segment with an explicit style.
    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }
}

/// One non-loopback IPv4 network interface, captured at `Context`-build time.
///
/// The widgets (`lan_ip`, `tailscale_ip`) select from this list rather than
/// reading the OS, keeping invariant #1 (Context is the sole render input).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetIface {
    pub name: String,
    pub ipv4: Ipv4Addr,
}

/// Charge state of the host battery. A small typed domain — not a stringly
/// value — mapped from the Linux sysfs `status` file and macOS `pmset` state
/// words at Context-build time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatteryState {
    Charging,
    Discharging,
    Full,
    Unknown,
}

/// A battery snapshot captured at Context-build time. `percent` is `0..=100`.
///
/// `Context::battery` is `None` on hosts without a battery, on unsupported
/// platforms, or when the read failed — never a fabricated `0%` (invariant #6),
/// mirroring `loadavg`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Battery {
    pub percent: u8,
    pub state: BatteryState,
}

/// A memory snapshot captured at Context-build time. All values are bytes;
/// `used_bytes = total_bytes - available_bytes` (saturating). `Context::memory`
/// is `None` on unsupported platforms or when the read failed — never a
/// fabricated `0` (invariant #6), mirroring `loadavg`/`battery`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
}

/// A CPU-utilization snapshot: the busy fraction measured over a short sampling
/// window at Context-build time, as a percentage clamped to `0.0..=100.0`.
/// `Context::cpu` is `None` on unsupported platforms or when the read failed.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CpuUsage {
    pub percent: f32,
}

/// A git repository status snapshot for the pane's current working directory,
/// captured at Context-build time by shelling out to `git status
/// --porcelain=v2 --branch`. `branch` is the current branch name, or the
/// 7-character short SHA when `HEAD` is detached. `Context::git` is `None`
/// when `git` is missing, the pane isn't inside a repository, or the read
/// failed — never a fabricated "clean" reading (invariant #6), mirroring
/// `loadavg`/`battery`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitInfo {
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub unstaged: u32,
}

/// A filesystem-usage snapshot for a configured mount, captured at
/// Context-build time via `statvfs(2)`. All values are bytes;
/// `used_bytes = total_bytes - free_bytes` (saturating, so it accounts for
/// the filesystem's reserved-for-root blocks that `available_bytes` excludes).
/// `Context::disk` is `None` when the mount can't be `statvfs`'d — never a
/// fabricated `0` (invariant #6), mirroring `MemInfo`/`GitInfo`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
}

/// A now-playing media snapshot, captured at Context-build time by shelling
/// out to `playerctl metadata`. `Context::media` is `None` when `playerctl`
/// is missing, no player is running, or the read failed — never a fabricated
/// "not playing" reading (invariant #6), mirroring `GitInfo`/`DiskInfo`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaInfo {
    pub artist: String,
    pub title: String,
    pub status: String,
}

/// The guest-side wire mirror of `rustline_core::WindowCtx`. A WASM guest
/// deserializes this typed struct rather than hand-walking the JSON.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireWindowCtx {
    pub index: String,
    pub name: String,
    pub flags: String,
    pub is_current: bool,
}

/// The guest-side, typed mirror of `rustline_core::Context` as it appears on
/// the WASM wire. Field-for-field identical to `Context` except `now` is a
/// plain RFC3339 `String` (the host's `DateTime<Local>` serializes to one) and
/// `window` nests [`WireWindowCtx`], so this crate stays chrono-free. Guests
/// deserialize this instead of walking an untyped `serde_json::Value`.
///
/// The field names and serde behavior must match `Context`'s exactly: the host
/// serializes `Context` verbatim and this must parse those bytes unchanged (the
/// round-trip seam test in `rustline-wasm` pins the two together). No
/// `deny_unknown_fields` — host/guest version skew must stay total, matching
/// `Context`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WireContext {
    pub session_name: String,
    pub window_index: String,
    pub pane_index: String,
    pub pane_current_path: String,
    pub home: String,
    pub hostname: String,
    pub loadavg: Option<[f64; 3]>,
    /// The host's `DateTime<Local>` serialized as an RFC3339 string; a guest
    /// that needs the instant parses it (`DateTime::parse_from_rfc3339`).
    pub now: String,
    pub window: Option<WireWindowCtx>,
    pub interfaces: Vec<NetIface>,
    pub battery: Option<Battery>,
    pub cpu: Option<CpuUsage>,
    pub memory: Option<MemInfo>,
    pub git: Option<GitInfo>,
    pub disk: Option<DiskInfo>,
    pub os: String,
    pub arch: String,
    #[serde(default)]
    pub toggled: BTreeSet<String>,
    #[serde(default)]
    pub colors: ThemeColors,
}

/// The whole input shape a guest's `render` export receives: the typed
/// [`WireContext`] plus the opaque plugin `config` table (kept as
/// `serde_json::Value` so a plugin reads its own keys). Mirrors the host's
/// `rustline_wasm::abi::RenderInput`.
#[derive(Clone, Debug, Deserialize)]
pub struct GuestRender {
    pub context: WireContext,
    pub config: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_state_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&BatteryState::Discharging).unwrap(),
            "\"discharging\""
        );
        assert_eq!(
            serde_json::to_string(&BatteryState::Full).unwrap(),
            "\"full\""
        );
    }

    #[test]
    fn color_to_tmux_named_indexed_rgb() {
        assert_eq!(Color::Named("cyan".into()).to_tmux(), "cyan");
        assert_eq!(Color::Indexed(236).to_tmux(), "colour236");
        assert_eq!(Color::Rgb(0x1a, 0x2b, 0x3c).to_tmux(), "#1a2b3c");
    }

    #[test]
    fn segment_new_defaults_style() {
        let s = Segment::new("hi");
        assert_eq!(s.text, "hi");
        assert_eq!(s.style, Style::default());
    }

    #[test]
    fn theme_colors_default_and_serde_round_trip() {
        let d = ThemeColors::default();
        assert_eq!(d.fg, Color::Indexed(255));
        assert_eq!(d.bar_bg, Color::Indexed(234));
        assert_eq!(d.success, Color::Indexed(35));
        assert_eq!(d.info, Color::Indexed(39));
        assert_eq!(d.warning, Color::Indexed(214));
        assert_eq!(d.error, Color::Indexed(196));
        let json = serde_json::to_string(&d).unwrap();
        let back: ThemeColors = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn wire_context_deserializes_representative_literal() {
        let json = r#"{
            "session_name":"main","window_index":"2","pane_index":"1",
            "pane_current_path":"/home/steve/src","home":"/home/steve",
            "hostname":"scadrial","loadavg":[0.42,0.31,0.29],
            "now":"2026-07-20T17:49:00-04:00",
            "window":{"index":"2","name":"editor","flags":"*","is_current":true},
            "interfaces":[{"name":"eth0","ipv4":"192.168.1.20"}],
            "battery":{"percent":73,"state":"discharging"},
            "cpu":{"percent":12.5},
            "memory":{"total_bytes":100,"used_bytes":40,"available_bytes":60},
            "os":"linux","arch":"x86_64",
            "toggled":["weather"],
            "colors":{"fg":{"Indexed":255},"bar_bg":{"Indexed":234},
                "success":{"Indexed":35},"info":{"Indexed":39},
                "warning":{"Indexed":214},"error":{"Rgb":[1,2,3]}}
        }"#;
        let wire: WireContext = serde_json::from_str(json).unwrap();
        assert_eq!(wire.session_name, "main");
        assert_eq!(wire.now, "2026-07-20T17:49:00-04:00");
        let win = wire.window.as_ref().unwrap();
        assert_eq!(win.name, "editor");
        assert!(win.is_current);
        assert_eq!(wire.interfaces[0].name, "eth0");
        assert_eq!(wire.battery.unwrap().percent, 73);
        assert_eq!(wire.battery.unwrap().state, BatteryState::Discharging);
        assert!(wire.toggled.contains("weather"));
        assert_eq!(wire.colors.error, Color::Rgb(1, 2, 3));
    }

    #[test]
    fn wire_context_defaults_toggled_and_colors_when_absent() {
        // A minimal `WireContext` JSON omitting `toggled` and `colors` must
        // still deserialize (host/guest version skew stays total, matching
        // `Context`'s `#[serde(default)]` on the same fields).
        let json = r#"{
            "session_name":"0","window_index":"0","pane_index":"0",
            "pane_current_path":"/home/steve","home":"/home/steve",
            "hostname":"scadrial","loadavg":null,
            "now":"2026-07-20T17:49:00-04:00",
            "window":null,"interfaces":[],
            "battery":null,"cpu":null,"memory":null,
            "os":"linux","arch":"x86_64"
        }"#;
        let wire: WireContext = serde_json::from_str(json).unwrap();
        assert!(wire.toggled.is_empty());
        assert_eq!(wire.colors, ThemeColors::default());
    }

    #[test]
    fn guest_render_parses_context_and_opaque_config() {
        let json = r#"{
            "context":{
                "session_name":"0","window_index":"0","pane_index":"0",
                "pane_current_path":"/home/steve","home":"/home/steve",
                "hostname":"scadrial","loadavg":null,
                "now":"2026-07-20T17:49:00-04:00",
                "window":null,"interfaces":[],
                "battery":null,"cpu":null,"memory":null,
                "os":"linux","arch":"x86_64","toggled":["weather"]
            },
            "config":{"zip":"48183","refresh_secs":1800}
        }"#;
        let input: GuestRender = serde_json::from_str(json).unwrap();
        assert!(input.context.toggled.contains("weather"));
        assert_eq!(input.config["zip"], "48183");
        assert_eq!(input.config["refresh_secs"], 1800);
    }
}
