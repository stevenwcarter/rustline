//! Wire protocol + length-prefixed framing for the optional persistent
//! daemon (W48). This module is self-contained: it defines the
//! request/response shapes a future daemon client/server will exchange over
//! a Unix socket, and the `write_frame`/`read_frame` helpers that encode a
//! message as a 4-byte little-endian length prefix followed by its JSON
//! encoding. No socket, listener, or client lives here yet — later tasks
//! build the server and client on top of this seam.
//!
//! `DaemonRequest::Render` mirrors the CLI's `render left|right|window`
//! subcommands (see `cli::Render`) so a daemon client can build the same
//! request shape it already builds today, just routed over a socket instead
//! of a process spawn. `RenderArgsWire` combines `cli::RegionArgs`'s
//! left/right fields and `cli::WindowArgs`'s window fields into one struct
//! covering all three `RegionKind`s — fields that don't apply to the
//! requested kind are simply absent/default.

// Every item here is `pub` for the daemon client/server tasks that consume
// this module next; until they land, nothing in the binary references them,
// so `dead_code` would otherwise fire on the whole module (the tests below
// only prove the round-trip, they don't count as a production call site).
#![allow(dead_code)]

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// A request the daemon client sends to the daemon server.
#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub enum DaemonRequest {
    /// Render one status-line region or window segment.
    Render {
        region: RegionKind,
        args: RenderArgsWire,
    },
    /// Liveness check; the server replies [`DaemonResponse::Pong`].
    Ping,
    /// Ask the server to stop; it replies [`DaemonResponse::ShuttingDown`]
    /// before exiting.
    Shutdown,
}

/// Which status-line region (or window segment) a `DaemonRequest::Render`
/// asks for — the wire equivalent of `cli::Render`'s three variants.
#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub enum RegionKind {
    Left,
    Right,
    Window,
}

/// The tmux-sourced render arguments carried over the wire. Combines
/// `cli::RegionArgs` (`session`/`window`/`pane`/`pane_path`, used by
/// `RegionKind::Left`/`Right`) and `cli::WindowArgs`
/// (`index`/`name`/`flags`/`current`, used by `RegionKind::Window`) into one
/// shape, since a single `DaemonRequest::Render` message must serve all three
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
    /// A rendered region/window's tmux markup (`DaemonRequest::Render`'s reply).
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
            DaemonRequest::Render {
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
    fn truncated_frame_is_err_not_panic() {
        let bytes = [0u8, 0, 0, 10, b'{']; // claims 10 bytes, has 1
        let r: io::Result<DaemonRequest> = read_frame(&mut &bytes[..]);
        assert!(r.is_err());
    }
}
