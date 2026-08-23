# `rustline-core` — `segment.rs`, `context.rs`

> **Read when:** adding a field to `Context`, changing what the render input carries, or touching the re-exported wire types.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- `segment.rs` — `pub use rustline_abi::{Color, Segment, Style};` — a
  re-export module so existing `rustline_core::segment::…` paths keep
  resolving now that the types themselves live in `rustline-abi`.
- `context.rs` — `Context` (session/window/pane ids, `pane_current_path`,
  `home`, `hostname`, `loadavg: Option<[f64;3]>`, `now: DateTime<Local>`,
  `window: Option<WindowCtx>`, `interfaces: Vec<NetIface>`,
  `battery: Option<Battery>`, `cpu: Option<CpuUsage>`,
  `cpu_history: Vec<f32>`, `memory: Option<MemInfo>`, `mem_history: Vec<f32>`,
  `git: Option<GitInfo>`, `disk: Option<DiskInfo>`,
  `throughput: Option<Throughput>`, `uptime: Option<u64>` (seconds),
  `media: Option<MediaInfo>`,
  `disks: BTreeMap<String, DiskInfo>` (keyed by mount) and
  `throughputs: BTreeMap<String, Throughput>` (keyed by interface, `""` =
  aggregate) — the per-instance reading maps multiple `disk`/`throughput`
  widget **instances** (W46, see `config.rs` below) each consult by their own
  `mount`/`iface_key`; both `#[serde(default)]` and the only two `Context`
  fields NOT mirrored into `WireContext` (invariant #2 — `throughput`/
  `uptime`/`media`/`cpu_history`/`mem_history` ARE mirrored as of W54, see
  below) — `Context.disk`/`Context.throughput` stay populated with
  just the *base* widget's reading (the WASM mirror; a guest never sees the
  maps),
  `os: String`, `arch:
  String`, `toggled: BTreeSet<WidgetName>`, `colors: ThemeColors`), plus
  `Context::default()` (an empty, epoch-timestamped instance, so test/synthetic
  construction sites can use struct-update syntax instead of spelling out
  every field as the type grows), and `WindowCtx`. `NetIface`, `Battery`/
  `BatteryState`, `CpuUsage`, and `MemInfo` all now live in `rustline-abi`
  (chrono-free, so a WASM guest can share them directly) and are re-exported
  here — the same `Segment`/`Style`/`Color` precedent as `segment.rs` — so
  existing `rustline_core::context::…`/`rustline_core::…` paths keep
  resolving. `NetIface { name, ipv4: Ipv4Addr }` (one non-loopback IPv4 interface,
  read once at `Context`-build time; the IP widgets select from this list
  rather than touching the OS mid-render). `Battery { percent: u8, state:
  BatteryState }` and `BatteryState { Charging, Discharging, Full, Unknown }`
  (serde `snake_case`) are a battery snapshot read once at `Context`-build
  time; `CpuUsage { percent: f32 }` and `MemInfo { total_bytes, used_bytes,
  available_bytes }` (all bytes as `u64`) are the cpu/memory snapshots,
  likewise read once at `Context`-build time; `GitInfo { branch, ahead: u32,
  behind: u32, staged: u32, unstaged: u32 }` is a git branch/status snapshot,
  read once at `Context`-build time ONLY when `git` is in the active layout
  (mirroring the `cpu`/`memory` read-gating below); `DiskInfo { total_bytes,
  used_bytes, available_bytes }` (all bytes as `u64`) is a filesystem-usage
  snapshot for a configured mount, likewise read once at `Context`-build time
  ONLY when `disk` is in the active layout; `throughput` (a `Throughput`
  down/up bytes-per-sec snapshot) is read once at build time, gated the same
  way on `throughput` being in the active layout, and is `None` on the first
  invocation (a rate is a delta — nothing yet to diff against; invariant #6);
  `cpu_history`/`mem_history` (`Vec<f32>`, `#[serde(default)]`) are the recent
  cpu%/memory-used% readings (oldest first) feeding the `{spark}` placeholder,
  populated at build time from a persisted ring ONLY when the respective
  widget's `format` **or** `alt_format` references `{spark}` (W56; empty
  otherwise — no history I/O);
  `uptime` (seconds since boot) and
  `media` (a `MediaInfo` now-playing snapshot) are read once at build time and
  gated the same way on their widget being in the active layout. `throughput`/
  `uptime`/`media`/`cpu_history`/`mem_history` are all mirrored into
  `WireContext` (W54, purely additive) — a WASM guest now sees a
  field-for-field `Context` mirror minus only `disks`/`throughputs` (the
  per-instance maps above); `os`/`arch` come from
  `std::env::consts::OS`/`ARCH`; `toggled` (`#[serde(default)]`) is the set of
  widget/plugin names the user has click-toggled to their `alt_format` view,
  read once at `Context`-build time from the toggles state file (invariant #1)
  and serialized to WASM guests; `colors` (`#[serde(default)]`) is the
  resolved theme's `fg`/`bar_bg` plus its four semantic colors
  (`success`/`info`/`warning`/`error`), copied in at `Context`-build time so
  threshold-aware widgets and WASM guests can style alert badges without
  seeing `Theme` (see Themes under Config).
