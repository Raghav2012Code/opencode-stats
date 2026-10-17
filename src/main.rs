mod analytics;
mod cache;
mod config;
mod db;
mod stats_output;
mod ui;
mod utils;

use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use color_eyre::eyre::{Context, ContextCompat, Result, bail};

use crate::analytics::build_snapshot;
use crate::cache::models_cache::{PricingCatalog, default_cache_path, refresh_pricing_catalog};
use crate::config::app_config::AppConfig;
use crate::config::theme_config::ThemeCatalog;
use crate::db::models::InputOptions;
use crate::db::queries::load_app_data;
use crate::stats_output::{StatsFormat, format_stats_line, stats_csv, stats_json};
use crate::ui::app::{App, print_exit_art};
use crate::ui::theme::{Theme, ThemeKind, ThemeMode};
use crate::utils::pricing::ZeroCostBehavior;
use crate::utils::time::TimeRange;

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    let cli = CliArgs::parse();
    if let Some(command) = cli.command {
        return run_command(command, cli.database_path, cli.ignore_zero).await;
    }

    let input_options = InputOptions {
        database_path: cli.database_path,
    };
    let data = load_app_data(&input_options).wrap_err("failed to load OpenCode usage data")?;

    let pricing = PricingCatalog::load().wrap_err("failed to load pricing catalog")?;
    let (theme_kind, theme) = resolve_theme(cli.theme).wrap_err("failed to resolve theme")?;
    let zero_cost_behavior = ZeroCostBehavior::from_ignore_zero(cli.ignore_zero);
    let app = App::new(data, pricing, theme, zero_cost_behavior, input_options);
    app.run().await?;
    print_exit_art(theme_kind);
    Ok(())
}

#[derive(Debug, Parser)]
#[command(name = "oc-stats")]
#[command(version, about)]
struct CliArgs {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(
        long = "db",
        value_name = "PATH",
        global = true,
        help = "Path to OpenCode SQLite database file"
    )]
    database_path: Option<PathBuf>,

    #[arg(long = "theme", help = "Theme to use for the application")]
    theme: Option<ThemeMode>,

    #[arg(
        long = "ignore-zero",
        global = true,
        help = "Treat zero stored costs as missing and estimate them"
    )]
    ignore_zero: bool,
}

#[derive(Debug, Subcommand)]
enum Command {
    Cache {
        #[command(subcommand)]
        action: CacheCommand,
    },
    #[command(about = "Generate shell completions for oc-stats")]
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
    #[command(
        about = "Print usage statistics for a range as one line",
        long_about = "Print usage statistics for a range as a single machine-readable line \
            with no colour or terminal codes, so the output stays pipeable.\n\n\
            Shape: range=<name> tokens=<n> input=<n> output=<n> cache=<n> \
            cost=<usd> sessions=<n> messages=<n> prompts=<n> models=<n> active_days=<n>\n\n\
            <name> is one of all|today|7d|30d. Token counts and cost are the \
            exact numbers the overview reports; cost is the known USD total \
            as a plain decimal (0 when nothing is priced) and models is the \
            number of distinct models used."
    )]
    Stats {
        #[arg(
            long = "range",
            default_value = "all",
            value_parser = parse_stats_range,
            help = "Time range to report: all|today|7d|30d"
        )]
        range: TimeRange,
        #[arg(long = "format", value_enum, default_value_t = StatsFormat::Line)]
        format: StatsFormat,
    },
}

fn parse_stats_range(value: &str) -> std::result::Result<TimeRange, String> {
    TimeRange::from_cli_name(value)
        .ok_or_else(|| format!("invalid range '{value}': expected one of all|today|7d|30d"))
}

#[derive(Debug, Subcommand)]
#[command(about = "Manage the local cache of model pricing data")]
enum CacheCommand {
    #[command(about = "Show the path to the local pricing cache file")]
    Path,
    #[command(about = "Update the local pricing cache")]
    Update,
    #[command(about = "Clean the local pricing cache")]
    Clean,
}

async fn run_command(
    command: Command,
    database_path: Option<PathBuf>,
    ignore_zero: bool,
) -> Result<()> {
    match command {
        Command::Stats { range, format } => {
            let data = load_app_data(&InputOptions { database_path })
                .wrap_err("failed to load OpenCode usage data")?;
            let pricing = PricingCatalog::load().wrap_err("failed to load pricing catalog")?;
            let snapshot = build_snapshot(
                &data,
                &pricing,
                range,
                ZeroCostBehavior::from_ignore_zero(ignore_zero),
            );
            match format {
                StatsFormat::Line => println!("{}", format_stats_line(&snapshot, range)),
                StatsFormat::Json => {
                    println!("{}", serde_json::to_string(&stats_json(&snapshot, range))?)
                }
                StatsFormat::Csv => print!("{}", stats_csv(&snapshot, range)),
            }
            Ok(())
        }
        Command::Cache { action } => match action {
            CacheCommand::Path => {
                println!("{}", default_cache_path()?.display());
                Ok(())
            }
            CacheCommand::Update => {
                println!("Updating pricing cache...");
                let path = default_cache_path()?;
                let current = PricingCatalog::load().ok();
                let message = finalize_cache_update(
                    &path,
                    current.as_ref(),
                    refresh_pricing_catalog(path.clone())
                        .await
                        .map_err(color_eyre::eyre::Error::from),
                )?;
                println!("{message}");
                Ok(())
            }
            CacheCommand::Clean => {
                let path = default_cache_path()?;
                if path.exists() {
                    std::fs::remove_file(&path)
                        .wrap_err_with(|| format!("failed to remove {}", path.display()))?;
                }
                println!("Cleaned {}", path.display());
                Ok(())
            }
        },
        Command::Completions { shell } => {
            let mut cmd = CliArgs::command();
            let name = cmd.get_name().to_string();
            clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
            Ok(())
        }
    }
}

fn finalize_cache_update(
    path: &std::path::Path,
    current: Option<&PricingCatalog>,
    result: Result<PricingCatalog>,
) -> Result<String> {
    match result {
        Ok(_) => Ok(format!("Updated {}", path.display())),
        Err(err) => {
            let fallback_hint = current
                .map(PricingCatalog::refresh_failure_hint)
                .unwrap_or("current pricing fallback status is unknown");
            Err(err.wrap_err(format!(
                "failed to update {}; {fallback_hint}",
                path.display()
            )))
        }
    }
}

fn resolve_theme(cli_theme: Option<ThemeMode>) -> Result<(ThemeKind, Theme)> {
    let app_config = AppConfig::load().wrap_err("failed to load config.toml")?;
    let catalog = ThemeCatalog::load().wrap_err("failed to load theme catalog")?;

    let mode = cli_theme.unwrap_or(app_config.theme.default);
    let kind = mode.resolve();
    let selected_name = match kind {
        ThemeKind::Dark => app_config.theme.dark.as_str(),
        ThemeKind::Light => app_config.theme.light.as_str(),
    };

    let selected = catalog.get(selected_name).wrap_err_with(|| {
        format!(
            "theme '{selected_name}' not found; available themes: {}",
            catalog.names().join(", ")
        )
    })?;

    if selected.kind != kind {
        bail!(
            "theme '{selected_name}' has type {:?}, expected {:?}",
            selected.kind,
            kind
        );
    }

    Ok((kind, selected.theme.clone()))
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre::{Result, eyre};

    use super::finalize_cache_update;
    use crate::cache::models_cache::{PricingAvailability, PricingCatalog};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    fn test_catalog(availability: PricingAvailability) -> PricingCatalog {
        PricingCatalog {
            models: BTreeMap::new(),
            cache_path: PathBuf::from("/tmp/models.json"),
            refresh_needed: false,
            availability,
            load_notice: None,
        }
    }

    #[test]
    fn cache_update_success_keeps_success_message() {
        let path = Path::new("/tmp/models.json");
        let result = finalize_cache_update(
            path,
            None,
            Ok::<PricingCatalog, _>(test_catalog(PricingAvailability::Cached)),
        )
        .unwrap();

        assert_eq!(result, "Updated /tmp/models.json");
    }

    #[test]
    fn cache_update_failure_returns_error_with_fallback_hint() {
        let path = Path::new("/tmp/models.json");
        let err = finalize_cache_update(
            path,
            Some(&test_catalog(PricingAvailability::OverridesOnly)),
            Err(eyre!("network down")),
        )
        .unwrap_err();

        let message = format!("{err:#}");
        assert!(message.contains("failed to update /tmp/models.json"));
        assert!(message.contains("using local pricing overrides only"));
    }

    #[test]
    fn cache_update_failure_without_catalog_still_returns_error() {
        let path = Path::new("/tmp/models.json");
        let result: Result<PricingCatalog> = Err(eyre!("network down"));
        let err = finalize_cache_update(path, None, result).unwrap_err();

        assert!(format!("{err:#}").contains("current pricing fallback status is unknown"));
    }
}
