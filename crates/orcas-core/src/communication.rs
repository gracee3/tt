use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::Error as DeError};
use serde_json::Value;

use crate::authority;
use crate::collaboration::{ReportConfidence, ReportDisposition, ReportParseResult};
use crate::planning::{PlanExecutionKind, PlanId, PlanItemId};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrackedThreadWorkspaceOperationKind {
    #[default]
    PrepareWorkspace,
    RefreshWorkspace,
    MergePrep,
    PruneWorkspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrackedThreadWorkspaceOperationStatus {
    #[default]
    Requested,
    Dispatched,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrackedThreadLandingExecutionResultStatus {
    #[default]
    Succeeded,
    Failed,
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrackedThreadPruneWorkspaceResultStatus {
    #[default]
    Succeeded,
    Failed,
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceCriterionStatus {
    #[serde(alias = "pass", alias = "passed")]
    Met,
    #[serde(alias = "partial", alias = "partially_met")]
    PartiallyMet,
    #[serde(alias = "fail", alias = "failed")]
    NotMet,
    NotAttempted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    #[serde(alias = "add")]
    Added,
    #[serde(alias = "modify")]
    Modified,
    #[serde(alias = "delete")]
    Deleted,
    #[serde(alias = "rename")]
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
    pub plan_id: Option<PlanId>,
    #[serde(default)]
    pub plan_version: Option<u64>,
    #[serde(default)]
    pub plan_item_id: Option<PlanItemId>,
    #[serde(default)]
    pub execution_kind: PlanExecutionKind,
    #[serde(default)]
    pub alignment_rationale: Option<String>,
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
    #[serde(default)]
    pub workspace_operation: Option<TrackedThreadWorkspaceOperationContract>,
    #[serde(default)]
    pub prune_workspace: Option<TrackedThreadPruneWorkspaceContract>,
    #[serde(default)]
    pub landing_execution: Option<TrackedThreadLandingExecutionContract>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignmentWorkspaceContract {
    pub tracked_thread_id: authority::TrackedThreadId,
    pub tracked_thread_title: String,
    pub workspace: authority::TrackedThreadWorkspace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedThreadWorkspaceOperationContract {
    pub kind: TrackedThreadWorkspaceOperationKind,
    pub tracked_thread_id: authority::TrackedThreadId,
    pub tracked_thread_title: String,
    pub workspace: authority::TrackedThreadWorkspace,
    #[serde(default)]
    pub requested_by: Option<String>,
    #[serde(default)]
    pub request_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedThreadPruneWorkspaceContract {
    pub tracked_thread_id: authority::TrackedThreadId,
    pub tracked_thread_title: String,
    pub repository_root: String,
    pub worktree_path: String,
    pub branch_name: String,
    pub landing_target: String,
    pub workspace: authority::TrackedThreadWorkspace,
    #[serde(default)]
    pub linked_landing_execution_id: Option<String>,
    #[serde(default)]
    pub requested_by: Option<String>,
    #[serde(default)]
    pub request_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedThreadLandingExecutionContract {
    pub tracked_thread_id: authority::TrackedThreadId,
    pub tracked_thread_title: String,
    pub landing_authorization_id: String,
    pub authorized_head_commit: String,
    pub landing_target: String,
    #[serde(default)]
    pub requested_by: Option<String>,
    #[serde(default)]
    pub request_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerWorkspaceReport {
    pub tracked_thread_id: authority::TrackedThreadId,
    pub repository_root: String,
    pub worktree_path: String,
    pub branch_name: String,
    pub base_ref: String,
    #[serde(default)]
    pub base_commit: Option<String>,
    #[serde(default)]
    pub head_commit: Option<String>,
    pub workspace_status: authority::TrackedThreadWorkspaceStatus,
    #[serde(default)]
    pub worktree_created: Option<bool>,
    #[serde(default)]
    pub worktree_reused: Option<bool>,
    #[serde(default)]
    pub workspace_dirty: Option<bool>,
    #[serde(default)]
    pub rebase_attempted: Option<bool>,
    #[serde(default)]
    pub rebase_succeeded: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentCommunicationPacket {
    pub schema_version: String,
    pub packet_id: String,
    pub assignment_id: String,
    pub workstream_id: String,
    pub work_unit_id: String,
    #[serde(default)]
    pub plan_id: Option<PlanId>,
    #[serde(default)]
    pub plan_version: Option<u64>,
    #[serde(default)]
    pub plan_item_id: Option<PlanItemId>,
    #[serde(default)]
    pub execution_kind: PlanExecutionKind,
    #[serde(default)]
    pub alignment_rationale: Option<String>,
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
    #[serde(default)]
    pub workspace_contract: Option<AssignmentWorkspaceContract>,
    #[serde(default)]
    pub workspace_operation: Option<TrackedThreadWorkspaceOperationContract>,
    #[serde(default)]
    pub prune_workspace: Option<TrackedThreadPruneWorkspaceContract>,
    #[serde(default)]
    pub landing_execution: Option<TrackedThreadLandingExecutionContract>,
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
    #[serde(alias = "acceptance_id", alias = "id")]
    pub criterion_id: String,
    pub status: AcceptanceCriterionStatus,
    #[serde(default, alias = "details")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TouchedFile {
    pub path: String,
    #[serde(alias = "change_type")]
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
    #[serde(default, deserialize_with = "deserialize_stringish_vec")]
    pub semantic_changes: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_stringish_vec")]
    pub tests_run: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_stringish_vec")]
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
    #[serde(
        default = "default_worker_report_task_mode",
        deserialize_with = "deserialize_assignment_task_mode"
    )]
    pub task_mode: AssignmentTaskMode,
    #[serde(
        default = "default_worker_report_disposition",
        deserialize_with = "deserialize_report_disposition"
    )]
    pub disposition: ReportDisposition,
    #[serde(
        default = "default_worker_report_summary",
        deserialize_with = "deserialize_stringish_string"
    )]
    pub summary: String,
    #[serde(
        default = "default_worker_report_confidence",
        deserialize_with = "deserialize_report_confidence"
    )]
    pub confidence: ReportConfidence,
    #[serde(default, deserialize_with = "deserialize_acceptance_results")]
    pub acceptance_results: Vec<AcceptanceResult>,
    #[serde(default)]
    pub triggered_stop_condition_ids: Vec<String>,
    #[serde(deserialize_with = "deserialize_touched_files")]
    pub touched_files: Vec<TouchedFile>,
    #[serde(default, deserialize_with = "deserialize_stringish_vec")]
    pub commands_run: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_stringish_vec")]
    pub artifacts: Vec<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub questions: Vec<String>,
    #[serde(default)]
    pub recommended_next_actions: Vec<String>,
    #[serde(default)]
    pub uncertainties: Vec<String>,
    pub review_signal: ReviewSignal,
    #[serde(default)]
    pub workspace_report: Option<WorkerWorkspaceReport>,
    #[serde(default)]
    pub prune_workspace_result: Option<TrackedThreadPruneWorkspaceResult>,
    #[serde(default)]
    pub landing_execution_result: Option<TrackedThreadLandingExecutionResult>,
    pub mode_payload: WorkerReportModePayload,
}

fn deserialize_stringish_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<Value>::deserialize(deserializer)?;
    Ok(values.into_iter().map(stringish_value_to_string).collect())
}

fn stringish_value_to_string(value: Value) -> String {
    match value {
        Value::String(value) => value,
        Value::Object(map) => {
            if let Some(summary) = map.get("summary").and_then(Value::as_str) {
                summary.to_string()
            } else if let Some(command) = map.get("command").and_then(Value::as_str) {
                if let Some(exit_code) = map.get("exit_code").and_then(Value::as_i64) {
                    format!("{command} (exit {exit_code})")
                } else {
                    command.to_string()
                }
            } else if let (Some(name), Some(status)) = (
                map.get("name").and_then(Value::as_str),
                map.get("status").and_then(Value::as_str),
            ) {
                format!("{name}: {status}")
            } else if let Some(path) = map.get("path").and_then(Value::as_str) {
                path.to_string()
            } else {
                Value::Object(map).to_string()
            }
        }
        value => value.to_string(),
    }
}

fn deserialize_stringish_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(stringish_value_to_string(value))
}

fn deserialize_assignment_task_mode<'de, D>(deserializer: D) -> Result<AssignmentTaskMode, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::String(value) => match value.as_str() {
            "implement" => Ok(AssignmentTaskMode::Implement),
            "inspect" => Ok(AssignmentTaskMode::Inspect),
            "debug" => Ok(AssignmentTaskMode::Debug),
            "design" => Ok(AssignmentTaskMode::Design),
            "test" => Ok(AssignmentTaskMode::Test),
            _ => Ok(default_worker_report_task_mode()),
        },
        _ => Ok(default_worker_report_task_mode()),
    }
}

fn deserialize_report_disposition<'de, D>(deserializer: D) -> Result<ReportDisposition, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::String(value) => match value.as_str() {
            "completed" => Ok(ReportDisposition::Completed),
            "partial" => Ok(ReportDisposition::Partial),
            "blocked" => Ok(ReportDisposition::Blocked),
            "failed" => Ok(ReportDisposition::Failed),
            "interrupted" => Ok(ReportDisposition::Interrupted),
            "unknown" => Ok(ReportDisposition::Unknown),
            _ => Ok(default_worker_report_disposition()),
        },
        _ => Ok(default_worker_report_disposition()),
    }
}

fn deserialize_report_confidence<'de, D>(deserializer: D) -> Result<ReportConfidence, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::String(value) => match value.as_str() {
            "low" => Ok(ReportConfidence::Low),
            "medium" => Ok(ReportConfidence::Medium),
            "high" => Ok(ReportConfidence::High),
            "unknown" => Ok(ReportConfidence::Unknown),
            _ => Ok(default_worker_report_confidence()),
        },
        _ => Ok(default_worker_report_confidence()),
    }
}

fn deserialize_acceptance_results<'de, D>(
    deserializer: D,
) -> Result<Vec<AcceptanceResult>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<Value>::deserialize(deserializer)?;
    Ok(values
        .into_iter()
        .map(acceptance_result_from_value)
        .collect())
}

fn acceptance_result_criterion_id_from_text(text: &str) -> Option<String> {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix('[')
        && let Some((criterion_id, _)) = rest.split_once(']')
    {
        let criterion_id = criterion_id.trim();
        if !criterion_id.is_empty() {
            return Some(criterion_id.to_string());
        }
    }
    if let Some((criterion_id, _)) = text.split_once(':') {
        let criterion_id = criterion_id.trim();
        if !criterion_id.is_empty() {
            return Some(criterion_id.to_string());
        }
    }
    None
}

fn default_worker_report_task_mode() -> AssignmentTaskMode {
    AssignmentTaskMode::Implement
}

fn default_worker_report_disposition() -> ReportDisposition {
    ReportDisposition::Completed
}

fn default_worker_report_summary() -> String {
    "Worker completed the bounded change.".to_string()
}

fn default_worker_report_confidence() -> ReportConfidence {
    ReportConfidence::High
}

fn deserialize_touched_files<'de, D>(deserializer: D) -> Result<Vec<TouchedFile>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<Value>::deserialize(deserializer)?;
    values.into_iter().map(touched_file_from_value).collect()
}

fn strip_line_suffix(path: &str) -> String {
    let Some((base, suffix)) = path.rsplit_once(':') else {
        return path.to_string();
    };
    if suffix.chars().all(|ch| ch.is_ascii_digit()) {
        base.to_string()
    } else {
        path.to_string()
    }
}

fn acceptance_result_from_value(value: Value) -> AcceptanceResult {
    match value {
        Value::String(value) => {
            let criterion_id =
                acceptance_result_criterion_id_from_text(&value).unwrap_or_else(|| value.clone());
            let note = (!value.trim().is_empty()).then_some(value);
            AcceptanceResult {
                criterion_id,
                status: AcceptanceCriterionStatus::NotAttempted,
                note,
            }
        }
        Value::Object(map) => {
            let criterion_id = map
                .get("criterion_id")
                .or_else(|| map.get("acceptance_id"))
                .or_else(|| map.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("acceptance_unknown")
                .to_string();
            let status = map
                .get("status")
                .and_then(Value::as_str)
                .map(|status| match status {
                    "pass" | "passed" => AcceptanceCriterionStatus::Met,
                    "partial" | "partially_met" => AcceptanceCriterionStatus::PartiallyMet,
                    "fail" | "failed" => AcceptanceCriterionStatus::NotMet,
                    _ => AcceptanceCriterionStatus::NotAttempted,
                })
                .unwrap_or(AcceptanceCriterionStatus::NotAttempted);
            let note = map
                .get("note")
                .or_else(|| map.get("details"))
                .or_else(|| map.get("evidence"))
                .or_else(|| map.get("summary"))
                .map(|value| stringish_value_to_string(value.clone()))
                .filter(|value| !value.trim().is_empty());
            AcceptanceResult {
                criterion_id,
                status,
                note,
            }
        }
        value => AcceptanceResult {
            criterion_id: "acceptance_unknown".to_string(),
            status: AcceptanceCriterionStatus::NotAttempted,
            note: (!value.is_null()).then_some(value.to_string()),
        },
    }
}

fn touched_file_from_value<E>(value: Value) -> Result<TouchedFile, E>
where
    E: serde::de::Error,
{
    match value {
        Value::String(path) => {
            let path = strip_line_suffix(&path);
            Ok(TouchedFile {
                summary: path.clone(),
                path,
                change_kind: FileChangeKind::Modified,
            })
        }
        Value::Object(map) => {
            let Some(path) = map.get("path").and_then(Value::as_str) else {
                return Err(DeError::custom("touched file entry missing path"));
            };
            let path = strip_line_suffix(path);
            let summary = map
                .get("summary")
                .or_else(|| map.get("description"))
                .or_else(|| map.get("name"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| path.clone());
            let change_kind = map
                .get("change_kind")
                .or_else(|| map.get("change_type"))
                .or_else(|| map.get("kind"))
                .and_then(|value| match value {
                    Value::String(value) => match value.as_str() {
                        "add" | "added" => Some(FileChangeKind::Added),
                        "modify" | "modified" => Some(FileChangeKind::Modified),
                        "delete" | "deleted" => Some(FileChangeKind::Deleted),
                        "rename" | "renamed" => Some(FileChangeKind::Renamed),
                        _ => None,
                    },
                    _ => serde_json::from_value::<FileChangeKind>(value.clone()).ok(),
                })
                .unwrap_or(FileChangeKind::Modified);
            Ok(TouchedFile {
                path,
                summary,
                change_kind,
            })
        }
        _ => Err(DeError::custom(
            "touched file entry must be string or object",
        )),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedThreadLandingExecutionResult {
    #[serde(default)]
    pub tracked_thread_id: Option<authority::TrackedThreadId>,
    pub landing_authorization_id: String,
    pub attempted_head_commit: String,
    pub landing_target: String,
    pub status: TrackedThreadLandingExecutionResultStatus,
    #[serde(default)]
    pub landed_commit: Option<String>,
    #[serde(default)]
    pub landing_ref_updated: Option<bool>,
    #[serde(default)]
    pub failure_reason: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedThreadPruneWorkspaceResult {
    #[serde(default)]
    pub tracked_thread_id: Option<authority::TrackedThreadId>,
    pub worktree_path: String,
    #[serde(default)]
    pub branch_name: Option<String>,
    pub status: TrackedThreadPruneWorkspaceResultStatus,
    #[serde(default)]
    pub worktree_removed: Option<bool>,
    #[serde(default)]
    pub branch_removed: Option<bool>,
    #[serde(default)]
    pub refusal_reason: Option<String>,
    #[serde(default)]
    pub failure_reason: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
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
    #[serde(default)]
    pub plan_id: Option<PlanId>,
    #[serde(default)]
    pub plan_version: Option<u64>,
    #[serde(default)]
    pub plan_item_id: Option<PlanItemId>,
    #[serde(default)]
    pub execution_kind: PlanExecutionKind,
    #[serde(default)]
    pub alignment_rationale: Option<String>,
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

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::{
        AcceptanceCriterionStatus, AcceptanceResult, AssignmentChangePolicy,
        AssignmentChecklistItem, AssignmentCommunicationPacket, AssignmentCommunicationPolicy,
        AssignmentCommunicationRecord, AssignmentCommunicationSeed, AssignmentContextBlock,
        AssignmentExecutionContext, AssignmentModeSpec, AssignmentScopeBoundary,
        AssignmentTaskMode, ImplementModePayload, ImplementModeSpec, ReviewSignal,
        ReviewSignalLevel, TouchedFile, WorkerReportContract, WorkerReportEnvelope,
        WorkerReportModePayload, WorkerWorkspaceReport,
    };
    use crate::{
        FileChangeKind, ReportConfidence, ReportDisposition,
        authority::{TrackedThreadId, TrackedThreadWorkspaceStatus},
    };

    fn fixed_now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 7, 8, 9, 10, 11)
            .single()
            .expect("valid timestamp")
    }

    fn sample_packet() -> AssignmentCommunicationPacket {
        AssignmentCommunicationPacket {
            schema_version: "assignment_communication_packet.v1".to_string(),
            packet_id: "packet-1".to_string(),
            assignment_id: "assignment-1".to_string(),
            workstream_id: "ws-1".to_string(),
            work_unit_id: "wu-1".to_string(),
            plan_id: None,
            plan_version: None,
            plan_item_id: None,
            execution_kind: crate::planning::PlanExecutionKind::DirectExecution,
            alignment_rationale: None,
            worker_id: "worker-1".to_string(),
            worker_session_id: "session-1".to_string(),
            created_at: fixed_now(),
            source_decision_id: Some("decision-1".to_string()),
            source_report_id: Some("report-1".to_string()),
            source_proposal_id: None,
            predecessor_assignment_id: Some("assignment-0".to_string()),
            task_mode: AssignmentTaskMode::Implement,
            mode_spec: AssignmentModeSpec::Implement(ImplementModeSpec {
                expected_verification_commands: vec!["cargo test".to_string()],
            }),
            execution_context: AssignmentExecutionContext {
                runtime_kind: "codex_app_server".to_string(),
                repo_root: Some("/repo".to_string()),
                cwd: Some("/repo".to_string()),
                related_repo_roots: vec!["/repo/submodule".to_string()],
                requested_model: Some("gpt-5".to_string()),
                shell: Some("/bin/bash".to_string()),
            },
            objective: "Complete a bounded task.".to_string(),
            instructions: vec!["Stay in scope.".to_string()],
            acceptance_criteria: vec![AssignmentChecklistItem {
                id: "acceptance_1".to_string(),
                text: "Return a valid report.".to_string(),
            }],
            stop_conditions: vec![AssignmentChecklistItem {
                id: "stop_1".to_string(),
                text: "Stop when blocked.".to_string(),
            }],
            allowed_scope: AssignmentScopeBoundary {
                change_policy: AssignmentChangePolicy::CodeAllowed,
                allowed_operations: vec!["edit_repo".to_string()],
                allowed_write_paths: vec!["/repo".to_string()],
                disallowed_paths: vec!["/repo/target".to_string()],
            },
            disallowed_scope: vec!["Do not broaden scope.".to_string()],
            non_goals: vec!["No follow-on work.".to_string()],
            included_context: vec![AssignmentContextBlock {
                id: "ctx-1".to_string(),
                kind: "report".to_string(),
                source_ref: "report-1".to_string(),
                title: "Source report".to_string(),
                lines: vec!["Summary: previous report".to_string()],
                required: true,
                truncated: false,
            }],
            workspace_contract: None,
            workspace_operation: None,
            prune_workspace: None,
            landing_execution: None,
            response_contract: WorkerReportContract {
                schema_version: "worker_report_contract.v1".to_string(),
                task_mode: AssignmentTaskMode::Implement,
                marker_begin: "ORCAS_REPORT_BEGIN".to_string(),
                marker_end: "ORCAS_REPORT_END".to_string(),
                required_common_fields: vec!["summary".to_string()],
                required_mode_fields: vec!["mode_payload.semantic_changes".to_string()],
                allowed_dispositions: vec![ReportDisposition::Completed],
                strict_single_envelope: true,
            },
            policy: AssignmentCommunicationPolicy {
                stop_at_boundary: true,
                single_report_required: true,
                recommendations_are_non_authoritative: true,
                enforce_scope_boundary: true,
            },
        }
    }

    #[test]
    fn assignment_communication_seed_round_trips_optional_and_default_lists() {
        let seed = AssignmentCommunicationSeed {
            plan_id: None,
            plan_version: None,
            plan_item_id: None,
            execution_kind: crate::planning::PlanExecutionKind::DirectExecution,
            alignment_rationale: None,
            source_decision_id: Some("decision-1".to_string()),
            source_report_id: None,
            source_proposal_id: Some("proposal-1".to_string()),
            predecessor_assignment_id: Some("assignment-0".to_string()),
            objective: "Bounded objective".to_string(),
            instructions: vec!["Inspect only the target module.".to_string()],
            acceptance_criteria: vec!["Return a clean report.".to_string()],
            stop_conditions: vec!["Stop on ambiguity.".to_string()],
            required_context_refs: vec!["ctx-1".to_string()],
            expected_report_fields: vec!["summary".to_string()],
            boundedness_note: Some("Stay within the boundary.".to_string()),
            workspace_operation: None,
            prune_workspace: None,
            landing_execution: None,
            mode_spec: AssignmentModeSpec::Implement(ImplementModeSpec {
                expected_verification_commands: vec!["cargo test -p orcas-core".to_string()],
            }),
        };

        let value = serde_json::to_value(&seed).expect("serialize seed");
        assert_eq!(value["mode_spec"]["kind"], "implement");
        assert_eq!(value["source_decision_id"], "decision-1");

        let round_trip =
            serde_json::from_value::<AssignmentCommunicationSeed>(value).expect("deserialize seed");
        assert_eq!(round_trip.task_mode(), AssignmentTaskMode::Implement);
        assert_eq!(round_trip.required_context_refs, vec!["ctx-1".to_string()]);
        assert_eq!(
            round_trip.boundedness_note.as_deref(),
            Some("Stay within the boundary.")
        );
    }

    #[test]
    fn assignment_communication_packet_round_trips_nested_contract_fields() {
        let packet = sample_packet();

        let value = serde_json::to_value(&packet).expect("serialize packet");
        assert_eq!(value["mode_spec"]["kind"], "implement");
        assert_eq!(value["allowed_scope"]["change_policy"], "code_allowed");
        assert_eq!(
            value["response_contract"]["marker_begin"],
            "ORCAS_REPORT_BEGIN"
        );

        let round_trip = serde_json::from_value::<AssignmentCommunicationPacket>(value)
            .expect("deserialize packet");
        assert_eq!(round_trip.task_mode, AssignmentTaskMode::Implement);
        assert_eq!(
            round_trip.allowed_scope.allowed_write_paths,
            vec!["/repo".to_string()]
        );
        assert_eq!(round_trip.included_context.len(), 1);
        assert!(round_trip.policy.stop_at_boundary);
    }

    #[test]
    fn worker_report_envelope_round_trips_nested_payload_and_review_signal() {
        let envelope = WorkerReportEnvelope {
            schema_version: "worker_report_envelope.v1".to_string(),
            assignment_id: "assignment-1".to_string(),
            packet_id: "packet-1".to_string(),
            task_mode: AssignmentTaskMode::Implement,
            disposition: ReportDisposition::Partial,
            summary: "Completed part of the bounded task.".to_string(),
            confidence: ReportConfidence::Medium,
            acceptance_results: vec![AcceptanceResult {
                criterion_id: "acceptance_1".to_string(),
                status: AcceptanceCriterionStatus::PartiallyMet,
                note: Some("One edge remains.".to_string()),
            }],
            triggered_stop_condition_ids: vec!["stop_1".to_string()],
            touched_files: vec![TouchedFile {
                path: "/repo/src/lib.rs".to_string(),
                change_kind: FileChangeKind::Modified,
                summary: "Adjusted a parser branch.".to_string(),
            }],
            commands_run: vec!["cargo test -p orcas-core".to_string()],
            artifacts: vec!["artifact.txt".to_string()],
            blockers: vec!["missing fixture".to_string()],
            questions: vec!["Should we add another contract test?".to_string()],
            recommended_next_actions: vec!["Request review.".to_string()],
            uncertainties: vec!["One edge may still exist.".to_string()],
            review_signal: ReviewSignal {
                level: ReviewSignalLevel::Elevated,
                reasons: vec!["partial result".to_string()],
                focus: vec!["review the remaining edge".to_string()],
            },
            workspace_report: None,
            prune_workspace_result: None,
            landing_execution_result: None,
            mode_payload: WorkerReportModePayload::Implement(ImplementModePayload {
                semantic_changes: vec!["Updated parser logic.".to_string()],
                tests_run: vec!["cargo test -p orcas-core".to_string()],
                rough_edges: vec!["No fixture for one edge case.".to_string()],
            }),
        };

        let value = serde_json::to_value(&envelope).expect("serialize envelope");
        assert_eq!(value["mode_payload"]["kind"], "implement");
        assert_eq!(value["review_signal"]["level"], "elevated");
        assert_eq!(value["touched_files"][0]["change_kind"], "modified");

        let round_trip =
            serde_json::from_value::<WorkerReportEnvelope>(value).expect("deserialize envelope");
        assert_eq!(
            round_trip.mode_payload.task_mode(),
            AssignmentTaskMode::Implement
        );
        assert_eq!(round_trip.review_signal.level, ReviewSignalLevel::Elevated);
        assert_eq!(
            round_trip.acceptance_results[0].status,
            AcceptanceCriterionStatus::PartiallyMet
        );
    }

    #[test]
    fn worker_report_envelope_accepts_live_object_commands_and_acceptance_shape() {
        let value = json!({
            "schema_version": "worker_report_envelope.v1",
            "assignment_id": "assignment-1",
            "packet_id": "packet-1",
            "task_mode": "implement",
            "disposition": "completed",
            "summary": "Fixed the bounded bug.",
            "confidence": "high",
            "acceptance_results": [
                {
                    "id": "acceptance_1",
                    "status": "passed",
                    "details": "Implemented the minimal code change."
                }
            ],
            "triggered_stop_condition_ids": ["stop_1"],
            "touched_files": [
                {
                    "path": "main.c",
                    "change_type": "modified",
                    "summary": "Updated the greeting string."
                }
            ],
            "commands_run": [
                {
                    "command": "make test -j1 V=1",
                    "exit_code": 0
                }
            ],
            "artifacts": [
                {
                    "name": "patch",
                    "mime": "text/x-diff",
                    "content": "*** Begin Patch"
                }
            ],
            "blockers": [],
            "questions": [],
            "recommended_next_actions": [],
            "uncertainties": [],
            "review_signal": {
                "level": "normal",
                "reasons": [],
                "focus": []
            },
            "workspace_report": null,
            "prune_workspace_result": null,
            "landing_execution_result": null,
            "mode_payload": {
                "kind": "implement",
                "semantic_changes": ["Changed one line in main.c."],
                "tests_run": ["make test"],
                "rough_edges": []
            }
        });

        let envelope =
            serde_json::from_value::<WorkerReportEnvelope>(value).expect("deserialize live shape");
        assert_eq!(
            envelope.acceptance_results[0].status,
            AcceptanceCriterionStatus::Met
        );
        assert_eq!(
            envelope.commands_run,
            vec!["make test -j1 V=1 (exit 0)".to_string()]
        );
        assert_eq!(
            envelope.artifacts,
            vec![
                "{\"content\":\"*** Begin Patch\",\"mime\":\"text/x-diff\",\"name\":\"patch\"}"
                    .to_string()
            ]
        );
    }

    #[test]
    fn worker_report_envelope_derives_touched_file_summary_when_missing() {
        let value = json!({
            "schema_version": "worker_report_envelope.v1",
            "assignment_id": "assignment-1",
            "packet_id": "packet-1",
            "task_mode": "implement",
            "disposition": "completed",
            "summary": "Fixed the bounded bug.",
            "confidence": "high",
            "acceptance_results": [],
            "triggered_stop_condition_ids": [],
            "touched_files": [
                {
                    "path": "main.c",
                    "change_type": "modified"
                }
            ],
            "commands_run": [],
            "artifacts": [],
            "blockers": [],
            "questions": [],
            "recommended_next_actions": [],
            "uncertainties": [],
            "review_signal": {
                "level": "normal",
                "reasons": [],
                "focus": []
            },
            "workspace_report": null,
            "prune_workspace_result": null,
            "landing_execution_result": null,
            "mode_payload": {
                "kind": "implement",
                "semantic_changes": ["Changed one line in main.c."],
                "tests_run": ["make test"],
                "rough_edges": []
            }
        });

        let envelope =
            serde_json::from_value::<WorkerReportEnvelope>(value).expect("deserialize envelope");
        assert_eq!(envelope.touched_files[0].path, "main.c");
        assert_eq!(envelope.touched_files[0].summary, "main.c");
        assert_eq!(
            envelope.touched_files[0].change_kind,
            FileChangeKind::Modified
        );
    }

    #[test]
    fn worker_workspace_report_round_trips_optional_observations() {
        let report = WorkerWorkspaceReport {
            tracked_thread_id: TrackedThreadId::parse("tt-1").expect("tracked thread id"),
            repository_root: "/repo".to_string(),
            worktree_path: "/repo/.worktrees/tt-1".to_string(),
            branch_name: "orcas/tt-1".to_string(),
            base_ref: "origin/main".to_string(),
            base_commit: Some("base-123".to_string()),
            head_commit: Some("head-456".to_string()),
            workspace_status: TrackedThreadWorkspaceStatus::Ahead,
            worktree_created: Some(false),
            worktree_reused: Some(true),
            workspace_dirty: Some(false),
            rebase_attempted: Some(true),
            rebase_succeeded: Some(true),
        };

        let value = serde_json::to_value(&report).expect("serialize workspace report");
        assert_eq!(value["workspace_status"], "ahead");
        assert_eq!(value["tracked_thread_id"], "tt-1");

        let round_trip = serde_json::from_value::<WorkerWorkspaceReport>(value)
            .expect("deserialize workspace report");
        assert_eq!(round_trip.head_commit.as_deref(), Some("head-456"));
        assert_eq!(
            round_trip.workspace_status,
            TrackedThreadWorkspaceStatus::Ahead
        );
    }

    #[test]
    fn assignment_communication_record_defaults_optional_fields_when_missing() {
        let packet = sample_packet();
        let record = serde_json::from_value::<AssignmentCommunicationRecord>(json!({
            "assignment_id": "assignment-1",
            "work_unit_id": "wu-1",
            "workstream_id": "ws-1",
            "created_at": fixed_now(),
            "packet": packet,
            "prompt_render": {
                "render_spec": {
                    "template_version": "assignment_prompt.v1",
                    "response_marker_begin": "ORCAS_REPORT_BEGIN",
                    "response_marker_end": "ORCAS_REPORT_END",
                    "style": "plain_text_markdown"
                },
                "rendered_at": fixed_now(),
                "prompt_text": "prompt",
                "packet_hash": "packet-hash",
                "prompt_hash": "prompt-hash"
            },
            "packet_hash": "packet-hash",
            "prompt_hash": "prompt-hash"
        }))
        .expect("deserialize record");

        assert!(record.response_envelope.is_none());
        assert!(record.validation.is_none());
        assert!(record.raw_output_hash.is_none());
        assert!(record.prompt_render.render_spec.section_order.is_empty());
    }
}
