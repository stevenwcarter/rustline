use crate::{Context, Segment, Widget};

/// Renders the local hostname, truncated at the first `.` so a fully
/// qualified name like `scadrial.example.com` shows just `scadrial`, via a
/// `format` template (default `"{host}"`) whose `{host}` placeholder is
/// replaced with the truncated name; any other text (e.g. a Nerd-Font icon
/// or label) is emitted verbatim. Unknown placeholders pass through
/// untouched.
pub struct Hostname {
    pub format: String,
}

impl Default for Hostname {
    fn default() -> Self {
        Self {
            format: "{host}".into(),
        }
    }
}

impl Widget for Hostname {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let short = match ctx.hostname.split_once('.') {
            Some((head, _)) => head,
            None => &ctx.hostname,
        };
        vec![Segment::new(self.format.replace("{host}", short))]
    }
}

#[cfg(test)]
mod tests {
    use crate::{Context, Widget};
    use chrono::{Local, TimeZone};

    use super::Hostname;

    fn ctx() -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/".into(),
            home: "/h".into(),
            hostname: "scadrial.example.com".into(),
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
    fn hostname_default_format_matches_current() {
        assert_eq!(Hostname::default().render(&ctx())[0].text, "scadrial");
    }

    #[test]
    fn hostname_custom_label_prepends() {
        let w = Hostname {
            format: "\u{f108} {host}".into(),
        };
        assert_eq!(w.render(&ctx())[0].text, "\u{f108} scadrial");
    }
}
