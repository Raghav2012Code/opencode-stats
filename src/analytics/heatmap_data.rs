use chrono::{Datelike, Duration, NaiveDate, Weekday};

use crate::db::models::UsageEvent;

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

pub fn build_heatmap_data(events: &[UsageEvent], today: NaiveDate) -> HeatmapData {
    let end = today;
    let start = today - Duration::days(364);
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
    use super::{intensity_thresholds, map_intensity};

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
}
