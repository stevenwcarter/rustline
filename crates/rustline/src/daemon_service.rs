//! `rustline daemon install`/`uninstall`: make the optional render daemon
//! (`daemon.rs`) survive logins/reboots without the user hand-writing a service
//! file, using each platform's native per-user service mechanism:
//!
//! - **Linux** → a systemd **user** unit at
//!   `$XDG_CONFIG_HOME/systemd/user/rustline-daemon.service`, wired via
//!   `systemctl --user`.
//! - **macOS** → a launchd **LaunchAgent** plist at
//!   `~/Library/LaunchAgents/rustline-daemon.plist`, wired via `launchctl`
//!   (`bootstrap`/`bootout gui/$UID`). `RunAtLoad` starts it at each login and
//!   `KeepAlive={SuccessfulExit=false}` restarts it on failure only — the
//!   direct analogue of the systemd unit's `Restart=on-failure`.
//!
//! [`install`]/[`uninstall`] are platform-agnostic entry points that dispatch
//! to the right backend at compile time; `main.rs` names neither the path nor
//! the service manager. The pure builders (`service_unit`/`plist_contents`) and
//! path resolvers (`service_unit_path`/`plist_path`) are `#[cfg(any(target_os =
//! …, test))]`-compiled so **both** backends unit-test on the Linux CI box and
//! on macOS. The service managers themselves sit behind the [`Systemctl`] /
//! [`Launchctl`] traits — the same seam `click.rs`'s `ClickExecutor` uses — so
//! the install/uninstall flows are verified with recording fakes instead of a
//! real `systemctl`/`launchctl`. Every real invocation is best-effort: a spawn
//! failure, non-zero exit, or a missing tool is just `false`, never a panic.
//! Neither operation treats a service-manager failure as fatal — the unit/plist
//! **file** is the durable part, and both degrade to printing a manual command.

#[cfg(any(target_os = "linux", target_os = "macos", test))]
use std::env;
#[cfg(any(target_os = "linux", target_os = "macos", test))]
use std::fs;
use std::path::Path;
#[cfg(any(target_os = "linux", target_os = "macos", test))]
use std::path::PathBuf;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::process::{Command, Stdio};

// ===========================================================================
// Platform-agnostic entry points
// ===========================================================================

/// `rustline daemon install`: write the platform's service file for a daemon
/// started via the resolved absolute `binary` path and (unless `write_only`)
/// load/enable it so it starts now and at every login. Dispatches to the
/// systemd (Linux) or launchd (macOS) backend at compile time; an unsupported
/// platform prints a notice and does nothing.
pub fn install(binary: &str, write_only: bool) {
    #[cfg(target_os = "linux")]
    {
        install_systemd(&service_unit_path(), binary, write_only, &RealSystemctl);
    }
    #[cfg(target_os = "macos")]
    {
        install_launchd(&plist_path(), binary, write_only, &RealLaunchctl);
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (binary, write_only);
        eprintln!(
            "`rustline daemon install` is only supported on Linux (systemd) and macOS (launchd)."
        );
    }
}

/// `rustline daemon uninstall`: unload/disable the service and remove its file.
/// Idempotent and safe to run when nothing is installed. Dispatches per OS.
pub fn uninstall() {
    #[cfg(target_os = "linux")]
    {
        uninstall_systemd(&service_unit_path(), &RealSystemctl);
    }
    #[cfg(target_os = "macos")]
    {
        uninstall_launchd(&plist_path(), &RealLaunchctl);
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        eprintln!(
            "`rustline daemon uninstall` is only supported on Linux (systemd) and macOS (launchd)."
        );
    }
}

/// Whether `name` resolves to an executable file in some `$PATH` entry (the
/// same probe `doctor.rs`'s `binary_on_path` uses for `rustline` itself).
/// Shared by both backends' `available()`.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn command_on_path(name: &str) -> bool {
    env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}

/// Run `program args…` with all stdio silenced, returning `true` iff it spawned
/// and exited successfully. Shared by the systemd and launchd real managers.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn run_silent(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

// ===========================================================================
// Linux backend — systemd user unit
// ===========================================================================

/// The systemd unit name, and the argument `systemctl --user enable|disable
/// --now` is given.
#[cfg(any(target_os = "linux", test))]
const UNIT_NAME: &str = "rustline-daemon.service";

/// The systemd unit text for a daemon started via the resolved absolute
/// `binary` path (same resolution `rustline init` uses for its tmux block —
/// see `crate::resolve_binary`).
#[cfg(any(target_os = "linux", test))]
fn service_unit(binary: &str) -> String {
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
#[cfg(any(target_os = "linux", test))]
fn service_unit_path() -> PathBuf {
    let base = env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env::var("HOME").unwrap_or_default()).join(".config"));
    base.join("systemd").join("user").join(UNIT_NAME)
}

/// Executes the `systemctl --user` operations the systemd backend needs. Behind
/// a trait so install/uninstall are unit-tested with a recording fake.
#[cfg(any(target_os = "linux", test))]
trait Systemctl {
    /// Whether `systemctl` is usable (found on `$PATH`).
    fn available(&self) -> bool;
    /// `systemctl --user daemon-reload`.
    fn daemon_reload(&self) -> bool;
    /// `systemctl --user enable --now <unit>`.
    fn enable_now(&self, unit: &str) -> bool;
    /// `systemctl --user disable --now <unit>`.
    fn disable_now(&self, unit: &str) -> bool;
}

/// The production [`Systemctl`]: real `systemctl --user` invocations, every
/// method best-effort (`false` on spawn failure / non-zero exit).
#[cfg(target_os = "linux")]
struct RealSystemctl;

#[cfg(target_os = "linux")]
impl Systemctl for RealSystemctl {
    fn available(&self) -> bool {
        command_on_path("systemctl")
    }
    fn daemon_reload(&self) -> bool {
        run_silent("systemctl", &["--user", "daemon-reload"])
    }
    fn enable_now(&self, unit: &str) -> bool {
        run_silent("systemctl", &["--user", "enable", "--now", unit])
    }
    fn disable_now(&self, unit: &str) -> bool {
        run_silent("systemctl", &["--user", "disable", "--now", unit])
    }
}

/// Print the manual follow-up for a systemd unit written but not enabled.
#[cfg(any(target_os = "linux", test))]
fn print_manual_start_hint() {
    eprintln!("Start it with:  systemctl --user enable --now {UNIT_NAME}");
}

/// Write the systemd unit to `unit_path` (creating parent dirs; overwriting a
/// rustline-managed file is fine and reported as a replace), then either leave
/// it for the user (`write_only`, or `systemctl` unavailable) or reload the
/// user daemon-reload cache and enable+start it. Never exits on a `systemctl`
/// failure — the unit file is the durable part.
#[cfg(any(target_os = "linux", test))]
fn install_systemd(unit_path: &Path, binary: &str, write_only: bool, sc: &dyn Systemctl) {
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

/// Best-effort disable+stop the unit (ignoring a "not loaded" failure), remove
/// `unit_path` if present, then reload the user daemon-reload cache. A missing
/// file is reported, not an error — a second uninstall is always safe.
#[cfg(any(target_os = "linux", test))]
fn uninstall_systemd(unit_path: &Path, sc: &dyn Systemctl) {
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

// ===========================================================================
// macOS backend — launchd LaunchAgent
// ===========================================================================

/// The launchd job label, and the basename (`<label>.plist`) of the LaunchAgent
/// file. Kept symmetric with the systemd `rustline-daemon.service` unit name.
#[cfg(any(target_os = "macos", test))]
const AGENT_LABEL: &str = "rustline-daemon";

/// XML-escape the characters that are significant inside a plist `<string>`
/// element's text (`&`, `<`, `>`). A `current_exe()` path realistically won't
/// contain them, but escaping keeps the plist well-formed regardless.
#[cfg(any(target_os = "macos", test))]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The launchd LaunchAgent plist text for a daemon started via the resolved
/// absolute `binary` path. `RunAtLoad` starts it at login;
/// `KeepAlive={SuccessfulExit=false}` restarts it only on a non-zero exit
/// (a clean `rustline daemon stop` stays stopped), mirroring the systemd unit's
/// `Restart=on-failure`.
#[cfg(any(target_os = "macos", test))]
fn plist_contents(binary: &str) -> String {
    let binary = xml_escape(binary);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Managed by `rustline daemon install`. -->
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{AGENT_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{binary}</string>
        <string>daemon</string>
        <string>run</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
</dict>
</plist>
"#
    )
}

/// Where the LaunchAgent goes: `$HOME/Library/LaunchAgents/rustline-daemon.plist`
/// (macOS per-user agents live under `~/Library`, not XDG). launchd loads every
/// plist here at login.
#[cfg(any(target_os = "macos", test))]
fn plist_path() -> PathBuf {
    PathBuf::from(env::var("HOME").unwrap_or_default())
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{AGENT_LABEL}.plist"))
}

/// Executes the `launchctl` operations the launchd backend needs. Behind a
/// trait so install/uninstall are unit-tested with a recording fake. The
/// `gui/$UID` domain the real calls target is an implementation detail of
/// [`RealLaunchctl`], not exposed here.
#[cfg(any(target_os = "macos", test))]
trait Launchctl {
    /// Whether `launchctl` is usable (found on `$PATH`).
    fn available(&self) -> bool;
    /// `launchctl bootstrap gui/$UID <plist>` — load it and (via `RunAtLoad`)
    /// start it now.
    fn bootstrap(&self, plist: &Path) -> bool;
    /// `launchctl bootout gui/$UID <plist>` — unload it.
    fn bootout(&self, plist: &Path) -> bool;
}

/// The production [`Launchctl`]: real `launchctl` invocations against the
/// caller's `gui/$UID` GUI domain, every method best-effort.
#[cfg(target_os = "macos")]
struct RealLaunchctl;

#[cfg(target_os = "macos")]
impl Launchctl for RealLaunchctl {
    fn available(&self) -> bool {
        command_on_path("launchctl")
    }
    fn bootstrap(&self, plist: &Path) -> bool {
        // `bootstrap` must name the plist file — it reads it to load the job.
        let domain = gui_domain();
        let plist = plist.to_string_lossy();
        run_silent("launchctl", &["bootstrap", domain.as_str(), plist.as_ref()])
    }
    fn bootout(&self, _plist: &Path) -> bool {
        // Unload by the job's service target `gui/$UID/<label>` rather than the
        // plist path, so uninstall still unloads the job even if its plist file
        // was manually removed first — symmetric with the systemd path's
        // label-based `disable_now(UNIT_NAME)`.
        let target = format!("{}/{AGENT_LABEL}", gui_domain());
        run_silent("launchctl", &["bootout", &target])
    }
}

/// The caller's per-user GUI launchd domain, `gui/<uid>`.
#[cfg(target_os = "macos")]
fn gui_domain() -> String {
    // SAFETY: `getuid` has no preconditions and always succeeds (POSIX).
    format!("gui/{}", unsafe { libc::getuid() })
}

/// Print the manual follow-up for a LaunchAgent written but not loaded.
#[cfg(any(target_os = "macos", test))]
fn print_manual_load_hint(plist: &Path) {
    eprintln!(
        "Load it with:  launchctl bootstrap gui/$(id -u) {}",
        plist.display()
    );
}

/// Write the LaunchAgent plist to `plist_path` (creating parent dirs;
/// overwriting a rustline-managed file is fine and reported as a replace), then
/// either leave it for the user (`write_only`, or `launchctl` unavailable) or
/// reload it: `bootout` any stale instance first (ignored if not loaded, which
/// makes reinstall idempotent) then `bootstrap` it. Never exits on a
/// `launchctl` failure — the plist file is the durable part.
#[cfg(any(target_os = "macos", test))]
fn install_launchd(plist_path: &Path, binary: &str, write_only: bool, lc: &dyn Launchctl) {
    let replacing = plist_path.exists();
    if let Some(parent) = plist_path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        eprintln!("failed to create {}: {e}", parent.display());
        std::process::exit(1);
    }
    if let Err(e) = fs::write(plist_path, plist_contents(binary)) {
        eprintln!("failed to write {}: {e}", plist_path.display());
        std::process::exit(1);
    }
    if replacing {
        eprintln!("Replaced existing LaunchAgent at {}", plist_path.display());
    } else {
        eprintln!("Wrote {}", plist_path.display());
    }

    if write_only {
        print_manual_load_hint(plist_path);
        return;
    }
    if !lc.available() {
        eprintln!("launchctl not found on PATH; the LaunchAgent was written but left inactive.");
        print_manual_load_hint(plist_path);
        return;
    }

    // Unload any prior instance first so a reinstall is idempotent (a fresh
    // install's bootout just no-ops), then load the freshly written plist.
    eprintln!("Loading {AGENT_LABEL} via launchctl...");
    lc.bootout(plist_path);
    if lc.bootstrap(plist_path) {
        eprintln!("Loaded {AGENT_LABEL}.");
    } else {
        eprintln!("failed to load {AGENT_LABEL}.");
        print_manual_load_hint(plist_path);
    }
}

/// Best-effort unload the agent (ignoring a "not loaded" failure), then remove
/// `plist_path` if present. A missing file is reported, not an error — a second
/// uninstall is always safe.
#[cfg(any(target_os = "macos", test))]
fn uninstall_launchd(plist_path: &Path, lc: &dyn Launchctl) {
    if lc.available() {
        lc.bootout(plist_path);
    }
    if plist_path.exists() {
        match fs::remove_file(plist_path) {
            Ok(()) => eprintln!("Removed {}", plist_path.display()),
            Err(e) => eprintln!("failed to remove {}: {e}", plist_path.display()),
        }
    } else {
        eprintln!("nothing to remove: {} not found", plist_path.display());
    }
}

// ===========================================================================
// Tests — both backends exercised on every dev box via the injected fakes.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    // ---- systemd backend ----

    /// A [`Systemctl`] that records every call instead of spawning anything.
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
    fn install_systemd_writes_unit_file_with_service_unit_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(UNIT_NAME);
        let sc = FakeSystemctl::new(true);

        install_systemd(&path, "/abs/rustline", true, &sc);

        let written = fs::read_to_string(&path).unwrap();
        assert_eq!(written, service_unit("/abs/rustline"));
    }

    #[test]
    fn install_systemd_reloads_and_enables_when_available_and_not_write_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(UNIT_NAME);
        let sc = FakeSystemctl::new(true);

        install_systemd(&path, "/abs/rustline", false, &sc);

        assert_eq!(
            *sc.calls.borrow(),
            vec![
                "daemon_reload".to_string(),
                format!("enable_now:{UNIT_NAME}"),
            ]
        );
    }

    #[test]
    fn install_systemd_write_only_writes_file_but_never_enables() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(UNIT_NAME);
        let sc = FakeSystemctl::new(true);

        install_systemd(&path, "/abs/rustline", true, &sc);

        assert!(path.is_file());
        assert!(sc.calls.borrow().is_empty());
    }

    #[test]
    fn install_systemd_degrades_gracefully_when_systemctl_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(UNIT_NAME);
        let sc = FakeSystemctl::new(false);

        install_systemd(&path, "/abs/rustline", false, &sc);

        assert!(path.is_file(), "the unit file is still written");
        assert!(
            sc.calls.borrow().is_empty(),
            "no systemctl call is attempted when unavailable"
        );
    }

    #[test]
    fn uninstall_systemd_removes_file_and_disables_then_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(UNIT_NAME);
        fs::write(&path, service_unit("/abs/rustline")).unwrap();
        let sc = FakeSystemctl::new(true);

        uninstall_systemd(&path, &sc);

        assert!(!path.exists());
        assert_eq!(
            *sc.calls.borrow(),
            vec![
                format!("disable_now:{UNIT_NAME}"),
                "daemon_reload".to_string(),
            ]
        );

        // A second uninstall (file already gone) must not error/panic.
        uninstall_systemd(&path, &sc);
        assert!(!path.exists());
    }

    // ---- launchd backend ----

    /// A [`Launchctl`] that records every call instead of spawning anything.
    struct FakeLaunchctl {
        available: bool,
        calls: RefCell<Vec<String>>,
    }

    impl FakeLaunchctl {
        fn new(available: bool) -> Self {
            FakeLaunchctl {
                available,
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl Launchctl for FakeLaunchctl {
        fn available(&self) -> bool {
            self.available
        }
        fn bootstrap(&self, _plist: &Path) -> bool {
            self.calls.borrow_mut().push("bootstrap".to_string());
            true
        }
        fn bootout(&self, _plist: &Path) -> bool {
            self.calls.borrow_mut().push("bootout".to_string());
            true
        }
    }

    #[test]
    fn plist_contents_has_required_keys() {
        let plist = plist_contents("/abs/path/rustline");
        assert!(plist.contains("<string>rustline-daemon</string>"));
        assert!(plist.contains("<string>/abs/path/rustline</string>"));
        assert!(plist.contains("<string>daemon</string>"));
        assert!(plist.contains("<string>run</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>"));
        // KeepAlive only on non-zero exit (mirrors systemd Restart=on-failure).
        assert!(plist.contains("<key>KeepAlive</key>"));
        assert!(plist.contains("<key>SuccessfulExit</key>"));
    }

    #[test]
    fn plist_contents_xml_escapes_the_binary_path() {
        let plist = plist_contents("/weird & <path>/rustline");
        assert!(plist.contains("<string>/weird &amp; &lt;path&gt;/rustline</string>"));
        assert!(!plist.contains("& <path>"));
    }

    #[test]
    fn plist_path_ends_with_launchagents_suffix() {
        let path = plist_path();
        assert!(
            path.ends_with("Library/LaunchAgents/rustline-daemon.plist"),
            "{path:?}"
        );
    }

    #[test]
    fn install_launchd_writes_plist_with_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("rustline-daemon.plist");
        let lc = FakeLaunchctl::new(true);

        install_launchd(&path, "/abs/rustline", true, &lc);

        let written = fs::read_to_string(&path).unwrap();
        assert_eq!(written, plist_contents("/abs/rustline"));
    }

    #[test]
    fn install_launchd_boots_out_then_bootstraps_when_available_and_not_write_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rustline-daemon.plist");
        let lc = FakeLaunchctl::new(true);

        install_launchd(&path, "/abs/rustline", false, &lc);

        // bootout (clear any stale instance) precedes bootstrap (load fresh).
        assert_eq!(
            *lc.calls.borrow(),
            vec!["bootout".to_string(), "bootstrap".to_string()]
        );
    }

    #[test]
    fn install_launchd_write_only_writes_file_but_never_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rustline-daemon.plist");
        let lc = FakeLaunchctl::new(true);

        install_launchd(&path, "/abs/rustline", true, &lc);

        assert!(path.is_file());
        assert!(lc.calls.borrow().is_empty());
    }

    #[test]
    fn install_launchd_degrades_gracefully_when_launchctl_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rustline-daemon.plist");
        let lc = FakeLaunchctl::new(false);

        install_launchd(&path, "/abs/rustline", false, &lc);

        assert!(path.is_file(), "the plist file is still written");
        assert!(
            lc.calls.borrow().is_empty(),
            "no launchctl call is attempted when unavailable"
        );
    }

    #[test]
    fn uninstall_launchd_removes_plist_and_boots_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rustline-daemon.plist");
        fs::write(&path, plist_contents("/abs/rustline")).unwrap();
        let lc = FakeLaunchctl::new(true);

        uninstall_launchd(&path, &lc);

        assert!(!path.exists());
        assert_eq!(*lc.calls.borrow(), vec!["bootout".to_string()]);

        // A second uninstall (file already gone) must not error/panic.
        uninstall_launchd(&path, &lc);
        assert!(!path.exists());
    }
}
