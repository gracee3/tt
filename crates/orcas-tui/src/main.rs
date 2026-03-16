#![allow(unused_crate_dependencies)]

use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

use orcas_core::{AppPaths, DaemonStatusResponse, EventEnvelope, OrcasEvent, ThreadSummary};
use orcas_daemon::{
    OrcasDaemonLaunch, OrcasDaemonProcessManager, OrcasIpcClient, OrcasRuntimeOverrides,
};

const MAX_LOG_LINES: usize = 32;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut app = TuiApp::bootstrap().await?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, &mut app).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut TuiApp,
) -> Result<()> {
    let mut next_refresh = Instant::now();
    loop {
        app.drain_events();
        if next_refresh <= Instant::now() {
            app.refresh().await;
            next_refresh = Instant::now() + Duration::from_secs(2);
        }

        terminal.draw(|frame| {
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(7),
                    Constraint::Length(10),
                    Constraint::Min(10),
                ])
                .split(frame.area());

            let header = Paragraph::new(app.render_status())
                .block(Block::default().title("Daemon").borders(Borders::ALL));
            let threads = Paragraph::new(app.render_threads())
                .block(Block::default().title("Threads").borders(Borders::ALL));
            let log = Paragraph::new(app.render_log())
                .block(Block::default().title("Event Log").borders(Borders::ALL));

            frame.render_widget(header, layout[0]);
            frame.render_widget(threads, layout[1]);
            frame.render_widget(log, layout[2]);
        })?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('r') => {
                        app.refresh().await;
                        next_refresh = Instant::now() + Duration::from_secs(2);
                    }
                    KeyCode::Char('?') => app.show_help = !app.show_help,
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

struct TuiApp {
    paths: AppPaths,
    daemon: OrcasDaemonProcessManager,
    client: Option<Arc<OrcasIpcClient>>,
    events: Option<tokio::sync::broadcast::Receiver<EventEnvelope>>,
    status: Option<DaemonStatusResponse>,
    threads: Vec<ThreadSummary>,
    event_log: VecDeque<String>,
    last_error: Option<String>,
    show_help: bool,
}

impl TuiApp {
    async fn bootstrap() -> Result<Self> {
        let paths = AppPaths::discover()?;
        paths.ensure().await?;
        let daemon =
            OrcasDaemonProcessManager::new(paths.clone(), OrcasRuntimeOverrides::default());
        let mut app = Self {
            paths,
            daemon,
            client: None,
            events: None,
            status: None,
            threads: Vec::new(),
            event_log: VecDeque::new(),
            last_error: None,
            show_help: false,
        };
        app.refresh().await;
        Ok(app)
    }

    async fn refresh(&mut self) {
        if self.client.is_none()
            && let Err(error) = self.connect().await
        {
            self.last_error = Some(error.to_string());
            self.push_log(format!("event> connect failed: {error}"));
            return;
        }

        let Some(client) = self.client.as_ref() else {
            return;
        };

        match client.daemon_status().await {
            Ok(status) => {
                self.status = Some(status);
                self.last_error = None;
            }
            Err(error) => {
                self.last_error = Some(error.to_string());
                self.client = None;
                self.events = None;
                return;
            }
        }

        match client.threads_list().await {
            Ok(response) => self.threads = response.data,
            Err(error) => {
                self.last_error = Some(error.to_string());
            }
        }
    }

    async fn connect(&mut self) -> Result<()> {
        self.daemon
            .ensure_running(OrcasDaemonLaunch::IfNeeded)
            .await?;
        let client = OrcasIpcClient::connect(&self.paths).await?;
        client.daemon_connect().await?;
        let (events, snapshot) = client.subscribe_events(true).await?;
        if let Some(snapshot) = snapshot {
            self.status = Some(snapshot.status);
            self.threads = snapshot.threads;
            for event in snapshot.recent_events {
                self.push_log(format_event(&event));
            }
        }
        self.client = Some(client);
        self.events = Some(events);
        self.last_error = None;
        Ok(())
    }

    fn drain_events(&mut self) {
        loop {
            let next = match self.events.as_mut() {
                Some(events) => events.try_recv(),
                None => break,
            };
            match next {
                Ok(event) => {
                    if let OrcasEvent::ConnectionStateChanged(state) = &event.event
                        && let Some(status) = self.status.as_mut()
                    {
                        status.upstream = state.clone();
                    }
                    self.push_log(format_event(&event));
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(skipped)) => {
                    self.push_log(format!("event> lagged; skipped {skipped} events"));
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                    self.push_log("event> daemon event stream closed".to_string());
                    self.events = None;
                    break;
                }
            }
        }
    }

    fn render_status(&self) -> Text<'static> {
        let mut lines = vec![
            Line::styled("Orcas TUI", Style::default().add_modifier(Modifier::BOLD)),
            Line::from(format!("socket: {}", self.paths.socket_file.display())),
        ];

        if let Some(status) = &self.status {
            lines.push(Line::from(format!(
                "codex: {} [{}]",
                status.codex_endpoint, status.upstream.status
            )));
            lines.push(Line::from(format!(
                "clients: {}  threads: {}",
                status.client_count, status.known_threads
            )));
        } else {
            lines.push(Line::from("codex: unavailable"));
            lines.push(Line::from("clients: 0  threads: 0"));
        }

        if let Some(error) = &self.last_error {
            lines.push(Line::from(format!("error: {error}")));
        } else if self.show_help {
            lines.push(Line::from("keys: q quit, r refresh, ? help"));
        } else {
            lines.push(Line::from("keys: q quit, r refresh, ? help"));
        }

        Text::from(lines)
    }

    fn render_threads(&self) -> Text<'static> {
        if self.threads.is_empty() {
            return Text::from(vec![Line::from("No threads loaded yet.")]);
        }

        Text::from(
            self.threads
                .iter()
                .take(8)
                .map(|thread| {
                    Line::from(format!(
                        "{}  {}  {}",
                        thread.id,
                        thread.status,
                        thread.preview.replace('\n', " ")
                    ))
                })
                .collect::<Vec<_>>(),
        )
    }

    fn render_log(&self) -> Text<'static> {
        if self.event_log.is_empty() {
            return Text::from(vec![Line::from(
                "No events yet. Start a thread or run a prompt from the supervisor.",
            )]);
        }

        Text::from(
            self.event_log
                .iter()
                .cloned()
                .map(Line::from)
                .collect::<Vec<_>>(),
        )
    }

    fn push_log(&mut self, line: String) {
        if self.event_log.len() >= MAX_LOG_LINES {
            self.event_log.pop_front();
        }
        self.event_log.push_back(line);
    }
}

fn format_event(event: &EventEnvelope) -> String {
    match &event.event {
        OrcasEvent::ConnectionStateChanged(state) => {
            format!("event> upstream {} {}", state.endpoint, state.status)
        }
        OrcasEvent::ThreadStarted { thread_id, preview } => {
            format!(
                "event> thread started {thread_id} {}",
                preview.replace('\n', " ")
            )
        }
        OrcasEvent::ThreadStatusChanged { thread_id, status } => {
            format!("event> thread {thread_id} status {status}")
        }
        OrcasEvent::TurnStarted { thread_id, turn_id } => {
            format!("event> turn started {thread_id}/{turn_id}")
        }
        OrcasEvent::TurnCompleted {
            thread_id,
            turn_id,
            status,
        } => format!("event> turn completed {thread_id}/{turn_id} {status}"),
        OrcasEvent::ItemStarted {
            thread_id,
            turn_id,
            item_type,
            ..
        } => format!("event> item started {thread_id}/{turn_id} {item_type}"),
        OrcasEvent::ItemCompleted {
            thread_id,
            turn_id,
            item_type,
            ..
        } => format!("event> item completed {thread_id}/{turn_id} {item_type}"),
        OrcasEvent::AgentMessageDelta { delta, .. } => {
            format!("event> delta {}", delta.replace('\n', "\\n"))
        }
        OrcasEvent::ServerRequest { method } => format!("event> server request {method}"),
        OrcasEvent::Warning { message } => format!("event> warning {message}"),
    }
}
