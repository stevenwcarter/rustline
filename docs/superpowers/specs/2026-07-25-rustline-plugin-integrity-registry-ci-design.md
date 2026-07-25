# rustline — plugin integrity, plugin registry, CI/CD, and the v0.1.0 release

Date: 2026-07-25
Branch: `feat/2026-07-25-plugin-integrity-registry-ci`
Supersedes/closes: **W19** (checksum verification of installed plugin wasm) and
**W49** (plugin registry/index for discovery) from `WHATS-NEXT.md`.

## Overview

Four workstreams shipped as one branch:

1. **W19 — plugin integrity.** Verify a discovered `.wasm` against the `checksum`
   already recorded in `[plugins.<name>]` *before* instantiating it. Today that
   field is written by `plugin install`/`update` and **read by nothing**.
2. **W49 — plugin discovery.** A curated `registry/index.json` in this repo plus
   `rustline plugin search`, so a user can find widgets without already knowing
   the owner/repo.
3. **CI.** Greenfield GitHub Actions: format, clippy, workspace tests, the five
   excluded example plugins, and the opt-in wasm end-to-end suites — all on PRs.
4. **Release.** A tag-triggered workflow producing four platform tarballs, then a
   `v0.1.0` pre-release.

W19, W49, and CI are mutually independent. The release is strictly last.

## Item 1 — W19: plugin checksum verification

### Where it goes

`register_plugins` (`crates/rustline-wasm/src/lib.rs`) already has everything the
check needs, in the right order and with no extra I/O:

```
:112  let pc = cfg.plugins.get(stem)...      // PluginConfig, hence pc.checksum
:113  let Ok(wasm) = std::fs::read(&path)    // full bytes in memory
      <-- CHECKSUM GATE GOES HERE -->
:117  CapabilityCtx::from_config(...)
:121  host::build_plugin(&wasm, ctx)         // takes bytes, not a path
```

The gate sits between the successful read and the `CapabilityCtx` construction —
before any capability object exists and before wasmtime ever sees the module.

### Behavior

Policy: **verify if recorded, allow if absent.**

| `pc.checksum` | Bytes | Outcome |
|---|---|---|
| `None` | any | Load (today's behavior, unchanged) |
| `Some(h)`, well-formed, matches | — | Load |
| `Some(h)`, well-formed, differs | — | `warn!` + skip registration |
| `Some(h)`, malformed | — | `warn!` + skip registration (**fail closed**) |

Malformed fails closed deliberately: a user who wrote a `checksum` is asking for
verification, and a value we cannot parse means we cannot verify. This is
consistent with every other skip site in `register_plugins` (built-in collision,
`name()` mismatch, ABI mismatch) — a skipped plugin is a widget that doesn't
render, never a broken bar.

Accepted forms for a recorded digest: 64 hex characters, case-insensitive,
optionally prefixed `sha256:`, surrounding whitespace trimmed. Anything else is
malformed.

### Code shape

New module `crates/rustline-wasm/src/integrity.rs`:

```rust
pub fn sha256_hex(bytes: &[u8]) -> String;

pub enum ChecksumVerdict {
    NotRecorded,
    Match,
    Mismatch { expected: String, actual: String },
    Malformed { recorded: String },
}

pub fn verify_checksum(recorded: Option<&str>, bytes: &[u8]) -> ChecksumVerdict;
```

`verify_checksum` is pure and fully unit-testable with no wasm, no filesystem,
and no Extism — this is the primary test seam.

**One definition of `sha256_hex`.** It exists today at
`crates/rustline/src/plugin_install.rs:115` but `rustline-wasm` has no `sha2`
dependency. Rather than a second copy (the drift hazard W51 removed for the wire
types), the function moves to `rustline-wasm::integrity` and the bin calls it
from there — the bin already depends on `rustline-wasm`, so the dependency
direction works. `crates/rustline-wasm/Cargo.toml` gains `sha2 = "0.10"`, which
is already in the workspace lock via the bin, so no new graph node and no change
to the rustls-only posture.

### `plugin run` (the dev harness) is warn-only

`instantiate_named` powers `rustline plugin run <name>`, a read-only dev harness
that already deliberately bypasses the `needed` filter, the built-in collision
check, and the `name()`/ABI verification. It computes the same verdict and
`warn!`s on a mismatch, but **still runs the plugin**: you use this command
immediately after rebuilding a plugin you are developing, where a recorded digest
will legitimately mismatch on every iteration. The real gate is
`register_plugins`, which is what the bar, the daemon, and `plugin list` all go
through.

### Daemon interaction

`daemon.rs` builds its warm `Registry` via the same `register_plugins`, so the
daemon inherits verification with no extra work. A swapped `.wasm` is caught on
the next registry rebuild (i.e. on a config-file mtime change, per
`reload_if_changed`) rather than instantly — noted in the docs, not a defect.

## Item 2 — W49: plugin registry / discovery

### The index

A curated `registry/index.json` committed in this repo. It lives at the repo root
under `registry/`, not under `plugins/` (which holds crates), so the raw URL is
stable and independent of the plugin crates.

```json
{
  "schema_version": 1,
  "plugins": [
    {
      "name": "weather",
      "description": "Nerd-Font condition icon + °F for a zip code, via wttr.in",
      "source": "stevenwcarter/rustline",
      "bundled": true,
      "capabilities": ["http_cached"]
    }
  ]
}
```

- `name` — the `.wasm` stem, and therefore the widget/range name. Must satisfy the
  same rule as any clickable widget name (invariant #7): `[A-Za-z0-9_-]`, ≤ 15
  bytes, not `window`.
- `source` — the `owner/repo` you would pass to `plugin install`.
- `bundled` — true for the five example plugins that ship in this repo and are
  built with `just build-plugin <name>` rather than downloaded from a release.
  A bundled entry's hint is a build command, not an install command; this is
  honest about the fact that the examples have no published `.wasm` asset.
- `capabilities` — informational only. It advertises what the plugin will ask
  for so search output can warn before a user installs; it **grants nothing** and
  is not consulted by the host at load time.

The index seeds with the five bundled examples, which makes `plugin search`
immediately useful and documents the format by example.

### Fetch, cache, override

New module `crates/rustline/src/plugin_index.rs`:

- Default source: the raw URL for this repo's `registry/index.json` on `main`,
  as a compile-time constant.
- Override: a new top-level `plugin_index_url: Option<String>` in `Config`
  (`#[serde(default)]`, alongside `plugin_dir`), so a self-hosted or alternate
  index needs no code change.
- Cache: `<state_root>/plugin-index.json`, written atomically, holding
  `{ "fetched_at": "<rfc3339>", "index": { ... } }`. TTL 24h.
- Fetch reuses the existing `Downloader` trait seam from `plugin_install.rs`
  (`UreqDownloader`, rustls, redirect-limited, `User-Agent`) — no new HTTP client
  and no change to the rustls-only guarantee. The trait is what lets the fetch
  and cache logic be tested with a fake and no network.
- **Serve stale on failure.** A fetch error with a cache present returns the
  cached index and warns that it is stale; with no cache it prints a clear error.
  Never panics.
- `--refresh` bypasses the TTL and forces a fetch.

`plugin-index.json` joins the documented list of host-owned state-file names that
a plugin should avoid colliding with (alongside `cpu-sample`, `cpu-history`,
`throughput-sample*`, `wasmtime-cache*`, `daemon.sock`).

### CLI

```
rustline plugin search [QUERY] [--json] [--refresh]
```

One command, not two. An omitted `QUERY` lists the whole index; a supplied one
filters case-insensitively over name and description. `plugin list` keeps its
current meaning — "what I have configured" — rather than being overloaded with
"what exists". Each row marks whether that plugin is already installed (its
`.wasm` present in the plugin dir, via `discover_plugin_names`).

`--json` follows the established convention exactly (`plugin_list_json`,
`pattern_list_json`, `denials_json`): a pretty-printed array built from a local
`#[derive(Serialize)]` struct, printed with one `println!`, falling back to `"[]"`
on serialize failure, taken as an early return at the top of the command.

Pure, directly-testable functions: `parse_index`, `filter_entries`,
`index_is_fresh`, `search_json`, and the human row formatter.

## Item 3 — CI

Two workflows. Toolchain via `dtolnay/rust-toolchain@stable` with
`Swatinem/rust-cache@v2` (the extism/wasmtime graph is heavy enough that caching
dominates wall-clock). **No `rust-toolchain.toml` is added** — that would change
local developer behavior repo-wide as a side effect of adding CI.

### `.github/workflows/ci.yml` — on `pull_request` and pushes to `main`

| Job | Command |
|---|---|
| `lint` | `just lint` (`cargo fmt --all --check` + `cargo clippy --all-targets -- -D warnings`) |
| `test` | `just test` (`cargo test --workspace`) |
| `plugins` | `just lint-plugins` + `just test-plugins` (matrix over the five) |
| `wasm-e2e` | `just test-wasm` (installs the wasm32 target, builds `weather.wasm`, runs both e2e suites) |

**CI calls `just`, not raw cargo lines** (via `extractions/setup-just@v3`). The
justfile already describes `lint` as "CI-style checks"; routing CI through the
same recipes means the local command and the gate can't drift. Two new recipes
are added for the plugin legs:

```
lint-plugins   # per plugin: fmt --check, clippy (host), clippy (wasm32)
test-plugins   # per plugin: cargo test
```

Both are useful locally in their own right — today nothing checks the example
plugins at all, since they are *excluded* workspace members and thus invisible to
`cargo test --workspace`, `cargo fmt --all`, and `cargo clippy`.

**The plugin lint must run clippy on both targets.** Each plugin's guest code sits
behind `#[cfg(target_arch = "wasm32")] mod guest`, so a host-target clippy pass
compiles only the pure logic and never sees the guest module. Host-only coverage
would look green while checking roughly half the file.

Verified locally before writing this spec: all five plugins are already fmt-clean,
clippy-clean on both targets, and their 22 host tests pass — so these legs go
green on introduction rather than needing a fix-up pass.

Concurrency: cancel in-progress runs per ref.

### `.github/workflows/release.yml` — on `v*` tag push

`permissions: contents: write`.

Build matrix, all native (no cross-compilation toolchain beyond `musl-tools`):

| Target | Runner |
|---|---|
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` |
| `x86_64-unknown-linux-musl` | `ubuntu-latest` + `musl-tools` |
| `aarch64-apple-darwin` | `macos-14` |
| `x86_64-apple-darwin` | `macos-13` |

Each leg builds `--release`, then runs the freshly-built binary to generate shell
completions (every target is native to its runner, and the static musl binary runs
on the gnu host), and packages:

```
rustline-v0.1.0-<target>.tar.gz
└── rustline-v0.1.0-<target>/
    ├── rustline
    ├── completions/{rustline.bash,_rustline,rustline.fish}
    ├── README.md
    └── LICENSE
```

A final job downloads all artifacts, emits `SHA256SUMS`, and publishes via
`softprops/action-gh-release` with `prerelease: true`.

**Known risk — the musl leg.** `extism` pulls wasmtime/cranelift, and the release
profile is `lto = "fat"` with `codegen-units = 1`. Static musl should work, but
this is the leg most likely to fight. If it cannot be made to build in reasonable
time, the fallback is to drop that one target and ship Linux gnu only, rather than
block the release. This is an explicit, pre-authorized fallback — not a silent
narrowing.

## Item 4 — the v0.1.0 release

After this branch merges to `main` and CI is green there:

1. Tag `v0.1.0` (matching the workspace version, which is already `0.1.0`).
2. Push the tag, which triggers `release.yml`.
3. The workflow publishes a **pre-release** with the four tarballs + `SHA256SUMS`.

**This step is confirm-first.** The repo is public and currently has zero tags;
pushing a tag and publishing a release are outward-facing and awkward to reverse.
The tag name and pre-release status are already decided, but the actual push is
checked with the user at the moment it happens.

## Invariants this feature depends on

Recorded explicitly so a later change touching these funnels can grep for who
relies on them.

- **#3 — `Config::load` is total.** The new `plugin_index_url` is
  `Option<String>` with `#[serde(default)]`; a garbage value must degrade to a
  failed fetch (then stale cache, then a printed error), never a load failure.
- **#7 — the name is one identity end-to-end.** Index `name` values must obey the
  same `[A-Za-z0-9_-]` / ≤15-byte / not-`window` rule as any widget range name,
  because an installed plugin's `.wasm` stem *is* its click-toggle identity.
- **N1 — zero ambient authority.** Neither feature adds a guest capability. The
  index fetch is host-side CLI code in the `rustline` bin, reachable only from
  `rustline plugin search`; no new host function is bound, and the guest sandbox
  is untouched. `capabilities` in the index is advertising copy, not a grant.
- **N2 — a plugin never breaks the bar.** A checksum mismatch skips one plugin
  with a `warn!`, exactly like the existing collision/name/ABI skips. It must
  never propagate an error into the render path.
- **N4 — per-plugin capability scope.** Unchanged. Verification reads only that
  plugin's own `PluginConfig`.
- **W38's rule — installing grants nothing.** Extended: *searching* grants
  nothing either. Discovery never widens an allowlist; only `plugin approve` or a
  hand edit does.
- **The rustls-only posture.** The index fetch reuses the existing `ureq`
  (`default-features = false`, `tls`+`json`) client. `cargo tree -i openssl` and
  `-i native-tls` must remain empty across the whole graph.

## Testing strategy

Following the project's test discipline: no test is skipped by appealing to a
current global invariant, and the checksum gate — a shared funnel that every
plugin load now passes through — gets **one test per legitimate producer that
must survive it**.

**Pure unit tests** (`integrity.rs`): every `ChecksumVerdict` variant, including
uppercase hex, a `sha256:` prefix, surrounding whitespace, wrong length, non-hex
characters, and empty string.

**Integration at the seam the feature actually crosses** — `register_plugins`
with a real `.wasm`, therefore in `crates/rustline-wasm/tests/e2e.rs` behind the
existing `wasm-e2e` feature. The legitimate producers enumerated:

1. A plugin with **no recorded checksum** (hand-installed, or built by
   `just build-plugin`) still registers.
2. A plugin whose **recorded checksum matches** registers.
3. A plugin whose **recorded checksum differs** does not register — and the rest
   of the registry is unaffected.
4. A **malformed** recorded checksum does not register.

**Round-trip across the install→load seam** (the load-bearing one): a digest
written by the install path is accepted by the load path. This pins the two
halves together so a later change to either — say, a different hex encoding or a
`sha256:` prefix on write — fails loudly instead of silently disabling every
verified plugin. Uses `sha256_hex` + `write_install_record` on one side and
`verify_checksum` on the other.

**`plugin_index.rs`**: pure `parse_index` (valid, unknown-field-tolerant,
malformed, wrong `schema_version`), `filter_entries` (name hit, description hit,
case-insensitivity, no match, empty query), `index_is_fresh` (fresh, expired,
backward clock), and fetch/cache behavior against a **fake `Downloader`** — cache
miss populates, cache hit skips the fetch, `--refresh` forces one, and a fetch
failure with a stale cache serves the stale copy.

**CLI smoke tests** (`crates/rustline/tests/smoke.rs`): `plugin search` and
`plugin search --json` against a pre-seeded cache file inside the existing
`isolate()` tempdir harness — asserting the JSON shape and that no network is
touched.

## Out of scope

- Signature / public-key verification of plugins (W19 names it as a later step;
  this ships the digest half).
- A `require_checksum` strict mode that refuses unpinned plugins.
- Publishing the five example plugins as separately installable releases.
- `cargo-dist`, installer shell scripts, package-manager formulae.
- Windows binaries.
- Verifying the recorded checksum against a remote pin at install time — the
  recorded digest stays TOFU (trust-on-first-use), as W38 shipped it.

## Documentation

`CLAUDE.md` and `README.md` both get: the new `plugin search` subcommand, the
`plugin_index_url` config key, the checksum-verification behavior and its
fail-closed malformed case, `plugin-index.json` added to the host-owned
state-file list, the two new justfile recipes, and the CI/release story. W19 and
W49 are stripped from `WHATS-NEXT.md` in the same change that ships them.
