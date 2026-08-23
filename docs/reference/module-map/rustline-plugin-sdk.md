# `rustline-plugin-sdk`

> **Read when:** changing the guest-side SDK: typed capability wrappers, wire re-exports, or the `export_plugin!` macro.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- `lib.rs` — the guest-side SDK (W39). Typed host-capability wrappers
  (`http_get`, `http_get_cached`, `state_read`/`state_write`,
  `file_read`/`file_write`, `exec`/`exec_cached`, `log`) that call the host
  functions and decode their JSON responses into result structs, returning
  `Result<_, HostError>` (`{Call, Decode, Unavailable}`) instead of an untyped
  `serde_json::Value`; `exec`/`exec_cached` are thin wrappers over
  `rl_exec`/`rl_exec_cached` — `exec(program, args)` encodes `args` as JSON
  for the host boundary and decodes an `ExecResult`; `exec_cached` adds
  `ttl_secs`/`now` and decodes a `CachedExecResult`, documented (matching
  `perform_exec_cached`'s own doc comment precisely) to return a non-zero
  exit as fresh, uncached data and fall back to a stale cached entry only on
  a run that couldn't happen at all (denied, spawn failure, timeout); re-exports
  of `rustline_abi::{Color, GuestRender, Segment, Style, WireContext}` and the
  host-effect result types (`HttpResult`, `CachedHttpResult`, `ReadResult`,
  `WriteResult`, `ExecResult`, `CachedExecResult`); the `active_format` toggle
  helper and `LogLevel` enum; and the
  `export_plugin!` macro, which emits the `name`/`render`/`abi_version` Extism
  exports (the last returning the real `rustline_abi::ABI_VERSION`) from one
  line. The capability wrappers link the Extism PDK **only on `wasm32`**; on the
  host target they return `HostError::Unavailable` so a plugin's pure logic
  compiles and unit-tests under `cargo test`. The four original host-effect
  wire-result types (`HttpResult`/`CachedHttpResult`/`ReadResult`/`WriteResult`)
  are re-exported from `rustline-abi` (W51) rather than re-declared here — the
  previous SDK-local copy (kept in sync only by the e2e test) is gone, removing
  the drift risk; `ExecResult`/`CachedExecResult` were added there directly,
  so the exec capability never had a duplicate-copy problem to begin with.

