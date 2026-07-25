# Plugin integrity, plugin registry, CI/CD, and v0.1.0 release — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Verify a plugin `.wasm` against its recorded checksum before loading it (W19), add a curated plugin index with `rustline plugin search` (W49), stand up GitHub Actions CI and a tag-triggered release workflow, and cut a `v0.1.0` pre-release.

**Architecture:** The checksum gate slots into `register_plugins` between the existing `fs::read` and `CapabilityCtx` construction — the bytes and the `PluginConfig` are both already in hand there, so no extra I/O. Its decision logic is a pure function in a new `rustline-wasm::integrity` module. The plugin index is a JSON file committed at `registry/index.json`, fetched over the existing `Downloader` trait seam from `plugin_install.rs` and cached under the state dir with a TTL. CI routes every check through `just` recipes so the local command and the CI gate cannot drift.

**Tech Stack:** Rust 1.97 / edition 2024, `sha2`, `serde`/`serde_json`, `ureq` (rustls), `clap`, `toml_edit`, `tracing`, GitHub Actions.

## Global Constraints

- **Edition 2024** in every crate; `rustfmt.toml` is edition 2024 and all crate editions must equal it.
- Must stay **clippy-clean** (`cargo clippy --all-targets -- -D warnings`) and **rustfmt-clean** (`cargo fmt --all --check`). Run `cargo fmt --all` before every commit — there is no pre-commit hook.
- **rustls-only.** `cargo tree -i openssl` and `cargo tree -i native-tls` must stay empty across the whole graph. Never add a dependency that pulls either.
- **Commit `Cargo.lock`** alongside any dependency change.
- **Invariant #3 — `Config::load` is total.** A bad config must never break the bar.
- **Invariant N2 — a plugin never breaks the bar.** A rejected plugin is skipped with a `warn!`; it never propagates an error into the render path.
- **Invariant N1 — zero ambient authority.** No new guest host function is added by any task here. The index fetch is host-side CLI code only.
- **Invariant #7 — the name is one identity end-to-end.** Plugin names are `[A-Za-z0-9_-]`, ≤ 15 bytes, never `window`.
- Existing `--json` convention: a pretty-printed array from a local `#[derive(Serialize)]` struct, one `println!`, `"[]"` fallback on serialize failure, taken as an early return at the top of the command function.
- Tests are TDD. Do **not** skip a test by appealing to a current global invariant.

---

## File Structure

**Created:**
- `crates/rustline-wasm/src/integrity.rs` — pure digest + verdict logic (Task 1)
- `crates/rustline/src/plugin_index.rs` — index types, parse, filter, fetch, cache (Tasks 4–5)
- `registry/index.json` — the curated index data (Task 4)
- `.github/workflows/ci.yml` — PR gates (Task 8)
- `.github/workflows/release.yml` — tag-triggered release (Task 9)

**Modified:**
- `crates/rustline-wasm/Cargo.toml` — add `sha2` (Task 1)
- `crates/rustline-wasm/src/lib.rs` — module decl, re-exports, the gate in `register_plugins` and `instantiate_named` (Tasks 1–2)
- `crates/rustline-wasm/tests/e2e.rs` — checksum integration tests (Task 3)
- `crates/rustline/Cargo.toml` — drop `sha2` (Task 2)
- `crates/rustline/src/plugin_install.rs` — delete the duplicate `sha256_hex` (Task 2)
- `crates/rustline-core/src/config.rs` — add `plugin_index_url` (Task 5)
- `crates/rustline/src/cli.rs` — `PluginCmd::Search` (Task 6)
- `crates/rustline/src/plugin_cmd.rs` — the `search` command (Task 6)
- `crates/rustline/src/main.rs` — declare `mod plugin_index` (Task 4)
- `crates/rustline/tests/smoke.rs` — CLI smoke tests (Task 6)
- `justfile` — `lint-plugins`, `test-plugins` (Task 7)
- `CLAUDE.md`, `README.md`, `WHATS-NEXT.md` (Task 10)

**Dependency order:** 1 → 2 → 3. 4 → 5 → 6. 7 → 8. Task 9 is independent. Task 10 last.
Tasks 1 and 4 touch disjoint files and may run concurrently; likewise 7 and 9.

---

### Task 1: Pure checksum verification module

**Files:**
- Create: `crates/rustline-wasm/src/integrity.rs`
- Modify: `crates/rustline-wasm/Cargo.toml`
- Modify: `crates/rustline-wasm/src/lib.rs` (module declaration + re-export only)

**Interfaces:**
- Consumes: nothing.
- Produces: `rustline_wasm::integrity::{sha256_hex, verify_checksum, ChecksumVerdict}`, re-exported as `rustline_wasm::{sha256_hex, verify_checksum, ChecksumVerdict}`.
  - `pub fn sha256_hex(bytes: &[u8]) -> String`
  - `pub fn verify_checksum(recorded: Option<&str>, bytes: &[u8]) -> ChecksumVerdict`
  - `pub enum ChecksumVerdict { NotRecorded, Match, Mismatch { expected: String, actual: String }, Malformed { recorded: String } }`
  - `pub fn ChecksumVerdict::allows_load(&self) -> bool`

- [ ] **Step 1: Add the `sha2` dependency**

In `crates/rustline-wasm/Cargo.toml`, add to `[dependencies]` (keep the existing entries; place it after `serde_json` to match the file's rough grouping):

```toml
# sha256 for plugin-binary integrity verification (W19). Already in the
# workspace lock via the bin, so this adds no new graph node — and no TLS.
sha2 = "0.10"
```

- [ ] **Step 2: Write the failing tests**

Create `crates/rustline-wasm/src/integrity.rs` containing ONLY the test module for now (the implementation comes in Step 4):

```rust
//! Plugin binary integrity (W19): the sha256 digest helper plus the pure
//! decision function a plugin load is gated on.
//!
//! Kept deliberately free of I/O, Extism, and config types so the whole policy
//! — including every rejection path — is unit-testable without a real `.wasm`.

#[cfg(test)]
mod tests {
    use super::*;

    /// sha256 of the empty input, from the FIPS 180-4 test vectors.
    const EMPTY_SHA256: &str =
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    /// sha256 of b"hello".
    const HELLO_SHA256: &str =
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[test]
    fn sha256_hex_matches_known_vectors() {
        assert_eq!(sha256_hex(b""), EMPTY_SHA256);
        assert_eq!(sha256_hex(b"hello"), HELLO_SHA256);
    }

    #[test]
    fn sha256_hex_is_64_lowercase_hex_chars() {
        let h = sha256_hex(b"anything");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn absent_checksum_is_not_recorded() {
        assert_eq!(verify_checksum(None, b"hello"), ChecksumVerdict::NotRecorded);
    }

    #[test]
    fn empty_or_blank_checksum_counts_as_not_recorded() {
        // A user clearing the field means "don't verify", not "verify against
        // nothing" — this must not brick the plugin.
        assert_eq!(verify_checksum(Some(""), b"hello"), ChecksumVerdict::NotRecorded);
        assert_eq!(verify_checksum(Some("   "), b"hello"), ChecksumVerdict::NotRecorded);
    }

    #[test]
    fn matching_checksum_verifies() {
        assert_eq!(verify_checksum(Some(HELLO_SHA256), b"hello"), ChecksumVerdict::Match);
    }

    #[test]
    fn matching_checksum_is_case_insensitive() {
        let upper = HELLO_SHA256.to_ascii_uppercase();
        assert_eq!(verify_checksum(Some(&upper), b"hello"), ChecksumVerdict::Match);
    }

    #[test]
    fn sha256_prefix_is_accepted() {
        let prefixed = format!("sha256:{HELLO_SHA256}");
        assert_eq!(verify_checksum(Some(&prefixed), b"hello"), ChecksumVerdict::Match);
    }

    #[test]
    fn surrounding_whitespace_is_tolerated() {
        let padded = format!("  {HELLO_SHA256}  ");
        assert_eq!(verify_checksum(Some(&padded), b"hello"), ChecksumVerdict::Match);
    }

    #[test]
    fn differing_checksum_is_a_mismatch_reporting_both_digests() {
        let verdict = verify_checksum(Some(EMPTY_SHA256), b"hello");
        assert_eq!(
            verdict,
            ChecksumVerdict::Mismatch {
                expected: EMPTY_SHA256.to_string(),
                actual: HELLO_SHA256.to_string(),
            }
        );
    }

    #[test]
    fn too_short_checksum_is_malformed() {
        assert_eq!(
            verify_checksum(Some("abc123"), b"hello"),
            ChecksumVerdict::Malformed { recorded: "abc123".to_string() }
        );
    }

    #[test]
    fn too_long_checksum_is_malformed() {
        let long = format!("{HELLO_SHA256}ff");
        assert_eq!(
            verify_checksum(Some(&long), b"hello"),
            ChecksumVerdict::Malformed { recorded: long }
        );
    }

    #[test]
    fn non_hex_checksum_of_correct_length_is_malformed() {
        let bad = "z".repeat(64);
        assert_eq!(
            verify_checksum(Some(&bad), b"hello"),
            ChecksumVerdict::Malformed { recorded: bad }
        );
    }

    #[test]
    fn allows_load_permits_only_absent_and_matching() {
        assert!(ChecksumVerdict::NotRecorded.allows_load());
        assert!(ChecksumVerdict::Match.allows_load());
        assert!(
            !ChecksumVerdict::Mismatch {
                expected: EMPTY_SHA256.to_string(),
                actual: HELLO_SHA256.to_string(),
            }
            .allows_load()
        );
        assert!(!ChecksumVerdict::Malformed { recorded: "x".to_string() }.allows_load());
    }
}
```

Add the module declaration to `crates/rustline-wasm/src/lib.rs`, alongside the existing `mod` lines (keep them alphabetical where they already are):

```rust
pub mod integrity;
```

and add to the existing re-export block near `pub use host::{...}`:

```rust
pub use integrity::{ChecksumVerdict, sha256_hex, verify_checksum};
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p rustline-wasm integrity`
Expected: FAIL to compile — `cannot find function \`sha256_hex\` in this scope` (and the same for `verify_checksum` / `ChecksumVerdict`).

- [ ] **Step 4: Write the implementation**

Insert above the `#[cfg(test)] mod tests` block in `crates/rustline-wasm/src/integrity.rs`:

```rust
use std::fmt::Write as _;

use sha2::{Digest, Sha256};

/// Lowercase hex sha256 of `bytes` (64 chars) — the `checksum` recorded for an
/// installed plugin, and the value verified against at load time.
///
/// This is the single definition; `rustline`'s `plugin_install` calls it from
/// here rather than keeping its own copy, so the digest written at install time
/// and the digest checked at load time can never drift apart.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().fold(String::with_capacity(64), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// The outcome of checking a plugin's bytes against its recorded `checksum`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChecksumVerdict {
    /// No digest recorded (or the field was cleared) — load as before.
    NotRecorded,
    /// Recorded digest matches the bytes on disk.
    Match,
    /// Recorded digest is well-formed but describes different bytes.
    Mismatch { expected: String, actual: String },
    /// Recorded digest could not be parsed as a sha256 hex string.
    ///
    /// Treated as a refusal (**fail closed**): a user who recorded a checksum
    /// is asking for verification, and a value we cannot parse is one we cannot
    /// verify against.
    Malformed { recorded: String },
}

impl ChecksumVerdict {
    /// Whether a plugin with this verdict may be registered.
    pub fn allows_load(&self) -> bool {
        matches!(self, Self::NotRecorded | Self::Match)
    }
}

/// Normalize a recorded digest for comparison: trim, drop an optional
/// `sha256:` prefix, lowercase. `None` when the result is not 64 hex digits.
fn normalize(recorded: &str) -> Option<String> {
    let trimmed = recorded.trim();
    let trimmed = trimmed.strip_prefix("sha256:").unwrap_or(trimmed).trim();
    if trimmed.len() != 64 || !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

/// Check `bytes` against a plugin's recorded `checksum`.
///
/// An absent or blank `recorded` yields [`ChecksumVerdict::NotRecorded`]; an
/// unparseable one yields [`ChecksumVerdict::Malformed`] and fails closed.
pub fn verify_checksum(recorded: Option<&str>, bytes: &[u8]) -> ChecksumVerdict {
    let Some(recorded) = recorded else {
        return ChecksumVerdict::NotRecorded;
    };
    if recorded.trim().is_empty() {
        return ChecksumVerdict::NotRecorded;
    }
    let Some(expected) = normalize(recorded) else {
        return ChecksumVerdict::Malformed { recorded: recorded.to_string() };
    };
    let actual = sha256_hex(bytes);
    if actual == expected {
        ChecksumVerdict::Match
    } else {
        ChecksumVerdict::Mismatch { expected, actual }
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rustline-wasm integrity`
Expected: PASS — 13 tests.

- [ ] **Step 6: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add crates/rustline-wasm/src/integrity.rs crates/rustline-wasm/src/lib.rs crates/rustline-wasm/Cargo.toml Cargo.lock
git commit -m "feat(wasm): pure plugin checksum verification (W19)"
```

---

### Task 2: Wire the gate into plugin loading; remove the duplicate digest fn

**Files:**
- Modify: `crates/rustline-wasm/src/lib.rs` (`register_plugins` ~:113–117, `instantiate_named` ~:194)
- Modify: `crates/rustline/src/plugin_install.rs:113-121` (delete `sha256_hex`)
- Modify: `crates/rustline/Cargo.toml` (drop `sha2`)

**Interfaces:**
- Consumes: `rustline_wasm::integrity::{verify_checksum, ChecksumVerdict}` from Task 1.
- Produces: `register_plugins` now refuses a plugin whose recorded checksum does not verify. `crates/rustline/src/plugin_install.rs` re-exports `sha256_hex` from `rustline_wasm`, so `crate::plugin_install::sha256_hex` keeps resolving for existing callers and tests.

**Note on this task's test cycle.** A meaningful test of the gate needs a real
`.wasm`, because on a stub file a checksum refusal and an instantiation failure
are indistinguishable — both just leave the registry empty. A unit test here
would pass for the wrong reason. So this task's verification is *"the gate is
wired and nothing regressed"* (the full workspace suite plus the four
pre-existing wasm-e2e tests), and **Task 3 is where the gate is actually
proven**. Do not write a stub-file unit test to satisfy TDD ritual — it would be
a test that cannot fail for the right reason.

- [ ] **Step 1: Record the pre-change baseline**

```bash
cargo test --workspace 2>&1 | grep -c '^test result: ok'
just test-wasm
```
Expected: workspace green (859 passing at branch point); `just test-wasm` green with 4 e2e + 2 wasm_wiring tests. Write the numbers down — Step 6 compares against them.

- [ ] **Step 2: Add the gate to `register_plugins`**

In `crates/rustline-wasm/src/lib.rs`, immediately after the `let Ok(wasm) = std::fs::read(&path) else { ... };` block (currently ~:113–116) and **before** the `FileDenialObserver` line, insert:

```rust
        // W19: verify the recorded digest before building any capability object
        // and before wasmtime ever sees the module. An absent checksum loads as
        // before; a mismatched or unparseable one fails closed. Like every other
        // skip site here, this drops one widget with a warn — never an error
        // into the render path (invariant N2).
        match integrity::verify_checksum(pc.checksum.as_deref(), &wasm) {
            integrity::ChecksumVerdict::NotRecorded | integrity::ChecksumVerdict::Match => {}
            integrity::ChecksumVerdict::Mismatch { expected, actual } => {
                tracing::warn!(
                    plugin = %stem,
                    %expected,
                    %actual,
                    "plugin checksum mismatch, skipping"
                );
                continue;
            }
            integrity::ChecksumVerdict::Malformed { recorded } => {
                tracing::warn!(
                    plugin = %stem,
                    %recorded,
                    "recorded plugin checksum is not a valid sha256 digest, skipping"
                );
                continue;
            }
        }
```

- [ ] **Step 3: Make `instantiate_named` warn-only**

In `crates/rustline-wasm/src/lib.rs`, in `instantiate_named`, replace the first line of the body:

```rust
    let wasm = std::fs::read(plugin_dir.join(format!("{name}.wasm"))).ok()?;
```

with:

```rust
    let wasm = std::fs::read(plugin_dir.join(format!("{name}.wasm"))).ok()?;
    // Dev harness (`rustline plugin run`): report a bad digest but still run.
    // This command is used while iterating on a plugin you just rebuilt, where
    // a recorded checksum legitimately mismatches on every build. The real gate
    // is `register_plugins`, which the bar, the daemon, and `plugin list` all
    // go through.
    let verdict = integrity::verify_checksum(pc.checksum.as_deref(), &wasm);
    if !verdict.allows_load() {
        tracing::warn!(
            plugin = %name,
            ?verdict,
            "plugin checksum did not verify; running anyway (dev harness)"
        );
    }
```

Also extend `instantiate_named`'s doc comment — it enumerates the checks it
bypasses, and this is now one of them. Append to the existing sentence listing
bypassed checks: `, and (unlike \`register_plugins\`) treats a failed checksum
verification as a warning rather than a refusal`.

- [ ] **Step 4: Collapse the duplicate `sha256_hex`**

In `crates/rustline/src/plugin_install.rs`, delete the whole `sha256_hex` function (currently :113–121) and replace it with a re-export placed next to the other `use` statements:

```rust
/// Re-exported so `plugin_install::sha256_hex` keeps resolving for existing
/// callers and tests. The definition lives in `rustline-wasm` beside the
/// verification that consumes it, so the digest written at install time and the
/// one checked at load time cannot drift (the same single-definition argument
/// W51 applied to the wire types).
pub use rustline_wasm::sha256_hex;
```

Then remove the now-unused imports from the top of that file: `use sha2::{Digest, Sha256};`, and `use std::fmt::Write as _;` **only if** nothing else in the file uses `write!` on a `String` — let `cargo clippy -D warnings` tell you (an unused import is an error under `-D warnings`).

Remove `sha2 = "0.10"` from `crates/rustline/Cargo.toml`'s `[dependencies]`.

- [ ] **Step 5: Verify nothing regressed**

Run: `cargo test --workspace`
Expected: PASS with the same count as Step 1's baseline.

Run: `just test-wasm`
Expected: PASS — the 4 pre-existing e2e tests and 2 `wasm_wiring` tests still
green. This is the meaningful check for this task: those tests load a real
plugin with **no** recorded checksum, so they prove the new gate did not break
the unpinned path that every existing user is on.

Run: `cargo tree -i openssl; cargo tree -i native-tls`
Expected: both report nothing (`package ID specification ... did not match any packages`).

- [ ] **Step 6: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add -A
git commit -m "feat(wasm): gate plugin registration on the recorded checksum (W19)

register_plugins now verifies a plugin's bytes against [plugins.<name>].checksum
before constructing its CapabilityCtx or handing the module to wasmtime. Absent
checksum loads as before; mismatch and malformed both fail closed with a warn,
matching the existing collision/name/ABI skip sites (N2).

plugin run stays warn-only — it is a dev harness used right after a rebuild.

sha256_hex moves to rustline-wasm so install-time and load-time digests share
one definition; the bin re-exports it.

No new unit test here on purpose: on a stub file a checksum refusal and an
instantiation failure are indistinguishable, so such a test could not fail for
the right reason. The gate is proven against a real .wasm in the next commit."
```

---

### Task 3: End-to-end checksum tests against a real `.wasm`

**Files:**
- Modify: `crates/rustline-wasm/tests/e2e.rs`

**Interfaces:**
- Consumes: `register_plugins` from Task 2; `rustline_wasm::sha256_hex` from Task 1.
- Produces: nothing consumed downstream.

This is the load-bearing task. The checksum gate is a shared funnel every plugin
load now passes through, so each legitimate producer that must survive it gets
its own test — per the project's rule that a skipped test justified by an
invariant is exactly the test you need.

- [ ] **Step 1: Write the failing tests**

Append to `crates/rustline-wasm/tests/e2e.rs`. Reuse the file's existing helper
for locating `weather.wasm` and building a `Config` — match the local style; the
sketch below shows the assertions, not a new set of helpers.

The file already has a `weather_wasm() -> Vec<u8>` helper and imports
`rustline_core::{Config, PluginConfig, Registry, ...}` plus
`rustline_wasm::register_plugins` at the top — reuse them. Note the existing
tests use **`Registry::new()`**, not `Registry::default()`. Add
`rustline_wasm::sha256_hex` to the existing `rustline_wasm::{...}` import.

```rust
/// Stage the real weather plugin into a fresh dir, returning (dir, bytes) so a
/// test can compute — or deliberately corrupt — its digest.
fn staged_weather() -> (tempfile::TempDir, Vec<u8>) {
    let bytes = weather_wasm();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("weather.wasm"), &bytes).expect("stage plugin");
    (dir, bytes)
}

/// A Config pinning `weather` to `checksum`.
fn cfg_with_checksum(checksum: Option<String>) -> Config {
    let mut cfg = Config::default();
    cfg.plugins.insert(
        "weather".to_string(),
        PluginConfig { checksum, ..Default::default() },
    );
    cfg
}

fn register_weather(cfg: &Config, dir: &std::path::Path) -> Registry {
    let mut reg = Registry::new();
    register_plugins(&mut reg, cfg, dir, &["weather".to_string()]);
    reg
}

/// Producer 1: a hand-installed plugin (or one built by `just build-plugin`)
/// has no recorded digest and must keep loading.
#[test]
fn plugin_without_a_recorded_checksum_still_registers() {
    let (dir, _bytes) = staged_weather();
    let reg = register_weather(&cfg_with_checksum(None), dir.path());
    assert!(reg.contains("weather"), "an unpinned plugin must still load");
}

/// Producer 2: a plugin installed via `plugin install` records the digest of
/// exactly these bytes, so it must verify.
#[test]
fn plugin_with_a_matching_checksum_registers() {
    let (dir, bytes) = staged_weather();
    let reg = register_weather(&cfg_with_checksum(Some(sha256_hex(&bytes))), dir.path());
    assert!(reg.contains("weather"), "a correctly pinned plugin must load");
}

/// The threat this feature exists for: the file on disk was swapped after the
/// digest was recorded. Note the module itself is perfectly valid here — only
/// the digest disagrees — so this can only pass if the gate is real.
#[test]
fn plugin_with_a_mismatched_checksum_does_not_register() {
    let (dir, _bytes) = staged_weather();
    let wrong = sha256_hex(b"different bytes entirely");
    let reg = register_weather(&cfg_with_checksum(Some(wrong)), dir.path());
    assert!(
        !reg.contains("weather"),
        "a tampered plugin must be refused even though the module is valid"
    );
}

/// Fail closed: an unparseable digest is a request to verify that we cannot honour.
#[test]
fn plugin_with_a_malformed_checksum_does_not_register() {
    let (dir, _bytes) = staged_weather();
    let reg = register_weather(
        &cfg_with_checksum(Some("not-a-digest".to_string())),
        dir.path(),
    );
    assert!(!reg.contains("weather"), "a malformed pin must fail closed");
}

/// Round-trip across the install -> load seam. This is what pins the two halves
/// together: if either side ever changes its hex encoding, adds a `sha256:`
/// prefix on write, or hashes something other than the raw file bytes, this
/// fails loudly instead of silently disabling every pinned plugin.
#[test]
fn a_digest_written_at_install_time_is_accepted_at_load_time() {
    let (dir, bytes) = staged_weather();
    // Exactly what `plugin install` records: sha256_hex over the downloaded
    // asset bytes, which are the same bytes written to <plugin_dir>/<name>.wasm.
    let recorded = sha256_hex(&bytes);
    assert_eq!(recorded.len(), 64, "recorded digest shape must stay stable");

    let reg = register_weather(&cfg_with_checksum(Some(recorded)), dir.path());
    assert!(
        reg.contains("weather"),
        "the install path's digest must satisfy the load path's verification"
    );
}
```

- [ ] **Step 2: Build the weather plugin and run the tests to verify they fail**

```bash
just build-weather
cargo test -p rustline-wasm --features wasm-e2e --test e2e checksum
```

Expected: the mismatch/malformed tests FAIL if Task 2's gate is missing; with Task 2 done they should pass. If every test passes on first run, confirm they are real by temporarily reverting the gate and watching `plugin_with_a_mismatched_checksum_does_not_register` fail.

- [ ] **Step 3: Run the full opt-in suite**

Run: `just test-wasm`
Expected: PASS — the 4 pre-existing e2e tests plus the 5 new ones, and the 2 `wasm_wiring` tests.

- [ ] **Step 4: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add crates/rustline-wasm/tests/e2e.rs
git commit -m "test(wasm): pin the checksum gate against a real .wasm

One test per legitimate producer that must survive the new load funnel
(unpinned, correctly pinned, tampered, malformed), plus a round-trip test
tying the install-time digest to the load-time check."
```

---

### Task 4: The curated index file and pure index logic

**Files:**
- Create: `registry/index.json`
- Create: `crates/rustline/src/plugin_index.rs`
- Modify: `crates/rustline/src/main.rs` (add `mod plugin_index;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct PluginIndex { pub schema_version: u32, pub plugins: Vec<IndexEntry> }`
  - `pub struct IndexEntry { pub name: String, pub description: String, pub source: Option<String>, pub bundled: bool, pub capabilities: Vec<String> }`
  - `pub const SCHEMA_VERSION: u32 = 1`
  - `pub const DEFAULT_INDEX_URL: &str`
  - `pub const INDEX_CACHE_FILE: &str = "plugin-index.json"`
  - `pub const INDEX_TTL_SECS: u64`
  - `pub fn parse_index(body: &str) -> anyhow::Result<PluginIndex>`
  - `pub fn parse_index_value(v: &serde_json::Value) -> anyhow::Result<PluginIndex>`
  - `pub fn filter_entries<'a>(index: &'a PluginIndex, query: Option<&str>) -> Vec<&'a IndexEntry>`
  - `pub fn index_is_fresh(fetched_at: u64, now: u64, ttl_secs: u64) -> bool`

- [ ] **Step 1: Create the index data**

Create `registry/index.json`:

```json
{
  "schema_version": 1,
  "plugins": [
    {
      "name": "weather",
      "description": "Nerd-Font condition icon + °F for a configured zip code, via wttr.in",
      "source": "stevenwcarter/rustline",
      "bundled": true,
      "capabilities": ["http_cached"]
    },
    {
      "name": "counter",
      "description": "Sandboxed-state counter: reads, increments, and persists a count each render",
      "source": "stevenwcarter/rustline",
      "bundled": true,
      "capabilities": ["state"]
    },
    {
      "name": "filewatch",
      "description": "First line and line count of a configured file, via the gated file-read capability",
      "source": "stevenwcarter/rustline",
      "bundled": true,
      "capabilities": ["file_read"]
    },
    {
      "name": "httpget",
      "description": "Snippet of a configured URL's body, via the plain uncached HTTP GET capability",
      "source": "stevenwcarter/rustline",
      "bundled": true,
      "capabilities": ["http"]
    },
    {
      "name": "cmdrun",
      "description": "First line of a configured command's stdout, via the gated exec capability",
      "source": "stevenwcarter/rustline",
      "bundled": true,
      "capabilities": ["exec", "exec_cached"]
    }
  ]
}
```

- [ ] **Step 2: Write the failing tests**

Create `crates/rustline/src/plugin_index.rs` with the doc comment and test module only:

```rust
//! The curated plugin index (W49): the wire types for `registry/index.json`,
//! the pure parse/filter/freshness logic, and the fetch-with-TTL-cache that
//! backs `rustline plugin search`.
//!
//! Discovery grants nothing. Like `plugin install`, finding a plugin here never
//! widens an allowlist — only `plugin approve` or a hand edit does. The
//! `capabilities` field is advertising copy so a user can see what a plugin will
//! ask for *before* installing it; the host never consults it.

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
        assert!(parse_index(body).is_err(), "a future schema must be refused, not misread");
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
        assert!(index_is_fresh(1_000, 1_099, 100), "inside the window is fresh");
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
                e.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "{}: plugin names must be [A-Za-z0-9_-] (invariant #7)",
                e.name
            );
            assert_ne!(e.name, "window", "`window` is reserved");
            assert!(!e.description.is_empty(), "{}: needs a description", e.name);
        }
    }
}
```

Add `mod plugin_index;` to `crates/rustline/src/main.rs` alongside the other `mod` declarations.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p rustline plugin_index`
Expected: FAIL to compile — `cannot find function \`parse_index\``.

- [ ] **Step 4: Write the implementation**

Insert above the test module in `crates/rustline/src/plugin_index.rs`:

```rust
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
    let index: PluginIndex =
        serde_json::from_str(body).context("parse plugin index JSON")?;
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
        .filter(|e| {
            e.name.to_lowercase().contains(&q) || e.description.to_lowercase().contains(&q)
        })
        .collect()
}

/// Whether a cache stamped `fetched_at` is still fresh at `now`. A clock that
/// moved backwards counts as stale (refetch) rather than fresh forever.
pub fn index_is_fresh(fetched_at: u64, now: u64, ttl_secs: u64) -> bool {
    now >= fetched_at && now.saturating_sub(fetched_at) < ttl_secs
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rustline plugin_index`
Expected: PASS — 12 tests.

- [ ] **Step 6: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add registry/index.json crates/rustline/src/plugin_index.rs crates/rustline/src/main.rs
git commit -m "feat(plugin): curated plugin index data + pure parse/filter logic (W49)"
```

---

### Task 5: Index fetch, TTL cache, and the config override

**Files:**
- Modify: `crates/rustline-core/src/config.rs` (add `plugin_index_url` to `Config`)
- Modify: `crates/rustline/src/plugin_index.rs` (add fetch/cache)

**Interfaces:**
- Consumes: Task 4's types and constants; `crate::plugin_install::Downloader`; `crate::sample_store::{read_sample, write_sample, now_unix_secs}`.
- Produces:
  - `Config.plugin_index_url: Option<String>`
  - `pub struct LoadedIndex { pub index: PluginIndex, pub stale: bool }`
  - `pub fn load_index<D: Downloader>(dl: &D, state_dir: &Path, url: &str, refresh: bool, now: u64) -> anyhow::Result<LoadedIndex>`

- [ ] **Step 1: Add the config field with a failing test**

Add to `crates/rustline-core/src/config.rs`, inside `pub struct Config` right after the `plugin_dir` field:

```rust
    /// Where `rustline plugin search` fetches the curated plugin index from;
    /// overrides the built-in default URL. Lets a user point at a self-hosted
    /// or alternate index without a code change.
    #[serde(default)]
    pub plugin_index_url: Option<String>,
```

Add a test in that file's existing `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn plugin_index_url_defaults_to_none_and_round_trips() {
        let cfg: Config = toml::from_str("").expect("empty config parses");
        assert_eq!(cfg.plugin_index_url, None);

        let cfg: Config = toml::from_str(r#"plugin_index_url = "https://example.test/i.json""#)
            .expect("config with an index url parses");
        assert_eq!(cfg.plugin_index_url.as_deref(), Some("https://example.test/i.json"));
    }

    #[test]
    fn a_garbage_plugin_index_url_still_loads_the_config() {
        // Invariant #3: Config::load is total. A bad URL is a failed fetch at
        // search time, never a config-load failure that breaks the bar.
        let cfg: Config = toml::from_str(r#"plugin_index_url = "not a url at all""#)
            .expect("any string value must parse");
        assert!(cfg.plugin_index_url.is_some());
    }
```

Run: `cargo test -p rustline-core plugin_index_url`
Expected: FAIL to compile until the field is added; PASS after.

- [ ] **Step 2: Write the failing fetch/cache tests**

Append to the test module in `crates/rustline/src/plugin_index.rs`:

```rust
    use std::cell::RefCell;

    /// A `Downloader` that serves a canned body and counts calls, so a test can
    /// assert the cache actually prevented a fetch.
    struct FakeDownloader {
        body: RefCell<Result<String, String>>,
        calls: RefCell<usize>,
    }

    impl FakeDownloader {
        fn ok(body: &str) -> Self {
            Self { body: RefCell::new(Ok(body.to_string())), calls: RefCell::new(0) }
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
        assert!(dir.path().join(INDEX_CACHE_FILE).exists(), "the fetch must be cached");
    }

    #[test]
    fn a_fresh_cache_is_served_without_fetching() {
        let dir = tempfile::tempdir().unwrap();
        let dl = FakeDownloader::ok(sample_json());
        load_index(&dl, dir.path(), "http://x", false, 1_000).unwrap();

        let loaded = load_index(&dl, dir.path(), "http://x", false, 1_500).expect("cache hit");
        assert_eq!(dl.calls(), 1, "a fresh cache must not hit the network again");
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

        load_index(&dl, dir.path(), "http://x", false, 1_000 + INDEX_TTL_SECS + 1).unwrap();
        assert_eq!(dl.calls(), 2);
    }

    #[test]
    fn a_failed_fetch_serves_the_stale_cache_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        load_index(&FakeDownloader::ok(sample_json()), dir.path(), "http://x", false, 1_000)
            .unwrap();

        let dl = FakeDownloader::failing();
        let loaded = load_index(&dl, dir.path(), "http://x", false, 9_999_999)
            .expect("a stale cache beats no answer");
        assert!(loaded.stale, "the caller must be able to warn that this is stale");
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
```

Add `tempfile` to `crates/rustline`'s `[dev-dependencies]` if it is not already there (it is).

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p rustline plugin_index`
Expected: FAIL to compile — `cannot find function \`load_index\``.

- [ ] **Step 4: Implement fetch + cache**

Append to the implementation portion of `crates/rustline/src/plugin_index.rs`:

```rust
use std::path::Path;

use crate::plugin_install::Downloader;
use crate::sample_store::{read_sample, write_sample};

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
    let cached = CachedIndex { fetched_at: now, index: index.clone() };
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
        return Ok(LoadedIndex { index: c.index.clone(), stale: false });
    }

    match dl.get_json(url).and_then(|v| parse_index_value(&v)) {
        Ok(index) => {
            write_cache(state_dir, &index, now);
            Ok(LoadedIndex { index, stale: false })
        }
        Err(fetch_err) => match cached {
            Some(c) => {
                tracing::warn!(%url, error = %fetch_err, "plugin index fetch failed; serving the cached copy");
                Ok(LoadedIndex { index: c.index, stale: true })
            }
            None => Err(fetch_err.context(format!("fetch plugin index from {url}"))),
        },
    }
}
```

> Note: `if !refresh && let Some(..) = .. && ..` uses let-chains, stable in
> edition 2024. If the toolchain rejects it, rewrite as nested `if let` blocks —
> do not change the behavior.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rustline plugin_index && cargo test -p rustline-core plugin_index_url`
Expected: PASS — 19 index tests + 2 config tests.

- [ ] **Step 6: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add -A
git commit -m "feat(plugin): fetch the plugin index with a TTL cache and config override (W49)

Reuses plugin_install's existing rustls Downloader seam, caches under the state
dir for 24h, serves the last-known-good copy when a fetch fails, and honours a
new plugin_index_url config key."
```

---

### Task 6: The `plugin search` command

**Files:**
- Modify: `crates/rustline/src/cli.rs` (`PluginCmd`)
- Modify: `crates/rustline/src/plugin_cmd.rs` (dispatch + the `search` fn + JSON)
- Modify: `crates/rustline/tests/smoke.rs`

**Interfaces:**
- Consumes: `plugin_index::{load_index, filter_entries, search_json, DEFAULT_INDEX_URL, IndexEntry}`; `rustline_wasm::discover_plugin_names`.
- Produces: the `rustline plugin search [QUERY] [--json] [--refresh]` CLI surface. `pub fn search_json(entries: &[&IndexEntry], installed: &[String]) -> String` in `plugin_index`.

- [ ] **Step 1: Add the CLI variant**

In `crates/rustline/src/cli.rs`, add to `pub enum PluginCmd` (after `List`):

```rust
    /// Search the curated plugin index for widgets you can install.
    Search {
        /// Case-insensitive filter over name and description. Omit to list
        /// every plugin in the index.
        query: Option<String>,
        /// Emit JSON instead of human-readable text.
        #[arg(long)]
        json: bool,
        /// Bypass the 24-hour cache and fetch the index now.
        #[arg(long)]
        refresh: bool,
    },
```

- [ ] **Step 2: Write the failing `search_json` test**

Append to the test module in `crates/rustline/src/plugin_index.rs`:

```rust
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
```

Run: `cargo test -p rustline search_json` → FAIL (`cannot find function search_json`).

- [ ] **Step 3: Implement `search_json`**

Append to `crates/rustline/src/plugin_index.rs`'s implementation section:

```rust
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
```

Run: `cargo test -p rustline search_json` → PASS.

- [ ] **Step 4: Wire the command**

In `crates/rustline/src/plugin_cmd.rs`, add the match arm to `run`'s dispatch:

```rust
        PluginCmd::Search { query, json, refresh } => {
            search(config_path, plugin_dir, query.as_deref(), json, refresh)
        }
```

and add the function (place it next to `list`):

```rust
/// `rustline plugin search [QUERY] [--json] [--refresh]` — browse the curated
/// plugin index. Read-only: it touches neither the config nor the plugin dir,
/// and finding a plugin here grants it nothing (only `plugin approve` or a hand
/// edit ever widens an allowlist).
fn search(config_path: &Path, plugin_dir: &Path, query: Option<&str>, json: bool, refresh: bool) {
    let cfg = Config::load(config_path);
    let url = cfg
        .plugin_index_url
        .as_deref()
        .unwrap_or(crate::plugin_index::DEFAULT_INDEX_URL);
    let state_dir = rustline_wasm::state_root();
    let now = crate::sample_store::now_unix_secs();

    let loaded = match crate::plugin_index::load_index(
        &crate::plugin_install::UreqDownloader,
        &state_dir,
        url,
        refresh,
        now,
    ) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("could not load the plugin index: {error:#}");
            std::process::exit(1);
        }
    };

    let entries = crate::plugin_index::filter_entries(&loaded.index, query);
    let installed = rustline_wasm::discover_plugin_names(plugin_dir);

    if json {
        println!("{}", crate::plugin_index::search_json(&entries, &installed));
        return;
    }

    if loaded.stale {
        eprintln!("warning: could not refresh the plugin index; showing a cached copy");
    }
    if entries.is_empty() {
        match query {
            Some(q) => println!("no plugins in the index match {q:?}"),
            None => println!("the plugin index is empty"),
        }
        return;
    }

    for entry in &entries {
        let mark = if installed.iter().any(|n| n == &entry.name) {
            "  [installed]"
        } else {
            ""
        };
        println!("{}{}", entry.name, mark);
        if !entry.description.is_empty() {
            println!("  {}", entry.description);
        }
        if !entry.capabilities.is_empty() {
            println!("  capabilities: {}", entry.capabilities.join(", "));
        }
        match (entry.bundled, entry.source.as_deref()) {
            (true, _) => println!("  build:   just build-plugin {}", entry.name),
            (false, Some(source)) => {
                println!("  install: rustline plugin install {source}");
            }
            (false, None) => {}
        }
        println!();
    }
}
```

- [ ] **Step 5: Write the CLI smoke tests**

Append to `crates/rustline/tests/smoke.rs`. The file's existing helper is
`fn isolate(cmd: &mut Command, tmp: &Path)` — it sets `HOME`, `XDG_DATA_HOME =
tmp/data`, and `XDG_RUNTIME_DIR` on a command you have already built. Each test
constructs `Command::new(env!("CARGO_BIN_EXE_rustline"))` inline; follow that
pattern rather than adding a runner helper.

Because `isolate` sets `XDG_DATA_HOME = tmp/data` and
`rustline_wasm::state_root()` is `$XDG_DATA_HOME/rustline/state`, the cache the
test seeds must land at `tmp/data/rustline/state/plugin-index.json`.

```rust
/// Seed the plugin-index cache so `plugin search` answers from disk and never
/// touches the network. Path mirrors `state_root()` under the `XDG_DATA_HOME`
/// that `isolate` sets.
fn seed_index_cache(tmp: &Path) {
    let state = tmp.join("data/rustline/state");
    fs::create_dir_all(&state).expect("state dir");
    // A far-future `fetched_at` keeps the entry fresh regardless of the clock,
    // so the command never attempts a fetch.
    let body = r#"{"fetched_at":99999999999,"index":{"schema_version":1,"plugins":[
        {"name":"weather","description":"Weather from wttr.in","source":"o/r","bundled":true,"capabilities":["http_cached"]},
        {"name":"othertool","description":"Something else entirely","source":"o/r2","bundled":false,"capabilities":[]}
    ]}}"#;
    fs::write(state.join("plugin-index.json"), body).expect("seed index cache");
}

#[test]
fn plugin_search_lists_the_index() {
    let tmp = tempdir().unwrap();
    seed_index_cache(tmp.path());
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["plugin", "search"]);
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();

    assert!(out.status.success(), "plugin search should succeed");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("weather"), "index entries listed: {s}");
    assert!(s.contains("othertool"), "{s}");
    assert!(
        s.contains("just build-plugin weather"),
        "a bundled entry shows a build hint, not an install command: {s}"
    );
    assert!(
        s.contains("rustline plugin install o/r2"),
        "a non-bundled entry shows its install command: {s}"
    );
}

#[test]
fn plugin_search_filters_by_query() {
    let tmp = tempdir().unwrap();
    seed_index_cache(tmp.path());
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["plugin", "search", "weath"]);
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();

    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("weather"), "{s}");
    assert!(!s.contains("othertool"), "the query should exclude non-matches: {s}");
}

#[test]
fn plugin_search_json_emits_an_array() {
    let tmp = tempdir().unwrap();
    seed_index_cache(tmp.path());
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["plugin", "search", "--json"]);
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();

    let s = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value =
        serde_json::from_str(&s).unwrap_or_else(|e| panic!("--json must emit valid JSON: {e}: {s}"));
    let arr = v.as_array().expect("array");
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["name"], "weather");
    assert_eq!(arr[0]["installed"], false, "nothing installed in the tempdir");
    assert_eq!(arr[1]["source"], "o/r2");
}
```

> `serde_json` is already a dependency of `crates/rustline`, and `smoke.rs`
> already imports `std::fs`, `std::path::Path`, `std::process::Command`, and
> `tempfile::tempdir` at the top.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p rustline plugin_search`
Expected: PASS — 3 smoke tests.

Run: `cargo test --workspace`
Expected: PASS, no regressions.

- [ ] **Step 7: Verify by hand**

```bash
cargo run -q -- plugin search --json | head -20
cargo run -q -- plugin search weather
```
Expected: the five bundled entries from the real index (this does hit the network
once, then caches).

- [ ] **Step 8: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
git add -A
git commit -m "feat(cli): rustline plugin search over the curated index (W49)"
```

---

### Task 7: Justfile recipes for the excluded plugins

**Files:**
- Modify: `justfile`

**Interfaces:**
- Consumes: nothing.
- Produces: `just lint-plugins` and `just test-plugins`, called by Task 8's CI.

- [ ] **Step 1: Add the recipes**

Append to `justfile`, after the existing `build-plugin` recipe:

```make
# Lint the excluded example plugins (host + wasm32 targets).
#
# plugins/* are EXCLUDED workspace members, so `cargo fmt --all`, `cargo clippy`
# and `cargo test --workspace` at the root never see them. The wasm32 pass is
# load-bearing, not redundant: each plugin's guest code lives behind
# `#[cfg(target_arch = "wasm32")] mod guest`, so a host-only lint compiles just
# the pure logic and never checks the half that actually runs in the sandbox.
lint-plugins:
    #!/usr/bin/env bash
    set -euo pipefail
    rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true
    for p in weather counter filewatch httpget cmdrun; do
        echo "== $p =="
        cargo fmt --check --manifest-path "plugins/$p/Cargo.toml"
        cargo clippy --manifest-path "plugins/$p/Cargo.toml" --all-targets -- -D warnings
        cargo clippy --manifest-path "plugins/$p/Cargo.toml" --target wasm32-unknown-unknown -- -D warnings
    done

# Run the excluded example plugins' host-side unit tests (their pure logic).
test-plugins:
    #!/usr/bin/env bash
    set -euo pipefail
    for p in weather counter filewatch httpget cmdrun; do
        echo "== $p =="
        cargo test --manifest-path "plugins/$p/Cargo.toml"
    done
```

- [ ] **Step 2: Verify both recipes pass**

Run: `just lint-plugins`
Expected: five `== <name> ==` blocks, exit 0.

Run: `just test-plugins`
Expected: five blocks, 22 tests total (3+3+4+4+8), exit 0.

- [ ] **Step 3: Commit**

```bash
git add justfile
git commit -m "chore: just lint-plugins / test-plugins for the excluded example plugins

They are excluded workspace members, so nothing at the root has ever linted or
tested them. The wasm32 clippy pass is what actually covers each plugin's
#[cfg(target_arch = \"wasm32\")] guest module."
```

---

### Task 8: CI workflow

**Files:**
- Create: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `just lint`, `just test`, `just lint-plugins`, `just test-plugins`, `just test-wasm`.
- Produces: PR status checks.

- [ ] **Step 1: Write the workflow**

Create `.github/workflows/ci.yml`:

```yaml
name: CI

on:
  pull_request:
  push:
    branches: [main]

# A newer push to the same ref supersedes an in-flight run.
concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: -D warnings

jobs:
  lint:
    name: fmt + clippy
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - uses: extractions/setup-just@v3
      # `just lint` is the same command the CONTRIBUTING/CLAUDE docs tell a
      # developer to run locally, so the gate and the local check cannot drift.
      - run: just lint

  test:
    name: workspace tests
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - uses: extractions/setup-just@v3
      - run: just test

  plugins:
    name: example plugins
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
          targets: wasm32-unknown-unknown
      - uses: Swatinem/rust-cache@v2
      - uses: extractions/setup-just@v3
      # plugins/* are excluded workspace members — invisible to the jobs above.
      - run: just lint-plugins
      - run: just test-plugins

  wasm-e2e:
    name: wasm end-to-end
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-unknown-unknown
      - uses: Swatinem/rust-cache@v2
      - uses: extractions/setup-just@v3
      # Builds weather.wasm, then runs both opt-in suites — the only coverage of
      # the Extism host boundary. Both bind localhost mock servers; no network.
      - run: just test-wasm
```

- [ ] **Step 2: Validate the YAML locally**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('ci.yml parses')"`
Expected: `ci.yml parses`

- [ ] **Step 3: Sanity-run each job's command locally**

```bash
just lint
just test
just lint-plugins
just test-plugins
just test-wasm
```
Expected: all exit 0. (These are exactly what CI will run.)

> Note on `RUSTFLAGS: -D warnings`: this makes *any* rustc warning fail CI, not
> just clippy lints. If it turns any job red on code that is otherwise fine,
> remove the `env:` block rather than sprinkling `#[allow]` — `just lint`
> already enforces `-D warnings` for clippy specifically.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: fmt, clippy, workspace tests, example plugins, and wasm e2e on PRs

Every job shells out to an existing just recipe so the CI gate and the
documented local command stay identical."
```

---

### Task 9: Release workflow

**Files:**
- Create: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: a tag-triggered GitHub pre-release with four tarballs + `SHA256SUMS`.

- [ ] **Step 1: Write the workflow**

Create `.github/workflows/release.yml`:

```yaml
name: Release

on:
  push:
    tags: ['v*']

permissions:
  contents: write

env:
  CARGO_TERM_COLOR: always

jobs:
  build:
    name: ${{ matrix.target }}
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - target: x86_64-unknown-linux-gnu
            os: ubuntu-latest
          - target: x86_64-unknown-linux-musl
            os: ubuntu-latest
          - target: aarch64-apple-darwin
            os: macos-14
          - target: x86_64-apple-darwin
            os: macos-13
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - uses: Swatinem/rust-cache@v2
        with:
          key: ${{ matrix.target }}

      # Static musl needs its linker; every other leg is native to its runner.
      - name: Install musl tooling
        if: matrix.target == 'x86_64-unknown-linux-musl'
        run: sudo apt-get update && sudo apt-get install -y musl-tools

      - name: Build
        run: cargo build --release --locked --target ${{ matrix.target }} -p rustline

      - name: Package
        shell: bash
        run: |
          set -euo pipefail
          version="${GITHUB_REF_NAME}"
          stage="rustline-${version}-${{ matrix.target }}"
          mkdir -p "dist/${stage}/completions"

          bin="target/${{ matrix.target }}/release/rustline"
          cp "$bin" "dist/${stage}/rustline"
          cp README.md LICENSE "dist/${stage}/"

          # Every target is native to its runner (the static musl binary runs on
          # the gnu host), so the freshly built binary can generate its own
          # completions.
          "$bin" completions bash > "dist/${stage}/completions/rustline.bash"
          "$bin" completions zsh  > "dist/${stage}/completions/_rustline"
          "$bin" completions fish > "dist/${stage}/completions/rustline.fish"

          tar -C dist -czf "dist/${stage}.tar.gz" "${stage}"
          echo "asset=dist/${stage}.tar.gz" >> "$GITHUB_ENV"

      - uses: actions/upload-artifact@v4
        with:
          name: rustline-${{ matrix.target }}
          path: dist/*.tar.gz
          if-no-files-found: error

  publish:
    name: publish release
    needs: build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/download-artifact@v4
        with:
          path: artifacts
          merge-multiple: true

      - name: Checksums
        shell: bash
        run: |
          set -euo pipefail
          cd artifacts
          sha256sum *.tar.gz > SHA256SUMS
          cat SHA256SUMS

      - uses: softprops/action-gh-release@v2
        with:
          files: |
            artifacts/*.tar.gz
            artifacts/SHA256SUMS
          prerelease: true
          generate_release_notes: true
```

- [ ] **Step 2: Validate the YAML**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/release.yml')); print('release.yml parses')"`
Expected: `release.yml parses`

- [ ] **Step 3: Verify the packaging steps work locally for the host target**

```bash
cargo build --release --locked --target x86_64-unknown-linux-gnu -p rustline
./target/x86_64-unknown-linux-gnu/release/rustline completions bash | head -3
./target/x86_64-unknown-linux-gnu/release/rustline completions zsh | head -3
./target/x86_64-unknown-linux-gnu/release/rustline completions fish | head -3
```
Expected: three non-empty completion scripts. This proves the `completions`
subcommand names used above are correct before relying on them in CI.

> **Known risk — the musl leg.** `extism` pulls wasmtime/cranelift and the
> release profile is `lto = "fat"` + `codegen-units = 1`. `fail-fast: false` is
> set deliberately so a musl failure does not cancel the three healthy legs. If
> musl cannot be made to build, the pre-authorized fallback is to delete that
> one matrix entry and ship Linux gnu only — **say so explicitly in the PR/release
> notes rather than quietly dropping a target.** Do not spend the branch fighting
> it.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: tag-triggered release with linux gnu/musl + macOS arm64/x86_64 tarballs

Each tarball carries the binary, generated shell completions, README and
LICENSE; the publish job emits SHA256SUMS and cuts a pre-release."
```

---

### Task 10: Documentation sync and findings-file cleanup

**Files:**
- Modify: `CLAUDE.md`
- Modify: `README.md`
- Modify: `WHATS-NEXT.md` (git-ignored — edit on disk, do not `git add`)

**Interfaces:**
- Consumes: everything above.
- Produces: docs matching the shipped behavior.

This is the final step per the project's standing rule: the widget/plugin lists
and CLI docs must be synced in **both** `CLAUDE.md` and `README.md`.

- [ ] **Step 1: Update `CLAUDE.md`**

Make each of these edits:

1. **Module map — `rustline-wasm`:** add an `integrity.rs` entry describing
   `sha256_hex`/`ChecksumVerdict`/`verify_checksum` and the fail-closed policy.
2. **Module map — `rustline-wasm/lib.rs`:** in the `register_plugins` paragraph,
   add the checksum gate to the list of skip reasons, noting it runs after the
   file read and before instantiation.
3. **Module map — `instantiate_named`:** note it is warn-only on a checksum
   failure, and why.
4. **Module map — `rustline` bin:** add a `plugin_index.rs` entry.
5. **Module map — `plugin_cmd.rs`:** add `search` to its list of handled
   subcommands.
6. **CLI section:** add
   `rustline plugin search [QUERY] [--json] [--refresh]` with a description
   matching the other `plugin` entries' style.
7. **Config section:** document `plugin_index_url` next to `plugin_dir`, and
   document that `checksum` is now *verified at load time* (previously
   "recorded, never verified") — including that an absent checksum loads as
   before and a malformed one fails closed.
8. **Host-owned state-file names:** add `plugin-index.json` to the list that
   currently names `cpu-sample`, `cpu-history`, `memory-history`,
   `throughput-sample*`, `wasmtime-cache*`, and `daemon.sock`.
9. **Development section:** add `just lint-plugins` and `just test-plugins` to
   the `just` recipe list, and add a short "CI" paragraph describing the two
   workflows and what runs on a PR.
10. **Roadmap:** add a "Done" entry for this branch covering W19 and W49, linking
    the spec and plan paths.
11. **Design docs list:** add the spec and plan for this branch.

- [ ] **Step 2: Update `README.md`**

1. Add `rustline plugin search` to the plugin CLI documentation, with an example
   invocation and sample output.
2. Document `plugin_index_url` in the config reference.
3. Document checksum verification in the plugin-security section: what is
   verified, when, and the absent/mismatch/malformed outcomes.
4. Add an installation section for the release binaries: the four targets, what
   each tarball contains, and how to verify a download against `SHA256SUMS`.
5. Add the two new `just` recipes wherever the others are listed.

- [ ] **Step 3: Strip the shipped items from `WHATS-NEXT.md`**

Remove the whole `### W19.` and `### W49.` entries (including their in-flight
markers). Add a line to the maintenance log at the top:

```
Shipped 2026-07-25 (branch `feat/2026-07-25-plugin-integrity-registry-ci`):
stripped W19 (plugin checksum verification at load time) and W49 (curated
plugin index + `plugin search`). This branch also added greenfield GitHub
Actions CI and a tag-triggered release workflow — not previously tracked here.
No skips this round.
```

`WHATS-NEXT.md` is git-ignored in this repo, so edit it on disk and do not stage it.

- [ ] **Step 4: Verify the docs match reality**

```bash
cargo run -q -- plugin search --help
cargo run -q -- plugin --help
```
Expected: the help text matches what you documented. Fix any drift.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md README.md
git commit -m "docs: plugin checksum verification, plugin search, and the CI/release story"
```

---

## Final verification

Before the branch is considered done:

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
just lint-plugins
just test-plugins
just test-wasm
cargo tree -i openssl
cargo tree -i native-tls
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml')); yaml.safe_load(open('.github/workflows/release.yml')); print('workflows parse')"
```

All must pass; the two `cargo tree` invocations must find nothing.

## Release (post-merge, confirm-first)

Not a task above — it happens after this branch merges to `main` and CI is green
there. **Confirm with the user immediately before executing**, since the repo is
public and currently has zero tags:

```bash
git switch main
git pull
git tag -a v0.1.0 -m "rustline v0.1.0"
git push origin v0.1.0
```

The tag push triggers `release.yml`, which builds the four targets and publishes
a **pre-release** with the tarballs and `SHA256SUMS`.
