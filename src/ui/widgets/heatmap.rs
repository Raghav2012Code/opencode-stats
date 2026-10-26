use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::analytics::heatmap_data::{HEAT_LEVELS, HeatmapData, weekday_labels};
use crate::ui::theme::Theme;

/// One glyph per level, from no usage to the busiest day. The ink grows with the level,
/// so the ramp still reads when the colours are close together.
/// Every glyph fills its whole cell, so neighbouring cells join without gaps or
/// ragged edges and an active stretch reads as one solid block.
const LEVEL_GLYPHS: [char; HEAT_LEVELS] = ['·', '░', '▒', '▓', '▉', '█'];

pub struct HeatmapWidget<'a> {
    pub data: &'a HeatmapData,
    pub theme: &'a Theme,
}

const ROW_LABEL_WIDTH: usize = 4;

impl Widget for HeatmapWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut lines = Vec::new();

        // When the area is narrower than the year, the oldest weeks go, not the newest: the
        // most recent days are the ones a glance is for.
        let shown_weeks = (area.width as usize).saturating_sub(ROW_LABEL_WIDTH);
        let skip = self.data.weeks.len().saturating_sub(shown_weeks);

        lines.push(Line::styled(
            build_month_header(self.data, area.width as usize, skip),
            self.theme.muted_style(),
        ));

        for (row_index, weekday) in weekday_labels().iter().enumerate() {
            let name = weekday_label(row_index, weekday);
            let mut spans = vec![Span::styled(name, self.theme.muted_style())];
            for week in self.data.weeks.iter().skip(skip) {
                let cell = &week[row_index];
                let (symbol, style) = if cell.is_future {
                    (' ', Style::default())
                } else {
                    let level = (cell.intensity as usize).min(HEAT_LEVELS - 1);
                    (
                        LEVEL_GLYPHS[level],
                        Style::default().fg(self.theme.heat_level(level as u8)),
                    )
                };
                spans.push(Span::styled(symbol.to_string(), style));
            }
            lines.push(Line::from(spans));
        }

        Paragraph::new(lines).render(area, buf);
    }
}

pub fn heatmap_legend_line(theme: &Theme) -> Line<'static> {
    let mut spans = vec![Span::styled("    Less ", theme.muted_style())];
    for (index, glyph) in LEVEL_GLYPHS.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            glyph.to_string(),
            Style::default().fg(theme.heat_level(index as u8)),
        ));
    }
    spans.push(Span::styled(" More", theme.muted_style()));

    Line::from(spans)
}

/// The month names over the drawn weeks. `skip` leading weeks are not drawn, and the
/// labels that belong to them are not either.
fn build_month_header(data: &HeatmapData, max_width: usize, skip: usize) -> String {
    let width = max_width.min(ROW_LABEL_WIDTH + data.weeks.len().saturating_sub(skip));
    let mut header = vec![' '; width];
    let mut next_free_column = ROW_LABEL_WIDTH;

    for (week_index, label) in &data.month_labels {
        let Some(drawn_index) = week_index.checked_sub(skip) else {
            continue;
        };
        let start = ROW_LABEL_WIDTH + drawn_index;
        if start < next_free_column || start >= width {
            continue;
        }

        // A label that would run past the visible width is dropped whole: a cut-off
        // "Sep" reading as "S" looks like a missing month, while a missing label is
        // just a quieter header.
        if start + label.chars().count() > width {
            continue;
        }

        for (offset, ch) in label.chars().enumerate() {
            let column = start + offset;
            if column >= width {
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
    use crate::analytics::heatmap_data::{HEAT_LEVELS, HeatmapCell, HeatmapData};
    use crate::ui::theme::Theme;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use ratatui::widgets::Paragraph;

    /// One column per level, so every level appears in the rendered grid.
    fn data_with_every_level() -> HeatmapData {
        let weeks = (0..HEAT_LEVELS as u8)
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
            for x in ROW_LABEL_WIDTH as u16..(ROW_LABEL_WIDTH + HEAT_LEVELS) as u16 {
                if let Some(cell) = buffer.cell((x, y)) {
                    grid.push(cell.fg);
                }
            }
        }
        for x in 0..40u16 {
            if let Some(cell) = buffer.cell((x, 9))
                && matches!(cell.symbol(), "·" | "░" | "▒" | "▓" | "▉" | "█")
            {
                legend.push(cell.fg);
            }
        }

        (distinct(grid), distinct(legend))
    }

    fn header_row(data: &HeatmapData, theme: &Theme, width: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 10)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(HeatmapWidget { data, theme }, Rect::new(0, 0, width, 10));
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.width)
            .map(|x| buffer.cell((x, 0)).map(|cell| cell.symbol()).unwrap_or(" "))
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    fn full_year_labels() -> HeatmapData {
        let weeks = (0..53)
            .map(|_| {
                (0..7)
                    .map(|_| HeatmapCell {
                        intensity: 1,
                        is_future: false,
                    })
                    .collect()
            })
            .collect();
        // The real layout for a year ending 20 Oct 2026: twelve labels, unevenly spaced.
        let months = [
            (2, "Nov"),
            (6, "Dec"),
            (11, "Jan"),
            (15, "Feb"),
            (19, "Mar"),
            (24, "Apr"),
            (28, "May"),
            (32, "Jun"),
            (37, "Jul"),
            (41, "Aug"),
            (46, "Sep"),
            (50, "Oct"),
        ];
        HeatmapData {
            weeks,
            month_labels: months
                .into_iter()
                .map(|(week, label)| (week, label.to_string()))
                .collect(),
        }
    }

    #[test]
    fn the_month_header_shows_every_label_whole_at_full_width() {
        let theme = Theme::builtin_dark();
        let row = header_row(&full_year_labels(), &theme, 70);

        for label in [
            "Nov", "Dec", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct",
        ] {
            assert!(row.contains(label), "missing {label} in {row:?}");
        }
    }

    #[test]
    fn a_narrow_header_drops_labels_whole_instead_of_cutting_them() {
        let theme = Theme::builtin_dark();
        let data = full_year_labels();
        let row = header_row(&data, &theme, 50);

        assert!(row.contains("Aug"), "{row:?}");
        assert!(
            !row.contains('S') || row.contains("Sep"),
            "no cut-off label: {row:?}"
        );
        for label in [
            "Nov", "Dec", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct",
        ] {
            let count = row.matches(label).count();
            assert!(count <= 1, "{label} appears cut or doubled: {row:?}");
            if count == 1 {
                let at = row.find(label).unwrap();
                assert!(
                    at + label.len() <= 50,
                    "{label} runs past the visible width: {row:?}"
                );
            }
        }
    }
    #[test]
    fn a_narrow_heatmap_keeps_the_newest_weeks_on_screen() {
        // Twenty weeks, the last one busy. At 14 columns only the last ten weeks fit, so
        // the busy week has to be the rightmost cell of the Monday row.
        let quiet = || {
            (0..7)
                .map(|_| HeatmapCell {
                    intensity: 0,
                    is_future: false,
                })
                .collect::<Vec<_>>()
        };
        let mut weeks = (0..20).map(|_| quiet()).collect::<Vec<_>>();
        weeks[19][0].intensity = 5;
        let data = HeatmapData {
            weeks,
            month_labels: Vec::new(),
        };

        let theme = Theme::builtin_dark();
        let mut terminal = Terminal::new(TestBackend::new(14, 10)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    HeatmapWidget {
                        data: &data,
                        theme: &theme,
                    },
                    Rect::new(0, 0, 14, 10),
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let monday: String = (0..14)
            .map(|x| {
                buffer
                    .cell((x, 1))
                    .map(|cell| cell.symbol().to_string())
                    .unwrap_or_default()
            })
            .collect();

        assert!(
            monday.ends_with('█'),
            "the newest week is cut off: {monday:?}"
        );
    }

    #[test]
    fn every_heat_level_renders_a_distinct_colour() {
        for theme in [Theme::builtin_dark(), Theme::builtin_light()] {
            let (grid, _) = rendered_heat_colours(&theme);
            let expected = (0..HEAT_LEVELS as u8)
                .map(|level| theme.heat_level(level))
                .collect::<Vec<_>>();

            assert_eq!(
                grid.len(),
                expected.len(),
                "each of the levels must have its own colour, got {grid:?}"
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
            let expected = (0..HEAT_LEVELS as u8)
                .map(|level| theme.heat_level(level))
                .collect::<Vec<_>>();

            assert_eq!(
                legend.len(),
                expected.len(),
                "the legend must show every level once, got {legend:?}"
            );
            for color in &expected {
                assert!(legend.contains(color), "legend is missing {color:?}");
                assert!(grid.contains(color), "cells are missing {color:?}");
            }
        }
    }

    #[test]
    fn the_ramp_grows_ink_with_every_level() {
        // The glyphs are the level as well as the colour: each one covers more of the
        // cell than the one before it, so the ramp reads without colour.
        let covered: Vec<u32> = super::LEVEL_GLYPHS
            .iter()
            .map(|glyph| match glyph {
                '·' => 0,
                '░' => 1,
                '▒' => 2,
                '▓' => 3,
                '▉' => 4,
                '█' => 5,
                other => panic!("unexpected level glyph {other:?}"),
            })
            .collect();

        assert_eq!(covered.len(), HEAT_LEVELS);
        for pair in covered.windows(2) {
            assert!(pair[1] > pair[0], "the ramp must keep growing: {covered:?}");
        }
    }

    #[test]
    fn an_all_levels_row_occupies_exactly_one_cell_per_week() {
        // Every level glyph is one column wide: a row of all six levels takes the
        // label plus six cells, with nothing wrapping onto the next line and no
        // column pushed out of place. Lower blocks join raggedly and ambiguous
        // widths stagger the grid on CJK-font terminals, which is what this pins.
        let theme = Theme::builtin_dark();
        let data = data_with_every_level();
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    HeatmapWidget {
                        data: &data,
                        theme: &theme,
                    },
                    Rect::new(0, 0, 40, 10),
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let row: String = (0..buffer.area.width)
            .map(|x| {
                buffer
                    .cell((x, 1))
                    .map(|cell| cell.symbol().to_string())
                    .unwrap_or_default()
            })
            .collect();
        let row = row.trim_end().to_string();

        let mut expected = "Mon ".to_string();
        expected.extend(super::LEVEL_GLYPHS.iter());
        assert_eq!(row, expected, "one cell per week, no wrap: {row:?}");
    }
}
