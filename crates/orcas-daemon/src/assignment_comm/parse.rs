use chrono::Utc;

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
            parsed
        }
        _ => parsed,
    }
}

pub fn parse_worker_report(
    raw_output: &str,
    assignment: &Assignment,
    record: &AssignmentCommunicationRecord,
) -> ParsedWorkerReport {
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
        return fallback(Some(
            "worker output did not contain exactly one Orcas report envelope".to_string(),
        ));
    };

    let envelope: WorkerReportEnvelope = match serde_json::from_str(json_payload.trim()) {
        Ok(envelope) => envelope,
        Err(error) => {
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

    ParsedWorkerReport {
        disposition: envelope.disposition,
        summary: envelope.summary.clone(),
        findings,
        blockers: envelope.blockers.clone(),
        questions: envelope.questions.clone(),
        recommended_next_actions: envelope.recommended_next_actions.clone(),
        confidence: envelope.confidence,
        envelope: Some(envelope),
        validation: report_validation,
    }
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
