use rust_decimal::Decimal;

use crate::utils::pricing::PriceSummary;

pub fn format_tokens(value: u64) -> String {
    match value {
        0..=999 => value.to_string(),
        1_000..=999_999 => format!("{:.2}K", value as f64 / 1_000.0),
        1_000_000..=999_999_999 => format!("{:.2}M", value as f64 / 1_000_000.0),
        _ => format!("{:.2}B", value as f64 / 1_000_000_000.0),
    }
}

pub fn format_usd_precise(value: Decimal) -> String {
    if value >= Decimal::ONE {
        format!("${:.2}", value.round_dp(2))
    } else if value == Decimal::ZERO {
        "$0.00".to_string()
    } else {
        format!("${:.4}", value.round_dp(4))
    }
}

pub fn format_price_summary(value: &PriceSummary) -> String {
    if !value.has_known {
        return if value.missing {
            "--".to_string()
        } else {
            format_usd_precise(Decimal::ZERO)
        };
    }

    let amount = format_usd_precise(value.known);
    if value.missing {
        format!("{amount} + ?")
    } else {
        amount
    }
}

pub fn tokens_comparison_text(total_tokens: u64) -> String {
    if total_tokens == 0 {
        return "No activity yet. Start a conversation!".to_string();
    }

    let tokens = total_tokens as f64;

    // -- Time Constants (Thresholds for 1.0) --
    const MIN_READING: f64 = 300.0;
    const HOUR_READING: f64 = MIN_READING * 60.0; // 18,000 tokens
    const DAY_COZY_READING: f64 = HOUR_READING * 4.0; // 72,000 tokens (4 hours/day)
    const YEAR_JOURNALING: f64 = 200_000.0; // 每年写日记
    const YEAR_SPEAKING: f64 = 6_300_000.0; // 每年日常说话
    const LIFETIME_SPEAKING: f64 = YEAR_SPEAKING * 80.0; // 504,000,000 tokens (80 years)
    const MILLENNIUM_SPEAKING: f64 = YEAR_SPEAKING * 1000.0; // 6,300,000,000 tokens
    const EPOCH_CIVILIZATION: f64 = YEAR_SPEAKING * 100_000.0; // 630,000,000,000 tokens

    // -- Volume Constants (Thresholds for 1.0) --
    const LETTER: f64 = 400.0; // 信件
    const NOTEBOOK: f64 = 25_000.0; // 笔记本
    const NOVEL: f64 = 120_000.0; // 小说
    const BOOKSHELF: f64 = NOVEL * 40.0; // 4,800,000 tokens (书架)
    const LIBRARY_SECTION: f64 = BOOKSHELF * 10.0; // 48,000,000 tokens (图书馆专区)
    const TOWN_LIBRARY: f64 = NOVEL * 20_000.0; // 2,400,000,000 tokens (小镇图书馆)
    const NATIONAL_LIBRARY: f64 = NOVEL * 10_000_000.0; // 1,200,000,000,000 tokens (国家图书馆)

    let obj_str = if tokens >= NATIONAL_LIBRARY {
        format!(
            "Roughly {:.2} national libraries",
            tokens / NATIONAL_LIBRARY
        )
    } else if tokens >= TOWN_LIBRARY {
        format!("Like {:.2} town libraries", tokens / TOWN_LIBRARY)
    } else if tokens >= LIBRARY_SECTION {
        format!("About {:.2} library sections", tokens / LIBRARY_SECTION)
    } else if tokens >= BOOKSHELF {
        format!("Like {:.2} packed bookshelves", tokens / BOOKSHELF)
    } else if tokens >= NOVEL {
        format!("Around {:.2} thick novels", tokens / NOVEL)
    } else if tokens >= NOTEBOOK {
        format!("Roughly {:.2} filled notebooks", tokens / NOTEBOOK)
    } else {
        format!("About {:.2} handwritten letters", tokens / LETTER)
    };

    let time_str = if tokens >= EPOCH_CIVILIZATION {
        format!("{:.2} epochs of civilization", tokens / EPOCH_CIVILIZATION)
    } else if tokens >= MILLENNIUM_SPEAKING {
        format!(
            "{:.2} millennia of human speech",
            tokens / MILLENNIUM_SPEAKING
        )
    } else if tokens >= LIFETIME_SPEAKING {
        format!("{:.2} lifetimes of words", tokens / LIFETIME_SPEAKING)
    } else if tokens >= YEAR_SPEAKING {
        format!("{:.2} years of daily speaking", tokens / YEAR_SPEAKING)
    } else if tokens >= YEAR_JOURNALING {
        format!("{:.2} years of journaling", tokens / YEAR_JOURNALING)
    } else if tokens >= DAY_COZY_READING {
        format!("{:.2} days of cozy reading", tokens / DAY_COZY_READING)
    } else if tokens >= HOUR_READING {
        format!("{:.2} hours of reading", tokens / HOUR_READING)
    } else {
        format!("{:.2} mins of reading", tokens / MIN_READING)
    };

    format!("{}, or {}.", obj_str, time_str)
}

pub fn percentage(part: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (part as f64 / total as f64) * 100.0
    }
}

/// A whole-seconds span as a short human duration: `45s`, `12m`, `3h 12m`, `2d 4h`.
pub fn format_duration(total_secs: i64) -> String {
    let total_secs = total_secs.max(0);
    let days = total_secs / 86_400;
    let hours = total_secs % 86_400 / 3_600;
    let minutes = total_secs % 3_600 / 60;
    let seconds = total_secs % 60;

    if days > 0 {
        if hours > 0 {
            format!("{days}d {hours}h")
        } else {
            format!("{days}d")
        }
    } else if hours > 0 {
        if minutes > 0 {
            format!("{hours}h {minutes}m")
        } else {
            format!("{hours}h")
        }
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::format_duration;

    #[test]
    fn formats_durations_without_trailing_zero_units() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(45), "45s");
        assert_eq!(format_duration(60), "1m");
        assert_eq!(format_duration(150), "2m");
        assert_eq!(format_duration(3_600), "1h");
        assert_eq!(format_duration(11_520), "3h 12m");
        assert_eq!(format_duration(183_600), "2d 3h");
        assert_eq!(format_duration(172_800), "2d");
        assert_eq!(format_duration(-5), "0s");
    }
}
