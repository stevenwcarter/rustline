//! The rustline WASM plugin host: an Extism runtime with capability-gated
//! host functions (network + filesystem + subprocess) plus one intentionally
//! capability-free logging function (`rl_log`), and discovery/registration
//! of plugins as `rustline_core::Widget`s. All capability checks happen
//! here — guests have zero ambient authority.

pub mod abi;
pub mod allow;
pub mod argv;
pub mod cache;
pub mod capability;
pub mod denials;
pub mod fetch;
pub mod host;
pub mod integrity;
pub mod manifest;
pub mod paths;
pub mod perform;
pub mod run;
pub mod state;

use std::path::Path;
use std::sync::Arc;

use rustline_abi::ABI_VERSION;
use rustline_core::{
    Config, PluginConfig, RANGE_NAME_MAX_BYTES, Registry, WidgetDescriptor, WidgetSource,
};

pub use argv::canonical_argv;
pub use capability::{CapabilityCtx, DenialKind, DenialObserver};
pub use denials::{Denial, FileDenialObserver, denials_path, read_denials};
pub use host::{CompileCache, WasmWidget, build_plugin, build_plugin_with_cache};
pub use integrity::{ChecksumVerdict, sha256_hex, verify_checksum};
pub use manifest::{PluginManifest, resolve_manifest};
pub use paths::{
    data_root, default_plugin_dir, ensure_wasmtime_cache_config, expand_tilde, state_root,
    wasmtime_cache_config_path,
};
pub use run::{ProcessRunner, Runner};

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

/// The `.wasm` stems present in `plugin_dir`, sorted, **without reading or
/// instantiating any of them**. This is the discovery half of
/// `register_plugins`, split out so `rustline widget list`/`widget edit` can
/// show plugin widgets without paying wasm cold-start or running guest code.
/// A missing or unreadable directory yields an empty vec, never an error.
pub fn discover_plugin_names(plugin_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(plugin_dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("wasm") {
                return None;
            }
            path.file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
        })
        .collect();
    names.sort();
    names
}

/// Discover `*.wasm` in `plugin_dir` and register each **needed** plugin as a
/// widget. Only plugins whose filename stem appears in `needed` are
/// instantiated (avoids wasm cold-start for unused plugins). A stem colliding
/// with a built-in, a `name()` export that disagrees with the stem, or any
/// instantiation error is logged and skipped — never fatal.
pub fn register_plugins(reg: &mut Registry, cfg: &Config, plugin_dir: &Path, needed: &[String]) {
    let root = state_root();
    // One resolved path, reused per plugin below: each instance gets its own
    // `FileDenialObserver` (cheap — just a `PathBuf` clone) writing to the
    // same persisted record, so `rustline plugin denials <name>` has
    // something real to show instead of the default `NoopObserver`.
    let denials_path = denials::denials_path();
    for stem in discover_plugin_names(plugin_dir) {
        let path = plugin_dir.join(format!("{stem}.wasm"));
        let stem = stem.as_str();
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
        // W19: verify the recorded digest before building any capability object
        // and before wasmtime ever sees the module. An absent checksum loads as
        // before; a mismatched or unparseable one fails closed. Like every other
        // skip site here, this drops one widget with a warn — never an error
        // into the render path (invariant N2).
        match integrity::verify_checksum(pc.checksum.as_deref(), &wasm) {
            integrity::ChecksumVerdict::NotRecorded | integrity::ChecksumVerdict::Match => {}
            integrity::ChecksumVerdict::Mismatch { expected, actual } => {
                tracing::warn!(
                    plugin = %stem,
                    %expected,
                    %actual,
                    "plugin checksum mismatch, skipping"
                );
                continue;
            }
            integrity::ChecksumVerdict::Malformed { recorded } => {
                tracing::warn!(
                    plugin = %stem,
                    %recorded,
                    "recorded plugin checksum is not a valid sha256 digest, skipping"
                );
                continue;
            }
        }
        let observer = denials::FileDenialObserver::new(denials_path.clone());
        let ctx = capability::CapabilityCtx::from_config(stem, &pc, root.clone())
            .with_observer(Arc::new(observer));
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
/// verification `register_plugins` does, and (unlike `register_plugins`)
/// treats a failed checksum verification as a warning rather than a refusal,
/// since a one-off harness run doesn't need any of them. Returns `None` on any
/// read/instantiation failure, mirroring `register_plugins`'s never-fatal
/// behavior. Doesn't touch the `Registry` and doesn't disturb
/// `register_plugins` itself.
pub fn instantiate_named(
    plugin_dir: &Path,
    name: &str,
    pc: &PluginConfig,
    observer: Arc<dyn DenialObserver + Send + Sync>,
) -> Option<WasmWidget> {
    let wasm = std::fs::read(plugin_dir.join(format!("{name}.wasm"))).ok()?;
    // Dev harness (`rustline plugin run`): report a bad digest but still run.
    // This command is used while iterating on a plugin you just rebuilt, where
    // a recorded checksum legitimately mismatches on every build. The real gate
    // is `register_plugins`, which the bar, the daemon, and `plugin list` all
    // go through.
    let verdict = integrity::verify_checksum(pc.checksum.as_deref(), &wasm);
    if !verdict.allows_load() {
        tracing::warn!(
            plugin = %name,
            ?verdict,
            "plugin checksum did not verify; running anyway (dev harness)"
        );
    }
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

    use super::{
        AbiDecision, DenialObserver, abi_decision, discover_plugin_names, instantiate_named,
    };
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

    #[test]
    fn discover_plugin_names_lists_wasm_stems_sorted_and_ignores_other_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("zulu.wasm"), b"\0asm").unwrap();
        std::fs::write(dir.path().join("alpha.wasm"), b"\0asm").unwrap();
        std::fs::write(dir.path().join("alpha.toml"), b"name = 'alpha'").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"hi").unwrap();

        assert_eq!(discover_plugin_names(dir.path()), ["alpha", "zulu"]);
    }

    #[test]
    fn discover_plugin_names_on_a_missing_dir_is_empty_not_an_error() {
        assert!(discover_plugin_names(std::path::Path::new("/nonexistent-plugin-dir")).is_empty());
    }
}
