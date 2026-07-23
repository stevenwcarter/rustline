//! `rustline doctor`: diagnoses the prerequisites documented in the README
//! (tmux >= 3.1, `set -g mouse on`, a truecolor terminal, `rustline` on
//! tmux's PATH, and the managed tmux-conf block) and reports each as
//! pass/warn/fail, alongside the resolved config/themes/plugin/log paths.
//!
//! Follows the same pure-parser / thin-I/O-shell split as `battery.rs`:
//! `parse_tmux_version`, `truecolor_from_env`, and `block_installed` are
//! pure and unit-tested directly; `run` is the I/O shell that spawns `tmux`,
//! reads env vars and `~/.tmux.conf`, and prints the report. A doctor run
//! never writes anything — it only reads and prints, like the other
//! stdout-is-for-humans commands (`theme list`, `plugin list`).

use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

/// Minimum tmux version rustline's click-to-toggle needs: status-line click
/// ranges and the `mouse_status_range` format variable were added in 3.1.
const MIN_TMUX_VERSION: (u32, u32) = (3, 1);

/// The outcome of one doctor check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

impl CheckStatus {
    fn label(self) -> &'static str {
        match self {
            CheckStatus::Ok => "ok",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "fail",
        }
    }
}

/// One diagnostic result: a named check, its status, and a human-readable detail.
struct Check {
    name: &'static str,
    status: CheckStatus,
    detail: String,
}

/// Resolved paths a doctor run checks and reports, resolved by the caller
/// the same way every other subcommand resolves them (`config_path`,
/// `themes_dir`, `resolve_plugin_dir`, `logging::log_path`, `tmux_conf_path`
/// in `main.rs`).
pub(crate) struct DoctorPaths<'a> {
    pub config: &'a Path,
    pub themes_dir: &'a Path,
    pub plugin_dir: &'a Path,
    pub log_file: &'a Path,
    pub tmux_conf: &'a Path,
}

/// Parse `tmux -V` output (e.g. `"tmux 3.4\n"`, `"tmux 3.1a"`,
/// `"tmux next-3.4"`) into its `(major, minor)` version. Finds the first
/// digit run and reads `major.minor` from there, tolerating a trailing
/// non-digit suffix on the minor component (the `a`/`b` patch letters some
/// tmux distros append). A string with no digits, or no `.`-separated minor,
/// is unparseable and yields `None`.
fn parse_tmux_version(output: &str) -> Option<(u32, u32)> {
    let start = output.find(|c: char| c.is_ascii_digit())?;
    let (major_str, minor_rest) = output[start..].split_once('.')?;
    let major = major_str.parse().ok()?;
    let minor_str: String = minor_rest
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let minor = minor_str.parse().ok()?;
    Some((major, minor))
}

/// True when `$COLORTERM` is `"truecolor"` or `"24bit"` (whitespace
/// tolerated) — the signal most terminal emulators use to advertise 24-bit
/// RGB support, which rustline's six curated themes (everything but
/// `default`) rely on.
fn truecolor_from_env(colorterm: Option<&str>) -> bool {
    matches!(colorterm.map(str::trim), Some("truecolor") | Some("24bit"))
}

/// True when the `rustline init`-managed region (bracketed by
/// [`crate::tmux_conf::TMUX_BEGIN`]/[`crate::tmux_conf::TMUX_END`]) is
/// present in `tmux_conf_contents`.
fn block_installed(tmux_conf_contents: &str) -> bool {
    tmux_conf_contents.contains(crate::tmux_conf::TMUX_BEGIN)
        && tmux_conf_contents.contains(crate::tmux_conf::TMUX_END)
}

/// tmux presence + version: missing binary is a hard `Fail` (nothing in
/// rustline works without tmux); a parseable version below
/// [`MIN_TMUX_VERSION`] is also a `Fail`; unparseable output is a `Warn`
/// (tmux is clearly present, but this check can't confirm the version).
fn check_tmux() -> Check {
    match Command::new("tmux").arg("-V").output() {
        Ok(output) => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            match parse_tmux_version(&text) {
                Some(version) if version >= MIN_TMUX_VERSION => Check {
                    name: "tmux",
                    status: CheckStatus::Ok,
                    detail: format!("{text} detected"),
                },
                Some((major, minor)) => Check {
                    name: "tmux",
                    status: CheckStatus::Fail,
                    detail: format!(
                        "{text} detected; rustline needs tmux >= {}.{} for click-to-toggle \
                         and truecolor themes (found {major}.{minor})",
                        MIN_TMUX_VERSION.0, MIN_TMUX_VERSION.1
                    ),
                },
                None => Check {
                    name: "tmux",
                    status: CheckStatus::Warn,
                    detail: format!("could not parse a version from tmux -V output: {text:?}"),
                },
            }
        }
        Err(e) => Check {
            name: "tmux",
            status: CheckStatus::Fail,
            detail: format!("tmux not found on PATH: {e}"),
        },
    }
}

/// `set -g mouse on`, only checkable from inside a running tmux session
/// (`$TMUX` set) — outside tmux this degrades to an informational `Warn`
/// rather than guessing.
fn check_mouse() -> Check {
    if env::var_os("TMUX").is_none() {
        return Check {
            name: "tmux mouse",
            status: CheckStatus::Warn,
            detail: "not running inside tmux; run `rustline doctor` from inside a session \
                     to check the `mouse` setting"
                .to_string(),
        };
    }
    match Command::new("tmux").args(["show", "-gv", "mouse"]).output() {
        Ok(output) if output.status.success() => {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if value == "on" {
                Check {
                    name: "tmux mouse",
                    status: CheckStatus::Ok,
                    detail: "mouse is on".to_string(),
                }
            } else {
                Check {
                    name: "tmux mouse",
                    status: CheckStatus::Warn,
                    detail: format!(
                        "mouse is {value:?}, not \"on\"; click-to-toggle widgets won't respond \
                         to clicks (set `set -g mouse on`)"
                    ),
                }
            }
        }
        _ => Check {
            name: "tmux mouse",
            status: CheckStatus::Warn,
            detail: "could not query tmux's `mouse` setting".to_string(),
        },
    }
}

/// `$COLORTERM` truecolor advertisement (see [`truecolor_from_env`]).
fn check_truecolor() -> Check {
    let colorterm = env::var("COLORTERM").ok();
    if truecolor_from_env(colorterm.as_deref()) {
        Check {
            name: "truecolor terminal",
            status: CheckStatus::Ok,
            detail: format!("$COLORTERM={:?}", colorterm.unwrap_or_default()),
        }
    } else {
        Check {
            name: "truecolor terminal",
            status: CheckStatus::Warn,
            detail: format!(
                "$COLORTERM is {:?}, not \"truecolor\"/\"24bit\"; the six curated themes use \
                 truecolor RGB and may look wrong",
                colorterm.unwrap_or_default()
            ),
        }
    }
}

/// Whether `name` resolves to an executable file in some `$PATH` entry.
fn binary_on_path(name: &str) -> bool {
    env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}

/// `rustline` reachable on `$PATH`. Only a `Warn`: the managed tmux block
/// (since W6) calls the binary by its resolved absolute path, so this only
/// matters if something else — a shell alias, a manual invocation — expects
/// to find `rustline` by bare name.
fn check_binary_on_path() -> Check {
    if binary_on_path("rustline") {
        Check {
            name: "rustline on PATH",
            status: CheckStatus::Ok,
            detail: "found on $PATH".to_string(),
        }
    } else {
        Check {
            name: "rustline on PATH",
            status: CheckStatus::Warn,
            detail: "not found on $PATH; harmless if you only run it via the managed tmux \
                     block, which calls it by its resolved absolute path (see `rustline \
                     init`), but matters if you invoke `rustline` by name yourself"
                .to_string(),
        }
    }
}

/// Whether the `rustline init`-managed block is installed in `tmux_conf`.
fn check_managed_block(tmux_conf: &Path) -> Check {
    match fs::read_to_string(tmux_conf) {
        Ok(contents) if block_installed(&contents) => Check {
            name: "tmux config block",
            status: CheckStatus::Ok,
            detail: format!("installed in {}", tmux_conf.display()),
        },
        Ok(_) => Check {
            name: "tmux config block",
            status: CheckStatus::Warn,
            detail: format!(
                "managed block not found in {}; run `rustline init` to install it",
                tmux_conf.display()
            ),
        },
        Err(_) => Check {
            name: "tmux config block",
            status: CheckStatus::Warn,
            detail: format!(
                "{} not found; run `rustline init` to create it",
                tmux_conf.display()
            ),
        },
    }
}

/// Whether a resolved directory (config/themes/plugin/log) already exists.
/// Absence is only a `Warn` — every one of these is created on first use
/// (invariant: `Config::load` is total), so a fresh install legitimately has
/// none of them yet.
fn check_dir(name: &'static str, dir: &Path) -> Check {
    if dir.is_dir() {
        Check {
            name,
            status: CheckStatus::Ok,
            detail: dir.display().to_string(),
        }
    } else {
        Check {
            name,
            status: CheckStatus::Warn,
            detail: format!(
                "{} does not exist yet (created automatically when needed)",
                dir.display()
            ),
        }
    }
}

/// Run every check, print the pass/warn/fail report plus the resolved
/// paths, and return the process exit code: `1` if any check `Fail`ed, else
/// `0` (a `Warn` never fails the run — these are advisories, not errors).
pub(crate) fn run(paths: &DoctorPaths) -> i32 {
    let config_dir = paths.config.parent().unwrap_or(paths.config);
    let log_dir = paths.log_file.parent().unwrap_or(paths.log_file);

    let checks = [
        check_tmux(),
        check_mouse(),
        check_truecolor(),
        check_binary_on_path(),
        check_managed_block(paths.tmux_conf),
        check_dir("config dir", config_dir),
        check_dir("themes dir", paths.themes_dir),
        check_dir("plugin dir", paths.plugin_dir),
        check_dir("log dir", log_dir),
    ];

    println!("rustline doctor");
    println!("===============\n");
    for check in &checks {
        println!(
            "[{:<4}] {:<20} {}",
            check.status.label(),
            check.name,
            check.detail
        );
    }

    println!("\nResolved paths:");
    println!("  config:  {}", paths.config.display());
    println!("  themes:  {}", paths.themes_dir.display());
    println!("  plugins: {}", paths.plugin_dir.display());
    println!("  log:     {}", paths.log_file.display());

    let fail_count = checks
        .iter()
        .filter(|c| c.status == CheckStatus::Fail)
        .count();
    let warn_count = checks
        .iter()
        .filter(|c| c.status == CheckStatus::Warn)
        .count();
    let ok_count = checks.len() - fail_count - warn_count;
    println!("\n{ok_count} ok, {warn_count} warn, {fail_count} fail");

    i32::from(fail_count > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux_conf::{TMUX_BEGIN, TMUX_END};

    #[test]
    fn parses_plain_version() {
        assert_eq!(parse_tmux_version("tmux 3.4\n"), Some((3, 4)));
    }

    #[test]
    fn parses_trailing_letter_suffix() {
        assert_eq!(parse_tmux_version("tmux 3.1a"), Some((3, 1)));
    }

    #[test]
    fn parses_next_prefixed_version() {
        assert_eq!(parse_tmux_version("tmux next-3.4"), Some((3, 4)));
    }

    #[test]
    fn rejects_unparseable_output() {
        assert_eq!(parse_tmux_version("not tmux at all"), None);
        assert_eq!(parse_tmux_version(""), None);
        assert_eq!(parse_tmux_version("tmux 3"), None); // no `.`-separated minor
    }

    #[test]
    fn version_threshold_comparison() {
        assert!(parse_tmux_version("tmux 3.0a").unwrap() < MIN_TMUX_VERSION);
        assert!(parse_tmux_version("tmux 3.1").unwrap() >= MIN_TMUX_VERSION);
        assert!(parse_tmux_version("tmux 3.4").unwrap() >= MIN_TMUX_VERSION);
    }

    #[test]
    fn truecolor_detects_known_values() {
        assert!(truecolor_from_env(Some("truecolor")));
        assert!(truecolor_from_env(Some("24bit")));
        assert!(!truecolor_from_env(Some("256color")));
        assert!(!truecolor_from_env(None));
    }

    #[test]
    fn block_installed_detects_managed_region() {
        let with_block = format!("before\n{TMUX_BEGIN}\nBLOCK\n{TMUX_END}\nafter\n");
        assert!(block_installed(&with_block));
        assert!(!block_installed("no markers here"));
    }
}
