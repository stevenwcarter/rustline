//! Region assembly: turns a set of widget names plus a [`Context`] into the
//! rendered tmux status-line string for one region, gluing together
//! [`Registry`] resolution, per-segment palette assignment, and
//! [`render_region`](crate::render::render_region).

use crate::render::{Direction, RangeGroup, Theme, render_region_ranged, render_window_pill};
use crate::{Context, Registry, Segment, Widget};

/// Fill in each segment's background from `theme.palette`, cycling through
/// it in order, but only where a segment doesn't already carry an explicit
/// background (e.g. the `windows` widget's current-window emphasis). A
/// widget that wants a specific color sets `style.bg` itself and is left
/// untouched here.
///
/// No-op when the palette is empty, since `i % 0` would panic.
pub fn assign_palette(segments: &mut [Segment], theme: &Theme) {
    if theme.palette.is_empty() {
        return;
    }
    for (i, s) in segments.iter_mut().enumerate() {
        if s.style.bg.is_none() {
            s.style.bg = Some(theme.palette[i % theme.palette.len()].clone());
        }
    }
}

/// Render a widget, converting a panic into an empty segment list plus a
/// warning instead of letting it unwind through the whole region. A single
/// misbehaving widget (built-in or plugin) must never take down the rest of
/// the status line.
fn render_guarded(widget: &dyn Widget, ctx: &Context) -> Vec<Segment> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| widget.render(ctx))) {
        Ok(segments) => segments,
        Err(_) => {
            tracing::warn!("widget panicked, skipping");
            vec![]
        }
    }
}

/// Resolve `names` against `registry`, render each widget (panic-guarded),
/// flatten the resulting segments in the given order, assign palette
/// backgrounds to any that lack one, and render the region for `dir`.
///
/// Widget order is preserved as given in `names`: `render_region` always
/// places `segments[0]` leftmost regardless of `dir`, so callers are
/// responsible for passing widgets in the visual left-to-right order for
/// their region.
pub fn render_named_region(
    dir: Direction,
    names: &[String],
    ctx: &Context,
    registry: &Registry,
    theme: &Theme,
) -> String {
    let widgets = registry.resolve(names);
    // Render each widget (panic-guarded), keeping its clickable range name.
    let rendered: Vec<(Option<String>, Vec<Segment>)> = widgets
        .iter()
        .map(|w| {
            (
                w.range_name().map(str::to_string),
                render_guarded(w.as_ref(), ctx),
            )
        })
        .collect();

    // Assign palette across the FLATTENED region (unchanged global cycling),
    // then regroup by remembered lengths so range markup can bracket each widget.
    let range_names: Vec<Option<String>> = rendered.iter().map(|(n, _)| n.clone()).collect();
    let lens: Vec<usize> = rendered.iter().map(|(_, s)| s.len()).collect();
    let mut flat: Vec<Segment> = rendered.into_iter().flat_map(|(_, s)| s).collect();
    assign_palette(&mut flat, theme);

    let mut it = flat.into_iter();
    let groups: Vec<RangeGroup> = range_names
        .into_iter()
        .zip(lens)
        .map(|(range, len)| RangeGroup {
            range,
            segments: (&mut it).take(len).collect(),
        })
        .collect();

    render_region_ranged(dir, &groups, theme)
}

/// Render the single `windows` segment as a rounded pill. Unlike
/// [`render_named_region`], this does not go through
/// [`render_region`](crate::render::render_region)'s pointed
/// separators or `assign_palette`: the window list owns a dedicated rounded-cap
/// pill ([`render_window_pill`]), colored by the theme from the window's
/// current/inactive state. A panicking or absent window degrades to `""`.
pub fn render_window(ctx: &Context, registry: &Registry, theme: &Theme) -> String {
    let widgets = registry.resolve(&["windows".to_string()]);
    let segments: Vec<Segment> = widgets
        .iter()
        .flat_map(|w| render_guarded(w.as_ref(), ctx))
        .collect();
    let Some(seg) = segments.first() else {
        return String::new();
    };
    let is_current = ctx.window.as_ref().is_some_and(|w| w.is_current);
    render_window_pill(&seg.text, is_current, theme)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, Context, Direction};
    use chrono::{Local, TimeZone};

    fn ctx() -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/home/steve/x".into(),
            home: "/home/steve".into(),
            hostname: "scadrial".into(),
            loadavg: Some([0.1, 0.2, 0.3]),
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
    fn render_left_default_layout_contains_widgets() {
        let cfg = Config::default();
        let reg = Registry::with_builtins(&cfg);
        let theme = cfg.to_theme();
        let out = render_named_region(Direction::Left, &cfg.layout.left, &ctx(), &reg, &theme);
        assert!(out.contains("0:0.0"), "pane_id: {out}");
        assert!(out.contains("scadrial"), "hostname: {out}");
        assert!(out.contains("#["), "styled: {out}");
    }

    #[test]
    fn assign_palette_fills_missing_bg_alternating() {
        let theme = crate::Theme::default(); // palette len 2
        let mut segs = vec![
            crate::Segment::new("a"),
            crate::Segment::new("b"),
            crate::Segment::new("c"),
        ];
        assign_palette(&mut segs, &theme);
        assert_eq!(segs[0].style.bg, Some(theme.palette[0].clone()));
        assert_eq!(segs[1].style.bg, Some(theme.palette[1].clone()));
        assert_eq!(segs[2].style.bg, Some(theme.palette[0].clone()));
    }

    #[test]
    fn assign_palette_skips_explicit_bg_and_fills_neighbors() {
        use crate::{Color, Style};
        let theme = crate::Theme::default(); // palette len 2: [31, 238]
        let mut segs = vec![
            crate::Segment::new("a"), // no bg -> palette[0]
            crate::Segment::styled(
                "b",
                Style {
                    fg: None,
                    bg: Some(Color::Indexed(99)), // explicit -> must be left untouched
                    bold: false,
                },
            ),
            crate::Segment::new("c"), // no bg -> palette[2 % 2] = palette[0]
        ];
        assign_palette(&mut segs, &theme);
        assert_eq!(segs[0].style.bg, Some(theme.palette[0].clone()));
        assert_eq!(
            segs[1].style.bg,
            Some(Color::Indexed(99)),
            "explicit bg preserved"
        );
        assert_eq!(segs[2].style.bg, Some(theme.palette[0].clone()));
    }

    #[test]
    fn render_window_current_is_bold_accent_pill() {
        let cfg = Config::default();
        let reg = Registry::with_builtins(&cfg);
        let theme = cfg.to_theme();
        let mut c = ctx();
        c.window = Some(crate::WindowCtx {
            index: "1".into(),
            name: "shell".into(),
            flags: "*".into(),
            is_current: true,
        });
        let out = render_window(&c, &reg, &theme);
        assert!(
            out.contains('\u{e0b6}') && out.contains('\u{e0b4}'),
            "rounded caps: {out}"
        );
        assert!(out.contains("1* shell"), "text: {out}");
        assert!(out.contains(",bold]"), "current bold: {out}");
        assert!(
            out.contains(&format!("bg={}", theme.win_current_bg.to_tmux())),
            "accent fill: {out}"
        );
    }

    #[test]
    fn render_window_inactive_is_gray_pill() {
        let cfg = Config::default();
        let reg = Registry::with_builtins(&cfg);
        let theme = cfg.to_theme();
        let mut c = ctx();
        c.window = Some(crate::WindowCtx {
            index: "2".into(),
            name: "editor".into(),
            flags: "".into(),
            is_current: false,
        });
        let out = render_window(&c, &reg, &theme);
        assert!(out.contains("2 editor"), "text: {out}");
        assert!(
            out.contains(&format!("bg={}", theme.win_inactive_bg.to_tmux())),
            "gray fill: {out}"
        );
        assert!(!out.contains(",bold]"), "inactive not bold: {out}");
    }

    #[test]
    fn render_window_no_window_is_empty() {
        let cfg = Config::default();
        let reg = Registry::with_builtins(&cfg);
        let theme = cfg.to_theme();
        assert_eq!(render_window(&ctx(), &reg, &theme), "");
    }

    #[test]
    fn panicking_widget_does_not_break_region() {
        use crate::{Segment, Widget};
        struct Boom;
        impl Widget for Boom {
            fn render(&self, _c: &Context) -> Vec<Segment> {
                panic!("boom")
            }
        }
        let mut reg = Registry::with_builtins(&Config::default());
        reg.register("boom", Box::new(|| Box::new(Boom)));
        let theme = Theme::default();
        let names = vec!["boom".to_string(), "hostname".to_string()];
        let out = render_named_region(Direction::Left, &names, &ctx(), &reg, &theme);
        assert!(
            out.contains("scadrial"),
            "surviving widget still renders: {out}"
        );
    }

    #[test]
    fn render_right_region_shows_alert_badge_markup() {
        // Cross-module integration: Context.colors -> the cpu widget's
        // alert_over/alert_style -> a styled Segment -> assign_palette
        // (which must skip a segment's explicit bg) -> render_region_ranged
        // markup. Pins that a crit-tier reading actually surfaces as tmux
        // badge markup end to end, not just that the pure helpers agree.
        let cfg = Config::default();
        let reg = Registry::with_builtins(&cfg);
        let theme = cfg.to_theme();
        let mut c = ctx();
        c.cpu = Some(crate::CpuUsage { percent: 96.0 }); // >= default crit 95 -> error tier
        c.colors = crate::ThemeColors {
            error: crate::Color::Indexed(196),
            bar_bg: crate::Color::Indexed(234),
            ..Default::default()
        };
        let out = render_named_region(Direction::Right, &["cpu".to_string()], &c, &reg, &theme);
        assert!(out.contains("bg=colour196"), "alert badge bg: {out}");
        assert!(out.contains("bold"), "alert badge bold: {out}");
    }

    #[test]
    fn named_region_wraps_clickable_widget_range() {
        use crate::{Segment, Widget};
        struct Clicky;
        impl Widget for Clicky {
            fn render(&self, _c: &Context) -> Vec<Segment> {
                vec![Segment::new("hi")]
            }
            fn range_name(&self) -> Option<&str> {
                Some("clicky")
            }
        }
        let mut reg = Registry::with_builtins(&Config::default());
        reg.register("clicky", Box::new(|| Box::new(Clicky)));
        let out = render_named_region(
            Direction::Left,
            &["clicky".into(), "hostname".into()],
            &ctx(),
            &reg,
            &Theme::default(),
        );
        assert!(
            out.contains("#[range=user|clicky]"),
            "wraps clickable: {out}"
        );
        assert!(out.contains("#[norange]"), "closes range: {out}");
        assert!(out.contains("hi"), "text present: {out}");
    }
}
