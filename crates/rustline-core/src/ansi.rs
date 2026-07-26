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
//!
//! **Two sinks, one scanner.** ANSI escapes only mean anything to a *terminal*
//! that interprets them, so [`tmux_to_ansi`] is right for `println!`-to-stdout
//! (`render … --preview`, `theme show`) and categorically wrong for a sink that
//! writes cells directly — a `ratatui` buffer renders an escape sequence as
//! literal visible characters and applies no styling at all. [`parse_markup`]
//! is the alternative for such a sink: the same markup resolved into
//! `(style, text)` runs the caller can translate into its own style type.
//! Both share [`scan`] so the fiddly part — where a directive starts and ends,
//! and what an unterminated `#[` means — has exactly one definition.

use crate::segment::{Color, Style};

/// One lexical piece of markup: either literal text or the inside of one
/// `#[...]` directive.
enum Token {
    Text(String),
    Directive(String),
}

/// Split markup into literal-text and directive tokens.
///
/// An unterminated `#[` is not a directive — the characters we consumed looking
/// for the `]` are folded back into literal text, verbatim.
/// `##` is tmux's escape for a literal `#` and collapses to one `#` (this is
/// the inverse of `render::sanitize_text`, so a preview renders exactly what
/// tmux draws). A single `#` not followed by `[` or `#` is ordinary text.
fn scan(markup: &str) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut text = String::new();
    let mut chars = markup.chars().peekable();

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
                if !text.is_empty() {
                    tokens.push(Token::Text(std::mem::take(&mut text)));
                }
                tokens.push(Token::Directive(inner));
            } else {
                // Unterminated directive: keep what we consumed, verbatim.
                text.push('#');
                text.push('[');
                text.push_str(&inner);
            }
        } else {
            text.push(c);
        }
    }
    if !text.is_empty() {
        tokens.push(Token::Text(text));
    }
    tokens
}

/// Convert a tmux-markup string (as produced by [`crate::render_region`]) into a
/// string with ANSI SGR escape sequences in place of the `#[...]` directives.
/// Non-directive text — including multi-byte powerline glyphs and stray `#`
/// characters — passes through verbatim.
///
/// The result is meant for a **terminal**, which is what actually interprets
/// the escapes. Handing it to a cell-buffer renderer (`ratatui`) draws the
/// escape bytes as visible text — use [`parse_markup`] there instead.
pub fn tmux_to_ansi(markup: &str) -> String {
    let mut out = String::with_capacity(markup.len() + 16);
    for token in scan(markup) {
        match token {
            Token::Text(text) => out.push_str(&text),
            Token::Directive(inner) => out.push_str(&directive_to_sgr(&inner)),
        }
    }
    out
}

/// A run of text together with the style in effect when it was emitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledSpan {
    pub style: Style,
    pub text: String,
}

/// Resolve tmux markup into styled text runs — the sink-agnostic counterpart to
/// [`tmux_to_ansi`], for a renderer that carries its own style type rather than
/// interpreting ANSI escapes.
///
/// tmux style directives are *stateful*: `#[fg=red]` sets the foreground until
/// something changes it, and a later `#[bg=blue]` leaves that foreground alone.
/// A terminal maintains that state itself, which is why `tmux_to_ansi` can emit
/// one SGR per directive and forget it. A cell buffer can't — each run has to
/// carry its whole resolved style — so this accumulates the running state and
/// attaches a snapshot of it to every run.
///
/// `#[default]` resets everything. `fg=default`/`bg=default` reset just that
/// channel to `None`, meaning "the terminal's own default" (not black).
/// Unrecognised attributes are ignored, exactly as in `tmux_to_ansi`. Runs are
/// emitted only for non-empty text, so a directive-only string yields nothing.
pub fn parse_markup(markup: &str) -> Vec<StyledSpan> {
    let mut spans: Vec<StyledSpan> = Vec::new();
    let mut style = Style::default();

    for token in scan(markup) {
        match token {
            Token::Text(text) => spans.push(StyledSpan {
                style: style.clone(),
                text,
            }),
            Token::Directive(inner) => apply_directive(&mut style, &inner),
        }
    }
    spans
}

/// Apply one `#[...]` directive's attributes onto the running style.
fn apply_directive(style: &mut Style, inner: &str) {
    for attr in inner.split(',') {
        let attr = attr.trim();
        match attr {
            "default" => *style = Style::default(),
            "bold" => style.bold = true,
            _ => {
                if let Some(spec) = attr.strip_prefix("fg=")
                    && let Some(color) = parse_color(spec)
                {
                    style.fg = color;
                } else if let Some(spec) = attr.strip_prefix("bg=")
                    && let Some(color) = parse_color(spec)
                {
                    style.bg = color;
                }
            }
        }
    }
}

/// Parse one tmux colour spec into a channel value.
///
/// `None` means "not a colour we recognise" — leave the channel untouched.
/// `Some(None)` is tmux's `default`: reset this channel to the terminal's own
/// default. `Some(Some(c))` is a real colour.
fn parse_color(spec: &str) -> Option<Option<Color>> {
    if spec == "default" {
        return Some(None);
    }
    if let Some(n) = spec
        .strip_prefix("colour")
        .or_else(|| spec.strip_prefix("color"))
    {
        return n.parse().ok().map(|idx| Some(Color::Indexed(idx)));
    }
    if let Some(hex) = spec.strip_prefix('#') {
        if hex.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        return Some(Some(Color::Rgb(r, g, b)));
    }
    // A named colour: recognised iff `named_sgr` knows it, so the two
    // renderers agree on exactly which names are colours.
    named_sgr(spec, true).map(|_| Some(Color::Named(spec.to_string())))
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

    // --- parse_markup: the cell-buffer (ratatui) sink -------------------

    #[test]
    fn parse_markup_attaches_the_directive_style_to_the_following_text() {
        let spans = parse_markup("#[fg=colour255,bg=colour31] hi ");
        assert_eq!(
            spans,
            vec![StyledSpan {
                style: Style {
                    fg: Some(Color::Indexed(255)),
                    bg: Some(Color::Indexed(31)),
                    bold: false,
                },
                text: " hi ".to_string(),
            }]
        );
    }

    #[test]
    fn parse_markup_carries_style_state_across_directives() {
        // tmux directives are stateful: setting bg later must not clear the
        // fg set earlier. This is the whole reason parse_markup accumulates.
        let spans = parse_markup("#[fg=red]a#[bg=blue]b");
        assert_eq!(spans[0].style.fg, Some(Color::Named("red".into())));
        assert_eq!(spans[0].style.bg, None);
        assert_eq!(spans[1].style.fg, Some(Color::Named("red".into())));
        assert_eq!(spans[1].style.bg, Some(Color::Named("blue".into())));
    }

    #[test]
    fn parse_markup_default_resets_every_channel() {
        let spans = parse_markup("#[fg=red,bg=blue,bold]a#[default]b");
        assert!(spans[0].style.bold);
        assert_eq!(spans[1].style, Style::default());
    }

    #[test]
    fn parse_markup_channel_default_resets_only_that_channel() {
        // `fg=default` means "the terminal's own default", not black — and it
        // must leave bg alone.
        let spans = parse_markup("#[fg=red,bg=blue]a#[fg=default]b");
        assert_eq!(spans[1].style.fg, None);
        assert_eq!(spans[1].style.bg, Some(Color::Named("blue".into())));
    }

    #[test]
    fn parse_markup_handles_truecolor_and_ignores_unknown_attributes() {
        let spans = parse_markup("#[fg=#1a2b3c,align=centre,range=user|cpu]x");
        assert_eq!(spans[0].style.fg, Some(Color::Rgb(0x1a, 0x2b, 0x3c)));
        assert_eq!(spans[0].style.bg, None);
    }

    #[test]
    fn parse_markup_passes_unterminated_directives_through_as_text() {
        let spans = parse_markup("a#[fg=red");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "a#[fg=red");
        assert_eq!(spans[0].style, Style::default());
    }

    #[test]
    fn parse_markup_emits_no_span_for_directive_only_markup() {
        assert!(parse_markup("#[fg=red]#[default]").is_empty());
        assert!(parse_markup("").is_empty());
    }

    #[test]
    fn parse_markup_preserves_powerline_glyphs_and_bare_hashes() {
        let spans = parse_markup("#[fg=red]\u{e0b0} #chan");
        assert_eq!(spans[0].text, "\u{e0b0} #chan");
    }

    #[test]
    fn parse_markup_concatenated_text_round_trips_the_markup_minus_directives() {
        // The two renderers must agree on what counts as text: stripping every
        // SGR escape from tmux_to_ansi's output must equal parse_markup's
        // concatenated spans, for the real markup the renderer emits.
        let markup =
            "#[fg=colour255,bg=colour31] x:1.0 #[fg=colour31,bg=colour24]\u{e0b0}#[default]";
        let from_spans: String = parse_markup(markup)
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        let stripped = strip_sgr(&tmux_to_ansi(markup));
        assert_eq!(from_spans, stripped);
    }

    /// Remove every `ESC[...m` sequence, leaving just the visible text.
    fn strip_sgr(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for d in chars.by_ref() {
                    if d == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

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
}
