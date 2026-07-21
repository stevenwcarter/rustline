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

#[test]
fn plugin_url_add_remove_roundtrips_preserving_comments() {
    let dir = std::env::temp_dir().join("rustline_smoke_pluginedit");
    let cfgdir = dir.join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    std::fs::write(&cfg, "# keepme\n[plugins.weather]\nallowed_urls = []\n").unwrap();

    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_rustline"))
            .args(args)
            .env("XDG_CONFIG_HOME", &dir)
            .output()
            .unwrap()
    };

    assert!(
        run(&["plugin", "url", "add", "weather", "https://wttr.in/*"])
            .status
            .success()
    );
    let after_add = std::fs::read_to_string(&cfg).unwrap();
    assert!(
        after_add.contains("# keepme"),
        "comment preserved: {after_add}"
    );
    assert!(
        after_add.contains("https://wttr.in/*"),
        "pattern added: {after_add}"
    );

    // idempotent add
    assert!(
        run(&["plugin", "url", "add", "weather", "https://wttr.in/*"])
            .status
            .success()
    );
    let dup = std::fs::read_to_string(&cfg).unwrap();
    assert_eq!(
        dup.matches("https://wttr.in/*").count(),
        1,
        "no duplicate: {dup}"
    );

    assert!(
        run(&["plugin", "url", "remove", "weather", "https://wttr.in/*"])
            .status
            .success()
    );
    let after_rm = std::fs::read_to_string(&cfg).unwrap();
    assert!(
        !after_rm.contains("https://wttr.in/*"),
        "pattern removed: {after_rm}"
    );
    assert!(
        after_rm.contains("# keepme"),
        "comment still there: {after_rm}"
    );
}

#[test]
fn plugin_add_on_malformed_config_errors_cleanly() {
    // A pre-existing config where `allowed_urls` is a string instead of an
    // array must fail with a clean, user-facing error (exit 1), never a
    // panic (exit 101) from an `.expect()` deep in `mutate`.
    let dir = std::env::temp_dir().join("rustline_smoke_pluginmalformed");
    let cfgdir = dir.join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    std::fs::write(&cfg, "[plugins.weather]\nallowed_urls = \"notanarray\"\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_rustline"))
        .args(["plugin", "url", "add", "weather", "https://wttr.in/*"])
        .env("XDG_CONFIG_HOME", &dir)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "clean error exit, not a panic; stderr={stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "must not panic: stderr={stderr}"
    );
}

#[test]
fn plugin_add_on_unparseable_config_preserves_file() {
    // A pre-existing config with a TOML *syntax* error must abort with exit 1
    // and leave the file byte-for-byte intact — never truncate the user's whole
    // config (layout/theme/other plugins) down to `[plugins.<x>]`.
    let dir = std::env::temp_dir().join("rustline_smoke_pluginunparseable");
    let cfgdir = dir.join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    let invalid = "this = = [[[\n";
    std::fs::write(&cfg, invalid).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_rustline"))
        .args(["plugin", "url", "add", "weather", "https://wttr.in/*"])
        .env("XDG_CONFIG_HOME", &dir)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "clean error exit, not a panic; stderr={stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "must not panic: stderr={stderr}"
    );
    let after = std::fs::read_to_string(&cfg).unwrap();
    assert_eq!(after, invalid, "config left byte-for-byte unchanged");
}
