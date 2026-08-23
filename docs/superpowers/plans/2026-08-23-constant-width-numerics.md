# Constant-Width Numeric Fields Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the tmux statusline jittering by pinning the throughput widget's byte-rates and the `cpu`/`memory`/`battery`/`disk` `{percent}` values to fixed-width right-aligned numeral fields.

**Architecture:** One new private module, `crates/rustline-core/src/widgets/width.rs`, holding two pure formatters — `format_rate` (4-wide numeral + 1-char unit = always 5 chars) and `pad_percent` (3-wide) — following the existing `alert.rs`/`bar.rs`/`spark.rs` pattern of a private `mod` with `pub(crate) fn`s re-exported from `widgets/mod.rs`. Call sites swap a `.to_string()` / `format_bytes(..)` for the new formatter; nothing else in the render pipeline changes. `memory.rs`'s `format_bytes` is left exactly as it is, because `memory`/`disk`'s `{used}`/`{total}`/`{avail}` must keep their current output.

**Tech Stack:** Rust edition 2024, `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all`. No new dependencies.

**Spec:** [`docs/superpowers/specs/2026-08-23-constant-width-numerics-design.md`](../specs/2026-08-23-constant-width-numerics-design.md)

## Global Constraints

- **Edition 2024** in every crate; `rustfmt.toml` is `edition = "2024"`. Run `cargo fmt --all` before every commit — there is **no pre-commit hook**.
- The tree must stay **clippy-clean**: `cargo clippy --all-targets -- -D warnings` must pass with zero output. This matters for task ordering: a `pub(crate) fn` with no caller is a `dead_code` warning and therefore a build failure, so **every function this plan adds is wired to its call site in the same task that creates it.**
- The tree must stay **rustfmt-clean**: `cargo fmt --all --check` must pass.
- **No new dependency**, so no `Cargo.lock` churn and no `just check-lock` involvement.
- **No config key, no ABI change, no wire-type change.** `rustline-abi` is not touched. WASM plugins emit their own `Segment`s and are unaffected.
- **`down_format` paths are never padded.** All five widgets collapse `{percent}`/`{down}`/`{up}` to `""` when the reading is `None`, and that behavior must survive unchanged — padding a reading the widget is declining to invent would fabricate width (invariant #6).
- **`memory::format_bytes` must not be modified.** Its existing test `format_bytes_humanizes` in `memory.rs` must still pass untouched.
- Widths are a **minimum, never a truncation.** No value is ever clipped to fit its field.

---

### Task 1: `format_rate` and the throughput widget

**Files:**
- Create: `crates/rustline-core/src/widgets/width.rs`
- Modify: `crates/rustline-core/src/widgets/mod.rs` (add `mod width;`, add the re-export, fix one test assertion at line ~826)
- Modify: `crates/rustline-core/src/widgets/throughput.rs` (swap the formatter, fix four test assertions, add the seam test)
- Modify: `crates/rustline-core/src/render.rs` (test module only — two characterization tests pinning that the markup path preserves padding)
- Test: the `#[cfg(test)] mod tests` blocks inside `width.rs` and `throughput.rs` (this codebase puts unit tests inline, not in a `tests/` dir)

**Interfaces:**
- Consumes: `crate::widgets::throughput::ThroughputWidget` (existing), `rustline_abi::Throughput` with fields `down_bytes_per_sec: u64` / `up_bytes_per_sec: u64` (existing).
- Produces: `pub(crate) fn format_rate(bytes_per_sec: u64) -> String`, re-exported as `crate::widgets::format_rate`. Returns exactly 5 characters: a 4-wide right-aligned numeral followed by a one-character unit from `["B", "K", "M", "G", "T"]`. Task 2 adds a second function to the same module and extends the same re-export line.

- [ ] **Step 1: Write the failing tests**

Create `crates/rustline-core/src/widgets/width.rs` with the module doc, the constants, and the tests — but **no function bodies yet**; write `format_rate` as a stub that returns `String::new()` so the file compiles and the tests fail on the assertion rather than on a missing symbol.

```rust
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
    let _ = bytes_per_sec;
    String::new()
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
```

Register the module in `crates/rustline-core/src/widgets/mod.rs`. Add `mod width;` to the alphabetised `mod` list — it goes after `mod toggle;` and before `pub mod uptime;`:

```rust
mod toggle;
pub mod uptime;
mod width;
pub mod windows;
```

(`mod width;` sorts after `uptime` alphabetically; put it exactly as shown so `cargo fmt` and the existing ordering agree.)

And add the re-export next to the existing `pub(crate) use alert::{...}` line:

```rust
// Re-exported for the numeric widgets: constant-width numerals so the bar
// doesn't shift sideways on every status-interval tick.
pub(crate) use width::format_rate;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rustline-core widgets::width -- --nocapture`

Expected: both tests FAIL. `format_rate_value_table` fails on the first assertion with `assertion \`left == right\` failed: left: "", right: "   0B"`; `format_rate_is_always_five_chars` fails with `format_rate(0) = ""`.

- [ ] **Step 3: Write the implementation**

Replace the `format_rate` stub body in `crates/rustline-core/src/widgets/width.rs`:

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rustline-core widgets::width`

Expected: PASS, 2 tests.

- [ ] **Step 5: Wire it into the throughput widget**

In `crates/rustline-core/src/widgets/throughput.rs`, change the import on line 1 and the two substitutions in `render`.

Replace:

```rust
use crate::widgets::memory::format_bytes;
use crate::{Context, RangeName, Segment, Widget, WidgetName};
```

with:

```rust
use crate::widgets::format_rate;
use crate::{Context, RangeName, Segment, Widget, WidgetName};
```

Replace the two `.replace(...)` calls in `render`:

```rust
                let text = fmt
                    .replace(
                        "{down}",
                        &format!("{}/s", format_rate(t.down_bytes_per_sec)),
                    )
                    .replace("{up}", &format!("{}/s", format_rate(t.up_bytes_per_sec)));
```

Also update the struct's doc comment, which currently claims `format_bytes` and `1.2M/s`. Replace the sentence

```rust
/// Pure — reads only that map. `{down}`/`{up}` are human-readable binary
/// sizes (via `memory.rs`'s `format_bytes`) suffixed `/s`, e.g. `1.2M/s`.
```

with

```rust
/// Pure — reads only that map. `{down}`/`{up}` are constant-width binary
/// rates (via `width.rs`'s `format_rate`) suffixed `/s` — always exactly
/// seven characters, e.g. ` 1.2M/s`, so the segment never shifts width
/// between ticks.
```

- [ ] **Step 6: Update the throughput tests to the padded expectations**

In `crates/rustline-core/src/widgets/throughput.rs`'s test module, four assertions change. Each rate is now 7 characters (`   0B/s`, ` 1.0K/s`, ` 512M/s`).

```rust
    #[test]
    fn renders_down_and_up_as_human_readable_rates() {
        let g = 1024u64.pow(3);
        let out = w("{down} {up}", "").render(&ctx(rate(g, 512 * 1024 * 1024)));
        assert_eq!(out[0].text, " 1.0G/s  512M/s");
    }

    #[test]
    fn renders_small_rates_in_bytes() {
        let out = w("down={down}", "").render(&ctx(rate(0, 42)));
        assert_eq!(out[0].text, "down=   0B/s");
    }
```

In `throughput_toggled_uses_alt_format`:

```rust
        assert_eq!(out[0].text, " 1.0K/s/ 2.0K/s");
```

In `throughput_widget_reads_its_interface_from_throughputs_map`:

```rust
        assert_eq!(out[0].text, " 1.0K/s  2.0K/s");
```

- [ ] **Step 7: Add the rendered-width seam test**

Append this test to the same test module in `crates/rustline-core/src/widgets/throughput.rs`:

```rust
    /// The seam the feature actually crosses is the rendered segment text,
    /// not the formatter in isolation. This pins that nothing between
    /// `format_rate` and `Segment::text` trims or collapses the padding — a
    /// future render-pipeline change that started trimming would otherwise
    /// kill constant width silently, with every other test still green.
    #[test]
    fn rendered_rate_width_is_constant_across_every_magnitude() {
        let k = 1024u64;
        let rates = [
            (0, 0),
            (1, 42),
            (1023, 999),
            (k, 2 * k),
            (13_000, 102_360),
            (126_000, 512 * k),
            (k.pow(2), k.pow(2) * 3 / 2),
            (512 * k.pow(2), k.pow(3)),
            (k.pow(3), 42 * k.pow(3)),
            (k.pow(4), 1023 * k.pow(3)),
        ];
        let widths: Vec<usize> = rates
            .iter()
            .map(|&(down, up)| {
                let out = w(" {down} {up}", "").render(&ctx(rate(down, up)));
                out[0].text.chars().count()
            })
            .collect();
        assert!(
            widths.windows(2).all(|p| p[0] == p[1]),
            "throughput render width jitters across magnitudes: {widths:?}"
        );
        // " " + 7-char rate + " " + 7-char rate.
        assert_eq!(widths[0], 16);
    }
```

- [ ] **Step 8: Pin the padding through the markup path**

`Widget::render` is only half the seam. The padding also has to survive
`sanitize_text` and `render_region` on its way into tmux markup — the spec
names "`sanitize_text` does not alter spaces" as a load-bearing invariant, and
right now nothing asserts it. Add both tests to
`crates/rustline-core/src/render.rs`'s test module (it already has `seg`,
`theme`, and `sanitize_text` in scope).

```rust
    /// `sanitize_text` guards `#` and control characters (invariant #8). It
    /// must leave ASCII space alone, because the numeric widgets encode
    /// constant width as leading spaces — see `widgets/width.rs`.
    #[test]
    fn sanitize_preserves_padding_spaces() {
        for s in ["   0B/s", " 1.2K/s", "12.7K/s", "  7%", "100%"] {
            assert_eq!(sanitize_text(s), s, "padding altered in {s:?}");
            assert!(
                matches!(sanitize_text(s), std::borrow::Cow::Borrowed(_)),
                "padding forced an allocation in {s:?}"
            );
        }
    }

    /// The full markup path preserves constant width: two equal-width
    /// segment texts must produce equal-length markup. If a future change
    /// added trimming anywhere between `Segment` and the emitted string,
    /// every widget-level width test would still pass and only this one
    /// would fail.
    #[test]
    fn render_region_preserves_constant_segment_width() {
        let lens: Vec<usize> = ["   0B/s", " 1.2K/s", "12.7K/s", " 123K/s"]
            .iter()
            .map(|t| render_region(Direction::Left, &[seg(t, 31)], &theme()).chars().count())
            .collect();
        assert!(
            lens.windows(2).all(|p| p[0] == p[1]),
            "markup width jitters for equal-width segment texts: {lens:?}"
        );
    }
```

Run: `cargo test -p rustline-core render::tests::sanitize_preserves_padding_spaces render::tests::render_region_preserves_constant_segment_width`

Expected: PASS. These pass on today's `sanitize_text`/`render_region` — they
are characterization tests that pin behavior the feature now depends on, not
tests of new code.

- [ ] **Step 9: Fix the registry test in `widgets/mod.rs`**

In `crates/rustline-core/src/widgets/mod.rs`, the `throughput` registry test renders the default `" {down} {up}"` format with a 1024/2048 rate. Change:

```rust
        // default format " {down} {up}".
        assert_eq!(texts, vec!["  1.0K/s  2.0K/s".to_string()]);
```

(Two leading spaces: one from the format string, one from `format_rate`'s padding.)

- [ ] **Step 10: Run the full suite, clippy, and fmt**

```bash
cargo test --workspace
cargo clippy --all-targets -- -D warnings
cargo fmt --all
cargo fmt --all --check
```

Expected: all tests pass, clippy silent, fmt produces no diff. If clippy reports `format_bytes` as newly unused, that is a mistake — `disk.rs` still calls it; re-check that `memory.rs` was not edited.

- [ ] **Step 11: Commit**

```bash
git add crates/rustline-core/src/widgets/width.rs \
        crates/rustline-core/src/widgets/mod.rs \
        crates/rustline-core/src/widgets/throughput.rs \
        crates/rustline-core/src/render.rs
git commit -m "feat(throughput): constant-width byte rates

The {down}/{up} placeholders rendered through format_bytes, whose width
swings from 2 to 5 characters, so the segment and everything left of it
shifted sideways on every one-second refresh.

Add widgets/width.rs::format_rate: a 4-wide right-aligned numeral plus a
one-character unit, always exactly 5 characters, so a rendered rate is
always 7. The decimal is kept by measuring the rendered numeral rather
than by decade, so the 99.96K boundary can't overflow the field.
format_bytes is untouched for memory/disk's {used}/{total}."
```

---

### Task 2: `pad_percent` and the four percentage widgets

**Files:**
- Modify: `crates/rustline-core/src/widgets/width.rs` (add the second formatter + its tests)
- Modify: `crates/rustline-core/src/widgets/mod.rs` (extend the re-export, fix two test assertions at lines ~617 and ~656)
- Modify: `crates/rustline-core/src/widgets/cpu.rs` (one substitution, eight test assertions, one new seam test)
- Modify: `crates/rustline-core/src/widgets/memory.rs` (one substitution, five test assertions, one new seam test)
- Modify: `crates/rustline-core/src/widgets/battery.rs` (one substitution, four test assertions, one new seam test)
- Modify: `crates/rustline-core/src/widgets/disk.rs` (one substitution, three test assertions, one new seam test)

**Interfaces:**
- Consumes: `crate::widgets::width`'s module from Task 1, and its `UNITS`/`RATE_DIGITS` file layout (add alongside, do not disturb).
- Produces: `pub(crate) fn pad_percent(percent: u64) -> String`, re-exported as `crate::widgets::pad_percent`. Returns a right-aligned integer at least 3 characters wide.

- [ ] **Step 1: Write the failing tests**

Add to `crates/rustline-core/src/widgets/width.rs`, after `format_rate`. Include the constant, the stub, and the tests in this step:

```rust
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
    let _ = percent;
    String::new()
}
```

and to the test module:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rustline-core widgets::width`

Expected: `pad_percent_right_aligns_to_three` and `pad_percent_never_truncates` FAIL with `left: ""`. The two `format_rate` tests still PASS.

- [ ] **Step 3: Write the implementation**

Replace the `pad_percent` stub body in `crates/rustline-core/src/widgets/width.rs`:

```rust
pub(crate) fn pad_percent(percent: u64) -> String {
    format!("{percent:>w$}", w = PERCENT_DIGITS)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rustline-core widgets::width`

Expected: PASS, 4 tests.

- [ ] **Step 5: Extend the re-export**

In `crates/rustline-core/src/widgets/mod.rs`, change the Task 1 re-export line to:

```rust
pub(crate) use width::{format_rate, pad_percent};
```

- [ ] **Step 6: Wire `pad_percent` into all four widgets**

Each of the four widgets substitutes `{percent}` in exactly one *live* place. Change only the live path — leave every `None`-branch `.replace("{percent}", "")` exactly as it is.

`crates/rustline-core/src/widgets/cpu.rs` (in `render`, the `Some(c)` arm):

```rust
                    .replace("{percent}", &crate::widgets::pad_percent(percent))
```

`crates/rustline-core/src/widgets/memory.rs` (in `render`, the `Some(m)` arm) — identical line:

```rust
                    .replace("{percent}", &crate::widgets::pad_percent(percent))
```

`crates/rustline-core/src/widgets/disk.rs` (in `render`, the `Some(&d)` arm) — identical line:

```rust
                    .replace("{percent}", &crate::widgets::pad_percent(percent))
```

`crates/rustline-core/src/widgets/battery.rs` (in `render`, the `Some(b)` arm) — note the `u8` cast, since `Battery::percent` is a `u8`:

```rust
                    .replace("{percent}", &crate::widgets::pad_percent(b.percent as u64))
```

- [ ] **Step 7: Update the existing assertions in `cpu.rs`**

Every `{percent}` render gains a leading space at 2-digit values. In `crates/rustline-core/src/widgets/cpu.rs`'s test module, these eight assertions change:

| current | new |
|---|---|
| `"\u{f061a} ████░░░░ 50%"` | `"\u{f061a} ████░░░░  50%"` |
| `"50%"` (twice) | `" 50%"` |
| `"37%"` | `" 37%"` |
| `"38%"` | `" 38%"` |
| `"C 50%"` | `"C  50%"` |
| `"\u{f061a} 50%"` | `"\u{f061a}  50%"` |
| `"\u{f061a} 37%"` | `"\u{f061a}  37%"` |

- [ ] **Step 8: Update the existing assertions in `memory.rs`, `battery.rs`, `disk.rs`**

`crates/rustline-core/src/widgets/memory.rs`:

| current | new |
|---|---|
| `"6.0G/16G 10G 38%"` | `"6.0G/16G 10G  38%"` |
| `"0% ░░░░░░░░"` | `"  0% ░░░░░░░░"` |
| `"M 50%"` | `"M  50%"` |
| `"\u{f035b} 50%"` | `"\u{f035b}  50%"` |
| `"6.0G/16G 38%"` | `"6.0G/16G  38%"` |

Leave `format_bytes_humanizes` alone — `format_bytes` is not changing, so `"16G"`, `"6.2G"`, `"512M"`, `"1.5G"`, `"0B"`, `"512B"`, `"3.0T"` all stay.

`crates/rustline-core/src/widgets/battery.rs`:

| current | new |
|---|---|
| `"\u{f0080} 73% discharging"` (twice) | `"\u{f0080}  73% discharging"` |
| `"B 73%"` | `"B  73%"` |
| `"\u{f0080} 73%"` | `"\u{f0080}  73%"` |

`crates/rustline-core/src/widgets/disk.rs`:

| current | new |
|---|---|
| `"6.0G/16G 10G 38%"` | `"6.0G/16G 10G  38%"` |
| `"/home: 50%"` | `"/home:  50%"` |
| `"0% ░░░░░░░░"` | `"  0% ░░░░░░░░"` |

- [ ] **Step 9: Fix the two registry tests in `widgets/mod.rs`**

```rust
        // default format "{icon} {percent}%", 73% discharging -> md-battery-70.
        assert_eq!(texts, vec!["\u{f0080}  73%".to_string()]);
```

and, in `cpu_memory_registered_and_render_from_context`:

```rust
            vec![
                "\u{f061a}  50%".to_string(),
                "\u{f035b} 8.0G/16G".to_string()
            ]
```

(The `memory` entry is unchanged — its default format has no `{percent}`.)

- [ ] **Step 10: Add a rendered-width seam test to each of the four widgets**

Same reasoning as Task 1 Step 7: the assertion belongs at the rendered-segment seam, not only on `pad_percent`.

Append to `crates/rustline-core/src/widgets/cpu.rs`'s test module:

```rust
    /// Constant rendered width across the 1-, 2-, and 3-digit percentages,
    /// asserted on the segment text so a future render-pipeline change that
    /// trimmed padding would fail here rather than silently reintroduce the
    /// jitter.
    #[test]
    fn rendered_percent_width_is_constant() {
        let widths: Vec<usize> = [0.0f32, 7.0, 42.0, 100.0]
            .iter()
            .map(|&percent| {
                let out = w("{percent}%", "").render(&ctx(Some(CpuUsage { percent })));
                out[0].text.chars().count()
            })
            .collect();
        assert!(
            widths.windows(2).all(|p| p[0] == p[1]),
            "cpu render width jitters: {widths:?}"
        );
        assert_eq!(widths[0], 4); // 3-wide numeral + "%"
    }
```

Append to `crates/rustline-core/src/widgets/memory.rs`'s test module. The readings are chosen so `used / total` lands on 0%, 7%, 42% and 100%:

```rust
    #[test]
    fn rendered_percent_width_is_constant() {
        let g = 1024u64.pow(3);
        let widths: Vec<usize> = [0, 7, 42, 100]
            .iter()
            .map(|&pct| {
                let used = 100 * g / 100 * pct;
                let out = w("{percent}%", "").render(&ctx(mem(100 * g, used, 100 * g - used)));
                out[0].text.chars().count()
            })
            .collect();
        assert!(
            widths.windows(2).all(|p| p[0] == p[1]),
            "memory render width jitters: {widths:?}"
        );
        assert_eq!(widths[0], 4);
    }
```

Append to `crates/rustline-core/src/widgets/disk.rs`'s test module:

```rust
    #[test]
    fn rendered_percent_width_is_constant() {
        let g = 1024u64.pow(3);
        let widths: Vec<usize> = [0, 7, 42, 100]
            .iter()
            .map(|&pct| {
                let used = 100 * g / 100 * pct;
                let out = w("{percent}%", "").render(&ctx(disk(100 * g, used, 100 * g - used)));
                out[0].text.chars().count()
            })
            .collect();
        assert!(
            widths.windows(2).all(|p| p[0] == p[1]),
            "disk render width jitters: {widths:?}"
        );
        assert_eq!(widths[0], 4);
    }
```

Append to `crates/rustline-core/src/widgets/battery.rs`'s test module. The icon is pinned to a fixed glyph so the bucketed battery icon (which changes with the level) can't confound the width measurement — the test is about the numeral:

```rust
    #[test]
    fn rendered_percent_width_is_constant() {
        let widths: Vec<usize> = [0u8, 7, 42, 100]
            .iter()
            .map(|&percent| {
                let widget = BatteryWidget {
                    name: "battery".into(),
                    format: "{icon} {percent}%".into(),
                    alt_format: String::new(),
                    down_format: String::new(),
                    warn_percent: 20.0,
                    crit_percent: 10.0,
                    icon: Some("B".into()),
                };
                let out = widget.render(&ctx(bat(percent, BatteryState::Discharging)));
                out[0].text.chars().count()
            })
            .collect();
        assert!(
            widths.windows(2).all(|p| p[0] == p[1]),
            "battery render width jitters: {widths:?}"
        );
        assert_eq!(widths[0], 6); // "B" + " " + 3-wide numeral + "%"
    }
```

- [ ] **Step 11: Run the full suite, clippy, and fmt**

```bash
cargo test --workspace
cargo clippy --all-targets -- -D warnings
cargo fmt --all
cargo fmt --all --check
```

Expected: all tests pass, clippy silent, fmt produces no diff. If any assertion elsewhere in the workspace fails on a percentage string, update it to the padded form — the grep that produced the tables above covered `crates/`, but re-run `grep -rn --include='*.rs' -E '"[^"]*[0-9]%' crates` to confirm nothing was missed.

- [ ] **Step 12: Commit**

```bash
git add crates/rustline-core/src/widgets/
git commit -m "feat(widgets): constant-width {percent} on cpu/memory/battery/disk

cpu's default \"{icon} {percent}%\" swings two characters wide as the
reading crosses 10% and 100%, shifting every segment beside it. Right-align
the percentage into a 3-wide field via width.rs::pad_percent.

The width is a floor, not a clip: memory and disk derive the percentage
from an unclamped used/total, so an over-100 reading renders itself. The
down_format paths still collapse {percent} to empty (invariant #6)."
```

---

### Task 3: Documentation sync

**Files:**
- Modify: `README.md` (Throughput widget section)
- Modify: `docs/reference/config/widgets-optional.md` (the `throughput` paragraph)
- Modify: `docs/reference/config/widgets-default.md` (the `{percent}` widgets)
- Modify: `docs/reference/module-map/rustline-core/widgets.md` (add `width.rs`)

**Interfaces:**
- Consumes: the behavior shipped by Tasks 1 and 2. Produces no code.

- [ ] **Step 1: Read what the docs currently claim**

```bash
grep -rn "format_bytes\|1\.2M/s\|{percent}" README.md docs/reference/config/widgets-optional.md docs/reference/config/widgets-default.md docs/reference/module-map/rustline-core/widgets.md
```

Two statements are now factually wrong and must change, not merely be softened:
1. `docs/reference/config/widgets-optional.md` says `{down}`/`{up}` are "human-readable binary sizes suffixed `/s`, reusing `memory`'s `format_bytes`, e.g. `1.2M/s`". After Task 1 they do **not** reuse `format_bytes`.
2. `README.md`'s Throughput section repeats the same `e.g. 1.2M/s`.

- [ ] **Step 2: Update the throughput docs**

In both `README.md` and `docs/reference/config/widgets-optional.md`, replace the `format_bytes` / `1.2M/s` phrasing with the constant-width description. Use wording along these lines, adapted to each file's surrounding voice:

> `{down}`/`{up}` render as constant-width binary rates suffixed `/s` — a
> 4-character right-aligned numeral plus a one-character unit, always exactly
> seven characters (` 1.2K/s`, `12.7K/s`, ` 123K/s`, `   0B/s`). The decimal
> is kept only where it fits, so the widget's width never changes between
> refreshes. This is `width.rs`'s `format_rate`, not `memory`'s
> `format_bytes` — `{used}`/`{total}` keep the latter's variable-width output.

- [ ] **Step 3: Update the `{percent}` docs**

In `docs/reference/config/widgets-default.md`, note on the `cpu`/`memory`/`battery`/`disk` entries that `{percent}` renders right-aligned in a 3-character field (`  7`, ` 42`, `100`) so the widget's width is stable. Add the same one-line note to `README.md` wherever it documents those widgets' placeholders.

- [ ] **Step 4: Add `width.rs` to the module map**

In `docs/reference/module-map/rustline-core/widgets.md`, add a `width.rs` row/section matching the file's existing format for `alert.rs`/`bar.rs`/`spark.rs`, with a "Read when" framing: *Read when changing how a byte-rate or a percentage renders, or when adding a numeric placeholder that must not jitter.* State that it holds `format_rate` (5 chars, rates) and `pad_percent` (3-wide, percentages), that both are pure, and that `memory.rs`'s `format_bytes` is deliberately separate.

- [ ] **Step 5: Verify no stale claim survives**

```bash
grep -rn "1\.2M/s" README.md docs/
grep -rn "format_bytes" docs/
```

Expected: no hit claims `throughput` uses `format_bytes`. Any remaining `format_bytes` mention must be about `memory`/`disk`'s `{used}`/`{total}`/`{avail}`, which is still correct.

- [ ] **Step 6: Commit**

```bash
git add README.md docs/
git commit -m "docs: constant-width rates and percentages

Sync the throughput and {percent} documentation to width.rs's formatters,
and add width.rs to the rustline-core module map. The old 'reusing memory's
format_bytes, e.g. 1.2M/s' claim is now wrong on both counts."
```

---

## Final verification

After Task 3, before finishing the branch:

```bash
just test
just lint
cargo fmt --all --check
```

Then run the bar for real and confirm the throughput segment holds still across refreshes:

```bash
cargo run -- render right
```
