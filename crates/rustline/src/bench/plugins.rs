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

use std::path::Path;

use rustline_core::{Config, Registry};
use rustline_wasm::{CapabilityCtx, CompileCache, build_plugin_with_cache, state_root};

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
///
/// `plugin_dir` is the already-resolved discovery directory (see
/// `crate::resolve_plugin_dir`), resolved by the caller *before* any
/// `--state-dir` env override so plugin discovery never follows `--state-dir`
/// — only the state/cache root (`rustline_wasm::state_root()`, used inside
/// `register_plugins`) does.
pub fn bench_plugins(
    cfg: &Config,
    plugin_dir: &Path,
    args: &BenchArgs,
    iters: usize,
    real_iters: usize,
) -> Group {
    let stems = discover_wasm_stems(plugin_dir);
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
    rustline_wasm::register_plugins(&mut registry, cfg, plugin_dir, &stems);
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

/// Time a full fresh `build_plugin` per plugin, wasmtime compile cache OFF vs
/// ON — the Cranelift-compile cost W43 targets, which the preserved-state pass
/// above does NOT isolate (it clones one already-compiled `Arc<Plugin>`). Each
/// row instantiates from raw bytes every iteration:
///
/// - **cache OFF** (`with_cache_disabled`): every build is a full Cranelift
///   compile — the cost each cold `rustline render` process pays today.
/// - **cache ON**: warmed first (the warmup build compiles + writes the on-disk
///   artifact), so the timed builds measure the warm-cache *deserialize* path —
///   what a cold spawn pays once the cache is populated.
///
/// The delta between the two is the per-plugin cold-start compile saving.
pub fn bench_plugin_builds(cfg: &Config, plugin_dir: &Path, real_iters: usize) -> Group {
    let stems = discover_wasm_stems(plugin_dir);
    if stems.is_empty() {
        return Group {
            title: "Plugins — build_plugin compile cache".into(),
            note: Some(format!("no *.wasm in {} — skipped", plugin_dir.display())),
            rows: Vec::new(),
        };
    }

    let root = state_root();
    let mut rows = Vec::new();
    for stem in &stems {
        let Ok(bytes) = std::fs::read(plugin_dir.join(format!("{stem}.wasm"))) else {
            continue;
        };
        let pc = cfg.plugins.get(stem).cloned().unwrap_or_default();
        let build = |cache: CompileCache| {
            let ctx = CapabilityCtx::from_config(stem, &pc, root.clone());
            let _ = build_plugin_with_cache(&bytes, ctx, cache);
        };

        // Cache OFF: no warmup needed — every build is a full compile anyway.
        let off = measure(1, real_iters, || build(CompileCache::Disabled));
        rows.push(Row {
            label: format!("{stem} (build_plugin: cache OFF — full Cranelift compile)"),
            stats: summarize(&off),
        });
        // Cache ON: the warmup build populates the on-disk cache so the timed
        // builds measure the deserialize path.
        let on = measure(1, real_iters, || build(CompileCache::Enabled));
        rows.push(Row {
            label: format!("{stem} (build_plugin: cache ON — warm deserialize)"),
            stats: summarize(&on),
        });
    }

    Group {
        title: "Plugins — build_plugin compile cache (cache OFF vs ON)".into(),
        note: None,
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bench_args_with_state_dir(state_dir: Option<&str>) -> BenchArgs {
        BenchArgs {
            only: "plugins".into(),
            iters: 1,
            real_iters: 1,
            warmup: 0,
            cold: false,
            format: "table".into(),
            output: None,
            plugin_dir: None,
            state_dir: state_dir.map(Into::into),
        }
    }

    #[test]
    fn empty_plugin_dir_is_skipped_with_note() {
        let tmp = tempfile::tempdir().unwrap();
        let args = bench_args_with_state_dir(None);
        let g = bench_plugins(&Config::default(), tmp.path(), &args, 1, 1);
        assert!(g.rows.is_empty());
        assert!(g.note.as_deref().unwrap().contains("no *.wasm"));
    }

    /// `bench_plugins` takes the discovery dir as an explicit parameter, not
    /// derived from `args.state_dir` — so discovery finds a plugin dropped in
    /// the passed dir even when `args.state_dir` points elsewhere entirely.
    /// This is the regression test for the bug where `--state-dir`'s
    /// `XDG_DATA_HOME` override leaked into plugin discovery.
    #[test]
    fn discovery_uses_passed_plugin_dir_independent_of_state_dir() {
        let plugin_tmp = tempfile::tempdir().unwrap();
        let state_tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            plugin_tmp.path().join("foo.wasm"),
            b"not a real wasm module",
        )
        .unwrap();

        let args = bench_args_with_state_dir(Some(state_tmp.path().to_str().unwrap()));
        let g = bench_plugins(&Config::default(), plugin_tmp.path(), &args, 1, 1);

        assert!(
            g.note.is_none(),
            "expected discovery to find foo.wasm in the passed plugin dir, got skip note: {:?}",
            g.note
        );
        assert!(
            g.rows.iter().any(|r| r.label.starts_with("foo")),
            "expected a row for the discovered `foo` plugin, got rows: {:?}",
            g.rows.iter().map(|r| &r.label).collect::<Vec<_>>()
        );
    }
}
