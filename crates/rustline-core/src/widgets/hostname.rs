use crate::{Context, Segment, Widget};

/// Renders the local hostname, truncated at the first `.` so a fully
/// qualified name like `scadrial.example.com` shows just `scadrial`.
pub struct Hostname;

impl Widget for Hostname {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let short = match ctx.hostname.split_once('.') {
            Some((head, _)) => head,
            None => &ctx.hostname,
        };
        vec![Segment::new(short)]
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
            os: String::new(),
            arch: String::new(),
        }
    }

    #[test]
    fn hostname_truncates_at_first_dot() {
        assert_eq!(Hostname.render(&ctx())[0].text, "scadrial");
    }
}
