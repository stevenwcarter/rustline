//! The global click-toggle state file: which widgets are currently showing
//! their `alt_format`. Read once at Context-build time, written by `rustline
//! click`. Newline-delimited widget names under `$XDG_DATA_HOME/rustline/toggles`.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// Path to the toggles state file (reuses the wasm crate's XDG data-root resolver
/// so there is one base dir: `$XDG_DATA_HOME/rustline`, fallback
/// `~/.local/share/rustline`).
#[allow(
    dead_code,
    reason = "wired up by tasks 8-9; kept here to land the module standalone"
)]
pub fn toggles_path() -> PathBuf {
    rustline_wasm::data_root().join("toggles")
}

/// Parse newline-delimited names into a set. Total: trims each line, drops
/// blanks; any malformed/partial content simply yields fewer names.
pub fn parse_toggles(contents: &str) -> BTreeSet<String> {
    contents
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Serialize a set to sorted, newline-delimited text (trailing newline).
pub fn serialize_toggles(set: &BTreeSet<String>) -> String {
    let mut s = String::new();
    for name in set {
        s.push_str(name);
        s.push('\n');
    }
    s
}

/// Flip `name`'s membership.
#[allow(
    dead_code,
    reason = "wired up by tasks 8-9; kept here to land the module standalone"
)]
pub fn apply_toggle(set: &mut BTreeSet<String>, name: &str) {
    if !set.remove(name) {
        set.insert(name.to_string());
    }
}

/// Read the toggle set; a missing/unreadable file yields an empty set.
#[allow(
    dead_code,
    reason = "wired up by tasks 8-9; kept here to land the module standalone"
)]
pub fn read_toggles() -> BTreeSet<String> {
    match std::fs::read_to_string(toggles_path()) {
        Ok(text) => parse_toggles(&text),
        Err(_) => BTreeSet::new(),
    }
}

/// Best-effort atomic write (temp file + rename); logs a warning on failure and
/// never panics — a broken toggle must never break the bar.
#[allow(
    dead_code,
    reason = "wired up by tasks 8-9; kept here to land the module standalone"
)]
pub fn write_toggles(set: &BTreeSet<String>) {
    let path = toggles_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("tmp");
    if let Err(error) = std::fs::write(&tmp, serialize_toggles(set)) {
        tracing::warn!(%error, "failed to write toggles temp file");
        return;
    }
    if let Err(error) = std::fs::rename(&tmp, &path) {
        tracing::warn!(%error, "failed to rename toggles file");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_is_total_over_blanks_and_whitespace() {
        let set = parse_toggles("cpu\n\n  memory  \n\n");
        assert_eq!(
            set,
            BTreeSet::from(["cpu".to_string(), "memory".to_string()])
        );
        assert!(parse_toggles("").is_empty());
    }

    #[test]
    fn parse_serialize_round_trips() {
        let set = BTreeSet::from(["battery".to_string(), "cpu".to_string()]);
        assert_eq!(parse_toggles(&serialize_toggles(&set)), set);
    }

    #[test]
    fn apply_toggle_flips_membership() {
        let mut set = BTreeSet::new();
        apply_toggle(&mut set, "cpu");
        assert!(set.contains("cpu"));
        apply_toggle(&mut set, "cpu");
        assert!(!set.contains("cpu"));
    }
}
