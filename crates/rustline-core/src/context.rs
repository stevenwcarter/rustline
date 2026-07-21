use std::net::Ipv4Addr;

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

/// Metadata about a single tmux window, used to render per-window segments
/// (e.g. the window list).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowCtx {
    pub index: String,
    pub name: String,
    pub flags: String,
    pub is_current: bool,
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

/// Everything the renderer needs to know about the current tmux session,
/// pane, and host in order to produce a status line.
///
/// No `PartialEq`/`Eq` derive: `DateTime<Local>` and the `f64` load-average
/// array make a blanket equality check awkward and rarely meaningful, so
/// callers compare the specific fields they care about instead.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Context {
    pub session_name: String,
    pub window_index: String,
    pub pane_index: String,
    pub pane_current_path: String,
    pub home: String,
    pub hostname: String,
    pub loadavg: Option<[f64; 3]>,
    pub now: DateTime<Local>,
    pub window: Option<WindowCtx>,
    /// Non-loopback IPv4 interfaces read once at build time; the IP widgets
    /// select from this rather than touching the OS mid-render.
    pub interfaces: Vec<NetIface>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};

    fn sample() -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/home/steve/src/rustline".into(),
            home: "/home/steve".into(),
            hostname: "scadrial".into(),
            loadavg: Some([0.42, 0.31, 0.29]),
            now: Local
                .with_ymd_and_hms(2026, 7, 20, 17, 49, 0)
                .single()
                .unwrap(),
            window: None,
            interfaces: vec![NetIface {
                name: "eth0".into(),
                ipv4: "192.168.1.20".parse().unwrap(),
            }],
        }
    }

    #[test]
    fn context_serde_round_trip() {
        let ctx = sample();
        let json = serde_json::to_string(&ctx).unwrap();
        let back: Context = serde_json::from_str(&json).unwrap();
        assert_eq!(back.session_name, ctx.session_name);
        assert_eq!(back.loadavg, ctx.loadavg);
        assert_eq!(back.now, ctx.now);
    }

    #[test]
    fn context_interfaces_survive_serde() {
        let ctx = sample();
        let json = serde_json::to_string(&ctx).unwrap();
        let back: Context = serde_json::from_str(&json).unwrap();
        assert_eq!(back.interfaces, ctx.interfaces);
        assert_eq!(back.interfaces[0].name, "eth0");
        assert_eq!(
            back.interfaces[0].ipv4,
            "192.168.1.20".parse::<std::net::Ipv4Addr>().unwrap()
        );
    }
}
