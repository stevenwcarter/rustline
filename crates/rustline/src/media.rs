//! Now-playing media read, isolated at the `Context`-build edge, mirroring
//! `git.rs`: `read_media` shells out to `playerctl` and `parse_playerctl` is a
//! pure parser, unit-tested independently of any real media player.

use rustline_core::MediaInfo;

/// Parse `playerctl metadata --format '{{artist}}\t{{title}}\t{{status}}'`
/// stdout (tab-separated `artist`/`title`/`status`, one line) into a
/// [`MediaInfo`]. Pure; unit-tested directly against fixture text.
///
/// A blank first line, or a line with neither an artist nor a title, yields
/// `None` — never a fabricated "not playing" reading (invariant #6). Missing
/// individual fields (e.g. no artist tag) are tolerated as empty strings, so
/// `Some` is still returned as long as either `artist` or `title` is present.
pub(crate) fn parse_playerctl(s: &str) -> Option<MediaInfo> {
    let line = s.lines().next()?;
    if line.trim().is_empty() {
        return None;
    }
    let mut fields = line.splitn(3, '\t');
    let artist = fields.next().unwrap_or("").to_string();
    let title = fields.next().unwrap_or("").to_string();
    let status = fields.next().unwrap_or("").to_string();
    if artist.is_empty() && title.is_empty() {
        return None;
    }
    Some(MediaInfo {
        artist,
        title,
        status,
    })
}

/// Read the current now-playing media via `playerctl metadata`, or `None` on
/// any failure: `playerctl` missing from `PATH`, no player running, or a
/// non-zero exit — never a fabricated "not playing" reading (invariant #6).
/// Called once at Context-build time, only when the `media` widget is in the
/// active layout (see `build_context.rs`).
#[cfg(target_os = "linux")]
pub fn read_media() -> Option<MediaInfo> {
    let output = std::process::Command::new("playerctl")
        .args(["metadata", "--format", "{{artist}}\t{{title}}\t{{status}}"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_playerctl(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(not(target_os = "linux"))]
pub fn read_media() -> Option<MediaInfo> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_tab_separated() {
        let m = parse_playerctl("Radiohead\tKarma Police\tPlaying\n").unwrap();
        assert_eq!(m.artist, "Radiohead");
        assert_eq!(m.title, "Karma Police");
        assert_eq!(m.status, "Playing");
        assert!(parse_playerctl("").is_none());
        // Missing fields tolerated as empty strings, still Some if a title exists.
        let m2 = parse_playerctl("\tOnly Title\t").unwrap();
        assert_eq!(m2.artist, "");
        assert_eq!(m2.title, "Only Title");
    }

    #[test]
    fn blank_line_is_none() {
        assert!(parse_playerctl("\n").is_none());
        assert!(parse_playerctl("   \n").is_none());
    }

    #[test]
    fn read_media_never_panics() {
        // Host-dependent (requires `playerctl` on PATH and a running player);
        // only assert it does not panic.
        let _ = read_media();
    }
}
