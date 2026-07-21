# Battery Widget + OS-Specific Reads + os/arch on Context — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a built-in `battery` widget fed by a single platform-gated host read (Linux sysfs / macOS `pmset`) whose pure parsers are unit-tested on the Linux dev box, plus `os`/`arch` fields on `Context` for WASM plugins.

**Architecture:** The platform-specific read lives at the `Context`-build edge in the binary (`rustline/src/battery.rs`), mirroring `read_loadavg`/`read_interfaces`; it captures a typed `Option<Battery>` into `Context`. The widget (`rustline-core`) is pure and reads only from `Context`. Each `#[cfg(target_os)]` arm delegates to a pure parser that is `#[cfg(…, test)]`-compiled and tested on any host.

**Tech Stack:** Rust edition 2024, workspace; `std::fs`/`std::process` for the reads (no new dependency); `serde` for the ABI; `chrono` in `Context`.

## Global Constraints

- **Edition 2024** in every crate; keep all crate editions equal to `rustfmt.toml`.
- Must stay **clippy-clean**: `cargo clippy --all-targets -- -D warnings`.
- Must stay **rustfmt-clean**: `cargo fmt --all --check`. **No pre-commit hook** — run `cargo fmt --all` before every commit.
- **`just test` stays hermetic** — no wasm toolchain required by any test added here.
- **No OpenSSL / native-tls** may enter the graph (`cargo tree -i openssl` / `-i native-tls` stay empty). This feature adds **no new dependency**, so `Cargo.lock` is unchanged.
- **Invariant #1:** widgets read only from `Context`, never the environment mid-render.
- **Invariant #2:** `Context`/`Segment`/`Style`/`Color` stay serde-serializable (the WASM ABI). New `Context` fields are additive.
- **Invariant #3:** `Config::load` is total — a bad/partial config never breaks the bar; every new config field is `#[serde(default)]`.
- **Invariant #6:** no fabricated data — absence is `None`, rendered as `down_format` (default empty → nothing).

---

### Task 1: `Context` data model — `Battery`/`BatteryState` types + `battery`/`os`/`arch` fields

**Files:**
- Modify: `crates/rustline-core/src/context.rs` (add types, three fields, update `sample()` fixture + round-trip tests)
- Modify: `crates/rustline-core/src/lib.rs:14` (re-export `Battery`, `BatteryState`)
- Modify (add the three fields to each `Context { … }` literal): `crates/rustline-core/src/widget.rs:82`, `crates/rustline-core/src/assemble.rs:86`, `crates/rustline-core/src/widgets/loadavg.rs:25`, `crates/rustline-core/src/widgets/mod.rs:80`, `crates/rustline-core/src/widgets/tailscale_ip.rs:24`, `crates/rustline-core/src/widgets/cwd.rs:63,95,104,113`, `crates/rustline-core/src/widgets/windows.rs:33`, `crates/rustline-core/src/widgets/lan_ip.rs:25`, `crates/rustline-core/src/widgets/pane_id.rs:24`, `crates/rustline-core/src/widgets/datetime.rs:29`, `crates/rustline-core/src/widgets/hostname.rs:25`
- Modify: `crates/rustline/src/build_context.rs:53` (set `os`/`arch` from `std::env::consts`, `battery: None` for now — Task 2 wires the read)
- Modify: `crates/rustline-wasm/tests/e2e.rs:48` (feature-gated fixture — keeps `just test-wasm` green)

**Interfaces:**
- Produces:
  - `rustline_core::BatteryState` — `enum { Charging, Discharging, Full, Unknown }`, `#[serde(rename_all = "snake_case")]`, `Copy`.
  - `rustline_core::Battery` — `struct { pub percent: u8, pub state: BatteryState }`, `Copy`.
  - `Context.battery: Option<Battery>`, `Context.os: String`, `Context.arch: String`.

- [ ] **Step 1: Write the failing test** — append to `crates/rustline-core/src/context.rs` `mod tests`, and update `sample()` to populate the new fields:

Change `sample()`'s literal to include (add after `interfaces: vec![…]`):
```rust
            battery: Some(Battery {
                percent: 73,
                state: BatteryState::Discharging,
            }),
            os: "linux".into(),
            arch: "x86_64".into(),
```
Then add these tests:
```rust
    #[test]
    fn context_battery_os_arch_survive_serde() {
        let ctx = sample();
        let json = serde_json::to_string(&ctx).unwrap();
        let back: Context = serde_json::from_str(&json).unwrap();
        assert_eq!(back.battery, ctx.battery);
        assert_eq!(back.os, "linux");
        assert_eq!(back.arch, "x86_64");
    }

    #[test]
    fn battery_state_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&BatteryState::Discharging).unwrap(),
            "\"discharging\""
        );
        assert_eq!(
            serde_json::to_string(&BatteryState::Full).unwrap(),
            "\"full\""
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail (do not compile)**

Run: `cargo test -p rustline-core context 2>&1 | tail -20`
Expected: FAIL — `cannot find type Battery`, `no field battery on Context`.

- [ ] **Step 3: Add the types + fields**

In `crates/rustline-core/src/context.rs`, after the `NetIface` struct (before the `Context` doc comment), add:
```rust
/// Charge state of the host battery. A small typed domain — not a stringly
/// value — mapped from the Linux sysfs `status` file and macOS `pmset` state
/// words at Context-build time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatteryState {
    Charging,
    Discharging,
    Full,
    Unknown,
}

/// A battery snapshot captured at Context-build time. `percent` is `0..=100`.
///
/// `Context::battery` is `None` on hosts without a battery, on unsupported
/// platforms, or when the read failed — never a fabricated `0%` (invariant #6),
/// mirroring `loadavg`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Battery {
    pub percent: u8,
    pub state: BatteryState,
}
```

In the `Context` struct, after the `interfaces` field, add:
```rust
    /// Battery snapshot read once at build time; `None` when absent/unsupported.
    pub battery: Option<Battery>,
    /// Host OS (`std::env::consts::OS`, e.g. `"linux"`, `"macos"`). Additive
    /// platform metadata for WASM guests.
    pub os: String,
    /// Host CPU arch (`std::env::consts::ARCH`, e.g. `"x86_64"`, `"aarch64"`).
    pub arch: String,
```

In `crates/rustline-core/src/lib.rs:14`, change:
```rust
pub use context::{Context, NetIface, WindowCtx};
```
to:
```rust
pub use context::{Battery, BatteryState, Context, NetIface, WindowCtx};
```

- [ ] **Step 4: Update every other `Context { … }` literal so the workspace compiles**

To **each** literal listed in **Files** (except `context.rs::sample()`, already done, and `build_context.rs`, handled next), add these three fields (place them after the existing `interfaces: …` / `window: …` line):
```rust
            battery: None,
            os: String::new(),
            arch: String::new(),
```
For `crates/rustline-wasm/tests/e2e.rs`'s `ctx_now`, add the same three fields to its `Context { … }` literal.

In `crates/rustline/src/build_context.rs`, inside `build_region_context`'s returned `Context { … }`, after `interfaces: read_interfaces(),` add:
```rust
        battery: None, // Task 2 replaces this with crate::battery::read_battery()
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
```

- [ ] **Step 5: Run tests + clippy to verify green**

Run: `cargo test -p rustline-core context 2>&1 | tail -20`
Expected: PASS (both new tests).
Run: `cargo build --workspace 2>&1 | tail -5`
Expected: builds clean (all fixtures updated).
Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(core): Battery/BatteryState types + battery/os/arch on Context"
```

---

### Task 2: Platform battery read at the edge (`rustline/src/battery.rs`)

**Files:**
- Create: `crates/rustline/src/battery.rs`
- Modify: `crates/rustline/src/main.rs:1-5` (declare `mod battery;`)
- Modify: `crates/rustline/src/build_context.rs` (call `crate::battery::read_battery()`)

**Interfaces:**
- Consumes: `rustline_core::{Battery, BatteryState}` (Task 1).
- Produces: `pub fn read_battery() -> Option<Battery>`; pure parsers `parse_linux(capacity: &str, status: &str) -> Option<Battery>` and `parse_pmset(output: &str) -> Option<Battery>` (crate-private, tested).

- [ ] **Step 1: Write the failing tests** — create `crates/rustline/src/battery.rs` with the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rustline_core::BatteryState::*;

    #[test]
    fn linux_parses_percent_and_state() {
        assert_eq!(parse_linux("73\n", "Discharging\n").unwrap().percent, 73);
        assert_eq!(parse_linux("73", "Discharging").unwrap().state, Discharging);
        assert_eq!(parse_linux("55", "Charging").unwrap().state, Charging);
        assert_eq!(parse_linux("100", "Full").unwrap().state, Full);
        // "Not charging" = plugged in and topped off -> Full for display.
        assert_eq!(parse_linux("100", "Not charging").unwrap().state, Full);
        // Unknown status word.
        assert_eq!(parse_linux("40", "Weird").unwrap().state, Unknown);
    }

    #[test]
    fn linux_clamps_and_rejects_garbage() {
        assert_eq!(parse_linux("150", "Full").unwrap().percent, 100); // clamp
        assert!(parse_linux("nope", "Full").is_none()); // non-numeric -> None
    }

    #[test]
    fn pmset_parses_discharging() {
        let out = "Now drawing from 'Battery Power'\n \
            -InternalBattery-0 (id=1234567)\t73%; discharging; 3:21 remaining present: true\n";
        let b = parse_pmset(out).unwrap();
        assert_eq!(b.percent, 73);
        assert_eq!(b.state, Discharging);
    }

    #[test]
    fn pmset_parses_charging_and_charged() {
        let charging = " -InternalBattery-0 (id=1)\t46%; charging; 1:12 remaining present: true\n";
        assert_eq!(parse_pmset(charging).unwrap().percent, 46);
        assert_eq!(parse_pmset(charging).unwrap().state, Charging);

        let charged = " -InternalBattery-0 (id=1)\t100%; charged; 0:00 remaining present: true\n";
        assert_eq!(parse_pmset(charged).unwrap().state, Full);
    }

    #[test]
    fn pmset_rejects_no_battery() {
        assert!(parse_pmset("Now drawing from 'AC Power'\nNo internal battery\n").is_none());
        assert!(parse_pmset("garbage with no percent sign").is_none());
    }

    #[test]
    fn read_battery_never_panics() {
        // Host-dependent value; only assert it does not panic and is in range.
        if let Some(b) = read_battery() {
            assert!(b.percent <= 100);
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail (do not compile)**

Run: `cargo test -p rustline battery 2>&1 | tail -20`
Expected: FAIL — `parse_linux`/`parse_pmset`/`read_battery` not found.

- [ ] **Step 3: Write the module (above the test module)**

Prepend to `crates/rustline/src/battery.rs`:
```rust
//! Platform-specific battery read, isolated at the `Context`-build edge.
//!
//! `read_battery` is the only `#[cfg(target_os)]` surface; each arm delegates
//! to a pure parser (`parse_linux`/`parse_pmset`) that compiles under `test`
//! on any host, so both parsers are unit-tested on the Linux dev box even
//! though only one arm's reader compiles per platform.

use rustline_core::{Battery, BatteryState};

/// Read the host battery, or `None` if there is no battery, the platform is
/// unsupported, or the read failed. Called once at Context-build time.
pub fn read_battery() -> Option<Battery> {
    #[cfg(target_os = "linux")]
    {
        read_battery_linux()
    }
    #[cfg(target_os = "macos")]
    {
        read_battery_macos()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
fn read_battery_linux() -> Option<Battery> {
    let base = std::path::Path::new("/sys/class/power_supply");
    for entry in std::fs::read_dir(base).ok()? {
        let Ok(entry) = entry else { continue };
        let dir = entry.path();
        // Only real batteries (type == "Battery"), not Mains/AC adapters.
        let is_battery = std::fs::read_to_string(dir.join("type"))
            .map(|t| t.trim() == "Battery")
            .unwrap_or(false);
        if !is_battery {
            continue;
        }
        let (Ok(capacity), Ok(status)) = (
            std::fs::read_to_string(dir.join("capacity")),
            std::fs::read_to_string(dir.join("status")),
        ) else {
            continue;
        };
        return parse_linux(&capacity, &status);
    }
    None
}

#[cfg(target_os = "macos")]
fn read_battery_macos() -> Option<Battery> {
    let output = std::process::Command::new("pmset")
        .args(["-g", "batt"])
        .output()
        .ok()?;
    let stdout = String::from_utf8(output.stdout).ok()?;
    parse_pmset(&stdout)
}

/// Parse Linux sysfs `capacity` + `status` file contents into a `Battery`.
/// Non-numeric capacity → `None`; out-of-range capacity clamps to 100.
#[cfg(any(target_os = "linux", test))]
fn parse_linux(capacity: &str, status: &str) -> Option<Battery> {
    let percent = capacity.trim().parse::<u32>().ok()?.min(100) as u8;
    let state = match status.trim().to_ascii_lowercase().as_str() {
        "charging" => BatteryState::Charging,
        "discharging" => BatteryState::Discharging,
        // "Not charging" = plugged in, topped off; shown as Full.
        "full" | "not charging" => BatteryState::Full,
        _ => BatteryState::Unknown,
    };
    Some(Battery { percent, state })
}

/// Parse `pmset -g batt` stdout into a `Battery`. Reads the first line
/// containing a `%`, taking the digit run before `%` as the percentage and the
/// word after the first `;` as the state. No battery / no percent → `None`.
#[cfg(any(target_os = "macos", test))]
fn parse_pmset(output: &str) -> Option<Battery> {
    let line = output.lines().find(|l| l.contains('%'))?;
    let pct_end = line.find('%')?;
    let percent = line[..pct_end]
        .rsplit(|c: char| !c.is_ascii_digit())
        .next()
        .filter(|d| !d.is_empty())?
        .parse::<u32>()
        .ok()?
        .min(100) as u8;
    let state = line[pct_end + 1..]
        .split(';')
        .nth(1)
        .map(str::trim)
        .map(|s| match s.to_ascii_lowercase().as_str() {
            "charging" | "finishing charge" => BatteryState::Charging,
            "discharging" => BatteryState::Discharging,
            "charged" => BatteryState::Full,
            _ => BatteryState::Unknown,
        })
        .unwrap_or(BatteryState::Unknown);
    Some(Battery { percent, state })
}
```

Note the `#[cfg(any(target_os = "…", test))]` on the parsers: on a Linux non-test build `parse_pmset` is excluded (no `read_battery_macos` caller), so there is **no `dead_code` warning**; under `test` (or on macOS) it compiles and is exercised. This is what keeps `cargo clippy -- -D warnings` clean on the dev box.

- [ ] **Step 4: Declare the module and run tests**

In `crates/rustline/src/main.rs`, add `mod battery;` to the module list (line 1-5 block), keeping it alphabetical:
```rust
mod battery;
mod build_context;
mod cli;
mod logging;
mod plugin_cmd;
mod tmux_conf;
```

Run: `cargo test -p rustline battery 2>&1 | tail -20`
Expected: PASS (all parser tests + `read_battery_never_panics`).

- [ ] **Step 5: Wire `read_battery` into `build_context`**

In `crates/rustline/src/build_context.rs`, replace the temporary line
```rust
        battery: None, // Task 2 replaces this with crate::battery::read_battery()
```
with:
```rust
        battery: crate::battery::read_battery(),
```

Run: `cargo build -p rustline 2>&1 | tail -5`
Expected: clean.
Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: no warnings (verifies the parser `#[cfg]` gating on a non-test lib/bin build too).

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(bin): platform battery read (linux sysfs / macos pmset) at Context edge"
```

---

### Task 3: `battery` widget (`rustline-core/src/widgets/battery.rs`)

**Files:**
- Create: `crates/rustline-core/src/widgets/battery.rs`

**Interfaces:**
- Consumes: `rustline_core::{Battery, BatteryState, Context, Segment, Widget}` (Task 1).
- Produces: `pub struct BatteryWidget { pub format: String, pub down_format: String }` impl `Widget`; pure `fn battery_icon(b: &Battery) -> &'static str`.

- [ ] **Step 1: Write the failing tests** — create `crates/rustline-core/src/widgets/battery.rs`:

```rust
use crate::{Battery, BatteryState, Context, Segment, Widget};

// (implementation goes above this test module in Step 3)

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};

    fn ctx(battery: Option<Battery>) -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/".into(),
            home: "/h".into(),
            hostname: "h".into(),
            loadavg: None,
            now: Local
                .with_ymd_and_hms(2026, 7, 20, 17, 49, 0)
                .single()
                .unwrap(),
            window: None,
            interfaces: Vec::new(),
            battery,
            os: String::new(),
            arch: String::new(),
        }
    }

    fn bat(percent: u8, state: BatteryState) -> Option<Battery> {
        Some(Battery { percent, state })
    }

    fn w() -> BatteryWidget {
        BatteryWidget {
            format: "{icon} {percent}%".into(),
            down_format: String::new(),
        }
    }

    #[test]
    fn renders_icon_percent_state() {
        let widget = BatteryWidget {
            format: "{icon} {percent}% {state}".into(),
            down_format: String::new(),
        };
        let out = widget.render(&ctx(bat(73, BatteryState::Discharging)));
        assert_eq!(out[0].text, "\u{f0080} 73% discharging");
    }

    #[test]
    fn icon_buckets_by_level_and_state() {
        assert_eq!(battery_icon(&Battery { percent: 40, state: BatteryState::Charging }), "\u{f0084}");
        assert_eq!(battery_icon(&Battery { percent: 100, state: BatteryState::Full }), "\u{f0079}");
        assert_eq!(battery_icon(&Battery { percent: 95, state: BatteryState::Discharging }), "\u{f0082}");
        assert_eq!(battery_icon(&Battery { percent: 70, state: BatteryState::Discharging }), "\u{f0080}");
        assert_eq!(battery_icon(&Battery { percent: 50, state: BatteryState::Discharging }), "\u{f007e}");
        assert_eq!(battery_icon(&Battery { percent: 30, state: BatteryState::Discharging }), "\u{f007c}");
        assert_eq!(battery_icon(&Battery { percent: 10, state: BatteryState::Discharging }), "\u{f007a}");
        assert_eq!(battery_icon(&Battery { percent: 5, state: BatteryState::Discharging }), "\u{f0083}");
        assert_eq!(battery_icon(&Battery { percent: 50, state: BatteryState::Unknown }), "\u{f0091}");
    }

    #[test]
    fn none_with_empty_down_format_skips() {
        assert!(w().render(&ctx(None)).is_empty());
    }

    #[test]
    fn none_with_down_format_renders_and_collapses_placeholders() {
        let widget = BatteryWidget {
            format: "{icon} {percent}%".into(),
            down_format: "no-batt {percent}{icon}{state}".into(),
        };
        let out = widget.render(&ctx(None));
        assert_eq!(out[0].text, "no-batt ");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail (do not compile)**

Run: `cargo test -p rustline-core widgets::battery 2>&1 | tail -20`
Expected: FAIL — `BatteryWidget`/`battery_icon` not found.

- [ ] **Step 3: Write the implementation (above the test module)**

Prepend to `crates/rustline-core/src/widgets/battery.rs` (after the `use` line):
```rust
/// Renders battery percentage, charge state, and a level-bucketed,
/// charging-aware Nerd-Font icon. Pure — reads only `Context::battery`.
pub struct BatteryWidget {
    pub format: String,
    pub down_format: String,
}

/// A Nerd-Font (nf-md battery ramp) glyph for the given battery. Charging →
/// charging glyph; `Full` → full glyph; `Unknown` → unknown glyph; otherwise a
/// discharge-level bucket. Pure + unit-tested; the bucketing is the contract,
/// the exact codepoints are the nf-md battery set.
fn battery_icon(b: &Battery) -> &'static str {
    match b.state {
        BatteryState::Charging => "\u{f0084}", // md-battery-charging
        BatteryState::Full => "\u{f0079}",     // md-battery (full)
        BatteryState::Unknown => "\u{f0091}",  // md-battery-unknown
        BatteryState::Discharging => match b.percent {
            p if p >= 90 => "\u{f0082}", // md-battery-90
            p if p >= 70 => "\u{f0080}", // md-battery-70
            p if p >= 50 => "\u{f007e}", // md-battery-50
            p if p >= 30 => "\u{f007c}", // md-battery-30
            p if p >= 10 => "\u{f007a}", // md-battery-10
            _ => "\u{f0083}",            // md-battery-alert (<10%)
        },
    }
}

/// The lowercase state word substituted for `{state}`.
fn state_word(state: BatteryState) -> &'static str {
    match state {
        BatteryState::Charging => "charging",
        BatteryState::Discharging => "discharging",
        BatteryState::Full => "full",
        BatteryState::Unknown => "unknown",
    }
}

impl Widget for BatteryWidget {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        match ctx.battery {
            Some(b) => {
                let text = self
                    .format
                    .replace("{icon}", battery_icon(&b))
                    .replace("{percent}", &b.percent.to_string())
                    .replace("{state}", state_word(b.state));
                vec![Segment::new(text)]
            }
            None => {
                if self.down_format.is_empty() {
                    return vec![];
                }
                // Collapse any placeholder so a stray token never renders and
                // no fake value shows (invariant #6).
                let text = self
                    .down_format
                    .replace("{icon}", "")
                    .replace("{percent}", "")
                    .replace("{state}", "");
                vec![Segment::new(text)]
            }
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rustline-core widgets::battery 2>&1 | tail -20`
Expected: PASS (all four tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(core): battery widget (icon bucketing + format), pure over Context"
```

---

### Task 4: `[widgets.battery]` config (`rustline-core/src/config.rs`)

**Files:**
- Modify: `crates/rustline-core/src/config.rs` (add `BatteryOpts`, add to `WidgetOpts`, tests)

**Interfaces:**
- Produces: `BatteryOpts { format: String, down_format: String }`; `WidgetOpts.battery: BatteryOpts`.

- [ ] **Step 1: Write the failing tests** — append to `config.rs` `mod tests`:

```rust
    #[test]
    fn battery_opts_parse_with_defaults() {
        let toml = "[widgets.battery]\nformat = \"{percent}% {state}\"\n";
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.widgets.battery.format, "{percent}% {state}");
        assert_eq!(c.widgets.battery.down_format, ""); // omitted -> default
    }

    #[test]
    fn battery_opts_default_when_absent() {
        let c = Config::default();
        assert_eq!(c.widgets.battery.format, "{icon} {percent}%");
        assert_eq!(c.widgets.battery.down_format, "");
    }

    #[test]
    fn malformed_battery_table_falls_back_to_default() {
        let dir = std::env::temp_dir().join("rustline_test_badbattery");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        // format must be a string; an integer makes the table invalid.
        std::fs::write(&p, "[widgets.battery]\nformat = 5\n").unwrap();
        let c = Config::load(&p);
        assert_eq!(c.widgets.battery.format, "{icon} {percent}%");
        assert_eq!(c.layout.left, Config::default().layout.left);
    }
```

- [ ] **Step 2: Run tests to verify they fail (do not compile)**

Run: `cargo test -p rustline-core config::tests::battery 2>&1 | tail -20`
Expected: FAIL — no field `battery` on `WidgetOpts`.

- [ ] **Step 3: Add `BatteryOpts` and wire into `WidgetOpts`**

In `config.rs`, after `TailscaleIpOpts`'s `impl Default` block, add:
```rust
/// Default `format` for the `battery` widget.
fn default_battery_format() -> String {
    "{icon} {percent}%".into()
}

/// Options for the `battery` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BatteryOpts {
    #[serde(default = "default_battery_format")]
    pub format: String,
    #[serde(default)]
    pub down_format: String,
}

impl Default for BatteryOpts {
    fn default() -> Self {
        Self {
            format: default_battery_format(),
            down_format: String::new(),
        }
    }
}
```

In the `WidgetOpts` struct, after the `tailscale_ip` field, add:
```rust
    #[serde(default)]
    pub battery: BatteryOpts,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rustline-core config 2>&1 | tail -20`
Expected: PASS (new tests + existing config tests unaffected).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(core): [widgets.battery] config (format + down_format)"
```

---

### Task 5: Register `battery` in the builtin registry (`widgets/mod.rs`)

**Files:**
- Modify: `crates/rustline-core/src/widgets/mod.rs` (declare module, register `"battery"`, doc-count, resolves-and-renders test)

**Interfaces:**
- Consumes: `BatteryWidget` (Task 3), `BatteryOpts` via `cfg.widgets.battery` (Task 4).
- Produces: registry key `"battery"`.

- [ ] **Step 1: Write the failing test** — append to `widgets/mod.rs` `mod tests` (the `ctx` helper there is updated by Task 1 to include the new fields; add a battery-carrying variant inline):

```rust
    #[test]
    fn battery_registered_and_renders_from_context() {
        use crate::{Battery, BatteryState};
        let cfg = Config::default();
        let reg = Registry::with_builtins(&cfg);
        assert!(reg.contains("battery"));

        let mut c = ctx(Vec::new());
        c.battery = Some(Battery {
            percent: 73,
            state: BatteryState::Discharging,
        });
        let widgets = reg.resolve(&["battery".into()]);
        let texts: Vec<String> = widgets
            .iter()
            .flat_map(|w| w.render(&c))
            .map(|s| s.text)
            .collect();
        // default format "{icon} {percent}%", 73% discharging -> md-battery-70.
        assert_eq!(texts, vec!["\u{f0080} 73%".to_string()]);

        // No battery + default (empty) down_format -> widget skipped.
        let mut c0 = ctx(Vec::new());
        c0.battery = None;
        let widgets = reg.resolve(&["battery".into()]);
        let texts: Vec<String> = widgets
            .iter()
            .flat_map(|w| w.render(&c0))
            .map(|s| s.text)
            .collect();
        assert!(texts.is_empty());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustline-core widgets::tests::battery_registered 2>&1 | tail -20`
Expected: FAIL — `contains("battery")` is false (`resolve` returns empty → assert on `contains` fails).

- [ ] **Step 3: Register the widget**

In `widgets/mod.rs`, add to the module declarations (alphabetical, near `pub mod cwd;`):
```rust
pub mod battery;
```
and to the re-exports:
```rust
pub use battery::BatteryWidget;
```
In `Registry::with_builtins`, before `registry` is returned, add:
```rust
        let battery = cfg.widgets.battery.clone();
        registry.register(
            "battery",
            Box::new(move || {
                Box::new(BatteryWidget {
                    format: battery.format.clone(),
                    down_format: battery.down_format.clone(),
                })
            }),
        );
```
Update the doc comment on `with_builtins` from "all eight built-in widgets" to "all nine built-in widgets" and the "(`datetime`, `cwd`)" note is unchanged.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rustline-core 2>&1 | tail -20`
Expected: PASS (new test + all core tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(core): register battery in the builtin widget registry"
```

---

### Task 6: Host-independent smoke test + full verification

**Files:**
- Modify: `crates/rustline/tests/smoke.rs` (add a battery-in-layout smoke test)

**Interfaces:**
- Consumes: the wired binary (Tasks 1–5).

- [ ] **Step 1: Write the smoke test** — append to `crates/rustline/tests/smoke.rs`:

```rust
#[test]
fn render_right_with_battery_renders_gracefully() {
    // `battery` in a layout must render alongside built-ins and exit 0 on ANY
    // host, whether or not it actually has a battery (desktops/CI have none →
    // the widget skips via its empty down_format; laptops render the level).
    // This proves the build_context -> read_battery -> Context -> widget wiring
    // does not crash; the deterministic icon/percent formatting is pinned by
    // the widget's own unit tests (host-independent there).
    let tmp = tempfile::tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    std::fs::write(
        cfgdir.join("config.toml"),
        "[layout]\nright = [\"battery\", \"datetime\"]\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["render", "right"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("#["), "built-ins still render: {s}");
}
```

- [ ] **Step 2: Run the smoke test**

Run: `cargo test -p rustline --test smoke render_right_with_battery 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 3: Full verification sweep**

Run: `just test 2>&1 | tail -25`
Expected: all tests pass, hermetic (no wasm).
Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: no warnings.
Run: `cargo fmt --all --check 2>&1 | tail -5`
Expected: no diff.
Run: `cargo tree -i openssl 2>&1 | tail -3; cargo tree -i native-tls 2>&1 | tail -3`
Expected: both report the package is not in the dependency graph (no OpenSSL/native-tls).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "test(smoke): battery widget renders in a layout, host-independent"
```

---

### Task 7: Documentation (CLAUDE.md, README.md, TODO.md)

**Files:**
- Modify: `CLAUDE.md` (module map, widget count, Config section, invariant/roadmap note)
- Modify: `README.md` (widget list + `[widgets.battery]` config example)
- Modify: `TODO.md` (remove the completed line)

**Interfaces:** none (docs only).

- [ ] **Step 1: Update `CLAUDE.md`**

- In the `rustline-core` module map:
  - `context.rs` bullet: add `battery: Option<Battery>`, `os`, `arch` to the `Context` field list, and note the new `Battery { percent, state }` / `BatteryState` types.
  - `widgets/` bullet: change "the eight built-ins" → "the nine built-ins" and add `battery` to the list.
- In the `rustline` (bin) module map: add a `battery.rs` bullet — "`read_battery()` (the sole `#[cfg(target_os)]` surface: Linux sysfs / macOS `pmset`), delegating to pure `parse_linux`/`parse_pmset` parsers unit-tested on any host"; note `build_context.rs` now also populates `battery`, `os`, `arch`.
- In the **Config** section: add the `[widgets.battery]` table with `format` (default `"{icon} {percent}%"`) and `down_format` (default `""`), noting `battery` is opt-in (not in the default layout).
- Add a short note (near the invariants or in Roadmap "Done") recording the pattern: *platform-specific reads stay at the `Context`-build edge; each `#[cfg(target_os)]` arm delegates to a pure, `#[cfg(…, test)]`-compiled parser so it is tested on the dev box; `os`/`arch` are now on `Context` for guests.*

- [ ] **Step 2: Update `README.md`**

Add `battery` to the widget list with a one-line description, and add a `[widgets.battery]` example mirroring the IP-widget examples:
```toml
[widgets.battery]
format = "{icon} {percent}%"   # {icon}, {percent}, {state}
down_format = ""               # shown when no battery (desktops); default: nothing
```
(If `README.md` documents platform support, note battery works on Linux + macOS; other platforms render nothing.)

- [ ] **Step 3: Update `TODO.md`**

Remove the completed OS-specific-details line. If it was the only line, leave the file empty (or with a placeholder heading if the repo convention expects one).

- [ ] **Step 4: Verify docs reference reality**

Run: `grep -n "battery" CLAUDE.md README.md 2>&1 | head -20`
Expected: the new references are present in both files.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "docs: document battery widget, os/arch context, platform-read pattern"
```

---

## Task dependency / parallelism notes (for the SDD driver)

- **Task 1** must land first — it is the compile-forcing data-model change every other task builds on.
- **Tasks 2, 3, 4** depend only on Task 1 and touch disjoint files (bin `battery.rs`/`build_context.rs`; core `widgets/battery.rs`; core `config.rs`) — they may run in parallel.
- **Task 5** depends on Task 3 (`BatteryWidget`) **and** Task 4 (`BatteryOpts`).
- **Task 6** depends on Task 5 (and Task 2 for the live read).
- **Task 7** (docs) depends on all — do it last.

## Self-Review

**Spec coverage:** §3 data model → Task 1; §4 platform read + pure parsers → Task 2; §5 widget + icon bucketing → Task 3; §6 config → Task 4; §7 wiring (registry + build_context + module decl) → Tasks 2/5; §8 testing → Tasks 1–6 (each behavior has a test); §9 invariants → pinned by Task 1 (serde), Task 4 (total load), Task 3 (None branch), Task 6 (smoke, no crash); §10 docs → Task 7. No gaps.

**Placeholder scan:** every code step shows complete code; no TBD/TODO/"handle edge cases". The one intentional latitude — exact Nerd-Font codepoints — is fully pinned to concrete `\u{…}` values in Tasks 3 & 5.

**Type consistency:** `BatteryState`/`Battery` (Task 1) used verbatim in Tasks 2/3/5; `BatteryWidget { format, down_format }` (Task 3) constructed identically in Task 5's registry factory and tests; `BatteryOpts { format, down_format }` (Task 4) read as `cfg.widgets.battery` in Task 5; icon codepoints match between the widget test (Task 3), the registry test (Task 5, `\u{f0080}` for 73% discharging), and the implementation.
