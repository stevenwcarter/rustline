# Config — plugin checksum verification

> **Read when:** touching the load-time checksum gate, `plugin install/update/build` checksum recording, or reasoning about what this gate does and does not defend against.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

**The recorded `checksum` is verified at load time (W19) — it is no longer
"recorded, never checked."** `register_plugins` (`rustline-wasm::integrity::
verify_checksum`) hashes a discovered `.wasm`'s bytes and compares them
against `[plugins.<name>].checksum` right after reading the file and before
the plugin ever reaches `CapabilityCtx`/wasmtime (invariant N1: a rejected
plugin never receives a capability object). Three outcomes: an **absent** (or
blank) checksum loads exactly as before — nothing recorded means nothing to
verify, so this is fully backward compatible; a well-formed digest that
**mismatches** the bytes on disk is skipped with a `warn!` (a swapped or
tampered `.wasm` can no longer load silently); and a **malformed** value —
anything other than 64 hex chars, tolerating an optional `sha256:` prefix and
surrounding whitespace — **fails closed**, also skipped, on the reasoning
that a checksum we can't even parse is a verification request we cannot
honor, not evidence the plugin is unpinned. `rustline plugin run`
(`instantiate_named`) is the one deliberate exception: it's **warn-only** on
a bad verdict — it logs and still runs the plugin — since that dev harness is
used right after rebuilding a plugin, where a recorded digest legitimately
mismatches every build; the bar and the daemon (which rebuilds its registry
via this same `register_plugins` on a config-file mtime change) both go
through the real, non-bypassable gate. `plugin list` is not gated by this at
all — a checksum-failing (or entirely missing) plugin still appears in the
list exactly like a healthy one, with its full allowlists — but it now
*surfaces* the same read for visibility: both the human and `--json` output
include a per-plugin checksum status (`unpinned`/`verified`/`mismatch`/
`malformed`/`missing`, via `plugin_checksum::status_for` — the same
read-and-verify helper backing `doctor`'s "plugin checksums" row; see the CLI
section above). Purely informational: `list` reads the `.wasm` bytes to
compute it, but the verdict never changes what gets listed.

**Rebuilding an installed plugin's stale checksum is handled by `plugin
build` itself, not left as a hazard.** `checksum` is written **only** by
`plugin install`/`plugin update` (grep confirms `plugin_install.rs` is the
sole writer of that field) — `plugin build` (and `just build-plugin`, which
now routes through it) just installs the freshly built `.wasm`. But after a
successful install, `build_plugin` calls `plugin_cmd.rs`'s
`maybe_refresh_stale_checksum`: if that plugin has a recorded checksum and
the rebuilt bytes no longer verify (`!ChecksumVerdict::allows_load()`), it
offers to fix it right there — on a TTY, `confirm_checksum_refresh` asks
`Update the recorded checksum? [Y/n]` (Enter means yes, unlike `plugin
approve`'s no-defaulting confirm, since this just re-pins a hash to bytes
the user built themselves rather than granting a new capability); `--yes`
(`BuildArgs::yes`) does the same non-interactively.
On a **non-TTY** run without `--yes` — the CI/script case — it deliberately
does **neither**: no prompt, no rewrite, just `stale_checksum_notice`'s
printed reminder, so a compromised local toolchain can't silently bless its
own rebuilt output by re-pinning the checksum on its own. It stays quiet
whenever no checksum is recorded or the bytes already match. For that
non-TTY case, or a `plugin build` run before this refresh existed, the manual
recovery still works: clear `[plugins.<name>].checksum` (opts that plugin
back out of verification) or, if the plugin has a recorded `owner/repo`
`source`, run `rustline plugin update <name>` — though that re-downloads the
published release and overwrites your local rebuild rather than hashing it,
so it's only the right recovery when you're fine reverting to what's
published.

**What checksum verification does and does not defend against.** It catches
accidental swaps, truncated/corrupt downloads, stale build artifacts, and a
plugin dir writable by a different principal than the config — genuinely
useful failure modes. It is **not** a privilege boundary: anyone who can write
`~/.local/share/rustline/plugins/*.wasm` can equally edit
`~/.config/rustline/config.toml` and delete the `checksum` line, and there is
no strict/"require checksum" mode that would stop them. It is also **not** a
pin against a compromised upstream: `plugin install`/`plugin update` both
*rewrite* the digest from whatever they just downloaded (trust-on-first-use;
see above) — a poisoned release is recorded and then verified against itself,
not caught.

