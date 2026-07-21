//! Pure selection + formatting logic for the IP widgets: which interface is
//! "the LAN", which is Tailscale, and how an address (or its absence) renders.
//! No I/O — operates entirely on the `Context.interfaces` snapshot.
//!
//! These `pub(crate)` helpers aren't called by production code yet — the
//! `LanIp`/`TailscaleIp` widgets that wire them in land in the very next task
//! of this feature's SDD sequence. Until then they're reachable only from
//! `tests`, which rustc's dead_code analysis doesn't count as "live".
#![allow(dead_code)]

use std::net::Ipv4Addr;

use crate::{NetIface, Segment};

/// True for Tailscale's `100.64.0.0/10` CGNAT range (RFC 6598). Used both to
/// detect the tailnet address and — implicitly, via `is_private` excluding it —
/// to keep it out of the LAN pick.
pub(crate) fn is_cgnat(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (64..=127).contains(&o[1])
}

/// True for interface names that carry private IPs but are never "the LAN":
/// container/VM bridges and the tailnet interface.
pub(crate) fn is_virtual_name(name: &str) -> bool {
    const VIRTUAL_PREFIXES: [&str; 5] = ["docker", "veth", "virbr", "br-", "tailscale"];
    VIRTUAL_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// The LAN IPv4. An explicit `interface` override wins unconditionally (even a
/// virtual/public NIC). Otherwise the first RFC1918-private address on a
/// non-virtual interface. `is_private` (10/8, 172.16/12, 192.168/16) already
/// excludes the 100.64/10 CGNAT range, so a tailnet address is never the LAN.
pub(crate) fn pick_lan(ifaces: &[NetIface], interface: Option<&str>) -> Option<Ipv4Addr> {
    if let Some(name) = interface {
        return ifaces.iter().find(|i| i.name == name).map(|i| i.ipv4);
    }
    ifaces
        .iter()
        .find(|i| i.ipv4.is_private() && !is_virtual_name(&i.name))
        .map(|i| i.ipv4)
}

/// The Tailscale IPv4: the first interface whose address is in the CGNAT range.
pub(crate) fn pick_tailscale(ifaces: &[NetIface]) -> Option<Ipv4Addr> {
    ifaces.iter().find(|i| is_cgnat(i.ipv4)).map(|i| i.ipv4)
}

/// Render an address (or its absence) to segments. `Some` → `format` with
/// `{ip}` substituted. `None` → nothing when `down_format` is empty (invariant
/// #6), else `down_format` with any `{ip}` collapsed to empty (never a fake IP).
pub(crate) fn render_ip(format: &str, ip: Option<Ipv4Addr>, down_format: &str) -> Vec<Segment> {
    match ip {
        Some(ip) => vec![Segment::new(format.replace("{ip}", &ip.to_string()))],
        None if down_format.is_empty() => vec![],
        None => vec![Segment::new(down_format.replace("{ip}", ""))],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NetIface;

    fn ifc(name: &str, ip: &str) -> NetIface {
        NetIface {
            name: name.into(),
            ipv4: ip.parse().unwrap(),
        }
    }

    #[test]
    fn cgnat_range_boundaries() {
        assert!(!is_cgnat("100.63.255.255".parse().unwrap()));
        assert!(is_cgnat("100.64.0.0".parse().unwrap()));
        assert!(is_cgnat("100.101.4.7".parse().unwrap()));
        assert!(is_cgnat("100.127.255.255".parse().unwrap()));
        assert!(!is_cgnat("100.128.0.0".parse().unwrap()));
        assert!(!is_cgnat("192.168.1.1".parse().unwrap()));
    }

    #[test]
    fn virtual_names_detected() {
        for n in ["docker0", "veth123", "virbr0", "br-abc", "tailscale0"] {
            assert!(is_virtual_name(n), "{n} should be virtual");
        }
        for n in ["eth0", "enp3s0", "wlp2s0", "wlan0"] {
            assert!(!is_virtual_name(n), "{n} should be real");
        }
    }

    #[test]
    fn lan_skips_docker_and_tailscale_picks_real_nic() {
        let ifaces = [
            ifc("docker0", "172.17.0.1"),
            ifc("tailscale0", "100.101.4.7"),
            ifc("eth0", "192.168.1.20"),
        ];
        assert_eq!(
            pick_lan(&ifaces, None),
            Some("192.168.1.20".parse().unwrap())
        );
    }

    #[test]
    fn lan_override_forces_named_nic_even_if_virtual() {
        let ifaces = [ifc("eth0", "192.168.1.20"), ifc("docker0", "172.17.0.1")];
        assert_eq!(
            pick_lan(&ifaces, Some("docker0")),
            Some("172.17.0.1".parse().unwrap())
        );
        // override naming a NIC that isn't present -> None
        assert_eq!(pick_lan(&ifaces, Some("wlp9s0")), None);
    }

    #[test]
    fn lan_none_when_only_public_or_cgnat() {
        let ifaces = [ifc("eth0", "100.101.4.7"), ifc("eth1", "8.8.8.8")];
        assert_eq!(pick_lan(&ifaces, None), None);
    }

    #[test]
    fn tailscale_finds_cgnat_else_none() {
        let with = [
            ifc("eth0", "192.168.1.20"),
            ifc("tailscale0", "100.101.4.7"),
        ];
        assert_eq!(pick_tailscale(&with), Some("100.101.4.7".parse().unwrap()));
        let without = [ifc("eth0", "192.168.1.20")];
        assert_eq!(pick_tailscale(&without), None);
    }

    #[test]
    fn render_substitutes_ip_and_preserves_label() {
        let out = render_ip("LAN {ip}", Some("192.168.1.20".parse().unwrap()), "");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "LAN 192.168.1.20");
    }

    #[test]
    fn render_none_empty_down_format_is_skipped() {
        assert!(render_ip("{ip}", None, "").is_empty());
    }

    #[test]
    fn render_none_with_down_format_shows_it_and_collapses_ip_token() {
        let out = render_ip("TS {ip}", None, "TS off {ip}");
        assert_eq!(out[0].text, "TS off ");
    }
}
