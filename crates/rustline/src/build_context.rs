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
pub(crate) fn read_loadavg() -> Option<[f64; 3]> {
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
pub(crate) fn read_interfaces() -> Vec<NetIface> {
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
///
/// `layout` is the region's widget-name list; the expensive cpu/memory reads
/// (`read_cpu` sleeps ~120ms on Linux; `read_memory` on macOS spawns `vm_stat`)
/// are taken ONLY when that region actually renders them — the same
/// "pay only for what the region references" gating `register_plugins` uses.
pub fn build_region_context(args: &RegionArgs, layout: &[String]) -> Context {
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
        cpu: if layout.iter().any(|w| w == "cpu") {
            crate::cpu::read_cpu()
        } else {
            None
        },
        memory: if layout.iter().any(|w| w == "memory") {
            crate::memory::read_memory()
        } else {
            None
        },
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        toggled: crate::toggles::read_toggles(),
    }
}

/// Build the [`Context`] for rendering a single window segment. Reuses
/// [`build_region_context`] for the host/pane-agnostic fields (there is no
/// pane in play for a window segment) and layers on the window-specific
/// fields from `args`.
pub fn build_window_context(args: &WindowArgs) -> Context {
    // Windows render only the window pill (builtins, never cpu/memory), so pass
    // an empty layout: no cpu/memory sampling, no per-window `read_cpu` sleep.
    let mut ctx = build_region_context(&RegionArgs::default(), &[]);
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
        let ctx = build_region_context(&RegionArgs::default(), &[]);
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
        let ctx = build_region_context(&RegionArgs::default(), &[]);
        assert!(
            ctx.interfaces
                .iter()
                .all(|i| i.ipv4 != std::net::Ipv4Addr::LOCALHOST)
        );
    }

    #[test]
    fn build_region_context_reads_toggles_from_state_file() {
        let tmp = tempfile::tempdir().unwrap();
        // Write the state file via the absolute tempdir path FIRST, before the
        // env var is ever set: neither `unwrap()` below can panic while
        // `XDG_DATA_HOME` is overridden, so a setup failure can't leak the
        // override into other tests.
        std::fs::create_dir_all(tmp.path().join("rustline")).unwrap();
        std::fs::write(tmp.path().join("rustline/toggles"), "cpu\nmemory\n").unwrap();
        // SAFETY: `build_region_context` now unconditionally calls
        // `read_toggles()` -> `rustline_wasm::data_root()`, which *reads*
        // `XDG_DATA_HOME` -- so the sibling tests in this module that also call
        // `build_region_context` (`home_from_env_used_when_present`,
        // `read_interfaces_excludes_loopback_and_never_panics`,
        // `cpu_memory_sampled_only_when_region_names_them`) transitively read
        // this var too, and cargo's test harness may run them concurrently
        // with the `set_var`/`remove_var` below. That's sound here because
        // none of those siblings assert on `ctx.toggled` or anything else
        // derived from `data_root()`, so a torn read during their call can't
        // change their outcome; this test is the only one whose assertion
        // depends on the value, and the mutation window is kept minimal
        // (just around the single `build_region_context` call below).
        unsafe {
            std::env::set_var("XDG_DATA_HOME", tmp.path());
        }
        let ctx = build_region_context(&RegionArgs::default(), &[]);
        // SAFETY: matches the set above; restores the process env for other tests.
        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        assert!(ctx.toggled.contains("cpu") && ctx.toggled.contains("memory"));
    }

    #[test]
    fn cpu_memory_sampled_only_when_region_names_them() {
        // Empty layout: neither expensive read runs, so both stay None — this is
        // what spares `render left` / `render window` the read_cpu sleep.
        let ctx = build_region_context(&RegionArgs::default(), &[]);
        assert!(ctx.cpu.is_none() && ctx.memory.is_none());
        // The window path uses an empty layout too.
        let wctx = build_window_context(&WindowArgs {
            current: false,
            index: String::new(),
            name: String::new(),
            flags: String::new(),
            preview: false,
        });
        assert!(wctx.cpu.is_none() && wctx.memory.is_none());
    }
}
