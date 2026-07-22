//! Tiny timing harness: run a closure N times, summarize the sample durations.
//! No statistical rigor — a "rough sense" tool (see the bench spec).

use std::time::{Duration, Instant};

/// Summary statistics over a set of timing samples.
#[derive(Clone, Copy, Debug)]
pub struct Stats {
    pub n: usize,
    pub min: Duration,
    pub median: Duration,
    pub mean: Duration,
    pub p95: Duration,
    pub max: Duration,
}

/// One labelled result row.
#[derive(Clone)]
pub struct Row {
    pub label: String,
    pub stats: Stats,
}

/// A titled group of rows (one report table).
#[derive(Clone)]
pub struct Group {
    pub title: String,
    pub note: Option<String>,
    pub rows: Vec<Row>,
}

/// Summarize samples (min/median/mean/p95/max). Empty input → all-zero, `n = 0`.
pub fn summarize(samples: &[Duration]) -> Stats {
    if samples.is_empty() {
        return Stats {
            n: 0,
            min: Duration::ZERO,
            median: Duration::ZERO,
            mean: Duration::ZERO,
            p95: Duration::ZERO,
            max: Duration::ZERO,
        };
    }
    let mut nanos: Vec<u128> = samples.iter().map(Duration::as_nanos).collect();
    nanos.sort_unstable();
    let n = nanos.len();
    let sum: u128 = nanos.iter().sum();
    let mean = sum / n as u128;
    let median = if n % 2 == 1 {
        nanos[n / 2]
    } else {
        (nanos[n / 2 - 1] + nanos[n / 2]) / 2
    };
    // nearest-rank p95
    let idx = (((n as f64) * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(n - 1);
    let ns = |v: u128| Duration::from_nanos(v.min(u64::MAX as u128) as u64);
    Stats {
        n,
        min: ns(nanos[0]),
        median: ns(median),
        mean: ns(mean),
        p95: ns(nanos[idx]),
        max: ns(nanos[n - 1]),
    }
}

/// Run `f` `warmup` times (discarded), then `iters` times recording each
/// wall-clock duration. Returns exactly `iters` samples. The warmup phase is
/// where once-per-run expensive work (instantiation, a cache-miss fetch) is
/// amortized before timing starts.
pub fn measure(warmup: usize, iters: usize, mut f: impl FnMut()) -> Vec<Duration> {
    for _ in 0..warmup {
        f();
    }
    let mut samples = Vec::with_capacity(iters);
    for _ in 0..iters {
        let start = Instant::now();
        f();
        samples.push(start.elapsed());
    }
    samples
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(x: u64) -> Duration {
        Duration::from_millis(x)
    }

    #[test]
    fn summarize_sorted_odd() {
        let s = summarize(&[ms(10), ms(20), ms(30)]);
        assert_eq!(s.n, 3);
        assert_eq!(s.min, ms(10));
        assert_eq!(s.max, ms(30));
        assert_eq!(s.median, ms(20));
        assert_eq!(s.mean, ms(20));
        assert_eq!(s.p95, ms(30)); // ceil(0.95*3)=3 -> idx 2
    }

    #[test]
    fn summarize_even_median_and_unsorted() {
        assert_eq!(summarize(&[ms(10), ms(20), ms(30), ms(40)]).median, ms(25));
        let s = summarize(&[ms(30), ms(10), ms(20)]);
        assert_eq!(s.min, ms(10));
        assert_eq!(s.max, ms(30));
        assert_eq!(s.median, ms(20));
    }

    #[test]
    fn summarize_single_and_empty() {
        let s = summarize(&[ms(7)]);
        assert_eq!(
            (s.min, s.median, s.p95, s.max),
            (ms(7), ms(7), ms(7), ms(7))
        );
        let e = summarize(&[]);
        assert_eq!(e.n, 0);
        assert_eq!(e.max, Duration::ZERO);
    }

    #[test]
    fn measure_runs_warmup_plus_iters_returns_iters() {
        let mut count = 0u32;
        let samples = measure(3, 5, || count += 1);
        assert_eq!(count, 8);
        assert_eq!(samples.len(), 5);
    }
}
