# `rustline-wasm` — capability and sandbox layer

> **Read when:** touching allowlists, path/argv sandboxing, quotas, the TTL cache primitives, or the `Fetcher`/`Runner` seams.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- `allow.rs` — `AllowSet<K: CapKind>`/`Pattern`: each `allowed_urls`/
  `allowed_paths`/`allowed_write_paths` entry is a glob by default or a regex
  when prefixed `re:`; deny-by-default (empty set matches nothing); malformed
  patterns are logged and skipped. `AllowSet` is phantom-tagged per capability
  family (T3): marker types `UrlCap`/`ReadPathCap`/`WritePathCap`/`CommandCap`
  each carry a `DENIAL: DenialKind` and a typed subject (`Url<'a>`/
  `ResolvedPath`/`CanonicalArgv`), so a URL can never be checked against a
  path (or command) allowlist, or vice versa — compiler-enforced, not just a
  runtime convention. `allowed_commands` (the exec capability's allowlist)
  compiles through this same `AllowSet<CommandCap>`. `ReadPathCap`/
  `WritePathCap` additionally implement the `PathCap`-bounded `check_path`,
  which consumes a `ResolvedPath` and mints the `AllowedPath` token (see
  `state.rs` below) on a match — the only way to obtain one.
- `argv.rs` — `canonical_argv(program, args) -> CanonicalArgv`, the exec
  capability's single **matching key** an `allowed_commands` pattern is checked
  against — never executed, since the host always spawns `program` + `args`
  directly with no shell. `CanonicalArgv` is a newtype (T3) whose sole
  constructor is this function, so the `allowed_commands` gate can never be
  checked against a hand-built string that bypassed it — that's type-enforced.
  The exec cache key is only derived from a `CanonicalArgv` by convention, at
  its single call site (`cache_path` in `cache.rs` still takes a bare `&str`
  key) — typing that parameter is open finding T22. Quotes an argument
  (single-quoted, POSIX-style, with
  an embedded `'` escaped as `'\''`) whenever it contains whitespace, a quote,
  or a backslash, or is empty, so two different argv vectors (e.g.
  `["log", "--author=a b"]` vs `["log", "--author=a", "b"]`) can never collapse
  onto the same canonical string and silently share a grant.
- `state.rs` — `sanitize_relpath -> SandboxRelPath` (rejects absolute/`..`
  paths for state I/O), `normalize_abs`/`resolve_for_allowlist -> ResolvedPath`
  (the symlink gate for arbitrary-file I/O, consulted before either read/write
  allowlist: by default denies outright on any symlink path component; when
  the plugin's `resolve_symlinks` is set, canonicalizes first — falling back
  to canonicalizing the parent for a not-yet-existing write target — and
  matches the allowlist against the *resolved* location, hard-denying rather
  than falling through if canonicalization fails; see the Config section
  below), and `dir_size`/`check_cap` (the quota primitives — `dir_size` is no
  longer walked per write; see `capability.rs`'s `state_size` memo below).
  `ResolvedPath`/`SandboxRelPath` are constructible only in this module (T2);
  `AllowedPath` (also here) is minted only by an allowlist's `check_path` on a
  match (see `allow.rs` above). The four filesystem-effect sites in
  `perform.rs` accept only these types, so "resolve → match → act" is a
  type-level fact rather than statement order.
- `paths.rs` — `expand_tilde`, `data_root`, `state_root`, `default_plugin_dir`
  (all under `$XDG_DATA_HOME/rustline`, falling back to `$HOME/.local/share/rustline`),
  plus `wasmtime_cache_config_path`/`ensure_wasmtime_cache_config` (W43): lazily
  writes `<state_root>/wasmtime-cache.toml` (via the same atomic temp-file +
  rename as `cpu.rs`'s snapshot store) — a `[cache]` block with ONLY
  `directory = <state_root>/wasmtime-cache/` and **no** `enabled` key (wasmtime
  43.x's `[cache]` is `deny_unknown_fields` and rejects `enabled`) — and returns
  its path, or `None` (best-effort, never panics) on any I/O failure or an
  unwritable root, so `build_plugin` degrades to no-cache rather than handing
  wasmtime an unusable config (N2). The cache dir is kept distinct from plugins'
  own state subdirs.
- `abi.rs` — `RenderInput` and `parse_render_output` (malformed JSON → empty
  `Vec`). The host-effect wire-result types (`HttpResult`, `CachedHttpResult`,
  `ReadResult`, `WriteResult`, and now `ExecResult`/`CachedExecResult`) all
  live in `rustline-abi` (W51, plus the exec pair added alongside `rl_exec`/
  `rl_exec_cached`) and are re-exported here, so existing `crate::abi::HttpResult`
  paths keep resolving (the same precedent as `rustline_core::segment`'s
  `Segment` re-export).
- `cache.rs` — pure helpers shared by both TTL caches: FNV-1a URL/argv→path,
  RFC3339 freshness (`age_secs`/`is_fresh`), quota-bounded `read_entry`/
  `write_entry`. `CacheEntry` carries `last_attempt_at` (when a refresh was
  last *tried*, success or not) alongside `fetched_at` (when one last
  *succeeded*). `is_fresh` itself is unchanged; the backoff is a second check
  in `perform.rs`'s cached-fetch paths (`perform_http_get_cached:90-102`,
  `perform_exec_cached:295-307`) that reuses `is_fresh` against
  `last_attempt_at` — when a last-good entry exists, a failing endpoint is
  retried at most once per TTL instead of every render. An endpoint that has
  *never* succeeded has no entry to back off against, so it is still fetched
  on every render — deliberately unchanged (see `perform.rs:172-175`).
  `write_entry` no longer wedges once its namespace hits quota:
  `evict_namespace` deletes entries oldest-`fetched_at`-
  first (a file that fails to parse sorts as oldest) until the namespace fits,
  and `write_entry` calls it only after `check_cap` first refuses the write —
  so a full state dir self-heals instead of failing every write from then on.
  `cache_path(state_dir, namespace, key)` takes a `namespace` argument —
  `HTTP_NAMESPACE`/`EXEC_NAMESPACE` — so the HTTP-response cache and the exec
  capability's own TTL cache (see `perform.rs` below) live in separate
  `<state_dir>/<namespace>/` subdirectories, both for collision-avoidance and
  so eviction in one never touches the other's entries.
- `capability.rs` — `CapabilityCtx`: one plugin instance's allowlists —
  `allowed_urls: AllowSet<UrlCap>`, `allowed_paths: AllowSet<ReadPathCap>`
  (read), `allowed_write_paths: AllowSet<WritePathCap>` (write, separate from
  `allowed_paths` — see the Config section below), `allowed_commands:
  AllowSet<CommandCap>` (the exec capability's gate) — each its own
  phantom-tagged type (T3) rather than four instances of the same untyped
  set — plus `resolve_symlinks`,
  state root, and quota, built from `PluginConfig` and held in Extism
  `UserData` so each instance only ever sees its own grants. `DenialKind`
  gains a `Command` variant alongside `Url`/`Path` (a denied write is also
  recorded as `Path`), so a denied exec call is recorded and reported
  (`rustline plugin denials`) the same way as a denied URL/path. `state_size`
  is an `AtomicU64` memo of the state dir's size: seeded by one `dir_size`
  walk on first use, then updated from each write's own return value rather
  than re-walked — `set_state_size`/`invalidate_state_size` (the latter
  called on any write path that fails, so a failed write can't leave the
  memo believing an eviction happened that didn't) keep it honest. `log_calls`
  is an `AtomicU32` counting `rl_log` calls toward `perform::
  MAX_GUEST_LOG_CALLS` — see `perform.rs`'s `perform_log` below for what
  "per process" means for a long-lived daemon instance.
- `fetch.rs` — `Fetcher` trait + `UreqFetcher` (the real rustls blocking HTTP
  client); the trait seam makes `perform_http_get`'s gating logic testable
  without a network.
- `run.rs` — `Runner` trait + `ProcessRunner`, the exec capability's
  counterpart to `fetch.rs`'s `Fetcher` seam: `Runner::run(program, args) ->
  Result<(exit_code, stdout, stderr), String>` lets every gate test in
  `perform.rs` run without spawning anything (the capability decision is made
  before `run` is ever called). `ProcessRunner` is the sole production impl —
  it spawns `program` directly with `args`, **no shell anywhere in the path**;
  stdin is closed (`Stdio::null()`, so a child reading stdin gets EOF instead
  of hanging), stdout/stderr are piped and capped at `MAX_OUTPUT_BYTES` (64
  KiB per stream, flagging `truncated`), and the whole run is bounded by
  `EXEC_TIMEOUT` (5 s, comfortably under Extism's 10 s plugin timeout) plus,
  at most, two short `OUTPUT_GRACE` periods (250 ms each) spent collecting
  output afterward. The child runs in its own process group
  (`process_group(0)`, the file's one
  `unsafe`) so a timeout/output-collection deadline kills the **group**, not
  just the immediate pid — otherwise a backgrounded descendant (`sh -c "long
  &"`) could keep the piped stdout/stderr open indefinitely and hang the
  render even after its immediate parent exits cleanly. `kill_group` only
  reaches that group, though: a descendant that escapes it via
  `setsid`/`setpgid` (a properly-daemonizing program, e.g. `emacs --daemon`,
  `ssh -f`, `tmux new-session -d`) keeps the piped fds open past the kill —
  `OUTPUT_GRACE` is what keeps that case bounded (the escaped descendant's
  own output is dropped, empty, rather than hanging the render thread and,
  transitively, the daemon's shared render lock).
