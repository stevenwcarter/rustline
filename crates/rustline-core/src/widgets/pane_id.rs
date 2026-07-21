use crate::{Context, Segment, Widget};

/// Renders the tmux target triple for the current pane, e.g. `0:0.0`
/// (`session:window.pane`).
pub struct PaneId;

impl Widget for PaneId {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        vec![Segment::new(format!(
            "{}:{}.{}",
            ctx.session_name, ctx.window_index, ctx.pane_index
        ))]
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
            os: String::new(),
            arch: String::new(),
        }
    }

    #[test]
    fn pane_id_formats_session_window_pane() {
        assert_eq!(PaneId.render(&ctx())[0].text, "0:0.0");
    }
}
