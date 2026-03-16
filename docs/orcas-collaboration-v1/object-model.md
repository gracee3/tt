# Orcas Collaboration v1 Object Model

## Object Graph

```text
Human
  -> Workstream
       -> Work Unit
            -> Assignment
                 -> Report
       -> Dependency
       -> Artifact
       -> Decision

Worker
  -> Worker Session
       -> Codex Thread
            -> Codex Turn(s)
```

V1 keeps the object model small. The protocol center is:

- `Workstream`
- `Work Unit`
- `Assignment`
- `Worker`
- `Worker Session`
- `Report`
- `Decision`
- `Artifact`
- `Dependency`

Codex threads and turns remain substrate references, not top-level Orcas workflow objects.

## Core Invariants

- Every `Work Unit` belongs to exactly one `Workstream`.
- Every `Assignment` targets exactly one `Work Unit` and one `Worker`.
- Every `Report` belongs to exactly one `Assignment`.
- Every `Decision` is made by the supervisor and references the report, work unit, or workstream it changes.
- A `Worker Session` is runtime-scoped. It may survive across multiple assignments, but only while Orcas can prove the backing session anchor.
- In v1, coordination between workers is never direct. Shared context moves through supervisor decisions and reassignment.

## Workstream

### What It Is

A `Workstream` is the durable supervisor-owned container for a broader objective.

### Why It Exists

It gives Orcas a stable place to hold:

- the top-level goal
- the current plan
- synthesized understanding across workers
- completion status

Without a workstream, the system collapses back into disconnected worker conversations.

### Candidate Fields

- `id`
- `title`
- `objective`
- `status`
- `priority`
- `created_at`
- `updated_at`
- `created_by`
- `success_criteria[]`
- `constraints[]`
- `summary`
- `active_work_unit_ids[]`
- `completed_work_unit_ids[]`
- `decision_ids[]`
- `artifact_ids[]`

### Major Relationships

- one `Workstream` has many `Work Unit`s
- one `Workstream` has many `Decision`s
- one `Workstream` may aggregate many `Artifact`s

## Work Unit

### What It Is

A `Work Unit` is the smallest bounded piece of work that the supervisor can schedule.

### Why It Exists

Workers need assignments that are narrow enough to execute and report on honestly. The work unit is that stable unit of scheduling and dependency management.

### Candidate Fields

- `id`
- `workstream_id`
- `title`
- `statement`
- `status`
- `priority`
- `acceptance_criteria[]`
- `stop_conditions[]`
- `dependency_ids[]`
- `input_artifact_ids[]`
- `output_artifact_expectations[]`
- `current_assignment_id`
- `attempt_count`
- `blocked_reason`
- `result_summary`

### Major Relationships

- many work units belong to one `Workstream`
- one work unit may have many historical `Assignment`s
- one work unit may have many incoming or outgoing `Dependency` edges

## Assignment

### What It Is

An `Assignment` is the supervisor's explicit dispatch of a work unit to a specific worker.

### Why It Exists

Assignments separate "what needs to be done" from "who is doing it right now." That distinction is necessary for retries, redirects, interruption, and reuse of a worker session.

### Candidate Fields

- `id`
- `workstream_id`
- `work_unit_id`
- `worker_id`
- `worker_session_id`
- `status`
- `brief`
- `constraints[]`
- `input_artifact_ids[]`
- `dependency_snapshot[]`
- `created_at`
- `started_at`
- `ended_at`
- `latest_thread_id`
- `latest_turn_id`
- `stop_reason`
- `report_id`

### Major Relationships

- one assignment targets one `Work Unit`
- one assignment is executed by one `Worker`
- one assignment may use one `Worker Session`
- one assignment may produce zero or one final `Report` in v1

## Worker

### What It Is

A `Worker` is a logical execution actor that can receive assignments.

### Why It Exists

Orcas needs a stable identity for scheduling even when the runtime session changes. "Worker" is that stable identity.

### Candidate Fields

- `id`
- `label`
- `kind`
- `status`
- `capability_tags[]`
- `current_assignment_id`
- `current_session_id`
- `last_report_at`

### Major Relationships

- one worker may have many historical `Assignment`s
- one worker may have many historical `Worker Session`s
- in v1, a worker has at most one active assignment

## Worker Session

### What It Is

A `Worker Session` is the concrete runtime binding used by a worker, typically an Orcas-managed Codex thread plus its known continuity state.

### Why It Exists

It separates logical worker identity from runtime continuity. A worker can persist even when the session is replaced, lost, or restarted.

### Candidate Fields

- `id`
- `worker_id`
- `status`
- `runtime_kind` such as `codex_thread`
- `daemon_instance_id`
- `thread_id`
- `active_turn_id`
- `attachable`
- `resumability`
- `continuity_reason`
- `created_at`
- `last_seen_at`

### Major Relationships

- one worker session belongs to one `Worker`
- one worker session may serve multiple assignments over time
- one worker session points to one primary Codex thread in v1

## Report

### What It Is

A `Report` is the worker's explicit protocol output after bounded execution.

### Why It Exists

The worker must return something more structured than raw stream text. The report is what the supervisor reads, stores, compares, and decides on.

### Candidate Fields

- `id`
- `assignment_id`
- `work_unit_id`
- `worker_id`
- `worker_session_id`
- `submitted_at`
- `disposition`
- `summary`
- `findings[]`
- `artifacts[]`
- `blockers[]`
- `questions[]`
- `recommended_next_actions[]`
- `confidence`
- `source_turn_ids[]`

### Major Relationships

- one report belongs to one `Assignment`
- one report may reference many `Artifact`s
- one or more `Decision`s may cite one report

## Decision

### What It Is

A `Decision` is the supervisor's explicit response to a report or workflow state change.

### Why It Exists

Without an explicit decision object, the most important state transition in the protocol becomes an implicit side effect. Orcas should not hide that transition.

### Candidate Fields

- `id`
- `workstream_id`
- `work_unit_id`
- `report_id`
- `decision_type`
- `rationale`
- `created_at`
- `created_by`
- `follow_up_work_unit_ids[]`
- `follow_up_assignment_ids[]`
- `human_review_required`

### Major Relationships

- one decision is authored by the supervisor
- one decision may accept or reject a report outcome
- one decision may create, close, split, merge, retry, or redirect work units

## Artifact

### What It Is

An `Artifact` is a durable reference to something produced, inspected, or required during work.

### Why It Exists

Supervisor synthesis needs stable references, not just prose. Artifacts give reports and decisions durable anchors.

### Candidate Fields

- `id`
- `kind`
- `locator`
- `description`
- `provenance`
- `producing_assignment_id`
- `observed_at`
- `integrity_hint`

### Major Relationships

- artifacts may be attached to workstreams, work units, reports, or decisions
- artifacts may be input to later assignments

## Dependency

### What It Is

A `Dependency` is an explicit edge between work units.

### Why It Exists

The supervisor-centered model needs a small, explicit way to represent sequencing and blocking. Hidden dependency logic in prompts is not enough.

### Candidate Fields

- `id`
- `from_work_unit_id`
- `to_work_unit_id`
- `kind`
- `condition`
- `status`

### Major Relationships

- dependencies only connect `Work Unit`s in v1
- dependency status influences whether a work unit is `ready` or `blocked`

## Recommended Minimal Enum Shapes

`Workstream.status`

- `draft`
- `active`
- `blocked`
- `completed`
- `abandoned`

`WorkUnit.status`

- `proposed`
- `ready`
- `assigned`
- `in_progress`
- `awaiting_decision`
- `blocked`
- `completed`
- `abandoned`

`Assignment.status`

- `created`
- `dispatched`
- `running`
- `report_submitted`
- `interrupted`
- `lost`
- `closed`

`Worker.status`

- `idle`
- `busy`
- `unavailable`

`WorkerSession.status`

- `created`
- `attached`
- `running`
- `stopped`
- `interrupted`
- `lost`
- `closed`

`Dependency.kind`

- `blocks_on`
- `relates_to`
- `supersedes`
