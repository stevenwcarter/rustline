# rustline

A tmux statusline system written in Rust. Most functionality is baked in
(non-WASM); the architecture reserves a seam for future WASM plugins keyed by
their GitHub repo. Primary target is Linux; the author's own use is the main
driver, but it's meant to be shareable.

## Architecture

Cargo **workspace**, edition 2024, `resolver = "2"`:

- `crates/rustline-core` — the **pure, front-end-agnostic core**. Given a
  `Context` snapshot it produces a tmux-format `String`. No I/O, no environment
  reads at render time. This is deliberately the reuse seam: today's only
  front-end is the CLI, but a future daemon or WASM host would build `Context`s
  from a different source and call the same core unchanged.
- `crates/rustline` — the **tmux front-end binary**. A `clap` CLI that builds a
  `Context` from CLI args + local system reads, calls the core, and prints.
- `crates/rustline-wasm` — **not built yet**; the reserved third member for the
  WASM plugin host.

### Render pipeline

`Context` → each widget's `render(&Context) -> Vec<Segment>` → `assign_palette`
fills segment backgrounds → `render_region` joins segments with powerline
separators into tmux `#[fg=..,bg=..]` markup. For the window list, tmux calls
`render window` once per window and each window is rendered as a self-contained
segment (no palette).

The core types (`Context`, `WindowCtx`, `Segment`, `Style`, `Color`) all derive
`serde::Serialize + Deserialize` **on purpose** — that is the future WASM plugin
ABI. Keep them serializable.

## Module map

`rustline-core`:
- `segment.rs` — `Segment { text, style }`, `Style { fg, bg, bold }`,
  `Color { Named | Indexed(u8) | Rgb(u8,u8,u8) }` (+ `Color::to_tmux()`).
- `context.rs` — `Context` (session/window/pane ids, `pane_current_path`,
  `home`, `hostname`, `loadavg: Option<[f64;3]>`, `now: DateTime<Local>`,
  `window: Option<WindowCtx>`) and `WindowCtx`.
- `render.rs` — `render_region(Direction, &[Segment], &Theme) -> String`, the
  load-bearing powerline renderer (hard `` `` / soft `` `` separators, edge
  blending to `bar_bg`); `Theme` (palette, glyphs, colors) with `Default`.
- `widget.rs` — `Widget` trait and `Registry` (name → factory; `resolve` skips
  unknown widget names with a `warn!`, never errors).
- `widgets/` — the six built-ins: `pane_id`, `hostname`, `windows`, `cwd`,
  `loadavg`, `datetime`, plus `Registry::with_builtins(&Config)` in `mod.rs`.
- `assemble.rs` — `assign_palette`, `render_named_region` (panic-guarded per
  widget via `catch_unwind`), `render_window`.
- `config.rs` — `Config` (TOML): `layout`, `theme`, `widgets`, and a reserved
  `plugins: HashMap<String, toml::Value>` table. `Config::load` is **total**
  (missing/invalid file → `warn!` + defaults); `to_theme()` maps overrides onto
  `Theme::default()`.
- `ansi.rs` — `tmux_to_ansi(&str) -> String`: transcodes the tmux markup we emit
  into ANSI SGR (`colourN` → 256-color, `#rrggbb` → truecolor, named → basic)
  for the `--preview` flag.

`rustline` (bin):
- `cli.rs` — `clap` derive. `render` is a subcommand *group*.
- `build_context.rs` — builds `Context` from args + `gethostname`,
  `libc::getloadavg` (the only `unsafe`, guarded on `n == 3`), `chrono::Local`,
  `$HOME`.
- `tmux_conf.rs` — `init_block(bar_bg, fg)`: the tmux config `rustline init`
  emits.
- `main.rs` — dispatch + the `emit(markup, preview)` helper (raw markup vs ANSI).

## CLI

- `rustline render left|right [--session= --window= --pane= --pane-path=] [--preview]`
- `rustline render window [--current] --index= [--name=] [--flags=] [--preview]`
- `rustline init` — prints the tmux config block (uses `theme.bar_bg`/`fg` for
  `status-style`).
- `rustline print-config` — effective config as TOML.

`--preview` prints a region in ANSI colour on the terminal (for manual
verification) instead of raw tmux markup; without it, stdout is the raw markup
tmux consumes (stdout is the status line — logs always go to stderr).

## tmux integration model

Shell-out per region on `status-interval` (no daemon in v1). `rustline init`
wires `status-left`/`status-right`/`window-status-format` to `#(rustline render …)`
and adds `after-select-pane`/`after-select-window` → `refresh-client -S` hooks
for instant updates.

**Injection safety (critical):** tmux expands `#{…}` inside `#(…)` *before*
`/bin/sh -c` and does not shell-escape. So the `init` block passes every tmux var
as `--flag=#{q:VAR}` — the `#{q:}` modifier escapes it and the `--flag=` form is
empty-safe. Never emit a bare `'#{window_name}'` or `'#{pane_current_path}'`.
This is why `render window` takes named args, not positional. See `tmux_conf.rs`.

## Config

Optional TOML at `~/.config/rustline/config.toml` (or
`$XDG_CONFIG_HOME/rustline/config.toml`). Zero-config works. Default layout:
left = `[pane_id, hostname]`, center = `[windows]`,
right = `[cwd, loadavg, datetime]`. Default datetime format
`"%a < %Y-%m-%d < %H:%M"` (the `<` are literal). Unknown widget names in a layout
are skipped, not fatal.

## Invariants (load-bearing — re-check when touching these)

1. **`Context` is the sole render input.** Widgets read only from `Context`,
   never the environment mid-render (keeps the daemon/WASM path viable). `cwd`
   reads `ctx.home`, not `$HOME`.
2. **`Segment`/`Context`/`Style`/`Color` stay serde-serializable** (the WASM ABI).
3. **`Config::load` is total** — a bad config must never break the bar.
4. **`init` output must be injection-safe** (`#{q:}` + `--flag=` form).
5. **`render_region` puts `segments[0]` leftmost regardless of `Direction`.** The
   caller passes widgets in visual left-to-right order (e.g. `cfg.layout.right`),
   which is not reversed.
6. **`loadavg` is `Option`** — a failed `getloadavg` renders nothing, never fake
   zeros. A panicking widget degrades to empty via the `catch_unwind` guard.

## Development

- **`just`** recipes: `just build`, `just test`, `just lint`,
  `just preview` (colour preview via `cargo run --`, live tmux context when
  inside tmux, else samples — needs a Nerd/powerline font for the glyphs).
- Toolchain: Rust 1.97, **edition 2024** in every crate; `rustfmt.toml` is
  edition 2024. Keep all crate editions equal to `rustfmt.toml`.
- Must stay **clippy-clean** (`cargo clippy --all-targets -- -D warnings`) and
  **rustfmt-clean** (`cargo fmt --all --check`). There is **no pre-commit hook**
  in this repo — run `cargo fmt --all` yourself before committing.
- Commit `Cargo.lock` alongside any dependency change.
- Tests are TDD unit tests in each core module (incl. the powerline renderer and
  the ANSI transcoder) plus `crates/rustline/tests/smoke.rs` integration tests.
- Follows the user's global Rust defaults in `~/.claude/rust-crate-decisions.md`
  and the `rust-developer` agent (clap, serde, chrono, tracing, thiserror/anyhow;
  rustls-only, but this project has no TLS/DB/network). The `2.3 MB` dynamic
  binary is fine here — the musl/`scratch` Docker policy is for server images,
  not this local CLI.

## Roadmap

- WASM plugins (config keyed by `owner/repo`; core types are already the ABI).
- Optional daemon front-end for sub-second / push-driven widgets (the pure core
  is already daemon-ready).
- Per-widget richer customization; naming the widget in the panic-guard `warn!`.

## Design docs

- Spec: `docs/superpowers/specs/2026-07-20-rustline-tmux-statusline-design.md`
- Plan: `docs/superpowers/plans/2026-07-20-rustline-tmux-statusline.md`
