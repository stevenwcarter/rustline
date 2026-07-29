mod alert;
mod bar;
pub mod battery;
pub mod cpu;
pub mod cwd;
pub mod datetime;
pub mod disk;
pub mod git;
pub mod hostname;
pub mod lan_ip;
pub mod loadavg;
pub mod media;
pub mod memory;
mod net;
pub mod pane_id;
mod spark;
pub mod tailscale_ip;
pub mod throughput;
mod toggle;
pub mod uptime;
pub mod windows;

pub use battery::BatteryWidget;
pub use cpu::CpuWidget;
pub use cwd::Cwd;
pub use datetime::DateTime;
pub use disk::DiskWidget;
pub use git::GitWidget;
pub use hostname::Hostname;
pub use lan_ip::LanIp;
pub use loadavg::LoadAvg;
pub use media::Media;
pub use memory::MemoryWidget;
pub use pane_id::PaneId;
pub use tailscale_ip::TailscaleIp;
// Re-exported for assemble.rs (Task 3) and the widgets' alt_format toggling
// (Task 4+) of the click-toggle plan.
pub use throughput::ThroughputWidget;
pub(crate) use toggle::{active_format, clickable_range};
pub use uptime::Uptime;
pub use windows::Windows;

// Re-exported for the numeric widgets (cpu/memory/battery/loadavg, Tasks
// 7-10) to render a threshold-alert badge.
pub(crate) use alert::{AlertKind, alert_over, alert_style, alert_under};

use crate::config::{
    BatteryOpts, CpuOpts, DateTimeOpts, DiskOpts, GitOpts, LanIpOpts, LoadAvgOpts, MediaOpts,
    MemoryOpts, TailscaleIpOpts, ThroughputOpts, UptimeOpts, instance_opts,
};
use crate::widget::{Registry, WidgetDescriptor, WidgetSource};
use crate::{Config, RangeName, Widget};

/// Build a minimal-boilerplate `WidgetDescriptor` for a built-in widget.
fn builtin_descriptor(name: &str, summary: &str, configurable: bool) -> WidgetDescriptor {
    WidgetDescriptor {
        name: name.to_string(),
        summary: summary.to_string(),
        configurable,
        source: WidgetSource::Builtin,
    }
}

/// Build a `WidgetDescriptor` for a named `[instances.<name>]` entry (W46),
/// labeled with its declared `kind` rather than [`WidgetSource::Builtin`].
///
/// Always configurable — an instance table is nothing *but* that kind's
/// options. Using this instead of [`builtin_descriptor`] in the
/// instance-registration pass below is what makes `registry.descriptors()`
/// truthful for every consumer (a prior bug reused `builtin_descriptor`
/// there, hardcoding every instance's `source` to `Builtin`).
fn instance_descriptor(name: &str, summary: &str, kind: &str) -> WidgetDescriptor {
    WidgetDescriptor {
        name: name.to_string(),
        summary: summary.to_string(),
        configurable: true,
        source: WidgetSource::Instance {
            kind: kind.to_string(),
        },
    }
}

// The twelve `build_<kind>` factories below each build one clickable/
// format-bearing widget instance under a caller-supplied `name` (its range/
// toggle identity, invariant #7) from that kind's current option values.
// Base registration below calls each with the kind name itself (so base
// output stays byte-identical); a later task (named `[instances]`
// registration, W46) reuses them to build additional instances under other
// names.

pub(crate) fn build_loadavg(name: &str, o: &LoadAvgOpts) -> Box<dyn Widget> {
    Box::new(LoadAvg {
        name: name.to_string(),
        format: o.format.clone(),
        alt_format: o.alt_format.clone(),
        down_format: o.down_format.clone(),
        warn_load: o.warn_load,
        crit_load: o.crit_load,
    })
}

pub(crate) fn build_datetime(name: &str, o: &DateTimeOpts) -> Box<dyn Widget> {
    Box::new(DateTime {
        name: name.to_string(),
        format: o.format.clone(),
        alt_format: o.alt_format.clone(),
        timezone: o.timezone.clone(),
    })
}

pub(crate) fn build_lan_ip(name: &str, o: &LanIpOpts) -> Box<dyn Widget> {
    Box::new(LanIp {
        name: name.to_string(),
        format: o.format.clone(),
        alt_format: o.alt_format.clone(),
        down_format: o.down_format.clone(),
        interface: o.interface.clone(),
    })
}

pub(crate) fn build_tailscale_ip(name: &str, o: &TailscaleIpOpts) -> Box<dyn Widget> {
    Box::new(TailscaleIp {
        name: name.to_string(),
        format: o.format.clone(),
        alt_format: o.alt_format.clone(),
        down_format: o.down_format.clone(),
    })
}

pub(crate) fn build_battery(name: &str, o: &BatteryOpts) -> Box<dyn Widget> {
    Box::new(BatteryWidget {
        name: name.to_string(),
        format: o.format.clone(),
        alt_format: o.alt_format.clone(),
        down_format: o.down_format.clone(),
        warn_percent: o.warn_percent,
        crit_percent: o.crit_percent,
        icon: o.icon.clone(),
    })
}

pub(crate) fn build_cpu(name: &str, o: &CpuOpts) -> Box<dyn Widget> {
    Box::new(CpuWidget {
        name: name.to_string(),
        format: o.format.clone(),
        alt_format: o.alt_format.clone(),
        down_format: o.down_format.clone(),
        bar_width: o.bar_width,
        warn_percent: o.warn_percent,
        crit_percent: o.crit_percent,
        icon: o.icon.clone(),
    })
}

pub(crate) fn build_memory(name: &str, o: &MemoryOpts) -> Box<dyn Widget> {
    Box::new(MemoryWidget {
        name: name.to_string(),
        format: o.format.clone(),
        alt_format: o.alt_format.clone(),
        down_format: o.down_format.clone(),
        bar_width: o.bar_width,
        warn_percent: o.warn_percent,
        crit_percent: o.crit_percent,
        icon: o.icon.clone(),
    })
}

pub(crate) fn build_git(name: &str, o: &GitOpts) -> Box<dyn Widget> {
    Box::new(GitWidget {
        name: name.to_string(),
        format: o.format.clone(),
        alt_format: o.alt_format.clone(),
        down_format: o.down_format.clone(),
        dirty_glyph: o.dirty_glyph.clone(),
    })
}

pub(crate) fn build_disk(name: &str, o: &DiskOpts) -> Box<dyn Widget> {
    Box::new(DiskWidget {
        name: name.to_string(),
        format: o.format.clone(),
        alt_format: o.alt_format.clone(),
        down_format: o.down_format.clone(),
        mount: o.mount.clone(),
        bar_width: o.bar_width,
        warn_percent: o.warn_percent,
        crit_percent: o.crit_percent,
    })
}

pub(crate) fn build_uptime(name: &str, o: &UptimeOpts) -> Box<dyn Widget> {
    Box::new(Uptime {
        name: name.to_string(),
        format: o.format.clone(),
        alt_format: o.alt_format.clone(),
        down_format: o.down_format.clone(),
    })
}

pub(crate) fn build_media(name: &str, o: &MediaOpts) -> Box<dyn Widget> {
    Box::new(Media {
        name: name.to_string(),
        format: o.format.clone(),
        alt_format: o.alt_format.clone(),
        down_format: o.down_format.clone(),
    })
}

pub(crate) fn build_throughput(name: &str, o: &ThroughputOpts) -> Box<dyn Widget> {
    Box::new(ThroughputWidget {
        name: name.to_string(),
        format: o.format.clone(),
        alt_format: o.alt_format.clone(),
        down_format: o.down_format.clone(),
        iface_key: o.interface.clone().unwrap_or_default(),
    })
}

impl Registry {
    /// Build a [`Registry`] pre-populated with all sixteen built-in widgets,
    /// configuring the ones that carry options (`pane_id`, `hostname`,
    /// `datetime`, `cwd`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`,
    /// `memory`, `loadavg`, `git`, `disk`, `uptime`, `media`, `throughput`)
    /// from `cfg`, then registers each `[instances.<name>]` entry (W46) as an
    /// additional named instance of one of the twelve clickable kinds under
    /// its own instance name (an unknown `kind` or a name colliding with an
    /// existing registration is skipped with a `warn!`).
    pub fn with_builtins(cfg: &Config) -> Registry {
        let mut registry = Registry::new();
        let pane_id = cfg.widgets.pane_id.clone();
        registry.register_described(
            builtin_descriptor(
                "pane_id",
                "The tmux pane target triple (session:window.pane)",
                true,
            ),
            Box::new(move || {
                Box::new(PaneId {
                    format: pane_id.format.clone(),
                })
            }),
        );
        let hostname = cfg.widgets.hostname.clone();
        registry.register_described(
            builtin_descriptor(
                "hostname",
                "The local hostname, truncated at the first dot",
                true,
            ),
            Box::new(move || {
                Box::new(Hostname {
                    format: hostname.format.clone(),
                })
            }),
        );
        registry.register_described(
            builtin_descriptor(
                "windows",
                "The tmux window list, rendered as rounded pills",
                false,
            ),
            Box::new(|| Box::new(Windows)),
        );
        let loadavg = cfg.widgets.loadavg.clone();
        registry.register_described(
            builtin_descriptor("loadavg", "1/5/15-minute load average", true),
            Box::new(move || build_loadavg("loadavg", &loadavg)),
        );

        let datetime = cfg.widgets.datetime.clone();
        registry.register_described(
            builtin_descriptor(
                "datetime",
                "The current time, `chrono` strftime-formatted",
                true,
            ),
            Box::new(move || build_datetime("datetime", &datetime)),
        );

        let cwd = cfg.widgets.cwd.clone();
        registry.register_described(
            builtin_descriptor("cwd", "The pane's current working directory", true),
            Box::new(move || {
                Box::new(Cwd {
                    abbreviate_home: cwd.abbreviate_home,
                    format: cwd.format.clone(),
                    max_depth: cwd.max_depth,
                    max_len: cwd.max_len,
                    abbreviate: cwd.abbreviate,
                })
            }),
        );

        let lan = cfg.widgets.lan_ip.clone();
        registry.register_described(
            builtin_descriptor("lan_ip", "The machine's LAN IPv4 address", true),
            Box::new(move || build_lan_ip("lan_ip", &lan)),
        );

        let ts = cfg.widgets.tailscale_ip.clone();
        registry.register_described(
            builtin_descriptor("tailscale_ip", "The machine's Tailscale IPv4 address", true),
            Box::new(move || build_tailscale_ip("tailscale_ip", &ts)),
        );

        let battery = cfg.widgets.battery.clone();
        registry.register_described(
            builtin_descriptor(
                "battery",
                "Battery percentage, charge state, and level icon",
                true,
            ),
            Box::new(move || build_battery("battery", &battery)),
        );

        let cpu = cfg.widgets.cpu.clone();
        registry.register_described(
            builtin_descriptor("cpu", "CPU utilization, with an optional gauge bar", true),
            Box::new(move || build_cpu("cpu", &cpu)),
        );

        let memory = cfg.widgets.memory.clone();
        registry.register_described(
            builtin_descriptor("memory", "Memory usage, with an optional gauge bar", true),
            Box::new(move || build_memory("memory", &memory)),
        );

        let git = cfg.widgets.git.clone();
        registry.register_described(
            builtin_descriptor(
                "git",
                "Current git branch, dirty marker, and ahead/behind counts",
                true,
            ),
            Box::new(move || build_git("git", &git)),
        );

        let disk = cfg.widgets.disk.clone();
        registry.register_described(
            builtin_descriptor("disk", "Filesystem usage for a configured mount", true),
            Box::new(move || build_disk("disk", &disk)),
        );

        let uptime = cfg.widgets.uptime.clone();
        registry.register_described(
            builtin_descriptor("uptime", "System uptime, humanized", true),
            Box::new(move || build_uptime("uptime", &uptime)),
        );

        let media = cfg.widgets.media.clone();
        registry.register_described(
            builtin_descriptor(
                "media",
                "Now-playing artist/title/status via playerctl",
                true,
            ),
            Box::new(move || build_media("media", &media)),
        );

        let throughput = cfg.widgets.throughput.clone();
        registry.register_described(
            builtin_descriptor(
                "throughput",
                "Network download/upload throughput (bytes-per-second)",
                true,
            ),
            Box::new(move || build_throughput("throughput", &throughput)),
        );

        // Second pass: `[instances.<name>]` (W46) registers additional named
        // instances of the twelve clickable/format-bearing kinds — the ones
        // with a `build_<kind>` helper above. Multi-instancing `cwd`/
        // `hostname`/`pane_id`/`windows` has no use case (YAGNI), so any
        // other `kind` (including those four) is simply an unsupported kind:
        // warn and skip, never registered. Each arm below records its
        // descriptor via `instance_descriptor`, not `builtin_descriptor` —
        // its `source` must be `WidgetSource::Instance { kind }`, since
        // `widget_placements` (crate::config) trusts `registry.descriptors()`
        // to say where a widget actually came from.
        for (name, table) in &cfg.instances {
            let Some(kind) = Config::instance_kind(table) else {
                crate::diag::warn_once(&format!("instance-no-kind:{name}"), || {
                    tracing::warn!(instance = %name, "instance missing `kind`, skipping");
                });
                continue;
            };
            if registry.contains(name) {
                crate::diag::warn_once(&format!("instance-collide:{name}"), || {
                    tracing::warn!(
                        instance = %name,
                        "instance name collides with an existing widget, skipping"
                    );
                });
                continue;
            }
            // Permissive (invariant N2): a name that fails to parse as a
            // `RangeName` (too long, a `[A-Za-z0-9_-]` violation, or the
            // reserved `window`) still registers below and still renders —
            // it just never becomes clickable, since `clickable_range` (via
            // each widget's `range_name()`) refuses to produce a `RangeName`
            // for it either. This is the same rule, checked once.
            if let Err(err) = RangeName::parse(name) {
                crate::diag::warn_once(&format!("instance-unclickable:{name}"), || {
                    tracing::warn!(instance = %name, %err, "not click-toggleable: {err}");
                });
            }
            let t = table.clone();
            let summary = format!("{kind} instance");
            match kind {
                "datetime" => {
                    let o: DateTimeOpts = instance_opts(name, kind, t);
                    let n = name.clone();
                    registry.register_described(
                        instance_descriptor(name, &summary, kind),
                        Box::new(move || build_datetime(&n, &o)),
                    );
                }
                "lan_ip" => {
                    let o: LanIpOpts = instance_opts(name, kind, t);
                    let n = name.clone();
                    registry.register_described(
                        instance_descriptor(name, &summary, kind),
                        Box::new(move || build_lan_ip(&n, &o)),
                    );
                }
                "tailscale_ip" => {
                    let o: TailscaleIpOpts = instance_opts(name, kind, t);
                    let n = name.clone();
                    registry.register_described(
                        instance_descriptor(name, &summary, kind),
                        Box::new(move || build_tailscale_ip(&n, &o)),
                    );
                }
                "battery" => {
                    let o: BatteryOpts = instance_opts(name, kind, t);
                    let n = name.clone();
                    registry.register_described(
                        instance_descriptor(name, &summary, kind),
                        Box::new(move || build_battery(&n, &o)),
                    );
                }
                "cpu" => {
                    let o: CpuOpts = instance_opts(name, kind, t);
                    let n = name.clone();
                    registry.register_described(
                        instance_descriptor(name, &summary, kind),
                        Box::new(move || build_cpu(&n, &o)),
                    );
                }
                "memory" => {
                    let o: MemoryOpts = instance_opts(name, kind, t);
                    let n = name.clone();
                    registry.register_described(
                        instance_descriptor(name, &summary, kind),
                        Box::new(move || build_memory(&n, &o)),
                    );
                }
                "loadavg" => {
                    let o: LoadAvgOpts = instance_opts(name, kind, t);
                    let n = name.clone();
                    registry.register_described(
                        instance_descriptor(name, &summary, kind),
                        Box::new(move || build_loadavg(&n, &o)),
                    );
                }
                "git" => {
                    let o: GitOpts = instance_opts(name, kind, t);
                    let n = name.clone();
                    registry.register_described(
                        instance_descriptor(name, &summary, kind),
                        Box::new(move || build_git(&n, &o)),
                    );
                }
                "disk" => {
                    let o: DiskOpts = instance_opts(name, kind, t);
                    let n = name.clone();
                    registry.register_described(
                        instance_descriptor(name, &summary, kind),
                        Box::new(move || build_disk(&n, &o)),
                    );
                }
                "uptime" => {
                    let o: UptimeOpts = instance_opts(name, kind, t);
                    let n = name.clone();
                    registry.register_described(
                        instance_descriptor(name, &summary, kind),
                        Box::new(move || build_uptime(&n, &o)),
                    );
                }
                "media" => {
                    let o: MediaOpts = instance_opts(name, kind, t);
                    let n = name.clone();
                    registry.register_described(
                        instance_descriptor(name, &summary, kind),
                        Box::new(move || build_media(&n, &o)),
                    );
                }
                "throughput" => {
                    let o: ThroughputOpts = instance_opts(name, kind, t);
                    let n = name.clone();
                    registry.register_described(
                        instance_descriptor(name, &summary, kind),
                        Box::new(move || build_throughput(&n, &o)),
                    );
                }
                other => {
                    crate::diag::warn_once(&format!("instance-kind:{name}:{other}"), || {
                        tracing::warn!(instance = %name, kind = %other, "unknown instance kind, skipping");
                    });
                }
            }
        }

        registry
    }
}

#[cfg(test)]
mod tests {
    use std::fmt;
    use std::sync::{Arc, Mutex};

    use super::{CpuOpts, instance_opts};
    use crate::widget::Registry;
    use crate::{Config, Context, NetIface};
    use chrono::{Local, TimeZone};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    fn ctx(ifaces: Vec<NetIface>) -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/".into(),
            home: "/h".into(),
            hostname: "h".into(),
            loadavg: None,
            now: Local
                .with_ymd_and_hms(2026, 7, 20, 17, 49, 0)
                .single()
                .unwrap(),
            window: None,
            interfaces: ifaces,
            battery: None,
            cpu: None,
            memory: None,
            git: None,
            disk: None,
            disks: Default::default(),
            throughput: None,
            throughputs: Default::default(),
            os: String::new(),
            arch: String::new(),
            uptime: None,
            media: None,
            toggled: Default::default(),
            cpu_history: Vec::new(),
            mem_history: Vec::new(),
            colors: Default::default(),
        }
    }

    #[test]
    fn with_builtins_descriptors_cover_all_sixteen_with_correct_configurable_flags() {
        let cfg = Config::default();
        let reg = Registry::with_builtins(&cfg);
        let names: Vec<&str> = reg.available_names().collect();
        for expected in [
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
        ] {
            assert!(names.contains(&expected), "missing descriptor: {expected}");
        }
        assert_eq!(names.len(), 16);

        let configurable = |name: &str| {
            reg.descriptors()
                .iter()
                .find(|d| d.name == name)
                .map(|d| d.configurable)
        };
        assert_eq!(configurable("cpu"), Some(true));
        assert_eq!(configurable("datetime"), Some(true));
        assert_eq!(configurable("pane_id"), Some(true));
        assert_eq!(configurable("hostname"), Some(true));
    }

    #[test]
    fn cwd_registered_and_renders_with_configured_options() {
        let mut cfg = Config::default();
        cfg.widgets.cwd.format = "cwd: {path}".into();
        cfg.widgets.cwd.max_depth = 1;
        let reg = Registry::with_builtins(&cfg);
        assert!(reg.contains("cwd"));

        let mut c = ctx(Vec::new());
        c.home = "/home/steve".into();
        c.pane_current_path = "/home/steve/src/rustline".into();
        let widgets = reg.resolve(&["cwd".into()]);
        let texts: Vec<String> = widgets
            .iter()
            .flat_map(|(_, w)| w.render(&c))
            .map(|s| s.text)
            .collect();
        // home-abbrev "~/src/rustline" -> max_depth 1 keeps "rustline" -> format wraps it.
        assert_eq!(texts, vec!["cwd: …/rustline".to_string()]);
    }

    #[test]
    fn ip_widgets_registered_and_render_from_context() {
        let mut cfg = Config::default();
        cfg.widgets.lan_ip.format = "LAN {ip}".into();
        cfg.widgets.tailscale_ip.down_format = "TS off".into();
        let reg = Registry::with_builtins(&cfg);
        assert!(reg.contains("lan_ip") && reg.contains("tailscale_ip"));

        let widgets = reg.resolve(&["lan_ip".into(), "tailscale_ip".into()]);
        let c = ctx(vec![
            NetIface {
                name: "eth0".into(),
                ipv4: "192.168.1.20".parse().unwrap(),
            },
            NetIface {
                name: "tailscale0".into(),
                ipv4: "100.101.4.7".parse().unwrap(),
            },
        ]);
        let texts: Vec<String> = widgets
            .iter()
            .flat_map(|(_, w)| w.render(&c))
            .map(|s| s.text)
            .collect();
        assert_eq!(
            texts,
            vec!["LAN 192.168.1.20".to_string(), "100.101.4.7".to_string()]
        );

        // no interfaces + default lan down_format -> lan_ip skipped, tailscale shows down text
        let widgets = reg.resolve(&["lan_ip".into(), "tailscale_ip".into()]);
        let texts: Vec<String> = widgets
            .iter()
            .flat_map(|(_, w)| w.render(&ctx(vec![])))
            .map(|s| s.text)
            .collect();
        assert_eq!(texts, vec!["TS off".to_string()]);
    }

    #[test]
    fn battery_registered_and_renders_from_context() {
        use crate::{Battery, BatteryState};
        let cfg = Config::default();
        let reg = Registry::with_builtins(&cfg);
        assert!(reg.contains("battery"));

        let mut c = ctx(Vec::new());
        c.battery = Some(Battery {
            percent: 73,
            state: BatteryState::Discharging,
        });
        let widgets = reg.resolve(&["battery".into()]);
        let texts: Vec<String> = widgets
            .iter()
            .flat_map(|(_, w)| w.render(&c))
            .map(|s| s.text)
            .collect();
        // default format "{icon} {percent}%", 73% discharging -> md-battery-70.
        assert_eq!(texts, vec!["\u{f0080} 73%".to_string()]);

        // No battery + default (empty) down_format -> widget skipped.
        let mut c0 = ctx(Vec::new());
        c0.battery = None;
        let widgets = reg.resolve(&["battery".into()]);
        let texts: Vec<String> = widgets
            .iter()
            .flat_map(|(_, w)| w.render(&c0))
            .map(|s| s.text)
            .collect();
        assert!(texts.is_empty());
    }

    #[test]
    fn cpu_memory_registered_and_render_from_context() {
        use crate::{CpuUsage, MemInfo};
        let cfg = Config::default();
        let reg = Registry::with_builtins(&cfg);
        assert!(reg.contains("cpu") && reg.contains("memory"));

        let mut c = ctx(Vec::new());
        c.cpu = Some(CpuUsage { percent: 50.0 });
        let g = 1024u64.pow(3);
        c.memory = Some(MemInfo {
            total_bytes: 16 * g,
            used_bytes: 8 * g,
            available_bytes: 8 * g,
        });
        let texts: Vec<String> = reg
            .resolve(&["cpu".into(), "memory".into()])
            .iter()
            .flat_map(|(_, w)| w.render(&c))
            .map(|s| s.text)
            .collect();
        // cpu default "{icon} {percent}%" and memory default "{icon} {used}/{total}"
        assert_eq!(
            texts,
            vec![
                "\u{f061a} 50%".to_string(),
                "\u{f035b} 8.0G/16G".to_string()
            ]
        );
    }

    #[test]
    fn git_registered_and_renders_from_context() {
        use crate::GitInfo;
        let cfg = Config::default();
        let reg = Registry::with_builtins(&cfg);
        assert!(reg.contains("git"));

        let mut c = ctx(Vec::new());
        c.git = Some(GitInfo {
            branch: "main".into(),
            ahead: 0,
            behind: 0,
            staged: 1,
            unstaged: 0,
        });
        let widgets = reg.resolve(&["git".into()]);
        let texts: Vec<String> = widgets
            .iter()
            .flat_map(|(_, w)| w.render(&c))
            .map(|s| s.text)
            .collect();
        // default format "\u{e0a0} {branch}{dirty}", dirty_glyph "*".
        assert_eq!(texts, vec!["\u{e0a0} main*".to_string()]);

        // No git info + default (empty) down_format -> widget skipped.
        let mut c0 = ctx(Vec::new());
        c0.git = None;
        let widgets = reg.resolve(&["git".into()]);
        let texts: Vec<String> = widgets
            .iter()
            .flat_map(|(_, w)| w.render(&c0))
            .map(|s| s.text)
            .collect();
        assert!(texts.is_empty());
    }

    #[test]
    fn disk_registered_and_renders_from_context() {
        use crate::DiskInfo;
        let cfg = Config::default();
        let reg = Registry::with_builtins(&cfg);
        assert!(reg.contains("disk"));

        // The base `disk` widget reads `cfg.widgets.disk.mount` (default "/")
        // as its lookup key into `Context.disks` (W46).
        let mut c = ctx(Vec::new());
        let g = 1024u64.pow(3);
        c.disks.insert(
            cfg.widgets.disk.mount.clone(),
            DiskInfo {
                total_bytes: 16 * g,
                used_bytes: 6 * g,
                available_bytes: 10 * g,
            },
        );
        let widgets = reg.resolve(&["disk".into()]);
        let texts: Vec<String> = widgets
            .iter()
            .flat_map(|(_, w)| w.render(&c))
            .map(|s| s.text)
            .collect();
        // default format " {used}/{total}".
        assert_eq!(texts, vec![" 6.0G/16G".to_string()]);

        // No entry for the configured mount + default (empty) down_format ->
        // widget skipped.
        let c0 = ctx(Vec::new());
        let widgets = reg.resolve(&["disk".into()]);
        let texts: Vec<String> = widgets
            .iter()
            .flat_map(|(_, w)| w.render(&c0))
            .map(|s| s.text)
            .collect();
        assert!(texts.is_empty());
    }

    #[test]
    fn uptime_registered_and_renders_from_context() {
        let cfg = Config::default();
        let reg = Registry::with_builtins(&cfg);
        assert!(reg.contains("uptime"));

        let mut c = ctx(Vec::new());
        c.uptime = Some(86_400 * 3 + 3600 * 4);
        let widgets = reg.resolve(&["uptime".into()]);
        let texts: Vec<String> = widgets
            .iter()
            .flat_map(|(_, w)| w.render(&c))
            .map(|s| s.text)
            .collect();
        // default format "{uptime}".
        assert_eq!(texts, vec!["3d 4h".to_string()]);

        // No uptime reading + default (empty) down_format -> widget skipped.
        let mut c0 = ctx(Vec::new());
        c0.uptime = None;
        let widgets = reg.resolve(&["uptime".into()]);
        let texts: Vec<String> = widgets
            .iter()
            .flat_map(|(_, w)| w.render(&c0))
            .map(|s| s.text)
            .collect();
        assert!(texts.is_empty());
    }

    #[test]
    fn media_registered_and_renders_from_context() {
        use crate::MediaInfo;
        let cfg = Config::default();
        let reg = Registry::with_builtins(&cfg);
        assert!(reg.contains("media"));

        let mut c = ctx(Vec::new());
        c.media = Some(MediaInfo {
            artist: "Radiohead".into(),
            title: "Karma Police".into(),
            status: "Playing".into(),
        });
        let widgets = reg.resolve(&["media".into()]);
        let texts: Vec<String> = widgets
            .iter()
            .flat_map(|(_, w)| w.render(&c))
            .map(|s| s.text)
            .collect();
        // default format "{title} — {artist}".
        assert_eq!(texts, vec!["Karma Police — Radiohead".to_string()]);

        // No media reading + default (empty) down_format -> widget skipped.
        let mut c0 = ctx(Vec::new());
        c0.media = None;
        let widgets = reg.resolve(&["media".into()]);
        let texts: Vec<String> = widgets
            .iter()
            .flat_map(|(_, w)| w.render(&c0))
            .map(|s| s.text)
            .collect();
        assert!(texts.is_empty());
    }

    #[test]
    fn throughput_registered_and_renders_from_context() {
        use crate::Throughput;
        let cfg = Config::default();
        let reg = Registry::with_builtins(&cfg);
        assert!(reg.contains("throughput"));

        // The base `throughput` widget reads `cfg.widgets.throughput.interface`
        // (default `None` -> "") as its lookup key into `Context.throughputs`
        // (W46).
        let mut c = ctx(Vec::new());
        c.throughputs.insert(
            cfg.widgets.throughput.interface.clone().unwrap_or_default(),
            Throughput {
                down_bytes_per_sec: 1024,
                up_bytes_per_sec: 2048,
            },
        );
        let widgets = reg.resolve(&["throughput".into()]);
        let texts: Vec<String> = widgets
            .iter()
            .flat_map(|(_, w)| w.render(&c))
            .map(|s| s.text)
            .collect();
        // default format " {down} {up}".
        assert_eq!(texts, vec![" 1.0K/s 2.0K/s".to_string()]);

        // No entry for the configured interface key + default (empty)
        // down_format -> widget skipped.
        let c0 = ctx(Vec::new());
        let widgets = reg.resolve(&["throughput".into()]);
        let texts: Vec<String> = widgets
            .iter()
            .flat_map(|(_, w)| w.render(&c0))
            .map(|s| s.text)
            .collect();
        assert!(texts.is_empty());
    }

    #[test]
    fn datetime_instance_uses_its_own_name_for_range_and_toggle() {
        // Build a datetime widget under a non-kind name and confirm its range
        // name and toggle key are that name, not "datetime".
        let w = super::build_datetime(
            "clock_utc",
            &crate::config::DateTimeOpts {
                alt_format: "%H:%M".into(),
                ..Default::default()
            },
        );
        assert_eq!(
            w.range_name(),
            Some(crate::RangeName::parse("clock_utc").unwrap())
        );
    }

    #[test]
    fn two_datetime_instances_render_distinct_timezones() {
        let mut cfg = Config::default();
        cfg.instances.insert(
            "clock_utc".into(),
            toml::from_str("kind='datetime'\ntimezone='UTC'\nformat='%H'").unwrap(),
        );
        cfg.instances.insert(
            "clock_ny".into(),
            toml::from_str("kind='datetime'\ntimezone='America/New_York'\nformat='%H'").unwrap(),
        );
        let reg = Registry::with_builtins(&cfg);
        assert!(reg.contains("clock_utc") && reg.contains("clock_ny"));
        let out = reg.resolve(&["clock_utc".into(), "clock_ny".into()]);
        assert_eq!(out.len(), 2);

        // A FIXED instant (from the `ctx` helper's pinned `now`) renders to
        // different hours in UTC vs America/New_York — the two zones are always
        // 4–5h apart, never equal — so `%H` must yield distinct text. Asserting
        // only `out.len() == 2` would pass even if timezone were ignored.
        let c = ctx(Vec::new());
        let texts: Vec<String> = out
            .iter()
            .map(|(_, w)| w.render(&c).into_iter().map(|s| s.text).collect())
            .collect();
        assert_ne!(
            texts[0], texts[1],
            "UTC and America/New_York must format to different hours"
        );
    }

    #[test]
    fn instance_descriptor_reports_its_kind_not_builtin() {
        // Regression test: `Registry::with_builtins`'s instance-registration
        // pass used to reuse `builtin_descriptor`, which hardcodes
        // `source: WidgetSource::Builtin` — so `registry.descriptors()`
        // mislabeled every `[instances.<name>]` entry as a built-in. Assert
        // directly on the registry's own descriptors (independent of
        // `widget_placements`) so any future consumer of `descriptors()` is
        // covered too.
        use crate::widget::WidgetSource;

        let mut cfg = Config::default();
        cfg.instances.insert(
            "clock_utc".into(),
            toml::from_str("kind='datetime'\ntimezone='UTC'").unwrap(),
        );
        let reg = Registry::with_builtins(&cfg);
        let desc = reg
            .descriptors()
            .iter()
            .find(|d| d.name == "clock_utc")
            .expect("clock_utc instance registered");
        assert_eq!(
            desc.source,
            WidgetSource::Instance {
                kind: "datetime".into()
            }
        );
    }

    #[test]
    fn unknown_kind_and_builtin_collision_are_skipped() {
        let mut cfg = Config::default();
        cfg.instances
            .insert("bogus".into(), toml::from_str("kind='nope'").unwrap());
        cfg.instances.insert(
            "cpu".into(),
            toml::from_str("kind='datetime'").unwrap(), // collides
        );
        let reg = Registry::with_builtins(&cfg);
        assert!(!reg.contains("bogus"));
        // "cpu" stays the built-in cpu widget, not the datetime instance:
        assert!(reg.contains("cpu"));
    }

    #[test]
    fn instance_range_name_and_toggle_use_instance_name() {
        let mut cfg = Config::default();
        cfg.instances.insert(
            "clk".into(),
            toml::from_str("kind='datetime'\nalt_format='%H:%M'").unwrap(),
        );
        let reg = Registry::with_builtins(&cfg);
        let w = reg.build("clk").unwrap();
        assert_eq!(
            w.range_name(),
            Some(crate::RangeName::parse("clk").unwrap())
        );
    }

    #[test]
    fn instance_opts_falls_back_and_reports_a_type_error() {
        let v: toml::Value = toml::from_str("spark_width = \"8\"").unwrap();
        // Fired under `capture` — discarding the returned events, since this
        // test only pins the fallback value — rather than under the ambient
        // dispatcher: `instance_opts::<CpuOpts>`'s `tracing::warn!` shares a
        // single process-wide callsite with the identical call in
        // `instance_opts_reports_the_type_error_via_warn_once` below. Firing
        // it here with no subscriber installed would race that other test's
        // `Dispatch::new` (from its own `capture` call) to register the
        // callsite's cached interest; see `capture`'s doc comment for the
        // measured failure rate this avoids.
        capture(|| {
            let got: CpuOpts = instance_opts("cpu_alt", "cpu", v);
            assert_eq!(got.spark_width, CpuOpts::default().spark_width);
        });
    }

    #[test]
    fn instance_opts_accepts_a_valid_table() {
        let v: toml::Value = toml::from_str("spark_width = 20").unwrap();
        let got: CpuOpts = instance_opts("cpu_alt", "cpu", v);
        assert_eq!(got.spark_width, 20);
    }

    #[test]
    fn an_instance_table_with_only_the_extra_kind_key_parses_cleanly() {
        // CLAUDE.md documents the extra `kind` key as harmless; it must not be
        // reported as a type error. Assert on a field the table actually
        // sets (`spark_width = 20`), not just the default: asserting only
        // `== CpuOpts::default().spark_width` can't tell a genuine parse
        // (which happens to default `spark_width` because the table doesn't
        // set it) apart from a parse *failure* (which also falls back to
        // `T::default()`) — both produce the same value. Verified this
        // version fails under a temporary `#[serde(deny_unknown_fields)]` on
        // `CpuOpts` and passes without it; see the task-3 report.
        let v: toml::Value = toml::from_str("kind = \"cpu\"\nspark_width = 20").unwrap();
        let got: CpuOpts = instance_opts("cpu_alt", "cpu", v);
        assert_eq!(got.spark_width, 20);
    }

    #[test]
    fn instance_opts_reports_the_type_error_via_warn_once() {
        // B12's whole point was that the old `try_into().unwrap_or_default()`
        // was *silent*. Assert the actual reported event, not just the
        // fallback value — a test that only checks `spark_width` would pass
        // just as happily against the pre-fix silent code.
        let v: toml::Value = toml::from_str("spark_width = \"8\"").unwrap();
        let events = capture(|| {
            let got: CpuOpts = instance_opts("cpu_alt", "cpu", v);
            assert_eq!(got.spark_width, CpuOpts::default().spark_width);
        });

        assert_eq!(events.len(), 1, "expected exactly one warn; got {events:?}");
        let (level, fields) = &events[0];
        assert_eq!(*level, Level::WARN);
        assert_eq!(field(fields, "instance"), Some("cpu_alt"));
        assert_eq!(field(fields, "kind"), Some("cpu"));
        assert!(
            field(fields, "error").is_some(),
            "expected an error field describing the type mismatch; got {fields:?}"
        );

        // Dedup itself (that a *repeat* call with the same key is
        // suppressed) is intentionally not pinned here: `warn_once` fails
        // open with no hook installed (`rustline_core::diag`'s default is
        // "always emit"), so a second call in this test would emit again
        // regardless of whether dedup logic is correct — it wouldn't pin
        // anything. Installing a hook to observe real dedup isn't safe from
        // a unit test either: the hook lives in a process-wide `OnceLock`
        // (`diag::HOOK`), so installing one here would make this test's
        // outcome depend on whichever test in this binary happens to run
        // first. That property is already covered end-to-end, across real
        // process boundaries, by
        // `crates/rustline/tests/smoke.rs::warn_dedup_resets_when_config_mtime_changes`.
    }

    /// Run `f` under a scoped recording subscriber and return every event it
    /// emitted, in order. Scoped via `tracing::subscriber::with_default`, so
    /// it can never race or interfere with any other test's subscriber
    /// regardless of test execution order. Ported from
    /// `rustline-wasm/src/lib.rs`'s `capture` harness rather than inventing a
    /// new approach.
    type CapturedEvent = (Level, Vec<(String, String)>);

    #[derive(Default)]
    struct FieldVisitor(Vec<(String, String)>);

    impl Visit for FieldVisitor {
        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            self.0
                .push((field.name().to_string(), format!("{value:?}")));
        }
    }

    /// A subscriber that accepts and records every event, purely so a test
    /// can assert what `instance_opts` logged.
    struct RecordingSubscriber(Arc<Mutex<Vec<CapturedEvent>>>);

    impl Subscriber for RecordingSubscriber {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut visitor = FieldVisitor::default();
            event.record(&mut visitor);
            self.0
                .lock()
                .unwrap()
                .push((*event.metadata().level(), visitor.0));
        }

        fn enter(&self, _span: &Id) {}
        fn exit(&self, _span: &Id) {}
    }

    /// Every call to `instance_opts` in this module's tests must run inside
    /// `capture`, even when the returned events are discarded (as in
    /// `instance_opts_falls_back_and_reports_a_type_error`). `tracing`
    /// registers a callsite's cached interest once per process, and that
    /// registration races a concurrent `Dispatch::new` (which `capture`
    /// triggers via `with_default`): firing `instance_opts::<CpuOpts>`'s
    /// `tracing::warn!` under the ambient no-op dispatcher can lose that race
    /// and leave the callsite cached as uninteresting, so a *later* call
    /// under a real subscriber sees nothing. Measured on this callsite before
    /// this fix: ~9.5% of filtered `instance_opts` runs and ~0.7% of whole
    /// `rustline-core` binary runs failed under `cargo test`'s default
    /// multi-threaded harness (57/600 and 2/300 respectively, launched
    /// 16-way concurrently to reproduce the race reliably).
    fn capture(f: impl FnOnce()) -> Vec<CapturedEvent> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscriber = RecordingSubscriber(events.clone());
        tracing::subscriber::with_default(subscriber, f);
        events.lock().unwrap().clone()
    }

    fn field<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
        fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}
