use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use crate::analytics::AnalyticsSnapshot;
use crate::config::app_config::Brand;
use crate::ui::theme::Theme;
use crate::ui::widgets::common::{metric_line_within, two_columns};
use crate::ui::widgets::heatmap::HeatmapWidget;
use crate::utils::formatting::{format_price_summary, format_tokens};

/// `57/94`: active days out of the days since first use, or the bare count when the
/// span is unknown.
fn active_days_value(active_days: usize, span: usize) -> String {
    if span > 0 {
        format!("{active_days}/{span}")
    } else {
        active_days.to_string()
    }
}

pub fn render_overview(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &AnalyticsSnapshot,
    brand: &Brand,
    theme: &Theme,
) {
    let [heatmap, legend, spacer, favorite, stats, fun] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(6),
            Constraint::Length(1),
        ])
        .areas(area);

    frame.render_widget(
        HeatmapWidget {
            data: &snapshot.heatmap,
            theme,
        },
        heatmap,
    );

    frame.render_widget(
        Paragraph::new(crate::ui::widgets::heatmap::heatmap_legend_line(theme)),
        legend,
    );
    frame.render_widget(Paragraph::new(""), spacer);

    // A configured tagline replaces the header line; otherwise the favourite model
    // shows exactly as before.
    let header = match brand.tagline.as_deref() {
        Some(tagline) => Line::from(Span::styled(tagline.to_string(), theme.accent_style())),
        None => {
            let favorite_model = snapshot
                .models
                .first()
                .map(|row| row.id.clone())
                .unwrap_or_else(|| "n/a".to_string());
            Line::from(vec![
                Span::styled("Favorite model: ", theme.muted_style()),
                Span::styled(favorite_model, theme.accent_style()),
            ])
        }
    };
    frame.render_widget(Paragraph::new(header), favorite);

    let [left, right] = two_columns(stats);
    let (left_width, right_width) = (left.width as usize, right.width as usize);

    let left_text = Text::from(vec![
        metric_line_within(
            "Total tokens: ",
            format_tokens(snapshot.overview.total_tokens),
            left_width,
            theme,
        ),
        metric_line_within(
            "Input: ",
            format_tokens(snapshot.overview.input_tokens),
            left_width,
            theme,
        ),
        metric_line_within(
            "Output: ",
            format_tokens(snapshot.overview.output_tokens),
            left_width,
            theme,
        ),
        metric_line_within(
            "Cache: ",
            format_tokens(snapshot.overview.cache_tokens),
            left_width,
            theme,
        ),
        metric_line_within(
            "Models used: ",
            snapshot.overview.models_used.to_string(),
            left_width,
            theme,
        ),
        metric_line_within(
            "Active days: ",
            active_days_value(
                snapshot.overview.active_days,
                snapshot.overview.active_days_span,
            ),
            left_width,
            theme,
        ),
    ]);
    frame.render_widget(Paragraph::new(left_text), left);

    let right_text = Text::from(vec![
        metric_line_within(
            "Total cost: ",
            format_price_summary(&snapshot.overview.total_cost),
            right_width,
            theme,
        ),
        metric_line_within(
            "Sessions: ",
            snapshot.overview.sessions.to_string(),
            right_width,
            theme,
        ),
        metric_line_within(
            "Messages: ",
            snapshot.overview.messages.to_string(),
            right_width,
            theme,
        ),
        metric_line_within(
            "Prompts: ",
            snapshot.overview.prompts.to_string(),
            right_width,
            theme,
        ),
        metric_line_within(
            "Current streak: ",
            snapshot.overview.current_streak.to_string(),
            right_width,
            theme,
        ),
        metric_line_within(
            "Longest streak: ",
            snapshot.overview.longest_streak.to_string(),
            right_width,
            theme,
        ),
    ]);
    frame.render_widget(Paragraph::new(right_text), right);

    // With the comparison off the row stays blank, so the layout below never moves.
    if brand.show_comparison {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                &snapshot.overview.fun_comparison,
                theme.comparison_style(),
            ))),
            fun,
        );
    } else {
        frame.render_widget(Paragraph::new(""), fun);
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::theme::Theme;
    use crate::utils::pricing::PriceSummary;

    #[test]
    fn the_overview_shows_both_streaks_next_to_the_active_days() {
        use crate::analytics::heatmap_data::HeatmapData;
        use crate::analytics::model_stats::ModelChartData;
        use crate::analytics::{AnalyticsSnapshot, OverviewStats};
        use crate::config::app_config::Brand;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let snapshot = AnalyticsSnapshot {
            overview: OverviewStats {
                total_tokens: 0,
                input_tokens: 0,
                output_tokens: 0,
                cache_tokens: 0,
                total_cost: PriceSummary::default(),
                sessions: 0,
                messages: 0,
                prompts: 0,
                models_used: 0,
                active_days: 7,
                active_days_span: 0,
                current_streak: 2,
                longest_streak: 5,
                fun_comparison: String::new(),
            },
            models: Vec::new(),
            chart: ModelChartData {
                x_bounds: [0.0, 1.0],
                y_bounds: [0.0, 1.0],
                x_labels: Vec::new(),
                series: Vec::new(),
            },
            providers: Vec::new(),
            provider_chart: ModelChartData {
                x_bounds: [0.0, 1.0],
                y_bounds: [0.0, 1.0],
                x_labels: Vec::new(),
                series: Vec::new(),
            },
            sessions: Vec::new(),
            heatmap: HeatmapData {
                weeks: Vec::new(),
                month_labels: Vec::new(),
            },
        };

        let theme = Theme::builtin_dark();
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|frame| {
                super::render_overview(frame, frame.area(), &snapshot, &Brand::default(), &theme);
            })
            .unwrap();

        let buffer = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                if let Some(cell) = buffer.cell((x, y)) {
                    text.push_str(cell.symbol());
                }
            }
        }
        for expected in ["Active days: 7", "Current streak: 2", "Longest streak: 5"] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
    }

    #[test]
    fn active_days_read_as_a_share_of_the_days_since_first_use() {
        assert_eq!(super::active_days_value(57, 94), "57/94");
        // No usage means no span: the bare count, never a misleading "0/0".
        assert_eq!(super::active_days_value(57, 0), "57");
        assert_eq!(super::active_days_value(0, 0), "0");
    }
}
