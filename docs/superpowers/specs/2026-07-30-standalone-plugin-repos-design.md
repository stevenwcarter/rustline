# Standalone `rustline-*` plugin repos — design

**Date:** 2026-07-30
**Status:** Approved (via `/ship-it --ask` Q&A)

## Goal

Demonstrate the third-party plugin story end-to-end by shipping four real,
daily-useful plugins, each in its **own public GitHub repo** named with the
`rustline-` prefix, each scaffolded via `rustline plugin new` (exercising that
command for real), each with release automation, installable via
`rustline plugin install`, and discoverable via `rustline plugin search`.

The five bundled `plugins/*` are capability demos that live inside the
rustline checkout with a path-dep SDK. Nothing today demonstrates what a
*third-party author* actually does: a standalone repo, a pinned SDK
dependency, a manifest that survives distribution, CI that publishes a
`.wasm` release asset, and a registry entry. These four repos are that
exemplar — and the exercise already surfaced one real rustline bug (install
naming, below).

## The four plugins

Widget/plugin names must be ≤ 15 bytes (`range=user|NAME` cap, enforced by
`plugin new`), so the **repo** carries the `rustline-` prefix and the
**plugin name** (crate name, `.wasm` stem, exported `name()`, config key,
layout entry, toggle identity) is the short stem. This mirrors tmux plugin
convention (repo `tmux-battery`, plugin `battery`).

| Repo | Plugin name | Capabilities | Data source |
|---|---|---|---|
| `rustline-updates` | `updates` | exec + state | pending package updates (`checkupdates` by default; configurable for apt/dnf/brew) |
| `rustline-pubip` | `pubip` | http_cached | public/WAN IP via https://api.ipify.org |
| `rustline-kube` | `kube` | file_read | current context/namespace from `~/.kube/config` |
| `rustline-ticker` | `ticker` | http_cached | coin price via CoinGecko's keyless simple-price API |

All four: pure logic unit-tested on the host target; `#[cfg(target_arch =
"wasm32")] mod guest` using the SDK; `format`/`alt_format`/`down_format`
options with the SDK's `active_format` click-toggle helper; `rl_log` on every
failure path; failure renders `down_format` (default `""`) — never a
fabricated reading (the built-ins' invariant #6, carried over).

## Shared repo shape

Each repo is created by running `rustline plugin new <name>` and promoting
the scaffold to the repo root at `~/src/rustline-<name>`:

```
rustline-<name>/
  Cargo.toml            # scaffold's: empty [workspace], edition 2024, cdylib
  Cargo.lock            # committed
  rustfmt.toml          # edition = "2024" (user's global Rust policy)
  .gitignore            # /target
  LICENSE               # MIT, Copyright (c) 2026 Steven Carter (matches rustline)
  README.md             # what it renders, install → approve → configure → place
  manifest.toml         # the capability manifest source (embedded at build, below)
  src/lib.rs            # pure logic + guest module
  .github/workflows/ci.yml       # push/PR: fmt --check, clippy (host + wasm32), test
  .github/workflows/release.yml  # v* tag: build wasm, attach <name>.wasm + SHA256SUMS
  .github/dependabot.yml         # github-actions ecosystem, weekly (actions pinned to SHAs)
```

**SDK dependency:** `rustline-plugin-sdk = { git =
"https://github.com/stevenwcarter/rustline", tag = "v0.1.0" }`. Verified:
the repo is public, the tag is on the remote, and its SDK already includes
every wrapper these plugins need (`exec`, `exec_cached`, `http_get_cached`,
`file_read`, `state_read`/`state_write`, `log`, `active_format`).

**Manifest embedding (the key distribution decision):** `plugin install`
downloads only the `.wasm` release asset, so a sidecar `<name>.toml` in the
repo would never reach the user's plugin dir and `plugin approve` would find
nothing. `resolve_manifest` already supports the fallback: an embedded
`rustline-manifest` wasm custom section. Each repo embeds `manifest.toml` at
build time with the standard link-section trick (no SDK change needed, works
at the v0.1.0 pin):

```rust
#[cfg(target_arch = "wasm32")]
#[used]
#[unsafe(link_section = "rustline-manifest")]
pub static MANIFEST: [u8; include_bytes!("../manifest.toml").len()] =
    *include_bytes!("../manifest.toml");
```

This is the first real exercise of the embedded-manifest path. Verification
that `plugin approve` resolves it from the built `.wasm` is a mandatory step
in the first repo built (pubip); if it proves unreliable, the fallback is a
documented `curl -O` of the sidecar into the plugin dir — but the embedded
path is expected to work (`find_custom_section` exists precisely for this).

**CI/release workflows:** actions pinned to full commit SHAs with `# vX.Y.Z`
comments plus a dependabot config to refresh them — same policy as rustline
itself (reuse rustline's current pins). `ci.yml` runs `cargo fmt --check`,
clippy on the host target and on `--target wasm32-unknown-unknown` (the guest
module is invisible to a host-only pass), and `cargo test`. `release.yml`
triggers on a `v*` tag: build `--release --target wasm32-unknown-unknown`,
copy the artifact to `<name>.wasm`, generate `SHA256SUMS`, create the GitHub
release with both assets (`gh release create` via `GITHUB_TOKEN`).

## Per-plugin design

### `updates` (repo `rustline-updates`)

The most instructive repo: a real polling widget composing **plain `exec`**
with a **state-backed TTL cache**. It deliberately does NOT use
`rl_exec_cached`: the host caches only zero-exit runs, and `checkupdates`
exits **2** in the everyday "no updates" case — with `exec_cached` that
common case would re-spawn `checkupdates` (which hits the network) on every
render tick. Instead the plugin persists `(unix_ts, count)` via
`state_write` and only execs when the persisted sample is older than
`refresh_secs`.

- Options: `program` (default `"checkupdates"`), `args` (string array,
  default `[]`), `refresh_secs` (default `3600`), `ok_statuses` (int array,
  default `[0, 2]` — exit statuses meaning "ran; stdout is the update
  list"; apt users set `[0]`, dnf users `[0, 100]` — README documents a
  per-distro table), `format` (default `"󰚰 {count}"`), `alt_format`,
  `down_format`, `zero_format` (default `""` — what to render when the count
  is 0; hiding a zero is the i3status/polybar convention).
- Render flow: parse `context.now` (RFC3339) to unix seconds via a small
  pure `rfc3339_to_unix` helper (days-from-civil algorithm, no chrono dep —
  itself a worked example of testable pure guest logic) → `state_read` the
  last sample → if fresh, render it → else `exec(program, args)`; a status
  in `ok_statuses` counts non-empty stdout lines, persists, renders; any
  other outcome (denied, spawn failure, timeout, unexpected status)
  `rl_log`s and serves the **stale persisted count** if one exists
  (mirroring the host's cached-HTTP serve-stale convention), else
  `down_format`.
- Placeholder: `{count}`.
- Manifest: `requested_commands = ["checkupdates"]`. A user who overrides
  `program` grants their own pattern (`plugin cmd add`); the README shows
  the apt/dnf/brew variants.

### `pubip` (repo `rustline-pubip`)

- Options: `url` (default `"https://api.ipify.org"`), `refresh_secs`
  (default `900`), `format` (default `"󰩟 {ip}"`), `alt_format`,
  `down_format`.
- Render flow: `http_get_cached(url, refresh_secs, now)`; trim the body and
  validate it looks like an IP (chars in `[0-9a-fA-F.:]`, length ≤ 45) so a
  captive-portal/error page never renders into the bar; invalid or failed →
  `rl_log` + `down_format`. Serve-stale comes free from the host cache.
- Placeholder: `{ip}`.
- Manifest: `requested_urls = ["https://api.ipify.org*"]` (globset `*`
  crosses `/`, so this covers the bare URL; the default matches the manifest
  exactly — a user pointing `url` elsewhere grants their own pattern).

### `kube` (repo `rustline-kube`)

- Options: `path` (default: `context.home` + `/.kube/config` — the guest
  can't read `$KUBECONFIG`; a user with a nonstandard config sets `path`),
  `format` (default `"󱃾 {context}"`), `alt_format` (README suggests
  `"󱃾 {context}:{namespace}"`), `down_format`.
- Render flow: `file_read(path)` → minimal hand-rolled, indentation-aware
  scan of the kubeconfig YAML (no serde_yaml dep — it's unmaintained and
  heavy for two fields): `current-context:` top-level scalar, then the
  matching `contexts:` entry's `namespace:` (absent → `"default"`). Pure and
  unit-tested against fixture kubeconfigs, including quoted values and a
  missing current-context. Denied/missing/unparseable → `rl_log` +
  `down_format`. No TTL — a local file read per render is cheap.
- Placeholders: `{context}`, `{namespace}`.
- Manifest: `requested_paths = ["/home/*/.kube/config",
  "/Users/*/.kube/config"]` (read-only; tight but portable — globset `*`
  crossing `/` makes these safe supersets of one home level).

### `ticker` (repo `rustline-ticker`)

- Options: `coin` (default `"bitcoin"` — a CoinGecko id), `currency`
  (default `"usd"`), `refresh_secs` (default `300`), `format` (default
  `"󰠓 {price}"`), `alt_format`, `down_format`.
- Render flow: validate `coin`/`currency` are `[a-z0-9-]` slugs (hygiene:
  config text is interpolated into the fetched URL) → build
  `https://api.coingecko.com/api/v3/simple/price?ids={coin}&vs_currencies={currency}`
  → `http_get_cached` → parse with `serde_json` → format via a pure
  `format_price` helper (≥ 1000: thousands-separated integer; ≥ 1: two
  decimals; < 1: four significant decimals — unit-tested buckets). Any
  failure → `rl_log` + `down_format`.
- Placeholders: `{price}`, `{coin}`, `{currency}`.
- Manifest: `requested_urls =
  ["https://api.coingecko.com/api/v3/simple/price*"]`.

## rustline-repo changes (this branch)

1. **`plugin install` default-name fix (bug surfaced by this exercise).**
   `do_install` currently defaults the plugin name to the **repo** name; for
   any repo following the `rustline-<name>` convention that saves
   `rustline-<name>.wasm`, whose stem can never equal the exported
   `name()` — `register_plugins` then skips the plugin at load. And `plugin
   search`'s hint prints exactly that bare install command. Fix: default the
   name to the selected **release asset's stem** (`updates.wasm` →
   `updates`), validated by the existing name validation; `--name` still
   overrides; an asset stem that fails validation is an error suggesting
   `--name`. With this, the search hint and a bare
   `rustline plugin install stevenwcarter/rustline-pubip` both just work.
   TDD: update/extend the existing `do_install` tests.
2. **Registry entries.** Four `bundled: false` entries in
   `registry/index.json` — `name` = short stem, `source` =
   `stevenwcarter/rustline-<name>`, honest `capabilities` advertising copy.
3. **Scaffold template comment.** `assets/plugin-cargo.toml.tmpl`'s SDK-dep
   comment gains the out-of-tree form (git dep pinned to a rustline tag)
   alongside the in-tree path form.
4. **Docs.** README gains a short "Writing an out-of-tree plugin" section
   (the authoring steps, linking the four repos as exemplars); CLAUDE.md
   gets its usual roadmap/doc-list sync.

## End-to-end verification (the point of the exercise)

For each repo: local `cargo test` + host/wasm clippy + fmt; local
`rustline plugin build` + `rustline plugin run <name>` smoke (dev harness,
read-only); embedded-manifest resolution check (`plugin approve` sees the
requests). Then `gh repo create stevenwcarter/rustline-<name> --public`,
push, tag `v0.1.0`, wait for the release workflow, and verify the real flow
against a **scratch** `--config`/`--plugin-dir` (never mutating the real
setup): `plugin install stevenwcarter/rustline-<name>` → checksum recorded →
`plugin approve <name> --yes` writes exactly the manifest's grants →
`plugin run <name>` renders (network-dependent renders may show
`down_format`; the assertion is "registered, gated, not skipped") →
`plugin search` marks it installed.

## Out of scope

- Publishing the SDK to crates.io (git dep chosen; revisit later).
- A fifth "template" repo (`plugin new` is the template mechanism).
- Stock tickers needing API keys; kube namespace edge cases beyond the
  minimal parser; per-distro update *application* (we only count).
- Any daemon/renderer changes — plugins ride the existing pipeline.
