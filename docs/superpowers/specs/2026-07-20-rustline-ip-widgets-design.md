# rustline IP-address widgets (`lan_ip` + `tailscale_ip`) — design

**Status:** approved (brainstorm, 2026-07-20)
**Depends on:** the v1 core (`docs/superpowers/specs/2026-07-20-rustline-tmux-statusline-design.md`)
**Scope:** Add two built-in widgets that show the machine's LAN IPv4 and its
Tailscale IPv4, sourced from a host-side network-interface read and rendered
by pure, config-driven selection logic.

## 1. Purpose & success criteria

rustline should be able to show the local machine's addresses on the status
line: the **LAN** IPv4 (the address other machines on the local network reach)
and the **Tailscale** IPv4 (the `100.64.0.0/10` CGNAT address the tailnet
assigns). Both come from enumerating the host's network interfaces — a local
system read in exactly the same class as the existing `loadavg`/`hostname`
reads — so they are **built-in widgets**, not WASM plugins. (The plugin sandbox
has no interface-enumeration capability by design; adding one would leak network
topology into the sandbox for what built-ins already do trivially.)

**Success when:**

1. Adding `"lan_ip"` to a layout region renders the machine's LAN IPv4 (e.g.
   `192.168.1.20`), auto-selected across a multi-interface dev box (Docker /
   libvirt bridges do not masquerade as the LAN).
2. Adding `"tailscale_ip"` renders the tailnet IPv4 (`100.x.y.z`) when Tailscale
   is up.
3. Each widget's `format` option lets the user prefix a label and/or a Nerd-Font
   glyph around an `{ip}` placeholder (e.g. `format = "TS {ip}"`), printed
   verbatim.
4. When an address is unavailable (no qualifying interface / Tailscale down),
   the widget renders `down_format`; its default `""` means render **nothing**
   and skip the widget (the invariant-#6 default).
5. `interface = "<name>"` on `lan_ip` forces a specific NIC, overriding the
   auto-pick heuristic.
6. Zero-config still works (neither widget is in the default layout — opt in by
   naming it); a bad/partial `[widgets.lan_ip]`/`[widgets.tailscale_ip]` table
   never breaks the bar (invariant #3); the `if-addrs` dep keeps the graph
   OpenSSL/native-tls-free; `just test` stays hermetic; clippy/fmt clean.

**Non-goals (deferred):** IPv6 addresses; showing more than one LAN address at
once / an interface-priority list (single override name is enough for v1); a
combined single-`ip`-widget format string (`{lan}`/`{tailscale}` in one
segment — the two-widget shape was chosen instead); public/WAN IP via an
external lookup; Windows (`if-addrs` is cross-platform, but the heuristic's
virtual-interface name set is tuned for Linux).

## 2. Architecture overview

```
crates/
  rustline-core/     pure. Gains:
                       - context.rs:  NetIface type + Context.interfaces field
                       - widgets/lan_ip.rs, widgets/tailscale_ip.rs (pure
                         selection + format; read ONLY from Context)
                       - widgets/net.rs: shared pure predicates + pickers
                       - config.rs:   LanIpOpts / TailscaleIpOpts under WidgetOpts
                       - widgets/mod.rs: register the two names
                     NO new dependency. NO I/O.
  rustline/          bin. Gains:
                       - build_context.rs: read_interfaces() via `if-addrs`,
                         populating Context.interfaces
                       - Cargo.toml: `if-addrs` dependency
```

The **read** (calling `getifaddrs(3)` through `if-addrs`) lives in the binary's
`build_context.rs`, keeping `rustline-core` I/O-free and dependency-light. The
**selection** (which interface is "the LAN", which is Tailscale) is pure logic
in the widgets, operating on `Context.interfaces` — fully unit-testable with no
network, and reusable verbatim by a future daemon front-end that builds
`Context`s from another source.

This upholds **invariant #1** (widgets read only from `Context`, never the
environment mid-render): the interface list is captured into `Context` at build
time; the widgets never touch the OS.

## 3. Data model

New type in `rustline-core/src/context.rs`:

```rust
use std::net::Ipv4Addr;

/// One non-loopback IPv4 network interface, captured at Context-build time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetIface {
    pub name: String,
    pub ipv4: Ipv4Addr,
}
```

`Context` gains one field:

```rust
pub interfaces: Vec<NetIface>,
```

`Ipv4Addr` is serde-serializable (JSON string form, e.g. `"192.168.1.20"`), so
**invariant #2** (Context/Segment/… stay serde-serializable — the WASM ABI)
holds. The field is **additive**: existing WASM guests that hand-parse `Context`
JSON ignore an unknown key, so no guest breaks. All construction sites of
`Context` (including the test fixtures in `context.rs`, `datetime.rs`,
`loadavg.rs`, etc.) must set the new field — an empty `Vec` for fixtures that
don't exercise it.

Only **non-loopback IPv4** interfaces are captured (loopback is never shown, so
it is filtered at read time to keep `Context` lean). Interface order is whatever
`getifaddrs` returns (roughly interface-index order); the pickers below do not
depend on a stable order beyond "first qualifying wins", and the `interface`
override is the escape hatch when that guesses wrong.

## 4. Selection logic (pure — `rustline-core/src/widgets/net.rs`)

Predicates over an `Ipv4Addr` / interface name:

- `is_private(ip)` — `10.0.0.0/8` OR `172.16.0.0/12` OR `192.168.0.0/16`.
- `is_cgnat(ip)` — `100.64.0.0/10` (Tailscale's range; also RFC 6598 CGNAT).
- `is_virtual_name(name)` — `name` starts with any of `docker`, `veth`,
  `virbr`, `br-`, `tailscale` (interfaces that must not be mistaken for the LAN).

Pickers over `&[NetIface]`:

```rust
/// The LAN IPv4. An explicit `interface` override wins unconditionally
/// (even if the named NIC is virtual/public). Otherwise: the first interface
/// whose IPv4 is private, is NOT in the CGNAT range, and whose name is not a
/// known virtual interface.
fn pick_lan(ifaces: &[NetIface], interface: Option<&str>) -> Option<Ipv4Addr>;

/// The Tailscale IPv4: the first interface whose IPv4 is in 100.64.0.0/10.
fn pick_tailscale(ifaces: &[NetIface]) -> Option<Ipv4Addr>;
```

Rationale for CGNAT-range detection over interface-name matching for Tailscale:
the address range is the reliable cross-OS signal (the interface is `tailscale0`
on Linux but `utun*` on macOS); the range check needs no name allowlist.

## 5. Widgets

`rustline-core/src/widgets/lan_ip.rs`:

```rust
pub struct LanIp {
    pub format: String,          // default "{ip}"
    pub down_format: String,     // default ""
    pub interface: Option<String>,
}
```

`rustline-core/src/widgets/tailscale_ip.rs`:

```rust
pub struct TailscaleIp {
    pub format: String,          // default "{ip}"
    pub down_format: String,     // default ""
}
```

Both `render(&Context) -> Vec<Segment>`:

1. Resolve the address (`pick_lan(&ctx.interfaces, self.interface.as_deref())`
   / `pick_tailscale(&ctx.interfaces)`).
2. **Some(ip)** → one `Segment` whose text is `format` with `{ip}` replaced by
   the address string.
3. **None** → if `down_format` is empty, return `vec![]` (widget skipped —
   invariant #6); else return one `Segment` whose text is `down_format` with any
   `{ip}` replaced by the empty string (so a stray `{ip}` never renders
   literally, and no fake address is shown).

`{ip}` substitution is a plain string replace of the literal token `{ip}`. The
label/glyph is whatever else the user put in the string, printed verbatim — a
Nerd-Font glyph is just bytes we pass through.

## 6. Config

`rustline-core/src/config.rs` — two new opt structs, added to `WidgetOpts`
(every field `#[serde(default)]`, upholding **invariant #3**, total load):

```rust
pub struct LanIpOpts {
    #[serde(default = "default_ip_format")]     // "{ip}"
    pub format: String,
    #[serde(default)]                            // ""
    pub down_format: String,
    #[serde(default)]                            // None
    pub interface: Option<String>,
}

pub struct TailscaleIpOpts {
    #[serde(default = "default_ip_format")]     // "{ip}"
    pub format: String,
    #[serde(default)]                            // ""
    pub down_format: String,
}

pub struct WidgetOpts {
    // ...existing datetime, cwd...
    #[serde(default)] pub lan_ip: LanIpOpts,
    #[serde(default)] pub tailscale_ip: TailscaleIpOpts,
}
```

Example config:

```toml
[layout]
right = ["lan_ip", "tailscale_ip", "cwd", "loadavg", "datetime"]

[widgets.lan_ip]
format = "LAN {ip}"          # or a glyph: "󰈀 {ip}"
down_format = ""             # default: render nothing when no LAN IP
# interface = "wlp3s0"       # optional; omit to auto-pick

[widgets.tailscale_ip]
format = "TS {ip}"
down_format = "TS off"       # explicit down indicator
```

## 7. Wiring

- `rustline-core/src/widgets/mod.rs`: `Registry::with_builtins` registers
  `"lan_ip"` and `"tailscale_ip"`, each factory capturing its opts from `cfg`
  (mirrors how `datetime`/`cwd` capture theirs). Doc comment updates from
  "six built-in widgets" to eight.
- `rustline/src/build_context.rs`: `read_interfaces() -> Vec<NetIface>` calls
  `if_addrs::get_if_addrs()`, keeps only non-loopback IPv4 entries, maps to
  `NetIface`. On error it returns an empty `Vec` (a failed read shows no IP,
  never a fake one — same spirit as `loadavg` returning `None`). Called in
  `build_region_context`; `build_window_context` reuses it (windows won't
  normally show these widgets, but a uniform `Context` is cheap and correct).
- `rustline/Cargo.toml`: add `if-addrs` (no default features needed; it is a
  syscall wrapper with no TLS — `cargo tree -i openssl`/`-i native-tls` stay
  empty). Commit `Cargo.lock`.

## 8. Testing (TDD — load-bearing first)

Pure logic (`widgets/net.rs`):
- `is_private` / `is_cgnat` boundary cases (e.g. `172.15.x` out, `172.16.x` in,
  `172.31.x` in, `172.32.x` out; `100.63.x` out, `100.64.x` in, `100.127.x` in,
  `100.128.x` out).
- `pick_lan`: a real NIC (`eth0` 192.168.x) beats a `docker0` (172.17.x) that
  appears first; `interface` override forces a named NIC even when it is virtual;
  `None` when no interface qualifies; CGNAT-only box → `None` for LAN.
- `pick_tailscale`: finds the `100.x` interface among others; `None` when absent.

Widgets (`lan_ip.rs` / `tailscale_ip.rs`):
- `format` substitution: `{ip}` replaced; surrounding label/glyph preserved
  verbatim.
- `None` + empty `down_format` → `vec![]` (widget skipped).
- `None` + non-empty `down_format` → that text renders; a `{ip}` inside
  `down_format` collapses to empty.

Config (`config.rs`):
- Parse `[widgets.lan_ip]` / `[widgets.tailscale_ip]` with partial fields →
  defaults fill the rest.
- Total-load fallback: a malformed widget table → `Config::default`.

Cross-cutting:
- `Context` serde round-trip includes `interfaces` (invariant #2).
- Smoke test (`crates/rustline/tests/smoke.rs`): with `lan_ip`/`tailscale_ip` in
  a layout region, both names resolve in the registry and a synthesized
  `Context` (with a known `interfaces` list) renders the expected addresses;
  and with an empty `interfaces` list + default `down_format`, the region omits
  them.

The `read_interfaces()` host read itself is thin I/O (like `read_loadavg`) and
is not unit-tested for specific addresses; all decision logic it feeds is pure
and covered above.

## 9. Invariants this feature depends on / touches

- **#1 (Context is the sole render input):** upheld — interfaces are captured
  into `Context` at build time; widgets never read the OS. The load-bearing
  test is the smoke test rendering from a synthesized `Context`, proving the
  widgets need nothing but `Context`.
- **#2 (serde-serializable Context/Segment):** upheld — `NetIface`/`interfaces`
  derive serde; pinned by the round-trip test.
- **#3 (Config::load is total):** upheld — new opts are all
  `#[serde(default)]`; pinned by the total-load fallback test.
- **#6 (Option semantics, no fake data; panicking widget → empty):** the
  no-address case renders `down_format` (default empty → nothing), never a
  fabricated `0.0.0.0`; pinned by the `None`-branch widget tests.

## 10. Documentation

Update `CLAUDE.md`: module map (`widgets/net.rs`, `lan_ip.rs`,
`tailscale_ip.rs`, the `NetIface`/`interfaces` field, `if-addrs` in the bin),
the built-in widget count (six → eight), the Config section (the two new
`[widgets.*]` tables and the `interface`/`format`/`down_format` options), and
note in the rustls/dependency paragraph that `if-addrs` is TLS-free. Link this
spec from the Design-docs list.
