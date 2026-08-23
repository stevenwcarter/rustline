# rustline reference docs

Detail split out of `CLAUDE.md` (repo root) so each topic can be loaded on
demand. `CLAUDE.md` holds the high-level map and the pointers into here; this
index is the same map with one more line of description per entry.

## Start here for a change

| Area | Docs |
|---|---|
| Orienting / adding a crate | [`architecture.md`](architecture.md) |
| **Before and after any render, config, tmux-output, or WASM change** | [`invariants.md`](invariants.md) |
| Finding the file that owns something | [`module-map/`](module-map/README.md) |
| Adding or changing a CLI command | [`cli/`](cli/README.md) |
| Adding or changing a config option | [`config/`](config/README.md) |
| tmux wiring, bindings, injection safety | [`tmux-integration.md`](tmux-integration.md) |
| Builds, tests, lints, CI, dependencies | [`development.md`](development.md) |

## Background

| Doc | Read when |
|---|---|
| [`roadmap.md`](roadmap.md) | Planning new work — the short list of known-open items. |
| [`history-features.md`](history-features.md) | You need rationale for an early standalone feature (widgets, themes, bench, wizard, manifests). |
| [`history-bundles.md`](history-bundles.md) | You need rationale for a multi-item bundle, or which branch introduced a feature. |
| [`design-docs.md`](design-docs.md) | You need the full long-form spec/plan behind a feature. |
