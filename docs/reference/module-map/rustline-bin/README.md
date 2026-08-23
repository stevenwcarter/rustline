# Module map — `rustline` (the binary)

The tmux front-end: a `clap` CLI that builds a `Context` from CLI args + local
system reads, discovers/registers WASM plugins, calls the core, and prints.

| File | Read when |
|---|---|
| [`cli-and-render-path.md`](cli-and-render-path.md) | Argument parsing, `Context` construction and its read gates, toggle state, click dispatch, `main.rs` dispatch. |
| [`read-surfaces.md`](read-surfaces.md) | Adding or changing an OS-specific reader (battery, cpu, memory, git, disk, uptime, media, throughput, windows) or the persisted sample/history helpers. |
| [`daemon.md`](daemon.md) | The daemon wire protocol, client fallback, server state/reload, or the systemd/launchd service installer. |
| [`plugin-commands.md`](plugin-commands.md) | `plugin install/update/remove`, the curated index and `plugin search`, or any other `rustline plugin …` subcommand. |
| [`widget-theme-commands.md`](widget-theme-commands.md) | `widget list/enable/disable/move`, the interactive `widget edit` TUI, or any `theme …` subcommand. |
| [`init-and-tmux-conf.md`](init-and-tmux-conf.md) | The onboarding wizard, its re-run seeding, or the managed `~/.tmux.conf` block. |
| [`logging-and-bench.md`](logging-and-bench.md) | The tracing subscriber, log rotation, cross-process warn dedup, or the bench passes. |
