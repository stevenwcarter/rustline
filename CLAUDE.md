# rustline

A tmux statusline system written in Rust. Most widgets are baked in, plus a
real WASM plugin host that runs third-party widgets under a capability-gated
sandbox. Primary target is Linux; the author's own use is the main driver, but
it's meant to be shareable.

**This file is the high-level map. Detailed reference lives in
[`docs/reference/`](docs/reference/README.md)** — read this file first, then
open only the leaf doc for the area you are changing. Every doc opens with a
"Read when" line so you can tell at a glance whether you need it.

## Architecture at a glance

Cargo **workspace**, edition 2024, `resolver = "2"`:

| Crate | Role |
|---|---|
| `crates/rustline-core` | The pure, front-end-agnostic core: `Context` → tmux markup. No I/O at render time. This is the reuse seam. |
| `crates/rustline-abi` | Small **serde-only** crate holding the WASM-boundary wire types. No I/O, no chrono. |
| `crates/rustline` | The tmux front-end **binary**: `clap` CLI, system reads, plugin discovery, printing. |
| `crates/rustline-wasm` | The **WASM plugin host**: Extism/wasmtime, nine host functions, capability gates, zero ambient authority. |
| `crates/rustline-plugin-sdk` | The **guest-side SDK** a WASM plugin depends on. |
| `plugins/` | Five example plugins, each an **excluded** workspace member (own `Cargo.lock`, built for `wasm32-unknown-unknown`). |

→ [`docs/reference/architecture.md`](docs/reference/architecture.md) for what
each crate contains and why the split exists.

### Render pipeline

`Context` → each widget's `render(&Context) -> Vec<Segment>` → `assign_palette`
fills segment backgrounds → `render_region` joins segments with powerline
separators into tmux `#[fg=..,bg=..]` markup. The window list is a separate
path (`render_window_pill` — self-contained rounded pills, not
`render_region`/`assign_palette`). A widget that opts into click-to-toggle
gets its cells wrapped in `#[range=user|NAME]…#[norange]`. WASM plugins
implement the same `Widget` trait via `WasmWidget` and resolve into the same
registry, so they flow through this pipeline unchanged.

The core types (`Context`, `WindowCtx`) live in `rustline-core` (they carry
`chrono`); the output types (`Segment`, `Style`, `Color`) live in
`rustline-abi` and are re-exported. All derive serde **on purpose** — that is
the WASM plugin ABI. Keep them serializable.

→ [`docs/reference/module-map/rustline-core/render.md`](docs/reference/module-map/rustline-core/render.md)

## Invariants (load-bearing)

Short forms only. The full statements — with the reasoning, the attack cases,
and the exemptions behind each — are in
[`docs/reference/invariants.md`](docs/reference/invariants.md). **Read that
file before changing rendering, config loading, tmux output, click identity,
or any WASM capability gate.**

1. **`Context` is the sole render input.** Widgets never read the environment
   mid-render (keeps the daemon/WASM path viable).
2. **The wire types stay serde-serializable and additive.** No
   `deny_unknown_fields`; `ABI_VERSION` gates host↔guest skew.
3. **`Config::load` is total** — a bad config must never break the bar.
4. **`init` output must be injection-safe** (`#{q:}` + `--flag=` form).
5. **`render_region` puts `segments[0]` leftmost** regardless of `Direction`.
6. **A missing reading is `Option`, never a fabricated zero.** A panicking
   widget degrades to empty via the `catch_unwind` guard.
7. **The click-toggle NAME is one identity end-to-end** — the `range=user|`
   markup, tmux's `mouse_status_range`, `rustline click --range`,
   `Context.toggled`, and the widget's own `range_name()` are all the same
   name. Break the chain anywhere and the widget silently stops working.
8. **Segment text is never interpolated into tmux markup unsanitized.**
   `sanitize_text` guards the three renderer sites; a `#` in *content* is a
   live tmux directive, and content is routinely untrusted.

**WASM host, N1–N4:** zero ambient authority (every effect is host-checked;
`rl_log` is the sole intentional exception) · a plugin never breaks the bar
(any error/timeout/malformed output degrades to empty segments) · state writes
are dir-sandboxed and quota-bounded · capability scope is strictly per-plugin.

**Platform-specific reads stay at the `Context`-build edge.** Each
`#[cfg(target_os)]` reader delegates to a pure parser compiled under
`#[cfg(any(target_os = …, test))]`, so both arms are unit-tested on one box.

## CLI at a glance

Global `-v` (repeatable) raises the **file** log level; global
`--config <path>` overrides the config-file path everywhere.

- `render left|right|window|windows` — what tmux shells out to.
- `widget list|enable|disable|move|edit` — the widget manager (`edit` is a
  full-screen `ratatui` editor).
- `init` — onboarding wizard; writes `config.toml` + a managed `~/.tmux.conf`
  block. `print-config`, `config path|edit|validate`, `doctor`, `completions`.
- `plugin …` — list, search, allowlist editing, new/build/run, approve,
  install/update/remove, denials.
- `theme list|show|use|new|pick` · `click` · `daemon run|status|stop|install|
  uninstall` · `bench` (feature-gated).

→ [`docs/reference/cli/`](docs/reference/cli/README.md) for every flag and
every subcommand's exact behavior.

## Config at a glance

Optional TOML at `~/.config/rustline/config.toml`. **Zero-config works**, and
`Config::load` is total (invariant #3). Every option defaults to reproducing
the pre-feature output byte-for-byte.

Default layout: left = `[pane_id, hostname]`, center = `[windows]`,
right = `[cwd, cpu, memory, loadavg, datetime]`.

Top-level shape: `[layout]`, `[theme]`, `[widgets.<name>]`,
`[instances.<name>]` (multiple instances of one widget kind), `[plugins.<name>]`,
`[log]`, plus `plugin_dir` / `plugin_index_url`.

Two things worth knowing before you touch config:

- **`[layout].center` is inert** — nothing in the render pipeline reads it.
  It still round-trips, and this is a deliberate documented limitation, not a
  bug to "fix" without first deciding what a configurable window list means.
- **A plugin allowlist is only ever widened by `plugin approve` or a hand
  edit.** A manifest, an index entry's `capabilities`, and an install all
  grant nothing on their own.

→ [`docs/reference/config/`](docs/reference/config/README.md) for every
option, every widget's placeholders, theme layering, and the plugin
capability/integrity rules.

## tmux integration

Shell-out per region on `status-interval`; `rustline init` writes a managed
`# >>> rustline >>>` block wiring `status-left`/`status-right`/
`window-status-format` to `#(<binary> render …)`, plus mouse and
`prefix + W` bindings. Needs **tmux ≥ 3.1** (`range=user|`), and ≥ 3.2 for the
widget-manager popup.

**Injection safety (critical):** tmux expands `#{…}` inside `#(…)` *before*
`/bin/sh -c` and does not shell-escape. Every tmux var must be passed as
`--flag=#{q:VAR}` — the `#{q:}` escapes it and the `--flag=` form is
empty-safe. Never emit a bare `'#{window_name}'` or `'#{pane_current_path}'`.

→ [`docs/reference/tmux-integration.md`](docs/reference/tmux-integration.md)

## Development

- **`just`** recipes: `just build`, `just test` (hermetic — no wasm toolchain
  needed), `just lint`, `just check-lock`, `just preview`,
  `just build-plugin NAME`, `just test-wasm` (opt-in, needs the wasm target),
  `just lint-plugins`, `just test-plugins`, `just bench`.
- Toolchain: Rust 1.97, **edition 2024 in every crate** — keep all crate
  editions equal to `rustfmt.toml`'s, or `cargo fmt --all` produces permanent
  stray diffs.
- Must stay **clippy-clean** (`cargo clippy --all-targets -- -D warnings`) and
  **rustfmt-clean** (`cargo fmt --all --check`). There is **no pre-commit
  hook** — run `cargo fmt --all` yourself before committing.
- The excluded `plugins/*` are invisible to the root `cargo fmt`/`clippy`/
  `cargo test --workspace`; `just lint-plugins`/`just test-plugins` cover
  them, including a **wasm32** clippy pass (guest code sits behind
  `#[cfg(target_arch = "wasm32")]`).
- Commit `Cargo.lock` alongside any dependency change — all **six** locks (the
  workspace plus each plugin). `just check-lock` runs in CI and fails if you
  forget.
- **rustls-only** across the whole graph: `cargo tree -i openssl` /
  `-i native-tls` must stay empty.
- CI runs five jobs; four wrap a `just` recipe so gate and local command can't
  drift. The fifth, `macos-check`, deliberately does not — nothing else in CI
  compiles the `#[cfg(target_os = "macos")]` arms.

→ [`docs/reference/development.md`](docs/reference/development.md) for the CI
job details, action-pinning rules, and the testing conventions.

## Where to look next

| You are… | Read |
|---|---|
| Finding the file that owns something | [`docs/reference/module-map/`](docs/reference/module-map/README.md) |
| Adding or changing a widget | [`module-map/rustline-core/widgets.md`](docs/reference/module-map/rustline-core/widgets.md) + [`config/widgets-*.md`](docs/reference/config/README.md) |
| Adding or changing a CLI command | [`docs/reference/cli/`](docs/reference/cli/README.md) |
| Adding a config option | [`docs/reference/config/`](docs/reference/config/README.md) |
| Touching a WASM capability gate | [`invariants.md`](docs/reference/invariants.md) + [`module-map/rustline-wasm/`](docs/reference/module-map/rustline-wasm/README.md) |
| Writing or debugging a plugin | [`module-map/example-plugins.md`](docs/reference/module-map/example-plugins.md) + [`module-map/rustline-plugin-sdk.md`](docs/reference/module-map/rustline-plugin-sdk.md) |
| Working on the daemon | [`module-map/rustline-bin/daemon.md`](docs/reference/module-map/rustline-bin/daemon.md) |
| Adding an OS-specific reader | [`module-map/rustline-bin/read-surfaces.md`](docs/reference/module-map/rustline-bin/read-surfaces.md) |
| Planning new work | [`roadmap.md`](docs/reference/roadmap.md) — then [`history-*.md`](docs/reference/README.md) to avoid re-proposing shipped work |
| Asking "why is it built this way?" | [`history-features.md`](docs/reference/history-features.md), [`history-bundles.md`](docs/reference/history-bundles.md), [`design-docs.md`](docs/reference/design-docs.md) |

### Doc maintenance

When you add or remove a **widget** or an **example plugin**, sync the lists in
all of: this file's architecture table (if a crate changed),
[`module-map/rustline-core/widgets.md`](docs/reference/module-map/rustline-core/widgets.md),
the relevant [`config/widgets-*.md`](docs/reference/config/README.md), and
`README.md`. Keep the "Read when" line at the top of any doc you add.
