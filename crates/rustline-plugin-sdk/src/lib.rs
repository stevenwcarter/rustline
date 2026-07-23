//! rustline-plugin-sdk: the guest-side crate a rustline WASM plugin depends on.
//!
//! It bundles everything a plugin needs so it imports **one** crate rather
//! than hand-rolling the Extism glue:
//!
//! - **Typed host-capability wrappers** ([`http_get`], [`http_get_cached`],
//!   [`state_read`], [`state_write`], [`file_read`], [`file_write`], [`log`])
//!   that call the host functions and decode their JSON responses into typed
//!   result structs, returning a [`Result`] instead of an untyped
//!   `serde_json::Value` the plugin must walk by hand.
//! - **Re-exports of the shared wire types** ([`GuestRender`], [`WireContext`],
//!   [`Segment`], [`Style`], [`Color`]) from `rustline-abi`.
//! - The **[`active_format`]** toggle helper (which format string is live given
//!   the click-toggle set).
//! - The **[`export_plugin!`]** macro, which wires a plugin's `name()`,
//!   `render()`, and `abi_version()` Extism exports from a single line.
//!
//! The capability wrappers link real host imports **only on `wasm32`**; on the
//! host target they degrade to [`HostError::Unavailable`] so a plugin's pure
//! logic (and this crate's own tests) still compile and run under `cargo test`.

use serde::{Deserialize, Serialize};

pub use rustline_abi;
pub use rustline_abi::{Color, GuestRender, Segment, Style, WireContext};

// Re-exported so [`export_plugin!`]'s expansion can name the Extism PDK without
// the plugin author importing it, while the host-target build (where the PDK is
// unavailable) simply never expands the macro's wasm-only body.
#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub use extism_pdk;

/// Why a host-capability call did not yield a decoded result.
///
/// On the host target every wrapper returns [`HostError::Unavailable`] (there
/// is no host to call); on `wasm32` the variants distinguish a failed host call
/// from a response whose JSON did not decode into the expected result struct.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// The host function itself returned an error (e.g. a trap or an
    /// out-of-fuel condition surfaced by the runtime).
    #[error("host call failed: {0}")]
    Call(String),
    /// The host returned a payload that did not decode into the result type.
    #[error("malformed host response: {0}")]
    Decode(String),
    /// No host is reachable — the host-target stub. A plugin's pure logic can
    /// still be exercised under `cargo test`; the effect just isn't performed.
    #[error("host function unavailable on this target")]
    Unavailable,
}

/// The severity a plugin attaches to a [`log`] line. Maps to the host's
/// `tracing` levels; the host degrades an unrecognized level to `info`, but
/// this enum keeps a plugin from ever emitting one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    /// The wire string the host's `rl_log` expects.
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }
}

/// Result of a plain (uncached) [`http_get`]. `ok` means the transport
/// completed for *any* status (including non-2xx), not that the response was
/// 2xx — inspect `status` for that. Guest-side mirror of the host's
/// `rustline_wasm::abi::HttpResult`; the weather e2e suite pins the two shapes
/// together end-to-end.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HttpResult {
    pub ok: bool,
    pub status: u16,
    pub body: String,
    pub error: String,
}

/// Result of a TTL-cached [`http_get_cached`]. `ok` means "a usable body is
/// present" (fresh OR stale), not "transport succeeded"; `stale` distinguishes
/// them. Guest-side mirror of the host's `CachedHttpResult`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CachedHttpResult {
    pub ok: bool,
    pub status: u16,
    pub body: String,
    pub error: String,
    pub stale: bool,
    pub age_secs: i64,
}

/// Result of a [`state_read`]/[`file_read`]. `ok=true` with `exists=false` is a
/// successful read of a missing path (not an error); `error` carries the
/// message only when `ok` is false. Guest-side mirror of the host's
/// `ReadResult`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReadResult {
    pub ok: bool,
    pub exists: bool,
    pub contents: String,
    pub error: String,
}

/// Result of a [`state_write`]/[`file_write`]. `ok` is true on success;
/// otherwise `error` carries the failure message. Guest-side mirror of the
/// host's `WriteResult`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WriteResult {
    pub ok: bool,
    pub error: String,
}

/// The host's [`ABI_VERSION`](rustline_abi::ABI_VERSION), stringified. The
/// [`export_plugin!`] macro emits this from the guest's `abi_version()` export
/// so the host's registration handshake sees a matching version and takes the
/// `Register` (not `RegisterLegacy`) path.
pub fn abi_version_string() -> String {
    rustline_abi::ABI_VERSION.to_string()
}

/// Decode a host response payload into a typed result struct.
///
/// Pure and host-testable: the capability wrappers are just a host call feeding
/// this. A malformed payload becomes [`HostError::Decode`] rather than a panic.
fn decode<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T, HostError> {
    serde_json::from_str(raw).map_err(|e| HostError::Decode(e.to_string()))
}

/// The active format string for a plugin given the click-toggle set: its `alt`
/// view when `alt` is non-empty **and** `name` is in `ctx.toggled`, else the
/// normal `format`. Mirrors `rustline_core`'s built-in `active_format` so a
/// plugin toggles identically to a built-in widget (see invariant #7 — one
/// name end-to-end).
pub fn active_format<'a>(ctx: &WireContext, name: &str, format: &'a str, alt: &'a str) -> &'a str {
    if !alt.is_empty() && ctx.toggled.contains(name) {
        alt
    } else {
        format
    }
}

/// Parse a `render` input payload into a typed [`GuestRender`] and hand it to
/// `f`. A malformed payload degrades to an empty segment vec — never break the
/// bar (invariant N2). [`export_plugin!`] calls this so a plugin's `render`
/// function only ever sees a decoded [`GuestRender`].
pub fn render_with<F>(input: &str, f: F) -> Vec<Segment>
where
    F: FnOnce(&GuestRender) -> Vec<Segment>,
{
    match serde_json::from_str::<GuestRender>(input) {
        Ok(render) => f(&render),
        Err(_) => Vec::new(),
    }
}

/// Wire a plugin's three Extism exports — `name()`, `render()`, and
/// `abi_version()` — from one invocation.
///
/// `render` names a `fn(&`[`GuestRender`]`) -> Vec<`[`Segment`]`>`; the macro
/// handles decoding the JSON input (malformed → empty render) and encoding the
/// output. The exports are emitted **only on `wasm32`**, so an unconditional
/// invocation is inert on the host target where the pure logic is unit-tested.
///
/// ```ignore
/// fn render(input: &rustline_plugin_sdk::GuestRender) -> Vec<rustline_plugin_sdk::Segment> {
///     vec![rustline_plugin_sdk::Segment::new("hi")]
/// }
/// rustline_plugin_sdk::export_plugin!(name: "myplugin", render: render);
/// ```
#[macro_export]
macro_rules! export_plugin {
    (name: $name:expr, render: $render:path $(,)?) => {
        // Resolved in the invocation scope (where `$name`/`$render` are
        // visible), then referenced via `super::` from the exports module. The
        // fn-pointer type pins the plugin's `render` signature at compile time.
        #[cfg(target_arch = "wasm32")]
        #[doc(hidden)]
        const __RUSTLINE_PLUGIN_NAME: &str = $name;

        #[cfg(target_arch = "wasm32")]
        #[doc(hidden)]
        const __RUSTLINE_PLUGIN_RENDER: fn(
            &$crate::GuestRender,
        ) -> ::std::vec::Vec<$crate::Segment> = $render;

        #[cfg(target_arch = "wasm32")]
        #[doc(hidden)]
        mod __rustline_plugin_exports {
            #[$crate::extism_pdk::plugin_fn]
            pub fn name() -> $crate::extism_pdk::FnResult<::std::string::String> {
                ::core::result::Result::Ok(::std::string::String::from(
                    super::__RUSTLINE_PLUGIN_NAME,
                ))
            }

            #[$crate::extism_pdk::plugin_fn]
            pub fn abi_version() -> $crate::extism_pdk::FnResult<::std::string::String> {
                ::core::result::Result::Ok($crate::abi_version_string())
            }

            #[$crate::extism_pdk::plugin_fn]
            pub fn render(
                input: ::std::string::String,
            ) -> $crate::extism_pdk::FnResult<
                $crate::extism_pdk::Json<::std::vec::Vec<$crate::Segment>>,
            > {
                ::core::result::Result::Ok($crate::extism_pdk::Json($crate::render_with(
                    &input,
                    super::__RUSTLINE_PLUGIN_RENDER,
                )))
            }
        }
    };
}

// The real host imports, linked only on the wasm target. Each wrapper is a
// thin `host call -> decode` pair; the decode is the pure, host-tested seam.
#[cfg(target_arch = "wasm32")]
mod raw {
    use super::HostError;
    use extism_pdk::host_fn;

    #[host_fn]
    extern "ExtismHost" {
        fn rl_http_get(url: String) -> String;
        fn rl_http_get_cached(url: String, ttl_secs: String, now: String) -> String;
        fn rl_state_read(relpath: String) -> String;
        fn rl_state_write(relpath: String, contents: String) -> String;
        fn rl_file_read(path: String) -> String;
        fn rl_file_write(path: String, contents: String) -> String;
        fn rl_log(level: String, msg: String) -> String;
    }

    fn call(res: Result<String, extism_pdk::Error>) -> Result<String, HostError> {
        res.map_err(|e| HostError::Call(e.to_string()))
    }

    pub fn http_get(url: &str) -> Result<String, HostError> {
        call(unsafe { rl_http_get(url.to_string()) })
    }

    pub fn http_get_cached(url: &str, ttl_secs: &str, now: &str) -> Result<String, HostError> {
        call(unsafe { rl_http_get_cached(url.to_string(), ttl_secs.to_string(), now.to_string()) })
    }

    pub fn state_read(relpath: &str) -> Result<String, HostError> {
        call(unsafe { rl_state_read(relpath.to_string()) })
    }

    pub fn state_write(relpath: &str, contents: &str) -> Result<String, HostError> {
        call(unsafe { rl_state_write(relpath.to_string(), contents.to_string()) })
    }

    pub fn file_read(path: &str) -> Result<String, HostError> {
        call(unsafe { rl_file_read(path.to_string()) })
    }

    pub fn file_write(path: &str, contents: &str) -> Result<String, HostError> {
        call(unsafe { rl_file_write(path.to_string(), contents.to_string()) })
    }

    pub fn log(level: &str, msg: &str) {
        let _ = unsafe { rl_log(level.to_string(), msg.to_string()) };
    }
}

// Host-target stubs: no host to call, so every effect is `Unavailable` and
// `log` is a no-op. Keeps a plugin's pure logic (and this crate's tests)
// compiling under `cargo test`.
#[cfg(not(target_arch = "wasm32"))]
mod raw {
    use super::HostError;

    pub fn http_get(_url: &str) -> Result<String, HostError> {
        Err(HostError::Unavailable)
    }
    pub fn http_get_cached(_url: &str, _ttl_secs: &str, _now: &str) -> Result<String, HostError> {
        Err(HostError::Unavailable)
    }
    pub fn state_read(_relpath: &str) -> Result<String, HostError> {
        Err(HostError::Unavailable)
    }
    pub fn state_write(_relpath: &str, _contents: &str) -> Result<String, HostError> {
        Err(HostError::Unavailable)
    }
    pub fn file_read(_path: &str) -> Result<String, HostError> {
        Err(HostError::Unavailable)
    }
    pub fn file_write(_path: &str, _contents: &str) -> Result<String, HostError> {
        Err(HostError::Unavailable)
    }
    pub fn log(_level: &str, _msg: &str) {}
}

/// Plain (uncached) HTTP GET through the host's `rl_http_get` capability (gated
/// by the plugin's `allowed_urls`). `ok` on the returned [`HttpResult`] means
/// the transport completed for any status — check `status` for 2xx yourself.
pub fn http_get(url: &str) -> Result<HttpResult, HostError> {
    decode(&raw::http_get(url)?)
}

/// TTL-cached HTTP GET through the host's `rl_http_get_cached` capability. The
/// host owns the cache (keyed by URL) and fetches at most once per `ttl_secs`,
/// serving a fresh or last-good-stale body; `now` is the current instant as an
/// RFC3339 string (take it from `context.now`).
pub fn http_get_cached(url: &str, ttl_secs: i64, now: &str) -> Result<CachedHttpResult, HostError> {
    decode(&raw::http_get_cached(url, &ttl_secs.to_string(), now)?)
}

/// Read from the plugin's sandboxed state dir via `rl_state_read`. `relpath` is
/// relative to that dir (absolute paths and `..` are rejected host-side).
pub fn state_read(relpath: &str) -> Result<ReadResult, HostError> {
    decode(&raw::state_read(relpath)?)
}

/// Write to the plugin's sandboxed, quota-bounded state dir via
/// `rl_state_write`.
pub fn state_write(relpath: &str, contents: &str) -> Result<WriteResult, HostError> {
    decode(&raw::state_write(relpath, contents)?)
}

/// Read an arbitrary file via `rl_file_read` (gated by the plugin's
/// `allowed_paths`).
pub fn file_read(path: &str) -> Result<ReadResult, HostError> {
    decode(&raw::file_read(path)?)
}

/// Write an arbitrary file via `rl_file_write` (gated by the plugin's
/// `allowed_paths`).
pub fn file_write(path: &str, contents: &str) -> Result<WriteResult, HostError> {
    decode(&raw::file_write(path, contents)?)
}

/// Emit a log line through the host's `tracing` subscriber via the
/// capability-free `rl_log`. Fire-and-forget: a plugin logs its own failure
/// paths without ever breaking the bar.
pub fn log(level: LogLevel, msg: &str) {
    raw::log(level.as_str(), msg);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `WireContext` with the given toggled names, for the toggle-helper
    /// tests. Parses a minimal literal so the many other fields come along for
    /// free (and stay pinned to `WireContext`'s serde shape).
    fn wire_ctx(toggled: &[&str]) -> WireContext {
        let list = toggled
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(",");
        let json = format!(
            r#"{{
                "session_name":"0","window_index":"0","pane_index":"0",
                "pane_current_path":"/","home":"/home/x","hostname":"h",
                "loadavg":null,"now":"2026-07-23T00:00:00-00:00","window":null,
                "interfaces":[],"battery":null,"cpu":null,"memory":null,
                "git":null,"disk":null,"os":"linux","arch":"x86_64",
                "toggled":[{list}]
            }}"#
        );
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn active_format_picks_alt_only_when_toggled_and_alt_nonempty() {
        assert_eq!(active_format(&wire_ctx(&["cpu"]), "cpu", "F", "A"), "A");
        assert_eq!(active_format(&wire_ctx(&[]), "cpu", "F", "A"), "F"); // not toggled
        assert_eq!(active_format(&wire_ctx(&["cpu"]), "cpu", "F", ""), "F"); // empty alt
        assert_eq!(active_format(&wire_ctx(&["mem"]), "cpu", "F", "A"), "F"); // other toggled
    }

    #[test]
    fn abi_version_string_matches_the_abi_constant() {
        assert_eq!(abi_version_string(), rustline_abi::ABI_VERSION.to_string());
    }

    #[test]
    fn http_result_decodes_representative_host_payload() {
        let raw = r#"{"ok":true,"status":200,"body":"hello","error":""}"#;
        let r: HttpResult = decode(raw).unwrap();
        assert!(r.ok);
        assert_eq!(r.status, 200);
        assert_eq!(r.body, "hello");
        assert!(r.error.is_empty());
    }

    #[test]
    fn cached_http_result_decodes_and_round_trips() {
        let src = CachedHttpResult {
            ok: true,
            status: 200,
            body: "72".into(),
            error: String::new(),
            stale: true,
            age_secs: 42,
        };
        let json = serde_json::to_string(&src).unwrap();
        let back: CachedHttpResult = decode(&json).unwrap();
        assert_eq!(back, src);
    }

    #[test]
    fn read_result_missing_file_is_ok_but_absent() {
        let raw = r#"{"ok":true,"exists":false,"contents":"","error":""}"#;
        let r: ReadResult = decode(raw).unwrap();
        assert!(r.ok);
        assert!(!r.exists);
    }

    #[test]
    fn write_result_carries_error_when_not_ok() {
        let raw = r#"{"ok":false,"error":"quota exceeded"}"#;
        let r: WriteResult = decode(raw).unwrap();
        assert!(!r.ok);
        assert_eq!(r.error, "quota exceeded");
    }

    #[test]
    fn result_decode_tolerates_missing_fields_via_serde_default() {
        // Forward-compat: a host that drops a field (or an older/newer shape)
        // still decodes — `#[serde(default)]` fills the rest.
        let r: HttpResult = decode(r#"{"ok":true}"#).unwrap();
        assert!(r.ok);
        assert_eq!(r.status, 0);
        assert!(r.body.is_empty());
    }

    #[test]
    fn decode_malformed_is_decode_error_not_panic() {
        let err = decode::<HttpResult>("not json").unwrap_err();
        assert!(matches!(err, HostError::Decode(_)));
    }

    #[test]
    fn render_with_parses_guest_render_and_calls_closure() {
        let json = r#"{
            "context":{
                "session_name":"0","window_index":"0","pane_index":"0",
                "pane_current_path":"/","home":"/home/x","hostname":"h",
                "loadavg":null,"now":"2026-07-23T00:00:00-00:00","window":null,
                "interfaces":[],"battery":null,"cpu":null,"memory":null,
                "git":null,"disk":null,"os":"linux","arch":"x86_64",
                "toggled":["weather"]
            },
            "config":{"format":"hi"}
        }"#;
        let segs = render_with(json, |g| {
            assert!(g.context.toggled.contains("weather"));
            let fmt = g.config["format"].as_str().unwrap_or("");
            vec![Segment::new(fmt)]
        });
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "hi");
    }

    #[test]
    fn render_with_malformed_input_is_empty_never_panics() {
        let segs = render_with("not json", |_| vec![Segment::new("unreachable")]);
        assert!(segs.is_empty());
    }

    #[test]
    fn host_wrappers_are_unavailable_on_the_host_target() {
        // The host-target stubs let a plugin's pure logic exercise the wrappers
        // under `cargo test` without a real host; every effect is `Unavailable`.
        assert!(matches!(http_get("http://x"), Err(HostError::Unavailable)));
        assert!(matches!(
            http_get_cached("http://x", 1800, "2026-07-23T00:00:00-00:00"),
            Err(HostError::Unavailable)
        ));
        assert!(matches!(state_read("k"), Err(HostError::Unavailable)));
        assert!(matches!(state_write("k", "v"), Err(HostError::Unavailable)));
        assert!(matches!(
            file_read("/etc/hostname"),
            Err(HostError::Unavailable)
        ));
        assert!(matches!(
            file_write("/tmp/x", "v"),
            Err(HostError::Unavailable)
        ));
        log(LogLevel::Warn, "no-op on host"); // must not panic
    }

    #[test]
    fn log_level_wire_strings() {
        assert_eq!(LogLevel::Error.as_str(), "error");
        assert_eq!(LogLevel::Warn.as_str(), "warn");
        assert_eq!(LogLevel::Info.as_str(), "info");
        assert_eq!(LogLevel::Debug.as_str(), "debug");
        assert_eq!(LogLevel::Trace.as_str(), "trace");
    }
}
