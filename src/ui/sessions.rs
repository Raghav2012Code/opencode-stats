//! The Sessions page: the session table and its detail pane.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::analytics::model_stats::SessionUsageRow;
use crate::ui::models::{PageContext, SearchState, render_list_page, render_metric_grid};
use crate::ui::theme::Theme;
use crate::ui::widgets::common::{pad_right, truncate_label};
use crate::utils::formatting::{format_duration, format_price_summary, format_tokens};

pub fn render_sessions(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    rows: &[SessionUsageRow],
    focused_index: usize,
    search: Option<&SearchState>,
    project: Option<&str>,
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
            empty_note: project,
        },
        theme,
        |frame, body, focus| render_session_table(frame, body, rows, focus, theme),
        |frame, area, row| render_session_detail(frame, area, row, theme),
    );
}

/// Widths of the table's columns. Each figure column is as wide as its widest value over
/// every session, so the list keeps its shape while you scroll; the title takes what is left.
struct SessionColumns {
    title: usize,
    cost: usize,
    tokens: usize,
    messages: usize,
}

/// Every column's width except the title: marker and gap, the number, the gaps between
/// columns, and the sub-agent flag. See [`session_row`] for the exact layout.
const FIXED_COLUMNS: usize = 17;
const TITLE_MIN: usize = 4;
const TITLE_MAX: usize = 30;

fn session_columns(rows: &[SessionUsageRow], width: usize) -> SessionColumns {
    let widest = |values: Vec<String>, heading: &str| {
        values
            .iter()
            .map(|value| UnicodeWidthStr::width(value.as_str()))
            .chain(std::iter::once(heading.len()))
            .max()
            .unwrap_or(0)
    };
    let cost = widest(
        rows.iter()
            .map(|row| format_price_summary(&row.cost))
            .collect(),
        "Cost",
    );
    let tokens = widest(
        rows.iter()
            .map(|row| format_tokens(row.total_tokens))
            .collect(),
        "Tokens",
    );
    let messages = widest(
        rows.iter().map(|row| row.messages.to_string()).collect(),
        "Msgs",
    );
    let title = width
        .saturating_sub(FIXED_COLUMNS + cost + tokens + messages)
        .clamp(TITLE_MIN, TITLE_MAX);
    SessionColumns {
        title,
        cost,
        tokens,
        messages,
    }
}

/// The column names over the rows: the same layout as [`session_row`].
fn session_header(columns: &SessionColumns) -> String {
    format!(
        " {:>3}  {}  {:>cost$}  {:>tokens$}  {:>messages$} {:<3}",
        "#",
        pad_right("Title", columns.title),
        "Cost",
        "Tokens",
        "Msgs",
        "Sub",
        cost = columns.cost,
        tokens = columns.tokens,
        messages = columns.messages,
    )
}

/// One session: the number, the title, then cost, tokens and messages, and a sub-agent flag.
fn session_row(columns: &SessionColumns, number: usize, row: &SessionUsageRow) -> String {
    let subagent = if row.is_subagent { "◈" } else { "" };
    format!(
        " {:>3}  {}  {:>cost$}  {:>tokens$}  {:>messages$} {:<3}",
        number,
        pad_right(&truncate_label(&row.title, columns.title), columns.title),
        format_price_summary(&row.cost),
        format_tokens(row.total_tokens),
        row.messages,
        subagent,
        cost = columns.cost,
        tokens = columns.tokens,
        messages = columns.messages,
    )
}

fn render_session_table(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    rows: &[SessionUsageRow],
    focused: Option<usize>,
    theme: &Theme,
) {
    let [header_area, list_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    let columns = session_columns(rows, area.width as usize);
    // The header gets the same one-column marker gap as the rows beneath it.
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {}", session_header(&columns)),
            theme.muted_style(),
        ))),
        header_area,
    );

    let height = list_area.height as usize;
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
        let paragraph = Paragraph::new(Line::from(vec![
            Span::styled(marker.to_string(), style),
            Span::raw(session_row(&columns, offset + 1, row)),
        ]));

        frame.render_widget(
            paragraph,
            Rect {
                x: list_area.x,
                y: list_area.y + (offset - visible_start) as u16,
                width: list_area.width,
                height: 1,
            },
        );
    }
}

/// Left column: token figures. Right column: cost and counts. Same rule as the Models page.
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
            ("Messages: ", row.messages.to_string()),
            ("Output: ", format_tokens(row.output_tokens)),
            ("Prompts: ", row.prompts.to_string()),
            ("Cache: ", format_tokens(row.cache_tokens)),
            (
                "Elapsed: ",
                row.elapsed_secs()
                    .map(format_duration)
                    .unwrap_or_else(|| "—".to_string()),
            ),
            ("Project: ", row.project.clone()),
            (
                "Active days: ",
                format!("{} · streak {}", row.active_days, row.longest_streak),
            ),
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
            longest_streak: 1,
            cost: PriceSummary::default(),
            first_activity: None,
            last_activity: None,
            percentage: 100.0,
        }
    }

    fn sessions_text(rows: Vec<SessionUsageRow>, project: Option<&str>) -> String {
        let theme = Theme::builtin_dark();
        let mut terminal = Terminal::new(TestBackend::new(70, 16)).unwrap();
        terminal
            .draw(|frame| {
                render_sessions(frame, frame.area(), &rows, 0, None, project, &theme);
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

    fn sessions_lines(
        rows: &[SessionUsageRow],
        focused: usize,
        height: u16,
        project: Option<&str>,
    ) -> Vec<String> {
        let theme = Theme::builtin_dark();
        let mut terminal = Terminal::new(TestBackend::new(70, height)).unwrap();
        terminal
            .draw(|frame| {
                render_sessions(frame, frame.area(), rows, focused, None, project, &theme);
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
        let lines = sessions_lines(&many_session_rows(2), 1, 16, None);
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
        let text = sessions_text(vec![session_row_fixture("ses_1", "Alpha", true)], None);
        for expected in [
            "Total tokens: 42",
            "Input: 21",
            "Output: 11",
            "Cache: 10",
            "Messages: 3",
            "Prompts: 2",
            "Project: demo",
            "Active days: 1 · streak 1",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
    }

    #[test]
    fn the_session_list_scrolls_to_keep_the_focused_row_on_screen() {
        // Height 20 leaves an 11-line table: the layout reserves 9 lines for the focus
        // line and the detail pane. Anything drawn below that is painted over them. The
        // table's first line is the column header, so 10 sessions fit beneath it.
        let rows = many_session_rows(20);
        let height = 20_u16;
        let table_height = height as usize - 9;

        let lines = sessions_lines(&rows, 0, height, None);
        assert!(
            lines.iter().any(|line| line.contains("row-01")),
            "the first row is visible at the top: {lines:#?}"
        );

        // Focus below the last visible row: the window has to follow it.
        let lines = sessions_lines(&rows, 15, height, None);
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
        // The header takes the table's first line, so one fewer session fits: the window
        // starts at row 7 and row 6 has scrolled off the top.
        assert!(
            lines.iter().any(|line| line.contains("row-07")),
            "the window scrolled with the focus: {lines:#?}"
        );
        assert!(
            !lines.iter().any(|line| line.contains("row-06")),
            "the window scrolled past the first row shown: {lines:#?}"
        );

        // Focus on the last row: the window stops at the end, with no blank rows.
        let lines = sessions_lines(&rows, 19, height, None);
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
        assert!(lines.iter().any(|line| line.contains("row-11")));
        assert!(
            !lines.iter().any(|line| line.contains("row-10")),
            "the window is clamped, not scrolled past the end: {lines:#?}"
        );
    }

    #[test]
    fn renders_session_titles_costs_and_the_subagent_marker() {
        let text = sessions_text(
            vec![
                session_row_fixture("ses_big", "Big job", true),
                session_row_fixture("ses_small", "Small chat", false),
            ],
            None,
        );

        assert!(text.contains("Big job"));
        assert!(text.contains("Small chat"));
        assert!(text.contains("◈"));
        assert!(text.contains("Total tokens: "));
        assert!(text.contains("Elapsed: "));
        assert!(text.contains("Project: "));
    }

    /// The frame as rows of terminal cells, as the terminal holds them. A wide character
    /// occupies one cell with the next cell empty, so the cells are the only faithful view
    /// of where things sit.
    fn sessions_cells(rows: &[SessionUsageRow], width: u16, height: u16) -> Vec<Vec<String>> {
        let theme = Theme::builtin_dark();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                render_sessions(frame, frame.area(), rows, 0, None, None, &theme);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| {
                        buffer
                            .cell((x, y))
                            .map(|cell| cell.symbol().to_string())
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .collect()
    }

    fn text_of(cells: &[String]) -> String {
        cells.concat()
    }

    /// The first line of cells that contains `text`. The table sits above the focus line,
    /// so a session's own row is found before the focus line repeats its title.
    fn row_with<'a>(lines: &'a [Vec<String>], text: &str) -> &'a Vec<String> {
        lines
            .iter()
            .find(|cells| text_of(cells).contains(text))
            .unwrap_or_else(|| panic!("no line with {text:?}"))
    }

    /// The terminal column where the ASCII `needle` starts on a line of cells.
    fn column_of(cells: &[String], needle: &str) -> usize {
        let chars = needle.chars().map(|c| c.to_string()).collect::<Vec<_>>();
        (0..cells.len())
            .find(|&x| x + chars.len() <= cells.len() && cells[x..x + chars.len()] == chars[..])
            .unwrap_or_else(|| panic!("{needle:?} is not on {:?}", text_of(cells)))
    }

    fn priced(cents: i64) -> PriceSummary {
        PriceSummary {
            known: rust_decimal::Decimal::new(cents, 2),
            has_known: true,
            missing: false,
        }
    }

    #[test]
    fn a_wide_title_does_not_push_the_costs_out_of_line() {
        let mut plain = session_row_fixture("ses_a", "Plain title", false);
        plain.cost = priced(125);
        let mut wide = session_row_fixture("ses_b", "修复登录页面的布局问题", false);
        wide.cost = priced(125);
        let lines = sessions_cells(&[plain, wide], 70, 16);

        let plain_line = row_with(&lines, "Plain title");
        let wide_line = row_with(&lines, "修");
        assert_eq!(
            column_of(plain_line, "$1.25"),
            column_of(wide_line, "$1.25"),
            "a CJK title is two columns per character:\n{}\n{}",
            text_of(plain_line),
            text_of(wide_line)
        );
    }

    #[test]
    fn every_row_shares_the_figure_edges_whatever_the_cost_looks_like() {
        // Three costs of different shapes: partly priced, very large, and free.
        let mut partial = session_row_fixture("ses_p", "Partial", false);
        partial.cost = PriceSummary {
            known: rust_decimal::Decimal::new(123, 2),
            has_known: true,
            missing: true,
        };
        let mut large = session_row_fixture("ses_l", "Large", false);
        large.cost = priced(123_456_789);
        let free = session_row_fixture("ses_f", "Free", false);
        let lines = sessions_cells(&[partial, large, free], 70, 16);

        let partial_line = row_with(&lines, "Partial");
        let large_line = row_with(&lines, "Large");
        let free_line = row_with(&lines, "Free");
        // The right edge of a figure: where its text ends.
        let cost_edge = |line: &[String], cost: &str| column_of(line, cost) + cost.len();

        let edge = cost_edge(partial_line, "$1.23 + ?");
        assert_eq!(cost_edge(large_line, "$1234567.89"), edge, "cost edge");
        assert_eq!(cost_edge(free_line, "$0.00"), edge, "cost edge");

        // Tokens are 42 on every row, so their right edge must match too.
        let token_edge = |line: &[String]| column_of(line, "42") + 2;
        assert_eq!(token_edge(large_line), token_edge(partial_line));
        assert_eq!(token_edge(free_line), token_edge(partial_line));
    }

    #[test]
    fn the_column_names_sit_over_their_figures() {
        let lines = sessions_cells(&[session_row_fixture("ses_1", "Alpha", true)], 70, 16);
        let header = row_with(&lines, "Tokens");
        for name in ["#", "Title", "Cost", "Tokens", "Msgs", "Sub"] {
            assert!(
                text_of(header).contains(name),
                "missing column {name:?} in {}",
                text_of(header)
            );
        }
        let row = row_with(&lines, "Alpha");
        assert_eq!(
            column_of(header, "Tokens") + 6,
            column_of(row, "42") + 2,
            "the Tokens figure ends under its heading:\n{}\n{}",
            text_of(header),
            text_of(row)
        );
    }

    #[test]
    fn the_detail_pairs_read_across_the_same_way_as_the_models_page() {
        let lines = sessions_lines(&[session_row_fixture("ses_1", "Alpha", false)], 0, 16, None);
        let pairs = [
            ("Total tokens: ", "Total cost: "),
            ("Input: ", "Messages: "),
            ("Output: ", "Prompts: "),
            ("Cache: ", "Elapsed: "),
            ("Project: ", "Active days: "),
        ];
        for (left, right) in pairs {
            let line = lines
                .iter()
                .find(|l| l.contains(left))
                .unwrap_or_else(|| panic!("no line with {left:?}: {lines:#?}"));
            assert!(
                line.contains(right),
                "{left:?} and {right:?} should share a line:\n{line}"
            );
        }
    }

    #[test]
    fn a_long_project_name_stays_inside_its_cell_at_a_narrow_width() {
        let mut row = session_row_fixture("ses_1", "Alpha", false);
        row.project = "an-extremely-long-project-directory-name".to_string();
        let lines = sessions_cells(&[row], 40, 16);
        let line = row_with(&lines, "Project: ");

        assert!(
            text_of(line).contains("..."),
            "the cut is marked: {}",
            text_of(line)
        );
        // At 40 columns each detail cell is (40 - 2) / 2 = 19 wide, and the right cell starts
        // after the 2-column gutter: column 21.
        assert!(
            column_of(line, "Active days: ") >= 21,
            "the right-hand pair starts after the gutter: {}",
            text_of(line)
        );
    }

    #[test]
    fn renders_the_sessions_empty_state() {
        assert!(
            sessions_text(Vec::new(), None).contains("No session activity in this time range.")
        );
        assert!(
            sessions_text(Vec::new(), Some("demo"))
                .contains("No session activity in this time range (demo).")
        );
    }
}
