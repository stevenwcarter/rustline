mod battery;
#[cfg(feature = "bench")]
mod bench;
mod build_context;
mod cli;
mod cpu;
mod disk;
mod git;
mod init;
mod logging;
mod memory;
mod plugin_cmd;
mod theme_cmd;
mod tmux_conf;
mod toggles;

use std::env;
use std::path::PathBuf;

use build_context::{build_region_context, build_window_context};
use clap::{CommandFactory, Parser};
use cli::{Cli, Command, Render};
use rustline_core::{
    Config, Direction, Registry, Theme, ThemeConfig, builtin_theme, render_named_region,
    render_window, tmux_to_ansi,
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

/// The rustline config base dir: `$XDG_CONFIG_HOME/rustline`, falling back to
/// `$HOME/.config/rustline`.
fn config_base() -> PathBuf {
    let base = env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env::var("HOME").unwrap_or_default()).join(".config"));
    base.join("rustline")
}

/// Resolve the config file path: `$XDG_CONFIG_HOME/rustline/config.toml`,
/// falling back to `$HOME/.config/rustline/config.toml` when unset.
fn config_path() -> PathBuf {
    config_base().join("config.toml")
}

/// Resolve the themes dir: `$XDG_CONFIG_HOME/rustline/themes` (fallback
/// `~/.config/rustline/themes`), parallel to `config_path`.
fn themes_dir() -> PathBuf {
    config_base().join("themes")
}

/// The user's tmux config file: `$HOME/.tmux.conf`.
fn tmux_conf_path() -> PathBuf {
    PathBuf::from(env::var("HOME").unwrap_or_default()).join(".tmux.conf")
}

/// Resolve the absolute binary path baked into the tmux block's `#(...)`
/// calls (see `tmux_conf::init_block`'s doc comment for why a bare `rustline`
/// isn't safe: the tmux server's `/bin/sh` PATH may not include wherever the
/// binary is installed). `--binary` wins if given; otherwise
/// `std::env::current_exe()`, falling back to the bare name `"rustline"` on
/// error rather than failing `init` outright — that's merely as fragile as
/// the pre-W6 behavior, never worse.
fn resolve_binary(flag: Option<&str>) -> String {
    if let Some(b) = flag {
        return b.to_string();
    }
    match env::current_exe() {
        Ok(path) => path.to_string_lossy().into_owned(),
        Err(e) => {
            tracing::warn!("could not resolve current_exe: {e}; using bare \"rustline\"");
            "rustline".to_string()
        }
    }
}

/// Resolve a base-theme name to a full `Theme`: a themes-dir `*.toml` file wins
/// over a same-named built-in (so a user file can shadow/override a built-in).
pub(crate) fn resolve_base_theme(name: &str) -> Option<Theme> {
    let file = themes_dir().join(format!("{name}.toml"));
    if let Ok(text) = std::fs::read_to_string(&file) {
        match toml::from_str::<ThemeConfig>(&text) {
            Ok(tc) => {
                let mut t = Theme::default();
                tc.apply_to(&mut t);
                return Some(t);
            }
            Err(e) => tracing::warn!("invalid theme file {}: {e}", file.display()),
        }
    }
    builtin_theme(name)
}

/// Resolve the effective theme: default → base (file-first, then built-in) →
/// inline `[theme]` overrides. An unresolvable base warns and falls back.
fn resolve_theme(cfg: &Config) -> Theme {
    let base = match cfg.theme.base.as_deref() {
        Some(name) => resolve_base_theme(name).unwrap_or_else(|| {
            tracing::warn!("unknown theme base {name:?}; using default");
            Theme::default()
        }),
        None => Theme::default(),
    };
    cfg.to_theme_over(base)
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
    let theme = resolve_theme(&cfg);

    match cli.command {
        Command::Render(Render::Left(args)) => {
            let plugin_dir = resolve_plugin_dir(args.plugin_dir.as_deref(), &cfg);
            let mut registry = Registry::with_builtins(&cfg);
            rustline_wasm::register_plugins(&mut registry, &cfg, &plugin_dir, &cfg.layout.left);
            let ctx =
                build_region_context(&args, &cfg.layout.left, &theme, &cfg.widgets.disk.mount);
            let out =
                render_named_region(Direction::Left, &cfg.layout.left, &ctx, &registry, &theme);
            emit(&out, args.preview);
        }
        Command::Render(Render::Right(args)) => {
            let plugin_dir = resolve_plugin_dir(args.plugin_dir.as_deref(), &cfg);
            let mut registry = Registry::with_builtins(&cfg);
            rustline_wasm::register_plugins(&mut registry, &cfg, &plugin_dir, &cfg.layout.right);
            let ctx =
                build_region_context(&args, &cfg.layout.right, &theme, &cfg.widgets.disk.mount);
            let out =
                render_named_region(Direction::Right, &cfg.layout.right, &ctx, &registry, &theme);
            emit(&out, args.preview);
        }
        Command::Render(Render::Window(args)) => {
            // Windows don't run plugins in v1: builtins only.
            let registry = Registry::with_builtins(&cfg);
            let ctx = build_window_context(&args);
            emit(&render_window(&ctx, &registry, &theme), args.preview);
        }
        Command::Init(args) => {
            let binary = resolve_binary(args.binary.as_deref());
            init::run(
                &args,
                &config_path(),
                &themes_dir(),
                &tmux_conf_path(),
                &theme,
                &binary,
            );
        }
        Command::PrintConfig => match toml::to_string_pretty(&cfg) {
            Ok(s) => print!("{s}"),
            Err(error) => {
                eprintln!("failed to serialize config: {error}");
                std::process::exit(1);
            }
        },
        Command::Plugin(cmd) => plugin_cmd::run(cmd, &config_path()),
        Command::Theme(cmd) => theme_cmd::run(cmd, &config_path(), &themes_dir()),
        Command::Click(args) => run_click(&args),
        Command::Completions { shell } => {
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "rustline",
                &mut std::io::stdout(),
            );
        }
        #[cfg(feature = "bench")]
        Command::Bench(args) => bench::run(&args, &cfg),
    }
}
