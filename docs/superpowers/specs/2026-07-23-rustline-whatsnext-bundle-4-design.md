# rustline whats-next bundle #4 — design

Date: 2026-07-23
Branch: `whats-next/2026-07-23-execute`
Source: `/whats-next --execute` handoff of W33 + W46 + W48 from `WHATS-NEXT.md`.

Three independent features, one branch, built smallest-blast-radius first:
**W33 (init wizard seeding + confirm) → W46 (multiple widget instances) →
W48 (optional persistent daemon)**. The daemon consumes the render path the
other two touch, so it lands last and reuses their code unchanged.

## Goals

- **W33** — Re-running `rustline init` to tweak one setting should not silently
  reset every other answer, and should show what will change and ask before
  overwriting `config.toml` / `~/.tmux.conf`.
- **W46** — Let the same widget *kind* appear more than once in a layout with
  distinct options (dual clocks in different timezones, a LAN + a labeled
  secondary IP, multiple disk mounts, per-interface throughput).
- **W48** — An optional, opt-in persistent daemon that keeps the parsed config,
  resolved theme, warm widget registry, and instantiated WASM plugins alive
  across tmux refreshes, so a refresh costs one warm render instead of N cold
  process spawns — without ever being able to break the bar.

## Non-goals

- No daemon self-daemonization (double-fork). `rustline daemon` runs in the
  foreground; a supervisor (systemd unit, tmux `run-shell … &`) backgrounds it.
- No auto-spawn of the daemon by render clients (explicitly rejected in
  brainstorming — opt-in only).
- No `tokio`/async. Std `UnixListener` + threads only.
- No new wizard question for the daemon (documented manual start; a `doctor`
  note reports whether a daemon socket is present). Wiring a `run-shell` line
  into the `init` block is a follow-up, not v1.
- No batched multi-window render (that is W42, separate).

## Invariants this feature depends on (re-check when touching these)

These are the load-bearing invariants (from `CLAUDE.md`) each feature relies on;
a later change to any of them must keep the enumerated producers working.

1. **`Context` is the sole render input** (invariant #1). W46's per-instance
   reads land in `Context` at build time; widgets stay `Context`-only. W48's
   daemon builds the *same* `Context` via `build_region_context` and renders via
   the *same* `render_named_region`, so daemon output is byte-identical to the
   in-process path.
2. **Wire types stay additive, no `deny_unknown_fields`** (invariant #2). W46's
   new `Context.disks`/`Context.throughputs` maps are NOT mirrored into
   `WireContext` (like `uptime`/`media`/`throughput`/`cpu_history`/`mem_history`
   already aren't). `Context.disk`/`Context.throughput` stay populated with the
   base instance's reading, so a WASM guest reading them is unaffected.
3. **`Config::load` is total** (invariant #3). A malformed `[instances]` entry,
   an unknown `kind`, or an instance name colliding with a built-in must warn +
   skip, never fail the parse or break the bar. Every new field is
   `#[serde(default)]`; absent `[instances]` renders byte-identically to today.
4. **`init` output stays injection-safe** (invariant #4). W33 changes only how
   answers are *gathered* and adds a confirm gate; the emitted tmux block and
   `#{q:}`/`--flag=` forms are unchanged.
5. **The click-toggle NAME is one identity end-to-end** (invariant #7). A W46
   instance's name is its layout entry, its `range=user|NAME`, its
   `Context.toggled` key, its `range_name()`, and its `active_format` toggle key
   — all the *instance* name, not the kind. This requires threading the instance
   name into each clickable widget (see W46 below).
6. **A plugin/instance never breaks the bar** (invariants N2, #6). W48's client
   falls back to the in-process render on ANY daemon failure; the daemon never
   fabricates output.

---

## W33 — init wizard: seed from existing config + confirm before write

### Current behavior (`crates/rustline/src/init.rs`)

`prompt_answers(themes_dir) -> InitAnswers` starts from `defaults()` and asks
~8 questions from hardcoded defaults. `run` then calls `apply` which overwrites
both files (after backups) with no review step. `InitAnswers` has: `theme`,
`two_line`, `mouse`, `battery`, `tailscale`, `lan_ip`, `clock`, `interval`.

### Change

**Seeding (config-only, per the chosen scope).** Add:

```rust
/// Pre-fill wizard answers from an existing config.toml. Recovers what config
/// actually stores; the tmux-only answers (mouse/two_line/interval) keep their
/// recommended defaults, since they live only in the managed tmux block.
fn seed_answers(config_path: &Path) -> InitAnswers
```

Behavior:
- If `config_path` does not exist: return `defaults()`, except
  `battery = crate::battery::read_battery().is_some()` (preserves today's
  hardware-detected battery default for a fresh run).
- If it exists: load via `Config::load` (total) and derive:
  - `theme` ← `cfg.theme.base` (else `"default"`).
  - `battery` / `lan_ip` / `tailscale` ← the name appears in any `cfg.layout.{left,center,right}`.
  - `clock` ← reverse-map `cfg.widgets.datetime.format` against the four
    `ClockStyle` presets' `.0` format string; no exact match → `defaults().clock`.
  - `two_line` / `mouse` / `interval` ← `defaults()` (not recoverable from config).

Add a pure helper `fn clock_from_format(fmt: &str) -> Option<ClockStyle>`
(exact match against the four presets) — unit-tested, the load-bearing reverse
map.

`prompt_answers` gains a `seed: &InitAnswers` parameter and uses each seed field
as that question's shown default (theme menu default = index of `seed.theme` in
the list, else 0; each `ask(...)` uses `seed.X`; clock default = index of
`seed.clock`; interval `ask` uses `seed.interval == 1`). The battery question no
longer computes `read_battery()` inline — the seed carries it. `run`'s
interactive branch becomes `prompt_answers(themes_dir, &seed_answers(config_path))`.

**Confirm before write.** Add a pure summary helper:

```rust
/// One line per collected answer, for the pre-write confirmation.
fn summarize_answers(a: &InitAnswers) -> String
```

Restructure `run`'s interactive+non-dry-run path so that, after gathering
answers, it:
1. prints `summarize_answers(&answers)`,
2. reuses the existing dry-run machinery (`dry_run_config` + `dry_run_tmux_block`
   + `line_diff`) to print the line-diff of each file against its current
   contents (the "resulting content" dump stays a `--dry-run`-only thing;
   confirm shows only the summary + diffs),
3. asks `ask("Write these changes?", true)`,
4. calls `apply` only on yes; on no prints "Aborted; nothing written." and
   returns.

Scope of the confirm gate: interactive runs only. `--defaults` (scripted,
non-interactive) and `--dry-run`/`--print`/`--uninstall` keep their current
behavior (no confirm). Non-TTY-without-a-flag still errors as today.

### Tests (W33)

- `clock_from_format` round-trips each preset and returns `None` for the
  zero-config default format and for garbage.
- `seed_answers` on a temp config with `battery`/`lan_ip` in the layout,
  `theme.base = "nord"`, and a 12-hour datetime format seeds those four fields;
  a nonexistent path returns a `defaults()`-shaped answer (battery from the
  hardware probe is allowed to be either value — assert the other fields).
- `summarize_answers` contains each answer (theme name, clock label, on/off
  flags, interval).
- The confirm decision is `ask` over the already-tested `parse_yes_no`; a test
  asserts `parse_yes_no("n", true) == false` gates the write (the write-skip is
  driven entirely by that return).

---

## W46 — multiple widget instances

### Config surface (`crates/rustline-core/src/config.rs`)

New top-level table, additive:

```toml
[instances.clock_utc]
kind = "datetime"
timezone = "UTC"
format = "%H:%MZ"

[instances.disk_data]
kind = "disk"
mount = "/data"
```

```rust
// on Config:
#[serde(default)]
pub instances: HashMap<String, toml::Value>,
```

Rationale for `toml::Value` (not a typed `WidgetInstance`): each entry's option
keys are the *per-kind* option set (`DateTimeOpts`, `DiskOpts`, …). Keeping the
raw table and re-parsing it per kind avoids a `#[serde(flatten)]`-into-`Value`
dance and reuses the existing typed Opts structs verbatim. The Opts structs have
no `deny_unknown_fields`, so the extra `kind` key is harmlessly ignored when the
table is `try_into`'d into the kind's Opts. `toml::Value` round-trips through
`print-config` unchanged.

Helpers on `Config`:
- `fn instance_kind(v: &toml::Value) -> Option<&str>` — reads `kind`.
- Extend `color_overrides()` and `click_map()` to also project each instance
  (dispatch on kind → parse the instance table into that kind's Opts → read its
  `.color` / `.alt_format` / `.click`, keyed by the instance name). A single
  `fn instance_meta(kind: &str, v: &toml::Value) -> Option<(ColorOverride, String, ClickBindings)>`
  backs both, so the per-kind match lives in one place.

### Registry (`crates/rustline-core/src/widgets/mod.rs`, `widget.rs`)

- Extract each built-in's construction into a named helper
  `fn build_<kind>(name: &str, o: &<Kind>Opts) -> Box<dyn Widget>` (reused by
  both base registration and instance registration — DRYs the factory bodies).
- Base registration passes the kind name (`"datetime"`); instance registration
  passes the instance name.
- `with_builtins(cfg)` gains a second pass over `cfg.instances`: for each
  `(name, table)`, read `kind`; if unknown or the name already `contains`
  (built-in/earlier instance) → `warn!` + skip (invariant #3); else parse the
  table into the kind's Opts (`try_into().unwrap_or_default()`) and register
  `build_<kind>(name, &opts)` under the instance name with a
  `WidgetDescriptor { source: Builtin, configurable: true, summary: "<kind> instance" }`.
- Instance-name length: names over `RANGE_NAME_MAX_BYTES` (15) still register
  but log a one-time "not click-toggleable" warn (mirrors plugin behavior).

**Instance identity (invariant #7).** The 12 clickable/format-bearing widgets
(`datetime`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`, `memory`, `loadavg`,
`git`, `disk`, `uptime`, `media`, `throughput`) currently hardcode their name in
`range_name()` (`clickable_range("datetime", …)`) and in `active_format(ctx,
"datetime", …)`. Add a `name: String` field to each of those structs; their
`render`/`range_name` use `self.name`. Base instances set `name` to the kind
name (output byte-identical to today); named instances set it to the instance
name, so click-toggle keys, `range=user|NAME`, and `Context.toggled` membership
all use the instance name end-to-end. The non-clickable configurable widgets
(`pane_id`, `hostname`, `cwd`, `windows`) need no `name` field (they emit no
range and don't read `toggled`); they can still be instanced, just never
clickable.

### Per-instance reads (the parameterized kinds)

Most kinds read a *shared singular* `Context` signal and differ only in
formatting/selection (datetime formats `Context.now` per its own timezone; the
IP widgets select from the shared `Context.interfaces` by their own `interface`;
cpu/memory/loadavg/git/uptime/media read one shared reading). Those need **no**
read-layer change.

Two kinds read *parameterized* data — `disk` by `mount`, `throughput` by
`interface` — so two instances with different parameters must read differently:

- Add to `Context` (additive; NOT in `WireContext`):
  ```rust
  pub disks: BTreeMap<String, DiskInfo>,        // keyed by mount
  pub throughputs: BTreeMap<String, Throughput>, // keyed by interface ("" = aggregate)
  ```
  Both `#[serde(default)]`. `Context::default()` gets empty maps.
- The `disk` widget carries its `mount` and reads `ctx.disks.get(&self.mount)`;
  the `throughput` widget carries its `interface` key and reads
  `ctx.throughputs.get(&self.key)` (`key` = the configured interface or `""`).
  `Context.disk`/`Context.throughput` stay populated with the *base*
  `[widgets.disk].mount` / `[widgets.throughput].interface` reading (for the
  WASM mirror + any direct reader); the built-in base widgets read from the maps
  like instances do (base disk `mount = "/"` → `ctx.disks["/"]`).
- **`build_region_context`** becomes instance-aware:
  - New param: the resolved instance→kind info for the region's layout. Pass
    `cfg.instances` (or a precomputed `&HashMap<String,String>` name→kind) so the
    builder can (a) map each layout entry to its kind for read-gating and (b)
    collect the distinct mounts/interfaces to read.
  - Read-gating (`layout_needs`) becomes **kind-aware**: a read fires if any
    layout entry is that kind (base name, or an instance whose `kind` matches).
  - Disk: collect the set of distinct `mount`s across all disk-kind entries in
    the layout (base uses `[widgets.disk].mount`; each disk instance uses its own
    `mount`, default `/`), `read_disk` each, fill `ctx.disks`. Set `ctx.disk` to
    the base mount's entry.
  - Throughput: same for distinct `interface`s; fill `ctx.throughputs`; set
    `ctx.throughput` to the base interface's entry.
  - Spark-format gating stays as-is for cpu/memory (singular; no instance
    multiplicity — cpu/memory read one shared value even if instanced).

- **Per-interface throughput sample files.** `read_throughput` currently
  persists one `<state_root>/throughput-sample`. Multiple interfaces would
  clobber each other, so key the sample file by interface: `throughput-sample`
  for the aggregate (None), `throughput-sample-<sanitized-iface>` for a named
  interface (sanitize `/` etc. so it's a safe filename). Reserve these names
  alongside the existing host-owned state files (doc note). Disk needs no
  persisted sample (statvfs is instantaneous).

### assemble.rs

`render_named_region` already keys color-overrides off the `(name, widget)`
pairs `resolve` returns, so instance color overrides work once
`color_overrides()` includes instances. No structural change beyond that.

### Tests (W46)

- Config: `[instances]` parse; `instance_kind`; `print-config` round-trip
  preserving an instance table; absent `[instances]` → empty map.
- Registry: two `datetime` instances with different timezones render distinctly;
  unknown-`kind` instance is skipped (not fatal); an instance name colliding with
  a built-in is skipped; instance `range_name()` returns the *instance* name; an
  instance with a non-empty `alt_format` toggles under its own name (seed
  `Context.toggled` with the instance name → alt view renders).
- Reads: two `disk` instances with different mounts populate `ctx.disks` with
  both and each widget renders its own mount; kind-aware `layout_needs` fires the
  disk read when only an instance (not the base `disk` name) is in the layout;
  throughput per-interface sample files don't clobber (two interfaces each get a
  Some on the second pass).
- Byte-identical: a config with no `[instances]` produces identical
  `render_named_region` output to before the feature (characterization).

---

## W48 — optional persistent daemon (opt-in)

### Modules (`crates/rustline/src/`)

- `daemon_proto.rs` — wire types + framing, pure and unit-tested:
  ```rust
  enum DaemonRequest {
      Render { region: RegionKind, args: RenderArgsWire },
      Ping,
      Shutdown,
  }
  enum RegionKind { Left, Right, Window }
  struct RenderArgsWire { /* session, window, pane, pane_path (left/right);
                             index, name, flags, current (window) */ }
  enum DaemonResponse { Markup(String), Pong, ShuttingDown }
  fn write_frame<W: Write>(w, &T: Serialize)  // u32-LE length prefix + JSON
  fn read_frame<R: Read, T: DeserializeOwned>(r) -> io::Result<T>
  ```
- `daemon.rs` — the server:
  - `daemon_socket_path()` → `$XDG_RUNTIME_DIR/rustline/daemon.sock`, fallback
    `<rustline_wasm::state_root()>/daemon.sock`.
  - `serve(config_path, plugin_dir_flag)` — bind `UnixListener` (unlink a stale
    socket first), print the reload/keep-alive hint, then accept-loop; each
    connection handled on its own thread against a shared
    `Arc<Mutex<DaemonState>>`. A `Render` request: lock state, reload if
    `config.toml` mtime changed, build `Context` (same builders), render (same
    `render_named_region`/`render_window`), reply `Markup`. `Ping` → `Pong`.
    `Shutdown` → reply `ShuttingDown`, break the loop, remove the socket, exit.
  - `DaemonState { config, theme, plugin_dir, registry, config_mtime }`, with
    `fn reload_if_changed(&mut self, config_path)` re-parsing config, re-resolving
    the theme, and rebuilding the registry (`with_builtins` + `register_plugins`
    over the union of both regions' layouts, so plugins are warm for either
    side). Rebuild only happens on an mtime change; the common case reuses warm
    state including the `Arc<Mutex<Plugin>>` WASM instances.
  - Renders serialize behind the state `Mutex` (fine — renders are fast; tmux
    refresh concurrency is low). This also satisfies `extism::Plugin`'s
    `!Sync`-across-threads constraint: only one thread renders at a time.
- `daemon_client.rs` — the client:
  - `fn try_render(region: RegionKind, args: RenderArgsWire) -> Option<String>`:
    if the socket file doesn't exist → `None` immediately (one `stat`, ~zero
    overhead when the daemon is off). Else connect (with a short read/write
    timeout), `write_frame` the request, `read_frame` the response, return the
    markup. `None` on ANY error.

### CLI (`crates/rustline/src/cli.rs`, `main.rs`)

- `Command::Daemon(DaemonCmd)` with `DaemonCmd { Run(DaemonRunArgs), Status, Stop }`
  → `rustline daemon` (== `run`, foreground; `--plugin-dir` override like render),
  `rustline daemon status` (connect + `Ping`; prints running/not-running, exit
  code reflects it), `rustline daemon stop` (connect + `Shutdown`).
- The three `Render` arms in `main.rs` become:
  ```rust
  match daemon_client::try_render(region, args_wire) {
      Some(markup) => emit(&markup, preview),
      None => { /* existing in-process path, verbatim */ }
  }
  ```
  `--preview` is applied client-side by `emit` in both cases (the daemon always
  returns raw markup), so preview works with or without the daemon.

### Docs / ops

- README gains a "Daemon (optional)" section: what it caches, the `daemon
  run|status|stop` commands, an example systemd user unit and a tmux `run-shell`
  line, and the "clients fall back to in-process automatically" guarantee.
- `doctor` gains one advisory check: whether a daemon socket is present +
  reachable (never a failure — purely informational).
- CLAUDE.md: new modules, the `daemon` command group, the socket path, and the
  reserved `daemon.sock` state name.

### Tests (W48)

- `daemon_proto`: `write_frame`→`read_frame` round-trip for each request/response
  variant; a truncated/garbage frame is an `Err` (never a panic).
- `daemon_client::try_render`: with no socket file → `None`; against a fake
  in-test `UnixListener` that replies with a known frame → returns that markup.
- Server: `reload_if_changed` rebuilds when the file mtime advances and is a
  no-op otherwise; a `serve` fixture (bound to a tempdir socket, builtins-only,
  no wasm) answers a `Render{Right}` request with markup byte-identical to a
  direct in-process `render_named_region` over the same fixture `Context`
  inputs; a `Ping` gets `Pong`; `Shutdown` stops the loop and removes the socket.
- Hermeticity: daemon tests bind a socket under a tempdir and use builtins only,
  so `just test` needs no wasm toolchain.

---

## Sequencing (for the plan)

1. **W33** — self-contained in `init.rs`; no dependency on the others.
2. **W46** — core config/registry/read changes; touches `Context`, `config.rs`,
   `widgets/*`, `build_context.rs`, `assemble` consumers.
3. **W48** — reuses the (now instance-aware) render path; new bin modules + CLI.
4. **Docs + memory** — sync `CLAUDE.md` + `README.md` widget/CLI/config lists and
   the WHATS-NEXT strip, per the standing "update doc lists when adding a
   widget/feature" rule.

Each feature is a run of TDD tasks (tests first). W46's per-instance-read work
(`Context.disks`/`throughputs`, kind-aware gating, per-interface sample files) is
sequenced as its own task after the instance-registration mechanism lands, so it
is a contained, separately-reviewable unit.
