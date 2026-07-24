//! Daemon vs in-process render timing: quantifies the win a warm, persistent
//! `rustline daemon run` (W48) gives over the default per-invocation render —
//! most visible on a plugin-heavy layout, since the daemon keeps WASM plugins
//! instantiated across renders while an in-process render (absent a daemon)
//! re-instantiates them every single call.
//!
//! Gated on a *reachable* daemon (`crate::daemon::status`); this pass never
//! spawns one itself. With no daemon reachable it returns a single
//! informational skip [`Group`] rather than measuring nothing.

use std::path::Path;

use rustline_core::{Config, Direction, Registry, render_named_region};

use super::harness::{Group, Row, Stats, measure, summarize};
use super::report::fmt_dur;
use crate::build_context::build_region_context;
use crate::cli::RegionArgs;
use crate::daemon_client;
use crate::daemon_proto::{RegionKind, RenderArgsWire};

/// Time both regions' in-process (cold) vs daemon (warm) renders, when a
/// daemon is reachable at the default socket. Skips with an informational
/// note otherwise — never tries to start a daemon.
pub fn bench_daemon(cfg: &Config, plugin_dir: &Path, real_iters: usize, warmup: usize) -> Group {
    bench_daemon_at(
        cfg,
        plugin_dir,
        &daemon_client::daemon_socket_path(),
        real_iters,
        warmup,
    )
}

/// [`bench_daemon`] against an explicit socket path — the seam a test uses to
/// point at a fake in-thread daemon, or a guaranteed-absent path to exercise
/// the skip branch deterministically instead of depending on whether the
/// host happens to have a real daemon reachable at the default socket.
fn bench_daemon_at(
    cfg: &Config,
    plugin_dir: &Path,
    sock: &Path,
    real_iters: usize,
    warmup: usize,
) -> Group {
    if !crate::daemon::status_at(sock) {
        return Group {
            title: "Daemon vs in-process".into(),
            note: Some(
                "daemon: not running — start `rustline daemon run` (with plugins in \
                 your layout) to benchmark the warm-render path"
                    .into(),
            ),
            rows: Vec::new(),
        };
    }

    let theme = cfg.to_theme();
    let overrides = cfg.color_overrides();
    let mut rows = Vec::new();
    let mut summary_lines = Vec::new();

    for (label, dir, layout, kind) in [
        ("left", Direction::Left, &cfg.layout.left, RegionKind::Left),
        (
            "right",
            Direction::Right,
            &cfg.layout.right,
            RegionKind::Right,
        ),
    ] {
        // In-process, cold: a fresh registry + fresh `register_plugins` every
        // call — the same sequence `main`'s `Render::Left`/`Right` arms run
        // on a daemon miss, so any WASM plugin in `layout` pays full
        // instantiation cost on every sample (unlike `render_passes`'s real
        // pass, which never registers plugins at all and so can't show this
        // win).
        let in_process = summarize(&measure(warmup, real_iters, || {
            let mut registry = Registry::with_builtins(cfg);
            rustline_wasm::register_plugins(&mut registry, cfg, plugin_dir, layout);
            let ctx = build_region_context(&RegionArgs::default(), layout, &theme, cfg);
            let _ = render_named_region(dir, layout, &ctx, &registry, &theme, &overrides);
        }));
        rows.push(Row {
            label: format!("{label} — in-process (cold, re-instantiates plugins)"),
            stats: in_process,
        });

        let daemon_stats = measure_daemon_region(sock, kind, real_iters, warmup);
        rows.push(Row {
            label: format!("{label} — daemon (warm round-trip)"),
            stats: daemon_stats,
        });

        summary_lines.push(speedup_line(label, in_process, daemon_stats));
    }

    Group {
        title: "Daemon vs in-process — plugin warm-render A/B".into(),
        note: Some(format!(
            "{}; plugin-heavy layouts benefit most (warm wasm) — a layout with few/no \
             plugin widgets may show little difference",
            summary_lines.join(" · ")
        )),
        rows,
    }
}

/// Time `real_iters` daemon round-trips of `region` against the daemon
/// listening at `sock` (preceded by `warmup` discarded round-trips). `sock`
/// is an explicit parameter rather than resolved internally via
/// `daemon_socket_path()`, so a test can point this at a fake in-thread
/// daemon — mirroring how `daemon_client` splits `try_render`/`try_render_at`.
fn measure_daemon_region(
    sock: &Path,
    region: RegionKind,
    real_iters: usize,
    warmup: usize,
) -> Stats {
    let samples = measure(warmup, real_iters, || {
        let _ = daemon_client::try_render_at(sock, region, RenderArgsWire::default());
    });
    summarize(&samples)
}

/// A `label: <ratio>x faster (median <in-process> vs <daemon>)` summary line
/// for the Group's `note`. Median-based (not mean) so one slow outlier
/// connection can't skew the headline ratio.
fn speedup_line(label: &str, in_process: Stats, daemon: Stats) -> String {
    let daemon_ns = daemon.median.as_nanos();
    if daemon_ns == 0 {
        return format!("{label}: daemon median rounded to 0ns — ratio not meaningful");
    }
    let ratio = in_process.median.as_nanos() as f64 / daemon_ns as f64;
    format!(
        "{label}: daemon {ratio:.1}x faster (median {} in-process vs {} daemon)",
        fmt_dur(in_process.median),
        fmt_dur(daemon.median)
    )
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;
    use crate::daemon_proto::{self, DaemonRequest, DaemonResponse};

    #[test]
    fn skip_group_when_no_daemon_reachable() {
        // A guaranteed-absent socket path, so this is deterministic
        // regardless of whether the host machine happens to have a real
        // daemon running elsewhere: `bench_daemon_at` must return the
        // informational skip Group (no rows, no measurement, no panic).
        let sock = Path::new("/nonexistent/rustline-bench-test.sock");
        let g = bench_daemon_at(
            &Config::default(),
            Path::new("/nonexistent/plugins"),
            sock,
            1,
            0,
        );
        assert!(g.rows.is_empty());
        assert!(g.note.as_deref().unwrap().contains("not running"));
    }

    #[test]
    fn measure_daemon_region_produces_a_measurement_against_a_fake_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("d.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let warmup = 1;
        let real_iters = 3;
        let total_requests = warmup + real_iters;

        let handle = thread::spawn(move || {
            for _ in 0..total_requests {
                let (mut stream, _) = listener.accept().unwrap();
                let _req: DaemonRequest = daemon_proto::read_frame(&mut stream).unwrap();
                daemon_proto::write_frame(&mut stream, &DaemonResponse::Markup("OK".into()))
                    .unwrap();
            }
        });

        let stats = measure_daemon_region(&sock, RegionKind::Right, real_iters, warmup);
        handle.join().unwrap();

        assert_eq!(stats.n, real_iters);
    }

    #[test]
    fn speedup_line_reports_ratio_and_both_medians() {
        let in_process = summarize(&[std::time::Duration::from_millis(40)]);
        let daemon = summarize(&[std::time::Duration::from_millis(4)]);
        let line = speedup_line("left", in_process, daemon);
        assert!(line.contains("left"));
        assert!(line.contains("10.0x"));
    }

    #[test]
    fn speedup_line_guards_zero_daemon_median() {
        let in_process = summarize(&[std::time::Duration::from_millis(1)]);
        let daemon = summarize(&[std::time::Duration::ZERO]);
        let line = speedup_line("right", in_process, daemon);
        assert!(line.contains("not meaningful"));
    }
}
