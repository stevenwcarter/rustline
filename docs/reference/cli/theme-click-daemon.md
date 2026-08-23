# CLI — `theme`, `click`, `daemon`

> **Read when:** changing theme management, click dispatch, or the daemon run/status/stop/install/uninstall commands.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- `rustline theme list [--json]` — every built-in + themes-dir theme, marking
  the active one and any built-in shadowed by a same-named file; `--json`
  emits a JSON array of `{name, active, source, shadowed}` (`source` is
  `"builtin"` or `"file"`) instead of human text.
- `rustline theme show <name>` — ANSI preview of `<name>` (default layout,
  synthetic Context tuned to show warning/error alert badges).
- `rustline theme use <name>` — set `[theme].base = "<name>"` in the config
  file (`toml_edit`, comment-preserving); errors without writing if `<name>`
  doesn't resolve.
- `rustline theme new <name> [--from <seed>] [--force] [--edit]` — scaffold
  `<themes_dir>/<name>.toml` as a complete, tweakable copy of `<seed>`
  (default `default`); refuses to overwrite an existing file without `--force`.
  `--edit` opens the new file in `$EDITOR` afterward (needs a TTY) and prints
  the `theme use <name>` follow-up either way.
- `rustline theme pick` — interactively list the themes (active marked,
  themes-dir files tagged `(custom)`), preview any by number (or `a`/`all` for
  every one), then prompt to set one by name or number (blank keeps the
  current theme), writing `[theme].base` via the same path as `theme use`.
  Previews default to a healthy bar (palette only); `t` toggles the
  warning/error alert-badge colors on/off. Requires a terminal — a non-TTY
  invocation prints a hint toward `theme show`/`theme use` and exits non-zero
  without writing.
- `rustline click --range=<name> [--button=left|middle|right]` — resolve the
  click via `click::resolve_click` and dispatch it (W36): the default
  (unconfigured) action is a left-click toggle of `<name>`'s membership in the
  global toggle state file; a `[widgets.<name>.click]` binding
  (`left_click`/`right_click`/`middle_click`) overrides per button with a
  toggle/`open_url`/`run` action. Invoked by the `init`-emitted
  `MouseDown{1,2,3}Status` tmux bindings (left/middle/right).
- `rustline daemon [run] [--plugin-dir <d>] | status | stop` (W48) — the
  optional persistent daemon. `rustline daemon` and `rustline daemon run` are
  equivalent (bare form defaults to `run`): runs the server in the
  foreground, keeping the parsed config, resolved theme, built widget
  registry, and instantiated WASM plugins warm across renders instead of
  rebuilding them on every tmux refresh; a supervisor (systemd unit, tmux
  `run-shell … &`) backgrounds it — rustline never self-daemonizes/forks.
  `rustline daemon status` connects and pings, printing running/not-running
  with the exit code reflecting it; `rustline daemon stop` connects and asks
  it to shut down. Render clients (`render left|right|window`) try the
  daemon's socket first and **transparently fall back to the normal
  in-process render** on any failure (including the daemon simply not
  running) — the daemon is purely an optional speedup, never a dependency.
  One caveat: a per-invocation `render --plugin-dir=X` is ignored while a
  daemon is reachable (the wire protocol carries no `plugin_dir`; the daemon
  always uses whatever `--plugin-dir` it was started with).
- `rustline daemon install [--write-only] [--binary <path>]` — install the
  daemon as a per-user service that starts at login, using the platform's
  native mechanism (chosen at compile time; `ExecStart`/`ProgramArguments`
  `<binary>` defaults to the running binary's own resolved absolute path, same
  resolution `init` uses). **On Linux** it writes a systemd **user** unit to
  `$XDG_CONFIG_HOME/systemd/user/rustline-daemon.service` (fallback
  `~/.config/systemd/user/`) and, unless `--write-only` or `systemctl` isn't on
  `PATH`, runs `systemctl --user daemon-reload` + `enable --now
  rustline-daemon.service`. **On macOS** it writes a launchd **LaunchAgent** to
  `~/Library/LaunchAgents/rustline-daemon.plist`
  (`RunAtLoad`, `KeepAlive={SuccessfulExit=false}` = restart on failure only)
  and, unless `--write-only` or `launchctl` isn't on `PATH`, loads it via
  `launchctl bootout` (clear any stale instance) + `bootstrap gui/$UID
  <plist>`. Either way it reports a replaced file and prints the manual load
  command as a fallback; a `systemctl`/`launchctl` failure is never fatal — the
  unit/plist file is already written.
- `rustline daemon uninstall` — best-effort unload the service (Linux
  `systemctl --user disable --now`, macOS `launchctl bootout`, ignoring "not
  loaded"), remove the installed unit/plist file if present (Linux also reloads
  the user daemon-reload cache); prints what it did, including a "nothing to
  remove" note if it was already gone. Safe to run more than once.
