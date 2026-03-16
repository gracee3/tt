# Next Implementation Cut

This document recommends the first implementation slice after the design phase.

## Recommendation

Build the smallest end-to-end supervisor-centered loop first.

That means:

- introduce the collaboration object model in Orcas state
- persist workstreams, work units, assignments, reports, and decisions
- connect one worker assignment flow to one Codex-backed worker session
- require structured reporting before broad multi-worker scheduling

Do not start with autonomous decomposition, worker-to-worker messaging, or broad scheduling heuristics.

## Why This Slice

This slice proves the protocol center:

- supervisor creates work
- worker executes bounded work
- worker returns a report
- supervisor records a decision

If that loop is weak, adding concurrency just creates more unstructured transcripts.

## Recommended Step Order

### 1. Add Orcas-Owned State Objects

Persist a minimal set:

- `Workstream`
- `WorkUnit`
- `Assignment`
- `Report`
- `Decision`
- `Worker`
- `WorkerSession`

The storage can stay lightweight and local, consistent with the current Orcas persistence model.

### 2. Add Narrow IPC And Snapshot Surfaces

Add a small first IPC slice such as:

- `workstreams/create`
- `workstreams/list`
- `workstreams/get`
- `work_units/create`
- `work_units/list`
- `assignments/create`
- `assignments/get`
- `reports/get`
- `decisions/create`

Keep the same Orcas principles:

- narrow methods
- Orcas-owned views
- snapshot-first recovery

### 3. Implement One Worker Session Binding

Support one durable worker session per worker, backed by one Orcas-managed Codex thread.

Required evidence for the session view:

- thread id
- latest turn id
- attachable state
- lost or unknown reason when continuity fails

### 4. Add One Structured Assignment Prompt Path

Implement one path where the supervisor:

1. creates a work unit
2. assigns it to a worker
3. launches a Codex turn with explicit report instructions
4. captures the final structured report

The report capture can start as:

- a strongly guided text contract
- parsed into Orcas report fields
- stored alongside raw turn output for debugging

Parsing should be conservative in the first cut:

- if the report shape is incomplete or ambiguous, keep the raw output
- mark the result as needing supervisor review
- do not silently coerce it into a fully valid report object

### 5. Add Supervisor Decision Recording

Implement a first decision set:

- `accept`
- `continue`
- `redirect`
- `mark_complete`
- `escalate_to_human`

That is enough to validate the loop without implementing every future decision type.

For v1, `continue` should mean a new assignment for the same work unit after supervisor review, not an indefinitely open assignment.

### 6. Add Minimal Visibility In Supervisor CLI And TUI

Expose:

- active workstreams
- work units by status
- active assignments by worker
- latest report summary
- latest supervisor decision
- worker session continuity state

This should be read-oriented first. Fancy interaction can wait.

## Proposed First End-To-End Demo

The first convincing demo should look like this:

1. Human creates one workstream.
2. Supervisor adds two work units, with one dependency.
3. Supervisor assigns the first unit to Worker A.
4. Worker A returns a structured report.
5. Supervisor records `accept`.
6. Supervisor assigns the dependent unit to Worker B with Worker A's report as context.
7. Worker B returns a report.
8. Supervisor records `mark_complete`.

That is modest, but it already demonstrates:

- bounded execution
- durable reporting
- supervisor-mediated sequencing
- explicit dependency handling

## What To Delay Until Later

- broad multi-worker parallel scheduling policies
- worker-to-worker handoff mechanics
- autonomous decomposition
- strict schema validation engine
- rich artifact search and visualization
- advanced TUI controls for every state transition

## Practical Notes For The First Cut

- Reuse the daemon's existing live turn and attachment model rather than inventing a separate continuity system.
- Keep report parsing conservative. If the structure is incomplete, store the raw output and mark the report as needing supervisor review.
- Prefer one active assignment flow that is inspectable and honest over a more ambitious scheduler with weak semantics.

## Success Criteria For The First Cut

The first implementation slice is successful if Orcas can honestly show:

- what work exists
- which worker is doing what
- what report came back
- what decision the supervisor made
- whether the worker session is still resumable or lost
