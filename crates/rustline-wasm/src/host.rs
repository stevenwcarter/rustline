//! Extism instantiation: bind the capability-gated host functions to each
//! plugin instance's `CapabilityCtx`, and wrap the instance as a `Widget` that
//! degrades to empty segments on any error.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use extism::{Manifest, PTR, PluginBuilder, UserData, Wasm, host_fn};
use rustline_core::{Context, Segment, Widget};

use crate::abi::{RenderInput, parse_render_output};
use crate::capability::CapabilityCtx;
use crate::fetch::UreqFetcher;
use crate::perform::{
    perform_file_read, perform_file_write, perform_http_get, perform_http_get_cached,
    perform_state_read, perform_state_write,
};

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string())
}

host_fn!(rl_http_get(user_data: CapabilityCtx; url: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    Ok(json(&perform_http_get(&ctx, &url, &UreqFetcher)))
});

host_fn!(rl_http_get_cached(user_data: CapabilityCtx; url: String, ttl_secs: String, now: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    let ttl: i64 = ttl_secs.parse().unwrap_or(0);
    Ok(json(&perform_http_get_cached(&ctx, &url, ttl, &now, &UreqFetcher)))
});

host_fn!(rl_state_read(user_data: CapabilityCtx; relpath: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    Ok(json(&perform_state_read(&ctx, &relpath)))
});

host_fn!(rl_state_write(user_data: CapabilityCtx; relpath: String, contents: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    Ok(json(&perform_state_write(&ctx, &relpath, &contents)))
});

host_fn!(rl_file_read(user_data: CapabilityCtx; path: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    Ok(json(&perform_file_read(&ctx, &path)))
});

host_fn!(rl_file_write(user_data: CapabilityCtx; path: String, contents: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    Ok(json(&perform_file_write(&ctx, &path, &contents)))
});

/// Build an Extism plugin from wasm bytes with wasi off, fuel + timeout +
/// memory caps, and the six capability-gated host functions bound to this
/// instance's `CapabilityCtx`.
pub fn build_plugin(wasm: &[u8], ctx: CapabilityCtx) -> Result<extism::Plugin, extism::Error> {
    let ud = UserData::new(ctx);
    let manifest = Manifest::new([Wasm::data(wasm.to_vec())])
        .with_timeout(Duration::from_secs(10))
        .with_memory_max(256); // 256 pages ≈ 16 MB
    PluginBuilder::new(manifest)
        .with_wasi(false)
        .with_fuel_limit(500_000_000)
        .with_function("rl_http_get", [PTR], [PTR], ud.clone(), rl_http_get)
        .with_function(
            "rl_http_get_cached",
            [PTR, PTR, PTR],
            [PTR],
            ud.clone(),
            rl_http_get_cached,
        )
        .with_function("rl_state_read", [PTR], [PTR], ud.clone(), rl_state_read)
        .with_function(
            "rl_state_write",
            [PTR, PTR],
            [PTR],
            ud.clone(),
            rl_state_write,
        )
        .with_function("rl_file_read", [PTR], [PTR], ud.clone(), rl_file_read)
        .with_function(
            "rl_file_write",
            [PTR, PTR],
            [PTR],
            ud.clone(),
            rl_file_write,
        )
        .build()
}

/// A discovered WASM plugin, rendered as a widget. Cheap to clone (shares the
/// instance behind an `Arc<Mutex<…>>`); any error/timeout/malformed output
/// degrades to empty segments so a plugin never breaks the bar.
#[derive(Clone)]
pub struct WasmWidget {
    plugin: Arc<Mutex<extism::Plugin>>,
    options: Arc<serde_json::Value>,
    name: Arc<str>,
}

impl WasmWidget {
    pub fn new(plugin: extism::Plugin, options: serde_json::Value, name: &str) -> Self {
        Self {
            plugin: Arc::new(Mutex::new(plugin)),
            options: Arc::new(options),
            name: Arc::from(name),
        }
    }
}

impl Widget for WasmWidget {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let input = RenderInput {
            context: ctx,
            config: &self.options,
        };
        let payload = match serde_json::to_string(&input) {
            Ok(p) => p,
            Err(error) => {
                tracing::warn!(%error, "failed to serialize render input");
                return Vec::new();
            }
        };
        let mut plugin = match self.plugin.lock() {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };
        match plugin.call::<&str, &str>("render", &payload) {
            Ok(out) => parse_render_output(out),
            Err(error) => {
                tracing::warn!(%error, "plugin render failed, rendering empty");
                Vec::new()
            }
        }
    }

    fn range_name(&self) -> Option<&str> {
        // A plugin is clickable when its name fits tmux's 15-byte user-range
        // limit; the guest decides whether to honor `context.toggled`.
        (self.name.len() <= 15).then_some(&*self.name)
    }
}

#[cfg(test)]
mod tests {
    use rustline_core::{Config, Context, Registry};

    use crate::abi::{RenderInput, parse_render_output};

    /// A minimal `Context` with `toggled` set to `{name}`, for pinning the
    /// host→guest seam that carries click-toggle state across the wasm
    /// boundary. Field list copied from `rustline-core`'s test `ctx()`.
    fn sample_ctx_with_toggle(name: &str) -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/".into(),
            home: "/home/steve".into(),
            hostname: "h".into(),
            loadavg: None,
            now: chrono::Local::now(),
            window: None,
            interfaces: Vec::new(),
            battery: None,
            cpu: None,
            memory: None,
            os: String::new(),
            arch: String::new(),
            toggled: std::collections::BTreeSet::from([name.to_string()]),
        }
    }

    #[test]
    fn render_input_serializes_toggled_for_guests() {
        // Build a minimal Context with a toggled entry and assert the guest
        // payload carries it — this is the seam a plugin depends on to honor
        // toggling.
        let json = serde_json::to_string(&RenderInput {
            context: &sample_ctx_with_toggle("weather"),
            config: &serde_json::json!({}),
        })
        .unwrap();
        assert!(
            json.contains("\"toggled\""),
            "payload carries toggled: {json}"
        );
        assert!(
            json.contains("weather"),
            "payload carries the toggled name: {json}"
        );
    }

    #[test]
    fn parse_output_degrades_on_malformed() {
        assert!(parse_render_output("not json").is_empty());
        let good = r#"[{"text":"x","style":{"fg":null,"bg":null,"bold":false}}]"#;
        assert_eq!(parse_render_output(good).len(), 1);
    }

    #[test]
    fn register_plugins_missing_dir_is_noop() {
        let mut reg = Registry::new();
        crate::register_plugins(
            &mut reg,
            &Config::default(),
            std::path::Path::new("/no/such/dir"),
            &["weather".into()],
        );
        assert!(!reg.contains("weather"));
    }

    #[test]
    fn register_plugins_skips_not_needed_and_garbage_wasm() {
        let dir = tempfile::tempdir().unwrap();
        // a garbage .wasm that IS needed -> instantiation fails -> skipped, no panic
        std::fs::write(dir.path().join("weather.wasm"), b"not real wasm").unwrap();
        // a .wasm that is NOT in `needed` -> never touched
        std::fs::write(dir.path().join("other.wasm"), b"nope").unwrap();
        let mut reg = Registry::new();
        crate::register_plugins(
            &mut reg,
            &Config::default(),
            dir.path(),
            &["weather".into()],
        );
        assert!(!reg.contains("weather"));
        assert!(!reg.contains("other"));
    }
}
