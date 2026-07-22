//! Timing passes over the render pipeline. Pure passes use `fabricated_context`
//! (no reads); the real-world region pass (Task 2) uses `build_region_context`.

use rustline_core::{Config, Direction, Registry, render_named_region, render_window};

use super::fixture::fabricated_context;
use super::harness::{Group, Row, measure, summarize};
use crate::build_context::{build_region_context, build_window_context};
use crate::cli::{RegionArgs, WindowArgs};

/// The built-in widget names, benched individually. Kept explicit so a missing
/// registration is caught by the completeness test.
pub const BUILTIN_WIDGETS: [&str; 11] = [
    "pane_id",
    "hostname",
    "windows",
    "cwd",
    "loadavg",
    "datetime",
    "lan_ip",
    "tailscale_ip",
    "battery",
    "cpu",
    "memory",
];

/// Time each built-in widget's `render` in isolation on the fabricated Context.
pub fn bench_widgets(cfg: &Config, iters: usize, warmup: usize) -> Group {
    let registry = Registry::with_builtins(cfg);
    let ctx = fabricated_context();
    let mut rows = Vec::new();
    for name in BUILTIN_WIDGETS {
        let Some(widget) = registry.build(name) else {
            continue;
        };
        let samples = measure(warmup, iters, || {
            let _ = widget.render(&ctx);
        });
        rows.push(Row {
            label: name.to_string(),
            stats: summarize(&samples),
        });
    }
    Group {
        title: "Widget render — pure, isolated".into(),
        note: None,
        rows,
    }
}

/// Pure render of each region on the fabricated Context (no OS reads).
pub fn bench_regions_pure(cfg: &Config, iters: usize, warmup: usize) -> Group {
    let registry = Registry::with_builtins(cfg);
    let theme = cfg.to_theme();
    let ctx = fabricated_context();
    let left = cfg.layout.left.clone();
    let right = cfg.layout.right.clone();
    let row = |label: &str, f: &mut dyn FnMut()| Row {
        label: label.to_string(),
        stats: summarize(&measure(warmup, iters, f)),
    };
    let mut rows = Vec::new();
    rows.push(row("left", &mut || {
        let _ = render_named_region(Direction::Left, &left, &ctx, &registry, &theme);
    }));
    rows.push(row("right", &mut || {
        let _ = render_named_region(Direction::Right, &right, &ctx, &registry, &theme);
    }));
    rows.push(row("window", &mut || {
        let _ = render_window(&ctx, &registry, &theme);
    }));
    rows.push(row("full-bar", &mut || {
        let _ = render_named_region(Direction::Left, &left, &ctx, &registry, &theme);
        let _ = render_named_region(Direction::Right, &right, &ctx, &registry, &theme);
        let _ = render_window(&ctx, &registry, &theme);
    }));
    Group {
        title: "Region render — pure (fabricated Context, no reads)".into(),
        note: None,
        rows,
    }
}

/// Real-world render of each region: real `build_region_context` (all OS reads,
/// incl. read_cpu's ~120 ms when the layout names `cpu`) + render.
pub fn bench_regions_real(cfg: &Config, real_iters: usize, warmup: usize) -> Group {
    let registry = Registry::with_builtins(cfg);
    let theme = cfg.to_theme();
    let left = cfg.layout.left.clone();
    let right = cfg.layout.right.clone();
    let region_args = RegionArgs::default();
    let win_args = WindowArgs {
        current: true,
        index: "1".into(),
        name: "editor".into(),
        flags: "*".into(),
        preview: false,
    };
    let mut rows = Vec::new();
    rows.push(Row {
        label: "left".into(),
        stats: summarize(&measure(warmup, real_iters, || {
            let ctx = build_region_context(&region_args, &left);
            let _ = render_named_region(Direction::Left, &left, &ctx, &registry, &theme);
        })),
    });
    rows.push(Row {
        label: "right".into(),
        stats: summarize(&measure(warmup, real_iters, || {
            let ctx = build_region_context(&region_args, &right);
            let _ = render_named_region(Direction::Right, &right, &ctx, &registry, &theme);
        })),
    });
    rows.push(Row {
        label: "window".into(),
        stats: summarize(&measure(warmup, real_iters, || {
            let ctx = build_window_context(&win_args);
            let _ = render_window(&ctx, &registry, &theme);
        })),
    });
    Group {
        title: "Region end-to-end — real-world (build_context + render)".into(),
        note: Some("`right` includes read_cpu's ~120ms sample window".into()),
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_renders_nonempty_on_fixture() {
        // Load-bearing: pins invariant #1's usefulness. If any widget degraded
        // to its empty/down_format path on the fixture, the pure numbers would
        // be measuring the wrong branch — so assert none does.
        let cfg = Config::default();
        let registry = Registry::with_builtins(&cfg);
        let ctx = fabricated_context();
        for name in BUILTIN_WIDGETS {
            let widget = registry
                .build(name)
                .unwrap_or_else(|| panic!("built-in {name} not registered"));
            assert!(
                !widget.render(&ctx).is_empty(),
                "widget {name} rendered empty on the fabricated context"
            );
        }
    }

    #[test]
    fn bench_widgets_has_a_row_per_builtin() {
        let g = bench_widgets(&Config::default(), 2, 0);
        assert_eq!(g.rows.len(), BUILTIN_WIDGETS.len());
        assert!(g.rows.iter().all(|r| r.stats.n == 2));
        assert!(g.rows.iter().any(|r| r.label == "cpu"));
    }

    #[test]
    fn regions_pure_has_expected_rows() {
        let g = bench_regions_pure(&Config::default(), 2, 0);
        let labels: Vec<&str> = g.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["left", "right", "window", "full-bar"]);
        assert!(g.rows.iter().all(|r| r.stats.n == 2));
    }

    #[test]
    fn regions_real_has_region_rows() {
        // real_iters=1 keeps this quick despite read_cpu's ~120ms.
        let g = bench_regions_real(&Config::default(), 1, 0);
        let labels: Vec<&str> = g.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["left", "right", "window"]);
    }
}
