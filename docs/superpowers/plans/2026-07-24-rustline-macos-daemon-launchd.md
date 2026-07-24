# Plan — macOS daemon install (launchd LaunchAgent)

Spec: `docs/superpowers/specs/2026-07-24-rustline-macos-daemon-launchd-design.md`

Implemented inline with TDD (single cohesive module), then an independent
code-reviewer pass. Tasks are sequential (compile dependencies).

## Task 1 — Refactor `daemon_service.rs` public API to platform-agnostic dispatch

- Rename existing `install`/`uninstall` → `install_systemd`/`uninstall_systemd`
  (bodies unchanged), and their `#[cfg]` where needed.
- Add platform-agnostic `pub fn install(binary: &str, write_only: bool)` and
  `pub fn uninstall()` that `#[cfg(target_os)]`-dispatch: Linux resolves
  `service_unit_path()` + `RealSystemctl`; other/unsupported prints a notice.
- Gate the systemd-specific items (`Systemctl`, `RealSystemctl`, `service_unit`,
  `service_unit_path`, `install_systemd`, `uninstall_systemd`, `UNIT_NAME`,
  `print_manual_start_hint`, `run_systemctl`) `#[cfg(any(target_os = "linux",
  test))]` so they still compile+test on macOS/CI but not in a macOS production
  build.
- Update existing systemd tests to call the renamed `_systemd` inner fns.

**Verify:** `cargo test -p rustline daemon_service` green; `cargo build`.

## Task 2 — Add the macOS launchd backend

- `plist_contents(binary) -> String` (pure, `#[cfg(any(target_os = "macos",
  test))]`): XML LaunchAgent with `Label=rustline-daemon`,
  `ProgramArguments=[binary, "daemon", "run"]`, `RunAtLoad=true`,
  `KeepAlive={SuccessfulExit=false}`. XML-escape `binary`.
- `plist_path() -> PathBuf` (`#[cfg(target_os = "macos")]`):
  `$HOME/Library/LaunchAgents/rustline-daemon.plist`.
- `Launchctl` trait + `RealLaunchctl` (`bootstrap`/`bootout`/`available`),
  `gui_domain()` via `libc::getuid()` (isolated `unsafe`), and the manual-hint
  printer, all `#[cfg]`-gated to `macos`(+`test` for the trait/fake-driven bits).
- `install_launchd(plist_path, binary, write_only, &dyn Launchctl)` /
  `uninstall_launchd(plist_path, &dyn Launchctl)` mirroring the systemd inner
  fns (create dir, write plist, bootout-stale-then-bootstrap, best-effort).
- Wire the macOS arm of the dispatcher `install`/`uninstall` from Task 1 to
  these.
- `FakeLaunchctl` (recording) + the 7 launchd tests from the spec.

**Verify:** `cargo test -p rustline daemon_service` green (both backends);
`cargo clippy --bin rustline --all-targets`; `cargo fmt --all --check`.

## Task 3 — `main.rs` wiring + docs

- `main.rs`: `DaemonCmd::Install` → `daemon_service::install(&binary,
  a.write_only)`; `DaemonCmd::Uninstall` → `daemon_service::uninstall()`.
- `CLAUDE.md`: update `daemon_service.rs` description + `daemon install`/
  `uninstall` CLI bullets (Linux systemd / macOS launchd).
- `README.md`: mention macOS launchd support.

**Verify:** full `cargo test`, `cargo clippy --bin rustline --all-targets -- -D
warnings` (scoped to changed files being clean), `cargo fmt --all --check`.
Manual smoke on macOS: `rustline daemon install --write-only` writes a valid
plist; `plutil -lint` it; `uninstall` removes it.

## Final — Independent review + finish branch

- Dispatch a code-reviewer subagent over the branch diff.
- Address Critical/Important findings; record Minor.
- `superpowers:finishing-a-development-branch`.
