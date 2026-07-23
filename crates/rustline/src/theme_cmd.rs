//! `rustline theme …` — list/preview/select/scaffold themes. Config mutations
//! (`use`) go through `toml_edit` so comments/formatting survive, mirroring
//! `plugin_cmd`.

use std::io::{BufRead, IsTerminal, Write};
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
        ThemeCmd::Pick => pick(config_path, themes_dir),
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
pub(crate) fn set_base(doc: &mut DocumentMut, name: &str) {
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
pub(crate) fn theme_files(themes_dir: &Path) -> Vec<String> {
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

/// A representative synthetic Context for previews. When `show_alerts` is set,
/// cpu/memory/battery are pegged past their thresholds so the preview trips the
/// warning+error badges and exercises the theme's semantic colors; otherwise
/// they carry healthy readings, so the preview shows only the palette — what a
/// normal bar actually looks like. `colors` come from `theme`.
fn sample_context(theme: &Theme, show_alerts: bool) -> Context {
    let gib = 1024u64.pow(3);
    let (cpu_pct, mem_used_gib, mem_avail_gib, batt_pct) = if show_alerts {
        (96.0, 14, 2, 15)
    } else {
        (12.0, 6, 10, 82)
    };
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
            percent: batt_pct,
            state: BatteryState::Discharging,
        }),
        cpu: Some(CpuUsage { percent: cpu_pct }),
        memory: Some(MemInfo {
            total_bytes: 16 * gib,
            used_bytes: mem_used_gib * gib,
            available_bytes: mem_avail_gib * gib,
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
/// window pill. `show_alerts` picks the pegged-vs-healthy synthetic readings
/// (see [`sample_context`]).
fn preview_theme_ansi(theme: &Theme, show_alerts: bool) -> String {
    let cfg = Config::default();
    let reg = Registry::with_builtins(&cfg);
    let mut ctx = sample_context(theme, show_alerts);
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
/// `None` if unknown. `show_alerts` selects the pegged-vs-healthy synthetic
/// readings (see [`sample_context`]).
fn preview_ansi(name: &str, show_alerts: bool) -> Option<String> {
    Some(preview_theme_ansi(&builtin_theme(name)?, show_alerts))
}

/// Resolve and ANSI-render a preview for theme `name` (themes-dir file first,
/// then built-in). `None` if unknown or the file is invalid. `show_alerts`
/// selects the pegged-vs-healthy synthetic readings (see [`sample_context`]).
pub(crate) fn preview_named(name: &str, themes_dir: &Path, show_alerts: bool) -> Option<String> {
    let file = themes_dir.join(format!("{name}.toml"));
    if let Ok(text) = std::fs::read_to_string(&file) {
        if let Ok(tc) = toml::from_str::<ThemeConfig>(&text) {
            let mut t = Theme::default();
            tc.apply_to(&mut t);
            return Some(preview_theme_ansi(&t, show_alerts));
        }
        return None;
    }
    preview_ansi(name, show_alerts)
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
                println!("{}", preview_theme_ansi(&t, true));
                return;
            }
            Err(e) => {
                eprintln!("invalid theme file {}: {e}", file.display());
                std::process::exit(1);
            }
        }
    }
    match preview_ansi(name, true) {
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

/// One selectable theme in the picker: its name, whether it's the active base,
/// and whether it comes from a themes-dir file (which shadows a same-named
/// built-in, since resolution is file-first).
#[derive(Debug, PartialEq, Eq)]
struct PickEntry {
    name: String,
    active: bool,
    from_file: bool,
}

/// Ordered, name-unique picker entries: built-ins (in `builtins` order) first,
/// then themes-dir stems not already among the built-ins. `active` marks the
/// single entry whose name equals `active`; `from_file` marks any entry whose
/// name appears in `files` (a file shadowing a built-in name is one entry with
/// `from_file = true`).
fn picker_entries(active: &str, builtins: &[&str], files: &[String]) -> Vec<PickEntry> {
    let mut entries = Vec::new();
    for name in builtins {
        entries.push(PickEntry {
            name: (*name).to_string(),
            active: *name == active,
            from_file: files.iter().any(|f| f == name),
        });
    }
    for f in files {
        if !builtins.iter().any(|b| b == f) {
            entries.push(PickEntry {
                name: f.clone(),
                active: f == active,
                from_file: true,
            });
        }
    }
    entries
}

/// A parsed preview-loop command.
#[derive(Debug, PartialEq, Eq)]
enum PreviewCmd {
    Done,
    All,
    Preview(usize),
    ToggleAlerts,
    Invalid,
}

/// Parse a preview-loop answer: blank → `Done`; `a`/`all` (case-insensitive) →
/// `All`; `t`/`toggle` (case-insensitive) → `ToggleAlerts`; a number in
/// `1..=n` → `Preview(k-1)`; anything else → `Invalid`.
fn parse_preview_input(input: &str, n: usize) -> PreviewCmd {
    let t = input.trim();
    if t.is_empty() {
        return PreviewCmd::Done;
    }
    let lower = t.to_ascii_lowercase();
    if lower == "a" || lower == "all" {
        return PreviewCmd::All;
    }
    if lower == "t" || lower == "toggle" {
        return PreviewCmd::ToggleAlerts;
    }
    match t.parse::<usize>() {
        Ok(k) if (1..=n).contains(&k) => PreviewCmd::Preview(k - 1),
        _ => PreviewCmd::Invalid,
    }
}

/// A parsed set-step command.
#[derive(Debug, PartialEq, Eq)]
enum SetCmd {
    Keep,
    Index(usize),
    Name(String),
}

/// Parse a set-step answer: blank → `Keep`; a number in `1..=n` → `Index(k-1)`;
/// any other non-blank → `Name(trimmed)` (an out-of-range number lands here and
/// is rejected by the caller's entry lookup).
fn parse_set_input(input: &str, n: usize) -> SetCmd {
    let t = input.trim();
    if t.is_empty() {
        return SetCmd::Keep;
    }
    if let Ok(k) = t.parse::<usize>()
        && (1..=n).contains(&k)
    {
        return SetCmd::Index(k - 1);
    }
    SetCmd::Name(t.to_string())
}

/// Read one line from `reader` (empty string on EOF).
fn read_line<R: BufRead>(reader: &mut R) -> String {
    let mut s = String::new();
    let _ = reader.read_line(&mut s);
    s
}

/// The last preview the user asked for, so a `t` toggle can re-render it with
/// the new alert setting instead of leaving them to re-type the number.
#[derive(Clone, Copy)]
enum LastPreview {
    One(usize),
    All,
}

/// Render one theme's labelled preview to `writer`.
fn render_one<W: Write>(
    writer: &mut W,
    entries: &[PickEntry],
    idx: usize,
    themes_dir: &Path,
    show_alerts: bool,
) {
    let name = &entries[idx].name;
    let _ = writeln!(writer, "\n== {name} ==");
    if let Some(p) = preview_named(name, themes_dir, show_alerts) {
        let _ = writeln!(writer, "{p}");
    }
}

/// Render every theme's labelled preview to `writer`.
fn render_all<W: Write>(
    writer: &mut W,
    entries: &[PickEntry],
    themes_dir: &Path,
    show_alerts: bool,
) {
    for (i, _) in entries.iter().enumerate() {
        render_one(writer, entries, i, themes_dir, show_alerts);
    }
}

/// Re-render `last` (if any) with the current alert setting.
fn replay_preview<W: Write>(
    writer: &mut W,
    entries: &[PickEntry],
    themes_dir: &Path,
    show_alerts: bool,
    last: Option<LastPreview>,
) {
    match last {
        Some(LastPreview::One(idx)) => render_one(writer, entries, idx, themes_dir, show_alerts),
        Some(LastPreview::All) => render_all(writer, entries, themes_dir, show_alerts),
        None => {}
    }
}

/// Drive the interactive preview+set loop over a generic reader/writer,
/// returning the chosen theme name to set, or `None` to keep the current one.
/// Performs NO config write and never exits the process, so it is fully
/// unit-testable with a byte-slice reader.
fn run_picker<R: BufRead, W: Write>(
    entries: &[PickEntry],
    themes_dir: &Path,
    reader: &mut R,
    writer: &mut W,
    active: &str,
) -> Option<String> {
    let _ = writeln!(writer, "Themes (* = active):");
    for (i, e) in entries.iter().enumerate() {
        let mark = if e.active { " *" } else { "" };
        let tag = if e.from_file { "  (custom)" } else { "" };
        let _ = writeln!(writer, "  {}) {}{mark}{tag}", i + 1, e.name);
    }
    // Previews default to a healthy bar (palette only, no alert badges) so they
    // match what a normal status line looks like; `t` toggles the warning/error
    // alert colors on to sample the theme's semantic colors.
    let mut show_alerts = false;
    let mut last_preview: Option<LastPreview> = None;
    loop {
        let _ = write!(
            writer,
            "Preview # (number, a=all, t=toggle alerts, enter=done): "
        );
        let _ = writer.flush();
        match parse_preview_input(&read_line(reader), entries.len()) {
            PreviewCmd::Done => break,
            PreviewCmd::All => {
                render_all(writer, entries, themes_dir, show_alerts);
                last_preview = Some(LastPreview::All);
            }
            PreviewCmd::Preview(idx) => {
                render_one(writer, entries, idx, themes_dir, show_alerts);
                last_preview = Some(LastPreview::One(idx));
            }
            PreviewCmd::ToggleAlerts => {
                show_alerts = !show_alerts;
                let state = if show_alerts { "on" } else { "off" };
                let _ = writeln!(
                    writer,
                    "alert badges: {state} (previews now show {})",
                    if show_alerts {
                        "warning/error colors"
                    } else {
                        "the palette only"
                    }
                );
                // Re-show the last preview so the toggle's effect is immediate.
                replay_preview(writer, entries, themes_dir, show_alerts, last_preview);
            }
            PreviewCmd::Invalid => {
                let _ = writeln!(
                    writer,
                    "enter a number 1-{}, 'a' for all, 't' to toggle alerts, or blank to finish",
                    entries.len()
                );
            }
        }
    }
    loop {
        let _ = write!(
            writer,
            "Set which theme? [name or #, enter=keep {active}]: "
        );
        let _ = writer.flush();
        match parse_set_input(&read_line(reader), entries.len()) {
            SetCmd::Keep => return None,
            SetCmd::Index(idx) => return Some(entries[idx].name.clone()),
            SetCmd::Name(s) => {
                if let Some(e) = entries.iter().find(|e| e.name.eq_ignore_ascii_case(&s)) {
                    return Some(e.name.clone());
                }
                let _ = writeln!(writer, "unknown theme: {s}");
            }
        }
    }
}

/// `rustline theme pick`: interactively browse theme previews and set one.
/// Requires a TTY; a non-interactive invocation prints a hint and exits.
fn pick(config_path: &Path, themes_dir: &Path) {
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "theme pick is interactive and needs a terminal.\n\
             Use `rustline theme show <name>` to preview or `rustline theme use <name>` to set non-interactively."
        );
        std::process::exit(2);
    }
    let cfg = Config::load(config_path);
    let active = cfg
        .theme
        .base
        .clone()
        .unwrap_or_else(|| "default".to_string());
    let entries = picker_entries(&active, builtin_theme_names(), &theme_files(themes_dir));
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let stderr = std::io::stderr();
    let mut writer = stderr.lock();
    match run_picker(&entries, themes_dir, &mut reader, &mut writer, &active) {
        Some(name) => use_theme(&name, config_path, themes_dir),
        None => println!("kept {active}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let out = super::preview_ansi("nord", true).expect("known theme");
        assert!(out.contains('\u{1b}'), "contains ANSI escape: {out:?}");
        assert!(out.contains("RIGHT"), "labels the right region");
        assert!(super::preview_ansi("nope", true).is_none());
    }

    #[test]
    fn preview_show_alerts_gates_the_warning_error_badge_colors() {
        // The default theme's error/warning semantics are colour196/colour214;
        // the ANSI transcoder emits an indexed background as `48;5;<n>`. With
        // alerts on, the pegged synthetic readings trip both badges; with alerts
        // off (the picker default), the healthy readings show only the palette,
        // so neither badge color appears.
        let with = super::preview_theme_ansi(&Theme::default(), true);
        assert!(
            with.contains("48;5;196"),
            "error badge bg present: {with:?}"
        );
        assert!(
            with.contains("48;5;214"),
            "warning badge bg present: {with:?}"
        );

        let without = super::preview_theme_ansi(&Theme::default(), false);
        assert!(
            !without.contains("48;5;196"),
            "no error badge when alerts off: {without:?}"
        );
        assert!(
            !without.contains("48;5;214"),
            "no warning badge when alerts off: {without:?}"
        );
    }

    #[test]
    fn parse_preview_input_recognizes_toggle() {
        assert_eq!(parse_preview_input("t", 5), PreviewCmd::ToggleAlerts);
        assert_eq!(parse_preview_input("  T ", 5), PreviewCmd::ToggleAlerts);
        assert_eq!(parse_preview_input("toggle", 5), PreviewCmd::ToggleAlerts);
        // Still distinct from the other commands.
        assert_eq!(parse_preview_input("a", 5), PreviewCmd::All);
        assert_eq!(parse_preview_input("2", 5), PreviewCmd::Preview(1));
        assert_eq!(parse_preview_input("", 5), PreviewCmd::Done);
    }

    #[test]
    fn run_picker_toggle_enables_badge_colors_in_previews() {
        // Preview theme #1 (default) with alerts off (default) → no badge color;
        // then toggle on and preview #1 again → badge color appears; then keep.
        let entries = vec![PickEntry {
            name: "default".to_string(),
            active: true,
            from_file: false,
        }];
        let dir = std::path::Path::new("/nonexistent-themes-dir");
        let input = b"1\nt\n1\n\n\n";
        let mut reader = &input[..];
        let mut out: Vec<u8> = Vec::new();
        let choice = run_picker(&entries, dir, &mut reader, &mut out, "default");
        assert_eq!(choice, None, "blank set-step keeps current theme");
        let text = String::from_utf8(out).unwrap();
        // The toggle status line is shown.
        assert!(
            text.contains("alert badges: on"),
            "toggle prints its new state: {text:?}"
        );
        // Split on the toggle status line: the badge color must be absent before
        // it and present after it.
        let (before, after) = text
            .split_once("alert badges: on")
            .expect("toggle status line present");
        assert!(
            !before.contains("48;5;196"),
            "no error badge before toggle: {before:?}"
        );
        assert!(
            after.contains("48;5;196"),
            "error badge appears after toggle: {after:?}"
        );
    }

    #[test]
    fn run_picker_toggle_re_previews_last_item() {
        // Preview #1 (alerts off → no badge), then toggle — WITHOUT re-typing a
        // number. The toggle must immediately re-render the last previewed item
        // with alerts on, so its badge color shows right away.
        let entries = vec![PickEntry {
            name: "default".to_string(),
            active: true,
            from_file: false,
        }];
        let dir = std::path::Path::new("/nonexistent-themes-dir");
        // preview 1, toggle, done-preview, keep.
        let input = b"1\nt\n\n\n";
        let mut reader = &input[..];
        let mut out: Vec<u8> = Vec::new();
        let choice = run_picker(&entries, dir, &mut reader, &mut out, "default");
        assert_eq!(choice, None);
        let text = String::from_utf8(out).unwrap();
        let (_before, after) = text
            .split_once("alert badges: on")
            .expect("toggle status line present");
        // A fresh preview block is rendered right after the toggle status line,
        // and it carries the error-badge color the healthy default lacked.
        assert!(
            after.contains("== default =="),
            "toggle re-previews the last item: {after:?}"
        );
        assert!(
            after.contains("48;5;196"),
            "re-preview shows the badge color: {after:?}"
        );
    }

    #[test]
    fn run_picker_toggle_without_prior_preview_shows_only_status() {
        // Toggling before previewing anything shows just the status line — there
        // is no "last item" to re-render, so no preview block appears.
        let entries = vec![PickEntry {
            name: "default".to_string(),
            active: true,
            from_file: false,
        }];
        let dir = std::path::Path::new("/nonexistent-themes-dir");
        let input = b"t\n\n\n"; // toggle, done-preview, keep.
        let mut reader = &input[..];
        let mut out: Vec<u8> = Vec::new();
        let choice = run_picker(&entries, dir, &mut reader, &mut out, "default");
        assert_eq!(choice, None);
        let text = String::from_utf8(out).unwrap();
        let (_before, after) = text
            .split_once("alert badges: on")
            .expect("toggle status line present");
        assert!(
            !after.contains("== default =="),
            "no auto-preview without a prior preview: {after:?}"
        );
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

    #[test]
    fn picker_entries_orders_dedups_and_marks() {
        let builtins = ["default", "nord", "gruvbox"];
        let files = vec!["nord".to_string(), "mine".to_string()]; // nord file shadows built-in
        let e = picker_entries("nord", &builtins, &files);
        assert_eq!(
            e.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
            vec!["default", "nord", "gruvbox", "mine"] // built-ins in order, then file-only
        );
        // exactly the nord entry is active, and it's from_file (file-first resolution)
        let nord = e.iter().find(|x| x.name == "nord").unwrap();
        assert!(nord.active && nord.from_file);
        assert_eq!(e.iter().filter(|x| x.active).count(), 1);
        assert!(e.iter().find(|x| x.name == "mine").unwrap().from_file);
        assert!(!e.iter().find(|x| x.name == "default").unwrap().from_file);
    }

    #[test]
    fn parse_preview_input_cases() {
        assert_eq!(parse_preview_input("", 6), PreviewCmd::Done);
        assert_eq!(parse_preview_input("  ", 6), PreviewCmd::Done);
        assert_eq!(parse_preview_input("a", 6), PreviewCmd::All);
        assert_eq!(parse_preview_input("ALL", 6), PreviewCmd::All);
        assert_eq!(parse_preview_input("1", 6), PreviewCmd::Preview(0));
        assert_eq!(parse_preview_input("6", 6), PreviewCmd::Preview(5));
        assert_eq!(parse_preview_input("0", 6), PreviewCmd::Invalid);
        assert_eq!(parse_preview_input("7", 6), PreviewCmd::Invalid);
        assert_eq!(parse_preview_input("x", 6), PreviewCmd::Invalid);
    }

    #[test]
    fn parse_set_input_cases() {
        assert_eq!(parse_set_input("", 6), SetCmd::Keep);
        assert_eq!(parse_set_input("3", 6), SetCmd::Index(2));
        assert_eq!(parse_set_input("nord", 6), SetCmd::Name("nord".to_string()));
        assert_eq!(parse_set_input("99", 6), SetCmd::Name("99".to_string())); // out-of-range → name
    }

    #[test]
    fn run_picker_previews_then_sets_by_index() {
        let td = tempfile::tempdir().unwrap();
        let entries = picker_entries("default", builtin_theme_names(), &[]);
        let input = b"2\n\n1\n"; // preview #2, blank ends preview loop, set #1
        let mut reader = std::io::Cursor::new(&input[..]);
        let mut out: Vec<u8> = Vec::new();
        let choice = run_picker(&entries, td.path(), &mut reader, &mut out, "default");
        assert_eq!(choice.as_deref(), Some(entries[0].name.as_str()));
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains(&format!("== {} ==", entries[1].name))
        );
    }

    #[test]
    fn run_picker_blank_keeps_current() {
        let td = tempfile::tempdir().unwrap();
        let entries = picker_entries("default", builtin_theme_names(), &[]);
        let input = b"\n\n"; // no preview, blank set → keep
        let mut reader = std::io::Cursor::new(&input[..]);
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            run_picker(&entries, td.path(), &mut reader, &mut out, "default"),
            None
        );
    }

    #[test]
    fn run_picker_all_then_set_by_name() {
        let td = tempfile::tempdir().unwrap();
        let entries = picker_entries("default", builtin_theme_names(), &[]);
        let input = b"a\n\nnord\n"; // preview all, then set by name
        let mut reader = std::io::Cursor::new(&input[..]);
        let mut out: Vec<u8> = Vec::new();
        let choice = run_picker(&entries, td.path(), &mut reader, &mut out, "default");
        assert_eq!(choice.as_deref(), Some("nord"));
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("== nord ==") && s.contains("== gruvbox =="));
    }

    #[test]
    fn run_picker_reasks_on_unknown_name() {
        let td = tempfile::tempdir().unwrap();
        let entries = picker_entries("default", builtin_theme_names(), &[]);
        let input = b"\nnope\nnord\n"; // blank preview-done; set "nope" (reask) then "nord"
        let mut reader = std::io::Cursor::new(&input[..]);
        let mut out: Vec<u8> = Vec::new();
        let choice = run_picker(&entries, td.path(), &mut reader, &mut out, "default");
        assert_eq!(choice.as_deref(), Some("nord"));
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("unknown theme: nope")
        );
    }

    #[test]
    fn run_picker_set_name_is_case_insensitive() {
        let td = tempfile::tempdir().unwrap();
        let entries = picker_entries("default", builtin_theme_names(), &[]);
        let input = b"\nNORD\n"; // blank ends preview loop; set "NORD"
        let mut reader = std::io::Cursor::new(&input[..]);
        let mut out: Vec<u8> = Vec::new();
        let choice = run_picker(&entries, td.path(), &mut reader, &mut out, "default");
        assert_eq!(choice.as_deref(), Some("nord"));
    }
}
