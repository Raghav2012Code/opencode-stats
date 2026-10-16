use chrono::{Datelike, Duration, NaiveDate, Weekday};

use crate::db::models::UsageEvent;
use crate::utils::time::TimeRange;

#[derive(Clone, Debug)]
pub struct HeatmapCell {
    pub intensity: u8,
    pub is_future: bool,
}

#[derive(Clone, Debug)]
pub struct HeatmapData {
    pub weeks: Vec<Vec<HeatmapCell>>,
    pub month_labels: Vec<(usize, String)>,
}

/// How far back the map reaches when the range is "all time".
const ALL_TIME_DAYS: i64 = 364;

/// Builds the contribution grid for the selected range: the window ends today and starts
/// at the range's start date, padded out to whole weeks. A range of "all time" keeps the
/// year-at-a-glance window it has always shown.
pub fn build_heatmap_data(
    events: &[&UsageEvent],
    range: TimeRange,
    today: NaiveDate,
) -> HeatmapData {
    let end = today;
    let start = range
        .start_date(today)
        .unwrap_or_else(|| today - Duration::days(ALL_TIME_DAYS));
    let grid_start = start - Duration::days(start.weekday().num_days_from_monday() as i64);
    let grid_end = end + Duration::days((6 - end.weekday().num_days_from_monday()) as i64);

    let mut totals = std::collections::BTreeMap::<NaiveDate, u64>::new();
    for event in events {
        let Some(date) = event.activity_date() else {
            continue;
        };
        if date < start || date > end {
            continue;
        }
        let total = totals.entry(date).or_default();
        *total = total.saturating_add(event.tokens.total());
    }

    let thresholds = intensity_thresholds(totals.values().copied().filter(|total| *total > 0));
    let mut date = grid_start;
    let mut weeks = Vec::new();
    let mut month_labels = Vec::new();
    let mut week_index = 0usize;
    while date <= grid_end {
        let mut week = Vec::new();
        for _ in 0..7 {
            let tokens = totals.get(&date).copied().unwrap_or_default();
            let intensity = map_intensity(tokens, &thresholds);
            week.push(HeatmapCell {
                intensity,
                is_future: date > end,
            });
            if date.day() == 1 {
                let label = date.format("%b").to_string();
                if month_labels
                    .last()
                    .map(|(_, current)| current != &label)
                    .unwrap_or(true)
                {
                    month_labels.push((week_index, label));
                }
            }
            date += Duration::days(1);
        }
        weeks.push(week);
        week_index += 1;
    }

    HeatmapData {
        weeks,
        month_labels,
    }
}

/// The quartiles of the window's active days, by nearest rank. Deriving the bucket edges
/// from the distribution, rather than from the busiest day, stops one outlier from pushing
/// every other day into the lowest bucket.
fn intensity_thresholds(active: impl Iterator<Item = u64>) -> [u64; 3] {
    let mut values = active.collect::<Vec<_>>();
    values.sort_unstable();

    if values.is_empty() {
        return [0, 0, 0];
    }

    let at = |numerator: usize, denominator: usize| {
        let rank = (values.len() * numerator).div_ceil(denominator).max(1);
        values[rank - 1]
    };

    [at(1, 4), at(1, 2), at(3, 4)]
}

/// Level 0 is a day with no usage at all, which is absence rather than low activity, so it
/// takes no part in the distribution. A token count equal to a threshold takes the lower
/// bucket, so two days with the same total never get different shades.
fn map_intensity(tokens: u64, thresholds: &[u64; 3]) -> u8 {
    if tokens == 0 {
        return 0;
    }

    let [first, second, third] = *thresholds;
    if tokens <= first {
        1
    } else if tokens <= second {
        2
    } else if tokens <= third {
        3
    } else {
        4
    }
}

pub fn weekday_labels() -> [Weekday; 7] {
    [
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
        Weekday::Sat,
        Weekday::Sun,
    ]
}

#[cfg(test)]
mod tests {
    use super::{build_heatmap_data, intensity_thresholds, map_intensity};
    use crate::db::models::{DataSourceKind, TokenUsage, UsageEvent};
    use crate::utils::time::TimeRange;
    use chrono::{Local, NaiveDate};

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 17).unwrap()
    }

    fn event_days_ago(days: i64, tokens: u64) -> UsageEvent {
        let date = today() - chrono::Duration::days(days);
        let created_at = date
            .and_hms_opt(9, 0, 0)
            .unwrap()
            .and_local_timezone(Local)
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
            provider_id: None,
            model_id: "gpt-5".to_string(),
            agent: None,
            finish_reason: None,
            tokens: TokenUsage {
                input: tokens,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            },
            created_at: Some(created_at),
            completed_at: Some(created_at),
            stored_cost_usd: None,
            source: DataSourceKind::Json,
        }
    }

    fn level_counts(data: &super::HeatmapData) -> [usize; 5] {
        let mut counts = [0usize; 5];
        for week in &data.weeks {
            for cell in week {
                counts[cell.intensity as usize] += 1;
            }
        }
        counts
    }

    fn thresholds(values: &[u64]) -> [u64; 3] {
        intensity_thresholds(values.iter().copied())
    }

    fn levels(values: &[u64]) -> [usize; 5] {
        let edges = thresholds(values);
        let mut counts = [0usize; 5];
        for tokens in values {
            counts[map_intensity(*tokens, &edges) as usize] += 1;
        }
        counts
    }

    #[test]
    fn thresholds_are_the_nearest_rank_quartiles() {
        let series = (1..=100u64).collect::<Vec<_>>();
        assert_eq!(thresholds(&series), [25, 50, 75]);
    }

    #[test]
    fn maps_levels_from_the_thresholds() {
        let edges = [10, 30, 60];

        assert_eq!(map_intensity(0, &edges), 0);
        assert_eq!(map_intensity(5, &edges), 1);
        assert_eq!(map_intensity(10, &edges), 1);
        assert_eq!(map_intensity(11, &edges), 2);
        assert_eq!(map_intensity(30, &edges), 2);
        assert_eq!(map_intensity(60, &edges), 3);
        assert_eq!(map_intensity(61, &edges), 4);
    }

    #[test]
    fn tied_days_share_a_bucket() {
        assert_eq!(thresholds(&[7, 7, 7, 7]), [7, 7, 7]);
        assert_eq!(map_intensity(7, &[7, 7, 7]), 1);
    }

    #[test]
    fn sparse_windows_are_handled() {
        assert_eq!(thresholds(&[]), [0, 0, 0]);
        assert_eq!(thresholds(&[5]), [5, 5, 5]);
        assert_eq!(thresholds(&[5, 9]), [5, 5, 9]);

        assert_eq!(map_intensity(5, &[5, 5, 5]), 1);
        assert_eq!(map_intensity(9, &[5, 5, 9]), 3);
    }

    #[test]
    fn one_outlier_day_does_not_flatten_the_map() {
        // The shape that collapsed the old even-interval buckets: one enormous day and a
        // long tail below a quarter of it.
        let mut series = (1..=50u64).map(|day| day * 1_000_000).collect::<Vec<_>>();
        series.push(1_000_000_000);

        let counts = levels(&series);

        for level in 1..=4 {
            assert!(
                counts[level] > 0,
                "every level must be used, got {counts:?}"
            );
        }
        assert!(
            counts[1] <= series.len() / 4 + 1,
            "the lowest level must not hold most active days, got {counts:?}"
        );
    }

    #[test]
    fn all_time_keeps_the_year_at_a_glance_window() {
        let events = [event_days_ago(0, 10)];
        let borrowed = events.iter().collect::<Vec<_>>();

        let data = build_heatmap_data(&borrowed, TimeRange::All, today());

        assert_eq!(data.weeks.len(), 53, "all time covers a padded year");
        assert_eq!(data.weeks[0].len(), 7);
        assert!(data.month_labels.iter().all(|(week, _)| *week < 53));
    }

    #[test]
    fn a_range_shrinks_the_grid_to_that_range() {
        let events = (0..40)
            .map(|days| event_days_ago(days, 1_000))
            .collect::<Vec<_>>();
        let borrowed = events.iter().collect::<Vec<_>>();

        let week = build_heatmap_data(&borrowed, TimeRange::Last7Days, today());
        let month = build_heatmap_data(&borrowed, TimeRange::Last30Days, today());

        assert_eq!(week.weeks.len(), 2, "seven days plus padding");
        assert_eq!(month.weeks.len(), 5, "thirty days plus padding");
        assert!(week.month_labels.iter().all(|(index, _)| *index < 2));
        assert!(month.month_labels.iter().all(|(index, _)| *index < 5));
    }

    #[test]
    fn a_range_ignores_usage_outside_it() {
        // The huge day is well outside the week, so it can neither colour a cell nor set
        // the bucket edges for the days that are inside.
        let events = [
            event_days_ago(0, 1_000),
            event_days_ago(3, 2_000),
            event_days_ago(40, 5_000_000_000),
        ];
        let borrowed = events.iter().collect::<Vec<_>>();

        let data = build_heatmap_data(&borrowed, TimeRange::Last7Days, today());
        let counts = level_counts(&data);

        assert_eq!(counts.iter().sum::<usize>(), 14, "two padded weeks");
        assert_eq!(
            counts[0], 12,
            "only the two days inside the range have usage, got {counts:?}"
        );
    }

    #[test]
    fn a_short_range_still_uses_every_level() {
        let events = (0..7)
            .map(|days| event_days_ago(days, (7 - days) as u64 * 1_000_000))
            .collect::<Vec<_>>();
        let borrowed = events.iter().collect::<Vec<_>>();

        let counts = level_counts(&build_heatmap_data(
            &borrowed,
            TimeRange::Last7Days,
            today(),
        ));

        for level in 1..=4 {
            assert!(
                counts[level] > 0,
                "a week of usage must still spread across the levels, got {counts:?}"
            );
        }
    }

    #[test]
    fn an_empty_range_renders_as_empty() {
        let data = build_heatmap_data(&[], TimeRange::Last7Days, today());

        assert_eq!(level_counts(&data), [14, 0, 0, 0, 0]);
    }
}
