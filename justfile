# rustline development tasks. Run `just` (or `just --list`) to see them.

# The excluded example plugins (own Cargo.lock, built for wasm32-unknown-unknown).
# One shared list so lint-plugins/test-plugins can't drift out of sync when a
# plugin is added to one loop and forgotten in the other.
plugins := "weather counter filewatch httpget cmdrun"

# Show available recipes
default:
    @just --list

mold-install: build-weather
    mold -run cargo install --path crates/rustline

install: build-weather
    cargo install --path crates/rustline

# Build the release binary
build:
    cargo build --release

# Run the full test suite
test:
    cargo test --workspace

# Build the weather plugin and run the end-to-end WASM host tests (opt-in)
test-wasm: build-weather
    cargo test -p rustline-wasm --features wasm-e2e --test e2e
    cargo test -p rustline --features wasm-e2e --test wasm_wiring

# CI-style checks: formatting and clippy
lint:
    cargo fmt --all --check
    cargo clippy --all-targets -- -D warnings
    # crates/rustline-wasm/tests/e2e.rs and crates/rustline/tests/wasm_wiring.rs
    # are `#![cfg(feature = "wasm-e2e")]`-gated, so the pass above never compiles
    # them and clippy never checks that code. This pass reaches both (one
    # `--features` flag applies to every selected package that declares the
    # feature). It only needs a host-target compile — the tests need a
    # prebuilt weather.wasm at *runtime* (see `just test-wasm`), not the
    # wasm32 target at lint time.
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
build-plugin NAME:
    #!/usr/bin/env bash
    set -euo pipefail
    rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true
    cargo build --release --target wasm32-unknown-unknown --manifest-path plugins/{{NAME}}/Cargo.toml
    dest="${XDG_DATA_HOME:-$HOME/.local/share}/rustline/plugins"
    mkdir -p "$dest"
    cp plugins/{{NAME}}/target/wasm32-unknown-unknown/release/{{NAME}}.wasm "$dest/{{NAME}}.wasm"
    echo "installed {{NAME}}.wasm -> $dest/{{NAME}}.wasm"

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
