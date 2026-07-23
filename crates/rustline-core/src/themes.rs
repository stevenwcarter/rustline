//! Built-in named themes. Each is a complete `Theme`; non-`default` themes are
//! multi-accent (palette length >= 4). Curated schemes use truecolor (RGB).
//! Every theme's `fg` is chosen to contrast with its palette accents; the
//! threshold alert badge (widgets) always uses `bar_bg` as its text color, so
//! semantic colors only need to be brighter than `bar_bg`.

use crate::{Color, Theme};

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

/// Look up a built-in theme by name; `None` for an unknown name.
pub fn builtin_theme(name: &str) -> Option<Theme> {
    Some(match name {
        "default" => Theme::default(),
        "pastel-rainbow" => pastel_rainbow(),
        "nord" => nord(),
        "gruvbox" => gruvbox(),
        "catppuccin-mocha" => catppuccin_mocha(),
        "tokyo-night" => tokyo_night(),
        "dracula" => dracula(),
        _ => return None,
    })
}

/// The built-in theme names, in display order.
pub fn builtin_theme_names() -> &'static [&'static str] {
    &[
        "default",
        "pastel-rainbow",
        "nord",
        "gruvbox",
        "catppuccin-mocha",
        "tokyo-night",
        "dracula",
    ]
}

fn pastel_rainbow() -> Theme {
    Theme {
        palette: vec![
            rgb(244, 166, 184),
            rgb(246, 199, 169),
            rgb(245, 230, 163),
            rgb(184, 230, 196),
            rgb(169, 211, 240),
            rgb(208, 189, 240),
        ],
        fg: rgb(43, 43, 58),
        bar_bg: rgb(42, 42, 54),
        soft_fg: rgb(122, 122, 138),
        win_current_bg: rgb(195, 169, 238),
        win_current_fg: rgb(43, 43, 58),
        win_inactive_bg: rgb(207, 207, 218),
        win_inactive_fg: rgb(85, 85, 106),
        success: rgb(168, 224, 176),
        info: rgb(169, 211, 240),
        warning: rgb(243, 217, 139),
        error: rgb(242, 161, 161),
        ..Theme::default()
    }
}

fn nord() -> Theme {
    Theme {
        palette: vec![
            rgb(94, 129, 172),
            rgb(129, 161, 193),
            rgb(163, 190, 140),
            rgb(180, 142, 173),
            rgb(208, 135, 112),
        ],
        fg: rgb(216, 222, 233),
        bar_bg: rgb(46, 52, 64),
        soft_fg: rgb(76, 86, 106),
        win_current_bg: rgb(136, 192, 208),
        win_current_fg: rgb(46, 52, 64),
        win_inactive_bg: rgb(59, 66, 82),
        win_inactive_fg: rgb(216, 222, 233),
        success: rgb(163, 190, 140),
        info: rgb(136, 192, 208),
        warning: rgb(235, 203, 139),
        error: rgb(191, 97, 106),
        ..Theme::default()
    }
}

fn gruvbox() -> Theme {
    Theme {
        palette: vec![
            rgb(215, 153, 33),
            rgb(152, 151, 26),
            rgb(69, 133, 136),
            rgb(177, 98, 134),
            rgb(214, 93, 14),
        ],
        fg: rgb(235, 219, 178),
        bar_bg: rgb(40, 40, 40),
        soft_fg: rgb(102, 92, 84),
        win_current_bg: rgb(250, 189, 47),
        win_current_fg: rgb(40, 40, 40),
        win_inactive_bg: rgb(60, 56, 54),
        win_inactive_fg: rgb(235, 219, 178),
        success: rgb(184, 187, 38),
        info: rgb(131, 165, 152),
        warning: rgb(250, 189, 47),
        error: rgb(251, 73, 52),
        ..Theme::default()
    }
}

fn catppuccin_mocha() -> Theme {
    Theme {
        palette: vec![
            rgb(137, 180, 250),
            rgb(245, 194, 231),
            rgb(166, 227, 161),
            rgb(249, 226, 175),
            rgb(250, 179, 135),
        ],
        fg: rgb(17, 17, 27),
        bar_bg: rgb(30, 30, 46),
        soft_fg: rgb(88, 91, 112),
        win_current_bg: rgb(203, 166, 247),
        win_current_fg: rgb(17, 17, 27),
        win_inactive_bg: rgb(49, 50, 68),
        win_inactive_fg: rgb(205, 214, 244),
        success: rgb(166, 227, 161),
        info: rgb(137, 220, 235),
        warning: rgb(249, 226, 175),
        error: rgb(243, 139, 168),
        ..Theme::default()
    }
}

fn tokyo_night() -> Theme {
    Theme {
        palette: vec![
            rgb(122, 162, 247),
            rgb(187, 154, 247),
            rgb(125, 207, 255),
            rgb(158, 206, 106),
            rgb(224, 175, 104),
        ],
        fg: rgb(22, 22, 30),
        bar_bg: rgb(26, 27, 38),
        soft_fg: rgb(86, 95, 137),
        win_current_bg: rgb(122, 162, 247),
        win_current_fg: rgb(22, 22, 30),
        win_inactive_bg: rgb(41, 46, 66),
        win_inactive_fg: rgb(192, 202, 245),
        success: rgb(158, 206, 106),
        info: rgb(125, 207, 255),
        warning: rgb(224, 175, 104),
        error: rgb(247, 118, 142),
        ..Theme::default()
    }
}

fn dracula() -> Theme {
    Theme {
        palette: vec![
            rgb(189, 147, 249), // purple
            rgb(255, 121, 198), // pink
            rgb(139, 233, 253), // cyan
            rgb(80, 250, 123),  // green
            rgb(255, 184, 108), // orange
        ],
        fg: rgb(40, 42, 54),
        bar_bg: rgb(40, 42, 54),
        soft_fg: rgb(98, 114, 164),
        win_current_bg: rgb(189, 147, 249),
        win_current_fg: rgb(40, 42, 54),
        win_inactive_bg: rgb(68, 71, 90),
        win_inactive_fg: rgb(248, 248, 242),
        success: rgb(80, 250, 123),
        info: rgb(139, 233, 253),
        warning: rgb(241, 250, 140),
        error: rgb(255, 85, 85),
        ..Theme::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_resolves_and_unknown_is_none() {
        for name in builtin_theme_names() {
            assert!(builtin_theme(name).is_some(), "missing built-in: {name}");
        }
        assert!(builtin_theme("nope").is_none());
        assert_eq!(builtin_theme_names().len(), 7);
    }

    #[test]
    fn non_default_themes_are_multi_accent_and_distinct() {
        for name in builtin_theme_names().iter().filter(|n| **n != "default") {
            let t = builtin_theme(name).unwrap();
            assert!(t.palette.len() >= 4, "{name} not multi-accent");
            // Not accidentally the default theme (a real override happened).
            assert_ne!(t.bar_bg, Theme::default().bar_bg, "{name} == default bg");
        }
    }

    #[test]
    fn themes_keep_default_separators() {
        // Curated themes inherit the powerline separators/caps from default.
        let t = nord();
        assert_eq!(t.hard_left, Theme::default().hard_left);
        assert_eq!(t.win_cap_left, Theme::default().win_cap_left);
    }
}
