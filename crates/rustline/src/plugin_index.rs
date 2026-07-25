//! The curated plugin index (W49): the wire types for `registry/index.json`,
//! the pure parse/filter/freshness logic, and the fetch-with-TTL-cache that
//! backs `rustline plugin search`.
//!
//! Discovery grants nothing. Like `plugin install`, finding a plugin here never
//! widens an allowlist — only `plugin approve` or a hand edit does. The
//! `capabilities` field is advertising copy so a user can see what a plugin will
//! ask for *before* installing it; the host never consults it.

use std::path::Path;

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};

use crate::plugin_install::Downloader;
use crate::sample_store::{read_sample, write_sample};

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

/// Parse an index document from JSON text. Only exercised by this module's own
/// tests today (production always fetches/caches via
/// [`Downloader::get_json`]'s already-deserialized `Value`, through
/// `parse_index_value` below) — `#[cfg(test)]` reflects that honestly instead
/// of masking it with a blanket `#[allow(dead_code)]`.
#[cfg(test)]
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

/// An index plus whether it came from a cache we could not refresh.
#[derive(Clone, Debug)]
pub struct LoadedIndex {
    pub index: PluginIndex,
    /// True when a fetch failed and this is the last-known-good cached copy.
    pub stale: bool,
}

/// The on-disk cache envelope: the index plus when it was fetched.
#[derive(Serialize, Deserialize)]
struct CachedIndex {
    fetched_at: u64,
    index: PluginIndex,
}

/// Read and parse the cache, or `None` if absent, unreadable, or corrupt.
/// A corrupt cache is never fatal — it simply falls through to a fetch.
fn read_cache(state_dir: &Path) -> Option<CachedIndex> {
    let raw = read_sample(state_dir, INDEX_CACHE_FILE)?;
    serde_json::from_str(&raw).ok()
}

/// Best-effort cache write; a failure is logged by `write_sample`, never fatal.
fn write_cache(state_dir: &Path, index: &PluginIndex, now: u64) {
    let cached = CachedIndex {
        fetched_at: now,
        index: index.clone(),
    };
    if let Ok(body) = serde_json::to_string(&cached) {
        write_sample(state_dir, INDEX_CACHE_FILE, &body);
    }
}

/// Load the plugin index: a fresh cache is served as-is; otherwise fetch,
/// cache, and return. A failed fetch falls back to the last-known-good cache
/// (flagged `stale`) and only errors when there is nothing cached at all.
///
/// `refresh` bypasses the TTL. `now` is injected so freshness is testable.
pub fn load_index<D: Downloader>(
    dl: &D,
    state_dir: &Path,
    url: &str,
    refresh: bool,
    now: u64,
) -> anyhow::Result<LoadedIndex> {
    let cached = read_cache(state_dir);

    if !refresh
        && let Some(c) = &cached
        && index_is_fresh(c.fetched_at, now, INDEX_TTL_SECS)
    {
        return Ok(LoadedIndex {
            index: c.index.clone(),
            stale: false,
        });
    }

    match dl.get_json(url).and_then(|v| parse_index_value(&v)) {
        Ok(index) => {
            write_cache(state_dir, &index, now);
            Ok(LoadedIndex {
                index,
                stale: false,
            })
        }
        Err(fetch_err) => match cached {
            Some(c) => {
                tracing::warn!(%url, error = %fetch_err, "plugin index fetch failed; serving the cached copy");
                Ok(LoadedIndex {
                    index: c.index,
                    stale: true,
                })
            }
            None => Err(fetch_err.context(format!("fetch plugin index from {url}"))),
        },
    }
}

/// One row of `plugin search --json`. Mirrors the existing `--json` convention
/// (`plugin_list_json`, `pattern_list_json`, `denials_json`): a local
/// `Serialize` struct rendered as a pretty-printed array.
#[derive(Serialize)]
struct SearchEntryJson<'a> {
    name: &'a str,
    description: &'a str,
    source: Option<&'a str>,
    bundled: bool,
    capabilities: &'a [String],
    /// Whether a `<name>.wasm` is already present in the plugin dir.
    installed: bool,
}

/// Render search results as a pretty-printed JSON array, `"[]"` on failure.
pub fn search_json(entries: &[&IndexEntry], installed: &[String]) -> String {
    let rows: Vec<SearchEntryJson<'_>> = entries
        .iter()
        .map(|e| SearchEntryJson {
            name: &e.name,
            description: &e.description,
            source: e.source.as_deref(),
            bundled: e.bundled,
            capabilities: &e.capabilities,
            installed: installed.iter().any(|n| n == &e.name),
        })
        .collect();
    serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".to_string())
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

    use std::cell::RefCell;

    /// A `Downloader` that serves a canned body and counts calls, so a test can
    /// assert the cache actually prevented a fetch.
    struct FakeDownloader {
        body: RefCell<Result<String, String>>,
        calls: RefCell<usize>,
    }

    impl FakeDownloader {
        fn ok(body: &str) -> Self {
            Self {
                body: RefCell::new(Ok(body.to_string())),
                calls: RefCell::new(0),
            }
        }
        fn failing() -> Self {
            Self {
                body: RefCell::new(Err("network down".to_string())),
                calls: RefCell::new(0),
            }
        }
        fn calls(&self) -> usize {
            *self.calls.borrow()
        }
    }

    impl crate::plugin_install::Downloader for FakeDownloader {
        fn get_json(&self, _url: &str) -> anyhow::Result<serde_json::Value> {
            *self.calls.borrow_mut() += 1;
            match &*self.body.borrow() {
                Ok(b) => Ok(serde_json::from_str(b)?),
                Err(e) => anyhow::bail!("{e}"),
            }
        }
        fn get_bytes(&self, _url: &str) -> anyhow::Result<Vec<u8>> {
            unreachable!("the index is fetched as JSON")
        }
    }

    #[test]
    fn a_cold_cache_fetches_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let dl = FakeDownloader::ok(sample_json());

        let loaded = load_index(&dl, dir.path(), "http://x", false, 1_000).expect("fetch");
        assert_eq!(dl.calls(), 1);
        assert!(!loaded.stale);
        assert_eq!(loaded.index.plugins.len(), 2);
        assert!(
            dir.path().join(INDEX_CACHE_FILE).exists(),
            "the fetch must be cached"
        );
    }

    #[test]
    fn a_fresh_cache_is_served_without_fetching() {
        let dir = tempfile::tempdir().unwrap();
        let dl = FakeDownloader::ok(sample_json());
        load_index(&dl, dir.path(), "http://x", false, 1_000).unwrap();

        let loaded = load_index(&dl, dir.path(), "http://x", false, 1_500).expect("cache hit");
        assert_eq!(
            dl.calls(),
            1,
            "a fresh cache must not hit the network again"
        );
        assert!(!loaded.stale);
    }

    #[test]
    fn refresh_forces_a_fetch_past_a_fresh_cache() {
        let dir = tempfile::tempdir().unwrap();
        let dl = FakeDownloader::ok(sample_json());
        load_index(&dl, dir.path(), "http://x", false, 1_000).unwrap();

        load_index(&dl, dir.path(), "http://x", true, 1_500).expect("forced fetch");
        assert_eq!(dl.calls(), 2, "--refresh must bypass the TTL");
    }

    #[test]
    fn an_expired_cache_triggers_a_refetch() {
        let dir = tempfile::tempdir().unwrap();
        let dl = FakeDownloader::ok(sample_json());
        load_index(&dl, dir.path(), "http://x", false, 1_000).unwrap();

        load_index(
            &dl,
            dir.path(),
            "http://x",
            false,
            1_000 + INDEX_TTL_SECS + 1,
        )
        .unwrap();
        assert_eq!(dl.calls(), 2);
    }

    #[test]
    fn a_failed_fetch_serves_the_stale_cache_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        load_index(
            &FakeDownloader::ok(sample_json()),
            dir.path(),
            "http://x",
            false,
            1_000,
        )
        .unwrap();

        let dl = FakeDownloader::failing();
        let loaded = load_index(&dl, dir.path(), "http://x", false, 9_999_999)
            .expect("a stale cache beats no answer");
        assert!(
            loaded.stale,
            "the caller must be able to warn that this is stale"
        );
        assert_eq!(loaded.index.plugins.len(), 2);
    }

    #[test]
    fn a_failed_fetch_with_no_cache_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let dl = FakeDownloader::failing();
        assert!(load_index(&dl, dir.path(), "http://x", false, 1_000).is_err());
    }

    #[test]
    fn a_corrupt_cache_file_is_ignored_rather_than_fatal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(INDEX_CACHE_FILE), "{{{ not json").unwrap();
        let dl = FakeDownloader::ok(sample_json());
        let loaded = load_index(&dl, dir.path(), "http://x", false, 1_000)
            .expect("a corrupt cache must fall through to a fetch");
        assert_eq!(dl.calls(), 1);
        assert!(!loaded.stale);
    }

    #[test]
    fn search_json_shape_and_installed_marker() {
        let idx = parse_index(sample_json()).unwrap();
        let entries = filter_entries(&idx, None);
        let installed = vec!["weather".to_string()];

        let out = search_json(&entries, &installed);
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON array");
        let arr = v.as_array().expect("array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["name"], "weather");
        assert_eq!(arr[0]["installed"], true);
        assert_eq!(arr[0]["bundled"], true);
        assert_eq!(arr[0]["capabilities"][0], "http_cached");
        assert_eq!(arr[1]["name"], "cmdrun");
        assert_eq!(arr[1]["installed"], false, "not present in the plugin dir");
        assert_eq!(arr[1]["source"], "o/r2");
    }

    #[test]
    fn search_json_of_nothing_is_an_empty_array() {
        let idx = parse_index(sample_json()).unwrap();
        let none: Vec<&IndexEntry> = filter_entries(&idx, Some("zzz"));
        assert_eq!(search_json(&none, &[]).trim(), "[]");
    }
}
