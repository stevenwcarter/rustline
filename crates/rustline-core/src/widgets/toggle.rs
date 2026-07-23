//! Shared click-toggle helpers: which format string is active given the
//! toggle set, and whether a widget is a clickable range.

use crate::{Context, RANGE_NAME_MAX_BYTES};

/// The active format string for a widget: its `alt` view when the widget has a
/// non-empty `alt` AND its `name` is in `ctx.toggled`, else its normal `format`.
pub(crate) fn active_format<'a>(
    ctx: &Context,
    name: &str,
    format: &'a str,
    alt: &'a str,
) -> &'a str {
    if !alt.is_empty() && ctx.toggled.contains(name) {
        alt
    } else {
        format
    }
}

/// A widget's clickable range name: `Some(name)` when it has a non-empty `alt`
/// view AND `name` fits tmux's [`RANGE_NAME_MAX_BYTES`]-byte `range=user|X`
/// limit; else `None` (the widget is not clickable and emits no range markup).
pub(crate) fn clickable_range<'a>(name: &'a str, alt: &str) -> Option<&'a str> {
    if !alt.is_empty() && name.len() <= RANGE_NAME_MAX_BYTES {
        Some(name)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};
    use std::collections::BTreeSet;

    fn ctx(toggled: &[&str]) -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/".into(),
            home: "/h".into(),
            hostname: "h".into(),
            loadavg: None,
            now: Local
                .with_ymd_and_hms(2026, 7, 21, 12, 0, 0)
                .single()
                .unwrap(),
            window: None,
            interfaces: Vec::new(),
            battery: None,
            cpu: None,
            memory: None,
            git: None,
            os: String::new(),
            arch: String::new(),
            toggled: toggled
                .iter()
                .map(|s| s.to_string())
                .collect::<BTreeSet<_>>(),
            colors: Default::default(),
        }
    }

    #[test]
    fn active_format_picks_alt_only_when_toggled_and_alt_nonempty() {
        assert_eq!(active_format(&ctx(&["cpu"]), "cpu", "F", "A"), "A");
        assert_eq!(active_format(&ctx(&[]), "cpu", "F", "A"), "F"); // not toggled
        assert_eq!(active_format(&ctx(&["cpu"]), "cpu", "F", ""), "F"); // empty alt
        assert_eq!(active_format(&ctx(&["mem"]), "cpu", "F", "A"), "F"); // other toggled
    }

    #[test]
    fn clickable_range_requires_alt_and_fits_15_bytes() {
        assert_eq!(clickable_range("cpu", "A"), Some("cpu"));
        assert_eq!(clickable_range("cpu", ""), None); // no alt -> not clickable
        assert_eq!(clickable_range("this_name_is_16b", "A"), None); // 16 bytes > 15
        assert_eq!(
            clickable_range("fifteen_bytes__", "A"),
            Some("fifteen_bytes__")
        ); // exactly 15
    }
}
