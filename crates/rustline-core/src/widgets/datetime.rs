use crate::{Context, Segment, Widget};

/// Renders the current time, formatted with a `chrono` strftime string.
pub struct DateTime {
    pub format: String,
    pub alt_format: String,
    /// An IANA zone name to render in instead of `ctx.now`'s local zone;
    /// `None` keeps the pre-feature local-time behavior.
    pub timezone: Option<String>,
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
            timezone: None,
        }
    }
}

impl Widget for DateTime {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let fmt = crate::widgets::active_format(ctx, Self::NAME, &self.format, &self.alt_format);
        let zone = self.timezone.as_deref();
        let formatted = match zone.and_then(|z| z.parse::<chrono_tz::Tz>().ok()) {
            Some(tz) => ctx.now.with_timezone(&tz).format(fmt).to_string(),
            None => {
                if zone.is_some_and(|z| !z.is_empty()) {
                    tracing::warn!(zone, "unknown timezone; using Local");
                }
                ctx.now.format(fmt).to_string()
            }
        };
        vec![Segment::new(formatted)]
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
            git: None,
            disk: None,
            os: String::new(),
            arch: String::new(),
            uptime: None,
            media: None,
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
            timezone: None,
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
            timezone: None,
        };
        assert_eq!(w.render(&c)[0].text, "2026-07-20 17:49");
        // untoggled
        let w = DateTime {
            format: "%H:%M".into(),
            alt_format: "%Y-%m-%d %H:%M".into(),
            timezone: None,
        };
        assert_eq!(w.render(&ctx_at())[0].text, "17:49");
    }

    #[test]
    fn configured_timezone_renders_that_zone() {
        // Fixed instant: 2026-01-01 00:30:00 UTC (1_767_227_400).
        let now = Local.timestamp_opt(1_767_227_400, 0).single().unwrap();
        let ctx = Context { now, ..ctx_at() };

        let utc = DateTime {
            format: "%H".into(),
            alt_format: String::new(),
            timezone: Some("UTC".into()),
        };
        assert_eq!(utc.render(&ctx)[0].text, "00");
    }

    #[test]
    fn unknown_timezone_falls_back_to_local_without_panicking() {
        let now = Local.timestamp_opt(1_767_227_400, 0).single().unwrap();
        let ctx = Context { now, ..ctx_at() };

        let bad = DateTime {
            format: "%H".into(),
            alt_format: String::new(),
            timezone: Some("Not/AZone".into()),
        };
        let _ = bad.render(&ctx);
    }

    #[test]
    fn no_timezone_uses_local_without_panicking() {
        let now = Local.timestamp_opt(1_767_227_400, 0).single().unwrap();
        let ctx = Context { now, ..ctx_at() };

        let local = DateTime {
            format: "%H".into(),
            alt_format: String::new(),
            timezone: None,
        };
        let _ = local.render(&ctx);
    }

    #[test]
    fn datetime_range_name_tracks_alt() {
        assert_eq!(
            DateTime {
                format: "%H:%M".into(),
                alt_format: String::new(),
                timezone: None,
            }
            .range_name(),
            None
        );
        assert_eq!(
            DateTime {
                format: "%H:%M".into(),
                alt_format: "%c".into(),
                timezone: None,
            }
            .range_name(),
            Some("datetime")
        );
    }
}
