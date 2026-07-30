# code-health batch 2 — caching, logging, and the write capability — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the twelve `[x] execute` findings from `bughunt.md` — bounded and self-healing caches, a log that survives the daemon and does not drown in repeats, and a filesystem write capability that is granted separately from read and cannot be redirected by a symlink.

**Architecture:** Three infrastructure pieces land first and the rest consume them: `tracing-appender`'s `RollingFileAppender` replaces the hand-rolled rotation (D1); a cross-process `warn_once` marker directory keyed on config mtime dedups the ~22 per-render WARN sites (D2); and one shared `write_atomic` primitive replaces four hand-rolled temp-file dances plus two raw `fs::write` calls (B15+B17). The remaining items are surgical edits to `crates/rustline-wasm/src/{cache,perform,state,capability,denials}.rs` and `crates/rustline/src/{daemon_client,daemon}.rs`. The largest change, D3, splits `allowed_paths` into read-only and a new `allowed_write_paths` and adds a symlink gate; it lands last so it rebases over a settled `perform.rs`.

**Tech Stack:** Rust edition 2024, workspace of five crates (`rustline-abi`, `rustline-core`, `rustline`, `rustline-wasm`, `rustline-plugin-sdk`). `tracing` + `tracing-subscriber` for logging, new `tracing-appender` dep. Extism host functions for the WASM plugin boundary. `just` for build/lint/test recipes.

## Global Constraints

- **Edition 2024** for every crate; `rustfmt.toml` is `edition = "2024"` and all workspace `Cargo.toml` editions must equal it.
- **Baseline is zero lint warnings.** `just lint` runs `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo clippy --workspace --all-targets --features wasm-e2e -- -D warnings`. Any warning your change produces is new and must be fixed.
- **Verification per task:** `cargo build --workspace`, `cargo test --workspace`, `just lint`. All three must pass before commit.
- **Commit format:** `fix(<category>): <summary> [<id>]` for fixes, `test: characterize <unit> before fix [<id>]` for the RED commit that precedes a high-risk fix.
- **Strip-on-fix (non-negotiable):** the commit that fixes a finding must also delete that finding's section from `bughunt.md`. `bughunt.md` reflects open issues only.
- **Invariant N1 — gate first:** a denied capability request makes no network call, spawns no process, and touches no cache file.
- **Invariant N2 — a plugin must never break the bar:** every new failure path degrades to a denied/empty result, never a panic.
- **Invariant N3 — state quota:** `check_cap` must still refuse a write that would exceed `max_state_bytes`; the check stays strictly before the write.
- **Invariant N4 — per-plugin scoping:** new per-`CapabilityCtx` state is per-instance, never shared between plugins.
- **Config rule #3 — `Config::load` is total:** new config keys are `#[serde(default)]` so an old config still loads.
- **`plugins/*` are excluded from the workspace** and each carries its own `Cargo.lock` and an empty `[workspace]` table. `just check-lock` verifies all six locks.
- **Do not refactor existing tests.** Fix production code and add NEW tests. Deleting a test is allowed only where this plan explicitly says so (Task 1's two rotation tests).

## File Structure

**New files:**
- `crates/rustline-core/src/diag.rs` — the `warn_once` hook seam (lowest common crate; `rustline-wasm` and `rustline` both depend on `rustline-core`).
- `crates/rustline/src/warn_once.rs` — the real marker-directory implementation, installed into the seam by `main`.

**Modified, by responsibility:**
- `crates/rustline/src/logging.rs` — rotation strategy (Task 1).
- `crates/rustline/src/main.rs` — installs the warn-once hook (Task 2).
- `crates/rustline-core/src/{widget.rs,widgets/mod.rs,widgets/datetime.rs}` + `crates/rustline-wasm/src/{lib.rs,allow.rs}` — warn-site conversions (Task 2), instance-opts helper (Task 3).
- `crates/rustline-wasm/src/cache.rs` — `last_attempt_at` (Task 4), eviction (Task 5), atomic write (Task 6), size accounting (Task 7).
- `crates/rustline-wasm/src/perform.rs` — cached-fetch backoff (Task 4), atomic state write (Task 6), size memo (Task 7), log caps (Task 8), path gates (Task 11).
- `crates/rustline-wasm/src/state.rs` — pure `check_cap` (Task 7), `resolve_for_allowlist` (Task 11).
- `crates/rustline-wasm/src/capability.rs` — memo (Task 7), log counter (Task 8), write allowlist (Task 11).
- `crates/rustline-wasm/src/denials.rs` — `record` returns newly-seen (Task 9).
- `crates/rustline/src/{daemon_client.rs,daemon.rs}` — fallback diagnostics (Task 10).
- `crates/rustline/src/{plugin_cmd.rs,cli.rs}` + `crates/rustline-wasm/src/manifest.rs` — the grant split's user surface (Task 11).
- `CLAUDE.md`, `README.md` — doc sync (Task 12).

---

## Task 1: Replace hand-rolled log rotation with `tracing-appender` [D1]

**Files:**
- Modify: `crates/rustline/Cargo.toml` (add dep)
- Modify: `crates/rustline/src/logging.rs` (replace `open_log`/`FileWriter`/`should_rotate`/`MAX_LOG_BYTES`; rework `log_path`)
- Modify: `crates/rustline/src/main.rs:330` (the `log_file:` field passed to doctor)
- Modify: `crates/rustline/src/doctor.rs` (label for the log path)
- Modify: `bughunt.md` (strip the `FileWriter` decision-needed marker)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `logging::log_dir(cfg: &LogConfig) -> PathBuf`, `logging::log_file_prefix(cfg: &LogConfig) -> String`, `logging::log_file_suffix(cfg: &LogConfig) -> String`, and `logging::current_log_path(cfg: &LogConfig) -> PathBuf` (the directory joined with `<prefix>.<today>.<suffix>`), all `pub(crate)`. `logging::init(cfg: &LogConfig, verbose: u8)` keeps its signature and still returns `()`.

**Context:** `open_log` (logging.rs:97) evaluates rotation once at process start and `FileWriter` holds that `Arc<File>` forever. `rustline daemon run` never exits, so after the second rotation it appends to an unlinked inode and every daemon diagnostic is lost. The user's decision, recorded in `bughunt.md`, is to adopt `RollingFileAppender`.

**Accepted behaviour changes (user-approved, do not re-litigate):** rotation becomes daily rather than at 5 MiB; retention becomes 7 daily generations rather than one `.1`; the file gains a date component. Note the shape: `tracing-appender`'s `join_date` emits `{prefix}.{date}.{suffix}`, i.e. **`rustline.2026-07-26.log`** — not `rustline.log.2026-07-26`.

- [ ] **Step 1: Add the dependency**

In `crates/rustline/Cargo.toml`, in `[dependencies]`, after the `tracing-subscriber` line:

```toml
# Daily log rotation that re-evaluates its target on every write. The
# hand-rolled size rotation it replaces decided once at process start, so the
# long-lived `daemon run` process kept appending to an unlinked inode after the
# second rotation and lost every diagnostic. Used as a blocking `MakeWriter`,
# NOT via `non_blocking`: that returns a `WorkerGuard` which must outlive the
# subscriber, and short-lived `render` processes would drop it and lose logs.
tracing-appender = "0.2.5"
```

- [ ] **Step 2: Write the failing tests**

Replace the `open_log_rotates_when_oversized` and `open_log_keeps_small_file` tests in `crates/rustline/src/logging.rs`'s `mod tests` — those two pin the size-based behaviour this task removes and are the only tests you may delete. Also delete `should_rotate_is_strict_greater_than`. Add:

```rust
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
```

If `LogConfig`'s fields are not all public or it lacks `Default`, construct it however the existing tests in `crates/rustline-core/src/config.rs` do; do not change `LogConfig`'s shape for the test's convenience.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p rustline logging:: 2>&1 | tail -20`
Expected: FAIL — `cannot find function log_dir`, `log_file_prefix`, `log_file_suffix`, `current_log_path`, `build_appender`.

- [ ] **Step 4: Implement**

In `crates/rustline/src/logging.rs`: delete `MAX_LOG_BYTES`, `should_rotate`, `open_log`, and `struct FileWriter` with its `MakeWriter` impl. Adjust the `use` block (`File`, `OpenOptions`, `Arc` become unused; add `tracing_appender::rolling::{Builder, RollingFileAppender, Rotation}`). Update the module doc's first bullet to say daily rotation with 7 retained generations rather than size rotation.

```rust
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
/// The date is **UTC** because tracing-appender's rotation boundary is UTC
/// (`OffsetDateTime::now_utc()`, unconditionally). Localizing this would make
/// `doctor` print a path that does not exist for part of every day.
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
fn build_appender(
    dir: &Path,
    prefix: &str,
    suffix: &str,
) -> Result<RollingFileAppender, tracing_appender::rolling::InitError> {
    Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix(prefix)
        .filename_suffix(suffix)
        .max_log_files(MAX_LOG_FILES)
        .build(dir)
}
```

**There must be exactly ONE builder chain.** `init` calls `build_appender` — it does not inline a second copy. A `#[cfg(test)]`-only helper that duplicates production's chain is worse than no helper: the test then passes against a stale replica while `init`'s real chain drifts (a dropped `max_log_files`, a changed `Rotation`) undetected. Returning the `Result` also keeps the panic out of the binary entirely, so `init` stays infallible (N2) without an `expect` anywhere:

```rust
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
```

`build_appender` is both the production path and the test seam (used by `appender_writes_into_the_configured_dir`). Do **not** gate it `#[cfg(test)]` — it is called from `init`, so it is live code and clippy will not flag it.

- [ ] **Step 5: Update the two consumers**

`crates/rustline/src/main.rs:330` currently passes `log_file: &logging::log_path(&cfg.log)`. Change to `log_file: &logging::current_log_path(&cfg.log)`.

In `crates/rustline/src/doctor.rs`, find the row that reports the log file and update its label/help text so it names the rotation scheme — e.g. append `(daily, 7 kept)` to the reported value's description. Read the surrounding rows and match their exact formatting; do not invent a new row shape.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p rustline logging:: 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 7: Full verification**

Run: `cargo build --workspace && cargo test --workspace && just lint`
Expected: all green, zero warnings.

- [ ] **Step 8: Strip the finding and commit**

Delete from `bughunt.md` the entire `> **decision-needed (architectural):** \`FileWriter\` …` block at lines 53-55 (the marker paragraph, its `User Note:` line, and its checkbox line).

```bash
git add crates/rustline/Cargo.toml Cargo.lock crates/rustline/src/logging.rs \
        crates/rustline/src/main.rs crates/rustline/src/doctor.rs bughunt.md
git commit -m "fix(observability): rotate the log with tracing-appender so the daemon follows it [D1]"
```

---

## Task 2: Cross-process `warn_once` infrastructure and site conversion [D2]

**Files:**
- Create: `crates/rustline-core/src/diag.rs`
- Create: `crates/rustline/src/warn_once.rs`
- Modify: `crates/rustline-core/src/lib.rs` (declare + re-export `diag`)
- Modify: `crates/rustline/src/main.rs` (declare `mod warn_once`; install the hook after `logging::init`; convert 2 sites)
- Modify: `crates/rustline-core/src/widget.rs:144`, `crates/rustline-core/src/widgets/mod.rs` (4 sites), `crates/rustline-core/src/widgets/datetime.rs:34`
- Modify: `crates/rustline-wasm/src/lib.rs` (10 plugin-skip warns), `crates/rustline-wasm/src/allow.rs:56`
- Modify: `bughunt.md` (strip the `Registry::resolve` decision-needed marker)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `rustline_core::diag::warn_once(key: &str, emit: impl Fn())` — calls `emit()` unless the installed hook suppresses it. With no hook installed, always calls `emit()`.
  - `rustline_core::diag::set_warn_once_hook(hook: Box<dyn Fn(&str, &dyn Fn()) + Send + Sync>)` — idempotent; a second call is a no-op.
  - `rustline::warn_once::install(config_path: &Path)` — clears the marker dir if the config's mtime changed, then installs the real hook.

**Context:** ~22 `warn!` sites re-fire every render because each render is a fresh process re-running config load, theme resolution, registry construction and plugin registration from cold. At `status-interval 1` one typo emits ~86,400 identical lines a day, rotating the log and evicting every other diagnostic. A per-process `OnceLock` cannot help. The user approved a new cross-process state file.

- [ ] **Step 1: Write the failing test for the seam**

Create `crates/rustline-core/src/diag.rs` with only this test module for now, plus an empty `mod` declaration in `crates/rustline-core/src/lib.rs` (`pub mod diag;`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn warn_once_emits_when_no_hook_is_installed() {
        // The hook is a process-wide OnceLock and other tests in this binary
        // must not depend on install order, so this asserts only the
        // no-hook-or-permissive default: `emit` runs.
        let hits = AtomicUsize::new(0);
        warn_once("diag-test-key", || {
            hits.fetch_add(1, Ordering::Relaxed);
        });
        assert!(hits.load(Ordering::Relaxed) >= 1);
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p rustline-core diag:: 2>&1 | tail -10`
Expected: FAIL — `cannot find function warn_once`.

- [ ] **Step 3: Implement the seam**

Prepend to `crates/rustline-core/src/diag.rs`:

```rust
//! The `warn_once` seam.
//!
//! Most of this crate's `warn!` sites describe *static misconfiguration* — an
//! unknown widget name, an unparseable instance table, a bad timezone. Every
//! `rustline render` tick is a fresh process that re-derives all of it from
//! cold, so those warns re-fire once per tick: at `status-interval 1` a single
//! typo writes ~86,400 identical lines a day and evicts every other diagnostic
//! from the log.
//!
//! Deduping needs state that outlives the process, which lives in the binary
//! crate (it owns the state root). This module is the seam: `rustline`'s
//! `main` installs a hook right after `logging::init`, and every crate below it
//! calls [`warn_once`] without knowing where the markers live.
//!
//! **Fail open.** With no hook installed — unit tests, `rustline-core` used
//! standalone — [`warn_once`] simply emits. A dedup layer must never be the
//! reason a diagnostic goes missing.

use std::sync::OnceLock;

type WarnOnceHook = Box<dyn Fn(&str, &dyn Fn()) + Send + Sync>;

static HOOK: OnceLock<WarnOnceHook> = OnceLock::new();

/// Install the process-wide dedup hook. Idempotent: the first caller wins and
/// later calls are ignored, so a test that installs one cannot be broken by
/// another test in the same binary.
pub fn set_warn_once_hook(hook: WarnOnceHook) {
    let _ = HOOK.set(hook);
}

/// Emit `emit()` unless the installed hook has already seen `key` this
/// generation. `key` must identify both the site and its payload — e.g.
/// `"unknown-widget:memroy"` — so two different typos are two different warns.
pub fn warn_once(key: &str, emit: impl Fn()) {
    match HOOK.get() {
        Some(hook) => hook(key, &emit),
        None => emit(),
    }
}
```

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -p rustline-core diag:: 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 5: Write the failing tests for the real implementation**

Create `crates/rustline/src/warn_once.rs` with this test module (implementation follows in Step 7):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn first_call_emits_and_second_suppresses() {
        let dir = tempfile::tempdir().unwrap();
        let marks = dir.path().join("warned");
        let hits = AtomicUsize::new(0);
        let bump = || {
            hits.fetch_add(1, Ordering::Relaxed);
        };
        assert!(should_emit(&marks, "k"));
        bump();
        assert!(!should_emit(&marks, "k"));
        assert_eq!(hits.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn distinct_keys_both_emit() {
        let dir = tempfile::tempdir().unwrap();
        let marks = dir.path().join("warned");
        assert!(should_emit(&marks, "a"));
        assert!(should_emit(&marks, "b"));
    }

    #[test]
    fn an_unwritable_marker_dir_fails_open() {
        // A regular file where the marker directory should be makes every
        // marker write fail; a dedup layer must never silence diagnostics.
        let blocker = tempfile::NamedTempFile::new().unwrap();
        let marks = blocker.path().join("warned");
        assert!(should_emit(&marks, "k"));
        assert!(should_emit(&marks, "k"), "still emits when markers fail");
    }

    #[test]
    fn a_changed_generation_rearms_every_key() {
        let dir = tempfile::tempdir().unwrap();
        let marks = dir.path().join("warned");
        reset_if_generation_changed(&marks, "gen-1");
        assert!(should_emit(&marks, "k"));
        assert!(!should_emit(&marks, "k"));
        reset_if_generation_changed(&marks, "gen-2");
        assert!(should_emit(&marks, "k"), "a config edit re-arms the warn");
    }

    #[test]
    fn an_unchanged_generation_keeps_suppression() {
        let dir = tempfile::tempdir().unwrap();
        let marks = dir.path().join("warned");
        reset_if_generation_changed(&marks, "gen-1");
        assert!(should_emit(&marks, "k"));
        reset_if_generation_changed(&marks, "gen-1");
        assert!(!should_emit(&marks, "k"));
    }
}
```

Add `mod warn_once;` to `crates/rustline/src/main.rs`'s module declarations (alphabetical order with its neighbours).

- [ ] **Step 6: Run them to verify they fail**

Run: `cargo test -p rustline warn_once:: 2>&1 | tail -10`
Expected: FAIL — `cannot find function should_emit` / `reset_if_generation_changed`.

- [ ] **Step 7: Implement**

Prepend to `crates/rustline/src/warn_once.rs`:

```rust
//! Cross-process dedup for the static-misconfiguration warns (see
//! `rustline_core::diag`). Each distinct warn is recorded as a marker file
//! under `<state_root>/warned/`; the whole directory is cleared whenever the
//! config file's mtime changes, so each misconfiguration is logged once per
//! config edit rather than once per render tick.
//!
//! `create_new(true)` is atomic on every filesystem we support, so concurrent
//! renderers racing the same key produce exactly one winner without a lock.
//!
//! Best-effort throughout: any I/O failure means "emit" (invariant N2's spirit
//! — a broken dedup cache must never silence a diagnostic).

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

/// Records the config generation the current markers belong to.
const GENERATION_FILE: &str = ".generation";

fn marker_dir() -> PathBuf {
    rustline_wasm::state_root().join("warned")
}

/// A stable, filesystem-safe name for `key`. Not cryptographic — a collision
/// only means one of two warns is suppressed for one config generation.
fn marker_name(key: &str) -> String {
    let mut h = DefaultHasher::new();
    key.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// The config file's mtime as an opaque generation string; an unreadable
/// config yields a constant, which simply means "don't reset".
fn generation_of(config_path: &Path) -> String {
    std::fs::metadata(config_path)
        .and_then(|m| m.modified())
        .map(|t| format!("{t:?}"))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Clear every marker when `generation` differs from the recorded one, then
/// record `generation`. Best-effort: a failure leaves markers in place, which
/// at worst suppresses a warn for one more config edit.
fn reset_if_generation_changed(dir: &Path, generation: &str) {
    let stamp = dir.join(GENERATION_FILE);
    if std::fs::read_to_string(&stamp).as_deref() == Ok(generation) {
        return;
    }
    let _ = std::fs::remove_dir_all(dir);
    if std::fs::create_dir_all(dir).is_ok() {
        let _ = std::fs::write(&stamp, generation);
    }
}

/// `true` iff `key` has not been marked in `dir` yet. Claims the marker as a
/// side effect. Any I/O error returns `true` (fail open).
fn should_emit(dir: &Path, key: &str) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return true;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(marker_name(key)))
    {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(_) => true,
    }
}

/// Install the real dedup hook. Call once, from `main`, immediately after
/// `logging::init` — every warn site below this crate routes through it.
pub fn install(config_path: &Path) {
    let dir = marker_dir();
    reset_if_generation_changed(&dir, &generation_of(config_path));
    rustline_core::diag::set_warn_once_hook(Box::new(move |key, emit| {
        if should_emit(&dir, key) {
            emit();
        }
    }));
}
```

- [ ] **Step 8: Run them to verify they pass**

Run: `cargo test -p rustline warn_once:: 2>&1 | tail -10`
Expected: PASS (5 tests).

- [ ] **Step 9: Install the hook in `main`**

In `crates/rustline/src/main.rs`, immediately after `logging::init(&cfg.log, cli.verbose);` (line 181) and **before** the `load_warning` emit:

```rust
    logging::init(&cfg.log, cli.verbose);
    // Dedup the static-misconfiguration warns across render processes. Must
    // follow `logging::init` (the hook's own failures would otherwise be
    // dropped) and precede every warn site below.
    warn_once::install(&cfg_path);
```

- [ ] **Step 10: Convert the recurring warn sites**

Each conversion wraps the existing `tracing::warn!` in `rustline_core::diag::warn_once` with a key that includes the site name and the varying payload. Do **not** change the message text or its fields. Pattern:

```rust
// before
tracing::warn!(widget = %name, "unknown widget, skipping");
// after
rustline_core::diag::warn_once(&format!("unknown-widget:{name}"), || {
    tracing::warn!(widget = %name, "unknown widget, skipping");
});
```

Inside `rustline-core` itself use `crate::diag::warn_once`. Convert exactly these:

| file | site | key |
|---|---|---|
| `crates/rustline/src/main.rs:150` | unknown theme base | `theme-base:{name}` |
| `crates/rustline/src/main.rs:183` | invalid config (the `load_warning` emit) | `config-load:{msg}` |
| `crates/rustline-core/src/widget.rs:144` | unknown widget, skipping | `unknown-widget:{name}` |
| `crates/rustline-core/src/widgets/mod.rs:382` | instance missing `kind` | `instance-no-kind:{name}` |
| `crates/rustline-core/src/widgets/mod.rs:386` | instance name collides | `instance-collide:{name}` |
| `crates/rustline-core/src/widgets/mod.rs:393` | name > 15 bytes | `instance-long-name:{name}` |
| `crates/rustline-core/src/widgets/mod.rs:495` | unknown instance kind | `instance-kind:{name}:{other}` |
| `crates/rustline-core/src/widgets/datetime.rs:34` | unknown timezone | `timezone:{tz}` |
| `crates/rustline-wasm/src/allow.rs:56` | invalid allow pattern | `allow-pattern:{entry}` |
| `crates/rustline-wasm/src/lib.rs:112–192` | the ten plugin-skip warns | `plugin-skip:<distinct-reason>:{stem}` |

For `lib.rs`'s ten sites, read each one and give it a distinct reason slug matching its message (e.g. `plugin-skip:checksum:{stem}`, `plugin-skip:not-approved:{stem}`). The stem must be in the key so two bad plugins both warn.

**Do NOT convert** per-render *runtime* failures — "plugin render failed", "http cache write failed", "failed to write sample-store temp file", denial warns, poisoned-mutex warns. Those describe changing conditions; suppressing them across processes would hide real incidents.

`rustline-core` does not currently depend on anything new for this — `diag` is in the same crate. `rustline-wasm` already depends on `rustline-core`; verify the `use` resolves.

- [ ] **Step 11: Full verification**

Run: `cargo build --workspace && cargo test --workspace && just lint`
Expected: all green. Existing tests that assert a warn is emitted still pass, because no hook is installed in unit tests (fail-open default). If any test *does* install a hook and another then fails, that is the `OnceLock` ordering trap — keep `set_warn_once_hook` idempotent and do not install hooks in tests.

- [ ] **Step 12: Strip the finding and commit**

Delete from `bughunt.md` the `> **decision-needed (architectural):** \`Registry::resolve\` …` block at lines 57-59 (marker, `User Note:`, checkbox).

```bash
git add crates/rustline-core/src/diag.rs crates/rustline-core/src/lib.rs \
        crates/rustline-core/src/widget.rs crates/rustline-core/src/widgets/mod.rs \
        crates/rustline-core/src/widgets/datetime.rs crates/rustline/src/warn_once.rs \
        crates/rustline/src/main.rs crates/rustline-wasm/src/lib.rs \
        crates/rustline-wasm/src/allow.rs bughunt.md
git commit -m "fix(observability): dedup static-misconfiguration warns across render processes [D2]"
```

---

## Task 3: Warn when an instance options table fails to parse [B12]

**Files:**
- Modify: `crates/rustline-core/src/widgets/mod.rs` (add helper; 12 instance arms)
- Modify: `crates/rustline-core/src/config.rs` (12 `instance_meta` arms at 1737–1781; `disk_mounts` 1825, `throughput_interfaces` 1845, `spark_referenced_in_layout` 1873/1877)
- Modify: `bughunt.md` (strip B12)

**Interfaces:**
- Consumes: `rustline_core::diag::warn_once` from Task 2.
- Produces: `pub(crate) fn instance_opts<T: Default + serde::de::DeserializeOwned>(name: &str, kind: &str, v: toml::Value) -> T` in `crates/rustline-core/src/widgets/mod.rs`.

**Context:** All twelve instance arms use `t.try_into().unwrap_or_default()`, as do ~16 more sites in `config.rs`. A quoted `spark_width = "8"` discards the user's *entire* instance config — format, thresholds, colours — silently, and because the name still registers there is no "unknown widget" warn either. The three surrounding failure modes all warn correctly, which makes this silence especially misleading.

**Risk: this warn fires once per render on a persistent misconfiguration.** It MUST route through Task 2's `warn_once` or it becomes exactly the problem D2 just fixed.

- [ ] **Step 1: Write the failing tests**

Add to `crates/rustline-core/src/widgets/mod.rs`'s `mod tests`:

```rust
    #[test]
    fn instance_opts_falls_back_and_reports_a_type_error() {
        let v: toml::Value = toml::from_str("spark_width = \"8\"").unwrap();
        let got: CpuOpts = instance_opts("cpu_alt", "cpu", v);
        assert_eq!(got.spark_width, CpuOpts::default().spark_width);
    }

    #[test]
    fn instance_opts_accepts_a_valid_table() {
        let v: toml::Value = toml::from_str("spark_width = 20").unwrap();
        let got: CpuOpts = instance_opts("cpu_alt", "cpu", v);
        assert_eq!(got.spark_width, 20);
    }

    #[test]
    fn an_instance_table_with_only_the_extra_kind_key_parses_cleanly() {
        // CLAUDE.md documents the extra `kind` key as harmless; it must not be
        // reported as a type error.
        let v: toml::Value = toml::from_str("kind = \"cpu\"").unwrap();
        let got: CpuOpts = instance_opts("cpu_alt", "cpu", v);
        assert_eq!(got.spark_width, CpuOpts::default().spark_width);
    }
```

`spark_width`'s exact type and default come from `CpuOpts`; read it and adjust the literals if it is not `usize`/`8`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p rustline-core instance_opts 2>&1 | tail -10`
Expected: FAIL — `cannot find function instance_opts`.

- [ ] **Step 3: Check the extra-`kind`-key behaviour before implementing**

Read how the arms build `t` (`widgets/mod.rs:395`, `let t = table.clone();`) and whether `kind` is stripped before `try_into`. If `<Kind>Opts` structs are **not** `deny_unknown_fields`, the extra key is already ignored and the third test passes for free. If they **are**, strip `kind` inside `instance_opts` before deserializing so the documented-harmless case stays silent. Do whichever the code requires — do not change the `Opts` structs.

- [ ] **Step 4: Implement the helper**

Add near the top of `crates/rustline-core/src/widgets/mod.rs`'s instance section:

```rust
/// Deserialize an `[instances.<name>]` options table, falling back to
/// `T::default()` on a type error — but *reporting* it first.
///
/// The silent `unwrap_or_default()` this replaces threw away the user's whole
/// instance config (format, thresholds, colour override) on one bad value and
/// still registered the name, so `resolve` found it and no "unknown widget"
/// warn fired either: the edit looked accepted and was ignored. Routed through
/// `warn_once` because a misconfiguration persists across every render tick.
pub(crate) fn instance_opts<T: Default + serde::de::DeserializeOwned>(
    name: &str,
    kind: &str,
    v: toml::Value,
) -> T {
    match v.try_into() {
        Ok(o) => o,
        Err(error) => {
            crate::diag::warn_once(&format!("instance-opts:{name}:{error}"), || {
                tracing::warn!(
                    instance = %name,
                    kind = %kind,
                    %error,
                    "invalid instance options, using defaults"
                );
            });
            T::default()
        }
    }
}
```

`toml::Value::try_into` consumes `self`, and the closure borrows `error` — bind `let error = error.to_string();` first if the borrow checker objects.

- [ ] **Step 5: Apply at every call site**

In `crates/rustline-core/src/widgets/mod.rs`, replace all twelve
`let o: <Kind>Opts = t.try_into().unwrap_or_default();`
with
`let o: <Kind>Opts = instance_opts(name, kind, t);`

In `crates/rustline-core/src/config.rs`, apply the same substitution at the twelve `instance_meta` arms (1737–1781) and at `disk_mounts` (1825), `throughput_interfaces` (1845), and `spark_referenced_in_layout` (1873, 1877). Those are in a different module — either make `instance_opts` `pub(crate)` and `use crate::widgets::instance_opts;`, or move it to `config.rs` and import it into `widgets/mod.rs`; pick whichever import direction the crate already uses between these two modules.

Each of those sites has a `name` and a `kind` in scope; if one does not, pass the name it does have and the literal kind string that arm matches.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p rustline-core instance_opts 2>&1 | tail -10`
Expected: PASS (3 tests).

- [ ] **Step 7: Full verification**

Run: `cargo build --workspace && cargo test --workspace && just lint`
Expected: all green.

- [ ] **Step 8: Strip B12 and commit**

Delete the `### B12.` section from `bughunt.md` (heading through its `- [x] execute   [ ] skip` line).

```bash
git add crates/rustline-core/src/widgets/mod.rs crates/rustline-core/src/config.rs bughunt.md
git commit -m "fix(observability): report instance-options type errors instead of silently defaulting [B12]"
```

---

## Task 4: Negative-cache a failed cache refresh [B4]

**Risk: high** — the affected path (a persistently failing endpoint) has no coverage. Write the characterization test first and commit it RED.

**Files:**
- Modify: `crates/rustline-wasm/src/cache.rs` (`CacheEntry`)
- Modify: `crates/rustline-wasm/src/perform.rs` (`perform_http_get_cached` ~49–140, `perform_exec_cached` ~218–300)
- Modify: `bughunt.md` (strip B4)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `CacheEntry` gains `#[serde(default)] pub last_attempt_at: String`. Every construction site must set it.

**Context:** `fetched_at` is written only on a 2xx. Once the TTL lapses on a dead endpoint the fresh-hit branch can never be taken again, so `fetcher.get(url)` is re-entered on *every* render — up to 5 s blocking inside the guest call, forever. Under the daemon that pins the shared render mutex for 5 s, so every other client hits its 250 ms timeout and falls back to an in-process render that repeats the same doomed fetch: one dead upstream becomes a full cold render per pane per tick. `perform_exec_cached` has the identical shape for spawn failures and timeouts.

- [ ] **Step 1: Write the failing characterization test**

Add to `crates/rustline-wasm/src/perform.rs`'s `mod tests`. Use the existing test scaffolding in that module for building a `CapabilityCtx` and a fake `Fetcher` — read it first and match it; the fake below is illustrative of the *counting* requirement, not of the exact type names.

```rust
    #[test]
    fn a_failing_refresh_is_not_retried_within_the_backoff_window() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx_with_urls(&dir, &["https://x/*"]);
        let url = "https://x/y";

        // Seed a good entry at T0.
        let ok = CountingFetcher::ok(200, "body");
        let r = perform_http_get_cached(&ctx, url, 60, "2026-07-26T12:00:00Z", &ok);
        assert!(r.ok && !r.stale);
        assert_eq!(ok.calls(), 1);

        // T0+120s: TTL lapsed, upstream now down -> one attempt, stale served.
        let down = CountingFetcher::err("connection refused");
        let r = perform_http_get_cached(&ctx, url, 60, "2026-07-26T12:02:00Z", &down);
        assert!(r.ok && r.stale, "last-good entry is served");
        assert_eq!(down.calls(), 1);

        // T0+130s: still inside the backoff window -> NO new attempt.
        let r = perform_http_get_cached(&ctx, url, 60, "2026-07-26T12:02:10Z", &down);
        assert!(r.ok && r.stale, "still served stale");
        assert_eq!(down.calls(), 1, "no second fetch inside the backoff window");
    }

    #[test]
    fn the_backoff_window_lapses_and_allows_another_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx_with_urls(&dir, &["https://x/*"]);
        let url = "https://x/y";
        let ok = CountingFetcher::ok(200, "body");
        perform_http_get_cached(&ctx, url, 60, "2026-07-26T12:00:00Z", &ok);

        let down = CountingFetcher::err("connection refused");
        perform_http_get_cached(&ctx, url, 60, "2026-07-26T12:02:00Z", &down);
        assert_eq!(down.calls(), 1);
        // one full TTL after the failed attempt
        perform_http_get_cached(&ctx, url, 60, "2026-07-26T12:03:01Z", &down);
        assert_eq!(down.calls(), 2, "backoff lapsed, retry allowed");
    }

    #[test]
    fn a_successful_refresh_clears_the_backoff() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx_with_urls(&dir, &["https://x/*"]);
        let url = "https://x/y";
        let ok = CountingFetcher::ok(200, "one");
        perform_http_get_cached(&ctx, url, 60, "2026-07-26T12:00:00Z", &ok);
        let ok2 = CountingFetcher::ok(200, "two");
        let r = perform_http_get_cached(&ctx, url, 60, "2026-07-26T12:02:00Z", &ok2);
        assert_eq!(r.body, "two");
        assert!(!r.stale);
    }

    #[test]
    fn a_failing_refresh_with_no_prior_entry_still_reports_failure() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx_with_urls(&dir, &["https://x/*"]);
        let down = CountingFetcher::err("connection refused");
        let r = perform_http_get_cached(&ctx, "https://x/y", 60, "2026-07-26T12:00:00Z", &down);
        assert!(!r.ok, "nothing to serve stale");
    }
```

Add to `crates/rustline-wasm/src/cache.rs`'s `mod tests`:

```rust
    #[test]
    fn an_entry_without_last_attempt_at_still_deserializes() {
        // Entries written by an older build must keep working (wire-type
        // discipline: additive, defaulted fields only).
        let old = r#"{"fetched_at":"2026-07-20T12:00:00-04:00","status":200,"body":"hi"}"#;
        let e: CacheEntry = serde_json::from_str(old).unwrap();
        assert_eq!(e.body, "hi");
        assert!(e.last_attempt_at.is_empty());
    }
```

Write a `CountingFetcher` (an `AtomicUsize` plus a canned `Ok`/`Err`) in the test module if the existing fakes do not already count calls. Do not modify an existing fake that other tests depend on — add a new one.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p rustline-wasm backoff 2>&1 | tail -20 && cargo test -p rustline-wasm last_attempt 2>&1 | tail -10`
Expected: FAIL — no `last_attempt_at` field; the backoff tests see a second fetch.

- [ ] **Step 3: Commit the RED test**

```bash
git add crates/rustline-wasm/src/perform.rs crates/rustline-wasm/src/cache.rs
git commit -m "test: characterize the cached-fetch retry storm before fix [B4]"
```

- [ ] **Step 4: Add the field**

In `crates/rustline-wasm/src/cache.rs`, extend `CacheEntry` and its doc comment:

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct CacheEntry {
    pub fetched_at: String,
    pub status: u16,
    pub body: String,
    /// When a refresh was last *attempted*, successful or not — RFC3339, or
    /// empty for an entry written before this field existed (or never
    /// attempted). `fetched_at` only advances on success, so without this a
    /// dead upstream is re-fetched on every single render forever: the
    /// fresh-hit branch can never be taken again once the TTL lapses.
    /// `#[serde(default)]` keeps old on-disk entries readable.
    #[serde(default)]
    pub last_attempt_at: String,
}
```

Update the existing `write_entry_roundtrips_and_enforces_cap` test's literal `CacheEntry { … }` to include the new field (that is a construction fix, not a behaviour change).

- [ ] **Step 5: Gate the refresh in `perform_http_get_cached`**

After the fresh-hit branch (perform.rs ~84) and before `match fetcher.get(url)`:

```rust
    // 2b) negative-cache backoff: a refresh was attempted within the last TTL
    //     window and we still hold a last-good entry, so serve it immediately
    //     rather than paying another blocking fetch. Without this, a dead
    //     upstream costs a full fetch timeout on EVERY render forever.
    if let Some(e) = &entry
        && let Some(since_attempt) = age_secs(now, &e.last_attempt_at)
        && is_fresh(since_attempt, ttl_secs)
    {
        return CachedHttpResult {
            ok: true,
            status: e.status,
            body: e.body.clone(),
            error: "refresh backoff; serving cached".to_string(),
            stale: true,
            age_secs: age_secs(now, &e.fetched_at).unwrap_or(0),
        };
    }
```

`age_secs` returns `None` for an empty/unparseable `last_attempt_at`, so a never-attempted entry falls through and refreshes — which is what the fourth test pins.

In the success arm, set both timestamps:

```rust
            let content = serde_json::to_string(&CacheEntry {
                fetched_at: now.to_string(),
                status,
                body: body.clone(),
                last_attempt_at: now.to_string(),
            })
```

In the failure arm's `Some(e)` branch, rewrite the entry with only `last_attempt_at` advanced, then serve stale as before:

```rust
                Some(e) => {
                    let age = age_secs(now, &e.fetched_at).unwrap_or(0);
                    // Record the failed attempt so the next render backs off
                    // instead of re-entering a blocking fetch. body/status/
                    // fetched_at are deliberately untouched: this entry is
                    // still the last-good response.
                    let content = serde_json::to_string(&CacheEntry {
                        fetched_at: e.fetched_at.clone(),
                        status: e.status,
                        body: e.body.clone(),
                        last_attempt_at: now.to_string(),
                    })
                    .unwrap_or_default();
                    if let Err(error) = write_entry(&dir, &path, &content, ctx.max_state_bytes) {
                        tracing::warn!(%error, "negative-cache write failed");
                    }
                    CachedHttpResult {
                        ok: true,
                        status: e.status,
                        body: e.body,
                        error,
                        stale: true,
                        age_secs: age,
                    }
                }
```

The `None` branch is unchanged: with no entry there is nothing to negative-cache against, and writing a body-less entry would corrupt the stale-serve path.

- [ ] **Step 6: Mirror it in `perform_exec_cached`**

`perform_exec_cached` (perform.rs ~218–300) has the same three arms. Apply the identical backoff gate, the identical success-arm timestamp, and the identical failure-arm rewrite, for spawn failures **and** timeouts. Read the function and match its result type's field names (`CachedExecResult`) rather than copying `CachedHttpResult`'s.

Add the exec-side equivalents of the first two tests from Step 1, using the existing exec-test fake runner.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p rustline-wasm 2>&1 | tail -20`
Expected: PASS, including the previously-RED tests.

- [ ] **Step 8: Full verification**

Run: `cargo build --workspace && cargo test --workspace && just lint`
Expected: all green.

- [ ] **Step 9: Strip B4 and commit**

Delete the `### B4.` section from `bughunt.md`.

```bash
git add crates/rustline-wasm/src/cache.rs crates/rustline-wasm/src/perform.rs bughunt.md
git commit -m "fix(caching): back off a failing cache refresh instead of retrying every render [B4]"
```

---

## Task 5: Evict the TTL cache instead of wedging at the quota [B13]

**Risk: high** — the wedged-cache path has no coverage. Characterization test first.

**Files:**
- Modify: `crates/rustline-wasm/src/cache.rs` (`evict_namespace`, `write_entry`)
- Modify: `bughunt.md` (strip B13)

**Interfaces:**
- Consumes: `CacheEntry.last_attempt_at` from Task 4 (do not reorder).
- Produces: `fn evict_namespace(dir: &Path, keep_bytes: u64) -> u64` (private to `cache.rs`; returns bytes freed). `write_entry` keeps its current signature `(state_dir, path, content, cap) -> Result<(), String>` **for this task** — Task 7 changes it.

**Context:** Nothing ever deletes a cache entry; expiry only overwrites the *same* key. A plugin whose key varies (a URL with a timestamp, `cmdrun` with varying args) grows the directory until `max_state_bytes` (default 50 MiB). After that **every** `write_entry` fails forever, so `perform_http_get_cached`/`perform_exec_cached` warn and return unpersisted on every render — a live 5 s fetch or a real subprocess spawn per tmux tick, permanently, while 50 MiB of dead files sit on disk.

- [ ] **Step 1: Write the failing tests**

Add to `crates/rustline-wasm/src/cache.rs`'s `mod tests`:

```rust
    fn entry_json(fetched_at: &str, body: &str) -> String {
        serde_json::to_string(&CacheEntry {
            fetched_at: fetched_at.to_string(),
            status: 200,
            body: body.to_string(),
            last_attempt_at: fetched_at.to_string(),
        })
        .unwrap()
    }

    #[test]
    fn a_full_cache_evicts_and_the_write_then_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let cap = 1_200;
        // Fill the namespace past the cap with distinct keys.
        for i in 0..10 {
            let p = cache_path(dir.path(), HTTP_NAMESPACE, &format!("https://x/{i}"));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, entry_json("2026-07-20T12:00:00-04:00", &"z".repeat(150))).unwrap();
        }
        let fresh = cache_path(dir.path(), HTTP_NAMESPACE, "https://x/new");
        // Before: over cap, so this write would be refused forever.
        assert!(crate::state::dir_size(dir.path()) > cap);
        write_entry(dir.path(), &fresh, &entry_json("2026-07-26T12:00:00-04:00", "new"), cap)
            .expect("eviction makes room");
        assert!(read_entry(&fresh).is_some(), "the new entry landed");
    }

    #[test]
    fn eviction_prefers_removing_the_oldest_entries() {
        let dir = tempfile::tempdir().unwrap();
        let ns = dir.path().join(HTTP_NAMESPACE);
        std::fs::create_dir_all(&ns).unwrap();
        let old = ns.join("old.json");
        let new = ns.join("new.json");
        std::fs::write(&old, entry_json("2020-01-01T00:00:00-00:00", &"z".repeat(400))).unwrap();
        std::fs::write(&new, entry_json("2026-07-26T12:00:00-04:00", &"z".repeat(400))).unwrap();
        let freed = evict_namespace(&ns, 500);
        assert!(freed > 0, "something was evicted");
        assert!(!old.exists(), "the oldest entry goes first");
        assert!(new.exists(), "the newest entry is retained");
    }

    #[test]
    fn a_cap_too_small_to_ever_fit_still_errors_without_looping() {
        let dir = tempfile::tempdir().unwrap();
        let p = cache_path(dir.path(), HTTP_NAMESPACE, "https://x/y");
        let big = "z".repeat(5_000);
        assert!(write_entry(dir.path(), &p, &big, 10).is_err());
    }

    #[test]
    fn eviction_never_touches_files_outside_the_namespace() {
        let dir = tempfile::tempdir().unwrap();
        let sibling = dir.path().join("counter-state.json");
        std::fs::write(&sibling, "keep me").unwrap();
        let ns = dir.path().join(HTTP_NAMESPACE);
        std::fs::create_dir_all(&ns).unwrap();
        std::fs::write(ns.join("a.json"), entry_json("2020-01-01T00:00:00-00:00", &"z".repeat(400)))
            .unwrap();
        evict_namespace(&ns, 0);
        assert!(sibling.exists(), "a sibling state file is not cache");
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p rustline-wasm evict 2>&1 | tail -20`
Expected: FAIL — `cannot find function evict_namespace`; the full-cache test fails because `write_entry` refuses.

- [ ] **Step 3: Commit the RED test**

```bash
git add crates/rustline-wasm/src/cache.rs
git commit -m "test: characterize the wedged full cache before fix [B13]"
```

- [ ] **Step 4: Implement eviction**

In `crates/rustline-wasm/src/cache.rs`:

```rust
/// Delete entries from a cache namespace directory until it fits in
/// `keep_bytes`, oldest first. Returns bytes freed.
///
/// Ordering is by the entry's own `fetched_at` when it parses, falling back to
/// mtime — a file we cannot parse is not a usable cache entry, so it sorts as
/// oldest and goes first.
///
/// Best-effort throughout (invariant N2: a cache is disposable). Only ever
/// touches regular files directly inside `dir`, which is always a
/// `HTTP_NAMESPACE`/`EXEC_NAMESPACE` subdirectory of one plugin's own state
/// dir — never the plugin's other state files, and never a subdirectory.
fn evict_namespace(dir: &Path, keep_bytes: u64) -> u64 {
    let Ok(read) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut entries: Vec<(i64, u64, PathBuf)> = read
        .filter_map(Result::ok)
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| {
            let path = e.path();
            let len = e.metadata().ok()?.len();
            let stamp = read_entry(&path)
                .and_then(|c| DateTime::parse_from_rfc3339(&c.fetched_at).ok())
                .map(|d| d.timestamp())
                .unwrap_or(i64::MIN);
            Some((stamp, len, path))
        })
        .collect();

    let mut total: u64 = entries.iter().map(|(_, len, _)| *len).sum();
    if total <= keep_bytes {
        return 0;
    }
    entries.sort_by_key(|(stamp, _, _)| *stamp); // oldest first

    let mut freed = 0;
    for (_, len, path) in entries {
        if total <= keep_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
            freed = freed.saturating_add(len);
        }
    }
    freed
}
```

Add `use std::path::PathBuf;` to the existing `use std::path::{Path, PathBuf};`.

- [ ] **Step 5: Wire it into `write_entry`**

```rust
pub fn write_entry(state_dir: &Path, path: &Path, content: &str, cap: u64) -> Result<(), String> {
    if let Err(first) = check_cap(state_dir, path, content.len() as u64, cap) {
        // The cache is at quota. Without eviction this returns Err forever:
        // every subsequent render then pays a live fetch or a real subprocess
        // spawn while a full state dir of dead entries sits on disk. Evict the
        // namespace down to a fraction of the cap and re-check once.
        let namespace = path.parent().unwrap_or(state_dir);
        evict_namespace(namespace, cap / 2);
        check_cap(state_dir, path, content.len() as u64, cap).map_err(|_| first)?;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, content).map_err(|e| e.to_string())
}
```

`evict_namespace` never deletes the file currently being written, because that file does not exist yet on a cold write, and on a rewrite the caller's own content is re-written immediately afterward.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p rustline-wasm 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 7: Full verification**

Run: `cargo build --workspace && cargo test --workspace && just lint`
Expected: all green.

- [ ] **Step 8: Strip B13 and commit**

```bash
git add crates/rustline-wasm/src/cache.rs bughunt.md
git commit -m "fix(caching): evict the TTL cache so a full state dir self-heals [B13]"
```

---

## Task 6: One collision-free atomic-write implementation [B15 + B17]

These land together on purpose: B17 is "the temp file's name is not unique" and B15 is "there is no temp file at all". Fixing them separately would leave two atomic-write shapes in the tree, which is the drift both findings describe.

**Files:**
- Modify: `crates/rustline-core/src/atomic_write.rs` (create) and `crates/rustline-core/src/lib.rs` (declare)
- Modify: `crates/rustline/src/sample_store.rs:30-43`, `crates/rustline/src/cpu.rs:199`, `crates/rustline/src/toggles.rs:53-65`
- Modify: `crates/rustline-wasm/src/paths.rs:79`, `crates/rustline-wasm/src/cache.rs:84`, `crates/rustline-wasm/src/perform.rs:362`
- Modify: `bughunt.md` (strip B15 and B17)

**Interfaces:**
- Consumes: `write_entry` from Task 5 (its body's final `fs::write` is what changes here).
- Produces: `rustline_core::atomic_write::write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()>`.

**Context:** `path.with_extension("tmp")` yields a single **process-independent** staging path (`cpu-sample.tmp`, `toggles.tmp`, …), repeated verbatim at four sites. tmux runs `render left` and `render right` as separate processes and spawns a job per client/session, so two rustline processes routinely stage the same file in the same tick: `fs::write` is create+O_TRUNC+write, so process B truncates the temp file A already filled and A renames a torn or zero-byte file into place. The parsers are total, so the damage is silent — a lost CPU sample costs a 120 ms `thread::sleep` on the next render, a lost throughput delta renders nothing, and a `{spark}` ring visibly loses samples. Separately, `write_entry` and `perform_state_write` have no temp file at all, so a concurrent reader gets a JSON parse failure, treats it as a cold cache, and does the exact live fetch the cache exists to avoid.

`rustline-core` is the shared home: both `rustline` and `rustline-wasm` depend on it, so one implementation serves all six sites.

- [ ] **Step 1: Write the failing tests**

Create `crates/rustline-core/src/atomic_write.rs` with this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        write_atomic(&p, b"hello").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello");
    }

    #[test]
    fn a_successful_write_leaves_no_temp_behind() {
        let dir = tempfile::tempdir().unwrap();
        write_atomic(&dir.path().join("f"), b"hello").unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
    }

    #[test]
    fn staging_names_are_unique_per_call() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        let a = staging_path(&p);
        let b = staging_path(&p);
        assert_ne!(a, b, "two writers must not share a staging path");
        assert!(
            a.file_name().unwrap().to_string_lossy().contains(&std::process::id().to_string()),
            "staging name carries the pid"
        );
        assert_eq!(a.parent(), p.parent(), "staged in the same dir, so rename is atomic");
    }

    #[test]
    fn overwrites_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        write_atomic(&p, b"old").unwrap();
        write_atomic(&p, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
    }

    #[test]
    fn concurrent_writers_always_leave_a_complete_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        let a = "a".repeat(4096);
        let b = "b".repeat(4096);
        std::thread::scope(|s| {
            for body in [&a, &b] {
                s.spawn(|| {
                    for _ in 0..50 {
                        let _ = write_atomic(&p, body.as_bytes());
                    }
                });
            }
        });
        let got = std::fs::read_to_string(&p).unwrap();
        assert!(got == a || got == b, "a reader never sees a torn file");
    }

    #[test]
    fn a_write_error_leaves_no_temp_behind() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope").join("f");
        assert!(write_atomic(&missing, b"x").is_err());
        assert!(!dir.path().join("nope").exists());
    }
}
```

Add `pub mod atomic_write;` to `crates/rustline-core/src/lib.rs`. `tempfile` must be in `crates/rustline-core/Cargo.toml`'s `[dev-dependencies]`; add it if absent.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p rustline-core atomic_write 2>&1 | tail -10`
Expected: FAIL — `cannot find function write_atomic` / `staging_path`.

- [ ] **Step 3: Implement**

Prepend to `crates/rustline-core/src/atomic_write.rs`:

```rust
//! The one atomic file-write primitive.
//!
//! Every persistence site in this workspace stages into a temp file and
//! renames onto the target, because rename-within-a-directory is atomic: a
//! concurrent reader sees either the old contents or the new ones, never a
//! partial write.
//!
//! The staging name must be unique **per writer**, not just per target. tmux
//! runs `render left` and `render right` as separate processes and spawns a
//! job per client/session, so two rustline processes routinely stage the same
//! file in the same tick. A shared `<name>.tmp` gives atomicity against a
//! crash and none at all against a concurrent writer: `fs::write` is
//! create+O_TRUNC+write, so B truncates the temp file A already filled and A
//! renames a torn file into place. The parsers downstream are all total, so
//! the corruption is silent — a lost CPU sample costs a 120 ms sleep on the
//! next render, a lost throughput delta renders nothing, a `{spark}` ring
//! visibly drops samples.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// A staging path next to `path`, unique to this process and this call:
/// `<path>.<pid>.<nanos>.tmp`. Same directory as the target, so the rename
/// that follows is atomic.
fn staging_path(path: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{}.{nanos}.tmp", std::process::id()));
    PathBuf::from(name)
}

/// Write `contents` to `path` atomically: stage into a per-writer temp file in
/// the same directory, then rename onto the target. Last writer wins on the
/// final path, which is the intended semantics for every caller here. The temp
/// file is removed on the error path so a failed write leaves no litter.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let tmp = staging_path(path);
    if let Err(e) = std::fs::write(&tmp, contents) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}
```

Two calls within the same nanosecond would collide; `staging_names_are_unique_per_call` may flake on a coarse clock. If it does, add a process-local `AtomicU64` counter to the name instead of relying on the clock alone — do not weaken the test.

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p rustline-core atomic_write 2>&1 | tail -10`
Expected: PASS (6 tests).

- [ ] **Step 5: Convert `sample_store::write_sample`**

In `crates/rustline/src/sample_store.rs`, replace the body of `write_sample` (lines 30-43):

```rust
pub fn write_sample(state_dir: &Path, name: &str, contents: &str) {
    let path = sample_path(state_dir, name);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = rustline_core::atomic_write::write_atomic(&path, contents.as_bytes()) {
        tracing::warn!(%error, %name, "failed to write sample-store file");
    }
}
```

Its five existing tests must still pass unchanged.

- [ ] **Step 6: Collapse `cpu::store_snapshot` and `toggles::write_toggles` onto it**

`crates/rustline/src/cpu.rs:199` (`store_snapshot`) and `crates/rustline/src/toggles.rs:53` (`write_toggles`) each hand-roll the same dance. Rewrite both to call `rustline_core::atomic_write::write_atomic`, keeping their existing `create_dir_all`, their existing `warn!` messages and their existing signatures. Do not change what they serialize.

- [ ] **Step 7: Convert the three `rustline-wasm` sites**

- `crates/rustline-wasm/src/paths.rs:79` (`ensure_wasmtime_cache_config`): replace `fs::write(&tmp, &body).ok()?; fs::rename(&tmp, &config_path).ok()?;` with `rustline_core::atomic_write::write_atomic(&config_path, body.as_bytes()).ok()?;` and drop the now-unused `tmp` binding.
- `crates/rustline-wasm/src/cache.rs`, `write_entry`'s final line: `rustline_core::atomic_write::write_atomic(path, content.as_bytes()).map_err(|e| e.to_string())`.
- `crates/rustline-wasm/src/perform.rs:362`, `perform_state_write`'s `match std::fs::write(&full, contents.as_bytes())`: change to `match rustline_core::atomic_write::write_atomic(&full, contents.as_bytes())`.

Confirm `rustline-wasm/Cargo.toml` already depends on `rustline-core` (it does — `capability.rs` imports `rustline_core::PluginConfig`).

- [ ] **Step 8: Full verification**

Run: `cargo build --workspace && cargo test --workspace && just lint`
Expected: all green.

- [ ] **Step 9: Strip B15 and B17 and commit**

Delete both the `### B15.` and `### B17.` sections from `bughunt.md`.

```bash
git add crates/rustline-core/src/atomic_write.rs crates/rustline-core/src/lib.rs \
        crates/rustline-core/Cargo.toml crates/rustline/src/sample_store.rs \
        crates/rustline/src/cpu.rs crates/rustline/src/toggles.rs \
        crates/rustline-wasm/src/paths.rs crates/rustline-wasm/src/cache.rs \
        crates/rustline-wasm/src/perform.rs Cargo.lock bughunt.md
git commit -m "fix(correctness): one collision-free atomic-write primitive for every persistence site [B15][B17]"
```

**Milestone check (5 findings done):** run `cargo test --workspace` in full and confirm green before starting Task 7. On red: bisect within this batch, revert the offender, and surface the diagnosis rather than pressing on.

---

## Task 7: Memoize the state-dir size instead of walking it per write [B19]

**Risk: high** — the quota path is covered but the memo's drift modes are not. Characterization test first.

**Files:**
- Modify: `crates/rustline-wasm/src/state.rs` (`check_cap` becomes pure)
- Modify: `crates/rustline-wasm/src/capability.rs` (the memo)
- Modify: `crates/rustline-wasm/src/cache.rs` (`write_entry` signature)
- Modify: `crates/rustline-wasm/src/perform.rs` (three call sites)
- Modify: `bughunt.md` (strip B19)

**Interfaces:**
- Consumes: `evict_namespace` from Task 5.
- Produces:
  - `state::check_cap(current_size: u64, target: &Path, new_len: u64, cap: u64) -> Result<(), String>` — **pure**, no `dir_size` walk.
  - `CapabilityCtx::state_size(&self) -> u64` — seeds from one `dir_size` walk on first call, then returns the memo.
  - `CapabilityCtx::set_state_size(&self, bytes: u64)` and `CapabilityCtx::invalidate_state_size(&self)`.
  - `cache::write_entry(state_dir: &Path, path: &Path, content: &str, current_size: u64, cap: u64) -> Result<u64, String>` — returns the new total size of `state_dir`.

**Context:** `check_cap` calls `dir_size` — a full recursive `walkdir` + `metadata()` over the plugin's whole state dir — on **every** write. All three trigger paths are per-render, and the walk includes both cache namespaces, so the syscall count per render grows monotonically with the number of cached keys: a plugin with a few thousand cached URLs turns one `rl_state_write` into a few thousand `stat()` calls per tmux tick.

**Invariant N3 is load-bearing here.** Memoize the *measurement*, never the *decision*: the check stays strictly before the write, and a memo that could be wrong must be invalidated (a re-walk is correct; a stale-high memo wedges writes that should succeed).

- [ ] **Step 1: Write the failing tests**

Add to `crates/rustline-wasm/src/state.rs`'s `mod tests` — rewrite the existing `check_cap_refuses_over_and_allows_replace_within` to the new signature (this is a signature migration of an existing test, which is allowed; its assertions must not weaken):

```rust
    #[test]
    fn check_cap_refuses_over_and_allows_replace_within() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("f");
        fs::write(&target, b"aaaa").unwrap(); // 4 bytes existing
        let current = dir_size(d.path());
        // replacing 4 bytes with 6 -> projected 6 <= cap 8 OK
        assert!(check_cap(current, &target, 6, 8).is_ok());
        // a brand-new 10-byte file on top of the existing 4 -> 14 > cap 8
        let other = d.path().join("g");
        assert!(check_cap(current, &other, 10, 8).is_err());
    }

    #[test]
    fn check_cap_is_pure_and_does_not_walk() {
        // A directory that does not exist has no size to walk; the caller
        // supplies the current size, so the check still decides correctly.
        assert!(check_cap(0, Path::new("/no/such/f"), 5, 10).is_ok());
        assert!(check_cap(9, Path::new("/no/such/f"), 5, 10).is_err());
    }
```

Add to `crates/rustline-wasm/src/capability.rs`'s `mod tests`:

```rust
    #[test]
    fn state_size_is_seeded_once_then_memoized() {
        let d = tempfile::tempdir().unwrap();
        let ctx = CapabilityCtx::from_config("p", &PluginConfig::default(), d.path().to_path_buf());
        std::fs::create_dir_all(ctx.state_dir()).unwrap();
        std::fs::write(ctx.state_dir().join("a"), b"12345").unwrap();
        assert_eq!(ctx.state_size(), 5);
        // A file added behind the memo's back is NOT observed: that is the
        // point — the walk happens once, not once per write.
        std::fs::write(ctx.state_dir().join("b"), b"12345").unwrap();
        assert_eq!(ctx.state_size(), 5, "memoized, not re-walked");
        ctx.invalidate_state_size();
        assert_eq!(ctx.state_size(), 10, "invalidation forces a fresh walk");
    }

    #[test]
    fn set_state_size_updates_the_memo() {
        let d = tempfile::tempdir().unwrap();
        let ctx = CapabilityCtx::from_config("p", &PluginConfig::default(), d.path().to_path_buf());
        ctx.set_state_size(42);
        assert_eq!(ctx.state_size(), 42);
    }
```

Add to `crates/rustline-wasm/src/cache.rs`'s `mod tests`:

```rust
    #[test]
    fn write_entry_reports_the_new_total_size() {
        let dir = tempfile::tempdir().unwrap();
        let p = cache_path(dir.path(), HTTP_NAMESPACE, "https://x/y");
        let content = entry_json("2026-07-26T12:00:00-04:00", "hi");
        let total = write_entry(dir.path(), &p, &content, 0, 10_000).unwrap();
        assert_eq!(total, content.len() as u64);
        assert_eq!(total, crate::state::dir_size(dir.path()));
    }

    #[test]
    fn write_entry_after_eviction_reports_a_re_measured_total() {
        let dir = tempfile::tempdir().unwrap();
        let cap = 1_200;
        for i in 0..10 {
            let p = cache_path(dir.path(), HTTP_NAMESPACE, &format!("https://x/{i}"));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, entry_json("2026-07-20T12:00:00-04:00", &"z".repeat(150))).unwrap();
        }
        let fresh = cache_path(dir.path(), HTTP_NAMESPACE, "https://x/new");
        let before = crate::state::dir_size(dir.path());
        let total = write_entry(
            dir.path(),
            &fresh,
            &entry_json("2026-07-26T12:00:00-04:00", "new"),
            before,
            cap,
        )
        .expect("eviction makes room");
        assert_eq!(total, crate::state::dir_size(dir.path()), "reported total is truthful");
        assert!(total <= cap);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p rustline-wasm state_size 2>&1 | tail -10; cargo test -p rustline-wasm check_cap 2>&1 | tail -10`
Expected: FAIL — wrong arity on `check_cap`; no `state_size`.

- [ ] **Step 3: Commit the RED test**

```bash
git add crates/rustline-wasm/src/state.rs crates/rustline-wasm/src/capability.rs \
        crates/rustline-wasm/src/cache.rs
git commit -m "test: characterize the per-write state-dir walk before fix [B19]"
```

- [ ] **Step 4: Make `check_cap` pure**

In `crates/rustline-wasm/src/state.rs`:

```rust
/// Ok iff writing `new_len` bytes to `target` (possibly replacing an existing
/// file) keeps a state dir of `current_size` bytes within `cap`.
///
/// **Pure by design.** This used to call `dir_size` itself, so every write —
/// three per render on a state-backed plugin — paid a full recursive walk of a
/// directory that includes both unbounded cache namespaces. The caller now
/// supplies the size from a memo (see `CapabilityCtx::state_size`); the
/// decision itself is unchanged, and stays strictly before the write
/// (invariant N3).
pub fn check_cap(current_size: u64, target: &Path, new_len: u64, cap: u64) -> Result<(), String> {
    let replaced = std::fs::metadata(target).map(|m| m.len()).unwrap_or(0);
    let projected = current_size.saturating_sub(replaced).saturating_add(new_len);
    if projected > cap {
        Err("state quota exceeded".into())
    } else {
        Ok(())
    }
}
```

The single `metadata(target)` stat stays — it is O(1) and the accounting needs it.

- [ ] **Step 5: Add the memo to `CapabilityCtx`**

In `crates/rustline-wasm/src/capability.rs`, add the field and methods. Use `AtomicU64` with a sentinel rather than `Cell`: `CapabilityCtx` lives in Extism `UserData`, which requires `Send`, and an atomic keeps that unconditional.

```rust
use std::sync::atomic::{AtomicU64, Ordering};

/// Sentinel for "not yet measured" in [`CapabilityCtx::state_size`].
const SIZE_UNSEEDED: u64 = u64::MAX;
```

Add to the struct:

```rust
    /// Memoized total byte size of this plugin's state dir. Seeded by one
    /// `dir_size` walk per process/daemon lifetime and then adjusted after
    /// each successful write, instead of re-walking on every write.
    state_size: AtomicU64,
```

Initialize it as `state_size: AtomicU64::new(SIZE_UNSEEDED)` in `from_config`. Add:

```rust
    /// This plugin's state-dir size in bytes, measured once and then memoized.
    pub fn state_size(&self) -> u64 {
        let cached = self.state_size.load(Ordering::Relaxed);
        if cached != SIZE_UNSEEDED {
            return cached;
        }
        let measured = state::dir_size(&self.state_dir());
        self.state_size.store(measured, Ordering::Relaxed);
        measured
    }

    /// Record a known-good size (after a successful write reports its new
    /// total).
    pub fn set_state_size(&self, bytes: u64) {
        self.state_size.store(bytes, Ordering::Relaxed);
    }

    /// Drop the memo so the next `state_size` re-walks. Call whenever the
    /// directory may have changed in a way this ctx did not account for — a
    /// failed write, an eviction it did not perform. A stale-high memo would
    /// wedge writes that should succeed, so when in doubt, invalidate.
    pub fn invalidate_state_size(&self) {
        self.state_size.store(SIZE_UNSEEDED, Ordering::Relaxed);
    }
```

`use crate::state;` is currently `#[cfg(test)]`-gated at the top of `capability.rs` — remove the gate, since `state_size` needs it in all builds.

- [ ] **Step 6: Change `write_entry` to take and return a size**

In `crates/rustline-wasm/src/cache.rs`:

```rust
/// Quota-checked, atomic write of `content` to `path`. `current_size` is the
/// caller's memoized state-dir size; the returned value is the new total,
/// which the caller should store back into that memo.
///
/// On a quota failure the namespace is evicted and the check retried once, so
/// a full cache self-heals rather than refusing every write forever. After an
/// eviction the total is re-measured rather than estimated, because eviction
/// frees an amount the caller cannot predict.
pub fn write_entry(
    state_dir: &Path,
    path: &Path,
    content: &str,
    current_size: u64,
    cap: u64,
) -> Result<u64, String> {
    let new_len = content.len() as u64;
    let replaced = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut base = current_size;
    if let Err(first) = check_cap(base, path, new_len, cap) {
        let namespace = path.parent().unwrap_or(state_dir);
        evict_namespace(namespace, cap / 2);
        base = crate::state::dir_size(state_dir);
        check_cap(base, path, new_len, cap).map_err(|_| first)?;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    rustline_core::atomic_write::write_atomic(path, content.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(base.saturating_sub(replaced).saturating_add(new_len))
}
```

`replaced` is read before the eviction because eviction may delete the very entry being replaced; re-measuring `base` afterwards makes the arithmetic consistent either way. Update the existing `write_entry_roundtrips_and_enforces_cap` and Task 5's eviction tests to the new arity.

- [ ] **Step 7: Update the three call sites**

In `crates/rustline-wasm/src/perform.rs`:

- `perform_http_get_cached`'s success arm and its new negative-cache write (Task 4):

```rust
            match write_entry(&dir, &path, &content, ctx.state_size(), ctx.max_state_bytes) {
                Ok(total) => ctx.set_state_size(total),
                Err(error) => {
                    ctx.invalidate_state_size();
                    tracing::warn!(%error, %url, "http cache write failed; returning body unpersisted");
                }
            }
```

- `perform_exec_cached`'s equivalent sites — same shape, keeping that site's existing message and fields.
- `perform_state_write`: replace `check_cap(&dir, &full, contents.len() as u64, ctx.max_state_bytes)` with `check_cap(ctx.state_size(), &full, contents.len() as u64, ctx.max_state_bytes)`, and after a successful `write_atomic` update the memo:

```rust
    let replaced = std::fs::metadata(&full).map(|m| m.len()).unwrap_or(0);
    // ... existing create_dir_all ...
    match rustline_core::atomic_write::write_atomic(&full, contents.as_bytes()) {
        Ok(()) => {
            ctx.set_state_size(
                ctx.state_size()
                    .saturating_sub(replaced)
                    .saturating_add(contents.len() as u64),
            );
            WriteResult { ok: true, error: String::new() }
        }
        Err(e) => {
            ctx.invalidate_state_size();
            WriteResult { ok: false, error: e.to_string() }
        }
    }
```

Read `replaced` **before** the write.

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p rustline-wasm 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 9: Full verification**

Run: `cargo build --workspace && cargo test --workspace && just lint`
Expected: all green.

- [ ] **Step 10: Strip B19 and commit**

```bash
git add crates/rustline-wasm/src/state.rs crates/rustline-wasm/src/capability.rs \
        crates/rustline-wasm/src/cache.rs crates/rustline-wasm/src/perform.rs bughunt.md
git commit -m "fix(caching): memoize the state-dir size instead of walking it per write [B19]"
```

---

## Task 8: Bound `rl_log` message size and rate [B18]

**Files:**
- Modify: `crates/rustline-wasm/src/perform.rs` (`perform_log` ~448)
- Modify: `crates/rustline-wasm/src/capability.rs` (call counter)
- Modify: `crates/rustline-wasm/src/host.rs:100-104` (the `host_fn!` wrapper)
- Modify: `bughunt.md` (strip B18)

**Interfaces:**
- Consumes: `CapabilityCtx` from Task 7 (add the counter field alongside the size memo).
- Produces: `perform_log(ctx: &CapabilityCtx, level: &str, msg: &str)` — signature change from `(plugin: &str, level: &str, msg: &str)`.

**Context:** `perform_log` is the one intentionally capability-free host function and applies no length cap, no rate cap, and no per-render budget — it forwards the guest's `msg` verbatim. A guest has 16 MB of memory and a 10 s / 500 M-fuel budget, so one render can issue thousands of calls or one ~16 MB message. The user's disk fills overnight and the log they would consult to find out why is the thing that filled it. The SDK's `log` wrapper is `pub` and unrestricted, so this is reachable by any plugin, not just a hostile one.

**`rl_log` stays capability-free (invariant N1).** It is *bounded*, not *gated* — there is still no allowlist and still no denial.

- [ ] **Step 1: Write the failing tests**

Add to `crates/rustline-wasm/src/perform.rs`'s existing `log_tests` module (read its `RecordingSubscriber` harness and match how the existing tests capture output):

```rust
    #[test]
    fn an_oversized_message_is_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = CapabilityCtx::from_config("p", &PluginConfig::default(), dir.path().into());
        let captured = with_recording_subscriber(|| {
            perform_log(&ctx, "info", &"x".repeat(10_000));
        });
        assert!(captured.len() < 10_000, "message was truncated");
        assert!(captured.contains("…(truncated)"), "truncation is marked");
    }

    #[test]
    fn truncation_never_splits_a_multibyte_char() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = CapabilityCtx::from_config("p", &PluginConfig::default(), dir.path().into());
        // 3-byte chars straddling the byte cap: a naive &msg[..CAP] panics.
        let msg = "€".repeat(MAX_GUEST_LOG_BYTES);
        let captured = with_recording_subscriber(|| {
            perform_log(&ctx, "info", &msg); // must not panic
        });
        assert!(captured.contains("…(truncated)"));
    }

    #[test]
    fn the_call_rate_is_capped_and_reported_once() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = CapabilityCtx::from_config("p", &PluginConfig::default(), dir.path().into());
        let captured = with_recording_subscriber(|| {
            for i in 0..(MAX_GUEST_LOG_CALLS + 50) {
                perform_log(&ctx, "info", &format!("line {i}"));
            }
        });
        assert!(!captured.contains(&format!("line {}", MAX_GUEST_LOG_CALLS + 10)));
        assert_eq!(
            captured.matches("guest log rate limit reached").count(),
            1,
            "the limit is reported exactly once"
        );
    }

    #[test]
    fn the_budget_is_per_plugin_instance() {
        let dir = tempfile::tempdir().unwrap();
        let a = CapabilityCtx::from_config("a", &PluginConfig::default(), dir.path().into());
        let b = CapabilityCtx::from_config("b", &PluginConfig::default(), dir.path().into());
        let captured = with_recording_subscriber(|| {
            for _ in 0..MAX_GUEST_LOG_CALLS {
                perform_log(&a, "info", "from a");
            }
            perform_log(&b, "info", "from b"); // b has its own budget (N4)
        });
        assert!(captured.contains("from b"));
    }
```

`with_recording_subscriber` is illustrative — use whatever the existing `log_tests` harness provides for capturing.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p rustline-wasm log_tests 2>&1 | tail -20`
Expected: FAIL — no `MAX_GUEST_LOG_BYTES`; `perform_log` has the wrong arity.

- [ ] **Step 3: Implement**

In `crates/rustline-wasm/src/capability.rs`, add alongside the size memo:

```rust
    /// How many `rl_log` calls this instance has made this process. `rl_log`
    /// is capability-free by design, so a rate cap — not a gate — is what
    /// keeps a buggy loop or a hostile plugin from filling the user's disk
    /// with the very log they would consult to find out why.
    log_calls: AtomicU32,
```

Initialize `log_calls: AtomicU32::new(0)`, `use std::sync::atomic::AtomicU32;`, and add:

```rust
    /// Claim one guest-log slot. Returns how many calls have now been made
    /// (1-based), so the caller can emit exactly one limit notice.
    pub fn claim_log_call(&self) -> u32 {
        self.log_calls.fetch_add(1, Ordering::Relaxed).saturating_add(1)
    }
```

In `crates/rustline-wasm/src/perform.rs`:

```rust
/// Longest guest log message forwarded to `tracing`, in bytes.
pub(crate) const MAX_GUEST_LOG_BYTES: usize = 2 * 1024;
/// Most `rl_log` calls forwarded per plugin instance per process.
pub(crate) const MAX_GUEST_LOG_CALLS: u32 = 256;

/// Truncate `msg` to at most `MAX_GUEST_LOG_BYTES`, never splitting a
/// character. The message is guest-supplied UTF-8, so a byte-index slice would
/// panic on a multi-byte char straddling the cap.
fn truncate_guest_msg(msg: &str) -> std::borrow::Cow<'_, str> {
    if msg.len() <= MAX_GUEST_LOG_BYTES {
        return std::borrow::Cow::Borrowed(msg);
    }
    let mut end = MAX_GUEST_LOG_BYTES;
    while end > 0 && !msg.is_char_boundary(end) {
        end -= 1;
    }
    std::borrow::Cow::Owned(format!("{}…(truncated)", &msg[..end]))
}
```

Rewrite `perform_log`, keeping and extending its existing doc comment (the N1 explanation stays; add why bounding is not gating):

```rust
pub fn perform_log(ctx: &CapabilityCtx, level: &str, msg: &str) {
    let calls = ctx.claim_log_call();
    if calls > MAX_GUEST_LOG_CALLS {
        if calls == MAX_GUEST_LOG_CALLS + 1 {
            tracing::warn!(
                target: "rustline_wasm::guest",
                plugin = %ctx.name,
                limit = MAX_GUEST_LOG_CALLS,
                "guest log rate limit reached; further messages dropped this process"
            );
        }
        return;
    }
    let plugin = &ctx.name;
    let msg = truncate_guest_msg(msg);
    match level {
        "error" => tracing::error!(target: "rustline_wasm::guest", %plugin, "{msg}"),
        "warn" => tracing::warn!(target: "rustline_wasm::guest", %plugin, "{msg}"),
        "info" => tracing::info!(target: "rustline_wasm::guest", %plugin, "{msg}"),
        "debug" => tracing::debug!(target: "rustline_wasm::guest", %plugin, "{msg}"),
        "trace" => tracing::trace!(target: "rustline_wasm::guest", %plugin, "{msg}"),
        _ => {
            tracing::info!(target: "rustline_wasm::guest", %plugin, orig_level = %level, "{msg}")
        }
    }
}
```

- [ ] **Step 4: Update the host wrapper**

`crates/rustline-wasm/src/host.rs:103` currently reads `perform_log(&ctx.name, &level, &msg);`. Change to `perform_log(&ctx, &level, &msg);`. The `host_fn!` at line 100 already binds `user_data: CapabilityCtx`, so `ctx` is in scope — confirm whether it is a guard/`MutexGuard` and deref accordingly (`&*ctx`).

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rustline-wasm log_tests 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: Full verification**

Run: `cargo build --workspace && cargo test --workspace && just lint`
Expected: all green.

- [ ] **Step 7: Strip B18 and commit**

```bash
git add crates/rustline-wasm/src/perform.rs crates/rustline-wasm/src/capability.rs \
        crates/rustline-wasm/src/host.rs bughunt.md
git commit -m "fix(observability): cap guest log message size and call rate [B18]"
```

---

## Task 9: Surface capability denials in the log [B16]

**Files:**
- Modify: `crates/rustline-wasm/src/denials.rs` (`record` returns `bool`; `observe` logs)
- Modify: `bughunt.md` (strip B16)

**Interfaces:**
- Consumes: nothing.
- Produces: `record(path, plugin, kind, target) -> bool` (private; `true` when newly recorded).

**Context:** Every deny site calls `observe_denial`, but the only sink is `FileDenialObserver` appending to `<data_root>/denials.jsonl`. No `tracing` event is emitted anywhere on a denial, so whether the user learns of one depends entirely on the *guest* choosing to call `rl_log`. A typo'd `allowed_urls` glob produces a blank widget, an empty log, and nothing pointing at `rustline plugin denials`. Two entirely different causes — a denial, versus the plugin simply returning no segments — are indistinguishable.

**Deliberate scope note:** `bughunt.md` suggests combining this with redaction so a `target` carrying a credential is not logged verbatim. That is finding **B33**, which the user did not select. This task logs `target` verbatim, matching what `denials.jsonl` already stores. Do not expand scope.

Do **not** route this through Task 2's `warn_once`: a denial is a runtime event, its dedup already lives in `record`, and the JSONL is its durable record.

- [ ] **Step 1: Write the failing tests**

Add to `crates/rustline-wasm/src/denials.rs`'s `mod tests`:

```rust
    #[test]
    fn record_reports_whether_the_triple_was_new() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("denials.jsonl");
        assert!(record(&path, "weather", DenialKind::Url, "https://a/"));
        assert!(!record(&path, "weather", DenialKind::Url, "https://a/"));
        assert!(record(&path, "weather", DenialKind::Url, "https://b/"));
    }

    #[test]
    fn a_failed_write_does_not_claim_the_triple_was_recorded() {
        let blocker = tempfile::NamedTempFile::new().unwrap();
        let path = blocker.path().join("sub").join("denials.jsonl");
        assert!(!record(&path, "weather", DenialKind::Url, "https://a/"));
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p rustline-wasm denials 2>&1 | tail -10`
Expected: FAIL — `record` returns `()`.

- [ ] **Step 3: Implement**

In `crates/rustline-wasm/src/denials.rs`, change `record`'s signature and every `return` in it:

```rust
/// Append `(plugin, kind, target)` to `path` unless it's already present.
/// Returns `true` iff the triple was newly recorded — the caller uses that to
/// log the denial exactly once, reusing the dedup that already keeps the JSONL
/// quiet.
///
/// Best-effort: any failure is `warn!`-logged, swallowed, and reported as
/// not-recorded (so a later successful write still logs it once).
fn record(path: &Path, plugin: &str, kind: DenialKind, target: &str) -> bool {
```

- the `already_recorded` early return becomes `return false;`
- the `let Ok(line) = … else` arm becomes `return false;`
- the `create_dir_all` failure arm becomes `return false;`
- the trailing write:

```rust
    match write_result {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "failed to write denial record");
            false
        }
    }
```

Then in `observe`:

```rust
impl DenialObserver for FileDenialObserver {
    fn observe(&self, plugin: &str, kind: DenialKind, target: &str) {
        // Emit *alongside* the record, not instead of it. Without a log line a
        // denial is invisible unless the guest happens to call `rl_log`, and a
        // blank widget looks identical to a plugin that simply returned
        // nothing. `record`'s dedup keeps this to one line per distinct
        // triple, so a persistently-denied plugin does not spam the log.
        if record(&self.path, plugin, kind, target) {
            tracing::warn!(
                %plugin,
                ?kind,
                %target,
                "capability denied; see `rustline plugin denials`"
            );
        }
    }
}
```

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p rustline-wasm denials 2>&1 | tail -10`
Expected: PASS. The five existing denials tests must still pass unchanged.

- [ ] **Step 5: Full verification**

Run: `cargo build --workspace && cargo test --workspace && just lint`
Expected: all green.

- [ ] **Step 6: Strip B16 and commit**

```bash
git add crates/rustline-wasm/src/denials.rs bughunt.md
git commit -m "fix(observability): log capability denials instead of only recording them [B16]"
```

---

## Task 10: Make the daemon fallback observable [B27]

**Files:**
- Modify: `crates/rustline/src/daemon_client.rs:49-77`
- Modify: `crates/rustline/src/daemon.rs:85` (`reload_if_changed`)
- Modify: `bughunt.md` (strip B27)

**Interfaces:**
- Consumes: `rustline_core::diag::warn_once` from Task 2.
- Produces: no new public API.

**Context:** Six consecutive `.ok()?` / `return None` points and no `tracing` call anywhere in the module. The fallback is correct and documented (N2 extended to the daemon path); its *silence* is not. A wedged daemon — or a dead one leaving a stale socket file, which `sock.exists()` happily accepts — makes every tick pay a 250 ms connect timeout before falling back. The bar becomes visibly sluggish, `daemon status` says not running, and nothing ties the two together. The daemon side is equally quiet: `reload_if_changed` rebuilds the whole config/theme/plugin registry on a config edit without logging it.

- [ ] **Step 1: Write the failing tests**

Add to `crates/rustline/src/daemon_client.rs`'s `mod tests`:

```rust
    #[test]
    fn a_missing_socket_falls_back_without_a_warning() {
        // The daemon simply isn't installed. That is not an error and must not
        // put a line in the log on every render tick.
        assert!(
            try_render_at(
                &PathBuf::from("/no/such/rustline.sock"),
                RegionKind::Right,
                RenderArgsWire::default(),
            )
            .is_none()
        );
    }

    #[test]
    fn a_stale_socket_file_falls_back_and_is_actionable() {
        // A plain file where a bound socket should be: `exists()` accepts it,
        // `connect` then fails. This is the stale-socket case a user must be
        // told about — it costs a connect attempt on every single render.
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("d.sock");
        std::fs::write(&sock, b"not a socket").unwrap();
        assert!(try_render_at(&sock, RegionKind::Right, RenderArgsWire::default()).is_none());
    }

    #[test]
    fn an_unexpected_response_variant_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("d.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _req: DaemonRequest = daemon_proto::read_frame(&mut stream).unwrap();
            daemon_proto::write_frame(&mut stream, &DaemonResponse::Pong).unwrap();
        });
        let out = try_render_at(&sock, RegionKind::Right, RenderArgsWire::default());
        handle.join().unwrap();
        assert!(out.is_none());
    }
```

These pin the *behaviour* (still falls back, never panics). Asserting on emitted tracing output from a binary crate without a subscriber harness is not worth building here — the observable contract is that every path still returns `None`.

- [ ] **Step 2: Run them to verify they fail or pass**

Run: `cargo test -p rustline daemon_client 2>&1 | tail -10`
Expected: `a_stale_socket_file_falls_back_and_is_actionable` and `an_unexpected_response_variant_falls_back` are new; they should PASS immediately (the fallback already works). That is fine — they are regression cover for the refactor in Step 3, which is where the risk is.

- [ ] **Step 3: Add the diagnostics**

Rewrite `try_render_at`'s body, keeping every early return returning `None`:

```rust
    // One `stat` before attempting to connect: near-zero overhead when the
    // daemon isn't running (the common case today), and avoids paying a
    // connect-timeout for a socket that was never bound. No log line here —
    // "not installed" is not a problem.
    if !sock.exists() {
        return None;
    }
    let mut stream = match UnixStream::connect(sock) {
        Ok(s) => s,
        Err(error) => {
            // The socket file exists but nothing is listening: a dead daemon
            // left it behind. Every render now pays a connect attempt before
            // falling back, which shows up as a sluggish bar with nothing in
            // the log to explain it. Actionable, and persistent — so it is
            // warned once per config generation rather than once per tick.
            rustline_core::diag::warn_once(
                &format!("daemon-stale-socket:{}", sock.display()),
                || {
                    tracing::warn!(
                        %error,
                        socket = %sock.display(),
                        "daemon socket exists but refuses connections; remove it or restart the daemon"
                    );
                },
            );
            return None;
        }
    };
    if let Err(error) = stream.set_read_timeout(Some(SOCKET_TIMEOUT)) {
        tracing::debug!(%error, "daemon set_read_timeout failed");
        return None;
    }
    if let Err(error) = stream.set_write_timeout(Some(SOCKET_TIMEOUT)) {
        tracing::debug!(%error, "daemon set_write_timeout failed");
        return None;
    }
    if let Err(error) = daemon_proto::write_frame(
        &mut stream,
        &DaemonRequest::RenderV2 {
            protocol: daemon_proto::DAEMON_PROTOCOL,
            region,
            args,
        },
    ) {
        tracing::debug!(%error, "daemon request write failed");
        return None;
    }
    let response: DaemonResponse = match daemon_proto::read_frame(&mut stream) {
        Ok(r) => r,
        Err(error) => {
            tracing::debug!(%error, "daemon response read failed or timed out");
            return None;
        }
    };
    match response {
        DaemonResponse::Markup(markup) => Some(markup),
        other => {
            tracing::debug!(?other, "unexpected daemon response; falling back");
            None
        }
    }
```

`DaemonResponse` needs `Debug` for `?other`; it derives `Serialize`/`Deserialize` already — add `Debug` to its derive list in `daemon_proto.rs` if absent. If the `Pong | ShuttingDown` arm was exhaustive, `other` keeps it exhaustive.

Update the module doc to say the module now reports *why* it fell back at `debug`, with the stale-socket case at `warn`.

- [ ] **Step 4: Log the daemon's warm-state rebuild**

In `crates/rustline/src/daemon.rs`, at the end of `DaemonState::reload_if_changed`'s rebuild branch (~line 85):

```rust
        // The daemon is long-lived, so this is one line per config edit, not
        // per render — and it is the only confirmation a user gets that a warm
        // daemon actually picked up their change.
        tracing::info!(config = %config_path.display(), "config changed; rebuilt warm state");
```

Match the actual variable name holding the config path in that scope.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rustline daemon 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: Full verification**

Run: `cargo build --workspace && cargo test --workspace && just lint`
Expected: all green.

- [ ] **Step 7: Strip B27 and commit**

```bash
git add crates/rustline/src/daemon_client.rs crates/rustline/src/daemon.rs \
        crates/rustline/src/daemon_proto.rs bughunt.md
git commit -m "fix(observability): report why a daemon render fell back in-process [B27]"
```

**Milestone check (10 findings done):** run `cargo test --workspace` in full and confirm green before starting Task 11.

---

## Task 11: Split the filesystem write capability out of `allowed_paths` [D3]

**Risk: high** — this narrows a shared funnel. Characterization tests for every legitimate producer come first.

This is the largest task in the plan. It is one task because the config key, the manifest key, the gate, the consent prompt, and the CLI must land together — a half-applied split is a security hole in either direction.

**Files:**
- Modify: `crates/rustline-core/src/config.rs` (`PluginConfig` ~1373-1421)
- Modify: `crates/rustline-wasm/src/state.rs` (`resolve_for_allowlist`)
- Modify: `crates/rustline-wasm/src/capability.rs` (`allowed_write_paths`, `resolve_symlinks`)
- Modify: `crates/rustline-wasm/src/perform.rs` (`perform_file_read` ~374, `perform_file_write` ~413)
- Modify: `crates/rustline-wasm/src/manifest.rs` (`requested_write_paths`)
- Modify: `crates/rustline/src/plugin_cmd.rs` (`Kind`, `manifest_report`, `write_requests`, `write_grants`, `run`, `list`)
- Modify: `crates/rustline/src/cli.rs` (`PluginCmd::Path` help; new `PluginCmd::WritePath`)
- Modify: `bughunt.md` (strip the `perform_file_write` decision-needed marker)

**User decision (verbatim, from `bughunt.md`):** *"treat existing key entries as read-only as the migration, and I agree with your proposed solution. Additionally, require the path provided to _not_ be a symlink unless a 'resolve symlinks' config value is enabled."*

**Interfaces:**
- Consumes: `CapabilityCtx` as extended by Tasks 7 and 8.
- Produces:
  - `PluginConfig.allowed_write_paths: Vec<String>` (`#[serde(default)]`, empty).
  - `PluginConfig.resolve_symlinks: bool` (`#[serde(default)]`, `false`).
  - `PluginManifest.requested_write_paths: Vec<String>` (`#[serde(default)]`).
  - `CapabilityCtx.allowed_write_paths: AllowSet`, `CapabilityCtx.resolve_symlinks: bool`.
  - `state::resolve_for_allowlist(path: &str, resolve_symlinks: bool) -> Result<String, String>`.
  - `plugin_cmd::Kind::WritePath` with key `"allowed_write_paths"`.

**Context:** `perform_file_read` and `perform_file_write` gate on the *same* `ctx.allowed_paths` — there is no separate write allowlist. `plugin approve` copies a plugin-supplied manifest's `requested_paths` verbatim into `allowed_paths` and prints a danger warning ONLY for `requested_commands`; the CLI help calls it merely "a plugin's filesystem-path allowlist". A plugin shipping a manifest that requests `/home/u/.bashrc`, advertised as "reads your aliases to show a badge", gets arbitrary file overwrite the moment the user types `y`. Separately, `normalize_abs` matches the allowlist against an *uncanonicalized* path, so a grant is a grant over *names*: any symlink under a granted prefix silently redirects the effect to its target.

- [ ] **Step 1: Write the failing producer tests**

Per the project's spec/test discipline, this narrows a shared funnel, so every legitimate producer gets a test. Add to `crates/rustline-wasm/src/perform.rs`'s `mod tests`, using the module's existing ctx-building helper (extend it to take write paths and a `resolve_symlinks` flag, or add a sibling helper — do not modify the existing helper's callers):

```rust
    #[test]
    fn a_read_grant_still_reads() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("notes.txt");
        std::fs::write(&f, "hi").unwrap();
        let ctx = test_ctx_paths(&dir, &[&format!("{}/*", dir.path().display())], &[], false);
        let r = perform_file_read(&ctx, f.to_str().unwrap());
        assert!(r.ok && r.exists && r.contents == "hi");
    }

    #[test]
    fn a_read_grant_alone_does_not_permit_a_write() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("notes.txt");
        std::fs::write(&f, "original").unwrap();
        let ctx = test_ctx_paths(&dir, &[&format!("{}/*", dir.path().display())], &[], false);
        let w = perform_file_write(&ctx, f.to_str().unwrap(), "pwned");
        assert!(!w.ok, "allowed_paths is read-only");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "original");
    }

    #[test]
    fn a_write_grant_permits_a_write() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("notes.txt");
        let ctx = test_ctx_paths(&dir, &[], &[&format!("{}/*", dir.path().display())], false);
        let w = perform_file_write(&ctx, f.to_str().unwrap(), "written");
        assert!(w.ok, "{}", w.error);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "written");
    }

    #[test]
    fn a_write_grant_alone_does_not_permit_a_read() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("notes.txt");
        std::fs::write(&f, "secret").unwrap();
        let ctx = test_ctx_paths(&dir, &[], &[&format!("{}/*", dir.path().display())], false);
        let r = perform_file_read(&ctx, f.to_str().unwrap());
        assert!(!r.ok, "a write grant is not a read grant");
    }

    #[test]
    fn a_symlink_under_a_grant_is_denied_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("bashrc");
        std::fs::write(&target, "original").unwrap();
        let link = dir.path().join("notes.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let glob = format!("{}/*", dir.path().display());
        let ctx = test_ctx_paths(&dir, &[&glob], &[&glob], false);

        assert!(!perform_file_read(&ctx, link.to_str().unwrap()).ok);
        assert!(!perform_file_write(&ctx, link.to_str().unwrap(), "pwned").ok);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
    }

    #[test]
    fn resolve_symlinks_matches_the_allowlist_against_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("bashrc");
        std::fs::write(&target, "original").unwrap();
        let link = dir.path().join("notes.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let glob = format!("{}/*", dir.path().display());
        let ctx = test_ctx_paths(&dir, &[&glob], &[&glob], true);

        // Resolution happens BEFORE matching, so the escape is caught by the
        // allowlist rather than slipping past it.
        assert!(!perform_file_write(&ctx, link.to_str().unwrap(), "pwned").ok);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
    }

    #[test]
    fn resolve_symlinks_allows_a_link_whose_target_is_inside_the_grant() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.txt");
        std::fs::write(&real, "hi").unwrap();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let glob = format!("{}/*", dir.path().display());
        let ctx = test_ctx_paths(&dir, &[&glob], &[], true);
        let r = perform_file_read(&ctx, link.to_str().unwrap());
        assert!(r.ok && r.contents == "hi", "{}", r.error);
    }

    #[test]
    fn a_not_yet_existing_write_target_under_a_grant_is_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("new.txt");
        let glob = format!("{}/*", dir.path().display());
        let ctx = test_ctx_paths(&dir, &[], &[&glob], true);
        let w = perform_file_write(&ctx, f.to_str().unwrap(), "fresh");
        assert!(w.ok, "{}", w.error);
    }
```

`tempfile::tempdir()` may itself sit under a symlinked `/tmp` on macOS. If the default-policy tests fail for that reason, canonicalize the tempdir root in `test_ctx_paths` before building the globs — do not weaken the assertions.

Add to `crates/rustline-wasm/src/state.rs`'s `mod tests`:

```rust
    #[test]
    fn resolve_for_allowlist_keeps_normalize_abs_rules() {
        assert!(resolve_for_allowlist("relative/x", false).is_err());
        assert!(resolve_for_allowlist("/ok/../escape", false).is_err());
    }

    #[test]
    fn resolve_for_allowlist_rejects_a_symlink_component_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let under = link.join("f");
        assert!(
            resolve_for_allowlist(under.to_str().unwrap(), false).is_err(),
            "a symlinked parent component escapes a name-based grant"
        );
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p rustline-wasm perform::tests 2>&1 | tail -30`
Expected: FAIL — no `test_ctx_paths`/`resolve_for_allowlist`; the write-denial tests fail because `allowed_paths` still grants write.

- [ ] **Step 3: Commit the RED tests**

```bash
git add crates/rustline-wasm/src/perform.rs crates/rustline-wasm/src/state.rs
git commit -m "test: characterize the read/write path grant and symlink escape before fix [D3]"
```

- [ ] **Step 4: Add the config keys**

In `crates/rustline-core/src/config.rs`'s `PluginConfig` (~1377), after `allowed_paths`:

```rust
    /// Filesystem paths this plugin may **write**, as globs. Deliberately
    /// separate from `allowed_paths`, which is read-only: the two used to be
    /// one list, so approving a manifest that looked like "reads your aliases
    /// to show a badge" also handed the plugin arbitrary overwrite of those
    /// files. Empty by default — deny by default (invariant N1).
    ///
    /// Migration: an existing `allowed_paths` entry grants read only. A plugin
    /// that was writing through it now fails closed, loudly (a denial record
    /// plus a log line), rather than silently.
    #[serde(default)]
    pub allowed_write_paths: Vec<String>,
    /// Resolve symlinks before matching a path against the allowlists.
    ///
    /// Off by default, in which case a path whose components include a symlink
    /// is denied outright: a grant is otherwise a grant over *names*, and any
    /// symlink planted under a granted prefix — by an extracted archive, a
    /// synced directory, another tool — silently redirects the effect to its
    /// target. Turning this on resolves the path first and matches the
    /// allowlist against the *resolved* location, which is safe but means a
    /// grant follows links out of the directory the user thought they granted.
    #[serde(default)]
    pub resolve_symlinks: bool,
```

Update the capability-fields doc comment at 1373-1374 to list both new keys, and add them to the `Default` impl (~1418): `allowed_write_paths: Vec::new(), resolve_symlinks: false,`.

Add a config test alongside the existing `[plugins.weather]` ones asserting both defaults and that a config setting them round-trips.

- [ ] **Step 5: Implement `resolve_for_allowlist`**

In `crates/rustline-wasm/src/state.rs`, keep `normalize_abs` as-is (it stays the cheap first gate and its tests stay) and add:

```rust
/// Resolve a plugin-supplied absolute path into the string the allowlists are
/// matched against.
///
/// `normalize_abs` alone matches against the path as *written*, so a grant is a
/// grant over names rather than over a filesystem subtree: with
/// `allowed_paths = ["/home/u/notes/*"]`, a symlink at `~/notes/todo` pointing
/// at `~/.bashrc` passes the gate and the effect lands on `.bashrc`. The `..`
/// rejection is complete for literal traversal and does nothing about this.
///
/// Default (`resolve_symlinks = false`): any existing symlink component is
/// rejected outright. With `resolve_symlinks = true`: the path is canonicalized
/// and the *canonical* string is returned, so the allowlist is matched against
/// where the write will actually land. Either way, resolution happens strictly
/// before the allowlist check.
pub fn resolve_for_allowlist(path: &str, resolve_symlinks: bool) -> Result<String, String> {
    let norm = normalize_abs(path)?;
    let p = Path::new(&norm);
    if !resolve_symlinks {
        // Walk every ancestor; a component that does not exist cannot be a
        // symlink, and a write target legitimately may not exist yet.
        for ancestor in p.ancestors() {
            match std::fs::symlink_metadata(ancestor) {
                Ok(m) if m.file_type().is_symlink() => {
                    return Err(
                        "symlink not allowed; set resolve_symlinks = true for this plugin".into(),
                    );
                }
                _ => {}
            }
        }
        return Ok(norm);
    }
    // Resolve. A not-yet-existing target resolves through its parent so a
    // first write to a granted directory still works.
    let resolved = match std::fs::canonicalize(p) {
        Ok(c) => c,
        Err(_) => {
            let parent = p.parent().ok_or("cannot resolve path")?;
            let name = p.file_name().ok_or("cannot resolve path")?;
            std::fs::canonicalize(parent)
                .map_err(|e| format!("cannot resolve path: {e}"))?
                .join(name)
        }
    };
    resolved
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| "resolved path is not valid UTF-8".to_string())
}
```

- [ ] **Step 6: Extend `CapabilityCtx`**

In `crates/rustline-wasm/src/capability.rs`, add `pub allowed_write_paths: AllowSet,` and `pub resolve_symlinks: bool,` to the struct, and to `from_config`:

```rust
            allowed_write_paths: AllowSet::compile(&pc.allowed_write_paths),
            resolve_symlinks: pc.resolve_symlinks,
```

Update the struct's doc comment to name the read/write split.

- [ ] **Step 7: Re-gate the two host functions**

In `crates/rustline-wasm/src/perform.rs`, rewrite both gates. `perform_file_read`:

```rust
pub fn perform_file_read(ctx: &CapabilityCtx, path: &str) -> ReadResult {
    let norm = match resolve_for_allowlist(path, ctx.resolve_symlinks) {
        Ok(p) => p,
        Err(error) => {
            return ReadResult { ok: false, error, ..Default::default() };
        }
    };
    if !ctx.allowed_paths.allows(&norm) {
        ctx.observe_denial(DenialKind::Path, &norm);
        return ReadResult {
            ok: false,
            error: format!("path not allowed: {norm}"),
            ..Default::default()
        };
    }
    // ... existing read body, reading `&norm`, unchanged ...
```

`perform_file_write` identically, except it matches `ctx.allowed_write_paths` and its doc comment states that `allowed_paths` does not authorize writes and why. Update the `use crate::state::{…}` import: `resolve_for_allowlist` replaces `normalize_abs` at these two sites (`normalize_abs` stays exported and tested).

Order is load-bearing and must not be reordered: **resolve → match → act**.

- [ ] **Step 8: Add the manifest key**

In `crates/rustline-wasm/src/manifest.rs`, after `requested_paths` (~47):

```rust
    /// Paths the plugin asks to be able to **write**. Approved into
    /// `allowed_write_paths`, which is separate from the read-only
    /// `allowed_paths` — see `PluginConfig` for why.
    #[serde(default)]
    pub requested_write_paths: Vec<String>,
```

Update the struct doc (~29-32) and add a parse test. Every literal `PluginManifest { … }` in tests needs the new field.

- [ ] **Step 9: Extend the consent surface**

In `crates/rustline/src/plugin_cmd.rs`:

```rust
enum Kind {
    Url,
    Path,
    WritePath,
    Command,
}

impl Kind {
    fn key(&self) -> &'static str {
        match self {
            Kind::Url => "allowed_urls",
            Kind::Path => "allowed_paths",
            Kind::WritePath => "allowed_write_paths",
            Kind::Command => "allowed_commands",
        }
    }
}
```

In `manifest_report`, add the write-paths line and its danger banner, and label the read line:

```rust
    write_requests(&mut out, "allowed_urls", &m.requested_urls);
    write_requests(&mut out, "allowed_paths (read-only)", &m.requested_paths);
    write_requests(&mut out, "allowed_write_paths", &m.requested_write_paths);
    write_requests(&mut out, "allowed_commands", &m.requested_commands);
    if !m.requested_write_paths.is_empty() {
        out.push_str(
            "\n  ! allowed_write_paths lets this plugin overwrite these files with\n\
             \x20 ! any content. Approve only paths you understand.\n",
        );
    }
    if !m.requested_commands.is_empty() {
        // ... existing command banner, unchanged ...
    }
```

In `write_grants`, add the fourth append:

```rust
    if !m.requested_write_paths.is_empty() {
        append_unique(
            allowlist_array(table, plugin, Kind::WritePath.key()),
            &m.requested_write_paths,
        );
    }
```

In `run`, add `PluginCmd::WritePath(pc) => pattern_cmd(pc, Kind::WritePath, config_path),`.

In `list`, show `allowed_write_paths` alongside the other three lists — read the function and match its existing per-list formatting exactly.

- [ ] **Step 10: Extend the CLI**

In `crates/rustline/src/cli.rs`'s `PluginCmd` (~219), change the `Path` doc comment to "Manage a plugin's filesystem-path **read** allowlist." and add after it:

```rust
    /// Manage a plugin's filesystem-path write allowlist. Separate from
    /// `path`: a read grant never authorizes a write.
    #[command(subcommand)]
    WritePath(PatternCmd),
```

`clap` derives the subcommand name `write-path` from the variant. If shell completions are generated from this enum, regenerate them if the repo commits generated completions (check `clap_complete` usage first).

- [ ] **Step 11: Check the bundled example plugins**

Grep `plugins/` for `rl_file_write` / the SDK's write wrapper. Any example that writes needs `requested_write_paths` in its manifest. `filewatch` is the read-only case — confirm it still works and needs no change. If any example needs a manifest edit, update it and note in the commit message that `just check-lock` still passes (those plugins carry their own `Cargo.lock`).

- [ ] **Step 12: Run the tests to verify they pass**

Run: `cargo test --workspace 2>&1 | tail -30`
Expected: PASS, including all eight producer tests.

- [ ] **Step 13: Full verification**

Run: `cargo build --workspace && cargo test --workspace && just lint && just check-lock`
Expected: all green.

- [ ] **Step 14: Strip the finding and commit**

Delete from `bughunt.md` the `> **decision-needed (policy decision):** \`perform_file_write\` …` block at lines 61-63.

```bash
git add crates/rustline-core/src/config.rs crates/rustline-wasm/src/state.rs \
        crates/rustline-wasm/src/capability.rs crates/rustline-wasm/src/perform.rs \
        crates/rustline-wasm/src/manifest.rs crates/rustline/src/plugin_cmd.rs \
        crates/rustline/src/cli.rs plugins bughunt.md
git commit -m "fix(security): grant plugin file writes separately from reads and close the symlink escape [D3]"
```

---

## Task 12: Documentation sync

**Files:**
- Modify: `CLAUDE.md`
- Modify: `README.md`

**Interfaces:**
- Consumes: every earlier task's user-visible surface.
- Produces: nothing code-facing.

**Context:** This repo's rule is that a change adding user-visible surface syncs the reference lists in **both** `CLAUDE.md` and `README.md`. This batch adds two config keys, one CLI subcommand, one manifest key, and changes the log file's name and rotation policy.

- [ ] **Step 1: Update `CLAUDE.md`**

Grep for each of these and update every occurrence:

1. The `[plugins.*]` reference: add `allowed_write_paths` (globs, empty = deny, write only) and `resolve_symlinks` (bool, default false). State explicitly that `allowed_paths` is **read-only** and that this changed in this batch.
2. The N3 / `allowed_paths` prose: note that the symlink gate closes the name-vs-subtree gap, and what `resolve_symlinks = true` trades away.
3. The `[log]` reference: rotation is daily with 7 generations retained; the file is **`rustline.<YYYY-MM-DD>.log`** (prefix, then date, then suffix — verify against a real run, not against this line); `[log].file` is decomposed into directory + prefix + suffix, and the date is UTC. Remove any mention of the 5 MiB cap or a single `.1` generation.
4. The `rustline plugin …` subcommand list: add `write-path`.
5. The plugin manifest reference: add `requested_write_paths`.
6. Any statement that `rl_log` is unbounded, or that the TTL cache never evicts, or that the denial recorder is the only sink for denials — all three are now false.
7. The line noting there is no quota or rotation on denials.jsonl is still true (B31 was not selected) — leave it.

- [ ] **Step 2: Update `README.md`**

Sync the same surfaces at the README's level of detail: the plugin capability table (read vs write paths, `resolve_symlinks`), the `plugin write-path` subcommand, and the log file's new name and rotation policy.

- [ ] **Step 3: Verify no stale claims remain**

Run: `grep -n "MAX_LOG_BYTES\|5 MiB\|rustline\.log\.1\|rustline\.log\b\|open_log\|log_path\|allowed_paths" CLAUDE.md README.md`
Expected: no surviving claim that `allowed_paths` grants write, and no surviving 5 MiB / `.1` rotation claim.

Two known stragglers the `[log]` config reference alone will not catch — check both explicitly:
- **`CLAUDE.md:1384`** names `open_log` and `log_path` in the module map. Both functions were deleted in Task 1; the replacements are `log_dir`, `log_file_prefix`, `log_file_suffix`, `current_log_path`, and `build_appender`.
- Anywhere claiming the log lives at a single stable `rustline.log`.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md README.md
git commit -m "docs: sync config, CLI, and logging reference for the code-health batch"
```

---

## Final verification

- [ ] **Run the full suite once more**

```bash
cargo build --workspace && cargo test --workspace && just lint && just check-lock
```

- [ ] **Confirm `bughunt.md` contains no fixed items**

```bash
grep -n "^### B4\.\|^### B12\.\|^### B13\.\|^### B15\.\|^### B16\.\|^### B17\.\|^### B18\.\|^### B19\.\|^### B27\." bughunt.md
```

Expected: no output. Also confirm the three `decision-needed` blocks that carried `User Note:` lines are gone, and that every *unchecked* finding is still present and untouched.

- [ ] **Report the two side-effect resolutions**

Two unchecked findings are expected to be resolved as a side effect of this batch. Do **not** strip them — report them for the user to confirm:

- **B20** (rotation race clobbering the single retained generation) — Task 1 removes the stat-then-rename sequence entirely, so the race has no mechanism left.
- **B36** (symlink escape past `normalize_abs`) — Task 11's `resolve_for_allowlist` is exactly that fix, with tests.

No summary commit — the per-finding commits are the audit trail.

## Self-review notes

- **Spec coverage:** D1→T1, D2→T2, B12→T3, B4→T4, B13→T5, B15+B17→T6, B19→T7, B18→T8, B16→T9, B27→T10, D3→T11, docs→T12. All twelve selected items have a task.
- **Ordering dependencies honoured:** B12 after D2 (needs `warn_once`); B13 after B4 (both touch `CacheEntry`); B19 after B13 (wires into eviction); T6 after B13 (rewrites `write_entry`'s tail); T8 and T11 after T7 (all extend `CapabilityCtx`); T10 after D2.
- **Signature churn is sequenced, not simultaneous:** `write_entry` changes in T5 (body only), T6 (tail), then T7 (signature). `perform_log` changes only in T8. `CapabilityCtx` gains fields in T7, T8, T11 in that order.
- **Deliberate scope limits, stated in-task:** B16 logs `target` unredacted (B33 not selected); B31's denial-file memoization is not in scope even though T9 touches `record`.
