use std::io::{self, IsTerminal, Read, Stdout, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use orcas_core::ipc;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::service::SupervisorService;

const HUD_TOP_HEIGHT: u16 = 5;
const HUD_BOTTOM_HEIGHT: u16 = 4;
const HUD_FADE_STEP: f32 = 0.22;

#[derive(Debug, Clone)]
enum SessionStatus {
    Starting,
    Running,
    Exited(String),
    Failed(String),
}

impl SessionStatus {
    fn label(&self) -> String {
        match self {
            Self::Starting => "starting".to_string(),
            Self::Running => "running".to_string(),
            Self::Exited(status) => format!("exited: {status}"),
            Self::Failed(error) => format!("failed: {error}"),
        }
    }

    fn is_live(&self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }
}

struct LiveSession {
    thread_id: String,
    lane_label: String,
    cwd: PathBuf,
    parser: Arc<Mutex<vt100::Parser>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    child: Arc<Mutex<Box<dyn Child + Send>>>,
    status: Arc<Mutex<SessionStatus>>,
    started_at: Instant,
}

impl LiveSession {
    fn launch(
        service: &SupervisorService,
        thread: &ipc::ThreadSummary,
        lane_label: &str,
        cols: u16,
        rows: u16,
    ) -> Result<Self> {
        let _ = service.prepare_shared_app_server_auth()?;
        if !thread.cwd.is_empty() && Path::new(&thread.cwd).is_dir() {
            service.trust_shared_app_server_projects(&[Path::new(&thread.cwd)])?;
        }
        let lane_label = lane_label.to_string();
        let cwd = if thread.cwd.is_empty() {
            PathBuf::from(".")
        } else {
            PathBuf::from(&thread.cwd)
        };

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: rows.max(12),
                cols: cols.max(40),
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("open pty for Codex TUI")?;

        let mut command = CommandBuilder::new(service.config.codex.binary_path.clone());
        command.arg("--remote");
        command.arg(service.config.codex.effective_listen_url());
        command.arg("resume");
        command.arg(&thread.id);
        command.env("CODEX_HOME", service.shared_app_server_codex_home());
        command.env("CODEX_SQLITE_HOME", service.shared_app_server_sqlite_home());
        if Path::new(&thread.cwd).is_dir() {
            command.cwd(&thread.cwd);
        }

        let child = pair
            .slave
            .spawn_command(command)
            .context("spawn child Codex TUI")?;
        let reader = pair.master.try_clone_reader().context("clone PTY reader")?;
        let writer = pair.master.take_writer().context("take PTY writer")?;
        let master = Arc::new(Mutex::new(pair.master));
        let parser = Arc::new(Mutex::new(vt100::Parser::new(
            rows.max(12),
            cols.max(40),
            0,
        )));
        let status = Arc::new(Mutex::new(SessionStatus::Starting));

        Self::spawn_reader(reader, parser.clone(), status.clone());
        let session = Self {
            thread_id: thread.id.clone(),
            lane_label,
            cwd,
            parser,
            writer: Arc::new(Mutex::new(writer)),
            master,
            child: Arc::new(Mutex::new(child)),
            status,
            started_at: Instant::now(),
        };
        session.set_status(SessionStatus::Running);
        Ok(session)
    }

    fn spawn_reader(
        mut reader: Box<dyn Read + Send>,
        parser: Arc<Mutex<vt100::Parser>>,
        status: Arc<Mutex<SessionStatus>>,
    ) {
        thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        if let Ok(mut guard) = status.lock() {
                            if guard.is_live() {
                                *guard = SessionStatus::Exited("pty closed".to_string());
                            }
                        }
                        break;
                    }
                    Ok(len) => {
                        if let Ok(mut guard) = parser.lock() {
                            guard.process(&buffer[..len]);
                        }
                    }
                    Err(error) => {
                        if let Ok(mut guard) = status.lock() {
                            *guard = SessionStatus::Failed(format!("pty read failed: {error}"));
                        }
                        break;
                    }
                }
            }
        });
    }

    fn short_label(&self) -> String {
        let short = self.thread_id.chars().take(8).collect::<String>();
        format!("{}:{}", self.lane_label, short)
    }

    fn set_status(&self, status: SessionStatus) {
        if let Ok(mut guard) = self.status.lock() {
            *guard = status;
        }
    }

    fn status(&self) -> SessionStatus {
        self.status
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| SessionStatus::Failed("status lock poisoned".to_string()))
    }

    fn screen_text(&self) -> String {
        self.parser
            .lock()
            .map(|guard| guard.screen().contents())
            .unwrap_or_else(|_| "[session buffer unavailable]".to_string())
    }

    fn send_key(&self, key: KeyEvent) -> Result<()> {
        let bytes = key_to_bytes(key);
        if bytes.is_empty() {
            return Ok(());
        }
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow::anyhow!("session writer lock poisoned"))?;
        writer
            .write_all(&bytes)
            .context("write key event to Codex PTY")?;
        writer.flush().ok();
        Ok(())
    }

    fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        let rows = rows.max(12);
        let cols = cols.max(40);
        if let Ok(mut master) = self.master.lock() {
            let _ = master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
        Ok(())
    }

    fn poll_exit(&self) {
        if let Ok(mut child) = self.child.lock() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.set_status(SessionStatus::Exited(status.to_string()));
                }
                Ok(None) => {}
                Err(error) => {
                    self.set_status(SessionStatus::Failed(format!("exit poll failed: {error}")));
                }
            }
        }
    }

    fn terminate(&self) -> Result<()> {
        if let Ok(mut child) = self.child.lock() {
            child.kill().context("terminate Codex child")?;
            self.set_status(SessionStatus::Exited("terminated".to_string()));
        }
        Ok(())
    }
}

struct SessionTab {
    session: LiveSession,
}

impl SessionTab {
    fn title(&self) -> String {
        self.session.short_label()
    }
}

struct DashboardState {
    status: String,
    last_refresh: Instant,
    show_hud: bool,
    hud_opacity: f32,
    active_tab: Option<usize>,
    sessions: Vec<SessionTab>,
}

impl DashboardState {
    fn with_sessions(sessions: Vec<SessionTab>) -> Self {
        Self {
            status: "supervisor lane ready".to_string(),
            last_refresh: Instant::now(),
            show_hud: true,
            hud_opacity: 1.0,
            active_tab: if sessions.is_empty() { None } else { Some(0) },
            sessions,
        }
    }

    fn normalize_selection(&mut self) {
        self.normalize_active_tab();
    }

    fn normalize_active_tab(&mut self) {
        if let Some(index) = self.active_tab
            && index >= self.sessions.len()
        {
            self.active_tab = None;
        }
    }

    fn step_hud_transition(&mut self) {
        let target = if self.show_hud { 1.0 } else { 0.0 };
        let delta = target - self.hud_opacity;
        if delta.abs() < 0.02 {
            self.hud_opacity = target;
            return;
        }
        self.hud_opacity = (self.hud_opacity + delta * HUD_FADE_STEP).clamp(0.0, 1.0);
    }

    fn hud_visible(&self) -> bool {
        self.hud_opacity > 0.0
    }

    fn active_session(&self) -> Option<&SessionTab> {
        self.active_tab.and_then(|index| self.sessions.get(index))
    }

    fn next_session(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let next = match self.active_tab {
            Some(index) => (index + 1) % self.sessions.len(),
            None => 0,
        };
        self.active_tab = Some(next);
        self.status = format!("focused tab {}", self.sessions[next].title());
    }

    fn previous_session(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let prev = match self.active_tab {
            Some(0) | None => self.sessions.len().saturating_sub(1),
            Some(index) => index.saturating_sub(1),
        };
        self.active_tab = Some(prev);
        self.status = format!("focused tab {}", self.sessions[prev].title());
    }

    fn send_key_to_active_session(&self, key: KeyEvent) -> Result<()> {
        if let Some(session) = self.active_session() {
            session.session.send_key(key)?;
        }
        Ok(())
    }

    fn terminate_active_session(&mut self) -> Result<()> {
        let Some(session) = self.active_session() else {
            self.status = "no active session to terminate".to_string();
            return Ok(());
        };
        session.session.terminate()?;
        self.status = format!("terminated session {}", session.session.thread_id);
        Ok(())
    }

    fn poll_session_statuses(&self) {
        for session in &self.sessions {
            session.session.poll_exit();
        }
    }

    fn resize_sessions(&self, rows: u16, cols: u16) {
        for session in &self.sessions {
            let _ = session.session.resize(rows, cols);
        }
    }
}

pub async fn run_dashboard(service: SupervisorService) -> Result<()> {
    if !io::stdout().is_terminal() {
        bail!("orcas tui requires an interactive terminal");
    }

    let supervisor_thread = service
        .resolve_supervisor_dashboard_thread()
        .await
        .context("resolve supervisor dashboard thread")?;
    enable_raw_mode().context("enable terminal raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, Hide).context("enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("create dashboard terminal")?;

    let size = terminal.size().context("read dashboard terminal size")?;
    let supervisor_session = LiveSession::launch(
        &service,
        &supervisor_thread,
        "supervisor",
        size.width,
        size.height,
    )?;
    let state = DashboardState::with_sessions(vec![SessionTab {
        session: supervisor_session,
    }]);

    let result = run_dashboard_loop(&mut terminal, state).await;

    cleanup_terminal(&mut terminal);
    result
}

async fn run_dashboard_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    mut state: DashboardState,
) -> Result<()> {
    let refresh_interval = Duration::from_millis(750);

    loop {
        state.normalize_selection();
        state.poll_session_statuses();
        state.step_hud_transition();
        let size = terminal.size().context("read terminal size")?;
        let root = Rect::new(0, 0, size.width, size.height);
        let hud_layout = state.hud_visible().then(|| border_hud_layout(root));
        state.resize_sessions(root.height, root.width);

        terminal
            .draw(|frame| render_dashboard_frame_internal(frame, &state, hud_layout))
            .context("render Orcas dashboard")?;

        if state.last_refresh.elapsed() >= refresh_interval {
            state.poll_session_statuses();
            state.last_refresh = Instant::now();
            continue;
        }

        if let Some(key) = poll_key_event(Duration::from_millis(125)).await? {
            match key.code {
                KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    state.status = "wrapper closed".to_string();
                    break;
                }
                KeyCode::F(2) => {
                    state.show_hud = !state.show_hud;
                    state.status = if state.show_hud {
                        "opened border HUD".to_string()
                    } else {
                        "closed border HUD".to_string()
                    };
                }
                KeyCode::F(5) => {
                    state.poll_session_statuses();
                    state.last_refresh = Instant::now();
                    state.status = "refreshed session status".to_string();
                }
                KeyCode::F(6) => {
                    state.next_session();
                }
                KeyCode::F(7) => {
                    state.previous_session();
                }
                KeyCode::F(8) => {
                    state.terminate_active_session()?;
                }
                _ if state.show_hud => match key.code {
                    KeyCode::Esc => {
                        state.show_hud = false;
                        state.status = "closed HUD".to_string();
                    }
                    _ => {}
                },
                _ => {
                    if state.active_session().is_some() {
                        state.send_key_to_active_session(key)?;
                    }
                }
            }
        }
    }

    Ok(())
}

fn render_dashboard_frame_internal(
    frame: &mut ratatui::Frame<'_>,
    state: &DashboardState,
    hud_layout: Option<BorderHudLayout>,
) {
    let root = frame.size();
    render_main(frame, root, state);
    if let Some(layout) = hud_layout {
        render_border_hud(frame, layout, state);
    } else if !state.show_hud && state.active_session().is_none() {
        render_canvas_hint(frame, root);
    }
}

#[derive(Clone, Copy)]
struct BorderHudLayout {
    top: Rect,
    bottom: Rect,
}

fn border_hud_layout(area: Rect) -> BorderHudLayout {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(HUD_TOP_HEIGHT),
            Constraint::Min(1),
            Constraint::Length(HUD_BOTTOM_HEIGHT),
        ])
        .split(area);
    BorderHudLayout {
        top: rows[0],
        bottom: rows[2],
    }
}

fn hud_style(opacity: f32) -> Style {
    let opacity = opacity.clamp(0.0, 1.0);
    let shade = (78.0 + (opacity * 177.0)).round() as u8;
    let mut style = Style::default().fg(Color::Rgb(shade, shade, shade));
    if opacity < 0.35 {
        style = style.add_modifier(Modifier::DIM);
    }
    if opacity > 0.75 {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

fn hud_accent_style(opacity: f32) -> Style {
    let opacity = opacity.clamp(0.0, 1.0);
    let shade = (96.0 + (opacity * 159.0)).round() as u8;
    Style::default()
        .fg(Color::Rgb(shade, shade, 255))
        .add_modifier(if opacity > 0.5 {
            Modifier::BOLD
        } else {
            Modifier::DIM
        })
}

fn render_main(frame: &mut ratatui::Frame<'_>, area: Rect, state: &DashboardState) {
    if let Some(session) = state.active_session() {
        let text = session.session.screen_text();
        let block = Block::default().borders(Borders::NONE).title(format!(
            "Codex Session: {} ({})",
            session.session.lane_label, session.session.thread_id
        ));
        let paragraph = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
        frame.render_widget(paragraph, area);
        return;
    }

    if state.show_hud {
        frame.render_widget(Paragraph::new(""), area);
    } else {
        render_canvas_hint(frame, area);
    }
}

fn render_canvas_hint(frame: &mut ratatui::Frame<'_>, area: Rect) {
    let hint = Paragraph::new(Line::from(vec![Span::styled(
        "press F2 for HUD",
        Style::default()
            .fg(Color::Rgb(180, 180, 180))
            .add_modifier(Modifier::DIM),
    )]))
    .alignment(Alignment::Center)
    .block(Block::default().borders(Borders::NONE));
    frame.render_widget(hint, area);
}

fn render_border_hud(
    frame: &mut ratatui::Frame<'_>,
    layout: BorderHudLayout,
    state: &DashboardState,
) {
    let hud_border = hud_style(state.hud_opacity);
    let hud_accent = hud_accent_style(state.hud_opacity);
    let title = Line::from(vec![
        Span::styled(" Orcas TUI ", hud_accent),
        Span::raw(" | supervisor lane"),
    ]);
    let shortcuts = Line::from(vec![
        Span::styled("Ctrl+q", hud_accent),
        Span::raw(" quit  "),
        Span::styled("F2", hud_accent),
        Span::raw(" hud  "),
        Span::styled("F5", hud_accent),
        Span::raw(" refresh  "),
        Span::styled("F6/F7", hud_accent),
        Span::raw(" tabs  "),
        Span::styled("F8", hud_accent),
        Span::raw(" terminate"),
    ]);
    let context = Line::from(vec![
        Span::styled("root:", hud_accent),
        Span::raw(" "),
        Span::raw("~/.orcas"),
        Span::raw(" | "),
        Span::raw("press F2 for HUD"),
    ]);
    let top_lines = vec![title, shortcuts, context];
    let top_block = Block::default()
        .borders(Borders::ALL)
        .border_style(hud_border)
        .title_style(hud_accent)
        .title("Orcas HUD");
    frame.render_widget(
        Paragraph::new(top_lines)
            .block(top_block)
            .wrap(Wrap { trim: true }),
        layout.top,
    );
    let active_session = state
        .active_session()
        .map(|session| {
            format!(
                "active_session: {} | status={} | cwd={} | age={}s",
                session.session.thread_id,
                session.session.status().label(),
                session.session.cwd.display(),
                session.session.started_at.elapsed().as_secs()
            )
        })
        .unwrap_or_else(|| "active_session: -".to_string());
    let bottom_lines = vec![
        Line::from(state.status.clone()),
        Line::from(active_session),
        Line::from(format!(
            "mode: {}",
            if state.show_hud { "hud" } else { "canvas" }
        )),
    ];
    let bottom_block = Block::default()
        .borders(Borders::ALL)
        .border_style(hud_border)
        .title_style(hud_accent)
        .title("HUD Details");
    frame.render_widget(
        Paragraph::new(bottom_lines)
            .block(bottom_block)
            .wrap(Wrap { trim: true }),
        layout.bottom,
    );
}

async fn poll_key_event(timeout: Duration) -> Result<Option<KeyEvent>> {
    tokio::task::spawn_blocking(move || -> Result<Option<KeyEvent>> {
        if event::poll(timeout).context("poll dashboard input")? {
            match event::read().context("read dashboard input")? {
                Event::Key(key) => Ok(Some(key)),
                _ => Ok(None),
            }
        } else {
            Ok(None)
        }
    })
    .await
    .context("join dashboard input task")?
}

fn key_to_bytes(key: KeyEvent) -> Vec<u8> {
    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let byte = match c {
                    '@' => 0x00,
                    ' ' => 0x00,
                    '[' => 0x1b,
                    '\\' => 0x1c,
                    ']' => 0x1d,
                    '^' => 0x1e,
                    '_' => 0x1f,
                    'a'..='z' => (c as u8 - b'a') + 1,
                    'A'..='Z' => (c as u8 - b'A') + 1,
                    _ => c as u8,
                };
                vec![byte]
            } else if key.modifiers.contains(KeyModifiers::ALT) {
                let mut bytes = vec![0x1b];
                bytes.extend(c.encode_utf8(&mut [0; 4]).as_bytes());
                bytes
            } else {
                let mut buf = [0; 4];
                c.encode_utf8(&mut buf).as_bytes().to_vec()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        _ => Vec::new(),
    }
}

fn cleanup_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) {
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen, Show);
    let _ = terminal.show_cursor();
}
