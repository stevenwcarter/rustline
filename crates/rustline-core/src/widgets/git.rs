use crate::{Context, Segment, Widget};

/// Renders the current git branch (or short SHA when `HEAD` is detached) plus
/// a dirty marker and ahead/behind/staged/unstaged counts for the pane's
/// working directory. Pure — reads only `Context::git`.
///
/// `Context::git` is `None` when `git` is missing, the pane isn't inside a
/// repository, or the read failed; the widget then renders nothing (or
/// `down_format`) rather than faking a clean status (invariant #6). Part of
/// the format-bearing widget family: a non-empty `alt_format` makes it
/// click-toggleable.
pub struct GitWidget {
    pub format: String,
    pub alt_format: String,
    pub down_format: String,
    /// Substituted for `{dirty}` when the repo has any staged or unstaged
    /// change; the empty string when clean.
    pub dirty_glyph: String,
}

impl GitWidget {
    /// Registry/layout name; the toggle key threaded through render + click.
    pub const NAME: &'static str = "git";
}

impl Widget for GitWidget {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        match &ctx.git {
            Some(info) => {
                let fmt =
                    crate::widgets::active_format(ctx, Self::NAME, &self.format, &self.alt_format);
                let dirty = if info.staged > 0 || info.unstaged > 0 {
                    self.dirty_glyph.as_str()
                } else {
                    ""
                };
                let text = fmt
                    .replace("{branch}", &info.branch)
                    .replace("{ahead}", &info.ahead.to_string())
                    .replace("{behind}", &info.behind.to_string())
                    .replace("{staged}", &info.staged.to_string())
                    .replace("{unstaged}", &info.unstaged.to_string())
                    .replace("{dirty}", dirty);
                vec![Segment::new(text)]
            }
            None => {
                if self.down_format.is_empty() {
                    return vec![];
                }
                // Collapse every placeholder so a stray token never renders
                // and no fake status shows (invariant #6).
                let text = self
                    .down_format
                    .replace("{branch}", "")
                    .replace("{ahead}", "")
                    .replace("{behind}", "")
                    .replace("{staged}", "")
                    .replace("{unstaged}", "")
                    .replace("{dirty}", "");
                vec![Segment::new(text)]
            }
        }
    }

    fn range_name(&self) -> Option<&str> {
        crate::widgets::clickable_range(Self::NAME, &self.alt_format)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GitInfo;
    use chrono::{Local, TimeZone};

    fn ctx(git: Option<GitInfo>) -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/".into(),
            home: "/h".into(),
            hostname: "h".into(),
            loadavg: None,
            now: Local
                .with_ymd_and_hms(2026, 7, 22, 12, 0, 0)
                .single()
                .unwrap(),
            window: None,
            interfaces: Vec::new(),
            battery: None,
            cpu: None,
            memory: None,
            git,
            os: String::new(),
            arch: String::new(),
            toggled: Default::default(),
            colors: Default::default(),
        }
    }

    fn info(branch: &str, ahead: u32, behind: u32, staged: u32, unstaged: u32) -> Option<GitInfo> {
        Some(GitInfo {
            branch: branch.into(),
            ahead,
            behind,
            staged,
            unstaged,
        })
    }

    fn w(format: &str, alt: &str, down: &str) -> GitWidget {
        GitWidget {
            format: format.into(),
            alt_format: alt.into(),
            down_format: down.into(),
            dirty_glyph: "*".into(),
        }
    }

    #[test]
    fn renders_default_format_clean() {
        let out = w("\u{e0a0} {branch}{dirty}", "", "").render(&ctx(info("main", 0, 0, 0, 0)));
        assert_eq!(out[0].text, "\u{e0a0} main");
    }

    #[test]
    fn dirty_glyph_shown_when_staged_or_unstaged() {
        let out = w("{branch}{dirty}", "", "").render(&ctx(info("main", 0, 0, 1, 0)));
        assert_eq!(out[0].text, "main*");
        let out = w("{branch}{dirty}", "", "").render(&ctx(info("main", 0, 0, 0, 2)));
        assert_eq!(out[0].text, "main*");
    }

    #[test]
    fn dirty_glyph_configurable() {
        let mut widget = w("{branch}{dirty}", "", "");
        widget.dirty_glyph = "!".into();
        let out = widget.render(&ctx(info("main", 0, 0, 1, 0)));
        assert_eq!(out[0].text, "main!");
    }

    #[test]
    fn ahead_behind_staged_unstaged_placeholders() {
        let out = w("{branch} +{ahead}-{behind} s{staged}u{unstaged}", "", "")
            .render(&ctx(info("main", 2, 1, 3, 4)));
        assert_eq!(out[0].text, "main +2-1 s3u4");
    }

    #[test]
    fn none_empty_down_renders_nothing() {
        assert!(w("{branch}", "", "").render(&ctx(None)).is_empty());
    }

    #[test]
    fn none_down_format_collapses_placeholders() {
        let out = w("{branch}", "", "no repo {branch}{dirty}").render(&ctx(None));
        assert_eq!(out[0].text, "no repo ");
    }

    #[test]
    fn toggled_uses_alt_format() {
        let mut c = ctx(info("main", 0, 0, 1, 0));
        c.toggled.insert("git".to_string());
        let out = w("{branch}", "{branch}{dirty}", "").render(&c);
        assert_eq!(out[0].text, "main*");
        // untoggled -> normal format
        let out = w("{branch}", "{branch}{dirty}", "").render(&ctx(info("main", 0, 0, 1, 0)));
        assert_eq!(out[0].text, "main");
    }

    #[test]
    fn range_name_some_only_with_alt_format() {
        assert_eq!(
            w("{branch}", "{branch}{dirty}", "").range_name(),
            Some("git")
        );
        assert_eq!(w("{branch}", "", "").range_name(), None);
    }
}
