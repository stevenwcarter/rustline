# `rustline theme pick` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `rustline theme pick` — an interactive command to browse theme previews and set one — reusing the existing theme helpers, with no new dependency.

**Architecture:** A `ThemeCmd::Pick` variant dispatched to a thin `pick()` I/O shell in `theme_cmd.rs`. The interaction core (`run_picker`) is generic over a reader/writer so the whole loop is unit-testable without a TTY; pure parsers (`picker_entries`, `parse_preview_input`, `parse_set_input`) carry the tests. The config write reuses the existing `use_theme`.

**Tech Stack:** Rust (edition 2024), clap derive, `std::io::{BufRead, Write, IsTerminal}`, existing `theme_cmd` helpers, `tempfile` (dev).

## Global Constraints

- Edition 2024; keep clippy-clean (`cargo clippy --all-targets -- -D warnings`, workspace) and rustfmt-clean (`cargo fmt --all --check`). Run `cargo fmt --all` before committing.
- **No new dependency** — plain line-prompts only (no TUI crate).
- **`theme use <name>` behavior is unchanged** — `pick` reuses `use_theme`, it must not modify it.
- **`Config::load` totality (invariant #3)** is untouched — `pick` reads config and writes only through `use_theme`.
- **Test command:** `cargo test -p rustline <filter>` — the `rustline` crate is **bin-only**, so `--lib` errors ("no library targets"). Never use `--lib`.
- **Theme resolution is file-first** (a themes-dir `*.toml` shadows a same-named built-in) — `picker_entries` must reflect this (one entry per name, `from_file` set when a file provides it).
- Commit `Cargo.lock` alongside any dependency change (none expected).

---

### Task 1: `theme pick` command — CLI, interactive core, and tests

**Files:**
- Modify: `crates/rustline/src/cli.rs` (add `Pick` to `ThemeCmd`)
- Modify: `crates/rustline/src/theme_cmd.rs` (dispatch arm; `PickEntry`, `PreviewCmd`, `SetCmd`, `picker_entries`, `parse_preview_input`, `parse_set_input`, `read_line`, `run_picker`, `pick`; unit tests)
- Test: `crates/rustline/src/theme_cmd.rs` (`#[cfg(test)] mod tests`) + `crates/rustline/tests/smoke.rs` (non-TTY integration)

**Interfaces:**
- Consumes (existing in `theme_cmd.rs`): `use_theme(&str,&Path,&Path)`, `preview_named(&str,&Path)->Option<String>`, `theme_files(&Path)->Vec<String>`, `resolvable`, and `rustline_core::{Config, builtin_theme_names}`.
- Produces: `ThemeCmd::Pick` (unit variant); everything else is `theme_cmd`-private.

- [ ] **Step 1: Write the failing unit + smoke tests**

Add to `theme_cmd.rs`'s existing `#[cfg(test)] mod tests` (the `use super::*;` is already there):

```rust
    #[test]
    fn picker_entries_orders_dedups_and_marks() {
        let builtins = ["default", "nord", "gruvbox"];
        let files = vec!["nord".to_string(), "mine".to_string()]; // nord file shadows built-in
        let e = picker_entries("nord", &builtins, &files);
        assert_eq!(
            e.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
            vec!["default", "nord", "gruvbox", "mine"] // built-ins in order, then file-only
        );
        // exactly the nord entry is active, and it's from_file (file-first resolution)
        let nord = e.iter().find(|x| x.name == "nord").unwrap();
        assert!(nord.active && nord.from_file);
        assert_eq!(e.iter().filter(|x| x.active).count(), 1);
        assert!(e.iter().find(|x| x.name == "mine").unwrap().from_file);
        assert!(!e.iter().find(|x| x.name == "default").unwrap().from_file);
    }

    #[test]
    fn parse_preview_input_cases() {
        assert_eq!(parse_preview_input("", 6), PreviewCmd::Done);
        assert_eq!(parse_preview_input("  ", 6), PreviewCmd::Done);
        assert_eq!(parse_preview_input("a", 6), PreviewCmd::All);
        assert_eq!(parse_preview_input("ALL", 6), PreviewCmd::All);
        assert_eq!(parse_preview_input("1", 6), PreviewCmd::Preview(0));
        assert_eq!(parse_preview_input("6", 6), PreviewCmd::Preview(5));
        assert_eq!(parse_preview_input("0", 6), PreviewCmd::Invalid);
        assert_eq!(parse_preview_input("7", 6), PreviewCmd::Invalid);
        assert_eq!(parse_preview_input("x", 6), PreviewCmd::Invalid);
    }

    #[test]
    fn parse_set_input_cases() {
        assert_eq!(parse_set_input("", 6), SetCmd::Keep);
        assert_eq!(parse_set_input("3", 6), SetCmd::Index(2));
        assert_eq!(parse_set_input("nord", 6), SetCmd::Name("nord".to_string()));
        assert_eq!(parse_set_input("99", 6), SetCmd::Name("99".to_string())); // out-of-range → name
    }

    #[test]
    fn run_picker_previews_then_sets_by_index() {
        let td = tempfile::tempdir().unwrap();
        let entries = picker_entries("default", builtin_theme_names(), &[]);
        let input = b"2\n\n1\n"; // preview #2, blank ends preview loop, set #1
        let mut reader = std::io::Cursor::new(&input[..]);
        let mut out: Vec<u8> = Vec::new();
        let choice = run_picker(&entries, td.path(), &mut reader, &mut out, "default");
        assert_eq!(choice.as_deref(), Some(entries[0].name.as_str()));
        assert!(String::from_utf8(out).unwrap().contains(&format!("== {} ==", entries[1].name)));
    }

    #[test]
    fn run_picker_blank_keeps_current() {
        let td = tempfile::tempdir().unwrap();
        let entries = picker_entries("default", builtin_theme_names(), &[]);
        let input = b"\n\n"; // no preview, blank set → keep
        let mut reader = std::io::Cursor::new(&input[..]);
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(run_picker(&entries, td.path(), &mut reader, &mut out, "default"), None);
    }

    #[test]
    fn run_picker_all_then_set_by_name() {
        let td = tempfile::tempdir().unwrap();
        let entries = picker_entries("default", builtin_theme_names(), &[]);
        let input = b"a\n\nnord\n"; // preview all, then set by name
        let mut reader = std::io::Cursor::new(&input[..]);
        let mut out: Vec<u8> = Vec::new();
        let choice = run_picker(&entries, td.path(), &mut reader, &mut out, "default");
        assert_eq!(choice.as_deref(), Some("nord"));
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("== nord ==") && s.contains("== gruvbox =="));
    }

    #[test]
    fn run_picker_reasks_on_unknown_name() {
        let td = tempfile::tempdir().unwrap();
        let entries = picker_entries("default", builtin_theme_names(), &[]);
        let input = b"\nnope\nnord\n"; // blank preview-done; set "nope" (reask) then "nord"
        let mut reader = std::io::Cursor::new(&input[..]);
        let mut out: Vec<u8> = Vec::new();
        let choice = run_picker(&entries, td.path(), &mut reader, &mut out, "default");
        assert_eq!(choice.as_deref(), Some("nord"));
        assert!(String::from_utf8(out).unwrap().contains("unknown theme: nope"));
    }
```

Add to `crates/rustline/tests/smoke.rs`:

```rust
#[test]
fn theme_pick_non_tty_errors_and_writes_nothing() {
    let tmp = tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustline"));
    cmd.arg("theme").arg("pick");
    isolate(&mut cmd, tmp.path());
    cmd.env("XDG_CONFIG_HOME", tmp.path().join("cfg"));
    let out = cmd.output().unwrap(); // no TTY under Command
    assert!(!out.status.success(), "non-TTY `theme pick` must error");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("theme show") || err.contains("theme use"),
        "hints the non-interactive alternatives: {err}"
    );
    assert!(
        !tmp.path().join("cfg").join("rustline").join("config.toml").exists(),
        "must not write config on the non-TTY path"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rustline theme_cmd` then `cargo test -p rustline --test smoke theme_pick`
Expected: FAIL to compile (`Pick` variant, `picker_entries`, `run_picker`, etc. undefined).

- [ ] **Step 3: Add the `Pick` CLI variant**

In `cli.rs`, add to the `ThemeCmd` enum (after `Use`):

```rust
    /// Interactively browse theme previews and set one.
    Pick,
```

- [ ] **Step 4: Implement the dispatch arm, helpers, `run_picker`, and `pick`**

In `theme_cmd.rs`, add imports near the top (alongside the existing `use std::path::Path;`):

```rust
use std::io::{BufRead, IsTerminal, Write as _};
```

Add the dispatch arm inside `run`'s `match cmd` (after the `Use` arm):

```rust
        ThemeCmd::Pick => pick(config_path, themes_dir),
```

Add the following (place the helpers/`run_picker`/`pick` above the `#[cfg(test)]` module):

```rust
/// One selectable theme in the picker: its name, whether it's the active base,
/// and whether it comes from a themes-dir file (which shadows a same-named
/// built-in, since resolution is file-first).
#[derive(Debug, PartialEq, Eq)]
struct PickEntry {
    name: String,
    active: bool,
    from_file: bool,
}

/// Ordered, name-unique picker entries: built-ins (in `builtins` order) first,
/// then themes-dir stems not already among the built-ins. `active` marks the
/// single entry whose name equals `active`; `from_file` marks any entry whose
/// name appears in `files` (a file shadowing a built-in name is one entry with
/// `from_file = true`).
fn picker_entries(active: &str, builtins: &[&str], files: &[String]) -> Vec<PickEntry> {
    let mut entries = Vec::new();
    for name in builtins {
        entries.push(PickEntry {
            name: (*name).to_string(),
            active: *name == active,
            from_file: files.iter().any(|f| f == name),
        });
    }
    for f in files {
        if !builtins.iter().any(|b| b == f) {
            entries.push(PickEntry {
                name: f.clone(),
                active: f == active,
                from_file: true,
            });
        }
    }
    entries
}

/// A parsed preview-loop command.
#[derive(Debug, PartialEq, Eq)]
enum PreviewCmd {
    Done,
    All,
    Preview(usize),
    Invalid,
}

/// Parse a preview-loop answer: blank → `Done`; `a`/`all` (case-insensitive) →
/// `All`; a number in `1..=n` → `Preview(k-1)`; anything else → `Invalid`.
fn parse_preview_input(input: &str, n: usize) -> PreviewCmd {
    let t = input.trim();
    if t.is_empty() {
        return PreviewCmd::Done;
    }
    let lower = t.to_ascii_lowercase();
    if lower == "a" || lower == "all" {
        return PreviewCmd::All;
    }
    match t.parse::<usize>() {
        Ok(k) if (1..=n).contains(&k) => PreviewCmd::Preview(k - 1),
        _ => PreviewCmd::Invalid,
    }
}

/// A parsed set-step command.
#[derive(Debug, PartialEq, Eq)]
enum SetCmd {
    Keep,
    Index(usize),
    Name(String),
}

/// Parse a set-step answer: blank → `Keep`; a number in `1..=n` → `Index(k-1)`;
/// any other non-blank → `Name(trimmed)` (an out-of-range number lands here and
/// is rejected by the caller's entry lookup).
fn parse_set_input(input: &str, n: usize) -> SetCmd {
    let t = input.trim();
    if t.is_empty() {
        return SetCmd::Keep;
    }
    if let Ok(k) = t.parse::<usize>()
        && (1..=n).contains(&k)
    {
        return SetCmd::Index(k - 1);
    }
    SetCmd::Name(t.to_string())
}

/// Read one line from `reader` (empty string on EOF).
fn read_line<R: BufRead>(reader: &mut R) -> String {
    let mut s = String::new();
    let _ = reader.read_line(&mut s);
    s
}

/// Drive the interactive preview+set loop over a generic reader/writer,
/// returning the chosen theme name to set, or `None` to keep the current one.
/// Performs NO config write and never exits the process, so it is fully
/// unit-testable with a byte-slice reader.
fn run_picker<R: BufRead, W: Write>(
    entries: &[PickEntry],
    themes_dir: &Path,
    reader: &mut R,
    writer: &mut W,
    active: &str,
) -> Option<String> {
    let _ = writeln!(writer, "Themes (* = active):");
    for (i, e) in entries.iter().enumerate() {
        let mark = if e.active { " *" } else { "" };
        let tag = if e.from_file { "  (custom)" } else { "" };
        let _ = writeln!(writer, "  {}) {}{mark}{tag}", i + 1, e.name);
    }
    loop {
        let _ = write!(writer, "Preview # (number, a=all, enter=done): ");
        let _ = writer.flush();
        match parse_preview_input(&read_line(reader), entries.len()) {
            PreviewCmd::Done => break,
            PreviewCmd::All => {
                for e in entries {
                    let _ = writeln!(writer, "\n== {} ==", e.name);
                    if let Some(p) = preview_named(&e.name, themes_dir) {
                        let _ = writeln!(writer, "{p}");
                    }
                }
            }
            PreviewCmd::Preview(idx) => {
                let name = &entries[idx].name;
                let _ = writeln!(writer, "\n== {name} ==");
                if let Some(p) = preview_named(name, themes_dir) {
                    let _ = writeln!(writer, "{p}");
                }
            }
            PreviewCmd::Invalid => {
                let _ = writeln!(
                    writer,
                    "enter a number 1-{}, 'a' for all, or blank to finish",
                    entries.len()
                );
            }
        }
    }
    loop {
        let _ = write!(writer, "Set which theme? [name or #, enter=keep {active}]: ");
        let _ = writer.flush();
        match parse_set_input(&read_line(reader), entries.len()) {
            SetCmd::Keep => return None,
            SetCmd::Index(idx) => return Some(entries[idx].name.clone()),
            SetCmd::Name(s) => {
                if let Some(e) = entries.iter().find(|e| e.name == s) {
                    return Some(e.name.clone());
                }
                let _ = writeln!(writer, "unknown theme: {s}");
            }
        }
    }
}

/// `rustline theme pick`: interactively browse theme previews and set one.
/// Requires a TTY; a non-interactive invocation prints a hint and exits.
fn pick(config_path: &Path, themes_dir: &Path) {
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "theme pick is interactive and needs a terminal.\n\
             Use `rustline theme show <name>` to preview or `rustline theme use <name>` to set non-interactively."
        );
        std::process::exit(2);
    }
    let cfg = Config::load(config_path);
    let active = cfg
        .theme
        .base
        .clone()
        .unwrap_or_else(|| "default".to_string());
    let entries = picker_entries(&active, builtin_theme_names(), &theme_files(themes_dir));
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let stderr = std::io::stderr();
    let mut writer = stderr.lock();
    match run_picker(&entries, themes_dir, &mut reader, &mut writer, &active) {
        Some(name) => use_theme(&name, config_path, themes_dir),
        None => println!("kept {active}"),
    }
}
```

Note: `use_theme` already prints `theme set to <name>` and handles write/parse errors (it `exit(1)`s on an unwritable/unparseable config) — do NOT duplicate that. `Config` and `builtin_theme_names` are already imported at the top of `theme_cmd.rs`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rustline theme_cmd && cargo test -p rustline --test smoke theme_pick`
Expected: PASS (6 new unit tests + the smoke test; existing `theme_cmd` tests still green).

- [ ] **Step 6: fmt + clippy + full test + commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test -p rustline
git add crates/rustline/src/cli.rs crates/rustline/src/theme_cmd.rs crates/rustline/tests/smoke.rs
git commit -m "feat(theme): interactive \`rustline theme pick\` (browse previews + set)"
```

---

### Task 2: Documentation

**Files:**
- Modify: `README.md` (theme-management section — add `theme pick`)
- Modify: `CLAUDE.md` (`theme_cmd.rs` module-map entry; CLI section theme subcommands; `ThemeCmd` variant list; Roadmap "Done")

**Interfaces:** none (docs only).

- [ ] **Step 1: Update README**

Find the theme CLI listing (the `rustline theme list|show|use|new` area) and add a `pick` entry, e.g.:

```markdown
- `rustline theme pick` — interactively browse theme previews and set one:
  lists the themes (active marked), previews any (or `a` for all), then sets
  your choice. Needs a terminal; for scripting use `theme show` / `theme use`.
```

Place it next to the other `rustline theme …` bullets and keep the existing
`theme use`/`show`/`new`/`list` descriptions intact.

- [ ] **Step 2: Update CLAUDE.md**

- The `theme_cmd.rs` module-map bullet: note the new `pick` (interactive
  browse-and-set) built on `run_picker` (reader/writer-generic, unit-tested
  interaction core returning the chosen name or `None`) plus the pure
  `picker_entries`/`parse_preview_input`/`parse_set_input`; it reuses
  `use_theme` for the write.
- The CLI section: add a `rustline theme pick` bullet mirroring the README line
  (interactive; non-TTY → hint + exit).
- The `ThemeCmd` description (it currently lists `List, Show, Use, New`): add
  `Pick`.
- Roadmap "Done": add a one-line entry linking the spec/plan
  (`docs/superpowers/specs/2026-07-22-rustline-theme-pick-design.md` /
  `docs/superpowers/plans/2026-07-22-rustline-theme-pick.md`).

- [ ] **Step 3: Verify + commit**

```bash
grep -n "theme pick" README.md CLAUDE.md
cargo test -p rustline   # docs change nothing; confirm still green
git add README.md CLAUDE.md
git commit -m "docs: rustline theme pick (README + CLAUDE.md)"
```

---

## Self-Review

**Spec coverage:**
- `ThemeCmd::Pick` + dispatch → Task 1 (Steps 3-4).
- TTY guard (non-TTY → hint + exit(2), no write) → `pick` (Task 1) + smoke test.
- Preview loop (number / `a`=all / blank=done), active-marked list, `(custom)` tag → `run_picker` (Task 1).
- Set step (blank=keep, number→index, name; reask on unknown) reusing `use_theme` → `run_picker` + `pick` (Task 1).
- Pure helpers with tests (`picker_entries`, `parse_preview_input`, `parse_set_input`) + reader/writer-generic `run_picker` unit tests → Task 1 Step 1.
- No new dependency; `theme use` unchanged; `Config::load` totality untouched → constraints honored (write goes only through `use_theme`).
- Docs → Task 2.

**Placeholder scan:** none — every step has complete code or exact edits.

**Type consistency:** `PickEntry { name, active, from_file }`, `PreviewCmd::{Done,All,Preview(usize),Invalid}`, `SetCmd::{Keep,Index(usize),Name(String)}`, and the `picker_entries`/`parse_preview_input`/`parse_set_input`/`run_picker`/`pick` signatures are used identically in the tests (Step 1) and implementation (Step 4). `run_picker` returns `Option<String>`; `pick` matches `Some(name) => use_theme(...)` / `None => println!`.
