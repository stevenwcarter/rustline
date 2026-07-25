//! The canonical argv string: the single key an `allowed_commands` pattern is
//! matched against.
//!
//! This is a **matching key only** — it is never executed. The host always
//! spawns `program` with the `args` vector directly, with no shell anywhere in
//! the path, so nothing ever re-parses this string. The quoting exists purely
//! so two *different* argv vectors can never render to the same string: without
//! it, `["log", "--author=a b"]` and `["log", "--author=a", "b"]` would look
//! identical to a pattern, and a grant written for one would silently cover the
//! other.

/// Characters that force an argument to be quoted in the canonical form.
fn needs_quotes(s: &str) -> bool {
    s.is_empty()
        || s.chars()
            .any(|c| c.is_whitespace() || matches!(c, '\'' | '"' | '\\'))
}

/// Render one argument: bare when unambiguous, else single-quoted with any
/// embedded single quote escaped as `'\''` (POSIX-style, for readability in
/// config files and denial records).
fn quote(s: &str) -> String {
    if !needs_quotes(s) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Render `program` + `args` to the string an `allowed_commands` pattern is
/// matched against. See the module docs for why this is quoted.
pub fn canonical_argv(program: &str, args: &[String]) -> String {
    let mut out = quote(program);
    for arg in args {
        out.push(' ');
        out.push_str(&quote(arg));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn plain_args_join_with_single_spaces() {
        assert_eq!(
            canonical_argv("playerctl", &s(&["metadata"])),
            "playerctl metadata"
        );
        assert_eq!(
            canonical_argv("git", &s(&["status", "--porcelain"])),
            "git status --porcelain"
        );
        assert_eq!(canonical_argv("uname", &[]), "uname");
    }

    #[test]
    fn an_arg_containing_whitespace_is_quoted_so_it_cannot_masquerade_as_two_args() {
        // Without quoting, this would render identically to
        // canonical_argv("git", ["log", "--author=a", "b"]) and could match a
        // pattern written for that different call.
        assert_eq!(
            canonical_argv("git", &s(&["log", "--author=a b"])),
            "git log '--author=a b'"
        );
        assert_ne!(
            canonical_argv("git", &s(&["log", "--author=a b"])),
            canonical_argv("git", &s(&["log", "--author=a", "b"]))
        );
    }

    #[test]
    fn tabs_and_newlines_are_treated_as_whitespace_needing_quotes() {
        assert_eq!(canonical_argv("x", &s(&["a\tb"])), "x 'a\tb'");
        assert_eq!(canonical_argv("x", &s(&["a\nb"])), "x 'a\nb'");
    }

    #[test]
    fn an_embedded_single_quote_is_escaped_not_dropped() {
        assert_eq!(
            canonical_argv("x", &s(&["it's here"])),
            r#"x 'it'\''s here'"#
        );
    }

    #[test]
    fn an_empty_arg_becomes_empty_quotes_rather_than_vanishing() {
        assert_eq!(canonical_argv("x", &s(&["", "y"])), "x '' y");
    }

    #[test]
    fn a_program_needing_quotes_is_quoted_too() {
        assert_eq!(
            canonical_argv("/opt/my tools/bin", &[]),
            "'/opt/my tools/bin'"
        );
    }

    #[test]
    fn a_bare_double_quote_triggers_quoting_rather_than_being_emitted_bare() {
        assert_eq!(canonical_argv("x", &s(&[r#"id="42""#])), r#"x 'id="42"'"#);
        // If a literal `"` were silently dropped or ignored instead of
        // forcing quoting, this would render identically to the quote-free
        // variant below, collapsing two distinct arguments onto the same
        // canonical string.
        assert_ne!(
            canonical_argv("x", &s(&[r#"id="42""#])),
            canonical_argv("x", &s(&["id=42"]))
        );
    }

    #[test]
    fn a_backslash_adjacent_to_a_single_quote_does_not_interfere_with_the_escape() {
        // `\'` (backslash then quote) is quoted, with the embedded quote
        // escaped as `'\''` and the raw backslash carried through unchanged.
        assert_eq!(canonical_argv("x", &s(&["\\'"])), "x '\\'\\'''");
        // Swapping the two characters' order (`'\` — quote then backslash)
        // must render to a DIFFERENT canonical string: if a raw backslash in
        // the input could be confused with the backslash the escape scheme
        // itself emits inside a `'\''` group, these two distinct
        // two-character arguments could collapse onto the same canonical
        // form.
        assert_ne!(
            canonical_argv("x", &s(&["\\'"])),
            canonical_argv("x", &s(&["'\\"]))
        );
    }
}
