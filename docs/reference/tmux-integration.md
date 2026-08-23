# tmux integration model

> **Read when:** changing what `rustline init` emits into `~/.tmux.conf`, adding a tmux binding, or touching anything that interpolates a tmux format variable.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

Shell-out per region on `status-interval`. Each `#(<binary> render …)` spawn
is still a fresh process either way — the difference an optional `rustline
daemon` (W48, see CLI above) makes is what happens *inside* that process:
with a reachable daemon, the spawn is a thin Unix-socket round-trip against
already-warm state instead of a cold rebuild of the config/theme/registry/
plugins. Nothing about the tmux wiring itself changes; `init` doesn't wire up
the daemon (no wizard question for it yet — start/manage it yourself, see
README). The block
`rustline init` writes (via `init_block`) wires `status-left`/`status-right`/
`window-status-format` to `#(<binary> render …)` — `<binary>` is the
resolved, shell-quoted absolute path to the running binary
(`std::env::current_exe()`, overridable with `init --binary <path>`), not a
bare `rustline`, since tmux's `#(...)` shells out via the *tmux server's*
`/bin/sh`, whose `$PATH` may not include wherever the user installed it
(e.g. `~/.local/bin`) — a bare name there can silently resolve to nothing and
leave the bar empty — and adds `after-select-pane`/`after-select-window` →
`refresh-client -S` hooks for instant updates. It also emits three
`bind -T root MouseDown{1,2,3}Status` blocks (left/middle/right — tmux button
numbering): the **left** binding keeps the default `select-window` on a
window-name click and otherwise runs `rustline click --range=… --button=left`;
the **middle**/**right** bindings (W36) have no window-list default to preserve,
so they simply dispatch any non-empty `#{mouse_status_range}` as
`--button=middle`/`--button=right` — the action per (widget, button) is chosen
by `[widgets.<name>.click]`, which is why right/middle previously shipped inert
(they resolved but had no tmux binding to fire them). Each block ends with
`refresh-client -S`. All three follow the same injection-safe
`--range=#{q:mouse_status_range}` form (invariant #4). This requires
**tmux ≥ 3.1** (that's when
`range=user|X` status ranges and the `mouse_status_range` format variable were
added) and, at the tmux-config level, `set -g mouse on` — the wizard's mouse
question (`InitAnswers.mouse`) can add that setter for you (`--print` never
does; it always emits the mouse-off, one-line legacy block regardless of
config).

**Widget-manager popup (`prefix + W`).** The block also emits
`bind-key W display-popup -E -w 80% -h 80% "<binary> widget edit"` —
unconditionally, the same way the three `MouseDown*Status` blocks are, and
with no tmux format variable interpolated (nothing to `#{q:}`-escape). This
needs **tmux ≥ 3.2** for `display-popup` (one version tier above the ≥ 3.1
the status line itself needs for `range=user|`); `doctor` carries an
advisory-only row for it (see CLI above). Because `init_block` emits this
line the same way it emits the mouse bindings, `rustline init --print`'s
legacy one-line block **also** carries the `bind-key W` line now — `--print`
was never special-cased to omit it, and no test pins the legacy block
byte-for-byte, so this is the correct, consistent outcome rather than a gap.
`init --uninstall` removes it along with the rest of the managed block, same
as every other binding.

**Two-line window list is batched (W42).** In two-line mode, `status-format[0]`
(the window-list line) used to invoke tmux's own `#{W:...}` loop, which expanded
to a `#{T:window-status-format}` per-window `#(<binary> render window …)` call —
one process spawn per window, every refresh. It's now a single `#(<binary>
render windows --session=#{q:session_name})` call: `rustline` itself shells out
to `tmux list-windows -F` (`windows.rs`, client-side — not another tmux format
expansion) and renders every window's pill in one process, re-emitting each
one's `#[range=window|IDX]` so tmux's built-in window-select click keeps
working (N spawns per refresh → 1). Injection-safe: the only interpolated tmux
var is the `#{q:}`-escaped `session_name`; window names/flags come back from
`tmux list-windows -F`, never through the shell. This batched path is
deliberately **in-process only** — unlike the singular `render window` (and
`render left`/`render right`), it does not try the daemon first, since a
systemd/launchd-managed daemon has no `$TMUX` in its environment to run `tmux
list-windows` against. **One-line mode is unchanged** —
`window-status-format`/`window-status-current-format` still call `render
window` once per window.

**Injection safety (critical):** tmux expands `#{…}` inside `#(…)` *before*
`/bin/sh -c` and does not shell-escape. So the `init` block passes every tmux var
as `--flag=#{q:VAR}` — the `#{q:}` modifier escapes it and the `--flag=` form is
empty-safe. Never emit a bare `'#{window_name}'` or `'#{pane_current_path}'`.
This is why `render window` takes named args, not positional; the click
binding's `--range=#{q:mouse_status_range}` follows the same rule. See
`tmux_conf.rs`.

