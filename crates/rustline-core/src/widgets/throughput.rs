use crate::widgets::memory::format_bytes;
use crate::{Context, RangeName, Segment, Widget, WidgetName};

/// Renders network throughput (download/upload bytes-per-second), read from
/// `Context.throughputs` keyed by this instance's own `iface_key` (W46 —
/// multiple `throughput` widget instances pinned to different interfaces
/// each read their own entry rather than sharing `Context.throughput`).
/// Pure — reads only that map. `{down}`/`{up}` are human-readable binary
/// sizes (via `memory.rs`'s `format_bytes`) suffixed `/s`, e.g. `1.2M/s`.
/// Not threshold-aware (no `alert.rs` use, unlike
/// `cpu`/`memory`/`battery`/`loadavg`/`disk`) — a throughput rate has no
/// universally "unhealthy" ceiling the way a percentage does.
///
/// Named `ThroughputWidget` (not bare `Throughput`) to avoid colliding with
/// the `rustline_abi::Throughput` data type carried on `Context.throughput`,
/// mirroring `DiskWidget`/`MemoryWidget`/`BatteryWidget`/`GitWidget`'s
/// suffix over their own same-named `*Info` data types.
pub struct ThroughputWidget {
    /// Registry/layout name; the toggle key threaded through render + click,
    /// and this instance's range name (invariant #7).
    pub name: WidgetName,
    pub format: String,
    pub alt_format: String,
    pub down_format: String,
    /// The configured interface, or `""` to aggregate every non-loopback
    /// interface — the key this instance looks up in `Context.throughputs`.
    pub iface_key: String,
}

impl Widget for ThroughputWidget {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        match ctx.throughputs.get(&self.iface_key) {
            Some(t) => {
                let fmt =
                    crate::widgets::active_format(ctx, &self.name, &self.format, &self.alt_format);
                let text = fmt
                    .replace(
                        "{down}",
                        &format!("{}/s", format_bytes(t.down_bytes_per_sec)),
                    )
                    .replace("{up}", &format!("{}/s", format_bytes(t.up_bytes_per_sec)));
                vec![Segment::new(text)]
            }
            None => {
                if self.down_format.is_empty() {
                    return vec![];
                }
                // Collapse the placeholders so a stray token never renders
                // (invariant #6).
                let text = self.down_format.replace("{down}", "").replace("{up}", "");
                vec![Segment::new(text)]
            }
        }
    }

    fn range_name(&self) -> Option<RangeName> {
        crate::widgets::clickable_range(&self.name, &self.alt_format)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{Context, Throughput, Widget};
    use chrono::{Local, TimeZone};

    /// Builds a `Context` with `throughput` populated under the `""`
    /// (aggregate) key of `throughputs` — the key every test widget below
    /// uses via `w()`'s default `iface_key: ""` (see
    /// `ThroughputWidget::render`'s `ctx.throughputs.get(&self.iface_key)`
    /// lookup, W46).
    fn ctx(throughput: Option<Throughput>) -> Context {
        let mut c = Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/".into(),
            home: "/h".into(),
            hostname: "h".into(),
            loadavg: None,
            now: Local
                .with_ymd_and_hms(2026, 7, 23, 12, 0, 0)
                .single()
                .unwrap(),
            window: None,
            interfaces: Vec::new(),
            battery: None,
            cpu: None,
            memory: None,
            git: None,
            disk: None,
            disks: BTreeMap::new(),
            throughput: None,
            throughputs: BTreeMap::new(),
            os: String::new(),
            arch: String::new(),
            uptime: None,
            media: None,
            toggled: Default::default(),
            cpu_history: Vec::new(),
            mem_history: Vec::new(),
            colors: Default::default(),
        };
        if let Some(t) = throughput {
            c.throughputs.insert(String::new(), t);
        }
        c
    }

    fn rate(down: u64, up: u64) -> Option<Throughput> {
        Some(Throughput {
            down_bytes_per_sec: down,
            up_bytes_per_sec: up,
        })
    }

    fn w(format: &str, down: &str) -> ThroughputWidget {
        ThroughputWidget {
            name: "throughput".into(),
            format: format.into(),
            alt_format: String::new(),
            down_format: down.into(),
            iface_key: String::new(),
        }
    }

    #[test]
    fn renders_down_and_up_as_human_readable_rates() {
        let g = 1024u64.pow(3);
        let out = w("{down} {up}", "").render(&ctx(rate(g, 512 * 1024 * 1024)));
        assert_eq!(out[0].text, "1.0G/s 512M/s");
    }

    #[test]
    fn renders_small_rates_in_bytes() {
        let out = w("down={down}", "").render(&ctx(rate(0, 42)));
        assert_eq!(out[0].text, "down=0B/s");
    }

    #[test]
    fn none_empty_down_skips() {
        assert!(w("{down}", "").render(&ctx(None)).is_empty());
    }

    #[test]
    fn none_down_format_collapses_placeholders() {
        let out = w("{down}", "n/a {down}{up}").render(&ctx(None));
        assert_eq!(out[0].text, "n/a ");
    }

    #[test]
    fn throughput_toggled_uses_alt_format() {
        let mut c = ctx(rate(1024, 2048));
        c.toggled.insert(WidgetName::from("throughput"));
        let out = ThroughputWidget {
            name: "throughput".into(),
            format: "{down}".into(),
            alt_format: "{down}/{up}".into(),
            down_format: String::new(),
            iface_key: String::new(),
        }
        .render(&c);
        assert_eq!(out[0].text, "1.0K/s/2.0K/s");
    }

    #[test]
    fn throughput_widget_reads_its_interface_from_throughputs_map() {
        let mut c = Context::default();
        c.throughputs.insert(
            "eth0".to_string(),
            Throughput {
                down_bytes_per_sec: 1024,
                up_bytes_per_sec: 2048,
            },
        );
        let widget = ThroughputWidget {
            iface_key: "eth0".into(),
            ..w("{down} {up}", "")
        };
        let out = widget.render(&c);
        assert_eq!(out[0].text, "1.0K/s 2.0K/s");
        // A different-keyed instance sees nothing for "eth0"'s data.
        let other = ThroughputWidget {
            iface_key: "wlan0".into(),
            ..w("{down} {up}", "")
        };
        assert!(other.render(&c).is_empty());
    }

    #[test]
    fn throughput_range_name_tracks_alt() {
        let base = w("x", "");
        assert_eq!(base.range_name(), None);
        let mut alt = w("x", "");
        alt.alt_format = "{down}".into();
        assert_eq!(
            alt.range_name(),
            Some(RangeName::parse("throughput").unwrap())
        );
    }
}
