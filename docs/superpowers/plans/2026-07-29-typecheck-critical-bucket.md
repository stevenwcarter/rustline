# typecheck Critical bucket (T1–T6) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Execute the six user-selected Critical findings from `TYPECHECK.md` — six compiler-enforced type migrations that turn rustline's security/identity conventions (sandbox path ordering, capability-list pairing, wire-flag assembly, range-name safety, widget-kind dispatch, widget identity) into compile-time facts.

**Architecture:** Each task is one finding = one commit, executed as a compiler-driven migration: introduce the new type with a smart constructor, change the signature at the source, run `cargo check` and fix every error it lists until green (the compiler is the to-do list — do NOT hand-grep for call sites). Order is dependency-driven: T2 → T3 → T4 (the rustline-wasm cluster), then T1 → T5 → T6 (core/bin). Load-bearing pinning tests are written and committed BEFORE the migration they guard.

**Tech Stack:** Rust 1.97, edition 2024, cargo workspace. No new dependencies anywhere in this plan.

## Global Constraints

- Typecheck gate before every commit: `cargo check --workspace --all-targets --all-features` green.
- Lint gate before every commit: `cargo clippy --all-targets --all-features -- -D warnings` and `cargo fmt --all` (run fmt, then `--check` must pass).
- Wire ABI byte-identical (invariant #2): every type serialized across the WASM boundary keeps its exact JSON encoding; newtypes crossing serde use `#[serde(transparent)]`; nothing gains `deny_unknown_fields`.
- `Config::load` stays total (invariant #3): accepted TOML shapes unchanged; bad values degrade warn-and-skip. Never `#[serde(tag = "kind")]` on `Config.instances`.
- Identity chain intact (invariant #7): the emitted range name, `--range` value, toggled key, and `active_format` key stay the same string end-to-end.
- Gate-first (invariant N1): a denied URL/path/argv never reaches a fetch/fs/spawn; every existing denied-case test keeps passing.
- Never break the bar (invariant N2): every warn-and-skip / degrade-to-empty path is preserved behavior.
- NO public-symbol renames (typecheck skill rule): changing a signature/return type is in scope; renaming an existing `pub` item is not — if a step seems to need one, convert the finding to a `decision-needed` marker in `TYPECHECK.md` and stop that task.
- Do not refactor existing test files/`#[cfg(test)]` blocks beyond what the compiler forces (type changes at call sites in tests are fine; restructuring tests is not). New tests are always allowed.
- Every commit strips its finding's block from `TYPECHECK.md` in the same commit (non-negotiable), message format `typecheck(<lens>): <summary> [T<n>]`, ending with the Claude-Session trailer.
- Milestones: after Task 3 (wasm cluster) and after Task 6 (core/bin cluster) run the FULL `just test` + `just lint` + `just check-lock`. On red: bisect within the batch, revert the offender, surface the diagnosis.

---

### Task 1: T2 — `ResolvedPath` / `SandboxRelPath` / `AllowedPath` in `rustline-wasm`

**Files:**
- Modify: `crates/rustline-wasm/src/state.rs` (new types + changed returns of `sanitize_relpath`:10, `normalize_abs`:80, `resolve_for_allowlist`:110)
- Modify: `crates/rustline-wasm/src/allow.rs` (add `AllowSet::check_path`)
- Modify: `crates/rustline-wasm/src/perform.rs` (the four effect sites: ~421, ~452, ~547, ~601, plus the gate calls that feed them)
- Test: in-file `#[cfg(test)]` additions in `state.rs` and `perform.rs`

**Interfaces:**
- Consumes: nothing from other tasks (first task).
- Produces: `state::ResolvedPath` (`as_str(&self) -> &str`, `impl AsRef<Path>`, `impl fmt::Display`; field private; constructed ONLY by `normalize_abs`/`resolve_for_allowlist`), `state::SandboxRelPath` (`impl AsRef<Path>`; constructed ONLY by `sanitize_relpath`), `state::AllowedPath` (`as_str`, `impl AsRef<Path>`; constructed ONLY by `AllowSet::check_path`). Task 2 re-keys `check_path` onto the typed `AllowSet<K>`.

- [ ] **Step 1: Write the failing compile-fence test** (proves construction is sealed). In `state.rs` tests add:

```rust
#[test]
fn resolved_path_only_comes_from_the_resolver() {
    // The only way to obtain a ResolvedPath is via normalize_abs /
    // resolve_for_allowlist. This test just exercises the two constructors
    // and the accessors; the real guarantee is the private field (a
    // `ResolvedPath("x".into())` outside state.rs must not compile).
    let p = normalize_abs("/etc/hostname").unwrap();
    assert_eq!(p.as_str(), "/etc/hostname");
    let r = resolve_for_allowlist("/etc/hostname", false).unwrap();
    assert!(r.as_str().starts_with('/'));
}
```

- [ ] **Step 2: Run it — expect FAIL to compile** (`as_str` on `String` is fine, so this fails once the types exist; before that it fails because the return types are `String`): `cargo test -p rustline-wasm resolved_path_only -- --nocapture`. Expected: compile error or assertion mismatch — either confirms the seam is untyped today.

- [ ] **Step 3: Introduce the three newtypes in `state.rs`:**

```rust
/// An absolute path that has been normalized (and, when configured,
/// symlink-resolved) by this module. Constructible ONLY here, so holding one
/// IS the proof that resolution ran before any allowlist check (N1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPath(String);

impl ResolvedPath {
    pub fn as_str(&self) -> &str { &self.0 }
}
impl AsRef<std::path::Path> for ResolvedPath {
    fn as_ref(&self) -> &std::path::Path { std::path::Path::new(&self.0) }
}
impl std::fmt::Display for ResolvedPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.0.fmt(f) }
}

/// A state-dir-relative path that passed `sanitize_relpath` (no absolute, no
/// `..`). Constructible only here; `state_dir().join(...)` should only ever
/// see one of these (N3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxRelPath(PathBuf);
impl AsRef<std::path::Path> for SandboxRelPath {
    fn as_ref(&self) -> &std::path::Path { &self.0 }
}

/// A ResolvedPath that additionally matched an allowlist. The filesystem
/// effect functions accept ONLY this token, making "resolve → match → act"
/// a type-level fact instead of statement order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllowedPath(ResolvedPath);
impl AllowedPath {
    pub fn as_str(&self) -> &str { self.0.as_str() }
    /// Sole constructor, used by `AllowSet::check_path`. `pub(crate)` so no
    /// caller outside the crate can mint one without an allowlist match.
    pub(crate) fn from_checked(p: ResolvedPath) -> Self { Self(p) }
}
impl AsRef<std::path::Path> for AllowedPath {
    fn as_ref(&self) -> &std::path::Path { std::path::Path::new(self.0.as_str()) }
}
```

Change returns: `sanitize_relpath(..) -> Result<SandboxRelPath, String>` (wrap the final `Ok(p)`), `normalize_abs(..) -> Result<ResolvedPath, PathResolveError>`, `resolve_for_allowlist(..) -> Result<ResolvedPath, PathResolveError>` (wrap each `Ok(...)` site). `PathResolveError` variants stay EXACTLY as they are (their payloads feed `observe_denial`'s verbatim `&str` target — deliberate simplification vs. the finding text; the type-level guarantee lives entirely in the `Ok` path).

- [ ] **Step 4: Add the allowlist token mint in `allow.rs`:**

```rust
impl AllowSet {
    /// Consume a resolved path; return the only token the filesystem effects
    /// accept, or give the path back on a miss (so the caller can report it).
    pub fn check_path(
        &self,
        path: crate::state::ResolvedPath,
    ) -> Result<crate::state::AllowedPath, crate::state::ResolvedPath> {
        if self.allows(path.as_str()) {
            Ok(crate::state::AllowedPath::from_checked(path))
        } else {
            Err(path)
        }
    }
}
```

- [ ] **Step 5: Migrate the source, compiler-driven.** Run `cargo check -p rustline-wasm 2>&1 | head -80`, and fix each error by these rules until green (repeat the check after each batch of fixes):
  - `perform_state_read`/`perform_state_write`: `let rel: SandboxRelPath = ...; let full = ctx.state_dir().join(&rel)` — `join(rel)` errors become `join(&rel)` via `AsRef<Path>`.
  - `perform_file_read`: replace the `resolve → allows(&str) → read` sequence with `let resolved = resolve_for_allowlist(..)?; match ctx.allowed_paths.check_path(resolved) { Ok(allowed) => std::fs::read_to_string(&allowed), Err(denied) => { ctx.observe_denial(DenialKind::Path, denied.as_str()); /* existing ok:false return, same error string */ } }`.
  - `perform_file_write`: same shape against `allowed_write_paths`, `std::fs::write(&allowed, ...)`.
  - Denial messages, `observe_denial` targets, and every returned `error` string stay byte-identical — thread `.as_str()`/`format!` exactly where a `String` was used before.
  - Existing tests referencing the old `String`/`PathBuf` returns: update the minimal expression (add `.as_str()` / compare via `AsRef<Path>`); do not restructure any test.

- [ ] **Step 6: Run the crate suite:** `cargo test -p rustline-wasm`. Expected: PASS, including every existing denied-case test unchanged in meaning.

- [ ] **Step 7: Workspace gates:** `cargo check --workspace --all-targets --all-features && cargo clippy --all-targets --all-features -- -D warnings && cargo fmt --all`. Expected: green.

- [ ] **Step 8: Strip T2's block from `TYPECHECK.md`** (the `### T2. …` heading through its `- [x] execute` line).

- [ ] **Step 9: Commit:**

```bash
git add -A
git commit -m "typecheck(newtype): seal sandbox path resolution behind ResolvedPath/SandboxRelPath/AllowedPath [T2]

Claude-Session: https://claude.ai/code/session_01VAaLGMwx9csJnEmzHMJeWq"
```

---

### Task 2: T3 — phantom-tagged `AllowSet<K>` + `CanonicalArgv`

**Files:**
- Modify: `crates/rustline-wasm/src/allow.rs` (generic `AllowSet<K>`, `CapKind` trait, four markers)
- Modify: `crates/rustline-wasm/src/capability.rs` (typed fields, `from_config`, `observe_denial` call sites)
- Modify: `crates/rustline-wasm/src/argv.rs` (`canonical_argv -> CanonicalArgv`)
- Modify: `crates/rustline-wasm/src/perform.rs` (gate sites)
- Test: in-file additions in `allow.rs`/`argv.rs`

**Interfaces:**
- Consumes: Task 1's `ResolvedPath`, `AllowedPath`, `AllowSet::check_path`.
- Produces: `allow::{CapKind, UrlCap, ReadPathCap, WritePathCap, CommandCap, AllowSet<K>}` with `AllowSet::<K>::compile(&[String]) -> AllowSet<K>` and `allows(&self, subject: &K::Subject<'_>) -> bool`; `allow::Url<'a>(pub &'a str)` wait — field private, constructor `Url::new(&str)`; `argv::CanonicalArgv` (`of(program, args)`, `as_str`, `Display`). Task 3 uses these gates unchanged.

- [ ] **Step 1: Write the failing test for the typed gate** in `allow.rs`:

```rust
#[test]
fn typed_allowsets_carry_their_denial_kind() {
    use crate::capability::DenialKind;
    assert!(matches!(UrlCap::DENIAL, DenialKind::Url));
    assert!(matches!(ReadPathCap::DENIAL, DenialKind::Path));
    assert!(matches!(WritePathCap::DENIAL, DenialKind::Path));
    assert!(matches!(CommandCap::DENIAL, DenialKind::Command));
    let urls: AllowSet<UrlCap> = AllowSet::compile(&["https://wttr.in/*".into()]);
    assert!(urls.allows(&Url::new("https://wttr.in/48183")));
}
```

- [ ] **Step 2: Run — expect FAIL** (`UrlCap` etc. undefined): `cargo test -p rustline-wasm typed_allowsets -- --nocapture`.

- [ ] **Step 3: Implement the trait + markers + generic set in `allow.rs`:**

```rust
use std::marker::PhantomData;
use crate::capability::DenialKind;
use crate::state::{AllowedPath, ResolvedPath};

/// One capability family. `DENIAL` is the record kind every deny site for
/// this family reports; `Subject` is the only value type the gate accepts,
/// so a URL can never be checked against a path allowlist (or vice versa).
pub trait CapKind {
    const DENIAL: DenialKind;
    type Subject<'a>;
    fn as_match_str<'a>(s: &'a Self::Subject<'_>) -> &'a str;
}

/// A URL about to be fetched. Thin borrow wrapper — the subject type for
/// `AllowSet<UrlCap>`.
pub struct Url<'a>(&'a str);
impl<'a> Url<'a> {
    pub fn new(s: &'a str) -> Self { Self(s) }
    pub fn as_str(&self) -> &'a str { self.0 }
}

pub struct UrlCap;
impl CapKind for UrlCap {
    const DENIAL: DenialKind = DenialKind::Url;
    type Subject<'a> = Url<'a>;
    fn as_match_str<'a>(s: &'a Url<'_>) -> &'a str { s.0 }
}

pub struct ReadPathCap;
impl CapKind for ReadPathCap {
    const DENIAL: DenialKind = DenialKind::Path;
    type Subject<'a> = ResolvedPath;
    fn as_match_str<'a>(s: &'a ResolvedPath) -> &'a str { s.as_str() }
}

pub struct WritePathCap;
impl CapKind for WritePathCap {
    const DENIAL: DenialKind = DenialKind::Path;
    type Subject<'a> = ResolvedPath;
    fn as_match_str<'a>(s: &'a ResolvedPath) -> &'a str { s.as_str() }
}

pub struct CommandCap;
impl CapKind for CommandCap {
    const DENIAL: DenialKind = DenialKind::Command;
    type Subject<'a> = crate::argv::CanonicalArgv;
    fn as_match_str<'a>(s: &'a crate::argv::CanonicalArgv) -> &'a str { s.as_str() }
}

pub struct AllowSet<K: CapKind>(Vec<Pattern>, PhantomData<K>);

impl<K: CapKind> AllowSet<K> {
    pub fn compile(entries: &[String]) -> AllowSet<K> {
        /* existing body, then */ AllowSet(patterns, PhantomData)
    }
    pub fn allows(&self, subject: &K::Subject<'_>) -> bool {
        let s = K::as_match_str(subject);
        self.0.iter().any(|p| p.is_match(s))
    }
}

/// Path-family sets additionally mint the AllowedPath token (Task 1).
pub trait PathCap: CapKind {}
impl PathCap for ReadPathCap {}
impl PathCap for WritePathCap {}
impl<K: PathCap> AllowSet<K> {
    pub fn check_path(&self, path: ResolvedPath) -> Result<AllowedPath, ResolvedPath> {
        if self.0.iter().any(|p| p.is_match(path.as_str())) {
            Ok(AllowedPath::from_checked(path))
        } else {
            Err(path)
        }
    }
}
```

(The Task-1 untagged `check_path` moves here; delete the untagged version.)

- [ ] **Step 4: `CanonicalArgv` in `argv.rs`** — keep the fn name, change the return type (signature change, not a rename):

```rust
/// The canonical rendering of one argv. Holding one is proof it came from
/// `canonical_argv` — the gate and the exec cache key both require it, so
/// checking the bare program name (bypassing the whole-argv gate) no longer
/// compiles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalArgv(String);
impl CanonicalArgv {
    pub fn as_str(&self) -> &str { &self.0 }
}
impl std::fmt::Display for CanonicalArgv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.0.fmt(f) }
}

pub fn canonical_argv(program: &str, args: &[String]) -> CanonicalArgv {
    /* existing body */ CanonicalArgv(out)
}
```

- [ ] **Step 5: Migrate, compiler-driven.** `cargo check -p rustline-wasm 2>&1 | head -80`; fix by rule until green:
  - `CapabilityCtx` fields: `allowed_urls: AllowSet<UrlCap>`, `allowed_paths: AllowSet<ReadPathCap>`, `allowed_write_paths: AllowSet<WritePathCap>`, `allowed_commands: AllowSet<CommandCap>`; `from_config` gains the four turbofishes (`AllowSet::<UrlCap>::compile(&pc.allowed_urls)` …).
  - Gate sites: `ctx.allowed_urls.allows(url)` → `ctx.allowed_urls.allows(&Url::new(url))`; exec gates pass `&candidate` (now `CanonicalArgv`); path gates already use `check_path` from Task 1.
  - Deny sites: replace the hand-paired kind with the marker's const — `ctx.observe_denial(UrlCap::DENIAL, url)` etc. (`observe_denial`'s own signature is unchanged).
  - Exec cache key: `cache_path(&dir, EXEC_NAMESPACE, candidate.as_str())` — the namespace/key typing itself is unselected finding T22; do NOT gold-plate it here.
  - Test call sites constructing `AllowSet` update to a turbofish; nothing else in tests restructures.
  - KNOWN LIMIT (state in the commit body): `AllowSet::<UrlCap>::compile(&pc.allowed_paths)` still compiles — typed *pattern lists* are unselected finding T9. What this task delivers is typed fields + typed gate subjects.

- [ ] **Step 6: Run:** `cargo test -p rustline-wasm`. Expected: PASS (all denied-case tests intact).

- [ ] **Step 7: Workspace gates** (same three commands as Task 1 Step 7). Expected: green.

- [ ] **Step 8: Strip T3's block from `TYPECHECK.md`.**

- [ ] **Step 9: Commit:** `typecheck(newtype): phantom-tag AllowSet per capability and type the exec argv gate [T3]` (with session trailer).

---

### Task 3: T4 — outcome enums behind the eight `perform_*` functions

**Files:**
- Modify: `crates/rustline-wasm/src/perform.rs` (five outcome enums + `From` impls + inner `_outcome` fns; `pub fn perform_*` signatures UNCHANGED)
- Test: new `#[cfg(test)] mod wire_pins` in `perform.rs` (written FIRST)

**Interfaces:**
- Consumes: Tasks 1–2's typed gates (already in place).
- Produces: nothing consumed downstream — the wire structs (`HttpResult` etc. in rustline-abi) and every `pub fn perform_*` signature stay exactly as-is. Purely internal single-authoring of the `ok`/`error`/`stale` flags.

- [ ] **Step 1: Write the wire-pinning characterization tests** (they must pass on TODAY's code — they pin the guest-visible JSON per outcome path so the refactor can't move it). Use the existing test fakes in `perform.rs`'s test module (fake `Fetcher`/`Runner`, tempdir state roots — mirror how the existing denied-case tests build a `CapabilityCtx`). One golden assertion per outcome path, e.g.:

```rust
#[test]
fn wire_pin_http_denied() {
    let ctx = ctx_with_no_grants(); // same helper style the existing denial tests use
    let r = perform_http_get(&ctx, "https://x.example/", &FailFetcher);
    assert_eq!(
        serde_json::to_string(&r).unwrap(),
        r#"{"ok":false,"status":0,"body":"","error":"url not allowed: https://x.example/"}"#
    );
}
```

Cover: http {denied, 2xx, transport-error}; cached-http {denied, fresh-hit, refresh, served-stale, never-succeeded}; exec {denied, ran-zero, ran-nonzero, could-not-run}; cached-exec {denied, fresh-hit, zero-exit-cached, nonzero-fresh, stale-fallback}; state-read {found, absent, failed, bad-relpath}; state-write {written, quota-refused, bad-relpath}; file-read {denied, found, failed}; file-write {denied, written, failed}. Where an existing test already exercises the path, only ADD the JSON assertion style here — never edit the existing test. If a golden string mismatches your expectation, the TEST is wrong: fix the test to record actual current output (that is what characterization means).

- [ ] **Step 2: Run them against unchanged code:** `cargo test -p rustline-wasm wire_pin`. Expected: PASS.

- [ ] **Step 3: Commit the pins alone:** `test: pin perform_* wire JSON before typecheck [T4]` (with session trailer).

- [ ] **Step 4: Introduce the outcome enums + `From` impls** (private to `perform.rs`):

```rust
enum HttpOutcome {
    Denied { url: String },
    Completed { status: u16, body: String },
    TransportFailed { error: String },
}
impl From<HttpOutcome> for HttpResult {
    fn from(o: HttpOutcome) -> HttpResult {
        match o {
            HttpOutcome::Denied { url } => HttpResult {
                ok: false,
                error: format!("url not allowed: {url}"),
                ..Default::default()
            },
            HttpOutcome::Completed { status, body } => HttpResult {
                ok: true, status, body, error: String::new(),
            },
            HttpOutcome::TransportFailed { error } => HttpResult {
                ok: false, error, ..Default::default()
            },
        }
    }
}
```

Same pattern for `CachedHttpOutcome { Denied { url }, Fresh { status, body, age_secs }, Backoff { status, body, age_secs }, Refreshed { status, body }, ServedStale { status, body, age_secs, error }, NoUsableAnswer { error } }`, `ExecOutcome { Denied { candidate }, Ran { status, stdout, stderr, truncated }, CouldNotRun { error } }`, `CachedExecOutcome` (adds `Fresh`/`Stale` data mirroring `CachedExecResult`'s `stale`/`age_secs`), `ReadOutcome { Denied { path }, Found { contents }, Absent, Failed { error } }`, `WriteOutcome { Denied { path }, Written, Failed { error } }`. Copy each flag combination VERBATIM from the return literal it replaces — the `From` impl is a transcription, not a redesign; the wire_pin tests are the referee. Preserve the existing doc'd divergence exactly (e.g. `Backoff`/`ServedStale` produce `ok: true, stale: true` with a non-empty `error`; a `Denied`/`bad-relpath` read produces `ok: false`; `Absent` produces `ok: true, exists: false`).

- [ ] **Step 5: Restructure each `pub fn perform_*` to an inner outcome fn**, e.g.:

```rust
pub fn perform_http_get(ctx: &CapabilityCtx, url: &str, fetcher: &dyn Fetcher) -> HttpResult {
    http_get_outcome(ctx, url, fetcher).into()
}

fn http_get_outcome(ctx: &CapabilityCtx, url: &str, fetcher: &dyn Fetcher) -> HttpOutcome {
    if !ctx.allowed_urls.allows(&Url::new(url)) {
        ctx.observe_denial(UrlCap::DENIAL, url);
        return HttpOutcome::Denied { url: url.to_string() };
    }
    match fetcher.get(url) {
        Ok((status, body)) => HttpOutcome::Completed { status, body },
        Err(error) => HttpOutcome::TransportFailed { error },
    }
}
```

Side effects (observe_denial, cache reads/writes, `invalidate_state_size`, tracing) stay inside the outcome fns at their current points in the control flow — only the RETURN VALUE construction moves into `From`.

- [ ] **Step 6: `cargo check -p rustline-wasm` until green, then run the full crate suite:** `cargo test -p rustline-wasm`. Expected: PASS — especially every `wire_pin` test, byte-identical.

- [ ] **Step 7: Workspace gates** (three commands). Expected: green.

- [ ] **Step 8: MILESTONE (end of wasm cluster):** `just test && just lint && just check-lock`. If the wasm toolchain is installed (`rustup target list --installed | grep wasm32-unknown-unknown`), also `just test-wasm`. On red: bisect Tasks 1–3, revert the offender, report.

- [ ] **Step 9: Strip T4's block from `TYPECHECK.md`.**

- [ ] **Step 10: Commit:** `typecheck(illegal-states): single-author perform_* wire flags via outcome enums [T4]` (with session trailer).

---

### Task 4: T1 — `RangeName`, the one definition of the clickable-name rule

**Files:**
- Create: `crates/rustline-core/src/range_name.rs`
- Modify: `crates/rustline-core/src/lib.rs` (module + re-exports; `RANGE_NAME_MAX_BYTES` moves here, root re-export path preserved)
- Modify: `crates/rustline-core/src/render.rs` (`RangeGroup.range: Option<RangeName>`, `render_region_ranged`)
- Modify: `crates/rustline-core/src/widget.rs` (`Widget::range_name(&self) -> Option<RangeName>`)
- Modify: `crates/rustline-core/src/widgets/toggle.rs` (`clickable_range -> Option<RangeName>`), the twelve clickable widget modules' `range_name` impls (compiler-listed), `crates/rustline-core/src/widgets/mod.rs` (instance-name warn)
- Modify: `crates/rustline-core/src/assemble.rs` (RangeGroup construction)
- Modify: `crates/rustline-wasm/src/host.rs` (`plugin_range_name`), `crates/rustline-wasm/src/lib.rs` (stem warn)
- Modify: `crates/rustline/src/plugin_cmd.rs` (`validate_plugin_name`), `crates/rustline/src/plugin_install.rs` (`validate_install_name`), `crates/rustline/src/plugin_index.rs` (entry-name validation)
- Test: `range_name.rs` in-file tests + a render-boundary characterization test in `render.rs`/`assemble.rs` tests

**Interfaces:**
- Consumes: nothing from Tasks 1–3.
- Produces: `rustline_core::{RangeName, NameError}` — `RangeName::parse(&str) -> Result<RangeName, NameError>` (sole constructor; non-empty, ≤ `RANGE_NAME_MAX_BYTES` bytes, `[A-Za-z0-9_-]` only, not `"window"`), `as_str()`, `Deref<Target = str>`, `Display`, `AsRef<str>`, `Clone/Debug/PartialEq/Eq`. `NameError { Empty, TooLong { len: usize }, BadChar { ch: char }, Reserved }` with `Display`. Task 6's `WidgetName::fits_tmux_range` delegates to this.

- [ ] **Step 1: Characterization + security tests FIRST** (risk: medium). In `range_name.rs`'s test module (new file may hold the tests before impl — write both test and skeleton together, test failing):

```rust
#[test]
fn parse_enforces_all_four_rules() {
    assert!(RangeName::parse("cpu").is_ok());
    assert!(RangeName::parse("clock_utc-2").is_ok());
    assert_eq!(RangeName::parse(""), Err(NameError::Empty));
    assert_eq!(RangeName::parse("sixteen_bytes_xx"), Err(NameError::TooLong { len: 16 }));
    assert_eq!(RangeName::parse("a#[norange]"), Err(NameError::BadChar { ch: '#' }));
    assert_eq!(RangeName::parse("window"), Err(NameError::Reserved));
}
```

And the load-bearing render-boundary test (in `assemble.rs`'s or `render.rs`'s test module — whichever already holds instance-render tests), asserting a charset-violating instance name emits NO range markup:

```rust
#[test]
fn charset_violating_instance_name_never_reaches_range_markup() {
    // Register an instance whose name would forge markup if interpolated.
    // After T1, range_name() refuses to produce a RangeName for it, so the
    // rendered output must contain no `#[range=user|` at all for it.
    let out = /* render a region containing only that instance, via the same
                 helper the existing instance render tests use */;
    assert!(!out.contains("#[range=user|a"));
}
```

- [ ] **Step 2: Run — expect FAIL** (types undefined): `cargo test -p rustline-core range_name -- --nocapture`.

- [ ] **Step 3: Implement `range_name.rs`:**

```rust
//! The one definition of invariant #7's clickable-name rule. A `RangeName`
//! is proof its bytes are safe to interpolate into `#[range=user|…]` markup
//! verbatim (invariant #8's range-name counterpart).

pub const RANGE_NAME_MAX_BYTES: usize = 15; // moved from render.rs; root re-export unchanged
const RESERVED: &str = "window";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RangeName(String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooLong { len: usize },
    BadChar { ch: char },
    Reserved,
}

impl RangeName {
    pub fn parse(s: &str) -> Result<RangeName, NameError> {
        if s.is_empty() { return Err(NameError::Empty); }
        if s.len() > RANGE_NAME_MAX_BYTES { return Err(NameError::TooLong { len: s.len() }); }
        if let Some(ch) = s.chars().find(|c| !c.is_ascii_alphanumeric() && *c != '_' && *c != '-') {
            return Err(NameError::BadChar { ch });
        }
        if s == RESERVED { return Err(NameError::Reserved); }
        Ok(RangeName(s.to_string()))
    }
    pub fn as_str(&self) -> &str { &self.0 }
}
// + Deref<Target=str>, Display, AsRef<str>, and Display for NameError
// (messages: "name must not be empty" / "name is {len} bytes (max {RANGE_NAME_MAX_BYTES})" /
//  "name may only contain letters, digits, `_`, and `-` (found {ch:?})" /
//  "name \"window\" is reserved").
```

Wire into `lib.rs`: `mod range_name; pub use range_name::{NameError, RangeName, RANGE_NAME_MAX_BYTES};` and delete the const from `render.rs` (fix its `use`).

- [ ] **Step 4: Migrate the render/registration boundary, compiler-driven.** `cargo check --workspace --all-targets --all-features 2>&1 | head -80`; rules:
  - `toggle.rs::clickable_range(name: &str, alt: &str) -> Option<RangeName>` = `if alt.is_empty() { None } else { RangeName::parse(name).ok() }` — this single change makes charset violations unclickable everywhere (the twelve widgets call it).
  - `Widget::range_name(&self) -> Option<RangeName>` (owned — a per-render ≤15-byte alloc for clickable widgets is negligible at 1 render/sec); the twelve impls and `WasmWidget`'s (`host.rs::plugin_range_name`) become `clickable_range(...)`-style parses; delete the hand-rolled length comparisons.
  - `RangeGroup { range: Option<RangeName> }`; `render_region_ranged` interpolates `range.as_str()` — now safe by construction; `assemble.rs` threads the owned values through the palette regroup.
  - Registration stays PERMISSIVE (deliberate, N2): `Registry::with_builtins`' instance pass and `register_plugins`' stem check still register on a parse failure, but the existing too-long `warn_once` broadens to report the specific `NameError` ("not click-toggleable: {err}"). Do NOT start skipping registration — the forgery hole is closed at `range_name()`.
  - `validate_plugin_name` (plugin_cmd.rs): rebuild on `match RangeName::parse(name)` — every `Err` is a hard error, keeping each current message string verbatim per variant. `validate_install_name` (plugin_install.rs): `Err(NameError::TooLong { .. }) => Ok(false)` (warn-don't-refuse, current behavior), every other `Err` maps to its current error string, `Ok(_) => Ok(true)`.
  - `plugin_index.rs:335-358`'s hand-rolled four-rule re-assertion becomes `RangeName::parse(&e.name).is_ok()` (plus whatever non-name checks that block also does — leave those).

- [ ] **Step 5: Run the Step-1 tests + both crates' suites:** `cargo test -p rustline-core && cargo test -p rustline-wasm && cargo test -p rustline`. Expected: PASS, including the new no-forgery test.

- [ ] **Step 6: Workspace gates** (three commands). Expected: green.

- [ ] **Step 7: Strip T1's block from `TYPECHECK.md`.**

- [ ] **Step 8: Commit:** `typecheck(parse-dont-validate): one RangeName parse for the clickable-name rule [T1]` (with session trailer).

---

### Task 5: T5 — `WidgetKind` enum + parse-once `resolved_instances()`

**Files:**
- Modify: `crates/rustline-core/src/config.rs` (the enum; `instance_kind`/`instance_meta`/`layout_kinds`/`disk_mounts`/`throughput_interfaces`/`spark_referenced_in_layout`/`instances_of_kind`; `resolved_instances` memo; delete `BUILTIN_WIDGET_NAMES`/`is_builtin_widget_name`)
- Modify: `crates/rustline-core/src/widget.rs` (`WidgetSource::Instance { kind: WidgetKind }`)
- Modify: `crates/rustline-core/src/widgets/mod.rs` (registration dispatch over `InstanceSpec`; `instance_opts`/`instance_descriptor` typed params)
- Modify: `crates/rustline/src/{build_context.rs, doctor.rs, widget_cmd.rs, widget_tui.rs, click.rs, theme_cmd.rs}` and any other bin site the compiler lists
- Test: in-file additions in `config.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `rustline_core::config::WidgetKind` (16 variants, `Copy`, `ALL: [WidgetKind; 16]`, `as_str() -> &'static str`, `parse(&str) -> Option<WidgetKind>`, `is_instanceable() -> bool`); `InstanceSpec` (12 variants wrapping the existing `<Kind>Opts` structs), `InstanceParse { Ok(InstanceSpec), NoKind, UnknownKind(String) }`, `Config::resolved_instances(&self) -> &BTreeMap<String, InstanceParse>`; `Config::layout_kinds(&self, layout) -> BTreeSet<WidgetKind>`. Task 6 leaves all of these alone (names become `WidgetName` there; kinds stay `WidgetKind`).

- [ ] **Step 1: Write the failing pinning tests** in `config.rs`'s test module:

```rust
#[test]
fn widget_kind_toml_spellings_are_the_accepted_ones() {
    // The two explicit renames are load-bearing: snake_case alone would emit
    // date_time / load_avg, which are NOT the accepted TOML spellings.
    for k in WidgetKind::ALL {
        assert_eq!(WidgetKind::parse(k.as_str()), Some(k));
    }
    assert_eq!(WidgetKind::parse("datetime"), Some(WidgetKind::DateTime));
    assert_eq!(WidgetKind::parse("loadavg"), Some(WidgetKind::LoadAvg));
    assert_eq!(WidgetKind::parse("date_time"), None);
    assert_eq!(WidgetKind::parse("load_avg"), None);
    assert_eq!(WidgetKind::ALL.len(), 16);
    assert_eq!(WidgetKind::ALL.iter().filter(|k| k.is_instanceable()).count(), 12);
}

#[test]
fn resolved_instances_parses_each_table_exactly_once_and_degrades_per_entry() {
    let cfg: Config = toml::from_str(r#"
        [instances.clock_utc]
        kind = "datetime"
        timezone = "UTC"
        [instances.mystery]
        kind = "not_a_kind"
        [instances.kindless]
        format = "x"
    "#).unwrap();
    let r = cfg.resolved_instances();
    assert!(matches!(r.get("clock_utc"), Some(InstanceParse::Ok(InstanceSpec::DateTime(_)))));
    assert!(matches!(r.get("mystery"), Some(InstanceParse::UnknownKind(k)) if k == "not_a_kind"));
    assert!(matches!(r.get("kindless"), Some(InstanceParse::NoKind)));
}
```

- [ ] **Step 2: Run — expect FAIL** (types undefined): `cargo test -p rustline-core widget_kind_toml -- --nocapture`.

- [ ] **Step 3: Implement layer 1 — the enum** (in `config.rs`, near `BUILTIN_WIDGET_NAMES`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetKind {
    PaneId,
    Hostname,
    Windows,
    #[serde(rename = "datetime")]
    DateTime,
    Cwd,
    LanIp,
    TailscaleIp,
    Battery,
    Cpu,
    Memory,
    #[serde(rename = "loadavg")]
    LoadAvg,
    Git,
    Disk,
    Uptime,
    Media,
    Throughput,
}

impl WidgetKind {
    pub const ALL: [WidgetKind; 16] = [ /* registration order, same as BUILTIN_WIDGET_NAMES today */ ];
    pub fn as_str(self) -> &'static str { match self { /* the 16 exact current strings */ } }
    pub fn parse(s: &str) -> Option<WidgetKind> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
    /// The twelve clickable/format-bearing kinds an `[instances.<name>]`
    /// table may declare; `cwd`/`hostname`/`pane_id`/`windows` are not.
    pub fn is_instanceable(self) -> bool {
        !matches!(self, WidgetKind::PaneId | WidgetKind::Hostname | WidgetKind::Windows | WidgetKind::Cwd)
    }
}
```

Copy `ALL`'s order and the 16 strings verbatim from today's `BUILTIN_WIDGET_NAMES`, then DELETE that const and `is_builtin_widget_name` (replace uses with `WidgetKind::parse(name).is_some()`), and delete the now-retired sync characterization test at config.rs:3091 (the enum makes it structurally impossible to drift — this deletion is the one sanctioned test removal, called out in the finding).

- [ ] **Step 4: Implement layer 2 — parse-once.** In `config.rs`:

```rust
#[derive(Clone, Debug)]
pub enum InstanceSpec {
    DateTime(DateTimeOpts), LanIp(LanIpOpts), TailscaleIp(TailscaleIpOpts),
    Battery(BatteryOpts), Cpu(CpuOpts), Memory(MemoryOpts), LoadAvg(LoadavgOpts),
    Git(GitOpts), Disk(DiskOpts), Uptime(UptimeOpts), Media(MediaOpts),
    Throughput(ThroughputOpts),
}
// (Use the EXACT existing Opts struct names — check them; e.g. if the loadavg
//  options struct is `LoadavgOpts` not `LoadAvgOpts`, follow the code.)

#[derive(Clone, Debug)]
pub enum InstanceParse {
    Ok(InstanceSpec),
    NoKind,
    UnknownKind(String),
}
```

Field on `Config`: `#[serde(skip)] resolved: std::sync::OnceLock<BTreeMap<String, InstanceParse>>` (Config derives `Clone, Debug, Default, Serialize, Deserialize` — `OnceLock<T: Clone>` satisfies all of those; a cloned Config re-resolves lazily, which is fine). `resolved_instances()` populates it with ONE pass: for each `(name, table)`, read `kind` via `Config::instance_kind` (now returning `Option<WidgetKind>` — `None` on a missing/non-string `kind` key → `NoKind`; a string that fails `WidgetKind::parse` → `UnknownKind(s)`), then one 12-arm `match kind` calling the existing `instance_opts::<XOpts>` per instanceable kind (non-instanceable kinds → `UnknownKind` is WRONG — they must keep today's behavior: warn-and-skip as "not instanceable"; model them as `UnknownKind` ONLY if today's code paths treat them identically — check `Registry::with_builtins`' current arms and mirror its exact warn keys/messages; if today distinguishes "not instanceable" from "unknown kind", add an `InstanceParse::NotInstanceable(WidgetKind)` variant so no message changes).
Then point `color_overrides`/`click_map`/`layout_kinds`/`disk_mounts`/`throughput_interfaces`/`spark_referenced_in_layout`/`instances_of_kind` AND `Registry::with_builtins`' registration pass at `resolved_instances()`, deleting `instance_meta`'s duplicate 12-arm match and the per-consumer `try_into` re-parses. Registration keeps its own concerns (name collision with a built-in, Task 4's RangeName warn) but takes the parsed `InstanceSpec` instead of re-dispatching on a string.

- [ ] **Step 5: Implement layer 3 — typed params, compiler-driven.** `cargo check --workspace --all-targets --all-features 2>&1 | head -100`; rules:
  - `layout_kinds -> BTreeSet<WidgetKind>` (a built-in name maps to its parsed kind; an instance to its spec's kind; an unknown/plugin name maps to NOTHING — note this is a semantic tightening from today's "maps to itself harmlessly": verify `build_context.rs`/`doctor.rs` only ever probe built-in kind names, which they do — the gates become `kinds.contains(&WidgetKind::Git)` etc.).
  - `WidgetSource::Instance { kind: WidgetKind }` (not Serialize — no wire concern); `widget_cmd.rs`/`widget_tui.rs` display sites keep their exact output strings via `kind.as_str()`: `format!("instance of {}", …)`, `"instance:{}"`, `"{} instance"`.
  - `instance_descriptor`/`instances_of_kind`/`spark_referenced_in_layout` take `WidgetKind` where they took `&str` kinds.
  - `doctor.rs`'s reader-kind table branches on `WidgetKind` variants — exhaustive `match` where it listed strings.

- [ ] **Step 6: Run everything:** `cargo test -p rustline-core && cargo test -p rustline`. Expected: PASS, including Step-1 pins and the untouched instance-behavior tests (warn-and-skip semantics unchanged).

- [ ] **Step 7: Workspace gates** (three commands). Expected: green.

- [ ] **Step 8: Strip T5's block from `TYPECHECK.md`.**

- [ ] **Step 9: Commit:** `typecheck(stringly-enum): WidgetKind enum + parse-once instance specs [T5]` (with session trailer).

---

### Task 6: T6 — `WidgetName`, the identity newtype

**Files:**
- Modify: `crates/rustline-abi/src/lib.rs` (the newtype; `WireContext.toggled: BTreeSet<WidgetName>`)
- Modify: `crates/rustline-core/src/{context.rs, widget.rs, config.rs, render.rs, assemble.rs, widgets/toggle.rs}` + the twelve clickable widget modules (compiler-listed)
- Modify: `crates/rustline-wasm/src/host.rs`
- Modify: `crates/rustline/src/{toggles.rs, click.rs, widget_cmd.rs, widget_tui.rs, build_context.rs}` + whatever else the compiler lists
- Test: in-file additions in `rustline-abi/src/lib.rs` and `config.rs` (written FIRST)

**Interfaces:**
- Consumes: Task 4's `RangeName` (for `fits_tmux_range`), Task 5's typed kinds (unchanged here).
- Produces: `rustline_abi::WidgetName` — `#[serde(transparent)]`, `Clone/Debug/PartialEq/Eq/PartialOrd/Ord/Hash/Serialize/Deserialize`, `WidgetName::new(impl Into<String>)`, `as_str()`, `From<&str>`, `From<String>`, `Display`, `AsRef<str>`, **`impl Borrow<str>`** (load-bearing: it lets `BTreeSet<WidgetName>::contains("cpu")` and `HashMap<WidgetName, _>::get(name_str)` work without allocating, and is what keeps most lookup sites one-line fixes).

- [ ] **Step 1: Write the failing wire/TOML pinning tests.** In `rustline-abi`:

```rust
#[test]
fn widget_name_is_wire_transparent() {
    let n = WidgetName::from("cpu");
    assert_eq!(serde_json::to_string(&n).unwrap(), r#""cpu""#);
    let set: std::collections::BTreeSet<WidgetName> =
        serde_json::from_str(r#"["cpu","weather"]"#).unwrap();
    assert!(set.contains("cpu")); // Borrow<str> lookup, no alloc
}
```

In `config.rs`:

```rust
#[test]
fn layout_and_map_keys_keep_their_toml_shape_with_widget_name() {
    let cfg: Config = toml::from_str(r#"
        [layout]
        left = ["pane_id", "hostname"]
        [plugins.weather]
        allowed_urls = ["https://wttr.in/*"]
        [instances.clock_utc]
        kind = "datetime"
    "#).unwrap();
    assert_eq!(cfg.layout.left, vec![WidgetName::from("pane_id"), WidgetName::from("hostname")]);
    assert!(cfg.plugins.contains_key("weather"));
    let back = toml::to_string(&cfg).unwrap();
    assert!(back.contains(r#"left = ["pane_id", "hostname"]"#));
}
```

- [ ] **Step 2: Run — expect FAIL** (type undefined): `cargo test -p rustline-abi widget_name_is_wire -- --nocapture`.

- [ ] **Step 3: Implement `WidgetName` in `rustline-abi`:**

```rust
/// The widget identity that invariant #7 requires to be ONE string end-to-end:
/// registry key = layout entry = tmux range name = toggle key = click/override
/// map key. `#[serde(transparent)]` keeps every wire and TOML shape it appears
/// in byte-identical to a plain string.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WidgetName(String);

impl WidgetName {
    pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}
impl From<&str> for WidgetName { fn from(s: &str) -> Self { Self(s.to_string()) } }
impl From<String> for WidgetName { fn from(s: String) -> Self { Self(s) } }
impl std::borrow::Borrow<str> for WidgetName { fn borrow(&self) -> &str { &self.0 } }
impl AsRef<str> for WidgetName { fn as_ref(&self) -> &str { &self.0 } }
impl std::fmt::Display for WidgetName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.0.fmt(f) }
}
impl PartialEq<str> for WidgetName { fn eq(&self, other: &str) -> bool { self.0 == other } }
impl PartialEq<&str> for WidgetName { fn eq(&self, other: &&str) -> bool { self.0 == *other } }
```

In `rustline-core`, `fits_tmux_range` lives as an extension where `RangeName` is visible (core, not abi — abi cannot depend on core): add in `range_name.rs`:

```rust
impl RangeName {
    /// Parse a widget identity into its clickable range form, if it fits.
    pub fn of_widget(name: &rustline_abi::WidgetName) -> Option<RangeName> {
        RangeName::parse(name.as_str()).ok()
    }
}
```

- [ ] **Step 4: Migrate at the sources, compiler-driven.** Change these five source declarations, then let `cargo check --workspace --all-targets --all-features` enumerate everything else:
  - `Context.toggled: BTreeSet<WidgetName>` (context.rs) and `WireContext.toggled: BTreeSet<WidgetName>` (abi).
  - `Layout { left, center, right: Vec<WidgetName> }` and `Config.plugins: HashMap<WidgetName, PluginConfig>`, `Config.instances: HashMap<WidgetName, toml::Value>` (config.rs).
  - `Registry` key type + `register/register_described/build/contains/resolve` signatures (widget.rs) — `resolve` returns `Vec<(WidgetName, Box<dyn Widget>)>`.
  - `toggle::active_format(ctx, name: &WidgetName, format: &str, alt: &str)` and `clickable_range(name: &WidgetName, alt: &str) -> Option<RangeName>` (the concrete swap this uncompiles: identity vs. the two format-template strings).
  - `toggles::{parse_toggles, apply_toggle, read_toggles, write_toggles}` over `BTreeSet<WidgetName>`; `click::{resolve_click, dispatch}` + `ClickExecutor::toggle(&WidgetName)`.
  Fix rules for the fallout: lookup sites use the `Borrow<str>` bridge (`set.contains(s)` / `map.get(s)` where `s: &str` still compiles); construction sites use `WidgetName::from(...)`; display sites use `{name}` unchanged (`Display`); the twelve widget structs keep `name: String` internally OR switch to `WidgetName` — switch them (`name: WidgetName`), since `active_format` now wants `&WidgetName` and the compiler will walk you through the twelve `build_<kind>` helpers. `WasmWidget.name` likewise. Serde derives on `Context`/`WireContext` need no attention beyond the field type (transparent inner). Where tests construct sets/maps with `String`, `WidgetName::from` at the literal is the whole fix.

- [ ] **Step 5: Run everything:** `cargo test --workspace --all-features` (hermetic — wasm-e2e tests skip without the artifact). Expected: PASS, including Step-1 pins.

- [ ] **Step 6: Workspace gates** (three commands). Expected: green.

- [ ] **Step 7: MILESTONE (end of plan):** `just test && just lint && just check-lock`, plus `just test-wasm` and `just lint-plugins && just test-plugins` if the wasm target is installed (the SDK re-exports and guest examples must still compile — `WidgetName` is additive for guests, but prove it). On red: bisect Tasks 4–6, revert the offender, report.

- [ ] **Step 8: Strip T6's block from `TYPECHECK.md`** — with T1–T5 already stripped, the Critical section becomes `_(none)_`; leave every other bucket and the Skip section untouched.

- [ ] **Step 9: Commit:** `typecheck(newtype): WidgetName identity newtype end-to-end [T6]` (with session trailer).

---

## Self-review notes (already applied)

- Spec coverage: T2→Task 1, T3→Task 2, T4→Task 3, T1→Task 4, T5→Task 5, T6→Task 6; both spec milestones present (Task 3 Step 8, Task 6 Step 7); strip-on-fix in every task; rename guard in Global Constraints.
- Known deviations from the finding text, made deliberately and stated in-task: `PathResolveError` payloads stay `String` (Task 1 Step 3); registration stays permissive under `RangeName` (Task 4 Step 4); exec cache key passes `candidate.as_str()` (Task 2 Step 5, unselected T22); typed pattern lists out of scope (unselected T9).
- Type consistency: `RangeName`/`NameError` (Task 4) are what Task 6's `of_widget` consumes; `ResolvedPath`/`AllowedPath` (Task 1) are what Task 2's `PathCap` impls consume; `WidgetKind` (Task 5) is untouched by Task 6.
- Exact-name caveats the implementer must verify against the code (the compiler enforces): the loadavg/datetime Opts struct spellings (Task 5 Step 4 note), the current warn-once keys/messages for non-instanceable vs unknown kinds (Task 5 Step 4), and the existing test-helper names for `CapabilityCtx` construction (Task 3 Step 1).
