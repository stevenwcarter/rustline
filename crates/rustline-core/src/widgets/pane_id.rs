use crate::{Context, Segment, Widget};

/// Renders the tmux target triple for the current pane, e.g. `0:0.0`
/// (`session:window.pane`), via a `format` template (default
/// `"{session}:{window}.{pane}"`) whose `{session}`/`{window}`/`{pane}`
/// placeholders are replaced; any other text (e.g. a Nerd-Font icon or
/// label) is emitted verbatim. Unknown placeholders pass through untouched.
pub struct PaneId {
    pub format: String,
}

impl Default for PaneId {
    fn default() -> Self {
        Self {
            format: "{session}:{window}.{pane}".into(),
        }
    }
}

impl Widget for PaneId {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let text = self
            .format
            .replace("{session}", &ctx.session_name)
            .replace("{window}", &ctx.window_index)
            .replace("{pane}", &ctx.pane_index);
        vec![Segment::new(text)]
    }
}

#[cfg(test)]
mod tests {
    use crate::{Context, Widget};
    use chrono::{Local, TimeZone};

    use super::PaneId;

    fn ctx() -> Context {
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
            throughput: None,
            os: String::new(),
            arch: String::new(),
            uptime: None,
            media: None,
            toggled: Default::default(),
            colors: Default::default(),
        }
    }

    #[test]
    fn pane_id_default_format_matches_current() {
        assert_eq!(PaneId::default().render(&ctx())[0].text, "0:0.0");
    }

    #[test]
    fn pane_id_custom_format() {
        let w = PaneId {
            format: "\u{f120} {session}/{window}/{pane}".into(),
        };
        assert_eq!(w.render(&ctx())[0].text, "\u{f120} 0/0/0");
    }
}
