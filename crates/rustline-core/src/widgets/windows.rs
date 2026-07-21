use crate::{Color, Context, Segment, Style, Widget};

/// Renders the current tmux window as a single segment (index, flags, and
/// name), emphasized when it is the active window.
pub struct Windows;

impl Widget for Windows {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let Some(w) = &ctx.window else {
            return vec![];
        };
        let text = format!("{}{} {}", w.index, w.flags, w.name);
        let style = if w.is_current {
            Style {
                fg: None,
                bg: Some(Color::Indexed(31)),
                bold: true,
            }
        } else {
            Style::default()
        };
        vec![Segment::styled(text, style)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Widget, WindowCtx};
    use chrono::{Local, TimeZone};

    fn ctx(win: Option<WindowCtx>) -> Context {
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
            window: win,
            interfaces: Vec::new(),
        }
    }

    #[test]
    fn current_window_text_and_emphasis() {
        let w = ctx(Some(WindowCtx {
            index: "0".into(),
            name: "name".into(),
            flags: "*".into(),
            is_current: true,
        }));
        let out = Windows.render(&w);
        assert_eq!(out[0].text, "0* name");
        assert!(out[0].style.bold);
        assert!(out[0].style.bg.is_some());
    }

    #[test]
    fn inactive_window_is_plain() {
        let w = ctx(Some(WindowCtx {
            index: "1".into(),
            name: "other".into(),
            flags: "".into(),
            is_current: false,
        }));
        let out = Windows.render(&w);
        assert_eq!(out[0].text, "1 other");
        assert!(!out[0].style.bold);
        assert!(out[0].style.bg.is_none());
    }

    #[test]
    fn no_window_ctx_renders_nothing() {
        assert!(Windows.render(&ctx(None)).is_empty());
    }
}
