//! Plugin capability *manifests*: how a plugin declares the URLs and
//! filesystem paths it needs, so `rustline plugin approve` can turn that
//! declaration into an allowlist in one step. A manifest never grants anything
//! on its own — capabilities stay deny-by-default (N1) until the user approves;
//! approval only ever writes an allowlist entry, never ambient authority.
//!
//! Two sources, in precedence order:
//! 1. A sidecar `<plugin_dir>/<name>.toml` — **primary**; its presence
//!    supersedes any embedded manifest.
//! 2. An embedded `rustline-manifest` wasm custom section — the **fallback**
//!    used only when no sidecar exists.
//!
//! Both carry the same TOML shape ([`PluginManifest`]). A present-but-malformed
//! manifest from either source is logged and treated as absent (`None`) — a bad
//! manifest must never break plugin discovery or rendering (N2). A malformed
//! *sidecar* does **not** silently fall through to the embedded section: the
//! sidecar supersedes unconditionally, so a broken one is an error to surface,
//! not a reason to quietly use a different manifest than the file the user sees.

use std::path::Path;

use serde::Deserialize;

/// The wasm custom-section name carrying an embedded TOML manifest.
const MANIFEST_SECTION: &str = "rustline-manifest";

/// A plugin's declared capability requests.
///
/// `requested_urls`/`requested_paths` are exactly what `plugin approve` writes
/// into the plugin's `allowed_urls`/`allowed_paths` — verbatim, and nothing
/// more (per-plugin scope, deny-by-default). Every field is `#[serde(default)]`
/// so a minimal manifest (e.g. just `requested_urls = [...]`) still parses;
/// `name`/`version` are informational, shown by `approve`/`list`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PluginManifest {
    /// The plugin's own name (should match the `.wasm`/sidecar stem).
    #[serde(default)]
    pub name: String,
    /// A free-form version string, shown to the user on approval.
    #[serde(default)]
    pub version: String,
    /// URL allow-patterns the plugin asks the user to approve.
    #[serde(default)]
    pub requested_urls: Vec<String>,
    /// Filesystem-path allow-patterns the plugin asks the user to approve.
    #[serde(default)]
    pub requested_paths: Vec<String>,
}

/// Resolve a plugin's manifest: the sidecar `<plugin_dir>/<name>.toml` first
/// (it supersedes), else the embedded `rustline-manifest` custom section of
/// `<plugin_dir>/<name>.wasm`, else `None`.
///
/// A present-but-malformed manifest from either source is logged and returns
/// `None`; this never panics and never breaks discovery (N2). A malformed
/// sidecar returns `None` without falling back to the embedded section.
pub fn resolve_manifest(plugin_dir: &Path, name: &str) -> Option<PluginManifest> {
    let sidecar = plugin_dir.join(format!("{name}.toml"));
    if sidecar.exists() {
        // Sidecar supersedes unconditionally — a malformed one is surfaced as
        // absent, not papered over by the embedded fallback.
        return match std::fs::read_to_string(&sidecar) {
            Ok(text) => parse_manifest(&text, name, "sidecar"),
            Err(error) => {
                tracing::warn!(plugin = %name, %error, "unreadable manifest sidecar; ignoring");
                None
            }
        };
    }

    let wasm = std::fs::read(plugin_dir.join(format!("{name}.wasm"))).ok()?;
    let section = find_custom_section(&wasm, MANIFEST_SECTION)?;
    match std::str::from_utf8(section) {
        Ok(text) => parse_manifest(text, name, "embedded"),
        Err(_) => {
            tracing::warn!(plugin = %name, "embedded manifest is not valid UTF-8; ignoring");
            None
        }
    }
}

/// Parse manifest TOML, logging and swallowing any error into `None`.
fn parse_manifest(text: &str, name: &str, source: &str) -> Option<PluginManifest> {
    match toml::from_str::<PluginManifest>(text) {
        Ok(manifest) => Some(manifest),
        Err(error) => {
            tracing::warn!(plugin = %name, source, %error, "malformed plugin manifest; ignoring");
            None
        }
    }
}

/// Find a wasm custom section's payload by name, with no wasm-parsing
/// dependency. The module format is an 8-byte header (`\0asm` + a little-endian
/// u32 version) followed by sections `(id: u8, size: u32 LEB128, payload)`; a
/// custom section has id 0 and its payload is `(name_len: u32 LEB128, name,
/// data)`. Returns the `data` slice of the first section named `section_name`,
/// or `None` on any truncation/malformation — bounds are checked at every step,
/// so it never panics on adversarial bytes.
pub fn find_custom_section<'a>(wasm: &'a [u8], section_name: &str) -> Option<&'a [u8]> {
    const HEADER_LEN: usize = 8;
    if wasm.len() < HEADER_LEN || &wasm[..4] != b"\0asm" {
        return None;
    }
    let mut rest = &wasm[HEADER_LEN..];
    while let Some((&id, after_id)) = rest.split_first() {
        let (size, after_size) = read_leb_u32(after_id)?;
        let size = size as usize;
        if size > after_size.len() {
            return None; // truncated section body
        }
        let (payload, next) = after_size.split_at(size);
        if id == 0 {
            // Custom section: payload = (name_len LEB128, name, data).
            let (name_len, after_name_len) = read_leb_u32(payload)?;
            let name_len = name_len as usize;
            if name_len > after_name_len.len() {
                return None;
            }
            let (name, data) = after_name_len.split_at(name_len);
            if name == section_name.as_bytes() {
                return Some(data);
            }
        }
        rest = next;
    }
    None
}

/// Read an unsigned LEB128 `u32` from the front of `bytes`, returning the value
/// and the remaining slice. `None` on truncation or an overlong/overflowing
/// encoding (a `u32` needs at most 5 LEB128 bytes). A `u64` accumulator holds
/// the 35 possible value bits; `try_from` rejects a real overflow.
fn read_leb_u32(bytes: &[u8]) -> Option<(u32, &[u8])> {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    for (i, &byte) in bytes.iter().enumerate() {
        if i >= 5 {
            return None;
        }
        result |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return u32::try_from(result).ok().map(|v| (v, &bytes[i + 1..]));
        }
        shift += 7;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal wasm module: header + one custom section named
    /// `section` carrying `data`.
    fn wasm_with_custom_section(section: &str, data: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        write_leb(&mut body, section.len() as u32);
        body.extend_from_slice(section.as_bytes());
        body.extend_from_slice(data);

        let mut module = Vec::new();
        module.extend_from_slice(b"\0asm");
        module.extend_from_slice(&1u32.to_le_bytes());
        module.push(0); // custom section id
        write_leb(&mut module, body.len() as u32);
        module.extend_from_slice(&body);
        module
    }

    /// Minimal unsigned LEB128 encoder (test-only mirror of the decoder).
    fn write_leb(out: &mut Vec<u8>, mut v: u32) {
        loop {
            let mut byte = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if v == 0 {
                break;
            }
        }
    }

    #[test]
    fn finds_named_custom_section() {
        let wasm = wasm_with_custom_section(MANIFEST_SECTION, b"payload-bytes");
        assert_eq!(
            find_custom_section(&wasm, MANIFEST_SECTION),
            Some(&b"payload-bytes"[..])
        );
        assert_eq!(find_custom_section(&wasm, "other"), None);
    }

    #[test]
    fn skips_over_earlier_section_to_find_manifest() {
        // A non-custom section (id 1) then the manifest custom section: the walk
        // must step over the first and still find the second.
        let mut wasm = Vec::new();
        wasm.extend_from_slice(b"\0asm");
        wasm.extend_from_slice(&1u32.to_le_bytes());
        wasm.push(1); // some non-custom section id
        write_leb(&mut wasm, 3);
        wasm.extend_from_slice(&[0xaa, 0xbb, 0xcc]);
        let tail = wasm_with_custom_section(MANIFEST_SECTION, b"here");
        wasm.extend_from_slice(&tail[8..]); // append the custom section (drop its header)
        assert_eq!(
            find_custom_section(&wasm, MANIFEST_SECTION),
            Some(&b"here"[..])
        );
    }

    #[test]
    fn rejects_non_wasm_bytes() {
        assert_eq!(find_custom_section(b"", MANIFEST_SECTION), None);
        assert_eq!(
            find_custom_section(b"not a wasm module", MANIFEST_SECTION),
            None
        );
        // Valid magic but a truncated section length must not panic.
        let mut truncated = Vec::new();
        truncated.extend_from_slice(b"\0asm");
        truncated.extend_from_slice(&1u32.to_le_bytes());
        truncated.push(0);
        truncated.push(0x80); // LEB128 continuation with no follow-up byte
        assert_eq!(find_custom_section(&truncated, MANIFEST_SECTION), None);
    }

    #[test]
    fn sidecar_parses_full_manifest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("w.toml"),
            "name = \"w\"\nversion = \"1.2.3\"\nrequested_urls = [\"https://a/*\"]\nrequested_paths = [\"/tmp/x\"]\n",
        )
        .unwrap();
        let m = resolve_manifest(dir.path(), "w").unwrap();
        assert_eq!(
            m,
            PluginManifest {
                name: "w".into(),
                version: "1.2.3".into(),
                requested_urls: vec!["https://a/*".into()],
                requested_paths: vec!["/tmp/x".into()],
            }
        );
    }

    #[test]
    fn minimal_sidecar_needs_only_a_request_list() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("w.toml"),
            "requested_urls = [\"https://a/*\"]\n",
        )
        .unwrap();
        let m = resolve_manifest(dir.path(), "w").unwrap();
        assert!(m.name.is_empty() && m.version.is_empty());
        assert_eq!(m.requested_urls, vec!["https://a/*".to_string()]);
        assert!(m.requested_paths.is_empty());
    }

    #[test]
    fn embedded_manifest_used_when_no_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let wasm = wasm_with_custom_section(
            MANIFEST_SECTION,
            b"name = \"w\"\nrequested_urls = [\"https://embedded/*\"]\n",
        );
        std::fs::write(dir.path().join("w.wasm"), &wasm).unwrap();
        let m = resolve_manifest(dir.path(), "w").unwrap();
        assert_eq!(m.requested_urls, vec!["https://embedded/*".to_string()]);
    }

    #[test]
    fn sidecar_supersedes_embedded_when_both_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("w.toml"),
            "requested_urls = [\"https://sidecar/*\"]\n",
        )
        .unwrap();
        let wasm = wasm_with_custom_section(
            MANIFEST_SECTION,
            b"requested_urls = [\"https://embedded/*\"]\n",
        );
        std::fs::write(dir.path().join("w.wasm"), &wasm).unwrap();
        let m = resolve_manifest(dir.path(), "w").unwrap();
        assert_eq!(m.requested_urls, vec!["https://sidecar/*".to_string()]);
    }

    #[test]
    fn neither_source_present_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_manifest(dir.path(), "missing").is_none());
    }

    #[test]
    fn malformed_sidecar_is_none_and_does_not_fall_back() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("w.toml"), "this is not = valid = toml").unwrap();
        // A valid embedded manifest exists, but the malformed sidecar supersedes
        // and must yield None rather than silently using the embedded one.
        let wasm =
            wasm_with_custom_section(MANIFEST_SECTION, b"requested_urls = [\"https://x/*\"]\n");
        std::fs::write(dir.path().join("w.wasm"), &wasm).unwrap();
        assert!(resolve_manifest(dir.path(), "w").is_none());
    }

    #[test]
    fn wasm_without_manifest_section_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let wasm = wasm_with_custom_section("unrelated", b"whatever");
        std::fs::write(dir.path().join("w.wasm"), &wasm).unwrap();
        assert!(resolve_manifest(dir.path(), "w").is_none());
    }

    #[test]
    fn malformed_embedded_manifest_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let wasm = wasm_with_custom_section(MANIFEST_SECTION, b"not = valid = toml");
        std::fs::write(dir.path().join("w.wasm"), &wasm).unwrap();
        assert!(resolve_manifest(dir.path(), "w").is_none());
    }

    #[test]
    fn leb128_roundtrips_multibyte_values() {
        for v in [0u32, 1, 127, 128, 300, 16_384, u32::MAX] {
            let mut buf = Vec::new();
            write_leb(&mut buf, v);
            buf.push(0x99); // trailing sentinel byte the reader must leave alone
            let (got, rest) = read_leb_u32(&buf).unwrap();
            assert_eq!(got, v);
            assert_eq!(rest, &[0x99]);
        }
    }

    #[test]
    fn leb128_rejects_overlong_u32() {
        // Six continuation bytes cannot encode a u32.
        assert!(read_leb_u32(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x00]).is_none());
    }
}
