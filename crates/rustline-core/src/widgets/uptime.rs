use crate::{Context, Segment, Widget};

/// Renders system uptime, humanized (`3d 4h`, `1h 15m`, `12m`, `<1m`).
///
/// `Context::uptime` is `None` on platforms/environments where it couldn't be
/// read; the widget then renders nothing (or `down_format`) rather than
/// faking a reading (invariant #6). Part of the format-bearing widget family:
/// a non-empty `alt_format` makes it click-toggleable.
pub struct Uptime {
    pub format: String,
    pub alt_format: String,
    pub down_format: String,
}

impl Uptime {
    /// Registry/layout name; the toggle key threaded through render + click.
    pub const NAME: &'static str = "uptime";
}

impl Widget for Uptime {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        match ctx.uptime {
            Some(secs) => {
                let fmt =
                    crate::widgets::active_format(ctx, Self::NAME, &self.format, &self.alt_format);
                let text = fmt.replace("{uptime}", &humanize_uptime(secs));
                vec![Segment::new(text)]
            }
            None => {
                if self.down_format.is_empty() {
                    return vec![];
                }
                // Collapse the placeholder so a stray token never renders
                // (invariant #6).
                vec![Segment::new(self.down_format.replace("{uptime}", ""))]
            }
        }
    }

    fn range_name(&self) -> Option<&str> {
        crate::widgets::clickable_range(Self::NAME, &self.alt_format)
    }
}

/// Humanize a seconds count into the coarsest non-zero unit pair: `Nd Nh`
/// once at least a day has elapsed, else `Nh Nm` once at least an hour has,
/// else `Nm` once at least a minute has, else `<1m`.
pub(crate) fn humanize_uptime(secs: u64) -> String {
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3600;
    let m = (secs % 3600) / 60;
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        "<1m".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};

    fn ctx(uptime: Option<u64>) -> Context {
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
            throughput: None,
            uptime,
            media: None,
            os: String::new(),
            arch: String::new(),
            toggled: Default::default(),
            cpu_history: Vec::new(),
            mem_history: Vec::new(),
            colors: Default::default(),
        }
    }

    fn w(format: &str, alt: &str, down: &str) -> Uptime {
        Uptime {
            format: format.into(),
            alt_format: alt.into(),
            down_format: down.into(),
        }
    }

    #[test]
    fn humanizes_uptime_buckets() {
        assert_eq!(humanize_uptime(0), "<1m");
        assert_eq!(humanize_uptime(59), "<1m");
        assert_eq!(humanize_uptime(60), "1m");
        assert_eq!(humanize_uptime(60 * 75), "1h 15m");
        assert_eq!(humanize_uptime(86_400 * 3 + 3600 * 4), "3d 4h");
        assert_eq!(humanize_uptime(86_400), "1d 0h");
    }

    #[test]
    fn renders_default_format() {
        let out = w("{uptime}", "", "").render(&ctx(Some(86_400 * 3 + 3600 * 4)));
        assert_eq!(out[0].text, "3d 4h");
    }

    #[test]
    fn none_empty_down_renders_nothing() {
        assert!(w("{uptime}", "", "").render(&ctx(None)).is_empty());
    }

    #[test]
    fn none_down_format_collapses_placeholder() {
        let out = w("{uptime}", "", "up n/a {uptime}").render(&ctx(None));
        assert_eq!(out[0].text, "up n/a ");
    }

    #[test]
    fn toggled_uses_alt_format() {
        let mut c = ctx(Some(90));
        c.toggled.insert("uptime".to_string());
        let out = w("up {uptime}", "u:{uptime}", "").render(&c);
        assert_eq!(out[0].text, "u:1m");
        // untoggled -> normal format
        let out = w("up {uptime}", "u:{uptime}", "").render(&ctx(Some(90)));
        assert_eq!(out[0].text, "up 1m");
    }

    #[test]
    fn range_name_some_only_with_alt_format() {
        assert_eq!(w("{uptime}", "u:{uptime}", "").range_name(), Some("uptime"));
        assert_eq!(w("{uptime}", "", "").range_name(), None);
    }
}
