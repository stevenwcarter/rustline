#![cfg(feature = "wasm-e2e")]
//! End-to-end: load the real weather.wasm, point it at a local mock wttr.in,
//! and assert the cache makes exactly one HTTP call, with stale fallback on
//! failure and no cross-zip leakage. Run via `just test-wasm` (requires the
//! wasm target + `just build-weather`).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rustline_core::{Config, Context, PluginConfig, Registry, Widget, WidgetSource};
use rustline_wasm::capability::CapabilityCtx;
use rustline_wasm::{WasmWidget, build_plugin, register_plugins, sha256_hex};

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
        battery: None,
        cpu: None,
        memory: None,
        git: None,
        disk: None,
        disks: Default::default(),
        throughput: None,
        throughputs: Default::default(),
        os: String::new(),
        arch: String::new(),
        uptime: None,
        media: None,
        cpu_history: Vec::new(),
        mem_history: Vec::new(),
        toggled: Default::default(),
        colors: Default::default(),
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
    WasmWidget::new(plugin, options, "weather")
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
fn register_plugins_records_a_plugin_sourced_descriptor() {
    let plugin_dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../plugins/weather/target/wasm32-unknown-unknown/release/weather.wasm"
        ),
        plugin_dir.path().join("weather.wasm"),
    )
    .expect("run `just build-weather` first");

    let mut reg = Registry::new();
    register_plugins(
        &mut reg,
        &Config::default(),
        plugin_dir.path(),
        &["weather".to_string()],
    );

    let desc = reg
        .descriptors()
        .iter()
        .find(|d| d.name == "weather")
        .expect("weather plugin registered a descriptor");
    assert_eq!(desc.source, WidgetSource::Plugin);
    assert!(desc.configurable);
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

/// Stage the real weather plugin into a fresh dir, returning (dir, bytes) so a
/// test can compute — or deliberately corrupt — its digest.
fn staged_weather() -> (tempfile::TempDir, Vec<u8>) {
    let bytes = weather_wasm();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("weather.wasm"), &bytes).expect("stage plugin");
    (dir, bytes)
}

/// A Config pinning `weather` to `checksum`.
fn cfg_with_checksum(checksum: Option<String>) -> Config {
    let mut cfg = Config::default();
    cfg.plugins.insert(
        "weather".to_string(),
        PluginConfig {
            checksum,
            ..Default::default()
        },
    );
    cfg
}

fn register_weather(cfg: &Config, dir: &std::path::Path) -> Registry {
    let mut reg = Registry::new();
    register_plugins(&mut reg, cfg, dir, &["weather".to_string()]);
    reg
}

/// Producer 1: a hand-installed plugin (or one built by `just build-plugin`)
/// has no recorded digest and must keep loading.
#[test]
fn plugin_without_a_recorded_checksum_still_registers() {
    let (dir, _bytes) = staged_weather();
    let reg = register_weather(&cfg_with_checksum(None), dir.path());
    assert!(
        reg.contains("weather"),
        "an unpinned plugin must still load"
    );
}

/// Producer 2: a plugin installed via `plugin install` records the digest of
/// exactly these bytes, so it must verify.
#[test]
fn plugin_with_a_matching_checksum_registers() {
    let (dir, bytes) = staged_weather();
    let reg = register_weather(&cfg_with_checksum(Some(sha256_hex(&bytes))), dir.path());
    assert!(
        reg.contains("weather"),
        "a correctly pinned plugin must load"
    );
}

/// The threat this feature exists for: the file on disk was swapped after the
/// digest was recorded. Note the module itself is perfectly valid here — only
/// the digest disagrees — so this can only pass if the gate is real.
#[test]
fn plugin_with_a_mismatched_checksum_does_not_register() {
    let (dir, _bytes) = staged_weather();
    let wrong = sha256_hex(b"different bytes entirely");
    let reg = register_weather(&cfg_with_checksum(Some(wrong)), dir.path());
    assert!(
        !reg.contains("weather"),
        "a tampered plugin must be refused even though the module is valid"
    );
}

/// Fail closed: an unparseable digest is a request to verify that we cannot honour.
#[test]
fn plugin_with_a_malformed_checksum_does_not_register() {
    let (dir, _bytes) = staged_weather();
    let reg = register_weather(
        &cfg_with_checksum(Some("not-a-digest".to_string())),
        dir.path(),
    );
    assert!(!reg.contains("weather"), "a malformed pin must fail closed");
}

/// The load gate accepts a canonically-shaped digest: 64 lowercase hex chars,
/// computed via `sha256_hex` over the exact bytes staged on disk.
///
/// NOTE on scope: this test calls `sha256_hex` from the same import the
/// verification gate itself uses internally, so it does NOT span the real
/// `plugin install` -> load seam — it cannot catch a bug where the install
/// path hashes different bytes than it writes, or where `write_install_record`
/// mangles the digest in transit to disk (`crates/rustline`'s
/// `write_install_record` is a private fn of the bin crate, unreachable from
/// this integration test — see that crate's module docs). It is
/// functionally the load-side half of `plugin_with_a_matching_checksum_registers`
/// above, plus the length pin. The real write -> load seam test — which
/// drives the actual `write_install_record` and `rustline_core::Config::load`
/// — lives in `crates/rustline/src/plugin_install.rs` as
/// `write_install_record_then_config_load_verifies_the_real_bytes` (and its
/// negative counterpart, `..._rejects_swapped_bytes`).
#[test]
fn a_canonically_shaped_digest_is_accepted_at_load_time() {
    let (dir, bytes) = staged_weather();
    // The shape `plugin install` would record: sha256_hex over the exact
    // bytes staged at <plugin_dir>/<name>.wasm.
    let recorded = sha256_hex(&bytes);
    assert_eq!(recorded.len(), 64, "recorded digest shape must stay stable");

    let reg = register_weather(&cfg_with_checksum(Some(recorded)), dir.path());
    assert!(
        reg.contains("weather"),
        "a digest of this shape matching the staged bytes must satisfy load-time verification"
    );
}
