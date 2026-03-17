# Orcas Assignment Communication Protocol v1

This document defines the next narrow protocol layer between:

- supervisor -> worker
- worker -> supervisor

It is a docs-first design pass for making assignment communication a first-class Orcas protocol concern.

It is intentionally not:

- a redesign of supervisor proposals
- a scheduler or planner
- a swarm or worker-to-worker protocol
- a transcript-first workflow model
- a prompt-text-as-truth architecture

The rendered worker prompt is derived. Orcas-owned structured state remains the source of truth.

## Design Summary

The v1 communication loop should be:

```text
Assignment + work-unit state + decision/proposal context
  -> AssignmentCommunicationPacket
  -> PromptTemplateRenderer
  -> rendered worker prompt
  -> raw worker output
  -> WorkerReportEnvelope extractor
  -> WorkerReport validator
  -> authoritative Report
  -> existing Decision / Proposal flow
```

Key design choices:

- one immutable communication packet per assignment
- one deterministic rendered prompt per packet/template version
- one bounded worker response envelope per assignment
- conservative parsing and validation
- raw output retained, but never promoted to truth by itself
- existing `Assignment`, `Report`, `Decision`, and `SupervisorProposal` semantics stay in charge

## Scope

V1 should solve one thing well:

- make the supervisor-to-worker contract explicit and inspectable
- make the worker-to-supervisor report contract explicit and parseable
- preserve replayability and deterministic rendering

V1 should not try to solve every future prompt or artifact pattern.

## Canonical Versus Derived State

| Object | Canonical Orcas state | Persisted | Purpose |
| --- | --- | --- | --- |
| `Assignment` | yes | yes | authoritative execution attempt and lifecycle |
| `AssignmentCommunicationPacket` | yes | yes | authoritative structured worker-facing contract for that assignment |
| `PromptRenderArtifact` | no | yes | exact derived prompt text sent to the worker for audit/replay |
| raw worker output | no | yes | exact runtime capture for audit/review |
| `WorkerReportEnvelope` | no | yes | extracted worker-supplied structured response before Orcas validation |
| `WorkerReportValidation` | yes | yes | Orcas-owned parse/validation result over the envelope |
| `Report` | yes | yes | authoritative supervisor-facing report state |
| `SupervisorProposalRecord` | no | yes | review artifact built from authoritative Orcas state |
| `Decision` | yes | yes | authoritative supervisor action |

Recommended rule:

- `AssignmentCommunicationPacket` is the source of truth for what Orcas asked the worker to do.
- `PromptRenderArtifact.prompt_text` is the exact compiled transmission sent to Codex.
- raw worker output is the audit trail of what came back.
- `WorkerReportEnvelope` is only a candidate report until Orcas validates it.
- `Report` is the authoritative report object that the rest of Orcas consumes.

## New Objects

### `AssignmentCommunicationRecord`

This should be the new persisted top-level communication artifact for an assignment.

Recommended fields:

- `assignment_id`
- `work_unit_id`
- `workstream_id`
- `created_at`
- `packet`
- `prompt_render`
- `packet_hash`
- `prompt_hash`

Why it should exist:

- the packet exists before any worker response
- the prompt render should be inspectable without bloating `Assignment`
- it creates one durable place to audit exactly what Orcas meant to communicate

Recommended state placement:

- add `assignment_communications: BTreeMap<String, AssignmentCommunicationRecord>` to `CollaborationState`
- key it by `assignment_id`
- allow exactly one communication record per assignment in v1
- do not introduce packet revision chains or multiple communication records for one assignment in v1

### `AssignmentCommunicationPacket`

This is the canonical supervisor -> worker contract.

Recommended fields:

- `schema_version`
- `packet_id`
- `assignment_id`
- `workstream_id`
- `work_unit_id`
- `worker_id`
- `worker_session_id`
- `created_at`
- `source_decision_id`
- `source_report_id`
- `source_proposal_id`
- `predecessor_assignment_id`
- `task_mode`
- `mode_spec`
- `execution_context`
- `objective`
- `instructions[]`
- `acceptance_criteria[]`
- `stop_conditions[]`
- `allowed_scope`
- `disallowed_scope[]`
- `non_goals[]`
- `included_context[]`
- `response_contract`
- `policy`

Recommended supporting shapes:

- `AssignmentChecklistItem`
  - `id`
  - `text`
- `AssignmentExecutionContext`
  - `runtime_kind`
  - `repo_root`
  - `cwd`
  - `related_repo_roots[]`
  - `requested_model`
  - `shell`
- `AssignmentScopeBoundary`
  - `change_policy`
  - `allowed_operations[]`
  - `allowed_write_paths[]`
  - `disallowed_paths[]`
- `AssignmentContextBlock`
  - `id`
  - `kind`
  - `source_ref`
  - `title`
  - `lines[]`
  - `required`
  - `truncated`

Important design position:

- `included_context[]` should be a resolved snapshot, not just loose ids.
- each context block should still carry its source reference so audit and history stay traceable.
- prompt rendering should not have to re-query live Orcas state to know what context the worker saw.
- `mode_spec` should be a closed typed union keyed by `task_mode`, not a loose JSON blob.

### `PromptRenderSpec`

This is a small Orcas-owned render descriptor, not a human-authored prompt object.

Recommended fields:

- `template_version`
- `section_order[]`
- `response_marker_begin`
- `response_marker_end`
- `style`

V1 should keep this small and deterministic. It exists to version the template boundary, not to create a giant general-purpose prompt system.

### `PromptRenderArtifact`

This is the derived render output actually passed to `turn/start`.

Recommended fields:

- `render_spec`
- `rendered_at`
- `prompt_text`
- `packet_hash`
- `prompt_hash`

### `WorkerReportContract`

This is the versioned expected worker response contract embedded in the packet.

Recommended fields:

- `schema_version`
- `task_mode`
- `marker_begin`
- `marker_end`
- `required_common_fields[]`
- `required_mode_fields[]`
- `allowed_dispositions[]`
- `strict_single_envelope`

### `WorkerReportEnvelope`

This is the extracted worker response payload.

Recommended fields:

- `schema_version`
- `assignment_id`
- `packet_id`
- `task_mode`
- `disposition`
- `summary`
- `confidence`
- `acceptance_results[]`
- `triggered_stop_condition_ids[]`
- `touched_files[]`
- `commands_run[]`
- `artifacts[]`
- `blockers[]`
- `questions[]`
- `recommended_next_actions[]`
- `uncertainties[]`
- `review_signal`
- `mode_payload`

Recommended supporting shapes:

- `AcceptanceResult`
  - `criterion_id`
  - `status`
  - `note`
- `TouchedFile`
  - `path`
  - `change_kind`
  - `summary`
- `CommandRun`
  - `command`
  - `purpose`
  - `outcome`
- `ReviewSignal`
  - `level`
  - `reasons[]`
  - `focus[]`

`WorkerReportEnvelope` should be stored on `Report` as a subordinate field, not as a second top-level workflow object.

Important design position:

- `mode_payload` should be a closed typed union keyed by `task_mode`, not a loose JSON blob.

### `WorkerReportValidation`

This is Orcas-owned validation output for the worker report.

Recommended fields:

- `validated_at`
- `parse_result`
- `structural_issues[]`
- `semantic_issues[]`
- `policy_violations[]`
- `needs_supervisor_review`

Recommended rule:

- `needs_supervisor_review` remains Orcas-owned.
- worker-supplied `review_signal` informs review, but does not replace Orcas validation state.

### `AssignmentTaskMode`

V1 should use a small closed enum:

- `implement`
- `inspect`
- `debug`
- `design`
- `test`

Each mode should have a small tagged `mode_spec` in the packet and a matching tagged `mode_payload` in the worker report envelope.

Implementation guidance:

- represent `mode_spec` and `mode_payload` as closed Rust enums with tagged serde forms
- do not use `serde_json::Value` or other open-ended payload containers for these fields in v1

## Required Packet Fields

At minimum, the packet for a real assignment should always include:

- assignment/work unit/workstream linkage
- source lineage:
  - predecessor assignment when present
  - source report when present
  - source decision when present
  - source proposal when present
- execution context:
  - repo root
  - cwd
  - runtime kind
  - related repos when relevant
- one explicit `task_mode`
- one bounded `objective`
- explicit `instructions[]`
- explicit `acceptance_criteria[]`
- explicit `stop_conditions[]`
- explicit allowed scope
- explicit disallowed scope or non-goals
- explicit included context blocks
- one explicit response contract
- one explicit policy block

Strong recommendation:

- do not allow a packet with no acceptance criteria
- do not allow a packet with no stop conditions
- do not allow a packet with no explicit scope boundary

Those are the minimum boundedness guarantees.

## Task Modes

V1 should start with a small fixed set.

### `implement`

Use when the worker is expected to make bounded repository changes.

Packet differences:

- `allowed_scope.change_policy` is normally `code_allowed`
- `allowed_write_paths[]` should usually be explicit
- `mode_spec` should capture expected verification commands when known

Prompt differences:

- prompt says repository changes are expected but bounded
- prompt emphasizes acceptance criteria and stop conditions over “keep going”

Response differences:

- require `mode_payload.semantic_changes[]`
- require `mode_payload.tests_run[]`
- require `mode_payload.rough_edges[]`

### `inspect`

Use for review, analysis, or code-reading tasks where Orcas wants findings, not changes.

Packet differences:

- `allowed_scope.change_policy` should be `read_only`
- `mode_spec` should capture review focus areas

Prompt differences:

- prompt explicitly forbids edits unless the packet says otherwise
- prompt says findings must be evidence-backed and severity-tagged

Response differences:

- require `mode_payload.findings[]`
- require `mode_payload.evidence_refs[]`
- `touched_files[]` should normally be empty

### `debug`

Use when the worker should reproduce, diagnose, and bound uncertainty around a failure.

Packet differences:

- `mode_spec` should capture the target symptom or reproduction goal
- `allowed_scope.change_policy` may be `read_only` or `code_allowed` depending on whether a confirming fix is allowed

Prompt differences:

- prompt says reproduce or diagnose before broad edits
- prompt emphasizes blocker honesty and confidence boundaries

Response differences:

- require `mode_payload.reproduction_status`
- require `mode_payload.root_cause[]`
- require `mode_payload.fix_options[]`

### `design`

Use for architecture/design/docs-first work where Orcas wants a protocol or implementation design rather than production code changes.

Packet differences:

- `allowed_scope.change_policy` should usually be `docs_only` or `read_only`
- `mode_spec` should capture target doc/artifact paths when known

Prompt differences:

- prompt says the worker is producing a design artifact, not silently implementing
- prompt emphasizes object model, invariants, tradeoffs, and implementation cut

Response differences:

- require `mode_payload.proposed_objects[]`
- require `mode_payload.tradeoffs[]`
- require `mode_payload.recommended_implementation_slice`

### `test`

Use when the assignment is primarily about tests, harnesses, or validation coverage.

Packet differences:

- `allowed_scope.change_policy` should usually be `tests_only`
- `mode_spec` should capture target commands or coverage goals

Prompt differences:

- prompt says prefer tests/harnesses over production changes unless explicitly allowed

Response differences:

- require `mode_payload.tests_added[]`
- require `mode_payload.test_results[]`
- require `mode_payload.remaining_gaps[]`

## Prompt Rendering Boundary

The rendering boundary should live inside Orcas daemon assignment dispatch, after packet construction and policy validation, and before `turn/start`.

Recommended rule:

- CLI, proposal approval, and other operator flows produce structured packet inputs.
- the daemon builds the canonical packet.
- the renderer compiles that packet into worker-facing text.
- only the rendered text crosses into the Codex worker substrate.

### Fixed Template Versus Runtime Data

Fixed template sections should be:

- worker contract banner
- assignment identity block
- mode banner
- scope boundary wording
- report-envelope instructions
- response marker syntax
- final stop/authority rules

Runtime data should be:

- objective
- instructions
- acceptance criteria
- stop conditions
- allowed/disallowed scope data
- included context blocks
- mode-specific spec data

Recommendation on prose generation:

- most worker-facing prose should be Orcas-fixed template text
- supervisor reasoning and human edits should fill structured slots, not author whole prompts
- a small bounded note field is acceptable
- long freeform supervisor-authored narrative should not be the canonical contract

### Section Ordering

Section ordering should be stable in v1:

1. Orcas worker contract
2. assignment identity
3. task mode
4. objective
5. instructions
6. scope and non-goals
7. acceptance criteria
8. stop conditions
9. included context
10. response contract
11. marker-delimited response example

Stable ordering matters because:

- humans can inspect it quickly
- replay stays comprehensible
- diffing packet/render changes stays easy

### Template Versioning

Prompt templates should be versioned.

Recommended v1 naming:

- `assignment_prompt.v1`
- `worker_report_contract.v1`
- `worker_report_envelope.v1`

Keep versions explicit in persisted artifacts so later template evolution does not erase what was actually sent.

## Worker Response Contract

The worker response should be one JSON envelope inside deterministic markers.

For the first cut, keep the current marker style and extend the payload:

```text
ORCAS_REPORT_BEGIN
{ ... worker_report_envelope.v1 json ... }
ORCAS_REPORT_END
```

### Common Required Fields Across Modes

Every mode should require:

- `schema_version`
- `assignment_id`
- `packet_id`
- `task_mode`
- `disposition`
- `summary`
- `confidence`
- `acceptance_results[]`
- `triggered_stop_condition_ids[]`
- `touched_files[]`
- `commands_run[]`
- `blockers[]`
- `questions[]`
- `recommended_next_actions[]`
- `uncertainties[]`
- `review_signal`

Required should mean:

- these fields must always be present in the envelope
- list fields may be empty when there is nothing honest to report
- workers should not invent placeholder entries just to satisfy shape

### Mode-Specific Sections

Each mode should also require one tagged `mode_payload`.

Required by mode:

- `implement`
  - `semantic_changes[]`
  - `tests_run[]`
  - `rough_edges[]`
- `inspect`
  - `findings[]`
  - `evidence_refs[]`
- `debug`
  - `reproduction_status`
  - `root_cause[]`
  - `fix_options[]`
- `design`
  - `proposed_objects[]`
  - `tradeoffs[]`
  - `recommended_implementation_slice`
- `test`
  - `tests_added[]`
  - `test_results[]`
  - `remaining_gaps[]`

### Disposition Semantics

Use the current bounded outcome set:

- `completed`
- `partial`
- `blocked`
- `failed`
- `interrupted`

Meaning should stay aligned with the existing report contract.

Important runtime rule:

- if Orcas runtime evidence says the turn was interrupted or continuity was lost, Orcas may override the final authoritative report disposition even if the worker envelope claimed otherwise

### Confidence And Uncertainty

Worker-supplied confidence should stay simple:

- `low`
- `medium`
- `high`

Uncertainty should be explicit through:

- `uncertainties[]`
- `blockers[]`
- `questions[]`
- `review_signal`

Do not ask the worker for fake precision beyond that.

### Explicit Supervisor-Review Signaling

Every response should include `review_signal`.

Recommended `level` enum:

- `normal`
- `elevated`
- `required`

Recommended reason examples:

- `blocked_on_decision`
- `low_confidence`
- `scope_tradeoff`
- `runtime_issue`
- `needs_human_choice`

This is not the same as Orcas parse validity. It is a worker-supplied escalation hint.

### How Structured v1 Should Be

V1 should be structured enough to parse conservatively, but not overbuilt.

Recommendation:

- require one JSON envelope in markers
- keep field names stable and snake_case
- allow arrays of small structs or strings
- do not require the worker to emit a giant nested artifact ontology
- do not accept freeform markdown prose as the canonical response object

## Parsing, Validation, And Policy

### Structural Validation

Orcas should validate:

- exactly one begin marker and one end marker
- supported schema version
- valid JSON payload
- required common fields present
- valid enum values
- `assignment_id` matches the current assignment
- `packet_id` matches the packet Orcas dispatched
- `task_mode` matches the packet task mode
- required mode-specific fields present

### Semantic Validation

Orcas should also validate:

- referenced acceptance criteria exist in the packet
- referenced stop conditions exist in the packet
- touched files do not obviously violate scope policy
- mode-specific payload matches the assignment mode
- recommendations remain non-authoritative
- runtime lifecycle does not contradict the claimed disposition without being flagged

Semantic validation should be deterministic and conservative. If Orcas cannot prove something, it should mark review rather than pretend validation succeeded.

### `parsed` Versus `ambiguous` Versus `invalid`

Recommended meanings:

`parsed`

- one unique envelope extracted
- structure is valid
- ids and mode match
- no extraction ambiguity

`ambiguous`

- a candidate envelope exists, but interpretation is not clean enough to trust fully without review
- examples:
  - extra freeform text outside the envelope
  - contradictory optional fields
  - runtime/evidence mismatch that still leaves useful report data
  - semantic or policy concerns that do not destroy the envelope

`invalid`

- no unique valid envelope can be extracted
- examples:
  - missing markers
  - malformed JSON
  - unsupported version
  - assignment or packet mismatch
  - multiple envelopes

Recommended v1 refinement:

- keep `parse_result` for structural status
- add `WorkerReportValidation.policy_violations[]` and `semantic_issues[]` so Orcas does not overload one enum with every problem

### Raw Output Retention

Raw worker output should always be retained exactly as received.

Why:

- auditability
- recovery review
- future parser improvement without falsifying current truth

But raw output should never become workflow truth by itself.

### Repair Policy

V1 should allow only deterministic repair:

- trim surrounding whitespace
- decode the single envelope between markers
- normalize runtime-driven interruption/lost outcomes into authoritative report semantics

V1 should not allow:

- model-based repair
- transcript summarization into canonical state
- silent reconstruction of missing fields
- “close enough” coercion of an invalid envelope into a valid report

This keeps the protocol honest and aligned with current conservative report parsing.

## Policy Guardrails Outside The Worker Model

The worker model should not be trusted to enforce the protocol alone.

Daemon-side policy should enforce:

- one assignment maps to one immutable communication packet
- one assignment may produce at most one authoritative final report in v1
- worker must stop at the assignment boundary
- worker must not silently continue into the next task
- worker must not create or execute follow-on work not present in the packet
- worker recommendations are never authoritative decisions
- disallowed scope must be explicit and validator-visible
- response must satisfy the packet’s expected mode contract
- assignments without explicit acceptance criteria or stop conditions are rejected before dispatch
- read-only/docs-only/tests-only modes are enforced outside prompt wording

These should be policy checks, not just nice wording in the prompt.

## Connection To Existing Orcas Semantics

### `Assignment`

`Assignment` remains the authoritative execution object.

Recommended evolution:

- add `communication_id`
- deprecate `Assignment.instructions` as canonical truth
- keep a short derived preview/excerpt only if needed for backwards-compatible summaries

### `Report`

`Report` remains the authoritative worker outcome object consumed by the rest of Orcas.

Recommended evolution:

- keep current top-level report summary fields
- add `communication_packet_id`
- keep `raw_output`
- add `response_envelope`
- add `validation`

Important rule:

- proposal generation and decisions should continue to consume `Report`, not raw worker output and not prompt text

### Current Report Parsing

The new parser/validator should replace the current ad hoc report extraction path, but keep the same conservative operational stance:

- retain raw output
- record `parsed` / `ambiguous` / `invalid`
- preserve explicit interrupted/lost semantics
- fail closed when the response cannot be trusted

### `SupervisorProposal`

The proposal layer should not be redesigned here.

Recommended integration:

- proposal generation still begins only after an authoritative `Report` exists
- proposals may later inspect `AssignmentCommunicationPacket` for richer context, but they do not become the communication source of truth
- current `DraftAssignment` remains a proposal artifact
- at approval time, Orcas should compile approved assignment intent into a canonical `AssignmentCommunicationPacket`
- current freeform `compile_assignment_instructions(...)` should become a packet-building bridge, not the terminal worker-prompt representation

This keeps the proposal layer above the communication layer, not parallel to it.

### Human Approval Flow

Human approval remains where it already belongs:

- above `Decision`
- above successor `Assignment`
- unchanged by this communication design

This protocol layer improves the quality of assignment/report objects. It does not remove human review boundaries.

### Future Auto-Proposal / Auto-Synthesis Boundaries

Those boundaries should stay unchanged in v1:

- no proposal without an authoritative report
- no authoritative state mutation from worker response alone
- no next assignment without decision logic and existing approval rules

## Storage, Replay, And Audit

Recommended persisted data per assignment/report boundary:

- canonical `AssignmentCommunicationPacket`
- `PromptRenderArtifact` with template version and exact prompt text
- `packet_hash`
- `prompt_hash`
- raw worker output
- `raw_output_hash`
- extracted `WorkerReportEnvelope` when present
- `WorkerReportValidation`
- authoritative `Report`

Recommended version stamps:

- packet schema version
- prompt template version
- worker report contract version
- worker report envelope version

Recommended hashes:

- hash canonical packet JSON
- hash rendered prompt text
- hash raw worker output

Why keep both packet and rendered prompt:

- the packet is the source of truth
- the prompt is the exact transmission artifact
- later renderer changes must not erase what was actually sent

Visibility recommendation:

- snapshot/event summaries should stay bounded
- full packet/render/raw-output detail should live in focused getter/history paths
- TUI remains a read-only consumer, not a second communication state machine

## Suggested Code Placement

Recommended high-level placement:

- core protocol types:
  - `crates/orcas-core/src/communication.rs`
- collaboration object references:
  - `crates/orcas-core/src/collaboration.rs`
- IPC exposure for later getters/summaries:
  - `crates/orcas-core/src/ipc.rs`
- daemon render/parse/policy logic:
  - `crates/orcas-daemon/src/assignment_comm/mod.rs`
  - `crates/orcas-daemon/src/assignment_comm/render.rs`
  - `crates/orcas-daemon/src/assignment_comm/parse.rs`
  - `crates/orcas-daemon/src/assignment_comm/policy.rs`
- daemon integration points:
  - `crates/orcas-daemon/src/service.rs`
  - later bridge from proposal approval:
    - `crates/orcas-daemon/src/supervisor.rs`

Recommendation on template/spec storage:

- keep v1 template/spec versions in Rust code, not an external general template engine
- one explicit renderer per template version is enough for v1

## Smallest Implementation Slice After This Design

The first build slice should stay narrow and testable.

Recommended slice:

1. Add the new core types:
   - `AssignmentCommunicationRecord`
   - `AssignmentCommunicationPacket`
   - `WorkerReportContract`
   - `WorkerReportEnvelope`
   - `WorkerReportValidation`
   - `AssignmentTaskMode`
2. Persist one communication record per assignment and replace the current `build_worker_prompt(instructions)` path with packet -> render for one real mode:
   - `implement`
3. Keep the current `ORCAS_REPORT_BEGIN` / `ORCAS_REPORT_END` marker style, but upgrade the JSON payload to carry:
   - `schema_version`
   - `packet_id`
   - `task_mode`
   - `acceptance_results`
   - `touched_files`
   - `commands_run`
   - `mode_payload`
4. Materialize the authoritative `Report` from the validated envelope while preserving current `parse_result`, `needs_supervisor_review`, and interrupted/lost behavior.
5. Leave proposal generation, approval, and human decision boundaries unchanged in that first slice.

Recommended test bar for that slice:

- packet persistence round-trip
- deterministic render snapshot for a fixed packet
- parser tests for `parsed`, `ambiguous`, and `invalid`
- fake-runtime assignment-path test proving packet -> prompt -> report recording through the real daemon assignment flow

Do not expand the first slice into:

- full multi-mode support
- new automation
- worker-to-worker handoff
- broad TUI write flows
- proposal redesign

## Closeout

The central v1 design decision is simple:

- Orcas should own structured assignment communication state
- prompt text should be compiled from that state
- worker response should come back through a bounded envelope
- authoritative Orcas report state should be produced only after conservative validation

That keeps the assignment/report boundary aligned with the rest of Orcas:

- explicit protocol objects
- deterministic packaging
- replayable audit trails
- human-supervised control
