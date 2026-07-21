# rustline WASM Plugin System + Weather Widget — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a capability-gated WebAssembly plugin runtime for rustline and ship a `weather` plugin as the worked example.

**Architecture:** A new native host crate `rustline-wasm` embeds Extism (wasmtime), enforces per-plugin URL/path allowlists and a state-dir quota, and registers each discovered plugin as a `Widget`. Plugins reach the network/FS **only** through host functions the host checks first (zero ambient authority). The example `weather` plugin (a separate `wasm32-unknown-unknown` crate) fetches wttr.in at most once per 30 min via its per-plugin state dir.

**Tech Stack:** Rust edition 2024, Extism 1.x (`extism` host SDK, `extism-pdk` guest), `ureq` 2.x (rustls) for HTTP, `globset` + `regex` for allowlists, `walkdir` for dir sizing, `toml_edit` for CLI config mutation, `serde`/`serde_json` for the wire format.

## Global Constraints

- **Edition 2024** in every new crate; keep all crate editions equal to `rustfmt.toml`.
- **OpenSSL-free / rustls only.** `cargo tree -i openssl` and `cargo tree -i native-tls` MUST be empty. All TLS-capable deps go in with `default-features = false` + explicit rustls feature.
- **`default-features = false`** on new deps; opt features in explicitly.
- Must stay **clippy-clean** (`cargo clippy --all-targets -- -D warnings`) and **rustfmt-clean** (`cargo fmt --all --check`). Run `cargo fmt --all` before every commit (no pre-commit hook).
- **Commit `Cargo.lock`** in the same commit as any dependency change.
- **`Config::load` stays total** — a bad/missing config must never break the bar (invariant #3).
- **Core stays pure & serde-serializable** — no wasmtime/I/O in `rustline-core`; `Context`/`Segment`/`Style`/`Color` remain the serde ABI (invariants #1, #2).
- **A plugin never breaks the bar** — every plugin error/timeout/malformed output degrades to empty segments.
- **Zero ambient authority** — every guest network/FS effect passes a host-side capability check; `with_wasi(false)`; no Extism built-in HTTP.
- **`just test` stays hermetic** — no wasm toolchain required; all real-wasm tests live behind `just test-wasm` / the `wasm-e2e` feature.
- `plugins/weather` is **excluded** from the root workspace (its own `Cargo.lock`, wasm-only).

## File Structure

```
Cargo.toml                                  MODIFY  add member rustline-wasm; exclude plugins/weather
crates/rustline-core/src/config.rs          MODIFY  typed PluginConfig, plugin_dir
crates/rustline-core/src/widget.rs          MODIFY  Registry::contains
crates/rustline-wasm/Cargo.toml             CREATE
crates/rustline-wasm/src/lib.rs             CREATE  re-exports + register_plugins
crates/rustline-wasm/src/paths.rs           CREATE  data_root/state_root/default_plugin_dir/expand_tilde
crates/rustline-wasm/src/allow.rs           CREATE  Pattern, AllowSet (glob | re:)
crates/rustline-wasm/src/state.rs           CREATE  sanitize_relpath, dir_size, check_cap, normalize_abs
crates/rustline-wasm/src/abi.rs             CREATE  HttpResult/ReadResult/WriteResult, RenderInput, parse_render_output
crates/rustline-wasm/src/capability.rs      CREATE  CapabilityCtx
crates/rustline-wasm/src/fetch.rs           CREATE  Fetcher trait, UreqFetcher
crates/rustline-wasm/src/perform.rs         CREATE  perform_http_get/state_read/state_write/file_read/file_write
crates/rustline-wasm/src/host.rs            CREATE  host_fn! wrappers, build_plugin, WasmWidget
crates/rustline-wasm/tests/e2e.rs           CREATE  feature = "wasm-e2e" end-to-end
crates/rustline/Cargo.toml                  MODIFY  add rustline-wasm, toml_edit
crates/rustline/src/cli.rs                  MODIFY  --plugin-dir, `plugin` subcommand group
crates/rustline/src/main.rs                 MODIFY  resolve plugin dir, register plugins, dispatch plugin cmds
crates/rustline/src/plugin_cmd.rs           CREATE  toml_edit list/add/remove
crates/rustline/tests/smoke.rs              MODIFY  degradation + CLI round-trip
plugins/weather/Cargo.toml                  CREATE  cdylib, wasm-only extism-pdk
plugins/weather/src/lib.rs                  CREATE  pure logic + #[cfg(wasm32)] guest glue
justfile                                    MODIFY  build-weather, test-wasm
CLAUDE.md / README.md                       MODIFY  docs
```

---

### Task 1: Typed plugin config schema (`rustline-core`)

**Files:**
- Modify: `crates/rustline-core/src/config.rs`

**Interfaces:**
- Produces: `PluginConfig { source: Option<String>, allowed_urls: Vec<String>, allowed_paths: Vec<String>, max_state_bytes: u64, options: toml::Value }`; `Config.plugin_dir: Option<String>`; `Config.plugins: HashMap<String, PluginConfig>`; `fn default_max_state_bytes() -> u64` (= `52_428_800`).

- [ ] **Step 1: Write failing tests** — replace the existing `plugins_table_retained` and `config_toml_roundtrip_preserves_plugin_entry` tests (they assume the old `HashMap<String, toml::Value>` shape) with the typed-schema tests below; keep the other config tests unchanged.

```rust
    #[test]
    fn plugin_config_typed_parse_with_defaults() {
        let toml = r#"
plugin_dir = "~/.local/share/rustline/plugins"
[plugins.weather]
source = "steve/rustline-weather"
allowed_urls = ["https://wttr.in/*"]
[plugins.weather.options]
zip = "48183"
format = "{icon} {temp_f}°F"
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.plugin_dir.as_deref(), Some("~/.local/share/rustline/plugins"));
        let w = c.plugins.get("weather").expect("weather entry");
        assert_eq!(w.source.as_deref(), Some("steve/rustline-weather"));
        assert_eq!(w.allowed_urls, vec!["https://wttr.in/*".to_string()]);
        assert!(w.allowed_paths.is_empty());
        // omitted -> default 50 MB
        assert_eq!(w.max_state_bytes, 52_428_800);
        assert_eq!(w.options.get("zip").and_then(toml::Value::as_str), Some("48183"));
    }

    #[test]
    fn plugin_config_roundtrip_preserves_options() {
        let src = r#"
[plugins.weather]
allowed_urls = ["https://wttr.in/*"]
max_state_bytes = 100
[plugins.weather.options]
zip = "48183"
"#;
        let c: Config = toml::from_str(src).unwrap();
        let serialized = toml::to_string(&c).unwrap();
        let back: Config = toml::from_str(&serialized).unwrap();
        let w = back.plugins.get("weather").unwrap();
        assert_eq!(w.max_state_bytes, 100);
        assert_eq!(w.allowed_urls, vec!["https://wttr.in/*".to_string()]);
        assert_eq!(w.options.get("zip").and_then(toml::Value::as_str), Some("48183"));
    }

    #[test]
    fn malformed_plugins_table_falls_back_to_default() {
        let dir = std::env::temp_dir().join("rustline_test_badplugins");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        // max_state_bytes must be an integer; a string makes the table invalid
        std::fs::write(&p, "[plugins.weather]\nmax_state_bytes = \"lots\"\n").unwrap();
        let c = Config::load(&p);
        assert!(c.plugins.is_empty());
        assert_eq!(c.layout.left, Config::default().layout.left);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rustline-core config::tests::plugin_config_typed_parse_with_defaults`
Expected: FAIL to compile (`PluginConfig` / `plugin_dir` undefined).

- [ ] **Step 3: Implement the typed schema** — in `config.rs`, add the struct and fields.

```rust
/// Per-plugin configuration, keyed by plugin name in [`Config::plugins`].
///
/// Capability fields (`allowed_urls`, `allowed_paths`, `max_state_bytes`) are
/// enforced by the WASM host, never by the guest. `options` is opaque to the
/// host and forwarded to the plugin verbatim.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginConfig {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub allowed_urls: Vec<String>,
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    #[serde(default = "default_max_state_bytes")]
    pub max_state_bytes: u64,
    #[serde(default = "empty_table")]
    pub options: Value,
}

/// 50 MB — the default per-plugin state-directory quota.
fn default_max_state_bytes() -> u64 {
    52_428_800
}

fn empty_table() -> Value {
    Value::Table(toml::map::Map::new())
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            source: None,
            allowed_urls: Vec::new(),
            allowed_paths: Vec::new(),
            max_state_bytes: default_max_state_bytes(),
            options: empty_table(),
        }
    }
}
```

Then change `Config` (add `plugin_dir`, retype `plugins`):

```rust
    /// Directory to discover `*.wasm` plugins from; overrides the default
    /// `$XDG_DATA_HOME/rustline/plugins`. A `--plugin-dir` CLI flag overrides
    /// this in turn.
    #[serde(default)]
    pub plugin_dir: Option<String>,
    /// Per-plugin config, keyed by plugin name.
    #[serde(default)]
    pub plugins: HashMap<String, PluginConfig>,
```

(`Value` is already imported as `use toml::Value;`.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rustline-core config::`
Expected: PASS (all config tests, including the three new ones).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt --all
cargo clippy -p rustline-core --all-targets -- -D warnings
git add crates/rustline-core/src/config.rs
git commit -m "feat(core): typed per-plugin config (allowlists, state quota, plugin_dir)"
```

---

### Task 2: `rustline-wasm` crate + allow-pattern matcher

**Files:**
- Modify: `Cargo.toml` (root — add member)
- Create: `crates/rustline-wasm/Cargo.toml`, `crates/rustline-wasm/src/lib.rs`, `crates/rustline-wasm/src/allow.rs`

**Interfaces:**
- Produces: `allow::Pattern::compile(&str) -> Result<Pattern, String>`, `Pattern::is_match(&self, &str) -> bool`; `allow::AllowSet::compile(&[String]) -> AllowSet`, `AllowSet::allows(&self, &str) -> bool`.

- [ ] **Step 1: Create the crate manifest** `crates/rustline-wasm/Cargo.toml`

```toml
[package]
name = "rustline-wasm"
edition.workspace = true
version.workspace = true
license.workspace = true

[dependencies]
rustline-core = { path = "../rustline-core" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
globset = "0.4"
regex = "1"
walkdir = "2"
tracing = "0.1"
thiserror = "2"

[dev-dependencies]
tempfile = "3"
```

Add the member to the root `Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["crates/rustline-core", "crates/rustline", "crates/rustline-wasm"]
```

- [ ] **Step 2: Create `src/lib.rs` with the module and a failing-nothing skeleton**

```rust
//! The rustline WASM plugin host: an Extism runtime with capability-gated
//! host functions (network + filesystem), plus discovery/registration of
//! plugins as `rustline_core::Widget`s. All capability checks happen here —
//! guests have zero ambient authority.

pub mod allow;
```

- [ ] **Step 3: Write failing tests** in `crates/rustline-wasm/src/allow.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_url_prefix() {
        let s = AllowSet::compile(&["https://wttr.in/*".into()]);
        assert!(s.allows("https://wttr.in/48183?format=j1"));
    }

    #[test]
    fn glob_denies_other_host() {
        let s = AllowSet::compile(&["https://wttr.in/*".into()]);
        assert!(!s.allows("https://evil.example/steal"));
    }

    #[test]
    fn empty_set_denies_everything() {
        let s = AllowSet::compile(&[]);
        assert!(!s.allows("https://wttr.in/48183"));
    }

    #[test]
    fn regex_prefix_matches() {
        let s = AllowSet::compile(&[r"re:^https://wttr\.in/\d{5}".into()]);
        assert!(s.allows("https://wttr.in/48183?format=j1"));
        assert!(!s.allows("https://wttr.in/abcde"));
    }

    #[test]
    fn malformed_pattern_is_skipped_not_fatal() {
        // one bad regex, one good glob -> the good one still works
        let s = AllowSet::compile(&["re:[".into(), "https://ok/*".into()]);
        assert!(s.allows("https://ok/path"));
        assert!(!s.allows("https://nope/x"));
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p rustline-wasm allow::`
Expected: FAIL to compile (`AllowSet` undefined).

- [ ] **Step 5: Implement `allow.rs`** (above the `tests` module)

```rust
//! URL/path allow-patterns. Each entry is a glob by default, or a regex when
//! prefixed with `re:`. Globs use `globset` defaults (`*` matches across `/`),
//! so `https://wttr.in/*` matches the full URL incl. its query string.

use globset::{Glob, GlobMatcher};
use regex::Regex;

/// A single compiled allow-pattern.
pub enum Pattern {
    Glob(GlobMatcher),
    Regex(Regex),
}

impl Pattern {
    /// Compile one entry; `re:` prefix selects regex, otherwise glob.
    pub fn compile(entry: &str) -> Result<Pattern, String> {
        if let Some(rx) = entry.strip_prefix("re:") {
            Regex::new(rx).map(Pattern::Regex).map_err(|e| e.to_string())
        } else {
            Glob::new(entry)
                .map(|g| Pattern::Glob(g.compile_matcher()))
                .map_err(|e| e.to_string())
        }
    }

    pub fn is_match(&self, s: &str) -> bool {
        match self {
            Pattern::Glob(g) => g.is_match(s),
            Pattern::Regex(r) => r.is_match(s),
        }
    }
}

/// A set of allow-patterns; `allows` is true iff any pattern matches. An empty
/// set denies everything (deny-by-default). Malformed entries are logged and
/// skipped, never fatal.
pub struct AllowSet(Vec<Pattern>);

impl AllowSet {
    pub fn compile(entries: &[String]) -> AllowSet {
        let mut patterns = Vec::new();
        for entry in entries {
            match Pattern::compile(entry) {
                Ok(p) => patterns.push(p),
                Err(error) => tracing::warn!(pattern = %entry, %error, "invalid allow pattern, skipping"),
            }
        }
        AllowSet(patterns)
    }

    pub fn allows(&self, subject: &str) -> bool {
        self.0.iter().any(|p| p.is_match(subject))
    }
}
```

Add `pub mod allow;` is already in lib.rs from Step 2. Add the module contents now include the impl + tests.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p rustline-wasm allow::`
Expected: PASS (5 tests). Then `cargo tree -i openssl` → empty.

- [ ] **Step 7: fmt + clippy + commit (with Cargo.lock)**

```bash
cargo fmt --all
cargo clippy -p rustline-wasm --all-targets -- -D warnings
git add Cargo.toml Cargo.lock crates/rustline-wasm/Cargo.toml crates/rustline-wasm/src/lib.rs crates/rustline-wasm/src/allow.rs
git commit -m "feat(wasm): rustline-wasm crate + glob/regex allow-pattern matcher"
```

---

### Task 3: Path sandbox, size accounting, and path helpers

**Files:**
- Create: `crates/rustline-wasm/src/state.rs`, `crates/rustline-wasm/src/paths.rs`
- Modify: `crates/rustline-wasm/src/lib.rs` (add `pub mod state; pub mod paths;`)

**Interfaces:**
- Produces:
  - `state::sanitize_relpath(&str) -> Result<PathBuf, String>` (rejects absolute + `..`)
  - `state::normalize_abs(&str) -> Result<String, String>` (require absolute, reject `..`; returns the path string)
  - `state::dir_size(&Path) -> u64`
  - `state::check_cap(dir: &Path, target: &Path, new_len: u64, cap: u64) -> Result<(), String>`
  - `paths::expand_tilde(&str) -> PathBuf`, `paths::data_root() -> PathBuf`, `paths::state_root() -> PathBuf`, `paths::default_plugin_dir() -> PathBuf`

- [ ] **Step 1: Write failing tests** in `crates/rustline-wasm/src/state.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn sanitize_rejects_absolute_and_parent() {
        assert!(sanitize_relpath("/etc/passwd").is_err());
        assert!(sanitize_relpath("../secrets").is_err());
        assert!(sanitize_relpath("a/../../b").is_err());
        assert!(sanitize_relpath("").is_err());
        assert_eq!(sanitize_relpath("weather.json").unwrap(), std::path::PathBuf::from("weather.json"));
        assert_eq!(sanitize_relpath("./sub/x").unwrap(), std::path::PathBuf::from("sub/x"));
    }

    #[test]
    fn normalize_abs_requires_absolute_and_rejects_parent() {
        assert!(normalize_abs("relative/x").is_err());
        assert!(normalize_abs("/ok/../escape").is_err());
        assert_eq!(normalize_abs("/var/lib/x").unwrap(), "/var/lib/x");
    }

    #[test]
    fn dir_size_sums_files() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("a"), b"12345").unwrap();
        fs::create_dir(d.path().join("sub")).unwrap();
        fs::write(d.path().join("sub/b"), b"678").unwrap();
        assert_eq!(dir_size(d.path()), 8);
    }

    #[test]
    fn check_cap_refuses_over_and_allows_replace_within() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("f");
        fs::write(&target, b"aaaa").unwrap(); // 4 bytes existing
        // replacing 4 bytes with 6 -> projected 6 <= cap 8 OK
        assert!(check_cap(d.path(), &target, 6, 8).is_ok());
        // a brand-new 10-byte file on top of the existing 4 -> 14 > cap 8
        let other = d.path().join("g");
        assert!(check_cap(d.path(), &other, 10, 8).is_err());
    }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p rustline-wasm state::`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `state.rs`**

```rust
//! Filesystem sandboxing + state-dir quota accounting. Pure helpers used by
//! the state/file host functions.

use std::path::{Component, Path, PathBuf};

/// Sanitize a plugin-supplied relative path for use under its own state dir.
/// Rejects absolute paths and any `..` traversal; strips `.`; requires a
/// non-empty result.
pub fn sanitize_relpath(relpath: &str) -> Result<PathBuf, String> {
    let p = Path::new(relpath);
    if p.is_absolute() {
        return Err("absolute path not allowed".into());
    }
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("path traversal not allowed".into());
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err("empty path".into());
    }
    Ok(out)
}

/// Normalize an absolute path for allowlist matching: require absolute, reject
/// any `..` component. Returns the path as a string (matched against globs).
pub fn normalize_abs(path: &str) -> Result<String, String> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err("path must be absolute".into());
    }
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("path traversal not allowed".into());
    }
    Ok(path.to_string())
}

/// Total size in bytes of all regular files under `dir` (0 if absent).
pub fn dir_size(dir: &Path) -> u64 {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// Ok iff writing `new_len` bytes to `target` (possibly replacing an existing
/// file) keeps `dir`'s total within `cap`.
pub fn check_cap(dir: &Path, target: &Path, new_len: u64, cap: u64) -> Result<(), String> {
    let current = dir_size(dir);
    let replaced = std::fs::metadata(target).map(|m| m.len()).unwrap_or(0);
    let projected = current.saturating_sub(replaced).saturating_add(new_len);
    if projected > cap {
        Err("state quota exceeded".into())
    } else {
        Ok(())
    }
}
```

- [ ] **Step 4: Implement `paths.rs`** (no separate test; covered indirectly + trivial)

```rust
//! XDG path resolution for the plugin dir and per-plugin state dirs, plus
//! `~/` expansion. All under `$XDG_DATA_HOME/rustline` (fallback
//! `$HOME/.local/share/rustline`) per the design.

use std::path::PathBuf;

/// Expand a leading `~/` to `$HOME`; otherwise return the path as-is.
pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
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
```

Add to `lib.rs`: `pub mod paths;` and `pub mod state;`.

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p rustline-wasm state::`
Expected: PASS (4 tests).

- [ ] **Step 6: fmt + clippy + commit**

```bash
cargo fmt --all
cargo clippy -p rustline-wasm --all-targets -- -D warnings
git add crates/rustline-wasm/src/state.rs crates/rustline-wasm/src/paths.rs crates/rustline-wasm/src/lib.rs Cargo.lock
git commit -m "feat(wasm): path sandbox, state-dir quota accounting, XDG path helpers"
```

---

### Task 4: Capability context, ABI result types, and gated `perform_*` logic

**Files:**
- Create: `crates/rustline-wasm/src/abi.rs`, `crates/rustline-wasm/src/capability.rs`, `crates/rustline-wasm/src/fetch.rs`, `crates/rustline-wasm/src/perform.rs`
- Modify: `crates/rustline-wasm/Cargo.toml` (add `ureq`), `crates/rustline-wasm/src/lib.rs`

**Interfaces:**
- Consumes: `allow::AllowSet`, `state::*`, `paths::*`, `rustline_core::{Context, Segment, PluginConfig}`.
- Produces:
  - `abi::HttpResult`, `abi::ReadResult`, `abi::WriteResult` (all `Serialize + Deserialize + Default`), `abi::RenderInput<'a>`, `abi::parse_render_output(&str) -> Vec<Segment>`.
  - `capability::CapabilityCtx { name, allowed_urls, allowed_paths, state_root, max_state_bytes }`, `CapabilityCtx::from_config(&str, &PluginConfig, PathBuf) -> Self`, `CapabilityCtx::state_dir(&self) -> PathBuf`.
  - `fetch::Fetcher` trait (`fn get(&self, &str) -> Result<(u16, String), String>`), `fetch::UreqFetcher`.
  - `perform::perform_http_get(&CapabilityCtx, &str, &dyn Fetcher) -> HttpResult`; `perform_state_read/state_write/file_read/file_write`.

- [ ] **Step 1: Add `ureq`** to `crates/rustline-wasm/Cargo.toml` dependencies

```toml
ureq = { version = "2", default-features = false, features = ["tls", "json"] }
```

(`tls` = rustls in ureq 2.x — keeps the graph OpenSSL-free.)

- [ ] **Step 2: Implement `abi.rs`**

```rust
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
```

- [ ] **Step 3: Implement `capability.rs`**

```rust
//! Per-plugin capability context: the *instance's* allowlists, state root, and
//! quota. Built from `PluginConfig`; stored in Extism `UserData` so each plugin
//! only ever sees its own grants (per-plugin scoping).

use std::path::PathBuf;

use rustline_core::PluginConfig;

use crate::allow::AllowSet;
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
```

- [ ] **Step 4: Implement `fetch.rs`**

```rust
//! The HTTP seam. `perform_http_get` takes a `Fetcher` so its gating logic is
//! testable without network; `UreqFetcher` is the real blocking rustls client.

use std::time::Duration;

/// A blocking HTTP GET. Returns `(status, body)` on a completed response
/// (including non-2xx), or `Err(message)` on transport failure.
pub trait Fetcher {
    fn get(&self, url: &str) -> Result<(u16, String), String>;
}

pub struct UreqFetcher;

impl Fetcher for UreqFetcher {
    fn get(&self, url: &str) -> Result<(u16, String), String> {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(5))
            .build();
        match agent.get(url).call() {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.into_string().map_err(|e| e.to_string())?;
                Ok((status, body))
            }
            Err(ureq::Error::Status(code, resp)) => {
                Ok((code, resp.into_string().unwrap_or_default()))
            }
            Err(e) => Err(e.to_string()),
        }
    }
}
```

> **Implementer note:** confirm the ureq 2.x API before relying on it — `AgentBuilder::timeout` (overall deadline) exists in ureq ≥ 2.5; if the resolved version lacks it, use `.timeout_read(Duration)` + `.timeout_connect(Duration)`. `resp.status() -> u16`, `resp.into_string() -> io::Result<String>`, and `ureq::Error::Status(u16, Response)` are stable across 2.x.

- [ ] **Step 5: Write failing tests** in `crates/rustline-wasm/src/perform.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityCtx;
    use rustline_core::PluginConfig;

    struct FakeFetcher(u16, &'static str);
    impl crate::fetch::Fetcher for FakeFetcher {
        fn get(&self, _url: &str) -> Result<(u16, String), String> {
            Ok((self.0, self.1.to_string()))
        }
    }
    struct DeadFetcher;
    impl crate::fetch::Fetcher for DeadFetcher {
        fn get(&self, _url: &str) -> Result<(u16, String), String> {
            Err("connection refused".into())
        }
    }

    fn ctx_with(urls: &[&str], root: std::path::PathBuf) -> CapabilityCtx {
        let pc = PluginConfig {
            allowed_urls: urls.iter().map(|s| s.to_string()).collect(),
            max_state_bytes: 16,
            ..PluginConfig::default()
        };
        CapabilityCtx::from_config("weather", &pc, root)
    }

    #[test]
    fn http_denied_when_not_allowlisted_makes_no_request() {
        let ctx = ctx_with(&[], std::env::temp_dir());
        let r = perform_http_get(&ctx, "https://wttr.in/48183", &FakeFetcher(200, "hi"));
        assert!(!r.ok);
        assert!(r.error.contains("not allowed"));
    }

    #[test]
    fn http_allowed_returns_body() {
        let ctx = ctx_with(&["https://wttr.in/*"], std::env::temp_dir());
        let r = perform_http_get(&ctx, "https://wttr.in/48183", &FakeFetcher(200, "sunny"));
        assert!(r.ok);
        assert_eq!(r.status, 200);
        assert_eq!(r.body, "sunny");
    }

    #[test]
    fn http_transport_error_reports_not_ok() {
        let ctx = ctx_with(&["https://wttr.in/*"], std::env::temp_dir());
        let r = perform_http_get(&ctx, "https://wttr.in/48183", &DeadFetcher);
        assert!(!r.ok);
        assert!(r.error.contains("refused"));
    }

    #[test]
    fn state_write_then_read_roundtrips_and_enforces_cap() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with(&[], root.path().to_path_buf());
        let w = perform_state_write(&ctx, "weather.json", "0123456789"); // 10 bytes, cap 16
        assert!(w.ok, "{:?}", w.error);
        let r = perform_state_read(&ctx, "weather.json");
        assert!(r.ok && r.exists);
        assert_eq!(r.contents, "0123456789");
        // read of an absent file: ok but exists=false
        let miss = perform_state_read(&ctx, "nope.json");
        assert!(miss.ok && !miss.exists);
        // a second big write over cap is refused
        let over = perform_state_write(&ctx, "big.json", "0123456789ABCDEF01"); // 18 > 16
        assert!(!over.ok);
        assert!(over.error.contains("quota"));
    }

    #[test]
    fn state_write_rejects_traversal() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with(&[], root.path().to_path_buf());
        let w = perform_state_write(&ctx, "../escape", "x");
        assert!(!w.ok);
        assert!(w.error.contains("traversal"));
    }

    #[test]
    fn file_read_denied_when_not_allowlisted() {
        let ctx = ctx_with(&[], std::env::temp_dir());
        let r = perform_file_read(&ctx, "/etc/hostname");
        assert!(!r.ok);
        assert!(r.error.contains("not allowed"));
    }
}
```

- [ ] **Step 6: Run to verify fail**

Run: `cargo test -p rustline-wasm perform::`
Expected: FAIL to compile.

- [ ] **Step 7: Implement `perform.rs`** (above the tests)

```rust
//! The capability-checked effect functions. Each returns a structured result
//! and never panics — the host_fn wrappers just serialize these to JSON.

use crate::abi::{HttpResult, ReadResult, WriteResult};
use crate::capability::CapabilityCtx;
use crate::fetch::Fetcher;
use crate::state::{check_cap, normalize_abs, sanitize_relpath};

pub fn perform_http_get(ctx: &CapabilityCtx, url: &str, fetcher: &dyn Fetcher) -> HttpResult {
    if !ctx.allowed_urls.allows(url) {
        return HttpResult {
            ok: false,
            error: format!("url not allowed: {url}"),
            ..Default::default()
        };
    }
    match fetcher.get(url) {
        Ok((status, body)) => HttpResult { ok: true, status, body, error: String::new() },
        Err(error) => HttpResult { ok: false, error, ..Default::default() },
    }
}

pub fn perform_state_read(ctx: &CapabilityCtx, relpath: &str) -> ReadResult {
    let rel = match sanitize_relpath(relpath) {
        Ok(r) => r,
        Err(error) => return ReadResult { ok: false, error, ..Default::default() },
    };
    let full = ctx.state_dir().join(rel);
    match std::fs::read_to_string(&full) {
        Ok(contents) => ReadResult { ok: true, exists: true, contents, error: String::new() },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            ReadResult { ok: true, exists: false, ..Default::default() }
        }
        Err(e) => ReadResult { ok: false, error: e.to_string(), ..Default::default() },
    }
}

pub fn perform_state_write(ctx: &CapabilityCtx, relpath: &str, contents: &str) -> WriteResult {
    let rel = match sanitize_relpath(relpath) {
        Ok(r) => r,
        Err(error) => return WriteResult { ok: false, error },
    };
    let dir = ctx.state_dir();
    let full = dir.join(rel);
    if let Err(error) = check_cap(&dir, &full, contents.len() as u64, ctx.max_state_bytes) {
        return WriteResult { ok: false, error };
    }
    if let Some(parent) = full.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return WriteResult { ok: false, error: e.to_string() };
        }
    }
    match std::fs::write(&full, contents.as_bytes()) {
        Ok(()) => WriteResult { ok: true, error: String::new() },
        Err(e) => WriteResult { ok: false, error: e.to_string() },
    }
}

pub fn perform_file_read(ctx: &CapabilityCtx, path: &str) -> ReadResult {
    let norm = match normalize_abs(path) {
        Ok(p) => p,
        Err(error) => return ReadResult { ok: false, error, ..Default::default() },
    };
    if !ctx.allowed_paths.allows(&norm) {
        return ReadResult { ok: false, error: format!("path not allowed: {norm}"), ..Default::default() };
    }
    match std::fs::read_to_string(&norm) {
        Ok(contents) => ReadResult { ok: true, exists: true, contents, error: String::new() },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            ReadResult { ok: true, exists: false, ..Default::default() }
        }
        Err(e) => ReadResult { ok: false, error: e.to_string(), ..Default::default() },
    }
}

pub fn perform_file_write(ctx: &CapabilityCtx, path: &str, contents: &str) -> WriteResult {
    let norm = match normalize_abs(path) {
        Ok(p) => p,
        Err(error) => return WriteResult { ok: false, error },
    };
    if !ctx.allowed_paths.allows(&norm) {
        return WriteResult { ok: false, error: format!("path not allowed: {norm}") };
    }
    match std::fs::write(&norm, contents.as_bytes()) {
        Ok(()) => WriteResult { ok: true, error: String::new() },
        Err(e) => WriteResult { ok: false, error: e.to_string() },
    }
}
```

Add to `lib.rs`: `pub mod abi; pub mod capability; pub mod fetch; pub mod perform;`.

- [ ] **Step 8: Run to verify pass + graph clean**

Run: `cargo test -p rustline-wasm` then `cargo tree -i openssl` and `cargo tree -i native-tls`
Expected: tests PASS; both `cargo tree` invocations print nothing (empty).

- [ ] **Step 9: fmt + clippy + commit**

```bash
cargo fmt --all
cargo clippy -p rustline-wasm --all-targets -- -D warnings
git add crates/rustline-wasm Cargo.lock
git commit -m "feat(wasm): capability ctx, wire types, and capability-gated effect fns"
```

---

### Task 5: Extism host-fn wiring, `WasmWidget`, and `register_plugins`

**Files:**
- Create: `crates/rustline-wasm/src/host.rs`
- Modify: `crates/rustline-wasm/Cargo.toml` (add `extism`), `crates/rustline-wasm/src/lib.rs`, `crates/rustline-core/src/widget.rs` (add `Registry::contains`)

**Interfaces:**
- Consumes: `perform::*`, `capability::CapabilityCtx`, `abi::*`, `fetch::UreqFetcher`, `paths::state_root`.
- Produces: `host::build_plugin(&[u8], CapabilityCtx) -> Result<extism::Plugin, extism::Error>`; `host::WasmWidget` (impl `rustline_core::Widget`, `Clone`); `lib::register_plugins(&mut Registry, &Config, &Path, &[String])`; core `Registry::contains(&self, &str) -> bool`.

- [ ] **Step 1: Add `Registry::contains`** to `crates/rustline-core/src/widget.rs` with a test.

In `impl Registry`, add:

```rust
    /// Whether a widget is already registered under `name`.
    pub fn contains(&self, name: &str) -> bool {
        self.factories.contains_key(name)
    }
```

Add to that file's `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn contains_reports_registration() {
        let mut r = Registry::new();
        assert!(!r.contains("a"));
        r.register("a", Box::new(|| Box::new(Fixed("A"))));
        assert!(r.contains("a"));
    }
```

Run: `cargo test -p rustline-core widget::tests::contains_reports_registration` → PASS.

- [ ] **Step 2: Add `extism`** to `crates/rustline-wasm/Cargo.toml`

```toml
extism = { version = "1", default-features = false }
```

> `default-features = false` avoids Extism's optional built-in HTTP client — we provide our own `rl_http_get`. Confirm the crate still builds; if a required feature was disabled, re-enable only the runtime (not `http`).

- [ ] **Step 3: Write failing tests** in `crates/rustline-wasm/src/host.rs` (hermetic — no valid wasm needed)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::parse_render_output;
    use rustline_core::{Config, Registry};

    #[test]
    fn parse_output_degrades_on_malformed() {
        assert!(parse_render_output("not json").is_empty());
        let good = r#"[{"text":"x","style":{"fg":null,"bg":null,"bold":false}}]"#;
        assert_eq!(parse_render_output(good).len(), 1);
    }

    #[test]
    fn register_plugins_missing_dir_is_noop() {
        let mut reg = Registry::new();
        crate::register_plugins(&mut reg, &Config::default(), std::path::Path::new("/no/such/dir"), &["weather".into()]);
        assert!(!reg.contains("weather"));
    }

    #[test]
    fn register_plugins_skips_not_needed_and_garbage_wasm() {
        let dir = tempfile::tempdir().unwrap();
        // a garbage .wasm that IS needed -> instantiation fails -> skipped, no panic
        std::fs::write(dir.path().join("weather.wasm"), b"not real wasm").unwrap();
        // a .wasm that is NOT in `needed` -> never touched
        std::fs::write(dir.path().join("other.wasm"), b"nope").unwrap();
        let mut reg = Registry::new();
        crate::register_plugins(&mut reg, &Config::default(), dir.path(), &["weather".into()]);
        assert!(!reg.contains("weather"));
        assert!(!reg.contains("other"));
    }
}
```

- [ ] **Step 4: Run to verify fail**

Run: `cargo test -p rustline-wasm host::`
Expected: FAIL to compile.

- [ ] **Step 5: Implement `host.rs`**

```rust
//! Extism instantiation: bind the capability-gated host functions to each
//! plugin instance's `CapabilityCtx`, and wrap the instance as a `Widget` that
//! degrades to empty segments on any error.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use extism::{Manifest, PluginBuilder, UserData, Wasm, PTR, host_fn};
use rustline_core::{Context, Segment, Widget};

use crate::abi::{RenderInput, parse_render_output};
use crate::capability::CapabilityCtx;
use crate::fetch::UreqFetcher;
use crate::perform::{
    perform_file_read, perform_file_write, perform_http_get, perform_state_read,
    perform_state_write,
};

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string())
}

host_fn!(rl_http_get(user_data: CapabilityCtx; url: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    Ok(json(&perform_http_get(&ctx, &url, &UreqFetcher)))
});

host_fn!(rl_state_read(user_data: CapabilityCtx; relpath: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    Ok(json(&perform_state_read(&ctx, &relpath)))
});

host_fn!(rl_state_write(user_data: CapabilityCtx; relpath: String, contents: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    Ok(json(&perform_state_write(&ctx, &relpath, &contents)))
});

host_fn!(rl_file_read(user_data: CapabilityCtx; path: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    Ok(json(&perform_file_read(&ctx, &path)))
});

host_fn!(rl_file_write(user_data: CapabilityCtx; path: String, contents: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    Ok(json(&perform_file_write(&ctx, &path, &contents)))
});

/// Build an Extism plugin from wasm bytes with wasi off, fuel + timeout +
/// memory caps, and the five capability-gated host functions bound to this
/// instance's `CapabilityCtx`.
pub fn build_plugin(wasm: &[u8], ctx: CapabilityCtx) -> Result<extism::Plugin, extism::Error> {
    let ud = UserData::new(ctx);
    let manifest = Manifest::new([Wasm::data(wasm.to_vec())])
        .with_timeout(Duration::from_secs(10))
        .with_memory_max(256); // 256 pages ≈ 16 MB
    PluginBuilder::new(manifest)
        .with_wasi(false)
        .with_fuel_limit(500_000_000)
        .with_function("rl_http_get", [PTR], [PTR], ud.clone(), rl_http_get)
        .with_function("rl_state_read", [PTR], [PTR], ud.clone(), rl_state_read)
        .with_function("rl_state_write", [PTR, PTR], [PTR], ud.clone(), rl_state_write)
        .with_function("rl_file_read", [PTR], [PTR], ud.clone(), rl_file_read)
        .with_function("rl_file_write", [PTR, PTR], [PTR], ud.clone(), rl_file_write)
        .build()
}

/// A discovered WASM plugin, rendered as a widget. Cheap to clone (shares the
/// instance behind an `Arc<Mutex<…>>`); any error/timeout/malformed output
/// degrades to empty segments so a plugin never breaks the bar.
#[derive(Clone)]
pub struct WasmWidget {
    plugin: Arc<Mutex<extism::Plugin>>,
    options: Arc<serde_json::Value>,
}

impl WasmWidget {
    pub fn new(plugin: extism::Plugin, options: serde_json::Value) -> Self {
        Self { plugin: Arc::new(Mutex::new(plugin)), options: Arc::new(options) }
    }
}

impl Widget for WasmWidget {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let input = RenderInput { context: ctx, config: &self.options };
        let payload = match serde_json::to_string(&input) {
            Ok(p) => p,
            Err(error) => {
                tracing::warn!(%error, "failed to serialize render input");
                return Vec::new();
            }
        };
        let mut plugin = match self.plugin.lock() {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };
        match plugin.call::<&str, &str>("render", &payload) {
            Ok(out) => parse_render_output(out),
            Err(error) => {
                tracing::warn!(%error, "plugin render failed, rendering empty");
                Vec::new()
            }
        }
    }
}
```

> **Implementer note (Send/Sync):** the registry factory closure must be `Fn() -> Box<dyn Widget> + Send + Sync`. `WasmWidget` is `Send + Sync` because `extism::Plugin: Send` and `Arc<Mutex<Plugin>>: Send + Sync`. If the compiler reports `Plugin` is not `Send`, wrap construction so the plugin is built and used on one thread — but current Extism 1.x `Plugin` is `Send`.

- [ ] **Step 6: Implement `register_plugins`** in `lib.rs`

```rust
pub mod abi;
pub mod allow;
pub mod capability;
pub mod fetch;
pub mod host;
pub mod paths;
pub mod perform;
pub mod state;

use std::path::Path;
use std::sync::Arc;

use rustline_core::{Config, Registry};

pub use host::{WasmWidget, build_plugin};
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
        let widget = host::WasmWidget::new(plugin, options);
        let shared = Arc::new(widget);
        reg.register(stem, Box::new(move || Box::new((*shared).clone())));
    }
}
```

- [ ] **Step 7: Run to verify pass**

Run: `cargo test -p rustline-wasm`
Expected: PASS (incl. the three `host::` tests and all earlier ones).

- [ ] **Step 8: fmt + clippy + commit**

```bash
cargo fmt --all
cargo clippy -p rustline-core -p rustline-wasm --all-targets -- -D warnings
git add crates/rustline-core/src/widget.rs crates/rustline-wasm Cargo.lock
git commit -m "feat(wasm): Extism host fns, WasmWidget, and plugin discovery/registration"
```

---

### Task 6: Bin integration — `--plugin-dir` and register plugins into the render pipeline

**Files:**
- Modify: `crates/rustline/Cargo.toml` (add `rustline-wasm`), `crates/rustline/src/cli.rs`, `crates/rustline/src/main.rs`, `crates/rustline/tests/smoke.rs`

**Interfaces:**
- Consumes: `rustline_wasm::{register_plugins, default_plugin_dir, expand_tilde}`.
- Produces: `RegionArgs.plugin_dir: Option<String>`; plugin-dir resolution (flag › config › default) in `main`.

- [ ] **Step 1: Add the dependency** to `crates/rustline/Cargo.toml`

```toml
rustline-wasm = { path = "../rustline-wasm" }
```

- [ ] **Step 2: Add `--plugin-dir`** to `RegionArgs` in `cli.rs` (append a field)

```rust
    /// Override the plugin discovery directory (default
    /// `$XDG_DATA_HOME/rustline/plugins`, or config `plugin_dir`).
    #[arg(long)]
    pub plugin_dir: Option<String>,
```

- [ ] **Step 3: Write the failing smoke test** — append to `crates/rustline/tests/smoke.rs`

```rust
#[test]
fn render_right_with_missing_plugin_degrades_gracefully() {
    // A layout naming a plugin with no .wasm present must not crash: the bar
    // still renders the built-in widgets and exits 0.
    let dir = std::env::temp_dir().join("rustline_smoke_pluginless");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("config.toml");
    std::fs::write(&cfg, "[layout]\nright = [\"datetime\", \"weather\"]\n").unwrap();
    let empty_plugins = dir.join("plugins_empty");
    std::fs::create_dir_all(&empty_plugins).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_rustline"))
        .args(["render", "right", "--plugin-dir"])
        .arg(&empty_plugins)
        .env("XDG_CONFIG_HOME", &dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "exit ok; stderr={}", String::from_utf8_lossy(&out.stderr));
    // datetime still renders (contains tmux style markup)
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("#["), "built-ins still render: {s}");
}
```

> Note: `XDG_CONFIG_HOME=<dir>` makes the binary load `<dir>/rustline/config.toml`. Create that path:

Adjust the test to write the config where `config_path()` looks: `dir/rustline/config.toml`.

```rust
    let cfgdir = dir.join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
```

(Use this corrected path in the test.)

- [ ] **Step 4: Run to verify fail**

Run: `cargo test -p rustline --test smoke render_right_with_missing_plugin_degrades_gracefully`
Expected: FAIL (unknown `--plugin-dir` until Step 2 is built, then FAIL because plugins aren't wired yet / or passes trivially — verify it compiles and runs).

- [ ] **Step 5: Wire registration** in `main.rs`. Add a helper and call it before each region render.

Add import: `use std::path::Path;` (if not present) and use `rustline_wasm`.

Add helper:

```rust
/// Resolve the plugin dir: `--plugin-dir` flag › config `plugin_dir` › default.
fn resolve_plugin_dir(flag: Option<&str>, cfg: &Config) -> PathBuf {
    if let Some(f) = flag {
        return rustline_wasm::expand_tilde(f);
    }
    if let Some(d) = &cfg.plugin_dir {
        return rustline_wasm::expand_tilde(d);
    }
    rustline_wasm::default_plugin_dir()
}
```

Make `registry` mutable and register plugins for the region being rendered. Update the `Render::Left`/`Render::Right` arms:

```rust
        Command::Render(Render::Left(args)) => {
            let plugin_dir = resolve_plugin_dir(args.plugin_dir.as_deref(), &cfg);
            let mut registry = Registry::with_builtins(&cfg);
            rustline_wasm::register_plugins(&mut registry, &cfg, &plugin_dir, &cfg.layout.left);
            let ctx = build_region_context(&args);
            let out = render_named_region(Direction::Left, &cfg.layout.left, &ctx, &registry, &theme);
            emit(&out, args.preview);
        }
        Command::Render(Render::Right(args)) => {
            let plugin_dir = resolve_plugin_dir(args.plugin_dir.as_deref(), &cfg);
            let mut registry = Registry::with_builtins(&cfg);
            rustline_wasm::register_plugins(&mut registry, &cfg, &plugin_dir, &cfg.layout.right);
            let ctx = build_region_context(&args);
            let out = render_named_region(Direction::Right, &cfg.layout.right, &ctx, &registry, &theme);
            emit(&out, args.preview);
        }
```

Remove the now-unused top-level `let registry = Registry::with_builtins(&cfg);` if it is only used by these arms; the `Window` arm and others build their own registry as needed (the `Window` arm uses `render_window(&ctx, &registry, &theme)` — give it its own `let registry = Registry::with_builtins(&cfg);` locally, since windows don't run plugins in v1).

- [ ] **Step 6: Run to verify pass**

Run: `cargo test -p rustline --test smoke`
Expected: PASS (all smoke tests, incl. the new one).

- [ ] **Step 7: fmt + clippy + commit**

```bash
cargo fmt --all
cargo clippy -p rustline --all-targets -- -D warnings
git add crates/rustline/Cargo.toml crates/rustline/src/cli.rs crates/rustline/src/main.rs crates/rustline/tests/smoke.rs Cargo.lock
git commit -m "feat(cli): discover + register WASM plugins per region; --plugin-dir flag"
```

---

### Task 7: `rustline plugin` CLI subcommands (toml_edit)

**Files:**
- Modify: `crates/rustline/Cargo.toml` (add `toml_edit`), `crates/rustline/src/cli.rs`, `crates/rustline/src/main.rs`
- Create: `crates/rustline/src/plugin_cmd.rs`
- Modify: `crates/rustline/tests/smoke.rs`

**Interfaces:**
- Consumes: `Config`, `config_path()`.
- Produces: `Command::Plugin(PluginCmd)`; `plugin_cmd::run(cmd, &Path)`; functions `list`, `list_patterns`, `add_pattern`, `remove_pattern`.

- [ ] **Step 1: Add `toml_edit`** to `crates/rustline/Cargo.toml`

```toml
toml_edit = "0.22"
```

- [ ] **Step 2: Add the CLI surface** to `cli.rs`

```rust
/// Manage discovered plugins and their capability allowlists.
#[derive(Subcommand)]
pub enum PluginCmd {
    /// List configured plugins and their allowlists/caps.
    List,
    /// Manage a plugin's URL allowlist.
    #[command(subcommand)]
    Url(PatternCmd),
    /// Manage a plugin's filesystem-path allowlist.
    #[command(subcommand)]
    Path(PatternCmd),
}

/// list/add/remove operations over one allowlist of a named plugin.
#[derive(Subcommand)]
pub enum PatternCmd {
    /// List the plugin's patterns.
    List { plugin: String },
    /// Append a pattern (idempotent).
    Add { plugin: String, pattern: String },
    /// Remove an exact-match pattern.
    Remove { plugin: String, pattern: String },
}
```

Add to `Command`:

```rust
    /// Manage plugins and their capability allowlists.
    #[command(subcommand)]
    Plugin(PluginCmd),
```

- [ ] **Step 3: Write failing tests** — append to `crates/rustline/tests/smoke.rs`

```rust
#[test]
fn plugin_url_add_remove_roundtrips_preserving_comments() {
    let dir = std::env::temp_dir().join("rustline_smoke_pluginedit");
    let cfgdir = dir.join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    std::fs::write(&cfg, "# keepme\n[plugins.weather]\nallowed_urls = []\n").unwrap();

    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_rustline"))
            .args(args)
            .env("XDG_CONFIG_HOME", &dir)
            .output()
            .unwrap()
    };

    assert!(run(&["plugin", "url", "add", "weather", "https://wttr.in/*"]).status.success());
    let after_add = std::fs::read_to_string(&cfg).unwrap();
    assert!(after_add.contains("# keepme"), "comment preserved: {after_add}");
    assert!(after_add.contains("https://wttr.in/*"), "pattern added: {after_add}");

    // idempotent add
    assert!(run(&["plugin", "url", "add", "weather", "https://wttr.in/*"]).status.success());
    let dup = std::fs::read_to_string(&cfg).unwrap();
    assert_eq!(dup.matches("https://wttr.in/*").count(), 1, "no duplicate: {dup}");

    assert!(run(&["plugin", "url", "remove", "weather", "https://wttr.in/*"]).status.success());
    let after_rm = std::fs::read_to_string(&cfg).unwrap();
    assert!(!after_rm.contains("https://wttr.in/*"), "pattern removed: {after_rm}");
    assert!(after_rm.contains("# keepme"), "comment still there: {after_rm}");
}
```

- [ ] **Step 4: Run to verify fail**

Run: `cargo test -p rustline --test smoke plugin_url_add_remove_roundtrips_preserving_comments`
Expected: FAIL to compile / unknown subcommand.

- [ ] **Step 5: Implement `plugin_cmd.rs`**

```rust
//! `rustline plugin …` — list plugins and edit their capability allowlists.
//! Mutations use `toml_edit` so the user's comments and formatting survive.

use std::path::Path;

use rustline_core::Config;
use toml_edit::{Array, DocumentMut, Item, Table, Value};

use crate::cli::{PatternCmd, PluginCmd};

/// Which allowlist array a pattern command targets.
enum Kind {
    Url,
    Path,
}

impl Kind {
    fn key(&self) -> &'static str {
        match self {
            Kind::Url => "allowed_urls",
            Kind::Path => "allowed_paths",
        }
    }
}

pub fn run(cmd: PluginCmd, config_path: &Path) {
    match cmd {
        PluginCmd::List => list(config_path),
        PluginCmd::Url(pc) => pattern_cmd(pc, Kind::Url, config_path),
        PluginCmd::Path(pc) => pattern_cmd(pc, Kind::Path, config_path),
    }
}

fn list(config_path: &Path) {
    let cfg = Config::load(config_path);
    if cfg.plugins.is_empty() {
        println!("no plugins configured");
        return;
    }
    for (name, pc) in &cfg.plugins {
        println!("{name}");
        if let Some(src) = &pc.source {
            println!("  source: {src}");
        }
        println!("  allowed_urls: {:?}", pc.allowed_urls);
        println!("  allowed_paths: {:?}", pc.allowed_paths);
        println!("  max_state_bytes: {}", pc.max_state_bytes);
    }
}

fn pattern_cmd(cmd: PatternCmd, kind: Kind, config_path: &Path) {
    match cmd {
        PatternCmd::List { plugin } => {
            let cfg = Config::load(config_path);
            let patterns = cfg.plugins.get(&plugin).map(|p| match kind {
                Kind::Url => p.allowed_urls.clone(),
                Kind::Path => p.allowed_paths.clone(),
            });
            match patterns {
                Some(list) if !list.is_empty() => list.iter().for_each(|p| println!("{p}")),
                Some(_) => println!("(none)"),
                None => println!("no such plugin: {plugin}"),
            }
        }
        PatternCmd::Add { plugin, pattern } => mutate(config_path, &plugin, kind, |arr| {
            if !arr.iter().any(|v| v.as_str() == Some(&pattern)) {
                arr.push(pattern.as_str());
            }
        }),
        PatternCmd::Remove { plugin, pattern } => mutate(config_path, &plugin, kind, |arr| {
            arr.retain(|v| v.as_str() != Some(&pattern));
        }),
    }
}

fn mutate(config_path: &Path, plugin: &str, kind: Kind, f: impl FnOnce(&mut Array)) {
    let text = std::fs::read_to_string(config_path).unwrap_or_default();
    let mut doc: DocumentMut = text.parse().unwrap_or_default();

    // ensure [plugins.<plugin>] exists
    let plugins = doc
        .entry("plugins")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .expect("plugins is a table");
    plugins.set_implicit(true);
    let entry = plugins
        .entry(plugin)
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .expect("plugin entry is a table");

    // ensure the allowlist array exists
    let item = entry
        .entry(kind.key())
        .or_insert(Item::Value(Value::Array(Array::new())));
    let arr = item.as_array_mut().expect("allowlist is an array");
    f(arr);

    if let Err(error) = std::fs::write(config_path, doc.to_string()) {
        eprintln!("failed to write config: {error}");
        std::process::exit(1);
    }
}
```

- [ ] **Step 6: Dispatch** in `main.rs`. Add `mod plugin_cmd;`, then a match arm:

```rust
        Command::Plugin(cmd) => plugin_cmd::run(cmd, &config_path()),
```

Update imports: `use cli::{Cli, Command, PluginCmd, Render};` (add `PluginCmd`; `plugin_cmd::run` takes it).

- [ ] **Step 7: Run to verify pass**

Run: `cargo test -p rustline --test smoke`
Expected: PASS.

- [ ] **Step 8: fmt + clippy + commit**

```bash
cargo fmt --all
cargo clippy -p rustline --all-targets -- -D warnings
git add crates/rustline/Cargo.toml crates/rustline/src/cli.rs crates/rustline/src/main.rs crates/rustline/src/plugin_cmd.rs crates/rustline/tests/smoke.rs Cargo.lock
git commit -m "feat(cli): rustline plugin list/url/path subcommands (toml_edit, comment-preserving)"
```

---

### Task 8: Weather plugin guest crate (pure logic + guest glue)

**Files:**
- Create: `plugins/weather/Cargo.toml`, `plugins/weather/src/lib.rs`
- Modify: root `Cargo.toml` (exclude), `justfile` (build-weather)

**Interfaces:**
- Produces (pure, host-testable): `code_to_icon(&str) -> &'static str`; `render_format(fmt, icon, temp_f, conditions, zip) -> String`; `is_fresh(now_rfc3339, fetched_rfc3339, refresh_secs, cache_zip, want_zip) -> bool`; `parse_wttr(&str) -> Option<Wttr>` where `Wttr { temp_f, code, desc }`.
- Guest exports (wasm only): `name() -> "weather"`, `render(String) -> String`.

- [ ] **Step 1: Exclude from the root workspace** — root `Cargo.toml`

```toml
[workspace]
resolver = "2"
members = ["crates/rustline-core", "crates/rustline", "crates/rustline-wasm"]
exclude = ["plugins/weather"]
```

- [ ] **Step 2: Create `plugins/weather/Cargo.toml`**

```toml
[package]
name = "weather"
version = "0.1.0"
edition = "2024"
license = "MIT"

[lib]
crate-type = ["cdylib"]

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", default-features = false, features = ["serde", "alloc"] }

# Guest bindings only compiled for the wasm target; host-side `cargo test`
# builds just the pure logic below.
[target.'cfg(target_arch = "wasm32")'.dependencies]
extism-pdk = "1"
```

- [ ] **Step 3: Write failing tests** in `plugins/weather/src/lib.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_maps_known_and_unknown_codes() {
        assert_eq!(code_to_icon("113"), "\u{e30d}"); // clear/sunny
        assert_eq!(code_to_icon("116"), "\u{e302}"); // partly cloudy
        assert_eq!(code_to_icon("296"), "\u{e318}"); // rain
        assert_eq!(code_to_icon("999"), "\u{e374}"); // unknown -> fallback (na)
    }

    #[test]
    fn format_substitutes_placeholders_and_passes_unknowns() {
        let out = render_format("{icon} {temp_f}°F {conditions} @{zip}", "☀", "72", "Sunny", "48183");
        assert_eq!(out, "☀ 72°F Sunny @48183");
        // unknown placeholder is left untouched
        assert_eq!(render_format("{bogus}", "i", "1", "c", "z"), "{bogus}");
    }

    #[test]
    fn freshness_respects_interval_and_zip() {
        let now = "2026-07-20T12:30:00-04:00";
        let recent = "2026-07-20T12:10:00-04:00"; // 20 min ago
        let old = "2026-07-20T11:00:00-04:00"; // 90 min ago
        assert!(is_fresh(now, recent, 1800, "48183", "48183")); // 20min < 30min
        assert!(!is_fresh(now, old, 1800, "48183", "48183")); // 90min > 30min
        assert!(!is_fresh(now, recent, 1800, "48183", "90210")); // zip changed
        assert!(!is_fresh(now, "garbage", 1800, "48183", "48183")); // unparseable
    }

    #[test]
    fn parse_wttr_extracts_current_condition() {
        let j = r#"{"current_condition":[{"temp_F":"72","weatherCode":"113","weatherDesc":[{"value":"Sunny"}]}]}"#;
        let w = parse_wttr(j).unwrap();
        assert_eq!(w.temp_f, "72");
        assert_eq!(w.code, "113");
        assert_eq!(w.desc, "Sunny");
        assert!(parse_wttr("{}").is_none());
    }
}
```

- [ ] **Step 4: Run to verify fail**

Run: `cargo test --manifest-path plugins/weather/Cargo.toml`
Expected: FAIL to compile.

- [ ] **Step 5: Implement the pure logic** (top of `plugins/weather/src/lib.rs`)

```rust
//! rustline `weather` plugin: shows a Nerd-Font condition icon + °F for a zip
//! code from wttr.in, cached to its own state dir (≤ 1 fetch / refresh_secs).
//!
//! The pure logic (icon map, formatting, freshness, JSON parse) is compiled and
//! unit-tested on the host; the Extism guest glue is wasm-only (see bottom).

use chrono::DateTime;
use serde::Deserialize;

/// Map a WWO `weatherCode` to a Nerd-Font weather glyph. Unknown → `nf-weather-na`.
pub fn code_to_icon(code: &str) -> &'static str {
    match code {
        "113" => "\u{e30d}",                                   // Sunny / Clear
        "116" => "\u{e302}",                                   // Partly cloudy
        "119" | "122" => "\u{e312}",                           // Cloudy / Overcast
        "143" | "248" | "260" => "\u{e313}",                   // Mist / Fog
        "176" | "263" | "266" | "281" | "284" | "293" | "296" | "299" | "302" | "305"
        | "308" | "311" | "314" | "353" | "356" | "359" => "\u{e318}", // Rain
        "200" | "386" | "389" | "392" | "395" => "\u{e31d}",   // Thundery
        "179" | "182" | "185" | "227" | "230" | "317" | "320" | "323" | "326" | "329"
        | "332" | "335" | "338" | "350" | "362" | "365" | "368" | "371" | "374"
        | "377" => "\u{e31a}",                                 // Snow / Sleet
        _ => "\u{e374}",                                        // na (unknown)
    }
}

/// Substitute `{icon}`, `{temp_f}`, `{conditions}`, `{zip}` in `fmt`. Unknown
/// placeholders pass through untouched.
pub fn render_format(fmt: &str, icon: &str, temp_f: &str, conditions: &str, zip: &str) -> String {
    fmt.replace("{icon}", icon)
        .replace("{temp_f}", temp_f)
        .replace("{conditions}", conditions)
        .replace("{zip}", zip)
}

/// A cache is fresh iff it is for the same zip and was fetched within
/// `refresh_secs` of `now`. Unparseable timestamps → not fresh (forces refetch).
pub fn is_fresh(
    now_rfc3339: &str,
    fetched_rfc3339: &str,
    refresh_secs: i64,
    cache_zip: &str,
    want_zip: &str,
) -> bool {
    if cache_zip != want_zip {
        return false;
    }
    let (Ok(now), Ok(fetched)) = (
        DateTime::parse_from_rfc3339(now_rfc3339),
        DateTime::parse_from_rfc3339(fetched_rfc3339),
    ) else {
        return false;
    };
    let age = now.timestamp() - fetched.timestamp();
    (0..refresh_secs).contains(&age)
}

/// Extracted wttr.in current conditions.
pub struct Wttr {
    pub temp_f: String,
    pub code: String,
    pub desc: String,
}

#[derive(Deserialize)]
struct WttrJson {
    current_condition: Vec<CurrentCondition>,
}
#[derive(Deserialize)]
struct CurrentCondition {
    temp_F: String,
    weatherCode: String,
    #[serde(default)]
    weatherDesc: Vec<DescVal>,
}
#[derive(Deserialize)]
struct DescVal {
    value: String,
}

/// Parse a wttr.in `format=j1` body into the current conditions.
pub fn parse_wttr(json: &str) -> Option<Wttr> {
    let parsed: WttrJson = serde_json::from_str(json).ok()?;
    let cc = parsed.current_condition.into_iter().next()?;
    Some(Wttr {
        temp_f: cc.temp_F,
        code: cc.weatherCode,
        desc: cc.weatherDesc.into_iter().next().map(|d| d.value).unwrap_or_default(),
    })
}
```

> The `temp_F`/`weatherCode` field names intentionally match wttr.in's JSON; add `#![allow(non_snake_case)]` at the crate top **or** use `#[serde(rename = "temp_F")]` on snake-case fields. Prefer the explicit renames to stay clippy-clean:
> ```rust
> #[derive(Deserialize)]
> struct CurrentCondition {
>     #[serde(rename = "temp_F")] temp_f: String,
>     #[serde(rename = "weatherCode")] code: String,
>     #[serde(rename = "weatherDesc", default)] desc: Vec<DescVal>,
> }
> ```
> and update `parse_wttr` to use `cc.temp_f`, `cc.code`, `cc.desc`.

- [ ] **Step 6: Run to verify pass**

Run: `cargo test --manifest-path plugins/weather/Cargo.toml`
Expected: PASS (4 tests).

- [ ] **Step 7: Add the wasm-only guest glue** (bottom of `lib.rs`)

```rust
#[cfg(target_arch = "wasm32")]
mod guest {
    use super::*;
    use extism_pdk::*;
    use serde_json::Value;

    #[host_fn]
    extern "ExtismHost" {
        fn rl_http_get(url: String) -> String;
        fn rl_state_read(relpath: String) -> String;
        fn rl_state_write(relpath: String, contents: String) -> String;
    }

    #[plugin_fn]
    pub fn name() -> FnResult<String> {
        Ok("weather".to_string())
    }

    #[plugin_fn]
    pub fn render(input: String) -> FnResult<String> {
        let v: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
        let now = v["context"]["now"].as_str().unwrap_or_default().to_string();
        let cfg = &v["config"];
        let zip = cfg["zip"].as_str().unwrap_or("48183").to_string();
        let format = cfg["format"].as_str().unwrap_or("{icon} {temp_f}°F").to_string();
        let refresh_secs = cfg["refresh_secs"].as_i64().unwrap_or(1800);
        let api_base = cfg["api_base"].as_str().unwrap_or("https://wttr.in").to_string();

        // 1) try fresh cache
        let cached = read_cache();
        if let Some((f_at, c_zip, temp_f, code, desc)) = &cached {
            if is_fresh(&now, f_at, refresh_secs, c_zip, &zip) {
                return Ok(segment(&format, code, temp_f, desc, &zip));
            }
        }

        // 2) fetch
        let url = format!("{api_base}/{zip}?format=j1");
        let fetched = unsafe { rl_http_get(url) }.ok().and_then(|r| {
            let hr: Value = serde_json::from_str(&r).ok()?;
            if hr["ok"].as_bool().unwrap_or(false) {
                parse_wttr(hr["body"].as_str().unwrap_or_default())
            } else {
                None
            }
        });

        match fetched {
            Some(w) => {
                write_cache(&now, &zip, &w);
                Ok(segment(&format, &w.code, &w.temp_f, &w.desc, &zip))
            }
            // 3) fetch failed: fall back to stale cache if any, else empty
            None => match cached {
                Some((_, _, temp_f, code, desc)) => Ok(segment(&format, &code, &temp_f, &desc, &zip)),
                None => Ok("[]".to_string()),
            },
        }
    }

    fn segment(format: &str, code: &str, temp_f: &str, desc: &str, zip: &str) -> String {
        let text = render_format(format, code_to_icon(code), temp_f, desc, zip);
        // one unstyled segment; the host assigns palette for left/right regions
        serde_json::json!([{ "text": text, "style": { "fg": null, "bg": null, "bold": false } }])
            .to_string()
    }

    fn read_cache() -> Option<(String, String, String, String, String)> {
        let r: Value = serde_json::from_str(&unsafe { rl_state_read("weather.json".into()) }.ok()?).ok()?;
        if !r["ok"].as_bool().unwrap_or(false) || !r["exists"].as_bool().unwrap_or(false) {
            return None;
        }
        let c: Value = serde_json::from_str(r["contents"].as_str().unwrap_or("{}")).ok()?;
        Some((
            c["fetched_at"].as_str()?.to_string(),
            c["zip"].as_str()?.to_string(),
            c["temp_f"].as_str()?.to_string(),
            c["code"].as_str()?.to_string(),
            c["desc"].as_str().unwrap_or_default().to_string(),
        ))
    }

    fn write_cache(now: &str, zip: &str, w: &Wttr) {
        let body = serde_json::json!({
            "fetched_at": now, "zip": zip,
            "temp_f": w.temp_f, "code": w.code, "desc": w.desc,
        })
        .to_string();
        let _ = unsafe { rl_state_write("weather.json".into(), body) };
    }
}
```

> **Implementer note:** the exact `#[host_fn]` extern call convention (`unsafe { … }`, return type `Result`) follows the extism-pdk 1.x macro. Verify against the pinned `extism-pdk` docs; the macro may generate the fns as returning `Result<String, extism_pdk::Error>` (hence `.ok()`), callable in an `unsafe` block. Adjust the `.ok()`/`unsafe` wrapping to match the generated signatures.

- [ ] **Step 8: Add the `just build-weather` recipe** to `justfile`

```make
# Build the example weather WASM plugin and install it into the plugin dir
build-weather:
    #!/usr/bin/env bash
    set -euo pipefail
    rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true
    cargo build --release --target wasm32-unknown-unknown --manifest-path plugins/weather/Cargo.toml
    dest="${XDG_DATA_HOME:-$HOME/.local/share}/rustline/plugins"
    mkdir -p "$dest"
    cp plugins/weather/target/wasm32-unknown-unknown/release/weather.wasm "$dest/weather.wasm"
    echo "installed weather.wasm -> $dest/weather.wasm"
```

- [ ] **Step 9: Verify the wasm build compiles** (not part of `just test`)

Run: `just build-weather`
Expected: builds `weather.wasm` and installs it; no errors.

- [ ] **Step 10: fmt + commit** (fmt only the workspace; the excluded crate is fmt'd separately)

```bash
cargo fmt --all
cargo fmt --manifest-path plugins/weather/Cargo.toml
git add plugins/weather/Cargo.toml plugins/weather/src/lib.rs Cargo.toml justfile
git commit -m "feat(weather): example WASM weather plugin (wttr.in, 30-min state cache)"
```

---

### Task 9: End-to-end WASM tests (behind `just test-wasm`)

**Files:**
- Create: `crates/rustline-wasm/tests/e2e.rs`
- Modify: `crates/rustline-wasm/Cargo.toml` (add `wasm-e2e` feature), `justfile` (add `test-wasm`)

**Interfaces:**
- Consumes: `rustline_wasm::{build_plugin, WasmWidget}`, `capability::CapabilityCtx`, a built `plugins/weather/…/weather.wasm`.

- [ ] **Step 1: Add the feature** to `crates/rustline-wasm/Cargo.toml`

```toml
[features]
wasm-e2e = []
```

- [ ] **Step 2: Write the e2e test** `crates/rustline-wasm/tests/e2e.rs` (uses a tiny std TCP mock — no async deps)

```rust
#![cfg(feature = "wasm-e2e")]
//! End-to-end: load the real weather.wasm, point it at a local mock wttr.in,
//! and assert the 30-min cache makes exactly one HTTP call, with stale fallback
//! on failure. Run via `just test-wasm` (requires the wasm target).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rustline_core::{Context, PluginConfig, Widget};
use rustline_wasm::capability::CapabilityCtx;
use rustline_wasm::{build_plugin, WasmWidget};

const WTTR_BODY: &str = r#"{"current_condition":[{"temp_F":"72","weatherCode":"113","weatherDesc":[{"value":"Sunny"}]}]}"#;

/// A one-shot-per-connection HTTP mock; counts hits.
fn spawn_mock(hits: Arc<AtomicUsize>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            hits.fetch_add(1, Ordering::SeqCst);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                WTTR_BODY.len(),
                WTTR_BODY
            );
            let _ = s.write_all(resp.as_bytes());
        }
    });
    format!("http://{addr}")
}

fn weather_wasm() -> Vec<u8> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/weather/target/wasm32-unknown-unknown/release/weather.wasm"
    );
    std::fs::read(path).expect("run `just build-weather` first")
}

fn ctx_now(rfc3339: &str) -> Context {
    Context {
        session_name: "0".into(), window_index: "0".into(), pane_index: "0".into(),
        pane_current_path: "/".into(), home: "/home/x".into(), hostname: "h".into(),
        loadavg: None,
        now: chrono::DateTime::parse_from_rfc3339(rfc3339).unwrap().with_timezone(&chrono::Local),
        window: None,
    }
}

fn build_widget(api_base: &str, state_root: std::path::PathBuf, zip: &str) -> WasmWidget {
    let pc = PluginConfig {
        allowed_urls: vec!["http://127.0.0.1:*/*".into()],
        ..PluginConfig::default()
    };
    let cap = CapabilityCtx::from_config("weather", &pc, state_root);
    let plugin = build_plugin(&weather_wasm(), cap).unwrap();
    let options = serde_json::json!({ "zip": zip, "api_base": api_base, "refresh_secs": 1800 });
    WasmWidget::new(plugin, options)
}

#[test]
fn caches_within_refresh_window_one_http_call() {
    let hits = Arc::new(AtomicUsize::new(0));
    let base = spawn_mock(hits.clone());
    let state = tempfile::tempdir().unwrap();

    let w = build_widget(&base, state.path().to_path_buf(), "48183");
    // first render: fetches + caches
    let s1 = w.render(&ctx_now("2026-07-20T12:00:00-04:00"));
    assert!(s1[0].text.contains("72"), "temp rendered: {:?}", s1);
    // second render 10 min later: served from cache, no new HTTP hit
    let s2 = w.render(&ctx_now("2026-07-20T12:10:00-04:00"));
    assert!(s2[0].text.contains("72"));
    assert_eq!(hits.load(Ordering::SeqCst), 1, "exactly one fetch within the window");
}

#[test]
fn stale_cache_used_when_fetch_fails() {
    let state = tempfile::tempdir().unwrap();
    // seed a stale cache directly in the plugin's state dir
    let dir = state.path().join("weather");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("weather.json"),
        r#"{"fetched_at":"2026-07-20T09:00:00-04:00","zip":"48183","temp_f":"55","code":"113","desc":"Sunny"}"#,
    )
    .unwrap();

    // point at a dead port so the fetch fails
    let w = build_widget("http://127.0.0.1:1", state.path().to_path_buf(), "48183");
    let s = w.render(&ctx_now("2026-07-20T15:00:00-04:00")); // 6h later -> stale, refetch attempted
    assert!(s[0].text.contains("55"), "fell back to stale cache: {:?}", s);
}
```

> Note: `CapabilityCtx` is referenced via `rustline_wasm::capability::CapabilityCtx`; the `capability` module is already `pub`. If the wasm allowlist glob `http://127.0.0.1:*/*` fails to match the random port, use `re:^http://127\.0\.0\.1:\d+/` instead.

- [ ] **Step 3: Add the `just test-wasm` recipe** to `justfile`

```make
# Build the weather plugin and run the end-to-end WASM host tests (opt-in)
test-wasm: build-weather
    cargo test -p rustline-wasm --features wasm-e2e --test e2e
```

- [ ] **Step 4: Run it**

Run: `just test-wasm`
Expected: PASS (2 e2e tests). The mock records exactly one hit in the caching test.

- [ ] **Step 5: Confirm `just test` is still hermetic**

Run: `just test`
Expected: PASS, and it does NOT build any wasm (the `wasm-e2e` cfg gate excludes `e2e.rs`).

- [ ] **Step 6: fmt + clippy + commit**

```bash
cargo fmt --all
cargo clippy -p rustline-wasm --all-targets -- -D warnings
git add crates/rustline-wasm/Cargo.toml crates/rustline-wasm/tests/e2e.rs justfile Cargo.lock
git commit -m "test(wasm): end-to-end weather plugin cache + stale-fallback (opt-in just test-wasm)"
```

---

### Task 10: Documentation

**Files:**
- Modify: `CLAUDE.md`, `README.md`

**Interfaces:** none (docs only).

- [ ] **Step 1: Update `CLAUDE.md`.** Make these concrete edits:
  - **Architecture list:** change the `rustline-wasm` bullet from "not built yet" to a description of the host (Extism runtime, capability-gated host fns, plugin discovery → Widget registration). Add `plugins/weather` as the example plugin (excluded workspace member, `wasm32-unknown-unknown`).
  - **Module map:** add a `rustline-wasm` subsection listing `allow.rs`, `state.rs`, `paths.rs`, `abi.rs`, `capability.rs`, `fetch.rs`, `perform.rs`, `host.rs`, `lib.rs::register_plugins`. Note the bin's new `plugin_cmd.rs` and `--plugin-dir`.
  - **CLI section:** add `rustline plugin list`, `rustline plugin url|path list|add|remove <plugin> [pattern]`, and the `--plugin-dir` flag on `render left|right`.
  - **Config section:** document the typed `[plugins.<name>]` table (`source`, `allowed_urls`, `allowed_paths`, `max_state_bytes`, `[plugins.<name>.options]`) and top-level `plugin_dir`. Note allow-pattern syntax (glob, or `re:` regex).
  - **Invariants:** add N1–N4 (zero ambient authority; a plugin never breaks the bar; state writes quota-bounded + dir-sandboxed; per-plugin capability scope). Note that `WasmWidget` composes with the existing `catch_unwind` guard.
  - **Development:** add `just build-weather` and `just test-wasm`; note `just test` stays hermetic; note `cargo tree -i openssl` must stay empty.
  - **Roadmap:** move "WASM plugins" from planned to done; keep "plugin auto-download by owner/repo" and "interactive capability approval" as next steps.
  - **Design docs:** add links to this spec + plan.

- [ ] **Step 2: Update `README.md`** — add a short "Plugins" section: what a plugin is (a wasm module exposing `name`/`render`), how capabilities work (config allowlists, state dir + quota), how to build/install the weather example (`just build-weather`), and a sample `[plugins.weather]` config block.

- [ ] **Step 3: Verify no stale claims** — grep for "not built yet" / "reserved" WASM language and fix.

Run: `grep -rn "not built yet\|WASM.*reserved\|reserved.*WASM" CLAUDE.md README.md`
Expected: no stale matches remain (or only historical/roadmap mentions that are still accurate).

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md README.md
git commit -m "docs: document the WASM plugin system, capabilities, and weather example"
```

---

## Self-Review

**1. Spec coverage:**
- §2/§2.1 crates & deps → Tasks 2,4,5 (rustline-wasm), 6,7 (bin), 8 (weather). ✓
- §3 ABI (name/render, host fns) → Tasks 4 (abi), 5 (host_fn! + build_plugin), 8 (guest). ✓
- §3.1 name identity (filename stem, verify export) → Task 5 `register_plugins`. ✓
- §4 capability enforcement (glob/`re:`, URL/path gates, state sandbox + cap) → Tasks 2,3,4. ✓ incl. denied-case tests.
- §5 config schema + totality → Task 1. ✓
- §6 discovery/registration/degradation → Tasks 5,6. ✓
- §7 weather behavior (cache freshness, stale fallback, icon map) → Task 8 + e2e Task 9. ✓
- §8 CLI → Tasks 6 (`--plugin-dir`), 7 (`plugin …`). ✓
- §9 build/tooling → Tasks 8 (`build-weather`), 9 (`test-wasm`). ✓
- §10 testing (pure units, denied cases, e2e one-hit + stale) → Tasks 1–9. ✓
- §11/§12 invariants → enforced in code + Task 10 docs. ✓

**2. Placeholder scan:** No "TBD"/"handle errors"/"similar to Task N". Each code step shows full code. Implementer notes flag the two genuine external-API uncertainties (ureq timeout method; extism-pdk macro call convention) with concrete fallbacks — these are verification prompts, not placeholders.

**3. Type consistency:** `CapabilityCtx::from_config(&str, &PluginConfig, PathBuf)` used identically in Tasks 4,5,9. `perform_*` signatures match between Task 4 (def) and Task 5 (host_fn callers). `HttpResult/ReadResult/WriteResult` fields (`ok`/`status`/`body`/`exists`/`contents`/`error`) match between abi.rs (Task 4), the guest's JSON reads (Task 8), and e2e (Task 9). `register_plugins(&mut Registry, &Config, &Path, &[String])` matches between Task 5 (def) and Task 6 (call). `WasmWidget::new(Plugin, serde_json::Value)` matches Tasks 5 and 9. Weather options keys (`zip`/`format`/`refresh_secs`/`api_base`) match between Task 8 guest and Task 9 e2e. ✓

## Execution Handoff

Per `/ship-it` (full-bypass after `--ask`): proceed directly to **subagent-driven-development** — no plan-review pause.
