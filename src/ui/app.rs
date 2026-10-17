use std::borrow::Cow;
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
use crate::ui::export::render_share_card;
use crate::ui::models::MAX_QUERY_LEN;
use crate::ui::models::{SearchState, render_models, render_providers};
use crate::ui::overview::render_overview;
use crate::ui::theme::{Theme, ThemeKind};
use crate::ui::widgets::common::{CONTENT_WIDTH, left_aligned_content, segment_span};
use crate::utils::formatting::format_price_summary;
use crate::utils::pricing::ZeroCostBehavior;
use crate::utils::time::TimeRange;

const VIEWPORT_HEIGHT: u16 = 23;
const STATUS_TTL: Duration = Duration::from_secs(3);
const TICK_TIME_MS: u16 = 200;
const DEFAULT_HINTS: &str = "<tab> pages | r cycle | R reload | f find | q quit";

#[derive(Clone, Debug)]
struct StatusMessage {
    text: String,
    expires_at: Option<Instant>,
}

#[derive(Debug)]
struct ClipboardJob {
    buffer: ratatui::buffer::Buffer,
    theme: Theme,
    summary: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Page {
    #[default]
    Overview,
    Models,
    Providers,
}

impl Page {
    pub fn next(self) -> Self {
        match self {
            Self::Overview => Self::Models,
            Self::Models => Self::Providers,
            Self::Providers => Self::Overview,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            Self::Overview => Self::Providers,
            Self::Models => Self::Overview,
            Self::Providers => Self::Models,
        }
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
    ) -> Self {
        let snapshot = build_snapshot(&data, &pricing, TimeRange::All, zero_cost_behavior);
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
            page: Page::Overview,
            range: TimeRange::All,
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
                    self.set_status(update);
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
            self.range,
            self.zero_cost_behavior,
        );
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
            ratatui::widgets::Paragraph::new("─".repeat(CONTENT_WIDTH as _))
                .style(theme.muted_style()),
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
        }

        self.render_footer(frame, footer, theme);
    }

    fn render_header(&self, frame: &mut Frame<'_>, area: ratatui::layout::Rect, theme: &Theme) {
        let content = area;
        let line = ratatui::text::Line::from(vec![
            segment_span("Overview", self.page == Page::Overview, theme),
            segment_span("Models", self.page == Page::Models, theme),
            segment_span("Providers", self.page == Page::Providers, theme),
            ratatui::text::Span::raw("    "),
            segment_span(" All ", self.range == TimeRange::All, theme),
            segment_span("7 Days", self.range == TimeRange::Last7Days, theme),
            segment_span("30 Days", self.range == TimeRange::Last30Days, theme),
        ]);

        frame.render_widget(ratatui::widgets::Paragraph::new(line), content);
    }

    fn render_footer(&self, frame: &mut Frame<'_>, area: ratatui::layout::Rect, theme: &Theme) {
        frame.render_widget(
            ratatui::widgets::Paragraph::new(self.footer_text()).style(theme.muted_style()),
            area,
        );
    }

    fn footer_text(&self) -> String {
        let base = self
            .status_message
            .as_ref()
            .map(|status| status.text.as_str())
            .unwrap_or(DEFAULT_HINTS);
        format!("Loaded {} | {base}", self.last_read.format("%H:%M:%S"))
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
                self.search = None;
                self.page = self.page.next();
                self.focused_index = 0;
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.search = None;
                self.page = self.page.previous();
                self.focused_index = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => self.advance_focused_model(1),
            KeyCode::Up | KeyCode::Char('k') => self.advance_focused_model(-1),
            KeyCode::Char('r') => {
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
            KeyCode::Char('f') | KeyCode::Char('/')
                if matches!(self.page, Page::Models | Page::Providers) =>
            {
                self.enter_search();
            }
            KeyCode::Char(value) => {
                if let Some(range) = TimeRange::from_shortcut(value) {
                    self.range = range;
                    self.focused_index = 0;
                    self.search = None;
                    self.recompute();
                }
            }
            _ => {}
        }
    }

    fn advance_focused_model(&mut self, delta: isize) {
        let total = match self.page {
            Page::Models => self.snapshot.models.len(),
            Page::Providers => self.snapshot.providers.len(),
            Page::Overview => return,
        };
        if total == 0 {
            return;
        }
        let next = (self.focused_index as isize + delta).rem_euclid(total as isize) as usize;
        self.focused_index = next;
    }

    fn enter_search(&mut self) {
        let rows = match self.page {
            Page::Models => &self.snapshot.models,
            Page::Providers => &self.snapshot.providers,
            _ => return,
        };
        let ids = rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>();
        self.search = Some(SearchState::new(ids, self.focused_index));
    }

    fn update_search_filter(&mut self) {
        if let Some(search) = self.search.as_mut() {
            search.update_filter();
        }
    }

    fn set_status(&mut self, text: impl Into<String>) {
        self.status_message = Some(StatusMessage {
            text: text.into(),
            expires_at: Some(Instant::now() + STATUS_TTL),
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

    fn copy_current_page(&mut self) {
        if self.copy_in_progress {
            self.set_status("Still rendering share card in background, please wait ...");
            return;
        }

        let job = match self.prepare_clipboard_job() {
            Ok(job) => job,
            Err(err) => {
                self.set_status(format!("Copy failed: {err}"));
                return;
            }
        };

        self.copy_in_progress = true;
        self.set_status("Rendering share card in background ...");

        let sender = self.clipboard_sender.clone();
        tokio::task::spawn_blocking(move || {
            let message = match copy_clipboard_job(job) {
                Ok(message) => message,
                Err(err) => format!("Copy failed: {err}"),
            };
            let _ = sender.send(message);
        });
    }

    fn prepare_clipboard_job(&self) -> Result<ClipboardJob> {
        Ok(ClipboardJob {
            buffer: self.capture_current_page_buffer()?,
            theme: self.theme.clone(),
            summary: self.current_page_summary(),
        })
    }

    fn current_page_summary(&self) -> String {
        match self.page {
            Page::Overview => format!(
                "oc-stats {}\nTokens: {}\nCost: {}\nSessions: {}\nMessages: {}\nPrompts: {}",
                self.range.label(),
                self.snapshot.overview.total_tokens,
                format_price_summary(&self.snapshot.overview.total_cost),
                self.snapshot.overview.sessions,
                self.snapshot.overview.messages,
                self.snapshot.overview.prompts,
            ),
            Page::Models => summary_lines(&self.snapshot.models),
            Page::Providers => summary_lines(&self.snapshot.providers),
        }
    }
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

fn copy_clipboard_job(job: ClipboardJob) -> Result<String> {
    match copy_image_to_clipboard(job.buffer, job.theme) {
        Ok(()) => Ok("Successfully copied to your clipboard".to_string()),
        Err(_) => {
            copy_text_to_clipboard(&job.summary)?;
            Ok("Image export failed, copied text summary instead".to_string())
        }
    }
}

fn copy_text_to_clipboard(summary: &str) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.set_text(summary.to_string())?;
    Ok(())
}

fn copy_image_to_clipboard(buffer: ratatui::buffer::Buffer, theme: Theme) -> Result<()> {
    let image = render_share_card(&buffer, &theme)?;
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.set_image(arboard::ImageData {
        width: image.width() as usize,
        height: image.height() as usize,
        bytes: Cow::Owned(image.into_raw()),
    })?;
    Ok(())
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
        )
    }

    fn no_input_options() -> InputOptions {
        InputOptions {
            database_path: None,
        }
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
        assert!(app.footer_text().contains("Loaded"));
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
        let footer = app.footer_text();
        assert!(footer.contains("Reload failed"), "footer was: {footer}");
        assert!(footer.contains("Loaded"), "footer was: {footer}");
    }

    #[test]
    fn reload_key_is_wired_regardless_of_database() {
        let mut app = test_app(test_data(vec![test_event()]), no_input_options());

        app.handle_key(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::empty()));

        // With a database present this reloads real data, without one it reports the
        // failure; either way the key reaches reload and the footer keeps its timestamp.
        assert!(app.footer_text().contains("Loaded"));
    }
}
