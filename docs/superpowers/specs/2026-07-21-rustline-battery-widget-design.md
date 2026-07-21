# rustline `battery` widget + OS-specific reads + `os`/`arch` on `Context` — design

**Status:** approved (brainstorm, 2026-07-21)
**Depends on:** the v1 core (`docs/superpowers/specs/2026-07-20-rustline-tmux-statusline-design.md`)
**Scope:** Add a built-in `battery` widget backed by a single platform-specific
host read (Linux sysfs / macOS `pmset`), establishing the pattern for keeping
OS-specific surfaces minimal and pinned to the `Context`-build edge; and add
`os`/`arch` to `Context` so WASM plugins can branch on platform.

## 1. Purpose & success criteria

rustline should show battery state on the status line for laptops, and — as the
first widget whose underlying value genuinely differs per OS — it drives the
project's answer to "how do we do platform-specific reads?" The answer mirrors
the existing `loadavg`/`interfaces` reads: the **platform-specific read lives at
the `Context`-build edge in the binary**, captured into a typed `Context` field;
the **widget is pure** and reads only from `Context`. The OS-specific surface is
one small function with `#[cfg(target_os = …)]` arms, each delegating to a
**pure, unconditionally-compiled parser** that is unit-tested on the Linux dev
box regardless of the arm's target.

Separately, the WASM `Context` gains `os` and `arch` strings so a future guest
plugin can adapt to the host platform (the TODO's third bullet). The `battery`
widget itself is a built-in and does not need these — they are additive
platform metadata for the plugin ABI.

**Success when:**

1. Adding `"battery"` to a layout region on a laptop renders the battery, e.g.
   `󰂁 73%`, reflecting the live charge percentage.
2. The `format` option composes `{percent}`, `{state}` (a word:
   `charging`/`discharging`/`full`/`unknown`), and `{icon}` (a level-bucketed,
   charging-aware Nerd-Font glyph) with any surrounding literal text, printed
   verbatim (default `format = "{icon} {percent}%"`).
3. On a desktop / VM / any host with no battery — and on any OS other than Linux
   or macOS — `Context.battery` is `None` and the widget renders `down_format`
   (default `""` → renders nothing, skips the widget: the invariant-#6 default).
   No fabricated `0%` ever appears.
4. On Linux the value comes from `/sys/class/power_supply/BAT*/{capacity,status}`;
   on macOS from `pmset -g batt`. The **parsers** for both formats are pure and
   unit-tested on the (Linux) dev host; only the file-read / subprocess-spawn is
   the untestable `#[cfg]` edge.
5. `Context` gains `os: String` and `arch: String` (from
   `std::env::consts::OS`/`ARCH`), serde round-trips, and is additive to the ABI
   (existing guests ignore the new keys).
6. Zero-config still works (`battery` is **not** in the default layout — opt in
   by naming it); a bad/partial `[widgets.battery]` never breaks the bar
   (invariant #3); no new heavyweight dependency enters the graph (`cargo tree -i
   openssl`/`-i native-tls` stay empty); `just test` stays hermetic; clippy/fmt
   clean.

**Non-goals (deferred):** low-battery warning color / thresholds (needs theme
plumbing — a natural follow-up); time-to-empty / time-to-full; per-battery
selection on multi-battery hosts (aggregate/first battery is enough for v1);
Windows/BSD battery reads (the `#[cfg]` fallback yields `None`, so they build and
degrade cleanly — a later arm can add them); IOKit-based macOS read (the `pmset`
subprocess is the v1 mechanism; IOKit is a later swap if the subprocess proves
undesirable — see §4).

## 2. Architecture overview

```
crates/
  rustline-core/     pure. Gains:
                       - context.rs:  Battery + BatteryState types; Context
                         gains `battery: Option<Battery>`, `os: String`,
                         `arch: String`
                       - widgets/battery.rs: pure widget (icon bucketing +
                         format; reads ONLY from Context)
                       - config.rs:   BatteryOpts under WidgetOpts
                       - widgets/mod.rs: register the "battery" name
                     NO new dependency. NO I/O.
  rustline/          bin. Gains:
                       - battery.rs: read_battery() (#[cfg] arms) + the pure
                         parsers parse_linux()/parse_pmset() (compiled on all
                         targets, unit-tested)
                       - build_context.rs: call read_battery(); populate
                         os/arch from std::env::consts
                     NO new dependency (sysfs = std::fs; pmset = std::process).
```

The **read** lives in the binary's `battery.rs`, keeping `rustline-core` I/O-free.
The **decision logic** — which glyph for a given percent+state, how to format —
is pure logic in the widget, operating on `Context.battery`, fully unit-testable
with no hardware and reusable verbatim by a future daemon front-end.

This upholds **invariant #1** (widgets read only from `Context`, never the
environment mid-render): the battery snapshot is captured into `Context` at build
time; the widget never touches the OS or spawns a process.

**Why the parsers are their own pure functions.** `read_battery` is
`#[cfg]`-split, so on the Linux dev box the macOS arm never compiles and its
logic would otherwise be untested. Splitting each arm into a thin
platform-gated reader (does the I/O) plus an unconditionally-compiled pure parser
(`&str`/`&str` → `Option<Battery>`) means `parse_pmset` is compiled and tested on
Linux even though `read_battery_macos` is not. This is the crux of "minimal
OS-specific surface": the only code that is truly platform-gated is the handful
of lines that read a file or spawn a process.

## 3. Data model

New types in `rustline-core/src/context.rs` (alongside `NetIface`):

```rust
/// Charge state of the battery. Small typed domain (not a stringly value):
/// maps from Linux sysfs `status` and macOS `pmset` state words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatteryState {
    Charging,
    Discharging,
    Full,
    Unknown,
}

/// A battery snapshot captured at Context-build time. `percent` is 0..=100.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Battery {
    pub percent: u8,
    pub state: BatteryState,
}
```

`Context` gains three fields:

```rust
pub battery: Option<Battery>,   // None: no battery / unsupported OS / read failed
pub os: String,                 // std::env::consts::OS  e.g. "linux", "macos"
pub arch: String,               // std::env::consts::ARCH e.g. "x86_64", "aarch64"
```

All three are serde-serializable, so **invariant #2** (Context/Segment/… stay
serde-serializable — the WASM ABI) holds; pinned by the round-trip test. The
fields are **additive**: existing WASM guests that hand-parse `Context` JSON
ignore unknown keys, so no guest breaks. `BatteryState` serializes as a
snake_case string (`"charging"`, …) — stable and readable for a hand-parsing
guest. Every construction site of `Context` (the fixtures in `context.rs`,
`datetime.rs`, `loadavg.rs`, `mod.rs`, plus binary/test builders) must set the
new fields.

`battery: Option<Battery>` follows `loadavg: Option<[f64;3]>` exactly: absence is
represented, never faked (invariant #6). `percent` is `u8` clamped to `0..=100`
by the parsers.

## 4. Platform read (`rustline/src/battery.rs`)

```rust
use rustline_core::{Battery, BatteryState};

/// Read the host battery, or None if there is no battery / the platform is
/// unsupported / the read failed. Called once at Context-build time.
pub fn read_battery() -> Option<Battery> {
    #[cfg(target_os = "linux")]
    { read_battery_linux() }
    #[cfg(target_os = "macos")]
    { read_battery_macos() }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    { None }
}
```

**Linux** (`#[cfg(target_os = "linux")]`): enumerate `/sys/class/power_supply/*`,
pick the first entry whose `type` file reads `Battery` (skips `Mains`/AC and
`type`-less entries), read its `capacity` (integer percent) and `status`
(`Charging`/`Discharging`/`Full`/`Not charging`/`Unknown`) files, and hand both
strings to the pure `parse_linux`. A missing/unreadable file → `None`.

**macOS** (`#[cfg(target_os = "macos")]`): run `pmset -g batt`, capture stdout,
hand it to the pure `parse_pmset`. A spawn failure / non-UTF-8 output → `None`.

**Pure parsers (compiled on all targets, unit-tested):**

```rust
/// Linux: (capacity, status) file contents -> Battery.
pub(crate) fn parse_linux(capacity: &str, status: &str) -> Option<Battery>;

/// macOS: full `pmset -g batt` stdout -> Battery.
pub(crate) fn parse_pmset(output: &str) -> Option<Battery>;
```

- `parse_linux`: trim + parse `capacity` as `u8`, clamp to `0..=100`; map
  `status` (case-insensitively, trimmed): `Charging`→`Charging`,
  `Discharging`→`Discharging`, `Full`→`Full`, `Not charging`→`Full` (plugged in
  and topped off — treated as `Full` for display; alternative words fall through
  to `Unknown`). A non-numeric capacity → `None`.
- `parse_pmset`: find the line describing the internal battery; extract the
  `NN%` percentage (the token immediately before `%`) and the state word after
  the `;` (`charging`→`Charging`, `discharging`→`Discharging`,
  `charged`→`Full`, `finishing charge`→`Charging`, anything else →`Unknown`).
  `No internal battery` / no percentage found → `None`.

  Reference `pmset -g batt` shapes the parser must handle (captured as test
  fixtures):
  ```
  Now drawing from 'Battery Power'
   -InternalBattery-0 (id=…)	73%; discharging; 3:21 remaining present: true
  ```
  ```
  Now drawing from 'AC Power'
   -InternalBattery-0 (id=…)	100%; charged; 0:00 remaining present: true
  ```
  ```
  Now drawing from 'AC Power'
   -InternalBattery-0 (id=…)	46%; charging; 1:12 remaining present: true
  ```

Neither parser does I/O, so both are covered by ordinary unit tests on Linux.
`read_battery` (the I/O wrapper) is thin, like `read_loadavg`, and is exercised
only by a "never panics" smoke assertion.

## 5. Widget (`rustline-core/src/widgets/battery.rs`)

```rust
pub struct BatteryWidget {     // widget; distinct from context::Battery data type
    pub format: String,        // default "{icon} {percent}%"
    pub down_format: String,   // default ""
}
```

> Naming note: the widget struct and the data struct would collide as `Battery`.
> The widget module imports the data type as `use crate::Battery as BatteryData;`
> (or refers to it as `context::Battery`) and names the **widget** struct
> `BatteryWidget` — matching how the module's public name is the registry key
> `"battery"`, not the struct name.

`render(&Context) -> Vec<Segment>`:

1. `match ctx.battery`:
2. **`Some(b)`** → one `Segment` whose text is `format` with `{percent}` → the
   number, `{state}` → the state word, `{icon}` → `battery_icon(b)`.
3. **`None`** → empty `down_format` → `vec![]` (widget skipped, invariant #6);
   else one `Segment` of `down_format` with `{percent}`/`{state}`/`{icon}`
   collapsed to empty (so a stray placeholder never renders literally and no fake
   value shows).

Substitution is plain literal-token replacement, like the IP widgets' `{ip}`.

**`battery_icon(b: &Battery) -> &'static str`** — pure, unit-tested. Charging (or
`Full` while implied plugged) shows a charging glyph; otherwise a level bucket:

| condition                    | glyph (Nerd Font, nf-md battery ramp) |
|------------------------------|----------------------------------------|
| `state == Charging`          | `󰂄` (`battery-charging`)               |
| `state == Full`              | `󰁹` (`battery` / full)                 |
| discharging, `percent >= 90` | `󰂂`                                    |
| discharging, `>= 70`         | `󰂀`                                    |
| discharging, `>= 50`         | `󰁿`                                    |
| discharging, `>= 30`         | `󰁽`                                    |
| discharging, `>= 10`         | `󰁻`                                    |
| discharging, `< 10`          | `󰂎` (`battery-alert`/low)              |
| `Unknown`                    | `󰂑` (`battery-unknown`)                |

(Exact codepoints are finalized in implementation from a Nerd Font chart; the
**bucketing logic** is what the tests pin. Glyphs require a Nerd/powerline font,
already a project rendering prerequisite.)

## 6. Config

`rustline-core/src/config.rs` — one new opt struct, added to `WidgetOpts` (every
field `#[serde(default)]`, upholding **invariant #3**, total load):

```rust
fn default_battery_format() -> String { "{icon} {percent}%".into() }

pub struct BatteryOpts {
    #[serde(default = "default_battery_format")]  // "{icon} {percent}%"
    pub format: String,
    #[serde(default)]                              // ""
    pub down_format: String,
}

pub struct WidgetOpts {
    // ...existing datetime, cwd, lan_ip, tailscale_ip...
    #[serde(default)] pub battery: BatteryOpts,
}
```

Example config:

```toml
[layout]
right = ["battery", "cwd", "loadavg", "datetime"]

[widgets.battery]
format = "{icon} {percent}%"   # default; or "{icon} {percent}% {state}"
down_format = ""               # default: render nothing when no battery
```

## 7. Wiring

- `rustline-core/src/widgets/mod.rs`: `Registry::with_builtins` registers
  `"battery"`, its factory capturing `cfg.widgets.battery` (mirrors how
  `lan_ip`/`tailscale_ip` capture theirs). Doc comment updates the built-in
  widget count (eight → nine).
- `rustline/src/battery.rs`: new module (see §4). Declared in `main.rs`.
- `rustline/src/build_context.rs`: `build_region_context` sets
  `battery: crate::battery::read_battery()`, `os: std::env::consts::OS.into()`,
  `arch: std::env::consts::ARCH.into()`. `build_window_context` reuses
  `build_region_context` and so inherits them (a uniform `Context` is cheap and
  correct; windows won't normally show `battery`).
- No `Cargo.toml` change: Linux read is `std::fs`, macOS read is
  `std::process::Command` — both in std. The dependency graph is unchanged, so
  the rustls/OpenSSL-free invariant is trivially preserved. `Cargo.lock` needs no
  change.

## 8. Testing (TDD — load-bearing first)

Pure parsers (`rustline/src/battery.rs`, compiled & tested on Linux):
- `parse_linux`: `("73\n","Discharging\n")` → `73`/`Discharging`;
  `("100","Full")` → `Full`; `("55","Charging")` → `Charging`;
  `("100","Not charging")` → `Full`; non-numeric capacity (`("x","Full")`) →
  `None`; out-of-range capacity clamps to `100`; unknown status word →
  `Unknown`.
- `parse_pmset`: the three reference outputs in §4 → `(73,Discharging)`,
  `(100,Full)`, `(46,Charging)`; a `No internal battery` output → `None`; a
  malformed line with no `%` → `None`.
- `read_battery()` returns without panicking (value is host-dependent; the test
  only asserts it does not panic and, if `Some`, `percent <= 100`).

Widget (`widgets/battery.rs`):
- `battery_icon` bucket boundaries: `Charging` at any percent → charging glyph;
  `Full` → full glyph; discharging at 90/89/70/50/30/10/9 land in the expected
  buckets; `Unknown` → unknown glyph.
- `format` substitution: `{icon}`/`{percent}`/`{state}` replaced; surrounding
  literal text preserved verbatim.
- `None` + empty `down_format` → `vec![]` (skipped).
- `None` + non-empty `down_format` → that text renders; any placeholder inside it
  collapses to empty.

Config (`config.rs`):
- Parse `[widgets.battery]` with a partial table → defaults fill the rest;
  absent → `format == "{icon} {percent}%"`, `down_format == ""`.
- Total-load fallback: a malformed `[widgets.battery]` table → `Config::default`.

Cross-cutting:
- `Context` serde round-trip includes `battery`, `os`, `arch` (invariant #2);
  `BatteryState` serializes to the expected snake_case strings.
- Smoke test (`crates/rustline/tests/smoke.rs`): with `"battery"` in a layout
  region, the name resolves in the registry; a synthesized `Context` with a known
  `Some(Battery{..})` renders the expected `{icon} {percent}%`; and with
  `battery: None` + default `down_format`, the region omits it.

## 9. Invariants this feature depends on / touches

- **#1 (Context is the sole render input):** upheld — the battery snapshot and
  os/arch are captured into `Context` at build time (the read/`pmset` spawn is at
  the edge, not mid-render); the widget never reads the OS. Load-bearing test:
  the smoke test rendering from a synthesized `Context`.
- **#2 (serde-serializable Context/Segment):** upheld — `Battery`/`BatteryState`
  and the new string fields derive/round-trip serde; pinned by the round-trip
  test and the snake_case-string assertion.
- **#3 (Config::load is total):** upheld — `BatteryOpts` fields are all
  `#[serde(default)]`; pinned by the total-load fallback test.
- **#6 (Option semantics, no fake data; panicking widget → empty):** the
  no-battery / unsupported-OS case renders `down_format` (default empty →
  nothing), never a fabricated `0%`; pinned by the `None`-branch widget tests.
  `read_battery` returning `None` on non-Linux/macOS keeps the binary building
  and degrading cleanly everywhere.

## 10. Documentation

Update `CLAUDE.md`: module map (`widgets/battery.rs`, the
`Battery`/`BatteryState`/`battery`/`os`/`arch` additions to `context.rs`, the new
`rustline/src/battery.rs` and its call from `build_context.rs`), the built-in
widget count (eight → nine), the Config section (the `[widgets.battery]` table
and `format`/`down_format`), and a line in the Roadmap / a short note recording
the "platform-specific reads stay at the `Context`-build edge; each `#[cfg]` arm
delegates to a pure, unconditionally-compiled parser" pattern (and that `os`/
`arch` are now on `Context` for guests). Update `README.md`'s widget list and add
the `[widgets.battery]` config example. Link this spec from the Design-docs list.
Remove the corresponding line from `TODO.md`.
