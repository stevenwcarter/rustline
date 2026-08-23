# `rustline-abi`

> **Read when:** changing anything crossing the WASM boundary: `Segment`/`Style`/`Color`, `WireContext`, host-effect result types, `WidgetName`, or `ABI_VERSION`.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- `lib.rs` — `Segment { text, style }`, `Style { fg, bg, bold }`,
  `Color { Named | Indexed(u8) | Rgb(u8,u8,u8) }` (+ `Color::to_tmux()`),
  `ThemeColors { fg, bar_bg, success, info, warning, error }` with `Default`
  (matches `Theme::default()`'s values) — the semantic-color snapshot carried
  on `Context.colors` so widgets and WASM guests can style alert badges
  without seeing `Theme`; `NetIface { name, ipv4: Ipv4Addr }`; `Battery
  { percent, state: BatteryState }`/`BatteryState { Charging, Discharging,
  Full, Unknown }`; `CpuUsage { percent: f32 }`; and `MemInfo { total_bytes,
  used_bytes, available_bytes }` — all moved here from
  `rustline-core::context` (chrono-free, so a WASM guest can share them
  directly) and re-exported by `rustline-core` — and `GitInfo { branch,
  ahead, behind, staged, unstaged }` (chrono-free, so no separate wire mirror
  is needed; the same type rides `Context.git` and `WireContext.git`), plus
  `DiskInfo { total_bytes, used_bytes, available_bytes }` (same chrono-free,
  no-separate-wire-mirror shape, riding `Context.disk`/`WireContext.disk`).
  Also `WireWindowCtx` (a chrono-free mirror of `WindowCtx`) and `WireContext`
  (the guest-side, typed mirror of `Context` itself — field-for-field
  identical except `now` is a plain RFC3339 `String` rather than
  `DateTime<Local>`), plus `GuestRender { context: WireContext, config:
  serde_json::Value }` — the whole shape a guest's `render` export
  deserializes (W26), so a plugin no longer hand-walks an untyped
  `serde_json::Value` for its `Context` input; `plugin new`'s scaffold and the
  `counter`/`filewatch`/`httpget` examples all use these typed wire structs.
  Also `MediaInfo { artist, title, status }` (W41, a now-playing snapshot
  riding `Context.media`; NOT in `WireContext` — not exposed to guests, like
  `Context.uptime`), `Throughput { down_bytes_per_sec, up_bytes_per_sec }`
  (W47, a network-rate snapshot riding `Context.throughput`; also NOT in
  `WireContext`, same as `MediaInfo`), and `pub const ABI_VERSION: u32 = 1` (W32) — the host↔guest
  ABI version the host stamps onto `RenderInput.abi_version` and a guest may
  echo from its optional `abi_version()` export (see `rustline-wasm`'s
  `abi_decision`).
  Also the four host-effect wire-result types (`HttpResult`,
  `CachedHttpResult`, `ReadResult`, `WriteResult`), moved here (W51) as the
  single canonical definition a host function's JSON response decodes into —
  previously duplicated between `rustline-wasm`'s `abi.rs` and
  `rustline-plugin-sdk`, kept in sync only by the e2e test; each carries a
  struct-level `#[serde(default)]` so a guest's decode stays forward-compatible
  with a host that adds/omits a field. `rustline-wasm` and the SDK re-export
  them. Two more joined them for the exec capability: `ExecResult { ok,
  status: i32, stdout, stderr, error, truncated }` (`rl_exec`'s result — `ok`
  means "allowed and ran to completion", NOT "succeeded"; a non-zero exit is
  `ok: true` with a non-zero `status`, `status: -1` on a signal kill or
  timeout) and `CachedExecResult` (`rl_exec_cached`'s result, adding
  `stale`/`age_secs` — same "usable result present, fresh or stale"
  convention as `CachedHttpResult`). Same struct-level `#[serde(default)]`
  forward-compatibility as the other four.
  Also `WidgetName` (T6, `#[serde(transparent)]`, `Borrow<str>`) — the
  identity newtype invariant #7 keys off: `Context.toggled`/
  `WireContext.toggled: BTreeSet<WidgetName>`, `Layout`'s three region
  `Vec<WidgetName>`s, and `Config.plugins`/`Config.instances`'s
  `HashMap<WidgetName, _>` keys all use it; `Registry::resolve` returns
  `Vec<(WidgetName, Box<dyn Widget>)>`; the twelve clickable widget structs
  plus `WasmWidget` carry a `name: WidgetName` field; `rustline-core`'s
  `RangeName::of_widget` bridges a `WidgetName` into the render/click-toggle
  boundary's `RangeName`. Wire JSON and TOML shapes stay byte-identical.
  The WASM wire types, re-exported by `rustline-core`.

