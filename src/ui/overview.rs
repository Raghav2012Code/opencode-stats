use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use crate::analytics::AnalyticsSnapshot;
use crate::analytics::BudgetState;
use crate::config::app_config::Brand;
use crate::ui::theme::Theme;
use crate::ui::widgets::common::metric_line;
use crate::ui::widgets::heatmap::HeatmapWidget;
use crate::utils::formatting::{format_price_summary, format_tokens, format_usd_precise};

pub fn render_overview(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &AnalyticsSnapshot,
    budget: Option<&BudgetState>,
    brand: &Brand,
    theme: &Theme,
) {
    let [heatmap, legend, spacer, favorite, stats, _, fun] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(6),
            Constraint::Length(1),
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

    // The budget line takes the spacer row, so the layout below never moves and a
    // dashboard without a budget draws exactly as before.
    if let Some(budget) = budget {
        frame.render_widget(Paragraph::new(budget_line(budget, theme)), spacer);
    }

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

    let [left, right] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .areas(stats);

    let left_text = Text::from(vec![
        metric_line(
            "Total tokens: ",
            format_tokens(snapshot.overview.total_tokens),
            theme,
        ),
        metric_line(
            "Input: ",
            format_tokens(snapshot.overview.input_tokens),
            theme,
        ),
        metric_line(
            "Output: ",
            format_tokens(snapshot.overview.output_tokens),
            theme,
        ),
        metric_line(
            "Cache: ",
            format_tokens(snapshot.overview.cache_tokens),
            theme,
        ),
        metric_line(
            "Models used: ",
            snapshot.overview.models_used.to_string(),
            theme,
        ),
        metric_line(
            "Active days: ",
            snapshot.overview.active_days.to_string(),
            theme,
        ),
    ]);
    frame.render_widget(Paragraph::new(left_text), left);

    let right_text = Text::from(vec![
        metric_line(
            "Total cost: ",
            format_price_summary(&snapshot.overview.total_cost),
            theme,
        ),
        metric_line("Sessions: ", snapshot.overview.sessions.to_string(), theme),
        metric_line("Messages: ", snapshot.overview.messages.to_string(), theme),
        metric_line("Prompts: ", snapshot.overview.prompts.to_string(), theme),
        metric_line(
            "Current streak: ",
            snapshot.overview.current_streak.to_string(),
            theme,
        ),
        metric_line(
            "Longest streak: ",
            snapshot.overview.longest_streak.to_string(),
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

/// One line: what the budget window cost, the cap, and what is left. The over state is
/// the only one that raises its voice, in the accent style. The window is named, so the
/// figure is never read as the all-time total shown further down.
fn budget_line(budget: &BudgetState, theme: &Theme) -> Line<'static> {
    let spent = format_usd_precise(budget.spend.known);
    let cap = format_usd_precise(budget.cap);
    let window = crate::analytics::BUDGET_WINDOW.label().to_lowercase();
    let mut spans = vec![
        Span::styled(format!("Budget ({window}): "), theme.muted_style()),
        Span::styled(format!("{spent} of {cap}"), theme.muted_style()),
    ];

    if budget.is_over() {
        spans.push(Span::styled(
            format!(
                " · OVER by {}",
                format_usd_precise(budget.remainder().abs())
            ),
            theme.accent_style(),
        ));
    } else {
        let remainder = budget.remainder();
        let tail = if remainder.is_zero() {
            " · at cap".to_string()
        } else {
            format!(" · {} left", format_usd_precise(remainder))
        };
        spans.push(Span::styled(tail, theme.muted_style()));
    }

    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::{BudgetState, budget_line};
    use crate::ui::theme::Theme;
    use crate::utils::pricing::PriceSummary;
    use rust_decimal::Decimal;

    fn state(spent: &str, cap: &str) -> BudgetState {
        BudgetState {
            cap: cap.parse().unwrap(),
            spend: PriceSummary {
                known: spent.parse().unwrap(),
                has_known: true,
                missing: false,
            },
        }
    }

    fn text(state: &BudgetState) -> String {
        let theme = Theme::builtin_dark();
        budget_line(state, &theme)
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn states_have_the_intended_words() {
        assert_eq!(
            text(&state("3", "10")),
            "Budget (last 30 days): $3.00 of $10.00 · $7.00 left"
        );
        assert_eq!(
            text(&state("10", "10")),
            "Budget (last 30 days): $10.00 of $10.00 · at cap"
        );
        assert_eq!(
            text(&state("12", "10")),
            "Budget (last 30 days): $12.00 of $10.00 · OVER by $2.00"
        );
    }

    #[test]
    fn the_state_knows_its_own_arithmetic() {
        assert!(!state("9", "10").is_over());
        assert!(!state("10", "10").is_over());
        assert!(state("11", "10").is_over());
        assert_eq!(state("4", "10").remainder(), Decimal::new(6, 0));
    }

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
                current_streak: 2,
                longest_streak: 5,
                fun_comparison: String::new(),
            },
            models: Vec::new(),
            chart: ModelChartData {
                x_bounds: [0.0, 1.0],
                y_bounds: [0.0, 1.0],
                x_labels: Vec::new(),
                y_labels: Vec::new(),
                series: Vec::new(),
            },
            providers: Vec::new(),
            provider_chart: ModelChartData {
                x_bounds: [0.0, 1.0],
                y_bounds: [0.0, 1.0],
                x_labels: Vec::new(),
                y_labels: Vec::new(),
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
                super::render_overview(
                    frame,
                    frame.area(),
                    &snapshot,
                    None,
                    &Brand::default(),
                    &theme,
                );
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
}
