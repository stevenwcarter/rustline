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
pub mod memory;
mod net;
pub mod pane_id;
pub mod tailscale_ip;
mod toggle;
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
pub use memory::MemoryWidget;
pub use pane_id::PaneId;
pub use tailscale_ip::TailscaleIp;
// Re-exported for assemble.rs (Task 3) and the widgets' alt_format toggling
// (Task 4+) of the click-toggle plan.
pub(crate) use toggle::{active_format, clickable_range};
pub use windows::Windows;

// Re-exported for the numeric widgets (cpu/memory/battery/loadavg, Tasks
// 7-10) to render a threshold-alert badge.
pub(crate) use alert::{AlertKind, alert_over, alert_style, alert_under};

use crate::Config;
use crate::widget::{Registry, WidgetDescriptor, WidgetSource};

/// Build a minimal-boilerplate `WidgetDescriptor` for a built-in widget.
fn builtin_descriptor(name: &str, summary: &str, configurable: bool) -> WidgetDescriptor {
    WidgetDescriptor {
        name: name.to_string(),
        summary: summary.to_string(),
        configurable,
        source: WidgetSource::Builtin,
    }
}

impl Registry {
    /// Build a [`Registry`] pre-populated with all thirteen built-in widgets,
    /// configuring the ones that carry options (`pane_id`, `hostname`,
    /// `datetime`, `cwd`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`,
    /// `memory`, `loadavg`, `git`, `disk`) from `cfg`.
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
            Box::new(move || {
                Box::new(LoadAvg {
                    format: loadavg.format.clone(),
                    alt_format: loadavg.alt_format.clone(),
                    down_format: loadavg.down_format.clone(),
                    warn_load: loadavg.warn_load,
                    crit_load: loadavg.crit_load,
                })
            }),
        );

        let datetime = cfg.widgets.datetime.clone();
        registry.register_described(
            builtin_descriptor(
                "datetime",
                "The current time, `chrono` strftime-formatted",
                true,
            ),
            Box::new(move || {
                Box::new(DateTime {
                    format: datetime.format.clone(),
                    alt_format: datetime.alt_format.clone(),
                })
            }),
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
            Box::new(move || {
                Box::new(LanIp {
                    format: lan.format.clone(),
                    alt_format: lan.alt_format.clone(),
                    down_format: lan.down_format.clone(),
                    interface: lan.interface.clone(),
                })
            }),
        );

        let ts = cfg.widgets.tailscale_ip.clone();
        registry.register_described(
            builtin_descriptor("tailscale_ip", "The machine's Tailscale IPv4 address", true),
            Box::new(move || {
                Box::new(TailscaleIp {
                    format: ts.format.clone(),
                    alt_format: ts.alt_format.clone(),
                    down_format: ts.down_format.clone(),
                })
            }),
        );

        let battery = cfg.widgets.battery.clone();
        registry.register_described(
            builtin_descriptor(
                "battery",
                "Battery percentage, charge state, and level icon",
                true,
            ),
            Box::new(move || {
                Box::new(BatteryWidget {
                    format: battery.format.clone(),
                    alt_format: battery.alt_format.clone(),
                    down_format: battery.down_format.clone(),
                    warn_percent: battery.warn_percent,
                    crit_percent: battery.crit_percent,
                    icon: battery.icon.clone(),
                })
            }),
        );

        let cpu = cfg.widgets.cpu.clone();
        registry.register_described(
            builtin_descriptor("cpu", "CPU utilization, with an optional gauge bar", true),
            Box::new(move || {
                Box::new(CpuWidget {
                    format: cpu.format.clone(),
                    alt_format: cpu.alt_format.clone(),
                    down_format: cpu.down_format.clone(),
                    bar_width: cpu.bar_width,
                    warn_percent: cpu.warn_percent,
                    crit_percent: cpu.crit_percent,
                    icon: cpu.icon.clone(),
                })
            }),
        );

        let memory = cfg.widgets.memory.clone();
        registry.register_described(
            builtin_descriptor("memory", "Memory usage, with an optional gauge bar", true),
            Box::new(move || {
                Box::new(MemoryWidget {
                    format: memory.format.clone(),
                    alt_format: memory.alt_format.clone(),
                    down_format: memory.down_format.clone(),
                    bar_width: memory.bar_width,
                    warn_percent: memory.warn_percent,
                    crit_percent: memory.crit_percent,
                    icon: memory.icon.clone(),
                })
            }),
        );

        let git = cfg.widgets.git.clone();
        registry.register_described(
            builtin_descriptor(
                "git",
                "Current git branch, dirty marker, and ahead/behind counts",
                true,
            ),
            Box::new(move || {
                Box::new(GitWidget {
                    format: git.format.clone(),
                    alt_format: git.alt_format.clone(),
                    down_format: git.down_format.clone(),
                    dirty_glyph: git.dirty_glyph.clone(),
                })
            }),
        );

        let disk = cfg.widgets.disk.clone();
        registry.register_described(
            builtin_descriptor("disk", "Filesystem usage for a configured mount", true),
            Box::new(move || {
                Box::new(DiskWidget {
                    format: disk.format.clone(),
                    alt_format: disk.alt_format.clone(),
                    down_format: disk.down_format.clone(),
                    mount: disk.mount.clone(),
                    bar_width: disk.bar_width,
                    warn_percent: disk.warn_percent,
                    crit_percent: disk.crit_percent,
                })
            }),
        );

        registry
    }
}

#[cfg(test)]
mod tests {
    use crate::widget::Registry;
    use crate::{Config, Context, NetIface};
    use chrono::{Local, TimeZone};

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
            os: String::new(),
            arch: String::new(),
            toggled: Default::default(),
            colors: Default::default(),
        }
    }

    #[test]
    fn with_builtins_descriptors_cover_all_thirteen_with_correct_configurable_flags() {
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
        ] {
            assert!(names.contains(&expected), "missing descriptor: {expected}");
        }
        assert_eq!(names.len(), 13);

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
            .flat_map(|w| w.render(&c))
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
            .flat_map(|w| w.render(&c))
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
            .flat_map(|w| w.render(&ctx(vec![])))
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
            .flat_map(|w| w.render(&c))
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
            .flat_map(|w| w.render(&c0))
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
            .flat_map(|w| w.render(&c))
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
            .flat_map(|w| w.render(&c))
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
            .flat_map(|w| w.render(&c0))
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

        let mut c = ctx(Vec::new());
        let g = 1024u64.pow(3);
        c.disk = Some(DiskInfo {
            total_bytes: 16 * g,
            used_bytes: 6 * g,
            available_bytes: 10 * g,
        });
        let widgets = reg.resolve(&["disk".into()]);
        let texts: Vec<String> = widgets
            .iter()
            .flat_map(|w| w.render(&c))
            .map(|s| s.text)
            .collect();
        // default format " {used}/{total}".
        assert_eq!(texts, vec![" 6.0G/16G".to_string()]);

        // No disk info + default (empty) down_format -> widget skipped.
        let mut c0 = ctx(Vec::new());
        c0.disk = None;
        let widgets = reg.resolve(&["disk".into()]);
        let texts: Vec<String> = widgets
            .iter()
            .flat_map(|w| w.render(&c0))
            .map(|s| s.text)
            .collect();
        assert!(texts.is_empty());
    }
}
