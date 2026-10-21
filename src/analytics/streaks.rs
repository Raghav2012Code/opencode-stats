use std::collections::BTreeSet;

use chrono::{Duration, NaiveDate};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Streaks {
    pub current: usize,
    pub longest: usize,
}

/// Current and longest run in `days`, ignoring any date after `today`.
pub fn streaks(days: &BTreeSet<NaiveDate>, today: NaiveDate) -> Streaks {
    let mut longest = 0usize;
    let mut run = 0usize;
    let mut prev: Option<NaiveDate> = None;
    for day in days.iter().copied().filter(|day| *day <= today) {
        if prev.is_some_and(|prev| day == prev + Duration::days(1)) {
            run += 1;
        } else {
            run = 1;
        }
        longest = longest.max(run);
        prev = Some(day);
    }

    let anchor = if days.contains(&today) {
        Some(today)
    } else if days.contains(&(today - Duration::days(1))) {
        Some(today - Duration::days(1))
    } else {
        None
    };
    let mut current = 0usize;
    let mut cursor = anchor;
    while let Some(day) = cursor {
        if days.contains(&day) {
            current += 1;
            cursor = Some(day - Duration::days(1));
        } else {
            break;
        }
    }

    Streaks { current, longest }
}

#[cfg(test)]
mod tests {
    use super::streaks;
    use chrono::NaiveDate;
    use std::collections::BTreeSet;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 20).unwrap()
    }

    fn set(offsets: &[i64]) -> BTreeSet<NaiveDate> {
        offsets
            .iter()
            .map(|ago| today() - chrono::Duration::days(*ago))
            .collect()
    }

    #[test]
    fn empty_has_no_streak() {
        assert_eq!(
            streaks(&BTreeSet::new(), today()),
            super::Streaks {
                current: 0,
                longest: 0
            }
        );
    }

    #[test]
    fn single_day_is_one() {
        let streak = streaks(&set(&[0]), today());

        assert_eq!(streak.current, 1);
        assert_eq!(streak.longest, 1);
    }

    #[test]
    fn unbroken_run_counts_fully() {
        let streak = streaks(&set(&[4, 3, 2, 1, 0]), today());

        assert_eq!(streak.current, 5);
        assert_eq!(streak.longest, 5);
    }

    #[test]
    fn broken_run_reports_both_sides() {
        let streak = streaks(&set(&[4, 3, 1, 0]), today());

        assert_eq!(streak.current, 2);
        assert_eq!(streak.longest, 2);
    }

    #[test]
    fn longest_can_predate_the_current_run() {
        let streak = streaks(&set(&[10, 9, 8, 7, 1, 0]), today());

        assert_eq!(streak.current, 2);
        assert_eq!(streak.longest, 4);
    }

    #[test]
    fn streak_ending_yesterday_still_counts_as_current() {
        let streak = streaks(&set(&[2, 1]), today());

        assert_eq!(streak.current, 2);
        assert_eq!(streak.longest, 2);
    }

    #[test]
    fn gap_before_today_breaks_the_current_streak() {
        let streak = streaks(&set(&[3, 2, 0]), today());

        assert_eq!(streak.current, 1);
        assert_eq!(streak.longest, 2);
    }

    #[test]
    fn future_days_never_extend_a_streak() {
        let mut days = set(&[1, 0]);
        days.insert(today() + chrono::Duration::days(1));
        days.insert(today() + chrono::Duration::days(2));

        let streak = streaks(&days, today());

        assert_eq!(streak.current, 2);
        assert_eq!(streak.longest, 2);
    }
}
