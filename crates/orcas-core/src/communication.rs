use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::collaboration::{ReportConfidence, ReportDisposition, ReportParseResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentTaskMode {
    Implement,
    Inspect,
    Debug,
    Design,
    Test,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentChangePolicy {
    CodeAllowed,
    ReadOnly,
    DocsOnly,
    TestsOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceCriterionStatus {
    Met,
    PartiallyMet,
    NotMet,
    NotAttempted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewSignalLevel {
    Normal,
    Elevated,
    Required,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentChecklistItem {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentExecutionContext {
    pub runtime_kind: String,
    pub repo_root: Option<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub related_repo_roots: Vec<String>,
    #[serde(default)]
    pub requested_model: Option<String>,
    #[serde(default)]
    pub shell: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentScopeBoundary {
    pub change_policy: AssignmentChangePolicy,
    #[serde(default)]
    pub allowed_operations: Vec<String>,
    #[serde(default)]
    pub allowed_write_paths: Vec<String>,
    #[serde(default)]
    pub disallowed_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentContextBlock {
    pub id: String,
    pub kind: String,
    pub source_ref: String,
    pub title: String,
    #[serde(default)]
    pub lines: Vec<String>,
    pub required: bool,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentCommunicationPolicy {
    pub stop_at_boundary: bool,
    pub single_report_required: bool,
    pub recommendations_are_non_authoritative: bool,
    pub enforce_scope_boundary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImplementModeSpec {
    #[serde(default)]
    pub expected_verification_commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssignmentModeSpec {
    Implement(ImplementModeSpec),
}

impl AssignmentModeSpec {
    #[must_use]
    pub const fn task_mode(&self) -> AssignmentTaskMode {
        match self {
            Self::Implement(_) => AssignmentTaskMode::Implement,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentCommunicationSeed {
    #[serde(default)]
    pub source_decision_id: Option<String>,
    #[serde(default)]
    pub source_report_id: Option<String>,
    #[serde(default)]
    pub source_proposal_id: Option<String>,
    #[serde(default)]
    pub predecessor_assignment_id: Option<String>,
    pub objective: String,
    #[serde(default)]
    pub instructions: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub stop_conditions: Vec<String>,
    #[serde(default)]
    pub required_context_refs: Vec<String>,
    #[serde(default)]
    pub expected_report_fields: Vec<String>,
    #[serde(default)]
    pub boundedness_note: Option<String>,
    pub mode_spec: AssignmentModeSpec,
}

impl AssignmentCommunicationSeed {
    #[must_use]
    pub const fn task_mode(&self) -> AssignmentTaskMode {
        self.mode_spec.task_mode()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerReportContract {
    pub schema_version: String,
    pub task_mode: AssignmentTaskMode,
    pub marker_begin: String,
    pub marker_end: String,
    #[serde(default)]
    pub required_common_fields: Vec<String>,
    #[serde(default)]
    pub required_mode_fields: Vec<String>,
    #[serde(default)]
    pub allowed_dispositions: Vec<ReportDisposition>,
    pub strict_single_envelope: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentCommunicationPacket {
    pub schema_version: String,
    pub packet_id: String,
    pub assignment_id: String,
    pub workstream_id: String,
    pub work_unit_id: String,
    pub worker_id: String,
    pub worker_session_id: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub source_decision_id: Option<String>,
    #[serde(default)]
    pub source_report_id: Option<String>,
    #[serde(default)]
    pub source_proposal_id: Option<String>,
    #[serde(default)]
    pub predecessor_assignment_id: Option<String>,
    pub task_mode: AssignmentTaskMode,
    pub mode_spec: AssignmentModeSpec,
    pub execution_context: AssignmentExecutionContext,
    pub objective: String,
    #[serde(default)]
    pub instructions: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<AssignmentChecklistItem>,
    #[serde(default)]
    pub stop_conditions: Vec<AssignmentChecklistItem>,
    pub allowed_scope: AssignmentScopeBoundary,
    #[serde(default)]
    pub disallowed_scope: Vec<String>,
    #[serde(default)]
    pub non_goals: Vec<String>,
    #[serde(default)]
    pub included_context: Vec<AssignmentContextBlock>,
    pub response_contract: WorkerReportContract,
    pub policy: AssignmentCommunicationPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptRenderSpec {
    pub template_version: String,
    #[serde(default)]
    pub section_order: Vec<String>,
    pub response_marker_begin: String,
    pub response_marker_end: String,
    pub style: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptRenderArtifact {
    pub render_spec: PromptRenderSpec,
    pub rendered_at: DateTime<Utc>,
    pub prompt_text: String,
    pub packet_hash: String,
    pub prompt_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptanceResult {
    pub criterion_id: String,
    pub status: AcceptanceCriterionStatus,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TouchedFile {
    pub path: String,
    pub change_kind: FileChangeKind,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewSignal {
    pub level: ReviewSignalLevel,
    #[serde(default)]
    pub reasons: Vec<String>,
    #[serde(default)]
    pub focus: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImplementModePayload {
    #[serde(default)]
    pub semantic_changes: Vec<String>,
    #[serde(default)]
    pub tests_run: Vec<String>,
    #[serde(default)]
    pub rough_edges: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerReportModePayload {
    Implement(ImplementModePayload),
}

impl WorkerReportModePayload {
    #[must_use]
    pub const fn task_mode(&self) -> AssignmentTaskMode {
        match self {
            Self::Implement(_) => AssignmentTaskMode::Implement,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerReportEnvelope {
    pub schema_version: String,
    pub assignment_id: String,
    pub packet_id: String,
    pub task_mode: AssignmentTaskMode,
    pub disposition: ReportDisposition,
    pub summary: String,
    pub confidence: ReportConfidence,
    pub acceptance_results: Vec<AcceptanceResult>,
    pub triggered_stop_condition_ids: Vec<String>,
    pub touched_files: Vec<TouchedFile>,
    pub commands_run: Vec<String>,
    pub artifacts: Vec<String>,
    pub blockers: Vec<String>,
    pub questions: Vec<String>,
    pub recommended_next_actions: Vec<String>,
    pub uncertainties: Vec<String>,
    pub review_signal: ReviewSignal,
    pub mode_payload: WorkerReportModePayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerReportValidation {
    pub validated_at: DateTime<Utc>,
    pub parse_result: ReportParseResult,
    #[serde(default)]
    pub structural_issues: Vec<String>,
    #[serde(default)]
    pub semantic_issues: Vec<String>,
    #[serde(default)]
    pub policy_violations: Vec<String>,
    pub needs_supervisor_review: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentCommunicationRecord {
    pub assignment_id: String,
    pub work_unit_id: String,
    pub workstream_id: String,
    pub created_at: DateTime<Utc>,
    pub packet: AssignmentCommunicationPacket,
    pub prompt_render: PromptRenderArtifact,
    pub packet_hash: String,
    pub prompt_hash: String,
    #[serde(default)]
    pub response_envelope: Option<WorkerReportEnvelope>,
    #[serde(default)]
    pub validation: Option<WorkerReportValidation>,
    #[serde(default)]
    pub raw_output_hash: Option<String>,
}
