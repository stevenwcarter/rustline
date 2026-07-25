#![cfg(feature = "wasm-e2e")]
//! Positive end-to-end proof that plugin registration is wired into the
//! `rustline` binary: renders a real `weather.wasm` through
//! `main.rs -> register_plugins -> WasmWidget -> guest`, which makes one
//! capability-allowed fetch to an in-process mock and renders the temp.
//! Run via `just test-wasm` (needs `just build-weather` first).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;

const WTTR_BODY: &str = r#"{"current_condition":[{"temp_F":"72","weatherCode":"113","weatherDesc":[{"value":"Sunny"}]}]}"#;

fn spawn_mock() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
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

#[test]
fn render_right_with_weather_plugin_fetches_and_renders_temp() {
    let wasm_src = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/weather/target/wasm32-unknown-unknown/release/weather.wasm"
    );
    if !std::path::Path::new(wasm_src).exists() {
        panic!("run `just build-weather` first");
    }

    let base = spawn_mock();
    let tmp = tempfile::tempdir().unwrap();
    let cfg_home = tmp.path().join("cfg");
    let data_home = tmp.path().join("data");

    let cfg_dir = cfg_home.join("rustline");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("config.toml"),
        format!(
            r#"[layout]
right = ["weather"]
[plugins.weather]
allowed_urls = ["http://127.0.0.1:*/*"]
[plugins.weather.options]
zip = "48183"
format = "{{temp_f}}"
api_base = "{base}"
"#
        ),
    )
    .unwrap();

    let plugin_dir = data_home.join("rustline").join("plugins");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::copy(wasm_src, plugin_dir.join("weather.wasm")).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_rustline"))
        .args(["render", "right"])
        .env("XDG_CONFIG_HOME", &cfg_home)
        .env("XDG_DATA_HOME", &data_home)
        // Isolate from any real `rustline daemon` the developer/CI box has
        // running: `render` tries the daemon socket at
        // `$XDG_RUNTIME_DIR/rustline/daemon.sock` first, and without this
        // override it would find the REAL daemon and render the developer's
        // production config instead of this test's fixture (see smoke.rs's
        // `isolate` helper for the same rationale).
        .env("XDG_RUNTIME_DIR", tmp.path().join("runtime"))
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("72"),
        "temp rendered via register_plugins -> WasmWidget -> guest -> cached fetch: {stdout}"
    );
}

/// Positive proof that the production `FileDenialObserver` wiring in
/// `register_plugins` (not just the `DenialObserver` seam itself) is live: a
/// real `weather.wasm`, configured with no `allowed_urls`, gets its one fetch
/// denied gate-first, and that denial lands in `<data_home>/rustline/denials.jsonl`.
#[test]
fn denied_plugin_persists_a_denial_record_via_register_plugins() {
    let wasm_src = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/weather/target/wasm32-unknown-unknown/release/weather.wasm"
    );
    if !std::path::Path::new(wasm_src).exists() {
        panic!("run `just build-weather` first");
    }

    let tmp = tempfile::tempdir().unwrap();
    let cfg_home = tmp.path().join("cfg");
    let data_home = tmp.path().join("data");

    let cfg_dir = cfg_home.join("rustline");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    // No `allowed_urls` for weather -> its one fetch is denied before any
    // network call; the widget still degrades to empty (invariant N2).
    std::fs::write(
        cfg_dir.join("config.toml"),
        r#"[layout]
right = ["weather"]
[plugins.weather]
"#,
    )
    .unwrap();

    let plugin_dir = data_home.join("rustline").join("plugins");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::copy(wasm_src, plugin_dir.join("weather.wasm")).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_rustline"))
        .args(["render", "right"])
        .env("XDG_CONFIG_HOME", &cfg_home)
        .env("XDG_DATA_HOME", &data_home)
        // See the isolation comment in the test above: without this, `render`
        // would find a REAL daemon (if one is running) and never exercise
        // this test's fixture at all.
        .env("XDG_RUNTIME_DIR", tmp.path().join("runtime"))
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "a denied plugin degrades to empty, never fails the process: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let record_path = data_home.join("rustline").join("denials.jsonl");
    let contents = std::fs::read_to_string(&record_path)
        .unwrap_or_else(|e| panic!("expected {} to exist: {e}", record_path.display()));
    assert!(
        contents.contains("\"plugin\":\"weather\"") && contents.contains("\"kind\":\"url\""),
        "denial persisted by the production FileDenialObserver: {contents}"
    );
}
