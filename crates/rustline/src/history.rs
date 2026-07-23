//! Pure helpers for a persisted sparkline-history ring: a single-line,
//! space-separated list of `f32` readings (oldest first). The read/write of
//! that line goes through `sample_store::{read_sample,write_sample}` — this
//! module owns only the ring's own shape (parse/serialize/push+truncate),
//! the same split `cpu.rs`'s `parse_snapshot`/`serialize_snapshot` and
//! `throughput.rs`'s `parse_sample`/`serialize_sample` each keep between
//! their own value's shape and `sample_store`'s generic file I/O. Shared by
//! `cpu.rs`'s and `memory.rs`'s `{spark}`-gated history read (W45) so neither
//! duplicates it.

/// Parse a persisted history ring from its serialized line. A non-numeric
/// token is skipped rather than failing the whole read — total over corrupt
/// input, never a panic (a broken cache must never break the bar).
pub fn parse_history(text: &str) -> Vec<f32> {
    text.lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(|tok| tok.parse::<f32>().ok())
        .collect()
}

/// Serialize a history ring to a single space-separated line (trailing
/// newline), the same plain-text convention as `cpu.rs`'s `serialize_snapshot`.
pub fn serialize_history(history: &[f32]) -> String {
    let line = history
        .iter()
        .map(f32::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    format!("{line}\n")
}

/// Push `value` onto `history`, keeping only the most recent `max_len`
/// readings (oldest dropped first). `max_len == 0` drains the ring entirely,
/// mirroring `bar::gauge_bar`'s `width == 0` -> empty convention.
pub fn push_truncate(history: &mut Vec<f32>, value: f32, max_len: usize) {
    history.push(value);
    if history.len() > max_len {
        let excess = history.len() - max_len;
        history.drain(0..excess);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_serialize_round_trips() {
        let h = vec![1.0, 2.5, 100.0];
        let text = serialize_history(&h);
        assert_eq!(parse_history(&text), h);
    }

    #[test]
    fn parse_is_total_over_corrupt_input() {
        for bad in ["", "   ", "\n", "x y z"] {
            assert!(parse_history(bad).is_empty(), "expected empty for {bad:?}");
        }
        // A mix of valid/invalid tokens keeps just the valid ones.
        assert_eq!(parse_history("1.0 garbage 2.0\n"), vec![1.0, 2.0]);
    }

    #[test]
    fn push_truncate_keeps_last_n() {
        let mut h = vec![1.0, 2.0, 3.0];
        push_truncate(&mut h, 4.0, 3);
        assert_eq!(h, vec![2.0, 3.0, 4.0]);
    }

    #[test]
    fn push_truncate_zero_max_len_drains_all() {
        let mut h = vec![1.0, 2.0];
        push_truncate(&mut h, 3.0, 0);
        assert!(h.is_empty());
    }

    #[test]
    fn push_truncate_under_capacity_keeps_all() {
        let mut h = vec![1.0];
        push_truncate(&mut h, 2.0, 5);
        assert_eq!(h, vec![1.0, 2.0]);
    }

    #[test]
    fn push_truncate_empty_history_seeds_first_reading() {
        let mut h: Vec<f32> = Vec::new();
        push_truncate(&mut h, 42.0, 8);
        assert_eq!(h, vec![42.0]);
    }
}
