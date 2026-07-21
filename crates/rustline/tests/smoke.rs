use std::process::Command;

#[test]
fn render_left_produces_styled_output() {
    let out = Command::new(env!("CARGO_BIN_EXE_rustline"))
        .args([
            "render",
            "left",
            "--session",
            "0",
            "--window",
            "0",
            "--pane",
            "0",
        ])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("0:0.0"), "pane id present: {s}");
    assert!(s.contains("#["), "styled: {s}");
}

#[test]
fn render_left_preview_emits_ansi_not_tmux_markup() {
    let out = Command::new(env!("CARGO_BIN_EXE_rustline"))
        .args([
            "render",
            "left",
            "--preview",
            "--session",
            "0",
            "--window",
            "0",
            "--pane",
            "0",
        ])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("0:0.0"), "pane id text present: {s}");
    assert!(s.contains('\u{1b}'), "contains ANSI escape: {s:?}");
    assert!(
        !s.contains("#["),
        "raw tmux markup fully transcoded in preview mode: {s:?}"
    );
}

#[test]
fn init_prints_block() {
    let out = Command::new(env!("CARGO_BIN_EXE_rustline"))
        .arg("init")
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("status-interval 1"));
}

#[test]
fn render_right_with_missing_plugin_degrades_gracefully() {
    // A layout naming a plugin with no .wasm present must not crash: the bar
    // still renders the built-in widgets and exits 0.
    let dir = std::env::temp_dir().join("rustline_smoke_pluginless");
    let cfgdir = dir.join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    std::fs::write(&cfg, "[layout]\nright = [\"datetime\", \"weather\"]\n").unwrap();
    let empty_plugins = dir.join("plugins_empty");
    std::fs::create_dir_all(&empty_plugins).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_rustline"))
        .args(["render", "right", "--plugin-dir"])
        .arg(&empty_plugins)
        .env("XDG_CONFIG_HOME", &dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    // datetime still renders (contains tmux style markup)
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("#["), "built-ins still render: {s}");
}
