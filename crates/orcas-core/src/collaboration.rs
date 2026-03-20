//! Daemon-owned collaboration and execution/runtime state.
//!
//! This module models the mutable state ORCAS persists for assignments, reports,
//! decisions, worker sessions, and supervisor workflows. It is not the canonical
//! planning hierarchy model; read `authority.rs` for that vocabulary and
//! `ipc.rs` for the public snapshot/event surfaces that expose collaboration
//! state to clients.
//!
//! The bridge sets on [`CollaborationState`] intentionally keep authority-owned
//! planning rows visible where execution/runtime compatibility still needs
//! them. They are not a second planning source of truth.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::communication::{AssignmentCommunicationRecord, AssignmentCommunicationSeed};
use crate::supervisor::SupervisorProposalRecord;

/// Daemon-owned collaboration and execution/runtime state.
///
/// This is the persistence model for runtime state, assignments, reports,
/// decisions, worker sessions, and compatibility bridge markers. It should not
/// be read as the old planning model; canonical planning hierarchy data lives
/// in `authority.rs` and is exposed through authority RPCs.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CollaborationState {
    /// Collaboration-native workstream rows plus authority compatibility bridge markers.
    #[serde(default)]
    pub workstreams: BTreeMap<String, Workstream>,
    /// Authority workstream IDs that are mirrored here for execution/runtime compatibility.
    #[serde(default)]
    pub authority_workstream_bridges: BTreeSet<String>,
    /// Collaboration-native work-unit rows plus authority compatibility bridge markers.
    #[serde(default)]
    pub work_units: BTreeMap<String, WorkUnit>,
    /// Authority work-unit IDs that are mirrored here for execution/runtime compatibility.
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

/// Lifecycle of a Codex thread assignment mirror in collaboration state.
///
/// This is runtime state, not planning hierarchy state. The values are persisted
/// because they drive daemon and TUI execution flows.
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

/// Policy that controls who may emit Codex prompts for an assignment mirror.
///
/// This is an execution/runtime policy, not a planning authority concept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodexThreadSendPolicy {
    #[default]
    HumanApprovalRequired,
    SupervisorMaySend,
}

/// Bootstrap state for a Codex thread assignment mirror.
///
/// This tracks the local/runtime bootstrap path only; it does not describe
/// authority planning lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodexThreadBootstrapState {
    NotNeeded,
    #[default]
    Pending,
    Proposed,
    Sent,
}

/// A daemon-side Codex assignment mirror tying planning work to a live thread.
///
/// The record is persisted in collaboration state so execution/runtime flows can
/// resume and reconcile without treating it as a canonical planning hierarchy
/// row.
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

/// Lifecycle kind for a supervisor decision.
///
/// These kinds describe what the decision is about, not the full state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorTurnDecisionKind {
    #[default]
    NextTurn,
    SteerActiveTurn,
    InterruptActiveTurn,
    NoAction,
}

/// Origin of a supervisor proposal attached to a decision.
///
/// This is persisted runtime state used to explain why a proposal exists.
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

/// Persisted status for a supervisor decision.
///
/// The values reflect runtime-driven transitions; the code does not enforce a
/// complete formal state machine beyond the transitions it explicitly applies.
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

/// A persisted runtime decision attached to a Codex thread assignment.
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

/// Collaboration/runtime status for a workstream.
///
/// This status is persisted with the daemon execution model and is not the
/// authority planning record status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkstreamStatus {
    #[default]
    Active,
    Blocked,
    Completed,
}

/// Daemon-side execution model for a workstream.
///
/// These rows are persisted in collaboration state and may mirror authority
/// planning rows via compatibility bridges.
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

/// Collaboration/runtime status for a work unit.
///
/// This status is persisted with execution state and may differ from the
/// authority planning record status used to seed it.
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

/// Daemon-side execution model for a work unit.
///
/// Work units live in collaboration state because assignments, reports, and
/// decisions need a mutable runtime record even when planning authority is
/// managed elsewhere.
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

/// Lifecycle of an execution assignment.
///
/// These transitions are runtime-derived and persisted with collaboration
/// state.
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

/// A persisted runtime assignment binding a worker to a work unit.
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

/// Execution worker availability in the collaboration model.
///
/// This is a runtime concern only; it does not describe planning authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    #[default]
    Idle,
    Busy,
    Unavailable,
}

/// A worker that can receive assignments in the daemon execution model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worker {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub status: WorkerStatus,
    pub current_assignment_id: Option<String>,
}

/// Persisted runtime state for a worker session.
///
/// This is the daemon's view of a worker session lifecycle, not the TUI-local
/// PTY session manager state.
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

/// Whether a worker session can currently be attached to.
///
/// This is a runtime-attachment concern, not a planning authority concept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkerSessionAttachability {
    Attachable,
    #[default]
    NotAttachable,
    Unknown,
}

/// A worker session record persisted by the daemon runtime model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerSession {
    pub id: String,
    pub worker_id: String,
    pub backend_type: String,
    pub thread_id: Option<String>,
    pub active_turn_id: Option<String>,
    #[serde(default)]
    pub runtime_status: WorkerSessionRuntimeStatus,
    #[serde(default)]
    pub attachability: WorkerSessionAttachability,
    pub updated_at: DateTime<Utc>,
}

/// Outcome classification for a report produced by an assignment.
///
/// The values are execution-derived and persisted in collaboration state.
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

/// Confidence classification for a persisted report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReportConfidence {
    Low,
    Medium,
    High,
    #[default]
    Unknown,
}

/// Parse result classification for a persisted report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReportParseResult {
    Parsed,
    Ambiguous,
    #[default]
    Invalid,
}

/// A persisted runtime report for an assignment.
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

/// Resulting decision applied to a report.
///
/// This is runtime-derived from report review and persisted in collaboration
/// state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionType {
    Accept,
    Continue,
    Redirect,
    MarkComplete,
    EscalateToHuman,
}

/// A persisted runtime decision made against a report.
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
