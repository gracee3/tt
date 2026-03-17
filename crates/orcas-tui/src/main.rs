#![allow(unused_crate_dependencies)]

use std::io::{self, IsTerminal};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tracing::{debug, info};

use orcas_core::{AppPaths, init_file_logger};
use orcas_tui::app::{Action, TopLevelView, UserAction};
use orcas_tui::backend::OrcasDaemonBackend;
use orcas_tui::render;
use orcas_tui::runtime::AppRuntime;

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::args().any(|arg| matches!(arg.as_str(), "--help" | "-h")) {
        println!("orcas-tui");
        println!("Usage: orcas-tui");
        println!("A terminal UI for Orcas daemon state inspection.");
        return Ok(());
    }

    let paths = AppPaths::discover()?;
    paths.ensure().await?;
    init_file_logger("orcas-tui", &paths.logs_dir.join("orcas-tui.log"))?;
    info!(version = env!("CARGO_PKG_VERSION"), "starting orcas-tui");

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
                if handle_key(runtime, key).await {
                    break;
                }
            }
        }
    }

    Ok(())
}

async fn handle_key(runtime: &mut AppRuntime<OrcasDaemonBackend>, key: KeyEvent) -> bool {
    debug!(
        key = ?key,
        current_view = ?runtime.state().current_view,
        "received key in tui"
    );
    if key.code == KeyCode::Char('q') && runtime.state().steer_compose.is_none() {
        return true;
    }

    let action = action_for_key(runtime.state(), key);

    if let Some(action) = action {
        info!(?action, "dispatching tui action");
        runtime.dispatch(Action::User(action));
    }
    false
}

fn action_for_key(state: &orcas_tui::app::AppState, key: KeyEvent) -> Option<UserAction> {
    if state.steer_compose.is_some() {
        return match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => Some(UserAction::CancelSteerCompose),
            (KeyCode::Char('s'), KeyModifiers::CONTROL) => Some(UserAction::SubmitSteerCompose),
            (KeyCode::Enter, _) => Some(UserAction::SteerComposeInsertNewline),
            (KeyCode::Backspace, _) => Some(UserAction::SteerComposeBackspace),
            (KeyCode::Delete, _) => Some(UserAction::SteerComposeDelete),
            (KeyCode::Left, _) => Some(UserAction::SteerComposeMoveLeft),
            (KeyCode::Right, _) => Some(UserAction::SteerComposeMoveRight),
            (KeyCode::Up, _) => Some(UserAction::SteerComposeMoveUp),
            (KeyCode::Down, _) => Some(UserAction::SteerComposeMoveDown),
            (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                Some(UserAction::SteerComposeAppend(ch))
            }
            _ => None,
        };
    }

    let current_view = state.current_view;
    let in_supervisor_view = current_view == TopLevelView::Supervisor;
    let in_threads_view = current_view == TopLevelView::Threads;
    match key.code {
        KeyCode::Char('r') => Some(UserAction::Refresh),
        KeyCode::Char('?') => Some(UserAction::ToggleHelp),
        KeyCode::Char('1') => Some(UserAction::ShowView(TopLevelView::Overview)),
        KeyCode::Char('2') => Some(UserAction::ShowView(TopLevelView::Threads)),
        KeyCode::Char('3') => Some(UserAction::ShowView(TopLevelView::Collaboration)),
        KeyCode::Char('4') => Some(UserAction::ShowView(TopLevelView::Supervisor)),
        KeyCode::Char('m') if in_supervisor_view => Some(UserAction::LoadModels),
        KeyCode::Char('s') if in_supervisor_view => Some(UserAction::StartDaemon),
        KeyCode::Char('x') if in_supervisor_view => Some(UserAction::StopDaemon),
        KeyCode::Char('R') if in_supervisor_view => Some(UserAction::RestartDaemon),
        KeyCode::Char('a') if in_threads_view => {
            Some(UserAction::ApproveSelectedSupervisorDecision)
        }
        KeyCode::Char('d') if in_threads_view => Some(UserAction::RejectSelectedSupervisorDecision),
        KeyCode::Char('s') if in_threads_view => Some(UserAction::ProposeSteerForSelectedThread),
        KeyCode::Char('e') if in_threads_view => {
            Some(UserAction::EditPendingSteerForSelectedThread)
        }
        KeyCode::Char('i') if in_threads_view => {
            Some(UserAction::ProposeInterruptForSelectedThread)
        }
        KeyCode::Down => Some(UserAction::SelectNextInView),
        KeyCode::Up => Some(UserAction::SelectPreviousInView),
        KeyCode::Left => Some(UserAction::ShowView(current_view.previous())),
        KeyCode::Right => Some(UserAction::ShowView(current_view.next())),
        KeyCode::Tab if current_view == TopLevelView::Collaboration => {
            Some(UserAction::CycleCollaborationFocus)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn left_and_right_cycle_top_level_views() {
        assert_eq!(
            action_for_key(&state_for_view(TopLevelView::Overview), key(KeyCode::Right)),
            Some(UserAction::ShowView(TopLevelView::Threads))
        );
        assert_eq!(
            action_for_key(&state_for_view(TopLevelView::Threads), key(KeyCode::Right)),
            Some(UserAction::ShowView(TopLevelView::Collaboration))
        );
        assert_eq!(
            action_for_key(
                &state_for_view(TopLevelView::Collaboration),
                key(KeyCode::Left)
            ),
            Some(UserAction::ShowView(TopLevelView::Threads))
        );
        assert_eq!(
            action_for_key(&state_for_view(TopLevelView::Overview), key(KeyCode::Left)),
            Some(UserAction::ShowView(TopLevelView::Supervisor))
        );
    }

    #[test]
    fn arrow_keys_drive_selection_and_tab_switches_collaboration_focus() {
        assert_eq!(
            action_for_key(&state_for_view(TopLevelView::Threads), key(KeyCode::Down)),
            Some(UserAction::SelectNextInView)
        );
        assert_eq!(
            action_for_key(&state_for_view(TopLevelView::Threads), key(KeyCode::Up)),
            Some(UserAction::SelectPreviousInView)
        );
        assert_eq!(
            action_for_key(
                &state_for_view(TopLevelView::Collaboration),
                key(KeyCode::Tab)
            ),
            Some(UserAction::CycleCollaborationFocus)
        );
        assert_eq!(
            action_for_key(&state_for_view(TopLevelView::Supervisor), key(KeyCode::Tab)),
            None
        );
    }

    #[test]
    fn legacy_j_k_h_l_keys_are_not_mapped_anymore() {
        assert_eq!(
            action_for_key(
                &state_for_view(TopLevelView::Threads),
                key(KeyCode::Char('j'))
            ),
            None
        );
        assert_eq!(
            action_for_key(
                &state_for_view(TopLevelView::Collaboration),
                key(KeyCode::Char('h'))
            ),
            None
        );
        assert_eq!(
            action_for_key(
                &state_for_view(TopLevelView::Collaboration),
                key(KeyCode::Char('l'))
            ),
            None
        );
    }

    #[test]
    fn threads_view_maps_supervisor_review_actions() {
        assert_eq!(
            action_for_key(
                &state_for_view(TopLevelView::Threads),
                key(KeyCode::Char('a'))
            ),
            Some(UserAction::ApproveSelectedSupervisorDecision)
        );
        assert_eq!(
            action_for_key(
                &state_for_view(TopLevelView::Threads),
                key(KeyCode::Char('d'))
            ),
            Some(UserAction::RejectSelectedSupervisorDecision)
        );
        assert_eq!(
            action_for_key(
                &state_for_view(TopLevelView::Threads),
                key(KeyCode::Char('s'))
            ),
            Some(UserAction::ProposeSteerForSelectedThread)
        );
        assert_eq!(
            action_for_key(
                &state_for_view(TopLevelView::Threads),
                key(KeyCode::Char('e'))
            ),
            Some(UserAction::EditPendingSteerForSelectedThread)
        );
        assert_eq!(
            action_for_key(
                &state_for_view(TopLevelView::Threads),
                key(KeyCode::Char('i'))
            ),
            Some(UserAction::ProposeInterruptForSelectedThread)
        );
    }

    #[test]
    fn steer_compose_mode_captures_text_keys() {
        let mut state = state_for_view(TopLevelView::Threads);
        state.steer_compose = Some(orcas_tui::app::SteerComposeState {
            assignment_id: "cta-1".to_string(),
            thread_id: "thread-1".to_string(),
            replace_decision_id: None,
            buffer: String::new(),
            cursor: 0,
            preferred_column: 0,
        });
        assert_eq!(
            action_for_key(&state, key(KeyCode::Char('x'))),
            Some(UserAction::SteerComposeAppend('x'))
        );
        assert_eq!(
            action_for_key(&state, key(KeyCode::Backspace)),
            Some(UserAction::SteerComposeBackspace)
        );
        assert_eq!(
            action_for_key(&state, key(KeyCode::Enter)),
            Some(UserAction::SteerComposeInsertNewline)
        );
        assert_eq!(
            action_for_key(&state, key(KeyCode::Esc)),
            Some(UserAction::CancelSteerCompose)
        );
        assert_eq!(
            action_for_key(&state, key(KeyCode::Delete)),
            Some(UserAction::SteerComposeDelete)
        );
        assert_eq!(
            action_for_key(&state, key(KeyCode::Left)),
            Some(UserAction::SteerComposeMoveLeft)
        );
        assert_eq!(
            action_for_key(&state, ctrl_key('s')),
            Some(UserAction::SubmitSteerCompose)
        );
    }

    fn state_for_view(view: TopLevelView) -> orcas_tui::app::AppState {
        orcas_tui::app::AppState {
            current_view: view,
            ..Default::default()
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl_key(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
    }
}
