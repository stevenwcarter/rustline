//! Git branch/status read, isolated at the `Context`-build edge, mirroring
//! `battery.rs`/`cpu.rs`/`memory.rs`: `read_git` shells out and `parse_git_status`
//! is a pure parser, unit-tested independently of any real repository.

use rustline_core::GitInfo;

/// Read the git branch/status for `path` via `git status --porcelain=v2
/// --branch`, or `None` on any failure: `git` missing from `PATH`, `path` not
/// inside a repository, or a non-zero exit — never a fabricated "clean"
/// reading (invariant #6). Called once at Context-build time, only when the
/// `git` widget is in the active layout (see `build_context.rs`).
pub fn read_git(path: &str) -> Option<GitInfo> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain=v2", "--branch"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    Some(parse_git_status(&stdout))
}

/// Parse `git status --porcelain=v2 --branch` stdout into a [`GitInfo`]. Pure;
/// unit-tested directly against fixture text, independent of any real
/// repository.
///
/// Counting rules:
/// - `branch`: the `# branch.head` value, or the 7-character short SHA taken
///   from `# branch.oid` when `HEAD` is detached (`# branch.head (detached)`).
/// - `ahead`/`behind`: from `# branch.ab +<ahead> -<behind>`; `0`/`0` when the
///   line is absent (no upstream configured).
/// - `staged`: the number of `1 <XY>…`/`2 <XY>…` (ordinary/renamed) entries
///   whose X (index) column is not `.`.
/// - `unstaged`: the number of `1 <XY>…`/`2 <XY>…` entries whose Y (worktree)
///   column is not `.`, plus the number of `? <path>` (untracked) entries.
/// - `u <XY>…` (unmerged/conflicted) entries count toward BOTH `staged` and
///   `unstaged` — a conflict touches both the index and the worktree, so it
///   doesn't cleanly belong to just one.
/// - `! <path>` (ignored) entries are never counted.
pub fn parse_git_status(out: &str) -> GitInfo {
    let mut branch = String::new();
    let mut short_oid = String::new();
    let mut detached = false;
    let mut ahead = 0u32;
    let mut behind = 0u32;
    let mut staged = 0u32;
    let mut unstaged = 0u32;

    for line in out.lines() {
        if let Some(oid) = line.strip_prefix("# branch.oid ") {
            short_oid = oid.chars().take(7).collect();
        } else if let Some(head) = line.strip_prefix("# branch.head ") {
            if head == "(detached)" {
                detached = true;
            } else {
                branch = head.to_string();
            }
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut parts = ab.split_whitespace();
            ahead = parts
                .next()
                .and_then(|a| a.strip_prefix('+'))
                .and_then(|a| a.parse().ok())
                .unwrap_or(0);
            behind = parts
                .next()
                .and_then(|b| b.strip_prefix('-'))
                .and_then(|b| b.parse().ok())
                .unwrap_or(0);
        } else if let Some(entry) = line.strip_prefix("1 ").or_else(|| line.strip_prefix("2 ")) {
            let mut xy = entry.split_whitespace().next().unwrap_or("").chars();
            let x = xy.next().unwrap_or('.');
            let y = xy.next().unwrap_or('.');
            if x != '.' {
                staged += 1;
            }
            if y != '.' {
                unstaged += 1;
            }
        } else if line.starts_with("u ") {
            staged += 1;
            unstaged += 1;
        } else if line.starts_with("? ") {
            unstaged += 1;
        }
        // `! <path>` (ignored) entries are intentionally not matched/counted.
    }

    GitInfo {
        branch: if detached { short_oid } else { branch },
        ahead,
        behind,
        staged,
        unstaged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_branch_with_upstream_and_zero_ahead_behind() {
        let out = "# branch.oid abc123def456\n\
                   # branch.head main\n\
                   # branch.upstream origin/main\n\
                   # branch.ab +0 -0\n";
        let info = parse_git_status(out);
        assert_eq!(info.branch, "main");
        assert_eq!(info.ahead, 0);
        assert_eq!(info.behind, 0);
        assert_eq!(info.staged, 0);
        assert_eq!(info.unstaged, 0);
    }

    #[test]
    fn ahead_and_behind_counts() {
        let out = "# branch.oid abc123def456\n\
                   # branch.head main\n\
                   # branch.upstream origin/main\n\
                   # branch.ab +2 -1\n";
        let info = parse_git_status(out);
        assert_eq!(info.ahead, 2);
        assert_eq!(info.behind, 1);
    }

    #[test]
    fn staged_unstaged_and_untracked_counts() {
        let out = "# branch.oid abc123def456\n\
                   # branch.head main\n\
                   1 M. N... file1\n\
                   1 .M N... file2\n\
                   ? file3\n";
        let info = parse_git_status(out);
        assert_eq!(info.staged, 1);
        assert_eq!(info.unstaged, 2);
    }

    #[test]
    fn detached_head_uses_short_sha_as_branch() {
        let out = "# branch.oid abc123def456789\n# branch.head (detached)\n";
        let info = parse_git_status(out);
        assert_eq!(info.branch, "abc123d");
        assert_eq!(info.ahead, 0);
        assert_eq!(info.behind, 0);
    }

    #[test]
    fn no_ab_line_defaults_ahead_behind_to_zero() {
        // No upstream configured: the `# branch.ab` line is entirely absent.
        let out = "# branch.oid abc123def456\n# branch.head main\n";
        let info = parse_git_status(out);
        assert_eq!(info.ahead, 0);
        assert_eq!(info.behind, 0);
    }

    #[test]
    fn renamed_entry_and_ignored_entry() {
        let out = "# branch.oid abc123def456\n\
                   # branch.head main\n\
                   2 R. N... 100644 100644 100644 hash1 hash2 R100 new.txt\told.txt\n\
                   ! build/\n";
        let info = parse_git_status(out);
        // Renamed with staged marker R -> staged; Y is '.' -> not unstaged.
        assert_eq!(info.staged, 1);
        assert_eq!(info.unstaged, 0);
    }

    #[test]
    fn unmerged_entry_counts_as_both_staged_and_unstaged() {
        let out = "# branch.oid abc123def456\n\
                   # branch.head main\n\
                   u UU N... 100644 100644 100644 100644 h1 h2 h3 conflicted.txt\n";
        let info = parse_git_status(out);
        assert_eq!(info.staged, 1);
        assert_eq!(info.unstaged, 1);
    }

    #[test]
    fn empty_output_is_all_zero_and_empty_branch() {
        let info = parse_git_status("");
        assert_eq!(info.branch, "");
        assert_eq!(info.ahead, 0);
        assert_eq!(info.behind, 0);
        assert_eq!(info.staged, 0);
        assert_eq!(info.unstaged, 0);
    }

    #[test]
    fn read_git_missing_repo_or_binary_is_none() {
        // Whether `git` is on PATH or not, a nonexistent directory can never
        // resolve to a repository: `-C` fails, so this must always be `None`.
        assert!(read_git("/nonexistent/path/that/does/not/exist").is_none());
    }

    #[test]
    fn read_git_never_panics_in_this_repo() {
        // Host-dependent (requires `git` on PATH); only assert it doesn't
        // panic and, when Some, the shape is sane. `CARGO_MANIFEST_DIR` is
        // this crate's dir, which is inside the rustline git repository.
        let dir = env!("CARGO_MANIFEST_DIR");
        if let Some(info) = read_git(dir) {
            assert!(!info.branch.is_empty());
        }
    }
}
