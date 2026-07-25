use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use tempfile::tempdir;

/// Point `HOME`, `XDG_DATA_HOME`, and `XDG_RUNTIME_DIR` at throwaway dirs
/// under `tmp` (and strip any inherited `RUST_LOG`), so a smoke-test spawn
/// can never create or append to the developer's real
/// `~/.local/share/rustline/rustline.log`, nor (W48) ever probe a REAL daemon
/// socket the developer might have running at their actual
/// `$XDG_RUNTIME_DIR/rustline/daemon.sock` — every `render`/`daemon` spawn
/// below now touches `daemon_client::try_render`/`daemon::status`, which
/// resolve the socket path from `XDG_RUNTIME_DIR` when it's set. Callers that
/// also need an isolated config dir set `XDG_CONFIG_HOME` themselves — this
/// only adds the vars every binary spawn needs.
fn isolate(cmd: &mut Command, tmp: &Path) {
    cmd.env("HOME", tmp.join("home"))
        .env("XDG_DATA_HOME", tmp.join("data"))
        .env("XDG_RUNTIME_DIR", tmp.join("runtime"))
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
fn init_dry_run_print_wins_and_writes_nothing() {
    // `--print` wins over `--dry-run`: stdout is the legacy one-line tmux
    // block only (no dry-run `# --- ... ---` headers), exit 0, no files or
    // backups written.
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.arg("init").arg("--dry-run").arg("--print");
    cmd.env("HOME", &home)
        .env("XDG_DATA_HOME", tmp.path().join("data"))
        .env("XDG_CONFIG_HOME", tmp.path().join("cfg"))
        .env_remove("RUST_LOG");
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "--print wins, exits 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("#('"), "shell-quotes the binary path: {s}");
    assert!(s.contains("' render left"), "prints the raw block: {s}");
    assert!(
        !s.contains("# --- "),
        "no dry-run artifact headers when --print wins: {s}"
    );
    assert!(!s.contains("set -g status 2"), "one-line by default");

    let cfg_path = tmp.path().join("cfg").join("rustline").join("config.toml");
    assert!(!cfg_path.exists(), "--print must not write config.toml");
    let tmux_path = home.join(".tmux.conf");
    assert!(!tmux_path.exists(), "--print must not write tmux.conf");
    assert!(
        !Path::new(&format!("{}.rustline.bak", cfg_path.display())).exists(),
        "--print must not write a config backup"
    );
    assert!(
        !Path::new(&format!("{}.rustline.bak", tmux_path.display())).exists(),
        "--print must not write a tmux backup"
    );
}

#[test]
fn init_dry_run_non_tty_without_defaults_hits_terminal_guard() {
    // `--dry-run` must not bypass the terminal-required guard: without a
    // TTY and without `--defaults`, `run` never gets far enough to compute
    // answers, so it exits the same way plain `init` does (exit 2), and
    // writes nothing.
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.arg("init").arg("--dry-run"); // stdin is not a TTY under Command
    cmd.env("HOME", &home)
        .env("XDG_DATA_HOME", tmp.path().join("data"))
        .env("XDG_CONFIG_HOME", tmp.path().join("cfg"))
        .env_remove("RUST_LOG");
    let out = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(2),
        "errors without a TTY and no --defaults/--print; stderr={stderr}"
    );
    assert!(
        stderr.contains("--defaults") || stderr.contains("--print"),
        "hints flags: {stderr}"
    );

    let cfg_path = tmp.path().join("cfg").join("rustline").join("config.toml");
    assert!(!cfg_path.exists(), "must not write config.toml");
    let tmux_path = home.join(".tmux.conf");
    assert!(!tmux_path.exists(), "must not write tmux.conf");
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
fn init_uninstall_removes_block_and_backs_up() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let run = |args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
        cmd.arg("init").args(args);
        cmd.env("HOME", &home)
            .env("XDG_DATA_HOME", tmp.path().join("data"))
            .env("XDG_CONFIG_HOME", tmp.path().join("cfg"))
            .env_remove("RUST_LOG");
        cmd.output().unwrap()
    };
    // Seed a real tmux.conf via --defaults, then wrap it with user lines on
    // both sides.
    let out = run(&["--defaults"]);
    assert!(out.status.success(), "seed init --defaults ok: {out:?}");
    let tmux_path = home.join(".tmux.conf");
    let seeded = fs::read_to_string(&tmux_path).expect("seeded tmux.conf");
    let wrapped = format!("# my own line\n{seeded}# trailing user line\n");
    fs::write(&tmux_path, &wrapped).unwrap();
    let cfg_path = tmp.path().join("cfg").join("rustline").join("config.toml");
    let cfg_before = fs::read_to_string(&cfg_path).expect("seeded config.toml");

    let out = run(&["--uninstall"]);
    assert!(
        out.status.success(),
        "uninstall exits ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("tmux source-file"),
        "prints the reload hint: {stderr}"
    );

    let bak_path = Path::new(&format!("{}.rustline.bak", tmux_path.display())).to_path_buf();
    let bak = fs::read_to_string(&bak_path).expect("backup written");
    assert_eq!(bak, wrapped, "backup matches pre-uninstall content exactly");

    let after = fs::read_to_string(&tmux_path).unwrap();
    assert!(
        !after.contains("# >>> rustline >>>"),
        "block start removed: {after}"
    );
    assert!(
        !after.contains("# <<< rustline <<<"),
        "block end removed: {after}"
    );
    assert!(
        after.contains("# my own line"),
        "leading line kept: {after}"
    );
    assert!(
        after.contains("# trailing user line"),
        "trailing line kept: {after}"
    );

    let cfg_after = fs::read_to_string(&cfg_path).unwrap();
    assert_eq!(
        cfg_after, cfg_before,
        "config.toml must be byte-for-byte unchanged by --uninstall"
    );
}

#[test]
fn init_uninstall_with_no_block_is_a_no_op() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let tmux_path = home.join(".tmux.conf");
    let original = "set -g mouse on\nset -g status-interval 5\n";
    fs::write(&tmux_path, original).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.arg("init").arg("--uninstall");
    cmd.env("HOME", &home)
        .env("XDG_DATA_HOME", tmp.path().join("data"))
        .env("XDG_CONFIG_HOME", tmp.path().join("cfg"))
        .env_remove("RUST_LOG");
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "no-op still exits ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("nothing to uninstall"),
        "explains the no-op: {stderr}"
    );
    assert_eq!(
        fs::read_to_string(&tmux_path).unwrap(),
        original,
        "tmux.conf left byte-for-byte unchanged"
    );
    assert!(
        !Path::new(&format!("{}.rustline.bak", tmux_path.display())).exists(),
        "no backup written when nothing was removed"
    );
}

#[test]
fn init_uninstall_missing_tmux_conf_is_a_no_op() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap(); // no .tmux.conf created

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.arg("init").arg("--uninstall");
    cmd.env("HOME", &home)
        .env("XDG_DATA_HOME", tmp.path().join("data"))
        .env("XDG_CONFIG_HOME", tmp.path().join("cfg"))
        .env_remove("RUST_LOG");
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "no-op still exits ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("nothing to uninstall"),
        "explains the no-op: {stderr}"
    );
    let tmux_path = home.join(".tmux.conf");
    assert!(!tmux_path.exists(), "no tmux.conf created");
    assert!(
        !Path::new(&format!("{}.rustline.bak", tmux_path.display())).exists(),
        "no backup created"
    );
}

#[test]
fn init_uninstall_and_print_together_print_wins() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let run = |args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
        cmd.arg("init").args(args);
        cmd.env("HOME", &home)
            .env("XDG_DATA_HOME", tmp.path().join("data"))
            .env("XDG_CONFIG_HOME", tmp.path().join("cfg"))
            .env_remove("RUST_LOG");
        cmd.output().unwrap()
    };
    // Seed a tmux.conf that has the block.
    let out = run(&["--defaults"]);
    assert!(out.status.success(), "seed init --defaults ok: {out:?}");
    let tmux_path = home.join(".tmux.conf");
    let seeded = fs::read_to_string(&tmux_path).expect("seeded tmux.conf");

    let out = run(&["--uninstall", "--print"]);
    assert!(
        out.status.success(),
        "print wins, exits ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("#('"), "shell-quotes the binary path: {s}");
    assert!(s.contains("' render left"), "prints the raw block: {s}");

    let after = fs::read_to_string(&tmux_path).unwrap();
    assert_eq!(
        after, seeded,
        "--print short-circuits before --uninstall is considered; file untouched"
    );
    assert!(
        !Path::new(&format!("{}.rustline.bak", tmux_path.display())).exists(),
        "no backup written: --print never touches disk"
    );
}

#[test]
fn init_uninstall_dry_run_previews_only_and_writes_nothing() {
    // CRITICAL regression (Phase 4 data-safety review): `--dry-run` must be a
    // true no-write modifier for `--uninstall` too. Before the fix, `run`
    // checked `--uninstall` before `--dry-run`, so `--uninstall --dry-run`
    // silently performed the REAL removal + backup despite `--dry-run`'s
    // doc-commented promise of "without touching disk".
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let run = |args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
        cmd.arg("init").args(args);
        cmd.env("HOME", &home)
            .env("XDG_DATA_HOME", tmp.path().join("data"))
            .env("XDG_CONFIG_HOME", tmp.path().join("cfg"))
            .env_remove("RUST_LOG");
        cmd.output().unwrap()
    };
    // Seed a real tmux.conf (with the managed block) via --defaults, then
    // wrap it with user lines on both sides, same as
    // `init_uninstall_removes_block_and_backs_up`.
    let out = run(&["--defaults"]);
    assert!(out.status.success(), "seed init --defaults ok: {out:?}");
    let tmux_path = home.join(".tmux.conf");
    let seeded = fs::read_to_string(&tmux_path).expect("seeded tmux.conf");
    let wrapped = format!("# my own line\n{seeded}# trailing user line\n");
    fs::write(&tmux_path, &wrapped).unwrap();

    let out = run(&["--uninstall", "--dry-run"]);
    assert!(
        out.status.success(),
        "uninstall --dry-run exits 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("tmux block"),
        "prints a preview of the tmux block change: {s}"
    );
    assert!(
        s.contains("-# >>> rustline >>>"),
        "diff shows the managed block would be removed: {s}"
    );

    let after = fs::read_to_string(&tmux_path).unwrap();
    assert_eq!(
        after, wrapped,
        "--dry-run must leave ~/.tmux.conf byte-for-byte unchanged"
    );
    assert!(
        !Path::new(&format!("{}.rustline.bak", tmux_path.display())).exists(),
        "--dry-run must create no backup file"
    );
}

#[test]
fn init_uninstall_without_dry_run_still_performs_real_uninstall() {
    // Companion to the regression above: plain `--uninstall` (no --dry-run)
    // must be unaffected by the fix and still perform the real write. This
    // guards against a fix that accidentally makes --uninstall a no-op.
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let run = |args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
        cmd.arg("init").args(args);
        cmd.env("HOME", &home)
            .env("XDG_DATA_HOME", tmp.path().join("data"))
            .env("XDG_CONFIG_HOME", tmp.path().join("cfg"))
            .env_remove("RUST_LOG");
        cmd.output().unwrap()
    };
    let out = run(&["--defaults"]);
    assert!(out.status.success(), "seed init --defaults ok: {out:?}");
    let tmux_path = home.join(".tmux.conf");
    let seeded = fs::read_to_string(&tmux_path).expect("seeded tmux.conf");

    let out = run(&["--uninstall"]);
    assert!(
        out.status.success(),
        "uninstall exits ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let after = fs::read_to_string(&tmux_path).unwrap();
    assert!(
        !after.contains("# >>> rustline >>>"),
        "real uninstall still removes the block: {after}"
    );
    assert!(
        Path::new(&format!("{}.rustline.bak", tmux_path.display())).exists(),
        "real uninstall still writes a backup"
    );
    let bak = fs::read_to_string(format!("{}.rustline.bak", tmux_path.display())).unwrap();
    assert_eq!(bak, seeded, "backup matches pre-uninstall content");
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
fn render_right_with_uptime_renders_humanized_reading() {
    // `uptime` alone in a layout must render non-empty humanized text on this
    // Linux box — exercising the full read_uptime -> Context -> widget chain
    // against the real `/proc/uptime`, which (like the disk real-mount test
    // above) is real and host-independent: tolerant of the actual uptime
    // value, just asserts the segment renders.
    let tmp = tempfile::tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    std::fs::write(
        cfgdir.join("config.toml"),
        "[layout]\nright = [\"uptime\"]\n",
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
    assert!(s.contains("#["), "uptime text should render: {s}");
}

#[test]
fn render_right_with_media_renders_gracefully() {
    // `media` in a layout must render alongside built-ins and exit 0 on ANY
    // host, whether or not `playerctl` is installed or anything is playing —
    // proves the build_context -> read_media -> Context -> widget wiring does
    // not crash when the shell-out fails/is absent (invariant #6), mirroring
    // `render_right_with_git_outside_repo_renders_gracefully` above. The
    // media widget itself may render nothing (its default down_format is
    // empty), so `datetime` is the one asserted to still produce markup.
    let tmp = tempfile::tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    std::fs::write(
        cfgdir.join("config.toml"),
        "[layout]\nright = [\"media\", \"datetime\"]\n",
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
fn render_right_with_throughput_renders_gracefully() {
    // `throughput` in a layout must render alongside built-ins and exit 0 —
    // including on the FIRST invocation, which is the common case here: a
    // throughput rate needs a prior persisted sample to diff against (see
    // `crate::throughput::read_throughput`), so a fresh, isolated
    // `XDG_DATA_HOME` (this test's, via `isolate`) has no prior sample and
    // `Context.throughput` stays `None` on this very first render. Proves
    // the build_context -> read_throughput -> Context -> widget wiring
    // degrades to `down_format` rather than crashing or fabricating a rate
    // (invariant #6), mirroring
    // `render_right_with_git_outside_repo_renders_gracefully`/
    // `render_right_with_disk_on_bogus_mount_renders_gracefully` above. A
    // `down_format` marker makes the degrade path deterministically visible
    // rather than merely inferred from "still exits 0".
    let tmp = tempfile::tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    std::fs::write(
        cfgdir.join("config.toml"),
        "[layout]\nright = [\"throughput\", \"datetime\"]\n\n\
         [widgets.throughput]\ndown_format = \"THROUGHPUT-DOWN\"\n",
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
    assert!(
        s.contains("THROUGHPUT-DOWN"),
        "first-run (no prior sample) degrades to down_format: {s}"
    );
}

#[test]
fn render_right_with_color_override_pins_widget_bg() {
    // W29: a `[widgets.<name>].bg` override must reach the actual rendered
    // markup end to end (Config::color_overrides -> render_named_region ->
    // render_region_ranged), pinning the segment's background rather than
    // letting `assign_palette` cycle it in as usual.
    let tmp = tempfile::tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    std::fs::write(
        cfgdir.join("config.toml"),
        "[layout]\nright = [\"datetime\"]\n\n[widgets.datetime]\nbg = { Named = \"blue\" }\n",
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
    assert!(s.contains("bg=blue"), "override bg reaches markup: {s}");
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

    // W36: a default left-click only toggles a *clickable* widget, so give cpu
    // a non-empty alt_format (what makes it click-toggleable) via config.
    let cfgdir = tmp.path().join("cfg").join("rustline");
    std::fs::create_dir_all(&cfgdir).unwrap();
    std::fs::write(
        cfgdir.join("config.toml"),
        "[widgets.cpu]\nalt_format = \"{icon} {bar}\"\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["click", "--range=cpu", "--button=left"]);
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg"));
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
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg"));
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

/// W36: a plugin/unknown range (absent from `[widgets.*]`) must still flip on a
/// left-click, exactly as the pre-W36 `run_click` did — the invariant-#7
/// characterization at the binary boundary (unit-tested in `click.rs` too).
#[test]
fn click_toggles_unknown_plugin_range_by_default() {
    let tmp = tempfile::tempdir().unwrap();
    let toggles_path = tmp.path().join("data/rustline/toggles");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["click", "--range=weatherplug", "--button=left"]);
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let toggles = std::fs::read_to_string(&toggles_path).unwrap();
    assert!(
        toggles.contains("weatherplug"),
        "plugin toggled on: {toggles:?}"
    );
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
fn theme_new_prints_use_followup() {
    // `--edit` is deliberately NOT passed, so no editor spawn is attempted.
    let tmp = tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["theme", "new", "mytheme"]);
    isolate(&mut cmd, tmp.path());
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg"));
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("theme use mytheme"), "prints the follow-up: {s}");
    assert!(
        tmp.path()
            .join("cfg")
            .join("rustline")
            .join("themes")
            .join("mytheme.toml")
            .exists(),
        "scaffold file written"
    );
}

#[test]
fn theme_new_edit_without_editor_hints_and_writes_no_spawn() {
    // `--edit` with `$EDITOR` unset must degrade to a hint, never attempt to
    // spawn anything (which would hang this test waiting on a real editor).
    let tmp = tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["theme", "new", "mytheme", "--edit"]);
    isolate(&mut cmd, tmp.path());
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg"))
        .env_remove("EDITOR");
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("theme use mytheme"), "prints the follow-up: {s}");
    assert!(s.contains("set $EDITOR"), "hints to set $EDITOR: {s}");
    assert!(
        tmp.path()
            .join("cfg")
            .join("rustline")
            .join("themes")
            .join("mytheme.toml")
            .exists(),
        "scaffold file still written"
    );
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

/// A `rustline` invocation with an isolated HOME/XDG environment so logging,
/// config, and (W48) daemon-socket probing all read/write a throwaway tree,
/// never the developer's real dirs or a real daemon they might have running.
fn isolated_cmd(home: &Path, xdg_data: &Path, xdg_config: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_rustline"));
    c.env("HOME", home)
        .env("XDG_DATA_HOME", xdg_data)
        .env("XDG_CONFIG_HOME", xdg_config)
        .env("XDG_RUNTIME_DIR", xdg_data.join("runtime"))
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

#[test]
fn config_path_prints_resolved_path() {
    let tmp = tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["config", "path"]);
    isolate(&mut cmd, tmp.path());
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg"));
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let expected = tmp.path().join("cfg").join("rustline").join("config.toml");
    let s = String::from_utf8_lossy(&out.stdout);
    assert_eq!(s.trim(), expected.display().to_string());
}

#[test]
fn config_validate_missing_file_is_ok() {
    // Config::load treats an absent file as "use defaults", never an error;
    // validate agrees rather than turning absence into a false alarm.
    let tmp = tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["config", "validate"]);
    isolate(&mut cmd, tmp.path());
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg"));
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "missing file is not an error; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("using defaults"), "explains the outcome: {s}");
}

#[test]
fn config_validate_good_config_exits_ok() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("cfg").join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    fs::write(
        cfgdir.join("config.toml"),
        "[layout]\nright = [\"datetime\"]\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["config", "validate"]);
    isolate(&mut cmd, tmp.path());
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg"));
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "good config validates ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("ok"), "prints ok: {s}");
}

#[test]
fn config_validate_malformed_config_exits_nonzero() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("cfg").join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    fs::write(
        cfgdir.join("config.toml"),
        "this is = = not valid toml [[[\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["config", "validate"]);
    isolate(&mut cmd, tmp.path());
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg"));
    let out = cmd.output().unwrap();
    assert!(
        !out.status.success(),
        "malformed config must fail validation"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("invalid config"),
        "reports the parse error: {stderr}"
    );
}

#[test]
fn config_edit_creates_starter_when_missing_and_hints_without_editor() {
    // No TTY under Command and $EDITOR unset: must create the file from the
    // starter template and print a hint, never attempt to spawn anything
    // (which would hang this test waiting on a real editor).
    let tmp = tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["config", "edit"]);
    isolate(&mut cmd, tmp.path());
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg"))
        .env_remove("EDITOR");
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cfg_path = tmp.path().join("cfg").join("rustline").join("config.toml");
    assert!(cfg_path.is_file(), "starter config created");
    let text = fs::read_to_string(&cfg_path).unwrap();
    assert!(!text.is_empty(), "starter template is non-empty");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("set $EDITOR"), "hints to set $EDITOR: {s}");
}

#[test]
fn config_edit_does_not_recreate_existing_file() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("cfg").join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg_path = cfgdir.join("config.toml");
    let original = "[widgets.cpu]\nformat = \"USER {percent}%\"\n";
    fs::write(&cfg_path, original).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["config", "edit"]);
    isolate(&mut cmd, tmp.path());
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg"))
        .env_remove("EDITOR");
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(&cfg_path).unwrap(),
        original,
        "an existing config must not be overwritten by `config edit`"
    );
}

#[test]
fn doctor_runs_and_prints_resolved_paths() {
    // Host-dependent (tmux may or may not be installed, running, or have
    // mouse mode on wherever CI runs this), so this only checks that the
    // report runs and prints the resolved config/themes/plugin/log paths --
    // never a particular check outcome or exit code.
    let dir = tempdir().unwrap();
    let (home, data, config) = (
        dir.path().join("home"),
        dir.path().join("data"),
        dir.path().join("config"),
    );
    let out = isolated_cmd(&home, &data, &config)
        .arg("doctor")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("rustline doctor"),
        "prints a report header: {stdout}"
    );
    assert!(
        stdout.contains("Resolved paths:"),
        "prints the resolved-paths section: {stdout}"
    );

    let expected_config = config.join("rustline/config.toml");
    let expected_themes = config.join("rustline/themes");
    let expected_plugins = data.join("rustline/plugins");
    let expected_log = data.join("rustline/rustline.log");
    assert!(
        stdout.contains(&expected_config.display().to_string()),
        "resolved config path present: {stdout}"
    );
    assert!(
        stdout.contains(&expected_themes.display().to_string()),
        "resolved themes path present: {stdout}"
    );
    assert!(
        stdout.contains(&expected_plugins.display().to_string()),
        "resolved plugin dir present: {stdout}"
    );
    assert!(
        stdout.contains(&expected_log.display().to_string()),
        "resolved log path present: {stdout}"
    );
}

/// `doctor`'s "plugin checksums" row is purely advisory: a plugin with a
/// checksum that no longer matches its installed `.wasm` is named in the
/// report, but never flips `doctor`'s own pass/fail exit code. Proven by
/// comparing the exit code of a run with no plugins configured against one
/// with a deliberately mismatched checksum — equal either way, regardless of
/// what tmux/mouse/etc. happen to report in this environment.
#[test]
fn doctor_checksum_mismatch_is_advisory_and_never_affects_exit_code() {
    let dir = tempdir().unwrap();
    let (home, data, config) = (
        dir.path().join("home"),
        dir.path().join("data"),
        dir.path().join("config"),
    );

    let baseline = isolated_cmd(&home, &data, &config)
        .arg("doctor")
        .output()
        .unwrap();

    // Now configure a plugin whose recorded checksum can never match: no
    // .wasm file is even installed for it, so `plugin_checksum::status_for`
    // reports "missing" rather than reading real bytes -- but the mismatch
    // case below is the one that actually exercises `verify_checksum`'s
    // comparison, so set that up precisely.
    let plugin_dir = data.join("rustline/plugins");
    fs::create_dir_all(&plugin_dir).unwrap();
    fs::write(plugin_dir.join("weather.wasm"), b"the real installed bytes").unwrap();
    let cfgdir = config.join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    fs::write(
        cfgdir.join("config.toml"),
        format!(
            "[plugins.weather]\nchecksum = \"{}\"\n",
            rustline_wasm::sha256_hex(b"a completely different set of bytes")
        ),
    )
    .unwrap();

    let with_mismatch = isolated_cmd(&home, &data, &config)
        .arg("doctor")
        .output()
        .unwrap();

    assert_eq!(
        baseline.status.code(),
        with_mismatch.status.code(),
        "a checksum mismatch must not change doctor's exit code: baseline stderr={} \
         mismatch stderr={}",
        String::from_utf8_lossy(&baseline.stderr),
        String::from_utf8_lossy(&with_mismatch.stderr)
    );

    let stdout = String::from_utf8_lossy(&with_mismatch.stdout);
    assert!(
        stdout.contains("plugin checksums"),
        "checksum row present: {stdout}"
    );
    assert!(
        stdout.contains("weather") && stdout.contains("mismatch"),
        "names the mismatched plugin: {stdout}"
    );
    // The row itself must be `[warn]`, never `[fail]` -- the exit-code
    // equality above already proves this behaviorally, but pin the visible
    // label too since that's what a human actually reads.
    let checksum_line = stdout
        .lines()
        .find(|l| l.contains("plugin checksums"))
        .unwrap();
    assert!(
        checksum_line.contains("[warn"),
        "checksum row is advisory (warn), not fail: {checksum_line}"
    );
}

/// An install with no plugins configured at all gets a clean, unambiguous
/// "no plugins configured" row rather than an empty/confusing one.
#[test]
fn doctor_checksum_row_reports_no_plugins_configured() {
    let dir = tempdir().unwrap();
    let (home, data, config) = (
        dir.path().join("home"),
        dir.path().join("data"),
        dir.path().join("config"),
    );
    let out = isolated_cmd(&home, &data, &config)
        .arg("doctor")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let checksum_line = stdout
        .lines()
        .find(|l| l.contains("plugin checksums"))
        .unwrap();
    assert!(
        checksum_line.contains("no plugins configured"),
        "{checksum_line}"
    );
    assert!(
        checksum_line.contains("[ok"),
        "no plugins configured is a clean pass: {checksum_line}"
    );
}

/// `plugin approve <name> --yes` resolves the plugin's sidecar manifest and
/// writes exactly its requested urls/paths into `[plugins.<name>]`, preserving
/// comments, and is idempotent.
#[test]
fn plugin_approve_yes_writes_requested_capabilities() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    fs::write(&cfg, "# keepme\n[plugins.weather]\nallowed_urls = []\n").unwrap();

    // Sidecar manifest in the default plugin dir ($XDG_DATA_HOME/rustline/plugins).
    let plugin_dir = tmp.path().join("data/rustline/plugins");
    fs::create_dir_all(&plugin_dir).unwrap();
    fs::write(
        plugin_dir.join("weather.toml"),
        concat!(
            "name = \"weather\"\n",
            "version = \"0.1.0\"\n",
            "requested_urls = [\"https://wttr.in/*\"]\n",
            "requested_paths = [\"/tmp/weather\"]\n",
        ),
    )
    .unwrap();

    let approve = || {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
        cmd.args(["plugin", "approve", "weather", "--yes"])
            .env("XDG_CONFIG_HOME", tmp.path());
        isolate(&mut cmd, tmp.path());
        cmd.output().unwrap()
    };

    let out = approve();
    assert!(
        out.status.success(),
        "approve --yes exits 0: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let after = fs::read_to_string(&cfg).unwrap();
    assert!(after.contains("# keepme"), "comment preserved: {after}");
    assert!(after.contains("https://wttr.in/*"), "url written: {after}");
    assert!(after.contains("/tmp/weather"), "path written: {after}");

    // Second approval is a no-op (idempotent), not a duplicate.
    assert!(approve().status.success());
    let again = fs::read_to_string(&cfg).unwrap();
    assert_eq!(
        again.matches("https://wttr.in/*").count(),
        1,
        "no dup url: {again}"
    );
    assert_eq!(
        again.matches("/tmp/weather").count(),
        1,
        "no dup path: {again}"
    );
}

/// Declining the `plugin approve` prompt (no `--yes`, `n` on stdin) writes
/// nothing.
#[test]
fn plugin_approve_declined_writes_nothing() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    let original = "[plugins.weather]\nallowed_urls = []\n";
    fs::write(&cfg, original).unwrap();

    let plugin_dir = tmp.path().join("data/rustline/plugins");
    fs::create_dir_all(&plugin_dir).unwrap();
    fs::write(
        plugin_dir.join("weather.toml"),
        "requested_urls = [\"https://wttr.in/*\"]\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["plugin", "approve", "weather"])
        .env("XDG_CONFIG_HOME", tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    isolate(&mut cmd, tmp.path());
    let mut child = cmd.spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"n\n").unwrap();
    let out = child.wait_with_output().unwrap();

    assert!(out.status.success());
    assert_eq!(
        fs::read_to_string(&cfg).unwrap(),
        original,
        "declined approval leaves config untouched"
    );
}

/// `plugin list --json` surfaces a per-plugin `checksum_status`, computed the
/// same way `doctor`'s row is: read the installed `.wasm`, verify it via
/// `rustline_wasm::verify_checksum`.
#[test]
fn plugin_list_json_reports_checksum_status() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("cfg/rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let plugin_dir = tmp.path().join("data/rustline/plugins");
    fs::create_dir_all(&plugin_dir).unwrap();

    let bytes = b"a real installed plugin binary";
    fs::write(plugin_dir.join("weather.wasm"), bytes).unwrap();
    fs::write(
        cfgdir.join("config.toml"),
        format!(
            "[plugins.weather]\nchecksum = \"{}\"\n",
            rustline_wasm::sha256_hex(bytes)
        ),
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["plugin", "list", "--json"])
        .env("XDG_CONFIG_HOME", tmp.path().join("cfg"));
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let w = v
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "weather")
        .unwrap();
    assert_eq!(w["checksum_status"], "verified");
    // Additive: the previously-existing fields are all still there, unchanged.
    assert!(w.get("allowed_urls").is_some());
    assert!(w.get("has_manifest").is_some());
}

/// `plugin list` (human, non-JSON) prints a `checksum:` status line matching
/// the same computation.
#[test]
fn plugin_list_human_output_shows_checksum_status() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("cfg/rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let plugin_dir = tmp.path().join("data/rustline/plugins");
    fs::create_dir_all(&plugin_dir).unwrap();

    // Deliberately no .wasm installed for this one: "missing" must show up as
    // a plain status value, never an error that aborts the listing.
    fs::write(
        cfgdir.join("config.toml"),
        "[plugins.ghost]\nchecksum = \"deadbeef\"\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["plugin", "list"])
        .env("XDG_CONFIG_HOME", tmp.path().join("cfg"));
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("checksum: missing"), "{stdout}");
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

/// A global `--config <path>` overrides the resolved config file everywhere:
/// `config path` prints the override, not the XDG-derived default.
#[test]
fn config_flag_overrides_path() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join("alt.toml");
    fs::write(&cfg, "layout.left = [\"hostname\"]\n").unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["--config", cfg.to_str().unwrap(), "config", "path"]);
    isolate(&mut cmd, dir.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        s.trim(),
        cfg.display().to_string(),
        "prints the --config override, not the XDG default"
    );
}

/// `--config` also threads into the actual config load: `print-config`
/// reflects the alternate file's contents, not the (nonexistent) default one.
///
/// The sentinel must be absent from every default, or this test passes even
/// when `--config` is silently ignored (a prior version used `"hostname"`,
/// which is also in the *default* `left` layout, so it couldn't tell "the
/// override was honored" from "the default loaded"). `theme.base = "nord"`
/// has no such overlap: default `print-config` output has no `base` line at
/// all under `[theme]` (confirmed by the negative-control assertion below),
/// so `nord` can only appear in the output if the `--config` file was
/// genuinely loaded.
#[test]
fn config_flag_overrides_print_config_contents() {
    let dir = tempdir().unwrap();
    let (home, data, config) = (
        dir.path().join("home"),
        dir.path().join("data"),
        dir.path().join("config"),
    );
    let cfg = dir.path().join("alt.toml");
    fs::write(&cfg, "theme.base = \"nord\"\n").unwrap();

    let out = isolated_cmd(&home, &data, &config)
        .args(["--config", cfg.to_str().unwrap(), "print-config"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("base = \"nord\""),
        "reflects the --config file's theme.base override: {s}"
    );

    // Negative control: same isolated env, no `--config`, no config file at
    // all in `config` — the sentinel must be absent here, otherwise the
    // assertion above wouldn't discriminate "override honored" from
    // "override ignored, default loaded".
    let default_out = isolated_cmd(&home, &data, &config)
        .arg("print-config")
        .output()
        .unwrap();
    assert!(default_out.status.success(), "default print-config exit ok");
    let default_s = String::from_utf8_lossy(&default_out.stdout);
    assert!(
        !default_s.contains("nord"),
        "sentinel must be absent from default output: {default_s}"
    );
}

/// `--config` threads all the way into a mutating subcommand's write path
/// too (`plugin url add`), not just the read-only `config`/`print-config`
/// commands — it writes the override file and leaves the default untouched.
#[test]
fn config_flag_threads_into_plugin_mutator_write_path() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join("alt.toml");
    fs::write(&cfg, "[plugins.weather]\nallowed_urls = []\n").unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args([
        "--config",
        cfg.to_str().unwrap(),
        "plugin",
        "url",
        "add",
        "weather",
        "https://wttr.in/*",
    ]);
    isolate(&mut cmd, dir.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let after = fs::read_to_string(&cfg).unwrap();
    assert!(
        after.contains("https://wttr.in/*"),
        "writes into the --config override file: {after}"
    );

    let default_cfg = dir
        .path()
        .join("home")
        .join(".config")
        .join("rustline")
        .join("config.toml");
    assert!(
        !default_cfg.exists(),
        "must not write to the default XDG config path"
    );
}

/// `plugin approve` with no manifest present reports it and writes nothing.
#[test]
fn plugin_approve_no_manifest_writes_nothing() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    let original = "[plugins.weather]\nallowed_urls = []\n";
    fs::write(&cfg, original).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["plugin", "approve", "weather", "--yes"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("no manifest found"),
        "reports missing manifest: {stdout}"
    );
    assert_eq!(
        fs::read_to_string(&cfg).unwrap(),
        original,
        "no manifest leaves config untouched"
    );
}

/// W48: `rustline daemon status` with no daemon bound at the socket path
/// exits non-zero and reports "not running" on stderr. `isolate` pins
/// `XDG_RUNTIME_DIR` to an isolated tempdir, so this never depends on, or
/// collides with, a real daemon the developer might already have running on
/// their own machine.
#[test]
fn daemon_status_with_no_daemon_running_exits_nonzero() {
    let tmp = tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["daemon", "status"]);
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        !out.status.success(),
        "no daemon running -> non-zero exit; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not running"),
        "prints \"not running\": {stderr}"
    );
}

/// `rustline widget …` — enable/disable/move against the `[layout]` arrays,
/// exercising `widget_cmd.rs`'s `toml_edit`-backed writer end to end through
/// the real binary (Finding 2: previously pinned only by a manual transcript).

#[test]
fn widget_enable_adds_widget_to_a_region() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    fs::write(&cfg, "[layout]\nleft = [\"pane_id\"]\nright = [\"cwd\"]\n").unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["widget", "enable", "git", "--region", "right"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let after = fs::read_to_string(&cfg).unwrap();
    assert!(after.contains("git"), "git added to right region: {after}");
}

/// The load-bearing contract: a refused edit writes nothing at all.
#[test]
fn widget_enable_already_present_refuses_and_writes_nothing() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    let original = "[layout]\nleft = [\"pane_id\"]\nright = [\"cwd\", \"git\"]\n";
    fs::write(&cfg, original).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["widget", "enable", "git", "--region", "right"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        !out.status.success(),
        "enabling an already-placed widget must be refused"
    );
    assert_eq!(
        fs::read_to_string(&cfg).unwrap().as_bytes(),
        original.as_bytes(),
        "config file left byte-for-byte unchanged by a refused edit"
    );
}

#[test]
fn widget_enable_unknown_name_refuses_and_lists_available() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    let original = "[layout]\nleft = [\"pane_id\"]\nright = [\"cwd\"]\n";
    fs::write(&cfg, original).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["widget", "enable", "not_a_real_widget"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(!out.status.success(), "unknown widget name must be refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not_a_real_widget"),
        "names the bad widget: {stderr}"
    );
    assert!(stderr.contains("cwd"), "lists available widgets: {stderr}");
    assert_eq!(
        fs::read_to_string(&cfg).unwrap(),
        original,
        "config file left unchanged"
    );
}

#[test]
fn widget_enable_unknown_region_refuses() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    let original = "[layout]\nleft = [\"pane_id\"]\nright = [\"cwd\"]\n";
    fs::write(&cfg, original).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["widget", "enable", "git", "--region", "nope"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(!out.status.success(), "unknown region must be refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("nope"), "names the bad region: {stderr}");
    assert_eq!(
        fs::read_to_string(&cfg).unwrap(),
        original,
        "config file left unchanged"
    );
}

#[test]
fn widget_disable_removes_widget_and_exits_ok() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    fs::write(
        &cfg,
        "[layout]\nleft = [\"pane_id\"]\nright = [\"cwd\", \"git\"]\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["widget", "disable", "git"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let after = fs::read_to_string(&cfg).unwrap();
    assert!(!after.contains("git"), "git removed: {after}");
    assert!(after.contains("cwd"), "cwd still present: {after}");
}

/// `widget move <name> --region <r>` with no `--index` must append (the
/// `usize::MAX` sentinel flowing into `layout_move`'s clamp), not prepend.
#[test]
fn widget_move_without_index_appends_to_destination_region() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    fs::write(
        &cfg,
        "[layout]\nleft = [\"pane_id\", \"git\"]\nright = [\"cwd\"]\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["widget", "move", "git", "--region", "right"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let after = fs::read_to_string(&cfg).unwrap();
    let right_line = after
        .lines()
        .find(|l| l.trim_start().starts_with("right"))
        .expect("right line present");
    let cwd_pos = right_line.find("cwd").expect("cwd present");
    let git_pos = right_line.find("git").expect("git present");
    assert!(
        git_pos > cwd_pos,
        "git appended after cwd (not prepended): {right_line}"
    );
    let left_line = after
        .lines()
        .find(|l| l.trim_start().starts_with("left"))
        .expect("left line present");
    assert!(
        !left_line.contains("git"),
        "git removed from its old region: {left_line}"
    );
}

#[test]
fn widget_enable_preserves_comments_in_config() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    fs::write(
        &cfg,
        "# my rustline config\n[layout]\nleft = [\"pane_id\"]\nright = [\"cwd\"]\n\n\
         [widgets.cpu]\nformat = \"{percent}\"\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["widget", "enable", "git", "--region", "right"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let after = fs::read_to_string(&cfg).unwrap();
    assert!(
        after.contains("# my rustline config"),
        "comment preserved: {after}"
    );
    assert!(
        after.contains("[widgets.cpu]"),
        "other table preserved: {after}"
    );
    assert!(after.contains("git"), "widget added: {after}");
}

/// Finding 1 repro (positive path): `layout = { ... }` (an inline table) is
/// valid config that `Config::load`/`print-config` round-trip fine, and must
/// be edited correctly rather than silently discarded or refused.
#[test]
fn widget_enable_on_inline_table_layout_edits_correctly() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    fs::write(
        &cfg,
        "layout = { left = [\"pane_id\"], right = [\"cwd\"] }\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["widget", "enable", "git", "--region", "right"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "inline-table layout must be edited, not refused/panicked; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_ne!(out.status.code(), Some(101), "must not panic");
    let after = fs::read_to_string(&cfg).unwrap();
    assert!(after.contains("git"), "widget added: {after}");
    assert!(after.contains("pane_id"), "left untouched: {after}");
}

/// Finding 1 repro (negative path): a genuinely non-table `layout` value
/// (`layout = "oops"`) must exit 1 cleanly — never panic (exit 101) — and
/// leave the file untouched.
#[test]
fn widget_enable_on_scalar_layout_refuses_without_panicking() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    let original = "layout = \"oops\"\n";
    fs::write(&cfg, original).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["widget", "enable", "git", "--region", "right"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "clean refusal, not a panic (exit 101); stderr={stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "must not panic: stderr={stderr}"
    );
    assert_eq!(
        fs::read_to_string(&cfg).unwrap(),
        original,
        "config file left byte-for-byte unchanged"
    );
}

/// `widget enable --region center` and `widget move --region center` must
/// NOT be refused — `center = ["windows"]` is the default config and has to
/// stay writable so a legitimate round-trip (e.g. re-writing the default
/// layout, or a config generated by another tool) never breaks. They must,
/// however, warn on stderr that tmux renders the window list in CENTER
/// itself, so a user isn't left wondering why the edit had no visible effect
/// on the rendered bar.
#[test]
fn widget_enable_region_center_still_writes_and_warns() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    fs::write(
        &cfg,
        "[layout]\nleft = [\"pane_id\"]\ncenter = [\"windows\"]\nright = [\"cwd\"]\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["widget", "enable", "git", "--region", "center"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "a center edit must still succeed; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let after = fs::read_to_string(&cfg).unwrap();
    assert!(after.contains("git"), "git added to center: {after}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("center") && stderr.contains("window"),
        "warns that tmux owns the window list in center: {stderr}"
    );
}

/// Same warn-but-succeed contract for `widget move --region center`.
#[test]
fn widget_move_region_center_still_writes_and_warns() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    fs::write(
        &cfg,
        "[layout]\nleft = [\"pane_id\", \"git\"]\ncenter = [\"windows\"]\nright = [\"cwd\"]\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["widget", "move", "git", "--region", "center"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "a center edit must still succeed; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let after = fs::read_to_string(&cfg).unwrap();
    let center_line = after
        .lines()
        .find(|l| l.trim_start().starts_with("center"))
        .expect("center line present");
    assert!(
        center_line.contains("git"),
        "git moved into center: {after}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("center") && stderr.contains("window"),
        "warns that tmux owns the window list in center: {stderr}"
    );
}

/// Same warn-but-succeed contract for `widget disable` of a widget that was
/// sitting in center — `widget disable` has no `--region` flag, so the
/// warning must be keyed off the region `layout_disable` actually removed
/// the widget from.
#[test]
fn widget_disable_from_center_still_writes_and_warns() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    fs::write(
        &cfg,
        "[layout]\nleft = [\"pane_id\"]\ncenter = [\"windows\"]\nright = [\"cwd\"]\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["widget", "disable", "windows"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "a center edit must still succeed; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let after = fs::read_to_string(&cfg).unwrap();
    let center_line = after
        .lines()
        .find(|l| l.trim_start().starts_with("center"))
        .expect("center line present");
    assert!(
        !center_line.contains("windows"),
        "windows removed from center: {after}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("center") && stderr.contains("window"),
        "warns that tmux owns the window list in center: {stderr}"
    );
}

/// I4: `disable` must operate on a placed name regardless of whether the
/// widget catalog (built-ins + instances + discovered plugins) recognizes
/// it — a layout can name a plugin whose `.wasm` is no longer present (a
/// fresh clone, an empty plugin dir, `plugin remove` without cleaning up
/// the layout), and the CLI must still be able to remove it.
#[test]
fn widget_disable_removes_a_placed_but_unknown_entry_and_exits_ok() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    fs::write(
        &cfg,
        "[layout]\nleft = [\"pane_id\"]\nright = [\"cwd\", \"ghostwidget\"]\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["widget", "disable", "ghostwidget"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "disable of a placed-but-unrecognized name must succeed; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let after = fs::read_to_string(&cfg).unwrap();
    assert!(!after.contains("ghostwidget"), "removed: {after}");
    assert!(after.contains("cwd"), "cwd still present: {after}");
}

/// `move` must have the same "operates on what's placed, regardless of
/// catalog membership" behavior as `disable`.
#[test]
fn widget_move_relocates_a_placed_but_unknown_entry() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    fs::write(
        &cfg,
        "[layout]\nleft = [\"pane_id\", \"ghostwidget\"]\nright = [\"cwd\"]\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["widget", "move", "ghostwidget", "--region", "right"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "move of a placed-but-unrecognized name must succeed; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let after = fs::read_to_string(&cfg).unwrap();
    let left_line = after
        .lines()
        .find(|l| l.trim_start().starts_with("left"))
        .expect("left line present");
    assert!(
        !left_line.contains("ghostwidget"),
        "removed from its old region: {left_line}"
    );
    let right_line = after
        .lines()
        .find(|l| l.trim_start().starts_with("right"))
        .expect("right line present");
    assert!(
        right_line.contains("ghostwidget"),
        "present in its new region: {right_line}"
    );
}

/// `enable` is the one verb that can introduce a name not already in the
/// layout, so it alone must still refuse an unrecognized name — otherwise a
/// typo would silently write garbage into `[layout]`.
#[test]
fn widget_enable_of_an_unknown_name_still_refuses() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    let original = "[layout]\nleft = [\"pane_id\"]\nright = [\"cwd\"]\n";
    fs::write(&cfg, original).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["widget", "enable", "totally-bogus"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        !out.status.success(),
        "enable of a name that isn't placed anywhere and isn't in the \
         catalog must still be refused"
    );
    assert_eq!(
        fs::read_to_string(&cfg).unwrap(),
        original,
        "config file left unchanged"
    );
}

/// Skipping the catalog check for `disable`/`move` (RequireKnown::No) must
/// not weaken their existing refusal of a name that isn't placed *anywhere*
/// (not even as an unrecognized layout entry) — `layout_disable`'s own
/// `LayoutEditError::NotPresent` is still the backstop.
#[test]
fn widget_disable_of_a_name_placed_nowhere_still_refuses_and_writes_nothing() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    let original = "[layout]\nleft = [\"pane_id\"]\nright = [\"cwd\"]\n";
    fs::write(&cfg, original).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["widget", "disable", "never-placed-anywhere"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        !out.status.success(),
        "disable of a name that was never placed anywhere must be refused"
    );
    assert_eq!(
        fs::read_to_string(&cfg).unwrap(),
        original,
        "config file left unchanged"
    );
}

/// `widget list`/`widget list --json` must surface a placed-but-unrecognized
/// entry, clearly flagged, rather than silently under-reporting the layout.
#[test]
fn widget_list_surfaces_a_placed_but_unknown_entry() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    fs::write(
        &cfg,
        "[layout]\nleft = [\"pane_id\"]\nright = [\"cwd\", \"ghostwidget\"]\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["widget", "list"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("ghostwidget"),
        "the placed-but-unrecognized name is listed: {stdout}"
    );
    assert!(
        stdout.contains("unknown"),
        "it's clearly flagged as unknown: {stdout}"
    );

    let mut json_cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    json_cmd
        .args(["widget", "list", "--json"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut json_cmd, tmp.path());
    let json_out = json_cmd.output().unwrap();
    assert!(json_out.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&json_out.stdout).unwrap();
    let rows = parsed.as_array().expect("top-level array");
    let ghost = rows
        .iter()
        .find(|r| r["name"] == "ghostwidget")
        .expect("ghostwidget present in --json output");
    assert_eq!(ghost["source"], "unknown");
    assert_eq!(ghost["region"], "right");
}

/// A disable out of `left`/`right` must NOT print the center warning — guards
/// against a version of the fix that warns unconditionally on every disable.
#[test]
fn widget_disable_from_non_center_region_prints_no_center_warning() {
    let tmp = tempdir().unwrap();
    let cfgdir = tmp.path().join("rustline");
    fs::create_dir_all(&cfgdir).unwrap();
    let cfg = cfgdir.join("config.toml");
    fs::write(
        &cfg,
        "[layout]\nleft = [\"pane_id\"]\nright = [\"cwd\", \"git\"]\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["widget", "disable", "git"])
        .env("XDG_CONFIG_HOME", tmp.path());
    isolate(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "exit ok; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let after = fs::read_to_string(&cfg).unwrap();
    assert!(!after.contains("git"), "git removed: {after}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("center"),
        "no center warning for a non-center disable: {stderr}"
    );
}

/// Real-clock unix seconds, for stamping a seeded plugin-index cache as
/// genuinely fresh. `plugin_index.rs`'s `index_is_fresh` is
/// `now >= fetched_at && now - fetched_at < ttl` — a `fetched_at` in the
/// FUTURE fails `now >= fetched_at` and is therefore **stale**, deliberately
/// (a backward clock must force a refetch rather than pin a cache fresh
/// forever; see `a_backward_clock_counts_as_stale_rather_than_forever_fresh`).
/// So a current-or-past timestamp is what makes a seeded cache fresh, not a
/// far-future one.
fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Seed the plugin-index cache so `plugin search` answers from disk and never
/// touches the network. Path mirrors `state_root()` under the `XDG_DATA_HOME`
/// that `isolate` sets. Stamped with the real current clock (`unix_now_secs`)
/// so the cache is genuinely inside the TTL and `load_index` returns it
/// without ever attempting a fetch.
fn seed_index_cache(tmp: &Path) {
    let state = tmp.join("data/rustline/state");
    fs::create_dir_all(&state).expect("state dir");
    let body = format!(
        r#"{{"fetched_at":{},"index":{{"schema_version":1,"plugins":[
        {{"name":"weather","description":"Weather from wttr.in","source":"o/r","bundled":true,"capabilities":["http_cached"]}},
        {{"name":"othertool","description":"Something else entirely","source":"o/r2","bundled":false,"capabilities":[]}}
    ]}}}}"#,
        unix_now_secs()
    );
    fs::write(state.join("plugin-index.json"), body).expect("seed index cache");
}

/// Belt-and-braces alongside a fresh-stamped seeded cache: point
/// `XDG_CONFIG_HOME` at a config that overrides `plugin_index_url` to a
/// loopback address nothing listens on. Even if a freshness regression crept
/// back in and `plugin search` attempted a fetch, it would fail fast and
/// offline (connection refused) instead of ever reaching the real
/// `DEFAULT_INDEX_URL` on GitHub — these tests must never depend on network
/// reachability, or on that URL 404ing before the branch merges.
fn isolate_with_dead_plugin_index(cmd: &mut Command, tmp: &Path) {
    isolate(cmd, tmp);
    let cfg_dir = tmp.join("cfg/rustline");
    fs::create_dir_all(&cfg_dir).expect("cfg dir");
    fs::write(
        cfg_dir.join("config.toml"),
        r#"plugin_index_url = "http://127.0.0.1:1/index.json""#,
    )
    .expect("seed config");
    cmd.env("XDG_CONFIG_HOME", tmp.join("cfg"));
}

/// Guards the *reason* the other `plugin_search_*` tests are hermetic.
///
/// They all seed a fresh cache AND point `plugin_index_url` at a dead loopback,
/// so if the freshness stamping ever regressed (e.g. back to a far-future
/// `fetched_at`, which `index_is_fresh` treats as STALE), they would still pass
/// — the failed fetch falls back to the same cached content and only stderr
/// differs. That would silently restore the network round-trip this suite
/// exists to avoid. Asserting the staleness warning is *absent* pins
/// "no fetch was attempted" rather than merely "the right bytes came out".
#[test]
fn plugin_search_with_a_fresh_cache_attempts_no_fetch() {
    let tmp = tempdir().unwrap();
    seed_index_cache(tmp.path());
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["plugin", "search"]);
    isolate_with_dead_plugin_index(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("could not refresh"),
        "a fresh cache must be served without any fetch attempt; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("weather"), "cache still served: {stdout}");
}

#[test]
fn plugin_search_lists_the_index() {
    let tmp = tempdir().unwrap();
    seed_index_cache(tmp.path());
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["plugin", "search"]);
    isolate_with_dead_plugin_index(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();

    assert!(out.status.success(), "plugin search should succeed");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("weather"), "index entries listed: {s}");
    assert!(s.contains("othertool"), "{s}");
    assert!(
        s.contains("just build-plugin weather"),
        "a bundled entry shows a build hint, not an install command: {s}"
    );
    assert!(
        s.contains("rustline plugin install o/r2"),
        "a non-bundled entry shows its install command: {s}"
    );
}

#[test]
fn plugin_search_filters_by_query() {
    let tmp = tempdir().unwrap();
    seed_index_cache(tmp.path());
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["plugin", "search", "weath"]);
    isolate_with_dead_plugin_index(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();

    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("weather"), "{s}");
    assert!(
        !s.contains("othertool"),
        "the query should exclude non-matches: {s}"
    );
}

#[test]
fn plugin_search_json_emits_an_array() {
    let tmp = tempdir().unwrap();
    seed_index_cache(tmp.path());
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["plugin", "search", "--json"]);
    isolate_with_dead_plugin_index(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();

    let s = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&s)
        .unwrap_or_else(|e| panic!("--json must emit valid JSON: {e}: {s}"));
    let arr = v.as_array().expect("array");
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["name"], "weather");
    assert_eq!(
        arr[0]["installed"], false,
        "nothing installed in the tempdir"
    );
    assert_eq!(arr[1]["source"], "o/r2");
}

#[test]
fn plugin_search_prints_no_action_hint_when_neither_bundled_nor_sourced() {
    // Coverage gap the review flagged: the `(false, None)` arm of `search`'s
    // bundled/source match (not bundled AND no recorded `source`) was never
    // exercised. An entry with neither must print neither a `just
    // build-plugin` hint nor a `plugin install` hint.
    let tmp = tempdir().unwrap();
    let state = tmp.path().join("data/rustline/state");
    fs::create_dir_all(&state).expect("state dir");
    let body = format!(
        r#"{{"fetched_at":{},"index":{{"schema_version":1,"plugins":[
        {{"name":"mystery","description":"No source, not bundled"}}
    ]}}}}"#,
        unix_now_secs()
    );
    fs::write(state.join("plugin-index.json"), body).expect("seed index cache");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["plugin", "search"]);
    isolate_with_dead_plugin_index(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();

    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("mystery"), "{s}");
    assert!(
        !s.contains("build-plugin"),
        "not bundled, no build hint: {s}"
    );
    assert!(
        !s.contains("plugin install"),
        "no recorded source, no install hint: {s}"
    );
}

#[test]
fn plugin_search_reports_no_matches_for_an_unmatched_query() {
    let tmp = tempdir().unwrap();
    seed_index_cache(tmp.path());
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["plugin", "search", "zzz-no-such-plugin"]);
    isolate_with_dead_plugin_index(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();

    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains(r#"no plugins in the index match "zzz-no-such-plugin""#),
        "{s}"
    );
}

#[test]
fn plugin_search_reports_an_empty_index() {
    let tmp = tempdir().unwrap();
    let state = tmp.path().join("data/rustline/state");
    fs::create_dir_all(&state).expect("state dir");
    let body = format!(
        r#"{{"fetched_at":{},"index":{{"schema_version":1,"plugins":[]}}}}"#,
        unix_now_secs()
    );
    fs::write(state.join("plugin-index.json"), body).expect("seed index cache");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["plugin", "search"]);
    isolate_with_dead_plugin_index(&mut cmd, tmp.path());
    let out = cmd.output().unwrap();

    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("the plugin index is empty"), "{s}");
}

#[test]
fn plugin_search_json_warns_of_staleness_on_stderr() {
    // Pins Defect 2's fix: `search`'s stale-cache warning must reach stderr
    // even under `--json` — previously the `--json` early return happened
    // BEFORE the staleness check ran at all, so a scripted `--json` consumer
    // had no way to learn its result was a day-old cached copy.
    let tmp = tempdir().unwrap();

    // Seed a STALE cache: `fetched_at=0` is unconditionally outside the 24h
    // TTL, so `load_index` attempts a refetch and falls back to this cache
    // (flagged `stale`) when that refetch fails.
    let state = tmp.path().join("data/rustline/state");
    fs::create_dir_all(&state).expect("state dir");
    let body = r#"{"fetched_at":0,"index":{"schema_version":1,"plugins":[
        {"name":"weather","description":"Weather from wttr.in","source":"o/r","bundled":true,"capabilities":["http_cached"]}
    ]}}"#;
    fs::write(state.join("plugin-index.json"), body).expect("seed index cache");

    // Point `plugin_index_url` at a loopback address nothing listens on, so
    // the refetch this triggers fails fast and offline (connection refused)
    // instead of ever reaching the real `DEFAULT_INDEX_URL` on GitHub — keeps
    // this test hermetic.
    let cfg_dir = tmp.path().join("cfg/rustline");
    fs::create_dir_all(&cfg_dir).expect("cfg dir");
    fs::write(
        cfg_dir.join("config.toml"),
        r#"plugin_index_url = "http://127.0.0.1:1/index.json""#,
    )
    .expect("seed config");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.args(["plugin", "search", "--json"]);
    isolate(&mut cmd, tmp.path());
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg"));
    let out = cmd.output().unwrap();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "search must still succeed, serving the stale cache: stderr={stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("stdout must still be valid JSON when serving a stale cache: {e}\n{stdout}")
    });
    let arr = v.as_array().expect("a JSON array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "weather");

    assert!(
        stderr.contains("could not refresh the plugin index"),
        "the staleness warning must reach stderr even under --json: {stderr}"
    );
}
