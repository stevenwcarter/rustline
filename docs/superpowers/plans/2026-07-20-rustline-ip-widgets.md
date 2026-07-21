# lan_ip + tailscale_ip Widgets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add two built-in widgets, `lan_ip` and `tailscale_ip`, that render the machine's LAN IPv4 and Tailscale IPv4 from a host-side interface read, with per-widget `format`/`down_format` and an optional LAN `interface` override.

**Architecture:** The binary reads network interfaces once at `Context`-build time (`if-addrs`) into a new `Context.interfaces: Vec<NetIface>` field; the two widgets live in `rustline-core` and do pure, config-driven selection + formatting over that field, reading nothing from the OS (upholds invariant #1). Selection/format logic is a pure module (`widgets/net.rs`) so it is fully unit-testable without a network.

**Tech Stack:** Rust (edition 2024), `serde`, `if-addrs` 0.15 (getifaddrs wrapper, TLS-free), `std::net::Ipv4Addr`.

## Global Constraints

- Edition 2024 in every crate; `rustfmt.toml` is edition 2024 — keep all editions equal.
- Must stay clippy-clean (`cargo clippy --all-targets -- -D warnings`) and rustfmt-clean (`cargo fmt --all --check`). No pre-commit hook — run `cargo fmt --all` before committing.
- Commit `Cargo.lock` in the same change that adds a dependency.
- `just test` must stay hermetic (no wasm toolchain). These widgets are host-only; do not touch the wasm-e2e feature.
- **Invariant #1:** widgets read only from `Context`, never the environment mid-render.
- **Invariant #2:** `Context`/`Segment`/… stay serde-serializable (the WASM ABI). New fields are additive.
- **Invariant #3:** `Config::load` is total — every config field is `#[serde(default)]`.
- **Invariant #6:** no faked data — an absent address renders `down_format` (default `""` → nothing), never `0.0.0.0`.
- rustls-only: `if-addrs` is a syscall wrapper with no TLS; `cargo tree -i openssl` / `-i native-tls` must stay empty.

---

### Task 1: `NetIface` type + `Context.interfaces` field

Adds the data model. This field is used by every `Context`, so this task updates every `Context { … }` construction site in the workspace to keep it compiling, and pins serde with a round-trip test.

**Files:**
- Modify: `crates/rustline-core/src/context.rs` (add type + field + fixture + test)
- Modify: `crates/rustline-core/src/lib.rs:14` (re-export `NetIface`)
- Modify (add `interfaces: Vec::new(),` to each `Context { … }` literal): `crates/rustline-core/src/assemble.rs:86`, `crates/rustline-core/src/widget.rs:82`, `crates/rustline-core/src/widgets/cwd.rs:63,94,103,112`, `crates/rustline-core/src/widgets/datetime.rs:29`, `crates/rustline-core/src/widgets/hostname.rs:25`, `crates/rustline-core/src/widgets/loadavg.rs:25`, `crates/rustline-core/src/widgets/pane_id.rs:24`, `crates/rustline-core/src/widgets/windows.rs:33`, `crates/rustline/src/build_context.rs:31`, `crates/rustline-wasm/tests/e2e.rs:48`

**Interfaces:**
- Produces: `pub struct NetIface { pub name: String, pub ipv4: std::net::Ipv4Addr }` (derives `Clone, Debug, PartialEq, Eq, Serialize, Deserialize`); `Context.interfaces: Vec<NetIface>`. Re-exported as `rustline_core::NetIface`.

- [ ] **Step 1: Write the failing test**

In `crates/rustline-core/src/context.rs`, extend the `sample()` fixture to include interfaces and add a round-trip test. Replace the existing `sample()` body's `window: None,` line to also set interfaces, and add the test:

```rust
// in sample(), add before the closing brace of the Context literal:
            interfaces: vec![NetIface {
                name: "eth0".into(),
                ipv4: "192.168.1.20".parse().unwrap(),
            }],
```

```rust
#[test]
fn context_interfaces_survive_serde() {
    let ctx = sample();
    let json = serde_json::to_string(&ctx).unwrap();
    let back: Context = serde_json::from_str(&json).unwrap();
    assert_eq!(back.interfaces, ctx.interfaces);
    assert_eq!(back.interfaces[0].name, "eth0");
    assert_eq!(back.interfaces[0].ipv4, "192.168.1.20".parse::<std::net::Ipv4Addr>().unwrap());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustline-core context_interfaces_survive_serde`
Expected: FAIL to compile — `NetIface` undefined and `Context` has no `interfaces` field.

- [ ] **Step 3: Write minimal implementation**

In `crates/rustline-core/src/context.rs`, add the import and type at the top (after the existing `use` lines) and the field to `Context`:

```rust
use std::net::Ipv4Addr;

/// One non-loopback IPv4 network interface, captured at `Context`-build time.
///
/// The widgets (`lan_ip`, `tailscale_ip`) select from this list rather than
/// reading the OS, keeping invariant #1 (Context is the sole render input).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetIface {
    pub name: String,
    pub ipv4: Ipv4Addr,
}
```

Add to the `Context` struct (after `pub window: Option<WindowCtx>,`):

```rust
    /// Non-loopback IPv4 interfaces read once at build time; the IP widgets
    /// select from this rather than touching the OS mid-render.
    pub interfaces: Vec<NetIface>,
```

In `crates/rustline-core/src/lib.rs`, update line 14 to re-export the type:

```rust
pub use context::{Context, NetIface, WindowCtx};
```

Add `interfaces: Vec::new(),` as the last field in every other `Context { … }` literal listed in **Files** above (each is a test fixture except `build_context.rs:31`; that real site also gets `interfaces: Vec::new(),` for now — Task 6 replaces it with the live read).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rustline-core && cargo test -p rustline && cargo build -p rustline-wasm --tests`
Expected: PASS — whole workspace compiles with the new field; the round-trip test passes.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(core): add NetIface type + Context.interfaces field"
```

---

### Task 2: Pure selection + format logic (`widgets/net.rs`)

The heuristic, the Tailscale detector, and the shared `{ip}`-substituting renderer — all pure, no I/O.

**Files:**
- Create: `crates/rustline-core/src/widgets/net.rs`
- Modify: `crates/rustline-core/src/widgets/mod.rs` (add `mod net;`)

**Interfaces:**
- Consumes: `NetIface` (Task 1), `Segment` (`crate::Segment`).
- Produces (all `pub(crate)`): `is_cgnat(Ipv4Addr) -> bool`, `is_virtual_name(&str) -> bool`, `pick_lan(&[NetIface], Option<&str>) -> Option<Ipv4Addr>`, `pick_tailscale(&[NetIface]) -> Option<Ipv4Addr>`, `render_ip(&str, Option<Ipv4Addr>, &str) -> Vec<Segment>`.

- [ ] **Step 1: Write the failing tests**

Create `crates/rustline-core/src/widgets/net.rs` with a `tests` module first (put the `use`/impl stubs in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::NetIface;

    fn ifc(name: &str, ip: &str) -> NetIface {
        NetIface { name: name.into(), ipv4: ip.parse().unwrap() }
    }

    #[test]
    fn cgnat_range_boundaries() {
        assert!(!is_cgnat("100.63.255.255".parse().unwrap()));
        assert!(is_cgnat("100.64.0.0".parse().unwrap()));
        assert!(is_cgnat("100.101.4.7".parse().unwrap()));
        assert!(is_cgnat("100.127.255.255".parse().unwrap()));
        assert!(!is_cgnat("100.128.0.0".parse().unwrap()));
        assert!(!is_cgnat("192.168.1.1".parse().unwrap()));
    }

    #[test]
    fn virtual_names_detected() {
        for n in ["docker0", "veth123", "virbr0", "br-abc", "tailscale0"] {
            assert!(is_virtual_name(n), "{n} should be virtual");
        }
        for n in ["eth0", "enp3s0", "wlp2s0", "wlan0"] {
            assert!(!is_virtual_name(n), "{n} should be real");
        }
    }

    #[test]
    fn lan_skips_docker_and_tailscale_picks_real_nic() {
        let ifaces = [
            ifc("docker0", "172.17.0.1"),
            ifc("tailscale0", "100.101.4.7"),
            ifc("eth0", "192.168.1.20"),
        ];
        assert_eq!(pick_lan(&ifaces, None), Some("192.168.1.20".parse().unwrap()));
    }

    #[test]
    fn lan_override_forces_named_nic_even_if_virtual() {
        let ifaces = [ifc("eth0", "192.168.1.20"), ifc("docker0", "172.17.0.1")];
        assert_eq!(pick_lan(&ifaces, Some("docker0")), Some("172.17.0.1".parse().unwrap()));
        // override naming a NIC that isn't present -> None
        assert_eq!(pick_lan(&ifaces, Some("wlp9s0")), None);
    }

    #[test]
    fn lan_none_when_only_public_or_cgnat() {
        let ifaces = [ifc("eth0", "100.101.4.7"), ifc("eth1", "8.8.8.8")];
        assert_eq!(pick_lan(&ifaces, None), None);
    }

    #[test]
    fn tailscale_finds_cgnat_else_none() {
        let with = [ifc("eth0", "192.168.1.20"), ifc("tailscale0", "100.101.4.7")];
        assert_eq!(pick_tailscale(&with), Some("100.101.4.7".parse().unwrap()));
        let without = [ifc("eth0", "192.168.1.20")];
        assert_eq!(pick_tailscale(&without), None);
    }

    #[test]
    fn render_substitutes_ip_and_preserves_label() {
        let out = render_ip("LAN {ip}", Some("192.168.1.20".parse().unwrap()), "");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "LAN 192.168.1.20");
    }

    #[test]
    fn render_none_empty_down_format_is_skipped() {
        assert!(render_ip("{ip}", None, "").is_empty());
    }

    #[test]
    fn render_none_with_down_format_shows_it_and_collapses_ip_token() {
        let out = render_ip("TS {ip}", None, "TS off {ip}");
        assert_eq!(out[0].text, "TS off ");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rustline-core --lib widgets::net`
Expected: FAIL to compile — the functions don't exist yet.

- [ ] **Step 3: Write minimal implementation**

Prepend this to `crates/rustline-core/src/widgets/net.rs` (above the `tests` module):

```rust
//! Pure selection + formatting logic for the IP widgets: which interface is
//! "the LAN", which is Tailscale, and how an address (or its absence) renders.
//! No I/O — operates entirely on the `Context.interfaces` snapshot.

use std::net::Ipv4Addr;

use crate::{NetIface, Segment};

/// True for Tailscale's `100.64.0.0/10` CGNAT range (RFC 6598). Used both to
/// detect the tailnet address and — implicitly, via `is_private` excluding it —
/// to keep it out of the LAN pick.
pub(crate) fn is_cgnat(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (64..=127).contains(&o[1])
}

/// True for interface names that carry private IPs but are never "the LAN":
/// container/VM bridges and the tailnet interface.
pub(crate) fn is_virtual_name(name: &str) -> bool {
    const VIRTUAL_PREFIXES: [&str; 5] = ["docker", "veth", "virbr", "br-", "tailscale"];
    VIRTUAL_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// The LAN IPv4. An explicit `interface` override wins unconditionally (even a
/// virtual/public NIC). Otherwise the first RFC1918-private address on a
/// non-virtual interface. `is_private` (10/8, 172.16/12, 192.168/16) already
/// excludes the 100.64/10 CGNAT range, so a tailnet address is never the LAN.
pub(crate) fn pick_lan(ifaces: &[NetIface], interface: Option<&str>) -> Option<Ipv4Addr> {
    if let Some(name) = interface {
        return ifaces.iter().find(|i| i.name == name).map(|i| i.ipv4);
    }
    ifaces
        .iter()
        .find(|i| i.ipv4.is_private() && !is_virtual_name(&i.name))
        .map(|i| i.ipv4)
}

/// The Tailscale IPv4: the first interface whose address is in the CGNAT range.
pub(crate) fn pick_tailscale(ifaces: &[NetIface]) -> Option<Ipv4Addr> {
    ifaces.iter().find(|i| is_cgnat(i.ipv4)).map(|i| i.ipv4)
}

/// Render an address (or its absence) to segments. `Some` → `format` with
/// `{ip}` substituted. `None` → nothing when `down_format` is empty (invariant
/// #6), else `down_format` with any `{ip}` collapsed to empty (never a fake IP).
pub(crate) fn render_ip(format: &str, ip: Option<Ipv4Addr>, down_format: &str) -> Vec<Segment> {
    match ip {
        Some(ip) => vec![Segment::new(format.replace("{ip}", &ip.to_string()))],
        None if down_format.is_empty() => vec![],
        None => vec![Segment::new(down_format.replace("{ip}", ""))],
    }
}
```

Add `mod net;` to `crates/rustline-core/src/widgets/mod.rs` (with the other `pub mod` lines; `net` is crate-internal so plain `mod net;` is fine).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rustline-core --lib widgets::net`
Expected: PASS (all net tests green).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(core): pure LAN/Tailscale selection + IP render logic"
```

---

### Task 3: `LanIp` + `TailscaleIp` widgets

Thin `Widget` impls delegating to Task 2's pure helpers.

**Files:**
- Create: `crates/rustline-core/src/widgets/lan_ip.rs`
- Create: `crates/rustline-core/src/widgets/tailscale_ip.rs`
- Modify: `crates/rustline-core/src/widgets/mod.rs` (declare + re-export both)

**Interfaces:**
- Consumes: `net::{pick_lan, pick_tailscale, render_ip}` (Task 2), `Context`/`Segment`/`Widget`.
- Produces: `pub struct LanIp { pub format: String, pub down_format: String, pub interface: Option<String> }`; `pub struct TailscaleIp { pub format: String, pub down_format: String }`; both `impl Widget`.

- [ ] **Step 1: Write the failing tests**

Create `crates/rustline-core/src/widgets/lan_ip.rs`:

```rust
use crate::widgets::net;
use crate::{Context, Segment, Widget};

/// Renders the machine's LAN IPv4, selected from `Context.interfaces`.
pub struct LanIp {
    pub format: String,
    pub down_format: String,
    pub interface: Option<String>,
}

impl Widget for LanIp {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let ip = net::pick_lan(&ctx.interfaces, self.interface.as_deref());
        net::render_ip(&self.format, ip, &self.down_format)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NetIface;
    use chrono::{Local, TimeZone};

    fn ctx(ifaces: Vec<NetIface>) -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/".into(),
            home: "/h".into(),
            hostname: "h".into(),
            loadavg: None,
            now: Local.with_ymd_and_hms(2026, 7, 20, 17, 49, 0).single().unwrap(),
            window: None,
            interfaces: ifaces,
        }
    }

    fn ifc(name: &str, ip: &str) -> NetIface {
        NetIface { name: name.into(), ipv4: ip.parse().unwrap() }
    }

    #[test]
    fn renders_lan_ip_with_label() {
        let w = LanIp { format: "LAN {ip}".into(), down_format: String::new(), interface: None };
        let out = w.render(&ctx(vec![ifc("eth0", "192.168.1.20")]));
        assert_eq!(out[0].text, "LAN 192.168.1.20");
    }

    #[test]
    fn no_lan_ip_and_empty_down_format_renders_nothing() {
        let w = LanIp { format: "{ip}".into(), down_format: String::new(), interface: None };
        assert!(w.render(&ctx(vec![])).is_empty());
    }

    #[test]
    fn no_lan_ip_with_down_format_renders_it() {
        let w = LanIp { format: "{ip}".into(), down_format: "no-lan".into(), interface: None };
        assert_eq!(w.render(&ctx(vec![]))[0].text, "no-lan");
    }

    #[test]
    fn interface_override_honored() {
        let w = LanIp { format: "{ip}".into(), down_format: String::new(), interface: Some("docker0".into()) };
        let out = w.render(&ctx(vec![ifc("eth0", "192.168.1.20"), ifc("docker0", "172.17.0.1")]));
        assert_eq!(out[0].text, "172.17.0.1");
    }
}
```

Create `crates/rustline-core/src/widgets/tailscale_ip.rs`:

```rust
use crate::widgets::net;
use crate::{Context, Segment, Widget};

/// Renders the machine's Tailscale IPv4 (the `100.64.0.0/10` address).
pub struct TailscaleIp {
    pub format: String,
    pub down_format: String,
}

impl Widget for TailscaleIp {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let ip = net::pick_tailscale(&ctx.interfaces);
        net::render_ip(&self.format, ip, &self.down_format)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NetIface;
    use chrono::{Local, TimeZone};

    fn ctx(ifaces: Vec<NetIface>) -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/".into(),
            home: "/h".into(),
            hostname: "h".into(),
            loadavg: None,
            now: Local.with_ymd_and_hms(2026, 7, 20, 17, 49, 0).single().unwrap(),
            window: None,
            interfaces: ifaces,
        }
    }

    fn ifc(name: &str, ip: &str) -> NetIface {
        NetIface { name: name.into(), ipv4: ip.parse().unwrap() }
    }

    #[test]
    fn renders_tailscale_ip() {
        let w = TailscaleIp { format: "TS {ip}".into(), down_format: "TS off".into() };
        let out = w.render(&ctx(vec![ifc("tailscale0", "100.101.4.7")]));
        assert_eq!(out[0].text, "TS 100.101.4.7");
    }

    #[test]
    fn down_format_when_tailscale_absent() {
        let w = TailscaleIp { format: "TS {ip}".into(), down_format: "TS off".into() };
        assert_eq!(w.render(&ctx(vec![ifc("eth0", "192.168.1.20")]))[0].text, "TS off");
    }

    #[test]
    fn empty_down_format_renders_nothing() {
        let w = TailscaleIp { format: "TS {ip}".into(), down_format: String::new() };
        assert!(w.render(&ctx(vec![ifc("eth0", "192.168.1.20")])).is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rustline-core --lib widgets::lan_ip widgets::tailscale_ip`
Expected: FAIL to compile — modules not declared in `mod.rs`.

- [ ] **Step 3: Write minimal implementation**

In `crates/rustline-core/src/widgets/mod.rs`, add the module declarations and re-exports (alongside the existing ones):

```rust
pub mod lan_ip;
pub mod tailscale_ip;
```

```rust
pub use lan_ip::LanIp;
pub use tailscale_ip::TailscaleIp;
```

(The widget bodies were written in Step 1.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rustline-core --lib widgets::lan_ip widgets::tailscale_ip`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(core): LanIp + TailscaleIp widgets"
```

---

### Task 4: Config options (`LanIpOpts` / `TailscaleIpOpts`)

Typed, fully-defaulted option tables so a config may set `format`/`down_format`/`interface`.

**Files:**
- Modify: `crates/rustline-core/src/config.rs` (add structs + `WidgetOpts` fields + tests)

**Interfaces:**
- Produces: `LanIpOpts { format: String, down_format: String, interface: Option<String> }`, `TailscaleIpOpts { format: String, down_format: String }` (both `Clone + Default + serde`); `WidgetOpts.lan_ip`, `WidgetOpts.tailscale_ip`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/rustline-core/src/config.rs`:

```rust
#[test]
fn ip_widget_opts_parse_with_defaults() {
    let toml = r#"
[widgets.lan_ip]
format = "LAN {ip}"
interface = "wlp3s0"
[widgets.tailscale_ip]
down_format = "TS off"
"#;
    let c: Config = toml::from_str(toml).unwrap();
    assert_eq!(c.widgets.lan_ip.format, "LAN {ip}");
    assert_eq!(c.widgets.lan_ip.interface.as_deref(), Some("wlp3s0"));
    // omitted -> defaults
    assert_eq!(c.widgets.lan_ip.down_format, "");
    assert_eq!(c.widgets.tailscale_ip.format, "{ip}");
    assert_eq!(c.widgets.tailscale_ip.down_format, "TS off");
}

#[test]
fn ip_widget_opts_default_when_absent() {
    let c = Config::default();
    assert_eq!(c.widgets.lan_ip.format, "{ip}");
    assert_eq!(c.widgets.lan_ip.down_format, "");
    assert_eq!(c.widgets.lan_ip.interface, None);
    assert_eq!(c.widgets.tailscale_ip.format, "{ip}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustline-core --lib config::tests::ip_widget_opts_parse_with_defaults`
Expected: FAIL to compile — `WidgetOpts` has no `lan_ip`/`tailscale_ip`.

- [ ] **Step 3: Write minimal implementation**

In `crates/rustline-core/src/config.rs`, add a shared default and the two structs (near `DateTimeOpts`/`CwdOpts`):

```rust
/// Default `format` for the IP widgets: the bare address, no label.
fn default_ip_format() -> String {
    "{ip}".into()
}

/// Options for the `lan_ip` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LanIpOpts {
    #[serde(default = "default_ip_format")]
    pub format: String,
    #[serde(default)]
    pub down_format: String,
    #[serde(default)]
    pub interface: Option<String>,
}

impl Default for LanIpOpts {
    fn default() -> Self {
        Self { format: default_ip_format(), down_format: String::new(), interface: None }
    }
}

/// Options for the `tailscale_ip` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TailscaleIpOpts {
    #[serde(default = "default_ip_format")]
    pub format: String,
    #[serde(default)]
    pub down_format: String,
}

impl Default for TailscaleIpOpts {
    fn default() -> Self {
        Self { format: default_ip_format(), down_format: String::new() }
    }
}
```

Add the two fields to `WidgetOpts` (after `cwd`):

```rust
    #[serde(default)]
    pub lan_ip: LanIpOpts,
    #[serde(default)]
    pub tailscale_ip: TailscaleIpOpts,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rustline-core --lib config`
Expected: PASS (new + existing config tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(core): lan_ip/tailscale_ip config options"
```

---

### Task 5: Register the widgets + end-to-end core render test

Wire the two names into `Registry::with_builtins`, capturing their config, and pin the whole pure path (config → registry → render from a synthesized `Context`), proving the widgets need only `Context` (invariant #1).

**Files:**
- Modify: `crates/rustline-core/src/widgets/mod.rs` (`with_builtins` registrations, doc comment, test)

**Interfaces:**
- Consumes: `LanIp`/`TailscaleIp` (Task 3), `LanIpOpts`/`TailscaleIpOpts` (Task 4), `Registry`, `Config`.

- [ ] **Step 1: Write the failing test**

Add a `tests` module at the bottom of `crates/rustline-core/src/widgets/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::widget::Registry;
    use crate::{Config, Context, NetIface};
    use chrono::{Local, TimeZone};

    fn ctx(ifaces: Vec<NetIface>) -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/".into(),
            home: "/h".into(),
            hostname: "h".into(),
            loadavg: None,
            now: Local.with_ymd_and_hms(2026, 7, 20, 17, 49, 0).single().unwrap(),
            window: None,
            interfaces: ifaces,
        }
    }

    #[test]
    fn ip_widgets_registered_and_render_from_context() {
        let mut cfg = Config::default();
        cfg.widgets.lan_ip.format = "LAN {ip}".into();
        cfg.widgets.tailscale_ip.down_format = "TS off".into();
        let reg = Registry::with_builtins(&cfg);
        assert!(reg.contains("lan_ip") && reg.contains("tailscale_ip"));

        let widgets = reg.resolve(&["lan_ip".into(), "tailscale_ip".into()]);
        let c = ctx(vec![
            NetIface { name: "eth0".into(), ipv4: "192.168.1.20".parse().unwrap() },
            NetIface { name: "tailscale0".into(), ipv4: "100.101.4.7".parse().unwrap() },
        ]);
        let texts: Vec<String> =
            widgets.iter().flat_map(|w| w.render(&c)).map(|s| s.text).collect();
        assert_eq!(texts, vec!["LAN 192.168.1.20".to_string(), "100.101.4.7".to_string()]);

        // no interfaces + default lan down_format -> lan_ip skipped, tailscale shows down text
        let widgets = reg.resolve(&["lan_ip".into(), "tailscale_ip".into()]);
        let texts: Vec<String> =
            widgets.iter().flat_map(|w| w.render(&ctx(vec![]))).map(|s| s.text).collect();
        assert_eq!(texts, vec!["TS off".to_string()]);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustline-core --lib widgets::tests::ip_widgets_registered_and_render_from_context`
Expected: FAIL — `lan_ip`/`tailscale_ip` not registered (`contains` is false / `resolve` yields nothing).

- [ ] **Step 3: Write minimal implementation**

In `crates/rustline-core/src/widgets/mod.rs`, inside `with_builtins` (after the `cwd` registration, before `registry`), add:

```rust
        let lan = cfg.widgets.lan_ip.clone();
        registry.register(
            "lan_ip",
            Box::new(move || {
                Box::new(LanIp {
                    format: lan.format.clone(),
                    down_format: lan.down_format.clone(),
                    interface: lan.interface.clone(),
                })
            }),
        );

        let ts = cfg.widgets.tailscale_ip.clone();
        registry.register(
            "tailscale_ip",
            Box::new(move || {
                Box::new(TailscaleIp {
                    format: ts.format.clone(),
                    down_format: ts.down_format.clone(),
                })
            }),
        );
```

Update the `with_builtins` doc comment: change "all six built-in widgets" to "all eight built-in widgets".

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rustline-core`
Expected: PASS (full core suite).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(core): register lan_ip + tailscale_ip built-ins"
```

---

### Task 6: Host interface read + bin smoke test

Read real interfaces in the binary and feed them into `Context`; add the `if-addrs` dep and a graceful-render smoke test.

**Files:**
- Modify: `crates/rustline/Cargo.toml` (add `if-addrs`)
- Modify: `crates/rustline/src/build_context.rs` (`read_interfaces()` + wire into `build_region_context`)
- Modify: `crates/rustline/tests/smoke.rs` (smoke test)
- Modify: `Cargo.lock` (dependency lock)

**Interfaces:**
- Consumes: `NetIface` (Task 1); `if_addrs::get_if_addrs`.
- Produces: `read_interfaces() -> Vec<NetIface>` (private to the bin), invoked in `build_region_context`.

- [ ] **Step 1: Write the failing test**

Add a unit test at the bottom of `crates/rustline/src/build_context.rs`'s `tests` module:

```rust
#[test]
fn read_interfaces_excludes_loopback_and_never_panics() {
    let ifaces = read_interfaces();
    // Loopback is filtered out; whatever the host has, 127.0.0.1 must not appear.
    assert!(
        ifaces.iter().all(|i| i.ipv4 != std::net::Ipv4Addr::LOCALHOST),
        "loopback IPv4 must be filtered: {ifaces:?}"
    );
    // And build_region_context wires it in (field is populated by the same read).
    let ctx = build_region_context(&RegionArgs::default());
    assert!(ctx.interfaces.iter().all(|i| i.ipv4 != std::net::Ipv4Addr::LOCALHOST));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustline read_interfaces_excludes_loopback_and_never_panics`
Expected: FAIL to compile — `read_interfaces` undefined and `if-addrs` not a dependency.

- [ ] **Step 3: Write minimal implementation**

Add `if-addrs` to `crates/rustline/Cargo.toml` `[dependencies]`:

```toml
if-addrs = "0.15"
```

In `crates/rustline/src/build_context.rs`, add the reader (near `read_loadavg`), importing `NetIface`:

```rust
use rustline_core::{Context, NetIface, WindowCtx};
```

```rust
/// Enumerate the host's non-loopback IPv4 network interfaces.
///
/// A failed read yields an empty `Vec` (the IP widgets then render nothing /
/// their `down_format`), never a fabricated address — same spirit as
/// `read_loadavg` returning `None`.
fn read_interfaces() -> Vec<NetIface> {
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    ifaces
        .into_iter()
        .filter(|iface| !iface.is_loopback())
        .filter_map(|iface| match iface.addr {
            if_addrs::IfAddr::V4(v4) => Some(NetIface { name: iface.name, ipv4: v4.ip }),
            if_addrs::IfAddr::V6(_) => None,
        })
        .collect()
}
```

In `build_region_context`, replace the placeholder `interfaces: Vec::new(),` line (added in Task 1) with:

```rust
        interfaces: read_interfaces(),
```

- [ ] **Step 4: Run tests + verify the dependency graph**

Run: `cargo test -p rustline && cargo tree -i openssl && cargo tree -i native-tls`
Expected: tests PASS; both `cargo tree` invocations print nothing (empty — no OpenSSL/native-tls pulled in by `if-addrs`).

- [ ] **Step 5: Add the smoke test**

Add to `crates/rustline/tests/smoke.rs`:

```rust
#[test]
fn render_right_with_ip_widgets_renders_gracefully() {
    // lan_ip/tailscale_ip in a layout must render alongside built-ins and exit 0
    // on ANY host, regardless of its real LAN/Tailscale addresses. We force
    // lan_ip to a nonexistent interface so its down_format ("LANOFF") renders
    // deterministically — this positively proves the bin wires the interface
    // read -> Context -> the widget end-to-end, WITHOUT depending on whether the
    // host has (or lacks) a LAN or Tailscale IP. (A `contains("TSOFF")`-style
    // assertion would be host-dependent: any dev box actually running Tailscale
    // renders its real 100.x address instead of the down text.)
    let tmp = tempfile::tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    std::fs::write(
        cfgdir.join("config.toml"),
        "[layout]\nright = [\"lan_ip\", \"tailscale_ip\", \"datetime\"]\n\
         [widgets.lan_ip]\ninterface = \"rustline-no-such-nic0\"\ndown_format = \"LANOFF\"\n",
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_rustline"))
        .args(["render", "right"])
        .env("XDG_CONFIG_HOME", tmp.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("#["), "built-ins still render: {s}");
    // forced-nonexistent lan interface -> down_format renders deterministically,
    // proving the interface-read -> Context -> lan_ip wiring, host-independent.
    assert!(s.contains("LANOFF"), "lan_ip down_format shown: {s}");
}
```

(`tempfile` is already a `dev-dependency` of the `rustline` crate — see `crates/rustline/Cargo.toml` — so no manifest change is needed for the test. Using it here auto-cleans and avoids the fixed-`/tmp`-path collision risk the other smoke tests carry.)

- [ ] **Step 6: Run all tests + lint, then commit**

Run: `cargo test -p rustline && cargo clippy --all-targets -- -D warnings && cargo fmt --all --check`
Expected: PASS / clean.

```bash
cargo fmt --all
git add -A
git commit -m "feat(rustline): read host interfaces into Context; smoke test"
```

---

### Task 7: Documentation

Update `CLAUDE.md` to describe the new widgets, config, and dependency.

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Edit `CLAUDE.md`**

Make these edits:

1. **Module map** — under `rustline-core` › `widgets/`, change "the six built-ins: `pane_id`, `hostname`, `windows`, `cwd`, `loadavg`, `datetime`" to add `lan_ip`, `tailscale_ip`, and mention `net.rs` (pure LAN/Tailscale selection + IP formatting). Under `context.rs`, add the `interfaces: Vec<NetIface>` field and the `NetIface` type to the `Context` description.
2. **Module map** — under `rustline` (bin) › `build_context.rs`, note it now also reads non-loopback IPv4 interfaces via `if-addrs` into `Context.interfaces`.
3. **Config** — add a subsection documenting `[widgets.lan_ip]` (`format`, `down_format`, `interface`) and `[widgets.tailscale_ip]` (`format`, `down_format`), including the `{ip}` placeholder and the "empty `down_format` → render nothing" default. Note neither is in the default layout (opt-in).
4. **Development / rustls paragraph** — note `if-addrs` is a TLS-free syscall wrapper, so `cargo tree -i openssl`/`-i native-tls` stay empty.
5. **Design docs** — add: `Spec (IP widgets): docs/superpowers/specs/2026-07-20-rustline-ip-widgets-design.md` and `Plan (IP widgets): docs/superpowers/plans/2026-07-20-rustline-ip-widgets.md`.

- [ ] **Step 2: Verify the build is still green**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --all --check`
Expected: PASS / clean (docs-only change, nothing should break).

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: document lan_ip + tailscale_ip widgets and config"
```

---

## Self-Review

**Spec coverage:**
- Success #1 (LAN auto-select, skip Docker/libvirt) → Task 2 (`pick_lan` + `is_virtual_name`), Task 3, Task 6.
- Success #2 (Tailscale IPv4) → Task 2 (`pick_tailscale`), Task 3.
- Success #3 (`format` + `{ip}` + glyph verbatim) → Task 2 (`render_ip`), Task 3, Task 4.
- Success #4 (`down_format`, default `""` → nothing) → Task 2/3 tests, Task 4.
- Success #5 (`interface` override) → Task 2 (`pick_lan`), Task 3, Task 4.
- Success #6 (opt-in, total config, TLS-free, hermetic, clippy/fmt) → Task 4 (defaults), Task 6 (`cargo tree`), all tasks (fmt/clippy gates).
- Data model (`NetIface`, `Context.interfaces`, serde) → Task 1.
- Wiring (registry, bin read, dep) → Task 5, Task 6.
- Docs → Task 7.

**Placeholder scan:** No TBD/TODO; every code step has complete code and exact commands.

**Type consistency:** `NetIface { name, ipv4 }`, `Context.interfaces`, `pick_lan(&[NetIface], Option<&str>) -> Option<Ipv4Addr>`, `pick_tailscale(&[NetIface]) -> Option<Ipv4Addr>`, `render_ip(&str, Option<Ipv4Addr>, &str) -> Vec<Segment>`, `LanIp { format, down_format, interface }`, `TailscaleIp { format, down_format }`, `LanIpOpts`/`TailscaleIpOpts`, `default_ip_format()` — used consistently across Tasks 1–6.

**Invariant coverage:** #1 pinned by Task 5's synthesized-Context render; #2 by Task 1's round-trip; #3 by Task 4's default/parse tests; #6 by Task 2/3's `None`-branch tests.
