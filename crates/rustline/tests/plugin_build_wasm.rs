#![cfg(feature = "wasm-e2e")]
//! `rustline plugin build` cargo-compiles the target crate for
//! `wasm32-unknown-unknown`, so exercising it for real requires that target
//! to be installed. Keeping these tests behind the opt-in `wasm-e2e` feature
//! (the same gate `wasm_wiring.rs`/`rustline-wasm`'s `e2e.rs` use) is what
//! preserves `just test`'s documented hermeticity — a plain `cargo test
//! --workspace` must never need the wasm toolchain. Run via `just test-wasm`
//! (which depends on `build-weather`, so `rustup target add
//! wasm32-unknown-unknown` has already happened by the time these run).
//!
//! These four tests (plus their two helpers) were moved here verbatim from
//! `smoke.rs`, where they had accidentally made the default `cargo test
//! --workspace` run depend on the wasm32 target being installed.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use tempfile::tempdir;

/// Point `HOME`, `XDG_DATA_HOME`, and `XDG_RUNTIME_DIR` at throwaway dirs
/// under `tmp` (and strip any inherited `RUST_LOG`), so a spawn here can
/// never create or append to the developer's real
/// `~/.local/share/rustline/rustline.log`, nor ever probe a REAL daemon
/// socket the developer might have running at their actual
/// `$XDG_RUNTIME_DIR/rustline/daemon.sock`. This is a verbatim copy of
/// `smoke.rs`'s `isolate` helper: Rust integration-test files are separate
/// crates and cannot share it, and this repo has no `tests/common/mod.rs` to
/// hold a shared copy.
fn isolate(cmd: &mut Command, tmp: &Path) {
    cmd.env("HOME", tmp.join("home"))
        .env("XDG_DATA_HOME", tmp.join("data"))
        .env("XDG_RUNTIME_DIR", tmp.join("runtime"))
        .env_remove("RUST_LOG");
}

/// Scaffold a minimal, fast-to-build WASM guest crate at `dir/<name>` — no
/// dependencies, just enough for `cargo build --target wasm32-unknown-unknown`
/// to produce a `.wasm` artifact — so `plugin build` smoke tests don't pay for
/// compiling a real plugin (e.g. `weather`) on every run.
fn scaffold_minimal_plugin_crate(dir: &Path, name: &str) {
    let crate_dir = dir.join(name);
    fs::create_dir_all(crate_dir.join("src")).unwrap();
    fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            "[workspace]\n\n[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n"
        ),
    )
    .unwrap();
    fs::write(crate_dir.join("src/lib.rs"), "").unwrap();
}

/// Run `cmd` (already `spawn`-able, e.g. with `Stdio::piped()`/`Stdio::null()`
/// stdio configured) and wait up to `timeout`, panicking loudly instead of
/// blocking forever if it doesn't exit in time — the load-bearing proof that
/// a supposedly non-interactive path never blocks on a stdin read. Draining
/// stdout/stderr happens on a background thread via `wait_with_output`
/// (which reads both concurrently), so a chatty child can't deadlock the wait
/// the way polling `try_wait` without draining could.
fn wait_with_timeout(mut cmd: Command, timeout: std::time::Duration) -> std::process::Output {
    let child = cmd.spawn().expect("failed to spawn");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(timeout) {
        Ok(result) => result.expect("failed to wait on child"),
        Err(_) => panic!(
            "process did not exit within {timeout:?} -- looks hung (a non-interactive stdin \
             read likely blocked waiting for input)"
        ),
    }
}

/// `plugin build`'s happy path end to end: build a fresh crate, install its
/// `.wasm`, and — since no checksum is recorded at all — print nothing extra
/// about checksums (the common case must stay quiet).
#[test]
fn plugin_build_installs_and_is_quiet_with_no_checksum_recorded() {
    let tmp = tempdir().unwrap();
    scaffold_minimal_plugin_crate(tmp.path(), "minibuild");
    let plugin_dir = tmp.path().join("plugins");
    let cfg = tmp.path().join("config.toml");
    fs::write(&cfg, "").unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args([
        "--config",
        cfg.to_str().unwrap(),
        "plugin",
        "build",
        tmp.path().join("minibuild").to_str().unwrap(),
        "--plugin-dir",
        plugin_dir.to_str().unwrap(),
    ]);
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(plugin_dir.join("minibuild.wasm").is_file());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.to_lowercase().contains("checksum"), "{stdout}");
    assert_eq!(
        fs::read_to_string(&cfg).unwrap(),
        "",
        "no checksum was ever recorded, so nothing should be written"
    );
}

/// `plugin build` also stays quiet when a recorded checksum still matches the
/// rebuilt bytes (a no-op rebuild).
#[test]
fn plugin_build_is_quiet_when_checksum_already_matches() {
    let tmp = tempdir().unwrap();
    scaffold_minimal_plugin_crate(tmp.path(), "minibuild");
    let plugin_dir = tmp.path().join("plugins");
    fs::create_dir_all(&plugin_dir).unwrap();
    let cfg = tmp.path().join("config.toml");

    // First build (no checksum recorded yet) establishes the real installed
    // artifact bytes, so we can pre-record their exact checksum -- avoids
    // hoping a fixed byte string happens to match whatever cargo emits.
    fs::write(&cfg, "").unwrap();
    let mut first_cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    first_cmd.args([
        "--config",
        cfg.to_str().unwrap(),
        "plugin",
        "build",
        tmp.path().join("minibuild").to_str().unwrap(),
        "--plugin-dir",
        plugin_dir.to_str().unwrap(),
    ]);
    isolate(&mut first_cmd, tmp.path());
    let first = first_cmd.output().unwrap();
    assert!(
        first.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&first.stderr)
    );
    let bytes = fs::read(plugin_dir.join("minibuild.wasm")).unwrap();
    fs::write(
        &cfg,
        format!(
            "[plugins.minibuild]\nchecksum = \"{}\"\n",
            rustline_wasm::sha256_hex(&bytes)
        ),
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args([
        "--config",
        cfg.to_str().unwrap(),
        "plugin",
        "build",
        tmp.path().join("minibuild").to_str().unwrap(),
        "--plugin-dir",
        plugin_dir.to_str().unwrap(),
    ]);
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.to_lowercase().contains("stale") && !stdout.to_lowercase().contains("note:"),
        "already-matching checksum must stay quiet: {stdout}"
    );
}

/// `plugin build --yes` refreshes a stale recorded checksum non-interactively
/// -- the explicit, scripted opt-in `--yes` covers.
#[test]
fn plugin_build_yes_flag_refreshes_a_stale_checksum() {
    let tmp = tempdir().unwrap();
    scaffold_minimal_plugin_crate(tmp.path(), "minibuild");
    let plugin_dir = tmp.path().join("plugins");
    let cfg = tmp.path().join("config.toml");
    let stale = rustline_wasm::sha256_hex(b"stale bytes from a previous install");
    fs::write(
        &cfg,
        format!("[plugins.minibuild]\nchecksum = \"{stale}\"\n"),
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args([
        "--config",
        cfg.to_str().unwrap(),
        "plugin",
        "build",
        tmp.path().join("minibuild").to_str().unwrap(),
        "--plugin-dir",
        plugin_dir.to_str().unwrap(),
        "--yes",
    ]);
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let installed = fs::read(plugin_dir.join("minibuild.wasm")).unwrap();
    let expected = rustline_wasm::sha256_hex(&installed);
    let text = fs::read_to_string(&cfg).unwrap();
    assert!(
        text.contains(&expected),
        "checksum refreshed to match the new build: {text}"
    );
    assert!(!text.contains(&stale), "{text}");
}

/// The load-bearing non-interactive proof: a stale checksum with NO `--yes`
/// and stdin that is not a terminal (here, `Stdio::null()`, matching a CI
/// runner or `< /dev/null`) must (a) complete promptly rather than hang
/// waiting on a prompt no one can answer, (b) leave the recorded checksum
/// untouched (never silently re-pinning it), and (c) print a clear notice
/// naming the plugin and how to fix it.
#[test]
fn plugin_build_noninteractive_stale_checksum_does_not_hang_and_does_not_rewrite() {
    let tmp = tempdir().unwrap();
    scaffold_minimal_plugin_crate(tmp.path(), "minibuild");
    let plugin_dir = tmp.path().join("plugins");
    let cfg = tmp.path().join("config.toml");
    let stale = rustline_wasm::sha256_hex(b"stale bytes from a previous install");
    fs::write(
        &cfg,
        format!("[plugins.minibuild]\nchecksum = \"{stale}\"\n"),
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args([
        "--config",
        cfg.to_str().unwrap(),
        "plugin",
        "build",
        tmp.path().join("minibuild").to_str().unwrap(),
        "--plugin-dir",
        plugin_dir.to_str().unwrap(),
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    isolate(&mut cmd, tmp.path());

    let out = wait_with_timeout(cmd, std::time::Duration::from_secs(30));

    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = fs::read_to_string(&cfg).unwrap();
    assert!(
        text.contains(&stale),
        "non-interactive run must leave the stale checksum alone: {text}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("minibuild") && stdout.to_lowercase().contains("checksum"),
        "prints a clear notice naming the plugin: {stdout}"
    );
}
