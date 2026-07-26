//! Daemon server (W48): a long-lived process that keeps the render state
//! (parsed [`Config`], resolved [`Theme`], and a warm [`Registry`] with its
//! WASM plugins already instantiated) hot in memory and answers
//! [`DaemonRequest`]s over a Unix socket, so a `render` no longer pays
//! per-invocation startup — config parse, theme resolution, and (the
//! expensive part) plugin instantiation.
//!
//! ## Concurrency model
//!
//! The state lives behind an `Arc<Mutex<DaemonState>>` and the accept loop
//! spawns one thread per connection. Renders therefore **serialize** behind
//! the state mutex: only one thread ever renders at a time. That is
//! intentional — `extism::Plugin` is `!Sync`, so a warm [`WasmWidget`]
//! (which holds an `Arc<Mutex<Plugin>>`) must not be rendered from two threads
//! at once. Serializing also keeps [`DaemonState::reload_if_changed`]'s
//! rebuild atomic w.r.t. concurrent renders. This is not a throughput
//! bottleneck for a status line: a render is sub-millisecond and tmux fires
//! them a handful of times per `status-interval`.
//!
//! ## Never break the bar
//!
//! A single connection's read/write error is logged and drops **only that
//! connection** — the daemon keeps serving (invariant N2). The client
//! (`daemon_client`) already falls back to an in-process render on any daemon
//! failure, so even a wholly dead daemon never breaks the bar. `serve` does
//! not self-daemonize (no fork): it binds the socket and blocks in the
//! foreground.

use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use rustline_core::{Config, Direction, Registry, Theme, render_named_region, render_window};

use crate::build_context::{build_region_context, build_window_context};
use crate::cli::{RegionArgs, WindowArgs};
use crate::daemon_client::daemon_socket_path;
use crate::daemon_proto::{self, DaemonRequest, DaemonResponse, RegionKind, RenderArgsWire};

/// How long a `status`/`stop` client waits on the daemon socket before giving
/// up. Short on purpose — these are liveness/teardown pokes, not renders.
const SOCKET_TIMEOUT: Duration = Duration::from_millis(250);

/// The warm render state a running daemon keeps in memory: the parsed config,
/// its resolved theme, the plugin discovery dir, a registry with built-ins +
/// WASM plugins already instantiated, and the config file's last-seen
/// modification time (so [`reload_if_changed`](DaemonState::reload_if_changed)
/// can rebuild when the file changes).
struct DaemonState {
    config: Config,
    theme: Theme,
    plugin_dir: PathBuf,
    registry: Registry,
    config_mtime: Option<SystemTime>,
}

impl DaemonState {
    /// Build the warm state from scratch: load the config, resolve the theme,
    /// and build a registry of built-ins plus every WASM plugin referenced by
    /// **either** region's layout (`layout.left ∪ layout.right`), so a plugin
    /// is warm regardless of which region a later `Render` asks for. Records
    /// the config file's current mtime for change detection.
    fn build(config_path: &Path, plugin_dir: PathBuf) -> DaemonState {
        let config = Config::load(config_path);
        let theme = crate::resolve_theme(&config);
        let mut registry = Registry::with_builtins(&config);
        let needed = plugin_union(&config);
        rustline_wasm::register_plugins(&mut registry, &config, &plugin_dir, &needed);
        let config_mtime = config_mtime(config_path);
        DaemonState {
            config,
            theme,
            plugin_dir,
            registry,
            config_mtime,
        }
    }

    /// If the config file's mtime differs from the one recorded at build time,
    /// rebuild everything (config, theme, registry, mtime) and return `true`;
    /// otherwise leave the state untouched and return `false`. A missing file
    /// reads as `None`, so appearing/disappearing also counts as a change.
    fn reload_if_changed(&mut self, config_path: &Path) -> bool {
        let current = config_mtime(config_path);
        if current == self.config_mtime {
            return false;
        }
        // `plugin_dir` isn't derived from the config, so it survives a reload;
        // clone it out before the assignment consumes `self`.
        *self = DaemonState::build(config_path, self.plugin_dir.clone());
        true
    }

    /// Render one region/window to tmux markup — byte-identical to the
    /// in-process `render left|right|window` path (same `build_*_context` +
    /// `render_named_region`/`render_window` calls over the same registry,
    /// theme, and color overrides).
    fn render(&self, region: RegionKind, args: &RenderArgsWire) -> String {
        match region {
            RegionKind::Left => self.render_region(Direction::Left, &self.config.layout.left, args),
            RegionKind::Right => {
                self.render_region(Direction::Right, &self.config.layout.right, args)
            }
            RegionKind::Window => {
                let window_args = WindowArgs {
                    current: args.current,
                    index: args.index.clone().unwrap_or_default(),
                    name: args.name.clone().unwrap_or_default(),
                    flags: args.flags.clone().unwrap_or_default(),
                    preview: args.preview,
                };
                let ctx = build_window_context(&window_args);
                render_window(&ctx, &self.registry, &self.theme)
            }
        }
    }

    /// Render a left/right region: convert the wire args, build the region
    /// context, and call `render_named_region` with the warm registry/theme
    /// plus the config's color overrides — the same sequence `main`'s
    /// `Render::Left`/`Right` arms run.
    fn render_region(&self, dir: Direction, layout: &[String], args: &RenderArgsWire) -> String {
        let region_args = RegionArgs {
            session: args.session.clone(),
            window: args.window.clone(),
            pane: args.pane.clone(),
            pane_path: args.pane_path.clone(),
            ..RegionArgs::default()
        };
        let ctx = build_region_context(&region_args, layout, &self.theme, &self.config);
        let overrides = self.config.color_overrides();
        render_named_region(dir, layout, &ctx, &self.registry, &self.theme, &overrides)
    }
}

/// The distinct plugin/widget names referenced by either region's layout
/// (`layout.left ∪ layout.right`) — what `register_plugins` needs so both
/// regions' plugins are instantiated once at build time.
fn plugin_union(config: &Config) -> Vec<String> {
    let mut names = config.layout.left.clone();
    for name in &config.layout.right {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    names
}

/// The config file's modification time, or `None` if it can't be stat'd
/// (e.g. the file doesn't exist yet).
fn config_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Whether the accept loop should keep serving after handling a request, or
/// tear down (remove the socket, then exit). Returned by [`handle_request`] /
/// [`serve_connection`] so the socket-removal + `process::exit` side effects
/// live only in the accept loop — keeping the request logic unit-testable
/// without exiting the test process.
enum Disposition {
    Continue,
    Shutdown,
}

/// Compute the response to one request (and whether the server should stop
/// after replying). Holds no socket/`process::exit` side effects, so it is
/// directly unit-testable. A `Render` locks the state, reloads on a config
/// change, and renders; `Ping`/`Shutdown` are trivial.
fn handle_request(
    state: &Mutex<DaemonState>,
    config_path: &Path,
    req: DaemonRequest,
) -> (DaemonResponse, Disposition) {
    match req {
        DaemonRequest::RenderV2 {
            protocol,
            region,
            args,
        } => {
            // A client built against a different protocol must not be handed
            // markup this daemon's semantics produced. Refusing sends it back
            // to its in-process fallback, which is always correct (N2).
            // Reusing `ShuttingDown` here (rather than a new variant) is
            // deliberate and safe: `try_render_at` already maps every
            // non-`Markup` response to `None`, so the client falls back
            // correctly with no protocol-aware handling needed on its end.
            if protocol != daemon_proto::DAEMON_PROTOCOL {
                tracing::warn!(
                    client_protocol = protocol,
                    daemon_protocol = daemon_proto::DAEMON_PROTOCOL,
                    "refusing a render request from a client with a different protocol; \
                     restart the daemon to pick up the new binary"
                );
                return (DaemonResponse::ShuttingDown, Disposition::Continue);
            }
            // A poisoned mutex (a prior render panicked mid-lock) must not kill
            // the daemon: recover the guard and carry on (never break the bar).
            let mut st = state.lock().unwrap_or_else(PoisonError::into_inner);
            st.reload_if_changed(config_path);
            let markup = st.render(region, &args);
            (DaemonResponse::Markup(markup), Disposition::Continue)
        }
        DaemonRequest::Ping => (DaemonResponse::Pong, Disposition::Continue),
        DaemonRequest::Shutdown => (DaemonResponse::ShuttingDown, Disposition::Shutdown),
    }
}

/// Read one request from `stream`, handle it, and write the reply. Returns the
/// [`Disposition`] the accept loop should act on. Any framing/IO error is
/// propagated so the caller can drop just this connection.
fn serve_connection(
    mut stream: UnixStream,
    state: &Mutex<DaemonState>,
    config_path: &Path,
) -> io::Result<Disposition> {
    // The client sets its own timeouts, but a server must never trust a peer to:
    // an accepted stream with no timeout lets a stalled peer pin this thread
    // forever. Bound both directions on the accepted stream. A `set_*_timeout`
    // error just drops this one connection (the `?` propagates to the accept
    // loop, which logs and moves on) — best-effort, never break the bar.
    stream.set_read_timeout(Some(SOCKET_TIMEOUT))?;
    stream.set_write_timeout(Some(SOCKET_TIMEOUT))?;
    let req: DaemonRequest = daemon_proto::read_frame(&mut stream)?;
    let (resp, disposition) = handle_request(state, config_path, req);
    daemon_proto::write_frame(&mut stream, &resp)?;
    Ok(disposition)
}

/// Remove the daemon socket file, ignoring an already-absent one. Used both to
/// clear a stale socket before binding (a leftover file makes `bind` fail with
/// `AddrInUse`) and on `Shutdown` before exiting. Best-effort: a real removal
/// failure is logged, never fatal.
fn remove_socket(sock: &Path) {
    match std::fs::remove_file(sock) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("failed to remove daemon socket {}: {e}", sock.display()),
    }
}

/// Run the daemon: bind the default socket and serve until a `Shutdown`
/// request (or Ctrl-C). Blocks in the foreground — no fork/self-daemonize.
pub fn serve(config_path: &Path, plugin_dir: PathBuf) -> io::Result<()> {
    serve_at(&daemon_socket_path(), config_path, plugin_dir)
}

/// [`serve`] against an explicit socket path (the seam tests bind on a tempdir
/// socket). Creates the socket's parent dir, clears any stale socket, binds,
/// then accept-loops one thread per connection over an
/// `Arc<Mutex<DaemonState>>`.
fn serve_at(sock: &Path, config_path: &Path, plugin_dir: PathBuf) -> io::Result<()> {
    if let Some(parent) = sock.parent() {
        std::fs::create_dir_all(parent)?;
    }
    remove_socket(sock);
    let listener = UnixListener::bind(sock)?;
    eprintln!(
        "rustline daemon listening on {} (Ctrl-C to stop)",
        sock.display()
    );

    let state = Arc::new(Mutex::new(DaemonState::build(config_path, plugin_dir)));
    for conn in listener.incoming() {
        let stream = match conn {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("daemon accept error: {e}");
                continue;
            }
        };
        let state = Arc::clone(&state);
        let config_path = config_path.to_path_buf();
        let sock = sock.to_path_buf();
        std::thread::spawn(
            move || match serve_connection(stream, &state, &config_path) {
                Ok(Disposition::Shutdown) => {
                    remove_socket(&sock);
                    std::process::exit(0);
                }
                Ok(Disposition::Continue) => {}
                Err(e) => {
                    // A client that hit its own short read timeout and fell
                    // back to the in-process render (invariant N2) closes its
                    // socket before we finish writing the response, so our
                    // `write_frame` sees EPIPE/ECONNRESET (or a write-timeout
                    // WouldBlock, or EOF mid-request). That's an expected,
                    // benign disconnect — never break the bar — so it logs at
                    // debug; only a genuine framing/IO fault stays a warn.
                    use std::io::ErrorKind::{
                        BrokenPipe, ConnectionReset, TimedOut, UnexpectedEof, WouldBlock,
                    };
                    if matches!(
                        e.kind(),
                        BrokenPipe | ConnectionReset | UnexpectedEof | TimedOut | WouldBlock
                    ) {
                        tracing::debug!("daemon client disconnected before response: {e}");
                    } else {
                        tracing::warn!("daemon connection dropped: {e}");
                    }
                }
            },
        );
    }
    Ok(())
}

/// Liveness check for `rustline daemon status`: connect to the default socket
/// and `Ping`; `true` iff a `Pong` comes back. Any failure (no socket, connect
/// error, timeout, unexpected reply) is `false`.
pub fn status() -> bool {
    status_at(&daemon_socket_path())
}

/// `pub(crate)` (rather than private) so the bench `daemon` pass can gate on
/// reachability of an explicit (e.g. fake/nonexistent) socket in its tests,
/// the same seam this module's own tests use.
pub(crate) fn status_at(sock: &Path) -> bool {
    let Ok(mut stream) = UnixStream::connect(sock) else {
        return false;
    };
    if stream.set_read_timeout(Some(SOCKET_TIMEOUT)).is_err()
        || stream.set_write_timeout(Some(SOCKET_TIMEOUT)).is_err()
    {
        return false;
    }
    if daemon_proto::write_frame(&mut stream, &DaemonRequest::Ping).is_err() {
        return false;
    }
    matches!(
        daemon_proto::read_frame::<_, DaemonResponse>(&mut stream),
        Ok(DaemonResponse::Pong)
    )
}

/// Ask the running daemon to stop, for `rustline daemon stop`: connect to the
/// default socket and send `Shutdown`. The `ShuttingDown` ack is read
/// best-effort (the daemon exits right after sending it).
pub fn stop() -> io::Result<()> {
    stop_at(&daemon_socket_path())
}

fn stop_at(sock: &Path) -> io::Result<()> {
    let mut stream = UnixStream::connect(sock)?;
    stream.set_read_timeout(Some(SOCKET_TIMEOUT))?;
    stream.set_write_timeout(Some(SOCKET_TIMEOUT))?;
    daemon_proto::write_frame(&mut stream, &DaemonRequest::Shutdown)?;
    let _: io::Result<DaemonResponse> = daemon_proto::read_frame(&mut stream);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::thread;

    use super::*;

    /// Advance `path`'s mtime deterministically (no sleep-and-hope, no
    /// `filetime` dependency) via the stable `File::set_modified`, so a reload
    /// test sees a strictly-later timestamp regardless of filesystem mtime
    /// resolution.
    fn bump_mtime(path: &Path) {
        let f = OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(SystemTime::now() + Duration::from_secs(2))
            .unwrap();
    }

    #[test]
    fn reload_if_changed_only_on_mtime_advance() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "").unwrap();
        let mut st = DaemonState::build(&cfg, dir.path().join("plugins"));

        assert!(
            !st.reload_if_changed(&cfg),
            "unchanged file must not reload"
        );

        std::fs::write(&cfg, "[layout]\nright = [\"datetime\"]\n").unwrap();
        bump_mtime(&cfg);
        assert!(
            st.reload_if_changed(&cfg),
            "advanced mtime must trigger a reload"
        );
        // The rebuilt state reflects the new config.
        assert_eq!(st.config.layout.right, vec!["datetime".to_string()]);
        // ...and a follow-up call with no further change reloads no more.
        assert!(!st.reload_if_changed(&cfg));
    }

    #[test]
    fn daemon_render_matches_in_process() {
        // A deterministic right layout (no time/cpu/memory/loadavg widgets,
        // whose live reads jitter between two independent `build_region_context`
        // calls and would make a byte comparison flaky). This pins the
        // load-bearing property: the daemon's render *plumbing* — region →
        // (layout, Direction), registry/theme/overrides selection, context
        // build, and the `render_named_region` call — is byte-identical to
        // `main`'s in-process `Render::Right` arm.
        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join("config.toml");
        std::fs::write(
            &cfg_path,
            "[layout]\nright = [\"pane_id\", \"hostname\", \"cwd\"]\n",
        )
        .unwrap();
        let plugin_dir = dir.path().join("plugins"); // nonexistent → no wasm

        let wire = RenderArgsWire {
            session: Some("main".into()),
            window: Some("1".into()),
            pane: Some("0".into()),
            pane_path: Some("/tmp/project".into()),
            ..RenderArgsWire::default()
        };

        let state = DaemonState::build(&cfg_path, plugin_dir.clone());
        let daemon_markup = state.render(RegionKind::Right, &wire);

        // Rebuild the exact in-process path `main`'s `Render::Right` arm runs.
        let cfg = Config::load(&cfg_path);
        let theme = crate::resolve_theme(&cfg);
        let mut registry = Registry::with_builtins(&cfg);
        rustline_wasm::register_plugins(&mut registry, &cfg, &plugin_dir, &cfg.layout.right);
        let region_args = RegionArgs {
            session: wire.session.clone(),
            window: wire.window.clone(),
            pane: wire.pane.clone(),
            pane_path: wire.pane_path.clone(),
            ..RegionArgs::default()
        };
        let ctx = build_region_context(&region_args, &cfg.layout.right, &theme, &cfg);
        let overrides = cfg.color_overrides();
        let in_process = render_named_region(
            Direction::Right,
            &cfg.layout.right,
            &ctx,
            &registry,
            &theme,
            &overrides,
        );

        assert_eq!(daemon_markup, in_process);
        assert!(!daemon_markup.is_empty(), "sanity: the layout renders");
    }

    #[test]
    fn daemon_window_render_matches_in_process() {
        // The window-pill path has no live/jittering reads (`build_window_context`
        // reads only `Context.window`), so this is deterministic. It pins the
        // currently-unguarded invariant that `render_window` resolves only
        // `["windows"]` — so the daemon's (potentially plugin-laden) registry and
        // a freshly built builtins registry produce byte-identical window output.
        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join("config.toml");
        std::fs::write(&cfg_path, "").unwrap();
        let plugin_dir = dir.path().join("plugins"); // nonexistent → no wasm

        let wire = RenderArgsWire {
            current: true,
            index: Some("2".into()),
            name: Some("editor".into()),
            flags: Some("*".into()),
            ..RenderArgsWire::default()
        };

        let state = DaemonState::build(&cfg_path, plugin_dir);
        let daemon_markup = state.render(RegionKind::Window, &wire);

        // Rebuild the exact in-process path `main`'s `Render::Window` arm runs.
        let cfg = Config::load(&cfg_path);
        let theme = crate::resolve_theme(&cfg);
        let registry = Registry::with_builtins(&cfg);
        let window_args = WindowArgs {
            current: wire.current,
            index: wire.index.clone().unwrap_or_default(),
            name: wire.name.clone().unwrap_or_default(),
            flags: wire.flags.clone().unwrap_or_default(),
            preview: wire.preview,
        };
        let ctx = build_window_context(&window_args);
        let in_process = render_window(&ctx, &registry, &theme);

        assert_eq!(daemon_markup, in_process);
        assert!(!daemon_markup.is_empty(), "sanity: the window pill renders");
    }

    #[test]
    fn handle_request_maps_each_variant() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "").unwrap();
        let state = Mutex::new(DaemonState::build(&cfg, dir.path().join("plugins")));

        let (resp, disp) = handle_request(&state, &cfg, DaemonRequest::Ping);
        assert_eq!(resp, DaemonResponse::Pong);
        assert!(matches!(disp, Disposition::Continue));

        let (resp, disp) = handle_request(&state, &cfg, DaemonRequest::Shutdown);
        assert_eq!(resp, DaemonResponse::ShuttingDown);
        assert!(matches!(disp, Disposition::Shutdown));

        let (resp, disp) = handle_request(
            &state,
            &cfg,
            DaemonRequest::RenderV2 {
                protocol: daemon_proto::DAEMON_PROTOCOL,
                region: RegionKind::Right,
                args: RenderArgsWire::default(),
            },
        );
        assert!(matches!(resp, DaemonResponse::Markup(_)));
        assert!(matches!(disp, Disposition::Continue));
    }

    #[test]
    fn serve_connection_round_trip_ping_then_shutdown() {
        // A real bound socket + real client connections exercise the full
        // per-connection path (`read_frame` → `handle_request` → `write_frame`).
        // The outer accept loop's `process::exit(0)` is the ONLY thing not
        // driven here (a unit test must not exit its own process); we instead
        // run exactly the cleanup that loop runs on `Shutdown` — `remove_socket`
        // — and assert the socket is gone, proving "Shutdown removes the socket".
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "").unwrap();
        let sock = dir.path().join("daemon.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let state = Mutex::new(DaemonState::build(&cfg, dir.path().join("plugins")));

        // Ping → Pong (server keeps serving).
        let sock_c = sock.clone();
        let client = thread::spawn(move || {
            let mut c = UnixStream::connect(&sock_c).unwrap();
            daemon_proto::write_frame(&mut c, &DaemonRequest::Ping).unwrap();
            daemon_proto::read_frame::<_, DaemonResponse>(&mut c).unwrap()
        });
        let (server, _) = listener.accept().unwrap();
        let disp = serve_connection(server, &state, &cfg).unwrap();
        assert!(matches!(disp, Disposition::Continue));
        assert_eq!(client.join().unwrap(), DaemonResponse::Pong);

        // Shutdown → ShuttingDown (server signals teardown).
        let sock_c = sock.clone();
        let client = thread::spawn(move || {
            let mut c = UnixStream::connect(&sock_c).unwrap();
            daemon_proto::write_frame(&mut c, &DaemonRequest::Shutdown).unwrap();
            daemon_proto::read_frame::<_, DaemonResponse>(&mut c).unwrap()
        });
        let (server, _) = listener.accept().unwrap();
        let disp = serve_connection(server, &state, &cfg).unwrap();
        assert!(matches!(disp, Disposition::Shutdown));
        assert_eq!(client.join().unwrap(), DaemonResponse::ShuttingDown);

        // The accept loop's `Shutdown` cleanup (minus `process::exit`).
        remove_socket(&sock);
        assert!(!sock.exists(), "Shutdown must remove the socket file");
    }

    #[test]
    fn a_mismatched_protocol_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "").unwrap();
        let state = Mutex::new(DaemonState::build(&cfg, dir.path().join("plugins")));

        let (response, disposition) = handle_request(
            &state,
            &cfg,
            DaemonRequest::RenderV2 {
                protocol: daemon_proto::DAEMON_PROTOCOL + 1,
                region: RegionKind::Right,
                args: RenderArgsWire::default(),
            },
        );
        assert!(
            !matches!(response, DaemonResponse::Markup(_)),
            "a newer client must not receive markup from this daemon"
        );
        assert!(matches!(disposition, Disposition::Continue));
    }

    #[test]
    fn a_matching_protocol_renders() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "").unwrap();
        let state = Mutex::new(DaemonState::build(&cfg, dir.path().join("plugins")));

        let (response, _) = handle_request(
            &state,
            &cfg,
            DaemonRequest::RenderV2 {
                protocol: daemon_proto::DAEMON_PROTOCOL,
                region: RegionKind::Right,
                args: RenderArgsWire::default(),
            },
        );
        assert!(matches!(response, DaemonResponse::Markup(_)));
    }

    #[test]
    fn status_at_true_for_a_ponging_daemon_false_without_one() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("d.sock");

        // No socket yet → not alive.
        assert!(!status_at(&sock));

        // A fake daemon that answers one Ping with a Pong.
        let listener = UnixListener::bind(&sock).unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let req: DaemonRequest = daemon_proto::read_frame(&mut stream).unwrap();
            assert_eq!(req, DaemonRequest::Ping);
            daemon_proto::write_frame(&mut stream, &DaemonResponse::Pong).unwrap();
        });
        assert!(status_at(&sock));
        handle.join().unwrap();
    }
}
