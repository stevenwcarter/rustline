use crate::{Context, Segment, Widget};

/// Renders the pane's current working directory.
///
/// When [`Cwd::abbreviate_home`] is set (the default), a leading `$HOME`
/// path component is replaced with `~`, matching the shorthand shells and
/// prompts commonly use.
pub struct Cwd {
    pub abbreviate_home: bool,
}

impl Default for Cwd {
    fn default() -> Self {
        Self {
            abbreviate_home: true,
        }
    }
}

impl Widget for Cwd {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let path = ctx.pane_current_path.as_str();
        let text = if self.abbreviate_home {
            abbreviate_home(path, &ctx.home)
        } else {
            path.to_string()
        };
        vec![Segment::new(text)]
    }
}

/// Replace a leading `home` path component of `path` with `~`.
///
/// A plain [`str::strip_prefix`] isn't sufficient on its own: the string
/// `/home/steve2` starts with `/home/steve` without actually being under
/// that directory, so the stripped remainder must be empty or start with
/// `/` for the abbreviation to apply.
fn abbreviate_home(path: &str, home: &str) -> String {
    if home.is_empty() {
        return path.to_string();
    }
    match path.strip_prefix(home) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
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
        }
    }

    #[test]
    fn cwd_abbreviates_home() {
        assert_eq!(Cwd::default().render(&ctx())[0].text, "~/src/rustline");
    }

    #[test]
    fn cwd_no_abbrev_when_disabled() {
        let w = Cwd {
            abbreviate_home: false,
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
}
