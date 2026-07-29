//! `rustline doctor`: diagnoses the prerequisites documented in the README
//! (tmux >= 3.1, `set -g mouse on`, a truecolor terminal, `rustline` on
//! tmux's PATH, and the managed tmux-conf block) and reports each as
//! pass/warn/fail, alongside the resolved config/themes/plugin/log paths.
//!
//! Follows the same pure-parser / thin-I/O-shell split as `battery.rs`:
//! `parse_tmux_version`, `truecolor_from_env`, and `block_installed` are
//! pure and unit-tested directly; `run` is the I/O shell that spawns `tmux`,
//! reads env vars and `~/.tmux.conf`, and prints the report.
//!
//! **What doctor writes.** A doctor run writes no configuration, theme, or
//! plugin-registry file — in that sense it's still a read-and-print command
//! like the other stdout-is-for-humans commands (`theme list`, `plugin
//! list`). It is not entirely write-free, though: [`check_readers`] probes
//! each platform reader whose widget kind is in the active layout by calling
//! the exact same function a render calls, and one of those,
//! `crate::cpu::read_cpu`, has a genuine write side effect — it best-effort
//! persists its `<state_root>/cpu-sample` delta-snapshot cache (see
//! `cpu.rs`) so the *next* call can take the zero-sleep fast path. That
//! write is not new or doctor-specific: it is the identical file a normal
//! `render right` already writes on every `status-interval` whenever `cpu`
//! is in the layout (the default), so running `doctor` just adds one more
//! instance of a write that is already continuous — never a write a render
//! wouldn't have produced moments later anyway. A write failure (e.g. an
//! unwritable or foreign-owned state dir) only ever `warn!`s into the log
//! file, exactly as it already would on every normal render in that same
//! environment — doctor introduces no new failure mode there, and the
//! warning never reaches doctor's own stdout report. Every other probed
//! reader (`memory`, `battery`, `uptime`, `disk`, `git`, `media`) performs no
//! persisted write at all.

use std::collections::HashMap;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::process::Command;

use rustline_core::{Config, PluginConfig, WidgetKind};

use crate::plugin_checksum::{PluginChecksumStatus, status_for};
use crate::{daemon, daemon_client};

/// Minimum tmux version rustline's click-to-toggle needs: status-line click
/// ranges and the `mouse_status_range` format variable were added in 3.1.
const MIN_TMUX_VERSION: (u32, u32) = (3, 1);

/// tmux version at which `display-popup` became available — what the
/// `prefix + W` widget-manager binding needs. Advisory only: the status line
/// itself works on 3.1.
const MIN_POPUP_TMUX_VERSION: (u32, u32) = (3, 2);

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
/// `themes_dir`, `resolve_plugin_dir`, `logging::current_log_path`, `tmux_conf_path`
/// in `main.rs`).
pub(crate) struct DoctorPaths<'a> {
    pub config: &'a Path,
    pub themes_dir: &'a Path,
    pub plugin_dir: &'a Path,
    pub log_file: &'a Path,
    pub tmux_conf: &'a Path,
    /// The configured `[plugins.*]` table, so the checksum row (see
    /// [`check_plugin_checksums`]) can report on every plugin the user has
    /// configured, not just ones that happen to be discovered on disk.
    pub plugins: &'a HashMap<String, PluginConfig>,
    /// The whole effective config, so [`check_readers`] can resolve which
    /// reader kinds the active layout actually uses (including
    /// `[instances.*]`) rather than guessing from widget names.
    pub cfg: &'a Config,
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
///
/// Also returns the parsed version alongside the check, so [`check_popup`]'s
/// advisory row can reuse it instead of shelling out to `tmux -V` again.
fn check_tmux() -> (Check, Option<(u32, u32)>) {
    match Command::new("tmux").arg("-V").output() {
        Ok(output) => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let version = parse_tmux_version(&text);
            let check = match version {
                Some(v) if v >= MIN_TMUX_VERSION => Check {
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
            };
            (check, version)
        }
        Err(e) => (
            Check {
                name: "tmux",
                status: CheckStatus::Fail,
                detail: format!("tmux not found on PATH: {e}"),
            },
            None,
        ),
    }
}

/// Advisory status for the widget-manager popup binding: `Ok` at tmux >= 3.2,
/// `Warn` below it or when the version can't be determined. Never `Fail` — a
/// missing popup does not break the status line, so this must not affect
/// doctor's exit code (the same shape as the daemon-reachability row).
fn popup_status(version: Option<(u32, u32)>) -> CheckStatus {
    match version {
        Some(v) if v >= MIN_POPUP_TMUX_VERSION => CheckStatus::Ok,
        _ => CheckStatus::Warn,
    }
}

/// Whether the `prefix + W` widget-manager popup binding (emitted
/// unconditionally by `rustline init`, see
/// `tmux_conf`'s `WIDGET_POPUP_BINDING`) will actually work: `display-popup`
/// needs tmux >= [`MIN_POPUP_TMUX_VERSION`]. Takes the version [`check_tmux`]
/// already parsed rather than shelling out to `tmux -V` a second time.
fn check_popup(version: Option<(u32, u32)>) -> Check {
    let status = popup_status(version);
    let detail = if status == CheckStatus::Ok {
        "display-popup available (prefix + W opens the widget manager)"
    } else {
        "prefix + W widget manager needs tmux >= 3.2 (display-popup); the status line itself works"
    };
    Check {
        name: "widget manager popup",
        status,
        detail: detail.to_string(),
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

/// Whether the optional persistent render daemon (`rustline daemon run`,
/// W48) is present and reachable. Purely informational: daemon mode is an
/// opt-in speedup, not a requirement (`try_render` always falls back to an
/// in-process render), so this is never a `Fail` — only `Ok` (reachable) or
/// `Warn` (not running, or a stale socket file that doesn't answer).
fn check_daemon() -> Check {
    let sock = daemon_client::daemon_socket_path();
    if !sock.exists() {
        return Check {
            name: "render daemon",
            status: CheckStatus::Warn,
            detail: "not running (optional; see `rustline daemon run`)".to_string(),
        };
    }
    if daemon::status() {
        Check {
            name: "render daemon",
            status: CheckStatus::Ok,
            detail: format!("running at {}", sock.display()),
        }
    } else {
        Check {
            name: "render daemon",
            status: CheckStatus::Warn,
            detail: format!(
                "socket present at {} but not reachable (stale socket from a killed daemon?)",
                sock.display()
            ),
        }
    }
}

/// Verify every configured plugin's installed `.wasm` against its recorded
/// `checksum` (the same [`rustline_wasm::verify_checksum`] call
/// `register_plugins` gates loading on — see `plugin_checksum::status_for`),
/// and summarize the result as a single advisory row.
///
/// **Always `Ok` or `Warn`, never `Fail`** — a checksum problem is real and
/// worth surfacing (unlike the other `Warn`-only rows here, an actual security
/// gate is silently rejecting a widget), but `doctor`'s exit code is reserved
/// for setup that's outright broken, and the checksum gate itself already
/// degrades a bad plugin to "widget missing," never to a broken bar (N2). The
/// `render daemon` row above is this check's precedent for an advisory-only,
/// never-fails row.
///
/// Summarizes rather than spams: a plugin that verifies or has no checksum
/// recorded contributes only to the leading counts, never named individually;
/// a plugin that mismatches, has a malformed digest, or has a missing `.wasm`
/// file is named explicitly, since those are the actionable ones. An empty
/// `[plugins.*]` table reports a clean, unambiguous "no plugins configured"
/// rather than an empty/confusing row.
fn check_plugin_checksums(plugins: &HashMap<String, PluginConfig>, plugin_dir: &Path) -> Check {
    const NAME: &str = "plugin checksums";
    if plugins.is_empty() {
        return Check {
            name: NAME,
            status: CheckStatus::Ok,
            detail: "no plugins configured".to_string(),
        };
    }

    let mut names: Vec<&str> = plugins.keys().map(String::as_str).collect();
    names.sort_unstable();

    let (mut verified, mut unpinned) = (0usize, 0usize);
    let (mut mismatched, mut malformed, mut missing) = (Vec::new(), Vec::new(), Vec::new());
    for name in names {
        let checksum = plugins[name].checksum.as_deref();
        match status_for(plugin_dir, name, checksum) {
            PluginChecksumStatus::Verified => verified += 1,
            PluginChecksumStatus::Unpinned => unpinned += 1,
            PluginChecksumStatus::Mismatch => mismatched.push(name),
            PluginChecksumStatus::Malformed => malformed.push(name),
            PluginChecksumStatus::Missing => missing.push(name),
        }
    }

    let mut detail = format!("{verified} verified, {unpinned} unpinned");
    let status = if mismatched.is_empty() && malformed.is_empty() && missing.is_empty() {
        CheckStatus::Ok
    } else {
        if !mismatched.is_empty() {
            let _ = write!(detail, "; checksum mismatch: {}", mismatched.join(", "));
        }
        if !malformed.is_empty() {
            let _ = write!(detail, "; malformed checksum: {}", malformed.join(", "));
        }
        if !missing.is_empty() {
            let _ = write!(detail, "; .wasm not found: {}", missing.join(", "));
        }
        CheckStatus::Warn
    };

    Check {
        name: NAME,
        status,
        detail,
    }
}

/// The widget names doctor's `widget readers` check should probe: every name
/// actually rendered through the ordinary widget pipeline — `left` ∪
/// `right` (`run` passes this to [`check_readers`], which resolves it to
/// reader *kinds* via `Config::layout_kinds`/`disk_mounts`, both of which
/// collect into a `BTreeSet`; a kind or mount repeated across regions, or
/// across several `[instances.*]` of the same kind, is naturally probed
/// once, not once per placement).
///
/// `center` is deliberately excluded. Nothing in the render pipeline reads
/// it — `assemble.rs`'s window-list render hardcodes
/// `registry.resolve(&["windows".to_string()])` rather than resolving
/// `center`'s contents (see the Config doc's "`[layout].center` is inert"
/// note) — so a reader for a widget kind placed only in `center` is never
/// actually invoked by a real render. Probing it here would report a
/// "reader failure" for a widget that isn't broken, it simply isn't
/// rendered at all — a different problem this check isn't meant to surface
/// (that one's already flagged by `widget enable/move --region center`'s
/// stderr note and `widget edit`'s outright refusal to edit `center`).
fn readable_layout(cfg: &Config) -> Vec<String> {
    let mut names = cfg.layout.left.clone();
    names.extend(cfg.layout.right.iter().cloned());
    names
}

/// Probe every reader whose widget kind is actually in `layout` and report
/// which ones currently yield nothing. This is the only diagnostic channel for
/// a reader failure: each one degrades to `down_format` (default `""`), so a
/// missing `git`/`playerctl` binary, an unmountable `[widgets.disk].mount`, or
/// a tmux that won't list windows all look identical to "widget not
/// configured" in the rendered bar. Run with `-vvv` to see each failed
/// reader's concrete cause via its `debug!` log line.
///
/// `run` calls this with [`readable_layout`] (the union of `left`/`right`),
/// so a reader-backed widget is probed regardless of which of those two
/// regions it's placed in.
///
/// **Never `Fail`.** `doctor`'s exit code is reserved for setup that is
/// outright broken, and a `None` reading is frequently legitimate — no
/// battery, not inside a repository, no media player running. Same rule
/// [`check_plugin_checksums`] follows.
fn check_readers(cfg: &Config, layout: &[String]) -> Check {
    const NAME: &str = "widget readers";
    let kinds = cfg.layout_kinds(layout);

    // (kind, is-currently-readable). Only probe what the layout actually uses,
    // so `doctor` never pays for a reader the user doesn't have configured.
    // Exhaustive over `WidgetKind` (no `_` arm) so a future kind forces a
    // decision here instead of silently going unprobed: `throughput` is
    // deliberately excluded (it legitimately reads `None` on a first
    // invocation, invariant #6, which would warn on every fresh state dir),
    // and the remaining eight kinds carry no platform reader at all.
    let mut probed: Vec<(&str, bool)> = Vec::new();
    for kind in kinds {
        match kind {
            WidgetKind::Git => probed.push(("git", crate::git::read_git(".").is_some())),
            WidgetKind::Media => probed.push(("media", crate::media::read_media().is_some())),
            WidgetKind::Battery => {
                probed.push(("battery", crate::battery::read_battery().is_some()));
            }
            WidgetKind::Uptime => probed.push(("uptime", crate::uptime::read_uptime().is_some())),
            WidgetKind::Cpu => probed.push(("cpu", crate::cpu::read_cpu().is_some())),
            WidgetKind::Memory => probed.push(("memory", crate::memory::read_memory().is_some())),
            WidgetKind::Disk => {
                for mount in cfg.disk_mounts(layout) {
                    probed.push(("disk", crate::disk::read_disk(&mount).is_some()));
                }
            }
            WidgetKind::Throughput
            | WidgetKind::PaneId
            | WidgetKind::Hostname
            | WidgetKind::Windows
            | WidgetKind::DateTime
            | WidgetKind::Cwd
            | WidgetKind::LanIp
            | WidgetKind::TailscaleIp
            | WidgetKind::LoadAvg => {}
        }
    }

    if probed.is_empty() {
        return Check {
            name: NAME,
            status: CheckStatus::Ok,
            detail: "no readers in the active layout".to_string(),
        };
    }

    let quiet: Vec<&str> = probed
        .iter()
        .filter(|(_, ok)| !ok)
        .map(|(kind, _)| *kind)
        .collect();
    let ok_count = probed.len() - quiet.len();

    if quiet.is_empty() {
        Check {
            name: NAME,
            status: CheckStatus::Ok,
            detail: format!("{ok_count} reading"),
        }
    } else {
        Check {
            name: NAME,
            status: CheckStatus::Warn,
            detail: format!(
                "{ok_count} reading; nothing to report from: {} (run with -vvv for the cause)",
                quiet.join(", ")
            ),
        }
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

    let (tmux_check, tmux_version) = check_tmux();
    let checks = [
        tmux_check,
        check_mouse(),
        check_truecolor(),
        check_binary_on_path(),
        check_managed_block(paths.tmux_conf),
        check_daemon(),
        check_popup(tmux_version),
        check_plugin_checksums(paths.plugins, paths.plugin_dir),
        check_readers(paths.cfg, &readable_layout(paths.cfg)),
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
    println!("  log:     {} (daily, 7 kept)", paths.log_file.display());

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

    #[test]
    fn popup_support_passes_at_3_2_and_warns_below() {
        assert_eq!(popup_status(Some((3, 2))), CheckStatus::Ok);
        assert_eq!(popup_status(Some((3, 4))), CheckStatus::Ok);
        assert_eq!(popup_status(Some((3, 1))), CheckStatus::Warn);
        assert_eq!(popup_status(None), CheckStatus::Warn);
    }

    fn pc(checksum: Option<&str>) -> PluginConfig {
        PluginConfig {
            checksum: checksum.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn no_plugins_configured_is_a_clean_pass() {
        let plugins = HashMap::new();
        let dir = tempfile::tempdir().unwrap();
        let check = check_plugin_checksums(&plugins, dir.path());
        assert_eq!(check.status, CheckStatus::Ok);
        assert!(
            check.detail.contains("no plugins configured"),
            "{}",
            check.detail
        );
    }

    #[test]
    fn every_plugin_verified_or_unpinned_is_a_single_ok_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("good.wasm"), b"good bytes").unwrap();
        std::fs::write(dir.path().join("bare.wasm"), b"whatever").unwrap();
        let mut plugins = HashMap::new();
        plugins.insert(
            "good".to_string(),
            pc(Some(&rustline_wasm::sha256_hex(b"good bytes"))),
        );
        plugins.insert("bare".to_string(), pc(None));

        let check = check_plugin_checksums(&plugins, dir.path());
        assert_eq!(check.status, CheckStatus::Ok);
        assert!(check.detail.contains("1 verified"), "{}", check.detail);
        assert!(check.detail.contains("1 unpinned"), "{}", check.detail);
    }

    #[test]
    fn mismatch_is_advisory_warn_never_fail_and_names_the_plugin() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bad.wasm"), b"actual bytes").unwrap();
        let mut plugins = HashMap::new();
        plugins.insert(
            "bad".to_string(),
            pc(Some(&rustline_wasm::sha256_hex(b"different bytes"))),
        );

        let check = check_plugin_checksums(&plugins, dir.path());
        assert_eq!(
            check.status,
            CheckStatus::Warn,
            "advisory only: {}",
            check.detail
        );
        assert!(check.detail.contains("bad"), "{}", check.detail);
        assert!(check.detail.contains("mismatch"), "{}", check.detail);
    }

    #[test]
    fn malformed_and_missing_are_also_advisory_warn_and_named() {
        let dir = tempfile::tempdir().unwrap();
        // `weird` has a malformed recorded digest but a real file present.
        std::fs::write(dir.path().join("weird.wasm"), b"bytes").unwrap();
        // `ghost` has no .wasm file on disk at all.
        let mut plugins = HashMap::new();
        plugins.insert("weird".to_string(), pc(Some("not-a-real-digest")));
        plugins.insert("ghost".to_string(), pc(None));

        let check = check_plugin_checksums(&plugins, dir.path());
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.detail.contains("weird"), "{}", check.detail);
        assert!(check.detail.contains("malformed"), "{}", check.detail);
        assert!(check.detail.contains("ghost"), "{}", check.detail);
        assert!(check.detail.contains("not found"), "{}", check.detail);
    }

    #[test]
    fn reader_check_never_fails_the_run() {
        // doctor's exit code is reserved for setup that is outright broken. A
        // reader returning None is frequently legitimate (no battery, not in a
        // repo, no player running), so this row must never produce Fail — the
        // same rule check_plugin_checksums follows.
        let cfg = Config::default();
        for layout in [
            vec![],
            vec!["cpu".to_string(), "memory".to_string()],
            vec![
                "git".to_string(),
                "media".to_string(),
                "battery".to_string(),
            ],
        ] {
            let check = check_readers(&cfg, &layout);
            assert_ne!(check.status, CheckStatus::Fail, "layout {layout:?}");
            assert_eq!(check.name, "widget readers");
        }
    }

    #[test]
    fn reader_check_reports_nothing_to_probe_for_an_empty_layout() {
        let cfg = Config::default();
        let check = check_readers(&cfg, &[]);
        assert_eq!(check.status, CheckStatus::Ok);
        assert!(check.detail.contains("no readers"), "got: {}", check.detail);
    }

    #[test]
    fn readable_layout_unions_left_and_right_but_excludes_center() {
        // Regression coverage for the code-health finding: `check_readers`
        // used to only ever see `cfg.layout.right`, so a reader-backed
        // widget placed in `left` (legal via `widget enable --region left`)
        // was silently never probed. `center` stays excluded on purpose —
        // nothing in the render pipeline reads it, so a reader for a
        // center-only widget was never actually invoked by a real render.
        let mut cfg = Config::default();
        cfg.layout.left = vec!["git".to_string()];
        cfg.layout.right = vec!["cpu".to_string()];
        cfg.layout.center = vec!["battery".to_string()];

        let layout = readable_layout(&cfg);
        assert!(layout.iter().any(|n| n == "git"), "{layout:?}");
        assert!(layout.iter().any(|n| n == "cpu"), "{layout:?}");
        assert!(
            !layout.iter().any(|n| n == "battery"),
            "center must stay excluded: {layout:?}"
        );
    }

    #[test]
    fn a_reader_kind_placed_in_left_is_probed() {
        // The bug: `run` used to call `check_readers(paths.cfg,
        // &paths.cfg.layout.right)`, so a `git`/`battery`/`disk` widget
        // living only in `left` was invisible to this check. Wiring `run`
        // through `readable_layout` fixes that; assert it end to end
        // through `check_readers` itself, not just the layout union.
        let mut cfg = Config::default();
        cfg.layout.left = vec!["git".to_string()];
        cfg.layout.right = vec![];

        let check = check_readers(&cfg, &readable_layout(&cfg));
        assert_ne!(check.status, CheckStatus::Fail);
        assert_ne!(
            check.detail, "no readers in the active layout",
            "a `left`-only reader kind must still be probed"
        );
    }
}
