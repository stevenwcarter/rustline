# CLI — `init`, `config`, `doctor`, `completions`

> **Read when:** changing the onboarding wizard, config-file subcommands, the diagnostics command, or shell completions.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- `rustline init` — interactive onboarding wizard (needs a TTY): asks theme
  (with preview), one-/two-line status, mouse/click-to-toggle, machine-type
  widgets (laptop → `battery`, Tailscale → `tailscale_ip`, LAN → `lan_ip`),
  clock style (12h/24h ± seconds), and refresh interval, then writes
  `~/.config/rustline/config.toml` (non-destructive merge — `[theme].base` is
  always set; existing `[layout]`/`[widgets.*]` are left alone; a
  `<path>.rustline.bak` backup is written first if the file already existed)
  and upserts an idempotent `# >>> rustline >>>` / `# <<< rustline <<<` block
  into `~/.tmux.conf` (also backed up first, to `~/.tmux.conf.rustline.bak`).
  A non-TTY invocation without a flag errors with a hint instead of writing
  silently. **Re-running the wizard (W33) seeds every question's shown default
  from your previous choices** instead of resetting to the recommended
  answers, reading each answer back from wherever it was actually recorded:
  your existing `config.toml` for theme/optional-widget membership/clock, and
  the managed block in `~/.tmux.conf` for the three tmux settings config never
  stores (one-/two-line, mouse, refresh interval) — see `init.rs`'s
  `seed_answers` above — and, before writing anything, prints
  a one-line-per-answer summary plus a line diff of both files against their
  current contents and asks "Write these changes?"; answering no aborts with
  nothing written. `rustline init --defaults` runs the same two writes
  non-interactively with recommended answers. `rustline init --print` is the
  legacy behavior: prints just the raw one-line tmux block to stdout (using
  `theme.bar_bg`/`fg` for `status-style`) and writes nothing. `rustline init
  --dry-run` previews, without touching disk, what a real run would write —
  the config.toml and tmux block, each with a line diff against any existing
  file — using answers gathered the same way (`--defaults` or the
  interactive wizard); `--print` still wins over it. `rustline init
  --uninstall` strips the managed tmux block from `~/.tmux.conf` (backing it
  up first) and prints the reload command, never touching `config.toml` and
  needing no TTY; combined with `--dry-run` it only *previews* the removal
  and writes nothing at all (no file, no backup). `rustline init --binary
  <path>` overrides the binary path baked into the tmux block's `#(...)`
  calls (default: the running binary's own resolved absolute path via
  `current_exe()`) — see the tmux integration model below for why the block
  calls an absolute path rather than bare `rustline`.
- `rustline print-config` — effective config as TOML.
- `rustline config path` — print the resolved config file path.
- `rustline config edit` — open the config file in `$EDITOR` (needs a TTY);
  creates it from the starter template first if it doesn't exist yet.
- `rustline config validate` — strictly parse the config file and report any
  error with its location (unlike the total `Config::load`, which silently
  falls back to defaults); a missing file is not an error.
- `rustline doctor` — diagnose the documented prerequisites (tmux ≥ 3.1,
  `set -g mouse on` when checkable from inside a running session, a
  truecolor terminal, `rustline` on `$PATH`, and the managed tmux-conf block)
  as pass/warn/fail, plus the resolved config/themes/plugin/log paths; only
  reads and prints, never writes; exits non-zero only if any check outright
  fails (a `warn` is advisory). Also (W48) an advisory-only "render daemon"
  row: `warn` (not `fail` — the daemon is opt-in) when
  `daemon_client::daemon_socket_path()` has no socket file at all ("not
  running"), `warn` when a socket exists but doesn't answer a `Ping`
  ("present but not reachable — a stale socket from a killed daemon?"), or
  `pass` when it's reachable; never affects `doctor`'s exit code. Also
  another advisory-only row: `pass` at tmux ≥ 3.2 ("display-popup available
  (prefix + W opens the widget manager)"), `warn` below 3.2 ("prefix + W
  widget manager needs tmux >= 3.2 (display-popup); the status line itself
  works") — the widget-manager popup binding needs `display-popup` (tmux
  ≥ 3.2), one version tier above the `range=user|` floor (≥ 3.1) the status
  line itself needs; this row never affects the exit code either. Also
  another advisory-only row, "plugin checksums": hashes every configured
  `[plugins.*]` entry's installed `.wasm` (via `plugin_checksum::status_for`
  — the same read-and-verify helper backing `plugin list`'s `checksum_status`
  field, below) and reports a summary — `"<N> verified, <M> unpinned"`, plus,
  only when there's something actionable, `"; checksum mismatch: <names>"` /
  `"; malformed checksum: <names>"` / `"; .wasm not found: <names>"` — or
  `"no plugins configured"` when `[plugins.*]` is empty. **Always `ok`/`warn`,
  never `fail`**: a checksum problem is real and worth surfacing, but
  `doctor`'s exit code is reserved for setup that's outright broken, and the
  checksum gate itself already degrades a bad plugin to "widget missing,"
  never to a broken bar (N2) — structurally, `run()`'s exit-code count only
  ever tallies `CheckStatus::Fail` entries, and this row is coded to never
  produce one. Finally, another advisory-only row, **"widget readers"**
  (`check_readers`): for each reader kind actually present in the configured
  layout (via `cfg.layout_kinds`, so both regions and `[instances.*]` count),
  it calls that reader and reports `warn` naming any that yield `None`. This
  is the diagnostic channel those readers otherwise lack — they each collapse
  every failure into `None`, so a missing `git`/`playerctl` binary, a
  configured `mount` that no longer exists, or a failing `tmux list-windows`
  produced a widget that silently vanished with nothing in the log. `warn`,
  never `fail`, like the rows above. Two deliberate exclusions: `throughput`
  is omitted because it legitimately returns `None` on a first invocation (a
  rate is a delta — invariant #6), which would warn on every fresh state dir;
  and `read_git` is probed against `doctor`'s own CWD rather than a pane path,
  so running `doctor` outside a repo reports git as unavailable even though
  the widget works fine in a pane that is inside one.
- `rustline completions <bash|zsh|fish>` — print a shell-completion script
  (via `clap_complete`) to stdout.
