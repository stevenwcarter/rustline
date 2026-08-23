# `rustline` bin — the optional render daemon

> **Read when:** touching the daemon protocol, client fallback, server state/reload, or the systemd/launchd service installer.
>
> Detail split out of `CLAUDE.md` (repo root), which holds the high-level map.

- `daemon_proto.rs` — the optional daemon's wire protocol (W48), pure and
  self-contained (no socket/listener/client here): `DaemonRequest {
  RenderV2 { protocol: u32, region: RegionKind, args: RenderArgsWire }, Ping,
  Shutdown }` plus `pub const DAEMON_PROTOCOL: u32` — the version handshake that
  keeps a **stale daemon from silently serving a new client** (the daemon is
  long-lived and `reload_if_changed` watches only the *config* file's mtime,
  never the binary's, so after a `cargo install` the old daemon would otherwise
  keep answering with old widget code and the client would emit that markup as
  its own). It is fail-closed **by construction** in both directions, not by a
  check: the request enum is externally tagged, so an old daemon simply cannot
  deserialize `RenderV2` (and a new daemon cannot deserialize the old `Render`)
  — the frame errors, that one connection is dropped, and `try_render`'s `None`
  falls back to the in-process render. A protocol number that merely *differs*
  is refused explicitly (`daemon.rs`), and `DaemonResponse`'s client-side match
  is exhaustive with no `_` arm so a future variant is a compile error rather
  than an accidental accept. `Ping`/`Shutdown` are deliberately left unversioned
  as unit variants so `daemon status`/`daemon stop` keep working across a skew.
  Note the `V2` in the variant name is the second generation of the *variant*
  and is independent of `DAEMON_PROTOCOL`'s value — a protocol bump to 2 does
  **not** rename it to `RenderV3`. Bumping `DAEMON_PROTOCOL` whenever render
  semantics change is a hand-maintained discipline; forgetting it reproduces the
  original silent-skew bug.
  `RegionKind { Left, Right, Window }` (mirrors `cli::Render`'s three
  variants), `RenderArgsWire` (combines `cli::RegionArgs`'s
  `session`/`window`/`pane`/`pane_path` and `cli::WindowArgs`'s
  `index`/`name`/`flags`/`current` into one struct covering all three
  `RegionKind`s — fields that don't apply to the requested kind are
  absent/default; derives `Default`), and `DaemonResponse { Markup(String),
  Pong, ShuttingDown }`. `fn write_frame<W: Write>(w, &impl Serialize) ->
  io::Result<()>`/`fn read_frame<R: Read, T: DeserializeOwned>(r) ->
  io::Result<T>` frame each message as a 4-byte little-endian length prefix
  + JSON, with an 8 MiB cap checked **before** allocating (an oversize or
  truncated frame is an `io::Error`, never a panic).
- `daemon_client.rs` — the daemon client (W48): `fn daemon_socket_path() ->
  PathBuf` resolves `$XDG_RUNTIME_DIR/rustline/daemon.sock`, falling back to
  `<state_root>/daemon.sock` when `XDG_RUNTIME_DIR` is unset; `fn
  try_render(region: RegionKind, args: RenderArgsWire) -> Option<String>`
  is the safety seam every render call site goes through: one `stat` first
  (socket file absent → `None` immediately, near-zero overhead when the
  daemon isn't running), else connect with a short (250 ms)
  read/write timeout, `write_frame` the request, `read_frame` the response,
  and return the markup **only** on `DaemonResponse::Markup` — `None` on
  *any* other outcome (connect/timeout/decode error, or a `Pong`/
  `ShuttingDown` reply), so the caller always has a safe in-process render to
  fall back to (invariant N2 extended to the daemon path — "never break the
  bar"). It also now reports *why* it fell back: a missing socket stays
  silent (not installed is not a problem), but every failure past that
  `stat` — a timeout setter, the write, the read, an unexpected response —
  logs at `debug` (cheap, per-tick, `-vvv` detail), except a socket that
  *exists but refuses the connection* (a dead daemon left it behind), which is
  `warn_once`'d instead (`"daemon-stale-socket:<path>"`) since that case is
  actionable and persists across ticks rather than being a one-off, unlike a
  per-tick `debug!`. `daemon.rs`'s `reload_if_changed` gets a matching
  `info!` — one line per config edit (it only runs when the mtime actually
  changed, so no dedup is needed), the only confirmation a user gets that a
  warm daemon picked up their edit.
- `daemon.rs` — the daemon server (W48): `DaemonState { config, theme,
  plugin_dir, registry, config_mtime }` holds everything warm across
  requests — parsed `Config`, resolved `Theme`, a built `Registry` (built
  over the **union** of both regions' layouts so plugins referenced by
  either side are warm), and the config file's last-seen mtime — with a
  `reload_if_changed` that re-parses/re-resolves/rebuilds only when
  `config.toml`'s mtime has actually advanced (the common case reuses
  everything, including live WASM plugin instances). `pub fn serve(config_path:
  &Path, plugin_dir: PathBuf) -> io::Result<()>` binds a `UnixListener` at
  `daemon_client::daemon_socket_path()` (removing a stale socket file first),
  then accepts connections on their own thread against a shared
  `Arc<Mutex<DaemonState>>`; a `Render` request builds the *same* `Context` via
  `build_region_context`/`build_window_context` and renders via the *same*
  `render_named_region`/`render_window` the in-process path uses, so daemon
  output is byte-identical; `Ping` → `Pong`; `Shutdown` → reply
  `ShuttingDown`, remove the socket, and exit. Renders serialize behind the
  state `Mutex` (fine — renders are fast, and it also satisfies
  `extism::Plugin`'s `!Sync` constraint: only one thread ever renders through
  a given plugin instance at a time), and the mutex is never held across a
  blocking socket read/write, so one slow/hung client can't stall the others
  or deadlock a reload. `pub fn status() -> bool` (connect + `Ping`, used by
  both `rustline daemon status` and `doctor`'s advisory check) and `pub fn
  stop() -> io::Result<()>` (connect + `Shutdown`) round out the client-side
  control operations.
- `daemon_service.rs` — `rustline daemon install|uninstall`: makes the daemon
  survive logins using each platform's native per-user service mechanism, with
  the OS backend chosen at **compile time**. The public `install(binary,
  write_only)`/`uninstall()` are platform-agnostic dispatchers — `main.rs`
  names neither the path nor the service manager; an unsupported platform
  prints a notice and does nothing. **Linux → systemd user unit** at
  `$XDG_CONFIG_HOME/systemd/user/rustline-daemon.service` (fallback
  `~/.config/systemd/user/`, `service_unit_path()`); `service_unit(binary)`
  (pure) renders `ExecStart=<binary> daemon run` + `Restart=on-failure`, wired
  via the `Systemctl` trait (`available`/`daemon_reload`/`enable_now`/
  `disable_now`), `RealSystemctl` the only production impl. **macOS → launchd
  LaunchAgent** at `~/Library/LaunchAgents/rustline-daemon.plist`
  (`plist_path()`); `plist_contents(binary)` (pure) renders the plist with
  `Label=rustline-daemon`, `ProgramArguments=[<binary>, daemon, run]`,
  `RunAtLoad=true`, and `KeepAlive={SuccessfulExit=false}` (restart on failure
  only — the launchd analogue of `Restart=on-failure`; a clean `daemon stop`
  stays stopped), wired via the `Launchctl` trait (`available`/`bootstrap`/
  `bootout`), `RealLaunchctl` the only production impl (`launchctl
  bootstrap|bootout gui/$UID <plist>`, `gui_domain()` via `libc::getuid`, the
  sole macOS `unsafe`). Both backends share `command_on_path` and a
  stdio-silenced `run_silent`; both traits are the same recording-fake seam
  `click.rs`'s `ClickExecutor` uses, so the pure builders + `install_*`/
  `uninstall_*` flows are unit-tested (via `FakeSystemctl`/`FakeLaunchctl` on a
  tempdir path) on both the Linux CI box and macOS — the pure fns/inner flows
  are `#[cfg(any(target_os = …, test))]`-compiled, the `Real*` impls
  `#[cfg(target_os = …)]`. `install` writes the unit/plist (overwriting/
  reporting a replace), then either prints the manual load command
  (`--write-only`, or the manager absent from `PATH`) or reloads/enables it
  (macOS `bootout`s any stale instance first so reinstall is idempotent);
  `uninstall` best-effort unloads and removes the file, safe to re-run. Every
  real invocation is best-effort (spawn failure/non-zero exit → `false`, never
  a panic) and neither operation treats a manager failure as fatal — the
  unit/plist file is the durable part.
