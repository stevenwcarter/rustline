//! rustline-abi: the serde-serializable types that cross the WASM plugin
//! boundary (Segment/Style/Color). No I/O, no chrono — the wire-format ABI.
use serde::{Deserialize, Serialize};

/// A terminal color, expressible in the ways tmux understands colors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Color {
    Named(String),
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    /// Render this color as a tmux-style color spec (e.g. `cyan`,
    /// `colour236`, `#1a2b3c`).
    pub fn to_tmux(&self) -> String {
        match self {
            Color::Named(n) => n.clone(),
            Color::Indexed(i) => format!("colour{i}"),
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        }
    }
}

/// Visual styling for a [`Segment`]: foreground/background color and
/// boldness.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    #[serde(default)]
    pub bold: bool,
}

/// A single piece of rendered status line text with its style.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub text: String,
    pub style: Style,
}

impl Segment {
    /// Create a segment with the default (unstyled) style.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::default(),
        }
    }

    /// Create a segment with an explicit style.
    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_to_tmux_named_indexed_rgb() {
        assert_eq!(Color::Named("cyan".into()).to_tmux(), "cyan");
        assert_eq!(Color::Indexed(236).to_tmux(), "colour236");
        assert_eq!(Color::Rgb(0x1a, 0x2b, 0x3c).to_tmux(), "#1a2b3c");
    }

    #[test]
    fn segment_new_defaults_style() {
        let s = Segment::new("hi");
        assert_eq!(s.text, "hi");
        assert_eq!(s.style, Style::default());
    }
}
