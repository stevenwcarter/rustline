use crate::{Context, Segment, Widget};

/// Renders the 1/5/15-minute load average, when available.
///
/// `Context::loadavg` is `None` on platforms/environments where it couldn't
/// be sampled; this widget renders nothing rather than faking zeros.
pub struct LoadAvg;

impl Widget for LoadAvg {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        match ctx.loadavg {
            Some([a, b, c]) => vec![Segment::new(format!("{a:.2} {b:.2} {c:.2}"))],
            None => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Widget};
    use chrono::{Local, TimeZone};

    fn ctx_load(l: Option<[f64; 3]>) -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/".into(),
            home: "/h".into(),
            hostname: "h".into(),
            loadavg: l,
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
    fn formats_three_values() {
        let out = LoadAvg.render(&ctx_load(Some([0.42, 0.31, 0.296])));
        assert_eq!(out[0].text, "0.42 0.31 0.30");
    }

    #[test]
    fn none_renders_nothing() {
        assert!(LoadAvg.render(&ctx_load(None)).is_empty());
    }
}
