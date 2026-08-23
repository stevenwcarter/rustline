# `rustline` bin — `init` wizard and tmux config block

> **Read when:** changing the onboarding wizard, its re-run seeding, or the managed `~/.tmux.conf` block `init` emits.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- `tmux_conf.rs` — `init_block(&InitBlockOpts)` (`bar_bg`, `fg`, `two_line`,
  `mouse`, `interval`, `binary`): the tmux config block `rustline init` emits,
  incl. `bind -T root MouseDown{1,2,3}Status` blocks — left (window-select
  default preserved, else `click --button=left`), plus middle/right dispatching
  `click --button=middle`/`--button=right` for W36 config bindings (see CLI
  below); one-line/mouse-off/interval-1 output stays otherwise unchanged. `two_line`
  additionally emits `set -g status 2` plus the author's verbatim two-line
  `status-format[0]`/`[1]` (window list on its own line). `TMUX_BEGIN`/
  `TMUX_END` (`# >>> rustline >>>` / `# <<< rustline <<<`) and
  `upsert_tmux_block(existing, block) -> String`: idempotently insert/replace
  the rustline-managed region in an existing `~/.tmux.conf`, leaving
  surrounding user content untouched. `BlockAnswers { two_line, mouse,
  interval: Option<u32> }` + `parse_block_answers(existing) ->
  Option<BlockAnswers>` are the **inverse of `init_block`** for the three
  wizard answers config never stores, so `init::seed_answers` can show them
  back as a re-run's defaults; `None` when no complete managed region is
  present, and `interval` is `Option` so the *caller* owns the fallback rather
  than this module knowing the wizard's defaults. Matching is deliberately
  **whole-line** (trimmed): the block emits the discoverability comment
  `(needs: set -g mouse on)` unconditionally, so a substring search would
  report `mouse: true` even for a mouse-off block. Its round-trip test drives
  `init_block` rather than a hand-written block, so changing an emitted
  setter line without teaching the parser fails loudly instead of silently
  resetting a user's choice.
- `init.rs` — the `rustline init` wizard shell: `InitAnswers`/`ClockStyle`
  (the collected answers + four datetime presets), `starter_config_toml(&
  InitAnswers) -> String` (mutates the embedded starter template — theme,
  layout arrays, datetime format, pruning unselected optional widgets'
  `[widgets.*]` sections), `merge_config(existing, generated, theme) ->
  Result<String, String>` (non-destructive merge: `[theme].base` always set,
  `[layout]`/each `[widgets.<name>]` added only if absent), `write_config`
  (backs up an existing file to `<path>.rustline.bak` first), `defaults()`
  (the `--defaults`/recommended answer set), `parse_menu_choice`/
  `parse_yes_no` (pure prompt-parsing, unit-tested), and `run`/`prompt_answers`
  (the I/O shell: `--print` wins and emits the legacy one-line block via the
  caller's already-resolved theme; else `--defaults` or the interactive
  prompt loop, erroring on non-TTY stdin without a flag). Also (W33):
  `fn seed_answers(config_path: &Path, tmux_conf_path: &Path) -> InitAnswers`
  pre-fills a re-run's answers from **both** places a previous run wrote them.
  From `config.toml`: `theme` from `cfg.theme.base`,
  `battery`/`lan_ip`/`tailscale` from whether that name appears anywhere in
  `cfg.layout.{left,center,right}`, and `clock` reverse-mapped from
  `cfg.widgets.datetime.format` via the pure `fn clock_from_format(fmt: &str)
  -> Option<ClockStyle>` (exact match against the four presets, `None` on no
  match). The three tmux-only answers (`two_line`/`mouse`/`interval`) are
  tmux settings that config never stores, so they're recovered by
  reverse-parsing `~/.tmux.conf`'s managed block via `tmux_conf::
  parse_block_answers` (see `tmux_conf.rs` below) and overlaid on top. That
  overlay is load-bearing for `two_line` specifically: its recommended
  default is one-line, so the original config-only seed meant an
  Enter-through re-run silently rewrote a two-line status bar back to one
  line (`mouse`/`interval` merely *happened* to match their defaults for a
  user who took the recommendations). Each source degrades independently — a
  missing config file keeps `defaults()` (with `battery` still
  hardware-probed via `battery::read_battery()`, preserving the pre-W33
  fresh-run default), and a missing or blockless tmux conf keeps the
  recommended tmux answers.
  `prompt_answers` gains a `seed: &InitAnswers` parameter and shows each
  seeded value as that question's default. `fn summarize_answers(a:
  &InitAnswers) -> String` renders one line per collected answer for the new
  confirm step: after gathering answers (interactive runs only —
  `--defaults`/`--dry-run`/`--print`/`--uninstall`/non-TTY are unchanged),
  `run` prints the summary plus the existing dry-run line-diff machinery
  (`dry_run_config`/`dry_run_tmux_block`/`line_diff`) against both files'
  current contents, asks `ask("Write these changes?", true)`, and only calls
  `apply` on yes — a no prints "Aborted; nothing written." and writes
  nothing. `assets/
  starter-config.toml` (embedded via `include_str!`) is the recommended
  starter template `init.rs` mutates — the shortened `alt_format`s and
  curated layout it seeds live only here, **not** in any widget's code
  `Default` (those stay `""`/unchanged; see Config below).
