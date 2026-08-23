# Config — logging and warn dedup

> **Read when:** changing the `[log]` table, log file rotation/naming, or how a static-misconfiguration warning is deduplicated.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

**Logging:** a `[log]` table controls the two sinks. `rustline` logs to a
**daily-rotated** file (default dir `$XDG_DATA_HOME/rustline`, default level
`info`) and to stderr (default level `error`). `RUST_LOG` is **not**
consulted. Raise the *file* level with repeated `-v` (`-v`=warn, `-vv`=info,
`-vvv`=debug, `-vvvv`=trace); `-v` never affects stderr. Today's file is
`<dir>/rustline.<YYYY-MM-DD>.log` (e.g. `rustline.2026-07-27.log`) — **not**
`rustline.log.<date>` — computed in **UTC**, matching the boundary
`tracing-appender`'s `RollingFileAppender` itself rotates on; **7** daily
generations are kept (`MAX_LOG_FILES`), each pruned as the eighth appears.
This replaced the old size-capped rotator: there is no 5 MiB cap and no
single `.1` backup anymore — `[log].file`, if set, no longer names one fixed
path. Instead it's decomposed into a directory (its parent), a filename
prefix (its stem), and a filename suffix (its extension), which the daily
appender recombines as `<prefix>.<date>.<suffix>` each day. Any level value
is `off|error|warn|info|debug|trace` and is parsed leniently (a typo falls
back to the default).

    [log]
    file_level   = "info"
    stderr_level = "error"
    file         = "~/.local/share/rustline/rustline.log"   # optional override; decomposed into dir/prefix/suffix — see above

**Warn dedup:** a `warn!`/`info!` that describes a *static misconfiguration*
(an unparseable theme file, an unknown widget name, a plugin with no
`abi_version` export, …) — one that would otherwise re-fire on every render
tick, forever — is logged at most once per config generation via
`rustline_core::diag::warn_once` (see `diag.rs`/`warn_once.rs` in the module
map above). The dedup state lives at `<data_root>/.warn-markers/`
(`~/.local/share/rustline/.warn-markers`), a sibling of `denials.jsonl`,
`toggles`, `plugin-index.json`, and the log directory under the same
`data_root()`; it is cleared whenever `config.toml`'s mtime changes, so
editing the config re-arms every warning for one more render. A genuine
per-occurrence runtime failure (a plugin render failing, a cache write
failing, a denial) is deliberately NOT routed through this — only a
misconfiguration that stays constant between renders is.

