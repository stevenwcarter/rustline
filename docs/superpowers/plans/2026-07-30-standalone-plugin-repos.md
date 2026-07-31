# Standalone `rustline-*` Plugin Repos Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship four real, standalone third-party plugin repos (`rustline-updates`, `rustline-pubip`, `rustline-kube`, `rustline-ticker`) — each scaffolded via `rustline plugin new`, published on GitHub with release automation, installable via `rustline plugin install`, discoverable via `rustline plugin search` — plus the rustline-side install-naming fix the exercise surfaced.

**Architecture:** Each repo is an independent wasm32 cdylib crate depending on `rustline-plugin-sdk` via a git dep pinned to rustline tag `v0.1.0`, with its capability manifest **embedded** in the `.wasm` as a `rustline-manifest` custom section (the sidecar file never survives `plugin install`). Pure logic is host-tested; the guest module compiles only on wasm32. The rustline repo itself gets: the `plugin install` default-name fix (asset stem, not repo name), four registry entries, a scaffold-template comment for the out-of-tree dep form, and docs.

**Tech Stack:** Rust edition 2024, extism-pdk 1, rustline-plugin-sdk (git tag v0.1.0), serde/serde_json, GitHub Actions (SHA-pinned), `gh` CLI.

**Spec:** `docs/superpowers/specs/2026-07-30-standalone-plugin-repos-design.md`

## Global Constraints

- Rustline work happens on branch `feat/2026-07-30-standalone-plugin-repos` in `/home/steve/src/rustline`; each plugin repo is its own git repo at `/home/steve/src/rustline-<name>` with its own history.
- Plugin names (crate name, `.wasm` stem, exported `name()`, config key) are the short stems: `updates`, `pubip`, `kube`, `ticker`. Repo names carry the `rustline-` prefix. Never swap these.
- Every crate: `edition = "2024"`, plus a `rustfmt.toml` containing exactly `edition = "2024"`.
- Everything must be clippy-clean (`-D warnings`, host AND `--target wasm32-unknown-unknown` for plugin repos) and `cargo fmt --check`-clean. Commit `Cargo.lock` in every plugin repo.
- SDK dependency (plugin repos): `rustline-plugin-sdk = { git = "https://github.com/stevenwcarter/rustline", tag = "v0.1.0" }` — never a path dep, never a floating branch.
- GitHub Actions pinned to full commit SHAs with trailing `# vX.Y.Z` comments (exact SHAs given in Task 3), plus a dependabot config per repo.
- License: MIT, copied verbatim from `/home/steve/src/rustline/LICENSE`.
- Failure convention (all four plugins): any failure renders `down_format` (default `""` = nothing) — never a fabricated reading — and logs the reason via the SDK `log()` (`rl_log`). A zero-length `down_format` yields `Vec::new()`, not an empty-text segment.
- Guest code goes in `#[cfg(target_arch = "wasm32")] mod guest { ... }`; pure logic above it is host-tested. Model: `/home/steve/src/rustline/plugins/cmdrun/src/lib.rs`.
- All rustline commands in these tasks run from `/home/steve/src/rustline` via `cargo run -p rustline --` unless stated otherwise. End-to-end verification uses scratch `--config`/`plugin_dir` files under `/tmp/claude-1000/-home-steve-src-rustline/a7b5d764-b4da-43d5-acc9-bdf6f63e5ec3/scratchpad` — NEVER the real `~/.config/rustline/config.toml` or `~/.local/share/rustline/plugins`.
- Commit messages end with:
  `Claude-Session: https://claude.ai/code/session_01VAaLGMwx9csJnEmzHMJeWq`

---

### Task 1: `plugin install` default name from the release asset stem

The current default (`name = repo`) breaks every repo following the `rustline-<name>` convention: it saves `rustline-pubip.wasm`, whose stem can never equal the exported `name()` (`pubip`), so `register_plugins` skips the plugin at load — and `plugin search`'s hint prints exactly that bare install command. Fix: derive the default from the selected `.wasm` asset's stem.

**Files:**
- Modify: `/home/steve/src/rustline/crates/rustline/src/plugin_install.rs` (fn `do_install`, ~line 160; tests at bottom)

**Interfaces:**
- Consumes: existing `select_wasm_asset(&Value) -> Option<(String, String)>`, `validate_install_name(&str) -> Result<bool, NameError>`.
- Produces: `do_install` unchanged signature; new default-name behavior relied on by Task 8's bare `plugin install stevenwcarter/rustline-<name>` verification.

- [ ] **Step 1: Write the failing tests**

In the `tests` module of `plugin_install.rs` (note `SAMPLE_RELEASE_JSON`'s asset is `weather.wasm`), rename/replace the existing `install_default_name_is_repo_and_preserves_existing_config` test's naming assertion and add an invalid-stem test. First read the existing test to preserve its "preserves existing config" half — only the expected default name changes (it currently installs under the repo name; it must now expect `weather`). Then add:

```rust
#[test]
fn install_default_name_comes_from_asset_stem_not_repo() {
    let dir = tempfile::tempdir().unwrap();
    let plugin_dir = dir.path().join("plugins");
    let config = dir.path().join("config.toml");
    // Repo follows the rustline-<name> convention; the asset is weather.wasm.
    let name = do_install(
        &fake(b"wasm-bytes"),
        "steve/rustline-weather",
        None,
        None,
        &plugin_dir,
        &config,
    )
    .unwrap();
    assert_eq!(name, "weather");
    assert!(plugin_dir.join("weather.wasm").exists());
    let cfg = std::fs::read_to_string(&config).unwrap();
    assert!(cfg.contains("[plugins.weather]"), "{cfg}");
}

#[test]
fn install_name_override_still_wins() {
    let dir = tempfile::tempdir().unwrap();
    let plugin_dir = dir.path().join("plugins");
    let config = dir.path().join("config.toml");
    let name = do_install(
        &fake(b"wasm-bytes"),
        "steve/rustline-weather",
        Some("wx"),
        None,
        &plugin_dir,
        &config,
    )
    .unwrap();
    assert_eq!(name, "wx");
    assert!(plugin_dir.join("wx.wasm").exists());
}

#[test]
fn install_rejects_invalid_asset_stem_suggesting_name_flag() {
    let dir = tempfile::tempdir().unwrap();
    // An asset stem that fails name validation (space = charset violation).
    let json = serde_json::json!({
        "tag_name": "v1",
        "assets": [{ "name": "my plugin.wasm",
                     "browser_download_url": "https://x/my plugin.wasm" }]
    });
    let dl = FakeDownloader { json, bytes: b"wasm".to_vec() };
    let err = do_install(
        &dl,
        "steve/rustline-weather",
        None,
        None,
        &dir.path().join("plugins"),
        &dir.path().join("config.toml"),
    )
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("--name"), "error should suggest --name: {msg}");
}
```

- [ ] **Step 2: Run tests to verify the new ones fail**

Run: `cargo test -p rustline plugin_install -- --nocapture` (from `/home/steve/src/rustline`)
Expected: the two new tests FAIL (default name is currently the repo, `rustline-weather`); `install_name_override_still_wins` may already pass — that's fine, it pins behavior.

- [ ] **Step 3: Implement the fix**

In `do_install`, move the name derivation to AFTER `select_wasm_asset` and derive the default from the asset stem:

```rust
    let url = release_api_url(&owner, &repo, tag);
    let release = dl
        .get_json(&url)
        .with_context(|| format!("fetch release metadata for {owner}/{repo}"))?;
    let (asset_name, asset_url) = select_wasm_asset(&release)
        .ok_or_else(|| anyhow!("no .wasm asset in the release for {owner}/{repo}"))?;

    // The default name is the release asset's stem, NOT the repo name: the
    // installed `.wasm` stem must equal the plugin's exported `name()` or
    // `register_plugins` skips it at load, and a repo following the
    // `rustline-<name>` convention (asset `<name>.wasm`) would otherwise
    // install under a stem that can never match.
    let name = match name_override {
        Some(n) => n.to_string(),
        None => asset_name.trim_end_matches(".wasm").to_string(),
    };
    let clickable = validate_install_name(&name).map_err(|e| {
        anyhow!("invalid plugin name {name:?} (from release asset {asset_name:?}): {e}; pass --name to override")
    })?;
```

Keep the existing `if !clickable { tracing::warn!(...) }` block after it, and delete the old `let name = name_override.map_or_else(|| repo.clone(), str::to_string);` + its validation from before the release fetch. Update the `do_install` doc comment and the module-level doc if either states the repo-name default. Grep for other statements of the old default: `grep -rn "repo name" crates/rustline/src/plugin_install.rs crates/rustline/src/cli.rs`.

- [ ] **Step 4: Run the full test suite**

Run: `cargo test -p rustline`
Expected: PASS (including the previously-existing install tests you adjusted).

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add crates/rustline/src/plugin_install.rs
git commit -m "fix(plugin-install): default plugin name from release asset stem

A repo named rustline-<name> releasing <name>.wasm used to install as
rustline-<name>.wasm, whose stem can never match the exported name() --
register_plugins then skips the plugin at load, and plugin search's
hint prints exactly that broken bare install command.

Claude-Session: https://claude.ai/code/session_01VAaLGMwx9csJnEmzHMJeWq"
```

---

### Task 2: Scaffold template shows the out-of-tree SDK dep form

**Files:**
- Modify: `/home/steve/src/rustline/crates/rustline/assets/plugin-cargo.toml.tmpl` (lines 26-35, the SDK dep comment + line)

**Interfaces:**
- Consumes/Produces: template text only; `plugin new` embeds it verbatim via `include_str!`.

- [ ] **Step 1: Check whether any test pins the template text**

Run: `grep -rn "rustline-plugin-sdk" crates/rustline/src crates/rustline/tests | grep -v plugin-cargo`
Expected: note any test asserting on scaffold content (e.g. in `plugin_cmd.rs`'s tests or `tests/smoke.rs`); adjust it in Step 3 if it pins the comment text.

- [ ] **Step 2: Edit the template**

Replace the current comment block + dep line (the text from `# The rustline plugin SDK:` through the `rustline-plugin-sdk = { path = ... }` line) with:

```toml
# The rustline plugin SDK: typed host-capability wrappers (`http_get`,
# `state_read`, …), the shared wire types (`GuestRender`/`WireContext`/
# `Segment`/…), a click-toggle helper, and the `export_plugin!` macro — so this
# plugin depends on one crate rather than hand-rolling the Extism glue.
#
# In-tree (scaffolded inside rustline's own `plugins/` directory): use the
# path dependency below, adjusting the relative path if you scaffolded
# elsewhere in the checkout.
#
# Standalone repo (out-of-tree): replace it with the git dependency pinned to
# a rustline release tag — see the rustline-pubip / rustline-ticker /
# rustline-kube / rustline-updates repos for complete worked examples:
#
#   rustline-plugin-sdk = { git = "https://github.com/stevenwcarter/rustline", tag = "v0.1.0" }
rustline-plugin-sdk = { path = "../../crates/rustline-plugin-sdk" }
```

- [ ] **Step 3: Verify the scaffold still works and tests pass**

Run: `cargo test -p rustline plugin` then `cargo run -p rustline -- plugin new smoketest --path /tmp/claude-1000/-home-steve-src-rustline/a7b5d764-b4da-43d5-acc9-bdf6f63e5ec3/scratchpad && rm -rf /tmp/claude-1000/-home-steve-src-rustline/a7b5d764-b4da-43d5-acc9-bdf6f63e5ec3/scratchpad/smoketest`
Expected: tests PASS; scaffold prints its usual next-steps output.

- [ ] **Step 4: Lint and commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add crates/rustline/assets/plugin-cargo.toml.tmpl
git commit -m "docs(plugin-new): show the out-of-tree git-dep SDK form in the scaffold

Claude-Session: https://claude.ai/code/session_01VAaLGMwx9csJnEmzHMJeWq"
```

---

### Task 3: `rustline-pubip` repo (establishes the shared repo shape)

The first standalone repo; Tasks 4-6 copy its boilerplate. Renders the public/WAN IP via the TTL-cached HTTP capability.

**Files (all under `/home/steve/src/rustline-pubip` unless noted):**
- Create: entire repo — `Cargo.toml`, `Cargo.lock`, `rustfmt.toml`, `.gitignore`, `LICENSE`, `README.md`, `manifest.toml`, `src/lib.rs`, `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `.github/dependabot.yml`

**Interfaces:**
- Consumes: SDK `http_get_cached(url: &str, ttl_secs: i64, now: &str) -> Result<CachedHttpResult, HostError>` (fields: `ok, status, body, error, stale, age_secs`), `active_format(&WireContext, name, format, alt) -> &str`, `log(LogLevel, &str)`, `export_plugin!(name: "pubip", render: render)`; `GuestRender { context: WireContext, config: serde_json::Value }`.
- Produces: public GitHub repo `stevenwcarter/rustline-pubip` (Task 7 publishes; Task 8 installs). Boilerplate files copied by Tasks 4-6.

- [ ] **Step 1: Scaffold via `rustline plugin new` and init the repo**

```bash
cd /home/steve/src/rustline
cargo run -p rustline -- plugin new pubip --path /home/steve/src
mv /home/steve/src/pubip /home/steve/src/rustline-pubip
cd /home/steve/src/rustline-pubip
git init -b main
git add -A
git commit -m "chore: scaffold via 'rustline plugin new pubip'

Unmodified output of the scaffold command, kept as the first commit so
the adaptation to a standalone repo is visible in history.

Claude-Session: https://claude.ai/code/session_01VAaLGMwx9csJnEmzHMJeWq"
```

- [ ] **Step 2: Repo boilerplate (license, fmt, gitignore, workflows, dependabot)**

```bash
cp /home/steve/src/rustline/LICENSE LICENSE
printf 'edition = "2024"\n' > rustfmt.toml
printf '/target\n' > .gitignore
mkdir -p .github/workflows
```

Write `.github/dependabot.yml`:

```yaml
version: 2
updates:
  - package-ecosystem: github-actions
    directory: /
    schedule:
      interval: weekly
```

Write `.github/workflows/ci.yml` (pins copied from rustline's own CI; dtolnay's SHA is the `stable` branch tip — its ref selects the toolchain, dependabot can't bump it):

```yaml
name: CI

# Actions are pinned to full commit SHAs (the trailing comment records the
# readable version); .github/dependabot.yml refreshes them by PR.
# dtolnay/rust-toolchain's SHA is the tip of its `stable` branch -- the ref
# selects the toolchain, so refresh that one by hand.

on:
  pull_request:
  push:
    branches: [main]

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always

jobs:
  check:
    name: fmt + clippy + test
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
      - uses: dtolnay/rust-toolchain@4cda84d5c5c54efe2404f9d843567869ab1699d4 # stable branch tip
        with:
          components: rustfmt, clippy
          targets: wasm32-unknown-unknown
      - uses: Swatinem/rust-cache@c19371144df3bb44fab255c43d04cbc2ab54d1c4 # v2.9.1
      - run: cargo fmt --check
      # Host pass covers the pure logic + tests; the wasm pass is what
      # actually compiles the guest module (invisible to the host pass).
      - run: cargo clippy --all-targets --locked -- -D warnings
      - run: cargo clippy --target wasm32-unknown-unknown --locked -- -D warnings
      - run: cargo test --locked
```

Write `.github/workflows/release.yml`:

```yaml
name: Release

# A v* tag builds the wasm and publishes it as a GitHub release with a
# SHA256SUMS file -- the asset `rustline plugin install` downloads. The asset
# is named after the plugin (its crate name), NOT the repo: the installed
# .wasm stem must equal the plugin's exported name().

on:
  push:
    tags: ["v*"]

permissions:
  contents: write

jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
      - uses: dtolnay/rust-toolchain@4cda84d5c5c54efe2404f9d843567869ab1699d4 # stable branch tip
        with:
          targets: wasm32-unknown-unknown
      - uses: Swatinem/rust-cache@c19371144df3bb44fab255c43d04cbc2ab54d1c4 # v2.9.1
      - name: Build
        run: cargo build --release --locked --target wasm32-unknown-unknown
      - name: Stage assets
        run: |
          mkdir dist
          cp target/wasm32-unknown-unknown/release/pubip.wasm dist/
          (cd dist && sha256sum *.wasm > SHA256SUMS)
      - name: Publish release
        uses: softprops/action-gh-release@3d0d9888cb7fd7b750713d6e236d1fcb99157228 # v3.0.2
        with:
          files: dist/*
          generate_release_notes: true
```

- [ ] **Step 3: Adapt `Cargo.toml` (git-dep SDK, metadata, serde)**

Replace the scaffolded `Cargo.toml` with:

```toml
# Empty [workspace] table: this plugin builds standalone for wasm32 and must
# not be adopted by any ancestor Cargo workspace it happens to be nested in.
[workspace]

[package]
name = "pubip"
version = "0.1.0"
edition = "2024"
license = "MIT"
repository = "https://github.com/stevenwcarter/rustline-pubip"
description = "rustline widget: your public/WAN IP via the TTL-cached HTTP capability"

[lib]
crate-type = ["cdylib"]

# Host-side too: Options derives Deserialize and the guest decodes its
# [plugins.pubip.options] table from the render input's JSON config.
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Guest bindings only compile for the wasm target; `cargo test` on the host
# builds just the pure logic in src/lib.rs.
[target.'cfg(target_arch = "wasm32")'.dependencies]
extism-pdk = "1"
# The rustline plugin SDK, pinned to a rustline release tag. This is the
# out-of-tree form; an in-tree plugin would use a path dependency instead.
rustline-plugin-sdk = { git = "https://github.com/stevenwcarter/rustline", tag = "v0.1.0" }
```

- [ ] **Step 4: Write the capability manifest**

Write `manifest.toml` (embedded into the `.wasm` in Step 6; `rustline plugin approve pubip` reads it from the binary and offers exactly these grants):

```toml
# Capability manifest for `rustline plugin approve pubip`. Embedded into the
# built .wasm as the `rustline-manifest` custom section (see src/lib.rs), so
# it survives distribution -- `plugin install` only downloads the .wasm.
name = "pubip"
version = "0.1.0"
requested_urls = ["https://api.ipify.org*"]
```

- [ ] **Step 5: Write the failing pure-logic tests**

Replace `src/lib.rs`'s scaffolded content with the pure half plus tests (guest module comes in Step 6):

```rust
//! `pubip` — a rustline widget showing your public/WAN IP.
//!
//! Fetches a configured URL (default <https://api.ipify.org>, which returns
//! the caller's IP as plain text) through the host's TTL-cached HTTP
//! capability, so the network is hit at most once per `refresh_secs` and a
//! stale answer is served while the endpoint is unreachable.
//!
//! Pure logic lives here and is unit-tested on the host target (`cargo
//! test`); the Extism guest glue below only compiles for wasm32.

use serde::Deserialize;

/// Longest plausible IP literal (an IPv6 address maxes out at 45 chars
/// including an IPv4-mapped tail). Anything longer is not an IP.
pub const MAX_IP_CHARS: usize = 45;

/// This plugin's `[plugins.pubip.options]` table.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Options {
    /// Endpoint returning the caller's IP as a plain-text body.
    pub url: String,
    /// Cache TTL in seconds (the host owns the cache).
    pub refresh_secs: i64,
    /// Render format. `{ip}` is the fetched address.
    pub format: String,
    /// Click-toggle alternate view.
    pub alt_format: String,
    /// Shown when no usable answer exists. Empty renders nothing.
    pub down_format: String,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            url: "https://api.ipify.org".to_string(),
            refresh_secs: 900,
            format: "\u{f0a5f} {ip}".to_string(), // nf-md-ip_network + space
            alt_format: String::new(),
            down_format: String::new(),
        }
    }
}

/// Trim and validate a response body as an IP-address-looking string, so a
/// captive portal's HTML error page never renders into the status line.
/// Accepts IPv4 (`203.0.113.9`) and IPv6 (`2001:db8::1`) shapes: hex digits,
/// dots, and colons only, at most [`MAX_IP_CHARS`] chars.
pub fn extract_ip(body: &str) -> Option<String> {
    let s = body.trim();
    if s.is_empty() || s.len() > MAX_IP_CHARS {
        return None;
    }
    s.chars()
        .all(|c| c.is_ascii_hexdigit() || c == '.' || c == ':')
        .then(|| s.to_string())
}

/// Substitute `{ip}`. Unknown placeholders pass through untouched, the same
/// convention as the built-in widgets' formats.
pub fn render_format(format: &str, ip: &str) -> String {
    format.replace("{ip}", ip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_ipv4_with_surrounding_whitespace() {
        assert_eq!(extract_ip("203.0.113.9\n"), Some("203.0.113.9".to_string()));
    }

    #[test]
    fn extracts_ipv6() {
        assert_eq!(extract_ip("2001:db8::8a2e:370:7334"), Some("2001:db8::8a2e:370:7334".to_string()));
    }

    #[test]
    fn rejects_html_error_pages() {
        assert_eq!(extract_ip("<html><body>captive portal</body></html>"), None);
    }

    #[test]
    fn rejects_empty_and_overlong() {
        assert_eq!(extract_ip("   "), None);
        assert_eq!(extract_ip(&"1".repeat(46)), None);
    }

    #[test]
    fn render_format_substitutes_ip_and_keeps_unknown_placeholders() {
        assert_eq!(render_format("IP {ip} {x}", "1.2.3.4"), "IP 1.2.3.4 {x}");
    }

    #[test]
    fn default_options_match_readme() {
        let o = Options::default();
        assert_eq!(o.url, "https://api.ipify.org");
        assert_eq!(o.refresh_secs, 900);
        assert!(o.format.contains("{ip}"));
    }
}
```

- [ ] **Step 6: Run tests (fail → pass), then add the guest module + embedded manifest**

Run: `cargo test` — expected: compile error first if you wrote tests before the fns (fine); iterate until PASS.

Then append to `src/lib.rs` (after the pure logic, before `#[cfg(test)]`):

```rust
/// The capability manifest, embedded as the `rustline-manifest` custom
/// section so `rustline plugin approve pubip` can read the requests straight
/// out of the distributed `.wasm` — `plugin install` downloads only this
/// binary, so a sidecar file would never reach the user's plugin dir.
#[cfg(target_arch = "wasm32")]
#[used]
#[unsafe(link_section = "rustline-manifest")]
pub static MANIFEST: [u8; include_bytes!("../manifest.toml").len()] =
    *include_bytes!("../manifest.toml");

#[cfg(target_arch = "wasm32")]
mod guest {
    use super::*;
    use rustline_plugin_sdk::{
        GuestRender, LogLevel, Segment, active_format, export_plugin, http_get_cached, log,
    };

    fn render(input: &GuestRender) -> Vec<Segment> {
        let opts: Options = serde_json::from_value(input.config.clone()).unwrap_or_default();
        let format = active_format(&input.context, "pubip", &opts.format, &opts.alt_format);

        match http_get_cached(&opts.url, opts.refresh_secs, &input.context.now) {
            Ok(r) if r.ok => match extract_ip(&r.body) {
                Some(ip) => vec![Segment::new(render_format(format, &ip))],
                None => {
                    log(
                        LogLevel::Warn,
                        &format!("pubip: {} returned a body that doesn't look like an IP", opts.url),
                    );
                    down_segment(&opts.down_format)
                }
            },
            Ok(r) => {
                log(LogLevel::Warn, &format!("pubip: fetch failed: {}", r.error));
                down_segment(&opts.down_format)
            }
            Err(e) => {
                log(LogLevel::Warn, &format!("pubip: host call failed: {e}"));
                down_segment(&opts.down_format)
            }
        }
    }

    /// `down_format` verbatim, or no segment at all when it's empty — the
    /// built-in widgets' collapse-to-nothing convention.
    fn down_segment(down_format: &str) -> Vec<Segment> {
        if down_format.is_empty() {
            Vec::new()
        } else {
            vec![Segment::new(down_format.to_string())]
        }
    }

    export_plugin!(name: "pubip", render: render);
}
```

- [ ] **Step 7: Build for wasm and verify the embedded manifest + gating end-to-end (local)**

```bash
rustup target list --installed | grep -q wasm32-unknown-unknown || rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
SCRATCH=/tmp/claude-1000/-home-steve-src-rustline/a7b5d764-b4da-43d5-acc9-bdf6f63e5ec3/scratchpad/pubip-e2e
mkdir -p "$SCRATCH/plugins"
cp target/wasm32-unknown-unknown/release/pubip.wasm "$SCRATCH/plugins/"
printf 'plugin_dir = "%s/plugins"\n' "$SCRATCH" > "$SCRATCH/config.toml"
cd /home/steve/src/rustline
# THE key check: approve must resolve the manifest from the wasm custom
# section (no sidecar pubip.toml exists in the scratch plugin dir).
cargo run -p rustline -- --config "$SCRATCH/config.toml" plugin approve pubip --yes
grep -A2 '\[plugins.pubip\]' "$SCRATCH/config.toml"
# Expected: allowed_urls = ["https://api.ipify.org*"]
cargo run -p rustline -- --config "$SCRATCH/config.toml" plugin run pubip --plugin-dir "$SCRATCH/plugins"
# Expected: a segment like "󰩟 <your-ip>" (with network), or an empty render
# plus a logged fetch failure (without) — and NO capability denials either way.
```

If `approve` reports "no manifest": the link-section embedding didn't survive. Debug before proceeding (check `#[used]` is present, the static is `pub`, and the section name is exactly `rustline-manifest`); as a last resort fall back to documenting a sidecar download in the README — but the embedded path is expected to work (`manifest.rs`'s `find_custom_section` exists for it).

- [ ] **Step 8: Write the README**

Write `README.md`:

```markdown
# rustline-pubip

Your public/WAN IP in your tmux status line, as a
[rustline](https://github.com/stevenwcarter/rustline) WASM plugin.

Fetches https://api.ipify.org (configurable) through rustline's TTL-cached,
capability-gated HTTP host function: the network is hit at most once per
`refresh_secs`, a stale answer is served while the endpoint is unreachable,
and the plugin can only reach URLs you explicitly allow.

## Install

    rustline plugin install stevenwcarter/rustline-pubip --name pubip
    rustline plugin approve pubip        # grants: https://api.ipify.org*

(`--name pubip` is only needed on rustline v0.1.0, whose default install
name came from the repo; newer builds derive it from the release asset.)

Place it:

    rustline widget enable pubip

## Configure

    [plugins.pubip.options]
    # url = "https://api.ipify.org"   # any endpoint returning a plain-text IP
    # refresh_secs = 900              # host-side cache TTL
    # format = "󰩟 {ip}"
    # alt_format = ""                 # non-empty makes the widget click-toggleable
    # down_format = ""                # shown when no usable answer exists

A body that doesn't look like an IP (a captive portal's HTML page, say) is
treated as a failure — the widget renders `down_format`, never garbage.

## Building from source

    cargo build --release --target wasm32-unknown-unknown
    cp target/wasm32-unknown-unknown/release/pubip.wasm ~/.local/share/rustline/plugins/

The capability manifest (`manifest.toml`) is embedded into the `.wasm` as the
`rustline-manifest` custom section at build time, so `plugin approve` works
on the distributed binary alone. This repo doubles as a worked example of a
standalone (out-of-tree) rustline plugin: scaffolded with `rustline plugin
new`, SDK via a git dependency pinned to a rustline release tag, CI + tag
releases publishing the installable `.wasm`.

## License

MIT
```

- [ ] **Step 9: Lint, generate the lockfile, commit**

```bash
cd /home/steve/src/rustline-pubip
cargo fmt --check || cargo fmt
cargo clippy --all-targets --locked -- -D warnings || cargo clippy --all-targets -- -D warnings
cargo clippy --target wasm32-unknown-unknown -- -D warnings
cargo test
git add -A
git commit -m "feat: public-IP widget as a standalone rustline plugin

Pure logic host-tested; guest fetches via the TTL-cached HTTP capability;
capability manifest embedded as the rustline-manifest custom section; CI +
tag-release workflow publishing pubip.wasm.

Claude-Session: https://claude.ai/code/session_01VAaLGMwx9csJnEmzHMJeWq"
```

---

### Task 4: `rustline-ticker` repo

Coin price via CoinGecko's keyless simple-price API. Same repo shape as Task 3 — boilerplate is **copied from `rustline-pubip`**, then name-adjusted.

**Files (all under `/home/steve/src/rustline-ticker`):**
- Create: same file set as Task 3.

**Interfaces:**
- Consumes: SDK `http_get_cached`, `active_format`, `log`, `export_plugin!`; `CachedHttpResult` fields as in Task 3.
- Produces: public GitHub repo `stevenwcarter/rustline-ticker` (published in Task 7, installed in Task 8).

- [ ] **Step 1: Scaffold, init, boilerplate**

```bash
cd /home/steve/src/rustline
cargo run -p rustline -- plugin new ticker --path /home/steve/src
mv /home/steve/src/ticker /home/steve/src/rustline-ticker
cd /home/steve/src/rustline-ticker
git init -b main && git add -A
git commit -m "chore: scaffold via 'rustline plugin new ticker'

Claude-Session: https://claude.ai/code/session_01VAaLGMwx9csJnEmzHMJeWq"
cp ../rustline-pubip/LICENSE ../rustline-pubip/rustfmt.toml ../rustline-pubip/.gitignore .
mkdir -p .github/workflows
cp ../rustline-pubip/.github/dependabot.yml .github/
cp ../rustline-pubip/.github/workflows/ci.yml .github/workflows/
sed 's/pubip\.wasm/ticker.wasm/' ../rustline-pubip/.github/workflows/release.yml > .github/workflows/release.yml
```

Adapt `Cargo.toml` exactly as Task 3 Step 3, with `name = "ticker"`, `repository = "https://github.com/stevenwcarter/rustline-ticker"`, `description = "rustline widget: a coin price via CoinGecko, TTL-cached"`.

- [ ] **Step 2: Write `manifest.toml`**

```toml
# Embedded into the built .wasm as the `rustline-manifest` custom section.
name = "ticker"
version = "0.1.0"
requested_urls = ["https://api.coingecko.com/api/v3/simple/price*"]
```

- [ ] **Step 3: Write failing pure-logic tests, then the implementation**

`src/lib.rs` pure half:

```rust
//! `ticker` — a rustline widget showing a coin price via CoinGecko's keyless
//! simple-price API, fetched through the host's TTL-cached HTTP capability.
//!
//! Pure logic lives here and is unit-tested on the host target; the Extism
//! guest glue below only compiles for wasm32.

use serde::Deserialize;

/// This plugin's `[plugins.ticker.options]` table.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Options {
    /// A CoinGecko coin id (lowercase slug), e.g. "bitcoin", "ethereum".
    pub coin: String,
    /// A CoinGecko vs-currency (lowercase slug), e.g. "usd", "eur".
    pub currency: String,
    /// Cache TTL in seconds (the host owns the cache).
    pub refresh_secs: i64,
    /// Render format. `{price}`, `{coin}`, `{currency}` substitute.
    pub format: String,
    /// Click-toggle alternate view.
    pub alt_format: String,
    /// Shown when no usable answer exists. Empty renders nothing.
    pub down_format: String,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            coin: "bitcoin".to_string(),
            currency: "usd".to_string(),
            refresh_secs: 300,
            format: "\u{f0813} {price}".to_string(), // nf-md-bitcoin + space
            alt_format: String::new(),
            down_format: String::new(),
        }
    }
}

/// CoinGecko ids and currencies are lowercase slugs. Refusing anything else
/// keeps config text from smuggling arbitrary content into the fetched URL.
pub fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The simple-price endpoint for one coin/currency pair.
pub fn price_url(coin: &str, currency: &str) -> String {
    format!("https://api.coingecko.com/api/v3/simple/price?ids={coin}&vs_currencies={currency}")
}

/// Pull the price out of CoinGecko's `{"bitcoin":{"usd":118000.5}}` shape.
pub fn extract_price(body: &str, coin: &str, currency: &str) -> Option<f64> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get(coin)?.get(currency)?.as_f64()
}

/// Human price formatting: >= 1000 rounds to a thousands-separated integer;
/// >= 1 keeps two decimals; >= 0.0001 four decimals; below that eight (small
/// coins would otherwise render as 0.0000).
pub fn format_price(p: f64) -> String {
    if p >= 1000.0 {
        let s = format!("{:.0}", p.round());
        let mut out = String::new();
        for (i, c) in s.chars().enumerate() {
            if i > 0 && (s.len() - i) % 3 == 0 {
                out.push(',');
            }
            out.push(c);
        }
        out
    } else if p >= 1.0 {
        format!("{p:.2}")
    } else if p >= 0.0001 {
        format!("{p:.4}")
    } else {
        format!("{p:.8}")
    }
}

/// Substitute `{price}`/`{coin}`/`{currency}`; unknown placeholders pass
/// through untouched.
pub fn render_format(format: &str, price: &str, coin: &str, currency: &str) -> String {
    format
        .replace("{price}", price)
        .replace("{coin}", coin)
        .replace("{currency}", currency)
}
```

Tests (write first, watch them fail to compile, implement, pass):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_validation() {
        assert!(valid_slug("bitcoin"));
        assert!(valid_slug("matic-network"));
        assert!(!valid_slug(""));
        assert!(!valid_slug("Bitcoin"));
        assert!(!valid_slug("btc&x=1"));
    }

    #[test]
    fn price_url_shape() {
        assert_eq!(
            price_url("bitcoin", "usd"),
            "https://api.coingecko.com/api/v3/simple/price?ids=bitcoin&vs_currencies=usd"
        );
    }

    #[test]
    fn extracts_price_from_coingecko_shape() {
        let body = r#"{"bitcoin":{"usd":118000.5}}"#;
        assert_eq!(extract_price(body, "bitcoin", "usd"), Some(118000.5));
        assert_eq!(extract_price(body, "ethereum", "usd"), None);
        assert_eq!(extract_price("not json", "bitcoin", "usd"), None);
    }

    #[test]
    fn price_formatting_buckets() {
        assert_eq!(format_price(118000.5), "118,001");
        assert_eq!(format_price(1234567.0), "1,234,567");
        assert_eq!(format_price(999.994), "999.99");
        assert_eq!(format_price(2.5), "2.50");
        assert_eq!(format_price(0.0834), "0.0834");
        assert_eq!(format_price(0.00001234), "0.00001234");
    }

    #[test]
    fn render_format_substitutes_all() {
        assert_eq!(
            render_format("{coin} {price} {currency}", "1,000", "bitcoin", "usd"),
            "bitcoin 1,000 usd"
        );
    }
}
```

- [ ] **Step 4: Guest module + embedded manifest**

Append (same `MANIFEST` static as Task 3 Step 6, verbatim — it reads `../manifest.toml`), then:

```rust
#[cfg(target_arch = "wasm32")]
mod guest {
    use super::*;
    use rustline_plugin_sdk::{
        GuestRender, LogLevel, Segment, active_format, export_plugin, http_get_cached, log,
    };

    fn render(input: &GuestRender) -> Vec<Segment> {
        let opts: Options = serde_json::from_value(input.config.clone()).unwrap_or_default();
        if !valid_slug(&opts.coin) || !valid_slug(&opts.currency) {
            log(
                LogLevel::Warn,
                &format!("ticker: invalid coin/currency slug {:?}/{:?}", opts.coin, opts.currency),
            );
            return down_segment(&opts.down_format);
        }
        let format = active_format(&input.context, "ticker", &opts.format, &opts.alt_format);
        let url = price_url(&opts.coin, &opts.currency);

        match http_get_cached(&url, opts.refresh_secs, &input.context.now) {
            Ok(r) if r.ok => match extract_price(&r.body, &opts.coin, &opts.currency) {
                Some(p) => {
                    let price = format_price(p);
                    vec![Segment::new(render_format(format, &price, &opts.coin, &opts.currency))]
                }
                None => {
                    log(LogLevel::Warn, &format!("ticker: no {}/{} price in the response", opts.coin, opts.currency));
                    down_segment(&opts.down_format)
                }
            },
            Ok(r) => {
                log(LogLevel::Warn, &format!("ticker: fetch failed: {}", r.error));
                down_segment(&opts.down_format)
            }
            Err(e) => {
                log(LogLevel::Warn, &format!("ticker: host call failed: {e}"));
                down_segment(&opts.down_format)
            }
        }
    }

    fn down_segment(down_format: &str) -> Vec<Segment> {
        if down_format.is_empty() {
            Vec::new()
        } else {
            vec![Segment::new(down_format.to_string())]
        }
    }

    export_plugin!(name: "ticker", render: render);
}
```

- [ ] **Step 5: Build, scratch-verify, README, lint, commit**

Same scratch flow as Task 3 Step 7 with `ticker` substituted (approve must show the CoinGecko URL pattern; `plugin run` renders a price with network, or logs a fetch failure without — no denials). README follows Task 3 Step 8's structure with: title `rustline-ticker`, the CoinGecko description, install commands using `--name ticker`, config sample:

```toml
[plugins.ticker.options]
# coin = "bitcoin"        # any CoinGecko coin id
# currency = "usd"
# refresh_secs = 300
# format = "󰠓 {price}"    # {price} {coin} {currency} substitute
# alt_format = ""
# down_format = ""
```

plus the same "Building from source" and license sections (adjusted names). Then:

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
cargo clippy --target wasm32-unknown-unknown -- -D warnings && cargo test
git add -A
git commit -m "feat: CoinGecko price-ticker widget as a standalone rustline plugin

Claude-Session: https://claude.ai/code/session_01VAaLGMwx9csJnEmzHMJeWq"
```

---

### Task 5: `rustline-kube` repo

Current Kubernetes context/namespace from a kubeconfig, via the gated file-read capability. Boilerplate copied from `rustline-pubip`.

**Files (all under `/home/steve/src/rustline-kube`):**
- Create: same file set as Task 3.

**Interfaces:**
- Consumes: SDK `file_read(path: &str) -> Result<ReadResult, HostError>` (`ReadResult { ok, exists, contents, error }` — `ok=true, exists=false` is a successful read of a missing file), `active_format`, `log`, `export_plugin!`; `WireContext.home: String`.
- Produces: public GitHub repo `stevenwcarter/rustline-kube`.

- [ ] **Step 1: Scaffold, init, boilerplate**

Same commands as Task 4 Step 1 with `kube` substituted for `ticker` (and `pubip.wasm` → `kube.wasm` in the release-workflow sed). `Cargo.toml`: `name = "kube"`, `repository = "https://github.com/stevenwcarter/rustline-kube"`, `description = "rustline widget: current Kubernetes context/namespace from your kubeconfig"`. Commit the raw scaffold first, exactly as before.

- [ ] **Step 2: Write `manifest.toml`**

```toml
# Embedded into the built .wasm as the `rustline-manifest` custom section.
# Read-only; globset's `*` crosses `/`, so these cover one home level on
# Linux and macOS. A nonstandard kubeconfig location needs its own
# `rustline plugin path add kube <pattern>` grant plus the `path` option.
name = "kube"
version = "0.1.0"
requested_paths = ["/home/*/.kube/config", "/Users/*/.kube/config"]
```

- [ ] **Step 3: Write failing parser tests, then the parser**

`src/lib.rs` pure half:

```rust
//! `kube` — a rustline widget showing the current Kubernetes context (and
//! its namespace) from a kubeconfig, read through the host's capability-gated
//! file-read. The kubeconfig is YAML, but only two fields matter here, so a
//! minimal indentation-aware scan replaces a full YAML dependency
//! (serde_yaml is unmaintained; a statusline widget doesn't need it).
//!
//! Pure logic lives here and is unit-tested on the host target; the Extism
//! guest glue below only compiles for wasm32.

use serde::Deserialize;

/// This plugin's `[plugins.kube.options]` table.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Options {
    /// Kubeconfig path. Empty (the default) derives `<home>/.kube/config`
    /// from the render context — the guest can't read `$KUBECONFIG`.
    pub path: String,
    /// Render format. `{context}` and `{namespace}` substitute.
    pub format: String,
    /// Click-toggle alternate view.
    pub alt_format: String,
    /// Shown when no kubeconfig/current-context exists. Empty renders nothing.
    pub down_format: String,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            path: String::new(),
            format: "\u{f10fe} {context}".to_string(), // nf-md-kubernetes + space
            alt_format: String::new(),
            down_format: String::new(),
        }
    }
}

/// A parsed kubeconfig: the active context name and its namespace.
#[derive(Debug, PartialEq, Eq)]
pub struct KubeInfo {
    pub context: String,
    pub namespace: String,
}

fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"'))
            || (s.starts_with('\'') && s.ends_with('\'')))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// `key: value` on one (trimmed) line -> the unquoted value, else None.
fn value_of<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.trim().strip_prefix(key)?.strip_prefix(':')?;
    let v = strip_quotes(rest);
    (!v.is_empty()).then_some(v)
}

/// Extract the current context + its namespace. `None` when there is no
/// top-level `current-context:` (an empty or foreign file). A context whose
/// entry carries no `namespace:` — or is missing from `contexts:` — reports
/// namespace `"default"`, matching kubectl's own behavior.
pub fn parse_kubeconfig(text: &str) -> Option<KubeInfo> {
    let current = text
        .lines()
        .filter(|l| !l.starts_with([' ', '\t']))
        .find_map(|l| value_of(l, "current-context"))?
        .to_string();
    let namespace = context_namespace(text, &current)
        .unwrap_or("default")
        .to_string();
    Some(KubeInfo { context: current, namespace })
}

/// The `namespace:` recorded for the context named `current`, scanning only
/// the top-level `contexts:` section — the `clusters:`/`users:` lists carry
/// `name:` keys too and must not be consulted. Handles both kubectl's
/// column-0 list items (`- context:` at the left margin) and indented ones.
fn context_namespace<'a>(text: &'a str, current: &str) -> Option<&'a str> {
    let mut in_contexts = false;
    let mut name: Option<&str> = None;
    let mut ns: Option<&str> = None;
    for line in text.lines() {
        let is_top_level_key =
            !line.trim().is_empty() && !line.starts_with([' ', '\t']) && !line.starts_with('-');
        if is_top_level_key {
            if in_contexts && name == Some(current) {
                return ns;
            }
            in_contexts = line.trim_end() == "contexts:";
            (name, ns) = (None, None);
            continue;
        }
        if !in_contexts {
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("- ") || trimmed == "-" {
            // New list item: flush the previous one.
            if name == Some(current) {
                return ns;
            }
            (name, ns) = (None, None);
        }
        let item_line = trimmed.strip_prefix("- ").unwrap_or(trimmed);
        if let Some(v) = value_of(item_line, "name") {
            name = Some(v);
        }
        if let Some(v) = value_of(item_line, "namespace") {
            ns = Some(v);
        }
    }
    if in_contexts && name == Some(current) { ns } else { None }
}

/// Substitute `{context}`/`{namespace}`; unknown placeholders pass through.
pub fn render_format(format: &str, info: &KubeInfo) -> String {
    format
        .replace("{context}", &info.context)
        .replace("{namespace}", &info.namespace)
}
```

Tests (kubectl writes list items at column 0 — the first fixture pins that):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const KUBECTL_STYLE: &str = "\
apiVersion: v1
clusters:
- cluster:
    server: https://prod.example:6443
  name: prod-cluster
contexts:
- context:
    cluster: prod-cluster
    namespace: monitoring
    user: admin
  name: prod
- context:
    cluster: dev-cluster
    user: dev
  name: dev
current-context: prod
kind: Config
users:
- name: admin
  user: {}
";

    #[test]
    fn parses_kubectl_style_column_zero_items() {
        let info = parse_kubeconfig(KUBECTL_STYLE).unwrap();
        assert_eq!(info.context, "prod");
        assert_eq!(info.namespace, "monitoring");
    }

    #[test]
    fn context_without_namespace_reports_default() {
        let text = KUBECTL_STYLE.replace("current-context: prod", "current-context: dev");
        let info = parse_kubeconfig(&text).unwrap();
        assert_eq!(info.context, "dev");
        assert_eq!(info.namespace, "default");
    }

    #[test]
    fn current_context_missing_from_contexts_list_reports_default() {
        let text = KUBECTL_STYLE.replace("current-context: prod", "current-context: gone");
        assert_eq!(parse_kubeconfig(&text).unwrap().namespace, "default");
    }

    #[test]
    fn clusters_and_users_sections_never_match() {
        // A cluster named like the current context must not donate fields.
        let text = "\
clusters:
- cluster:
    namespace: wrong
  name: prod
contexts:
- context:
    cluster: c
    namespace: right
  name: prod
current-context: prod
";
        assert_eq!(parse_kubeconfig(text).unwrap().namespace, "right");
    }

    #[test]
    fn quoted_values_are_unquoted() {
        let text = "current-context: \"prod\"\ncontexts:\n- context:\n    namespace: 'ns1'\n  name: \"prod\"\n";
        let info = parse_kubeconfig(text).unwrap();
        assert_eq!(info.context, "prod");
        assert_eq!(info.namespace, "ns1");
    }

    #[test]
    fn indented_list_items_also_parse() {
        let text = "\
current-context: prod
contexts:
  - context:
      namespace: ns2
    name: prod
";
        assert_eq!(parse_kubeconfig(text).unwrap().namespace, "ns2");
    }

    #[test]
    fn no_current_context_is_none() {
        assert_eq!(parse_kubeconfig(""), None);
        assert_eq!(parse_kubeconfig("apiVersion: v1\n"), None);
    }

    #[test]
    fn render_format_substitutes_both() {
        let info = KubeInfo { context: "prod".into(), namespace: "mon".into() };
        assert_eq!(render_format("{context}:{namespace}", &info), "prod:mon");
    }
}
```

Run `cargo test` (fail first, then implement until PASS). Note `parse_kubeconfig` returning `Option<KubeInfo>` requires `KubeInfo: PartialEq` for the `assert_eq!(None)` comparisons — the derive above provides it.

- [ ] **Step 4: Guest module + embedded manifest**

Append the same `MANIFEST` static (verbatim from Task 3 Step 6), then:

```rust
#[cfg(target_arch = "wasm32")]
mod guest {
    use super::*;
    use rustline_plugin_sdk::{
        GuestRender, LogLevel, Segment, active_format, export_plugin, file_read, log,
    };

    fn render(input: &GuestRender) -> Vec<Segment> {
        let opts: Options = serde_json::from_value(input.config.clone()).unwrap_or_default();
        let path = if !opts.path.is_empty() {
            opts.path.clone()
        } else if !input.context.home.is_empty() {
            format!("{}/.kube/config", input.context.home)
        } else {
            log(LogLevel::Warn, "kube: no `path` option and no home dir in context");
            return down_segment(&opts.down_format);
        };

        match file_read(&path) {
            Ok(r) if r.ok && r.exists => match parse_kubeconfig(&r.contents) {
                Some(info) => {
                    let format =
                        active_format(&input.context, "kube", &opts.format, &opts.alt_format);
                    vec![Segment::new(render_format(format, &info))]
                }
                None => {
                    log(LogLevel::Info, &format!("kube: no current-context in {path}"));
                    down_segment(&opts.down_format)
                }
            },
            Ok(r) if r.ok => {
                log(LogLevel::Info, &format!("kube: {path} does not exist"));
                down_segment(&opts.down_format)
            }
            Ok(r) => {
                log(LogLevel::Warn, &format!("kube: read denied/failed: {}", r.error));
                down_segment(&opts.down_format)
            }
            Err(e) => {
                log(LogLevel::Warn, &format!("kube: host call failed: {e}"));
                down_segment(&opts.down_format)
            }
        }
    }

    fn down_segment(down_format: &str) -> Vec<Segment> {
        if down_format.is_empty() {
            Vec::new()
        } else {
            vec![Segment::new(down_format.to_string())]
        }
    }

    export_plugin!(name: "kube", render: render);
}
```

- [ ] **Step 5: Build + deterministic scratch verification**

Unlike pubip/ticker, this one verifies **deterministically** with a fixture kubeconfig:

```bash
cargo build --release --target wasm32-unknown-unknown
SCRATCH=/tmp/claude-1000/-home-steve-src-rustline/a7b5d764-b4da-43d5-acc9-bdf6f63e5ec3/scratchpad/kube-e2e
mkdir -p "$SCRATCH/plugins"
cp target/wasm32-unknown-unknown/release/kube.wasm "$SCRATCH/plugins/"
printf 'contexts:\n- context:\n    namespace: monitoring\n  name: prod\ncurrent-context: prod\n' > "$SCRATCH/kubeconfig"
cat > "$SCRATCH/config.toml" <<EOF
plugin_dir = "$SCRATCH/plugins"

[plugins.kube]
allowed_paths = ["$SCRATCH/*"]

[plugins.kube.options]
path = "$SCRATCH/kubeconfig"
alt_format = "{context}:{namespace}"
EOF
cd /home/steve/src/rustline
# Embedded-manifest check (no sidecar in the scratch dir):
cargo run -p rustline -- --config "$SCRATCH/config.toml" plugin approve kube --yes
grep 'allowed_paths' "$SCRATCH/config.toml"
# Expected: the scratch grant plus the two appended manifest patterns.
cargo run -p rustline -- --config "$SCRATCH/config.toml" plugin run kube --plugin-dir "$SCRATCH/plugins"
# Expected: a segment "󱃾 prod" and no denials.
```

- [ ] **Step 6: README, lint, commit**

README per Task 3 Step 8's structure: title `rustline-kube`, description (current context/namespace, kube-tmux equivalent, file-read capability), install with `--name kube`, approve note listing the two path patterns, config sample:

```toml
[plugins.kube.options]
# path = ""                          # default: <home>/.kube/config
# format = "󱃾 {context}"             # {context} {namespace} substitute
# alt_format = "󱃾 {context}:{namespace}"   # click to toggle namespace on/off
# down_format = ""
```

plus a note that `$KUBECONFIG` is invisible to a sandboxed guest (set `path` instead, and `rustline plugin path add kube <pattern>` for a nonstandard location). Lint/test/commit as Task 4 Step 5, message `"feat: Kubernetes context widget as a standalone rustline plugin"`.

---

### Task 6: `rustline-updates` repo

Pending package updates. The most instructive repo: composes plain `exec` with a state-backed TTL so the (network-hitting) checker runs at most once per `refresh_secs` — deliberately NOT `exec_cached`, whose host cache stores only zero-exit runs while `checkupdates` exits **2** in the everyday "no updates" case (caching nothing → a spawn every render tick).

**Files (all under `/home/steve/src/rustline-updates`):**
- Create: same file set as Task 3.

**Interfaces:**
- Consumes: SDK `exec(program: &str, args: &[&str]) -> Result<ExecResult, HostError>` (`ExecResult { ok, status, stdout, stderr, error, truncated }` — `ok` means "allowed and ran", non-zero exit is data), `state_read(relpath) -> Result<ReadResult, HostError>`, `state_write(relpath, contents) -> Result<WriteResult, HostError>`, `active_format`, `log`, `export_plugin!`; `WireContext.now: String` (RFC3339).
- Produces: public GitHub repo `stevenwcarter/rustline-updates`.

- [ ] **Step 1: Scaffold, init, boilerplate**

Same commands as Task 4 Step 1 with `updates` substituted (release sed target `updates.wasm`). `Cargo.toml`: `name = "updates"`, `repository = "https://github.com/stevenwcarter/rustline-updates"`, `description = "rustline widget: count of pending package updates (checkupdates/apt/dnf/brew)"`. Commit the raw scaffold first.

- [ ] **Step 2: Write `manifest.toml`**

```toml
# Embedded into the built .wasm as the `rustline-manifest` custom section.
# The default checker only; overriding `program`/`args` needs a matching
# `rustline plugin cmd add updates "<pattern>"` grant (patterns match the
# WHOLE canonical argv, not just the program).
name = "updates"
version = "0.1.0"
requested_commands = ["checkupdates"]
```

- [ ] **Step 3: Write failing pure-logic tests, then the implementation**

`src/lib.rs` pure half:

```rust
//! `updates` — a rustline widget counting pending package updates.
//!
//! Runs a configured checker (default `checkupdates`, from Arch's
//! pacman-contrib) through the host's capability-gated exec, at most once per
//! `refresh_secs`: the result is persisted via the sandboxed state capability
//! and reused until stale. Deliberately NOT `rl_exec_cached` — the host's
//! exec cache stores only zero-exit runs, and `checkupdates` exits 2 in the
//! everyday "no updates" case, which would re-spawn a network-hitting checker
//! on every render tick.
//!
//! Pure logic lives here and is unit-tested on the host target; the Extism
//! guest glue below only compiles for wasm32.

use serde::Deserialize;

/// This plugin's `[plugins.updates.options]` table.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Options {
    /// The checker program (argv[0]; no shell is involved).
    pub program: String,
    /// Its arguments, passed through verbatim.
    pub args: Vec<String>,
    /// How often to actually run the checker, in seconds.
    pub refresh_secs: i64,
    /// Exit statuses meaning "the checker ran; stdout is the update list".
    /// `checkupdates` exits 2 for "no updates" (default `[0, 2]`); apt wants
    /// `[0]`, dnf `[0, 100]`.
    pub ok_statuses: Vec<i32>,
    /// Render format. `{count}` is the number of pending updates.
    pub format: String,
    /// Click-toggle alternate view.
    pub alt_format: String,
    /// Shown when no usable count exists at all. Empty renders nothing.
    pub down_format: String,
    /// Shown when the count is 0. Empty (the default) renders nothing —
    /// the i3status/polybar convention. Set it to your `format` to show 0.
    pub zero_format: String,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            program: "checkupdates".to_string(),
            args: Vec::new(),
            refresh_secs: 3600,
            ok_statuses: vec![0, 2],
            format: "\u{f06b0} {count}".to_string(), // nf-md-update + space
            alt_format: String::new(),
            down_format: String::new(),
            zero_format: String::new(),
        }
    }
}

/// Number of pending updates: non-empty stdout lines.
pub fn count_updates(stdout: &str) -> usize {
    stdout.lines().filter(|l| !l.trim().is_empty()).count()
}

/// State file contents: `"<unix_ts> <count>"` on one line.
pub fn parse_state(contents: &str) -> Option<(i64, usize)> {
    let mut it = contents.split_whitespace();
    let ts = it.next()?.parse().ok()?;
    let count = it.next()?.parse().ok()?;
    Some((ts, count))
}

pub fn serialize_state(ts: i64, count: usize) -> String {
    format!("{ts} {count}\n")
}

/// A sample is fresh while `now` is within `ttl` after `ts`; a clock that
/// moved backwards counts as stale (mirrors the host caches' convention).
pub fn is_fresh(now: i64, ts: i64, ttl: i64) -> bool {
    now >= ts && now - ts < ttl
}

/// Which format renders a given count, honoring the hide-zero convention.
/// `None` means render nothing.
pub fn render_count(active_format: &str, zero_format: &str, count: usize) -> Option<String> {
    let f = if count == 0 { zero_format } else { active_format };
    (!f.is_empty()).then(|| f.replace("{count}", &count.to_string()))
}

/// RFC3339 (`context.now`) -> unix seconds, no chrono: fixed-position
/// date/time fields plus an optional fractional-seconds run and a
/// `Z`/`±HH:MM` offset, over Howard Hinnant's days-from-civil algorithm.
pub fn rfc3339_to_unix(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    if b[4] != b'-'
        || b[7] != b'-'
        || !(b[10] == b'T' || b[10] == b't' || b[10] == b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> { s.get(r)?.parse().ok() };
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, min, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let rest = &s[19..];
    let rest = if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return None;
        }
        &frac[digits..]
    } else {
        rest
    };
    let offset_secs: i64 = if rest.is_empty() || rest == "Z" || rest == "z" {
        0
    } else {
        let sign = match rest.as_bytes()[0] {
            b'+' => 1,
            b'-' => -1,
            _ => return None,
        };
        let os = rest.get(1..)?;
        let oh: i64 = os.get(0..2)?.parse().ok()?;
        if os.as_bytes().get(2) != Some(&b':') {
            return None;
        }
        let om: i64 = os.get(3..5)?.parse().ok()?;
        sign * (oh * 3600 + om * 60)
    };

    // Days-from-civil (Hinnant): valid over the whole proleptic Gregorian
    // calendar with integer math only.
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hour * 3600 + min * 60 + sec - offset_secs)
}
```

Tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_non_empty_lines() {
        assert_eq!(count_updates(""), 0);
        assert_eq!(count_updates("linux 6.9 -> 6.10\n"), 1);
        assert_eq!(count_updates("a 1 -> 2\nb 3 -> 4\n\n"), 2);
    }

    #[test]
    fn state_round_trips() {
        let s = serialize_state(1785427200, 42);
        assert_eq!(parse_state(&s), Some((1785427200, 42)));
        assert_eq!(parse_state("garbage"), None);
        assert_eq!(parse_state(""), None);
    }

    #[test]
    fn freshness_window_and_backward_clock() {
        assert!(is_fresh(1000, 900, 3600));
        assert!(!is_fresh(5000, 900, 3600));
        assert!(!is_fresh(800, 900, 3600)); // clock went backwards -> stale
    }

    #[test]
    fn render_count_hides_zero_by_default() {
        assert_eq!(render_count("U {count}", "", 3), Some("U 3".to_string()));
        assert_eq!(render_count("U {count}", "", 0), None);
        assert_eq!(render_count("U {count}", "up to date", 0), Some("up to date".to_string()));
        assert_eq!(render_count("", "", 3), None);
    }

    #[test]
    fn rfc3339_epoch_and_offsets() {
        assert_eq!(rfc3339_to_unix("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339_to_unix("2026-07-30T16:00:00Z"), Some(1785427200));
        // Same instant expressed with a -04:00 offset.
        assert_eq!(rfc3339_to_unix("2026-07-30T12:00:00-04:00"), Some(1785427200));
        // Fractional seconds are tolerated and ignored.
        assert_eq!(rfc3339_to_unix("2026-07-30T16:00:00.123Z"), Some(1785427200));
    }

    #[test]
    fn rfc3339_rejects_garbage() {
        assert_eq!(rfc3339_to_unix(""), None);
        assert_eq!(rfc3339_to_unix("not a timestamp"), None);
        assert_eq!(rfc3339_to_unix("2026-13-01T00:00:00Z"), None);
    }
}
```

Run `cargo test` (fail first, implement, PASS).

- [ ] **Step 4: Guest module + embedded manifest**

Append the same `MANIFEST` static (verbatim from Task 3 Step 6), then:

```rust
#[cfg(target_arch = "wasm32")]
mod guest {
    use super::*;
    use rustline_plugin_sdk::{
        GuestRender, LogLevel, Segment, active_format, exec, export_plugin, log, state_read,
        state_write,
    };

    /// State-file name under this plugin's sandboxed state dir.
    const STATE_FILE: &str = "last";

    fn render(input: &GuestRender) -> Vec<Segment> {
        let opts: Options = serde_json::from_value(input.config.clone()).unwrap_or_default();
        let Some(now) = rfc3339_to_unix(&input.context.now) else {
            log(LogLevel::Warn, "updates: could not parse context.now");
            return down_segment(&opts.down_format);
        };

        let persisted = state_read(STATE_FILE)
            .ok()
            .filter(|r| r.ok && r.exists)
            .and_then(|r| parse_state(&r.contents));

        let count = match persisted {
            Some((ts, c)) if is_fresh(now, ts, opts.refresh_secs) => Some(c),
            _ => match run_checker(&opts) {
                Some(c) => {
                    match state_write(STATE_FILE, &serialize_state(now, c)) {
                        Ok(w) if w.ok => {}
                        Ok(w) => log(LogLevel::Warn, &format!("updates: state write failed: {}", w.error)),
                        Err(e) => log(LogLevel::Warn, &format!("updates: state write failed: {e}")),
                    }
                    Some(c)
                }
                // The checker couldn't run: serve the stale count if any
                // (the host caches' serve-stale convention), else go down.
                None => persisted.map(|(_, c)| c),
            },
        };

        match count {
            Some(c) => {
                let format = active_format(&input.context, "updates", &opts.format, &opts.alt_format);
                match render_count(format, &opts.zero_format, c) {
                    Some(text) => vec![Segment::new(text)],
                    None => Vec::new(),
                }
            }
            None => down_segment(&opts.down_format),
        }
    }

    /// Run the checker once. `Some(count)` iff it was allowed, spawned, and
    /// exited with a status in `ok_statuses`.
    fn run_checker(opts: &Options) -> Option<usize> {
        let args: Vec<&str> = opts.args.iter().map(String::as_str).collect();
        match exec(&opts.program, &args) {
            Ok(r) if r.ok && opts.ok_statuses.contains(&r.status) => {
                if r.truncated {
                    log(
                        LogLevel::Info,
                        &format!("updates: {} output truncated; count is a floor", opts.program),
                    );
                }
                Some(count_updates(&r.stdout))
            }
            Ok(r) if r.ok => {
                log(
                    LogLevel::Warn,
                    &format!("updates: {} exited with unexpected status {}", opts.program, r.status),
                );
                None
            }
            Ok(r) => {
                log(
                    LogLevel::Warn,
                    &format!("updates: {} denied or failed to run: {}", opts.program, r.error),
                );
                None
            }
            Err(e) => {
                log(LogLevel::Warn, &format!("updates: host call failed: {e}"));
                None
            }
        }
    }

    fn down_segment(down_format: &str) -> Vec<Segment> {
        if down_format.is_empty() {
            Vec::new()
        } else {
            vec![Segment::new(down_format.to_string())]
        }
    }

    export_plugin!(name: "updates", render: render);
}
```

- [ ] **Step 5: Build + deterministic scratch verification**

Deterministic via `echo` as the checker (grant `echo *`; one line of stdout → count 1):

```bash
cargo build --release --target wasm32-unknown-unknown
SCRATCH=/tmp/claude-1000/-home-steve-src-rustline/a7b5d764-b4da-43d5-acc9-bdf6f63e5ec3/scratchpad/updates-e2e
mkdir -p "$SCRATCH/plugins"
cp target/wasm32-unknown-unknown/release/updates.wasm "$SCRATCH/plugins/"
cat > "$SCRATCH/config.toml" <<EOF
plugin_dir = "$SCRATCH/plugins"

[plugins.updates]
allowed_commands = ["echo *"]

[plugins.updates.options]
program = "echo"
args = ["linux 6.9 -> 6.10"]
EOF
cd /home/steve/src/rustline
# Embedded-manifest check:
cargo run -p rustline -- --config "$SCRATCH/config.toml" plugin approve updates --yes
grep 'allowed_commands' "$SCRATCH/config.toml"
# Expected: ["echo *", "checkupdates"] (idempotent append of the manifest).
cargo run -p rustline -- --config "$SCRATCH/config.toml" plugin run updates --plugin-dir "$SCRATCH/plugins"
# Expected: a segment "󰚰 1" and no denials. Run it a second time: same
# output, now served from the persisted state without re-exec'ing.
```

- [ ] **Step 6: README, lint, commit**

README per Task 3 Step 8's structure: title `rustline-updates`, description (count of pending updates; why plain exec + state TTL instead of exec_cached — one sentence), install with `--name updates`, approve note (`checkupdates` only; the exec warning `plugin approve` prints is expected), config sample **including the per-distro table**:

```toml
[plugins.updates.options]
# program = "checkupdates"   # Arch (pacman-contrib)
# args = []
# refresh_secs = 3600
# ok_statuses = [0, 2]       # checkupdates: 2 = "no updates"
# format = "󰚰 {count}"
# zero_format = ""           # "" hides the widget when up to date
# alt_format = ""
# down_format = ""
```

| Distro | `program` / `args` | `ok_statuses` | grant (`plugin cmd add updates …`) |
|---|---|---|---|
| Arch | `checkupdates` | `[0, 2]` | `checkupdates` (via `approve`) |
| Debian/Ubuntu | `apt` / `["list", "--upgradable"]` | `[0]` | `apt list --upgradable` |
| Fedora | `dnf` / `["check-update", "-q"]` | `[0, 100]` | `dnf check-update -q` |
| Homebrew | `brew` / `["outdated"]` | `[0]` | `brew outdated` |

(Note under the table: apt's output includes a `Listing...` header line — the count is one high; a `WARNING` line from apt lands on stderr, not stdout, so it doesn't affect the count.)

Lint/test/commit as Task 4 Step 5, message `"feat: pending-package-updates widget as a standalone rustline plugin"`.

---

### Task 7: Publish all four repos (GitHub + v0.1.0 releases)

**Files:** none (GitHub state only).

**Interfaces:**
- Consumes: the four local repos from Tasks 3-6.
- Produces: public repos `stevenwcarter/rustline-{updates,pubip,kube,ticker}`, each with a `v0.1.0` release carrying `<name>.wasm` + `SHA256SUMS` — consumed by Task 8's `plugin install`.

- [ ] **Step 1: Create and push the four repos**

For each pair `(rustline-pubip, "Public/WAN IP widget for rustline")`, `(rustline-ticker, "CoinGecko price-ticker widget for rustline")`, `(rustline-kube, "Kubernetes context widget for rustline")`, `(rustline-updates, "Pending package updates widget for rustline")`:

```bash
cd /home/steve/src/<repo>
gh repo create stevenwcarter/<repo> --public --description "<description>" \
  --source . --remote origin --push
```

- [ ] **Step 2: Verify CI is green before tagging**

```bash
for r in rustline-pubip rustline-ticker rustline-kube rustline-updates; do
  gh run watch -R "stevenwcarter/$r" --exit-status \
    "$(gh run list -R "stevenwcarter/$r" --limit 1 --json databaseId -q '.[0].databaseId')"
done
```
Expected: all four CI runs conclude success. A failure here is a real defect — fix it in that repo (commit, push, re-watch) before tagging.

- [ ] **Step 3: Tag v0.1.0 and watch the release workflows**

```bash
for r in rustline-pubip rustline-ticker rustline-kube rustline-updates; do
  (cd "/home/steve/src/$r" && git tag v0.1.0 && git push origin v0.1.0)
done
# Give the runs a moment to register, then watch each to completion:
for r in rustline-pubip rustline-ticker rustline-kube rustline-updates; do
  gh run watch -R "stevenwcarter/$r" --exit-status \
    "$(gh run list -R "stevenwcarter/$r" --workflow Release --limit 1 --json databaseId -q '.[0].databaseId')"
done
```

- [ ] **Step 4: Verify the release assets**

```bash
for r in rustline-pubip rustline-ticker rustline-kube rustline-updates; do
  echo "== $r"; gh release view v0.1.0 -R "stevenwcarter/$r" --json assets -q '.assets[].name'
done
```
Expected per repo: `<name>.wasm` (short stem!) and `SHA256SUMS`.

---

### Task 8: Registry entries + real end-to-end install verification

**Files:**
- Modify: `/home/steve/src/rustline/registry/index.json`

**Interfaces:**
- Consumes: Task 1's asset-stem install default; Task 7's published releases.
- Produces: four `bundled: false` registry entries served to `plugin search`.

- [ ] **Step 1: Add the four registry entries**

Append to the `plugins` array in `registry/index.json` (after `cmdrun`; keep `schema_version` 1):

```json
    {
      "name": "pubip",
      "description": "Your public/WAN IP via a TTL-cached HTTP GET (api.ipify.org by default)",
      "source": "stevenwcarter/rustline-pubip",
      "bundled": false,
      "capabilities": ["http_cached"]
    },
    {
      "name": "ticker",
      "description": "A coin price via CoinGecko's keyless simple-price API, TTL-cached",
      "source": "stevenwcarter/rustline-ticker",
      "bundled": false,
      "capabilities": ["http_cached"]
    },
    {
      "name": "kube",
      "description": "Current Kubernetes context and namespace from your kubeconfig",
      "source": "stevenwcarter/rustline-kube",
      "bundled": false,
      "capabilities": ["file_read"]
    },
    {
      "name": "updates",
      "description": "Count of pending package updates (checkupdates/apt/dnf/brew), exec + state TTL",
      "source": "stevenwcarter/rustline-updates",
      "bundled": false,
      "capabilities": ["exec", "state"]
    }
```

Validate: `python3 -m json.tool registry/index.json > /dev/null && cargo test -p rustline plugin_index`.

- [ ] **Step 2: Commit and push the branch (search verification needs the raw URL to exist)**

```bash
cd /home/steve/src/rustline
git add registry/index.json
git commit -m "feat(registry): list the four standalone rustline-* plugins

Claude-Session: https://claude.ai/code/session_01VAaLGMwx9csJnEmzHMJeWq"
git push -u origin feat/2026-07-30-standalone-plugin-repos
```

- [ ] **Step 3: Real end-to-end install against the published releases**

Fresh scratch (bare install — no `--name` — proves Task 1's fix; the branch's raw-URL index proves the search flow):

```bash
SCRATCH=/tmp/claude-1000/-home-steve-src-rustline/a7b5d764-b4da-43d5-acc9-bdf6f63e5ec3/scratchpad/install-e2e
mkdir -p "$SCRATCH/plugins" "$SCRATCH/state"
cat > "$SCRATCH/config.toml" <<EOF
plugin_dir = "$SCRATCH/plugins"
plugin_index_url = "https://raw.githubusercontent.com/stevenwcarter/rustline/feat/2026-07-30-standalone-plugin-repos/registry/index.json"
EOF
cd /home/steve/src/rustline
for repo in rustline-pubip rustline-ticker rustline-kube rustline-updates; do
  cargo run -p rustline -- --config "$SCRATCH/config.toml" \
    plugin install "stevenwcarter/$repo" --plugin-dir "$SCRATCH/plugins"
done
ls "$SCRATCH/plugins"
# Expected: pubip.wasm ticker.wasm kube.wasm updates.wasm  (short stems!)
grep -c 'checksum' "$SCRATCH/config.toml"
# Expected: 4 (one recorded per install)
for name in pubip ticker kube updates; do
  cargo run -p rustline -- --config "$SCRATCH/config.toml" plugin approve "$name" --yes
  cargo run -p rustline -- --config "$SCRATCH/config.toml" plugin run "$name" --plugin-dir "$SCRATCH/plugins"
done
# Expected: each approve writes its manifest grants (exec warning on
# `updates` is expected); each run registers and renders without a
# capability denial (network-dependent ones may render nothing offline;
# kube/updates render down/nothing without a real kubeconfig/checkupdates —
# the assertion is "installed under the right stem, gated, not skipped").
cargo run -p rustline -- --config "$SCRATCH/config.toml" plugin search --refresh
# Expected: all nine entries; the four new ones marked [installed] with
# their install hints.
```

If a bare install lands a wrong stem or a checksum/approve step fails, that is a defect in Task 1 or the repos — fix there before proceeding.

---

### Task 9: rustline docs (README + CLAUDE.md)

**Files:**
- Modify: `/home/steve/src/rustline/README.md` (plugins section — find it via `grep -n "plugin" README.md | head`)
- Modify: `/home/steve/src/rustline/CLAUDE.md` (roadmap + `plugin_install.rs` bullet + registry/index description)

**Interfaces:** docs only.

- [ ] **Step 1: README — "Writing an out-of-tree plugin" section**

Add after the existing plugin-authoring/bundled-plugins material (adapt heading level to the file's structure):

```markdown
### Writing an out-of-tree plugin

The bundled `plugins/*` live inside this repo; a real third-party plugin is
its own repo. Four worked examples demonstrate the full shape — scaffolded
with `rustline plugin new`, SDK via a git dependency pinned to a rustline
release tag, capability manifest embedded in the `.wasm` (a sidecar file
doesn't survive `plugin install`), CI plus a tag-triggered release that
publishes the installable `<name>.wasm`:

- [rustline-updates](https://github.com/stevenwcarter/rustline-updates) —
  pending package updates (exec + state TTL)
- [rustline-pubip](https://github.com/stevenwcarter/rustline-pubip) —
  public/WAN IP (TTL-cached HTTP)
- [rustline-kube](https://github.com/stevenwcarter/rustline-kube) —
  Kubernetes context/namespace (gated file read)
- [rustline-ticker](https://github.com/stevenwcarter/rustline-ticker) —
  CoinGecko coin price (TTL-cached HTTP)

The short version: `rustline plugin new <name>` (≤ 15 bytes, the name is the
`.wasm` stem AND the exported `name()` — name the *repo* `rustline-<name>`,
not the plugin), swap the SDK path dep for
`rustline-plugin-sdk = { git = "https://github.com/stevenwcarter/rustline", tag = "v0.1.0" }`,
embed your `manifest.toml` with
`#[unsafe(link_section = "rustline-manifest")]` (see any repo above), release
a `<name>.wasm` asset from a `v*` tag, and open a PR adding your plugin to
`registry/index.json` so `rustline plugin search` finds it.
```

- [ ] **Step 2: CLAUDE.md sync**

Three edits:
1. `plugin_install.rs` module-map bullet: note the default install name now
   derives from the release **asset stem** (repo name no longer assumed), so
   `rustline-<name>` repos releasing `<name>.wasm` install correctly.
2. `plugin_index.rs`/registry description: the committed index now also
   lists the four standalone `rustline-*` repos (`bundled: false`).
3. Roadmap: add a "Done" entry summarizing this branch (four standalone
   plugin repos as the out-of-tree exemplar; embedded manifests; install
   default-name fix; registry entries; scaffold-template git-dep comment).

- [ ] **Step 3: Full verification + commit**

```bash
cd /home/steve/src/rustline
just lint && just test
git add README.md CLAUDE.md
git commit -m "docs: out-of-tree plugin authoring section + CLAUDE.md sync

Claude-Session: https://claude.ai/code/session_01VAaLGMwx9csJnEmzHMJeWq"
git push
```

---

## Plan Self-Review Notes

- Spec coverage: naming split (Task 3-6 + Global Constraints), git-dep pin (Task 3 Step 3, copied by 4-6), embedded manifest + first-exercise verification (Task 3 Step 7; deterministic re-checks in 5/6), per-plugin designs (Tasks 3-6 match the spec's options/flows, incl. updates' plain-exec+state rationale and ticker's slug hygiene), install-name fix (Task 1), search-hint correctness via the fix (Task 1, verified Task 8), registry entries (Task 8), scaffold comment (Task 2), README/CLAUDE.md (Task 9), full e2e (Tasks 7-8). The spec's `< 1: four significant decimals` is implemented as two explicit sub-buckets (4 places ≥ 0.0001, else 8) — deliberate refinement, noted in `format_price`'s doc.
- The `install_name_override_still_wins` test may pass before the fix (override path unchanged) — kept as a pin, not a TDD driver.
- Tasks 3-6 are independent of each other but all depend on Tasks 1-2 only via the final e2e (Task 8); they can be implemented in order without cross-references except the deliberate boilerplate copy from `rustline-pubip`.
- Type consistency: `Options` field sets match each guest's uses; `render_count(active_format, zero_format, count)` signature consistent between definition, tests, and guest call.
