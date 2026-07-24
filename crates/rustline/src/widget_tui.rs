//! The interactive widget editor: a pure state machine ([`EditorState`]) plus
//! a thin ratatui draw loop (Task 5). Everything interesting — focus, add and
//! remove, reorder, dirty tracking, quit confirmation — lives in `on_key`,
//! which takes a [`KeyKind`] and returns an [`EditorAction`]. No terminal is
//! involved, so the behavior is unit-tested directly, the same split
//! `theme_cmd.rs`'s reader/writer-generic `run_picker` uses.
//!
//! This module's public surface (`EditorState`'s accessors, `Column::title`,
//! `KeyKind::{Help,Other}`, `EditorAction`, `mark_write_failed`, …) is the
//! contract the not-yet-written ratatui draw loop consumes; today it's
//! exercised only by the `#[cfg(test)]` module below, which reads as dead
//! code to a plain (non-test) build of this bin crate. Allowed narrowly here
//! rather than per-item because rustc collapses per-item dead-code
//! diagnostics into a single "enum/struct is never used" once the whole type
//! is unreachable from `main`, so per-item `#[expect(dead_code)]` bounces.
//! Remove this allow once Task 5 wires `run` up to `EditorState`.
#![allow(dead_code)]

use std::path::Path;

use rustline_core::{
    Layout, Region, WidgetPlacement, WidgetSource, layout_disable, layout_enable, layout_nudge,
};

/// The four focusable columns: the three layout regions plus the pool of
/// widgets that aren't currently placed.
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
            Column::Center => "CENTER",
            Column::Right => "RIGHT",
            Column::Available => "AVAILABLE",
        }
    }
}

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

    /// Space: AVAILABLE → append to the last focused region; a region → back
    /// to AVAILABLE.
    fn toggle_selected(&mut self) {
        let Some(name) = self.selected().map(str::to_string) else {
            return;
        };
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

pub fn run(_config_path: &Path, _plugin_dir: &Path) -> i32 {
    eprintln!("the widget editor is not implemented yet");
    1
}

#[cfg(test)]
mod tests {
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
}
