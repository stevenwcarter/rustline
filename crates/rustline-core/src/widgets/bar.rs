//! Shared pure rendering for the cpu/memory "gauge" bar: a fixed-width
//! horizontal meter drawn with Unicode block-eighths. No I/O; called by the
//! `cpu`/`memory` widgets. Stays private (`mod bar;`) with a `pub(crate)` helper.

/// Partial block-eighth glyphs indexed by remainder `1..=7` (index 0 unused).
const PARTIALS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];

/// Upper bound on a rendered gauge's cell count.
///
/// `bar_width` is a `usize` deserialized straight from user TOML with no
/// ceiling, and TOML integers reach `i64::MAX`. Without this clamp,
/// `[widgets.cpu] bar_width = 1000000000` allocates ~3 GB on every render and
/// an even larger value wraps the `width * 8` / `width * 3` products in
/// release. `render_named_region`'s `catch_unwind` guard cannot rescue either
/// — it catches a panic, not an allocation abort and not an unbounded loop —
/// so the bar would hang rather than degrade, breaking invariant #3 ("a bad
/// config must never break the bar").
///
/// Clamped here rather than at deserialization so the bound holds for every
/// caller regardless of how the value arrived. No real status line is anywhere
/// near this wide, so a legitimate configuration never notices.
pub(crate) const MAX_BAR_WIDTH: usize = 256;

/// Render `fraction` (clamped to `0.0..=1.0`) as a `width`-cell horizontal bar:
/// full cells `█`, one sub-cell partial (`▏`..`▉`) at the boundary, the rest a
/// `░` track. `width == 0` yields an empty string. `width` is clamped to
/// `MAX_BAR_WIDTH` first, so a hostile config degrades to a maximally-wide bar
/// instead of allocating gigabytes or overflowing.
pub(crate) fn gauge_bar(fraction: f64, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let width = width.min(MAX_BAR_WIDTH);
    let eighths = (fraction.clamp(0.0, 1.0) * (width * 8) as f64).round() as usize;
    let full = eighths / 8;
    let rem = eighths % 8;
    let mut out = String::with_capacity(width * 3);
    for _ in 0..full {
        out.push('█');
    }
    if rem > 0 {
        out.push_str(PARTIALS[rem]);
    }
    let track = width - full - usize::from(rem > 0);
    for _ in 0..track {
        out.push('░');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_full() {
        assert_eq!(gauge_bar(0.0, 8), "░░░░░░░░");
        assert_eq!(gauge_bar(1.0, 8), "████████");
    }

    #[test]
    fn half_at_various_widths() {
        assert_eq!(gauge_bar(0.5, 8), "████░░░░");
        assert_eq!(gauge_bar(0.5, 4), "██░░");
    }

    #[test]
    fn sub_cell_partial() {
        // 0.3125 * 64 = 20 eighths -> 2 full + 4/8 partial (▌) + 5 track
        assert_eq!(gauge_bar(0.3125, 8), "██▌░░░░░");
    }

    #[test]
    fn clamps_and_zero_width() {
        assert_eq!(gauge_bar(1.5, 4), "████");
        assert_eq!(gauge_bar(-0.2, 4), "░░░░");
        assert_eq!(gauge_bar(0.5, 0), "");
    }

    #[test]
    fn always_width_cells() {
        for f in [0.0, 0.1, 0.37, 0.5, 0.99, 1.0] {
            assert_eq!(gauge_bar(f, 8).chars().count(), 8, "f={f}");
        }
    }

    #[test]
    fn absurd_width_is_clamped_not_allocated() {
        // A fat-fingered config must degrade, not hang or abort. catch_unwind
        // cannot rescue an allocation abort or an unbounded loop (invariant #3).
        let out = gauge_bar(0.5, 1_000_000_000);
        assert_eq!(out.chars().count(), MAX_BAR_WIDTH);
    }

    #[test]
    fn usize_max_width_does_not_overflow() {
        // width * 8 and width * 3 would both wrap in release without the clamp.
        let out = gauge_bar(1.0, usize::MAX);
        assert_eq!(out.chars().count(), MAX_BAR_WIDTH);
    }

    #[test]
    fn default_widths_are_byte_identical_to_before_the_clamp() {
        assert_eq!(gauge_bar(0.0, 8), "░░░░░░░░");
        assert_eq!(gauge_bar(1.0, 8), "████████");
        assert_eq!(gauge_bar(0.5, 8), "████░░░░");
        assert_eq!(gauge_bar(0.5, 0), "");
    }
}
