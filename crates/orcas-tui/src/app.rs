use std::collections::{BTreeSet, HashMap, VecDeque};

use crate::codex::CodexThreadSessions;
use orcas_core::{ConnectionState, ipc};

const MAX_LOG_ENTRIES: usize = 64;
const MAIN_HIERARCHY_SCROLL_WINDOW: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TopLevelView {
    #[default]
    Overview,
    Threads,
    Collaboration,
    Supervisor,
}

impl TopLevelView {
    pub fn next(self) -> Self {
        match self {
            Self::Overview => Self::Threads,
            Self::Threads => Self::Collaboration,
            Self::Collaboration => Self::Supervisor,
            Self::Supervisor => Self::Overview,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            Self::Overview => Self::Supervisor,
            Self::Threads => Self::Overview,
            Self::Collaboration => Self::Threads,
            Self::Supervisor => Self::Collaboration,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CollaborationFocus {
    #[default]
    Workstreams,
    WorkUnits,
}

impl CollaborationFocus {
    pub fn next(self) -> Self {
        match self {
            Self::Workstreams => Self::WorkUnits,
            Self::WorkUnits => Self::Workstreams,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProgramView {
    #[default]
    Main,
    Review,
}

impl ProgramView {
    pub fn next(self) -> Self {
        match self {
            Self::Main => Self::Review,
            Self::Review => Self::Main,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MainHierarchySelection {
    Workstream {
        workstream_id: String,
    },
    WorkUnit {
        workstream_id: String,
        work_unit_id: String,
    },
    Thread {
        workstream_id: String,
        work_unit_id: String,
        thread_id: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MainViewState {
    pub program_view: ProgramView,
    pub selected: Option<MainHierarchySelection>,
    pub expanded_workstreams: BTreeSet<String>,
    pub expanded_work_units: BTreeSet<String>,
    pub scroll_offset: usize,
    pub initialized: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DaemonConnectionPhase {
    #[default]
    Disconnected,
    Reconnecting,
    Connected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DaemonLifecycleState {
    #[default]
    Unknown,
    Stopped,
    Starting,
    Stopping,
    Restarting,
    Running,
    Failed,
}

#[derive(Debug, Clone, Default)]
pub struct AppState {
    pub daemon: Option<ipc::DaemonStatusResponse>,
    pub daemon_phase: DaemonConnectionPhase,
    pub daemon_lifecycle: DaemonLifecycleState,
    pub daemon_lifecycle_error: Option<String>,
    pub reconnect_attempt: u32,
    pub session: ipc::SessionState,
    pub collaboration: ipc::CollaborationSnapshot,
    pub threads: Vec<ipc::ThreadSummary>,
    pub daemon_models: Vec<ipc::ModelSummary>,
    pub models_loading: bool,
    pub thread_details: HashMap<String, ipc::ThreadView>,
    pub turn_states: HashMap<String, ipc::TurnStateView>,
    pub codex_sessions: HashMap<String, CodexThreadSessions>,
    pub current_view: TopLevelView,
    pub main_view: MainViewState,
    pub selected_thread_id: Option<String>,
    pub selected_workstream_id: Option<String>,
    pub selected_work_unit_id: Option<String>,
    pub work_unit_details: HashMap<String, ipc::WorkunitGetResponse>,
    pub collaboration_focus: CollaborationFocus,
    pub recent_events: VecDeque<ipc::EventSummary>,
    pub prompt_in_flight: bool,
    pub steer_compose: Option<SteerComposeState>,
    pub banner: Option<StatusBanner>,
    pub show_help: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteerComposeState {
    pub assignment_id: String,
    pub thread_id: String,
    pub replace_decision_id: Option<String>,
    pub buffer: String,
    pub cursor: usize,
    pub preferred_column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusBanner {
    pub level: BannerLevel,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub enum Action {
    Start,
    User(UserAction),
    Event(UiEvent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserAction {
    Refresh,
    LoadModels,
    StartDaemon,
    RestartDaemon,
    StopDaemon,
    ToggleHelp,
    CycleView,
    ShowView(TopLevelView),
    CycleProgramView,
    ShowProgramView(ProgramView),
    CycleCollaborationFocus,
    SelectNextInView,
    SelectPreviousInView,
    ExpandSelectedInView,
    CollapseSelectedInView,
    SelectNextThread,
    SelectPreviousThread,
    SelectThread(String),
    EnterPromptMode,
    ExitPromptMode,
    PromptAppend(char),
    PromptBackspace,
    SubmitPrompt,
    ResumeSelectedThreadInCodex,
    ProposeSteerForSelectedThread,
    EditPendingSteerForSelectedThread,
    SteerComposeAppend(char),
    SteerComposeInsertNewline,
    SteerComposeBackspace,
    SteerComposeDelete,
    SteerComposeMoveLeft,
    SteerComposeMoveRight,
    SteerComposeMoveUp,
    SteerComposeMoveDown,
    SubmitSteerCompose,
    CancelSteerCompose,
    ProposeInterruptForSelectedThread,
    RecordNoActionForSelectedThread,
    ManualRefreshForSelectedThread,
    ApproveSelectedSupervisorDecision,
    RejectSelectedSupervisorDecision,
}

#[derive(Debug, Clone)]
pub enum UiEvent {
    SnapshotLoaded(ipc::StateSnapshot),
    ReconnectScheduled {
        attempt: u32,
        delay_ms: u64,
    },
    ConnectionLost(String),
    ThreadLoaded(ipc::ThreadView),
    ThreadAttached(ipc::ThreadAttachResponse),
    ActiveTurnsLoaded(Vec<ipc::TurnStateView>),
    TurnStateLoaded(ipc::TurnAttachResponse),
    ModelsLoaded(Vec<ipc::ModelSummary>),
    DaemonStarted {
        connected: bool,
    },
    DaemonStopped {
        stopping: bool,
    },
    DaemonStartFailed(String),
    DaemonStopFailed(String),
    PromptStarted {
        thread_id: String,
        turn_id: String,
    },
    SteerComposeCommitted {
        decision_id: String,
    },
    UpstreamChanged(ConnectionState),
    SessionChanged(ipc::SessionState),
    ThreadUpdated(ipc::ThreadSummary),
    WorkstreamLifecycle {
        action: ipc::CollaborationLifecycleAction,
        workstream: ipc::WorkstreamSummary,
    },
    WorkUnitLifecycle {
        action: ipc::CollaborationLifecycleAction,
        work_unit: ipc::WorkUnitSummary,
    },
    AssignmentLifecycle {
        action: ipc::AssignmentLifecycleAction,
        assignment: ipc::AssignmentSummary,
    },
    CodexAssignmentLifecycle {
        action: ipc::CodexAssignmentLifecycleAction,
        assignment: ipc::CodexThreadAssignmentSummary,
    },
    SupervisorDecisionLifecycle {
        action: ipc::SupervisorDecisionLifecycleAction,
        decision: ipc::SupervisorTurnDecisionSummary,
    },
    ReportRecorded(ipc::ReportSummary),
    DecisionApplied(ipc::DecisionSummary),
    ProposalLifecycle {
        action: ipc::ProposalLifecycleAction,
        proposal: ipc::ProposalSummary,
        work_unit: ipc::WorkUnitSummary,
    },
    WorkUnitDetailLoaded(ipc::WorkunitGetResponse),
    TurnUpdated {
        thread_id: String,
        turn: ipc::TurnView,
    },
    ItemUpdated {
        thread_id: String,
        turn_id: String,
        item: ipc::ItemView,
    },
    OutputDelta {
        thread_id: String,
        turn_id: String,
        item_id: String,
        delta: String,
    },
    CodexSessionsChanged {
        sessions: HashMap<String, CodexThreadSessions>,
    },
    Ignored,
    Warning(String),
    Error(String),
}

impl UiEvent {
    pub fn from_daemon(event: ipc::DaemonEventEnvelope) -> Self {
        match event.event {
            ipc::DaemonEvent::UpstreamStatusChanged { upstream } => Self::UpstreamChanged(upstream),
            ipc::DaemonEvent::SessionChanged { session } => Self::SessionChanged(session),
            ipc::DaemonEvent::ThreadUpdated { thread } => Self::ThreadUpdated(thread),
            ipc::DaemonEvent::WorkstreamLifecycle { action, workstream } => {
                Self::WorkstreamLifecycle { action, workstream }
            }
            ipc::DaemonEvent::WorkUnitLifecycle { action, work_unit } => {
                Self::WorkUnitLifecycle { action, work_unit }
            }
            ipc::DaemonEvent::AssignmentLifecycle { action, assignment } => {
                Self::AssignmentLifecycle { action, assignment }
            }
            ipc::DaemonEvent::CodexAssignmentLifecycle { action, assignment } => {
                Self::CodexAssignmentLifecycle { action, assignment }
            }
            ipc::DaemonEvent::SupervisorDecisionLifecycle { action, decision } => {
                Self::SupervisorDecisionLifecycle { action, decision }
            }
            ipc::DaemonEvent::ReportRecorded { report } => Self::ReportRecorded(report),
            ipc::DaemonEvent::DecisionApplied { decision } => Self::DecisionApplied(decision),
            ipc::DaemonEvent::ProposalLifecycle {
                action,
                proposal,
                work_unit,
            } => Self::ProposalLifecycle {
                action,
                proposal,
                work_unit,
            },
            ipc::DaemonEvent::TurnUpdated { thread_id, turn } => {
                Self::TurnUpdated { thread_id, turn }
            }
            ipc::DaemonEvent::ItemUpdated {
                thread_id,
                turn_id,
                item,
            } => Self::ItemUpdated {
                thread_id,
                turn_id,
                item,
            },
            ipc::DaemonEvent::OutputDelta {
                thread_id,
                turn_id,
                item_id,
                delta,
            } => Self::OutputDelta {
                thread_id,
                turn_id,
                item_id,
                delta,
            },
            ipc::DaemonEvent::Warning { message } => Self::Warning(message),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Effect {
    RefreshSnapshot,
    SubscribeEvents,
    ScheduleReconnect,
    LoadActiveTurns,
    LoadThread {
        thread_id: String,
    },
    AttachThread {
        thread_id: String,
    },
    LoadTurnState {
        thread_id: String,
        turn_id: String,
    },
    LoadWorkUnitDetail {
        work_unit_id: String,
    },
    SubmitPrompt {
        thread_id: String,
        text: String,
    },
    ProposeSteerDecision {
        assignment_id: String,
        proposed_text: String,
    },
    ReplacePendingSteerDecision {
        decision_id: String,
        proposed_text: String,
    },
    ProposeInterruptDecision {
        assignment_id: String,
    },
    RecordNoActionDecision {
        decision_id: String,
    },
    ManualRefreshDecision {
        assignment_id: String,
    },
    ApproveSupervisorDecision {
        decision_id: String,
    },
    RejectSupervisorDecision {
        decision_id: String,
    },
    LoadModels,
    StartDaemon,
    RestartDaemon,
    StopDaemon,
}

pub fn reduce(state: &mut AppState, action: Action) -> Vec<Effect> {
    match action {
        Action::Start => vec![Effect::RefreshSnapshot],
        Action::User(user_action) => reduce_user_action(state, user_action),
        Action::Event(event) => reduce_event(state, event),
    }
}

fn reduce_user_action(state: &mut AppState, action: UserAction) -> Vec<Effect> {
    match action {
        UserAction::Refresh => {
            let mut effects = vec![Effect::RefreshSnapshot];
            if state.current_view == TopLevelView::Supervisor {
                state.models_loading = true;
                effects.push(Effect::LoadModels);
            }
            effects
        }
        UserAction::LoadModels => {
            state.models_loading = true;
            vec![Effect::LoadModels]
        }
        UserAction::StopDaemon => {
            if is_daemon_action_in_flight(state) {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Daemon action in progress. Please wait.".to_string(),
                });
                return Vec::new();
            }
            if !daemon_is_connected(state) {
                state.daemon_lifecycle = DaemonLifecycleState::Stopped;
                state.daemon_lifecycle_error = Some("daemon already stopped".to_string());
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Info,
                    message: "Daemon already stopped.".to_string(),
                });
                return Vec::new();
            }
            state.daemon_lifecycle_error = None;
            state.daemon_lifecycle = DaemonLifecycleState::Stopping;
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: "Stopping daemon...".to_string(),
            });
            vec![Effect::StopDaemon]
        }
        UserAction::StartDaemon => {
            if is_daemon_action_in_flight(state) {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Daemon action in progress. Please wait.".to_string(),
                });
                return Vec::new();
            }
            if daemon_is_connected(state) {
                state.daemon_lifecycle = DaemonLifecycleState::Running;
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Info,
                    message: "Daemon already running.".to_string(),
                });
                return Vec::new();
            }
            state.daemon_lifecycle_error = None;
            state.daemon_lifecycle = DaemonLifecycleState::Starting;
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: "Starting daemon...".to_string(),
            });
            vec![Effect::StartDaemon]
        }
        UserAction::RestartDaemon => {
            if is_daemon_action_in_flight(state) {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Daemon action in progress. Please wait.".to_string(),
                });
                return Vec::new();
            }
            state.daemon_lifecycle_error = None;
            state.daemon_lifecycle = DaemonLifecycleState::Restarting;
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: "Restarting daemon...".to_string(),
            });
            vec![Effect::RestartDaemon]
        }
        UserAction::ToggleHelp => {
            state.show_help = !state.show_help;
            Vec::new()
        }
        UserAction::CycleView => {
            state.current_view = state.current_view.next();
            if state.current_view == TopLevelView::Supervisor {
                state.models_loading = true;
                vec![Effect::LoadModels]
            } else {
                Vec::new()
            }
        }
        UserAction::ShowView(view) => {
            state.current_view = view;
            if state.current_view == TopLevelView::Supervisor {
                state.models_loading = true;
                vec![Effect::LoadModels]
            } else {
                Vec::new()
            }
        }
        UserAction::CycleProgramView => {
            if state.current_view == TopLevelView::Overview {
                state.main_view.program_view = state.main_view.program_view.next();
            }
            Vec::new()
        }
        UserAction::ShowProgramView(view) => {
            if state.current_view == TopLevelView::Overview {
                state.main_view.program_view = view;
            }
            Vec::new()
        }
        UserAction::CycleCollaborationFocus => {
            if state.current_view == TopLevelView::Collaboration {
                state.collaboration_focus = state.collaboration_focus.next();
            }
            Vec::new()
        }
        UserAction::SelectNextInView => select_relative_in_view(state, 1),
        UserAction::SelectPreviousInView => select_relative_in_view(state, -1),
        UserAction::ExpandSelectedInView => expand_selected_in_view(state),
        UserAction::CollapseSelectedInView => collapse_selected_in_view(state),
        UserAction::SelectNextThread => select_relative_thread(state, 1),
        UserAction::SelectPreviousThread => select_relative_thread(state, -1),
        UserAction::SelectThread(thread_id) => select_thread(state, thread_id),
        UserAction::EnterPromptMode
        | UserAction::ExitPromptMode
        | UserAction::PromptAppend(_)
        | UserAction::PromptBackspace
        | UserAction::SubmitPrompt => {
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: "TUI is read-only in this pass. Use the Orcas CLI for prompt submission."
                    .to_string(),
            });
            Vec::new()
        }
        UserAction::ResumeSelectedThreadInCodex => {
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: "Resume in Codex is handled by the terminal host.".to_string(),
            });
            Vec::new()
        }
        UserAction::ProposeSteerForSelectedThread => {
            let Some(thread_id) = state.selected_thread_id.clone() else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Selected thread cannot propose a steer right now.".to_string(),
                });
                return Vec::new();
            };
            let Some(assignment_id) = selected_thread_steer_assignment_id(state) else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Selected thread cannot propose a steer right now.".to_string(),
                });
                return Vec::new();
            };
            state.steer_compose = Some(SteerComposeState {
                assignment_id: assignment_id.clone(),
                thread_id,
                replace_decision_id: None,
                buffer: String::new(),
                cursor: 0,
                preferred_column: 0,
            });
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: "Author multiline steer text. Ctrl+S saves, Esc cancels.".to_string(),
            });
            Vec::new()
        }
        UserAction::EditPendingSteerForSelectedThread => {
            let Some(thread_id) = state.selected_thread_id.clone() else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Selected thread has no editable pending steer decision.".to_string(),
                });
                return Vec::new();
            };
            let Some(decision) = selected_thread_pending_steer_decision(state).cloned() else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Selected thread has no editable pending steer decision.".to_string(),
                });
                return Vec::new();
            };
            let buffer = decision.proposed_text.clone().unwrap_or_default();
            let cursor = buffer.len();
            let preferred_column = compose_column_at(&buffer, cursor);
            state.steer_compose = Some(SteerComposeState {
                assignment_id: decision.assignment_id,
                thread_id,
                replace_decision_id: Some(decision.decision_id),
                buffer,
                cursor,
                preferred_column,
            });
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: "Edit multiline steer text. Ctrl+S saves, Esc cancels.".to_string(),
            });
            Vec::new()
        }
        UserAction::SteerComposeAppend(ch) => {
            let Some(compose) = state.steer_compose.as_mut() else {
                return Vec::new();
            };
            steer_compose_insert(compose, ch);
            state.banner = None;
            Vec::new()
        }
        UserAction::SteerComposeInsertNewline => {
            let Some(compose) = state.steer_compose.as_mut() else {
                return Vec::new();
            };
            steer_compose_insert(compose, '\n');
            state.banner = None;
            Vec::new()
        }
        UserAction::SteerComposeBackspace => {
            if let Some(compose) = state.steer_compose.as_mut() {
                steer_compose_backspace(compose);
            }
            state.banner = None;
            Vec::new()
        }
        UserAction::SteerComposeDelete => {
            if let Some(compose) = state.steer_compose.as_mut() {
                steer_compose_delete(compose);
            }
            state.banner = None;
            Vec::new()
        }
        UserAction::SteerComposeMoveLeft => {
            if let Some(compose) = state.steer_compose.as_mut() {
                steer_compose_move_left(compose);
            }
            state.banner = None;
            Vec::new()
        }
        UserAction::SteerComposeMoveRight => {
            if let Some(compose) = state.steer_compose.as_mut() {
                steer_compose_move_right(compose);
            }
            state.banner = None;
            Vec::new()
        }
        UserAction::SteerComposeMoveUp => {
            if let Some(compose) = state.steer_compose.as_mut() {
                steer_compose_move_vertical(compose, -1);
            }
            state.banner = None;
            Vec::new()
        }
        UserAction::SteerComposeMoveDown => {
            if let Some(compose) = state.steer_compose.as_mut() {
                steer_compose_move_vertical(compose, 1);
            }
            state.banner = None;
            Vec::new()
        }
        UserAction::SubmitSteerCompose => {
            let Some(compose) = state.steer_compose.as_ref() else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "No steer proposal is being edited.".to_string(),
                });
                return Vec::new();
            };
            let proposed_text = compose.buffer.trim().to_string();
            if proposed_text.is_empty() {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Steer text must not be empty.".to_string(),
                });
                return Vec::new();
            }
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: if let Some(decision_id) = compose.replace_decision_id.as_ref() {
                    format!("Replacing pending steer proposal {}...", decision_id)
                } else {
                    format!(
                        "Creating steer proposal for assignment {}...",
                        compose.assignment_id
                    )
                },
            });
            if let Some(decision_id) = compose.replace_decision_id.clone() {
                vec![Effect::ReplacePendingSteerDecision {
                    decision_id,
                    proposed_text,
                }]
            } else {
                vec![Effect::ProposeSteerDecision {
                    assignment_id: compose.assignment_id.clone(),
                    proposed_text,
                }]
            }
        }
        UserAction::CancelSteerCompose => {
            if state.steer_compose.take().is_some() {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Info,
                    message: "Cancelled steer text editing.".to_string(),
                });
            }
            Vec::new()
        }
        UserAction::ProposeInterruptForSelectedThread => {
            let Some(assignment_id) = selected_thread_interrupt_assignment_id(state) else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Selected thread cannot propose an interrupt right now.".to_string(),
                });
                return Vec::new();
            };
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: format!(
                    "Creating interrupt proposal for assignment {}...",
                    assignment_id
                ),
            });
            vec![Effect::ProposeInterruptDecision { assignment_id }]
        }
        UserAction::RecordNoActionForSelectedThread => {
            let Some(decision_id) = selected_thread_record_no_action_decision_id(state) else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message:
                        "Selected thread has no pending next-turn proposal to record as no action."
                            .to_string(),
                });
                return Vec::new();
            };
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: format!(
                    "Recording no_action for supervisor proposal {}...",
                    decision_id
                ),
            });
            vec![Effect::RecordNoActionDecision { decision_id }]
        }
        UserAction::ManualRefreshForSelectedThread => {
            let Some(assignment_id) = selected_thread_manual_refresh_assignment_id(state) else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message:
                        "Selected thread cannot manual-refresh a next-turn proposal right now."
                            .to_string(),
                });
                return Vec::new();
            };
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: format!(
                    "Regenerating next-turn proposal for assignment {}...",
                    assignment_id
                ),
            });
            vec![Effect::ManualRefreshDecision { assignment_id }]
        }
        UserAction::ApproveSelectedSupervisorDecision => {
            let Some(decision_id) = selected_thread_pending_supervisor_decision(state)
                .map(|decision| decision.decision_id.clone())
            else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Selected thread has no pending supervisor proposal.".to_string(),
                });
                return Vec::new();
            };
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: format!(
                    "Approving and sending supervisor proposal {}...",
                    decision_id
                ),
            });
            vec![Effect::ApproveSupervisorDecision { decision_id }]
        }
        UserAction::RejectSelectedSupervisorDecision => {
            let Some(decision_id) = selected_thread_pending_supervisor_decision(state)
                .map(|decision| decision.decision_id.clone())
            else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Selected thread has no pending supervisor proposal.".to_string(),
                });
                return Vec::new();
            };
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: format!("Rejecting supervisor proposal {}...", decision_id),
            });
            vec![Effect::RejectSupervisorDecision { decision_id }]
        }
    }
}

fn reduce_event(state: &mut AppState, event: UiEvent) -> Vec<Effect> {
    let mut effects = Vec::new();
    if let Some(summary) = event_summary_from_ui_event(&event) {
        push_event_summary(state, summary);
    }

    match event {
        UiEvent::SnapshotLoaded(snapshot) => {
            let preserved_thread_selection = state.selected_thread_id.clone();
            state.daemon_phase = DaemonConnectionPhase::Connected;
            state.reconnect_attempt = 0;
            state.daemon_lifecycle =
                lifecycle_from_upstream_status(&snapshot.daemon.upstream.status);
            state.daemon_lifecycle_error = None;
            state.daemon = Some(snapshot.daemon);
            state.session = snapshot.session;
            state.collaboration = snapshot.collaboration;
            state.threads = snapshot.threads;
            state.thread_details.clear();
            state.turn_states.clear();
            state.work_unit_details.clear();
            state.recent_events = snapshot.recent_events.into_iter().collect();
            state.selected_thread_id = preserved_thread_selection;
            state.prompt_in_flight = false;
            if let Some(thread) = snapshot.active_thread {
                let turn_ids = thread
                    .turns
                    .iter()
                    .map(|turn| turn.id.clone())
                    .collect::<Vec<_>>();
                state.selected_thread_id = Some(thread.summary.id.clone());
                state
                    .thread_details
                    .insert(thread.summary.id.clone(), thread.clone());
                effects.extend(turn_ids.into_iter().map(|turn_id| Effect::LoadTurnState {
                    thread_id: thread.summary.id.clone(),
                    turn_id,
                }));
            }
            let preferred_thread_id = state.session.active_thread_id.clone();
            reconcile_thread_selection(state, preferred_thread_id.as_deref());
            if let Some(thread_id) = state.selected_thread_id.clone()
                && !state.thread_details.contains_key(&thread_id)
            {
                effects.push(Effect::LoadThread { thread_id });
            }
            if let Some(thread_id) = state.selected_thread_id.clone()
                && let Some(thread) = state.thread_details.get(&thread_id)
                && thread.summary.monitor_state != ipc::ThreadMonitorState::Attached
            {
                effects.push(Effect::AttachThread { thread_id });
            }
            reconcile_collaboration_selection(state);
            reconcile_main_view(state);
            effects.extend(load_selected_work_unit_detail_if_needed(state));
            effects.push(Effect::LoadActiveTurns);
            state.banner = None;
        }
        UiEvent::ReconnectScheduled { attempt, delay_ms } => {
            state.daemon_phase = DaemonConnectionPhase::Reconnecting;
            state.reconnect_attempt = attempt;
            state.banner = Some(StatusBanner {
                level: BannerLevel::Warning,
                message: format!(
                    "Daemon unavailable. Reconnecting in {delay_ms}ms (attempt {attempt})."
                ),
            });
        }
        UiEvent::ConnectionLost(message) => {
            state.models_loading = false;
            state.daemon_phase = DaemonConnectionPhase::Reconnecting;
            state.prompt_in_flight = false;
            if let Some(daemon) = state.daemon.as_mut() {
                daemon.upstream.status = "disconnected".to_string();
                daemon.upstream.detail = Some(message.clone());
            }
            state.daemon_lifecycle = DaemonLifecycleState::Failed;
            state.daemon_lifecycle_error = Some(format!("daemon unavailable: {message}"));
            state.banner = Some(StatusBanner {
                level: BannerLevel::Warning,
                message: format!("Daemon unavailable: {message}"),
            });
            effects.push(Effect::ScheduleReconnect);
        }
        UiEvent::ThreadLoaded(thread) => {
            let thread_id = thread.summary.id.clone();
            let turn_ids = thread
                .turns
                .iter()
                .map(|turn| turn.id.clone())
                .collect::<Vec<_>>();
            upsert_thread_summary(&mut state.threads, thread.summary.clone());
            state.thread_details.insert(thread_id.clone(), thread);
            if state.selected_thread_id.is_none() {
                state.selected_thread_id = Some(thread_id.clone());
            }
            effects.extend(turn_ids.into_iter().map(|turn_id| Effect::LoadTurnState {
                thread_id: thread_id.clone(),
                turn_id,
            }));
            if state.thread_details.get(&thread_id).is_some_and(|thread| {
                thread.summary.monitor_state != ipc::ThreadMonitorState::Attached
            }) {
                effects.push(Effect::AttachThread { thread_id });
            }
            reconcile_main_view(state);
            state.banner = None;
        }
        UiEvent::ThreadAttached(response) => {
            if let Some(thread) = response.thread {
                let thread_id = thread.summary.id.clone();
                upsert_thread_summary(&mut state.threads, thread.summary.clone());
                state.thread_details.insert(thread_id, thread);
            }
            if !response.attached
                && let Some(reason) = response.reason
            {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: format!("Live attach unavailable: {reason}"),
                });
            }
            reconcile_main_view(state);
        }
        UiEvent::ActiveTurnsLoaded(turns) => {
            state.turn_states.retain(|_, turn| !turn.attachable);
            for turn in turns {
                upsert_turn_state(state, turn);
            }
            refresh_prompt_in_flight(state);
        }
        UiEvent::TurnStateLoaded(response) => {
            if let Some(turn) = response.turn {
                upsert_turn_state(state, turn);
            }
            refresh_prompt_in_flight(state);
        }
        UiEvent::PromptStarted { thread_id, turn_id } => {
            state.selected_thread_id = Some(thread_id.clone());
            state.prompt_in_flight = true;
            state.daemon_lifecycle_error = None;
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: "Prompt submitted.".to_string(),
            });
            effects.push(Effect::LoadTurnState { thread_id, turn_id });
        }
        UiEvent::SteerComposeCommitted { .. } => {
            state.steer_compose = None;
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: "Steer proposal saved for review.".to_string(),
            });
        }
        UiEvent::ModelsLoaded(models) => {
            state.daemon_models = models;
            state.models_loading = false;
            state.daemon_lifecycle_error = None;
        }
        UiEvent::DaemonStarted { connected } => {
            state.daemon_lifecycle = if connected {
                DaemonLifecycleState::Running
            } else {
                DaemonLifecycleState::Failed
            };
            state.daemon_lifecycle_error = if connected {
                None
            } else {
                Some("daemon start was not accepted".to_string())
            };
            state.banner = Some(StatusBanner {
                level: if connected {
                    BannerLevel::Info
                } else {
                    BannerLevel::Warning
                },
                message: if connected {
                    "Daemon start completed.".to_string()
                } else {
                    "Daemon start was not accepted.".to_string()
                },
            });
            effects.push(Effect::RefreshSnapshot);
            if state.current_view == TopLevelView::Supervisor {
                state.models_loading = true;
                effects.push(Effect::LoadModels);
            }
        }
        UiEvent::DaemonStopped { stopping } => {
            if stopping {
                state.daemon_lifecycle = DaemonLifecycleState::Stopping;
                state.daemon_lifecycle_error = None;
            } else {
                state.daemon_lifecycle = DaemonLifecycleState::Failed;
                state.daemon_lifecycle_error = Some("daemon stop was not accepted".to_string());
            }
            state.daemon_models.clear();
            state.banner = Some(StatusBanner {
                level: if stopping {
                    BannerLevel::Info
                } else {
                    BannerLevel::Warning
                },
                message: if stopping {
                    "Stopping daemon...".to_string()
                } else {
                    "Daemon stop was not accepted.".to_string()
                },
            });
            if stopping {
                effects.push(Effect::RefreshSnapshot);
            }
        }
        UiEvent::DaemonStartFailed(message) => {
            state.daemon_lifecycle = DaemonLifecycleState::Failed;
            state.daemon_lifecycle_error = Some(message.clone());
            state.banner = Some(StatusBanner {
                level: BannerLevel::Error,
                message: format!("Daemon start failed: {message}"),
            });
        }
        UiEvent::DaemonStopFailed(message) => {
            state.daemon_lifecycle = DaemonLifecycleState::Failed;
            state.daemon_lifecycle_error = Some(message.clone());
            state.banner = Some(StatusBanner {
                level: BannerLevel::Error,
                message: format!("Daemon stop failed: {message}"),
            });
        }
        UiEvent::UpstreamChanged(upstream) => {
            if let Some(daemon) = state.daemon.as_mut() {
                daemon.upstream = upstream.clone();
            }
            state.daemon_lifecycle = lifecycle_from_upstream_status(&upstream.status);
            state.daemon_lifecycle_error = None;
            if upstream.status != "connected" {
                state.prompt_in_flight = false;
            }
        }
        UiEvent::SessionChanged(session) => {
            let active_thread_id = session.active_thread_id.clone();
            state.session = session;
            effects.push(Effect::LoadActiveTurns);
            if let Some(thread_id) = active_thread_id {
                if state.selected_thread_id.is_none() {
                    state.selected_thread_id = Some(thread_id.clone());
                }
                if !state.thread_details.contains_key(&thread_id) {
                    effects.push(Effect::LoadThread { thread_id });
                } else {
                    effects.push(Effect::AttachThread { thread_id });
                }
            }
            reconcile_main_view(state);
        }
        UiEvent::ThreadUpdated(thread) => {
            let thread_id = thread.id.clone();
            upsert_thread_summary(&mut state.threads, thread.clone());
            if let Some(detail) = state.thread_details.get_mut(&thread_id) {
                detail.summary = thread;
            }
            if state.selected_thread_id.is_none() {
                state.selected_thread_id = Some(thread_id.clone());
            }
            if state.selected_thread_id.as_deref() == Some(thread_id.as_str())
                && !state.thread_details.contains_key(&thread_id)
            {
                effects.push(Effect::LoadThread { thread_id });
            }
            reconcile_main_view(state);
        }
        UiEvent::WorkstreamLifecycle { workstream, .. } => {
            upsert_workstream_summary(&mut state.collaboration.workstreams, workstream);
            reconcile_collaboration_selection(state);
            reconcile_main_view(state);
            effects.extend(load_selected_work_unit_detail_if_needed(state));
        }
        UiEvent::WorkUnitLifecycle { work_unit, .. } => {
            let selected = state.selected_work_unit_id.as_deref() == Some(work_unit.id.as_str());
            upsert_work_unit_summary(&mut state.collaboration.work_units, work_unit);
            reconcile_collaboration_selection(state);
            reconcile_main_view(state);
            if selected {
                effects.extend(load_selected_work_unit_detail(state));
            } else {
                effects.extend(load_selected_work_unit_detail_if_needed(state));
            }
        }
        UiEvent::AssignmentLifecycle { assignment, .. } => {
            let selected =
                state.selected_work_unit_id.as_deref() == Some(assignment.work_unit_id.as_str());
            upsert_assignment_summary(&mut state.collaboration.assignments, assignment);
            reconcile_main_view(state);
            if selected {
                effects.extend(load_selected_work_unit_detail(state));
            }
        }
        UiEvent::CodexAssignmentLifecycle { assignment, .. } => {
            upsert_codex_assignment_summary(
                &mut state.collaboration.codex_thread_assignments,
                assignment,
            );
            reconcile_main_view(state);
        }
        UiEvent::SupervisorDecisionLifecycle { decision, .. } => {
            upsert_supervisor_turn_decision_summary(
                &mut state.collaboration.supervisor_turn_decisions,
                decision,
            );
            reconcile_main_view(state);
        }
        UiEvent::ReportRecorded(report) => {
            let selected =
                state.selected_work_unit_id.as_deref() == Some(report.work_unit_id.as_str());
            upsert_report_summary(&mut state.collaboration.reports, report);
            reconcile_main_view(state);
            if selected {
                effects.extend(load_selected_work_unit_detail(state));
            }
        }
        UiEvent::DecisionApplied(decision) => {
            let selected =
                state.selected_work_unit_id.as_deref() == Some(decision.work_unit_id.as_str());
            upsert_decision_summary(&mut state.collaboration.decisions, decision);
            reconcile_main_view(state);
            if selected {
                effects.extend(load_selected_work_unit_detail(state));
            }
        }
        UiEvent::ProposalLifecycle {
            proposal,
            work_unit,
            ..
        } => {
            let selected = state.selected_work_unit_id.as_deref() == Some(work_unit.id.as_str());
            upsert_work_unit_summary(&mut state.collaboration.work_units, work_unit);
            reconcile_main_view(state);
            if selected {
                effects.extend(load_selected_work_unit_detail(state));
            } else {
                state
                    .work_unit_details
                    .remove(&proposal.primary_work_unit_id);
            }
        }
        UiEvent::WorkUnitDetailLoaded(detail) => {
            state
                .work_unit_details
                .insert(detail.work_unit.id.clone(), detail);
            reconcile_main_view(state);
        }
        UiEvent::TurnUpdated { thread_id, turn } => {
            ensure_thread_detail(state, &thread_id);
            if let Some(detail) = state.thread_details.get_mut(&thread_id) {
                upsert_turn(detail, turn.clone());
            }
            if matches!(
                turn.status.as_str(),
                "completed" | "failed" | "cancelled" | "interrupted" | "lost"
            ) {
                state.prompt_in_flight = false;
            }
            effects.push(Effect::LoadTurnState {
                thread_id,
                turn_id: turn.id,
            });
        }
        UiEvent::ItemUpdated {
            thread_id,
            turn_id,
            item,
        } => {
            ensure_thread_detail(state, &thread_id);
            if let Some(detail) = state.thread_details.get_mut(&thread_id) {
                let turn = ensure_turn(detail, &turn_id);
                upsert_item(turn, item);
            }
        }
        UiEvent::OutputDelta {
            thread_id,
            turn_id,
            item_id,
            delta,
        } => {
            ensure_thread_detail(state, &thread_id);
            if let Some(detail) = state.thread_details.get_mut(&thread_id) {
                let turn = ensure_turn(detail, &turn_id);
                let item = ensure_item(turn, &item_id);
                item.status = Some("streaming".to_string());
                item.text.get_or_insert_with(String::new).push_str(&delta);
            }
        }
        UiEvent::CodexSessionsChanged { sessions } => {
            state.codex_sessions = sessions;
            reconcile_main_view(state);
        }
        UiEvent::Ignored => {}
        UiEvent::Warning(message) => {
            state.banner = Some(StatusBanner {
                level: BannerLevel::Warning,
                message,
            });
        }
        UiEvent::Error(message) => {
            state.models_loading = false;
            state.prompt_in_flight = false;
            state.banner = Some(StatusBanner {
                level: BannerLevel::Error,
                message,
            });
        }
    }

    effects
}

fn select_relative_thread(state: &mut AppState, delta: isize) -> Vec<Effect> {
    if state.threads.is_empty() {
        return Vec::new();
    }
    let current_index = state
        .selected_thread_id
        .as_ref()
        .and_then(|thread_id| {
            state
                .threads
                .iter()
                .position(|thread| thread.id == *thread_id)
        })
        .unwrap_or(0);
    let next_index = if delta.is_negative() {
        current_index.saturating_sub(delta.unsigned_abs())
    } else {
        (current_index + delta as usize).min(state.threads.len().saturating_sub(1))
    };
    select_thread(state, state.threads[next_index].id.clone())
}

fn select_thread(state: &mut AppState, thread_id: String) -> Vec<Effect> {
    state.selected_thread_id = Some(thread_id.clone());
    if let Some(thread) = state.thread_details.get(&thread_id) {
        if thread.summary.monitor_state != ipc::ThreadMonitorState::Attached {
            vec![Effect::AttachThread { thread_id }]
        } else {
            Vec::new()
        }
    } else {
        vec![Effect::LoadThread { thread_id }]
    }
}

fn select_relative_in_view(state: &mut AppState, delta: isize) -> Vec<Effect> {
    match state.current_view {
        TopLevelView::Overview => select_relative_main_hierarchy(state, delta),
        TopLevelView::Threads => select_relative_thread(state, delta),
        TopLevelView::Collaboration => match state.collaboration_focus {
            CollaborationFocus::Workstreams => select_relative_workstream(state, delta),
            CollaborationFocus::WorkUnits => select_relative_work_unit(state, delta),
        },
        TopLevelView::Supervisor => Vec::new(),
    }
}

fn expand_selected_in_view(state: &mut AppState) -> Vec<Effect> {
    match state.current_view {
        TopLevelView::Overview => expand_selected_main_hierarchy(state),
        _ => Vec::new(),
    }
}

fn collapse_selected_in_view(state: &mut AppState) -> Vec<Effect> {
    match state.current_view {
        TopLevelView::Overview => collapse_selected_main_hierarchy(state),
        _ => Vec::new(),
    }
}

fn reconcile_main_view(state: &mut AppState) {
    state
        .main_view
        .expanded_workstreams
        .retain(|workstream_id| {
            state
                .collaboration
                .workstreams
                .iter()
                .any(|workstream| workstream.id == *workstream_id)
        });
    state.main_view.expanded_work_units.retain(|work_unit_id| {
        state
            .collaboration
            .work_units
            .iter()
            .any(|work_unit| work_unit.id == *work_unit_id)
    });

    if !state.main_view.initialized {
        state.main_view.expanded_workstreams.extend(
            state
                .collaboration
                .workstreams
                .iter()
                .map(|workstream| workstream.id.clone()),
        );
        state.main_view.expanded_work_units.extend(
            state
                .collaboration
                .work_units
                .iter()
                .map(|work_unit| work_unit.id.clone()),
        );
        state.main_view.initialized = true;
    }

    let visible_rows = visible_main_hierarchy_rows(state);
    if visible_rows.is_empty() {
        state.main_view.selected = None;
        state.main_view.scroll_offset = 0;
        return;
    }

    let selection = state
        .main_view
        .selected
        .clone()
        .filter(|selected| visible_rows.contains(selected))
        .or_else(|| preferred_main_selection(state))
        .unwrap_or_else(|| visible_rows[0].clone());
    restore_main_selection(state, selection);
}

fn preferred_main_selection(state: &AppState) -> Option<MainHierarchySelection> {
    selection_from_thread_id(state, state.selected_thread_id.as_deref())
        .or_else(|| selection_from_work_unit_id(state, state.selected_work_unit_id.as_deref()))
        .or_else(|| selection_from_workstream_id(state, state.selected_workstream_id.as_deref()))
        .or_else(|| selection_from_thread_id(state, state.session.active_thread_id.as_deref()))
}

fn selection_from_workstream_id(
    state: &AppState,
    workstream_id: Option<&str>,
) -> Option<MainHierarchySelection> {
    let workstream_id = workstream_id?;
    state
        .collaboration
        .workstreams
        .iter()
        .any(|workstream| workstream.id == workstream_id)
        .then(|| MainHierarchySelection::Workstream {
            workstream_id: workstream_id.to_string(),
        })
}

fn selection_from_work_unit_id(
    state: &AppState,
    work_unit_id: Option<&str>,
) -> Option<MainHierarchySelection> {
    let work_unit = state
        .collaboration
        .work_units
        .iter()
        .find(|work_unit| Some(work_unit.id.as_str()) == work_unit_id)?;
    Some(MainHierarchySelection::WorkUnit {
        workstream_id: work_unit.workstream_id.clone(),
        work_unit_id: work_unit.id.clone(),
    })
}

fn selection_from_thread_id(
    state: &AppState,
    thread_id: Option<&str>,
) -> Option<MainHierarchySelection> {
    let thread_id = thread_id?;
    let assignment = state
        .collaboration
        .codex_thread_assignments
        .iter()
        .find(|assignment| assignment.codex_thread_id == thread_id)?;
    Some(MainHierarchySelection::Thread {
        workstream_id: assignment.workstream_id.clone(),
        work_unit_id: assignment.work_unit_id.clone(),
        thread_id: assignment.codex_thread_id.clone(),
    })
}

fn visible_main_hierarchy_rows(state: &AppState) -> Vec<MainHierarchySelection> {
    let mut rows = Vec::new();
    for workstream in &state.collaboration.workstreams {
        rows.push(MainHierarchySelection::Workstream {
            workstream_id: workstream.id.clone(),
        });
        if !state
            .main_view
            .expanded_workstreams
            .contains(workstream.id.as_str())
        {
            continue;
        }

        for work_unit in state
            .collaboration
            .work_units
            .iter()
            .filter(|work_unit| work_unit.workstream_id == workstream.id)
        {
            rows.push(MainHierarchySelection::WorkUnit {
                workstream_id: workstream.id.clone(),
                work_unit_id: work_unit.id.clone(),
            });
            if !state
                .main_view
                .expanded_work_units
                .contains(work_unit.id.as_str())
            {
                continue;
            }

            for thread_id in thread_ids_for_work_unit(state, &work_unit.id) {
                rows.push(MainHierarchySelection::Thread {
                    workstream_id: workstream.id.clone(),
                    work_unit_id: work_unit.id.clone(),
                    thread_id,
                });
            }
        }
    }
    rows
}

fn thread_ids_for_work_unit(state: &AppState, work_unit_id: &str) -> Vec<String> {
    let mut thread_ids = state
        .collaboration
        .codex_thread_assignments
        .iter()
        .filter(|assignment| assignment.work_unit_id == work_unit_id)
        .map(|assignment| assignment.codex_thread_id.clone())
        .collect::<Vec<_>>();
    thread_ids.sort_by(|left, right| {
        thread_updated_at(state, right)
            .cmp(&thread_updated_at(state, left))
            .then_with(|| left.cmp(right))
    });
    thread_ids.dedup();
    thread_ids
}

fn thread_updated_at(state: &AppState, thread_id: &str) -> i64 {
    state
        .threads
        .iter()
        .find(|thread| thread.id == thread_id)
        .map(|thread| thread.updated_at)
        .unwrap_or_default()
}

fn select_relative_main_hierarchy(state: &mut AppState, delta: isize) -> Vec<Effect> {
    let visible_rows = visible_main_hierarchy_rows(state);
    if visible_rows.is_empty() {
        state.main_view.selected = None;
        state.main_view.scroll_offset = 0;
        return Vec::new();
    }

    let current_index = state
        .main_view
        .selected
        .as_ref()
        .and_then(|selected| visible_rows.iter().position(|row| row == selected))
        .unwrap_or(0);
    let next_index = if delta.is_negative() {
        current_index.saturating_sub(delta.unsigned_abs())
    } else {
        (current_index + delta as usize).min(visible_rows.len().saturating_sub(1))
    };
    set_main_selection(state, visible_rows[next_index].clone())
}

fn expand_selected_main_hierarchy(state: &mut AppState) -> Vec<Effect> {
    let Some(selected) = state.main_view.selected.clone() else {
        return Vec::new();
    };
    match selected {
        MainHierarchySelection::Workstream { workstream_id } => {
            state
                .main_view
                .expanded_workstreams
                .insert(workstream_id.clone());
            set_main_selection(state, MainHierarchySelection::Workstream { workstream_id })
        }
        MainHierarchySelection::WorkUnit {
            workstream_id,
            work_unit_id,
        } => {
            state
                .main_view
                .expanded_workstreams
                .insert(workstream_id.clone());
            state
                .main_view
                .expanded_work_units
                .insert(work_unit_id.clone());
            set_main_selection(
                state,
                MainHierarchySelection::WorkUnit {
                    workstream_id,
                    work_unit_id,
                },
            )
        }
        MainHierarchySelection::Thread { .. } => Vec::new(),
    }
}

fn collapse_selected_main_hierarchy(state: &mut AppState) -> Vec<Effect> {
    let Some(selected) = state.main_view.selected.clone() else {
        return Vec::new();
    };
    match selected {
        MainHierarchySelection::Workstream { workstream_id } => {
            state
                .main_view
                .expanded_workstreams
                .remove(workstream_id.as_str());
            set_main_selection(state, MainHierarchySelection::Workstream { workstream_id })
        }
        MainHierarchySelection::WorkUnit {
            workstream_id,
            work_unit_id,
        } => {
            if state
                .main_view
                .expanded_work_units
                .remove(work_unit_id.as_str())
            {
                set_main_selection(
                    state,
                    MainHierarchySelection::WorkUnit {
                        workstream_id,
                        work_unit_id,
                    },
                )
            } else {
                set_main_selection(state, MainHierarchySelection::Workstream { workstream_id })
            }
        }
        MainHierarchySelection::Thread {
            workstream_id,
            work_unit_id,
            ..
        } => set_main_selection(
            state,
            MainHierarchySelection::WorkUnit {
                workstream_id,
                work_unit_id,
            },
        ),
    }
}

fn set_main_selection(state: &mut AppState, selection: MainHierarchySelection) -> Vec<Effect> {
    apply_main_selection(state, selection, true, true)
}

fn restore_main_selection(state: &mut AppState, selection: MainHierarchySelection) {
    let _ = apply_main_selection(state, selection, false, false);
}

fn apply_main_selection(
    state: &mut AppState,
    selection: MainHierarchySelection,
    sync_legacy: bool,
    load_effects: bool,
) -> Vec<Effect> {
    expand_main_selection_ancestors(state, &selection);
    state.main_view.selected = Some(selection.clone());
    if sync_legacy {
        sync_legacy_selection_from_main(state, &selection);
    }
    let visible_rows = visible_main_hierarchy_rows(state);
    if let Some(selected_index) = visible_rows.iter().position(|row| row == &selection) {
        adjust_main_scroll(state, selected_index, visible_rows.len());
    } else {
        state.main_view.scroll_offset = 0;
    }

    if !load_effects {
        return Vec::new();
    }

    match selection {
        MainHierarchySelection::Thread { thread_id, .. } => select_thread(state, thread_id),
        MainHierarchySelection::WorkUnit { .. } => load_selected_work_unit_detail_if_needed(state),
        MainHierarchySelection::Workstream { .. } => {
            load_selected_work_unit_detail_if_needed(state)
        }
    }
}

fn expand_main_selection_ancestors(state: &mut AppState, selection: &MainHierarchySelection) {
    match selection {
        MainHierarchySelection::Workstream { .. } => {}
        MainHierarchySelection::WorkUnit { workstream_id, .. } => {
            state
                .main_view
                .expanded_workstreams
                .insert(workstream_id.clone());
        }
        MainHierarchySelection::Thread {
            workstream_id,
            work_unit_id,
            ..
        } => {
            state
                .main_view
                .expanded_workstreams
                .insert(workstream_id.clone());
            state
                .main_view
                .expanded_work_units
                .insert(work_unit_id.clone());
        }
    }
}

fn sync_legacy_selection_from_main(state: &mut AppState, selection: &MainHierarchySelection) {
    match selection {
        MainHierarchySelection::Workstream { workstream_id } => {
            state.selected_workstream_id = Some(workstream_id.clone());
            let selected_belongs_to_workstream =
                state
                    .selected_work_unit_id
                    .as_ref()
                    .is_some_and(|selected_work_unit_id| {
                        state.collaboration.work_units.iter().any(|work_unit| {
                            work_unit.id == *selected_work_unit_id
                                && work_unit.workstream_id == *workstream_id
                        })
                    });
            if !selected_belongs_to_workstream {
                state.selected_work_unit_id = state
                    .collaboration
                    .work_units
                    .iter()
                    .find(|work_unit| work_unit.workstream_id == *workstream_id)
                    .map(|work_unit| work_unit.id.clone());
            }
        }
        MainHierarchySelection::WorkUnit {
            workstream_id,
            work_unit_id,
        } => {
            state.selected_workstream_id = Some(workstream_id.clone());
            state.selected_work_unit_id = Some(work_unit_id.clone());
            state.selected_thread_id = thread_ids_for_work_unit(state, work_unit_id)
                .into_iter()
                .next()
                .or_else(|| state.selected_thread_id.clone());
        }
        MainHierarchySelection::Thread {
            workstream_id,
            work_unit_id,
            thread_id,
        } => {
            state.selected_workstream_id = Some(workstream_id.clone());
            state.selected_work_unit_id = Some(work_unit_id.clone());
            state.selected_thread_id = Some(thread_id.clone());
        }
    }
}

fn adjust_main_scroll(state: &mut AppState, selected_index: usize, row_count: usize) {
    if selected_index < state.main_view.scroll_offset {
        state.main_view.scroll_offset = selected_index;
    } else {
        let visible_end = state.main_view.scroll_offset + MAIN_HIERARCHY_SCROLL_WINDOW;
        if selected_index >= visible_end {
            state.main_view.scroll_offset = selected_index + 1 - MAIN_HIERARCHY_SCROLL_WINDOW;
        }
    }
    let max_offset = row_count.saturating_sub(MAIN_HIERARCHY_SCROLL_WINDOW);
    state.main_view.scroll_offset = state.main_view.scroll_offset.min(max_offset);
}

fn reconcile_thread_selection(state: &mut AppState, preferred_thread_id: Option<&str>) {
    if state.threads.is_empty() {
        state.selected_thread_id = None;
        return;
    }

    let selected_thread_exists = state
        .selected_thread_id
        .as_ref()
        .is_some_and(|thread_id| state.threads.iter().any(|thread| thread.id == *thread_id));
    if selected_thread_exists {
        return;
    }

    state.selected_thread_id = preferred_thread_id
        .and_then(|thread_id| {
            state
                .threads
                .iter()
                .find(|thread| thread.id == thread_id)
                .map(|thread| thread.id.clone())
        })
        .or_else(|| state.threads.first().map(|thread| thread.id.clone()));
}

fn select_relative_workstream(state: &mut AppState, delta: isize) -> Vec<Effect> {
    if state.collaboration.workstreams.is_empty() {
        state.selected_workstream_id = None;
        state.selected_work_unit_id = None;
        return Vec::new();
    }
    let current_index = state
        .selected_workstream_id
        .as_ref()
        .and_then(|workstream_id| {
            state
                .collaboration
                .workstreams
                .iter()
                .position(|workstream| workstream.id == *workstream_id)
        })
        .unwrap_or(0);
    let next_index = if delta.is_negative() {
        current_index.saturating_sub(delta.unsigned_abs())
    } else {
        (current_index + delta as usize)
            .min(state.collaboration.workstreams.len().saturating_sub(1))
    };
    state.selected_workstream_id = Some(state.collaboration.workstreams[next_index].id.clone());
    state.selected_work_unit_id =
        first_work_unit_for_selected_workstream(state).map(|work_unit| work_unit.id.clone());
    load_selected_work_unit_detail_if_needed(state)
}

fn select_relative_work_unit(state: &mut AppState, delta: isize) -> Vec<Effect> {
    let work_units = work_units_for_selected_workstream(state);
    if work_units.is_empty() {
        state.selected_work_unit_id = None;
        return Vec::new();
    }
    let current_index = state
        .selected_work_unit_id
        .as_ref()
        .and_then(|work_unit_id| {
            work_units
                .iter()
                .position(|work_unit| work_unit.id == *work_unit_id)
        })
        .unwrap_or(0);
    let next_index = if delta.is_negative() {
        current_index.saturating_sub(delta.unsigned_abs())
    } else {
        (current_index + delta as usize).min(work_units.len().saturating_sub(1))
    };
    state.selected_work_unit_id = Some(work_units[next_index].id.clone());
    load_selected_work_unit_detail_if_needed(state)
}

fn reconcile_collaboration_selection(state: &mut AppState) {
    if state.collaboration.workstreams.is_empty() {
        state.selected_workstream_id = None;
        state.selected_work_unit_id = None;
        return;
    }

    let selected_workstream_exists =
        state
            .selected_workstream_id
            .as_ref()
            .is_some_and(|workstream_id| {
                state
                    .collaboration
                    .workstreams
                    .iter()
                    .any(|workstream| workstream.id == *workstream_id)
            });
    if !selected_workstream_exists {
        state.selected_workstream_id = state
            .collaboration
            .workstreams
            .first()
            .map(|workstream| workstream.id.clone());
    }

    let selected_work_units = work_units_for_selected_workstream(state);
    if selected_work_units.is_empty() {
        state.selected_work_unit_id = None;
        return;
    }

    let selected_work_unit_exists =
        state
            .selected_work_unit_id
            .as_ref()
            .is_some_and(|work_unit_id| {
                selected_work_units
                    .iter()
                    .any(|work_unit| work_unit.id == *work_unit_id)
            });
    if !selected_work_unit_exists {
        state.selected_work_unit_id = selected_work_units
            .first()
            .map(|work_unit| work_unit.id.clone());
    }
}

fn first_work_unit_for_selected_workstream(state: &AppState) -> Option<&ipc::WorkUnitSummary> {
    work_units_for_selected_workstream(state).into_iter().next()
}

fn work_units_for_selected_workstream(state: &AppState) -> Vec<&ipc::WorkUnitSummary> {
    let Some(workstream_id) = state.selected_workstream_id.as_ref() else {
        return Vec::new();
    };
    state
        .collaboration
        .work_units
        .iter()
        .filter(|work_unit| work_unit.workstream_id == *workstream_id)
        .collect()
}

fn load_selected_work_unit_detail_if_needed(state: &AppState) -> Vec<Effect> {
    let Some(work_unit_id) = state.selected_work_unit_id.clone() else {
        return Vec::new();
    };
    if state.work_unit_details.contains_key(&work_unit_id) {
        Vec::new()
    } else {
        vec![Effect::LoadWorkUnitDetail { work_unit_id }]
    }
}

fn load_selected_work_unit_detail(state: &AppState) -> Vec<Effect> {
    state
        .selected_work_unit_id
        .clone()
        .map(|work_unit_id| Effect::LoadWorkUnitDetail { work_unit_id })
        .into_iter()
        .collect()
}

fn ensure_thread_detail(state: &mut AppState, thread_id: &str) {
    if state.thread_details.contains_key(thread_id) {
        return;
    }
    let summary = state
        .threads
        .iter()
        .find(|thread| thread.id == thread_id)
        .cloned()
        .unwrap_or_else(|| ipc::ThreadSummary {
            id: thread_id.to_string(),
            preview: String::new(),
            name: None,
            model_provider: String::new(),
            cwd: String::new(),
            status: "pending".to_string(),
            created_at: 0,
            updated_at: 0,
            scope: "live_observed".to_string(),
            archived: false,
            loaded_status: ipc::ThreadLoadedStatus::Unknown,
            active_flags: Vec::new(),
            active_turn_id: None,
            last_seen_turn_id: None,
            recent_output: None,
            recent_event: None,
            turn_in_flight: false,
            monitor_state: ipc::ThreadMonitorState::Detached,
            last_sync_at: chrono::Utc::now(),
            source_kind: None,
            raw_summary: None,
        });
    state.thread_details.insert(
        thread_id.to_string(),
        ipc::ThreadView {
            summary,
            history_loaded: false,
            turns: Vec::new(),
        },
    );
}

fn upsert_turn_state(state: &mut AppState, turn: ipc::TurnStateView) {
    state
        .turn_states
        .insert(turn_state_key(&turn.thread_id, &turn.turn_id), turn);
}

fn turn_state_key(thread_id: &str, turn_id: &str) -> String {
    format!("{thread_id}/{turn_id}")
}

fn refresh_prompt_in_flight(state: &mut AppState) {
    let explicit_active = state
        .turn_states
        .values()
        .any(|turn| turn.attachable && matches!(turn.lifecycle, ipc::TurnLifecycleState::Active));
    state.prompt_in_flight = if explicit_active {
        true
    } else {
        !state.session.active_turns.is_empty()
    };
}

fn steer_compose_insert(compose: &mut SteerComposeState, ch: char) {
    compose.buffer.insert(compose.cursor, ch);
    compose.cursor += ch.len_utf8();
    compose.preferred_column = compose_column_at(&compose.buffer, compose.cursor);
}

fn steer_compose_backspace(compose: &mut SteerComposeState) {
    if compose.cursor == 0 {
        return;
    }
    let previous = previous_char_boundary(&compose.buffer, compose.cursor);
    compose.buffer.drain(previous..compose.cursor);
    compose.cursor = previous;
    compose.preferred_column = compose_column_at(&compose.buffer, compose.cursor);
}

fn steer_compose_delete(compose: &mut SteerComposeState) {
    if compose.cursor >= compose.buffer.len() {
        return;
    }
    let next = next_char_boundary(&compose.buffer, compose.cursor);
    compose.buffer.drain(compose.cursor..next);
    compose.preferred_column = compose_column_at(&compose.buffer, compose.cursor);
}

fn steer_compose_move_left(compose: &mut SteerComposeState) {
    if compose.cursor == 0 {
        return;
    }
    compose.cursor = previous_char_boundary(&compose.buffer, compose.cursor);
    compose.preferred_column = compose_column_at(&compose.buffer, compose.cursor);
}

fn steer_compose_move_right(compose: &mut SteerComposeState) {
    if compose.cursor >= compose.buffer.len() {
        return;
    }
    compose.cursor = next_char_boundary(&compose.buffer, compose.cursor);
    compose.preferred_column = compose_column_at(&compose.buffer, compose.cursor);
}

fn steer_compose_move_vertical(compose: &mut SteerComposeState, delta: isize) {
    let lines = compose.buffer.split('\n').collect::<Vec<_>>();
    let (line_index, _, _) = compose_line_column_position(&compose.buffer, compose.cursor);
    let target_line_index = if delta.is_negative() {
        line_index.saturating_sub(delta.unsigned_abs())
    } else {
        (line_index + delta as usize).min(lines.len().saturating_sub(1))
    };
    if target_line_index == line_index {
        return;
    }
    let target_column = compose
        .preferred_column
        .min(lines[target_line_index].chars().count());
    compose.cursor = cursor_for_line_column(&compose.buffer, target_line_index, target_column);
}

fn previous_char_boundary(buffer: &str, cursor: usize) -> usize {
    buffer[..cursor]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_char_boundary(buffer: &str, cursor: usize) -> usize {
    let slice = &buffer[cursor..];
    slice
        .chars()
        .next()
        .map(|ch| cursor + ch.len_utf8())
        .unwrap_or(cursor)
}

fn compose_column_at(buffer: &str, cursor: usize) -> usize {
    let (_, column, _) = compose_line_column_position(buffer, cursor);
    column
}

fn compose_line_column_position(buffer: &str, cursor: usize) -> (usize, usize, usize) {
    let mut line_index = 0usize;
    let mut column = 0usize;
    let mut line_start = 0usize;
    for (index, ch) in buffer.char_indices() {
        if index >= cursor {
            break;
        }
        if ch == '\n' {
            line_index += 1;
            column = 0;
            line_start = index + ch.len_utf8();
        } else {
            column += 1;
        }
    }
    (line_index, column, line_start)
}

fn cursor_for_line_column(buffer: &str, target_line_index: usize, target_column: usize) -> usize {
    let mut line_index = 0usize;
    let mut line_start = 0usize;
    for (index, ch) in buffer.char_indices() {
        if line_index == target_line_index {
            break;
        }
        if ch == '\n' {
            line_index += 1;
            line_start = index + ch.len_utf8();
        }
    }
    if line_index != target_line_index {
        return buffer.len();
    }
    let line = buffer[line_start..].split('\n').next().unwrap_or_default();
    line.char_indices()
        .nth(target_column)
        .map(|(offset, _)| line_start + offset)
        .unwrap_or(line_start + line.len())
}

fn is_daemon_action_in_flight(state: &AppState) -> bool {
    matches!(
        state.daemon_lifecycle,
        DaemonLifecycleState::Starting
            | DaemonLifecycleState::Stopping
            | DaemonLifecycleState::Restarting
    )
}

fn daemon_is_connected(state: &AppState) -> bool {
    state
        .daemon
        .as_ref()
        .is_some_and(|daemon| daemon.upstream.status == "connected")
}

fn lifecycle_from_upstream_status(status: &str) -> DaemonLifecycleState {
    match status {
        "connected" => DaemonLifecycleState::Running,
        "disconnected" | "stopped" => DaemonLifecycleState::Stopped,
        _ => DaemonLifecycleState::Unknown,
    }
}

fn ensure_turn<'a>(thread: &'a mut ipc::ThreadView, turn_id: &str) -> &'a mut ipc::TurnView {
    if let Some(index) = thread.turns.iter().position(|turn| turn.id == turn_id) {
        return &mut thread.turns[index];
    }
    thread.turns.push(ipc::TurnView {
        id: turn_id.to_string(),
        status: "in_progress".to_string(),
        error_message: None,
        error_summary: None,
        started_at: None,
        completed_at: None,
        latest_diff: None,
        latest_plan_snapshot: None,
        token_usage_snapshot: None,
        items: Vec::new(),
    });
    let index = thread.turns.len() - 1;
    &mut thread.turns[index]
}

fn ensure_item<'a>(turn: &'a mut ipc::TurnView, item_id: &str) -> &'a mut ipc::ItemView {
    if let Some(index) = turn.items.iter().position(|item| item.id == item_id) {
        return &mut turn.items[index];
    }
    turn.items.push(ipc::ItemView {
        id: item_id.to_string(),
        item_type: "agent_message".to_string(),
        status: None,
        text: None,
        summary: None,
        payload: None,
    });
    let index = turn.items.len() - 1;
    &mut turn.items[index]
}

fn upsert_turn(thread: &mut ipc::ThreadView, turn: ipc::TurnView) {
    if let Some(existing) = thread
        .turns
        .iter_mut()
        .find(|existing| existing.id == turn.id)
    {
        existing.status = turn.status;
        existing.error_message = turn.error_message;
        if turn.error_summary.is_some() {
            existing.error_summary = turn.error_summary;
        }
        if turn.started_at.is_some() {
            existing.started_at = turn.started_at;
        }
        if turn.completed_at.is_some() {
            existing.completed_at = turn.completed_at;
        }
        if turn.latest_diff.is_some() {
            existing.latest_diff = turn.latest_diff;
        }
        if turn.latest_plan_snapshot.is_some() {
            existing.latest_plan_snapshot = turn.latest_plan_snapshot;
        }
        if turn.token_usage_snapshot.is_some() {
            existing.token_usage_snapshot = turn.token_usage_snapshot;
        }
        for item in turn.items {
            upsert_item(existing, item);
        }
        return;
    }
    thread.turns.push(turn);
}

fn upsert_item(turn: &mut ipc::TurnView, item: ipc::ItemView) {
    if let Some(existing) = turn
        .items
        .iter_mut()
        .find(|existing| existing.id == item.id)
    {
        existing.item_type = item.item_type;
        if item.status.is_some() {
            existing.status = item.status;
        }
        if item.text.is_some() {
            existing.text = item.text;
        }
        if item.summary.is_some() {
            existing.summary = item.summary;
        }
        if item.payload.is_some() {
            existing.payload = item.payload;
        }
        return;
    }
    turn.items.push(item);
}

fn upsert_thread_summary(threads: &mut Vec<ipc::ThreadSummary>, summary: ipc::ThreadSummary) {
    if let Some(existing) = threads.iter_mut().find(|thread| thread.id == summary.id) {
        *existing = summary;
    } else {
        threads.push(summary);
    }
    threads.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn upsert_workstream_summary(
    workstreams: &mut Vec<ipc::WorkstreamSummary>,
    summary: ipc::WorkstreamSummary,
) {
    if let Some(existing) = workstreams
        .iter_mut()
        .find(|workstream| workstream.id == summary.id)
    {
        *existing = summary;
    } else {
        workstreams.push(summary);
    }
    workstreams.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn upsert_work_unit_summary(
    work_units: &mut Vec<ipc::WorkUnitSummary>,
    summary: ipc::WorkUnitSummary,
) {
    if let Some(existing) = work_units
        .iter_mut()
        .find(|work_unit| work_unit.id == summary.id)
    {
        *existing = summary;
    } else {
        work_units.push(summary);
    }
    work_units.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn upsert_assignment_summary(
    assignments: &mut Vec<ipc::AssignmentSummary>,
    summary: ipc::AssignmentSummary,
) {
    if let Some(existing) = assignments
        .iter_mut()
        .find(|assignment| assignment.id == summary.id)
    {
        *existing = summary;
    } else {
        assignments.push(summary);
    }
    assignments.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn upsert_codex_assignment_summary(
    assignments: &mut Vec<ipc::CodexThreadAssignmentSummary>,
    summary: ipc::CodexThreadAssignmentSummary,
) {
    if let Some(existing) = assignments
        .iter_mut()
        .find(|assignment| assignment.assignment_id == summary.assignment_id)
    {
        *existing = summary;
    } else {
        assignments.push(summary);
    }
    assignments.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.assignment_id.cmp(&right.assignment_id))
    });
}

fn upsert_supervisor_turn_decision_summary(
    decisions: &mut Vec<ipc::SupervisorTurnDecisionSummary>,
    summary: ipc::SupervisorTurnDecisionSummary,
) {
    if let Some(existing) = decisions
        .iter_mut()
        .find(|decision| decision.decision_id == summary.decision_id)
    {
        *existing = summary;
    } else {
        decisions.push(summary);
    }
    decisions.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.decision_id.cmp(&right.decision_id))
    });
}

fn selected_thread_pending_supervisor_decision(
    state: &AppState,
) -> Option<&ipc::SupervisorTurnDecisionSummary> {
    let thread_id = state.selected_thread_id.as_deref()?;
    state
        .collaboration
        .supervisor_turn_decisions
        .iter()
        .filter(|decision| {
            decision.codex_thread_id == thread_id
                && decision.status == orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman
        })
        .max_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.decision_id.cmp(&right.decision_id))
        })
}

fn selected_thread_pending_steer_decision(
    state: &AppState,
) -> Option<&ipc::SupervisorTurnDecisionSummary> {
    selected_thread_pending_supervisor_decision(state)
        .filter(|decision| decision.kind == orcas_core::SupervisorTurnDecisionKind::SteerActiveTurn)
}

fn selected_thread_record_no_action_decision_id(state: &AppState) -> Option<String> {
    selected_thread_pending_supervisor_decision(state)
        .filter(|decision| decision.kind == orcas_core::SupervisorTurnDecisionKind::NextTurn)
        .map(|decision| decision.decision_id.clone())
}

fn selected_thread_manual_refresh_assignment_id(state: &AppState) -> Option<String> {
    let thread_id = state.selected_thread_id.as_deref()?;
    let assignment = state
        .collaboration
        .codex_thread_assignments
        .iter()
        .find(|assignment| {
            assignment.codex_thread_id == thread_id
                && assignment.status == orcas_core::CodexThreadAssignmentStatus::Active
        })?;
    let has_active_turn = state
        .thread_details
        .get(thread_id)
        .map(|thread| thread.summary.active_turn_id.is_some())
        .or_else(|| {
            state
                .threads
                .iter()
                .find(|thread| thread.id == thread_id)
                .map(|thread| thread.active_turn_id.is_some())
        })
        .unwrap_or(false);
    if has_active_turn {
        return None;
    }
    if state
        .collaboration
        .supervisor_turn_decisions
        .iter()
        .any(|decision| decision.assignment_id == assignment.assignment_id && decision.open)
    {
        return None;
    }
    let basis_turn_id = state
        .thread_details
        .get(thread_id)
        .map(|thread| thread.summary.last_seen_turn_id.clone())
        .or_else(|| {
            state
                .threads
                .iter()
                .find(|thread| thread.id == thread_id)
                .and_then(|thread| thread.last_seen_turn_id.clone())
                .map(Some)
        })
        .unwrap_or(None);
    let latest_basis_decision = state
        .collaboration
        .supervisor_turn_decisions
        .iter()
        .filter(|decision| {
            decision.assignment_id == assignment.assignment_id
                && decision.basis_turn_id == basis_turn_id
        })
        .max_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.decision_id.cmp(&right.decision_id))
        })?;
    if latest_basis_decision.kind != orcas_core::SupervisorTurnDecisionKind::NoAction
        || latest_basis_decision.status != orcas_core::SupervisorTurnDecisionStatus::Recorded
    {
        return None;
    }
    Some(assignment.assignment_id.clone())
}

fn selected_thread_interrupt_assignment_id(state: &AppState) -> Option<String> {
    let thread_id = state.selected_thread_id.as_deref()?;
    let assignment = state
        .collaboration
        .codex_thread_assignments
        .iter()
        .find(|assignment| {
            assignment.codex_thread_id == thread_id
                && assignment.status == orcas_core::CodexThreadAssignmentStatus::Active
        })?;
    let active_turn_id = state
        .thread_details
        .get(thread_id)
        .and_then(|thread| thread.summary.active_turn_id.clone())
        .or_else(|| {
            state
                .threads
                .iter()
                .find(|thread| thread.id == thread_id)
                .and_then(|thread| thread.active_turn_id.clone())
        });
    if active_turn_id.is_none() {
        return None;
    }
    if state
        .collaboration
        .supervisor_turn_decisions
        .iter()
        .any(|decision| decision.assignment_id == assignment.assignment_id && decision.open)
    {
        return None;
    }
    Some(assignment.assignment_id.clone())
}

fn selected_thread_steer_assignment_id(state: &AppState) -> Option<String> {
    selected_thread_interrupt_assignment_id(state)
}

fn upsert_report_summary(reports: &mut Vec<ipc::ReportSummary>, summary: ipc::ReportSummary) {
    if let Some(existing) = reports.iter_mut().find(|report| report.id == summary.id) {
        *existing = summary;
    } else {
        reports.push(summary);
    }
    reports.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn upsert_decision_summary(
    decisions: &mut Vec<ipc::DecisionSummary>,
    summary: ipc::DecisionSummary,
) {
    if let Some(existing) = decisions
        .iter_mut()
        .find(|decision| decision.id == summary.id)
    {
        *existing = summary;
    } else {
        decisions.push(summary);
    }
    decisions.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn push_event_summary(state: &mut AppState, summary: ipc::EventSummary) {
    if state.recent_events.len() >= MAX_LOG_ENTRIES {
        state.recent_events.pop_front();
    }
    state.recent_events.push_back(summary);
}

fn event_summary_from_ui_event(event: &UiEvent) -> Option<ipc::EventSummary> {
    let timestamp = chrono::Utc::now();
    let (kind, message, thread_id, turn_id) = match event {
        UiEvent::SnapshotLoaded(_) => return None,
        UiEvent::WorkUnitDetailLoaded(_) => return None,
        UiEvent::Ignored => return None,
        UiEvent::CodexSessionsChanged { .. } => return None,
        UiEvent::ReconnectScheduled { attempt, .. } => (
            "reconnect",
            format!("scheduled reconnect attempt {attempt}"),
            None,
            None,
        ),
        UiEvent::ConnectionLost(message) => ("disconnect", message.clone(), None, None),
        UiEvent::ThreadLoaded(thread) => (
            "thread",
            format!("loaded thread {}", thread.summary.id),
            Some(thread.summary.id.clone()),
            None,
        ),
        UiEvent::ThreadAttached(response) => (
            "thread_attach",
            if response.attached {
                "thread live attach confirmed".to_string()
            } else {
                response
                    .reason
                    .clone()
                    .unwrap_or_else(|| "thread live attach unavailable".to_string())
            },
            response
                .thread
                .as_ref()
                .map(|thread| thread.summary.id.clone()),
            None,
        ),
        UiEvent::ActiveTurnsLoaded(turns) => (
            "turns",
            format!("loaded {} active turns", turns.len()),
            turns.first().map(|turn| turn.thread_id.clone()),
            turns.first().map(|turn| turn.turn_id.clone()),
        ),
        UiEvent::TurnStateLoaded(response) => (
            "turn_state",
            if response.attached {
                "turn attachment confirmed".to_string()
            } else {
                response
                    .reason
                    .clone()
                    .unwrap_or_else(|| "turn state refreshed".to_string())
            },
            response.turn.as_ref().map(|turn| turn.thread_id.clone()),
            response.turn.as_ref().map(|turn| turn.turn_id.clone()),
        ),
        UiEvent::PromptStarted { thread_id, turn_id } => (
            "prompt",
            format!("submitted turn {turn_id}"),
            Some(thread_id.clone()),
            Some(turn_id.clone()),
        ),
        UiEvent::SteerComposeCommitted { .. } => return None,
        UiEvent::ModelsLoaded(_) => return None,
        UiEvent::DaemonStarted { .. } => return None,
        UiEvent::DaemonStopped { .. } => return None,
        UiEvent::DaemonStartFailed(_) | UiEvent::DaemonStopFailed(_) => (
            "daemon",
            "daemon lifecycle command failed".to_string(),
            None,
            None,
        ),
        UiEvent::UpstreamChanged(upstream) => (
            "upstream",
            format!("upstream {}", upstream.status),
            None,
            None,
        ),
        UiEvent::SessionChanged(session) => (
            "session",
            format!("active turns {}", session.active_turns.len()),
            session.active_thread_id.clone(),
            None,
        ),
        UiEvent::ThreadUpdated(thread) => (
            "thread",
            format!("thread {} {}", thread.id, thread.status),
            Some(thread.id.clone()),
            None,
        ),
        UiEvent::WorkstreamLifecycle { action, workstream } => (
            "workstream",
            format!(
                "workstream {} {}",
                workstream.id,
                collaboration_action_label(*action)
            ),
            None,
            None,
        ),
        UiEvent::WorkUnitLifecycle { action, work_unit } => (
            "work_unit",
            format!(
                "work unit {} {}",
                work_unit.id,
                collaboration_action_label(*action)
            ),
            None,
            None,
        ),
        UiEvent::AssignmentLifecycle { action, assignment } => (
            "assignment",
            format!(
                "assignment {} {}",
                assignment.id,
                assignment_action_label(*action)
            ),
            None,
            None,
        ),
        UiEvent::CodexAssignmentLifecycle { action, assignment } => (
            "codex_assignment",
            format!(
                "assignment {} {}",
                assignment.assignment_id,
                codex_assignment_action_label(*action)
            ),
            Some(assignment.codex_thread_id.clone()),
            assignment.latest_basis_turn_id.clone(),
        ),
        UiEvent::SupervisorDecisionLifecycle { action, decision } => (
            "supervisor_decision",
            format!(
                "decision {} {}",
                decision.decision_id,
                supervisor_decision_action_label(*action)
            ),
            Some(decision.codex_thread_id.clone()),
            decision.basis_turn_id.clone(),
        ),
        UiEvent::ReportRecorded(report) => (
            "report",
            format!("report {} {:?}", report.id, report.parse_result),
            None,
            None,
        ),
        UiEvent::DecisionApplied(decision) => (
            "decision",
            format!("decision {} {:?}", decision.id, decision.decision_type),
            None,
            None,
        ),
        UiEvent::ProposalLifecycle {
            action,
            proposal,
            work_unit,
        } => (
            "proposal",
            format!(
                "proposal {} {} for {}",
                proposal.id,
                proposal_action_label(*action),
                work_unit.id
            ),
            None,
            None,
        ),
        UiEvent::TurnUpdated { thread_id, turn } => (
            "turn",
            format!("turn {} {}", turn.id, turn.status),
            Some(thread_id.clone()),
            Some(turn.id.clone()),
        ),
        UiEvent::ItemUpdated {
            thread_id,
            turn_id,
            item,
        } => (
            "item",
            format!(
                "item {} {}",
                item.id,
                item.status.clone().unwrap_or_else(|| "updated".to_string())
            ),
            Some(thread_id.clone()),
            Some(turn_id.clone()),
        ),
        UiEvent::OutputDelta { .. } => return None,
        UiEvent::Warning(message) => ("warning", message.clone(), None, None),
        UiEvent::Error(message) => ("error", message.clone(), None, None),
    };
    Some(ipc::EventSummary {
        timestamp,
        kind: kind.to_string(),
        message,
        thread_id,
        turn_id,
    })
}

fn collaboration_action_label(action: ipc::CollaborationLifecycleAction) -> &'static str {
    match action {
        ipc::CollaborationLifecycleAction::Created => "created",
        ipc::CollaborationLifecycleAction::Updated => "updated",
        ipc::CollaborationLifecycleAction::Completed => "completed",
        ipc::CollaborationLifecycleAction::Escalated => "escalated",
    }
}

fn assignment_action_label(action: ipc::AssignmentLifecycleAction) -> &'static str {
    match action {
        ipc::AssignmentLifecycleAction::Created => "created",
        ipc::AssignmentLifecycleAction::Started => "started",
        ipc::AssignmentLifecycleAction::Reported => "reported",
        ipc::AssignmentLifecycleAction::Closed => "closed",
        ipc::AssignmentLifecycleAction::Interrupted => "interrupted",
        ipc::AssignmentLifecycleAction::Failed => "failed",
    }
}

fn codex_assignment_action_label(action: ipc::CodexAssignmentLifecycleAction) -> &'static str {
    match action {
        ipc::CodexAssignmentLifecycleAction::Created => "created",
        ipc::CodexAssignmentLifecycleAction::Paused => "paused",
        ipc::CodexAssignmentLifecycleAction::Resumed => "resumed",
        ipc::CodexAssignmentLifecycleAction::Released => "released",
        ipc::CodexAssignmentLifecycleAction::Updated => "updated",
    }
}

fn supervisor_decision_action_label(
    action: ipc::SupervisorDecisionLifecycleAction,
) -> &'static str {
    match action {
        ipc::SupervisorDecisionLifecycleAction::Created => "created",
        ipc::SupervisorDecisionLifecycleAction::Approved => "approved",
        ipc::SupervisorDecisionLifecycleAction::Sent => "sent",
        ipc::SupervisorDecisionLifecycleAction::Rejected => "rejected",
        ipc::SupervisorDecisionLifecycleAction::Superseded => "superseded",
        ipc::SupervisorDecisionLifecycleAction::Stale => "stale",
    }
}

fn proposal_action_label(action: ipc::ProposalLifecycleAction) -> &'static str {
    match action {
        ipc::ProposalLifecycleAction::Created => "created",
        ipc::ProposalLifecycleAction::GenerationFailed => "generation_failed",
        ipc::ProposalLifecycleAction::Approved => "approved",
        ipc::ProposalLifecycleAction::Rejected => "rejected",
        ipc::ProposalLifecycleAction::Superseded => "superseded",
        ipc::ProposalLifecycleAction::Stale => "stale",
    }
}
