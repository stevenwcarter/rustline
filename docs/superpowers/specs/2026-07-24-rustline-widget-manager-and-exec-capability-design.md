# rustline — widget manager (CLI + TUI + tmux popup) and the plugin exec capability

Date: 2026-07-24
Branch: `feat/2026-07-24-widget-manager-exec`
Status: design approved (via `/ship-it --ask`)

Two independent features ship in this phase. They share no code and can be
implemented in either order; they are bundled because they were requested
together.

- **Part A — widget management.** A `rustline widget` command group
  (`list|enable|disable|move`), a ratatui full-screen editor
  (`rustline widget edit`) with a live preview, and a `prefix + W` tmux binding
  in `init`'s managed block that opens the editor in a `display-popup`. Closes
  `TODO.md`'s "Widget-management TUI / modal" and `WHATS-NEXT.md` W44.
- **Part B — the exec capability.** A seventh and eighth capability-gated host
  function, `rl_exec` and `rl_exec_cached`, gated by a new per-plugin
  `allowed_commands` allowlist, declared via `requested_commands` in a plugin
  manifest and granted by the existing `rustline plugin approve` flow. Plus a
  fifth worked example plugin, `cmdrun`.

---

## Part A — widget management

### A0. Problem

Adding, removing, or reordering a widget means hand-editing the `[layout]`
arrays in `config.toml` and reloading tmux. It is the single most common
ongoing configuration task and the only one with no CLI affordance — `theme`
has `theme use`/`theme pick`, `plugin` has `plugin url add`, layout has
nothing.

### A1. Layout algebra — `rustline-core::config` (pure, no I/O)

The mutation core lives next to the existing layout helpers (`layout_kinds`,
`disk_mounts`, `throughput_interfaces`, `is_builtin_widget_name`) because it
must agree with them about what a valid layout entry is: a built-in name, an
`[instances.<name>]` name, or a plugin stem. Splitting that knowledge across
crates is how the instance-shadowing bug in W46 happened.

```rust
/// Which of the three layout arrays a widget sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Region { Left, Center, Right }

impl Region {
    pub fn as_str(&self) -> &'static str;             // "left" | "center" | "right"
    pub fn parse(s: &str) -> Option<Region>;          // case-insensitive
    pub const ALL: [Region; 3];
}

impl Layout {
    pub fn get(&self, r: Region) -> &[String];
    pub fn get_mut(&mut self, r: Region) -> &mut Vec<String>;
    /// Which region `name` currently sits in, and at what index.
    pub fn find(&self, name: &str) -> Option<(Region, usize)>;
}
```

Four pure edit operations, each returning `Result<LayoutChange, LayoutEditError>`
and **never** mutating on the error path:

```rust
pub enum LayoutEditError {
    /// `name` is already in the layout (in `region`, at `index`).
    AlreadyPresent { region: Region, index: usize },
    /// `name` is not in the layout at all.
    NotPresent,
    /// The move would leave `name` where it already is (a no-op).
    NoOp,
}

/// A successful edit, described so a caller can report it without diffing.
pub struct LayoutChange {
    pub name: String,
    pub from: Option<(Region, usize)>,
    pub to: Option<(Region, usize)>,
}

pub fn layout_enable(layout: &mut Layout, name: &str, region: Region, at: Option<usize>)
    -> Result<LayoutChange, LayoutEditError>;
pub fn layout_disable(layout: &mut Layout, name: &str)
    -> Result<LayoutChange, LayoutEditError>;
/// Move within or across regions. `to_index` is clamped to the destination's
/// length, so `move x right 99` means "append".
pub fn layout_move(layout: &mut Layout, name: &str, to: Region, to_index: usize)
    -> Result<LayoutChange, LayoutEditError>;
/// Reorder by one step inside the widget's current region (the TUI's J/K).
pub fn layout_nudge(layout: &mut Layout, name: &str, delta: i32)
    -> Result<LayoutChange, LayoutEditError>;
```

Deliberate semantics:

- A widget appears **at most once** across all three regions. `layout_enable`
  on a name already present is `AlreadyPresent`, not a silent second copy —
  two copies of the same name would give both instances the same
  click-toggle/range identity, breaking invariant #7. (Two clocks is what
  `[instances.<name>]` is for, and an instance has its own distinct name.)
- Order within a region is visual left-to-right, matching invariant #5.
- `layout_enable`'s `at: None` appends.

**Availability** is a separate pure concern — what *could* be enabled:

```rust
/// One row of `widget list`: a name, whether/where it is placed, and where it
/// came from. `source` is `WidgetSource::{Builtin, Plugin}` reused from
/// `widget.rs`, plus a new `Instance` variant for an `[instances.<name>]` entry.
pub struct WidgetPlacement {
    pub name: String,
    pub summary: String,
    pub source: WidgetSource,
    pub placement: Option<(Region, usize)>,
}
```

`WidgetSource` gains an `Instance { kind: String }` variant. This is an
enum-variant addition to a public type; `plugin list --json` and any existing
match sites must be updated (there are few — `descriptors()` consumers).

### A2. Config writer — `rustline/src/widget_cmd.rs`

Mirrors `plugin_cmd.rs`'s `toml_edit` in-place editing exactly: load the file
as a `DocumentMut`, mutate, write back, preserving comments and formatting.

```rust
/// Read the layout as it exists in the *file* (not the effective config):
/// missing `[layout]` or a missing array means "use Config::load's default
/// for that region", so an edit against a zero-config install still writes a
/// complete, correct array.
fn read_layout(doc: &DocumentMut) -> Layout;

/// Write all three arrays back into `[layout]`, creating the table if absent.
/// Arrays are written multi-line when they exceed one line's worth, matching
/// the starter template's shape.
fn write_layout(doc: &mut DocumentMut, layout: &Layout);
```

The subcommands are thin: read → call the pure op → on `Ok` write + report,
on `Err` print a message and **exit non-zero without writing**. Every one of
them is a no-op on failure — a bad `widget` invocation must never corrupt
`config.toml` (the spirit of invariant #3).

After a successful write, if `$TMUX` is set, run `tmux refresh-client -S`
(best-effort; a spawn failure is logged, never fatal) so the bar updates
immediately. This is the same "make the change visible" step `TODO.md` calls
for.

### A3. CLI surface

```
rustline widget list [--json]
rustline widget enable <name> [--region left|center|right] [--index N]
rustline widget disable <name>
rustline widget move <name> --region <r> [--index N]
rustline widget edit
```

- `list` prints every registered widget — built-ins, `[instances.*]`, and
  discovered plugin stems — one per line, marking placement:
  `right[1]  cpu       CPU usage (builtin)`, `  -       git       git branch/status (builtin)`.
  `--json` emits `[{name, summary, source, region, index}]`, matching the
  `--json` convention W40 established on the other read-only list surfaces.
- `enable`'s `--region` defaults to `right` (where all but three of the
  built-ins belong, and where the default layout puts everything optional).
- `move` without `--index` appends to the destination region.
- Unknown `<name>` (not a built-in, not an instance, not a discovered plugin
  stem) is an error that writes nothing, listing the available names — the
  same shape as `theme use`'s unknown-name error and its
  `available_themes_line` hint.

**Plugin discovery without instantiation.** `widget list` and the TUI must
show plugin widgets, but must not *run* them: instantiating a guest is slow
and executes third-party code. `register_plugins` currently interleaves
discovery with instantiation, so `rustline-wasm` gains:

```rust
/// The `.wasm` stems present in `plugin_dir`, sorted, without reading or
/// instantiating any of them. Missing/unreadable dir → empty vec.
pub fn discover_plugin_names(plugin_dir: &Path) -> Vec<String>;
```

`register_plugins` is refactored to use it for its own directory scan, so
there is one definition of "what plugins exist".

### A4. TUI — `rustline/src/widget_tui.rs`

Layout (the mockup approved during brainstorming):

```
┌ rustline widget edit ───────────────────────────────┐
│ LEFT      CENTER      RIGHT       │ AVAILABLE       │
│ ▸pane_id  windows     cwd         │  battery        │
│  hostname             cpu         │  git            │
│                       memory      │  disk           │
│                       loadavg     │  uptime         │
│                       datetime    │  media          │
│                                   │  weather (plug) │
├─────────────────────────────────────────────────────┤
│  0:1.0  host      1:zsh* 2:vim    ~/src/rustline  … │
├─────────────────────────────────────────────────────┤
│ ←→ region  ↑↓ select  space add/remove  J/K reorder │
│ w write   q quit                                    │
└─────────────────────────────────────────────────────┘
```

Four columns (LEFT / CENTER / RIGHT / AVAILABLE), a live preview strip, and a
key-hint footer.

**The state machine is pure and separately tested.** ratatui only draws and
feeds keys:

```rust
/// The four focusable columns. The three region columns map onto `Region`;
/// `Available` is the not-currently-placed pool.
pub enum Column { Left, Center, Right, Available }

/// Everything the editor knows. No terminal, no files, no I/O.
pub struct EditorState {
    layout: Layout,
    available: Vec<WidgetPlacement>,  // not-currently-placed, sorted
    column: Column,                   // Left | Center | Right | Available
    cursor: usize,                    // index within the focused column
    dirty: bool,
}

pub enum EditorAction { Redraw, Quit, Write, ConfirmQuit }

impl EditorState {
    pub fn new(layout: Layout, all: Vec<WidgetPlacement>) -> Self;
    /// The one entry point ratatui calls. Pure: key in, action out, state mutated.
    pub fn on_key(&mut self, key: KeyKind) -> EditorAction;
    pub fn columns(&self) -> [(Column, Vec<&str>); 4];  // what to draw
    pub fn selected(&self) -> Option<&str>;
    pub fn is_dirty(&self) -> bool;
    pub fn layout(&self) -> &Layout;
}
```

`KeyKind` is rustline's own small enum (`Up`, `Down`, `Left`, `Right`,
`Space`, `NudgeUp`, `NudgeDown`, `RegionPrev`, `RegionNext`, `Write`, `Quit`,
`Help`, `Other`) mapped from
crossterm's `KeyEvent` by one thin `fn map_key(KeyEvent) -> KeyKind`. Tests
drive `on_key` with `KeyKind` values directly — no terminal, no crossterm
event loop, the same "make the interesting part I/O-free" move
`theme_cmd.rs`'s reader/writer-generic `run_picker` makes.

Bindings:

| Key | Action |
|---|---|
| `←` / `→` / `h` / `l` | move focus between the four columns |
| `↑` / `↓` / `k` / `j` | move the cursor within the focused column |
| `space` / `enter` | AVAILABLE → append to RIGHT (the only region AVAILABLE placement appends into); a region column → back to AVAILABLE |
| `J` / `K` | nudge the selected widget down/up within its region |
| `H` / `L` | move the selected *placed* widget directly to the other editable region (LEFT ↔ RIGHT, skipping over the fixed CENTER region in between) — this is the only way to place a widget in LEFT; refused with an explanation if the widget is currently in CENTER |
| `w` | write `config.toml` and stay open (clears dirty, reports in the footer) |
| `q` / `esc` | quit; if dirty, one `y/N` confirm line before discarding |
| `?` | toggle an expanded help/legend |

Note: an earlier revision of this spec described `space`/`enter` as appending
into the "last-focused region." In practice `Column::ALL`'s fixed adjacency
(`[Left, Center, Right, Available]`) means normal focus navigation always
passes through RIGHT immediately before reaching AVAILABLE, overwriting
`last_region` to RIGHT every time — so AVAILABLE placement always lands in
RIGHT. `H`/`L` (added after a whole-branch review caught this) are what make
LEFT reachable at all.

**Live preview.** Below the columns, one line rendering the current (unsaved)
layout: `sample_context(false)` + the resolved theme + `render_named_region`,
transcoded to ANSI via `tmux_to_ansi` — reusing exactly the pipeline
`theme show` and `theme pick` already use. Left/center/right are drawn joined,
truncated to the terminal width.

**Plugins render as a static chip in the preview**, e.g. a `[weather]`
placeholder styled with the theme's `info` color, *not* a live guest render.
Instantiating WASM guests on every keystroke is slow and would execute
third-party code inside a config editor. This is a documented limitation, not
an oversight; the footer notes it the first time a plugin is placed.

**Terminal lifecycle.** Raw mode + alternate screen entered on start and
restored on *every* exit path, including a panic — the restore is installed as
a panic hook (and run in a `Drop` guard) so a panic inside the draw loop can't
leave the user's terminal in raw mode. `rustline widget edit` requires a TTY;
a non-TTY invocation prints a hint toward `rustline widget list`/`enable`
and exits non-zero without drawing, mirroring `theme pick`'s TTY guard.

### A5. tmux integration

`tmux_conf.rs`'s `init_block` gains one stanza, emitted unconditionally like
the three `MouseDown*Status` bindings (the `mouse` answer only controls
`set -g mouse on`, so it does not gate this either):

```
# rustline widget manager (prefix + W)
bind-key W display-popup -E -w 80% -h 80% "@BINARY@ widget edit"
```

`@BINARY@` goes through the same blanket replace as every other call site, so
it is the shell-quoted absolute path (invariant #4 and the "tmux's `/bin/sh`
may not have your `$PATH`" rationale). No tmux format variable is
interpolated here at all, so there is no `#{q:}` concern.

Consequences to keep consistent:
- The `init --print` legacy one-line block is **not** special-cased here:
  `--print` calls the same `tmux_conf::init_block` as every other path (just
  with `two_line: false, mouse: false, interval: 1`), and `init_block` has
  always emitted the `MouseDown{1,2,3}Status` bindings unconditionally. So
  `--print`'s block **also** gains the new `bind-key W` line — that is the
  correct, consistent outcome, not a gap to special-case around. No existing
  test pins `--print`'s output byte-for-byte, so nothing needs to change to
  make this true; it already falls out of `init_block` being one shared
  function.
- `init --uninstall` already strips the whole marker block, so the binding is
  removed with it — nothing to add.
- `tmux_conf.rs`'s characterization tests must be updated to expect the new
  line; that is the point of those tests.
- Requires tmux ≥ 3.2 for `display-popup`. `doctor` currently checks for
  ≥ 3.1 (the `range=user|` floor). It gains an advisory-only row: `pass` at
  ≥ 3.2, `warn` at 3.1.x ("`prefix + W` widget manager needs tmux ≥ 3.2; the
  status line itself works"), never affecting the exit code — the same shape
  as the daemon-reachability row.

### A6. Dependencies

`ratatui = "0.30"` on the bin only, default features (which pull
`ratatui-crossterm` and re-export crossterm as `ratatui::crossterm`, so
crossterm is not a separate direct dependency). **Not feature-gated**, unlike
`bench`: `init` writes the `prefix + W` binding unconditionally, so a
feature-gated subcommand could be missing at the other end of a binding we
emitted. ratatui's MSRV is 1.88; the project is on 1.97.

`cargo tree -i openssl` / `-i native-tls` must stay empty (the standing
rustls-only policy). Verified against a scratch crate on ratatui 0.30.2:
`ratatui::crossterm::{event, terminal}` resolves with default features (via
`ratatui-crossterm` 0.1.2 → crossterm 0.29), and both `cargo tree -i` queries
match no packages.

---

## Part B — the plugin exec capability

### B0. Problem and threat model

A plugin can fetch a URL and read a file but cannot ask the system a question
that only a program can answer (`git`, `playerctl`, `kubectl`, a user script).
Adding process execution to a sandbox is the highest-risk capability in the
system, so the design is deliberately the most restrictive shape that is still
useful:

- **No shell, ever.** The host spawns `program` with an explicit `args` vector
  via `std::process::Command`. Nothing re-parses a string, so there is no
  quoting, word-splitting, globbing, or injection surface. This is the same
  reasoning behind invariant #4's `#{q:}` rule and `click.rs`'s rule that the
  only text `sh -c` ever sees comes from the user's own config.
- **The whole argv is gated, not just the program.** `git status --porcelain`
  and `git push --force` are different grants.
- **Deny by default.** An empty (or absent) `allowed_commands` matches
  nothing, exactly like `allowed_urls`/`allowed_paths` today.

### B1. Wire types — `rustline-abi`

One new result type, following W51's "one canonical definition, re-exported by
both sides" rule and the struct-level `#[serde(default)]` forward-compat rule:

```rust
/// Result of a host-executed subprocess. `ok` means "the process was allowed
/// and ran to completion" — NOT that it succeeded; check `status`. A non-zero
/// exit is data, not an error, so a guest can render a fallback from it.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExecResult {
    pub ok: bool,
    /// The process exit code, or -1 if it was killed by a signal or timed out.
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
    /// Non-empty iff `ok` is false: denied, spawn failure, or timeout.
    pub error: String,
    /// True iff stdout or stderr was truncated at the output cap.
    pub truncated: bool,
}

/// Result of a TTL-cached exec. `ok` means "a usable result is present"
/// (fresh OR stale) — the same convention as `CachedHttpResult`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CachedExecResult {
    pub ok: bool,
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
    pub error: String,
    pub stale: bool,
    pub age_secs: i64,
    pub truncated: bool,
}
```

`ABI_VERSION` stays `1`. Adding host functions and adding fields to types the
guest doesn't already receive is purely additive: an existing guest never
calls the new imports and deserializes an unchanged `GuestRender`. Per
invariant #2's additive rule, this is not a breaking wire change.

### B2. Gating — `rustline-wasm`

`PluginConfig` gains one field, `#[serde(default)]` like every other:

```rust
/// Command allow-patterns. Each entry is a glob by default, or a regex when
/// prefixed `re:`, matched against the **canonical argv string** (below).
#[serde(default)]
pub allowed_commands: Vec<String>,
```

`CapabilityCtx` gains `allowed_commands: AllowSet`, compiled the same way.
`DenialKind` gains a `Command` variant (serde `snake_case` → `"command"`), so
a denied exec is recorded by `FileDenialObserver` into `denials.jsonl` and
surfaced by `rustline plugin denials <name>` with no further change.

**The canonical argv string** is the one thing to get exactly right, because
it is what the user's pattern is matched against:

```rust
/// Render `program` + `args` to the single string an `allowed_commands`
/// pattern is matched against: space-joined, with any element containing a
/// space, tab, newline, quote, or backslash wrapped in single quotes and its
/// own single quotes escaped.
///
/// This is a MATCHING key, never something that is executed — the spawn always
/// uses `program` and `args` directly. The quoting exists so that
/// `exec("git", ["log", "--author=a b"])` cannot be made to match a pattern
/// written for `exec("git", ["log", "--author=a", "b"])`.
pub fn canonical_argv(program: &str, args: &[String]) -> String;
```

Examples:

| call | canonical string |
|---|---|
| `exec("playerctl", ["metadata"])` | `playerctl metadata` |
| `exec("git", ["log", "--author=a b"])` | `git log '--author=a b'` |
| `exec("sh", ["-c", "rm -rf /"])` | `sh -c 'rm -rf /'` |

The third row is the important one: running `sh` is not special-cased or
forbidden — it is simply a command like any other, and a user who writes
`allowed_commands = ["sh *"]` has granted it explicitly and visibly. The host
never *introduces* a shell on its own.

### B3. Execution — `perform.rs` + a `Runner` seam

Mirroring `Fetcher`/`UreqFetcher`, so every gate test runs without spawning a
process:

```rust
pub trait Runner {
    /// Run `program` with `args`. Returns (exit_code, stdout, stderr) on a
    /// completed run, or Err(message) on spawn failure or timeout.
    fn run(&self, program: &str, args: &[String]) -> Result<(i32, String, String), String>;
}

/// The production runner: direct spawn, no shell, piped stdout/stderr,
/// stdin null, inherited env and cwd, wall-clock bounded, output capped.
pub struct ProcessRunner;
```

Bounds, all constants in one place:

- **Wall clock: 5 s**, deliberately under Extism's 10 s plugin timeout so an
  exec that hangs surfaces as `ok: false, error: "timed out"` — a result the
  guest can render a fallback from — rather than killing the whole plugin
  render. On timeout the child is killed and reaped.
- **Output: 64 KiB each** for stdout and stderr. Beyond that the stream is
  truncated at a UTF-8 boundary and `truncated: true` is set. Lossy UTF-8
  conversion, so binary output degrades to replacement characters instead of
  an error.
- **stdin is `Stdio::null()`** — a child that reads stdin gets EOF immediately
  instead of blocking until the timeout.
- **Environment and cwd are inherited** and documented as such. Scrubbing is
  safer in the abstract but breaks the actual use cases (`playerctl` needs
  `DBUS_SESSION_BUS_ADDRESS`, `git` needs `HOME`). This is written into the
  README's plugin-security section, not left implicit.

The two effect functions:

```rust
pub fn perform_exec(ctx: &CapabilityCtx, program: &str, args: &[String], runner: &dyn Runner)
    -> ExecResult;

/// Gate-first (a denied argv spawns nothing and touches no cache), then a
/// TTL-cached run keyed on the canonical argv. Only a **zero-exit** run is
/// cached; a non-zero exit is fresh, real data and is returned as-is, not
/// cached — only a spawn failure or timeout falls back to the last-good entry
/// served stale. Exactly `perform_http_get_cached`'s shape, with "2xx"
/// replaced by "exit 0".
pub fn perform_exec_cached(
    ctx: &CapabilityCtx, program: &str, args: &[String],
    ttl_secs: i64, now: &str, runner: &dyn Runner,
) -> CachedExecResult;
```

**Cache namespacing.** `cache.rs`'s `cache_path` currently hardcodes
`__http_cache__/`. It gains a namespace parameter so an exec entry can never
collide with an HTTP entry that happens to hash the same:

```rust
pub fn cache_path(state_dir: &Path, namespace: &str, key: &str) -> PathBuf;
// http:  cache_path(dir, "__http_cache__", url)
// exec:  cache_path(dir, "__exec_cache__", canonical_argv(program, args))
```

Both live under the plugin's own state dir, so `check_cap`'s quota accounting
(invariant N3) already covers them with no change.

### B4. Host functions — `host.rs`

Two more `host_fn!` wrappers, bringing the total to nine (eight
capability-gated + the capability-free `rl_log`):

```rust
host_fn!(rl_exec(user_data: CapabilityCtx; program: String, args_json: String) -> String);
host_fn!(rl_exec_cached(user_data: CapabilityCtx; program: String, args_json: String,
                        ttl_secs: String, now: String) -> String);
```

`args` crosses the boundary as a JSON array string (Extism host functions take
scalars/strings, and the existing `ttl_secs: String` sets the precedent for
"encode it and parse host-side"). A malformed `args_json` is `ok: false` with
an error, never a panic and never a spawn.

### B5. Manifest and approval

`PluginManifest` gains one field:

```rust
/// Command allow-patterns the plugin asks the user to approve.
#[serde(default)]
pub requested_commands: Vec<String>,
```

`plugin approve` prints it alongside the other two and writes it verbatim into
`allowed_commands` — the same idempotent-append, never-widen rule. `print_manifest`
gains a third `print_requests("allowed_commands", …)` line.

**The approval prompt calls out that this one is different.** Where the
manifest requests commands, `approve` prints a warning line before the
confirmation:

```
plugin cmdrun (version 0.1.0) requests:
  allowed_urls:
    (none)
  allowed_paths:
    (none)
  allowed_commands:
    playerctl metadata *

  ! allowed_commands runs real programs on your machine with your
  ! environment and permissions. Approve only patterns you understand.

Approve these capabilities? [y/N]
```

`--yes` still bypasses the prompt (it is the documented non-interactive
escape hatch), but the warning is printed either way so it lands in logs and
transcripts.

`plugin list [--json]` gains `allowed_commands` alongside the other two
allowlists, and `plugin url|path list|add|remove` gains a parallel
`plugin cmd list|add|remove <plugin> [pattern]` group so a command grant can
be inspected and revoked from the CLI like the other two.

### B6. Guest SDK

`rustline-plugin-sdk` gains typed wrappers following the existing shape
exactly — real Extism imports on `wasm32`, `HostError::Unavailable` stubs on
the host target so a plugin's pure logic still unit-tests under `cargo test`:

```rust
pub fn exec(program: &str, args: &[&str]) -> Result<ExecResult, HostError>;
pub fn exec_cached(program: &str, args: &[&str], ttl_secs: u64, now: &str)
    -> Result<CachedExecResult, HostError>;
```

plus re-exports of `ExecResult`/`CachedExecResult` from `rustline-abi`.

### B7. Example plugin — `plugins/cmdrun`

A fifth excluded workspace member (`wasm32-unknown-unknown`, its own
`Cargo.lock`, an **empty `[workspace]` table** so nested-worktree builds work),
mirroring `httpget`'s shape:

- Options: `program` (string), `args` (array of strings), `ttl_secs`
  (integer, `0` = use the plain uncached `rl_exec`), `format` (with `{out}`,
  `{status}` placeholders), `down_format`.
- Runs the configured command, renders a first-line snippet of stdout.
- A denial, spawn failure, timeout, or non-zero exit is `rl_log`ged with the
  reason and falls back to `down_format` — the same convention as the other
  three examples.
- Ships a sidecar `cmdrun.toml` manifest requesting a narrow example pattern,
  so `rustline plugin approve cmdrun` has something real to demonstrate.
- Pure logic (`extract_snippet`, `render_format`, `select_host_fn`) unit-tested
  on the host target; the `#[cfg(target_arch = "wasm32")] mod guest` holds the
  Extism exports.

`justfile`'s `build-plugin NAME` recipe is already generic, so
`just build-plugin cmdrun` works with no change.

---

## Invariants this feature depends on

Listed explicitly so a later change touching one of these funnels can grep for
who relies on it.

1. **Invariant #7 (one click-toggle identity end-to-end).** Part A's
   "a widget appears at most once in the layout" rule exists *because* two
   copies would share a range name. A future change that permits duplicates
   must revisit `layout_enable`.
2. **Invariant #3 (`Config::load` is total).** Part A's writer must never
   produce a file that `Config::load` would reject; every write path is
   round-tripped through a strict parse in tests.
3. **Invariant #2 (wire types stay additive, no `deny_unknown_fields`).**
   Part B adds `ExecResult`/`CachedExecResult` and two host imports without
   bumping `ABI_VERSION`, which is only sound while guests tolerate unknown
   fields and simply never call imports they don't declare.
4. **Invariant N1 (zero ambient authority).** `perform_exec`/
   `perform_exec_cached` gate before *any* effect — before spawning and before
   touching the cache — and call `observe_denial` at each deny site.
5. **Invariant N2 (a plugin never breaks the bar).** Every exec failure mode
   (denied, malformed args, spawn failure, timeout, non-UTF-8 output) is a
   populated result struct, never a panic and never an `Err` that aborts the
   render.
6. **Invariant N3 (state is quota-bounded).** The exec cache lives under the
   plugin's own state dir so `check_cap` already accounts for it; the
   namespace change to `cache_path` must not move it outside that dir.
7. **Invariant #4 (`init` output is injection-safe).** The new `bind-key W`
   line interpolates no tmux format variable, only the already-shell-quoted
   `@BINARY@`.
8. **`Registry`/`is_builtin_widget_name` precedence (built-in wins).**
   Part A's name resolution must reject a `widget enable` of a name that
   `Registry::with_builtins` would skip, rather than writing a layout entry
   that silently renders nothing.

## Testing

Per the project's TDD discipline, and specifically writing the tests that
would otherwise be declined on an invariant argument.

**Part A — pure (unit, `rustline-core`):**
- Each of the four layout ops: success, each error variant, and that the error
  path leaves `Layout` untouched (asserted by comparing against a clone).
- `layout_enable` rejects a name already present in a *different* region.
- `layout_move` clamps an out-of-range index rather than erroring.
- `layout_nudge` at a region boundary is `NoOp`, not a wrap-around.
- Placement/availability derivation over a config with built-ins, an
  `[instances.<name>]`, and a plugin stem — including that an instance whose
  name collides with a built-in is not offered (the W46 precedence guard).

**Part A — writer (unit, bin):**
- `toml_edit` round-trip preserves a comment and a non-layout table.
- Editing a config file with **no** `[layout]` table writes all three arrays
  seeded from `Config::load`'s defaults.
- Every writer output is re-parsed with the *strict* parser (`config validate`'s
  path), not just `Config::load` — a write that only survives the total loader's
  fallback is a bug.

**Part A — TUI state (unit, bin):** `on_key` sequences over a synthetic
`EditorState` — focus movement, add/remove, nudge, dirty tracking, and that
`Quit` while dirty yields `ConfirmQuit` rather than `Quit`. No terminal.

**Part A — tmux block (characterization, bin):** the existing `init_block`
characterization tests updated to include the `bind-key W` line, and one
asserting `--print`'s legacy block is still byte-identical.

**Part B — gating (unit, `rustline-wasm`, load-bearing per N1):**
- `canonical_argv` quoting: plain args, an arg with a space, an arg with an
  embedded single quote, an empty arg.
- **Denied-case tests** (the ones that matter): an argv not matching any
  pattern returns `ok: false`, calls `observe_denial` with
  `DenialKind::Command`, and — asserted with a recording `Runner` — **never
  calls `run`**. Same for the cached variant, plus that it never touches the
  cache dir.
- An `allowed_commands` pattern that matches the program but not the args
  (`git status *` vs `git push`) denies.
- A malformed `args_json` denies without spawning.
- Cached: fresh hit doesn't re-run; TTL expiry re-runs; a non-zero exit is not
  cached; a failed refresh serves the previous entry with `stale: true`.
- Cache namespacing: an http entry and an exec entry with the same key string
  resolve to different paths.

**Part B — runner (unit, bin/`rustline-wasm`):** `ProcessRunner` against
real trivial commands where the platform allows — exit code propagation,
stderr capture, the output cap setting `truncated`, and a timeout killing a
`sleep`. These are the only tests that spawn; they use commands guaranteed
present (`true`/`false`/`sleep` on POSIX) and are `#[cfg(unix)]`.

**Part B — e2e (`wasm-e2e` feature):** the `cmdrun` plugin rendered through
the real host, once denied and once approved, asserting the denial is recorded.

## Documentation

`CLAUDE.md` and `README.md` both carry lists this change invalidates, and the
project's standing rule is that a new widget/plugin/capability syncs both:

- The `plugins/` list gains `cmdrun` (CLAUDE.md's architecture section, its
  module map, and README's plugin list).
- The host-function count changes from seven to nine everywhere it is stated,
  including invariant N1's wording.
- `PluginConfig`/`[plugins.<name>]` documentation gains `allowed_commands`;
  the manifest section gains `requested_commands`; a new
  "the exec capability" subsection covers the no-shell rule, the argv gate,
  the bounds, and the inherited-environment caveat.
- The CLI section gains the `widget` group, `plugin cmd`, and the changed
  `plugin list --json` shape.
- The tmux integration section gains the `prefix + W` binding and the
  tmux ≥ 3.2 note.
- The module map gains `widget_cmd.rs`, `widget_tui.rs`, and the
  `rustline-core` layout-algebra additions.
- `TODO.md`'s "Widget-management TUI / modal" section is removed (it is
  delivered); `WHATS-NEXT.md`'s W44 is stripped per that file's own
  strip-on-ship rule.
- The roadmap gains a "Done" entry for both parts, linking this spec and its
  plan.

## Out of scope

Explicitly not in this phase, to keep the boundary clear:

- Per-widget `format`/`alt_format` editing in the TUI (the tier-2 option that
  was not selected).
- Creating/deleting `[instances.<name>]` from the TUI (tier 3).
- A tmux `display-menu` right-click variant (a possible later addition; the
  mutation core is shared, so it stays cheap).
- Live plugin rendering in the TUI preview.
- `TODO.md`'s "generic informational pop-ups" item and the weather-widget
  improvements — unrelated, untouched.
- Environment scrubbing / a per-plugin env allowlist for exec. Worth
  revisiting; noted as a follow-up rather than half-built here.
