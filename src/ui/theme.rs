use std::collections::BTreeMap;

use clap::ValueEnum;
use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;

use crate::analytics::heatmap_data::HEAT_LEVELS;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    Auto,
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ThemeKind {
    Dark,
    Light,
}

impl ThemeMode {
    pub fn resolve(self) -> ThemeKind {
        match self {
            Self::Auto => detect_terminal_theme().unwrap_or(ThemeKind::Dark),
            Self::Dark => ThemeKind::Dark,
            Self::Light => ThemeKind::Light,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub foreground: Color,
    pub card_background: Color,
    pub card_border: Color,
    pub card_shadow: Color,
    pub muted: Color,
    pub accent: Color,
    pub comparison: Color,
    pub tab_active_fg: Color,
    pub tab_active_bg: Color,
    pub heat_0: Color,
    pub heat_5: Color,
    pub model_series: [Color; 12],
}

impl Theme {
    pub fn builtin_dark() -> Self {
        Self {
            foreground: Color::Rgb(229, 233, 240),
            card_background: Color::Rgb(26, 26, 26),
            card_border: Color::Rgb(120, 130, 155),
            card_shadow: Color::Rgb(0, 0, 0),
            muted: Color::Rgb(128, 134, 152),
            accent: Color::Rgb(136, 192, 208),
            comparison: Color::Rgb(180, 190, 254),
            tab_active_fg: Color::Black,
            tab_active_bg: Color::Rgb(136, 192, 208),
            heat_0: Color::Rgb(94, 98, 115),
            heat_5: Color::Rgb(136, 192, 208),
            model_series: [
                Color::Rgb(191, 97, 106),
                Color::Rgb(208, 135, 112),
                Color::Rgb(235, 203, 139),
                Color::Rgb(163, 190, 140),
                Color::Rgb(136, 192, 208),
                Color::Rgb(129, 161, 193),
                Color::Rgb(180, 142, 173),
                Color::Rgb(171, 121, 103),
                Color::Rgb(94, 129, 172),
                Color::Rgb(143, 188, 187),
                Color::Rgb(216, 222, 233),
                Color::Rgb(76, 86, 106),
            ],
        }
    }

    pub fn builtin_light() -> Self {
        Self {
            foreground: Color::Rgb(37, 41, 51),
            card_background: Color::Rgb(252, 253, 255),
            card_border: Color::Rgb(173, 183, 201),
            card_shadow: Color::Rgb(96, 107, 128),
            muted: Color::Rgb(90, 98, 115),
            accent: Color::Rgb(0, 122, 163),
            comparison: Color::Rgb(94, 92, 230),
            tab_active_fg: Color::White,
            tab_active_bg: Color::Rgb(0, 122, 163),
            heat_0: Color::Rgb(160, 170, 186),
            heat_5: Color::Rgb(0, 122, 163),
            model_series: [
                Color::Rgb(167, 40, 40),
                Color::Rgb(175, 94, 0),
                Color::Rgb(145, 108, 0),
                Color::Rgb(51, 122, 68),
                Color::Rgb(0, 122, 163),
                Color::Rgb(50, 88, 160),
                Color::Rgb(126, 76, 142),
                Color::Rgb(120, 78, 52),
                Color::Rgb(72, 106, 154),
                Color::Rgb(70, 150, 154),
                Color::Rgb(34, 34, 34),
                Color::Rgb(90, 98, 115),
            ],
        }
    }

    pub fn builtin_for(kind: ThemeKind) -> Self {
        match kind {
            ThemeKind::Dark => Self::builtin_dark(),
            ThemeKind::Light => Self::builtin_light(),
        }
    }

    pub fn muted_style(&self) -> Style {
        Style::default().fg(self.muted)
    }

    pub fn accent_style(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::BOLD)
    }

    pub fn comparison_style(&self) -> Style {
        Style::default()
            .fg(self.comparison)
            .add_modifier(Modifier::BOLD)
    }

    pub fn series_color(&self, index: usize) -> Color {
        self.model_series[index % self.model_series.len()]
    }

    /// The colour of a heatmap intensity level. Level 0 is the empty colour and the last
    /// level the active one, so the levels between are derived from the theme's two
    /// endpoints rather than configured.
    pub fn heat_level(&self, level: u8) -> Color {
        let top = (HEAT_LEVELS - 1) as u8;
        match level {
            0 => self.heat_0,
            level if level >= top => self.heat_5,
            level => lerp_color(self.heat_0, self.heat_5, f32::from(level) / f32::from(top)),
        }
    }
}

fn lerp_color(from: Color, to: Color, t: f32) -> Color {
    let (Color::Rgb(from_red, from_green, from_blue), Color::Rgb(to_red, to_green, to_blue)) =
        (from, to)
    else {
        return from;
    };

    Color::Rgb(
        lerp_channel(from_red, to_red, t),
        lerp_channel(from_green, to_green, t),
        lerp_channel(from_blue, to_blue, t),
    )
}

fn lerp_channel(from: u8, to: u8, t: f32) -> u8 {
    let value = f32::from(from) + (f32::from(to) - f32::from(from)) * t;
    value.round().clamp(f32::from(u8::MIN), f32::from(u8::MAX)) as u8
}

#[derive(Clone, Debug)]
pub struct NamedTheme {
    pub kind: ThemeKind,
    pub theme: Theme,
}

pub fn builtin_themes() -> BTreeMap<String, NamedTheme> {
    let mut themes = BTreeMap::new();
    themes.insert(
        "dark".to_string(),
        NamedTheme {
            kind: ThemeKind::Dark,
            theme: Theme::builtin_for(ThemeKind::Dark),
        },
    );
    themes.insert(
        "light".to_string(),
        NamedTheme {
            kind: ThemeKind::Light,
            theme: Theme::builtin_for(ThemeKind::Light),
        },
    );
    themes
}

fn detect_terminal_theme() -> Option<ThemeKind> {
    let env_hint = std::env::var("TERM_THEME")
        .ok()
        .or_else(|| std::env::var("TERM_BACKGROUND").ok());
    if let Some(mode) = env_hint.and_then(|value| parse_mode_hint(&value)) {
        return Some(mode);
    }

    if let Some(mode) = detect_terminal_theme_from_luma() {
        return Some(mode);
    }

    std::env::var("COLORFGBG")
        .ok()
        .and_then(|value| parse_colorfgbg(&value))
}

fn detect_terminal_theme_from_luma() -> Option<ThemeKind> {
    terminal_light::luma().ok().map(|luma| {
        if luma > 0.6 {
            ThemeKind::Light
        } else {
            ThemeKind::Dark
        }
    })
}

fn parse_mode_hint(value: &str) -> Option<ThemeKind> {
    let lowercase = value.trim().to_ascii_lowercase();
    if lowercase.contains("dark") {
        return Some(ThemeKind::Dark);
    }
    if lowercase.contains("light") {
        return Some(ThemeKind::Light);
    }
    None
}

fn parse_colorfgbg(value: &str) -> Option<ThemeKind> {
    let background = value.split(';').next_back()?.trim().parse::<u8>().ok()?;
    if background <= 6 || background == 8 {
        Some(ThemeKind::Dark)
    } else {
        Some(ThemeKind::Light)
    }
}

#[cfg(test)]
mod tests {
    use super::{Theme, ThemeKind, parse_colorfgbg, parse_mode_hint};
    use crate::analytics::heatmap_data::HEAT_LEVELS;

    fn channel_delta(left: ratatui::style::Color, right: ratatui::style::Color) -> u32 {
        let (
            ratatui::style::Color::Rgb(left_red, left_green, left_blue),
            ratatui::style::Color::Rgb(right_red, right_green, right_blue),
        ) = (left, right)
        else {
            panic!("expected rgb colours, got {left:?} and {right:?}");
        };

        u32::from(left_red.abs_diff(right_red))
            + u32::from(left_green.abs_diff(right_green))
            + u32::from(left_blue.abs_diff(right_blue))
    }

    #[test]
    fn builtin_themes_have_a_distinct_heat_ramp() {
        for theme in [Theme::builtin_dark(), Theme::builtin_light()] {
            let levels = (0..HEAT_LEVELS as u8)
                .map(|level| theme.heat_level(level))
                .collect::<Vec<_>>();

            let mut distinct = levels.clone();
            distinct.sort_by_key(|color| format!("{color:?}"));
            distinct.dedup();
            assert_eq!(distinct.len(), levels.len(), "levels must be distinct");

            for pair in levels.windows(2) {
                assert!(
                    channel_delta(pair[0], pair[1]) >= 20,
                    "adjacent heat levels must be visibly apart, got {pair:?}"
                );
            }

            assert_eq!(levels[0], theme.heat_0);
            assert_eq!(levels[HEAT_LEVELS - 1], theme.heat_5);
        }
    }

    #[test]
    fn parses_term_theme_hint() {
        assert_eq!(parse_mode_hint("dark"), Some(ThemeKind::Dark));
        assert_eq!(parse_mode_hint("LIGHT"), Some(ThemeKind::Light));
        assert_eq!(parse_mode_hint("unknown"), None);
    }

    #[test]
    fn parses_colorfgbg_background_index() {
        assert_eq!(parse_colorfgbg("15;0"), Some(ThemeKind::Dark));
        assert_eq!(parse_colorfgbg("0;15"), Some(ThemeKind::Light));
    }
}
