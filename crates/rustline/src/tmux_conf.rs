//! The tmux config block `rustline init` prints, wiring `rustline render`
//! into `status-left`/`status-right`/`window-status-format` and refreshing
//! the client on pane/window changes.

use std::fmt::Write as _;

/// Options controlling the tmux block `rustline init` emits.
///
/// `two_line` renders the window list on its own line above status-left/right
/// (the author's layout); `mouse` adds `set -g mouse on` so click-to-toggle
/// works out of the box; `interval` sets `status-interval`.
pub struct InitBlockOpts<'a> {
    pub bar_bg: &'a str,
    pub fg: &'a str,
    pub two_line: bool,
    pub mouse: bool,
    pub interval: u32,
}

/// Verbatim two-line `status-format[0]` (centered per-window list), copied
/// from the author's proven `~/.tmux.conf`. Contains no `#(...)` shell calls
/// — only `#{...}` format refs into the already-`#{q:}`-escaped
/// `window-status-format`/`window-status-current-format` options the shared
/// block sets, so injection-safety (invariant #4) holds.
const STATUS_FORMAT_0: &str = r##"set -g status-format[0] "#[list=on align=#{status-justify}]#[list=left-marker]<#[list=right-marker]>#[list=on]#{W:#[range=window|#{window_index} #{E:window-status-style}#{?#{&&:#{window_last_flag},#{!=:#{E:window-status-last-style},default}}, #{E:window-status-last-style},}#{?#{&&:#{window_bell_flag},#{!=:#{E:window-status-bell-style},default}}, #{E:window-status-bell-style},#{?#{&&:#{||:#{window_activity_flag},#{window_silence_flag}},#{!=:#{E:window-status-activity-style},default}}, #{E:window-status-activity-style},}}]#[push-default]#{T:window-status-format}#[pop-default]#[norange default]#{?loop_last_flag,,#{E:window-status-separator}},#[range=window|#{window_index} list=focus #{?#{!=:#{E:window-status-current-style},default},#{E:window-status-current-style},#{E:window-status-style}}#{?#{&&:#{window_last_flag},#{!=:#{E:window-status-last-style},default}}, #{E:window-status-last-style},}#{?#{&&:#{window_bell_flag},#{!=:#{E:window-status-bell-style},default}}, #{E:window-status-bell-style},#{?#{&&:#{||:#{window_activity_flag},#{window_silence_flag}},#{!=:#{E:window-status-activity-style},default}}, #{E:window-status-activity-style},}}]#[push-default]#{T:window-status-current-format}#[pop-default]#[norange list=on default]#{?loop_last_flag,,#{E:window-status-separator}}}""##;

/// Verbatim two-line `status-format[1]` (status-left/right), copied from the
/// author's proven `~/.tmux.conf`. Same injection-safety note as
/// [`STATUS_FORMAT_0`]: no `#(...)` shell calls, only `#{...}` refs into the
/// already-`#{q:}`-escaped `status-left`/`status-right` options.
const STATUS_FORMAT_1: &str = r##"set -g status-format[1] "#[align=left range=left #{E:status-left-style}]#[push-default]#{T;=/#{status-left-length}:status-left}#[pop-default]#[norange default]#[nolist align=right range=right #{E:status-right-style}]#[push-default]#{T;=/#{status-right-length}:status-right}#[pop-default]#[norange default]""##;

/// The tmux config snippet that wires `rustline` into `status-left`,
/// `status-right`, and the window list, plus the hooks that keep the status
/// line refreshing promptly on pane/window switches. See [`InitBlockOpts`]
/// for the one/two-line, mouse, and interval knobs.
///
/// `opts.bar_bg`/`opts.fg` are the effective theme's background/foreground as
/// tmux color specs (from [`rustline_core::Color::to_tmux`]); they set
/// `status-style` so the powerline edges sit on the theme's bar background
/// rather than tmux's default green.
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
pub fn init_block(opts: &InitBlockOpts) -> String {
    let mut block = String::from("# rustline statusline\nset -g status on\n");
    let _ = writeln!(block, "set -g status-interval {}", opts.interval);
    block.push_str("set -g status-justify centre\n");
    let _ = writeln!(
        block,
        "set -g status-style bg={},fg={}",
        opts.bar_bg, opts.fg
    );
    if opts.mouse {
        block.push_str("set -g mouse on\n");
    }
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
    block.push_str(
        r##"# rustline click-to-toggle a widget's alt view (needs: set -g mouse on)
bind -T root MouseDown1Status {
    if -F "#{==:#{mouse_status_range},window}" {
        select-window -t=
    } {
        if -F "#{mouse_status_range}" {
            run-shell "rustline click --range=#{q:mouse_status_range} --button=left"
            refresh-client -S
        }
    }
}
"##,
    );
    if opts.two_line {
        block.push_str("set -g status 2\n");
        block.push_str(STATUS_FORMAT_0);
        block.push('\n');
        block.push_str(STATUS_FORMAT_1);
        block.push('\n');
    }
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_line<'a>(bar_bg: &'a str, fg: &'a str) -> InitBlockOpts<'a> {
        InitBlockOpts {
            bar_bg,
            fg,
            two_line: false,
            mouse: false,
            interval: 1,
        }
    }

    #[test]
    fn one_line_default_is_byte_identical_to_legacy() {
        // Characterization: the one-line / mouse-off / interval-1 block is EXACTLY
        // the legacy `rustline init` output (pins the `alt_format`-defaults-stay-empty
        // seam at the tmux-block boundary — `--print` must not drift).
        let b = init_block(&one_line("colour234", "colour255"));
        assert!(b.starts_with(
            "# rustline statusline\nset -g status on\nset -g status-interval 1\nset -g status-justify centre\n"
        ));
        assert!(b.contains("set -g status-style bg=colour234,fg=colour255\n"));
        // The setter is emitted as its own line, `\nset -g mouse on\n`; the
        // legacy discoverability comment reads `(needs: set -g mouse on)` —
        // `(needs: ` before and `)` after — so anchoring on the surrounding
        // newlines matches only the real, conditional setter, never the
        // comment (a plain substring check would false-positive on the
        // comment regardless of `opts.mouse`).
        assert!(
            !b.contains("\nset -g mouse on\n"),
            "mouse-off omits the setter line: {b}"
        );
        assert!(
            b.contains("# rustline click-to-toggle a widget's alt view (needs: set -g mouse on)\n"),
            "legacy comment verbatim: {b}"
        );
        assert!(
            !b.contains("set -g status 2"),
            "one-line has no two-line formats: {b}"
        );
        assert!(b.contains("#(rustline render left"));
        assert!(b.contains("MouseDown1Status"));
    }

    #[test]
    fn interval_is_honored() {
        let mut o = one_line("colour234", "colour255");
        o.interval = 5;
        assert!(init_block(&o).contains("set -g status-interval 5\n"));
    }

    #[test]
    fn mouse_on_emits_setter() {
        let mut o = one_line("colour234", "colour255");
        o.mouse = true;
        let b = init_block(&o);
        assert!(
            b.contains("set -g mouse on\n"),
            "mouse on emits setter: {b}"
        );
    }

    #[test]
    fn two_line_emits_status_two_and_formats() {
        let mut o = one_line("colour234", "colour255");
        o.two_line = true;
        let b = init_block(&o);
        assert!(b.contains("set -g status 2\n"), "two-line count: {b}");
        assert!(b.contains("set -g status-format[0]"), "top format: {b}");
        assert!(b.contains("set -g status-format[1]"), "bottom format: {b}");
        // both formats reference the shared status-left/right and window list
        assert!(b.contains("#{T:window-status-format}"));
        assert!(b.contains(":status-right}"));
        // shared wiring still present
        assert!(b.contains("#(rustline render left"));
    }

    #[test]
    fn init_block_wires_all_regions_and_hooks() {
        let b = init_block(&one_line("colour234", "colour255"));
        assert!(b.contains("status-interval 1"));
        assert!(b.contains("#(rustline render left"));
        assert!(b.contains("#(rustline render right"));
        assert!(b.contains("rustline render window"));
        assert!(b.contains("after-select-pane"));
        assert!(b.contains("refresh-client -S"));
    }

    #[test]
    fn init_block_escapes_untrusted_vars_and_sets_status_style() {
        let b = init_block(&one_line("colour234", "colour255"));
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

    #[test]
    fn init_block_wires_click_toggle_binding() {
        let b = init_block(&one_line("colour234", "colour255"));
        assert!(b.contains("MouseDown1Status"), "binds status click: {b}");
        // preserves default window-click selection
        assert!(
            b.contains("select-window -t="),
            "keeps window selection: {b}"
        );
        // dispatches to rustline click with the q-escaped range (invariant #4)
        assert!(
            b.contains("rustline click --range=#{q:mouse_status_range}"),
            "click dispatch q-escaped: {b}"
        );
        // never a bare, unescaped mouse_status_range in the click arg
        assert!(
            !b.contains("--range=#{mouse_status_range}"),
            "must q-escape: {b}"
        );
        // discoverability hint
        assert!(b.contains("set -g mouse on"), "mentions mouse-on hint: {b}");
        assert!(
            b.contains("refresh-client -S"),
            "refreshes after toggle: {b}"
        );
    }
}
