use crate::widgets::net;
use crate::{Context, Segment, Widget};

/// Renders the machine's Tailscale IPv4 (the `100.64.0.0/10` address).
pub struct TailscaleIp {
    pub format: String,
    pub alt_format: String,
    pub down_format: String,
}

impl TailscaleIp {
    /// Registry/layout name; the toggle key threaded through render + click.
    pub const NAME: &'static str = "tailscale_ip";
}

impl Widget for TailscaleIp {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let ip = net::pick_tailscale(&ctx.interfaces);
        let fmt = crate::widgets::active_format(ctx, Self::NAME, &self.format, &self.alt_format);
        net::render_ip(fmt, ip, &self.down_format)
    }

    fn range_name(&self) -> Option<&str> {
        crate::widgets::clickable_range(Self::NAME, &self.alt_format)
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
            battery: None,
            cpu: None,
            memory: None,
            git: None,
            disk: None,
            os: String::new(),
            arch: String::new(),
            uptime: None,
            toggled: Default::default(),
            colors: Default::default(),
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
            alt_format: String::new(),
            down_format: "TS off".into(),
        };
        let out = w.render(&ctx(vec![ifc("tailscale0", "100.101.4.7")]));
        assert_eq!(out[0].text, "TS 100.101.4.7");
    }

    #[test]
    fn down_format_when_tailscale_absent() {
        let w = TailscaleIp {
            format: "TS {ip}".into(),
            alt_format: String::new(),
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
            alt_format: String::new(),
            down_format: String::new(),
        };
        assert!(w.render(&ctx(vec![ifc("eth0", "192.168.1.20")])).is_empty());
    }

    #[test]
    fn tailscale_toggled_uses_alt_format() {
        let mut c = ctx(vec![ifc("tailscale0", "100.101.4.7")]);
        c.toggled.insert("tailscale_ip".to_string());
        let w = TailscaleIp {
            format: "{ip}".into(),
            alt_format: "TS {ip}".into(),
            down_format: String::new(),
        };
        assert_eq!(w.render(&c)[0].text, "TS 100.101.4.7");
    }
}
