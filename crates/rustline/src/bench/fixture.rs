//! The fabricated `Context` used by the pure passes. Because `Widget::render`
//! reads only from `Context` (invariant #1), a hand-built Context with every
//! `Option` field populated bypasses ALL OS reads — including `read_cpu`'s
//! ~120 ms sample. This IS the "mock": a future slow read is skipped by the
//! pure pass simply by filling its field here.

use chrono::{Local, TimeZone};
use rustline_core::{
    Battery, BatteryState, Context, CpuUsage, MemInfo, NetIface, ThemeColors, WindowCtx,
};

/// A representative, fully-populated `Context`. Every widget renders its real
/// `format` branch on it (see the completeness test) — so no widget degrades to
/// `down_format`, which would make the pure numbers meaningless.
///
/// Interfaces carry both a LAN address (`192.168.1.42` on a non-virtual NIC, so
/// `pick_lan` selects it) and a Tailscale CGNAT address (`100.101.4.7`, so
/// `pick_tailscale` selects it) — see `rustline-core/src/widgets/net.rs`.
pub fn fabricated_context() -> Context {
    Context {
        session_name: "0".into(),
        window_index: "1".into(),
        pane_index: "0".into(),
        pane_current_path: "/home/steve/src/rustline".into(),
        home: "/home/steve".into(),
        hostname: "benchbox".into(),
        loadavg: Some([0.42, 0.37, 0.30]),
        now: Local
            .with_ymd_and_hms(2026, 7, 21, 12, 0, 0)
            .single()
            .expect("fixed timestamp is valid"),
        window: Some(WindowCtx {
            index: "1".into(),
            name: "editor".into(),
            flags: "*".into(),
            is_current: true,
        }),
        interfaces: vec![
            NetIface {
                name: "eth0".into(),
                ipv4: "192.168.1.42".parse().expect("valid ipv4"),
            },
            NetIface {
                name: "tailscale0".into(),
                ipv4: "100.101.4.7".parse().expect("valid ipv4"),
            },
        ],
        battery: Some(Battery {
            percent: 76,
            state: BatteryState::Discharging,
        }),
        cpu: Some(CpuUsage { percent: 23.5 }),
        memory: Some(MemInfo {
            total_bytes: 16 * 1024 * 1024 * 1024,
            used_bytes: 6 * 1024 * 1024 * 1024,
            available_bytes: 10 * 1024 * 1024 * 1024,
        }),
        os: "linux".into(),
        arch: "x86_64".into(),
        toggled: Default::default(),
        // Theme-derived colors added when the theme feature landed on main; the
        // fixture's readings sit below every alert threshold, so no widget takes
        // its alert-badge path — default colors keep the pure pass representative.
        colors: ThemeColors::default(),
    }
}
