//! rustline `filewatch` plugin: reads a configured file (via the host's
//! `rl_file_read` capability, gated by the user's `allowed_paths`) and
//! renders a one-line summary — first line plus total line count. Contrasts
//! with `weather`'s network capability and `counter`'s state capability by
//! demonstrating the arbitrary-file-read capability instead.
//!
//! Pure logic lives here and is unit-tested on the host target (`cargo
//! test`); the Extism guest glue below only compiles for wasm32. See
//! `plugins/weather` in the rustline repo for the cached-HTTP worked
//! example.

/// A file's summary: its first line and total line count.
#[derive(Debug, PartialEq, Eq)]
pub struct Summary {
    pub first_line: String,
    pub lines: usize,
}

/// Summarize a file's contents: the first line (never including its
/// trailing newline) and the total number of lines. Empty contents
/// summarize as an empty first line and zero lines.
pub fn summarize(contents: &str) -> Summary {
    Summary {
        first_line: contents.lines().next().unwrap_or_default().to_string(),
        lines: contents.lines().count(),
    }
}

/// Substitute `{first_line}`, `{lines}`, and `{path}` in `format`. Unknown
/// placeholders pass through untouched.
pub fn render_format(format: &str, summary: &Summary, path: &str) -> String {
    format
        .replace("{first_line}", &summary.first_line)
        .replace("{lines}", &summary.lines.to_string())
        .replace("{path}", path)
}

#[cfg(target_arch = "wasm32")]
mod guest {
    use super::*;
    use rustline_plugin_sdk::{GuestRender, LogLevel, Segment, export_plugin, file_read, log};

    fn render(input: &GuestRender) -> Vec<Segment> {
        let cfg = &input.config;
        let path = cfg["path"].as_str().unwrap_or("");
        if path.is_empty() {
            // Nothing configured to watch.
            return Vec::new();
        }
        let format = cfg["format"].as_str().unwrap_or("{first_line} ({lines}L)");
        let down_format = cfg["down_format"].as_str().unwrap_or("");

        match read_file(path) {
            Some(contents) => vec![Segment::new(render_format(
                format,
                &summarize(&contents),
                path,
            ))],
            None => down_segment(down_format),
        }
    }

    /// Read `path` via the SDK's `file_read`. Any denial (not in
    /// `allowed_paths`), missing file, or host-call error logs why (via `log`)
    /// and returns `None` rather than erroring.
    fn read_file(path: &str) -> Option<String> {
        match file_read(path) {
            Ok(r) if r.ok && r.exists => Some(r.contents),
            Ok(r) => {
                let reason = if r.error.is_empty() {
                    "file missing"
                } else {
                    r.error.as_str()
                };
                log(
                    LogLevel::Warn,
                    &format!("filewatch: {path} unavailable: {reason}"),
                );
                None
            }
            Err(error) => {
                log(
                    LogLevel::Warn,
                    &format!("filewatch: host call failed for {path}: {error}"),
                );
                None
            }
        }
    }

    /// The unavailable-file view: `down_format` verbatim, or no segment at
    /// all when it's empty — same collapse-to-nothing convention the
    /// built-in widgets' `down_format` follows.
    fn down_segment(down_format: &str) -> Vec<Segment> {
        if down_format.is_empty() {
            Vec::new()
        } else {
            vec![Segment::new(down_format.to_string())]
        }
    }

    export_plugin!(name: "filewatch", render: render);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_empty_contents() {
        let s = summarize("");
        assert_eq!(s.first_line, "");
        assert_eq!(s.lines, 0);
    }

    #[test]
    fn summarize_single_line_no_trailing_newline() {
        let s = summarize("hello world");
        assert_eq!(s.first_line, "hello world");
        assert_eq!(s.lines, 1);
    }

    #[test]
    fn summarize_multiple_lines_counts_and_extracts_first() {
        let s = summarize("first\nsecond\nthird\n");
        assert_eq!(s.first_line, "first");
        assert_eq!(s.lines, 3);
    }

    #[test]
    fn render_format_substitutes_placeholders_and_passes_unknowns() {
        let s = Summary {
            first_line: "hi".to_string(),
            lines: 3,
        };
        assert_eq!(
            render_format("{first_line} ({lines}L) @{path}", &s, "/tmp/x"),
            "hi (3L) @/tmp/x"
        );
        assert_eq!(render_format("{bogus}", &s, "/tmp/x"), "{bogus}");
    }
}
