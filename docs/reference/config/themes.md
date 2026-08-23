# Config — themes and the window pill

> **Read when:** changing theme layering/resolution, adding a built-in theme, touching the six `win_*` pill fields, or the four semantic colors used by threshold badges.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

**Window pill (`[theme]`):** the window list renders as a rounded pill (see
Render pipeline). Six optional `[theme]` fields override the defaults — active
pill in the accent color, inactive pills gray, rounded caps:

```toml
[theme]
win_current_bg = { Indexed = 31 }    # active fill (accent); default colour31
win_current_fg = { Indexed = 255 }   # active text (bold); default colour255
win_inactive_bg = { Indexed = 236 }  # inactive fill (gray); default colour236
win_inactive_fg = { Indexed = 250 }  # inactive text; default colour250
win_cap_left = ""                    # rounded left cap; default U+E0B6
win_cap_right = ""                   # rounded right cap; default U+E0B4
```

Active is always bold (intrinsic, not a field). Colors are `Color` enums
(`{ Indexed = N }`, `{ Named = "cyan" }`, or `{ Rgb = [r,g,b] }`); caps are
strings. All `#[serde(default)]`, so they stay covered by invariant #3.

**Themes:** `[theme].base` names a starting theme; resolution layers
`Theme::default()` → the selected base → the inline `[theme]` field overrides
above (`win_*`, `palette`, `fg`, `bar_bg`, the separators, the semantics
below) on top:

```toml
[theme]
base  = "nord"                # a built-in name, or a *.toml stem in the themes dir
error = { Named = "red" }     # per-field overrides still apply on top of base
```

Seven built-ins ship in `rustline-core::themes` (`builtin_theme_names()`):
`default`, `pastel-rainbow` (the flagship multi-accent), `nord`, `gruvbox`,
`catppuccin-mocha`, `tokyo-night`, `dracula`. Every theme but `default` is multi-accent
(`palette.len() >= 4`) and uses **truecolor** (`Color::Rgb`) — a
truecolor-capable terminal and tmux `RGB`/`Tc` are required to see the exact
colors (see README). `base` is resolved **themes-dir file first**
(`$XDG_CONFIG_HOME/rustline/themes/<name>.toml`, fallback
`~/.config/rustline/themes`), then the built-in registry — a user file
**shadows** a same-named built-in. An unknown/unresolvable `base`, or an
unparseable theme file, `warn!`s and falls back to `default` (invariant #3).

Every theme also sets four **semantic colors** — `success`/`info`/`warning`/
`error` — which reach both built-in widgets and WASM plugins via
`Context.colors: ThemeColors` (see `context.rs` above), not `Theme` directly.
The five threshold-aware widgets (`cpu`, `memory`, `battery`, `loadavg`, `disk` — see
their config blocks above for each widget's `warn_*`/`crit_*` field and
default) use them for an inverse alert badge (`bg`=semantic, `fg`=`bar_bg`,
bold) when a reading crosses a threshold; `0` (or `0.0`) disables a tier, and
a widget with every tier disabled or every reading below threshold renders
byte-identically to before this feature.

Manage themes from the command line (`rustline theme list|show|use|new|pick` —
see CLI above) instead of hand-writing `[theme]`/theme files. See the
[design spec](../../superpowers/specs/2026-07-21-rustline-themes-theme-picker-design.md)
for the full layering rules, the six themes' exact color values, and the
threshold-badge contrast rationale.

