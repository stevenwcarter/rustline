//! Shared pure rendering for the cpu/memory "sparkline" history: a compact
//! run of Unicode block-eighths glyphs, one per historical reading. No I/O —
//! the history itself lives on `Context.cpu_history`/`mem_history`, populated
//! at the bin's build edge (`crates/rustline/src/cpu.rs`/`memory.rs`, via
//! `sample_store`) only when a widget's `format` references `{spark}`.
//! Mirrors `bar.rs`'s `gauge_bar`: stays private (`mod spark;`) with a
//! `pub(crate)` helper, and like it takes an explicit `width` so the rendered
//! cell count is fixed from the first tick instead of growing as the ring
//! fills.

/// The 8 block-eighths glyphs, low to high, one per eighth of `max`.
const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Upper bound on a rendered sparkline, mirroring `bar.rs`'s `MAX_BAR_WIDTH`:
/// `spark_width` is unbounded user config and this function allocates a
/// `String` from it on every render, so a hostile width degrades to a
/// maximally-wide spark rather than an unbounded allocation.
pub(crate) const MAX_SPARK_WIDTH: usize = 256;

/// Render `samples` (oldest first) as a sparkline of exactly `width`
/// characters: each reading's fraction of `max` (clamped `0.0..=1.0`) maps to
/// one of the 8 block-eighths glyphs, newest at the right edge, **left-padded
/// with spaces** when there are fewer than `width` readings. `max <= 0.0`
/// treats every reading as maxed out (`█`) rather than dividing by zero.
/// `width == 0` yields `""`, mirroring `bar::gauge_bar`'s `width == 0`
/// convention.
///
/// The padding is spaces, not a zero reading, on purpose. The history ring
/// starts empty and fills one entry per `status-interval` tick, so without
/// padding the widget grows a cell per tick and shifts everything beside it —
/// the jitter `width.rs` exists to remove. Padding it with `0.0` would render
/// `▁`, indistinguishable from a real 0% reading, and claim a measurement
/// that was never taken (invariant #6). A space claims nothing and occupies
/// the same cell, so the ring on disk stays honest: only real readings.
///
/// `width` is clamped to [`MAX_SPARK_WIDTH`] first. More than `width` samples
/// keeps the newest `width` — the ring and this function read the same
/// `spark_width` config, but nothing forces them to agree at any instant (the
/// ring is sized at the bin's build edge, from a config the user can change
/// between renders), so this is total either way rather than trusting them to
/// match.
pub(crate) fn sparkline(samples: &[f32], max: f32, width: usize) -> String {
    let width = width.min(MAX_SPARK_WIDTH);
    let shown = samples.len().min(width);
    let mut out = String::with_capacity(width * 3);
    out.extend(std::iter::repeat_n(' ', width - shown));
    out.extend(
        samples[samples.len() - shown..]
            .iter()
            .map(|&v| glyph_for(v, max)),
    );
    out
}

/// Map a single reading to its block-eighths glyph.
fn glyph_for(value: f32, max: f32) -> char {
    let fraction = if max <= 0.0 {
        1.0
    } else {
        (value / max).clamp(0.0, 1.0)
    };
    let index = (fraction * (BLOCKS.len() - 1) as f32).round() as usize;
    BLOCKS[index.min(BLOCKS.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparkline_maps_readings_to_blocks() {
        assert_eq!(sparkline(&[], 100.0, 0), "");
        assert_eq!(sparkline(&[0.0], 100.0, 1), "\u{2581}");
        assert_eq!(sparkline(&[100.0], 100.0, 1), "\u{2588}");
        // a ramp spans low->high blocks; equal readings render equal glyphs
        let s = sparkline(&[0.0, 50.0, 100.0], 100.0, 3);
        assert_eq!(s.chars().count(), 3);
        assert_eq!(s.chars().next().unwrap(), '\u{2581}');
        assert_eq!(s.chars().last().unwrap(), '\u{2588}');
        // clamp above max
        assert_eq!(sparkline(&[200.0], 100.0, 1), "\u{2588}");
    }

    #[test]
    fn equal_readings_render_equal_glyphs() {
        let s = sparkline(&[42.0, 42.0, 42.0], 100.0, 3);
        let chars: Vec<char> = s.chars().collect();
        assert_eq!(chars.len(), 3);
        assert!(chars.windows(2).all(|w| w[0] == w[1]));
    }

    #[test]
    fn negative_reading_clamps_to_lowest_block() {
        assert_eq!(sparkline(&[-5.0], 100.0, 1), "\u{2581}");
    }

    #[test]
    fn zero_or_negative_max_treats_every_reading_as_maxed() {
        assert_eq!(sparkline(&[0.0, 50.0], 0.0, 2), "\u{2588}\u{2588}");
        assert_eq!(sparkline(&[10.0], -1.0, 1), "\u{2588}");
    }

    /// The property this width parameter exists for: the rendered cell count
    /// is `width` from the very first tick, not once the ring has filled.
    /// Without it the widget grew one cell per `status-interval` and shifted
    /// every segment beside it.
    #[test]
    fn sparkline_is_always_width_chars_while_the_ring_fills() {
        for n in 0..=8 {
            let samples = vec![50.0f32; n];
            let out = sparkline(&samples, 100.0, 8);
            assert_eq!(
                out.chars().count(),
                8,
                "{n} sample(s) rendered {out:?}, expected 8 cells"
            );
        }
    }

    /// Padding is spaces and it sits on the LEFT, so the newest reading stays
    /// pinned to the right edge and the history grows leftward into the pad.
    #[test]
    fn partial_history_left_pads_with_spaces_newest_at_right() {
        let out = sparkline(&[100.0, 0.0], 100.0, 5);
        assert_eq!(out, "   \u{2588}\u{2581}");
        assert_eq!(out.chars().filter(|c| *c == ' ').count(), 3);
        // The newest reading is the last cell, not the first.
        assert_eq!(out.chars().last().unwrap(), '\u{2581}');
    }

    /// An empty ring is all padding, never a row of `\u{2581}` that would read as
    /// "the CPU was pinned at 0% for the last 8 ticks" (invariant #6: never a
    /// fabricated reading).
    #[test]
    fn empty_history_renders_only_spaces() {
        assert_eq!(sparkline(&[], 100.0, 6), "      ");
    }

    /// The ring and this function read the same `spark_width`, but the ring is
    /// sized at the bin's build edge and the user can change the config
    /// between renders — so an over-long ring must be total, keeping the
    /// NEWEST `width` readings rather than the oldest.
    #[test]
    fn more_samples_than_width_keeps_the_newest() {
        let out = sparkline(&[0.0, 0.0, 0.0, 100.0, 100.0], 100.0, 2);
        assert_eq!(out, "\u{2588}\u{2588}");
    }

    #[test]
    fn zero_width_renders_empty() {
        assert_eq!(sparkline(&[50.0, 50.0], 100.0, 0), "");
    }

    /// `spark_width` is unbounded user config and this function allocates from
    /// it every render, so a hostile value must clamp rather than allocate.
    /// The sample slice is deliberately shorter than the cap, so the clamp has
    /// to bite on the PADDING path, which is the one this change added.
    #[test]
    fn absurd_width_clamps_to_the_cap() {
        let out = sparkline(&[50.0], 100.0, usize::MAX);
        assert_eq!(out.chars().count(), MAX_SPARK_WIDTH);
        assert_eq!(out.chars().last().unwrap(), '\u{2585}');
    }
}
