//! The curated plugin index (W49): the wire types for `registry/index.json`,
//! the pure parse/filter/freshness logic, and the fetch-with-TTL-cache that
//! backs `rustline plugin search`.
//!
//! Discovery grants nothing. Like `plugin install`, finding a plugin here never
//! widens an allowlist — only `plugin approve` or a hand edit does. The
//! `capabilities` field is advertising copy so a user can see what a plugin will
//! ask for *before* installing it; the host never consults it.

#![allow(dead_code)]

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};

/// The index schema this build understands.
pub const SCHEMA_VERSION: u32 = 1;

/// Where the curated index is fetched from when config doesn't override it.
pub const DEFAULT_INDEX_URL: &str =
    "https://raw.githubusercontent.com/stevenwcarter/rustline/main/registry/index.json";

/// Cache file name under the state root. Listed in the docs among the
/// host-owned state-file names a plugin should avoid colliding with.
pub const INDEX_CACHE_FILE: &str = "plugin-index.json";

/// How long a fetched index stays fresh before `plugin search` refetches.
pub const INDEX_TTL_SECS: u64 = 24 * 60 * 60;

/// The whole `registry/index.json` document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginIndex {
    pub schema_version: u32,
    #[serde(default)]
    pub plugins: Vec<IndexEntry>,
}

/// One discoverable plugin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    /// The `.wasm` stem, and therefore the widget/range name (invariant #7).
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// `owner/repo` to pass to `plugin install`, when installable.
    #[serde(default)]
    pub source: Option<String>,
    /// True for the example plugins that ship in this repo and are built with
    /// `just build-plugin <name>` rather than downloaded from a release.
    #[serde(default)]
    pub bundled: bool,
    /// Informational only — what this plugin will ask to be granted. Never
    /// consulted by the host; it grants nothing.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// Reject a schema this build cannot read rather than silently misinterpreting it.
fn validate(index: PluginIndex) -> anyhow::Result<PluginIndex> {
    if index.schema_version != SCHEMA_VERSION {
        bail!(
            "plugin index schema_version {} is not supported (this rustline understands {SCHEMA_VERSION}); upgrade rustline",
            index.schema_version
        );
    }
    Ok(index)
}

/// Parse an index document from JSON text.
pub fn parse_index(body: &str) -> anyhow::Result<PluginIndex> {
    let index: PluginIndex = serde_json::from_str(body).context("parse plugin index JSON")?;
    validate(index)
}

/// Parse an index document from an already-deserialized JSON value (the shape
/// [`crate::plugin_install::Downloader::get_json`] returns).
pub fn parse_index_value(v: &serde_json::Value) -> anyhow::Result<PluginIndex> {
    let index: PluginIndex =
        serde_json::from_value(v.clone()).context("parse plugin index JSON")?;
    validate(index)
}

/// Entries matching `query` (case-insensitive substring over name and
/// description). An absent or blank query returns every entry, in index order.
pub fn filter_entries<'a>(index: &'a PluginIndex, query: Option<&str>) -> Vec<&'a IndexEntry> {
    let Some(q) = query.map(str::trim).filter(|q| !q.is_empty()) else {
        return index.plugins.iter().collect();
    };
    let q = q.to_lowercase();
    index
        .plugins
        .iter()
        .filter(|e| e.name.to_lowercase().contains(&q) || e.description.to_lowercase().contains(&q))
        .collect()
}

/// Whether a cache stamped `fetched_at` is still fresh at `now`. A clock that
/// moved backwards counts as stale (refetch) rather than fresh forever.
pub fn index_is_fresh(fetched_at: u64, now: u64, ttl_secs: u64) -> bool {
    now >= fetched_at && now.saturating_sub(fetched_at) < ttl_secs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_json() -> &'static str {
        r#"{
          "schema_version": 1,
          "plugins": [
            {"name":"weather","description":"Weather from wttr.in","source":"o/r","bundled":true,"capabilities":["http_cached"]},
            {"name":"cmdrun","description":"Runs a configured command","source":"o/r2","bundled":false,"capabilities":["exec"]}
          ]
        }"#
    }

    #[test]
    fn parses_a_valid_index() {
        let idx = parse_index(sample_json()).expect("valid index");
        assert_eq!(idx.schema_version, 1);
        assert_eq!(idx.plugins.len(), 2);
        assert_eq!(idx.plugins[0].name, "weather");
        assert!(idx.plugins[0].bundled);
        assert_eq!(idx.plugins[1].source.as_deref(), Some("o/r2"));
    }

    #[test]
    fn tolerates_unknown_fields_so_the_index_can_grow() {
        let body = r#"{"schema_version":1,"plugins":[
            {"name":"x","description":"d","future_field":"ignored"}
        ]}"#;
        let idx = parse_index(body).expect("unknown fields must not break parsing");
        assert_eq!(idx.plugins[0].name, "x");
    }

    #[test]
    fn defaults_optional_entry_fields() {
        let body = r#"{"schema_version":1,"plugins":[{"name":"x"}]}"#;
        let idx = parse_index(body).expect("minimal entry");
        assert_eq!(idx.plugins[0].description, "");
        assert_eq!(idx.plugins[0].source, None);
        assert!(!idx.plugins[0].bundled);
        assert!(idx.plugins[0].capabilities.is_empty());
    }

    #[test]
    fn rejects_an_unsupported_schema_version() {
        let body = r#"{"schema_version":999,"plugins":[]}"#;
        assert!(
            parse_index(body).is_err(),
            "a future schema must be refused, not misread"
        );
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(parse_index("{not json").is_err());
    }

    #[test]
    fn no_query_returns_every_entry() {
        let idx = parse_index(sample_json()).unwrap();
        assert_eq!(filter_entries(&idx, None).len(), 2);
        assert_eq!(filter_entries(&idx, Some("")).len(), 2);
        assert_eq!(filter_entries(&idx, Some("   ")).len(), 2);
    }

    #[test]
    fn filters_by_name_case_insensitively() {
        let idx = parse_index(sample_json()).unwrap();
        let hits = filter_entries(&idx, Some("WEATH"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "weather");
    }

    #[test]
    fn filters_by_description_too() {
        let idx = parse_index(sample_json()).unwrap();
        let hits = filter_entries(&idx, Some("configured command"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "cmdrun");
    }

    #[test]
    fn a_query_matching_nothing_returns_empty() {
        let idx = parse_index(sample_json()).unwrap();
        assert!(filter_entries(&idx, Some("zzzzz")).is_empty());
    }

    #[test]
    fn freshness_respects_the_ttl() {
        assert!(index_is_fresh(1_000, 1_000, 100), "just fetched is fresh");
        assert!(
            index_is_fresh(1_000, 1_099, 100),
            "inside the window is fresh"
        );
        assert!(!index_is_fresh(1_000, 1_100, 100), "at the ttl is stale");
        assert!(!index_is_fresh(1_000, 5_000, 100), "well past is stale");
    }

    #[test]
    fn a_backward_clock_counts_as_stale_rather_than_forever_fresh() {
        assert!(!index_is_fresh(5_000, 1_000, 100));
    }

    #[test]
    fn the_shipped_index_file_parses_and_is_well_formed() {
        // Guards the committed data, not just the parser: a typo in
        // registry/index.json fails CI instead of only failing at runtime for
        // whoever runs `plugin search` next.
        let body = include_str!("../../../registry/index.json");
        let idx = parse_index(body).expect("registry/index.json must parse");
        assert!(!idx.plugins.is_empty());
        for e in &idx.plugins {
            assert!(!e.name.is_empty(), "every entry needs a name");
            assert!(
                e.name.len() <= 15,
                "{}: a plugin name over 15 bytes is not click-toggleable (invariant #7)",
                e.name
            );
            assert!(
                e.name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "{}: plugin names must be [A-Za-z0-9_-] (invariant #7)",
                e.name
            );
            assert_ne!(e.name, "window", "`window` is reserved");
            assert!(!e.description.is_empty(), "{}: needs a description", e.name);
        }
    }
}
