# `rustline` bin — platform read surfaces

> **Read when:** adding or changing an OS-specific reader (battery, cpu, memory, git, disk, uptime, media, throughput, windows) or the persisted sample/history helpers.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- `battery.rs` — `read_battery()`, a `#[cfg(target_os)]` read surface (one of
  three — see `cpu.rs`/`memory.rs` below): a Linux sysfs
  (`/sys/class/power_supply/*/{capacity,status}`) arm and a macOS
  `pmset -g batt` arm, each delegating to a pure parser (`parse_linux`/
  `parse_pmset`) that is `#[cfg(any(target_os = …, test))]`-compiled so both
  are unit-tested on the Linux dev box even though only one reader arm
  compiles per platform. Any other platform, or a failed read, yields `None`.
- `cpu.rs` — `read_cpu()`, a `#[cfg(target_os)]` read surface. Linux now
  persists a `<state_root>/cpu-sample` snapshot (`CpuSnapshot { idle, total,
  ts }`) across invocations (W11): the fast path reads the current
  `/proc/stat` line and diffs it against that persisted snapshot if one
  exists and is fresh (`busy_from_snapshots`, within
  `CPU_SNAPSHOT_STALENESS_SECS` = 60s), returning with **no sleep**; only
  when there's no fresh persisted snapshot (first run, a stale one, or a
  backward clock) does it fall back to the classic two-sample read
  (`parse_proc_stat` + `busy_percent`, sampling `CPU_SAMPLE_WINDOW` ~120 ms
  apart). Either way the current reading is persisted afterward
  (`store_snapshot`, via the shared `write_atomic` primitive — see
  `rustline-core`'s `atomic_write.rs` above) so the *next*
  call can take the fast path — this is no longer a stateless delta. **macOS now
  shares this exact snapshot-delta path**: `read_cpu_macos_in` reads cumulative
  CPU ticks from the mach kernel via `read_mach_cpu_ticks`
  (`host_statistics(HOST_CPU_LOAD_INFO)`, the file's only `unsafe`, a scoped
  `#[allow(deprecated)]` over libc's mach bindings so the dep graph stays
  minimal) into the same `(idle, total)` `CpuTimes` shape Linux derives from
  `/proc/stat`, then takes the same zero-sleep cross-invocation delta (fast
  path) or the `CPU_SAMPLE_WINDOW` ~120 ms two-sample fallback, persisting to
  the same `<state_root>/cpu-sample` file — so the snapshot machinery
  (`CpuSnapshot`/`busy_from_snapshots`/`serialize_snapshot`/`parse_snapshot`/
  `load_snapshot`/`store_snapshot`/`busy_percent`) is now
  `#[cfg(any(target_os = "linux", target_os = "macos", test))]`. This
  **replaced the old `top -l 2 -n 0` shell-out** (`parse_top_cpu`, now gone),
  which slept ~1.4 s internally on every read: under the render daemon's shared
  render lock that pinned the whole refresh burst and dropped queued clients
  with EPIPE (see `daemon.rs`'s benign-disconnect logging). All the pure
  helpers (`parse_proc_stat`/`busy_percent`/`busy_from_snapshots`/
  `parse_snapshot`/`serialize_snapshot`) are
  `#[cfg(any(target_os = …, test))]`-compiled and unit-tested on the dev box,
  with the snapshot-cache tests injecting a tempdir rather than touching the
  real state dir. Unsupported platform or failed read → `None`. Also `read_cpu_history(state_dir, current_percent,
  spark_width) -> Vec<f32>` (W45): a SEPARATE persisted ring at
  `<state_root>/cpu-history` (distinct from the `cpu-sample` snapshot above) —
  load via `sample_store` + `history::parse_history`, push the current reading,
  truncate to `spark_width`, persist, and return — feeding the `cpu` widget's
  `{spark}` placeholder. Only called from the build edge when `cpu`'s `format`
  **or** `alt_format` references `{spark}` (W56).
- `memory.rs` — `read_memory()`, a `#[cfg(target_os)]` read surface: Linux
  reads `/proc/meminfo` (`MemTotal`/`MemAvailable` in kB, `parse_meminfo`);
  macOS reads the total via `libc::sysctlbyname("hw.memsize")` — memoized in a
  `OnceLock` (installed RAM can't change while the process lives), the file's
  one `unsafe`, replacing a per-render `sysctl -n hw.memsize` spawn the same way
  `cpu.rs` replaced its `top -l 2` shell-out — and still shells out to `vm_stat`
  for the page counters, deriving available bytes from free/inactive/speculative
  pages at the reported page size (`parse_macos_memory`). So, like `cpu.rs`,
  macOS memory is now **half FFI, half text parser** rather than the pure
  shell-out pair the cfg-gated-parser pattern describes: `memsize_to_text`
  renders the FFI `u64` back into the decimal text `parse_macos_memory` already
  took, which keeps the parser's contract — and its Linux-run unit tests —
  unchanged. Same cfg-gated pure-parser pattern as `battery.rs`/`cpu.rs`
  otherwise. Unsupported platform or failed read → `None` (the `sysctlbyname`
  failure is latched by the `OnceLock` for the process lifetime; see the
  function's doc comment). Also
  `read_memory_history(state_dir, current_percent, spark_width) -> Vec<f32>`
  (W45), the `memory` counterpart to `cpu.rs`'s `read_cpu_history`, persisting a
  `<state_root>/memory-history` ring for `memory`'s `{spark}`.
- `git.rs` — `read_git(path) -> Option<GitInfo>`, a platform-agnostic (no
  `#[cfg(target_os)]`) shell-out read: runs `git -C <path> status
  --porcelain=v2 --branch`, `None` on ANY failure (`git` missing, non-repo,
  non-zero exit, **or the spawn exceeding its timeout**). Delegates to the pure
  `parse_git_status(&str) -> GitInfo`
  (unconditionally unit-tested, no cfg-gating needed since there's no OS
  branching) — same pure-parser-behind-the-read-surface shape as
  `battery.rs`/`cpu.rs`/`memory.rs`, just keyed on tool availability rather
  than platform.
  This spawn, and the siblings in `media.rs`/`windows.rs`, go through
  `rustline_wasm::run::run_bounded` rather than a bare `Command::output()`:
  `pane_current_path` comes straight from tmux, so with the pane sitting on an
  unresponsive NFS/sshfs mount a blocking `git status` never returns, the
  `#()` job never completes, and the region goes permanently blank — and under
  the daemon it pins the shared render lock for the process's life, so every
  other client times out and falls back in-process onto the same hang. This is
  the protection the exec capability always had (`run.rs`'s `EXEC_TIMEOUT` +
  process-group kill, documented there as guarding exactly that lock); the
  built-in readers simply never got it. A timeout is treated like the existing
  non-zero-exit arm. Caveat worth knowing: `run_bounded` also inherits that
  capability's 64 KiB per-stream output cap, which was sized for plugin
  commands — a repo with roughly 600+ changed *tracked* files can undercount
  `{staged}`/`{unstaged}`. Porcelain v2 emits its headers first, so
  `{branch}`/`{ahead}`/`{behind}` are unaffected, and `tmux list-windows` /
  `playerctl` are nowhere near the cap.
- `disk.rs` — `read_disk(mount) -> Option<DiskInfo>`, a `statvfs(2)` read.
  Unlike `battery`/`cpu`/`memory`, the syscall itself is POSIX and needs no
  `#[cfg(target_os)]` split at all (it's available unconditionally on Linux
  and macOS); only the pure derivation, `disk_info_from_statvfs(f_blocks,
  f_bfree, f_bavail, f_frsize) -> DiskInfo` (all `u64`, saturating
  arithmetic), is `#[cfg(any(target_os = …, test))]`-gated, matching the
  platforms it's exercised on. `None` on any failure: a nul byte in `mount`,
  or the `statvfs` call itself failing (e.g. a nonexistent mount).
- `uptime.rs` — `read_uptime() -> Option<u64>` (W37), a `#[cfg(target_os)]`
  read surface following the `battery.rs`/`cpu.rs` pattern: Linux parses
  `/proc/uptime` (`parse_proc_uptime`), macOS derives it from `sysctl -n
  kern.boottime` vs now (`parse_kern_boottime`); both pure parsers are
  `#[cfg(any(target_os = …, test))]`-compiled and unit-tested on Linux.
- `media.rs` — `read_media() -> Option<MediaInfo>` (W41), a Linux shell-out to
  `playerctl metadata` behind the pure `parse_playerctl`; `None` on any failure
  (`playerctl` missing, no player, malformed output). Non-Linux → `None`.
- `throughput.rs` — `read_throughput(state_dir, interface) -> Option<Throughput>`
  (W47), a Linux read surface at the `Context`-build edge. A rate is a delta,
  so — mirroring `cpu.rs`'s persisted-snapshot pattern — it diffs the current
  `/proc/net/dev` counters against a prior `(rx, tx, ts)` sample persisted at
  a per-interface sample file (W46 — `fn sample_name(interface:
  Option<&str>) -> String`: `throughput-sample` for the aggregate read
  (`None`), `throughput-sample-<sanitized-iface>` for a named interface, any
  non-`[A-Za-z0-9_-]` character replaced so multiple simultaneous
  `throughput` instances on different interfaces read/write distinct files
  instead of clobbering one shared sample) via `sample_store` (no sleep
  across two live reads). `None` on the first invocation (nothing to diff
  against), an
  unsupported platform, or a read failure — never a fabricated `0` (invariant
  #6). The pure `parse_proc_net_dev`/`throughput_rate`/`aggregate` helpers are
  `#[cfg(any(target_os = "linux", test))]`-compiled (same as `cpu.rs`), so
  they're unit-tested on any dev box; `interface` pins the read to one named
  interface, `None` aggregates all non-loopback interfaces.
- `windows.rs` — `read_windows(session: Option<&str>) -> Vec<WindowCtx>` (W42),
  a platform-agnostic shell-out to `tmux list-windows -F
  "#{window_index}\t#{window_active}\t#{window_flags}\t#{window_name}"` (name
  last so a tab inside a window name can't misalign the earlier fields; the
  pure `parse_list_windows` does the `splitn(4, '\t')` parse, unconditionally
  unit-tested — no cfg-gating needed, same shape as `git.rs`'s tool-shell-out
  pattern). Empty `Vec` on ANY failure (`tmux` missing, bad session, non-zero
  exit) — never a fabricated window (invariant #6). Backs `render windows`
  (the batched two-line window-list render; called directly from `main.rs`,
  not routed through `build_context.rs`).
- `sample_store.rs` — shared best-effort atomic per-widget state persistence
  (W52): `read_sample`/`write_sample` read/write a small text file under a state
  dir via `write_atomic` (`rustline-core`'s `atomic_write.rs`), `warn!`ing
  (never panicking) on I/O failure. A generalization of `cpu.rs`'s
  pre-existing `cpu-sample` dance, reused by
  `throughput.rs` and the `{spark}` history reads; each caller keeps its own
  sample serialization/parsing.
- `history.rs` — pure sparkline-history ring helpers (W45): `parse_history`/
  `serialize_history` over a single space-separated line of `f32` readings
  (oldest first, total over corrupt tokens), plus push/truncate — the ring's
  own shape, split from `sample_store`'s generic file I/O the same way
  `cpu.rs`'s `parse_snapshot`/`serialize_snapshot` are. Shared by `cpu.rs`'s and
  `memory.rs`'s `{spark}` history reads.
- `sample_context.rs` — the one shared synthetic-`Context` builder (W52):
  `sample_context(show_alerts) -> Context`, a representative fully-populated
  fixture. The three previously near-identical hand-rolled fixtures
  (`theme_cmd.rs`'s preview `sample_context`, `bench/fixture.rs`'s
  `fabricated_context`, `plugin_cmd.rs`'s `plugin run` harness) now delegate
  here via struct-update, so they no longer drift as `Context` grows.
  `theme show`/`theme pick`'s rendered preview is the load-bearing consumer —
  every field its preview layout can observe keeps theme_cmd's original
  pre-consolidation value verbatim (pinned byte-identical by a characterization
  test); the extra superset fields carry synthetic data no preview widget reads.
