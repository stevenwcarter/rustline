# `rustline` bin — widget and theme commands

> **Read when:** changing `widget list/enable/disable/move`, the interactive `widget edit` TUI, or any `theme …` subcommand.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- `widget_cmd.rs` — `rustline widget …`: `read_layout`/`write_layout` are a
  `toml_edit` reader/writer pair over `[layout]` (handling both a standard
  `[layout]` table and an inline `layout = { ... }` form alike, and falling
  back per-region to `Config::load`'s defaults when a region is absent) —
  the same in-place-edit approach `plugin_cmd.rs`/`theme_cmd.rs` use.
  Every mutation goes through `rustline-core`'s pure `layout_enable`/
  `layout_disable`/`layout_move` (see `config.rs` above) via a shared
  `mutate` helper: load config → validate the name against `known_names`
  (built-ins + non-colliding instances + discovered plugin stems, via
  `widget_placements`) → apply the edit → write + `println!` a human summary
  (`describe`) → best-effort `tmux refresh-client -S` when inside tmux. A
  refused edit (unknown name, unknown region, or a `LayoutEditError`) prints
  to stderr and exits `1` **without writing anything**. `--region center` is
  the one case that's legal but inert here (see the module doc and invariant
  discussion in `config.rs`/CENTER-region above): `warn_if_center` prints a
  note rather than refusing, since the CLI path must still be able to
  round-trip the default `center = ["windows"]`. `list [--json]` renders
  `widget_placements` as `<region>[<index>] name summary (source)` lines, or
  `placements_json` — `{name, summary, source, region, index}` per row,
  `source` one of `"builtin"`/`"plugin"`/`"instance of <kind>"`
  (`"instance:<kind>"` in JSON). `Edit` dispatches to `widget_tui::run`.
- `widget_tui.rs` — the interactive `rustline widget edit` full-screen editor
  (`ratatui` + `crossterm`), built the same way `theme_cmd.rs`'s `run_picker`
  is: a pure state machine (`EditorState::on_key(KeyKind) -> EditorAction`,
  unit-tested with no terminal involved) plus a thin terminal-owning shell
  (`run`, which needs a TTY — mirroring `theme pick`'s guard — and errors with
  a hint toward `widget list` on a non-interactive invocation). Four
  `Column`s (`Left`, `Center`, `Right`, `Available`) are drawn side by side;
  arrow keys or vim keys (`h`/`j`/`k`/`l`) move focus/cursor, `space`/`Enter`
  adds a selected AVAILABLE widget to RIGHT (the only region AVAILABLE
  placement ever appends into — `last_region` is unconditionally RIGHT once
  focus reaches AVAILABLE, since `Column::ALL`'s `[Left, Center, Right,
  Available]` adjacency always passes through RIGHT immediately beforehand)
  or removes a placed one back to AVAILABLE, `J`/`K` nudge a widget's
  position within its region (via `layout_nudge`), `H`/`L` (shifted,
  mirroring `J`/`K`; both the `Char('H')` and `Char('h')`+SHIFT reporting
  forms) move a *placed* widget directly to the other editable region (via
  `layout_move` + the free `adjacent_editable_region` helper, which skips
  over CENTER rather than stopping there — the only way LEFT is reachable
  from RIGHT at all) — CENTER is still refused as a source (focus a widget
  in CENTER and press `H`/`L`, same as `space`/`J`/`K`, and it's refused
  with `CENTER_FIXED_STATUS`), `w` writes the in-memory `Layout` to
  `config.toml` (same `read_layout`/`write_layout` as `widget_cmd.rs`), `q`
  quits (prompting to confirm first if there are unsaved changes — `dirty`
  tracking — else quitting immediately), and `?` toggles a help overlay. A
  live preview strip along the bottom re-renders the regions (via
  `render_named_region` + `ansi.rs`'s `parse_markup`, mapped onto ratatui
  `Style`s by `to_ui_style`/`to_ui_color`) against the **in-memory,
  possibly-unsaved** layout, so a move/add/remove is visible immediately,
  before `w`. It deliberately does NOT use `tmux_to_ansi`: ratatui writes
  cells and never interprets ANSI escapes, so escape bytes handed to it are
  drawn as literal text with no colour at all. The strip is one line when the
  regions fit side by side and otherwise stacks one region per line —
  `preview_layout(widths, available)` decides, measuring **display width**
  (`unicode-width`, since the bar is full of multi-byte and double-width
  glyphs) plus a 3-column gap per adjacent pair; stacked rows keep each
  region's real alignment (LEFT left, CENTER centred, RIGHT right) so a
  narrow terminal still conveys where each region sits. `Column::
  Center` stays focusable/visible (so a user can see the window list lives
  there) but every edit targeting it is refused outright with an
  explanatory status line (`CENTER_FIXED_STATUS`) — unlike `widget_cmd.rs`'s
  CLI path, which still writes a center edit with a warning (round-tripping
  the default config matters there; a "wrote but nothing happened" surprise
  is cheaper to prevent up front in an interactive editor). `TerminalGuard`
  (a `Drop` impl) restores raw mode and leaves the alternate screen on every
  exit path, including a panic unwinding through the draw loop (this
  workspace's default `panic = "unwind"`, so `Drop` still runs).
- `theme_cmd.rs` — `rustline theme …`, mirroring `plugin_cmd.rs`'s `toml_edit`
  approach: `list` prints every built-in and themes-dir `*.toml` stem,
  marking the active one (`cfg.theme.base`, default `"default"`) with `*` and
  a built-in **shadowed** by a same-named file; `list --json` (W40) emits
  `theme_list_json` instead — one `{name, active, source, shadowed}` entry per
  theme (`source` `"builtin"`/`"file"`), built-ins first then themes-dir
  files, human output unchanged when the flag is absent; `show <name>` resolves
  (file-first, then built-in), builds a synthetic `Context` engineered to
  trip warning+error badges, renders the default layout, and prints ANSI via
  `tmux_to_ansi`; `use <name>` validates `<name>` resolves, then sets
  `[theme].base = "<name>"` in the config file via `toml_edit` (refuses to
  write on an unknown name or unparseable existing config). Both `use` and
  `show`'s unknown-name error lists every resolvable theme via the shared
  `available_themes_line(themes_dir)` (W18) — built-ins AND themes-dir custom
  stems, not just built-ins (the pre-W18 hint). `new <name>
  [--from <seed>] [--force]` resolves `<seed>` (default `"default"`) to a
  full `Theme`, converts it via `ThemeConfig::from_theme` to an all-`Some`
  config, and writes `<themes_dir>/<name>.toml` with a header comment
  (refuses to overwrite without `--force`; rejects a `name` containing `/`,
  `\`, `..`, or empty); `pick` is the interactive browse-and-set command,
  layered on top of the same helpers: `picker_entries` builds the ordered,
  name-deduped `PickEntry { name, active, from_file }` list (built-ins first,
  then themes-dir-only stems), `parse_preview_input`/`parse_set_input` are
  pure parsers for the two prompt loops (the preview loop also accepts
  `t`/`toggle` → `PreviewCmd::ToggleAlerts`), and `run_picker` (reader/writer-
  generic over `BufRead`/`Write`, so it's unit-tested with byte-slice
  reader/writer — no real TTY needed) drives preview-then-set and returns the
  chosen name or `None` to keep the current theme. Its previews default to a
  **healthy** synthetic bar (palette only, no alert badges — what a normal
  status line looks like); `t` toggles the warning/error alert colors on to
  sample a theme's semantic colors, and immediately **re-renders the last
  previewed item** (tracked as `LastPreview::{One(idx),All}`) via the shared
  `render_one`/`render_all`/`replay_preview` helpers, so the toggle's effect is
  visible without re-typing a number (a toggle before any preview just prints
  the status line). The healthy-vs-pegged synthetic readings
  are chosen by a `show_alerts` bool threaded through
  `sample_context`/`preview_theme_ansi`/`preview_named` (`theme show` and the
  `init` wizard's one-shot preview pass `true`, keeping their alert-badge
  demo). `pick` itself requires a
  TTY (`stdin().is_terminal()`; a non-interactive invocation prints a hint
  toward `theme show`/`theme use` and exits non-zero, writing nothing) and,
  on a choice, reuses `use_theme` for the actual config write.
