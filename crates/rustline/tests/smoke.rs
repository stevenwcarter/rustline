use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

/// Point `HOME` and `XDG_DATA_HOME` at throwaway dirs under `tmp` (and strip
/// any inherited `RUST_LOG`), so a smoke-test spawn can never create or
/// append to the developer's real `~/.local/share/rustline/rustline.log`.
/// Callers that also need an isolated config dir set `XDG_CONFIG_HOME`
/// themselves — this only adds the two vars every binary spawn needs.
fn isolate(cmd: &mut Command, tmp: &Path) {
    cmd.env("HOME", tmp.join("home"))
        .env("XDG_DATA_HOME", tmp.join("data"))
        .env_remove("RUST_LOG");
}

#[test]
fn render_left_produces_styled_output() {
    let tmp = tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args([
        "render",
        "left",
        "--session",
        "0",
        "--window",
        "0",
        "--pane",
        "0",
    ]);
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("0:0.0"), "pane id present: {s}");
    assert!(s.contains("#["), "styled: {s}");
}

#[test]
fn render_left_preview_emits_ansi_not_tmux_markup() {
    let tmp = tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args([
        "render",
        "left",
        "--preview",
        "--session",
        "0",
        "--window",
        "0",
        "--pane",
        "0",
    ]);
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("0:0.0"), "pane id text present: {s}");
    assert!(s.contains('\u{1b}'), "contains ANSI escape: {s:?}");
    assert!(
        !s.contains("#["),
        "raw tmux markup fully transcoded in preview mode: {s:?}"
    );
}

#[test]
fn render_window_pill_matches_expected_markup() {
    // Characterization: `build_window_context` was leaned out to skip
    // reads the window pill never uses (loadavg/toggles/hostname/etc.) --
    // the rendered pill markup must stay byte-identical regardless.
    let tmp = tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args([
        "render",
        "window",
        "--current",
        "--index",
        "1",
        "--name",
        "shell",
        "--flags",
        "*",
    ]);
    isolate(&mut cmd, tmp.path());
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg")); // no config file -> default theme
    let out = cmd.output().unwrap();
    assert_eq!(
        out.stdout,
        b"#[fg=colour31,bg=colour234]\xee\x82\xb6\
#[fg=colour255,bg=colour31,bold] 1* shell \
#[fg=colour31,bg=colour234]\xee\x82\xb4#[default]"
    );

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["render", "window", "--index", "2", "--name", "editor"]);
    isolate(&mut cmd, tmp.path());
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg"));
    let out = cmd.output().unwrap();
    assert_eq!(
        out.stdout,
        b"#[fg=colour236,bg=colour234]\xee\x82\xb6\
#[fg=colour250,bg=colour236] 2 editor \
#[fg=colour236,bg=colour234]\xee\x82\xb4#[default]"
    );
}

#[test]
fn init_print_emits_block_and_writes_nothing() {
    let tmp = tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.arg("init").arg("--print");
    isolate(&mut cmd, tmp.path());
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg"));
    let out = cmd.output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    // The binary path is the test binary's own resolved `current_exe()`, so
    // assert the shape (shell-quoted call) rather than an exact path.
    assert!(s.contains("#('"), "shell-quotes the binary path: {s}");
    assert!(s.contains("' render left"), "prints block: {s}");
    assert!(!s.contains("set -g status 2"), "one-line by default");
    // wrote no config file
    assert!(
        !tmp.path()
            .join("cfg")
            .join("rustline")
            .join("config.toml")
            .exists()
    );
}

#[test]
fn init_print_binary_flag_overrides_current_exe() {
    // `--binary` wins over the resolved `current_exe()`, and the tmux var
    // quoting (`#{q:...}`) stays untouched alongside it.
    let tmp = tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.arg("init")
        .arg("--print")
        .arg("--binary")
        .arg("/opt/rustline/bin/rustline");
    isolate(&mut cmd, tmp.path());
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg"));
    let out = cmd.output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("#('/opt/rustline/bin/rustline' render left"),
        "uses the overridden binary path: {s}"
    );
    assert!(
        s.contains("--pane-path=#{q:pane_current_path}"),
        "tmux var quoting untouched: {s}"
    );
}

#[test]
fn init_print_honors_configured_theme() {
    // `--print` must stay byte-identical to today's `rustline init`, which
    // colored `status-style` from the user's FULLY RESOLVED theme
    // (`resolve_theme(&cfg)`, applying `[theme].base` AND inline `[theme]`
    // overrides) — not a hardcoded "default". A zero-config invocation can't
    // distinguish the two, so this pins an inline override deterministically.
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("cfg").join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    fs::write(
        cfgdir.join("config.toml"),
        "[theme]\nbar_bg = { Indexed = 42 }\nfg = { Indexed = 43 }\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.arg("init").arg("--print");
    isolate(&mut cmd, tmp.path());
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg"));
    let out = cmd.output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("status-style bg=colour42,fg=colour43"),
        "print honors the configured theme override: {s}"
    );
}

#[test]
fn init_defaults_does_not_clobber_unreadable_tmux_conf() {
    // A present-but-unreadable ~/.tmux.conf (e.g. non-UTF8 contents) must abort
    // rather than collapsing the read error to empty, which would silently
    // skip the backup and overwrite the file `apply` couldn't safely read.
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let tmux_path = home.join(".tmux.conf");
    let original = [0xff_u8, 0xfe, 0x00];
    fs::write(&tmux_path, original).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.arg("init").arg("--defaults");
    cmd.env("HOME", &home)
        .env("XDG_DATA_HOME", tmp.path().join("data"))
        .env("XDG_CONFIG_HOME", tmp.path().join("cfg"))
        .env_remove("RUST_LOG");
    let out = cmd.output().unwrap();

    assert!(
        !out.status.success(),
        "must not succeed when tmux.conf can't be read; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    // config.toml may have been written already (it's written before the tmux
    // step) — only the tmux.conf file's untouched-ness is under test here.
    assert_eq!(
        fs::read(&tmux_path).unwrap(),
        original,
        "unreadable tmux.conf must be left byte-for-byte untouched"
    );
}

#[test]
fn init_defaults_writes_config_and_tmux_marker_block() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let run = |tmp: &Path| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
        cmd.arg("init").arg("--defaults");
        cmd.env("HOME", &home)
            .env("XDG_DATA_HOME", tmp.join("data"))
            .env("XDG_CONFIG_HOME", tmp.join("cfg"))
            .env_remove("RUST_LOG");
        cmd.output().unwrap()
    };
    let out = run(tmp.path());
    assert!(out.status.success(), "init --defaults ok: {out:?}");
    let cfg_path = tmp.path().join("cfg").join("rustline").join("config.toml");
    let cfg_text = fs::read_to_string(&cfg_path).expect("config written");
    assert!(cfg_text.contains("[theme]"), "has theme: {cfg_text}");
    let tmux_path = home.join(".tmux.conf");
    let tmux_text = fs::read_to_string(&tmux_path).expect("tmux.conf written");
    assert!(
        tmux_text.contains("# >>> rustline >>>"),
        "marker block: {tmux_text}"
    );
    assert!(tmux_text.contains("#('"), "shell-quotes the binary path");
    assert!(tmux_text.contains("' render left"));

    // Idempotent: a user edit outside the markers survives; the region is unchanged.
    fs::write(&tmux_path, format!("# my own line\n{tmux_text}")).unwrap();
    let before = fs::read_to_string(&tmux_path).unwrap();
    let _ = run(tmp.path());
    let after = fs::read_to_string(&tmux_path).unwrap();
    assert!(after.contains("# my own line"), "user edit preserved");
    assert_eq!(
        after.matches("# >>> rustline >>>").count(),
        1,
        "no duplicate block"
    );
    assert_eq!(
        before, after,
        "second --defaults run is a no-op on tmux.conf"
    );
}

#[test]
fn init_dry_run_defaults_prints_both_artifacts_and_writes_nothing() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.arg("init").arg("--dry-run").arg("--defaults");
    cmd.env("HOME", &home)
        .env("XDG_DATA_HOME", tmp.path().join("data"))
        .env("XDG_CONFIG_HOME", tmp.path().join("cfg"))
        .env_remove("RUST_LOG");
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "dry-run exits 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("config.toml"), "config header present: {s}");
    assert!(s.contains("[theme]"), "config content printed: {s}");
    assert!(s.contains("tmux block"), "tmux header present: {s}");
    assert!(s.contains("#('"), "tmux block content printed: {s}");
    assert!(
        s.matches("new file").count() == 2,
        "both artifacts noted as new: {s}"
    );

    let cfg_path = tmp.path().join("cfg").join("rustline").join("config.toml");
    assert!(!cfg_path.exists(), "dry-run must not write config.toml");
    let tmux_path = home.join(".tmux.conf");
    assert!(!tmux_path.exists(), "dry-run must not write tmux.conf");
    assert!(
        !Path::new(&format!("{}.rustline.bak", cfg_path.display())).exists(),
        "dry-run must not write a config backup"
    );
}

#[test]
fn init_dry_run_with_existing_files_shows_diff_and_writes_nothing() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let cfgdir = tmp.path().join("cfg").join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg_path = cfgdir.join("config.toml");
    let cfg_original = "[widgets.cpu]\nformat = \"USER {percent}%\"\n";
    fs::write(&cfg_path, cfg_original).unwrap();
    let tmux_path = home.join(".tmux.conf");
    let tmux_original = "# my own line\n";
    fs::write(&tmux_path, tmux_original).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.arg("init").arg("--dry-run").arg("--defaults");
    cmd.env("HOME", &home)
        .env("XDG_DATA_HOME", tmp.path().join("data"))
        .env("XDG_CONFIG_HOME", tmp.path().join("cfg"))
        .env_remove("RUST_LOG");
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "dry-run exits 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.matches("existing file found").count() == 2,
        "both artifacts noted as existing: {s}"
    );
    assert!(
        s.contains("USER {percent}%"),
        "preserved user cpu format visible in resulting content: {s}"
    );
    assert!(
        s.contains("+# >>> rustline >>>"),
        "diff shows the tmux marker block being added: {s}"
    );
    assert!(
        s.contains("+[theme]"),
        "diff shows added config section: {s}"
    );

    assert_eq!(
        fs::read_to_string(&cfg_path).unwrap(),
        cfg_original,
        "existing config.toml must be byte-for-byte unchanged"
    );
    assert_eq!(
        fs::read_to_string(&tmux_path).unwrap(),
        tmux_original,
        "existing tmux.conf must be byte-for-byte unchanged"
    );
    assert!(
        !Path::new(&format!("{}.rustline.bak", cfg_path.display())).exists(),
        "dry-run must not write a config backup"
    );
    assert!(
        !Path::new(&format!("{}.rustline.bak", tmux_path.display())).exists(),
        "dry-run must not write a tmux backup"
    );
}

#[test]
fn init_non_tty_without_flags_errors() {
    let tmp = tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.arg("init"); // stdin is not a TTY under Command
    isolate(&mut cmd, tmp.path());
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg"));
    let out = cmd.output().unwrap();
    assert!(
        !out.status.success(),
        "errors without a TTY and no --defaults/--print"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("--defaults") || err.contains("--print"),
        "hints flags: {err}"
    );
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

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["render", "right", "--plugin-dir"])
        .arg(&empty_plugins)
        .env("XDG_CONFIG_HOME", &dir);
    isolate(&mut cmd, &dir);
    let out = cmd.output().unwrap();
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
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
        cmd.args(args).env("XDG_CONFIG_HOME", &dir);
        isolate(&mut cmd, &dir);
        cmd.output().unwrap()
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

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["plugin", "url", "add", "weather", "https://wttr.in/*"])
        .env("XDG_CONFIG_HOME", &dir);
    isolate(&mut cmd, &dir);
    let out = cmd.output().unwrap();

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

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["render", "right"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
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
fn render_right_with_battery_renders_gracefully() {
    // `battery` in a layout must render alongside built-ins and exit 0 on ANY
    // host, whether or not it actually has a battery (desktops/CI have none →
    // the widget skips via its empty down_format; laptops render the level).
    // This proves the build_context -> read_battery -> Context -> widget wiring
    // does not crash; the deterministic icon/percent formatting is pinned by
    // the widget's own unit tests (host-independent there).
    let tmp = tempfile::tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    std::fs::write(
        cfgdir.join("config.toml"),
        "[layout]\nright = [\"battery\", \"datetime\"]\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["render", "right"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("#["), "built-ins still render: {s}");
}

#[test]
fn render_right_with_cpu_memory_renders_gracefully() {
    // `cpu`/`memory` in a layout must render alongside built-ins and exit 0 on
    // ANY host. Proves the build_context -> read_cpu/read_memory -> Context ->
    // widgets wiring does not crash; deterministic formatting is pinned by the
    // widgets' own unit tests. On Linux the /proc reads succeed and the widgets
    // render live values; elsewhere they skip via empty down_format — either way
    // `datetime` guarantees non-empty tmux markup.
    let tmp = tempfile::tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    std::fs::write(
        cfgdir.join("config.toml"),
        "[layout]\nright = [\"cpu\", \"memory\", \"datetime\"]\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["render", "right"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("#["), "built-ins still render: {s}");
    // On Linux the /proc reads succeed, so the cpu widget renders its icon —
    // this guards the read -> Context -> widget seam end to end (host-dependent,
    // so Linux-gated).
    #[cfg(target_os = "linux")]
    assert!(
        s.contains('\u{f061a}'),
        "cpu widget should render its icon on Linux: {s}"
    );
}

#[test]
fn render_right_with_git_outside_repo_renders_gracefully() {
    // `git` in a layout, with `--pane-path` pointed at a bare tempdir (never a
    // git repository), must degrade to its empty down_format rather than crash
    // or hang — proves the build_context -> read_git -> Context -> widget wiring
    // does not break the bar when `git status` fails (invariant #6).
    let tmp = tempfile::tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    std::fs::write(
        cfgdir.join("config.toml"),
        "[layout]\nright = [\"git\", \"datetime\"]\n",
    )
    .unwrap();
    let pane_dir = tmp.path().join("not_a_repo");
    std::fs::create_dir_all(&pane_dir).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["render", "right", "--pane-path", pane_dir.to_str().unwrap()])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("#["), "built-ins still render: {s}");
}

#[test]
fn render_right_with_git_inside_repo_renders_branch() {
    // Pointing `--pane-path` at this checkout's own repo root must render a
    // non-empty branch glyph — the positive-path counterpart to the
    // outside-a-repo test above, exercising the full read_git -> parse ->
    // widget chain against a real repository rather than a fixture string.
    let repo_root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let tmp = tempfile::tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    std::fs::write(
        cfgdir.join("config.toml"),
        "[layout]\nright = [\"git\"]\n\n[widgets.git]\nformat = \"{branch}\"\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["render", "right", "--pane-path", repo_root])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("#["), "git branch text should render: {s}");
}

#[test]
fn render_right_with_disk_on_bogus_mount_renders_gracefully() {
    // `disk` in a layout, configured against a mount that doesn't exist, must
    // degrade to its empty down_format rather than crash — proves the
    // build_context -> read_disk -> Context -> widget wiring does not break
    // the bar when `statvfs` fails (invariant #6). The other default-layout
    // built-ins (e.g. `datetime`) must still render.
    let tmp = tempfile::tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    std::fs::write(
        cfgdir.join("config.toml"),
        "[layout]\nright = [\"disk\", \"datetime\"]\n\n\
         [widgets.disk]\nmount = \"/nonexistent/bogus/mount/path\"\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["render", "right"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("#["), "built-ins still render: {s}");
}

#[test]
fn render_right_with_disk_on_real_mount_renders_usage() {
    // Pointing `[widgets.disk].mount` at the real root filesystem must render
    // non-empty usage text — the positive-path counterpart to the bogus-mount
    // test above, exercising the full read_disk -> derive -> widget chain
    // against a real mount rather than a fixture struct. Tolerant of the
    // actual disk size on the test box: just asserts the segment renders.
    let tmp = tempfile::tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    std::fs::write(
        cfgdir.join("config.toml"),
        "[layout]\nright = [\"disk\"]\n\n[widgets.disk]\nmount = \"/\"\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["render", "right"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("#["), "disk usage text should render: {s}");
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

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["plugin", "url", "add", "weather", "https://wttr.in/*"])
        .env("XDG_CONFIG_HOME", &dir);
    isolate(&mut cmd, &dir);
    let out = cmd.output().unwrap();

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

#[test]
fn click_toggles_state_file() {
    let tmp = tempfile::tempdir().unwrap();
    let toggles_path = tmp.path().join("data/rustline/toggles");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["click", "--range=cpu", "--button=left"]);
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let toggles = std::fs::read_to_string(&toggles_path).unwrap();
    assert!(toggles.contains("cpu"), "cpu toggled on: {toggles:?}");

    // second click toggles off
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["click", "--range=cpu", "--button=left"]);
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let toggles = std::fs::read_to_string(&toggles_path).unwrap_or_default();
    assert!(!toggles.contains("cpu"), "cpu toggled off: {toggles:?}");
}

#[test]
fn completions_prints_nonempty_script_for_each_shell() {
    let tmp = tempdir().unwrap();
    for shell in ["bash", "zsh", "fish"] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
        cmd.args(["completions", shell]);
        isolate(&mut cmd, tmp.path());
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "completions {shell} exits ok; stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        let s = String::from_utf8_lossy(&out.stdout);
        assert!(!s.is_empty(), "{shell} completion script is non-empty");
        assert!(
            s.contains("rustline"),
            "{shell} completion script mentions rustline: {s}"
        );
    }
}

#[test]
fn theme_pick_non_tty_errors_and_writes_nothing() {
    let tmp = tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.arg("theme").arg("pick");
    isolate(&mut cmd, tmp.path());
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg"));
    let out = cmd.output().unwrap(); // no TTY under Command
    assert!(!out.status.success(), "non-TTY `theme pick` must error");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("theme show") || err.contains("theme use"),
        "hints the non-interactive alternatives: {err}"
    );
    assert!(
        !tmp.path()
            .join("cfg")
            .join("rustline")
            .join("config.toml")
            .exists(),
        "must not write config on the non-TTY path"
    );
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

#[test]
fn invalid_config_warning_lands_in_log_file() {
    // `main` orders `Config::load_reporting` before `logging::init`, then
    // emits the deferred load-failure warning once the subscriber exists —
    // this pins that ordering seam. A regression that emits the warning
    // before `logging::init` would drop it (no subscriber yet), and this
    // test would fail to find it in the log file.
    let dir = tempdir().unwrap();
    let (home, data, config) = (
        dir.path().join("home"),
        dir.path().join("data"),
        dir.path().join("config"),
    );
    fs::create_dir_all(config.join("rustline")).unwrap();
    fs::write(
        config.join("rustline/config.toml"),
        "this is = = not valid toml [[[\n",
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

    assert!(
        out.status.success(),
        "a bad config must never break the bar; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let log = fs::read_to_string(data.join("rustline/rustline.log")).expect("log file created");
    assert!(
        log.contains("invalid config"),
        "deferred load-warning reaches the log file after logging::init; got: {log}"
    );
}

#[test]
fn unwritable_log_dir_degrades_to_stderr_only() {
    // If the log file's parent dir can't be created (here: a regular file
    // already occupies that name), the subscriber must degrade to
    // stderr-only rather than crash — the bar keeps rendering, and the
    // failure is reported via a deferred `error!` that passes the default
    // ERROR stderr filter.
    let dir = tempdir().unwrap();
    let (home, data, config) = (
        dir.path().join("home"),
        dir.path().join("data"),
        dir.path().join("config"),
    );
    fs::create_dir_all(&data).unwrap();
    // Occupies `$XDG_DATA_HOME/rustline`, so `open_log`'s
    // `create_dir_all($XDG_DATA_HOME/rustline)` fails: a non-directory
    // already exists at that path.
    fs::write(data.join("rustline"), "not a directory").unwrap();

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

    assert!(
        out.status.success(),
        "the bar renders even when the log file can't be opened; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("0:0.0"), "bar unaffected: {stdout}");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot open log file"),
        "file-open failure degrades to a stderr-only report; got: {stderr}"
    );
}
