pub mod heatmap_data;
pub mod model_stats;
pub mod streaks;
pub mod view;

use std::collections::BTreeSet;

use chrono::NaiveDate;

use rust_decimal::Decimal;

use crate::analytics::heatmap_data::{HeatmapData, build_heatmap_data};
use crate::analytics::model_stats::{
    ModelChartData, SessionUsageRow, UsageRow, build_model_chart, build_provider_chart,
    build_session_rows,
};
use crate::analytics::streaks::streaks;
use crate::analytics::view::View;
use crate::cache::models_cache::PricingCatalog;
use crate::db::models::{AppData, MessageRecord, UsageEvent};
use crate::utils::pricing::{PriceSummary, ZeroCostBehavior, update_price_summary};
use crate::utils::time::{TimeRange, current_local_date};

#[derive(Clone, Debug)]
pub struct OverviewStats {
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_tokens: u64,
    pub total_cost: PriceSummary,
    pub sessions: usize,
    pub messages: usize,
    pub prompts: usize,
    pub models_used: usize,
    pub active_days: usize,
    /// Days from the first use (or the range start, whichever is later) through today,
    /// inclusive; `0` when there is no usage. Active days read as a share of this.
    pub active_days_span: usize,
    pub current_streak: usize,
    pub longest_streak: usize,
    pub fun_comparison: String,
}

#[derive(Clone, Debug)]
pub struct AnalyticsSnapshot {
    pub overview: OverviewStats,
    pub models: Vec<UsageRow>,
    pub chart: ModelChartData,
    pub providers: Vec<UsageRow>,
    pub provider_chart: ModelChartData,
    pub sessions: Vec<SessionUsageRow>,
    pub heatmap: HeatmapData,
}

/// The snapshot with nothing filtered out, for tests and for callers with no view.
#[cfg(test)]
pub fn build_snapshot(
    data: &AppData,
    pricing: &PricingCatalog,
    range: TimeRange,
    zero_cost_behavior: ZeroCostBehavior,
) -> AnalyticsSnapshot {
    build_snapshot_view(data, pricing, range, zero_cost_behavior, View::default())
}

/// The snapshot as the user asked to see it: the view is applied before anything is
/// aggregated, so every figure describes the rows that survive.
pub fn build_snapshot_view(
    data: &AppData,
    pricing: &PricingCatalog,
    range: TimeRange,
    zero_cost_behavior: ZeroCostBehavior,
    view: View,
) -> AnalyticsSnapshot {
    let today = current_local_date();
    let mut filtered_events = filter_events(&data.events, range, today);
    let mut filtered_messages = filter_messages(&data.messages, range, today);
    // Sessions belong to the project of their events, so the project filter keeps
    // whole sessions: their events and their messages alike.
    let project_sessions: Option<BTreeSet<String>> = view
        .project
        .as_deref()
        .map(|project| sessions_in_project(&data.events, project));
    if let Some(allowed) = &project_sessions {
        filtered_events.retain(|event| allowed.contains(event.session_id.as_str()));
        filtered_messages.retain(|message| allowed.contains(message.session_id.as_str()));
    }
    let (models, chart) = build_model_chart(
        &filtered_events,
        &filtered_messages,
        pricing,
        range,
        today,
        zero_cost_behavior,
    );
    let (providers, provider_chart) = build_provider_chart(
        &filtered_events,
        &filtered_messages,
        pricing,
        range,
        today,
        zero_cost_behavior,
    );
    let heatmap = build_heatmap_data(&filtered_events, range, today);

    let total_tokens = saturating_sum(filtered_events.iter().map(|event| event.tokens.total()));
    let input_tokens = saturating_sum(filtered_events.iter().map(|event| event.tokens.input));
    let output_tokens = saturating_sum(filtered_events.iter().map(|event| event.tokens.output));
    let cache_tokens = saturating_sum(filtered_events.iter().map(|event| {
        event
            .tokens
            .cache_read
            .saturating_add(event.tokens.cache_write)
    }));
    let mut total_cost = PriceSummary::default();
    for event in &filtered_events {
        update_price_summary(&mut total_cost, pricing, event, zero_cost_behavior);
    }
    let mut session_ids =
        filter_session_ids(data, &filtered_events, &filtered_messages, range, today);
    // The records table knows no projects, so a session it names can still belong to a
    // project the filter hides: intersect, or the counts would describe other sessions.
    if let Some(allowed) = &project_sessions {
        session_ids.retain(|id| allowed.contains(id));
    }
    // The same messages the list pages count: dated in the range and inside the view, so
    // the overview's totals equal the sums over the model, provider and session rows. A
    // turn with no model or provider attached counts toward its session's dominant row;
    // only turns in sessions without usage stay overview- and sessions-only. Compaction
    // digests are usage, not conversation, and are not counted here.
    let messages = filtered_messages
        .iter()
        .filter(|message| matches!(message.role.as_deref(), Some("assistant") | Some("user")))
        .count();
    let prompts = filtered_messages
        .iter()
        .filter(|message| message.role.as_deref() == Some("user"))
        .count();
    let sessions = session_ids.len();
    let models_used = filtered_events
        .iter()
        .map(|event| event.model_id.clone())
        .collect::<BTreeSet<_>>()
        .len();
    let active_day_set = filtered_events
        .iter()
        .filter_map(|event| event.activity_date())
        .collect::<BTreeSet<_>>();
    let active_days = active_day_set.len();
    // The denominator of the active-days figure: every day since the first use in this
    // view, up to today, and no more than the selected range covers, so a 7d view reads
    // "5/7" rather than "5/168".
    let first_use = data
        .events
        .iter()
        .filter(|event| {
            project_sessions
                .as_ref()
                .is_none_or(|allowed| allowed.contains(event.session_id.as_str()))
        })
        .filter_map(|event| event.activity_date())
        .min();
    let active_days_span = match (first_use, range.start_date(today)) {
        (Some(first), Some(start)) => Some(first.max(start)),
        (Some(first), None) => Some(first),
        (None, _) => None,
    }
    .map(|start| (today - start).num_days().max(0) as usize + 1)
    .unwrap_or(0);
    let overall_streaks = streaks(&active_day_set, today);

    let mut snapshot = AnalyticsSnapshot {
        overview: OverviewStats {
            total_tokens,
            input_tokens,
            output_tokens,
            cache_tokens,
            total_cost,
            sessions,
            messages,
            prompts,
            models_used,
            active_days,
            active_days_span,
            current_streak: overall_streaks.current,
            longest_streak: overall_streaks.longest,
            fun_comparison: crate::utils::formatting::tokens_comparison_text(
                input_tokens + output_tokens,
            ),
        },
        models,
        chart,
        providers,
        provider_chart,
        sessions: build_session_rows(
            &filtered_events,
            &filtered_messages,
            pricing,
            zero_cost_behavior,
            today,
        ),
        heatmap,
    };

    apply_pins(&mut snapshot, &view);
    snapshot
}

/// Puts pinned rows first, in the configured order, leaving everything else stable.
/// Unknown names match nothing and are ignored. The chart series travel with their
/// rows and take the new position's colour, so the list and the chart keep agreeing;
/// the Overview's favourite model is the first row, so it follows the pins too.
fn apply_pins(snapshot: &mut AnalyticsSnapshot, view: &View) {
    order_pinned(&mut snapshot.models, &view.pinned_models, |row| {
        row.id.as_str()
    });
    order_pinned(&mut snapshot.providers, &view.pinned_providers, |row| {
        row.id.as_str()
    });
    reorder_chart(&mut snapshot.chart, &snapshot.models);
    reorder_chart(&mut snapshot.provider_chart, &snapshot.providers);
}

fn order_pinned<T>(rows: &mut [T], pinned: &[String], key: impl Fn(&T) -> &str) {
    if pinned.is_empty() {
        return;
    }
    let position: std::collections::HashMap<&str, usize> = pinned
        .iter()
        .enumerate()
        .map(|(index, name)| (name.as_str(), index))
        .collect();
    // `sort_by_key` is stable: pinned rows come first in the given order, the rest
    // keep the order they had.
    rows.sort_by_key(|row| position.get(key(row)).copied().unwrap_or(usize::MAX));
}

fn reorder_chart(chart: &mut ModelChartData, rows: &[UsageRow]) {
    let position: std::collections::HashMap<&str, usize> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| (row.id.as_str(), index))
        .collect();
    chart.series.sort_by_key(|series| {
        position
            .get(series.model_id.as_str())
            .copied()
            .unwrap_or(usize::MAX)
    });
    for (index, series) in chart.series.iter_mut().enumerate() {
        series.palette_index = index;
    }
}

fn saturating_sum(values: impl IntoIterator<Item = u64>) -> u64 {
    values
        .into_iter()
        .fold(0u64, |total, value| total.saturating_add(value))
}

/// What the user has spent in the budget's month, and what the cap is.
#[derive(Clone, Debug)]
pub struct BudgetState {
    pub cap: Decimal,
    pub spend: PriceSummary,
}

impl BudgetState {
    pub fn remainder(&self) -> Decimal {
        self.cap - self.spend.known
    }

    pub fn is_over(&self) -> bool {
        self.spend.known > self.cap
    }
}

/// The window the budget is measured over: the same one the `30d` range shows, so the
/// budget and the list pages agree about what the last month means.
pub const BUDGET_WINDOW: TimeRange = TimeRange::Last30Days;

/// The sessions whose project matches, resolved the same way session rows resolve it:
/// the worktree's file name first, then the stored project name, compared without
/// regard to case. A session with no project on any event never matches.
fn sessions_in_project(events: &[UsageEvent], project: &str) -> BTreeSet<String> {
    events
        .iter()
        .filter(|event| {
            session_project(event).is_some_and(|name| name.eq_ignore_ascii_case(project))
        })
        .map(|event| event.session_id.clone())
        .collect()
}

/// What a session row would call this event's project.
fn session_project(event: &UsageEvent) -> Option<String> {
    event
        .project_path
        .as_deref()
        .and_then(std::path::Path::file_name)
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .or_else(|| event.project_name.clone())
}

fn filter_events(events: &[UsageEvent], range: TimeRange, today: NaiveDate) -> Vec<&UsageEvent> {
    events
        .iter()
        .filter(|event| {
            event
                .activity_date()
                .is_some_and(|date| crate::utils::time::in_range(date, range, today))
        })
        .collect()
}

fn filter_messages(
    messages: &[MessageRecord],
    range: TimeRange,
    today: NaiveDate,
) -> Vec<&MessageRecord> {
    messages
        .iter()
        .filter(|message| {
            message
                .activity_date()
                .is_some_and(|date| crate::utils::time::in_range(date, range, today))
        })
        .collect()
}

fn filter_session_ids(
    data: &AppData,
    filtered_events: &[&UsageEvent],
    filtered_messages: &[&MessageRecord],
    range: TimeRange,
    today: NaiveDate,
) -> BTreeSet<String> {
    let session_ids = data
        .session_records
        .iter()
        .filter(|session| {
            crate::utils::time::in_range(session.updated_at.date_naive(), range, today)
        })
        .map(|session| session.session_id.clone())
        .collect::<BTreeSet<_>>();

    if !session_ids.is_empty() {
        return session_ids;
    }

    filtered_events
        .iter()
        .map(|event| event.session_id.clone())
        .chain(
            filtered_messages
                .iter()
                .map(|message| message.session_id.clone()),
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::build_snapshot;
    use crate::cache::models_cache::{PricingAvailability, PricingCatalog};
    use crate::db::models::{
        AppData, ImportStats, MessageRecord, SessionRecord, TokenUsage, UsageEvent,
    };
    use crate::utils::pricing::ZeroCostBehavior;
    use crate::utils::time::{TimeRange, current_local_date};
    use chrono::{Local, TimeZone};
    use rust_decimal::Decimal;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn event_with(model: &str, provider: &str, cost: Option<Decimal>, input: u64) -> UsageEvent {
        let when = Local
            .with_ymd_and_hms(2026, 3, 12, 9, 30, 0)
            .single()
            .unwrap();

        UsageEvent {
            session_id: format!("ses_{model}"),
            parent_session_id: None,
            session_title: None,
            session_started_at: Some(when),
            session_archived_at: None,
            project_name: None,
            project_path: None,
            provider_id: Some(provider.to_string()),
            model_id: model.to_string(),
            agent: None,
            finish_reason: None,
            tokens: TokenUsage {
                input,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            },
            created_at: Some(when),
            completed_at: Some(when),
            stored_cost_usd: cost,
        }
    }

    fn empty_catalog() -> PricingCatalog {
        PricingCatalog {
            models: BTreeMap::new(),
            cache_path: PathBuf::from("/tmp/models.json"),
            refresh_needed: false,
            availability: PricingAvailability::Empty,
            load_notice: None,
        }
    }

    fn two_models_and_an_unpriced_one() -> AppData {
        AppData {
            events: vec![
                event_with(
                    "free-model",
                    "free-provider",
                    Some(Decimal::ZERO),
                    1_000_000,
                ),
                event_with("paid-model", "paid-provider", Some(Decimal::new(2, 0)), 100),
                event_with("unpriced-model", "paid-provider", None, 50),
            ],
            messages: Vec::new(),
            session_records: Vec::new(),
            import_stats: ImportStats::default(),
        }
    }

    #[test]
    fn a_free_model_is_a_row_and_its_tokens_count() {
        // A model that cost nothing is still usage: it stays a row, and its tokens,
        // sessions and days count towards the totals. Nothing filters it out.
        let data = two_models_and_an_unpriced_one();
        let catalog = empty_catalog();

        let snapshot =
            super::build_snapshot(&data, &catalog, TimeRange::All, ZeroCostBehavior::KeepZero);

        let models: Vec<&str> = snapshot.models.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(
            models,
            vec!["free-model", "paid-model", "unpriced-model"],
            "the free model is a row like any other"
        );
        assert_eq!(snapshot.overview.total_tokens, 1_000_150);
        assert_eq!(snapshot.overview.models_used, 3);
        assert_eq!(snapshot.overview.sessions, 3);
        assert_eq!(snapshot.overview.total_cost.known, Decimal::new(2, 0));
        assert!(
            snapshot
                .providers
                .iter()
                .any(|row| row.id == "free-provider"),
            "the provider that served the free model is there too"
        );
    }

    #[test]
    fn a_model_with_no_price_is_kept_and_marked_as_missing() {
        let data = AppData {
            events: vec![event_with("unpriced-model", "acme", None, 10)],
            messages: Vec::new(),
            session_records: Vec::new(),
            import_stats: ImportStats::default(),
        };

        let snapshot = super::build_snapshot(
            &data,
            &empty_catalog(),
            TimeRange::All,
            ZeroCostBehavior::KeepZero,
        );

        assert_eq!(
            snapshot.models.len(),
            1,
            "unknown is not zero: the row is kept and marked as missing"
        );
        assert!(snapshot.overview.total_cost.missing);
    }

    #[test]
    fn the_overview_and_the_rows_agree_on_messages_and_prompts() {
        // User and assistant turns are messages; a user turn is also a prompt. A compaction
        // digest is usage, so it is in neither count. The overview and both kinds of row
        // must give the same totals.
        let when = Local
            .with_ymd_and_hms(2026, 3, 12, 9, 30, 0)
            .single()
            .unwrap();
        let turn = |role: &str| MessageRecord {
            session_id: "ses_gpt-5".to_string(),
            role: Some(role.to_string()),
            provider_id: Some("openai".to_string()),
            model_id: Some("gpt-5".to_string()),
            created_at: Some(when),
        };
        let data = AppData {
            events: vec![event_with("gpt-5", "openai", Some(Decimal::ONE), 10)],
            messages: vec![
                turn("user"),
                turn("assistant"),
                turn("compaction"),
                turn("user"),
                // A modern user turn carries no model or provider: it still counts,
                // toward its session's dominant row.
                MessageRecord {
                    session_id: "ses_gpt-5".to_string(),
                    role: Some("user".to_string()),
                    provider_id: None,
                    model_id: None,
                    created_at: Some(when),
                },
            ],
            session_records: Vec::new(),
            import_stats: ImportStats::default(),
        };

        let snapshot = build_snapshot(
            &data,
            &empty_catalog(),
            TimeRange::All,
            ZeroCostBehavior::KeepZero,
        );

        assert_eq!(snapshot.overview.messages, 4);
        assert_eq!(snapshot.overview.prompts, 3);
        assert_eq!(snapshot.models[0].messages, 4);
        assert_eq!(snapshot.models[0].prompts, 3);
        assert_eq!(snapshot.providers[0].messages, 4);
        assert_eq!(snapshot.providers[0].prompts, 3);
        assert_eq!(snapshot.sessions[0].messages, 4);
        assert_eq!(snapshot.sessions[0].prompts, 3);
    }

    #[test]
    fn active_days_span_from_first_use_and_stop_at_the_range() {
        // Active days read as a share of the days available: the whole span since first
        // use for "all", and the selected range when that is shorter.
        let today = current_local_date();
        let event_on = |days_ago: i64| {
            let when = (today - chrono::Duration::days(days_ago))
                .and_hms_opt(9, 0, 0)
                .unwrap()
                .and_local_timezone(Local)
                .unwrap();
            UsageEvent {
                session_id: "ses_span".to_string(),
                parent_session_id: None,
                session_title: None,
                session_started_at: Some(when),
                session_archived_at: None,
                project_name: None,
                project_path: None,
                provider_id: Some("openai".to_string()),
                model_id: "gpt-5".to_string(),
                agent: None,
                finish_reason: None,
                tokens: TokenUsage {
                    input: 1,
                    output: 1,
                    cache_read: 0,
                    cache_write: 0,
                },
                created_at: Some(when),
                completed_at: Some(when),
                stored_cost_usd: None,
            }
        };
        let data = AppData {
            events: vec![event_on(9), event_on(0)],
            messages: Vec::new(),
            session_records: Vec::new(),
            import_stats: ImportStats::default(),
        };

        let all = build_snapshot(
            &data,
            &empty_catalog(),
            TimeRange::All,
            ZeroCostBehavior::KeepZero,
        );
        assert_eq!(all.overview.active_days, 2);
        assert_eq!(all.overview.active_days_span, 10);

        let week = build_snapshot(
            &data,
            &empty_catalog(),
            TimeRange::Last7Days,
            ZeroCostBehavior::KeepZero,
        );
        assert_eq!(week.overview.active_days, 1);
        assert_eq!(week.overview.active_days_span, 7);

        let empty = build_snapshot(
            &AppData {
                events: Vec::new(),
                messages: Vec::new(),
                session_records: Vec::new(),
                import_stats: ImportStats::default(),
            },
            &empty_catalog(),
            TimeRange::All,
            ZeroCostBehavior::KeepZero,
        );
        assert_eq!(empty.overview.active_days_span, 0);
    }

    #[test]
    fn the_default_view_changes_nothing() {
        let data = two_models_and_an_unpriced_one();
        let catalog = empty_catalog();

        let without =
            super::build_snapshot(&data, &catalog, TimeRange::All, ZeroCostBehavior::KeepZero);
        let with_default = super::build_snapshot_view(
            &data,
            &catalog,
            TimeRange::All,
            ZeroCostBehavior::KeepZero,
            super::View::default(),
        );

        assert_eq!(without.models.len(), with_default.models.len());
        assert_eq!(
            without.overview.total_tokens,
            with_default.overview.total_tokens
        );
        assert_eq!(
            without.overview.total_cost.known,
            with_default.overview.total_cost.known
        );
    }

    fn event_in(project: Option<&str>, path: Option<&str>) -> UsageEvent {
        let now = Local::now();
        let mut event = event_with("m", "acme", Some(Decimal::ONE), 10);
        event.session_id = format!("ses_{}", project.or(path).unwrap_or("none"));
        event.created_at = Some(now);
        event.session_started_at = Some(now);
        event.project_name = project.map(str::to_string);
        event.project_path = path.map(PathBuf::from);
        event
    }

    fn project_data() -> AppData {
        AppData {
            events: vec![
                event_in(Some("Demo"), None),
                event_in(None, Some("/tmp/demo")),
                event_in(Some("Other"), None),
            ],
            messages: Vec::new(),
            session_records: Vec::new(),
            import_stats: ImportStats::default(),
        }
    }

    fn project_view(name: &str) -> super::View {
        super::View {
            project: Some(name.to_string()),
            pinned_models: Vec::new(),
            pinned_providers: Vec::new(),
        }
    }

    #[test]
    fn the_project_filter_keeps_only_that_projects_rows() {
        let data = project_data();

        let filtered = super::build_snapshot_view(
            &data,
            &empty_catalog(),
            TimeRange::All,
            ZeroCostBehavior::KeepZero,
            project_view("demo"),
        );

        // "Demo" by name and "/tmp/demo" by worktree basename, case-insensitively.
        assert_eq!(filtered.overview.sessions, 2, "both demo sessions");
        assert_eq!(filtered.overview.total_tokens, 20);
        assert_eq!(filtered.sessions.len(), 2);
        assert!(
            filtered.sessions.iter().all(|row| row.id != "ses_Other"),
            "the other project is gone"
        );
    }

    #[test]
    fn project_matching_is_case_insensitive_on_both_forms() {
        let data = project_data();

        for name in ["demo", "DEMO", "Demo", "dEmO"] {
            let filtered = super::build_snapshot_view(
                &data,
                &empty_catalog(),
                TimeRange::All,
                ZeroCostBehavior::KeepZero,
                project_view(name),
            );
            assert_eq!(filtered.overview.sessions, 2, "filter {name:?}");
        }
    }

    #[test]
    fn a_project_with_no_activity_leaves_everything_empty() {
        let data = project_data();

        let filtered = super::build_snapshot_view(
            &data,
            &empty_catalog(),
            TimeRange::All,
            ZeroCostBehavior::KeepZero,
            project_view("absent"),
        );

        assert_eq!(filtered.overview.sessions, 0);
        assert_eq!(filtered.overview.total_tokens, 0);
        assert!(filtered.models.is_empty());
        assert!(filtered.providers.is_empty());
        assert!(filtered.sessions.is_empty());
    }

    fn pinned_view(models: &[&str], providers: &[&str]) -> super::View {
        super::View {
            project: None,
            pinned_models: models.iter().map(|name| name.to_string()).collect(),
            pinned_providers: providers.iter().map(|name| name.to_string()).collect(),
        }
    }

    fn three_model_data() -> AppData {
        AppData {
            events: vec![
                event_with("aaa", "zulu", Some(Decimal::new(3, 0)), 300),
                event_with("bbb", "alpha", Some(Decimal::new(2, 0)), 200),
                event_with("ccc", "alpha", Some(Decimal::new(1, 0)), 100),
            ],
            messages: Vec::new(),
            session_records: Vec::new(),
            import_stats: ImportStats::default(),
        }
    }

    fn model_ids(snapshot: &super::AnalyticsSnapshot) -> Vec<&str> {
        snapshot.models.iter().map(|row| row.id.as_str()).collect()
    }

    #[test]
    fn pinned_models_come_first_in_the_configured_order() {
        let data = three_model_data();

        let pinned = super::build_snapshot_view(
            &data,
            &empty_catalog(),
            TimeRange::All,
            ZeroCostBehavior::KeepZero,
            pinned_view(&["ccc", "aaa"], &[]),
        );

        assert_eq!(model_ids(&pinned), vec!["ccc", "aaa", "bbb"]);
        assert_eq!(pinned.models[0].id, "ccc");
    }

    #[test]
    fn unpinned_rows_keep_their_previous_order() {
        let data = three_model_data();

        let plain = super::build_snapshot(
            &data,
            &empty_catalog(),
            TimeRange::All,
            ZeroCostBehavior::KeepZero,
        );
        let pinned = super::build_snapshot_view(
            &data,
            &empty_catalog(),
            TimeRange::All,
            ZeroCostBehavior::KeepZero,
            pinned_view(&["bbb"], &[]),
        );

        assert_eq!(model_ids(&plain), vec!["aaa", "bbb", "ccc"]);
        assert_eq!(model_ids(&pinned), vec!["bbb", "aaa", "ccc"]);
    }

    #[test]
    fn unknown_pins_are_ignored_without_failing() {
        let data = three_model_data();

        let pinned = super::build_snapshot_view(
            &data,
            &empty_catalog(),
            TimeRange::All,
            ZeroCostBehavior::KeepZero,
            pinned_view(&["nope", "aaa"], &["missing"]),
        );

        assert_eq!(model_ids(&pinned), vec!["aaa", "bbb", "ccc"]);
        let providers: Vec<&str> = pinned.providers.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(providers, vec!["alpha", "zulu"]);
    }

    #[test]
    fn pinned_providers_come_first_and_the_favourite_follows() {
        let data = three_model_data();

        let pinned = super::build_snapshot_view(
            &data,
            &empty_catalog(),
            TimeRange::All,
            ZeroCostBehavior::KeepZero,
            pinned_view(&["ccc"], &["zulu"]),
        );

        let providers: Vec<&str> = pinned.providers.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(providers, vec!["zulu", "alpha"]);
        // The Overview's favourite is the first model row, so it follows the pins.
        assert_eq!(pinned.models[0].id, "ccc");
    }

    #[test]
    fn overview_reports_current_and_longest_streaks() {
        fn event_days_ago(days_ago: i64, session: &str) -> UsageEvent {
            let when = Local::now() - chrono::Duration::days(days_ago);
            UsageEvent {
                session_id: session.to_string(),
                parent_session_id: None,
                session_title: None,
                session_started_at: Some(when),
                session_archived_at: None,
                project_name: None,
                project_path: None,
                provider_id: Some("acme".to_string()),
                model_id: "m".to_string(),
                agent: None,
                finish_reason: None,
                tokens: TokenUsage {
                    input: 10,
                    output: 0,
                    cache_read: 0,
                    cache_write: 0,
                },
                created_at: Some(when),
                completed_at: Some(when),
                stored_cost_usd: None,
            }
        }

        // Current two-day run plus an older five-day run across a gap.
        let data = AppData {
            events: vec![
                event_days_ago(0, "ses_today"),
                event_days_ago(1, "ses_yesterday"),
                event_days_ago(30, "ses_old_0"),
                event_days_ago(31, "ses_old_1"),
                event_days_ago(32, "ses_old_2"),
                event_days_ago(33, "ses_old_3"),
                event_days_ago(34, "ses_old_4"),
            ],
            messages: Vec::new(),
            session_records: Vec::new(),
            import_stats: ImportStats::default(),
        };

        let snapshot = build_snapshot(
            &data,
            &empty_catalog(),
            TimeRange::All,
            ZeroCostBehavior::KeepZero,
        );

        assert_eq!(snapshot.overview.current_streak, 2);
        assert_eq!(snapshot.overview.longest_streak, 5);
    }

    #[test]
    fn overview_current_streak_counts_from_yesterday_when_today_is_empty() {
        fn event_days_ago(days_ago: i64) -> UsageEvent {
            let when = Local::now() - chrono::Duration::days(days_ago);
            UsageEvent {
                session_id: format!("ses_{days_ago}"),
                parent_session_id: None,
                session_title: None,
                session_started_at: Some(when),
                session_archived_at: None,
                project_name: None,
                project_path: None,
                provider_id: Some("acme".to_string()),
                model_id: "m".to_string(),
                agent: None,
                finish_reason: None,
                tokens: TokenUsage {
                    input: 10,
                    output: 0,
                    cache_read: 0,
                    cache_write: 0,
                },
                created_at: Some(when),
                completed_at: Some(when),
                stored_cost_usd: None,
            }
        }

        let data = AppData {
            events: vec![event_days_ago(1), event_days_ago(2)],
            messages: Vec::new(),
            session_records: Vec::new(),
            import_stats: ImportStats::default(),
        };

        let snapshot = build_snapshot(
            &data,
            &empty_catalog(),
            TimeRange::All,
            ZeroCostBehavior::KeepZero,
        );

        assert_eq!(snapshot.overview.current_streak, 2);
        assert_eq!(snapshot.overview.longest_streak, 2);
    }

    #[test]
    fn overview_counts_match_even_without_session_records() {
        let created_at = Local
            .with_ymd_and_hms(2026, 3, 12, 9, 30, 0)
            .single()
            .unwrap();
        let updated_at = Local
            .with_ymd_and_hms(2026, 3, 12, 10, 0, 0)
            .single()
            .unwrap();

        let messages = vec![
            MessageRecord {
                session_id: "ses_1".to_string(),
                role: Some("user".to_string()),
                provider_id: None,
                model_id: None,
                created_at: Some(created_at),
            },
            MessageRecord {
                session_id: "ses_1".to_string(),
                role: Some("assistant".to_string()),
                provider_id: Some("openai".to_string()),
                model_id: Some("gpt-5".to_string()),
                created_at: Some(updated_at),
            },
        ];

        let events = vec![UsageEvent {
            session_id: "ses_1".to_string(),
            parent_session_id: None,
            session_title: Some("Session 1".to_string()),
            session_started_at: Some(created_at),
            session_archived_at: None,
            project_name: Some("demo".to_string()),
            project_path: None,
            provider_id: Some("openai".to_string()),
            model_id: "gpt-5".to_string(),
            agent: None,
            finish_reason: Some("stop".to_string()),
            tokens: TokenUsage {
                input: 10,
                output: 20,
                cache_read: 0,
                cache_write: 0,
            },
            created_at: Some(updated_at),
            completed_at: Some(updated_at),
            stored_cost_usd: None,
        }];

        let with_records = AppData {
            events: events.clone(),
            messages: messages.clone(),
            session_records: vec![SessionRecord {
                session_id: "ses_1".to_string(),
                created_at,
                updated_at,
            }],
            import_stats: ImportStats::default(),
        };
        let without_records = AppData {
            events,
            messages,
            session_records: Vec::new(),
            import_stats: ImportStats::default(),
        };
        let pricing = PricingCatalog {
            models: BTreeMap::new(),
            cache_path: PathBuf::from("/tmp/models.json"),
            refresh_needed: false,
            availability: PricingAvailability::Empty,
            load_notice: None,
        };

        let with_records_snapshot = build_snapshot(
            &with_records,
            &pricing,
            TimeRange::All,
            ZeroCostBehavior::KeepZero,
        );
        let without_records_snapshot = build_snapshot(
            &without_records,
            &pricing,
            TimeRange::All,
            ZeroCostBehavior::KeepZero,
        );

        assert_eq!(with_records_snapshot.overview.sessions, 1);
        assert_eq!(with_records_snapshot.overview.messages, 2);
        assert_eq!(with_records_snapshot.overview.prompts, 1);
        assert_eq!(
            with_records_snapshot.overview.sessions,
            without_records_snapshot.overview.sessions
        );
        assert_eq!(
            with_records_snapshot.overview.messages,
            without_records_snapshot.overview.messages
        );
        assert_eq!(
            with_records_snapshot.overview.prompts,
            without_records_snapshot.overview.prompts
        );
        assert_eq!(
            with_records_snapshot.overview.total_tokens,
            without_records_snapshot.overview.total_tokens
        );
        assert_eq!(
            with_records_snapshot.overview.models_used,
            without_records_snapshot.overview.models_used
        );
    }

    #[test]
    fn overview_prefers_session_records_for_session_count() {
        let created_at = Local
            .with_ymd_and_hms(2026, 3, 12, 9, 30, 0)
            .single()
            .unwrap();
        let updated_at = Local
            .with_ymd_and_hms(2026, 3, 12, 10, 0, 0)
            .single()
            .unwrap();

        let data = AppData {
            events: Vec::new(),
            messages: Vec::new(),
            session_records: vec![SessionRecord {
                session_id: "ses_1".to_string(),
                created_at,
                updated_at,
            }],
            import_stats: ImportStats::default(),
        };
        let pricing = PricingCatalog {
            models: BTreeMap::new(),
            cache_path: PathBuf::from("/tmp/models.json"),
            refresh_needed: false,
            availability: PricingAvailability::Empty,
            load_notice: None,
        };

        let snapshot = build_snapshot(&data, &pricing, TimeRange::All, ZeroCostBehavior::KeepZero);

        assert_eq!(snapshot.overview.sessions, 1);
    }

    #[test]
    fn overview_cost_estimates_when_stored_zero_looks_like_placeholder() {
        let created_at = Local
            .with_ymd_and_hms(2026, 3, 12, 9, 30, 0)
            .single()
            .unwrap();
        let mut models = BTreeMap::new();
        models.insert(
            "openai/gpt-5".to_string(),
            crate::cache::models_cache::ModelPricing {
                input: rust_decimal::Decimal::new(100, 0),
                output: rust_decimal::Decimal::new(100, 0),
                cache_write: rust_decimal::Decimal::ZERO,
                cache_read: rust_decimal::Decimal::ZERO,
            },
        );
        let data = AppData {
            events: vec![UsageEvent {
                session_id: "ses_1".to_string(),
                parent_session_id: None,
                session_title: None,
                session_started_at: Some(created_at),
                session_archived_at: None,
                project_name: None,
                project_path: None,
                provider_id: Some("openai".to_string()),
                model_id: "gpt-5".to_string(),
                agent: None,
                finish_reason: Some("stop".to_string()),
                tokens: TokenUsage {
                    input: 1_000_000,
                    output: 1_000_000,
                    cache_read: 0,
                    cache_write: 0,
                },
                created_at: Some(created_at),
                completed_at: Some(created_at),
                stored_cost_usd: Some(rust_decimal::Decimal::ZERO),
            }],
            messages: Vec::new(),
            session_records: Vec::new(),
            import_stats: ImportStats::default(),
        };
        let pricing = PricingCatalog {
            models,
            cache_path: PathBuf::from("/tmp/models.json"),
            refresh_needed: false,
            availability: PricingAvailability::Cached,
            load_notice: None,
        };

        let snapshot = build_snapshot(
            &data,
            &pricing,
            TimeRange::All,
            ZeroCostBehavior::EstimateWhenZero,
        );

        assert_eq!(
            snapshot.overview.total_cost.known,
            rust_decimal::Decimal::new(200, 0)
        );
        assert!(snapshot.overview.total_cost.has_known);
        assert!(!snapshot.overview.total_cost.missing);
    }

    #[test]
    fn overview_cost_can_keep_zero_when_requested() {
        let created_at = Local
            .with_ymd_and_hms(2026, 3, 12, 9, 30, 0)
            .single()
            .unwrap();
        let mut models = BTreeMap::new();
        models.insert(
            "openai/gpt-5".to_string(),
            crate::cache::models_cache::ModelPricing {
                input: rust_decimal::Decimal::new(100, 0),
                output: rust_decimal::Decimal::new(100, 0),
                cache_write: rust_decimal::Decimal::ZERO,
                cache_read: rust_decimal::Decimal::ZERO,
            },
        );
        let data = AppData {
            events: vec![UsageEvent {
                session_id: "ses_1".to_string(),
                parent_session_id: None,
                session_title: None,
                session_started_at: Some(created_at),
                session_archived_at: None,
                project_name: None,
                project_path: None,
                provider_id: Some("openai".to_string()),
                model_id: "gpt-5".to_string(),
                agent: None,
                finish_reason: Some("stop".to_string()),
                tokens: TokenUsage {
                    input: 1_000_000,
                    output: 1_000_000,
                    cache_read: 0,
                    cache_write: 0,
                },
                created_at: Some(created_at),
                completed_at: Some(created_at),
                stored_cost_usd: Some(rust_decimal::Decimal::ZERO),
            }],
            messages: Vec::new(),
            session_records: Vec::new(),
            import_stats: ImportStats::default(),
        };
        let pricing = PricingCatalog {
            models,
            cache_path: PathBuf::from("/tmp/models.json"),
            refresh_needed: false,
            availability: PricingAvailability::Cached,
            load_notice: None,
        };

        let snapshot = build_snapshot(&data, &pricing, TimeRange::All, ZeroCostBehavior::KeepZero);

        assert_eq!(
            snapshot.overview.total_cost.known,
            rust_decimal::Decimal::ZERO
        );
        assert!(snapshot.overview.total_cost.has_known);
        assert!(!snapshot.overview.total_cost.missing);
    }
}
