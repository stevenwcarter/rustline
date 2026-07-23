use crate::widgets::memory::format_bytes;
use crate::{Context, Segment, Widget};

/// Renders network throughput (download/upload bytes-per-second) from
/// `Context::throughput`. Pure — reads only that field. `{down}`/`{up}` are
/// human-readable binary sizes (via `memory.rs`'s `format_bytes`) suffixed
/// `/s`, e.g. `1.2M/s`. Not threshold-aware (no `alert.rs` use, unlike
/// `cpu`/`memory`/`battery`/`loadavg`/`disk`) — a throughput rate has no
/// universally "unhealthy" ceiling the way a percentage does.
///
/// Named `ThroughputWidget` (not bare `Throughput`) to avoid colliding with
/// the `rustline_abi::Throughput` data type carried on `Context.throughput`,
/// mirroring `DiskWidget`/`MemoryWidget`/`BatteryWidget`/`GitWidget`'s
/// suffix over their own same-named `*Info` data types.
pub struct ThroughputWidget {
    pub format: String,
    pub alt_format: String,
    pub down_format: String,
}

impl ThroughputWidget {
    /// Registry/layout name; the toggle key threaded through render + click.
    pub const NAME: &'static str = "throughput";
}

impl Widget for ThroughputWidget {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        match &ctx.throughput {
            Some(t) => {
                let fmt =
                    crate::widgets::active_format(ctx, Self::NAME, &self.format, &self.alt_format);
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

    fn range_name(&self) -> Option<&str> {
        crate::widgets::clickable_range(Self::NAME, &self.alt_format)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Throughput, Widget};
    use chrono::{Local, TimeZone};

    fn ctx(throughput: Option<Throughput>) -> Context {
        Context {
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
            throughput,
            os: String::new(),
            arch: String::new(),
            uptime: None,
            media: None,
            toggled: Default::default(),
            colors: Default::default(),
        }
    }

    fn rate(down: u64, up: u64) -> Option<Throughput> {
        Some(Throughput {
            down_bytes_per_sec: down,
            up_bytes_per_sec: up,
        })
    }

    fn w(format: &str, down: &str) -> ThroughputWidget {
        ThroughputWidget {
            format: format.into(),
            alt_format: String::new(),
            down_format: down.into(),
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
        c.toggled.insert("throughput".to_string());
        let out = ThroughputWidget {
            format: "{down}".into(),
            alt_format: "{down}/{up}".into(),
            down_format: String::new(),
        }
        .render(&c);
        assert_eq!(out[0].text, "1.0K/s/2.0K/s");
    }

    #[test]
    fn throughput_range_name_tracks_alt() {
        let base = w("x", "");
        assert_eq!(base.range_name(), None);
        let mut alt = w("x", "");
        alt.alt_format = "{down}".into();
        assert_eq!(alt.range_name(), Some("throughput"));
    }
}
