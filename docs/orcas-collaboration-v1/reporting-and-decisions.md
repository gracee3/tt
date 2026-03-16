# Orcas Collaboration v1 Reporting And Decisions

This document proposes the worker report contract and the supervisor decision contract for v1.

## Design Position

Worker output is not complete until it has been turned into a report object.

Supervisor control is not complete until it has been turned into a decision object.

That is the core protocol discipline for v1.

## Worker Report Contract

## Report Purpose

A report is the bounded handoff from worker execution back to the supervisor.

It should answer:

- what happened
- what was learned
- what artifacts matter
- what remains blocked or uncertain
- what the worker recommends next

## Required Top-Level Fields

- `id`
- `assignment_id`
- `work_unit_id`
- `worker_id`
- `worker_session_id`
- `submitted_at`
- `disposition`
- `summary`
- `confidence`

## Recommended Fields

- `findings[]`
- `artifacts[]`
- `blockers[]`
- `questions[]`
- `recommended_next_actions[]`
- `source_turn_ids[]`
- `notes`

## Recommended `disposition` Enum

- `completed`
- `partial`
- `blocked`
- `failed`
- `interrupted`

### Meaning Of Each Disposition

`completed`

- the assignment goals were met closely enough for supervisor review

`partial`

- useful work was completed, but the acceptance target was only partly satisfied

`blocked`

- progress stopped because external information, approval, or decision is required

`failed`

- the worker reached a concrete failure state and cannot recommend continuation without reframing

`interrupted`

- execution stopped because the supervisor interrupted it or runtime continuity was lost

## Findings Shape

Each finding should be small and reviewable.

Recommended fields:

- `kind`
- `summary`
- `evidence[]`
- `severity`
- `confidence`

Typical `kind` values:

- `fact`
- `risk`
- `regression`
- `root_cause`
- `recommendation`

## Artifact Shape

Recommended fields:

- `kind`
- `locator`
- `description`
- `provenance`

Examples:

- repo path
- patch diff reference
- command output reference
- screenshot or log reference
- generated design note

## Blocker Shape

Recommended fields:

- `summary`
- `requires`
- `impact`

Typical `requires` values:

- `supervisor_decision`
- `human_input`
- `runtime_recovery`
- `approval`
- `missing_artifact`

## Question Shape

Recommended fields:

- `summary`
- `why_it_matters`
- `suggested_options[]`

Questions should be specific enough that the supervisor can turn them into a decision.

## Recommended Next Actions

These are proposals from the worker, not self-authorized continuation.

Recommended fields:

- `action`
- `target`
- `reason`

Examples:

- inspect a specific file
- retry with narrowed scope
- ask the human to choose between two behaviors
- split the work unit

## Confidence

V1 should use a simple enum:

- `low`
- `medium`
- `high`

That is enough for routing and review without inventing fake precision.

## Candidate Report Schema

```json
{
  "id": "report-44",
  "assignment_id": "assignment-44",
  "work_unit_id": "wu-12",
  "worker_id": "worker-b",
  "worker_session_id": "session-b-3",
  "submitted_at": "2026-03-16T15:04:00Z",
  "disposition": "blocked",
  "summary": "Reproduction is confirmed, but daemon replacement breaks continuity proof for the active turn.",
  "findings": [
    {
      "kind": "root_cause",
      "summary": "UI currently treats cached turn state as if live continuation were proven.",
      "evidence": ["crates/orcas-supervisor/src/streaming.rs"],
      "severity": "high",
      "confidence": "high"
    }
  ],
  "artifacts": [
    {
      "kind": "repo_path",
      "locator": "crates/orcas-supervisor/src/streaming.rs",
      "description": "Reconnect recovery path",
      "provenance": "observed"
    }
  ],
  "blockers": [
    {
      "summary": "Product decision needed on whether to show recovered terminal state or interrupted stream semantics.",
      "requires": "human_input",
      "impact": "Cannot finalize user-facing behavior honestly."
    }
  ],
  "questions": [
    {
      "summary": "Should Orcas prefer interruption messaging when live continuity is not provable?",
      "why_it_matters": "This choice determines whether the worker should patch recovery UX or preserve current messaging.",
      "suggested_options": [
        "Prefer interruption wording",
        "Prefer recovered cached-state wording"
      ]
    }
  ],
  "recommended_next_actions": [
    {
      "action": "escalate_to_human",
      "target": "wu-12",
      "reason": "Behavior choice is product-facing and not purely technical."
    }
  ],
  "confidence": "high",
  "source_turn_ids": ["turn-901"]
}
```

## Supervisor Decision Contract

## Decision Purpose

A decision is the explicit supervisor response that advances, redirects, pauses, or closes work.

## Required Fields

- `id`
- `decision_type`
- `created_at`
- `created_by`
- `rationale`

## Recommended Context Fields

- `workstream_id`
- `work_unit_id`
- `report_id`
- `assignment_id`
- `follow_up_work_unit_ids[]`
- `follow_up_assignment_ids[]`
- `human_review_required`

## Recommended `decision_type` Enum

- `accept`
- `continue`
- `retry`
- `redirect`
- `split`
- `merge`
- `escalate_to_human`
- `interrupt`
- `abandon`
- `mark_complete`

## Decision Semantics

`accept`

- accept the report as a valid contribution without necessarily closing the workstream

`continue`

- keep the same work unit open and authorize another bounded execution segment

`retry`

- rerun the work unit because the previous assignment failed or lost continuity

`redirect`

- change the brief, constraints, or target of the work unit and create a new assignment

`split`

- replace one work unit with multiple narrower work units

`merge`

- combine multiple work units into one synthesized follow-up unit

`escalate_to_human`

- stop automatic progression until the human reviews or decides

`interrupt`

- stop active work immediately and return control to the supervisor

`abandon`

- close the work unit or workstream without further execution

`mark_complete`

- mark the work unit or workstream complete based on accepted evidence

## Candidate Decision Schema

```json
{
  "id": "decision-44",
  "workstream_id": "ws-1",
  "work_unit_id": "wu-12",
  "report_id": "report-44",
  "assignment_id": "assignment-44",
  "decision_type": "escalate_to_human",
  "rationale": "The worker identified a product-facing ambiguity that should not be auto-resolved.",
  "created_at": "2026-03-16T15:07:00Z",
  "created_by": "supervisor",
  "follow_up_work_unit_ids": [],
  "follow_up_assignment_ids": [],
  "human_review_required": true
}
```

## Report Review Rules

The supervisor should review a report in this order:

1. Is the report structurally valid?
2. What is the disposition?
3. Are there blockers or questions?
4. Are the findings supported by evidence?
5. What decision should be recorded next?

The decision step should be mandatory. A report without a follow-up decision leaves the workflow in limbo.

## Recommended v1 Prompt Discipline

Worker prompts should require the worker to end in report mode, not open-ended prose mode.

The worker should be instructed to:

- execute the bounded assignment
- stop when stop conditions are met
- return a structured report
- avoid self-authorizing further decomposition

That prompt discipline is important even before Orcas has a strict serialized schema validator.

## Report Parsing Strictness

V1 should use a strongly guided report contract, but parse it conservatively.

Recommended rule:

- attempt to parse worker output into Orcas report fields
- accept only the fields that can be extracted confidently
- retain the raw turn output as the audit record
- if parsing is incomplete or ambiguous, mark the result as needing supervisor review rather than pretending the report is fully valid

This keeps the first implementation tractable without weakening protocol honesty.

## Failure And Edge Cases

### If The Worker Streams Useful Output But Never Produces A Clean Report

Orcas may retain the raw turn output, but the assignment should still be treated as incomplete until the supervisor records either:

- a recovery report synthesized from terminal output, or
- an interruption or lost-continuity outcome

### If The Worker Is Interrupted Mid-Assignment

The assignment should produce either:

- an `interrupted` report, if enough structured output exists, or
- an assignment outcome of `interrupted` or `lost` with no report, followed by a supervisor decision

### If The Worker Returns Multiple Candidate Actions

Those remain recommendations. Only the supervisor decision can authorize the next workflow step.

## Minimal v1 Acceptance Bar

V1 does not need a perfect schema engine, but it does need:

- stable report fields
- stable decision types
- persistent storage for both
- explicit transitions driven by those objects
