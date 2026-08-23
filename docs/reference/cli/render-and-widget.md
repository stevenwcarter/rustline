# CLI — `render` and `widget`

> **Read when:** changing render invocation, the batched window list, or the widget-manager commands (`list`/`enable`/`disable`/`move`/`edit`).
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- `rustline render left|right [--session= --window= --pane= --pane-path=] [--preview] [--plugin-dir=]`
- `rustline render window [--current] --index= [--name=] [--flags=] [--preview]`
  (no `--plugin-dir` — windows don't run plugins in v1)
- `rustline render windows [--session=<s>] [--preview]` (W42) — renders a
  whole session's window list in one call (batched; backs the two-line
  `status-format[0]`); shells out to `tmux list-windows` client-side; runs
  **in-process, NOT daemon-routed** (a systemd/launchd daemon has no `$TMUX`
  to run `tmux list-windows` against). One-line mode is unchanged — it still
  renders per-window via `render window`.
- `rustline widget list [--json]` — every widget (built-ins, `[instances.*]`,
  and discovered plugin stems) and where, if anywhere, it currently sits in
  `[layout]`; `--json` emits `{name, summary, source, region, index}` per row
  (`source` `"builtin"`/`"plugin"`/`"instance:<kind>"`; `region`/`index` are
  `null` for an unplaced widget) instead of human text.
- `rustline widget enable <name> [--region left|center|right] [--index N]` —
  add a widget to a layout region (default `right`, appended unless
  `--index` is given); refuses (writing nothing, exit `1`) if `<name>` is
  unknown or already placed somewhere.
- `rustline widget disable <name>` — remove a widget from whichever region
  holds it; refuses if it isn't placed anywhere.
- `rustline widget move <name> --region <r> [--index N]` — relocate a placed
  widget to another region and/or position.
- `rustline widget edit` — open the interactive full-screen `ratatui` editor
  (needs a TTY): four columns (LEFT/CENTER/RIGHT/AVAILABLE), arrow or vim
  keys to move focus, `space`/`Enter` to add/remove, `J`/`K` to reorder,
  `w` to write, `q` to quit (confirms first if there are unsaved changes),
  `?` for a help overlay, plus a live colour preview strip of the layout so
  an edit's effect is visible before you save — one line when the regions fit
  side by side, otherwise one region per line. See `widget_tui.rs` above.
  `enable`/`move --region center` still write and exit `0` (with a stderr
  note) since nothing renders `[layout].center` — see the Config section's
  CENTER-region note below; `widget edit`'s TUI refuses a CENTER edit
  outright instead, with an on-screen explanation.
