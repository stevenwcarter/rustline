//! Transcode rustline's tmux status-line markup into ANSI escape sequences, so
//! a rendered region can be previewed in colour directly on the terminal (for
//! manual testing/verification, via `rustline render … --preview`).
//!
//! rustline only ever emits a small, known subset of tmux markup:
//! - `#[fg=<colour>,bg=<colour>[,bold]]` style directives,
//! - `#[default]` to reset, and
//! - literal text (including powerline separator glyphs).
//!
//! This transcoder parses exactly that and passes any other text through
//! verbatim. Colours map as: `colourN` → 256-colour (`38;5;N` / `48;5;N`),
//! `#rrggbb` → truecolour (`38;2;r;g;b`), the eight named colours (+`bright*`,
//! `default`) → basic SGR codes; unrecognised attributes are ignored.

/// Convert a tmux-markup string (as produced by [`crate::render_region`]) into a
/// string with ANSI SGR escape sequences in place of the `#[...]` directives.
/// Non-directive text — including multi-byte powerline glyphs and stray `#`
/// characters — passes through verbatim.
pub fn tmux_to_ansi(markup: &str) -> String {
    let mut out = String::with_capacity(markup.len() + 16);
    let mut chars = markup.chars().peekable();

    while let Some(c) = chars.next() {
        // A style directive is the two-char sequence "#[" … "]".
        if c == '#' && chars.peek() == Some(&'[') {
            chars.next(); // consume '['
            let mut inner = String::new();
            let mut closed = false;
            for d in chars.by_ref() {
                if d == ']' {
                    closed = true;
                    break;
                }
                inner.push(d);
            }
            if closed {
                out.push_str(&directive_to_sgr(&inner));
            } else {
                // Unterminated directive: emit what we consumed, verbatim.
                out.push('#');
                out.push('[');
                out.push_str(&inner);
            }
        } else {
            out.push(c);
        }
    }

    out
}

/// Translate the inside of one `#[...]` directive (its comma-separated
/// attributes) into a single ANSI SGR sequence, or the empty string if it
/// carries nothing we render.
fn directive_to_sgr(inner: &str) -> String {
    let mut codes: Vec<String> = Vec::new();

    for attr in inner.split(',') {
        let attr = attr.trim();
        // Map this attribute to an SGR parameter, if it's one we render.
        // Anything else (empty, align=…, norange, …) contributes no code.
        let code = match attr {
            "default" => Some("0".to_string()),
            "bold" => Some("1".to_string()),
            _ => {
                if let Some(spec) = attr.strip_prefix("fg=") {
                    color_sgr(spec, true)
                } else if let Some(spec) = attr.strip_prefix("bg=") {
                    color_sgr(spec, false)
                } else {
                    None
                }
            }
        };
        if let Some(code) = code {
            codes.push(code);
        }
    }

    if codes.is_empty() {
        String::new()
    } else {
        format!("\x1b[{}m", codes.join(";"))
    }
}

/// Map one tmux colour spec to the SGR parameters for a foreground (`is_fg`) or
/// background colour, or `None` if it isn't a colour we recognise.
fn color_sgr(spec: &str, is_fg: bool) -> Option<String> {
    let base = if is_fg { "38" } else { "48" };

    if spec == "default" {
        return Some(if is_fg { "39" } else { "49" }.to_string());
    }
    if let Some(n) = spec
        .strip_prefix("colour")
        .or_else(|| spec.strip_prefix("color"))
    {
        let idx: u8 = n.parse().ok()?;
        return Some(format!("{base};5;{idx}"));
    }
    if let Some(hex) = spec.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(format!("{base};2;{r};{g};{b}"));
        }
        return None;
    }
    named_sgr(spec, is_fg)
}

/// Map a named tmux colour (`red`, `brightblue`, …) to its basic SGR code.
fn named_sgr(name: &str, is_fg: bool) -> Option<String> {
    let (name, bright) = match name.strip_prefix("bright") {
        Some(rest) => (rest, true),
        None => (name, false),
    };
    let offset = match name {
        "black" => 0,
        "red" => 1,
        "green" => 2,
        "yellow" => 3,
        "blue" => 4,
        "magenta" => 5,
        "cyan" => 6,
        "white" => 7,
        _ => return None,
    };
    let start = match (is_fg, bright) {
        (true, false) => 30,
        (false, false) => 40,
        (true, true) => 90,
        (false, true) => 100,
    };
    Some((start + offset).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_fg_bg_and_default_reset() {
        assert_eq!(
            tmux_to_ansi("#[fg=colour255,bg=colour31] x #[default]"),
            "\x1b[38;5;255;48;5;31m x \x1b[0m"
        );
    }

    #[test]
    fn bold_attribute_included() {
        assert_eq!(
            tmux_to_ansi("#[fg=colour255,bg=colour31,bold]y"),
            "\x1b[38;5;255;48;5;31;1my"
        );
    }

    #[test]
    fn truecolor_rgb() {
        assert_eq!(tmux_to_ansi("#[fg=#1a2b3c]z"), "\x1b[38;2;26;43;60mz");
    }

    #[test]
    fn named_colors_basic_and_bright() {
        assert_eq!(tmux_to_ansi("#[fg=cyan,bg=red]t"), "\x1b[36;41mt");
        assert_eq!(tmux_to_ansi("#[fg=brightblue]t"), "\x1b[94mt");
        assert_eq!(tmux_to_ansi("#[bg=default]t"), "\x1b[49mt");
    }

    #[test]
    fn literal_text_glyphs_and_stray_hash_passthrough() {
        // A powerline glyph (multi-byte) and a lone '#' must survive verbatim.
        assert_eq!(tmux_to_ansi("a\u{e0b0}b#c"), "a\u{e0b0}b#c");
    }

    #[test]
    fn unknown_attributes_produce_no_codes() {
        assert_eq!(tmux_to_ansi("#[align=left,norange]hi"), "hi");
    }

    #[test]
    fn unterminated_directive_is_emitted_verbatim() {
        assert_eq!(tmux_to_ansi("#[fg=colour1 oops"), "#[fg=colour1 oops");
    }

    #[test]
    fn plain_text_unchanged() {
        assert_eq!(
            tmux_to_ansi("Mon < 2026-07-20 < 19:04"),
            "Mon < 2026-07-20 < 19:04"
        );
    }
}
