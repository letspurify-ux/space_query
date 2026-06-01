use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};

use crate::db::ConnectionInfo;
use crate::utils::arithmetic::safe_div;
use crate::utils::{logging, AppConfig};
use crate::version;

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(250);
const LOG_SCROLL_STEP: u16 = 1;
const LOG_PAGE_SCROLL_STEP: u16 = 8;

type TerminalBackend = CrosstermBackend<Stdout>;
type TerminalHandle = Terminal<TerminalBackend>;

pub struct TerminalApp {
    state: AppState,
}

struct AppState {
    config: AppConfig,
    logs: Vec<logging::LogEntry>,
    selected_connection: usize,
    focus: Focus,
    log_scroll: u16,
    crash_scroll: u16,
    crash_report: Option<String>,
    show_help: bool,
    show_crash_report: bool,
    should_quit: bool,
    status_line: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Connections,
    Logs,
}

struct TerminalSession {
    terminal: TerminalHandle,
}

impl TerminalSession {
    fn start() -> io::Result<Self> {
        enable_raw_mode()?;

        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;

        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;
        terminal.hide_cursor()?;

        Ok(Self { terminal })
    }

    fn terminal_mut(&mut self) -> &mut TerminalHandle {
        &mut self.terminal
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

impl TerminalApp {
    pub fn new(config: AppConfig, crash_report: Option<String>) -> Self {
        Self {
            state: AppState::new(config, crash_report),
        }
    }

    pub fn run(&mut self) -> io::Result<()> {
        let mut terminal = TerminalSession::start()?;

        loop {
            terminal
                .terminal_mut()
                .draw(|frame| self.state.render(frame))?;

            if self.state.should_quit {
                break;
            }

            if event::poll(EVENT_POLL_INTERVAL)? {
                if let Event::Key(key) = event::read()? {
                    self.state.handle_key_event(key);
                }
            } else {
                self.state.refresh_logs();
            }
        }

        Ok(())
    }
}

impl AppState {
    fn new(config: AppConfig, crash_report: Option<String>) -> Self {
        let selected_connection = resolve_selected_connection(&config);
        let logs = logging::get_log_entries();
        let show_crash_report = crash_report.is_some();
        let status_line = if show_crash_report {
            "이전 크래시 리포트가 있습니다. x 로 열고 Esc 로 닫습니다.".to_string()
        } else {
            "Tab 으로 패널 전환, q 로 종료합니다.".to_string()
        };

        Self {
            config,
            logs,
            selected_connection,
            focus: Focus::Connections,
            log_scroll: 0,
            crash_scroll: 0,
            crash_report,
            show_help: false,
            show_crash_report,
            should_quit: false,
            status_line,
        }
    }

    fn render(&self, frame: &mut ratatui::Frame<'_>) {
        let root = frame.area();
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(12),
                Constraint::Length(2),
            ])
            .split(root);

        self.render_header(frame, sections[0]);
        self.render_body(frame, sections[1]);
        self.render_footer(frame, sections[2]);

        if self.show_help {
            self.render_help_popup(frame);
        }

        if self.show_crash_report {
            self.render_crash_popup(frame);
        }
    }

    fn render_header(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let title = format!(
            "SPACE Query {} | ratatui migration shell",
            version::display_version()
        );
        let subtitle = format!(
            "saved connections: {} | pool size: {} | auto commit: {}",
            self.config.recent_connections.len(),
            self.config.normalized_connection_pool_size(),
            if self.config.auto_commit { "on" } else { "off" }
        );
        let last_connection = self.config.last_connection.as_deref().unwrap_or("none");

        let text = Text::from(vec![
            Line::from(Span::styled(
                title,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled("last connection: ", Style::default().fg(Color::DarkGray)),
                Span::raw(last_connection),
                Span::raw(" | "),
                Span::raw(subtitle),
            ]),
        ]);

        let header = Paragraph::new(text)
            .block(block("Overview", false))
            .wrap(Wrap { trim: true });
        frame.render_widget(header, area);
    }

    fn render_body(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(32), Constraint::Percentage(68)])
            .split(area);

        self.render_connections(frame, columns[0]);

        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(13), Constraint::Min(8)])
            .split(columns[1]);

        self.render_connection_details(frame, right[0]);
        self.render_logs(frame, right[1]);
    }

    fn render_connections(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let items = if self.config.recent_connections.is_empty() {
            vec![ListItem::new("저장된 연결이 없습니다.")]
        } else {
            self.config
                .recent_connections
                .iter()
                .map(|connection| {
                    let marker = if self.config.last_connection.as_deref()
                        == Some(connection.name.as_str())
                    {
                        "●"
                    } else {
                        " "
                    };
                    let label = format!("{} {} [{}]", marker, connection.name, connection.db_type);
                    ListItem::new(label)
                })
                .collect::<Vec<_>>()
        };

        let mut state = ListState::default();
        if !self.config.recent_connections.is_empty() {
            state.select(Some(self.selected_connection));
        }

        let list = List::new(items)
            .block(block("Connections", self.focus == Focus::Connections))
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("› ");

        frame.render_stateful_widget(list, area, &mut state);
    }

    fn render_connection_details(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let text = match self.selected_connection() {
            Some(connection) => Text::from(vec![
                detail_line("name", connection.name.clone()),
                detail_line("database", connection.db_type.to_string()),
                detail_line("username", connection.username.clone()),
                detail_line("host", connection.host.clone()),
                detail_line("port", connection.port.to_string()),
                detail_line("service", display_service_name(connection)),
                detail_line(
                    "selected",
                    if self.config.last_connection.as_deref() == Some(connection.name.as_str()) {
                        "yes"
                    } else {
                        "no"
                    },
                ),
                Line::from(""),
                Line::from(Span::styled(
                    "Migration note",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from("현재 SQL 편집기/결과 그리드는 ratatui로 아직 이관되지 않았습니다."),
                Line::from(
                    "이번 단계는 실행 UI를 TUI로 전환하고 설정/진단 화면을 제공하는 작업입니다.",
                ),
            ]),
            None => Text::from(vec![
                Line::from("저장된 연결이 없습니다."),
                Line::from("기존 config.json 에 저장된 최근 연결이 있으면 이 패널에 표시됩니다."),
                Line::from(""),
                Line::from("Migration note"),
                Line::from("현재 기본 실행 경로는 ratatui TUI입니다."),
            ]),
        };

        let details = Paragraph::new(text)
            .block(block("Connection Detail", false))
            .wrap(Wrap { trim: false });
        frame.render_widget(details, area);
    }

    fn render_logs(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let lines = if self.logs.is_empty() {
            vec![Line::from("로그가 없습니다.")]
        } else {
            self.logs
                .iter()
                .map(|entry| {
                    Line::from(vec![
                        Span::styled(
                            format!("[{}] ", entry.level),
                            Style::default()
                                .fg(log_level_color(entry.level))
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("{} ", entry.timestamp),
                            Style::default().fg(Color::DarkGray),
                        ),
                        Span::styled(
                            format!("{} ", entry.source),
                            Style::default().fg(Color::LightBlue),
                        ),
                        Span::raw(entry.message.as_str()),
                    ])
                })
                .collect::<Vec<_>>()
        };

        let logs = Paragraph::new(Text::from(lines))
            .block(block("Application Log", self.focus == Focus::Logs))
            .scroll((self.log_scroll, 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(logs, area);
    }

    fn render_footer(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let footer_text = Text::from(vec![
            Line::from("Tab 패널 전환 | ↑↓ 이동/스크롤 | Enter 최근 연결 선택 | r 새로고침 | c 로그 비우기 | x 크래시 리포트 | ? 도움말 | q 종료"),
            Line::from(Span::styled(
                self.status_line.as_str(),
                Style::default().fg(Color::Green),
            )),
        ]);

        let footer = Paragraph::new(footer_text)
            .block(block("Controls", false))
            .wrap(Wrap { trim: false });
        frame.render_widget(footer, area);
    }

    fn render_help_popup(&self, frame: &mut ratatui::Frame<'_>) {
        let area = centered_rect(72, 56, frame.area());
        frame.render_widget(Clear, area);

        let help = Paragraph::new(Text::from(vec![
            Line::from(Span::styled(
                "Keyboard",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from("Tab / Shift+Tab: 패널 전환"),
            Line::from("Up / Down: 연결 선택 또는 로그 스크롤"),
            Line::from("PageUp / PageDown: 로그/크래시 리포트 빠른 스크롤"),
            Line::from("Enter: 선택한 최근 연결을 현재 연결로 표시하고 config 저장"),
            Line::from("r: config + 로그 다시 로드"),
            Line::from("c: 로그 비우기"),
            Line::from("x: 이전 크래시 리포트 팝업 열기/닫기"),
            Line::from("? / Esc: 도움말 닫기"),
            Line::from("q / Ctrl+C: 종료"),
            Line::from(""),
            Line::from(Span::styled(
                "Scope",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from("이번 단계는 FLTK 메인 윈도우를 ratatui TUI로 교체하는 1차 전환입니다."),
            Line::from(
                "SQL 편집기, 결과 그리드, 오브젝트 브라우저는 아직 TUI로 이관되지 않았습니다.",
            ),
        ]))
        .block(block("Help", false))
        .wrap(Wrap { trim: false });
        frame.render_widget(help, area);
    }

    fn render_crash_popup(&self, frame: &mut ratatui::Frame<'_>) {
        let area = centered_rect(84, 72, frame.area());
        frame.render_widget(Clear, area);

        let text = self
            .crash_report
            .as_deref()
            .unwrap_or("표시할 이전 크래시 리포트가 없습니다.");
        let popup = Paragraph::new(text)
            .block(block("Previous Crash Report", false))
            .scroll((self.crash_scroll, 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(popup, area);
    }

    fn handle_key_event(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        if self.show_help {
            match key.code {
                KeyCode::Esc | KeyCode::Char('?') => {
                    self.show_help = false;
                    self.set_status("도움말을 닫았습니다.");
                }
                KeyCode::Char('q') => self.should_quit = true,
                _ => {}
            }
            return;
        }

        if self.show_crash_report {
            match key.code {
                KeyCode::Esc | KeyCode::Char('x') => {
                    self.show_crash_report = false;
                    self.crash_scroll = 0;
                    self.set_status("크래시 리포트를 닫았습니다.");
                }
                KeyCode::Up => {
                    self.crash_scroll = self.crash_scroll.saturating_sub(LOG_SCROLL_STEP);
                }
                KeyCode::Down => {
                    self.crash_scroll = self.crash_scroll.saturating_add(LOG_SCROLL_STEP);
                }
                KeyCode::PageUp => {
                    self.crash_scroll = self.crash_scroll.saturating_sub(LOG_PAGE_SCROLL_STEP);
                }
                KeyCode::PageDown => {
                    self.crash_scroll = self.crash_scroll.saturating_add(LOG_PAGE_SCROLL_STEP);
                }
                KeyCode::Home => self.crash_scroll = 0,
                KeyCode::Char('q') => self.should_quit = true,
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Tab | KeyCode::Right => self.focus = self.focus.next(),
            KeyCode::BackTab | KeyCode::Left => self.focus = self.focus.previous(),
            KeyCode::Up => self.move_focus_cursor(-1),
            KeyCode::Down => self.move_focus_cursor(1),
            KeyCode::Home => self.move_to_start(),
            KeyCode::End => self.move_to_end(),
            KeyCode::PageUp => self.page_scroll(-1),
            KeyCode::PageDown => self.page_scroll(1),
            KeyCode::Enter => self.select_current_connection(),
            KeyCode::Char('r') => self.reload(),
            KeyCode::Char('c') => self.clear_log(),
            KeyCode::Char('x') => self.toggle_crash_report(),
            KeyCode::Char('?') => self.show_help = true,
            _ => {}
        }
    }

    fn refresh_logs(&mut self) {
        self.logs = logging::get_log_entries();
    }

    fn reload(&mut self) {
        self.config = AppConfig::load();
        self.logs = logging::get_log_entries();
        self.selected_connection = resolve_selected_connection(&self.config);
        self.log_scroll = 0;
        self.crash_scroll = 0;
        self.set_status("config 와 로그를 다시 불러왔습니다.");
    }

    fn clear_log(&mut self) {
        match logging::clear_log() {
            Ok(()) => {
                self.logs.clear();
                self.log_scroll = 0;
                self.set_status("앱 로그를 비웠습니다.");
            }
            Err(err) => {
                self.set_status(format!("로그 비우기 실패: {err}"));
            }
        }
    }

    fn toggle_crash_report(&mut self) {
        if self.crash_report.is_some() {
            self.show_crash_report = !self.show_crash_report;
            self.crash_scroll = 0;
            self.set_status(if self.show_crash_report {
                "크래시 리포트를 표시합니다."
            } else {
                "크래시 리포트를 닫았습니다."
            });
        } else {
            self.set_status("이전 크래시 리포트가 없습니다.");
        }
    }

    fn move_focus_cursor(&mut self, delta: isize) {
        match self.focus {
            Focus::Connections => {
                if self.config.recent_connections.is_empty() {
                    return;
                }

                let last_index = self.config.recent_connections.len().saturating_sub(1);
                let next = self
                    .selected_connection
                    .saturating_add_signed(delta)
                    .min(last_index);
                self.selected_connection = next;
            }
            Focus::Logs => {
                if delta.is_negative() {
                    self.log_scroll = self.log_scroll.saturating_sub(LOG_SCROLL_STEP);
                } else {
                    self.log_scroll = self.log_scroll.saturating_add(LOG_SCROLL_STEP);
                }
            }
        }
    }

    fn move_to_start(&mut self) {
        match self.focus {
            Focus::Connections => self.selected_connection = 0,
            Focus::Logs => self.log_scroll = 0,
        }
    }

    fn move_to_end(&mut self) {
        match self.focus {
            Focus::Connections => {
                self.selected_connection = self.config.recent_connections.len().saturating_sub(1);
            }
            Focus::Logs => {
                self.log_scroll = self.logs.len().min(u16::MAX as usize) as u16;
            }
        }
    }

    fn page_scroll(&mut self, direction: isize) {
        match self.focus {
            Focus::Connections => {
                let step = 5isize * direction.signum();
                self.move_focus_cursor(step);
            }
            Focus::Logs => {
                if direction.is_negative() {
                    self.log_scroll = self.log_scroll.saturating_sub(LOG_PAGE_SCROLL_STEP);
                } else {
                    self.log_scroll = self.log_scroll.saturating_add(LOG_PAGE_SCROLL_STEP);
                }
            }
        }
    }

    fn select_current_connection(&mut self) {
        let Some(connection) = self.selected_connection().cloned() else {
            self.set_status("선택할 최근 연결이 없습니다.");
            return;
        };

        self.config.last_connection = Some(connection.name.clone());
        match self.config.save() {
            Ok(()) => {
                logging::log_info(
                    "tui",
                    &format!("Selected recent connection '{}'", connection.name),
                );
                self.set_status(format!(
                    "최근 연결 '{}' 을(를) 현재 연결로 표시했습니다.",
                    connection.name
                ));
            }
            Err(err) => {
                self.set_status(format!("연결 선택 저장 실패: {err}"));
            }
        }
    }

    fn selected_connection(&self) -> Option<&ConnectionInfo> {
        self.config.recent_connections.get(self.selected_connection)
    }

    fn set_status(&mut self, message: impl Into<String>) {
        self.status_line = message.into();
    }
}

impl Focus {
    fn next(self) -> Self {
        match self {
            Self::Connections => Self::Logs,
            Self::Logs => Self::Connections,
        }
    }

    fn previous(self) -> Self {
        self.next()
    }
}

fn resolve_selected_connection(config: &AppConfig) -> usize {
    if config.recent_connections.is_empty() {
        return 0;
    }

    config
        .last_connection
        .as_deref()
        .and_then(|name| {
            config
                .recent_connections
                .iter()
                .position(|connection| connection.name == name)
        })
        .unwrap_or(0)
}

fn display_service_name(connection: &ConnectionInfo) -> String {
    if connection.uses_oracle_tns_alias() {
        format!("{} (TNS alias)", connection.service_name)
    } else {
        connection.service_name.clone()
    }
}

fn detail_line(label: &str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:>9}: "),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(value.into()),
    ])
}

fn log_level_color(level: logging::LogLevel) -> Color {
    match level {
        logging::LogLevel::Debug => Color::Gray,
        logging::LogLevel::Info => Color::Green,
        logging::LogLevel::Warning => Color::Yellow,
        logging::LogLevel::Error => Color::Red,
    }
}

fn block(title: &str, focused: bool) -> Block<'static> {
    let border_style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    Block::default()
        .title(Span::styled(
            title.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(border_style)
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(safe_div(100 - percent_y, 2)),
            Constraint::Percentage(percent_y),
            Constraint::Percentage(safe_div(100 - percent_y, 2)),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(safe_div(100 - percent_x, 2)),
            Constraint::Percentage(percent_x),
            Constraint::Percentage(safe_div(100 - percent_x, 2)),
        ])
        .split(popup_layout[1])[1]
}

#[cfg(test)]
mod tests {
    use super::{display_service_name, resolve_selected_connection, AppState};
    use crate::db::{ConnectionInfo, DatabaseType};
    use crate::utils::AppConfig;

    fn sample_connection(name: &str, db_type: DatabaseType) -> ConnectionInfo {
        ConnectionInfo {
            name: name.to_string(),
            username: "tester".to_string(),
            password: String::new(),
            host: "localhost".to_string(),
            port: db_type.connection_form_spec().default_port,
            service_name: "FREE".to_string(),
            db_type,
            advanced: crate::db::ConnectionAdvancedSettings::default_for(db_type),
            debug_oracle_thin_protocol_version: None,
        }
    }

    #[test]
    fn resolve_selected_connection_prefers_last_connection_name() {
        let mut config = AppConfig::new();
        config.recent_connections = vec![
            sample_connection("oracle-dev", DatabaseType::Oracle),
            sample_connection("mysql-dev", DatabaseType::MySQL),
        ];
        config.last_connection = Some("mysql-dev".to_string());

        assert_eq!(resolve_selected_connection(&config), 1);
    }

    #[test]
    fn resolve_selected_connection_falls_back_to_first_entry() {
        let mut config = AppConfig::new();
        config.recent_connections = vec![sample_connection("oracle-dev", DatabaseType::Oracle)];
        config.last_connection = Some("missing".to_string());

        assert_eq!(resolve_selected_connection(&config), 0);
    }

    #[test]
    fn app_state_starts_with_crash_popup_when_report_exists() {
        let state = AppState::new(AppConfig::new(), Some("panic".to_string()));

        assert!(state.show_crash_report);
    }

    #[test]
    fn display_service_name_marks_tns_alias_connections() {
        let connection = ConnectionInfo {
            name: "oracle-tns".to_string(),
            username: "tester".to_string(),
            password: String::new(),
            host: String::new(),
            port: 1521,
            service_name: "FREE".to_string(),
            db_type: DatabaseType::Oracle,
            advanced: crate::db::ConnectionAdvancedSettings::default_for(DatabaseType::Oracle),
            debug_oracle_thin_protocol_version: None,
        };

        assert_eq!(display_service_name(&connection), "FREE (TNS alias)");
    }
}
