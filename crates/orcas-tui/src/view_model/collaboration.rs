use crate::app::{AppState, CollaborationFocus};
use orcas_core::{
    AssignmentStatus, DecisionType, ReportConfidence, ReportDisposition, ReportParseResult,
    SupervisorProposalEdits, SupervisorProposalFailureStage, SupervisorProposalStatus,
    WorkUnitStatus, WorkstreamStatus, ipc,
};

use super::shared::{abbreviate, compact_line, short_id, timestamp_label};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollaborationStatusViewModel {
    pub focus: CollaborationFocus,
    pub workstream_count: usize,
    pub work_unit_count: usize,
    pub active_assignment_count: usize,
    pub review_count: usize,
    pub selected_workstream_title: Option<String>,
    pub selected_work_unit_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkstreamRowViewModel {
    pub id: String,
    pub title: String,
    pub status: String,
    pub counts: String,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkstreamListViewModel {
    pub rows: Vec<WorkstreamRowViewModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkstreamDetailViewModel {
    pub title: String,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitRowViewModel {
    pub id: String,
    pub title: String,
    pub status: String,
    pub current_assignment: String,
    pub latest_report_parse_result: String,
    pub needs_supervisor_review: bool,
    pub proposal_status: String,
    pub latest_decision: String,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitListViewModel {
    pub rows: Vec<WorkUnitRowViewModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentRowViewModel {
    pub id: String,
    pub work_unit_title: String,
    pub worker_id: String,
    pub worker_session_id: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentListViewModel {
    pub rows: Vec<AssignmentRowViewModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollaborationDetailViewModel {
    pub title: String,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollaborationHistoryViewModel {
    pub title: String,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollaborationViewModel {
    pub status: CollaborationStatusViewModel,
    pub workstreams: WorkstreamListViewModel,
    pub workstream_detail: WorkstreamDetailViewModel,
    pub work_units: WorkUnitListViewModel,
    pub detail: CollaborationDetailViewModel,
    pub history: CollaborationHistoryViewModel,
}

pub fn collaboration_status(state: &AppState) -> CollaborationStatusViewModel {
    CollaborationStatusViewModel {
        focus: state.collaboration_focus,
        workstream_count: state.collaboration.workstreams.len(),
        work_unit_count: state.collaboration.work_units.len(),
        active_assignment_count: state
            .collaboration
            .assignments
            .iter()
            .filter(|assignment| {
                matches!(
                    assignment.status,
                    AssignmentStatus::Created
                        | AssignmentStatus::Running
                        | AssignmentStatus::AwaitingDecision
                )
            })
            .count(),
        review_count: state
            .collaboration
            .reports
            .iter()
            .filter(|report| report.needs_supervisor_review)
            .count(),
        selected_workstream_title: state
            .selected_workstream_id
            .as_ref()
            .and_then(|workstream_id| {
                state
                    .collaboration
                    .workstreams
                    .iter()
                    .find(|workstream| workstream.id == *workstream_id)
            })
            .map(|workstream| workstream.title.clone()),
        selected_work_unit_title: state
            .selected_work_unit_id
            .as_ref()
            .and_then(|work_unit_id| {
                state
                    .collaboration
                    .work_units
                    .iter()
                    .find(|work_unit| work_unit.id == *work_unit_id)
            })
            .map(|work_unit| work_unit.title.clone()),
    }
}

pub fn workstream_list(state: &AppState) -> WorkstreamListViewModel {
    WorkstreamListViewModel {
        rows: state
            .collaboration
            .workstreams
            .iter()
            .map(|workstream| {
                let work_units = state
                    .collaboration
                    .work_units
                    .iter()
                    .filter(|work_unit| work_unit.workstream_id == workstream.id)
                    .collect::<Vec<_>>();
                let review_count = work_units
                    .iter()
                    .filter(|work_unit| {
                        latest_report_for_work_unit(state, &work_unit.id)
                            .is_some_and(|report| report.needs_supervisor_review)
                    })
                    .count();
                WorkstreamRowViewModel {
                    id: workstream.id.clone(),
                    title: abbreviate(&workstream.title, 32),
                    status: collaboration_status_label(workstream.status),
                    counts: format!("units={} review={review_count}", work_units.len()),
                    selected: state.selected_workstream_id.as_deref()
                        == Some(workstream.id.as_str()),
                }
            })
            .collect(),
    }
}

pub fn workstream_detail(state: &AppState) -> WorkstreamDetailViewModel {
    let Some(workstream_id) = state.selected_workstream_id.as_ref() else {
        return WorkstreamDetailViewModel {
            title: "Workstream Summary".to_string(),
            lines: vec!["No workstream selected.".to_string()],
        };
    };
    let Some(workstream) = state
        .collaboration
        .workstreams
        .iter()
        .find(|workstream| workstream.id == *workstream_id)
    else {
        return WorkstreamDetailViewModel {
            title: format!("Workstream Summary {workstream_id}"),
            lines: vec!["Selected workstream is no longer present.".to_string()],
        };
    };

    let work_units = state
        .collaboration
        .work_units
        .iter()
        .filter(|work_unit| work_unit.workstream_id == workstream.id)
        .collect::<Vec<_>>();
    let completed_count = work_units
        .iter()
        .filter(|work_unit| matches!(work_unit.status, WorkUnitStatus::Completed))
        .count();
    let review_count = work_units
        .iter()
        .filter(|work_unit| {
            latest_report_for_work_unit(state, &work_unit.id)
                .is_some_and(|report| report.needs_supervisor_review)
        })
        .count();

    WorkstreamDetailViewModel {
        title: format!("Workstream {}", workstream.title),
        lines: vec![
            format!("id: {}", workstream.id),
            format!("status: {}", collaboration_status_label(workstream.status)),
            format!("priority: {}", workstream.priority),
            format!("objective: {}", compact_line(&workstream.objective)),
            format!(
                "units: total={} completed={} review={}",
                work_units.len(),
                completed_count,
                review_count
            ),
        ],
    }
}

pub fn work_unit_list(state: &AppState) -> WorkUnitListViewModel {
    let Some(workstream_id) = state.selected_workstream_id.as_ref() else {
        return WorkUnitListViewModel { rows: Vec::new() };
    };

    WorkUnitListViewModel {
        rows: state
            .collaboration
            .work_units
            .iter()
            .filter(|work_unit| work_unit.workstream_id == *workstream_id)
            .map(|work_unit| {
                let latest_report = latest_report_for_work_unit(state, &work_unit.id);
                let latest_decision = latest_decision_for_work_unit(state, &work_unit.id);
                WorkUnitRowViewModel {
                    id: work_unit.id.clone(),
                    title: abbreviate(&work_unit.title, 28),
                    status: work_unit_status_label(work_unit.status),
                    current_assignment: work_unit
                        .current_assignment_id
                        .clone()
                        .map(|id| short_id(&id))
                        .unwrap_or_else(|| "-".to_string()),
                    latest_report_parse_result: latest_report
                        .map(|report| report_parse_result_label(report.parse_result).to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    needs_supervisor_review: latest_report
                        .is_some_and(|report| report.needs_supervisor_review),
                    proposal_status: work_unit
                        .proposal
                        .as_ref()
                        .map(proposal_summary_label)
                        .unwrap_or_else(|| "-".to_string()),
                    latest_decision: latest_decision
                        .map(|decision| decision_type_label(decision.decision_type).to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    selected: state.selected_work_unit_id.as_deref() == Some(work_unit.id.as_str()),
                }
            })
            .collect(),
    }
}

pub fn assignment_list(state: &AppState) -> AssignmentListViewModel {
    let workstream_work_units = state
        .selected_workstream_id
        .as_ref()
        .map(|workstream_id| {
            state
                .collaboration
                .work_units
                .iter()
                .filter(|work_unit| work_unit.workstream_id == *workstream_id)
                .map(|work_unit| work_unit.id.as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    AssignmentListViewModel {
        rows: state
            .collaboration
            .assignments
            .iter()
            .filter(|assignment| {
                matches!(
                    assignment.status,
                    AssignmentStatus::Created
                        | AssignmentStatus::Running
                        | AssignmentStatus::AwaitingDecision
                ) && workstream_work_units.contains(&assignment.work_unit_id.as_str())
            })
            .map(|assignment| AssignmentRowViewModel {
                id: short_id(&assignment.id),
                work_unit_title: state
                    .collaboration
                    .work_units
                    .iter()
                    .find(|work_unit| work_unit.id == assignment.work_unit_id)
                    .map(|work_unit| work_unit.title.clone())
                    .map(|title| abbreviate(&title, 18))
                    .unwrap_or_else(|| short_id(&assignment.work_unit_id)),
                worker_id: abbreviate(&assignment.worker_id, 12),
                worker_session_id: short_id(&assignment.worker_session_id),
                status: assignment_status_label(assignment.status),
            })
            .collect(),
    }
}

pub fn collaboration_detail(state: &AppState) -> CollaborationDetailViewModel {
    let Some(work_unit_id) = state.selected_work_unit_id.as_ref() else {
        return CollaborationDetailViewModel {
            title: "Work Unit Detail".to_string(),
            lines: vec!["No work unit selected.".to_string()],
        };
    };
    let Some(work_unit) = state
        .collaboration
        .work_units
        .iter()
        .find(|work_unit| work_unit.id == *work_unit_id)
    else {
        return CollaborationDetailViewModel {
            title: "Work Unit Detail".to_string(),
            lines: vec!["Selected work unit is no longer present.".to_string()],
        };
    };

    let latest_report = latest_report_for_work_unit(state, work_unit_id);
    let latest_decision = latest_decision_for_work_unit(state, work_unit_id);
    let latest_proposal = work_unit.proposal.as_ref();
    let assignment = work_unit
        .current_assignment_id
        .as_ref()
        .and_then(|assignment_id| {
            state
                .collaboration
                .assignments
                .iter()
                .find(|assignment| assignment.id == *assignment_id)
        });

    let mut lines = vec![
        format!("work unit: {}", work_unit.title),
        format!("status: {}", work_unit_status_label(work_unit.status)),
    ];

    if let Some(report) = latest_report {
        lines.push(format!(
            "report: {} parse={} review={}",
            report.id,
            report_parse_result_label(report.parse_result),
            report.needs_supervisor_review
        ));
        lines.push(format!(
            "disposition: {} confidence: {}",
            report_disposition_label(report.disposition),
            report_confidence_label(report.confidence)
        ));
        lines.push(format!(
            "report_summary: {}",
            abbreviate(&compact_line(&report.summary), 84)
        ));
    } else {
        lines.push("report: -".to_string());
    }

    if let Some(assignment) = assignment {
        lines.push(format!(
            "assignment: {} [{}] worker={} session={}",
            assignment.id,
            assignment_status_label(assignment.status),
            assignment.worker_id,
            assignment.worker_session_id
        ));
    } else {
        lines.push("assignment: -".to_string());
    }

    if let Some(decision) = latest_decision {
        lines.push(format!(
            "decision: {} [{}]",
            decision.id,
            decision_type_label(decision.decision_type)
        ));
        lines.push(format!(
            "decision_rationale: {}",
            abbreviate(&compact_line(&decision.rationale), 84)
        ));
    } else {
        lines.push("decision: -".to_string());
    }

    if let Some(proposal) = latest_proposal {
        lines.push(format!(
            "proposal: {} status={} latest_decision={} open={} stale_or_superseded={} failed={} edits={}",
            short_id(&proposal.latest_proposal_id),
            proposal_status_label(proposal.latest_status),
            proposal
                .latest_proposed_decision_type
                .map(|decision| decision_type_label(decision).to_string())
                .unwrap_or_else(|| "-".to_string()),
            proposal.has_open_proposal,
            proposal.has_stale_or_superseded,
            proposal.has_generation_failed,
            proposal.latest_has_approval_edits
        ));
        if let Some(open_decision) = proposal.open_proposed_decision_type {
            lines.push(format!(
                "proposal_open: {} decision={}",
                proposal
                    .open_proposal_id
                    .as_ref()
                    .map(|id| short_id(id))
                    .unwrap_or_else(|| "-".to_string()),
                decision_type_label(open_decision)
            ));
        }
        lines.push(format!(
            "proposal_timing: created={} reviewed={}",
            timestamp_label(proposal.latest_created_at),
            proposal
                .latest_reviewed_at
                .map(timestamp_label)
                .unwrap_or_else(|| "-".to_string())
        ));
        if let Some(stage) = proposal.latest_failure_stage {
            lines.push(format!(
                "proposal_failure: {}",
                proposal_failure_stage_label(stage)
            ));
        }
    } else {
        lines.push("proposal: -".to_string());
    }

    CollaborationDetailViewModel {
        title: format!("Work Unit {}", work_unit.id),
        lines,
    }
}

pub fn collaboration_history(state: &AppState) -> CollaborationHistoryViewModel {
    let Some(work_unit_id) = state.selected_work_unit_id.as_ref() else {
        return CollaborationHistoryViewModel {
            title: "History".to_string(),
            lines: vec!["No work unit selected.".to_string()],
        };
    };
    let Some(detail) = state.work_unit_details.get(work_unit_id) else {
        return CollaborationHistoryViewModel {
            title: format!("History {}", short_id(work_unit_id)),
            lines: vec!["Loading history...".to_string()],
        };
    };

    let mut lines = Vec::new();
    lines.push("Assignments".to_string());
    if detail.assignments.is_empty() {
        lines.push("  none".to_string());
    } else {
        for assignment in detail.assignments.iter().rev().take(6) {
            let current = if detail.work_unit.current_assignment_id.as_deref()
                == Some(assignment.id.as_str())
            {
                " current"
            } else {
                ""
            };
            lines.push(format!(
                "  {} [{}] attempt={} worker={} session={}{} @ {}",
                short_id(&assignment.id),
                assignment_status_label(assignment.status),
                assignment.attempt_number,
                abbreviate(&assignment.worker_id, 12),
                short_id(&assignment.worker_session_id),
                current,
                timestamp_label(assignment.updated_at)
            ));
        }
    }

    lines.push(String::new());
    lines.push("Reports".to_string());
    if detail.reports.is_empty() {
        lines.push("  none".to_string());
    } else {
        for report in detail.reports.iter().rev().take(6) {
            let review = if report.needs_supervisor_review {
                " review=true"
            } else {
                " review=false"
            };
            lines.push(format!(
                "  {} [{} {}{}] conf={} @ {}",
                short_id(&report.id),
                report_disposition_label(report.disposition),
                report_parse_result_label(report.parse_result),
                review,
                report_confidence_label(report.confidence),
                timestamp_label(report.created_at)
            ));
            lines.push(format!(
                "    {}",
                abbreviate(&compact_line(&report.summary), 88)
            ));
        }
    }

    lines.push(String::new());
    lines.push("Decisions".to_string());
    if detail.decisions.is_empty() {
        lines.push("  none".to_string());
    } else {
        for decision in detail.decisions.iter().rev().take(6) {
            lines.push(format!(
                "  {} [{}] @ {}",
                short_id(&decision.id),
                decision_type_label(decision.decision_type),
                timestamp_label(decision.created_at)
            ));
            lines.push(format!(
                "    {}",
                abbreviate(&compact_line(&decision.rationale), 88)
            ));
        }
    }

    lines.push(String::new());
    lines.push("Proposals".to_string());
    if detail.proposals.is_empty() {
        lines.push("  none".to_string());
    } else {
        for proposal in detail.proposals.iter().rev().take(6) {
            lines.push(format!(
                "  {} [{}] decision={} edits={} @ {}",
                short_id(&proposal.id),
                proposal_status_label(proposal.status),
                proposal
                    .approved_proposal
                    .as_ref()
                    .or(proposal.proposal.as_ref())
                    .map(|proposal| decision_type_label(proposal.proposed_decision.decision_type))
                    .unwrap_or("-"),
                proposal
                    .approval_edits
                    .as_ref()
                    .is_some_and(|edits| !edits.is_empty()),
                timestamp_label(proposal.created_at)
            ));
            if let Some(summary) = proposal
                .approved_proposal
                .as_ref()
                .map(|proposal| &proposal.summary)
                .or_else(|| proposal.proposal.as_ref().map(|proposal| &proposal.summary))
            {
                lines.push(format!(
                    "    {}",
                    abbreviate(&compact_line(&summary.headline), 88)
                ));
            }
            if let Some(failure) = proposal.generation_failure.as_ref() {
                lines.push(format!(
                    "    failure {}: {}",
                    proposal_failure_stage_label(failure.stage),
                    abbreviate(&compact_line(&failure.message), 88)
                ));
            }
            if let Some(edits) = proposal
                .approval_edits
                .as_ref()
                .filter(|edits| !edits.is_empty())
            {
                lines.push(format!("    edits {}", proposal_edits_label(edits)));
            }
            if let Some(approved) = proposal.approved_proposal.as_ref() {
                lines.push(format!(
                    "    approved {} next_assignment={}",
                    decision_type_label(approved.proposed_decision.decision_type),
                    proposal.approved_assignment_id.is_some()
                ));
            }
            if let Some(note) = proposal.review_note.as_ref() {
                lines.push(format!("    note {}", abbreviate(&compact_line(note), 88)));
            }
            if let Some(output) = proposal.reasoner_output_text.as_ref() {
                lines.push(format!("    raw {}", abbreviate(&compact_line(output), 88)));
            }
        }
    }

    CollaborationHistoryViewModel {
        title: format!("History {}", abbreviate(&detail.work_unit.title, 24)),
        lines,
    }
}

pub fn collaboration_view(state: &AppState) -> CollaborationViewModel {
    CollaborationViewModel {
        status: collaboration_status(state),
        workstreams: workstream_list(state),
        workstream_detail: workstream_detail(state),
        work_units: work_unit_list(state),
        detail: collaboration_detail(state),
        history: collaboration_history(state),
    }
}

fn latest_report_for_work_unit<'a>(
    state: &'a AppState,
    work_unit_id: &str,
) -> Option<&'a ipc::ReportSummary> {
    state
        .collaboration
        .reports
        .iter()
        .filter(|report| report.work_unit_id == work_unit_id)
        .max_by(|left, right| left.created_at.cmp(&right.created_at))
}

fn latest_decision_for_work_unit<'a>(
    state: &'a AppState,
    work_unit_id: &str,
) -> Option<&'a ipc::DecisionSummary> {
    state
        .collaboration
        .decisions
        .iter()
        .filter(|decision| decision.work_unit_id == work_unit_id)
        .max_by(|left, right| left.created_at.cmp(&right.created_at))
}

fn collaboration_status_label(status: WorkstreamStatus) -> String {
    match status {
        WorkstreamStatus::Active => "active".to_string(),
        WorkstreamStatus::Blocked => "blocked".to_string(),
        WorkstreamStatus::Completed => "completed".to_string(),
    }
}

fn work_unit_status_label(status: WorkUnitStatus) -> String {
    match status {
        WorkUnitStatus::Ready => "ready".to_string(),
        WorkUnitStatus::Blocked => "blocked".to_string(),
        WorkUnitStatus::Running => "running".to_string(),
        WorkUnitStatus::AwaitingDecision => "awaiting_decision".to_string(),
        WorkUnitStatus::Accepted => "accepted".to_string(),
        WorkUnitStatus::NeedsHuman => "needs_human".to_string(),
        WorkUnitStatus::Completed => "completed".to_string(),
    }
}

fn assignment_status_label(status: AssignmentStatus) -> String {
    match status {
        AssignmentStatus::Created => "created".to_string(),
        AssignmentStatus::Running => "running".to_string(),
        AssignmentStatus::AwaitingDecision => "awaiting_decision".to_string(),
        AssignmentStatus::Failed => "failed".to_string(),
        AssignmentStatus::Closed => "closed".to_string(),
        AssignmentStatus::Interrupted => "interrupted".to_string(),
        AssignmentStatus::Lost => "lost".to_string(),
    }
}

fn report_parse_result_label(result: ReportParseResult) -> &'static str {
    match result {
        ReportParseResult::Parsed => "parsed",
        ReportParseResult::Ambiguous => "ambiguous",
        ReportParseResult::Invalid => "invalid",
    }
}

fn report_disposition_label(disposition: ReportDisposition) -> &'static str {
    match disposition {
        ReportDisposition::Completed => "completed",
        ReportDisposition::Partial => "partial",
        ReportDisposition::Blocked => "blocked",
        ReportDisposition::Failed => "failed",
        ReportDisposition::Interrupted => "interrupted",
        ReportDisposition::Unknown => "unknown",
    }
}

fn report_confidence_label(confidence: ReportConfidence) -> &'static str {
    match confidence {
        ReportConfidence::Low => "low",
        ReportConfidence::Medium => "medium",
        ReportConfidence::High => "high",
        ReportConfidence::Unknown => "unknown",
    }
}

fn decision_type_label(decision_type: DecisionType) -> &'static str {
    match decision_type {
        DecisionType::Accept => "accept",
        DecisionType::Continue => "continue",
        DecisionType::Redirect => "redirect",
        DecisionType::MarkComplete => "mark_complete",
        DecisionType::EscalateToHuman => "escalate_to_human",
    }
}

fn proposal_summary_label(summary: &ipc::WorkUnitProposalSummary) -> String {
    let mut label = proposal_status_label(summary.latest_status).to_string();
    if let Some(decision) = summary.latest_proposed_decision_type {
        label.push('/');
        label.push_str(decision_type_label(decision));
    }
    if summary.latest_has_approval_edits {
        label.push_str(" edited");
    }
    if let Some(stage) = summary.latest_failure_stage {
        label.push('/');
        label.push_str(proposal_failure_stage_label(stage));
    }
    label
}

fn proposal_status_label(status: SupervisorProposalStatus) -> &'static str {
    match status {
        SupervisorProposalStatus::Open => "open",
        SupervisorProposalStatus::Approved => "approved",
        SupervisorProposalStatus::Rejected => "rejected",
        SupervisorProposalStatus::Superseded => "superseded",
        SupervisorProposalStatus::Stale => "stale",
        SupervisorProposalStatus::GenerationFailed => "generation_failed",
    }
}

fn proposal_failure_stage_label(stage: SupervisorProposalFailureStage) -> &'static str {
    match stage {
        SupervisorProposalFailureStage::Backend => "backend",
        SupervisorProposalFailureStage::ResponseMalformed => "response_malformed",
        SupervisorProposalFailureStage::ProposalMalformed => "proposal_malformed",
        SupervisorProposalFailureStage::ProposalValidation => "proposal_validation",
    }
}

fn proposal_edits_label(edits: &SupervisorProposalEdits) -> String {
    let mut parts = Vec::new();
    if let Some(decision_type) = edits.decision_type {
        parts.push(format!("decision={}", decision_type_label(decision_type)));
    }
    if let Some(worker_id) = edits.preferred_worker_id.as_ref() {
        parts.push(format!("worker={worker_id}"));
    }
    if let Some(worker_kind) = edits.worker_kind.as_ref() {
        parts.push(format!("kind={worker_kind}"));
    }
    if let Some(objective) = edits.objective.as_ref() {
        parts.push(format!(
            "objective={}",
            abbreviate(&compact_line(objective), 28)
        ));
    }
    if !edits.instructions.is_empty() {
        parts.push(format!("instructions={}", edits.instructions.len()));
    }
    if !edits.stop_conditions.is_empty() {
        parts.push(format!("stop={}", edits.stop_conditions.len()));
    }
    if !edits.expected_report_fields.is_empty() {
        parts.push(format!(
            "report_fields={}",
            edits.expected_report_fields.len()
        ));
    }
    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(" ")
    }
}
