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

/// Width of the numeral field in [`format_rate`], in characters.
const RATE_DIGITS: usize = 4;

/// The 1024-based unit ladder, matching `memory::format_bytes`. Every entry
/// is exactly one character, which is what makes [`format_rate`]'s total
/// width constant.
const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];

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
/// The width is a minimum, never a truncation. The ladder caps at `T` with a
/// scaled value below 1024, so `{:.0}` yields at most `"1024"` and the field
/// does not overflow for any rate a NIC counter diff can produce.
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
    /// decade-based rule would overflow the field.
    #[test]
    fn format_rate_is_always_five_chars() {
        let mut inputs: Vec<u64> = (0..2048).collect();
        for shift in 0..41 {
            let base = 1u64 << shift;
            for delta in [0u64, 1, 7, 999] {
                inputs.push(base.saturating_add(delta));
                inputs.push(base.saturating_sub(delta));
            }
        }
        for unit_shift in [10u32, 20, 30, 40] {
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
}
