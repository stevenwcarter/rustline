//! The host↔guest wire types. Host functions return these as JSON strings;
//! `render` receives `RenderInput` and returns `Vec<Segment>` as JSON.

use rustline_core::{Context, Segment};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct HttpResult {
    pub ok: bool,
    pub status: u16,
    pub body: String,
    pub error: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ReadResult {
    pub ok: bool,
    pub exists: bool,
    pub contents: String,
    pub error: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct WriteResult {
    pub ok: bool,
    pub error: String,
}

/// Result of a TTL-cached HTTP GET. `ok` means "a usable body is present"
/// (fresh OR stale), not "transport succeeded"; `stale` distinguishes them.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CachedHttpResult {
    pub ok: bool,
    pub status: u16,
    pub body: String,
    pub error: String,
    pub stale: bool,
    pub age_secs: i64,
}

/// What the host passes to a plugin's `render` export.
#[derive(Serialize)]
pub struct RenderInput<'a> {
    pub context: &'a Context,
    pub config: &'a serde_json::Value,
}

/// Parse a plugin's `render` output into segments; any malformed output
/// degrades to an empty vec (never breaks the bar).
pub fn parse_render_output(s: &str) -> Vec<Segment> {
    serde_json::from_str(s).unwrap_or_default()
}
