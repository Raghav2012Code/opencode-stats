//! The Sessions page: the session table and its detail pane.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::analytics::model_stats::SessionUsageRow;
use crate::ui::models::{PageContext, SearchState, render_list_page, render_metric_grid};
use crate::ui::theme::Theme;
use crate::ui::widgets::common::truncate_label;
use crate::utils::formatting::{format_duration, format_price_summary, format_tokens};

pub fn render_sessions(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    rows: &[SessionUsageRow],
    focused_index: usize,
    search: Option<&SearchState>,
    theme: &Theme,
) {
    render_list_page(
        frame,
        area,
        PageContext {
            rows,
            noun: "session",
            focused_index,
            search,
        },
        theme,
        |frame, body, focus| render_session_table(frame, body, rows, focus, theme),
        |frame, area, row| render_session_detail(frame, area, row, theme),
    );
}

fn render_session_table(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    rows: &[SessionUsageRow],
    focused: Option<usize>,
    theme: &Theme,
) {
    let height = area.height as usize;
    if height == 0 {
        return;
    }

    // Keep the focused row on screen: scroll just far enough to reach it, and stop at
    // the end so the last page stays full. Drawing rows past the area would paint them
    // over the detail pane below, which reads as the list running off the bottom.
    let visible_start = focused
        .map(|index| index.saturating_sub(height - 1))
        .unwrap_or(0)
        .min(rows.len().saturating_sub(height));

    for (offset, row) in rows.iter().enumerate().skip(visible_start).take(height) {
        let marker = if Some(offset) == focused { "●" } else { " " };
        let style = if Some(offset) == focused {
            theme.accent_style()
        } else {
            theme.muted_style()
        };
        let subagent = if row.is_subagent { "◈" } else { " " };
        let paragraph = Paragraph::new(Line::from(vec![
            Span::styled(marker.to_string(), style),
            Span::raw(format!(
                " {:>3}  {:<30}  {:>8}  {:>8}  {:>4} {}",
                offset + 1,
                truncate_label(&row.title, 30),
                format_price_summary(&row.cost),
                format_tokens(row.total_tokens),
                row.messages,
                subagent,
            )),
        ]));

        frame.render_widget(
            paragraph,
            Rect {
                x: area.x,
                y: area.y + (offset - visible_start) as u16,
                width: area.width,
                height: 1,
            },
        );
    }
}

fn render_session_detail(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    row: &SessionUsageRow,
    theme: &Theme,
) {
    render_metric_grid(
        frame,
        area,
        &[
            ("Total tokens: ", format_tokens(row.total_tokens)),
            ("Total cost: ", format_price_summary(&row.cost)),
            ("Input: ", format_tokens(row.input_tokens)),
            ("Output: ", format_tokens(row.output_tokens)),
            ("Cache: ", format_tokens(row.cache_tokens)),
            ("Messages: ", row.messages.to_string()),
            ("Prompts: ", row.prompts.to_string()),
            (
                "Elapsed: ",
                row.elapsed_secs()
                    .map(format_duration)
                    .unwrap_or_else(|| "—".to_string()),
            ),
            ("Project: ", row.project.clone()),
            ("Active days: ", row.active_days.to_string()),
        ],
        theme,
    );
}

#[cfg(test)]
mod tests {
    use super::render_sessions;
    use crate::analytics::model_stats::SessionUsageRow;
    use crate::ui::theme::Theme;
    use crate::utils::pricing::PriceSummary;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn session_row_fixture(id: &str, title: &str, subagent: bool) -> SessionUsageRow {
        SessionUsageRow {
            id: id.to_string(),
            title: title.to_string(),
            project: "demo".to_string(),
            is_subagent: subagent,
            total_tokens: 42,
            input_tokens: 21,
            output_tokens: 11,
            cache_tokens: 10,
            messages: 3,
            prompts: 2,
            active_days: 1,
            cost: PriceSummary::default(),
            first_activity: None,
            last_activity: None,
            percentage: 100.0,
        }
    }

    fn sessions_text(rows: Vec<SessionUsageRow>) -> String {
        let theme = Theme::builtin_dark();
        let mut terminal = Terminal::new(TestBackend::new(70, 16)).unwrap();
        terminal
            .draw(|frame| {
                render_sessions(frame, frame.area(), &rows, 0, None, &theme);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                if let Some(cell) = buffer.cell((x, y)) {
                    text.push_str(cell.symbol());
                }
            }
        }
        text
    }

    fn sessions_lines(rows: &[SessionUsageRow], focused: usize, height: u16) -> Vec<String> {
        let theme = Theme::builtin_dark();
        let mut terminal = Terminal::new(TestBackend::new(70, height)).unwrap();
        terminal
            .draw(|frame| {
                render_sessions(frame, frame.area(), rows, focused, None, &theme);
            })
            .unwrap();

        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer.cell((x, y)).map(|cell| cell.symbol()).unwrap_or(" "))
                    .collect::<String>()
            })
            .collect()
    }

    fn many_session_rows(count: usize) -> Vec<SessionUsageRow> {
        (1..=count)
            .map(|n| session_row_fixture(&format!("ses_{n}"), &format!("row-{n:02}"), false))
            .collect()
    }

    #[test]
    fn the_focus_line_names_the_focused_session() {
        let lines = sessions_lines(&many_session_rows(2), 1, 16);
        let focus = lines
            .iter()
            .find(|line| line.contains("/2"))
            .unwrap_or_else(|| panic!("no focus line: {lines:#?}"));

        assert!(focus.contains("row-02"), "{focus:?}");
        assert!(focus.contains("2/2"), "{focus:?}");
        assert!(focus.contains("j/k"), "{focus:?}");
        assert!(focus.contains("f find"), "{focus:?}");
    }

    #[test]
    fn the_detail_pane_reports_the_focused_sessions_numbers() {
        let text = sessions_text(vec![session_row_fixture("ses_1", "Alpha", true)]);
        for expected in [
            "Total tokens: 42",
            "Input: 21",
            "Output: 11",
            "Cache: 10",
            "Messages: 3",
            "Prompts: 2",
            "Project: demo",
            "Active days: 1",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
    }

    #[test]
    fn the_session_list_scrolls_to_keep_the_focused_row_on_screen() {
        // Height 20 leaves an 11-row table: the layout reserves 9 lines for the focus
        // line and the detail pane. Anything drawn below that is painted over them.
        let rows = many_session_rows(20);
        let height = 20_u16;
        let table_height = height as usize - 9;

        let lines = sessions_lines(&rows, 0, height);
        assert!(
            lines.iter().any(|line| line.contains("row-01")),
            "the first row is visible at the top: {lines:#?}"
        );

        // Focus below the last visible row: the window has to follow it.
        let lines = sessions_lines(&rows, 15, height);
        let position = lines
            .iter()
            .position(|line| line.contains("row-16"))
            .unwrap_or_else(|| panic!("the focused row must be on screen: {lines:#?}"));
        assert!(
            lines[position].contains('●'),
            "the marker sits on the focused row: {:?}",
            lines[position]
        );
        assert!(
            position < table_height,
            "the focused row belongs inside the table area, not over the detail pane: \
             line {position} of {table_height}"
        );
        assert!(
            lines.iter().any(|line| line.contains("row-06")),
            "the window scrolled with the focus: {lines:#?}"
        );

        // Focus on the last row: the window stops at the end, with no blank rows.
        let lines = sessions_lines(&rows, 19, height);
        let position = lines
            .iter()
            .position(|line| line.contains("row-20"))
            .unwrap_or_else(|| panic!("the last row must be on screen: {lines:#?}"));
        assert!(
            lines[position].contains('●'),
            "the marker follows to the end: {:?}",
            lines[position]
        );
        assert!(
            position < table_height,
            "the last row lands on the last table line, not below it: line {position}"
        );
        assert!(lines.iter().any(|line| line.contains("row-10")));
        assert!(
            !lines.iter().any(|line| line.contains("row-09")),
            "the window is clamped, not scrolled past the end: {lines:#?}"
        );
    }

    #[test]
    fn renders_session_titles_costs_and_the_subagent_marker() {
        let text = sessions_text(vec![
            session_row_fixture("ses_big", "Big job", true),
            session_row_fixture("ses_small", "Small chat", false),
        ]);

        assert!(text.contains("Big job"));
        assert!(text.contains("Small chat"));
        assert!(text.contains("◈"));
        assert!(text.contains("Total tokens: "));
        assert!(text.contains("Elapsed: "));
        assert!(text.contains("Project: "));
    }

    #[test]
    fn renders_the_sessions_empty_state() {
        assert!(sessions_text(Vec::new()).contains("No session activity in this time range."));
    }
}
