# Orcas Collaboration v1 Lifecycles

This document defines the main lifecycle cuts and protocol invariants for Orcas collaboration v1.

## Lifecycle Principles

- The supervisor owns workflow state transitions.
- Workers execute bounded assignments and must return control through a report or an interruption outcome.
- Runtime continuity and workflow continuity are related but not identical.
- If runtime evidence is missing, Orcas must degrade to `interrupted`, `lost`, or `unknown` rather than implying continuity.

## Global Invariants

- A work unit has at most one active assignment in v1.
- A worker has at most one active assignment in v1.
- A worker session has at most one active turn at a time in v1.
- A report closes the current worker execution segment and returns control to the supervisor.
- A supervisor decision is required before the same work unit can continue after a report, interruption, or lost continuity.

## Workstream Lifecycle

### States

- `draft`
- `active`
- `blocked`
- `completed`
- `abandoned`

### Transition Rules

`draft -> active`

- the supervisor has defined the objective and at least one work unit exists

`active -> blocked`

- no ready work units remain and at least one blocker requires external resolution

`blocked -> active`

- a supervisor decision or human action resolves the blocker

`active -> completed`

- the success criteria are satisfied and remaining units are completed or intentionally abandoned

`active|blocked -> abandoned`

- the supervisor or human decides the objective is no longer worth pursuing

### Workstream Invariants

- completion is a supervisor conclusion, not a worker claim
- a workstream may remain active while some work units are completed and others are still pending

## Work Unit Lifecycle

### States

- `proposed`
- `ready`
- `assigned`
- `in_progress`
- `awaiting_decision`
- `blocked`
- `completed`
- `abandoned`

### Transition Rules

`proposed -> ready`

- the work unit has a clear statement, acceptance criteria, and no unresolved dependencies

`proposed -> blocked`

- the work unit exists but depends on other work or missing human input

`ready -> assigned`

- the supervisor creates an assignment for a specific worker

`assigned -> in_progress`

- a worker session is attached and execution begins

`in_progress -> awaiting_decision`

- the worker stops and submits a report

`in_progress -> blocked`

- runtime continuity is lost or the assignment hits a blocker before a report can be completed cleanly

`awaiting_decision -> ready`

- the supervisor decides to continue, retry, or redirect the same work unit

`awaiting_decision -> completed`

- the supervisor accepts the outcome as sufficient

`awaiting_decision -> blocked`

- the supervisor escalates a blocker or awaits human input

`awaiting_decision -> abandoned`

- the supervisor decides not to continue the unit

### Work Unit Invariants

- a work unit is the unit of dependency tracking
- a work unit is not complete until the supervisor says it is complete

## Assignment Lifecycle

### States

- `created`
- `dispatched`
- `running`
- `report_submitted`
- `interrupted`
- `lost`
- `closed`

### Transition Rules

`created -> dispatched`

- the supervisor has emitted the assignment brief to a worker

`dispatched -> running`

- Orcas can prove a bound worker session and started execution

`running -> report_submitted`

- the worker returns a structured report

`running -> interrupted`

- the supervisor interrupts, the worker stops, or the runtime returns an interruption outcome

`running -> lost`

- Orcas loses continuity and cannot prove the assignment's active execution state anymore

`report_submitted|interrupted|lost -> closed`

- the supervisor records the next decision and ends this assignment instance

### Assignment Invariants

- an assignment is an execution attempt, not the whole life of a work unit
- `continue` creates a new execution segment and a new assignment for the same work unit after supervisor review; it does not leave one assignment open indefinitely
- retrying or redirecting creates a new assignment even if the worker session is reused

## Worker And Worker Session Lifecycle

## Worker Lifecycle

### States

- `idle`
- `busy`
- `unavailable`

### Rules

- `idle -> busy` when an assignment starts running
- `busy -> idle` when the assignment closes cleanly
- `busy -> unavailable` when the session is lost or the worker is administratively disabled

The worker identity persists across session replacement.

## Worker Session Lifecycle

### States

- `created`
- `attached`
- `running`
- `stopped`
- `interrupted`
- `lost`
- `closed`

### Rules

`created -> attached`

- Orcas binds the session to a concrete runtime anchor such as a Codex thread

`attached -> running`

- an active turn begins for the current assignment

`running -> stopped`

- the worker finishes execution and returns a report

`running -> interrupted`

- the supervisor interrupts or the runtime reports interruption

`running -> lost`

- the current daemon instance cannot prove continuity anymore

`stopped|interrupted -> attached`

- the session remains reusable for a later assignment in the same thread context

`lost -> closed`

- the session is no longer trusted as a continuity anchor

### Worker Session Invariants

- a session is runtime-scoped and evidence-based
- a lost session may still have queryable cached state, but it is not a live continuity anchor

## Report To Decision Loop

The report-to-decision loop is the center of the protocol.

```text
1. Supervisor creates assignment
2. Worker executes bounded work
3. Worker stops and submits report
4. Supervisor reviews report
5. Supervisor records explicit decision
6. Decision changes workstream or work unit state
7. New assignment starts only after that decision
```

### Why This Matters

- it makes control transfer explicit
- it prevents silent continuation after important uncertainty
- it gives the human a clear point to inspect and redirect the workflow

## When Workers Must Stop

Workers are expected to stop and report when:

- the assignment acceptance criteria are satisfied
- the assignment stop condition is reached
- a meaningful blocker requires supervisor input
- the worker has a concrete question that changes the plan
- the worker's confidence is too low to continue honestly
- Orcas interrupts the assignment
- Orcas loses live continuity for the assignment

V1 should bias toward earlier reporting rather than indefinite worker wandering.

## Interruption, Resume, And Redirect

## Interrupt

Interrupt is a supervisor action against an active assignment.

Expected behavior:

1. Orcas sends interruption to the active turn when possible
2. worker session transitions to `interrupted` or `lost`
3. assignment closes after a decision
4. work unit moves to `awaiting_decision` or `blocked`

## Resume

Resume in v1 has two distinct meanings:

- `live resume`: Orcas can still attach to the active turn in the current daemon instance
- `workflow continue`: the supervisor starts a new assignment or a new execution segment after reviewing current state

Only the first case is true runtime continuity. The second case is a new supervisor-directed step and must not be presented as uninterrupted execution.

## Redirect

Redirect means the supervisor changes the assignment brief or scope based on a report or interruption outcome.

In v1, redirect should:

- close the current assignment
- preserve the worker session if still valid
- create a new assignment with revised instructions

That keeps assignment history honest and reviewable.

## Resumability Evidence Rules

These are the minimum honest claims Orcas should make in v1.

### Assignment Is Live-Resumable

Only when all of the following are true:

- the worker session still exists
- the current daemon instance can prove the bound active turn
- `turn/attach` or equivalent session attachment succeeds
- the turn lifecycle is still active

### Worker Session Is Reusable But The Assignment Is Not Live-Resumable

When:

- the backing thread still exists
- Orcas can still bind the worker session to that thread
- but there is no attachable active turn for the interrupted assignment

This is a valid base for a new assignment, retry, or redirected follow-up. It is not proof of uninterrupted execution.

### Neither Session Nor Assignment Is Reliably Resumable

When:

- Orcas only has cached or query-only state
- the daemon instance changed and continuity cannot be re-proven
- the thread or session anchor is missing

At that point the correct workflow outcome is `lost`, `interrupted`, or human review, not silent continuation.

## Example Lifecycle Walkthrough

```text
WS-1 active
  WU-1 ready
  WU-2 ready

Supervisor assigns WU-1 to Worker A
  WU-1 assigned
  A-1 dispatched

Worker session attaches and starts a turn
  WU-1 in_progress
  A-1 running
  Session S-A running

Worker reports blocker
  A-1 report_submitted
  WU-1 awaiting_decision
  Session S-A stopped

Supervisor decides redirect
  A-1 closed
  WU-1 ready
  A-2 created for Worker A with clarified brief
```

The main pattern is: reports and decisions, not free-running chat, are the protocol heartbeat.
