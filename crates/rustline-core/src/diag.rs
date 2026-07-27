//! The `warn_once` seam.
//!
//! Most of this crate's `warn!` sites describe *static misconfiguration* — an
//! unknown widget name, an unparseable instance table, a bad timezone. Every
//! `rustline render` tick is a fresh process that re-derives all of it from
//! cold, so those warns re-fire once per tick: at `status-interval 1` a single
//! typo writes ~86,400 identical lines a day and evicts every other diagnostic
//! from the log.
//!
//! Deduping needs state that outlives the process, which lives in the binary
//! crate (it owns the state root). This module is the seam: `rustline`'s
//! `main` installs a hook right after `logging::init`, and every crate below it
//! calls [`warn_once`] without knowing where the markers live.
//!
//! **Fail open.** With no hook installed — unit tests, `rustline-core` used
//! standalone — [`warn_once`] simply emits. A dedup layer must never be the
//! reason a diagnostic goes missing.

use std::sync::OnceLock;

type WarnOnceHook = Box<dyn Fn(&str, &dyn Fn()) + Send + Sync>;

static HOOK: OnceLock<WarnOnceHook> = OnceLock::new();

/// Install the process-wide dedup hook. Idempotent: the first caller wins and
/// later calls are ignored, so a test that installs one cannot be broken by
/// another test in the same binary.
pub fn set_warn_once_hook(hook: WarnOnceHook) {
    let _ = HOOK.set(hook);
}

/// Emit `emit()` unless the installed hook has already seen `key` this
/// generation. `key` must identify both the site and its payload — e.g.
/// `"unknown-widget:memroy"` — so two different typos are two different warns.
pub fn warn_once(key: &str, emit: impl Fn()) {
    match HOOK.get() {
        Some(hook) => hook(key, &emit),
        None => emit(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn warn_once_emits_when_no_hook_is_installed() {
        // The hook is a process-wide OnceLock and other tests in this binary
        // must not depend on install order, so this asserts only the
        // no-hook-or-permissive default: `emit` runs.
        let hits = AtomicUsize::new(0);
        warn_once("diag-test-key", || {
            hits.fetch_add(1, Ordering::Relaxed);
        });
        assert!(hits.load(Ordering::Relaxed) >= 1);
    }
}
