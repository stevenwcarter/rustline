//! rustline `httpget` plugin: a plain (uncached) `rl_http_get` widget that
//! fetches a configured URL and renders a snippet of the body — contrasting
//! with `weather`'s TTL-cached `rl_http_get_cached` path. Because this fires
//! a real request on every render, it's meant as a worked example of the
//! plain capability, not a widget you'd actually want on a fast
//! `status-interval`.
//!
//! Pure logic lives here and is unit-tested on the host target (`cargo
//! test`); the Extism guest glue below only compiles for wasm32. See
//! `plugins/weather` in the rustline repo for the cached worked example.

/// Extract a short, status-line-safe snippet from a fetched body: the first
/// non-blank line, truncated to `max_chars` characters (never splitting a
/// multi-byte character), with a trailing `…` when truncated.
pub fn extract_snippet(body: &str, max_chars: usize) -> String {
    let first_line = body
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let truncated: String = first_line.chars().take(max_chars).collect();
    if first_line.chars().count() > max_chars {
        format!("{truncated}\u{2026}")
    } else {
        truncated
    }
}

/// Substitute the `{body}` placeholder in `format`. Unknown placeholders
/// pass through untouched.
pub fn render_format(format: &str, snippet: &str) -> String {
    format.replace("{body}", snippet)
}

#[cfg(target_arch = "wasm32")]
mod guest {
    use super::*;
    use rustline_plugin_sdk::{GuestRender, LogLevel, Segment, export_plugin, http_get, log};

    fn render(input: &GuestRender) -> Vec<Segment> {
        let cfg = &input.config;
        let url = cfg["url"].as_str().unwrap_or("");
        if url.is_empty() {
            // Nothing configured to fetch.
            return Vec::new();
        }
        let format = cfg["format"].as_str().unwrap_or("{body}");
        let max_chars = cfg["max_chars"].as_u64().unwrap_or(40) as usize;
        let down_format = cfg["down_format"].as_str().unwrap_or("");

        match fetch_body(url) {
            Some(body) => vec![Segment::new(render_format(
                format,
                &extract_snippet(&body, max_chars),
            ))],
            None => down_segment(down_format),
        }
    }

    /// Plain (uncached) GET via the SDK's `http_get`. `ok` only means the
    /// transport completed — a non-2xx status is still `ok`, so this checks the
    /// status range itself (unlike `weather`'s cached path, where the host
    /// already restricts caching to 2xx responses). Any denial (not in
    /// `allowed_urls`), transport error, or non-2xx status logs why (via `log`)
    /// and returns `None`.
    fn fetch_body(url: &str) -> Option<String> {
        match http_get(url) {
            Ok(r) if r.ok && (200u16..300).contains(&r.status) => Some(r.body),
            Ok(r) => {
                let reason = r.error.as_str();
                log(
                    LogLevel::Warn,
                    &format!("httpget: {url} failed (status {}): {reason}", r.status),
                );
                None
            }
            Err(error) => {
                log(
                    LogLevel::Warn,
                    &format!("httpget: host call failed for {url}: {error}"),
                );
                None
            }
        }
    }

    /// The failed-fetch view: `down_format` verbatim, or no segment at all
    /// when it's empty — same collapse-to-nothing convention the built-in
    /// widgets' `down_format` follows.
    fn down_segment(down_format: &str) -> Vec<Segment> {
        if down_format.is_empty() {
            Vec::new()
        } else {
            vec![Segment::new(down_format.to_string())]
        }
    }

    export_plugin!(name: "httpget", render: render);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_snippet_takes_first_non_blank_line() {
        assert_eq!(
            extract_snippet("\n  \nhello world\nsecond", 40),
            "hello world"
        );
    }

    #[test]
    fn extract_snippet_truncates_with_ellipsis() {
        assert_eq!(extract_snippet("0123456789", 5), "01234\u{2026}");
    }

    #[test]
    fn extract_snippet_empty_body_is_empty() {
        assert_eq!(extract_snippet("", 40), "");
        assert_eq!(extract_snippet("\n\n", 40), "");
    }

    #[test]
    fn render_format_substitutes_body_and_passes_unknowns() {
        assert_eq!(render_format("=> {body}", "ok"), "=> ok");
        assert_eq!(render_format("{bogus}", "ok"), "{bogus}");
    }
}
