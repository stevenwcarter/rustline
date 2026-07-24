//! `rustline widget …`: list, enable, disable, and reorder the widgets in the
//! `[layout]` arrays, editing `config.toml` in place with `toml_edit` so
//! comments and formatting survive — the same approach `plugin_cmd.rs` uses
//! for allowlists and `theme_cmd.rs` for `[theme].base`.
//!
//! Every mutation goes through `rustline-core`'s pure layout algebra
//! (`layout_enable`/`layout_disable`/`layout_move`), so the CLI and the TUI
//! (`widget_tui.rs`) share one definition of a legal edit. A refused edit
//! writes nothing at all and exits non-zero.

use std::io::Write;
use std::path::Path;

use rustline_core::{
    Config, Layout, LayoutChange, Region, WidgetPlacement, WidgetSource, layout_disable,
    layout_enable, layout_move, widget_placements,
};
use rustline_wasm::discover_plugin_names;
use toml_edit::{Array, DocumentMut, Item, Table, value};

use crate::cli::WidgetCmd;

/// Read the layout as it exists in the *file*. A missing `[layout]` table, or
/// a missing array within it, falls back to `Config::load`'s default for that
/// region — so an edit against a zero-config install writes three complete,
/// correct arrays rather than orphaning the two the user never mentioned.
///
/// `[layout]` may be written as a standard table (`[layout]` header) or as an
/// inline table (`layout = { left = [...], right = [...] }`) — both are
/// valid config that `Config::load` parses identically, so both are read
/// here via `as_table_like`, which treats the two alike. A `layout` key
/// holding neither (a scalar, e.g. `layout = "oops"`) is simply not
/// table-like, so `table` is `None` and every region falls back to its
/// default, matching `Config::load`'s own total fallback.
pub(crate) fn read_layout(doc: &DocumentMut) -> Layout {
    let defaults = Layout::default();
    let table = doc.get("layout").and_then(Item::as_table_like);
    let region = |key: &str, fallback: &[String]| -> Vec<String> {
        table
            .and_then(|t| t.get(key))
            .and_then(Item::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_else(|| fallback.to_vec())
    };
    Layout {
        left: region("left", &defaults.left),
        center: region("center", &defaults.center),
        right: region("right", &defaults.right),
    }
}

/// Write all three arrays back into `[layout]`, creating the table if absent.
/// Only the three arrays are touched; everything else in the document —
/// comments, ordering, other tables — is left exactly as it was.
///
/// Mirrors [`read_layout`]'s table-like handling: an existing standard table
/// OR inline table is written into via `as_table_like_mut` alike. A `layout`
/// key holding a genuinely non-table value (e.g. `layout = "oops"` or
/// `layout = 42`) is refused with `Err` — nothing is mutated — rather than
/// panicking, upholding the "a refused edit writes nothing" contract; the
/// caller must check this before touching disk.
pub(crate) fn write_layout(doc: &mut DocumentMut, layout: &Layout) -> Result<(), String> {
    let item = doc
        .entry("layout")
        .or_insert_with(|| Item::Table(Table::new()));
    let table = item.as_table_like_mut().ok_or_else(|| {
        "`layout` in the config file is not a table (found a scalar or array); \
         refusing to edit it"
            .to_string()
    })?;
    for region in Region::ALL {
        let mut arr = Array::new();
        for name in layout.get(region) {
            arr.push(name.as_str());
        }
        table.insert(region.as_str(), value(arr));
    }
    Ok(())
}

/// Every name a layout entry may legally take: registered built-ins,
/// non-colliding `[instances.<name>]` entries, and discovered plugin stems.
fn known_names(cfg: &Config, plugin_dir: &Path) -> Vec<String> {
    let registry = rustline_core::Registry::with_builtins(cfg);
    let plugins = discover_plugin_names(plugin_dir);
    widget_placements(cfg, registry.descriptors(), &plugins)
        .into_iter()
        .map(|p| p.name)
        .collect()
}

/// Accept `name` iff it is a known widget; otherwise an error message listing
/// what *is* available — the same shape as `theme use`'s unknown-name error.
fn resolve_name(name: &str, known: &[String]) -> Result<(), String> {
    if known.iter().any(|k| k == name) {
        return Ok(());
    }
    Err(format!(
        "unknown widget `{name}`\navailable: {}",
        known.join(", ")
    ))
}

/// Human line for a completed edit.
fn describe(change: &LayoutChange) -> String {
    match (change.from, change.to) {
        (None, Some((r, i))) => format!("enabled {} in {}[{i}]", change.name, r.as_str()),
        (Some((r, _)), None) => format!("disabled {} (was in {})", change.name, r.as_str()),
        (Some((fr, _)), Some((tr, ti))) => format!(
            "moved {} from {} to {}[{ti}]",
            change.name,
            fr.as_str(),
            tr.as_str()
        ),
        (None, None) => format!("{}: no change", change.name),
    }
}

/// Ask tmux to redraw the status line, so an edit is visible immediately.
/// Best-effort: outside tmux, or if the spawn fails, this is a silent no-op —
/// a failed refresh must never turn a successful config write into an error.
fn refresh_tmux() {
    if std::env::var_os("TMUX").is_none() {
        return;
    }
    if let Err(error) = std::process::Command::new("tmux")
        .args(["refresh-client", "-S"])
        .status()
    {
        tracing::warn!(%error, "tmux refresh-client failed");
    }
}

/// Dispatch a `rustline widget …` invocation. Returns the process exit code:
/// `0` on success, `1` on a refused edit (which writes nothing).
pub fn run(cmd: WidgetCmd, config_path: &Path, plugin_dir: &Path) -> i32 {
    match cmd {
        WidgetCmd::List { json } => {
            list(config_path, plugin_dir, json);
            0
        }
        WidgetCmd::Enable {
            name,
            region,
            index,
        } => {
            let region = match Region::parse(&region) {
                Some(r) => r,
                None => {
                    eprintln!("unknown region `{region}` (expected left, center, or right)");
                    return 1;
                }
            };
            mutate(config_path, plugin_dir, &name, |layout| {
                layout_enable(layout, &name, region, index)
            })
        }
        WidgetCmd::Disable { name } => mutate(config_path, plugin_dir, &name, |layout| {
            layout_disable(layout, &name)
        }),
        WidgetCmd::Move {
            name,
            region,
            index,
        } => {
            let region = match Region::parse(&region) {
                Some(r) => r,
                None => {
                    eprintln!("unknown region `{region}` (expected left, center, or right)");
                    return 1;
                }
            };
            mutate(config_path, plugin_dir, &name, |layout| {
                layout_move(layout, &name, region, index.unwrap_or(usize::MAX))
            })
        }
        WidgetCmd::Edit => crate::widget_tui::run(config_path, plugin_dir),
    }
}

/// Load → validate the name → apply `edit` → write + report. Any failure
/// short-circuits before the write, so `config.toml` is untouched.
fn mutate(
    config_path: &Path,
    plugin_dir: &Path,
    name: &str,
    edit: impl FnOnce(&mut Layout) -> Result<LayoutChange, rustline_core::LayoutEditError>,
) -> i32 {
    let cfg = Config::load(config_path);
    let known = known_names(&cfg, plugin_dir);
    if let Err(message) = resolve_name(name, &known) {
        eprintln!("{message}");
        return 1;
    }
    let text = std::fs::read_to_string(config_path).unwrap_or_default();
    let mut doc = match text.parse::<DocumentMut>() {
        Ok(doc) => doc,
        Err(error) => {
            eprintln!("config file is not valid TOML; refusing to edit it: {error}");
            return 1;
        }
    };
    let mut layout = read_layout(&doc);
    let change = match edit(&mut layout) {
        Ok(change) => change,
        Err(error) => {
            eprintln!("{name}: {error}");
            return 1;
        }
    };
    if let Err(error) = write_layout(&mut doc, &layout) {
        eprintln!("{error}");
        return 1;
    }
    if let Some(parent) = config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = std::fs::write(config_path, doc.to_string()) {
        eprintln!("failed to write {}: {error}", config_path.display());
        return 1;
    }
    println!("{}", describe(&change));
    refresh_tmux();
    0
}

/// `widget list`: every widget, marked with where it sits.
fn list(config_path: &Path, plugin_dir: &Path, json: bool) {
    let cfg = Config::load(config_path);
    let registry = rustline_core::Registry::with_builtins(&cfg);
    let plugins = discover_plugin_names(plugin_dir);
    let rows = widget_placements(&cfg, registry.descriptors(), &plugins);
    if json {
        println!("{}", placements_json(&rows));
        return;
    }
    let mut out = std::io::stdout().lock();
    for row in &rows {
        let placed = match row.placement {
            Some((r, i)) => format!("{}[{i}]", r.as_str()),
            None => "-".to_string(),
        };
        let source = match &row.source {
            WidgetSource::Builtin => "builtin".to_string(),
            WidgetSource::Plugin => "plugin".to_string(),
            WidgetSource::Instance { kind } => format!("instance of {kind}"),
        };
        let _ = writeln!(
            out,
            "{placed:<10} {:<16} {} ({source})",
            row.name, row.summary
        );
    }
}

/// `widget list --json`, matching W40's convention on the other list surfaces.
fn placements_json(rows: &[WidgetPlacement]) -> String {
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "name": r.name,
                "summary": r.summary,
                "source": match &r.source {
                    WidgetSource::Builtin => "builtin".to_string(),
                    WidgetSource::Plugin => "plugin".to_string(),
                    WidgetSource::Instance { kind } => format!("instance:{kind}"),
                },
                "region": r.placement.map(|(reg, _)| reg.as_str()),
                "index": r.placement.map(|(_, i)| i),
            })
        })
        .collect();
    serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_of(text: &str) -> DocumentMut {
        text.parse::<DocumentMut>().unwrap()
    }

    #[test]
    fn read_layout_falls_back_to_defaults_when_the_table_is_absent() {
        let layout = read_layout(&doc_of("[widgets.cpu]\nformat = \"x\"\n"));
        // Config::load's defaults, not empty arrays.
        assert_eq!(layout.left, ["pane_id", "hostname"]);
        assert_eq!(layout.center, ["windows"]);
        assert!(layout.right.contains(&"datetime".to_string()));
    }

    #[test]
    fn read_layout_falls_back_per_region() {
        // Only `right` is specified; left/center must still come from defaults.
        let layout = read_layout(&doc_of("[layout]\nright = [\"cwd\"]\n"));
        assert_eq!(layout.left, ["pane_id", "hostname"]);
        assert_eq!(layout.center, ["windows"]);
        assert_eq!(layout.right, ["cwd"]);
    }

    #[test]
    fn write_layout_preserves_comments_and_unrelated_tables() {
        let mut doc = doc_of(
            "# my config\n[layout]\nleft = [\"pane_id\"]\n\n[widgets.cpu]\nformat = \"{percent}\"\n",
        );
        let layout = Layout {
            left: vec!["pane_id".into(), "hostname".into()],
            center: vec!["windows".into()],
            right: vec!["cwd".into()],
        };
        write_layout(&mut doc, &layout).unwrap();
        let text = doc.to_string();
        assert!(text.contains("# my config"), "comment preserved: {text}");
        assert!(
            text.contains("[widgets.cpu]"),
            "other table preserved: {text}"
        );
        assert!(text.contains("format = \"{percent}\""));
    }

    #[test]
    fn write_layout_creates_the_table_when_absent() {
        let mut doc = doc_of("[widgets.cpu]\nformat = \"x\"\n");
        let layout = Layout {
            left: vec!["pane_id".into()],
            center: vec![],
            right: vec!["cwd".into()],
        };
        write_layout(&mut doc, &layout).unwrap();
        let text = doc.to_string();
        assert!(text.contains("[layout]"), "{text}");
    }

    #[test]
    fn every_written_document_reparses_under_the_strict_parser() {
        // A write that only survives Config::load's total fallback is a bug:
        // it must parse strictly, the way `rustline config validate` does.
        let mut doc = doc_of("# hi\n[layout]\nleft = [\"pane_id\"]\n");
        let layout = Layout {
            left: vec!["pane_id".into(), "hostname".into()],
            center: vec!["windows".into()],
            right: vec!["cwd".into(), "cpu".into()],
        };
        write_layout(&mut doc, &layout).unwrap();
        let parsed: Config = toml::from_str(&doc.to_string()).expect("strict parse");
        assert_eq!(parsed.layout, layout);
    }

    #[test]
    fn write_layout_round_trips_an_empty_region() {
        let mut doc = doc_of("");
        let layout = Layout {
            left: vec![],
            center: vec![],
            right: vec!["cwd".into()],
        };
        write_layout(&mut doc, &layout).unwrap();
        let parsed: Config = toml::from_str(&doc.to_string()).unwrap();
        assert!(parsed.layout.left.is_empty());
        assert!(parsed.layout.center.is_empty());
        assert_eq!(parsed.layout.right, ["cwd"]);
    }

    #[test]
    fn read_layout_reads_an_inline_table_layout() {
        // `layout = { ... }` is valid config (`Config::load`/`print-config`
        // round-trip it fine); `read_layout` must return the user's real
        // arrays, not silently fall back to the defaults.
        let layout = read_layout(&doc_of(
            "layout = { left = [\"pane_id\"], right = [\"cwd\"] }\n",
        ));
        assert_eq!(layout.left, ["pane_id"]);
        assert_eq!(
            layout.center,
            Layout::default().center,
            "no center key: default"
        );
        assert_eq!(layout.right, ["cwd"]);
    }

    #[test]
    fn write_layout_round_trips_an_inline_table_layout() {
        // Writing back into an inline-table layout must succeed (not refuse,
        // not panic) and the result must still parse under the strict parser.
        let mut doc = doc_of("layout = { left = [\"pane_id\"], right = [\"cwd\"] }\n");
        let layout = Layout {
            left: vec!["pane_id".into(), "hostname".into()],
            center: vec!["windows".into()],
            right: vec!["cwd".into(), "cpu".into()],
        };
        write_layout(&mut doc, &layout).expect("inline table is table-like");
        let text = doc.to_string();
        assert!(
            text.contains("hostname"),
            "new entry present in the inline table: {text}"
        );
        let parsed: Config = toml::from_str(&text).expect("strict parse");
        assert_eq!(parsed.layout, layout);
    }

    #[test]
    fn write_layout_refuses_a_scalar_layout_cleanly() {
        // `layout = "oops"` (or any non-table scalar/array) must be refused
        // with `Err`, never a panic — the write-nothing contract this
        // function's caller (`mutate`) depends on.
        let mut doc = doc_of("layout = \"oops\"\n");
        let before = doc.to_string();
        let error = write_layout(&mut doc, &Layout::default())
            .expect_err("a scalar layout value must be refused, not accepted");
        assert!(error.contains("layout"), "error names the problem: {error}");
        assert_eq!(doc.to_string(), before, "document left untouched");
    }

    #[test]
    fn write_layout_refuses_a_numeric_layout_cleanly() {
        let mut doc = doc_of("layout = 42\n");
        write_layout(&mut doc, &Layout::default())
            .expect_err("a numeric layout value must be refused, not accepted");
    }

    /// Finding 3: `placements_json` must emit well-formed JSON with the
    /// correct `source` string for all three `WidgetSource` variants, and
    /// correct `region`/`index` for both a placed and an unplaced widget.
    #[test]
    fn placements_json_emits_correct_source_and_placement_for_every_variant() {
        let rows = [
            WidgetPlacement {
                name: "cpu".to_string(),
                summary: "CPU usage".to_string(),
                source: WidgetSource::Builtin,
                placement: Some((Region::Right, 1)),
            },
            WidgetPlacement {
                name: "weather".to_string(),
                summary: "Current weather".to_string(),
                source: WidgetSource::Plugin,
                placement: None,
            },
            WidgetPlacement {
                name: "clock_utc".to_string(),
                summary: "UTC clock".to_string(),
                source: WidgetSource::Instance {
                    kind: "datetime".to_string(),
                },
                placement: Some((Region::Left, 0)),
            },
        ];
        let json = placements_json(&rows);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("well-formed JSON");
        let items = parsed.as_array().expect("top-level array");
        assert_eq!(items.len(), 3);

        let cpu = &items[0];
        assert_eq!(cpu["name"], "cpu");
        assert_eq!(cpu["source"], "builtin");
        assert_eq!(cpu["region"], "right");
        assert_eq!(cpu["index"], 1);

        let weather = &items[1];
        assert_eq!(weather["name"], "weather");
        assert_eq!(weather["source"], "plugin");
        assert!(weather["region"].is_null(), "unplaced: {weather}");
        assert!(weather["index"].is_null(), "unplaced: {weather}");

        let clock = &items[2];
        assert_eq!(clock["name"], "clock_utc");
        assert_eq!(clock["source"], "instance:datetime");
        assert_eq!(clock["region"], "left");
        assert_eq!(clock["index"], 0);
    }

    #[test]
    fn resolve_name_accepts_builtins_instances_and_plugins_and_rejects_the_unknown() {
        let known = [
            "cpu".to_string(),
            "clock_utc".to_string(),
            "weather".to_string(),
        ];
        assert!(resolve_name("cpu", &known).is_ok());
        assert!(resolve_name("weather", &known).is_ok());
        let err = resolve_name("nope", &known).unwrap_err();
        assert!(err.contains("nope"), "names the bad widget: {err}");
        assert!(err.contains("cpu"), "lists what is available: {err}");
    }
}
