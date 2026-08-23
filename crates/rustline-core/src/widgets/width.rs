//! Constant-width numeric formatting for the statusline's numeric widgets.
//!
//! **Read when:** changing how a byte-rate or a percentage renders.
//!
//! tmux re-renders the bar on every `status-interval` tick, and
//! `render_region` joins segments positionally — so a numeral whose width
//! changes shifts every segment beside it sideways, once a second. These
//! formatters pin the numeral to a fixed-width right-aligned field, which is
//! enough to make the whole component width-stable: the unit letter and `%`
//! are one character each, so the numeral is the only variable-width part.
//!
//! Both functions are pure and take no `Context`. Deliberately separate from
//! `memory.rs`'s `format_bytes`, which `memory`/`disk` still use for
//! `{used}`/`{total}`/`{avail}` and which keeps its variable-width output.
//! One difference is easy to miss: `format_rate` keeps the decimal for
//! two-digit values where `format_bytes` drops it — the same 13,000 bytes
//! renders `12.7K` via `format_rate` and `13K` via `format_bytes`, and both
//! can appear in the same bar.

/// Width of the numeral field in [`format_rate`], in characters.
const RATE_DIGITS: usize = 4;

/// The 1024-based unit ladder. Shares its first five entries (`B`..`T`) with
/// `memory::format_bytes`'s ladder — pinned by a cross-check test below —
/// then extends through `P` and `E` so [`format_rate`]'s width stays
/// constant across the entire `u64` domain (see its doc comment).
/// `format_bytes` stays frozen at `T`, so the two ladders intentionally
/// diverge above `1024^5`. Every entry is exactly one character, which is
/// what makes [`format_rate`]'s total width constant.
const UNITS: [&str; 7] = ["B", "K", "M", "G", "T", "P", "E"];

/// A byte-rate as a constant-width numeral plus unit: the numeral
/// right-aligned in [`RATE_DIGITS`] characters, then the one-character unit
/// — always exactly 5 characters. `throughput.rs` appends `/s`, so a
/// rendered rate is always exactly 7.
///
/// The decimal is kept only when it fits, and that is decided by *measuring*
/// the rendered numeral rather than by which decade the value falls in. A
/// decade rule breaks at the rounding boundaries: `99.96` is a two-digit
/// value, but `{:.1}` renders it `"100.0"` — five characters. Raw bytes are
/// integers and never take a decimal.
///
/// The width is a minimum, never a truncation, and the 5-character contract
/// holds unconditionally across the *entire* `u64` domain, not just
/// realistic NIC rates: `u64::MAX / 2^60` is exactly `16.0`, so the ladder's
/// last tier (`E`) can only ever hold a scaled value in `1.0..=16.0`, and
/// `{:.1}` on that range is at most `"16.0"` — four characters. There is no
/// input that clips the numeral or clamps it to a sentinel; the value shown
/// is always the true reading, just expressed in a large enough unit.
///
/// ```text
///        0 -> "   0B"        1_234 -> " 1.2K"
///    1_023 -> "1023B"       13_000 -> "12.7K"
///  102_360 -> " 100K"      126_000 -> " 123K"
/// ```
pub(crate) fn format_rate(bytes_per_sec: u64) -> String {
    let mut value = bytes_per_sec as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let numeral = if unit == 0 {
        // Bytes are integers; a decimal on a raw byte count is noise.
        bytes_per_sec.to_string()
    } else {
        let one_decimal = format!("{value:.1}");
        if one_decimal.len() <= RATE_DIGITS {
            one_decimal
        } else {
            format!("{value:.0}")
        }
    };
    let suffix = UNITS[unit];
    format!("{numeral:>w$}{suffix}", w = RATE_DIGITS)
}

/// Width of the numeral field in [`pad_percent`], in characters.
const PERCENT_DIGITS: usize = 3;

/// An integer percentage right-aligned in [`PERCENT_DIGITS`] characters:
/// `"  7"`, `" 42"`, `"100"`. Keeps `cpu`/`memory`/`battery`/`disk` from
/// shifting two characters wide as a reading crosses 10% and 100%.
///
/// The width is a minimum, never a truncation: `memory` and `disk` derive
/// the percentage from an unclamped `used / total`, so a pathological
/// over-100 reading must render itself rather than be cut to fit.
pub(crate) fn pad_percent(percent: u64) -> String {
    format!("{percent:>w$}", w = PERCENT_DIGITS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every unit and every precision case, including the `99.96 K`
    /// rounding boundary that a decade-based rule renders as `"100.0"`.
    #[test]
    fn format_rate_value_table() {
        let k = 1024u64;
        assert_eq!(format_rate(0), "   0B");
        assert_eq!(format_rate(42), "  42B");
        assert_eq!(format_rate(1023), "1023B");
        assert_eq!(format_rate(k), " 1.0K");
        assert_eq!(format_rate(1234), " 1.2K");
        assert_eq!(format_rate(13_000), "12.7K");
        assert_eq!(format_rate(102_360), " 100K"); // 99.96 K -> not "100.0"
        assert_eq!(format_rate(126_000), " 123K");
        assert_eq!(format_rate(k.pow(2)), " 1.0M");
        assert_eq!(format_rate(512 * k.pow(2)), " 512M");
        assert_eq!(format_rate(k.pow(3)), " 1.0G");
        assert_eq!(format_rate(k.pow(4)), " 1.0T");
    }

    /// The property the whole feature exists for. Sweeps every magnitude a
    /// counter diff can reach plus the rounding boundaries where a
    /// decade-based rule would overflow the field, across the full `u64`
    /// range (not just realistic NIC rates — see
    /// `format_rate_is_five_chars_at_the_u64_ceiling`).
    #[test]
    fn format_rate_is_always_five_chars() {
        let mut inputs: Vec<u64> = (0..2048).collect();
        for shift in 0..64 {
            let base = 1u64 << shift;
            for delta in [0u64, 1, 7, 999] {
                inputs.push(base.saturating_add(delta));
                inputs.push(base.saturating_sub(delta));
            }
        }
        for unit_shift in [10u32, 20, 30, 40, 50, 60] {
            let unit = (1u64 << unit_shift) as f64;
            for scaled in [9.95_f64, 99.95, 99.96, 999.5, 1023.6] {
                inputs.push((scaled * unit) as u64);
            }
        }
        for n in inputs {
            let out = format_rate(n);
            assert_eq!(out.chars().count(), 5, "format_rate({n}) = {out:?}");
        }
    }

    /// The 5-character contract holds across the whole `u64` domain, not just
    /// realistic NIC rates: `u64::MAX / 2^60` is exactly 16.0, so the `E` tier
    /// can only ever hold `1.0..=16.0`. Pinned because the honest way to keep
    /// the field constant is a taller ladder — never clipping the numeral or
    /// clamping it to a sentinel, either of which would show a number that is
    /// not the reading.
    #[test]
    fn format_rate_is_five_chars_at_the_u64_ceiling() {
        assert_eq!(format_rate(u64::MAX), "16.0E");
        assert_eq!(format_rate(1024u64.pow(5)), " 1.0P");
        assert_eq!(format_rate(1024u64.pow(6)), " 1.0E");
        for n in [u64::MAX, u64::MAX - 1, 1024u64.pow(5), 1024u64.pow(6)] {
            assert_eq!(format_rate(n).chars().count(), 5, "format_rate({n})");
        }
    }

    /// `format_rate` and `format_bytes` must agree on which unit letter a
    /// given magnitude selects, or `throughput` and `memory`/`disk` could
    /// show conflicting units in the same bar (e.g. `1.5P` next to `1536T`).
    /// Swept only below `1024^5`, where both ladders are defined identically
    /// (see the `UNITS` doc comment) — above it the two intentionally
    /// diverge: `format_bytes` stays frozen at `T` while `format_rate`
    /// (Fix 1) continues on to `P`/`E` to keep its own field constant across
    /// all of `u64`.
    #[test]
    fn format_rate_and_format_bytes_agree_on_unit_below_the_divergence_point() {
        use crate::widgets::memory::format_bytes;

        let divergence_point = 1024u64.pow(5);
        let mut inputs: Vec<u64> = (0..2048).collect();
        for shift in 0..50 {
            let base = 1u64 << shift;
            for delta in [0u64, 1, 7, 999] {
                inputs.push(base.saturating_add(delta));
                inputs.push(base.saturating_sub(delta));
            }
        }
        for n in inputs {
            if n >= divergence_point {
                continue;
            }
            let rate_unit = format_rate(n).chars().next_back();
            let bytes_unit = format_bytes(n).chars().next_back();
            assert_eq!(rate_unit, bytes_unit, "n={n}");
        }
    }

    #[test]
    fn pad_percent_right_aligns_to_three() {
        assert_eq!(pad_percent(0), "  0");
        assert_eq!(pad_percent(7), "  7");
        assert_eq!(pad_percent(42), " 42");
        assert_eq!(pad_percent(100), "100");
    }

    /// The width is a floor, not a field to clip into: `memory`/`disk`
    /// compute their percentage from an unclamped `used / total`.
    #[test]
    fn pad_percent_never_truncates() {
        assert_eq!(pad_percent(105), "105");
        assert_eq!(pad_percent(1000), "1000");
    }
}
