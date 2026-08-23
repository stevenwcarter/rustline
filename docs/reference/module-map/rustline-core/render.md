# `rustline-core` — rendering (`render.rs`, `themes.rs`, `range_name.rs`, `assemble.rs`)

> **Read when:** touching the powerline renderer, window pills, markup sanitization (invariant #8), clickable range names (invariant #7), the built-in themes, or region assembly.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- `render.rs` — `render_region(Direction, &[Segment], &Theme) -> String`, the
  load-bearing powerline renderer (hard `` `` / soft `` `` separators, edge
  blending to `bar_bg`); `render_window_pill(text, is_current, &Theme) ->
  String`, the window-list rounded-pill renderer (rounded `` / `` caps colored
  `fg=pill,bg=bar_bg` — the *opposite* of a pointed separator); `RangeGroup`
  (a widget's segments plus its optional clickable range name, typed
  `range: Option<RangeName>` — see `range_name.rs` below — so the bytes
  interpolated into `#[range=user|…]` are safe by construction rather than
  re-checked here) and
  `render_region_ranged(Direction, &[RangeGroup], &Theme) -> String`, which
  brackets each clickable group in `#[range=user|NAME]…#[norange]` with
  separators/edges kept outside any range — byte-identical to `render_region`
  when every group's range is `None`; `Theme`
  (palette, glyphs, colors, incl. the six `win_*` pill fields and the four
  `success`/`info`/`warning`/`error` semantic colors) with `Default`, plus
  `Theme::colors() -> ThemeColors` (projects the fg/bar_bg/semantic fields for
  `Context.colors`). Also the private `sanitize_text` — the single
  markup-escaping choke point (invariant #8): it doubles `#` and replaces C0
  control characters, and is applied by all three of `render_region`,
  `render_region_ranged` and `render_window_pill` to segment text and nothing
  else. It borrows when there is nothing to change, so the common path
  allocates nothing.
- `themes.rs` — `builtin_theme(name) -> Option<Theme>` and
  `builtin_theme_names() -> &[&str]`, the seven built-in themes (`default`,
  `pastel-rainbow`, `nord`, `gruvbox`, `catppuccin-mocha`, `tokyo-night`,
  `dracula`); each
  is a complete `Theme` (all fields, incl. semantics), and every non-`default`
  theme is multi-accent (`palette.len() >= 4`) using truecolor (`Color::Rgb`).
  See Themes under Config for the full list and layering rules.
- `range_name.rs` — `RangeName`/`NameError`: the one definition of invariant
  #7's clickable-name rule (non-empty, at most `RANGE_NAME_MAX_BYTES` bytes —
  moved here from `render.rs` — `[A-Za-z0-9_-]` only, and not the reserved
  name `"window"`). `RangeName::parse` is the sole constructor, checking (in
  order) empty → bad char → reserved → too long, so a name that is both
  charset-invalid and too long always reports the charset violation, never
  masked by the length check; `RangeName::of_widget(&WidgetName) ->
  Option<RangeName>` bridges a widget's identity into its clickable form.
  Holding a `RangeName` is proof its bytes are safe to interpolate into
  `#[range=user|…]` verbatim — nothing downstream re-checks length or
  charset. Closes the range-forging hole: a charset-violating name (e.g.
  carrying a `#`) could previously still pass the old length-only check and
  reach `#[range=user|…]` markup unescaped.

- `assemble.rs` — `assign_palette`, `render_named_region` (panic-guarded per
  widget via `catch_unwind`; now range-wraps via `render_region_ranged`,
  remembering each widget's `range_name()` across the palette-assignment
  flatten/regroup, and keying each widget's color override off the `(name,
  widget)` pairs `Registry::resolve` now returns (W53) — the old second
  `resolved_names`/`registry.contains` re-filter is gone), `render_window`
  (wraps the `windows` text in a
  themed rounded pill via `render.rs::render_window_pill`, keyed on
  `ctx.window.is_current`; still `catch_unwind`-guarded → `""` on panic/no
  window; the window pill is never clickable — `render window` has no
  `--plugin-dir` and no range wrapping), and `render_windows(windows:
  &[WindowCtx], &Registry, &Theme) -> String` (W42, the batched two-line
  window-list render). `render_window` and `render_windows` now share a
  private `window_pill` helper (one window → one pill, factored out of
  `render_window`'s prior body); unlike the singular path, `render_windows`
  DOES wrap each pill in `#[range=window|<index>]…#[norange]` — it re-emits
  the same `range=window|IDX` markers tmux's own `#{W:}` loop used to (see
  tmux integration model below), so window-select clicks keep working. Pills
  are joined with no separator (each is self-contained).
