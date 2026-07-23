//! Build a [`Context`] from CLI arguments plus live host state (env vars,
//! load average, hostname, wall clock).

use std::env;

use crate::cli::{RegionArgs, WindowArgs};
use rustline_core::{Context, NetIface, Theme, WindowCtx};

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

/// Whether `layout` (a region's widget-name list) references `name`.
///
/// The one predicate behind every "only pay for a read the region actually
/// renders" gate below (cpu/memory/git/disk/battery/uptime/media/interfaces) —
/// factored out so each gate is the same one-line check instead of its own
/// `.iter().any(...)`.
fn layout_needs(layout: &[String], name: &str) -> bool {
    layout.iter().any(|w| w == name)
}

/// Build the [`Context`] for rendering a left/right region from the tmux
/// format-variable values passed on the command line, plus live host state.
///
/// `layout` is the region's widget-name list; the expensive cpu/memory/git/
/// disk/battery/uptime/media/interfaces reads (`read_cpu` sleeps ~120ms on
/// Linux; `read_memory` on macOS spawns `vm_stat`; `read_git` shells out to
/// `git`; `read_disk` calls `statvfs(2)`; `read_battery` scans sysfs;
/// `read_uptime` reads `/proc/uptime` (Linux) or shells out to `sysctl`
/// (macOS); `read_media` shells out to `playerctl` (Linux only);
/// `read_interfaces` calls `getifaddrs(3)`) are taken ONLY when that region
/// actually renders them — the same "pay only for what the region
/// references" gating `register_plugins` uses. `disk_mount` is the configured
/// `[widgets.disk].mount` (unused unless `layout` names `disk`).
pub fn build_region_context(
    args: &RegionArgs,
    layout: &[String],
    theme: &Theme,
    disk_mount: &str,
) -> Context {
    let pane_current_path = args.pane_path.clone().unwrap_or_default();
    let git = if layout_needs(layout, "git") {
        crate::git::read_git(&pane_current_path)
    } else {
        None
    };
    let disk = if layout_needs(layout, "disk") {
        crate::disk::read_disk(disk_mount)
    } else {
        None
    };
    // Interfaces feed both IP widgets, so either one names the read.
    let interfaces = if layout_needs(layout, "lan_ip") || layout_needs(layout, "tailscale_ip") {
        read_interfaces()
    } else {
        Vec::new()
    };
    let battery = if layout_needs(layout, "battery") {
        crate::battery::read_battery()
    } else {
        None
    };
    let uptime = if layout_needs(layout, "uptime") {
        crate::uptime::read_uptime()
    } else {
        None
    };
    let media = if layout_needs(layout, "media") {
        crate::media::read_media()
    } else {
        None
    };
    Context {
        session_name: args.session.clone().unwrap_or_default(),
        window_index: args.window.clone().unwrap_or_default(),
        pane_index: args.pane.clone().unwrap_or_default(),
        pane_current_path,
        home: env::var("HOME").unwrap_or_default(),
        hostname: hostname(),
        loadavg: read_loadavg(),
        now: chrono::Local::now(),
        window: None,
        interfaces,
        battery,
        cpu: if layout_needs(layout, "cpu") {
            crate::cpu::read_cpu()
        } else {
            None
        },
        memory: if layout_needs(layout, "memory") {
            crate::memory::read_memory()
        } else {
            None
        },
        git,
        disk,
        uptime,
        media,
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        toggled: crate::toggles::read_toggles(),
        colors: theme.colors(),
    }
}

/// Build the minimal [`Context`] needed to render a single window segment.
///
/// tmux spawns `rustline render window` once PER WINDOW on every refresh, so
/// unlike [`build_region_context`] this does not route through it at all:
/// the window-pill render path (`render_window`/`render_window_pill` in
/// `rustline-core`, verified by reading `Windows::render` and both) reads
/// only `Context.window` — the pill's colors come from the `Theme` passed
/// directly to `render_window`, not from `Context.colors`. So this builder
/// skips every other read `build_region_context` performs even with an empty
/// layout: `getloadavg`, the toggles-file read, `gethostname`, `$HOME`, and
/// `now`. For a session with N windows those reads would otherwise repeat N
/// times per refresh for no benefit.
pub fn build_window_context(args: &WindowArgs) -> Context {
    Context {
        window: Some(WindowCtx {
            index: args.index.clone(),
            name: args.name.clone(),
            flags: args.flags.clone(),
            is_current: args.current,
        }),
        ..Context::default()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Guards the two tests below that mutate the process-global
    /// `XDG_DATA_HOME` env var: cargo's test harness runs tests in the same
    /// process concurrently, and both tests' assertions depend on the value
    /// `read_toggles()` sees during their own critical section, so an
    /// unguarded interleaving of one test's `set_var`/`remove_var` with the
    /// other's read would be a real race, not just a theoretical one.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn home_from_env_used_when_present() {
        // build_context reads $HOME; assert the field is populated non-empty
        let ctx = build_region_context(&RegionArgs::default(), &[], &Theme::default(), "");
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
        // And build_region_context wires it in when an IP widget is in the
        // layout (field is populated by the same read).
        let ctx = build_region_context(
            &RegionArgs::default(),
            &["lan_ip".to_string()],
            &Theme::default(),
            "",
        );
        assert!(
            ctx.interfaces
                .iter()
                .all(|i| i.ipv4 != std::net::Ipv4Addr::LOCALHOST)
        );
    }

    #[test]
    fn layout_needs_true_when_present_false_when_absent() {
        let layout = ["cpu".to_string(), "battery".to_string()];
        assert!(layout_needs(&layout, "cpu"));
        assert!(layout_needs(&layout, "battery"));
        assert!(!layout_needs(&layout, "memory"));
        assert!(!layout_needs(&[], "cpu"));
    }

    #[test]
    fn interfaces_sampled_only_when_region_names_an_ip_widget() {
        // Empty layout: getifaddrs never runs, so interfaces stays at its
        // not-found value (empty), never a stale/fabricated read.
        let ctx = build_region_context(&RegionArgs::default(), &[], &Theme::default(), "");
        assert!(ctx.interfaces.is_empty());

        // Named in the layout (either IP widget triggers the shared read):
        // the real read runs, matching a direct read_interfaces() call.
        for name in ["lan_ip", "tailscale_ip"] {
            let ctx = build_region_context(
                &RegionArgs::default(),
                &[name.to_string()],
                &Theme::default(),
                "",
            );
            assert_eq!(ctx.interfaces, read_interfaces());
        }
    }

    #[test]
    fn battery_sampled_only_when_region_names_it() {
        // Empty layout: the sysfs scan never runs, so it stays None.
        let ctx = build_region_context(&RegionArgs::default(), &[], &Theme::default(), "");
        assert!(ctx.battery.is_none());

        // Named in the layout: the real read runs, matching a direct
        // read_battery() call.
        let ctx = build_region_context(
            &RegionArgs::default(),
            &["battery".to_string()],
            &Theme::default(),
            "",
        );
        assert_eq!(ctx.battery, crate::battery::read_battery());
    }

    #[test]
    fn uptime_sampled_only_when_region_names_it() {
        // Empty layout: the read never runs, so it stays None — same
        // "pay only for what the region references" gating as battery.
        let ctx = build_region_context(&RegionArgs::default(), &[], &Theme::default(), "");
        assert!(ctx.uptime.is_none());

        // Named in the layout: the real read runs, matching a direct
        // read_uptime() call.
        let ctx = build_region_context(
            &RegionArgs::default(),
            &["uptime".to_string()],
            &Theme::default(),
            "",
        );
        assert_eq!(ctx.uptime, crate::uptime::read_uptime());
    }

    #[test]
    fn media_read_only_when_region_names_it() {
        // Empty layout: the playerctl shell-out never runs, so it stays None —
        // same "pay only for what the region references" gating as
        // cpu/memory/git/disk/battery/uptime.
        let ctx = build_region_context(&RegionArgs::default(), &[], &Theme::default(), "");
        assert!(ctx.media.is_none());

        // Named in the layout: the real read runs, matching a direct
        // read_media() call.
        let ctx = build_region_context(
            &RegionArgs::default(),
            &["media".to_string()],
            &Theme::default(),
            "",
        );
        assert_eq!(ctx.media, crate::media::read_media());
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
        // `build_region_context` unconditionally calls `read_toggles()` ->
        // `rustline_wasm::data_root()`, which *reads* `XDG_DATA_HOME`; sibling
        // tests in this module that also call `build_region_context`
        // transitively read this var too, but none of them assert on
        // `ctx.toggled`/anything derived from `data_root()`, so a torn read
        // during their call can't change their outcome. This test and
        // `window_context_sets_only_window_and_skips_every_other_read` are the
        // only two whose assertions DO depend on the value, so both take
        // `ENV_LOCK` to serialize against each other; the mutation window is
        // kept minimal (just around the single call below).
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: serialized by `ENV_LOCK` above against the only other test
        // that also mutates this var.
        unsafe {
            std::env::set_var("XDG_DATA_HOME", tmp.path());
        }
        let ctx = build_region_context(&RegionArgs::default(), &[], &Theme::default(), "");
        // SAFETY: matches the set above; restores the process env for other tests.
        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        drop(guard);
        assert!(ctx.toggled.contains("cpu") && ctx.toggled.contains("memory"));
    }

    #[test]
    fn cpu_memory_sampled_only_when_region_names_them() {
        // Empty layout: neither expensive read runs, so both stay None — this is
        // what spares `render left` / `render window` the read_cpu sleep.
        let ctx = build_region_context(&RegionArgs::default(), &[], &Theme::default(), "");
        assert!(ctx.cpu.is_none() && ctx.memory.is_none());
        // The window path never samples cpu/memory at all.
        let wctx = build_window_context(&WindowArgs {
            current: false,
            index: String::new(),
            name: String::new(),
            flags: String::new(),
            preview: false,
        });
        assert!(wctx.cpu.is_none() && wctx.memory.is_none());
    }

    #[test]
    fn git_read_only_when_region_names_it() {
        // Empty layout: the git shell-out never runs, so it stays None — same
        // "pay only for what the region references" gating as cpu/memory.
        let ctx = build_region_context(&RegionArgs::default(), &[], &Theme::default(), "");
        assert!(ctx.git.is_none());
    }

    #[test]
    fn disk_read_only_when_region_names_it() {
        // Empty layout: the statvfs read never runs, so it stays None — same
        // "pay only for what the region references" gating as cpu/memory/git.
        let ctx = build_region_context(&RegionArgs::default(), &[], &Theme::default(), "/");
        assert!(ctx.disk.is_none());
    }

    #[test]
    fn disk_read_when_region_names_it_uses_configured_mount() {
        // Named in the layout: the configured mount is actually read.
        let ctx = build_region_context(
            &RegionArgs::default(),
            &["disk".to_string()],
            &Theme::default(),
            "/",
        );
        assert!(ctx.disk.is_some());
    }

    #[test]
    fn window_context_sets_only_window_and_skips_every_other_read() {
        // The window pill render path (verified by reading
        // `assemble::render_window`, `Windows::render`, and
        // `render::render_window_pill`) consumes only `Context.window`; the
        // pill's colors come from the `Theme` passed directly to
        // `render_window`, never `Context.colors`. So even with a populated
        // toggles file on disk, the lean builder must NOT read it (or
        // hostname/loadavg/interfaces/battery) -- proving it no longer routes
        // through `build_region_context`.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("rustline")).unwrap();
        std::fs::write(tmp.path().join("rustline/toggles"), "cpu\n").unwrap();
        // Same pattern/rationale as `build_region_context_reads_toggles_from_
        // state_file` above: this test's assertion also depends on the value
        // `read_toggles()` would see, so it takes the same `ENV_LOCK` to
        // serialize against that test rather than racing on the shared
        // process-global `XDG_DATA_HOME`.
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: serialized by `ENV_LOCK` above against the only other test
        // that also mutates this var.
        unsafe {
            std::env::set_var("XDG_DATA_HOME", tmp.path());
        }
        let ctx = build_window_context(&WindowArgs {
            current: true,
            index: "1".into(),
            name: "shell".into(),
            flags: "*".into(),
            preview: false,
        });
        // SAFETY: matches the set above; restores the process env for other tests.
        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        drop(guard);

        assert_eq!(
            ctx.window,
            Some(WindowCtx {
                index: "1".into(),
                name: "shell".into(),
                flags: "*".into(),
                is_current: true,
            })
        );
        assert!(
            ctx.toggled.is_empty(),
            "toggles file on disk must not be read: {:?}",
            ctx.toggled
        );
        assert!(ctx.hostname.is_empty(), "hostname must not be read");
        assert!(ctx.home.is_empty(), "$HOME must not be read");
        assert!(ctx.loadavg.is_none(), "getloadavg must not be called");
        assert!(ctx.interfaces.is_empty(), "getifaddrs must not be called");
        assert!(ctx.battery.is_none());
        assert!(ctx.cpu.is_none() && ctx.memory.is_none());
        assert!(ctx.git.is_none() && ctx.disk.is_none());
        assert!(ctx.uptime.is_none());
        assert!(ctx.media.is_none());
    }
}
