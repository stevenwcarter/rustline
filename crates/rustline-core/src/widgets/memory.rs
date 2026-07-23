use crate::widgets::bar;
use crate::{Context, Segment, Widget};

/// Nerd-Font memory/RAM glyph (nf-md-memory 󰍛).
const MEMORY_ICON: &str = "\u{f035b}";

/// Renders memory usage from `Context::memory`. Pure — reads only that field.
pub struct MemoryWidget {
    pub format: String,
    pub alt_format: String,
    pub down_format: String,
    pub bar_width: usize,
    pub warn_percent: f64,
    pub crit_percent: f64,
    /// Overrides `{icon}` with a fixed glyph instead of [`MEMORY_ICON`].
    /// `None` keeps the built-in glyph.
    pub icon: Option<String>,
}

impl MemoryWidget {
    /// Registry/layout name; the toggle key threaded through render + click.
    pub const NAME: &'static str = "memory";
}

/// Human-readable binary size (1024-based): the largest of `B/K/M/G/T` where the
/// scaled value is `>= 1`, one decimal below 10 and none at/above 10 (bytes are
/// always integer). E.g. `6.2 GiB -> "6.2G"`, `512 MiB -> "512M"`, `0 -> "0B"`.
/// `pub(crate)` so `disk.rs` reuses it rather than duplicating the logic.
pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes}B")
    } else if value < 10.0 {
        format!("{value:.1}{}", UNITS[unit])
    } else {
        format!("{value:.0}{}", UNITS[unit])
    }
}

impl Widget for MemoryWidget {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        match ctx.memory {
            Some(m) => {
                let fraction = if m.total_bytes == 0 {
                    0.0
                } else {
                    m.used_bytes as f64 / m.total_bytes as f64
                };
                let percent = (fraction * 100.0).round() as u64;
                let fmt =
                    crate::widgets::active_format(ctx, Self::NAME, &self.format, &self.alt_format);
                let text = fmt
                    .replace("{used}", &format_bytes(m.used_bytes))
                    .replace("{total}", &format_bytes(m.total_bytes))
                    .replace("{avail}", &format_bytes(m.available_bytes))
                    .replace("{percent}", &percent.to_string())
                    .replace("{bar}", &bar::gauge_bar(fraction, self.bar_width))
                    .replace("{icon}", self.icon.as_deref().unwrap_or(MEMORY_ICON));
                let kind = crate::widgets::alert_over(
                    fraction * 100.0,
                    self.warn_percent,
                    self.crit_percent,
                );
                match crate::widgets::alert_style(kind, &ctx.colors) {
                    Some(style) => vec![Segment::styled(text, style)],
                    None => vec![Segment::new(text)],
                }
            }
            None => {
                if self.down_format.is_empty() {
                    return vec![];
                }
                let text = self
                    .down_format
                    .replace("{used}", "")
                    .replace("{total}", "")
                    .replace("{avail}", "")
                    .replace("{percent}", "")
                    .replace("{bar}", "")
                    .replace("{icon}", "");
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
    use crate::{Context, MemInfo, Widget};
    use chrono::{Local, TimeZone};

    fn ctx(memory: Option<MemInfo>) -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/".into(),
            home: "/h".into(),
            hostname: "h".into(),
            loadavg: None,
            now: Local
                .with_ymd_and_hms(2026, 7, 21, 12, 0, 0)
                .single()
                .unwrap(),
            window: None,
            interfaces: Vec::new(),
            battery: None,
            cpu: None,
            memory,
            git: None,
            disk: None,
            throughput: None,
            os: String::new(),
            arch: String::new(),
            uptime: None,
            media: None,
            toggled: Default::default(),
            colors: Default::default(),
        }
    }

    fn mem(total: u64, used: u64, avail: u64) -> Option<MemInfo> {
        Some(MemInfo {
            total_bytes: total,
            used_bytes: used,
            available_bytes: avail,
        })
    }

    fn w(format: &str, down: &str) -> MemoryWidget {
        MemoryWidget {
            format: format.into(),
            alt_format: String::new(),
            down_format: down.into(),
            bar_width: 8,
            warn_percent: 80.0,
            crit_percent: 92.0,
            icon: None,
        }
    }

    #[test]
    fn format_bytes_humanizes() {
        assert_eq!(format_bytes(16 * 1024u64.pow(3)), "16G");
        assert_eq!(format_bytes((6.2 * 1024f64.powi(3)) as u64), "6.2G");
        assert_eq!(format_bytes(512 * 1024u64.pow(2)), "512M");
        assert_eq!(format_bytes(1536 * 1024u64.pow(2)), "1.5G");
        assert_eq!(format_bytes(0), "0B");
        assert_eq!(format_bytes(512), "512B"); // < 1 KiB stays in bytes
        assert_eq!(format_bytes(3 * 1024u64.pow(4)), "3.0T"); // TiB unit
    }

    #[test]
    fn renders_used_total_percent() {
        let g = 1024u64.pow(3);
        let out =
            w("{used}/{total} {avail} {percent}%", "").render(&ctx(mem(16 * g, 6 * g, 10 * g)));
        assert_eq!(out[0].text, "6.0G/16G 10G 38%"); // avail 10 GiB -> "10G"; 6/16 = 37.5 -> 38
    }

    #[test]
    fn renders_bar_and_icon() {
        let g = 1024u64.pow(3);
        let out = w("{icon} {bar}", "").render(&ctx(mem(16 * g, 8 * g, 8 * g)));
        // 8/16 = 0.5 over width 8 -> "████░░░░", icon prefixed
        assert_eq!(out[0].text, "\u{f035b} ████░░░░");
    }

    #[test]
    fn zero_total_does_not_divide_by_zero() {
        let out = w("{percent}% {bar}", "").render(&ctx(mem(0, 0, 0)));
        assert_eq!(out[0].text, "0% ░░░░░░░░");
    }

    #[test]
    fn none_empty_down_skips() {
        assert!(w("{used}", "").render(&ctx(None)).is_empty());
    }

    #[test]
    fn none_down_format_collapses_placeholders() {
        let out = w("{used}", "n/a {used}{total}{avail}{bar}{percent}{icon}").render(&ctx(None));
        assert_eq!(out[0].text, "n/a ");
    }

    #[test]
    fn memory_toggled_uses_alt_format() {
        let g = 1024u64.pow(3);
        let mut c = ctx(mem(16 * g, 8 * g, 8 * g));
        c.toggled.insert("memory".to_string());
        let out = MemoryWidget {
            format: "{percent}%".into(),
            alt_format: "{icon} {bar}".into(),
            down_format: String::new(),
            bar_width: 8,
            warn_percent: 80.0,
            crit_percent: 92.0,
            icon: None,
        }
        .render(&c);
        assert_eq!(out[0].text, "\u{f035b} ████░░░░");
    }

    #[test]
    fn memory_range_name_tracks_alt() {
        let base = MemoryWidget {
            format: "x".into(),
            alt_format: String::new(),
            down_format: String::new(),
            bar_width: 8,
            warn_percent: 80.0,
            crit_percent: 92.0,
            icon: None,
        };
        assert_eq!(base.range_name(), None);
        let alt = MemoryWidget {
            alt_format: "{bar}".into(),
            format: "x".into(),
            down_format: String::new(),
            bar_width: 8,
            warn_percent: 80.0,
            crit_percent: 92.0,
            icon: None,
        };
        assert_eq!(alt.range_name(), Some("memory"));
    }

    #[test]
    fn memory_icon_override_replaces_glyph() {
        let g = 1024u64.pow(3);
        let mut widget = w("{icon} {percent}%", "");
        widget.icon = Some("M".into());
        let out = widget.render(&ctx(mem(16 * g, 8 * g, 8 * g)));
        assert_eq!(out[0].text, "M 50%");
    }

    #[test]
    fn memory_icon_none_uses_default() {
        // Characterization: an unset icon renders the built-in glyph unchanged.
        let g = 1024u64.pow(3);
        let out = w("{icon} {percent}%", "").render(&ctx(mem(16 * g, 8 * g, 8 * g)));
        assert_eq!(out[0].text, "\u{f035b} 50%");
    }

    #[test]
    fn below_threshold_plain_over_threshold_badge() {
        let g = 1024u64.pow(3);
        // 8/16 = 50% -> plain
        let out = w("{percent}%", "").render(&ctx(mem(16 * g, 8 * g, 8 * g)));
        assert_eq!(out[0].style, crate::Style::default());
        // 15/16 ~= 94% -> crit
        let mut c = ctx(mem(16 * g, 15 * g, g));
        c.colors = crate::ThemeColors {
            error: crate::Color::Indexed(196),
            warning: crate::Color::Indexed(214),
            bar_bg: crate::Color::Indexed(234),
            ..Default::default()
        };
        let out = w("{percent}%", "").render(&c);
        assert_eq!(out[0].style.bg, Some(crate::Color::Indexed(196)));
        assert!(out[0].style.bold);
    }
}
