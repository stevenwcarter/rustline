//! `rustline bench`: time the render pipeline and print tables. Feature-gated.

mod fixture;
mod harness;
mod render_passes;
mod report;

use rustline_core::Config;

use crate::cli::BenchArgs;
use harness::Group;

/// Entry point for `rustline bench`.
pub fn run(args: &BenchArgs, cfg: &Config) {
    // `--state-dir` relocates state_root()/data_root() (both key off
    // XDG_DATA_HOME). Set before any read/plugin instantiation.
    if let Some(dir) = &args.state_dir {
        // SAFETY: set once at the very top of the bench command, before any
        // reads or wasm host threads are spawned; the process is single-threaded
        // here. This is a bench-only tool.
        unsafe {
            std::env::set_var("XDG_DATA_HOME", dir);
        }
    }

    let only = args.only.as_str();
    let want = |g: &str| only == "all" || only == g;
    let mut groups: Vec<Group> = Vec::new();

    if want("widgets") {
        groups.push(render_passes::bench_widgets(cfg, args.iters, args.warmup));
    }

    let markdown = args.format == "markdown";
    let text = report::render_report(&groups, markdown);
    match &args.output {
        Some(path) => match std::fs::write(path, &text) {
            Ok(()) => println!("wrote report to {path}"),
            Err(error) => {
                eprintln!("failed to write {path}: {error}");
                print!("{text}");
            }
        },
        None => print!("{text}"),
    }
}
