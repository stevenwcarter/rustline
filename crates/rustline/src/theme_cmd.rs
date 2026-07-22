//! `rustline theme …` — list/preview/select/scaffold themes. Config mutations
//! (`use`) go through `toml_edit` so comments/formatting survive, mirroring
//! `plugin_cmd`.

use std::path::Path;

use rustline_core::{Config, builtin_theme_names};

use crate::cli::ThemeCmd;

/// Dispatch a `rustline theme …` invocation.
pub fn run(cmd: ThemeCmd, config_path: &Path, themes_dir: &Path) {
    match cmd {
        ThemeCmd::List => list(config_path, themes_dir),
        ThemeCmd::Show { name } => {
            let _ = (&name, themes_dir); // Task 12
        }
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
fn list_lines(active: &str, files: &[String]) -> Vec<String> {
    let mut lines = Vec::new();
    for name in builtin_theme_names() {
        let mark = if *name == active { " *" } else { "" };
        let shadowed = if files.iter().any(|f| f == name) {
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
}
