//! `rustline plugin …` — list plugins, edit their capability allowlists, and
//! approve a plugin's declared capability manifest. Mutations use `toml_edit`
//! so the user's comments and formatting survive.

use std::io::Write as _;
use std::path::Path;

use rustline_core::Config;
use rustline_wasm::{PluginManifest, resolve_manifest};
use toml_edit::{Array, DocumentMut, Item, Table, Value};

use crate::cli::{ApproveArgs, PatternCmd, PluginCmd};

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
/// `config_path`, discovering manifests under `plugin_dir`.
pub fn run(cmd: PluginCmd, config_path: &Path, plugin_dir: &Path) {
    match cmd {
        PluginCmd::List => list(config_path, plugin_dir),
        PluginCmd::Url(pc) => pattern_cmd(pc, Kind::Url, config_path),
        PluginCmd::Path(pc) => pattern_cmd(pc, Kind::Path, config_path),
        PluginCmd::Approve(args) => approve(args, config_path, plugin_dir),
    }
}

/// Print every configured plugin's source and allowlists/caps, noting any
/// declared capability manifest.
fn list(config_path: &Path, plugin_dir: &Path) {
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
        if let Some(m) = resolve_manifest(plugin_dir, name) {
            println!(
                "  declared capabilities: {} urls, {} paths (run `plugin approve {name}`)",
                m.requested_urls.len(),
                m.requested_paths.len()
            );
        }
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
            append_unique(arr, std::slice::from_ref(&pattern));
        }),
        PatternCmd::Remove { plugin, pattern } => mutate(config_path, &plugin, kind, |arr| {
            arr.retain(|v| v.as_str() != Some(&pattern));
        }),
    }
}

/// `rustline plugin approve <name> [--yes]`: resolve the plugin's manifest,
/// show its requested capabilities, and — on consent — write **exactly** those
/// requested URL/path patterns into the plugin's allowlists, verbatim and
/// idempotently. Approval only ever writes an allowlist entry (deny-by-default
/// still holds until then) and never widens beyond what the manifest declares
/// (N4), so it cannot over-grant.
fn approve(args: ApproveArgs, config_path: &Path, plugin_dir: &Path) {
    let Some(manifest) = resolve_manifest(plugin_dir, &args.plugin) else {
        println!("no manifest found for {}", args.plugin);
        return;
    };
    print_manifest(&manifest);
    if manifest.requested_urls.is_empty() && manifest.requested_paths.is_empty() {
        println!("manifest requests no capabilities; nothing to approve");
        return;
    }
    if !args.yes && !confirm() {
        println!("declined; no changes written");
        return;
    }
    write_grants(config_path, &args.plugin, &manifest);
    println!("approved capabilities for {}", args.plugin);
}

/// Show a manifest's identity and the exact capabilities it requests.
fn print_manifest(m: &PluginManifest) {
    let name = if m.name.is_empty() { "?" } else { &m.name };
    let version = if m.version.is_empty() {
        "?"
    } else {
        &m.version
    };
    println!("plugin {name} (version {version}) requests:");
    print_requests("allowed_urls", &m.requested_urls);
    print_requests("allowed_paths", &m.requested_paths);
}

/// Print one requested-capability list under `label`, or `(none)`.
fn print_requests(label: &str, entries: &[String]) {
    println!("  {label}:");
    if entries.is_empty() {
        println!("    (none)");
    } else {
        entries.iter().for_each(|e| println!("    {e}"));
    }
}

/// Interactive y/N confirmation. Defaults to No on EOF, a read error, or any
/// non-`y` reply, so a non-interactive invocation without `--yes` declines
/// rather than approving.
fn confirm() -> bool {
    print!("Approve these capabilities? [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Append each manifest-requested pattern (idempotently) into the plugin's
/// allowlists in one load/write, preserving comments/formatting and any
/// pre-existing user entries. Only the requested strings are written, verbatim
/// — never a widened or expanded grant.
fn write_grants(config_path: &Path, plugin: &str, m: &PluginManifest) {
    let mut doc = load_doc(config_path);
    let table = plugin_table(&mut doc, plugin);
    if !m.requested_urls.is_empty() {
        append_unique(
            allowlist_array(table, plugin, Kind::Url.key()),
            &m.requested_urls,
        );
    }
    if !m.requested_paths.is_empty() {
        append_unique(
            allowlist_array(table, plugin, Kind::Path.key()),
            &m.requested_paths,
        );
    }
    write_doc(config_path, &doc);
}

/// Append entries not already present, matched by exact string.
fn append_unique(arr: &mut Array, entries: &[String]) {
    for entry in entries {
        if !arr.iter().any(|v| v.as_str() == Some(entry.as_str())) {
            arr.push(entry.as_str());
        }
    }
}

/// Load `config_path`, ensure `[plugins.<plugin>]` and its `kind` allowlist
/// array exist, apply `f` to that array, and write the document back. Comments
/// and formatting elsewhere are untouched.
fn mutate(config_path: &Path, plugin: &str, kind: Kind, f: impl FnOnce(&mut Array)) {
    let mut doc = load_doc(config_path);
    let table = plugin_table(&mut doc, plugin);
    f(allowlist_array(table, plugin, kind.key()));
    write_doc(config_path, &doc);
}

/// Load `config_path` as a format-preserving `toml_edit` document.
///
/// Absent / unreadable / invalid are distinguished so a mutation never blows
/// away an existing config we merely failed to load: only a genuinely missing
/// file is a legitimate fresh-start; a read error or a TOML syntax error aborts
/// *before* any write, or a `plugin add`/`approve` would silently truncate the
/// user's whole config down to `[plugins.<x>]`.
fn load_doc(config_path: &Path) -> DocumentMut {
    match std::fs::read_to_string(config_path) {
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
    }
}

/// Ensure `[plugins.<plugin>]` exists and return it as a mutable table,
/// exiting cleanly if either `plugins` or `plugins.<plugin>` is a non-table.
fn plugin_table<'a>(doc: &'a mut DocumentMut, plugin: &str) -> &'a mut Table {
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
    match plugins
        .entry(plugin)
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
    {
        Some(t) => t,
        None => {
            eprintln!("config error: `plugins.{plugin}` is not a table");
            std::process::exit(1);
        }
    }
}

/// Ensure `table[key]` is an array and return it, exiting cleanly otherwise.
fn allowlist_array<'a>(table: &'a mut Table, plugin: &str, key: &str) -> &'a mut Array {
    let item = table
        .entry(key)
        .or_insert(Item::Value(Value::Array(Array::new())));
    match item.as_array_mut() {
        Some(a) => a,
        None => {
            eprintln!("config error: `{plugin}.{key}` is not an array");
            std::process::exit(1);
        }
    }
}

/// Write the document back to `config_path`, exiting cleanly on an I/O error.
fn write_doc(config_path: &Path, doc: &DocumentMut) {
    if let Err(error) = std::fs::write(config_path, doc.to_string()) {
        eprintln!("failed to write config: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(urls: &[&str], paths: &[&str]) -> PluginManifest {
        PluginManifest {
            name: "w".into(),
            version: "1".into(),
            requested_urls: urls.iter().map(|s| s.to_string()).collect(),
            requested_paths: paths.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Read a plugin allowlist array back out of a rendered config, non-panicky
    /// (missing → empty), so tests can assert exact contents.
    fn list_of(text: &str, plugin: &str, key: &str) -> Vec<String> {
        let doc: DocumentMut = text.parse().unwrap();
        doc.get("plugins")
            .and_then(Item::as_table)
            .and_then(|t| t.get(plugin))
            .and_then(Item::as_table)
            .and_then(|t| t.get(key))
            .and_then(Item::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn write_grants_writes_exactly_requested_into_fresh_config() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "# keep\n[plugins.w]\nallowed_urls = []\n").unwrap();

        write_grants(&cfg, "w", &manifest(&["https://a/*"], &["/tmp/x"]));

        let text = std::fs::read_to_string(&cfg).unwrap();
        assert!(text.contains("# keep"), "comment preserved: {text}");
        assert_eq!(list_of(&text, "w", "allowed_urls"), ["https://a/*"]);
        assert_eq!(list_of(&text, "w", "allowed_paths"), ["/tmp/x"]);
    }

    #[test]
    fn write_grants_preserves_existing_and_never_over_grants() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(
            &cfg,
            "[plugins.w]\nallowed_urls = [\"https://existing/*\"]\n",
        )
        .unwrap();

        // Manifest requests one url and no paths.
        write_grants(&cfg, "w", &manifest(&["https://a/*"], &[]));

        let text = std::fs::read_to_string(&cfg).unwrap();
        // Pre-existing user grant kept, the requested one appended — nothing else.
        assert_eq!(
            list_of(&text, "w", "allowed_urls"),
            ["https://existing/*", "https://a/*"]
        );
        // No paths requested → no allowed_paths written at all (no over-grant).
        assert!(list_of(&text, "w", "allowed_paths").is_empty());
        assert!(
            !text.contains("allowed_paths"),
            "no empty path array added: {text}"
        );
    }

    #[test]
    fn write_grants_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "[plugins.w]\n").unwrap();
        let m = manifest(&["https://a/*"], &["/tmp/x"]);

        write_grants(&cfg, "w", &m);
        write_grants(&cfg, "w", &m);

        let text = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(list_of(&text, "w", "allowed_urls"), ["https://a/*"]);
        assert_eq!(list_of(&text, "w", "allowed_paths"), ["/tmp/x"]);
    }
}
