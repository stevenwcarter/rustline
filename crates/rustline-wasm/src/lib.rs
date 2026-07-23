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

use rustline_core::{Config, RANGE_NAME_MAX_BYTES, Registry, WidgetDescriptor, WidgetSource};

pub use host::{WasmWidget, build_plugin};
pub use manifest::{PluginManifest, resolve_manifest};
pub use paths::{data_root, default_plugin_dir, expand_tilde, state_root};

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
