# `rustline theme pick` — interactive theme previewer/switcher — design

## Problem

Choosing a theme from the command line today is a multi-step shuffle:
`rustline theme list` shows names, `rustline theme show <name>` previews one at a
time, and `rustline theme use <name>` sets one. To compare a few themes you run
`theme show` repeatedly, remember which you liked, then type `theme use`. There's
no single "experiment, see several, and set" flow — the interactive picker that
exists lives *inside* the `init` wizard, which also rewrites `~/.tmux.conf` and
`config.toml` and is far too heavy for "I just want to try themes and pick one."

## Goal

Add `rustline theme pick`: an interactive command that lists the available
themes (built-ins + themes-dir files, active one marked), lets you preview any
of them on demand (or all at once), and then sets your choice — writing
`[theme].base` through the existing comment-preserving path. Plain line-prompts,
**no new dependency**, consistent with the `init` wizard's prompt style.

## Non-goals

- No full-screen / arrow-key TUI (would need `crossterm`/`ratatui` — a new
  dependency that departs from the repo's plain-prompt, rustls-lean style).
- No change to `theme use <name>` (stays the direct, scriptable, non-interactive
  setter) or to `list`/`show`/`new`.
- No theme *editing* (that's `theme new` + hand-editing / future work).
- No fuzzy search/filter — with ~6 built-ins plus a handful of files, a numbered
  menu is enough (YAGNI).

## Approach

A new `ThemeCmd::Pick` variant, implemented in `theme_cmd.rs` as a thin
interactive shell over pure, unit-tested helpers, reusing existing pieces
(`theme_files`, `preview_named`, `use_theme`, `resolvable`,
`builtin_theme_names`). The interaction core is factored over a generic
reader/writer so the full loop is unit-testable without a TTY.

### CLI

`ThemeCmd` gains a unit variant:

```rust
/// Interactively browse theme previews and set one.
Pick,
```

`theme_cmd::run` dispatches `ThemeCmd::Pick => pick(config_path, themes_dir)`.
`main.rs` is unchanged (it already routes `Command::Theme(cmd) =>
theme_cmd::run(cmd, &config_path(), &themes_dir())`).

### Interaction flow (`rustline theme pick`)

1. **TTY guard:** if `std::io::stdin().is_terminal()` is false, print a hint to
   stderr (`theme pick is interactive; use \`theme show <name>\` / \`theme use
   <name>\` non-interactively`) and exit non-zero (code 2). Mirrors the `init`
   wizard's non-TTY handling. Never writes config on this path.
2. **Build the list:** `Config::load(config_path)`; `active =
   cfg.theme.base.as_deref().unwrap_or("default")`. `picker_entries(active,
   builtin_theme_names(), &theme_files(themes_dir))` produces the ordered,
   name-unique entry list (below).
3. **Preview loop** (prompts + previews to stderr):
   ```
   Themes (* = active):
     1) default *
     2) pastel-rainbow
     3) nord
     ...
     7) my-theme  (custom)
   Preview # (number, a=all, enter=done): 3
     <ANSI preview of nord>
   Preview # (number, a=all, enter=done): a
     <ANSI preview of every theme, each labelled>
   Preview # (number, a=all, enter=done):        # blank → leave the loop
   ```
   Each non-blank entry is parsed by `parse_preview_input`; a number renders
   that theme via `preview_named` (the same LEFT/CENTER/RIGHT synthetic sample,
   with warning/error alert badges, that `theme show` uses); `a`/`all` renders
   every entry in turn; anything else prints a one-line usage hint and re-asks.
4. **Set step** (prompt to stderr):
   ```
   Set which theme? [name or #, enter=keep default]: nord
   ```
   Parsed by `parse_set_input`: blank → keep the current theme (no write, print
   `kept <active>`); a number in range → that entry's name; otherwise a typed
   name. A number/name that resolves to an entry is set via `use_theme`
   (comment-preserving `set_base` + write + `theme set to <name>`); an
   unrecognized name re-asks.

### The testable core

`run_picker` holds the whole loop over a generic reader/writer and performs **no
writes and no `process::exit`** — it returns the chosen theme name (or `None` to
keep current), so `pick` does the actual config write. This makes the entire
interaction unit-testable by feeding a byte-slice reader and inspecting the
captured writer.

```rust
fn run_picker<R: BufRead, W: Write>(
    entries: &[PickEntry],
    themes_dir: &Path,
    reader: &mut R,
    writer: &mut W,
    active: &str,
) -> Option<String>   // Some(name) to set; None to keep current
```

`pick` wraps it:

```rust
fn pick(config_path: &Path, themes_dir: &Path) {
    if !std::io::stdin().is_terminal() { /* hint + exit(2) */ }
    let cfg = Config::load(config_path);
    let active = cfg.theme.base.clone().unwrap_or_else(|| "default".into());
    let entries = picker_entries(&active, builtin_theme_names(), &theme_files(themes_dir));
    let stdin = std::io::stdin();
    let mut r = stdin.lock();
    let mut w = std::io::stderr().lock();
    match run_picker(&entries, themes_dir, &mut r, &mut w, &active) {
        Some(name) => use_theme(&name, config_path, themes_dir), // writes + prints "theme set to name"
        None => println!("kept {active}"),
    }
}
```

### Pure helpers (unit-tested)

```rust
struct PickEntry { name: String, active: bool, from_file: bool }

/// Ordered, name-UNIQUE entries: built-ins (in `builtin_theme_names()` order)
/// first, then themes-dir stems not already present. `active` is set on the
/// single entry whose name equals the active base; `from_file` marks entries
/// that come from a themes-dir file (a file shadowing a same-named built-in is
/// one entry, `from_file = true`, since resolution is file-first).
fn picker_entries(active: &str, builtins: &[&str], files: &[String]) -> Vec<PickEntry>;

enum PreviewCmd { Done, All, Preview(usize /*0-based*/), Invalid }
/// blank → Done; "a"/"all" (trimmed, case-insensitive) → All; a number in
/// `1..=n` → Preview(k-1); anything else → Invalid.
fn parse_preview_input(input: &str, n: usize) -> PreviewCmd;

enum SetCmd { Keep, Index(usize /*0-based*/), Name(String) }
/// blank → Keep; a number in `1..=n` → Index(k-1); any other non-blank →
/// Name(trimmed) (an out-of-range number falls here and is rejected by the
/// caller's entry lookup).
fn parse_set_input(input: &str, n: usize) -> SetCmd;
```

`run_picker`'s set step resolves `SetCmd::Index(i)` to `entries[i].name` and
`SetCmd::Name(s)` to `s` iff some `entry.name == s` (equivalent to `resolvable`,
but over the already-built entry set — no extra I/O); an unmatched name re-asks.

### Reuse / DRY

- `use_theme` performs the entire validated, comment-preserving set-and-write
  (incl. the `[theme]`-not-a-table guard and unparseable-config abort). `pick`
  calls it rather than re-implementing the write.
- `preview_named`, `theme_files`, `resolvable`, `builtin_theme_names` are reused
  as-is. A small private `read_line(reader)` helper reads one trimmed line.
- The `init` wizard's theme step is **not** refactored into this: its flow
  differs (pick → confirm → *return a name to seed the config*, no write) from
  `theme pick` (preview-many → *write immediately*). The genuine overlap is the
  leaf helpers, which are already shared; unifying the two loops would be a
  flags-and-branches abstraction more complex than the two thin callers (YAGNI).

## Error handling / safety

- Non-TTY → hint + exit(2), no write.
- `Config::load` totality is untouched (invariant #3).
- The write path is entirely `use_theme`, which already validates and aborts
  before writing on an unresolvable name or unparseable config — no new
  clobber surface.
- `run_picker` has no side effects (no write, no exit), so a malformed answer
  only re-asks; it can never corrupt state.

## Testing (TDD)

Unit tests in `theme_cmd.rs`:

- `picker_entries`: ordering (built-ins then files), name-uniqueness (a file
  named like a built-in yields ONE entry, `from_file = true`), `active` set on
  exactly the matching entry (and on the file entry when a file shadows the
  active built-in name), `from_file` flags.
- `parse_preview_input`: blank→Done, `a`/`A`/`all`→All, `1`/`n`→Preview(k-1),
  `0`/`n+1`/`x`→Invalid.
- `parse_set_input`: blank→Keep, in-range number→Index(k-1), name→Name, out-of-
  range number→Name.
- `run_picker` (the interaction, driven by a `Cursor<&[u8]>` reader + a `Vec<u8>`
  writer, `themes_dir` = an empty tempdir so built-in previews resolve):
  - input `"3\n\n2\n"` → previews entry 3, then (blank ends preview loop), then
    set entry 2 → returns `Some(entries[1].name)`; the writer contains entry 3's
    preview.
  - input `"\n\n"` (no preview, blank set) → returns `None` (keep current).
  - input `"a\n\nnord\n"` → All previews rendered, then name `nord` → `Some("nord")`.
  - input `"\n99\nnord\n"` → set `99` re-asks (out-of-range → unmatched name),
    then `nord` → `Some("nord")`.

Integration (`crates/rustline/tests/smoke.rs`):
- `theme_pick_non_tty_errors_and_writes_nothing`: pipe stdin (non-TTY under
  `Command`) to `rustline theme pick`; assert exit non-zero, a stderr hint
  naming `theme show`/`theme use`, and that no `config.toml` was written.
  (The interactive happy path can't be subprocess-tested — no TTY — so it is
  covered by the `run_picker` unit tests over a fake reader instead.)

## Docs to update

- `README.md`: add `rustline theme pick` to the theme-management section (a line
  in the CLI list / the "Themes" area).
- `CLAUDE.md`: the `theme_cmd.rs` module-map entry (new `pick`/`run_picker` +
  the pure `picker_entries`/`parse_preview_input`/`parse_set_input`), the CLI
  section's theme subcommands, the `ThemeCmd` variant list, and the Roadmap
  "Done" list.

## Invariants this feature depends on / must preserve

- **`Config::load` totality (#3):** unchanged — `pick` reads config, and writes
  only through the already-safe `use_theme`.
- **`theme use <name>` behavior is unchanged** — `pick` reuses `use_theme`, it
  does not alter it.
- **Theme resolution is file-first** (a themes-dir file shadows a same-named
  built-in): `picker_entries` reflects this by yielding one entry per name with
  `from_file` set when a file provides it, so what the picker sets matches what
  the renderer resolves.
