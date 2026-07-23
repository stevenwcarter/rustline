//! `rustline plugin …` — list plugins, edit their capability allowlists, and
//! approve a plugin's declared capability manifest. Mutations use `toml_edit`
//! so the user's comments and formatting survive.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use anyhow::{Context, bail};
use chrono::Local;
use rustline_core::{
    Battery, BatteryState, Config, Context as CoreContext, CpuUsage, DiskInfo, GitInfo, MemInfo,
    NetIface, Segment, Widget, WindowCtx,
};
use rustline_wasm::{DenialKind, DenialObserver, PluginManifest, resolve_manifest};
use toml_edit::{Array, DocumentMut, Item, Table, Value};

use crate::cli::{ApproveArgs, BuildArgs, NewPluginArgs, PatternCmd, PluginCmd, RunArgs};

/// The reserved widget name that a plugin must never claim (it names the
/// built-in window-list renderer, which isn't a plugin-resolvable slot).
const RESERVED_PLUGIN_NAME: &str = "window";

/// `tmux`'s `range=user|X` status-range argument is byte-capped; a plugin
/// name longer than this can never be click-toggleable (invariant #7).
const MAX_PLUGIN_NAME_BYTES: usize = 15;

/// The embedded `Cargo.toml`/`src/lib.rs` templates `plugin new` scaffolds,
/// mirroring how `init.rs` embeds its starter config template.
const PLUGIN_CARGO_TEMPLATE: &str = include_str!("../assets/plugin-cargo.toml.tmpl");
const PLUGIN_LIB_TEMPLATE: &str = include_str!("../assets/plugin-lib.rs.tmpl");

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
        PluginCmd::New(args) => new_plugin(&args),
        PluginCmd::Build(args) => {
            if let Err(error) = build_plugin(&args, plugin_dir) {
                eprintln!("plugin build failed: {error:#}");
                std::process::exit(1);
            }
        }
        PluginCmd::Run(args) => run_plugin(&args, config_path, plugin_dir),
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
    }
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
/// so a plugin author can exercise `render` without a live tmux pane.
fn sample_context() -> CoreContext {
    CoreContext {
        session_name: "0".into(),
        window_index: "1".into(),
        pane_index: "0".into(),
        pane_current_path: "/home/steve/src/rustline".into(),
        home: "/home/steve".into(),
        hostname: "devbox".into(),
        loadavg: Some([0.42, 0.37, 0.30]),
        now: Local::now(),
        window: Some(WindowCtx {
            index: "1".into(),
            name: "editor".into(),
            flags: "*".into(),
            is_current: true,
        }),
        interfaces: vec![NetIface {
            name: "eth0".into(),
            ipv4: "192.168.1.42".parse().expect("valid ipv4 literal"),
        }],
        battery: Some(Battery {
            percent: 76,
            state: BatteryState::Discharging,
        }),
        cpu: Some(CpuUsage { percent: 23.5 }),
        memory: Some(MemInfo {
            total_bytes: 16 * 1024 * 1024 * 1024,
            used_bytes: 6 * 1024 * 1024 * 1024,
            available_bytes: 10 * 1024 * 1024 * 1024,
        }),
        git: Some(GitInfo {
            branch: "main".into(),
            ahead: 1,
            behind: 0,
            staged: 1,
            unstaged: 2,
        }),
        disk: Some(DiskInfo {
            total_bytes: 512 * 1024 * 1024 * 1024,
            used_bytes: 200 * 1024 * 1024 * 1024,
            available_bytes: 300 * 1024 * 1024 * 1024,
        }),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        ..Default::default()
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

/// Validate a plugin name: non-empty, only `[A-Za-z0-9_-]` (so `/`, `\`,
/// `..`, spaces, and dots are all rejected without special-casing each), at
/// most 15 bytes (tmux's `range=user|X` byte cap — invariant #7, so the
/// scaffolded plugin stays click-toggleable), and not the reserved name
/// `window`. Pure and unit-tested so the scaffold command can reject a bad
/// name before touching disk.
fn validate_plugin_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("plugin name must not be empty".to_string());
    }
    if name == RESERVED_PLUGIN_NAME {
        return Err(format!("plugin name {RESERVED_PLUGIN_NAME:?} is reserved"));
    }
    if name.len() > MAX_PLUGIN_NAME_BYTES {
        return Err(format!(
            "plugin name {name:?} is {} bytes; must be at most {MAX_PLUGIN_NAME_BYTES} \
             (tmux's range=user|X limit)",
            name.len()
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!(
            "plugin name {name:?} may only contain letters, digits, `_`, and `-`"
        ));
    }
    Ok(())
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

/// `rustline plugin build <dir> [--release] [--plugin-dir <dir>]`: build any
/// WASM guest plugin crate at `<dir>` — not limited to this repo's own
/// `plugins/*`, the generic counterpart to `just build-plugin NAME` — and
/// install the resulting `.wasm` into `plugin_dir`, named after the crate's
/// own `[package].name` (hyphens intact, matching plugin discovery's
/// filename-stem convention). A missing wasm target or non-zero `cargo build`
/// exit surfaces as the process's own stderr output plus a clear error here;
/// a missing artifact afterward (e.g. a non-`cdylib` crate) is likewise a
/// clear error — never a panic.
fn build_plugin(args: &BuildArgs, plugin_dir: &Path) -> anyhow::Result<()> {
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
    std::fs::copy(&artifact, &dest).with_context(|| {
        format!(
            "failed to install {} to {}",
            artifact.display(),
            dest.display()
        )
    })?;

    println!("built and installed {name}.wasm -> {}", dest.display());
    Ok(())
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
