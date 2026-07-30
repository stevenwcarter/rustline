# code-health batch 2 — caching, logging, and the write capability

Date: 2026-07-26
Source: `bughunt.md` (triage of 2026-07-26), items checked `[x] execute`.
Branch: `codehealth/2026-07-26-batch2` off `main` @ `b682120`.

This spec covers **only** the selected findings. Unchecked findings stay in
`bughunt.md` for a future pass.

## Selected items

| id  | title | category | impact |
|-----|-------|----------|--------|
| D1  | Daemon pins the log fd; rotation evaluated once at process start | observability | decision-needed → approved |
| D2  | ~22 WARN sites fire once per render; one typo evicts the whole log | observability | decision-needed → approved |
| D3  | `allowed_paths` grants write as well as read | security | decision-needed → approved |
| B4  | Failed cache refresh is never negative-cached | caching | 16 |
| B12 | Instance table that fails to parse silently falls back to defaults | observability | 12 |
| B13 | HTTP/exec TTL cache has no eviction | caching | 12 |
| B15 | Cache/state writes use plain `fs::write`, no temp file | caching | 9 |
| B16 | Capability denials never reach the log file | observability | 9 |
| B17 | Atomic-write helpers share one fixed `.tmp` filename | correctness | 9 |
| B18 | `rl_log` unbounded in message size and rate | observability | 9 |
| B19 | Quota accounting walks the whole state dir on every write | caching | 9 |
| B27 | Daemon fallback is completely silent | observability | 6 |

The three `decision-needed` items carry explicit user decisions recorded in
`bughunt.md`; those decisions are transcribed verbatim in each section below and
are the authority for the design here.

## Invariants this feature depends on

Changes below cross these existing invariants (CLAUDE.md). Any task that would
weaken one must stop and surface it rather than proceed.

- **N1 — gate first.** A denied capability request makes no network call, spawns
  no process, and touches no cache file. D3 adds a *second* gate (write paths)
  and a symlink gate; both must run *before* any filesystem effect. B18's rate
  limit must not turn `rl_log` into a capability-checked function — it stays
  capability-free, only bounded.
- **N2 — a plugin must never break the bar.** Every new failure path (eviction
  error, canonicalization failure, symlink denial, log-rate cutoff) degrades to
  a denied/empty result, never a panic or an abort.
- **N3 — state quota.** `check_cap` must still refuse a write that would exceed
  `max_state_bytes`. B19 memoizes the *measurement*, not the *decision*: the
  check stays strictly before the write and stays conservative.
- **N4 — per-plugin scoping.** A `CapabilityCtx` only ever sees its own grants,
  its own state dir, and its own denials. New per-ctx state (B18's counter,
  B19's memo, D3's write allowlist) is per-instance, never shared.
- **#3 — config load is total.** `Config::load` accepts any syntactically valid
  TOML. D3's new keys must be `#[serde(default)]` so an old config still loads;
  B12 must warn rather than fail.
- **#7 — the click identity chain.** Untouched by this batch, but B12's warn
  must not change which instances register.

## D1 — replace hand-rolled log rotation with `tracing-appender`

**User decision (verbatim):** *"use `RollingFileAppender` instead"*

### Problem

`open_log` (`crates/rustline/src/logging.rs:97`) evaluates rotation exactly once,
at process start, and `FileWriter` then holds that `Arc<File>` for the process
lifetime. Short-lived `rustline render …` processes re-check every tick, but
`rustline daemon run` never exits: after the second rotation the daemon is
appending to an unlinked inode, so every daemon diagnostic is invisible and the
5 MiB cap is never enforced against the daemon's own writer.

### Design

Add `tracing-appender = "0.2.5"` to `crates/rustline/Cargo.toml` and replace
`open_log` + `FileWriter` with a `RollingFileAppender`:

```rust
tracing_appender::rolling::Builder::new()
    .rotation(Rotation::DAILY)
    .filename_prefix(prefix)     // "rustline"
    .filename_suffix(suffix)     // "log"
    .max_log_files(7)
    .build(dir)
```

- Use the **blocking** appender as the `MakeWriter` directly
  (`.with_writer(appender)`). Do **not** use `tracing_appender::non_blocking`:
  it returns a `WorkerGuard` that must outlive the subscriber, and `init()`
  returns `()` into short-lived render processes that would drop the guard and
  lose their logs.
- `RollingFileAppender` re-evaluates the rotation target on every write, which
  is exactly the property the daemon needs.

### Accepted behaviour changes (user-approved)

1. **Rotation is time-based, not size-based.** `MAX_LOG_BYTES` and
   `should_rotate` are deleted along with their tests. Retention becomes
   `max_log_files(7)` — seven daily generations instead of one `.1`.
2. **The filename gains a date component**: `rustline.2026-07-26.log`
   (`tracing-appender` emits `{prefix}.{date}.{suffix}`). There is no longer a
   single stable `rustline.log`. The date is **UTC**, matching the appender's
   own rotation boundary.

### Config and reporting surface

`[log].file` is currently a full file *path*. Keep the key and reinterpret it:
its parent directory becomes the appender's directory, its file stem the
`filename_prefix`, and its extension the `filename_suffix`. A `[log].file` of
`~/logs/rl.txt` therefore yields `~/logs/rl.<date>.txt`. Default stays
`$XDG_DATA_HOME/rustline/rustline.<date>.log`.

`logging::log_path` is `pub(crate)` and consumed by `doctor.rs`. Replace it with
a function that returns the *directory* plus the current day's filename so
`doctor` reports something that exists, and update `doctor`'s label to name the
pattern (`rustline.<date>.log`, 7 kept) rather than a single file.

### Tests

- The appender writes into the configured directory with the expected
  prefix/suffix.
- `[log].file` decomposition: a configured path yields the right dir, prefix,
  and suffix (including the no-extension and no-parent edge cases).
- `doctor`'s reported path resolves under the same directory.

Deleting `open_log`'s two rotation tests is expected — they pin behaviour this
item intentionally removes. Do not delete any other test.

### Note for the final report

This eliminates the stat-then-rename sequence entirely, which is the mechanism
behind unchecked finding **B20** (rotation race clobbering the single retained
generation). Flag B20 as moot at the end; do not silently strip it.

## D2 — dedup the per-render WARN sites across processes

**User decision (verbatim):** *"I am fine with the new state file and agree with
your proposed solution"*

### Problem

Roughly 22 `warn!` sites re-fire on every render because each render is a fresh
process re-running `Config::load` → `resolve_theme` → `Registry::with_builtins`
→ `register_plugins` → `Registry::resolve` from cold. At `status-interval 1`, one
typo'd layout entry emits ~86,400 identical lines a day, rotating the log and
evicting every other diagnostic. A per-process `OnceLock` cannot help — the
process is new each tick.

### Design

New module `crates/rustline/src/warn_once.rs` (binary crate — `rustline-core`
and `rustline-wasm` reach it through an injected hook, see below).

- Marker directory `<state_root>/warned/`.
- `pub fn warn_once(key: &str, emit: impl FnOnce())` — hash `key` (reuse the
  FNV-1a shape already in `cache.rs`, or `DefaultHasher`; it is a dedup key, not
  a security boundary), and create `<state_root>/warned/<hash>` with
  `OpenOptions::new().create_new(true)`. `Ok` → first sighting → call `emit()`.
  `AlreadyExists` → suppress. **Any other IO error → call `emit()`** (fail open:
  a broken marker dir must never silence diagnostics).
- **Generation reset:** the whole `warned/` directory is cleared whenever the
  config file's mtime changes. Store the observed mtime in
  `<state_root>/warned/.generation`; on mismatch, remove the directory contents
  and rewrite `.generation`. This is the "once per config edit" semantics the
  finding specifies.
- `create_new` is atomic on all supported filesystems, so concurrent renderers
  racing the same key produce exactly one winner — no lock needed.

### Reaching the sites in `rustline-core` / `rustline-wasm`

The warn sites live in three crates but the state root lives in the binary.
Rather than adding a state-root dependency to `rustline-core`, install a process
-wide hook: `rustline_core::diag::set_warn_once_hook(fn(&str, &dyn Fn()))`
(a `OnceLock<Box<dyn Fn(&str, &dyn Fn()) + Send + Sync>>`) which `main` sets
immediately after `logging::init`. When unset (tests, `rustline-core` used
standalone) the default is "always emit", so no diagnostic is ever lost by
omission.

### Sites to convert

Convert the ~22 recurring, config-derived sites and no others:

- `crates/rustline/src/main.rs` — unknown theme base (`:150`), invalid config
  (`:183`)
- `crates/rustline-core/src/widget.rs:144` — unknown widget, skipping
- `crates/rustline-core/src/widgets/mod.rs` — the four instance warns
  (`:382`, `:386`, `:393`, `:495`)
- `crates/rustline-core/src/widgets/datetime.rs:34` — unknown timezone
- `crates/rustline-wasm/src/lib.rs:112–192` — the ten plugin-skip warns
- `crates/rustline-wasm/src/allow.rs:56`

The dedup key must include the site *and* the message payload (e.g.
`"unknown-widget:memroy"`), so two different typos are two different warns.

**Do not convert** per-render *runtime* failures (plugin render failed, cache
write failed, denial warns) — those describe changing conditions, not static
misconfiguration.

### Tests

- First call emits, second suppresses, and a changed config mtime re-arms.
- An unwritable marker dir still emits (fail-open).
- Two distinct keys both emit.

## B12 — warn when an instance options table fails to parse

Depends on **D2** (this warn fires once per render on a persistent
misconfiguration; it must be deduped or it becomes the very problem D2 fixes).
Sequence B12 after D2.

### Problem

All twelve instance arms in `Registry::with_builtins`
(`crates/rustline-core/src/widgets/mod.rs:399`) use
`t.try_into().unwrap_or_default()`, as do the twelve arms in
`Config::instance_meta` and the `disk_mounts` / `throughput_interfaces` /
`spark_referenced_in_layout` helpers — ~28 sites, none of which log. A single
type error (`spark_width = "8"` quoted) discards the user's entire instance
config silently, and because the name still registers there is no "unknown
widget" warn either.

### Design

Add a shared helper next to the arms:

```rust
fn instance_opts<T: Default + serde::de::DeserializeOwned>(
    name: &str, kind: &str, v: toml::Value,
) -> T
```

On `Err(error)` it emits (through D2's `warn_once`)
`warn!(instance = %name, kind = %kind, %error, "invalid instance options, using defaults")`
and returns `T::default()`. Apply at all twelve `widgets/mod.rs` arms and the
twelve `instance_meta` arms.

The extra-`kind`-key case that CLAUDE.md documents as harmless must stay silent
— verify the helper does not warn for it (the `kind` key is consumed before the
`try_into`, or the arms already strip it; confirm against the current code and
add a test either way).

### Tests

- A type-error instance table warns once and falls back to defaults.
- An instance table carrying only the documented extra `kind` key does **not**
  warn.

## D3 — split the filesystem write capability out of `allowed_paths`

**User decision (verbatim):** *"treat existing key entries as read-only as the
migration, and I agree with your proposed solution. Additionally, require the
path provided to _not_ be a symlink unless a 'resolve symlinks' config value is
enabled."*

### Problem

`perform_file_read` (`perform.rs:374`) and `perform_file_write` (`perform.rs:413`)
gate on the *same* `ctx.allowed_paths`. `plugin approve` copies a plugin-supplied
manifest's `requested_paths` verbatim into `allowed_paths` and prints a danger
banner only for `requested_commands`. A plugin advertised as "reads your aliases"
can therefore obtain arbitrary file *overwrite* — e.g. `~/.bashrc` — from a grant
the user believed was read-only.

Separately, `normalize_abs` matches the allowlist against an *uncanonicalized*
path, so a symlink under a granted prefix redirects the effect outside the grant.

### Design — the grant split

- `PluginConfig` gains `#[serde(default)] pub allowed_write_paths: Vec<String>`.
  Empty = deny by default (N1). `allowed_paths` keeps its name and becomes
  **read-only**, which is the user-chosen migration: an existing config grants
  exactly what it granted before, minus write.
- `CapabilityCtx` gains `allowed_write_paths: AllowSet`, compiled in
  `from_config`.
- `perform_file_write` gates on `allowed_write_paths` only. `perform_file_read`
  gates on `allowed_paths` only. A write path is **not** implicitly readable —
  if a plugin needs both it must be granted both. State the reasoning in the
  doc comment.
- `PluginManifest` gains `#[serde(default)] pub requested_write_paths: Vec<String>`.
- `write_grants` (`plugin_cmd.rs`) writes `requested_write_paths` into
  `allowed_write_paths`.
- `manifest_report` lists `allowed_write_paths` as its own line and, when
  non-empty, prints a danger banner in the same shape as the existing
  `allowed_commands` one:

  > `! allowed_write_paths lets this plugin overwrite these files with any`
  > `! content. Approve only paths you understand.`

  It must also state, on the `allowed_paths` line, that the grant is read-only —
  the stopgap from the finding, now accurate rather than a warning about a gap.
- `Kind` gains `WritePath` (key `allowed_write_paths`), and the CLI gains
  `rustline plugin write-path <add|rm|list>` mirroring `plugin path`, so a
  user can manage the new list without hand-editing TOML. `plugin list` must
  show the new list alongside the others.
- `cli.rs:219`'s help for `plugin path` changes from "a plugin's filesystem-path
  allowlist" to "…read allowlist"; the new subcommand's help says write.

### Design — the symlink policy

- `PluginConfig` gains `#[serde(default)] pub resolve_symlinks: bool` (default
  `false`).
- New helper in `crates/rustline-wasm/src/state.rs`:

  ```rust
  pub fn resolve_for_allowlist(path: &str, resolve_symlinks: bool) -> Result<String, String>
  ```

  1. `normalize_abs` first (absolute, no literal `..`) — the cheap gate stays.
  2. `resolve_symlinks == false` (default): walk the path's ancestors and reject
     if any **existing** component is a symlink
     (`symlink_metadata()?.file_type().is_symlink()`), with the error
     `"symlink not allowed; set resolve_symlinks = true for this plugin"`.
     Return the literal normalized path. A component that does not exist is not
     a symlink and does not fail the check (a write target may legitimately not
     exist yet).
  3. `resolve_symlinks == true`: `std::fs::canonicalize` the path; if it does
     not exist, canonicalize the parent and re-join the file name. Return the
     canonical string. **Deny when canonicalization fails.**
  4. The allowlist is matched against whatever string this returns — so under
     `resolve_symlinks` the grant is over the resolved subtree, never the name.

- Both `perform_file_read` and `perform_file_write` call it, after
  `normalize_abs`'s job is folded in and before the allowlist check. Order is
  load-bearing: resolve → match → act. Denials keep using `DenialKind::Path` and
  still call `observe_denial`.

### Migration and documentation

- No config rewrite. An existing `allowed_paths` simply stops authorizing writes
  — this is the user's chosen fail-closed reading. A plugin that was writing
  through `allowed_paths` breaks loudly (denied + a denial record + B16's log
  line) rather than silently.
- CLAUDE.md's `[plugins.*]` reference and the N3/`allowed_paths` text must be
  updated: document `allowed_write_paths`, `resolve_symlinks`, that
  `allowed_paths` is read-only, and that the symlink gate closes the
  name-vs-subtree gap.
- README.md's plugin/config surface must be synced in the same pass.
- The five bundled example plugins (`weather`, `counter`, `filewatch`,
  `httpget`, `cmdrun`) and their manifests must be checked: any that writes a
  file through `rl_file_write` needs `requested_write_paths`. `filewatch` is
  the read-only case the finding names — confirm it stays read-only and keeps
  working.

### Producers that must survive the narrowed gate

Per the spec/test discipline: this narrows a shared funnel, so enumerate the
legitimate producers and give each a test.

1. `filewatch`-shaped read of a granted path — must still succeed.
2. A plugin granted `allowed_write_paths` — write must succeed.
3. A plugin granted only `allowed_paths` — write must be **denied**.
4. A plugin granted only `allowed_write_paths` — read must be **denied**.
5. A symlink inside a granted directory pointing outside it — denied for both
   read and write with `resolve_symlinks = false`.
6. The same symlink with `resolve_symlinks = true` — resolved, then matched
   against the allowlist, and denied because the *target* is outside the grant.
7. A symlink whose target is *inside* the grant with `resolve_symlinks = true`
   — allowed.
8. A not-yet-existing write target under a granted directory — allowed (the
   parent canonicalizes).

### Note for the final report

Item 5–7 above is the substance of unchecked finding **B36** (symlink escape).
Flag B36 as resolved by this work at the end; do not silently strip it.

## B4 — negative-cache a failed refresh

### Problem

`perform_http_get_cached` writes `fetched_at` only on a 2xx
(`perform.rs:89`). Once the TTL lapses on a dead endpoint, the fresh-hit branch
can never be taken again, so `fetcher.get(url)` is re-entered on *every* render
— up to 5 s of blocking inside the guest call, per render, forever. Under the
daemon that pins the shared render mutex and stampedes every other client into
an in-process fallback that repeats the same doomed fetch.
`perform_exec_cached` (`perform.rs:280`) has the identical shape.

### Design

- `CacheEntry` gains `#[serde(default)] pub last_attempt_at: String` — additive
  and forward-compatible, matching the wire-type discipline used elsewhere. An
  entry written by an older build deserializes with an empty string.
- On a **successful** refresh, set both `fetched_at` and `last_attempt_at` to
  `now`.
- On a **failed** refresh where an entry exists, rewrite the entry with
  `last_attempt_at = now`, leaving `body` / `status` / `fetched_at` untouched,
  then serve it stale exactly as today.
- Gate the refresh: skip the fetch and serve the stale entry immediately when
  `age_secs(now, last_attempt_at) < backoff`. Define `backoff` as the entry's
  own `ttl_secs` (so a dead endpoint is retried at most once per TTL window);
  an empty/unparseable `last_attempt_at` means "never attempted" and does not
  block a refresh.
- Mirror the field and the gate in `perform_exec_cached` for spawn failures and
  timeouts.
- On a failed refresh with **no** existing entry, behaviour is unchanged
  (`ok: false`) — there is nothing to negative-cache against, and writing a
  body-less entry would corrupt the stale-serve path.

### Tests

- A failing fetcher with a stale entry present: the first render fetches, the
  second within the backoff window does **not** call the fetcher and still
  serves the stale body with `stale: true`.
- After the backoff window lapses, the fetcher is called again.
- A successful refresh clears the backoff (both timestamps advance).
- An entry serialized without `last_attempt_at` still deserializes.
- The same three for `perform_exec_cached`.

## B13 — evict the TTL cache instead of wedging at the quota

Sequence **after B4** (both touch `CacheEntry` and `write_entry`) and **before
B19** (eviction bounds B19's seed walk).

### Problem

Nothing ever deletes a cache entry. Every distinct URL or canonical argv leaves
a file forever, even after its TTL expires, because expiry only overwrites the
*same* key. Once the directory reaches `max_state_bytes` (default 50 MiB) every
`write_entry` fails permanently, so the cache is silently off while every render
pays a live 5 s fetch or a real subprocess spawn — with 50 MiB of dead files on
disk.

### Design

In `crates/rustline-wasm/src/cache.rs`:

```rust
fn evict_namespace(dir: &Path, keep_bytes: u64) -> u64
```

Reads the namespace directory, deletes entries whose `fetched_at` is older than
a generous multiple of the TTL and, if still over `keep_bytes`, removes
oldest-mtime files until it fits. Returns bytes freed.

`write_entry` calls it **only when `check_cap` fails**, then retries the cap
check once. A full cache therefore self-heals instead of wedging permanently,
and the happy path pays nothing.

Eviction is best-effort: an `unlink` error is ignored (N2 — a cache is
disposable). Never delete anything outside the namespace directory, and never
delete the entry currently being written.

### Tests

- A namespace over the cap: `write_entry` evicts and the write then succeeds.
- Entries newer than the TTL window are preferred for retention over older ones.
- A cap so small that eviction cannot make room still returns `Err` (no
  infinite retry) and leaves the directory in a sane state.
- Files outside the namespace dir are untouched.

## B15 + B17 — one atomic-write implementation, collision-free

These land as **one task**: B17 is "the temp file's name is not unique" and B15
is "there is no temp file at all". Fixing them separately would leave two
different atomic-write shapes in the tree, which is the drift both findings
describe.

### Problem

- **B17:** `path.with_extension("tmp")` yields a single process-independent
  staging path, repeated verbatim at `sample_store.rs:35`, `cpu.rs:199`,
  `toggles.rs:58` and `rustline-wasm/paths.rs:79`. tmux runs `render left` and
  `render right` as separate processes, so process B can truncate the temp file
  A has already filled; A then renames a torn or zero-byte file into place. The
  parsers are total, so the damage is silent: a lost CPU sample costs a 120 ms
  `thread::sleep` on the next render, a lost throughput delta renders nothing,
  and a `{spark}` ring visibly loses samples.
- **B15:** `write_entry` (`cache.rs:84`) and `perform_state_write`
  (`perform.rs:362`) use plain `fs::write` — truncate-then-write, no temp file
  at all — so a concurrent reader gets a `serde_json` failure, treats it as a
  cold cache, and does the full live fetch the cache exists to avoid.

### Design

One helper, used everywhere:

```rust
// crates/rustline/src/sample_store.rs
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()>
```

Stages at `<path>.<pid>.<nanos>.tmp` in the same directory, writes, then
`fs::rename` onto the final path. Best-effort `remove_file` of the temp on the
write-error path so a crashed process leaves no litter. Rename is atomic within
a directory, so a concurrent reader sees either the old or the new entry, never
a partial one; last-writer-wins on the final path is unchanged and fine.

Call sites:

- `sample_store::write_sample` — collapse onto the helper.
- `cpu::store_snapshot` (`cpu.rs:199`) and `toggles::write_toggles`
  (`toggles.rs:58`) — collapse onto `sample_store::write_sample`/`write_atomic`
  so there is exactly one implementation to get right.
- `rustline-wasm/src/paths.rs:79` (`ensure_wasmtime_cache_config`).
- `rustline-wasm/src/cache.rs:84` (`write_entry`) — currently no temp file.
- `rustline-wasm/src/perform.rs:362` (`perform_state_write`) — currently no
  temp file.

`rustline-wasm` cannot depend on the `rustline` binary crate, so it needs its
own copy of the primitive (or the primitive moves to `rustline-core`). Prefer
moving it to a shared location over duplicating it — duplication is what these
two findings are about. If a shared home is not clean, duplicate it *once* with
a comment cross-referencing the other copy, and say so in the commit.

### Tests

- The staging name contains the pid (two writers produce different temp paths).
- A successful write leaves no `.tmp` behind.
- A concurrent-writer simulation (two writers, interleaved) always leaves a
  parseable file on disk.
- `write_entry`/`perform_state_write` round-trip unchanged.

## B16 — surface capability denials in the log

### Problem

Every deny site calls `observe_denial`, but the only sink is
`FileDenialObserver` appending to `<data_root>/denials.jsonl`. No `tracing`
event is emitted anywhere. Whether the user ever learns of a denial depends on
the *guest* choosing to call `rl_log`. A typo'd `allowed_urls` glob therefore
produces a blank widget, an empty log, and nothing pointing at
`rustline plugin denials`.

### Design

In `crates/rustline-wasm/src/denials.rs`:

- Change `record` to return `bool` (newly-recorded), reusing the dedup that
  already keeps the JSONL quiet.
- In `FileDenialObserver::observe`, when `record` reports a new triple, also
  emit:

  ```rust
  tracing::warn!(plugin, kind = ?kind, target,
      "capability denied; see `rustline plugin denials`")
  ```

Emit *alongside* the record, never instead of it. The dedup means a
persistently-denied plugin logs once per process, not once per render — do
**not** route this through D2's cross-process `warn_once`: a denial is a runtime
event and the JSONL is its durable record.

### Deliberate scope note

`bughunt.md` suggests combining this with redaction so a `target` carrying a
credential is not written verbatim. That redaction work is finding **B33**,
which the user did **not** select. This task therefore logs `target` verbatim,
matching what `denials.jsonl` already stores. Call this out in the final report
as a known interaction rather than expanding scope unasked.

### Tests

- A first denial logs; a repeat of the same triple does not.
- A different triple logs.
- The JSONL record is written in both cases exactly as before (no behaviour
  change to the existing sink).

## B18 — bound `rl_log` message size and rate

### Problem

`perform_log` (`perform.rs:448`) is the intentionally capability-free host
function and applies no length cap, no rate cap, and no per-render budget. A
guest has 16 MB of memory and a 10 s / 500 M-fuel budget, so one render can
issue thousands of calls or one ~16 MB message — filling the user's data dir and
destroying the very log they would consult to find out why.

### Design

Bound it at the host boundary, where the trust decision belongs. `rl_log` stays
capability-free (N1) — it is *bounded*, not *gated*.

- Truncate `msg` at `MAX_GUEST_LOG_BYTES` (2 KiB), appending `…(truncated)`.
  Truncate on a **char boundary** — the message is guest-supplied UTF-8 and a
  byte-index slice would panic, which is exactly the class of bug B21 describes.
- Add an `AtomicU32` call counter to `CapabilityCtx`. After
  `MAX_GUEST_LOG_CALLS` (e.g. 256) per plugin per process, stop emitting and
  log one final `warn!(plugin, dropped = n, "guest log rate limit reached")`.
- `perform_log` currently takes `&str` args and no ctx; it needs the
  `CapabilityCtx` for the counter. Update the `host_fn` wrapper in `host.rs`
  accordingly — the ctx is already in `UserData` at every other host function.

### Tests

The existing `RecordingSubscriber` harness in `perform.rs`'s `log_tests` module
asserts these directly:

- A 10 KiB message is emitted truncated to the cap plus the marker.
- A multi-byte character straddling the cap does not panic and is not split.
- Call N+1 is dropped and the one-time limit warning is emitted exactly once.
- The counter is per-`CapabilityCtx` (two ctxs have independent budgets — N4).

## B19 — memoize the state-dir size instead of walking it per write

Sequence **after B13** (eviction bounds the seed walk).

### Problem

`check_cap` calls `dir_size` — a full recursive `walkdir` + `metadata()` over
the plugin's whole state dir — on every write. All three trigger paths are
per-render (`perform_state_write`, and `write_entry` for both caches), and the
walk includes the two cache namespaces, so the syscall count per render grows
monotonically with the number of cached keys.

### Design

- Keep `check_cap` **pure**: change its signature to take the current size as a
  parameter rather than measuring it. This preserves N3 — the check stays
  strictly before the write and stays conservative.
- Memoize on `CapabilityCtx`: a `Cell<Option<u64>>` (the ctx is per-instance and
  not `Sync`-shared across threads for this purpose — if it must be, use
  `AtomicU64` + a seeded flag) seeded by one `dir_size` per process/daemon
  lifetime, then adjusted by `new_len - replaced` after each **successful**
  write.
- A failed write must not adjust the memo. An eviction (B13) must invalidate or
  decrement it — wire the two together so the memo cannot drift above the truth
  and wedge writes that should succeed. When in doubt, invalidate: a re-walk is
  correct, a stale-high memo is not.

### Tests

- The memo is seeded once and reused: N writes perform one `dir_size` walk
  (assert via a counting shim or by observing the memo).
- Quota enforcement is unchanged — a write that would exceed the cap is still
  refused (re-run the existing `check_cap` cases against the new signature).
- Eviction updates the memo so a post-eviction write succeeds.

## B27 — make the daemon fallback observable

### Problem

`try_render_at` (`daemon_client.rs:49`) has six consecutive `.ok()?` /
`return None` points and no `tracing` call anywhere in the module. The fallback
is correct and documented (N2 extended to the daemon path); its *silence* is
not. A wedged daemon — or a dead one leaving a stale socket file, which
`sock.exists()` happily accepts — makes every tick pay a 250 ms connect timeout,
and nothing in the log ties the sluggish bar to the daemon.

### Design

1. In `try_render_at`, replace the terminal `?`s with an `else`/`match` that
   `debug!`s the specific failing stage — "daemon connect failed",
   "daemon read timed out", "unexpected daemon response" — so `-vvv`
   distinguishes a stale socket from a wedged daemon without adding
   default-level volume. Route **connect-failure-on-an-existing-socket**
   specifically to `warn!`: that means a stale socket file needs cleaning and is
   actionable.
2. In `DaemonState::reload_if_changed` (`daemon.rs:85`), add
   `info!(config = %config_path.display(), "config changed; rebuilt warm state")`
   after the rebuild. The daemon is long-lived, so this is one line per config
   edit, not per render.

The `warn!` in (1) fires once per render while a stale socket exists — that is a
persistent misconfiguration, so route it through **D2's `warn_once`**, keyed on
the socket path.

### Tests

- Stale-socket path: a socket file that exists but refuses connection produces
  the warn and still falls back (returns `None`).
- Missing-socket path: no warn (the daemon is simply not installed — that is not
  an error).
- `reload_if_changed` logs exactly once per observed config change.

## Execution order

Dependencies are real; this order avoids rework and merge conflicts.

1. **D1** — logging.rs rewrite (touches logging.rs alone before D2 adds to it)
2. **D2** — `warn_once` infrastructure + the ~22 site conversions
3. **B12** — instance-opts warn (consumes D2)
4. **B4** — `last_attempt_at` negative caching
5. **B13** — cache eviction (after B4's `CacheEntry` change)
6. **B15 + B17** — one atomic-write implementation (after B13 settles cache.rs)
7. **B19** — memoize `dir_size` (after B13's eviction exists to wire into)
8. **B18** — `rl_log` caps
9. **B16** — denial → log
10. **B27** — daemon fallback logging (consumes D2)
11. **D3** — write-capability split + symlink policy (largest; lands last so it
    rebases over a settled `perform.rs`)
12. **Docs** — CLAUDE.md + README.md sync for every new config key and CLI
    subcommand, per the project's doc-list rule.

## Per-item contract

Every item follows the code-health Phase 4 contract:

- `risk: high` items (**B34** is not in this batch; **B4**, **B13**, **B20**-adjacent
  work, **B30** are not either) — for anything where the affected behaviour is
  not already covered, write a failing regression/characterization test first,
  confirm RED, and commit it as `test: characterize <unit> before fix [<id>]`.
  B4, B13, B19 and D3 all have uncovered affected paths and get this treatment.
- Apply the fix; the regression test must go GREEN.
- Run `cargo build --workspace`, `cargo test --workspace`, and `just lint`.
  Fix warnings the change introduced; leave preexisting unrelated ones alone.
  The baseline on this branch is **zero** lint warnings, so any warning is new.
- Commit as `fix(<category>): <summary> [<id>]`.
- **Strip the finding from `bughunt.md` in the same commit.**
- Full test suite at every 5th item and at each bucket boundary.

## Out of scope

Everything in `bughunt.md` not listed in the table above, specifically the
unchecked items B20, B21, B22, B23, B24, B25, B26, B28, B29, B30, B31, B32,
B33, B34, B35, B36, B37, B38, B39, and the two remaining `decision-needed`
markers (`handle_request` lock scope, `read_git`/`read_media` sample caching).

Two unchecked items are expected to be *resolved as a side effect* — **B20** by
D1 and **B36** by D3. Report them at the end for the user to confirm before they
are stripped.
