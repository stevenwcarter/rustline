//! Plugin timing against real, preserved host state so cached fast-paths are
//! measured honestly (an empty state dir would make every render a cache miss).
//! Three measurements per discovered plugin:
//!
//! - first call: fresh instantiate + first render (cold if the cache is stale)
//! - per-tick: fresh instantiate + render vs the WARM disk cache (the true
//!   cost per real `rustline render`, which is a new process)
//! - warm: one instance reused, render only (future-daemon cost)
//!
//! `--cold` additionally clears the plugin's state dir to force a genuine miss.

use std::path::{Path, PathBuf};

use rustline_core::{Config, Registry};

use super::fixture::fabricated_context;
use super::harness::{Group, Row, measure, summarize};
use crate::cli::BenchArgs;

/// The `*.wasm` filename stems in `dir` (empty if the dir is missing).
fn discover_wasm_stems(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension().and_then(|x| x.to_str()) == Some("wasm"))
                .then(|| p.file_stem().and_then(|s| s.to_str()).map(str::to_string))
                .flatten()
        })
        .collect()
}

/// Bench each discovered plugin. Skips with a note when none are present.
pub fn bench_plugins(cfg: &Config, args: &BenchArgs, iters: usize, real_iters: usize) -> Group {
    let plugin_dir: PathBuf = crate::resolve_plugin_dir(args.plugin_dir.as_deref(), cfg);
    let stems = discover_wasm_stems(&plugin_dir);
    if stems.is_empty() {
        return Group {
            title: "Plugins".into(),
            note: Some(format!(
                "no *.wasm in {} — skipped (build one with `just build-weather`)",
                plugin_dir.display()
            )),
            rows: Vec::new(),
        };
    }

    let mut registry = Registry::new();
    rustline_wasm::register_plugins(&mut registry, cfg, &plugin_dir, &stems);
    let ctx = fabricated_context();
    let mut rows = Vec::new();

    for stem in &stems {
        if !registry.contains(stem) {
            rows.push(Row {
                label: format!("{stem} (failed to instantiate — skipped)"),
                stats: summarize(&[]),
            });
            continue;
        }

        // Optional cold-start: clear the plugin's state dir to force a miss.
        if args.cold {
            let _ = std::fs::remove_dir_all(rustline_wasm::state_root().join(stem));
            let first = measure(0, 1, || {
                if let Some(w) = registry.build(stem) {
                    let _ = w.render(&ctx);
                }
            });
            rows.push(Row {
                label: format!("{stem} (cold-start: cache cleared, may hit network)"),
                stats: summarize(&first),
            });
        }

        // Warm the disk cache before the timed passes.
        if let Some(w) = registry.build(stem) {
            for _ in 0..2 {
                let _ = w.render(&ctx);
            }
        }

        // per-tick: fresh instantiate + render each iteration, warm cache.
        let per_tick = measure(0, real_iters, || {
            if let Some(w) = registry.build(stem) {
                let _ = w.render(&ctx);
            }
        });
        rows.push(Row {
            label: format!("{stem} (per-tick: instantiate + render, warm cache)"),
            stats: summarize(&per_tick),
        });

        // warm in-process: reuse one instance.
        if let Some(w) = registry.build(stem) {
            let warm = measure(2, iters, || {
                let _ = w.render(&ctx);
            });
            rows.push(Row {
                label: format!("{stem} (warm in-process render)"),
                stats: summarize(&warm),
            });
        }
    }

    Group {
        title: "Plugins — real preserved state".into(),
        note: None,
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bench_args_with_plugin_dir(dir: &str) -> BenchArgs {
        BenchArgs {
            only: "plugins".into(),
            iters: 1,
            real_iters: 1,
            warmup: 0,
            cold: false,
            format: "table".into(),
            output: None,
            plugin_dir: Some(dir.into()),
            state_dir: None,
        }
    }

    #[test]
    fn empty_plugin_dir_is_skipped_with_note() {
        let tmp = tempfile::tempdir().unwrap();
        let args = bench_args_with_plugin_dir(tmp.path().to_str().unwrap());
        let g = bench_plugins(&Config::default(), &args, 1, 1);
        assert!(g.rows.is_empty());
        assert!(g.note.as_deref().unwrap().contains("no *.wasm"));
    }
}
