//! Build a [`Context`] from CLI arguments plus live host state (env vars,
//! load average, hostname, wall clock).

use std::env;

use crate::cli::{RegionArgs, WindowArgs};
use rustline_core::{Context, WindowCtx};

/// Read the 1/5/15-minute load average via `getloadavg(3)`.
///
/// Returns `None` if the platform call doesn't report all three samples
/// (its documented failure mode), so a widget can fall back gracefully
/// instead of showing bogus zeros.
fn read_loadavg() -> Option<[f64; 3]> {
    let mut out = [0f64; 3];
    // SAFETY: `out` is a valid, properly aligned buffer for 3 `f64`s, and
    // `getloadavg` is documented to write at most `out.len()` samples into it.
    let n = unsafe { libc::getloadavg(out.as_mut_ptr(), 3) };
    if n == 3 { Some(out) } else { None }
}

/// The local machine's hostname, lossily converted to UTF-8 (hostnames are
/// display-only here, never round-tripped back to the OS).
fn hostname() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

/// Build the [`Context`] for rendering a left/right region from the tmux
/// format-variable values passed on the command line, plus live host state.
pub fn build_region_context(args: &RegionArgs) -> Context {
    Context {
        session_name: args.session.clone().unwrap_or_default(),
        window_index: args.window.clone().unwrap_or_default(),
        pane_index: args.pane.clone().unwrap_or_default(),
        pane_current_path: args.pane_path.clone().unwrap_or_default(),
        home: env::var("HOME").unwrap_or_default(),
        hostname: hostname(),
        loadavg: read_loadavg(),
        now: chrono::Local::now(),
        window: None,
        interfaces: Vec::new(),
    }
}

/// Build the [`Context`] for rendering a single window segment. Reuses
/// [`build_region_context`] for the host/pane-agnostic fields (there is no
/// pane in play for a window segment) and layers on the window-specific
/// fields from `args`.
pub fn build_window_context(args: &WindowArgs) -> Context {
    let mut ctx = build_region_context(&RegionArgs::default());
    ctx.window = Some(WindowCtx {
        index: args.index.clone(),
        name: args.name.clone(),
        flags: args.flags.clone(),
        is_current: args.current,
    });
    ctx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_from_env_used_when_present() {
        // build_context reads $HOME; assert the field is populated non-empty
        let ctx = build_region_context(&RegionArgs::default());
        assert!(!ctx.home.is_empty() || std::env::var("HOME").is_err());
    }
}
