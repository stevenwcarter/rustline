# Code-Health Batch: Markup Safety & Silent-Failure Diagnostics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the 11 code-health findings the user selected in `bughunt.md` — close a tmux-markup injection hole reachable from plugins and external data, bound the render path's unbounded subprocess reads, and make six classes of silent failure visible in the log.

**Architecture:** Every change is surgical and local. The two markup findings (B1, B3) introduce one sanitizing helper in `render.rs` applied at three existing emission sites, plus a matching un-escape in `ansi.rs::scan` so the preview renderers stay faithful. The subprocess finding (B5) extracts the already-hardened spawn+timeout+process-group-kill logic out of `rustline-wasm::run` and reuses it at three call sites. The remaining eight are additive logging, serde defaults, a clamp, and a protocol version field.

**Tech Stack:** Rust edition 2024, workspace of five crates (`rustline-abi`, `rustline-core`, `rustline-wasm`, `rustline-plugin-sdk`, `rustline` bin). `tracing` for logs, `serde`/`serde_json` for the wire types, `libc` for FFI, `extism` for the WASM host. Tests are in-module `#[cfg(test)] mod tests` plus `crates/rustline/tests/smoke.rs`.

## Global Constraints

- **Edition 2024 in every crate**, matching `rustfmt.toml`. Never change a crate's edition.
- **Baseline to preserve:** `cargo build --workspace` clean, `just lint` clean, **937 tests passing, 0 failing, 1 ignored**. Never let any of these regress.
- **`just lint` is three passes**, all of which must stay clean: `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo clippy --workspace --all-targets --features wasm-e2e -- -D warnings`.
- **There is no pre-commit hook.** Run `cargo fmt --all` yourself before every commit.
- **One commit per finding**, message format `fix(<category>): <summary> [B<n>]`.
- **Strip the fixed finding's whole block from `bughunt.md`** as part of that task. `bughunt.md` is listed in `.git/info/exclude`, so it is untracked and will NOT appear in `git status` or in the commit — strip it anyway; the file must reflect open issues only.
- **Never modify or weaken an existing test** to make a change pass. Add new tests. In particular do not touch `read_throughput_first_run_is_none_then_some_on_second_call` (`crates/rustline/src/throughput.rs:341`) — it pins behaviour that finding B30 flags as wrong, but B30 is **not** in this batch.
- **Do not touch any unselected finding.** Out of scope and must remain reproducible: B4, B12, B13, B15–B39, and all five `decision-needed` markers. Specifically **B26** — the two existing `tracing::warn!` lines at `crates/rustline-wasm/src/host.rs:219` and `:230` do not carry `plugin = %self.name`, and Tasks 8 and 9 edit that same function. **Leave those two lines exactly as they are.**
- **Invariant #6 (never fabricate a reading)** — a timeout must map to `None`/empty, never to a zero or a default value.
- **Invariant #2 (wire types stay additive)** — never add `deny_unknown_fields` anywhere.
- Milestone full-suite runs after Task 5 and Task 10, plus a final one after Task 11.

---

### Task 1: B1 — escape `#` in segment text before it becomes tmux markup

**Files:**
- Modify: `crates/rustline-core/src/render.rs` (add helper; apply at lines 172-178, 253-260, 285-296)
- Modify: `crates/rustline-core/src/ansi.rs:40-77` (`scan`)
- Test: `crates/rustline-core/src/render.rs` (in-module `mod tests`), `crates/rustline-core/src/ansi.rs` (in-module `mod tests`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `fn sanitize_text(s: &str) -> std::borrow::Cow<'_, str>` — private to `render.rs`. Task 2 extends this same function's body. Named `sanitize_text` (not `escape_markup`) precisely because Task 2 adds control-character handling to it.

**Background you need:** tmux parses `#[...]` directives inside the output of a `#(shell-command)` job — that is how rustline colours the bar at all. Segment text is interpolated raw into that markup today, so any `#` in a window name, a git branch, a now-playing title, a directory name, or a WASM plugin's `Segment.text` is live markup. tmux's documented escape for a literal `#` is `##`.

- [x] **Step 1: Verify empirically that `##` collapses in `#()` output — ALREADY DONE BY THE CONTROLLER, DO NOT REDO**

**Result on tmux 3.7b (this machine), captured through a real pty client:**

| status-right `#()` output | bytes tmux actually drew | meaning |
|---|---|---|
| `A#[bg=red]B` (unescaped) | `A\033[41mB` | directive **fires** — the vulnerability, confirmed live |
| `A##[bg=red]B` (escaped) | `A#[bg=red]B` | literal text, **no SGR** — the escape **works** |

So **use Step 3a (double the `#`)**. The Step 3b fallback is NOT needed — it is
retained below only as a record of the contingency. Skip the commands in this
step; they are kept for reproducibility.

Reproduction, for the record (note `capture-pane` does NOT capture the status
line — a pty client is required, and `cat -v` may be aliased to `bat`):

```bash
tmux -V
printf '#!/bin/sh\nprintf "a##[bg=red]b"\n' > /tmp/rl-esc-test.sh && chmod +x /tmp/rl-esc-test.sh
tmux new-session -d -s rlesc 'sleep 30'
tmux set-option -t rlesc status-right '#(/tmp/rl-esc-test.sh)'
tmux set-option -t rlesc status-right-length 40
sleep 1
tmux capture-pane -p -t rlesc -S -0 2>/dev/null | tail -2
tmux display-message -p -t rlesc '#{T:status-right}'
```

Expected if `##` works: the status shows the literal text `a#[bg=red]b` with no colour change.

If `##` does NOT collapse there, STOP and use the fallback in Step 3b instead of Step 3a. Record which variant you used. Clean up with `tmux kill-session -t rlesc`.

- [ ] **Step 2: Write the failing tests**

Add to `crates/rustline-core/src/render.rs`'s `mod tests`:

```rust
#[test]
fn segment_text_hash_is_escaped_not_a_directive() {
    let theme = Theme::default();
    let segs = vec![Segment::new("#[bg=red]")];
    let out = render_region(Direction::Right, &segs, &theme);
    // The literal text must be escaped, so tmux draws it instead of obeying it.
    assert!(out.contains("##[bg=red]"), "got: {out}");
    // Exactly the directives the renderer itself emitted, and no more:
    // one style directive for the single segment, plus the trailing reset.
    assert_eq!(count_real_directives(&out), 2, "got: {out}");
}

#[test]
fn guest_text_cannot_forge_a_range() {
    let theme = Theme::default();
    let groups = vec![RangeGroup {
        range: Some("weather".to_string()),
        segments: vec![Segment::new("#[norange]#[range=user|cpu]x")],
    }];
    let out = render_region_ranged(Direction::Right, &groups, &theme);
    // The only range in the output is the one the renderer opened.
    assert_eq!(out.matches("#[range=user|").count(), 1, "got: {out}");
    assert!(!out.contains("#[range=user|cpu]"), "got: {out}");
    assert_eq!(out.matches("#[norange]").count(), 1, "got: {out}");
}

#[test]
fn window_pill_text_hash_is_escaped() {
    let theme = Theme::default();
    let out = render_window_pill("#[bg=red]win", true, &theme);
    assert!(out.contains("##[bg=red]win"), "got: {out}");
}

/// Count directives the RENDERER emitted, i.e. `#[` occurrences that are not
/// part of an escaped `##`. Walks bytes so an escaped `##[` is skipped whole.
fn count_real_directives(s: &str) -> usize {
    let b = s.as_bytes();
    let (mut i, mut n) = (0usize, 0usize);
    while i + 1 < b.len() {
        if b[i] == b'#' && b[i + 1] == b'#' {
            i += 2; // escaped literal '#': skip both, never counts
        } else if b[i] == b'#' && b[i + 1] == b'[' {
            n += 1;
            i += 2;
        } else {
            i += 1;
        }
    }
    n
}
```

Add to `crates/rustline-core/src/ansi.rs`'s `mod tests`:

```rust
#[test]
fn escaped_hash_collapses_to_one_literal_hash() {
    // `##` is a literal '#', not the start of a directive.
    assert_eq!(tmux_to_ansi("a##[bg=red]b"), "a#[bg=red]b");
    let spans = parse_markup("a##[bg=red]b");
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].text, "a#[bg=red]b");
    assert_eq!(spans[0].style, Style::default());
}

#[test]
fn a_real_directive_after_an_escaped_hash_still_applies() {
    let spans = parse_markup("##[x]#[fg=red]y");
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].text, "#[x]");
    assert_eq!(spans[0].style.fg, None);
    assert_eq!(spans[1].text, "y");
    assert_eq!(spans[1].style.fg, Some(Color::Named("red".into())));
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p rustline-core -- escaped_hash guest_text_cannot segment_text_hash window_pill_text_hash a_real_directive`
Expected: FAIL — the render tests fail on the missing `##`, the ansi tests fail because `scan` currently treats `##[bg=red]` as a literal `#` followed by a real directive.

- [ ] **Step 3a: Add the sanitizer to `render.rs` (the `##` variant)**

Add this immediately above `pub fn render_region`:

```rust
/// Make a widget's text safe to interpolate into tmux markup.
///
/// tmux parses `#[...]` directives inside the output of a `#()` job — that is
/// how this renderer colours the bar — so a `#` in *content* is live markup.
/// Content reaching here is frequently external and untrusted: a tmux window
/// name, a git branch from a cloned repo, an MPRIS now-playing title, a
/// directory name, or a WASM guest's `Segment.text`. Doubling `#` is tmux's
/// documented escape for a literal `#`, so the text is drawn rather than
/// obeyed. Without this, a zero-capability plugin can emit
/// `#[norange]#[range=user|cpu]` and forge another widget's clickable range,
/// which `rustline click` then dispatches as that widget's binding.
///
/// Only segment TEXT goes through this. The separator, edge, style and
/// `range=user|` bytes are produced by the renderer itself, not by content,
/// and are written unescaped.
///
/// Borrows when there is nothing to change, so the common path allocates nothing.
fn sanitize_text(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains('#') {
        return std::borrow::Cow::Borrowed(s);
    }
    std::borrow::Cow::Owned(s.replace('#', "##"))
}
```

- [ ] **Step 3b: FALLBACK — only if Step 1 showed `##` does not collapse**

Replace the body above with a variant that neutralizes the directive opener instead of doubling, and adjust the Step 2 render assertions to match (`assert!(!out.contains("#[bg=red]"))` in place of the `##[bg=red]` assertions), leaving the ansi.rs change out entirely:

```rust
fn sanitize_text(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains("#[") {
        return std::borrow::Cow::Borrowed(s);
    }
    // A '#' only starts a directive when followed by '['. Break that pair.
    std::borrow::Cow::Owned(s.replace("#[", "#\u{200b}["))
}
```

- [ ] **Step 4: Apply it at the three emission sites**

In `render_region`, change the segment write (currently `s.text,`) to:

```rust
        let bold = if s.style.bold { ",bold" } else { "" };
        let _ = write!(
            out,
            "#[fg={},bg={}{bold}] {} ",
            eff_fg(s, theme).to_tmux(),
            cur_bg.to_tmux(),
            sanitize_text(&s.text),
        );
```

In `render_region_ranged`, make the identical change to its segment write (the one inside `for (i, s) in group.segments.iter().enumerate()`).

In `render_window_pill`, change the signature body so `text` is sanitized before the `format!`:

```rust
pub fn render_window_pill(text: &str, is_current: bool, theme: &Theme) -> String {
    let (pill, fg, bold) = if is_current {
        (&theme.win_current_bg, &theme.win_current_fg, ",bold")
    } else {
        (&theme.win_inactive_bg, &theme.win_inactive_fg, "")
    };
    let (pill, fg) = (pill.to_tmux(), fg.to_tmux());
    let bar = theme.bar_bg.to_tmux();
    let text = sanitize_text(text);
    format!(
        "#[fg={pill},bg={bar}]{cap_l}#[fg={fg},bg={pill}{bold}] {text} #[fg={pill},bg={bar}]{cap_r}#[default]",
        cap_l = theme.win_cap_left,
        cap_r = theme.win_cap_right,
    )
}
```

- [ ] **Step 5: Teach `ansi.rs::scan` about `##` (skip if you took Step 3b)**

In `scan`, insert an `##` arm BEFORE the existing `#[` arm — order matters, because `##[` is an escaped `#` followed by a literal `[`, not a directive:

```rust
    while let Some(c) = chars.next() {
        // `##` is tmux's escape for a literal '#' (see `render::sanitize_text`).
        // Checked BEFORE the `#[` arm: `##[` is an escaped '#' followed by a
        // literal '[', not a directive.
        if c == '#' && chars.peek() == Some(&'#') {
            chars.next(); // consume the second '#'
            text.push('#');
            continue;
        }
        // A style directive is the two-char sequence "#[" … "]".
        if c == '#' && chars.peek() == Some(&'[') {
```

Also update `scan`'s doc comment: replace the sentence "A `#` not followed by `[` is ordinary text (rustline emits bare `#` in window names and format strings)." with:

```rust
/// `##` is tmux's escape for a literal `#` and collapses to one `#` (this is
/// the inverse of `render::sanitize_text`, so a preview renders exactly what
/// tmux draws). A single `#` not followed by `[` or `#` is ordinary text.
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p rustline-core`
Expected: PASS, including the pre-existing byte-identity tests that assert `render_region_ranged` with all-`None` ranges equals `render_region`. If any pre-existing test now fails because it hardcoded a `#` inside segment text, that test is documenting the OLD unsafe behaviour — do not weaken it; report it and confirm the new expectation.

- [ ] **Step 7: Full verification**

```bash
cargo fmt --all
cargo build --workspace && just lint && cargo test --workspace
```
Expected: build clean, lint clean, ≥937 passing, 0 failing.

- [ ] **Step 8: Strip B1 from `bughunt.md` and commit**

Delete the entire `### B1. …` block through its `- [ ] execute   [ ] skip` line from `bughunt.md`.

```bash
git add -A
git commit -m "$(cat <<'EOF'
fix(security): escape # in segment text before it reaches tmux markup [B1]

Widget and plugin segment text was interpolated verbatim between #[...]
directives, and tmux parses those inside a #() job's output. Any # in a
window name, git branch, MPRIS title, directory name, or a WASM guest's
Segment.text was therefore live markup. A zero-capability plugin could emit
#[norange]#[range=user|cpu] to forge another widget's clickable range, which
rustline click then dispatches as that widget's binding (invariant #7, N1).

render::sanitize_text doubles # at the three emission sites (render_region,
render_region_ranged, render_window_pill); separators, edges and range bytes
stay unescaped since the renderer produces them. ansi.rs::scan collapses ##
back so --preview, theme show and the widget-edit preview strip agree with
what tmux draws.

Policy: escaping is central rather than per-substitution, because that is the
only variant covering plugin-supplied Segment.text — the highest-authority
vector. A plugin can no longer emit raw markup; that is the intent.

Claude-Session: https://claude.ai/code/session_01QUGuvDUj6gG5xQPiN3X2bG
EOF
)"
```

---

### Task 2: B3 — strip C0 control characters at the same sites

**Files:**
- Modify: `crates/rustline-core/src/render.rs` (extend `sanitize_text`)
- Test: `crates/rustline-core/src/render.rs` (in-module `mod tests`)

**Interfaces:**
- Consumes: `sanitize_text` from Task 1 — already applied at all three emission sites, so this task changes only its body.
- Produces: nothing new.

**Background you need:** tmux(1) FORMATS says "the last line of a shell command's output may be inserted using `#()`". rustline prints one unterminated line per region. A newline anywhere in segment text therefore makes tmux keep only the tail — silently deleting every widget to its left plus the opening style directive and leading edge glyph. Directory names and tmux window names can both legally contain `\n`. A tab is the milder sibling: `parse_list_windows` deliberately preserves tabs inside a window name (pinned by `windows.rs:70`), and a tab drawn into the status line jumps to the next tab stop, misaligning every pill after it.

- [ ] **Step 1: Write the failing tests**

Add to `crates/rustline-core/src/render.rs`'s `mod tests`:

```rust
#[test]
fn newline_in_segment_text_cannot_truncate_the_region() {
    let theme = Theme::default();
    let segs = vec![Segment::new("a"), Segment::new("x\n#[bg=red]y")];
    let out = render_region(Direction::Right, &segs, &theme);
    assert!(!out.contains('\n'), "region must stay single-line: {out:?}");
    // The first segment survives — tmux would otherwise drop everything left
    // of the newline, including the opening directive and the edge glyph.
    assert!(out.contains(" a "), "got: {out}");
}

#[test]
fn tab_and_carriage_return_are_replaced() {
    let theme = Theme::default();
    let segs = vec![Segment::new("a\tb\rc")];
    let out = render_region(Direction::Right, &segs, &theme);
    assert!(!out.contains('\t'), "got: {out:?}");
    assert!(!out.contains('\r'), "got: {out:?}");
    assert!(out.contains("a b c"), "got: {out}");
}

#[test]
fn window_pill_text_controls_are_replaced() {
    let theme = Theme::default();
    let out = render_window_pill("a\nb", false, &theme);
    assert!(!out.contains('\n'), "got: {out:?}");
    assert!(out.contains("a b"), "got: {out}");
}

#[test]
fn ordinary_text_is_untouched_and_still_borrows() {
    // The common path must not allocate or alter anything.
    let s = "cpu 42%";
    assert!(matches!(sanitize_text(s), std::borrow::Cow::Borrowed(_)));
    assert_eq!(sanitize_text(s), "cpu 42%");
    // Multi-byte glyphs the bar is full of must pass through unchanged.
    let glyphs = "\u{e0b0}\u{f011b} ~/src";
    assert_eq!(sanitize_text(glyphs), glyphs);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rustline-core -- newline_in_segment tab_and_carriage window_pill_text_controls ordinary_text_is_untouched`
Expected: FAIL — the control characters currently pass through verbatim.

- [ ] **Step 3: Extend `sanitize_text`**

Replace the Task 1 body with:

```rust
/// Make a widget's text safe to interpolate into tmux markup.
///
/// Two distinct hazards, one pass:
///
/// 1. **`#` is live markup.** tmux parses `#[...]` directives inside the
///    output of a `#()` job — that is how this renderer colours the bar — so a
///    `#` in *content* is a directive. Content reaching here is frequently
///    external and untrusted: a tmux window name, a git branch from a cloned
///    repo, an MPRIS now-playing title, a directory name, or a WASM guest's
///    `Segment.text`. Doubling `#` is tmux's documented escape for a literal
///    `#`. Without it a zero-capability plugin can emit
///    `#[norange]#[range=user|cpu]` and forge another widget's clickable
///    range, which `rustline click` then dispatches as that widget's binding.
///
/// 2. **A control character truncates or misaligns the region.** tmux inserts
///    only the LAST line of a `#()` job's output, and this renderer prints one
///    unterminated line per region — so a `\n` in content silently deletes
///    every widget to its left along with the opening style directive and the
///    leading edge glyph. A `\t` jumps to the next tab stop and misaligns
///    every pill after it (window names legitimately carry tabs — see
///    `parse_list_windows`, which preserves them on purpose).
///
/// Sanitizing at the render boundary rather than in each reader is deliberate:
/// it is the only place that also covers plugin-supplied `Segment.text`, which
/// no reader touches.
///
/// Only segment TEXT goes through this. The separator, edge, style and
/// `range=user|` bytes are produced by the renderer itself, not by content,
/// and are written unescaped.
///
/// Borrows when there is nothing to change, so the common path allocates nothing.
fn sanitize_text(s: &str) -> std::borrow::Cow<'_, str> {
    let needs_work = s.chars().any(|c| c == '#' || c.is_control());
    if !needs_work {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '#' => out.push_str("##"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    std::borrow::Cow::Owned(out)
}
```

If you took Task 1's Step 3b fallback, keep that task's `#`-handling arm here instead of `'#' => out.push_str("##")`, but keep the control-character arm exactly as written.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rustline-core`
Expected: PASS, including every Task 1 test.

- [ ] **Step 5: Full verification**

```bash
cargo fmt --all
cargo build --workspace && just lint && cargo test --workspace
```
Expected: build clean, lint clean, 0 failing.

- [ ] **Step 6: Strip B3 from `bughunt.md` and commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
fix(frontend): replace control characters in segment text at the render boundary [B3]

tmux inserts only the LAST line of a #() job's output and rustline prints one
unterminated line per region, so a \n anywhere in segment text silently
deleted every widget to its left along with the opening style directive and
the leading edge glyph. Reachable from a directory name (legal on Linux) and
from `tmux rename-window $'a\nb'`. A \t jumps to the next tab stop and
misaligns every pill after it; parse_list_windows preserves tabs in window
names on purpose, so that path was live too.

Extends render::sanitize_text (added for B1) to replace char::is_control()
with a space, at the same three emission sites. Kept at the render boundary
rather than in each reader so plugin-supplied Segment.text is covered too.

Claude-Session: https://claude.ai/code/session_01QUGuvDUj6gG5xQPiN3X2bG
EOF
)"
```

---

### Task 3: B2 — make silent reader failures diagnosable

**Files:**
- Modify: `crates/rustline/src/git.rs:12-24`, `media.rs:39-49`, `battery.rs`, `disk.rs`, `uptime.rs`, `throughput.rs`, `cpu.rs`, `memory.rs`, `windows.rs:12-30` (add `tracing::debug!` at failure returns)
- Modify: `crates/rustline/src/doctor.rs` (add `check_readers`, extend the `checks` array, extend `DoctorPaths`)
- Modify: `crates/rustline/src/main.rs:326-333` (pass the new `DoctorPaths` field)
- Test: `crates/rustline/src/doctor.rs` (in-module `mod tests`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `fn check_readers(cfg: &Config, layout: &[String]) -> Check` in `doctor.rs`; `DoctorPaths` gains `pub config_ref: &'a Config`. Task 4 extends the same reader failure sites with a timeout cause.

**Background you need:** ten readers collapse every failure into `None`/empty via `.ok()?` with no logging at all, and each widget then renders `down_format`, which defaults to `""`. A user whose tmux server started without `git` on `$PATH` sees the git widget render nothing, with an empty log — indistinguishable from "not in layout", "clean repo", or "config didn't parse". `doctor` never invokes any reader, so there is no diagnostic channel.

- [ ] **Step 1: Write the failing test**

Add to `crates/rustline/src/doctor.rs`'s `mod tests`:

```rust
#[test]
fn reader_check_never_fails_the_run() {
    // doctor's exit code is reserved for setup that is outright broken. A
    // reader returning None is frequently legitimate (no battery, not in a
    // repo, no player running), so this row must never produce Fail — the
    // same rule check_plugin_checksums follows.
    let cfg = Config::default();
    for layout in [
        vec![],
        vec!["cpu".to_string(), "memory".to_string()],
        vec!["git".to_string(), "media".to_string(), "battery".to_string()],
    ] {
        let check = check_readers(&cfg, &layout);
        assert_ne!(check.status, CheckStatus::Fail, "layout {layout:?}");
        assert_eq!(check.name, "widget readers");
    }
}

#[test]
fn reader_check_reports_nothing_to_probe_for_an_empty_layout() {
    let cfg = Config::default();
    let check = check_readers(&cfg, &[]);
    assert_eq!(check.status, CheckStatus::Ok);
    assert!(check.detail.contains("no readers"), "got: {}", check.detail);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rustline -- reader_check`
Expected: FAIL with "cannot find function `check_readers`".

- [ ] **Step 3: Add `debug!` at every reader failure return**

The pattern at each site: convert the silent `.ok()?` / early return into a logged one. `debug!` specifically — NOT `warn!` — because these run once per render and `warn!` passes the default `info` file level, which would rotate the log daily (that is exactly finding B8-adjacent problem the unselected warn-volume marker describes).

`crates/rustline/src/git.rs` — replace the body of `read_git`:

```rust
pub fn read_git(path: &str) -> Option<GitInfo> {
    let output = match std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain=v2", "--branch"])
        .output()
    {
        Ok(o) => o,
        Err(error) => {
            tracing::debug!(reader = "git", %error, path, "git spawn failed");
            return None;
        }
    };
    if !output.status.success() {
        tracing::debug!(
            reader = "git",
            code = output.status.code(),
            path,
            "git exited non-zero (not a repository?)"
        );
        return None;
    }
    let stdout = String::from_utf8(output.stdout)
        .inspect_err(|error| tracing::debug!(reader = "git", %error, "git output was not utf-8"))
        .ok()?;
    Some(parse_git_status(&stdout))
}
```

`crates/rustline/src/media.rs` — the Linux `read_media`:

```rust
#[cfg(target_os = "linux")]
pub fn read_media() -> Option<MediaInfo> {
    let output = match std::process::Command::new("playerctl")
        .args(["metadata", "--format", "{{artist}}\t{{title}}\t{{status}}"])
        .output()
    {
        Ok(o) => o,
        Err(error) => {
            tracing::debug!(reader = "media", %error, "playerctl spawn failed");
            return None;
        }
    };
    if !output.status.success() {
        tracing::debug!(
            reader = "media",
            code = output.status.code(),
            "playerctl exited non-zero (no player running?)"
        );
        return None;
    }
    parse_playerctl(&String::from_utf8_lossy(&output.stdout))
}
```

Add to the non-Linux `read_media`, and to every other platform-unsupported arm you touch:

```rust
#[cfg(not(target_os = "linux"))]
pub fn read_media() -> Option<MediaInfo> {
    tracing::debug!(reader = "media", "unsupported platform");
    None
}
```

`crates/rustline/src/windows.rs` — `read_windows`:

```rust
    let out = match cmd.output() {
        Ok(o) => o,
        Err(error) => {
            tracing::debug!(reader = "windows", %error, "tmux list-windows spawn failed");
            return Vec::new();
        }
    };
    if !out.status.success() {
        tracing::debug!(
            reader = "windows",
            code = out.status.code(),
            "tmux list-windows exited non-zero"
        );
        return Vec::new();
    }
```

Apply the same shape at the failure returns in `battery.rs` (`read_battery`), `disk.rs` (`read_disk` — log the `mount`), `uptime.rs` (`read_uptime`), `throughput.rs` (`read_throughput` — note its documented `None`-on-first-invocation case is NOT a failure; log that one as `"no prior sample yet"` so it reads as expected rather than broken), `cpu.rs` (`read_cpu`) and `memory.rs` (`read_memory`). Use `reader = "<name>"` as the first field in every one so `-vvv` output greps cleanly.

**Do not change any reader's return value, signature, or gating.** This step is additive logging only.

- [ ] **Step 4: Add the `doctor` row**

In `crates/rustline/src/doctor.rs`, add:

```rust
/// Probe every reader whose widget kind is actually in the layout and report
/// which ones currently yield nothing. This is the only diagnostic channel for
/// a reader failure: each one degrades to `down_format` (default `""`), so a
/// missing `git`/`playerctl` binary, an unmountable `[widgets.disk].mount`, or
/// a tmux that won't list windows all look identical to "widget not
/// configured" in the rendered bar.
///
/// **Never `Fail`.** `doctor`'s exit code is reserved for setup that is
/// outright broken, and a `None` reading is frequently legitimate — no
/// battery, not inside a repository, no media player running. Same rule
/// [`check_plugin_checksums`] follows.
fn check_readers(cfg: &Config, layout: &[String]) -> Check {
    const NAME: &str = "widget readers";
    let kinds = cfg.layout_kinds(layout);

    // (kind, is-currently-readable). Only probe what the layout actually uses,
    // so `doctor` never pays for a reader the user doesn't have configured.
    let mut probed: Vec<(&str, bool)> = Vec::new();
    if kinds.contains("git") {
        probed.push(("git", crate::git::read_git(".").is_some()));
    }
    if kinds.contains("media") {
        probed.push(("media", crate::media::read_media().is_some()));
    }
    if kinds.contains("battery") {
        probed.push(("battery", crate::battery::read_battery().is_some()));
    }
    if kinds.contains("uptime") {
        probed.push(("uptime", crate::uptime::read_uptime().is_some()));
    }
    if kinds.contains("cpu") {
        probed.push(("cpu", crate::cpu::read_cpu().is_some()));
    }
    if kinds.contains("memory") {
        probed.push(("memory", crate::memory::read_memory().is_some()));
    }
    if kinds.contains("disk") {
        for mount in cfg.disk_mounts(layout) {
            probed.push((
                "disk",
                crate::disk::read_disk(&mount).is_some(),
            ));
        }
    }

    if probed.is_empty() {
        return Check {
            name: NAME,
            status: CheckStatus::Ok,
            detail: "no readers in the active layout".to_string(),
        };
    }

    let quiet: Vec<&str> = probed
        .iter()
        .filter(|(_, ok)| !ok)
        .map(|(kind, _)| *kind)
        .collect();
    let ok_count = probed.len() - quiet.len();

    if quiet.is_empty() {
        Check {
            name: NAME,
            status: CheckStatus::Ok,
            detail: format!("{ok_count} reading"),
        }
    } else {
        Check {
            name: NAME,
            status: CheckStatus::Warn,
            detail: format!(
                "{ok_count} reading; nothing to report from: {} (run with -vvv for the cause)",
                quiet.join(", ")
            ),
        }
    }
}
```

Note `read_git(".")` deliberately probes the current directory rather than a pane path — `doctor` has no pane context, and the point is to detect a missing/broken `git`, not to report on a specific repo.

Extend `DoctorPaths` with the config it needs:

```rust
pub(crate) struct DoctorPaths<'a> {
    pub config: &'a Path,
    pub themes_dir: &'a Path,
    pub plugin_dir: &'a Path,
    pub log_file: &'a Path,
    pub tmux_conf: &'a Path,
    /// The configured `[plugins.*]` table, so the checksum row (see
    /// [`check_plugin_checksums`]) can report on every plugin the user has
    /// configured, not just ones that happen to be discovered on disk.
    pub plugins: &'a HashMap<String, PluginConfig>,
    /// The whole effective config, so [`check_readers`] can resolve which
    /// reader kinds the active layout actually uses (including
    /// `[instances.*]`) rather than guessing from widget names.
    pub cfg: &'a Config,
}
```

In `run`, add the row to the `checks` array (it becomes 13 elements) right after `check_plugin_checksums`:

```rust
        check_plugin_checksums(paths.plugins, paths.plugin_dir),
        check_readers(paths.cfg, &paths.cfg.layout.right),
```

In `crates/rustline/src/main.rs:326-333`, add the field:

```rust
            let paths = doctor::DoctorPaths {
                config: &cfg_path,
                themes_dir: &themes_dir(),
                plugin_dir: &plugin_dir,
                log_file: &logging::log_path(&cfg.log),
                tmux_conf: &tmux_conf_path(),
                plugins: &cfg.plugins,
                cfg: &cfg,
            };
```

If the borrow checker objects to `plugins: &cfg.plugins` alongside `cfg: &cfg` (both are shared borrows, so it should not), drop the `plugins` field and have `check_plugin_checksums` read `paths.cfg.plugins` instead.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rustline -- reader_check`
Expected: PASS.

- [ ] **Step 6: Verify the logging actually appears**

```bash
cargo run -q -- -vvv render right --pane-path=/nonexistent 2>&1 | head -20
cargo run -q -- doctor 2>&1 | grep -i 'widget readers'
```
Expected: `doctor` prints a `widget readers` row; the `-vvv` run emits `reader=` debug lines. Confirm a plain `cargo run -q -- render right` (no `-v`) emits NO reader lines to stderr.

- [ ] **Step 7: Full verification**

```bash
cargo fmt --all
cargo build --workspace && just lint && cargo test --workspace
```

- [ ] **Step 8: Strip B2 from `bughunt.md` and commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
fix(observability): make silent reader failures diagnosable [B2]

read_git/read_media/read_battery/read_disk/read_uptime/read_throughput/
read_cpu/read_memory/read_windows all collapsed every failure into None or an
empty Vec via .ok()? with no tracing call, and each widget then renders
down_format (default ""). A tmux server started without git on $PATH left the
git widget blank with an entirely empty log — indistinguishable from "not in
layout", "clean repo", or "config didn't parse". doctor never invoked a
reader, so there was no diagnostic channel at all.

Adds a debug! at each failure return naming the reader and the concrete cause
(debug!, not warn!, so -vvv reproduces it on demand without writing a line per
render at the default info level), plus a `widget readers` doctor row that
probes only the kinds in the active layout. Like check_plugin_checksums, that
row is coded so it can never produce Fail — doctor's exit code stays reserved
for setup that is outright broken, and a None reading is often legitimate.

No reader's return value or gating changed.

Claude-Session: https://claude.ai/code/session_01QUGuvDUj6gG5xQPiN3X2bG
EOF
)"
```

---

### Task 4: B5 — bound the render-path subprocess reads

**Files:**
- Modify: `crates/rustline-wasm/src/run.rs` (extract `run_bounded`)
- Modify: `crates/rustline-wasm/src/lib.rs` (re-export)
- Modify: `crates/rustline/src/git.rs`, `media.rs`, `windows.rs` (call sites)
- Test: `crates/rustline-wasm/src/run.rs` (in-module `mod tests`)

**Interfaces:**
- Consumes: Task 3's `debug!` lines at these three readers — extend them with a timeout cause, do not delete them.
- Produces: `pub fn run_bounded(program: &str, args: &[String], timeout: Duration) -> Result<(i32, String, String), String>` in `rustline_wasm::run`, re-exported as `rustline_wasm::run_bounded`.

**Background you need:** `read_git`, `read_media` and `read_windows` all call `Command::output()`, which blocks until the child exits, with no wall-clock bound. `pane_current_path` comes from tmux and is fully user-influenced, so with the git widget in the layout and a pane cwd on an unresponsive NFS/sshfs mount, `git status` blocks forever and the region goes permanently blank. Under the daemon it is worse: `handle_request` holds the `Mutex<DaemonState>` across `st.render(...)`, so one hung read pins the shared render lock for the life of the process. `rustline-wasm::run::ProcessRunner` already solves exactly this for the exec capability (spawn + `EXEC_TIMEOUT` + `kill_group` + output caps), and `rustline` already depends on `rustline-wasm`.

- [ ] **Step 1: Write the failing test**

Add to `crates/rustline-wasm/src/run.rs`'s `mod tests`:

```rust
#[test]
fn run_bounded_times_out_instead_of_hanging() {
    let start = std::time::Instant::now();
    let result = run_bounded("sleep", &["30".to_string()], Duration::from_millis(300));
    let elapsed = start.elapsed();
    assert!(result.is_err(), "a command past its deadline must be Err");
    assert!(
        elapsed < Duration::from_secs(5),
        "must not wait for the child: took {elapsed:?}"
    );
}

#[test]
fn run_bounded_returns_output_and_status_for_a_fast_command() {
    let (code, stdout, _stderr) =
        run_bounded("echo", &["hi".to_string()], Duration::from_secs(5)).expect("echo runs");
    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), "hi");
}

#[test]
fn run_bounded_reports_a_nonzero_exit_as_data_not_an_error() {
    // A non-zero exit means the process ran and reported that status; only a
    // run that could not happen at all is Err. Same convention as Runner::run.
    let (code, _, _) = run_bounded("false", &[], Duration::from_secs(5)).expect("false runs");
    assert_ne!(code, 0);
}

#[test]
fn run_bounded_errors_when_the_program_is_missing() {
    assert!(run_bounded("rustline-no-such-binary-xyz", &[], Duration::from_secs(5)).is_err());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rustline-wasm -- run_bounded`
Expected: FAIL with "cannot find function `run_bounded`".

- [ ] **Step 3: Extract `run_bounded` from `ProcessRunner::run`**

`ProcessRunner::run` currently hardcodes `EXEC_TIMEOUT`. Parameterize the timeout by moving the existing body into a free function and having the trait impl delegate. In `crates/rustline-wasm/src/run.rs`:

```rust
impl Runner for ProcessRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<(i32, String, String), String> {
        run_bounded(program, args, EXEC_TIMEOUT)
    }
}

/// Spawn `program` with `args`, bounded by `timeout`, and return
/// `(exit_code, stdout, stderr)`.
///
/// This is `ProcessRunner::run`'s body with the deadline parameterized, made
/// public so the bin's own render-path readers (`git.rs`, `media.rs`,
/// `windows.rs`) get the same hardening the exec capability already had:
/// **no shell anywhere in the path**, stdin closed (`Stdio::null()`, so a
/// child that reads stdin gets EOF instead of hanging), stdout/stderr piped
/// and capped at [`MAX_OUTPUT_BYTES`] per stream, the child in its own process
/// group so a backgrounded descendant can't hold the pipes open past the
/// deadline, and at most two [`OUTPUT_GRACE`] periods spent collecting output
/// afterward.
///
/// `Ok((code, ..))` whenever the process ran to completion — a non-zero exit
/// is data, not an error, and killed-by-signal maps to `-1`. `Err(message)`
/// only when the run could not happen at all: a spawn failure or the timeout.
pub fn run_bounded(
    program: &str,
    args: &[String],
    timeout: Duration,
) -> Result<(i32, String, String), String> {
    // ... the entire existing body of ProcessRunner::run, moved verbatim,
    // with the two references to EXEC_TIMEOUT replaced by `timeout`:
    //   let deadline = Instant::now() + timeout;
    //   ... format!("{program} exceeded the {}s exec timeout", timeout.as_secs())
    // Change that message to read "{program} timed out after {:?}", timeout
    // so it is accurate for a sub-second budget too.
}
```

Move the body verbatim. Do **not** alter the spawn flags, the reader-thread structure, the deadline handling, `kill_and_reap`, `kill_group`, or any SAFETY comment. Note finding B39 (open, unselected) concerns `kill_group` being called on an already-reaped pid — do not fix it here and do not restructure that path.

Re-export in `crates/rustline-wasm/src/lib.rs` alongside the existing re-exports:

```rust
pub use run::run_bounded;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rustline-wasm`
Expected: PASS, including every pre-existing exec-capability test (they now exercise `run_bounded` through the delegating `Runner` impl).

- [ ] **Step 5: Swap the three call sites**

Add to `crates/rustline/src/git.rs` (and mirror in the other two):

```rust
/// Wall-clock budget for a render-path subprocess read. Comfortably under a
/// 1 s `status-interval` so a wedged `git`/`playerctl`/`tmux` degrades to
/// `down_format` within one tick instead of blocking the region forever — and,
/// under the daemon, instead of pinning the shared render lock (`daemon.rs`'s
/// `handle_request` holds it across the whole render).
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);
```

`read_git`:

```rust
pub fn read_git(path: &str) -> Option<GitInfo> {
    let args = [
        "-C".to_string(),
        path.to_string(),
        "status".to_string(),
        "--porcelain=v2".to_string(),
        "--branch".to_string(),
    ];
    let (code, stdout, _stderr) = match rustline_wasm::run_bounded("git", &args, READ_TIMEOUT) {
        Ok(t) => t,
        Err(error) => {
            // Covers both a spawn failure (git missing) and the timeout (a
            // pane cwd on an unresponsive network mount).
            tracing::debug!(reader = "git", %error, path, "git read failed");
            return None;
        }
    };
    if code != 0 {
        tracing::debug!(
            reader = "git",
            code,
            path,
            "git exited non-zero (not a repository?)"
        );
        return None;
    }
    Some(parse_git_status(&stdout))
}
```

`read_media` (Linux arm) — same shape, `"playerctl"` with
`["metadata".to_string(), "--format".to_string(), "{{artist}}\t{{title}}\t{{status}}".to_string()]`, then `parse_playerctl(&stdout)`.

`read_windows` — same shape, `"tmux"` with `list-windows`, the optional `["-t", s]`, and the `-F` format string, returning `Vec::new()` on `Err` or non-zero and `parse_list_windows(&stdout)` otherwise.

`run_bounded` already returns lossy-UTF-8 `String`s, so the `String::from_utf8`/`from_utf8_lossy` conversions at these sites go away. Watch for one behaviour change worth accepting: `read_git` previously used strict `String::from_utf8` and returned `None` on invalid UTF-8; it is now lossy. That is strictly more permissive and matches how `read_media`/`read_windows` already behaved.

- [ ] **Step 6: Verify the readers still work end to end**

```bash
cargo run -q -- render right --pane-path="$PWD" --preview
cargo run -q -- render windows 2>/dev/null | head -3
cargo test --workspace
```
Expected: the git widget still renders this repo's branch; no test regressions.

- [ ] **Step 7: Full verification**

```bash
cargo fmt --all
cargo build --workspace && just lint && cargo test --workspace
```

- [ ] **Step 8: Strip B5 from `bughunt.md` and commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
fix(api-surface): bound the render-path subprocess reads with a timeout [B5]

read_git, read_media and read_windows all called Command::output(), which
blocks until the child exits with no wall-clock bound. pane_current_path comes
from tmux and is fully user-influenced, so with the git widget in the layout
and a pane cwd on an unresponsive NFS/sshfs mount, git status blocks forever
and the region goes permanently blank. Under the daemon it is worse:
handle_request holds Mutex<DaemonState> across st.render(...), so one hung
read pins the shared render lock for the life of the process.

Parameterizes ProcessRunner::run's deadline into a public run_bounded and
reuses it at the three call sites with a 500 ms budget. The exec capability
already had this hardening (spawn + timeout + process-group kill + output
caps); the built-in readers never got it. A timeout is treated exactly like
the existing non-zero-exit arm — None/empty, never a fabricated reading
(invariant #6) — and extends B2's debug! rather than replacing it.

ProcessRunner's spawn flags, reader threads, kill_group and SAFETY comments
are moved verbatim and otherwise untouched.

Claude-Session: https://claude.ai/code/session_01QUGuvDUj6gG5xQPiN3X2bG
EOF
)"
```

- [ ] **Step 9: MILESTONE — full suite**

```bash
cargo test --workspace 2>&1 | grep -E '^test result' 
just lint
```
Expected: 0 failing across every target; lint clean. On red: bisect within these four commits, revert the offender, and report the diagnosis before continuing.

---

### Task 5: B7 — give `WireContext` struct-level serde defaults

**Files:**
- Modify: `crates/rustline-abi/src/lib.rs:245-280` (`WireContext`), and `WireWindowCtx`
- Modify: `crates/rustline-plugin-sdk/src/lib.rs` (`render_with`'s `Err` arm)
- Test: `crates/rustline-abi/src/lib.rs` (in-module `mod tests`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `WireContext: Default`, `WireWindowCtx: Default`.

**Background you need:** `ABI_VERSION`'s doc claims an additive wire change needs no version bump because "serde already tolerates that on both sides". That is true only host→older-guest. `WireContext` marks only `cpu_history`, `mem_history`, `throughput`, `uptime`, `media`, `toggled` and `colors` as `#[serde(default)]`; `git`, `disk`, `os`, `arch`, `interfaces`, `battery`, `cpu`, `memory`, `loadavg`, `window`, and the six leading `String` fields are all required. A plugin built today against the SDK and run on a host predating `git`/`disk`/`os`/`arch` gets a Context JSON missing those keys, `from_str::<GuestRender>` fails, and `render_with` returns `Vec::new()` on `Err(_)` with no log — the widget silently disappears. `abi_decision` cannot catch it because both sides honestly report `ABI_VERSION == 1`.

- [ ] **Step 1: Write the failing test**

Add to `crates/rustline-abi/src/lib.rs`'s `mod tests` (create the module if absent):

```rust
#[test]
fn wire_context_decodes_from_an_older_host_missing_later_fields() {
    // A guest built against today's SDK, run on a host that predates
    // git/disk/os/arch/interfaces/battery/cpu/memory. Every absent field must
    // fall back to its default rather than failing the whole decode — a decode
    // failure makes the widget silently render empty.
    let json = r#"{"session_name":"0","window_index":"1","pane_index":"2",
        "pane_current_path":"/","home":"/home/x","hostname":"h",
        "loadavg":null,"now":"2026-07-26T00:00:00+00:00","window":null}"#;
    let ctx: WireContext = serde_json::from_str(json).expect("must decode");
    assert_eq!(ctx.hostname, "h");
    assert!(ctx.git.is_none());
    assert!(ctx.disk.is_none());
    assert_eq!(ctx.os, "");
    assert_eq!(ctx.arch, "");
    assert!(ctx.interfaces.is_empty());
}

#[test]
fn wire_context_decodes_from_an_empty_object() {
    // The degenerate case: struct-level default means even {} is decodable.
    let ctx: WireContext = serde_json::from_str("{}").expect("must decode");
    assert_eq!(ctx.session_name, "");
    assert!(ctx.window.is_none());
}

#[test]
fn guest_render_decodes_with_a_sparse_context() {
    let json = r#"{"context":{"hostname":"h"},"config":{"format":"x"}}"#;
    let g: GuestRender = serde_json::from_str(json).expect("must decode");
    assert_eq!(g.context.hostname, "h");
    assert_eq!(g.config["format"], "x");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rustline-abi -- wire_context guest_render_decodes`
Expected: FAIL with a serde "missing field" error.

- [ ] **Step 3: Add `Default` + struct-level `#[serde(default)]`**

On `WireContext`, change the derive line and add the attribute, and delete the now-redundant per-field `#[serde(default)]` attributes (the struct-level one covers them):

```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WireContext {
```

Do the same on `WireWindowCtx`. Every field is `String`/`Option`/`Vec`/`BTreeSet`/`bool`, so `Default` derives cleanly.

Extend the type's doc comment with the rationale:

```rust
/// Struct-level `#[serde(default)]` (matching `HttpResult`/`ExecResult` and
/// the other host-effect result types) makes the decode total in BOTH
/// directions of version skew. Without it, a guest built against a newer SDK
/// and run on an older host hit a "missing field" error on `git`/`disk`/
/// `os`/`arch`/…, `render_with` returned an empty `Vec`, and the widget
/// silently disappeared — with `abi_decision` unable to catch it, since both
/// sides honestly report the same `ABI_VERSION`. Still no
/// `deny_unknown_fields`: skew must stay total in the other direction too
/// (invariant #2).
```

- [ ] **Step 4: Log the decode failure in the SDK**

In `crates/rustline-plugin-sdk/src/lib.rs`, replace `render_with`'s `Err` arm:

```rust
pub fn render_with<F>(input: &str, f: F) -> Vec<Segment>
where
    F: FnOnce(&GuestRender) -> Vec<Segment>,
{
    match serde_json::from_str::<GuestRender>(input) {
        Ok(render) => f(&render),
        Err(error) => {
            // A silent empty render is indistinguishable from "the plugin had
            // nothing to show". Host/guest wire skew is the likeliest cause,
            // so say so through the host's own logger.
            log(
                LogLevel::Warn,
                &format!("could not decode the host's render input: {error}"),
            );
            Vec::new()
        }
    }
}
```

Check `log`/`LogLevel` are in scope in that module; they are defined in the same file. On the host target `log` degrades to `HostError::Unavailable` and is a no-op, so the existing `render_with_malformed_input_is_empty_never_panics` test still passes.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rustline-abi && cargo test -p rustline-plugin-sdk && cargo test -p rustline-wasm`
Expected: PASS. `rustline-wasm` matters here — it has a round-trip seam test pinning `Context`'s serialization against `WireContext`'s deserialization; it must still pass.

- [ ] **Step 6: Full verification**

```bash
cargo fmt --all
cargo build --workspace && just lint && cargo test --workspace
```

- [ ] **Step 7: Strip B7 from `bughunt.md` and commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
fix(api-surface): give WireContext struct-level serde defaults [B7]

ABI_VERSION's doc says an additive wire change needs no bump because "serde
already tolerates that on both sides" — true only host->older-guest.
WireContext marked just seven fields #[serde(default)]; git, disk, os, arch,
interfaces, battery, cpu, memory, loadavg, window and the six leading Strings
were all required. A plugin built against today's SDK, run on a host predating
those fields, failed to decode GuestRender entirely; render_with returned an
empty Vec and the widget silently disappeared, with abi_decision unable to
catch it since both sides honestly report ABI_VERSION == 1.

Derives Default and moves to struct-level #[serde(default)], matching the
discipline HttpResult/ExecResult already follow. No deny_unknown_fields
anywhere — skew stays total in both directions (invariant #2).

Also makes the SDK's render_with log the decode error through the host's own
logger instead of swallowing it.

Claude-Session: https://claude.ai/code/session_01QUGuvDUj6gG5xQPiN3X2bG
EOF
)"
```

- [ ] **Step 8: MILESTONE — full suite** (5 tasks done)

```bash
cargo test --workspace 2>&1 | grep -E '^test result'
just lint
```
Expected: 0 failing; lint clean. On red: bisect, revert the offender, report.

---

### Task 6: B8 — name the widget in the panic guard

**Files:**
- Modify: `crates/rustline-core/src/assemble.rs:29-40` (`render_guarded`), `:93`, `:127`
- Test: `crates/rustline-core/src/assemble.rs` (in-module `mod tests`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `fn render_guarded(name: &str, widget: &dyn Widget, ctx: &Context) -> Vec<Segment>` (private).

**Background you need:** the guard discards the payload with `Err(_)` and logs a bare `"widget panicked, skipping"` — no name, no message. The real panic text goes to stderr via the default hook, which under tmux is the tmux server's stderr (typically `/dev/null`), so it never reaches the log file. With six widgets in a bar the user cannot tell which died. CLAUDE.md already tracks "naming the widget in the panic-guard `warn!`" as an open roadmap item.

- [ ] **Step 1: Write the failing test**

Add to `crates/rustline-core/src/assemble.rs`'s `mod tests` (create the module if absent — check first; if it exists, append):

```rust
struct PanickingWidget;
impl Widget for PanickingWidget {
    fn render(&self, _ctx: &Context) -> Vec<Segment> {
        panic!("boom from the test widget");
    }
}

#[test]
fn panic_guard_degrades_to_empty_and_keeps_the_name() {
    // The guard must still contain the panic (invariant #6 / N2)...
    let segs = render_guarded("cpu", &PanickingWidget, &Context::default());
    assert!(segs.is_empty());
}

#[test]
fn panic_guard_extracts_the_payload_message() {
    // ...and the payload must be recoverable, since the default panic hook
    // writes it to stderr, which under tmux goes to the server's /dev/null.
    let payload: Box<dyn std::any::Any + Send> = Box::new("boom from the test widget");
    assert_eq!(panic_message(&payload), "boom from the test widget");
    let payload: Box<dyn std::any::Any + Send> = Box::new(String::from("owned boom"));
    assert_eq!(panic_message(&payload), "owned boom");
    let payload: Box<dyn std::any::Any + Send> = Box::new(42u8);
    assert_eq!(panic_message(&payload), "<non-string panic payload>");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rustline-core -- panic_guard`
Expected: FAIL — `render_guarded` takes two arguments, and `panic_message` does not exist.

- [ ] **Step 3: Implement**

Replace `render_guarded` and add the payload helper:

```rust
/// Extract a readable message from a panic payload. `panic!` produces either a
/// `&'static str` (a literal) or a `String` (a formatted message); anything
/// else is possible via `panic_any` but has no text to show.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> &str {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        s
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.as_str()
    } else {
        "<non-string panic payload>"
    }
}

/// Render a widget, converting a panic into an empty segment list plus a
/// warning instead of letting it unwind through the whole region. A single
/// misbehaving widget (built-in or plugin) must never take down the rest of
/// the status line.
///
/// `name` is the layout/registry name the widget was resolved under, and it is
/// logged: the default panic hook writes the real panic text to STDERR, which
/// under tmux is the tmux *server's* stderr (typically `/dev/null`), so the
/// log file is the only channel the user can actually consult. Without the
/// name, a six-widget bar loses one widget and the log says only that
/// "a widget" panicked.
fn render_guarded(name: &str, widget: &dyn Widget, ctx: &Context) -> Vec<Segment> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| widget.render(ctx))) {
        Ok(segments) => segments,
        Err(payload) => {
            tracing::warn!(
                widget = %name,
                panic = %panic_message(&payload),
                "widget panicked, skipping"
            );
            vec![]
        }
    }
}
```

If clippy objects to `&Box<dyn Any + Send>` (`clippy::borrowed_box`), change the parameter to `payload: &(dyn std::any::Any + Send)` and call it as `panic_message(payload.as_ref())` — adjust the test's construction to match.

Update the two call sites. In `render_named_region` (line ~93), inside the `.map(|(name, w)| {` closure:

```rust
            let mut segments = render_guarded(name, w.as_ref(), ctx);
```

In `window_pill` (line ~127):

```rust
    let segments: Vec<Segment> = widgets
        .iter()
        .flat_map(|(name, w)| render_guarded(name, w.as_ref(), ctx))
        .collect();
```

Note the second closure currently binds `(_, w)`; change it to `(name, w)`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rustline-core`
Expected: PASS.

- [ ] **Step 5: Full verification**

```bash
cargo fmt --all
cargo build --workspace && just lint && cargo test --workspace
```

- [ ] **Step 6: Strip B8 from `bughunt.md` and commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
fix(observability): name the widget and its panic payload in the guard [B8]

render_guarded discarded the payload with Err(_) and logged a bare
"widget panicked, skipping" — no name, no message, no location. The real panic
text is written by the default hook to STDERR, which under tmux is the tmux
server's stderr (typically /dev/null), so it never reached the log file, the
only channel the user can consult. With six widgets in a bar, one vanishing
was untraceable.

Threads the layout/registry name into the guard (both call sites already had
it — render_named_region destructures Registry::resolve's W53 pairs, and
window_pill's is "windows") and downcasts the payload to &str/String. Private
fns inside the crate, so no public API change. The guard still degrades to
empty segments (invariant #6 / N2 unchanged).

Closes the "naming the widget in the panic-guard warn!" roadmap item.

Claude-Session: https://claude.ai/code/session_01QUGuvDUj6gG5xQPiN3X2bG
EOF
)"
```

---

### Task 7: B9 — clamp `bar_width` / `spark_width` inside the renderers

**Files:**
- Modify: `crates/rustline-core/src/widgets/bar.rs:11-30` (`gauge_bar`)
- Modify: `crates/rustline-core/src/widgets/spark.rs:17-19` (`sparkline`)
- Modify: `crates/rustline/src/history.rs` (`push_truncate`)
- Test: `crates/rustline-core/src/widgets/bar.rs`, `spark.rs`, `crates/rustline/src/history.rs` (in-module `mod tests`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `pub(crate) const MAX_BAR_WIDTH: usize = 256;` in `bar.rs`; an equivalent cap in `spark.rs` and `history.rs`.

**Background you need:** `CpuOpts`/`MemoryOpts`/`DiskOpts` deserialize `bar_width: usize` with a serde default but no upper clamp, and TOML integers go to `i64::MAX`. `gauge_bar` then does `String::with_capacity(width * 3)` and pushes `full` chars in a loop. `[widgets.cpu] bar_width = 1000000000` allocates ~3 GB every render; at `usize::MAX`-scale values the `width * 8` and `width * 3` products wrap in release. This breaks invariant #3 ("a bad config must never break the bar") in the one way the `catch_unwind` guard cannot help with — it catches a panic, but not an allocation abort and not an unbounded loop, so the bar hangs instead of degrading.

- [ ] **Step 1: Write the failing tests**

Add to `crates/rustline-core/src/widgets/bar.rs`'s `mod tests`:

```rust
#[test]
fn absurd_width_is_clamped_not_allocated() {
    // A fat-fingered config must degrade, not hang or abort. catch_unwind
    // cannot rescue an allocation abort or an unbounded loop (invariant #3).
    let out = gauge_bar(0.5, 1_000_000_000);
    assert_eq!(out.chars().count(), MAX_BAR_WIDTH);
}

#[test]
fn usize_max_width_does_not_overflow() {
    // width * 8 and width * 3 would both wrap in release without the clamp.
    let out = gauge_bar(1.0, usize::MAX);
    assert_eq!(out.chars().count(), MAX_BAR_WIDTH);
}

#[test]
fn default_widths_are_byte_identical_to_before_the_clamp() {
    assert_eq!(gauge_bar(0.0, 8), "░░░░░░░░");
    assert_eq!(gauge_bar(1.0, 8), "████████");
    assert_eq!(gauge_bar(0.5, 8), "████░░░░");
    assert_eq!(gauge_bar(0.5, 0), "");
}
```

Add to `crates/rustline-core/src/widgets/spark.rs`'s `mod tests`:

```rust
#[test]
fn sparkline_output_length_follows_the_sample_count_only() {
    // sparkline maps one glyph per sample, so its own bound is the ring
    // length; the ring is what history::push_truncate caps.
    let samples = vec![50.0f32; 12];
    assert_eq!(sparkline(&samples, 100.0).chars().count(), 12);
}
```

Add to `crates/rustline/src/history.rs`'s `mod tests`:

```rust
#[test]
fn push_truncate_clamps_an_absurd_width() {
    let mut h: Vec<f32> = (0..10).map(|i| i as f32).collect();
    push_truncate(&mut h, 99.0, usize::MAX);
    assert!(
        h.len() <= MAX_HISTORY_WIDTH,
        "ring grew to {} entries",
        h.len()
    );
}

#[test]
fn push_truncate_default_width_is_unchanged() {
    let mut h: Vec<f32> = (0..8).map(|i| i as f32).collect();
    push_truncate(&mut h, 99.0, 8);
    assert_eq!(h.len(), 8);
    assert_eq!(h.last().copied(), Some(99.0));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rustline-core -- absurd_width usize_max_width && cargo test -p rustline -- push_truncate_clamps`
Expected: FAIL — `MAX_BAR_WIDTH`/`MAX_HISTORY_WIDTH` don't exist. Do **not** actually run the unclamped `usize::MAX` case against the old code; it would hang or abort. Trust the reasoning.

- [ ] **Step 3: Implement the clamps**

In `crates/rustline-core/src/widgets/bar.rs`, above `gauge_bar`:

```rust
/// Upper bound on a rendered gauge's cell count.
///
/// `bar_width` is a `usize` deserialized straight from user TOML with no
/// ceiling, and TOML integers reach `i64::MAX`. Without this clamp,
/// `[widgets.cpu] bar_width = 1000000000` allocates ~3 GB on every render and
/// an even larger value wraps the `width * 8` / `width * 3` products in
/// release. `render_named_region`'s `catch_unwind` guard cannot rescue either
/// — it catches a panic, not an allocation abort and not an unbounded loop —
/// so the bar would hang rather than degrade, breaking invariant #3 ("a bad
/// config must never break the bar").
///
/// Clamped here rather than at deserialization so the bound holds for every
/// caller regardless of how the value arrived. No real status line is anywhere
/// near this wide, so a legitimate configuration never notices.
pub(crate) const MAX_BAR_WIDTH: usize = 256;
```

And as `gauge_bar`'s first statement, after the `width == 0` early return:

```rust
pub(crate) fn gauge_bar(fraction: f64, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let width = width.min(MAX_BAR_WIDTH);
    let eighths = (fraction.clamp(0.0, 1.0) * (width * 8) as f64).round() as usize;
```

In `crates/rustline/src/history.rs`, add the constant and clamp inside `push_truncate`:

```rust
/// Upper bound on the persisted `{spark}` history ring, mirroring
/// `bar.rs`'s `MAX_BAR_WIDTH`: `spark_width` is unbounded user config, and the
/// ring is both allocated and written to disk on every render.
pub(crate) const MAX_HISTORY_WIDTH: usize = 256;
```

Then clamp `spark_width` at the top of `push_truncate` with `let spark_width = spark_width.min(MAX_HISTORY_WIDTH);`. Read the existing function first and preserve its exact push/truncate semantics — only the width bound changes.

`sparkline` itself needs no clamp: it emits one glyph per sample, so the ring length (now clamped) is its bound. Add a one-line comment saying so, so a future reader doesn't think it was missed.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rustline-core && cargo test -p rustline -- push_truncate`
Expected: PASS, including the pre-existing `empty_and_full` bar test — default-width output must be byte-identical.

- [ ] **Step 5: Verify a hostile config degrades instead of hanging**

```bash
TMPCFG=$(mktemp -d)
cat > "$TMPCFG/config.toml" <<'TOML'
[layout]
right = ["cpu"]
[widgets.cpu]
format = "{bar}"
bar_width = 1000000000
TOML
timeout 20 cargo run -q -- --config "$TMPCFG/config.toml" render right --preview; echo "exit=$?"
rm -rf "$TMPCFG"
```
Expected: returns promptly with a 256-cell bar, `exit=0`. A timeout (`exit=124`) means the clamp is not on the reachable path.

- [ ] **Step 6: Full verification**

```bash
cargo fmt --all
cargo build --workspace && just lint && cargo test --workspace
```

- [ ] **Step 7: Strip B9 from `bughunt.md` and commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
fix(api-surface): clamp bar_width and spark_width inside the renderers [B9]

bar_width/spark_width are usize values deserialized straight from user TOML
with no ceiling, and TOML integers reach i64::MAX. gauge_bar did
String::with_capacity(width * 3) plus a push loop, so
[widgets.cpu] bar_width = 1000000000 allocated ~3 GB every render, and a
larger value wrapped the width * 8 / width * 3 products in release.

render_named_region's catch_unwind cannot rescue either case — it catches a
panic, not an allocation abort and not an unbounded loop — so the bar hung
rather than degrading, breaking invariant #3 ("a bad config must never break
the bar").

Clamps at MAX_BAR_WIDTH/MAX_HISTORY_WIDTH (256) inside gauge_bar and
history::push_truncate rather than at deserialization, so the bound holds for
every caller regardless of how the value arrived. sparkline needs no clamp of
its own: it emits one glyph per sample and the ring is now bounded.
Default-width output is byte-identical.

Claude-Session: https://claude.ai/code/session_01QUGuvDUj6gG5xQPiN3X2bG
EOF
)"
```

---

### Task 8: B10 — log malformed plugin render output

**Files:**
- Modify: `crates/rustline-wasm/src/host.rs:223-235` (`WasmWidget::render`)
- Test: `crates/rustline-wasm/src/host.rs` (in-module `mod tests`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: nothing new. Task 9 edits the same function — do that one next while the code is fresh.

**Background you need:** `parse_render_output` does `serde_json::from_str(s).unwrap_or_default()`, turning any decode failure into an empty `Vec<Segment>`. The caller took the `Ok(out)` branch, so it logs nothing either — the guest ran fine and returned a string; only the parse failed. This is the only plugin failure mode with no distinguishing message, against ten distinct `warn!`s in `register_plugins`. B7 (Task 5) fixes one common guest-side cause; this makes the remaining causes visible.

**Constraint:** `parse_render_output` is `pub`. Do the match inline in `WasmWidget::render`, which already has `self.name` in scope, rather than changing the public signature.

- [ ] **Step 1: Write the failing test**

Add to `crates/rustline-wasm/src/host.rs`'s `mod tests`:

```rust
#[test]
fn parse_render_output_still_degrades_to_empty() {
    // The N2 guarantee is unchanged: malformed output never breaks the bar.
    assert!(parse_render_output("not json").is_empty());
    assert!(parse_render_output(r#"{"segments":[]}"#).is_empty());
    assert!(!parse_render_output(r#"[{"text":"hi","style":{}}]"#).is_empty());
}

#[test]
fn decode_render_output_reports_the_error_for_malformed_json() {
    // The distinguishing signal the log was missing: which plugin, and why.
    let (segs, err) = decode_render_output("weather", r#"{"segments":[]}"#);
    assert!(segs.is_empty());
    assert!(err.is_some(), "a decode failure must be reportable");

    let (segs, err) = decode_render_output("weather", r#"[{"text":"hi","style":{}}]"#);
    assert_eq!(segs.len(), 1);
    assert!(err.is_none());
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rustline-wasm -- decode_render_output`
Expected: FAIL with "cannot find function `decode_render_output`".

- [ ] **Step 3: Implement**

Add to `crates/rustline-wasm/src/host.rs`, near `plugin_range_name`:

```rust
/// Decode a guest's `render` output, returning the segments plus the decode
/// error when there was one.
///
/// Split out of [`WasmWidget::render`] so the decode outcome can be asserted by
/// a hermetic unit test without a real `extism::Plugin`, the same way
/// `plugin_range_name` is. `parse_render_output` stays as-is (it is `pub`, and
/// its `unwrap_or_default` contract — malformed output never breaks the bar,
/// invariant N2 — is unchanged); this only makes the failure *reportable*.
fn decode_render_output(name: &str, out: &str) -> (Vec<Segment>, Option<serde_json::Error>) {
    match serde_json::from_str::<Vec<Segment>>(out) {
        Ok(segments) => (segments, None),
        Err(error) => {
            tracing::warn!(
                plugin = %name,
                %error,
                len = out.len(),
                "malformed plugin render output, rendering empty"
            );
            (Vec::new(), Some(error))
        }
    }
}
```

In `WasmWidget::render`, change only the `Ok` arm of the `plugin.call` match:

```rust
        match plugin.call::<&str, &str>("render", &payload) {
            Ok(out) => decode_render_output(&self.name, out).0,
            Err(error) => {
                tracing::warn!(%error, "plugin render failed, rendering empty");
                Vec::new()
            }
        }
```

**Leave the `Err` arm's `tracing::warn!` exactly as written** — adding `plugin = %self.name` to it is finding B26, which is not in this batch.

You will need `use rustline_core::Segment;` in scope if it isn't already (it is — `WasmWidget::render` returns `Vec<Segment>`).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rustline-wasm`
Expected: PASS.

- [ ] **Step 5: Full verification**

```bash
cargo fmt --all
cargo build --workspace && just lint && cargo test --workspace
```

- [ ] **Step 6: Strip B10 from `bughunt.md` and commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
fix(observability): report malformed plugin render output [B10]

parse_render_output turned any decode failure into an empty Vec<Segment> via
unwrap_or_default, and the caller had taken the Ok branch — the guest ran fine
and returned a string; only the parse failed — so nothing logged anywhere.
This was the only plugin failure mode with no distinguishing message, against
ten distinct warn!s in register_plugins: an author who bumps their SDK and
emits {"segments":[...]} instead of a bare array just watches their widget
disappear with an empty log.

Adds decode_render_output, which logs the plugin name, the serde error and the
payload length before degrading. Split out as its own fn so the outcome is
unit-testable without a real extism::Plugin, mirroring plugin_range_name.
parse_render_output's public signature and its never-break-the-bar contract
(N2) are unchanged.

Claude-Session: https://claude.ai/code/session_01QUGuvDUj6gG5xQPiN3X2bG
EOF
)"
```

---

### Task 9: B11 — recover a poisoned plugin mutex

**Files:**
- Modify: `crates/rustline-wasm/src/host.rs:190-235` (`WasmWidget` struct + `render`)
- Test: `crates/rustline-wasm/src/host.rs` (in-module `mod tests`)

**Interfaces:**
- Consumes: Task 8's edits to the same function.
- Produces: `WasmWidget` gains a `poison_reported: Arc<AtomicBool>` field.

**Background you need:** `self.plugin.lock()` returns `Vec::new()` on `Err(_)` with no log. Poisoning means a prior render panicked while holding the lock. In the CLI path each render is a fresh process, so it is rare and self-healing; in the W48 daemon the `WasmWidget` is warm for the process lifetime, so ONE poisoning panic makes that plugin render empty on every subsequent tick, permanently, until the daemon restarts. `daemon.rs:180` already recovers ITS state mutex via `PoisonError::into_inner` for exactly this reason — the plugin mutex one level down does not.

- [ ] **Step 1: Write the failing test**

Add to `crates/rustline-wasm/src/host.rs`'s `mod tests`:

```rust
#[test]
fn a_poisoned_lock_is_recovered_rather_than_disabling_the_plugin() {
    use std::sync::{Arc, Mutex};

    // Model the exact shape WasmWidget uses: poison the mutex, then confirm
    // the recovery path still yields the guard. In the daemon the widget is
    // warm for the process lifetime, so bailing out on Err(_) meant one
    // panic killed that plugin permanently and silently.
    let m: Arc<Mutex<u32>> = Arc::new(Mutex::new(7));
    let m2 = Arc::clone(&m);
    let _ = std::thread::spawn(move || {
        let _g = m2.lock().unwrap();
        panic!("poison it");
    })
    .join();
    assert!(m.lock().is_err(), "the mutex must actually be poisoned");

    let guard = m.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(*guard, 7, "recovery must still hand back the value");
}

#[test]
fn poison_is_reported_only_once() {
    use std::sync::atomic::{AtomicBool, Ordering};
    // The daemon renders every tick; a poison warning must not repeat forever.
    let flag = AtomicBool::new(false);
    assert!(!flag.swap(true, Ordering::Relaxed), "first report happens");
    assert!(flag.swap(true, Ordering::Relaxed), "second is suppressed");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rustline-wasm -- a_poisoned_lock poison_is_reported`
Expected: these two pass immediately (they assert std behaviour and the intended pattern) — that is fine; they are the characterization half. The real RED signal is that `WasmWidget::render` still has the `Err(_) => return Vec::new()` arm. Confirm by grepping:

```bash
grep -n 'Err(_) => return Vec::new()' crates/rustline-wasm/src/host.rs
```
Expected: one hit, at the `self.plugin.lock()` match. That line is what Step 3 removes.

- [ ] **Step 3: Implement**

Add the field to the struct and constructor:

```rust
#[derive(Clone)]
pub struct WasmWidget {
    plugin: Arc<Mutex<extism::Plugin>>,
    options: Arc<serde_json::Value>,
    name: Arc<str>,
    /// One-shot latch for the poisoned-mutex warning. The daemon renders this
    /// widget every tick, so without it a single earlier panic would log once
    /// per refresh forever.
    poison_reported: Arc<std::sync::atomic::AtomicBool>,
}

impl WasmWidget {
    pub fn new(plugin: extism::Plugin, options: serde_json::Value, name: &str) -> Self {
        Self {
            plugin: Arc::new(Mutex::new(plugin)),
            options: Arc::new(options),
            name: Arc::from(name),
            poison_reported: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}
```

Replace the lock acquisition in `render`. Match on the `lock()` result so the
poison case can be reported before recovering the guard:

```rust
        let mut plugin = match self.plugin.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                if !self
                    .poison_reported
                    .swap(true, std::sync::atomic::Ordering::Relaxed)
                {
                    tracing::warn!(
                        plugin = %self.name,
                        "plugin mutex was poisoned by an earlier panic; recovering"
                    );
                }
                poisoned.into_inner()
            }
        };
```

Rationale to preserve in a comment above it: a poisoned mutex means an EARLIER
render panicked while holding it. Bailing out returned empty segments forever —
in the CLI path each render is a fresh process so it self-heals, but under the
W48 daemon this widget is warm for the process lifetime, so one panic killed the
plugin permanently and silently, fixable only by restarting the daemon.
`daemon.rs`'s `handle_request` already recovers ITS state mutex the same way.
Recovery is sound because the guarded value is an `extism::Plugin` whose own
state lives in the wasm instance: a panic in `plugin.call` unwinds Rust-side
without leaving a partially-updated Rust struct behind, and genuinely broken
guest state surfaces as the next `call` returning `Err`, which the arm below
already degrades to empty segments (N2).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rustline-wasm` and re-grep:

```bash
grep -n 'Err(_) => return Vec::new()' crates/rustline-wasm/src/host.rs
```
Expected: tests PASS; the grep returns nothing.

- [ ] **Step 5: Full verification**

```bash
cargo fmt --all
cargo build --workspace && just lint && cargo test --workspace
```

- [ ] **Step 6: Strip B11 from `bughunt.md` and commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
fix(observability): recover a poisoned plugin mutex instead of dying silently [B11]

WasmWidget::render returned Vec::new() on Err(_) from self.plugin.lock() with
no log at all. Poisoning means an earlier render panicked while holding the
lock. In the CLI path each render is a fresh process so it self-heals; under
the W48 daemon the WasmWidget is warm for the whole process lifetime, so ONE
poisoning panic made that plugin render empty on every subsequent tick,
permanently, with an empty log — the user sees a widget that worked at 09:00
and is gone at 09:01 with zero evidence.

Mirrors daemon.rs's handle_request, which already recovers its own state mutex
via PoisonError::into_inner for exactly this reason. The warning is latched
behind an AtomicBool so a warm daemon logs it once rather than every tick.

Recovery is sound because the guarded value is an extism::Plugin whose state
lives in the wasm instance; genuinely broken guest state surfaces as the next
call returning Err, which already degrades to empty segments (N2).

Claude-Session: https://claude.ai/code/session_01QUGuvDUj6gG5xQPiN3X2bG
EOF
)"
```

---

### Task 10: B14 — version the daemon wire protocol

**Files:**
- Modify: `crates/rustline/src/daemon_proto.rs:22-35` (`DaemonRequest`)
- Modify: `crates/rustline/src/daemon_client.rs:49-70` (`try_render_at`)
- Modify: `crates/rustline/src/daemon.rs:171-188` (`handle_request`)
- Test: `crates/rustline/src/daemon_proto.rs`, `daemon.rs` (in-module `mod tests`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `pub const DAEMON_PROTOCOL: u32 = 1;` and `DaemonRequest::RenderV2 { protocol: u32, region: RegionKind, args: RenderArgsWire }` in `daemon_proto`.

**Background you need:** the protocol has no version field and `try_render_at` accepts any `DaemonResponse::Markup(String)` as authoritative. The daemon is long-lived and `reload_if_changed` watches only the *config file's* mtime, never the binary's. After `cargo install` the old daemon keeps running while tmux invokes the new binary: the new client connects, the old daemon renders with old config semantics, old theme resolution and old widget code, and the client emits that markup as its own. Field additions skew silently too, because serde ignores unknown struct fields. Only a *new enum variant* fails loudly — which is the behaviour the whole protocol should have.

- [ ] **Step 1: Write the failing tests**

Add to `crates/rustline/src/daemon_proto.rs`'s `mod tests`:

```rust
#[test]
fn an_old_daemon_cannot_decode_a_versioned_render_request() {
    // This is the whole mechanism: a daemon predating RenderV2 sees an unknown
    // enum variant, fails to deserialize, drops the connection, and the client
    // falls back in-process. Fail-closed by construction.
    #[derive(serde::Deserialize)]
    enum OldRequest {
        Render {
            region: RegionKind,
            args: RenderArgsWire,
        },
        Ping,
        Shutdown,
    }
    let new = DaemonRequest::RenderV2 {
        protocol: DAEMON_PROTOCOL,
        region: RegionKind::Right,
        args: RenderArgsWire::default(),
    };
    let bytes = serde_json::to_vec(&new).unwrap();
    assert!(serde_json::from_slice::<OldRequest>(&bytes).is_err());
}

#[test]
fn ping_and_shutdown_stay_wire_compatible_across_the_bump() {
    // daemon status/stop must keep working against a skewed daemon —
    // otherwise the user cannot stop the stale daemon that caused the problem.
    #[derive(serde::Deserialize, PartialEq, Debug)]
    enum OldRequest {
        Render {
            region: RegionKind,
            args: RenderArgsWire,
        },
        Ping,
        Shutdown,
    }
    for req in [DaemonRequest::Ping, DaemonRequest::Shutdown] {
        let bytes = serde_json::to_vec(&req).unwrap();
        assert!(serde_json::from_slice::<OldRequest>(&bytes).is_ok());
    }
}
```

Add to `crates/rustline/src/daemon.rs`'s `mod tests`:

```rust
#[test]
fn a_mismatched_protocol_is_refused() {
    let state = Mutex::new(test_state());
    let (response, disposition) = handle_request(
        &state,
        Path::new("/nonexistent/config.toml"),
        DaemonRequest::RenderV2 {
            protocol: DAEMON_PROTOCOL + 1,
            region: RegionKind::Right,
            args: RenderArgsWire::default(),
        },
    );
    assert!(
        !matches!(response, DaemonResponse::Markup(_)),
        "a newer client must not receive markup from this daemon"
    );
    assert!(matches!(disposition, Disposition::Continue));
}

#[test]
fn a_matching_protocol_renders() {
    let state = Mutex::new(test_state());
    let (response, _) = handle_request(
        &state,
        Path::new("/nonexistent/config.toml"),
        DaemonRequest::RenderV2 {
            protocol: DAEMON_PROTOCOL,
            region: RegionKind::Right,
            args: RenderArgsWire::default(),
        },
    );
    assert!(matches!(response, DaemonResponse::Markup(_)));
}
```

Read `daemon.rs`'s existing `mod tests` first — reuse whatever helper it already has for building a `DaemonState`; name it `test_state()` above only as a placeholder for that existing helper.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rustline -- protocol old_daemon ping_and_shutdown`
Expected: FAIL — `RenderV2` and `DAEMON_PROTOCOL` don't exist.

- [ ] **Step 3: Implement the protocol field**

In `crates/rustline/src/daemon_proto.rs`:

```rust
/// The daemon wire-protocol version. Bump this whenever [`RenderArgsWire`]
/// gains or loses a field, or whenever the render semantics behind a request
/// change.
///
/// The daemon is long-lived and only reloads on the *config file's* mtime —
/// never the binary's — so after `cargo install` an old daemon keeps serving a
/// new client. Without a version it did so silently, rendering with old config
/// semantics, old theme resolution and old widget code, which the client then
/// emitted as its own output. Serde also ignores unknown struct fields, so a
/// field addition skewed silently too; only an unknown *enum variant* fails
/// loudly, which is why the version rides a new variant rather than a new
/// field on the old one.
pub const DAEMON_PROTOCOL: u32 = 1;

/// A request the daemon client sends to the daemon server.
#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub enum DaemonRequest {
    /// Render one status-line region or window segment, tagged with the
    /// client's [`DAEMON_PROTOCOL`]. A daemon predating this variant cannot
    /// deserialize it at all, drops the connection, and the client falls back
    /// to an in-process render — fail-closed by construction. A daemon that
    /// understands it still refuses a `protocol` it does not equal, so a
    /// *newer* daemon also declines an older client.
    RenderV2 {
        protocol: u32,
        region: RegionKind,
        args: RenderArgsWire,
    },
    /// Liveness check; the server replies [`DaemonResponse::Pong`].
    ///
    /// Deliberately NOT versioned, along with [`DaemonRequest::Shutdown`]: if
    /// a version skew made `daemon status`/`daemon stop` fail too, the user
    /// would have no way to stop the stale daemon that caused the problem.
    Ping,
    /// Ask the server to stop; it replies [`DaemonResponse::ShuttingDown`]
    /// before exiting.
    Shutdown,
}
```

Remove the old `Render` variant. Check for other references first:

```bash
grep -rn 'DaemonRequest::Render' crates/
```
Update every hit — expect `daemon_client.rs`, `daemon.rs`, and possibly `bench/daemon.rs`.

In `crates/rustline/src/daemon_client.rs`, `try_render_at`:

```rust
    daemon_proto::write_frame(
        &mut stream,
        &DaemonRequest::RenderV2 {
            protocol: daemon_proto::DAEMON_PROTOCOL,
            region,
            args,
        },
    )
    .ok()?;
```

In `crates/rustline/src/daemon.rs`, `handle_request`:

```rust
    match req {
        DaemonRequest::RenderV2 {
            protocol,
            region,
            args,
        } => {
            // A client built against a different protocol must not be handed
            // markup this daemon's semantics produced. Refusing sends it back
            // to its in-process fallback, which is always correct (N2).
            if protocol != DAEMON_PROTOCOL {
                tracing::warn!(
                    client_protocol = protocol,
                    daemon_protocol = DAEMON_PROTOCOL,
                    "refusing a render request from a client with a different protocol; \
                     restart the daemon to pick up the new binary"
                );
                return (DaemonResponse::ShuttingDown, Disposition::Continue);
            }
            // A poisoned mutex (a prior render panicked mid-lock) must not kill
            // the daemon: recover the guard and carry on (never break the bar).
            let mut st = state.lock().unwrap_or_else(PoisonError::into_inner);
            st.reload_if_changed(config_path);
            let markup = st.render(region, &args);
            (DaemonResponse::Markup(markup), Disposition::Continue)
        }
        DaemonRequest::Ping => (DaemonResponse::Pong, Disposition::Continue),
        DaemonRequest::Shutdown => (DaemonResponse::ShuttingDown, Disposition::Shutdown),
    }
```

Reusing `ShuttingDown` as the refusal reply is deliberate and safe: `try_render_at` already maps every non-`Markup` response to `None`, so the client falls back correctly with no new variant needed. Note that in the doc comment so it doesn't read as a copy-paste error.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rustline`
Expected: PASS, including the existing `try_render_reads_markup_from_a_fake_daemon` test — its fake daemon must be updated to decode `RenderV2`.

- [ ] **Step 5: Verify a real round-trip still works**

```bash
cargo build --workspace
(./target/debug/rustline daemon run &) ; sleep 2
./target/debug/rustline daemon status; echo "status exit=$?"
./target/debug/rustline render right | head -c 200; echo
./target/debug/rustline daemon stop; echo "stop exit=$?"
```
Expected: status reports running, `render right` returns markup, stop succeeds. Confirm `render right` output is identical with and without the daemon running.

- [ ] **Step 6: Full verification**

```bash
cargo fmt --all
cargo build --workspace && just lint && cargo test --workspace
```

- [ ] **Step 7: Strip B14 from `bughunt.md` and commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
fix(api-surface): version the daemon wire protocol [B14]

DaemonRequest/DaemonResponse/RenderArgsWire had no protocol or build version,
and try_render_at accepted any Markup(String) as authoritative. The daemon is
long-lived and reload_if_changed watches only the config file's mtime, never
the binary's, so after cargo install the old daemon kept serving the new
client — rendering with old config semantics, old theme resolution and old
widget code, which the client emitted as its own output, silently. Serde also
ignores unknown struct fields, so a RenderArgsWire field addition skewed
silently too.

Replaces Render with RenderV2 { protocol, region, args }: an old daemon cannot
deserialize an unknown enum variant, drops the connection, and the client
falls back in-process — fail-closed by construction — while a new daemon
refuses a protocol it does not equal, so the check runs both ways. Ping and
Shutdown stay unversioned so daemon status/stop keep working across a skew;
otherwise the user could not stop the stale daemon that caused the problem.

The refusal reuses DaemonResponse::ShuttingDown because try_render_at already
maps every non-Markup response to None, so the client falls back correctly
without a new variant (N2 preserved).

Claude-Session: https://claude.ai/code/session_01QUGuvDUj6gG5xQPiN3X2bG
EOF
)"
```

- [ ] **Step 8: MILESTONE — full suite** (10 tasks done)

```bash
cargo test --workspace 2>&1 | grep -E '^test result'
just lint
```
Expected: 0 failing; lint clean.

---

### Task 11: B6 — stop re-reading a machine constant every render on macOS

**Files:**
- Modify: `crates/rustline/src/memory.rs:59-68` (`read_memory_macos`)
- Test: `crates/rustline/src/memory.rs` (in-module `mod tests`)

**Interfaces:**
- Consumes: Task 3's `debug!` at `read_memory`'s failure return.
- Produces: nothing other tasks use. This is the last task.

**READ THIS BEFORE STARTING — this task cannot be verified on this machine.**

`read_memory_macos` is behind `#[cfg(target_os = "macos")]`. Cross-checking was attempted during planning and **failed**: `cargo check -p rustline --target aarch64-apple-darwin` cannot build `ring` (pulled in via rustls → ureq) without a darwin C cross-compiler, and the `-arch`/`-mmacosx-version-min` flags are rejected by the host `cc`. So the macOS arm will be **neither compiled nor executed** before commit.

Because of that, this task is deliberately staged to the low-risk half:

- **Do** replace the `sysctl -n hw.memsize` subprocess spawn with `libc::sysctlbyname`, memoized in a `OnceLock`.
- **Do NOT** migrate `vm_stat` to `host_statistics64(HOST_VM_INFO64)`. That is the higher-risk half, it is not needed to fix the constant-re-read defect, and it cannot be validated here. Leave `parse_macos_memory` and its call path intact.
- **Do** follow the codebase's established answer to exactly this problem: keep the pure logic behind `#[cfg(any(target_os = "macos", test))]` and unit-test it on Linux, as `parse_macos_memory`, `parse_pmset` and `parse_kern_boottime` already are.
- **If** you cannot structure the change so the derivation logic is Linux-testable, STOP and report rather than committing an uncompilable, untestable edit.

**Background you need:** `memory` is in the DEFAULT right layout, so on macOS every `render right` — every tmux tick, on every macOS user's machine out of the box — pays two fork+exec (~5–10 ms of pure startup). `hw.memsize` is constant for the machine's lifetime and is re-read every single tick. The author already fixed this exact class of problem for cpu: `read_cpu_macos` was migrated off a `top -l 2` shell-out to a `host_statistics` mach FFI call for precisely this reason, so the precedent, the `libc` dependency, and the `unsafe`-commenting convention are all in-tree at `crates/rustline/src/cpu.rs:251-300`. Read that function before writing this one and mirror its shape exactly.

- [ ] **Step 1: Write the failing test for the Linux-testable half**

Add to `crates/rustline/src/memory.rs`'s `mod tests`:

```rust
#[test]
fn memsize_text_parses_the_same_whatever_produced_it() {
    // parse_macos_memory's contract is unchanged by the sysctl migration: it
    // still takes the total as text. Pinning it here is what makes the FFI
    // swap a *source* change rather than a behaviour change — the only part
    // that cannot be compiled on this box is the syscall wrapper itself.
    let vm = "Mach Virtual Memory Statistics: (page size of 4096 bytes)\n\
              Pages free:                          1000.\n\
              Pages inactive:                      2000.\n\
              Pages speculative:                    500.\n";
    let from_spawn = parse_macos_memory("17179869184\n", vm).expect("parses");
    let from_ffi = parse_macos_memory(&format!("{}", 17179869184u64), vm).expect("parses");
    assert_eq!(from_spawn, from_ffi);
    assert_eq!(from_spawn.total_bytes, 17179869184);
}

#[test]
fn memsize_text_round_trips_through_the_ffi_formatting() {
    // The FFI path formats a u64 into the same text parse_macos_memory reads.
    for total in [0u64, 1, 8 * 1024 * 1024 * 1024, u64::MAX] {
        assert_eq!(memsize_to_text(total).trim().parse::<u64>().ok(), Some(total));
    }
}
```

If `MemInfo` does not derive `PartialEq`/`Debug`, compare fields individually instead of using `assert_eq!` on the struct — do not add derives to a shared wire type for a test's convenience.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rustline -- memsize_text`
Expected: FAIL with "cannot find function `memsize_to_text`".

- [ ] **Step 3: Implement**

In `crates/rustline/src/memory.rs`:

```rust
/// Render a `hw.memsize` byte count as the decimal text `parse_macos_memory`
/// expects. Trivial, but it is the seam that keeps the FFI migration a pure
/// *source* change: the parser's contract is untouched, so everything except
/// the syscall wrapper itself stays unit-tested on any host.
#[cfg(any(target_os = "macos", test))]
fn memsize_to_text(total_bytes: u64) -> String {
    total_bytes.to_string()
}

/// Total physical memory (`hw.memsize`) via `sysctlbyname`, memoized for the
/// process lifetime.
///
/// This replaced a `sysctl -n hw.memsize` subprocess spawn. `memory` is in the
/// default right layout, so that spawn ran on every `render right` — every
/// tmux tick, forever, on every macOS machine — to re-read a value that is
/// constant for the machine's lifetime. Mirrors `cpu.rs`'s
/// `read_mach_cpu_ticks`, which replaced a `top -l 2` shell-out for the same
/// reason; `libc` is already a dependency, so this adds no crate.
///
/// `None` if the sysctl fails, which degrades exactly as the failed spawn did.
#[cfg(target_os = "macos")]
fn hw_memsize() -> Option<u64> {
    use std::sync::OnceLock;

    static MEMSIZE: OnceLock<Option<u64>> = OnceLock::new();
    *MEMSIZE.get_or_init(|| {
        let mut value: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        // SAFETY: `sysctlbyname` writes at most `len` bytes into `value`'s
        // storage and updates `len` to what it actually wrote. `value` is a
        // live, correctly-aligned `u64` we own, and `len` starts as exactly
        // its size, so the write cannot overrun. The name is a NUL-terminated
        // literal. We read `value` only when the call returned 0 (success) AND
        // reported writing a full `u64`, so it is always fully initialized
        // before use. No pointer outlives this block.
        let rc = unsafe {
            libc::sysctlbyname(
                c"hw.memsize".as_ptr(),
                (&raw mut value).cast::<libc::c_void>(),
                &raw mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (rc == 0 && len == std::mem::size_of::<u64>()).then_some(value)
    })
}

#[cfg(target_os = "macos")]
fn read_memory_macos() -> Option<MemInfo> {
    let memsize = hw_memsize()?;
    let vm = std::process::Command::new("vm_stat").output().ok()?;
    let vm = String::from_utf8(vm.stdout).ok()?;
    // `vm_stat` stays a spawn for now: migrating it to
    // host_statistics64(HOST_VM_INFO64) is a separate, higher-risk change that
    // cannot be validated from a Linux dev box (see this commit's message).
    parse_macos_memory(&memsize_to_text(memsize), &vm)
}
```

Two details to check against the toolchain (Rust 1.97, edition 2024): `c"…"` C-string literals are stable since 1.77 so they are available, and `&raw mut` is stable since 1.82. If clippy prefers `std::ptr::addr_of_mut!`, use that instead. If `libc::sysctlbyname`'s first parameter type is `*const c_char` rather than `*const u8`, add `.cast::<libc::c_char>()`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rustline -- memsize_text`
Expected: PASS.

- [ ] **Step 5: Verify what CAN be verified, and be explicit about what cannot**

```bash
cargo build --workspace && just lint && cargo test --workspace
```
Expected: clean — but note this compiles the **Linux** arm only. `hw_memsize` and the edited `read_memory_macos` are behind `#[cfg(target_os = "macos")]` and were **not** compiled by any command in this plan. Say so in the commit message and in the final report.

Sanity-check the unverified code by eye against `crates/rustline/src/cpu.rs:251-300`, which is the working in-tree precedent for a `OnceLock`-memoized mach/sysctl call with a scoped SAFETY comment.

- [ ] **Step 6: Strip B6 from `bughunt.md` and commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
fix(caching): read hw.memsize via sysctlbyname once instead of spawning per render [B6]

read_memory_macos spawned `sysctl -n hw.memsize` AND `vm_stat` on every call,
and memory is in the DEFAULT right layout — so every macOS user paid two
fork+exec per tmux tick out of the box, one of them to re-read a value that is
constant for the machine's lifetime.

Replaces the sysctl spawn with a OnceLock-memoized libc::sysctlbyname,
mirroring cpu.rs's read_mach_cpu_ticks, which replaced a `top -l 2` shell-out
for exactly this reason. libc was already a dependency, so no crate is added.
memsize_to_text keeps parse_macos_memory's contract byte-identical, so the
derivation stays unit-tested on Linux and only the syscall wrapper is new.

Deliberately NOT migrating vm_stat to host_statistics64(HOST_VM_INFO64): that
is the higher-risk half, is not needed for this defect, and cannot be
validated here.

VERIFICATION GAP: this code is behind #[cfg(target_os = "macos")] and was
neither compiled nor executed. Cross-checking was attempted and failed —
`cargo check --target aarch64-apple-darwin` cannot build ring (via rustls ->
ureq) without a darwin C cross-compiler. Needs a build and a `rustline render
right` on real macOS hardware before it can be considered done.

Claude-Session: https://claude.ai/code/session_01QUGuvDUj6gG5xQPiN3X2bG
EOF
)"
```

- [ ] **Step 7: FINAL — full suite and report**

```bash
cargo build --workspace
just lint
cargo test --workspace 2>&1 | grep -E '^test result'
git log --oneline 128cd8a..HEAD
grep -c '^### B' bughunt.md
```
Expected: build clean, lint clean, ≥937 passing / 0 failing, 11 commits, and `bughunt.md` down to 28 remaining findings.

---

## Self-Review Notes

**Spec coverage:** all 11 selected findings have exactly one task (B1→1, B3→2, B2→3, B5→4, B7→5, B8→6, B9→7, B10→8, B11→9, B14→10, B6→11). The spec's global rules map to the Global Constraints block and to each task's verification steps. The spec's "Invariants this work depends on" section is enforced per-task: #2 in Task 5, #3 in Task 7, #5 in Tasks 1-2 (via the byte-identity test), #6 in Tasks 3-4, #7 in Task 1, N1 in Task 1, N2 in Tasks 8/9/10.

**Known ordering dependencies:** Task 2 edits the function Task 1 creates. Task 4 edits the log lines Task 3 adds. Task 9 edits the function Task 8 edits. Task 11 depends on Task 3's `read_memory` logging being in place. Do not reorder.

**Deliberate omissions, for the reviewer's benefit:** B26's two `warn!` lines at `host.rs:219`/`:230` are untouched by Tasks 8 and 9 on purpose. B39's `kill_group`-after-reap path is untouched by Task 4 on purpose. B30's wrong-behaviour throughput test is untouched on purpose. All three remain open findings.
