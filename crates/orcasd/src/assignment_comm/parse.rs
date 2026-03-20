use std::time::Instant;

use chrono::Utc;
use tracing::{debug, warn};

use orcas_core::{
    Assignment, AssignmentCommunicationRecord, ImplementModePayload, ReportConfidence,
    ReportDisposition, ReportParseResult, WorkerReportEnvelope, WorkerReportModePayload,
    WorkerReportValidation, ipc,
};

use crate::assignment_comm::{
    EnvelopeExtraction, REPORT_MARKER_BEGIN, REPORT_MARKER_END,
    policy::validate_worker_report_envelope,
};

#[derive(Debug, Clone)]
pub struct ParsedWorkerReport {
    pub envelope: Option<WorkerReportEnvelope>,
    pub validation: WorkerReportValidation,
    pub disposition: ReportDisposition,
    pub summary: String,
    pub findings: Vec<String>,
    pub blockers: Vec<String>,
    pub questions: Vec<String>,
    pub recommended_next_actions: Vec<String>,
    pub confidence: ReportConfidence,
}

pub fn parse_worker_report_for_turn(
    raw_output: &str,
    lifecycle: ipc::TurnLifecycleState,
    assignment: &Assignment,
    record: &AssignmentCommunicationRecord,
) -> ParsedWorkerReport {
    let started_at = Instant::now();
    debug!(
        assignment_id = %assignment.id,
        packet_id = %record.packet.packet_id,
        lifecycle = ?lifecycle,
        raw_output_len = raw_output.len(),
        "parsing worker report for turn outcome"
    );
    let mut parsed = parse_worker_report(raw_output, assignment, record);
    match lifecycle {
        ipc::TurnLifecycleState::Interrupted => {
            parsed.disposition = ReportDisposition::Interrupted;
            parsed.summary = if raw_output.trim().is_empty() {
                "Execution was interrupted before a valid Orcas report envelope was produced."
                    .to_string()
            } else {
                "Execution was interrupted. Raw output was retained for supervisor review."
                    .to_string()
            };
            parsed.findings.clear();
            parsed.blockers.clear();
            parsed.questions.clear();
            parsed.recommended_next_actions.clear();
            parsed.confidence = ReportConfidence::Unknown;
            if parsed.validation.parse_result == ReportParseResult::Parsed {
                parsed.validation.parse_result = ReportParseResult::Ambiguous;
            }
            parsed.validation.needs_supervisor_review = true;
            parsed.validation.semantic_issues.push(
                "runtime interrupted the turn before Orcas could trust the report as authoritative"
                    .to_string(),
            );
            warn!(
                assignment_id = %assignment.id,
                packet_id = %record.packet.packet_id,
                lifecycle = ?lifecycle,
                parse_result = ?parsed.validation.parse_result,
                needs_supervisor_review = parsed.validation.needs_supervisor_review,
                duration_ms = started_at.elapsed().as_millis() as u64,
                "worker report downgraded due to interrupted turn lifecycle"
            );
            parsed
        }
        ipc::TurnLifecycleState::Lost | ipc::TurnLifecycleState::Unknown => {
            parsed.disposition = ReportDisposition::Failed;
            parsed.summary = if raw_output.trim().is_empty() {
                "Execution lost runtime continuity before a valid Orcas report envelope was produced."
                    .to_string()
            } else {
                "Execution lost runtime continuity. Raw output was retained for supervisor review."
                    .to_string()
            };
            parsed.findings.clear();
            parsed.blockers.clear();
            parsed.questions.clear();
            parsed.recommended_next_actions.clear();
            parsed.confidence = ReportConfidence::Unknown;
            if parsed.validation.parse_result == ReportParseResult::Parsed {
                parsed.validation.parse_result = ReportParseResult::Ambiguous;
            }
            parsed.validation.needs_supervisor_review = true;
            parsed.validation.semantic_issues.push(
                "runtime continuity was lost before Orcas could trust the report as authoritative"
                    .to_string(),
            );
            warn!(
                assignment_id = %assignment.id,
                packet_id = %record.packet.packet_id,
                lifecycle = ?lifecycle,
                parse_result = ?parsed.validation.parse_result,
                needs_supervisor_review = parsed.validation.needs_supervisor_review,
                duration_ms = started_at.elapsed().as_millis() as u64,
                "worker report downgraded due to lost turn lifecycle"
            );
            parsed
        }
        _ => {
            debug!(
                assignment_id = %assignment.id,
                packet_id = %record.packet.packet_id,
                parse_result = ?parsed.validation.parse_result,
                needs_supervisor_review = parsed.validation.needs_supervisor_review,
                duration_ms = started_at.elapsed().as_millis() as u64,
                "worker report parsed for turn outcome"
            );
            parsed
        }
    }
}

pub fn parse_worker_report(
    raw_output: &str,
    assignment: &Assignment,
    record: &AssignmentCommunicationRecord,
) -> ParsedWorkerReport {
    let started_at = Instant::now();
    debug!(
        assignment_id = %assignment.id,
        packet_id = %record.packet.packet_id,
        raw_output_len = raw_output.len(),
        "parsing worker report envelope"
    );
    let fallback = |structural_issue: Option<String>| {
        let mut structural_issues = Vec::new();
        if let Some(issue) = structural_issue {
            structural_issues.push(issue);
        }
        ParsedWorkerReport {
            envelope: None,
            validation: WorkerReportValidation {
                validated_at: Utc::now(),
                parse_result: ReportParseResult::Invalid,
                structural_issues,
                semantic_issues: Vec::new(),
                policy_violations: Vec::new(),
                needs_supervisor_review: true,
            },
            disposition: ReportDisposition::Unknown,
            summary:
                "Worker output retained for supervisor review because the structured report was invalid or incomplete."
                    .to_string(),
            findings: Vec::new(),
            blockers: Vec::new(),
            questions: Vec::new(),
            recommended_next_actions: Vec::new(),
            confidence: ReportConfidence::Unknown,
        }
    };

    let extraction = extract_envelope(raw_output);
    let Some(json_payload) = extraction.json_payload else {
        warn!(
            assignment_id = %assignment.id,
            packet_id = %record.packet.packet_id,
            stage = "extract_envelope",
            duration_ms = started_at.elapsed().as_millis() as u64,
            "worker report envelope extraction failed"
        );
        return fallback(Some(
            "worker output did not contain exactly one Orcas report envelope".to_string(),
        ));
    };
    debug!(
        assignment_id = %assignment.id,
        packet_id = %record.packet.packet_id,
        stage = "extract_envelope",
        surrounding_text = extraction.surrounding_text,
        envelope_bytes = json_payload.len(),
        "worker report envelope extracted"
    );

    let envelope: WorkerReportEnvelope = match serde_json::from_str(json_payload.trim()) {
        Ok(envelope) => envelope,
        Err(error) => {
            warn!(
                assignment_id = %assignment.id,
                packet_id = %record.packet.packet_id,
                stage = "decode_envelope",
                duration_ms = started_at.elapsed().as_millis() as u64,
                error = %error,
                "worker report envelope decode failed"
            );
            return fallback(Some(format!(
                "worker report envelope JSON could not be decoded: {error}"
            )));
        }
    };

    let validation =
        validate_worker_report_envelope(&envelope, assignment, record, extraction.surrounding_text);
    let report_validation = WorkerReportValidation {
        validated_at: Utc::now(),
        parse_result: validation.parse_result,
        structural_issues: validation.structural_issues,
        semantic_issues: validation.semantic_issues,
        policy_violations: validation.policy_violations,
        needs_supervisor_review: validation.needs_supervisor_review,
    };

    if report_validation.parse_result == ReportParseResult::Invalid {
        warn!(
            assignment_id = %assignment.id,
            packet_id = %record.packet.packet_id,
            stage = "validate_envelope",
            parse_result = ?report_validation.parse_result,
            structural_issue_count = report_validation.structural_issues.len(),
            semantic_issue_count = report_validation.semantic_issues.len(),
            policy_violation_count = report_validation.policy_violations.len(),
            duration_ms = started_at.elapsed().as_millis() as u64,
            "worker report validation failed"
        );
        return ParsedWorkerReport {
            envelope: Some(envelope),
            validation: report_validation,
            disposition: ReportDisposition::Unknown,
            summary:
                "Worker output retained for supervisor review because the structured report was invalid or incomplete."
                    .to_string(),
            findings: Vec::new(),
            blockers: Vec::new(),
            questions: Vec::new(),
            recommended_next_actions: Vec::new(),
            confidence: ReportConfidence::Unknown,
        };
    }

    let findings = match &envelope.mode_payload {
        WorkerReportModePayload::Implement(ImplementModePayload {
            semantic_changes, ..
        }) => semantic_changes.clone(),
    };

    let parsed = ParsedWorkerReport {
        disposition: envelope.disposition,
        summary: envelope.summary.clone(),
        findings,
        blockers: envelope.blockers.clone(),
        questions: envelope.questions.clone(),
        recommended_next_actions: envelope.recommended_next_actions.clone(),
        confidence: envelope.confidence,
        envelope: Some(envelope),
        validation: report_validation,
    };
    if parsed.validation.needs_supervisor_review {
        warn!(
            assignment_id = %assignment.id,
            packet_id = %record.packet.packet_id,
            stage = "finalize_report",
            parse_result = ?parsed.validation.parse_result,
            disposition = ?parsed.disposition,
            finding_count = parsed.findings.len(),
            blocker_count = parsed.blockers.len(),
            question_count = parsed.questions.len(),
            duration_ms = started_at.elapsed().as_millis() as u64,
            "worker report parsed with supervisor review required"
        );
    } else {
        debug!(
            assignment_id = %assignment.id,
            packet_id = %record.packet.packet_id,
            stage = "finalize_report",
            parse_result = ?parsed.validation.parse_result,
            disposition = ?parsed.disposition,
            finding_count = parsed.findings.len(),
            blocker_count = parsed.blockers.len(),
            question_count = parsed.questions.len(),
            duration_ms = started_at.elapsed().as_millis() as u64,
            "worker report parsed successfully"
        );
    }
    parsed
}

fn extract_envelope(raw_output: &str) -> EnvelopeExtraction {
    let Some((prefix, after_begin)) = raw_output.split_once(REPORT_MARKER_BEGIN) else {
        return EnvelopeExtraction {
            json_payload: None,
            surrounding_text: false,
        };
    };
    let Some((json_payload, suffix)) = after_begin.split_once(REPORT_MARKER_END) else {
        return EnvelopeExtraction {
            json_payload: None,
            surrounding_text: false,
        };
    };
    if after_begin.contains(REPORT_MARKER_BEGIN) || suffix.contains(REPORT_MARKER_END) {
        return EnvelopeExtraction {
            json_payload: None,
            surrounding_text: false,
        };
    }

    EnvelopeExtraction {
        json_payload: Some(json_payload.trim().to_string()),
        surrounding_text: !prefix.trim().is_empty() || !suffix.trim().is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use orcas_core::{
        Assignment, AssignmentCommunicationSeed, AssignmentModeSpec, AssignmentTaskMode,
        ImplementModePayload, ImplementModeSpec, ReportConfidence, ReportDisposition,
        ReportParseResult, ReviewSignal, ReviewSignalLevel, WorkUnit, WorkUnitStatus,
        WorkerReportModePayload, Workstream, WorkstreamStatus, ipc,
    };

    use super::{extract_envelope, parse_worker_report, parse_worker_report_for_turn};
    use crate::assignment_comm::{
        REPORT_MARKER_BEGIN, REPORT_MARKER_END, render::build_assignment_communication_record,
    };

    fn fixed_now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 1, 2, 3, 4, 5)
            .single()
            .expect("valid timestamp")
    }

    fn sample_assignment() -> Assignment {
        Assignment {
            id: "assignment-1".to_string(),
            work_unit_id: "work-unit-1".to_string(),
            worker_id: "worker-1".to_string(),
            worker_session_id: "session-1".to_string(),
            instructions: "Implement the bounded task.".to_string(),
            communication_seed: Some(AssignmentCommunicationSeed {
                source_decision_id: None,
                source_report_id: None,
                source_proposal_id: None,
                predecessor_assignment_id: None,
                objective: "Implement one bounded change.".to_string(),
                instructions: vec!["Touch only the bounded scope.".to_string()],
                acceptance_criteria: vec!["Return a valid report envelope.".to_string()],
                stop_conditions: vec!["Stop when blocked.".to_string()],
                required_context_refs: Vec::new(),
                expected_report_fields: Vec::new(),
                boundedness_note: Some("Do not broaden scope.".to_string()),
                mode_spec: AssignmentModeSpec::Implement(ImplementModeSpec {
                    expected_verification_commands: Vec::new(),
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
            objective: "Primary objective".to_string(),
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
            task_statement: "Implement the targeted change.".to_string(),
            status: WorkUnitStatus::Ready,
            dependencies: Vec::new(),
            latest_report_id: None,
            current_assignment_id: None,
            created_at: fixed_now(),
            updated_at: fixed_now(),
        }
    }

    fn sample_record(assignment: &Assignment) -> orcas_core::AssignmentCommunicationRecord {
        let mut collaboration = orcas_core::CollaborationState::default();
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
        .expect("build assignment communication record")
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
            acceptance_results: Vec::new(),
            triggered_stop_condition_ids: Vec::new(),
            touched_files: Vec::new(),
            commands_run: vec!["cargo test -p orcasd assignment_comm".to_string()],
            artifacts: Vec::new(),
            blockers: vec!["none".to_string()],
            questions: vec!["Should we add follow-up coverage?".to_string()],
            recommended_next_actions: vec!["Request supervisor review.".to_string()],
            uncertainties: Vec::new(),
            review_signal: ReviewSignal {
                level: ReviewSignalLevel::Normal,
                reasons: Vec::new(),
                focus: Vec::new(),
            },
            mode_payload: WorkerReportModePayload::Implement(ImplementModePayload {
                semantic_changes: vec!["Updated the parser boundary.".to_string()],
                tests_run: vec!["cargo test -p orcasd assignment_comm".to_string()],
                rough_edges: vec!["No additional rough edges.".to_string()],
            }),
        }
    }

    fn wrap_report(json_payload: &str) -> String {
        format!("{REPORT_MARKER_BEGIN}\n{json_payload}\n{REPORT_MARKER_END}")
    }

    #[test]
    fn extract_envelope_marks_surrounding_noise_without_losing_payload() {
        let extraction = extract_envelope(
            "worker preamble\nORCAS_REPORT_BEGIN\n{\"summary\":\"ok\"}\nORCAS_REPORT_END\nworker epilogue",
        );

        assert_eq!(
            extraction.json_payload.as_deref(),
            Some("{\"summary\":\"ok\"}")
        );
        assert!(extraction.surrounding_text);
    }

    #[test]
    fn extract_envelope_rejects_missing_end_marker() {
        let extraction = extract_envelope("ORCAS_REPORT_BEGIN\n{\"summary\":\"unterminated\"}");

        assert!(extraction.json_payload.is_none());
        assert!(!extraction.surrounding_text);
    }

    #[test]
    fn extract_envelope_rejects_multiple_envelopes_in_one_payload() {
        let extraction = extract_envelope(
            "ORCAS_REPORT_BEGIN\n{\"summary\":\"first\"}\nORCAS_REPORT_END\nORCAS_REPORT_BEGIN\n{\"summary\":\"second\"}\nORCAS_REPORT_END",
        );

        assert!(extraction.json_payload.is_none());
        assert!(!extraction.surrounding_text);
    }

    #[test]
    fn parse_worker_report_marks_surrounding_noise_as_ambiguous_but_keeps_report_contents() {
        let assignment = sample_assignment();
        let record = sample_record(&assignment);
        let envelope = sample_envelope(&assignment, &record.packet.packet_id);
        let raw = format!(
            "debug line before\n{}\nextra trailing line",
            wrap_report(&serde_json::to_string(&envelope).expect("serialize envelope"))
        );

        let parsed = parse_worker_report(&raw, &assignment, &record);

        assert_eq!(parsed.validation.parse_result, ReportParseResult::Ambiguous);
        assert!(parsed.validation.needs_supervisor_review);
        assert!(
            parsed
                .validation
                .structural_issues
                .iter()
                .any(|issue| issue.contains("extra text outside the Orcas report envelope"))
        );
        assert_eq!(parsed.disposition, ReportDisposition::Completed);
        assert_eq!(parsed.summary, "Completed the bounded change.");
        assert_eq!(
            parsed.findings,
            vec!["Updated the parser boundary.".to_string()]
        );
        assert_eq!(
            parsed.recommended_next_actions,
            vec!["Request supervisor review.".to_string()]
        );
        assert!(parsed.envelope.is_some());
    }

    #[test]
    fn parse_worker_report_rejects_missing_begin_marker() {
        let assignment = sample_assignment();
        let record = sample_record(&assignment);

        let parsed = parse_worker_report("{\"summary\":\"not wrapped\"}", &assignment, &record);

        assert!(parsed.envelope.is_none());
        assert_eq!(parsed.validation.parse_result, ReportParseResult::Invalid);
        assert!(parsed.validation.needs_supervisor_review);
        assert!(
            parsed
                .validation
                .structural_issues
                .iter()
                .any(|issue| issue.contains("did not contain exactly one Orcas report envelope"))
        );
    }

    #[test]
    fn parse_worker_report_rejects_malformed_json_inside_envelope() {
        let assignment = sample_assignment();
        let record = sample_record(&assignment);

        let parsed = parse_worker_report(&wrap_report("{ not valid json }"), &assignment, &record);

        assert!(parsed.envelope.is_none());
        assert_eq!(parsed.validation.parse_result, ReportParseResult::Invalid);
        assert!(
            parsed
                .validation
                .structural_issues
                .iter()
                .any(|issue| issue.contains("JSON could not be decoded"))
        );
    }

    #[test]
    fn interrupted_turn_downgrades_even_valid_report_and_clears_details() {
        let assignment = sample_assignment();
        let record = sample_record(&assignment);
        let raw = wrap_report(
            &serde_json::to_string(&sample_envelope(&assignment, &record.packet.packet_id))
                .expect("serialize envelope"),
        );

        let parsed = parse_worker_report_for_turn(
            &raw,
            ipc::TurnLifecycleState::Interrupted,
            &assignment,
            &record,
        );

        assert_eq!(parsed.disposition, ReportDisposition::Interrupted);
        assert_eq!(parsed.validation.parse_result, ReportParseResult::Ambiguous);
        assert!(parsed.validation.needs_supervisor_review);
        assert!(
            parsed
                .validation
                .semantic_issues
                .iter()
                .any(|issue| issue.contains("runtime interrupted the turn"))
        );
        assert!(parsed.findings.is_empty());
        assert!(parsed.blockers.is_empty());
        assert!(parsed.questions.is_empty());
        assert!(parsed.recommended_next_actions.is_empty());
        assert_eq!(parsed.confidence, ReportConfidence::Unknown);
    }
}
