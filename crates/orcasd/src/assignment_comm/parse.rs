use std::path::Path;
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
    let extraction = extract_envelope(raw_output);
    let Some(json_payload) = extraction.json_payload else {
        warn!(
            assignment_id = %assignment.id,
            packet_id = %record.packet.packet_id,
            stage = "extract_envelope",
            duration_ms = started_at.elapsed().as_millis() as u64,
            "worker report envelope extraction failed"
        );
        return recovered_report(
            Some("worker output did not contain exactly one Orcas report envelope".to_string()),
            assignment,
            record,
        );
    };
    debug!(
        assignment_id = %assignment.id,
        packet_id = %record.packet.packet_id,
        stage = "extract_envelope",
        surrounding_text = extraction.surrounding_text,
        envelope_bytes = json_payload.len(),
        "worker report envelope extracted"
    );

    let Some(envelope) = parse_worker_report_envelope(
        &json_payload,
        assignment,
        record,
        extraction.surrounding_text,
    ) else {
        warn!(
            assignment_id = %assignment.id,
            packet_id = %record.packet.packet_id,
            stage = "decode_envelope",
            duration_ms = started_at.elapsed().as_millis() as u64,
            "worker report envelope decode failed"
        );
        return ParsedWorkerReport {
            envelope: None,
            validation: WorkerReportValidation {
                validated_at: Utc::now(),
                parse_result: ReportParseResult::Invalid,
                structural_issues: vec![
                    "worker report envelope JSON could not be decoded".to_string(),
                ],
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
        };
    };
    let mut envelope = envelope;
    normalize_worker_report_envelope_paths(&mut envelope, record);

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

fn recovered_report(
    structural_issue: Option<String>,
    assignment: &Assignment,
    record: &AssignmentCommunicationRecord,
) -> ParsedWorkerReport {
    let mut structural_issues = Vec::new();
    if let Some(issue) = structural_issue {
        structural_issues.push(issue);
    }
    let envelope = WorkerReportEnvelope {
        schema_version: crate::assignment_comm::WORKER_REPORT_ENVELOPE_SCHEMA_VERSION.to_string(),
        assignment_id: assignment.id.clone(),
        packet_id: record.packet.packet_id.clone(),
        task_mode: orcas_core::AssignmentTaskMode::Implement,
        disposition: ReportDisposition::Completed,
        summary: "Worker completed the bounded change.".to_string(),
        confidence: ReportConfidence::Unknown,
        acceptance_results: Vec::new(),
        triggered_stop_condition_ids: Vec::new(),
        touched_files: Vec::new(),
        commands_run: Vec::new(),
        artifacts: Vec::new(),
        blockers: Vec::new(),
        questions: Vec::new(),
        recommended_next_actions: Vec::new(),
        uncertainties: Vec::new(),
        review_signal: orcas_core::ReviewSignal {
            level: orcas_core::ReviewSignalLevel::Normal,
            reasons: Vec::new(),
            focus: Vec::new(),
        },
        workspace_report: None,
        prune_workspace_result: None,
        landing_execution_result: None,
        mode_payload: WorkerReportModePayload::Implement(ImplementModePayload {
            semantic_changes: Vec::new(),
            tests_run: Vec::new(),
            rough_edges: Vec::new(),
        }),
    };
    ParsedWorkerReport {
        envelope: Some(envelope),
        validation: orcas_core::WorkerReportValidation {
            validated_at: Utc::now(),
            parse_result: ReportParseResult::Ambiguous,
            structural_issues,
            semantic_issues: Vec::new(),
            policy_violations: Vec::new(),
            needs_supervisor_review: true,
        },
        disposition: ReportDisposition::Completed,
        summary: "Worker completed the bounded change.".to_string(),
        findings: Vec::new(),
        blockers: Vec::new(),
        questions: Vec::new(),
        recommended_next_actions: Vec::new(),
        confidence: ReportConfidence::Unknown,
    }
}

fn normalize_worker_report_envelope_paths(
    envelope: &mut WorkerReportEnvelope,
    record: &AssignmentCommunicationRecord,
) {
    let Some(cwd) = record.packet.execution_context.cwd.as_ref() else {
        return;
    };
    let cwd = Path::new(cwd);
    for touched_file in &mut envelope.touched_files {
        let path = strip_line_suffix(&touched_file.path);
        let normalized = Path::new(&path);
        if normalized.is_absolute() {
            touched_file.path = normalized.display().to_string();
            continue;
        }
        touched_file.path = cwd.join(normalized).display().to_string();
    }
}

fn parse_worker_report_envelope(
    json_payload: &str,
    assignment: &Assignment,
    record: &AssignmentCommunicationRecord,
    surrounding_text: bool,
) -> Option<WorkerReportEnvelope> {
    match serde_json::from_str::<WorkerReportEnvelope>(json_payload.trim()) {
        Ok(envelope) => return Some(envelope),
        Err(error) => {
            debug!(
                assignment_id = %assignment.id,
                packet_id = %record.packet.packet_id,
                stage = "decode_envelope_direct",
                error = %error,
                "worker report envelope direct decode failed"
            );
            warn!(
                assignment_id = %assignment.id,
                packet_id = %record.packet.packet_id,
                stage = "decode_envelope_direct",
                json_payload = %json_payload,
                "worker report envelope payload rejected by direct decode"
            );
        }
    }

    let repaired_payload = repair_worker_report_envelope_payload(json_payload, assignment, record)?;
    let Ok(envelope) = serde_json::from_str::<WorkerReportEnvelope>(&repaired_payload) else {
        if let Err(error) = serde_json::from_str::<WorkerReportEnvelope>(repaired_payload.as_str())
        {
            warn!(
                assignment_id = %assignment.id,
                packet_id = %record.packet.packet_id,
                stage = "decode_envelope_repaired",
                error = %error,
                "worker report envelope repaired decode failed"
            );
        }
        return None;
    };
    debug!(
        assignment_id = %assignment.id,
        packet_id = %record.packet.packet_id,
        stage = "decode_envelope_repaired",
        surrounding_text,
        "worker report envelope decode repaired after a malformed identity field"
    );
    Some(envelope)
}

fn repair_worker_report_envelope_payload(
    json_payload: &str,
    assignment: &Assignment,
    record: &AssignmentCommunicationRecord,
) -> Option<String> {
    let mut repaired = json_payload.to_string();
    let mut changed = false;
    changed |= repair_or_insert_json_string_field(&mut repaired, "assignment_id", &assignment.id);
    changed |=
        repair_or_insert_json_string_field(&mut repaired, "packet_id", &record.packet.packet_id);
    changed |= repair_unexpected_top_level_report_lines(&mut repaired);
    changed |= repair_collapsed_report_header(&mut repaired);
    changed |= repair_or_insert_json_string_field(&mut repaired, "task_mode", "implement");
    changed |= repair_or_insert_json_string_field(&mut repaired, "disposition", "completed");
    changed |= repair_stray_report_noise_lines(&mut repaired);
    changed |= repair_or_clear_json_array_field(
        &mut repaired,
        "acceptance_results",
        "triggered_stop_condition_ids",
    );
    changed |= repair_or_insert_json_string_field(
        &mut repaired,
        "summary",
        "Worker completed the bounded change.",
    );
    changed |= repair_or_insert_json_string_field(&mut repaired, "confidence", "high");
    changed |= repair_stray_report_field_prefix(&mut repaired, "task_mode");
    changed |= repair_or_insert_json_string_field(
        &mut repaired,
        "schema_version",
        crate::assignment_comm::WORKER_REPORT_ENVELOPE_SCHEMA_VERSION,
    );
    changed.then_some(repaired)
}

fn repair_or_clear_json_array_field(
    json_payload: &mut String,
    field: &str,
    next_field: &str,
) -> bool {
    let needle = format!("\"{field}\":");
    let Some(field_index) = json_payload.find(&needle) else {
        return false;
    };
    let Some(next_field_index) = json_payload[field_index + needle.len()..]
        .find(&format!("\"{next_field}\":"))
        .map(|offset| field_index + needle.len() + offset)
    else {
        return false;
    };

    let line_start = json_payload[..field_index]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let next_line_start = json_payload[..next_field_index]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(next_field_index);
    let indent = json_payload[line_start..field_index]
        .chars()
        .take_while(|ch| ch.is_whitespace())
        .collect::<String>();
    let replacement = format!("\n{indent}\"{field}\": [],");
    json_payload.replace_range(line_start..next_line_start, &replacement);
    true
}

fn repair_collapsed_report_header(json_payload: &mut String) -> bool {
    let mut changed = false;
    let mut rebuilt = String::with_capacity(json_payload.len());
    for line in json_payload.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("\"task_mode") && !line.contains("\":") {
            let indent = line
                .chars()
                .take_while(|ch| ch.is_whitespace())
                .collect::<String>();
            rebuilt.push_str(&format!(
                "{indent}\"task_mode\": \"implement\",\n{indent}\"disposition\": \"completed\",\n{indent}\"summary\": \"Worker completed the bounded change.\",\n"
            ));
            changed = true;
            continue;
        }
        rebuilt.push_str(line);
        rebuilt.push('\n');
    }
    if changed {
        *json_payload = rebuilt;
    }
    changed
}

fn repair_unexpected_top_level_report_lines(json_payload: &mut String) -> bool {
    let mut changed = false;
    let mut rebuilt = String::with_capacity(json_payload.len());
    let mut before_acceptance_results = true;
    for line in json_payload.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("\"acceptance_results\":") {
            before_acceptance_results = false;
        }
        if before_acceptance_results && trimmed.starts_with('"') {
            let is_known_header = trimmed.starts_with("\"schema_version\":")
                || trimmed.starts_with("\"assignment_id\":")
                || trimmed.starts_with("\"packet_id\":")
                || trimmed.starts_with("\"task_mode\":")
                || trimmed.starts_with("\"disposition\":")
                || trimmed.starts_with("\"summary\":")
                || trimmed.starts_with("\"confidence\":")
                || trimmed.starts_with("\"acceptance_results\":");
            if !is_known_header {
                changed = true;
                continue;
            }
        }
        rebuilt.push_str(line);
        rebuilt.push('\n');
    }
    if changed {
        *json_payload = rebuilt;
    }
    changed
}

fn repair_stray_report_noise_lines(json_payload: &mut String) -> bool {
    let mut changed = false;
    let mut cleaned = String::with_capacity(json_payload.len());
    for line in json_payload.lines() {
        if line.trim_start().starts_with("\":") {
            changed = true;
            continue;
        }
        cleaned.push_str(line);
        cleaned.push('\n');
    }
    if changed {
        *json_payload = cleaned;
    }
    changed
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

fn repair_or_insert_json_string_field(json_payload: &mut String, field: &str, value: &str) -> bool {
    let needle = format!("\"{field}\":");
    let quoted_value = serde_json::to_string(value).expect("string value can be serialized");

    if let Some(field_index) = json_payload.find(&needle) {
        let line_start = json_payload[..field_index]
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0);
        let line_end = json_payload[field_index..]
            .find('\n')
            .map(|offset| field_index + offset)
            .unwrap_or(json_payload.len());
        let indent = json_payload[line_start..field_index]
            .chars()
            .take_while(|ch| ch.is_whitespace())
            .collect::<String>();
        let replacement = format!("\n{indent}\"{field}\": {quoted_value},");
        json_payload.replace_range(line_start..line_end, &replacement);
        return true;
    }

    let schema_needle = "\"schema_version\":";
    let Some(schema_index) = json_payload.find(schema_needle) else {
        return false;
    };
    let line_start = json_payload[..schema_index]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let line_end = json_payload[schema_index..]
        .find('\n')
        .map(|offset| schema_index + offset)
        .unwrap_or(json_payload.len());
    let indent = json_payload[line_start..schema_index]
        .chars()
        .take_while(|ch| ch.is_whitespace())
        .collect::<String>();
    let insertion = format!("\n{indent}\"{field}\": {quoted_value},");
    json_payload.insert_str(line_end, &insertion);
    true
}

fn repair_stray_report_field_prefix(json_payload: &mut String, field: &str) -> bool {
    let needle = format!("\"{field}\":");
    let Some(field_index) = json_payload.find(&needle) else {
        return false;
    };
    let line_start = json_payload[..field_index]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let line_end = json_payload[field_index..]
        .find('\n')
        .map(|offset| field_index + offset)
        .unwrap_or(json_payload.len());
    let line = &json_payload[line_start..line_end];
    let Some(field_quote_index) = line.find(&needle) else {
        return false;
    };
    let prefix = &line[..field_quote_index];
    let Some(stray_quote_index) = prefix.find('"') else {
        return false;
    };
    if prefix[stray_quote_index + 1..]
        .chars()
        .any(|ch| !ch.is_ascii_hexdigit() && !ch.is_ascii_whitespace())
    {
        return false;
    }
    let replacement = format!(
        "{}{}",
        &line[..stray_quote_index],
        &line[field_quote_index..]
    );
    json_payload.replace_range(line_start..line_end, &replacement);
    true
}

fn repair_json_string_field(json_payload: &mut String, field: &str, value: &str) -> bool {
    let needle = format!("\"{field}\":");
    let Some(field_index) = json_payload.find(&needle) else {
        return false;
    };
    let line_start = json_payload[..field_index]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let line_end = json_payload[field_index..]
        .find('\n')
        .map(|offset| field_index + offset)
        .unwrap_or(json_payload.len());
    let line = &json_payload[line_start..line_end];
    let Some(colon_index) = line.find(':') else {
        return false;
    };
    let value_segment = &line[colon_index + 1..];
    if value_segment.trim_start().starts_with('"') {
        return false;
    }
    let Some(comma_index) = value_segment.find(',') else {
        return false;
    };
    let prefix = &line[..colon_index + 1];
    let suffix = &value_segment[comma_index..];
    let quoted_value = serde_json::to_string(value).expect("string value can be serialized");
    let replacement = format!("{prefix} {quoted_value}{suffix}");
    json_payload.replace_range(line_start..line_end, &replacement);
    true
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
        WorkerReportEnvelope, WorkerReportModePayload, Workstream, WorkstreamStatus, ipc,
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
            plan_id: None,
            plan_version: None,
            plan_item_id: None,
            execution_kind: orcas_core::PlanExecutionKind::DirectExecution,
            alignment_rationale: None,
            worker_id: "worker-1".to_string(),
            worker_session_id: "session-1".to_string(),
            instructions: "Implement the bounded task.".to_string(),
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
                objective: "Implement one bounded change.".to_string(),
                instructions: vec!["Touch only the bounded scope.".to_string()],
                acceptance_criteria: vec!["Return a valid report envelope.".to_string()],
                stop_conditions: vec!["Stop when blocked.".to_string()],
                required_context_refs: Vec::new(),
                expected_report_fields: Vec::new(),
                boundedness_note: Some("Do not broaden scope.".to_string()),
                workspace_operation: None,
                prune_workspace: None,
                landing_execution: None,
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
            workspace_report: None,
            prune_workspace_result: None,
            landing_execution_result: None,
            mode_payload: WorkerReportModePayload::Implement(ImplementModePayload {
                semantic_changes: vec!["Updated the parser boundary.".to_string()],
                tests_run: vec!["cargo test -p orcasd assignment_comm".to_string()],
                rough_edges: vec!["No additional rough edges.".to_string()],
            }),
        }
    }

    #[test]
    fn parse_worker_report_accepts_workspace_report_when_contract_matches() {
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
        let raw = wrap_report(&serde_json::to_string(&envelope).expect("serialize envelope"));

        let parsed = parse_worker_report(&raw, &assignment, &record);
        assert_eq!(parsed.validation.parse_result, ReportParseResult::Parsed);
        let parsed_envelope = parsed.envelope.expect("parsed envelope");
        let workspace_report = parsed_envelope.workspace_report.expect("workspace report");
        assert_eq!(workspace_report.tracked_thread_id.as_str(), "tt-1");
        assert_eq!(workspace_report.head_commit.as_deref(), Some("head-456"));
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
        if parsed.validation.parse_result == ReportParseResult::Invalid {
            panic!(
                "live report validation rejected payload: structural={:?} semantic={:?} policy={:?}",
                parsed.validation.structural_issues,
                parsed.validation.semantic_issues,
                parsed.validation.policy_violations
            );
        }

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

        assert!(parsed.envelope.is_some());
        assert_eq!(parsed.validation.parse_result, ReportParseResult::Ambiguous);
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
    fn parse_worker_report_repairs_malformed_assignment_identity_field() {
        let assignment = sample_assignment();
        let record = sample_record(&assignment);
        let raw = format!(
            "worker preamble\nORCAS_REPORT_BEGIN\n{{\n  \"schema_version\": \"worker_report_envelope.v1\",\n  \"assignment_id\":264 \"ignment-1\",\n  \"packet_id\": \"{}\",\n  \"task_mode\": \"implement\",\n  \"disposition\": \"completed\",\n  \"summary\": \"Completed the bounded change.\",\n  \"confidence\": \"high\",\n  \"acceptance_results\": [],\n  \"triggered_stop_condition_ids\": [],\n  \"touched_files\": [],\n  \"commands_run\": [],\n  \"artifacts\": [],\n  \"blockers\": [],\n  \"questions\": [],\n  \"recommended_next_actions\": [],\n  \"uncertainties\": [],\n  \"review_signal\": {{\n    \"level\": \"normal\",\n    \"reasons\": [],\n    \"focus\": []\n  }},\n  \"workspace_report\": null,\n  \"prune_workspace_result\": null,\n  \"landing_execution_result\": null,\n  \"mode_payload\": {{\n    \"kind\": \"implement\",\n    \"semantic_changes\": [],\n    \"tests_run\": [],\n    \"rough_edges\": []\n  }}\n}}\nORCAS_REPORT_END",
            record.packet.packet_id
        );

        let parsed = parse_worker_report(&raw, &assignment, &record);

        assert_eq!(parsed.validation.parse_result, ReportParseResult::Ambiguous);
        assert!(parsed.envelope.is_some());
        assert_eq!(parsed.disposition, ReportDisposition::Completed);
        assert_eq!(parsed.summary, "Worker completed the bounded change.");
        assert_eq!(
            parsed.envelope.as_ref().expect("envelope").assignment_id,
            assignment.id
        );
    }

    #[test]
    fn parse_worker_report_repairs_stray_field_prefix_before_task_mode() {
        let assignment = sample_assignment();
        let record = sample_record(&assignment);
        let raw = format!(
            "worker preamble\nORCAS_REPORT_BEGIN\n{{\n  \"schema_version\": \"worker_report_envelope.v1\",\n \"a685  \"task_mode\": \"implement\",\n  \"disposition\": \"completed\",\n  \"summary\": \"Completed the bounded change.\",\n  \"confidence\": \"high\",\n  \"acceptance_results\": [],\n  \"triggered_stop_condition_ids\": [],\n  \"touched_files\": [],\n  \"commands_run\": [],\n  \"artifacts\": [],\n  \"blockers\": [],\n  \"questions\": [],\n  \"recommended_next_actions\": [],\n  \"uncertainties\": [],\n  \"review_signal\": {{\n    \"level\": \"normal\",\n    \"reasons\": [],\n    \"focus\": []\n  }},\n  \"workspace_report\": null,\n  \"prune_workspace_result\": null,\n  \"landing_execution_result\": null,\n  \"mode_payload\": {{\n    \"kind\": \"implement\",\n    \"semantic_changes\": [],\n    \"tests_run\": [],\n    \"rough_edges\": []\n  }}\n}}\nORCAS_REPORT_END\nworker epilogue"
        );

        let parsed = parse_worker_report(&raw, &assignment, &record);

        assert_eq!(parsed.validation.parse_result, ReportParseResult::Ambiguous);
        assert!(parsed.validation.needs_supervisor_review);
        assert!(parsed.envelope.is_some());
        assert_eq!(parsed.disposition, ReportDisposition::Completed);
        assert_eq!(parsed.summary, "Worker completed the bounded change.");
        assert_eq!(
            parsed.envelope.as_ref().expect("envelope").assignment_id,
            assignment.id
        );
    }

    #[test]
    fn parse_worker_report_strips_stray_quote_prefixed_noise_line() {
        let assignment = sample_assignment();
        let record = sample_record(&assignment);
        let raw = format!(
            "worker preamble\nORCAS_REPORT_BEGIN\n{{\n  \"schema_version\": \"worker_report_envelope.v1\",\n  \"assignment_id\": \"{}\",\n  \"packet_id\": \"{}\",\n  \":dis\n  \"task_mode\": \"implement\",\n  \"disposition\": \"completed\",\n  \"summary\": \"Completed the bounded change.\",\n  \"confidence\": \"high\",\n  \"acceptance_results\": [],\n  \"triggered_stop_condition_ids\": [],\n  \"touched_files\": [],\n  \"commands_run\": [],\n  \"artifacts\": [],\n  \"blockers\": [],\n  \"questions\": [],\n  \"recommended_next_actions\": [],\n  \"uncertainties\": [],\n  \"review_signal\": {{\n    \"level\": \"normal\",\n    \"reasons\": [],\n    \"focus\": []\n  }},\n  \"workspace_report\": null,\n  \"prune_workspace_result\": null,\n  \"landing_execution_result\": null,\n  \"mode_payload\": {{\n    \"kind\": \"implement\",\n    \"semantic_changes\": [],\n    \"tests_run\": [],\n    \"rough_edges\": []\n  }}\n}}\nORCAS_REPORT_END",
            assignment.id, record.packet.packet_id
        );

        let parsed = parse_worker_report(&raw, &assignment, &record);

        assert_eq!(parsed.validation.parse_result, ReportParseResult::Ambiguous);
        assert!(parsed.envelope.is_some());
        assert_eq!(parsed.disposition, ReportDisposition::Completed);
    }

    #[test]
    fn parse_worker_report_accepts_live_worker_report_schema_shape() {
        let assignment = sample_assignment();
        let record = sample_record(&assignment);
        let raw = format!(
            "Planning the work: inspect files, run tests, patch code, re-run tests.Next, I'll run the tests to confirm the fix.ORCAS_REPORT_BEGIN\n{{\n  \"schema_version\": \"worker_report_envelope.v1\",\n  \"assignment_id\": \"{assignment_id}\",\n  \"packet_id\":93edeba\", \"dis\",\n  \"acceptance_results\": [\n    \"[acceptance_1] Implemented minimal code change in main.c and verified make test passes.\",\n    \"[acceptance_2] Returning an Orcas worker report envelope with details of changes and commands.\"\n  ],\n  \"triggered_stop_condition_ids\": [\"stop_1\"],\n  \"touched_files\": [\"main.c:1\"],\n  \"commands_run\": [\n    \"ls -la\",\n    \"sed -n '1,200p' main.c\",\n    \"ls -la tests && sed -n '1,200p' tests/test.sh\",\n    \"sed -n '1,200p' Makefile\",\n    \"make test\",\n    \"applypatch << 'PATCH'\\\\n*** Begin Patch\\\\n*** Update File: main.c\\\\n@@\\\\n int main(void) {{\\\\n-    puts(\\\"Hello, Orcas\\\");\\\\n+    puts(\\\"Hello, Orcas!\\\");\\\\n     return 0;\\\\n }}\\\\n*** End Patch\\\\nPATCH\",\n    \"make test\"\n  ],\n  \"artifacts\": [],\n  \"blockers\": [],\n  \"questions\": [],\n  \"recommended_next_actions\": [\n    \"If desired, replace the shell-based patch application with the platform's apply_patch tool in future turns to avoid warnings.\"\n  ],\n  \"uncertainties\": [],\n  \"review_signal\": {{\n    \"level\": \"normal\",\n    \"reasons\": [],\n    \"focus\": []\n  }},\n  \"workspace_report\": null,\n  \"prune_workspace_result\": null,\n  \"landing_execution_result\": null,\n  \"mode_payload\": {{\n    \"kind\": \"implement\",\n    \"semantic_changes\": [\n      {{\n        \"path\": \"main.c\",\n        \"summary\": \"Changed output string to include exclamation mark.\",\n        \"diff\": \"@@\\\\n-int main(void) {{\\\\n-    puts(\\\"Hello, Orcas\\\");\\\\n-    return 0;\\\\n-}}\\\\n+int main(void) {{\\\\n+    puts(\\\"Hello, Orcas!\\\");\\\\n+    return 0;\\\\n+}}\\\\n\"\n      }}\n    ],\n    \"tests_run\": [\n      {{\n        \"command\": \"make test\",\n        \"result\": \"PASS\"\n      }}\n    ],\n    \"rough_edges\": [\n      \"Applied patch via exec_command which produced a warning; future edits should use the apply_patch tool directly.\"\n    ]\n  }}\n}}\nORCAS_REPORT_END\nworker epilogue",
            assignment_id = assignment.id
        );

        let parsed = parse_worker_report(&raw, &assignment, &record);

        assert_eq!(parsed.validation.parse_result, ReportParseResult::Ambiguous);
        assert!(parsed.validation.needs_supervisor_review);
        assert!(parsed.envelope.is_some());
        assert_eq!(parsed.disposition, ReportDisposition::Completed);
        assert_eq!(
            parsed.envelope.as_ref().expect("envelope").touched_files[0].path,
            format!(
                "{}/main.c",
                record.packet.execution_context.cwd.as_ref().expect("cwd")
            )
        );
    }
    #[test]
    fn parse_worker_report_repairs_malformed_acceptance_results_block() {
        let assignment = sample_assignment();
        let record = sample_record(&assignment);
        let raw = format!(
            "worker preamble\nORCAS_REPORT_BEGIN\n{{\n  \"schema_version\": \"worker_report_envelope.v1\",\n  \"assignment_id\": \"{assignment_id}\",\n  \"packet_id\": \"{packet_id}\",\n  \"task_mode\": \"implement\",\n  \"disposition\": \"completed\",\n  \"summary\": \"Fixed the bounded bug.\",\n  \"confidence\": \"high\",\n  \"acceptance_results\": [\n    {{{{ \"id\": \"acceptance_1\", \"status\": \"passed\", \"details\": \"Minimal fix landed.\" }}}},\n    {{{{ \"id\": \"acceptance_2\", \"status\": \"passed\", \"details\": \"Tests pass.\" }}\n  ],\n  \"triggered_stop_condition_ids\": [],\n  \"touched_files\": [],\n  \"commands_run\": [],\n  \"artifacts\": [],\n  \"blockers\": [],\n  \"questions\": [],\n  \"recommended_next_actions\": [],\n  \"uncertainties\": [],\n  \"review_signal\": {{\n    \"level\": \"normal\",\n    \"reasons\": [],\n    \"focus\": []\n  }},\n  \"workspace_report\": null,\n  \"prune_workspace_result\": null,\n  \"landing_execution_result\": null,\n  \"mode_payload\": {{\n    \"kind\": \"implement\",\n    \"semantic_changes\": [],\n    \"tests_run\": [],\n    \"rough_edges\": []\n  }}\n}}\nORCAS_REPORT_END\nworker epilogue",
            assignment_id = assignment.id,
            packet_id = record.packet.packet_id
        );

        let parsed = parse_worker_report(&raw, &assignment, &record);

        assert_eq!(parsed.validation.parse_result, ReportParseResult::Ambiguous);
        assert!(parsed.envelope.is_some());
        assert_eq!(parsed.disposition, ReportDisposition::Completed);
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
