//! URL/path allow-patterns. Each entry is a glob by default, or a regex when
//! prefixed with `re:`. Globs use `globset` defaults (`*` matches across `/`),
//! so `https://wttr.in/*` matches the full URL incl. its query string.

use globset::{Glob, GlobMatcher};
use regex::Regex;

/// A single compiled allow-pattern.
pub enum Pattern {
    Glob(GlobMatcher),
    Regex(Regex),
}

impl Pattern {
    /// Compile one entry; `re:` prefix selects regex, otherwise glob.
    pub fn compile(entry: &str) -> Result<Pattern, String> {
        if let Some(rx) = entry.strip_prefix("re:") {
            // Anchor to a full-string match (uniform with globs) so a bare
            // host regex like `re:wttr\.in` can't be satisfied by an
            // off-allowlist URL that merely contains it in a query param.
            Regex::new(&format!("^(?:{rx})$"))
                .map(Pattern::Regex)
                .map_err(|e| e.to_string())
        } else {
            Glob::new(entry)
                .map(|g| Pattern::Glob(g.compile_matcher()))
                .map_err(|e| e.to_string())
        }
    }

    /// Does `s` match this pattern?
    pub fn is_match(&self, s: &str) -> bool {
        match self {
            Pattern::Glob(g) => g.is_match(s),
            Pattern::Regex(r) => r.is_match(s),
        }
    }
}

/// A set of allow-patterns; `allows` is true iff any pattern matches. An empty
/// set denies everything (deny-by-default). Malformed entries are logged and
/// skipped, never fatal.
pub struct AllowSet(Vec<Pattern>);

impl AllowSet {
    /// Compile every entry, warning on and skipping malformed ones.
    pub fn compile(entries: &[String]) -> AllowSet {
        let mut patterns = Vec::new();
        for entry in entries {
            match Pattern::compile(entry) {
                Ok(p) => patterns.push(p),
                Err(error) => {
                    tracing::warn!(pattern = %entry, %error, "invalid allow pattern, skipping");
                }
            }
        }
        AllowSet(patterns)
    }

    /// True iff any pattern in the set matches `subject`.
    pub fn allows(&self, subject: &str) -> bool {
        self.0.iter().any(|p| p.is_match(subject))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_url_prefix() {
        let s = AllowSet::compile(&["https://wttr.in/*".into()]);
        assert!(s.allows("https://wttr.in/48183?format=j1"));
    }

    #[test]
    fn glob_denies_other_host() {
        let s = AllowSet::compile(&["https://wttr.in/*".into()]);
        assert!(!s.allows("https://evil.example/steal"));
    }

    #[test]
    fn empty_set_denies_everything() {
        let s = AllowSet::compile(&[]);
        assert!(!s.allows("https://wttr.in/48183"));
    }

    #[test]
    fn regex_prefix_matches() {
        // Patterns are anchored (full-string), so a trailing `.*` is needed to
        // consume the query tail after the 5-digit zip.
        let s = AllowSet::compile(&[r"re:https://wttr\.in/\d{5}.*".into()]);
        assert!(s.allows("https://wttr.in/48183?format=j1"));
        assert!(!s.allows("https://wttr.in/abcde"));
    }

    #[test]
    fn regex_is_anchored_not_substring() {
        // Fail-safe: a bare host regex must not be satisfied by an off-allowlist
        // URL that only mentions the host in a query param.
        let s = AllowSet::compile(&[r"re:wttr\.in".into()]);
        assert!(!s.allows("https://evil.example/?x=wttr.in"));
        assert!(s.allows("wttr.in"));
    }

    #[test]
    fn malformed_pattern_is_skipped_not_fatal() {
        // one bad regex, one good glob -> the good one still works
        let s = AllowSet::compile(&["re:[".into(), "https://ok/*".into()]);
        assert!(s.allows("https://ok/path"));
        assert!(!s.allows("https://nope/x"));
    }
}
