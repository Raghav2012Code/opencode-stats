use nucleo_matcher::Matcher;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::analytics::AnalyticsSnapshot;
use crate::analytics::model_stats::{SessionUsageRow, UsageRow, chart_with_focus};
use crate::ui::theme::Theme;
use crate::ui::widgets::common::{metric_line_within, truncate_label, two_columns};
use crate::ui::widgets::linechart::render_chart;
use crate::utils::formatting::{format_price_summary, format_tokens};

#[derive(Clone, Debug)]
pub struct SearchState {
    pub query: String,
    pub ids: Vec<String>,
    pub filtered_indices: Vec<usize>,
    pub selected: usize,
    pub scroll_offset: usize,
}

impl SearchState {
    pub fn new(ids: Vec<String>, focused_index: usize) -> Self {
        let total = ids.len();
        let selected = focused_index.min(total.saturating_sub(1));
        let scroll_offset = selected.saturating_sub(4);
        Self {
            query: String::new(),
            ids,
            filtered_indices: (0..total).collect(),
            selected,
            scroll_offset,
        }
    }

    pub fn update_filter(&mut self) {
        let has_query = !self.query.trim().is_empty();
        self.filtered_indices = filter_indices(&self.query, &self.ids);
        if has_query {
            self.selected = 0;
            self.scroll_offset = 0;
        } else {
            self.selected = self
                .selected
                .min(self.filtered_indices.len().saturating_sub(1));
            let filtered_total = self.filtered_indices.len();
            self.scroll_offset = self.scroll_offset.min(filtered_total.saturating_sub(5));
            if self.scroll_offset > self.selected {
                self.scroll_offset = self.selected;
            }
        }
    }
}

fn filter_indices(query: &str, items: &[String]) -> Vec<usize> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return (0..items.len()).collect();
    }

    let mut matcher = Matcher::default();
    let terms: Vec<&str> = trimmed.split_whitespace().collect();
    let mut results: Vec<(usize, u32)> = Vec::new();

    for (i, item) in items.iter().enumerate() {
        let mut char_buf = Vec::new();
        let haystack = nucleo_matcher::Utf32Str::new(item, &mut char_buf);
        let mut all_matched = true;
        let mut total_score = 0u32;

        for term in &terms {
            let pattern = Pattern::parse(term, CaseMatching::Ignore, Normalization::Smart);
            if let Some(score) = pattern.score(haystack, &mut matcher) {
                total_score += score;
            } else {
                all_matched = false;
                break;
            }
        }

        if all_matched {
            results.push((i, total_score));
        }
    }

    results.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    results.into_iter().map(|(i, _)| i).collect()
}

fn fuzzy_match_positions(query: &str, text: &str) -> Vec<usize> {
    let mut matcher = Matcher::default();
    let terms: Vec<&str> = query.split_whitespace().collect();
    let mut char_buf = Vec::new();
    let haystack = nucleo_matcher::Utf32Str::new(text, &mut char_buf);
    let mut positions: Vec<usize> = Vec::new();

    for term in &terms {
        let pattern = Pattern::parse(term, CaseMatching::Ignore, Normalization::Smart);
        let mut indices_buf = Vec::new();
        pattern.indices(haystack, &mut matcher, &mut indices_buf);
        if !indices_buf.is_empty() {
            for idx in indices_buf {
                positions.push(idx as usize);
            }
        }
    }

    positions.sort_unstable();
    positions.dedup();
    positions
}

fn merge_to_ranges(positions: &[usize]) -> Vec<(usize, usize)> {
    if positions.is_empty() {
        return vec![];
    }
    let mut ranges = Vec::new();
    let mut start = positions[0];
    let mut end = start + 1;
    for &pos in &positions[1..] {
        if pos == end {
            end = pos + 1;
        } else {
            ranges.push((start, end));
            start = pos;
            end = pos + 1;
        }
    }
    ranges.push((start, end));
    ranges
}

fn truncate_query_left(query: &str, max_width: u16) -> String {
    let full_w = UnicodeWidthStr::width(query) as u16;
    if full_w <= max_width {
        return query.to_string();
    }
    let avail = max_width.saturating_sub(1);
    let chars: Vec<char> = query.chars().collect();
    let mut w = 0u16;
    let mut i = chars.len();
    while i > 0 {
        i -= 1;
        w += UnicodeWidthChar::width(chars[i]).unwrap_or(0) as u16;
        if w > avail {
            i += 1;
            break;
        }
    }
    let mut result = String::from("…");
    result.extend(&chars[i..]);
    result
}

pub const MAX_QUERY_LEN: usize = 256;

pub fn render_models(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &AnalyticsSnapshot,
    focused_index: usize,
    search: Option<&SearchState>,
    project: Option<&str>,
    theme: &Theme,
) {
    render_list_page(
        frame,
        area,
        PageContext {
            rows: &snapshot.models,
            noun: "model",
            focused_index,
            search,
            empty_note: project,
        },
        theme,
        |frame, body, focus| {
            let focused_id = focus
                .and_then(|index| snapshot.models.get(index))
                .map(ListRow::row_id);
            let chart = chart_with_focus(&snapshot.chart, focused_id);
            render_chart(frame, body, &chart, theme);
        },
        |frame, area, row| render_detail(frame, area, row, theme),
    );
}

/// What every list page needs from a row: a stable key, a share of the total, the
/// text search matches against, and the name shown in the header.
pub(crate) trait ListRow {
    fn row_id(&self) -> &str;
    fn row_percentage(&self) -> f64;
    fn search_text(&self) -> &str;
    fn display_name(&self) -> String;
}

impl ListRow for UsageRow {
    fn row_id(&self) -> &str {
        &self.id
    }

    fn row_percentage(&self) -> f64 {
        self.percentage
    }

    fn search_text(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> String {
        self.id.clone()
    }
}

impl ListRow for SessionUsageRow {
    fn row_id(&self) -> &str {
        &self.id
    }

    fn row_percentage(&self) -> f64 {
        self.percentage
    }

    fn search_text(&self) -> &str {
        &self.title
    }

    fn display_name(&self) -> String {
        if self.is_subagent {
            format!("◈ {}", self.title)
        } else {
            self.title.clone()
        }
    }
}

/// The one list page: the layout, which row the search points at, and the choice between
/// the search overlay, the focus line with its detail pane, and the empty state. The page
/// supplies its body and its detail pane, which are the only things that differ.
/// What a list page is showing: the rows, what to call them when there are none, which one
/// is focused, and the search overlay if one is open.
pub(crate) struct PageContext<'a, R> {
    pub(crate) rows: &'a [R],
    pub(crate) noun: &'a str,
    pub(crate) focused_index: usize,
    pub(crate) search: Option<&'a SearchState>,
    /// Names the active filter in the empty state, so an empty page never reads as
    /// missing data. `None` renders the plain empty state, exactly as before.
    pub(crate) empty_note: Option<&'a str>,
}

pub(crate) fn render_list_page<R, B, D>(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    page: PageContext<'_, R>,
    theme: &Theme,
    draw_body: B,
    draw_detail: D,
) where
    R: ListRow,
    B: FnOnce(&mut ratatui::Frame<'_>, Rect, Option<usize>),
    D: FnOnce(&mut ratatui::Frame<'_>, Rect, &R),
{
    let PageContext {
        rows,
        noun,
        focused_index,
        search,
        empty_note,
    } = page;

    let [body_area, spacer1, header_area, spacer2, detail_area, _] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(5),
            Constraint::Length(1),
        ])
        .areas(area);

    let effective_focus: Option<usize> = search
        .and_then(|s| s.filtered_indices.get(s.selected).copied())
        .or(Some(focused_index));

    draw_body(frame, body_area, effective_focus);

    if let Some(search) = search {
        frame.render_widget(Paragraph::new(""), spacer1);
        render_search_overlay(
            frame,
            header_area,
            spacer2,
            detail_area,
            search,
            rows,
            theme,
        );
    } else if let Some(row) = effective_focus.and_then(|index| rows.get(index)) {
        frame.render_widget(
            Paragraph::new(focus_line(row, focused_index, rows.len(), theme)),
            header_area,
        );
        frame.render_widget(Paragraph::new(""), spacer2);
        draw_detail(frame, detail_area, row);
    } else {
        frame.render_widget(Paragraph::new(""), spacer2);
        let empty = match empty_note {
            Some(note) => format!("No {noun} activity in this time range ({note})."),
            None => format!("No {noun} activity in this time range."),
        };
        frame.render_widget(
            Paragraph::new(empty).style(theme.muted_style()),
            detail_area,
        );
    }
}

fn focus_line(
    row: &impl ListRow,
    focused_index: usize,
    total_rows: usize,
    theme: &Theme,
) -> Line<'static> {
    let total = total_rows.max(1);
    Line::from(vec![
        Span::styled(
            format!("  ● {}", truncate_label(&row.display_name(), 26)),
            Style::default().fg(theme.series_color(focused_index)),
        ),
        Span::styled(
            format!("  ({:.2}%)", row.row_percentage()),
            theme.muted_style(),
        ),
        Span::styled(" | ", theme.muted_style()),
        Span::styled(
            format!("{}/{}", focused_index.min(total - 1) + 1, total),
            theme.muted_style(),
        ),
        Span::styled(" | ", theme.muted_style()),
        Span::styled("j/k ↑/↓", theme.muted_style()),
        Span::styled(" | ", theme.muted_style()),
        Span::styled("f find", theme.muted_style()),
    ])
}

/// The one metric grid every page's detail pane uses: label and value pairs laid out
/// row-major across five rows of two columns.
pub(crate) fn render_metric_grid(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    pairs: &[(&str, String)],
    theme: &Theme,
) {
    // Five rows of two columns: pairs fill the left column of each row, then the right.
    let rows = Layout::vertical([Constraint::Length(1); 5]).areas::<5>(area);

    for (index, (label, value)) in pairs.iter().take(rows.len() * 2).enumerate() {
        let [left, right] = two_columns(rows[index / 2]);
        let cell = if index % 2 == 0 { left } else { right };
        frame.render_widget(
            Paragraph::new(metric_line_within(
                label,
                value.clone(),
                cell.width as usize,
                theme,
            )),
            cell,
        );
    }
}

fn render_detail(frame: &mut ratatui::Frame<'_>, area: Rect, row: &UsageRow, theme: &Theme) {
    render_metric_grid(
        frame,
        area,
        &[
            ("Total tokens: ", format_tokens(row.total_tokens)),
            ("Total cost: ", format_price_summary(&row.cost)),
            ("Input: ", format_tokens(row.input_tokens)),
            ("Sessions: ", row.sessions.to_string()),
            ("Output: ", format_tokens(row.output_tokens)),
            ("Messages: ", row.messages.to_string()),
            ("Cache: ", format_tokens(row.cache_tokens)),
            ("Prompts: ", row.prompts.to_string()),
            (
                "Rate: ",
                match row.p50_output_tokens_per_second {
                    Some(rate) => format!("{rate:.2} tok/s"),
                    None => "—".to_string(),
                },
            ),
            ("Active days: ", row.active_days.to_string()),
        ],
        theme,
    );
}

pub fn render_providers(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &AnalyticsSnapshot,
    focused_index: usize,
    search: Option<&SearchState>,
    project: Option<&str>,
    theme: &Theme,
) {
    render_list_page(
        frame,
        area,
        PageContext {
            rows: &snapshot.providers,
            noun: "provider",
            focused_index,
            search,
            empty_note: project,
        },
        theme,
        |frame, body, focus| {
            let focused_id = focus
                .and_then(|index| snapshot.providers.get(index))
                .map(ListRow::row_id);
            let chart = chart_with_focus(&snapshot.provider_chart, focused_id);
            render_chart(frame, body, &chart, theme);
        },
        |frame, area, row| render_detail(frame, area, row, theme),
    );
}

fn render_search_overlay<R: ListRow>(
    frame: &mut ratatui::Frame<'_>,
    header_area: Rect,
    spacer_area: Rect,
    detail_area: Rect,
    search: &SearchState,
    items: &[R],
    theme: &Theme,
) {
    let filtered_total = search.filtered_indices.len();
    let visible_start = search.scroll_offset;
    let visible_end = (visible_start + 5).min(filtered_total);
    let visible_slice = &search.filtered_indices[visible_start..visible_end];

    let hint = format!(
        "{}/{} | ↑/↓ | <esc> quit | <enter> select",
        filtered_total,
        items.len(),
    );
    let hint_width = UnicodeWidthStr::width(hint.as_str()) as u16;
    let [input_area, hint_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(hint_width)])
            .areas::<2>(header_area);

    let input_prefix = "  ● ";
    let prefix_w = UnicodeWidthStr::width(input_prefix) as u16;
    let max_input_w = input_area.width.saturating_sub(prefix_w).saturating_sub(1);
    let display_query = truncate_query_left(&search.query, max_input_w);
    let input_line = Line::from(vec![
        Span::styled(input_prefix, theme.muted_style()),
        Span::styled(
            format!("{}_", display_query),
            Style::default().fg(theme.foreground),
        ),
    ]);
    frame.render_widget(Paragraph::new(input_line), input_area);
    frame.render_widget(
        Paragraph::new(Span::styled(hint, theme.muted_style())),
        hint_area,
    );
    frame.render_widget(Paragraph::new(""), spacer_area);

    for line in 0..5 {
        let row_y = detail_area.y + line as u16;
        let row = Rect {
            x: detail_area.x,
            y: row_y,
            width: detail_area.width,
            height: 1,
        };

        let [indent, text_area, sb_area] = Layout::horizontal([
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas::<3>(row);

        if line < visible_slice.len() {
            let real_idx = visible_slice[line];
            let item = &items[real_idx];
            let id = item.search_text();
            let pct = item.row_percentage();
            let is_selected = (visible_start + line) == search.selected;

            let indicator_style = if is_selected {
                theme.accent_style()
            } else {
                theme.muted_style()
            };
            frame.render_widget(
                Paragraph::new(Span::styled("▌", indicator_style)),
                Rect {
                    x: indent.x + 2,
                    y: indent.y,
                    width: 1,
                    height: 1,
                },
            );

            let mut spans: Vec<Span> = Vec::new();
            let fg = Style::default().fg(theme.foreground);
            if search.query.trim().is_empty() {
                spans.push(Span::styled(id.to_string(), fg));
            } else {
                let positions = fuzzy_match_positions(&search.query, id);
                if positions.is_empty() {
                    spans.push(Span::styled(id.to_string(), fg));
                } else {
                    let ranges = merge_to_ranges(&positions);
                    let mut cursor = 0usize;
                    for (start, end) in &ranges {
                        if *start > cursor {
                            spans.push(Span::styled(id[cursor..*start].to_string(), fg));
                        }
                        spans.push(Span::styled(&id[*start..*end], theme.accent_style()));
                        cursor = *end;
                    }
                    if cursor < id.len() {
                        spans.push(Span::styled(id[cursor..].to_string(), fg));
                    }
                }
            }
            spans.push(Span::styled(format!(" ({:.2}%)", pct), theme.muted_style()));

            let text_rect = Rect {
                x: text_area.x + 1,
                y: text_area.y,
                width: text_area.width.saturating_sub(1),
                height: 1,
            };
            frame.render_widget(Paragraph::new(Line::from(spans)), text_rect);
        }

        if filtered_total > 5 {
            let thumb_size = (5.0 * 5.0 / filtered_total as f64).max(1.0) as usize;
            let thumb_start = ((search.scroll_offset as f64 / (filtered_total - 5) as f64)
                * (5.0 - thumb_size as f64)) as usize;
            let ch = if line >= thumb_start && line < thumb_start + thumb_size {
                "┃"
            } else {
                " "
            };
            frame.render_widget(
                Paragraph::new(Span::styled(ch, theme.muted_style())),
                sb_area,
            );
        }
    }
}

/// Sessions ranked by cost, most expensive first. The top area lists the leading
/// rows so the page answers its question at a glance; the detail panel below shows
/// the focused session. There is no chart: with hundreds of sessions a per-session
/// series would be noise.
#[cfg(test)]
mod tests {
    use super::{render_models, render_providers};
    use crate::analytics::heatmap_data::HeatmapData;
    use crate::analytics::model_stats::{ModelChartData, UsageRow};
    use crate::analytics::{AnalyticsSnapshot, OverviewStats};
    use crate::ui::theme::Theme;
    use crate::utils::pricing::PriceSummary;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn usage_row(id: &str, tokens: u64) -> UsageRow {
        UsageRow {
            id: id.to_string(),
            total_tokens: tokens,
            input_tokens: tokens / 2,
            output_tokens: tokens / 4,
            cache_tokens: tokens / 4,
            messages: 3,
            prompts: 2,
            sessions: 1,
            active_days: 1,
            cost: PriceSummary::default(),
            percentage: 100.0,
            p50_output_tokens_per_second: Some(12.5),
        }
    }

    fn empty_chart() -> ModelChartData {
        ModelChartData {
            x_bounds: [0.0, 1.0],
            y_bounds: [0.0, 1.0],
            x_labels: Vec::new(),
            series: Vec::new(),
        }
    }

    fn snapshot(models: Vec<UsageRow>, providers: Vec<UsageRow>) -> AnalyticsSnapshot {
        AnalyticsSnapshot {
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
                active_days: 0,
                current_streak: 0,
                longest_streak: 0,
                fun_comparison: String::new(),
            },
            models,
            chart: empty_chart(),
            providers,
            provider_chart: empty_chart(),
            sessions: Vec::new(),
            heatmap: HeatmapData {
                weeks: Vec::new(),
                month_labels: Vec::new(),
            },
        }
    }

    fn list_lines(
        snapshot: &AnalyticsSnapshot,
        providers_page: bool,
        focused: usize,
        search: Option<&super::SearchState>,
        project: Option<&str>,
    ) -> Vec<String> {
        let theme = Theme::builtin_dark();
        let mut terminal = Terminal::new(TestBackend::new(70, 16)).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                if providers_page {
                    render_providers(frame, area, snapshot, focused, search, project, &theme);
                } else {
                    render_models(frame, area, snapshot, focused, search, project, &theme);
                }
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

    fn page_text(snapshot: &AnalyticsSnapshot, providers_page: bool) -> String {
        list_lines(snapshot, providers_page, 0, None, None).join("")
    }

    #[test]
    fn renders_rows_and_detail_for_both_pages() {
        let snapshot = snapshot(vec![usage_row("gpt-5", 42)], vec![usage_row("openai", 21)]);

        let models_text = page_text(&snapshot, false);
        assert!(models_text.contains("gpt-5"));
        assert!(!models_text.contains("openai"));
        assert!(models_text.contains("1/1"));
        assert!(models_text.contains("Total tokens: "));
        assert!(models_text.contains("Rate: "));

        let providers_text = page_text(&snapshot, true);
        assert!(providers_text.contains("openai"));
        assert!(!providers_text.contains("gpt-5"));
        assert!(providers_text.contains("Total tokens: "));
    }

    #[test]
    fn the_focus_line_names_the_focused_row_and_its_position() {
        let snapshot = snapshot(
            vec![usage_row("gpt-5", 42), usage_row("claude-4", 21)],
            vec![usage_row("openai", 21), usage_row("anthropic", 9)],
        );

        for (providers_page, first, second) in
            [(false, "gpt-5", "claude-4"), (true, "openai", "anthropic")]
        {
            let first_focused = list_lines(&snapshot, providers_page, 0, None, None);
            let focus = first_focused
                .iter()
                .find(|line| line.contains("/2"))
                .unwrap_or_else(|| panic!("no focus line: {first_focused:#?}"));
            assert!(focus.contains(first), "{focus:?}");
            assert!(focus.contains("1/2"), "{focus:?}");
            assert!(focus.contains("j/k"), "{focus:?}");
            assert!(focus.contains("f find"), "{focus:?}");

            let second_focused = list_lines(&snapshot, providers_page, 1, None, None);
            let focus = second_focused
                .iter()
                .find(|line| line.contains("/2"))
                .unwrap_or_else(|| panic!("no focus line: {second_focused:#?}"));
            assert!(focus.contains(second), "{focus:?}");
            assert!(focus.contains("2/2"), "{focus:?}");
        }
    }

    #[test]
    fn the_detail_pane_reports_the_focused_rows_numbers() {
        let snapshot = snapshot(vec![usage_row("gpt-5", 40)], vec![]);
        let text = list_lines(&snapshot, false, 0, None, None).join("\n");
        for expected in [
            "Total tokens: 40",
            "Input: 20",
            "Output: 10",
            "Cache: 10",
            "Sessions: 1",
            "Messages: 3",
            "Prompts: 2",
            "Rate: 12.50 tok/s",
            "Active days: 1",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
    }

    #[test]
    fn a_row_with_no_measurable_rate_shows_a_dash_not_zero() {
        let mut row = usage_row("gpt-5", 40);
        row.p50_output_tokens_per_second = None;
        let text = list_lines(&snapshot(vec![row], vec![]), false, 0, None, None).join("\n");

        assert!(text.contains("Rate: —"), "{text}");
        assert!(!text.contains("0.00 tok/s"), "{text}");
    }

    #[test]
    fn the_search_overlay_shows_the_query_the_counter_and_the_selection() {
        let snapshot = snapshot(
            vec![usage_row("gpt-5", 42), usage_row("claude-4", 21)],
            vec![],
        );
        let mut search =
            super::SearchState::new(vec!["gpt-5".to_string(), "claude-4".to_string()], 0);
        search.query = "gpt".to_string();
        search.update_filter();

        let text = list_lines(&snapshot, false, 1, Some(&search), None).join("\n");

        assert!(text.contains("gpt_"), "the query with its cursor: {text}");
        assert!(text.contains("1/2"), "filtered over total: {text}");
        assert!(text.contains("<esc> quit"), "{text}");
        assert!(text.contains("<enter> select"), "{text}");
        assert!(text.contains('▌'), "the selected row is marked: {text}");
    }

    #[test]
    fn renders_the_empty_state_per_page() {
        let empty = snapshot(Vec::new(), Vec::new());

        assert!(page_text(&empty, false).contains("No model activity in this time range."));
        assert!(page_text(&empty, true).contains("No provider activity in this time range."));
        // The sessions page has its own empty-state test in the sessions module.
    }
}
