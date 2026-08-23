# Invariants (load-bearing)

> **Read when:** touching rendering, `Context`, config loading, tmux output, click/toggle names, or anything in the WASM host. Re-check these before and after any such change.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

1. **`Context` is the sole render input.** Widgets read only from `Context`,
   never the environment mid-render (keeps the daemon/WASM path viable). `cwd`
   reads `ctx.home`, not `$HOME`.
2. **`Segment`/`Context`/`Style`/`Color` stay serde-serializable** — this is
   the WASM ABI. `Segment`/`Style`/`Color` now live in `rustline-abi`
   (re-exported by `rustline-core`); `Context`/`WindowCtx` stay in
   `rustline-core` (they carry `chrono`). `rustline_abi::ABI_VERSION` (W32,
   currently `1`) versions this wire contract: the host stamps it onto every
   `RenderInput`, and `abi_decision` **skips** a guest that declares a
   *different* version (a guest with no `abi_version` export still registers as
   legacy). Keep the wire types **additive** — no `deny_unknown_fields`, so an
   older guest keeps deserializing a newer `Context` (this is why W54 could
   mirror `uptime`/`media`/`throughput`/`cpu_history`/`mem_history` into
   `WireContext` as a plain field addition, breaking nothing). W46's
   `Context.disks`/`Context.throughputs` maps (the per-instance disk/
   throughput readings) are the only fields still NOT mirrored into
   `WireContext`; a WASM guest still sees only the
   base `Context.disk`/`Context.throughput` singular reading, unaffected by
   how many `disk`/`throughput` instances exist in the layout).
3. **`Config::load` is total** — a bad config must never break the bar.
4. **`init` output must be injection-safe** (`#{q:}` + `--flag=` form).
5. **`render_region` puts `segments[0]` leftmost regardless of `Direction`.** The
   caller passes widgets in visual left-to-right order (e.g. `cfg.layout.right`),
   which is not reversed.
6. **`loadavg` is `Option`** — a failed `getloadavg` renders nothing, never fake
   zeros. A panicking widget degrades to empty via the `catch_unwind` guard.
7. **The click-toggle NAME is one identity end-to-end.** The range name
   `render_region_ranged` emits (`#[range=user|NAME]`), tmux's
   `#{mouse_status_range}`, the `--range` value `rustline click` receives, the
   `Context.toggled` key, and a widget's/plugin's own `range_name()`/
   `active_format` key must all be the *same* layout/registry name. Break that
   chain anywhere and the widget silently stops being clickable or
   toggleable — there's no error, just a click that does nothing. W46
   (multiple widget instances) extends this: for a named `[instances.<name>]`
   entry, that identity chain runs on the **instance** name, not its `kind` —
   each of the twelve clickable widget structs now carries its own `name:
   WidgetName` field precisely so `render`/`range_name`/`active_format` all key
   off the instance name instead of a hardcoded kind literal. This also
   means an instance can never be allowed to shadow a built-in of the same
   name (it would silently steal that built-in's click/toggle identity) —
   `Registry::with_builtins` skips a colliding instance outright (built-in
   wins), and `Config::color_overrides()`/`click_map()`/`layout_kinds` apply
   the same `WidgetKind::parse(name).is_some()` precedence check so a colliding
   instance's config can't hijack the built-in's projected color/click
   binding either (a W46 review-caught bug — see `config.rs` above).
8. **Segment text is never interpolated into tmux markup unsanitized.** tmux
   parses `#[...]` in the output of a `#()` job — that is exactly how this
   renderer colours the bar — so a `#` in *content* is a live directive, and
   content reaching the renderer is routinely external and untrusted: a tmux
   window name, a git branch from a cloned repo, an MPRIS now-playing title, a
   directory name, or a WASM guest's `Segment.text`. `render.rs`'s private
   `sanitize_text` doubles `#` (tmux's documented literal-`#` escape) and
   replaces C0 control characters, and it is applied at the **three and only
   three** sites that write segment text: `render_region`,
   `render_region_ranged`, and `render_window_pill`. Without it a
   zero-capability plugin can emit `#[norange]#[range=user|cpu]`, forge another
   widget's clickable range, and have `rustline click` dispatch that widget's
   configured `run =` binding through `sh -c` — authority `CapabilityCtx` never
   granted (N1) and a direct break of invariant #7's identity chain. A newline
   is just as damaging without any injection intent: tmux inserts only the LAST
   line of a `#()` job's output, so a `\n` in a path silently deletes every
   widget to its left. Sanitizing at the render boundary rather than in each
   reader is deliberate — it is the only place that also covers plugin-supplied
   `Segment.text`, which no reader touches. `ansi.rs`'s scanner collapses `##`
   back to a literal `#` so `--preview`, `theme show` and the `widget edit`
   preview strip agree with what tmux actually draws. **Adding a fourth site
   that writes non-renderer-produced text into markup means routing it through
   `sanitize_text` too.** The one deliberate exemption is
   `assemble.rs`'s `#[range=window|{index}]`, whose only production feed is
   tmux's own integer `#{window_index}` (documented at that call site). Note
   this is the *output* direction; invariant #4 covers the opposite one (tmux
   format vars into rustline's argv), and `#{q:}` escapes for `/bin/sh`, not
   for tmux markup, so the two are unrelated defences.

**Platform-specific reads stay at the `Context`-build edge.** `read_battery()`
(`crates/rustline/src/battery.rs`), `read_cpu()` (`crates/rustline/src/cpu.rs`),
and `read_memory()` (`crates/rustline/src/memory.rs`) are the three
`#[cfg(target_os)]` surfaces in the codebase; each OS arm (Linux sysfs/`/proc`,
macOS `pmset`/`sysctl`+`vm_stat`, and cpu's mach `host_statistics`) delegates to
a pure parser where the source is text (`parse_linux`/`parse_pmset`,
`parse_proc_stat`, `parse_meminfo`/`parse_macos_memory`) that is
`#[cfg(any(target_os = …, test))]`-compiled, so those are unit-tested on the
dev box even though only one reader arm per module compiles per platform. (The
cpu reader is the one exception to the pure-text-parser shape: its macOS arm is
a mach FFI call returning counters directly, so it feeds the *shared*
snapshot-delta helpers rather than a bespoke parser — see `cpu.rs` above.) Follow this
pattern for any future OS-specific signal. `Context.os`/`Context.arch` (from
`std::env::consts::OS`/`ARCH`) are now available for WASM guests that want to
branch on platform.

**WASM plugin invariants (added by the plugin system — re-check when touching
`rustline-wasm` or `plugins/*`):**

9. **N1. Zero ambient authority.** A guest runs with `with_wasi(false)` and no
   Extism built-in HTTP/FS/process-spawn; every network/filesystem/exec
   effect goes through a host function that checks the plugin's
   `CapabilityCtx` first. Adding a new host capability means adding its gate
   *and* a denied-case test. The
   TTL-cached GET (`rl_http_get_cached`) gates `allowed_urls` before any fetch
   (gate-first: a denied URL makes no network call and touches no cache),
   with its own denied-case test. The exec capability (`rl_exec`/
   `rl_exec_cached`) is the most recent worked instance of this pattern:
   `perform_exec`/`perform_exec_cached` gate on `canonical_argv(program,
   args)` — the whole quoted command line, not just `program` — against
   `allowed_commands` before `runner.run` is ever called, with denied-case
   tests proving a denied argv never reaches a spawn (and, for the cached
   variant, never touches its own cache file either). Every deny site also
   calls `observe_denial` (before returning `ok:false`) so the default
   `FileDenialObserver` both records the `(kind, target)` for `rustline
   plugin denials` (W28) and logs a deduped `warn!` pointing at that command
   — recording (and now logging) is best-effort and never changes the gate
   outcome. `perform_file_read`/`perform_file_write` are the filesystem
   capability's instance of this pattern: they gate on the separate
   `allowed_paths` (read) / `allowed_write_paths` (write) allowlists, after
   first resolving the path through `resolve_for_allowlist`'s symlink gate —
   see the Config section's filesystem-capability paragraph.
10. **N2. A plugin never breaks the bar.** Any instantiation error, render
   error, timeout, or malformed output degrades to empty segments
   (`WasmWidget::render`), bounded by fuel + wall-clock timeout + memory caps.
   This composes with, not replaces, the existing `catch_unwind` per-widget
   guard in `render_named_region`.
11. **N3. State writes are dir-sandboxed and quota-bounded.** `rl_state_*` is
    confined to `<state_root>/<name>/` (`sanitize_relpath` rejects absolute
    paths and any `..` component) and refuses a write that would push the
    plugin's state dir over `max_state_bytes` (`check_cap`).
12. **N4. Per-plugin capability scope.** Allowlists/state root/quota come from
    that plugin instance's own `CapabilityCtx` (Extism `UserData`); one
    plugin can never use another's grants.

