# Config — the `[plugins.<name>]` table

> **Read when:** adding a plugin config key, changing `plugin_dir`/`plugin_index_url` resolution, or the allow-pattern glob/`re:` syntax.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

**Plugins:** an optional top-level `plugin_dir` (default
`$XDG_DATA_HOME/rustline/plugins`, `~/` expanded), an optional top-level
`plugin_index_url` (W49 — overrides where `rustline plugin search` fetches
the curated plugin index from; default: `registry/index.json` on this
repo's `main` branch, via raw.githubusercontent.com), plus a typed
`[plugins.<name>]` table per plugin, keyed by the plugin's name (the `.wasm`
filename stem):

```toml
plugin_dir       = "~/.local/share/rustline/plugins"     # optional
plugin_index_url = "https://example.com/my-index.json"   # optional; overrides the built-in default

[plugins.weather]
source = "steve/rustline-weather"          # owner/repo; consumed by `plugin update`
allowed_urls = ["https://wttr.in/*"]        # glob, or "re:<pattern>" for regex
allowed_paths = []                          # READ-only; see the filesystem-capability paragraph below
allowed_write_paths = []                    # WRITE-only, globs; empty = deny by default
resolve_symlinks = false                    # default; see below for what `true` trades away
allowed_commands = []                       # glob/"re:", matched against the WHOLE argv (see below)
max_state_bytes = 52428800                  # default: 50 MB
# tag = "v1.2.0"                            # recorded by `plugin install/update`
# checksum = "<sha256-hex>"                 # recorded by `plugin install/update`; VERIFIED at load time (W19, see below)

[plugins.weather.options]
zip = "48183"
format = "{icon} {temp_f}°F"
refresh_secs = 1800
```

Every `PluginConfig` field is `#[serde(default)]`, so the whole table stays
covered by invariant #3. `options` is an opaque TOML table forwarded to the
plugin's `render` call verbatim. Allow-pattern entries are a **glob** by
default (matched against the full URL/path string), or a **regex** when
prefixed `re:` — regex entries are **anchored to a full-string match** (uniform
with globs); to match a prefix/substring, include `.*` in the pattern (e.g.
`re:https://wttr\.in/.*`).

