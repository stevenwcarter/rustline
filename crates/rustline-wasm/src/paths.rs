//! XDG path resolution for the plugin dir and per-plugin state dirs, plus
//! `~/` expansion. All under `$XDG_DATA_HOME/rustline` (fallback
//! `$HOME/.local/share/rustline`) per the design.

use std::path::PathBuf;

/// Expand a leading `~/` to `$HOME`; otherwise return the path as-is.
pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(p)
}

/// `$XDG_DATA_HOME/rustline` (fallback `$HOME/.local/share/rustline`).
pub fn data_root() -> PathBuf {
    std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share")
        })
        .join("rustline")
}

/// Root of per-plugin state dirs: `<data_root>/state`.
pub fn state_root() -> PathBuf {
    data_root().join("state")
}

/// Default plugin discovery dir: `<data_root>/plugins`.
pub fn default_plugin_dir() -> PathBuf {
    data_root().join("plugins")
}
