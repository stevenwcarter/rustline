//! Shared pure rendering for the cpu/memory "sparkline" history: a compact
//! run of Unicode block-eighths glyphs, one per historical reading. No I/O —
//! the history itself lives on `Context.cpu_history`/`mem_history`, populated
//! at the bin's build edge (`crates/rustline/src/cpu.rs`/`memory.rs`, via
//! `sample_store`) only when a widget's `format` references `{spark}`.
//! Mirrors `bar.rs`'s `gauge_bar`: stays private (`mod spark;`) with a
//! `pub(crate)` helper.

/// The 8 block-eighths glyphs, low to high, one per eighth of `max`.
const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Render `samples` (oldest first) as a sparkline: each reading's fraction of
/// `max` (clamped `0.0..=1.0`) maps to one of the 8 block-eighths glyphs.
/// `max <= 0.0` treats every reading as maxed out (`█`) rather than dividing
/// by zero. An empty slice yields `""`.
pub(crate) fn sparkline(samples: &[f32], max: f32) -> String {
    samples.iter().map(|&v| glyph_for(v, max)).collect()
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
        assert_eq!(sparkline(&[], 100.0), "");
        assert_eq!(sparkline(&[0.0], 100.0), "▁");
        assert_eq!(sparkline(&[100.0], 100.0), "█");
        // a ramp spans low→high blocks; equal readings render equal glyphs
        let s = sparkline(&[0.0, 50.0, 100.0], 100.0);
        assert_eq!(s.chars().count(), 3);
        assert_eq!(s.chars().next().unwrap(), '▁');
        assert_eq!(s.chars().last().unwrap(), '█');
        // clamp above max
        assert_eq!(sparkline(&[200.0], 100.0), "█");
    }

    #[test]
    fn equal_readings_render_equal_glyphs() {
        let s = sparkline(&[42.0, 42.0, 42.0], 100.0);
        let chars: Vec<char> = s.chars().collect();
        assert_eq!(chars.len(), 3);
        assert!(chars.windows(2).all(|w| w[0] == w[1]));
    }

    #[test]
    fn negative_reading_clamps_to_lowest_block() {
        assert_eq!(sparkline(&[-5.0], 100.0), "▁");
    }

    #[test]
    fn zero_or_negative_max_treats_every_reading_as_maxed() {
        assert_eq!(sparkline(&[0.0, 50.0], 0.0), "██");
        assert_eq!(sparkline(&[10.0], -1.0), "█");
    }
}
