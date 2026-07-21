//! The capability-checked effect functions. Each returns a structured result
//! and never panics — the host_fn wrappers just serialize these to JSON.

use crate::abi::{HttpResult, ReadResult, WriteResult};
use crate::capability::CapabilityCtx;
use crate::fetch::Fetcher;
use crate::state::{check_cap, normalize_abs, sanitize_relpath};

pub fn perform_http_get(ctx: &CapabilityCtx, url: &str, fetcher: &dyn Fetcher) -> HttpResult {
    if !ctx.allowed_urls.allows(url) {
        return HttpResult {
            ok: false,
            error: format!("url not allowed: {url}"),
            ..Default::default()
        };
    }
    match fetcher.get(url) {
        Ok((status, body)) => HttpResult {
            ok: true,
            status,
            body,
            error: String::new(),
        },
        Err(error) => HttpResult {
            ok: false,
            error,
            ..Default::default()
        },
    }
}

pub fn perform_state_read(ctx: &CapabilityCtx, relpath: &str) -> ReadResult {
    let rel = match sanitize_relpath(relpath) {
        Ok(r) => r,
        Err(error) => {
            return ReadResult {
                ok: false,
                error,
                ..Default::default()
            };
        }
    };
    let full = ctx.state_dir().join(rel);
    match std::fs::read_to_string(&full) {
        Ok(contents) => ReadResult {
            ok: true,
            exists: true,
            contents,
            error: String::new(),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ReadResult {
            ok: true,
            exists: false,
            ..Default::default()
        },
        Err(e) => ReadResult {
            ok: false,
            error: e.to_string(),
            ..Default::default()
        },
    }
}

pub fn perform_state_write(ctx: &CapabilityCtx, relpath: &str, contents: &str) -> WriteResult {
    let rel = match sanitize_relpath(relpath) {
        Ok(r) => r,
        Err(error) => return WriteResult { ok: false, error },
    };
    let dir = ctx.state_dir();
    let full = dir.join(rel);
    if let Err(error) = check_cap(&dir, &full, contents.len() as u64, ctx.max_state_bytes) {
        return WriteResult { ok: false, error };
    }
    if let Some(parent) = full.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return WriteResult {
            ok: false,
            error: e.to_string(),
        };
    }
    match std::fs::write(&full, contents.as_bytes()) {
        Ok(()) => WriteResult {
            ok: true,
            error: String::new(),
        },
        Err(e) => WriteResult {
            ok: false,
            error: e.to_string(),
        },
    }
}

pub fn perform_file_read(ctx: &CapabilityCtx, path: &str) -> ReadResult {
    let norm = match normalize_abs(path) {
        Ok(p) => p,
        Err(error) => {
            return ReadResult {
                ok: false,
                error,
                ..Default::default()
            };
        }
    };
    if !ctx.allowed_paths.allows(&norm) {
        return ReadResult {
            ok: false,
            error: format!("path not allowed: {norm}"),
            ..Default::default()
        };
    }
    match std::fs::read_to_string(&norm) {
        Ok(contents) => ReadResult {
            ok: true,
            exists: true,
            contents,
            error: String::new(),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ReadResult {
            ok: true,
            exists: false,
            ..Default::default()
        },
        Err(e) => ReadResult {
            ok: false,
            error: e.to_string(),
            ..Default::default()
        },
    }
}

pub fn perform_file_write(ctx: &CapabilityCtx, path: &str, contents: &str) -> WriteResult {
    let norm = match normalize_abs(path) {
        Ok(p) => p,
        Err(error) => return WriteResult { ok: false, error },
    };
    if !ctx.allowed_paths.allows(&norm) {
        return WriteResult {
            ok: false,
            error: format!("path not allowed: {norm}"),
        };
    }
    match std::fs::write(&norm, contents.as_bytes()) {
        Ok(()) => WriteResult {
            ok: true,
            error: String::new(),
        },
        Err(e) => WriteResult {
            ok: false,
            error: e.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityCtx;
    use rustline_core::PluginConfig;

    struct FakeFetcher(u16, &'static str);
    impl crate::fetch::Fetcher for FakeFetcher {
        fn get(&self, _url: &str) -> Result<(u16, String), String> {
            Ok((self.0, self.1.to_string()))
        }
    }
    struct DeadFetcher;
    impl crate::fetch::Fetcher for DeadFetcher {
        fn get(&self, _url: &str) -> Result<(u16, String), String> {
            Err("connection refused".into())
        }
    }

    fn ctx_with(urls: &[&str], root: std::path::PathBuf) -> CapabilityCtx {
        let pc = PluginConfig {
            allowed_urls: urls.iter().map(|s| s.to_string()).collect(),
            max_state_bytes: 16,
            ..PluginConfig::default()
        };
        CapabilityCtx::from_config("weather", &pc, root)
    }

    #[test]
    fn http_denied_when_not_allowlisted_makes_no_request() {
        let ctx = ctx_with(&[], std::env::temp_dir());
        let r = perform_http_get(&ctx, "https://wttr.in/48183", &FakeFetcher(200, "hi"));
        assert!(!r.ok);
        assert!(r.error.contains("not allowed"));
    }

    #[test]
    fn http_allowed_returns_body() {
        let ctx = ctx_with(&["https://wttr.in/*"], std::env::temp_dir());
        let r = perform_http_get(&ctx, "https://wttr.in/48183", &FakeFetcher(200, "sunny"));
        assert!(r.ok);
        assert_eq!(r.status, 200);
        assert_eq!(r.body, "sunny");
    }

    #[test]
    fn http_transport_error_reports_not_ok() {
        let ctx = ctx_with(&["https://wttr.in/*"], std::env::temp_dir());
        let r = perform_http_get(&ctx, "https://wttr.in/48183", &DeadFetcher);
        assert!(!r.ok);
        assert!(r.error.contains("refused"));
    }

    #[test]
    fn state_write_then_read_roundtrips_and_enforces_cap() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with(&[], root.path().to_path_buf());
        let w = perform_state_write(&ctx, "weather.json", "0123456789"); // 10 bytes, cap 16
        assert!(w.ok, "{:?}", w.error);
        let r = perform_state_read(&ctx, "weather.json");
        assert!(r.ok && r.exists);
        assert_eq!(r.contents, "0123456789");
        // read of an absent file: ok but exists=false
        let miss = perform_state_read(&ctx, "nope.json");
        assert!(miss.ok && !miss.exists);
        // a second big write over cap is refused
        let over = perform_state_write(&ctx, "big.json", "0123456789ABCDEF01"); // 18 > 16
        assert!(!over.ok);
        assert!(over.error.contains("quota"));
    }

    #[test]
    fn state_write_rejects_traversal() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with(&[], root.path().to_path_buf());
        let w = perform_state_write(&ctx, "../escape", "x");
        assert!(!w.ok);
        assert!(w.error.contains("traversal"));
    }

    #[test]
    fn file_read_denied_when_not_allowlisted() {
        let ctx = ctx_with(&[], std::env::temp_dir());
        let r = perform_file_read(&ctx, "/etc/hostname");
        assert!(!r.ok);
        assert!(r.error.contains("not allowed"));
    }
}
