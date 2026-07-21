use crate::widgets::net;
use crate::{Context, Segment, Widget};

/// Renders the machine's LAN IPv4, selected from `Context.interfaces`.
pub struct LanIp {
    pub format: String,
    pub down_format: String,
    pub interface: Option<String>,
}

impl Widget for LanIp {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let ip = net::pick_lan(&ctx.interfaces, self.interface.as_deref());
        net::render_ip(&self.format, ip, &self.down_format)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NetIface;
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
        }
    }

    fn ifc(name: &str, ip: &str) -> NetIface {
        NetIface {
            name: name.into(),
            ipv4: ip.parse().unwrap(),
        }
    }

    #[test]
    fn renders_lan_ip_with_label() {
        let w = LanIp {
            format: "LAN {ip}".into(),
            down_format: String::new(),
            interface: None,
        };
        let out = w.render(&ctx(vec![ifc("eth0", "192.168.1.20")]));
        assert_eq!(out[0].text, "LAN 192.168.1.20");
    }

    #[test]
    fn no_lan_ip_and_empty_down_format_renders_nothing() {
        let w = LanIp {
            format: "{ip}".into(),
            down_format: String::new(),
            interface: None,
        };
        assert!(w.render(&ctx(vec![])).is_empty());
    }

    #[test]
    fn no_lan_ip_with_down_format_renders_it() {
        let w = LanIp {
            format: "{ip}".into(),
            down_format: "no-lan".into(),
            interface: None,
        };
        assert_eq!(w.render(&ctx(vec![]))[0].text, "no-lan");
    }

    #[test]
    fn interface_override_honored() {
        let w = LanIp {
            format: "{ip}".into(),
            down_format: String::new(),
            interface: Some("docker0".into()),
        };
        let out = w.render(&ctx(vec![
            ifc("eth0", "192.168.1.20"),
            ifc("docker0", "172.17.0.1"),
        ]));
        assert_eq!(out[0].text, "172.17.0.1");
    }
}
