//! `rustline theme …` — list/preview/select/scaffold themes. Config mutations
//! (`use`) go through `toml_edit` so comments/formatting survive, mirroring
//! `plugin_cmd`.

use std::path::Path;

use chrono::Local;
use rustline_core::{
    Battery, BatteryState, Config, Context, CpuUsage, Direction, MemInfo, Registry, Theme,
    ThemeConfig, WindowCtx, builtin_theme, builtin_theme_names, render_named_region, render_window,
    tmux_to_ansi,
};

use crate::cli::ThemeCmd;

/// Dispatch a `rustline theme …` invocation.
pub fn run(cmd: ThemeCmd, config_path: &Path, themes_dir: &Path) {
    match cmd {
        ThemeCmd::List => list(config_path, themes_dir),
        ThemeCmd::Show { name } => show(&name, themes_dir),
        ThemeCmd::Use { name } => {
            let _ = (&name, config_path); // Task 13
        }
        ThemeCmd::New { name, from, force } => {
            let _ = (&name, &from, force, themes_dir); // Task 14
        }
    }
}

/// Read the themes-dir `*.toml` stems (empty on any error).
fn theme_files(themes_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(themes_dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension()?.to_str()? == "toml")
                .then(|| p.file_stem()?.to_str().map(str::to_string))
                .flatten()
        })
        .collect();
    names.sort();
    names
}

/// Build the `list` output lines. `active` is the current base (or "default").
///
/// Base resolution is file-first (`resolve_base_theme` in `main.rs`): when a
/// themes-dir file shadows a built-in of the same name, only the FILE is
/// actually active, so the built-in line must not also claim `*`.
fn list_lines(active: &str, files: &[String]) -> Vec<String> {
    let mut lines = Vec::new();
    for name in builtin_theme_names() {
        let shadowed_by_file = files.iter().any(|f| f == name);
        let is_active = *name == active && !shadowed_by_file;
        let mark = if is_active { " *" } else { "" };
        let shadowed = if shadowed_by_file {
            "  (shadowed by file)"
        } else {
            ""
        };
        lines.push(format!("{name}  (built-in){mark}{shadowed}"));
    }
    for f in files {
        let mark = if f == active { " *" } else { "" };
        lines.push(format!("{f}  (file){mark}"));
    }
    lines
}

fn list(config_path: &Path, themes_dir: &Path) {
    let cfg = Config::load(config_path);
    let active = cfg.theme.base.as_deref().unwrap_or("default");
    for line in list_lines(active, &theme_files(themes_dir)) {
        println!("{line}");
    }
}

/// A representative synthetic Context that trips warning+error badges so a
/// preview exercises the semantic colors. `colors` come from `theme`.
fn sample_context(theme: &Theme) -> Context {
    Context {
        session_name: "0".into(),
        window_index: "1".into(),
        pane_index: "0".into(),
        pane_current_path: "/home/steve/src/rustline".into(),
        home: "/home/steve".into(),
        hostname: "scadrial".into(),
        loadavg: Some([0.42, 0.31, 0.30]),
        now: Local::now(),
        window: None,
        interfaces: Vec::new(),
        battery: Some(Battery {
            percent: 15,
            state: BatteryState::Discharging,
        }),
        cpu: Some(CpuUsage { percent: 96.0 }),
        memory: Some(MemInfo {
            total_bytes: 16 * 1024u64.pow(3),
            used_bytes: 14 * 1024u64.pow(3),
            available_bytes: 2 * 1024u64.pow(3),
        }),
        os: "linux".into(),
        arch: "x86_64".into(),
        toggled: Default::default(),
        colors: theme.colors(),
    }
}

/// Render a labelled, ANSI-coloured preview of `theme`. Uses the default left
/// layout plus an explicit right list that includes `battery` (not in the
/// default layout) so its alert badge shows, and both an active and inactive
/// window pill.
fn preview_theme_ansi(theme: &Theme) -> String {
    let cfg = Config::default();
    let reg = Registry::with_builtins(&cfg);
    let mut ctx = sample_context(theme);
    let right = vec![
        "cwd".to_string(),
        "cpu".to_string(),
        "memory".to_string(),
        "battery".to_string(),
        "loadavg".to_string(),
        "datetime".to_string(),
    ];
    let left = render_named_region(Direction::Left, &cfg.layout.left, &ctx, &reg, theme);
    let right_out = render_named_region(Direction::Right, &right, &ctx, &reg, theme);
    ctx.window = Some(WindowCtx {
        index: "1".into(),
        name: "shell".into(),
        flags: "*".into(),
        is_current: true,
    });
    let win_active = render_window(&ctx, &reg, theme);
    ctx.window = Some(WindowCtx {
        index: "2".into(),
        name: "editor".into(),
        flags: String::new(),
        is_current: false,
    });
    let win_inactive = render_window(&ctx, &reg, theme);
    format!(
        "LEFT   : {}\nCENTER : {}{}\nRIGHT  : {}",
        tmux_to_ansi(&left),
        tmux_to_ansi(&win_active),
        tmux_to_ansi(&win_inactive),
        tmux_to_ansi(&right_out),
    )
}

/// Render a labelled, ANSI-coloured preview of the built-in theme `name`, or
/// `None` if unknown.
fn preview_ansi(name: &str) -> Option<String> {
    Some(preview_theme_ansi(&builtin_theme(name)?))
}

/// Preview theme `name`: a themes-dir `<name>.toml` file shadows a same-named
/// built-in (file-first, mirroring `resolve_base_theme` in `main.rs`); an
/// invalid file or an unknown name exits non-zero rather than panicking.
fn show(name: &str, themes_dir: &Path) {
    let file = themes_dir.join(format!("{name}.toml"));
    if let Ok(text) = std::fs::read_to_string(&file) {
        match toml::from_str::<ThemeConfig>(&text) {
            Ok(tc) => {
                let mut t = Theme::default();
                tc.apply_to(&mut t);
                println!("{}", preview_theme_ansi(&t));
                return;
            }
            Err(e) => {
                eprintln!("invalid theme file {}: {e}", file.display());
                std::process::exit(1);
            }
        }
    }
    match preview_ansi(name) {
        Some(s) => println!("{s}"),
        None => {
            eprintln!(
                "unknown theme: {name}\navailable: {}",
                builtin_theme_names().join(", ")
            );
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn list_lines_mark_active_and_shadowed() {
        // built-ins: default active; a "nord" file shadows the built-in nord.
        let files = vec!["nord".to_string(), "mine".to_string()];
        let lines = super::list_lines("pastel-rainbow", &files);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("pastel-rainbow") && l.contains('*'))
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("nord") && l.contains("shadowed"))
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("mine") && l.contains("file"))
        );
    }

    #[test]
    fn list_lines_active_builtin_shadowed_by_file_marks_only_the_file() {
        // "nord" is both a built-in and a themes-dir file, and it's the active
        // base. Resolution is file-first (see resolve_base_theme in main.rs),
        // so only the FILE line is actually active — the built-in line must
        // not also claim `*`.
        let files = vec!["nord".to_string()];
        let lines = super::list_lines("nord", &files);
        let builtin_line = lines
            .iter()
            .find(|l| l.contains("nord") && l.contains("(built-in)"))
            .expect("built-in nord line present");
        assert!(
            !builtin_line.contains('*'),
            "shadowed built-in must not be marked active: {builtin_line:?}"
        );
        let file_line = lines
            .iter()
            .find(|l| l.contains("nord") && l.contains("(file)"))
            .expect("file nord line present");
        assert!(
            file_line.contains('*'),
            "the shadowing file must be marked active: {file_line:?}"
        );
    }

    #[test]
    fn preview_ansi_is_nonempty_and_colored_for_builtin() {
        let out = super::preview_ansi("nord").expect("known theme");
        assert!(out.contains('\u{1b}'), "contains ANSI escape: {out:?}");
        assert!(out.contains("RIGHT"), "labels the right region");
        assert!(super::preview_ansi("nope").is_none());
    }
}
