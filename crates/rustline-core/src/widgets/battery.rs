use crate::{Battery, BatteryState, Context, Segment, Widget};

/// Renders battery percentage, charge state, and a level-bucketed,
/// charging-aware Nerd-Font icon. Pure — reads only `Context::battery`.
pub struct BatteryWidget {
    pub format: String,
    pub alt_format: String,
    pub down_format: String,
    pub warn_percent: f64,
    pub crit_percent: f64,
    /// Overrides `{icon}` with a fixed glyph, replacing the level-bucketed,
    /// charging-aware computed icon ([`battery_icon`]) entirely. `None`
    /// keeps the computed glyph.
    pub icon: Option<String>,
}

impl BatteryWidget {
    /// Registry/layout name; the toggle key threaded through render + click.
    pub const NAME: &'static str = "battery";
}

/// A Nerd-Font (nf-md battery ramp) glyph for the given battery. Charging →
/// charging glyph; `Full` → full glyph; `Unknown` → unknown glyph; otherwise a
/// discharge-level bucket. Pure + unit-tested; the bucketing is the contract,
/// the exact codepoints are the nf-md battery set.
fn battery_icon(b: &Battery) -> &'static str {
    match b.state {
        BatteryState::Charging => "\u{f0084}", // md-battery-charging
        BatteryState::Full => "\u{f0079}",     // md-battery (full)
        BatteryState::Unknown => "\u{f0091}",  // md-battery-unknown
        BatteryState::Discharging => match b.percent {
            p if p >= 90 => "\u{f0082}", // md-battery-90
            p if p >= 70 => "\u{f0080}", // md-battery-70
            p if p >= 50 => "\u{f007e}", // md-battery-50
            p if p >= 30 => "\u{f007c}", // md-battery-30
            p if p >= 10 => "\u{f007a}", // md-battery-10
            _ => "\u{f0083}",            // md-battery-alert (<10%)
        },
    }
}

/// The lowercase state word substituted for `{state}`.
fn state_word(state: BatteryState) -> &'static str {
    match state {
        BatteryState::Charging => "charging",
        BatteryState::Discharging => "discharging",
        BatteryState::Full => "full",
        BatteryState::Unknown => "unknown",
    }
}

impl Widget for BatteryWidget {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        match ctx.battery {
            Some(b) => {
                let fmt =
                    crate::widgets::active_format(ctx, Self::NAME, &self.format, &self.alt_format);
                let icon = self.icon.as_deref().unwrap_or_else(|| battery_icon(&b));
                let text = fmt
                    .replace("{icon}", icon)
                    .replace("{percent}", &b.percent.to_string())
                    .replace("{state}", state_word(b.state));
                let kind = if b.state == BatteryState::Discharging {
                    crate::widgets::alert_under(
                        b.percent as f64,
                        self.warn_percent,
                        self.crit_percent,
                    )
                } else {
                    crate::widgets::AlertKind::None
                };
                match crate::widgets::alert_style(kind, &ctx.colors) {
                    Some(style) => vec![Segment::styled(text, style)],
                    None => vec![Segment::new(text)],
                }
            }
            None => {
                if self.down_format.is_empty() {
                    return vec![];
                }
                // Collapse any placeholder so a stray token never renders and
                // no fake value shows (invariant #6).
                let text = self
                    .down_format
                    .replace("{icon}", "")
                    .replace("{percent}", "")
                    .replace("{state}", "");
                vec![Segment::new(text)]
            }
        }
    }

    fn range_name(&self) -> Option<&str> {
        crate::widgets::clickable_range(Self::NAME, &self.alt_format)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};

    fn ctx(battery: Option<Battery>) -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/".into(),
            home: "/h".into(),
            hostname: "h".into(),
            loadavg: None,
            now: Local
                .with_ymd_and_hms(2026, 7, 20, 17, 49, 0)
                .single()
                .unwrap(),
            window: None,
            interfaces: Vec::new(),
            battery,
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

    fn bat(percent: u8, state: BatteryState) -> Option<Battery> {
        Some(Battery { percent, state })
    }

    fn w() -> BatteryWidget {
        BatteryWidget {
            format: "{icon} {percent}%".into(),
            alt_format: String::new(),
            down_format: String::new(),
            warn_percent: 20.0,
            crit_percent: 10.0,
            icon: None,
        }
    }

    #[test]
    fn renders_icon_percent_state() {
        let widget = BatteryWidget {
            format: "{icon} {percent}% {state}".into(),
            alt_format: String::new(),
            down_format: String::new(),
            warn_percent: 20.0,
            crit_percent: 10.0,
            icon: None,
        };
        let out = widget.render(&ctx(bat(73, BatteryState::Discharging)));
        assert_eq!(out[0].text, "\u{f0080} 73% discharging");
    }

    #[test]
    fn icon_buckets_by_level_and_state() {
        assert_eq!(
            battery_icon(&Battery {
                percent: 40,
                state: BatteryState::Charging
            }),
            "\u{f0084}"
        );
        assert_eq!(
            battery_icon(&Battery {
                percent: 100,
                state: BatteryState::Full
            }),
            "\u{f0079}"
        );
        assert_eq!(
            battery_icon(&Battery {
                percent: 95,
                state: BatteryState::Discharging
            }),
            "\u{f0082}"
        );
        // Exact >=90 boundary: 90 is in-bucket, 89 drops to the next (spec §8).
        assert_eq!(
            battery_icon(&Battery {
                percent: 90,
                state: BatteryState::Discharging
            }),
            "\u{f0082}"
        );
        assert_eq!(
            battery_icon(&Battery {
                percent: 89,
                state: BatteryState::Discharging
            }),
            "\u{f0080}"
        );
        assert_eq!(
            battery_icon(&Battery {
                percent: 70,
                state: BatteryState::Discharging
            }),
            "\u{f0080}"
        );
        assert_eq!(
            battery_icon(&Battery {
                percent: 50,
                state: BatteryState::Discharging
            }),
            "\u{f007e}"
        );
        assert_eq!(
            battery_icon(&Battery {
                percent: 30,
                state: BatteryState::Discharging
            }),
            "\u{f007c}"
        );
        assert_eq!(
            battery_icon(&Battery {
                percent: 10,
                state: BatteryState::Discharging
            }),
            "\u{f007a}"
        );
        assert_eq!(
            battery_icon(&Battery {
                percent: 5,
                state: BatteryState::Discharging
            }),
            "\u{f0083}"
        );
        assert_eq!(
            battery_icon(&Battery {
                percent: 50,
                state: BatteryState::Unknown
            }),
            "\u{f0091}"
        );
    }

    #[test]
    fn none_with_empty_down_format_skips() {
        assert!(w().render(&ctx(None)).is_empty());
    }

    #[test]
    fn none_with_down_format_renders_and_collapses_placeholders() {
        let widget = BatteryWidget {
            format: "{icon} {percent}%".into(),
            alt_format: String::new(),
            down_format: "no-batt {percent}{icon}{state}".into(),
            warn_percent: 20.0,
            crit_percent: 10.0,
            icon: None,
        };
        let out = widget.render(&ctx(None));
        assert_eq!(out[0].text, "no-batt ");
    }

    #[test]
    fn battery_toggled_uses_alt_format() {
        let mut c = ctx(bat(73, BatteryState::Discharging));
        c.toggled.insert("battery".to_string());
        let out = BatteryWidget {
            format: "{percent}%".into(),
            alt_format: "{icon} {percent}% {state}".into(),
            down_format: String::new(),
            warn_percent: 20.0,
            crit_percent: 10.0,
            icon: None,
        }
        .render(&c);
        assert_eq!(out[0].text, "\u{f0080} 73% discharging");
    }

    #[test]
    fn battery_range_name_tracks_alt() {
        assert_eq!(
            BatteryWidget {
                format: "x".into(),
                alt_format: String::new(),
                down_format: String::new(),
                warn_percent: 20.0,
                crit_percent: 10.0,
                icon: None,
            }
            .range_name(),
            None
        );
        assert_eq!(
            BatteryWidget {
                format: "x".into(),
                alt_format: "{state}".into(),
                down_format: String::new(),
                warn_percent: 20.0,
                crit_percent: 10.0,
                icon: None,
            }
            .range_name(),
            Some("battery")
        );
    }

    #[test]
    fn battery_icon_override_replaces_bucketed_glyph() {
        let mut widget = w();
        widget.icon = Some("B".into());
        // 73% discharging would normally bucket to \u{f0080}; the override wins.
        let out = widget.render(&ctx(bat(73, BatteryState::Discharging)));
        assert_eq!(out[0].text, "B 73%");
    }

    #[test]
    fn battery_icon_none_uses_default() {
        // Characterization: an unset icon renders the computed bucketed glyph
        // unchanged.
        let out = w().render(&ctx(bat(73, BatteryState::Discharging)));
        assert_eq!(out[0].text, "\u{f0080} 73%");
    }

    #[test]
    fn low_discharging_alerts_but_charging_does_not() {
        let mut c = ctx(bat(15, BatteryState::Discharging));
        c.colors = crate::ThemeColors {
            warning: crate::Color::Indexed(214),
            error: crate::Color::Indexed(196),
            bar_bg: crate::Color::Indexed(234),
            ..Default::default()
        };
        let out = w().render(&c); // 15% <= warn(20) -> warn
        assert_eq!(out[0].style.bg, Some(crate::Color::Indexed(214)));

        let mut c2 = ctx(bat(15, BatteryState::Charging));
        c2.colors = c.colors.clone();
        let out = w().render(&c2); // charging -> no alert
        assert_eq!(out[0].style, crate::Style::default());

        let mut c3 = ctx(bat(8, BatteryState::Discharging));
        c3.colors = c.colors.clone();
        let out = w().render(&c3); // 8% <= crit(10) -> crit
        assert_eq!(out[0].style.bg, Some(crate::Color::Indexed(196)));
    }
}
