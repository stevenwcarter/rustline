# rustline whats-next bundle #2 — design spec

Date: 2026-07-23
Status: approved (brainstorm), ready for plan

A second whats-next `--execute` bundle: 12 features plus one time-boxed
feasibility spike, selected from `WHATS-NEXT.md` and handed off as one combined
session. Executed as **one phased spec** (the same shape as the prior 22-item
bundle), dependency-ordered into seven phases.

## Selected items

| ID  | Title                                        | Lens             | Phase |
|-----|----------------------------------------------|------------------|-------|
| W37 | Uptime widget                                | feature-gap      | 1     |
| W30 | Per-widget timezone (second clock)           | feature-gap      | 1     |
| W41 | Now-playing / media widget                   | feature-gap      | 2     |
| W35 | Global `--config` path override              | ux               | 3     |
| W29 | Explicit per-widget color override           | feature-gap      | 3     |
| W36 | Configurable click bindings                  | unblock-debt     | 4     |
| W32 | Host/guest ABI version negotiation           | plugin-ecosystem | 5     |
| W31 | Generic `plugin build <dir>`                 | plugin-ecosystem | 5     |
| W34 | Local `plugin run` dev harness               | plugin-ecosystem | 5     |
| W39 | `rustline-plugin-sdk` guest crate            | plugin-ecosystem | 5     |
| W28 | Capability denial-observation seam           | unblock-debt     | 6     |
| W38 | Plugin install by `owner/repo`               | plugin-ecosystem | 6     |
| W43 | WASM compiled-module cache (**spike only**)  | scale-perf       | 7     |

## Global principles (apply to every item)

1. **Byte-identical at defaults.** Every new config option defaults such that
   existing output is reproduced byte-for-byte. New widgets are opt-in (not in
   the default layout).
2. **TDD.** Pure parsers/resolvers get unit tests first. Invariants that a
   feature depends on are pinned with a test at the seam they cross, never left
   to prose (per the repo's spec-discipline rule).
3. **rustls-only stays true.** `cargo tree -i openssl` and `-i native-tls` must
   remain empty across the whole graph. Any new HTTP path uses `ureq` with
   `default-features = false` + `tls` (rustls).
4. **Edition 2024** for any new crate; `rustfmt.toml` parity preserved.
5. **Clippy/fmt clean;** `cargo clippy --all-targets -- -D warnings` and
   `cargo fmt --all --check` pass.
6. **Docs synced last.** The final task updates the widget/plugin/CLI/config
   lists in **both** `CLAUDE.md` and `README.md` (standing rule).
7. **`Cargo.lock` committed** with every dependency change.
8. **Context-build reads stay gated** by `layout_needs` (the cpu/memory/git/disk
   pattern); a new platform read follows the `#[cfg(target_os = …, test)]`
   pure-parser-behind-a-read-surface shape.

---

## Phase 1 — Widgets & datetime

### W37 — Uptime widget

**What.** A `uptime` built-in showing humanized system uptime.

**Design.**
- New read surface `crates/rustline/src/uptime.rs`: `read_uptime() -> Option<u64>`
  (seconds since boot). Linux arm parses `/proc/uptime` (first float);
  macOS arm shells `sysctl -n kern.boottime` and subtracts from now. Each arm
  delegates to a pure parser (`parse_proc_uptime`, `parse_kern_boottime`)
  compiled under `#[cfg(any(target_os = …, test))]` and unit-tested on Linux.
  Any other platform / failed read → `None`.
- New `rustline-abi` field is **not** needed as a struct — uptime is a scalar.
  Add `Context.uptime: Option<u64>` (seconds) directly to `context.rs`
  (chrono-free `u64`, `#[serde(default)]`).
- Gated read in `build_context.rs`: `let uptime = if layout_needs(layout,
  "uptime") { crate::uptime::read_uptime() } else { None };`.
- Widget `crates/rustline-core/src/widgets/uptime.rs`: pure over `Context.uptime`.
  Config `UptimeOpts { format: String (default "{uptime}"), down_format,
  alt_format }`. `{uptime}` renders a humanized duration via a pure
  `humanize_uptime(secs) -> String` (e.g. `3d 4h`, `4h 12m`, `12m`, `<1m`);
  format rules: show the two largest non-zero units among d/h/m, drop m when
  days present, etc. — pin exact strings with tests. `down_format` when `None`.
  Opt-in, click-toggleable (register in `toggle`-aware family only if
  `alt_format` present).
- Register in `widgets/mod.rs::with_builtins`; **bump the registry-count test
  from 13** (it becomes 14 after this item; 15 after W41).

**Tests.** `parse_proc_uptime` (valid, extra whitespace, garbage → None-ish via
Option), `parse_kern_boottime`, `humanize_uptime` across buckets, widget render
(value present, `None` → down_format, format substitution, unknown placeholder
passthrough).

**Invariants depended on.** #1 (widget reads only `Context.uptime`), #6
(`Option` — a failed read renders nothing, never fake `0`).

### W30 — Per-widget timezone (second clock)

**What.** Let `datetime` render a zone other than `Local`.

**Design.**
- Add `chrono-tz` (default features, or the smallest feature set that yields the
  IANA `Tz` `FromStr`) to `rustline-core`'s deps.
- `DateTimeOpts.timezone: Option<String>` (`#[serde(default)]`, IANA name).
- In `datetime` render: if `timezone` is `Some(name)` and `name.parse::<Tz>()`
  succeeds, format `ctx.now.with_timezone(&tz)`; otherwise format `ctx.now`
  (Local). An **unparseable** zone → `warn!` once + fall back to `Local`
  (config-total spirit; must not break the bar).
- `None` (default) = `Local` = **byte-identical** to current output.

**Tests.** Render with a fixed `ctx.now` and `timezone = "UTC"` vs `None`
(assert the hour differs correctly / matches Local); unparseable zone falls back
to Local (and does not panic). Because `ctx.now` is `DateTime<Local>`, tests set
a fixed instant and assert the converted wall-clock string.

**Invariants depended on.** #1, #3 (bad config value tolerated).

---

## Phase 2 — Now-playing / media widget

### W41 — Media widget

**What.** A `media`/`nowplaying` built-in showing the current track.

**Design — shell-out, no D-Bus dependency.**
- New read surface `crates/rustline/src/media.rs`: `read_media() -> Option<MediaInfo>`.
  Linux arm shells `playerctl metadata --format
  '{{artist}}\t{{title}}\t{{status}}'` (single call), parses the tab-separated
  line via a pure `parse_playerctl(&str) -> Option<MediaInfo>`. `None` when
  `playerctl` is absent (spawn error), exits non-zero (no player), or the line
  is empty/malformed. Non-Linux → `None`. Follows the git.rs shell-out +
  pure-parser pattern; **adds no new Rust dependency** (deliberately not
  zbus/dbus — a heavy async stack — and a plugin cannot reach playerctl since
  there is no such host capability, so a built-in is the only option).
- `rustline-abi`: `MediaInfo { artist: String, title: String, status: String }`
  (chrono-free, `Serialize + Deserialize`, re-exported by `rustline-core`).
- `Context.media: Option<MediaInfo>` (`#[serde(default)]`); gated read on
  `layout_needs(layout, "media")`.
- Widget `crates/rustline-core/src/widgets/media.rs`: `MediaOpts { format
  (default "{title} — {artist}" or similar; finalize in impl), down_format,
  alt_format }`; `{artist}/{title}/{status}` placeholders; `down_format` (default
  `""`) when `None`. Opt-in, click-toggleable. Register + bump count test (→ 15).

**Tests.** `parse_playerctl` (well-formed, missing fields → empty strings,
empty input → None); widget render (present, `None` → down_format, placeholder
substitution). The shell-out itself is not unit-tested (same as git's
`read_git`), only the pure parser.

**Invariants depended on.** #1, #6.

---

## Phase 3 — Config / CLI surface

### W35 — Global `--config <path>` override

**What.** Point any command at an alternate config file.

**Design.**
- Add `#[arg(long = "config", global = true)] config: Option<PathBuf>` to the
  top-level `Cli` (alongside `--verbose`).
- Introduce `effective_config_path(flag: &Option<PathBuf>) -> PathBuf` in
  `main.rs` (flag › `config_path()`); `config_path()` stays the default
  resolver. Resolve **once** in `main` and thread the `PathBuf` into every
  subcommand path that currently calls the bare `config_path()` — dispatch,
  `print-config`, `config path|edit|validate`, the `plugin`/`theme` mutators,
  and the render path's `Config::load_reporting`.
- No behavioral change when the flag is absent.

**Tests.** Integration (smoke): `--config <tmpfile> print-config` reflects the
alternate file; `config path` prints the overridden path; a `plugin url add`
with `--config` mutates the alternate file, not the default.

**Invariants depended on.** #3 (a bad `--config` file still degrades via
`Config::load`'s total load).

### W29 — Explicit per-widget color override

**What.** Pin a widget's segment fg/bg from config instead of only the
auto-cycled palette color.

**Design.**
- Config: per-widget optional `fg: Option<Color>` and `bg: Option<Color>`.
  To avoid duplicating two fields across ~15 opts structs, embed a shared
  `#[serde(default, flatten)] color: ColorOverride { fg, bg }` in each
  format-bearing widget's opts (or a single shared newtype). All `Option`,
  `#[serde(default)]` → invariant #3 coverage.
- Apply **centrally in `assemble`**, not inside widgets (keeps invariant #1:
  widgets read only `Context`). The binary projects config into a
  `HashMap<String, ColorOverride>` (layout-name → override) and passes it to
  `render_named_region`; after a widget renders its `Vec<Segment>` and **before**
  `assign_palette`, apply the override: set `bg` only on segments that don't
  already carry an explicit bg (matching `assign_palette`'s existing skip rule),
  set `fg` unconditionally where specified. This uniformly covers plugin widgets
  too (keyed by name).
- Default (no override) = palette cycling exactly as today = byte-identical.

**Tests.** `assign_palette`/apply-override unit test: a widget with `bg` set is
left untouched by `assign_palette`; an override map sets bg/fg on the named
widget's segments and not others; empty map = unchanged output (byte-identical
characterization test against a known region render).

**Invariants depended on.** #1 (override applied post-render in assemble, not in
the widget), #5 (segment order unchanged).

---

## Phase 4 — Configurable click bindings

### W36 — Click-binding dispatch

**What.** Replace `run_click`'s hardwired `button==left → toggle` with a
config-driven per-widget, per-button action resolver.

**Design.**
- Action model — a typed enum:
  - `toggle` (existing behavior; the default action for a widget with a
    non-empty `alt_format` and left button, preserving today's semantics),
  - `open_url = "<url>"` (spawn the OS opener — `xdg-open` on Linux, `open` on
    macOS),
  - `run = "<shell cmd>"` (execute via `/bin/sh -c`; the user's own config, so
    their own commands — opt-in per binding, not remote input).
- Config surface: `[widgets.<name>]` per-button bindings, e.g.
  `left_click = { toggle = true }` / `right_click = { run = "…" }` /
  `middle_click = { open_url = "…" }`. Finalize the exact TOML shape in impl
  (a small `ClickBinding` map keyed by button name); every field
  `#[serde(default)]`.
- Widen `run_click` (the acknowledged single choke point, `main.rs:138`) into
  `resolve_click(cfg, range, button) -> ClickAction` + a dispatcher. Default
  resolution when no binding is configured: `left` on a toggleable widget →
  toggle (today's behavior, byte-identical); anything else → no-op.
- Range/name identity (invariant #7) is unchanged: the `--range` value keys both
  the toggle set and the new binding lookup.
- Safety: `run`/`open_url` are executed detached and best-effort; a failed spawn
  is `warn!`-logged and never breaks click handling. No shell string is built
  from tmux-provided data — the command comes from the user's config, the
  `--range` only selects which binding.

**Tests.** `resolve_click` unit tests: no config + left + toggleable → Toggle;
no config + right → NoOp; configured `run`/`open_url`/`toggle` per button
resolve correctly; unknown button → NoOp. The actual process spawn is behind a
seam (a `ClickExecutor` trait) so dispatch is tested without spawning.

**Invariants depended on.** #4 (click binding still injection-safe — the tmux
binding passes `--range=#{q:…}`; the command text is config-owned), #7.

---

## Phase 5 — Plugin dev experience & ABI

### W32 — Host/guest ABI version negotiation

**What.** An explicit version handshake so an incompatible guest warns instead
of silently rendering empty.

**Design.**
- `rustline_abi::ABI_VERSION: u32` (start at `1`).
- Add `abi_version: u32` to `RenderInput` (host → guest), so a guest *can* read
  it.
- Host checks a guest `abi_version()` export at registration, alongside the
  existing `name()` check (`lib.rs:66`):
  - export **missing** → legacy guest: one-time `info!`, register anyway
    (existing plugins like `weather` don't export it — must not break them),
  - export **present and == `ABI_VERSION`** → register,
  - export **present and != `ABI_VERSION`** → `warn!` (naming the plugin and
    both versions) + skip.
- `WasmWidget`/`build_plugin` unchanged otherwise.

**Tests.** Host-side unit test of the version-decision function (missing /
match / mismatch → register-legacy / register / skip). e2e (behind `wasm-e2e`)
only if cheap.

**Invariants depended on.** N2 (a skipped/incompatible plugin never breaks the
bar).

### W31 — Generic `rustline plugin build <dir>`

**What.** Build any guest crate for wasm32 and install it, from the CLI.

**Design.**
- New subcommand `plugin build <dir> [--release] [--plugin-dir <dir>]`:
  shells `cargo build --target wasm32-unknown-unknown [--release]` in `<dir>`,
  locates the produced `*.wasm` under
  `<dir>/target/wasm32-unknown-unknown/{release,debug}/`, and copies it to the
  resolved plugin dir as `<stem>.wasm`. A missing wasm target or non-zero cargo
  exit → clear error (non-zero exit), not a panic.
- Complements the existing generic `just build-plugin NAME` recipe (which only
  covers in-repo `plugins/<NAME>`); this CLI path handles arbitrary external
  crate dirs.

**Tests.** Argument/plumbing unit tests where practical (artifact-path
resolution given a fake target dir); the cargo invocation itself is integration
territory and may be left to manual/`wasm-e2e`.

### W34 — Local `plugin run <name>` dev harness

**What.** Render one plugin outside tmux and see its output + denials.

**Design.**
- New subcommand `plugin run <name> [--plugin-dir <dir>]`: resolve config +
  plugin dir, instantiate the single named plugin (`build_plugin` +
  `register_plugins`-style path for just that name), build a **sample**
  `Context` (a fabricated one, reusing the bench fixture shape or a small local
  builder), call `render`, and print the returned `Vec<Segment>` (debug/pretty)
  plus **any denied capability attempts** captured via W28's denial seam during
  the render.
- Read-only; never writes config.

**Tests.** The Context-fabrication + segment-formatting helper is unit-tested;
the wasm instantiation is `wasm-e2e`/manual.

**Dependency.** Consumes W28's seam for the denial listing → W28 lands first (or
the seam is introduced in this phase and W28 builds its recorder on it). Order:
W28's seam type before W34's use; the persisted recorder (W28) can follow.

### W39 — `rustline-plugin-sdk` guest crate

**What.** A guest-side SDK that removes the copy-the-weather-plugin boilerplate.

**Design.**
- New workspace crate `crates/rustline-plugin-sdk` (edition 2024, `cdylib`-free
  library; buildable for `wasm32-unknown-unknown`), depending on `rustline-abi`
  by path.
- Provides:
  - typed `extern` declarations + safe Rust wrappers for all seven host fns
    (`rl_http_get`, `rl_http_get_cached`, `rl_state_read`, `rl_state_write`,
    `rl_file_read`, `rl_file_write`, `rl_log`), each returning a `Result` over
    the `rustline-abi` wire result types,
  - re-exports of `GuestRender`/`WireContext`/`Segment`/`Style`/`Color` so a
    guest imports one crate,
  - a toggle-select helper (the `active_format`/`select_*` logic the weather
    example hand-rolls),
  - an `export_plugin!` macro that wires the `name()`, `render()`, **and**
    `abi_version()` exports (the latter emitting `rustline_abi::ABI_VERSION`,
    tying into W32).
- **Migrate all four example plugins** (`weather`, `counter`, `filewatch`,
  `httpget`) onto the SDK as the proof it works and to de-dup their glue.
  `plugin new`'s scaffold template is updated to depend on the SDK and use
  `export_plugin!`.
- The SDK's pure logic (wrappers, toggle helper, macro-generated shape) is
  unit-tested on the host target, mirroring how the plugins test pure logic.

**Tests.** Host-target unit tests for the wrappers' encode/decode and the
toggle helper; a compile-and-run check of at least one migrated plugin under
`wasm-e2e`.

**Dependency.** Builds on W32 (`ABI_VERSION` + `abi_version()`), so W32 lands
first.

---

## Phase 6 — Capability observability & distribution

### W28 — Capability denial-observation seam

**What.** Record `(plugin, requested url/path)` when a capability is denied.

**Design.**
- Introduce a **denial-observation seam**: a `DenialObserver` (trait or boxed
  callback) held on `CapabilityCtx` (`capability.rs`). `perform_http_get`,
  `perform_http_get_cached`, `perform_file_read`, `perform_file_write` call
  `ctx.observe_denial(kind, target)` at each deny site (where `ctx.name` is
  already in scope) **before** returning `ok:false`. `perform_*` stay pure —
  the observer is injected, defaulting to a no-op in unit tests.
- Default recorder (wired in `host.rs`/`build_plugin`): append a **deduped**
  denial record (plugin name, kind ∈ {url,path}, target, first-seen) to a file
  under the data dir (e.g. `<data_root>/denials.jsonl` or a per-plugin file);
  dedup on `(plugin, kind, target)`. Best-effort I/O (a write failure `warn!`s,
  never breaks the render — same discipline as the toggles file).
- New CLI `rustline plugin denials <name>` lists that plugin's recorded
  denials; `plugin approve <name>` prints a hint pointing at it when denials
  exist. (Approve still only writes exactly what a manifest declares — denials
  inform the user, they do not auto-widen anything.)

**Tests.** Deny-path tests assert the observer is invoked with the right
`(kind, target)` for each of the four `perform_*` functions (extends the
existing load-bearing denied-case tests); dedup logic unit-tested; the
persisted-record read/write round-trips.

**Invariants depended on.** N1 (adding observation does not change the
gate-first, zero-ambient-authority behavior — the call is purely a side-channel
before the existing `ok:false` return), N4 (the observer sees only that
plugin's `ctx.name`).

### W38 — Plugin install by `owner/repo`

**What.** Obtain a plugin by name instead of by hand.

**Design.**
- **Dedicated bytes downloader in the bin** — the existing
  `rustline-wasm::Fetcher` is String-body + `redirects(0)` and cannot fetch
  GitHub's redirecting binary release assets. Add `ureq` (rustls:
  `default-features = false`, `features = ["tls", "json"]`) + `sha2` to
  `crates/rustline`. A small `plugin_install` module with a `Downloader` seam
  (trait) so resolution/verification logic is testable without network.
- `PluginConfig.source`: promote from inert `Option<String>` to a typed
  `PluginSource` behind a resolver seam — `owner/repo` | `url` | `path`
  (serde-compatible; a bare string still deserializes as `owner/repo` for
  back-compat). Add `checksum: Option<String>` (sha256 hex) and a resolved
  `tag`/`version` note field.
- New subcommands:
  - `plugin install <owner/repo> [--name <n>] [--tag <t>] [--plugin-dir <d>]`:
    query `api.github.com/repos/<owner>/<repo>/releases/{latest|tags/<t>}`
    (rustls GET, JSON), pick a `.wasm` asset, download it (following redirects)
    to the plugin dir, compute its sha256, and write `[plugins.<name>]` with
    `source = "<owner/repo>"`, resolved `tag`, and `checksum`.
  - `plugin update <name>`: re-resolve `latest` for the recorded source,
    re-download, update `checksum`/`tag`.
  - `plugin remove <name>`: delete the installed `.wasm` and (prompt or
    `--yes`) remove the `[plugins.<name>]` entry.
- **Install grants no capabilities.** The downloaded wasm is still fully
  capability-gated at runtime (`allowed_urls`/`allowed_paths` + `plugin
  approve`). Verify-on-load (W19) is a separate, unselected item — this records
  the checksum/pin so a later W19 can verify it, but does not itself verify at
  load.
- rustls-only invariant: after adding `ureq`/`sha2`, `cargo tree -i openssl`
  must still be empty.

**Tests.** Pure unit tests for: GitHub release-JSON asset selection (given a
sample JSON, pick the right `.wasm`), `owner/repo` parsing/validation, sha256
computation, and the `toml_edit` config write (source/tag/checksum land in
`[plugins.<name>]`). The network fetch is behind the `Downloader` seam; a fake
downloader drives the install flow end-to-end without a socket.

**Invariants depended on.** #3 (config write preserves formatting via
`toml_edit`, like existing mutators), rustls-only.

---

## Phase 7 — Spike: WASM compiled-module cache (W43)

**What.** A time-boxed feasibility investigation, **not** a committed feature.

**Design.**
- Investigate whether Extism 1.x exposes any precompiled/serialized-module
  loading usable across the **cold-spawn-per-refresh** model: examine
  `CompiledPlugin`/`Plugin` APIs, whether wasmtime's `Module::serialize`/
  `deserialize` is reachable, and whether a disk-persisted precompiled artifact
  is possible without bypassing Extism's capability-gated host.
- Deliverable: a short feasibility finding appended to this spec's follow-up /
  the roadmap:
  - **If feasible:** a minimal prototype behind a seam in `host.rs` (hash the
    `.wasm` bytes → cache the precompiled artifact under the state dir → load it
    on the next cold spawn), with `rustline bench --cold` before/after numbers.
  - **If infeasible:** record precisely why and what would unblock it (the
    daemon front-end W48, which keeps instances warm, or an upstream Extism
    precompile API), and leave W43 in `WHATS-NEXT.md` annotated.
- No production code merges from this phase unless the prototype proves out and
  passes review.

---

## Cross-cutting work (final tasks)

1. **Registry-count test** updated for the two new widgets (13 → 15).
2. **Docs:** `CLAUDE.md` (module map, CLI, Config, Invariants, Roadmap) and
   `README.md` updated for: the `uptime` and `media` widgets; `datetime`
   `timezone`; per-widget `fg`/`bg`; click bindings; ABI versioning; `plugin
   build`/`run`/`install`/`update`/`remove`/`denials`; the `--config` global;
   the new `rustline-plugin-sdk` crate. Mark the corresponding Roadmap items
   Done and strip shipped entries from `WHATS-NEXT.md` (per its self-maintaining
   rule) at branch-finish.
3. **Dependency review:** `cargo tree -i openssl` / `-i native-tls` empty;
   `Cargo.lock` committed; new crate edition 2024.
4. **Full green:** `just test` (hermetic), `cargo clippy --all-targets -- -D
   warnings`, `cargo fmt --all --check`. `just test-wasm` where guest changes
   (W32/W39) warrant it.

## Out of scope (explicitly not in this bundle)

- W19 checksum **verify-on-load** (W38 records the checksum; verification is
  separate and unselected).
- W48 daemon front-end (referenced by the W43 spike as the real unblock).
- The other unchecked `WHATS-NEXT.md` items (W2, W17, W18, W25, W33, W40, W42,
  W44–W49).

## Invariants this bundle depends on (pin with tests, don't assume)

- **#1** widgets read only `Context` — W29 applies color post-render in assemble,
  W37/W41 add gated `Context` fields.
- **#3** `Config::load` total — W30/W36/W29/W38 all add config that must degrade
  gracefully.
- **#4** injection-safe click path — W36 keeps command text config-owned, tmux
  vars still `#{q:}`-escaped.
- **#6** `Option` reads — W37/W41 render nothing on a failed read, never fake
  values.
- **#7** click-toggle name identity — W36's binding lookup keys on the same
  range name.
- **N1** zero ambient authority — W28 observes denials as a pure side-channel
  before the existing `ok:false`, changing no gating.
- **N2** a plugin never breaks the bar — W32 skips/legacy-registers without
  panicking.
- **N4** per-plugin capability scope — W28's observer sees only `ctx.name`.
