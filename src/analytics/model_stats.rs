use std::collections::{BTreeMap, BTreeSet};

use chrono::{Datelike, NaiveDate};

use crate::analytics::streaks::streaks;
use crate::cache::models_cache::PricingCatalog;
use crate::db::models::{MessageRecord, TokenUsage, UsageEvent};
use crate::utils::formatting::percentage;
use crate::utils::pricing::{PriceSummary, ZeroCostBehavior, update_price_summary};
use crate::utils::time::TimeRange;

/// One aggregated usage row. `id` is a model id on the Models page and a provider id
/// on the Providers page; every other field means the same on both.
#[derive(Clone, Debug)]
pub struct UsageRow {
    pub id: String,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_tokens: u64,
    pub messages: usize,
    pub prompts: usize,
    pub sessions: usize,
    pub active_days: usize,
    pub cost: PriceSummary,
    pub percentage: f64,
    /// `None` when no event in the row has a measurable output rate.
    pub p50_output_tokens_per_second: Option<f64>,
}

/// One aggregated usage row per session. Unlike [`UsageRow`] this carries the identity
/// a user recognises (title and project) and the span the session covers.
#[derive(Clone, Debug)]
pub struct SessionUsageRow {
    pub id: String,
    pub title: String,
    pub project: String,
    pub is_subagent: bool,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_tokens: u64,
    pub messages: usize,
    pub prompts: usize,
    pub active_days: usize,
    pub longest_streak: usize,
    pub cost: PriceSummary,
    pub first_activity: Option<chrono::DateTime<chrono::Local>>,
    pub last_activity: Option<chrono::DateTime<chrono::Local>>,
    pub percentage: f64,
}

impl SessionUsageRow {
    /// Whole seconds between first and last activity, if both are known.
    pub fn elapsed_secs(&self) -> Option<i64> {
        let (first, last) = (self.first_activity?, self.last_activity?);
        Some(last.signed_duration_since(first).num_seconds().max(0))
    }
}

#[derive(Clone, Debug)]
pub struct ModelChartSeries {
    pub model_id: String,
    pub palette_index: usize,
    pub points: Vec<(f64, f64)>,
}

/// The chart's data. `x_labels[k]` names the point at fraction `k / (len - 1)` of the x
/// axis, and `y_bounds[1]` is the largest value. The renderer draws both label rows, since
/// their exact positions depend on the terminal size.
#[derive(Clone, Debug)]
pub struct ModelChartData {
    pub x_bounds: [f64; 2],
    pub y_bounds: [f64; 2],
    pub x_labels: Vec<String>,
    pub series: Vec<ModelChartSeries>,
}

pub fn build_model_chart(
    events: &[&UsageEvent],
    messages: &[&MessageRecord],
    pricing: &PricingCatalog,
    range: TimeRange,
    today: NaiveDate,
    zero_cost_behavior: ZeroCostBehavior,
) -> (Vec<UsageRow>, ModelChartData) {
    let rows = build_rows(
        events,
        messages,
        pricing,
        zero_cost_behavior,
        model_event_key,
        model_message_key,
    );
    let keys = rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>();
    let chart = build_chart_for_models(events, &keys, range, today, model_event_key);
    (rows, chart)
}

pub fn build_provider_chart(
    events: &[&UsageEvent],
    messages: &[&MessageRecord],
    pricing: &PricingCatalog,
    range: TimeRange,
    today: NaiveDate,
    zero_cost_behavior: ZeroCostBehavior,
) -> (Vec<UsageRow>, ModelChartData) {
    let rows = build_rows(
        events,
        messages,
        pricing,
        zero_cost_behavior,
        provider_event_key,
        provider_message_key,
    );
    let keys = rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>();
    let chart = build_chart_for_models(events, &keys, range, today, provider_event_key);
    (rows, chart)
}

fn provider_label(provider_id: Option<&str>) -> String {
    provider_id.unwrap_or("unknown").to_string()
}

fn model_event_key(event: &UsageEvent) -> String {
    event.model_id.clone()
}

fn provider_event_key(event: &UsageEvent) -> String {
    provider_label(event.provider_id.as_deref())
}

fn model_message_key(message: &MessageRecord) -> Option<String> {
    message.model_id.clone()
}

fn provider_message_key(message: &MessageRecord) -> Option<String> {
    Some(provider_label(message.provider_id.as_deref()))
}

fn build_rows(
    events: &[&UsageEvent],
    messages: &[&MessageRecord],
    pricing: &PricingCatalog,
    zero_cost_behavior: ZeroCostBehavior,
    event_key: fn(&UsageEvent) -> String,
    message_key: fn(&MessageRecord) -> Option<String>,
) -> Vec<UsageRow> {
    let mut grouped = BTreeMap::<String, UsageAccumulator>::new();

    for message in messages {
        let Some(key) = message_key(message) else {
            continue;
        };
        let entry = grouped.entry(key).or_default();
        match message.role.as_deref() {
            Some("assistant") => entry.messages += 1,
            Some("user") => {
                entry.messages += 1;
                entry.prompts += 1;
            }
            _ => {}
        }
    }

    for event in events {
        let event = *event;
        let key = event_key(event);
        let entry = grouped.entry(key).or_default();
        entry.tokens.add_assign(&event.tokens);
        entry.sessions.insert(event.session_id.clone());
        update_price_summary(&mut entry.cost, pricing, event, zero_cost_behavior);
        if let Some(date) = event.activity_date() {
            entry.active_days.insert(date);
            let total = entry.daily_tokens.entry(date).or_default();
            *total = total.saturating_add(event.tokens.total());
        }
        if event.is_rate_eligible()
            && let Some(duration_ms) = event.duration_ms()
        {
            let rate = event.tokens.output as f64 / (duration_ms as f64 / 1_000.0);
            entry.output_rates.push(rate);
        }
    }

    let overall_tokens = grouped
        .values()
        .map(|row| row.tokens.total())
        .fold(0u64, |total, value| total.saturating_add(value));
    let mut rows = grouped
        .into_iter()
        .map(|(id, row)| UsageRow {
            id,
            total_tokens: row.tokens.total(),
            input_tokens: row.tokens.input,
            output_tokens: row.tokens.output,
            cache_tokens: row.tokens.cache_read.saturating_add(row.tokens.cache_write),
            percentage: percentage(row.tokens.total(), overall_tokens),
            messages: row.messages,
            prompts: row.prompts,
            sessions: row.sessions.len(),
            active_days: row.active_days.len(),
            cost: row.cost,
            p50_output_tokens_per_second: median(&row.output_rates),
        })
        .collect::<Vec<_>>();

    rows.sort_by_key(|right| std::cmp::Reverse(right.total_tokens));

    rows
}

/// One row per session in `events`, ordered by cost descending. Message and prompt
/// counts come from `messages` (all roles, not just ones with a model attached), so a
/// session with usage but no prompts still appears.
pub fn build_session_rows(
    events: &[&UsageEvent],
    messages: &[&MessageRecord],
    pricing: &PricingCatalog,
    zero_cost_behavior: ZeroCostBehavior,
    today: NaiveDate,
) -> Vec<SessionUsageRow> {
    #[derive(Default)]
    struct SessionAccumulator {
        tokens: TokenUsage,
        messages: usize,
        prompts: usize,
        active_days: BTreeSet<NaiveDate>,
        cost: PriceSummary,
        first_activity: Option<chrono::DateTime<chrono::Local>>,
        last_activity: Option<chrono::DateTime<chrono::Local>>,
        title: Option<String>,
        project: Option<String>,
        is_subagent: bool,
    }

    let mut grouped = BTreeMap::<String, SessionAccumulator>::new();

    for message in messages {
        let entry = grouped.entry(message.session_id.clone()).or_default();
        match message.role.as_deref() {
            Some("assistant") => entry.messages += 1,
            Some("user") => {
                entry.messages += 1;
                entry.prompts += 1;
            }
            _ => {}
        }
    }

    for event in events {
        let event = *event;
        let entry = grouped.entry(event.session_id.clone()).or_default();
        entry.tokens.add_assign(&event.tokens);
        update_price_summary(&mut entry.cost, pricing, event, zero_cost_behavior);
        if let Some(date) = event.activity_date() {
            entry.active_days.insert(date);
        }
        for moment in [event.created_at, event.completed_at].into_iter().flatten() {
            if entry.first_activity.is_none_or(|first| moment < first) {
                entry.first_activity = Some(moment);
            }
            if entry.last_activity.is_none_or(|last| moment > last) {
                entry.last_activity = Some(moment);
            }
        }
        if entry.title.is_none() {
            entry.title = event.session_title.clone();
        }
        if entry.project.is_none() {
            entry.project = event
                .project_path
                .as_deref()
                .and_then(std::path::Path::file_name)
                .and_then(|name| name.to_str())
                .map(ToOwned::to_owned)
                .or_else(|| event.project_name.clone());
        }
        entry.is_subagent |= event.parent_session_id.is_some();
    }

    let overall_tokens = grouped
        .values()
        .map(|row| row.tokens.total())
        .fold(0u64, |total, value| total.saturating_add(value));
    let mut rows = grouped
        .into_iter()
        .map(|(id, row)| {
            let title = row
                .title
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| format!("Session {}", id.chars().take(8).collect::<String>()));
            SessionUsageRow {
                id,
                title,
                project: row.project.unwrap_or_else(|| "Unknown project".to_string()),
                is_subagent: row.is_subagent,
                total_tokens: row.tokens.total(),
                input_tokens: row.tokens.input,
                output_tokens: row.tokens.output,
                cache_tokens: row.tokens.cache_read.saturating_add(row.tokens.cache_write),
                messages: row.messages,
                prompts: row.prompts,
                active_days: row.active_days.len(),
                longest_streak: streaks(&row.active_days, today).longest,
                cost: row.cost,
                first_activity: row.first_activity,
                last_activity: row.last_activity,
                percentage: percentage(row.tokens.total(), overall_tokens),
            }
        })
        .collect::<Vec<_>>();

    rows.sort_by(|left, right| {
        right
            .cost
            .known
            .cmp(&left.cost.known)
            .then_with(|| right.total_tokens.cmp(&left.total_tokens))
            .then_with(|| left.id.cmp(&right.id))
    });

    rows
}

pub fn chart_with_focus(chart: &ModelChartData, focused_model_id: Option<&str>) -> ModelChartData {
    let mut series = chart.series.clone();

    if let Some(model_id) = focused_model_id
        && let Some(index) = series.iter().position(|series| series.model_id == model_id)
    {
        let focused = series.remove(index);
        series.push(focused);
    }

    ModelChartData {
        x_bounds: chart.x_bounds,
        y_bounds: chart.y_bounds,
        x_labels: chart.x_labels.clone(),
        series,
    }
}

fn build_chart_for_models<F>(
    events: &[&UsageEvent],
    top_models: &[String],
    range: TimeRange,
    today: NaiveDate,
    key_fn: F,
) -> ModelChartData
where
    F: Fn(&UsageEvent) -> String,
{
    let mut daily_values = BTreeMap::<String, BTreeMap<NaiveDate, u64>>::new();
    let mut min_date = today;
    let mut max_date = today;
    let mut has_dates = false;

    for event in events {
        let key = key_fn(event);
        if !top_models.contains(&key) {
            continue;
        }
        let Some(date) = event.activity_date() else {
            continue;
        };
        has_dates = true;
        if date < min_date {
            min_date = date;
        }
        if date > max_date {
            max_date = date;
        }
        let total = daily_values
            .entry(key)
            .or_default()
            .entry(date)
            .or_default();
        *total = total.saturating_add(event.tokens.total());
    }

    if !has_dates {
        return ModelChartData {
            x_bounds: [0.0, 1.0],
            y_bounds: [0.0, 1.0],
            x_labels: vec!["Start".to_string(), "End".to_string()],
            series: Vec::new(),
        };
    }

    // A ranged chart spans the whole range, up to today, even where the first days have no
    // usage; "all time" spans only the days that have data.
    let (first_day, last_day) = match range.start_date(today) {
        Some(start) => (start, today),
        None => (min_date, max_date),
    };
    let mut days = Vec::new();
    let mut cursor = first_day;
    while cursor <= last_day {
        days.push(cursor);
        cursor += chrono::Duration::days(1);
    }

    let mut y_max = 1f64;
    let mut series = Vec::new();
    for (palette_index, model_id) in top_models.iter().enumerate() {
        let points = days
            .iter()
            .enumerate()
            .map(|(index, day)| {
                let value = daily_values
                    .get(model_id)
                    .and_then(|map| map.get(day))
                    .copied()
                    .unwrap_or_default();
                y_max = y_max.max(value as f64);
                (index as f64, value as f64)
            })
            .collect::<Vec<_>>();
        series.push(ModelChartSeries {
            model_id: model_id.clone(),
            palette_index,
            points,
        });
    }

    let last_index = (days.len().saturating_sub(1)) as f64;
    let x_labels = build_x_labels(&days, range);

    ModelChartData {
        x_bounds: [0.0, last_index.max(1.0)],
        y_bounds: [0.0, y_max.max(1.0)],
        x_labels,
        series,
    }
}

fn format_label(date: &NaiveDate) -> String {
    format!("{} {}", date.format("%b"), date.day())
}

/// `Sep 12 '26`: the apostrophe marks the year, so `26` cannot read as a day.
fn format_label_all(date: &NaiveDate) -> String {
    format!(
        "{} {} '{:02}",
        date.format("%b"),
        date.day(),
        date.year() % 100
    )
}

/// `count` positions spread evenly over `len` days, first and last included. Ticks sit
/// evenly on screen, so each label has to name the day at its own even position.
fn label_indices(len: usize, count: usize) -> Vec<usize> {
    let last = len.saturating_sub(1);
    if count < 2 {
        return vec![last];
    }
    (0..count)
        .map(|k| ((k * last) as f64 / (count - 1) as f64).round() as usize)
        .collect()
}

fn build_x_labels(days: &[NaiveDate], range: TimeRange) -> Vec<String> {
    if days.is_empty() {
        return vec!["Start".to_string(), "Mid".to_string(), "End".to_string()];
    }
    if days.len() == 1 {
        let label = format_label(&days[0]);
        return vec![label.clone(), label.clone(), label];
    }

    match range {
        TimeRange::Last7Days | TimeRange::Today => days.iter().map(format_label).collect(),
        TimeRange::Last30Days => label_indices(days.len(), 5)
            .into_iter()
            .map(|index| format_label(&days[index]))
            .collect(),
        TimeRange::All => label_indices(days.len(), 5)
            .into_iter()
            .map(|index| format_label_all(&days[index]))
            .collect(),
    }
}

/// Ticks for a y axis of `intervals` equal steps from zero: a nice step, so the labels read
/// as round numbers, and a top that is at least `max_value`. Returns the tick values and the
/// top of the axis.
pub fn y_axis_ticks(max_value: f64, intervals: usize) -> (Vec<f64>, f64) {
    let intervals = intervals.max(1);
    let step = nice_step((max_value.max(1.0) / intervals as f64).ceil().max(1.0));
    let ticks = (0..=intervals).map(|index| index as f64 * step).collect();
    (ticks, step * intervals as f64)
}

fn nice_step(rough_step: f64) -> f64 {
    if rough_step <= 1.0 {
        return 1.0;
    }

    let magnitude = 10_f64.powf(rough_step.log10().floor());
    let normalized = rough_step / magnitude;
    let nice = if normalized <= 1.0 {
        1.0
    } else if normalized <= 2.0 {
        2.0
    } else if normalized <= 5.0 {
        5.0
    } else {
        10.0
    };

    (nice * magnitude).round()
}

pub fn format_tick_label(value: f64) -> String {
    if value >= 1_000_000_000.0 {
        format!("{:.0}B", value / 1_000_000_000.0)
    } else if value >= 1_000_000.0 {
        format!("{:.0}M", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        format!("{:.0}K", value / 1_000.0)
    } else {
        format!("{value:.0}")
    }
}

#[derive(Default)]
struct UsageAccumulator {
    tokens: TokenUsage,
    messages: usize,
    prompts: usize,
    sessions: BTreeSet<String>,
    active_days: BTreeSet<NaiveDate>,
    cost: PriceSummary,
    daily_tokens: BTreeMap<NaiveDate, u64>,
    output_rates: Vec<f64>,
}

fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut values = values.to_vec();
    values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        Some((values[middle - 1] + values[middle]) / 2.0)
    } else {
        Some(values[middle])
    }
}

#[cfg(test)]
mod tests {
    use super::{build_chart_for_models, build_model_chart, build_provider_chart};
    use crate::cache::models_cache::{PricingAvailability, PricingCatalog};
    use crate::db::models::{MessageRecord, TokenUsage, UsageEvent};
    use crate::utils::pricing::ZeroCostBehavior;
    use crate::utils::time::TimeRange;
    use chrono::{Local, NaiveDate, TimeZone};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn empty_pricing() -> PricingCatalog {
        PricingCatalog {
            models: BTreeMap::new(),
            cache_path: PathBuf::from("/tmp/models.json"),
            refresh_needed: false,
            availability: PricingAvailability::Empty,
            load_notice: None,
        }
    }

    fn usage_event(provider_id: &str, model_id: &str, day: u32) -> UsageEvent {
        let created_at = Local
            .with_ymd_and_hms(2026, 3, day, 9, 30, 0)
            .single()
            .unwrap();

        UsageEvent {
            session_id: "ses_1".to_string(),
            parent_session_id: None,
            session_title: None,
            session_started_at: Some(created_at),
            session_archived_at: None,
            project_name: None,
            project_path: None,
            provider_id: Some(provider_id.to_string()),
            model_id: model_id.to_string(),
            agent: None,
            finish_reason: None,
            tokens: TokenUsage {
                input: 1,
                output: 2,
                cache_read: 0,
                cache_write: 0,
            },
            created_at: Some(created_at),
            completed_at: Some(created_at),
            stored_cost_usd: None,
        }
    }

    #[test]
    fn each_page_keys_its_rows_and_chart_by_its_own_field() {
        let event = usage_event("openai", "gpt-5", 12);
        let message = MessageRecord {
            session_id: "ses_1".to_string(),
            role: Some("assistant".to_string()),
            provider_id: Some("openai".to_string()),
            model_id: Some("gpt-5".to_string()),
            created_at: event.created_at,
        };
        let today = NaiveDate::from_ymd_opt(2026, 3, 12).unwrap();
        let pricing = empty_pricing();

        let (model_rows, model_chart) = build_model_chart(
            &[&event],
            &[&message],
            &pricing,
            TimeRange::All,
            today,
            ZeroCostBehavior::KeepZero,
        );
        let (provider_rows, provider_chart) = build_provider_chart(
            &[&event],
            &[&message],
            &pricing,
            TimeRange::All,
            today,
            ZeroCostBehavior::KeepZero,
        );

        assert_eq!(model_rows.len(), 1);
        assert_eq!(model_rows[0].id, "gpt-5");
        assert_eq!(provider_rows.len(), 1);
        assert_eq!(provider_rows[0].id, "openai");

        assert_eq!(model_chart.series.len(), 1);
        assert_eq!(model_chart.series[0].model_id, "gpt-5");
        assert_eq!(provider_chart.series.len(), 1);
        assert_eq!(provider_chart.series[0].model_id, "openai");
    }

    fn session_event(
        session_id: &str,
        parent_session_id: Option<&str>,
        title: Option<&str>,
        project: Option<&str>,
        input_tokens: u64,
        cost_cents: i64,
    ) -> UsageEvent {
        let base = usage_event("openai", "gpt-5", 12);
        UsageEvent {
            session_id: session_id.to_string(),
            parent_session_id: parent_session_id.map(ToOwned::to_owned),
            session_title: title.map(ToOwned::to_owned),
            project_name: project.map(ToOwned::to_owned),
            tokens: TokenUsage {
                input: input_tokens,
                ..base.tokens
            },
            stored_cost_usd: Some(rust_decimal::Decimal::new(cost_cents, 2)),
            ..base
        }
    }

    fn session_message(session_id: &str, role: &str) -> MessageRecord {
        MessageRecord {
            session_id: session_id.to_string(),
            role: Some(role.to_string()),
            provider_id: Some("openai".to_string()),
            model_id: Some("gpt-5".to_string()),
            created_at: None,
        }
    }

    #[test]
    fn sessions_order_by_cost_and_carry_identity() {
        let cheap = session_event("ses_cheap", None, Some("Cheap chat"), Some("demo"), 10, 10);
        let big = session_event(
            "ses_big",
            Some("ses_1"),
            Some("Big job"),
            Some("demo"),
            20,
            500,
        );
        let events = [cheap, big];
        let m1 = session_message("ses_cheap", "user");
        let m2 = session_message("ses_cheap", "assistant");
        let m3 = session_message("ses_big", "assistant");
        let messages = [m1, m2, m3];
        let borrowed_events = [&events[0], &events[1]];
        let borrowed_messages = [&messages[0], &messages[1], &messages[2]];

        let rows = super::build_session_rows(
            &borrowed_events,
            &borrowed_messages,
            &empty_pricing(),
            ZeroCostBehavior::KeepZero,
            NaiveDate::from_ymd_opt(2026, 3, 12).unwrap(),
        );

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "ses_big");
        assert_eq!(rows[0].title, "Big job");
        assert_eq!(rows[0].project, "demo");
        assert!(rows[0].is_subagent);
        assert_eq!(rows[0].messages, 1);
        assert_eq!(rows[0].prompts, 0);
        assert_eq!(rows[1].id, "ses_cheap");
        assert!(!rows[1].is_subagent);
        // One user turn and one assistant turn: two messages, one of them a prompt.
        assert_eq!(rows[1].messages, 2);
        assert_eq!(rows[1].prompts, 1);
        assert!(rows[0].percentage > rows[1].percentage);
    }

    #[test]
    fn session_without_prompts_still_appears() {
        let quiet = session_event("ses_quiet", None, None, None, 5, 7);
        let only = session_message("ses_quiet", "assistant");
        let borrowed_events = [&quiet];
        let borrowed_messages = [&only];

        let rows = super::build_session_rows(
            &borrowed_events,
            &borrowed_messages,
            &empty_pricing(),
            ZeroCostBehavior::KeepZero,
            NaiveDate::from_ymd_opt(2026, 3, 12).unwrap(),
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].prompts, 0);
        assert_eq!(rows[0].messages, 1);
        assert_eq!(rows[0].project, "Unknown project");
        assert!(rows[0].title.starts_with("Session "));
    }

    #[test]
    fn session_span_covers_first_to_last_activity() {
        let morning = UsageEvent {
            created_at: Local.with_ymd_and_hms(2026, 3, 12, 9, 0, 0).single(),
            completed_at: Local.with_ymd_and_hms(2026, 3, 12, 9, 0, 0).single(),
            ..session_event("ses_long", None, None, None, 5, 7)
        };
        let evening = UsageEvent {
            created_at: Local.with_ymd_and_hms(2026, 3, 12, 12, 30, 0).single(),
            completed_at: Local.with_ymd_and_hms(2026, 3, 12, 12, 30, 0).single(),
            ..session_event("ses_long", None, None, None, 5, 7)
        };
        let borrowed = [&evening, &morning];
        let no_messages: Vec<&MessageRecord> = Vec::new();

        let rows = super::build_session_rows(
            &borrowed,
            &no_messages,
            &empty_pricing(),
            ZeroCostBehavior::KeepZero,
            NaiveDate::from_ymd_opt(2026, 3, 12).unwrap(),
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].elapsed_secs(), Some(3 * 3_600 + 30 * 60));
        assert!(rows[0].first_activity < rows[0].last_activity);
    }

    #[test]
    fn session_rows_carry_the_longest_streak() {
        let today = NaiveDate::from_ymd_opt(2026, 3, 12).unwrap();
        let on_day = |session: &str, day: u32| {
            let when = Local.with_ymd_and_hms(2026, 3, day, 9, 0, 0).single();
            let mut event = session_event(session, None, None, None, 5, 7);
            event.created_at = when;
            event.completed_at = when;
            event.session_started_at = when;
            event
        };
        let events = [
            on_day("ses_streaky", 8),
            on_day("ses_streaky", 9),
            on_day("ses_streaky", 11),
            on_day("ses_streaky", 12),
            on_day("ses_once", 12),
        ];
        let borrowed = events.iter().collect::<Vec<_>>();
        let no_messages: Vec<&MessageRecord> = Vec::new();

        let rows = super::build_session_rows(
            &borrowed,
            &no_messages,
            &empty_pricing(),
            ZeroCostBehavior::KeepZero,
            today,
        );

        let streaky = rows.iter().find(|row| row.id == "ses_streaky").unwrap();
        assert_eq!(streaky.active_days, 4);
        assert_eq!(streaky.longest_streak, 2);
        let once = rows.iter().find(|row| row.id == "ses_once").unwrap();
        assert_eq!(once.longest_streak, 1);
    }

    #[test]
    fn single_day_chart_builds_labels_without_unwrap_assumptions() {
        let day = NaiveDate::from_ymd_opt(2026, 3, 12).unwrap();
        let created_at = Local
            .with_ymd_and_hms(2026, 3, 12, 9, 30, 0)
            .single()
            .unwrap();
        let event = UsageEvent {
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
            finish_reason: None,
            tokens: TokenUsage {
                input: 1,
                output: 2,
                cache_read: 0,
                cache_write: 0,
            },
            created_at: Some(created_at),
            completed_at: Some(created_at),
            stored_cost_usd: None,
        };
        let chart = build_chart_for_models(
            &[&event],
            &["gpt-5".to_string()],
            TimeRange::All,
            day,
            |event| event.model_id.clone(),
        );

        assert_eq!(chart.x_labels.len(), 3);
        assert_eq!(chart.x_labels[0], "Mar 12");
        assert_eq!(chart.x_labels[2], "Mar 12");
    }

    fn event_on(model: &str, day: NaiveDate) -> UsageEvent {
        let at = day
            .and_hms_opt(9, 0, 0)
            .unwrap()
            .and_local_timezone(Local)
            .unwrap();
        UsageEvent {
            session_id: "ses_1".to_string(),
            parent_session_id: None,
            session_title: None,
            session_started_at: Some(at),
            session_archived_at: None,
            project_name: None,
            project_path: None,
            provider_id: Some("openai".to_string()),
            model_id: model.to_string(),
            agent: None,
            finish_reason: Some("stop".to_string()),
            tokens: TokenUsage {
                input: 1,
                output: 2,
                cache_read: 0,
                cache_write: 0,
            },
            created_at: Some(at),
            completed_at: Some(at),
            stored_cost_usd: None,
        }
    }

    #[test]
    fn a_ranged_chart_spans_the_whole_range_up_to_today() {
        // Usage only two days ago in a 7-day window: the chart still covers all seven days.
        let today = NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
        let event = event_on("gpt-5", today - chrono::Duration::days(2));
        let chart = build_chart_for_models(
            &[&event],
            &["gpt-5".to_string()],
            TimeRange::Last7Days,
            today,
            |event| event.model_id.clone(),
        );

        assert_eq!(chart.series[0].points.len(), 7);
        assert_eq!(chart.x_labels.len(), 7);
        assert_eq!(chart.x_labels[0], "Oct 18");
        assert_eq!(chart.x_labels[6], "Oct 24");
    }

    #[test]
    fn the_thirty_day_chart_labels_sit_at_even_positions() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
        let event = event_on("gpt-5", today - chrono::Duration::days(29));
        let chart = build_chart_for_models(
            &[&event],
            &["gpt-5".to_string()],
            TimeRange::Last30Days,
            today,
            |event| event.model_id.clone(),
        );

        // 30 days, 5 labels: indices 0, 7, 15, 22, 29 (each share rounded to a whole day).
        assert_eq!(
            chart.x_labels,
            ["Sep 25", "Oct 2", "Oct 10", "Oct 17", "Oct 24"]
        );
    }

    #[test]
    fn all_time_labels_carry_the_year_and_sit_at_even_positions() {
        let first = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        let last = NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
        let events = [event_on("gpt-5", first), event_on("gpt-5", last)];
        let refs = events.iter().collect::<Vec<_>>();
        let chart = build_chart_for_models(
            &refs,
            &["gpt-5".to_string()],
            TimeRange::All,
            last,
            |event| event.model_id.clone(),
        );

        let span = (last - first).num_days();
        assert_eq!(chart.x_labels.len(), 5);
        assert_eq!(chart.x_labels[0], "Mar 1 '26");
        assert_eq!(chart.x_labels[4], "Oct 24 '26");
        // The middle label names the day at the middle of the span.
        let middle = first + chrono::Duration::days((span as f64 / 2.0).round() as i64);
        assert_eq!(chart.x_labels[2], super::format_label_all(&middle));
    }

    #[test]
    fn large_tick_values_are_written_in_billions() {
        assert_eq!(super::format_tick_label(150_000_000_000.0), "150B");
        assert_eq!(super::format_tick_label(2_000_000_000.0), "2B");
        assert_eq!(super::format_tick_label(3_000_000.0), "3M");
    }
}
