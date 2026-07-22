use crate::{Context, Segment, Widget};

/// Renders the current time, formatted with a `chrono` strftime string.
pub struct DateTime {
    pub format: String,
    pub alt_format: String,
}

impl DateTime {
    /// Registry/layout name; the toggle key threaded through render + click.
    pub const NAME: &'static str = "datetime";
}

impl Default for DateTime {
    fn default() -> Self {
        Self {
            format: "%a < %Y-%m-%d < %H:%M".into(),
            alt_format: String::new(),
        }
    }
}

impl Widget for DateTime {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let fmt = crate::widgets::active_format(ctx, Self::NAME, &self.format, &self.alt_format);
        vec![Segment::new(ctx.now.format(fmt).to_string())]
    }

    fn range_name(&self) -> Option<&str> {
        crate::widgets::clickable_range(Self::NAME, &self.alt_format)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Widget};
    use chrono::{Local, TimeZone};

    fn ctx_at() -> Context {
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
            interfaces: Vec::new(),
            battery: None,
            cpu: None,
            memory: None,
            os: String::new(),
            arch: String::new(),
            toggled: Default::default(),
            colors: Default::default(),
        }
    }

    #[test]
    fn default_format_renders_expected() {
        let w = DateTime::default();
        assert_eq!(w.render(&ctx_at())[0].text, "Mon < 2026-07-20 < 17:49");
    }

    #[test]
    fn custom_format_honored() {
        let w = DateTime {
            format: "%H:%M".into(),
            alt_format: String::new(),
        };
        assert_eq!(w.render(&ctx_at())[0].text, "17:49");
    }

    #[test]
    fn datetime_toggled_uses_alt_format() {
        let mut c = ctx_at();
        c.toggled.insert("datetime".to_string());
        let w = DateTime {
            format: "%H:%M".into(),
            alt_format: "%Y-%m-%d %H:%M".into(),
        };
        assert_eq!(w.render(&c)[0].text, "2026-07-20 17:49");
        // untoggled
        let w = DateTime {
            format: "%H:%M".into(),
            alt_format: "%Y-%m-%d %H:%M".into(),
        };
        assert_eq!(w.render(&ctx_at())[0].text, "17:49");
    }

    #[test]
    fn datetime_range_name_tracks_alt() {
        assert_eq!(
            DateTime {
                format: "%H:%M".into(),
                alt_format: String::new(),
            }
            .range_name(),
            None
        );
        assert_eq!(
            DateTime {
                format: "%H:%M".into(),
                alt_format: "%c".into(),
            }
            .range_name(),
            Some("datetime")
        );
    }
}
