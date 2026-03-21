//! Shared JSON-RPC, snapshot, and event vocabulary for ORCAS.
//!
//! This module defines the public wire contract used by the daemon, CLI, and
//! operator client. It intentionally separates three classes of surface:
//! - snapshot and recovery reads such as `state/get` and `events/subscribe`
//! - canonical authority planning reads and writes under `authority/*`
//! - retained runtime-detail exceptions such as `workunit/get`
//!
//! `state/get` is collaboration-first and should not be read as the canonical
//! planning hierarchy surface. `authority/hierarchy/get` and the related
//! `authority/*` methods are the canonical planning reads and writes.
//! `DaemonEvent` is a visibility stream, not replay/history truth.
//!
//! Read this alongside `authority.rs` for canonical planning records and
//! `collaboration.rs` for the daemon-owned execution/runtime model.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::form_urlencoded;
use std::collections::BTreeMap;

use crate::authority;
use crate::collaboration::{
    Assignment, AssignmentStatus, CodexThreadAssignment, CodexThreadAssignmentStatus,
    CodexThreadBootstrapState, CodexThreadSendPolicy, Decision, DecisionType,
    LandingAuthorizationRecord, LandingExecutionRecord, PlanningSession,
    PlanningSessionResearchStatus, PlanningSessionStructuredSummary, Report, ReportConfidence,
    ReportDisposition, ReportParseResult, SupervisorTurnDecision, SupervisorTurnDecisionKind,
    SupervisorTurnDecisionStatus, SupervisorTurnProposalKind, WorkUnit, WorkUnitStatus, Worker,
    WorkerSession, WorkspaceOperationRecord, Workstream, WorkstreamStatus,
};
use crate::communication::{AssignmentCommunicationRecord, TrackedThreadPruneWorkspaceResult};
use crate::events::ConnectionState;
use crate::planning::{PlanAssessment, PlanRevisionProposal, PlanningState, WorkstreamPlan};
use crate::supervisor::{
    SupervisorPromptRenderArtifact, SupervisorProposal, SupervisorProposalEdits,
    SupervisorProposalFailure, SupervisorProposalFailureStage, SupervisorProposalRecord,
    SupervisorProposalStatus, SupervisorResponseArtifact,
};

/// JSON-RPC method names grouped by public surface family.
///
/// The family boundaries matter more than the individual names: `state/get`
/// and `events/subscribe` are recovery-oriented collaboration surfaces,
/// `authority/*` and `authority/hierarchy/get` are canonical planning surfaces,
/// and `workunit/get` is a retained execution-detail exception.
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
    pub const THREADS_LIST_LOADED: &str = "threads/list_loaded";
    pub const THREAD_START: &str = "thread/start";
    pub const THREAD_READ: &str = "thread/read";
    pub const THREAD_READ_HISTORY: &str = "thread/read_history";
    pub const THREAD_GET: &str = "thread/get";
    pub const THREAD_ATTACH: &str = "thread/attach";
    pub const THREAD_DETACH: &str = "thread/detach";
    pub const THREAD_RESUME: &str = "thread/resume";
    pub const TURNS_LIST_ACTIVE: &str = "turns/list_active";
    pub const TURNS_RECENT: &str = "turns/recent";
    pub const TURN_GET: &str = "turn/get";
    pub const TURN_ATTACH: &str = "turn/attach";
    pub const TURN_START: &str = "turn/start";
    pub const TURN_STEER: &str = "turn/steer";
    pub const TURN_INTERRUPT: &str = "turn/interrupt";
    pub const WORKUNIT_GET: &str = "workunit/get";
    pub const AUTHORITY_HIERARCHY_GET: &str = "authority/hierarchy/get";
    pub const AUTHORITY_DELETE_PLAN: &str = "authority/delete/plan";
    pub const AUTHORITY_WORKSTREAM_CREATE: &str = "authority/workstream/create";
    pub const AUTHORITY_WORKSTREAM_EDIT: &str = "authority/workstream/edit";
    pub const AUTHORITY_WORKSTREAM_DELETE: &str = "authority/workstream/delete";
    pub const AUTHORITY_WORKSTREAM_LIST: &str = "authority/workstream/list";
    pub const AUTHORITY_WORKSTREAM_GET: &str = "authority/workstream/get";
    pub const AUTHORITY_WORKUNIT_CREATE: &str = "authority/workunit/create";
    pub const AUTHORITY_WORKUNIT_EDIT: &str = "authority/workunit/edit";
    pub const AUTHORITY_WORKUNIT_DELETE: &str = "authority/workunit/delete";
    pub const AUTHORITY_WORKUNIT_LIST: &str = "authority/workunit/list";
    pub const AUTHORITY_WORKUNIT_GET: &str = "authority/workunit/get";
    pub const AUTHORITY_TRACKED_THREAD_CREATE: &str = "authority/tracked_thread/create";
    pub const AUTHORITY_TRACKED_THREAD_EDIT: &str = "authority/tracked_thread/edit";
    pub const AUTHORITY_TRACKED_THREAD_DELETE: &str = "authority/tracked_thread/delete";
    pub const AUTHORITY_TRACKED_THREAD_LIST: &str = "authority/tracked_thread/list";
    pub const AUTHORITY_TRACKED_THREAD_GET: &str = "authority/tracked_thread/get";
    pub const AUTHORITY_EVENTS_EXPORT: &str = "authority/events/export";
    pub const AUTHORITY_EVENTS_ACK: &str = "authority/events/ack";
    pub const AUTHORITY_EVENTS_REPLAY: &str = "authority/events/replay";
    pub const OPERATOR_INBOX_LIST: &str = "operator_inbox/list";
    pub const OPERATOR_INBOX_GET: &str = "operator_inbox/get";
    pub const OPERATOR_INBOX_CHECKPOINT: &str = "operator_inbox/checkpoint";
    pub const OPERATOR_INBOX_CHANGES: &str = "operator_inbox/changes";
    pub const OPERATOR_INBOX_ACTION_ROUTE: &str = "operator_inbox/action_route";
    pub const OPERATOR_INBOX_WAIT_FOR_CHECKPOINT: &str = "operator_inbox/wait_for_checkpoint";
    pub const OPERATOR_INBOX_REPLAY: &str = "operator_inbox/replay";
    pub const OPERATOR_INBOX_EXPORT: &str = "operator_inbox/export";
    pub const OPERATOR_INBOX_ACK: &str = "operator_inbox/ack";
    pub const OPERATOR_INBOX_MIRROR_CHECKPOINT: &str = "operator_inbox/mirror_checkpoint";
    pub const OPERATOR_NOTIFICATION_LIST: &str = "operator_notification/list";
    pub const OPERATOR_NOTIFICATION_GET: &str = "operator_notification/get";
    pub const OPERATOR_NOTIFICATION_ACK: &str = "operator_notification/ack";
    pub const OPERATOR_NOTIFICATION_SUPPRESS: &str = "operator_notification/suppress";
    pub const OPERATOR_NOTIFICATION_RECIPIENT_UPSERT: &str =
        "operator_notification/recipient/upsert";
    pub const OPERATOR_NOTIFICATION_RECIPIENT_LIST: &str = "operator_notification/recipient/list";
    pub const OPERATOR_NOTIFICATION_SUBSCRIPTION_UPSERT: &str =
        "operator_notification/subscription/upsert";
    pub const OPERATOR_NOTIFICATION_SUBSCRIPTION_LIST: &str =
        "operator_notification/subscription/list";
    pub const OPERATOR_NOTIFICATION_SUBSCRIPTION_SET_ENABLED: &str =
        "operator_notification/subscription/set_enabled";
    pub const OPERATOR_NOTIFICATION_DELIVERY_JOB_LIST: &str =
        "operator_notification/delivery_job/list";
    pub const OPERATOR_NOTIFICATION_DELIVERY_JOB_GET: &str =
        "operator_notification/delivery_job/get";
    pub const OPERATOR_NOTIFICATION_DELIVERY_RUN_PENDING: &str =
        "operator_notification/delivery/run_pending";
    pub const WORKSTREAM_PLAN_GET: &str = "workstream_plan/get";
    pub const WORKSTREAM_PLAN_LIST: &str = "workstream_plan/list";
    pub const PLAN_ASSESSMENT_LIST: &str = "plan_assessment/list";
    pub const PLAN_REVISION_PROPOSAL_LIST: &str = "plan_revision_proposal/list";
    pub const PLANNING_SESSION_CREATE: &str = "planning_session/create";
    pub const PLANNING_SESSION_GET: &str = "planning_session/get";
    pub const PLANNING_SESSION_LIST: &str = "planning_session/list";
    pub const PLANNING_SESSION_UPDATE_SUMMARY: &str = "planning_session/update_summary";
    pub const PLANNING_SESSION_REQUEST_SUPERVISOR_CONTEXT: &str =
        "planning_session/request_supervisor_context";
    pub const PLANNING_SESSION_REQUEST_RESEARCH: &str = "planning_session/request_research";
    pub const PLANNING_SESSION_MARK_READY_FOR_REVIEW: &str =
        "planning_session/mark_ready_for_review";
    pub const PLANNING_SESSION_ABORT: &str = "planning_session/abort";
    pub const PLANNING_SESSION_APPROVE: &str = "planning_session/approve";
    pub const PLANNING_SESSION_REJECT: &str = "planning_session/reject";
    pub const PLANNING_SESSION_SUPERSEDE: &str = "planning_session/supersede";
    pub const AUTHORITY_TRACKED_THREAD_PREPARE_WORKSPACE: &str =
        "authority/tracked_thread/prepare_workspace";
    pub const AUTHORITY_TRACKED_THREAD_REFRESH_WORKSPACE: &str =
        "authority/tracked_thread/refresh_workspace";
    pub const AUTHORITY_TRACKED_THREAD_MERGE_PREP: &str = "authority/tracked_thread/merge_prep";
    pub const AUTHORITY_TRACKED_THREAD_AUTHORIZE_MERGE: &str =
        "authority/tracked_thread/authorize_merge";
    pub const AUTHORITY_TRACKED_THREAD_EXECUTE_LANDING: &str =
        "authority/tracked_thread/execute_landing";
    pub const AUTHORITY_TRACKED_THREAD_PRUNE_WORKSPACE: &str =
        "authority/tracked_thread/prune_workspace";
    pub const ASSIGNMENT_START: &str = "assignment/start";
    pub const ASSIGNMENT_GET: &str = "assignment/get";
    pub const ASSIGNMENT_COMMUNICATION_GET: &str = "assignment_communication/get";
    pub const CODEX_ASSIGNMENT_CREATE: &str = "codex_assignment/create";
    pub const CODEX_ASSIGNMENT_GET: &str = "codex_assignment/get";
    pub const CODEX_ASSIGNMENT_LIST: &str = "codex_assignment/list";
    pub const CODEX_ASSIGNMENT_PAUSE: &str = "codex_assignment/pause";
    pub const CODEX_ASSIGNMENT_RESUME: &str = "codex_assignment/resume";
    pub const CODEX_ASSIGNMENT_RELEASE: &str = "codex_assignment/release";
    pub const SUPERVISOR_DECISION_LIST: &str = "supervisor_decision/list";
    pub const SUPERVISOR_DECISION_GET: &str = "supervisor_decision/get";
    pub const SUPERVISOR_DECISION_PROPOSE_STEER: &str = "supervisor_decision/propose_steer";
    pub const SUPERVISOR_DECISION_REPLACE_PENDING_STEER: &str =
        "supervisor_decision/replace_pending_steer";
    pub const SUPERVISOR_DECISION_PROPOSE_INTERRUPT: &str = "supervisor_decision/propose_interrupt";
    pub const SUPERVISOR_DECISION_RECORD_NO_ACTION: &str = "supervisor_decision/record_no_action";
    pub const SUPERVISOR_DECISION_MANUAL_REFRESH: &str = "supervisor_decision/manual_refresh";
    pub const SUPERVISOR_DECISION_APPROVE_AND_SEND: &str = "supervisor_decision/approve_and_send";
    pub const SUPERVISOR_DECISION_REJECT: &str = "supervisor_decision/reject";
    pub const REPORT_GET: &str = "report/get";
    pub const REPORT_LIST_FOR_WORKUNIT: &str = "report/list_for_workunit";
    pub const DECISION_APPLY: &str = "decision/apply";
    pub const PROPOSAL_CREATE: &str = "proposal/create";
    pub const PROPOSAL_GET: &str = "proposal/get";
    pub const PROPOSAL_ARTIFACT_SUMMARY_GET: &str = "proposal/artifact_summary/get";
    pub const PROPOSAL_ARTIFACT_DETAIL_GET: &str = "proposal/artifact_detail/get";
    pub const PROPOSAL_ARTIFACT_EXPORT_GET: &str = "proposal/artifact_export/get";
    pub const PROPOSAL_ARTIFACT_SUMMARY_LIST_FOR_WORKUNIT: &str =
        "proposal/artifact_summary/list_for_workunit";
    pub const PROPOSAL_LIST_FOR_WORKUNIT: &str = "proposal/list_for_workunit";
    pub const PROPOSAL_APPROVE: &str = "proposal/approve";
    pub const PROPOSAL_RECONCILE: &str = "proposal/reconcile";
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

/// Returns a merged daemon snapshot with collaboration-first state.
///
/// This is useful for reconnect and operator inspection, but it is not the
/// canonical authority planning hierarchy view. A future operator inbox should
/// remain a derived server-side read model in the same spirit, not a new
/// authority truth surface.
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

/// Subscribes to daemon visibility events, optionally with an initial snapshot.
///
/// The initial snapshot is a recovery aid only; future `DaemonEvent`s are
/// visibility signals, not a replay log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventsSubscribeResponse {
    pub subscribed: bool,
    pub snapshot: Option<StateSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventsNotification {
    pub event: DaemonEventEnvelope,
}

/// A daemon snapshot combines runtime daemon/session/thread state with the
/// collaboration snapshot used for recovery and operator inspection plus the
/// derived operator inbox read model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub daemon: DaemonStatusResponse,
    pub session: SessionState,
    pub threads: Vec<ThreadSummary>,
    pub active_thread: Option<ThreadView>,
    #[serde(default)]
    pub collaboration: CollaborationSnapshot,
    #[serde(default)]
    pub operator_inbox: OperatorInboxState,
    #[serde(default)]
    pub recent_events: Vec<EventSummary>,
}

/// The collaboration portion of a daemon snapshot.
///
/// These summaries are execution/runtime oriented and may include compatibility
/// bridge rows, but they are not a canonical planning hierarchy projection.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CollaborationSnapshot {
    pub workstreams: Vec<WorkstreamSummary>,
    pub work_units: Vec<WorkUnitSummary>,
    pub assignments: Vec<AssignmentSummary>,
    #[serde(default)]
    pub codex_thread_assignments: Vec<CodexThreadAssignmentSummary>,
    #[serde(default)]
    pub supervisor_turn_decisions: Vec<SupervisorTurnDecisionSummary>,
    pub reports: Vec<ReportSummary>,
    pub decisions: Vec<DecisionSummary>,
    #[serde(default)]
    pub planning: PlanningState,
}

/// Derived local read model for actionable operator review.
///
/// This is not canonical planning truth. It is a persisted projection over
/// durable collaboration/runtime records so a future server or client can query
/// review work without reinterpreting the source domains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OperatorInboxSourceKind {
    #[default]
    SupervisorProposal,
    SupervisorDecision,
    PlanningSession,
    PlanRevisionProposal,
}

/// Stable operator-facing status for a derived inbox item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OperatorInboxItemStatus {
    #[default]
    Open,
    Resolved,
    Stale,
    Superseded,
}

/// Available review actions for a derived inbox item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OperatorInboxActionKind {
    #[default]
    Approve,
    Reject,
    ApproveAndSend,
    RecordNoAction,
    ManualRefresh,
    Reconcile,
    Retry,
    Supersede,
    MarkReadyForReview,
}

/// Kind of mutation route a derived inbox action resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OperatorInboxChangeKind {
    #[default]
    Upsert,
    Removed,
}

/// A single derived operator-actionable item.
///
/// The item id is stable and derived from the source record identity so the
/// projection can be rebuilt or synced without inventing a new canonical key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorInboxItem {
    pub id: String,
    pub sequence: u64,
    pub source_kind: OperatorInboxSourceKind,
    pub actionable_object_id: String,
    #[serde(default)]
    pub workstream_id: Option<String>,
    #[serde(default)]
    pub work_unit_id: Option<String>,
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub status: OperatorInboxItemStatus,
    #[serde(default)]
    pub available_actions: Vec<OperatorInboxActionKind>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub resolved_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub rationale: Option<String>,
    #[serde(default)]
    pub provenance: Option<String>,
}

/// A durable checkpoint for the derived inbox read model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorInboxCheckpoint {
    pub current_sequence: u64,
    pub updated_at: DateTime<Utc>,
}

impl Default for OperatorInboxCheckpoint {
    fn default() -> Self {
        Self {
            current_sequence: 0,
            updated_at: Utc::now(),
        }
    }
}

/// A durable incremental change for the inbox projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorInboxChange {
    pub sequence: u64,
    pub kind: OperatorInboxChangeKind,
    pub item: OperatorInboxItem,
    pub changed_at: DateTime<Utc>,
}

/// Persisted local state for the derived operator inbox projection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorInboxState {
    #[serde(default)]
    pub items: Vec<OperatorInboxItem>,
    #[serde(default)]
    pub checkpoint: OperatorInboxCheckpoint,
    #[serde(default)]
    pub changes: Vec<OperatorInboxChange>,
}

/// Peer-scoped mirror/export cursor for the derived inbox projection.
///
/// This is intentionally separate from authority replication state: it tracks
/// how far a remote peer has exported or acknowledged the local inbox read
/// model, not canonical planning truth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorInboxMirrorCheckpoint {
    pub peer_id: String,
    pub last_exported_sequence: u64,
    pub last_acked_sequence: u64,
    pub updated_at: DateTime<Utc>,
}

impl Default for OperatorInboxMirrorCheckpoint {
    fn default() -> Self {
        Self {
            peer_id: String::new(),
            last_exported_sequence: 0,
            last_acked_sequence: 0,
            updated_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorInboxMirrorState {
    #[serde(default)]
    pub peers: BTreeMap<String, OperatorInboxMirrorCheckpoint>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OperatorInboxListRequest {
    #[serde(default)]
    pub workstream_id: Option<String>,
    #[serde(default)]
    pub work_unit_id: Option<String>,
    #[serde(default)]
    pub source_kind: Option<OperatorInboxSourceKind>,
    #[serde(default)]
    pub status: Option<OperatorInboxItemStatus>,
    #[serde(default)]
    pub include_closed: bool,
    #[serde(default)]
    pub actionable_only: bool,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxListResponse {
    pub items: Vec<OperatorInboxItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxGetRequest {
    pub item_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxGetResponse {
    pub item: OperatorInboxItem,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OperatorInboxCheckpointRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxCheckpointResponse {
    pub checkpoint: OperatorInboxCheckpoint,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OperatorInboxWaitRequest {
    #[serde(default)]
    pub after_sequence: u64,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxWaitResponse {
    pub checkpoint: OperatorInboxCheckpoint,
    pub advanced: bool,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OperatorInboxChangesRequest {
    #[serde(default)]
    pub after_sequence: u64,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxChangesResponse {
    pub checkpoint: OperatorInboxCheckpoint,
    pub changes: Vec<OperatorInboxChange>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OperatorInboxReplayRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxReplayResponse {
    pub checkpoint: OperatorInboxCheckpoint,
    pub items: Vec<OperatorInboxItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxExportRequest {
    pub peer_id: String,
    #[serde(default)]
    pub after_sequence: Option<u64>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxExportResponse {
    pub checkpoint: OperatorInboxCheckpoint,
    pub mirror_checkpoint: OperatorInboxMirrorCheckpoint,
    pub changes: Vec<OperatorInboxChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxAckRequest {
    pub peer_id: String,
    pub through_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxAckResponse {
    pub mirror_checkpoint: OperatorInboxMirrorCheckpoint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxMirrorCheckpointRequest {
    pub peer_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxMirrorCheckpointResponse {
    pub mirror_checkpoint: OperatorInboxMirrorCheckpoint,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OperatorInboxMirrorCheckpointQueryRequest {
    pub origin_node_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxMirrorCheckpointQueryResponse {
    pub origin_node_id: String,
    pub checkpoint: OperatorInboxCheckpoint,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OperatorInboxWaitForCheckpointRequest {
    pub origin_node_id: String,
    #[serde(default)]
    pub after_sequence: Option<u64>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxWaitForCheckpointResponse {
    pub origin_node_id: String,
    pub checkpoint: OperatorInboxCheckpoint,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxMirrorApplyRequest {
    pub origin_node_id: String,
    pub checkpoint: OperatorInboxCheckpoint,
    #[serde(default)]
    pub changes: Vec<OperatorInboxChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxMirrorApplyResponse {
    pub origin_node_id: String,
    pub checkpoint: OperatorInboxCheckpoint,
    pub mirror_checkpoint: OperatorInboxMirrorCheckpoint,
    pub applied_changes: usize,
    pub skipped_changes: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OperatorInboxMirrorListRequest {
    pub origin_node_id: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxMirrorListResponse {
    pub origin_node_id: String,
    pub checkpoint: OperatorInboxCheckpoint,
    pub items: Vec<OperatorInboxItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxMirrorGetRequest {
    pub origin_node_id: String,
    pub item_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxMirrorGetResponse {
    pub origin_node_id: String,
    pub item: Option<OperatorInboxItem>,
}

/// Server-side notification readiness status derived from mirrored inbox state.
///
/// This is not workflow truth and it does not replace the inbox projection. It
/// only tracks whether a mirrored inbox item has already been surfaced, seen,
/// dismissed, or rendered obsolete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OperatorNotificationCandidateStatus {
    /// The mirrored inbox item is currently actionable and has not been
    /// acknowledged or suppressed yet.
    #[default]
    Pending,
    /// The operator has seen the candidate.
    Acknowledged,
    /// The candidate was intentionally dismissed or muted.
    Suppressed,
    /// The mirrored inbox item is no longer actionable or has disappeared.
    Obsolete,
}

/// A derived server-side notification-readiness candidate.
///
/// The candidate is keyed by origin and inbox item identity so a future server
/// can mirror, acknowledge, and suppress actionable review work without
/// inventing a new source of workflow truth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorNotificationCandidate {
    pub candidate_id: String,
    pub origin_node_id: String,
    pub item_id: String,
    pub trigger_sequence: u64,
    pub status: OperatorNotificationCandidateStatus,
    pub item: OperatorInboxItem,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub acknowledged_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub suppressed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OperatorNotificationListRequest {
    pub origin_node_id: String,
    #[serde(default)]
    pub status: Option<OperatorNotificationCandidateStatus>,
    #[serde(default)]
    pub pending_only: bool,
    #[serde(default)]
    pub actionable_only: bool,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorNotificationListResponse {
    pub origin_node_id: String,
    pub candidates: Vec<OperatorNotificationCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorNotificationGetRequest {
    pub origin_node_id: String,
    pub candidate_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorNotificationGetResponse {
    pub origin_node_id: String,
    pub candidate: Option<OperatorNotificationCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorNotificationAckRequest {
    pub origin_node_id: String,
    pub candidate_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorNotificationAckResponse {
    pub candidate: OperatorNotificationCandidate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorNotificationSuppressRequest {
    pub origin_node_id: String,
    pub candidate_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorNotificationSuppressResponse {
    pub candidate: OperatorNotificationCandidate,
}

/// Control-plane intent for a remote operator action routed through the server.
///
/// This is not workflow truth. It is a durable queue entry that points back to
/// the mirrored inbox item and later resolves through the daemon's existing
/// source-domain mutation APIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OperatorRemoteActionRequestStatus {
    #[default]
    Pending,
    Claimed,
    Completed,
    Failed,
    Canceled,
    Stale,
}

/// A durable remote action request derived from a mirrored inbox item.
///
/// The request carries the mirrored inbox snapshot needed for remote clients
/// to understand what will be executed, but the daemon remains the execution
/// point for the real source-domain mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorRemoteActionRequest {
    pub request_id: String,
    pub origin_node_id: String,
    pub candidate_id: String,
    pub item_id: String,
    pub trigger_sequence: u64,
    pub action_kind: OperatorInboxActionKind,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    pub item: OperatorInboxItem,
    #[serde(default)]
    pub requested_by: Option<String>,
    #[serde(default)]
    pub request_note: Option<String>,
    pub status: OperatorRemoteActionRequestStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub claimed_by: Option<String>,
    #[serde(default)]
    pub claimed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub claimed_until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub claim_token: Option<String>,
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub failed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub canceled_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub stale_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub attempt_count: u64,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OperatorRemoteActionCreateRequest {
    pub origin_node_id: String,
    pub item_id: String,
    pub action_kind: OperatorInboxActionKind,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub requested_by: Option<String>,
    #[serde(default)]
    pub request_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorRemoteActionCreateResponse {
    pub request: OperatorRemoteActionRequest,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OperatorRemoteActionListRequest {
    pub origin_node_id: String,
    #[serde(default)]
    pub candidate_id: Option<String>,
    #[serde(default)]
    pub item_id: Option<String>,
    #[serde(default)]
    pub action_kind: Option<OperatorInboxActionKind>,
    #[serde(default)]
    pub status: Option<OperatorRemoteActionRequestStatus>,
    #[serde(default)]
    pub pending_only: bool,
    #[serde(default)]
    pub actionable_only: bool,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OperatorRemoteActionWaitRequest {
    pub origin_node_id: String,
    pub request_id: String,
    #[serde(default)]
    pub after_updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorRemoteActionWaitResponse {
    pub origin_node_id: String,
    pub request: Option<OperatorRemoteActionRequest>,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorRemoteActionListResponse {
    pub origin_node_id: String,
    pub requests: Vec<OperatorRemoteActionRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorRemoteActionGetRequest {
    pub origin_node_id: String,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorRemoteActionGetResponse {
    pub origin_node_id: String,
    pub request: Option<OperatorRemoteActionRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorRemoteActionClaimRequest {
    pub origin_node_id: String,
    pub worker_id: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub lease_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorRemoteActionClaimedRequest {
    pub request: OperatorRemoteActionRequest,
    pub claim_token: String,
    pub claimed_until: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorRemoteActionClaimResponse {
    pub origin_node_id: String,
    pub requests: Vec<OperatorRemoteActionClaimedRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorRemoteActionCompleteRequest {
    pub origin_node_id: String,
    pub request_id: String,
    pub claim_token: String,
    #[serde(default)]
    pub result: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorRemoteActionCompleteResponse {
    pub origin_node_id: String,
    pub request: OperatorRemoteActionRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorRemoteActionFailRequest {
    pub origin_node_id: String,
    pub request_id: String,
    pub claim_token: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorRemoteActionFailResponse {
    pub origin_node_id: String,
    pub request: OperatorRemoteActionRequest,
}

/// Transport families available to notification delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NotificationTransportKind {
    #[default]
    Log,
    Mock,
    Webhook,
    Apns,
    Fcm,
    WebPush,
}

/// A durable recipient identity for notification delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationRecipient {
    pub recipient_id: String,
    pub display_name: String,
    #[serde(default)]
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotificationRecipientUpsertRequest {
    pub recipient_id: String,
    pub display_name: String,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationRecipientUpsertResponse {
    pub recipient: NotificationRecipient,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotificationRecipientListRequest {
    #[serde(default)]
    pub include_disabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationRecipientListResponse {
    pub recipients: Vec<NotificationRecipient>,
}

/// A durable notification subscription bound to a recipient.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationSubscription {
    pub subscription_id: String,
    pub recipient_id: String,
    pub transport_kind: NotificationTransportKind,
    pub endpoint: Value,
    #[serde(default)]
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotificationSubscriptionUpsertRequest {
    pub subscription_id: String,
    pub recipient_id: String,
    pub transport_kind: NotificationTransportKind,
    #[serde(default)]
    pub endpoint: Value,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationSubscriptionUpsertResponse {
    pub subscription: NotificationSubscription,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotificationSubscriptionListRequest {
    #[serde(default)]
    pub recipient_id: Option<String>,
    #[serde(default)]
    pub enabled_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationSubscriptionListResponse {
    pub subscriptions: Vec<NotificationSubscription>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationSubscriptionSetEnabledRequest {
    pub subscription_id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationSubscriptionSetEnabledResponse {
    pub subscription: NotificationSubscription,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationDeliveryJob {
    pub job_id: String,
    pub origin_node_id: String,
    pub candidate_id: String,
    pub trigger_sequence: u64,
    pub recipient_id: String,
    pub subscription_id: String,
    pub transport_kind: NotificationTransportKind,
    pub status: NotificationDeliveryJobStatus,
    pub attempt_count: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub dispatched_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub delivered_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub failed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub suppressed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub skipped_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub obsolete_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub receipt: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NotificationDeliveryJobStatus {
    #[default]
    Pending,
    Dispatched,
    Delivered,
    Failed,
    Suppressed,
    Skipped,
    Obsolete,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotificationDeliveryJobListRequest {
    #[serde(default)]
    pub origin_node_id: Option<String>,
    #[serde(default)]
    pub candidate_id: Option<String>,
    #[serde(default)]
    pub subscription_id: Option<String>,
    #[serde(default)]
    pub status: Option<NotificationDeliveryJobStatus>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationDeliveryJobListResponse {
    pub jobs: Vec<NotificationDeliveryJob>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationDeliveryJobGetRequest {
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationDeliveryJobGetResponse {
    pub job: Option<NotificationDeliveryJob>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationDeliveryRunPendingRequest {
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub transport_kind: Option<NotificationTransportKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationDeliveryRunPendingResponse {
    pub jobs: Vec<NotificationDeliveryJob>,
}

/// Read-model payload for a browser push notification.
///
/// The payload is derived from mirrored notification readiness and delivery
/// state. It is delivery/display metadata only and does not change workflow
/// truth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserPushNotificationPayload {
    pub notification_id: String,
    pub title: String,
    pub body: String,
    pub route: BrowserPushNotificationRoute,
    #[serde(default)]
    pub source_kind: Option<OperatorInboxSourceKind>,
    #[serde(default)]
    pub candidate_status: Option<OperatorNotificationCandidateStatus>,
    #[serde(default)]
    pub item_status: Option<OperatorInboxItemStatus>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub badge: Option<String>,
}

impl BrowserPushNotificationPayload {
    pub fn route_path(&self) -> String {
        self.route.route_path(self.notification_id.as_str())
    }
}

/// Click-through target for a browser push notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserPushNotificationRoute {
    InboxItem {
        origin_node_id: String,
        item_id: String,
        candidate_id: String,
    },
    RemoteActionRequest {
        origin_node_id: String,
        request_id: String,
    },
    Notifications {
        origin_node_id: String,
    },
    Deliveries {
        origin_node_id: String,
    },
}

impl BrowserPushNotificationRoute {
    pub fn route_path(&self, notification_id: &str) -> String {
        match self {
            Self::InboxItem {
                origin_node_id,
                item_id,
                candidate_id,
            } => {
                let mut query = form_urlencoded::Serializer::new(String::new());
                query.append_pair("origin_node_id", origin_node_id);
                query.append_pair("candidate_id", candidate_id);
                query.append_pair("notification_id", notification_id);
                query.append_pair("push", "1");
                format!("/inbox/{item_id}?{}", query.finish())
            }
            Self::RemoteActionRequest {
                origin_node_id,
                request_id,
            } => {
                let mut query = form_urlencoded::Serializer::new(String::new());
                query.append_pair("origin_node_id", origin_node_id);
                query.append_pair("request_id", request_id);
                query.append_pair("notification_id", notification_id);
                query.append_pair("push", "1");
                format!("/actions/{request_id}?{}", query.finish())
            }
            Self::Notifications { origin_node_id } => {
                let mut query = form_urlencoded::Serializer::new(String::new());
                query.append_pair("origin_node_id", origin_node_id);
                query.append_pair("notification_id", notification_id);
                query.append_pair("push", "1");
                format!("/notifications?{}", query.finish())
            }
            Self::Deliveries { origin_node_id } => {
                let mut query = form_urlencoded::Serializer::new(String::new());
                query.append_pair("origin_node_id", origin_node_id);
                query.append_pair("notification_id", notification_id);
                query.append_pair("push", "1");
                format!("/deliveries?{}", query.finish())
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxActionRouteRequest {
    pub item_id: String,
    pub action_kind: OperatorInboxActionKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInboxActionRouteResponse {
    pub route: OperatorInboxActionRoute,
}

/// Route metadata for a derived inbox action.
///
/// This is read-only guidance that points a remote client back to the existing
/// proposal/decision/session APIs. It is not an inbox-owned mutation surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OperatorInboxActionRoute {
    Proposal {
        item_id: String,
        proposal_id: String,
        method: String,
    },
    SupervisorDecision {
        item_id: String,
        decision_id: String,
        method: String,
    },
    PlanningSession {
        item_id: String,
        session_id: String,
        method: String,
    },
    PlanRevisionProposal {
        item_id: String,
        proposal_id: String,
        method: String,
    },
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

/// A time-stamped event summary retained in snapshots for operator context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSummary {
    pub timestamp: DateTime<Utc>,
    pub kind: String,
    pub message: String,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
}

/// A daemon event envelope adds the emission timestamp around a visibility
/// event.
///
/// It does not provide replay guarantees.
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

/// Visibility events emitted after daemon-side state changes.
///
/// These events notify subscribers that something changed. They do not promise
/// a complete history stream and they do not replace the canonical read
/// surfaces.
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
    TrackedThreadLifecycle {
        action: CollaborationLifecycleAction,
        tracked_thread: authority::TrackedThreadSummary,
    },
    AssignmentLifecycle {
        action: AssignmentLifecycleAction,
        assignment: AssignmentSummary,
    },
    CodexAssignmentLifecycle {
        action: CodexAssignmentLifecycleAction,
        assignment: CodexThreadAssignmentSummary,
    },
    SupervisorDecisionLifecycle {
        action: SupervisorDecisionLifecycleAction,
        decision: SupervisorTurnDecisionSummary,
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

/// Lifecycle verbs used by collaboration-surface event families.
///
/// `Deleted` is an explicit tombstone notification, not a cleanup guarantee for
/// every read surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationLifecycleAction {
    Created,
    Updated,
    Deleted,
    Completed,
    Escalated,
}

/// Lifecycle verbs for collaboration assignment visibility events.
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

/// Lifecycle verbs for Codex assignment visibility events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexAssignmentLifecycleAction {
    Created,
    Paused,
    Resumed,
    Released,
    Updated,
}

/// Lifecycle verbs for supervisor decision visibility events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorDecisionLifecycleAction {
    Created,
    Approved,
    Sent,
    Rejected,
    Superseded,
    Stale,
}

/// Lifecycle verbs for proposal visibility events.
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

/// Provenance marker for planning summaries.
///
/// `Collaboration` means the row came from daemon collaboration state.
/// `AuthorityCompatibilityBridge` means the row is a collaboration-shaped
/// mirror retained for execution/runtime compatibility.
/// `AuthorityProjection` means the row came directly from the authority store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanningSummarySourceKind {
    /// Collaboration-native summary from daemon collaboration state.
    #[default]
    Collaboration,
    /// Collaboration-shaped bridge row retained only for execution compatibility.
    AuthorityCompatibilityBridge,
    /// Authority-owned lifecycle or summary row emitted from the authority store.
    AuthorityProjection,
}

/// Summary view of a workstream in daemon snapshots and events.
///
/// Inspect `source_kind` to distinguish collaboration-native rows from
/// compatibility bridges and authority projections.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkstreamSummary {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub objective: String,
    pub status: WorkstreamStatus,
    pub priority: String,
    #[serde(default)]
    pub source_kind: PlanningSummarySourceKind,
    pub updated_at: DateTime<Utc>,
}

/// Summary view of a work unit in daemon snapshots and events.
///
/// Inspect `source_kind` to distinguish collaboration-native rows from
/// compatibility bridges and authority projections.
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
    #[serde(default)]
    pub source_kind: PlanningSummarySourceKind,
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

/// Summary view of a runtime assignment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentSummary {
    pub id: String,
    pub work_unit_id: String,
    #[serde(default)]
    pub plan_id: Option<String>,
    #[serde(default)]
    pub plan_version: Option<u64>,
    #[serde(default)]
    pub plan_item_id: Option<String>,
    #[serde(default)]
    pub execution_kind: crate::planning::PlanExecutionKind,
    #[serde(default)]
    pub alignment_rationale: Option<String>,
    pub worker_id: String,
    pub worker_session_id: String,
    pub status: AssignmentStatus,
    pub attempt_number: u32,
    pub updated_at: DateTime<Utc>,
}

/// Summary view of a Codex thread assignment mirror.
///
/// This is execution/runtime state, not planning authority state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexThreadAssignmentSummary {
    pub assignment_id: String,
    pub codex_thread_id: String,
    pub workstream_id: String,
    pub work_unit_id: String,
    pub supervisor_id: String,
    pub assigned_by: String,
    pub assigned_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub status: CodexThreadAssignmentStatus,
    pub send_policy: CodexThreadSendPolicy,
    pub bootstrap_state: CodexThreadBootstrapState,
    pub latest_basis_turn_id: Option<String>,
    pub latest_decision_id: Option<String>,
    pub notes: Option<String>,
    pub active: bool,
}

/// Summary view of a supervisor decision.
///
/// Decisions are runtime review state and do not replace the canonical planning
/// hierarchy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorTurnDecisionSummary {
    pub decision_id: String,
    pub assignment_id: String,
    pub codex_thread_id: String,
    #[serde(default)]
    pub workstream_id: Option<String>,
    #[serde(default)]
    pub work_unit_id: Option<String>,
    #[serde(default)]
    pub supervisor_id: Option<String>,
    pub basis_turn_id: Option<String>,
    pub kind: SupervisorTurnDecisionKind,
    pub proposal_kind: SupervisorTurnProposalKind,
    pub proposed_text: Option<String>,
    pub rationale_summary: String,
    pub status: SupervisorTurnDecisionStatus,
    pub created_at: DateTime<Utc>,
    pub approved_at: Option<DateTime<Utc>>,
    pub rejected_at: Option<DateTime<Utc>>,
    pub sent_at: Option<DateTime<Utc>>,
    pub superseded_by: Option<String>,
    pub sent_turn_id: Option<String>,
    pub notes: Option<String>,
    pub open: bool,
}

/// Summary view of a runtime report.
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

/// Summary view of a runtime decision applied to a report.
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

/// Summary view of a supervisor proposal.
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
    #[serde(default)]
    pub has_plan_revision_proposal: bool,
    pub generation_failure_stage: Option<SupervisorProposalFailureStage>,
    pub reasoner_model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorProposalArtifactSummary {
    pub proposal_id: String,
    pub proposal_status: SupervisorProposalStatus,
    #[serde(default)]
    pub prompt_artifact_present: bool,
    pub prompt_template_version: Option<String>,
    pub prompt_hash: Option<String>,
    pub request_body_hash: Option<String>,
    #[serde(default)]
    pub response_artifact_present: bool,
    pub response_hash: Option<String>,
    #[serde(default)]
    pub raw_response_body_present: bool,
    pub raw_response_body_hash: Option<String>,
    pub reasoner_backend: String,
    pub reasoner_model: String,
    pub reasoner_response_id: Option<String>,
    #[serde(default)]
    pub parsed_proposal_present: bool,
    #[serde(default)]
    pub approved_proposal_present: bool,
    pub generation_failure_stage: Option<SupervisorProposalFailureStage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorProposalArtifactDetail {
    pub proposal_id: String,
    pub proposal_status: SupervisorProposalStatus,
    pub created_at: DateTime<Utc>,
    pub validated_at: Option<DateTime<Utc>>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub reasoner_backend: String,
    pub reasoner_model: String,
    pub reasoner_response_id: Option<String>,
    #[serde(default)]
    pub prompt_render: Option<SupervisorPromptRenderArtifact>,
    #[serde(default)]
    pub response_artifact: Option<SupervisorResponseArtifact>,
    #[serde(default)]
    pub reasoner_output_text: Option<String>,
    #[serde(default)]
    pub parsed_proposal: Option<SupervisorProposal>,
    #[serde(default)]
    pub approved_proposal: Option<SupervisorProposal>,
    #[serde(default)]
    pub generation_failure: Option<SupervisorProposalFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorProposalArtifactExport {
    pub proposal_id: String,
    pub primary_work_unit_id: String,
    pub source_report_id: String,
    pub proposal_status: SupervisorProposalStatus,
    pub created_at: DateTime<Utc>,
    pub validated_at: Option<DateTime<Utc>>,
    pub reviewed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub reviewed_by: Option<String>,
    #[serde(default)]
    pub review_note: Option<String>,
    #[serde(default)]
    pub approved_decision_id: Option<String>,
    #[serde(default)]
    pub approved_assignment_id: Option<String>,
    pub artifact_summary: SupervisorProposalArtifactSummary,
    pub artifact_detail: SupervisorProposalArtifactDetail,
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;

    use super::{
        BrowserPushNotificationPayload, BrowserPushNotificationRoute, DaemonEvent,
        DecisionSummary, EventsSubscribeRequest, StateSnapshot,
    };
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

    #[test]
    fn daemon_event_uses_stable_snake_case_tagged_shape() {
        let event = DaemonEvent::WorkUnitLifecycle {
            action: super::CollaborationLifecycleAction::Updated,
            work_unit: super::WorkUnitSummary {
                id: "wu-1".to_string(),
                workstream_id: "ws-1".to_string(),
                title: "Unit".to_string(),
                status: crate::WorkUnitStatus::Ready,
                dependency_count: 0,
                current_assignment_id: None,
                latest_report_id: None,
                proposal: None,
                source_kind: super::PlanningSummarySourceKind::Collaboration,
                updated_at: Utc::now(),
            },
        };

        let value = serde_json::to_value(&event).expect("serialize daemon event");
        assert_eq!(value["type"], "work_unit_lifecycle");
        assert_eq!(value["action"], "updated");
        assert_eq!(value["work_unit"]["id"], "wu-1");

        let round_trip: DaemonEvent =
            serde_json::from_value(value).expect("deserialize daemon event");
        match round_trip {
            DaemonEvent::WorkUnitLifecycle { action, work_unit } => {
                assert_eq!(action, super::CollaborationLifecycleAction::Updated);
                assert_eq!(work_unit.id, "wu-1");
            }
            other => panic!("unexpected event variant: {other:?}"),
        }
    }

    #[test]
    fn events_subscribe_request_requires_explicit_include_snapshot_flag() {
        let error =
            serde_json::from_value::<EventsSubscribeRequest>(json!({})).expect_err("missing field");
        assert!(
            error
                .to_string()
                .contains("missing field `include_snapshot`")
        );

        let request = EventsSubscribeRequest {
            include_snapshot: false,
        };
        let serialized = serde_json::to_value(&request).expect("serialize request");
        assert_eq!(serialized, json!({ "include_snapshot": false }));
    }

    #[test]
    fn browser_push_notification_route_serializes_into_push_deep_link() {
        let payload = BrowserPushNotificationPayload {
            notification_id: "notification-1".to_string(),
            title: "Title".to_string(),
            body: "Body".to_string(),
            route: BrowserPushNotificationRoute::InboxItem {
                origin_node_id: "origin-a".to_string(),
                item_id: "item-1".to_string(),
                candidate_id: "candidate-1".to_string(),
            },
            source_kind: None,
            candidate_status: None,
            item_status: None,
            icon: None,
            badge: None,
        };
        assert_eq!(
            payload.route_path(),
            "/inbox/item-1?origin_node_id=origin-a&candidate_id=candidate-1&notification_id=notification-1&push=1"
        );
    }

    #[test]
    fn browser_push_notification_payload_round_trips_through_serde() {
        let payload = BrowserPushNotificationPayload {
            notification_id: "notification-1".to_string(),
            title: "Title".to_string(),
            body: "Body".to_string(),
            route: BrowserPushNotificationRoute::Notifications {
                origin_node_id: "origin-a".to_string(),
            },
            source_kind: Some(crate::OperatorInboxSourceKind::SupervisorProposal),
            candidate_status: Some(crate::OperatorNotificationCandidateStatus::Pending),
            item_status: Some(crate::OperatorInboxItemStatus::Open),
            icon: Some("/icon-192.svg".to_string()),
            badge: Some("/badge.svg".to_string()),
        };
        let json = serde_json::to_value(&payload).expect("serialize payload");
        let decoded: BrowserPushNotificationPayload =
            serde_json::from_value(json).expect("deserialize payload");
        assert_eq!(decoded, payload);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsListResponse {
    pub data: Vec<ModelSummary>,
}

fn default_sync_timestamp() -> DateTime<Utc> {
    Utc::now()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSummary {
    pub id: String,
    pub display_name: String,
    pub hidden: bool,
    pub is_default: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadLoadedStatus {
    NotLoaded,
    Idle,
    Active,
    SystemError,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadMonitorState {
    #[default]
    Detached,
    Attaching,
    Attached,
    Errored,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThreadsListRequest {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThreadsListScopedRequest {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThreadsListLoadedRequest {}

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
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub loaded_status: ThreadLoadedStatus,
    #[serde(default)]
    pub active_flags: Vec<String>,
    #[serde(default)]
    pub active_turn_id: Option<String>,
    #[serde(default)]
    pub last_seen_turn_id: Option<String>,
    pub recent_output: Option<String>,
    pub recent_event: Option<String>,
    pub turn_in_flight: bool,
    #[serde(default)]
    pub monitor_state: ThreadMonitorState,
    #[serde(default = "default_sync_timestamp")]
    pub last_sync_at: DateTime<Utc>,
    #[serde(default)]
    pub source_kind: Option<String>,
    #[serde(default)]
    pub raw_summary: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadView {
    pub summary: ThreadSummary,
    #[serde(default)]
    pub history_loaded: bool,
    pub turns: Vec<TurnView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnView {
    pub id: String,
    pub status: String,
    pub error_message: Option<String>,
    #[serde(default)]
    pub error_summary: Option<String>,
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub latest_diff: Option<String>,
    #[serde(default)]
    pub latest_plan_snapshot: Option<Value>,
    #[serde(default)]
    pub token_usage_snapshot: Option<Value>,
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
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub payload: Option<Value>,
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
pub struct ThreadReadHistoryRequest {
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadReadHistoryResponse {
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
pub struct ThreadAttachRequest {
    pub thread_id: String,
    pub cwd: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadAttachResponse {
    pub thread: Option<ThreadView>,
    pub attached: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadDetachRequest {
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadDetachResponse {
    pub thread: Option<ThreadView>,
    pub detached: bool,
    pub reason: Option<String>,
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
pub struct TurnSteerRequest {
    pub thread_id: String,
    pub expected_turn_id: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnSteerResponse {
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

/// Snapshot read for `workunit/get`, the retained public runtime-detail
/// exception.
///
/// This is not a canonical planning API. It carries execution detail that the
/// operator client and operator tools still need even when planning authority lives under
/// `authority/*`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkunitGetResponse {
    pub work_unit: WorkUnit,
    pub assignments: Vec<Assignment>,
    pub reports: Vec<Report>,
    pub decisions: Vec<Decision>,
    #[serde(default)]
    pub proposals: Vec<SupervisorProposalRecord>,
}

/// Canonical planning hierarchy snapshot read.
///
/// `include_deleted` controls whether tombstoned authority records are
/// included.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthorityHierarchyGetRequest {
    #[serde(default)]
    pub include_deleted: bool,
}

/// Canonical authority hierarchy read response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityHierarchyGetResponse {
    pub hierarchy: authority::HierarchySnapshot,
}

/// Stored authority events exported in sequence order for replication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityEventsExportRequest {
    pub peer_id: String,
    #[serde(default)]
    pub after_sequence: Option<u64>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityEventsExportResponse {
    pub events: Vec<authority::StoredAuthorityEvent>,
    pub checkpoint: authority::AuthorityReplicationCheckpoint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityEventsAckRequest {
    pub peer_id: String,
    pub through_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityEventsAckResponse {
    pub checkpoint: authority::AuthorityReplicationCheckpoint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityEventsReplayRequest {
    /// Stored authority events to apply into an authority store/projection path.
    ///
    /// The current daemon uses its local authority store. A future server can
    /// reuse the same shape against its own authority storage without turning
    /// collaboration/runtime state into canonical truth.
    pub events: Vec<authority::StoredAuthorityEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityEventsReplayResponse {
    pub replayed_events: Vec<authority::StoredAuthorityEvent>,
    pub projection_checkpoint: Option<authority::ProjectionCheckpoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityDeletePlanRequest {
    pub target: authority::DeleteTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityDeletePlanResponse {
    pub delete_plan: authority::DeletePlan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityWorkstreamCreateRequest {
    pub command: authority::CreateWorkstream,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityWorkstreamCreateResponse {
    pub workstream: authority::WorkstreamRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityWorkstreamEditRequest {
    pub command: authority::EditWorkstream,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityWorkstreamEditResponse {
    pub workstream: authority::WorkstreamRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityWorkstreamDeleteRequest {
    pub command: authority::DeleteWorkstream,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityWorkstreamDeleteResponse {
    pub workstream: authority::WorkstreamRecord,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthorityWorkstreamListRequest {
    #[serde(default)]
    pub include_deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityWorkstreamListResponse {
    pub workstreams: Vec<authority::WorkstreamSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityWorkstreamGetRequest {
    pub workstream_id: authority::WorkstreamId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityWorkstreamGetResponse {
    pub workstream: authority::WorkstreamRecord,
    pub work_units: Vec<authority::WorkUnitSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityWorkunitCreateRequest {
    pub command: authority::CreateWorkUnit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityWorkunitCreateResponse {
    pub work_unit: authority::WorkUnitRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityWorkunitEditRequest {
    pub command: authority::EditWorkUnit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityWorkunitEditResponse {
    pub work_unit: authority::WorkUnitRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityWorkunitDeleteRequest {
    pub command: authority::DeleteWorkUnit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityWorkunitDeleteResponse {
    pub work_unit: authority::WorkUnitRecord,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthorityWorkunitListRequest {
    #[serde(default)]
    pub workstream_id: Option<authority::WorkstreamId>,
    #[serde(default)]
    pub include_deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityWorkunitListResponse {
    pub work_units: Vec<authority::WorkUnitSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityWorkunitGetRequest {
    pub work_unit_id: authority::WorkUnitId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityWorkunitGetResponse {
    pub work_unit: authority::WorkUnitRecord,
    pub tracked_threads: Vec<authority::TrackedThreadSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadCreateRequest {
    pub command: authority::CreateTrackedThread,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadCreateResponse {
    pub tracked_thread: authority::TrackedThreadRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadEditRequest {
    pub command: authority::EditTrackedThread,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadEditResponse {
    pub tracked_thread: authority::TrackedThreadRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadDeleteRequest {
    pub command: authority::DeleteTrackedThread,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadDeleteResponse {
    pub tracked_thread: authority::TrackedThreadRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadListRequest {
    pub work_unit_id: authority::WorkUnitId,
    #[serde(default)]
    pub include_deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadListResponse {
    pub tracked_threads: Vec<authority::TrackedThreadSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadGetRequest {
    pub tracked_thread_id: authority::TrackedThreadId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadGetResponse {
    pub tracked_thread: authority::TrackedThreadRecord,
    #[serde(default)]
    pub workspace_inspection: Option<TrackedThreadWorkspaceInspection>,
    #[serde(default)]
    pub workspace_operation: Option<WorkspaceOperationRecord>,
    #[serde(default)]
    pub prune_workspace_operation: Option<WorkspaceOperationRecord>,
    #[serde(default)]
    pub merge_prep_assessment: Option<TrackedThreadMergePrepAssessment>,
    #[serde(default)]
    pub landing_authorization: Option<LandingAuthorizationRecord>,
    #[serde(default)]
    pub landing_authorization_is_current: Option<bool>,
    #[serde(default)]
    pub landing_execution: Option<LandingExecutionRecord>,
    #[serde(default)]
    pub landing_execution_matches_authorization_basis: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadPrepareWorkspaceRequest {
    pub tracked_thread_id: authority::TrackedThreadId,
    #[serde(default)]
    pub requested_by: Option<String>,
    #[serde(default)]
    pub request_note: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadPrepareWorkspaceResponse {
    pub workspace_operation: WorkspaceOperationRecord,
    pub assignment: Assignment,
    pub worker: Worker,
    pub worker_session: WorkerSession,
    pub report: Report,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadRefreshWorkspaceRequest {
    pub tracked_thread_id: authority::TrackedThreadId,
    #[serde(default)]
    pub requested_by: Option<String>,
    #[serde(default)]
    pub request_note: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadRefreshWorkspaceResponse {
    pub workspace_operation: WorkspaceOperationRecord,
    pub assignment: Assignment,
    pub worker: Worker,
    pub worker_session: WorkerSession,
    pub report: Report,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadMergePrepRequest {
    pub tracked_thread_id: authority::TrackedThreadId,
    #[serde(default)]
    pub requested_by: Option<String>,
    #[serde(default)]
    pub request_note: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadMergePrepResponse {
    pub workspace_operation: WorkspaceOperationRecord,
    #[serde(default)]
    pub merge_prep_assessment: Option<TrackedThreadMergePrepAssessment>,
    pub assignment: Assignment,
    pub worker: Worker,
    pub worker_session: WorkerSession,
    pub report: Report,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadAuthorizeMergeRequest {
    pub tracked_thread_id: authority::TrackedThreadId,
    #[serde(default)]
    pub authorized_by: Option<String>,
    #[serde(default)]
    pub request_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadAuthorizeMergeResponse {
    pub landing_authorization: LandingAuthorizationRecord,
    #[serde(default)]
    pub landing_authorization_is_current: Option<bool>,
    #[serde(default)]
    pub merge_prep_assessment: Option<TrackedThreadMergePrepAssessment>,
    #[serde(default)]
    pub workspace_inspection: Option<TrackedThreadWorkspaceInspection>,
    pub tracked_thread: authority::TrackedThreadRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadExecuteLandingRequest {
    pub tracked_thread_id: authority::TrackedThreadId,
    #[serde(default)]
    pub authorized_by: Option<String>,
    #[serde(default)]
    pub request_note: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadExecuteLandingResponse {
    pub landing_execution: LandingExecutionRecord,
    #[serde(default)]
    pub landing_execution_matches_authorization_basis: Option<bool>,
    #[serde(default)]
    pub landing_authorization: Option<LandingAuthorizationRecord>,
    #[serde(default)]
    pub landing_authorization_is_current: Option<bool>,
    #[serde(default)]
    pub merge_prep_assessment: Option<TrackedThreadMergePrepAssessment>,
    #[serde(default)]
    pub workspace_inspection: Option<TrackedThreadWorkspaceInspection>,
    pub tracked_thread: authority::TrackedThreadRecord,
    pub assignment: Assignment,
    pub worker: Worker,
    pub worker_session: WorkerSession,
    pub report: Report,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadPruneWorkspaceRequest {
    pub tracked_thread_id: authority::TrackedThreadId,
    #[serde(default)]
    pub requested_by: Option<String>,
    #[serde(default)]
    pub request_note: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityTrackedThreadPruneWorkspaceResponse {
    pub workspace_operation: WorkspaceOperationRecord,
    #[serde(default)]
    pub prune_workspace_result: Option<TrackedThreadPruneWorkspaceResult>,
    pub assignment: Assignment,
    pub worker: Worker,
    pub worker_session: WorkerSession,
    pub report: Report,
}

/// Read-only daemon-side inspection of a tracked-thread workspace.
///
/// This is an observed-state payload only. It does not replace the canonical
/// workspace intent stored on the tracked-thread record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedThreadWorkspaceInspection {
    pub inspected_at: DateTime<Utc>,
    pub repository_root: String,
    pub worktree_path: String,
    pub exists: bool,
    pub is_git_worktree: bool,
    #[serde(default)]
    pub current_branch: Option<String>,
    #[serde(default)]
    pub current_head_commit: Option<String>,
    #[serde(default)]
    pub dirty: Option<bool>,
    #[serde(default)]
    pub base_ref: Option<String>,
    #[serde(default)]
    pub base_commit: Option<String>,
    #[serde(default)]
    pub landing_target: Option<String>,
    #[serde(default)]
    pub base_commit_comparison: Option<TrackedThreadWorkspaceRefComparison>,
    #[serde(default)]
    pub landing_target_comparison: Option<TrackedThreadWorkspaceRefComparison>,
    #[serde(default)]
    pub warnings: Vec<TrackedThreadWorkspaceInspectionWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedThreadWorkspaceRefComparison {
    pub reference: String,
    pub ahead_by: u64,
    pub behind_by: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackedThreadWorkspaceInspectionWarning {
    MissingWorktree,
    InvalidWorktree,
    DetachedHead,
    DirtyWorkspace,
    BaseCommitMismatch,
    BehindLandingTarget,
    DivergedFromLandingTarget,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrackedThreadMergePrepReadiness {
    Ready,
    NotReady,
    Blocked,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackedThreadMergePrepReason {
    MissingSuccessfulReport,
    MissingWorkerReportedHead,
    MissingWorktree,
    InvalidWorktree,
    DirtyWorkspace,
    DetachedHead,
    BaseCommitMismatch,
    BehindLandingTarget,
    DivergedFromLandingTarget,
    HeadMismatch,
    UnknownInspectionState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedThreadMergePrepAssessment {
    pub assessed_at: DateTime<Utc>,
    pub readiness: TrackedThreadMergePrepReadiness,
    #[serde(default)]
    pub reasons: Vec<TrackedThreadMergePrepReason>,
    #[serde(default)]
    pub local_head_commit: Option<String>,
    #[serde(default)]
    pub worker_reported_head_commit: Option<String>,
    #[serde(default)]
    pub report_id: Option<String>,
    #[serde(default)]
    pub report_disposition: Option<ReportDisposition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkstreamPlanGetRequest {
    pub workstream_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkstreamPlanGetResponse {
    pub plan: WorkstreamPlan,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkstreamPlanListRequest {
    #[serde(default)]
    pub include_superseded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkstreamPlanListResponse {
    pub plans: Vec<WorkstreamPlan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionCreateRequest {
    pub workstream_id: String,
    #[serde(default)]
    pub planning_thread_id: Option<String>,
    #[serde(default)]
    pub initial_objective: String,
    #[serde(default)]
    pub research_status: PlanningSessionResearchStatus,
    #[serde(default)]
    pub requirements: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub non_goals: Vec<String>,
    #[serde(default)]
    pub open_questions: Vec<String>,
    #[serde(default)]
    pub draft_plan_summary: Option<String>,
    #[serde(default)]
    pub created_by: Option<String>,
    #[serde(default)]
    pub request_note: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionCreateResponse {
    pub session: PlanningSession,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionGetRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionGetResponse {
    pub session: PlanningSession,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanningSessionListRequest {
    #[serde(default)]
    pub workstream_id: Option<String>,
    #[serde(default)]
    pub include_closed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionListResponse {
    pub sessions: Vec<PlanningSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionUpdateSummaryRequest {
    pub session_id: String,
    pub summary: PlanningSessionStructuredSummary,
    #[serde(default)]
    pub updated_by: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionUpdateSummaryResponse {
    pub session: PlanningSession,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionRequestSupervisorContextRequest {
    pub session_id: String,
    #[serde(default)]
    pub requested_by: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionRequestSupervisorContextResponse {
    pub session: PlanningSession,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionRequestResearchRequest {
    pub session_id: String,
    pub worker_id: String,
    #[serde(default)]
    pub requested_by: Option<String>,
    #[serde(default)]
    pub request_note: Option<String>,
    #[serde(default)]
    pub worker_kind: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionRequestResearchResponse {
    pub session: PlanningSession,
    pub assignment: Assignment,
    pub worker: Worker,
    pub worker_session: WorkerSession,
    pub report: Report,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionMarkReadyForReviewRequest {
    pub session_id: String,
    #[serde(default)]
    pub updated_by: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionMarkReadyForReviewResponse {
    pub session: PlanningSession,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionAbortRequest {
    pub session_id: String,
    #[serde(default)]
    pub updated_by: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionAbortResponse {
    pub session: PlanningSession,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionApproveRequest {
    pub session_id: String,
    #[serde(default)]
    pub approved_by: Option<String>,
    #[serde(default)]
    pub review_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionApproveResponse {
    pub session: PlanningSession,
    #[serde(default)]
    pub revision_proposal: Option<PlanRevisionProposal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionRejectRequest {
    pub session_id: String,
    #[serde(default)]
    pub rejected_by: Option<String>,
    #[serde(default)]
    pub review_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionRejectResponse {
    pub session: PlanningSession,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionSupersedeRequest {
    pub session_id: String,
    #[serde(default)]
    pub superseded_by_session_id: Option<String>,
    #[serde(default)]
    pub updated_by: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningSessionSupersedeResponse {
    pub session: PlanningSession,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanAssessmentListRequest {
    #[serde(default)]
    pub workstream_id: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanAssessmentListResponse {
    pub assessments: Vec<PlanAssessment>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanRevisionProposalListRequest {
    #[serde(default)]
    pub workstream_id: Option<String>,
    #[serde(default)]
    pub include_closed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanRevisionProposalListResponse {
    pub proposals: Vec<PlanRevisionProposal>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssignmentStartRequest {
    pub work_unit_id: String,
    pub worker_id: String,
    pub worker_kind: Option<String>,
    pub instructions: Option<String>,
    pub model: Option<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub plan_id: Option<String>,
    #[serde(default)]
    pub plan_version: Option<u64>,
    #[serde(default)]
    pub plan_item_id: Option<String>,
    #[serde(default)]
    pub execution_kind: crate::planning::PlanExecutionKind,
    #[serde(default)]
    pub alignment_rationale: Option<String>,
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
pub struct CodexAssignmentCreateRequest {
    pub codex_thread_id: String,
    pub workstream_id: String,
    pub work_unit_id: String,
    pub supervisor_id: String,
    pub assigned_by: String,
    #[serde(default)]
    pub send_policy: Option<CodexThreadSendPolicy>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexAssignmentCreateResponse {
    pub assignment: CodexThreadAssignment,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexAssignmentGetRequest {
    pub assignment_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexAssignmentGetResponse {
    pub assignment: CodexThreadAssignment,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodexAssignmentListRequest {
    #[serde(default)]
    pub codex_thread_id: Option<String>,
    #[serde(default)]
    pub workstream_id: Option<String>,
    #[serde(default)]
    pub work_unit_id: Option<String>,
    #[serde(default)]
    pub include_inactive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexAssignmentListResponse {
    pub assignments: Vec<CodexThreadAssignmentSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexAssignmentPauseRequest {
    pub assignment_id: String,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexAssignmentPauseResponse {
    pub assignment: CodexThreadAssignment,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexAssignmentResumeRequest {
    pub assignment_id: String,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexAssignmentResumeResponse {
    pub assignment: CodexThreadAssignment,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexAssignmentReleaseRequest {
    pub assignment_id: String,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexAssignmentReleaseResponse {
    pub assignment: CodexThreadAssignment,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SupervisorDecisionListRequest {
    #[serde(default)]
    pub assignment_id: Option<String>,
    #[serde(default)]
    pub codex_thread_id: Option<String>,
    #[serde(default)]
    pub workstream_id: Option<String>,
    #[serde(default)]
    pub work_unit_id: Option<String>,
    #[serde(default)]
    pub supervisor_id: Option<String>,
    #[serde(default)]
    pub status: Option<SupervisorTurnDecisionStatus>,
    #[serde(default)]
    pub kind: Option<SupervisorTurnDecisionKind>,
    #[serde(default)]
    pub include_closed: bool,
    #[serde(default)]
    pub include_superseded: bool,
    #[serde(default)]
    pub actionable_only: bool,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorDecisionListResponse {
    pub decisions: Vec<SupervisorTurnDecisionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorDecisionGetRequest {
    pub decision_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorDecisionGetResponse {
    pub decision: SupervisorTurnDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorDecisionProposeSteerRequest {
    pub assignment_id: String,
    #[serde(default)]
    pub requested_by: Option<String>,
    #[serde(default)]
    pub proposed_text: Option<String>,
    #[serde(default)]
    pub rationale_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorDecisionProposeSteerResponse {
    pub decision: SupervisorTurnDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorDecisionReplacePendingSteerRequest {
    pub decision_id: String,
    pub requested_by: Option<String>,
    pub proposed_text: String,
    #[serde(default)]
    pub rationale_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorDecisionReplacePendingSteerResponse {
    pub decision: SupervisorTurnDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorDecisionProposeInterruptRequest {
    pub assignment_id: String,
    pub requested_by: Option<String>,
    pub rationale_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorDecisionProposeInterruptResponse {
    pub decision: SupervisorTurnDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorDecisionRecordNoActionRequest {
    pub decision_id: String,
    #[serde(default)]
    pub reviewed_by: Option<String>,
    #[serde(default)]
    pub review_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorDecisionRecordNoActionResponse {
    pub decision: SupervisorTurnDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorDecisionManualRefreshRequest {
    pub assignment_id: String,
    #[serde(default)]
    pub requested_by: Option<String>,
    #[serde(default)]
    pub rationale_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorDecisionManualRefreshResponse {
    pub decision: SupervisorTurnDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorDecisionApproveAndSendRequest {
    pub decision_id: String,
    pub reviewed_by: Option<String>,
    pub review_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorDecisionApproveAndSendResponse {
    pub decision: SupervisorTurnDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorDecisionRejectRequest {
    pub decision_id: String,
    pub reviewed_by: Option<String>,
    pub review_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorDecisionRejectResponse {
    pub decision: SupervisorTurnDecision,
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
pub struct ProposalArtifactSummaryGetRequest {
    pub proposal_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalArtifactSummaryGetResponse {
    pub summary: SupervisorProposalArtifactSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalArtifactDetailGetRequest {
    pub proposal_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalArtifactDetailGetResponse {
    pub detail: SupervisorProposalArtifactDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalArtifactExportGetRequest {
    pub proposal_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalArtifactExportGetResponse {
    pub export: SupervisorProposalArtifactExport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalArtifactSummaryListForWorkunitRequest {
    pub work_unit_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalArtifactSummaryListForWorkunitResponse {
    pub work_unit_id: String,
    pub summaries: Vec<SupervisorProposalArtifactSummary>,
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
pub struct ProposalReconcileRequest {
    pub proposal_id: String,
    pub reviewed_by: Option<String>,
    pub review_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalReconcileResponse {
    pub proposal: SupervisorProposalRecord,
    pub plan_revision: PlanRevisionProposal,
    pub applied_plan: WorkstreamPlan,
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
