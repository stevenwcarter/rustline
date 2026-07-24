//! Build a [`Context`] from CLI arguments plus live host state (env vars,
//! load average, hostname, wall clock).

use std::collections::BTreeMap;
use std::env;

use crate::cli::{RegionArgs, WindowArgs};
use rustline_core::{Config, Context, NetIface, Theme, WindowCtx};

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
/// Every expensive OS read (`read_cpu` sleeps ~120ms on Linux; `read_memory`
/// on macOS spawns `vm_stat`; `read_git` shells out to `git`; `read_disk`
/// calls `statvfs(2)`; `read_throughput` reads `/proc/net/dev` (Linux only);
/// `read_battery` scans sysfs; `read_uptime` reads `/proc/uptime`;
/// `read_media` shells out to `playerctl`; `read_interfaces` calls
/// `getifaddrs(3)`) is taken ONLY when the region actually renders a widget of
/// that kind — the same "pay only for what the region references" gating
/// `register_plugins` uses, made **kind-aware** by [`Config::layout_kinds`] so
/// a `[instances.<name>]` widget (W46) drives its kind's read just like the
/// built-in of that name.
///
/// Disk and throughput fan out to one read per distinct target the layout
/// references ([`Config::disk_mounts`]/[`Config::throughput_interfaces`]):
/// `Context.disks`/`throughputs` get every instance's mount/interface, while
/// the singular `Context.disk`/`throughput` stay the *base* widget's entry so
/// the built-in `disk`/`throughput` widget resolves unchanged. The cpu/memory
/// `{spark}` history is read+persisted only when the base widget both is in
/// the layout AND its configured `format` contains the literal `{spark}` —
/// otherwise `Context.cpu_history`/`mem_history` stay empty with no history
/// I/O, keeping `{spark}`-absent output byte-identical (W45).
pub fn build_region_context(
    args: &RegionArgs,
    layout: &[String],
    theme: &Theme,
    cfg: &Config,
) -> Context {
    let kinds = cfg.layout_kinds(layout);
    let pane_current_path = args.pane_path.clone().unwrap_or_default();
    let git = if kinds.contains("git") {
        crate::git::read_git(&pane_current_path)
    } else {
        None
    };
    // One `read_disk` per distinct mount the layout references (the base
    // `[widgets.disk]` plus every `disk`-kind instance, W46); `Context.disk`
    // stays the base mount's entry so the built-in `disk` widget still
    // resolves via `ctx.disks.get(&self.mount)`.
    let mut disks = BTreeMap::new();
    for mount in cfg.disk_mounts(layout) {
        if let Some(info) = crate::disk::read_disk(&mount) {
            disks.insert(mount, info);
        }
    }
    let disk = disks.get(&cfg.widgets.disk.mount).cloned();
    // One `read_throughput` per distinct interface the layout references,
    // keyed the same way `ThroughputWidget` computes its `iface_key`
    // (`interface.unwrap_or_default()`, `""` = aggregate). `read_throughput`
    // persists a per-interface sample file, so distinct interfaces don't
    // clobber. `Context.throughput` stays the base interface's entry.
    let mut throughputs = BTreeMap::new();
    let throughput_ifaces = cfg.throughput_interfaces(layout);
    if !throughput_ifaces.is_empty() {
        let state_root = rustline_wasm::state_root();
        for iface in throughput_ifaces {
            if let Some(rate) = crate::throughput::read_throughput(&state_root, iface.as_deref()) {
                throughputs.insert(iface.unwrap_or_default(), rate);
            }
        }
    }
    let base_iface_key = cfg.widgets.throughput.interface.clone().unwrap_or_default();
    let throughput = throughputs.get(&base_iface_key).cloned();
    // Interfaces feed both IP widgets, so either kind names the read.
    let interfaces = if kinds.contains("lan_ip") || kinds.contains("tailscale_ip") {
        read_interfaces()
    } else {
        Vec::new()
    };
    let battery = if kinds.contains("battery") {
        crate::battery::read_battery()
    } else {
        None
    };
    let uptime = if kinds.contains("uptime") {
        crate::uptime::read_uptime()
    } else {
        None
    };
    let media = if kinds.contains("media") {
        crate::media::read_media()
    } else {
        None
    };
    let cpu = if kinds.contains("cpu") {
        crate::cpu::read_cpu()
    } else {
        None
    };
    let cpu_history = match cpu {
        Some(c) if cfg.widgets.cpu.format.contains("{spark}") => crate::cpu::read_cpu_history(
            &rustline_wasm::state_root(),
            c.percent,
            cfg.widgets.cpu.spark_width,
        ),
        _ => Vec::new(),
    };
    let memory = if kinds.contains("memory") {
        crate::memory::read_memory()
    } else {
        None
    };
    let mem_history = match memory {
        Some(m) if cfg.widgets.memory.format.contains("{spark}") => {
            let percent = if m.total_bytes == 0 {
                0.0
            } else {
                (m.used_bytes as f64 / m.total_bytes as f64 * 100.0) as f32
            };
            crate::memory::read_memory_history(
                &rustline_wasm::state_root(),
                percent,
                cfg.widgets.memory.spark_width,
            )
        }
        _ => Vec::new(),
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
        cpu,
        cpu_history,
        memory,
        mem_history,
        git,
        disk,
        disks,
        throughput,
        throughputs,
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

    /// Guards the tests below that mutate the process-global `XDG_DATA_HOME`
    /// env var: cargo's test harness runs tests in the same process
    /// concurrently, and each test's assertions depend on the value
    /// `read_toggles()`/`read_throughput()` (via `rustline_wasm::state_root()`)
    /// sees during its own critical section, so an unguarded interleaving of
    /// one test's `set_var`/`remove_var` with another's read would be a real
    /// race, not just a theoretical one.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn home_from_env_used_when_present() {
        // build_context reads $HOME; assert the field is populated non-empty
        let ctx = build_region_context(
            &RegionArgs::default(),
            &[],
            &Theme::default(),
            &Config::default(),
        );
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
            &Config::default(),
        );
        assert!(
            ctx.interfaces
                .iter()
                .all(|i| i.ipv4 != std::net::Ipv4Addr::LOCALHOST)
        );
    }

    #[test]
    fn interfaces_sampled_only_when_region_names_an_ip_widget() {
        // Empty layout: getifaddrs never runs, so interfaces stays at its
        // not-found value (empty), never a stale/fabricated read.
        let ctx = build_region_context(
            &RegionArgs::default(),
            &[],
            &Theme::default(),
            &Config::default(),
        );
        assert!(ctx.interfaces.is_empty());

        // Named in the layout (either IP widget triggers the shared read):
        // the real read runs, matching a direct read_interfaces() call.
        for name in ["lan_ip", "tailscale_ip"] {
            let ctx = build_region_context(
                &RegionArgs::default(),
                &[name.to_string()],
                &Theme::default(),
                &Config::default(),
            );
            assert_eq!(ctx.interfaces, read_interfaces());
        }
    }

    #[test]
    fn battery_sampled_only_when_region_names_it() {
        // Empty layout: the sysfs scan never runs, so it stays None.
        let ctx = build_region_context(
            &RegionArgs::default(),
            &[],
            &Theme::default(),
            &Config::default(),
        );
        assert!(ctx.battery.is_none());

        // Named in the layout: the real read runs, matching a direct
        // read_battery() call.
        let ctx = build_region_context(
            &RegionArgs::default(),
            &["battery".to_string()],
            &Theme::default(),
            &Config::default(),
        );
        assert_eq!(ctx.battery, crate::battery::read_battery());
    }

    #[test]
    fn uptime_sampled_only_when_region_names_it() {
        // Empty layout: the read never runs, so it stays None — same
        // "pay only for what the region references" gating as battery.
        let ctx = build_region_context(
            &RegionArgs::default(),
            &[],
            &Theme::default(),
            &Config::default(),
        );
        assert!(ctx.uptime.is_none());

        // Named in the layout: the real read runs, matching a direct
        // read_uptime() call.
        let ctx = build_region_context(
            &RegionArgs::default(),
            &["uptime".to_string()],
            &Theme::default(),
            &Config::default(),
        );
        assert_eq!(ctx.uptime, crate::uptime::read_uptime());
    }

    #[test]
    fn media_read_only_when_region_names_it() {
        // Empty layout: the playerctl shell-out never runs, so it stays None —
        // same "pay only for what the region references" gating as
        // cpu/memory/git/disk/battery/uptime.
        let ctx = build_region_context(
            &RegionArgs::default(),
            &[],
            &Theme::default(),
            &Config::default(),
        );
        assert!(ctx.media.is_none());

        // Named in the layout: the real read runs, matching a direct
        // read_media() call.
        let ctx = build_region_context(
            &RegionArgs::default(),
            &["media".to_string()],
            &Theme::default(),
            &Config::default(),
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
        let ctx = build_region_context(
            &RegionArgs::default(),
            &[],
            &Theme::default(),
            &Config::default(),
        );
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
        let ctx = build_region_context(
            &RegionArgs::default(),
            &[],
            &Theme::default(),
            &Config::default(),
        );
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
    fn cpu_mem_history_empty_when_spark_absent_from_format() {
        // cpu/memory are in the layout (so they're read at all), but neither
        // configured format references {spark} -> no history I/O, both
        // histories stay empty. The default config's formats
        // (`{icon} {percent}%` / `{icon} {used}/{total}`) never touch the
        // history ring — the byte-identical-by-default case (W45).
        let layout = ["cpu".to_string(), "memory".to_string()];
        let ctx = build_region_context(
            &RegionArgs::default(),
            &layout,
            &Theme::default(),
            &Config::default(),
        );
        assert!(ctx.cpu_history.is_empty());
        assert!(ctx.mem_history.is_empty());
    }

    #[test]
    fn cpu_mem_history_populated_only_when_format_references_spark() {
        // cpu/memory in the layout AND their configured format references
        // {spark}: the history read/persist actually runs, so the just-read
        // percent lands in the returned history.
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.widgets.cpu.format = "{icon} {spark} {percent}%".into();
        cfg.widgets.memory.format = "{icon} {spark} {percent}%".into();
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: serialized by `ENV_LOCK` against the other tests in this
        // module that also mutate this var (history I/O routes through
        // `rustline_wasm::state_root()`, which reads it).
        unsafe {
            std::env::set_var("XDG_DATA_HOME", tmp.path());
        }
        let layout = ["cpu".to_string(), "memory".to_string()];
        let ctx = build_region_context(&RegionArgs::default(), &layout, &Theme::default(), &cfg);
        // SAFETY: matches the set above; restores the process env for other tests.
        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        drop(guard);
        assert_eq!(ctx.cpu_history.len(), 1);
        assert_eq!(ctx.mem_history.len(), 1);
    }

    #[test]
    fn git_read_only_when_region_names_it() {
        // Empty layout: the git shell-out never runs, so it stays None — same
        // "pay only for what the region references" gating as cpu/memory.
        let ctx = build_region_context(
            &RegionArgs::default(),
            &[],
            &Theme::default(),
            &Config::default(),
        );
        assert!(ctx.git.is_none());
    }

    #[test]
    fn disk_read_only_when_region_names_it() {
        // Empty layout: the statvfs read never runs, so it stays None — same
        // "pay only for what the region references" gating as cpu/memory/git.
        let ctx = build_region_context(
            &RegionArgs::default(),
            &[],
            &Theme::default(),
            &Config::default(),
        );
        assert!(ctx.disk.is_none());
        assert!(ctx.disks.is_empty());
    }

    #[test]
    fn disk_read_when_region_names_it_uses_configured_mount() {
        // Named in the layout: the configured base mount ("/") is read into
        // both the singular `disk` and the `disks` map.
        let ctx = build_region_context(
            &RegionArgs::default(),
            &["disk".to_string()],
            &Theme::default(),
            &Config::default(),
        );
        assert!(ctx.disk.is_some());
        assert!(ctx.disks.contains_key("/"));
    }

    #[test]
    fn two_disk_instances_populate_disks_map_with_both_mounts() {
        // A base `disk` (mount "/") plus a `disk`-kind instance on a distinct
        // mount both drive their own `read_disk`, so `Context.disks` carries an
        // entry per mount (W46) rather than one clobbering the other. The
        // instance mount is a real, statvfs-able directory (a fresh tempdir) so
        // the read genuinely succeeds and proves the instance path fired — not
        // just the base "/" the pre-W46 bridge already read.
        let tmp = tempfile::tempdir().unwrap();
        let mount2 = tmp.path().to_str().unwrap().to_string();
        let mut table = toml::value::Table::new();
        table.insert("kind".into(), "disk".into());
        table.insert("mount".into(), mount2.clone().into());
        let mut cfg = Config::default();
        cfg.instances
            .insert("disk_data".into(), toml::Value::Table(table));
        let layout = ["disk".to_string(), "disk_data".to_string()];
        let ctx = build_region_context(&RegionArgs::default(), &layout, &Theme::default(), &cfg);
        assert!(ctx.disks.contains_key("/"), "base mount read fired");
        assert!(ctx.disks.contains_key(&mount2), "instance mount read fired");
        // The singular `disk` remains the base mount's entry.
        assert_eq!(ctx.disk, ctx.disks.get("/").copied());
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
        assert!(ctx.throughput.is_none());
        assert!(ctx.uptime.is_none());
        assert!(ctx.media.is_none());
    }

    #[test]
    fn throughput_sampled_only_when_region_names_it() {
        // Empty layout: the /proc/net/dev read never runs, so it stays None —
        // same "pay only for what the region references" gating as
        // cpu/memory/git/disk. `layout_kinds` yields no "throughput" kind, so
        // `read_throughput` (and thus `state_root()`) is never touched and this
        // half needs no env isolation.
        let ctx = build_region_context(
            &RegionArgs::default(),
            &[],
            &Theme::default(),
            &Config::default(),
        );
        assert!(ctx.throughput.is_none());
        assert!(ctx.throughputs.is_empty());

        // Named in the layout: the real read fires. `read_throughput` is
        // stateful (persists a sample it diffs the *next* call against), so
        // — like `build_region_context_reads_toggles_from_state_file` above —
        // this redirects `XDG_DATA_HOME` to an isolated tempdir first: partly
        // so the assertion is deterministic (first call: no prior sample ->
        // None; second call: diffs against what the first call persisted ->
        // Some, proving the gate actually let the read through), and partly
        // so a throughput test never writes into the developer's real
        // `~/.local/share/rustline/state/`.
        let tmp = tempfile::tempdir().unwrap();
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: serialized by `ENV_LOCK` against the other tests in this
        // module that also mutate this var.
        unsafe {
            std::env::set_var("XDG_DATA_HOME", tmp.path());
        }
        let layout = ["throughput".to_string()];
        let first = build_region_context(
            &RegionArgs::default(),
            &layout,
            &Theme::default(),
            &Config::default(),
        );
        let second = build_region_context(
            &RegionArgs::default(),
            &layout,
            &Theme::default(),
            &Config::default(),
        );
        // SAFETY: matches the set above; restores the process env for other tests.
        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        drop(guard);

        assert!(first.throughput.is_none(), "first run has no prior sample");
        assert!(
            second.throughput.is_some(),
            "second run diffs against the sample the first run persisted"
        );
    }
}
