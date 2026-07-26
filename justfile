# rustline development tasks. Run `just` (or `just --list`) to see them.

# The excluded example plugins (own Cargo.lock, built for wasm32-unknown-unknown).
# One shared list so lint-plugins/test-plugins can't drift out of sync when a
# plugin is added to one loop and forgotten in the other.
plugins := "weather counter filewatch httpget cmdrun"

# Show available recipes
default:
    @just --list

mold-install:
    mold -run cargo install --path crates/rustline

install:
    cargo install --path crates/rustline

# Build the release binary
build:
    cargo build --release

# Run the full test suite
test:
    cargo test --workspace

# Verify every committed Cargo.lock is up to date (workspace + the five
# excluded example plugins, which each carry their own lock).
#
# `release.yml` builds with `--locked`, but `just test`/`just lint` do not --
# so without this gate a stale lock passes every PR check and only fails at
# tag-push time, the one moment it is most expensive to discover. Kept as its
# own recipe rather than adding `--locked` to `test`/`lint` so local runs keep
# their normal auto-refresh behaviour; `cargo metadata` resolves the graph
# without compiling, so this costs seconds.
check-lock:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo metadata --locked --format-version 1 --manifest-path Cargo.toml >/dev/null
    for p in {{plugins}}; do
        cargo metadata --locked --format-version 1 \
            --manifest-path "plugins/$p/Cargo.toml" >/dev/null
    done

# Build the weather plugin and run the end-to-end WASM host tests (opt-in)
test-wasm: build-weather
    cargo test -p rustline-wasm --features wasm-e2e --test e2e
    cargo test -p rustline --features wasm-e2e --test wasm_wiring
    cargo test -p rustline --features wasm-e2e --test plugin_build_wasm

# CI-style checks: formatting and clippy
lint:
    cargo fmt --all --check
    cargo clippy --all-targets -- -D warnings
    # crates/rustline-wasm/tests/e2e.rs and crates/rustline/tests/{wasm_wiring,
    # plugin_build_wasm}.rs are all `#![cfg(feature = "wasm-e2e")]`-gated, so
    # the pass above never compiles them and clippy never checks that code.
    # This pass reaches all three (one `--features` flag applies to every
    # selected package that declares the feature). It only needs a
    # host-target compile — the tests need a prebuilt weather.wasm (or, for
    # plugin_build_wasm, the wasm32 target itself) at *runtime* (see `just
    # test-wasm`), not at lint time.
    cargo clippy --workspace --all-targets --features wasm-e2e -- -D warnings

# Preview the rendered bar in colour (live tmux context inside tmux, else samples)
preview: build
    #!/usr/bin/env bash
    # Manual colour preview of the status bar. Powerline separators need a
    # Nerd/powerline-patched terminal font to show as arrows rather than boxes.
    set -euo pipefail
    rl() { cargo run -q --release -- "$@"; }
    if [ -n "${TMUX:-}" ]; then
        s=$(tmux display-message -p '#{session_name}')
        w=$(tmux display-message -p '#{window_index}')
        p=$(tmux display-message -p '#{pane_index}')
        path=$(tmux display-message -p '#{pane_current_path}')
        echo "context: live tmux (session=$s window=$w pane=$p)"
        left=$(rl render left --preview --session="$s" --window="$w" --pane="$p" --pane-path="$path")
        right=$(rl render right --preview --session="$s" --window="$w" --pane="$p" --pane-path="$path")
        center=""
        fmt=$'#{window_index}\t#{window_name}\t#{window_flags}\t#{window_active}'
        while IFS=$'\t' read -r idx name flags active; do
            if [ "${active:-0}" = "1" ]; then
                center+=$(rl render window --preview --current --index="$idx" --name="$name" --flags="$flags")
            else
                center+=$(rl render window --preview --index="$idx" --name="$name" --flags="$flags")
            fi
        done < <(tmux list-windows -F "$fmt")
    else
        echo "context: sample values (not inside tmux)"
        left=$(rl render left --preview --session=0 --window=1 --pane=0 --pane-path="$HOME/src/rustline")
        right=$(rl render right --preview --pane-path="$HOME/src/rustline")
        center=$(rl render window --preview --current --index=0 --name=editor --flags='*')
        center+=$(rl render window --preview --index=1 --name=shell --flags='')
    fi
    printf 'LEFT   : %s\n' "$left"
    printf 'CENTER : %s\n' "$center"
    printf 'RIGHT  : %s\n' "$right"

# Benchmark the render pipeline (regions, widgets, data sources, plugins).
# Pure passes use a fabricated Context (no OS reads); real-world passes pay the
# real reads incl. read_cpu's ~120ms sample. See `rustline bench --help`.
bench *ARGS: build-weather
    cargo run -q --release --features bench -- bench {{ARGS}}

# Build any plugins/<NAME> WASM plugin and install it into the plugin dir.
# Generic recipe backing the per-plugin ones below (and directly usable for
# a plugin that doesn't have its own alias, e.g. `just build-plugin counter`).
#
# Delegates to the real `rustline plugin build` command (not a raw `cargo
# build` + `cp`) so this recipe exercises the exact same path a user's own
# `plugin build` invocation does -- including its post-build stale-checksum
# check (see plugin_cmd.rs::maybe_refresh_stale_checksum): if a plugin was
# `install`ed with a recorded checksum and this rebuild produces different
# bytes, that check fires here too instead of only when a user runs the
# command by hand. `--release` here is the *plugin crate's* build profile.
build-plugin NAME:
    #!/usr/bin/env bash
    set -euo pipefail
    rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true
    cargo run -q -p rustline -- plugin build plugins/{{NAME}} --release

# Build the example weather WASM plugin and install it into the plugin dir
build-weather: (build-plugin "weather")

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
    for p in {{plugins}}; do
        echo "== $p =="
        cargo fmt --check --manifest-path "plugins/$p/Cargo.toml"
        cargo clippy --manifest-path "plugins/$p/Cargo.toml" --all-targets -- -D warnings
        cargo clippy --manifest-path "plugins/$p/Cargo.toml" --target wasm32-unknown-unknown -- -D warnings
    done

# Run the excluded example plugins' host-side unit tests (their pure logic).
test-plugins:
    #!/usr/bin/env bash
    set -euo pipefail
    for p in {{plugins}}; do
        echo "== $p =="
        cargo test --manifest-path "plugins/$p/Cargo.toml"
    done
