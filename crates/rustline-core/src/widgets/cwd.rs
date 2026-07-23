use crate::{Context, Segment, Widget};

/// Renders the pane's current working directory, with optional shortening
/// and a `format` template.
///
/// When [`Cwd::abbreviate_home`] is set (the default), a leading `$HOME`
/// path component is replaced with `~`, matching the shorthand shells and
/// prompts commonly use. [`Cwd::abbreviate`] additionally shortens every
/// path component but the last to its first character, fish-shell style
/// (e.g. `~/src/rustline` becomes `~/s/rustline`). [`Cwd::max_depth`] (`0` =
/// unlimited) then keeps only the last N `/`-separated components, prefixing
/// a leading `…/` when components were dropped. [`Cwd::max_len`] (`0` =
/// unlimited) then left-truncates the result to at most N characters,
/// prefixing a leading `…`. The final string is substituted into
/// [`Cwd::format`]'s `{path}` placeholder (default `"{path}"`, i.e. the path
/// verbatim); any other text in `format` (e.g. a Nerd-Font icon or label) is
/// emitted verbatim, and unknown placeholders pass through untouched.
///
/// These transforms apply in that fixed order — home-abbreviation,
/// `abbreviate`, `max_depth`, `max_len`, then the `format` substitution — so
/// with every default (`format = "{path}"`, `max_depth = 0`, `max_len = 0`,
/// `abbreviate = false`), rendering is byte-identical to before this
/// feature.
pub struct Cwd {
    pub abbreviate_home: bool,
    pub format: String,
    pub max_depth: usize,
    pub max_len: usize,
    pub abbreviate: bool,
}

impl Default for Cwd {
    fn default() -> Self {
        Self {
            abbreviate_home: true,
            format: "{path}".into(),
            max_depth: 0,
            max_len: 0,
            abbreviate: false,
        }
    }
}

impl Widget for Cwd {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let path = ctx.pane_current_path.as_str();
        let mut text = if self.abbreviate_home {
            abbreviate_home(path, &ctx.home)
        } else {
            path.to_string()
        };
        if self.abbreviate {
            text = abbreviate_components(&text);
        }
        text = limit_depth(&text, self.max_depth);
        text = limit_len(&text, self.max_len);
        vec![Segment::new(self.format.replace("{path}", &text))]
    }
}

/// Replace a leading `home` path component of `path` with `~`.
///
/// A plain [`str::strip_prefix`] isn't sufficient on its own: the string
/// `/home/steve2` starts with `/home/steve` without actually being under
/// that directory, so the stripped remainder must be empty or start with
/// `/` for the abbreviation to apply.
///
/// `home` is normalized by trimming a trailing `/` first, so a
/// trailing-slash `$HOME` (e.g. `/home/steve/`) still matches genuine
/// subdirectories. Trimming a bare `/` yields an empty string, which falls
/// through to the empty-home guard below and leaves `path` unchanged rather
/// than abbreviating the root.
fn abbreviate_home(path: &str, home: &str) -> String {
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        return path.to_string();
    }
    match path.strip_prefix(home) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

/// Fish-shell-style shortening: every `/`-separated component except the
/// last is reduced to its first `char` (not byte, so multi-byte glyphs
/// aren't split). An empty component — from a leading `/` on an absolute
/// path — stays empty, preserving the leading slash/`~` marker.
fn abbreviate_components(path: &str) -> String {
    let components: Vec<&str> = path.split('/').collect();
    let last = components.len().saturating_sub(1);
    components
        .iter()
        .enumerate()
        .map(|(i, comp)| {
            if i == last {
                return comp.to_string();
            }
            match comp.chars().next() {
                Some(c) => c.to_string(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Keep only the last `max_depth` `/`-separated components of `text`,
/// prefixing a leading `…/` when components were dropped. `0` (the default)
/// means unlimited — a no-op.
///
/// An absolute path's leading `/` produces an empty leading component
/// (`"/etc".split('/') == ["", "etc"]`); that empty component is excluded
/// from the `max_depth` comparison so it isn't charged against the limit,
/// mirroring [`abbreviate_components`]'s treatment of the same empty
/// component.
fn limit_depth(text: &str, max_depth: usize) -> String {
    if max_depth == 0 {
        return text.to_string();
    }
    let all: Vec<&str> = text.split('/').collect();
    let has_leading_slash = all.first().is_some_and(|c| c.is_empty());
    let real = if has_leading_slash {
        &all[1..]
    } else {
        &all[..]
    };
    if real.len() <= max_depth {
        return text.to_string();
    }
    format!("…/{}", real[real.len() - max_depth..].join("/"))
}

/// Left-truncate `text` to at most `max_len` characters, prefixing a
/// leading `…`. Counts `char`s, not bytes, so multi-byte glyphs aren't
/// split. `0` (the default) means unlimited — a no-op.
fn limit_len(text: &str, max_len: usize) -> String {
    if max_len == 0 {
        return text.to_string();
    }
    let char_count = text.chars().count();
    if char_count <= max_len {
        return text.to_string();
    }
    let keep = max_len.saturating_sub(1);
    let tail: String = text.chars().skip(char_count - keep).collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use crate::{Context, Widget};
    use chrono::{Local, TimeZone};

    use super::Cwd;

    fn ctx() -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/home/steve/src/rustline".into(),
            home: "/home/steve".into(),
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
            os: String::new(),
            arch: String::new(),
            toggled: Default::default(),
            colors: Default::default(),
        }
    }

    #[test]
    fn cwd_default_unchanged() {
        // Characterization: every new option at its default reproduces the
        // pre-feature output byte-for-byte.
        assert_eq!(Cwd::default().render(&ctx())[0].text, "~/src/rustline");
    }

    #[test]
    fn cwd_abbreviates_home() {
        assert_eq!(Cwd::default().render(&ctx())[0].text, "~/src/rustline");
    }

    #[test]
    fn cwd_no_abbrev_when_disabled() {
        let w = Cwd {
            abbreviate_home: false,
            ..Default::default()
        };
        assert_eq!(w.render(&ctx())[0].text, "/home/steve/src/rustline");
    }

    #[test]
    fn cwd_unchanged_outside_home() {
        let c = Context {
            pane_current_path: "/etc".into(),
            ..ctx()
        };
        assert_eq!(Cwd::default().render(&c)[0].text, "/etc");
    }

    #[test]
    fn cwd_unchanged_for_sibling_prefix() {
        let c = Context {
            pane_current_path: "/home/steve2/foo".into(),
            ..ctx()
        };
        assert_eq!(Cwd::default().render(&c)[0].text, "/home/steve2/foo");
    }

    #[test]
    fn cwd_abbreviates_with_trailing_slash_home() {
        let c = Context {
            home: "/home/steve/".into(),
            pane_current_path: "/home/steve/src".into(),
            ..ctx()
        };
        assert_eq!(Cwd::default().render(&c)[0].text, "~/src");
    }

    #[test]
    fn cwd_abbreviate_shortens_components() {
        let c = Context {
            pane_current_path: "/home/steve/src/really/rustline".into(),
            ..ctx()
        };
        let w = Cwd {
            abbreviate: true,
            ..Default::default()
        };
        // "~/src/really/rustline" -> every component but the last shrinks to
        // its first char.
        assert_eq!(w.render(&c)[0].text, "~/s/r/rustline");
    }

    #[test]
    fn cwd_max_depth_keeps_last_n_with_ellipsis() {
        let w = Cwd {
            max_depth: 2,
            ..Default::default()
        };
        // "~/src/rustline" has 3 components; keeping the last 2 drops "~".
        assert_eq!(w.render(&ctx())[0].text, "…/src/rustline");
    }

    #[test]
    fn cwd_max_depth_noop_when_not_exceeded() {
        let w = Cwd {
            max_depth: 10,
            ..Default::default()
        };
        assert_eq!(w.render(&ctx())[0].text, "~/src/rustline");
    }

    #[test]
    fn cwd_max_depth_preserves_absolute_leading_slash_when_fits() {
        // An absolute path's leading `/` must not be charged against
        // max_depth: "/etc".split('/') == ["", "etc"] has exactly 1 real
        // component, so it fits max_depth=1 unchanged.
        let w = Cwd {
            abbreviate_home: false,
            max_depth: 1,
            ..Default::default()
        };
        let c = Context {
            pane_current_path: "/etc".into(),
            ..ctx()
        };
        assert_eq!(w.render(&c)[0].text, "/etc");

        let w2 = Cwd {
            abbreviate_home: false,
            max_depth: 2,
            ..Default::default()
        };
        let c2 = Context {
            pane_current_path: "/var/log".into(),
            ..ctx()
        };
        assert_eq!(w2.render(&c2)[0].text, "/var/log");
    }

    #[test]
    fn cwd_max_depth_truncates_absolute_path() {
        // 3 real components ("var", "log", "nginx") exceeds max_depth=2, so
        // the last 2 are kept behind the usual ellipsis prefix.
        let w = Cwd {
            abbreviate_home: false,
            max_depth: 2,
            ..Default::default()
        };
        let c = Context {
            pane_current_path: "/var/log/nginx".into(),
            ..ctx()
        };
        assert_eq!(w.render(&c)[0].text, "…/log/nginx");
    }

    #[test]
    fn cwd_max_len_left_truncates() {
        let w = Cwd {
            max_len: 6,
            ..Default::default()
        };
        // "~/src/rustline" (14 chars) truncated to 6: "…" + last 5 chars.
        assert_eq!(w.render(&ctx())[0].text, "…tline");
    }

    #[test]
    fn cwd_max_len_noop_when_not_exceeded() {
        let w = Cwd {
            max_len: 100,
            ..Default::default()
        };
        assert_eq!(w.render(&ctx())[0].text, "~/src/rustline");
    }

    #[test]
    fn cwd_format_wraps_path() {
        let w = Cwd {
            format: "\u{f07c} {path}".into(),
            ..Default::default()
        };
        assert_eq!(w.render(&ctx())[0].text, "\u{f07c} ~/src/rustline");
    }
}
