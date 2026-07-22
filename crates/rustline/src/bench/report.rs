//! Render `Group`s as tables via comfy-table (pretty preset for the terminal,
//! ASCII_MARKDOWN preset for `--format markdown`).

use std::time::Duration;

use comfy_table::{Table, presets};

use super::harness::Group;

/// Human-readable duration (ns/µs/ms/s).
pub fn fmt_dur(d: Duration) -> String {
    let ns = d.as_nanos();
    if ns < 1_000 {
        format!("{ns} ns")
    } else if ns < 1_000_000 {
        format!("{:.2} µs", ns as f64 / 1_000.0)
    } else if ns < 1_000_000_000 {
        format!("{:.2} ms", ns as f64 / 1_000_000.0)
    } else {
        format!("{:.3} s", ns as f64 / 1_000_000_000.0)
    }
}

/// Render all groups to a single string.
pub fn render_report(groups: &[Group], markdown: bool) -> String {
    let mut out = String::new();
    for g in groups {
        out.push('\n');
        out.push_str(&g.title);
        out.push('\n');
        if let Some(note) = &g.note {
            out.push_str("  ");
            out.push_str(note);
            out.push('\n');
        }
        if g.rows.is_empty() {
            continue;
        }
        let mut table = Table::new();
        table.load_preset(if markdown {
            presets::ASCII_MARKDOWN
        } else {
            presets::UTF8_FULL_CONDENSED
        });
        table.set_header(["pass", "n", "min", "median", "mean", "p95", "max"]);
        for row in &g.rows {
            table.add_row([
                row.label.clone(),
                row.stats.n.to_string(),
                fmt_dur(row.stats.min),
                fmt_dur(row.stats.median),
                fmt_dur(row.stats.mean),
                fmt_dur(row.stats.p95),
                fmt_dur(row.stats.max),
            ]);
        }
        out.push_str(&table.to_string());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::harness::{Row, summarize};
    use super::*;

    fn group() -> Group {
        Group {
            title: "T".into(),
            note: None,
            rows: vec![Row {
                label: "cpu".into(),
                stats: summarize(&[Duration::from_millis(120)]),
            }],
        }
    }

    #[test]
    fn fmt_dur_units() {
        assert_eq!(fmt_dur(Duration::from_nanos(500)), "500 ns");
        assert!(fmt_dur(Duration::from_micros(3)).contains("µs"));
        assert!(fmt_dur(Duration::from_millis(120)).contains("ms"));
        assert!(fmt_dur(Duration::from_secs(2)).contains('s'));
    }

    #[test]
    fn report_contains_label_and_value() {
        let out = render_report(&[group()], false);
        assert!(out.contains("cpu"));
        assert!(out.contains("120"));
    }

    #[test]
    fn markdown_preset_uses_pipes() {
        let out = render_report(&[group()], true);
        assert!(out.contains('|'));
    }
}
