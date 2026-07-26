# code-health execution: markup safety, bounded reads, and silent-failure diagnostics

Date: 2026-07-26
Branch: `codehealth/2026-07-26`
Baseline: 128cd8a — build clean, `just lint` clean, 937 tests passing / 1 ignored.
Source: `bughunt.md` (39 findings, 5 decision-needed markers); this spec covers
**only** the 11 items the user checked `[x] execute`.

## Scope

| Task | ID | Category | Impact | Effort | Risk | Primary site |
|------|----|----------|--------|--------|------|--------------|
| 1 | B1 | security | 25 | M | high | `crates/rustline-core/src/render.rs:177` |
| 2 | B3 | frontend | 16 | S | high | `crates/rustline-core/src/render.rs:255` |
| 3 | B2 | observability | 20 | M | low | `crates/rustline/src/build_context.rs:84` |
| 4 | B5 | api-surface | 16 | M | medium | `crates/rustline/src/git.rs:12` |
| 5 | B7 | api-surface | 12 | S | medium | `crates/rustline-abi/src/lib.rs:246` |
| 6 | B8 | observability | 12 | S | low | `crates/rustline-core/src/assemble.rs:37` |
| 7 | B9 | api-surface | 12 | S | low | `crates/rustline-core/src/widgets/bar.rs:11` |
| 8 | B10 | observability | 12 | S | low | `crates/rustline-wasm/src/abi.rs:35` |
| 9 | B11 | observability | 12 | S | medium | `crates/rustline-wasm/src/host.rs:223` |
| 10 | B14 | api-surface | 12 | M | high | `crates/rustline/src/daemon_proto.rs:24` |
| 11 | B6 | caching | 16 | M | high | `crates/rustline/src/memory.rs:59` |

**Explicitly out of scope** (left unchecked in `bughunt.md`; must remain open and
untouched): B4, B12, B13, B15–B39, and all five `decision-needed` markers.
In particular **B26** (`WasmWidget::render`'s two existing `warn!` lines do not
carry `plugin = %self.name`) is NOT selected — tasks 8 and 9 touch that same
function and must leave those two existing lines alone so B26 stays a valid,
accurate open finding.

## Execution order and why it differs from rank order

Rank order is the *selection* key, not the execution key. Two pairs share a fix
site, so they run adjacently to avoid interleaving unrelated commits through the
same lines:

- **B1 → B3** share the three segment-text emission sites in `render.rs`
  (174/177, 255/260, 283/292). B1 introduces the sanitizing helper; B3 extends
  that same helper. Doing them adjacently means one helper is designed once.
- **B2 → B5** share `git.rs`, `media.rs`, `windows.rs`. B2 adds failure-path
  logging; B5 then replaces the spawn mechanism underneath it. B2 runs first so
  B5's task inherits a shape that already has the log lines, and B5 is
  responsible for adding a distinct `timeout` cause to them rather than dropping
  them.
- **B6 runs last** because it is the only task that cannot be compiled or tested
  on this machine (see Constraints), so it is the cheapest to revert if the
  milestone suite goes red.

## Per-task requirements

### Task 1 — B1: escape `#` in segment text before it becomes tmux markup

Add a private `fn escape_markup(s: &str) -> Cow<'_, str>` in `render.rs` that
doubles `#`, and apply it to `s.text` at the three emission sites: `render_region`
(render.rs:177), `render_region_ranged` (render.rs:260), `render_window_pill`
(render.rs:292). Separator, edge and range bytes stay unescaped — the renderer,
not the content, produces them.

Then teach `ansi.rs::scan` (ansi.rs:40/47) to collapse `##` back to a literal `#`
so `render … --preview`, `theme show`/`theme pick` and `widget_tui`'s
`parse_markup` preview strip keep agreeing with what tmux actually draws. Both
`tmux_to_ansi` and `parse_markup` share `scan`, so this is one change.

**Verification obligation (this is the load-bearing one).** `##` is tmux's
documented literal-`#` escape, but the spec author has not proven it collapses
inside a `#()` job's *output* on this tmux build. Before finalizing, run a real
check inside tmux — e.g. set a status-left to a `#()` script echoing
`a##[bg=red]b` and confirm the pane shows `a#[bg=red]b` as visible text with no
colour change. If it does **not** collapse there, fall back to the documented
alternative: strip or neutralize the `[` that follows a `#` instead of doubling.
Record which variant was used in the commit message.

Tests: a segment whose text is `#[bg=red]` must round-trip as visible characters
and emit no additional directive; a guest-style
`#[norange]#[range=user|cpu]` payload must not produce a second range; and the
existing byte-identical-output tests for range-free regions must still pass.

**Invariants this task depends on:** invariant #5 (`segments[0]` leftmost) and
invariant #7 (one range name end-to-end) must both still hold; the
`render_region_ranged` == `render_region` byte-identity property when every group's
range is `None` must be re-asserted after the change.

**Policy note (decide and record, do not silently pick):** the central escape also
neuters markup a WASM plugin or a user's own `format` string deliberately emits.
Central escaping is the recommendation because it is the only variant that covers
plugin-supplied `Segment.text`, which is the highest-authority vector. State the
chosen policy in the commit body.

### Task 2 — B3: strip C0 control characters at the same sites

Extend Task 1's helper so `\n`, `\r`, `\t` and other `char::is_control()`
characters in segment text are replaced (space or U+FFFD) before being written, at
render.rs:174/255/292. Keep the sanitization at the render boundary rather than in
each reader, so plugin-supplied `Segment.text` — which no reader touches — is
covered by the same guard.

Tests: a segment text containing `\n` must yield single-line markup (tmux keeps
only the last line of `#()` output, so a newline currently deletes everything to
its left); a segment text containing `\t` must not survive into the output.

### Task 3 — B2: make silent reader failures diagnosable

Two parts, neither of which may add per-render volume at the default `info` file
level.

1. Add a `tracing::debug!` at each reader's failure return, naming the reader and
   the concrete cause (missing binary / non-zero exit / unreadable path /
   unsupported platform): `git.rs:18`, `media.rs:44`, `battery.rs:30`,
   `disk.rs:23`, `uptime.rs:35`, `throughput.rs:71`, `cpu.rs:93`, `memory.rs:54`,
   `windows.rs:23`. `debug!` specifically, so `-vvv` reproduces it on demand
   without writing a line per second.
2. Add a `check_readers` pass to `doctor.rs` that calls each reader whose kind
   appears in `cfg.layout_kinds(&layout)` and reports `Warn` with the widget name
   for each that yields `None`, mirroring the existing `check_plugin_checksums`
   shape.

**Hard constraint:** like `check_plugin_checksums`, this row must be coded so it
can never produce `CheckStatus::Fail` — `doctor`'s exit code stays reserved for
setup that is outright broken, and a reader returning `None` is frequently
legitimate (no battery, not in a repo, no player running).

Do not change any reader's return value or gating. This task is additive only.

### Task 4 — B5: bound the render-path subprocess reads

`read_git` (git.rs:13), `read_media` (media.rs:41) and `read_windows`
(windows.rs:23) all call `Command::output()`, which blocks forever; under the
daemon a hang pins the shared render mutex for the life of the process.

Lift a `pub fn run_bounded(program, args, timeout) -> Result<(i32, String, String),
String>` out of `crates/rustline-wasm/src/run.rs` — which already implements spawn
+ timeout + process-group kill — and call it from `git.rs`, `media.rs` and
`windows.rs` with a sub-status-interval budget (500 ms). `rustline` already
depends on `rustline-wasm`, so this introduces no new dependency edge.

A timeout must be treated exactly like the existing non-zero-exit arm: `None` /
empty `Vec`, so invariant #6 holds (never a fabricated reading) and the widget
falls to `down_format`. Extend Task 3's `debug!` at each site with a distinct
timeout cause rather than dropping the logging.

Do not alter `run.rs`'s existing exec-capability behaviour, its `EXEC_TIMEOUT`, or
its `kill_group` reasoning; this is an extraction plus three call-site swaps.
Note B39 (an open, unselected finding) concerns `kill_group` being called on an
already-reaped pid — do not fix it here, and do not make the extraction depend on
that path's current shape being correct.

### Task 5 — B7: give `WireContext` struct-level serde defaults

Derive `Default` on `WireContext` and `WireWindowCtx` and add struct-level
`#[serde(default)]`, matching the discipline `HttpResult`/`ExecResult` and the
other host-effect result types already follow. Every field is
`String`/`Option`/`Vec`/`BTreeSet`, so `Default` derives cleanly.

Rationale: `ABI_VERSION`'s doc claims an additive wire change needs no bump because
"serde already tolerates that on both sides" — true only host→older-guest. Today
`git`, `disk`, `os`, `arch`, `interfaces`, `battery`, `cpu`, `memory`, `loadavg`
and `window` are required, so a newer-SDK guest on an older host fails to decode
and renders empty with no log.

Separately, make the SDK's `render_with` `Err(e)` arm call the SDK's own
`log(LogLevel::Warn, …)` instead of swallowing the decode error.

Test: a `GuestRender` JSON missing every one of those ten keys must deserialize to
defaults rather than erroring. This is the wire contract (invariant #2) — keep it
additive; do not add `deny_unknown_fields` anywhere.

### Task 6 — B8: name the widget in the panic guard

Change `render_guarded` to take the widget name (`assemble.rs:37`); both call
sites already have it — assemble.rs:93 destructures `(name, w)` from
`Registry::resolve`'s W53 pairs, and assemble.rs:127 can pass the literal
`"windows"`. In the `Err(payload)` arm, downcast the payload to `&str`/`String`
and log `tracing::warn!(widget = %name, panic = %msg, "widget panicked, skipping")`.

Both are private-fn changes inside the crate, so there is no public API break.
This closes a roadmap item CLAUDE.md already tracks ("naming the widget in the
panic-guard `warn!`").

Test: a deliberately panicking test widget must produce a warn carrying its name,
and must still degrade to empty segments (invariant #6 / N2 unchanged).

### Task 7 — B9: clamp `bar_width` / `spark_width` inside the renderers

Clamp inside `gauge_bar` itself (`let width = width.min(MAX_BAR_WIDTH);`,
`MAX_BAR_WIDTH` ≈ 256 — a status line is never wider) so the clamp holds for every
caller regardless of how the value arrived. Apply the same clamp to `sparkline`
and `history::push_truncate`.

Rationale: `bar_width: usize` has no upper bound, so `[widgets.cpu] bar_width =
1000000000` allocates ~3 GB per render and `i64::MAX` wraps the `width * 8` /
`width * 3` products in release. `catch_unwind` catches a panic but cannot catch
an allocation abort or an unbounded loop, so this currently breaks invariant #3
("a bad config must never break the bar") by hanging rather than degrading.

Optionally mirror the clamp on the config side so `print-config` / `config validate`
display the effective value. Default-width output must stay byte-identical.

### Task 8 — B10: log malformed plugin render output

`parse_render_output` turns any decode failure into an empty `Vec<Segment>` via
`unwrap_or_default()`, and the caller took the `Ok` branch, so nothing logs — the
only plugin failure mode with no distinguishing message, against ten distinct
`warn!`s in `register_plugins`.

Match on `serde_json::from_str::<Vec<Segment>>(s)` and in the `Err(error)` arm log
`tracing::warn!(%error, len = s.len(), "malformed plugin render output, rendering
empty")` before returning `Vec::new()`. `parse_render_output` is `pub`, so prefer
doing the match inline in `WasmWidget::render` (which already has `self.name` in
scope) over changing the public signature; if the signature is changed instead,
justify it.

**Constraint:** do not modify the two existing `warn!` lines at host.rs:219/230 —
that is B26, which is not selected.

### Task 9 — B11: recover (or at least report) a poisoned plugin mutex

`WasmWidget::render` returns `Vec::new()` on `Err(_)` from `self.plugin.lock()`
with no log. In the daemon the `WasmWidget` is warm for the process lifetime, so
one poisoning panic kills that plugin permanently and silently.

Mirror `daemon.rs:180`, which already recovers its own state mutex for exactly this
reason: `self.plugin.lock().unwrap_or_else(PoisonError::into_inner)`, plus a
one-shot `tracing::warn!(plugin = %self.name, …)` guarded by an `AtomicBool` on
`WasmWidget` so it logs once rather than every tick.

If recovery is judged unsafe for `extism::Plugin`'s internal state, keep the
bail-out but still add the warn — a permanently dead plugin must not be silent
either way. Record which option was taken and why.

**Constraint:** same as Task 8 — leave host.rs:219/230's existing lines alone.

### Task 10 — B14: version the daemon wire protocol

Add a versioned request variant, e.g.
`DaemonRequest::RenderV2 { protocol: u32, region, args }`. An old daemon cannot
deserialize an unknown variant, errors out, and `try_render_at` falls back
in-process — fail-closed by construction. The server must also reject a `protocol`
it does not equal, so a *new* daemon refuses an old client.

Keep `Ping` and `Shutdown` unversioned so `daemon status` / `daemon stop` still
work across a skew — otherwise the user cannot stop the stale daemon that caused
the problem.

Rationale: the daemon is long-lived and `reload_if_changed` watches only the
config file's mtime, never the binary's, so after `cargo install` the old daemon
keeps serving the new client with old semantics, silently.

Tests: a request carrying a mismatched `protocol` must be refused by the server;
`try_render_at` must return `None` (not panic, not a partial render) when the
response is anything other than a matching `Markup`. Preserve invariant N2 as
extended to the daemon path — any failure falls back to a correct in-process
render.

### Task 11 — B6: stop re-reading a machine constant every render on macOS

`read_memory_macos` spawns `sysctl -n hw.memsize` **and** `vm_stat` per call, and
`memory` is in the default right layout, so every macOS user pays two process
spawns per tmux tick out of the box. `hw.memsize` is constant for the machine's
lifetime.

**Constraint — this code cannot be verified on this machine.** It is behind
`#[cfg(target_os = "macos")]`, and cross-checking was attempted and failed:
`cargo check -p rustline --target aarch64-apple-darwin` cannot build `ring` (via
rustls → ureq) without a darwin C cross-compiler. So the macOS arm will be neither
compiled nor run before commit.

Therefore this task is **deliberately staged to the low-risk half**:

- Replace the `sysctl -n hw.memsize` **spawn** with `libc::sysctlbyname`, memoized
  in a `OnceLock`. `libc` is already a dependency and `cpu.rs` already establishes
  the mach-FFI precedent in this crate (`read_mach_cpu_ticks`, with its scoped
  `#[allow(deprecated)]`), so follow that file's shape and its `unsafe` commenting
  convention exactly.
- **Do not** migrate `vm_stat` to `host_statistics64(HOST_VM_INFO64)` in this
  batch. That is the higher-risk half, it is not needed to remove the
  constant-re-read defect, and it cannot be validated here. Leave
  `parse_macos_memory` and its call path intact.
- Follow the codebase's established answer to exactly this problem: put whatever
  new logic can be pure behind `#[cfg(any(target_os = "macos", test))]` and
  unit-test it on Linux, as `parse_macos_memory`, `parse_pmset` and
  `parse_kern_boottime` already are. The unverified surface must be reduced to the
  thin `sysctlbyname` wrapper.
- If the change cannot be structured so that the memoization/derivation logic is
  Linux-testable, stop and report rather than committing an untestable,
  uncompilable edit.

The commit body must state plainly that the macOS arm was not compiled or executed
and that verification on real hardware is outstanding.

## Global rules for every task

1. `risk: high` tasks (B1, B3, B14, B6) require a failing regression /
   characterization test committed **first** as
   `test: characterize <unit> before fix [B<n>]`, confirmed RED on unchanged code.
   B6 is the exception the plan must handle explicitly: its target code cannot run
   here, so the RED test applies to whatever pure helper is extracted, not to the
   macOS arm.
2. Never refactor or weaken an existing test to make a change pass. Add new tests.
   (One narrow exception exists in the open findings — B30 notes that
   `read_throughput_first_run_is_none_then_some_on_second_call` pins wrong
   behaviour — but B30 is **not** selected, so that test is not to be touched.)
3. Run `cargo build --workspace`, `just lint`, `cargo test --workspace` after every
   task. `just lint` is `cargo fmt --all --check` + clippy `-D warnings` + a
   `--features wasm-e2e` clippy pass; all three must stay clean. Fix warnings the
   change introduced; leave preexisting unrelated ones alone (baseline: zero).
4. Full suite at milestone boundaries — after tasks 5 and 10, and at the end.
   Baseline to beat: 937 passing, 0 failing, 1 ignored.
5. One commit per finding, `fix(<category>): <summary> [B<n>]`.
6. Strip the finding from `bughunt.md` as it lands. Note `bughunt.md` is listed in
   `.git/info/exclude`, so it is untracked and cannot literally ride in the commit;
   strip it anyway so the file keeps reflecting open issues only, and say so in the
   final report.
7. Editions stay 2024 across every crate, matching `rustfmt.toml` (per the user's
   global Rust instructions — mismatched editions cause permanent
   `cargo fmt --all` stray diffs).
8. Do not touch the five `decision-needed` markers or any unchecked finding.
9. If a task turns out to require a public-API signature break, a big rewrite, or an
   architectural change beyond what is written above, convert it to a
   `decision-needed` marker in `bughunt.md` and skip it rather than applying it.

## Invariants this work depends on

Re-check each of these after the batch; several tasks touch their enforcement
points directly:

- **#1** `Context` is the sole render input — Task 3/4 must keep all reads at the
  build edge, not mid-render.
- **#2** Wire types stay serde-serializable and additive — Task 5 changes exactly
  this contract.
- **#3** `Config::load` is total; a bad config must never break the bar — Task 7 is
  a direct repair of this.
- **#5** `segments[0]` leftmost regardless of `Direction` — Tasks 1/2 rewrite the
  emission sites that guarantee it.
- **#6** A failed read renders nothing, never fabricated values — Tasks 3/4 must not
  introduce a fabricated fallback on timeout.
- **#7** The click-toggle name is one identity end-to-end — Task 1 must not alter
  the bytes of an emitted `range=user|NAME`.
- **N1** Zero ambient authority — Task 1 is partly a repair of this (a
  zero-capability guest can currently forge markup).
- **N2** A plugin never breaks the bar — Tasks 8/9 must keep degrading to empty
  segments, and Task 10 must keep the daemon path falling back in-process.
