use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::Path;

use ratatui::style::Color;
use serde::Deserialize;

use super::errors::{Error, Result};
use crate::config;
use crate::ui::theme::{NamedTheme, Theme, ThemeKind, builtin_themes};

#[derive(Clone, Debug, Default)]
pub struct ThemeCatalog {
    themes: BTreeMap<String, NamedTheme>,
}

impl ThemeCatalog {
    pub fn load() -> Result<Self> {
        match config::themes_dir_path() {
            Some(dir) => Self::load_from_dir(&dir),
            None => Ok(Self {
                themes: builtin_themes(),
            }),
        }
    }

    /// The built-in themes, overridden by the theme files in `dir`. A theme's name is its
    /// file name, and a file wins over a built-in with the same name. The directory need
    /// not exist, in which case only the built-ins are loaded.
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let mut themes = builtin_themes();

        if dir.exists() {
            let mut entries: Vec<_> = std::fs::read_dir(dir)
                .map_err(|e| Error::theme_read(dir, e))?
                .collect::<std::io::Result<_>>()
                .map_err(|e| Error::theme_read(dir, e))?;

            entries.sort_by_key(|e| e.path());

            for entry in entries {
                let path = entry.path();
                if path.is_file() && path.extension().and_then(OsStr::to_str) == Some("toml") {
                    let key = derive_theme_name_from_path(&path)?;
                    let file_theme = read_toml::<ThemeContent>(&path)?;
                    themes.insert(
                        key,
                        NamedTheme {
                            kind: file_theme.kind,
                            theme: file_theme.into_runtime_theme()?,
                        },
                    );
                }
            }
        }

        Ok(Self { themes })
    }

    pub fn get(&self, name: &str) -> Option<&NamedTheme> {
        self.themes.get(&name.trim().to_ascii_lowercase())
    }

    pub fn names(&self) -> Vec<&str> {
        self.themes.keys().map(String::as_str).collect()
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeContent {
    #[serde(rename = "type")]
    kind: ThemeKind,
    base: BasePalette,
    card: CardPalette,
    accent: AccentPalette,
    tab: TabPalette,
    heatmap: HeatmapPalette,
    series: SeriesPalette,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BasePalette {
    foreground: String,
    muted: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CardPalette {
    background: String,
    border: String,
    shadow: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AccentPalette {
    primary: String,
    comparison: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TabPalette {
    active_fg: String,
    active_bg: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HeatmapPalette {
    empty: String,
    active: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeriesPalette {
    model: Vec<String>,
}

impl ThemeContent {
    fn into_runtime_theme(self) -> Result<Theme> {
        Ok(Theme {
            foreground: parse_hex_color(&self.base.foreground)?,
            card_background: parse_hex_color(&self.card.background)?,
            card_border: parse_hex_color(&self.card.border)?,
            card_shadow: parse_hex_color(&self.card.shadow)?,
            muted: parse_hex_color(&self.base.muted)?,
            accent: parse_hex_color(&self.accent.primary)?,
            comparison: parse_hex_color(&self.accent.comparison)?,
            tab_active_fg: parse_hex_color(&self.tab.active_fg)?,
            tab_active_bg: parse_hex_color(&self.tab.active_bg)?,
            heat_0: parse_hex_color(&self.heatmap.empty)?,
            heat_4: parse_hex_color(&self.heatmap.active)?,
            model_series: parse_model_series(&self.series.model)?,
        })
    }
}

fn parse_model_series(values: &[String]) -> Result<[Color; 12]> {
    if values.len() != 12 {
        return Err(Error::ModelColorNum(values.len()));
    }

    let parsed: Vec<Color> = values
        .iter()
        .map(|v| parse_hex_color(v))
        .collect::<Result<_>>()?;

    parsed.try_into().map_err(|_| Error::ModelColorParse)
}

fn parse_hex_color(value: &str) -> Result<Color> {
    let raw = value.trim();
    let hex = raw
        .strip_prefix('#')
        .ok_or_else(|| Error::invalid_color(raw))?;

    if hex.len() != 6 {
        return Err(Error::invalid_color(raw));
    }

    let r = u8::from_str_radix(&hex[0..2], 16).map_err(|_| Error::invalid_color(raw))?;
    let g = u8::from_str_radix(&hex[2..4], 16).map_err(|_| Error::invalid_color(raw))?;
    let b = u8::from_str_radix(&hex[4..6], 16).map_err(|_| Error::invalid_color(raw))?;

    Ok(Color::Rgb(r, g, b))
}

fn normalize_theme_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(Error::EmptyThemeName);
    }
    Ok(trimmed.to_ascii_lowercase())
}

fn derive_theme_name_from_path(path: &Path) -> Result<String> {
    let stem = path
        .file_stem()
        .and_then(OsStr::to_str)
        .ok_or_else(|| Error::invalid_theme_filename(path))?;
    normalize_theme_name(stem)
}

fn read_toml<T>(path: &Path) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let contents = std::fs::read_to_string(path).map_err(|e| Error::theme_read(path, e))?;
    toml::from_str(&contents).map_err(|e| Error::theme_parse(path, e))
}

#[cfg(test)]
mod tests {
    use super::{ThemeCatalog, ThemeContent, ThemeKind, parse_hex_color};
    use crate::test_support::unique_temp_dir;
    use ratatui::style::Color;
    use std::path::Path;

    #[test]
    fn parses_hex_colors() {
        assert_eq!(parse_hex_color("#010203").unwrap(), Color::Rgb(1, 2, 3));
        assert!(parse_hex_color("010203").is_err());
    }

    #[test]
    fn parses_theme_content() {
        let value = toml::from_str::<ThemeContent>(
            r##"
type = "dark"

[base]
foreground = "#E5E9F0"
muted = "#808698"

[card]
background = "#1C212B"
border = "#78829B"
shadow = "#000000"

[accent]
primary = "#88C0D0"
comparison = "#B4BEFE"

[tab]
active_fg = "#000000"
active_bg = "#88C0D0"

[heatmap]
empty = "#5E6273"
active = "#88C0D0"

[series]
model = ["#BF616A", "#D08770", "#EBCB8B", "#A3BE8C", "#88C0D0", "#81A1C1", "#B48EAD", "#AB7967", "#5E81AC", "#8FBCBB", "#D8DEE9", "#4C566A"]
"##,
        )
        .unwrap();

        assert_eq!(value.kind, ThemeKind::Dark);
        assert_eq!(
            value.into_runtime_theme().unwrap().foreground,
            Color::Rgb(229, 233, 240)
        );
    }

    /// A theme file with a chosen foreground, so a loaded catalog can be told apart.
    fn theme_file(foreground: &str) -> String {
        format!(
            r##"
type = "dark"

[base]
foreground = "{foreground}"
muted = "#808698"

[card]
background = "#1C212B"
border = "#78829B"
shadow = "#000000"

[accent]
primary = "#88C0D0"
comparison = "#B4BEFE"

[tab]
active_fg = "#000000"
active_bg = "#88C0D0"

[heatmap]
empty = "#5E6273"
active = "#88C0D0"

[series]
model = ["#BF616A", "#D08770", "#EBCB8B", "#A3BE8C", "#88C0D0", "#81A1C1", "#B48EAD", "#AB7967", "#5E81AC", "#8FBCBB", "#D8DEE9", "#4C566A"]
"##
        )
    }

    fn write_theme(dir: &Path, file_name: &str, foreground: &str) {
        std::fs::write(dir.join(file_name), theme_file(foreground)).unwrap();
    }

    #[test]
    fn loads_a_theme_from_the_directory_under_its_file_name() {
        let dir = unique_temp_dir("themes-dir");
        write_theme(&dir, "Midnight Blue.toml", "#010203");

        let catalog = ThemeCatalog::load_from_dir(&dir).unwrap();
        let loaded = catalog
            .get("midnight blue")
            .expect("the file name is the theme name");

        assert_eq!(loaded.kind, ThemeKind::Dark);
        assert_eq!(loaded.theme.foreground, Color::Rgb(1, 2, 3));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_directory_theme_overrides_a_built_in_of_the_same_name() {
        let dir = unique_temp_dir("themes-override");
        assert!(
            ThemeCatalog::load_from_dir(&dir)
                .unwrap()
                .get("dark")
                .is_some(),
            "the built-in themes are there without any files"
        );

        write_theme(&dir, "dark.toml", "#040506");

        let catalog = ThemeCatalog::load_from_dir(&dir).unwrap();
        assert_eq!(
            catalog.get("dark").unwrap().theme.foreground,
            Color::Rgb(4, 5, 6),
            "the file wins over the built-in"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_directory_leaves_the_built_in_themes() {
        let dir = unique_temp_dir("themes-absent");
        std::fs::remove_dir_all(&dir).unwrap();

        let catalog = ThemeCatalog::load_from_dir(&dir).unwrap();
        assert!(catalog.get("dark").is_some() && catalog.get("light").is_some());
    }
}
