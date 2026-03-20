#![allow(unused_crate_dependencies)]

use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use clap::{Args, Parser};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use tracing::{info, trace, warn};

use orcas_core::{AppPaths, init_file_logger};
use orcas_tui::app::{Action, MainFooterState, ProgramView, TopLevelView, UserAction};
use orcas_tui::backend::OrcasDaemonBackend;
use orcas_tui::codex::{
    CodexResumeDescriptor, CodexSessionManager, DEFAULT_PTY_RING_BUFFER_CAPACITY, OrcasTerminal,
};
use orcas_tui::render;
use orcas_tui::runtime::AppRuntime;

#[derive(Debug, Parser)]
#[command(name = "orcas-tui", version, about = "Orcas terminal UI")]
struct TuiCli {
    #[command(flatten)]
    runtime: TuiRuntimeArgs,
}

#[derive(Debug, Clone, Args, Default, PartialEq, Eq)]
struct TuiRuntimeArgs {
    #[arg(long)]
    codex_bin: Option<PathBuf>,
    #[arg(long)]
    listen_url: Option<String>,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long, default_value_t = false)]
    connect_only: bool,
    #[arg(long, default_value_t = false)]
    force_spawn: bool,
}

impl TuiRuntimeArgs {
    fn into_runtime_overrides(self) -> orcasd::OrcasRuntimeOverrides {
        orcasd::OrcasRuntimeOverrides {
            codex_bin: self.codex_bin,
            listen_url: self.listen_url,
            cwd: self.cwd,
            model: self.model,
            connect_only: self.connect_only,
            force_spawn: self.force_spawn,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = TuiCli::parse();

    let paths = AppPaths::discover()?;
    paths.ensure().await?;
    init_file_logger("orcas-tui", &paths.logs_dir.join("orcas-tui.log"))?;
    info!(version = env!("CARGO_PKG_VERSION"), "starting orcas-tui");

    if !(io::stdout().is_terminal() && io::stdin().is_terminal()) {
        anyhow::bail!("orcas-tui requires an interactive terminal (TTY)");
    }

    let runtime_overrides =
        orcasd::OrcasRuntimeOverrides::from_env().overlay(&cli.runtime.into_runtime_overrides());
    let backend = Arc::new(OrcasDaemonBackend::discover_with_overrides(runtime_overrides).await?);
    let mut runtime = AppRuntime::new(backend);
    runtime.bootstrap().await;

    // This PTY manager is a TUI-local interactive attachment surface. It is not part of the
    // daemon's persisted collaboration or authority state model.
    let mut codex_sessions = CodexSessionManager::new(DEFAULT_PTY_RING_BUFFER_CAPACITY);
    let shutdown_requested = codex_sessions.shutdown_handle();
    let _shutdown_watcher = spawn_shutdown_watcher(Arc::clone(&shutdown_requested));
    let _terminal_guard = TuiSessionGuard::enter()?;
    let stdout = io::stdout();
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = OrcasTerminal::new(backend)?;

    let result = run_app(
        &mut terminal,
        &mut runtime,
        &mut codex_sessions,
        shutdown_requested,
    )
    .await;
    codex_sessions.shutdown();
    result
}

async fn run_app(
    terminal: &mut OrcasTerminal,
    runtime: &mut AppRuntime<OrcasDaemonBackend>,
    codex_sessions: &mut CodexSessionManager,
    shutdown_requested: Arc<AtomicBool>,
) -> Result<()> {
    loop {
        if shutdown_requested.load(Ordering::Relaxed) {
            break;
        }
        sync_codex_sessions(runtime, codex_sessions)?;
        runtime.process_all().await;
        sync_codex_sessions(runtime, codex_sessions)?;
        terminal.draw(|frame| render::render(frame, runtime.state()))?;

        if event::poll(Duration::from_millis(100))? {
            if shutdown_requested.load(Ordering::Relaxed) {
                break;
            }
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if handle_key(terminal, runtime, codex_sessions, key).await? {
                    break;
                }
            }
        }
    }

    Ok(())
}

fn spawn_shutdown_watcher(shutdown_requested: Arc<AtomicBool>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};

            let mut sigterm = match signal(SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(error) => {
                    warn!(%error, "failed to listen for SIGTERM");
                    if let Err(error) = tokio::signal::ctrl_c().await {
                        warn!(%error, "failed to listen for ctrl-c");
                    }
                    shutdown_requested.store(true, Ordering::Relaxed);
                    return;
                }
            };

            tokio::select! {
                signal = tokio::signal::ctrl_c() => {
                    if let Err(error) = signal {
                        warn!(%error, "failed to listen for ctrl-c");
                    }
                }
                _ = sigterm.recv() => {}
            }
        }

        #[cfg(not(unix))]
        {
            if let Err(error) = tokio::signal::ctrl_c().await {
                warn!(%error, "failed to listen for shutdown signal");
            }
        }

        shutdown_requested.store(true, Ordering::Relaxed);
    })
}

struct TuiSessionGuard;

impl TuiSessionGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        Ok(Self)
    }
}

impl Drop for TuiSessionGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen, Show);
    }
}

async fn handle_key(
    terminal: &mut OrcasTerminal,
    runtime: &mut AppRuntime<OrcasDaemonBackend>,
    codex_sessions: &mut CodexSessionManager,
    key: KeyEvent,
) -> Result<bool> {
    sync_codex_sessions(runtime, codex_sessions)?;
    trace!(
        key = ?key,
        current_view = ?runtime.state().current_view,
        "received key in tui"
    );
    if key.code == KeyCode::Char('q') && runtime.state().steer_compose.is_none() {
        return Ok(true);
    }

    let action = action_for_key(runtime.state(), key);

    if let Some(action) = action {
        if action == UserAction::ResumeSelectedThreadInCodex {
            match CodexResumeDescriptor::for_selected_thread(runtime.state()) {
                Ok(descriptor) => {
                    match tokio::task::block_in_place(|| {
                        codex_sessions.attach_or_resume(terminal, descriptor)
                    }) {
                        Ok(session_id) => {
                            sync_codex_sessions(runtime, codex_sessions)?;
                            if let Some(message) = codex_sessions
                                .session(session_id)
                                .and_then(orcas_tui::codex::CodexSession::terminal_message)
                            {
                                runtime.dispatch(Action::Event(orcas_tui::app::UiEvent::Warning(
                                    message,
                                )));
                            }
                        }
                        Err(error) => {
                            runtime.dispatch(Action::Event(orcas_tui::app::UiEvent::Error(
                                format!("Codex resume failed: {error}"),
                            )));
                        }
                    }
                }
                Err(error) => {
                    runtime.dispatch(Action::Event(orcas_tui::app::UiEvent::Warning(format!(
                        "Cannot resume selected thread in Codex: {error}"
                    ))));
                }
            }
        } else {
            trace!(
                action = user_action_label(&action),
                "dispatching tui action"
            );
            runtime.dispatch(Action::User(action));
        }
    }
    Ok(false)
}

fn user_action_label(action: &UserAction) -> &'static str {
    match action {
        UserAction::Refresh => "refresh",
        UserAction::LoadModels => "load_models",
        UserAction::StartDaemon => "start_daemon",
        UserAction::RestartDaemon => "restart_daemon",
        UserAction::StopDaemon => "stop_daemon",
        UserAction::ToggleHelp => "toggle_help",
        UserAction::CycleView => "cycle_view",
        UserAction::ShowView(_) => "show_view",
        UserAction::CycleProgramView => "cycle_program_view",
        UserAction::ShowProgramView(_) => "show_program_view",
        UserAction::CycleCollaborationFocus => "cycle_collaboration_focus",
        UserAction::SelectNextInView => "select_next_in_view",
        UserAction::SelectPreviousInView => "select_previous_in_view",
        UserAction::ExpandSelectedInView => "expand_selected_in_view",
        UserAction::CollapseSelectedInView => "collapse_selected_in_view",
        UserAction::SelectNextThread => "select_next_thread",
        UserAction::SelectPreviousThread => "select_previous_thread",
        UserAction::SelectThread(_) => "select_thread",
        UserAction::CreateWorkstream => "create_workstream",
        UserAction::CreateWorkUnitForSelection => "create_work_unit_for_selection",
        UserAction::CreateTrackedThreadForSelection => "create_tracked_thread_for_selection",
        UserAction::EditSelectedMainEntity => "edit_selected_main_entity",
        UserAction::DeleteSelectedMainEntity => "delete_selected_main_entity",
        UserAction::MainFooterAppend(_) => "main_footer_append",
        UserAction::MainFooterBackspace => "main_footer_backspace",
        UserAction::MainFooterDelete => "main_footer_delete",
        UserAction::MainFooterMoveLeft => "main_footer_move_left",
        UserAction::MainFooterMoveRight => "main_footer_move_right",
        UserAction::MainFooterNextField => "main_footer_next_field",
        UserAction::MainFooterPreviousField => "main_footer_previous_field",
        UserAction::SubmitMainFooter => "submit_main_footer",
        UserAction::CancelMainFooter => "cancel_main_footer",
        UserAction::EnterPromptMode => "enter_prompt_mode",
        UserAction::ExitPromptMode => "exit_prompt_mode",
        UserAction::PromptAppend(_) => "prompt_append",
        UserAction::PromptBackspace => "prompt_backspace",
        UserAction::SubmitPrompt => "submit_prompt",
        UserAction::ResumeSelectedThreadInCodex => "resume_selected_thread_in_codex",
        UserAction::ProposeSteerForSelectedThread => "propose_steer_for_selected_thread",
        UserAction::EditPendingSteerForSelectedThread => "edit_pending_steer_for_selected_thread",
        UserAction::SteerComposeAppend(_) => "steer_compose_append",
        UserAction::SteerComposeInsertNewline => "steer_compose_insert_newline",
        UserAction::SteerComposeBackspace => "steer_compose_backspace",
        UserAction::SteerComposeDelete => "steer_compose_delete",
        UserAction::SteerComposeMoveLeft => "steer_compose_move_left",
        UserAction::SteerComposeMoveRight => "steer_compose_move_right",
        UserAction::SteerComposeMoveUp => "steer_compose_move_up",
        UserAction::SteerComposeMoveDown => "steer_compose_move_down",
        UserAction::SubmitSteerCompose => "submit_steer_compose",
        UserAction::CancelSteerCompose => "cancel_steer_compose",
        UserAction::ProposeInterruptForSelectedThread => "propose_interrupt_for_selected_thread",
        UserAction::RecordNoActionForSelectedThread => "record_no_action_for_selected_thread",
        UserAction::ManualRefreshForSelectedThread => "manual_refresh_for_selected_thread",
        UserAction::ApproveSelectedSupervisorDecision => "approve_selected_supervisor_decision",
        UserAction::RejectSelectedSupervisorDecision => "reject_selected_supervisor_decision",
        UserAction::OpenSelectedProposalArtifactDetail => "open_selected_proposal_artifact_detail",
        UserAction::CloseReviewArtifactDetail => "close_review_artifact_detail",
        UserAction::ScrollReviewArtifactDetail(_) => "scroll_review_artifact_detail",
    }
}

fn sync_codex_sessions(
    runtime: &mut AppRuntime<OrcasDaemonBackend>,
    codex_sessions: &mut CodexSessionManager,
) -> Result<()> {
    codex_sessions.drain_background_events()?;
    let sessions = codex_sessions.thread_sessions();
    if runtime.state().codex_sessions != sessions {
        runtime.dispatch(Action::Event(
            orcas_tui::app::UiEvent::CodexSessionsChanged { sessions },
        ));
    }
    Ok(())
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

    let main_footer_active = state.current_view == TopLevelView::Overview
        && state.main_view.program_view == ProgramView::Main
        && !matches!(state.authority_main.footer, MainFooterState::Inspect);
    if main_footer_active {
        let submit_or_advance = match &state.authority_main.footer {
            MainFooterState::CreateWorkstream(form) | MainFooterState::EditWorkstream(form) => {
                if form.active_field + 1 >= 2 {
                    UserAction::SubmitMainFooter
                } else {
                    UserAction::MainFooterNextField
                }
            }
            MainFooterState::CreateWorkUnit(form) | MainFooterState::EditWorkUnit(form) => {
                if form.active_field + 1 >= 1 {
                    UserAction::SubmitMainFooter
                } else {
                    UserAction::MainFooterNextField
                }
            }
            MainFooterState::CreateTrackedThread(form)
            | MainFooterState::EditTrackedThread(form) => {
                if form.active_field + 1 >= 2 {
                    UserAction::SubmitMainFooter
                } else {
                    UserAction::MainFooterNextField
                }
            }
            MainFooterState::ConfirmDelete(_) => UserAction::SubmitMainFooter,
            MainFooterState::Inspect => UserAction::SubmitMainFooter,
        };
        return match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => Some(UserAction::CancelMainFooter),
            (KeyCode::Char('s'), KeyModifiers::CONTROL) => Some(UserAction::SubmitMainFooter),
            (KeyCode::Tab, KeyModifiers::NONE) => Some(UserAction::MainFooterNextField),
            (KeyCode::Enter, KeyModifiers::NONE) => Some(submit_or_advance),
            (KeyCode::BackTab, _) => Some(UserAction::MainFooterPreviousField),
            (KeyCode::Backspace, _) => Some(UserAction::MainFooterBackspace),
            (KeyCode::Delete, _) => Some(UserAction::MainFooterDelete),
            (KeyCode::Left, _) => Some(UserAction::MainFooterMoveLeft),
            (KeyCode::Right, _) => Some(UserAction::MainFooterMoveRight),
            (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                Some(UserAction::MainFooterAppend(ch))
            }
            _ => None,
        };
    }

    let current_view = state.current_view;
    let in_main_view =
        current_view == TopLevelView::Overview && state.main_view.program_view == ProgramView::Main;
    let in_review_view = current_view == TopLevelView::Overview
        && state.main_view.program_view == ProgramView::Review;
    let in_supervisor_view = current_view == TopLevelView::Supervisor;
    let in_threads_view = current_view == TopLevelView::Threads;
    let in_overview_program = current_view == TopLevelView::Overview;
    if in_review_view && state.review_view.artifact_detail.is_some() {
        return match key.code {
            KeyCode::Esc => Some(UserAction::CloseReviewArtifactDetail),
            KeyCode::Char('v') | KeyCode::Enter => Some(UserAction::CloseReviewArtifactDetail),
            KeyCode::Up => Some(UserAction::ScrollReviewArtifactDetail(-1)),
            KeyCode::Down => Some(UserAction::ScrollReviewArtifactDetail(1)),
            KeyCode::PageUp => Some(UserAction::ScrollReviewArtifactDetail(-8)),
            KeyCode::PageDown => Some(UserAction::ScrollReviewArtifactDetail(8)),
            _ => None,
        };
    }
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
        KeyCode::Char('a') if in_threads_view || in_review_view => {
            Some(UserAction::ApproveSelectedSupervisorDecision)
        }
        KeyCode::Char('c') if in_threads_view => Some(UserAction::ResumeSelectedThreadInCodex),
        KeyCode::Char('d') if in_threads_view || in_review_view => {
            Some(UserAction::RejectSelectedSupervisorDecision)
        }
        KeyCode::Char('v') if in_review_view => {
            Some(UserAction::OpenSelectedProposalArtifactDetail)
        }
        KeyCode::Char('s') if in_threads_view => Some(UserAction::ProposeSteerForSelectedThread),
        KeyCode::Char('e') if in_threads_view => {
            Some(UserAction::EditPendingSteerForSelectedThread)
        }
        KeyCode::Char('i') if in_threads_view => {
            Some(UserAction::ProposeInterruptForSelectedThread)
        }
        KeyCode::Char('w') if in_threads_view => Some(UserAction::RecordNoActionForSelectedThread),
        KeyCode::Char('m') if in_threads_view => Some(UserAction::ManualRefreshForSelectedThread),
        KeyCode::Char('n') if in_main_view => Some(UserAction::CreateWorkstream),
        KeyCode::Char('u') if in_main_view => Some(UserAction::CreateWorkUnitForSelection),
        KeyCode::Char('t') if in_main_view => Some(UserAction::CreateTrackedThreadForSelection),
        KeyCode::Char('e') if in_main_view => Some(UserAction::EditSelectedMainEntity),
        KeyCode::Char('d') if in_main_view => Some(UserAction::DeleteSelectedMainEntity),
        KeyCode::Down => Some(UserAction::SelectNextInView),
        KeyCode::Up => Some(UserAction::SelectPreviousInView),
        KeyCode::Left if in_main_view => Some(UserAction::CollapseSelectedInView),
        KeyCode::Right if in_main_view => Some(UserAction::ExpandSelectedInView),
        KeyCode::Left | KeyCode::Right if in_overview_program => None,
        KeyCode::Left => Some(UserAction::ShowView(current_view.previous())),
        KeyCode::Right => Some(UserAction::ShowView(current_view.next())),
        KeyCode::Tab if in_overview_program => Some(UserAction::CycleProgramView),
        KeyCode::Tab if current_view == TopLevelView::Collaboration => {
            Some(UserAction::CycleCollaborationFocus)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};
    use crossterm::event::{KeyCode, KeyModifiers};
    use orcas_core::WorkstreamStatus;
    use std::path::PathBuf;

    #[test]
    fn parses_direct_tui_runtime_flags() {
        let cli = TuiCli::parse_from([
            "orcas-tui",
            "--codex-bin",
            "/tmp/codex",
            "--listen-url",
            "ws://127.0.0.1:4510",
            "--cwd",
            "/tmp/work",
            "--model",
            "gpt-5.4",
            "--connect-only",
        ]);

        assert_eq!(
            cli.runtime.codex_bin.as_deref(),
            Some(std::path::Path::new("/tmp/codex"))
        );
        assert_eq!(
            cli.runtime.listen_url.as_deref(),
            Some("ws://127.0.0.1:4510")
        );
        assert_eq!(
            cli.runtime.cwd.as_deref(),
            Some(std::path::Path::new("/tmp/work"))
        );
        assert_eq!(cli.runtime.model.as_deref(), Some("gpt-5.4"));
        assert!(cli.runtime.connect_only);
        assert!(!cli.runtime.force_spawn);
    }

    #[test]
    fn tui_runtime_args_default_cleanly() {
        let runtime = TuiRuntimeArgs::default();

        assert!(runtime.codex_bin.is_none());
        assert!(runtime.listen_url.is_none());
        assert!(runtime.cwd.is_none());
        assert!(runtime.model.is_none());
        assert!(!runtime.connect_only);
        assert!(!runtime.force_spawn);
    }

    #[test]
    fn tui_runtime_args_convert_to_runtime_overrides() {
        let runtime = TuiRuntimeArgs {
            codex_bin: Some(PathBuf::from("/tmp/codex")),
            listen_url: Some("ws://127.0.0.1:4510".to_string()),
            cwd: Some(PathBuf::from("/tmp/work")),
            model: Some("gpt-5.4".to_string()),
            connect_only: true,
            force_spawn: false,
        };

        let overrides = runtime.into_runtime_overrides();

        assert_eq!(
            overrides.codex_bin.as_deref(),
            Some(std::path::Path::new("/tmp/codex"))
        );
        assert_eq!(overrides.listen_url.as_deref(), Some("ws://127.0.0.1:4510"));
        assert_eq!(
            overrides.cwd.as_deref(),
            Some(std::path::Path::new("/tmp/work"))
        );
        assert_eq!(overrides.model.as_deref(), Some("gpt-5.4"));
        assert!(overrides.connect_only);
        assert!(!overrides.force_spawn);
    }

    #[test]
    fn tui_help_mentions_the_terminal_ui() {
        let help = TuiCli::command().render_help().to_string();

        assert!(help.contains("Orcas terminal UI"));
        assert!(help.contains("--connect-only"));
        assert!(help.contains("--force-spawn"));
    }

    #[test]
    fn tui_version_matches_crate_version() {
        let version = TuiCli::command().render_version().to_string();

        assert!(version.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn left_and_right_cycle_top_level_views_outside_main_surface() {
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
            action_for_key(
                &state_for_view(TopLevelView::Supervisor),
                key(KeyCode::Right)
            ),
            Some(UserAction::ShowView(TopLevelView::Overview))
        );
    }

    #[test]
    fn arrow_keys_drive_selection_and_tab_switches_view_specific_focus() {
        assert_eq!(
            action_for_key(
                &state_for_overview_program(ProgramView::Main),
                key(KeyCode::Left)
            ),
            Some(UserAction::CollapseSelectedInView)
        );
        assert_eq!(
            action_for_key(
                &state_for_overview_program(ProgramView::Main),
                key(KeyCode::Right)
            ),
            Some(UserAction::ExpandSelectedInView)
        );
        assert_eq!(
            action_for_key(
                &state_for_overview_program(ProgramView::Main),
                key(KeyCode::Tab)
            ),
            Some(UserAction::CycleProgramView)
        );
        assert_eq!(
            action_for_key(
                &state_for_overview_program(ProgramView::Review),
                key(KeyCode::Left)
            ),
            None
        );
        assert_eq!(
            action_for_key(
                &state_for_overview_program(ProgramView::Review),
                key(KeyCode::Right)
            ),
            None
        );
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
                key(KeyCode::Char('c'))
            ),
            Some(UserAction::ResumeSelectedThreadInCodex)
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
        assert_eq!(
            action_for_key(
                &state_for_view(TopLevelView::Threads),
                key(KeyCode::Char('w'))
            ),
            Some(UserAction::RecordNoActionForSelectedThread)
        );
        assert_eq!(
            action_for_key(
                &state_for_view(TopLevelView::Threads),
                key(KeyCode::Char('m'))
            ),
            Some(UserAction::ManualRefreshForSelectedThread)
        );
    }

    #[test]
    fn review_view_maps_supervisor_review_actions() {
        assert_eq!(
            action_for_key(
                &state_for_overview_program(ProgramView::Review),
                key(KeyCode::Char('a'))
            ),
            Some(UserAction::ApproveSelectedSupervisorDecision)
        );
        assert_eq!(
            action_for_key(
                &state_for_overview_program(ProgramView::Review),
                key(KeyCode::Char('d'))
            ),
            Some(UserAction::RejectSelectedSupervisorDecision)
        );
        assert_eq!(
            action_for_key(
                &state_for_overview_program(ProgramView::Review),
                key(KeyCode::Char('c'))
            ),
            None
        );
        assert_eq!(
            action_for_key(
                &state_for_overview_program(ProgramView::Review),
                key(KeyCode::Char('v'))
            ),
            Some(UserAction::OpenSelectedProposalArtifactDetail)
        );
    }

    #[test]
    fn review_artifact_detail_overlay_captures_scroll_and_close_keys() {
        let mut state = state_for_overview_program(ProgramView::Review);
        state.review_view.artifact_detail = Some(orcas_tui::app::ReviewArtifactDetailState {
            proposal_id: "proposal-1".to_string(),
            scroll_offset: 0,
        });

        assert_eq!(
            action_for_key(&state, key(KeyCode::Esc)),
            Some(UserAction::CloseReviewArtifactDetail)
        );
        assert_eq!(
            action_for_key(&state, key(KeyCode::Down)),
            Some(UserAction::ScrollReviewArtifactDetail(1))
        );
        assert_eq!(
            action_for_key(&state, key(KeyCode::Up)),
            Some(UserAction::ScrollReviewArtifactDetail(-1))
        );
        assert_eq!(action_for_key(&state, key(KeyCode::Char('a'))), None);
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

    #[test]
    fn main_footer_enter_advances_until_last_field_then_submits() {
        let mut state = state_for_overview_program(ProgramView::Main);
        state.authority_main.footer = orcas_tui::app::MainFooterState::CreateWorkstream(
            orcas_tui::app::WorkstreamFooterForm {
                workstream_id: None,
                expected_revision: None,
                title: orcas_tui::app::FooterFieldState {
                    value: "alpha".to_string(),
                    cursor: 5,
                },
                root_dir: orcas_tui::app::FooterFieldState {
                    value: "/repo/orcas".to_string(),
                    cursor: 11,
                },
                status: WorkstreamStatus::Active,
                priority: "0".to_string(),
                active_field: 0,
            },
        );
        assert_eq!(
            action_for_key(&state, key(KeyCode::Enter)),
            Some(UserAction::MainFooterNextField)
        );
        state.authority_main.footer = match state.authority_main.footer {
            orcas_tui::app::MainFooterState::CreateWorkstream(mut form) => {
                form.active_field = 1;
                orcas_tui::app::MainFooterState::CreateWorkstream(form)
            }
            other => other,
        };
        assert_eq!(
            action_for_key(&state, key(KeyCode::Enter)),
            Some(UserAction::SubmitMainFooter)
        );
    }

    fn state_for_view(view: TopLevelView) -> orcas_tui::app::AppState {
        orcas_tui::app::AppState {
            current_view: view,
            ..Default::default()
        }
    }

    fn state_for_overview_program(program_view: ProgramView) -> orcas_tui::app::AppState {
        let mut state = state_for_view(TopLevelView::Overview);
        state.main_view.program_view = program_view;
        state
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl_key(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
    }
}
