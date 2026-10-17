use nucleo_matcher::Matcher;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::analytics::AnalyticsSnapshot;
use crate::analytics::model_stats::{ModelChartData, SessionUsageRow, UsageRow, chart_with_focus};
use crate::ui::theme::Theme;
use crate::ui::widgets::common::{metric_line, truncate_label};
use crate::ui::widgets::linechart::build_chart;
use crate::utils::formatting::{format_duration, format_price_summary, format_tokens};

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
    theme: &Theme,
) {
    render_usage_list(
        frame,
        area,
        ListPage {
            rows: &snapshot.models,
            chart: &snapshot.chart,
            noun: "model",
        },
        focused_index,
        search,
        theme,
    );
}

/// What every list page needs from a row: a stable key, a share of the total, the
/// text search matches against, and the name shown in the header.
trait ListRow {
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

/// One list page: the rows to show, the chart above them, and what the rows are called
/// when there are none. This is the only thing the Models and Providers pages differ in.
struct ListPage<'a> {
    rows: &'a [UsageRow],
    chart: &'a ModelChartData,
    noun: &'a str,
}

fn render_usage_list(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    page: ListPage<'_>,
    focused_index: usize,
    search: Option<&SearchState>,
    theme: &Theme,
) {
    let ListPage { rows, chart, noun } = page;
    let [chart_area, spacer1, header_area, spacer2, detail_area, _] = Layout::default()
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

    let focused_row = effective_focus.and_then(|index| rows.get(index));
    let chart_data = chart_with_focus(chart, focused_row.map(ListRow::row_id));
    frame.render_widget(build_chart(&chart_data, theme), chart_area);

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
    } else if let Some(row) = focused_row {
        frame.render_widget(
            Paragraph::new(focus_line(row, focused_index, rows.len(), theme)),
            header_area,
        );
        frame.render_widget(Paragraph::new(""), spacer2);
        render_detail(frame, detail_area, row, theme);
    } else {
        frame.render_widget(Paragraph::new(""), spacer2);
        frame.render_widget(
            Paragraph::new(format!("No {noun} activity in this time range."))
                .style(theme.muted_style()),
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

fn render_detail(frame: &mut ratatui::Frame<'_>, area: Rect, row: &UsageRow, theme: &Theme) {
    let rows = layout_rows::<5, 2>(area);

    frame.render_widget(
        Paragraph::new(metric_line(
            "Total tokens: ",
            format_tokens(row.total_tokens),
            theme,
        )),
        rows[0][0],
    );
    frame.render_widget(
        Paragraph::new(metric_line(
            "Total cost: ",
            format_price_summary(&row.cost),
            theme,
        )),
        rows[0][1],
    );
    frame.render_widget(
        Paragraph::new(metric_line(
            "Input: ",
            format_tokens(row.input_tokens),
            theme,
        )),
        rows[1][0],
    );
    frame.render_widget(
        Paragraph::new(metric_line("Sessions: ", row.sessions.to_string(), theme)),
        rows[1][1],
    );
    frame.render_widget(
        Paragraph::new(metric_line(
            "Output: ",
            format_tokens(row.output_tokens),
            theme,
        )),
        rows[2][0],
    );
    frame.render_widget(
        Paragraph::new(metric_line("Messages: ", row.messages.to_string(), theme)),
        rows[2][1],
    );
    frame.render_widget(
        Paragraph::new(metric_line(
            "Cache: ",
            format_tokens(row.cache_tokens),
            theme,
        )),
        rows[3][0],
    );
    frame.render_widget(
        Paragraph::new(metric_line("Prompts: ", row.prompts.to_string(), theme)),
        rows[3][1],
    );
    frame.render_widget(
        Paragraph::new(metric_line(
            "Rate: ",
            format!("{:.2} tok/s", row.p50_output_tokens_per_second),
            theme,
        )),
        rows[4][0],
    );
    frame.render_widget(
        Paragraph::new(metric_line(
            "Active days: ",
            row.active_days.to_string(),
            theme,
        )),
        rows[4][1],
    );
}

pub fn render_providers(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &AnalyticsSnapshot,
    focused_index: usize,
    search: Option<&SearchState>,
    theme: &Theme,
) {
    render_usage_list(
        frame,
        area,
        ListPage {
            rows: &snapshot.providers,
            chart: &snapshot.provider_chart,
            noun: "provider",
        },
        focused_index,
        search,
        theme,
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
pub fn render_sessions(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    rows: &[SessionUsageRow],
    focused_index: usize,
    search: Option<&SearchState>,
    theme: &Theme,
) {
    let [table_area, spacer1, header_area, spacer2, detail_area, _] = Layout::default()
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

    let focused_row = effective_focus.and_then(|index| rows.get(index));
    render_session_table(frame, table_area, rows, effective_focus, theme);

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
    } else if let Some(row) = focused_row {
        frame.render_widget(
            Paragraph::new(focus_line(row, focused_index, rows.len(), theme)),
            header_area,
        );
        frame.render_widget(Paragraph::new(""), spacer2);
        render_session_detail(frame, detail_area, row, theme);
    } else {
        frame.render_widget(Paragraph::new(""), spacer2);
        frame.render_widget(
            Paragraph::new("No session activity in this time range.").style(theme.muted_style()),
            detail_area,
        );
    }
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
    let detail = layout_rows::<5, 2>(area);

    frame.render_widget(
        Paragraph::new(metric_line(
            "Total tokens: ",
            format_tokens(row.total_tokens),
            theme,
        )),
        detail[0][0],
    );
    frame.render_widget(
        Paragraph::new(metric_line(
            "Total cost: ",
            format_price_summary(&row.cost),
            theme,
        )),
        detail[0][1],
    );
    frame.render_widget(
        Paragraph::new(metric_line(
            "Input: ",
            format_tokens(row.input_tokens),
            theme,
        )),
        detail[1][0],
    );
    frame.render_widget(
        Paragraph::new(metric_line(
            "Output: ",
            format_tokens(row.output_tokens),
            theme,
        )),
        detail[1][1],
    );
    frame.render_widget(
        Paragraph::new(metric_line(
            "Cache: ",
            format_tokens(row.cache_tokens),
            theme,
        )),
        detail[2][0],
    );
    frame.render_widget(
        Paragraph::new(metric_line("Messages: ", row.messages.to_string(), theme)),
        detail[2][1],
    );
    frame.render_widget(
        Paragraph::new(metric_line("Prompts: ", row.prompts.to_string(), theme)),
        detail[3][0],
    );
    frame.render_widget(
        Paragraph::new(metric_line(
            "Elapsed: ",
            row.elapsed_secs()
                .map(format_duration)
                .unwrap_or_else(|| "—".to_string()),
            theme,
        )),
        detail[3][1],
    );
    frame.render_widget(
        Paragraph::new(metric_line("Project: ", row.project.clone(), theme)),
        detail[4][0],
    );
    frame.render_widget(
        Paragraph::new(metric_line(
            "Active days: ",
            row.active_days.to_string(),
            theme,
        )),
        detail[4][1],
    );
}

fn layout_rows<const ROW: usize, const COL: usize>(area: Rect) -> [[Rect; COL]; ROW] {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1); ROW])
        .areas::<ROW>(area)
        .map(|line| {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Fill(1); COL])
                .areas::<COL>(line)
        })
}

#[cfg(test)]
mod tests {
    use super::{render_models, render_providers, render_sessions};
    use crate::analytics::heatmap_data::HeatmapData;
    use crate::analytics::model_stats::{ModelChartData, SessionUsageRow, UsageRow};
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
            p50_output_tokens_per_second: 12.5,
        }
    }

    fn empty_chart() -> ModelChartData {
        ModelChartData {
            x_bounds: [0.0, 1.0],
            y_bounds: [0.0, 1.0],
            x_labels: Vec::new(),
            y_labels: Vec::new(),
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

    fn page_text(snapshot: &AnalyticsSnapshot, providers_page: bool) -> String {
        let theme = Theme::builtin_dark();
        let mut terminal = Terminal::new(TestBackend::new(70, 16)).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                if providers_page {
                    render_providers(frame, area, snapshot, 0, None, &theme);
                } else {
                    render_models(frame, area, snapshot, 0, None, &theme);
                }
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
    fn renders_the_empty_state_per_page() {
        let empty = snapshot(Vec::new(), Vec::new());

        assert!(page_text(&empty, false).contains("No model activity in this time range."));
        assert!(page_text(&empty, true).contains("No provider activity in this time range."));
    }

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
