//! `cmdrun` — the worked example for the host's exec capability.
//!
//! Runs a configured `program` + `args` through `rl_exec` (or `rl_exec_cached`
//! when `ttl_secs > 0`) and renders a snippet of its stdout. This is the exec
//! counterpart to `httpget`'s plain-HTTP example: the same "configured input,
//! rendered snippet, `down_format` on any failure" shape, exercising the one
//! capability the other four examples don't touch.
//!
//! Every failure path — denied by `allowed_commands`, spawn failure, timeout,
//! or a non-zero exit — is logged via `rl_log` with the reason and falls back
//! to `down_format` (empty by default, i.e. render nothing), the same
//! convention as the built-in widgets.
//!
//! Pure logic lives here and is unit-tested on the host target (`cargo
//! test`); the Extism guest glue below only compiles for wasm32 — see
//! `plugins/httpget` in the rustline repo, whose structure this mirrors.

use serde::Deserialize;

/// Cap on the rendered snippet, in characters. A command that prints a very
/// long first line must not be able to swamp the status line.
pub const MAX_SNIPPET_CHARS: usize = 60;

/// This plugin's `[plugins.cmdrun.options]` table.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Options {
    /// The program to run. No shell is involved — this is argv[0].
    pub program: String,
    /// Its arguments, passed through verbatim.
    pub args: Vec<String>,
    /// Cache TTL in seconds. `0` (the default) uses the plain, uncached host
    /// function — the deliberate contrast this example demonstrates.
    pub ttl_secs: u64,
    /// Render format. `{out}` is the stdout snippet; `{status}` the exit code.
    #[serde(default = "default_format")]
    pub format: String,
    /// Shown when the command couldn't be run. Empty renders nothing.
    pub down_format: String,
    /// Click-toggle alternate view.
    pub alt_format: String,
}

fn default_format() -> String {
    "{out}".to_string()
}

/// Which host function a given TTL selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `rl_exec` — runs every render.
    Plain,
    /// `rl_exec_cached` — the host caches for `ttl_secs`.
    Cached,
}

/// `ttl_secs == 0` means "no caching"; anything else caches.
pub fn select_mode(ttl_secs: u64) -> Mode {
    if ttl_secs == 0 {
        Mode::Plain
    } else {
        Mode::Cached
    }
}

/// The command's first line, trimmed and capped at [`MAX_SNIPPET_CHARS`]
/// characters (not bytes — truncating mid-codepoint would panic).
pub fn extract_snippet(stdout: &str) -> String {
    let first = stdout.lines().next().unwrap_or("").trim();
    first.chars().take(MAX_SNIPPET_CHARS).collect()
}

/// Substitute `{out}` and `{status}`. Unknown placeholders pass through
/// untouched, the same convention as the built-in widgets' formats.
pub fn render_format(format: &str, out: &str, status: i32) -> String {
    format
        .replace("{out}", out)
        .replace("{status}", &status.to_string())
}

#[cfg(target_arch = "wasm32")]
mod guest {
    use super::*;
    use rustline_plugin_sdk::{
        CachedExecResult, ExecResult, GuestRender, HostError, LogLevel, Segment, active_format,
        exec, exec_cached, export_plugin, log,
    };

    /// A host exec response, normalized across the plain (`ExecResult`) and
    /// cached (`CachedExecResult`) shapes so the caller has one match to
    /// write regardless of which host function ran.
    struct Outcome {
        ok: bool,
        status: i32,
        stdout: String,
        error: String,
    }

    impl From<ExecResult> for Outcome {
        fn from(r: ExecResult) -> Self {
            Outcome {
                ok: r.ok,
                status: r.status,
                stdout: r.stdout,
                error: r.error,
            }
        }
    }

    impl From<CachedExecResult> for Outcome {
        fn from(r: CachedExecResult) -> Self {
            Outcome {
                ok: r.ok,
                status: r.status,
                stdout: r.stdout,
                error: r.error,
            }
        }
    }

    fn render(input: &GuestRender) -> Vec<Segment> {
        let opts: Options = serde_json::from_value(input.config.clone()).unwrap_or_default();
        if opts.program.is_empty() {
            // Nothing configured to run.
            return Vec::new();
        }
        let format = active_format(&input.context, "cmdrun", &opts.format, &opts.alt_format);

        match run_command(&opts, &input.context.now) {
            Ok(outcome) if outcome.ok && outcome.status == 0 => {
                let snippet = extract_snippet(&outcome.stdout);
                vec![Segment::new(render_format(
                    format,
                    &snippet,
                    outcome.status,
                ))]
            }
            Ok(outcome) if outcome.ok => {
                log(
                    LogLevel::Warn,
                    &format!(
                        "cmdrun: {} exited with status {}",
                        opts.program, outcome.status
                    ),
                );
                down_segment(&opts.down_format)
            }
            Ok(outcome) => {
                log(
                    LogLevel::Warn,
                    &format!(
                        "cmdrun: {} denied or failed to run: {}",
                        opts.program, outcome.error
                    ),
                );
                down_segment(&opts.down_format)
            }
            Err(err) => {
                log(
                    LogLevel::Warn,
                    &format!("cmdrun: host call failed for {}: {err}", opts.program),
                );
                down_segment(&opts.down_format)
            }
        }
    }

    /// Run the configured command through the host, plain or TTL-cached per
    /// [`select_mode`]. `now` is the current instant (`context.now`), the same
    /// RFC3339 string the cached HTTP path uses.
    fn run_command(opts: &Options, now: &str) -> Result<Outcome, HostError> {
        let args: Vec<&str> = opts.args.iter().map(String::as_str).collect();
        match select_mode(opts.ttl_secs) {
            Mode::Plain => exec(&opts.program, &args).map(Outcome::from),
            Mode::Cached => {
                exec_cached(&opts.program, &args, opts.ttl_secs, now).map(Outcome::from)
            }
        }
    }

    /// The failed-run view: `down_format` verbatim, or no segment at all when
    /// it's empty — same collapse-to-nothing convention the built-in widgets'
    /// `down_format` follows.
    fn down_segment(down_format: &str) -> Vec<Segment> {
        if down_format.is_empty() {
            Vec::new()
        } else {
            vec![Segment::new(down_format.to_string())]
        }
    }

    export_plugin!(name: "cmdrun", render: render);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_takes_the_first_line_and_trims_it() {
        assert_eq!(extract_snippet("hello\nworld\n"), "hello");
        assert_eq!(extract_snippet("  spaced  \n"), "spaced");
        assert_eq!(extract_snippet(""), "");
        assert_eq!(extract_snippet("\n\n"), "");
    }

    #[test]
    fn snippet_is_capped_so_one_long_line_cannot_swamp_the_bar() {
        let long = "x".repeat(500);
        let out = extract_snippet(&long);
        assert!(
            out.chars().count() <= MAX_SNIPPET_CHARS,
            "capped: {}",
            out.chars().count()
        );
    }

    #[test]
    fn snippet_caps_on_characters_not_bytes() {
        // A uniform repeat of any single multi-byte character can't actually
        // exercise the byte-vs-char distinction here: MAX_SNIPPET_CHARS (60)
        // is evenly divisible by every possible UTF-8 character width (1-4),
        // so a byte offset of 60 always lands on a character boundary
        // regardless of which fixed-width character is repeated -- a
        // byte-based `&s[..60]` would never panic, and an assertion of only
        // `out.chars().count() <= MAX_SNIPPET_CHARS` (an upper bound, not an
        // exact one) would pass even if the byte-based version silently
        // under-counted (e.g. 30 two-byte chars, not 60). 59 single-byte
        // characters followed by a 3-byte one straddles byte offset 60
        // exactly: a byte-based slice there cuts the 60th character in half
        // and panics, while `.chars().take(60)` must not.
        let long = format!("{}{}", "a".repeat(59), "€".repeat(500));
        let out = extract_snippet(&long);
        assert_eq!(out.chars().count(), MAX_SNIPPET_CHARS);
    }

    #[test]
    fn render_format_substitutes_out_and_status() {
        assert_eq!(render_format("{out}", "hi", 0), "hi");
        assert_eq!(render_format("[{status}] {out}", "hi", 3), "[3] hi");
        assert_eq!(render_format("no placeholders", "hi", 0), "no placeholders");
    }

    #[test]
    fn render_format_leaves_unknown_placeholders_alone() {
        assert_eq!(render_format("{nope} {out}", "hi", 0), "{nope} hi");
    }

    #[test]
    fn options_parse_with_sensible_defaults() {
        let o: Options = serde_json::from_str("{}").unwrap();
        assert!(o.program.is_empty());
        assert!(o.args.is_empty());
        assert_eq!(o.ttl_secs, 0);
        assert_eq!(o.format, "{out}");
        assert!(o.down_format.is_empty());
    }

    #[test]
    fn options_parse_a_full_table() {
        let o: Options = serde_json::from_str(
            r#"{"program":"git","args":["status","-s"],"ttl_secs":30,"format":"g {out}","down_format":"g ?"}"#,
        )
        .unwrap();
        assert_eq!(o.program, "git");
        assert_eq!(o.args, ["status", "-s"]);
        assert_eq!(o.ttl_secs, 30);
        assert_eq!(o.format, "g {out}");
        assert_eq!(o.down_format, "g ?");
    }

    #[test]
    fn a_zero_ttl_selects_the_plain_uncached_host_fn() {
        assert_eq!(select_mode(0), Mode::Plain);
        assert_eq!(select_mode(1), Mode::Cached);
        assert_eq!(select_mode(3600), Mode::Cached);
    }
}
