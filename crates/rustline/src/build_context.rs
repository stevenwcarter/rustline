//! Build a [`Context`] from CLI arguments plus live host state (env vars,
//! load average, hostname, wall clock).

use std::env;

use crate::cli::{RegionArgs, WindowArgs};
use rustline_core::{Context, NetIface, WindowCtx};

/// Read the 1/5/15-minute load average via `getloadavg(3)`.
///
/// Returns `None` if the platform call doesn't report all three samples
/// (its documented failure mode), so a widget can fall back gracefully
/// instead of showing bogus zeros.
fn read_loadavg() -> Option<[f64; 3]> {
    let mut out = [0f64; 3];
    // SAFETY: `out` is a valid, properly aligned buffer for 3 `f64`s, and
    // `getloadavg` is documented to write at most `out.len()` samples into it.
    let n = unsafe { libc::getloadavg(out.as_mut_ptr(), 3) };
    if n == 3 { Some(out) } else { None }
}

/// The local machine's hostname, lossily converted to UTF-8 (hostnames are
/// display-only here, never round-tripped back to the OS).
fn hostname() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

/// Enumerate the host's non-loopback IPv4 network interfaces.
///
/// A failed read yields an empty `Vec` (the IP widgets then render nothing /
/// their `down_format`), never a fabricated address — same spirit as
/// `read_loadavg` returning `None`.
fn read_interfaces() -> Vec<NetIface> {
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    ifaces
        .into_iter()
        .filter(|iface| !iface.is_loopback())
        .filter_map(|iface| match iface.addr {
            if_addrs::IfAddr::V4(v4) => Some(NetIface {
                name: iface.name,
                ipv4: v4.ip,
            }),
            if_addrs::IfAddr::V6(_) => None,
        })
        .collect()
}

/// Build the [`Context`] for rendering a left/right region from the tmux
/// format-variable values passed on the command line, plus live host state.
pub fn build_region_context(args: &RegionArgs) -> Context {
    Context {
        session_name: args.session.clone().unwrap_or_default(),
        window_index: args.window.clone().unwrap_or_default(),
        pane_index: args.pane.clone().unwrap_or_default(),
        pane_current_path: args.pane_path.clone().unwrap_or_default(),
        home: env::var("HOME").unwrap_or_default(),
        hostname: hostname(),
        loadavg: read_loadavg(),
        now: chrono::Local::now(),
        window: None,
        interfaces: read_interfaces(),
        battery: crate::battery::read_battery(),
        cpu: None,
        memory: None,
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    }
}

/// Build the [`Context`] for rendering a single window segment. Reuses
/// [`build_region_context`] for the host/pane-agnostic fields (there is no
/// pane in play for a window segment) and layers on the window-specific
/// fields from `args`.
pub fn build_window_context(args: &WindowArgs) -> Context {
    let mut ctx = build_region_context(&RegionArgs::default());
    ctx.window = Some(WindowCtx {
        index: args.index.clone(),
        name: args.name.clone(),
        flags: args.flags.clone(),
        is_current: args.current,
    });
    ctx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_from_env_used_when_present() {
        // build_context reads $HOME; assert the field is populated non-empty
        let ctx = build_region_context(&RegionArgs::default());
        assert!(!ctx.home.is_empty() || std::env::var("HOME").is_err());
    }

    #[test]
    fn read_interfaces_excludes_loopback_and_never_panics() {
        let ifaces = read_interfaces();
        // Loopback is filtered out; whatever the host has, 127.0.0.1 must not appear.
        assert!(
            ifaces
                .iter()
                .all(|i| i.ipv4 != std::net::Ipv4Addr::LOCALHOST),
            "loopback IPv4 must be filtered: {ifaces:?}"
        );
        // And build_region_context wires it in (field is populated by the same read).
        let ctx = build_region_context(&RegionArgs::default());
        assert!(
            ctx.interfaces
                .iter()
                .all(|i| i.ipv4 != std::net::Ipv4Addr::LOCALHOST)
        );
    }
}
