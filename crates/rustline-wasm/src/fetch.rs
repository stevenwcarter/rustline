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
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(5))
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
