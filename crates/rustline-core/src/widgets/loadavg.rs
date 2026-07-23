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
    pub warn_load: f64,
    pub crit_load: f64,
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
                let text = substitute(fmt, Some(vals));
                let kind = crate::widgets::alert_over(vals[0], self.warn_load, self.crit_load);
                match crate::widgets::alert_style(kind, &ctx.colors) {
                    Some(style) => vec![Segment::styled(text, style)],
                    None => vec![Segment::new(text)],
                }
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
            git: None,
            disk: None,
            throughput: None,
            os: String::new(),
            arch: String::new(),
            uptime: None,
            media: None,
            toggled: Default::default(),
            colors: Default::default(),
        }
    }

    fn w(format: &str, alt: &str, down: &str) -> LoadAvg {
        LoadAvg {
            format: format.into(),
            alt_format: alt.into(),
            down_format: down.into(),
            warn_load: 0.0,
            crit_load: 0.0,
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
        // 0.2965 has no exact f64 representation; the nearest value is
        // 0.29649999999999998579..., which correctly rounds down at 3 d.p.
        // (verified: `format!("{:.3}", 0.2965_f64)` == "0.296" in plain Rust,
        // matching this scanner's plain `format!("{:.*}", precision, v)`).
        assert_eq!(substitute("{load1:.3}", Some([0.2965, 0.0, 0.0])), "0.296");
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
        assert_eq!(
            substitute("{load1:.15}", Some([0.5, 0.0, 0.0])),
            "0.5000000000"
        );
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
        let out =
            w("{load1} {load5} {load15}", "", "").render(&ctx_load(Some([0.42, 0.31, 0.296])));
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

    #[test]
    fn default_thresholds_off_no_style() {
        // Load-bearing: default (0/0) -> plain segment, unchanged output.
        let out = w("{load1}", "", "").render(&ctx_load(Some([9.9, 0.0, 0.0])));
        assert_eq!(out[0].text, "9.90");
        assert_eq!(out[0].style, crate::Style::default());
    }

    #[test]
    fn configured_thresholds_alert_on_load1() {
        let mut c = ctx_load(Some([6.0, 1.0, 1.0]));
        c.colors = crate::ThemeColors {
            warning: crate::Color::Indexed(214),
            error: crate::Color::Indexed(196),
            bar_bg: crate::Color::Indexed(234),
            ..Default::default()
        };
        let mut widget = w("{load1}", "", "");
        widget.warn_load = 4.0;
        widget.crit_load = 8.0;
        let out = widget.render(&c); // 6 >= warn(4) -> warn
        assert_eq!(out[0].style.bg, Some(crate::Color::Indexed(214)));
    }
}
