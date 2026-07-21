# rustline file logging — design

- **Date:** 2026-07-21
- **Status:** approved (brainstorm)
- **Topic:** persist rustline's `tracing` output to a rotated file under the XDG
  data dir, keep errors on stderr, and control levels via a `-v` flag + config.

## Motivation

`TODO.md`:

> All logging to a file in `~/.local/share`. Error-level logs are also written
> to stderr by default. Default file logging level is info. Both can be
> overridden by the config file, or by specifying more levels of `-v` on the
> command line. `-v` goes to WARN, `-vv` goes to INFO, etc.

Today `crates/rustline/src/main.rs` installs a single stderr `fmt` subscriber
driven by `RUST_LOG`/`EnvFilter`, defaulting to `warn` (`main.rs:54-55`). Because
tmux runs `rustline render …` as a shell-out on every `status-interval`, any
stderr a widget/plugin warning produces is effectively invisible (tmux captures
stdout for the bar and discards/obscures stderr). There is no durable record of
warnings like "unknown widget", "plugin name mismatch", or "invalid config,
using defaults". This feature gives rustline a persistent, size-bounded log file
while keeping genuine errors on stderr for interactive/manual runs.

## Resolved decisions (from brainstorming)

1. **`-v` raises only the file sink.** stderr is fixed at ERROR (config can
   change it) and is never affected by `-v`.
2. **`RUST_LOG` is dropped.** No `EnvFilter`; each sink gets a plain
   `LevelFilter`. Levels come only from `-v` and config. (This is a deliberate
   departure from the global rust-crate-decisions default of RUST_LOG+EnvFilter,
   chosen for a single simple mental model on a statusline tool.)
3. **Size-capped rotation, keep one old file.** At open time, if the log exceeds
   5 MiB, rename it to `rustline.log.1` (overwriting any prior `.1`), then open a
   fresh file in append mode. No new dependency, no background writer thread.

## Design

### Two-sink subscriber

Replace the single stderr subscriber with a `tracing_subscriber::registry()`
carrying two independently-filtered `fmt` layers:

| Sink | Writer | ANSI | Default level | Level resolution (highest → lowest) |
|------|--------|------|---------------|-------------------------------------|
| **File** | `~/.local/share/rustline/rustline.log` (append) | off | INFO | `-v` (if present) › `config.log.file_level` › **INFO** |
| **stderr** | `io::stderr` | on | ERROR | `config.log.stderr_level` › **ERROR** |

- File layer: `.with_ansi(false)` (no color escapes in the file),
  `.with_writer(file_writer)`, `.with_filter(file_level)`.
- stderr layer: default ANSI, `.with_writer(io::stderr)`,
  `.with_filter(stderr_level)`.
- `"off"` is a valid level for either sink and disables it
  (`LevelFilter::OFF`).
- The `env-filter` feature of `tracing-subscriber` is removed from
  `crates/rustline/Cargo.toml` (RUST_LOG is gone). The remaining default
  features (`fmt`, `registry`, `std`, `ansi`) provide layer filtering
  (`Layer::with_filter`) and `registry()`.

### The `-v` scale (file sink only)

A global clap count flag on the top-level `Cli`:

```rust
/// Increase file-log verbosity: -v=warn, -vv=info, -vvv=debug, -vvvv=trace.
#[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count, global = true)]
pub verbose: u8,
```

`global = true` so it parses in any position (`rustline -vv render left` and
`rustline render left -vv` both work). clap reserves `-V`/`--version` for
`#[command(version)]`, so `-v` is free.

Mapping (`verbosity_to_level`), ERROR-based, applied **only when `-v` is
present** — `count == 0` means "no override, use config/default":

| `-v` count | file level |
|-----------|-----------|
| 0 | `None` → config/default (INFO) |
| 1 | WARN |
| 2 | INFO |
| 3 | DEBUG |
| ≥4 | TRACE (clamped) |

Note the intended quirk from the TODO: with no flag the file is INFO, and a
single `-v` *lowers* it to WARN. That is the literal spec and is preserved.

### Config surface

Add a `[log]` table to `rustline-core`'s `Config` (in `config.rs`), all fields
`#[serde(default)]` so a missing or partial table falls back to defaults
(preserves invariant #3, `Config::load` totality):

```toml
[log]
file_level   = "info"    # off | error | warn | info | debug | trace
stderr_level = "error"
file         = "~/.local/share/rustline/rustline.log"   # optional path override
```

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogConfig {
    #[serde(default = "default_file_level")]   // "info"
    pub file_level: String,
    #[serde(default = "default_stderr_level")] // "error"
    pub stderr_level: String,
    #[serde(default)]
    pub file: Option<String>,   // path override; ~/ expanded by the bin
}
```

Levels are stored as **`String`** and parsed leniently at logging-init time
(`parse_level`), **deliberately not an enum**. Rationale (leave a code comment
so `/typecheck` does not "promote" it): a `#[derive(Deserialize)]` enum would
make a typo in `file_level` a hard `toml` parse error, and because
`Config::load` is total, that error would discard the *entire* config —
resetting the user's layout, theme, and plugin allowlists over a single
mistyped log level. A lenient string keeps the blast radius to just the log
level (unknown → default + a `warn!`).

`Config` gains:

```rust
#[serde(default)]
pub log: LogConfig,
```

### Default log path

```rust
// bin, logging.rs
fn log_path(cfg: &LogConfig) -> PathBuf {
    match &cfg.file {
        Some(p) => rustline_wasm::expand_tilde(p),
        None => rustline_wasm::data_root().join("rustline.log"),
    }
}
```

`rustline_wasm::data_root()` already yields `$XDG_DATA_HOME/rustline` (fallback
`~/.local/share/rustline`); the bin already depends on `rustline-wasm` and uses
its `expand_tilde`/`default_plugin_dir`, so this reuses the established path
convention rather than duplicating XDG logic.

### Rotation + file open

```rust
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;   // 5 MiB

fn should_rotate(size: u64, cap: u64) -> bool { size > cap }

/// Ensure the parent dir exists, rotate if oversized, open in append mode.
fn open_log(path: &Path, cap: u64) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Ok(meta) = fs::metadata(path) {
        if should_rotate(meta.len(), cap) {
            let mut rotated = path.as_os_str().to_owned();
            rotated.push(".1");                 // rustline.log -> rustline.log.1
            let _ = fs::rename(path, PathBuf::from(rotated));  // best-effort
        }
    }
    OpenOptions::new().create(true).append(true).open(path)
}
```

- The rotated name is built by appending `.1` to the **full path**
  (`rustline.log` → `rustline.log.1`), not via `with_extension`.
- Concurrent short-lived writers are safe: `rename(2)` is atomic; if two
  processes both rotate, the second just overwrites `.1` again. A process
  holding an fd to a just-rotated inode keeps appending to `rustline.log.1`
  (no loss, no corruption). New processes open the fresh file.
- `O_APPEND` makes each `write(2)` position at EOF atomically. Interleaving is
  at write-call granularity; a warning storm from multiple processes could in
  principle split a line — accepted as a cosmetic limitation, not worth
  cross-process `flock` in v1.

### Writer (lock-free)

```rust
struct FileWriter(Arc<File>);

impl<'a> MakeWriter<'a> for FileWriter {
    type Writer = &'a File;              // std provides `impl Write for &File`
    fn make_writer(&'a self) -> &'a File { &self.0 }
}
```

No `Mutex`: the kernel's `O_APPEND` provides the atomicity, and `&File: Write`
lets multiple in-process threads share one fd. (rustline render is effectively
single-threaded, but this stays correct if the wasm host ever writes from a
worker.)

### Init ordering (preserve the config-load warning)

The file level now comes *from* config, so the subscriber can only be built
*after* `Config::load`. But `Config::load` currently emits
`warn!("invalid config, using defaults")` (`config.rs:237`) — if that fires
before any subscriber exists, `tracing` drops it, and losing precisely that
warning in a feature whose whole point is capturing warnings would be a
regression.

Fix: make config loading *report* its failure instead of logging it inline, so
`main` can emit it after the subscriber is up.

- Add `Config::load_reporting(path) -> (Config, Option<String>)` containing the
  current body; the returned `Option<String>` is the human warning message
  (`None` on success or a cleanly-absent file).
- `Config::load` becomes a thin wrapper that calls `load_reporting` and emits
  the warning via `tracing::warn!` (unchanged behavior for existing callers and
  tests).

`main` sequence:

```rust
let cli = Cli::parse();                          // -> cli.verbose: u8
let (cfg, load_warning) = Config::load_reporting(&config_path());
logging::init(&cfg.log, cli.verbose);            // installs subscriber; emits its own deferred warnings
if let Some(msg) = load_warning {
    tracing::warn!("{msg}");                      // now captured by the file sink
}
// ... existing dispatch unchanged
```

`logging::init` itself may need to warn about (a) an unparseable level string
and (b) a failed file open. It computes levels + opens the file first, installs
the subscriber, then emits those deferred warnings so they too land in the sinks.

### Degradation (never break the bar)

- If `open_log` fails (permissions, read-only home, etc.), install a
  **stderr-only** subscriber and emit one `error!`/`warn!` about the failed log
  path. rustline continues normally — stdout (the status line) is never touched
  by logging.
- An unparseable `file_level`/`stderr_level` falls back to its default
  (INFO/ERROR) plus a deferred `warn!`.
- Logging init is best-effort and infallible from `main`'s perspective; it never
  returns an error that could abort rendering.

### Applies to all subcommands

`logging::init` runs once for every invocation (render, init, print-config,
plugin …), matching today's behavior where the subscriber is set up before the
`match`. Creating/opening the log file on `rustline init`/`print-config` is a
harmless side effect; keeping it uniform means `plugin add` warnings are logged
too.

## Module layout

- **`crates/rustline/src/logging.rs`** (new): `init(&LogConfig, verbose: u8)`,
  the `FileWriter` `MakeWriter`, and the pure helpers `verbosity_to_level`,
  `parse_level`, `resolve_file_level`, `resolve_stderr_level`, `should_rotate`,
  `open_log`, `log_path`, plus `MAX_LOG_BYTES`.
- **`crates/rustline/src/main.rs`**: drop the `EnvFilter`/`fmt` init; call
  `logging::init`; adopt the `load_reporting` ordering; add `mod logging;`.
- **`crates/rustline/src/cli.rs`**: add the global `verbose: u8` count flag to
  `Cli`.
- **`crates/rustline-core/src/config.rs`**: add `LogConfig`, the `log` field on
  `Config`, and `Config::load_reporting`; keep `Config::load` as a wrapper.
- **`crates/rustline/Cargo.toml`**: drop the `env-filter` feature from
  `tracing-subscriber`.

Core stays free of `tracing-subscriber`: it owns only the `[log]` *schema*
(plain serde strings); the bin owns interpretation (string → `LevelFilter`) and
all I/O.

## Invariants this feature depends on

- **Invariant #1 (Context is the sole render input):** untouched — logging is a
  process-lifecycle concern, not a render-time read.
- **Invariant #3 (`Config::load` is total):** must hold for the new `[log]`
  table. Guaranteed by all-`#[serde(default)]` fields *and* lenient string level
  parsing (a bad level never fails the parse). The `load_reporting` refactor
  keeps totality (same fallback, warning just deferred).
- **stdout is the status line:** logging writes only to the file and stderr,
  never stdout. Any new `println!`/stdout write in logging is forbidden.

## Testing plan

Pure logic is unit-tested in `logging.rs`; the wiring is covered by a smoke test
(the global subscriber can only init once per process, so keep decisions pure).

Unit (in `logging.rs`):
- `verbosity_to_level`: 0→None; 1→WARN; 2→INFO; 3→DEBUG; 4 and 5→TRACE.
- `parse_level`: each of off/error/warn/info/debug/trace (case-insensitive,
  trimmed); unknown → `None`.
- `resolve_file_level`: `-v` beats config; config used when `verbose == 0`;
  unknown config string → INFO default; `verbose >= 1` ignores config.
- `resolve_stderr_level`: config value; unknown/empty → ERROR default.
- `should_rotate`: boundary (`== cap` false, `> cap` true).
- `open_log` (tempfile): (a) writing >5 MiB then reopening produces
  `rustline.log.1` and a fresh empty `rustline.log`; (b) opening in a
  non-existent dir creates the dir and the file; (c) a small file is *not*
  rotated.

Unit (in `config.rs`):
- `[log]` round-trips; a missing `[log]` table yields the INFO/ERROR defaults;
  a partial table (only `file_level`) keeps the other defaults.
- `load_reporting`: a valid file → `(cfg, None)`; a malformed file →
  `(Config::default(), Some(_))`; an absent file → `(Config::default(), None)`
  (absence is not a warning today).

Integration (`crates/rustline/tests/smoke.rs`) — pins the load-bearing seams the
unit tests can't reach (per the project's "test at the seam" guidance):
- Run the binary as a subprocess with a temp `HOME` + `XDG_DATA_HOME` and a
  config whose layout references an **unknown widget** (which triggers
  `warn!("unknown widget, skipping")`). Assert:
  - `$XDG_DATA_HOME/rustline/rustline.log` exists and **contains** that warning
    line (proves file sink at INFO captures WARN);
  - the warning does **not** appear on stderr at the default stderr level
    (ERROR) — i.e. stderr stays quiet for non-errors;
  - stdout still contains the rendered region markup (logging didn't disturb the
    bar).
- A second case runs with `[log] stderr_level = "warn"` and asserts the same
  warning now *also* appears on stderr (proves `stderr_level` override).

## Documentation updates (part of this branch)

- `CLAUDE.md`: document the `[log]` config table, the `-v` verbosity scale, the
  default log path + 5 MiB rotation, and that `RUST_LOG` is no longer consulted;
  note the new `logging.rs` in the `rustline` (bin) module map and the
  `load_reporting` addition in the `rustline-core` map; add a logging line to the
  Invariants/Config sections as appropriate.
- `README.md`: a short "Logging" note (path, default level, `-v`, config keys).
- Remove the completed item from `TODO.md`.

## Out of scope (YAGNI)

- Configurable `max_bytes` (the 5 MiB cap is a constant).
- More than one rotated generation (`.2`, `.3`, …).
- Daily/time-based rotation.
- Per-module filter directives (dropped with `RUST_LOG`).
- A daemon-mode long-lived logger (the pure helpers stay reusable if that
  arrives).
