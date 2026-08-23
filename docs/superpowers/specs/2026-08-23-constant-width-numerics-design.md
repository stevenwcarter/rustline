# Constant-width numeric fields (throughput rates + `{percent}`)

**Status:** approved 2026-08-23
**Scope:** `crates/rustline-core` widgets + doc sync. No config surface, no ABI change.

## Problem

The `throughput` widget's `{down}`/`{up}` placeholders render via `memory.rs`'s
`format_bytes`, whose output width swings between 2 and 5 characters
(`0B`, `42B`, `1.2K`, `512M`, `1023B`). At a one-second `status-interval` the
whole segment — and every segment to its left, since `render_region` joins
segments positionally — shifts sideways on nearly every refresh. The bar
visibly jitters.

The same class of jitter exists in the *default* layout: `cpu`'s default
format `"{icon} {percent}%"` swings `7%` → `42%` → `100%`, two characters of
movement every refresh, and `memory`/`battery`/`disk` share the `{percent}`
placeholder.

The numeral is the only variable-width part of these renders — the unit letter
(`B K M G T`) is always one character and `%` is always one character — so
making the numeral a fixed-width right-aligned field makes the whole component
width-stable. That is the entire fix; no layout, padding, or alignment
machinery in the render pipeline is needed.

## Decisions

Two decisions were taken during brainstorming and are settled:

1. **Always on**, not opt-in. No `fixed_width` config key and no parallel
   `{down_pad}` placeholder. This deliberately departs from the project's usual
   "every option defaults to reproducing pre-feature output byte-for-byte"
   habit: the pre-feature output is the bug, and a knob to re-enable jitter has
   no user.
2. **Scope is `throughput` + `{percent}`.** The `{used}`/`{total}`/`{avail}`
   placeholders on `memory`/`disk` keep using `format_bytes` unchanged and are
   explicitly out of scope for this change.

## Design

### New module: `crates/rustline-core/src/widgets/width.rs`

A private module holding both formatters, following the existing
`alert.rs` / `bar.rs` / `spark.rs` pattern (private `mod`, `pub(crate) fn`,
re-exported from `widgets/mod.rs`). Both functions are pure and take no
`Context`.

#### `pub(crate) fn format_rate(bytes_per_sec: u64) -> String`

Selects a unit with the same 1024-based ladder as `format_bytes`
(`B K M G T`, largest unit whose scaled value is `>= 1`), then renders the
numeral right-aligned into a **4-character field**, keeping a decimal only
where one fits:

```
(value, unit) = scale_1024(bytes_per_sec)          // B K M G T
numeral = if unit == B {
              bytes_per_sec.to_string()            // bytes are integers
          } else {
              let one = format!("{value:.1}");
              if one.len() <= 4 { one }             // room for the decimal
              else { format!("{value:.0}") }        // no room -> drop it
          };
format!("{numeral:>4}{unit}")
```

The decimal is decided by **measuring the rendered string**, not by which
decade the value falls in. A band rule (`< 10` -> one decimal, `>= 100` ->
none) is wrong at the rounding boundaries: `99.96 K` is in the two-digit band
but `format!("{:.1}")` yields `"100.0"`, five characters. Measuring catches
that and drops to `"100"`.

The resulting numeral is always `<= 4` characters, because the scaled value is
always `< 1024` and `format!("{:.0}")` of any such value is at most `"1024"`.

| input | scaled | numeral | rendered |
|---|---|---|---|
| `0` | `0 B` | `   0` | `   0B/s` |
| `42` | `42 B` | `  42` | `  42B/s` |
| `1023` | `1023 B` | `1023` | `1023B/s` |
| `1234` | `1.205 K` | ` 1.2` | ` 1.2K/s` |
| `13_000` | `12.70 K` | `12.7` | `12.7K/s` |
| `102_360` | `99.96 K` | ` 100` | ` 100K/s` |
| `126_000` | `123.0 K` | ` 123` | ` 123K/s` |
| `1024^3` | `1.0 G` | ` 1.0` | ` 1.0G/s` |

`format_rate` returns the numeral **plus the unit letter** - always exactly 5
characters. `throughput.rs` appends `/s`, so a rendered rate is always exactly
**7 characters**.

Two properties worth stating explicitly because they are easy to get wrong:

- This **diverges from `format_bytes`** for two-digit values.
  `format_bytes` drops the decimal at `>= 10` (`13K`); `format_rate` keeps it
  (`12.7K`), because a 4-wide field has room and the extra digit is free.
  `format_bytes` is **not** modified — `memory` and `disk` must keep their
  current output.
- The width is a **minimum, never a truncation**. Values are never clipped to
  fit. The unit ladder caps at `T` with a scaled value `< 1024`, so the field
  overflows only above 1024 TiB/s, which is not reachable from a NIC counter
  diff; if it ever were, the render would grow wider rather than wrong —
  `u64::MAX` renders `"16777216T"`, 9 characters, not 6.

#### `pub(crate) fn pad_percent(percent: u64) -> String`

Right-aligns an integer percentage into a **3-character field**: `"  7"`,
`" 42"`, `"100"`. Same minimum-not-truncation rule — `memory` and `disk`
compute `percent` from a `used / total` fraction that is not clamped, so a
pathological `>100` value must render as itself (`"105"`, `"1000"`), never be
cut.

### Call-site changes

| File | Change |
|---|---|
| `widgets/throughput.rs` | `format_bytes(t.down_bytes_per_sec)` → `format_rate(...)`, same for `up`. Drop the now-unused `format_bytes` import. |
| `widgets/cpu.rs` | `.replace("{percent}", &percent.to_string())` → `&pad_percent(percent)` |
| `widgets/memory.rs` | same |
| `widgets/battery.rs` | same, on `b.percent as u64` |
| `widgets/disk.rs` | same |
| `widgets/mod.rs` | `mod width;` + `pub(crate) use width::{format_rate, pad_percent};` |

**The `down_format` paths are not changed.** All five widgets collapse
`{percent}`/`{down}`/`{up}` to `""` when the reading is `None`, and that stays.
Padding a reading that does not exist would fabricate width for a value the
widget is specifically declining to invent (invariant #6: a missing reading is
`Option`, never a fabricated zero — and never a fabricated three spaces).

`format_bytes` stays `pub(crate)` in `memory.rs` and keeps its `disk.rs`
caller.

## Invariants this feature depends on

- **`render_region` joins segment text verbatim.** Leading spaces inside a
  segment survive to the tmux markup — they are not trimmed, collapsed, or
  normalized anywhere between `Widget::render` and the emitted
  `#[fg=..,bg=..]` string. If a future change adds trimming to the render
  pipeline, every constant-width guarantee here dies silently.
- **`sanitize_text` does not alter spaces.** It guards `#` (invariant #8); it
  must keep passing ASCII space through untouched.
- **The unit letter is exactly one character** for every unit `format_rate`
  can select, and `%` is one character. Constant total width follows from
  constant numeral width only while that holds.

These are pinned by tests below rather than left as prose, per the project's
"no test needed because of invariant X is a red flag" rule.

## Testing

The value tables are the cheap part. The **load-bearing** tests are the width
assertions, and they are placed at the seam the feature actually crosses —
the rendered segment text — not only on the pure function.

1. **`width.rs` unit tests.** Value table for `format_rate` covering each
   precision band and each unit (`0`, `42`, `1023`, `1024`, `1234`, `13_000`,
   `126_000`, `1024^2`, `1024^3`, `1024^4`), **including the `102_360`
   (`99.96 K`) rounding boundary that a band-based rule gets wrong**, and for
   `pad_percent` (`0`, `7`, `42`, `100`, and an over-100 no-truncation case).
2. **`format_rate` width sweep.** Over a wide generated range of inputs
   (powers and near-boundary values across every unit), assert
   `format_rate(n).chars().count() == 5`. This is the property the whole
   feature exists for.
3. **Rendered-width seam test, throughput.** Render `ThroughputWidget` with the
   default `" {down} {up}"` format across a sweep of `(down, up)` rate pairs
   spanning all units and precision bands, and assert every resulting
   `Segment::text` has an identical `chars().count()`. This is the test that
   would have caught the original bug and that catches a future render-pipeline
   trim.
4. **Rendered-width seam test, `{percent}`.** For each of `cpu`, `memory`,
   `battery`, `disk`: render the widget's default format across readings
   producing 1-, 2-, and 3-digit percentages and assert constant
   `chars().count()`.
5. **Existing assertions updated.** Roughly 49 literal expectations across the
   four `{percent}` widget test modules plus `throughput.rs`, and any rendered
   previews in `widgets/mod.rs`, `render.rs`, `widget_cmd.rs`, `init.rs`, and
   `sample_context.rs`. These are updated to the padded strings, not deleted.

`cargo test --workspace` must be green, and the tree must stay clippy-clean
(`cargo clippy --all-targets -- -D warnings`) and rustfmt-clean.

## Documentation

The current docs state the old behavior in prose and examples and would become
wrong:

- `README.md` — the Throughput widget section's `e.g. 1.2M/s`, and the widget
  table entries showing unpadded `{percent}`.
- `docs/reference/config/widgets-optional.md` — the `throughput` paragraph's
  "human-readable binary sizes suffixed `/s`, reusing `memory`'s
  `format_bytes`, e.g. `1.2M/s`", which is doubly stale after this change
  (it no longer reuses `format_bytes`).
- `docs/reference/config/widgets-default.md` — `{percent}` on
  `cpu`/`memory`/`battery`/`disk`.
- `docs/reference/module-map/rustline-core/widgets.md` — add `width.rs` to the
  module list with its "Read when" framing.

`CLAUDE.md` needs no change: it is a high-level map and names no formatter.

## Out of scope

- Padding `{used}`/`{total}`/`{avail}` (`memory`, `disk`). `format_bytes` is
  untouched.
- `loadavg`'s figures.
- The pre-existing `format_bytes` cosmetic quirk where `1023.6 KiB` renders
  `1024K` rather than rolling to `1.0M`. `format_rate` inherits the same
  rollover point; it is still constant-width, so it does not jitter.
- Any config key, any ABI or wire-type change, any change to WASM plugin
  rendering (plugins emit their own `Segment`s and are unaffected).
