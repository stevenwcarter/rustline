//! File + stderr logging setup for the `rustline` binary.
//!
//! Two independently-filtered `tracing` sinks:
//! - a size-rotated append-mode file at `$XDG_DATA_HOME/rustline/rustline.log`
//!   (default level INFO; raised only by `-v`), and
//! - stderr (default level ERROR; config-overridable).
//!
//! Level strings are parsed leniently so a bad value degrades to the sink's
//! default rather than silencing logging or failing the config. Logging is
//! best-effort: a file that can't be opened degrades to stderr-only, never a
//! crash — stdout is the tmux status line and is never written here.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustline_core::LogConfig;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;

/// Rotate the log file once it exceeds this many bytes.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024; // 5 MiB

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

/// The effective log-file path: config override (`~/` expanded) or
/// `$XDG_DATA_HOME/rustline/rustline.log`. `pub(crate)` so `doctor.rs` can
/// report it without duplicating the resolution logic.
pub(crate) fn log_path(cfg: &LogConfig) -> PathBuf {
    match &cfg.file {
        Some(p) => rustline_wasm::expand_tilde(p),
        None => rustline_wasm::data_root().join("rustline.log"),
    }
}

fn should_rotate(size: u64, cap: u64) -> bool {
    size > cap
}

/// Ensure the parent dir exists, rotate the file to `<path>.1` if it exceeds
/// `cap`, then open it in append mode.
fn open_log(path: &Path, cap: u64) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Ok(meta) = fs::metadata(path)
        && should_rotate(meta.len(), cap)
    {
        let mut rotated = path.as_os_str().to_owned();
        rotated.push(".1");
        let _ = fs::rename(path, PathBuf::from(rotated)); // best-effort
    }
    OpenOptions::new().create(true).append(true).open(path)
}

/// A `MakeWriter` over a shared append-mode file handle. Lock-free: the
/// kernel's `O_APPEND` gives atomic end-of-file writes and `&File: Write`
/// lets threads share one fd.
struct FileWriter(Arc<File>);

impl<'a> MakeWriter<'a> for FileWriter {
    type Writer = &'a File;
    fn make_writer(&'a self) -> Self::Writer {
        &self.0
    }
}

/// Install the two-sink subscriber. Best-effort and infallible: a file that
/// can't be opened degrades to stderr-only. Emits any deferred warnings
/// (unparseable levels, file-open failure) after the subscriber is live.
pub fn init(cfg: &LogConfig, verbose: u8) {
    let (file_level, file_warn) = resolve_file_level(verbose, &cfg.file_level);
    let (stderr_level, stderr_warn) = resolve_stderr_level(&cfg.stderr_level);
    let path = log_path(cfg);

    let (file, open_warn) = match open_log(&path, MAX_LOG_BYTES) {
        Ok(f) => (Some(Arc::new(f)), None),
        Err(e) => (
            None,
            Some(format!("cannot open log file {}: {e}", path.display())),
        ),
    };

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(io::stderr)
        .with_filter(stderr_level);

    let file_layer = file.map(|f| {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(FileWriter(f))
            .with_filter(file_level)
    });

    // `Option<L: Layer>` is itself a `Layer` (None = no-op), so a missing file
    // simply contributes nothing.
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
    use std::io::Write;
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
    fn should_rotate_is_strict_greater_than() {
        assert!(!should_rotate(10, 10));
        assert!(should_rotate(11, 10));
    }

    #[test]
    fn open_log_rotates_when_oversized() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sub").join("rustline.log"); // nested: dir is created
        // First open (dir absent) creates dir + file.
        {
            let f = open_log(&path, 100).unwrap();
            let mut f = &f; // &File: Write
            f.write_all(&[b'x'; 200]).unwrap(); // exceed cap
        }
        assert!(path.exists());
        // Second open sees >100 bytes and rotates.
        {
            let _f = open_log(&path, 100).unwrap();
        }
        let rotated = {
            let mut p = path.as_os_str().to_owned();
            p.push(".1");
            PathBuf::from(p)
        };
        assert!(rotated.exists(), "rotated file exists");
        assert_eq!(fs::metadata(&path).unwrap().len(), 0, "fresh log is empty");
    }

    #[test]
    fn open_log_keeps_small_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rustline.log");
        {
            let f = open_log(&path, 100).unwrap();
            let mut f = &f;
            f.write_all(b"tiny").unwrap();
        }
        {
            let _f = open_log(&path, 100).unwrap();
        }
        let mut rotated = path.as_os_str().to_owned();
        rotated.push(".1");
        assert!(!PathBuf::from(rotated).exists(), "small file not rotated");
        assert_eq!(fs::metadata(&path).unwrap().len(), 4, "content preserved");
    }
}
