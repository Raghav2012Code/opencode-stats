use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::ui::theme::Theme;

pub const CONTENT_WIDTH: u16 = 70;

pub fn left_aligned_content(area: Rect) -> Rect {
    let width = CONTENT_WIDTH.min(area.width);
    Rect {
        x: area.x,
        y: area.y,
        width,
        height: area.height,
    }
}

pub fn segment_span(label: &str, active: bool, theme: &Theme) -> Span<'static> {
    let style = if active {
        Style::default()
            .fg(theme.tab_active_fg)
            .bg(theme.tab_active_bg)
            .add_modifier(Modifier::BOLD)
    } else {
        theme.muted_style()
    };

    Span::styled(format!(" {} ", label), style)
}

/// Shortens `value` to at most `max_columns` terminal columns, ending in `...` when it
/// is cut. Columns are counted by display width, so a CJK character takes two.
pub fn truncate_label(value: &str, max_columns: usize) -> String {
    if UnicodeWidthStr::width(value) <= max_columns {
        return value.to_string();
    }

    let budget = max_columns.saturating_sub(3);
    let mut truncated = String::new();
    let mut used = 0;
    for ch in value.chars() {
        let width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + width > budget {
            break;
        }
        truncated.push(ch);
        used += width;
    }
    truncated.push_str("...");
    truncated
}

/// `value` padded with spaces on the right to exactly `columns` terminal columns.
pub fn pad_right(value: &str, columns: usize) -> String {
    let fill = columns.saturating_sub(UnicodeWidthStr::width(value));
    format!("{value}{}", " ".repeat(fill))
}

/// Splits `area` into two columns with a two-column gutter, so neighbouring text cannot
/// run into each other.
pub fn two_columns(area: Rect) -> [Rect; 2] {
    let [left, _gutter, right] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(GUTTER),
        Constraint::Fill(1),
    ])
    .areas(area);
    [left, right]
}

/// Columns between the two halves of a metric row.
pub const GUTTER: u16 = 2;

pub fn metric_line<'a>(label: &'a str, value: String, theme: &Theme) -> Line<'a> {
    Line::from(vec![
        Span::styled(label.to_string(), theme.muted_style()),
        Span::raw(value),
    ])
}

/// A metric that fits in `columns`: when label and value together are too wide, the value
/// is shortened with `...` rather than cut silently, so a shortened figure is never read
/// as the whole one.
pub fn metric_line_within<'a>(
    label: &'a str,
    value: String,
    columns: usize,
    theme: &Theme,
) -> Line<'a> {
    let room = columns.saturating_sub(UnicodeWidthStr::width(label));
    metric_line(label, truncate_label(&value, room), theme)
}
