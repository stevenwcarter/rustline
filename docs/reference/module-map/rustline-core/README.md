# Module map — `rustline-core`

The pure, front-end-agnostic core: given a `Context` snapshot it produces a
tmux-format `String`. No I/O, no environment reads at render time.

Read the leaf file that matches what you are touching:

| File | Read when |
|---|---|
| [`context.md`](context.md) | Adding a field to `Context`, changing the render input, or the re-exported wire types (`segment.rs`, `context.rs`). |
| [`render.md`](render.md) | Powerline rendering, window pills, markup sanitization (invariant #8), clickable range names (invariant #7), built-in themes, region assembly. |
| [`widgets.md`](widgets.md) | Adding or changing a built-in widget, the `Widget` trait / `Registry`, widget instances, or the shared bar/spark/alert helpers. |
| [`config.md`](config.md) | Adding a config option, `Config` parsing, `[instances.*]`, the pure layout algebra, or the `resolved_instances` memo. |
| [`support.md`](support.md) | Transcoding tmux markup (`ansi.rs`), atomic file writes, or `warn_once`-deduped diagnostics. |
