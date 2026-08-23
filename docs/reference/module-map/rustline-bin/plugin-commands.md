# `rustline` bin — plugin commands

> **Read when:** changing `plugin install/update/remove`, the curated plugin index and `plugin search`, or any other `rustline plugin …` subcommand.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- `plugin_install.rs` — `plugin install/update/remove` (W38): a `Downloader`
  seam (`UreqDownloader`, rustls, redirect-limited, User-Agent) over the GitHub
  releases API, pure `parse_owner_repo`/`select_wasm_asset`. `sha256_hex`
  itself now lives in `rustline-wasm::integrity` (W19); this module re-exports
  it (`pub use rustline_wasm::sha256_hex;`) so existing `plugin_install::
  sha256_hex` call sites keep resolving, rather than keeping its own
  duplicate copy as before. Install downloads a repo's `.wasm` into the plugin
  dir and records `source`/`tag`/`checksum` — granting **no** capabilities
  (TOFU at write time: it records the hash of what it downloaded, doesn't
  verify against an external pin; that recorded digest IS verified later, at
  load time, by `register_plugins` — see `integrity.rs`/Config below). Absent
  `--name`, `do_install`'s default installed name is the selected release
  **asset's own stem**, NOT the repo name (branch
  `feat/2026-07-30-standalone-plugin-repos`): the installed `.wasm` stem must
  equal the plugin's exported `name()` for `register_plugins` to find it at
  load time, so a repo following the `rustline-<name>` convention (repo
  `rustline-<name>`, release asset `<name>.wasm`) now installs under `<name>`
  correctly instead of the un-findable `rustline-<name>` the old repo-name
  default produced.
- `plugin_index.rs` — the curated plugin index (W49) backing `rustline
  plugin search`: the wire types (`PluginIndex { schema_version, plugins:
  Vec<IndexEntry> }`, `IndexEntry { name, description, source, bundled,
  capabilities }` — `capabilities` is advertising copy only, consulted by
  nothing in the host and granting nothing — the committed
  `registry/index.json` now also lists the four standalone `rustline-*`
  repos — `pubip`, `ticker`, `kube`, `updates` — each with `bundled: false`
  and its own `source: "stevenwcarter/rustline-<name>"`, alongside the five
  `bundled: true` in-repo examples, branch
  `feat/2026-07-30-standalone-plugin-repos`), `parse_index_value`/`validate`
  (rejects a `schema_version` this build doesn't understand rather than
  misreading it; checked identically on both the fresh-fetch path and the
  cache-read path, so a cache written by a different-schema build can never
  be served unvalidated), `filter_entries` (case-insensitive substring match
  over name + description; no/blank query returns every entry),
  `index_is_fresh` (24h TTL via `INDEX_TTL_SECS`; a clock that moved
  backwards counts as stale, not fresh forever), `read_cache`/`write_cache`
  (best-effort, atomic, via `sample_store::{read_sample, write_sample}`),
  `load_index` (a fresh cache is served as-is; otherwise fetch, cache, and
  return; a failed fetch falls back to the last-known-good cache flagged
  `stale`, erroring only when nothing is cached at all), and `search_json`/
  `LoadedIndex`. `DEFAULT_INDEX_URL` points at `registry/index.json` on this
  repo's `main` branch (overridable via `Config.plugin_index_url`);
  `INDEX_CACHE_FILE` is `plugin-index.json` under the state root (see the
  host-owned state-file names list under Config below).

- `plugin_cmd.rs` — `rustline plugin …`: `list` reads the effective `Config`;
  `list`/`url|path|write-path|cmd list`/`denials` each take a `--json` flag
  (W40, `plugin_list_json`/`pattern_list_json`/`denials_json`) emitting a
  `serde_json`-pretty array instead of the human text, human output
  unchanged when the flag is absent — `list --json`'s per-plugin object now
  also carries `allowed_write_paths` and `resolve_symlinks` (D3, so a plugin
  holding a write grant can't render as read-only in machine-readable
  output), alongside the existing `allowed_urls`/`allowed_paths`/
  `allowed_commands`; the human `list` output prints `resolve_symlinks` too,
  so the doc comment's "same fields the human list prints" claim stays true.
  `url|path|write-path|cmd add/remove` all share one `pattern_cmd` dispatch
  keyed by a private `Kind { Url, Path, WritePath, Command }` (`Kind::key()`
  -> `"allowed_urls"`/`"allowed_paths"`/`"allowed_write_paths"`/
  `"allowed_commands"`) that mutates the config file in place via `toml_edit`
  (preserving comments/formatting), creating `[plugins.<name>]` if absent;
  `new <name> [--path] [--force]` scaffolds a ready-to-build WASM guest
  plugin crate from embedded templates (`assets/plugin-cargo.toml.tmpl`/
  `plugin-lib.rs.tmpl`, mirroring `init.rs`'s `include_str!` approach):
  `validate_plugin_name` enforces the same `[A-Za-z0-9_-]`/≤15-byte/not-
  `window` rules as a widget's click-toggle range name (invariant #7), and
  the generated `src/lib.rs` deserializes the typed `GuestRender`/
  `WireContext` (not a hand-walked `serde_json::Value`). Refuses to
  overwrite an existing `<name>/` dir without `--force`; prints the
  `cargo build --target wasm32-unknown-unknown` + install step and a
  starter `[plugins.<name>]` config snippet afterward. `approve <name>
  [--yes]` (W24) resolves the plugin's manifest via `resolve_manifest`,
  prints what it requests via `manifest_report` — labeling the read line
  `allowed_paths (read-only)` and, when `requested_write_paths` is
  non-empty, appending its own overwrite-danger banner ("lets this plugin
  overwrite these files with any content"), mirroring the existing warning
  that fires when `requested_commands` is non-empty (that
  `allowed_commands` runs real programs with the user's own environment and
  permissions) — approve only patterns you understand — and, after an
  interactive y/N confirmation (or unconditionally with `--yes`), writes
  **exactly** those requested URL/read-path/write-path/command patterns into
  `[plugins.<name>]`'s allowlists (idempotent append, never a wider grant);
  the "nothing to approve" guard checks all four requested lists, so a
  manifest requesting only `requested_write_paths` is still approvable.
  Both warnings are part of `manifest_report`'s printed text, not the
  confirmation prompt, so they still show up under `--yes` and in captured
  output/logs, not just an interactive session. `list` also now shows a `run
  \`plugin approve <name>\`` hint when a manifest resolves for that plugin.
  Also handles `build` (W31, any
  cdylib crate → `.wasm` in the plugin dir; pure `wasm_artifact_path`/
  `cargo_build_args`/`package_name`), `run` (W34, the read-only dev harness via
  `rustline_wasm::instantiate_named` + `format_run_output`, printing segments
  and captured denials), `denials` (W28, read-only over
  `rustline_wasm::denials::read_denials`), `search` (W49, thin CLI wiring
  only: resolves `cfg.plugin_index_url`/the state root, calls
  `plugin_index::load_index`, filters via `plugin_index::filter_entries`, and
  prints human text or `plugin_index::search_json`; the fetch/parse/cache
  logic itself lives in `plugin_index.rs` above), and delegates
  `install`/`update`/`remove` to `plugin_install.rs`.
