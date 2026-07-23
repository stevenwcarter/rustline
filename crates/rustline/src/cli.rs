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
    /// Increase file-log verbosity: -v=warn, -vv=info, -vvv=debug, -vvvv=trace.
    /// Without -v the file logs at info (or the config's `log.file_level`);
    /// stderr is unaffected (see `log.stderr_level`).
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level subcommands.
#[derive(Subcommand)]
pub enum Command {
    /// Render a region or a single window segment.
    #[command(subcommand)]
    Render(Render),
    /// Onboarding wizard: write config.toml + a tmux marker-block. `--defaults`
    /// runs non-interactively; `--print` emits the raw tmux block (legacy);
    /// `--uninstall` removes the managed tmux block instead.
    Init(InitArgs),
    /// Print the effective config as TOML.
    PrintConfig,
    /// Manage plugins and their capability allowlists.
    #[command(subcommand)]
    Plugin(PluginCmd),
    /// List, preview, select, or scaffold themes.
    #[command(subcommand)]
    Theme(ThemeCmd),
    /// Toggle a widget's alt view (invoked by the tmux MouseDown1Status binding).
    Click(ClickArgs),
    /// Diagnose documented prerequisites (tmux version, mouse mode,
    /// truecolor terminal, PATH, the managed tmux config block) and report
    /// pass/warn/fail, plus the resolved config/themes/plugin/log paths.
    Doctor,
    /// Print a shell-completion script for the given shell to stdout.
    Completions {
        /// Which shell to generate a completion script for.
        shell: clap_complete::Shell,
    },
    /// Benchmark the render pipeline (feature `bench`).
    #[cfg(feature = "bench")]
    Bench(BenchArgs),
}

/// Arguments for `rustline init`.
#[derive(Args, Default)]
pub struct InitArgs {
    /// Non-interactive: use recommended defaults and write both files.
    #[arg(long)]
    pub defaults: bool,
    /// Print the raw one-line tmux block to stdout and write nothing (legacy).
    #[arg(long)]
    pub print: bool,
    /// Preview the config.toml and tmux block that would be written, without
    /// touching disk. Answers are gathered the same way as a real run
    /// (`--defaults`, or the interactive wizard on a TTY); `--print` takes
    /// precedence if both are given.
    #[arg(long)]
    pub dry_run: bool,
    /// Remove the rustline-managed tmux marker-block from `~/.tmux.conf`
    /// (backing it up first) and print the reload command; touches nothing
    /// else — `config.toml` is left alone — and needs no TTY. Checked before
    /// `--defaults`/the interactive wizard; `--print` still wins if both are
    /// given (see `init::run`).
    #[arg(long)]
    pub uninstall: bool,
    /// Override the binary path baked into the tmux block's `#(...)` calls
    /// (default: the running binary's own resolved absolute path via
    /// `std::env::current_exe()`).
    #[arg(long)]
    pub binary: Option<String>,
}

/// Manage themes: list, preview, select, and scaffold new ones.
#[derive(Subcommand)]
pub enum ThemeCmd {
    /// List built-in and themes-dir themes (marks the active one).
    List,
    /// Print an ANSI colour preview of a theme.
    Show { name: String },
    /// Select a theme by writing `[theme].base` into the config file.
    Use { name: String },
    /// Interactively browse theme previews and set one.
    Pick,
    /// Scaffold a new tweakable theme file seeded from an existing theme.
    New {
        name: String,
        /// Seed theme to copy from (built-in or themes-dir stem). Default: `default`.
        #[arg(long, default_value = "default")]
        from: String,
        /// Overwrite an existing theme file.
        #[arg(long)]
        force: bool,
    },
}

/// Manage discovered plugins and their capability allowlists.
#[derive(Subcommand)]
pub enum PluginCmd {
    /// List configured plugins and their allowlists/caps.
    List,
    /// Manage a plugin's URL allowlist.
    #[command(subcommand)]
    Url(PatternCmd),
    /// Manage a plugin's filesystem-path allowlist.
    #[command(subcommand)]
    Path(PatternCmd),
}

/// list/add/remove operations over one allowlist of a named plugin.
#[derive(Subcommand)]
pub enum PatternCmd {
    /// List the plugin's patterns.
    List { plugin: String },
    /// Append a pattern (idempotent).
    Add { plugin: String, pattern: String },
    /// Remove an exact-match pattern.
    Remove { plugin: String, pattern: String },
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
    /// Print the rendered region in ANSI colour (for manual terminal preview)
    /// instead of raw tmux markup.
    #[arg(long)]
    pub preview: bool,
    /// Override the plugin discovery directory (default
    /// `$XDG_DATA_HOME/rustline/plugins`, or config `plugin_dir`).
    #[arg(long)]
    pub plugin_dir: Option<String>,
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
    /// Print the rendered segment in ANSI colour (for manual terminal preview)
    /// instead of raw tmux markup.
    #[arg(long)]
    pub preview: bool,
}

/// Arguments for `rustline click`, sourced from the tmux mouse binding.
#[derive(Args)]
pub struct ClickArgs {
    /// The clicked widget's range name (tmux `#{mouse_status_range}`); empty = no-op.
    #[arg(long, default_value = "")]
    pub range: String,
    /// Which mouse button (currently only `left` acts; others are reserved).
    #[arg(long, default_value = "left")]
    pub button: String,
}

/// Arguments for `rustline bench` (feature `bench`).
#[cfg(feature = "bench")]
#[derive(Args, Debug)]
pub struct BenchArgs {
    /// Which group to bench: regions|widgets|sources|plugins|all.
    #[arg(long, default_value = "all")]
    pub only: String,
    /// Samples for the fast/pure passes.
    #[arg(long, default_value_t = 1000)]
    pub iters: usize,
    /// Samples for the real-I/O passes (reads, real-world regions, plugin per-tick).
    #[arg(long = "real-iters", default_value_t = 25)]
    pub real_iters: usize,
    /// Warmup iterations (discarded) for the pure passes.
    #[arg(long, default_value_t = 50)]
    pub warmup: usize,
    /// Include plugin cold-start (clears the plugin's cache; may hit the network).
    #[arg(long)]
    pub cold: bool,
    /// Output format: table|markdown.
    #[arg(long, default_value = "table")]
    pub format: String,
    /// Write the report to a file instead of stdout.
    #[arg(long)]
    pub output: Option<String>,
    /// Override the plugin discovery directory (same resolution as render).
    #[arg(long = "plugin-dir")]
    pub plugin_dir: Option<String>,
    /// Override the plugin state/cache root (default: real state_root());
    /// does not affect plugin discovery.
    #[arg(long = "state-dir")]
    pub state_dir: Option<String>,
}
