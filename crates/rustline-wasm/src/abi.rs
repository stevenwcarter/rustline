//! The host↔guest wire types. Host functions return these as JSON strings;
//! `render` receives `RenderInput` and returns `Vec<Segment>` as JSON.

use rustline_core::{Context, Segment};
use serde::{Deserialize, Serialize};

/// Result of a plain (uncached) HTTP GET. `ok` means the transport completed
/// for *any* status, including non-2xx (not that the response was 2xx); the
/// HTTP status is in `status` and `error` carries a transport failure.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct HttpResult {
    pub ok: bool,
    pub status: u16,
    pub body: String,
    pub error: String,
}

/// Result of a host state/file read. `ok=true` with `exists=false` is a
/// successful read of a missing file (not an error); `error` carries the
/// message only when `ok` is false.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ReadResult {
    pub ok: bool,
    pub exists: bool,
    pub contents: String,
    pub error: String,
}

/// Result of a host state/file write. `ok` is true on success; otherwise
/// `error` carries the failure message.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct WriteResult {
    pub ok: bool,
    pub error: String,
}

/// Result of a TTL-cached HTTP GET. `ok` means "a usable body is present"
/// (fresh OR stale), not "transport succeeded"; `stale` distinguishes them.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CachedHttpResult {
    pub ok: bool,
    pub status: u16,
    pub body: String,
    pub error: String,
    pub stale: bool,
    pub age_secs: i64,
}

/// What the host passes to a plugin's `render` export.
#[derive(Serialize)]
pub struct RenderInput<'a> {
    pub context: &'a Context,
    pub config: &'a serde_json::Value,
}

/// Parse a plugin's `render` output into segments; any malformed output
/// degrades to an empty vec (never breaks the bar).
pub fn parse_render_output(s: &str) -> Vec<Segment> {
    serde_json::from_str(s).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use chrono::{DateTime, Local, TimeZone};
    use rustline_core::{
        Battery, BatteryState, Color, Context, CpuUsage, DiskInfo, GitInfo, MemInfo, NetIface,
        ThemeColors, WindowCtx,
    };

    use super::*;

    /// A representative `Context` exercising every wire field (non-epoch `now`,
    /// a battery/cpu/memory reading, a non-loopback interface, a toggled entry,
    /// custom colors, and a current window).
    fn sample_context() -> Context {
        Context {
            session_name: "main".into(),
            window_index: "2".into(),
            pane_index: "1".into(),
            pane_current_path: "/home/steve/src/rustline".into(),
            home: "/home/steve".into(),
            hostname: "scadrial".into(),
            loadavg: Some([0.42, 0.31, 0.29]),
            now: Local
                .with_ymd_and_hms(2026, 7, 20, 17, 49, 0)
                .single()
                .unwrap(),
            window: Some(WindowCtx {
                index: "2".into(),
                name: "editor".into(),
                flags: "*".into(),
                is_current: true,
            }),
            interfaces: vec![NetIface {
                name: "eth0".into(),
                ipv4: "192.168.1.20".parse().unwrap(),
            }],
            battery: Some(Battery {
                percent: 73,
                state: BatteryState::Discharging,
            }),
            cpu: Some(CpuUsage { percent: 12.5 }),
            memory: Some(MemInfo {
                total_bytes: 16 * 1024 * 1024 * 1024,
                used_bytes: 6 * 1024 * 1024 * 1024,
                available_bytes: 10 * 1024 * 1024 * 1024,
            }),
            git: Some(GitInfo {
                branch: "main".into(),
                ahead: 2,
                behind: 1,
                staged: 1,
                unstaged: 3,
            }),
            disk: Some(DiskInfo {
                total_bytes: 512 * 1024 * 1024 * 1024,
                used_bytes: 200 * 1024 * 1024 * 1024,
                available_bytes: 300 * 1024 * 1024 * 1024,
            }),
            os: "linux".into(),
            arch: "x86_64".into(),
            uptime: Some(86_400 * 3 + 3600 * 4), // 3d 4h
            toggled: BTreeSet::from(["weather".to_string()]),
            colors: ThemeColors {
                error: Color::Rgb(1, 2, 3),
                ..ThemeColors::default()
            },
        }
    }

    /// The load-bearing seam test: the host serializes `&Context` verbatim (see
    /// `RenderInput`), so the guest-side `WireContext` must parse those exact
    /// bytes. Pins the two shapes together — if `Context` gains/renames a field
    /// without a matching `WireContext` change, this fails.
    #[test]
    fn wire_context_round_trips_host_context_bytes() {
        let ctx = sample_context();
        let json = serde_json::to_string(&ctx).unwrap();
        let wire: rustline_abi::WireContext = serde_json::from_str(&json).unwrap();

        // `now` crosses as an RFC3339 string that parses back to the instant.
        let parsed = DateTime::parse_from_rfc3339(&wire.now).unwrap();
        assert_eq!(parsed, ctx.now);

        assert_eq!(wire.session_name, ctx.session_name);
        assert_eq!(wire.window_index, ctx.window_index);
        assert_eq!(wire.pane_index, ctx.pane_index);
        assert_eq!(wire.pane_current_path, ctx.pane_current_path);
        assert_eq!(wire.home, ctx.home);
        assert_eq!(wire.hostname, ctx.hostname);
        assert_eq!(wire.loadavg, ctx.loadavg);
        assert_eq!(wire.interfaces, ctx.interfaces);
        assert_eq!(wire.battery, ctx.battery);
        assert_eq!(wire.cpu, ctx.cpu);
        assert_eq!(wire.memory, ctx.memory);
        assert_eq!(wire.git, ctx.git);
        assert_eq!(wire.disk, ctx.disk);
        assert_eq!(wire.os, ctx.os);
        assert_eq!(wire.arch, ctx.arch);
        assert_eq!(wire.toggled, ctx.toggled);
        assert_eq!(wire.colors, ctx.colors);
        assert_eq!(
            wire.window.map(|w| w.is_current),
            ctx.window.map(|w| w.is_current)
        );
    }

    /// The full guest input shape (`GuestRender`) parses the host's
    /// `RenderInput` JSON: a typed `context` plus the opaque `config` table.
    #[test]
    fn guest_render_parses_full_input() {
        let ctx = sample_context();
        let config = serde_json::json!({ "zip": "48183" });
        let input = RenderInput {
            context: &ctx,
            config: &config,
        };
        let json = serde_json::to_string(&input).unwrap();
        let parsed: rustline_abi::GuestRender = serde_json::from_str(&json).unwrap();
        assert!(parsed.context.toggled.contains("weather"));
        assert_eq!(parsed.config["zip"], "48183");
    }
}
