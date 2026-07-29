//! `rustline plugin …` — list plugins, edit their capability allowlists, and
//! approve a plugin's declared capability manifest. Mutations use `toml_edit`
//! so the user's comments and formatting survive.

use std::fmt::Write as _;
use std::io::{IsTerminal, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use anyhow::{Context, bail};
use rustline_core::{Config, Context as CoreContext, NameError, RangeName, Segment, Widget};
use rustline_wasm::{DenialKind, DenialObserver, PluginManifest, resolve_manifest};
use toml_edit::{Array, DocumentMut, Item, Table, Value};

use crate::cli::{ApproveArgs, BuildArgs, NewPluginArgs, PatternCmd, PluginCmd, RunArgs};

/// The reserved widget name that a plugin must never claim (it names the
/// built-in window-list renderer, which isn't a plugin-resolvable slot).
pub(crate) const RESERVED_PLUGIN_NAME: &str = "window";

/// `tmux`'s `range=user|X` status-range argument is byte-capped; a plugin
/// name longer than this can never be click-toggleable (invariant #7).
pub(crate) const MAX_PLUGIN_NAME_BYTES: usize = 15;

/// The embedded `Cargo.toml`/`src/lib.rs` templates `plugin new` scaffolds,
/// mirroring how `init.rs` embeds its starter config template.
const PLUGIN_CARGO_TEMPLATE: &str = include_str!("../assets/plugin-cargo.toml.tmpl");
const PLUGIN_LIB_TEMPLATE: &str = include_str!("../assets/plugin-lib.rs.tmpl");

/// Which allowlist array a pattern command targets.
enum Kind {
    Url,
    Path,
    WritePath,
    Command,
}

impl Kind {
    fn key(&self) -> &'static str {
        match self {
            Kind::Url => "allowed_urls",
            Kind::Path => "allowed_paths",
            Kind::WritePath => "allowed_write_paths",
            Kind::Command => "allowed_commands",
        }
    }
}

/// Dispatch a `rustline plugin …` invocation against the config at
/// `config_path`, discovering manifests under `plugin_dir`.
pub fn run(cmd: PluginCmd, config_path: &Path, plugin_dir: &Path) {
    match cmd {
        PluginCmd::List { json } => list(config_path, plugin_dir, json),
        PluginCmd::Search {
            query,
            json,
            refresh,
        } => search(config_path, plugin_dir, query.as_deref(), json, refresh),
        PluginCmd::Url(pc) => pattern_cmd(pc, Kind::Url, config_path),
        PluginCmd::Path(pc) => pattern_cmd(pc, Kind::Path, config_path),
        PluginCmd::WritePath(pc) => pattern_cmd(pc, Kind::WritePath, config_path),
        PluginCmd::Cmd(pc) => pattern_cmd(pc, Kind::Command, config_path),
        PluginCmd::Approve(args) => approve(args, config_path, plugin_dir),
        PluginCmd::Install(args) => crate::plugin_install::install(&args, config_path, plugin_dir),
        PluginCmd::Update(args) => crate::plugin_install::update(&args, config_path, plugin_dir),
        PluginCmd::Remove(args) => crate::plugin_install::remove(&args, config_path, plugin_dir),
        PluginCmd::New(args) => new_plugin(&args),
        PluginCmd::Build(args) => {
            if let Err(error) = build_plugin(&args, plugin_dir, config_path) {
                eprintln!("plugin build failed: {error:#}");
                std::process::exit(1);
            }
        }
        PluginCmd::Run(args) => run_plugin(&args, config_path, plugin_dir),
        PluginCmd::Denials { name, json } => denials_cmd(&name, json),
    }
}

/// `rustline plugin denials <name>`: list every capability denial the
/// production `FileDenialObserver` has recorded for `name` (one line per
/// distinct `(kind, target)` — see `rustline_wasm::denials`). Read-only.
fn denials_cmd(name: &str, json: bool) {
    let denials = rustline_wasm::read_denials(name);
    if json {
        println!("{}", denials_json(&denials));
        return;
    }
    if denials.is_empty() {
        println!("no recorded denials for {name}");
        return;
    }
    for d in denials {
        println!("{} denied: {}", denial_kind_label(d.kind), d.target);
    }
}

/// One denied capability request captured during `plugin run`'s render, for
/// its report. Harness-local: not part of any persisted denial log (a
/// separate, later concern — see the module doc on `capability::DenialObserver`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Denial {
    kind: DenialKind,
    target: String,
}

/// A [`DenialObserver`] that records every denial into a shared `Vec`, so
/// `plugin run` can report them once the plugin's single render call
/// returns. `observe` takes `&self` (the trait's signature), so the list
/// lives behind a `Mutex`; `snapshot` clones the collected denials out
/// without requiring exclusive ownership — the observer stays shared with
/// the plugin's `CapabilityCtx` for as long as the widget is alive.
#[derive(Default)]
struct CollectingObserver {
    denials: Mutex<Vec<Denial>>,
}

impl DenialObserver for CollectingObserver {
    fn observe(&self, _plugin: &str, kind: DenialKind, target: &str) {
        if let Ok(mut denials) = self.denials.lock() {
            denials.push(Denial {
                kind,
                target: target.to_string(),
            });
        }
    }
}

impl CollectingObserver {
    fn snapshot(&self) -> Vec<Denial> {
        self.denials.lock().map(|d| d.clone()).unwrap_or_default()
    }
}

/// A short label for a [`DenialKind`], for the `plugin run` report.
fn denial_kind_label(kind: DenialKind) -> &'static str {
    match kind {
        DenialKind::Url => "url",
        DenialKind::Path => "path",
        DenialKind::Command => "command",
    }
}

#[derive(serde::Serialize)]
struct PluginEntryJson<'a> {
    name: &'a str,
    source: Option<String>,
    tag: Option<&'a str>,
    allowed_urls: &'a [String],
    allowed_paths: &'a [String],
    /// The plugin's **write** allowlist — separate from `allowed_paths`,
    /// which is read-only. Omitting this field would let a plugin holding an
    /// arbitrary-overwrite grant render as read-only in machine-readable
    /// output, which is exactly the understated-grant shape this task's
    /// human/write split exists to close.
    allowed_write_paths: &'a [String],
    /// Whether a symlink component in a requested path is resolved (and
    /// matched against its target) rather than denied outright. See
    /// `PluginConfig::resolve_symlinks`.
    resolve_symlinks: bool,
    allowed_commands: &'a [String],
    max_state_bytes: u64,
    has_manifest: bool,
    /// Whether the installed `.wasm` verifies against the recorded
    /// `checksum` — `"verified"`/`"unpinned"`/`"mismatch"`/`"malformed"`/
    /// `"missing"` (see `plugin_checksum::PluginChecksumStatus`). Additive
    /// field: every other field/shape here is unchanged.
    checksum_status: &'static str,
}

/// The `plugin list --json` payload — one entry per configured plugin, same
/// fields the human `list` prints, plus `has_manifest` (whether a capability
/// manifest resolves) and `checksum_status` (whether the installed `.wasm`
/// verifies against the recorded `checksum`, via `plugin_checksum::status_for`
/// — the same `rustline_wasm::verify_checksum` call `register_plugins` gates
/// loading on). An empty plugins map serializes to `[]`.
fn plugin_list_json(cfg: &Config, plugin_dir: &Path) -> String {
    let entries: Vec<PluginEntryJson> = cfg
        .plugins
        .iter()
        .map(|(name, pc)| PluginEntryJson {
            name,
            // Display keeps `source` a flat string for the `string|null` JSON schema; the
            // future-only Url/Path PluginSource variants would Display as "url: X"/"path: X"
            // — revisit if install-by-url/path ships.
            source: pc.source.as_ref().map(|s| s.to_string()),
            tag: pc.tag.as_deref(),
            allowed_urls: &pc.allowed_urls,
            allowed_paths: &pc.allowed_paths,
            allowed_write_paths: &pc.allowed_write_paths,
            resolve_symlinks: pc.resolve_symlinks,
            allowed_commands: &pc.allowed_commands,
            max_state_bytes: pc.max_state_bytes,
            has_manifest: resolve_manifest(plugin_dir, name).is_some(),
            checksum_status: crate::plugin_checksum::status_for(
                plugin_dir,
                name,
                pc.checksum.as_deref(),
            )
            .label(),
        })
        .collect();
    serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".to_string())
}

/// The `plugin url|path list --json` payload: a JSON array of the allowlist
/// strings. An absent plugin or empty list → `[]` (stdout stays valid JSON in a
/// CI audit; the human path's "no such plugin" distinction is intentionally
/// dropped in JSON mode — callers check `plugin list --json` for existence).
fn pattern_list_json(patterns: Option<&Vec<String>>) -> String {
    match patterns {
        Some(list) => serde_json::to_string_pretty(list).unwrap_or_else(|_| "[]".to_string()),
        None => "[]".to_string(),
    }
}

#[derive(serde::Serialize)]
struct DenialEntryJson {
    kind: &'static str,
    target: String,
}

/// The `plugin denials --json` payload: one entry per recorded denial, `kind`
/// as the same lowercase label the human path prints.
fn denials_json(denials: &[rustline_wasm::Denial]) -> String {
    let entries: Vec<DenialEntryJson> = denials
        .iter()
        .map(|d| DenialEntryJson {
            kind: denial_kind_label(d.kind),
            target: d.target.clone(),
        })
        .collect();
    serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".to_string())
}

/// Render a `plugin run` report: each rendered segment's text, then any
/// capability denials the plugin triggered during that render. Pure, so it's
/// unit-tested directly; `run_plugin` is its only caller.
fn format_run_output(segments: &[Segment], denials: &[Denial]) -> String {
    let mut out = String::new();
    out.push_str("segments:\n");
    if segments.is_empty() {
        out.push_str("  (none — plugin rendered nothing)\n");
    } else {
        for seg in segments {
            let _ = writeln!(out, "  {}", seg.text);
        }
    }
    out.push_str("denials:\n");
    if denials.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for d in denials {
            let _ = writeln!(out, "  {} denied: {}", denial_kind_label(d.kind), d.target);
        }
    }
    out
}

/// A representative, fully-populated sample `Context` for `plugin run`'s
/// one-off render — a fabricated stand-in for the real `Context` built at
/// render time (invariant #1: a plugin's `render` only ever sees `Context`),
/// so a plugin author can exercise `render` without a live tmux pane. Built
/// from the shared [`crate::sample_context::sample_context`] (W52) with
/// `hostname` and the real host `os`/`arch` swapped in, since a plugin author
/// testing `render` locally likely wants their actual platform reported
/// rather than a fixed one.
fn sample_context() -> CoreContext {
    CoreContext {
        hostname: "devbox".into(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        ..crate::sample_context::sample_context(false)
    }
}

/// `rustline plugin run <name> [--plugin-dir <dir>]`: instantiate exactly the
/// named plugin — bypassing the layout-`needed` discovery filter `render`
/// uses — render it once against a fabricated [`sample_context`], and print
/// its segments plus any capability denials it triggered along the way (via a
/// [`CollectingObserver`] wired through `rustline_wasm::instantiate_named`).
/// Read-only: loads the config but never writes it, and never touches the
/// toggles file.
fn run_plugin(args: &RunArgs, config_path: &Path, plugin_dir: &Path) {
    let cfg = Config::load(config_path);
    let pc = cfg.plugins.get(&args.name).cloned().unwrap_or_default();
    let observer = Arc::new(CollectingObserver::default());
    let widget = rustline_wasm::instantiate_named(plugin_dir, &args.name, &pc, observer.clone());
    let Some(widget) = widget else {
        eprintln!(
            "failed to instantiate plugin {:?} from {}",
            args.name,
            plugin_dir.display()
        );
        std::process::exit(1);
    };
    let segments = widget.render(&sample_context());
    let denials = observer.snapshot();
    print!("{}", format_run_output(&segments, &denials));
}

/// `rustline plugin search [QUERY] [--json] [--refresh]` — browse the curated
/// plugin index. It never *modifies* anything: it reads the config (for
/// `plugin_index_url`) and reads the plugin dir (to mark which entries are
/// already installed), and its only write is the index cache under the state
/// dir. Finding a plugin here grants it nothing — only `plugin approve` or a
/// hand edit ever widens an allowlist.
fn search(config_path: &Path, plugin_dir: &Path, query: Option<&str>, json: bool, refresh: bool) {
    let cfg = Config::load(config_path);
    let url = cfg
        .plugin_index_url
        .as_deref()
        .unwrap_or(crate::plugin_index::DEFAULT_INDEX_URL);
    let state_dir = rustline_wasm::state_root();
    let now = crate::sample_store::now_unix_secs();

    let loaded = match crate::plugin_index::load_index(
        &crate::plugin_install::UreqDownloader,
        &state_dir,
        url,
        refresh,
        now,
    ) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("could not load the plugin index: {error:#}");
            std::process::exit(1);
        }
    };

    let entries = crate::plugin_index::filter_entries(&loaded.index, query);
    let installed = rustline_wasm::discover_plugin_names(plugin_dir);

    // Emit the staleness warning before the `--json` early return: it's on
    // stderr, so it never corrupts the machine-readable stdout, and a
    // scripted `--json` consumer must still be able to see it (the feature's
    // own requirement carries no `--json` exemption).
    if loaded.stale {
        eprintln!("warning: could not refresh the plugin index; showing a cached copy");
    }

    if json {
        println!("{}", crate::plugin_index::search_json(&entries, &installed));
        return;
    }

    if entries.is_empty() {
        match query {
            Some(q) => println!("no plugins in the index match {q:?}"),
            None => println!("the plugin index is empty"),
        }
        return;
    }

    for entry in &entries {
        let mark = if installed.iter().any(|n| n == &entry.name) {
            "  [installed]"
        } else {
            ""
        };
        println!("{}{}", entry.name, mark);
        if !entry.description.is_empty() {
            println!("  {}", entry.description);
        }
        if !entry.capabilities.is_empty() {
            println!("  capabilities: {}", entry.capabilities.join(", "));
        }
        match (entry.bundled, entry.source.as_deref()) {
            (true, _) => println!("  build:   just build-plugin {}", entry.name),
            (false, Some(source)) => {
                println!("  install: rustline plugin install {source}");
            }
            (false, None) => {}
        }
        println!();
    }
}

/// Print every configured plugin's source and allowlists/caps, noting any
/// declared capability manifest. With `json`, emit `plugin_list_json` instead.
fn list(config_path: &Path, plugin_dir: &Path, json: bool) {
    let cfg = Config::load(config_path);
    if json {
        println!("{}", plugin_list_json(&cfg, plugin_dir));
        return;
    }
    if cfg.plugins.is_empty() {
        println!("no plugins configured");
        return;
    }
    for (name, pc) in &cfg.plugins {
        println!("{name}");
        if let Some(src) = &pc.source {
            println!("  source: {src}");
            if let Some(tag) = &pc.tag {
                println!("  tag: {tag}");
            }
        }
        let checksum_status =
            crate::plugin_checksum::status_for(plugin_dir, name, pc.checksum.as_deref());
        println!("  checksum: {}", checksum_status.label());
        println!("  allowed_urls: {:?}", pc.allowed_urls);
        println!("  allowed_paths: {:?}", pc.allowed_paths);
        println!("  allowed_write_paths: {:?}", pc.allowed_write_paths);
        println!("  resolve_symlinks: {}", pc.resolve_symlinks);
        println!("  allowed_commands: {:?}", pc.allowed_commands);
        println!("  max_state_bytes: {}", pc.max_state_bytes);
        if let Some(m) = resolve_manifest(plugin_dir, name) {
            println!(
                "  declared capabilities: {} urls, {} paths, {} write paths, {} commands (run `plugin approve {name}`)",
                m.requested_urls.len(),
                m.requested_paths.len(),
                m.requested_write_paths.len(),
                m.requested_commands.len()
            );
        }
    }
}

/// Run a `list`/`add`/`remove` operation over one allowlist (`kind`) of one
/// named plugin.
fn pattern_cmd(cmd: PatternCmd, kind: Kind, config_path: &Path) {
    match cmd {
        PatternCmd::List { plugin, json } => {
            let cfg = Config::load(config_path);
            let patterns = cfg.plugins.get(&plugin).map(|p| match kind {
                Kind::Url => &p.allowed_urls,
                Kind::Path => &p.allowed_paths,
                Kind::WritePath => &p.allowed_write_paths,
                Kind::Command => &p.allowed_commands,
            });
            if json {
                println!("{}", pattern_list_json(patterns));
            } else {
                match patterns {
                    Some(list) if !list.is_empty() => list.iter().for_each(|p| println!("{p}")),
                    Some(_) => println!("(none)"),
                    None => println!("no such plugin: {plugin}"),
                }
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
///
/// Also surfaces a `plugin denials` hint whenever `<name>` has any persisted
/// denials, independent of whether a manifest resolves or approval proceeds —
/// a denial record is informational only, never itself grounds for widening
/// an allowlist.
fn approve(args: ApproveArgs, config_path: &Path, plugin_dir: &Path) {
    if !rustline_wasm::read_denials(&args.plugin).is_empty() {
        println!("recorded denials: run `plugin denials {}`", args.plugin);
    }
    let Some(manifest) = resolve_manifest(plugin_dir, &args.plugin) else {
        println!("no manifest found for {}", args.plugin);
        return;
    };
    print!("{}", manifest_report(&manifest));
    if manifest.requested_urls.is_empty()
        && manifest.requested_paths.is_empty()
        && manifest.requested_write_paths.is_empty()
        && manifest.requested_commands.is_empty()
    {
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

/// The text `approve` shows before its confirmation prompt: the plugin's
/// identity, exactly what it requests, and — when it asks for commands — a
/// warning that this capability is categorically different from reading a URL
/// or a file. The warning is part of the report (not the prompt) so it also
/// lands in a `--yes` run's output and in logs.
fn manifest_report(m: &PluginManifest) -> String {
    let name = if m.name.is_empty() { "?" } else { &m.name };
    let version = if m.version.is_empty() {
        "?"
    } else {
        &m.version
    };
    let mut out = String::new();
    let _ = writeln!(out, "plugin {name} (version {version}) requests:");
    write_requests(&mut out, "allowed_urls", &m.requested_urls);
    write_requests(&mut out, "allowed_paths (read-only)", &m.requested_paths);
    write_requests(&mut out, "allowed_write_paths", &m.requested_write_paths);
    write_requests(&mut out, "allowed_commands", &m.requested_commands);
    if !m.requested_write_paths.is_empty() {
        out.push_str(
            "\n  ! allowed_write_paths lets this plugin overwrite these files with\n\
             \x20 ! any content. Approve only paths you understand.\n",
        );
    }
    if !m.requested_commands.is_empty() {
        out.push_str(
            "\n  ! allowed_commands runs real programs on your machine with your\n\
             \x20 ! environment and permissions. Approve only patterns you understand.\n",
        );
    }
    out
}

/// Append one requested-capability list under `label`, or `(none)`.
fn write_requests(out: &mut String, label: &str, entries: &[String]) {
    let _ = writeln!(out, "  {label}:");
    if entries.is_empty() {
        out.push_str("    (none)\n");
    } else {
        for e in entries {
            let _ = writeln!(out, "    {e}");
        }
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
    if !m.requested_write_paths.is_empty() {
        append_unique(
            allowlist_array(table, plugin, Kind::WritePath.key()),
            &m.requested_write_paths,
        );
    }
    if !m.requested_commands.is_empty() {
        append_unique(
            allowlist_array(table, plugin, Kind::Command.key()),
            &m.requested_commands,
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

/// Validate a plugin name: non-empty, only `[A-Za-z0-9_-]` (so `/`, `\`,
/// `..`, spaces, and dots are all rejected without special-casing each), at
/// most 15 bytes (tmux's `range=user|X` byte cap — invariant #7, so the
/// scaffolded plugin stays click-toggleable), and not the reserved name
/// `window`. Rebuilt on [`RangeName::parse`] (T1) — the one place this rule
/// is actually checked — but keeps its own pre-existing message text per
/// variant rather than `NameError`'s `Display`, since those strings are this
/// command's user-facing contract. Pure and unit-tested so the scaffold
/// command can reject a bad name before touching disk.
fn validate_plugin_name(name: &str) -> Result<(), String> {
    match RangeName::parse(name) {
        Ok(_) => Ok(()),
        Err(NameError::Empty) => Err("plugin name must not be empty".to_string()),
        Err(NameError::TooLong { len }) => Err(format!(
            "plugin name {name:?} is {len} bytes; must be at most {MAX_PLUGIN_NAME_BYTES} \
             (tmux's range=user|X limit)"
        )),
        Err(NameError::BadChar { .. }) => Err(format!(
            "plugin name {name:?} may only contain letters, digits, `_`, and `-`"
        )),
        Err(NameError::Reserved) => {
            Err(format!("plugin name {RESERVED_PLUGIN_NAME:?} is reserved"))
        }
    }
}

/// Whether `plugin new` must refuse to scaffold into a directory that
/// `dir_exists` without `force`. Factored out so the refusal condition is
/// unit-testable without exercising `new_plugin`'s `process::exit`.
fn should_refuse_overwrite(dir_exists: bool, force: bool) -> bool {
    dir_exists && !force
}

/// Substitute every `{{name}}` placeholder in an embedded template with
/// `name`. `name` is already validated to `[A-Za-z0-9_-]`, so it never needs
/// escaping in the TOML/Rust-string contexts it lands in.
fn render_template(template: &str, name: &str) -> String {
    template.replace("{{name}}", name)
}

/// `rustline plugin new <name> [--path] [--force]`: scaffold a ready-to-edit
/// WASM guest plugin crate at `<path or cwd>/<name>/` from the embedded
/// templates, then print the `.wasm` install step and a starter
/// `[plugins.<name>]` config snippet.
fn new_plugin(args: &NewPluginArgs) {
    let name = args.name.as_str();
    if let Err(e) = validate_plugin_name(name) {
        eprintln!("invalid plugin name: {e}");
        std::process::exit(1);
    }
    let base = args
        .path
        .as_deref()
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    let dir = base.join(name);
    if should_refuse_overwrite(dir.exists(), args.force) {
        eprintln!(
            "{} already exists (use --force to overwrite)",
            dir.display()
        );
        std::process::exit(1);
    }
    let src_dir = dir.join("src");
    if let Err(e) = std::fs::create_dir_all(&src_dir) {
        eprintln!("failed to create {}: {e}", src_dir.display());
        std::process::exit(1);
    }
    if let Err(e) = std::fs::write(
        dir.join("Cargo.toml"),
        render_template(PLUGIN_CARGO_TEMPLATE, name),
    ) {
        eprintln!("failed to write Cargo.toml: {e}");
        std::process::exit(1);
    }
    if let Err(e) = std::fs::write(
        src_dir.join("lib.rs"),
        render_template(PLUGIN_LIB_TEMPLATE, name),
    ) {
        eprintln!("failed to write src/lib.rs: {e}");
        std::process::exit(1);
    }
    println!("scaffolded plugin crate at {}", dir.display());
    print_next_steps(name, &dir);
}

/// Print the build/install steps and a starter `[plugins.<name>]` config
/// snippet for a freshly scaffolded plugin at `dir`.
///
/// Cargo normalizes a hyphenated package name to underscores for the built
/// artifact's filename (e.g. `my-widget` builds `my_widget.wasm`), but
/// plugin discovery matches a plugin's exported `name()` against the
/// *installed* file's stem (see `rustline-wasm::register_plugins`), so the
/// `cp` step renames back to the hyphenated form the plugin actually
/// identifies as.
fn print_next_steps(name: &str, dir: &Path) {
    let build_stem = name.replace('-', "_");
    println!();
    println!("build it for wasm32 and install it into your plugin dir:");
    println!();
    println!("  cd {}", dir.display());
    println!("  cargo build --release --target wasm32-unknown-unknown");
    println!(
        "  cp target/wasm32-unknown-unknown/release/{build_stem}.wasm <plugin_dir>/{name}.wasm"
    );
    println!();
    println!("wire it into config.toml:");
    println!();
    println!("[plugins.{name}]");
    println!("allowed_urls = []");
    println!("allowed_paths = []");
    println!("allowed_write_paths = []");
    println!("resolve_symlinks = false");
    println!();
    println!("[plugins.{name}.options]");
    println!("format = \"{name}: hello!\"");
}

/// Read a crate's `[package].name` out of its `Cargo.toml`, so `plugin build`
/// can derive a plugin's identity — and thus its installed `.wasm` filename —
/// from an arbitrary external crate directory, the same identity `plugin new`
/// fixes at scaffold time.
fn package_name(cargo_toml: &Path) -> anyhow::Result<String> {
    let text = std::fs::read_to_string(cargo_toml)
        .with_context(|| format!("failed to read {}", cargo_toml.display()))?;
    let doc: DocumentMut = text
        .parse()
        .with_context(|| format!("{} is not valid TOML", cargo_toml.display()))?;
    doc.get("package")
        .and_then(Item::as_table)
        .and_then(|t| t.get("name"))
        .and_then(Item::as_str)
        .map(str::to_string)
        .with_context(|| format!("{} has no [package].name", cargo_toml.display()))
}

/// The `cargo build` arguments for compiling a plugin crate to the wasm32
/// target, honoring `--release`. Factored out as a pure function so the
/// argument assembly is unit-tested without shelling out to a real `cargo`.
fn cargo_build_args(release: bool) -> Vec<&'static str> {
    let mut args = vec!["build", "--target", "wasm32-unknown-unknown"];
    if release {
        args.push("--release");
    }
    args
}

/// Where `cargo build --target wasm32-unknown-unknown [--release]` writes its
/// build artifact: `<target_dir>/wasm32-unknown-unknown/{release|debug}/<stem>.wasm`.
/// `stem` is the *build* artifact's stem — cargo normalizes a hyphenated
/// crate name to underscores for the output filename (see `print_next_steps`
/// above) — which can differ from the plugin's own hyphenated identity;
/// `build_plugin` resolves both and keeps them straight.
fn wasm_artifact_path(target_dir: &Path, stem: &str, release: bool) -> PathBuf {
    let profile = if release { "release" } else { "debug" };
    target_dir
        .join("wasm32-unknown-unknown")
        .join(profile)
        .join(format!("{stem}.wasm"))
}

/// `rustline plugin build <dir> [--release] [--plugin-dir <dir>] [--yes]`:
/// build any WASM guest plugin crate at `<dir>` — not limited to this repo's
/// own `plugins/*`, the generic counterpart to `just build-plugin NAME` — and
/// install the resulting `.wasm` into `plugin_dir`, named after the crate's
/// own `[package].name` (hyphens intact, matching plugin discovery's
/// filename-stem convention). A missing wasm target or non-zero `cargo build`
/// exit surfaces as the process's own stderr output plus a clear error here;
/// a missing artifact afterward (e.g. a non-`cdylib` crate) is likewise a
/// clear error — never a panic.
///
/// After a successful install, checks the freshly built bytes against
/// `config_path`'s recorded `[plugins.<name>].checksum` (see
/// [`maybe_refresh_stale_checksum`]): `plugin install`/`update` record a
/// checksum, and rebuilding from source without updating it would otherwise
/// silently fail `register_plugins`' load-time gate on the very next render.
fn build_plugin(args: &BuildArgs, plugin_dir: &Path, config_path: &Path) -> anyhow::Result<()> {
    let name = package_name(&args.dir.join("Cargo.toml"))?;
    let build_stem = name.replace('-', "_");

    let status = Command::new("cargo")
        .args(cargo_build_args(args.release))
        .current_dir(&args.dir)
        .status()
        .with_context(|| format!("failed to run `cargo build` in {}", args.dir.display()))?;
    if !status.success() {
        bail!("cargo build failed in {} ({status})", args.dir.display());
    }

    let artifact = wasm_artifact_path(&args.dir.join("target"), &build_stem, args.release);
    if !artifact.is_file() {
        bail!(
            "expected wasm artifact not found at {} (is the wasm32-unknown-unknown target \
             installed via `rustup target add wasm32-unknown-unknown`, and is `{name}`'s \
             [lib] crate-type [\"cdylib\"]?)",
            artifact.display()
        );
    }

    std::fs::create_dir_all(plugin_dir)
        .with_context(|| format!("failed to create plugin dir {}", plugin_dir.display()))?;
    let dest = plugin_dir.join(format!("{name}.wasm"));
    let bytes = std::fs::read(&artifact)
        .with_context(|| format!("failed to read {}", artifact.display()))?;
    std::fs::write(&dest, &bytes).with_context(|| {
        format!(
            "failed to install {} to {}",
            artifact.display(),
            dest.display()
        )
    })?;

    println!("built and installed {name}.wasm -> {}", dest.display());
    maybe_refresh_stale_checksum(config_path, &name, &bytes, args.yes);
    Ok(())
}

/// After `plugin build` installs a fresh `.wasm`, react to its recorded
/// `checksum` (if any) no longer matching. Quiet whenever
/// [`rustline_wasm::ChecksumVerdict::allows_load`] is true — no checksum
/// recorded, or the freshly built bytes still match — since that's the common
/// case and must stay silent. Otherwise:
///
/// - `--yes` (`auto_yes`), or an interactive terminal answering yes
///   (Enter defaults to yes — refreshing a hash after the user's own rebuild
///   isn't a new capability grant, unlike `plugin approve`'s confirm, which
///   defaults to no) — rewrites `[plugins.<name>].checksum` via
///   [`crate::plugin_install::write_checksum`].
/// - Otherwise (declined, or stdin isn't a terminal) — leaves the config
///   alone and prints [`stale_checksum_notice`]. A non-interactive run is
///   never prompted (it would hang a script/CI) and never silently re-pins
///   the checksum on its own — that would let a compromised local toolchain
///   bless itself automatically, exactly what recording a checksum guards
///   against.
fn maybe_refresh_stale_checksum(config_path: &Path, name: &str, bytes: &[u8], auto_yes: bool) {
    let cfg = Config::load(config_path);
    let Some(pc) = cfg.plugins.get(name) else {
        return; // plugin isn't configured at all -> nothing recorded to go stale
    };
    if rustline_wasm::verify_checksum(pc.checksum.as_deref(), bytes).allows_load() {
        return; // no checksum recorded, or it still matches: stay quiet
    }

    let refresh = auto_yes || (std::io::stdin().is_terminal() && confirm_checksum_refresh());
    if refresh {
        let new_checksum = rustline_wasm::sha256_hex(bytes);
        match crate::plugin_install::write_checksum(config_path, name, &new_checksum) {
            Ok(()) => println!("updated the recorded checksum for {name}"),
            Err(e) => eprintln!("failed to update the recorded checksum for {name}: {e:#}"),
        }
        return;
    }
    println!("{}", stale_checksum_notice(name));
}

/// Interactive confirmation for refreshing a stale recorded checksum after a
/// rebuild. Enter means yes — unlike `confirm()`'s approve prompt (which
/// defaults to no, since that one grants a new capability), this is just
/// re-pinning a hash to match bytes the user just built themselves. Reuses
/// `init::parse_yes_no` for the actual yes/no parse.
fn confirm_checksum_refresh() -> bool {
    print!("Update the recorded checksum? [Y/n] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    crate::init::parse_yes_no(&line, true)
}

/// The message printed when a plugin's recorded checksum no longer matches a
/// freshly built `.wasm` and it wasn't refreshed (declined interactively, or
/// skipped because stdin isn't a terminal). Pure and unit-tested.
fn stale_checksum_notice(name: &str) -> String {
    format!(
        "note: the recorded checksum for {name} no longer matches this build; it will be \
         rejected at load time until refreshed. Re-run `plugin build` with --yes, answer y \
         at the prompt, or edit [plugins.{name}].checksum by hand."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(urls: &[&str], paths: &[&str]) -> PluginManifest {
        manifest_with_write_paths(urls, paths, &[])
    }

    /// Sibling to [`manifest`] for tests that need `requested_write_paths`
    /// populated too, without adding a rarely-used parameter to every
    /// existing `manifest(urls, paths)` call site.
    fn manifest_with_write_paths(
        urls: &[&str],
        paths: &[&str],
        write_paths: &[&str],
    ) -> PluginManifest {
        PluginManifest {
            name: "w".into(),
            version: "1".into(),
            requested_urls: urls.iter().map(|s| s.to_string()).collect(),
            requested_paths: paths.iter().map(|s| s.to_string()).collect(),
            requested_write_paths: write_paths.iter().map(|s| s.to_string()).collect(),
            requested_commands: Vec::new(),
        }
    }

    fn denial(kind: DenialKind, target: &str) -> Denial {
        Denial {
            kind,
            target: target.to_string(),
        }
    }

    #[test]
    fn format_run_output_lists_segment_text_and_denials() {
        let segments = vec![Segment::new("cool widget"), Segment::new("42")];
        let denials = vec![
            denial(DenialKind::Url, "https://evil.example/"),
            denial(DenialKind::Path, "/etc/passwd"),
        ];

        let out = format_run_output(&segments, &denials);

        assert!(out.contains("cool widget"), "{out}");
        assert!(out.contains("42"), "{out}");
        assert!(out.contains("url denied: https://evil.example/"), "{out}");
        assert!(out.contains("path denied: /etc/passwd"), "{out}");
    }

    #[test]
    fn format_run_output_reports_none_when_nothing_rendered_or_denied() {
        let out = format_run_output(&[], &[]);
        assert!(out.contains("(none"), "empty segments section: {out}");
        assert!(out.contains("(none)"), "empty denials section: {out}");
    }

    #[test]
    fn format_run_output_denials_none_when_only_segments_present() {
        let segments = vec![Segment::new("hi")];
        let out = format_run_output(&segments, &[]);
        assert!(out.contains("hi"), "{out}");
        assert!(out.contains("denials:\n  (none)"), "{out}");
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
    fn write_grants_writes_requested_commands_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "[plugins.w]\n").unwrap();

        let mut m = manifest(&[], &[]);
        m.requested_commands = vec!["playerctl metadata*".to_string()];
        write_grants(&cfg, "w", &m);

        let text = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(
            list_of(&text, "w", "allowed_commands"),
            ["playerctl metadata*"]
        );
        // Nothing was widened beyond what was requested.
        assert!(!text.contains("allowed_urls"), "{text}");
    }

    #[test]
    fn approving_commands_twice_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "[plugins.w]\n").unwrap();
        let mut m = manifest(&[], &[]);
        m.requested_commands = vec!["git status*".to_string()];
        write_grants(&cfg, "w", &m);
        write_grants(&cfg, "w", &m);
        let text = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(list_of(&text, "w", "allowed_commands"), ["git status*"]);
    }

    /// The load-bearing regression test for this task: a manifest requesting
    /// only a *read* path must never grant a write. Before the split,
    /// `requested_paths` was written straight into the single `allowed_paths`
    /// list that both `perform_file_read` and `perform_file_write` gated on;
    /// this pins that `write_grants` now writes `requested_paths` into
    /// `allowed_paths` ONLY, never touching `allowed_write_paths`.
    #[test]
    fn write_grants_a_read_only_manifest_never_populates_the_write_allowlist() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "[plugins.w]\n").unwrap();

        write_grants(&cfg, "w", &manifest(&[], &["/home/u/.bashrc"]));

        let text = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(list_of(&text, "w", "allowed_paths"), ["/home/u/.bashrc"]);
        assert!(
            list_of(&text, "w", "allowed_write_paths").is_empty(),
            "a read-only request must not grant a write: {text}"
        );
        assert!(!text.contains("allowed_write_paths"), "{text}");
    }

    #[test]
    fn write_grants_writes_requested_write_paths_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "[plugins.w]\n").unwrap();

        write_grants(
            &cfg,
            "w",
            &manifest_with_write_paths(&[], &[], &["/home/u/notes/*"]),
        );

        let text = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(
            list_of(&text, "w", "allowed_write_paths"),
            ["/home/u/notes/*"]
        );
        // Nothing was widened beyond what was requested.
        assert!(list_of(&text, "w", "allowed_paths").is_empty());
        assert!(!text.contains("allowed_urls"), "{text}");
    }

    #[test]
    fn approving_write_paths_twice_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "[plugins.w]\n").unwrap();
        let m = manifest_with_write_paths(&[], &[], &["/home/u/notes/*"]);
        write_grants(&cfg, "w", &m);
        write_grants(&cfg, "w", &m);
        let text = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(
            list_of(&text, "w", "allowed_write_paths"),
            ["/home/u/notes/*"]
        );
    }

    /// End-to-end coverage for `approve` itself (not just `write_grants`): a
    /// manifest requesting *only* `requested_write_paths` — no urls, read
    /// paths, or commands — must still be approved. `approve`'s "nothing to
    /// approve" guard checks all four requested-capability lists; dropping
    /// the `requested_write_paths.is_empty()` term makes a write-only
    /// manifest look like it requests nothing, so `approve` would print
    /// "manifest requests no capabilities; nothing to approve" and return
    /// before ever calling `write_grants` — silently failing closed for the
    /// one manifest shape this task introduces.
    #[test]
    fn approve_grants_a_write_only_manifest() {
        let plugin_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            plugin_dir.path().join("w.toml"),
            "name = \"w\"\nversion = \"1\"\nrequested_write_paths = [\"/home/u/notes/*\"]\n",
        )
        .unwrap();

        let config_dir = tempfile::tempdir().unwrap();
        let cfg = config_dir.path().join("config.toml");
        std::fs::write(&cfg, "[plugins.w]\n").unwrap();

        approve(
            ApproveArgs {
                plugin: "w".to_string(),
                yes: true,
            },
            &cfg,
            plugin_dir.path(),
        );

        let text = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(
            list_of(&text, "w", "allowed_write_paths"),
            ["/home/u/notes/*"],
            "a write-only manifest must be approved, not silently skipped: {text}"
        );
    }

    #[test]
    fn the_command_kind_maps_to_the_allowed_commands_key() {
        assert_eq!(Kind::Command.key(), "allowed_commands");
    }

    #[test]
    fn the_write_path_kind_maps_to_the_allowed_write_paths_key() {
        assert_eq!(Kind::WritePath.key(), "allowed_write_paths");
    }

    #[test]
    fn a_manifest_requesting_commands_prints_the_execution_warning() {
        let mut m = manifest(&[], &[]);
        m.requested_commands = vec!["git status*".to_string()];
        let text = manifest_report(&m);
        assert!(text.contains("allowed_commands"), "{text}");
        assert!(text.contains("git status*"), "{text}");
        assert!(
            text.to_lowercase().contains("runs real programs"),
            "the exec warning is shown: {text}"
        );
    }

    #[test]
    fn a_manifest_without_commands_prints_no_execution_warning() {
        let m = manifest(&["https://a/*"], &[]);
        let text = manifest_report(&m);
        assert!(
            !text.to_lowercase().contains("runs real programs"),
            "{text}"
        );
    }

    #[test]
    fn a_manifest_requesting_write_paths_prints_the_overwrite_warning() {
        let m = manifest_with_write_paths(&[], &[], &["/home/u/notes/*"]);
        let text = manifest_report(&m);
        assert!(text.contains("allowed_write_paths"), "{text}");
        assert!(text.contains("/home/u/notes/*"), "{text}");
        assert!(
            text.to_lowercase().contains("overwrite"),
            "the overwrite warning is shown: {text}"
        );
    }

    #[test]
    fn a_manifest_without_write_paths_prints_no_overwrite_warning() {
        let m = manifest(&["https://a/*"], &["/tmp/x"]);
        let text = manifest_report(&m);
        assert!(!text.to_lowercase().contains("overwrite"), "{text}");
    }

    #[test]
    fn a_manifest_report_labels_the_read_only_path_line() {
        let m = manifest(&[], &["/tmp/x"]);
        let text = manifest_report(&m);
        assert!(text.contains("allowed_paths (read-only)"), "{text}");
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

    #[test]
    fn validate_plugin_name_accepts_ordinary_names() {
        assert!(validate_plugin_name("my-widget").is_ok());
        assert!(validate_plugin_name("w1").is_ok());
        assert!(validate_plugin_name("under_score").is_ok());
        // exactly 15 bytes is still allowed
        assert_eq!("exactly15bytesX".len(), 15);
        assert!(validate_plugin_name("exactly15bytesX").is_ok());
    }

    #[test]
    fn validate_plugin_name_rejects_bad_names() {
        assert!(validate_plugin_name("has/slash").is_err());
        assert!(validate_plugin_name("has\\backslash").is_err());
        assert!(validate_plugin_name("..").is_err());
        assert!(validate_plugin_name("dot.dot").is_err());
        assert!(validate_plugin_name("way-too-long-name-16b").is_err()); // >15 bytes
        assert!(validate_plugin_name("window").is_err()); // reserved
        assert!(validate_plugin_name("").is_err());
        assert!(validate_plugin_name("foo bar").is_err()); // space
    }

    #[test]
    fn render_template_substitutes_every_occurrence() {
        let out = render_template("name={{name}} again={{name}}", "my-widget");
        assert_eq!(out, "name=my-widget again=my-widget");
    }

    #[test]
    fn new_plugin_scaffolds_cargo_toml_and_lib_rs() {
        let tmp = tempfile::tempdir().unwrap();
        new_plugin(&NewPluginArgs {
            name: "mywidget".to_string(),
            path: Some(tmp.path().to_string_lossy().into_owned()),
            force: false,
        });

        let dir = tmp.path().join("mywidget");
        let cargo_toml = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(
            cargo_toml.contains("[workspace]"),
            "empty workspace table: {cargo_toml}"
        );
        assert!(cargo_toml.contains("edition = \"2024\""));
        assert!(cargo_toml.contains("crate-type = [\"cdylib\"]"));
        assert!(cargo_toml.contains("name = \"mywidget\""));

        let lib_rs = std::fs::read_to_string(dir.join("src/lib.rs")).unwrap();
        assert!(lib_rs.contains("WireContext") || lib_rs.contains("GuestRender"));
        assert!(lib_rs.contains("\"mywidget\""));
    }

    #[test]
    fn should_refuse_overwrite_only_when_exists_and_not_forced() {
        // This is exactly the guard `new_plugin` checks before writing, kept
        // as a standalone pure predicate so it's testable without exercising
        // `new_plugin`'s `process::exit` refusal path.
        assert!(should_refuse_overwrite(true, false));
        assert!(!should_refuse_overwrite(true, true));
        assert!(!should_refuse_overwrite(false, false));
        assert!(!should_refuse_overwrite(false, true));
    }

    #[test]
    fn cargo_build_args_release_vs_debug() {
        assert_eq!(
            cargo_build_args(true),
            vec!["build", "--target", "wasm32-unknown-unknown", "--release"]
        );
        assert_eq!(
            cargo_build_args(false),
            vec!["build", "--target", "wasm32-unknown-unknown"]
        );
    }

    #[test]
    fn package_name_reads_from_cargo_toml() {
        let dir = tempfile::tempdir().unwrap();
        let cargo_toml = dir.path().join("Cargo.toml");
        std::fs::write(
            &cargo_toml,
            "[package]\nname = \"my-widget\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        assert_eq!(package_name(&cargo_toml).unwrap(), "my-widget");
    }

    #[test]
    fn package_name_errors_on_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(package_name(&dir.path().join("Cargo.toml")).is_err());
    }

    #[test]
    fn package_name_errors_on_missing_package_table() {
        let dir = tempfile::tempdir().unwrap();
        let cargo_toml = dir.path().join("Cargo.toml");
        std::fs::write(&cargo_toml, "[dependencies]\n").unwrap();
        assert!(package_name(&cargo_toml).is_err());
    }

    #[test]
    fn pattern_list_json_none_is_empty_array_some_is_array() {
        // Absent/empty allowlist → valid empty JSON array (stdout stays
        // parseable in CI), never a human "no such plugin" string.
        assert_eq!(pattern_list_json(None), "[]");
        let patterns = vec!["https://wttr.in/*".to_string()];
        let json = pattern_list_json(Some(&patterns));
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0], "https://wttr.in/*");
    }

    #[test]
    fn denials_json_shape() {
        let denials = vec![
            rustline_wasm::Denial {
                plugin: "weather".into(),
                kind: DenialKind::Url,
                target: "https://evil.example/".into(),
            },
            rustline_wasm::Denial {
                plugin: "weather".into(),
                kind: DenialKind::Path,
                target: "/etc/passwd".into(),
            },
        ];
        let json = denials_json(&denials);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v[0]["kind"], "url");
        assert_eq!(v[0]["target"], "https://evil.example/");
        assert_eq!(v[1]["kind"], "path");
    }

    /// A plugin holding a write grant must not render as read-only in
    /// machine-readable output — the same understated-grant shape D3 exists
    /// to close, reproduced one layer down at the `--json` audit surface.
    #[test]
    fn plugin_list_json_surfaces_a_write_grant() {
        let mut cfg = Config::default();
        let pc = rustline_core::PluginConfig {
            allowed_write_paths: vec!["/home/u/.bashrc".to_string()],
            resolve_symlinks: true,
            ..Default::default()
        };
        cfg.plugins.insert("evil".to_string(), pc);

        let json = plugin_list_json(&cfg, std::path::Path::new("/nonexistent-plugin-dir"));
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let e = v
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "evil")
            .unwrap();
        assert_eq!(e["allowed_write_paths"][0], "/home/u/.bashrc", "{json}");
        assert_eq!(e["resolve_symlinks"], true, "{json}");
    }

    #[test]
    fn plugin_list_json_has_expected_fields() {
        let mut cfg = Config::default();
        let pc = rustline_core::PluginConfig {
            allowed_urls: vec!["https://wttr.in/*".to_string()],
            ..Default::default()
        };
        cfg.plugins.insert("weather".to_string(), pc);
        // A path with no manifest sidecar → has_manifest false.
        let json = plugin_list_json(&cfg, std::path::Path::new("/nonexistent-plugin-dir"));
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let w = v
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "weather")
            .unwrap();
        assert_eq!(w["allowed_urls"][0], "https://wttr.in/*");
        assert_eq!(w["has_manifest"], false);
        assert!(w.get("max_state_bytes").is_some());
        // No .wasm at all under a nonexistent plugin dir -> "missing", not an
        // error and not silently dropped from the payload.
        assert_eq!(w["checksum_status"], "missing");
    }

    #[test]
    fn plugin_list_json_reports_a_verified_checksum() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"totally a wasm binary";
        std::fs::write(dir.path().join("weather.wasm"), bytes).unwrap();

        let mut cfg = Config::default();
        let pc = rustline_core::PluginConfig {
            checksum: Some(rustline_wasm::sha256_hex(bytes)),
            ..Default::default()
        };
        cfg.plugins.insert("weather".to_string(), pc);

        let json = plugin_list_json(&cfg, dir.path());
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let w = v
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "weather")
            .unwrap();
        assert_eq!(w["checksum_status"], "verified");
    }

    #[test]
    fn plugin_list_json_reports_a_mismatched_checksum() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("weather.wasm"), b"actual bytes").unwrap();

        let mut cfg = Config::default();
        let pc = rustline_core::PluginConfig {
            checksum: Some(rustline_wasm::sha256_hex(b"different bytes")),
            ..Default::default()
        };
        cfg.plugins.insert("weather".to_string(), pc);

        let json = plugin_list_json(&cfg, dir.path());
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let w = v
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "weather")
            .unwrap();
        assert_eq!(w["checksum_status"], "mismatch");
    }

    #[test]
    fn stale_checksum_notice_names_the_plugin_and_how_to_fix_it() {
        let msg = stale_checksum_notice("weather");
        assert!(msg.contains("weather"), "{msg}");
        assert!(msg.to_lowercase().contains("checksum"), "{msg}");
        assert!(msg.contains("--yes"), "{msg}");
    }

    #[test]
    fn maybe_refresh_stale_checksum_is_quiet_when_plugin_unconfigured() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "").unwrap();
        maybe_refresh_stale_checksum(&cfg, "weather", b"bytes", true);
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            "",
            "nothing configured -> nothing to check, nothing written"
        );
    }

    #[test]
    fn maybe_refresh_stale_checksum_is_quiet_when_no_checksum_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let original = "[plugins.weather]\nallowed_urls = []\n";
        std::fs::write(&cfg, original).unwrap();
        maybe_refresh_stale_checksum(&cfg, "weather", b"bytes", true);
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            original,
            "no checksum recorded -> must stay quiet, even with --yes"
        );
    }

    #[test]
    fn maybe_refresh_stale_checksum_is_quiet_when_it_already_matches() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let sum = rustline_wasm::sha256_hex(b"bytes");
        let original = format!("[plugins.weather]\nchecksum = \"{sum}\"\n");
        std::fs::write(&cfg, &original).unwrap();
        maybe_refresh_stale_checksum(&cfg, "weather", b"bytes", true);
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            original,
            "already-matching checksum -> must stay quiet"
        );
    }

    #[test]
    fn maybe_refresh_stale_checksum_with_yes_rewrites_a_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let stale = rustline_wasm::sha256_hex(b"old bytes");
        std::fs::write(&cfg, format!("[plugins.weather]\nchecksum = \"{stale}\"\n")).unwrap();

        maybe_refresh_stale_checksum(&cfg, "weather", b"new bytes", true);

        let text = std::fs::read_to_string(&cfg).unwrap();
        let expected = rustline_wasm::sha256_hex(b"new bytes");
        assert!(text.contains(&expected), "{text}");
        assert!(!text.contains(&stale), "{text}");
    }

    #[test]
    fn maybe_refresh_stale_checksum_with_yes_rewrites_a_malformed_digest() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        std::fs::write(
            &cfg,
            "[plugins.weather]\nchecksum = \"not-a-real-digest\"\n",
        )
        .unwrap();

        maybe_refresh_stale_checksum(&cfg, "weather", b"new bytes", true);

        let text = std::fs::read_to_string(&cfg).unwrap();
        let expected = rustline_wasm::sha256_hex(b"new bytes");
        assert!(text.contains(&expected), "{text}");
        assert!(!text.contains("not-a-real-digest"), "{text}");
    }

    #[test]
    fn maybe_refresh_stale_checksum_without_yes_leaves_config_untouched() {
        // No --yes and (per the doc comment) this never prompts/blocks on its
        // own when called directly without a live confirmation loop backing
        // it — proven at the process level by the non-TTY smoke test; this
        // unit test pins the "config is untouched unless refresh happens"
        // half by driving `write_checksum` around it, not this function's
        // internal TTY probe (which depends on the test harness's own stdin).
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let stale = rustline_wasm::sha256_hex(b"old bytes");
        let original = format!("[plugins.weather]\nchecksum = \"{stale}\"\n");
        std::fs::write(&cfg, &original).unwrap();

        // auto_yes=false without a TTY: `is_terminal()` is false in the test
        // harness's typical (non-interactive) stdin, so this must not prompt
        // and must not rewrite.
        if !std::io::stdin().is_terminal() {
            maybe_refresh_stale_checksum(&cfg, "weather", b"new bytes", false);
            assert_eq!(
                std::fs::read_to_string(&cfg).unwrap(),
                original,
                "non-interactive run without --yes must leave the checksum alone"
            );
        }
    }

    #[test]
    fn wasm_artifact_path_release_vs_debug() {
        let target_dir = PathBuf::from("/proj/target");
        assert_eq!(
            wasm_artifact_path(&target_dir, "my_widget", true),
            PathBuf::from("/proj/target/wasm32-unknown-unknown/release/my_widget.wasm")
        );
        assert_eq!(
            wasm_artifact_path(&target_dir, "my_widget", false),
            PathBuf::from("/proj/target/wasm32-unknown-unknown/debug/my_widget.wasm")
        );
    }

    #[test]
    fn new_plugin_force_overwrites_existing_scaffold() {
        let tmp = tempfile::tempdir().unwrap();
        let args = NewPluginArgs {
            name: "mywidget".to_string(),
            path: Some(tmp.path().to_string_lossy().into_owned()),
            force: false,
        };
        new_plugin(&args); // first run: dir doesn't exist yet, succeeds

        // Mutate the scaffolded file to prove a subsequent forced run
        // actually rewrites it rather than leaving it alone.
        let cargo_path = tmp.path().join("mywidget").join("Cargo.toml");
        std::fs::write(&cargo_path, "mutated").unwrap();

        let forced = NewPluginArgs {
            name: "mywidget".to_string(),
            path: Some(tmp.path().to_string_lossy().into_owned()),
            force: true,
        };
        new_plugin(&forced);
        let overwritten = std::fs::read_to_string(&cargo_path).unwrap();
        assert_ne!(overwritten, "mutated");
        assert!(overwritten.contains("[workspace]"));
    }
}
