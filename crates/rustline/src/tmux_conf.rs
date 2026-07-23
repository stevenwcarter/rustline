//! The tmux config block `rustline init` prints, wiring `rustline render`
//! into `status-left`/`status-right`/`window-status-format` and refreshing
//! the client on pane/window changes.

use std::fmt::Write as _;

/// Options controlling the tmux block `rustline init` emits.
///
/// `two_line` renders the window list on its own line above status-left/right
/// (the author's layout); `mouse` adds `set -g mouse on` so click-to-toggle
/// works out of the box; `interval` sets `status-interval`; `binary` is the
/// resolved path substituted for every `#(...)` call (see [`init_block`]'s
/// injection-safety note).
pub struct InitBlockOpts<'a> {
    pub bar_bg: &'a str,
    pub fg: &'a str,
    pub two_line: bool,
    pub mouse: bool,
    pub interval: u32,
    pub binary: &'a str,
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
/// `opts.binary` replaces the bare `rustline` in every `#(...)`/`run-shell`
/// call with the resolved absolute path to the binary (see the caller:
/// `std::env::current_exe()`, overridable with `init --binary`). tmux's `#()`
/// shells out via the *tmux server's* `/bin/sh`, whose `PATH` may not include
/// wherever the user installed `rustline` (e.g. `~/.local/bin`), so a bare
/// name can silently resolve to nothing and leave the bar empty.
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
///
/// `opts.binary` is not one of these untrusted tmux variables — it's a fixed
/// string the caller resolved itself — so it doesn't need (and can't use)
/// tmux's `#{q:...}` modifier. It still needs *shell* quoting, though: unlike
/// the rest of this codebase's install path, a binary path can contain a
/// space (e.g. `~/My Programs/rustline`), which would otherwise split into
/// two argv words for `/bin/sh`. [`shell_quote`] wraps it in single quotes
/// (escaping any embedded single quote), so it always reaches `/bin/sh` as
/// one token regardless of its contents.
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
    // `@BINARY@` is a placeholder substituted below (see this fn's doc
    // comment); it appears nowhere else in this block or in
    // STATUS_FORMAT_0/1, so the final blanket `.replace` can't misfire.
    block.push_str(
        r##"set -g status-left-length 100
set -g status-right-length 200
set -g status-left  "#(@BINARY@ render left --session=#{q:session_name} --window=#{q:window_index} --pane=#{q:pane_index} --pane-path=#{q:pane_current_path})"
set -g status-right "#(@BINARY@ render right --session=#{q:session_name} --window=#{q:window_index} --pane=#{q:pane_index} --pane-path=#{q:pane_current_path})"
set -g window-status-separator ""
setw -g window-status-format         "#(@BINARY@ render window --index=#{q:window_index} --name=#{q:window_name} --flags=#{q:window_flags})"
setw -g window-status-current-format "#(@BINARY@ render window --current --index=#{q:window_index} --name=#{q:window_name} --flags=#{q:window_flags})"
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
            run-shell "@BINARY@ click --range=#{q:mouse_status_range} --button=left"
            refresh-client -S
        }
    }
}
# rustline middle/right-click bindings ([widgets.*.click] actions; run/open_url).
# Middle/right have no default window-list action to preserve, so they just
# dispatch any non-empty range to `rustline click` (tmux button numbering:
# 2=middle, 3=right). Injection-safe like the left binding: #{q:...} + --flag=.
bind -T root MouseDown2Status {
    if -F "#{mouse_status_range}" {
        run-shell "@BINARY@ click --range=#{q:mouse_status_range} --button=middle"
        refresh-client -S
    }
}
bind -T root MouseDown3Status {
    if -F "#{mouse_status_range}" {
        run-shell "@BINARY@ click --range=#{q:mouse_status_range} --button=right"
        refresh-client -S
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
    block.replace("@BINARY@", &shell_quote(opts.binary))
}

/// Wrap `s` in single quotes for embedding in a `/bin/sh` command line,
/// escaping any embedded single quote with the standard `'\''` trick (close
/// the quote, emit an escaped literal `'`, reopen the quote). Used only for
/// [`InitBlockOpts::binary`]: a fixed, host-controlled string that still
/// needs *shell* quoting because it can contain spaces.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Marker lines bracketing the rustline-managed region in `~/.tmux.conf`, so
/// re-running `rustline init` replaces that region instead of appending a
/// duplicate.
pub const TMUX_BEGIN: &str = "# >>> rustline >>>";
pub const TMUX_END: &str = "# <<< rustline <<<";

/// Find an existing `TMUX_BEGIN..=TMUX_END` region in `s`, returning the exact
/// text before/after it. The region's own trailing newline (if present) is
/// folded into the split point rather than left in `after`, so a caller that
/// splices `before`/`wrapped`/`after` back together never accumulates an extra
/// blank line across repeated upserts. `None` if no complete region is found.
fn find_region(s: &str) -> Option<(&str, &str)> {
    let b = s.find(TMUX_BEGIN)?;
    let e = s.find(TMUX_END)?;
    if e < b {
        return None;
    }
    let mut end = e + TMUX_END.len();
    if s[end..].starts_with('\n') {
        end += 1;
    }
    Some((&s[..b], &s[end..]))
}

/// Insert or replace the rustline-managed block in an existing `~/.tmux.conf`.
/// Idempotent: `upsert(upsert(x, b), b) == upsert(x, b)`. An existing region is
/// replaced strictly in place (the surrounding text is untouched, whatever its
/// whitespace), so re-running with an unchanged `block` is a true byte-for-byte
/// no-op. Only the no-markers-yet (first-time) case normalizes spacing: prior
/// content is separated from the newly appended block by exactly one blank line.
pub fn upsert_tmux_block(existing: &str, block: &str) -> String {
    let wrapped = format!(
        "{TMUX_BEGIN}\n{}\n{TMUX_END}\n",
        block.trim_end_matches('\n')
    );
    if let Some((before, after)) = find_region(existing) {
        return format!("{before}{wrapped}{after}");
    }
    let trimmed = existing.trim_end_matches('\n');
    if trimmed.trim().is_empty() {
        wrapped
    } else {
        format!("{trimmed}\n\n{wrapped}")
    }
}

/// Strip the rustline-managed block from an existing `~/.tmux.conf`, leaving
/// the surrounding text byte-identical (whatever its whitespace). A no-op —
/// returns `existing` unchanged — when no complete region is present, so it's
/// safe to call speculatively. Idempotent:
/// `remove(remove(x)) == remove(x)`.
pub fn remove_tmux_block(existing: &str) -> String {
    match find_region(existing) {
        Some((before, after)) => format!("{before}{after}"),
        None => existing.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixed test binary path — deterministic, so expected output never
    /// depends on the test process's own `current_exe()`.
    const TEST_BIN: &str = "/usr/bin/rustline";

    fn one_line<'a>(bar_bg: &'a str, fg: &'a str) -> InitBlockOpts<'a> {
        InitBlockOpts {
            bar_bg,
            fg,
            two_line: false,
            mouse: false,
            interval: 1,
            binary: TEST_BIN,
        }
    }

    #[test]
    fn one_line_default_matches_legacy_shape() {
        // Characterization: the one-line / mouse-off / interval-1 block matches
        // the legacy `rustline init` shape (pins the `alt_format`-defaults-stay-
        // empty seam at the tmux-block boundary — `--print` must not drift),
        // except the bare `rustline` binary name is now the resolved,
        // shell-quoted absolute path (see `block_uses_binary_path`).
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
        assert!(b.contains("#('/usr/bin/rustline' render left"));
        assert!(b.contains("MouseDown1Status"));
    }

    #[test]
    fn block_uses_binary_path() {
        // The resolved absolute binary path replaces the bare `rustline` in
        // every `#(...)`/`run-shell` call, single-quoted for the shell (it's a
        // fixed, host-controlled string, not a tmux `#{...}` var, so tmux's
        // `#{q:...}` quoting doesn't apply — see
        // `binary_quoting_is_independent_of_tmux_var_quoting`).
        let b = init_block(&one_line("colour234", "colour255"));
        assert!(b.contains("#('/usr/bin/rustline' render left"), "{b}");
        assert!(b.contains("#('/usr/bin/rustline' render right"), "{b}");
        assert!(b.contains("#('/usr/bin/rustline' render window"), "{b}");
        assert!(
            b.contains("run-shell \"'/usr/bin/rustline' click --range="),
            "{b}"
        );
        assert!(!b.contains("#(rustline render"), "no bare binary name: {b}");
        assert!(
            !b.contains("\"rustline click"),
            "click dispatch also uses the resolved path: {b}"
        );
    }

    #[test]
    fn binary_flag_overrides() {
        let mut o = one_line("colour234", "colour255");
        o.binary = "/opt/my rustline/rustline";
        let b = init_block(&o);
        // A path containing a space is single-quoted so /bin/sh sees one argv
        // word instead of splitting it.
        assert!(
            b.contains("#('/opt/my rustline/rustline' render left"),
            "{b}"
        );
        assert!(
            b.contains("run-shell \"'/opt/my rustline/rustline' click"),
            "{b}"
        );
    }

    #[test]
    fn binary_quoting_is_independent_of_tmux_var_quoting() {
        // invariant #4: only the *tmux* `#{...}` format variables need
        // `#{q:...}` (they carry untrusted, attacker-settable content like a
        // window title); the binary path is a value the caller resolved
        // itself and is shell-quoted instead. Both quoting mechanisms must
        // coexist unchanged in the same `#(...)` call.
        let b = init_block(&one_line("colour234", "colour255"));
        assert!(
            b.contains("#{q:session_name}"),
            "tmux var still q-escaped: {b}"
        );
        assert!(
            b.contains("#{q:window_name}"),
            "tmux var still q-escaped: {b}"
        );
        assert!(
            b.contains("--pane-path=#{q:pane_current_path}"),
            "=-form preserved: {b}"
        );
        assert!(
            b.contains("--range=#{q:mouse_status_range}"),
            "click range still q-escaped: {b}"
        );
    }

    #[test]
    fn shell_quote_escapes_embedded_single_quotes() {
        assert_eq!(shell_quote("/usr/bin/rustline"), "'/usr/bin/rustline'");
        assert_eq!(shell_quote("it's/rustline"), r"'it'\''s/rustline'");
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
        assert!(b.contains("#('/usr/bin/rustline' render left"));
    }

    #[test]
    fn two_line_formats_contain_no_shell_calls() {
        // invariant #4 in two-line mode: the status-format strings are pure tmux
        // format refs (#{...}/#[...]) into already-#{q:}-escaped options — never a
        // #(...) shell call. A later edit that sneaks one in must fail here.
        assert!(
            !STATUS_FORMAT_0.contains("#("),
            "STATUS_FORMAT_0 has no shell call"
        );
        assert!(
            !STATUS_FORMAT_1.contains("#("),
            "STATUS_FORMAT_1 has no shell call"
        );
    }

    #[test]
    fn init_block_wires_all_regions_and_hooks() {
        let b = init_block(&one_line("colour234", "colour255"));
        assert!(b.contains("status-interval 1"));
        assert!(b.contains("#('/usr/bin/rustline' render left"));
        assert!(b.contains("#('/usr/bin/rustline' render right"));
        assert!(b.contains("'/usr/bin/rustline' render window"));
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
        // dispatches to the resolved binary with the q-escaped range (invariant #4)
        assert!(
            b.contains("'/usr/bin/rustline' click --range=#{q:mouse_status_range}"),
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

    #[test]
    fn init_block_wires_middle_and_right_click_bindings() {
        // W36 click bindings ship live: middle/right mouse clicks on a status
        // range dispatch to `rustline click` with the matching `--button`
        // value, mirroring the left binding's injection-safe `#{q:...}` +
        // `--flag=` form (invariant #4). tmux button numbering: 1=left,
        // 2=middle, 3=right.
        let b = init_block(&one_line("colour234", "colour255"));
        assert!(b.contains("MouseDown2Status"), "binds middle click: {b}");
        assert!(b.contains("MouseDown3Status"), "binds right click: {b}");
        assert!(
            b.contains("'/usr/bin/rustline' click --range=#{q:mouse_status_range} --button=middle"),
            "middle dispatch q-escaped with --button=middle: {b}"
        );
        assert!(
            b.contains("'/usr/bin/rustline' click --range=#{q:mouse_status_range} --button=right"),
            "right dispatch q-escaped with --button=right: {b}"
        );
        // Never a bare, unescaped range in any binding.
        assert!(
            !b.contains("--range=#{mouse_status_range}"),
            "must q-escape every binding's range: {b}"
        );
        // Every binding refreshes the client so the change shows immediately.
        assert_eq!(
            b.matches("refresh-client -S").count(),
            5,
            "two after-select hooks + three MouseDown{{1,2,3}} bindings each refresh: {b}"
        );
    }

    #[test]
    fn upsert_appends_when_no_markers() {
        let out = upsert_tmux_block("set -g mouse on\n", "BLOCK");
        assert!(out.contains("set -g mouse on"), "keeps user content: {out}");
        assert!(out.contains(TMUX_BEGIN) && out.contains(TMUX_END));
        assert!(out.contains("\nBLOCK\n"), "wraps block: {out}");
    }

    #[test]
    fn upsert_into_empty_is_just_the_wrapped_block() {
        let out = upsert_tmux_block("", "BLOCK");
        assert_eq!(out, format!("{TMUX_BEGIN}\nBLOCK\n{TMUX_END}\n"));
    }

    #[test]
    fn upsert_replaces_existing_region_and_preserves_surroundings() {
        let first = upsert_tmux_block("user before\n", "OLD");
        let second = upsert_tmux_block(&first, "NEW");
        assert!(
            second.contains("user before"),
            "keeps content before markers"
        );
        assert!(
            second.contains("NEW") && !second.contains("OLD"),
            "replaced: {second}"
        );
        // exactly one marker pair
        assert_eq!(second.matches(TMUX_BEGIN).count(), 1);
        assert_eq!(second.matches(TMUX_END).count(), 1);
    }

    #[test]
    fn upsert_is_idempotent() {
        let once = upsert_tmux_block("user before\n", "BLOCK");
        let twice = upsert_tmux_block(&once, "BLOCK");
        assert_eq!(once, twice, "re-running with same block is a no-op");
    }

    #[test]
    fn remove_strips_region_leaves_surroundings_byte_identical() {
        let input = format!("before line\n{TMUX_BEGIN}\nBLOCK\n{TMUX_END}\nafter line\n");
        assert_eq!(remove_tmux_block(&input), "before line\nafter line\n");
    }

    #[test]
    fn remove_is_a_no_op_when_no_block() {
        let input = "just user content\nno markers here\n";
        let once = remove_tmux_block(input);
        assert_eq!(once, input, "unchanged when no block is present");
        let twice = remove_tmux_block(&once);
        assert_eq!(twice, once, "idempotent: a second call changes nothing");
    }

    #[test]
    fn remove_round_trips_upsert_first_insert_exact() {
        // upsert's first-insert path normalizes prior content to exactly one
        // blank line before the block, so starting from that already-
        // normalized shape means remove's output matches the original bytes
        // exactly. An input with a *different* trailing-whitespace shape
        // round-trips to the equivalent-but-reformatted shape, not
        // necessarily the original bytes -- that's upsert's documented
        // normalization, not a bug in `remove`.
        let original = "user before\n\n";
        let inserted = upsert_tmux_block(original, "BLOCK");
        assert_eq!(remove_tmux_block(&inserted), original);
    }

    #[test]
    fn remove_round_trips_via_replace_path_for_any_input() {
        // The replace path (a block already present) never touches
        // surrounding whitespace at all, so this round-trip is exact for
        // *any* input shape, regardless of which block content was inside.
        let first = upsert_tmux_block("user before\n", "OLD");
        let second = upsert_tmux_block(&first, "NEW");
        assert_eq!(remove_tmux_block(&second), remove_tmux_block(&first));
    }
}
