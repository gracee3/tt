//! Reducer-driven TUI state and state transitions.
//!
//! This module owns the TUI's in-memory state tree and the rules for how it is
//! updated. Different slices of `AppState` have different authority levels:
//! collaboration snapshot state comes from `state/get`, authority hierarchy and
//! detail caches come from authority RPCs, footer/editor state is transient UI
//! state, and PTY/session references are TUI-local. Reconnect and delete
//! boundaries are handled as invalidation plus reload, not replay-driven
//! convergence.

use std::collections::{BTreeSet, HashMap, VecDeque};

use crate::codex::CodexThreadSessions;
use orcas_core::{ConnectionState, WorkUnitStatus, WorkstreamStatus, authority, ipc};

const MAX_LOG_ENTRIES: usize = 64;
const MAIN_HIERARCHY_SCROLL_WINDOW: usize = 12;
const REVIEW_QUEUE_SCROLL_WINDOW: usize = 12;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewSelection {
    Proposal {
        work_unit_id: String,
        proposal_id: String,
    },
    Decision {
        decision_id: String,
    },
    Failure {
        work_unit_id: String,
        proposal_id: String,
    },
    ReviewRequired {
        work_unit_id: String,
        report_id: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
/// Main hierarchy view state.
///
/// `selected` is the visible row, while `pending_selection` preserves user
/// intent across refreshes until the hierarchy is rebuilt and the row is known
/// to exist again.
pub struct MainViewState {
    pub program_view: ProgramView,
    /// Current visible hierarchy row in the main view.
    pub selected: Option<MainHierarchySelection>,
    /// Selection intent preserved across invalidation boundaries.
    pub pending_selection: Option<MainHierarchySelection>,
    pub expanded_workstreams: BTreeSet<String>,
    pub expanded_work_units: BTreeSet<String>,
    pub scroll_offset: usize,
    /// Tracks whether the view has already performed its first auto-expansion.
    pub initialized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FooterFieldState {
    pub value: String,
    pub cursor: usize,
}

impl FooterFieldState {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.len();
        Self { value, cursor }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkstreamFooterForm {
    pub workstream_id: Option<String>,
    pub expected_revision: Option<authority::Revision>,
    pub active_field: usize,
    pub title: FooterFieldState,
    pub root_dir: FooterFieldState,
    pub status: WorkstreamStatus,
    pub priority: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitFooterForm {
    pub workstream_id: String,
    pub work_unit_id: Option<String>,
    pub expected_revision: Option<authority::Revision>,
    pub active_field: usize,
    pub title: FooterFieldState,
    pub task_statement: String,
    pub status: WorkUnitStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedThreadFooterForm {
    pub work_unit_id: String,
    pub tracked_thread_id: Option<String>,
    pub expected_revision: Option<authority::Revision>,
    pub active_field: usize,
    pub title: FooterFieldState,
    pub root_dir: FooterFieldState,
    pub notes: Option<String>,
    pub backend_kind: authority::TrackedThreadBackendKind,
    pub upstream_thread_id: Option<String>,
    pub binding_state: authority::TrackedThreadBindingState,
    pub preferred_model: Option<String>,
    pub last_seen_turn_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteFooterState {
    pub target: authority::DeleteTarget,
    pub label: String,
    pub expected_revision: authority::Revision,
    pub confirmation_token: authority::DeleteToken,
    pub requires_typed_confirmation: bool,
    pub active_field: usize,
    pub typed_confirmation: FooterFieldState,
    pub affected_work_units: u64,
    pub affected_tracked_threads: u64,
    pub has_upstream_bindings: bool,
    pub fallback_selection: Option<MainHierarchySelection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MainFooterState {
    Inspect,
    CreateWorkstream(WorkstreamFooterForm),
    EditWorkstream(WorkstreamFooterForm),
    CreateWorkUnit(WorkUnitFooterForm),
    EditWorkUnit(WorkUnitFooterForm),
    CreateTrackedThread(TrackedThreadFooterForm),
    EditTrackedThread(TrackedThreadFooterForm),
    ConfirmDelete(DeleteFooterState),
}

impl Default for MainFooterState {
    fn default() -> Self {
        Self::Inspect
    }
}

#[derive(Debug, Clone, Default)]
/// Cached authority hierarchy plus detail responses and transient footer state.
///
/// The hierarchy is refreshed separately from the collaboration snapshot, and
/// the detail caches are invalidated on reconnect/delete boundaries rather than
/// being treated as part of the snapshot.
pub struct AuthorityMainState {
    /// Canonical planning hierarchy read from the authority surface.
    pub hierarchy: authority::HierarchySnapshot,
    /// Cached authority workstream detail responses keyed by id.
    pub workstream_details: HashMap<String, ipc::AuthorityWorkstreamGetResponse>,
    /// Cached authority work-unit detail responses keyed by id.
    pub work_unit_details: HashMap<String, ipc::AuthorityWorkunitGetResponse>,
    /// Cached authority tracked-thread detail responses keyed by id.
    pub tracked_thread_details: HashMap<String, ipc::AuthorityTrackedThreadGetResponse>,
    /// Transient editor/delete state for authority-facing mutations.
    pub footer: MainFooterState,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
/// Review-queue selection and artifact detail/export state.
///
/// This state is driven by collaboration/runtime data and the retained
/// supervisor artifact surfaces, not by the canonical authority hierarchy.
pub struct ReviewViewState {
    pub selected: Option<ReviewSelection>,
    pub scroll_offset: usize,
    pub selection_anchor: usize,
    pub artifact_detail: Option<ReviewArtifactDetailState>,
    pub artifact_export: Option<ReviewArtifactExportState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewArtifactDetailState {
    pub proposal_id: String,
    pub scroll_offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewArtifactExportState {
    pub proposal_id: String,
    pub format: ReviewArtifactExportFormat,
    pub destination: FooterFieldState,
    pub auto_destination: String,
    pub destination_is_auto: bool,
    pub in_flight: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReviewArtifactExportFormat {
    Json,
    Markdown,
}

impl ReviewArtifactExportFormat {
    pub fn toggle(self) -> Self {
        match self {
            Self::Json => Self::Markdown,
            Self::Markdown => Self::Json,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Markdown => "md",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Markdown => "md",
        }
    }
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
/// Full TUI state tree.
///
/// `AppState` intentionally mixes several ownership classes: collaboration
/// snapshot data, authority caches, transient UI/editor state, and TUI-local
/// PTY/session state references. That split matters when deciding what to
/// preserve versus invalidate on reconnect or delete.
pub struct AppState {
    /// Latest daemon status response from `state/get`.
    pub daemon: Option<ipc::DaemonStatusResponse>,
    /// Connection phase for the current daemon socket.
    pub daemon_phase: DaemonConnectionPhase,
    /// Lifecycle state for the daemon process as observed by the TUI.
    pub daemon_lifecycle: DaemonLifecycleState,
    pub daemon_lifecycle_error: Option<String>,
    /// How many reconnect attempts have been scheduled since the last good snapshot.
    pub reconnect_attempt: u32,
    /// Collaboration/runtime snapshot from `state/get`.
    pub session: ipc::SessionState,
    /// Collaboration snapshot data from `state/get`. This is not the canonical
    /// planning hierarchy read surface.
    pub collaboration: ipc::CollaborationSnapshot,
    pub proposal_artifact_summary_work_units: HashMap<String, Vec<String>>,
    pub loaded_proposal_artifact_summary_work_units: BTreeSet<String>,
    pub loading_proposal_artifact_summary_work_units: BTreeSet<String>,
    pub proposal_artifact_summary_work_unit_errors: HashMap<String, String>,
    pub proposal_artifact_summaries: HashMap<String, ipc::SupervisorProposalArtifactSummary>,
    pub proposal_artifact_details: HashMap<String, ipc::SupervisorProposalArtifactDetail>,
    pub loading_proposal_artifact_summaries: BTreeSet<String>,
    pub loading_proposal_artifact_details: BTreeSet<String>,
    pub proposal_artifact_summary_errors: HashMap<String, String>,
    pub proposal_artifact_detail_errors: HashMap<String, String>,
    /// Canonical planning hierarchy and detail cache state.
    pub authority_main: AuthorityMainState,
    /// TUI-visible thread summaries and details from the daemon runtime view.
    pub threads: Vec<ipc::ThreadSummary>,
    /// Cached daemon model list for the supervisor view.
    pub daemon_models: Vec<ipc::ModelSummary>,
    pub models_loading: bool,
    /// Cached daemon thread detail views for the currently loaded threads.
    pub thread_details: HashMap<String, ipc::ThreadView>,
    /// Cached turn state by thread/turn id.
    pub turn_states: HashMap<String, ipc::TurnStateView>,
    /// TUI-local PTY-backed Codex session history. These sessions are owned by
    /// the TUI process, not the daemon or the authority model.
    pub codex_sessions: HashMap<String, CodexThreadSessions>,
    /// Current top-level screen.
    pub current_view: TopLevelView,
    /// Main hierarchy substate, including selection intent and cached expansion.
    pub main_view: MainViewState,
    /// Review queue substate.
    pub review_view: ReviewViewState,
    /// Currently selected daemon thread id, if any.
    pub selected_thread_id: Option<String>,
    /// Selected collaboration workstream id for sidebar/context navigation.
    pub selected_workstream_id: Option<String>,
    /// Selected collaboration work-unit id for sidebar/context navigation.
    pub selected_work_unit_id: Option<String>,
    /// Retained runtime-detail work-unit responses from `workunit/get`.
    pub work_unit_details: HashMap<String, ipc::WorkunitGetResponse>,
    pub collaboration_focus: CollaborationFocus,
    pub recent_events: VecDeque<ipc::EventSummary>,
    /// Whether a prompt/turn interaction is currently active in the TUI.
    pub prompt_in_flight: bool,
    /// Transient draft state for steer/review composition.
    pub steer_compose: Option<SteerComposeState>,
    /// Status banner shown in the TUI chrome.
    pub banner: Option<StatusBanner>,
    /// Help overlay toggle.
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
    CreateWorkstream,
    CreateWorkUnitForSelection,
    CreateTrackedThreadForSelection,
    EditSelectedMainEntity,
    DeleteSelectedMainEntity,
    MainFooterAppend(char),
    MainFooterBackspace,
    MainFooterDelete,
    MainFooterMoveLeft,
    MainFooterMoveRight,
    MainFooterNextField,
    MainFooterPreviousField,
    SubmitMainFooter,
    CancelMainFooter,
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
    OpenSelectedProposalArtifactDetail,
    CloseReviewArtifactDetail,
    ScrollReviewArtifactDetail(i16),
    OpenSelectedProposalArtifactExport,
    CloseReviewArtifactExport,
    SubmitReviewArtifactExport,
    ReviewArtifactExportToggleFormat,
    ReviewArtifactExportAppend(char),
    ReviewArtifactExportBackspace,
    ReviewArtifactExportDelete,
    ReviewArtifactExportMoveLeft,
    ReviewArtifactExportMoveRight,
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
    AuthorityHierarchyLoaded(authority::HierarchySnapshot),
    AuthorityWorkstreamDetailLoaded(ipc::AuthorityWorkstreamGetResponse),
    AuthorityWorkUnitDetailLoaded(ipc::AuthorityWorkunitGetResponse),
    AuthorityTrackedThreadDetailLoaded(ipc::AuthorityTrackedThreadGetResponse),
    AuthorityDeletePlanLoaded(authority::DeletePlan),
    AuthorityWorkstreamCreated(authority::WorkstreamRecord),
    AuthorityWorkstreamEdited(authority::WorkstreamRecord),
    AuthorityWorkstreamDeleted(authority::WorkstreamRecord),
    AuthorityWorkUnitCreated(authority::WorkUnitRecord),
    AuthorityWorkUnitEdited(authority::WorkUnitRecord),
    AuthorityWorkUnitDeleted(authority::WorkUnitRecord),
    AuthorityTrackedThreadCreated(authority::TrackedThreadRecord),
    AuthorityTrackedThreadEdited(authority::TrackedThreadRecord),
    AuthorityTrackedThreadDeleted(authority::TrackedThreadRecord),
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
    TrackedThreadLifecycle {
        action: ipc::CollaborationLifecycleAction,
        tracked_thread: authority::TrackedThreadSummary,
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
    ProposalArtifactSummaryListLoaded(ipc::ProposalArtifactSummaryListForWorkunitResponse),
    ProposalArtifactSummaryListLoadFailed {
        work_unit_id: String,
        message: String,
    },
    ProposalArtifactSummaryLoaded(ipc::SupervisorProposalArtifactSummary),
    ProposalArtifactSummaryLoadFailed {
        proposal_id: String,
        message: String,
    },
    ProposalArtifactDetailLoaded(ipc::SupervisorProposalArtifactDetail),
    ProposalArtifactDetailLoadFailed {
        proposal_id: String,
        message: String,
    },
    ProposalArtifactExported {
        proposal_id: String,
        destination: String,
        format: ReviewArtifactExportFormat,
    },
    ProposalArtifactExportFailed {
        proposal_id: String,
        message: String,
        format: ReviewArtifactExportFormat,
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
            ipc::DaemonEvent::TrackedThreadLifecycle {
                action,
                tracked_thread,
            } => Self::TrackedThreadLifecycle {
                action,
                tracked_thread,
            },
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
    LoadAuthorityHierarchy,
    LoadAuthorityWorkstreamDetail {
        workstream_id: String,
    },
    LoadAuthorityWorkUnitDetail {
        work_unit_id: String,
    },
    LoadAuthorityTrackedThreadDetail {
        tracked_thread_id: String,
    },
    LoadAuthorityDeletePlan {
        target: authority::DeleteTarget,
    },
    CreateAuthorityWorkstream {
        command: authority::CreateWorkstream,
    },
    EditAuthorityWorkstream {
        command: authority::EditWorkstream,
    },
    DeleteAuthorityWorkstream {
        command: authority::DeleteWorkstream,
    },
    CreateAuthorityWorkUnit {
        command: authority::CreateWorkUnit,
    },
    EditAuthorityWorkUnit {
        command: authority::EditWorkUnit,
    },
    DeleteAuthorityWorkUnit {
        command: authority::DeleteWorkUnit,
    },
    CreateAuthorityTrackedThread {
        command: authority::CreateTrackedThread,
    },
    EditAuthorityTrackedThread {
        command: authority::EditTrackedThread,
    },
    DeleteAuthorityTrackedThread {
        command: authority::DeleteTrackedThread,
    },
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
    LoadProposalArtifactSummaryListForWorkUnit {
        work_unit_id: String,
    },
    LoadProposalArtifactSummary {
        proposal_id: String,
    },
    LoadProposalArtifactDetail {
        proposal_id: String,
    },
    ExportProposalArtifact {
        proposal_id: String,
        destination: String,
        format: ReviewArtifactExportFormat,
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
        // The TUI needs both daemon surfaces at startup: `state/get` for the live supervision
        // snapshot and `authority/hierarchy/get` for authority-only hierarchy state.
        Action::Start => vec![Effect::RefreshSnapshot, Effect::LoadAuthorityHierarchy],
        Action::User(user_action) => reduce_user_action(state, user_action),
        Action::Event(event) => reduce_event(state, event),
    }
}

fn reduce_user_action(state: &mut AppState, action: UserAction) -> Vec<Effect> {
    match action {
        UserAction::Refresh => {
            let mut effects = vec![Effect::RefreshSnapshot, Effect::LoadAuthorityHierarchy];
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
                reconcile_main_view(state);
                return activate_program_view(state);
            }
            Vec::new()
        }
        UserAction::ShowProgramView(view) => {
            if state.current_view == TopLevelView::Overview {
                state.main_view.program_view = view;
                reconcile_main_view(state);
                return activate_program_view(state);
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
        UserAction::CreateWorkstream => {
            state.authority_main.footer =
                MainFooterState::CreateWorkstream(default_workstream_form());
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: "Creating a new workstream. Tab moves fields, Ctrl+S submits.".to_string(),
            });
            Vec::new()
        }
        UserAction::CreateWorkUnitForSelection => {
            let Some(workstream_id) = selected_main_workstream_id(state) else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Select a workstream to create a work unit.".to_string(),
                });
                return Vec::new();
            };
            state.authority_main.footer =
                MainFooterState::CreateWorkUnit(default_work_unit_form(workstream_id));
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: "Creating a work unit under the selected workstream.".to_string(),
            });
            Vec::new()
        }
        UserAction::CreateTrackedThreadForSelection => {
            let Some(work_unit_id) = selected_main_work_unit_id(state) else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Select a work unit to create a tracked thread.".to_string(),
                });
                return Vec::new();
            };
            state.authority_main.footer =
                MainFooterState::CreateTrackedThread(default_tracked_thread_form(work_unit_id));
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: "Creating a tracked thread under the selected work unit.".to_string(),
            });
            Vec::new()
        }
        UserAction::EditSelectedMainEntity => open_main_footer_for_edit(state),
        UserAction::DeleteSelectedMainEntity => {
            let Some(target) = selected_main_delete_target(state) else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Select a workstream, work unit, or tracked thread to delete."
                        .to_string(),
                });
                return Vec::new();
            };
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: "Loading delete confirmation…".to_string(),
            });
            vec![Effect::LoadAuthorityDeletePlan { target }]
        }
        UserAction::MainFooterAppend(ch) => {
            if let Some(field) = active_main_footer_field_mut(state) {
                footer_field_insert(field, ch);
                state.banner = None;
            }
            Vec::new()
        }
        UserAction::MainFooterBackspace => {
            if let Some(field) = active_main_footer_field_mut(state) {
                footer_field_backspace(field);
                state.banner = None;
            }
            Vec::new()
        }
        UserAction::MainFooterDelete => {
            if let Some(field) = active_main_footer_field_mut(state) {
                footer_field_delete(field);
                state.banner = None;
            }
            Vec::new()
        }
        UserAction::MainFooterMoveLeft => {
            if let Some(field) = active_main_footer_field_mut(state) {
                footer_field_move_left(field);
                state.banner = None;
            }
            Vec::new()
        }
        UserAction::MainFooterMoveRight => {
            if let Some(field) = active_main_footer_field_mut(state) {
                footer_field_move_right(field);
                state.banner = None;
            }
            Vec::new()
        }
        UserAction::MainFooterNextField => {
            cycle_main_footer_field(state, 1);
            state.banner = None;
            Vec::new()
        }
        UserAction::MainFooterPreviousField => {
            cycle_main_footer_field(state, -1);
            state.banner = None;
            Vec::new()
        }
        UserAction::SubmitMainFooter => submit_main_footer(state),
        UserAction::CancelMainFooter => {
            if !matches!(state.authority_main.footer, MainFooterState::Inspect) {
                state.authority_main.footer = MainFooterState::Inspect;
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Info,
                    message: "Cancelled footer action.".to_string(),
                });
            }
            Vec::new()
        }
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
            let Some(decision_id) = selected_supervisor_decision_id_for_review_action(state) else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: selected_supervisor_decision_action_unavailable_message(state),
                });
                return Vec::new();
            };
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: format!(
                    "Approving and sending supervisor decision {}...",
                    decision_id
                ),
            });
            vec![Effect::ApproveSupervisorDecision { decision_id }]
        }
        UserAction::RejectSelectedSupervisorDecision => {
            let Some(decision_id) = selected_supervisor_decision_id_for_review_action(state) else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: selected_supervisor_decision_action_unavailable_message(state),
                });
                return Vec::new();
            };
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: format!("Rejecting supervisor decision {}...", decision_id),
            });
            vec![Effect::RejectSupervisorDecision { decision_id }]
        }
        UserAction::OpenSelectedProposalArtifactDetail => {
            let Some(proposal_id) = selected_review_proposal_id(state).map(ToOwned::to_owned)
            else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Select a proposal or failure row to inspect artifacts.".to_string(),
                });
                return Vec::new();
            };
            state.review_view.artifact_export = None;
            state.review_view.artifact_detail = Some(ReviewArtifactDetailState {
                proposal_id: proposal_id.clone(),
                scroll_offset: 0,
            });
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: format!("Opening artifact evidence for proposal {}...", proposal_id),
            });
            if state.proposal_artifact_details.contains_key(&proposal_id)
                || state
                    .loading_proposal_artifact_details
                    .contains(proposal_id.as_str())
            {
                Vec::new()
            } else {
                state
                    .proposal_artifact_detail_errors
                    .remove(proposal_id.as_str());
                state
                    .loading_proposal_artifact_details
                    .insert(proposal_id.clone());
                vec![Effect::LoadProposalArtifactDetail { proposal_id }]
            }
        }
        UserAction::CloseReviewArtifactDetail => {
            if state.review_view.artifact_detail.take().is_some() {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Info,
                    message: "Closed proposal artifact detail.".to_string(),
                });
            }
            Vec::new()
        }
        UserAction::ScrollReviewArtifactDetail(delta) => {
            if let Some(detail) = state.review_view.artifact_detail.as_mut() {
                if delta.is_negative() {
                    detail.scroll_offset = detail
                        .scroll_offset
                        .saturating_sub(delta.unsigned_abs() as usize);
                } else {
                    detail.scroll_offset = detail.scroll_offset.saturating_add(delta as usize);
                }
            }
            Vec::new()
        }
        UserAction::OpenSelectedProposalArtifactExport => {
            let Some(proposal_id) = selected_review_proposal_id(state).map(ToOwned::to_owned)
            else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Select a proposal or failure row to export artifacts.".to_string(),
                });
                return Vec::new();
            };
            state.review_view.artifact_detail = None;
            let format = ReviewArtifactExportFormat::Json;
            let auto_destination =
                default_review_artifact_export_path(proposal_id.as_str(), format);
            state.review_view.artifact_export = Some(ReviewArtifactExportState {
                proposal_id: proposal_id.clone(),
                format,
                destination: FooterFieldState::new(auto_destination.clone()),
                auto_destination,
                destination_is_auto: true,
                in_flight: false,
                error: None,
            });
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: format!(
                    "Preparing JSON artifact export for proposal {}.",
                    proposal_id
                ),
            });
            Vec::new()
        }
        UserAction::CloseReviewArtifactExport => {
            if state.review_view.artifact_export.take().is_some() {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Info,
                    message: "Cancelled proposal artifact export.".to_string(),
                });
            }
            Vec::new()
        }
        UserAction::SubmitReviewArtifactExport => {
            let Some(export) = state.review_view.artifact_export.as_mut() else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "No proposal artifact export is active.".to_string(),
                });
                return Vec::new();
            };
            let destination = export.destination.value.trim().to_string();
            if destination.is_empty() {
                export.error = Some("Destination path must not be empty.".to_string());
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Destination path must not be empty.".to_string(),
                });
                return Vec::new();
            }
            export.destination = FooterFieldState::new(destination.clone());
            export.in_flight = true;
            export.error = None;
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: format!(
                    "Exporting proposal artifact as {} to {}...",
                    export.format.label(),
                    destination
                ),
            });
            vec![Effect::ExportProposalArtifact {
                proposal_id: export.proposal_id.clone(),
                destination,
                format: export.format,
            }]
        }
        UserAction::ReviewArtifactExportToggleFormat => {
            let Some(export) = state.review_view.artifact_export.as_mut() else {
                return Vec::new();
            };
            export.format = export.format.toggle();
            export.error = None;
            let next_auto =
                default_review_artifact_export_path(export.proposal_id.as_str(), export.format);
            if export.destination_is_auto && export.destination.value == export.auto_destination {
                export.destination = FooterFieldState::new(next_auto.clone());
            }
            export.auto_destination = next_auto;
            if export.destination.value == export.auto_destination {
                export.destination_is_auto = true;
            }
            state.banner = None;
            Vec::new()
        }
        UserAction::ReviewArtifactExportAppend(ch) => {
            if let Some(export) = state.review_view.artifact_export.as_mut() {
                footer_field_insert(&mut export.destination, ch);
                export.destination_is_auto = false;
                export.error = None;
                state.banner = None;
            }
            Vec::new()
        }
        UserAction::ReviewArtifactExportBackspace => {
            if let Some(export) = state.review_view.artifact_export.as_mut() {
                footer_field_backspace(&mut export.destination);
                export.destination_is_auto = false;
                export.error = None;
                state.banner = None;
            }
            Vec::new()
        }
        UserAction::ReviewArtifactExportDelete => {
            if let Some(export) = state.review_view.artifact_export.as_mut() {
                footer_field_delete(&mut export.destination);
                export.destination_is_auto = false;
                export.error = None;
                state.banner = None;
            }
            Vec::new()
        }
        UserAction::ReviewArtifactExportMoveLeft => {
            if let Some(export) = state.review_view.artifact_export.as_mut() {
                footer_field_move_left(&mut export.destination);
                export.error = None;
                state.banner = None;
            }
            Vec::new()
        }
        UserAction::ReviewArtifactExportMoveRight => {
            if let Some(export) = state.review_view.artifact_export.as_mut() {
                footer_field_move_right(&mut export.destination);
                export.error = None;
                state.banner = None;
            }
            Vec::new()
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
            // A fresh snapshot only re-establishes the collaboration/runtime view. The authority
            // hierarchy and authority detail panes must be reloaded separately after reconnect or
            // restart so the TUI does not keep rendering stale pre-restart authority records.
            invalidate_authority_view_state(state, true);
            state.daemon_phase = DaemonConnectionPhase::Connected;
            state.reconnect_attempt = 0;
            state.daemon_lifecycle =
                lifecycle_from_upstream_status(&snapshot.daemon.upstream.status);
            state.daemon_lifecycle_error = None;
            state.daemon = Some(snapshot.daemon);
            state.session = snapshot.session;
            state.collaboration = snapshot.collaboration;
            state.proposal_artifact_summary_work_units.clear();
            state.loaded_proposal_artifact_summary_work_units.clear();
            state.loading_proposal_artifact_summary_work_units.clear();
            state.proposal_artifact_summary_work_unit_errors.clear();
            state.proposal_artifact_summaries.clear();
            state.proposal_artifact_details.clear();
            state.loading_proposal_artifact_summaries.clear();
            state.loading_proposal_artifact_details.clear();
            state.proposal_artifact_summary_errors.clear();
            state.proposal_artifact_detail_errors.clear();
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
            effects.extend(load_review_queue_artifact_summaries_if_needed(state));
            effects.push(Effect::LoadActiveTurns);
            effects.push(Effect::LoadAuthorityHierarchy);
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
            // Disconnect invalidates authority caches immediately. Keep only the
            // user's selection intent so the next snapshot can restore the same
            // row if it still exists.
            state.models_loading = false;
            state.daemon_phase = DaemonConnectionPhase::Reconnecting;
            state.prompt_in_flight = false;
            state.proposal_artifact_summary_work_units.clear();
            state.loaded_proposal_artifact_summary_work_units.clear();
            state.loading_proposal_artifact_summary_work_units.clear();
            state.proposal_artifact_summary_work_unit_errors.clear();
            state.proposal_artifact_summaries.clear();
            state.proposal_artifact_details.clear();
            state.loading_proposal_artifact_summaries.clear();
            state.loading_proposal_artifact_details.clear();
            state.proposal_artifact_summary_errors.clear();
            state.proposal_artifact_detail_errors.clear();
            state.review_view.artifact_detail = None;
            invalidate_authority_view_state(state, true);
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
        UiEvent::AuthorityHierarchyLoaded(hierarchy) => {
            // The hierarchy refresh updates the selectable tree, but detail
            // panes are still loaded independently so stale edit forms do not
            // survive a reconnect or delete.
            state.authority_main.hierarchy = hierarchy;
            if state.main_view.expanded_workstreams.is_empty()
                && state.main_view.expanded_work_units.is_empty()
            {
                state.main_view.expanded_workstreams.extend(
                    state
                        .authority_main
                        .hierarchy
                        .workstreams
                        .iter()
                        .map(|workstream| workstream.workstream.id.to_string()),
                );
                state.main_view.expanded_work_units.extend(
                    state
                        .authority_main
                        .hierarchy
                        .workstreams
                        .iter()
                        .flat_map(|workstream| {
                            workstream
                                .work_units
                                .iter()
                                .map(|work_unit| work_unit.work_unit.id.to_string())
                        }),
                );
            }
            reconcile_main_view(state);
            effects.extend(load_selected_main_detail_if_needed(state));
        }
        UiEvent::AuthorityWorkstreamDetailLoaded(detail) => {
            // Authority detail caches are separate from the hierarchy snapshot;
            // load them explicitly so edit forms always read a current record.
            state
                .authority_main
                .workstream_details
                .insert(detail.workstream.id.to_string(), detail);
            reconcile_main_view(state);
        }
        UiEvent::AuthorityWorkUnitDetailLoaded(detail) => {
            state
                .authority_main
                .work_unit_details
                .insert(detail.work_unit.id.to_string(), detail);
            reconcile_main_view(state);
        }
        UiEvent::AuthorityTrackedThreadDetailLoaded(detail) => {
            state
                .authority_main
                .tracked_thread_details
                .insert(detail.tracked_thread.id.to_string(), detail);
            reconcile_main_view(state);
        }
        UiEvent::AuthorityDeletePlanLoaded(delete_plan) => {
            let Some(target) = delete_target_from_aggregate_key(&delete_plan.target.aggregate_key)
            else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Error,
                    message: "Delete plan target could not be decoded.".to_string(),
                });
                return effects;
            };
            state.authority_main.footer = MainFooterState::ConfirmDelete(DeleteFooterState {
                fallback_selection: delete_fallback_for_target(state, &target),
                target,
                label: delete_plan.target.label.clone(),
                expected_revision: delete_plan.expected_revision,
                confirmation_token: delete_plan.confirmation_token,
                requires_typed_confirmation: delete_plan.requires_typed_confirmation,
                active_field: 0,
                typed_confirmation: FooterFieldState::new(String::new()),
                affected_work_units: delete_plan.affected_work_units,
                affected_tracked_threads: delete_plan.affected_tracked_threads,
                has_upstream_bindings: delete_plan.has_upstream_bindings,
            });
            state.banner = Some(StatusBanner {
                level: BannerLevel::Warning,
                message: format!("Confirm delete for `{}`.", delete_plan.target.label),
            });
        }
        // Authority mutations follow the same pattern across workstream,
        // work unit, and tracked-thread records: keep user intent, clear the
        // stale footer, and reload hierarchy/detail caches rather than trying
        // to mutate the event payload in place.
        UiEvent::AuthorityWorkstreamCreated(workstream)
        | UiEvent::AuthorityWorkstreamEdited(workstream) => {
            state.authority_main.footer = MainFooterState::Inspect;
            state.main_view.pending_selection = Some(MainHierarchySelection::Workstream {
                workstream_id: workstream.id.to_string(),
            });
            state
                .authority_main
                .workstream_details
                .remove(workstream.id.as_str());
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: format!("Saved workstream `{}`.", workstream.title),
            });
            effects.push(Effect::LoadAuthorityHierarchy);
            effects.push(Effect::LoadAuthorityWorkstreamDetail {
                workstream_id: workstream.id.to_string(),
            });
        }
        // Authority delete boundaries follow the same pattern across workstream,
        // work unit, and tracked-thread records: close stale detail panes and
        // preserve a valid fallback selection instead of keeping deleted rows alive.
        UiEvent::AuthorityWorkstreamDeleted(workstream) => {
            let fallback = selected_main_delete_fallback(state);
            state.authority_main.footer = MainFooterState::Inspect;
            state
                .authority_main
                .workstream_details
                .remove(workstream.id.as_str());
            state.main_view.pending_selection = fallback;
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: format!("Deleted workstream `{}`.", workstream.title),
            });
            effects.push(Effect::LoadAuthorityHierarchy);
        }
        UiEvent::AuthorityWorkUnitCreated(work_unit)
        | UiEvent::AuthorityWorkUnitEdited(work_unit) => {
            state.authority_main.footer = MainFooterState::Inspect;
            state.main_view.pending_selection = Some(MainHierarchySelection::WorkUnit {
                workstream_id: work_unit.workstream_id.to_string(),
                work_unit_id: work_unit.id.to_string(),
            });
            state
                .authority_main
                .work_unit_details
                .remove(work_unit.id.as_str());
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: format!("Saved work unit `{}`.", work_unit.title),
            });
            effects.push(Effect::LoadAuthorityHierarchy);
            effects.push(Effect::LoadAuthorityWorkUnitDetail {
                work_unit_id: work_unit.id.to_string(),
            });
        }
        UiEvent::AuthorityWorkUnitDeleted(work_unit) => {
            let fallback = selected_main_delete_fallback(state);
            state.authority_main.footer = MainFooterState::Inspect;
            state
                .authority_main
                .work_unit_details
                .remove(work_unit.id.as_str());
            state.main_view.pending_selection = fallback;
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: format!("Deleted work unit `{}`.", work_unit.title),
            });
            effects.push(Effect::LoadAuthorityHierarchy);
        }
        UiEvent::AuthorityTrackedThreadCreated(tracked_thread)
        | UiEvent::AuthorityTrackedThreadEdited(tracked_thread) => {
            let parent_workstream_id =
                workstream_id_for_work_unit(state, tracked_thread.work_unit_id.as_str())
                    .unwrap_or_default();
            state.authority_main.footer = MainFooterState::Inspect;
            state.main_view.pending_selection = Some(MainHierarchySelection::Thread {
                workstream_id: parent_workstream_id,
                work_unit_id: tracked_thread.work_unit_id.to_string(),
                thread_id: tracked_thread.id.to_string(),
            });
            state
                .authority_main
                .tracked_thread_details
                .remove(tracked_thread.id.as_str());
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: format!("Saved tracked thread `{}`.", tracked_thread.title),
            });
            effects.push(Effect::LoadAuthorityHierarchy);
            effects.push(Effect::LoadAuthorityTrackedThreadDetail {
                tracked_thread_id: tracked_thread.id.to_string(),
            });
        }
        UiEvent::AuthorityTrackedThreadDeleted(tracked_thread) => {
            let fallback = selected_main_delete_fallback(state);
            state.authority_main.footer = MainFooterState::Inspect;
            state
                .authority_main
                .tracked_thread_details
                .remove(tracked_thread.id.as_str());
            state.main_view.pending_selection = fallback;
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: format!("Deleted tracked thread `{}` locally.", tracked_thread.title),
            });
            effects.push(Effect::LoadAuthorityHierarchy);
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
        UiEvent::WorkstreamLifecycle { action, workstream } => {
            if workstream.source_kind == ipc::PlanningSummarySourceKind::AuthorityProjection {
                if action == ipc::CollaborationLifecycleAction::Deleted {
                    state
                        .authority_main
                        .workstream_details
                        .remove(workstream.id.as_str());
                }
                effects.push(Effect::LoadAuthorityHierarchy);
                if matches!(
                    state.main_view.selected.as_ref(),
                    Some(MainHierarchySelection::Workstream { workstream_id })
                        if workstream_id == workstream.id.as_str()
                ) && action != ipc::CollaborationLifecycleAction::Deleted
                {
                    effects.push(Effect::LoadAuthorityWorkstreamDetail {
                        workstream_id: workstream.id.clone(),
                    });
                }
            } else if action != ipc::CollaborationLifecycleAction::Deleted {
                upsert_workstream_summary(&mut state.collaboration.workstreams, workstream);
                effects.extend(load_selected_work_unit_detail_if_needed(state));
            }
            reconcile_collaboration_selection(state);
            reconcile_main_view(state);
        }
        UiEvent::WorkUnitLifecycle { action, work_unit } => {
            let selected = state.selected_work_unit_id.as_deref() == Some(work_unit.id.as_str());
            if work_unit.source_kind == ipc::PlanningSummarySourceKind::AuthorityProjection {
                if action == ipc::CollaborationLifecycleAction::Deleted {
                    state
                        .authority_main
                        .work_unit_details
                        .remove(work_unit.id.as_str());
                }
                effects.push(Effect::LoadAuthorityHierarchy);
                if matches!(
                    state.main_view.selected.as_ref(),
                    Some(MainHierarchySelection::WorkUnit { work_unit_id, .. })
                        if work_unit_id == work_unit.id.as_str()
                ) && action != ipc::CollaborationLifecycleAction::Deleted
                {
                    effects.push(Effect::LoadAuthorityWorkUnitDetail {
                        work_unit_id: work_unit.id.clone(),
                    });
                }
            } else if action != ipc::CollaborationLifecycleAction::Deleted {
                upsert_work_unit_summary(&mut state.collaboration.work_units, work_unit);
                if selected {
                    effects.extend(load_selected_work_unit_detail(state));
                } else {
                    effects.extend(load_selected_work_unit_detail_if_needed(state));
                }
            }
            reconcile_collaboration_selection(state);
            reconcile_main_view(state);
        }
        UiEvent::TrackedThreadLifecycle {
            action,
            tracked_thread,
        } => {
            if action == ipc::CollaborationLifecycleAction::Deleted {
                state
                    .authority_main
                    .tracked_thread_details
                    .remove(tracked_thread.id.as_str());
            }
            effects.push(Effect::LoadAuthorityHierarchy);
            if matches!(
                state.main_view.selected.as_ref(),
                Some(MainHierarchySelection::Thread { thread_id, .. })
                    if thread_id == tracked_thread.id.as_str()
            ) && action != ipc::CollaborationLifecycleAction::Deleted
            {
                effects.push(Effect::LoadAuthorityTrackedThreadDetail {
                    tracked_thread_id: tracked_thread.id.to_string(),
                });
            }
            reconcile_main_view(state);
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
            state
                .proposal_artifact_summaries
                .remove(proposal.id.as_str());
            state.proposal_artifact_details.remove(proposal.id.as_str());
            state
                .loading_proposal_artifact_summaries
                .remove(proposal.id.as_str());
            state
                .loading_proposal_artifact_details
                .remove(proposal.id.as_str());
            state
                .proposal_artifact_summary_errors
                .remove(proposal.id.as_str());
            state
                .proposal_artifact_detail_errors
                .remove(proposal.id.as_str());
            invalidate_work_unit_artifact_summary_cache(state, &proposal.primary_work_unit_id);
            upsert_work_unit_summary(&mut state.collaboration.work_units, work_unit);
            reconcile_main_view(state);
            if selected {
                effects.extend(load_selected_work_unit_detail(state));
            } else {
                state
                    .work_unit_details
                    .remove(&proposal.primary_work_unit_id);
            }
            effects.extend(load_review_queue_artifact_summaries_if_needed(state));
        }
        UiEvent::ProposalArtifactSummaryListLoaded(response) => {
            state
                .loading_proposal_artifact_summary_work_units
                .remove(response.work_unit_id.as_str());
            state
                .proposal_artifact_summary_work_unit_errors
                .remove(response.work_unit_id.as_str());
            replace_work_unit_artifact_summaries(state, &response.work_unit_id, response.summaries);
            state
                .loaded_proposal_artifact_summary_work_units
                .insert(response.work_unit_id);
        }
        UiEvent::ProposalArtifactSummaryListLoadFailed {
            work_unit_id,
            message,
        } => {
            state
                .loading_proposal_artifact_summary_work_units
                .remove(work_unit_id.as_str());
            state
                .proposal_artifact_summary_work_unit_errors
                .insert(work_unit_id.clone(), message.clone());
            state.banner = Some(StatusBanner {
                level: BannerLevel::Warning,
                message: format!(
                    "Proposal artifact summaries unavailable for {work_unit_id}: {message}"
                ),
            });
        }
        UiEvent::ProposalArtifactSummaryLoaded(summary) => {
            state
                .loading_proposal_artifact_summaries
                .remove(summary.proposal_id.as_str());
            state
                .proposal_artifact_summary_errors
                .remove(summary.proposal_id.as_str());
            state
                .proposal_artifact_summaries
                .insert(summary.proposal_id.clone(), summary);
        }
        UiEvent::ProposalArtifactSummaryLoadFailed {
            proposal_id,
            message,
        } => {
            state
                .loading_proposal_artifact_summaries
                .remove(proposal_id.as_str());
            state
                .proposal_artifact_summary_errors
                .insert(proposal_id.clone(), message.clone());
            state.banner = Some(StatusBanner {
                level: BannerLevel::Warning,
                message: format!(
                    "Proposal artifact summary unavailable for {proposal_id}: {message}"
                ),
            });
        }
        UiEvent::ProposalArtifactDetailLoaded(detail) => {
            state
                .loading_proposal_artifact_details
                .remove(detail.proposal_id.as_str());
            state
                .proposal_artifact_detail_errors
                .remove(detail.proposal_id.as_str());
            state
                .proposal_artifact_details
                .insert(detail.proposal_id.clone(), detail);
        }
        UiEvent::ProposalArtifactDetailLoadFailed {
            proposal_id,
            message,
        } => {
            state
                .loading_proposal_artifact_details
                .remove(proposal_id.as_str());
            state
                .proposal_artifact_detail_errors
                .insert(proposal_id.clone(), message.clone());
            state.banner = Some(StatusBanner {
                level: BannerLevel::Warning,
                message: format!(
                    "Proposal artifact detail unavailable for {proposal_id}: {message}"
                ),
            });
        }
        UiEvent::ProposalArtifactExported {
            proposal_id,
            destination,
            format,
        } => {
            if state
                .review_view
                .artifact_export
                .as_ref()
                .is_some_and(|export| export.proposal_id == proposal_id)
            {
                state.review_view.artifact_export = None;
            }
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: format!(
                    "Exported proposal artifact {proposal_id} as {} to {destination}.",
                    format.label()
                ),
            });
        }
        UiEvent::ProposalArtifactExportFailed {
            proposal_id,
            message,
            format,
        } => {
            if let Some(export) = state.review_view.artifact_export.as_mut() {
                if export.proposal_id == proposal_id {
                    export.in_flight = false;
                    export.error = Some(message.clone());
                }
            }
            state.banner = Some(StatusBanner {
                level: BannerLevel::Error,
                message: format!(
                    "Proposal artifact export failed for {proposal_id} as {}: {message}",
                    format.label()
                ),
            });
        }
        UiEvent::WorkUnitDetailLoaded(detail) => {
            state
                .work_unit_details
                .insert(detail.work_unit.id.clone(), detail);
            reconcile_main_view(state);
            effects.extend(load_review_queue_artifact_summaries_if_needed(state));
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
        TopLevelView::Overview => match state.main_view.program_view {
            ProgramView::Main => select_relative_main_hierarchy(state, delta),
            ProgramView::Review => select_relative_review_queue(state, delta),
        },
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
        TopLevelView::Overview => match state.main_view.program_view {
            ProgramView::Main => expand_selected_main_hierarchy(state),
            ProgramView::Review => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn collapse_selected_in_view(state: &mut AppState) -> Vec<Effect> {
    match state.current_view {
        TopLevelView::Overview => match state.main_view.program_view {
            ProgramView::Main => collapse_selected_main_hierarchy(state),
            ProgramView::Review => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn activate_program_view(state: &mut AppState) -> Vec<Effect> {
    match state.main_view.program_view {
        ProgramView::Main => state
            .main_view
            .selected
            .clone()
            .map(|selection| apply_main_selection(state, selection, false, true))
            .unwrap_or_default(),
        ProgramView::Review => state
            .review_view
            .selected
            .clone()
            .map(|selection| apply_review_selection(state, selection, true))
            .unwrap_or_default(),
    }
}

fn reconcile_main_view(state: &mut AppState) {
    // Reconcile selection against the current authority hierarchy. `pending`
    // selection preserves intent across invalidation boundaries, but it only
    // becomes the active selection once the row is visible again.
    state
        .main_view
        .expanded_workstreams
        .retain(|workstream_id| {
            state
                .authority_main
                .hierarchy
                .workstreams
                .iter()
                .any(|workstream| workstream.workstream.id.as_str() == workstream_id)
        });
    state.main_view.expanded_work_units.retain(|work_unit_id| {
        state
            .authority_main
            .hierarchy
            .workstreams
            .iter()
            .any(|workstream| {
                workstream
                    .work_units
                    .iter()
                    .any(|work_unit| work_unit.work_unit.id.as_str() == work_unit_id)
            })
    });

    if !state.main_view.initialized {
        state.main_view.expanded_workstreams.extend(
            state
                .authority_main
                .hierarchy
                .workstreams
                .iter()
                .map(|workstream| workstream.workstream.id.to_string()),
        );
        state.main_view.expanded_work_units.extend(
            state
                .authority_main
                .hierarchy
                .workstreams
                .iter()
                .flat_map(|workstream| {
                    workstream
                        .work_units
                        .iter()
                        .map(|work_unit| work_unit.work_unit.id.to_string())
                }),
        );
        state.main_view.initialized = true;
    }

    let visible_rows = visible_main_hierarchy_rows(state);
    if visible_rows.is_empty() {
        state.main_view.selected = None;
        state.main_view.scroll_offset = 0;
        reconcile_review_view(state);
        return;
    }

    let selection = state
        .main_view
        .pending_selection
        .clone()
        .filter(|selected| visible_rows.contains(selected))
        .or_else(|| {
            state
                .main_view
                .selected
                .clone()
                .filter(|selected| visible_rows.contains(selected))
        })
        .or_else(|| preferred_main_selection(state))
        .unwrap_or_else(|| visible_rows[0].clone());
    state.main_view.pending_selection = None;
    restore_main_selection(state, selection);
    reconcile_review_view(state);
}

fn reconcile_review_view(state: &mut AppState) {
    let visible_rows = review_queue_selections(state);
    if visible_rows.is_empty() {
        state.review_view.selected = None;
        state.review_view.scroll_offset = 0;
        state.review_view.selection_anchor = 0;
        state.review_view.artifact_detail = None;
        state.review_view.artifact_export = None;
        return;
    }

    let selection = state
        .review_view
        .selected
        .clone()
        .filter(|selected| visible_rows.contains(selected))
        .or_else(|| {
            let anchored_index = state
                .review_view
                .selection_anchor
                .min(visible_rows.len().saturating_sub(1));
            visible_rows.get(anchored_index).cloned()
        })
        .unwrap_or_else(|| visible_rows[0].clone());
    restore_review_selection(state, selection);
    if state
        .review_view
        .artifact_detail
        .as_ref()
        .is_some_and(|detail| {
            state
                .review_view
                .selected
                .as_ref()
                .and_then(review_selection_proposal_id)
                != Some(detail.proposal_id.as_str())
        })
    {
        state.review_view.artifact_detail = None;
    }
    if state
        .review_view
        .artifact_export
        .as_ref()
        .is_some_and(|export| {
            state
                .review_view
                .selected
                .as_ref()
                .and_then(review_selection_proposal_id)
                != Some(export.proposal_id.as_str())
        })
    {
        state.review_view.artifact_export = None;
    }
}

pub(crate) fn review_queue_selections(state: &AppState) -> Vec<ReviewSelection> {
    let mut queue = state
        .collaboration
        .supervisor_turn_decisions
        .iter()
        .filter(|decision| {
            decision.status == orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman
                || decision.open
        })
        .map(|decision| {
            (
                0u8,
                decision.created_at,
                ReviewSelection::Decision {
                    decision_id: decision.decision_id.clone(),
                },
            )
        })
        .collect::<Vec<_>>();

    queue.extend(
        state
            .collaboration
            .work_units
            .iter()
            .filter_map(|work_unit| {
                let proposal = work_unit.proposal.as_ref()?;
                match proposal.latest_status {
                    orcas_core::SupervisorProposalStatus::Open => Some((
                        1u8,
                        proposal.latest_created_at,
                        ReviewSelection::Proposal {
                            work_unit_id: work_unit.id.clone(),
                            proposal_id: proposal.latest_proposal_id.clone(),
                        },
                    )),
                    orcas_core::SupervisorProposalStatus::GenerationFailed => Some((
                        2u8,
                        proposal.latest_created_at,
                        ReviewSelection::Failure {
                            work_unit_id: work_unit.id.clone(),
                            proposal_id: proposal.latest_proposal_id.clone(),
                        },
                    )),
                    _ => None,
                }
            }),
    );

    queue.extend(
        state
            .collaboration
            .reports
            .iter()
            .filter(|report| report.needs_supervisor_review)
            .map(|report| {
                (
                    3u8,
                    report.created_at,
                    ReviewSelection::ReviewRequired {
                        work_unit_id: report.work_unit_id.clone(),
                        report_id: report.id.clone(),
                    },
                )
            }),
    );

    queue.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| right.1.cmp(&left.1))
            .then_with(|| {
                review_selection_sort_key(&left.2).cmp(&review_selection_sort_key(&right.2))
            })
    });

    queue
        .into_iter()
        .map(|(_, _, selection)| selection)
        .collect()
}

fn review_selection_sort_key(selection: &ReviewSelection) -> String {
    match selection {
        ReviewSelection::Proposal {
            work_unit_id,
            proposal_id,
        } => format!("proposal/{work_unit_id}/{proposal_id}"),
        ReviewSelection::Decision { decision_id } => format!("decision/{decision_id}"),
        ReviewSelection::Failure {
            work_unit_id,
            proposal_id,
        } => format!("failure/{work_unit_id}/{proposal_id}"),
        ReviewSelection::ReviewRequired {
            work_unit_id,
            report_id,
        } => format!("review/{work_unit_id}/{report_id}"),
    }
}

fn preferred_main_selection(state: &AppState) -> Option<MainHierarchySelection> {
    selection_from_upstream_thread(state, state.selected_thread_id.as_deref())
        .or_else(|| {
            selection_from_upstream_thread(state, state.session.active_thread_id.as_deref())
        })
        .or_else(|| state.main_view.selected.clone())
        .or_else(|| {
            state
                .authority_main
                .hierarchy
                .workstreams
                .iter()
                .find_map(|workstream| {
                    workstream.work_units.iter().find_map(|work_unit| {
                        work_unit.tracked_threads.first().map(|tracked_thread| {
                            MainHierarchySelection::Thread {
                                workstream_id: workstream.workstream.id.to_string(),
                                work_unit_id: work_unit.work_unit.id.to_string(),
                                thread_id: tracked_thread.id.to_string(),
                            }
                        })
                    })
                })
        })
        .or_else(|| {
            state
                .authority_main
                .hierarchy
                .workstreams
                .iter()
                .find_map(|workstream| {
                    workstream.work_units.first().map(|work_unit| {
                        MainHierarchySelection::WorkUnit {
                            workstream_id: workstream.workstream.id.to_string(),
                            work_unit_id: work_unit.work_unit.id.to_string(),
                        }
                    })
                })
        })
        .or_else(|| {
            state
                .authority_main
                .hierarchy
                .workstreams
                .first()
                .map(|workstream| MainHierarchySelection::Workstream {
                    workstream_id: workstream.workstream.id.to_string(),
                })
        })
}

fn selection_from_upstream_thread(
    state: &AppState,
    upstream_thread_id: Option<&str>,
) -> Option<MainHierarchySelection> {
    let upstream_thread_id = upstream_thread_id?;
    state
        .authority_main
        .hierarchy
        .workstreams
        .iter()
        .find_map(|workstream| {
            workstream.work_units.iter().find_map(|work_unit| {
                work_unit.tracked_threads.iter().find_map(|tracked_thread| {
                    (tracked_thread.upstream_thread_id.as_deref() == Some(upstream_thread_id)).then(
                        || MainHierarchySelection::Thread {
                            workstream_id: workstream.workstream.id.to_string(),
                            work_unit_id: work_unit.work_unit.id.to_string(),
                            thread_id: tracked_thread.id.to_string(),
                        },
                    )
                })
            })
        })
}

fn visible_main_hierarchy_rows(state: &AppState) -> Vec<MainHierarchySelection> {
    let mut rows = Vec::new();
    for workstream in &state.authority_main.hierarchy.workstreams {
        let workstream_id = workstream.workstream.id.to_string();
        rows.push(MainHierarchySelection::Workstream {
            workstream_id: workstream_id.clone(),
        });
        if !state
            .main_view
            .expanded_workstreams
            .contains(workstream_id.as_str())
        {
            continue;
        }

        for work_unit in &workstream.work_units {
            let work_unit_id = work_unit.work_unit.id.to_string();
            rows.push(MainHierarchySelection::WorkUnit {
                workstream_id: workstream_id.clone(),
                work_unit_id: work_unit_id.clone(),
            });
            if !state
                .main_view
                .expanded_work_units
                .contains(work_unit_id.as_str())
            {
                continue;
            }

            for tracked_thread in &work_unit.tracked_threads {
                rows.push(MainHierarchySelection::Thread {
                    workstream_id: workstream_id.clone(),
                    work_unit_id: work_unit_id.clone(),
                    thread_id: tracked_thread.id.to_string(),
                });
            }
        }
    }
    rows
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

fn select_relative_review_queue(state: &mut AppState, delta: isize) -> Vec<Effect> {
    let visible_rows = review_queue_selections(state);
    if visible_rows.is_empty() {
        state.review_view.selected = None;
        state.review_view.scroll_offset = 0;
        return Vec::new();
    }

    let current_index = state
        .review_view
        .selected
        .as_ref()
        .and_then(|selected| visible_rows.iter().position(|row| row == selected))
        .unwrap_or(0);
    let next_index = if delta.is_negative() {
        current_index.saturating_sub(delta.unsigned_abs())
    } else {
        (current_index + delta as usize).min(visible_rows.len().saturating_sub(1))
    };
    set_review_selection(state, visible_rows[next_index].clone())
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

fn set_review_selection(state: &mut AppState, selection: ReviewSelection) -> Vec<Effect> {
    apply_review_selection(state, selection, true)
}

fn restore_review_selection(state: &mut AppState, selection: ReviewSelection) {
    let _ = apply_review_selection(state, selection, false);
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
        MainHierarchySelection::Thread { .. }
        | MainHierarchySelection::WorkUnit { .. }
        | MainHierarchySelection::Workstream { .. } => load_selected_main_detail_if_needed(state),
    }
}

fn apply_review_selection(
    state: &mut AppState,
    selection: ReviewSelection,
    load_effects: bool,
) -> Vec<Effect> {
    state.review_view.selected = Some(selection.clone());
    let visible_rows = review_queue_selections(state);
    if let Some(selected_index) = visible_rows.iter().position(|row| row == &selection) {
        state.review_view.selection_anchor = selected_index;
        adjust_review_scroll(state, selected_index, visible_rows.len());
    } else {
        state.review_view.selection_anchor = 0;
        state.review_view.scroll_offset = 0;
    }

    if !load_effects {
        return Vec::new();
    }

    match selection {
        ReviewSelection::Proposal { work_unit_id, .. }
        | ReviewSelection::Failure { work_unit_id, .. }
        | ReviewSelection::ReviewRequired { work_unit_id, .. } => {
            let mut effects = load_work_unit_detail_if_needed_for(state, &work_unit_id);
            effects.extend(load_review_queue_artifact_summaries_if_needed(state));
            effects
        }
        ReviewSelection::Decision { .. } => Vec::new(),
    }
}

fn load_review_queue_artifact_summaries_if_needed(state: &mut AppState) -> Vec<Effect> {
    let mut work_unit_ids = review_queue_selections(state)
        .into_iter()
        .filter_map(|selection| match selection {
            ReviewSelection::Proposal { work_unit_id, .. }
            | ReviewSelection::Failure { work_unit_id, .. } => Some(work_unit_id),
            ReviewSelection::Decision { .. } | ReviewSelection::ReviewRequired { .. } => None,
        })
        .collect::<Vec<_>>();
    work_unit_ids.sort();
    work_unit_ids.dedup();

    let mut effects = Vec::new();
    for work_unit_id in work_unit_ids {
        if state
            .loaded_proposal_artifact_summary_work_units
            .contains(work_unit_id.as_str())
            || state
                .loading_proposal_artifact_summary_work_units
                .contains(work_unit_id.as_str())
            || state
                .proposal_artifact_summary_work_unit_errors
                .contains_key(work_unit_id.as_str())
        {
            continue;
        }
        state
            .loading_proposal_artifact_summary_work_units
            .insert(work_unit_id.clone());
        effects.push(Effect::LoadProposalArtifactSummaryListForWorkUnit { work_unit_id });
    }
    effects
}

fn invalidate_work_unit_artifact_summary_cache(state: &mut AppState, work_unit_id: &str) {
    state
        .loaded_proposal_artifact_summary_work_units
        .remove(work_unit_id);
    state
        .loading_proposal_artifact_summary_work_units
        .remove(work_unit_id);
    state
        .proposal_artifact_summary_work_unit_errors
        .remove(work_unit_id);
    if let Some(proposal_ids) = state
        .proposal_artifact_summary_work_units
        .remove(work_unit_id)
    {
        for proposal_id in proposal_ids {
            state
                .proposal_artifact_summaries
                .remove(proposal_id.as_str());
            state
                .loading_proposal_artifact_summaries
                .remove(proposal_id.as_str());
            state
                .proposal_artifact_summary_errors
                .remove(proposal_id.as_str());
        }
    }
}

fn replace_work_unit_artifact_summaries(
    state: &mut AppState,
    work_unit_id: &str,
    summaries: Vec<ipc::SupervisorProposalArtifactSummary>,
) {
    invalidate_work_unit_artifact_summary_cache(state, work_unit_id);
    let proposal_ids = summaries
        .iter()
        .map(|summary| summary.proposal_id.clone())
        .collect::<Vec<_>>();
    for summary in summaries {
        state
            .proposal_artifact_summaries
            .insert(summary.proposal_id.clone(), summary);
    }
    state
        .proposal_artifact_summary_work_units
        .insert(work_unit_id.to_string(), proposal_ids);
}

pub(crate) fn review_selection_proposal_id(selection: &ReviewSelection) -> Option<&str> {
    match selection {
        ReviewSelection::Proposal { proposal_id, .. }
        | ReviewSelection::Failure { proposal_id, .. } => Some(proposal_id.as_str()),
        ReviewSelection::Decision { .. } | ReviewSelection::ReviewRequired { .. } => None,
    }
}

fn selected_review_proposal_id(state: &AppState) -> Option<&str> {
    state
        .review_view
        .selected
        .as_ref()
        .and_then(review_selection_proposal_id)
}

fn default_review_artifact_export_path(
    proposal_id: &str,
    format: ReviewArtifactExportFormat,
) -> String {
    std::env::temp_dir()
        .join("orcas-proposal-exports")
        .join(format!("{proposal_id}.{}", format.extension()))
        .display()
        .to_string()
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
            state.selected_work_unit_id =
                first_authority_work_unit_for_workstream(state, workstream_id)
                    .map(|work_unit| work_unit.work_unit.id.to_string());
        }
        MainHierarchySelection::WorkUnit {
            workstream_id,
            work_unit_id,
        } => {
            state.selected_workstream_id = Some(workstream_id.clone());
            state.selected_work_unit_id = Some(work_unit_id.clone());
            state.selected_thread_id = first_upstream_thread_for_work_unit(state, work_unit_id)
                .or_else(|| state.selected_thread_id.clone());
        }
        MainHierarchySelection::Thread {
            workstream_id,
            work_unit_id,
            thread_id,
        } => {
            state.selected_workstream_id = Some(workstream_id.clone());
            state.selected_work_unit_id = Some(work_unit_id.clone());
            state.selected_thread_id = tracked_thread_upstream_binding(state, thread_id)
                .or_else(|| state.selected_thread_id.clone());
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

fn adjust_review_scroll(state: &mut AppState, selected_index: usize, row_count: usize) {
    if selected_index < state.review_view.scroll_offset {
        state.review_view.scroll_offset = selected_index;
    } else {
        let visible_end = state.review_view.scroll_offset + REVIEW_QUEUE_SCROLL_WINDOW;
        if selected_index >= visible_end {
            state.review_view.scroll_offset = selected_index + 1 - REVIEW_QUEUE_SCROLL_WINDOW;
        }
    }
    let max_offset = row_count.saturating_sub(REVIEW_QUEUE_SCROLL_WINDOW);
    state.review_view.scroll_offset = state.review_view.scroll_offset.min(max_offset);
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
    load_work_unit_detail_if_needed_for(state, &work_unit_id)
}

fn load_selected_work_unit_detail(state: &AppState) -> Vec<Effect> {
    state
        .selected_work_unit_id
        .clone()
        .map(|work_unit_id| Effect::LoadWorkUnitDetail { work_unit_id })
        .into_iter()
        .collect()
}

fn load_work_unit_detail_if_needed_for(state: &AppState, work_unit_id: &str) -> Vec<Effect> {
    if state.work_unit_details.contains_key(work_unit_id) {
        Vec::new()
    } else {
        vec![Effect::LoadWorkUnitDetail {
            work_unit_id: work_unit_id.to_string(),
        }]
    }
}

fn load_selected_main_detail_if_needed(state: &AppState) -> Vec<Effect> {
    match state.main_view.selected.as_ref() {
        Some(MainHierarchySelection::Workstream { workstream_id }) => {
            if state
                .authority_main
                .workstream_details
                .contains_key(workstream_id)
            {
                Vec::new()
            } else {
                vec![Effect::LoadAuthorityWorkstreamDetail {
                    workstream_id: workstream_id.clone(),
                }]
            }
        }
        Some(MainHierarchySelection::WorkUnit { work_unit_id, .. }) => {
            if state
                .authority_main
                .work_unit_details
                .contains_key(work_unit_id)
            {
                Vec::new()
            } else {
                vec![Effect::LoadAuthorityWorkUnitDetail {
                    work_unit_id: work_unit_id.clone(),
                }]
            }
        }
        Some(MainHierarchySelection::Thread { thread_id, .. }) => {
            if state
                .authority_main
                .tracked_thread_details
                .contains_key(thread_id)
            {
                Vec::new()
            } else {
                vec![Effect::LoadAuthorityTrackedThreadDetail {
                    tracked_thread_id: thread_id.clone(),
                }]
            }
        }
        None => Vec::new(),
    }
}

fn selected_main_workstream_id(state: &AppState) -> Option<String> {
    match state.main_view.selected.as_ref() {
        Some(MainHierarchySelection::Workstream { workstream_id })
        | Some(MainHierarchySelection::WorkUnit { workstream_id, .. })
        | Some(MainHierarchySelection::Thread { workstream_id, .. }) => Some(workstream_id.clone()),
        None => None,
    }
}

fn selected_main_work_unit_id(state: &AppState) -> Option<String> {
    match state.main_view.selected.as_ref() {
        Some(MainHierarchySelection::WorkUnit { work_unit_id, .. })
        | Some(MainHierarchySelection::Thread { work_unit_id, .. }) => Some(work_unit_id.clone()),
        _ => None,
    }
}

fn selected_main_delete_target(state: &AppState) -> Option<authority::DeleteTarget> {
    match state.main_view.selected.as_ref()? {
        MainHierarchySelection::Workstream { workstream_id } => {
            Some(authority::DeleteTarget::Workstream {
                workstream_id: authority::WorkstreamId::parse(workstream_id.clone()).ok()?,
            })
        }
        MainHierarchySelection::WorkUnit { work_unit_id, .. } => {
            Some(authority::DeleteTarget::WorkUnit {
                work_unit_id: authority::WorkUnitId::parse(work_unit_id.clone()).ok()?,
            })
        }
        MainHierarchySelection::Thread { thread_id, .. } => {
            Some(authority::DeleteTarget::TrackedThread {
                tracked_thread_id: authority::TrackedThreadId::parse(thread_id.clone()).ok()?,
            })
        }
    }
}

fn selected_main_delete_fallback(state: &AppState) -> Option<MainHierarchySelection> {
    match &state.authority_main.footer {
        MainFooterState::ConfirmDelete(delete) => delete.fallback_selection.clone(),
        _ => None,
    }
}

fn delete_fallback_for_target(
    state: &AppState,
    target: &authority::DeleteTarget,
) -> Option<MainHierarchySelection> {
    let rows = visible_main_hierarchy_rows(state);
    let selected = match target {
        authority::DeleteTarget::Workstream { workstream_id } => {
            MainHierarchySelection::Workstream {
                workstream_id: workstream_id.to_string(),
            }
        }
        authority::DeleteTarget::WorkUnit { work_unit_id } => {
            let workstream_id = workstream_id_for_work_unit(state, work_unit_id.as_str())?;
            MainHierarchySelection::WorkUnit {
                workstream_id,
                work_unit_id: work_unit_id.to_string(),
            }
        }
        authority::DeleteTarget::TrackedThread { tracked_thread_id } => {
            let work_unit_id = work_unit_id_for_tracked_thread(state, tracked_thread_id.as_str())?;
            let workstream_id = workstream_id_for_work_unit(state, &work_unit_id)?;
            MainHierarchySelection::Thread {
                workstream_id,
                work_unit_id,
                thread_id: tracked_thread_id.to_string(),
            }
        }
    };
    let selected_index = rows.iter().position(|row| row == &selected)?;
    rows.iter()
        .skip(selected_index + 1)
        .chain(rows.iter().take(selected_index).rev())
        .find(|candidate| !selection_is_deleted_by_target(candidate, target))
        .cloned()
}

fn selection_is_deleted_by_target(
    selection: &MainHierarchySelection,
    target: &authority::DeleteTarget,
) -> bool {
    match (selection, target) {
        (
            MainHierarchySelection::Workstream { workstream_id },
            authority::DeleteTarget::Workstream {
                workstream_id: target_id,
            },
        ) => workstream_id == target_id.as_str(),
        (
            MainHierarchySelection::WorkUnit {
                workstream_id,
                work_unit_id,
            },
            authority::DeleteTarget::Workstream {
                workstream_id: target_id,
            },
        ) => workstream_id == target_id.as_str() || work_unit_id == target_id.as_str(),
        (
            MainHierarchySelection::Thread {
                workstream_id,
                work_unit_id,
                ..
            },
            authority::DeleteTarget::Workstream {
                workstream_id: target_id,
            },
        ) => workstream_id == target_id.as_str() || work_unit_id == target_id.as_str(),
        (
            MainHierarchySelection::WorkUnit { work_unit_id, .. },
            authority::DeleteTarget::WorkUnit {
                work_unit_id: target_id,
            },
        ) => work_unit_id == target_id.as_str(),
        (
            MainHierarchySelection::Thread {
                work_unit_id,
                thread_id,
                ..
            },
            authority::DeleteTarget::WorkUnit {
                work_unit_id: target_id,
            },
        ) => work_unit_id == target_id.as_str() || thread_id == target_id.as_str(),
        (
            MainHierarchySelection::Thread { thread_id, .. },
            authority::DeleteTarget::TrackedThread { tracked_thread_id },
        ) => thread_id == tracked_thread_id.as_str(),
        _ => false,
    }
}

fn delete_target_from_aggregate_key(
    aggregate_key: &authority::AggregateKey,
) -> Option<authority::DeleteTarget> {
    match aggregate_key.aggregate_type {
        authority::AggregateType::Workstream => Some(authority::DeleteTarget::Workstream {
            workstream_id: authority::WorkstreamId::parse(aggregate_key.aggregate_id.clone())
                .ok()?,
        }),
        authority::AggregateType::WorkUnit => Some(authority::DeleteTarget::WorkUnit {
            work_unit_id: authority::WorkUnitId::parse(aggregate_key.aggregate_id.clone()).ok()?,
        }),
        authority::AggregateType::TrackedThread => Some(authority::DeleteTarget::TrackedThread {
            tracked_thread_id: authority::TrackedThreadId::parse(
                aggregate_key.aggregate_id.clone(),
            )
            .ok()?,
        }),
    }
}

fn default_workstream_form() -> WorkstreamFooterForm {
    WorkstreamFooterForm {
        workstream_id: None,
        expected_revision: None,
        active_field: 0,
        title: FooterFieldState::new(String::new()),
        root_dir: FooterFieldState::new(String::new()),
        status: WorkstreamStatus::Active,
        priority: "normal".to_string(),
    }
}

fn default_work_unit_form(workstream_id: String) -> WorkUnitFooterForm {
    WorkUnitFooterForm {
        workstream_id,
        work_unit_id: None,
        expected_revision: None,
        active_field: 0,
        title: FooterFieldState::new(String::new()),
        task_statement: String::new(),
        status: WorkUnitStatus::Ready,
    }
}

fn default_tracked_thread_form(work_unit_id: String) -> TrackedThreadFooterForm {
    TrackedThreadFooterForm {
        work_unit_id,
        tracked_thread_id: None,
        expected_revision: None,
        active_field: 0,
        title: FooterFieldState::new(String::new()),
        root_dir: FooterFieldState::new(String::new()),
        notes: None,
        backend_kind: authority::TrackedThreadBackendKind::Codex,
        upstream_thread_id: None,
        binding_state: authority::TrackedThreadBindingState::Unbound,
        preferred_model: None,
        last_seen_turn_id: None,
    }
}

fn open_main_footer_for_edit(state: &mut AppState) -> Vec<Effect> {
    match state.main_view.selected.as_ref() {
        Some(MainHierarchySelection::Workstream { workstream_id }) => {
            let workstream = state
                .authority_main
                .workstream_details
                .get(workstream_id)
                .map(|detail| detail.workstream.clone());
            let Some(workstream) = workstream else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Workstream detail is still loading.".to_string(),
                });
                return load_selected_main_detail_if_needed(state);
            };
            state.authority_main.footer = MainFooterState::EditWorkstream(WorkstreamFooterForm {
                workstream_id: Some(workstream.id.to_string()),
                expected_revision: Some(workstream.revision),
                active_field: 0,
                title: FooterFieldState::new(workstream.title),
                root_dir: FooterFieldState::new(workstream.objective),
                status: workstream.status,
                priority: workstream.priority,
            });
        }
        Some(MainHierarchySelection::WorkUnit {
            workstream_id,
            work_unit_id,
        }) => {
            let work_unit = state
                .authority_main
                .work_unit_details
                .get(work_unit_id)
                .map(|detail| detail.work_unit.clone());
            let Some(work_unit) = work_unit else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Work unit detail is still loading.".to_string(),
                });
                return load_selected_main_detail_if_needed(state);
            };
            state.authority_main.footer = MainFooterState::EditWorkUnit(WorkUnitFooterForm {
                workstream_id: workstream_id.clone(),
                work_unit_id: Some(work_unit.id.to_string()),
                expected_revision: Some(work_unit.revision),
                active_field: 0,
                title: FooterFieldState::new(work_unit.title),
                task_statement: work_unit.task_statement,
                status: work_unit.status,
            });
        }
        Some(MainHierarchySelection::Thread {
            work_unit_id,
            thread_id,
            ..
        }) => {
            let tracked_thread = state
                .authority_main
                .tracked_thread_details
                .get(thread_id)
                .map(|detail| detail.tracked_thread.clone());
            let Some(tracked_thread) = tracked_thread else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Tracked thread detail is still loading.".to_string(),
                });
                return load_selected_main_detail_if_needed(state);
            };
            state.authority_main.footer =
                MainFooterState::EditTrackedThread(TrackedThreadFooterForm {
                    work_unit_id: work_unit_id.clone(),
                    tracked_thread_id: Some(tracked_thread.id.to_string()),
                    expected_revision: Some(tracked_thread.revision),
                    active_field: 0,
                    title: FooterFieldState::new(tracked_thread.title),
                    root_dir: FooterFieldState::new(
                        tracked_thread.preferred_cwd.unwrap_or_default(),
                    ),
                    notes: tracked_thread.notes,
                    backend_kind: tracked_thread.backend_kind,
                    upstream_thread_id: tracked_thread.upstream_thread_id,
                    binding_state: tracked_thread.binding_state,
                    preferred_model: tracked_thread.preferred_model,
                    last_seen_turn_id: tracked_thread.last_seen_turn_id,
                });
        }
        None => {
            state.banner = Some(StatusBanner {
                level: BannerLevel::Warning,
                message: "No Main hierarchy row is selected.".to_string(),
            });
        }
    }
    Vec::new()
}

fn active_main_footer_field_mut(state: &mut AppState) -> Option<&mut FooterFieldState> {
    match &mut state.authority_main.footer {
        MainFooterState::Inspect => None,
        MainFooterState::CreateWorkstream(form) | MainFooterState::EditWorkstream(form) => {
            match form.active_field {
                0 => Some(&mut form.title),
                1 => Some(&mut form.root_dir),
                _ => None,
            }
        }
        MainFooterState::CreateWorkUnit(form) | MainFooterState::EditWorkUnit(form) => {
            match form.active_field {
                0 => Some(&mut form.title),
                _ => None,
            }
        }
        MainFooterState::CreateTrackedThread(form) | MainFooterState::EditTrackedThread(form) => {
            match form.active_field {
                0 => Some(&mut form.title),
                1 => Some(&mut form.root_dir),
                _ => None,
            }
        }
        MainFooterState::ConfirmDelete(delete) => Some(&mut delete.typed_confirmation),
    }
}

fn cycle_main_footer_field(state: &mut AppState, delta: isize) {
    let field_count = match &state.authority_main.footer {
        MainFooterState::Inspect => 0,
        MainFooterState::CreateWorkstream(_) | MainFooterState::EditWorkstream(_) => 2,
        MainFooterState::CreateWorkUnit(_) | MainFooterState::EditWorkUnit(_) => 1,
        MainFooterState::CreateTrackedThread(_) | MainFooterState::EditTrackedThread(_) => 2,
        MainFooterState::ConfirmDelete(_) => 1,
    };
    if field_count <= 1 {
        return;
    }
    let next_index = |current: usize| {
        if delta.is_negative() {
            current
                .saturating_sub(delta.unsigned_abs())
                .min(field_count - 1)
        } else {
            (current + delta as usize).min(field_count - 1)
        }
    };
    match &mut state.authority_main.footer {
        MainFooterState::CreateWorkstream(form) | MainFooterState::EditWorkstream(form) => {
            form.active_field = next_index(form.active_field);
        }
        MainFooterState::CreateWorkUnit(form) | MainFooterState::EditWorkUnit(form) => {
            form.active_field = next_index(form.active_field);
        }
        MainFooterState::CreateTrackedThread(form) | MainFooterState::EditTrackedThread(form) => {
            form.active_field = next_index(form.active_field);
        }
        MainFooterState::ConfirmDelete(delete) => {
            delete.active_field = next_index(delete.active_field);
        }
        MainFooterState::Inspect => {}
    }
}

fn submit_main_footer(state: &mut AppState) -> Vec<Effect> {
    let footer = state.authority_main.footer.clone();
    match footer {
        MainFooterState::Inspect => Vec::new(),
        MainFooterState::CreateWorkstream(form) => {
            let title = form.title.value.trim().to_string();
            let root_dir = form.root_dir.value.trim().to_string();
            if title.is_empty() || root_dir.is_empty() {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Workstream title and root directory are required.".to_string(),
                });
                return Vec::new();
            }
            vec![Effect::CreateAuthorityWorkstream {
                command: authority::CreateWorkstream {
                    metadata: tui_command_metadata(),
                    workstream_id: authority::WorkstreamId::new(),
                    title,
                    objective: root_dir,
                    status: form.status,
                    priority: form.priority,
                },
            }]
        }
        MainFooterState::EditWorkstream(form) => {
            let Some(workstream_id) = form.workstream_id.clone() else {
                return Vec::new();
            };
            let Some(expected_revision) = form.expected_revision else {
                return Vec::new();
            };
            let title = form.title.value.trim().to_string();
            let root_dir = form.root_dir.value.trim().to_string();
            if title.is_empty() || root_dir.is_empty() {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Workstream title and root directory are required.".to_string(),
                });
                return Vec::new();
            }
            vec![Effect::EditAuthorityWorkstream {
                command: authority::EditWorkstream {
                    metadata: tui_command_metadata(),
                    workstream_id: authority::WorkstreamId::parse(workstream_id)
                        .expect("selection id"),
                    expected_revision,
                    changes: authority::WorkstreamPatch {
                        title: Some(title),
                        objective: Some(root_dir),
                        status: None,
                        priority: None,
                    },
                },
            }]
        }
        MainFooterState::CreateWorkUnit(form) => {
            let title = form.title.value.trim().to_string();
            if title.is_empty() {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Work unit title is required.".to_string(),
                });
                return Vec::new();
            }
            vec![Effect::CreateAuthorityWorkUnit {
                command: authority::CreateWorkUnit {
                    metadata: tui_command_metadata(),
                    work_unit_id: authority::WorkUnitId::new(),
                    workstream_id: authority::WorkstreamId::parse(form.workstream_id)
                        .expect("selected workstream id"),
                    title: title.clone(),
                    task_statement: if form.task_statement.trim().is_empty() {
                        title
                    } else {
                        form.task_statement
                    },
                    status: form.status,
                },
            }]
        }
        MainFooterState::EditWorkUnit(form) => {
            let Some(work_unit_id) = form.work_unit_id.clone() else {
                return Vec::new();
            };
            let Some(expected_revision) = form.expected_revision else {
                return Vec::new();
            };
            let title = form.title.value.trim().to_string();
            if title.is_empty() {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Work unit title is required.".to_string(),
                });
                return Vec::new();
            }
            vec![Effect::EditAuthorityWorkUnit {
                command: authority::EditWorkUnit {
                    metadata: tui_command_metadata(),
                    work_unit_id: authority::WorkUnitId::parse(work_unit_id)
                        .expect("selected work unit id"),
                    expected_revision,
                    changes: authority::WorkUnitPatch {
                        title: Some(title),
                        task_statement: None,
                        status: None,
                    },
                },
            }]
        }
        MainFooterState::CreateTrackedThread(form) => {
            let title = form.title.value.trim().to_string();
            let root_dir = form.root_dir.value.trim().to_string();
            if title.is_empty() || root_dir.is_empty() {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Tracked thread name and root directory are required.".to_string(),
                });
                return Vec::new();
            }
            vec![Effect::CreateAuthorityTrackedThread {
                command: authority::CreateTrackedThread {
                    metadata: tui_command_metadata(),
                    tracked_thread_id: authority::TrackedThreadId::new(),
                    work_unit_id: authority::WorkUnitId::parse(form.work_unit_id)
                        .expect("selected work unit id"),
                    title,
                    notes: form.notes,
                    backend_kind: form.backend_kind,
                    upstream_thread_id: form.upstream_thread_id,
                    preferred_cwd: Some(root_dir),
                    preferred_model: form.preferred_model,
                },
            }]
        }
        MainFooterState::EditTrackedThread(form) => {
            let Some(tracked_thread_id) = form.tracked_thread_id.clone() else {
                return Vec::new();
            };
            let Some(expected_revision) = form.expected_revision else {
                return Vec::new();
            };
            let title = form.title.value.trim().to_string();
            let root_dir = form.root_dir.value.trim().to_string();
            if title.is_empty() || root_dir.is_empty() {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: "Tracked thread name and root directory are required.".to_string(),
                });
                return Vec::new();
            }
            vec![Effect::EditAuthorityTrackedThread {
                command: authority::EditTrackedThread {
                    metadata: tui_command_metadata(),
                    tracked_thread_id: authority::TrackedThreadId::parse(tracked_thread_id)
                        .expect("selected tracked thread id"),
                    expected_revision,
                    changes: authority::TrackedThreadPatch {
                        title: Some(title),
                        notes: None,
                        backend_kind: None,
                        upstream_thread_id: None,
                        binding_state: None,
                        preferred_cwd: Some(Some(root_dir)),
                        preferred_model: None,
                        last_seen_turn_id: None,
                    },
                },
            }]
        }
        MainFooterState::ConfirmDelete(delete) => {
            if delete.requires_typed_confirmation
                && delete.typed_confirmation.value.trim() != delete.label
            {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Warning,
                    message: format!("Type `{}` to confirm delete.", delete.label),
                });
                return Vec::new();
            }
            match delete.target {
                authority::DeleteTarget::Workstream { workstream_id } => {
                    vec![Effect::DeleteAuthorityWorkstream {
                        command: authority::DeleteWorkstream {
                            metadata: tui_command_metadata(),
                            workstream_id,
                            expected_revision: delete.expected_revision,
                            delete_token: delete.confirmation_token,
                        },
                    }]
                }
                authority::DeleteTarget::WorkUnit { work_unit_id } => {
                    vec![Effect::DeleteAuthorityWorkUnit {
                        command: authority::DeleteWorkUnit {
                            metadata: tui_command_metadata(),
                            work_unit_id,
                            expected_revision: delete.expected_revision,
                            delete_token: delete.confirmation_token,
                        },
                    }]
                }
                authority::DeleteTarget::TrackedThread { tracked_thread_id } => {
                    vec![Effect::DeleteAuthorityTrackedThread {
                        command: authority::DeleteTrackedThread {
                            metadata: tui_command_metadata(),
                            tracked_thread_id,
                            expected_revision: delete.expected_revision,
                            delete_token: delete.confirmation_token,
                        },
                    }]
                }
            }
        }
    }
}

fn tui_command_metadata() -> authority::CommandMetadata {
    authority::CommandMetadata::new(
        authority::OriginNodeId::parse("orcas-tui").expect("static origin node id"),
        authority::CommandActor::parse("tui_operator").expect("static command actor"),
    )
}

fn footer_field_insert(field: &mut FooterFieldState, ch: char) {
    field.value.insert(field.cursor, ch);
    field.cursor += ch.len_utf8();
}

fn footer_field_backspace(field: &mut FooterFieldState) {
    if field.cursor == 0 {
        return;
    }
    let previous = previous_char_boundary(&field.value, field.cursor);
    field.value.drain(previous..field.cursor);
    field.cursor = previous;
}

fn footer_field_delete(field: &mut FooterFieldState) {
    if field.cursor >= field.value.len() {
        return;
    }
    let next = next_char_boundary(&field.value, field.cursor);
    field.value.drain(field.cursor..next);
}

fn footer_field_move_left(field: &mut FooterFieldState) {
    if field.cursor == 0 {
        return;
    }
    field.cursor = previous_char_boundary(&field.value, field.cursor);
}

fn footer_field_move_right(field: &mut FooterFieldState) {
    if field.cursor >= field.value.len() {
        return;
    }
    field.cursor = next_char_boundary(&field.value, field.cursor);
}

fn first_authority_work_unit_for_workstream<'a>(
    state: &'a AppState,
    workstream_id: &str,
) -> Option<&'a authority::WorkUnitNode> {
    state
        .authority_main
        .hierarchy
        .workstreams
        .iter()
        .find_map(|workstream| {
            (workstream.workstream.id.as_str() == workstream_id)
                .then(|| workstream.work_units.first())
                .flatten()
        })
}

fn first_upstream_thread_for_work_unit(state: &AppState, work_unit_id: &str) -> Option<String> {
    state
        .authority_main
        .hierarchy
        .workstreams
        .iter()
        .find_map(|workstream| {
            workstream.work_units.iter().find_map(|work_unit| {
                (work_unit.work_unit.id.as_str() == work_unit_id)
                    .then(|| {
                        work_unit
                            .tracked_threads
                            .iter()
                            .find_map(|tracked_thread| tracked_thread.upstream_thread_id.clone())
                    })
                    .flatten()
            })
        })
}

fn tracked_thread_upstream_binding(state: &AppState, tracked_thread_id: &str) -> Option<String> {
    state
        .authority_main
        .tracked_thread_details
        .get(tracked_thread_id)
        .and_then(|detail| detail.tracked_thread.upstream_thread_id.clone())
        .or_else(|| {
            state
                .authority_main
                .hierarchy
                .workstreams
                .iter()
                .find_map(|workstream| {
                    workstream.work_units.iter().find_map(|work_unit| {
                        work_unit.tracked_threads.iter().find_map(|tracked_thread| {
                            (tracked_thread.id.as_str() == tracked_thread_id)
                                .then(|| tracked_thread.upstream_thread_id.clone())
                                .flatten()
                        })
                    })
                })
        })
}

fn workstream_id_for_work_unit(state: &AppState, work_unit_id: &str) -> Option<String> {
    state
        .authority_main
        .hierarchy
        .workstreams
        .iter()
        .find_map(|workstream| {
            workstream
                .work_units
                .iter()
                .any(|work_unit| work_unit.work_unit.id.as_str() == work_unit_id)
                .then(|| workstream.workstream.id.to_string())
        })
}

fn work_unit_id_for_tracked_thread(state: &AppState, tracked_thread_id: &str) -> Option<String> {
    state
        .authority_main
        .hierarchy
        .workstreams
        .iter()
        .find_map(|workstream| {
            workstream.work_units.iter().find_map(|work_unit| {
                work_unit
                    .tracked_threads
                    .iter()
                    .any(|tracked_thread| tracked_thread.id.as_str() == tracked_thread_id)
                    .then(|| work_unit.work_unit.id.to_string())
            })
        })
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

fn invalidate_authority_view_state(state: &mut AppState, preserve_selection: bool) {
    if preserve_selection && state.main_view.pending_selection.is_none() {
        // Preserve intent, not stale data. The cached hierarchy/detail panes are
        // always cleared so a reconnect/delete boundary can rebuild them fresh.
        state.main_view.pending_selection = state.main_view.selected.clone();
    }
    state.authority_main.hierarchy = authority::HierarchySnapshot::default();
    state.authority_main.workstream_details.clear();
    state.authority_main.work_unit_details.clear();
    state.authority_main.tracked_thread_details.clear();
    state.authority_main.footer = MainFooterState::Inspect;
    reconcile_main_view(state);
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

fn selected_review_pending_supervisor_decision(
    state: &AppState,
) -> Option<&ipc::SupervisorTurnDecisionSummary> {
    if state.current_view != TopLevelView::Overview
        || state.main_view.program_view != ProgramView::Review
    {
        return None;
    }

    let ReviewSelection::Decision { decision_id } = state.review_view.selected.as_ref()? else {
        return None;
    };

    state
        .collaboration
        .supervisor_turn_decisions
        .iter()
        .find(|decision| {
            decision.decision_id == *decision_id
                && decision.status == orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman
        })
}

fn selected_supervisor_decision_id_for_review_action(state: &AppState) -> Option<String> {
    if state.current_view == TopLevelView::Overview
        && state.main_view.program_view == ProgramView::Review
    {
        return selected_review_pending_supervisor_decision(state)
            .map(|decision| decision.decision_id.clone());
    }

    selected_thread_pending_supervisor_decision(state).map(|decision| decision.decision_id.clone())
}

fn selected_supervisor_decision_action_unavailable_message(state: &AppState) -> String {
    if state.current_view == TopLevelView::Overview
        && state.main_view.program_view == ProgramView::Review
    {
        return match state.review_view.selected.as_ref() {
            Some(ReviewSelection::Decision { .. }) => {
                "Selected review decision is not awaiting human approval.".to_string()
            }
            Some(_) => "Selected review item has no approve/reject action yet.".to_string(),
            None => "No review item selected.".to_string(),
        };
    }

    "Selected thread has no pending supervisor decision.".to_string()
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
        UiEvent::AuthorityHierarchyLoaded(_) => return None,
        UiEvent::AuthorityWorkstreamDetailLoaded(_)
        | UiEvent::AuthorityWorkUnitDetailLoaded(_)
        | UiEvent::AuthorityTrackedThreadDetailLoaded(_)
        | UiEvent::AuthorityDeletePlanLoaded(_) => return None,
        UiEvent::WorkUnitDetailLoaded(_)
        | UiEvent::ProposalArtifactSummaryListLoaded(_)
        | UiEvent::ProposalArtifactSummaryListLoadFailed { .. }
        | UiEvent::ProposalArtifactSummaryLoaded(_)
        | UiEvent::ProposalArtifactSummaryLoadFailed { .. }
        | UiEvent::ProposalArtifactDetailLoaded(_)
        | UiEvent::ProposalArtifactDetailLoadFailed { .. }
        | UiEvent::ProposalArtifactExported { .. }
        | UiEvent::ProposalArtifactExportFailed { .. } => return None,
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
        UiEvent::AuthorityWorkstreamCreated(workstream) => (
            "authority_workstream",
            format!("created workstream {}", workstream.id),
            None,
            None,
        ),
        UiEvent::AuthorityWorkstreamEdited(workstream) => (
            "authority_workstream",
            format!("edited workstream {}", workstream.id),
            None,
            None,
        ),
        UiEvent::AuthorityWorkstreamDeleted(workstream) => (
            "authority_workstream",
            format!("deleted workstream {}", workstream.id),
            None,
            None,
        ),
        UiEvent::AuthorityWorkUnitCreated(work_unit) => (
            "authority_work_unit",
            format!("created work unit {}", work_unit.id),
            None,
            None,
        ),
        UiEvent::AuthorityWorkUnitEdited(work_unit) => (
            "authority_work_unit",
            format!("edited work unit {}", work_unit.id),
            None,
            None,
        ),
        UiEvent::AuthorityWorkUnitDeleted(work_unit) => (
            "authority_work_unit",
            format!("deleted work unit {}", work_unit.id),
            None,
            None,
        ),
        UiEvent::AuthorityTrackedThreadCreated(tracked_thread) => (
            "authority_tracked_thread",
            format!("created tracked thread {}", tracked_thread.id),
            tracked_thread.upstream_thread_id.clone(),
            None,
        ),
        UiEvent::AuthorityTrackedThreadEdited(tracked_thread) => (
            "authority_tracked_thread",
            format!("edited tracked thread {}", tracked_thread.id),
            tracked_thread.upstream_thread_id.clone(),
            None,
        ),
        UiEvent::AuthorityTrackedThreadDeleted(tracked_thread) => (
            "authority_tracked_thread",
            format!("deleted tracked thread {}", tracked_thread.id),
            tracked_thread.upstream_thread_id.clone(),
            None,
        ),
        UiEvent::TrackedThreadLifecycle {
            action,
            tracked_thread,
        } => (
            "tracked_thread",
            format!(
                "tracked thread {} {}",
                tracked_thread.id,
                collaboration_action_label(*action)
            ),
            tracked_thread.upstream_thread_id.clone(),
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
        ipc::CollaborationLifecycleAction::Deleted => "deleted",
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
