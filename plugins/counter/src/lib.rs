//! rustline `counter` plugin: a state-backed counter demonstrating the host's
//! `rl_state_read`/`rl_state_write` capability (rather than network, as
//! `weather` demonstrates). Each render reads the previously-stored count
//! from this plugin's own sandboxed state directory, increments it, writes
//! the new value back, and renders it — state that survives across tmux
//! status-line refreshes, no network capability needed at all.
//!
//! Pure logic lives here and is unit-tested on the host target (`cargo
//! test`); the Extism guest glue below only compiles for wasm32. See
//! `plugins/weather` in the rustline repo for the cached-HTTP worked
//! example.

/// The state-file key this plugin reads/writes, relative to its own
/// sandboxed state directory (see `rl_state_read`/`rl_state_write`).
pub const STATE_KEY: &str = "count";

/// Parse a previously-stored count. Missing state, an empty/whitespace body,
/// or anything that doesn't parse as `u64` all count as "no previous count"
/// (0) rather than erroring — a plugin must never break the bar over a
/// corrupted state file.
pub fn parse_count(contents: &str) -> u64 {
    contents.trim().parse().unwrap_or(0)
}

/// Increment `prev`, saturating at `u64::MAX` instead of wrapping.
pub fn next_count(prev: u64) -> u64 {
    prev.saturating_add(1)
}

/// Substitute the `{count}` placeholder in `format`. Unknown placeholders
/// pass through untouched.
pub fn render_format(format: &str, count: u64) -> String {
    format.replace("{count}", &count.to_string())
}

#[cfg(target_arch = "wasm32")]
mod guest {
    use super::*;
    use rustline_plugin_sdk::{
        GuestRender, LogLevel, Segment, export_plugin, log, state_read, state_write,
    };

    fn render(input: &GuestRender) -> Vec<Segment> {
        let format = input.config["format"].as_str().unwrap_or("{count}");
        let count = next_count(read_count());
        write_count(count);
        vec![Segment::new(render_format(format, count))]
    }

    /// Read the previously-stored count via `state_read`; missing state, a
    /// host-call error, or a not-ok result all fall back to 0 (same "never
    /// break the bar" degrade as everywhere else in this plugin).
    fn read_count() -> u64 {
        match state_read(STATE_KEY) {
            Ok(r) if r.ok && r.exists => parse_count(&r.contents),
            _ => 0,
        }
    }

    /// Persist `count` via `state_write`. A failed write is logged through the
    /// SDK's `log` (the capability-free logging host fn) rather than breaking
    /// the render — the count just won't have advanced next time.
    fn write_count(count: u64) {
        let ok = state_write(STATE_KEY, &count.to_string()).is_ok_and(|r| r.ok);
        if !ok {
            log(
                LogLevel::Warn,
                &format!("counter: failed to persist count {count}"),
            );
        }
    }

    export_plugin!(name: "counter", render: render);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_count_defaults_missing_or_invalid_to_zero() {
        assert_eq!(parse_count(""), 0);
        assert_eq!(parse_count("   "), 0);
        assert_eq!(parse_count("not a number"), 0);
        assert_eq!(parse_count("42"), 42);
        assert_eq!(parse_count(" 7 \n"), 7);
    }

    #[test]
    fn next_count_increments_and_saturates() {
        assert_eq!(next_count(0), 1);
        assert_eq!(next_count(41), 42);
        assert_eq!(next_count(u64::MAX), u64::MAX);
    }

    #[test]
    fn render_format_substitutes_count_and_passes_unknowns_through() {
        assert_eq!(render_format("seen {count} times", 5), "seen 5 times");
        assert_eq!(render_format("{bogus}", 5), "{bogus}");
    }
}
