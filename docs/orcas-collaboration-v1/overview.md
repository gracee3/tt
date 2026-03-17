# Orcas Collaboration v1 Overview

## Related Docs

- [Scope And Non-Goals](./v1-scope.md)
- [Object Model](./object-model.md)
- [Reporting And Decisions](./reporting-and-decisions.md)
- [Assignment Communication Protocol v1](./assignment-communication-protocol-v1.md)
- [Lifecycles](./lifecycles.md)
- [Runtime Mapping](./runtime-mapping.md)
- [Supervisor Proposal v1](./supervisor-proposal-v1.md)
- [Milestone Closeout](./milestone-closeout.md)
- [Next Implementation Cut](./next-implementation-cut.md)

## What Orcas Is At This Layer

Orcas v1 is a supervisor-centered collaboration protocol built on top of the existing Orcas runtime.

At the runtime layer, `orcasd` owns:

- the persistent Codex app-server connection
- local IPC
- live snapshots and event streams
- turn attachment and recovery semantics

At the collaboration layer, Orcas adds:

- first-class workflow objects
- explicit worker assignments
- explicit worker reports
- explicit supervisor decisions
- durable visibility over multi-worker progress

Codex remains the execution substrate. Orcas is the workflow and coordination layer above it.

## Current Milestone Status

The current collaboration and supervisor-proposal milestone is implemented and in a clean stopping state.

Today Orcas already has:

- canonical collaboration state and persistence
- bounded supervisor proposal generation and approval flow
- proposal observability in snapshot, events, history, and read-only TUI surfaces
- opt-in auto-proposal creation on `report_recorded`
- real-path confidence coverage for completed and interrupted runtime outcomes

Current control boundaries remain strict:

- Orcas state is authoritative
- proposals are review artifacts, not workflow truth
- human approval is required before an authoritative `Decision` or successor `Assignment` exists
- auto-proposal is opt-in, conservative, and fail-closed

See [Milestone Closeout](./milestone-closeout.md) for the implemented scope, guarantees, and intentionally deferred follow-up work.

## Why This Is Not Just A Codex Client

A plain Codex client mainly opens threads, sends prompts, and consumes output.

Orcas v1 does more than that:

- tracks human goals as durable workstreams
- decomposes work into bounded work units
- assigns work to specific workers
- requires workers to stop and report
- requires the supervisor to decide what happens next
- preserves a durable control-plane view across concurrent workers

The center of gravity is the supervisor loop, not the individual prompt session.

## Roles

## Human

The human stays above the protocol.

The human is expected to:

- create or approve top-level goals
- review significant blockers, risks, and outcomes
- redirect priorities
- approve abandonment or completion when needed

The human is not expected to micromanage every worker turn.

## Supervisor

The supervisor is the coordination authority inside Orcas.

The supervisor owns:

- workstream creation
- work decomposition
- dependency awareness
- worker assignment
- report review
- synthesis across workers
- explicit decisions

In v1, all worker coordination flows through the supervisor.

## Worker

A worker executes a bounded assignment.

A worker does not own the larger plan. A worker does not autonomously redefine top-level goals. A worker stops when its assignment exit condition is reached, when it hits a blocker, when confidence is too low to continue honestly, or when the supervisor interrupts it.

## Runtime

The runtime is the existing Orcas plus Codex execution substrate:

- `orcasd`
- Orcas IPC
- Codex threads
- Codex turns
- Codex item and event streams

The runtime proves what is live, attachable, cached, lost, or unknown. The collaboration layer must not claim more continuity than the runtime can prove.

## Design Goals

- Supervisor-centered coordination, not peer-to-peer agent swarms
- Bounded worker execution with explicit stop conditions
- Structured reports as first-class protocol objects
- Explicit supervisor decisions as first-class protocol objects
- Durable visibility over concurrent work
- Honest resumability and interruption semantics
- Narrow v1 scope that maps cleanly onto the existing daemon foundation

## Key Constraints

- Orcas already has one durable daemon and one persistent upstream Codex connection
- Orcas already has snapshot-first recovery and conservative turn attachment semantics
- Codex threads and turns are execution primitives, not the Orcas collaboration model
- v1 should support multiple workers, but with simple supervisor-mediated coordination only
- v1 should not assume fully autonomous planning or decomposition

## Direct Answers

### What exactly is a Workstream versus a Work Unit?

A `Workstream` is the supervisor-owned container for a broader objective, context, and evolving synthesis.

A `Work Unit` is the smallest supervisor-schedulable bounded piece of work inside that workstream. Work units are what get assigned to workers.

### What is the difference between a Worker and a Worker Session?

A `Worker` is a logical actor that can receive assignments.

A `Worker Session` is the concrete Codex-backed execution context currently bound to that worker, usually represented by one Orcas-managed thread plus its live or cached runtime evidence.

### When is a worker expected to stop and submit a report?

A worker stops when any of the following happens:

- the assignment exit condition is satisfied
- a blocker requires supervisor or human input
- an explicit uncertainty boundary is reached
- the supervisor interrupts the assignment
- the runtime loses continuity and live progress can no longer be proven

### What types of supervisor decisions exist in v1?

The full collaboration v1 decision universe is:

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

The first model-backed supervisor proposal loop is intentionally narrower. It only proposes:

- `accept`
- `continue`
- `redirect`
- `mark_complete`
- `escalate_to_human`

That narrower slice is defined in [Supervisor Proposal v1](./supervisor-proposal-v1.md). The broader v1 decision universe still exists at the protocol level, but it is not all part of the first proposal-generation implementation cut.

### How should dependencies be represented in v1?

Dependencies are explicit edges between work units. V1 only needs a small dependency model: `blocks_on`, `relates_to`, and `supersedes` are enough to start.

### What runtime evidence is sufficient to say a worker session or assignment is resumable?

For a live in-flight assignment, Orcas must be able to prove an attachable active turn in the current daemon instance.

For a reusable worker session, Orcas must at least be able to prove the backing thread still exists and is still the session anchor. If only cached or terminal state remains, the session may be inspectable, but the interrupted assignment is not live-resumable.

### What should the human be expected to see and control?

The human should be able to see:

- active workstreams and work units
- assignments by worker
- dependencies and blockers
- reports
- supervisor decisions
- runtime continuity state for worker sessions

The human should be able to create, reprioritize, interrupt, redirect, and close work through the supervisor.

### What is the narrowest viable v1 that is still genuinely supervisor-centered?

The narrowest viable v1 is:

- one human
- one supervisor
- multiple workers
- one active workstream
- bounded work units with explicit dependencies
- one active assignment per worker
- structured reports
- explicit supervisor decisions
- supervisor-mediated coordination only

That is enough to be a real collaboration protocol rather than a thin prompt router.

## Worked Example

Objective: stabilize a reconnect regression and prepare a small release note.

```text
Human -> Supervisor:
  "Investigate reconnect regression and prepare release-note text."

Supervisor -> Workstream WS-1:
  Create workstream with success criteria and priority.

Supervisor -> Work Units:
  WU-1: isolate root cause in reconnect flow
  WU-2: inspect TUI impact and user-visible failure mode
  WU-3: draft release-note text after WU-1 and WU-2

Dependency graph:
  WU-3 blocks_on WU-1
  WU-3 blocks_on WU-2

Supervisor -> Worker A:
  Assign WU-1

Supervisor -> Worker B:
  Assign WU-2

Worker A -> Report R-1:
  disposition=completed
  findings=[root cause, affected code path]
  artifacts=[file references]
  recommended_next_actions=[patch direction]

Worker B -> Report R-2:
  disposition=blocked
  blockers=[cannot confirm behavior after daemon replacement]
  questions=[should this path show interrupted or recovered?]

Supervisor -> Decision D-1:
  accept R-1

Supervisor -> Decision D-2:
  escalate_to_human on R-2

Human -> Supervisor:
  "Show interrupted when continuity cannot be proven."

Supervisor -> Decision D-3:
  redirect WU-2 with clarified requirement

Worker B -> Report R-3:
  disposition=completed

Supervisor -> Worker C:
  Assign WU-3 with R-1 and R-3 as inputs

Supervisor -> Decision D-4:
  mark_complete WS-1
```

The important property is that worker outputs do not directly alter global state. The supervisor receives reports, integrates them, and decides what happens next.
