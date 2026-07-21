use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

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
fn render_right_with_ip_widgets_renders_gracefully() {
    // lan_ip/tailscale_ip in a layout must render alongside built-ins and exit 0
    // on ANY host, regardless of its real LAN/Tailscale addresses. We force
    // lan_ip to a nonexistent interface so its down_format ("LANOFF") renders
    // deterministically — this positively proves the bin wires the interface
    // read -> Context -> the widget end-to-end, WITHOUT depending on whether the
    // host has (or lacks) a LAN or Tailscale IP. (A `contains("TSOFF")`-style
    // assertion would be host-dependent: any dev box actually running Tailscale
    // renders its real 100.x address instead of the down text.)
    let tmp = tempfile::tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    std::fs::write(
        cfgdir.join("config.toml"),
        "[layout]\nright = [\"lan_ip\", \"tailscale_ip\", \"datetime\"]\n\
         [widgets.lan_ip]\ninterface = \"rustline-no-such-nic0\"\ndown_format = \"LANOFF\"\n",
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_rustline"))
        .args(["render", "right"])
        .env("XDG_CONFIG_HOME", tmp.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("#["), "built-ins still render: {s}");
    // forced-nonexistent lan interface -> down_format renders deterministically,
    // proving the interface-read -> Context -> lan_ip wiring, host-independent.
    assert!(s.contains("LANOFF"), "lan_ip down_format shown: {s}");
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

/// A `rustline` invocation with an isolated HOME/XDG environment so logging
/// and config read/write a throwaway tree, never the developer's real dirs.
fn isolated_cmd(home: &Path, xdg_data: &Path, xdg_config: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_rustline"));
    c.env("HOME", home)
        .env("XDG_DATA_HOME", xdg_data)
        .env("XDG_CONFIG_HOME", xdg_config)
        .env_remove("RUST_LOG");
    c
}

#[test]
fn warning_lands_in_log_file_and_not_stderr_at_default() {
    let dir = tempdir().unwrap();
    let (home, data, config) = (
        dir.path().join("home"),
        dir.path().join("data"),
        dir.path().join("config"),
    );
    fs::create_dir_all(config.join("rustline")).unwrap();
    // An unknown widget name triggers `warn!("unknown widget, skipping")`.
    fs::write(
        config.join("rustline/config.toml"),
        "[layout]\nleft = [\"definitely_not_a_widget\"]\n",
    )
    .unwrap();

    let out = isolated_cmd(&home, &data, &config)
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

    assert!(out.status.success(), "render exited 0");

    // Default stderr level is ERROR, so a WARN must NOT surface on stderr.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("unknown widget"),
        "warning must not hit stderr at default level; got: {stderr}"
    );

    // The file sink (INFO) captured the WARN.
    let log = fs::read_to_string(data.join("rustline/rustline.log")).expect("log file created");
    assert!(
        log.contains("unknown widget"),
        "warning captured in log file; got: {log}"
    );
}

#[test]
fn stderr_level_override_promotes_warning_to_stderr() {
    let dir = tempdir().unwrap();
    let (home, data, config) = (
        dir.path().join("home"),
        dir.path().join("data"),
        dir.path().join("config"),
    );
    fs::create_dir_all(config.join("rustline")).unwrap();
    fs::write(
        config.join("rustline/config.toml"),
        "[layout]\nleft = [\"definitely_not_a_widget\"]\n\n[log]\nstderr_level = \"warn\"\n",
    )
    .unwrap();

    let out = isolated_cmd(&home, &data, &config)
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

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown widget"),
        "stderr_level=warn surfaces the warning on stderr; got: {stderr}"
    );
}
