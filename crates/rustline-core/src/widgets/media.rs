use crate::{Context, Segment, Widget};

/// Renders the current now-playing track (artist/title/status) from
/// `Context::media`. Pure — reads only `Context::media`.
///
/// `Context::media` is `None` when `playerctl` is missing, no player is
/// running, or the read failed; the widget then renders nothing (or
/// `down_format`) rather than faking a "not playing" reading (invariant #6).
/// Part of the format-bearing widget family: a non-empty `alt_format` makes
/// it click-toggleable.
pub struct Media {
    pub format: String,
    pub alt_format: String,
    pub down_format: String,
}

impl Media {
    /// Registry/layout name; the toggle key threaded through render + click.
    pub const NAME: &'static str = "media";
}

impl Widget for Media {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        match &ctx.media {
            Some(info) => {
                let fmt =
                    crate::widgets::active_format(ctx, Self::NAME, &self.format, &self.alt_format);
                let text = fmt
                    .replace("{artist}", &info.artist)
                    .replace("{title}", &info.title)
                    .replace("{status}", &info.status);
                vec![Segment::new(text)]
            }
            None => {
                if self.down_format.is_empty() {
                    return vec![];
                }
                // Collapse every placeholder so a stray token never renders
                // and no fake track shows (invariant #6).
                let text = self
                    .down_format
                    .replace("{artist}", "")
                    .replace("{title}", "")
                    .replace("{status}", "");
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
    use crate::MediaInfo;
    use chrono::{Local, TimeZone};

    fn ctx(media: Option<MediaInfo>) -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/".into(),
            home: "/h".into(),
            hostname: "h".into(),
            loadavg: None,
            now: Local
                .with_ymd_and_hms(2026, 7, 23, 12, 0, 0)
                .single()
                .unwrap(),
            window: None,
            interfaces: Vec::new(),
            battery: None,
            cpu: None,
            memory: None,
            git: None,
            disk: None,
            throughput: None,
            uptime: None,
            media,
            os: String::new(),
            arch: String::new(),
            toggled: Default::default(),
            colors: Default::default(),
        }
    }

    fn info(artist: &str, title: &str, status: &str) -> Option<MediaInfo> {
        Some(MediaInfo {
            artist: artist.into(),
            title: title.into(),
            status: status.into(),
        })
    }

    fn w(format: &str, alt: &str, down: &str) -> Media {
        Media {
            format: format.into(),
            alt_format: alt.into(),
            down_format: down.into(),
        }
    }

    #[test]
    fn renders_default_format() {
        let out = w("{title} — {artist}", "", "").render(&ctx(info(
            "Radiohead",
            "Karma Police",
            "Playing",
        )));
        assert_eq!(out[0].text, "Karma Police — Radiohead");
    }

    #[test]
    fn status_placeholder_substitutes() {
        let out = w("{status}: {title}", "", "").render(&ctx(info(
            "Radiohead",
            "Karma Police",
            "Paused",
        )));
        assert_eq!(out[0].text, "Paused: Karma Police");
    }

    #[test]
    fn none_empty_down_renders_nothing() {
        assert!(w("{title}", "", "").render(&ctx(None)).is_empty());
    }

    #[test]
    fn none_down_format_collapses_placeholders() {
        let out = w("{title}", "", "no media {artist}{title}{status}").render(&ctx(None));
        assert_eq!(out[0].text, "no media ");
    }

    #[test]
    fn toggled_uses_alt_format() {
        let mut c = ctx(info("Radiohead", "Karma Police", "Playing"));
        c.toggled.insert("media".to_string());
        let out = w("{title}", "{artist} - {title}", "").render(&c);
        assert_eq!(out[0].text, "Radiohead - Karma Police");
        // untoggled -> normal format
        let out = w("{title}", "{artist} - {title}", "").render(&ctx(info(
            "Radiohead",
            "Karma Police",
            "Playing",
        )));
        assert_eq!(out[0].text, "Karma Police");
    }

    #[test]
    fn range_name_some_only_with_alt_format() {
        assert_eq!(w("{title}", "{artist}", "").range_name(), Some("media"));
        assert_eq!(w("{title}", "", "").range_name(), None);
    }
}
