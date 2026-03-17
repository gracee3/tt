#![allow(unused_crate_dependencies)]

use std::io::{self, IsTerminal};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use orcas_tui::app::{Action, TopLevelView, UserAction};
use orcas_tui::backend::OrcasDaemonBackend;
use orcas_tui::render;
use orcas_tui::runtime::AppRuntime;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();

    if !(io::stdout().is_terminal() && io::stdin().is_terminal()) {
        anyhow::bail!("orcas-tui requires an interactive terminal (TTY)");
    }

    let backend = Arc::new(OrcasDaemonBackend::discover().await?);
    let mut runtime = AppRuntime::new(backend);
    runtime.bootstrap().await;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, &mut runtime).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    runtime: &mut AppRuntime<OrcasDaemonBackend>,
) -> Result<()> {
    loop {
        runtime.process_all().await;
        terminal.draw(|frame| render::render(frame, runtime.state()))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if handle_key(runtime, key.code).await {
                    break;
                }
            }
        }
    }

    Ok(())
}

async fn handle_key(runtime: &mut AppRuntime<OrcasDaemonBackend>, code: KeyCode) -> bool {
    let in_supervisor_view = runtime.state().current_view == TopLevelView::Supervisor;
    let action = match code {
        KeyCode::Char('q') => return true,
        KeyCode::Char('r') => Some(UserAction::Refresh),
        KeyCode::Char('?') => Some(UserAction::ToggleHelp),
        KeyCode::Tab => Some(UserAction::CycleView),
        KeyCode::Char('1') => Some(UserAction::ShowView(TopLevelView::Overview)),
        KeyCode::Char('2') => Some(UserAction::ShowView(TopLevelView::Threads)),
        KeyCode::Char('3') => Some(UserAction::ShowView(TopLevelView::Collaboration)),
        KeyCode::Char('4') => Some(UserAction::ShowView(TopLevelView::Supervisor)),
        KeyCode::Char('m') if in_supervisor_view => Some(UserAction::LoadModels),
        KeyCode::Char('x') if in_supervisor_view => Some(UserAction::StopDaemon),
        KeyCode::Char('j') | KeyCode::Down => Some(UserAction::SelectNextInView),
        KeyCode::Char('k') | KeyCode::Up => Some(UserAction::SelectPreviousInView),
        KeyCode::Char('h') | KeyCode::Left => Some(UserAction::CycleCollaborationFocus),
        KeyCode::Char('l') | KeyCode::Right => Some(UserAction::CycleCollaborationFocus),
        _ => None,
    };

    if let Some(action) = action {
        runtime.dispatch(Action::User(action));
    }
    false
}
