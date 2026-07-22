//! `rustline theme …` — list/preview/select/scaffold themes. Config mutations
//! (`use`) go through `toml_edit` so comments/formatting survive, mirroring
//! `plugin_cmd`.

use std::path::Path;

use chrono::Local;
use rustline_core::{
    Battery, BatteryState, Color, Config, Context, CpuUsage, Direction, MemInfo, Registry, Theme,
    ThemeConfig, WindowCtx, builtin_theme, builtin_theme_names, render_named_region, render_window,
    tmux_to_ansi,
};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value as EditValue, value};

use crate::cli::ThemeCmd;

/// Dispatch a `rustline theme …` invocation.
pub fn run(cmd: ThemeCmd, config_path: &Path, themes_dir: &Path) {
    match cmd {
        ThemeCmd::List => list(config_path, themes_dir),
        ThemeCmd::Show { name } => show(&name, themes_dir),
        ThemeCmd::Use { name } => use_theme(&name, config_path, themes_dir),
        ThemeCmd::New { name, from, force } => new_theme(&name, &from, force, themes_dir),
    }
}

/// Whether `name` resolves to a themes-dir file or a built-in.
fn resolvable(name: &str, themes_dir: &Path) -> bool {
    themes_dir.join(format!("{name}.toml")).is_file() || builtin_theme(name).is_some()
}

/// Set `[theme].base = name` in `doc`, creating `[theme]` if absent. Other
/// keys/comments are untouched. If `theme` exists but isn't a table (e.g. a
/// stray `theme = "dark"` in a hand-edited config), reports the error and
/// exits rather than panicking — same shape as `plugin_cmd::mutate`'s
/// not-a-table guards.
fn set_base(doc: &mut DocumentMut, name: &str) {
    let theme = match doc
        .entry("theme")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
    {
        Some(t) => t,
        None => {
            eprintln!("config error: `theme` is not a table");
            std::process::exit(1);
        }
    };
    theme.set_implicit(false);
    theme["base"] = value(name);
}

/// Set the config's active theme base to `name`, validating first that it
/// resolves to a themes-dir file or a built-in. Mirrors `plugin_cmd::mutate`'s
/// read/parse guard: a missing config starts fresh, but an unreadable or
/// invalid one aborts *before* any write so `theme use` never truncates a
/// config it merely failed to parse.
fn use_theme(name: &str, config_path: &Path, themes_dir: &Path) {
    if !resolvable(name, themes_dir) {
        eprintln!(
            "unknown theme: {name}\navailable built-ins: {}",
            builtin_theme_names().join(", ")
        );
        std::process::exit(1);
    }
    let mut doc = match std::fs::read_to_string(config_path) {
        Ok(text) => match text.parse::<DocumentMut>() {
            Ok(doc) => doc,
            Err(_) => {
                eprintln!(
                    "config error: {} is not valid TOML; refusing to overwrite",
                    config_path.display()
                );
                std::process::exit(1);
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DocumentMut::new(),
        Err(e) => {
            eprintln!("config error: cannot read {}: {e}", config_path.display());
            std::process::exit(1);
        }
    };
    set_base(&mut doc, name);
    if let Some(parent) = config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(config_path, doc.to_string()) {
        eprintln!("failed to write config: {e}");
        std::process::exit(1);
    }
    println!("theme set to {name}");
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

/// A theme name is a bare filename stem: non-empty, no path separators or `..`.
fn valid_theme_name(name: &str) -> bool {
    !name.is_empty()
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
}

/// A `Color` as a `toml_edit` inline value in the documented enum form, e.g.
/// `{ Rgb = [42, 42, 54] }`, `{ Indexed = 31 }`, `{ Named = "cyan" }`.
fn color_value(c: &Color) -> EditValue {
    let mut t = InlineTable::new();
    match c {
        Color::Named(n) => {
            t.insert("Named", n.as_str().into());
        }
        Color::Indexed(i) => {
            t.insert("Indexed", (*i as i64).into());
        }
        Color::Rgb(r, g, b) => {
            let mut arr = Array::new();
            arr.push(*r as i64);
            arr.push(*g as i64);
            arr.push(*b as i64);
            t.insert("Rgb", EditValue::Array(arr));
        }
    }
    EditValue::InlineTable(t)
}

/// Build the full theme-file TOML text for `theme`, with a header comment.
fn scaffold_toml(name: &str, from: &str, theme: &Theme) -> String {
    let tc = ThemeConfig::from_theme(theme);
    let mut doc = DocumentMut::new();
    // Colors as inline tables; strings/arrays as their natural forms.
    macro_rules! put_color {
        ($k:literal, $v:expr) => {
            if let Some(c) = &$v {
                doc[$k] = Item::Value(color_value(c));
            }
        };
    }
    macro_rules! put_str {
        ($k:literal, $v:expr) => {
            if let Some(s) = &$v {
                doc[$k] = value(s.as_str());
            }
        };
    }
    if let Some(palette) = &tc.palette {
        let mut arr = Array::new();
        for c in palette {
            arr.push(color_value(c));
        }
        doc["palette"] = Item::Value(EditValue::Array(arr));
    }
    put_color!("fg", tc.fg);
    put_color!("bar_bg", tc.bar_bg);
    put_str!("hard_left", tc.hard_left);
    put_str!("hard_right", tc.hard_right);
    put_str!("soft_left", tc.soft_left);
    put_str!("soft_right", tc.soft_right);
    put_color!("soft_fg", tc.soft_fg);
    put_str!("win_cap_left", tc.win_cap_left);
    put_str!("win_cap_right", tc.win_cap_right);
    put_color!("win_current_bg", tc.win_current_bg);
    put_color!("win_current_fg", tc.win_current_fg);
    put_color!("win_inactive_bg", tc.win_inactive_bg);
    put_color!("win_inactive_fg", tc.win_inactive_fg);
    put_color!("success", tc.success);
    put_color!("info", tc.info);
    put_color!("warning", tc.warning);
    put_color!("error", tc.error);
    format!(
        "# rustline theme \"{name}\" (seeded from \"{from}\")\n# select with: rustline theme use {name}\n{doc}"
    )
}

/// Scaffold `<themes_dir>/<name>.toml` seeded from theme `from` (a themes-dir
/// file first, then a built-in). Refuses an invalid `name`, an unknown seed,
/// or overwriting an existing file unless `force`.
fn new_theme(name: &str, from: &str, force: bool, themes_dir: &Path) {
    if !valid_theme_name(name) {
        eprintln!("invalid theme name: {name:?} (no empty, `/`, `\\`, or `..`)");
        std::process::exit(1);
    }
    // Resolve the seed: themes-dir file first, then built-in.
    let seed = {
        let file = themes_dir.join(format!("{from}.toml"));
        if let Ok(text) = std::fs::read_to_string(&file) {
            match toml::from_str::<ThemeConfig>(&text) {
                Ok(tc) => {
                    let mut t = Theme::default();
                    tc.apply_to(&mut t);
                    t
                }
                Err(e) => {
                    eprintln!("invalid seed theme file {}: {e}", file.display());
                    std::process::exit(1);
                }
            }
        } else {
            match builtin_theme(from) {
                Some(t) => t,
                None => {
                    eprintln!("unknown seed theme: {from}");
                    std::process::exit(1);
                }
            }
        }
    };
    let dest = themes_dir.join(format!("{name}.toml"));
    if dest.exists() && !force {
        eprintln!(
            "{} already exists (use --force to overwrite)",
            dest.display()
        );
        std::process::exit(1);
    }
    if let Err(e) = std::fs::create_dir_all(themes_dir) {
        eprintln!("cannot create themes dir {}: {e}", themes_dir.display());
        std::process::exit(1);
    }
    if let Err(e) = std::fs::write(&dest, scaffold_toml(name, from, &seed)) {
        eprintln!("failed to write {}: {e}", dest.display());
        std::process::exit(1);
    }
    println!("wrote {}", dest.display());
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

    #[test]
    fn set_base_preserves_comments_and_creates_theme_table() {
        use toml_edit::DocumentMut;
        let mut doc = "# my config\n[layout]\nright = [\"datetime\"]\n"
            .parse::<DocumentMut>()
            .unwrap();
        super::set_base(&mut doc, "nord");
        let s = doc.to_string();
        assert!(s.contains("# my config"), "comment preserved: {s}");
        assert!(s.contains("[theme]"), "theme table created: {s}");
        assert!(s.contains("base = \"nord\""), "base set: {s}");
        // idempotent overwrite
        super::set_base(&mut doc, "gruvbox");
        assert!(doc.to_string().contains("base = \"gruvbox\""));
        assert!(!doc.to_string().contains("nord"));
    }

    #[test]
    fn resolvable_accepts_builtin_and_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("mine.toml"), "fg = { Indexed = 1 }\n").unwrap();
        assert!(super::resolvable("nord", tmp.path()));
        assert!(super::resolvable("mine", tmp.path()));
        assert!(!super::resolvable("nope", tmp.path()));
    }

    #[test]
    fn valid_name_rejects_path_traversal() {
        assert!(super::valid_theme_name("my-pastel"));
        assert!(!super::valid_theme_name(""));
        assert!(!super::valid_theme_name("a/b"));
        assert!(!super::valid_theme_name(".."));
        assert!(!super::valid_theme_name("a\\b"));
    }

    #[test]
    fn scaffold_round_trips_to_seed_theme() {
        // The written file re-parses to a ThemeConfig whose apply_to(default)
        // reproduces the seed built-in theme.
        let toml = super::scaffold_toml(
            "my-nord",
            "nord",
            &rustline_core::builtin_theme("nord").unwrap(),
        );
        assert!(toml.starts_with("# rustline theme"), "has header: {toml}");
        let tc: rustline_core::ThemeConfig = toml::from_str(&toml).unwrap();
        let mut t = rustline_core::Theme::default();
        tc.apply_to(&mut t);
        assert_eq!(t, rustline_core::builtin_theme("nord").unwrap());
    }
}
