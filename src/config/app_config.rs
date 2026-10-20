use std::path::{Path, PathBuf};

use rust_decimal::Decimal;
use serde::Deserialize;

use crate::analytics::view::View;
use crate::config;
use crate::config::errors::{Error, Result};
use crate::ui::app::Page;
use crate::ui::theme::ThemeMode;
use crate::utils::time::TimeRange;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub theme: ThemeConfig,
    pub app: AppSection,
    pub view: ViewSection,
    pub budget: BudgetSection,
    pub brand: BrandSection,
}

/// Words of your own on the dashboard. Everything here is optional; unset values
/// render exactly as today.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct BrandSection {
    pub tagline: Option<String>,
    pub comparison: Option<bool>,
    pub exit_art: Option<bool>,
}

/// Brand strings with the defaults filled in, so rendering never branches on Options.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Brand {
    pub tagline: Option<String>,
    pub show_comparison: bool,
    pub show_exit_art: bool,
}

impl Default for Brand {
    fn default() -> Self {
        Self {
            tagline: None,
            show_comparison: true,
            show_exit_art: true,
        }
    }
}

impl BrandSection {
    pub fn brand(&self) -> Result<Brand> {
        if let Some(tagline) = self.tagline.as_deref()
            && tagline.trim().is_empty()
        {
            return Err(Error::invalid_app_value(
                "[brand] tagline",
                tagline,
                &["non-empty text"],
            ));
        }
        Ok(Brand {
            tagline: self.tagline.clone(),
            show_comparison: self.comparison.unwrap_or(true),
            show_exit_art: self.exit_art.unwrap_or(true),
        })
    }
}

/// A monthly spending cap in US dollars. Absent means no budget line.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct BudgetSection {
    pub monthly: Option<f64>,
}

impl BudgetSection {
    pub fn monthly_cap(&self) -> Result<Option<Decimal>> {
        match self.monthly {
            None => Ok(None),
            Some(value) => {
                let cap = Decimal::from_f64_retain(value).ok_or_else(|| {
                    Error::invalid_app_value(
                        "[budget] monthly",
                        value.to_string(),
                        &["a non-negative USD amount"],
                    )
                })?;
                if cap.is_sign_negative() {
                    return Err(Error::invalid_app_value(
                        "[budget] monthly",
                        value.to_string(),
                        &["a non-negative USD amount"],
                    ));
                }
                Ok(Some(cap))
            }
        }
    }
}

/// What to leave out of the picture. Every filter is off unless asked for.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ViewSection {
    pub hide_zero_cost: bool,
}

impl ViewSection {
    pub fn view(&self) -> View {
        View {
            hide_zero_cost: self.hide_zero_cost,
        }
    }
}

/// Optional startup choices. Every value absent means today's behaviour: the Overview
/// page, all time, and the database the discovery finds.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AppSection {
    pub page: Option<String>,
    pub range: Option<String>,
    pub db: Option<PathBuf>,
}

impl AppSection {
    pub fn page(&self) -> Result<Page> {
        match self.page.as_deref() {
            None => Ok(Page::Overview),
            Some(value) => Page::from_name(value)
                .ok_or_else(|| Error::invalid_app_value("[app] page", value, &Page::names())),
        }
    }

    pub fn range(&self) -> Result<TimeRange> {
        match self.range.as_deref() {
            None => Ok(TimeRange::All),
            Some(value) => TimeRange::from_cli_name(value)
                .ok_or_else(|| Error::invalid_app_value("[app] range", value, &TimeRange::names())),
        }
    }

    /// The configured database, if any. It is a named path, so it is used or reported
    /// missing; it never falls back to the standard locations.
    pub fn database_path(&self) -> Option<&Path> {
        self.db.as_deref()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeConfig {
    pub default: ThemeMode,
    pub dark: String,
    pub light: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            default: ThemeMode::Auto,
            dark: "dark".to_string(),
            light: "light".to_string(),
        }
    }
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        let Some(path) = config::config_path() else {
            return Ok(Self::default());
        };
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = std::fs::read_to_string(&path).map_err(|e| Error::config_read(&path, e))?;
        let parsed: Self = toml::from_str(&contents).map_err(|e| Error::config_parse(&path, e))?;
        parsed.validate()?;
        Ok(parsed)
    }

    /// Validates the sections that name a choice. Done at load so a typo is reported when
    /// the app starts rather than only when that value happens to be used, which would
    /// leave `stats` and the interactive report disagreeing about the same config.
    pub fn validate(&self) -> Result<()> {
        self.app.page()?;
        self.app.range()?;
        self.budget.monthly_cap()?;
        self.brand.brand()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, AppSection, ThemeConfig};
    use crate::ui::app::Page;
    use crate::ui::theme::ThemeMode;
    use crate::utils::time::TimeRange;
    use rust_decimal::Decimal;

    #[test]
    fn defaults_match_builtin_theme_names() {
        let config = AppConfig::default();
        assert_eq!(config.theme.default, ThemeMode::Auto);
        assert_eq!(config.theme.dark, "dark");
        assert_eq!(config.theme.light, "light");
    }

    #[test]
    fn an_absent_app_section_opens_on_the_overview_all_time() {
        let config = AppConfig::default();

        assert_eq!(config.app.page().unwrap(), Page::Overview);
        assert_eq!(config.app.range().unwrap(), TimeRange::All);
        assert_eq!(config.app.database_path(), None);

        // A present but empty section behaves the same as an absent one.
        let empty: AppConfig = toml::from_str("[app]\n").unwrap();
        assert_eq!(empty.app, AppSection::default());
        assert_eq!(empty.app.page().unwrap(), Page::Overview);
        assert_eq!(empty.app.range().unwrap(), TimeRange::All);
    }

    #[test]
    fn parses_the_app_section() {
        let parsed = toml::from_str::<AppConfig>(
            r#"
[app]
page = "sessions"
range = "7d"
db = "/tmp/opencode.db"
"#,
        )
        .unwrap();

        assert_eq!(parsed.app.page().unwrap(), Page::Sessions);
        assert_eq!(parsed.app.range().unwrap(), TimeRange::Last7Days);
        assert_eq!(
            parsed.app.database_path(),
            Some(std::path::Path::new("/tmp/opencode.db"))
        );
    }

    #[test]
    fn page_names_are_case_insensitive_and_match_the_tab_bar() {
        for page in Page::ALL {
            let parsed = toml::from_str::<AppConfig>(&format!(
                "[app]\npage = \"{}\"\n",
                page.name().to_ascii_uppercase()
            ))
            .unwrap();
            assert_eq!(parsed.app.page().unwrap(), page);
        }
        assert_eq!(
            Page::names(),
            vec!["overview", "models", "providers", "sessions"]
        );
    }

    #[test]
    fn an_unknown_page_names_the_key_and_the_accepted_values() {
        let parsed = toml::from_str::<AppConfig>("[app]\npage = \"graphs\"\n").unwrap();
        let message = format!("{}", parsed.app.page().unwrap_err());

        assert!(message.contains("[app] page"), "{message}");
        assert!(message.contains("graphs"), "{message}");
        assert!(
            message.contains("overview, models, providers, sessions"),
            "{message}"
        );
    }

    #[test]
    fn an_unknown_range_names_the_key_and_the_accepted_values() {
        let parsed = toml::from_str::<AppConfig>("[app]\nrange = \"yesterday\"\n").unwrap();
        let message = format!("{}", parsed.app.range().unwrap_err());

        assert!(message.contains("[app] range"), "{message}");
        assert!(message.contains("all, today, 7d, 30d"), "{message}");
    }

    #[test]
    fn rejects_unknown_fields_in_the_app_section() {
        let parsed = toml::from_str::<AppConfig>("[app]\npagez = \"overview\"\n").unwrap_err();
        assert!(format!("{parsed:#}").contains("unknown field"));
    }

    #[test]
    fn the_view_section_is_off_unless_asked() {
        assert!(!AppConfig::default().view.hide_zero_cost);
        assert_eq!(AppConfig::default().view.view().label(), None);

        let parsed: AppConfig = toml::from_str("[view]\nhide_zero_cost = true\n").unwrap();
        assert!(parsed.view.hide_zero_cost);
        assert_eq!(parsed.view.view().label(), Some("hiding zero-cost rows"));
    }

    #[test]
    fn rejects_unknown_fields_in_the_view_section() {
        let err = toml::from_str::<AppConfig>("[view]\nhide_free_models = true\n").unwrap_err();
        assert!(format!("{err:#}").contains("unknown field"));
    }

    #[test]
    fn an_absent_budget_means_no_cap() {
        assert_eq!(AppConfig::default().budget.monthly_cap().unwrap(), None);

        let empty: AppConfig = toml::from_str("[app]\n").unwrap();
        assert_eq!(empty.budget.monthly_cap().unwrap(), None);
    }

    #[test]
    fn parses_a_monthly_budget() {
        let parsed: AppConfig = toml::from_str("[budget]\nmonthly = 50\n").unwrap();
        assert_eq!(
            parsed.budget.monthly_cap().unwrap(),
            Some(Decimal::new(50, 0))
        );
    }

    #[test]
    fn a_negative_budget_fails_naming_the_key() {
        let parsed: AppConfig = toml::from_str("[budget]\nmonthly = -5\n").unwrap();
        let message = format!("{}", parsed.budget.monthly_cap().unwrap_err());

        assert!(message.contains("[budget] monthly"), "{message}");
        assert!(message.contains("non-negative"), "{message}");
    }

    #[test]
    fn rejects_unknown_fields_in_the_budget_section() {
        let err = toml::from_str::<AppConfig>("[budget]\nweekly = 10\n").unwrap_err();
        assert!(format!("{err:#}").contains("unknown field"));
    }

    #[test]
    fn brand_defaults_to_todays_strings() {
        let brand = AppConfig::default().brand.brand().unwrap();

        assert_eq!(brand.tagline, None);
        assert!(brand.show_comparison);
        assert!(brand.show_exit_art);
    }

    #[test]
    fn parses_every_brand_value() {
        let parsed: AppConfig = toml::from_str(
            "[brand]\ntagline = \"Ship it\"\ncomparison = false\nexit_art = false\n",
        )
        .unwrap();
        let brand = parsed.brand.brand().unwrap();

        assert_eq!(brand.tagline.as_deref(), Some("Ship it"));
        assert!(!brand.show_comparison);
        assert!(!brand.show_exit_art);
    }

    #[test]
    fn an_empty_tagline_fails_naming_the_key() {
        let parsed: AppConfig = toml::from_str("[brand]\ntagline = \"  \"\n").unwrap();
        let message = format!("{}", parsed.brand.brand().unwrap_err());

        assert!(message.contains("[brand] tagline"), "{message}");
        assert!(message.contains("non-empty"), "{message}");
    }

    #[test]
    fn rejects_unknown_fields_in_the_brand_section() {
        let err = toml::from_str::<AppConfig>("[brand]\nfont = \"x\"\n").unwrap_err();
        assert!(format!("{err:#}").contains("unknown field"));
    }

    #[test]
    fn load_rejects_invalid_choices_before_anything_uses_them() {
        // `validate` is what `load` runs, so a typo is an error at startup for every
        // invocation, including the scripted ones that never read the page or range.
        let bad_page: AppConfig = toml::from_str("[app]\npage = \"graphs\"\n").unwrap();
        assert!(bad_page.validate().is_err());

        let bad_range: AppConfig = toml::from_str("[app]\nrange = \"yesterday\"\n").unwrap();
        assert!(bad_range.validate().is_err());

        let fine: AppConfig =
            toml::from_str("[app]\npage = \"models\"\nrange = \"30d\"\n").unwrap();
        assert!(fine.validate().is_ok());

        assert!(AppConfig::default().validate().is_ok());
    }

    #[test]
    fn parses_theme_block() {
        let parsed = toml::from_str::<AppConfig>(
            r#"
[theme]
default = "light"
dark = "nord"
light = "paper"
"#,
        )
        .unwrap();

        assert_eq!(parsed.theme.default, ThemeMode::Light);
        assert_eq!(parsed.theme.dark, "nord");
        assert_eq!(parsed.theme.light, "paper");
    }

    #[test]
    fn allows_missing_theme_block() {
        let parsed = toml::from_str::<AppConfig>("").unwrap();
        assert_eq!(parsed.theme, ThemeConfig::default());
    }

    #[test]
    fn rejects_unknown_fields() {
        let parsed = toml::from_str::<AppConfig>(
            r#"
[theme]
default = "auto"
dark = "dark"
light = "light"
extra = "nope"
"#,
        )
        .unwrap_err();
        assert!(format!("{parsed:#}").contains("unknown field"));
    }
}
