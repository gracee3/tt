use std::io::{self, IsTerminal, Stdout};
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use orcas_core::ipc;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use tokio::process::Command;

use crate::service::SupervisorService;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusPane {
    Workstreams,
    Threads,
}

#[derive(Debug)]
struct DashboardState {
    snapshot: ipc::StateSnapshot,
    focus: FocusPane,
    workstream_index: usize,
    thread_index: usize,
    status: String,
    last_refresh: Instant,
}

impl DashboardState {
    fn new(snapshot: ipc::StateSnapshot) -> Self {
        Self {
            snapshot,
            focus: FocusPane::Workstreams,
            workstream_index: 0,
            thread_index: 0,
            status: "dashboard ready".to_string(),
            last_refresh: Instant::now(),
        }
    }

    fn workstreams(&self) -> &[ipc::WorkstreamSummary] {
        &self.snapshot.collaboration.workstreams
    }

    fn filtered_threads(&self) -> Vec<&ipc::ThreadSummary> {
        let selected = self.selected_workstream_id();
        self.snapshot
            .threads
            .iter()
            .filter(|thread| match selected {
                Some(workstream_id) => {
                    thread.owner_workstream_id.as_deref() == Some(workstream_id)
                        || thread.runtime_workstream_id.as_deref() == Some(workstream_id)
                }
                None => true,
            })
            .collect()
    }

    fn selected_workstream_id(&self) -> Option<&str> {
        self.workstreams()
            .get(self.workstream_index)
            .map(|workstream| workstream.id.as_str())
    }

    fn selected_workstream(&self) -> Option<&ipc::WorkstreamSummary> {
        self.workstreams().get(self.workstream_index)
    }

    fn selected_thread(&self) -> Option<&ipc::ThreadSummary> {
        self.filtered_threads().get(self.thread_index).copied()
    }

    fn normalize_selection(&mut self) {
        if self.workstream_index >= self.workstreams().len() {
            self.workstream_index = 0;
        }
        let thread_count = self.filtered_threads().len();
        if self.thread_index >= thread_count {
            self.thread_index = 0;
        }
        if matches!(self.focus, FocusPane::Threads) && thread_count == 0 {
            self.focus = FocusPane::Workstreams;
        }
    }

    fn move_up(&mut self) {
        match self.focus {
            FocusPane::Workstreams => {
                let len = self.workstreams().len();
                if len > 0 {
                    self.workstream_index = self.workstream_index.saturating_sub(1);
                }
                self.thread_index = 0;
            }
            FocusPane::Threads => {
                let len = self.filtered_threads().len();
                if len > 0 {
                    self.thread_index = self.thread_index.saturating_sub(1);
                }
            }
        }
    }

    fn move_down(&mut self) {
        match self.focus {
            FocusPane::Workstreams => {
                let len = self.workstreams().len();
                if len > 0 && self.workstream_index + 1 < len {
                    self.workstream_index += 1;
                }
                self.thread_index = 0;
            }
            FocusPane::Threads => {
                let len = self.filtered_threads().len();
                if len > 0 && self.thread_index + 1 < len {
                    self.thread_index += 1;
                }
            }
        }
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            FocusPane::Workstreams => FocusPane::Threads,
            FocusPane::Threads => FocusPane::Workstreams,
        };
        self.normalize_selection();
    }

    fn thread_count(&self) -> usize {
        self.filtered_threads().len()
    }
}

pub async fn run_dashboard(service: SupervisorService) -> Result<()> {
    if !io::stdout().is_terminal() {
        bail!("orcas tui requires an interactive terminal");
    }

    enable_raw_mode().context("enable terminal raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, Hide).context("enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("create dashboard terminal")?;

    let result = run_dashboard_loop(&service, &mut terminal).await;

    cleanup_terminal(&mut terminal);
    result
}

async fn run_dashboard_loop(
    service: &SupervisorService,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
) -> Result<()> {
    let snapshot = service.dashboard_snapshot().await?;
    let mut state = DashboardState::new(snapshot);
    let refresh_interval = Duration::from_millis(750);

    loop {
        state.normalize_selection();
        terminal
            .draw(|frame| render_dashboard(frame, &state))
            .context("render Orcas dashboard")?;

        if state.last_refresh.elapsed() >= refresh_interval {
            state.snapshot = service.dashboard_snapshot().await?;
            state.last_refresh = Instant::now();
            continue;
        }

        if let Some(key) = poll_key_event(Duration::from_millis(125)).await? {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => {
                    state.status = "wrapper closed".to_string();
                    break;
                }
                KeyCode::Char('r') => {
                    state.snapshot = service.dashboard_snapshot().await?;
                    state.last_refresh = Instant::now();
                    state.status = "refreshed daemon snapshot".to_string();
                }
                KeyCode::Tab => {
                    state.toggle_focus();
                    state.status = match state.focus {
                        FocusPane::Workstreams => "focused workstreams".to_string(),
                        FocusPane::Threads => "focused threads".to_string(),
                    };
                }
                KeyCode::Up => {
                    state.move_up();
                }
                KeyCode::Down => {
                    state.move_down();
                }
                KeyCode::Enter => match state.focus {
                    FocusPane::Workstreams => {
                        if state.thread_count() > 0 {
                            state.focus = FocusPane::Threads;
                            state.thread_index = 0;
                            state.status = "focused threads".to_string();
                        } else {
                            state.status = "selected workstream has no threads".to_string();
                        }
                    }
                    FocusPane::Threads => {
                        if let Some(thread) = state.selected_thread().cloned() {
                            state.status = format!("launching codex resume for {}", thread.id);
                            cleanup_terminal(terminal);
                            let launch_result = launch_codex_resume(service, &thread).await;
                            enable_raw_mode().context("re-enter terminal raw mode")?;
                            execute!(terminal.backend_mut(), EnterAlternateScreen, Hide)
                                .context("re-enter alternate screen")?;
                            terminal.clear().ok();
                            state.snapshot = service.dashboard_snapshot().await?;
                            state.last_refresh = Instant::now();
                            state.normalize_selection();
                            match launch_result {
                                Ok(()) => {
                                    state.status =
                                        format!("returned from codex resume for {}", thread.id)
                                }
                                Err(error) => {
                                    state.status =
                                        format!("codex resume for {} failed: {error:#}", thread.id);
                                }
                            }
                        } else {
                            state.status = "no thread selected".to_string();
                        }
                    }
                },
                KeyCode::Char('g') => {
                    state.focus = FocusPane::Workstreams;
                    state.status = "focused workstreams".to_string();
                }
                KeyCode::Char('t') => {
                    state.focus = FocusPane::Threads;
                    state.status = "focused threads".to_string();
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn render_dashboard(frame: &mut ratatui::Frame<'_>, state: &DashboardState) {
    let root = frame.size();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(8),
            Constraint::Length(7),
        ])
        .split(root);

    render_header(frame, layout[0], state);
    render_body(frame, layout[1], state);
    render_footer(frame, layout[2], state);
}

fn render_header(frame: &mut ratatui::Frame<'_>, area: Rect, state: &DashboardState) {
    let daemon = &state.snapshot.daemon;
    let active = state
        .snapshot
        .session
        .active_thread_id
        .as_deref()
        .unwrap_or("-");
    let title = Line::from(vec![
        Span::styled(" Orcas TUI ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" supervisor dashboard "),
        Span::raw(" | "),
        Span::raw(format!("daemon={}", daemon.upstream.status)),
        Span::raw(" | "),
        Span::raw(format!("active_thread={active}")),
        Span::raw(" | "),
        Span::raw(format!("workstreams={}", state.workstreams().len())),
        Span::raw(" | "),
        Span::raw(format!("threads={}", state.snapshot.threads.len())),
    ]);
    let help = Line::from(vec![
        Span::raw("q"),
        Span::raw(" quit wrapper  "),
        Span::raw("tab"),
        Span::raw(" switch pane  "),
        Span::raw("enter"),
        Span::raw(" open Codex TUI  "),
        Span::raw("r"),
        Span::raw(" refresh"),
    ]);
    let block = Block::default().borders(Borders::ALL).title(title);
    let paragraph = Paragraph::new(help).block(block).wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

fn render_body(frame: &mut ratatui::Frame<'_>, area: Rect, state: &DashboardState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    let workstream_items: Vec<ListItem<'_>> = state
        .workstreams()
        .iter()
        .map(|workstream| {
            let mut lines = Vec::new();
            lines.push(Line::from(vec![Span::styled(
                workstream.title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            )]));
            lines.push(Line::from(format!("id: {}", workstream.id)));
            lines.push(Line::from(format!("status: {:?}", workstream.status)));
            lines.push(Line::from(format!("priority: {}", workstream.priority)));
            ListItem::new(lines)
        })
        .collect();
    let mut workstream_state = ListState::default();
    if !state.workstreams().is_empty() {
        workstream_state.select(Some(state.workstream_index));
    }
    let workstream_block = Block::default().borders(Borders::ALL).title(
        if matches!(state.focus, FocusPane::Workstreams) {
            "Workstreams (focused)"
        } else {
            "Workstreams"
        },
    );
    let workstream_list = List::new(workstream_items)
        .block(workstream_block)
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .bg(ratatui::style::Color::Blue),
        )
        .highlight_symbol(">> ");
    frame.render_stateful_widget(workstream_list, chunks[0], &mut workstream_state);

    let thread_items: Vec<ListItem<'_>> = state
        .filtered_threads()
        .into_iter()
        .map(|thread| {
            let lines = vec![
                Line::from(vec![Span::styled(
                    thread.id.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                )]),
                Line::from(format!("status: {}", thread.status)),
                Line::from(format!(
                    "owner/runtime: {}/{}",
                    thread.owner_workstream_id.as_deref().unwrap_or("-"),
                    thread.runtime_workstream_id.as_deref().unwrap_or("-")
                )),
                Line::from(thread.preview.clone().replace('\n', " ")),
            ];
            ListItem::new(lines)
        })
        .collect();
    let mut thread_state = ListState::default();
    if !thread_items.is_empty() {
        thread_state.select(Some(state.thread_index));
    }
    let thread_block = Block::default().borders(Borders::ALL).title(
        if matches!(state.focus, FocusPane::Threads) {
            "Threads (focused)"
        } else {
            "Threads"
        },
    );
    let thread_list = List::new(thread_items)
        .block(thread_block)
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .bg(ratatui::style::Color::Blue),
        )
        .highlight_symbol(">> ");
    frame.render_stateful_widget(thread_list, chunks[1], &mut thread_state);
}

fn render_footer(frame: &mut ratatui::Frame<'_>, area: Rect, state: &DashboardState) {
    let selected_workstream = state
        .selected_workstream()
        .map(|workstream| {
            format!(
                "selected_workstream: {} | id={} | status={:?} | objective={}",
                workstream.title, workstream.id, workstream.status, workstream.objective
            )
        })
        .unwrap_or_else(|| "selected_workstream: -".to_string());
    let selected_thread = state
        .selected_thread()
        .map(|thread| {
            format!(
                "selected_thread: {} | cwd={} | model_provider={} | recent_event={}",
                thread.id,
                thread.cwd,
                thread.model_provider,
                thread.recent_event.as_deref().unwrap_or("-")
            )
        })
        .unwrap_or_else(|| "selected_thread: -".to_string());
    let lines = vec![
        Line::from(state.status.clone()),
        Line::from(selected_workstream),
        Line::from(selected_thread),
    ];
    let block = Block::default().borders(Borders::ALL).title("Details");
    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
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

async fn launch_codex_resume(
    service: &SupervisorService,
    thread: &ipc::ThreadSummary,
) -> Result<()> {
    let mut command = Command::new(&service.config.codex.binary_path);
    command.arg("resume").arg(&thread.id);
    if !thread.cwd.is_empty() && Path::new(&thread.cwd).is_dir() {
        command.current_dir(&thread.cwd);
    }
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());

    let status = command
        .status()
        .await
        .with_context(|| format!("launch Codex TUI for thread {}", thread.id))?;
    if !status.success() {
        bail!("codex resume for thread {} exited with {status}", thread.id);
    }
    Ok(())
}

fn cleanup_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) {
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen, Show);
    let _ = terminal.show_cursor();
}
