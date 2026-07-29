use crate::widgets::bar;
use crate::widgets::memory::format_bytes;
use crate::{Context, RangeName, Segment, Widget, WidgetName};

/// Renders filesystem usage for a configured mount, read from
/// `Context.disks` keyed by this instance's own `mount` (W46 — multiple
/// `disk` widget instances with different mounts each read their own entry
/// rather than sharing `Context.disk`). Pure — reads only that map. Structured
/// like `MemoryWidget` (used/total/avail/percent/bar plus a warn/crit
/// threshold badge), minus an `{icon}` placeholder, plus a static `{mount}`
/// placeholder for the configured mount string (not read from `Context` —
/// it's widget config, not a live signal).
pub struct DiskWidget {
    /// Registry/layout name; the toggle key threaded through render + click,
    /// and this instance's range name (invariant #7).
    pub name: WidgetName,
    pub format: String,
    pub alt_format: String,
    pub down_format: String,
    pub mount: String,
    pub bar_width: usize,
    pub warn_percent: f64,
    pub crit_percent: f64,
}

impl Widget for DiskWidget {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        match ctx.disks.get(&self.mount) {
            Some(&d) => {
                let fraction = if d.total_bytes == 0 {
                    0.0
                } else {
                    d.used_bytes as f64 / d.total_bytes as f64
                };
                let percent = (fraction * 100.0).round() as u64;
                let fmt =
                    crate::widgets::active_format(ctx, &self.name, &self.format, &self.alt_format);
                let text = fmt
                    .replace("{used}", &format_bytes(d.used_bytes))
                    .replace("{total}", &format_bytes(d.total_bytes))
                    .replace("{avail}", &format_bytes(d.available_bytes))
                    .replace("{percent}", &percent.to_string())
                    .replace("{bar}", &bar::gauge_bar(fraction, self.bar_width))
                    .replace("{mount}", &self.mount);
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
                    .replace("{mount}", "");
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
    use crate::{Context, DiskInfo, Widget};
    use chrono::{Local, TimeZone};

    /// Builds a `Context` with `disk` populated under the "/" key of
    /// `disks` — the mount every test widget below uses via `w()`'s default
    /// `mount: "/"` (see `DiskWidget::render`'s `ctx.disks.get(&self.mount)`
    /// lookup, W46).
    fn ctx(disk: Option<DiskInfo>) -> Context {
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
        if let Some(d) = disk {
            c.disks.insert("/".to_string(), d);
        }
        c
    }

    fn disk(total: u64, used: u64, avail: u64) -> Option<DiskInfo> {
        Some(DiskInfo {
            total_bytes: total,
            used_bytes: used,
            available_bytes: avail,
        })
    }

    fn w(format: &str, down: &str) -> DiskWidget {
        DiskWidget {
            name: "disk".into(),
            format: format.into(),
            alt_format: String::new(),
            down_format: down.into(),
            mount: "/".into(),
            bar_width: 8,
            warn_percent: 85.0,
            crit_percent: 95.0,
        }
    }

    #[test]
    fn renders_used_total_percent() {
        let g = 1024u64.pow(3);
        let out =
            w("{used}/{total} {avail} {percent}%", "").render(&ctx(disk(16 * g, 6 * g, 10 * g)));
        assert_eq!(out[0].text, "6.0G/16G 10G 38%"); // 6/16 = 37.5 -> 38
    }

    #[test]
    fn renders_bar() {
        let g = 1024u64.pow(3);
        let out = w("{bar}", "").render(&ctx(disk(16 * g, 8 * g, 8 * g)));
        // 8/16 = 0.5 over width 8 -> "████░░░░"
        assert_eq!(out[0].text, "████░░░░");
    }

    #[test]
    fn renders_mount_from_widget_config_not_context() {
        // The `{mount}` placeholder always comes from the widget's own
        // `mount` field, never a `Context` field — but `mount` also now
        // selects which `disks` entry is read (W46), so the data must be
        // keyed under that same mount for the widget to render it at all.
        let g = 1024u64.pow(3);
        let mut widget = w("{mount}: {percent}%", "");
        widget.mount = "/home".into();
        let mut c = ctx(None);
        c.disks.insert(
            "/home".to_string(),
            DiskInfo {
                total_bytes: 16 * g,
                used_bytes: 8 * g,
                available_bytes: 8 * g,
            },
        );
        let out = widget.render(&c);
        assert_eq!(out[0].text, "/home: 50%");
    }

    #[test]
    fn zero_total_does_not_divide_by_zero() {
        let out = w("{percent}% {bar}", "").render(&ctx(disk(0, 0, 0)));
        assert_eq!(out[0].text, "0% ░░░░░░░░");
    }

    #[test]
    fn none_empty_down_skips() {
        assert!(w("{used}", "").render(&ctx(None)).is_empty());
    }

    #[test]
    fn none_down_format_collapses_placeholders() {
        let out = w("{used}", "n/a {used}{total}{avail}{bar}{percent}{mount}").render(&ctx(None));
        assert_eq!(out[0].text, "n/a ");
    }

    #[test]
    fn disk_toggled_uses_alt_format() {
        let g = 1024u64.pow(3);
        let mut c = ctx(disk(16 * g, 8 * g, 8 * g));
        c.toggled.insert(WidgetName::from("disk"));
        let out = DiskWidget {
            name: "disk".into(),
            format: "{percent}%".into(),
            alt_format: "{bar}".into(),
            down_format: String::new(),
            mount: "/".into(),
            bar_width: 8,
            warn_percent: 85.0,
            crit_percent: 95.0,
        }
        .render(&c);
        assert_eq!(out[0].text, "████░░░░");
    }

    #[test]
    fn disk_range_name_tracks_alt() {
        let base = w("x", "");
        assert_eq!(base.range_name(), None);
        let mut alt = w("x", "");
        alt.alt_format = "{bar}".into();
        assert_eq!(alt.range_name(), Some(RangeName::parse("disk").unwrap()));
    }

    #[test]
    fn below_threshold_plain_over_threshold_badge() {
        let g = 1024u64.pow(3);
        // 8/16 = 50% -> plain
        let out = w("{percent}%", "").render(&ctx(disk(16 * g, 8 * g, 8 * g)));
        assert_eq!(out[0].style, crate::Style::default());
        // 96/100 = 96% -> crit (>= 95 default)
        let mut c = ctx(disk(100 * g, 96 * g, 4 * g));
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

    #[test]
    fn disk_widget_reads_its_mount_from_disks_map() {
        let mut c = Context::default();
        let g = 1024u64.pow(3);
        c.disks.insert(
            "/data".into(),
            DiskInfo {
                total_bytes: 10 * g,
                used_bytes: 4 * g,
                available_bytes: 6 * g,
            },
        );
        let w = crate::widgets::build_disk(
            "disk_data",
            &crate::config::DiskOpts {
                mount: "/data".into(),
                ..Default::default()
            },
        );
        let texts: Vec<String> = w.render(&c).into_iter().map(|s| s.text).collect();
        assert_eq!(texts, vec![" 4.0G/10G".to_string()]); // default " {used}/{total}"
    }

    #[test]
    fn warn_threshold_badge() {
        let g = 1024u64.pow(3);
        // 87/100 = 87% -> crosses warn (85) but not crit (95)
        let mut c = ctx(disk(100 * g, 87 * g, 13 * g));
        c.colors = crate::ThemeColors {
            error: crate::Color::Indexed(196),
            warning: crate::Color::Indexed(214),
            bar_bg: crate::Color::Indexed(234),
            ..Default::default()
        };
        let out = w("{percent}%", "").render(&c);
        assert_eq!(out[0].style.bg, Some(crate::Color::Indexed(214)));
        assert!(out[0].style.bold);
    }
}
