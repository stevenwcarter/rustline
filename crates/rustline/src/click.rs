//! Click resolution and dispatch (W36): map a `(range, button)` click from the
//! tmux mouse binding to a typed [`ClickAction`] and execute its side effect.
//!
//! Resolution ([`resolve_click`]) is pure over the [`Config`]; execution goes
//! through the [`ClickExecutor`] seam so the resolve+dispatch logic is
//! unit-tested without spawning real processes. The only production executor,
//! [`RealExecutor`], flips the toggle state file (unchanged from before W36),
//! opens a URL with the OS opener, or runs a detached shell command.
//!
//! **Default preservation.** With no binding configured, resolution is
//! byte-identical to the pre-W36 `run_click`: a left-click on a clickable
//! widget or a plugin/unknown range flips its toggle-set membership, and every
//! other case is a no-op.
//!
//! **Injection safety (invariant #4).** The URL/command text a click runs comes
//! *only* from the user's own `config.toml` (a [`ClickBinding`]); the tmux
//! `range` value (`#{q:mouse_status_range}`, already `#{q:}`-escaped as a
//! `--flag=` arg) is used *only* to select which binding runs — it is never
//! interpolated into a shell string. No tmux-provided data reaches `sh -c`.

use std::process::{Command, Stdio};

use rustline_core::{ClickBinding, Config};

use crate::toggles;

/// The button name tmux's `MouseDown1Status` binding sends today. Right/middle
/// bindings are parsed and resolvable but need their own tmux bindings to fire.
const LEFT: &str = "left";

/// The resolved action for a click, after applying the configured bindings and
/// the default behavior. This is the runtime type the dispatcher acts on —
/// distinct from the config-value [`ClickBinding`] (which has no `NoOp` and
/// whose `Toggle` carries the `{ toggle = true }` bool).
#[derive(Clone, Debug, PartialEq)]
pub enum ClickAction {
    /// Flip the widget/plugin's membership in the global toggle set.
    Toggle,
    /// Open a URL with the OS opener (`xdg-open`/`open`).
    OpenUrl(String),
    /// Run a shell command (`sh -c <cmd>`), detached.
    Run(String),
    /// Do nothing.
    NoOp,
}

/// Executes a resolved [`ClickAction`]'s side effect. Behind a trait so
/// [`resolve_click`] + [`dispatch`] are unit-tested with a recording fake,
/// never spawning real processes.
pub trait ClickExecutor {
    /// Flip `range`'s membership in the toggle state file.
    fn toggle(&self, range: &str);
    /// Open `url` with the OS opener.
    fn open_url(&self, url: &str);
    /// Run `command` via `sh -c`, detached.
    fn run(&self, command: &str);
}

/// Map a configured [`ClickBinding`] to the runtime [`ClickAction`] it selects.
/// `Toggle(false)` explicitly disables the default toggle (→ `NoOp`).
fn binding_to_action(binding: &ClickBinding) -> ClickAction {
    match binding {
        ClickBinding::Toggle(true) => ClickAction::Toggle,
        ClickBinding::Toggle(false) => ClickAction::NoOp,
        ClickBinding::OpenUrl(url) => ClickAction::OpenUrl(url.clone()),
        ClickBinding::Run(command) => ClickAction::Run(command.clone()),
    }
}

/// Resolve the action for a click on `range` with mouse `button`.
///
/// A binding configured for `(range, button)` wins. Otherwise the default:
/// a left-click on a *clickable* range → [`ClickAction::Toggle`], everything
/// else → [`ClickAction::NoOp`]. "Clickable" means the widget has a non-empty
/// `alt_format` (so it emits a range and left-click toggles it) — or the range
/// isn't a known built-in at all (a WASM plugin, or an unknown name), which is
/// treated as clickable to preserve the pre-W36 behavior of flipping any
/// plugin/unknown range on a left-click (invariant #7).
pub fn resolve_click(cfg: &Config, range: &str, button: &str) -> ClickAction {
    if range.is_empty() {
        return ClickAction::NoOp;
    }
    match cfg.click_map().get(range) {
        // A known clickable-candidate built-in widget.
        Some(widget) => {
            if let Some(binding) = widget.bindings.for_button(button) {
                binding_to_action(binding)
            } else if button == LEFT && widget.toggleable {
                ClickAction::Toggle
            } else {
                ClickAction::NoOp
            }
        }
        // Not a known built-in (a plugin — bindings live under [plugins.*] —
        // or an unknown range): preserve the pre-W36 flip-any-range-on-left
        // behavior so plugin click-toggle keeps working (invariant #7).
        None => {
            if button == LEFT {
                ClickAction::Toggle
            } else {
                ClickAction::NoOp
            }
        }
    }
}

/// Execute a resolved [`ClickAction`] through `exec`. `range` is only needed by
/// [`ClickAction::Toggle`].
pub fn dispatch(action: ClickAction, range: &str, exec: &impl ClickExecutor) {
    match action {
        ClickAction::Toggle => exec.toggle(range),
        ClickAction::OpenUrl(url) => exec.open_url(&url),
        ClickAction::Run(command) => exec.run(&command),
        ClickAction::NoOp => {}
    }
}

/// The production [`ClickExecutor`]: real toggle-file writes and real
/// (best-effort, detached) process spawns.
pub struct RealExecutor;

impl ClickExecutor for RealExecutor {
    fn toggle(&self, range: &str) {
        let mut set = toggles::read_toggles();
        toggles::apply_toggle(&mut set, range);
        toggles::write_toggles(&set);
    }

    fn open_url(&self, url: &str) {
        spawn_detached(opener_command(), &[url]);
    }

    fn run(&self, command: &str) {
        spawn_detached("sh", &["-c", command]);
    }
}

/// The OS "open a URL/file" launcher: `open` on macOS, `xdg-open` elsewhere.
fn opener_command() -> &'static str {
    if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    }
}

/// Spawn `program args`, fully detached and best-effort: stdio is redirected to
/// `/dev/null` (so the child never holds tmux's `run-shell` output pipe open),
/// the child is never waited on, and a spawn failure is logged rather than
/// fatal — a click must never break the bar.
fn spawn_detached(program: &str, args: &[&str]) {
    let spawn = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Err(error) = spawn {
        tracing::warn!("failed to spawn click handler `{program}` {args:?}: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A [`ClickExecutor`] that records the method + argument it was called
    /// with, so dispatch is verified without spawning anything.
    #[derive(Default)]
    struct FakeExecutor {
        calls: RefCell<Vec<String>>,
    }

    impl ClickExecutor for FakeExecutor {
        fn toggle(&self, range: &str) {
            self.calls.borrow_mut().push(format!("toggle:{range}"));
        }
        fn open_url(&self, url: &str) {
            self.calls.borrow_mut().push(format!("open_url:{url}"));
        }
        fn run(&self, command: &str) {
            self.calls.borrow_mut().push(format!("run:{command}"));
        }
    }

    /// A default config with `datetime` made click-toggleable (non-empty
    /// `alt_format`), no bindings.
    fn cfg_datetime_toggleable() -> Config {
        let mut cfg = Config::default();
        cfg.widgets.datetime.alt_format = "%H:%M".into();
        cfg
    }

    #[test]
    fn resolves_default_and_configured() {
        // No binding: left on a toggleable widget toggles; right is a no-op.
        let cfg = cfg_datetime_toggleable();
        assert!(matches!(
            resolve_click(&cfg, "datetime", "left"),
            ClickAction::Toggle
        ));
        assert!(matches!(
            resolve_click(&cfg, "datetime", "right"),
            ClickAction::NoOp
        ));

        // A configured right-click `run` resolves to Run; an unbound button
        // on that widget stays a no-op.
        let mut cfg2 = Config::default();
        cfg2.widgets.cpu.click.right_click = Some(ClickBinding::Run("htop".into()));
        assert!(matches!(
            resolve_click(&cfg2, "cpu", "right"),
            ClickAction::Run(ref c) if c == "htop"
        ));
        assert!(matches!(
            resolve_click(&cfg2, "cpu", "middle"),
            ClickAction::NoOp
        ));
    }

    #[test]
    fn default_left_on_non_toggleable_widget_is_noop() {
        // cpu is a known built-in with an empty alt_format by default, so a
        // left-click without a binding does nothing.
        let cfg = Config::default();
        assert!(matches!(
            resolve_click(&cfg, "cpu", "left"),
            ClickAction::NoOp
        ));
    }

    #[test]
    fn configured_open_url_and_explicit_toggle_override_default() {
        let mut cfg = Config::default();
        cfg.widgets.datetime.click.middle_click =
            Some(ClickBinding::OpenUrl("https://example.com".into()));
        // An explicit toggle binding wins even though cpu is not toggleable
        // by default (empty alt_format).
        cfg.widgets.cpu.click.left_click = Some(ClickBinding::Toggle(true));

        assert!(matches!(
            resolve_click(&cfg, "datetime", "middle"),
            ClickAction::OpenUrl(ref u) if u == "https://example.com"
        ));
        assert!(matches!(
            resolve_click(&cfg, "cpu", "left"),
            ClickAction::Toggle
        ));
    }

    #[test]
    fn explicit_toggle_false_binding_is_noop() {
        let mut cfg = cfg_datetime_toggleable();
        // Disable the default left toggle on an otherwise-toggleable widget.
        cfg.widgets.datetime.click.left_click = Some(ClickBinding::Toggle(false));
        assert!(matches!(
            resolve_click(&cfg, "datetime", "left"),
            ClickAction::NoOp
        ));
    }

    #[test]
    fn unknown_button_is_noop() {
        let mut cfg = Config::default();
        cfg.widgets.cpu.click.right_click = Some(ClickBinding::Run("htop".into()));
        assert!(matches!(
            resolve_click(&cfg, "cpu", "scroll"),
            ClickAction::NoOp
        ));
    }

    #[test]
    fn empty_range_is_noop() {
        assert!(matches!(
            resolve_click(&Config::default(), "", "left"),
            ClickAction::NoOp
        ));
        assert!(matches!(
            resolve_click(&Config::default(), "", "right"),
            ClickAction::NoOp
        ));
    }

    #[test]
    fn plugin_or_unknown_name_preserves_left_toggle() {
        // A plugin (configured under [plugins.*], absent from the widget map)
        // — or any unknown range — must still flip on a left-click, exactly
        // as the pre-W36 run_click did (invariant #7); non-left is a no-op.
        let cfg = Config::default();
        assert!(matches!(
            resolve_click(&cfg, "weatherplug", "left"),
            ClickAction::Toggle
        ));
        assert!(matches!(
            resolve_click(&cfg, "weatherplug", "right"),
            ClickAction::NoOp
        ));
    }

    #[test]
    fn default_dispatch_reproduces_toggle_behavior() {
        // Characterization: with no bindings, a left-click on a toggleable
        // widget flips its toggle-set membership (via the executor), and
        // nothing else fires.
        let cfg = cfg_datetime_toggleable();

        let fake = FakeExecutor::default();
        dispatch(resolve_click(&cfg, "datetime", "left"), "datetime", &fake);
        assert_eq!(*fake.calls.borrow(), vec!["toggle:datetime".to_string()]);

        let fake2 = FakeExecutor::default();
        dispatch(resolve_click(&cfg, "datetime", "right"), "datetime", &fake2);
        assert!(fake2.calls.borrow().is_empty());
    }

    #[test]
    fn dispatch_routes_to_executor_without_spawning() {
        let fake = FakeExecutor::default();
        dispatch(ClickAction::Run("htop".into()), "cpu", &fake);
        dispatch(ClickAction::OpenUrl("https://x".into()), "cpu", &fake);
        dispatch(ClickAction::Toggle, "cpu", &fake);
        dispatch(ClickAction::NoOp, "cpu", &fake); // adds nothing
        assert_eq!(
            *fake.calls.borrow(),
            vec![
                "run:htop".to_string(),
                "open_url:https://x".to_string(),
                "toggle:cpu".to_string(),
            ]
        );
    }
}
