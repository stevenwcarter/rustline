# File Logging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist rustline's `tracing` output to a size-rotated file under the XDG data dir (INFO by default), keep errors on stderr, and control levels via a `-v` flag + a `[log]` config table.

**Architecture:** Replace the single stderr `EnvFilter` subscriber with a two-layer `tracing_subscriber::registry()` — a rotated append-mode file sink and a stderr sink, each with its own `LevelFilter`. Pure helpers (level mapping, string parsing, rotation) live in a new `crates/rustline/src/logging.rs` and are unit-tested; the wiring is covered by subprocess smoke tests. Config loading is refactored so the "invalid config" warning is emitted *after* the subscriber exists.

**Tech Stack:** Rust edition 2024, `tracing` + `tracing-subscriber` 0.3 (layered, no `env-filter`), `clap` derive (count flag), `serde`/`toml`, `tempfile` (tests).

## Global Constraints

- **Edition 2024** in every crate; keep editions equal to `rustfmt.toml`.
- Must stay **clippy-clean** (`cargo clippy --all-targets -- -D warnings`) and **rustfmt-clean** (`cargo fmt --all --check`). Run `cargo fmt --all` before each commit.
- **Commit `Cargo.lock`** alongside any dependency/feature change.
- **Invariant #3 — `Config::load` is total:** a bad/missing config never breaks the bar. New `[log]` fields are all `#[serde(default)]`; level values are lenient strings (a typo degrades one level, never fails the whole parse).
- **stdout is the tmux status line:** logging writes only to the file and stderr — never stdout. No `println!`/stdout writes in logging code.
- **Never break the bar (Invariant #2 spirit):** logging init is best-effort and infallible — a file that can't be opened degrades to stderr-only, never a panic/early-exit.
- **rustls-only:** introduce no TLS/dep that pulls OpenSSL. (This feature adds no network deps.)
- Spec: `docs/superpowers/specs/2026-07-21-rustline-file-logging-design.md`.

---

### Task 1: `[log]` config table + `load_reporting`

Adds the `LogConfig` schema to the core `Config` and splits config loading so the failure warning can be deferred until a subscriber is installed.

**Files:**
- Modify: `crates/rustline-core/src/config.rs` (add `LogConfig`, `Config.log`, `load_reporting`; make `load` a wrapper; add tests)
- Modify: `crates/rustline-core/src/lib.rs:13` (re-export `LogConfig`)
- Modify: `crates/rustline-core/Cargo.toml` (add `tempfile` dev-dep)

**Interfaces:**
- Produces:
  - `pub struct LogConfig { pub file_level: String, pub stderr_level: String, pub file: Option<String> }` with `Default` = `{ "info", "error", None }`, re-exported as `rustline_core::LogConfig`.
  - `Config.log: LogConfig` (last field).
  - `Config::load_reporting(path: &Path) -> (Config, Option<String>)` — `(cfg, None)` on success or absent file; `(Config::default(), Some(msg))` on parse error.
  - `Config::load(path: &Path) -> Config` unchanged signature/behavior (now delegates to `load_reporting` and logs the message).

- [ ] **Step 1: Add `tempfile` dev-dependency to core**

Edit `crates/rustline-core/Cargo.toml`. If there is no `[dev-dependencies]` section, add one:

```toml
[dev-dependencies]
tempfile = "3"
```

(If `[dev-dependencies]` already exists, add the `tempfile = "3"` line to it.)

- [ ] **Step 2: Write failing tests for `LogConfig` defaults and `load_reporting`**

In `crates/rustline-core/src/config.rs`, inside the existing `#[cfg(test)] mod tests { ... }` block, add (the module already has `use super::*;`; add `use std::io::Write;` and `use tempfile::NamedTempFile;` at the top of the test module if not present):

```rust
#[test]
fn log_config_defaults_when_absent() {
    let c: Config = toml::from_str("").unwrap();
    assert_eq!(c.log.file_level, "info");
    assert_eq!(c.log.stderr_level, "error");
    assert_eq!(c.log.file, None);
}

#[test]
fn log_config_partial_keeps_other_defaults() {
    let c: Config = toml::from_str("[log]\nfile_level = \"debug\"\n").unwrap();
    assert_eq!(c.log.file_level, "debug");
    assert_eq!(c.log.stderr_level, "error"); // untouched
    assert_eq!(c.log.file, None);
}

#[test]
fn load_reporting_ok_has_no_warning() {
    let mut f = NamedTempFile::new().unwrap();
    write!(f, "[log]\nfile_level = \"trace\"\n").unwrap();
    let (cfg, warn) = Config::load_reporting(f.path());
    assert_eq!(cfg.log.file_level, "trace");
    assert!(warn.is_none());
}

#[test]
fn load_reporting_bad_file_defaults_with_warning() {
    let mut f = NamedTempFile::new().unwrap();
    write!(f, "this is = = not valid toml [[[").unwrap();
    let (cfg, warn) = Config::load_reporting(f.path());
    assert_eq!(cfg.log.file_level, "info"); // fell back to default
    assert!(warn.is_some());
}

#[test]
fn load_reporting_absent_file_is_not_a_warning() {
    let (cfg, warn) = Config::load_reporting(Path::new("/no/such/rustline/config.toml"));
    assert_eq!(cfg.log.file_level, "info");
    assert!(warn.is_none());
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p rustline-core log_config load_reporting`
Expected: FAIL — `LogConfig`/`log`/`load_reporting` do not exist yet (compile errors).

- [ ] **Step 4: Add the `LogConfig` type**

In `crates/rustline-core/src/config.rs`, near the other option structs (e.g. after `DateTimeOpts`), add:

```rust
/// Logging configuration: per-sink level thresholds and an optional log-file
/// path override. Level strings are parsed leniently by the binary — an
/// unknown value falls back to that sink's default rather than failing the
/// whole config parse, so `Config::load` stays total (invariant #3). Do NOT
/// promote these to an enum: a `#[derive(Deserialize)]` enum would make a
/// typo in `file_level` discard the entire config (layout, theme, plugins).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogConfig {
    /// File-sink level: off|error|warn|info|debug|trace. Default "info".
    #[serde(default = "default_file_level")]
    pub file_level: String,
    /// stderr-sink level: off|error|warn|info|debug|trace. Default "error".
    #[serde(default = "default_stderr_level")]
    pub stderr_level: String,
    /// Log-file path override (`~/` expanded by the binary). Default:
    /// `$XDG_DATA_HOME/rustline/rustline.log`.
    #[serde(default)]
    pub file: Option<String>,
}

fn default_file_level() -> String {
    "info".into()
}

fn default_stderr_level() -> String {
    "error".into()
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            file_level: default_file_level(),
            stderr_level: default_stderr_level(),
            file: None,
        }
    }
}
```

- [ ] **Step 5: Add the `log` field to `Config`**

In `crates/rustline-core/src/config.rs`, add as the **last** field of `struct Config` (after `plugins`):

```rust
    /// File + stderr logging configuration.
    #[serde(default)]
    pub log: LogConfig,
```

- [ ] **Step 6: Refactor `load` into `load_reporting` + wrapper**

Replace the existing `Config::load` body (currently `crates/rustline-core/src/config.rs:229-241`) with:

```rust
    /// Load config from `path`, never failing: a missing file or a parse
    /// error both yield [`Config::default`] (the latter after logging a
    /// warning), so the status line keeps rendering.
    pub fn load(path: &Path) -> Config {
        let (config, warning) = Config::load_reporting(path);
        if let Some(msg) = warning {
            tracing::warn!("{msg}");
        }
        config
    }

    /// Like [`Config::load`] but *returns* the failure message instead of
    /// logging it, so a caller can install its logging subscriber first and
    /// then emit the warning into it. `None` = success or an absent file
    /// (absence is not a warning); `Some(msg)` = a present-but-unparseable
    /// file (config defaulted).
    pub fn load_reporting(path: &Path) -> (Config, Option<String>) {
        let Ok(text) = fs::read_to_string(path) else {
            return (Config::default(), None);
        };
        match toml::from_str(&text) {
            Ok(config) => (config, None),
            Err(error) => (
                Config::default(),
                Some(format!(
                    "invalid config at {}: {error}; using defaults",
                    path.display()
                )),
            ),
        }
    }
```

- [ ] **Step 7: Re-export `LogConfig`**

In `crates/rustline-core/src/lib.rs:13`, change:

```rust
pub use config::{Config, PluginConfig};
```
to:
```rust
pub use config::{Config, LogConfig, PluginConfig};
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p rustline-core`
Expected: PASS — the new tests plus all existing config tests (incl. `plugin_config_roundtrip_preserves_options`, which still only asserts plugin fields).

- [ ] **Step 9: Lint + format**

Run: `cargo fmt --all && cargo clippy -p rustline-core --all-targets -- -D warnings`
Expected: no diffs, no warnings.

- [ ] **Step 10: Commit**

```bash
git add crates/rustline-core/src/config.rs crates/rustline-core/src/lib.rs crates/rustline-core/Cargo.toml Cargo.lock
git commit -m "feat(config): add [log] table and Config::load_reporting"
```

(`Cargo.lock` may be unchanged if `tempfile` was already resolved for the workspace; include it if `git status` shows it modified.)

---

### Task 2: logging module + `-v` flag + main wiring

Creates the two-sink subscriber and its pure helpers, adds the global `-v` count flag, and rewires `main` to load config → install logging → emit the deferred config warning.

**Files:**
- Create: `crates/rustline/src/logging.rs`
- Modify: `crates/rustline/src/cli.rs` (add `verbose` to `Cli`)
- Modify: `crates/rustline/src/main.rs` (add `mod logging;`; replace the old subscriber init; new load/init ordering)
- Modify: `crates/rustline/Cargo.toml` (drop `env-filter` feature from `tracing-subscriber`)

**Interfaces:**
- Consumes: `rustline_core::LogConfig`, `rustline_core::Config::load_reporting` (Task 1); `rustline_wasm::{data_root, expand_tilde}`.
- Produces: `logging::init(cfg: &LogConfig, verbose: u8)`; `Cli.verbose: u8`.

- [ ] **Step 1: Write the logging module with failing unit tests**

Create `crates/rustline/src/logging.rs`:

```rust
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
/// `$XDG_DATA_HOME/rustline/rustline.log`.
fn log_path(cfg: &LogConfig) -> PathBuf {
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
        assert_eq!(
            resolve_file_level(2, "trace").0,
            LevelFilter::INFO
        );
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
            f.write_all(&vec![b'x'; 200]).unwrap(); // exceed cap
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
```

- [ ] **Step 2: Run the logging unit tests to verify they fail**

Run: `cargo test -p rustline --lib` (or `cargo test -p rustline logging`)
Expected: FAIL to compile — `mod logging;` is not yet declared and `rustline_core::LogConfig` is only available after Task 1 (which is done). The immediate failure is that `logging` isn't part of the crate.

- [ ] **Step 3: Declare the module in `main.rs`**

In `crates/rustline/src/main.rs`, add `mod logging;` to the module list at the top (keep alphabetical with the others):

```rust
mod build_context;
mod cli;
mod logging;
mod plugin_cmd;
mod tmux_conf;
```

- [ ] **Step 4: Run the logging unit tests to verify they pass**

Run: `cargo test -p rustline logging`
Expected: PASS — all 8 `logging::tests::*` pass. (The binary still uses the old init; that is replaced next.)

- [ ] **Step 5: Add the `-v` count flag to `Cli`**

In `crates/rustline/src/cli.rs`, add a `verbose` field to `struct Cli` (before `command`):

```rust
#[derive(Parser)]
#[command(version, about = "Rust tmux statusline")]
pub struct Cli {
    /// Increase file-log verbosity: -v=warn, -vv=info, -vvv=debug, -vvvv=trace.
    /// Without -v the file logs at info (or the config's `log.file_level`);
    /// stderr is unaffected (see `log.stderr_level`).
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,
    #[command(subcommand)]
    pub command: Command,
}
```

- [ ] **Step 6: Rewire `main` — drop `EnvFilter`, install `logging::init`, reorder load**

In `crates/rustline/src/main.rs`:

a) Remove the now-unused imports. Delete:
```rust
use std::io;
```
and
```rust
use tracing_subscriber::{EnvFilter, fmt};
```

b) Replace the opening of `fn main()` (currently the comment + `let filter = ...; fmt()....init();` at `main.rs:52-55`, plus the existing `let cli = Cli::parse();` / `let cfg = Config::load(...)` lines) with:

```rust
fn main() {
    let cli = Cli::parse();
    // Load config first so logging can honor `[log]`; defer the load-failure
    // warning until the subscriber exists (else it would be dropped).
    let (cfg, load_warning) = Config::load_reporting(&config_path());
    logging::init(&cfg.log, cli.verbose);
    if let Some(msg) = load_warning {
        tracing::warn!("{msg}");
    }
    let theme = cfg.to_theme();

    match cli.command {
        // ... unchanged arms ...
    }
}
```

Leave every `match cli.command` arm exactly as-is.

- [ ] **Step 7: Drop the `env-filter` feature from `tracing-subscriber`**

In `crates/rustline/Cargo.toml`, change:
```toml
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```
to:
```toml
tracing-subscriber = { version = "0.3", features = ["registry", "fmt", "ansi"] }
```

- [ ] **Step 8: Build + full test + lint**

Run:
```bash
cargo build -p rustline
cargo test -p rustline
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```
Expected: builds clean; all rustline tests pass (existing smoke tests still green — logging writes to a file/stderr, not stdout, so `render left` stdout is unchanged); no clippy/fmt diffs.

- [ ] **Step 9: Commit (include `Cargo.lock`)**

```bash
cargo fmt --all
git add crates/rustline/src/logging.rs crates/rustline/src/cli.rs crates/rustline/src/main.rs crates/rustline/Cargo.toml Cargo.lock
git commit -m "feat(logging): two-sink file+stderr subscriber with -v scale"
```

(`Cargo.lock` changes because dropping `env-filter` prunes `matchers`; `regex` stays via `rustline-wasm`.)

---

### Task 3: integration smoke tests (the seams)

Pins the load-bearing end-to-end behavior the unit tests can't reach: a WARN lands in the file, stderr stays quiet at the default, and `stderr_level` promotes it to stderr — all without disturbing stdout.

**Files:**
- Modify: `crates/rustline/tests/smoke.rs` (add a helper + two tests)

**Interfaces:**
- Consumes: the built `rustline` binary (`env!("CARGO_BIN_EXE_rustline")`), Task 1/2 behavior.

- [ ] **Step 1: Write the failing integration tests**

At the top of `crates/rustline/tests/smoke.rs`, ensure these imports exist (add any missing):

```rust
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;
```

Append these tests to the file:

```rust
/// A `rustline` invocation with an isolated HOME/XDG environment so logging
/// and config read/write a throwaway tree, never the developer's real dirs.
fn isolated_cmd(home: &Path, xdg_data: &Path, xdg_config: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_rustline"));
    c.env("HOME", home)
        .env("XDG_DATA_HOME", xdg_data)
        .env("XDG_CONFIG_HOME", xdg_config)
        .env_remove("RUST_LOG");
    c
}

#[test]
fn warning_lands_in_log_file_and_not_stderr_at_default() {
    let dir = tempdir().unwrap();
    let (home, data, config) = (
        dir.path().join("home"),
        dir.path().join("data"),
        dir.path().join("config"),
    );
    fs::create_dir_all(config.join("rustline")).unwrap();
    // An unknown widget name triggers `warn!("unknown widget, skipping")`.
    fs::write(
        config.join("rustline/config.toml"),
        "[layout]\nleft = [\"definitely_not_a_widget\"]\n",
    )
    .unwrap();

    let out = isolated_cmd(&home, &data, &config)
        .args([
            "render", "left", "--session", "0", "--window", "0", "--pane", "0",
        ])
        .output()
        .unwrap();

    assert!(out.status.success(), "render exited 0");

    // Default stderr level is ERROR, so a WARN must NOT surface on stderr.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("unknown widget"),
        "warning must not hit stderr at default level; got: {stderr}"
    );

    // The file sink (INFO) captured the WARN.
    let log = fs::read_to_string(data.join("rustline/rustline.log"))
        .expect("log file created");
    assert!(
        log.contains("unknown widget"),
        "warning captured in log file; got: {log}"
    );
}

#[test]
fn stderr_level_override_promotes_warning_to_stderr() {
    let dir = tempdir().unwrap();
    let (home, data, config) = (
        dir.path().join("home"),
        dir.path().join("data"),
        dir.path().join("config"),
    );
    fs::create_dir_all(config.join("rustline")).unwrap();
    fs::write(
        config.join("rustline/config.toml"),
        "[layout]\nleft = [\"definitely_not_a_widget\"]\n\n[log]\nstderr_level = \"warn\"\n",
    )
    .unwrap();

    let out = isolated_cmd(&home, &data, &config)
        .args([
            "render", "left", "--session", "0", "--window", "0", "--pane", "0",
        ])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown widget"),
        "stderr_level=warn surfaces the warning on stderr; got: {stderr}"
    );
}
```

- [ ] **Step 2: Run to verify they pass**

Run: `cargo test -p rustline --test smoke warning_lands_in_log_file_and_not_stderr_at_default stderr_level_override_promotes_warning_to_stderr`
Expected: PASS. (If a stray `use` was already present and now duplicated, dedupe it — `cargo test` will name the duplicate import.)

- [ ] **Step 3: Run the whole smoke suite + lint**

Run:
```bash
cargo test -p rustline --test smoke
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```
Expected: all smoke tests pass; no diffs/warnings.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
git add crates/rustline/tests/smoke.rs
git commit -m "test(logging): seam tests for file capture + stderr override"
```

---

### Task 4: documentation + TODO cleanup

Brings the living docs in line with the new behavior and removes the completed TODO item.

**Files:**
- Modify: `CLAUDE.md`
- Modify: `README.md`
- Modify: `TODO.md`

**Interfaces:** none (docs only).

- [ ] **Step 1: Update `CLAUDE.md` — module map**

In the `rustline` (bin) module-map list, add a bullet for the new module (keep the existing bullets):

```markdown
- `logging.rs` — `init(&LogConfig, verbose)`: installs the two-sink `tracing`
  subscriber (rotated file + stderr), plus the pure helpers `verbosity_to_level`,
  `parse_level`, `resolve_file_level`/`resolve_stderr_level`, `should_rotate`,
  `open_log`, `log_path`. Best-effort: a file that can't be opened degrades to
  stderr-only; never writes stdout.
```

In the `rustline-core` `config.rs` bullet, append a sentence:

> `Config::load_reporting` returns the load-failure message instead of logging
> it, so the binary can install its log subscriber first and then emit the
> `"invalid config"` warning into the file.

- [ ] **Step 2: Update `CLAUDE.md` — Config + a Logging note**

In the **Config** section, add a subsection:

```markdown
**Logging:** a `[log]` table controls the two sinks. `rustline` logs to a
rotated file (`$XDG_DATA_HOME/rustline/rustline.log`, default level `info`) and
to stderr (default level `error`). `RUST_LOG` is **not** consulted. Raise the
*file* level with repeated `-v` (`-v`=warn, `-vv`=info, `-vvv`=debug,
`-vvvv`=trace); `-v` never affects stderr. The file rotates to `rustline.log.1`
once it passes 5 MiB (one generation kept). Any level value is `off|error|warn|
info|debug|trace` and is parsed leniently (a typo falls back to the default).

    [log]
    file_level   = "info"
    stderr_level = "error"
    file         = "~/.local/share/rustline/rustline.log"   # optional override
```

- [ ] **Step 3: Update the `CLI` section of `CLAUDE.md`**

Note the global flag near the CLI command list:

> A global `-v`/`--verbose` (repeatable) raises the **file** log level:
> `-v`=warn, `-vv`=info, `-vvv`=debug, `-vvvv`=trace. Works in any position
> (`rustline -vv render left`).

- [ ] **Step 4: Add a Logging section to `README.md`**

Insert a new `## Logging` section (near the Configuration docs) reading:

```markdown
## Logging

rustline writes logs to `~/.local/share/rustline/rustline.log`
(`$XDG_DATA_HOME/rustline/rustline.log`) at `info` by default, and error-level
messages to stderr. The file rotates to `rustline.log.1` once it exceeds 5 MiB.

Raise the file verbosity with repeated `-v` (file sink only):

| flag    | file level |
|---------|-----------|
| (none)  | info       |
| `-v`    | warn       |
| `-vv`   | info       |
| `-vvv`  | debug      |
| `-vvvv` | trace      |

Override either sink in `config.toml` (`RUST_LOG` is not used):

    [log]
    file_level   = "info"    # off|error|warn|info|debug|trace
    stderr_level = "error"
    file         = "~/.local/share/rustline/rustline.log"   # optional
```

- [ ] **Step 5: Remove the completed item from `TODO.md`**

Delete the logging line from `TODO.md`. If it becomes the only content, leave the file empty (do not delete the file).

- [ ] **Step 6: Sanity-check docs render + nothing else changed**

Run: `git diff --stat`
Expected: only `CLAUDE.md`, `README.md`, `TODO.md` changed.

- [ ] **Step 7: Commit**

```bash
git add CLAUDE.md README.md TODO.md
git commit -m "docs: document file logging, [log] config, and -v scale"
```

---

## Self-Review

**1. Spec coverage:**
- File sink in `~/.local/share`, INFO default → Task 2 (`log_path`, `open_log`, file layer) + Task 1 (default `file_level="info"`). ✓
- Errors also to stderr by default → Task 2 (stderr layer, default ERROR). ✓
- `-v` raises file sink only (WARN/INFO/DEBUG/TRACE) → Task 2 (`verbosity_to_level`, global count flag). ✓
- Config overrides both sink levels → Task 1 (`[log]` table) + Task 2 (`resolve_*`). ✓
- `RUST_LOG` dropped → Task 2 (env-filter feature removed, plain `LevelFilter`). ✓
- Size-cap rotation keeping one old file → Task 2 (`should_rotate`/`open_log`) + unit tests. ✓
- Load ordering preserves the "invalid config" warning → Task 1 (`load_reporting`) + Task 2 (main sequence). ✓
- Degrade to stderr-only on file-open failure → Task 2 (`init`, `Option<Layer>`). ✓
- Lenient level parsing keeps `Config::load` total → Task 1 (`String` fields) + Task 2 (`parse_level`). ✓
- Seam tests (file capture, stderr quiet at default, stderr override, stdout intact) → Task 3. ✓
- Docs (CLAUDE.md, README.md, TODO.md) → Task 4. ✓

**2. Placeholder scan:** no TBD/TODO/"handle errors"/"similar to"; every code step carries complete code. ✓

**3. Type consistency:** `LogConfig`/`file_level`/`stderr_level`/`file` identical across Tasks 1–2; `load_reporting -> (Config, Option<String>)` consumed exactly in Task 2's `main`; `logging::init(&LogConfig, u8)` and `Cli.verbose: u8` match; `verbosity_to_level`/`parse_level`/`resolve_file_level`/`resolve_stderr_level`/`should_rotate`/`open_log`/`log_path` names are stable between the module body and its tests. ✓
