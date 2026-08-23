# `rustline` bin — logging and the bench harness

> **Read when:** changing the tracing subscriber, log rotation, cross-process warn dedup, or the feature-gated bench passes.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- `logging.rs` — `init(&LogConfig, verbose)`: installs the two-sink `tracing`
  subscriber (a daily-rotated file, via `tracing_appender`'s
  `RollingFileAppender`, plus stderr), and the pure helpers
  `verbosity_to_level`, `parse_level`, `resolve_file_level`/
  `resolve_stderr_level`, `log_dir`/`log_file_prefix`/`log_file_suffix`
  (decompose `[log].file` into the directory + filename prefix/suffix the
  appender needs to name each day's generation
  `<prefix>.<YYYY-MM-DD>.<suffix>`), `current_log_path` (today's file, in
  UTC — the appender rotates on a UTC day boundary, so this must not use
  local time, or `doctor`'s reported path would disagree with what the
  appender actually writes for anyone off UTC+0), and `build_appender` (the
  one builder chain both production `init` and the test suite call, so they
  can't drift). `open_log`/`log_path`/`should_rotate` from the old
  size-capped, single-generation rotator no longer exist. Best-effort: a log
  dir that can't be created or opened degrades to stderr-only; never writes
  stdout.
- `warn_once.rs` — the real cross-process dedup layer behind
  `rustline-core`'s `diag::warn_once` seam (above). `install(config_path)`,
  called once from `main` immediately after `logging::init`, installs a hook
  that claims a marker file per distinct `warn_once` key under
  `<data_root>/.warn-markers/` (a sibling of `state/`, deliberately not a
  descendant of it — see the module doc for the collision an unvalidated
  plugin stem like `.warn-markers` could otherwise cause) via
  `OpenOptions::create_new(true)`, atomic on every filesystem this project
  supports, so concurrent renderers racing the same key still produce exactly
  one winner with no lock. The whole marker directory is cleared whenever the
  config file's mtime changes (`reset_if_generation_changed`), so each
  misconfiguration is logged once per config edit rather than once per render
  tick. **Fails open, all the way down**: a marker I/O failure is treated as
  "emit", and a marker dir that exists but is wedged (present, non-empty,
  unwritable) makes `install` skip installing the hook at all for that run —
  degrading to no dedup rather than to permanent silence.

- `bench/` (`#[cfg(feature = "bench")]`) — the `rustline bench` subcommand:
  `harness.rs` (`summarize`/`measure`/`Stats`/`Row`/`Group`), `fixture.rs`
  (`fabricated_context` — the pure-pass mock seam, now a thin wrapper over the
  shared `sample_context`, W52), `render_passes.rs` (pure
  widget + pure/real region passes), `sources.rs` (per-read timing + the
  source registry), `plugins.rs` (real-preserved-state plugin timing, plus
  `bench_plugin_builds` (W43): a per-plugin `build_plugin` A/B — compile cache
  OFF (`with_cache_disabled`, a full Cranelift compile every build) vs ON (warm
  deserialize) — the cold-start compile cost the preserved-state pass doesn't
  isolate; measured ~13× faster (~48→~3.7 ms, ~45 ms/plugin saved)), `daemon.rs`
  (`bench_daemon`, gated on `crate::daemon::status_at`): a daemon-vs-in-process
  A/B for `left`/`right`, quantifying the win a warm persistent daemon (W48)
  gives by keeping WASM plugins instantiated — the in-process side rebuilds the
  registry and re-runs `rustline_wasm::register_plugins` fresh every sample
  (unlike `render_passes.rs`'s real pass, which never registers plugins at
  all), while the daemon side is a real round-trip via
  `daemon_client::try_render_at`; skips with an informational note (never
  spawns a daemon) when none is reachable at the resolved socket, and reports
  a per-region speedup line in the `Group`'s `note` (median-based). And
  `report.rs` (comfy-table pretty/markdown). Gated behind the `bench` cargo
  feature; the default binary is unchanged.

