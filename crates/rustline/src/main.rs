mod battery;
mod build_context;
mod cli;
mod cpu;
mod logging;
mod memory;
mod plugin_cmd;
mod tmux_conf;
mod toggles;

use std::env;
use std::path::PathBuf;

use build_context::{build_region_context, build_window_context};
use clap::Parser;
use cli::{Cli, Command, Render};
use rustline_core::{
    Config, Direction, Registry, render_named_region, render_window, tmux_to_ansi,
};

/// Print a rendered region to stdout: as ANSI-coloured text (with a trailing
/// reset and newline, for terminal preview) when `preview` is set, otherwise as
/// the raw tmux markup tmux itself consumes.
fn emit(markup: &str, preview: bool) {
    if preview {
        println!("{}\x1b[0m", tmux_to_ansi(markup));
    } else {
        print!("{markup}");
    }
}

/// Resolve the plugin discovery dir: `--plugin-dir` flag › config
/// `plugin_dir` › `rustline_wasm::default_plugin_dir()`. A `~/` prefix in the
/// flag or config value is expanded to `$HOME`.
fn resolve_plugin_dir(flag: Option<&str>, cfg: &Config) -> PathBuf {
    if let Some(f) = flag {
        return rustline_wasm::expand_tilde(f);
    }
    if let Some(d) = &cfg.plugin_dir {
        return rustline_wasm::expand_tilde(d);
    }
    rustline_wasm::default_plugin_dir()
}

/// Resolve the config file path: `$XDG_CONFIG_HOME/rustline/config.toml`,
/// falling back to `$HOME/.config/rustline/config.toml` when unset.
fn config_path() -> PathBuf {
    let base = env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env::var("HOME").unwrap_or_default()).join(".config"));
    base.join("rustline").join("config.toml")
}

/// Handle `rustline click`: on a left-click with a non-empty range, flip that
/// widget's membership in the toggle state file. Any other button, or an
/// empty range, is a no-op. Never fails the process (invariant: never break
/// the bar).
///
/// This is the single choke point for click dispatch: a future `left_click`/
/// `right_click` script-handler mechanism should extend the resolution here
/// rather than adding parallel dispatch elsewhere.
fn run_click(args: &cli::ClickArgs) {
    if args.button != "left" || args.range.is_empty() {
        return;
    }
    let mut set = toggles::read_toggles();
    toggles::apply_toggle(&mut set, &args.range);
    toggles::write_toggles(&set);
}

fn main() {
    let cli = Cli::parse();
    // Load config first so logging can honor `[log]`; defer the load-failure
    // warning until the subscriber exists (else it would be dropped).
    let (cfg, load_warning) = Config::load_reporting(&config_path());
    logging::init(&cfg.log, cli.verbose);
    if let Some(msg) = load_warning {
        tracing::warn!("{msg}");
    }
    let theme = cfg.to_theme();

    match cli.command {
        Command::Render(Render::Left(args)) => {
            let plugin_dir = resolve_plugin_dir(args.plugin_dir.as_deref(), &cfg);
            let mut registry = Registry::with_builtins(&cfg);
            rustline_wasm::register_plugins(&mut registry, &cfg, &plugin_dir, &cfg.layout.left);
            let ctx = build_region_context(&args, &cfg.layout.left, &theme);
            let out =
                render_named_region(Direction::Left, &cfg.layout.left, &ctx, &registry, &theme);
            emit(&out, args.preview);
        }
        Command::Render(Render::Right(args)) => {
            let plugin_dir = resolve_plugin_dir(args.plugin_dir.as_deref(), &cfg);
            let mut registry = Registry::with_builtins(&cfg);
            rustline_wasm::register_plugins(&mut registry, &cfg, &plugin_dir, &cfg.layout.right);
            let ctx = build_region_context(&args, &cfg.layout.right, &theme);
            let out =
                render_named_region(Direction::Right, &cfg.layout.right, &ctx, &registry, &theme);
            emit(&out, args.preview);
        }
        Command::Render(Render::Window(args)) => {
            // Windows don't run plugins in v1: builtins only.
            let registry = Registry::with_builtins(&cfg);
            let ctx = build_window_context(&args, &theme);
            emit(&render_window(&ctx, &registry, &theme), args.preview);
        }
        Command::Init => print!(
            "{}",
            tmux_conf::init_block(&theme.bar_bg.to_tmux(), &theme.fg.to_tmux())
        ),
        Command::PrintConfig => match toml::to_string_pretty(&cfg) {
            Ok(s) => print!("{s}"),
            Err(error) => {
                eprintln!("failed to serialize config: {error}");
                std::process::exit(1);
            }
        },
        Command::Plugin(cmd) => plugin_cmd::run(cmd, &config_path()),
        Command::Click(args) => run_click(&args),
    }
}
