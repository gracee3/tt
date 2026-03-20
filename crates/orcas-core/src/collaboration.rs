use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::authority;
use crate::communication::{AssignmentCommunicationRecord, AssignmentCommunicationSeed};
use crate::supervisor::SupervisorProposalRecord;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CollaborationState {
    #[serde(default)]
    pub workstreams: BTreeMap<String, Workstream>,
    #[serde(default)]
    pub authority_workstream_bridges: BTreeSet<String>,
    #[serde(default)]
    pub work_units: BTreeMap<String, WorkUnit>,
    #[serde(default)]
    pub authority_work_unit_bridges: BTreeSet<String>,
    #[serde(default)]
    pub assignments: BTreeMap<String, Assignment>,
    #[serde(default)]
    pub workers: BTreeMap<String, Worker>,
    #[serde(default)]
    pub worker_sessions: BTreeMap<String, WorkerSession>,
    #[serde(default)]
    pub reports: BTreeMap<String, Report>,
    #[serde(default)]
    pub decisions: BTreeMap<String, Decision>,
    #[serde(default)]
    pub assignment_communications: BTreeMap<String, AssignmentCommunicationRecord>,
    #[serde(default)]
    pub supervisor_proposals: BTreeMap<String, SupervisorProposalRecord>,
    #[serde(default)]
    pub codex_thread_assignments: BTreeMap<String, CodexThreadAssignment>,
    #[serde(default)]
    pub supervisor_turn_decisions: BTreeMap<String, SupervisorTurnDecision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodexThreadAssignmentStatus {
    Proposed,
    #[default]
    Active,
    Paused,
    Completed,
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodexThreadSendPolicy {
    #[default]
    HumanApprovalRequired,
    SupervisorMaySend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodexThreadBootstrapState {
    NotNeeded,
    #[default]
    Pending,
    Proposed,
    Sent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexThreadAssignment {
    pub assignment_id: String,
    pub codex_thread_id: String,
    pub workstream_id: String,
    pub work_unit_id: String,
    pub supervisor_id: String,
    pub assigned_by: String,
    pub assigned_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub status: CodexThreadAssignmentStatus,
    #[serde(default)]
    pub send_policy: CodexThreadSendPolicy,
    #[serde(default)]
    pub bootstrap_state: CodexThreadBootstrapState,
    #[serde(default)]
    pub latest_basis_turn_id: Option<String>,
    #[serde(default)]
    pub latest_decision_id: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorTurnDecisionKind {
    #[default]
    NextTurn,
    SteerActiveTurn,
    InterruptActiveTurn,
    NoAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorTurnProposalKind {
    #[default]
    Bootstrap,
    ContinueAfterTurn,
    ManualRefresh,
    OperatorSteer,
    OperatorInterrupt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorTurnDecisionStatus {
    Draft,
    #[default]
    ProposedToHuman,
    Approved,
    Rejected,
    Recorded,
    Sent,
    Superseded,
    Stale,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorTurnDecision {
    pub decision_id: String,
    pub assignment_id: String,
    pub codex_thread_id: String,
    #[serde(default)]
    pub basis_turn_id: Option<String>,
    #[serde(default)]
    pub kind: SupervisorTurnDecisionKind,
    #[serde(default)]
    pub proposal_kind: SupervisorTurnProposalKind,
    #[serde(default)]
    pub proposed_text: Option<String>,
    pub rationale_summary: String,
    #[serde(default)]
    pub status: SupervisorTurnDecisionStatus,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub approved_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub rejected_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub sent_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub superseded_by: Option<String>,
    #[serde(default)]
    pub sent_turn_id: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkstreamStatus {
    #[default]
    Active,
    Blocked,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workstream {
    pub id: String,
    pub title: String,
    pub objective: String,
    pub status: WorkstreamStatus,
    pub priority: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkUnitStatus {
    #[default]
    Ready,
    Blocked,
    Running,
    AwaitingDecision,
    Accepted,
    NeedsHuman,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkUnit {
    pub id: String,
    pub workstream_id: String,
    pub title: String,
    pub task_statement: String,
    #[serde(default)]
    pub status: WorkUnitStatus,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub latest_report_id: Option<String>,
    pub current_assignment_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentStatus {
    #[default]
    Created,
    Running,
    AwaitingDecision,
    Failed,
    Closed,
    Interrupted,
    Lost,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assignment {
    pub id: String,
    pub work_unit_id: String,
    pub worker_id: String,
    pub worker_session_id: String,
    pub instructions: String,
    #[serde(default)]
    pub communication_seed: Option<AssignmentCommunicationSeed>,
    #[serde(default)]
    pub status: AssignmentStatus,
    pub attempt_number: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    #[default]
    Idle,
    Busy,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worker {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub status: WorkerStatus,
    pub current_assignment_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkerSessionRuntimeStatus {
    #[default]
    Idle,
    Running,
    Completed,
    Failed,
    Interrupted,
    Lost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkerSessionAttachability {
    Attachable,
    #[default]
    NotAttachable,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerSession {
    pub id: String,
    pub worker_id: String,
    pub backend_type: String,
    pub thread_id: Option<String>,
    #[serde(default)]
    pub tracked_thread_id: Option<authority::TrackedThreadId>,
    pub active_turn_id: Option<String>,
    #[serde(default)]
    pub runtime_status: WorkerSessionRuntimeStatus,
    #[serde(default)]
    pub attachability: WorkerSessionAttachability,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReportDisposition {
    Completed,
    Partial,
    Blocked,
    Failed,
    Interrupted,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReportConfidence {
    Low,
    Medium,
    High,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReportParseResult {
    Parsed,
    Ambiguous,
    #[default]
    Invalid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub id: String,
    pub work_unit_id: String,
    pub assignment_id: String,
    pub worker_id: String,
    #[serde(default)]
    pub disposition: ReportDisposition,
    pub summary: String,
    #[serde(default)]
    pub findings: Vec<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub questions: Vec<String>,
    #[serde(default)]
    pub recommended_next_actions: Vec<String>,
    #[serde(default)]
    pub confidence: ReportConfidence,
    pub raw_output: String,
    #[serde(default)]
    pub parse_result: ReportParseResult,
    #[serde(default)]
    pub needs_supervisor_review: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionType {
    Accept,
    Continue,
    Redirect,
    MarkComplete,
    EscalateToHuman,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub work_unit_id: String,
    pub report_id: Option<String>,
    pub decision_type: DecisionType,
    pub rationale: String,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::{
        CollaborationState, SupervisorTurnDecision, SupervisorTurnDecisionKind,
        SupervisorTurnDecisionStatus, SupervisorTurnProposalKind,
    };

    fn fixed_now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 8, 9, 10, 11, 12)
            .single()
            .expect("valid timestamp")
    }

    #[test]
    fn collaboration_state_defaults_missing_maps_to_empty() {
        let state =
            serde_json::from_value::<CollaborationState>(json!({})).expect("deserialize state");

        assert!(state.workstreams.is_empty());
        assert!(state.work_units.is_empty());
        assert!(state.assignments.is_empty());
        assert!(state.reports.is_empty());
        assert!(state.supervisor_turn_decisions.is_empty());
    }

    #[test]
    fn supervisor_turn_decision_defaults_optional_and_status_fields_when_missing() {
        let decision = serde_json::from_value::<SupervisorTurnDecision>(json!({
            "decision_id": "decision-1",
            "assignment_id": "assignment-1",
            "codex_thread_id": "thread-1",
            "rationale_summary": "Bootstrap review required.",
            "created_at": fixed_now()
        }))
        .expect("deserialize supervisor turn decision");

        assert_eq!(decision.kind, SupervisorTurnDecisionKind::NextTurn);
        assert_eq!(
            decision.proposal_kind,
            SupervisorTurnProposalKind::Bootstrap
        );
        assert_eq!(
            decision.status,
            SupervisorTurnDecisionStatus::ProposedToHuman
        );
        assert!(decision.basis_turn_id.is_none());
        assert!(decision.proposed_text.is_none());
        assert!(decision.approved_at.is_none());
        assert!(decision.rejected_at.is_none());
        assert!(decision.sent_at.is_none());
        assert!(decision.superseded_by.is_none());
        assert!(decision.sent_turn_id.is_none());
        assert!(decision.notes.is_none());
    }
}
