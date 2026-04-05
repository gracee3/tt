use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::planning::{PlanAssessment, PlanExecutionKind, PlanRevisionProposal, WorkstreamPlan};
use crate::{DecisionType, ReportConfidence, ReportDisposition, ReportParseResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorProposalTriggerKind {
    ReportRecorded,
    HumanRequested,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorProposalTrigger {
    pub kind: SupervisorProposalTriggerKind,
    pub requested_at: DateTime<Utc>,
    pub requested_by: String,
    pub source_report_id: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionPolicy {
    pub supported_decisions: Vec<DecisionType>,
    pub allowed_decisions: Vec<DecisionType>,
    pub disallowed_decisions: Vec<DecisionType>,
    #[serde(default)]
    pub disallowed_reasons_by_decision: BTreeMap<String, String>,
    pub assignment_required_for: Vec<DecisionType>,
    pub assignment_forbidden_for: Vec<DecisionType>,
    pub human_review_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorPackLimits {
    pub max_related_work_units: usize,
    pub max_prior_reports: usize,
    pub max_prior_decisions: usize,
    pub max_artifacts: usize,
    pub max_raw_report_chars: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SupervisorPackTruncation {
    pub related_work_units_truncated: bool,
    pub prior_reports_truncated: bool,
    pub prior_decisions_truncated: bool,
    pub artifacts_truncated: bool,
    pub raw_report_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorStateAnchor {
    pub workstream_id: String,
    pub primary_work_unit_id: String,
    pub source_report_id: String,
    pub source_report_created_at: DateTime<Utc>,
    pub current_assignment_id: Option<String>,
    pub primary_work_unit_updated_at: DateTime<Utc>,
    pub latest_decision_id: Option<String>,
    pub latest_decision_created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorWorkstreamContext {
    pub id: String,
    pub title: String,
    pub objective: String,
    pub status: String,
    pub priority: String,
    #[serde(default)]
    pub success_criteria: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
    pub summary: Option<String>,
    pub open_work_unit_count: usize,
    pub blocked_work_unit_count: usize,
    pub completed_work_unit_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorWorkstreamPlanContext {
    pub active_plan: WorkstreamPlan,
    #[serde(default)]
    pub recent_assessments: Vec<PlanAssessment>,
    #[serde(default)]
    pub pending_revision_proposals: Vec<PlanRevisionProposal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorWorkUnitContext {
    pub id: String,
    pub title: String,
    pub task_statement: String,
    pub status: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub current_assignment_id: Option<String>,
    pub latest_report_id: Option<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub stop_conditions: Vec<String>,
    pub result_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorSourceReportContext {
    pub id: String,
    pub assignment_id: String,
    pub worker_id: String,
    pub worker_session_id: Option<String>,
    pub submitted_at: DateTime<Utc>,
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
    pub confidence: ReportConfidence,
    pub parse_result: ReportParseResult,
    pub needs_supervisor_review: bool,
    pub raw_output_excerpt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorAssignmentContext {
    pub id: String,
    pub status: String,
    pub attempt_number: u32,
    #[serde(default)]
    pub plan_id: Option<String>,
    #[serde(default)]
    pub plan_version: Option<u64>,
    #[serde(default)]
    pub plan_item_id: Option<String>,
    #[serde(default)]
    pub execution_kind: PlanExecutionKind,
    #[serde(default)]
    pub alignment_rationale: Option<String>,
    pub worker_id: String,
    pub worker_session_id: String,
    pub instructions: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorWorkerSessionContext {
    pub id: String,
    pub worker_id: String,
    pub backend_type: String,
    pub thread_id: Option<String>,
    pub active_turn_id: Option<String>,
    pub runtime_status: String,
    pub attachability: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorDependencyContextItem {
    pub work_unit_id: String,
    pub title: String,
    pub status: String,
    pub latest_report_id: Option<String>,
    pub latest_decision_id: Option<String>,
    pub relation: String,
    pub blocking: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SupervisorDependencyContext {
    #[serde(default)]
    pub upstream_dependencies: Vec<SupervisorDependencyContextItem>,
    #[serde(default)]
    pub downstream_dependents: Vec<SupervisorDependencyContextItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedWorkUnitContext {
    pub id: String,
    pub title: String,
    pub status: String,
    pub latest_report_summary: Option<String>,
    pub latest_decision_type: Option<DecisionType>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriorReportContext {
    pub id: String,
    pub disposition: ReportDisposition,
    pub summary: String,
    pub parse_result: ReportParseResult,
    pub needs_supervisor_review: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriorDecisionContext {
    pub id: String,
    pub decision_type: DecisionType,
    pub rationale: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RecentPrimaryHistory {
    #[serde(default)]
    pub prior_reports: Vec<PriorReportContext>,
    #[serde(default)]
    pub prior_decisions: Vec<PriorDecisionContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorArtifactRef {
    pub kind: String,
    pub locator: String,
    pub description: String,
    pub source_object_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorOperatorRequest {
    pub summary: String,
    pub focus: Option<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorContextPack {
    pub schema_version: String,
    pub generated_at: DateTime<Utc>,
    pub trigger: SupervisorProposalTrigger,
    pub pack_limits: SupervisorPackLimits,
    pub truncation: SupervisorPackTruncation,
    pub state_anchor: SupervisorStateAnchor,
    pub decision_policy: DecisionPolicy,
    pub workstream: SupervisorWorkstreamContext,
    #[serde(default)]
    pub workstream_plan: Option<SupervisorWorkstreamPlanContext>,
    pub primary_work_unit: SupervisorWorkUnitContext,
    pub source_report: SupervisorSourceReportContext,
    pub current_assignment: SupervisorAssignmentContext,
    pub worker_session: SupervisorWorkerSessionContext,
    pub dependency_context: SupervisorDependencyContext,
    #[serde(default)]
    pub related_work_units: Vec<RelatedWorkUnitContext>,
    pub recent_primary_history: RecentPrimaryHistory,
    #[serde(default)]
    pub relevant_artifacts: Vec<SupervisorArtifactRef>,
    pub operator_request: Option<SupervisorOperatorRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorSummary {
    pub headline: String,
    pub situation: String,
    pub recommended_action: String,
    #[serde(default)]
    pub key_evidence: Vec<String>,
    #[serde(default)]
    pub risks: Vec<String>,
    #[serde(default)]
    pub review_focus: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposedDecision {
    pub decision_type: DecisionType,
    pub target_work_unit_id: String,
    pub source_report_id: String,
    pub rationale: String,
    pub expected_work_unit_status: String,
    pub requires_assignment: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftAssignment {
    pub target_work_unit_id: String,
    pub predecessor_assignment_id: String,
    pub derived_from_decision_type: DecisionType,
    #[serde(default)]
    pub plan_id: Option<String>,
    #[serde(default)]
    pub plan_version: Option<u64>,
    #[serde(default)]
    pub plan_item_id: Option<String>,
    #[serde(default)]
    pub execution_kind: PlanExecutionKind,
    #[serde(default)]
    pub alignment_rationale: Option<String>,
    pub preferred_worker_id: Option<String>,
    pub worker_kind: Option<String>,
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
    pub boundedness_note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorProposal {
    pub schema_version: String,
    pub summary: SupervisorSummary,
    pub proposed_decision: ProposedDecision,
    pub draft_next_assignment: Option<DraftAssignment>,
    pub confidence: ReportConfidence,
    #[serde(default)]
    pub plan_assessment: Option<PlanAssessment>,
    #[serde(default)]
    pub plan_revision_proposal: Option<PlanRevisionProposal>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub open_questions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SupervisorReasonerUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorPromptRenderSpec {
    pub template_version: String,
    pub context_schema_version: String,
    pub proposal_schema_name: String,
    pub proposal_schema_version: String,
    pub response_format: String,
    pub strict_schema: bool,
    pub context_serialization: String,
    pub style: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorPromptRenderArtifact {
    pub render_spec: SupervisorPromptRenderSpec,
    pub instructions_text: String,
    pub user_content_text: String,
    pub context_pack_text: String,
    pub prompt_hash: String,
    #[serde(default)]
    pub request_body_hash: Option<String>,
    pub rendered_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorResponseContentPart {
    pub part_type: String,
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorResponseOutputItem {
    pub item_type: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub content: Vec<SupervisorResponseContentPart>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorResponseArtifact {
    pub backend_kind: String,
    pub model: String,
    pub response_id: Option<String>,
    #[serde(default)]
    pub usage: Option<SupervisorReasonerUsage>,
    #[serde(default)]
    pub output_items: Vec<SupervisorResponseOutputItem>,
    #[serde(default)]
    pub extracted_output_text: Option<String>,
    pub response_hash: String,
    #[serde(default)]
    pub raw_response_body: Option<String>,
    #[serde(default)]
    pub raw_response_body_hash: Option<String>,
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorProposalStatus {
    #[default]
    Open,
    Approved,
    Rejected,
    Superseded,
    Stale,
    GenerationFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorProposalFailureStage {
    Backend,
    ResponseMalformed,
    ProposalMalformed,
    ProposalValidation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorProposalFailure {
    pub stage: SupervisorProposalFailureStage,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorProposalRecord {
    pub id: String,
    pub workstream_id: String,
    pub primary_work_unit_id: String,
    pub source_report_id: String,
    pub trigger: SupervisorProposalTrigger,
    #[serde(default)]
    pub status: SupervisorProposalStatus,
    pub created_at: DateTime<Utc>,
    pub reasoner_backend: String,
    pub reasoner_model: String,
    pub reasoner_response_id: Option<String>,
    pub reasoner_usage: Option<SupervisorReasonerUsage>,
    #[serde(default)]
    pub reasoner_output_text: Option<String>,
    pub context_pack: SupervisorContextPack,
    #[serde(default)]
    pub prompt_render: Option<SupervisorPromptRenderArtifact>,
    #[serde(default)]
    pub response_artifact: Option<SupervisorResponseArtifact>,
    #[serde(default)]
    pub proposal: Option<SupervisorProposal>,
    #[serde(default)]
    pub approval_edits: Option<SupervisorProposalEdits>,
    pub approved_proposal: Option<SupervisorProposal>,
    #[serde(default)]
    pub generation_failure: Option<SupervisorProposalFailure>,
    pub validated_at: Option<DateTime<Utc>>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub reviewed_by: Option<String>,
    pub review_note: Option<String>,
    pub approved_decision_id: Option<String>,
    pub approved_assignment_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SupervisorProposalEdits {
    pub decision_type: Option<DecisionType>,
    pub decision_rationale: Option<String>,
    pub preferred_worker_id: Option<String>,
    pub worker_kind: Option<String>,
    pub objective: Option<String>,
    #[serde(default)]
    pub instructions: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub stop_conditions: Vec<String>,
    #[serde(default)]
    pub expected_report_fields: Vec<String>,
}

impl SupervisorProposalEdits {
    pub fn is_empty(&self) -> bool {
        self.decision_type.is_none()
            && self.decision_rationale.is_none()
            && self.preferred_worker_id.is_none()
            && self.worker_kind.is_none()
            && self.objective.is_none()
            && self.instructions.is_empty()
            && self.acceptance_criteria.is_empty()
            && self.stop_conditions.is_empty()
            && self.expected_report_fields.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::{
        DraftAssignment, ProposedDecision, SupervisorContextPack, SupervisorPackLimits,
        SupervisorPackTruncation, SupervisorProposal, SupervisorProposalEdits,
        SupervisorProposalFailure, SupervisorProposalFailureStage, SupervisorProposalRecord,
        SupervisorProposalStatus, SupervisorProposalTrigger, SupervisorProposalTriggerKind,
        SupervisorSourceReportContext, SupervisorStateAnchor, SupervisorSummary,
        SupervisorWorkUnitContext,
    };
    use crate::{
        DecisionType, ReportConfidence, ReportDisposition, ReportParseResult,
        planning::PlanExecutionKind,
    };

    fn fixed_now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 5, 6, 7, 8, 9)
            .single()
            .expect("valid timestamp")
    }

    fn sample_context_pack() -> SupervisorContextPack {
        SupervisorContextPack {
            schema_version: "supervisor_context_pack.v2".to_string(),
            generated_at: fixed_now(),
            trigger: SupervisorProposalTrigger {
                kind: SupervisorProposalTriggerKind::ReportRecorded,
                requested_at: fixed_now(),
                requested_by: "daemon".to_string(),
                source_report_id: "report-1".to_string(),
                note: None,
            },
            pack_limits: SupervisorPackLimits {
                max_related_work_units: 8,
                max_prior_reports: 3,
                max_prior_decisions: 3,
                max_artifacts: 0,
                max_raw_report_chars: 3_000,
            },
            truncation: SupervisorPackTruncation::default(),
            state_anchor: SupervisorStateAnchor {
                workstream_id: "ws-1".to_string(),
                primary_work_unit_id: "wu-1".to_string(),
                source_report_id: "report-1".to_string(),
                source_report_created_at: fixed_now(),
                current_assignment_id: Some("assignment-1".to_string()),
                primary_work_unit_updated_at: fixed_now(),
                latest_decision_id: None,
                latest_decision_created_at: None,
            },
            decision_policy: super::DecisionPolicy {
                supported_decisions: vec![DecisionType::Continue, DecisionType::EscalateToHuman],
                allowed_decisions: vec![DecisionType::Continue, DecisionType::EscalateToHuman],
                disallowed_decisions: Vec::new(),
                disallowed_reasons_by_decision: Default::default(),
                assignment_required_for: vec![DecisionType::Continue, DecisionType::Redirect],
                assignment_forbidden_for: vec![
                    DecisionType::Accept,
                    DecisionType::MarkComplete,
                    DecisionType::EscalateToHuman,
                ],
                human_review_required: true,
            },
            workstream: super::SupervisorWorkstreamContext {
                id: "ws-1".to_string(),
                title: "Workstream".to_string(),
                objective: "Objective".to_string(),
                status: "active".to_string(),
                priority: "high".to_string(),
                success_criteria: Vec::new(),
                constraints: Vec::new(),
                summary: None,
                open_work_unit_count: 1,
                blocked_work_unit_count: 0,
                completed_work_unit_count: 0,
            },
            workstream_plan: None,
            primary_work_unit: SupervisorWorkUnitContext {
                id: "wu-1".to_string(),
                title: "Work unit".to_string(),
                task_statement: "Task".to_string(),
                status: "awaiting_decision".to_string(),
                dependencies: Vec::new(),
                current_assignment_id: Some("assignment-1".to_string()),
                latest_report_id: Some("report-1".to_string()),
                acceptance_criteria: Vec::new(),
                stop_conditions: Vec::new(),
                result_summary: None,
            },
            source_report: SupervisorSourceReportContext {
                id: "report-1".to_string(),
                assignment_id: "assignment-1".to_string(),
                worker_id: "worker-1".to_string(),
                worker_session_id: Some("session-1".to_string()),
                submitted_at: fixed_now(),
                disposition: ReportDisposition::Completed,
                summary: "Completed".to_string(),
                findings: Vec::new(),
                blockers: Vec::new(),
                questions: Vec::new(),
                recommended_next_actions: Vec::new(),
                confidence: ReportConfidence::High,
                parse_result: ReportParseResult::Parsed,
                needs_supervisor_review: false,
                raw_output_excerpt: "raw".to_string(),
            },
            current_assignment: super::SupervisorAssignmentContext {
                id: "assignment-1".to_string(),
                status: "awaiting_decision".to_string(),
                attempt_number: 1,
                plan_id: None,
                plan_version: None,
                plan_item_id: None,
                execution_kind: PlanExecutionKind::DirectExecution,
                alignment_rationale: None,
                worker_id: "worker-1".to_string(),
                worker_session_id: "session-1".to_string(),
                instructions: "Do the task".to_string(),
                created_at: fixed_now(),
                updated_at: fixed_now(),
            },
            worker_session: super::SupervisorWorkerSessionContext {
                id: "session-1".to_string(),
                worker_id: "worker-1".to_string(),
                backend_type: "tt".to_string(),
                thread_id: Some("thread-1".to_string()),
                active_turn_id: None,
                runtime_status: "completed".to_string(),
                attachability: "attachable".to_string(),
                updated_at: fixed_now(),
            },
            dependency_context: Default::default(),
            related_work_units: Vec::new(),
            recent_primary_history: Default::default(),
            relevant_artifacts: Vec::new(),
            operator_request: None,
        }
    }

    #[test]
    fn supervisor_proposal_round_trips_nested_draft_and_defaults() {
        let proposal = SupervisorProposal {
            schema_version: "supervisor_proposal.v2".to_string(),
            summary: SupervisorSummary {
                headline: "headline".to_string(),
                situation: "situation".to_string(),
                recommended_action: "action".to_string(),
                key_evidence: vec!["evidence".to_string()],
                risks: Vec::new(),
                review_focus: vec!["focus".to_string()],
            },
            proposed_decision: ProposedDecision {
                decision_type: DecisionType::Continue,
                target_work_unit_id: "wu-1".to_string(),
                source_report_id: "report-1".to_string(),
                rationale: "rationale".to_string(),
                expected_work_unit_status: "ready".to_string(),
                requires_assignment: true,
            },
            draft_next_assignment: Some(DraftAssignment {
                target_work_unit_id: "wu-1".to_string(),
                predecessor_assignment_id: "assignment-1".to_string(),
                derived_from_decision_type: DecisionType::Continue,
                plan_id: None,
                plan_version: None,
                plan_item_id: None,
                execution_kind: PlanExecutionKind::DirectExecution,
                alignment_rationale: None,
                preferred_worker_id: None,
                worker_kind: Some("tt".to_string()),
                objective: "Follow-up objective".to_string(),
                instructions: vec!["step".to_string()],
                acceptance_criteria: vec!["criterion".to_string()],
                stop_conditions: vec!["stop".to_string()],
                required_context_refs: vec!["report-1".to_string()],
                expected_report_fields: vec!["summary".to_string()],
                boundedness_note: "bounded".to_string(),
            }),
            confidence: ReportConfidence::Medium,
            plan_assessment: None,
            plan_revision_proposal: None,
            warnings: Vec::new(),
            open_questions: vec!["question".to_string()],
        };

        let value = serde_json::to_value(&proposal).expect("serialize proposal");
        assert_eq!(value["proposed_decision"]["decision_type"], "continue");
        assert_eq!(
            value["draft_next_assignment"]["derived_from_decision_type"],
            "continue"
        );

        let round_trip: SupervisorProposal =
            serde_json::from_value(value).expect("deserialize proposal");
        assert_eq!(
            round_trip.proposed_decision.decision_type,
            DecisionType::Continue
        );
        assert_eq!(
            round_trip
                .draft_next_assignment
                .expect("draft")
                .expected_report_fields,
            vec!["summary".to_string()]
        );
        assert!(round_trip.warnings.is_empty());
        assert_eq!(round_trip.open_questions, vec!["question".to_string()]);
    }

    #[test]
    fn supervisor_proposal_edits_defaults_missing_fields_and_is_empty() {
        let edits = serde_json::from_value::<SupervisorProposalEdits>(json!({}))
            .expect("deserialize edits");

        assert!(edits.is_empty());
        assert!(edits.instructions.is_empty());
        assert!(edits.acceptance_criteria.is_empty());
        assert!(edits.stop_conditions.is_empty());
        assert!(edits.expected_report_fields.is_empty());
    }

    #[test]
    fn supervisor_proposal_record_defaults_status_and_optional_fields() {
        let record = serde_json::from_value::<SupervisorProposalRecord>(json!({
            "id": "proposal-1",
            "workstream_id": "ws-1",
            "primary_work_unit_id": "wu-1",
            "source_report_id": "report-1",
            "trigger": {
                "kind": "report_recorded",
                "requested_at": fixed_now(),
                "requested_by": "daemon",
                "source_report_id": "report-1",
                "note": null
            },
            "created_at": fixed_now(),
            "reasoner_backend": "responses_api",
            "reasoner_model": "gpt-test",
            "reasoner_response_id": null,
            "reasoner_usage": null,
            "context_pack": sample_context_pack(),
            "proposal": null,
            "approved_proposal": null,
            "validated_at": null,
            "reviewed_at": null,
            "reviewed_by": null,
            "review_note": null,
            "approved_decision_id": null,
            "approved_assignment_id": null
        }))
        .expect("deserialize proposal record");

        assert_eq!(record.status, SupervisorProposalStatus::Open);
        assert!(record.reasoner_output_text.is_none());
        assert!(record.approval_edits.is_none());
        assert!(record.generation_failure.is_none());
    }

    #[test]
    fn supervisor_proposal_failure_stage_uses_stable_snake_case_shape() {
        let failure = SupervisorProposalFailure {
            stage: SupervisorProposalFailureStage::ProposalValidation,
            message: "bad proposal".to_string(),
        };

        let value = serde_json::to_value(&failure).expect("serialize failure");
        assert_eq!(value["stage"], "proposal_validation");

        let round_trip: SupervisorProposalFailure =
            serde_json::from_value(value).expect("deserialize failure");
        assert_eq!(
            round_trip.stage,
            SupervisorProposalFailureStage::ProposalValidation
        );
    }
}
