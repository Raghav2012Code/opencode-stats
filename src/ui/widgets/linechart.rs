//! The usage line chart, with its labels placed by hand.
//!
//! Ratatui spaces its own axis labels by cell, not by the data they name, and it places
//! y ticks by integer division, so their spacing comes out uneven. Both label rows are
//! therefore drawn here: y ticks on exact rows, and date labels under the points they name.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Chart, Dataset, GraphType, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::analytics::model_stats::{ModelChartData, format_tick_label, y_axis_ticks};
use crate::ui::theme::Theme;

/// Rows beside the graph: the x axis line and the date row under it.
const AXIS_ROWS: u16 = 2;
/// The y axis prefers this many equal intervals: four tick labels, `0` to the top.
const PREFERRED_INTERVALS: u16 = 3;

/// Draws the chart into the top of `area`. Graph rows are chosen so the y ticks land on
/// exact rows; any spare rows at the bottom stay blank.
pub fn render_chart(frame: &mut Frame<'_>, area: Rect, chart: &ModelChartData, theme: &Theme) {
    let Some((graph_rows, intervals)) = graph_rows_for(area.height) else {
        return;
    };
    let chart_area = Rect {
        height: graph_rows + AXIS_ROWS,
        ..area
    };

    let (ticks, top) = y_axis_ticks(chart.y_bounds[1], intervals as usize);
    let y_labels = ticks
        .iter()
        .map(|tick| format_tick_label(*tick))
        .collect::<Vec<_>>();
    let label_width = y_labels
        .iter()
        .map(|label| UnicodeWidthStr::width(label.as_str()))
        .max()
        .unwrap_or(0) as u16;
    if label_width.saturating_add(2) >= area.width {
        return;
    }

    let datasets = chart
        .series
        .iter()
        .map(|series| {
            Dataset::default()
                .name(series.model_id.clone())
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(theme.series_color(series.palette_index)))
                .data(&series.points)
        })
        .collect::<Vec<_>>();

    // Blank labels keep ratatui's axis line; the real date labels are drawn below.
    let x_axis = Axis::default()
        .bounds(chart.x_bounds)
        .style(Style::default().fg(theme.muted))
        .labels(vec![Line::from(" "), Line::from(" ")])
        .labels_alignment(Alignment::Center);
    let y_axis = Axis::default()
        .bounds([0.0, top])
        .style(Style::default().fg(theme.muted))
        .labels(y_labels.into_iter().map(Line::from).collect::<Vec<_>>())
        .labels_alignment(Alignment::Right);

    frame.render_widget(
        Chart::new(datasets)
            .legend_position(None)
            .x_axis(x_axis)
            .y_axis(y_axis),
        chart_area,
    );

    // The graph starts one column past the widest y label, where the axis line is.
    let graph_left = chart_area.x + label_width + 1;
    let graph_width = chart_area.right().saturating_sub(graph_left);
    let mut row = vec![' '; graph_width as usize];
    for (start, label) in place_date_labels(&chart.x_labels, graph_width) {
        for (offset, ch) in label.chars().enumerate() {
            row[start as usize + offset] = ch;
        }
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            row.into_iter().collect::<String>(),
            theme.muted_style(),
        ))),
        Rect {
            x: graph_left,
            y: chart_area.bottom() - 1,
            width: graph_width,
            height: 1,
        },
    );
}

/// The graph rows and the number of y intervals for an area `available` rows tall. The
/// graph has `rows - 1` gaps between its ticks, so the intervals must divide that exactly:
/// the largest such row count wins. When none fits, the axis keeps just its two ends.
fn graph_rows_for(available: u16) -> Option<(u16, u16)> {
    let max_rows = available.checked_sub(AXIS_ROWS)?;
    if max_rows < 2 {
        return None;
    }
    (2..=max_rows)
        .rev()
        .find(|rows| (rows - 1) % PREFERRED_INTERVALS == 0)
        .map(|rows| (rows, PREFERRED_INTERVALS))
        .or(Some((max_rows, 1)))
}

/// Where each date label starts, counted from the graph's left edge, or nothing when the
/// labels cannot all stay readable. The first label starts at the left edge, the last ends
/// at the right edge, and each middle label is centred on its own point. The most labels
/// that fit are used, always taken from the list at even fractions, so every label still
/// names the point it sits under.
fn place_date_labels(labels: &[String], graph_width: u16) -> Vec<(u16, String)> {
    let count_all = labels.len();
    if count_all < 2 || graph_width == 0 {
        return Vec::new();
    }
    let last_index = count_all - 1;

    for count in (2..=count_all).rev() {
        let mut placed: Vec<(u16, String)> = Vec::with_capacity(count);
        let mut fits = true;
        for j in 0..count {
            let index = (j * last_index + (count - 1) / 2) / (count - 1);
            let label = &labels[index];
            let width = UnicodeWidthStr::width(label.as_str()) as i32;
            let start = if j == 0 {
                0
            } else if j == count - 1 {
                graph_width as i32 - width
            } else {
                let centre =
                    (index as f64 / last_index as f64 * (graph_width - 1) as f64).round() as i32;
                centre - width / 2
            };
            let clear_of_previous = placed.last().is_none_or(|(prev_start, prev)| {
                start > *prev_start as i32 + UnicodeWidthStr::width(prev.as_str()) as i32
            });
            if start < 0 || start + width > graph_width as i32 || !clear_of_previous {
                fits = false;
                break;
            }
            placed.push((start as u16, label.clone()));
        }
        if fits {
            return placed;
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::{graph_rows_for, place_date_labels};

    fn labels(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn the_graph_rows_leave_exact_tick_spacing() {
        // 10 rows: one stays blank and the graph is 7 rows, giving 3 exact intervals.
        assert_eq!(graph_rows_for(10), Some((7, 3)));
        // A taller area keeps the same rule: the largest fitting graph.
        assert_eq!(graph_rows_for(23), Some((19, 3)));
        // Too short for any interval: the two ends only.
        assert_eq!(graph_rows_for(4), Some((2, 1)));
        assert_eq!(graph_rows_for(3), None);
    }

    #[test]
    fn every_graph_height_gives_exact_spacing() {
        for available in 4..40u16 {
            let (rows, intervals) = graph_rows_for(available).unwrap();
            assert_eq!(
                (rows - 1) % intervals,
                0,
                "{available} rows give uneven ticks"
            );
        }
    }

    #[test]
    fn a_week_of_labels_fits_a_wide_graph_whole() {
        let week = labels(&[
            "Oct 18", "Oct 19", "Oct 20", "Oct 21", "Oct 22", "Oct 23", "Oct 24",
        ]);
        let placed = place_date_labels(&week, 59);

        assert_eq!(placed.len(), 7);
        assert_eq!(placed[0], (0, "Oct 18".to_string()));
        assert_eq!(placed[6].1, "Oct 24");
        assert_eq!(placed[6].0 + 6, 59, "the last label ends at the right edge");
    }

    #[test]
    fn a_narrow_graph_drops_labels_but_keeps_both_ends() {
        let week = labels(&[
            "Oct 18", "Oct 19", "Oct 20", "Oct 21", "Oct 22", "Oct 23", "Oct 24",
        ]);
        let placed = place_date_labels(&week, 30);

        assert!(placed.len() < 7, "{placed:?}");
        assert_eq!(placed.first().unwrap().1, "Oct 18");
        assert_eq!(placed.last().unwrap().1, "Oct 24");
    }

    #[test]
    fn placed_labels_never_touch_or_overflow() {
        let all = labels(&[
            "Mar 1 '26",
            "Apr 27 '26",
            "Jun 22 '26",
            "Aug 17 '26",
            "Oct 24 '26",
        ]);
        for width in 8..120u16 {
            let placed = place_date_labels(&all, width);
            for pair in placed.windows(2) {
                let (start, text) = (&pair[0].0, &pair[0].1);
                assert!(
                    pair[1].0 > start + text.len() as u16,
                    "labels touch at width {width}: {placed:?}"
                );
            }
            if let Some((start, text)) = placed.last() {
                assert!(
                    start + text.len() as u16 <= width,
                    "overflow at width {width}"
                );
            }
        }
    }

    #[test]
    fn labels_that_cannot_fit_at_all_are_left_out() {
        let two = labels(&["Mar 1 '26", "Oct 24 '26"]);
        assert!(place_date_labels(&two, 5).is_empty());
    }
}
