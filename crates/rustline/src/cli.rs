//! Command-line surface for the `rustline` binary, defined declaratively
//! with `clap`'s derive API.
//!
//! `render` is a subcommand *group* (not a flat set of flags) so that
//! `rustline render left`, `rustline render right`, and
//! `rustline render window [--current] --index <i> --name <n> --flags <f>` all
//! parse as `rustline render <region-or-window> ...`.

use clap::{Args, Parser, Subcommand};

/// Rust tmux statusline: renders status-line regions and window segments,
/// and helps wire itself into a tmux config.
#[derive(Parser)]
#[command(version, about = "Rust tmux statusline")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level subcommands.
#[derive(Subcommand)]
pub enum Command {
    /// Render a region or a single window segment.
    #[command(subcommand)]
    Render(Render),
    /// Print the tmux.conf block to enable rustline.
    Init,
    /// Print the effective config as TOML.
    PrintConfig,
}

/// The `render` subcommand group: which region or window segment to render.
#[derive(Subcommand)]
pub enum Render {
    /// Render the left status-line region.
    Left(RegionArgs),
    /// Render the right status-line region.
    Right(RegionArgs),
    /// Render a single window's segment (for `window-status-format`).
    Window(WindowArgs),
}

/// Arguments for rendering a left/right region, sourced from tmux format
/// variables (e.g. `#{session_name}`) by the tmux config `init` produces.
///
/// All fields are optional so the same struct can be defaulted for contexts
/// that don't apply (e.g. building a window context, which has no pane).
#[derive(Args, Default)]
pub struct RegionArgs {
    #[arg(long)]
    pub session: Option<String>,
    #[arg(long)]
    pub window: Option<String>,
    #[arg(long)]
    pub pane: Option<String>,
    #[arg(long)]
    pub pane_path: Option<String>,
}

/// Arguments for rendering one window's segment in the window list, sourced
/// from tmux format variables by the config `init` produces.
///
/// These are named (`--index`/`--name`/`--flags`) rather than positional so the
/// tmux config can pass each value in injection-safe `--flag=#{q:...}` form —
/// see [`crate::tmux_conf::init_block`]. `--name`/`--flags` default to empty so
/// an unnamed or unflagged window still parses as a present, empty value.
#[derive(Args)]
pub struct WindowArgs {
    /// Whether this is the currently active window.
    #[arg(long)]
    pub current: bool,
    /// The window's index (tmux `#{window_index}`).
    #[arg(long)]
    pub index: String,
    /// The window's name (tmux `#{window_name}`); may be empty.
    #[arg(long, default_value = "")]
    pub name: String,
    /// The window's flags (tmux `#{window_flags}`); may be empty.
    #[arg(long, default_value = "")]
    pub flags: String,
}
