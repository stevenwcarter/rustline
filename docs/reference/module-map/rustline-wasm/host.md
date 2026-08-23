# `rustline-wasm` — host functions, registration, integrity

> **Read when:** adding or changing a host function, plugin instantiation, manifests, denial recording, checksum verification, or plugin discovery/registration.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- `perform.rs` — the eight capability-checked effect functions
  (`perform_http_get`, `perform_http_get_cached` — the TTL-cached GET:
  gate-first, 2xx-only caching, serve-stale, and now backing off to at most
  one refresh attempt per TTL window on a failure (see `cache.rs` above) —
  `perform_state_read/write`, `perform_file_read/write`, and
  `perform_exec`/`perform_exec_cached`); pure enough to unit-test directly,
  incl. the denied-case tests. Each of the eight delegates to a private
  outcome enum (`HttpOutcome`/`CachedHttpOutcome`/`ExecOutcome`/
  `CachedExecOutcome`/`ReadOutcome`/`WriteOutcome`, T4) with exactly one
  `From` impl converting it to its wire struct — the single author of every
  ok/error/stale flag combination, so a caller can't hand-assemble an
  inconsistent wire result; the wire JSON itself is unchanged, pinned by a
  `wire_pins` test module. `perform_file_read` gates on `allowed_paths`
  and `perform_file_write` on the separate `allowed_write_paths` — an entry in
  one never authorizes the other — both resolved through
  `state::resolve_for_allowlist` first (see `state.rs` above); `perform_exec`
  gates on `canonical_argv(program, args)` — the **whole** command line, not
  just `program` — against `allowed_commands` before ever touching `runner`
  (gate-first, invariant N1); a non-zero exit is `ok: true` (the process ran;
  its own exit status is data, not a host-level error). `perform_exec_cached`
  mirrors `perform_http_get_cached`'s shape with one deliberate difference:
  only a **zero-exit** run is written to the cache, and a **non-zero exit is
  returned as fresh data** rather than triggering a stale-serve fallback (it
  genuinely ran and reported that status) — only a run that couldn't happen
  at all (denied, spawn failure, timeout) falls back to the last-good cached
  entry with `stale: true`. Plus `perform_log(plugin, level, msg)` (W7): the
  one intentional **capability-free** host function — it only ever writes to
  the host's `tracing` subscriber, so unlike the eight above it has no
  `CapabilityCtx` allowlist to check and no denied-case test; an unrecognized
  `level` string degrades to `info` (keeping the original as a field) rather
  than dropping the message or panicking. Capability-free does not mean
  unbounded, though: a guest message is truncated to `MAX_GUEST_LOG_BYTES` (2
  KiB, the truncation marker budgeted inside that cap, not added on top), and
  calls are capped at `MAX_GUEST_LOG_CALLS` (256) per plugin instance per
  process — neither is a capability gate (nothing is denied, no
  `observe_denial` fires), just a size/rate bound so a buggy or hostile guest
  can't fill the user's disk. "Per process" means per render on the CLI path,
  but per **daemon lifetime** once a plugin instance is warm there (see
  `capability.rs`'s `log_calls` above), so a well-behaved plugin logging once
  a render goes quiet after `MAX_GUEST_LOG_CALLS` renders until the daemon
  restarts or reloads — a one-shot `warn!` at the boundary leaves a breadcrumb
  explaining the silence.
- `host.rs` — the `host_fn!` wrappers binding `perform_*` (incl.
  `rl_http_get_cached`, `rl_exec`/`rl_exec_cached`, and, W7, `rl_log`) to each
  plugin's `CapabilityCtx`, `build_plugin` (Extism instantiation: wasi off,
  fuel + timeout + memory caps, all **nine** host functions bound), `build_plugin_with_cache` +
  `CompileCache { Enabled, Disabled }` (W43): `build_plugin` now points
  wasmtime at an on-disk compile cache under the state root via
  `PluginBuilder::with_cache_config` (from `wasmtime_cache_config_path`) so a
  later cold spawn deserializes an unchanged plugin's precompiled artifact
  instead of re-running Cranelift — best-effort (no config producible → no
  cache, never a failed build or dropped plugin, N2; guest authority N1–N4
  untouched); `CompileCache::Disabled` (`with_cache_disabled`) is the bench's
  A/B toggle that forces a full compile. And `WasmWidget` (wraps an
  `extism::Plugin`; `Widget::render` degrades to empty segments on any
  error/timeout/malformed output; carries its own `name: WidgetName` and
  implements `range_name` via `RangeName::parse` (rejecting a
  charset-violating `.wasm` stem too, not just an over-length one — the same
  forgery hole `RangeName` closes elsewhere, see `range_name.rs` above) —
  the guest itself decides whether to honor `context.toggled`).
- `manifest.rs` — plugin capability *manifests* (W24): `PluginManifest
  { name, version, requested_urls, requested_paths, requested_write_paths,
  requested_commands }` (D3 added `requested_write_paths`, parsed separately
  from the read-only `requested_paths`) and `resolve_manifest(plugin_dir,
  name) -> Option<PluginManifest>`, which
  resolves a sidecar `<plugin_dir>/<name>.toml` first (primary; supersedes
  unconditionally, even if malformed) or else an embedded `rustline-manifest`
  wasm custom section (fallback, via a hand-rolled `find_custom_section`
  reader — no wasm-parsing dependency), else `None`. A manifest never grants
  anything itself — it's just a declaration `rustline plugin approve` turns
  into an allowlist write; a malformed manifest from either source is logged
  and treated as absent, never breaking discovery (N2).
- `denials.rs` — `FileDenialObserver` (W28), the production `DenialObserver`
  wired as the default in `register_plugins`: it dedupes `(plugin, kind,
  target)` and, for each newly-seen triple, both appends a JSON line to
  `<data_root>/denials.jsonl` (best-effort — a write failure `warn!`s, never
  panics, per N2) **and** logs a `tracing::warn!` ("capability denied; see
  `rustline plugin denials <plugin>`") — the JSONL file is no longer the only
  place a denial surfaces, so one shows up in the log the first time it
  happens instead of only on a manual `plugin denials` lookup; the log line
  reuses the same dedup as the record, so a repeated denial still logs once.
  `denials_path()`/`read_denials_at`/`read_denials` back `rustline plugin
  denials <name>`. NOTE: the record has no quota/rotation yet — a guest that
  varies its `target` defeats the dedup and grows the file unbounded (a
  follow-up; see WHATS-NEXT).
- `integrity.rs` — plugin binary integrity (W19): `sha256_hex(bytes: &[u8]) ->
  String`, the single definition of the digest both `plugin_install.rs` (at
  install time) and `register_plugins` (at load time) now share — the prior
  duplicate copy in `plugin_install.rs` is gone, so the two can never drift
  apart. `ChecksumVerdict { NotRecorded, Match, Mismatch { expected, actual },
  Malformed { recorded } }` and `verify_checksum(recorded: Option<&str>,
  bytes: &[u8]) -> ChecksumVerdict` accept 64 hex chars (case-insensitive), an
  optional `sha256:` prefix, and surrounding whitespace; an absent or blank
  `recorded` is `NotRecorded` (nothing to verify, loads as before), not a
  refusal. `register_plugins` — the real gate the bar and the daemon go
  through — matches the four `ChecksumVerdict` variants
  directly rather than calling `allows_load()`, so it can log a distinct
  `warn!` message per rejection reason: `NotRecorded`/`Match` permit
  registration; `Mismatch`/`Malformed` skip the plugin — a value this build
  cannot even parse as a digest **fails closed** (treated as a rejection, the
  same as a proven mismatch, never as "effectively unpinned").
  `ChecksumVerdict::allows_load()` is a convenience predicate with a single
  caller, `instantiate_named` (the `plugin run` dev harness): it uses the
  verdict only to decide whether to `warn!` before loading anyway (a harness
  iterating on a plugin you just rebuilt expects a stale digest), never to
  refuse — the harness always instantiates regardless of the verdict. Pure
  and I/O-free, so every rejection path is unit-tested without touching a
  real `.wasm`. Re-exported at the crate root as
  `rustline_wasm::{sha256_hex, verify_checksum, ChecksumVerdict}`.
- `lib.rs::{abi_decision, register_plugins, instantiate_named,
  discover_plugin_names}` — `pub fn discover_plugin_names(plugin_dir: &Path)
  -> Vec<String>` is the discovery half of `register_plugins` split out on its
  own: the sorted `.wasm` stems present in `plugin_dir`, **without reading or
  instantiating any of them** — a missing/unreadable dir yields an empty
  `Vec`, never an error. This is what lets `rustline widget list`/`widget edit`
  show plugin widgets in the AVAILABLE column without paying wasm cold-start
  or running guest code just to render a picker.
  `abi_decision(host: u32, guest: Option<u32>) -> AbiDecision`
  (`{Register, RegisterLegacy, Skip}`, W32) is the pure ABI-version handshake:
  a guest declaring the host's version registers; a guest with no
  `abi_version` export registers as legacy (existing plugins keep working); a
  mismatched version is **skipped, never registered**. `register_plugins`
  discovers `*.wasm` in the plugin dir, and for
  each name in the caller's `needed` list (i.e. actually referenced by a
  layout region — avoids paying wasm cold-start for unused plugins): skips a
  stem colliding with a built-in (built-in wins) or an unreadable file, then
  — right after reading the file's bytes and before building a
  `CapabilityCtx` or handing anything to wasmtime — verifies those bytes
  against `[plugins.<name>].checksum` (W19, `integrity::verify_checksum`): a
  `Mismatch` or `Malformed` verdict skips with a `warn!`, exactly like every
  other skip site here (a rejected plugin never reaches `build_plugin`,
  invariant N1); `NotRecorded`/`Match` proceed. Only then does it instantiate
  the plugin, run `abi_decision`, and verify the exported `name()` equals the
  filename stem (mismatch → `warn!` + skip), before registering a
  `WasmWidget` factory (each instance getting its own `FileDenialObserver`).
  A stem longer than 15 bytes gets a one-time `warn!` (not click-toggleable)
  but still registers.
  `instantiate_named(plugin_dir, name, &PluginConfig, observer)` builds a
  single named plugin (reusing `build_plugin` + `with_observer`) for the
  read-only `rustline plugin run` dev harness, capturing denials via a
  `CollectingObserver`. It deliberately runs the checksum gate **warn-only**:
  a bad verdict is logged but the plugin still instantiates and runs, because
  this harness is used right after rebuilding a plugin, where a recorded
  digest legitimately mismatches on every build. The real gate — the bar and
  the daemon — always goes through `register_plugins` above, which does not
  bypass it.

