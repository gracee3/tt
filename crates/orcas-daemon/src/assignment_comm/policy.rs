use orcas_core::{
    Assignment, AssignmentCommunicationPacket, AssignmentCommunicationRecord, AssignmentTaskMode,
    OrcasError, OrcasResult, ReportConfidence, ReportDisposition, ReportParseResult,
    WorkerReportEnvelope,
};

use crate::assignment_comm::{
    ASSIGNMENT_COMMUNICATION_PACKET_SCHEMA_VERSION, EnvelopeValidationResult, REPORT_MARKER_BEGIN,
    REPORT_MARKER_END, WORKER_REPORT_CONTRACT_SCHEMA_VERSION,
    WORKER_REPORT_ENVELOPE_SCHEMA_VERSION,
};

pub fn validate_assignment_packet(packet: &AssignmentCommunicationPacket) -> OrcasResult<()> {
    if packet.schema_version != ASSIGNMENT_COMMUNICATION_PACKET_SCHEMA_VERSION {
        return Err(OrcasError::Protocol(format!(
            "unsupported assignment communication packet schema `{}`",
            packet.schema_version
        )));
    }
    if packet.assignment_id.trim().is_empty() || packet.packet_id.trim().is_empty() {
        return Err(OrcasError::Protocol(
            "assignment communication packet requires assignment_id and packet_id".to_string(),
        ));
    }
    if packet.task_mode != AssignmentTaskMode::Implement {
        return Err(OrcasError::Protocol(format!(
            "unsupported assignment task mode `{:?}` in v1 implement-mode slice",
            packet.task_mode
        )));
    }
    if packet.mode_spec.task_mode() != packet.task_mode {
        return Err(OrcasError::Protocol(
            "assignment communication packet mode_spec does not match task_mode".to_string(),
        ));
    }
    if packet.acceptance_criteria.is_empty() {
        return Err(OrcasError::Protocol(
            "assignment communication packet requires at least one acceptance criterion"
                .to_string(),
        ));
    }
    if packet.stop_conditions.is_empty() {
        return Err(OrcasError::Protocol(
            "assignment communication packet requires at least one stop condition".to_string(),
        ));
    }
    if packet.response_contract.schema_version != WORKER_REPORT_CONTRACT_SCHEMA_VERSION {
        return Err(OrcasError::Protocol(format!(
            "unsupported worker report contract schema `{}`",
            packet.response_contract.schema_version
        )));
    }
    if packet.response_contract.task_mode != packet.task_mode {
        return Err(OrcasError::Protocol(
            "worker report contract task_mode does not match packet task_mode".to_string(),
        ));
    }
    if packet.response_contract.marker_begin != REPORT_MARKER_BEGIN
        || packet.response_contract.marker_end != REPORT_MARKER_END
    {
        return Err(OrcasError::Protocol(
            "worker report contract markers do not match Orcas v1 markers".to_string(),
        ));
    }
    Ok(())
}

pub fn validate_worker_report_envelope(
    envelope: &WorkerReportEnvelope,
    assignment: &Assignment,
    record: &AssignmentCommunicationRecord,
    surrounding_text: bool,
) -> EnvelopeValidationResult {
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
            parse_result = ReportParseResult::Invalid;
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
            parse_result = ReportParseResult::Invalid;
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

    EnvelopeValidationResult {
        needs_supervisor_review: parse_result != ReportParseResult::Parsed,
        parse_result,
        structural_issues,
        semantic_issues,
        policy_violations,
    }
}
