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

    if want("regions") {
        groups.push(render_passes::bench_regions_pure(
            cfg,
            args.iters,
            args.warmup,
        ));
        // Real passes: small fixed warmup (each `right` build pays ~120ms).
        groups.push(render_passes::bench_regions_real(cfg, args.real_iters, 2));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::BenchArgs;

    fn args(only: &str, out: &str) -> BenchArgs {
        BenchArgs {
            only: only.into(),
            iters: 2,
            real_iters: 1,
            warmup: 0,
            cold: false,
            format: "markdown".into(),
            output: Some(out.into()),
            plugin_dir: None,
            state_dir: None,
        }
    }

    #[test]
    fn run_writes_widget_report_to_output_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("report.md");
        run(&args("widgets", path.to_str().unwrap()), &Config::default());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("cpu"));
        assert!(content.contains('|')); // markdown table
    }
}
