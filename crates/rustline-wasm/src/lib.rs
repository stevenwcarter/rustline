//! The rustline WASM plugin host: an Extism runtime with capability-gated
//! host functions (network + filesystem) plus one intentionally
//! capability-free logging function (`rl_log`), and discovery/registration
//! of plugins as `rustline_core::Widget`s. All capability checks happen
//! here — guests have zero ambient authority.

pub mod abi;
pub mod allow;
pub mod cache;
pub mod capability;
pub mod fetch;
pub mod host;
pub mod manifest;
pub mod paths;
pub mod perform;
pub mod state;

use std::path::Path;
use std::sync::Arc;

use rustline_abi::ABI_VERSION;
use rustline_core::{
    Config, PluginConfig, RANGE_NAME_MAX_BYTES, Registry, WidgetDescriptor, WidgetSource,
};

pub use capability::{CapabilityCtx, DenialKind, DenialObserver};
pub use host::{WasmWidget, build_plugin};
pub use manifest::{PluginManifest, resolve_manifest};
pub use paths::{data_root, default_plugin_dir, expand_tilde, state_root};

/// The outcome of comparing the host's [`ABI_VERSION`] against a guest's
/// declared version (its optional `abi_version` export).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiDecision {
    /// The guest declares the same ABI version as the host — register.
    Register,
    /// The guest has no `abi_version` export (a pre-negotiation plugin) —
    /// register anyway so existing plugins keep working.
    RegisterLegacy,
    /// The guest declares a different ABI version — skip; never register.
    Skip,
}

/// Decide whether to register a plugin, given the host's ABI version and the
/// guest's declared version (`None` when the guest has no `abi_version`
/// export, or its output failed to parse as `u32`). Pure, so it's unit-tested
/// directly; `register_plugins` is its only caller.
pub fn abi_decision(host: u32, guest: Option<u32>) -> AbiDecision {
    match guest {
        Some(v) if v == host => AbiDecision::Register,
        None => AbiDecision::RegisterLegacy,
        Some(_) => AbiDecision::Skip,
    }
}

/// Discover `*.wasm` in `plugin_dir` and register each **needed** plugin as a
/// widget. Only plugins whose filename stem appears in `needed` are
/// instantiated (avoids wasm cold-start for unused plugins). A stem colliding
/// with a built-in, a `name()` export that disagrees with the stem, or any
/// instantiation error is logged and skipped — never fatal.
pub fn register_plugins(reg: &mut Registry, cfg: &Config, plugin_dir: &Path, needed: &[String]) {
    let root = state_root();
    let Ok(entries) = std::fs::read_dir(plugin_dir) else {
        return; // missing dir → no plugins, no error
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("wasm") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !needed.iter().any(|n| n == stem) {
            continue;
        }
        if reg.contains(stem) {
            tracing::warn!(plugin = %stem, "plugin name collides with a built-in, skipping");
            continue;
        }
        let pc = cfg.plugins.get(stem).cloned().unwrap_or_default();
        let Ok(wasm) = std::fs::read(&path) else {
            tracing::warn!(plugin = %stem, "failed to read plugin file, skipping");
            continue;
        };
        let ctx = capability::CapabilityCtx::from_config(stem, &pc, root.clone());
        let options = serde_json::to_value(&pc.options).unwrap_or_default();
        let mut plugin = match host::build_plugin(&wasm, ctx) {
            Ok(p) => p,
            Err(error) => {
                tracing::warn!(plugin = %stem, %error, "failed to instantiate plugin, skipping");
                continue;
            }
        };
        match plugin.call::<&str, &str>("name", "") {
            Ok(name) if name == stem => {}
            Ok(name) => {
                tracing::warn!(plugin = %stem, exported = %name, "plugin name mismatch, skipping");
                continue;
            }
            Err(error) => {
                tracing::warn!(plugin = %stem, %error, "plugin missing name export, skipping");
                continue;
            }
        }
        // Optional handshake: a guest may export `abi_version() -> String`
        // declaring the ABI version it was built against. No export (an
        // existing, pre-negotiation plugin) or an unparseable result both
        // count as "unknown" and register anyway (invariant N2: a plugin
        // never breaks the bar just for lacking this export).
        let guest_abi_version = plugin
            .call::<&str, &str>("abi_version", "")
            .ok()
            .and_then(|v| v.parse::<u32>().ok());
        match abi_decision(ABI_VERSION, guest_abi_version) {
            AbiDecision::Register => {}
            AbiDecision::RegisterLegacy => {
                tracing::info!(plugin = %stem, "plugin has no abi_version export, registering as legacy");
            }
            AbiDecision::Skip => {
                tracing::warn!(
                    plugin = %stem,
                    host = ABI_VERSION,
                    guest = ?guest_abi_version,
                    "plugin ABI version mismatch, skipping"
                );
                continue;
            }
        }
        if stem.len() > RANGE_NAME_MAX_BYTES {
            tracing::warn!(plugin = %stem, "plugin name > 15 bytes; not click-toggleable");
        }
        let widget = host::WasmWidget::new(plugin, options, stem);
        let shared = Arc::new(widget);
        reg.register_described(
            WidgetDescriptor {
                name: stem.to_string(),
                summary: "WASM plugin".to_string(),
                configurable: true,
                source: WidgetSource::Plugin,
            },
            Box::new(move || Box::new((*shared).clone())),
        );
    }
}

/// Instantiate exactly one named plugin from `plugin_dir` with a
/// caller-supplied [`DenialObserver`] — e.g. a collecting observer for a local
/// dev harness (`rustline plugin run`) — bypassing the `needed`-list discovery
/// filter, the built-in-name-collision check, and the `name()`/ABI-export
/// verification `register_plugins` does, since a one-off harness run doesn't
/// need any of them. Returns `None` on any read/instantiation failure,
/// mirroring `register_plugins`'s never-fatal behavior. Doesn't touch the
/// `Registry` and doesn't disturb `register_plugins` itself.
pub fn instantiate_named(
    plugin_dir: &Path,
    name: &str,
    pc: &PluginConfig,
    observer: Arc<dyn DenialObserver + Send + Sync>,
) -> Option<WasmWidget> {
    let wasm = std::fs::read(plugin_dir.join(format!("{name}.wasm"))).ok()?;
    let ctx =
        capability::CapabilityCtx::from_config(name, pc, state_root()).with_observer(observer);
    let plugin = host::build_plugin(&wasm, ctx).ok()?;
    let options = serde_json::to_value(&pc.options).unwrap_or_default();
    Some(host::WasmWidget::new(plugin, options, name))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rustline_core::PluginConfig;

    use super::{AbiDecision, DenialObserver, abi_decision, instantiate_named};
    use crate::capability::NoopObserver;

    #[test]
    fn abi_decision_matrix() {
        assert!(matches!(abi_decision(1, Some(1)), AbiDecision::Register));
        assert!(matches!(abi_decision(1, None), AbiDecision::RegisterLegacy));
        assert!(matches!(abi_decision(1, Some(2)), AbiDecision::Skip));
    }

    fn noop() -> Arc<dyn DenialObserver + Send + Sync> {
        Arc::new(NoopObserver)
    }

    #[test]
    fn instantiate_named_missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(instantiate_named(dir.path(), "nope", &PluginConfig::default(), noop()).is_none());
    }

    #[test]
    fn instantiate_named_garbage_wasm_is_none() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("w.wasm"), b"not real wasm").unwrap();
        assert!(instantiate_named(dir.path(), "w", &PluginConfig::default(), noop()).is_none());
    }
}
