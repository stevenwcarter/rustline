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
use ratatui::style::{Modifier, Style as UiStyle};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::{Terminal, backend::CrosstermBackend};
use rustline_core::{
    Config, Layout, Region, Registry, WidgetPlacement, WidgetSource, layout_disable, layout_enable,
    layout_nudge, render_named_region, tmux_to_ansi,
};
use toml_edit::DocumentMut;

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
            Some(region) => self.layout.get(region).to_vec(),
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
            Some(region) => self.layout.get(region).get(index).map(String::as_str),
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
    /// directly from it, or adding into it from AVAILABLE (when CENTER was
    /// the last focused region before AVAILABLE). Both are refused; see
    /// [`CENTER_FIXED_STATUS`].
    fn targets_center(&self) -> bool {
        self.column == Column::Center
            || (self.column == Column::Available && self.last_region == Column::Center)
    }

    /// Space: AVAILABLE → append to the last focused region; a region → back
    /// to AVAILABLE.
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

    fn clamp_cursor(&mut self) {
        let len = self.column_items(self.column).len();
        let clamped = self.cursor().min(len.saturating_sub(1));
        self.set_cursor(clamped);
    }
}

/// Map a crossterm key event onto the editor's own [`KeyKind`]. Arrow keys and
/// their vim equivalents are interchangeable; `J`/`K` (shifted, however the
/// terminal reports it) reorder.
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
        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::SHIFT) => KeyKind::NudgeDown,
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::SHIFT) => KeyKind::NudgeUp,
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
fn preview_line(state: &EditorState, cfg: &Config) -> String {
    let theme = crate::resolve_theme(cfg);
    let ctx = crate::sample_context::sample_context(false);
    let registry = Registry::with_builtins(cfg);
    let overrides = cfg.color_overrides();
    let mut parts: Vec<String> = Vec::new();
    for region in Region::ALL {
        let all = state.layout().get(region);
        // Only names the registry knows render for real; anything else (a
        // plugin stem, an unresolvable instance) becomes a chip.
        let names: Vec<String> = all
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
        let markup = render_named_region(dir, &names, &ctx, &registry, &theme, &overrides);
        let mut rendered = tmux_to_ansi(&markup);
        if !chips.is_empty() {
            rendered.push(' ');
            rendered.push_str(&chips.join(" "));
        }
        if !rendered.trim().is_empty() {
            parts.push(rendered);
        }
    }
    parts.join("   ")
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
const HELP_LINE: &str = "←→ region  ↑↓ select  space add/remove  J/K reorder  w write  q quit";

/// The fuller hint shown while `?` help is toggled on, appended above the
/// footer.
const HELP_DETAIL: &str =
    "vim keys: h j k l   Enter also adds/removes   Esc also quits   ? toggles this line";

/// Render one frame. Pure with respect to `state`/`cfg` — it reads
/// [`EditorState::columns`], [`EditorState::column`],
/// [`EditorState::cursor_index`], [`EditorState::status`],
/// [`EditorState::show_help`], [`EditorState::confirming_quit`], and
/// [`EditorState::is_dirty`] (for the footer's `[modified]` marker), and
/// never mutates anything.
fn draw(frame: &mut ratatui::Frame, state: &EditorState, cfg: &Config) {
    let help_height = u16::from(state.show_help());
    let rows = UiLayout::default()
        .direction(LayoutDirection::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(1), // preview strip
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

    frame.render_widget(Paragraph::new(preview_line(state, cfg)), rows[1]);

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
    let catalog = rustline_core::widget_placements(
        &cfg,
        Registry::with_builtins(&cfg).descriptors(),
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
        if terminal.draw(|frame| draw(frame, &state, &cfg)).is_err() {
            return 1;
        }
        let Ok(Event::Key(key)) = event::read() else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match state.on_key(map_key(key)) {
            EditorAction::Redraw | EditorAction::ConfirmQuit => {}
            EditorAction::Write => {
                match crate::widget_cmd::write_layout(&mut doc, state.layout()) {
                    Ok(()) => match std::fs::write(config_path, doc.to_string()) {
                        Ok(()) => state.mark_written(),
                        Err(error) => state.mark_write_failed(&error.to_string()),
                    },
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
        let line = preview_line(&state, &Config::default());
        // The built-in rendered its real text; the plugin is a placeholder chip and
        // was never instantiated.
        assert!(line.contains("[weather]"), "plugin chip present: {line}");
        assert!(!line.is_empty());
    }
}
