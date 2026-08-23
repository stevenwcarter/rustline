# Config — filesystem and exec capabilities

> **Read when:** changing the read/write path allowlists, the symlink gate, or the `allowed_commands` exec capability and its blast radius.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

**Filesystem capability (`allowed_paths` / `allowed_write_paths` /
`resolve_symlinks`):** `rl_file_read`/`perform_file_read` is gated by
`allowed_paths`, which is **read-only**, and `rl_file_write`/
`perform_file_write` by the separate `allowed_write_paths`, which is
**write-only** — an entry in one never authorizes the other. Both are empty
(deny-by-default) unless populated, same glob-or-`re:` shape as the other
allowlists. **This is a change from before this batch**, when one
`allowed_paths` list authorized both directions: an existing `allowed_paths`
entry now grants read only, and a plugin that was writing through it fails
closed — a denial record plus a log line — rather than silently. Every
read/write request is resolved through `resolve_for_allowlist`
(`rustline-wasm`'s `state.rs`, see above) before either list is consulted,
which also closes a symlink escape: a grant is otherwise a grant over
**names**, and a symlink planted under a granted prefix (an extracted
archive, a synced directory, another tool) would silently redirect the
effect to wherever it points — past the subtree the user thought they'd
granted. By default (`resolve_symlinks = false`) a path with any symlink
component is denied outright, closing that name-vs-subtree gap for good;
setting `resolve_symlinks = true` on a plugin trades that closure away — the
path is canonicalized first (falling back to canonicalizing the parent for a
not-yet-existing write target, so a first write to a granted directory still
works) and the allowlist is matched against the **resolved** location, so a
grant then legitimately follows symlinks out of the directory the user
thought they were granting. Canonicalization failing is a hard denial, never
a silent fall-through to the raw path.

A plugin may also declare a capability **manifest** — a sidecar
`<plugin_dir>/<name>.toml` (or an embedded `rustline-manifest` wasm custom
section) listing `requested_urls`/`requested_paths`/`requested_write_paths`/
`requested_commands` — which `rustline plugin approve <name>` turns into
exactly those allowlist entries above, after confirmation (see CLI above). A
manifest alone grants nothing; only `approve` (or hand-editing the config)
ever widens an allowlist. `plugin approve` prints an explicit
overwrite-danger warning when a manifest requests write paths, and a
separate explicit warning when it requests commands — see the exec-capability
paragraph below.

`source` is a typed `PluginSource` that still accepts a bare `owner/repo`
string; `rustline plugin install <owner/repo>` (W38) downloads the plugin's
`.wasm` from its GitHub release into the plugin dir and records
`source`/`tag`/`checksum` here — but grants **no** capabilities (the same
allowlist-widening rule: run `approve` or `url|path|cmd add` afterward). The
recorded `checksum` is TOFU at write time (trust-on-first-use: it records the
hash of what it downloaded, not verified against any external pin). `plugin
update` re-resolves the latest release for that `source` and refreshes
`checksum`/`tag`.


**Exec capability (`allowed_commands`):** a plugin can ask the host to run a
program on its behalf via `rl_exec` (plain, every render) or `rl_exec_cached`
(TTL-cached, keyed on the whole command line — `cmdrun`'s worked example,
see the Architecture section above), gated by `[plugins.<name>].allowed_commands`
(glob by default, `re:` for regex, same shape as `allowed_urls`/`allowed_paths`,
empty = deny by default). Four properties make this capability's blast radius
explicit rather than implicit:
- **No shell, ever.** The host spawns `program` plus an explicit `args` array
  directly (`std::process::Command`) — nothing re-parses a string, so there is
  no quoting/word-splitting surface. A plugin that wants a shell must be
  granted one explicitly and visibly, e.g. `allowed_commands = ["sh -c *"]`;
  the host never introduces one on its own.
- **The gate matches the whole canonical argv, not just the program.** A grant
  for `git status*` does **not** cover `git push --force` — `rustline_wasm::
  canonical_argv(program, args)` renders `program` + every argument
  (quoting one that contains whitespace/quotes/is empty, so two different
  argv vectors can never collapse onto the same matched string) and the
  *whole* rendered string is what a pattern is checked against.
- **Bounded and capped.** A 5 s wall-clock timeout (under Extism's 10 s plugin
  timeout, so a hung command surfaces as a renderable failure rather than
  killing the whole plugin render), a 64 KiB cap per stdout/stderr stream
  (flagged `truncated` past it, not silently dropped), and stdin closed
  (`Stdio::null()`, so a command that tries to read stdin gets EOF instead of
  hanging). The child runs in its own process group so a backgrounded
  descendant (e.g. `sh -c "long_thing &"`) can't hold the piped output open
  past the timeout and hang the render — and even a descendant that escapes
  that group entirely (`setsid`/`setpgid`) is bounded by a short output-
  collection grace period afterward, so its own output is dropped rather than
  hanging the render (`rustline-wasm::run`'s `OUTPUT_GRACE`).
- **Environment and working directory are inherited from the host process —
  plainly, not as a footnote.** This is why `playerctl`/`git`/etc. work
  without extra plumbing, and it's a real part of what you're granting: an
  approved command sees your shell's environment variables and runs from
  wherever `rustline` itself runs, with your user's permissions. Approve
  `allowed_commands` patterns you actually understand.

Cache semantics for `rl_exec_cached` mirror `rl_http_get_cached`'s shape with
one deliberate difference: only a **zero-exit** run is written to the cache,
and a **non-zero exit is returned immediately as fresh data** (the command
genuinely ran and reported that status — it isn't a transient failure to
paper over with stale data); only a run that couldn't happen at all (denied,
spawn failure, or timeout) falls back to the last-good cached entry with
`stale: true`.

