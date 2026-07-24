//! Batched window-list read: `tmux list-windows -F` behind the pure
//! `parse_list_windows`, mirroring `git.rs`'s tool-shell-out pattern.

use std::process::Command;

use rustline_core::WindowCtx;

/// Enumerate a tmux session's windows via `tmux list-windows -F`. A `None`
/// session lists the server's current session. Empty Vec on ANY failure (tmux
/// missing, bad session, non-zero exit) — never a panic, never a fabricated
/// window (invariant: never break the bar).
#[allow(dead_code)] // wired in Task 2
pub fn read_windows(session: Option<&str>) -> Vec<WindowCtx> {
    let mut cmd = Command::new("tmux");
    cmd.arg("list-windows");
    if let Some(s) = session {
        cmd.args(["-t", s]);
    }
    // Name LAST so a tab inside it can't misalign earlier fields (splitn(4)).
    cmd.args([
        "-F",
        "#{window_index}\t#{window_active}\t#{window_flags}\t#{window_name}",
    ]);
    let Ok(out) = cmd.output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_list_windows(&String::from_utf8_lossy(&out.stdout))
}

/// Pure parse of `list-windows -F` output. Tolerant: a line missing any of the
/// three leading tab fields is skipped; the name is the `splitn(4)` remainder.
fn parse_list_windows(s: &str) -> Vec<WindowCtx> {
    s.lines()
        .filter_map(|line| {
            let mut it = line.splitn(4, '\t');
            let index = it.next()?.to_string();
            let active = it.next()?;
            let flags = it.next()?.to_string();
            let name = it.next().unwrap_or("").to_string();
            Some(WindowCtx {
                index,
                name,
                flags,
                is_current: active == "1",
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_list_windows_parses_fields_and_active() {
        let out = "0\t1\t*\tshell\n1\t0\t-\tmy editor\n";
        let ws = parse_list_windows(out);
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[0].index, "0");
        assert!(ws[0].is_current, "active=1 → is_current");
        assert_eq!(ws[0].flags, "*");
        assert_eq!(ws[0].name, "shell");
        assert_eq!(ws[1].name, "my editor", "name may contain spaces");
        assert!(!ws[1].is_current);
    }

    #[test]
    fn parse_list_windows_name_may_contain_tab_and_skips_malformed() {
        // splitn(4) makes the name the remainder, so a tab inside it survives.
        let out = "2\t0\t-\tna\tme\nBADLINE\n";
        let ws = parse_list_windows(out);
        assert_eq!(ws.len(), 1, "malformed line skipped: {ws:?}");
        assert_eq!(ws[0].name, "na\tme");
    }

    #[test]
    fn parse_list_windows_empty_is_empty() {
        assert!(parse_list_windows("").is_empty());
    }
}
