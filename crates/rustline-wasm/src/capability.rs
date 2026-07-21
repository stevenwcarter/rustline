//! Per-plugin capability context: the *instance's* allowlists, state root, and
//! quota. Built from `PluginConfig`; stored in Extism `UserData` so each plugin
//! only ever sees its own grants (per-plugin scoping).

use std::path::PathBuf;

use rustline_core::PluginConfig;

use crate::allow::AllowSet;
#[cfg(test)]
use crate::state;

pub struct CapabilityCtx {
    pub name: String,
    pub allowed_urls: AllowSet,
    pub allowed_paths: AllowSet,
    pub state_root: PathBuf,
    pub max_state_bytes: u64,
}

impl CapabilityCtx {
    pub fn from_config(name: &str, pc: &PluginConfig, state_root: PathBuf) -> Self {
        Self {
            name: name.to_string(),
            allowed_urls: AllowSet::compile(&pc.allowed_urls),
            allowed_paths: AllowSet::compile(&pc.allowed_paths),
            state_root,
            max_state_bytes: pc.max_state_bytes,
        }
    }

    /// This plugin's own state dir: `<state_root>/<name>`.
    pub fn state_dir(&self) -> PathBuf {
        self.state_root.join(&self.name)
    }

    // re-exported here so tests can build a ctx without touching the module path
    #[cfg(test)]
    pub fn state_sub(&self, rel: &str) -> Result<PathBuf, String> {
        Ok(self.state_dir().join(state::sanitize_relpath(rel)?))
    }
}
