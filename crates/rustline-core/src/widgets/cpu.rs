use crate::widgets::bar;
use crate::widgets::spark::sparkline;
use crate::{Context, RangeName, Segment, Widget, WidgetName};

/// Nerd-Font CPU/chip glyph (nf-md-chip 󰘚).
const CPU_ICON: &str = "\u{f061a}";

/// Renders CPU utilization from `Context::cpu`. Pure — reads only that field.
pub struct CpuWidget {
    /// Registry/layout name; the toggle key threaded through render + click,
    /// and this instance's range name (invariant #7).
    pub name: WidgetName,
    pub format: String,
    pub alt_format: String,
    pub down_format: String,
    pub bar_width: usize,
    /// Rendered cell count for `{spark}`. Fixed rather than derived from the
    /// history length, so the widget is the same width on the first tick as on
    /// the hundredth — the ring starts empty and fills one entry per refresh.
    pub spark_width: usize,
    pub warn_percent: f64,
    pub crit_percent: f64,
    /// Overrides `{icon}` with a fixed glyph instead of [`CPU_ICON`]. `None`
    /// keeps the built-in glyph.
    pub icon: Option<String>,
}

impl Widget for CpuWidget {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        match ctx.cpu {
            Some(c) => {
                let percent = c.percent.round() as u64;
                let fmt =
                    crate::widgets::active_format(ctx, &self.name, &self.format, &self.alt_format);
                let text = fmt
                    .replace("{percent}", &crate::widgets::pad_percent(percent))
                    .replace(
                        "{bar}",
                        &bar::gauge_bar(c.percent as f64 / 100.0, self.bar_width),
                    )
                    .replace(
                        "{spark}",
                        &sparkline(&ctx.cpu_history, 100.0, self.spark_width),
                    )
                    .replace("{icon}", self.icon.as_deref().unwrap_or(CPU_ICON));
                let kind = crate::widgets::alert_over(
                    c.percent as f64,
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
                    .replace("{percent}", "")
                    .replace("{bar}", "")
                    .replace("{spark}", "")
                    .replace("{icon}", "");
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
            git: None,
            disk: None,
            disks: Default::default(),
            throughput: None,
            throughputs: Default::default(),
            os: String::new(),
            arch: String::new(),
            uptime: None,
            media: None,
            toggled: Default::default(),
            cpu_history: Vec::new(),
            mem_history: Vec::new(),
            colors: Default::default(),
        }
    }

    fn w(format: &str, down: &str) -> CpuWidget {
        CpuWidget {
            name: "cpu".into(),
            format: format.into(),
            alt_format: String::new(),
            down_format: down.into(),
            bar_width: 8,
            spark_width: 8,
            warn_percent: 80.0,
            crit_percent: 95.0,
            icon: None,
        }
    }

    fn w2(format: &str, alt: &str, down: &str) -> CpuWidget {
        CpuWidget {
            name: "cpu".into(),
            format: format.into(),
            alt_format: alt.into(),
            down_format: down.into(),
            bar_width: 8,
            spark_width: 8,
            warn_percent: 80.0,
            crit_percent: 95.0,
            icon: None,
        }
    }

    #[test]
    fn toggled_uses_alt_format() {
        let mut c = ctx(Some(CpuUsage { percent: 50.0 }));
        c.toggled.insert(WidgetName::from("cpu"));
        let out = w2("{percent}%", "{icon} {bar} {percent}%", "").render(&c);
        assert_eq!(out[0].text, "\u{f061a} ████░░░░  50%");
        // untoggled -> normal format
        let out = w2("{percent}%", "{icon} {bar} {percent}%", "")
            .render(&ctx(Some(CpuUsage { percent: 50.0 })));
        assert_eq!(out[0].text, " 50%");
    }

    #[test]
    fn range_name_some_only_with_alt_format() {
        assert_eq!(
            w2("{percent}%", "{bar}", "").range_name(),
            Some(RangeName::parse("cpu").unwrap())
        );
        assert_eq!(w2("{percent}%", "", "").range_name(), None);
    }

    #[test]
    fn renders_percent_rounded() {
        let out = w("{percent}%", "").render(&ctx(Some(CpuUsage { percent: 37.4 })));
        assert_eq!(out[0].text, " 37%");
        let out = w("{percent}%", "").render(&ctx(Some(CpuUsage { percent: 37.6 })));
        assert_eq!(out[0].text, " 38%");
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

    #[test]
    fn below_threshold_is_plain_segment() {
        // Characterization: no alert -> default (unstyled) segment, as before.
        let out = w("{percent}%", "").render(&ctx(Some(CpuUsage { percent: 50.0 })));
        assert_eq!(out[0].text, " 50%");
        assert_eq!(out[0].style, crate::Style::default());
    }

    #[test]
    fn warn_and_crit_apply_badge_style() {
        let mut c = ctx(Some(CpuUsage { percent: 85.0 }));
        c.colors = crate::ThemeColors {
            warning: crate::Color::Indexed(214),
            error: crate::Color::Indexed(196),
            bar_bg: crate::Color::Indexed(234),
            ..Default::default()
        };
        let out = w("{percent}%", "").render(&c);
        assert_eq!(out[0].style.bg, Some(crate::Color::Indexed(214))); // warn
        assert_eq!(out[0].style.fg, Some(crate::Color::Indexed(234)));
        assert!(out[0].style.bold);

        c.cpu = Some(CpuUsage { percent: 96.0 });
        let out = w("{percent}%", "").render(&c);
        assert_eq!(out[0].style.bg, Some(crate::Color::Indexed(196))); // crit
    }

    #[test]
    fn cpu_icon_override_replaces_glyph() {
        let mut widget = w("{icon} {percent}%", "");
        widget.icon = Some("C".into());
        let out = widget.render(&ctx(Some(CpuUsage { percent: 50.0 })));
        assert_eq!(out[0].text, "C  50%");
    }

    #[test]
    fn cpu_icon_none_uses_default() {
        // Characterization: an unset icon renders the built-in glyph unchanged.
        let out = w("{icon} {percent}%", "").render(&ctx(Some(CpuUsage { percent: 50.0 })));
        assert_eq!(out[0].text, "\u{f061a}  50%");
    }

    #[test]
    fn thresholds_disabled_never_alert() {
        let mut c = ctx(Some(CpuUsage { percent: 100.0 }));
        c.colors = crate::ThemeColors::default();
        let mut widget = w("{percent}%", "");
        widget.warn_percent = 0.0;
        widget.crit_percent = 0.0;
        let out = widget.render(&c);
        assert_eq!(out[0].style, crate::Style::default());
    }

    #[test]
    fn spark_placeholder_renders_history_sparkline() {
        let mut c = ctx(Some(CpuUsage { percent: 50.0 }));
        c.cpu_history = vec![0.0, 50.0, 100.0];
        let out = w("{spark}", "").render(&c);
        // 3 readings in an 8-cell spark: left-padded, newest at the right.
        assert_eq!(out[0].text, "     ▁▅█");
    }

    #[test]
    fn spark_placeholder_empty_history_renders_full_width_padding() {
        // The ring starts empty and gains one entry per refresh, so on the
        // first tick {spark} has nothing to draw. It still occupies its full
        // spark_width in spaces rather than collapsing to "" -- otherwise the
        // widget grows a cell per tick and shifts every segment beside it.
        // Spaces, not ▁, because a zero glyph would claim a 0% reading
        // that was never taken (invariant #6).
        let out = w("{spark}", "").render(&ctx(Some(CpuUsage { percent: 50.0 })));
        assert_eq!(out[0].text, "        ");
    }

    /// The seam this change exists for: the rendered segment is the same
    /// width on tick 1 as on tick 8, measured on the segment text rather than
    /// on `sparkline` alone.
    #[test]
    fn rendered_spark_width_is_constant_while_the_ring_fills() {
        let widths: Vec<usize> = (0..=8)
            .map(|n| {
                let mut c = ctx(Some(CpuUsage { percent: 50.0 }));
                c.cpu_history = vec![42.0; n];
                w("{spark}", "").render(&c)[0].text.chars().count()
            })
            .collect();
        assert!(
            widths.windows(2).all(|p| p[0] == p[1]),
            "cpu spark width jitters as the ring fills: {widths:?}"
        );
        assert_eq!(widths[0], 8);
    }

    #[test]
    fn spark_absent_from_format_is_byte_identical_regardless_of_history() {
        // Characterization (W45): a format that never references {spark}
        // renders identically whether or not Context.cpu_history happens to
        // be populated -- the bin only populates it when {spark} is used, but
        // the widget itself must not depend on that invariant either.
        let mut c = ctx(Some(CpuUsage { percent: 37.4 }));
        let without_history = w("{icon} {percent}%", "").render(&c);
        c.cpu_history = vec![10.0, 20.0, 30.0];
        let with_history = w("{icon} {percent}%", "").render(&c);
        assert_eq!(without_history[0].text, with_history[0].text);
        assert_eq!(with_history[0].text, "\u{f061a}  37%");
    }

    /// Constant rendered width across the 1-, 2-, and 3-digit percentages,
    /// asserted on the segment text so a future render-pipeline change that
    /// trimmed padding would fail here rather than silently reintroduce the
    /// jitter.
    #[test]
    fn rendered_percent_width_is_constant() {
        let widths: Vec<usize> = [0.0f32, 7.0, 42.0, 100.0]
            .iter()
            .map(|&percent| {
                let out = w("{percent}%", "").render(&ctx(Some(CpuUsage { percent })));
                out[0].text.chars().count()
            })
            .collect();
        assert!(
            widths.windows(2).all(|p| p[0] == p[1]),
            "cpu render width jitters: {widths:?}"
        );
        assert_eq!(widths[0], 4); // 3-wide numeral + "%"
    }
}
