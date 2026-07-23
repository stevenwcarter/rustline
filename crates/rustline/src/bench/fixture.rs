//! The fabricated `Context` used by the pure passes. Because `Widget::render`
//! reads only from `Context` (invariant #1), a hand-built Context with every
//! `Option` field populated bypasses ALL OS reads — including `read_cpu`'s
//! ~120 ms sample. This IS the "mock": a future slow read is skipped by the
//! pure pass simply by filling its field here.

use chrono::{Local, TimeZone};
use rustline_core::Context;

/// A representative, fully-populated `Context`, built from the shared
/// [`crate::sample_context::sample_context`] (W52) with healthy (non-alerting)
/// readings and a fixed timestamp swapped in for reproducibility. Every
/// widget renders its real `format` branch on it (see the completeness
/// test) — so no widget degrades to `down_format`, which would make the pure
/// numbers meaningless.
///
/// Interfaces carry both a LAN address (`192.168.1.42` on a non-virtual NIC, so
/// `pick_lan` selects it) and a Tailscale CGNAT address (`100.101.4.7`, so
/// `pick_tailscale` selects it) — see `rustline-core/src/widgets/net.rs`.
pub fn fabricated_context() -> Context {
    Context {
        hostname: "benchbox".into(),
        loadavg: Some([0.42, 0.37, 0.30]),
        now: Local
            .with_ymd_and_hms(2026, 7, 21, 12, 0, 0)
            .single()
            .expect("fixed timestamp is valid"),
        ..crate::sample_context::sample_context(false)
    }
}
