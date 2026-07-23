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
    use extism_pdk::*;
    use rustline_abi::{GuestRender, Segment};
    use serde_json::Value;

    #[host_fn]
    extern "ExtismHost" {
        fn rl_state_read(relpath: String) -> String;
        fn rl_state_write(relpath: String, contents: String) -> String;
        fn rl_log(level: String, msg: String) -> String;
    }

    #[plugin_fn]
    pub fn name() -> FnResult<String> {
        Ok("counter".to_string())
    }

    #[plugin_fn]
    pub fn render(input: String) -> FnResult<Json<Vec<Segment>>> {
        // A malformed input degrades to an empty render (never break the
        // bar) rather than erroring.
        let Ok(input) = serde_json::from_str::<GuestRender>(&input) else {
            return Ok(Json(Vec::new()));
        };
        let format = input.config["format"].as_str().unwrap_or("{count}");

        let count = next_count(read_count());
        write_count(count);

        Ok(Json(vec![Segment::new(render_format(format, count))]))
    }

    /// Read the previously-stored count via `rl_state_read`; missing state,
    /// a host-call error, or malformed JSON all fall back to 0 (same "never
    /// break the bar" degrade as everywhere else in this plugin).
    fn read_count() -> u64 {
        let call = unsafe { rl_state_read(STATE_KEY.to_string()) };
        let Ok(raw) = call else { return 0 };
        let Ok(result) = serde_json::from_str::<Value>(&raw) else {
            return 0;
        };
        if result["ok"].as_bool().unwrap_or(false) && result["exists"].as_bool().unwrap_or(false) {
            parse_count(result["contents"].as_str().unwrap_or_default())
        } else {
            0
        }
    }

    /// Persist `count` via `rl_state_write`. A failed write is logged
    /// through `rl_log` (demonstrating the capability-free logging host fn)
    /// rather than breaking the render — the count just won't have advanced
    /// next time.
    fn write_count(count: u64) {
        let call = unsafe { rl_state_write(STATE_KEY.to_string(), count.to_string()) };
        let ok = call
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .is_some_and(|result: Value| result["ok"].as_bool().unwrap_or(false));
        if !ok {
            let _ = unsafe {
                rl_log(
                    "warn".to_string(),
                    format!("counter: failed to persist count {count}"),
                )
            };
        }
    }
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
