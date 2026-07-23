//! `rustline config …` — print the resolved config path, open it in
//! `$EDITOR`, or strictly validate it.
//!
//! `Config::load` (in `rustline-core`) is deliberately *total*: a missing or
//! malformed file falls back to defaults with a logged warning, so a bad
//! config never breaks the bar (invariant #3). `validate` does NOT go
//! through that path — it parses the raw contents with `toml::from_str`
//! directly so a malformed file is surfaced as an explicit, actionable error
//! instead of silently vanishing into a warning + defaults.

use std::io::IsTerminal;
use std::path::Path;
use std::process::Command;

use rustline_core::Config;

use crate::cli::ConfigCmd;
use crate::init::STARTER_TEMPLATE;

/// Dispatch a `rustline config …` invocation against the config at
/// `config_path`.
pub fn run(cmd: ConfigCmd, config_path: &Path) {
    match cmd {
        ConfigCmd::Path => println!("{}", config_path.display()),
        ConfigCmd::Edit => edit(config_path),
        ConfigCmd::Validate => validate(config_path),
    }
}

/// The action `config edit` takes after ensuring the config file exists. Kept
/// separate from the actual spawn so the decision ("should we open an
/// editor, and with what?") is a pure function, unit-testable without a real
/// TTY or `$EDITOR` — mirrors `theme_cmd::editor_command`.
#[derive(Debug, PartialEq, Eq)]
enum EditAction {
    /// Spawn this editor command (e.g. `$EDITOR`'s value) on the config file.
    Spawn(String),
    /// `$EDITOR` isn't set, or stdin isn't a TTY: print the path and a hint
    /// instead of guessing at an interactive spawn that might hang.
    Hint,
}

/// Decide whether/how to open an editor for `config edit`, from `$EDITOR` and
/// whether stdin is a TTY. Only a set `$EDITOR` on a TTY spawns; handing an
/// interactive editor a non-interactive stdin (e.g. piped output) would hang
/// or misbehave.
fn edit_action(editor_env: Option<&str>, is_tty: bool) -> EditAction {
    match editor_env {
        Some(editor) if is_tty => EditAction::Spawn(editor.to_string()),
        _ => EditAction::Hint,
    }
}

/// `rustline config edit`: create the config file from the starter template
/// if it doesn't exist yet, then open it in `$EDITOR` (see [`edit_action`]).
/// Never panics on a spawn failure — worst case, the user gets the path and
/// has to open it themselves.
fn edit(config_path: &Path) {
    if !config_path.exists() {
        if let Some(parent) = config_path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            eprintln!("failed to create {}: {e}", parent.display());
            std::process::exit(1);
        }
        if let Err(e) = std::fs::write(config_path, STARTER_TEMPLATE) {
            eprintln!("failed to create {}: {e}", config_path.display());
            std::process::exit(1);
        }
        println!("created {}", config_path.display());
    }

    let editor = std::env::var("EDITOR").ok();
    let is_tty = std::io::stdin().is_terminal();
    match edit_action(editor.as_deref(), is_tty) {
        EditAction::Spawn(editor) => match Command::new(&editor).arg(config_path).status() {
            Ok(status) if !status.success() => eprintln!("{editor} exited with {status}"),
            Ok(_) => {}
            Err(e) => eprintln!("failed to launch editor {editor:?}: {e}"),
        },
        EditAction::Hint => {
            println!("{}", config_path.display());
            println!(
                "set $EDITOR and run `rustline config edit` from a terminal to open it automatically"
            );
        }
    }
}

/// Strictly parse `contents` as a [`Config`] — unlike the total
/// [`Config::load`], which swallows a parse error into defaults + a logged
/// warning, this surfaces `toml`'s own line/column-annotated message. `Ok`
/// discards the parsed value; callers only care whether it parsed.
fn validate_config_str(contents: &str) -> Result<(), String> {
    toml::from_str::<Config>(contents)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// `rustline config validate`: strictly parse the config file at
/// `config_path` and report the outcome. A missing file is not an error
/// (`Config::load` treats absence as "use defaults", and validate agrees);
/// a present-but-malformed file exits non-zero with the parse error.
fn validate(config_path: &Path) {
    let contents = match std::fs::read_to_string(config_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!(
                "no config file at {} (using defaults)",
                config_path.display()
            );
            return;
        }
        Err(e) => {
            eprintln!("cannot read {}: {e}", config_path.display());
            std::process::exit(1);
        }
    };
    match validate_config_str(&contents) {
        Ok(()) => println!("ok: {}", config_path.display()),
        Err(msg) => {
            eprintln!("invalid config at {}: {msg}", config_path.display());
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_action_spawns_only_with_editor_and_tty() {
        assert_eq!(
            edit_action(Some("vim"), true),
            EditAction::Spawn("vim".to_string())
        );
    }

    #[test]
    fn edit_action_hints_without_editor_or_tty() {
        assert_eq!(edit_action(None, true), EditAction::Hint);
        assert_eq!(edit_action(Some("vim"), false), EditAction::Hint);
        assert_eq!(edit_action(None, false), EditAction::Hint);
    }

    #[test]
    fn validate_config_str_accepts_good_toml() {
        let good = "[layout]\nright = [\"datetime\"]\n";
        assert!(validate_config_str(good).is_ok());
    }

    #[test]
    fn validate_config_str_accepts_empty() {
        // Total-config semantics: an empty file is a perfectly valid Config.
        assert!(validate_config_str("").is_ok());
    }

    #[test]
    fn validate_config_str_rejects_unterminated_array() {
        let err = validate_config_str("layout = [").unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn validate_config_str_rejects_wrong_typed_field() {
        // bar_width must be an integer; a string makes the table invalid.
        let err = validate_config_str("[widgets.cpu]\nbar_width = \"wide\"\n").unwrap_err();
        assert!(!err.is_empty());
    }
}
