# CLI reference

A global `-v`/`--verbose` (repeatable) raises the **file** log level:
`-v`=warn, `-vv`=info, `-vvv`=debug, `-vvvv`=trace. Works in any position
(`rustline -vv render left`). A global `--config <path>` overrides the
config-file path for every subcommand that reads or writes it (default:
`$XDG_CONFIG_HOME/rustline/config.toml`, falling back to
`~/.config/rustline/config.toml`).

`--plugin-dir` overrides plugin discovery for that invocation (flag › config
`plugin_dir` › `rustline_wasm::default_plugin_dir()`).

`--preview` prints a region in ANSI colour on the terminal (for manual
verification) instead of raw tmux markup; without it, stdout is the raw markup
tmux consumes (stdout is the status line — logs always go to stderr).

| File | Commands |
|---|---|
| [`render-and-widget.md`](render-and-widget.md) | `render left\|right\|window\|windows`, `widget list\|enable\|disable\|move\|edit` |
| [`init-and-config.md`](init-and-config.md) | `init`, `print-config`, `config path\|edit\|validate`, `doctor`, `completions` |
| [`plugin.md`](plugin.md) | `plugin list\|search\|url\|path\|write-path\|cmd\|new\|approve\|build\|run\|install\|update\|remove\|denials` |
| [`theme-click-daemon.md`](theme-click-daemon.md) | `theme list\|show\|use\|new\|pick`, `click`, `daemon run\|status\|stop\|install\|uninstall` |
| [`bench.md`](bench.md) | `bench` (feature-gated) |
