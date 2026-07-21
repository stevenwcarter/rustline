use crate::widgets::bar;
use crate::{Context, Segment, Widget};

/// Nerd-Font CPU/chip glyph (nf-md-chip 󰘚).
const CPU_ICON: &str = "\u{f061a}";

/// Renders CPU utilization from `Context::cpu`. Pure — reads only that field.
pub struct CpuWidget {
    pub format: String,
    pub alt_format: String,
    pub down_format: String,
    pub bar_width: usize,
}

impl CpuWidget {
    /// Registry/layout name; the toggle key threaded through render + click.
    pub const NAME: &'static str = "cpu";
}

impl Widget for CpuWidget {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        match ctx.cpu {
            Some(c) => {
                let percent = c.percent.round() as u64;
                let fmt =
                    crate::widgets::active_format(ctx, Self::NAME, &self.format, &self.alt_format);
                let text = fmt
                    .replace("{percent}", &percent.to_string())
                    .replace(
                        "{bar}",
                        &bar::gauge_bar(c.percent as f64 / 100.0, self.bar_width),
                    )
                    .replace("{icon}", CPU_ICON);
                vec![Segment::new(text)]
            }
            None => {
                if self.down_format.is_empty() {
                    return vec![];
                }
                let text = self
                    .down_format
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
    use crate::{Context, CpuUsage, Widget};
    use chrono::{Local, TimeZone};

    fn ctx(cpu: Option<CpuUsage>) -> Context {
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
            cpu,
            memory: None,
            os: String::new(),
            arch: String::new(),
            toggled: Default::default(),
        }
    }

    fn w(format: &str, down: &str) -> CpuWidget {
        CpuWidget {
            format: format.into(),
            alt_format: String::new(),
            down_format: down.into(),
            bar_width: 8,
        }
    }

    fn w2(format: &str, alt: &str, down: &str) -> CpuWidget {
        CpuWidget {
            format: format.into(),
            alt_format: alt.into(),
            down_format: down.into(),
            bar_width: 8,
        }
    }

    #[test]
    fn toggled_uses_alt_format() {
        let mut c = ctx(Some(CpuUsage { percent: 50.0 }));
        c.toggled.insert("cpu".to_string());
        let out = w2("{percent}%", "{icon} {bar} {percent}%", "").render(&c);
        assert_eq!(out[0].text, "\u{f061a} ████░░░░ 50%");
        // untoggled -> normal format
        let out = w2("{percent}%", "{icon} {bar} {percent}%", "")
            .render(&ctx(Some(CpuUsage { percent: 50.0 })));
        assert_eq!(out[0].text, "50%");
    }

    #[test]
    fn range_name_some_only_with_alt_format() {
        assert_eq!(w2("{percent}%", "{bar}", "").range_name(), Some("cpu"));
        assert_eq!(w2("{percent}%", "", "").range_name(), None);
    }

    #[test]
    fn renders_percent_rounded() {
        let out = w("{percent}%", "").render(&ctx(Some(CpuUsage { percent: 37.4 })));
        assert_eq!(out[0].text, "37%");
        let out = w("{percent}%", "").render(&ctx(Some(CpuUsage { percent: 37.6 })));
        assert_eq!(out[0].text, "38%");
    }

    #[test]
    fn renders_bar_and_icon() {
        // 50% over width 8 -> "████░░░░"
        let out = w("{icon} {bar}", "").render(&ctx(Some(CpuUsage { percent: 50.0 })));
        assert_eq!(out[0].text, "\u{f061a} ████░░░░");
    }

    #[test]
    fn none_empty_down_skips() {
        assert!(w("{percent}%", "").render(&ctx(None)).is_empty());
    }

    #[test]
    fn none_down_format_collapses_placeholders() {
        let out = w("{percent}%", "cpu? {percent}{bar}{icon}").render(&ctx(None));
        assert_eq!(out[0].text, "cpu? ");
    }
}
