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

/// A spending cap in US dollars, measured over the last 30 days. Absent means no
/// budget line.
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
    pub project: Option<String>,
    pub pinned_models: Vec<String>,
    pub pinned_providers: Vec<String>,
}

impl ViewSection {
    pub fn view(&self) -> View {
        View {
            project: self.project.clone(),
            pinned_models: self.pinned_models.clone(),
            pinned_providers: self.pinned_providers.clone(),
        }
    }
}

/// Optional startup choices. Every value absent means today's behaviour: the Overview
/// page, all time, and the database the discovery finds.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AppSection {
    pub page: Option<String>,
    pub range: Option<String>,
    pub db: Option<PathBuf>,
    pub reload_every: Option<toml::Value>,
}

impl AppSection {
    /// How often to re-read the database: a number of seconds, or a string like
    /// "30s", "5m" or "1h". Absent means no automatic reload at all.
    pub fn reload_interval(&self) -> Result<Option<std::time::Duration>> {
        match &self.reload_every {
            None => Ok(None),
            Some(value) => parse_duration(value).map(Some).map_err(|reason| {
                Error::invalid_app_value("[app] reload_every", describe_value(value), &[reason])
            }),
        }
    }
}

/// A human reading of the TOML value, for the error when it is not a duration.
fn describe_value(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn parse_duration(value: &toml::Value) -> std::result::Result<std::time::Duration, &'static str> {
    const HINT: &str = "a number of seconds or a duration like \"30s\", \"5m\" or \"1h\"";
    match value {
        toml::Value::Integer(seconds) => u64::try_from(*seconds)
            .ok()
            .filter(|seconds| *seconds > 0)
            .map(std::time::Duration::from_secs)
            .ok_or(HINT),
        toml::Value::String(text) => {
            let (number, unit) = text.trim().split_at(
                text.trim()
                    .find(|c: char| c.is_alphabetic())
                    .unwrap_or(text.trim().len()),
            );
            let number: u64 = number.parse().map_err(|_| HINT)?;
            if number == 0 {
                return Err(HINT);
            }
            match unit {
                "s" => Ok(std::time::Duration::from_secs(number)),
                "m" => Ok(std::time::Duration::from_secs(number * 60)),
                "h" => Ok(std::time::Duration::from_secs(number * 60 * 60)),
                _ => Err(HINT),
            }
        }
        _ => Err(HINT),
    }
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
        self.app.reload_interval()?;
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
    fn the_view_section_parses_pins_and_the_project() {
        let parsed: AppConfig = toml::from_str(
            "[view]\npinned_models = [\"ccc\", \"aaa\"]\npinned_providers = [\"zulu\"]\nproject = \"demo\"\n",
        )
        .unwrap();
        let view = parsed.view.view();

        assert_eq!(
            view.pinned_models,
            vec!["ccc".to_string(), "aaa".to_string()]
        );
        assert_eq!(view.pinned_providers, vec!["zulu".to_string()]);
        assert_eq!(view.project.as_deref(), Some("demo"));

        let empty: AppConfig = toml::from_str("[view]\n").unwrap();
        let view = empty.view.view();
        assert!(view.pinned_models.is_empty() && view.pinned_providers.is_empty());
    }

    #[test]
    fn the_view_section_parses_the_project() {
        let parsed: AppConfig = toml::from_str("[view]\nproject = \"demo\"\n").unwrap();
        assert_eq!(parsed.view.view().project.as_deref(), Some("demo"));

        assert_eq!(AppConfig::default().view.view().project, None);
    }

    #[test]
    fn the_view_section_filters_nothing_unless_asked() {
        assert_eq!(AppConfig::default().view.view().label(), None);

        let parsed: AppConfig = toml::from_str("[view]\nproject = \"demo\"\n").unwrap();
        assert_eq!(
            parsed.view.view().label(),
            Some("project: demo".to_string())
        );
    }

    #[test]
    fn rejects_unknown_fields_in_the_view_section() {
        // `hide_zero_cost` is one of them: free models are never filtered out, so the
        // key that used to hide them is gone.
        for key in ["hide_free_models", "hide_zero_cost"] {
            let err = toml::from_str::<AppConfig>(&format!("[view]\n{key} = true\n")).unwrap_err();
            assert!(format!("{err:#}").contains("unknown field"));
        }
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
    fn an_absent_reload_interval_means_no_polling() {
        let parsed: AppConfig = toml::from_str("[app]\n").unwrap();
        assert_eq!(parsed.app.reload_interval().unwrap(), None);

        assert_eq!(AppConfig::default().app.reload_interval().unwrap(), None);
    }

    #[test]
    fn parses_seconds_and_suffixed_durations() {
        use std::time::Duration;

        for (text, expected) in [
            ("300", Duration::from_secs(300)),
            ("\"90s\"", Duration::from_secs(90)),
            ("\"5m\"", Duration::from_secs(300)),
            ("\"1h\"", Duration::from_secs(3600)),
        ] {
            let parsed: AppConfig =
                toml::from_str(&format!("[app]\nreload_every = {text}\n")).unwrap();
            assert_eq!(parsed.app.reload_interval().unwrap(), Some(expected));
        }
    }

    #[test]
    fn an_invalid_duration_fails_naming_the_key() {
        for text in ["\"soon\"", "\"0s\"", "0", "-5", "true"] {
            let parsed: AppConfig =
                toml::from_str(&format!("[app]\nreload_every = {text}\n")).unwrap();
            let message = format!("{}", parsed.app.reload_interval().unwrap_err());
            assert!(message.contains("[app] reload_every"), "{text}: {message}");
        }
    }

    #[test]
    fn rejects_unknown_fields_still_applies_with_the_new_key() {
        let parsed: AppConfig = toml::from_str("[app]\nreload_every = \"30s\"\n").unwrap();
        assert_eq!(
            parsed.app.reload_interval().unwrap(),
            Some(std::time::Duration::from_secs(30))
        );

        let err = toml::from_str::<AppConfig>("[app]\nreload_everyy = \"30s\"\n").unwrap_err();
        assert!(format!("{err:#}").contains("unknown field"));
    }

    #[test]
    fn every_example_in_the_config_reference_parses() {
        // docs/config.md is the reference: each fenced TOML block is a complete config,
        // so a stale example breaks the build instead of misleading a reader. Exactly one
        // block is intentionally invalid, demonstrating the startup error.
        let doc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/config.md"))
            .expect("the config reference is part of the repository");

        let mut blocks = Vec::new();
        let mut rest = doc.as_str();
        while let Some(start) = rest.find("```toml") {
            let after = &rest[start + "```toml".len()..];
            let end = after.find("```").expect("every fence closes");
            blocks.push(after[..end].trim().to_string());
            rest = &after[end + "```".len()..];
        }

        assert!(
            blocks.len() >= 6,
            "the reference shows every section, found {} TOML blocks",
            blocks.len()
        );

        let mut validated = 0;
        for block in &blocks {
            let config: AppConfig = toml::from_str(block)
                .unwrap_or_else(|_| panic!("example does not parse:\n{block}"));
            if block.contains("graphs") {
                assert!(
                    config.validate().is_err(),
                    "the intentionally invalid example still fails"
                );
            } else {
                config
                    .validate()
                    .unwrap_or_else(|_| panic!("example does not validate:\n{block}"));
                validated += 1;
            }
        }
        assert!(validated >= 5, "most examples are valid configs");
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
