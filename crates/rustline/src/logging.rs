//! File + stderr logging setup for the `rustline` binary.
//!
//! Two independently-filtered `tracing` sinks:
//! - a daily-rotated append-mode file at
//!   `$XDG_DATA_HOME/rustline/rustline.<date>.log`, keeping 7 generations
//!   (default level INFO; raised only by `-v`), and
//! - stderr (default level ERROR; config-overridable).
//!
//! Level strings are parsed leniently so a bad value degrades to the sink's
//! default rather than silencing logging or failing the config. Logging is
//! best-effort: a file that can't be opened degrades to stderr-only, never a
//! crash — stdout is the tmux status line and is never written here.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rustline_core::LogConfig;
use tracing_appender::rolling::{Builder, InitError, RollingFileAppender, Rotation};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::prelude::*;

/// Map a `-v` repetition count to a file-sink level. `0` means "no override"
/// (use the config/default level). ERROR-based scale, clamped at TRACE.
fn verbosity_to_level(count: u8) -> Option<LevelFilter> {
    match count {
        0 => None,
        1 => Some(LevelFilter::WARN),
        2 => Some(LevelFilter::INFO),
        3 => Some(LevelFilter::DEBUG),
        _ => Some(LevelFilter::TRACE),
    }
}

/// Parse a level string (case-insensitive, trimmed). `None` on an unknown
/// value so the caller can fall back to a default and warn.
fn parse_level(s: &str) -> Option<LevelFilter> {
    match s.trim().to_ascii_lowercase().as_str() {
        "off" => Some(LevelFilter::OFF),
        "error" => Some(LevelFilter::ERROR),
        "warn" => Some(LevelFilter::WARN),
        "info" => Some(LevelFilter::INFO),
        "debug" => Some(LevelFilter::DEBUG),
        "trace" => Some(LevelFilter::TRACE),
        _ => None,
    }
}

/// Resolve the file-sink level: `-v` wins if present, else the config value,
/// else INFO. The `Option<String>` is a deferred warning about an unparseable
/// config value (emitted only after the subscriber exists).
fn resolve_file_level(verbose: u8, cfg_level: &str) -> (LevelFilter, Option<String>) {
    if let Some(level) = verbosity_to_level(verbose) {
        return (level, None);
    }
    match parse_level(cfg_level) {
        Some(level) => (level, None),
        None => (
            LevelFilter::INFO,
            Some(format!("invalid log.file_level {cfg_level:?}; using info")),
        ),
    }
}

/// Resolve the stderr-sink level: the config value, else ERROR.
fn resolve_stderr_level(cfg_level: &str) -> (LevelFilter, Option<String>) {
    match parse_level(cfg_level) {
        Some(level) => (level, None),
        None => (
            LevelFilter::ERROR,
            Some(format!(
                "invalid log.stderr_level {cfg_level:?}; using error"
            )),
        ),
    }
}

/// How many daily generations to keep.
const MAX_LOG_FILES: usize = 7;
/// Filename suffix used when `[log].file` is unset or has no extension.
const DEFAULT_LOG_SUFFIX: &str = "log";
/// Filename stem used when `[log].file` is unset.
const DEFAULT_LOG_PREFIX: &str = "rustline";

/// The directory the log files live in: the parent of a configured
/// `[log].file`, else `$XDG_DATA_HOME/rustline`.
pub(crate) fn log_dir(cfg: &LogConfig) -> PathBuf {
    match &cfg.file {
        Some(p) => rustline_wasm::expand_tilde(p)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(rustline_wasm::data_root),
        None => rustline_wasm::data_root(),
    }
}

/// The filename stem: a configured `[log].file`'s stem, else `rustline`.
pub(crate) fn log_file_prefix(cfg: &LogConfig) -> String {
    cfg.file
        .as_ref()
        .map(|p| rustline_wasm::expand_tilde(p))
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_LOG_PREFIX.to_string())
}

/// The filename extension: a configured `[log].file`'s extension, else `log`.
pub(crate) fn log_file_suffix(cfg: &LogConfig) -> String {
    cfg.file
        .as_ref()
        .map(|p| rustline_wasm::expand_tilde(p))
        .and_then(|p| p.extension().map(|s| s.to_string_lossy().into_owned()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_LOG_SUFFIX.to_string())
}

/// Today's log file: `<log_dir>/<prefix>.<YYYY-MM-DD>.<suffix>`. Reported by
/// `doctor`; the appender derives the same name internally.
///
/// The date is computed in UTC, not local time: `RollingFileAppender`
/// unconditionally rotates on a UTC day boundary, so using local time here
/// would let this reported path disagree with the file the appender is
/// actually writing to for any user not at UTC+0. Do not "fix" this to
/// `chrono::Local::now()` again.
pub(crate) fn current_log_path(cfg: &LogConfig) -> PathBuf {
    let date = chrono::Utc::now().format("%Y-%m-%d");
    log_dir(cfg).join(format!(
        "{}.{date}.{}",
        log_file_prefix(cfg),
        log_file_suffix(cfg)
    ))
}

/// Build the daily-rotating appender. Blocking (no `WorkerGuard` to keep
/// alive), and it re-evaluates its target on every write — which is what lets
/// the long-lived daemon follow a rotation instead of pinning a dead inode.
///
/// The single builder chain for both production and tests: `init` calls this
/// directly, so a drift here can't leave the test seam pinned to a stale copy
/// of what production actually builds.
fn build_appender(
    dir: &Path,
    prefix: &str,
    suffix: &str,
) -> Result<RollingFileAppender, InitError> {
    Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix(prefix)
        .filename_suffix(suffix)
        .max_log_files(MAX_LOG_FILES)
        .build(dir)
}

/// Install the two-sink subscriber. Best-effort and infallible: a log
/// directory that can't be created or opened degrades to stderr-only. Emits
/// any deferred warnings (unparseable levels, open failure) after the
/// subscriber is live.
pub fn init(cfg: &LogConfig, verbose: u8) {
    let (file_level, file_warn) = resolve_file_level(verbose, &cfg.file_level);
    let (stderr_level, stderr_warn) = resolve_stderr_level(&cfg.stderr_level);
    let dir = log_dir(cfg);

    let (appender, open_warn) = match fs::create_dir_all(&dir)
        .map_err(|e| e.to_string())
        .and_then(|()| {
            build_appender(&dir, &log_file_prefix(cfg), &log_file_suffix(cfg))
                .map_err(|e| e.to_string())
        }) {
        Ok(a) => (Some(a), None),
        Err(e) => (
            None,
            Some(format!("cannot open log dir {}: {e}", dir.display())),
        ),
    };

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(io::stderr)
        .with_filter(stderr_level);

    let file_layer = appender.map(|a| {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(a)
            .with_filter(file_level)
    });

    // `Option<L: Layer>` is itself a `Layer` (None = no-op), so a missing
    // appender simply contributes nothing.
    tracing_subscriber::registry()
        .with(stderr_layer)
        .with(file_layer)
        .init();

    if let Some(msg) = file_warn {
        tracing::warn!("{msg}");
    }
    if let Some(msg) = stderr_warn {
        tracing::warn!("{msg}");
    }
    if let Some(msg) = open_warn {
        tracing::error!("{msg}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn verbosity_scale_is_error_based_and_clamps() {
        assert_eq!(verbosity_to_level(0), None);
        assert_eq!(verbosity_to_level(1), Some(LevelFilter::WARN));
        assert_eq!(verbosity_to_level(2), Some(LevelFilter::INFO));
        assert_eq!(verbosity_to_level(3), Some(LevelFilter::DEBUG));
        assert_eq!(verbosity_to_level(4), Some(LevelFilter::TRACE));
        assert_eq!(verbosity_to_level(9), Some(LevelFilter::TRACE));
    }

    #[test]
    fn parse_level_is_case_insensitive_and_lenient() {
        assert_eq!(parse_level("off"), Some(LevelFilter::OFF));
        assert_eq!(parse_level("ERROR"), Some(LevelFilter::ERROR));
        assert_eq!(parse_level("  Warn "), Some(LevelFilter::WARN));
        assert_eq!(parse_level("info"), Some(LevelFilter::INFO));
        assert_eq!(parse_level("debug"), Some(LevelFilter::DEBUG));
        assert_eq!(parse_level("trace"), Some(LevelFilter::TRACE));
        assert_eq!(parse_level("bogus"), None);
    }

    #[test]
    fn resolve_file_level_precedence() {
        // -v wins over config
        assert_eq!(resolve_file_level(2, "trace").0, LevelFilter::INFO);
        // no -v -> config value
        assert_eq!(resolve_file_level(0, "debug").0, LevelFilter::DEBUG);
        // no -v, bad config -> INFO default + warning
        let (lvl, warn) = resolve_file_level(0, "nope");
        assert_eq!(lvl, LevelFilter::INFO);
        assert!(warn.is_some());
    }

    #[test]
    fn resolve_stderr_level_defaults_to_error() {
        assert_eq!(resolve_stderr_level("warn").0, LevelFilter::WARN);
        let (lvl, warn) = resolve_stderr_level("nope");
        assert_eq!(lvl, LevelFilter::ERROR);
        assert!(warn.is_some());
    }

    #[test]
    fn log_path_parts_default_to_data_root_rustline_log() {
        let cfg = LogConfig::default();
        assert_eq!(log_file_prefix(&cfg), "rustline");
        assert_eq!(log_file_suffix(&cfg), "log");
        assert!(log_dir(&cfg).ends_with("rustline"));
    }

    #[test]
    fn configured_file_decomposes_into_dir_prefix_and_suffix() {
        let cfg = LogConfig {
            file: Some("/var/tmp/rl/app.txt".to_string()),
            ..LogConfig::default()
        };
        assert_eq!(log_dir(&cfg), PathBuf::from("/var/tmp/rl"));
        assert_eq!(log_file_prefix(&cfg), "app");
        assert_eq!(log_file_suffix(&cfg), "txt");
    }

    #[test]
    fn configured_file_without_extension_uses_the_default_suffix() {
        let cfg = LogConfig {
            file: Some("/var/tmp/rl/app".to_string()),
            ..LogConfig::default()
        };
        assert_eq!(log_file_prefix(&cfg), "app");
        assert_eq!(log_file_suffix(&cfg), "log");
    }

    #[test]
    fn current_log_path_sits_in_the_log_dir_with_the_prefix() {
        let cfg = LogConfig {
            file: Some("/var/tmp/rl/app.txt".to_string()),
            ..LogConfig::default()
        };
        let p = current_log_path(&cfg);
        assert!(p.starts_with("/var/tmp/rl"));
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("app."), "got {name}");
        assert!(name.ends_with(".txt"), "got {name}");
    }

    #[test]
    fn appender_writes_into_the_configured_dir() {
        use std::io::Write as _;
        let dir = tempdir().unwrap();
        let mut appender = build_appender(dir.path(), "rustline", "log").unwrap();
        appender.write_all(b"hello\n").unwrap();
        appender.flush().unwrap();
        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries.len(), 1, "one log file: {entries:?}");
        assert!(entries[0].starts_with("rustline."), "got {entries:?}");
        assert!(entries[0].ends_with(".log"), "got {entries:?}");
    }

    /// Round-trips `current_log_path` against the file the appender actually
    /// creates, pinning that the two agree on the whole filename shape
    /// (dir/prefix/date/suffix) for a single write under the test process's
    /// own clock.
    ///
    /// This does *not* by itself prove the date component is
    /// timezone-invariant: both `current_log_path` and the appender read the
    /// same process clock here, so a shared bug (e.g. both computing the
    /// date in local time instead of UTC) would still pass this test,
    /// especially on a UTC-hosted CI box where local time and UTC never
    /// disagree. That gap is closed separately by
    /// `doctor_log_path_is_timezone_invariant` in `tests/smoke.rs`, which
    /// spawns `rustline doctor` under two widely-separated `TZ` values and
    /// asserts the reported filename doesn't change.
    #[test]
    fn current_log_path_matches_the_file_the_appender_actually_creates() {
        use std::io::Write as _;
        let dir = tempdir().unwrap();
        let cfg = LogConfig {
            file: Some(
                dir.path()
                    .join("rustline.log")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ..LogConfig::default()
        };

        let mut appender =
            build_appender(dir.path(), &log_file_prefix(&cfg), &log_file_suffix(&cfg)).unwrap();
        appender.write_all(b"hello\n").unwrap();
        appender.flush().unwrap();

        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(entries.len(), 1, "one log file: {entries:?}");

        assert_eq!(
            current_log_path(&cfg).file_name(),
            entries[0].path().file_name(),
            "doctor's reported path must name the file the appender wrote"
        );
    }
}
