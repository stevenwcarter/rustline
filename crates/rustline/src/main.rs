mod build_context;
mod cli;
mod tmux_conf;

use std::env;
use std::io;
use std::path::PathBuf;

use build_context::{build_region_context, build_window_context};
use clap::Parser;
use cli::{Cli, Command, Render};
use rustline_core::{Config, Direction, Registry, render_named_region, render_window};
use tracing_subscriber::{EnvFilter, fmt};

/// Resolve the config file path: `$XDG_CONFIG_HOME/rustline/config.toml`,
/// falling back to `$HOME/.config/rustline/config.toml` when unset.
fn config_path() -> PathBuf {
    let base = env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env::var("HOME").unwrap_or_default()).join(".config"));
    base.join("rustline").join("config.toml")
}

fn main() {
    // This CLI's stdout IS the tmux status line, so keep it quiet by
    // default; logs go to stderr, never stdout.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    fmt().with_env_filter(filter).with_writer(io::stderr).init();

    let cli = Cli::parse();
    let cfg = Config::load(&config_path());
    let registry = Registry::with_builtins(&cfg);
    let theme = cfg.to_theme();

    match cli.command {
        Command::Render(Render::Left(args)) => {
            let ctx = build_region_context(&args);
            let out =
                render_named_region(Direction::Left, &cfg.layout.left, &ctx, &registry, &theme);
            print!("{out}");
        }
        Command::Render(Render::Right(args)) => {
            let ctx = build_region_context(&args);
            let out =
                render_named_region(Direction::Right, &cfg.layout.right, &ctx, &registry, &theme);
            print!("{out}");
        }
        Command::Render(Render::Window(args)) => {
            let ctx = build_window_context(&args);
            print!("{}", render_window(&ctx, &registry, &theme));
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
    }
}
