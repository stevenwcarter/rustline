//! The one definition of invariant #7's clickable-name rule. A [`RangeName`]
//! is proof its bytes are safe to interpolate into `#[range=user|…]` tmux
//! markup verbatim (invariant #8's range-name counterpart): the only way to
//! obtain one is [`RangeName::parse`], so nothing downstream needs to
//! re-check non-emptiness, length, charset, or the reserved name — and
//! nothing can forge markup through a widget/plugin/instance name that never
//! parsed. See invariant #7 in `CLAUDE.md` for the identity chain this name
//! anchors (range markup, `Context.toggled` key, `active_format` key, the
//! `--range` value `rustline click` receives).

use std::fmt;
use std::ops::Deref;

/// tmux's `range=user|<name>` argument caps `<name>` at this many bytes. The
/// single source of truth for that limit: [`RangeName::parse`] and every
/// caller that used to compare against a hardcoded `15` now goes through it
/// instead, so the limit only needs to change in one place.
pub const RANGE_NAME_MAX_BYTES: usize = 15;

/// The reserved name that can never be a click-toggle identity: it names the
/// built-in window-list renderer, which is not a widget/plugin/instance
/// resolvable slot.
const RESERVED: &str = "window";

/// A name proven safe to interpolate verbatim into tmux's
/// `#[range=user|<name>]` markup: non-empty, at most [`RANGE_NAME_MAX_BYTES`]
/// bytes, `[A-Za-z0-9_-]` only, and not the reserved name `"window"`.
///
/// [`RangeName::parse`] is the sole constructor, so every `RangeName` value
/// that exists already satisfies these rules — a caller consumes an
/// already-valid value rather than re-validating the same string over and
/// over at each of the render/registration/CLI boundaries that used to
/// implement this rule independently (and, in one case, incompletely).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RangeName(String);

/// Why a candidate string failed to parse as a [`RangeName`].
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    #[error("name must not be empty")]
    Empty,
    #[error("name is {len} bytes (max {RANGE_NAME_MAX_BYTES})")]
    TooLong { len: usize },
    #[error("name may only contain letters, digits, `_`, and `-` (found {ch:?})")]
    BadChar { ch: char },
    #[error("name \"window\" is reserved")]
    Reserved,
}

impl RangeName {
    /// Parse `s` into a [`RangeName`], checking (in order) non-emptiness,
    /// the [`RANGE_NAME_MAX_BYTES`] length cap, the `[A-Za-z0-9_-]` charset,
    /// and the reserved name `"window"`.
    pub fn parse(s: &str) -> Result<RangeName, NameError> {
        if s.is_empty() {
            return Err(NameError::Empty);
        }
        if s.len() > RANGE_NAME_MAX_BYTES {
            return Err(NameError::TooLong { len: s.len() });
        }
        if let Some(ch) = s
            .chars()
            .find(|c| !c.is_ascii_alphanumeric() && *c != '_' && *c != '-')
        {
            return Err(NameError::BadChar { ch });
        }
        if s == RESERVED {
            return Err(NameError::Reserved);
        }
        Ok(RangeName(s.to_string()))
    }

    /// The validated name, borrowed.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for RangeName {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RangeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for RangeName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_enforces_all_four_rules() {
        assert!(RangeName::parse("cpu").is_ok());
        assert!(RangeName::parse("clock_utc-2").is_ok());
        assert_eq!(RangeName::parse(""), Err(NameError::Empty));
        assert_eq!(
            RangeName::parse("sixteen_bytes_xx"),
            Err(NameError::TooLong { len: 16 })
        );
        assert_eq!(
            RangeName::parse("a#[norange]"),
            Err(NameError::BadChar { ch: '#' })
        );
        assert_eq!(RangeName::parse("window"), Err(NameError::Reserved));
    }

    #[test]
    fn exactly_max_bytes_is_ok_one_over_is_too_long() {
        let fifteen = "fifteen_bytes__";
        assert_eq!(fifteen.len(), RANGE_NAME_MAX_BYTES);
        assert!(RangeName::parse(fifteen).is_ok());

        let sixteen = "this_name_is_16b";
        assert_eq!(sixteen.len(), RANGE_NAME_MAX_BYTES + 1);
        assert_eq!(
            RangeName::parse(sixteen),
            Err(NameError::TooLong { len: 16 })
        );
    }

    #[test]
    fn as_str_display_deref_and_as_ref_all_agree() {
        let n = RangeName::parse("cpu").unwrap();
        assert_eq!(n.as_str(), "cpu");
        assert_eq!(n.to_string(), "cpu");
        assert_eq!(&*n, "cpu");
        assert_eq!(n.as_ref(), "cpu");
    }

    #[test]
    fn name_error_messages_are_human_readable() {
        assert_eq!(NameError::Empty.to_string(), "name must not be empty");
        assert_eq!(
            NameError::TooLong { len: 16 }.to_string(),
            "name is 16 bytes (max 15)"
        );
        assert_eq!(
            NameError::BadChar { ch: '#' }.to_string(),
            "name may only contain letters, digits, `_`, and `-` (found '#')"
        );
        assert_eq!(
            NameError::Reserved.to_string(),
            "name \"window\" is reserved"
        );
    }
}
