# Named themes, semantic colors, and theme-picker CLI — design

**Date:** 2026-07-21
**Status:** Approved (shipped via `/ship-it --ask`)

## Motivation

Today rustline has exactly **one** theme. Users can override individual
`[theme]` fields onto `Theme::default()`, but there is no way to *select* a
different curated look, and the only "accent" mechanism most users notice is
the two-color default `palette` (`colour31`, `colour238`). Three gaps:

1. **No named/multi-accent themes.** The renderer already cycles a
   `palette: Vec<Color>` per segment (`assign_palette`), so multi-accent is a
   *content* gap, not an engine gap — there just are no curated palettes to
   pick from, and no way to name/select one.
2. **No semantic colors.** Widgets and WASM plugins have no themeable notion of
   success / info / warning / error, so nothing can render "battery critical" in
   the theme's red or "all good" in its green.
3. **No picker/authoring workflow.** A user who likes a built-in but wants to
   tweak it has to hand-write a full `[theme]` block from scratch.

This feature adds: **six built-in themes** (incl. a flagship multi-accent
`pastel-rainbow`), **four semantic colors** wired through to built-in widgets
*and* WASM plugins, and a **`rustline theme` CLI** to list/preview/select
themes and scaffold a new tweakable theme file seeded from any existing theme.

## Design

### 1. Theme selection layering — `[theme].base`

A new scalar `base` inside the existing `[theme]` table names a theme to start
from. Resolution is three layers, most-specific last:

```
Theme::default()  →  the selected base theme  →  inline [theme] field overrides
```

```toml
[theme]
base  = "pastel-rainbow"     # a built-in name, OR a *.toml stem in the themes dir
error = { Named = "red" }    # any individual field still overrides on top
```

- **Backward-compatible:** no `base` ⇒ today's behavior (`default` + overrides).
- **One config surface:** per-field overrides keep working *on top of* a base.
- **Precedence:** a `base` name is resolved by checking the **themes dir first**
  (`$XDG_CONFIG_HOME/rustline/themes/<name>.toml`), then the built-in registry.
  A user file therefore **shadows** a same-named built-in — the whole point of
  "seed a built-in, then tweak it."
- **Total (invariant #3):** an unknown/unresolvable `base`, or an unparseable
  theme file, logs a `warn!` and falls back (default base) — it never fails the
  render.

### 2. Types — `ThemeConfig` becomes a full optional mirror of `Theme`

`ThemeConfig` (config.rs) currently covers only a subset of `Theme`'s fields
(no separators/`soft_fg`). Extend it to a **complete** optional mirror plus the
new keys, so a built-in or a hand-written file can specify *anything*:

```rust
pub struct ThemeConfig {
    // selection (only meaningful in the main config's [theme]; ignored in files)
    #[serde(default)] pub base: Option<String>,
    // existing
    #[serde(default)] pub palette: Option<Vec<Color>>,
    #[serde(default)] pub fg: Option<Color>,
    #[serde(default)] pub bar_bg: Option<Color>,
    #[serde(default)] pub win_cap_left: Option<String>,
    #[serde(default)] pub win_cap_right: Option<String>,
    #[serde(default)] pub win_current_bg: Option<Color>,
    #[serde(default)] pub win_current_fg: Option<Color>,
    #[serde(default)] pub win_inactive_bg: Option<Color>,
    #[serde(default)] pub win_inactive_fg: Option<Color>,
    // NEW: separators (previously unthemeable) — a bonus, all optional
    #[serde(default)] pub hard_left: Option<String>,
    #[serde(default)] pub hard_right: Option<String>,
    #[serde(default)] pub soft_left: Option<String>,
    #[serde(default)] pub soft_right: Option<String>,
    #[serde(default)] pub soft_fg: Option<Color>,
    // NEW: semantic colors
    #[serde(default)] pub success: Option<Color>,
    #[serde(default)] pub info: Option<Color>,
    #[serde(default)] pub warning: Option<Color>,
    #[serde(default)] pub error: Option<Color>,
}
```

Every field stays `#[serde(default)]` (invariant #3). `Theme` (render.rs) gains
the four semantic fields: `success`, `info`, `warning`, `error: Color`.

Two pure helpers on `ThemeConfig`:

```rust
/// Apply each Some field onto `theme` (skips `base`, which isn't a color).
pub fn apply_to(&self, theme: &mut Theme);
/// Produce an all-Some config mirroring `theme` (base = None). Used to
/// serialize a fully-resolved theme to a scaffold file (`theme new`).
pub fn from_theme(theme: &Theme) -> ThemeConfig;
```

Resolution API on `Config` (core owns merging; the binary owns themes-dir I/O):

```rust
/// Apply the inline [theme] overrides on top of an already-resolved `base`.
pub fn to_theme_over(&self, base: Theme) -> Theme {
    let mut theme = base;
    self.theme.apply_to(&mut theme);
    theme
}
/// Convenience: resolve using BUILT-IN themes only (no themes-dir lookup).
/// Used by core tests and any caller without a themes dir.
pub fn to_theme(&self) -> Theme {
    let base = self.theme.base.as_deref()
        .and_then(builtin_theme)
        .unwrap_or_default();
    self.to_theme_over(base)
}
```

The existing `Config::to_theme()` keeps its signature/name (many callers/tests
use it) but now honors a built-in `base`. The binary uses `to_theme_over` after
its file-first base resolution (§6).

### 3. Semantic colors reach widgets AND plugins via `Context`

Widgets can't see `Theme` (by design), and WASM guests only receive `Context`
as JSON. So the colors a widget/plugin may need are surfaced on `Context`.

Add a serde struct to **`rustline-abi`** (next to `Color`, so guests share it):

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeColors {
    pub fg: Color,
    pub bar_bg: Color,
    pub success: Color,
    pub info: Color,
    pub warning: Color,
    pub error: Color,
}
impl Default for ThemeColors { /* matches Theme::default(): 255/234/35/39/214/196 */ }
```

`Context` (core) gains `#[serde(default)] pub colors: ThemeColors`, **populated
at Context-build time from the resolved theme** (build_context.rs). Rationale:

- **Invariant #1 preserved:** widgets still read only from `Context`. Theme-derived
  data flows *into* `Context` at build time; nothing reads the environment mid-render.
- **Invariant #2 preserved:** `Context` stays fully serde. The field is
  `#[serde(default)]`, so an older guest that omits it still deserializes, and
  the extra JSON key is harmless to guests that ignore it. The `weather` example
  gains a one-line demonstration of reading `context.colors.info`.

`fg`/`bar_bg` are included (not just the four semantics) because the threshold
treatment (§4) needs a guaranteed-contrast text color, and plugins benefit from
knowing the bar's fg/bg to blend consistently.

### 4. Threshold-aware built-in widgets

`cpu`, `memory`, `battery`, `loadavg` color a crossed-threshold cell as an
**inverse alert badge**: segment `bg = <semantic color>`, `fg = colors.bar_bg`,
`bold = true`.

**Why `fg = bar_bg` and not `theme.fg`?** `bar_bg` is dark in every theme and
every semantic color is brighter than it, so dark-text-on-bright-badge has
**guaranteed contrast in all six themes** — regardless of whether the theme's
normal `fg` is light or dark. (Coloring text with the semantic color instead
would depend on contrast against the per-segment palette accent, which varies.)

Widgets set this by writing an explicit `Style` on their segment; `assign_palette`
already leaves segments with an explicit `bg` untouched, so no renderer change is
needed. New per-widget config knobs, **`0` (or `0.0`) = that tier disabled**:

| Widget | warn field (default) | crit field (default) | Comparison | Notes |
|--------|----------------------|----------------------|------------|-------|
| `cpu` | `warn_percent` (80) | `crit_percent` (95) | `cpu.percent ≥ thr` | crit wins over warn |
| `memory` | `warn_percent` (80) | `crit_percent` (92) | `used% ≥ thr` | |
| `battery` | `warn_percent` (20) | `crit_percent` (10) | `percent ≤ thr` **and discharging** | charging/full/unknown ⇒ no alert |
| `loadavg` | `warn_load` (0.0=off) | `crit_load` (0.0=off) | `load1 ≥ thr` | default off: absolute load needs core count |

Precedence within a widget: **crit (error) beats warn (warning)**; below both ⇒
normal palette. The chosen semantic color is `colors.warning` or `colors.error`.
Threshold evaluation is a small pure helper per widget (or a shared
`widgets::alert` helper) so it is unit-testable at the boundaries.

Interaction with existing placeholders: the alert style applies to the **whole
rendered segment** (icon + `{bar}` + text all take the badge colors), which is
the intended "this widget is alarming" read. `down_format` (no reading) is never
alerted. A `battery`/`cpu`/etc. with alerts disabled (all thresholds 0) renders
**byte-identically to today** (load-bearing — see Invariants).

### 5. Six built-in themes

Built-ins live in a new `crates/rustline-core/src/themes.rs`:

```rust
pub fn builtin_theme(name: &str) -> Option<Theme>;      // None if unknown
pub fn builtin_theme_names() -> &'static [&'static str]; // for `theme list`
```

Each returns a **complete** `Theme` (all fields incl. semantics). `default` is
`Theme::default()` (with the new semantic fields filled in). Every non-`default`
theme is **multi-accent** (palette length ≥ 4). Concrete starting values (RGB
truecolor for the curated schemes; the plan includes a visual-check step to
tweak any low-contrast pairing via `rustline theme show`):

**Contrast rule each theme obeys:** `fg` contrasts with every palette accent
(light `fg` for saturated/dark accents; dark `fg` for pastel/light accents).
The threshold badge always uses `bar_bg` as its text color, so semantic colors
only need to be *brighter than `bar_bg`* (trivially true).

- **`default`** — unchanged existing values; semantics
  `success=colour35, info=colour39, warning=colour214, error=colour196` (light `fg`).
- **`pastel-rainbow`** (flagship, dark `fg` on pastel cells):
  `bar_bg=#2a2a36, fg=#2b2b3a, soft_fg=#7a7a8a`,
  palette `[#f4a6b8, #f6c7a9, #f5e6a3, #b8e6c4, #a9d3f0, #d0bdf0]`,
  `win_current_bg=#c3a9ee/fg=#2b2b3a`, `win_inactive_bg=#cfcfda/fg=#55556a`,
  semantics `success=#a8e0b0, info=#a9d3f0, warning=#f3d98b, error=#f2a1a1`.
- **`nord`** (light `fg`): `bar_bg=#2e3440, fg=#d8dee9, soft_fg=#4c566a`,
  palette `[#5e81ac, #81a1c1, #a3be8c, #b48ead, #d08770]`,
  `win_current_bg=#88c0d0/fg=#2e3440`, `win_inactive_bg=#3b4252/fg=#d8dee9`,
  semantics `success=#a3be8c, info=#88c0d0, warning=#ebcb8b, error=#bf616a`.
- **`gruvbox`** (dark, light `fg`): `bar_bg=#282828, fg=#ebdbb2, soft_fg=#665c54`,
  palette `[#d79921, #98971a, #458588, #b16286, #d65d0e]`,
  `win_current_bg=#fabd2f/fg=#282828`, `win_inactive_bg=#3c3836/fg=#ebdbb2`,
  semantics `success=#b8bb26, info=#83a598, warning=#fabd2f, error=#fb4934`.
- **`catppuccin-mocha`** (pastel-on-dark, dark `fg` — the accents are light
  pastels, so segment text must be dark): `bar_bg=#1e1e2e, fg=#11111b,
  soft_fg=#585b70`,
  palette `[#89b4fa, #f5c2e7, #a6e3a1, #f9e2af, #fab387]`,
  `win_current_bg=#cba6f7/fg=#11111b`, `win_inactive_bg=#313244/fg=#cdd6f4`,
  semantics `success=#a6e3a1, info=#89dceb, warning=#f9e2af, error=#f38ba8`.
- **`tokyo-night`** (bright accents, dark `fg`): `bar_bg=#1a1b26, fg=#16161e`,
  `soft_fg=#565f89`,
  palette `[#7aa2f7, #bb9af7, #7dcfff, #9ece6a, #e0af68]`,
  `win_current_bg=#7aa2f7/fg=#16161e`, `win_inactive_bg=#292e42/fg=#c0caf5`,
  semantics `success=#9ece6a, info=#7dcfff, warning=#e0af68, error=#f7768e`.

(`catppuccin-mocha` and `tokyo-night` use a **dark `fg`** because their accents
are light/medium — dark text reads on them. `default`/`nord`/`gruvbox` use a
light `fg`. This is the contrast rule above, applied.)

### 6. CLI — `rustline theme <sub>`

New `theme_cmd.rs` in the binary, mirroring `plugin_cmd.rs` (`toml_edit`
mutations preserve comments/formatting). `themes_dir()` in main.rs resolves
`$XDG_CONFIG_HOME/rustline/themes` (fallback `~/.config/rustline/themes`),
parallel to `config_path()`.

```
rustline theme list
rustline theme show <name>
rustline theme use  <name>
rustline theme new  <name> [--from <seed>] [--force]
```

- **`list`** — prints every built-in and every `*.toml` in the themes dir
  (by stem), one per line, annotated with its source (`built-in` / the file
  path) and marking (a) the **active** theme (`cfg.theme.base`, or `default`
  when unset) with `*`, and (b) a built-in **shadowed** by a same-named file.
- **`show <name>`** — resolves `<name>` (themes-dir file first, then built-in;
  error if neither), builds a **representative synthetic `Context`** (sample
  host/path/loadavg, plus `cpu=96%`, `memory=85%`, `battery=15% discharging`
  so the preview exercises warning+error badges), renders the **default layout**
  (left/center/right) with that theme, and prints it as ANSI via the existing
  `tmux_to_ansi` (same path as `render --preview`). Self-contained: no live tmux
  or plugin cold-start.
- **`use <name>`** — validates `<name>` resolves (file or built-in); on failure,
  prints an error listing available names and exits non-zero **without writing**.
  On success, sets `[theme].base = "<name>"` in the config file via `toml_edit`
  (creating `[theme]` if absent), preserving all other comments/formatting.
  Reuses the read/parse/refuse-to-clobber-invalid-TOML guard from `plugin_cmd`.
- **`new <name> [--from <seed>] [--force]`** — `<name>` must be a bare filename
  (no `/`, `\`, or `..`; non-empty), else error. Resolves `<seed>` (default
  `"default"`; file-first then built-in) to a full `Theme`, converts via
  `ThemeConfig::from_theme` to an all-Some config, and writes
  `<themes_dir>/<name>.toml` (creating the dir). The file is prefixed with a
  comment header (`# rustline theme "<name>" (seeded from "<seed>")` +
  `# select with: rustline theme use <name>`). Refuses to overwrite an existing
  file unless `--force`. This is the "start from something close, then tweak"
  path: the resulting file is a **complete** set of fields the user can edit.

Wire `Command::Theme` into `cli.rs` (a subcommand group like `Plugin`) and
`main.rs` dispatch (`Command::Theme(cmd) => theme_cmd::run(cmd, &config_path(), &themes_dir())`).

### 7. Theme file format

A theme file is a **serialized `ThemeConfig`** (partial allowed; a hand-edited
file may set just a few fields, the rest coming from `Theme::default()` when it
is used as a `base`). `theme new` writes an all-Some (complete) one. `base`
inside a theme file is ignored (no recursive bases — YAGNI). Colors serialize in
the documented enum form (`{ Rgb = [..] }`, `{ Indexed = N }`, `{ Named = ".." }`),
matching the existing `[theme]` docs. A round-trip test pins that a written file
re-parses to the same resolved `Theme`.

## Invariants this feature depends on

- **#1 (Context is the sole render input):** semantic colors are copied into
  `Context.colors` at build time; widgets never read `Theme` or the env
  mid-render. Pinned by a widget test that alerts using `ctx.colors`, and by
  build_context populating `colors` from the theme.
- **#2 (Segment/Context/Style/Color stay serde):** `ThemeColors` is serde and
  added to `Context` with `#[serde(default)]`. **Load-bearing test (do not skip
  on the "it's obviously still serde" invariant):** a `Context` JSON round-trip
  test, *and* a back-compat test that a JSON payload **omitting** `colors`
  deserializes to `ThemeColors::default()` — this is the exact seam a WASM guest
  crosses, and the default is what an older guest/newer-host mix relies on.
- **#3 (`Config::load` is total):** all new `ThemeConfig`/opts fields are
  `#[serde(default)]`; an unknown `base` or unparseable theme file ⇒ `warn!` +
  fallback. Covered by malformed-theme-file and unknown-base fallback tests.
- **#5 (`render_region` order):** unchanged; alert segments are ordinary
  segments with an explicit style.
- **#6 (`loadavg` is `Option`; widgets degrade):** alerting reads only from
  present values; a `None` reading never alerts.
- **Byte-identical no-alert output (new, load-bearing):** with all thresholds
  disabled (the default for `loadavg`; and for cpu/memory/battery when the
  reading is below every threshold), each widget renders exactly as today.
  Pinned by characterization tests per widget (a below-threshold `Context`
  produces the pre-feature segment/style).
- **Producers that must survive the `to_theme`/`apply_to` funnel:** every
  existing `[theme]` override key (palette, fg, bar_bg, the six `win_*`) plus
  the new separators and semantics must still map through `apply_to`. Covered by
  extending the existing `to_theme_maps_window_pill_overrides` test to assert the
  new keys too, and a test that a built-in's every field survives
  `from_theme` → serialize → parse → `apply_to`.

## Testing (TDD)

**rustline-abi:** `ThemeColors::default()` values; serde round-trip; a struct
omitting it (via `#[serde(default)]` on the containing field) defaults.

**rustline-core:**
- `themes.rs`: `builtin_theme` returns `Some` for each of the six names and
  `None` for unknown; `builtin_theme_names()` lists exactly the six; each
  non-`default` theme has `palette.len() ≥ 4`; every theme has all fields set
  (no accidental `Theme::default()` leakage — e.g. assert `pastel-rainbow`'s
  `bar_bg` ≠ default's).
- `config.rs`: `to_theme` layering — no base (=default), builtin base
  (`base="nord"` ⇒ nord values), base + inline override (override wins);
  unknown builtin base ⇒ default (total); parse a `[theme]` with `base` +
  separators + semantics; `apply_to` sets only Some fields; `from_theme` is
  all-Some and round-trips (`from_theme(t)` applied onto default == `t`).
- widgets: for each of cpu/memory/battery/loadavg — below-threshold ⇒ normal
  (characterization, byte-identical style); at/over `warn` ⇒ `bg=warning,
  fg=bar_bg,bold`; at/over `crit` ⇒ `error`; `0`/`0.0` disables a tier;
  battery only alerts while **discharging**; crit beats warn at the boundary.

**rustline-wasm / weather:** the guest reading `context.colors.info` (host-side
pure unit test of the render helper; the plugin still degrades safely).

**rustline (bin):**
- `theme use` writes `[theme].base` preserving a surrounding comment
  (`toml_edit`); refuses an unknown name without writing; refuses to overwrite
  invalid TOML (reuses the `plugin_cmd` guard, add a test).
- `theme new` writes `<dir>/<name>.toml` seeded from a built-in and from
  `default`; refuses overwrite without `--force`, succeeds with it; rejects a
  name containing `/` or `..`; the written file re-parses as a `ThemeConfig`
  whose `apply_to`(default) equals the seed theme.
- `theme show` produces non-empty ANSI (contains an ESC `\x1b[` sequence) for a
  built-in and includes the error-badge color for the crafted critical `Context`.
- `themes_dir()` honors `$XDG_CONFIG_HOME` and the `~/.config` fallback.
- smoke: `render right` with the default (alerts-off) config is unchanged;
  `render right` with a low-battery/high-cpu `Context` shows badge markup.

## Documentation

- **CLAUDE.md:** new `themes.rs` in the core module map; `ThemeColors` in
  rustline-abi; `Context.colors`; the extended `ThemeConfig`; the four
  threshold-aware widgets + their config knobs; the `rustline theme` CLI group
  and `themes_dir`; a **Themes** subsection in Config (built-in names, `base`
  layering, precedence, semantic colors, threshold knobs); a roadmap "Done"
  entry. Per the standing note, **update the widget/feature lists in BOTH
  CLAUDE.md and README.md.**
- **README.md:** a "Themes" section (list the six, `theme list/show/use/new`
  with examples, `[theme].base`, semantic colors, per-widget thresholds); note
  the curated themes use **truecolor** (require a truecolor-capable terminal +
  tmux `Tc`/`RGB`).

## Out of scope (YAGNI)

- An interactive TUI picker (`theme pick`) — subcommands + `show` preview cover it.
- Auto-installing built-ins as files on first run — built-ins are baked in.
- Recursive `base` chains in theme files — files are flat.
- Per-theme separator glyph *sets* beyond the single `hard/soft` pair already
  modeled (the new optional separator overrides are enough).
- Luminance/auto-contrast computation — the `fg = bar_bg` badge rule and the
  per-theme contrast rule avoid needing it.
- Threshold coloring of non-numeric widgets (hostname, cwd, etc.).
