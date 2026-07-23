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
    use extism_pdk::*;
    use rustline_abi::{GuestRender, Segment};
    use serde_json::Value;

    #[host_fn]
    extern "ExtismHost" {
        fn rl_file_read(path: String) -> String;
        fn rl_log(level: String, msg: String) -> String;
    }

    #[plugin_fn]
    pub fn name() -> FnResult<String> {
        Ok("filewatch".to_string())
    }

    #[plugin_fn]
    pub fn render(input: String) -> FnResult<Json<Vec<Segment>>> {
        // A malformed input degrades to an empty render (never break the
        // bar) rather than erroring.
        let Ok(input) = serde_json::from_str::<GuestRender>(&input) else {
            return Ok(Json(Vec::new()));
        };
        let cfg = &input.config;
        let path = cfg["path"].as_str().unwrap_or("");
        if path.is_empty() {
            // Nothing configured to watch.
            return Ok(Json(Vec::new()));
        }
        let format = cfg["format"].as_str().unwrap_or("{first_line} ({lines}L)");
        let down_format = cfg["down_format"].as_str().unwrap_or("");

        Ok(Json(match read_file(path) {
            Some(contents) => vec![Segment::new(render_format(
                format,
                &summarize(&contents),
                path,
            ))],
            None => down_segment(down_format),
        }))
    }

    /// Read `path` via `rl_file_read`. Any denial (not in `allowed_paths`),
    /// missing file, host-call error, or malformed JSON logs why (via
    /// `rl_log`) and returns `None` rather than erroring.
    fn read_file(path: &str) -> Option<String> {
        let call = unsafe { rl_file_read(path.to_string()) };
        let raw = match call {
            Ok(raw) => raw,
            Err(error) => {
                let _ = unsafe {
                    rl_log(
                        "warn".to_string(),
                        format!("filewatch: host call failed for {path}: {error}"),
                    )
                };
                return None;
            }
        };
        let result: Value = serde_json::from_str(&raw).ok()?;
        if result["ok"].as_bool().unwrap_or(false) && result["exists"].as_bool().unwrap_or(false) {
            Some(result["contents"].as_str().unwrap_or_default().to_string())
        } else {
            let reason = result["error"].as_str().unwrap_or("file missing");
            let _ = unsafe {
                rl_log(
                    "warn".to_string(),
                    format!("filewatch: {path} unavailable: {reason}"),
                )
            };
            None
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
