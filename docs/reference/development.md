# Development

> **Read when:** running builds/tests/lints, adding a `just` recipe, changing CI, bumping dependencies, or touching `Cargo.lock`.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- **`just`** recipes: `just build`, `just test` (hermetic — no wasm toolchain
  needed), `just lint`, `just check-lock` (verifies all **six** committed
  `Cargo.lock` files — the workspace plus each of the five excluded
  `plugins/*` — are up to date, via `cargo metadata --locked`, which resolves
  without compiling and so runs in about a second; deliberately its own recipe
  rather than `--locked` bolted onto `test`/`lint`, so a local `just test`
  keeps auto-refreshing the lock as before), `just preview` (colour preview via `cargo run --`, live
  tmux context when inside tmux, else samples — needs a Nerd/powerline font for
  the glyphs), `just build-plugin NAME` (builds `plugins/<NAME>` for
  `wasm32-unknown-unknown` and installs `<NAME>.wasm` into the plugin dir —
  generic across all five example plugins, e.g. `just build-plugin cmdrun`),
  `just build-weather` (an alias: `build-plugin "weather"`), `just test-wasm`
  (opt-in: builds the weather plugin, then runs the feature-gated
  `rustline-wasm` e2e test, the bin's `wasm_wiring` test, and the bin's
  `plugin_build_wasm` test (the four `plugin build` tests that cargo-compile
  a throwaway crate for `wasm32-unknown-unknown`, moved out of `smoke.rs` so
  the default suite doesn't need that target) — needs the wasm target; `just
  test` never requires it), `just bench [ARGS]` (builds the
  weather plugin, then runs the real `rustline bench` tool via `cargo run
  --release --features bench -- bench {{ARGS}}`), `just lint-plugins` (the
  five excluded `plugins/*` are invisible to the root `cargo fmt`/`clippy`/
  `cargo test --workspace`, so this loops over them: `cargo fmt --check` +
  host clippy + a **wasm32** clippy pass — load-bearing, not redundant, since
  each plugin's guest code sits behind `#[cfg(target_arch = "wasm32")] mod
  guest`, invisible to a host-only pass), `just test-plugins` (per-plugin
  `cargo test` over the same five, 22 tests total). Both share one `plugins`
  justfile variable so the two loops can't drift out of sync when a plugin
  is added.
- Toolchain: Rust 1.97, **edition 2024** in every crate (incl. `rustline-abi`
  and the excluded `plugins/weather`, `plugins/counter`, `plugins/filewatch`,
  `plugins/httpget`, `plugins/cmdrun`); `rustfmt.toml` is edition 2024. Keep
  all crate editions equal to `rustfmt.toml`.
- `ratatui` `0.30` (default features, pulling `ratatui-crossterm` so
  `ratatui::crossterm` re-exports crossterm — no separate direct crossterm
  dependency) backs the `rustline widget edit` interactive editor
  (`widget_tui.rs`). Deliberately **not** feature-gated (unlike `bench`):
  `rustline init` emits the `prefix + W` `display-popup` binding
  unconditionally, so the subcommand it points at (`widget edit`) must
  always exist in every build, not just an opt-in one. `unicode-width` (also
  already in the tree via `ratatui-core`, so no new node) measures the
  preview strip's display width — byte or `char` length would both misjudge
  the powerline/Nerd Font glyphs the bar is made of.
- Must stay **clippy-clean** (`cargo clippy --all-targets -- -D warnings`) and
  **rustfmt-clean** (`cargo fmt --all --check`). There is **no pre-commit hook**
  in this repo — run `cargo fmt --all` yourself before committing.
- **CI** (`.github/workflows/ci.yml`, greenfield — the repo had no CI before
  this branch): runs on every pull request and on a push to `main`, cancelling
  a superseded run on the same ref. **Five** independent jobs. Four are a thin
  wrapper around an existing `just` recipe so the gate and the documented
  local command can't drift apart: `lint` → `just lint`, `test` → `just test`,
  `plugins` → `just lint-plugins` + `just test-plugins`, and `wasm-e2e` →
  `just test-wasm`. The fifth, **`macos-check`**, is the deliberate exception:
  it runs `cargo check --workspace --all-targets` on `macos-latest` directly,
  NOT through a `just` recipe, because there is no equivalent command a
  developer on Linux could run — a recipe would be a local no-op pretending to
  be a check. It exists because every other job is ubuntu-only, so nothing in
  CI ever compiled the `#[cfg(target_os = "macos")]` arms of `battery.rs`,
  `cpu.rs` (mach FFI), `memory.rs` (`sysctlbyname` FFI), `uptime.rs` or
  `daemon_service.rs` (launchd); the only macOS build anywhere was
  `release.yml`'s `aarch64-apple-darwin` leg on a `v*` tag push, so a compile
  error in a Mac-gated arm first surfaced at tag time — the same
  discovered-too-late shape `just check-lock` exists to prevent for
  `Cargo.lock`. It is check-only on purpose: it catches the compile break that
  would reach a release, and does NOT prove the FFI behaves correctly at
  runtime, which still needs a real Mac.
  The `test` job runs `just check-lock` ahead of `just test`:
  `release.yml` builds with `--locked` and nothing else did, so before this a
  stale `Cargo.lock` passed every PR check and first failed at tag-push time —
  the most expensive moment to discover it. Toolchain via
  `dtolnay/rust-toolchain` + `Swatinem/rust-cache`; `just` itself via
  `extractions/setup-just`.
  Deliberately no `rust-toolchain.toml` (would change every developer's local
  toolchain resolution) and no workspace-level `RUSTFLAGS: -D warnings`
  (redundant — `just lint`'s own clippy `-D warnings` already enforces the
  zero-warning bar). A `v*` tag push instead triggers
  `.github/workflows/release.yml` — see the Roadmap entry below for what it
  builds/publishes and README's install section for verifying a downloaded
  tarball against `SHA256SUMS`.
- **Actions are pinned to full commit SHAs** in both workflows, each with a
  trailing `# vX.Y.Z` comment, and `.github/dependabot.yml` (github-actions
  ecosystem, weekly) opens PRs to refresh them — a pin nothing refreshes is
  just a frozen unpatched version. Dependabot is deliberately **not** given a
  `cargo` entry: Rust bumps go through the `/upgrade-packages` flow, and cargo
  PRs would bury the action refreshes. One wrinkle worth knowing before
  touching those pins: **`dtolnay/rust-toolchain`'s ref selects the
  toolchain** — each branch (`stable`, `nightly`, `1.90.0`) ships an
  `action.yml` whose `toolchain` input defaults to that branch's own name — so
  its pin is the tip of the **`stable` branch**, and repointing it at a commit
  from another branch would silently change the Rust version with nothing in
  the diff to say so. It publishes no release tags, so dependabot cannot bump
  that one; refresh it by hand. Note this pins the *action code*, not the Rust
  version: `stable` still resolves to whatever stable is on the day a job runs,
  so a new stable release can still redden CI with no repo change.
- Commit `Cargo.lock` alongside any dependency change — `just check-lock` (run
  in CI's `test` job) fails the build if you forget, for the workspace lock and
  all five plugin locks alike.
- Tests are TDD unit tests in each core module (incl. the powerline renderer and
  the ANSI transcoder) plus `crates/rustline/tests/smoke.rs` integration tests.
  `rustline-wasm` adds unit tests for allow-patterns, path sandboxing, and
  quota accounting (denied-case tests are load-bearing, not just the happy
  path); the opt-in `wasm-e2e` feature gates the real-wasm end-to-end tests.
- Follows the user's global Rust defaults in `~/.claude/rust-crate-decisions.md`
  and the `rust-developer` agent (clap, serde, chrono, tracing, thiserror/anyhow).
  **rustls-only, still true with the plugin host**: `rustline-wasm`'s HTTP
  capability uses `ureq` with `default-features = false` + the `tls`/`json`
  features (rustls), and `extism` is built with `default-features = false`
  (its built-in HTTP client is deliberately dropped — `rl_http_get` and
  `rl_http_get_cached` are the only network paths). `cargo tree -i openssl` /
  `-i native-tls` stay empty across the whole graph, including
  `plugins/weather`, `plugins/counter`, `plugins/filewatch`, `plugins/httpget`,
  and `plugins/cmdrun` (`httpget` is the only other example plugin that
  touches the network at all, via the plain `rl_http_get` host fn — still
  rustls under the hood, same as `weather`'s cached path; `cmdrun` touches
  neither TLS nor the network — it spawns a subprocess). The `2.3 MB`
  dynamic binary is
  fine here — the musl/`scratch` Docker policy is for server images, not this
  local CLI. `if-addrs` (host interface enumeration for the IP widgets, in
  `crates/rustline`) is a thin syscall wrapper with no TLS involved, so it
  doesn't disturb this either.

