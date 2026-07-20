use crate::{Context, Segment, Widget};

/// Renders the current time, formatted with a `chrono` strftime string.
pub struct DateTime {
    pub format: String,
}

impl Default for DateTime {
    fn default() -> Self {
        Self {
            format: "%a < %Y-%m-%d < %H:%M".into(),
        }
    }
}

impl Widget for DateTime {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        vec![Segment::new(ctx.now.format(&self.format).to_string())]
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
        };
        assert_eq!(w.render(&ctx_at())[0].text, "17:49");
    }
}
