# Configurable `loadavg` format — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the `loadavg` widget a configurable `format` with inline
per-value precision (`{load1:.1}`), plus `alt_format` click-toggle and
`down_format`, matching the existing format-bearing widget family.

**Architecture:** Add `LoadAvgOpts` to the TOML config, then rewrite the bare
`LoadAvg` unit struct into a `{ format, alt_format, down_format }` widget whose
`render` runs a small pure placeholder scanner (`substitute`) supporting
`{load1|load5|load15}` with an optional `:.N` precision spec (default 2). The
default format reproduces today's `0.42 0.31 0.30` output byte-for-byte.

**Tech Stack:** Rust (edition 2024), `serde` (TOML config), `chrono` (only in
test `Context` construction). No new dependencies.

## Global Constraints

- Edition 2024 in every crate; `rustfmt.toml` is edition 2024. Run
  `cargo fmt --all` before committing.
- Must stay clippy-clean: `cargo clippy --all-targets -- -D warnings`.
- Must stay rustfmt-clean: `cargo fmt --all --check`.
- **Invariant #3 — `Config::load` is total:** every new config field is
  `#[serde(default)]`; a bad `[widgets.loadavg]` table must fall back to
  defaults, never break the bar.
- **Invariant #6 — `loadavg` is `Option`:** `None` renders nothing (never fake
  zeros) unless a non-empty `down_format` is set.
- **Invariant #7 — click-toggle name is one identity:** the layout name, the
  `NAME` const, the `active_format`/`clickable_range` key, and the emitted
  `range=user|…` are all the single string `"loadavg"`.
- **Byte-identical default (load-bearing):** default format at precision 2
  equals the old `format!("{a:.2} {b:.2} {c:.2}")` output. Pinned by a
  characterization test.
- Tests are hermetic (`just test`, no wasm toolchain).
- Commit `Cargo.lock` alongside any dependency change (there are none here).

## File Structure

- `crates/rustline-core/src/config.rs` — add `LoadAvgOpts` + `WidgetOpts.loadavg` (Task 1).
- `crates/rustline-core/src/widgets/loadavg.rs` — rewrite widget + add `substitute` scanner (Task 2).
- `crates/rustline-core/src/widgets/mod.rs` — config-driven `loadavg` registration (Task 2).
- `CLAUDE.md`, `README.md` — doc updates (Task 3).

---

### Task 1: `LoadAvgOpts` config

**Files:**
- Modify: `crates/rustline-core/src/config.rs` (add struct + `WidgetOpts` field + tests)

**Interfaces:**
- Consumes: nothing (new leaf config type).
- Produces: `config::LoadAvgOpts { format: String, alt_format: String, down_format: String }`
  with `Default` (format = `"{load1} {load5} {load15}"`, others empty), and a
  `#[serde(default)] pub loadavg: LoadAvgOpts` field on `WidgetOpts`. Consumed
  by Task 2's `with_builtins`.

- [ ] **Step 1: Write the failing config tests**

Add to the `#[cfg(test)] mod tests` block in `crates/rustline-core/src/config.rs`:

```rust
    #[test]
    fn loadavg_opts_parse_with_defaults() {
        let toml = r#"
[widgets.loadavg]
format = "L {load1:.1}"
alt_format = "{load1} {load5} {load15}"
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.widgets.loadavg.format, "L {load1:.1}");
        assert_eq!(c.widgets.loadavg.alt_format, "{load1} {load5} {load15}");
        assert_eq!(c.widgets.loadavg.down_format, ""); // omitted -> default
    }

    #[test]
    fn loadavg_opts_default_when_absent() {
        let c = Config::default();
        assert_eq!(c.widgets.loadavg.format, "{load1} {load5} {load15}");
        assert_eq!(c.widgets.loadavg.alt_format, "");
        assert_eq!(c.widgets.loadavg.down_format, "");
    }

    #[test]
    fn malformed_loadavg_table_falls_back_to_default() {
        let dir = std::env::temp_dir().join("rustline_test_badloadavg");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        // format must be a string; an integer makes the table invalid.
        std::fs::write(&p, "[widgets.loadavg]\nformat = 5\n").unwrap();
        let c = Config::load(&p);
        assert_eq!(c.widgets.loadavg.format, "{load1} {load5} {load15}");
        assert_eq!(c.layout.left, Config::default().layout.left);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rustline-core --lib config::tests::loadavg 2>&1 | tail -20`
Expected: FAIL — compile error, `no field \`loadavg\` on type \`WidgetOpts\``.

- [ ] **Step 3: Add `LoadAvgOpts` and wire it into `WidgetOpts`**

In `crates/rustline-core/src/config.rs`, add (near the other opts structs, e.g.
just before `WidgetOpts`):

```rust
/// Default `format` for the `loadavg` widget: 1/5/15-min values at 2 decimals,
/// reproducing the pre-config output byte-for-byte.
fn default_loadavg_format() -> String {
    "{load1} {load5} {load15}".into()
}

/// Options for the `loadavg` widget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoadAvgOpts {
    #[serde(default = "default_loadavg_format")]
    pub format: String,
    #[serde(default)]
    pub alt_format: String,
    #[serde(default)]
    pub down_format: String,
}

impl Default for LoadAvgOpts {
    fn default() -> Self {
        Self {
            format: default_loadavg_format(),
            alt_format: String::new(),
            down_format: String::new(),
        }
    }
}
```

Then add the field to `WidgetOpts` (alongside `datetime`, `cwd`, …):

```rust
    #[serde(default)]
    pub loadavg: LoadAvgOpts,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rustline-core --lib config::tests::loadavg 2>&1 | tail -20`
Expected: PASS (3 tests).

- [ ] **Step 5: Lint + format**

Run: `cargo clippy -p rustline-core --all-targets -- -D warnings && cargo fmt --all`
Expected: no warnings, no diff after fmt.

- [ ] **Step 6: Commit**

```bash
git add crates/rustline-core/src/config.rs
git commit -m "feat(config): add LoadAvgOpts (format/alt_format/down_format)

Claude-Session: https://claude.ai/code/session_01Fddrk6VgQAPGvRg71F2eB1"
```

---

### Task 2: `LoadAvg` widget + inline-precision scanner + registration

**Files:**
- Modify: `crates/rustline-core/src/widgets/loadavg.rs` (rewrite struct, add `substitute`, rewrite tests)
- Modify: `crates/rustline-core/src/widgets/mod.rs` (config-driven registration + doc comment)

**Interfaces:**
- Consumes: `config::LoadAvgOpts` (Task 1); `crate::widgets::{active_format, clickable_range}` (existing helpers).
- Produces: `widgets::loadavg::LoadAvg { format: String, alt_format: String, down_format: String }`
  with `const NAME: &'static str = "loadavg"`, implementing `Widget::render` and
  `Widget::range_name`. Private `fn substitute(fmt: &str, values: Option<[f64; 3]>) -> String`.

- [ ] **Step 1: Write the failing scanner + widget tests**

Replace the **entire** `#[cfg(test)] mod tests { … }` block at the bottom of
`crates/rustline-core/src/widgets/loadavg.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Widget};
    use chrono::{Local, TimeZone};

    fn ctx_load(l: Option<[f64; 3]>) -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/".into(),
            home: "/h".into(),
            hostname: "h".into(),
            loadavg: l,
            now: Local
                .with_ymd_and_hms(2026, 7, 20, 17, 49, 0)
                .single()
                .unwrap(),
            window: None,
            interfaces: Vec::new(),
            battery: None,
            cpu: None,
            memory: None,
            os: String::new(),
            arch: String::new(),
            toggled: Default::default(),
        }
    }

    fn w(format: &str, alt: &str, down: &str) -> LoadAvg {
        LoadAvg {
            format: format.into(),
            alt_format: alt.into(),
            down_format: down.into(),
        }
    }

    // --- scanner ---

    #[test]
    fn characterization_default_matches_legacy_output() {
        // Load-bearing: default format at precision 2 must equal the old
        // format!("{a:.2} {b:.2} {c:.2}") output byte-for-byte. .296 -> 0.30.
        assert_eq!(
            substitute("{load1} {load5} {load15}", Some([0.42, 0.31, 0.296])),
            "0.42 0.31 0.30"
        );
    }

    #[test]
    fn inline_precision_spec_honored() {
        assert_eq!(substitute("{load1:.1}", Some([0.456, 0.0, 0.0])), "0.5");
        assert_eq!(substitute("{load1:.0}", Some([0.456, 0.0, 0.0])), "0");
        assert_eq!(substitute("{load1:.3}", Some([0.2965, 0.0, 0.0])), "0.297");
    }

    #[test]
    fn per_value_mixed_precision() {
        assert_eq!(
            substitute(
                "{load1:.2} {load5:.1} {load15:.0}",
                Some([1.234, 0.56, 2.7])
            ),
            "1.23 0.6 3"
        );
    }

    #[test]
    fn precision_clamped_no_panic() {
        // .15 clamps to 10 dp; must not panic.
        assert_eq!(substitute("{load1:.15}", Some([0.5, 0.0, 0.0])), "0.5000000000");
    }

    #[test]
    fn unknown_token_passes_through_verbatim() {
        assert_eq!(
            substitute("{cpu} {load2} {load1}", Some([0.42, 0.0, 0.0])),
            "{cpu} {load2} 0.42"
        );
        assert_eq!(substitute("{}", Some([0.0, 0.0, 0.0])), "{}");
    }

    #[test]
    fn malformed_spec_passes_through_verbatim() {
        assert_eq!(substitute("{load1:x}", Some([0.42, 0.0, 0.0])), "{load1:x}");
        assert_eq!(substitute("{load1:.}", Some([0.42, 0.0, 0.0])), "{load1:.}");
    }

    #[test]
    fn literal_text_and_unterminated_brace_preserved() {
        assert_eq!(
            substitute("load: {load1}!", Some([0.42, 0.0, 0.0])),
            "load: 0.42!"
        );
        assert_eq!(substitute("a {load1", Some([0.42, 0.0, 0.0])), "a {load1");
    }

    #[test]
    fn none_collapses_recognized_placeholders() {
        assert_eq!(substitute("load {load1} {load5}?", None), "load  ?");
        // unknown tokens still verbatim in down state
        assert_eq!(substitute("{cpu} {load1}", None), "{cpu} ");
    }

    // --- widget ---

    #[test]
    fn renders_default_format() {
        let out = w("{load1} {load5} {load15}", "", "").render(&ctx_load(Some([0.42, 0.31, 0.296])));
        assert_eq!(out[0].text, "0.42 0.31 0.30");
    }

    #[test]
    fn none_empty_down_renders_nothing() {
        assert!(w("{load1}", "", "").render(&ctx_load(None)).is_empty());
    }

    #[test]
    fn none_down_format_collapses() {
        let out = w("{load1}", "", "load n/a {load1}").render(&ctx_load(None));
        assert_eq!(out[0].text, "load n/a ");
    }

    #[test]
    fn toggled_uses_alt_format() {
        let mut c = ctx_load(Some([0.42, 0.31, 0.296]));
        c.toggled.insert("loadavg".to_string());
        let out = w("{load1}", "{load1:.1} {load5:.1} {load15:.1}", "").render(&c);
        assert_eq!(out[0].text, "0.4 0.3 0.3");
        // untoggled -> normal format
        let out = w("{load1}", "{load1:.1} {load5:.1} {load15:.1}", "")
            .render(&ctx_load(Some([0.42, 0.31, 0.296])));
        assert_eq!(out[0].text, "0.42");
    }

    #[test]
    fn range_name_some_only_with_alt_format() {
        assert_eq!(w("{load1}", "{load1:.1}", "").range_name(), Some("loadavg"));
        assert_eq!(w("{load1}", "", "").range_name(), None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rustline-core --lib widgets::loadavg 2>&1 | tail -20`
Expected: FAIL — compile error (`substitute` not found; `LoadAvg` has no fields).

- [ ] **Step 3: Rewrite the non-test portion of `loadavg.rs`**

Replace everything **above** the `#[cfg(test)]` line in
`crates/rustline-core/src/widgets/loadavg.rs` with:

```rust
use crate::{Context, Segment, Widget};

/// Renders the 1/5/15-minute load average, when available.
///
/// `Context::loadavg` is `None` on platforms/environments where it couldn't be
/// sampled; the widget then renders nothing (or `down_format`) rather than
/// faking zeros. Part of the format-bearing widget family: a non-empty
/// `alt_format` makes it click-toggleable.
pub struct LoadAvg {
    pub format: String,
    pub alt_format: String,
    pub down_format: String,
}

impl LoadAvg {
    /// Registry/layout name; the toggle key threaded through render + click.
    pub const NAME: &'static str = "loadavg";
}

impl Widget for LoadAvg {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        match ctx.loadavg {
            Some(vals) => {
                let fmt =
                    crate::widgets::active_format(ctx, Self::NAME, &self.format, &self.alt_format);
                vec![Segment::new(substitute(fmt, Some(vals)))]
            }
            None => {
                if self.down_format.is_empty() {
                    return vec![];
                }
                vec![Segment::new(substitute(&self.down_format, None))]
            }
        }
    }

    fn range_name(&self) -> Option<&str> {
        crate::widgets::clickable_range(Self::NAME, &self.alt_format)
    }
}

/// Substitute `{load1|load5|load15}` placeholders in `fmt`. Each may carry an
/// inline precision spec `:.N` (default 2, clamped to `0..=10`).
/// `values = Some([a, b, c])` formats each value; `values = None` (down state)
/// collapses every recognized placeholder to the empty string. Any
/// unrecognized `{…}` token — unknown name or malformed spec — is emitted
/// verbatim, matching how the other widgets leave unknown placeholders alone.
fn substitute(fmt: &str, values: Option<[f64; 3]>) -> String {
    let mut out = String::with_capacity(fmt.len());
    let mut rest = fmt;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            // No closing brace: copy the remainder verbatim (incl. the '{').
            out.push_str(&rest[open..]);
            return out;
        };
        let token = &after[..close];
        match render_token(token, values) {
            Some(text) => out.push_str(&text),
            None => {
                out.push('{');
                out.push_str(token);
                out.push('}');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Resolve one `{…}` token's inner text. `Some(text)` for a recognized
/// `loadN[:.N]`; `None` if the name isn't a load placeholder or the spec is
/// malformed (the caller then emits it verbatim).
fn render_token(token: &str, values: Option<[f64; 3]>) -> Option<String> {
    let (name, precision) = match token.split_once(':') {
        Some((name, spec)) => (name, parse_precision(spec)?),
        None => (token, 2usize),
    };
    let idx = match name {
        "load1" => 0,
        "load5" => 1,
        "load15" => 2,
        _ => return None,
    };
    Some(match values {
        Some(v) => format!("{:.*}", precision, v[idx]),
        None => String::new(),
    })
}

/// Parse a precision spec `.N` (decimal digits), clamped to `0..=10`. `None`
/// for any other shape, so a malformed token passes through verbatim.
fn parse_precision(spec: &str) -> Option<usize> {
    let digits = spec.strip_prefix('.')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(digits.parse::<usize>().unwrap_or(10).min(10))
}
```

- [ ] **Step 4: Rewire registration in `mod.rs`**

In `crates/rustline-core/src/widgets/mod.rs`, replace the bare registration line:

```rust
        registry.register("loadavg", Box::new(|| Box::new(LoadAvg)));
```

with:

```rust
        let loadavg = cfg.widgets.loadavg.clone();
        registry.register(
            "loadavg",
            Box::new(move || {
                Box::new(LoadAvg {
                    format: loadavg.format.clone(),
                    alt_format: loadavg.alt_format.clone(),
                    down_format: loadavg.down_format.clone(),
                })
            }),
        );
```

Then update the `with_builtins` doc comment: change the list of
option-carrying widgets `(datetime, cwd, lan_ip, tailscale_ip, battery, cpu,
memory)` to also include `loadavg`.

- [ ] **Step 5: Run the loadavg + full core tests to verify they pass**

Run: `cargo test -p rustline-core --lib 2>&1 | tail -20`
Expected: PASS (all core lib tests, including the new `widgets::loadavg` and
`config::tests::loadavg` tests).

- [ ] **Step 6: Lint + format**

Run: `cargo clippy -p rustline-core --all-targets -- -D warnings && cargo fmt --all --check`
Expected: no warnings, no diff.

- [ ] **Step 7: Commit**

```bash
git add crates/rustline-core/src/widgets/loadavg.rs crates/rustline-core/src/widgets/mod.rs
git commit -m "feat(loadavg): configurable format with inline :.N precision + click-toggle

{load1}/{load5}/{load15} placeholders, optional per-value precision spec
(default 2 -> byte-identical default output), alt_format click-toggle, and
down_format. loadavg becomes the seventh format-bearing widget.

Claude-Session: https://claude.ai/code/session_01Fddrk6VgQAPGvRg71F2eB1"
```

---

### Task 3: Documentation

**Files:**
- Modify: `CLAUDE.md`
- Modify: `README.md`

**Interfaces:**
- Consumes: the shipped behavior from Tasks 1–2. Produces: no code.

No test cycle (docs only). Verify by re-reading the edited sections.

- [ ] **Step 1: Update CLAUDE.md "six format-bearing widgets" references**

Two occurrences. Replace each `the six format-bearing widgets` phrasing and its
six-name list with the seven-name version including `loadavg`:

- In the `toggle.rs` module-map bullet: change
  ``the six format-bearing widgets (`datetime`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`, `memory`)``
  to
  ``the seven format-bearing widgets (`datetime`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`, `memory`, `loadavg`)``.
- In the **Click-to-toggle widget views** Config section: change
  ``the six format-bearing widgets — `datetime`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`, `memory` —``
  to
  ``the seven format-bearing widgets — `datetime`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`, `memory`, `loadavg` —``.

- [ ] **Step 2: Add a `loadavg` module-map note + Config paragraph in CLAUDE.md**

In the `rustline-core` module map, add a `loadavg.rs` note near the other
widget descriptions (e.g. after the `windows.rs` sentence):

```
`loadavg.rs` is the `loadavg` widget: pure over `Context.loadavg`, with
`{load1}`/`{load5}`/`{load15}` placeholders that each accept an inline Rust-style
precision spec (`{load1:.1}`; default 2 decimals, so the default format is
byte-identical to the pre-config output), plus `alt_format`/`down_format` like
the rest of the family; a private `substitute` scanner does the replacement.
```

In the **Config** section, add a short `loadavg` subsection (near the cpu/memory
or click-toggle text):

```
**Load average widget:** `loadavg` is in the **default** right layout. It takes
a `format` (default `"{load1} {load5} {load15}"`) with `{load1}`/`{load5}`/
`{load15}` placeholders (1/5/15-minute averages), each accepting an inline
precision spec `:.N` (e.g. `{load1:.1}`; bare `{loadN}` is 2 decimals, `N`
clamped to 0–10). Also takes an `alt_format` (click-toggle) and a `down_format`
(shown when `getloadavg` fails; default empty → renders nothing).

    [widgets.loadavg]
    format      = "{load1} {load5} {load15}"   # default
    alt_format  = "{load1:.1} {load5:.1} {load15:.1}"   # left-click toggles to this
    down_format = ""
```

- [ ] **Step 3: Add a "Load average widget" subsection to README.md**

Insert after the "CPU and memory widgets" subsection and before
"Click-to-toggle widget views":

```markdown
### Load average widget

`loadavg` is built-in and **in the default right layout** — it shows the
1/5/15-minute system load average (nothing on platforms where it can't be read,
rather than fake zeros).

Takes a `format` with `{load1}`/`{load5}`/`{load15}` placeholders, each of which
accepts an inline Rust-style precision spec `:.N` — `{load1:.1}` → `0.4`. A bare
`{load1}` is two decimals (so the default renders exactly like older versions),
and `N` is clamped to 0–10. Also takes a `down_format` (default empty, shown
when the load can't be read) and an `alt_format` for
[click-to-toggle](#click-to-toggle-widget-views).

```toml
[widgets.loadavg]
format      = "{load1} {load5} {load15}"          # default
alt_format  = "{load1:.1} {load5:.1} {load15:.1}" # left-click toggles to this
```
```

- [ ] **Step 4: Update the README click-to-toggle roster**

Change
``` `datetime`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`, and `memory` each take an `alt_format` ```
to
``` `datetime`, `lan_ip`, `tailscale_ip`, `battery`, `cpu`, `memory`, and `loadavg` each take an `alt_format` ```

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md README.md
git commit -m "docs: document configurable loadavg format (7th format-bearing widget)

Claude-Session: https://claude.ai/code/session_01Fddrk6VgQAPGvRg71F2eB1"
```

---

## Final verification (after Task 3)

- [ ] Run the full hermetic suite + lints:

```bash
just test && just lint && cargo fmt --all --check
```

Expected: all tests pass, no clippy warnings, no fmt diff.

- [ ] Sanity-check the rendered output end to end:

```bash
cargo run -p rustline -- render right --preview
```

Expected: the load-average segment shows `X.XX X.XX X.XX` (2-decimal default),
identical to before the change.

## Self-Review notes

- **Spec coverage:** config (Task 1), scanner + widget + click-toggle +
  down_format + registration (Task 2), byte-identical characterization test
  (Task 2 Step 1), docs incl. both CLAUDE.md + README (Task 3). All spec
  sections mapped.
- **Type consistency:** `substitute(&str, Option<[f64;3]>) -> String`,
  `LoadAvg { format, alt_format, down_format }`, `NAME = "loadavg"`, and
  `LoadAvgOpts { format, alt_format, down_format }` are used consistently
  across tasks.
- **No placeholders:** every code/test step contains complete content.
