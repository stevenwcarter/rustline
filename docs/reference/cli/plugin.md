# CLI — `plugin`

> **Read when:** changing any `rustline plugin …` subcommand: list, search, allowlist editing, new/build/run, approve, install/update/remove, denials.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- `rustline plugin list [--json]` — discovered/configured plugins with their
  source, allowlists, state quota, and checksum status; `--json` emits a JSON
  array of `{name, source, tag, allowed_urls, allowed_paths,
  allowed_write_paths, resolve_symlinks, allowed_commands, max_state_bytes,
  has_manifest, checksum_status}` instead of human text (the human path
  prints the same values, incl. `resolve_symlinks`, as `checksum: `/other
  lines per plugin). `allowed_paths` is **read-only**; `allowed_write_paths`
  (globs, empty = deny) is the separate write allowlist — see the Config
  section's filesystem-capability paragraph above. `checksum_status` is one
  of `"verified"`/`"unpinned"`/
  `"mismatch"`/`"malformed"`/`"missing"` (`plugin_checksum::
  PluginChecksumStatus`, the same read-and-verify helper backing `doctor`'s
  "plugin checksums" row above) — read-only and purely informational: it
  reports what `register_plugins`' real load-time gate would decide, but
  doesn't gate `list` itself, so a mismatching/malformed/missing plugin still
  appears with its full allowlists.
- `rustline plugin search [QUERY] [--json] [--refresh]` — browse the curated
  plugin index (W49) for widgets you can install; omitting `QUERY` lists
  every entry, otherwise a case-insensitive substring filter over name and
  description. Fetches `plugin_index_url` (default: `registry/index.json` on
  this repo's `main` branch) into a 24h-TTL cache at
  `<state_root>/plugin-index.json`; `--refresh` bypasses the cache. A fetch
  failure serves the last-known-good cached copy with a staleness warning on
  stderr (so `--json`'s stdout stays clean); a fetch failure with nothing
  cached is a hard error. Each row marks whether the plugin is already
  installed (via `discover_plugin_names`, without instantiating anything)
  and shows a `just build-plugin <name>` hint for a bundled entry or
  `rustline plugin install <owner/repo>` for an installable one. `--json`
  follows the repo's existing convention (a pretty-printed array). Strictly
  read-only — writes neither config nor the toggles file.
- `rustline plugin url|path|write-path|cmd list [--json]|add|remove <plugin>
  [pattern]` — read or edit a plugin's `allowed_urls`/`allowed_paths`
  (read)/`allowed_write_paths` (write)/`allowed_commands` (`add`/`remove`
  rewrite the config file in place via `toml_edit`, preserving
  comments/formatting); `list --json` emits the patterns as a JSON array of
  strings (`[]` for an absent/empty plugin) instead of one per line.
  `write-path` is `path`'s write-only counterpart — a grant in one never
  authorizes the other (see the Config section's filesystem-capability
  paragraph above). `cmd` is the exec capability's counterpart to
  `url`/`path`: its patterns are matched against the whole canonical argv
  (`rustline_wasm::canonical_argv`), not just the program name.
- `rustline plugin new <name> [--path <dir>] [--force]` — scaffold a
  ready-to-build WASM guest plugin crate at `<dir or cwd>/<name>/`
  (`Cargo.toml` with an empty `[workspace]` table + edition 2024 + cdylib,
  and a `src/lib.rs` skeleton using the typed `GuestRender`/`WireContext`),
  and print the build/install step plus a starter `[plugins.<name>]` config
  snippet. `<name>` must be `[A-Za-z0-9_-]`, ≤15 bytes, and not `window`
  (same rule as a widget's click-toggle range name); refuses to overwrite an
  existing `<name>/` directory without `--force`.
- `rustline plugin approve <name> [--yes]` — resolve `<name>`'s declared
  capability manifest (a sidecar `<name>.toml` in the plugin dir, or an
  embedded `rustline-manifest` wasm custom section), print what it requests —
  labeling the read line `allowed_paths (read-only)`, and including, when the
  manifest asks for `requested_write_paths`, an explicit overwrite-danger
  warning, and when it asks for `requested_commands`, an explicit warning
  that granting `allowed_commands` runs real programs with the user's own
  environment and permissions (both part of the printed report, so they show
  up under `--yes` too, not just an interactive prompt) — and, after an
  interactive y/N confirmation (or unconditionally with `--yes`), write
  exactly those requested URL/read-path/write-path/command patterns into
  `[plugins.<name>]`'s allowlists; declines (writing nothing) without
  confirmation, and does nothing if the plugin has no manifest or if it
  requests no capabilities at all.
- `rustline plugin build <dir> [--release] [--plugin-dir <d>] [--yes]` —
  build any WASM guest plugin crate (any `cdylib`-for-`wasm32-unknown-unknown`
  crate, not just this repo's `plugins/*`) and install the resulting `.wasm`
  into the plugin dir. Errors (never panics) on a missing crate/artifact.
  After a successful install, if that plugin has a recorded
  `[plugins.<name>].checksum` that no longer matches the freshly built bytes,
  it offers to refresh it: on a TTY it asks `Update the recorded checksum?
  [Y/n]` (Enter means yes — unlike `plugin approve`'s confirm, which defaults
  to no, since this is just re-pinning a hash to bytes you built yourself,
  not a new capability grant); `--yes` refreshes non-interactively (for
  scripts/CI). On a **non-TTY** run without `--yes`, it deliberately does
  **not** prompt and does **not** rewrite the checksum — it prints a notice
  and leaves the stale digest in place, since silently re-pinning in a
  non-interactive context would let a compromised local toolchain bless its
  own output. Stays silent whenever no checksum is recorded or the bytes
  already match. `just build-plugin` (which routes through this command)
  surfaces the same prompt/notice.
- `rustline plugin run <name> [--plugin-dir <d>]` — dev harness: instantiate
  one plugin, render it against a fabricated sample `Context`, and print its
  segments plus any capability denials it triggered. Read-only — touches
  neither config nor the toggles file.
- `rustline plugin install <owner/repo> [--name <n>] [--tag <t>] [--plugin-dir
  <d>]` — download a plugin's `.wasm` from its GitHub release into the plugin
  dir and record `source`/`tag`/`checksum` in `[plugins.<name>]`, granting
  **no** capabilities (run `approve` or `url|path add` afterward).
- `rustline plugin update <name> [--plugin-dir <d>]` — re-resolve the latest
  release for a recorded `owner/repo` source, re-download, and refresh the
  recorded `checksum`/`tag`.
- `rustline plugin remove <name> [--yes] [--plugin-dir <d>]` — delete an
  installed plugin's `.wasm`; with `--yes` also drop its `[plugins.<name>]`
  config entry.
- `rustline plugin denials <name> [--json]` — list a plugin's persisted
  capability denials (every distinct `(kind, target)` it was actually denied,
  recorded by the host's `FileDenialObserver` in `<data_root>/denials.jsonl`).
  Read-only; `--json` emits a JSON array of `{kind, target}` instead of human
  text.
