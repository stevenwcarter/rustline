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

/// A mock that serves exactly one successful response then closes its
/// listener, so a later fetch to the same URL fails (connection refused).
fn spawn_mock_once() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Some(Ok(mut s)) = listener.incoming().next() {
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                WTTR_BODY.len(),
                WTTR_BODY
            );
            let _ = s.write_all(resp.as_bytes());
        }
        // listener dropped here -> port closed
    });
    format!("http://{addr}")
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
fn stale_cache_served_when_refresh_fails() {
    let state = tempfile::tempdir().unwrap();
    let base = spawn_mock_once();
    let w = build_widget(&base, state.path().to_path_buf(), "48183");
    // T0: fetch + cache (temp 72)
    let s1 = w.render(&ctx_now("2026-07-20T12:00:00-04:00"));
    assert!(s1[0].text.contains("72"), "first render fetched: {s1:?}");
    // 6h later: cache expired, endpoint now dead -> host serves stale
    let s2 = w.render(&ctx_now("2026-07-20T18:00:00-04:00"));
    assert!(
        s2[0].text.contains("72"),
        "stale body served on refresh failure: {s2:?}"
    );
}

#[test]
fn cross_zip_isolation_no_leak() {
    let state = tempfile::tempdir().unwrap();
    let base = spawn_mock(Arc::new(AtomicUsize::new(0)));
    // widget A (48183) fetches + caches into the shared state root
    let a = build_widget(&base, state.path().to_path_buf(), "48183");
    let sa = a.render(&ctx_now("2026-07-20T12:00:00-04:00"));
    assert!(sa[0].text.contains("72"));
    // widget B (90210) points at a dead endpoint but shares the state root.
    // Its URL (different zip AND host) has no cache entry -> empty. It must
    // never surface A's cached entry.
    let b = build_widget("http://127.0.0.1:1", state.path().to_path_buf(), "90210");
    let sb = b.render(&ctx_now("2026-07-20T12:05:00-04:00"));
    assert!(
        sb.is_empty(),
        "no entry for 90210 + failed fetch -> empty: {sb:?}"
    );
}
