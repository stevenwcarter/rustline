//! The tmux config block `rustline init` prints, wiring `rustline render`
//! into `status-left`/`status-right`/`window-status-format` and refreshing
//! the client on pane/window changes.

use std::fmt::Write as _;

/// The tmux config snippet that wires `rustline` into `status-left`,
/// `status-right`, and the window list, plus the hooks that keep the status
/// line refreshing promptly on pane/window switches.
///
/// `bar_bg`/`fg` are the effective theme's background/foreground as tmux color
/// specs (from [`rustline_core::Color::to_tmux`]); they set `status-style` so
/// the powerline edges sit on the theme's bar background rather than tmux's
/// default green.
///
/// # Injection safety
///
/// tmux expands `#{...}` format variables *before* handing the `#(...)` body to
/// `/bin/sh`, and does not shell-escape them. An untrusted value — a window
/// title or `pane_current_path`, both settable via terminal escape sequences —
/// could otherwise inject shell commands that run every `status-interval`. So
/// every interpolated variable is wrapped in tmux's `#{q:...}` quoting modifier
/// and passed in `--flag=value` form (no surrounding quotes, no positional
/// args): tmux escapes the value into a single literal shell token, and the
/// `=` form keeps an empty value present rather than shifting later args.
pub fn init_block(bar_bg: &str, fg: &str) -> String {
    let mut block = String::from(
        "# rustline statusline\n\
         set -g status on\n\
         set -g status-interval 1\n\
         set -g status-justify centre\n",
    );
    let _ = writeln!(block, "set -g status-style bg={bar_bg},fg={fg}");
    block.push_str(
        r##"set -g status-left-length 100
set -g status-right-length 200
set -g status-left  "#(rustline render left --session=#{q:session_name} --window=#{q:window_index} --pane=#{q:pane_index} --pane-path=#{q:pane_current_path})"
set -g status-right "#(rustline render right --session=#{q:session_name} --window=#{q:window_index} --pane=#{q:pane_index} --pane-path=#{q:pane_current_path})"
set -g window-status-separator ""
setw -g window-status-format         "#(rustline render window --index=#{q:window_index} --name=#{q:window_name} --flags=#{q:window_flags})"
setw -g window-status-current-format "#(rustline render window --current --index=#{q:window_index} --name=#{q:window_name} --flags=#{q:window_flags})"
set-hook -g after-select-pane   "refresh-client -S"
set-hook -g after-select-window "refresh-client -S"
"##,
    );
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_block_wires_all_regions_and_hooks() {
        let b = init_block("colour234", "colour255");
        assert!(b.contains("status-interval 1"));
        assert!(b.contains("#(rustline render left"));
        assert!(b.contains("#(rustline render right"));
        assert!(b.contains("rustline render window"));
        assert!(b.contains("after-select-pane"));
        assert!(b.contains("refresh-client -S"));
    }

    #[test]
    fn init_block_escapes_untrusted_vars_and_sets_status_style() {
        let b = init_block("colour234", "colour255");
        // Untrusted tmux vars must be q-escaped (tmux `#{q:...}`) so a malicious
        // window title or path can't break out of the `#(...)` shell command.
        assert!(b.contains("#{q:window_name}"), "window_name q-escaped: {b}");
        assert!(
            b.contains("#{q:pane_current_path}"),
            "pane_current_path q-escaped: {b}"
        );
        // ...and never interpolated inside bare single quotes (the old, injectable form).
        assert!(!b.contains("'#{window_name}'"), "no bare quoted var: {b}");
        assert!(
            !b.contains("'#{pane_current_path}'"),
            "no bare quoted var: {b}"
        );
        // `--flag=value` form keeps each value a single shell arg (no positional shift).
        assert!(b.contains("--name=#{q:window_name}"), "=-form name: {b}");
        assert!(
            b.contains("--pane-path=#{q:pane_current_path}"),
            "=-form pane-path: {b}"
        );
        // The status background is set so powerline edges don't float on tmux's
        // default green, and the window list is centered.
        assert!(
            b.contains("set -g status-style bg=colour234"),
            "status-style bg: {b}"
        );
        assert!(b.contains("status-justify centre"), "centered: {b}");
    }
}
