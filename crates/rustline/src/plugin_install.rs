//! `rustline plugin install|update|remove <owner/repo>` (W38): download a
//! plugin's `.wasm` from its GitHub release into the plugin dir and record
//! provenance (`source`/`tag`/`checksum`), **without granting any
//! capabilities** — the runtime allowlists plus `plugin approve` remain the
//! sole authority on what a plugin may do.
//!
//! The download uses a dedicated [`Downloader`] seam rather than
//! `rustline-wasm`'s `Fetcher`: that one returns a `String` body and follows
//! *no* redirects (it gates the exact requested URL), so it can't fetch
//! GitHub's redirecting, binary release assets. [`UreqDownloader`] is the real
//! rustls client — it follows redirects for the asset bytes and sends the
//! `User-Agent` the GitHub API requires. The trait lets the install/update
//! flows be unit-tested with a fake, no network involved.

use std::fmt::Write as _;
use std::io::Read as _;
use std::path::Path;

use anyhow::{Context as _, anyhow, bail};
use rustline_core::{Config, PluginSource};
use serde_json::Value;
use sha2::{Digest, Sha256};
use toml_edit::{DocumentMut, Item, Table, value};

use crate::cli::{InstallArgs, RemoveArgs, UpdateArgs};
use crate::plugin_cmd::{MAX_PLUGIN_NAME_BYTES, RESERVED_PLUGIN_NAME};

/// A minimal HTTP GET seam over the two shapes `plugin install` needs: the
/// release JSON from the GitHub API, and the raw asset bytes (following
/// redirects). Split from `rustline-wasm`'s capability-gated `Fetcher` because
/// this one intentionally follows redirects and returns bytes.
pub trait Downloader {
    /// GET `url` and parse the response body as JSON.
    fn get_json(&self, url: &str) -> anyhow::Result<Value>;
    /// GET `url`, following redirects, and return the raw response body.
    fn get_bytes(&self, url: &str) -> anyhow::Result<Vec<u8>>;
}

/// `User-Agent` for GitHub API requests — the API rejects requests without one.
const USER_AGENT: &str = concat!("rustline/", env!("CARGO_PKG_VERSION"));

/// The real downloader: a blocking rustls `ureq` client that follows redirects
/// (GitHub's `browser_download_url` 302-redirects to a CDN host).
pub struct UreqDownloader;

impl UreqDownloader {
    /// An agent that follows up to five redirects — the default, made explicit
    /// because binary asset downloads *depend* on redirect-following (the
    /// opposite of `rustline-wasm`'s `redirects(0)` gate).
    fn agent() -> ureq::Agent {
        ureq::AgentBuilder::new().redirects(5).build()
    }
}

impl Downloader for UreqDownloader {
    fn get_json(&self, url: &str) -> anyhow::Result<Value> {
        Self::agent()
            .get(url)
            .set("User-Agent", USER_AGENT)
            .set("Accept", "application/vnd.github+json")
            .call()
            .with_context(|| format!("GET {url}"))?
            .into_json()
            .with_context(|| format!("parse JSON from {url}"))
    }

    fn get_bytes(&self, url: &str) -> anyhow::Result<Vec<u8>> {
        let resp = Self::agent()
            .get(url)
            .set("User-Agent", USER_AGENT)
            .call()
            .with_context(|| format!("GET {url}"))?;
        let mut bytes = Vec::new();
        resp.into_reader()
            .read_to_end(&mut bytes)
            .with_context(|| format!("read body from {url}"))?;
        Ok(bytes)
    }
}

/// Parse an `owner/repo` slug into its parts. Both must be non-empty and
/// contain only the characters GitHub allows in a slug (`[A-Za-z0-9._-]`), so
/// the value is safe to interpolate into the release-API URL; anything else
/// (no slash, an extra slash, empty part, stray character) is `None`.
pub fn parse_owner_repo(spec: &str) -> Option<(String, String)> {
    let (owner, repo) = spec.split_once('/')?;
    if !is_slug(owner) || !is_slug(repo) {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

/// A single GitHub owner or repo slug segment: non-empty, `[A-Za-z0-9._-]`.
fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Pick the first `.wasm` release asset's `(name, browser_download_url)` from a
/// GitHub release JSON object, or `None` if the release ships no `.wasm`.
pub fn select_wasm_asset(release: &Value) -> Option<(String, String)> {
    release.get("assets")?.as_array()?.iter().find_map(|asset| {
        let name = asset.get("name")?.as_str()?;
        if !name.ends_with(".wasm") {
            return None;
        }
        let url = asset.get("browser_download_url")?.as_str()?;
        Some((name.to_string(), url.to_string()))
    })
}

/// Lowercase hex sha256 of `bytes` (64 chars) — the `checksum` recorded for an
/// installed plugin.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().fold(String::with_capacity(64), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// The GitHub release-API URL for `owner/repo`: the latest release, or a
/// specific tag when `tag` is `Some`.
fn release_api_url(owner: &str, repo: &str, tag: Option<&str>) -> String {
    match tag {
        Some(t) => format!("https://api.github.com/repos/{owner}/{repo}/releases/tags/{t}"),
        None => format!("https://api.github.com/repos/{owner}/{repo}/releases/latest"),
    }
}

/// Validate an installed plugin's name (its `.wasm` stem and config key):
/// non-empty, not the reserved `window`, and `[A-Za-z0-9_-]` only, so it is a
/// safe filename and TOML key. Unlike a *scaffolded* plugin's name, the 15-byte
/// tmux `range=user|X` cap is NOT a hard error here — a longer plugin simply
/// isn't click-toggleable (`register_plugins` warns and registers it anyway),
/// so `install` warns rather than refusing. Returns `Ok(clickable)` where
/// `clickable` is whether the name fits the range cap.
fn validate_install_name(name: &str) -> Result<bool, String> {
    if name.is_empty() {
        return Err("plugin name must not be empty".to_string());
    }
    if name == RESERVED_PLUGIN_NAME {
        return Err(format!("plugin name {RESERVED_PLUGIN_NAME:?} is reserved"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!(
            "plugin name {name:?} may only contain letters, digits, `_`, and `-`"
        ));
    }
    Ok(name.len() <= MAX_PLUGIN_NAME_BYTES)
}

/// Resolve a release, download its `.wasm`, install it into `plugin_dir`, and
/// record `[plugins.<name>]` (`source`/`tag`/`checksum`) in the config. Generic
/// over [`Downloader`] so tests drive it with a fake — no network. Returns the
/// installed plugin name.
///
/// Records provenance ONLY: it never writes any `allowed_urls`/`allowed_paths`,
/// so an installed plugin starts with zero capabilities (deny-by-default holds
/// until the user runs `plugin approve` or hand-edits an allowlist).
fn do_install<D: Downloader>(
    dl: &D,
    repo_spec: &str,
    name_override: Option<&str>,
    tag: Option<&str>,
    plugin_dir: &Path,
    config_path: &Path,
) -> anyhow::Result<String> {
    let (owner, repo) = parse_owner_repo(repo_spec)
        .ok_or_else(|| anyhow!("invalid owner/repo {repo_spec:?}; expected \"owner/repo\""))?;

    let name = name_override.map_or_else(|| repo.clone(), str::to_string);
    let clickable =
        validate_install_name(&name).map_err(|e| anyhow!("invalid plugin name: {e}"))?;
    if !clickable {
        tracing::warn!(
            "plugin name {name:?} is {} bytes (> {MAX_PLUGIN_NAME_BYTES}); it will install \
             but won't be click-toggleable — pass --name to shorten it",
            name.len()
        );
    }

    let url = release_api_url(&owner, &repo, tag);
    let release = dl
        .get_json(&url)
        .with_context(|| format!("fetch release metadata for {owner}/{repo}"))?;
    let (asset_name, asset_url) = select_wasm_asset(&release)
        .ok_or_else(|| anyhow!("no .wasm asset in the release for {owner}/{repo}"))?;
    let bytes = dl
        .get_bytes(&asset_url)
        .with_context(|| format!("download asset {asset_name}"))?;
    let checksum = sha256_hex(&bytes);

    // Prefer the release's own `tag_name`; fall back to a `--tag` the caller
    // pinned (the `latest` endpoint always carries `tag_name`, so this only
    // matters for an unusual response).
    let resolved_tag = release
        .get("tag_name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| tag.map(str::to_string));

    std::fs::create_dir_all(plugin_dir)
        .with_context(|| format!("create plugin dir {}", plugin_dir.display()))?;
    let wasm_path = plugin_dir.join(format!("{name}.wasm"));
    std::fs::write(&wasm_path, &bytes).with_context(|| format!("write {}", wasm_path.display()))?;

    write_install_record(
        config_path,
        &name,
        repo_spec,
        resolved_tag.as_deref(),
        &checksum,
    )
    .with_context(|| format!("record [plugins.{name}] in {}", config_path.display()))?;

    Ok(name)
}

/// Read `config_path` as a format-preserving `toml_edit` document, treating a
/// genuinely missing file as a fresh empty document but a syntax error / read
/// error as a hard failure — so an install never truncates an existing config
/// it merely failed to parse (the same care as `plugin_cmd::load_doc`, but
/// returning `Result` instead of exiting, so it's testable).
fn read_doc(config_path: &Path) -> anyhow::Result<DocumentMut> {
    match std::fs::read_to_string(config_path) {
        Ok(text) => text
            .parse::<DocumentMut>()
            .with_context(|| format!("{} is not valid TOML", config_path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(e) => Err(anyhow!("cannot read {}: {e}", config_path.display())),
    }
}

/// Get `[plugins.<name>]` as a mutable table, creating `plugins`/`plugins.<name>`
/// if absent and erroring if either is present but not a table.
fn plugin_table<'a>(doc: &'a mut DocumentMut, name: &str) -> anyhow::Result<&'a mut Table> {
    let plugins = doc
        .entry("plugins")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| anyhow!("`plugins` is not a table"))?;
    plugins.set_implicit(true);
    plugins
        .entry(name)
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| anyhow!("`plugins.{name}` is not a table"))
}

/// Write `source`/`tag`/`checksum` into `[plugins.<name>]`, preserving the rest
/// of the config's comments/formatting and any pre-existing keys (e.g. an
/// allowlist the user already granted). `source` is written as a bare
/// `owner/repo` string (the `OwnerRepo` wire form). A `None` tag removes any
/// stale `tag` key rather than leaving a wrong value behind.
fn write_install_record(
    config_path: &Path,
    name: &str,
    source: &str,
    tag: Option<&str>,
    checksum: &str,
) -> anyhow::Result<()> {
    let mut doc = read_doc(config_path)?;
    let table = plugin_table(&mut doc, name)?;
    table["source"] = value(source);
    match tag {
        Some(t) => table["tag"] = value(t),
        None => {
            table.remove("tag");
        }
    }
    table["checksum"] = value(checksum);
    std::fs::write(config_path, doc.to_string())
        .with_context(|| format!("write config {}", config_path.display()))?;
    Ok(())
}

/// Remove `[plugins.<name>]` from the config, if present. Leaves the rest of
/// the document untouched.
fn remove_config_entry(config_path: &Path, name: &str) -> anyhow::Result<()> {
    let mut doc = read_doc(config_path)?;
    if let Some(plugins) = doc.get_mut("plugins").and_then(Item::as_table_mut) {
        plugins.remove(name);
    }
    std::fs::write(config_path, doc.to_string())
        .with_context(|| format!("write config {}", config_path.display()))?;
    Ok(())
}

/// Re-resolve the *latest* release for a plugin's recorded `owner/repo`
/// `source`, re-download, and rewrite its `tag`/`checksum`. Only owner/repo
/// sources can update (a hand-installed plugin, or a URL/path source, has no
/// `latest` to re-resolve). Generic over [`Downloader`] for testability.
fn do_update<D: Downloader>(
    dl: &D,
    name: &str,
    plugin_dir: &Path,
    config_path: &Path,
) -> anyhow::Result<()> {
    let cfg = Config::load(config_path);
    let pc = cfg
        .plugins
        .get(name)
        .ok_or_else(|| anyhow!("no configured plugin named {name:?}"))?;
    let repo_spec = match pc.source.as_ref() {
        Some(PluginSource::OwnerRepo(s)) => s.clone(),
        Some(other) => bail!("plugin {name:?} source ({other}) is not an owner/repo; can't update"),
        None => bail!("plugin {name:?} has no recorded source; can't update"),
    };
    do_install(dl, &repo_spec, Some(name), None, plugin_dir, config_path)?;
    Ok(())
}

/// Print the post-install note: the plugin is installed but powerless until the
/// user grants capabilities explicitly. This is the whole security posture of
/// `plugin install` — download ≠ trust.
fn print_no_capabilities(name: &str) {
    println!();
    println!("No capabilities were granted. {name} cannot reach the network or the");
    println!("filesystem until you grant it explicitly:");
    println!();
    println!("  rustline plugin approve {name}          # if it ships a capability manifest");
    println!("  rustline plugin url add {name} <glob>   # or grant a URL/path by hand");
    println!("  rustline plugin path add {name} <glob>");
}

/// `rustline plugin install <owner/repo> [--name] [--tag] [--plugin-dir]`.
pub fn install(args: &InstallArgs, config_path: &Path, plugin_dir: &Path) {
    match do_install(
        &UreqDownloader,
        &args.repo,
        args.name.as_deref(),
        args.tag.as_deref(),
        plugin_dir,
        config_path,
    ) {
        Ok(name) => {
            println!(
                "installed {name}.wasm -> {}",
                plugin_dir.join(format!("{name}.wasm")).display()
            );
            print_no_capabilities(&name);
        }
        Err(e) => {
            eprintln!("plugin install failed: {e:#}");
            std::process::exit(1);
        }
    }
}

/// `rustline plugin update <name> [--plugin-dir]`.
pub fn update(args: &UpdateArgs, config_path: &Path, plugin_dir: &Path) {
    match do_update(&UreqDownloader, &args.name, plugin_dir, config_path) {
        Ok(()) => println!("updated {}", args.name),
        Err(e) => {
            eprintln!("plugin update failed: {e:#}");
            std::process::exit(1);
        }
    }
}

/// `rustline plugin remove <name> [--yes] [--plugin-dir]`: delete the installed
/// `.wasm`; with `--yes` (or an interactive confirm) also drop the
/// `[plugins.<name>]` config entry.
pub fn remove(args: &RemoveArgs, config_path: &Path, plugin_dir: &Path) {
    let wasm_path = plugin_dir.join(format!("{}.wasm", args.name));
    match std::fs::remove_file(&wasm_path) {
        Ok(()) => println!("removed {}", wasm_path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("no installed {} (nothing to remove)", wasm_path.display());
        }
        Err(e) => {
            eprintln!("failed to remove {}: {e}", wasm_path.display());
            std::process::exit(1);
        }
    }

    let drop_config = args.yes || confirm_drop_config(&args.name);
    if !drop_config {
        println!("left [plugins.{}] in the config", args.name);
        return;
    }
    if let Err(e) = remove_config_entry(config_path, &args.name) {
        eprintln!("failed to update config: {e:#}");
        std::process::exit(1);
    }
    println!("removed [plugins.{}] from the config", args.name);
}

/// Interactive y/N confirmation for dropping the config entry. Defaults to No
/// on EOF / read error / any non-`y` reply, so a non-interactive `remove`
/// without `--yes` keeps the config entry rather than silently deleting it.
fn confirm_drop_config(name: &str) -> bool {
    use std::io::Write as _;
    print!("Also remove [plugins.{name}] from the config? [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A realistic GitHub release JSON: two assets, only one of which is a
    /// `.wasm`, plus the surrounding fields a real response carries.
    const SAMPLE_RELEASE_JSON: &str = r#"{
      "tag_name": "v1.2.0",
      "name": "Weather v1.2.0",
      "draft": false,
      "prerelease": false,
      "assets": [
        {
          "name": "checksums.txt",
          "browser_download_url": "https://github.com/steve/rustline-weather/releases/download/v1.2.0/checksums.txt"
        },
        {
          "name": "weather.wasm",
          "browser_download_url": "https://github.com/steve/rustline-weather/releases/download/v1.2.0/weather.wasm"
        }
      ]
    }"#;

    /// A fake [`Downloader`] returning canned JSON + bytes — no network.
    struct FakeDownloader {
        json: Value,
        bytes: Vec<u8>,
    }

    impl Downloader for FakeDownloader {
        fn get_json(&self, _url: &str) -> anyhow::Result<Value> {
            Ok(self.json.clone())
        }
        fn get_bytes(&self, _url: &str) -> anyhow::Result<Vec<u8>> {
            Ok(self.bytes.clone())
        }
    }

    fn fake(bytes: &[u8]) -> FakeDownloader {
        FakeDownloader {
            json: serde_json::from_str(SAMPLE_RELEASE_JSON).unwrap(),
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn parses_owner_repo() {
        assert_eq!(
            parse_owner_repo("steve/rustline-weather"),
            Some(("steve".into(), "rustline-weather".into()))
        );
        assert_eq!(parse_owner_repo("nope"), None);
        assert_eq!(parse_owner_repo("a/b/c"), None); // extra slash
        assert_eq!(parse_owner_repo("/repo"), None); // empty owner
        assert_eq!(parse_owner_repo("owner/"), None); // empty repo
        assert_eq!(parse_owner_repo("bad owner/repo"), None); // stray space
    }

    #[test]
    fn selects_wasm_asset_from_release_json() {
        let json: Value = serde_json::from_str(SAMPLE_RELEASE_JSON).unwrap();
        let (name, url) = select_wasm_asset(&json).unwrap();
        assert!(name.ends_with(".wasm"), "{name}");
        assert!(url.starts_with("https://"), "{url}");
        assert_eq!(name, "weather.wasm");
    }

    #[test]
    fn selects_none_when_no_wasm_asset() {
        let json: Value = serde_json::json!({
            "tag_name": "v1",
            "assets": [{ "name": "readme.txt", "browser_download_url": "https://x/readme.txt" }]
        });
        assert_eq!(select_wasm_asset(&json), None);
    }

    #[test]
    fn sha256_is_stable() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn release_api_url_latest_vs_tag() {
        assert_eq!(
            release_api_url("steve", "weather", None),
            "https://api.github.com/repos/steve/weather/releases/latest"
        );
        assert_eq!(
            release_api_url("steve", "weather", Some("v1.2.0")),
            "https://api.github.com/repos/steve/weather/releases/tags/v1.2.0"
        );
    }

    #[test]
    fn validate_install_name_allows_long_but_flags_unclickable() {
        assert_eq!(validate_install_name("weather"), Ok(true));
        // 16 bytes: installable, but not click-toggleable.
        assert_eq!(validate_install_name("rustline-weather"), Ok(false));
        assert!(validate_install_name("").is_err());
        assert!(validate_install_name("window").is_err());
        assert!(validate_install_name("has/slash").is_err());
    }

    #[test]
    fn install_writes_source_tag_checksum() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_dir = tmp.path().join("plugins");
        let config_path = tmp.path().join("config.toml");
        let bytes = b"\x00asm-fake-wasm-payload".to_vec();

        let name = do_install(
            &fake(&bytes),
            "steve/rustline-weather",
            Some("weather"),
            None,
            &plugin_dir,
            &config_path,
        )
        .unwrap();
        assert_eq!(name, "weather");

        // The .wasm landed in the plugin dir, byte-for-byte.
        let installed = std::fs::read(plugin_dir.join("weather.wasm")).unwrap();
        assert_eq!(installed, bytes);

        // The config recorded source/tag/checksum, and re-parses through serde
        // as an OwnerRepo (proving the bare-string write is the wire form).
        let text = std::fs::read_to_string(&config_path).unwrap();
        let cfg: Config = toml::from_str(&text).unwrap();
        let pc = cfg.plugins.get("weather").unwrap();
        assert_eq!(
            pc.source,
            Some(PluginSource::OwnerRepo("steve/rustline-weather".into()))
        );
        assert_eq!(pc.tag.as_deref(), Some("v1.2.0"));
        assert_eq!(pc.checksum.as_deref(), Some(sha256_hex(&bytes).as_str()));

        // Install grants nothing: no allowlist entries appear.
        assert!(pc.allowed_urls.is_empty());
        assert!(pc.allowed_paths.is_empty());
    }

    #[test]
    fn install_default_name_is_repo_and_preserves_existing_config() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_dir = tmp.path().join("plugins");
        let config_path = tmp.path().join("config.toml");
        std::fs::write(&config_path, "# my config\n[layout]\nleft = [\"cwd\"]\n").unwrap();

        let name = do_install(
            &fake(b"bytes"),
            "steve/rustline-weather",
            None, // default name = repo
            None,
            &plugin_dir,
            &config_path,
        )
        .unwrap();
        assert_eq!(name, "rustline-weather");

        let text = std::fs::read_to_string(&config_path).unwrap();
        assert!(text.contains("# my config"), "comment preserved: {text}");
        assert!(
            text.contains("left = [\"cwd\"]"),
            "layout preserved: {text}"
        );
        let cfg: Config = toml::from_str(&text).unwrap();
        assert!(cfg.plugins.contains_key("rustline-weather"));
    }

    #[test]
    fn update_re_resolves_recorded_owner_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_dir = tmp.path().join("plugins");
        let config_path = tmp.path().join("config.toml");
        // A prior install recorded an OwnerRepo source with an older tag.
        std::fs::write(
            &config_path,
            "[plugins.weather]\nsource = \"steve/rustline-weather\"\ntag = \"v1.0.0\"\nchecksum = \"old\"\n",
        )
        .unwrap();

        let bytes = b"new-payload".to_vec();
        do_update(&fake(&bytes), "weather", &plugin_dir, &config_path).unwrap();

        let cfg: Config = toml::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        let pc = cfg.plugins.get("weather").unwrap();
        assert_eq!(pc.tag.as_deref(), Some("v1.2.0")); // re-resolved to the release's tag
        assert_eq!(pc.checksum.as_deref(), Some(sha256_hex(&bytes).as_str()));
        assert_eq!(
            std::fs::read(plugin_dir.join("weather.wasm")).unwrap(),
            bytes
        );
    }

    #[test]
    fn update_refuses_without_owner_repo_source() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_dir = tmp.path().join("plugins");
        let config_path = tmp.path().join("config.toml");
        std::fs::write(&config_path, "[plugins.weather]\nallowed_urls = []\n").unwrap();
        // No source recorded -> can't update.
        let err = do_update(&fake(b"x"), "weather", &plugin_dir, &config_path).unwrap_err();
        assert!(err.to_string().contains("no recorded source"), "{err}");
    }

    #[test]
    fn remove_config_entry_drops_only_that_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.toml");
        std::fs::write(
            &config_path,
            "[plugins.weather]\nsource = \"o/r\"\n[plugins.counter]\nsource = \"o/c\"\n",
        )
        .unwrap();

        remove_config_entry(&config_path, "weather").unwrap();

        let cfg: Config = toml::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert!(!cfg.plugins.contains_key("weather"));
        assert!(cfg.plugins.contains_key("counter"));
    }

    #[test]
    fn read_doc_errors_on_invalid_toml_rather_than_truncating() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.toml");
        std::fs::write(&config_path, "not = = valid [[[").unwrap();
        assert!(read_doc(&config_path).is_err());
    }
}
