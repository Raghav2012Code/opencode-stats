use std::io::stdout;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use color_eyre::eyre::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::{StyledContent, Stylize};
use futures::StreamExt;
use ratatui::layout::{Constraint, Layout};
use ratatui::prelude::CrosstermBackend;
use ratatui::widgets::{Block, Padding};
use ratatui::{DefaultTerminal, Frame, Terminal, TerminalOptions, Viewport, backend::TestBackend};
use tokio::sync::mpsc;

use crate::analytics::model_stats::UsageRow;
use crate::analytics::{AnalyticsSnapshot, build_snapshot};
use crate::cache::models_cache::{PricingCatalog, refresh_remote_models};
use crate::db::models::{AppData, InputOptions};
use crate::db::queries::load_app_data;
use crate::stats_output::{stats_csv, stats_json};
use crate::ui::export::{artifact_prefix, share_card_file_name};
use crate::ui::models::MAX_QUERY_LEN;
use crate::ui::models::{SearchState, render_models, render_providers};
use crate::ui::overview::render_overview;
use crate::ui::sessions::render_sessions;
use crate::ui::share::{ClipboardSink, ShareJob, SystemClipboard, perform_share};
use crate::ui::theme::{Theme, ThemeKind};
use crate::ui::widgets::common::{CONTENT_WIDTH, left_aligned_content, segment_span};
use crate::utils::formatting::format_price_summary;
use crate::utils::pricing::ZeroCostBehavior;
use crate::utils::time::TimeRange;

const VIEWPORT_HEIGHT: u16 = 23;
const STATUS_TTL: Duration = Duration::from_secs(3);
/// A copy result names a file the user may want to read or open, so it is not the 3-second
/// flash used for passing notices.
const SHARE_STATUS_TTL: Duration = Duration::from_secs(10);
const TICK_TIME_MS: u16 = 200;
/// Footer hints as key and action. Drawn in this order and dropped from the right when the
/// footer is too narrow, so a hint is never cut in half.
const FOOTER_HINTS: &[(&str, &str)] = &[
    ("tab", "pages"),
    ("r/R", "range"),
    ("f", "find"),
    ("s/^s", "share"),
    ("e", "export"),
    ("q", "quit"),
];
/// What sits at the right end of the rule above the content.
const LOADED_LABEL: &str = "Loaded ";

#[derive(Clone, Debug)]
struct StatusMessage {
    text: String,
    expires_at: Option<Instant>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Page {
    #[default]
    Overview,
    Models,
    Providers,
    Sessions,
}

impl Page {
    pub fn next(self) -> Self {
        match self {
            Self::Overview => Self::Models,
            Self::Models => Self::Providers,
            Self::Providers => Self::Sessions,
            Self::Sessions => Self::Overview,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            Self::Overview => Self::Sessions,
            Self::Models => Self::Overview,
            Self::Providers => Self::Models,
            Self::Sessions => Self::Providers,
        }
    }

    /// Every page in tab order, so config validation offers the same names the tab bar
    /// shows instead of repeating them.
    pub const ALL: [Self; 4] = [
        Self::Overview,
        Self::Models,
        Self::Providers,
        Self::Sessions,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Models => "models",
            Self::Providers => "providers",
            Self::Sessions => "sessions",
        }
    }

    pub fn from_name(value: &str) -> Option<Self> {
        let lowered = value.to_ascii_lowercase();
        Self::ALL.into_iter().find(|page| page.name() == lowered)
    }

    pub fn names() -> Vec<&'static str> {
        Self::ALL.into_iter().map(Self::name).collect()
    }

    /// Whether the time range applies here. The overview is the whole picture by design:
    /// it always shows all time, so it offers no range selector and ignores the range keys.
    /// The list pages are the ones you narrow down.
    pub fn uses_range(self) -> bool {
        !matches!(self, Self::Overview)
    }
}

pub struct App {
    pub data: AppData,
    pub pricing: PricingCatalog,
    pub snapshot: AnalyticsSnapshot,
    pub page: Page,
    pub range: TimeRange,
    pub theme: Theme,
    pub zero_cost_behavior: ZeroCostBehavior,
    pub should_quit: bool,
    status_message: Option<StatusMessage>,
    pub focused_index: usize,
    input_options: InputOptions,
    last_read: DateTime<Local>,
    pub search: Option<SearchState>,
    pricing_updates: mpsc::UnboundedReceiver<Result<PricingCatalog>>,
    clipboard_sender: mpsc::UnboundedSender<String>,
    clipboard_updates: mpsc::UnboundedReceiver<String>,
    copy_in_progress: bool,
}

impl App {
    pub fn new(
        data: AppData,
        pricing: PricingCatalog,
        theme: Theme,
        zero_cost_behavior: ZeroCostBehavior,
        input_options: InputOptions,
        page: Page,
        range: TimeRange,
    ) -> Self {
        // The overview is always all time; the chosen range applies to the list pages.
        let initial_range = if page.uses_range() {
            range
        } else {
            TimeRange::All
        };
        let snapshot = build_snapshot(&data, &pricing, initial_range, zero_cost_behavior);
        let (sender, receiver) = mpsc::unbounded_channel();
        let (clipboard_sender, clipboard_updates) = mpsc::unbounded_channel();
        if pricing.refresh_needed {
            let cache_path = pricing.cache_path.clone();
            tokio::spawn(refresh_remote_models(cache_path, sender));
        }

        let mut app = Self {
            data,
            pricing,
            snapshot,
            page,
            range,
            theme,
            zero_cost_behavior,
            should_quit: false,
            status_message: None,
            focused_index: 0,
            input_options,
            last_read: Local::now(),
            search: None,
            pricing_updates: receiver,
            clipboard_sender,
            clipboard_updates,
            copy_in_progress: false,
        };

        let notices = [
            app.data.import_stats.summary(),
            app.pricing.load_notice.clone(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        if !notices.is_empty() {
            app.set_status(notices.join(" | "));
        }

        app
    }

    pub async fn run(mut self) -> Result<()> {
        let mut terminal = Self::init_with_options(TerminalOptions {
            viewport: Viewport::Inline(VIEWPORT_HEIGHT),
        })?;
        let app_result = self.run_loop(&mut terminal).await;
        Self::restore(&mut terminal)?;
        app_result
    }

    /// 为什么 ratatui::restore 的 panic hook 默认行为有个 LeaveAlternateScreen ……
    fn init_with_options(options: TerminalOptions) -> Result<DefaultTerminal> {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            eprintln!("\r\n");
            if let Err(err) = crossterm::terminal::disable_raw_mode() {
                eprintln!("Failed to disable raw mode: {err}");
            }
            hook(info);
        }));
        crossterm::terminal::enable_raw_mode()?;
        let backend = CrosstermBackend::new(stdout());
        let terminal = Terminal::with_options(backend, options)?;
        Ok(terminal)
    }

    /// 默认的 ratatui::restore 在 Inline Viewport 下有错误的行为，
    /// 此处重置终端以确保光标和输入状态正确恢复
    fn restore(terminal: &mut DefaultTerminal) -> Result<()> {
        terminal.clear()?;
        crossterm::terminal::disable_raw_mode()?;
        Ok(())
    }

    async fn run_loop(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let mut event_stream = EventStream::new();
        let mut tick = tokio::time::interval(Duration::from_millis(TICK_TIME_MS as _));

        while !self.should_quit {
            self.clear_expired_status();
            terminal.draw(|frame| self.render(frame))?;

            tokio::select! {
                Some(event) = event_stream.next() => {
                    let event = event?;
                    if let Event::Key(key) = event && key.kind == KeyEventKind::Press {
                        self.handle_key(key);
                    }
                }
                Some(pricing) = self.pricing_updates.recv() => {
                    if let Ok(pricing) = pricing {
                        self.pricing = pricing;
                        self.recompute();
                        self.set_status("Pricing cache refreshed from models.dev");
                    } else {
                        self.set_status(format!(
                            "Failed to refresh cache from models.dev; {}",
                            self.pricing.refresh_failure_hint()
                        ));
                    }
                }
                Some(update) = self.clipboard_updates.recv() => {
                    self.copy_in_progress = false;
                    // The result names a file, so give it longer than a passing notice.
                    self.set_status_for(update, SHARE_STATUS_TTL);
                }
                _ = tick.tick() => {}
            }
        }
        Ok(())
    }

    fn recompute(&mut self) {
        self.snapshot = build_snapshot(
            &self.data,
            &self.pricing,
            self.effective_range(),
            self.zero_cost_behavior,
        );
    }

    /// The range the current page is drawn with. The overview ignores the selection and is
    /// always all time; every other page uses what the user picked.
    fn effective_range(&self) -> TimeRange {
        if self.page.uses_range() {
            self.range
        } else {
            TimeRange::All
        }
    }

    /// Moves to a page, rebuilding when that page changes which range applies, and keeping
    /// the range the user picked for the pages that use it.
    fn set_page(&mut self, page: Page) {
        let before = self.effective_range();
        self.page = page;
        self.search = None;
        self.focused_index = 0;
        if self.effective_range() != before {
            self.recompute();
        }
    }

    fn reload(&mut self) {
        let result = load_app_data(&self.input_options);
        self.apply_reload(result);
    }

    /// Swaps in freshly loaded data, or reports the failure and keeps what is on screen.
    /// Split from [`App::reload`] so the failure path is testable without depending on
    /// which database files happen to exist on the machine running the tests.
    fn apply_reload(&mut self, result: crate::db::errors::Result<AppData>) {
        match result {
            Ok(data) => self.finish_reload(data),
            Err(err) => self.set_status(format!("Reload failed: {err:#}")),
        }
    }

    fn finish_reload(&mut self, data: AppData) {
        self.data = data;
        self.recompute();
        self.last_read = Local::now();
        self.set_status("Reloaded usage data from disk");
    }

    fn render(&self, frame: &mut Frame<'_>) {
        let theme = &self.theme;
        let area = frame.area();
        let vertical = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(16),
            Constraint::Length(1),
        ]);

        let [divider, header, spacer, body, footer] =
            vertical.areas(area).map(left_aligned_content);
        let [header, body, footer] = [header, body, footer]
            .map(|area| Block::new().padding(Padding::horizontal(1)).inner(area));

        frame.render_widget(
            ratatui::widgets::Paragraph::new(self.divider_line(divider.width, theme)),
            divider,
        );

        self.render_header(frame, header, theme);
        frame.render_widget(ratatui::widgets::Paragraph::new(""), spacer);

        match self.page {
            Page::Overview => render_overview(frame, body, &self.snapshot, theme),
            Page::Models => render_models(
                frame,
                body,
                &self.snapshot,
                self.focused_index,
                self.search.as_ref(),
                theme,
            ),
            Page::Providers => render_providers(
                frame,
                body,
                &self.snapshot,
                self.focused_index,
                self.search.as_ref(),
                theme,
            ),
            Page::Sessions => render_sessions(
                frame,
                body,
                &self.snapshot.sessions,
                self.focused_index,
                self.search.as_ref(),
                theme,
            ),
        }

        self.render_footer(frame, footer, theme);
    }

    fn render_header(&self, frame: &mut Frame<'_>, area: ratatui::layout::Rect, theme: &Theme) {
        let content = area;
        let mut spans = vec![
            segment_span("Overview", self.page == Page::Overview, theme),
            segment_span("Models", self.page == Page::Models, theme),
            segment_span("Providers", self.page == Page::Providers, theme),
            segment_span("Sessions", self.page == Page::Sessions, theme),
        ];
        // The overview is all time, so it shows no range selector to click through. The
        // labels are the short names `stats --range` uses, and the strip is only drawn in
        // full when it fits: a narrow terminal falls back to the active range on its own,
        // because a clipped label reads as a missing range.
        if self.page.uses_range() {
            let ranges = [
                (" All ", TimeRange::All),
                ("Today", TimeRange::Today),
                ("7d", TimeRange::Last7Days),
                ("30d", TimeRange::Last30Days),
            ];
            let used: usize = spans.iter().map(|span| span.content.chars().count()).sum();
            // `segment_span` renders one space either side of the label.
            let needed: usize = ranges
                .iter()
                .map(|(label, _)| label.chars().count() + 2)
                .sum();

            if used + needed <= area.width as usize {
                for (label, range) in ranges {
                    spans.push(segment_span(label, self.range == range, theme));
                }
            } else if let Some((label, _)) = ranges.iter().find(|(_, range)| *range == self.range) {
                spans.push(segment_span(label, true, theme));
            }
        }

        frame.render_widget(
            ratatui::widgets::Paragraph::new(ratatui::text::Line::from(spans)),
            content,
        );
    }

    fn render_footer(&self, frame: &mut Frame<'_>, area: ratatui::layout::Rect, theme: &Theme) {
        let line = match &self.status_message {
            Some(status) => ratatui::text::Line::from(ratatui::text::Span::styled(
                status.text.clone(),
                theme.muted_style(),
            )),
            None => footer_hints(area.width, theme),
        };

        frame.render_widget(ratatui::widgets::Paragraph::new(line), area);
    }

    /// The rule under the title, with when the data was last read at its right end. Keeping
    /// the timestamp here leaves the footer free for hints and status, and it never has to
    /// be shortened to make room for them.
    fn divider_line(&self, width: u16, theme: &Theme) -> ratatui::text::Line<'static> {
        let stamp = format!("{LOADED_LABEL}{}", self.last_read.format("%H:%M:%S"));
        let stamp_width = stamp.chars().count() + 2;
        let rule_width = (width as usize).saturating_sub(stamp_width);

        ratatui::text::Line::from(vec![
            ratatui::text::Span::styled("─".repeat(rule_width), theme.muted_style()),
            ratatui::text::Span::styled(format!("  {stamp}"), theme.muted_style()),
        ])
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if let Some(ref mut search) = self.search {
            match key.code {
                KeyCode::Esc => self.search = None,
                KeyCode::Enter => {
                    if let Some(&real_idx) = search.filtered_indices.get(search.selected)
                        && !matches!(self.page, Page::Overview)
                    {
                        self.focused_index = real_idx;
                    }
                    self.search = None;
                }
                KeyCode::Backspace if !search.query.is_empty() => {
                    search.query.pop();
                    self.update_search_filter();
                }
                KeyCode::Down if !search.filtered_indices.is_empty() => {
                    let total = search.filtered_indices.len();
                    if search.selected + 1 >= total {
                        search.selected = 0;
                        search.scroll_offset = 0;
                    } else {
                        search.selected += 1;
                        if search.selected >= search.scroll_offset + 5 {
                            search.scroll_offset = search.selected.saturating_sub(4);
                        }
                    }
                }
                KeyCode::Up => {
                    if search.selected == 0 {
                        let total = search.filtered_indices.len();
                        search.selected = total.saturating_sub(1);
                        search.scroll_offset = search.selected.saturating_sub(4);
                    } else {
                        search.selected -= 1;
                        if search.selected < search.scroll_offset {
                            search.scroll_offset = search.selected;
                        }
                    }
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.should_quit = true;
                }
                KeyCode::Char(' ') if search.query.len() < MAX_QUERY_LEN => {
                    search.query.push(' ');
                    self.update_search_filter();
                }
                KeyCode::Char(value)
                    if !value.is_control()
                        && value.is_ascii_graphic()
                        && search.query.len() < MAX_QUERY_LEN =>
                {
                    search.query.push(value);
                    self.update_search_filter();
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                let next = self.page.next();
                self.set_page(next);
            }
            KeyCode::Left | KeyCode::Char('h') => {
                let previous = self.page.previous();
                self.set_page(previous);
            }
            KeyCode::Down | KeyCode::Char('j') => self.advance_focus(1),
            KeyCode::Up | KeyCode::Char('k') => self.advance_focus(-1),
            // The overview shows all time, so it has no range to cycle or set.
            KeyCode::Char('r') if self.page.uses_range() => {
                self.range = self.range.cycle();
                self.focused_index = 0;
                self.search = None;
                self.recompute();
            }
            // Lowercase 'r' cycles the time range, so live reload takes shifted 'R'.
            KeyCode::Char('R') => self.reload(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.copy_current_page();
            }
            // Some terminals take Ctrl+S as XOFF and never deliver it, so the same action
            // is on plain 's' as well.
            KeyCode::Char('s') => self.copy_current_page(),
            KeyCode::Char('f') | KeyCode::Char('/')
                if matches!(self.page, Page::Models | Page::Providers | Page::Sessions) =>
            {
                self.enter_search();
            }
            KeyCode::Char('e') => self.export_current_view(),
            KeyCode::Char(value) => {
                if self.page.uses_range()
                    && let Some(range) = TimeRange::from_shortcut(value)
                {
                    self.range = range;
                    self.focused_index = 0;
                    self.search = None;
                    self.recompute();
                }
            }
            _ => {}
        }
    }

    fn advance_focus(&mut self, delta: isize) {
        let total = match self.page {
            Page::Models => self.snapshot.models.len(),
            Page::Providers => self.snapshot.providers.len(),
            Page::Sessions => self.snapshot.sessions.len(),
            Page::Overview => return,
        };
        if total == 0 {
            return;
        }
        let next = (self.focused_index as isize + delta).rem_euclid(total as isize) as usize;
        self.focused_index = next;
    }

    fn enter_search(&mut self) {
        let ids = match self.page {
            Page::Models => self
                .snapshot
                .models
                .iter()
                .map(|row| row.id.clone())
                .collect::<Vec<_>>(),
            Page::Providers => self
                .snapshot
                .providers
                .iter()
                .map(|row| row.id.clone())
                .collect::<Vec<_>>(),
            Page::Sessions => self
                .snapshot
                .sessions
                .iter()
                .map(|row| row.title.clone())
                .collect::<Vec<_>>(),
            Page::Overview => return,
        };
        self.search = Some(SearchState::new(ids, self.focused_index));
    }

    fn update_search_filter(&mut self) {
        if let Some(search) = self.search.as_mut() {
            search.update_filter();
        }
    }

    fn set_status(&mut self, text: impl Into<String>) {
        self.set_status_for(text, STATUS_TTL);
    }

    /// A status that stays long enough to read, for results that name a file.
    fn set_status_for(&mut self, text: impl Into<String>, ttl: Duration) {
        self.status_message = Some(StatusMessage {
            text: text.into(),
            expires_at: Some(Instant::now() + ttl),
        });
    }

    fn clear_expired_status(&mut self) {
        if self
            .status_message
            .as_ref()
            .and_then(|status| status.expires_at)
            .is_some_and(|expires_at| Instant::now() >= expires_at)
        {
            self.status_message = None;
        }
    }

    fn capture_current_page_buffer(&self) -> Result<ratatui::buffer::Buffer> {
        let backend = TestBackend::new(CONTENT_WIDTH, VIEWPORT_HEIGHT);
        let mut terminal = Terminal::new(backend)?;
        let frame = terminal.draw(|frame| self.render(frame))?;
        Ok(frame.buffer.clone())
    }

    /// Writes what is on screen — overview, model rows, provider rows and the heatmap
    /// window, for the selected range — to timestamped CSV and JSON files in the working
    /// directory, reusing the `stats` command's serializers so the two can never disagree.
    /// A failed write keeps the dashboard as it is and reports the failure.
    fn export_current_view(&mut self) {
        let timestamp = chrono::Local::now().timestamp();
        let mut written = Vec::new();
        for (filename, content) in
            export_documents(&self.snapshot, self.effective_range(), timestamp)
        {
            match std::fs::write(&filename, content) {
                Ok(()) => written.push(filename),
                Err(err) => {
                    self.set_status(format!("Export failed for {filename}: {err}"));
                    return;
                }
            }
        }
        self.set_status(format!("Exported {}", written.join(", ")));
    }

    fn copy_current_page(&mut self) {
        if self.copy_in_progress {
            self.set_status("Still rendering share card in background, please wait ...");
            return;
        }

        let job = match self.prepare_share_job() {
            Ok(job) => job,
            Err(err) => {
                self.set_status(format!("Copy failed: {err}"));
                return;
            }
        };

        self.copy_in_progress = true;
        self.set_status_for("Rendering share card in background ...", SHARE_STATUS_TTL);

        let sender = self.clipboard_sender.clone();
        tokio::task::spawn_blocking(move || {
            let outcome = perform_share(job, || {
                SystemClipboard::open().map(|sink| Box::new(sink) as Box<dyn ClipboardSink>)
            });
            // The message names what the paste will produce and where the PNG went.
            let _ = sender.send(outcome.message());
        });
    }

    /// Captures the page as it is on screen, plus where its PNG belongs, so the rendering
    /// thread touches only its own copy.
    fn prepare_share_job(&self) -> Result<ShareJob> {
        let timestamp = chrono::Local::now().timestamp();
        Ok(ShareJob {
            buffer: self.capture_current_page_buffer()?,
            theme: self.theme.clone(),
            summary: self.current_page_summary(),
            png_path: std::path::PathBuf::from(share_card_file_name(
                self.effective_range(),
                timestamp,
            )),
        })
    }

    fn current_page_summary(&self) -> String {
        match self.page {
            Page::Overview => format!(
                "oc-stats {}\nTokens: {}\nCost: {}\nSessions: {}\nMessages: {}\nPrompts: {}",
                self.effective_range().label(),
                self.snapshot.overview.total_tokens,
                format_price_summary(&self.snapshot.overview.total_cost),
                self.snapshot.overview.sessions,
                self.snapshot.overview.messages,
                self.snapshot.overview.prompts,
            ),
            Page::Models => summary_lines(&self.snapshot.models),
            Page::Providers => summary_lines(&self.snapshot.providers),
            Page::Sessions => self
                .snapshot
                .sessions
                .iter()
                .take(8)
                .map(|row| {
                    format!(
                        "{}: {} tokens ({})",
                        row.title,
                        row.total_tokens,
                        format_price_summary(&row.cost),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

/// The hint row: each key in the foreground colour and its action muted, separated by dots.
/// Hints that do not fit are dropped rather than clipped.
fn footer_hints(width: u16, theme: &Theme) -> ratatui::text::Line<'static> {
    use ratatui::style::Style;
    use ratatui::text::Span;

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;

    for (index, (key, action)) in FOOTER_HINTS.iter().enumerate() {
        let separator = if index == 0 { "" } else { " · " };
        let segment_width =
            separator.chars().count() + key.chars().count() + 1 + action.chars().count();
        if used + segment_width > width as usize {
            break;
        }
        used += segment_width;

        if !separator.is_empty() {
            spans.push(Span::styled(separator.to_string(), theme.muted_style()));
        }
        spans.push(Span::styled(
            (*key).to_string(),
            Style::default().fg(theme.foreground),
        ));
        spans.push(Span::styled(format!(" {action}"), theme.muted_style()));
    }

    ratatui::text::Line::from(spans)
}

fn summary_lines(rows: &[UsageRow]) -> String {
    rows.iter()
        .take(8)
        .map(|row| {
            format!(
                "{}: {} tokens ({:.2}%)",
                row.id, row.total_tokens, row.percentage
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The files [`App::export_current_view`] writes: `(filename, content)` pairs with
/// timestamped names, kept pure so tests can assert on the content without touching disk.
fn export_documents(
    snapshot: &AnalyticsSnapshot,
    range: TimeRange,
    timestamp: i64,
) -> Vec<(String, String)> {
    let prefix = artifact_prefix(range, timestamp);
    vec![
        (format!("{prefix}.csv"), stats_csv(snapshot, range)),
        (
            format!("{prefix}.json"),
            serde_json::to_string_pretty(&stats_json(snapshot, range))
                .unwrap_or_else(|_| "{}".to_string()),
        ),
    ]
}

pub fn print_exit_art(theme_kind: ThemeKind) {
    type NumStr = [&'static str; 4];
    const O: NumStr = ["    ", "█▀▀█", "█  █", "▀▀▀▀"];
    const C: NumStr = ["    ", "█▀▀▀", "█   ", "▀▀▀▀"];
    const S: NumStr = ["    ", "▄▀▀▀", "▀▀▀█", "▀▀▀ "];
    const T: NumStr = [" ▄  ", "▀█▀▀", " █  ", "  ▀▀"];
    const A: NumStr = ["    ", "█▀▀█", "█  █", "▀▀▀ ▀"];

    fn eprint_last_line(str: &str, theme: ThemeKind) {
        for ch in str.chars() {
            match ch {
                ' ' => eprint!(
                    "{}",
                    match theme {
                        ThemeKind::Dark => "▀".dark_grey(),
                        ThemeKind::Light => "▀".white(),
                    }
                ),
                ch => eprint!("{ch}"),
            }
        }
    }

    fn line_2_oc(str: &str, theme: ThemeKind) -> StyledContent<&str> {
        match theme {
            ThemeKind::Dark => str.dark_grey().on_black(),
            ThemeKind::Light => str.dark_grey().on_white(),
        }
    }

    fn line_2_stats(str: &str, theme: ThemeKind) -> StyledContent<&str> {
        match theme {
            ThemeKind::Dark => str.on_dark_grey(),
            ThemeKind::Light => str.on_white(),
        }
    }

    for idx in 0..O.len() {
        let (o, c, s, t, a) = (O[idx], C[idx], S[idx], T[idx], A[idx]);
        match idx {
            0..2 => eprintln!(
                "  {o} {c}  {s} {t} {a} {t} {s}",
                o = o.dark_grey(),
                c = c.dark_grey(),
            ),
            2 => eprintln!(
                "  {o} {c}  {s} {t} {a} {t} {s}",
                o = line_2_oc(o, theme_kind),
                c = line_2_oc(c, theme_kind),
                s = line_2_stats(s, theme_kind),
                t = line_2_stats(t, theme_kind),
                a = line_2_stats(a, theme_kind),
            ),
            3 => {
                eprint!("  {o} {c}  ", o = o.dark_grey(), c = c.dark_grey());
                eprint_last_line(s, theme_kind);
                eprint!(" ");
                eprint_last_line(t, theme_kind);
                eprint!(" ");
                eprint_last_line(a, theme_kind);
                eprint_last_line(t, theme_kind);
                eprint!(" ");
                eprint_last_line(s, theme_kind);
                eprintln!();
            }
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::models_cache::PricingAvailability;
    use crate::db::models::{ImportStats, TokenUsage, UsageEvent};
    use std::collections::BTreeMap;

    fn test_pricing() -> PricingCatalog {
        PricingCatalog {
            models: BTreeMap::new(),
            cache_path: std::path::PathBuf::from("/tmp/models.json"),
            refresh_needed: false,
            availability: PricingAvailability::Empty,
            load_notice: None,
        }
    }

    fn test_event() -> UsageEvent {
        let now = Local::now();
        UsageEvent {
            session_id: "ses_1".to_string(),
            parent_session_id: None,
            session_title: None,
            session_started_at: Some(now),
            session_archived_at: None,
            project_name: None,
            project_path: None,
            provider_id: Some("openai".to_string()),
            model_id: "gpt-5".to_string(),
            agent: None,
            finish_reason: Some("stop".to_string()),
            tokens: TokenUsage {
                input: 10,
                output: 20,
                cache_read: 0,
                cache_write: 0,
            },
            created_at: Some(now),
            completed_at: Some(now),
            stored_cost_usd: None,
        }
    }

    fn test_data(events: Vec<UsageEvent>) -> AppData {
        AppData {
            events,
            messages: Vec::new(),
            session_records: Vec::new(),
            import_stats: ImportStats::default(),
        }
    }

    fn test_app(data: AppData, input_options: InputOptions) -> App {
        App::new(
            data,
            test_pricing(),
            Theme::builtin_dark(),
            ZeroCostBehavior::KeepZero,
            input_options,
            Page::Overview,
            TimeRange::All,
        )
    }

    fn no_input_options() -> InputOptions {
        InputOptions {
            database_path: None,
        }
    }

    fn app_with(page: Page, range: TimeRange, events: Vec<UsageEvent>) -> App {
        App::new(
            test_data(events),
            test_pricing(),
            Theme::builtin_dark(),
            ZeroCostBehavior::KeepZero,
            no_input_options(),
            page,
            range,
        )
    }

    fn frame_text(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer.cell((x, y)).map(|cell| cell.symbol()).unwrap_or(" "))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn an_event_dated(days_ago: i64) -> UsageEvent {
        let when = Local::now() - chrono::Duration::days(days_ago);
        let mut event = test_event();
        event.created_at = Some(when);
        event.session_started_at = Some(when);
        event
    }

    #[test]
    fn the_overview_offers_no_range_selector_and_the_list_pages_do() {
        let header_line = |app: &App| {
            let frame = frame_text(app, 100, 24);
            frame
                .lines()
                .find(|line| line.contains("Overview"))
                .unwrap_or_default()
                .to_string()
        };

        let overview = app_with(Page::Overview, TimeRange::Last7Days, vec![test_event()]);
        let header = header_line(&overview);

        assert!(header.contains("Overview"), "{header}");
        for range in ["Today", "7d", "30d"] {
            assert!(
                !header.contains(range),
                "the overview has no range selector, found {range}: {header}"
            );
        }

        for page in [Page::Models, Page::Providers, Page::Sessions] {
            let app = app_with(page, TimeRange::Last7Days, vec![test_event()]);
            let header = header_line(&app);
            assert!(
                header.contains("7d") && header.contains("30d"),
                "{page:?} keeps its full range selector, nothing clipped: {header}"
            );
        }
    }

    #[test]
    fn the_overview_is_all_time_whatever_the_selected_range_is() {
        // Two events, one well outside the 7-day window.
        let events = vec![an_event_dated(40), an_event_dated(0)];

        let mut app = app_with(Page::Models, TimeRange::Last7Days, events);
        assert_eq!(
            app.snapshot.overview.total_tokens, 30,
            "the list pages obey the selected range"
        );

        app.set_page(Page::Overview);
        assert_eq!(app.range, TimeRange::Last7Days, "the choice is remembered");
        assert_eq!(
            app.effective_range(),
            TimeRange::All,
            "the overview is always all time"
        );
        assert_eq!(
            app.snapshot.overview.total_tokens, 60,
            "the overview shows the old event too"
        );

        app.set_page(Page::Models);
        assert_eq!(
            app.snapshot.overview.total_tokens, 30,
            "returning to a list page applies the remembered range again"
        );
    }

    #[test]
    fn the_range_keys_do_nothing_on_the_overview_and_work_on_the_list_pages() {
        let key = |code: KeyCode| KeyEvent::new(code, KeyModifiers::NONE);

        let mut app = app_with(Page::Overview, TimeRange::Last7Days, vec![test_event()]);
        app.handle_key(key(KeyCode::Char('r')));
        assert_eq!(
            app.range,
            TimeRange::Last7Days,
            "'r' is not a range toggle here"
        );
        app.handle_key(key(KeyCode::Char('3')));
        assert_eq!(
            app.range,
            TimeRange::Last7Days,
            "'3' is not a range shortcut here"
        );

        app.set_page(Page::Providers);
        app.handle_key(key(KeyCode::Char('r')));
        assert_eq!(
            app.range,
            TimeRange::Last30Days,
            "'r' cycles on a list page"
        );
        app.handle_key(key(KeyCode::Char('2')));
        assert_eq!(
            app.range,
            TimeRange::Today,
            "'2' selects Today on a list page"
        );
    }

    #[test]
    fn the_range_strip_never_clips_in_a_narrow_terminal() {
        // 70 columns is the content width; below that the full strip cannot fit, so the
        // active range is shown alone rather than a cut-off label.
        let app = app_with(Page::Models, TimeRange::Last30Days, vec![test_event()]);

        let wide = frame_text(&app, 100, 24);
        let wide_header = wide
            .lines()
            .find(|line| line.contains("Overview"))
            .unwrap_or_default()
            .to_string();
        assert!(
            wide_header.contains("All") && wide_header.contains("30d"),
            "the full strip fits the content width: {wide_header}"
        );

        let narrow = frame_text(&app, 50, 24);
        let narrow_header = narrow
            .lines()
            .find(|line| line.contains("Overview"))
            .unwrap_or_default()
            .to_string();
        assert!(
            narrow_header.contains("30d"),
            "the selected range is still readable when narrow: {narrow_header}"
        );
        assert!(
            !narrow_header.contains("Today"),
            "the other ranges step aside rather than clip: {narrow_header}"
        );
    }

    #[test]
    fn the_footer_hints_are_spaced_and_never_clipped() {
        let app = app_with(Page::Overview, TimeRange::All, vec![test_event()]);

        let footer = |width: u16| {
            frame_text(&app, width, 24)
                .lines()
                .last()
                .unwrap_or_default()
                .to_string()
        };

        // Wide: every hint, separated rather than run together.
        let wide = footer(100);
        for hint in [
            "tab pages",
            "r/R range",
            "f find",
            "s/^s share",
            "e export",
            "q quit",
        ] {
            assert!(wide.contains(hint), "missing {hint:?} in {wide:?}");
        }
        assert!(wide.contains(" · "), "hints are separated: {wide:?}");

        // Narrow: hints drop off the end instead of being cut in half.
        let narrow = footer(40);
        assert!(narrow.contains("tab pages"), "{narrow:?}");
        assert!(!narrow.contains("qui"), "no half-drawn hint: {narrow:?}");
        for hint in FOOTER_HINTS {
            let drawn = narrow.contains(&format!("{} {}", hint.0, hint.1));
            let action_only = narrow.contains(hint.1);
            assert!(
                drawn || !action_only,
                "{} appears without its key: {narrow:?}",
                hint.1
            );
        }
    }

    #[test]
    fn the_read_time_rides_the_rule_and_leaves_the_footer_to_the_hints() {
        let app = app_with(Page::Models, TimeRange::All, vec![test_event()]);
        let frame = frame_text(&app, 100, 24);
        let mut lines = frame.lines();

        let rule = lines.next().unwrap_or_default().to_string();
        let footer = lines.last().unwrap_or_default().to_string();

        assert!(
            rule.contains("Loaded "),
            "the rule carries the time: {rule:?}"
        );
        assert!(
            rule.matches('─').count() < rule.chars().count(),
            "the rule is shortened to make room rather than overlapping: {rule:?}"
        );
        assert!(
            !footer.contains("Loaded"),
            "the footer is left to the hints: {footer:?}"
        );
    }

    #[test]
    fn the_share_and_export_of_the_overview_are_all_time() {
        let app = app_with(Page::Overview, TimeRange::Last7Days, vec![test_event()]);

        assert_eq!(app.effective_range(), TimeRange::All);
        assert!(
            share_card_file_name(app.effective_range(), 1_700_000_000).starts_with("oc-stats-all-"),
            "the overview's artifacts are named for all time"
        );
        assert!(
            app.current_page_summary().starts_with("oc-stats All time"),
            "and the shared summary says all time: {}",
            app.current_page_summary()
        );
    }

    #[test]
    fn the_configured_page_is_the_one_the_first_frame_draws() {
        // State says which page is open; the drawn frame proves that is the page on
        // screen, which is what "the app opens on it" means.
        let app = App::new(
            test_data(vec![test_event()]),
            test_pricing(),
            Theme::builtin_dark(),
            ZeroCostBehavior::KeepZero,
            no_input_options(),
            Page::Sessions,
            TimeRange::Last7Days,
        );

        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let drawn = terminal.backend().buffer().clone();

        let text: String = drawn.content().iter().map(|cell| cell.symbol()).collect();
        assert!(
            text.contains("Sessions"),
            "the tab bar must show the configured page: {text}"
        );
        // The session from the event is listed, which only the Sessions page renders.
        assert!(
            text.contains("Session ses_1"),
            "the Sessions page must be the one drawn: {text}"
        );
        assert!(
            text.contains("Active days"),
            "the session detail pane must be the one drawn: {text}"
        );
        assert!(
            text.contains("7d"),
            "the configured range must be the one highlighted: {text}"
        );
    }

    #[test]
    fn opens_on_the_configured_page_and_range() {
        // What `[app] page` and `[app] range` resolve to is what the app opens with, and
        // the first snapshot is built for that range rather than for all time.
        let old = Local::now() - chrono::Duration::days(40);
        let mut event = test_event();
        event.created_at = Some(old);
        event.session_started_at = Some(old);

        let app = App::new(
            test_data(vec![event]),
            test_pricing(),
            Theme::builtin_dark(),
            ZeroCostBehavior::KeepZero,
            no_input_options(),
            Page::Sessions,
            TimeRange::Last7Days,
        );

        assert_eq!(app.page, Page::Sessions);
        assert_eq!(app.range, TimeRange::Last7Days);
        assert!(
            app.snapshot.models.is_empty(),
            "the first snapshot must be built for the configured range, not for all time"
        );
    }

    #[test]
    fn reload_replaces_snapshot_and_preserves_view_state() {
        let mut app = test_app(test_data(Vec::new()), no_input_options());
        assert!(app.snapshot.models.is_empty());
        app.page = Page::Models;
        app.range = TimeRange::Last7Days;
        app.search = Some(SearchState::new(vec!["gpt-5".to_string()], 0));
        let previous_read = app.last_read;

        app.finish_reload(test_data(vec![test_event()]));

        assert_eq!(app.snapshot.models.len(), 1);
        assert_eq!(app.data.events.len(), 1);
        assert_eq!(app.page, Page::Models);
        assert_eq!(app.range, TimeRange::Last7Days);
        assert_eq!(app.focused_index, 0);
        assert!(app.search.is_some());
        assert!(app.last_read >= previous_read);
        let frame = frame_text(&app, 100, 24);
        assert!(
            frame.lines().next().unwrap_or_default().contains("Loaded"),
            "the rule carries the read time: {frame}"
        );
        assert!(
            frame.contains("Reloaded usage data from disk"),
            "the reload notice takes the footer: {frame}"
        );

        // Once the notice expires, the hints are back in the footer.
        app.status_message = None;
        let frame = frame_text(&app, 100, 24);
        assert!(
            frame.contains("tab") && frame.contains("pages"),
            "the hints return: {frame}"
        );
    }

    #[test]
    fn reload_failure_keeps_old_snapshot() {
        use crate::db::errors::Error;

        let mut app = test_app(test_data(vec![test_event()]), no_input_options());
        assert_eq!(app.snapshot.models.len(), 1);

        // A failure must not depend on which files exist on the test machine: any bad
        // database path falls through to the real database when one is present, so the
        // error is constructed directly instead.
        app.apply_reload(Err(Error::database_not_found(vec![])));

        assert_eq!(app.snapshot.models.len(), 1);
        assert_eq!(app.data.events.len(), 1);
        let frame = frame_text(&app, 100, 24);
        assert!(frame.contains("Reload failed"), "footer was: {frame}");
        assert!(
            frame.lines().next().unwrap_or_default().contains("Loaded"),
            "the read time stays on the rule: {frame}"
        );
    }

    #[test]
    fn reload_key_is_wired_regardless_of_database() {
        let mut app = test_app(test_data(vec![test_event()]), no_input_options());

        app.handle_key(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::empty()));

        // With a database present this reloads real data, without one it reports the
        // failure; either way the key reaches reload and the rule keeps its timestamp.
        let frame = frame_text(&app, 100, 24);
        assert!(
            frame.lines().next().unwrap_or_default().contains("Loaded"),
            "{frame}"
        );
    }

    use crate::analytics::build_snapshot;
    use crate::test_support::unique_temp_dir;
    use std::path::PathBuf;

    fn empty_snapshot() -> crate::analytics::AnalyticsSnapshot {
        let data = AppData {
            events: Vec::new(),
            messages: Vec::new(),
            session_records: Vec::new(),
            import_stats: ImportStats::default(),
        };
        let pricing = PricingCatalog {
            models: BTreeMap::new(),
            cache_path: PathBuf::from("/tmp/models.json"),
            refresh_needed: false,
            availability: PricingAvailability::Empty,
            load_notice: None,
        };
        build_snapshot(&data, &pricing, TimeRange::All, ZeroCostBehavior::KeepZero)
    }

    #[test]
    fn export_uses_timestamped_names_for_both_formats() {
        let snapshot = empty_snapshot();
        let documents = export_documents(&snapshot, TimeRange::Last7Days, 1_700_000_000);

        assert_eq!(documents.len(), 2);
        assert_eq!(documents[0].0, "oc-stats-7d-1700000000.csv");
        assert_eq!(documents[1].0, "oc-stats-7d-1700000000.json");
    }

    #[test]
    fn the_share_card_sorts_beside_the_exports() {
        let range = TimeRange::Last7Days;
        let timestamp = 1_700_000_000;
        let shared = share_card_file_name(range, timestamp);
        let exported = export_documents(&empty_snapshot(), range, timestamp);

        assert_eq!(shared, "oc-stats-7d-1700000000.png");
        assert_eq!(
            shared.trim_end_matches(".png"),
            exported[0].0.trim_end_matches(".csv"),
            "one base name for everything a run writes"
        );
    }

    #[test]
    fn export_content_matches_the_stats_serializers() {
        let snapshot = empty_snapshot();
        let documents = export_documents(&snapshot, TimeRange::All, 1_700_000_000);

        assert_eq!(
            documents[0].1,
            crate::stats_output::stats_csv(&snapshot, TimeRange::All),
        );
        assert!(documents[1].1.contains("\"range\": \"all\""));

        let dir = unique_temp_dir("export-test");
        std::fs::create_dir_all(&dir).unwrap();
        for (filename, content) in &documents {
            std::fs::write(dir.join(filename), content).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(dir.join(&documents[0].0)).unwrap(),
            documents[0].1,
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(&documents[1].0)).unwrap(),
            documents[1].1,
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
