# rustline — macOS daemon install (launchd LaunchAgent)

**Date:** 2026-07-24
**Status:** design
**Scope:** add macOS support to `rustline daemon install` / `rustline daemon
uninstall`, so the optional render daemon (W48) can auto-launch at user login
on macOS, mirroring the existing systemd **user**-unit install on Linux.

## Problem

`rustline daemon install`/`uninstall` (`crates/rustline/src/daemon_service.rs`)
today generate a **systemd user unit** and wire it up via `systemctl --user`.
The module isn't `#[cfg]`-gated — it compiles everywhere but its production
`RealSystemctl` just shells out to `systemctl`, which does not exist on macOS.
So a macOS user has no first-class way to make the daemon survive logins; they
must hand-roll a launchd job.

macOS's native per-user, run-at-login service mechanism is a **launchd
LaunchAgent**: a `.plist` in `~/Library/LaunchAgents/` that launchd loads at
each login. This is the direct analogue of a systemd user unit.

## Goals

- `rustline daemon install` on macOS writes a launchd LaunchAgent plist and
  loads it (auto-start now + at every login), mirroring the systemd flow.
- `rustline daemon uninstall` on macOS unloads and removes the plist.
- The **same commands, flags, and UX** as Linux — compile-time per-OS backend
  dispatch, `--write-only` and `--binary` reused verbatim.
- Same robustness contract as the systemd path: the plist **file** is the
  durable part; a `launchctl` failure or absence is never fatal (best-effort,
  never panics — consistent with N2 "never break the bar").
- Fully unit-tested on both the Linux CI box and macOS via the existing
  injected-fake + temp-path seam.

## Non-goals

- No system-wide `LaunchDaemon` (root, `/Library/LaunchDaemons`) — the daemon
  is a per-user service, exactly like the systemd **user** unit.
- No wizard/`init` integration (the systemd path has none either).
- No change to the daemon itself, the socket protocol, or the render path.
- No new runtime dependency (`libc` is already a dep; used for `getuid`).

## Decisions (from brainstorming)

1. **Label / plist filename:** `rustline-daemon` →
   `~/Library/LaunchAgents/rustline-daemon.plist`, launchd label
   `rustline-daemon`. Chosen for symmetry with the systemd
   `rustline-daemon.service` unit name.
2. **Restart policy:** `KeepAlive = { SuccessfulExit = false }` — relaunch only
   on a non-zero exit. A clean `rustline daemon stop` (exit 0) stays stopped,
   exactly mirroring systemd `Restart=on-failure`.
3. **Command shape:** one command, compile-time per-OS backend. `main.rs` calls
   a platform-agnostic `daemon_service::install(&binary, write_only)` /
   `uninstall()`; path + real service-manager selection moves inside the module
   behind `#[cfg(target_os)]`.

## Design

### Module layout — `daemon_service.rs`

Keep it a single file, following this repo's established
`#[cfg(target_os)]`-in-one-file platform-read pattern (`cpu.rs`, `memory.rs`,
`battery.rs`). Three parts:

**Shared entry points (platform-agnostic public API):**

```rust
pub fn install(binary: &str, write_only: bool);
pub fn uninstall();
```

Each is `#[cfg(target_os)]`-gated internally: on Linux it resolves
`service_unit_path()` + `RealSystemctl` and calls the systemd inner fn; on
macOS it resolves `plist_path()` + `RealLaunchctl` and calls the launchd inner
fn; on any other OS it prints "unsupported on this platform" and returns
(never panics). `main.rs` no longer names the path or the manager.

**Linux backend (existing, lightly refactored):** `Systemctl` trait,
`RealSystemctl`, `service_unit(binary)`, `service_unit_path()`, and the inner
`install_systemd(unit_path, binary, write_only, &dyn Systemctl)` /
`uninstall_systemd(unit_path, &dyn Systemctl)` (today's `install`/`uninstall`,
renamed so the public names are free for the dispatcher). Behavior byte-for-
byte unchanged; existing tests keep passing (renamed call sites only).

**macOS backend (new):**

- `pub fn plist_contents(binary: &str) -> String` — the LaunchAgent plist
  (pure). `Label = rustline-daemon`, `ProgramArguments = [<binary>, daemon,
  run]`, `RunAtLoad = true`, `KeepAlive = { SuccessfulExit = false }`. XML with
  the standard `<?xml …><!DOCTYPE …>` header. `binary` is XML-escaped
  defensively (path could contain `&`/`<`), though `current_exe()` paths
  realistically won't.
- `pub fn plist_path() -> PathBuf` — `$HOME/Library/LaunchAgents/
  rustline-daemon.plist` (macOS LaunchAgents live under `~/Library`, not XDG).
- `Launchctl` trait — the macOS analogue of `Systemctl`:
  ```rust
  pub trait Launchctl {
      fn available(&self) -> bool;                 // `launchctl` on PATH
      fn bootstrap(&self, domain: &str, plist: &Path) -> bool; // load+RunAtLoad
      fn bootout(&self, domain: &str, plist: &Path) -> bool;   // unload
  }
  ```
  `domain` is `gui/<uid>` (modern per-user GUI domain).
- `RealLaunchctl` — production impl. `bootstrap` →
  `launchctl bootstrap gui/$UID <plist>`, `bootout` →
  `launchctl bootout gui/$UID <plist>`, both stdio-silenced, best-effort
  (`false` on spawn failure / non-zero exit, never panics). `available()`
  reuses the shared `command_on_path("launchctl")`.
- `fn gui_domain() -> String` → `format!("gui/{}", unsafe { libc::getuid() })`
  (the sole macOS `unsafe`, isolated; `getuid` cannot fail).
- Inner `install_launchd(plist_path, binary, write_only, &dyn Launchctl)` /
  `uninstall_launchd(plist_path, &dyn Launchctl)`, structurally identical to
  the systemd inner fns:
  - **install:** create `~/Library/LaunchAgents`, write the plist (report
    "Wrote"/"Replaced"), then — unless `write_only` or `launchctl` is absent —
    `bootout` any stale instance first (ignored if not loaded) and `bootstrap`
    the plist; print a manual `launchctl bootstrap gui/$UID <plist>` hint on
    failure or in `--write-only`.
  - **uninstall:** best-effort `bootout` (ignoring "not loaded"), remove the
    plist if present (report, or "nothing to remove"), safe to re-run.

### `main.rs` wiring

```rust
DaemonCmd::Install(a) => {
    let binary = resolve_binary(a.binary.as_deref());
    daemon_service::install(&binary, a.write_only);
}
DaemonCmd::Uninstall => daemon_service::uninstall(),
```

No platform names in `main.rs`. `DaemonInstallArgs { write_only, binary }`
unchanged; both flags apply identically on macOS.

### Testing

Pure fns gated `#[cfg(any(target_os = "<os>", test))]` so both backends'
builders compile and unit-test on Linux CI *and* macOS. A `FakeLaunchctl`
(recording, mirroring `FakeSystemctl`) drives `install_launchd`/
`uninstall_launchd` against a `tempfile::tempdir()` plist path — no real
`launchctl`, no `~/Library` writes.

Tests:
- `plist_contents_has_required_keys` — Label, `ProgramArguments` with `<binary>
  daemon run`, `RunAtLoad`, `KeepAlive`/`SuccessfulExit`.
- `plist_path_ends_with_launchagents_suffix`.
- `install_launchd_writes_plist_with_contents`.
- `install_launchd_bootstraps_when_available_and_not_write_only` (asserts the
  recorded bootout-then-bootstrap call order).
- `install_launchd_write_only_writes_but_never_loads`.
- `install_launchd_degrades_when_launchctl_unavailable`.
- `uninstall_launchd_removes_plist_and_boots_out` (+ idempotent second run).
- Existing systemd tests unchanged (call sites renamed to `_systemd`).

### Docs

- `CLAUDE.md`: update the `daemon_service.rs` module description to "Linux →
  systemd user unit; macOS → launchd LaunchAgent," and the `daemon install`/
  `uninstall` CLI bullets.
- `README.md`: note macOS support alongside the systemd mention.

## Robustness / invariants

- Best-effort throughout: the plist file is the durable artifact; every
  `launchctl` call degrades to a printed manual hint, never a panic or a
  non-zero-exit process abort on a `launchctl` failure (matches the systemd
  contract and N2).
- `uninstall` is idempotent and safe to run when nothing is installed.
- No change to the render/daemon runtime, socket protocol, or any invariant.

## Alternatives considered

- **Legacy `launchctl load -w`/`unload -w`** instead of `bootstrap`/`bootout`:
  works on older macOS but deprecated on modern releases; the target is a
  recent macOS. Rejected in favor of the modern domain-target subcommands.
- **Unified single `ServiceManager` trait** across both OSes: the systemd
  (daemon-reload + enable_now) and launchd (bootstrap + bootout) verb sets
  don't line up cleanly; a forced common interface is more awkward than two
  small parallel traits. Rejected.
- **System-wide `LaunchDaemon`:** wrong scope — needs root and doesn't match
  the per-user systemd unit. Rejected (non-goal).
