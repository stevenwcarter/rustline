//! Shared pure rendering for the cpu/memory "gauge" bar: a fixed-width
//! horizontal meter drawn with Unicode block-eighths. No I/O; called by the
//! `cpu`/`memory` widgets. Stays private (`mod bar;`) with a `pub(crate)` helper.

/// Partial block-eighth glyphs indexed by remainder `1..=7` (index 0 unused).
const PARTIALS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];

/// Render `fraction` (clamped to `0.0..=1.0`) as a `width`-cell horizontal bar:
/// full cells `█`, one sub-cell partial (`▏`..`▉`) at the boundary, the rest a
/// `░` track. `width == 0` yields an empty string.
pub(crate) fn gauge_bar(fraction: f64, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
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
}
