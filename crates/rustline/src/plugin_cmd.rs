//! `rustline plugin …` — list plugins and edit their capability allowlists.
//! Mutations use `toml_edit` so the user's comments and formatting survive.

use std::path::Path;

use rustline_core::Config;
use toml_edit::{Array, DocumentMut, Item, Table, Value};

use crate::cli::{PatternCmd, PluginCmd};

/// Which allowlist array a pattern command targets.
enum Kind {
    Url,
    Path,
}

impl Kind {
    fn key(&self) -> &'static str {
        match self {
            Kind::Url => "allowed_urls",
            Kind::Path => "allowed_paths",
        }
    }
}

/// Dispatch a `rustline plugin …` invocation against the config at
/// `config_path`.
pub fn run(cmd: PluginCmd, config_path: &Path) {
    match cmd {
        PluginCmd::List => list(config_path),
        PluginCmd::Url(pc) => pattern_cmd(pc, Kind::Url, config_path),
        PluginCmd::Path(pc) => pattern_cmd(pc, Kind::Path, config_path),
    }
}

/// Print every configured plugin's source and allowlists/caps.
fn list(config_path: &Path) {
    let cfg = Config::load(config_path);
    if cfg.plugins.is_empty() {
        println!("no plugins configured");
        return;
    }
    for (name, pc) in &cfg.plugins {
        println!("{name}");
        if let Some(src) = &pc.source {
            println!("  source: {src}");
        }
        println!("  allowed_urls: {:?}", pc.allowed_urls);
        println!("  allowed_paths: {:?}", pc.allowed_paths);
        println!("  max_state_bytes: {}", pc.max_state_bytes);
    }
}

/// Run a `list`/`add`/`remove` operation over one allowlist (`kind`) of one
/// named plugin.
fn pattern_cmd(cmd: PatternCmd, kind: Kind, config_path: &Path) {
    match cmd {
        PatternCmd::List { plugin } => {
            let cfg = Config::load(config_path);
            let patterns = cfg.plugins.get(&plugin).map(|p| match kind {
                Kind::Url => &p.allowed_urls,
                Kind::Path => &p.allowed_paths,
            });
            match patterns {
                Some(list) if !list.is_empty() => list.iter().for_each(|p| println!("{p}")),
                Some(_) => println!("(none)"),
                None => println!("no such plugin: {plugin}"),
            }
        }
        PatternCmd::Add { plugin, pattern } => mutate(config_path, &plugin, kind, |arr| {
            if !arr.iter().any(|v| v.as_str() == Some(&pattern)) {
                arr.push(pattern.as_str());
            }
        }),
        PatternCmd::Remove { plugin, pattern } => mutate(config_path, &plugin, kind, |arr| {
            arr.retain(|v| v.as_str() != Some(&pattern));
        }),
    }
}

/// Load `config_path` as a format-preserving `toml_edit` document, ensure
/// `[plugins.<plugin>]` and its `kind` allowlist array exist, apply `f` to
/// that array, and write the document back. Comments and formatting
/// elsewhere in the file are untouched.
fn mutate(config_path: &Path, plugin: &str, kind: Kind, f: impl FnOnce(&mut Array)) {
    // Distinguish absent / unreadable / invalid so we never blow away an
    // existing config we merely failed to load: only a genuinely missing file
    // is a legitimate fresh-start; a read error or a TOML syntax error must
    // abort *before* the write below, or a `plugin add` would silently
    // truncate the user's whole config down to `[plugins.<x>]`.
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

    // Ensure [plugins.<plugin>] exists.
    let plugins = match doc
        .entry("plugins")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
    {
        Some(t) => t,
        None => {
            eprintln!("config error: `plugins` is not a table");
            std::process::exit(1);
        }
    };
    plugins.set_implicit(true);
    let entry = match plugins
        .entry(plugin)
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
    {
        Some(t) => t,
        None => {
            eprintln!("config error: `plugins.{plugin}` is not a table");
            std::process::exit(1);
        }
    };

    // Ensure the allowlist array exists.
    let key = kind.key();
    let item = entry
        .entry(key)
        .or_insert(Item::Value(Value::Array(Array::new())));
    let arr = match item.as_array_mut() {
        Some(a) => a,
        None => {
            eprintln!("config error: `{plugin}.{key}` is not an array");
            std::process::exit(1);
        }
    };
    f(arr);

    if let Err(error) = std::fs::write(config_path, doc.to_string()) {
        eprintln!("failed to write config: {error}");
        std::process::exit(1);
    }
}
