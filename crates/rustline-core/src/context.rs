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
    /// Battery snapshot read once at build time; `None` when absent/unsupported.
    pub battery: Option<Battery>,
    /// CPU-utilization snapshot read once at build time; `None` when
    /// absent/unsupported.
    pub cpu: Option<CpuUsage>,
    /// Memory snapshot read once at build time; `None` when absent/unsupported.
    pub memory: Option<MemInfo>,
    /// Host OS (`std::env::consts::OS`, e.g. `"linux"`, `"macos"`). Additive
    /// platform metadata for WASM guests.
    pub os: String,
    /// Host CPU arch (`std::env::consts::ARCH`, e.g. `"x86_64"`, `"aarch64"`).
    pub arch: String,
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
            os: "linux".into(),
            arch: "x86_64".into(),
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

    #[test]
    fn context_battery_os_arch_survive_serde() {
        let ctx = sample();
        let json = serde_json::to_string(&ctx).unwrap();
        let back: Context = serde_json::from_str(&json).unwrap();
        assert_eq!(back.battery, ctx.battery);
        assert_eq!(back.os, "linux");
        assert_eq!(back.arch, "x86_64");
    }

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
    fn context_cpu_memory_survive_serde() {
        let mut ctx = sample();
        ctx.cpu = Some(CpuUsage { percent: 37.5 });
        ctx.memory = Some(MemInfo {
            total_bytes: 16 * 1024 * 1024 * 1024,
            used_bytes: 6 * 1024 * 1024 * 1024,
            available_bytes: 10 * 1024 * 1024 * 1024,
        });
        let json = serde_json::to_string(&ctx).unwrap();
        let back: Context = serde_json::from_str(&json).unwrap();
        assert_eq!(back.cpu, ctx.cpu);
        assert_eq!(back.memory, ctx.memory);
    }
}
