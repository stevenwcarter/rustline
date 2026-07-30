//! The interactive widget editor: a pure state machine ([`EditorState`]) plus
//! a thin ratatui draw loop wired up in [`run`]. Everything interesting —
//! focus, add and remove, reorder, dirty tracking, quit confirmation — lives
//! in `on_key`, which takes a [`KeyKind`] and returns an [`EditorAction`]. No
//! terminal is involved there, so that behavior is unit-tested directly, the
//! same split `theme_cmd.rs`'s reader/writer-generic `run_picker` uses.
//!
//! `run` is the terminal-owning shell: it requires a TTY (mirroring `theme
//! pick`'s guard), builds the initial [`EditorState`] from the config file and
//! the widget catalog, then drives a `ratatui`/crossterm draw loop until the
//! user quits. [`TerminalGuard`] restores raw mode and the alternate screen on
//! every exit path, including a panic unwinding through the loop.
//!
//! CENTER is not a fourth freely-editable region: tmux's window list is
//! rendered by a dedicated, hardcoded path (`assemble.rs`'s `window_pill`,
//! resolving only the `windows` widget), so `[layout].center` has no render
//! path that reads it. [`Column::Center`] stays focusable/visible so a user
//! can see the window list lives there, but every edit that would touch it
//! is refused (see [`EditorState::targets_center`]) — a silent no-op, which
//! is what an edit to CENTER used to be before this restriction, would be
//! worse than being told why.

use std::io::{self, IsTerminal, Stdout};
use std::path::Path;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::layout::{Constraint, Direction as LayoutDirection, Layout as UiLayout};
use ratatui::style::{Color as UiColor, Modifier, Style as UiStyle};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::{Terminal, backend::CrosstermBackend};
use rustline_core::{
    Color, Config, Layout, Region, Registry, Style, Theme, WidgetName, WidgetPlacement,
    WidgetSource, layout_disable, layout_enable, layout_move, layout_nudge, parse_markup,
    render_named_region,
};
use toml_edit::DocumentMut;
use unicode_width::UnicodeWidthStr;

/// The four focusable columns: the three layout regions plus the pool of
/// widgets that aren't currently placed.
///
/// `Center` is special: tmux renders the window list there itself
/// (`assemble.rs`'s `window_pill`, called by both `render_window` and
/// `render_windows`, hardcodes `registry.resolve(&["windows".to_string()])`
/// — no render path reads `[layout].center`). It stays **focusable and
/// visible** rather than being skipped entirely, so a user who lands on it
/// can see what's there and learn *why* it can't be changed; every edit that
/// would touch it is refused instead (see [`EditorState::targets_center`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Left,
    Center,
    Right,
    Available,
}

impl Column {
    const ALL: [Column; 4] = [
        Column::Left,
        Column::Center,
        Column::Right,
        Column::Available,
    ];

    /// The layout region this column edits, or `None` for `Available`.
    fn region(self) -> Option<Region> {
        match self {
            Column::Left => Some(Region::Left),
            Column::Center => Some(Region::Center),
            Column::Right => Some(Region::Right),
            Column::Available => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Column::Left => "LEFT",
            Column::Center => "CENTER (fixed)",
            Column::Right => "RIGHT",
            Column::Available => "AVAILABLE",
        }
    }
}

/// Shown in the status line when an edit is refused because it targets
/// CENTER — tmux renders the window list there itself, so
/// `[layout].center`'s contents aren't configurable from this editor (or from
/// `widget enable|move --region center`; see `widget_cmd.rs`'s
/// `warn_if_center`, which warns rather than refuses there, since CLI edits
/// to `center` still need to remain writable — e.g. round-tripping the
/// default `center = ["windows"]`).
const CENTER_FIXED_STATUS: &str =
    "CENTER is fixed: tmux renders the window list there, not [layout].center — nothing to edit";

/// A key press, already mapped out of crossterm's `KeyEvent` so the state
/// machine has no terminal dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Up,
    Down,
    Left,
    Right,
    Space,
    NudgeUp,
    NudgeDown,
    RegionPrev,
    RegionNext,
    Write,
    Quit,
    Help,
    Other,
}

/// What the draw loop should do after a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorAction {
    /// Redraw and keep going.
    Redraw,
    /// Write the layout to disk, then keep going.
    Write,
    /// Exit the editor.
    Quit,
    /// Unsaved changes: show the confirm prompt instead of quitting.
    ConfirmQuit,
}

/// Everything the editor knows. Pure — no terminal, no files.
pub struct EditorState {
    layout: Layout,
    /// Every selectable widget, by name, with its metadata. The layout is the
    /// source of truth for placement; this is only for names/summaries.
    catalog: Vec<WidgetPlacement>,
    column: Column,
    /// Cursor index within each column, kept per column so focus changes
    /// don't lose your place.
    cursors: [usize; 4],
    /// Which region a widget taken from AVAILABLE goes into.
    last_region: Column,
    dirty: bool,
    /// True while the quit confirmation is showing.
    confirming_quit: bool,
    status: String,
    show_help: bool,
}

impl EditorState {
    pub fn new(layout: Layout, catalog: Vec<WidgetPlacement>) -> Self {
        Self {
            layout,
            catalog,
            column: Column::Left,
            cursors: [0; 4],
            last_region: Column::Right,
            dirty: false,
            confirming_quit: false,
            status: String::new(),
            show_help: false,
        }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn column(&self) -> Column {
        self.column
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn show_help(&self) -> bool {
        self.show_help
    }

    pub fn confirming_quit(&self) -> bool {
        self.confirming_quit
    }

    /// Look up a catalog entry, for the source badge the draw loop shows.
    pub fn source_of(&self, name: &str) -> Option<&WidgetSource> {
        self.catalog
            .iter()
            .find(|p| p.name == name)
            .map(|p| &p.source)
    }

    /// The names shown in `column`, in display order. AVAILABLE is every
    /// catalog entry not currently placed, sorted by name.
    pub fn column_items(&self, column: Column) -> Vec<String> {
        match column.region() {
            Some(region) => self
                .layout
                .get(region)
                .iter()
                .map(|n| n.to_string())
                .collect(),
            None => {
                let mut names: Vec<String> = self
                    .catalog
                    .iter()
                    .filter(|p| self.layout.find(&p.name).is_none())
                    .map(|p| p.name.clone())
                    .collect();
                names.sort();
                names
            }
        }
    }

    /// All four columns' items, for one draw pass.
    pub fn columns(&self) -> Vec<(Column, Vec<String>)> {
        Column::ALL
            .into_iter()
            .map(|c| (c, self.column_items(c)))
            .collect()
    }

    fn cursor(&self) -> usize {
        self.cursors[Column::ALL.iter().position(|c| *c == self.column).unwrap()]
    }

    fn set_cursor(&mut self, index: usize) {
        let slot = Column::ALL.iter().position(|c| *c == self.column).unwrap();
        self.cursors[slot] = index;
    }

    /// The widget under the cursor in the focused column, if any.
    pub fn selected(&self) -> Option<&str> {
        let items = self.column_items(self.column);
        let index = self.cursor().min(items.len().saturating_sub(1));
        if items.is_empty() {
            return None;
        }
        // Re-borrow from the layout/catalog rather than the temporary vec.
        match self.column.region() {
            Some(region) => self.layout.get(region).get(index).map(WidgetName::as_str),
            None => self
                .catalog
                .iter()
                .find(|p| p.name == items[index])
                .map(|p| p.name.as_str()),
        }
    }

    /// The cursor's row index within the focused column, clamped to its length.
    pub fn cursor_index(&self) -> usize {
        let len = self.column_items(self.column).len();
        self.cursor().min(len.saturating_sub(1))
    }

    /// Handle one key. The single entry point the draw loop calls.
    pub fn on_key(&mut self, key: KeyKind) -> EditorAction {
        // Any key other than a second Quit cancels a pending confirmation.
        if self.confirming_quit && key != KeyKind::Quit {
            self.confirming_quit = false;
            self.status.clear();
        }
        match key {
            KeyKind::Left => {
                self.shift_column(-1);
                EditorAction::Redraw
            }
            KeyKind::Right => {
                self.shift_column(1);
                EditorAction::Redraw
            }
            KeyKind::Up => {
                self.move_cursor(-1);
                EditorAction::Redraw
            }
            KeyKind::Down => {
                self.move_cursor(1);
                EditorAction::Redraw
            }
            KeyKind::Space => {
                self.toggle_selected();
                EditorAction::Redraw
            }
            KeyKind::NudgeUp => {
                self.nudge(-1);
                EditorAction::Redraw
            }
            KeyKind::NudgeDown => {
                self.nudge(1);
                EditorAction::Redraw
            }
            KeyKind::RegionPrev => {
                self.move_region(-1);
                EditorAction::Redraw
            }
            KeyKind::RegionNext => {
                self.move_region(1);
                EditorAction::Redraw
            }
            KeyKind::Write => EditorAction::Write,
            KeyKind::Help => {
                self.show_help = !self.show_help;
                EditorAction::Redraw
            }
            KeyKind::Quit => {
                if self.dirty && !self.confirming_quit {
                    self.confirming_quit = true;
                    self.status = "unsaved changes — press q again to discard, w to write".into();
                    return EditorAction::ConfirmQuit;
                }
                EditorAction::Quit
            }
            KeyKind::Other => EditorAction::Redraw,
        }
    }

    /// Called by the draw loop after a successful write.
    pub fn mark_written(&mut self) {
        self.dirty = false;
        self.confirming_quit = false;
        self.status = "written".into();
    }

    /// Called by the draw loop when a write failed.
    pub fn mark_write_failed(&mut self, error: &str) {
        self.status = format!("write failed: {error}");
    }

    fn shift_column(&mut self, delta: i32) {
        let current = Column::ALL.iter().position(|c| *c == self.column).unwrap() as i32;
        let next = (current + delta).clamp(0, Column::ALL.len() as i32 - 1) as usize;
        if self.column.region().is_some() {
            self.last_region = self.column;
        }
        self.column = Column::ALL[next];
    }

    fn move_cursor(&mut self, delta: i32) {
        let len = self.column_items(self.column).len();
        if len == 0 {
            self.set_cursor(0);
            return;
        }
        let next = (self.cursor_index() as i32 + delta).clamp(0, len as i32 - 1) as usize;
        self.set_cursor(next);
    }

    /// True when the pending Space edit would touch CENTER — either removing
    /// directly from it, or adding into it from AVAILABLE. Both are refused;
    /// see [`CENTER_FIXED_STATUS`].
    ///
    /// The AVAILABLE clause is **unreachable today**: `last_region` is only
    /// ever written by `shift_column` (to the column being *left*), and the
    /// sole path into AVAILABLE is from RIGHT — so it is always `Right` when
    /// `column == Available`. It is kept, and pinned by
    /// `placing_from_available_into_center_is_refused`, so the guard stays
    /// correct however focus arrived: a future jump-to-column key or a
    /// reordered `Column::ALL` would make the path live without anyone having
    /// to rediscover that CENTER needs refusing here too.
    fn targets_center(&self) -> bool {
        self.column == Column::Center
            || (self.column == Column::Available && self.last_region == Column::Center)
    }

    /// Space: AVAILABLE → append to **RIGHT**; a region → back to AVAILABLE.
    ///
    /// Placement targets `last_region`, which is always `Right` in practice
    /// (see `targets_center`'s doc for why). Use `H`/`L` to move a placed
    /// widget to another region — that, not Space, is how a widget reaches
    /// LEFT.
    fn toggle_selected(&mut self) {
        let Some(name) = self.selected().map(str::to_string) else {
            return;
        };
        if self.targets_center() {
            self.status = CENTER_FIXED_STATUS.to_string();
            return;
        }
        let result = match self.column.region() {
            Some(_) => layout_disable(&mut self.layout, &name),
            None => {
                let region = self.last_region.region().unwrap_or(Region::Right);
                layout_enable(&mut self.layout, &name, region, None)
            }
        };
        match result {
            Ok(_) => {
                self.dirty = true;
                self.status = String::new();
                self.clamp_cursor();
            }
            Err(error) => self.status = format!("{name}: {error}"),
        }
    }

    fn nudge(&mut self, delta: i32) {
        if self.column.region().is_none() {
            return;
        }
        if self.column == Column::Center {
            self.status = CENTER_FIXED_STATUS.to_string();
            return;
        }
        let Some(name) = self.selected().map(str::to_string) else {
            return;
        };
        // A boundary nudge is a no-op, not an error worth surfacing.
        if let Ok(change) = layout_nudge(&mut self.layout, &name, delta) {
            self.dirty = true;
            self.status = String::new();
            if let Some((_, index)) = change.to {
                self.set_cursor(index);
            }
        }
    }

    /// `H`/`L`: move the selected *placed* widget to the previous/next
    /// **editable** layout region, via [`layout_move`]. This is the only
    /// path that can ever place a widget in LEFT — AVAILABLE→region
    /// (`toggle_selected`) always appends to `last_region`, which is
    /// unconditionally RIGHT in practice (see `targets_center`'s doc for the
    /// `shift_column`/`Column::ALL` adjacency argument), so LEFT was
    /// previously unreachable from the editor entirely.
    ///
    /// CENTER sits between LEFT and RIGHT in `Region::ALL`'s (visual)
    /// order, but is never a valid H/L destination — [`adjacent_editable_region`]
    /// skips over it, so a single `H`/`L` moves directly between LEFT and
    /// RIGHT rather than getting stuck refusing at CENTER, which would make
    /// LEFT unreachable from RIGHT again. CENTER *is* still refused as a
    /// **source**: focus a widget in the CENTER column and press `H`/`L`
    /// (as well as `space`/`J`/`K`) and it's refused with
    /// [`CENTER_FIXED_STATUS`] — the same explanatory status `nudge`/
    /// `toggle_selected` already use, now reachable via H/L too. A move
    /// past either end (nothing before LEFT, nothing after RIGHT) is a
    /// silent boundary no-op, mirroring [`Self::nudge`]'s boundary
    /// handling. On success, focus follows the widget into its new
    /// region's column.
    fn move_region(&mut self, delta: i32) {
        let Some(region) = self.column.region() else {
            return; // AVAILABLE: nothing placed to move between regions.
        };
        if region == Region::Center {
            self.status = CENTER_FIXED_STATUS.to_string();
            return;
        }
        let Some(name) = self.selected().map(str::to_string) else {
            return;
        };
        let Some(target_region) = adjacent_editable_region(region, delta) else {
            return;
        };
        if let Ok(change) = layout_move(&mut self.layout, &name, target_region, usize::MAX) {
            self.dirty = true;
            self.status = String::new();
            if let Some(column) = Column::ALL
                .into_iter()
                .find(|c| c.region() == Some(target_region))
            {
                self.column = column;
            }
            if let Some((_, index)) = change.to {
                self.set_cursor(index);
            }
        }
    }

    fn clamp_cursor(&mut self) {
        let len = self.column_items(self.column).len();
        let clamped = self.cursor().min(len.saturating_sub(1));
        self.set_cursor(clamped);
    }
}

/// The next region from `from` in `delta`'s direction (`Region::ALL`'s
/// visual order), skipping CENTER — never a real H/L destination, see
/// [`EditorState::move_region`] — and stopping at either end. `None` at a
/// boundary (nothing further in that direction), mirroring
/// [`layout_nudge`]'s boundary `NoOp`.
fn adjacent_editable_region(from: Region, delta: i32) -> Option<Region> {
    let mut index = Region::ALL.iter().position(|r| *r == from)? as i32;
    loop {
        index += delta.signum();
        let next = *Region::ALL.get(usize::try_from(index).ok()?)?;
        if next != Region::Center {
            return Some(next);
        }
    }
}

/// Map a crossterm key event onto the editor's own [`KeyKind`]. Arrow keys and
/// their vim equivalents are interchangeable; `J`/`K` (shifted, however the
/// terminal reports it) reorder within a region, and `H`/`L` (same shifted
/// convention) move a placed widget to the previous/next region.
fn map_key(key: KeyEvent) -> KeyKind {
    match key.code {
        KeyCode::Up => KeyKind::Up,
        KeyCode::Down => KeyKind::Down,
        KeyCode::Left => KeyKind::Left,
        KeyCode::Right => KeyKind::Right,
        KeyCode::Enter | KeyCode::Char(' ') => KeyKind::Space,
        KeyCode::Esc => KeyKind::Quit,
        KeyCode::Char('J') => KeyKind::NudgeDown,
        KeyCode::Char('K') => KeyKind::NudgeUp,
        KeyCode::Char('H') => KeyKind::RegionPrev,
        KeyCode::Char('L') => KeyKind::RegionNext,
        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::SHIFT) => KeyKind::NudgeDown,
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::SHIFT) => KeyKind::NudgeUp,
        KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::SHIFT) => KeyKind::RegionPrev,
        KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::SHIFT) => KeyKind::RegionNext,
        KeyCode::Char('j') => KeyKind::Down,
        KeyCode::Char('k') => KeyKind::Up,
        KeyCode::Char('h') => KeyKind::Left,
        KeyCode::Char('l') => KeyKind::Right,
        KeyCode::Char('w') => KeyKind::Write,
        KeyCode::Char('q') => KeyKind::Quit,
        KeyCode::Char('?') => KeyKind::Help,
        _ => KeyKind::Other,
    }
}

/// The ANSI preview strip: the current (possibly unsaved) layout rendered
/// through the real pipeline — `sample_context` + the resolved theme +
/// `render_named_region` + `tmux_to_ansi`, exactly as `render left`/`render
/// right` do (see `main.rs`).
///
/// **Plugins are drawn as a static `[name]` chip, never instantiated.**
/// Instantiating a WASM guest on every keystroke would be slow and would run
/// third-party code inside a config editor; the placeholder makes the widget's
/// position visible without that cost. `Center` has no real
/// `render_named_region` call site (the window list is rendered as pills via a
/// dedicated path — see `assemble.rs`), so this preview treats it like `Left`
/// purely for a stable, readable strip; it is not meant to match tmux's actual
/// window-pill rendering.
///
/// `theme`/`registry` are built once by [`run`] and passed in, not rebuilt
/// here — this function is called on every draw (so, every keystroke); a
/// `Registry::with_builtins` rebuild that expensive-per-keystroke would also
/// `warn!` once per malformed `[instances.<name>]` on *every single one* of
/// those rebuilds, spamming the rotated log file at typing speed (M4). Only
/// `cfg.color_overrides()` is still recomputed per draw — a pure, cheap
/// projection over already-parsed config with no I/O or logging of its own.
fn preview_regions(
    state: &EditorState,
    cfg: &Config,
    theme: &Theme,
    registry: &Registry,
) -> Vec<(Region, Line<'static>)> {
    let ctx = crate::sample_context::sample_context(false);
    let overrides = cfg.color_overrides();
    let mut out: Vec<(Region, Line<'static>)> = Vec::new();

    for region in Region::ALL {
        let all = state.layout().get(region);
        // Only names the registry knows render for real; anything else (a
        // plugin stem, an unresolvable instance) becomes a chip.
        let names: Vec<WidgetName> = all
            .iter()
            .filter(|n| registry.contains(n.as_str()))
            .cloned()
            .collect();
        let chips: Vec<String> = all
            .iter()
            .filter(|n| !registry.contains(n.as_str()))
            .map(|n| format!("[{n}]"))
            .collect();
        let dir = match region {
            Region::Right => rustline_core::Direction::Right,
            Region::Left | Region::Center => rustline_core::Direction::Left,
        };
        let markup = render_named_region(dir, &names, &ctx, registry, theme, &overrides);

        // `parse_markup`, not `tmux_to_ansi`: ratatui writes cells and never
        // interprets ANSI escapes, so escape bytes handed to it are drawn as
        // literal text with no styling at all. See `ansi.rs`'s module doc.
        let mut spans: Vec<Span<'static>> = parse_markup(&markup)
            .into_iter()
            .map(|s| Span::styled(s.text, to_ui_style(&s.style)))
            .collect();
        if !chips.is_empty() {
            spans.push(Span::raw(format!(" {}", chips.join(" "))));
        }
        if spans.iter().any(|s| !s.content.trim().is_empty()) {
            out.push((region, Line::from(spans)));
        }
    }
    out
}

/// Translate a rustline [`Style`] into ratatui's own. A `None` channel is the
/// terminal's default (`Color::Reset`), not black — tmux's `fg=default` and an
/// unstyled segment both mean "leave it alone".
fn to_ui_style(style: &Style) -> UiStyle {
    let mut ui = UiStyle::default()
        .fg(style.fg.as_ref().map_or(UiColor::Reset, to_ui_color))
        .bg(style.bg.as_ref().map_or(UiColor::Reset, to_ui_color));
    if style.bold {
        ui = ui.add_modifier(Modifier::BOLD);
    }
    ui
}

/// Map one rustline colour onto ratatui's palette. The named arm mirrors
/// `ansi.rs`'s SGR mapping exactly: the basic eight are 30–37 (tmux `white` is
/// SGR 37, which ratatui calls `Gray`) and `bright*` are 90–97 (tmux
/// `brightwhite` is 97 — ratatui's `White`).
fn to_ui_color(color: &Color) -> UiColor {
    match color {
        Color::Indexed(n) => UiColor::Indexed(*n),
        Color::Rgb(r, g, b) => UiColor::Rgb(*r, *g, *b),
        Color::Named(name) => match name.as_str() {
            "black" => UiColor::Black,
            "red" => UiColor::Red,
            "green" => UiColor::Green,
            "yellow" => UiColor::Yellow,
            "blue" => UiColor::Blue,
            "magenta" => UiColor::Magenta,
            "cyan" => UiColor::Cyan,
            "white" => UiColor::Gray,
            "brightblack" => UiColor::DarkGray,
            "brightred" => UiColor::LightRed,
            "brightgreen" => UiColor::LightGreen,
            "brightyellow" => UiColor::LightYellow,
            "brightblue" => UiColor::LightBlue,
            "brightmagenta" => UiColor::LightMagenta,
            "brightcyan" => UiColor::LightCyan,
            "brightwhite" => UiColor::White,
            _ => UiColor::Reset,
        },
    }
}

/// Spaces between regions when they share one line.
const PREVIEW_GAP: usize = 3;

/// How the preview strip arranges the regions at a given terminal width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewLayout {
    /// Everything fits: regions sit side by side on one line, as the real bar
    /// does.
    SideBySide,
    /// Too narrow: one region per line, so nothing is silently clipped.
    Stacked,
}

/// Decide how to arrange `widths` (one per non-empty region, in display
/// columns) in `available` columns.
///
/// Side by side needs every region plus a [`PREVIEW_GAP`] between each
/// adjacent pair; anything wider than the terminal stacks instead. Measured in
/// *display width*, not bytes or chars — the bar is full of multi-byte
/// powerline glyphs and Nerd Font icons, and some are double-width.
pub fn preview_layout(widths: &[usize], available: usize) -> PreviewLayout {
    if widths.len() < 2 {
        return PreviewLayout::SideBySide;
    }
    let gaps = PREVIEW_GAP * (widths.len() - 1);
    let total: usize = widths.iter().sum::<usize>() + gaps;
    if total <= available {
        PreviewLayout::SideBySide
    } else {
        PreviewLayout::Stacked
    }
}

/// Rows the preview strip needs: one when side by side, else one per region
/// (at least one, so the strip never collapses to nothing).
fn preview_height(layout: PreviewLayout, regions: usize) -> u16 {
    match layout {
        PreviewLayout::SideBySide => 1,
        PreviewLayout::Stacked => regions.max(1) as u16,
    }
}

/// The display width of a rendered line, in terminal columns.
fn line_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|s| s.content.as_ref().width())
        .sum::<usize>()
}

/// Restores the terminal on every exit path — normal return, `?`, or a panic
/// unwinding through the draw loop. Without this a panic inside ratatui leaves
/// the user's shell in raw mode with no echo. The default panic strategy for
/// this workspace is `unwind` (no crate sets `panic = "abort"`), so a panic
/// anywhere in the loop below still drops this guard on its way up.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        Terminal::new(CrosstermBackend::new(stdout))
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

/// The key-hint footer shown when there's no status message.
const HELP_LINE: &str =
    "←→ region  ↑↓ select  space add/remove  J/K reorder  H/L move region  w write  q quit";

/// The fuller hint shown while `?` help is toggled on, appended above the
/// footer.
const HELP_DETAIL: &str = "vim keys: h j k l (shift: H/L move region, J/K reorder)   Enter also adds/removes   Esc also quits   ? toggles this line";

/// Render one frame. Pure with respect to its arguments — it reads
/// [`EditorState::columns`], [`EditorState::column`],
/// [`EditorState::cursor_index`], [`EditorState::status`],
/// [`EditorState::show_help`], [`EditorState::confirming_quit`], and
/// [`EditorState::is_dirty`] (for the footer's `[modified]` marker), and
/// never mutates anything. `theme`/`registry` are built once by [`run`] and
/// threaded through to [`preview_line`] — see its doc for why (M4).
fn draw(
    frame: &mut ratatui::Frame,
    state: &EditorState,
    cfg: &Config,
    theme: &Theme,
    registry: &Registry,
) {
    let help_height = u16::from(state.show_help());

    // The preview is measured before the vertical split, because how many rows
    // it needs depends on whether the regions fit side by side at this width.
    let regions = preview_regions(state, cfg, theme, registry);
    let widths: Vec<usize> = regions.iter().map(|(_, line)| line_width(line)).collect();
    let layout = preview_layout(&widths, frame.area().width as usize);
    let preview_rows = preview_height(layout, regions.len());

    let rows = UiLayout::default()
        .direction(LayoutDirection::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(preview_rows),
            Constraint::Length(help_height),
            Constraint::Length(1), // footer / status
        ])
        .split(frame.area());

    let columns = UiLayout::default()
        .direction(LayoutDirection::Horizontal)
        .constraints([Constraint::Ratio(1, 4); 4])
        .split(rows[0]);

    for (area, (column, items)) in columns.iter().zip(state.columns()) {
        let focused = column == state.column();
        let border_style = if focused {
            UiStyle::default().add_modifier(Modifier::BOLD)
        } else {
            UiStyle::default()
        };
        let list_items: Vec<ListItem> = items
            .iter()
            .map(|name| {
                let tag = match state.source_of(name) {
                    Some(WidgetSource::Plugin) => " (plugin)",
                    Some(WidgetSource::Instance { .. }) => " (instance)",
                    Some(WidgetSource::Unknown) => " (unknown)",
                    _ => "",
                };
                ListItem::new(Line::from(Span::raw(format!("{name}{tag}"))))
            })
            .collect();
        let block = Block::default()
            .title(column.title())
            .borders(Borders::ALL)
            .border_style(border_style);
        let list = List::new(list_items)
            .block(block)
            .highlight_style(UiStyle::default().add_modifier(Modifier::REVERSED));
        let mut list_state = ListState::default();
        if focused && !items.is_empty() {
            list_state.select(Some(state.cursor_index()));
        }
        frame.render_stateful_widget(list, *area, &mut list_state);
    }

    render_preview(frame, rows[1], regions, layout);

    if state.show_help() {
        frame.render_widget(Paragraph::new(HELP_DETAIL), rows[2]);
    }

    let footer_style = if state.confirming_quit() {
        UiStyle::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        UiStyle::default()
    };
    let footer_text = if !state.status().is_empty() {
        state.status().to_string()
    } else if state.is_dirty() {
        format!("{HELP_LINE}   [modified]")
    } else {
        HELP_LINE.to_string()
    };
    frame.render_widget(Paragraph::new(footer_text).style(footer_style), rows[3]);
}

/// Draw the preview strip into `area`.
///
/// Side by side, the regions share one line separated by [`PREVIEW_GAP`].
/// Stacked, each gets its own row, aligned the way it sits on the real status
/// line — LEFT left, CENTER centred, RIGHT right — so a narrow terminal still
/// conveys where each region actually lives rather than reading as one
/// left-packed list.
fn render_preview(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    regions: Vec<(Region, Line<'static>)>,
    layout: PreviewLayout,
) {
    match layout {
        PreviewLayout::SideBySide => {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (i, (_, line)) in regions.into_iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw(" ".repeat(PREVIEW_GAP)));
                }
                spans.extend(line.spans);
            }
            frame.render_widget(Paragraph::new(Line::from(spans)), area);
        }
        PreviewLayout::Stacked => {
            let slots = UiLayout::default()
                .direction(LayoutDirection::Vertical)
                .constraints(vec![Constraint::Length(1); regions.len().max(1)])
                .split(area);
            for (slot, (region, line)) in slots.iter().zip(regions) {
                let aligned = match region {
                    Region::Left => line.left_aligned(),
                    Region::Center => line.centered(),
                    Region::Right => line.right_aligned(),
                };
                frame.render_widget(Paragraph::new(aligned), *slot);
            }
        }
    }
}

/// Open the interactive widget editor. Requires a TTY; a non-interactive
/// invocation prints a hint toward the scriptable subcommands and exits
/// non-zero without drawing anything (mirroring `theme pick`'s guard).
pub fn run(config_path: &Path, plugin_dir: &Path) -> i32 {
    if !io::stdin().is_terminal() {
        eprintln!(
            "`widget edit` needs a terminal; use `rustline widget list`, \
             `widget enable <name>`, or `widget disable <name>` instead"
        );
        return 1;
    }
    let cfg = Config::load(config_path);
    let text = std::fs::read_to_string(config_path).unwrap_or_default();
    let mut doc = match text.parse::<DocumentMut>() {
        Ok(doc) => doc,
        Err(error) => {
            eprintln!("config file is not valid TOML; refusing to edit it: {error}");
            return 1;
        }
    };
    let layout = crate::widget_cmd::read_layout(&doc);
    // Built once for the whole session, not per draw (M4): neither `cfg` nor
    // the resolved theme changes while the editor is open, so rebuilding
    // `Registry::with_builtins` on every keystroke was pure waste — and,
    // worse, re-`warn!`ed for every malformed `[instances.<name>]` on each
    // of those rebuilds, spamming the rotated log file at typing speed.
    let registry = Registry::with_builtins(&cfg);
    let theme = crate::resolve_theme(&cfg);
    let catalog = rustline_core::widget_placements(
        &cfg,
        registry.descriptors(),
        &rustline_wasm::discover_plugin_names(plugin_dir),
    );
    let mut state = EditorState::new(layout, catalog);

    // The guard must be constructed before raw mode is entered, and must
    // outlive the loop, so a panic anywhere below still restores the
    // terminal on unwind.
    let _guard = TerminalGuard;
    let mut terminal = match TerminalGuard::enter() {
        Ok(t) => t,
        Err(error) => {
            eprintln!("failed to start the editor: {error}");
            return 1;
        }
    };

    loop {
        if terminal
            .draw(|frame| draw(frame, &state, &cfg, &theme, &registry))
            .is_err()
        {
            return 1;
        }
        // An `Err` here (as opposed to `Ok` of a non-key event, e.g. a
        // resize or mouse event, which is legitimate and just redraws) means
        // the terminal itself is no longer readable -- e.g. stdin closed out
        // from under raw mode (the popup's pty torn down while this process
        // lingers). That can't self-heal by looping: continuing would spin
        // this loop at 100% CPU forever, since `event::read()` keeps failing
        // the same way on every subsequent call. Bail instead.
        let event = match event::read() {
            Ok(event) => event,
            Err(error) => {
                eprintln!("terminal event stream closed; exiting: {error}");
                return 1;
            }
        };
        let Event::Key(key) = event else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match state.on_key(map_key(key)) {
            EditorAction::Redraw | EditorAction::ConfirmQuit => {}
            EditorAction::Write => {
                match crate::widget_cmd::write_layout(&mut doc, state.layout()) {
                    Ok(()) => {
                        // Mirrors `widget_cmd.rs`'s `mutate`: a config directory
                        // that doesn't exist yet (e.g. a machine with no prior
                        // `rustline` config) must not turn a successful in-memory
                        // edit into an unrecoverable "No such file or directory"
                        // at save time. Best-effort: a failure here still
                        // surfaces through the `std::fs::write` below.
                        if let Some(parent) = config_path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        match std::fs::write(config_path, doc.to_string()) {
                            Ok(()) => {
                                state.mark_written();
                                // Same refresh `widget_cmd.rs`'s CLI mutations
                                // trigger — this editor's primary entry point is
                                // a tmux popup, exactly where it's both possible
                                // and most visible (M3).
                                crate::widget_cmd::refresh_tmux();
                            }
                            Err(error) => state.mark_write_failed(&error.to_string()),
                        }
                    }
                    Err(error) => state.mark_write_failed(&error),
                }
            }
            EditorAction::Quit => break,
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rustline_core::Config;

    use super::*;

    fn placement(name: &str) -> WidgetPlacement {
        WidgetPlacement {
            name: name.to_string(),
            summary: format!("{name} widget"),
            source: WidgetSource::Builtin,
            placement: None,
        }
    }

    fn state() -> EditorState {
        let layout = Layout {
            left: vec!["pane_id".into()],
            center: vec!["windows".into()],
            right: vec!["cwd".into(), "cpu".into()],
        };
        let all = vec![
            placement("pane_id"),
            placement("windows"),
            placement("cwd"),
            placement("cpu"),
            placement("git"),
            placement("disk"),
        ];
        EditorState::new(layout, all)
    }

    #[test]
    fn available_holds_exactly_the_unplaced_widgets() {
        let s = state();
        assert_eq!(s.column_items(Column::Available), ["disk", "git"]);
    }

    #[test]
    fn focus_cycles_across_the_four_columns() {
        let mut s = state();
        assert_eq!(s.column(), Column::Left);
        s.on_key(KeyKind::Right);
        assert_eq!(s.column(), Column::Center);
        s.on_key(KeyKind::Right);
        s.on_key(KeyKind::Right);
        assert_eq!(s.column(), Column::Available);
        s.on_key(KeyKind::Right);
        assert_eq!(
            s.column(),
            Column::Available,
            "no wrap past the last column"
        );
        s.on_key(KeyKind::Left);
        assert_eq!(s.column(), Column::Right);
    }

    #[test]
    fn cursor_moves_within_a_column_and_clamps_at_the_ends() {
        let mut s = state();
        s.on_key(KeyKind::Right);
        s.on_key(KeyKind::Right); // Right region: [cwd, cpu]
        assert_eq!(s.selected(), Some("cwd"));
        s.on_key(KeyKind::Down);
        assert_eq!(s.selected(), Some("cpu"));
        s.on_key(KeyKind::Down);
        assert_eq!(s.selected(), Some("cpu"), "clamps at the bottom");
        s.on_key(KeyKind::Up);
        s.on_key(KeyKind::Up);
        assert_eq!(s.selected(), Some("cwd"), "clamps at the top");
    }

    #[test]
    fn space_in_available_places_the_widget_in_the_last_focused_region() {
        let mut s = state();
        s.on_key(KeyKind::Right);
        s.on_key(KeyKind::Right); // focus Right
        s.on_key(KeyKind::Right); // focus Available
        assert_eq!(s.selected(), Some("disk"));
        s.on_key(KeyKind::Space);
        assert_eq!(s.layout().right, ["cwd", "cpu", "disk"]);
        assert_eq!(s.column_items(Column::Available), ["git"]);
        assert!(s.is_dirty());
    }

    #[test]
    fn space_in_a_region_returns_the_widget_to_available() {
        let mut s = state();
        s.on_key(KeyKind::Right);
        s.on_key(KeyKind::Right); // Right region, cursor on cwd
        s.on_key(KeyKind::Space);
        assert_eq!(s.layout().right, ["cpu"]);
        assert_eq!(s.column_items(Column::Available), ["cwd", "disk", "git"]);
        assert!(s.is_dirty());
    }

    #[test]
    fn removing_the_last_item_clamps_the_cursor_rather_than_dangling() {
        let mut s = state();
        // Left region holds only pane_id; remove it.
        s.on_key(KeyKind::Space);
        assert!(s.layout().left.is_empty());
        assert_eq!(s.selected(), None);
        // A further key must not panic.
        s.on_key(KeyKind::Down);
        s.on_key(KeyKind::Space);
    }

    #[test]
    fn nudge_reorders_within_the_region_and_follows_the_widget() {
        let mut s = state();
        s.on_key(KeyKind::Right);
        s.on_key(KeyKind::Right);
        s.on_key(KeyKind::Down); // Right region, cursor on cpu (index 1)
        s.on_key(KeyKind::NudgeUp);
        assert_eq!(s.layout().right, ["cpu", "cwd"]);
        assert_eq!(s.selected(), Some("cpu"), "cursor follows the moved widget");
    }

    #[test]
    fn nudge_at_the_boundary_changes_nothing_and_does_not_dirty() {
        let mut s = state();
        s.on_key(KeyKind::Right);
        s.on_key(KeyKind::Right); // cursor on cwd (index 0)
        s.on_key(KeyKind::NudgeUp);
        assert_eq!(s.layout().right, ["cwd", "cpu"]);
        assert!(
            !s.is_dirty(),
            "a refused edit must not mark the buffer dirty"
        );
    }

    #[test]
    fn nudge_in_the_available_column_is_ignored() {
        let mut s = state();
        for _ in 0..3 {
            s.on_key(KeyKind::Right);
        }
        assert_eq!(s.column(), Column::Available);
        s.on_key(KeyKind::NudgeDown);
        assert!(!s.is_dirty());
    }

    /// I2: `H`/`L` must be able to place a widget in LEFT — previously no key
    /// sequence could. A single `H` from RIGHT lands directly on LEFT,
    /// skipping over the fixed CENTER region in between rather than getting
    /// stuck refusing there (which would make LEFT unreachable from RIGHT
    /// all over again).
    #[test]
    fn region_move_moves_a_placed_widget_directly_from_right_to_left() {
        let mut s = state();
        s.on_key(KeyKind::Right); // Left -> Center
        s.on_key(KeyKind::Right); // Center -> Right, cursor on "cwd"
        assert_eq!(s.column(), Column::Right);
        assert_eq!(s.selected(), Some("cwd"));
        s.on_key(KeyKind::RegionPrev);
        assert_eq!(s.layout().left, ["pane_id", "cwd"]);
        assert_eq!(s.layout().right, ["cpu"]);
        assert_eq!(s.column(), Column::Left, "focus follows the moved widget");
        assert_eq!(s.selected(), Some("cwd"));
        assert!(s.is_dirty());
    }

    #[test]
    fn region_move_moves_a_placed_widget_directly_from_left_to_right() {
        let mut s = state();
        assert_eq!(s.column(), Column::Left);
        assert_eq!(s.selected(), Some("pane_id"));
        s.on_key(KeyKind::RegionNext);
        assert!(s.layout().left.is_empty());
        assert_eq!(s.layout().right, ["cwd", "cpu", "pane_id"]);
        assert_eq!(s.column(), Column::Right, "focus follows the moved widget");
        assert!(s.is_dirty());
    }

    #[test]
    fn region_move_past_either_end_is_a_noop_and_does_not_dirty() {
        let mut s = state();
        // LEFT: nothing before it.
        s.on_key(KeyKind::RegionPrev);
        assert_eq!(s.layout().left, ["pane_id"]);
        assert!(!s.is_dirty());

        // RIGHT: nothing after it.
        s.on_key(KeyKind::Right);
        s.on_key(KeyKind::Right);
        s.on_key(KeyKind::RegionNext);
        assert_eq!(s.layout().right, ["cwd", "cpu"]);
        assert!(!s.is_dirty());
    }

    #[test]
    fn region_move_sourced_from_center_is_refused_leaves_layout_unchanged_and_does_not_dirty() {
        let mut s = state();
        s.on_key(KeyKind::Right); // Left -> Center, cursor on "windows"
        assert_eq!(s.column(), Column::Center);
        let before = s.layout().clone();

        s.on_key(KeyKind::RegionPrev);
        assert_eq!(s.layout(), &before);
        assert!(!s.is_dirty());
        assert!(
            !s.status().is_empty(),
            "status explains why: {}",
            s.status()
        );

        s.on_key(KeyKind::RegionNext);
        assert_eq!(s.layout(), &before);
        assert!(!s.is_dirty());
    }

    #[test]
    fn region_move_in_the_available_column_is_ignored() {
        let mut s = state();
        for _ in 0..3 {
            s.on_key(KeyKind::Right);
        }
        assert_eq!(s.column(), Column::Available);
        s.on_key(KeyKind::RegionPrev);
        assert!(!s.is_dirty());
        s.on_key(KeyKind::RegionNext);
        assert!(!s.is_dirty());
    }

    #[test]
    fn write_requests_a_write_and_clears_dirty_when_confirmed() {
        let mut s = state();
        s.on_key(KeyKind::Space); // remove pane_id -> dirty
        assert!(s.is_dirty());
        assert_eq!(s.on_key(KeyKind::Write), EditorAction::Write);
        s.mark_written();
        assert!(!s.is_dirty());
    }

    #[test]
    fn quit_while_dirty_asks_first_then_quits_on_the_second_press() {
        let mut s = state();
        s.on_key(KeyKind::Space);
        assert_eq!(s.on_key(KeyKind::Quit), EditorAction::ConfirmQuit);
        assert_eq!(s.on_key(KeyKind::Quit), EditorAction::Quit);
    }

    #[test]
    fn quit_while_clean_quits_immediately() {
        let mut s = state();
        assert_eq!(s.on_key(KeyKind::Quit), EditorAction::Quit);
    }

    #[test]
    fn any_key_after_a_confirm_prompt_cancels_the_quit() {
        let mut s = state();
        s.on_key(KeyKind::Space);
        assert_eq!(s.on_key(KeyKind::Quit), EditorAction::ConfirmQuit);
        s.on_key(KeyKind::Down);
        // Still dirty, and quitting asks again rather than exiting.
        assert_eq!(s.on_key(KeyKind::Quit), EditorAction::ConfirmQuit);
    }

    #[test]
    fn map_key_covers_both_arrow_and_vim_bindings() {
        let ev = |code, mods| KeyEvent::new(code, mods);
        assert_eq!(map_key(ev(KeyCode::Up, KeyModifiers::NONE)), KeyKind::Up);
        assert_eq!(
            map_key(ev(KeyCode::Char('k'), KeyModifiers::NONE)),
            KeyKind::Up
        );
        assert_eq!(
            map_key(ev(KeyCode::Down, KeyModifiers::NONE)),
            KeyKind::Down
        );
        assert_eq!(
            map_key(ev(KeyCode::Char('j'), KeyModifiers::NONE)),
            KeyKind::Down
        );
        assert_eq!(
            map_key(ev(KeyCode::Left, KeyModifiers::NONE)),
            KeyKind::Left
        );
        assert_eq!(
            map_key(ev(KeyCode::Char('h'), KeyModifiers::NONE)),
            KeyKind::Left
        );
        assert_eq!(
            map_key(ev(KeyCode::Right, KeyModifiers::NONE)),
            KeyKind::Right
        );
        assert_eq!(
            map_key(ev(KeyCode::Char('l'), KeyModifiers::NONE)),
            KeyKind::Right
        );
    }

    #[test]
    fn map_key_distinguishes_shifted_nudges_from_plain_motion() {
        // Terminals report Shift+j either as 'J' or as 'j' with the SHIFT modifier.
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT)),
            KeyKind::NudgeDown
        );
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT)),
            KeyKind::NudgeUp
        );
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('J'), KeyModifiers::NONE)),
            KeyKind::NudgeDown
        );
    }

    #[test]
    fn map_key_distinguishes_shifted_region_moves_from_plain_motion() {
        // Terminals report Shift+h either as 'H' or as 'h' with the SHIFT
        // modifier — both must map to a region move, not plain focus motion.
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT)),
            KeyKind::RegionPrev
        );
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT)),
            KeyKind::RegionNext
        );
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('H'), KeyModifiers::NONE)),
            KeyKind::RegionPrev
        );
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::NONE)),
            KeyKind::RegionNext
        );
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::SHIFT)),
            KeyKind::RegionPrev
        );
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::SHIFT)),
            KeyKind::RegionNext
        );
        // Bare (unshifted) lowercase still means plain focus movement.
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)),
            KeyKind::Left
        );
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE)),
            KeyKind::Right
        );
    }

    #[test]
    fn map_key_maps_the_command_keys_and_ignores_the_rest() {
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE)),
            KeyKind::Write
        );
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
            KeyKind::Quit
        );
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            KeyKind::Quit
        );
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)),
            KeyKind::Space
        );
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            KeyKind::Space
        );
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE)),
            KeyKind::Help
        );
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE)),
            KeyKind::Other
        );
    }

    #[test]
    fn center_remains_focusable_and_shows_its_contents() {
        // tmux owns the window list, but a curious user must still be able
        // to land on the column and see what's there — see Column::Center's
        // doc comment for why it isn't skipped from focus entirely.
        let mut s = state();
        s.on_key(KeyKind::Right);
        assert_eq!(s.column(), Column::Center);
        assert_eq!(s.column_items(Column::Center), ["windows"]);
        assert_eq!(s.selected(), Some("windows"));
    }

    #[test]
    fn center_title_is_marked_fixed() {
        assert_eq!(Column::Center.title(), "CENTER (fixed)");
    }

    #[test]
    fn space_on_center_is_refused_leaves_layout_unchanged_and_does_not_dirty() {
        let mut s = state();
        s.on_key(KeyKind::Right); // Left -> Center, cursor on "windows"
        assert_eq!(s.column(), Column::Center);
        let before = s.layout().clone();
        s.on_key(KeyKind::Space);
        assert_eq!(s.layout(), &before, "center layout must be unchanged");
        assert!(
            !s.is_dirty(),
            "a refused edit must not mark the buffer dirty"
        );
        assert!(
            !s.status().is_empty(),
            "status explains why the edit was refused"
        );
    }

    #[test]
    fn nudge_on_center_is_refused_leaves_layout_unchanged_and_does_not_dirty() {
        let mut s = state();
        s.on_key(KeyKind::Right); // Left -> Center
        let before = s.layout().clone();

        s.on_key(KeyKind::NudgeDown);
        assert_eq!(s.layout(), &before);
        assert!(!s.is_dirty());
        assert!(!s.status().is_empty());

        s.on_key(KeyKind::NudgeUp);
        assert_eq!(s.layout(), &before);
        assert!(!s.is_dirty());
    }

    #[test]
    fn placing_from_available_into_center_is_refused() {
        // The direct path to this state is `focus AVAILABLE with CENTER as
        // the last focused region`. Column::ALL's fixed adjacency
        // ([Left, Center, Right, Available]) means normal per-key navigation
        // always passes through Right immediately before reaching Available,
        // which overwrites `last_region` to `Right` — so this exact state
        // isn't reachable via keys today. Set the fields directly (legal:
        // this test module is a descendant of widget_tui, so it shares
        // privacy with EditorState's fields) so the guard is proven correct
        // regardless of how focus got there, and stays correct if a future
        // change (a "jump to column" key, a reordered Column::ALL) makes the
        // path reachable.
        let mut s = state();
        s.column = Column::Available;
        s.last_region = Column::Center;
        assert_eq!(s.selected(), Some("disk"));
        let before = s.layout().clone();
        s.on_key(KeyKind::Space);
        assert_eq!(s.layout(), &before, "center layout must be unchanged");
        assert!(
            !s.is_dirty(),
            "a refused edit must not mark the buffer dirty"
        );
        assert!(
            !s.status().is_empty(),
            "status explains why the edit was refused"
        );
    }

    #[test]
    fn preview_renders_builtins_and_shows_plugins_as_a_static_chip() {
        let layout = Layout {
            left: vec!["hostname".into()],
            center: vec![],
            right: vec!["weather".into()],
        };
        let catalog = vec![
            WidgetPlacement {
                name: "hostname".into(),
                summary: "host".into(),
                source: WidgetSource::Builtin,
                placement: None,
            },
            WidgetPlacement {
                name: "weather".into(),
                summary: "weather".into(),
                source: WidgetSource::Plugin,
                placement: None,
            },
        ];
        let state = EditorState::new(layout, catalog);
        let cfg = Config::default();
        let registry = Registry::with_builtins(&cfg);
        let theme = Theme::default();
        let regions = preview_regions(&state, &cfg, &theme, &registry);
        let text: String = regions
            .iter()
            .flat_map(|(_, line)| line.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        // The built-in rendered its real text; the plugin is a placeholder chip and
        // was never instantiated.
        assert!(text.contains("[weather]"), "plugin chip present: {text}");
        assert!(!text.is_empty());
    }

    /// The bug this module was reported for: the preview handed ratatui a
    /// string full of ANSI escapes, which a cell buffer draws as literal
    /// characters (it never interprets them) — so the bar appeared as raw
    /// `[38;2;...m` noise with no colour. The preview must carry ratatui
    /// styles instead, and no span's *text* may contain an ESC byte.
    #[test]
    fn preview_carries_ratatui_styles_and_never_literal_escape_bytes() {
        let layout = Layout {
            left: vec!["hostname".into()],
            center: vec![],
            right: vec![],
        };
        let catalog = vec![WidgetPlacement {
            name: "hostname".into(),
            summary: "host".into(),
            source: WidgetSource::Builtin,
            placement: None,
        }];
        let state = EditorState::new(layout, catalog);
        let cfg = Config::default();
        let registry = Registry::with_builtins(&cfg);
        let theme = Theme::default();

        let regions = preview_regions(&state, &cfg, &theme, &registry);
        let spans: Vec<_> = regions
            .iter()
            .flat_map(|(_, line)| line.spans.iter())
            .collect();
        assert!(!spans.is_empty(), "the preview rendered something");

        for span in &spans {
            assert!(
                !span.content.contains('\x1b'),
                "no raw escape byte reaches a ratatui cell: {:?}",
                span.content
            );
            assert!(
                !span.content.contains("38;2;") && !span.content.contains("38;5;"),
                "no SGR parameter text reaches a ratatui cell: {:?}",
                span.content
            );
        }
        // And the styling actually landed somewhere, rather than every span
        // being unstyled text.
        assert!(
            spans
                .iter()
                .any(|s| s.style.fg.is_some() || s.style.bg.is_some()),
            "at least one span carries a real ratatui colour"
        );
    }

    /// End-to-end at the level the bug was actually seen: render a real frame
    /// and inspect the resulting cells. The unit test above pins the spans;
    /// this pins that `draw` wires them through, so styling survives all the
    /// way into the buffer and no escape text is ever drawn.
    #[test]
    fn drawn_frame_has_colored_preview_cells_and_no_escape_text() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let layout = Layout {
            left: vec!["hostname".into()],
            center: vec![],
            right: vec!["cwd".into()],
        };
        let catalog = vec![
            WidgetPlacement {
                name: "hostname".into(),
                summary: "host".into(),
                source: WidgetSource::Builtin,
                placement: None,
            },
            WidgetPlacement {
                name: "cwd".into(),
                summary: "cwd".into(),
                source: WidgetSource::Builtin,
                placement: None,
            },
        ];
        let state = EditorState::new(layout, catalog);
        let cfg = Config::default();
        let registry = Registry::with_builtins(&cfg);
        let theme = Theme::default();

        let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
        term.draw(|f| draw(f, &state, &cfg, &theme, &registry))
            .unwrap();
        let buf = term.backend().buffer();

        let whole: String = (0..12)
            .flat_map(|y| (0..120).map(move |x| (x, y)))
            .map(|(x, y)| buf[(x, y)].symbol().to_string())
            .collect();
        assert!(
            !whole.contains("38;2;") && !whole.contains("38;5;") && !whole.contains("[0m"),
            "no SGR text was drawn into any cell"
        );

        // Some cell in the frame carries a real background colour — the
        // powerline bar. Before the fix every cell was Color::Reset.
        let colored = (0..12)
            .flat_map(|y| (0..120).map(move |x| (x, y)))
            .any(|(x, y)| {
                let s = buf[(x, y)].style();
                s.bg.is_some_and(|c| c != ratatui::style::Color::Reset)
            });
        assert!(colored, "the preview painted at least one colored cell");
    }

    #[test]
    fn preview_stacks_only_when_the_regions_do_not_fit() {
        // Three 10-wide regions need 30 + two 3-wide gaps = 36 columns.
        let widths = vec![10, 10, 10];
        assert_eq!(preview_layout(&widths, 36), PreviewLayout::SideBySide);
        assert_eq!(preview_layout(&widths, 35), PreviewLayout::Stacked);
        assert_eq!(preview_layout(&widths, 200), PreviewLayout::SideBySide);
    }

    #[test]
    fn preview_single_region_never_stacks_and_needs_no_gap() {
        // One region has no separator to pay for, so it stays side-by-side
        // even when it overflows — stacking one line onto one line is no help.
        assert_eq!(preview_layout(&[80], 20), PreviewLayout::SideBySide);
        assert_eq!(preview_layout(&[], 20), PreviewLayout::SideBySide);
    }

    #[test]
    fn preview_height_is_one_line_side_by_side_and_one_per_region_stacked() {
        assert_eq!(preview_height(PreviewLayout::SideBySide, 3), 1);
        assert_eq!(preview_height(PreviewLayout::Stacked, 3), 3);
        assert_eq!(preview_height(PreviewLayout::Stacked, 2), 2);
        // Never collapses to zero rows, even with nothing to show.
        assert_eq!(preview_height(PreviewLayout::Stacked, 0), 1);
    }

    #[test]
    fn preview_width_counts_display_columns_not_bytes() {
        // A powerline separator is 3 bytes but one column; measuring bytes
        // would stack a bar that fits perfectly well.
        let line = Line::from(vec![Span::raw("ab"), Span::raw("\u{e0b0}")]);
        assert_eq!(line_width(&line), 3);
        assert!(
            "ab\u{e0b0}".len() > line_width(&line),
            "byte length really does overstate this"
        );
    }

    /// I4: a layout can name a widget `widget_placements` doesn't otherwise
    /// recognize (e.g. a plugin whose `.wasm` is gone) — `widget_placements`
    /// now surfaces it as `WidgetSource::Unknown` in the catalog the TUI is
    /// built from, rather than silently omitting it. This pins the editor's
    /// decision for what happens to it: removing it returns it to AVAILABLE
    /// (never lost) tagged `(unknown)`, and from there it can be re-placed
    /// exactly like any other widget — `layout_enable` never checks catalog
    /// membership.
    #[test]
    fn removing_a_placed_but_unknown_widget_returns_it_to_available_for_re_placement() {
        let layout = Layout {
            left: vec!["pane_id".into()],
            center: vec!["windows".into()],
            right: vec!["ghostwidget".into()],
        };
        let mut catalog = vec![placement("pane_id"), placement("windows")];
        catalog.push(WidgetPlacement {
            name: "ghostwidget".to_string(),
            summary: "placed but not a recognized widget (unknown)".to_string(),
            source: WidgetSource::Unknown,
            placement: Some((Region::Right, 0)),
        });
        let mut s = EditorState::new(layout, catalog);

        s.on_key(KeyKind::Right); // Left -> Center
        s.on_key(KeyKind::Right); // Center -> Right, cursor on "ghostwidget"
        assert_eq!(s.selected(), Some("ghostwidget"));

        s.on_key(KeyKind::Space); // remove it
        assert!(s.layout().right.is_empty(), "removed from its region");
        assert_eq!(
            s.column_items(Column::Available),
            ["ghostwidget"],
            "not lost -- back in AVAILABLE"
        );
        assert_eq!(
            s.source_of("ghostwidget"),
            Some(&WidgetSource::Unknown),
            "still tagged unknown, not silently reclassified"
        );

        // And it can be re-placed exactly like any other AVAILABLE widget.
        s.on_key(KeyKind::Right); // Right -> Available
        assert_eq!(s.selected(), Some("ghostwidget"));
        s.on_key(KeyKind::Space);
        assert_eq!(s.layout().right, ["ghostwidget"]);
    }
}
