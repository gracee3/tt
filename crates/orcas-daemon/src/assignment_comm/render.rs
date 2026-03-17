use std::path::PathBuf;

use chrono::{DateTime, Utc};

use orcas_core::{
    Assignment, AssignmentChangePolicy, AssignmentChecklistItem, AssignmentCommunicationPacket,
    AssignmentCommunicationPolicy, AssignmentCommunicationRecord, AssignmentCommunicationSeed,
    AssignmentContextBlock, AssignmentExecutionContext, AssignmentModeSpec,
    AssignmentScopeBoundary, AssignmentTaskMode, CollaborationState, ImplementModePayload,
    ImplementModeSpec, OrcasError, OrcasResult, PromptRenderArtifact, PromptRenderSpec,
    ReportConfidence, ReportDisposition, ReviewSignal, ReviewSignalLevel, WorkUnit,
    WorkerReportContract, WorkerReportEnvelope, WorkerReportModePayload, Workstream,
};

use crate::assignment_comm::{
    ASSIGNMENT_COMMUNICATION_PACKET_SCHEMA_VERSION, ASSIGNMENT_PROMPT_TEMPLATE_VERSION,
    REPORT_MARKER_BEGIN, REPORT_MARKER_END, WORKER_REPORT_CONTRACT_SCHEMA_VERSION,
    WORKER_REPORT_ENVELOPE_SCHEMA_VERSION, json_fingerprint,
};

const SECTION_ORDER: &[&str] = &[
    "worker_contract",
    "assignment_identity",
    "task_mode",
    "objective",
    "instructions",
    "scope_and_non_goals",
    "acceptance_criteria",
    "stop_conditions",
    "included_context",
    "response_contract",
    "response_example",
];

#[derive(Debug, Default, Clone)]
struct LegacyInstructionSeed {
    objective: Option<String>,
    predecessor_assignment_id: Option<String>,
    source_report_id: Option<String>,
    instructions: Vec<String>,
    acceptance_criteria: Vec<String>,
    stop_conditions: Vec<String>,
    required_context_refs: Vec<String>,
    boundedness_note: Option<String>,
}

pub fn build_assignment_communication_record(
    collaboration: &CollaborationState,
    assignment: &Assignment,
    requested_model: Option<String>,
    requested_cwd: Option<String>,
    default_cwd: Option<&PathBuf>,
    now: DateTime<Utc>,
) -> OrcasResult<AssignmentCommunicationRecord> {
    let work_unit = collaboration
        .work_units
        .get(&assignment.work_unit_id)
        .ok_or_else(|| {
            OrcasError::Protocol(format!(
                "unknown work unit `{}` for assignment communication packet",
                assignment.work_unit_id
            ))
        })?;
    let workstream = collaboration
        .workstreams
        .get(&work_unit.workstream_id)
        .ok_or_else(|| {
            OrcasError::Protocol(format!(
                "unknown workstream `{}` for assignment communication packet",
                work_unit.workstream_id
            ))
        })?;

    let execution_context = AssignmentExecutionContext {
        runtime_kind: "codex_app_server".to_string(),
        repo_root: requested_cwd
            .clone()
            .or_else(|| default_cwd.map(|path| path.display().to_string())),
        cwd: requested_cwd.or_else(|| default_cwd.map(|path| path.display().to_string())),
        related_repo_roots: Vec::new(),
        requested_model,
        shell: std::env::var("SHELL").ok(),
    };

    let response_contract = worker_report_contract();
    let packet = if let Some(seed) = assignment.communication_seed.as_ref() {
        build_packet_from_seed(
            collaboration,
            assignment,
            work_unit,
            workstream,
            execution_context,
            response_contract,
            seed,
            now,
        )
    } else {
        // Legacy back-compat only: older assignments may still lack structured communication
        // seed data and must recover packet semantics from the stored instruction preview.
        build_packet_from_legacy_assignment_instructions(
            collaboration,
            assignment,
            work_unit,
            workstream,
            execution_context,
            response_contract,
            now,
        )
    };

    let packet_hash = json_fingerprint(&packet)?;
    let prompt_render = render_prompt(&packet, &packet_hash, now)?;
    Ok(AssignmentCommunicationRecord {
        assignment_id: assignment.id.clone(),
        work_unit_id: work_unit.id.clone(),
        workstream_id: workstream.id.clone(),
        created_at: now,
        packet,
        prompt_render: prompt_render.clone(),
        packet_hash,
        prompt_hash: prompt_render.prompt_hash.clone(),
        response_envelope: None,
        validation: None,
        raw_output_hash: None,
    })
}

pub fn worker_report_contract() -> WorkerReportContract {
    WorkerReportContract {
        schema_version: WORKER_REPORT_CONTRACT_SCHEMA_VERSION.to_string(),
        task_mode: AssignmentTaskMode::Implement,
        marker_begin: REPORT_MARKER_BEGIN.to_string(),
        marker_end: REPORT_MARKER_END.to_string(),
        required_common_fields: vec![
            "schema_version".to_string(),
            "assignment_id".to_string(),
            "packet_id".to_string(),
            "task_mode".to_string(),
            "disposition".to_string(),
            "summary".to_string(),
            "confidence".to_string(),
            "acceptance_results".to_string(),
            "triggered_stop_condition_ids".to_string(),
            "touched_files".to_string(),
            "commands_run".to_string(),
            "artifacts".to_string(),
            "blockers".to_string(),
            "questions".to_string(),
            "recommended_next_actions".to_string(),
            "uncertainties".to_string(),
            "review_signal".to_string(),
        ],
        required_mode_fields: vec![
            "mode_payload.semantic_changes".to_string(),
            "mode_payload.tests_run".to_string(),
            "mode_payload.rough_edges".to_string(),
        ],
        allowed_dispositions: vec![
            ReportDisposition::Completed,
            ReportDisposition::Partial,
            ReportDisposition::Blocked,
            ReportDisposition::Failed,
            ReportDisposition::Interrupted,
        ],
        strict_single_envelope: true,
    }
}

fn build_packet_from_seed(
    collaboration: &CollaborationState,
    assignment: &Assignment,
    work_unit: &WorkUnit,
    workstream: &Workstream,
    execution_context: AssignmentExecutionContext,
    response_contract: WorkerReportContract,
    seed: &AssignmentCommunicationSeed,
    now: DateTime<Utc>,
) -> AssignmentCommunicationPacket {
    let task_mode = seed.task_mode();
    AssignmentCommunicationPacket {
        schema_version: ASSIGNMENT_COMMUNICATION_PACKET_SCHEMA_VERSION.to_string(),
        packet_id: format!("packet-{}", assignment.id),
        assignment_id: assignment.id.clone(),
        workstream_id: workstream.id.clone(),
        work_unit_id: work_unit.id.clone(),
        worker_id: assignment.worker_id.clone(),
        worker_session_id: assignment.worker_session_id.clone(),
        created_at: now,
        source_decision_id: seed.source_decision_id.clone(),
        source_report_id: seed
            .source_report_id
            .clone()
            .or_else(|| work_unit.latest_report_id.clone()),
        source_proposal_id: seed.source_proposal_id.clone(),
        predecessor_assignment_id: seed.predecessor_assignment_id.clone(),
        task_mode,
        mode_spec: seed.mode_spec.clone(),
        execution_context: execution_context.clone(),
        objective: derive_structured_objective(work_unit, seed),
        instructions: derive_structured_instructions(seed),
        acceptance_criteria: derive_structured_acceptance_criteria(seed),
        stop_conditions: derive_structured_stop_conditions(seed),
        allowed_scope: derive_structured_allowed_scope(&execution_context),
        disallowed_scope: default_disallowed_scope(),
        non_goals: seed
            .boundedness_note
            .clone()
            .into_iter()
            .collect::<Vec<_>>(),
        included_context: build_context_blocks_from_seed(
            collaboration,
            assignment,
            work_unit,
            workstream,
            seed,
        ),
        response_contract,
        policy: default_assignment_policy(),
    }
}

fn build_packet_from_legacy_assignment_instructions(
    collaboration: &CollaborationState,
    assignment: &Assignment,
    work_unit: &WorkUnit,
    workstream: &Workstream,
    execution_context: AssignmentExecutionContext,
    response_contract: WorkerReportContract,
    now: DateTime<Utc>,
) -> AssignmentCommunicationPacket {
    let legacy = parse_legacy_instruction_seed(&assignment.instructions);
    AssignmentCommunicationPacket {
        schema_version: ASSIGNMENT_COMMUNICATION_PACKET_SCHEMA_VERSION.to_string(),
        packet_id: format!("packet-{}", assignment.id),
        assignment_id: assignment.id.clone(),
        workstream_id: workstream.id.clone(),
        work_unit_id: work_unit.id.clone(),
        worker_id: assignment.worker_id.clone(),
        worker_session_id: assignment.worker_session_id.clone(),
        created_at: now,
        source_decision_id: None,
        source_report_id: legacy
            .source_report_id
            .clone()
            .or_else(|| work_unit.latest_report_id.clone()),
        source_proposal_id: None,
        predecessor_assignment_id: legacy.predecessor_assignment_id.clone(),
        task_mode: AssignmentTaskMode::Implement,
        mode_spec: AssignmentModeSpec::Implement(ImplementModeSpec {
            expected_verification_commands: Vec::new(),
        }),
        execution_context: execution_context.clone(),
        objective: derive_legacy_objective(assignment, work_unit, &legacy),
        instructions: derive_legacy_instructions(assignment, work_unit, &legacy),
        acceptance_criteria: derive_legacy_acceptance_criteria(&legacy),
        stop_conditions: derive_legacy_stop_conditions(&legacy),
        allowed_scope: derive_legacy_allowed_scope(assignment, &execution_context),
        disallowed_scope: default_disallowed_scope(),
        non_goals: legacy
            .boundedness_note
            .clone()
            .into_iter()
            .collect::<Vec<_>>(),
        included_context: build_context_blocks_from_legacy(
            collaboration,
            assignment,
            work_unit,
            workstream,
            &legacy,
        ),
        response_contract,
        policy: default_assignment_policy(),
    }
}

pub fn render_prompt(
    packet: &AssignmentCommunicationPacket,
    packet_hash: &str,
    rendered_at: DateTime<Utc>,
) -> OrcasResult<PromptRenderArtifact> {
    let render_spec = PromptRenderSpec {
        template_version: ASSIGNMENT_PROMPT_TEMPLATE_VERSION.to_string(),
        section_order: SECTION_ORDER
            .iter()
            .map(|section| (*section).to_string())
            .collect(),
        response_marker_begin: REPORT_MARKER_BEGIN.to_string(),
        response_marker_end: REPORT_MARKER_END.to_string(),
        style: "plain_text_markdown".to_string(),
    };
    let example = example_report_envelope(packet);
    let example_json = serde_json::to_string_pretty(&example)?;

    let mut prompt = String::new();
    prompt.push_str("You are an Orcas worker executing one bounded assignment.\n");
    prompt.push_str(
        "Orcas protocol state is authoritative. Rendered prompt text is derived from Orcas communication state.\n",
    );
    prompt.push_str(
        "Stop at the assignment boundary. Do not continue into unassigned follow-on work.\n\n",
    );

    prompt.push_str(&format!(
        "Template version: {}\nAssignment id: {}\nPacket id: {}\nTask mode: {}\n\n",
        render_spec.template_version,
        packet.assignment_id,
        packet.packet_id,
        task_mode_label(packet.task_mode),
    ));

    prompt.push_str("Objective:\n");
    prompt.push_str(&format!("{}\n\n", packet.objective));

    prompt.push_str("Instructions:\n");
    render_string_list(
        &mut prompt,
        &packet.instructions,
        "No additional instructions.",
    );
    prompt.push('\n');

    prompt.push_str("Scope And Non-Goals:\n");
    prompt.push_str(&format!(
        "- Change policy: {}\n",
        change_policy_label(packet.allowed_scope.change_policy)
    ));
    prompt.push_str(&format!(
        "- Allowed operations: {}\n",
        join_or_none(&packet.allowed_scope.allowed_operations)
    ));
    prompt.push_str(&format!(
        "- Allowed write paths: {}\n",
        join_or_none(&packet.allowed_scope.allowed_write_paths)
    ));
    prompt.push_str(&format!(
        "- Disallowed paths: {}\n",
        join_or_none(&packet.allowed_scope.disallowed_paths)
    ));
    render_prefixed_list(&mut prompt, "Disallowed scope", &packet.disallowed_scope);
    render_prefixed_list(&mut prompt, "Non-goals", &packet.non_goals);
    prompt.push('\n');

    prompt.push_str("Acceptance Criteria:\n");
    render_checklist(&mut prompt, &packet.acceptance_criteria);
    prompt.push('\n');

    prompt.push_str("Stop Conditions:\n");
    render_checklist(&mut prompt, &packet.stop_conditions);
    prompt.push('\n');

    prompt.push_str("Included Context:\n");
    if packet.included_context.is_empty() {
        prompt.push_str("- No additional context blocks.\n");
    } else {
        for block in &packet.included_context {
            prompt.push_str(&format!(
                "- [{}] {} ({})\n",
                block.kind, block.title, block.source_ref
            ));
            for line in &block.lines {
                prompt.push_str(&format!("  - {line}\n"));
            }
        }
    }
    prompt.push('\n');

    prompt.push_str("Response Contract:\n");
    prompt.push_str(&format!(
        "- Emit exactly one JSON envelope between {} and {}.\n",
        packet.response_contract.marker_begin, packet.response_contract.marker_end
    ));
    prompt.push_str("- Common required fields must always exist.\n");
    prompt.push_str("- Array fields may be empty when there is nothing honest to report.\n");
    prompt.push_str("- Do not wrap the envelope in markdown fences.\n");
    prompt.push_str("- Worker recommendations are non-authoritative.\n");
    prompt.push_str(&format!("- Packet fingerprint: {packet_hash}\n"));
    prompt.push('\n');

    prompt.push_str("Response Example:\n");
    prompt.push_str(REPORT_MARKER_BEGIN);
    prompt.push('\n');
    prompt.push_str(&example_json);
    prompt.push('\n');
    prompt.push_str(REPORT_MARKER_END);
    prompt.push('\n');

    let prompt_hash = json_fingerprint(&prompt)?;
    Ok(PromptRenderArtifact {
        render_spec,
        rendered_at,
        prompt_text: prompt,
        packet_hash: packet_hash.to_string(),
        prompt_hash,
    })
}

fn derive_structured_objective(work_unit: &WorkUnit, seed: &AssignmentCommunicationSeed) -> String {
    seed.objective
        .trim()
        .is_empty()
        .then_some(())
        .and_then(|_| {
            (!work_unit.task_statement.trim().is_empty())
                .then_some(work_unit.task_statement.clone())
        })
        .unwrap_or_else(|| seed.objective.clone())
}

fn derive_structured_instructions(seed: &AssignmentCommunicationSeed) -> Vec<String> {
    if seed.instructions.is_empty() {
        return default_instruction_lines();
    }
    seed.instructions.clone()
}

fn derive_structured_acceptance_criteria(
    seed: &AssignmentCommunicationSeed,
) -> Vec<AssignmentChecklistItem> {
    checklist_items(
        "acceptance",
        seed.acceptance_criteria.clone(),
        default_acceptance_lines(),
    )
}

fn derive_structured_stop_conditions(
    seed: &AssignmentCommunicationSeed,
) -> Vec<AssignmentChecklistItem> {
    checklist_items(
        "stop",
        seed.stop_conditions.clone(),
        default_stop_condition_lines(),
    )
}

fn derive_legacy_objective(
    assignment: &Assignment,
    work_unit: &WorkUnit,
    legacy: &LegacyInstructionSeed,
) -> String {
    legacy
        .objective
        .clone()
        .filter(|objective| !objective.trim().is_empty())
        .or_else(|| {
            (!work_unit.task_statement.trim().is_empty())
                .then_some(work_unit.task_statement.clone())
        })
        .or_else(|| {
            (!assignment.instructions.trim().is_empty()).then_some(assignment.instructions.clone())
        })
        .unwrap_or_else(|| format!("Complete the bounded work for {}", work_unit.title))
}

fn derive_legacy_instructions(
    assignment: &Assignment,
    work_unit: &WorkUnit,
    legacy: &LegacyInstructionSeed,
) -> Vec<String> {
    if !legacy.instructions.is_empty() {
        return legacy.instructions.clone();
    }
    if !assignment.instructions.trim().is_empty()
        && assignment.instructions != work_unit.task_statement
    {
        return vec![assignment.instructions.clone()];
    }
    default_instruction_lines()
}

fn derive_legacy_acceptance_criteria(
    legacy: &LegacyInstructionSeed,
) -> Vec<AssignmentChecklistItem> {
    checklist_items(
        "acceptance",
        legacy.acceptance_criteria.clone(),
        default_acceptance_lines(),
    )
}

fn derive_legacy_stop_conditions(legacy: &LegacyInstructionSeed) -> Vec<AssignmentChecklistItem> {
    checklist_items(
        "stop",
        legacy.stop_conditions.clone(),
        default_stop_condition_lines(),
    )
}

fn derive_structured_allowed_scope(
    execution_context: &AssignmentExecutionContext,
) -> AssignmentScopeBoundary {
    let allowed_write_paths = execution_context
        .repo_root
        .clone()
        .or_else(|| execution_context.cwd.clone())
        .into_iter()
        .collect::<Vec<_>>();
    AssignmentScopeBoundary {
        change_policy: AssignmentChangePolicy::CodeAllowed,
        allowed_operations: vec![
            "read_repo".to_string(),
            "edit_repo".to_string(),
            "run_commands".to_string(),
            "run_tests".to_string(),
        ],
        allowed_write_paths,
        disallowed_paths: Vec::new(),
    }
}

fn derive_legacy_allowed_scope(
    assignment: &Assignment,
    execution_context: &AssignmentExecutionContext,
) -> AssignmentScopeBoundary {
    let mut scope = derive_structured_allowed_scope(execution_context);
    if scope.allowed_write_paths.is_empty()
        && let Some(path) = assignment
            .instructions
            .lines()
            .find_map(|line| line.strip_prefix("Repo root: "))
    {
        scope.allowed_write_paths.push(path.trim().to_string());
    }
    scope
}

fn build_context_blocks_from_seed(
    collaboration: &CollaborationState,
    assignment: &Assignment,
    work_unit: &WorkUnit,
    workstream: &Workstream,
    seed: &AssignmentCommunicationSeed,
) -> Vec<AssignmentContextBlock> {
    let mut blocks = default_context_blocks(work_unit, workstream);

    if let Some(source_report_id) = seed
        .source_report_id
        .as_ref()
        .or(work_unit.latest_report_id.as_ref())
        && let Some(report) = collaboration.reports.get(source_report_id)
    {
        blocks.push(source_report_context_block(
            report.id.clone(),
            report.summary.clone(),
            report.disposition,
        ));
    }

    if !seed.required_context_refs.is_empty() {
        blocks.push(AssignmentContextBlock {
            id: "required_context_refs".to_string(),
            kind: "context_refs".to_string(),
            source_ref: assignment.id.clone(),
            title: "Required context refs".to_string(),
            lines: seed.required_context_refs.clone(),
            required: false,
            truncated: false,
        });
    }

    blocks
}

fn build_context_blocks_from_legacy(
    collaboration: &CollaborationState,
    assignment: &Assignment,
    work_unit: &WorkUnit,
    workstream: &Workstream,
    legacy: &LegacyInstructionSeed,
) -> Vec<AssignmentContextBlock> {
    let mut blocks = default_context_blocks(work_unit, workstream);

    if let Some(source_report_id) = legacy
        .source_report_id
        .as_ref()
        .or(work_unit.latest_report_id.as_ref())
        && let Some(report) = collaboration.reports.get(source_report_id)
    {
        blocks.push(source_report_context_block(
            report.id.clone(),
            report.summary.clone(),
            report.disposition,
        ));
    }

    if !legacy.required_context_refs.is_empty() {
        blocks.push(AssignmentContextBlock {
            id: "required_context_refs".to_string(),
            kind: "context_refs".to_string(),
            source_ref: assignment.id.clone(),
            title: "Required context refs".to_string(),
            lines: legacy.required_context_refs.clone(),
            required: false,
            truncated: false,
        });
    }

    blocks
}

fn default_context_blocks(
    work_unit: &WorkUnit,
    workstream: &Workstream,
) -> Vec<AssignmentContextBlock> {
    vec![
        AssignmentContextBlock {
            id: "workstream".to_string(),
            kind: "workstream".to_string(),
            source_ref: workstream.id.clone(),
            title: workstream.title.clone(),
            lines: vec![format!("Objective: {}", workstream.objective)],
            required: true,
            truncated: false,
        },
        AssignmentContextBlock {
            id: "work_unit".to_string(),
            kind: "work_unit".to_string(),
            source_ref: work_unit.id.clone(),
            title: work_unit.title.clone(),
            lines: vec![format!("Task statement: {}", work_unit.task_statement)],
            required: true,
            truncated: false,
        },
    ]
}

fn source_report_context_block(
    report_id: String,
    summary: String,
    disposition: ReportDisposition,
) -> AssignmentContextBlock {
    AssignmentContextBlock {
        id: "source_report".to_string(),
        kind: "report".to_string(),
        source_ref: report_id,
        title: "Source report".to_string(),
        lines: vec![
            format!("Disposition: {:?}", disposition),
            format!("Summary: {}", summary),
        ],
        required: false,
        truncated: false,
    }
}

fn checklist_items(
    prefix: &str,
    items: Vec<String>,
    defaults: Vec<String>,
) -> Vec<AssignmentChecklistItem> {
    let items = if items.is_empty() { defaults } else { items };
    items
        .into_iter()
        .enumerate()
        .map(|(index, text)| AssignmentChecklistItem {
            id: format!("{prefix}_{}", index + 1),
            text,
        })
        .collect()
}

fn default_instruction_lines() -> Vec<String> {
    vec!["Implement the bounded task without broadening scope.".to_string()]
}

fn default_acceptance_lines() -> Vec<String> {
    vec![
        "Complete the bounded implement task described in the objective and instructions."
            .to_string(),
        "Return a valid Orcas worker report envelope with honest implementation details."
            .to_string(),
    ]
}

fn default_stop_condition_lines() -> Vec<String> {
    vec![
        "Stop when the bounded implement task is complete.".to_string(),
        "Stop when blocked or when supervisor or human input is required.".to_string(),
        "Stop rather than broadening scope beyond the assignment boundary.".to_string(),
    ]
}

fn default_disallowed_scope() -> Vec<String> {
    vec![
        "Do not create or execute follow-on work outside this assignment.".to_string(),
        "Do not broaden scope beyond the bounded implement task.".to_string(),
    ]
}

fn default_assignment_policy() -> AssignmentCommunicationPolicy {
    AssignmentCommunicationPolicy {
        stop_at_boundary: true,
        single_report_required: true,
        recommendations_are_non_authoritative: true,
        enforce_scope_boundary: true,
    }
}

fn parse_legacy_instruction_seed(input: &str) -> LegacyInstructionSeed {
    let mut seed = LegacyInstructionSeed::default();
    let mut section: Option<&str> = None;
    let lines = input.lines().collect::<Vec<_>>();
    for raw_line in lines {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(value) = line.strip_prefix("Objective: ") {
            seed.objective = Some(value.trim().to_string());
            section = None;
            continue;
        }
        if let Some(value) = line.strip_prefix("Predecessor assignment: ") {
            seed.predecessor_assignment_id = Some(value.trim().to_string());
            section = None;
            continue;
        }
        if let Some(value) = line.strip_prefix("Source report: ") {
            seed.source_report_id = Some(value.trim().to_string());
            section = None;
            continue;
        }
        if let Some(value) = line.strip_prefix("Required context refs: ") {
            seed.required_context_refs = value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect();
            section = None;
            continue;
        }
        if let Some(value) = line.strip_prefix("Boundedness note: ") {
            seed.boundedness_note = Some(value.trim().to_string());
            section = None;
            continue;
        }
        match line {
            "Instructions:" => {
                section = Some("instructions");
                continue;
            }
            "Acceptance criteria:" => {
                section = Some("acceptance_criteria");
                continue;
            }
            "Stop conditions:" => {
                section = Some("stop_conditions");
                continue;
            }
            _ => {}
        }

        if let Some(value) = line.strip_prefix("- ") {
            match section {
                Some("instructions") => seed.instructions.push(value.to_string()),
                Some("acceptance_criteria") => {
                    seed.acceptance_criteria.push(value.to_string());
                }
                Some("stop_conditions") => seed.stop_conditions.push(value.to_string()),
                _ => {}
            }
        }
    }
    seed
}

fn example_report_envelope(packet: &AssignmentCommunicationPacket) -> WorkerReportEnvelope {
    WorkerReportEnvelope {
        schema_version: WORKER_REPORT_ENVELOPE_SCHEMA_VERSION.to_string(),
        assignment_id: packet.assignment_id.clone(),
        packet_id: packet.packet_id.clone(),
        task_mode: AssignmentTaskMode::Implement,
        disposition: ReportDisposition::Completed,
        summary: "Summarize the bounded implementation result.".to_string(),
        confidence: ReportConfidence::Medium,
        acceptance_results: Vec::new(),
        triggered_stop_condition_ids: Vec::new(),
        touched_files: Vec::new(),
        commands_run: Vec::new(),
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
        mode_payload: WorkerReportModePayload::Implement(ImplementModePayload {
            semantic_changes: Vec::new(),
            tests_run: Vec::new(),
            rough_edges: Vec::new(),
        }),
    }
}

fn render_string_list(prompt: &mut String, items: &[String], empty_label: &str) {
    if items.is_empty() {
        prompt.push_str(&format!("- {empty_label}\n"));
        return;
    }
    for item in items {
        prompt.push_str(&format!("- {item}\n"));
    }
}

fn render_prefixed_list(prompt: &mut String, label: &str, items: &[String]) {
    prompt.push_str(&format!("- {label}: {}\n", join_or_none(items)));
}

fn render_checklist(prompt: &mut String, items: &[AssignmentChecklistItem]) {
    if items.is_empty() {
        prompt.push_str("- none\n");
        return;
    }
    for item in items {
        prompt.push_str(&format!("- [{}] {}\n", item.id, item.text));
    }
}

fn join_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

fn task_mode_label(mode: AssignmentTaskMode) -> &'static str {
    match mode {
        AssignmentTaskMode::Implement => "implement",
        AssignmentTaskMode::Inspect => "inspect",
        AssignmentTaskMode::Debug => "debug",
        AssignmentTaskMode::Design => "design",
        AssignmentTaskMode::Test => "test",
    }
}

fn change_policy_label(policy: AssignmentChangePolicy) -> &'static str {
    match policy {
        AssignmentChangePolicy::CodeAllowed => "code_allowed",
        AssignmentChangePolicy::ReadOnly => "read_only",
        AssignmentChangePolicy::DocsOnly => "docs_only",
        AssignmentChangePolicy::TestsOnly => "tests_only",
    }
}
