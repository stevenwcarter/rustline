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
