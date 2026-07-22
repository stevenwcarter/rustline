//! Powerline rendering: turn a slice of [`Segment`]s into tmux status-line
//! markup with hard/soft powerline separators and blended outer edges.

use std::fmt::Write;

use crate::{Color, Segment};

/// Which side of the status bar a region is anchored to. This decides the
/// powerline glyph orientation and how separator colors are mirrored: `Left`
/// arrows point rightwards (into the bar), `Right` arrows point leftwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
}

/// Visual theme for a rendered region: the color palette, default foreground,
/// bar background, and the four powerline separator glyphs (hard/soft for each
/// direction) plus the color used to draw soft separators.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub palette: Vec<Color>,
    pub fg: Color,
    pub bar_bg: Color,
    pub hard_left: String,
    pub hard_right: String,
    pub soft_left: String,
    pub soft_right: String,
    pub soft_fg: Color,
    /// Rounded left/right cap glyphs for the window-list pill (distinct from the
    /// pointed `hard_*`/`soft_*` separators used by [`render_region`]).
    pub win_cap_left: String,
    pub win_cap_right: String,
    /// Fill/text colors for the active (current) window pill; the active pill is
    /// also rendered bold.
    pub win_current_bg: Color,
    pub win_current_fg: Color,
    /// Fill/text colors for inactive window pills.
    pub win_inactive_bg: Color,
    pub win_inactive_fg: Color,
    /// Semantic colors, available to widgets/plugins via `Context.colors` and
    /// used by threshold-aware widgets for alert badges.
    pub success: Color,
    pub info: Color,
    pub warning: Color,
    pub error: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            palette: vec![Color::Indexed(31), Color::Indexed(238)],
            fg: Color::Indexed(255),
            bar_bg: Color::Indexed(234),
            hard_left: "\u{e0b0}".into(),
            hard_right: "\u{e0b2}".into(),
            soft_left: "\u{e0b1}".into(),
            soft_right: "\u{e0b3}".into(),
            soft_fg: Color::Indexed(240),
            win_cap_left: "\u{e0b6}".into(),
            win_cap_right: "\u{e0b4}".into(),
            win_current_bg: Color::Indexed(31),
            win_current_fg: Color::Indexed(255),
            win_inactive_bg: Color::Indexed(236),
            win_inactive_fg: Color::Indexed(250),
            success: Color::Indexed(35),
            info: Color::Indexed(39),
            warning: Color::Indexed(214),
            error: Color::Indexed(196),
        }
    }
}

impl Theme {
    /// The hard (background-changing) separator glyph for `dir`.
    fn hard(&self, dir: Direction) -> &str {
        match dir {
            Direction::Left => &self.hard_left,
            Direction::Right => &self.hard_right,
        }
    }

    /// The soft (same-background) separator glyph for `dir`.
    fn soft(&self, dir: Direction) -> &str {
        match dir {
            Direction::Left => &self.soft_left,
            Direction::Right => &self.soft_right,
        }
    }

    /// Bundle the theme-derived colors a widget/plugin may consume.
    pub fn colors(&self) -> crate::ThemeColors {
        crate::ThemeColors {
            fg: self.fg.clone(),
            bar_bg: self.bar_bg.clone(),
            success: self.success.clone(),
            info: self.info.clone(),
            warning: self.warning.clone(),
            error: self.error.clone(),
        }
    }
}

/// The effective foreground for a segment: its own `fg`, or the theme default.
fn eff_fg<'a>(s: &'a Segment, theme: &'a Theme) -> &'a Color {
    s.style.fg.as_ref().unwrap_or(&theme.fg)
}

/// The effective background for a segment: its own `bg`, or the bar background.
fn eff_bg<'a>(s: &'a Segment, theme: &'a Theme) -> &'a Color {
    s.style.bg.as_ref().unwrap_or(&theme.bar_bg)
}

/// Write a hard powerline glyph at a boundary between a `left` and a `right`
/// background color. For [`Direction::Left`] the arrow is colored
/// `fg=left,bg=right`; for [`Direction::Right`] the roles mirror
/// (`fg=right,bg=left`) so the glyph points outward. This single rule drives
/// both between-segment separators and the outer region edges.
fn write_hard(out: &mut String, theme: &Theme, dir: Direction, left: &Color, right: &Color) {
    let (fg, bg) = match dir {
        Direction::Left => (left, right),
        Direction::Right => (right, left),
    };
    let _ = write!(
        out,
        "#[fg={},bg={}]{}",
        fg.to_tmux(),
        bg.to_tmux(),
        theme.hard(dir),
    );
}

/// Render `segments` into tmux status-line markup for the given `dir`.
///
/// Each segment becomes `#[fg=<F>,bg=<B>] text ` where `F`/`B` are the
/// segment's effective foreground/background. Adjacent segments are joined by a
/// hard glyph when their backgrounds differ, or a soft glyph when they match.
/// The region blends into the bar at both outer edges (a hard glyph to/from
/// `theme.bar_bg`), except where a bordering segment's background already equals
/// `bar_bg`. The string always ends with `#[default]`; empty input renders `""`.
pub fn render_region(dir: Direction, segments: &[Segment], theme: &Theme) -> String {
    let (Some(first), Some(last)) = (segments.first(), segments.last()) else {
        return String::new();
    };

    let mut out = String::new();

    // Leading (outer) edge: blend the bar background into the first segment when
    // their backgrounds differ. For a Right region this is the left-hand edge.
    let first_bg = eff_bg(first, theme);
    if first_bg != &theme.bar_bg {
        write_hard(&mut out, theme, dir, &theme.bar_bg, first_bg);
    }

    let mut prev_bg: Option<&Color> = None;
    for s in segments {
        let cur_bg = eff_bg(s, theme);
        if let Some(prev_bg) = prev_bg {
            if prev_bg != cur_bg {
                write_hard(&mut out, theme, dir, prev_bg, cur_bg);
            } else {
                let _ = write!(
                    out,
                    "#[fg={},bg={}]{}",
                    theme.soft_fg.to_tmux(),
                    cur_bg.to_tmux(),
                    theme.soft(dir),
                );
            }
        }
        let bold = if s.style.bold { ",bold" } else { "" };
        let _ = write!(
            out,
            "#[fg={},bg={}{bold}] {} ",
            eff_fg(s, theme).to_tmux(),
            cur_bg.to_tmux(),
            s.text,
        );
        prev_bg = Some(cur_bg);
    }

    // Trailing (outer) edge: blend the last segment back into the bar when their
    // backgrounds differ. For a Right region this is the far right-hand edge.
    let last_bg = eff_bg(last, theme);
    if last_bg != &theme.bar_bg {
        write_hard(&mut out, theme, dir, last_bg, &theme.bar_bg);
    }

    out.push_str("#[default]");
    out
}

/// tmux's `range=user|<name>` argument caps `<name>` at this many bytes. The
/// single source of truth for that limit: `widgets::toggle::clickable_range`
/// and `rustline-wasm`'s plugin-name gate both compare against this constant
/// rather than a hardcoded `15`, so the limit only needs to change in one place.
pub const RANGE_NAME_MAX_BYTES: usize = 15;

/// A widget's rendered segments plus its optional clickable range name. The
/// assemble layer builds these so `render_region_ranged` can bracket clickable
/// widgets in `#[range=user|NAME]…#[norange]` while keeping every other byte of
/// output identical to `render_region`.
pub struct RangeGroup {
    pub range: Option<String>,
    pub segments: Vec<Segment>,
}

/// Like `render_region`, but bracket each group whose `range` is `Some(name)` in
/// `#[range=user|name]…#[norange]`. Inter-widget separators and both outer edge
/// glyphs are emitted OUTSIDE any range. With every group's `range == None` the
/// output is byte-identical to `render_region` over the flattened segments.
pub fn render_region_ranged(dir: Direction, groups: &[RangeGroup], theme: &Theme) -> String {
    let flat: Vec<&Segment> = groups.iter().flat_map(|g| g.segments.iter()).collect();
    let (Some(&first), Some(&last)) = (flat.first(), flat.last()) else {
        return String::new();
    };

    let mut out = String::new();
    let first_bg = eff_bg(first, theme);
    if first_bg != &theme.bar_bg {
        write_hard(&mut out, theme, dir, &theme.bar_bg, first_bg);
    }

    let mut prev_bg: Option<&Color> = None;
    let mut open_range = false;
    for group in groups {
        for (i, s) in group.segments.iter().enumerate() {
            let cur_bg = eff_bg(s, theme);
            if let Some(prev_bg) = prev_bg {
                // At a group boundary, close the previous range BEFORE the
                // separator so the separator glyph is not clickable.
                if i == 0 && open_range {
                    out.push_str("#[norange]");
                    open_range = false;
                }
                if prev_bg != cur_bg {
                    write_hard(&mut out, theme, dir, prev_bg, cur_bg);
                } else {
                    let _ = write!(
                        out,
                        "#[fg={},bg={}]{}",
                        theme.soft_fg.to_tmux(),
                        cur_bg.to_tmux(),
                        theme.soft(dir),
                    );
                }
            }
            if i == 0
                && let Some(name) = &group.range
            {
                let _ = write!(out, "#[range=user|{name}]");
                open_range = true;
            }
            let bold = if s.style.bold { ",bold" } else { "" };
            let _ = write!(
                out,
                "#[fg={},bg={}{bold}] {} ",
                eff_fg(s, theme).to_tmux(),
                cur_bg.to_tmux(),
                s.text,
            );
            prev_bg = Some(cur_bg);
        }
    }
    if open_range {
        out.push_str("#[norange]");
    }

    let last_bg = eff_bg(last, theme);
    if last_bg != &theme.bar_bg {
        write_hard(&mut out, theme, dir, last_bg, &theme.bar_bg);
    }
    out.push_str("#[default]");
    out
}

/// Render one window as a self-contained rounded "pill": a left rounded cap, the
/// ` text ` body, and a right rounded cap. The caps are colored
/// `fg=<pill>,bg=<bar_bg>` (the opposite of the pointed powerline separators in
/// [`render_region`]), which is what makes them read as the rounded ends of the
/// pill rather than arrows into the next segment. The active window uses the
/// accent fill + bold; inactive windows use the gray fill.
pub fn render_window_pill(text: &str, is_current: bool, theme: &Theme) -> String {
    let (pill, fg, bold) = if is_current {
        (&theme.win_current_bg, &theme.win_current_fg, ",bold")
    } else {
        (&theme.win_inactive_bg, &theme.win_inactive_fg, "")
    };
    let (pill, fg) = (pill.to_tmux(), fg.to_tmux());
    let bar = theme.bar_bg.to_tmux();
    format!(
        "#[fg={pill},bg={bar}]{cap_l}#[fg={fg},bg={pill}{bold}] {text} #[fg={pill},bg={bar}]{cap_r}#[default]",
        cap_l = theme.win_cap_left,
        cap_r = theme.win_cap_right,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Color, Segment, Style};

    fn theme() -> Theme {
        Theme {
            palette: vec![Color::Indexed(31), Color::Indexed(238)],
            fg: Color::Indexed(255),
            bar_bg: Color::Indexed(234),
            hard_left: "\u{e0b0}".into(),
            hard_right: "\u{e0b2}".into(),
            soft_left: "\u{e0b1}".into(),
            soft_right: "\u{e0b3}".into(),
            soft_fg: Color::Indexed(240),
            win_cap_left: "\u{e0b6}".into(),
            win_cap_right: "\u{e0b4}".into(),
            win_current_bg: Color::Indexed(31),
            win_current_fg: Color::Indexed(255),
            win_inactive_bg: Color::Indexed(236),
            win_inactive_fg: Color::Indexed(250),
            success: Color::Indexed(35),
            info: Color::Indexed(39),
            warning: Color::Indexed(214),
            error: Color::Indexed(196),
        }
    }

    fn seg(text: &str, bg: u8) -> Segment {
        Segment::styled(
            text,
            Style {
                fg: None,
                bg: Some(Color::Indexed(bg)),
                bold: false,
            },
        )
    }

    #[test]
    fn empty_is_empty() {
        assert_eq!(render_region(Direction::Left, &[], &theme()), "");
    }

    #[test]
    fn window_pill_current_is_accent_bold_rounded() {
        let t = theme();
        let out = render_window_pill("1* shell", true, &t);
        assert!(out.contains('\u{e0b6}'), "left rounded cap: {out}");
        assert!(out.contains('\u{e0b4}'), "right rounded cap: {out}");
        assert!(
            out.contains(&format!(
                "#[fg={},bg={}]\u{e0b6}",
                t.win_current_bg.to_tmux(),
                t.bar_bg.to_tmux()
            )),
            "left cap fg=pill,bg=bar: {out}"
        );
        assert!(
            out.contains(&format!(
                "#[fg={},bg={},bold] 1* shell ",
                t.win_current_fg.to_tmux(),
                t.win_current_bg.to_tmux()
            )),
            "current body: {out}"
        );
        assert!(out.ends_with("#[default]"), "ends default: {out}");
    }

    #[test]
    fn window_pill_inactive_is_gray_not_bold() {
        let t = theme();
        let out = render_window_pill("2 editor", false, &t);
        assert!(
            out.contains('\u{e0b6}') && out.contains('\u{e0b4}'),
            "rounded caps: {out}"
        );
        assert!(
            out.contains(&format!(
                "#[fg={},bg={}] 2 editor ",
                t.win_inactive_fg.to_tmux(),
                t.win_inactive_bg.to_tmux()
            )),
            "inactive body, no bold: {out}"
        );
        assert!(!out.contains("bold"), "inactive not bold: {out}");
    }

    #[test]
    fn single_segment_has_text_and_default_reset() {
        let out = render_region(Direction::Left, &[seg("hi", 31)], &theme());
        assert!(out.contains("hi"), "text present: {out}");
        assert!(out.contains("bg=colour31"), "seg bg: {out}");
        assert!(out.ends_with("#[default]"), "reset: {out}");
        // seg bg (31) != bar_bg (234) => trailing edge arrow to bar bg
        assert!(out.contains("\u{e0b0}"), "edge glyph: {out}");
    }

    #[test]
    fn different_bg_uses_hard_separator() {
        let out = render_region(Direction::Left, &[seg("a", 31), seg("b", 238)], &theme());
        // hard separator between them, fg=prev.bg bg=next.bg
        assert!(
            out.contains("#[fg=colour31,bg=colour238]\u{e0b0}"),
            "hard sep: {out}"
        );
    }

    #[test]
    fn same_bg_uses_soft_separator() {
        let out = render_region(Direction::Left, &[seg("a", 31), seg("b", 31)], &theme());
        assert!(
            out.contains("#[fg=colour240,bg=colour31]\u{e0b1}"),
            "soft sep: {out}"
        );
    }

    #[test]
    fn right_direction_uses_right_glyphs() {
        let out = render_region(Direction::Right, &[seg("a", 31), seg("b", 238)], &theme());
        assert!(out.contains("\u{e0b2}"), "right hard glyph: {out}");
    }

    #[test]
    fn bg_equal_bar_bg_has_no_edge_glyph() {
        let s = seg("plain", 234); // == bar_bg
        let out = render_region(Direction::Left, &[s], &theme());
        assert!(
            !out.contains("\u{e0b0}"),
            "no edge glyph when bg==bar_bg: {out}"
        );
        assert!(out.contains("plain"));
    }

    #[test]
    fn right_leading_edge_blends_bar_bg_into_first_segment() {
        // A Right region's outer edge is on its LEFT: a right-facing hard glyph
        // (U+E0B2) transitioning bar_bg -> first-segment.bg. With the Right
        // fg/bg mirror the glyph is colored fg=first.bg, bg=bar_bg, and it is
        // the very first thing emitted (the leading edge).
        let out = render_region(Direction::Right, &[seg("a", 31)], &theme());
        assert!(
            out.starts_with("#[fg=colour31,bg=colour234]\u{e0b2}"),
            "right leading edge (bar_bg -> first.bg): {out}"
        );
        // A bg matching bar_bg must still produce no leading edge glyph.
        let plain = render_region(Direction::Right, &[seg("p", 234)], &theme());
        assert!(
            !plain.contains("\u{e0b2}"),
            "no right edge glyph when bg==bar_bg: {plain}"
        );
    }

    #[test]
    fn right_multi_segment_trailing_edge_blends_last_into_bar() {
        // A Right region with two segments (bg 31 then 238): the far-right
        // trailing edge is a right-facing hard glyph (U+E0B2) from the LAST
        // segment's bg (238) back into bar_bg (234). With the Right fg/bg mirror
        // it is colored fg=bar_bg(234), bg=last(238), and it is the final thing
        // emitted before the `#[default]` reset.
        let out = render_region(Direction::Right, &[seg("a", 31), seg("b", 238)], &theme());
        assert!(
            out.ends_with("#[fg=colour234,bg=colour238]\u{e0b2}#[default]"),
            "right trailing edge (last.bg -> bar_bg): {out}"
        );
    }

    fn group(range: Option<&str>, segs: Vec<Segment>) -> RangeGroup {
        RangeGroup {
            range: range.map(str::to_string),
            segments: segs,
        }
    }

    #[test]
    fn ranged_wraps_clickable_group_and_leaves_separators_outside() {
        let groups = vec![
            group(Some("cpu"), vec![seg("a", 31)]),
            group(None, vec![seg("b", 238)]),
        ];
        let out = render_region_ranged(Direction::Left, &groups, &theme());
        // clickable group is bracketed; non-clickable group is not.
        assert!(out.contains("#[range=user|cpu]"), "opens range: {out}");
        assert!(out.contains("#[norange]"), "closes range: {out}");
        // the hard separator between the two groups sits OUTSIDE the range
        // (norange precedes the separator glyph).
        let sep = "#[fg=colour31,bg=colour238]\u{e0b0}";
        let nr = out.find("#[norange]").unwrap();
        let sp = out.find(sep).unwrap();
        assert!(nr < sp, "norange before separator: {out}");
    }

    #[test]
    fn ranged_all_none_is_byte_identical_to_render_region() {
        let segs = vec![seg("a", 31), seg("b", 238)];
        let groups = vec![
            group(None, vec![segs[0].clone()]),
            group(None, vec![segs[1].clone()]),
        ];
        assert_eq!(
            render_region_ranged(Direction::Left, &groups, &theme()),
            render_region(Direction::Left, &segs, &theme()),
        );
    }

    #[test]
    fn ranged_stripping_range_tokens_equals_render_region() {
        // Non-destructive: with a clickable group, stripping the range tokens
        // reproduces the plain powerline output.
        let segs = vec![seg("a", 31), seg("b", 238)];
        let groups = vec![
            group(Some("cpu"), vec![segs[0].clone()]),
            group(Some("memory"), vec![segs[1].clone()]),
        ];
        let ranged = render_region_ranged(Direction::Left, &groups, &theme());
        let stripped = ranged
            .replace("#[range=user|cpu]", "")
            .replace("#[range=user|memory]", "")
            .replace("#[norange]", "");
        assert_eq!(stripped, render_region(Direction::Left, &segs, &theme()));
    }

    #[test]
    fn ranged_empty_is_empty() {
        assert_eq!(render_region_ranged(Direction::Left, &[], &theme()), "");
    }

    #[test]
    fn ranged_multisegment_widget_keeps_internal_separator_inside_range() {
        // A single clickable widget that renders MORE THAN ONE segment (e.g. a
        // future multi-part widget) must keep its own internal separator
        // between the range-open and range-close tokens, so the whole widget
        // — including the glyph joining its parts — is one clickable range.
        let groups = vec![group(Some("multi"), vec![seg("a", 31), seg("b", 31)])];
        let out = render_region_ranged(Direction::Left, &groups, &theme());
        let open = out.find("#[range=user|multi]").unwrap();
        let close = out.find("#[norange]").unwrap();
        // same bg (31, 31) => the two segments are joined by a SOFT separator.
        let sep = out.find(&theme().soft_left).unwrap();
        assert!(
            open < sep && sep < close,
            "internal separator inside range: {out}"
        );
    }

    #[test]
    fn theme_default_has_semantic_colors_and_colors_bundle() {
        let t = Theme::default();
        assert_eq!(t.success, Color::Indexed(35));
        assert_eq!(t.info, Color::Indexed(39));
        assert_eq!(t.warning, Color::Indexed(214));
        assert_eq!(t.error, Color::Indexed(196));
        let c = t.colors();
        assert_eq!(c.fg, t.fg);
        assert_eq!(c.bar_bg, t.bar_bg);
        assert_eq!(c.success, t.success);
        assert_eq!(c.info, t.info);
        assert_eq!(c.warning, t.warning);
        assert_eq!(c.error, t.error);
    }
}
