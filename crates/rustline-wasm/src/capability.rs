//! Per-plugin capability context: the *instance's* allowlists, state root, and
//! quota. Built from `PluginConfig`; stored in Extism `UserData` so each plugin
//! only ever sees its own grants (per-plugin scoping).

use std::path::PathBuf;
use std::sync::Arc;

use rustline_core::PluginConfig;
use serde::{Deserialize, Serialize};

use crate::allow::AllowSet;
#[cfg(test)]
use crate::state;

/// The kind of capability a denied request was for, carried to a
/// [`DenialObserver`] alongside the plugin name and the denied target.
/// `Serialize`/`Deserialize` (`snake_case`: `url`/`path`) so it rides
/// [`crate::denials::Denial`]'s JSONL record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DenialKind {
    /// An HTTP GET (cached or uncached) denied by `allowed_urls`.
    Url,
    /// A file read/write denied by `allowed_paths`.
    Path,
}

/// Observes a plugin's denied capability requests. This is a pure
/// notification seam: an implementation decides whether/where to record a
/// denial, but `observe` is called *after* the deny decision is already made
/// (immediately before the `ok:false` return) — it never participates in the
/// gate itself (invariant N1).
pub trait DenialObserver {
    /// `plugin` is always the observing instance's own name (invariant N4 —
    /// an observer never sees another plugin's denials); `target` is the
    /// denied URL or path string, verbatim.
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
    pub state_root: PathBuf,
    pub max_state_bytes: u64,
    observer: Arc<dyn DenialObserver + Send + Sync>,
}

impl CapabilityCtx {
    pub fn from_config(name: &str, pc: &PluginConfig, state_root: PathBuf) -> Self {
        Self {
            name: name.to_string(),
            allowed_urls: AllowSet::compile(&pc.allowed_urls),
            allowed_paths: AllowSet::compile(&pc.allowed_paths),
            state_root,
            max_state_bytes: pc.max_state_bytes,
            observer: Arc::new(NoopObserver),
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

    // re-exported here so tests can build a ctx without touching the module path
    #[cfg(test)]
    pub fn state_sub(&self, rel: &str) -> Result<PathBuf, String> {
        Ok(self.state_dir().join(state::sanitize_relpath(rel)?))
    }
}
