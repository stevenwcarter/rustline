use crate::{Context, Segment, Widget};

/// Renders the current tmux window as a single text segment (index, flags, and
/// name). The pill styling and active/inactive colors are applied downstream by
/// the theme-aware window renderer (`render_window`/`render_window_pill`), since
/// widgets only see [`Context`], not the `Theme`.
pub struct Windows;

impl Widget for Windows {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let Some(w) = &ctx.window else {
            return vec![];
        };
        let text = format!("{}{} {}", w.index, w.flags, w.name);
        vec![Segment::new(text)]
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
            cpu_history: Vec::new(),
            mem_history: Vec::new(),
            colors: Default::default(),
        }
    }

    #[test]
    fn current_window_text_only() {
        let w = ctx(Some(WindowCtx {
            index: "0".into(),
            name: "name".into(),
            flags: "*".into(),
            is_current: true,
        }));
        let out = Windows.render(&w);
        assert_eq!(out[0].text, "0* name");
        // Styling now lives in the theme-aware pill renderer, not the widget.
        assert_eq!(out[0].style, crate::Style::default());
    }

    #[test]
    fn inactive_window_text_only() {
        let w = ctx(Some(WindowCtx {
            index: "1".into(),
            name: "other".into(),
            flags: "".into(),
            is_current: false,
        }));
        let out = Windows.render(&w);
        assert_eq!(out[0].text, "1 other");
        assert_eq!(out[0].style, crate::Style::default());
    }

    #[test]
    fn no_window_ctx_renders_nothing() {
        assert!(Windows.render(&ctx(None)).is_empty());
    }
}
