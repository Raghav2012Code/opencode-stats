use crate::analytics::AnalyticsSnapshot;
use crate::utils::time::TimeRange;

/// Output format for the `stats` subcommand.
#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
pub enum StatsFormat {
    /// One `key=value` line (see [`format_stats_line`]).
    #[default]
    #[value(name = "line")]
    Line,
}

/// One stable `key=value` line for the overview of `snapshot`:
///
/// `range=<name> tokens=<n> input=<n> output=<n> cache=<n> cost=<usd>
/// sessions=<n> messages=<n> prompts=<n> models=<n> active_days=<n>`
///
/// `<name>` is the `--range` name (`all|today|7d|30d`). Token counts and
/// `cost` are the exact numbers the overview reports; `cost` is the known
/// USD total as a plain decimal (`0` when nothing is priced). Models is the
/// number of distinct models used. The output is a single line with no
/// colour or terminal codes, so it stays pipeable.
pub fn format_stats_line(snapshot: &AnalyticsSnapshot, range: TimeRange) -> String {
    let overview = &snapshot.overview;
    format!(
        "range={} tokens={} input={} output={} cache={} cost={} sessions={} messages={} prompts={} models={} active_days={}",
        range.cli_name(),
        overview.total_tokens,
        overview.input_tokens,
        overview.output_tokens,
        overview.cache_tokens,
        overview.total_cost.known,
        overview.sessions,
        overview.messages,
        overview.prompts,
        overview.models_used,
        overview.active_days,
    )
}

#[cfg(test)]
mod tests {
    use super::{StatsFormat, format_stats_line};
    use crate::analytics::heatmap_data::HeatmapData;
    use crate::analytics::model_stats::ModelChartData;
    use crate::analytics::{AnalyticsSnapshot, OverviewStats};
    use crate::utils::pricing::PriceSummary;
    use crate::utils::time::TimeRange;

    fn synthetic_snapshot() -> AnalyticsSnapshot {
        AnalyticsSnapshot {
            overview: OverviewStats {
                total_tokens: 130,
                input_tokens: 100,
                output_tokens: 20,
                cache_tokens: 10,
                total_cost: PriceSummary {
                    known: rust_decimal::Decimal::new(123, 2),
                    has_known: true,
                    missing: false,
                },
                sessions: 2,
                messages: 5,
                prompts: 2,
                models_used: 1,
                active_days: 1,
                fun_comparison: String::new(),
            },
            models: Vec::new(),
            chart: ModelChartData {
                x_bounds: [0.0, 1.0],
                y_bounds: [0.0, 1.0],
                x_labels: Vec::new(),
                y_labels: Vec::new(),
                series: Vec::new(),
            },
            providers: Vec::new(),
            provider_chart: ModelChartData {
                x_bounds: [0.0, 1.0],
                y_bounds: [0.0, 1.0],
                x_labels: Vec::new(),
                y_labels: Vec::new(),
                series: Vec::new(),
            },
            heatmap: HeatmapData {
                weeks: Vec::new(),
                month_labels: Vec::new(),
            },
        }
    }

    #[test]
    fn line_reports_hand_computed_numbers() {
        let snapshot = synthetic_snapshot();

        assert_eq!(
            format_stats_line(&snapshot, TimeRange::Today),
            "range=today tokens=130 input=100 output=20 cache=10 cost=1.23 \
             sessions=2 messages=5 prompts=2 models=1 active_days=1",
        );
    }

    #[test]
    fn line_matches_the_overview_it_reports() {
        let snapshot = synthetic_snapshot();
        let line = format_stats_line(&snapshot, TimeRange::All);
        let pairs = line
            .split(' ')
            .map(|pair| {
                let (key, value) = pair.split_once('=').expect("every pair has a key");
                (key, value)
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let overview = &snapshot.overview;

        assert_eq!(line.lines().count(), 1, "output is a single line");
        assert_eq!(pairs["range"], TimeRange::All.cli_name());
        assert_eq!(pairs["tokens"], overview.total_tokens.to_string());
        assert_eq!(pairs["input"], overview.input_tokens.to_string());
        assert_eq!(pairs["output"], overview.output_tokens.to_string());
        assert_eq!(pairs["cache"], overview.cache_tokens.to_string());
        assert_eq!(pairs["cost"], overview.total_cost.known.to_string());
        assert_eq!(pairs["sessions"], overview.sessions.to_string());
        assert_eq!(pairs["messages"], overview.messages.to_string());
        assert_eq!(pairs["prompts"], overview.prompts.to_string());
        assert_eq!(pairs["models"], overview.models_used.to_string());
        assert_eq!(pairs["active_days"], overview.active_days.to_string());
    }

    #[test]
    fn default_format_is_line() {
        assert!(matches!(StatsFormat::default(), StatsFormat::Line));
    }
}
