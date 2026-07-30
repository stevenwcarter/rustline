//! URL/path/command allow-patterns. Each entry is a glob by default, or a
//! regex when prefixed with `re:`. Globs use `globset` defaults (`*` matches
//! across `/`), so `https://wttr.in/*` matches the full URL incl. its query
//! string, and `uname*` matches any argv string starting with `uname`
//! (including `unamex`, `uname-anything`, etc. — see `allowed_commands`'
//! own docs for the precision this implies for exec grants).
//!
//! [`AllowSet`] is phantom-tagged by capability family ([`CapKind`]): a
//! `AllowSet<UrlCap>` only ever accepts a [`Url`] subject, a
//! `AllowSet<ReadPathCap>`/`AllowSet<WritePathCap>` only a [`ResolvedPath`],
//! and a `AllowSet<CommandCap>` only a [`crate::argv::CanonicalArgv`] — so a
//! URL can never be checked against a path (or write) allowlist, and vice
//! versa; that used to be a runtime-only convention (all four fields were the
//! same untagged type) and is now enforced by the compiler.

use std::marker::PhantomData;

use globset::{Glob, GlobMatcher};
use regex::Regex;

use crate::argv::CanonicalArgv;
use crate::capability::DenialKind;
use crate::state::{AllowedPath, ResolvedPath};

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

/// One capability family. `DENIAL` is the record kind every deny site for
/// this family reports; `Subject` is the only value type the gate accepts,
/// so a URL can never be checked against a path allowlist (or vice versa).
pub trait CapKind {
    const DENIAL: DenialKind;
    type Subject<'a>;
    fn as_match_str<'a>(s: &'a Self::Subject<'_>) -> &'a str;
}

/// A URL about to be fetched. Thin borrow wrapper — the subject type for
/// `AllowSet<UrlCap>`.
pub struct Url<'a>(&'a str);

impl<'a> Url<'a> {
    pub fn new(s: &'a str) -> Self {
        Self(s)
    }

    pub fn as_str(&self) -> &'a str {
        self.0
    }
}

/// Marker for the `allowed_urls` capability family.
pub struct UrlCap;

impl CapKind for UrlCap {
    const DENIAL: DenialKind = DenialKind::Url;
    type Subject<'a> = Url<'a>;

    fn as_match_str<'a>(s: &'a Url<'_>) -> &'a str {
        s.0
    }
}

/// Marker for the `allowed_paths` (read) capability family.
pub struct ReadPathCap;

impl CapKind for ReadPathCap {
    const DENIAL: DenialKind = DenialKind::Path;
    type Subject<'a> = ResolvedPath;

    fn as_match_str(s: &ResolvedPath) -> &str {
        s.as_str()
    }
}

/// Marker for the `allowed_write_paths` (write) capability family.
pub struct WritePathCap;

impl CapKind for WritePathCap {
    const DENIAL: DenialKind = DenialKind::Path;
    type Subject<'a> = ResolvedPath;

    fn as_match_str(s: &ResolvedPath) -> &str {
        s.as_str()
    }
}

/// Marker for the `allowed_commands` (exec) capability family.
pub struct CommandCap;

impl CapKind for CommandCap {
    const DENIAL: DenialKind = DenialKind::Command;
    type Subject<'a> = CanonicalArgv;

    fn as_match_str(s: &CanonicalArgv) -> &str {
        s.as_str()
    }
}

/// A set of allow-patterns for one capability family `K`; `allows` is true
/// iff any pattern matches. An empty set denies everything (deny-by-default).
/// Malformed entries are logged and skipped, never fatal.
pub struct AllowSet<K: CapKind>(Vec<Pattern>, PhantomData<K>);

impl<K: CapKind> AllowSet<K> {
    /// Compile every entry, warning on and skipping malformed ones.
    pub fn compile(entries: &[String]) -> AllowSet<K> {
        let mut patterns = Vec::new();
        for entry in entries {
            match Pattern::compile(entry) {
                Ok(p) => patterns.push(p),
                Err(error) => {
                    rustline_core::diag::warn_once(&format!("allow-pattern:{entry}"), || {
                        tracing::warn!(pattern = %entry, %error, "invalid allow pattern, skipping");
                    });
                }
            }
        }
        AllowSet(patterns, PhantomData)
    }

    /// True iff any pattern in the set matches `subject`.
    pub fn allows(&self, subject: &K::Subject<'_>) -> bool {
        let s = K::as_match_str(subject);
        self.0.iter().any(|p| p.is_match(s))
    }
}

/// Path-family sets additionally mint the [`AllowedPath`] token (Task 1's
/// resolve→match→act seam): only `ReadPathCap`/`WritePathCap` implement this,
/// so `check_path` doesn't exist on `AllowSet<UrlCap>`/`AllowSet<CommandCap>`.
pub trait PathCap: CapKind {}
impl PathCap for ReadPathCap {}
impl PathCap for WritePathCap {}

impl<K: PathCap> AllowSet<K> {
    /// Consume a resolved path; return the only token the filesystem effects
    /// accept, or give the path back on a miss (so the caller can report it).
    pub fn check_path(&self, path: ResolvedPath) -> Result<AllowedPath, ResolvedPath> {
        if self.0.iter().any(|p| p.is_match(path.as_str())) {
            Ok(AllowedPath::from_checked(path))
        } else {
            Err(path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_url_prefix() {
        let s: AllowSet<UrlCap> = AllowSet::compile(&["https://wttr.in/*".into()]);
        assert!(s.allows(&Url::new("https://wttr.in/48183?format=j1")));
    }

    #[test]
    fn glob_denies_other_host() {
        let s: AllowSet<UrlCap> = AllowSet::compile(&["https://wttr.in/*".into()]);
        assert!(!s.allows(&Url::new("https://evil.example/steal")));
    }

    #[test]
    fn empty_set_denies_everything() {
        let s: AllowSet<UrlCap> = AllowSet::compile(&[]);
        assert!(!s.allows(&Url::new("https://wttr.in/48183")));
    }

    #[test]
    fn regex_prefix_matches() {
        // Patterns are anchored (full-string), so a trailing `.*` is needed to
        // consume the query tail after the 5-digit zip.
        let s: AllowSet<UrlCap> = AllowSet::compile(&[r"re:https://wttr\.in/\d{5}.*".into()]);
        assert!(s.allows(&Url::new("https://wttr.in/48183?format=j1")));
        assert!(!s.allows(&Url::new("https://wttr.in/abcde")));
    }

    #[test]
    fn regex_is_anchored_not_substring() {
        // Fail-safe: a bare host regex must not be satisfied by an off-allowlist
        // URL that only mentions the host in a query param.
        let s: AllowSet<UrlCap> = AllowSet::compile(&[r"re:wttr\.in".into()]);
        assert!(!s.allows(&Url::new("https://evil.example/?x=wttr.in")));
        assert!(s.allows(&Url::new("wttr.in")));
    }

    #[test]
    fn malformed_pattern_is_skipped_not_fatal() {
        // one bad regex, one good glob -> the good one still works
        let s: AllowSet<UrlCap> = AllowSet::compile(&["re:[".into(), "https://ok/*".into()]);
        assert!(s.allows(&Url::new("https://ok/path")));
        assert!(!s.allows(&Url::new("https://nope/x")));
    }

    #[test]
    fn typed_allowsets_carry_their_denial_kind() {
        use crate::capability::DenialKind;
        assert!(matches!(UrlCap::DENIAL, DenialKind::Url));
        assert!(matches!(ReadPathCap::DENIAL, DenialKind::Path));
        assert!(matches!(WritePathCap::DENIAL, DenialKind::Path));
        assert!(matches!(CommandCap::DENIAL, DenialKind::Command));
        let urls: AllowSet<UrlCap> = AllowSet::compile(&["https://wttr.in/*".into()]);
        assert!(urls.allows(&Url::new("https://wttr.in/48183")));
    }
}
