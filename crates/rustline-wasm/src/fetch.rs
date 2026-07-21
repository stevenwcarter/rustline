//! The HTTP seam. `perform_http_get` takes a `Fetcher` so its gating logic is
//! testable without network; `UreqFetcher` is the real blocking rustls client.

use std::time::Duration;

/// A blocking HTTP GET. Returns `(status, body)` on a completed response
/// (including non-2xx), or `Err(message)` on transport failure.
pub trait Fetcher {
    fn get(&self, url: &str) -> Result<(u16, String), String>;
}

pub struct UreqFetcher;

impl Fetcher for UreqFetcher {
    fn get(&self, url: &str) -> Result<(u16, String), String> {
        // redirects(0): do NOT follow 3xx. The allowlist is only checked against
        // the *requested* URL, so following a redirect to an off-allowlist host
        // would escape the allowlist and hand that host's body to the guest.
        // With redirects(0), ureq 2.12 returns the 3xx response as-is via `Ok`
        // (status < 400 is never mapped to `Error::Status`), so the `Ok` arm
        // below surfaces `(3xx, body)` to the already-gated caller.
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(5))
            .redirects(0)
            .build();
        match agent.get(url).call() {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.into_string().map_err(|e| e.to_string())?;
                Ok((status, body))
            }
            Err(ureq::Error::Status(code, resp)) => {
                Ok((code, resp.into_string().unwrap_or_default()))
            }
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    #[test]
    fn does_not_follow_redirect_off_allowlist() {
        // A tiny mock that 302s to a dead port. If UreqFetcher followed the
        // redirect it would hit 127.0.0.1:1 and error out; instead it must hand
        // the 302 straight back so the allowlist stays the sole authority on
        // which host's body reaches the guest.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf); // drain the request line/headers
            stream
                .write_all(
                    b"HTTP/1.1 302 Found\r\n\
                      Location: http://127.0.0.1:1/elsewhere\r\n\
                      Content-Length: 0\r\n\
                      Connection: close\r\n\r\n",
                )
                .unwrap();
        });

        let result = UreqFetcher.get(&format!("http://{addr}/"));
        server.join().unwrap();

        match result {
            Ok((status, _)) => assert_eq!(status, 302, "redirect must not be followed"),
            Err(e) => panic!("redirect was followed or otherwise errored: {e}"),
        }
    }
}
