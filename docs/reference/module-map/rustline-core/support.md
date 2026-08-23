# `rustline-core` — `ansi.rs`, `atomic_write.rs`, `diag.rs`

> **Read when:** transcoding tmux markup for a terminal or ratatui, persisting a file atomically, or adding a `warn_once`-deduped diagnostic.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- `ansi.rs` — two renderers over one scanner, for the two kinds of sink that
  consume tmux markup. `tmux_to_ansi(&str) -> String` transcodes it to ANSI SGR
  (`colourN` → 256-color, `#rrggbb` → truecolor, named → basic) for a sink the
  **terminal** interprets: `render … --preview`, `theme show`/`pick`.
  `parse_markup(&str) -> Vec<StyledSpan>` resolves the same markup into
  `StyledSpan { style: Style, text: String }` runs for a sink that carries its
  own style type and never interprets escapes — a `ratatui` cell buffer draws
  an ANSI escape as *literal visible characters* and applies no styling, so
  `widget_tui.rs`'s preview strip uses this and maps each span onto a ratatui
  `Style`. tmux directives are stateful (`#[bg=blue]` leaves an earlier
  `fg=red` alone), which a terminal tracks for itself but a cell buffer can't,
  so `parse_markup` accumulates the running style and snapshots it per run;
  `fg=default`/`bg=default` reset that channel to `None` ("the terminal's own
  default", not black). Both share a private `scan` so where a directive
  starts/ends — and what an unterminated `#[` means — has one definition.
- `atomic_write.rs` — `write_atomic(path, contents: &[u8]) -> io::Result<()>`,
  the workspace's **one** atomic-write primitive (B15+B17): stage into a
  per-writer, per-call temp file in the same directory
  (`<path>.<pid>.<nanos>.<counter>.tmp`) and `rename` onto the target, so a
  concurrent reader always sees either the old contents or the new ones,
  never a torn write — and, being uniquely named per call rather than a
  shared `<name>.tmp`, two rustline processes (tmux spawns `render left`/
  `render right` as separate processes every tick) staging the same target
  can no longer truncate each other's temp file. Every persistence site in
  the workspace goes through this: `sample_store.rs`, `toggles.rs`, `cpu.rs`
  (below), and `rustline-wasm`'s `perform::perform_state_write`, `paths.rs`,
  and `cache.rs`. Trade-offs accepted and documented on the module: a
  crash-orphaned staging file is swept inside the WASM cache namespaces
  (`evict_namespace`) but not under a plugin's state-dir root or
  `~/.local/state/rustline/` (open finding); the ~33-char staging suffix can
  push an already-long target name over the filesystem's name limit (only
  reachable via a plugin's state relpath, which has no length bound); and the
  brief on-disk overlap of the old file and the staging file can transiently
  over-count against a plugin's state-dir quota.
- `diag.rs` — the `warn_once` seam: `warn_once(key: &str, emit: impl Fn())`
  calls `emit()` unless a process-wide hook, installed once via
  `set_warn_once_hook`, has already seen `key`. **Fails open**: with no hook
  installed (every unit test, `rustline-core` used standalone) it always
  emits, so a dedup layer can never be the reason a diagnostic goes missing.
  The hook itself lives in the `rustline` bin crate's `warn_once.rs` (below) —
  the only thing that ever calls `set_warn_once_hook`. Every `warn!`/`info!`
  site here and in `rustline-wasm` that describes a *static misconfiguration*
  (one that would otherwise re-fire once per render, forever, at up to
  ~86,400 lines/day) routes through this function instead of calling
  `tracing::warn!`/`info!` directly; a genuine per-occurrence runtime failure
  (a plugin render failing, a cache write failing, a denial) never does. `key`
  must identify both the site and its varying payload (e.g.
  `"unknown-widget:memroy"`) so two distinct misconfigurations each warn once
  rather than one suppressing the other.

