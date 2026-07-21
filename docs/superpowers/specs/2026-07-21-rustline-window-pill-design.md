# Rustline window-list rounded pill — design

## Summary

Change rustline's built-in window-list rendering from a **blue, pointed-cap**
current-window pill (with plain-text inactive windows) to a **themeable
rounded-cap pill for every window**: the active window uses rustline's accent
color (cyan/blue `colour31`) with white bold text, inactive windows are dark-gray
pills (`colour236`) with light-gray text. The rounded caps (`` U+E0B6 / ``
U+E0B4) and all four pill colors become `Theme` fields, overridable via the
`[theme]` table in `config.toml`.

This is the new **default** window rendering in rustline core, so any front-end
(the CLI today, a daemon later) and any zero-config user gets the pill.

## Motivation

The user wants the window list to render as a gray rounded pill, matching a
rounded-cap powerline look they previously hand-rolled in tmux config. Doing it
in rustline core (rather than per-user tmux config) makes it the shared default
and keeps the window list flowing through `#(rustline render window …)`.

## Current behavior (what we're replacing)

- `widgets/windows.rs` (`Windows` widget) emits one `Segment` with text
  `"{index}{flags} {name}"`. Current window: `bg = Color::Indexed(31)`,
  `bold = true`. Inactive window: `Style::default()` (no bg).
- `assemble.rs::render_window` resolves the `windows` widget and calls the
  generic `render_region(Direction::Left, …)`.
- `render_region` draws the current-window pill using the **pointed** hard
  separators (`theme.hard_left` = ``, `theme.hard_right` = ``), colored
  `fg=bar_bg, bg=pill` at the edges. Inactive windows (bg == bar_bg) get no caps
  — plain text.

Key constraint: `render_region`'s pointed separators are *also* used for the
left/right status regions, so we cannot change the global `hard_*` glyphs to get
rounded window caps. And a rounded cap is colored the **opposite** way from a
pointed separator (`fg=pill, bg=bar_bg`), so it is a genuinely different render
construct, not a glyph swap.

## Design

### 1. Theme fields (`rustline-core/src/render.rs`)

Add six fields to `Theme` with these defaults:

| Field | Type | Default | Meaning |
|---|---|---|---|
| `win_cap_left` | `String` | `"\u{e0b6}"` (``) | rounded left cap glyph |
| `win_cap_right` | `String` | `"\u{e0b4}"` (``) | rounded right cap glyph |
| `win_current_bg` | `Color` | `Color::Indexed(31)` | active pill fill (accent) |
| `win_current_fg` | `Color` | `Color::Indexed(255)` | active text (white) |
| `win_inactive_bg` | `Color` | `Color::Indexed(236)` | inactive pill fill (dark gray) |
| `win_inactive_fg` | `Color` | `Color::Indexed(250)` | inactive text (light gray) |

The active pill stays **bold**; the inactive pill is not bold. Boldness is not a
theme field (it is intrinsic to active/inactive), matching how the widget
already encodes emphasis.

### 2. Pill renderer (`rustline-core/src/render.rs`)

Add a dedicated function, e.g.:

```rust
pub fn render_window_pill(text: &str, is_current: bool, theme: &Theme) -> String
```

It returns a self-contained pill:

```
#[fg=<pill>,bg=<bar_bg>]<cap_left>#[fg=<text_fg>,bg=<pill>{,bold}] <text> #[fg=<pill>,bg=<bar_bg>]<cap_right>#[default]
```

where, for `is_current`, `pill = win_current_bg`, `text_fg = win_current_fg`,
bold is added; otherwise `pill = win_inactive_bg`, `text_fg = win_inactive_fg`,
no bold. `bar_bg = theme.bar_bg`. The leading/trailing content space (`` <text>
``) matches `render_region`'s ` {} ` spacing.

Empty `text` still renders a pill wrapper; callers avoid that by not calling the
renderer when there is no window (see below).

### 3. `render_window` (`rustline-core/src/assemble.rs`)

`render_window` gains ownership of pill styling (it has both `ctx` and `theme`;
the `Widget` trait only sees `Context`, so colors must be applied here to be
themeable):

```rust
pub fn render_window(ctx: &Context, registry: &Registry, theme: &Theme) -> String {
    let widgets = registry.resolve(&["windows".to_string()]);
    let segments: Vec<Segment> = widgets
        .iter()
        .flat_map(|w| render_guarded(w.as_ref(), ctx))
        .collect();
    let Some(seg) = segments.first() else {
        return String::new();
    };
    let is_current = ctx.window.as_ref().is_some_and(|w| w.is_current);
    render_window_pill(&seg.text, is_current, theme)
}
```

The panic guard (`render_guarded`) is preserved: a panicking widget yields an
empty segment list → `""`, never a broken bar (invariant #6 / N2).

### 4. `Windows` widget (`rustline-core/src/widgets/windows.rs`)

The widget becomes a **text producer**: it emits one `Segment` with text
`"{index}{flags} {name}"` and default style. Pill color/emphasis now comes from
`render_window` via the theme. (The widget's own `Style` is no longer the source
of truth for the pill; this is intentional so colors are themeable.) `None`
window still returns `vec![]`.

### 5. Config (`rustline-core/src/config.rs`)

`ThemeConfig` gains six `Option` fields (`#[serde(default)]`):
`win_cap_left: Option<String>`, `win_cap_right: Option<String>`,
`win_current_bg`, `win_current_fg`, `win_inactive_bg`, `win_inactive_fg:
Option<Color>`. `Config::to_theme()` maps each `Some(_)` onto the
`Theme::default()` value, exactly like the existing `palette`/`fg`/`bar_bg`
handling. `Config::load` stays total (invariant #3) — the fields are all
`#[serde(default)]`.

Example override:

```toml
[theme]
win_current_bg = { Indexed = 31 }
win_inactive_bg = { Indexed = 236 }
win_current_fg = { Indexed = 255 }
win_inactive_fg = { Indexed = 250 }
win_cap_left = ""
win_cap_right = ""
```

## Testing

- **`render.rs`**: unit tests for `render_window_pill` — active pill uses
  `win_current_bg` (colour31), rounded caps (`\u{e0b6}` / `\u{e0b4}`), bold, white
  text; inactive uses `win_inactive_bg` (colour236), rounded caps, no bold,
  light-gray text; both wrap ` text ` and end with `#[default]`.
- **`widgets/windows.rs`**: update tests — the widget now emits plain text
  (`"0* name"`, `"1 other"`) with default style; drop the bold/bg assertions
  (those move to the pill renderer). `None` → empty.
- **`assemble.rs`**: `render_window` renders the current window as a bold cyan
  rounded pill and an inactive window as a gray rounded pill; no-window → `""`.
- **`config.rs`**: `to_theme` maps each new `[theme]` override; unset → default.
- **`crates/rustline/tests/smoke.rs`**: update any window assertions that assume
  the blue pointed-cap format to the rounded-pill format.

## Invariants preserved

- **#1** — windows still render only from `Context` (`is_current` from
  `ctx.window`); no environment reads.
- **#2** — `Segment`/`Style`/`Color`/`Context` stay serde-serializable
  (unchanged types; `Theme` is not part of the WASM ABI).
- **#3** — `Config::load` stays total; new fields are `#[serde(default)]`.
- **#6 / N2** — the `catch_unwind` guard in `render_window` is preserved; a
  panicking/absent window degrades to `""`.

## Out of scope

- Left/right status separators stay pointed (only the window list is rounded).
- No new CLI flags. No change to the two-line status layout (that lives in the
  user's tmux config and is orthogonal).
- Boldness remains intrinsic (not a theme field).

## Non-repo follow-up

The user's personal `~/.tmux.conf` currently has a hand-rolled native-tmux pill
(from an earlier iteration); revert its `window-status-format` /
`window-status-current-format` back to `#(rustline render window …)` so rustline
drives the pill. This is outside the repo commit.
