#![cfg(feature = "wasm-e2e")]
//! End-to-end: load the real weather.wasm, point it at a local mock wttr.in,
//! and assert the cache makes exactly one HTTP call, with stale fallback on
//! failure and no cross-zip leakage. Run via `just test-wasm` (requires the
//! wasm target + `just build-weather`).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rustline_core::{Context, PluginConfig, Widget};
use rustline_wasm::capability::CapabilityCtx;
use rustline_wasm::{WasmWidget, build_plugin};

const WTTR_BODY: &str = r#"{"current_condition":[{"temp_F":"72","weatherCode":"113","weatherDesc":[{"value":"Sunny"}]}]}"#;

/// A one-shot-per-connection HTTP mock; counts hits.
fn spawn_mock(hits: Arc<AtomicUsize>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            hits.fetch_add(1, Ordering::SeqCst);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                WTTR_BODY.len(),
                WTTR_BODY
            );
            let _ = s.write_all(resp.as_bytes());
        }
    });
    format!("http://{addr}")
}

fn weather_wasm() -> Vec<u8> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/weather/target/wasm32-unknown-unknown/release/weather.wasm"
    );
    std::fs::read(path).expect("run `just build-weather` first")
}

fn ctx_now(rfc3339: &str) -> Context {
    Context {
        session_name: "0".into(),
        window_index: "0".into(),
        pane_index: "0".into(),
        pane_current_path: "/".into(),
        home: "/home/x".into(),
        hostname: "h".into(),
        loadavg: None,
        now: chrono::DateTime::parse_from_rfc3339(rfc3339)
            .unwrap()
            .with_timezone(&chrono::Local),
        window: None,
        interfaces: Vec::new(),
    }
}

fn build_widget(api_base: &str, state_root: std::path::PathBuf, zip: &str) -> WasmWidget {
    let pc = PluginConfig {
        allowed_urls: vec!["http://127.0.0.1:*/*".into()],
        ..PluginConfig::default()
    };
    let cap = CapabilityCtx::from_config("weather", &pc, state_root);
    let plugin = build_plugin(&weather_wasm(), cap).unwrap();
    let options = serde_json::json!({ "zip": zip, "api_base": api_base, "refresh_secs": 1800 });
    WasmWidget::new(plugin, options)
}

/// Seed a cache entry directly in the plugin's state dir (`<root>/weather/weather.json`).
/// Field names match what the guest writes (`plugins/weather/src/lib.rs`).
fn seed_cache(state_root: &std::path::Path, fetched_at: &str, zip: &str, temp_f: &str) {
    let dir = state_root.join("weather");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("weather.json"),
        serde_json::json!({
            "fetched_at": fetched_at,
            "zip": zip,
            "temp_f": temp_f,
            "code": "113",
            "desc": "Sunny",
        })
        .to_string(),
    )
    .unwrap();
}

#[test]
fn caches_within_refresh_window_one_http_call() {
    let hits = Arc::new(AtomicUsize::new(0));
    let base = spawn_mock(hits.clone());
    let state = tempfile::tempdir().unwrap();

    let w = build_widget(&base, state.path().to_path_buf(), "48183");
    // first render: fetches + caches
    let s1 = w.render(&ctx_now("2026-07-20T12:00:00-04:00"));
    assert!(s1[0].text.contains("72"), "temp rendered: {s1:?}");
    // second render 10 min later: served from cache, no new HTTP hit
    let s2 = w.render(&ctx_now("2026-07-20T12:10:00-04:00"));
    assert!(s2[0].text.contains("72"));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "exactly one fetch within the window"
    );
}

#[test]
fn stale_cache_used_when_fetch_fails() {
    let state = tempfile::tempdir().unwrap();
    // seed a stale cache directly in the plugin's state dir
    seed_cache(state.path(), "2026-07-20T09:00:00-04:00", "48183", "55");

    // point at a dead port so the fetch fails
    let w = build_widget("http://127.0.0.1:1", state.path().to_path_buf(), "48183");
    let s = w.render(&ctx_now("2026-07-20T15:00:00-04:00")); // 6h later -> stale, refetch attempted
    assert!(s[0].text.contains("55"), "fell back to stale cache: {s:?}");
}

#[test]
fn cross_zip_fetch_fails_renders_empty() {
    let state = tempfile::tempdir().unwrap();
    // seed a stale cache for zip 48183 (temp 55)...
    seed_cache(state.path(), "2026-07-20T09:00:00-04:00", "48183", "55");

    // ...but render a DIFFERENT zip whose fetch fails (dead port). The guest must
    // NOT fall back to the 48183 cache under the 90210 label -> empty segments.
    let w = build_widget("http://127.0.0.1:1", state.path().to_path_buf(), "90210");
    let s = w.render(&ctx_now("2026-07-20T15:00:00-04:00"));
    assert!(
        s.is_empty(),
        "cross-zip stale fallback must not leak the 48183 cache: {s:?}"
    );
    assert!(
        !s.iter().any(|seg| seg.text.contains("55")),
        "must not show the other zip's temp: {s:?}"
    );
}
