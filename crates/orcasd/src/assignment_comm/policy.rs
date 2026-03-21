use std::time::Instant;

use orcas_core::{
    Assignment, AssignmentCommunicationPacket, AssignmentCommunicationRecord, AssignmentTaskMode,
    OrcasError, OrcasResult, ReportConfidence, ReportDisposition, ReportParseResult,
    TrackedThreadPruneWorkspaceResultStatus, WorkerReportEnvelope,
};
use tracing::{debug, warn};

use crate::assignment_comm::{
    ASSIGNMENT_COMMUNICATION_PACKET_SCHEMA_VERSION, EnvelopeValidationResult, REPORT_MARKER_BEGIN,
    REPORT_MARKER_END, WORKER_REPORT_CONTRACT_SCHEMA_VERSION,
    WORKER_REPORT_ENVELOPE_SCHEMA_VERSION,
};

pub fn validate_assignment_packet(packet: &AssignmentCommunicationPacket) -> OrcasResult<()> {
    let started_at = Instant::now();
    debug!(
        assignment_id = %packet.assignment_id,
        packet_id = %packet.packet_id,
        task_mode = ?packet.task_mode,
        "validating assignment communication packet"
    );

    let fail = |stage: &'static str, error: OrcasError| -> Result<(), OrcasError> {
        warn!(
            assignment_id = %packet.assignment_id,
            packet_id = %packet.packet_id,
            stage,
            duration_ms = started_at.elapsed().as_millis() as u64,
            error = %error,
            "assignment communication packet validation failed"
        );
        Err(error)
    };

    if packet.schema_version != ASSIGNMENT_COMMUNICATION_PACKET_SCHEMA_VERSION {
        return fail(
            "schema_version",
            OrcasError::Protocol(format!(
                "unsupported assignment communication packet schema `{}`",
                packet.schema_version
            )),
        );
    }
    if packet.assignment_id.trim().is_empty() || packet.packet_id.trim().is_empty() {
        return fail(
            "identity",
            OrcasError::Protocol(
                "assignment communication packet requires assignment_id and packet_id".to_string(),
            ),
        );
    }
    if packet.task_mode != AssignmentTaskMode::Implement {
        return fail(
            "task_mode",
            OrcasError::Protocol(format!(
                "unsupported assignment task mode `{:?}` in v1 implement-mode slice",
                packet.task_mode
            )),
        );
    }
    if packet.mode_spec.task_mode() != packet.task_mode {
        return fail(
            "mode_spec",
            OrcasError::Protocol(
                "assignment communication packet mode_spec does not match task_mode".to_string(),
            ),
        );
    }
    if packet.acceptance_criteria.is_empty() {
        return fail(
            "acceptance_criteria",
            OrcasError::Protocol(
                "assignment communication packet requires at least one acceptance criterion"
                    .to_string(),
            ),
        );
    }
    if packet.stop_conditions.is_empty() {
        return fail(
            "stop_conditions",
            OrcasError::Protocol(
                "assignment communication packet requires at least one stop condition".to_string(),
            ),
        );
    }
    if packet.response_contract.schema_version != WORKER_REPORT_CONTRACT_SCHEMA_VERSION {
        return fail(
            "response_contract_schema",
            OrcasError::Protocol(format!(
                "unsupported worker report contract schema `{}`",
                packet.response_contract.schema_version
            )),
        );
    }
    if packet.response_contract.task_mode != packet.task_mode {
        return fail(
            "response_contract_mode",
            OrcasError::Protocol(
                "worker report contract task_mode does not match packet task_mode".to_string(),
            ),
        );
    }
    if packet.response_contract.marker_begin != REPORT_MARKER_BEGIN
        || packet.response_contract.marker_end != REPORT_MARKER_END
    {
        return fail(
            "response_contract_markers",
            OrcasError::Protocol(
                "worker report contract markers do not match Orcas v1 markers".to_string(),
            ),
        );
    }
    debug!(
        assignment_id = %packet.assignment_id,
        packet_id = %packet.packet_id,
        acceptance_count = packet.acceptance_criteria.len(),
        stop_condition_count = packet.stop_conditions.len(),
        duration_ms = started_at.elapsed().as_millis() as u64,
        "assignment communication packet validated"
    );
    Ok(())
}

pub fn validate_worker_report_envelope(
    envelope: &WorkerReportEnvelope,
    assignment: &Assignment,
    record: &AssignmentCommunicationRecord,
    surrounding_text: bool,
) -> EnvelopeValidationResult {
    let started_at = Instant::now();
    debug!(
        assignment_id = %assignment.id,
        packet_id = %record.packet.packet_id,
        surrounding_text,
        "validating worker report envelope"
    );
    let mut structural_issues = Vec::new();
    let mut semantic_issues = Vec::new();
    let mut policy_violations = Vec::new();
    let mut parse_result = if surrounding_text {
        structural_issues.push(
            "worker output contained extra text outside the Orcas report envelope".to_string(),
        );
        ReportParseResult::Ambiguous
    } else {
        ReportParseResult::Parsed
    };

    if envelope.schema_version != WORKER_REPORT_ENVELOPE_SCHEMA_VERSION {
        structural_issues.push(format!(
            "unsupported worker report envelope schema `{}`",
            envelope.schema_version
        ));
        parse_result = ReportParseResult::Invalid;
    }
    if envelope.assignment_id != assignment.id {
        structural_issues.push(format!(
            "worker report assignment_id `{}` does not match assignment `{}`",
            envelope.assignment_id, assignment.id
        ));
        parse_result = ReportParseResult::Invalid;
    }
    if envelope.packet_id != record.packet.packet_id {
        structural_issues.push(format!(
            "worker report packet_id `{}` does not match packet `{}`",
            envelope.packet_id, record.packet.packet_id
        ));
        parse_result = ReportParseResult::Invalid;
    }
    if envelope.task_mode != record.packet.task_mode {
        structural_issues
            .push("worker report task_mode does not match packet task_mode".to_string());
        parse_result = ReportParseResult::Invalid;
    }
    if envelope.mode_payload.task_mode() != record.packet.task_mode {
        structural_issues
            .push("worker report mode_payload variant does not match packet task_mode".to_string());
        parse_result = ReportParseResult::Invalid;
    }
    if envelope.summary.trim().is_empty() {
        structural_issues.push("worker report summary was empty".to_string());
        parse_result = ReportParseResult::Invalid;
    }
    if envelope.disposition == ReportDisposition::Unknown {
        structural_issues.push("worker report disposition was unknown".to_string());
        parse_result = ReportParseResult::Invalid;
    }
    if envelope.confidence == ReportConfidence::Unknown {
        structural_issues.push("worker report confidence was unknown".to_string());
        parse_result = ReportParseResult::Invalid;
    }
    if envelope.review_signal.level == orcas_core::ReviewSignalLevel::Required
        && envelope.review_signal.reasons.is_empty()
    {
        semantic_issues
            .push("review_signal.level=required should include at least one reason".to_string());
        if parse_result == ReportParseResult::Parsed {
            parse_result = ReportParseResult::Ambiguous;
        }
    }

    if let Some(workspace_report) = envelope.workspace_report.as_ref() {
        if let Some(workspace_contract) = record.packet.workspace_contract.as_ref() {
            if workspace_report.tracked_thread_id != workspace_contract.tracked_thread_id {
                structural_issues.push(format!(
                    "workspace_report tracked_thread_id `{}` does not match workspace contract `{}`",
                    workspace_report.tracked_thread_id, workspace_contract.tracked_thread_id
                ));
                parse_result = ReportParseResult::Invalid;
            }
            if workspace_report.repository_root != workspace_contract.workspace.repository_root {
                semantic_issues.push(format!(
                    "workspace_report repository_root `{}` does not match workspace contract `{}`",
                    workspace_report.repository_root, workspace_contract.workspace.repository_root
                ));
                if parse_result == ReportParseResult::Parsed {
                    parse_result = ReportParseResult::Ambiguous;
                }
            }
            if workspace_report.worktree_path != workspace_contract.workspace.worktree_path {
                semantic_issues.push(format!(
                    "workspace_report worktree_path `{}` does not match workspace contract `{}`",
                    workspace_report.worktree_path, workspace_contract.workspace.worktree_path
                ));
                if parse_result == ReportParseResult::Parsed {
                    parse_result = ReportParseResult::Ambiguous;
                }
            }
            if workspace_report.branch_name != workspace_contract.workspace.branch_name {
                semantic_issues.push(format!(
                    "workspace_report branch_name `{}` does not match workspace contract `{}`",
                    workspace_report.branch_name, workspace_contract.workspace.branch_name
                ));
                if parse_result == ReportParseResult::Parsed {
                    parse_result = ReportParseResult::Ambiguous;
                }
            }
            if workspace_report.base_ref != workspace_contract.workspace.base_ref {
                semantic_issues.push(format!(
                    "workspace_report base_ref `{}` does not match workspace contract `{}`",
                    workspace_report.base_ref, workspace_contract.workspace.base_ref
                ));
                if parse_result == ReportParseResult::Parsed {
                    parse_result = ReportParseResult::Ambiguous;
                }
            }
            if workspace_report.base_commit.is_some()
                && workspace_report.base_commit != workspace_contract.workspace.base_commit
            {
                semantic_issues.push(
                    "workspace_report base_commit did not match the declared workspace base_commit"
                        .to_string(),
                );
                if parse_result == ReportParseResult::Parsed {
                    parse_result = ReportParseResult::Ambiguous;
                }
            }
        } else {
            semantic_issues
                .push("workspace_report was present without a workspace contract".to_string());
            if parse_result == ReportParseResult::Parsed {
                parse_result = ReportParseResult::Ambiguous;
            }
        }
    }

    if let Some(landing_execution) = envelope.landing_execution_result.as_ref() {
        if let Some(landing_execution_contract) = record.packet.landing_execution.as_ref() {
            if landing_execution.tracked_thread_id
                != Some(landing_execution_contract.tracked_thread_id.clone())
            {
                structural_issues.push(format!(
                    "landing_execution_result tracked_thread_id `{}` does not match landing contract `{}`",
                    landing_execution
                        .tracked_thread_id
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "missing".to_string()),
                    landing_execution_contract.tracked_thread_id
                ));
                parse_result = ReportParseResult::Invalid;
            }
            if landing_execution.landing_authorization_id
                != landing_execution_contract.landing_authorization_id
            {
                structural_issues.push(format!(
                    "landing_execution_result authorization_id `{}` does not match landing contract `{}`",
                    landing_execution.landing_authorization_id,
                    landing_execution_contract.landing_authorization_id
                ));
                parse_result = ReportParseResult::Invalid;
            }
            if landing_execution.attempted_head_commit
                != landing_execution_contract.authorized_head_commit
            {
                semantic_issues.push(format!(
                    "landing_execution_result attempted_head_commit `{}` does not match authorized head `{}`",
                    landing_execution.attempted_head_commit,
                    landing_execution_contract.authorized_head_commit
                ));
                if parse_result == ReportParseResult::Parsed {
                    parse_result = ReportParseResult::Ambiguous;
                }
            }
            if landing_execution.landing_target != landing_execution_contract.landing_target {
                structural_issues.push(format!(
                    "landing_execution_result landing_target `{}` does not match landing contract `{}`",
                    landing_execution.landing_target,
                    landing_execution_contract.landing_target
                ));
                parse_result = ReportParseResult::Invalid;
            }
        } else {
            semantic_issues.push(
                "landing_execution_result was present without a landing execution contract"
                    .to_string(),
            );
            if parse_result == ReportParseResult::Parsed {
                parse_result = ReportParseResult::Ambiguous;
            }
        }
    } else if record.packet.landing_execution.is_some() {
        structural_issues.push(
            "landing execution contract was present but landing_execution_result was missing"
                .to_string(),
        );
        parse_result = ReportParseResult::Invalid;
    }

    if let Some(prune_workspace_result) = envelope.prune_workspace_result.as_ref() {
        if let Some(prune_workspace_contract) = record.packet.prune_workspace.as_ref() {
            if prune_workspace_result.tracked_thread_id
                != Some(prune_workspace_contract.tracked_thread_id.clone())
            {
                structural_issues.push(format!(
                    "prune_workspace_result tracked_thread_id `{}` does not match prune contract `{}`",
                    prune_workspace_result
                        .tracked_thread_id
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "missing".to_string()),
                    prune_workspace_contract.tracked_thread_id
                ));
                parse_result = ReportParseResult::Invalid;
            }
            if prune_workspace_result.worktree_path != prune_workspace_contract.worktree_path {
                structural_issues.push(format!(
                    "prune_workspace_result worktree_path `{}` does not match prune contract `{}`",
                    prune_workspace_result.worktree_path, prune_workspace_contract.worktree_path
                ));
                parse_result = ReportParseResult::Invalid;
            }
            if prune_workspace_result.branch_name.as_deref()
                != Some(prune_workspace_contract.branch_name.as_str())
            {
                semantic_issues.push(format!(
                    "prune_workspace_result branch_name `{}` does not match prune contract `{}`",
                    prune_workspace_result
                        .branch_name
                        .as_deref()
                        .unwrap_or("unset"),
                    prune_workspace_contract.branch_name
                ));
                if parse_result == ReportParseResult::Parsed {
                    parse_result = ReportParseResult::Ambiguous;
                }
            }
            match prune_workspace_result.status {
                TrackedThreadPruneWorkspaceResultStatus::Succeeded => {
                    if prune_workspace_result.worktree_removed != Some(true) {
                        semantic_issues.push(
                            "successful prune_workspace_result should report worktree_removed=true"
                                .to_string(),
                        );
                        if parse_result == ReportParseResult::Parsed {
                            parse_result = ReportParseResult::Ambiguous;
                        }
                    }
                }
                TrackedThreadPruneWorkspaceResultStatus::Failed => {
                    if prune_workspace_result.failure_reason.is_none() {
                        semantic_issues.push(
                            "failed prune_workspace_result should include failure_reason"
                                .to_string(),
                        );
                        if parse_result == ReportParseResult::Parsed {
                            parse_result = ReportParseResult::Ambiguous;
                        }
                    }
                }
                TrackedThreadPruneWorkspaceResultStatus::Refused => {
                    if prune_workspace_result.refusal_reason.is_none() {
                        semantic_issues.push(
                            "refused prune_workspace_result should include refusal_reason"
                                .to_string(),
                        );
                        if parse_result == ReportParseResult::Parsed {
                            parse_result = ReportParseResult::Ambiguous;
                        }
                    }
                }
            }
        } else {
            semantic_issues.push(
                "prune_workspace_result was present without a prune workspace contract".to_string(),
            );
            if parse_result == ReportParseResult::Parsed {
                parse_result = ReportParseResult::Ambiguous;
            }
        }
    } else if record.packet.prune_workspace.is_some() {
        structural_issues.push(
            "prune workspace contract was present but prune_workspace_result was missing"
                .to_string(),
        );
        parse_result = ReportParseResult::Invalid;
    }

    for result in &envelope.acceptance_results {
        if !record
            .packet
            .acceptance_criteria
            .iter()
            .any(|criterion| criterion.id == result.criterion_id)
        {
            semantic_issues.push(format!(
                "worker report referenced unknown acceptance criterion `{}`",
                result.criterion_id
            ));
            if parse_result == ReportParseResult::Parsed {
                parse_result = ReportParseResult::Ambiguous;
            }
        }
    }
    for stop_condition_id in &envelope.triggered_stop_condition_ids {
        if !record
            .packet
            .stop_conditions
            .iter()
            .any(|condition| condition.id == *stop_condition_id)
        {
            semantic_issues.push(format!(
                "worker report referenced unknown stop condition `{}`",
                stop_condition_id
            ));
            if parse_result == ReportParseResult::Parsed {
                parse_result = ReportParseResult::Ambiguous;
            }
        }
    }
    for touched_file in &envelope.touched_files {
        if !record.packet.allowed_scope.allowed_write_paths.is_empty()
            && !record
                .packet
                .allowed_scope
                .allowed_write_paths
                .iter()
                .any(|prefix| touched_file.path.starts_with(prefix))
        {
            policy_violations.push(format!(
                "touched file `{}` is outside allowed_write_paths",
                touched_file.path
            ));
        }
    }
    if !policy_violations.is_empty() && parse_result == ReportParseResult::Parsed {
        parse_result = ReportParseResult::Ambiguous;
    }

    let result = EnvelopeValidationResult {
        needs_supervisor_review: parse_result != ReportParseResult::Parsed,
        parse_result,
        structural_issues,
        semantic_issues,
        policy_violations,
    };
    let duration_ms = started_at.elapsed().as_millis() as u64;
    if result.parse_result == ReportParseResult::Parsed {
        debug!(
            assignment_id = %assignment.id,
            packet_id = %record.packet.packet_id,
            parse_result = ?result.parse_result,
            duration_ms,
            "worker report envelope validated"
        );
    } else {
        warn!(
            assignment_id = %assignment.id,
            packet_id = %record.packet.packet_id,
            parse_result = ?result.parse_result,
            structural_issue_count = result.structural_issues.len(),
            semantic_issue_count = result.semantic_issues.len(),
            policy_violation_count = result.policy_violations.len(),
            needs_supervisor_review = result.needs_supervisor_review,
            duration_ms,
            "worker report envelope validation degraded"
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use orcas_core::{
        AcceptanceCriterionStatus, Assignment, AssignmentCommunicationSeed, AssignmentModeSpec,
        AssignmentTaskMode, CollaborationState, FileChangeKind, ImplementModePayload,
        ImplementModeSpec, ReportConfidence, ReportDisposition, ReportParseResult, ReviewSignal,
        ReviewSignalLevel, TouchedFile, WorkUnit, WorkUnitStatus, WorkerReportModePayload,
        Workstream, WorkstreamStatus,
    };

    use super::{validate_assignment_packet, validate_worker_report_envelope};
    use crate::assignment_comm::render::build_assignment_communication_record;

    fn fixed_now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 3, 4, 5, 6, 7)
            .single()
            .expect("valid timestamp")
    }

    fn sample_assignment() -> Assignment {
        Assignment {
            id: "assignment-1".to_string(),
            work_unit_id: "work-unit-1".to_string(),
            plan_id: None,
            plan_version: None,
            plan_item_id: None,
            execution_kind: orcas_core::PlanExecutionKind::DirectExecution,
            alignment_rationale: None,
            worker_id: "worker-1".to_string(),
            worker_session_id: "session-1".to_string(),
            instructions: "legacy fallback text".to_string(),
            communication_seed: Some(AssignmentCommunicationSeed {
                plan_id: None,
                plan_version: None,
                plan_item_id: None,
                execution_kind: orcas_core::PlanExecutionKind::DirectExecution,
                alignment_rationale: None,
                source_decision_id: None,
                source_report_id: None,
                source_proposal_id: None,
                predecessor_assignment_id: None,
                objective: "Implement one bounded policy change.".to_string(),
                instructions: vec!["Stay inside assignment_comm.".to_string()],
                acceptance_criteria: vec!["Return a valid worker report.".to_string()],
                stop_conditions: vec!["Stop when blocked.".to_string()],
                required_context_refs: Vec::new(),
                expected_report_fields: Vec::new(),
                boundedness_note: Some("Do not broaden scope.".to_string()),
                workspace_operation: None,
                prune_workspace: None,
                landing_execution: None,
                mode_spec: AssignmentModeSpec::Implement(ImplementModeSpec {
                    expected_verification_commands: vec![
                        "cargo test -p orcasd assignment_comm".to_string(),
                    ],
                }),
            }),
            status: Default::default(),
            attempt_number: 1,
            created_at: fixed_now(),
            updated_at: fixed_now(),
        }
    }

    fn sample_workstream() -> Workstream {
        Workstream {
            id: "workstream-1".to_string(),
            title: "Workstream".to_string(),
            objective: "Policy objective".to_string(),
            status: WorkstreamStatus::Active,
            priority: "high".to_string(),
            created_at: fixed_now(),
            updated_at: fixed_now(),
        }
    }

    fn sample_work_unit() -> WorkUnit {
        WorkUnit {
            id: "work-unit-1".to_string(),
            workstream_id: "workstream-1".to_string(),
            title: "Work Unit".to_string(),
            task_statement: "Implement bounded validation.".to_string(),
            status: WorkUnitStatus::Ready,
            dependencies: Vec::new(),
            latest_report_id: None,
            current_assignment_id: None,
            created_at: fixed_now(),
            updated_at: fixed_now(),
        }
    }

    fn sample_record(assignment: &Assignment) -> orcas_core::AssignmentCommunicationRecord {
        let mut collaboration = CollaborationState::default();
        collaboration
            .workstreams
            .insert("workstream-1".to_string(), sample_workstream());
        collaboration
            .work_units
            .insert("work-unit-1".to_string(), sample_work_unit());
        build_assignment_communication_record(
            &collaboration,
            assignment,
            Some("gpt-test".to_string()),
            Some("/repo".to_string()),
            None,
            None,
            fixed_now(),
        )
        .expect("build communication record")
    }

    fn sample_envelope(
        assignment: &Assignment,
        packet_id: &str,
    ) -> orcas_core::WorkerReportEnvelope {
        orcas_core::WorkerReportEnvelope {
            schema_version: "worker_report_envelope.v1".to_string(),
            assignment_id: assignment.id.clone(),
            packet_id: packet_id.to_string(),
            task_mode: AssignmentTaskMode::Implement,
            disposition: ReportDisposition::Completed,
            summary: "Completed the bounded change.".to_string(),
            confidence: ReportConfidence::High,
            acceptance_results: vec![orcas_core::AcceptanceResult {
                criterion_id: "acceptance_1".to_string(),
                status: AcceptanceCriterionStatus::Met,
                note: None,
            }],
            triggered_stop_condition_ids: vec!["stop_1".to_string()],
            touched_files: vec![TouchedFile {
                path: "/repo/src/lib.rs".to_string(),
                change_kind: FileChangeKind::Modified,
                summary: "Tightened the policy boundary.".to_string(),
            }],
            commands_run: vec!["cargo test -p orcasd assignment_comm".to_string()],
            artifacts: Vec::new(),
            blockers: Vec::new(),
            questions: Vec::new(),
            recommended_next_actions: Vec::new(),
            uncertainties: Vec::new(),
            review_signal: ReviewSignal {
                level: ReviewSignalLevel::Normal,
                reasons: Vec::new(),
                focus: Vec::new(),
            },
            workspace_report: None,
            prune_workspace_result: None,
            landing_execution_result: None,
            mode_payload: WorkerReportModePayload::Implement(ImplementModePayload {
                semantic_changes: vec!["Adjusted validation semantics.".to_string()],
                tests_run: vec!["cargo test -p orcasd assignment_comm".to_string()],
                rough_edges: Vec::new(),
            }),
        }
    }

    #[test]
    fn validate_assignment_packet_accepts_rendered_packet() {
        let assignment = sample_assignment();
        let record = sample_record(&assignment);

        validate_assignment_packet(&record.packet).expect("packet should validate");
    }

    #[test]
    fn validate_assignment_packet_rejects_marker_mismatch() {
        let assignment = sample_assignment();
        let mut record = sample_record(&assignment);
        record.packet.response_contract.marker_begin = "WRONG_BEGIN".to_string();
        record.packet.response_contract.marker_end = "WRONG_END".to_string();

        let error = validate_assignment_packet(&record.packet).expect_err("packet should fail");
        assert!(
            error
                .to_string()
                .contains("worker report contract markers do not match Orcas v1 markers")
        );
    }

    #[test]
    fn validate_assignment_packet_rejects_mode_spec_task_mode_mismatch() {
        let assignment = sample_assignment();
        let mut record = sample_record(&assignment);
        record.packet.task_mode = AssignmentTaskMode::Debug;

        let error = validate_assignment_packet(&record.packet).expect_err("packet should fail");
        assert!(
            error
                .to_string()
                .contains("unsupported assignment task mode `Debug`")
        );
    }

    #[test]
    fn validate_worker_report_envelope_accepts_clean_valid_report() {
        let assignment = sample_assignment();
        let record = sample_record(&assignment);
        let envelope = sample_envelope(&assignment, &record.packet.packet_id);

        let validation = validate_worker_report_envelope(&envelope, &assignment, &record, false);

        assert_eq!(validation.parse_result, ReportParseResult::Parsed);
        assert!(!validation.needs_supervisor_review);
        assert!(validation.structural_issues.is_empty());
        assert!(validation.semantic_issues.is_empty());
        assert!(validation.policy_violations.is_empty());
    }

    #[test]
    fn validate_worker_report_envelope_accepts_matching_workspace_report() {
        let assignment = sample_assignment();
        let mut record = sample_record(&assignment);
        record.packet.workspace_contract = Some(orcas_core::AssignmentWorkspaceContract {
            tracked_thread_id: orcas_core::authority::TrackedThreadId::parse("tt-1")
                .expect("tracked thread id"),
            tracked_thread_title: "Workspace thread".to_string(),
            workspace: orcas_core::authority::TrackedThreadWorkspace {
                repository_root: "/repo".to_string(),
                owner_tracked_thread_id: orcas_core::authority::TrackedThreadId::parse("tt-1")
                    .expect("tracked thread id"),
                strategy:
                    orcas_core::authority::TrackedThreadWorkspaceStrategy::DedicatedThreadWorktree,
                worktree_path: "/repo/.worktrees/tt-1".to_string(),
                branch_name: "orcas/tt-1".to_string(),
                base_ref: "origin/main".to_string(),
                base_commit: Some("base-123".to_string()),
                landing_target: "main".to_string(),
                landing_policy:
                    orcas_core::authority::TrackedThreadWorkspaceLandingPolicy::MergeToMain,
                sync_policy:
                    orcas_core::authority::TrackedThreadWorkspaceSyncPolicy::RebaseBeforeCompletion,
                cleanup_policy:
                    orcas_core::authority::TrackedThreadWorkspaceCleanupPolicy::PruneAfterMerge,
                last_reported_head_commit: None,
                status: orcas_core::authority::TrackedThreadWorkspaceStatus::Ready,
            },
        });
        let mut envelope = sample_envelope(&assignment, &record.packet.packet_id);
        envelope.workspace_report = Some(orcas_core::WorkerWorkspaceReport {
            tracked_thread_id: orcas_core::authority::TrackedThreadId::parse("tt-1")
                .expect("tracked thread id"),
            repository_root: "/repo".to_string(),
            worktree_path: "/repo/.worktrees/tt-1".to_string(),
            branch_name: "orcas/tt-1".to_string(),
            base_ref: "origin/main".to_string(),
            base_commit: Some("base-123".to_string()),
            head_commit: Some("head-456".to_string()),
            workspace_status: orcas_core::authority::TrackedThreadWorkspaceStatus::Ahead,
            worktree_created: Some(false),
            worktree_reused: Some(true),
            workspace_dirty: Some(false),
            rebase_attempted: Some(true),
            rebase_succeeded: Some(true),
        });

        let validation = validate_worker_report_envelope(&envelope, &assignment, &record, false);

        assert_eq!(validation.parse_result, ReportParseResult::Parsed);
        assert!(!validation.needs_supervisor_review);
        assert!(validation.structural_issues.is_empty());
        assert!(validation.semantic_issues.is_empty());
    }

    #[test]
    fn validate_worker_report_envelope_accepts_matching_landing_execution_result() {
        let assignment = sample_assignment();
        let mut record = sample_record(&assignment);
        record.packet.landing_execution = Some(orcas_core::TrackedThreadLandingExecutionContract {
            tracked_thread_id: orcas_core::authority::TrackedThreadId::parse("tt-1")
                .expect("tracked thread id"),
            tracked_thread_title: "Landing thread".to_string(),
            landing_authorization_id: "landing-auth-1".to_string(),
            authorized_head_commit: "head-123".to_string(),
            landing_target: "origin/main".to_string(),
            requested_by: Some("supervisor_cli_operator".to_string()),
            request_note: None,
        });
        let mut envelope = sample_envelope(&assignment, &record.packet.packet_id);
        envelope.landing_execution_result = Some(orcas_core::TrackedThreadLandingExecutionResult {
            tracked_thread_id: Some(
                orcas_core::authority::TrackedThreadId::parse("tt-1").expect("tracked thread id"),
            ),
            landing_authorization_id: "landing-auth-1".to_string(),
            attempted_head_commit: "head-123".to_string(),
            landing_target: "origin/main".to_string(),
            status: orcas_core::TrackedThreadLandingExecutionResultStatus::Succeeded,
            landed_commit: Some("head-456".to_string()),
            landing_ref_updated: Some(true),
            failure_reason: None,
            notes: None,
        });

        let validation = validate_worker_report_envelope(&envelope, &assignment, &record, false);

        assert_eq!(validation.parse_result, ReportParseResult::Parsed);
        assert!(validation.structural_issues.is_empty());
        assert!(validation.semantic_issues.is_empty());
    }

    #[test]
    fn validate_worker_report_envelope_downgrades_surrounding_noise_to_ambiguous() {
        let assignment = sample_assignment();
        let record = sample_record(&assignment);
        let envelope = sample_envelope(&assignment, &record.packet.packet_id);

        let validation = validate_worker_report_envelope(&envelope, &assignment, &record, true);

        assert_eq!(validation.parse_result, ReportParseResult::Ambiguous);
        assert!(validation.needs_supervisor_review);
        assert!(
            validation
                .structural_issues
                .iter()
                .any(|issue| issue.contains("extra text outside the Orcas report envelope"))
        );
    }

    #[test]
    fn validate_worker_report_envelope_marks_required_review_without_reasons_ambiguous() {
        let assignment = sample_assignment();
        let record = sample_record(&assignment);
        let mut envelope = sample_envelope(&assignment, &record.packet.packet_id);
        envelope.review_signal.level = ReviewSignalLevel::Required;

        let validation = validate_worker_report_envelope(&envelope, &assignment, &record, false);

        assert_eq!(validation.parse_result, ReportParseResult::Ambiguous);
        assert!(validation.needs_supervisor_review);
        assert!(
            validation
                .semantic_issues
                .iter()
                .any(|issue| issue.contains("review_signal.level=required"))
        );
    }

    #[test]
    fn validate_worker_report_envelope_treats_unknown_acceptance_criterion_as_ambiguous() {
        let assignment = sample_assignment();
        let record = sample_record(&assignment);
        let mut envelope = sample_envelope(&assignment, &record.packet.packet_id);
        envelope.acceptance_results[0].criterion_id = "acceptance_unknown".to_string();

        let validation = validate_worker_report_envelope(&envelope, &assignment, &record, false);

        assert_eq!(validation.parse_result, ReportParseResult::Ambiguous);
        assert!(validation.needs_supervisor_review);
        assert!(
            validation
                .semantic_issues
                .iter()
                .any(|issue| issue.contains("unknown acceptance criterion"))
        );
    }

    #[test]
    fn validate_worker_report_envelope_treats_out_of_scope_file_touches_as_ambiguous() {
        let assignment = sample_assignment();
        let record = sample_record(&assignment);
        let mut envelope = sample_envelope(&assignment, &record.packet.packet_id);
        envelope.touched_files = vec![TouchedFile {
            path: "/outside/src/lib.rs".to_string(),
            change_kind: FileChangeKind::Modified,
            summary: "Outside the allowed repo root.".to_string(),
        }];

        let validation = validate_worker_report_envelope(&envelope, &assignment, &record, false);

        assert_eq!(validation.parse_result, ReportParseResult::Ambiguous);
        assert!(validation.needs_supervisor_review);
        assert_eq!(validation.policy_violations.len(), 1);
        assert!(validation.policy_violations[0].contains("outside allowed_write_paths"));
    }

    #[test]
    fn validate_worker_report_envelope_rejects_identity_mismatch_even_with_noise() {
        let assignment = sample_assignment();
        let record = sample_record(&assignment);
        let mut envelope = sample_envelope(&assignment, &record.packet.packet_id);
        envelope.assignment_id = "assignment-other".to_string();

        let validation = validate_worker_report_envelope(&envelope, &assignment, &record, true);

        assert_eq!(validation.parse_result, ReportParseResult::Invalid);
        assert!(validation.needs_supervisor_review);
        assert!(
            validation
                .structural_issues
                .iter()
                .any(|issue| issue.contains("does not match assignment"))
        );
    }
}
