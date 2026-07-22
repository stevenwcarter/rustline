//! Timing the individual OS reads that populate `Context`. The single extension
//! point for future slow reads: add the read here (and fill its field in
//! `fixture::fabricated_context` so the pure passes keep skipping it).

use super::harness::{Group, Row, measure, summarize};

/// Named thunks over each real read, timed in isolation.
pub fn source_registry() -> Vec<(&'static str, fn())> {
    vec![
        ("read_cpu", || {
            let _ = crate::cpu::read_cpu();
        }),
        ("read_memory", || {
            let _ = crate::memory::read_memory();
        }),
        ("read_battery", || {
            let _ = crate::battery::read_battery();
        }),
        ("read_loadavg", || {
            let _ = crate::build_context::read_loadavg();
        }),
        ("read_interfaces", || {
            let _ = crate::build_context::read_interfaces();
        }),
        ("read_toggles", || {
            let _ = crate::toggles::read_toggles();
        }),
    ]
}

/// Time each read in the source registry.
pub fn bench_sources(real_iters: usize, warmup: usize) -> Group {
    let rows = source_registry()
        .into_iter()
        .map(|(name, f)| Row {
            label: name.to_string(),
            stats: summarize(&measure(warmup, real_iters, f)),
        })
        .collect();
    Group {
        title: "Data sources — real reads, isolated".into(),
        note: Some("read_cpu sleeps ~120ms sampling /proc/stat".into()),
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lists_expected_reads() {
        let names: Vec<&str> = source_registry().iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            [
                "read_cpu",
                "read_memory",
                "read_battery",
                "read_loadavg",
                "read_interfaces",
                "read_toggles",
            ]
        );
    }

    #[test]
    fn bench_sources_rows_match_registry() {
        let g = bench_sources(1, 0);
        assert_eq!(g.rows.len(), 6);
        assert_eq!(g.rows[0].label, "read_cpu");
    }
}
