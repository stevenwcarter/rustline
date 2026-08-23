# The five example plugins

> **Read when:** adding or changing a bundled example plugin, or looking for a worked example of a given host capability.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

`plugins/weather` (excluded workspace member, `wasm32-unknown-unknown`):
- `lib.rs` — pure logic (`code_to_icon`, `render_format`, `parse_wttr`,
  `select_weather_format` — the click-toggle exemplar: prefers a non-empty
  `options.alt_format` when the guest's `render` sees its own name, `"weather"`,
  in `context.toggled`) compiled and unit-tested on the host target, plus a
  `#[cfg(target_arch = "wasm32")] mod guest` with the Extism `name`/`render`
  exports and a single `rl_http_get_cached` guest import (the host owns the
  TTL cache).

`plugins/counter`, `plugins/filewatch`, `plugins/httpget`, `plugins/cmdrun`
(excluded workspace members, `wasm32-unknown-unknown`, same shape as
`plugins/weather` — pure logic unit-tested on the host target plus a
`#[cfg(target_arch = "wasm32")] mod guest`): four more worked examples, each
covering a host capability `weather` doesn't touch, and each logging its one
failure path via `rl_log` (W7) rather than staying silent:
- `plugins/counter/lib.rs` — `parse_count`/`next_count`/`render_format`; the
  guest reads its previous count via `rl_state_read`, increments it,
  persists the new value via `rl_state_write` (a failed write is `rl_log`ged
  and otherwise ignored — the count just won't have advanced next render),
  and renders it. No network capability at all.
- `plugins/filewatch/lib.rs` — `summarize`/`render_format`; the guest reads
  a configured `path` via `rl_file_read` (gated by the plugin's
  `allowed_paths`) and renders the first line + line count. A denial,
  missing file, or malformed read result is `rl_log`ged and falls back to
  `down_format` (empty → renders nothing, same convention as the built-in
  widgets' `down_format`).
- `plugins/httpget/lib.rs` — `extract_snippet`/`render_format`; the guest
  fetches a configured `url` via the **plain, uncached** `rl_http_get` (the
  deliberate contrast with `weather`'s `rl_http_get_cached`), checks the
  response is 2xx itself (`ok` on the wire only means "the transport
  completed", not "succeeded" — unlike the cached path, nothing upstream
  filters non-2xx for it), and falls back to `down_format` (same convention),
  `rl_log`ging the failure reason.
- `plugins/cmdrun/lib.rs` — `extract_snippet`/`render_format`/`select_mode`;
  the guest runs a configured `program`/`args` via the **plain** `rl_exec`
  (`ttl_secs == 0`, the default) or the **TTL-cached** `rl_exec_cached`
  (`ttl_secs > 0`) — the exec counterpart to `httpget`'s plain-vs-cached HTTP
  contrast — and renders the first line of stdout (capped at 60 chars). A
  denial, spawn failure, timeout, or non-zero exit is `rl_log`ged and falls
  back to `down_format` (same convention). Ships a sidecar `cmdrun.toml`
  manifest requesting `["uname", "uname *"]` (program `uname`, with or
  without arguments — not a `uname*` prefix glob, which would also match a
  program named `unamex`), so `rustline plugin approve cmdrun`
  demonstrates the exec-specific approval warning end to end.

