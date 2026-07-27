//! Per-plugin capability context: the *instance's* allowlists, state root, and
//! quota. Built from `PluginConfig`; stored in Extism `UserData` so each plugin
//! only ever sees its own grants (per-plugin scoping).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rustline_core::PluginConfig;
use serde::{Deserialize, Serialize};

use crate::allow::AllowSet;
use crate::state;

/// Sentinel for "not yet measured" in [`CapabilityCtx::state_size`]. This is
/// an in-band value, not a separate `Option` tag, so `set_state_size(u64::MAX)`
/// would silently be read back as "unseeded" rather than as a 16-exabyte
/// state dir. Unreachable in practice — nothing on a real filesystem gets
/// anywhere near `u64::MAX` bytes — so this isn't guarded further.
const SIZE_UNSEEDED: u64 = u64::MAX;

/// The kind of capability a denied request was for, carried to a
/// [`DenialObserver`] alongside the plugin name and the denied target.
/// `Serialize`/`Deserialize` (`snake_case`: `url`/`path`/`command`) so it
/// rides [`crate::denials::Denial`]'s JSONL record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DenialKind {
    /// An HTTP GET (cached or uncached) denied by `allowed_urls`.
    Url,
    /// A file read/write denied by `allowed_paths`.
    Path,
    /// A subprocess run (cached or uncached) denied by `allowed_commands`.
    Command,
}

/// Observes a plugin's denied capability requests. This is a pure
/// notification seam: an implementation decides whether/where to record a
/// denial, but `observe` is called *after* the deny decision is already made
/// (immediately before the `ok:false` return) — it never participates in the
/// gate itself (invariant N1).
pub trait DenialObserver {
    /// `plugin` is always the observing instance's own name (invariant N4 —
    /// an observer never sees another plugin's denials); `target` is the
    /// denied URL, path, or canonical argv string, verbatim.
    fn observe(&self, plugin: &str, kind: DenialKind, target: &str);
}

/// The default observer: does nothing. Every `CapabilityCtx` starts with
/// this, so existing construction (`from_config`) is unchanged; a real
/// persisted recorder is wired in later via [`CapabilityCtx::with_observer`].
#[derive(Default)]
pub struct NoopObserver;

impl DenialObserver for NoopObserver {
    fn observe(&self, _plugin: &str, _kind: DenialKind, _target: &str) {}
}

pub struct CapabilityCtx {
    pub name: String,
    pub allowed_urls: AllowSet,
    pub allowed_paths: AllowSet,
    pub allowed_commands: AllowSet,
    pub state_root: PathBuf,
    pub max_state_bytes: u64,
    observer: Arc<dyn DenialObserver + Send + Sync>,
    /// Memoized total byte size of this plugin's state dir. Seeded by one
    /// `dir_size` walk per process/daemon lifetime and then adjusted after
    /// each successful write, instead of re-walking on every write.
    /// `AtomicU64` rather than `Cell`: `CapabilityCtx` lives in Extism
    /// `UserData`, which requires `Send`, and an atomic keeps that
    /// unconditional.
    state_size: AtomicU64,
}

impl CapabilityCtx {
    pub fn from_config(name: &str, pc: &PluginConfig, state_root: PathBuf) -> Self {
        Self {
            name: name.to_string(),
            allowed_urls: AllowSet::compile(&pc.allowed_urls),
            allowed_paths: AllowSet::compile(&pc.allowed_paths),
            allowed_commands: AllowSet::compile(&pc.allowed_commands),
            state_root,
            max_state_bytes: pc.max_state_bytes,
            observer: Arc::new(NoopObserver),
            state_size: AtomicU64::new(SIZE_UNSEEDED),
        }
    }

    /// Replace this instance's denial observer (default: [`NoopObserver`]).
    /// Consumed and returned by value so it composes at the construction
    /// site: `CapabilityCtx::from_config(..).with_observer(o)`.
    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn DenialObserver + Send + Sync>) -> Self {
        self.observer = observer;
        self
    }

    /// This plugin's own state dir: `<state_root>/<name>`.
    pub fn state_dir(&self) -> PathBuf {
        self.state_root.join(&self.name)
    }

    /// Notify this instance's observer that a capability request was denied.
    /// Called at each `perform_*` deny site immediately before its existing
    /// `ok:false` return — it never changes the gate-first decision (N1).
    pub fn observe_denial(&self, kind: DenialKind, target: &str) {
        self.observer.observe(&self.name, kind, target);
    }

    /// This plugin's state-dir size in bytes, measured once and then
    /// memoized. `check_cap` stays pure and pays no walk of its own (see
    /// `state::check_cap`); this is the one place that pays it, and only on
    /// the first call.
    ///
    /// **The memo is this process's belief about the directory, not a fact
    /// about it.** It only ever tracks writes made *through this ctx*; any
    /// other mutation of the state dir is invisible to it until something
    /// invalidates it:
    /// - **Short-lived renders** (`rustline render left|right`): `render
    ///   left`/`render right` are separate processes, each handed only its
    ///   own region's plugin layout (`register_plugins` in `main.rs`), so
    ///   same-plugin contention is limited to a plugin listed in both
    ///   regions — and even then the stale window is one render, since the
    ///   process (and its memo) doesn't outlive it.
    /// - **Daemon**: every write is serialized through one long-lived ctx
    ///   (`daemon.rs`'s state lives behind one mutex), so the memo tracks its
    ///   own writes exactly; its entire exposure is *external* mutation of
    ///   the state dir, and it never re-walks unless a write fails or is
    ///   refused. That window can be the daemon's whole lifetime.
    ///
    /// Both directions are strictly looser than the pre-memo per-write walk
    /// this replaced — that laxness is the accepted trade for not walking a
    /// plugin's entire cache on every write (see `state::dir_size`'s doc).
    pub fn state_size(&self) -> u64 {
        let cached = self.state_size.load(Ordering::Relaxed);
        if cached != SIZE_UNSEEDED {
            return cached;
        }
        let measured = state::dir_size(&self.state_dir());
        self.state_size.store(measured, Ordering::Relaxed);
        measured
    }

    /// Record a known-good size (after a successful write reports its new
    /// total).
    pub fn set_state_size(&self, bytes: u64) {
        self.state_size.store(bytes, Ordering::Relaxed);
    }

    /// Drop the memo so the next `state_size` re-walks. Call whenever the
    /// directory may have changed in a way this ctx did not account for — a
    /// failed write, an eviction it did not perform. A stale-high memo would
    /// wedge writes that should succeed, so when in doubt, invalidate
    /// (invariant N3).
    pub fn invalidate_state_size(&self) {
        self.state_size.store(SIZE_UNSEEDED, Ordering::Relaxed);
    }

    // re-exported here so tests can build a ctx without touching the module path
    #[cfg(test)]
    pub fn state_sub(&self, rel: &str) -> Result<PathBuf, String> {
        Ok(self.state_dir().join(state::sanitize_relpath(rel)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denial_kind_command_serializes_as_snake_case() {
        let json = serde_json::to_string(&DenialKind::Command).unwrap();
        assert_eq!(json, "\"command\"");
        let back: DenialKind = serde_json::from_str("\"command\"").unwrap();
        assert_eq!(back, DenialKind::Command);
    }

    #[test]
    fn capability_ctx_compiles_the_command_allowlist_and_denies_by_default() {
        let pc = PluginConfig::default();
        let ctx = CapabilityCtx::from_config("p", &pc, std::path::PathBuf::from("/tmp"));
        assert!(
            !ctx.allowed_commands.allows("anything"),
            "an empty allowlist matches nothing (deny by default)"
        );

        let pc = PluginConfig {
            allowed_commands: vec!["playerctl metadata*".to_string()],
            ..PluginConfig::default()
        };
        let ctx = CapabilityCtx::from_config("p", &pc, std::path::PathBuf::from("/tmp"));
        assert!(ctx.allowed_commands.allows("playerctl metadata"));
        assert!(ctx.allowed_commands.allows("playerctl metadata --format x"));
        assert!(!ctx.allowed_commands.allows("playerctl play"));
    }

    #[test]
    fn state_size_is_seeded_once_then_memoized() {
        let d = tempfile::tempdir().unwrap();
        let ctx = CapabilityCtx::from_config("p", &PluginConfig::default(), d.path().to_path_buf());
        std::fs::create_dir_all(ctx.state_dir()).unwrap();
        std::fs::write(ctx.state_dir().join("a"), b"12345").unwrap();
        assert_eq!(ctx.state_size(), 5);
        // A file added behind the memo's back is NOT observed: that is the
        // point — the walk happens once, not once per write.
        std::fs::write(ctx.state_dir().join("b"), b"12345").unwrap();
        assert_eq!(ctx.state_size(), 5, "memoized, not re-walked");
        ctx.invalidate_state_size();
        assert_eq!(ctx.state_size(), 10, "invalidation forces a fresh walk");
    }

    #[test]
    fn set_state_size_updates_the_memo() {
        let d = tempfile::tempdir().unwrap();
        let ctx = CapabilityCtx::from_config("p", &PluginConfig::default(), d.path().to_path_buf());
        ctx.set_state_size(42);
        assert_eq!(ctx.state_size(), 42);
    }

    /// The load-bearing performance claim this memo exists for: however many
    /// times `state_size` is read, only the *first* one may pay a `dir_size`
    /// walk. `state_size_is_seeded_once_then_memoized` already proves this
    /// behaviourally (a file added behind the memo's back goes unobserved);
    /// this makes the walk count itself observable rather than inferred.
    #[test]
    fn state_size_pays_exactly_one_walk_no_matter_how_many_times_its_read() {
        let d = tempfile::tempdir().unwrap();
        let ctx = CapabilityCtx::from_config("p", &PluginConfig::default(), d.path().to_path_buf());
        std::fs::create_dir_all(ctx.state_dir()).unwrap();
        std::fs::write(ctx.state_dir().join("a"), b"12345").unwrap();
        state::reset_walk_count();
        for _ in 0..20 {
            assert_eq!(ctx.state_size(), 5);
        }
        assert_eq!(
            state::walk_count(),
            1,
            "20 reads of the memo must cost exactly one dir_size walk"
        );
    }
}
