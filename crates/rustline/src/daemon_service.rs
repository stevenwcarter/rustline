//! `rustline daemon install`/`uninstall`: generate a systemd **user** unit for
//! the render daemon (`daemon.rs`) and wire it up via `systemctl --user`, so
//! the daemon survives logins/reboots without the user hand-writing the unit
//! file the README used to document as a copy-paste example.
//!
//! [`service_unit`] (the unit text) and [`service_unit_path`] (where it goes)
//! are pure. `systemctl` itself sits behind the [`Systemctl`] trait — the same
//! seam `click.rs`'s `ClickExecutor` uses for `sh -c`/`xdg-open` — so
//! [`install`]/[`uninstall`] are unit-tested with a recording fake instead of
//! spawning a real `systemctl`. [`RealSystemctl`] is the only production
//! implementation: every method is best-effort (a spawn failure or non-zero
//! exit is just `false`). Neither [`install`] nor [`uninstall`] ever treats a
//! `systemctl` failure as fatal — the unit **file** is the durable part of
//! either operation, and both degrade gracefully when `systemctl` isn't on
//! `PATH` at all (e.g. a non-systemd system, or a minimal container).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The unit name `install`/`uninstall` manage, and the argument `systemctl
/// --user enable|disable --now` is given.
const UNIT_NAME: &str = "rustline-daemon.service";

/// The systemd unit text for a daemon started via the resolved absolute
/// `binary` path (same resolution `rustline init` uses for its tmux block —
/// see `crate::resolve_binary`).
pub fn service_unit(binary: &str) -> String {
    format!(
        "# Managed by `rustline daemon install`.\n\
         [Unit]\n\
         Description=rustline render daemon\n\
         \n\
         [Service]\n\
         ExecStart={binary} daemon run\n\
         Restart=on-failure\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

/// Where the unit file goes: `$XDG_CONFIG_HOME/systemd/user/rustline-daemon.service`,
/// falling back to `$HOME/.config/systemd/user/rustline-daemon.service` when
/// `XDG_CONFIG_HOME` is unset (the systemd user-unit search path).
pub fn service_unit_path() -> PathBuf {
    let base = env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env::var("HOME").unwrap_or_default()).join(".config"));
    base.join("systemd").join("user").join(UNIT_NAME)
}

/// Executes the `systemctl --user` operations [`install`]/[`uninstall`] need.
/// Behind a trait so they're unit-tested with a recording fake, never
/// spawning a real `systemctl`.
pub trait Systemctl {
    /// Whether `systemctl` is usable (found on `$PATH`).
    fn available(&self) -> bool;
    /// `systemctl --user daemon-reload`.
    fn daemon_reload(&self) -> bool;
    /// `systemctl --user enable --now <unit>`.
    fn enable_now(&self, unit: &str) -> bool;
    /// `systemctl --user disable --now <unit>`.
    fn disable_now(&self, unit: &str) -> bool;
}

/// The production [`Systemctl`]: real `systemctl --user` invocations. Every
/// method is best-effort — a spawn failure or non-zero exit is just `false`,
/// never a panic.
pub struct RealSystemctl;

impl Systemctl for RealSystemctl {
    fn available(&self) -> bool {
        command_on_path("systemctl")
    }

    fn daemon_reload(&self) -> bool {
        run_systemctl(&["--user", "daemon-reload"])
    }

    fn enable_now(&self, unit: &str) -> bool {
        run_systemctl(&["--user", "enable", "--now", unit])
    }

    fn disable_now(&self, unit: &str) -> bool {
        run_systemctl(&["--user", "disable", "--now", unit])
    }
}

/// Whether `name` resolves to an executable file in some `$PATH` entry (the
/// same probe `doctor.rs`'s `binary_on_path` uses for `rustline` itself).
fn command_on_path(name: &str) -> bool {
    env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}

/// Run `systemctl <args>` with stdio silenced, returning `true` iff it spawned
/// and exited successfully.
fn run_systemctl(args: &[&str]) -> bool {
    Command::new("systemctl")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Print the manual follow-up for a unit that was written but not enabled.
fn print_manual_start_hint() {
    eprintln!("Start it with:  systemctl --user enable --now {UNIT_NAME}");
}

/// `rustline daemon install`: write the systemd unit to `unit_path` (creating
/// its parent dirs; overwriting is fine — it's a rustline-managed file, and a
/// pre-existing one is reported as replaced), then either leave it for the
/// user to enable manually (`write_only`, or `systemctl` not being available)
/// or reload the user daemon-reload cache and enable+start it right away.
/// Never exits the process on a `systemctl` failure — the unit file was
/// already written, which is the durable part of this operation; a failed
/// `enable_now` just falls back to printing the manual command.
pub fn install(unit_path: &Path, binary: &str, write_only: bool, sc: &dyn Systemctl) {
    let replacing = unit_path.exists();
    if let Some(parent) = unit_path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        eprintln!("failed to create {}: {e}", parent.display());
        std::process::exit(1);
    }
    if let Err(e) = fs::write(unit_path, service_unit(binary)) {
        eprintln!("failed to write {}: {e}", unit_path.display());
        std::process::exit(1);
    }
    if replacing {
        eprintln!("Replaced existing unit at {}", unit_path.display());
    } else {
        eprintln!("Wrote {}", unit_path.display());
    }

    if write_only {
        print_manual_start_hint();
        return;
    }
    if !sc.available() {
        eprintln!("systemctl not found on PATH; the unit was written but left inactive.");
        print_manual_start_hint();
        return;
    }

    eprintln!("Running `systemctl --user daemon-reload`...");
    sc.daemon_reload();
    eprintln!("Running `systemctl --user enable --now {UNIT_NAME}`...");
    if sc.enable_now(UNIT_NAME) {
        eprintln!("Started {UNIT_NAME}.");
    } else {
        eprintln!("failed to enable/start {UNIT_NAME}.");
        print_manual_start_hint();
    }
}

/// `rustline daemon uninstall`: best-effort disable+stop the unit (ignoring a
/// "not loaded" failure — there's nothing to undo), remove `unit_path` if
/// present, then reload the user daemon-reload cache. A missing `unit_path`
/// (already removed, or never installed) is reported, not an error — a second
/// `uninstall` is always safe to run.
pub fn uninstall(unit_path: &Path, sc: &dyn Systemctl) {
    let available = sc.available();
    if available {
        sc.disable_now(UNIT_NAME);
    }
    if unit_path.exists() {
        match fs::remove_file(unit_path) {
            Ok(()) => eprintln!("Removed {}", unit_path.display()),
            Err(e) => eprintln!("failed to remove {}: {e}", unit_path.display()),
        }
    } else {
        eprintln!("nothing to remove: {} not found", unit_path.display());
    }
    if available {
        sc.daemon_reload();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A [`Systemctl`] that records every call it receives instead of
    /// spawning anything, so `install`/`uninstall` are verified without a
    /// real `systemctl` on the test machine.
    struct FakeSystemctl {
        available: bool,
        calls: RefCell<Vec<String>>,
    }

    impl FakeSystemctl {
        fn new(available: bool) -> Self {
            FakeSystemctl {
                available,
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl Systemctl for FakeSystemctl {
        fn available(&self) -> bool {
            self.available
        }
        fn daemon_reload(&self) -> bool {
            self.calls.borrow_mut().push("daemon_reload".to_string());
            true
        }
        fn enable_now(&self, unit: &str) -> bool {
            self.calls.borrow_mut().push(format!("enable_now:{unit}"));
            true
        }
        fn disable_now(&self, unit: &str) -> bool {
            self.calls.borrow_mut().push(format!("disable_now:{unit}"));
            true
        }
    }

    #[test]
    fn service_unit_has_required_directives() {
        let unit = service_unit("/abs/path/rustline");
        assert!(unit.contains("ExecStart=/abs/path/rustline daemon run"));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn service_unit_path_ends_with_expected_suffix() {
        let path = service_unit_path();
        assert!(
            path.ends_with("systemd/user/rustline-daemon.service"),
            "{path:?}"
        );
    }

    #[test]
    fn install_writes_unit_file_with_service_unit_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(UNIT_NAME);
        let sc = FakeSystemctl::new(true);

        install(&path, "/abs/rustline", true, &sc);

        let written = fs::read_to_string(&path).unwrap();
        assert_eq!(written, service_unit("/abs/rustline"));
    }

    #[test]
    fn install_reloads_and_enables_when_available_and_not_write_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(UNIT_NAME);
        let sc = FakeSystemctl::new(true);

        install(&path, "/abs/rustline", false, &sc);

        assert_eq!(
            *sc.calls.borrow(),
            vec![
                "daemon_reload".to_string(),
                format!("enable_now:{UNIT_NAME}"),
            ]
        );
    }

    #[test]
    fn install_write_only_writes_file_but_never_enables() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(UNIT_NAME);
        let sc = FakeSystemctl::new(true);

        install(&path, "/abs/rustline", true, &sc);

        assert!(path.is_file());
        assert!(sc.calls.borrow().is_empty());
    }

    #[test]
    fn install_degrades_gracefully_when_systemctl_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(UNIT_NAME);
        let sc = FakeSystemctl::new(false);

        install(&path, "/abs/rustline", false, &sc);

        assert!(path.is_file(), "the unit file is still written");
        assert!(
            sc.calls.borrow().is_empty(),
            "no systemctl call is attempted when unavailable"
        );
    }

    #[test]
    fn uninstall_removes_file_and_disables_then_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(UNIT_NAME);
        fs::write(&path, service_unit("/abs/rustline")).unwrap();
        let sc = FakeSystemctl::new(true);

        uninstall(&path, &sc);

        assert!(!path.exists());
        assert_eq!(
            *sc.calls.borrow(),
            vec![
                format!("disable_now:{UNIT_NAME}"),
                "daemon_reload".to_string(),
            ]
        );

        // A second uninstall (file already gone) must not error/panic.
        uninstall(&path, &sc);
        assert!(!path.exists());
    }
}
