# Assignment Communication Visibility v1

This doc describes the current narrow Assignment Communication Protocol v1 transmission layer as it exists now.

It is meant to answer, in one place:

- what Orcas treats as canonical packet input
- what text the worker actually sees
- what structured response the worker is expected to return
- how Orcas extracts and validates that response
- where authoritative report truth begins

## Purpose and Scope

ACP v1 is a bounded supervisor-to-worker transmission flow, not a general prompt system.

Current ownership boundaries:

- `AssignmentCommunicationSeed` is canonical input
- `AssignmentCommunicationPacket` is canonical structured transmission state
- `AssignmentCommunicationRecord` is the persisted audit record for the whole exchange
- `PromptRenderArtifact` is derived transmission output
- raw worker output is audit evidence only
- `WorkerReportEnvelope` is candidate structured output until Orcas validates it
- `Report` is the authoritative truth consumed downstream by decisions, proposals, snapshots, and UI summaries

Primary code paths:

- `crates/orcas-daemon/src/assignment_comm/render.rs`
- `crates/orcas-daemon/src/assignment_comm/parse.rs`
- `crates/orcas-daemon/src/assignment_comm/policy.rs`
- `crates/orcas-daemon/src/service.rs`

## End-to-End Flow

Implement-mode flow:

```text
AssignmentCommunicationSeed
-> AssignmentCommunicationPacket
-> AssignmentCommunicationRecord
-> PromptRenderArtifact
-> worker prompt text
-> raw worker output
-> ORCAS_REPORT_BEGIN / ORCAS_REPORT_END extraction
-> envelope validation
-> authoritative Report
```

Main functions and modules:

- seed population: `prepare_assignment`, `assignment_communication_seed_from_draft`, `assignment_communication_seed_from_packet`, `next_assignment_communication_seed` in `crates/orcas-daemon/src/service.rs`
- packet construction: `build_assignment_communication_record`, `build_packet_from_seed`, `build_packet_from_legacy_assignment_instructions` in `crates/orcas-daemon/src/assignment_comm/render.rs`
- prompt rendering: `render_prompt` in `crates/orcas-daemon/src/assignment_comm/render.rs`
- prompt dispatch: `assignment_start` in `crates/orcas-daemon/src/service.rs`
- envelope extraction and parsing: `parse_worker_report`, `parse_worker_report_for_turn`, `extract_envelope` in `crates/orcas-daemon/src/assignment_comm/parse.rs`
- validation policy: `validate_assignment_packet`, `validate_worker_report_envelope` in `crates/orcas-daemon/src/assignment_comm/policy.rs`
- authoritative report ingestion: `record_assignment_turn_outcome` in `crates/orcas-daemon/src/service.rs`

## Template / Contract Inventory

| Artifact | Where it lives | Canonical or derived | Fixed or data-driven | Purpose |
| --- | --- | --- | --- | --- |
| `assignment_communication_packet.v1` | `crates/orcas-daemon/src/assignment_comm/mod.rs` | Canonical | Fixed version tag | Identifies the packet schema used for the structured assignment payload |
| `assignment_prompt.v1` | `crates/orcas-daemon/src/assignment_comm/mod.rs` and `render.rs` | Derived | Mostly data-driven, with a fixed section order and header text | The exact worker prompt text compiled from the packet |
| `worker_report_contract.v1` | `crates/orcas-daemon/src/assignment_comm/mod.rs` and `render.rs` | Canonical contract, embedded in the packet | Fixed schema, data-driven field lists | Declares the worker response expectations and response markers |
| `worker_report_envelope.v1` | `crates/orcas-daemon/src/assignment_comm/mod.rs`, `render.rs`, `policy.rs`, `parse.rs` | Candidate worker output schema | Fixed schema version, data-driven payload content | The JSON envelope Orcas expects the worker to return |
| `ORCAS_REPORT_BEGIN` / `ORCAS_REPORT_END` | `crates/orcas-daemon/src/assignment_comm/mod.rs` | Fixed marker contract | Fixed | Delimits the one valid worker report envelope in raw output |

The worker prompt template is intentionally plain text:

```text
You are an Orcas worker executing one bounded assignment.
Orcas protocol state is authoritative. Rendered prompt text is derived from Orcas communication state.
Stop at the assignment boundary. Do not continue into unassigned follow-on work.

Template version: assignment_prompt.v1
Assignment id: <assignment_id>
Packet id: <packet_id>
Task mode: implement

Objective:
<objective>

Instructions:
- <instruction lines>

Scope And Non-Goals:
- Change policy: code_allowed
- Allowed operations: read_repo, edit_repo, run_commands, run_tests
- Allowed write paths: <repo-root or cwd>
- Disallowed paths: none
- Disallowed scope: <derived non-goals>
- Non-goals: <boundedness note or none>

Acceptance Criteria:
- [acceptance_1] ...

Stop Conditions:
- [stop_1] ...

Included Context:
- [workstream] ...
- [work_unit] ...

Response Contract:
- Emit exactly one JSON envelope between ORCAS_REPORT_BEGIN and ORCAS_REPORT_END.
- Common required fields must always exist.
- Array fields may be empty when there is nothing honest to report.
- Do not wrap the envelope in markdown fences.
- Worker recommendations are non-authoritative.
- Packet fingerprint: <packet_hash>

Response Example:
ORCAS_REPORT_BEGIN
{ ...example JSON... }
ORCAS_REPORT_END
```

## Frozen Concrete Example

This example is based on the current implement-mode test fixture in `crates/orcas-daemon/src/service.rs` and the current render path in `crates/orcas-daemon/src/assignment_comm/render.rs`.

### A. Structured seed

```json
{
  "source_decision_id": null,
  "source_report_id": "report-structured",
  "source_proposal_id": "proposal-structured",
  "predecessor_assignment_id": "assignment-parent",
  "objective": "Implement the structured recovery pass.",
  "instructions": [
    "Inspect only the structured recovery branch.",
    "Do not broaden beyond the current implement slice."
  ],
  "acceptance_criteria": [
    "The structured recovery branch behavior is confirmed."
  ],
  "stop_conditions": [
    "Stop if the recovery branch is not reproducible."
  ],
  "required_context_refs": [
    "ctx/recovery"
  ],
  "expected_report_fields": [
    "summary",
    "recommended_next_actions"
  ],
  "boundedness_note": "Keep the follow-up strictly bounded.",
  "mode_spec": {
    "kind": "implement",
    "expected_verification_commands": []
  }
}
```

### B. Canonical packet

```json
{
  "schema_version": "assignment_communication_packet.v1",
  "packet_id": "packet-assignment-structured",
  "assignment_id": "assignment-structured",
  "workstream_id": "ws-structured",
  "work_unit_id": "wu-structured",
  "worker_id": "worker-structured",
  "worker_session_id": "session-structured",
  "task_mode": "implement",
  "objective": "Implement the structured recovery pass.",
  "instructions": [
    "Inspect only the structured recovery branch.",
    "Do not broaden beyond the current implement slice."
  ],
  "acceptance_criteria": [
    { "id": "acceptance_1", "text": "The structured recovery branch behavior is confirmed." }
  ],
  "stop_conditions": [
    { "id": "stop_1", "text": "Stop if the recovery branch is not reproducible." }
  ],
  "included_context": [
    { "kind": "workstream", "title": "Recovery", "source_ref": "ws-structured", "lines": ["Objective: Recover the structured path"], "required": true, "truncated": false },
    { "kind": "work_unit", "title": "Recovery work", "source_ref": "wu-structured", "lines": ["Task statement: Implement the structured recovery pass."], "required": true, "truncated": false },
    { "kind": "context_refs", "title": "Required context refs", "source_ref": "assignment-structured", "lines": ["ctx/recovery"], "required": false, "truncated": false }
  ],
  "response_contract": {
    "schema_version": "worker_report_contract.v1",
    "task_mode": "implement",
    "marker_begin": "ORCAS_REPORT_BEGIN",
    "marker_end": "ORCAS_REPORT_END",
    "required_common_fields": ["schema_version", "assignment_id", "packet_id", "task_mode", "disposition", "summary", "confidence", "acceptance_results", "triggered_stop_condition_ids", "touched_files", "commands_run", "artifacts", "blockers", "questions", "recommended_next_actions", "uncertainties", "review_signal"],
    "required_mode_fields": ["mode_payload.semantic_changes", "mode_payload.tests_run", "mode_payload.rough_edges"],
    "allowed_dispositions": ["completed", "partial", "blocked", "failed", "interrupted"],
    "strict_single_envelope": true
  }
}
```

### C. Rendered prompt text

```text
You are an Orcas worker executing one bounded assignment.
Orcas protocol state is authoritative. Rendered prompt text is derived from Orcas communication state.
Stop at the assignment boundary. Do not continue into unassigned follow-on work.

Template version: assignment_prompt.v1
Assignment id: assignment-structured
Packet id: packet-assignment-structured
Task mode: implement

Objective:
Implement the structured recovery pass.

Instructions:
- Inspect only the structured recovery branch.
- Do not broaden beyond the current implement slice.

Scope And Non-Goals:
- Change policy: code_allowed
- Allowed operations: read_repo, edit_repo, run_commands, run_tests
- Allowed write paths: /home/emmy/git/orcas
- Disallowed paths: none
- Disallowed scope: Do not create or execute follow-on work outside this assignment., Do not broaden scope beyond the bounded implement task.
- Non-goals: Keep the follow-up strictly bounded.

Acceptance Criteria:
- [acceptance_1] The structured recovery branch behavior is confirmed.

Stop Conditions:
- [stop_1] Stop if the recovery branch is not reproducible.

Included Context:
- [workstream] Recovery (ws-structured)
  - Objective: Recover the structured path
- [work_unit] Recovery work (wu-structured)
  - Task statement: Implement the structured recovery pass.
- [context_refs] Required context refs (assignment-structured)
  - ctx/recovery

Response Contract:
- Emit exactly one JSON envelope between ORCAS_REPORT_BEGIN and ORCAS_REPORT_END.
- Common required fields must always exist.
- Array fields may be empty when there is nothing honest to report.
- Do not wrap the envelope in markdown fences.
- Worker recommendations are non-authoritative.
- Packet fingerprint: <packet_hash>

Response Example:
ORCAS_REPORT_BEGIN
{
  "schema_version": "worker_report_envelope.v1",
  "assignment_id": "assignment-structured",
  "packet_id": "packet-assignment-structured",
  "task_mode": "implement",
  "disposition": "completed",
  "summary": "finished the bounded task",
  "confidence": "high",
  "acceptance_results": [],
  "triggered_stop_condition_ids": [],
  "touched_files": [],
  "commands_run": [],
  "artifacts": [],
  "blockers": [],
  "questions": [],
  "recommended_next_actions": ["apply supervisor decision"],
  "uncertainties": [],
  "review_signal": {
    "level": "normal",
    "reasons": [],
    "focus": []
  },
  "mode_payload": {
    "kind": "implement",
    "semantic_changes": ["root cause isolated"],
    "tests_run": ["cargo test -p orcas-daemon"],
    "rough_edges": []
  }
}
ORCAS_REPORT_END
```

### D. Valid response envelope

```text
ORCAS_REPORT_BEGIN
{
  "schema_version": "worker_report_envelope.v1",
  "assignment_id": "assignment-structured",
  "packet_id": "packet-assignment-structured",
  "task_mode": "implement",
  "disposition": "completed",
  "summary": "finished the bounded task",
  "confidence": "high",
  "acceptance_results": [],
  "triggered_stop_condition_ids": [],
  "touched_files": [],
  "commands_run": [],
  "artifacts": [],
  "blockers": [],
  "questions": [],
  "recommended_next_actions": ["apply supervisor decision"],
  "uncertainties": [],
  "review_signal": { "level": "normal", "reasons": [], "focus": [] },
  "mode_payload": {
    "kind": "implement",
    "semantic_changes": ["root cause isolated"],
    "tests_run": ["cargo test -p orcas-daemon"],
    "rough_edges": []
  }
}
ORCAS_REPORT_END
```

Parser / validator outcome:

- `parse_result = Parsed`
- `needs_supervisor_review = false`
- `disposition = Completed`
- `confidence = High`
- findings are derived from `mode_payload.semantic_changes`

### E. Ambiguous response example

```text
here is the report
ORCAS_REPORT_BEGIN
{ valid envelope JSON from D }
ORCAS_REPORT_END
```

Parser / validator outcome:

- `extract_envelope` succeeds
- `surrounding_text = true`
- `parse_result = Ambiguous`
- `needs_supervisor_review = true`
- the envelope remains available for inspection, but it is not trusted as clean

### F. Invalid response example

```text
ORCAS_REPORT_BEGIN
{
  "schema_version": "worker_report_envelope.v1",
  "assignment_id": "assignment-other",
  "packet_id": "packet-assignment-structured",
  "task_mode": "implement",
  "disposition": "completed",
  "summary": "finished the bounded task",
  "confidence": "high"
}
ORCAS_REPORT_END
```

Parser / validator outcome:

- `parse_result = Invalid`
- `needs_supervisor_review = true`
- `response_envelope = Some(...)` if JSON decoding succeeds
- `Report` is still created from the parsed outcome, but it is marked for review and carries the invalid parse result

## Boundaries and Invariants

- Packet is canonical.
- Prompt text is derived transmission only.
- Raw worker output is audit evidence only.
- Worker envelope is candidate structured output until validated.
- Authoritative `Report` is the truth consumed downstream by decisions, proposals, snapshots, and UI summaries.

Implementation detail: `AssignmentCommunicationRecord` stores both the canonical packet and the derived transmission artifact so the exchange can be audited later, but it is still not the downstream truth object.

## Current Visibility Gaps

The core visibility gap described in the previous version of this note is now addressed by a first-class read-only IPC and CLI surface keyed by assignment id.

That surface exposes the exact packet, rendered prompt artifact, and stored validation / audit evidence without changing any write-path behavior.

## Next Read-Only Visibility Step

If more operator visibility is needed later, the next narrow step is to add a dedicated human-readable dump format for the same read-only record rather than broadening the protocol or adding list/search UX.
