# Architecture

> **Read when:** orienting in the codebase for the first time, adding a crate, or deciding which crate a new piece of behavior belongs in.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

Cargo **workspace**, edition 2024, `resolver = "2"`:

- `crates/rustline-core` — the **pure, front-end-agnostic core**. Given a
  `Context` snapshot it produces a tmux-format `String`. No I/O, no environment
  reads at render time. This is deliberately the reuse seam: today's only
  front-end is the CLI, but a future daemon would build `Context`s from a
  different source and call the same core unchanged — the optional W48
  render daemon (`crates/rustline/src/daemon.rs`) is a first cut at this: it
  still lives inside the `rustline` bin and still builds `Context`s the same
  way the CLI does per request, just keeping the config/theme/registry warm
  between requests rather than rebuilding them cold each time; a more
  ambitious future daemon (push-driven Context construction, sub-second
  widgets) remains open. Re-exports the
  `rustline-abi` types (`Segment`, `Style`, `Color`) so `rustline_core::Segment`
  etc. keep working unchanged.
- `crates/rustline-abi` — a small, **serde-only** crate holding the
  WASM-boundary output types (`Segment`, `Style`, `Color`). No I/O, no chrono.
  Split out of `rustline-core` so a wasm guest plugin can depend on just the
  wire types without pulling in core's heavier deps (config parsing, the
  render pipeline, etc.).
- `crates/rustline` — the **tmux front-end binary**. A `clap` CLI that builds a
  `Context` from CLI args + local system reads, discovers/registers WASM
  plugins, calls the core, and prints.
- `crates/rustline-wasm` — the **WASM plugin host**: an Extism (wasmtime)
  runtime with **nine** host functions — **eight** capability-gated
  (TTL-cached + raw network + state + arbitrary-file read/write + **command
  exec, cached and uncached**) plus one capability-free guest logger,
  `rl_log` — per-plugin allowlists and a sandboxed/quota-bounded state
  dir, and discovery of `*.wasm` files into `Widget` registrations. Zero
  ambient authority — guests run with wasi off and no built-in Extism
  HTTP/FS; every effect is host-checked (`rl_log` is the sole intentional
  exception — see invariant N1). Reusable verbatim by a future daemon
  front-end — the W48 render daemon already does this, building its warm
  `Registry` via the same `register_plugins`.
- `crates/rustline-plugin-sdk` — the **guest-side SDK** a WASM plugin depends
  on (W39): one crate bundling typed host-capability wrappers (`http_get`,
  `http_get_cached`, `state_read`/`state_write`, `file_read`/`file_write`,
  `log`), re-exports of the shared wire types (`GuestRender`, `WireContext`,
  `Segment`/`Style`/`Color` from `rustline-abi`), the `active_format` toggle
  helper, and the `export_plugin!` macro (wires the `name`/`render`/
  `abi_version` Extism exports in one line). Links the real Extism host imports
  **only on `wasm32`**; on the host target the wrappers degrade to
  `HostError::Unavailable` so a plugin's pure logic still compiles and
  unit-tests under `cargo test`. All five example plugins depend on it.

`plugins/` holds example/third-party plugin sources, each an **excluded**
workspace member (own `Cargo.lock`, built for `wasm32-unknown-unknown`):

- `plugins/weather` — the worked example: a Nerd-Font condition icon + °F for a
  configured zip code from wttr.in, fetched via the host's TTL-cached GET
  (`rl_http_get_cached`) so it hits the network at most once per `refresh_secs`
  (the host owns the cache; the guest no longer manages its own state dir).
- `plugins/counter` — a state-backed counter: reads its previous count via
  `rl_state_read`, increments it, writes it back via `rl_state_write`, and
  renders it, demonstrating the sandboxed-state capability (no network at
  all) plus `rl_log` on a failed write.
- `plugins/filewatch` — reads a configured file via `rl_file_read` (gated by
  the user's `allowed_paths`) and renders a first-line/line-count summary,
  demonstrating the arbitrary-file-read capability; a denied/missing file
  logs why via `rl_log` and falls back to `down_format`.
- `plugins/httpget` — a plain (uncached) `rl_http_get` widget that fetches a
  configured URL and renders a snippet of the body, contrasting with
  `weather`'s TTL-cached path; a non-2xx status or transport error logs why
  via `rl_log` and falls back to `down_format`.
- `plugins/cmdrun` — runs a configured `program` + `args` via the host's
  `rl_exec`/`rl_exec_cached` and renders a snippet of stdout, demonstrating
  the capability-gated exec capability; a denial, timeout, or non-zero exit
  logs why via `rl_log` and falls back to `down_format`.

### Render pipeline

`Context` → each widget's `render(&Context) -> Vec<Segment>` → `assign_palette`
fills segment backgrounds → `render_region` joins segments with powerline
separators into tmux `#[fg=..,bg=..]` markup. For the window list, tmux calls
`render window` once per window and each window is rendered as a self-contained
**rounded pill** (`render_window_pill`, not `render_region`/`assign_palette`):
rounded caps (`` / ``) colored `fg=pill,bg=bar_bg`, the active window in the
accent fill + bold and inactive windows in a gray fill — all six colors/glyphs
themeable via `[theme]` (see Config). WASM plugins implement the same `Widget` trait as
built-ins (via `WasmWidget`) and are resolved into the same registry, so they
flow through this pipeline unchanged.

A widget that opts into click-to-toggle (a non-empty `alt_format` and a name
that fits tmux's 15-byte range limit) reports `Widget::range_name() ->
Some(name)`; `render_named_region` then calls `render_region_ranged` instead of
`render_region`, wrapping that widget's cells in `#[range=user|NAME]…#[norange]`
so a tmux `MouseDown1Status` binding can tell which widget was clicked (see CLI
below: `rustline click`). With every widget's range `None`, output is
byte-identical to `render_region`.

The core types (`Context`, `WindowCtx`) live in `rustline-core` (they carry
`chrono`); the output types (`Segment`, `Style`, `Color`) live in
`rustline-abi` and are re-exported by `rustline-core`. All derive
`serde::Serialize + Deserialize` **on purpose** — that is the WASM plugin ABI.
A plugin's `render` crosses the Extism boundary as a JSON string (WebAssembly
can only pass scalars + linear memory); the JSON is just the serde encoding of
these shared types, not a design shortcut. Keep them serializable.

