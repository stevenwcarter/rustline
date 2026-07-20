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
