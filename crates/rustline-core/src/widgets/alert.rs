//! Shared threshold-alert helper for the numeric widgets (cpu/memory/battery/
//! loadavg). A crossed threshold turns the widget's cell into an inverse alert
//! badge: `bg = <semantic color>`, `fg = bar_bg` (dark in every theme, so the
//! badge always contrasts), `bold`. A threshold of `0` (or less) disables that
//! tier — so a widget with both tiers at `0` renders byte-identically to before.

use crate::{Style, ThemeColors};

/// Which alert tier a reading falls in. `Crit` outranks `Warn`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AlertKind {
    None,
    Warn,
    Crit,
}

/// "Higher is worse" (cpu %, memory %, load1): `value >= crit` -> `Crit`,
/// `value >= warn` -> `Warn`. A tier threshold `<= 0` is disabled.
pub(crate) fn alert_over(value: f64, warn: f64, crit: f64) -> AlertKind {
    if crit > 0.0 && value >= crit {
        AlertKind::Crit
    } else if warn > 0.0 && value >= warn {
        AlertKind::Warn
    } else {
        AlertKind::None
    }
}

/// "Lower is worse" (battery %): `value <= crit` -> `Crit`, `value <= warn` ->
/// `Warn`. A tier threshold `<= 0` is disabled.
pub(crate) fn alert_under(value: f64, warn: f64, crit: f64) -> AlertKind {
    if crit > 0.0 && value <= crit {
        AlertKind::Crit
    } else if warn > 0.0 && value <= warn {
        AlertKind::Warn
    } else {
        AlertKind::None
    }
}

/// The alert badge style for `kind`, or `None` when not alerting (leaving the
/// segment to normal palette assignment).
pub(crate) fn alert_style(kind: AlertKind, colors: &ThemeColors) -> Option<Style> {
    let bg = match kind {
        AlertKind::None => return None,
        AlertKind::Warn => colors.warning.clone(),
        AlertKind::Crit => colors.error.clone(),
    };
    Some(Style {
        fg: Some(colors.bar_bg.clone()),
        bg: Some(bg),
        bold: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Color;

    #[test]
    fn over_thresholds_and_disabled_tiers() {
        assert_eq!(alert_over(50.0, 80.0, 95.0), AlertKind::None);
        assert_eq!(alert_over(80.0, 80.0, 95.0), AlertKind::Warn); // boundary inclusive
        assert_eq!(alert_over(95.0, 80.0, 95.0), AlertKind::Crit); // crit beats warn
        assert_eq!(alert_over(99.0, 0.0, 0.0), AlertKind::None); // both disabled
        assert_eq!(alert_over(99.0, 0.0, 95.0), AlertKind::Crit); // warn off, crit on
        assert_eq!(alert_over(85.0, 80.0, 0.0), AlertKind::Warn); // crit off, warn on
    }

    #[test]
    fn under_thresholds_for_battery() {
        assert_eq!(alert_under(50.0, 20.0, 10.0), AlertKind::None);
        assert_eq!(alert_under(20.0, 20.0, 10.0), AlertKind::Warn);
        assert_eq!(alert_under(10.0, 20.0, 10.0), AlertKind::Crit);
        assert_eq!(alert_under(5.0, 0.0, 0.0), AlertKind::None); // disabled
    }

    #[test]
    fn style_uses_semantic_bg_and_bar_bg_fg_bold() {
        let colors = ThemeColors {
            warning: Color::Indexed(214),
            error: Color::Indexed(196),
            bar_bg: Color::Indexed(234),
            ..ThemeColors::default()
        };
        assert_eq!(alert_style(AlertKind::None, &colors), None);
        let warn = alert_style(AlertKind::Warn, &colors).unwrap();
        assert_eq!(warn.bg, Some(Color::Indexed(214)));
        assert_eq!(warn.fg, Some(Color::Indexed(234)));
        assert!(warn.bold);
        let crit = alert_style(AlertKind::Crit, &colors).unwrap();
        assert_eq!(crit.bg, Some(Color::Indexed(196)));
    }
}
