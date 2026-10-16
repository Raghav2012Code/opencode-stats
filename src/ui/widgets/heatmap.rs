use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::analytics::heatmap_data::{HeatmapData, weekday_labels};
use crate::ui::theme::Theme;

pub struct HeatmapWidget<'a> {
    pub data: &'a HeatmapData,
    pub theme: &'a Theme,
}

const ROW_LABEL_WIDTH: usize = 4;

impl Widget for HeatmapWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut lines = Vec::new();

        lines.push(Line::styled(
            build_month_header(self.data),
            self.theme.muted_style(),
        ));

        for (row_index, weekday) in weekday_labels().iter().enumerate() {
            let name = weekday_label(row_index, weekday);
            let mut spans = vec![Span::styled(name, self.theme.muted_style())];
            for week in &self.data.weeks {
                let cell = &week[row_index];
                let (symbol, style) = if cell.is_future {
                    (' ', Style::default())
                } else {
                    match cell.intensity {
                        0 => ('·', Style::default().fg(self.theme.heat_level(0))),
                        1 => ('░', Style::default().fg(self.theme.heat_level(1))),
                        2 => ('▒', Style::default().fg(self.theme.heat_level(2))),
                        3 => ('▓', Style::default().fg(self.theme.heat_level(3))),
                        _ => ('█', Style::default().fg(self.theme.heat_level(4))),
                    }
                };
                spans.push(Span::styled(symbol.to_string(), style));
            }
            lines.push(Line::from(spans));
        }

        Paragraph::new(lines).render(area, buf);
    }
}

pub fn heatmap_legend_line(theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled("    Less ", theme.muted_style()),
        Span::styled("·", Style::default().fg(theme.heat_0)),
        Span::raw(" "),
        Span::styled("░", Style::default().fg(theme.heat_4)),
        Span::raw(" "),
        Span::styled("▒", Style::default().fg(theme.heat_4)),
        Span::raw(" "),
        Span::styled("▓", Style::default().fg(theme.heat_4)),
        Span::raw(" "),
        Span::styled("█", Style::default().fg(theme.heat_4)),
        Span::styled(" More", theme.muted_style()),
    ])
}

fn build_month_header(data: &HeatmapData) -> String {
    let mut header = vec![' '; ROW_LABEL_WIDTH + data.weeks.len()];
    let mut next_free_column = ROW_LABEL_WIDTH;

    for (week_index, label) in &data.month_labels {
        let start = ROW_LABEL_WIDTH + *week_index;
        if start < next_free_column || start >= header.len() {
            continue;
        }

        for (offset, ch) in label.chars().enumerate() {
            let column = start + offset;
            if column >= header.len() {
                break;
            }
            header[column] = ch;
        }

        next_free_column = start + label.chars().count() + 1;
    }

    header.into_iter().collect()
}

fn weekday_label(row_index: usize, weekday: &chrono::Weekday) -> String {
    match row_index {
        0 | 2 | 4 | 6 => format!(
            "{:>3} ",
            format!("{:?}", weekday).chars().take(3).collect::<String>()
        ),
        _ => " ".repeat(ROW_LABEL_WIDTH),
    }
}

#[cfg(test)]
mod tests {
    use super::{HeatmapWidget, ROW_LABEL_WIDTH};
    use crate::analytics::heatmap_data::{HeatmapCell, HeatmapData};
    use crate::ui::theme::Theme;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use ratatui::widgets::Paragraph;

    /// One column per level, so every level appears in the rendered grid.
    fn data_with_every_level() -> HeatmapData {
        let weeks = (0..5u8)
            .map(|level| {
                (0..7)
                    .map(|_| HeatmapCell {
                        intensity: level,
                        is_future: false,
                    })
                    .collect()
            })
            .collect();

        HeatmapData {
            weeks,
            month_labels: Vec::new(),
        }
    }

    fn distinct(colors: Vec<Color>) -> Vec<Color> {
        let mut seen = Vec::new();
        for color in colors {
            if !seen.contains(&color) {
                seen.push(color);
            }
        }
        seen
    }

    /// Renders the grid and the legend, and returns the colours of each.
    fn rendered_heat_colours(theme: &Theme) -> (Vec<Color>, Vec<Color>) {
        let data = data_with_every_level();
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    HeatmapWidget { data: &data, theme },
                    Rect::new(0, 0, 40, 10),
                );
                frame.render_widget(
                    Paragraph::new(super::heatmap_legend_line(theme)),
                    Rect::new(0, 9, 40, 1),
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let mut grid = Vec::new();
        let mut legend = Vec::new();

        for y in 1..8u16 {
            for x in ROW_LABEL_WIDTH as u16..(ROW_LABEL_WIDTH + 5) as u16 {
                if let Some(cell) = buffer.cell((x, y)) {
                    grid.push(cell.fg);
                }
            }
        }
        for x in 0..40u16 {
            if let Some(cell) = buffer.cell((x, 9))
                && matches!(cell.symbol(), "·" | "░" | "▒" | "▓" | "█")
            {
                legend.push(cell.fg);
            }
        }

        (distinct(grid), distinct(legend))
    }

    #[test]
    fn every_heat_level_renders_a_distinct_colour() {
        for theme in [Theme::builtin_dark(), Theme::builtin_light()] {
            let (grid, _) = rendered_heat_colours(&theme);
            let expected = (0..=4)
                .map(|level| theme.heat_level(level))
                .collect::<Vec<_>>();

            assert_eq!(
                grid.len(),
                expected.len(),
                "each of the five levels must have its own colour, got {grid:?}"
            );
            for color in &expected {
                assert!(
                    grid.contains(color),
                    "{color:?} is missing from the rendered grid {grid:?}"
                );
            }
        }
    }

    #[test]
    fn the_legend_shows_the_same_colours_as_the_cells() {
        for theme in [Theme::builtin_dark(), Theme::builtin_light()] {
            let (grid, legend) = rendered_heat_colours(&theme);

            assert!(!legend.is_empty(), "the legend rendered no swatches");
            for color in &legend {
                assert!(
                    grid.contains(color),
                    "legend colour {color:?} is not one of the cell colours {grid:?}"
                );
            }
        }
    }
}
