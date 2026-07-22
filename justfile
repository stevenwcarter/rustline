# rustline development tasks. Run `just` (or `just --list`) to see them.

# Show available recipes
default:
    @just --list

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

# Preview the rendered bar in colour (live tmux context inside tmux, else samples)
preview:
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

# Build the example weather WASM plugin and install it into the plugin dir
build-weather:
    #!/usr/bin/env bash
    set -euo pipefail
    rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true
    cargo build --release --target wasm32-unknown-unknown --manifest-path plugins/weather/Cargo.toml
    dest="${XDG_DATA_HOME:-$HOME/.local/share}/rustline/plugins"
    mkdir -p "$dest"
    cp plugins/weather/target/wasm32-unknown-unknown/release/weather.wasm "$dest/weather.wasm"
    echo "installed weather.wasm -> $dest/weather.wasm"
