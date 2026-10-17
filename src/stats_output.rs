use crate::analytics::AnalyticsSnapshot;
use crate::analytics::model_stats::UsageRow;
use crate::utils::pricing::PriceSummary;
use crate::utils::time::TimeRange;

/// Output format for the `stats` subcommand.
#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
pub enum StatsFormat {
    /// One `key=value` line (see [`format_stats_line`]).
    #[default]
    #[value(name = "line")]
    Line,
    /// One JSON object (see [`stats_json`]).
    #[value(name = "json")]
    Json,
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

/// The same view as [`format_stats_line`] as one JSON object: the `range`
/// name plus the `overview` totals, the `models` and `providers` rows and
/// the `heatmap` window. Costs keep full precision: `known` serializes as
/// an exact decimal string, so a serialize→parse round-trip preserves the
/// value bit-for-bit. Like the line format this carries no colour or
/// terminal codes.
pub fn stats_json(snapshot: &AnalyticsSnapshot, range: TimeRange) -> serde_json::Value {
    let overview = &snapshot.overview;
    serde_json::json!({
        "range": range.cli_name(),
        "overview": {
            "tokens": overview.total_tokens,
            "input": overview.input_tokens,
            "output": overview.output_tokens,
            "cache": overview.cache_tokens,
            "cost": price_json(&overview.total_cost),
            "sessions": overview.sessions,
            "messages": overview.messages,
            "prompts": overview.prompts,
            "models": overview.models_used,
            "active_days": overview.active_days,
        },
        "models": snapshot.models.iter().map(usage_row_json).collect::<Vec<_>>(),
        "providers": snapshot.providers.iter().map(usage_row_json).collect::<Vec<_>>(),
        "heatmap": {
            "weeks": snapshot.heatmap.weeks.iter().map(|week| {
                week.iter().map(|cell| {
                    serde_json::json!({
                        "intensity": cell.intensity,
                        "is_future": cell.is_future,
                    })
                }).collect::<Vec<_>>()
            }).collect::<Vec<_>>(),
            "month_labels": snapshot.heatmap.month_labels.iter().map(|(week, label)| {
                serde_json::json!({ "week": week, "label": label })
            }).collect::<Vec<_>>(),
        },
    })
}

fn price_json(summary: &PriceSummary) -> serde_json::Value {
    serde_json::json!({
        "known": summary.known,
        "has_known": summary.has_known,
        "missing": summary.missing,
    })
}

fn usage_row_json(row: &UsageRow) -> serde_json::Value {
    serde_json::json!({
        "id": row.id,
        "tokens": row.total_tokens,
        "input": row.input_tokens,
        "output": row.output_tokens,
        "cache": row.cache_tokens,
        "cost": price_json(&row.cost),
        "sessions": row.sessions,
        "messages": row.messages,
        "prompts": row.prompts,
        "active_days": row.active_days,
        "percentage": row.percentage,
    })
}

#[cfg(test)]
mod tests {
    use super::{StatsFormat, format_stats_line, stats_json};
    use crate::analytics::heatmap_data::{HeatmapCell, HeatmapData};
    use crate::analytics::model_stats::{ModelChartData, UsageRow};
    use crate::analytics::{AnalyticsSnapshot, OverviewStats};
    use crate::utils::pricing::PriceSummary;
    use crate::utils::time::TimeRange;

    fn price(known: rust_decimal::Decimal) -> PriceSummary {
        PriceSummary {
            known,
            has_known: true,
            missing: false,
        }
    }

    fn row(id: &str, tokens: u64, known: rust_decimal::Decimal) -> UsageRow {
        UsageRow {
            id: id.to_string(),
            total_tokens: tokens,
            input_tokens: 100,
            output_tokens: 20,
            cache_tokens: 10,
            messages: 4,
            prompts: 1,
            sessions: 2,
            active_days: 1,
            cost: price(known),
            percentage: 75.0,
            p50_output_tokens_per_second: 0.0,
        }
    }

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
            models: vec![row(
                "acme \"quoted\" model",
                130,
                rust_decimal::Decimal::new(123, 2),
            )],
            chart: ModelChartData {
                x_bounds: [0.0, 1.0],
                y_bounds: [0.0, 1.0],
                x_labels: Vec::new(),
                y_labels: Vec::new(),
                series: Vec::new(),
            },
            providers: vec![row(
                "acme \"quoted\" provider",
                130,
                rust_decimal::Decimal::new(123, 2),
            )],
            provider_chart: ModelChartData {
                x_bounds: [0.0, 1.0],
                y_bounds: [0.0, 1.0],
                x_labels: Vec::new(),
                y_labels: Vec::new(),
                series: Vec::new(),
            },
            heatmap: HeatmapData {
                weeks: vec![vec![
                    HeatmapCell {
                        intensity: 0,
                        is_future: false,
                    },
                    HeatmapCell {
                        intensity: 3,
                        is_future: true,
                    },
                ]],
                month_labels: vec![(0, "Oct".to_string())],
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

    #[test]
    fn json_reports_the_full_view() {
        let snapshot = synthetic_snapshot();
        let value = stats_json(&snapshot, TimeRange::Today);

        assert_eq!(value["range"], "today");
        assert_eq!(value["overview"]["tokens"], 130);
        assert_eq!(value["overview"]["sessions"], 2);
        assert_eq!(value["models"].as_array().unwrap().len(), 1);
        assert_eq!(value["models"][0]["id"], "acme \"quoted\" model");
        assert_eq!(value["providers"].as_array().unwrap().len(), 1);
        assert_eq!(value["providers"][0]["id"], "acme \"quoted\" provider");
        assert_eq!(value["heatmap"]["weeks"].as_array().unwrap().len(), 1);
        assert_eq!(value["heatmap"]["weeks"][0].as_array().unwrap().len(), 2);
        assert_eq!(value["heatmap"]["weeks"][0][1]["intensity"], 3);
        assert_eq!(value["heatmap"]["weeks"][0][1]["is_future"], true);
        assert_eq!(value["heatmap"]["month_labels"][0]["label"], "Oct");
    }

    #[test]
    fn costs_survive_a_serialize_parse_round_trip() {
        use std::str::FromStr;
        let exact = rust_decimal::Decimal::from_str("37.262297028000000073595").unwrap();
        let mut snapshot = synthetic_snapshot();
        snapshot.overview.total_cost = price(exact);
        snapshot.models = vec![row("acme \"quoted\" model", 130, exact)];

        let text = serde_json::to_string(&stats_json(&snapshot, TimeRange::All)).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();

        for pointer in ["/overview/cost/known", "/models/0/cost/known"] {
            let known = parsed.pointer(pointer).unwrap().as_str().unwrap();
            assert_eq!(rust_decimal::Decimal::from_str(known).unwrap(), exact);
        }
        assert_eq!(parsed["models"][0]["id"], "acme \"quoted\" model");
    }

    #[test]
    fn json_agrees_with_the_line_format() {
        let snapshot = synthetic_snapshot();
        let line = format_stats_line(&snapshot, TimeRange::Last7Days);
        let pairs = line
            .split(' ')
            .map(|pair| {
                let (key, value) = pair.split_once('=').expect("every pair has a key");
                (key, value)
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let value = stats_json(&snapshot, TimeRange::Last7Days);

        assert_eq!(value["range"].as_str().unwrap(), pairs["range"]);
        let overview = value["overview"].as_object().unwrap();
        for (line_key, json_key) in [
            ("tokens", "tokens"),
            ("input", "input"),
            ("output", "output"),
            ("cache", "cache"),
            ("sessions", "sessions"),
            ("messages", "messages"),
            ("prompts", "prompts"),
            ("models", "models"),
            ("active_days", "active_days"),
        ] {
            assert_eq!(
                overview[json_key].as_u64().unwrap().to_string(),
                pairs[line_key]
            );
        }
        assert_eq!(overview["cost"]["known"].as_str().unwrap(), pairs["cost"]);
    }
}
