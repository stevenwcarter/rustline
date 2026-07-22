# rustline `init` onboarding wizard — design

## Problem

Setting up rustline on a new machine is awkward. Today `rustline init` prints a
single fixed tmux config block to stdout; the user is expected to
`rustline init >> ~/.tmux.conf`, then hand-edit `~/.config/rustline/config.toml`
to pick a theme, add opt-in widgets (battery, IP), and set up shortened
`alt_format` click-toggle views. Anyone who wants the author's own **two-line**
status layout (window list on its own line above status-left/right) has to copy
a large, gnarly `status-format[0]` string by hand. There is no on-ramp — the
good setup is tribal knowledge living in one person's dotfiles.

## Goal

Turn `rustline init` into an interactive **onboarding wizard** that, from a
handful of questions, writes a working two-file setup:

1. **`~/.config/rustline/config.toml`** — theme, a tailored widget layout, and a
   curated set of shortened `alt_format`s (the author's proven defaults), seeded
   from an embedded starter template.
2. **`~/.tmux.conf`** — the tmux wiring block (one- or two-line), enclosed in an
   idempotent marker block so re-running is safe, with the file backed up first.

A `--defaults` flag runs the same flow non-interactively with recommended
answers. A `--print` flag preserves today's exact behavior (raw one-line block
to stdout, writes nothing) for scripting.

## Non-goals

- No TUI framework / full-screen UI. Plain line-oriented prompts on stderr.
- No auto-running of `tmux source-file` (we instruct; we don't execute tmux).
- No weather/other WASM-plugin setup (requires a `.wasm` build; out of scope —
  the starter leaves a commented pointer).
- No change to any widget's rendered output for existing users. The code
  `Default` for every widget's `alt_format` **stays `""`** (load-bearing — see
  Invariants). The shortened forms live only in the starter template.

## Approach

`rustline init` becomes the wizard. Backward-compat is preserved by two flags:

- `rustline init` (interactive, stdin is a TTY) → run the wizard, write both files.
- `rustline init --defaults` → non-interactive; recommended answers; write both files.
- `rustline init --print` → today's behavior exactly: raw one-line tmux block to
  stdout, write nothing.
- `rustline init` with **stdin not a TTY** and no `--defaults`/`--print` → error
  with a hint to pass `--defaults` or `--print`. We never silently write files in
  a non-interactive context, and we never mix prompts with a redirected stdout.

Rationale for changing `init` rather than adding a `setup` verb: the motivation
is smoothing *first run*, and new users already reach for `init`. Hiding the good
path behind a second command undercuts the goal. `--print` loses nothing for
scripts.

### Wizard questions

Asked in order; each has a recommended default (used by `--defaults` and shown as
the bracketed default in interactive prompts). Answers collect into an
`InitAnswers` struct.

1. **Theme** (required). Menu of built-in theme names (`builtin_theme_names()`)
   plus any `themes-dir` `*.toml` stems, each shown with its ANSI preview
   (reusing the `preview_theme_ansi` machinery from `theme_cmd`). Selection →
   `[theme].base`. Default: `default`.
2. **Status lines**: one-line or two-line. Default: **one-line**.
3. **Mouse / click-to-toggle**: enable `set -g mouse on` so the shortened
   `alt_format` toggles work when clicked. Default: **yes**. (If no, the block
   still carries today's hint comment.)
4. **Machine-type widgets** (three yes/no):
   - *Laptop — show battery?* → adds `battery` to the right layout. Interactive
     default pre-filled from `battery::read_battery().is_some()`; `--defaults` = no.
   - *On a Tailscale network — show Tailscale IP?* → adds `tailscale_ip` to the
     left layout. Default: no.
   - *Show LAN IP?* → adds `lan_ip` to the left layout. Default: no.
5. **Clock style**: preset menu mapping to a `datetime` `format` + `alt_format`:
   | Preset | format | alt_format |
   |---|---|---|
   | 24h (default) | `%a %Y-%m-%d %H:%M` | `%m-%d %H:%M` |
   | 24h + seconds | `%a %Y-%m-%d %H:%M:%S` | `%m-%d %H:%M:%S` |
   | 12h | `%a %Y-%m-%d %I:%M %p` | `%m-%d %I:%M %p` |
   | 12h + seconds | `%a %Y-%m-%d %I:%M:%S %p` | `%m-%d %I:%M:%S %p` |
6. **Refresh interval**: `status-interval` seconds. Default: **1** (offer 1 or 5).

### Architecture

A thin I/O shell over pure, unit-tested functions.

**`crates/rustline/src/init.rs`** (new) — the wizard shell and its helpers:

- `struct InitAnswers { theme: String, two_line: bool, mouse: bool, battery: bool,
  tailscale: bool, lan_ip: bool, clock: ClockStyle, interval: u32 }` and
  `enum ClockStyle` (the four presets above, each `-> (format, alt_format)`).
- `fn defaults(battery_detected: bool) -> InitAnswers` — the recommended answer set.
- `fn run(args, config_path, themes_dir, tmux_conf_path)` — the entry point:
  decides interactive vs `--defaults` vs `--print` vs non-TTY-error, gathers
  answers (interactive path prompts on stderr / reads stdin), then applies them.
- Small pure parsers for the prompt layer, unit-tested independently of I/O:
  - `fn parse_menu_choice(input: &str, n: usize, default: usize) -> Option<usize>`
    (blank → default; out-of-range/garbage → `None` so the caller re-asks).
  - `fn parse_yes_no(input: &str, default: bool) -> bool` (blank → default;
    `y*`/`n*` case-insensitive).
- `fn starter_config_toml(answers: &InitAnswers) -> String` — parse the embedded
  template with `toml_edit`, then mutate per answers (below), serialize.
- `fn write_config(answers, existing_path)` — non-destructive merge (below) and write.
- `fn upsert_tmux_block(existing: &str, block: &str) -> String` — idempotent
  marker-block insertion/replacement (below).

**`crates/rustline/assets/starter-config.toml`** (new, embedded via
`include_str!`) — the single reviewable source of the recommended defaults. Holds
`[layout]`, `[theme]`, and every seeded `[widgets.*]` section with the author's
shortened `alt_format`s. Content:

```toml
# rustline config — generated by `rustline init`.
# Edit freely; re-running `rustline init` only ADDS sections you don't have and
# (re)sets [theme].base — it never overwrites options you've changed.
# Weather and other WASM plugins are opt-in: see the project README.

[layout]
left = ["pane_id", "hostname"]
center = ["windows"]
right = ["cwd", "cpu", "memory", "loadavg", "datetime"]

[theme]
base = "default"

[widgets.cwd]
abbreviate_home = true

[widgets.datetime]
format = "%a %Y-%m-%d %H:%M"
alt_format = "%m-%d %H:%M"

[widgets.cpu]
format = "{icon} {bar} {percent}%"
alt_format = "{icon} {percent}%"
bar_width = 6

[widgets.memory]
format = "{icon} {used}/{total}"
alt_format = "{icon} {used}"

[widgets.loadavg]
alt_format = "LD {load1:.1}"

[widgets.battery]
format = "{icon} {percent}%"
alt_format = "{icon}"

[widgets.lan_ip]
format = "󰌗 {ip}"
alt_format = "󰌗"

[widgets.tailscale_ip]
format = "󰖂 {ip}"
alt_format = "󰖂"
down_format = "TS off"
```

**`starter_config_toml` mutations** (via `toml_edit`, preserving comments):

- Set `[theme].base` to `answers.theme`.
- Set `[layout].left` = `["pane_id","hostname"]` + `["lan_ip"]?` + `["tailscale_ip"]?`.
- Set `[layout].right` = `["cwd","cpu","memory"]` + `["battery"]?` + `["loadavg","datetime"]`.
- Set `[widgets.datetime].format`/`alt_format` from `answers.clock`.
- **Prune** the option sections for any *optional* widget not selected
  (`battery`, `lan_ip`, `tailscale_ip`) so the generated file matches the layout.
  Always keep `cwd`, `datetime`, `cpu`, `memory`, `loadavg` (they're in the
  default layout regardless).

**Config write — non-destructive** (`write_config`):

- If no config file exists: write the full generated starter.
- If one exists: load with `toml_edit` and merge **non-destructively**:
  - `[theme].base` is **always (re)set** to the chosen theme (the user actively
    picked it — mirrors `rustline theme use`; reuse `theme_cmd::set_base`).
  - `[layout]` is written **only if absent**.
  - Each `[widgets.<name>]` from the generated starter is written **only if that
    table is absent** in the existing config.
  - Back up the existing file to `config.toml.rustline.bak` before writing.

**tmux block — `init_block` extended.** Change
`tmux_conf::init_block(bar_bg, fg)` to take an options struct:

```rust
pub struct InitBlockOpts { pub bar_bg, pub fg, pub two_line, pub mouse, pub interval }
pub fn init_block(opts: &InitBlockOpts) -> String
```

- `interval` replaces the hardcoded `status-interval 1`.
- `mouse` true → emit `set -g mouse on` (plus keep the existing explanatory
  comment); false → keep only the comment (today's behavior).
- `two_line` false → today's output (status-justify + status-left/right).
- `two_line` true → additionally emit `set -g status 2` and the
  `status-format[0]`/`status-format[1]` overrides, templated **verbatim** from
  the author's proven `~/.tmux.conf` (the centered per-window list on top,
  status-left/right on the bottom). `status-left`/`status-right`/
  `window-status-format`/hooks/mouse-binding are shared by both modes.
- Injection-safety invariant #4 is unchanged: every interpolated tmux var stays
  `#{q:...}` + `--flag=` form. The two-line `status-format` strings contain no
  `#(...)` shell calls themselves (they reference `#{T:status-left}` etc., which
  resolve to the already-safe `status-left`/`status-right` we set).

**tmux.conf write — idempotent marker block** (`upsert_tmux_block`):

- Markers: `# >>> rustline >>>` … `# <<< rustline <<<`.
- If both markers are present: replace the region between them with the new
  block. If absent: append (preceded by a blank line).
- Back up `~/.tmux.conf` to `~/.tmux.conf.rustline.bak` before writing.
- Print a next-step hint: `tmux source-file ~/.tmux.conf` (or restart tmux).

### CLI surface

`Command::Init` gains `InitArgs`:

```rust
pub struct InitArgs {
    /// Non-interactive: use recommended defaults, write both files.
    #[arg(long)] pub defaults: bool,
    /// Print the raw tmux block to stdout and write nothing (legacy behavior).
    #[arg(long)] pub print: bool,
}
```

`main.rs` dispatches `Command::Init(args) => init::run(&args, &config_path(),
&themes_dir(), &tmux_conf_path())`, where `tmux_conf_path()` resolves
`$HOME/.tmux.conf` (the target file; the wizard prints which path it writes).
The `--print` path calls the extended `init_block` with one-line defaults and
prints to stdout (byte-identical to today's default block).

**Flag precedence:** `--print` wins over `--defaults` if both are given (print,
write nothing) — `--print` is the "just show me, touch nothing" escape hatch.

## Error handling / safety

- `Config::load` totality (invariant #3) is untouched — the wizard writes config,
  it doesn't change how config is read.
- All file writes are best-effort with clear stderr errors; a failed backup
  aborts before the write (never lose the user's tmux.conf).
- Non-TTY without `--defaults`/`--print` → exit non-zero with guidance, no writes.
- The wizard never runs tmux; it only writes files and prints next steps.

## Testing (TDD)

Pure functions, unit-tested in `init.rs` / `tmux_conf.rs`:

- `parse_menu_choice` / `parse_yes_no`: blanks→default, out-of-range→None/re-ask,
  case-insensitive y/n.
- `ClockStyle` → `(format, alt_format)` mapping for all four presets.
- `starter_config_toml`: parses to a valid `Config`; `[theme].base`, layout
  arrays (with/without each optional widget), and datetime format/alt reflect
  the answers; unselected optional widget sections are pruned; the shortened
  `alt_format`s are present for selected widgets.
- `write_config` non-destructive merge: existing `[widgets.cpu]` is preserved;
  `[theme].base` is overwritten to the chosen theme; a missing widget section is
  added; a backup file is produced.
- `upsert_tmux_block`: append when no markers; replace-in-place when markers
  present; **idempotent** (running twice yields identical output).
- `init_block(opts)`: two-line emits `status 2` + `status-format[0]`/`[1]`;
  one-line does not; `mouse` toggles `set -g mouse on`; `interval` is honored;
  injection-safety assertions (`#{q:...}`, `--flag=`) hold in both modes; the
  `--print`/one-line/mouse-off/interval-1 output stays byte-identical to the
  pre-change `init_block` default (characterization test).

Integration (`crates/rustline/tests/smoke.rs`): `rustline init --print` prints a
block containing `#(rustline render left` and writes nothing; `rustline init
--defaults` with `HOME`/`XDG_CONFIG_HOME` pointed at a tempdir writes a
parseable `config.toml` and a `~/.tmux.conf` containing the marker block, and a
second `--defaults` run leaves the tmux.conf marker region unchanged (idempotent)
while preserving any user edits outside the markers.

## Invariants this feature depends on / must preserve

- **Widget `alt_format` code defaults stay `""`.** The shortened forms live only
  in `assets/starter-config.toml`. This keeps existing users' output and the
  byte-identical `alt_format` tests valid (per the project's "no test skipped by
  invariant" discipline, the characterization test on the one-line block pins
  this at the tmux-block seam).
- **Invariant #4 (init injection-safety)** holds for both one- and two-line
  blocks: `#{q:...}` + `--flag=` form for every interpolated var.
- **Invariant #7 (click-toggle name identity)**: the seeded `alt_format`s use the
  real widget names, so click ranges keep working end-to-end.
- **`--print` output is byte-identical** to today's `rustline init` default, so
  existing scripts/docs migrated to `--print` see no change.

## Docs to update

- `README.md` "Enable in tmux": recommend `rustline init` (wizard); document
  `--defaults` and `--print`; note the two-line option and the marker block.
- `CLAUDE.md`: `init` is now a wizard; new `init.rs` module and
  `assets/starter-config.toml`; extended `init_block(&InitBlockOpts)`; the CLI
  section's `rustline init` entry.
