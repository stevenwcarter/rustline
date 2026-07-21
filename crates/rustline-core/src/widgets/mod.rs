// `gauge_bar` has no production caller yet — the `cpu`/`memory` widgets that
// consume it land in Tasks 3-4 of
// docs/superpowers/plans/2026-07-21-rustline-cpu-memory-widgets.md. Remove
// this allow once either widget calls into `bar::gauge_bar`.
#[allow(dead_code)]
mod bar;
pub mod battery;
pub mod cwd;
pub mod datetime;
pub mod hostname;
pub mod lan_ip;
pub mod loadavg;
mod net;
pub mod pane_id;
pub mod tailscale_ip;
pub mod windows;

pub use battery::BatteryWidget;
pub use cwd::Cwd;
pub use datetime::DateTime;
pub use hostname::Hostname;
pub use lan_ip::LanIp;
pub use loadavg::LoadAvg;
pub use pane_id::PaneId;
pub use tailscale_ip::TailscaleIp;
pub use windows::Windows;

use crate::Config;
use crate::widget::Registry;

impl Registry {
    /// Build a [`Registry`] pre-populated with all nine built-in widgets,
    /// configuring the ones with options (`datetime`, `cwd`) from `cfg`.
    pub fn with_builtins(cfg: &Config) -> Registry {
        let mut registry = Registry::new();
        registry.register("pane_id", Box::new(|| Box::new(PaneId)));
        registry.register("hostname", Box::new(|| Box::new(Hostname)));
        registry.register("windows", Box::new(|| Box::new(Windows)));
        registry.register("loadavg", Box::new(|| Box::new(LoadAvg)));

        let format = cfg.widgets.datetime.format.clone();
        registry.register(
            "datetime",
            Box::new(move || {
                Box::new(DateTime {
                    format: format.clone(),
                })
            }),
        );

        let abbreviate_home = cfg.widgets.cwd.abbreviate_home;
        registry.register("cwd", Box::new(move || Box::new(Cwd { abbreviate_home })));

        let lan = cfg.widgets.lan_ip.clone();
        registry.register(
            "lan_ip",
            Box::new(move || {
                Box::new(LanIp {
                    format: lan.format.clone(),
                    down_format: lan.down_format.clone(),
                    interface: lan.interface.clone(),
                })
            }),
        );

        let ts = cfg.widgets.tailscale_ip.clone();
        registry.register(
            "tailscale_ip",
            Box::new(move || {
                Box::new(TailscaleIp {
                    format: ts.format.clone(),
                    down_format: ts.down_format.clone(),
                })
            }),
        );

        let battery = cfg.widgets.battery.clone();
        registry.register(
            "battery",
            Box::new(move || {
                Box::new(BatteryWidget {
                    format: battery.format.clone(),
                    down_format: battery.down_format.clone(),
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
            os: String::new(),
            arch: String::new(),
        }
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
}
