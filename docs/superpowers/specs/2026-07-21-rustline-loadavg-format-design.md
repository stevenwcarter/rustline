# Configurable `loadavg` format — design

**Date:** 2026-07-21
**Status:** Approved (shipped via `/ship-it` full bypass)

## Motivation

`loadavg` is the only widget in the **default** right layout with no
configurable output. It is a bare unit struct that hardcodes its formatting:

```rust
// crates/rustline-core/src/widgets/loadavg.rs (today)
Some([a, b, c]) => vec![Segment::new(format!("{a:.2} {b:.2} {c:.2}"))],
None => vec![],
```

and is registered with no config (`widgets/mod.rs`):

```rust
registry.register("loadavg", Box::new(|| Box::new(LoadAvg)));
```

Users cannot change the precision, add a label/glyph, reorder the three values,
or click-toggle it. Setting `[widgets.loadavg] alt_format = "…"` is **silently
ignored** because no `LoadAvgOpts` exists — this is exactly the surprise that
prompted the feature.

This change brings `loadavg` into the established **format-bearing widget
family** (`datetime`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`, `memory`),
making it the **seventh** such widget, with a `format`, an `alt_format`
(click-toggle), and a `down_format`.

## Design

### 1. Config: `[widgets.loadavg]`

Add `LoadAvgOpts` to `config.rs`, mirroring `CpuOpts`/`DateTimeOpts`, and wire
it into `WidgetOpts`:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoadAvgOpts {
    #[serde(default = "default_loadavg_format")]
    pub format: String,
    #[serde(default)]
    pub alt_format: String,
    #[serde(default)]
    pub down_format: String,
}

fn default_loadavg_format() -> String {
    "{load1} {load5} {load15}".into()
}
```

with a matching `Default` impl. All three fields are `#[serde(default)]`, so
`Config::load` stays **total** (invariant #3): a `[widgets.loadavg]` table may
specify any subset, and an absent/invalid one falls back to defaults.

`WidgetOpts` gains `#[serde(default)] pub loadavg: LoadAvgOpts`.

### 2. Placeholders + inline precision

Three placeholders, one per load value, each optionally carrying a Rust-style
precision spec `:.N`:

| Placeholder | Value | Default (`{loadN}`) | With spec |
|-------------|-------|---------------------|-----------|
| `{load1}`   | 1-min  | `0.42` (2 dp) | `{load1:.1}` → `0.4`, `{load1:.0}` → `0` |
| `{load5}`   | 5-min  | `0.31` | … |
| `{load15}`  | 15-min | `0.30` | … |

- A **bare** `{loadN}` renders at **2 decimals** — so the default format
  `"{load1} {load5} {load15}"` reproduces today's output **byte-for-byte**
  (`0.42 0.31 0.30`). This is the load-bearing invariant this feature depends
  on (see Invariants below).
- Per-value precision is allowed: `"{load1:.2} {load5:.1} {load15:.0}"`.
- Precision `N` is clamped to `0..=10` to avoid pathological output.

#### Substitution semantics (pure scanner)

A module-private pure function performs the substitution (the existing
`str::replace` approach can't vary precision per token). Signature:

```rust
/// Substitute `{load1|load5|load15}` placeholders (optional `:.N` precision,
/// default 2) in `fmt`. `values = Some([a,b,c])` formats each; `values = None`
/// (down state) collapses every recognized placeholder to the empty string.
/// Any unrecognized `{…}` token — unknown name, or a malformed precision spec —
/// is emitted verbatim, matching how the other widgets leave unknown
/// placeholders untouched.
fn substitute(fmt: &str, values: Option<[f64; 3]>) -> String
```

Scanner rules, applied left to right:

1. Text outside `{…}` is copied verbatim.
2. On `{`, read up to the next `}`. If there is no closing `}`, copy the
   remainder verbatim and stop.
3. Split the inner token on the first `:` into `name` and optional `spec`.
   - `name` must be exactly `load1`, `load5`, or `load15`; otherwise emit the
     original `{…}` (braces included) verbatim.
   - No `spec` → precision 2. A `spec` of the form `.<digits>` → that precision
     (clamped to ≤ 10). Any other `spec` shape → emit the original token
     verbatim (malformed passes through, never panics).
4. For a recognized `{loadN[:.N]}`:
   - `values = Some(v)` → `format!("{:.*}", precision, v[idx])`.
   - `values = None` → `""` (down-state collapse).

Multiple occurrences of the same placeholder are all substituted (the scanner
covers the whole string). Literal `{`/`}` that don't form a known token pass
through unchanged.

### 3. Widget: `LoadAvg` joins the family

```rust
pub struct LoadAvg {
    pub format: String,
    pub alt_format: String,
    pub down_format: String,
}

impl LoadAvg {
    pub const NAME: &'static str = "loadavg";
}

impl Widget for LoadAvg {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        match ctx.loadavg {
            Some(vals) => {
                let fmt = crate::widgets::active_format(
                    ctx, Self::NAME, &self.format, &self.alt_format);
                vec![Segment::new(substitute(fmt, Some(vals)))]
            }
            None => {
                if self.down_format.is_empty() {
                    return vec![];
                }
                vec![Segment::new(substitute(&self.down_format, None))]
            }
        }
    }

    fn range_name(&self) -> Option<&str> {
        crate::widgets::clickable_range(Self::NAME, &self.alt_format)
    }
}
```

This is the exact shape of `CpuWidget`/`DateTime`. Consequences:

- A non-empty `alt_format` makes `loadavg` **click-toggleable**. `"loadavg"` is
  7 bytes ≤ 15, so `clickable_range` returns `Some("loadavg")` and the assemble
  layer wraps it in `#[range=user|loadavg]…#[norange]` with no changes needed in
  `assemble.rs`/`render.rs` (they already handle `range_name()` generically).
- `down_format` (default `""`) is shown when `Context.loadavg` is `None` (a
  failed `getloadavg`), with placeholders collapsed to empty — same
  collapse-to-nothing behavior as `battery`/`cpu`/`memory`. Default empty ⇒
  renders nothing, exactly as today (invariant #6: never fake zeros).

### 4. Registration wiring

`widgets/mod.rs::with_builtins` replaces the bare `loadavg` registration with a
config-driven closure, mirroring `cpu`:

```rust
let loadavg = cfg.widgets.loadavg.clone();
registry.register(
    "loadavg",
    Box::new(move || {
        Box::new(LoadAvg {
            format: loadavg.format.clone(),
            alt_format: loadavg.alt_format.clone(),
            down_format: loadavg.down_format.clone(),
        })
    }),
);
```

The doc comment on `with_builtins` gains `loadavg` in its list of
option-carrying widgets.

## Invariants this feature depends on

- **#3 (`Config::load` is total):** all new fields `#[serde(default)]`. Covered
  by a malformed-`[widgets.loadavg]`-table fallback test.
- **#6 (`loadavg` is `Option`; never fake zeros):** `None` with an empty
  `down_format` still renders nothing (`vec![]`).
- **#7 (click-toggle name is one identity end-to-end):** the layout name
  `"loadavg"`, `NAME`, `active_format`/`clickable_range` key, and the emitted
  `range=user|loadavg` are all the single string `"loadavg"`.
- **Byte-identical default (new, load-bearing):** the default format at
  precision 2 must equal today's `"{a:.2} {b:.2} {c:.2}"` output. Pinned by a
  **characterization test** asserting `[0.42, 0.31, 0.296]` → `"0.42 0.31 0.30"`
  (note the `.296 → 0.30` rounding), so a later change to the scanner or the
  default can't silently alter existing bars.

## Testing

Pure/unit tests (TDD — the scanner has branching + parsing + math, so it is
genuinely test-worthy):

- **Scanner:** default precision (`{load1}` → 2 dp); inline `:.N`
  (`{load1:.1}`, `{load1:.0}`); per-value mixed precision; precision clamp
  (`.11`+ → ≤10 dp, no panic); unknown token passthrough (`{cpu}`, `{load2}`
  left verbatim); malformed spec passthrough (`{load1:x}`); literal text
  preserved; unterminated `{` passthrough; down-state collapse (`values =
  None` → recognized placeholders empty, unknowns verbatim).
- **Characterization:** `[0.42, 0.31, 0.296]` default format → `"0.42 0.31 0.30"`.
- **Widget:** `Some` uses `format`; `toggled` set uses `alt_format`; `None` +
  empty `down_format` → `[]`; `None` + non-empty `down_format` → collapsed
  text; `range_name()` is `Some("loadavg")` iff `alt_format` non-empty.
- **Config:** `[widgets.loadavg]` parse with defaults; default when absent;
  malformed table falls back to default (invariant #3).
- **Registry:** `loadavg` registered and renders from a configured `Context`.

Existing `formats_three_values`/`none_renders_nothing` tests are updated to go
through the new struct (constructing `LoadAvg` with the default format).

## Documentation

- **CLAUDE.md:** loadavg module-map note (now format-bearing); update the two
  "six format-bearing widgets" references (§133, §388) to seven incl.
  `loadavg`; add a `[widgets.loadavg]` mention in the Config section; note it
  in the click-toggle roster.
- **README.md:** add a short "Load average widget" subsection documenting
  `{load1}`/`{load5}`/`{load15}` + inline `:.N` precision, `alt_format`,
  `down_format`; add `loadavg` to the click-to-toggle roster (§175).

## Out of scope (YAGNI)

- `{a}`/`{b}`/`{c}` aliases — one placeholder scheme only.
- A separate `precision` config field — inline `:.N` covers it more flexibly.
- A combined `{load}` "all three" placeholder — the explicit three compose fine.
