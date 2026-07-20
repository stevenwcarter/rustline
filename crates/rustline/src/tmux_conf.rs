//! The tmux config block `rustline init` prints, wiring `rustline render`
//! into `status-left`/`status-right`/`window-status-format` and refreshing
//! the client on pane/window changes.

/// The tmux config snippet that wires `rustline` into `status-left`,
/// `status-right`, and the window list, plus the hooks that keep the status
/// line refreshing promptly on pane/window switches.
pub fn init_block() -> String {
    r##"# rustline statusline
set -g status on
set -g status-interval 1
set -g status-left-length 100
set -g status-right-length 200
set -g status-left  "#(rustline render left --session '#{session_name}' --window '#{window_index}' --pane '#{pane_index}' --pane-path '#{pane_current_path}')"
set -g status-right "#(rustline render right --pane-path '#{pane_current_path}')"
set -g window-status-separator ""
setw -g window-status-format         "#(rustline render window '#{window_index}' '#{window_name}' '#{window_flags}')"
setw -g window-status-current-format "#(rustline render window --current '#{window_index}' '#{window_name}' '#{window_flags}')"
set-hook -g after-select-pane   "refresh-client -S"
set-hook -g after-select-window "refresh-client -S"
"##
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_block_wires_all_regions_and_hooks() {
        let b = init_block();
        assert!(b.contains("status-interval 1"));
        assert!(b.contains("#(rustline render left"));
        assert!(b.contains("#(rustline render right"));
        assert!(b.contains("rustline render window"));
        assert!(b.contains("after-select-pane"));
        assert!(b.contains("refresh-client -S"));
    }
}
