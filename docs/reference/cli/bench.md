# CLI — `bench`

> **Read when:** changing the feature-gated benchmark subcommand or adding a new bench pass.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- `rustline bench [--only regions|widgets|sources|plugins|daemon|all] [--iters N]
  [--real-iters N] [--warmup N] [--cold] [--format table|markdown]
  [--output FILE] [--plugin-dir DIR] [--state-dir DIR]` — feature-gated
  (`--features bench`) benchmark of the render pipeline: a pure pass
  (fabricated `Context`, no reads) vs a real-world pass (real reads + render),
  plus per-widget, per-read, and per-plugin timing. Plugin passes run against
  real preserved state so cached fast-paths are measured honestly. `--only
  daemon` (included in `all`) benches `left`/`right` in-process (cold,
  re-instantiating any WASM plugins) against a real round-trip to a reachable
  `rustline daemon run` — skipped with a note (never spawning one) when no
  daemon is reachable; see `bench/daemon.rs` above.
