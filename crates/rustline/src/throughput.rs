//! Network throughput read surface, isolated at the `Context`-build edge.
//!
//! A throughput rate is a delta, not a single-shot reading, so — mirroring
//! `cpu.rs`'s persisted-snapshot pattern — `read_throughput` diffs the current
//! `/proc/net/dev` counters against a prior sample persisted via
//! `sample_store`, rather than sleeping across two live reads the way `cpu.rs`
//! falls back to on a cold cache. The pure `parse_proc_net_dev`/
//! `throughput_rate`/`aggregate` helpers are `#[cfg(any(target_os = "linux",
//! test))]`-compiled (same as `cpu.rs`'s `parse_proc_stat`/`busy_percent`), so
//! they're unit-tested on any dev box even though only Linux ever calls them
//! for real; every other platform's `read_throughput` degrades straight to
//! `None`.

use std::path::Path;

use rustline_core::Throughput;

/// State-file name under the state dir the prior `(rx, tx, ts)` sample is
/// persisted at, via `sample_store`.
#[cfg(target_os = "linux")]
const SAMPLE_NAME: &str = "throughput-sample";

/// Read network throughput, or `None` on the first invocation (nothing yet to
/// diff against), an unsupported platform, or a read failure — never a
/// fabricated `0` rate (invariant #6). `interface` pins the read to a single
/// named interface; `None` aggregates every non-loopback interface. Called
/// once at Context-build time, only when the `throughput` widget is in the
/// active layout (see `build_context.rs`).
#[cfg(target_os = "linux")]
pub fn read_throughput(state_dir: &Path, interface: Option<&str>) -> Option<Throughput> {
    let text = std::fs::read_to_string("/proc/net/dev").ok()?;
    let (rx, tx) = aggregate(&parse_proc_net_dev(&text), interface)?;
    let now = crate::sample_store::now_unix_secs();

    let prev = crate::sample_store::read_sample(state_dir, SAMPLE_NAME)
        .as_deref()
        .and_then(parse_sample);
    // Always persist the current reading so the next invocation has a prior
    // sample to diff against, mirroring `cpu.rs`'s `store_snapshot`.
    crate::sample_store::write_sample(state_dir, SAMPLE_NAME, &serialize_sample(rx, tx, now));

    let (prev_rx, prev_tx, prev_ts) = prev?;
    let dt_secs = now as i64 - prev_ts as i64;
    Some(throughput_rate(
        (prev_rx, prev_tx),
        (rx, tx),
        dt_secs as f64,
    ))
}

#[cfg(not(target_os = "linux"))]
pub fn read_throughput(_state_dir: &Path, _interface: Option<&str>) -> Option<Throughput> {
    None
}

/// Parse `/proc/net/dev`'s contents into `(interface, rx_bytes, tx_bytes)`
/// triples, excluding the loopback interface (`lo`). A line missing a `:`
/// (the two header rows) or with too-few/non-numeric fields is skipped rather
/// than failing the whole parse.
#[cfg(any(target_os = "linux", test))]
fn parse_proc_net_dev(text: &str) -> Vec<(String, u64, u64)> {
    text.lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once(':')?;
            let name = name.trim();
            if name.is_empty() || name == "lo" {
                return None;
            }
            let mut fields = rest.split_whitespace();
            let rx: u64 = fields.next()?.parse().ok()?;
            // Receive has 8 fields (bytes packets errs drops fifo frame
            // compressed multicast) before Transmit's bytes begins; skipping
            // 7 more from here (indices 1..=7) lands on index 8 (tx bytes).
            let tx: u64 = fields.nth(7)?.parse().ok()?;
            Some((name.to_string(), rx, tx))
        })
        .collect()
}

/// Sum (or select) the interface entries `parse_proc_net_dev` returned.
/// `interface = Some(name)` selects just that interface's counters
/// (`parse_proc_net_dev` already excludes `lo`); `None` aggregates every
/// entry, mirroring the `lan_ip`/`tailscale_ip` widgets' auto-select-vs-pin
/// split. `None` when the requested interface isn't present, or the system
/// has no non-loopback interfaces at all — never a fabricated `0`.
#[cfg(any(target_os = "linux", test))]
fn aggregate(entries: &[(String, u64, u64)], interface: Option<&str>) -> Option<(u64, u64)> {
    match interface {
        Some(name) => entries
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, rx, tx)| (*rx, *tx)),
        None if entries.is_empty() => None,
        None => Some(entries.iter().fold((0u64, 0u64), |(rx, tx), (_, r, t)| {
            (rx.saturating_add(*r), tx.saturating_add(*t))
        })),
    }
}

/// Compute a [`Throughput`] from two `(rx, tx)` byte-counter samples taken
/// `dt_secs` apart. Each direction is independent: a counter reset (current <
/// previous — e.g. an interface reset/replug) or a non-positive interval
/// yields a zero rate for that direction rather than a huge wrapped number.
#[cfg(any(target_os = "linux", test))]
fn throughput_rate(prev: (u64, u64), cur: (u64, u64), dt_secs: f64) -> Throughput {
    let rate = |c: u64, p: u64| -> u64 {
        if dt_secs <= 0.0 {
            return 0;
        }
        c.checked_sub(p)
            .map_or(0, |delta| (delta as f64 / dt_secs) as u64)
    };
    Throughput {
        down_bytes_per_sec: rate(cur.0, prev.0),
        up_bytes_per_sec: rate(cur.1, prev.1),
    }
}

/// Serialize an `(rx, tx, ts)` sample to a single `rx tx ts` line (trailing
/// newline), the same plain-text convention as `cpu.rs`'s `serialize_snapshot`.
#[cfg(any(target_os = "linux", test))]
fn serialize_sample(rx: u64, tx: u64, ts: u64) -> String {
    format!("{rx} {tx} {ts}\n")
}

/// Parse a `rx tx ts` line back into a sample. Missing/short/non-numeric
/// content yields `None` (treated as absent -> first-run fallback), never a
/// panic. Extra trailing tokens are ignored.
#[cfg(any(target_os = "linux", test))]
fn parse_sample(text: &str) -> Option<(u64, u64, u64)> {
    let mut fields = text.lines().next()?.split_whitespace();
    let rx = fields.next()?.parse().ok()?;
    let tx = fields.next()?.parse().ok()?;
    let ts = fields.next()?.parse().ok()?;
    Some((rx, tx, ts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_net_dev_excluding_loopback() {
        let s = "Inter-|   Receive ...\n face |bytes ...\n    lo: 100 0 0 0 0 0 0 0 200 0 ...\n  eth0: 1000 5 0 0 0 0 0 0 2000 7 ...\n";
        let v = parse_proc_net_dev(s);
        assert_eq!(v, vec![("eth0".to_string(), 1000u64, 2000u64)]); // lo excluded, (rx,tx)
    }

    #[test]
    fn rate_from_two_samples() {
        // prev rx=1000 tx=2000 @ t=0 ; cur rx=3000 tx=6000 @ t=2s -> 1000/s down, 2000/s up
        let r = throughput_rate((1000, 2000), (3000, 6000), 2.0);
        assert_eq!(r.down_bytes_per_sec, 1000);
        assert_eq!(r.up_bytes_per_sec, 2000);
        // counter reset (cur < prev) -> zero, not a huge saturating number
        let z = throughput_rate((5000, 5000), (10, 10), 1.0);
        assert_eq!((z.down_bytes_per_sec, z.up_bytes_per_sec), (0, 0));
    }

    #[test]
    fn rate_nonpositive_interval_is_zero() {
        let same_instant = throughput_rate((100, 100), (200, 200), 0.0);
        assert_eq!(
            (
                same_instant.down_bytes_per_sec,
                same_instant.up_bytes_per_sec
            ),
            (0, 0)
        );
        let backward_clock = throughput_rate((100, 100), (200, 200), -5.0);
        assert_eq!(
            (
                backward_clock.down_bytes_per_sec,
                backward_clock.up_bytes_per_sec
            ),
            (0, 0)
        );
    }

    #[test]
    fn rate_reset_is_independent_per_direction() {
        // rx advanced normally, tx reset (replug on tx counter only): down is a
        // real rate, up degrades to zero rather than dragging the whole
        // reading to zero.
        let r = throughput_rate((1000, 5000), (3000, 10), 2.0);
        assert_eq!(r.down_bytes_per_sec, 1000);
        assert_eq!(r.up_bytes_per_sec, 0);
    }

    #[test]
    fn multiple_interfaces_excluding_loopback_all_parse() {
        let s = "Inter-|\n face |\n    lo: 1 2 0 0 0 0 0 0 3 4 0 0 0 0 0 0\n  eth0: 10 0 0 0 0 0 0 0 20 0 0 0 0 0 0 0\n  wlan0: 30 0 0 0 0 0 0 0 40 0 0 0 0 0 0 0\n";
        let v = parse_proc_net_dev(s);
        assert_eq!(
            v,
            vec![("eth0".to_string(), 10, 20), ("wlan0".to_string(), 30, 40),]
        );
    }

    #[test]
    fn header_only_lines_yield_no_entries() {
        let s = "Inter-|   Receive\n face |bytes\n";
        assert!(parse_proc_net_dev(s).is_empty());
    }

    #[test]
    fn aggregate_sums_all_non_loopback_by_default() {
        let entries = vec![("eth0".to_string(), 10, 20), ("wlan0".to_string(), 30, 40)];
        assert_eq!(aggregate(&entries, None), Some((40, 60)));
    }

    #[test]
    fn aggregate_selects_named_interface() {
        let entries = vec![("eth0".to_string(), 10, 20), ("wlan0".to_string(), 30, 40)];
        assert_eq!(aggregate(&entries, Some("wlan0")), Some((30, 40)));
        assert_eq!(aggregate(&entries, Some("nope")), None);
    }

    #[test]
    fn aggregate_no_interfaces_is_none() {
        assert_eq!(aggregate(&[], None), None);
        assert_eq!(aggregate(&[], Some("eth0")), None);
    }

    #[test]
    fn sample_serialize_parse_round_trips() {
        let text = serialize_sample(1234, 5678, 1_700_000_000);
        assert_eq!(parse_sample(&text), Some((1234, 5678, 1_700_000_000)));
    }

    #[test]
    fn parse_sample_is_total_over_corrupt_input() {
        for bad in ["", "   ", "\n", "1000", "1000 800", "x y z", "garbage"] {
            assert!(parse_sample(bad).is_none(), "expected None for {bad:?}");
        }
        // Extra trailing junk on the line is ignored, not fatal.
        assert_eq!(
            parse_sample("1000 800 42 leftover\n"),
            Some((1000, 800, 42))
        );
    }

    // Linux takes the state-dir-injecting path so this never touches the real
    // XDG state dir. Other platforms' `read_throughput` never touches disk at
    // all, so calling the public function directly is already hermetic there.
    #[test]
    #[cfg(target_os = "linux")]
    fn read_throughput_first_run_is_none_then_some_on_second_call() {
        let dir = tempfile::tempdir().unwrap();
        // First call: no prior sample -> None, but persists the current one.
        let first = read_throughput(dir.path(), None);
        assert!(first.is_none(), "first run has nothing to diff against");
        // Second call (immediately after): a prior sample now exists, so this
        // returns Some — the exact rate depends on the live host counters
        // advancing between calls, so only the Some/None shape is asserted.
        let second = read_throughput(dir.path(), None);
        assert!(second.is_some(), "second run has a prior sample to diff");
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn read_throughput_is_none_on_unsupported_platform() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_throughput(dir.path(), None).is_none());
    }
}
