use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::collaboration::{
    Assignment, AssignmentStatus, Decision, DecisionType, Report, ReportConfidence,
    ReportDisposition, ReportParseResult, WorkUnit, WorkUnitStatus, Worker, WorkerSession,
    Workstream, WorkstreamStatus,
};
use crate::communication::AssignmentCommunicationRecord;
use crate::events::ConnectionState;
use crate::supervisor::{
    SupervisorProposalEdits, SupervisorProposalFailureStage, SupervisorProposalRecord,
    SupervisorProposalStatus,
};

pub mod methods {
    pub const DAEMON_STATUS: &str = "daemon/status";
    pub const DAEMON_CONNECT: &str = "daemon/connect";
    pub const DAEMON_STOP: &str = "daemon/stop";
    pub const DAEMON_DISCONNECT: &str = "daemon/disconnect";
    pub const STATE_GET: &str = "state/get";
    pub const SESSION_GET_ACTIVE: &str = "session/get_active";
    pub const MODELS_LIST: &str = "models/list";
    pub const THREADS_LIST: &str = "threads/list";
    pub const THREADS_LIST_SCOPED: &str = "threads/list_scoped";
    pub const THREAD_START: &str = "thread/start";
    pub const THREAD_READ: &str = "thread/read";
    pub const THREAD_GET: &str = "thread/get";
    pub const THREAD_RESUME: &str = "thread/resume";
    pub const TURNS_LIST_ACTIVE: &str = "turns/list_active";
    pub const TURNS_RECENT: &str = "turns/recent";
    pub const TURN_GET: &str = "turn/get";
    pub const TURN_ATTACH: &str = "turn/attach";
    pub const TURN_START: &str = "turn/start";
    pub const TURN_INTERRUPT: &str = "turn/interrupt";
    pub const WORKSTREAM_CREATE: &str = "workstream/create";
    pub const WORKSTREAM_LIST: &str = "workstream/list";
    pub const WORKSTREAM_GET: &str = "workstream/get";
    pub const WORKUNIT_CREATE: &str = "workunit/create";
    pub const WORKUNIT_LIST: &str = "workunit/list";
    pub const WORKUNIT_GET: &str = "workunit/get";
    pub const ASSIGNMENT_START: &str = "assignment/start";
    pub const ASSIGNMENT_GET: &str = "assignment/get";
    pub const ASSIGNMENT_COMMUNICATION_GET: &str = "assignment_communication/get";
    pub const REPORT_GET: &str = "report/get";
    pub const REPORT_LIST_FOR_WORKUNIT: &str = "report/list_for_workunit";
    pub const DECISION_APPLY: &str = "decision/apply";
    pub const PROPOSAL_CREATE: &str = "proposal/create";
    pub const PROPOSAL_GET: &str = "proposal/get";
    pub const PROPOSAL_LIST_FOR_WORKUNIT: &str = "proposal/list_for_workunit";
    pub const PROPOSAL_APPROVE: &str = "proposal/approve";
    pub const PROPOSAL_REJECT: &str = "proposal/reject";
    pub const EVENTS_SUBSCRIBE: &str = "events/subscribe";
    pub const EVENTS_NOTIFICATION: &str = "events/notification";
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Empty {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatusResponse {
    pub socket_path: String,
    pub metadata_path: String,
    pub codex_endpoint: String,
    pub codex_binary_path: String,
    pub upstream: ConnectionState,
    pub client_count: usize,
    pub known_threads: usize,
    pub runtime: DaemonRuntimeMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonRuntimeMetadata {
    pub pid: u32,
    pub started_at: DateTime<Utc>,
    pub version: String,
    pub build_fingerprint: String,
    pub binary_path: String,
    pub socket_path: String,
    pub metadata_path: String,
    pub git_commit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonBinarySummary {
    pub version: String,
    pub build_fingerprint: String,
    pub binary_path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DaemonConnectRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConnectResponse {
    pub status: DaemonStatusResponse,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DaemonStopRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStopResponse {
    pub stopping: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateGetRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateGetResponse {
    pub snapshot: StateSnapshot,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionGetActiveRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionGetActiveResponse {
    pub session: SessionState,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventsSubscribeRequest {
    pub include_snapshot: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventsSubscribeResponse {
    pub subscribed: bool,
    pub snapshot: Option<StateSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventsNotification {
    pub event: DaemonEventEnvelope,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub daemon: DaemonStatusResponse,
    pub session: SessionState,
    pub threads: Vec<ThreadSummary>,
    pub active_thread: Option<ThreadView>,
    #[serde(default)]
    pub collaboration: CollaborationSnapshot,
    #[serde(default)]
    pub recent_events: Vec<EventSummary>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CollaborationSnapshot {
    pub workstreams: Vec<WorkstreamSummary>,
    pub work_units: Vec<WorkUnitSummary>,
    pub assignments: Vec<AssignmentSummary>,
    pub reports: Vec<ReportSummary>,
    pub decisions: Vec<DecisionSummary>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionState {
    pub active_thread_id: Option<String>,
    pub active_turns: Vec<ActiveTurn>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveTurn {
    pub thread_id: String,
    pub turn_id: String,
    pub status: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSummary {
    pub timestamp: DateTime<Utc>,
    pub kind: String,
    pub message: String,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonEventEnvelope {
    pub emitted_at: DateTime<Utc>,
    pub event: DaemonEvent,
}

impl DaemonEventEnvelope {
    pub fn new(event: DaemonEvent) -> Self {
        Self {
            emitted_at: Utc::now(),
            event,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonEvent {
    UpstreamStatusChanged {
        upstream: ConnectionState,
    },
    SessionChanged {
        session: SessionState,
    },
    ThreadUpdated {
        thread: ThreadSummary,
    },
    TurnUpdated {
        thread_id: String,
        turn: TurnView,
    },
    ItemUpdated {
        thread_id: String,
        turn_id: String,
        item: ItemView,
    },
    OutputDelta {
        thread_id: String,
        turn_id: String,
        item_id: String,
        delta: String,
    },
    WorkstreamLifecycle {
        action: CollaborationLifecycleAction,
        workstream: WorkstreamSummary,
    },
    WorkUnitLifecycle {
        action: CollaborationLifecycleAction,
        work_unit: WorkUnitSummary,
    },
    AssignmentLifecycle {
        action: AssignmentLifecycleAction,
        assignment: AssignmentSummary,
    },
    ReportRecorded {
        report: ReportSummary,
    },
    DecisionApplied {
        decision: DecisionSummary,
    },
    ProposalLifecycle {
        action: ProposalLifecycleAction,
        proposal: ProposalSummary,
        work_unit: WorkUnitSummary,
    },
    Warning {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationLifecycleAction {
    Created,
    Updated,
    Completed,
    Escalated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentLifecycleAction {
    Created,
    Started,
    Reported,
    Closed,
    Interrupted,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalLifecycleAction {
    Created,
    GenerationFailed,
    Approved,
    Rejected,
    Superseded,
    Stale,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkstreamSummary {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub objective: String,
    pub status: WorkstreamStatus,
    pub priority: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkUnitSummary {
    pub id: String,
    pub workstream_id: String,
    pub title: String,
    pub status: WorkUnitStatus,
    pub dependency_count: usize,
    pub current_assignment_id: Option<String>,
    pub latest_report_id: Option<String>,
    #[serde(default)]
    pub proposal: Option<WorkUnitProposalSummary>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkUnitProposalSummary {
    pub latest_proposal_id: String,
    pub latest_status: SupervisorProposalStatus,
    pub latest_proposed_decision_type: Option<DecisionType>,
    pub latest_created_at: DateTime<Utc>,
    pub latest_reviewed_at: Option<DateTime<Utc>>,
    pub latest_has_approval_edits: bool,
    pub latest_failure_stage: Option<SupervisorProposalFailureStage>,
    pub has_open_proposal: bool,
    pub open_proposal_id: Option<String>,
    pub open_proposed_decision_type: Option<DecisionType>,
    pub has_generation_failed: bool,
    pub has_stale_or_superseded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentSummary {
    pub id: String,
    pub work_unit_id: String,
    pub worker_id: String,
    pub worker_session_id: String,
    pub status: AssignmentStatus,
    pub attempt_number: u32,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportSummary {
    pub id: String,
    pub work_unit_id: String,
    pub assignment_id: String,
    pub worker_id: String,
    pub disposition: ReportDisposition,
    pub summary: String,
    pub confidence: ReportConfidence,
    pub parse_result: ReportParseResult,
    pub needs_supervisor_review: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionSummary {
    pub id: String,
    pub work_unit_id: String,
    pub report_id: Option<String>,
    pub decision_type: DecisionType,
    #[serde(default)]
    pub rationale: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalSummary {
    pub id: String,
    pub primary_work_unit_id: String,
    pub source_report_id: String,
    pub status: SupervisorProposalStatus,
    pub proposed_decision_type: Option<DecisionType>,
    pub created_at: DateTime<Utc>,
    pub reviewed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub has_approval_edits: bool,
    pub generation_failure_stage: Option<SupervisorProposalFailureStage>,
    pub reasoner_model: String,
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;

    use super::{DecisionSummary, StateSnapshot};
    use crate::{DecisionType, WorkstreamStatus};

    #[test]
    fn state_snapshot_deserializes_when_collaboration_is_missing() {
        let snapshot = serde_json::from_value::<StateSnapshot>(json!({
            "daemon": {
                "socket_path": "/tmp/orcasd.sock",
                "metadata_path": "/tmp/orcasd.json",
                "codex_endpoint": "ws://127.0.0.1:4500",
                "codex_binary_path": "/tmp/codex",
                "upstream": {
                    "endpoint": "ws://127.0.0.1:4500",
                    "status": "connected",
                    "detail": null
                },
                "client_count": 1,
                "known_threads": 0,
                "runtime": {
                    "pid": 4242,
                    "started_at": Utc::now(),
                    "version": "0.1.0",
                    "build_fingerprint": "abc123",
                    "binary_path": "/tmp/orcasd",
                    "socket_path": "/tmp/orcasd.sock",
                    "metadata_path": "/tmp/orcasd.json",
                    "git_commit": null
                }
            },
            "session": {
                "active_thread_id": null,
                "active_turns": []
            },
            "threads": [],
            "active_thread": null,
            "recent_events": []
        }))
        .expect("legacy snapshot should deserialize");

        assert!(snapshot.collaboration.workstreams.is_empty());
        assert!(snapshot.collaboration.work_units.is_empty());
        assert!(snapshot.collaboration.assignments.is_empty());
    }

    #[test]
    fn summary_defaults_cover_missing_additive_fields() {
        let workstream = serde_json::from_value::<super::WorkstreamSummary>(json!({
            "id": "ws-1",
            "title": "Legacy",
            "status": WorkstreamStatus::Active,
            "priority": "high",
            "updated_at": Utc::now()
        }))
        .expect("legacy workstream summary should deserialize");
        assert!(workstream.objective.is_empty());

        let decision = serde_json::from_value::<DecisionSummary>(json!({
            "id": "decision-1",
            "work_unit_id": "wu-1",
            "report_id": null,
            "decision_type": DecisionType::Continue,
            "created_at": Utc::now()
        }))
        .expect("legacy decision summary should deserialize");
        assert!(decision.rationale.is_empty());

        let work_unit = serde_json::from_value::<super::WorkUnitSummary>(json!({
            "id": "wu-1",
            "workstream_id": "ws-1",
            "title": "Legacy work unit",
            "status": "ready",
            "dependency_count": 0,
            "current_assignment_id": null,
            "latest_report_id": null,
            "updated_at": Utc::now()
        }))
        .expect("legacy work unit summary should deserialize");
        assert!(work_unit.proposal.is_none());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsListResponse {
    pub data: Vec<ModelSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSummary {
    pub id: String,
    pub display_name: String,
    pub hidden: bool,
    pub is_default: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThreadsListRequest {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThreadsListScopedRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadsListResponse {
    pub data: Vec<ThreadSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadSummary {
    pub id: String,
    pub preview: String,
    pub name: Option<String>,
    pub model_provider: String,
    pub cwd: String,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub scope: String,
    pub recent_output: Option<String>,
    pub recent_event: Option<String>,
    pub turn_in_flight: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadView {
    pub summary: ThreadSummary,
    pub turns: Vec<TurnView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnView {
    pub id: String,
    pub status: String,
    pub error_message: Option<String>,
    pub items: Vec<ItemView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnLifecycleState {
    Active,
    Completed,
    Failed,
    Interrupted,
    Lost,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnStateView {
    pub thread_id: String,
    pub turn_id: String,
    pub lifecycle: TurnLifecycleState,
    pub status: String,
    pub attachable: bool,
    pub live_stream: bool,
    pub terminal: bool,
    pub recent_output: Option<String>,
    pub recent_event: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TurnsListActiveRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnsListActiveResponse {
    pub turns: Vec<TurnStateView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemView {
    pub id: String,
    pub item_type: String,
    pub status: Option<String>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadStartRequest {
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub ephemeral: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadStartResponse {
    pub thread: ThreadSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadResumeRequest {
    pub thread_id: String,
    pub cwd: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadResumeResponse {
    pub thread: ThreadSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadReadRequest {
    pub thread_id: String,
    pub include_turns: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadReadResponse {
    pub thread: ThreadView,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadGetRequest {
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadGetResponse {
    pub thread: ThreadView,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnsRecentRequest {
    pub thread_id: String,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnsRecentResponse {
    pub thread_id: String,
    pub turns: Vec<TurnView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnGetRequest {
    pub thread_id: String,
    pub turn_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnGetResponse {
    pub turn: Option<TurnStateView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnAttachRequest {
    pub thread_id: String,
    pub turn_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnAttachResponse {
    pub turn: Option<TurnStateView>,
    pub attached: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnStartRequest {
    pub thread_id: String,
    pub text: String,
    pub cwd: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnStartResponse {
    pub turn_id: String,
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnInterruptRequest {
    pub thread_id: String,
    pub turn_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkstreamCreateRequest {
    pub title: String,
    pub objective: String,
    pub priority: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkstreamCreateResponse {
    pub workstream: Workstream,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkstreamListRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkstreamListResponse {
    pub workstreams: Vec<Workstream>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkstreamGetRequest {
    pub workstream_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkstreamGetResponse {
    pub workstream: Workstream,
    pub work_units: Vec<WorkUnit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkunitCreateRequest {
    pub workstream_id: String,
    pub title: String,
    pub task_statement: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkunitCreateResponse {
    pub work_unit: WorkUnit,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkunitListRequest {
    pub workstream_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkunitListResponse {
    pub work_units: Vec<WorkUnit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkunitGetRequest {
    pub work_unit_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkunitGetResponse {
    pub work_unit: WorkUnit,
    pub assignments: Vec<Assignment>,
    pub reports: Vec<Report>,
    pub decisions: Vec<Decision>,
    #[serde(default)]
    pub proposals: Vec<SupervisorProposalRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentStartRequest {
    pub work_unit_id: String,
    pub worker_id: String,
    pub worker_kind: Option<String>,
    pub instructions: Option<String>,
    pub model: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentStartResponse {
    pub assignment: Assignment,
    pub worker: Worker,
    pub worker_session: WorkerSession,
    pub report: Report,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentGetRequest {
    pub assignment_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentGetResponse {
    pub assignment: Assignment,
    pub worker: Worker,
    pub worker_session: WorkerSession,
    pub report: Option<Report>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentCommunicationGetRequest {
    pub assignment_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentCommunicationGetResponse {
    pub record: AssignmentCommunicationRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportGetRequest {
    pub report_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportGetResponse {
    pub report: Report,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportListForWorkunitRequest {
    pub work_unit_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportListForWorkunitResponse {
    pub reports: Vec<Report>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionApplyRequest {
    pub work_unit_id: String,
    pub report_id: Option<String>,
    pub decision_type: DecisionType,
    pub rationale: String,
    pub instructions: Option<String>,
    pub worker_id: Option<String>,
    pub worker_kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionApplyResponse {
    pub decision: Decision,
    pub work_unit: WorkUnit,
    pub next_assignment: Option<Assignment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalCreateRequest {
    pub work_unit_id: String,
    pub source_report_id: Option<String>,
    pub requested_by: Option<String>,
    pub note: Option<String>,
    #[serde(default)]
    pub supersede_open: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalCreateResponse {
    pub proposal: SupervisorProposalRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalGetRequest {
    pub proposal_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalGetResponse {
    pub proposal: SupervisorProposalRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalListForWorkunitRequest {
    pub work_unit_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalListForWorkunitResponse {
    pub proposals: Vec<ProposalSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalApproveRequest {
    pub proposal_id: String,
    pub reviewed_by: Option<String>,
    pub review_note: Option<String>,
    #[serde(default)]
    pub edits: SupervisorProposalEdits,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalApproveResponse {
    pub proposal: SupervisorProposalRecord,
    pub decision: Decision,
    pub next_assignment: Option<Assignment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalRejectRequest {
    pub proposal_id: String,
    pub reviewed_by: Option<String>,
    pub review_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalRejectResponse {
    pub proposal: SupervisorProposalRecord,
}
