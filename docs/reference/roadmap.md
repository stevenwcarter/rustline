# Roadmap (open items)

> **Read when:** planning new work, to avoid re-proposing something already shipped or already known-and-deferred.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- Per-widget richer customization; naming the widget in the panic-guard `warn!`.
- Range-on-binding — today a `run`/`open_url` click binding only fires on a
  widget that already emits a clickable range (i.e. has a non-empty
  `alt_format`); making any widget with a configured binding emit a range would
  let a `run`/`open_url` binding fire without a throwaway `alt_format`.

