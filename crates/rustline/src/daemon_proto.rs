//! Wire protocol + length-prefixed framing for the optional persistent
//! daemon (W48). This module is self-contained: it defines the
//! request/response shapes a future daemon client/server will exchange over
//! a Unix socket, and the `write_frame`/`read_frame` helpers that encode a
//! message as a 4-byte little-endian length prefix followed by its JSON
//! encoding. No socket, listener, or client lives here yet — later tasks
//! build the server and client on top of this seam.
//!
//! `DaemonRequest::RenderV2` mirrors the CLI's `render left|right|window`
//! subcommands (see `cli::Render`) so a daemon client can build the same
//! request shape it already builds today, just routed over a socket instead
//! of a process spawn. `RenderArgsWire` combines `cli::RegionArgs`'s
//! left/right fields and `cli::WindowArgs`'s window fields into one struct
//! covering all three `RegionKind`s — fields that don't apply to the
//! requested kind are simply absent/default. The `protocol` field on
//! `RenderV2` is [`DAEMON_PROTOCOL`]; see its doc comment for why the wire
//! protocol is versioned via an enum variant rather than a struct field.

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The daemon wire-protocol version. Bump this whenever [`RenderArgsWire`]
/// gains or loses a field, or whenever the render semantics behind a request
/// change.
///
/// The daemon is long-lived and only reloads on the *config file's* mtime —
/// never the binary's — so after `cargo install` an old daemon keeps serving a
/// new client. Without a version it did so silently, rendering with old config
/// semantics, old theme resolution and old widget code, which the client then
/// emitted as its own output. Serde also ignores unknown struct fields, so a
/// field addition skewed silently too; only an unknown *enum variant* fails
/// loudly, which is why the version rides a new variant rather than a new
/// field on the old one.
pub const DAEMON_PROTOCOL: u32 = 1;

/// A request the daemon client sends to the daemon server.
#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub enum DaemonRequest {
    /// Render one status-line region or window segment, tagged with the
    /// client's [`DAEMON_PROTOCOL`]. A daemon predating this variant cannot
    /// deserialize it at all, drops the connection, and the client falls back
    /// to an in-process render — fail-closed by construction. A daemon that
    /// understands it still refuses a `protocol` it does not equal, so a
    /// *newer* daemon also declines an older client.
    RenderV2 {
        protocol: u32,
        region: RegionKind,
        args: RenderArgsWire,
    },
    /// Liveness check; the server replies [`DaemonResponse::Pong`].
    ///
    /// Deliberately NOT versioned, along with [`DaemonRequest::Shutdown`]: if
    /// a version skew made `daemon status`/`daemon stop` fail too, the user
    /// would have no way to stop the stale daemon that caused the problem.
    Ping,
    /// Ask the server to stop; it replies [`DaemonResponse::ShuttingDown`]
    /// before exiting.
    Shutdown,
}

/// Which status-line region (or window segment) a `DaemonRequest::RenderV2`
/// asks for — the wire equivalent of `cli::Render`'s three variants. `Copy`
/// so a caller (e.g. the bench `daemon` pass) can capture one by value in a
/// closure that's invoked repeatedly, without a per-call clone.
#[derive(Serialize, Deserialize, PartialEq, Clone, Copy, Debug)]
pub enum RegionKind {
    Left,
    Right,
    Window,
}

/// The tmux-sourced render arguments carried over the wire. Combines
/// `cli::RegionArgs` (`session`/`window`/`pane`/`pane_path`, used by
/// `RegionKind::Left`/`Right`) and `cli::WindowArgs`
/// (`index`/`name`/`flags`/`current`, used by `RegionKind::Window`) into one
/// shape, since a single `DaemonRequest::RenderV2` message must serve all three
/// region kinds — fields that don't apply to the requested kind are left at
/// their `Default` (`None`/`false`).
#[derive(Serialize, Deserialize, PartialEq, Debug, Default)]
pub struct RenderArgsWire {
    pub session: Option<String>,
    pub window: Option<String>,
    pub pane: Option<String>,
    pub pane_path: Option<String>,
    pub index: Option<String>,
    pub name: Option<String>,
    pub flags: Option<String>,
    pub current: bool,
    pub preview: bool,
}

/// The daemon server's reply to a `DaemonRequest`.
#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub enum DaemonResponse {
    /// A rendered region/window's tmux markup (`DaemonRequest::RenderV2`'s
    /// reply).
    Markup(String),
    /// Reply to `DaemonRequest::Ping`.
    Pong,
    /// Reply to `DaemonRequest::Shutdown`, sent just before the server exits.
    ShuttingDown,
}

/// A frame's length prefix caps out at 8 MiB. `read_frame` rejects any
/// claimed length above this *before* allocating a buffer for it, so a
/// corrupt or malicious peer can't force an unbounded allocation just by
/// sending a large length prefix.
const MAX_FRAME_LEN: u32 = 8 * 1024 * 1024;

/// Write `msg` to `w` as one frame: a 4-byte little-endian length prefix
/// followed by its JSON encoding.
pub fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
    let len = u32::try_from(body.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame body exceeds u32 len"))?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(&body)?;
    Ok(())
}

/// Read one frame from `r`: a 4-byte little-endian length prefix followed by
/// its JSON encoding, decoded into `T`. The claimed length is checked against
/// [`MAX_FRAME_LEN`] before any buffer for the body is allocated — an
/// oversized or truncated frame is an `Err`, never a panic or an
/// attacker-sized allocation.
pub fn read_frame<R: Read, T: DeserializeOwned>(r: &mut R) -> io::Result<T> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf);
    if len > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame length {len} exceeds cap of {MAX_FRAME_LEN} bytes"),
        ));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrips_each_variant() {
        for req in [
            DaemonRequest::Ping,
            DaemonRequest::Shutdown,
            DaemonRequest::RenderV2 {
                protocol: DAEMON_PROTOCOL,
                region: RegionKind::Right,
                args: RenderArgsWire::default(),
            },
        ] {
            let mut buf = Vec::new();
            write_frame(&mut buf, &req).unwrap();
            let back: DaemonRequest = read_frame(&mut &buf[..]).unwrap();
            assert_eq!(back, req);
        }
    }

    #[test]
    fn frame_roundtrips_each_response_variant() {
        for resp in [
            DaemonResponse::Markup("#[fg=colour1]hi".into()),
            DaemonResponse::Pong,
            DaemonResponse::ShuttingDown,
        ] {
            let mut buf = Vec::new();
            write_frame(&mut buf, &resp).unwrap();
            let back: DaemonResponse = read_frame(&mut &buf[..]).unwrap();
            assert_eq!(back, resp);
        }
    }

    #[test]
    fn truncated_frame_is_err_not_panic() {
        // Length prefix = 10 (LE) but only one body byte present, so the body's
        // `read_exact` hits `UnexpectedEof` — an `Err`, never a panic. (10 is
        // well under `MAX_FRAME_LEN`, so this exercises the truncated-body path,
        // not the length-cap path below.)
        let bytes = [10u8, 0, 0, 0, b'{'];
        let r: io::Result<DaemonRequest> = read_frame(&mut &bytes[..]);
        assert!(r.is_err());
    }

    #[test]
    fn an_old_daemon_cannot_decode_a_versioned_render_request() {
        // This is the whole mechanism: a daemon predating RenderV2 sees an unknown
        // enum variant, fails to deserialize, drops the connection, and the client
        // falls back in-process. Fail-closed by construction.
        //
        // `allow(dead_code)`: this type exists only to shape what an old daemon
        // would try to deserialize into; its fields are never read because the
        // assertion below only cares that decoding fails.
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        enum OldRequest {
            Render {
                region: RegionKind,
                args: RenderArgsWire,
            },
            Ping,
            Shutdown,
        }
        let new = DaemonRequest::RenderV2 {
            protocol: DAEMON_PROTOCOL,
            region: RegionKind::Right,
            args: RenderArgsWire::default(),
        };
        let bytes = serde_json::to_vec(&new).unwrap();
        assert!(serde_json::from_slice::<OldRequest>(&bytes).is_err());
    }

    #[test]
    fn ping_and_shutdown_stay_wire_compatible_across_the_bump() {
        // daemon status/stop must keep working against a skewed daemon —
        // otherwise the user cannot stop the stale daemon that caused the problem.
        #[derive(serde::Deserialize, PartialEq, Debug)]
        enum OldRequest {
            Render {
                region: RegionKind,
                args: RenderArgsWire,
            },
            Ping,
            Shutdown,
        }
        // Compare the decoded variant, not just `is_ok()`: these are unit
        // variants, so an `is_ok()` assertion would still pass if the two
        // swapped meanings while keeping their names — which would silently
        // turn a `daemon status` into a `daemon stop` across a version skew.
        for (req, expected) in [
            (DaemonRequest::Ping, OldRequest::Ping),
            (DaemonRequest::Shutdown, OldRequest::Shutdown),
        ] {
            let bytes = serde_json::to_vec(&req).unwrap();
            let decoded: OldRequest = serde_json::from_slice(&bytes).expect("old daemon decodes");
            assert_eq!(decoded, expected);
        }
    }

    #[test]
    fn oversized_frame_length_is_err_before_allocating() {
        // A claimed length above `MAX_FRAME_LEN` (9 MiB > the 8 MiB cap) is
        // rejected by the cap check *before* any body buffer is allocated: only
        // the 4-byte length prefix is present (no body follows), yet this is an
        // `Err`, proving the cap check fires first rather than trying to
        // `read_exact` — let alone allocate — a 9 MiB body.
        let bytes = (9u32 * 1024 * 1024).to_le_bytes();
        let r: io::Result<DaemonRequest> = read_frame(&mut &bytes[..]);
        let err = r.expect_err("over-cap length must be an error");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
