use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TimeRange {
    #[default]
    All,
    Today,
    Last30Days,
    Last7Days,
}

impl TimeRange {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All time",
            Self::Today => "Today",
            Self::Last30Days => "Last 30 days",
            Self::Last7Days => "Last 7 days",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::All => Self::Today,
            Self::Today => Self::Last7Days,
            Self::Last7Days => Self::Last30Days,
            Self::Last30Days => Self::All,
        }
    }

    pub fn from_shortcut(value: char) -> Option<Self> {
        match value {
            '1' => Some(Self::All),
            '2' => Some(Self::Today),
            '3' => Some(Self::Last7Days),
            '4' => Some(Self::Last30Days),
            _ => None,
        }
    }

    pub fn start_date(self, today: NaiveDate) -> Option<NaiveDate> {
        match self {
            Self::All => None,
            Self::Today => Some(today),
            Self::Last30Days => Some(today - Duration::days(29)),
            Self::Last7Days => Some(today - Duration::days(6)),
        }
    }

    /// Machine name used by `stats --range`: one of `all|today|7d|30d`.
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Today => "today",
            Self::Last7Days => "7d",
            Self::Last30Days => "30d",
        }
    }

    pub fn from_cli_name(value: &str) -> Option<Self> {
        match value {
            "all" => Some(Self::All),
            "today" => Some(Self::Today),
            "7d" => Some(Self::Last7Days),
            "30d" => Some(Self::Last30Days),
            _ => None,
        }
    }

    /// Every variant, so error messages and config validation list the same names the
    /// command line accepts instead of repeating them.
    pub const ALL: [Self; 4] = [Self::All, Self::Today, Self::Last7Days, Self::Last30Days];

    pub fn names() -> Vec<&'static str> {
        Self::ALL.into_iter().map(Self::cli_name).collect()
    }
}

pub fn timestamp_ms_to_local(timestamp_ms: i64) -> Option<DateTime<Local>> {
    Local.timestamp_millis_opt(timestamp_ms).single()
}

pub fn current_local_date() -> NaiveDate {
    Local::now().date_naive()
}

pub fn in_range(date: NaiveDate, range: TimeRange, today: NaiveDate) -> bool {
    match range.start_date(today) {
        Some(start) => date >= start && date <= today,
        None => date <= today,
    }
}

#[cfg(test)]
mod tests {
    use super::TimeRange;

    #[test]
    fn cycles_ranges() {
        assert_eq!(TimeRange::All.cycle(), TimeRange::Today);
        assert_eq!(TimeRange::Today.cycle(), TimeRange::Last7Days);
        assert_eq!(TimeRange::Last7Days.cycle(), TimeRange::Last30Days);
        assert_eq!(TimeRange::Last30Days.cycle(), TimeRange::All);
    }

    #[test]
    fn today_starts_at_today() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 17).unwrap();
        assert_eq!(TimeRange::Today.start_date(today), Some(today));
    }

    #[test]
    fn label_matches_cli_range_name() {
        // The `stats --range` CLI name for this variant is "today".
        assert_eq!(TimeRange::Today.label(), "Today");
        assert_eq!(TimeRange::Today.label().to_lowercase(), "today");
    }

    #[test]
    fn shortcuts_cover_every_range() {
        assert_eq!(TimeRange::from_shortcut('1'), Some(TimeRange::All));
        assert_eq!(TimeRange::from_shortcut('2'), Some(TimeRange::Today));
        assert_eq!(TimeRange::from_shortcut('3'), Some(TimeRange::Last7Days));
        assert_eq!(TimeRange::from_shortcut('4'), Some(TimeRange::Last30Days));
        assert_eq!(TimeRange::from_shortcut('5'), None);
    }

    #[test]
    fn cli_names_round_trip() {
        for range in [
            TimeRange::All,
            TimeRange::Today,
            TimeRange::Last7Days,
            TimeRange::Last30Days,
        ] {
            assert_eq!(TimeRange::from_cli_name(range.cli_name()), Some(range));
        }
        assert_eq!(TimeRange::from_cli_name("week"), None);
    }
}
