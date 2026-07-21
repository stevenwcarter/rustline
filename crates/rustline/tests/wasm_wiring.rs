#![cfg(feature = "wasm-e2e")]
//! Positive end-to-end proof that plugin registration is actually wired into
//! the `rustline` binary: renders a real `weather.wasm` through
//! `main.rs -> register_plugins -> WasmWidget -> guest` and asserts the
//! cached temperature appears in the rendered output.
//!
//! No network: a fresh cache is pre-seeded so the guest serves it without
//! ever calling `rl_http_get`. Run via `just test-wasm` (needs
//! `just build-weather` first).

use std::process::Command;

#[test]
fn render_right_with_weather_plugin_renders_cached_temp() {
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
    std::fs::write(
        cfg_dir.join("config.toml"),
        r#"[layout]
right = ["weather"]
[plugins.weather]
allowed_urls = []
[plugins.weather.options]
zip = "48183"
format = "{temp_f}"
"#,
    )
    .unwrap();

    let plugin_dir = data_home.join("rustline").join("plugins");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::copy(wasm_src, plugin_dir.join("weather.wasm")).unwrap();

    // Seed a fresh cache (within the plugin's refresh window) so the guest
    // serves it directly, with no HTTP call.
    let state_dir = data_home.join("rustline").join("state").join("weather");
    std::fs::create_dir_all(&state_dir).unwrap();
    let now = chrono::Local::now().to_rfc3339();
    std::fs::write(
        state_dir.join("weather.json"),
        format!(
            r#"{{"fetched_at":"{now}","zip":"48183","temp_f":"72","code":"113","desc":"Sunny"}}"#
        ),
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_rustline"))
        .args(["render", "right"])
        .env("XDG_CONFIG_HOME", &cfg_home)
        .env("XDG_DATA_HOME", &data_home)
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
        "cached temp rendered through register_plugins -> WasmWidget -> guest: {stdout}"
    );
}
