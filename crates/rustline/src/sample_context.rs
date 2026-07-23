//! The one shared synthetic-`Context` builder (W52). Three near-identical
//! hand-rolled fixtures used to live independently in `theme_cmd.rs` (theme
//! previews), `bench/fixture.rs` (the pure benchmark pass), and
//! `plugin_cmd.rs` (`plugin run`'s one-off harness render) — each spelling out
//! every `Context` field, so all three drifted separately as the type grew.
//! This module is now the single place that happens; each call site either
//! uses [`sample_context`] directly or thinly wraps it to tweak a field it
//! cares about (see each module's own `sample_context`/`fabricated_context`).
//!
//! `theme show`/`theme pick`'s rendered preview is the one LOAD-BEARING
//! consumer: its output must stay byte-identical to the pre-consolidation
//! behavior (pinned by `theme_cmd`'s
//! `sample_context_render_is_unchanged_by_consolidation` characterization
//! test), so every field theme_cmd's preview layout can actually observe
//! (the session/window/pane ids, `pane_current_path`, `home`, `hostname`,
//! `loadavg`, and the `battery`/`cpu`/`memory` peg-vs-healthy readings) keeps
//! theme_cmd's original pre-consolidation values verbatim. The remaining
//! superset fields (`interfaces`, `git`, `disk`, `uptime`, `media`, `window`)
//! are filled with representative synthetic data for the bench/plugin-run
//! consumers' richer needs — safe because none of those fields are read by
//! any widget in theme_cmd's preview layout (`cwd`, `cpu`, `memory`,
//! `battery`, `loadavg`, `datetime`, plus `pane_id`/`hostname` on the left).

use chrono::Local;
use rustline_core::{
    Battery, BatteryState, Context, CpuUsage, DiskInfo, GitInfo, MediaInfo, MemInfo, NetIface,
    WindowCtx,
};

/// A representative, fully-populated synthetic [`Context`]. `show_alerts`
/// pegs the `cpu`/`memory`/`battery`/`disk` readings past their default
/// warn+crit thresholds (tripping both alert-badge colors); `false` gives
/// healthy readings that trip none. The cpu/memory/battery peg and healthy
/// values are copied verbatim from `theme_cmd`'s pre-consolidation
/// `sample_context` (see the module doc above); `disk` is pegged/healthed the
/// same way for parity, and `loadavg` is held at one fixed value regardless
/// of `show_alerts` — the original never varied it (loadavg's own
/// `warn_load`/`crit_load` default to `0.0`, off), and changing it would
/// change the rendered preview text, breaking byte-identical output.
pub fn sample_context(show_alerts: bool) -> Context {
    let gib = 1024u64.pow(3);
    let (cpu_pct, mem_used_gib, mem_avail_gib, batt_pct, disk_used_gib, disk_avail_gib) =
        if show_alerts {
            (96.0, 14, 2, 15, 500, 12)
        } else {
            (12.0, 6, 10, 82, 200, 300)
        };
    Context {
        session_name: "0".into(),
        window_index: "1".into(),
        pane_index: "0".into(),
        pane_current_path: "/home/steve/src/rustline".into(),
        home: "/home/steve".into(),
        hostname: "scadrial".into(),
        loadavg: Some([0.42, 0.31, 0.30]),
        now: Local::now(),
        window: Some(WindowCtx {
            index: "1".into(),
            name: "editor".into(),
            flags: "*".into(),
            is_current: true,
        }),
        interfaces: vec![
            NetIface {
                name: "eth0".into(),
                ipv4: "192.168.1.42".parse().expect("valid ipv4"),
            },
            NetIface {
                name: "tailscale0".into(),
                ipv4: "100.101.4.7".parse().expect("valid ipv4"),
            },
        ],
        battery: Some(Battery {
            percent: batt_pct,
            state: BatteryState::Discharging,
        }),
        cpu: Some(CpuUsage { percent: cpu_pct }),
        memory: Some(MemInfo {
            total_bytes: 16 * gib,
            used_bytes: mem_used_gib * gib,
            available_bytes: mem_avail_gib * gib,
        }),
        git: Some(GitInfo {
            branch: "main".into(),
            ahead: 1,
            behind: 0,
            staged: 1,
            unstaged: 2,
        }),
        disk: Some(DiskInfo {
            total_bytes: 512 * gib,
            used_bytes: disk_used_gib * gib,
            available_bytes: disk_avail_gib * gib,
        }),
        os: "linux".into(),
        arch: "x86_64".into(),
        uptime: Some(86_400 * 3 + 3600 * 4), // 3d 4h
        media: Some(MediaInfo {
            artist: "Radiohead".into(),
            title: "Karma Police".into(),
            status: "Playing".into(),
        }),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use rustline_core::widgets::{BatteryWidget, CpuWidget, DiskWidget, MemoryWidget};
    use rustline_core::{Color, ThemeColors, Widget};

    use super::sample_context;

    /// Matches the alert colors `theme_cmd`'s own badge test uses, so a badge
    /// showing up is unambiguous.
    fn colors() -> ThemeColors {
        ThemeColors {
            warning: Color::Indexed(214),
            error: Color::Indexed(196),
            bar_bg: Color::Indexed(234),
            ..ThemeColors::default()
        }
    }

    /// The default `[widgets.*]` warn/crit thresholds (see each widget's own
    /// `Default` in `rustline-core::config`), constructed directly here so
    /// the test doesn't depend on `Config::default()`'s wider surface.
    fn cpu_widget() -> CpuWidget {
        CpuWidget {
            format: "{percent}".into(),
            alt_format: String::new(),
            down_format: String::new(),
            bar_width: 8,
            warn_percent: 80.0,
            crit_percent: 95.0,
            icon: None,
        }
    }

    fn memory_widget() -> MemoryWidget {
        MemoryWidget {
            format: "{percent}".into(),
            alt_format: String::new(),
            down_format: String::new(),
            bar_width: 8,
            warn_percent: 80.0,
            crit_percent: 92.0,
            icon: None,
        }
    }

    fn battery_widget() -> BatteryWidget {
        BatteryWidget {
            format: "{percent}".into(),
            alt_format: String::new(),
            down_format: String::new(),
            warn_percent: 20.0,
            crit_percent: 10.0,
            icon: None,
        }
    }

    fn disk_widget() -> DiskWidget {
        DiskWidget {
            format: "{percent}".into(),
            alt_format: String::new(),
            down_format: String::new(),
            mount: "/".into(),
            bar_width: 8,
            warn_percent: 85.0,
            crit_percent: 95.0,
        }
    }

    #[test]
    fn show_alerts_true_trips_warning_and_error_badges() {
        let mut ctx = sample_context(true);
        ctx.colors = colors();

        let cpu = cpu_widget().render(&ctx);
        let memory = memory_widget().render(&ctx);
        let battery = battery_widget().render(&ctx);
        let disk = disk_widget().render(&ctx);

        // cpu (96%) and disk (97.7%) cross crit; memory (87.5%) and battery
        // (15%, discharging) cross warn but not crit — so both badge colors
        // show up across the four, matching theme_cmd's own preview test.
        let bgs: Vec<Option<Color>> = [&cpu, &memory, &battery, &disk]
            .iter()
            .map(|segs| segs[0].style.bg.clone())
            .collect();
        assert!(
            bgs.contains(&Some(Color::Indexed(196))),
            "expected an error badge among {bgs:?}"
        );
        assert!(
            bgs.contains(&Some(Color::Indexed(214))),
            "expected a warning badge among {bgs:?}"
        );
    }

    #[test]
    fn show_alerts_false_is_healthy() {
        let mut ctx = sample_context(false);
        ctx.colors = colors();

        for segs in [
            cpu_widget().render(&ctx),
            memory_widget().render(&ctx),
            battery_widget().render(&ctx),
            disk_widget().render(&ctx),
        ] {
            assert_eq!(
                segs[0].style,
                rustline_core::Style::default(),
                "no widget should carry an alert badge when healthy"
            );
        }
    }

    #[test]
    fn every_option_field_is_populated() {
        // Superset coverage: both variants fill every `Option` field so a
        // consumer (bench's pure pass, `plugin run`'s harness) never
        // degrades to a widget's `down_format` path for lack of data.
        for show_alerts in [true, false] {
            let ctx = sample_context(show_alerts);
            assert!(ctx.loadavg.is_some());
            assert!(ctx.window.is_some());
            assert!(!ctx.interfaces.is_empty());
            assert!(ctx.battery.is_some());
            assert!(ctx.cpu.is_some());
            assert!(ctx.memory.is_some());
            assert!(ctx.git.is_some());
            assert!(ctx.disk.is_some());
            assert!(ctx.uptime.is_some());
            assert!(ctx.media.is_some());
        }
    }
}
