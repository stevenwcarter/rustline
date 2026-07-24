//! Daemon client (W48): the piece that makes the optional persistent daemon
//! safe to depend on. `try_render` tries the daemon's Unix socket and
//! returns `None` on ANY failure — socket absent, connect/timeout error,
//! malformed reply, or a non-`Markup` response — so the caller always has a
//! safe in-process render to fall back to (invariant N2, "never break the
//! bar"). No fallback logic lives here; that's the caller's job (Task 12
//! wires this into the render arms).

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::daemon_proto::{self, DaemonRequest, DaemonResponse, RegionKind, RenderArgsWire};

/// How long to wait on the daemon socket before giving up and falling back
/// to an in-process render. Short on purpose: a hung/overloaded daemon must
/// never stall the status line longer than a normal render would take.
const SOCKET_TIMEOUT: Duration = Duration::from_millis(250);

/// Resolve the daemon's Unix socket path: `$XDG_RUNTIME_DIR/rustline/daemon.sock`,
/// falling back to `<state_root>/daemon.sock` when `XDG_RUNTIME_DIR` is unset —
/// or set but empty, which the XDG spec says to treat as unset.
pub fn daemon_socket_path() -> PathBuf {
    match std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|d| !d.is_empty())
    {
        Some(dir) => PathBuf::from(dir).join("rustline").join("daemon.sock"),
        None => rustline_wasm::state_root().join("daemon.sock"),
    }
}

/// Try to render `region` via the daemon at the default socket path. `None`
/// on any failure, in which case the caller should fall back to its normal
/// in-process render.
pub fn try_render(region: RegionKind, args: RenderArgsWire) -> Option<String> {
    try_render_at(&daemon_socket_path(), region, args)
}

/// Try to render `region` via the daemon listening at `sock`. `None` if the
/// socket doesn't exist, the connection/round-trip fails or times out, or
/// the daemon replies with anything other than [`DaemonResponse::Markup`].
/// Never panics and never propagates an error — this is the seam that keeps
/// a dead/misbehaving daemon from ever breaking the bar.
///
/// `pub(crate)` (rather than private) so the bench `daemon` pass can time a
/// round-trip against an explicit socket too — the same seam this module's
/// own tests use to point at a fake in-thread daemon.
pub(crate) fn try_render_at(
    sock: &Path,
    region: RegionKind,
    args: RenderArgsWire,
) -> Option<String> {
    // One `stat` before attempting to connect: near-zero overhead when the
    // daemon isn't running (the common case today), and avoids paying a
    // connect-timeout for a socket that was never bound.
    if !sock.exists() {
        return None;
    }
    let mut stream = UnixStream::connect(sock).ok()?;
    stream.set_read_timeout(Some(SOCKET_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(SOCKET_TIMEOUT)).ok()?;
    daemon_proto::write_frame(&mut stream, &DaemonRequest::Render { region, args }).ok()?;
    let response: DaemonResponse = daemon_proto::read_frame(&mut stream).ok()?;
    match response {
        DaemonResponse::Markup(markup) => Some(markup),
        DaemonResponse::Pong | DaemonResponse::ShuttingDown => None,
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    #[test]
    fn try_render_returns_none_with_no_socket() {
        assert!(
            try_render_at(
                &PathBuf::from("/no/such/rustline.sock"),
                RegionKind::Right,
                RenderArgsWire::default(),
            )
            .is_none()
        );
    }

    #[test]
    fn try_render_reads_markup_from_a_fake_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("d.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _req: DaemonRequest = daemon_proto::read_frame(&mut stream).unwrap();
            daemon_proto::write_frame(&mut stream, &DaemonResponse::Markup("OK".into())).unwrap();
        });

        let out = try_render_at(&sock, RegionKind::Right, RenderArgsWire::default());
        handle.join().unwrap();

        assert_eq!(out.as_deref(), Some("OK"));
    }
}
