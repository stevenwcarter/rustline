use crate::widgets::net;
use crate::{Context, Segment, Widget};

/// Renders the machine's Tailscale IPv4 (the `100.64.0.0/10` address).
pub struct TailscaleIp {
    pub format: String,
    pub down_format: String,
}

impl Widget for TailscaleIp {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let ip = net::pick_tailscale(&ctx.interfaces);
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
    fn renders_tailscale_ip() {
        let w = TailscaleIp {
            format: "TS {ip}".into(),
            down_format: "TS off".into(),
        };
        let out = w.render(&ctx(vec![ifc("tailscale0", "100.101.4.7")]));
        assert_eq!(out[0].text, "TS 100.101.4.7");
    }

    #[test]
    fn down_format_when_tailscale_absent() {
        let w = TailscaleIp {
            format: "TS {ip}".into(),
            down_format: "TS off".into(),
        };
        assert_eq!(
            w.render(&ctx(vec![ifc("eth0", "192.168.1.20")]))[0].text,
            "TS off"
        );
    }

    #[test]
    fn empty_down_format_renders_nothing() {
        let w = TailscaleIp {
            format: "TS {ip}".into(),
            down_format: String::new(),
        };
        assert!(w.render(&ctx(vec![ifc("eth0", "192.168.1.20")])).is_empty());
    }
}
