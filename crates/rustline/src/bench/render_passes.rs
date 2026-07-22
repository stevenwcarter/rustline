//! Timing passes over the render pipeline. Pure passes use `fabricated_context`
//! (no reads); the real-world region pass (Task 2) uses `build_region_context`.

use rustline_core::{Config, Registry};

use super::fixture::fabricated_context;
use super::harness::{Group, Row, measure, summarize};

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
}
